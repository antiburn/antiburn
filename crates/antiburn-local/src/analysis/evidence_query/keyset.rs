use super::content::query_content_range_keyset_scoped;
use super::content::{ContentQueryRange, content_query_sql, fence_predicate, order_fragment};
use super::*;
use crate::checks::ignored_instructions::sha256_hex;
use serde::{Deserialize, Serialize};

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
    query_selected_page(conn, key, scope, request, None)
}

/// Validate an ordinary activity boundary without selecting its body. Scope
/// filters also bind the cursor and run before content coverage accounting.
pub fn query_turn_content_keyset_scoped(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    fence: &FenceScope<'_>,
    request: SelectedContentRequest<'_>,
    content_scope: &SelectedContentScope,
) -> Result<ScopedSelectedContentPage, SelectedContentQueryError> {
    let boundary =
        super::scope::query_boundary(conn, key, fence, content_scope, request.source_generation)?;
    let page = query_selected_page(conn, key, fence, request, Some(content_scope))?;
    Ok(ScopedSelectedContentPage { page, boundary })
}

/// Read one cited position with the same selected projection and size limits as a page.
pub fn query_turn_content_exact_selected(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    fence: &FenceScope<'_>,
    source_generation: i64,
    selection: JevInputSelection,
    content_scope: &SelectedContentScope,
) -> Result<PublishedContent, SelectedContentQueryError> {
    let sql = exact_scope_query_sql(fence.published.is_none());
    let (mut content, _) = super::content::query_content_range_with_sql(
        conn,
        key,
        fence,
        ContentQueryRange {
            after_ms: None,
            source_positions: &BTreeMap::new(),
            bounded_context: false,
            before_watermark: false,
            offset: 0,
            seek: None,
            selection,
        },
        true,
        Some(content_scope),
        Some(&sql),
    )?;
    content.source_generation = Some(source_generation);
    Ok(content)
}

fn exact_scope_query_sql(single_fence: bool) -> String {
    query_sql_with_scope(
        false,
        single_fence,
        false,
        false,
        true,
        "turn.source_key = json_extract(:content_scope, '$.source_key')
         AND turn.thread_id = json_extract(:content_scope, '$.thread_id')
         AND turn.scope = 'main'
         AND turn.turn_index = json_extract(:content_scope, '$.turn_index')
         AND content.part_index = json_extract(:content_scope, '$.part_index')
         AND (json_type(:content_scope, '$.native_record_ids') = 'null' OR
             COALESCE(turn.uuid, turn.message_id) IN (
                 SELECT value FROM json_each(:content_scope, '$.native_record_ids')))",
    )
}

fn query_selected_page(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    scope: &FenceScope<'_>,
    request: SelectedContentRequest<'_>,
    content_scope: Option<&SelectedContentScope>,
) -> Result<SelectedContentPage, SelectedContentQueryError> {
    let mut identity = serde_json::to_vec(&(
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
    if let Some(filter) = content_scope {
        identity.extend(
            serde_json::to_vec(filter)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
        );
    }
    let input_identity = sha256_hex(&identity);
    if request.cursor.is_some_and(|cursor| {
        cursor.revision != CURSOR_REVISION || cursor.input_identity != input_identity
    }) {
        return Err(SelectedContentQueryError::StaleCursor);
    }
    let (mut content, position) = query_content_range_keyset_scoped(
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
        content_scope,
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
            let (context, _) = query_content_range_keyset_scoped(
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
                content_scope,
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
    query_sql_with_scope(
        recent,
        single_fence,
        seek,
        offset,
        selected,
        super::scope::SCOPE_PREDICATE,
    )
}

fn query_sql_with_scope(
    recent: bool,
    single_fence: bool,
    seek: bool,
    offset: bool,
    selected: bool,
    scope_predicate: &str,
) -> String {
    let merge_fences = selected && !single_fence;
    let mut query = content_query_sql(
        if merge_fences {
            fence_predicate(true)
        } else {
            fence_predicate(single_fence)
        },
        true,
        scope_predicate,
    );
    if selected {
        query.push_str(" AND content.kind <> 'thinking' AND (
            content.kind NOT IN ('user', 'assistant') OR content.authority = content.kind
            OR (content.kind = 'user' AND content.authority = 'unknown' AND CASE
                WHEN length(CAST(content.normalized_fields_json AS BLOB)) > 131072 THEN 1
                ELSE json_type(content.normalized_fields_json, '$.metadata.selected_skill') = 'object'
                    OR json_type(content.normalized_fields_json, '$.metadata.recorded_skill_result') = 'object'
                    OR json_type(content.normalized_fields_json, '$.metadata.non_authorizing_context_proof') = 'object'
                    OR json_type(content.normalized_fields_json, '$.metadata.non_authorizing_context') = 'object'
                END)) ");
    }
    let seek_fragment = if seek {
        if recent {
            " AND (turn.turn_index, turn.source_key) <= (:seek_turn_index, :seek_source_key)
              AND (turn.turn_index, turn.source_key, turn.rowid, content.part_index) < (:seek_turn_index, :seek_source_key, :seek_turn_rowid, :seek_part_index) "
        } else {
            " AND (turn.source_key, turn.turn_index) >= (:seek_source_key, :seek_turn_index)
              AND (turn.source_key, turn.turn_index, turn.rowid, content.part_index) > (:seek_source_key, :seek_turn_index, :seek_turn_rowid, :seek_part_index) "
        }
    } else {
        " "
    };
    query.push_str(seek_fragment);
    if merge_fences {
        let published_predicate =
            "turn.claim_fence = :published_fence AND :published_fence <> :claim_fence AND EXISTS
            (SELECT 1 FROM json_each(:source_keys) WHERE value = turn.source_key)";
        let mut published = content_query_sql(published_predicate, true, scope_predicate);
        if selected {
            published.push_str(" AND content.kind <> 'thinking' AND (
                content.kind NOT IN ('user', 'assistant') OR content.authority = content.kind
                OR (content.kind = 'user' AND content.authority = 'unknown' AND CASE
                    WHEN length(CAST(content.normalized_fields_json AS BLOB)) > 131072 THEN 1
                    ELSE json_type(content.normalized_fields_json, '$.metadata.selected_skill') = 'object'
                        OR json_type(content.normalized_fields_json, '$.metadata.recorded_skill_result') = 'object'
                        OR json_type(content.normalized_fields_json, '$.metadata.non_authorizing_context_proof') = 'object'
                        OR json_type(content.normalized_fields_json, '$.metadata.non_authorizing_context') = 'object'
                    END)) ");
        }
        published.push_str(seek_fragment);
        query.push_str(" UNION ALL ");
        query.push_str(&published);
        query.push_str(order_fragment(recent, false));
        query.push_str(" LIMIT :limit");
    } else {
        query.push_str(order_fragment(recent, true));
        query.push_str(" LIMIT :limit");
    }
    if offset {
        query.push_str(" OFFSET :offset");
    }
    query
}

#[cfg(test)]
mod tests {
    use super::super::content::query_content_range_keyset;
    use super::super::tests::{KEY, base_row, test_connection};
    use super::*;
    use crate::analysis::interface::MAX_CONTENT_PART_BYTES;
    use crate::analysis::rows::insert_turn_rows;

    fn seed(conn: &Connection, count: u64) {
        let rows = (0..count)
            .map(|index| base_row("source", index))
            .collect::<Vec<_>>();
        insert_turn_rows(conn, &KEY, 1, &rows).unwrap();
        conn.execute("INSERT INTO turn_content (turn_rowid, part_index, kind, content, truncated, authority)
            SELECT rowid, 0, 'assistant', CAST('synthetic evidence' AS BLOB), 0, 'assistant' FROM turn", []).unwrap();
    }

    #[test]
    fn exact_scope_uses_source_position_and_part_indexes_for_both_fences() {
        let conn = test_connection();
        seed(&conn, 5000);
        conn.execute("UPDATE turn SET uuid = 'repeated-native-id'", [])
            .unwrap();
        conn.execute(
            "INSERT INTO turn_content (turn_rowid, part_index, kind, content, truncated, authority)
             SELECT rowid, 300, 'assistant', CAST('cited part' AS BLOB), 0, 'assistant'
             FROM turn WHERE turn_index = 1",
            [],
        )
        .unwrap();
        let content_scope = SelectedContentScope {
            source_key: "source".into(),
            thread_id: "source".into(),
            turn_index: 1,
            part_index: 300,
            native_record_ids: Some(BTreeSet::from(["repeated-native-id".into()])),
        };
        let scope_json = serde_json::to_string(&content_scope).unwrap();
        let sources = vec!["source".to_owned()];
        for mixed in [false, true] {
            let fence = FenceScope {
                claim_fence: if mixed { 2 } else { 1 },
                published: mixed.then_some(PublishedScope {
                    fence: 1,
                    source_keys: &sources,
                }),
            };
            let sql = exact_scope_query_sql(!mixed);
            let mut bindings = rusqlite::named_params! {
                ":environment_key": KEY.environment_key,
                ":agent": KEY.agent,
                ":session_id": KEY.session_id,
                ":claim_fence": fence.claim_fence,
                ":limit": 257,
                ":max_part_bytes": MAX_CONTENT_PART_BYTES as i64,
                ":after_ms": Option::<i64>::None,
                ":source_positions": "{}",
                ":before_watermark": 0,
                ":selection": 2,
                ":content_scope": scope_json,
            }
            .to_vec();
            if mixed {
                bindings.extend_from_slice(rusqlite::named_params! {
                    ":published_fence": 1,
                    ":source_keys": "[\"source\"]",
                });
            }
            let plan = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(bindings.as_slice(), |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let branches = if mixed { 2 } else { 1 };
            assert_eq!(
                plan.iter()
                    .filter(|line| {
                        (line.contains("SEARCH turn USING INDEX turn_content_source_page")
                            && line.contains("source_key=? AND turn_index=?"))
                            || (line.contains("SEARCH turn USING INDEX turn_content_recent_page")
                                && line.contains("turn_index=? AND source_key=?"))
                    })
                    .count(),
                branches,
                "{plan:?}"
            );
            assert_eq!(
                plan.iter()
                    .filter(|line| {
                        line.contains("SEARCH content USING PRIMARY KEY")
                            && line.contains("turn_rowid=? AND part_index=?")
                    })
                    .count(),
                branches,
                "{plan:?}"
            );
            assert!(
                !plan
                    .iter()
                    .any(|line| line.starts_with("SCAN turn") || line.starts_with("SCAN content")),
                "{plan:?}"
            );
            let content = query_turn_content_exact_selected(
                &conn,
                &KEY,
                &fence,
                7,
                JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
                &content_scope,
            )
            .unwrap();
            assert_eq!(content.parts.len(), 1);
            assert_eq!(content.parts[0].turn_index, 1);
            assert_eq!(content.parts[0].part_index, 300);
            assert_eq!(content.parts[0].part.text, "cited part");
            assert_eq!(content.publication_fence, fence.claim_fence);
            assert_eq!(content.source_generation, Some(7));
        }
    }

    #[test]
    fn exact_scope_preserves_identity_authority_selection_and_size_limits() {
        let conn = test_connection();
        seed(&conn, 2);
        conn.execute("UPDATE turn SET uuid = 'native-id'", [])
            .unwrap();
        let scope = SelectedContentScope {
            source_key: "source".into(),
            thread_id: "source".into(),
            turn_index: 1,
            part_index: 0,
            native_record_ids: Some(BTreeSet::from(["native-id".into()])),
        };
        let read = |scope: &SelectedContentScope, fence, selection| {
            query_turn_content_exact_selected(
                &conn,
                &KEY,
                &FenceScope::single(fence),
                7,
                selection,
                scope,
            )
            .unwrap()
        };
        let selection = JevInputSelection::from_fields(&[JevInputField::AssistantMessage]);
        assert_eq!(read(&scope, 1, selection).parts.len(), 1);
        assert!(read(&scope, 2, selection).parts.is_empty());
        assert!(read(&scope, 1, JevInputSelection::NONE).parts.is_empty());
        for field in 0..5 {
            let mut wrong = scope.clone();
            match field {
                0 => wrong.source_key = "other-source".into(),
                1 => wrong.thread_id = "other-thread".into(),
                2 => wrong.turn_index = 2,
                3 => wrong.part_index = 1,
                _ => wrong.native_record_ids = Some(BTreeSet::from(["other-record".into()])),
            }
            assert!(read(&wrong, 1, selection).parts.is_empty());
        }
        let denied_sources = vec!["other-source".to_owned()];
        let denied_fence = FenceScope {
            claim_fence: 2,
            published: Some(PublishedScope {
                fence: 1,
                source_keys: &denied_sources,
            }),
        };
        assert!(
            query_turn_content_exact_selected(&conn, &KEY, &denied_fence, 7, selection, &scope)
                .unwrap()
                .parts
                .is_empty()
        );
        conn.execute(
            "UPDATE turn SET scope = 'delegated' WHERE turn_index = 1",
            [],
        )
        .unwrap();
        assert!(read(&scope, 1, selection).parts.is_empty());
        conn.execute("UPDATE turn SET scope = 'main'", []).unwrap();
        for (kind, authority) in [("thinking", "assistant"), ("assistant", "unknown")] {
            conn.execute(
                "UPDATE turn_content SET kind = ?1, authority = ?2",
                params![kind, authority],
            )
            .unwrap();
            assert!(read(&scope, 1, JevInputSelection::ALL).parts.is_empty());
        }
        conn.execute("UPDATE turn_content SET kind = 'assistant', authority = 'assistant', content = zeroblob(?1)", params![MAX_CONTENT_PART_BYTES + 1]).unwrap();
        let oversized = read(&scope, 1, selection);
        assert!(oversized.parts.is_empty());
        assert_eq!(oversized.coverage.oversized_parts, 1);
    }

    #[test]
    fn exact_scope_reuses_selected_tool_projection_without_loading_excluded_fields() {
        let conn = test_connection();
        let mut row = base_row("source", 1);
        row.uuid = Some("native-id".into());
        let mut part = ContentPart::new(ContentKind::ToolInput, "excluded raw input")
            .with_tool_identity(Some("bash".into()), Some("call-id".into()));
        part.normalized_fields = Some(JevNormalizedFields {
            category: Some(JevNormalizedCategory::BashCommand),
            values: BTreeMap::from([
                (JevInputField::BashCommandInput, "cargo test".into()),
                (
                    JevInputField::FileEditContent,
                    "excluded field".repeat(MAX_CONTENT_PART_BYTES),
                ),
            ]),
            malformed: false,
        });
        row.content.push(part);
        insert_turn_rows(&conn, &KEY, 1, &[row]).unwrap();
        let scope = SelectedContentScope {
            source_key: "source".into(),
            thread_id: "source".into(),
            turn_index: 1,
            part_index: 0,
            native_record_ids: Some(BTreeSet::from(["native-id".into()])),
        };
        let selection = JevInputSelection::from_fields(&[JevInputField::BashCommandInput]);
        let exact = query_turn_content_exact_selected(
            &conn,
            &KEY,
            &FenceScope::single(1),
            7,
            selection,
            &scope,
        )
        .unwrap();
        let page = query_turn_content_keyset_selected(
            &conn,
            &KEY,
            &FenceScope::single(1),
            SelectedContentRequest {
                source_generation: 7,
                after_ms: None,
                source_positions: &BTreeMap::new(),
                selection,
                cursor: None,
            },
        )
        .unwrap();
        assert_eq!(exact.parts, page.content.parts);
        assert_eq!(exact.parts.len(), 1);
        assert_eq!(exact.coverage.oversized_parts, 0);
        let part = &exact.parts[0].part;
        assert!(part.text.is_empty());
        assert_eq!(
            part.normalized_fields.as_ref().unwrap().values,
            BTreeMap::from([(JevInputField::BashCommandInput, "cargo test".into())])
        );
    }

    #[test]
    fn mixed_fence_scoped_seek_composes_both_branches_and_global_order() {
        let conn = test_connection();
        for (fence, indexes) in [(1, [0, 1]), (2, [2, 3])] {
            let rows = indexes
                .into_iter()
                .map(|index| {
                    let mut row = base_row("source", index);
                    row.content = vec![ContentPart::new(
                        ContentKind::AssistantText,
                        format!("turn-{index}"),
                    )];
                    row
                })
                .collect::<Vec<_>>();
            insert_turn_rows(&conn, &KEY, fence, &rows).unwrap();
        }
        let source_keys = vec!["source".to_owned()];
        let fence_scope = FenceScope {
            claim_fence: 2,
            published: Some(PublishedScope {
                fence: 1,
                source_keys: &source_keys,
            }),
        };
        let content_scope = SelectedContentScope {
            source_key: "source".to_owned(),
            thread_id: "source".to_owned(),
            turn_index: 3,
            part_index: 0,
            native_record_ids: None,
        };
        let scope_json = serde_json::to_string(&content_scope).unwrap();
        let cursor_rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM turn WHERE turn_index = 0 AND claim_fence = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let sql = query_sql(false, false, true, false, true);
        assert_eq!(sql.matches("UNION ALL").count(), 1);
        assert_eq!(sql.matches(super::super::scope::SCOPE_PREDICATE).count(), 2);
        assert!(
            sql.contains("(turn.source_key, turn.turn_index, turn.rowid, content.part_index) >")
        );
        let mut statement = conn.prepare(&sql).unwrap();
        let rows = statement
            .query_map(
                rusqlite::named_params! {
                    ":environment_key": KEY.environment_key,
                    ":agent": KEY.agent,
                    ":session_id": KEY.session_id,
                    ":claim_fence": fence_scope.claim_fence,
                    ":published_fence": fence_scope.published.as_ref().unwrap().fence,
                    ":source_keys": serde_json::to_string(fence_scope.published.as_ref().unwrap().source_keys).unwrap(),
                    ":limit": 16,
                    ":max_part_bytes": 262144,
                    ":after_ms": Option::<i64>::None,
                    ":source_positions": "{}",
                    ":before_watermark": 0,
                    ":selection": JevInputSelection::from_fields(&[JevInputField::AssistantMessage]).bits(),
                    ":seek_source_key": "source",
                    ":seek_turn_index": 0,
                    ":seek_turn_rowid": cursor_rowid,
                    ":seek_part_index": 0,
                    ":content_scope": scope_json,
                },
                |row| row.get::<_, i64>(2),
            )
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(rows, [1, 2, 3]);
        let equal_fence = FenceScope {
            claim_fence: 1,
            published: Some(PublishedScope {
                fence: 1,
                source_keys: &source_keys,
            }),
        };
        for selected in [false, true] {
            let (page, _) = query_content_range_keyset_scoped(
                &conn,
                &KEY,
                &equal_fence,
                ContentQueryRange {
                    after_ms: None,
                    source_positions: &BTreeMap::new(),
                    bounded_context: false,
                    before_watermark: false,
                    offset: 0,
                    seek: None,
                    selection: JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
                },
                selected,
                Some(&content_scope),
            )
            .unwrap();
            assert_eq!(
                page.parts
                    .iter()
                    .map(|part| part.turn_index)
                    .collect::<Vec<_>>(),
                [0, 1]
            );
        }
    }

    #[test]
    fn composed_query_shapes_bind_exact_named_parameters() {
        let conn = test_connection();
        for recent in [false, true] {
            for single_fence in [false, true] {
                for seek in [false, true] {
                    for offset in [false, true] {
                        for selected in [false, true] {
                            for scoped in [false, true] {
                                let sql = query_sql(recent, single_fence, seek, offset, selected);
                                let mut statement = conn.prepare(&sql).unwrap();
                                let mut expected = BTreeSet::from([
                                    ":environment_key",
                                    ":agent",
                                    ":session_id",
                                    ":claim_fence",
                                    ":limit",
                                    ":max_part_bytes",
                                    ":after_ms",
                                    ":source_positions",
                                    ":before_watermark",
                                    ":selection",
                                    ":content_scope",
                                ]);
                                if !single_fence {
                                    expected.extend([":published_fence", ":source_keys"]);
                                }
                                if seek {
                                    expected.extend([
                                        ":seek_source_key",
                                        ":seek_turn_index",
                                        ":seek_turn_rowid",
                                        ":seek_part_index",
                                    ]);
                                }
                                if offset {
                                    expected.insert(":offset");
                                }
                                let actual = (1..=statement.parameter_count())
                                    .map(|index| statement.parameter_name(index).unwrap())
                                    .collect::<BTreeSet<_>>();
                                assert_eq!(actual, expected);
                                assert_eq!(statement.parameter_count(), expected.len());
                                assert!(matches!(
                                    statement.query(rusqlite::named_params! { ":unknown": 0 }),
                                    Err(rusqlite::Error::InvalidParameterName(name)) if name == ":unknown"
                                ));
                                let source_keys = vec!["source".to_owned()];
                                let fence = FenceScope {
                                    claim_fence: 2,
                                    published: (!single_fence).then_some(PublishedScope {
                                        fence: 1,
                                        source_keys: &source_keys,
                                    }),
                                };
                                let position = ContentPosition {
                                    source_key: "source".to_owned(),
                                    turn_index: 0,
                                    turn_rowid: 0,
                                    part_index: 0,
                                };
                                let content_scope = SelectedContentScope {
                                    source_key: "source".to_owned(),
                                    thread_id: "source".to_owned(),
                                    turn_index: 0,
                                    part_index: 0,
                                    native_record_ids: None,
                                };
                                let (page, _) = query_content_range_keyset_scoped(
                                    &conn,
                                    &KEY,
                                    &fence,
                                    ContentQueryRange {
                                        after_ms: recent.then_some(0),
                                        source_positions: &BTreeMap::new(),
                                        bounded_context: false,
                                        before_watermark: false,
                                        offset: usize::from(offset),
                                        seek: seek.then_some(&position),
                                        selection: JevInputSelection::from_fields(&[
                                            JevInputField::AssistantMessage,
                                        ]),
                                    },
                                    selected,
                                    scoped.then_some(&content_scope),
                                )
                                .unwrap();
                                assert!(page.parts.is_empty());
                                if selected {
                                    let mut plan =
                                        conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
                                    let details = plan
                                        .raw_query()
                                        .mapped(|row| row.get::<_, String>(3))
                                        .collect::<rusqlite::Result<Vec<_>>>()
                                        .unwrap();
                                    assert!(
                                        details.iter().any(|line| line.contains(if recent {
                                            "turn_content_recent_page"
                                        } else {
                                            "turn_content_source_page"
                                        })),
                                        "{details:?}"
                                    );
                                    assert!(
                                        !details
                                            .iter()
                                            .any(|line| line == "USE TEMP B-TREE FOR ORDER BY"),
                                        "{details:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn oversized_unknown_context_reports_coverage_without_decoding_invalid_proof() {
        let conn = test_connection();
        seed(&conn, 1);
        let oversized = serde_json::json!({"metadata": {
            "non_authorizing_context_proof": {"invalid": "é".repeat(MAX_CONTENT_PART_BYTES / 2 + 1)}
        }})
        .to_string();
        for (index, authority, text, metadata) in [
            (1, "unknown", "unqualified user text", None),
            (
                2,
                "unknown",
                "oversized generated context",
                Some(oversized.as_str()),
            ),
            (3, "user", "later human text", None),
        ] {
            conn.execute(
                "INSERT INTO turn_content (turn_rowid, part_index, kind, content,
                truncated, authority, normalized_fields_json)
                SELECT rowid, ?1, 'user', CAST(?2 AS BLOB), 0, ?3, ?4 FROM turn",
                params![index, text, authority, metadata],
            )
            .unwrap();
        }
        let page = query_turn_content_keyset_selected(
            &conn,
            &KEY,
            &FenceScope::single(1),
            SelectedContentRequest {
                source_generation: 1,
                after_ms: None,
                source_positions: &BTreeMap::new(),
                selection: JevInputSelection::from_fields(&[
                    JevInputField::UserMessage,
                    JevInputField::AssistantMessage,
                ]),
                cursor: None,
            },
        )
        .unwrap();
        assert_eq!(page.content.coverage.oversized_parts, 1);
        assert_eq!(
            page.content
                .parts
                .iter()
                .map(|part| part.part.text.as_str())
                .collect::<Vec<_>>(),
            ["synthetic evidence", "later human text"]
        );
        assert!(page.next_cursor.is_none());
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
                changed.input_identity = sha256_hex(&prior_identity);
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
                    let after_ms = recent.then_some(0);
                    let mut bindings = rusqlite::named_params! {
                        ":environment_key": "native",
                        ":agent": "claude",
                        ":session_id": "s1",
                        ":claim_fence": scope.claim_fence,
                        ":limit": 257,
                        ":max_part_bytes": 262144,
                        ":after_ms": after_ms,
                        ":source_positions": "{}",
                        ":before_watermark": 0,
                        ":selection": 2,
                        ":seek_source_key": cursor.position.source_key,
                        ":seek_turn_index": cursor.position.turn_index,
                        ":seek_turn_rowid": cursor.position.turn_rowid,
                        ":seek_part_index": cursor.position.part_index,
                        ":content_scope": Option::<String>::None,
                    }
                    .to_vec();
                    if mixed {
                        bindings.extend_from_slice(rusqlite::named_params! {
                            ":published_fence": 1,
                            ":source_keys": "[\"source\"]",
                        });
                    }
                    let plan = conn
                        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                        .unwrap()
                        .query_map(bindings.as_slice(), |row| row.get::<_, String>(3))
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

    fn assert_mixed_fence_answer_cursor_order(recent: bool) {
        use crate::analysis::jev_evidence::scope_answer_fixture;

        let conn = test_connection();
        let mut expected = Vec::new();
        for (source, fence) in [("shared", 1), ("shared", 2), ("excluded", 1), ("shared", 3)] {
            let mut row = base_row(source, 1);
            row.ts_ms = Some(1);
            let mut serialized_answers = Vec::new();
            row.content = (0..8)
                .map(|part_index| {
                    let mut answer = scope_answer_fixture();
                    answer.source.native_record_id = Some(format!("record-{:02}", 7 - part_index));
                    answer.source.call_id = Some(format!("call-{source}-{fence}"));
                    answer.source.order = part_index;
                    answer.free_text = Some("x".repeat(200 * 1024));
                    serialized_answers.push(serde_json::to_string(&vec![answer.clone()]).unwrap());
                    ContentPart::new(ContentKind::ToolResult, "excluded raw output")
                        .with_scope_evidence(vec![answer], Vec::new())
                        .unwrap()
                })
                .collect();
            assert!(serialized_answers.windows(2).all(|pair| pair[0] > pair[1]));
            insert_turn_rows(&conn, &KEY, fence, &[row]).unwrap();
            if source == "shared" && fence != 3 {
                let turn_rowid: i64 = conn
                    .query_row(
                        "SELECT rowid FROM turn WHERE source_key = ?1 AND claim_fence = ?2",
                        params![source, fence],
                        |row| row.get(0),
                    )
                    .unwrap();
                expected.extend(
                    (0..8).map(|part_index| {
                        (turn_rowid, part_index, format!("call-{source}-{fence}"))
                    }),
                );
            }
        }
        if recent {
            expected.reverse();
        }
        let sources = vec!["shared".to_owned()];
        let scope = FenceScope {
            claim_fence: 2,
            published: Some(PublishedScope {
                fence: 1,
                source_keys: &sources,
            }),
        };
        let positions = BTreeMap::new();
        let mut cursor = None;
        let mut seen = Vec::new();
        let mut pages = 0;
        loop {
            let (page, position) = query_content_range_keyset(
                &conn,
                &KEY,
                &scope,
                ContentQueryRange {
                    after_ms: recent.then_some(0),
                    source_positions: &positions,
                    bounded_context: false,
                    before_watermark: false,
                    offset: 0,
                    seek: cursor.as_ref(),
                    selection: JevInputSelection::from_fields(&[JevInputField::UserAnswer]),
                },
                true,
            )
            .unwrap();
            pages += 1;
            assert!(!page.parts.is_empty());
            assert!(!page.coverage.parts_capped);
            assert_eq!(page.coverage.oversized_parts, 0);
            let page_start = seen.len();
            let identities: Vec<_> = page
                .parts
                .iter()
                .map(|part| {
                    assert_eq!(part.source_key, "shared");
                    assert_eq!(part.turn_index, 1);
                    assert!(part.part.text.is_empty());
                    (
                        part.part_index,
                        part.part.metadata.user_answers[0]
                            .source
                            .call_id
                            .clone()
                            .unwrap(),
                    )
                })
                .collect();
            assert_eq!(
                identities,
                expected[page_start..page_start + identities.len()]
                    .iter()
                    .map(|(_, part_index, call_id)| (*part_index, call_id.clone()))
                    .collect::<Vec<_>>()
            );
            seen.extend(identities);
            let last = &expected[seen.len() - 1];
            let position = position.unwrap();
            assert_eq!(position.source_key, "shared");
            assert_eq!(position.turn_index, 1);
            assert_eq!(position.turn_rowid, last.0);
            assert_eq!(position.part_index, i64::from(last.1));
            if !page.coverage.more_parts {
                break;
            }
            assert!(page.coverage.bytes_capped);
            if pages == 1 {
                assert_eq!(page.parts.len(), 5);
                assert_eq!(position.part_index, if recent { 3 } else { 4 });
            }
            cursor = Some(position);
            assert!(pages < 5);
        }
        assert_eq!(pages, 4);
        assert_eq!(seen.len(), 16);
        assert_eq!(seen.iter().collect::<BTreeSet<_>>().len(), 16);
    }

    #[test]
    fn keyset_mixed_fence_answers_resume_mid_turn_in_forward_order() {
        assert_mixed_fence_answer_cursor_order(false);
    }

    #[test]
    fn keyset_mixed_fence_answers_resume_mid_turn_in_recent_order() {
        assert_mixed_fence_answer_cursor_order(true);
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
