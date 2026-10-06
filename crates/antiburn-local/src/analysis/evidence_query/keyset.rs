use super::content::{ContentQueryRange, ordered_content_query_sql, query_content_range_keyset};
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CURSOR_REVISION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ContentPosition {
    pub source_key: String,
    pub turn_index: i64,
    pub turn_rowid: i64,
    pub part_index: i64,
}

/// Local continuation state. Do not include this value in provider requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedContentCursor {
    revision: u32,
    input_identity: String,
    position: ContentPosition,
}

pub struct SelectedContentRequest<'a> {
    pub source_generation: i64,
    pub after_ms: Option<i64>,
    pub source_positions: &'a BTreeMap<String, u64>,
    pub selection: JevInputSelection,
    pub cursor: Option<&'a SelectedContentCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedContentPage {
    pub content: PublishedContent,
    pub next_cursor: Option<SelectedContentCursor>,
}

#[derive(Debug)]
pub enum SelectedContentQueryError {
    StaleCursor,
    Database(rusqlite::Error),
}

impl std::fmt::Display for SelectedContentQueryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleCursor => formatter.write_str("selected evidence cursor is stale"),
            Self::Database(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for SelectedContentQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::StaleCursor => None,
            Self::Database(error) => Some(error),
        }
    }
}

impl From<rusqlite::Error> for SelectedContentQueryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

/// Read a bounded selected page at a validated publication. A changed input
/// identity rejects continuation before SQLite reads any content.
/// The caller validates publication freshness in its store transaction and
/// supplies the current source generation. Keep a publication immutable while paging.
pub fn query_turn_content_keyset_selected(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    scope: &FenceScope<'_>,
    request: SelectedContentRequest<'_>,
) -> Result<SelectedContentPage, SelectedContentQueryError> {
    let identity = serde_json::to_vec(&(
        CURSOR_REVISION,
        crate::analysis::PARSER_REVISION,
        crate::analysis::EVIDENCE_SCHEMA_REVISION,
        key.environment_key,
        key.agent,
        key.session_id,
        scope_bind_values(scope),
        request.source_generation,
        request.after_ms,
        request.source_positions,
        request.selection,
    ))
    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let input_identity = format!("{:x}", Sha256::digest(identity));
    if request.cursor.is_some_and(|cursor| {
        cursor.revision != CURSOR_REVISION || cursor.input_identity != input_identity
    }) {
        return Err(SelectedContentQueryError::StaleCursor);
    }
    let (mut content, position) = query_content_range_keyset(
        conn,
        key,
        scope,
        ContentQueryRange {
            after_ms: request.after_ms,
            source_positions: request.source_positions,
            bounded_context: false,
            before_watermark: false,
            offset: 0,
            seek: request.cursor.map(|cursor| &cursor.position),
            selection: request.selection,
        },
        true,
    )?;
    let next_cursor = if content.coverage.more_parts {
        position.map(|position| SelectedContentCursor {
            revision: CURSOR_REVISION,
            input_identity,
            position,
        })
    } else {
        None
    };
    if request.after_ms.is_some() {
        for (before_watermark, seek) in [
            (true, None),
            (false, next_cursor.as_ref().map(|cursor| &cursor.position)),
        ] {
            if !before_watermark && next_cursor.is_none() {
                continue;
            }
            let (context, _) = query_content_range_keyset(
                conn,
                key,
                scope,
                ContentQueryRange {
                    after_ms: request.after_ms,
                    source_positions: request.source_positions,
                    bounded_context: true,
                    before_watermark,
                    offset: 0,
                    seek,
                    selection: request.selection,
                },
                true,
            )?;
            content.coverage.context_capped |=
                context.coverage.context_capped || context.coverage.oversized_parts > 0;
            content.coverage.oversized_parts = content
                .coverage
                .oversized_parts
                .saturating_add(context.coverage.oversized_parts);
            content.coverage.stored_truncated_parts = content
                .coverage
                .stored_truncated_parts
                .saturating_add(context.coverage.stored_truncated_parts);
            content
                .parts
                .extend(context.parts.into_iter().map(|mut part| {
                    part.context_only = true;
                    part
                }));
        }
        content.parts.sort_by(|a, b| {
            (&a.source_key, a.turn_index, a.part_index).cmp(&(
                &b.source_key,
                b.turn_index,
                b.part_index,
            ))
        });
    }
    content.source_generation = Some(request.source_generation);
    Ok(SelectedContentPage {
        content,
        next_cursor,
    })
}

pub(super) fn query_sql(
    recent: bool,
    single_fence: bool,
    seek: bool,
    offset: bool,
    selected: bool,
) -> String {
    let merge_fences = selected && !single_fence;
    let query = ordered_content_query_sql(recent, single_fence || merge_fences);
    let (query, _) = query.split_once("ORDER BY turn.").expect("content order");
    let mut query = query.replace(
        "FROM turn_content AS content",
        ", turn.rowid FROM turn_content AS content",
    );
    if selected {
        query.push_str(" AND content.kind <> 'thinking' AND (content.kind NOT IN ('user', 'assistant') OR content.authority = content.kind) ");
    }
    if seek {
        query.push_str(if recent {
            " AND (turn.turn_index, turn.source_key) <= (?15, ?14)
              AND (turn.turn_index, turn.source_key, turn.rowid, content.part_index) < (?15, ?14, ?16, ?17) "
        } else {
            " AND (turn.source_key, turn.turn_index) >= (?14, ?15)
              AND (turn.source_key, turn.turn_index, turn.rowid, content.part_index) > (?14, ?15, ?16, ?17) "
        });
    } else {
        query.push_str(" AND ?14 IS NULL AND ?15 IS NULL AND ?16 IS NULL AND ?17 IS NULL ");
    }
    if !offset {
        query.push_str(" AND ?12 = 0 ");
    }
    if merge_fences {
        let published = query.replace("AND turn.claim_fence = ?4", "AND turn.claim_fence = ?5 AND ?5 <> ?4 AND EXISTS (SELECT 1 FROM json_each(?6) WHERE value = turn.source_key)");
        query.push_str(" UNION ALL ");
        query.push_str(&published);
        query.push_str(if recent {
            "ORDER BY 3 DESC, 1 DESC, 27 DESC, 9 DESC LIMIT ?7"
        } else {
            "ORDER BY 1, 3, 27, 9 LIMIT ?7"
        });
    } else {
        query.push_str(if recent {
            "ORDER BY turn.turn_index DESC, turn.source_key DESC, turn.rowid DESC, content.part_index DESC LIMIT ?7"
        } else {
            "ORDER BY turn.source_key, turn.turn_index, turn.rowid, content.part_index LIMIT ?7"
        });
    }
    if offset {
        query.push_str(" OFFSET ?12");
    }
    query
}

#[cfg(test)]
mod tests {
    use super::super::tests::{KEY, base_row, test_connection};
    use super::*;
    use crate::analysis::rows::insert_turn_rows;

    fn seed(conn: &Connection, count: u64) {
        let rows = (0..count)
            .map(|index| base_row("source", index))
            .collect::<Vec<_>>();
        insert_turn_rows(conn, &KEY, 1, &rows).unwrap();
        conn.execute("INSERT INTO turn_content (turn_rowid, part_index, kind, content, truncated, authority)
            SELECT rowid, 0, 'assistant', CAST('synthetic evidence' AS BLOB), 0, 'assistant' FROM turn", []).unwrap();
    }

    fn page(
        conn: &Connection,
        cursor: Option<&SelectedContentCursor>,
        recent: bool,
    ) -> SelectedContentPage {
        scoped_page(conn, &FenceScope::single(1), cursor, recent)
    }

    fn scoped_page(
        conn: &Connection,
        scope: &FenceScope<'_>,
        cursor: Option<&SelectedContentCursor>,
        recent: bool,
    ) -> SelectedContentPage {
        query_turn_content_keyset_selected(
            conn,
            &KEY,
            scope,
            SelectedContentRequest {
                source_generation: 7,
                after_ms: recent.then_some(0),
                source_positions: &BTreeMap::from([("*".to_owned(), 0)]),
                selection: JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
                cursor,
            },
        )
        .unwrap()
    }

    #[test]
    fn keyset_persisted_cursor_survives_deletion_and_excludes_new_fences() {
        let conn = test_connection();
        seed(&conn, 600);
        let first = page(&conn, None, false);
        let json = serde_json::to_string(first.next_cursor.as_ref().unwrap()).unwrap();
        conn.execute_batch("CREATE TABLE saved_progress (cursor TEXT NOT NULL)")
            .unwrap();
        conn.execute("INSERT INTO saved_progress VALUES (?1)", [&json])
            .unwrap();
        let saved: String = conn
            .query_row("SELECT cursor FROM saved_progress", [], |row| row.get(0))
            .unwrap();
        let cursor: SelectedContentCursor = serde_json::from_str(&saved).unwrap();
        conn.execute("DELETE FROM turn WHERE turn_index < 100", [])
            .unwrap();
        insert_turn_rows(&conn, &KEY, 2, &[base_row("source", 600)]).unwrap();
        let next = page(&conn, Some(&cursor), false);
        assert_eq!(next.content.parts[0].turn_index, 256);
        let tail = page(&conn, next.next_cursor.as_ref(), false);
        assert_eq!(tail.content.parts.len(), 88);
        assert_eq!(tail.content.parts.last().unwrap().turn_index, 599);
        assert!(tail.next_cursor.is_none());
        assert_eq!(tail.content.source_generation, Some(7));
    }

    #[test]
    fn keyset_cursor_invalidates_changed_contracts_and_legacy_offsets() {
        let conn = test_connection();
        seed(&conn, 300);
        let cursor = page(&conn, None, false).next_cursor.unwrap();
        assert!(serde_json::from_str::<SelectedContentCursor>("{\"offset\":256}").is_err());
        for change in 0..10 {
            let mut changed = cursor.clone();
            if change == 0 {
                changed.revision += 1;
            }
            let positions = BTreeMap::from([("*".to_owned(), if change == 6 { 1 } else { 0 })]);
            if change == 1 || change == 9 {
                let prior_identity = serde_json::to_vec(&(
                    CURSOR_REVISION,
                    crate::analysis::PARSER_REVISION - i64::from(change == 1),
                    crate::analysis::EVIDENCE_SCHEMA_REVISION - i64::from(change == 9),
                    KEY.environment_key,
                    KEY.agent,
                    KEY.session_id,
                    scope_bind_values(&FenceScope::single(1)),
                    7_i64,
                    Option::<i64>::None,
                    &positions,
                    JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
                ))
                .unwrap();
                changed.input_identity = format!("{:x}", Sha256::digest(prior_identity));
            }
            let sources = vec!["source".to_owned()];
            let scope = FenceScope {
                claim_fence: if change == 2 { 2 } else { 1 },
                published: (change == 8).then_some(PublishedScope {
                    fence: 0,
                    source_keys: &sources,
                }),
            };
            let key = TurnSessionKey {
                session_id: if change == 7 { "other" } else { "s1" },
                ..KEY
            };
            assert!(
                matches!(
                    query_turn_content_keyset_selected(
                        &conn,
                        &key,
                        &scope,
                        SelectedContentRequest {
                            source_generation: if change == 3 { 8 } else { 7 },
                            after_ms: (change == 4).then_some(0),
                            source_positions: &positions,
                            selection: if change == 5 {
                                JevInputSelection::ALL
                            } else {
                                JevInputSelection::from_fields(&[JevInputField::AssistantMessage])
                            },
                            cursor: Some(&changed),
                        }
                    ),
                    Err(SelectedContentQueryError::StaleCursor)
                ),
                "change {change}"
            );
        }
    }

    #[test]
    fn keyset_visits_source_turn_row_and_part_ties_in_both_orders() {
        let conn = test_connection();
        let mut rows = Vec::new();
        for source in ["a", "b"] {
            for _ in 0..90 {
                let mut row = base_row("source", 1);
                row.source_key = source.to_owned();
                row.ts_ms = Some(1);
                rows.push(row);
            }
        }
        insert_turn_rows(&conn, &KEY, 1, &rows).unwrap();
        conn.execute("INSERT INTO turn_content (turn_rowid, part_index, kind, content, truncated, authority)
            SELECT rowid, value, 'assistant', CAST(rowid || ':' || value AS BLOB), 0, 'assistant' FROM turn, json_each('[0,1,2]')", []).unwrap();
        for recent in [false, true] {
            let mut cursor = None;
            let mut seen = Vec::new();
            loop {
                let next = page(&conn, cursor.as_ref(), recent);
                seen.extend(
                    next.content
                        .parts
                        .iter()
                        .filter(|part| !part.context_only)
                        .map(|part| part.part.text.clone()),
                );
                cursor = next.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
            assert_eq!(seen.len(), 540);
            assert_eq!(seen.iter().collect::<BTreeSet<_>>().len(), 540);
            if !recent {
                assert_eq!(seen[0], "1:0");
                assert_eq!(seen[539], "180:2");
            }
        }
    }

    #[test]
    fn keyset_byte_cut_and_oversized_skip_do_not_lose_evidence() {
        let conn = test_connection();
        seed(&conn, 7);
        conn.execute(
            "UPDATE turn_content SET content = zeroblob(262144) WHERE turn_rowid <= 5",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE turn_content SET content = zeroblob(262145) WHERE turn_rowid = 1",
            [],
        )
        .unwrap();
        let first = page(&conn, None, false);
        assert_eq!(first.content.coverage.oversized_parts, 1);
        assert_eq!(first.content.parts.len(), 4);
        assert!(first.content.coverage.bytes_capped);
        assert_eq!(first.next_cursor.as_ref().unwrap().position.turn_index, 4);
        let next = page(&conn, first.next_cursor.as_ref(), false);
        assert_eq!(
            next.content
                .parts
                .iter()
                .map(|part| part.turn_index)
                .collect::<Vec<_>>(),
            [5, 6]
        );
    }

    #[test]
    fn keyset_selection_skips_private_bodies_and_bounds_context() {
        let conn = test_connection();
        seed(&conn, 700);
        conn.execute("UPDATE turn SET ts_ms = turn_index", [])
            .unwrap();
        conn.execute("INSERT INTO turn_content (turn_rowid, part_index, kind, content, truncated, authority, tool_name)
            SELECT rowid, 1, 'tool_result', zeroblob(262145), 0, 'tool', 'bash' FROM turn WHERE rowid <= 32", []).unwrap();
        conn.execute(
            "INSERT INTO turn_content (turn_rowid, part_index, kind, content, truncated, authority)
            SELECT rowid, 2, 'thinking', CAST('private thinking sentinel' AS BLOB), 0, 'assistant' FROM turn",
            [],
        )
        .unwrap();
        let request = |selection| {
            query_turn_content_keyset_selected(
                &conn,
                &KEY,
                &FenceScope::single(1),
                SelectedContentRequest {
                    source_generation: 7,
                    after_ms: Some(100),
                    source_positions: &BTreeMap::new(),
                    selection,
                    cursor: None,
                },
            )
            .unwrap()
        };
        let result = request(JevInputSelection::from_fields(&[
            JevInputField::AssistantMessage,
        ]));
        assert_eq!(result.content.parts.len(), 288);
        assert_eq!(result.content.coverage.oversized_parts, 0);
        assert!(
            result
                .content
                .parts
                .iter()
                .all(|part| part.part.kind == ContentKind::AssistantText)
        );
        assert!(result.content.coverage.context_capped);
        assert!(
            request(JevInputSelection::ALL)
                .content
                .parts
                .iter()
                .all(|part| part.part.kind != ContentKind::Thinking)
        );
        assert!(request(JevInputSelection::NONE).content.parts.is_empty());
    }

    #[test]
    #[ignore = "offline release query benchmark"]
    fn benchmark_keyset_queries() {
        let conn = test_connection();
        seed(&conn, 32768);
        conn.execute("UPDATE turn SET ts_ms = turn_index", [])
            .unwrap();
        let sources = vec!["source".to_owned()];
        for mixed in [false, true] {
            if mixed {
                conn.execute(
                    "UPDATE turn SET claim_fence = 2 WHERE turn_index % 2 = 0",
                    [],
                )
                .unwrap();
            }
            let scope = FenceScope {
                claim_fence: if mixed { 2 } else { 1 },
                published: mixed.then_some(PublishedScope {
                    fence: 1,
                    source_keys: &sources,
                }),
            };
            for recent in [false, true] {
                for start in [0, 16384, 32512] {
                    let mut cursor = scoped_page(&conn, &scope, None, recent)
                        .next_cursor
                        .unwrap();
                    cursor.position = ContentPosition {
                        source_key: "source".to_owned(),
                        turn_index: if recent { 32768 - start } else { start - 1 },
                        turn_rowid: if recent { 32769 - start } else { start },
                        part_index: 0,
                    };
                    let sql = query_sql(recent, !mixed, true, false, true);
                    assert!(!sql.contains("OFFSET"));
                    let plan = conn
                        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                        .unwrap()
                        .query_map(
                            params![
                                "native",
                                "claude",
                                "s1",
                                scope.claim_fence,
                                1,
                                if mixed { "[\"source\"]" } else { "[]" },
                                257,
                                262144,
                                recent.then_some(0),
                                "{}",
                                0,
                                0,
                                2,
                                cursor.position.source_key,
                                cursor.position.turn_index,
                                cursor.position.turn_rowid,
                                cursor.position.part_index,
                            ],
                            |row| row.get::<_, String>(3),
                        )
                        .unwrap()
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .unwrap();
                    assert!(plan.iter().any(|line| line.contains(if recent {
                        "turn_content_recent_page"
                    } else {
                        "turn_content_source_page"
                    })));
                    assert!(
                        !plan
                            .iter()
                            .any(|line| line == "USE TEMP B-TREE FOR ORDER BY"),
                        "{plan:?}"
                    );
                    println!(
                        "keyset mixed={mixed} recent={recent} start={start} plan={}",
                        plan.join("; ")
                    );
                    scoped_page(&conn, &scope, Some(&cursor), recent);
                    let mut samples = Vec::new();
                    for _ in 0..7 {
                        let clock = std::time::Instant::now();
                        for _ in 0..10 {
                            let result = scoped_page(&conn, &scope, Some(&cursor), recent);
                            assert_eq!(
                                result
                                    .content
                                    .parts
                                    .iter()
                                    .filter(|part| !part.context_only)
                                    .count(),
                                256
                            );
                        }
                        samples.push(clock.elapsed().as_micros());
                    }
                    samples.sort();
                    println!(
                        "keyset rows=32768 mixed={mixed} recent={recent} start={start} runs=10 samples=7 median_us={} min_us={} max_us={}",
                        samples[3], samples[0], samples[6]
                    );
                }
            }
        }
    }

    #[test]
    fn keyset_retrieves_selected_outputs_across_resumed_source_fences() {
        let conn = test_connection();
        for (source, fence) in [("resumed", 1), ("other", 1), ("new", 2)] {
            let rows = (0..300)
                .map(|index| {
                    let mut row = base_row(source, index);
                    row.ts_ms = None;
                    row.content = vec![
                        ContentPart::new(ContentKind::ToolResult, format!("{source}:{index}"))
                            .with_tool_identity(
                                Some("bash".to_owned()),
                                Some(format!("call-{index}")),
                            ),
                    ];
                    row
                })
                .collect::<Vec<_>>();
            insert_turn_rows(&conn, &KEY, fence, &rows).unwrap();
        }
        let resumed = vec!["resumed".to_owned()];
        let scope = FenceScope {
            claim_fence: 2,
            published: Some(PublishedScope {
                fence: 1,
                source_keys: &resumed,
            }),
        };
        let positions = BTreeMap::from([("*".to_owned(), 10)]);
        let mut cursor = None;
        let mut seen = BTreeSet::new();
        loop {
            let next = query_turn_content_keyset_selected(
                &conn,
                &KEY,
                &scope,
                SelectedContentRequest {
                    source_generation: 7,
                    after_ms: Some(1000),
                    source_positions: &positions,
                    selection: JevInputSelection::from_fields(&[JevInputField::BashCommandOutput]),
                    cursor: cursor.as_ref(),
                },
            )
            .unwrap();
            for part in next.content.parts.iter().filter(|part| !part.context_only) {
                assert!(part.turn_index > 10);
                assert_ne!(part.source_key, "other");
                assert!(seen.insert(part.part.text.clone()));
            }
            cursor = next.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(seen.len(), 578);
        assert!(seen.contains("new:299") && seen.contains("resumed:11"));
    }

    #[test]
    fn keyset_empty_oversized_page_advances_to_later_selected_fields() {
        let conn = test_connection();
        seed(&conn, 257);
        conn.execute(
            "UPDATE turn_content SET content = zeroblob(262145) WHERE turn_rowid <= 256",
            [],
        )
        .unwrap();
        let first = page(&conn, None, false);
        assert!(first.content.parts.is_empty());
        assert_eq!(first.content.coverage.oversized_parts, 256);
        assert_eq!(first.next_cursor.as_ref().unwrap().position.turn_index, 255);
        let tail = page(&conn, first.next_cursor.as_ref(), false);
        assert_eq!(tail.content.parts[0].turn_index, 256);
        assert!(tail.next_cursor.is_none());
    }

    #[test]
    fn keyset_context_reports_skipped_and_truncated_selected_evidence() {
        let conn = test_connection();
        seed(&conn, 3);
        conn.execute(
            "UPDATE turn_content SET content = zeroblob(262145) WHERE turn_rowid = 1",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE turn_content SET truncated = 1 WHERE turn_rowid = 2",
            [],
        )
        .unwrap();
        let result = query_turn_content_keyset_selected(
            &conn,
            &KEY,
            &FenceScope::single(1),
            SelectedContentRequest {
                source_generation: 7,
                after_ms: Some(1002),
                source_positions: &BTreeMap::new(),
                selection: JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
                cursor: None,
            },
        )
        .unwrap();
        assert_eq!(result.content.parts.len(), 2);
        assert_eq!(result.content.coverage.oversized_parts, 1);
        assert_eq!(result.content.coverage.stored_truncated_parts, 1);
        assert!(result.content.coverage.context_capped);
        assert!(
            result
                .content
                .parts
                .iter()
                .any(|part| part.context_only && part.part.truncated)
        );
    }
}
