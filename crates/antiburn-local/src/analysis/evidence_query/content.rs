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

const TEXT_SELECTION_SQL: &str = "CASE
    WHEN content.kind = 'thinking' THEN :selection = 4095
    WHEN content.kind = 'user' THEN (:selection & 1) != 0
    WHEN content.kind = 'assistant' THEN (:selection & 2) != 0
    WHEN content.kind = 'tool_input' THEN CASE
        WHEN json_extract(content.normalized_fields_json, '$.category') = 'bash_command' THEN (:selection & 4) != 0
        WHEN json_extract(content.normalized_fields_json, '$.category') = 'file_edit' THEN (:selection & 48) != 0
        WHEN json_extract(content.normalized_fields_json, '$.category') = 'read_file' THEN (:selection & 49216) != 0
        WHEN json_extract(content.normalized_fields_json, '$.category') = 'search_files' THEN (:selection & 256) != 0
        WHEN json_extract(content.normalized_fields_json, '$.category') = 'other_tool' THEN (:selection & 1024) != 0
        WHEN json_extract(content.normalized_fields_json, '$.category') IS NOT NULL THEN 0
        WHEN lower(content.tool_name) IN ('bash','shell','terminal','run_command','command','exec_command','run_terminal_command','execute_command')
          OR lower(content.tool_name) GLOB '*__bash' OR lower(content.tool_name) GLOB '*__shell' OR lower(content.tool_name) GLOB '*__terminal' OR lower(content.tool_name) GLOB '*__run_command' OR lower(content.tool_name) GLOB '*__command' OR lower(content.tool_name) GLOB '*__exec_command' OR lower(content.tool_name) GLOB '*__run_terminal_command' OR lower(content.tool_name) GLOB '*__execute_command' OR lower(content.tool_name) LIKE '%/bash' OR lower(content.tool_name) LIKE '%.bash' OR lower(content.tool_name) LIKE '%:bash' THEN (:selection & 4) != 0
        WHEN lower(content.tool_name) IN ('edit','write','edit_file','write_file','multi_edit','multiedit','apply_patch','applypatch','patch')
          OR lower(content.tool_name) LIKE '%__edit' OR lower(content.tool_name) LIKE '%__write' OR lower(content.tool_name) LIKE '%__apply_patch' THEN (:selection & 48) != 0
        WHEN lower(content.tool_name) IN ('read','read_file','readfile','view_file')
          OR lower(content.tool_name) GLOB '*__read' OR lower(content.tool_name) GLOB '*__view_file' THEN (:selection & 49216) != 0
        WHEN lower(content.tool_name) IN ('grep','glob','find','search','code_search','file_search','search_files','grep_search','codebase_search','list_files')
          OR lower(content.tool_name) GLOB '*__grep' OR lower(content.tool_name) GLOB '*__glob' OR lower(content.tool_name) GLOB '*__search' THEN (:selection & 256) != 0
        ELSE (:selection & 1024) != 0 AND content.tool_name IS NOT NULL AND trim(content.tool_name) <> ''
    END
    WHEN content.kind = 'tool_result' THEN CASE
        WHEN lower(content.tool_name) IN ('bash','shell','terminal','run_command','command','exec_command','run_terminal_command','execute_command')
          OR lower(content.tool_name) GLOB '*__bash' OR lower(content.tool_name) GLOB '*__shell' OR lower(content.tool_name) GLOB '*__terminal' OR lower(content.tool_name) GLOB '*__run_command' OR lower(content.tool_name) GLOB '*__command' OR lower(content.tool_name) GLOB '*__exec_command' OR lower(content.tool_name) GLOB '*__run_terminal_command' OR lower(content.tool_name) GLOB '*__execute_command' OR lower(content.tool_name) LIKE '%/bash' OR lower(content.tool_name) LIKE '%.bash' OR lower(content.tool_name) LIKE '%:bash' THEN (:selection & 8) != 0
        WHEN lower(content.tool_name) IN ('read','read_file','readfile','view_file')
          OR lower(content.tool_name) GLOB '*__read' OR lower(content.tool_name) GLOB '*__read_file' OR lower(content.tool_name) GLOB '*__readfile' OR lower(content.tool_name) GLOB '*__view_file' OR lower(content.tool_name) LIKE '%/read' OR lower(content.tool_name) LIKE '%.read' OR lower(content.tool_name) LIKE '%:read' OR lower(content.tool_name) LIKE '%/read_file' OR lower(content.tool_name) LIKE '%.read_file' OR lower(content.tool_name) LIKE '%:read_file' OR lower(content.tool_name) LIKE '%/readfile' OR lower(content.tool_name) LIKE '%.readfile' OR lower(content.tool_name) LIKE '%/view_file' OR lower(content.tool_name) LIKE '%.view_file' OR lower(content.tool_name) LIKE '%:view_file' THEN (:selection & 32896) != 0
        WHEN lower(content.tool_name) IN ('grep','glob','find','search','code_search','file_search','search_files','grep_search','codebase_search','list_files')
          OR lower(content.tool_name) LIKE '%__grep' OR lower(content.tool_name) LIKE '%__glob' OR lower(content.tool_name) LIKE '%__find' OR lower(content.tool_name) LIKE '%__search' OR lower(content.tool_name) LIKE '%__code_search' OR lower(content.tool_name) LIKE '%__file_search' OR lower(content.tool_name) LIKE '%__search_files' OR lower(content.tool_name) LIKE '%__grep_search' OR lower(content.tool_name) LIKE '%__codebase_search' OR lower(content.tool_name) LIKE '%__list_files' THEN (:selection & 512) != 0
        ELSE (:selection & 2048) != 0 AND content.tool_name IS NOT NULL AND trim(content.tool_name) <> ''
    END
    ELSE 0
END OR (content.kind <> 'thinking' AND (
    ((:selection & 4096) != 0 AND json_array_length(json_extract(content.normalized_fields_json, '$.metadata.user_answers')) > 0)
    OR ((:selection & 8192) != 0 AND json_array_length(json_extract(content.normalized_fields_json, '$.metadata.plan_references')) > 0)
    OR ((:selection & 1) != 0 AND (
        json_type(content.normalized_fields_json, '$.metadata.selected_skill') = 'object'
        OR json_type(content.normalized_fields_json, '$.metadata.non_authorizing_context_proof') = 'object'
        OR json_type(content.normalized_fields_json, '$.metadata.non_authorizing_context') = 'object'))))";

pub(super) fn content_query_sql(
    fence_predicate: &str,
    include_rowid: bool,
    scope_predicate: &str,
) -> String {
    let query = format!("SELECT turn.source_key, turn.thread_id, turn.turn_index, turn.role,
                      turn.scope, turn.ts_ms, turn.uuid, turn.message_id,
                      content.part_index, content.kind,
                        CASE WHEN json_extract(content.normalized_fields_json, '$.category') IS NOT NULL OR NOT ({text_selection}) THEN ''
                            WHEN length(content.content) <= :max_part_bytes
                            THEN CAST(content.content AS TEXT) END,
                       CASE WHEN content.normalized_fields_json IS NULL
                             THEN CASE WHEN ({text_selection}) THEN length(content.content) ELSE 0 END
                             ELSE COALESCE(length(CAST(CASE WHEN (:selection & 4) != 0 THEN json_extract(content.normalized_fields_json, '$.values.bash_command_input') END AS BLOB)), 0)
                                + COALESCE(length(CAST(CASE WHEN (:selection & 16) != 0 THEN json_extract(content.normalized_fields_json, '$.values.file_edit_path') END AS BLOB)), 0)
                                + COALESCE(length(CAST(CASE WHEN (:selection & 32) != 0 THEN json_extract(content.normalized_fields_json, '$.values.file_edit_content') END AS BLOB)), 0)
                                + COALESCE(length(CAST(CASE WHEN (:selection & 64) != 0 THEN json_extract(content.normalized_fields_json, '$.values.read_file_path') END AS BLOB)), 0)
                                + COALESCE(length(CAST(CASE WHEN (:selection & 256) != 0 THEN json_extract(content.normalized_fields_json, '$.values.search_files_query') END AS BLOB)), 0)
                                 + COALESCE(length(CAST(CASE WHEN (:selection & 1024) != 0 THEN json_extract(content.normalized_fields_json, '$.values.other_tool_input') END AS BLOB)), 0)
                                 + COALESCE(length(CAST(CASE WHEN (:selection & 4096) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.user_answers') END AS BLOB)), 0)
                                 + COALESCE(length(CAST(CASE WHEN (:selection & 8192) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.plan_references') END AS BLOB)), 0)
                                   + COALESCE(length(CAST(CASE WHEN (:selection & 49152) != 0 THEN json_extract(content.normalized_fields_json, '$.values.read_file_request') END AS BLOB)), 0)
                                   + COALESCE(length(CAST(CASE WHEN (:selection & 16384) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.read_request') END AS BLOB)), 0)
                                   + COALESCE(length(CAST(CASE WHEN (:selection & 32768) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.read_result') END AS BLOB)), 0)
                                    + COALESCE(length(CAST(CASE WHEN (:selection & 1) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.user_text_history') END AS BLOB)), 0)
                                    + COALESCE(length(CAST(CASE WHEN (:selection & 1) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.selected_skill') END AS BLOB)), 0)
                                    + COALESCE(length(CAST(CASE WHEN (:selection & 1) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.non_authorizing_context_proof') END AS BLOB)), 0)
                                    + COALESCE(length(CAST(CASE WHEN (:selection & 1) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.non_authorizing_context') END AS BLOB)), 0)
                                    + length(CAST({selected_bindings} AS BLOB))
                                   + COALESCE(length(CAST(CASE WHEN (:selection & CASE json_extract(content.normalized_fields_json, '$.metadata.recorded_skill_result.field') WHEN 'other_tool_input' THEN 1024 WHEN 'other_tool_output' THEN 2048 WHEN 'user_message' THEN 1 ELSE 0 END) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.recorded_skill_result') END AS BLOB)), 0)
                                  + CASE WHEN json_extract(content.normalized_fields_json, '$.category') IS NULL AND ({text_selection}) THEN length(content.content) ELSE 0 END
                       END,
                       content.truncated, content.authority, content.tool_name,
                       content.tool_call_id,
                       CASE WHEN (:selection & 4) != 0 THEN json_extract(content.normalized_fields_json, '$.values.bash_command_input') END,
                       CASE WHEN (:selection & 16) != 0 THEN json_extract(content.normalized_fields_json, '$.values.file_edit_path') END,
                       CASE WHEN (:selection & 32) != 0 THEN json_extract(content.normalized_fields_json, '$.values.file_edit_content') END,
                       CASE WHEN (:selection & 64) != 0 THEN json_extract(content.normalized_fields_json, '$.values.read_file_path') END,
                       CASE WHEN (:selection & 256) != 0 THEN json_extract(content.normalized_fields_json, '$.values.search_files_query') END,
                       CASE WHEN (:selection & 1024) != 0 THEN json_extract(content.normalized_fields_json, '$.values.other_tool_input') END,
                       COALESCE(json_extract(content.normalized_fields_json, '$.malformed'), 0),
                        json_extract(content.normalized_fields_json, '$.category'),
                        json_extract(content.normalized_fields_json, '$.metadata.state'),
                          {selected_bindings},
                         CASE WHEN (:selection & 4096) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.user_answers') END,
                          CASE WHEN (:selection & 8192) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.plan_references') END,
                           CASE WHEN (:selection & 49152) != 0 THEN json_extract(content.normalized_fields_json, '$.values.read_file_request') END,
                             CASE WHEN (:selection & CASE json_extract(content.normalized_fields_json, '$.metadata.recorded_skill_result.field') WHEN 'other_tool_input' THEN 1024 WHEN 'other_tool_output' THEN 2048 WHEN 'user_message' THEN 1 ELSE 0 END) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.recorded_skill_result') END,
                             CASE WHEN (:selection & 1) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.user_text_history') END,
                             CASE WHEN (:selection & 16384) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.read_request') END,
                              CASE WHEN (:selection & 32768) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.read_result') END,
                              CASE WHEN (:selection & 1) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.selected_skill') END,
                              CASE WHEN (:selection & 1) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.non_authorizing_context_proof') END,
                               CASE WHEN (:selection & 1) != 0 THEN json_extract(content.normalized_fields_json, '$.metadata.non_authorizing_context') END
                               {rowid_projection}
                  FROM turn_content AS content
                  JOIN turn ON turn.rowid = content.turn_rowid
                 WHERE turn.environment_key = :environment_key AND turn.agent = :agent
                   AND turn.session_id = :session_id
                   AND {fence_predicate}
                   AND {scope_predicate}
                   AND (:after_ms IS NULL OR content.kind <> 'thinking')
                    AND ((:before_watermark = 0 AND (:after_ms IS NULL OR turn.ts_ms >= :after_ms OR
                        (turn.ts_ms IS NULL AND turn.turn_index >
                         COALESCE((SELECT value FROM json_each(:source_positions)
                                    WHERE key = turn.source_key),
                                  (SELECT value FROM json_each(:source_positions) WHERE key = '*'),
                                  9223372036854775807))))
                     OR (:before_watermark = 1 AND (turn.ts_ms < :after_ms OR
                        (turn.ts_ms IS NULL AND turn.turn_index <=
                         COALESCE((SELECT value FROM json_each(:source_positions)
                                    WHERE key = turn.source_key),
                                   (SELECT value FROM json_each(:source_positions) WHERE key = '*'), -1)))))
                     AND ({text_selection})
                  ",
        text_selection = TEXT_SELECTION_SQL,
        selected_bindings = SELECTED_BINDINGS_SQL,
        rowid_projection = if include_rowid {
            ", turn.rowid AS turn_rowid"
        } else {
            ""
        },
    );
    query
}

const SELECTED_BINDINGS_SQL: &str = "(SELECT json_group_array(json(binding.value))
    FROM json_each(content.normalized_fields_json, '$.metadata.bindings') AS binding
    WHERE (:selection & CASE json_extract(binding.value, '$.field')
        WHEN 'user_message' THEN 1
        WHEN 'assistant_message' THEN 2
        WHEN 'bash_command_input' THEN 4
        WHEN 'bash_command_output' THEN 8
        WHEN 'file_edit_path' THEN 16
        WHEN 'file_edit_content' THEN 32
        WHEN 'read_file_path' THEN 64
        WHEN 'read_file_output' THEN 128
        WHEN 'search_files_query' THEN 256
        WHEN 'search_files_output' THEN 512
        WHEN 'other_tool_input' THEN 1024
        WHEN 'other_tool_output' THEN 2048
        WHEN 'user_answer' THEN 4096
        WHEN 'plan_reference' THEN 8192
        WHEN 'read_file_request' THEN 16384
        WHEN 'read_file_result' THEN 32768
        ELSE 0 END) != 0)";

pub(super) fn fence_predicate(single_fence: bool) -> &'static str {
    if single_fence {
        "turn.claim_fence = :claim_fence"
    } else {
        "(turn.claim_fence = :claim_fence OR (turn.claim_fence = :published_fence AND turn.source_key IN
            (SELECT value FROM json_each(:source_keys))))"
    }
}

pub(super) fn order_fragment(recent: bool, qualified: bool) -> &'static str {
    match (recent, qualified) {
        (true, true) => {
            "ORDER BY turn.turn_index DESC, turn.source_key DESC, turn.rowid DESC, content.part_index DESC"
        }
        (true, false) => {
            "ORDER BY turn_index DESC, source_key DESC, turn_rowid DESC, part_index DESC"
        }
        (false, true) => {
            "ORDER BY turn.source_key, turn.turn_index, turn.rowid, content.part_index"
        }
        (false, false) => "ORDER BY source_key, turn_index, turn_rowid, part_index",
    }
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
    query_content_range_keyset_scoped(conn, key, scope, range, selected_keyset, None)
}

pub(super) fn query_content_range_keyset_scoped(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    scope: &FenceScope<'_>,
    range: ContentQueryRange<'_>,
    selected_keyset: bool,
    content_scope: Option<&SelectedContentScope>,
) -> rusqlite::Result<(PublishedContent, Option<keyset::ContentPosition>)> {
    query_content_range_with_sql(
        conn,
        key,
        scope,
        range,
        selected_keyset,
        content_scope,
        None,
    )
}

pub(super) fn query_content_range_with_sql(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    scope: &FenceScope<'_>,
    range: ContentQueryRange<'_>,
    selected_keyset: bool,
    content_scope: Option<&SelectedContentScope>,
    query_sql: Option<&str>,
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
    let scope_json = content_scope
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let mut statement = conn.prepare(query_sql.unwrap_or(&sql))?;
    let limit = (if bounded_context {
        context_parts
    } else {
        MAX_CONTENT_QUERY_PARTS
    } + 1) as i64;
    let max_part_bytes = crate::analysis::interface::MAX_CONTENT_PART_BYTES as i64;
    let before_watermark_value = i64::from(before_watermark);
    let offset_value = if before_watermark {
        0
    } else {
        i64::try_from(offset).unwrap_or(i64::MAX)
    };
    let selection_bits = selection.bits();
    let mut bindings = rusqlite::named_params! {
        ":environment_key": key.environment_key,
        ":agent": key.agent,
        ":session_id": key.session_id,
        ":claim_fence": claim_fence,
        ":limit": limit,
        ":max_part_bytes": max_part_bytes,
        ":after_ms": after_ms,
        ":source_positions": positions_json,
        ":before_watermark": before_watermark_value,
        ":selection": selection_bits,
        ":content_scope": scope_json,
    }
    .to_vec();
    if scope.published.is_some() {
        bindings.extend_from_slice(rusqlite::named_params! {
            ":published_fence": published_fence,
            ":source_keys": source_keys_json,
        });
    }
    if let Some(position) = seek {
        bindings.extend_from_slice(rusqlite::named_params! {
            ":seek_source_key": position.source_key,
            ":seek_turn_index": position.turn_index,
            ":seek_turn_rowid": position.turn_rowid,
            ":seek_part_index": position.part_index,
        });
    }
    if offset != 0 {
        bindings.push((":offset", &offset_value));
    }
    if bindings.len() != statement.parameter_count() {
        return Err(rusqlite::Error::InvalidParameterCount(
            bindings.len(),
            statement.parameter_count(),
        ));
    }
    let mut rows = statement.query(bindings.as_slice())?;
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
            turn_rowid: row.get("turn_rowid")?,
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
        let raw_text: String = row.get(10)?;
        let text = if matches!(kind, ContentKind::UserText)
            && !selection.includes(JevInputField::UserMessage)
            || matches!(kind, ContentKind::AssistantText)
                && !selection.includes(JevInputField::AssistantMessage)
            || matches!(kind, ContentKind::ToolResult)
                && !(selection.includes(JevInputField::ReadFileResult)
                    && super::super::jev_evidence::tool_output_field(
                        row.get::<_, Option<String>>(14)?.as_deref().unwrap_or(""),
                    ) == JevInputField::ReadFileOutput)
                && !selection.includes(super::super::jev_evidence::tool_output_field(
                    row.get::<_, Option<String>>(14)?.as_deref().unwrap_or(""),
                )) {
            String::new()
        } else {
            raw_text
        };
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
        if let Some(request) = row.get::<_, Option<String>>(28)? {
            normalized_values.insert(JevInputField::ReadFileRequest, request);
        }
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
                selected_skill: decode_scope_metadata(row, 33)?,
                non_authorizing_context_proof: decode_scope_metadata(row, 34)?,
                non_authorizing_context: decode_scope_metadata(row, 35)?,
                user_text_history: decode_scope_metadata(row, 30)?,
                recorded_skill_result: row
                    .get::<_, Option<String>>(29)?
                    .map(|text| serde_json::from_str(&text))
                    .transpose()
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            29,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                read_request: decode_scope_metadata(row, 31)?,
                read_result: decode_scope_metadata(row, 32)?,
                user_answers: decode_scope_metadata(row, 26)?,
                plan_references: decode_scope_metadata(row, 27)?,
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
                ..Default::default()
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

fn decode_scope_metadata<T: serde::de::DeserializeOwned + Default>(
    row: &rusqlite::Row<'_>,
    column: usize,
) -> rusqlite::Result<T> {
    row.get::<_, Option<String>>(column)?
        .map(|text| {
            serde_json::from_str(&text).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    column,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()
        .map(Option::unwrap_or_default)
}
