//! Parser 65 separates legacy Claude request text from new live work.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::store::SessionKey;

pub(crate) const RESET_REASON: &str = "claude-request-origin-parser-65";
const FORMAT: &str = "\"claude_jsonl\"";
const BINDING_PREFIX: &str = "internal:claudeRequestOriginParser65:";

fn binding_key(session: &SessionKey) -> String {
    format!("{BINDING_PREFIX}{}", session.to_identity_json())
}

pub(crate) fn boundary_sql(session: &str) -> String {
    format!("(SELECT MAX(origin.publication_sequence) FROM native_tool_result_publication origin
        JOIN setting origin_binding ON origin_binding.key = '{BINDING_PREFIX}' ||
          json_array({session}.environment_key, {session}.agent, {session}.session_id)
        WHERE origin.environment_key = {session}.environment_key AND origin.agent = {session}.agent
          AND origin.session_id = {session}.session_id AND origin.source_format = '\"claude_jsonl\"'
          AND ({session}.source_format IS NULL OR {session}.source_format = '\"claude_jsonl\"')
          AND origin.session_incarnation = {session}.incarnation AND origin.project_cwd IS {session}.cwd
          AND origin.outcome = 'coverage_gap' AND origin.gap_kind = 'segment_reset'
          AND origin.gap_reason = '{RESET_REASON}'
          AND json_extract(origin_binding.value, '$.publication_sequence') = origin.publication_sequence
          AND json_extract(origin_binding.value, '$.source_label') = {session}.source_label
          AND json_extract(origin_binding.value, '$.incarnation') = {session}.incarnation
          AND json_extract(origin_binding.value, '$.cwd') IS {session}.cwd
          AND json_extract(origin_binding.value, '$.source_format') = 'claude_jsonl')")
}

fn source_origin_proven_sql(session: &str) -> String {
    let boundary = boundary_sql(session);
    format!("({session}.agent <> 'claude-code' OR {session}.environment_key <> 'native' OR
        ({session}.source_kind = 'file' AND {session}.wsl_distro IS NULL AND {boundary} IS NOT NULL
         AND EXISTS(SELECT 1 FROM native_file_delivery origin_file
           WHERE origin_file.environment_key = {session}.environment_key AND origin_file.agent = {session}.agent
             AND origin_file.session_id = {session}.session_id AND origin_file.incarnation = {session}.incarnation
             AND origin_file.source_label = {session}.source_label AND origin_file.cwd IS {session}.cwd
                AND origin_file.parser_revision IN (65, 66, {})
             AND json_extract(origin_file.cursor_json, '$.revision') = 5
             AND json_extract(origin_file.cursor_json, '$.agent') = 'claude'
             AND json_extract(origin_file.cursor_json, '$.session_id') = {session}.session_id
             AND json_extract(origin_file.cursor_json, '$.source_format') = 'claude_jsonl'))) ",
        antiburn_local::analysis::PARSER_REVISION)
}

pub(crate) fn source_ready_sql(session: &str) -> String {
    let proven = source_origin_proven_sql(session);
    format!("({proven} AND ({session}.agent <> 'claude-code' OR {session}.environment_key <> 'native'
        OR EXISTS(SELECT 1 FROM native_file_delivery origin_health
          WHERE origin_health.environment_key = {session}.environment_key AND origin_health.agent = {session}.agent
            AND origin_health.session_id = {session}.session_id
            AND origin_health.blocked = 0 AND origin_health.last_error IS NULL)))")
}

pub(crate) fn publication_allowed_sql(publication: &str) -> String {
    let boundary = boundary_sql("origin_session");
    format!("(NOT ({publication}.environment_key = 'native' AND {publication}.agent = 'claude-code'
          AND {publication}.source_format = '\"claude_jsonl\"') OR EXISTS(
        SELECT 1 FROM session origin_session
         WHERE origin_session.environment_key = {publication}.environment_key
           AND origin_session.agent = {publication}.agent AND origin_session.session_id = {publication}.session_id
           AND origin_session.source_kind = 'file' AND origin_session.wsl_distro IS NULL
           AND {publication}.session_incarnation = origin_session.incarnation
           AND {publication}.project_cwd IS origin_session.cwd
           AND {publication}.publication_sequence >= {boundary}))")
}

pub(crate) fn publication_allowed_in(connection: &Connection, sequence: i64) -> Result<bool> {
    let allowed = publication_allowed_sql("p");
    Ok(connection.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM native_tool_result_publication p
            WHERE p.publication_sequence = ?1 AND {allowed})"
        ),
        [sequence],
        |row| row.get(0),
    )?)
}

pub(crate) fn new_candidate_request_origin_current_in(
    connection: &Connection,
    candidate: &str,
) -> Result<bool> {
    let proven = source_origin_proven_sql("s");
    let boundary = boundary_sql("s");
    Ok(connection.query_row(
        &format!(
            "SELECT NOT EXISTS(
        SELECT 1 FROM burn_check_candidate c WHERE c.candidate_id = ?1
          AND CASE WHEN json_valid(c.activation_scope_key)
            THEN json_extract(c.activation_scope_key, '$.run') = 'Live' ELSE 0 END
          AND json_extract(c.session_key_json, '$[0]') = 'native'
          AND json_extract(c.session_key_json, '$[1]') = 'claude-code'
          AND NOT EXISTS(SELECT 1 FROM session s JOIN burn_check_activation a
            ON a.check_id = c.check_id AND a.activation_scope_key = c.activation_scope_key
            WHERE json_array(s.environment_key, s.agent, s.session_id) = c.session_key_json
              AND s.cwd = json_extract(c.activation_scope_key, '$.session_cwd')
              AND a.generation = c.generation AND {proven} AND a.applied_sequence >= {boundary}
             AND c.selected_publication_head > {boundary}))"
        ),
        [candidate],
        |row| row.get(0),
    )?)
}

pub(crate) fn ensure_boundary_in(
    transaction: &Transaction<'_>,
    session: &SessionKey,
    generation: i64,
    fence: i64,
) -> Result<bool> {
    if session.environment_key != "native" || session.agent != "claude-code" {
        return Ok(false);
    }
    let boundary = boundary_sql("s");
    let sequence: Option<i64> = transaction.query_row(&format!("INSERT INTO native_tool_result_publication
        (outcome, environment_key, agent, session_id, source_format, complete,
         source_generation, claim_fence, project_cwd, session_incarnation, gap_reason, gap_kind)
        SELECT 'coverage_gap', s.environment_key, s.agent, s.session_id, ?4, 0,
            ?5, ?6, s.cwd, s.incarnation, ?7, 'segment_reset'
          FROM session s WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
             AND s.source_kind = 'file' AND s.wsl_distro IS NULL AND s.cwd IS NOT NULL AND s.cwd <> ''
             AND (s.source_format IS NULL OR s.source_format = ?4)
             AND {boundary} IS NULL RETURNING publication_sequence"),
        params![session.environment_key, session.agent, session.session_id, FORMAT,
            generation, fence, RESET_REASON], |row| row.get(0)).optional()?;
    let Some(sequence) = sequence else {
        return Ok(false);
    };
    // Keep one binding per session. The marker remains in the append-only journal.
    transaction.execute(
        "INSERT INTO setting(key, value)
         SELECT ?4, json_object('publication_sequence', ?5, 'source_label', source_label,
           'incarnation', incarnation, 'cwd', cwd, 'source_format', 'claude_jsonl')
         FROM session WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![
            session.environment_key,
            session.agent,
            session.session_id,
            binding_key(session),
            sequence
        ],
    )?;
    Ok(true)
}

pub(crate) fn delete_session_binding_in(
    connection: &Connection,
    session: &SessionKey,
) -> Result<()> {
    connection.execute("DELETE FROM setting WHERE key = ?1", [binding_key(session)])?;
    Ok(())
}

pub(crate) fn clear_bindings_in(connection: &Connection) -> Result<()> {
    connection.execute(
        "DELETE FROM setting WHERE key LIKE ?1",
        [format!("{BINDING_PREFIX}%")],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
