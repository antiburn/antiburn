use antiburn_local::analysis::jev_evidence::{
    JevOperationMetadata, is_recorded_skill_selection, prepare_session_content,
};
use antiburn_local::analysis::{
    ContentAuthority, ContentKind, ContentPart, PublishedContent, PublishedContentPart,
};
use rusqlite::{Connection, params};

use super::{SessionKey, SourceFormat};

pub(super) fn validate(
    connection: &Connection,
    key: &SessionKey,
    fence: i64,
    format: SourceFormat,
) -> rusqlite::Result<bool> {
    let mut statement = connection.prepare(
        "SELECT t.uuid, t.message_id,
            CASE WHEN length(c.normalized_fields_json) <= 131072
                THEN json_extract(c.normalized_fields_json, '$.metadata') ELSE NULL END, c.truncated,
            CASE WHEN length(c.content) <= ?5 THEN c.content ELSE NULL END,
            t.source_key, t.thread_id, t.turn_index, c.part_index, t.role
         FROM turn t JOIN turn_content c ON c.turn_rowid = t.rowid
         WHERE t.environment_key = ?1 AND t.agent = ?2 AND t.session_id = ?3
            AND t.claim_fence = ?4 AND t.scope = 'main'
            AND c.kind = 'user' AND c.authority != 'user'",
    )?;
    let mut rows = statement.query(params![
        key.environment_key,
        key.agent,
        key.session_id,
        fence,
        antiburn_local::analysis::MAX_CONTENT_PART_BYTES
    ])?;
    let mut count = 0;
    let mut retained_bytes = 0;
    while let Some(row) = rows.next()? {
        count += 1;
        if count > 65_536 {
            return Ok(false);
        }
        let uuid: Option<String> = row.get(0)?;
        let message_id: Option<String> = row.get(1)?;
        let metadata: Option<String> = row.get(2)?;
        let truncated: bool = row.get(3)?;
        let content: Option<Vec<u8>> = row.get(4)?;
        retained_bytes +=
            metadata.as_ref().map_or(0, String::len) + content.as_ref().map_or(0, Vec::len);
        if truncated || retained_bytes > 16 * 1024 * 1024 {
            return Ok(false);
        }
        let Some(metadata) = metadata
            .as_deref()
            .and_then(|json| serde_json::from_str::<JevOperationMetadata>(json).ok())
        else {
            return Ok(false);
        };
        let Some(text) = content.and_then(|bytes| String::from_utf8(bytes).ok()) else {
            return Ok(false);
        };
        let mut part =
            ContentPart::new(ContentKind::UserText, text).with_authority(ContentAuthority::Unknown);
        part.metadata = metadata;
        let role: String = row.get(9)?;
        if role != "user" {
            return Ok(false);
        }
        let normalized = prepare_session_content(
            &key.session_id,
            format,
            PublishedContent {
                parts: vec![PublishedContentPart {
                    source_key: row.get(5)?,
                    thread_id: row.get(6)?,
                    turn_index: row.get(7)?,
                    part_index: row.get(8)?,
                    role: "user",
                    scope: "main".into(),
                    ts_ms: None,
                    uuid,
                    message_id,
                    part,
                    context_only: false,
                    stable_event_identity: true,
                }],
                ..Default::default()
            },
            Vec::new(),
        );
        let [action] = normalized.actions.as_slice() else {
            return Ok(false);
        };
        let known_context = action
            .metadata
            .non_authorizing_context
            .as_ref()
            .is_some_and(|fact| fact.matches_session(action, format, &key.session_id));
        let known_skill = is_recorded_skill_selection(action)
            && action
                .metadata
                .recorded_skill_result
                .as_ref()
                .is_some_and(|fact| {
                    fact.source_format == format
                        && fact.session_id.as_deref() == Some(key.session_id.as_str())
                });
        if !known_context && !known_skill {
            return Ok(false);
        }
    }
    Ok(true)
}
