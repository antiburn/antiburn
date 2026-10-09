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
            if forked || coverage.thread_parent_unresolved {
                return Ok(Err(missing(ScopeMissingReason::BranchUnresolved)));
            }
            if coverage.coverage_schema_revision != COVERAGE_SCHEMA_REVISION
                || !matches!(coverage.source_acceptance, SourceAcceptance::AcceptedFull | SourceAcceptance::AcceptedPrefix { .. })
                || coverage.ordering != OrderingObservation::Monotonic
                || coverage.diagnostics.duplicate_turn_identities != 0
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
            {
                return Ok(Err(missing(ScopeMissingReason::IncompleteSource)));
            }

            // Check unselected rows too. Selection excludes unknown user authority.
            let known_context_complete = known_context::validate(connection, key, fence, format)?;
            let (rows, invalid): (i64, i64) = connection.query_row(
                "SELECT COUNT(*), COALESCE(SUM(CASE WHEN
                     (scope = 'main' AND source_key != ?3)
                      OR (scope = 'main' AND ?5 = 'open_code_sqlite_v2' AND (uuid IS NULL OR uuid = ''))
                      OR (scope = 'main' AND thread_id != ?6)
                     THEN 1 ELSE 0 END), 0)
                 FROM turn WHERE environment_key = ?1 AND agent = ?2
                      AND session_id = ?3 AND claim_fence = ?4 AND scope = 'main'",
                 params![key.environment_key, key.agent, key.session_id, fence, serde_json::to_value(format).expect("source format serializes").as_str().expect("source format string"), thread_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if invalid != 0 || rows <= 0 || rows > 65_536 {
                return Ok(Err(missing(ScopeMissingReason::IncompleteSource)));
            }
            let retained_context_loss: bool = connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM turn t
                 WHERE t.environment_key = ?1 AND t.agent = ?2 AND t.session_id = ?3
                  AND t.claim_fence = ?4 AND t.scope = 'main' AND (
                    t.is_compaction_boundary != 0
                    OR (t.role = 'user' AND NOT EXISTS (
                        SELECT 1 FROM turn_content c WHERE c.turn_rowid = t.rowid
                         AND (c.kind = 'user' OR (?5 = 'claude_jsonl' AND c.kind = 'tool_result'))))
                    OR EXISTS (SELECT 1 FROM turn_content c WHERE c.turn_rowid = t.rowid
                        AND c.kind IN ('user', 'assistant') AND
                         (c.truncated != 0 OR length(c.content) > ?6))))",
                params![key.environment_key, key.agent, key.session_id, fence,
                    serde_json::to_value(format).expect("source format serializes").as_str().expect("source format string"),
                    antiburn_local::analysis::MAX_CONTENT_PART_BYTES],
                |row| row.get(0),
            )?;
            let untrusted_user_parts = known_context::untrusted_user_parts(connection, key, fence, format)?;
            let source_complete = untrusted_user_parts.is_empty() && known_context_complete && !retained_context_loss
                && (!matches!(format, SourceFormat::OpenCodeSqliteV2 | SourceFormat::ClaudeJsonl) || first_role == "user")
                && coverage.source_acceptance == SourceAcceptance::AcceptedFull
                && coverage.summary_observed
                && coverage.record_loss_reason.is_none()
                && coverage.child_loss_reason.is_none()
                && !coverage.session_cap_exceeded
                && !coverage.subagent_linkage_incomplete
                && !coverage.thread_parent_unresolved
                && !coverage.subagents_cap_exceeded
                && coverage.diagnostics.records_unusable == 0
                && coverage.diagnostics.unusable_reasons.is_empty()
                && coverage.diagnostics.truncated_strings.is_empty()
                && coverage.diagnostics.capped_collections.is_empty()
                && rows as u64 == coverage.diagnostics.records_observed;
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
                source_complete,
                untrusted_user_parts,
            }))
        })
        .map_err(ScopeLoadError::Query)?
        .ok_or_else(|| missing(ScopeMissingReason::PublicationChanged))?
    }
}
