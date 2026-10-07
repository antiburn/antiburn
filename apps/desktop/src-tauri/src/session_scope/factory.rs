use antiburn_local::analysis::session_scope::SessionScopeBranch;
use antiburn_local::analysis::{
    COVERAGE_SCHEMA_REVISION, FenceScope, OrderingObservation, SourceAcceptance, SourceKind,
    query_coverage_record,
};
use rusqlite::{OptionalExtension, params};

use super::*;

#[path = "known_context.rs"]
mod known_context;

pub(super) fn turn_session_key(key: &SessionKey) -> antiburn_local::analysis::TurnSessionKey<'_> {
    antiburn_local::analysis::TurnSessionKey {
        environment_key: &key.environment_key,
        agent: &key.agent,
        session_id: &key.session_id,
    }
}

fn missing(reason: ScopeMissingReason) -> ScopeLoadError {
    ScopeLoadError::Scope(SessionScopeError::Missing(reason))
}

impl Store {
    /// Derive retained root history from an accepted source contract.
    /// The fence and generation identify the assessment, not its enrollment watermark.
    pub fn session_scope_request(
        &self,
        key: &SessionKey,
        publication_fence: i64,
        source_generation: i64,
    ) -> Result<ScopeLoadRequest, ScopeLoadError> {
        self.with_published_content(key, source_generation, |connection, fence| {
            if fence != publication_fence {
                return Ok(Err(missing(ScopeMissingReason::PublicationChanged)));
            }
            let Some(coverage) = query_coverage_record(
                connection,
                &turn_session_key(key),
                &FenceScope::single(fence),
            )?
            else {
                return Ok(Err(missing(ScopeMissingReason::IncompleteSource)));
            };
            let format = coverage.capabilities.source_format;
            let accepted_source = matches!(
                (key.agent.as_str(), format, coverage.source_kind),
                ("opencode", SourceFormat::OpenCodeSqliteV2, SourceKind::Sqlite)
                    | ("codex", SourceFormat::CodexRolloutJsonl, SourceKind::Jsonl | SourceKind::File)
                    | ("claude-code", SourceFormat::ClaudeJsonl, SourceKind::Jsonl | SourceKind::File)
                    | ("pi", SourceFormat::PiV3Jsonl, SourceKind::Jsonl | SourceKind::File)
            );
            if !accepted_source
                || coverage.identity.agent != key.agent
                || coverage.identity.session_id != key.session_id
            {
                return Ok(Err(missing(ScopeMissingReason::UnsupportedSource)));
            }
            let forked: bool = connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM session_relation
                 WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                 AND kind = 'forkParent')",
                params![key.environment_key, key.agent, key.session_id],
                |row| row.get(0),
            )?;
            if forked
                || coverage.coverage_schema_revision != COVERAGE_SCHEMA_REVISION
                || coverage.source_acceptance != SourceAcceptance::AcceptedFull
                || coverage.ordering != OrderingObservation::Monotonic
                || !coverage.summary_observed
                || coverage.record_loss_reason.is_some()
                || coverage.child_loss_reason.is_some()
                || coverage.session_cap_exceeded
                || coverage.thread_parent_unresolved
                || coverage.subagent_linkage_incomplete
                || coverage.diagnostics.records_unusable != 0
                || coverage.diagnostics.duplicate_turn_identities != 0
                || !coverage.diagnostics.unusable_reasons.is_empty()
                || !coverage.diagnostics.truncated_strings.is_empty()
                || !coverage.diagnostics.capped_collections.is_empty()
            {
                return Ok(Err(missing(ScopeMissingReason::IncompleteSource)));
            }

            let root: Option<(String, String)> = connection.query_row(
                "SELECT thread_id, role FROM turn WHERE environment_key = ?1 AND agent = ?2
                 AND session_id = ?3 AND claim_fence = ?4 AND scope = 'main'
                 ORDER BY turn_index LIMIT 1",
                params![key.environment_key, key.agent, key.session_id, fence],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            let Some((thread_id, first_role)) = root else {
                return Ok(Err(missing(ScopeMissingReason::IncompleteSource)));
            };
            if thread_id.is_empty()
                || matches!(format, SourceFormat::OpenCodeSqliteV2 | SourceFormat::CodexRolloutJsonl) && thread_id != key.session_id
                || matches!(format, SourceFormat::OpenCodeSqliteV2 | SourceFormat::ClaudeJsonl) && first_role != "user"
            {
                return Ok(Err(missing(ScopeMissingReason::IncompleteSource)));
            }

            // Check unselected rows too. Selection excludes unknown user authority.
            if !known_context::validate(connection, key, fence, format)? {
                return Ok(Err(missing(ScopeMissingReason::IncompleteSource)));
            }
            let (rows, invalid): (i64, i64) = connection.query_row(
                "SELECT COUNT(*), COALESCE(SUM(CASE WHEN
                     source_key != ?3
                      OR (scope = 'main' AND ?5 = 'open_code_sqlite_v2' AND (uuid IS NULL OR uuid = ''))
                     OR (scope = 'main' AND is_compaction_boundary != 0)
                     OR (scope = 'main' AND thread_id != ?6)
                     OR (scope = 'main' AND role = 'user' AND NOT EXISTS (
                          SELECT 1 FROM turn_content c WHERE c.turn_rowid = turn.rowid
                          AND (c.kind = 'user' OR (?5 = 'claude_jsonl' AND c.kind = 'tool_result'))))
                     OR (scope = 'main' AND EXISTS (
                         SELECT 1 FROM turn_content c WHERE c.turn_rowid = turn.rowid
                          AND c.kind = 'user' AND c.authority = 'user' AND (
                              json_extract(c.normalized_fields_json, '$.metadata.user_text_history.source_format') IS NOT ?5
                             OR json_extract(c.normalized_fields_json, '$.metadata.user_text_history.session_id') IS NOT ?3
                              OR json_extract(c.normalized_fields_json, '$.metadata.user_text_history.message_id') IS NOT COALESCE(turn.uuid, turn.message_id)
                              OR json_extract(c.normalized_fields_json, '$.metadata.user_text_history.revision') IS NOT 1
                              OR (?5 != 'open_code_sqlite_v2' AND NOT EXISTS (
                                  SELECT 1 FROM json_each(c.normalized_fields_json, '$.metadata.bindings') b
                                  WHERE json_extract(b.value, '$.field') = 'user_message'
                                  AND json_extract(b.value, '$.container') = 'record'
                                  AND json_extract(b.value, '$.native_record_id') = COALESCE(turn.uuid, turn.message_id)
                                  AND ((?5 = 'codex_rollout_jsonl' AND json_extract(b.value, '$.pointer') LIKE '/payload/content/%/text')
                                      OR (?5 IN ('claude_jsonl', 'pi_v3_jsonl') AND (
                                          json_extract(b.value, '$.pointer') = '/message/content'
                                          OR json_extract(b.value, '$.pointer') LIKE '/message/content/%/text')))
                                  AND json_extract(b.value, '$.start') >= 0
                                  AND json_extract(b.value, '$.end') <= 16777216
                                  AND json_extract(b.value, '$.end') - json_extract(b.value, '$.start') = length(c.content)
                              )))))
                     THEN 1 ELSE 0 END), 0)
                 FROM turn WHERE environment_key = ?1 AND agent = ?2
                     AND session_id = ?3 AND claim_fence = ?4",
                 params![key.environment_key, key.agent, key.session_id, fence, serde_json::to_value(format).expect("source format serializes").as_str().expect("source format string"), thread_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if invalid != 0 || rows <= 0 || rows > 65_536 || rows as u64 != coverage.diagnostics.records_observed {
                return Ok(Err(missing(ScopeMissingReason::IncompleteSource)));
            }
            let boundary = connection
                .query_row(
                    "SELECT t.turn_index, (SELECT MAX(c.part_index) FROM turn_content c
                     WHERE c.turn_rowid = t.rowid)
                 FROM turn t WHERE t.environment_key = ?1 AND t.agent = ?2
                     AND t.session_id = ?3 AND t.claim_fence = ?4
                      AND t.source_key = ?3 AND t.thread_id = ?5 AND t.scope = 'main'
                      AND EXISTS (SELECT 1 FROM turn_content c WHERE c.turn_rowid = t.rowid)
                 ORDER BY t.turn_index DESC LIMIT 1",
                    params![key.environment_key, key.agent, key.session_id, fence, thread_id],
                    |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Option<u32>>(1)?)),
                )
                .optional()?;
            let Some((turn_index, Some(part_index))) = boundary else {
                return Ok(Err(missing(ScopeMissingReason::BoundaryMissing)));
            };
            Ok(Ok(ScopeLoadRequest {
                key: key.clone(),
                source_format: coverage.capabilities.source_format,
                boundary: SessionScopeBoundary {
                    source_key: key.session_id.clone(),
                    thread_id,
                    turn_index,
                    part_index,
                    branch: SessionScopeBranch::ProvenLinear,
                },
                publication_fence: fence,
                source_generation,
                source_complete: true,
            }))
        })
        .map_err(ScopeLoadError::Query)?
        .ok_or_else(|| missing(ScopeMissingReason::PublicationChanged))?
    }
}
