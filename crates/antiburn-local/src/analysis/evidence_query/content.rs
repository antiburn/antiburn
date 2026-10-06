use super::*;

/// Reads private content only for the supplied fence scope and within fixed
/// part and byte bounds. Ordinary turn queries never call this function.
pub fn query_turn_content(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    scope: &FenceScope<'_>,
) -> rusqlite::Result<PublishedContent> {
    query_turn_content_after(conn, key, scope, None, &BTreeMap::new())
}

/// Read eligible actions with bounded context before enablement and after each page.
/// Positions are captured at enablement per source; an absent position is not
/// evidence that an existing timestamp-less source started after enablement.
pub fn query_turn_content_after(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    scope: &FenceScope<'_>,
    after_ms: Option<i64>,
    source_positions: &BTreeMap<String, u64>,
) -> rusqlite::Result<PublishedContent> {
    query_turn_content_page(conn, key, scope, after_ms, source_positions, 0)
}

pub fn query_turn_content_page(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    scope: &FenceScope<'_>,
    after_ms: Option<i64>,
    source_positions: &BTreeMap<String, u64>,
    page: usize,
) -> rusqlite::Result<PublishedContent> {
    query_turn_content_offset(
        conn,
        key,
        scope,
        after_ms,
        source_positions,
        page.saturating_mul(MAX_CONTENT_QUERY_PARTS),
    )
}

pub fn query_turn_content_offset(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    scope: &FenceScope<'_>,
    after_ms: Option<i64>,
    source_positions: &BTreeMap<String, u64>,
    offset: usize,
) -> rusqlite::Result<PublishedContent> {
    query_turn_content_offset_selected(
        conn,
        key,
        scope,
        after_ms,
        source_positions,
        offset,
        JevInputSelection::ALL,
    )
}

/// Reads only captured content kinds and tool categories allowed by `selection`.
pub fn query_turn_content_offset_selected(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    scope: &FenceScope<'_>,
    after_ms: Option<i64>,
    source_positions: &BTreeMap<String, u64>,
    offset: usize,
    selection: JevInputSelection,
) -> rusqlite::Result<PublishedContent> {
    let mut result = query_content_range(
        conn,
        key,
        scope,
        ContentQueryRange {
            after_ms,
            source_positions,
            bounded_context: false,
            before_watermark: false,
            offset,
            seek: None,
            selection,
        },
    )?;
    if after_ms.is_some() {
        let earlier = query_content_range(
            conn,
            key,
            scope,
            ContentQueryRange {
                after_ms,
                source_positions,
                bounded_context: true,
                before_watermark: true,
                offset: 0,
                seek: None,
                selection,
            },
        )?;
        result.coverage.context_capped = earlier.coverage.context_capped;
        result
            .parts
            .extend(earlier.parts.into_iter().map(|mut part| {
                part.context_only = true;
                part
            }));
        if result.coverage.more_parts {
            let next_context = query_content_range(
                conn,
                key,
                scope,
                ContentQueryRange {
                    after_ms,
                    source_positions,
                    bounded_context: true,
                    before_watermark: false,
                    offset: result.next_offset,
                    seek: None,
                    selection,
                },
            )?;
            result.coverage.context_capped |= next_context.coverage.context_capped;
            result
                .parts
                .extend(next_context.parts.into_iter().map(|mut part| {
                    part.context_only = true;
                    part
                }));
        }
        result.parts.sort_by(|a, b| {
            (&a.source_key, a.turn_index, a.part_index).cmp(&(
                &b.source_key,
                b.turn_index,
                b.part_index,
            ))
        });
    }
    Ok(result)
}

pub(super) struct ContentQueryRange<'a> {
    pub(super) after_ms: Option<i64>,
    pub(super) source_positions: &'a BTreeMap<String, u64>,
    pub(super) bounded_context: bool,
    pub(super) before_watermark: bool,
    pub(super) offset: usize,
    pub(super) seek: Option<&'a keyset::ContentPosition>,
    pub(super) selection: JevInputSelection,
}

pub(super) fn content_query_sql() -> &'static str {
    "SELECT turn.source_key, turn.thread_id, turn.turn_index, turn.role,
                      turn.scope, turn.ts_ms, turn.uuid, turn.message_id,
                      content.part_index, content.kind,
                       CASE WHEN content.normalized_fields_json IS NOT NULL THEN ''
                            WHEN length(content.content) <= ?8
                            THEN CAST(content.content AS TEXT) END,
                       CASE WHEN content.normalized_fields_json IS NULL
                            THEN length(content.content)
                             ELSE COALESCE(length(CAST(CASE WHEN (?13 & 4) != 0 THEN json_extract(content.normalized_fields_json, '$.values.bash_command_input') END AS BLOB)), 0)
                                + COALESCE(length(CAST(CASE WHEN (?13 & 16) != 0 THEN json_extract(content.normalized_fields_json, '$.values.file_edit_path') END AS BLOB)), 0)
                                + COALESCE(length(CAST(CASE WHEN (?13 & 32) != 0 THEN json_extract(content.normalized_fields_json, '$.values.file_edit_content') END AS BLOB)), 0)
                                + COALESCE(length(CAST(CASE WHEN (?13 & 64) != 0 THEN json_extract(content.normalized_fields_json, '$.values.read_file_path') END AS BLOB)), 0)
                                + COALESCE(length(CAST(CASE WHEN (?13 & 256) != 0 THEN json_extract(content.normalized_fields_json, '$.values.search_files_query') END AS BLOB)), 0)
                                + COALESCE(length(CAST(CASE WHEN (?13 & 1024) != 0 THEN json_extract(content.normalized_fields_json, '$.values.other_tool_input') END AS BLOB)), 0)
                       END,
                       content.truncated, content.authority, content.tool_name,
                       content.tool_call_id,
                       CASE WHEN (?13 & 4) != 0 THEN json_extract(content.normalized_fields_json, '$.values.bash_command_input') END,
                       CASE WHEN (?13 & 16) != 0 THEN json_extract(content.normalized_fields_json, '$.values.file_edit_path') END,
                       CASE WHEN (?13 & 32) != 0 THEN json_extract(content.normalized_fields_json, '$.values.file_edit_content') END,
                       CASE WHEN (?13 & 64) != 0 THEN json_extract(content.normalized_fields_json, '$.values.read_file_path') END,
                       CASE WHEN (?13 & 256) != 0 THEN json_extract(content.normalized_fields_json, '$.values.search_files_query') END,
                       CASE WHEN (?13 & 1024) != 0 THEN json_extract(content.normalized_fields_json, '$.values.other_tool_input') END,
                       COALESCE(json_extract(content.normalized_fields_json, '$.malformed'), 0),
                        json_extract(content.normalized_fields_json, '$.category'),
                        json_extract(content.normalized_fields_json, '$.metadata.state'),
                        (SELECT json_group_array(json(binding.value))
                           FROM json_each(content.normalized_fields_json, '$.metadata.bindings') AS binding
                          WHERE (?13 & CASE json_extract(binding.value, '$.field')
                              WHEN 'bash_command_input' THEN 4
                              WHEN 'file_edit_path' THEN 16
                              WHEN 'file_edit_content' THEN 32
                              WHEN 'read_file_path' THEN 64
                              WHEN 'search_files_query' THEN 256
                              WHEN 'other_tool_input' THEN 1024 ELSE 0 END) != 0)
                 FROM turn_content AS content
                 JOIN turn ON turn.rowid = content.turn_rowid
                WHERE turn.environment_key = ?1 AND turn.agent = ?2
                  AND turn.session_id = ?3
                  AND (turn.claim_fence = ?4 OR
                       (turn.claim_fence = ?5 AND turn.source_key IN
                           (SELECT value FROM json_each(?6))))
                   AND (?9 IS NULL OR content.kind <> 'thinking')
                    AND ((?11 = 0 AND (?9 IS NULL OR turn.ts_ms >= ?9 OR
                        (turn.ts_ms IS NULL AND turn.turn_index >
                         COALESCE((SELECT value FROM json_each(?10)
                                    WHERE key = turn.source_key),
                                  (SELECT value FROM json_each(?10) WHERE key = '*'),
                                  9223372036854775807))))
                     OR (?11 = 1 AND (turn.ts_ms < ?9 OR
                        (turn.ts_ms IS NULL AND turn.turn_index <=
                         COALESCE((SELECT value FROM json_each(?10)
                                    WHERE key = turn.source_key),
                                   (SELECT value FROM json_each(?10) WHERE key = '*'), -1)))))
                    AND CASE
                      WHEN content.kind = 'thinking' THEN ?13 = 4095
                      WHEN content.kind = 'user' THEN (?13 & 1) != 0
                      WHEN content.kind = 'assistant' THEN (?13 & 2) != 0
                      WHEN content.kind = 'tool_input' THEN CASE
                        WHEN json_extract(content.normalized_fields_json, '$.category') = 'bash_command'
                          THEN (?13 & 4) != 0
                        WHEN json_extract(content.normalized_fields_json, '$.category') = 'file_edit'
                          THEN (?13 & 48) != 0
                        WHEN json_extract(content.normalized_fields_json, '$.category') = 'read_file'
                          THEN (?13 & 64) != 0
                        WHEN json_extract(content.normalized_fields_json, '$.category') = 'search_files'
                          THEN (?13 & 256) != 0
                        WHEN json_extract(content.normalized_fields_json, '$.category') = 'other_tool'
                          THEN (?13 & 1024) != 0
                        WHEN content.normalized_fields_json IS NOT NULL THEN 0
                        WHEN lower(content.tool_name) IN ('bash','shell','terminal','run_command','command','exec_command','run_terminal_command','execute_command')
                          OR lower(content.tool_name) GLOB '*__bash' OR lower(content.tool_name) GLOB '*__shell' OR lower(content.tool_name) GLOB '*__terminal' OR lower(content.tool_name) GLOB '*__run_command' OR lower(content.tool_name) GLOB '*__command' OR lower(content.tool_name) GLOB '*__exec_command' OR lower(content.tool_name) GLOB '*__run_terminal_command' OR lower(content.tool_name) GLOB '*__execute_command' OR lower(content.tool_name) LIKE '%/bash' OR lower(content.tool_name) LIKE '%.bash' OR lower(content.tool_name) LIKE '%:bash'
                          THEN (?13 & 4) != 0
                        WHEN lower(content.tool_name) IN ('edit','write','edit_file','write_file','multi_edit','multiedit','apply_patch','applypatch','patch')
                          OR lower(content.tool_name) LIKE '%__edit' OR lower(content.tool_name) LIKE '%__write' OR lower(content.tool_name) LIKE '%__apply_patch'
                          THEN (?13 & 48) != 0
                        WHEN lower(content.tool_name) IN ('read','read_file','readfile','view_file')
                          OR lower(content.tool_name) LIKE '%__read' OR lower(content.tool_name) LIKE '%__view_file'
                          THEN (?13 & 64) != 0
                        WHEN lower(content.tool_name) IN ('grep','glob','find','search','code_search','file_search','search_files','grep_search','codebase_search','list_files')
                          OR lower(content.tool_name) LIKE '%__grep' OR lower(content.tool_name) LIKE '%__glob' OR lower(content.tool_name) LIKE '%__search'
                          THEN (?13 & 256) != 0
                        ELSE (?13 & 1024) != 0
                          AND content.tool_name IS NOT NULL AND trim(content.tool_name) <> ''
                      END
                      WHEN content.kind = 'tool_result' THEN CASE
                         WHEN lower(content.tool_name) IN ('bash','shell','terminal','run_command','command','exec_command','run_terminal_command','execute_command')
                            OR lower(content.tool_name) GLOB '*__bash' OR lower(content.tool_name) GLOB '*__shell' OR lower(content.tool_name) GLOB '*__terminal' OR lower(content.tool_name) GLOB '*__run_command' OR lower(content.tool_name) GLOB '*__command' OR lower(content.tool_name) GLOB '*__exec_command' OR lower(content.tool_name) GLOB '*__run_terminal_command' OR lower(content.tool_name) GLOB '*__execute_command' OR lower(content.tool_name) LIKE '%/bash' OR lower(content.tool_name) LIKE '%.bash' OR lower(content.tool_name) LIKE '%:bash'
                          THEN (?13 & 8) != 0
                         WHEN lower(content.tool_name) IN ('read','read_file','readfile','view_file')
                            OR lower(content.tool_name) GLOB '*__read' OR lower(content.tool_name) GLOB '*__read_file' OR lower(content.tool_name) GLOB '*__readfile' OR lower(content.tool_name) GLOB '*__view_file' OR lower(content.tool_name) LIKE '%/read' OR lower(content.tool_name) LIKE '%.read' OR lower(content.tool_name) LIKE '%:read' OR lower(content.tool_name) LIKE '%/read_file' OR lower(content.tool_name) LIKE '%.read_file' OR lower(content.tool_name) LIKE '%:read_file' OR lower(content.tool_name) LIKE '%/readfile' OR lower(content.tool_name) LIKE '%.readfile' OR lower(content.tool_name) LIKE '%/view_file' OR lower(content.tool_name) LIKE '%.view_file' OR lower(content.tool_name) LIKE '%:view_file'
                          THEN (?13 & 128) != 0
                         WHEN lower(content.tool_name) IN ('grep','glob','find','search','code_search','file_search','search_files','grep_search','codebase_search','list_files')
                           OR lower(content.tool_name) LIKE '%__grep' OR lower(content.tool_name) LIKE '%__glob' OR lower(content.tool_name) LIKE '%__find' OR lower(content.tool_name) LIKE '%__search' OR lower(content.tool_name) LIKE '%__code_search' OR lower(content.tool_name) LIKE '%__file_search' OR lower(content.tool_name) LIKE '%__search_files' OR lower(content.tool_name) LIKE '%__grep_search' OR lower(content.tool_name) LIKE '%__codebase_search' OR lower(content.tool_name) LIKE '%__list_files'
                          THEN (?13 & 512) != 0
                        ELSE (?13 & 2048) != 0
                          AND content.tool_name IS NOT NULL AND trim(content.tool_name) <> ''
                      END
                      ELSE 0
                    END
                 ORDER BY CASE WHEN ?9 IS NULL THEN turn.source_key END ASC,
                          CASE WHEN ?9 IS NULL THEN turn.turn_index END ASC,
                          CASE WHEN ?9 IS NULL THEN content.part_index END ASC,
                          turn.turn_index DESC, turn.source_key DESC, content.part_index DESC
                   LIMIT ?7 OFFSET ?12"
}

pub(super) fn ordered_content_query_sql(recent: bool, single_fence: bool) -> String {
    let (query, _) = content_query_sql()
        .split_once("ORDER BY CASE")
        .expect("the content query includes an order clause");
    let mut query = if single_fence {
        let (before, fence) = query
            .split_once("AND (turn.claim_fence = ?4 OR")
            .expect("the content query includes its fence");
        let (_, after) = fence
            .split_once("AND (?9")
            .expect("the content query includes its content boundary");
        format!("{before}AND turn.claim_fence = ?4 AND (?9{after}")
    } else {
        query.to_owned()
    };
    query.push_str(if recent {
        "ORDER BY turn.turn_index DESC, turn.source_key DESC, content.part_index DESC LIMIT ?7 OFFSET ?12"
    } else {
        "ORDER BY turn.source_key, turn.turn_index, content.part_index LIMIT ?7 OFFSET ?12"
    });
    query
}

pub(super) fn query_content_range(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    scope: &FenceScope<'_>,
    range: ContentQueryRange<'_>,
) -> rusqlite::Result<PublishedContent> {
    query_content_range_keyset(conn, key, scope, range, false).map(|(content, _)| content)
}

pub(super) fn query_content_range_keyset(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    scope: &FenceScope<'_>,
    range: ContentQueryRange<'_>,
    selected_keyset: bool,
) -> rusqlite::Result<(PublishedContent, Option<keyset::ContentPosition>)> {
    let ContentQueryRange {
        after_ms,
        source_positions,
        bounded_context,
        before_watermark,
        offset,
        seek,
        selection,
    } = range;
    let (claim_fence, published_fence, source_keys_json) = scope_bind_values(scope);
    let positions_json = serde_json::to_string(source_positions).unwrap_or_default();
    let context_parts = if selected_keyset {
        MAX_CONTENT_CONTEXT_PARTS / 2
    } else {
        MAX_CONTENT_CONTEXT_PARTS
    };
    let context_bytes = if selected_keyset {
        MAX_CONTENT_CONTEXT_BYTES / 2
    } else {
        MAX_CONTENT_CONTEXT_BYTES
    };
    let sql = keyset::query_sql(
        after_ms.is_some(),
        scope.published.is_none(),
        seek.is_some(),
        offset != 0,
        selected_keyset,
    );
    let mut statement = conn.prepare(&sql)?;
    let mut rows = statement.query(params![
        key.environment_key,
        key.agent,
        key.session_id,
        claim_fence,
        published_fence,
        source_keys_json,
        (if bounded_context {
            context_parts
        } else {
            MAX_CONTENT_QUERY_PARTS
        } + 1) as i64,
        crate::analysis::interface::MAX_CONTENT_PART_BYTES as i64,
        after_ms,
        positions_json,
        i64::from(before_watermark),
        if before_watermark {
            0
        } else {
            i64::try_from(offset).unwrap_or(i64::MAX)
        },
        selection.bits(),
        seek.map(|position| position.source_key.as_str()),
        seek.map(|position| position.turn_index),
        seek.map(|position| position.turn_rowid),
        seek.map(|position| position.part_index),
    ])?;
    let mut result = PublishedContent {
        publication_fence: scope.claim_fence,
        ..PublishedContent::default()
    };
    let mut retained_bytes = 0usize;
    let mut scanned_parts = 0usize;
    result.next_offset = offset;
    let mut last_position = seek.cloned();

    while let Some(row) = rows.next()? {
        if scanned_parts
            == if bounded_context {
                context_parts
            } else {
                MAX_CONTENT_QUERY_PARTS
            }
        {
            if bounded_context {
                result.coverage.context_capped = true;
            } else {
                result.coverage.parts_capped = true;
                result.coverage.more_parts = true;
                result.next_offset = offset.saturating_add(scanned_parts);
            }
            break;
        }
        scanned_parts += 1;

        let position = keyset::ContentPosition {
            source_key: row.get(0)?,
            turn_index: row.get(2)?,
            turn_rowid: row.get(26)?,
            part_index: row.get(8)?,
        };

        let byte_length = as_u64(row.get(11)?) as usize;
        let already_truncated: i64 = row.get(12)?;
        if byte_length > crate::analysis::interface::MAX_CONTENT_PART_BYTES {
            result.coverage.oversized_parts = result.coverage.oversized_parts.saturating_add(1);
            last_position = Some(position);
            continue;
        }
        if byte_length
            > (if bounded_context {
                context_bytes
            } else {
                MAX_CONTENT_QUERY_BYTES
            })
            .saturating_sub(retained_bytes)
        {
            if bounded_context {
                result.coverage.context_capped = true;
            } else {
                result.coverage.bytes_capped = true;
                result.coverage.more_parts = true;
                result.next_offset = offset.saturating_add(scanned_parts.saturating_sub(1));
            }
            break;
        }

        let kind_text: String = row.get(9)?;
        let kind = ContentKind::parse(&kind_text).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                format!("unrecognized content kind {kind_text:?}").into(),
            )
        })?;
        let authority_text: String = row.get(13)?;
        let authority = ContentAuthority::parse(&authority_text).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                13,
                rusqlite::types::Type::Text,
                format!("unrecognized content authority {authority_text:?}").into(),
            )
        })?;
        let text: String = row.get(10)?;
        let part_index = as_u64(row.get(8)?).min(u32::MAX as u64) as u32;
        let mut normalized_values = BTreeMap::new();
        for (field, column) in [
            (JevInputField::BashCommandInput, 16),
            (JevInputField::FileEditPath, 17),
            (JevInputField::FileEditContent, 18),
            (JevInputField::ReadFilePath, 19),
            (JevInputField::SearchFilesQuery, 20),
            (JevInputField::OtherToolInput, 21),
        ] {
            if let Some(value) = row.get::<_, Option<String>>(column)? {
                normalized_values.insert(field, value);
            }
        }
        let malformed: i64 = row.get(22)?;
        let category: Option<String> = row.get(23)?;
        let category = category.as_deref().and_then(JevNormalizedCategory::parse);
        let normalized_fields = (!normalized_values.is_empty()
            || malformed != 0
            || category.is_some())
        .then_some(JevNormalizedFields {
            category,
            values: normalized_values,
            malformed: malformed != 0,
        });
        let part = ContentPart {
            kind,
            authority,
            text,
            tool_name: row.get(14)?,
            tool_call_id: row.get(15)?,
            normalized_fields,
            metadata: crate::analysis::jev_evidence::JevOperationMetadata {
                state: row
                    .get::<_, Option<String>>(24)?
                    .map(|state| serde_json::from_value(serde_json::Value::String(state)))
                    .transpose()
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            24,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?
                    .unwrap_or_default(),
                bindings: serde_json::from_str(&row.get::<_, String>(25)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        25,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
            },
            truncated: already_truncated != 0,
        };
        if part.truncated {
            result.coverage.stored_truncated_parts =
                result.coverage.stored_truncated_parts.saturating_add(1);
        }
        retained_bytes = retained_bytes.saturating_add(byte_length);
        last_position = Some(position);
        let uuid: Option<String> = row.get(6)?;
        let message_id: Option<String> = row.get(7)?;
        let role_text: String = row.get(3)?;
        let role = parse_role(&role_text).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                format!("unrecognized turn role {role_text:?}").into(),
            )
        })?;
        result.parts.push(PublishedContentPart {
            source_key: row.get(0)?,
            thread_id: row.get(1)?,
            turn_index: as_u64(row.get(2)?),
            role,
            scope: row.get(4)?,
            ts_ms: row.get(5)?,
            stable_event_identity: uuid.is_some() || message_id.is_some(),
            uuid,
            message_id,
            part_index,
            part,
            context_only: false,
        });
    }
    if !before_watermark && !result.coverage.more_parts {
        result.next_offset = offset.saturating_add(scanned_parts);
    }
    Ok((result, last_position))
}
