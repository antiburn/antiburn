//! Shared durable state for session-scoped Burn Check assessments.

mod cache;
mod usage;

pub use cache::CachedAssessmentResponse;
use cache::RESPONSE_CACHE_KEY;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{SessionKey, SessionRecord, Store, session_from_row};

const ENABLED_AT_KEY: &str = "internal:burnChecksEnabledAtEpochV1";
const USAGE_LEDGER_KEY: &str = "internal:burnCheckUsageLedgerV1";
const USAGE_WINDOW_SECS: i64 = 24 * 60 * 60;
const GLOBAL_INPUT_TOKEN_LIMIT: u64 = 5_000_000;
const SESSION_INPUT_TOKEN_LIMIT: u64 = 250_000;
const MAX_USAGE_LEDGER_ENTRIES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnCheckInput {
    pub key: SessionKey,
    pub check_id: String,
    pub incarnation: u64,
    pub source_generation: i64,
    pub source_fingerprint: Option<String>,
    pub activity_cursor: String,
    pub published_fence: i64,
    pub input_revision: String,
    pub evaluator_revision: String,
    pub boundary_at_epoch: i64,
}

pub struct BurnCheckFailure<'a> {
    pub error_category: &'a str,
    pub result_json: &'a str,
    pub progress_json: &'a str,
    pub retry_at_epoch: Option<i64>,
}

type CurrentAssessmentSource = (u64, i64, Option<String>, String, Option<i64>, i64);
type ExistingAssessmentState = (Option<String>, String, Option<i64>, Option<i64>);

#[derive(Debug, Clone, PartialEq)]
pub struct BurnCheckCandidate {
    pub session: SessionRecord,
    pub incarnation: u64,
    pub source_generation: i64,
    pub source_fingerprint: Option<String>,
    pub activity_cursor: String,
    pub published_fence: i64,
    pub boundary_at_epoch: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnCheckAssessment {
    pub key: SessionKey,
    pub check_id: String,
    pub input_revision: Option<String>,
    pub status: String,
    pub progress_json: String,
    pub result_json: Option<String>,
    pub result_revision: Option<String>,
    pub request_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BurnCheckReservation {
    Reserved(String),
    UsageLimitReached,
    RequestLimitReached,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct UsageLedger {
    reservations: Vec<UsageReservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UsageReservation {
    id: String,
    session_key: String,
    input_tokens: u64,
    expires_at_epoch: i64,
}

impl Store {
    /// Capture the current source cursor for each check before enabling paid work.
    pub fn capture_burn_check_boundaries(
        &self,
        check_ids: &[&str],
        now_epoch: i64,
    ) -> anyhow::Result<usize> {
        if check_ids.is_empty() {
            return Ok(0);
        }
        if check_ids.iter().any(|id| id.is_empty() || id.len() > 256) {
            anyhow::bail!("Burn Check ID is invalid");
        }

        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![ENABLED_AT_KEY, now_epoch.to_string()],
        )?;
        let mut sessions = transaction.prepare(
            "SELECT environment_key, agent, session_id, incarnation,
                    source_generation, activity_cursor
               FROM session",
        )?;
        let rows = sessions
            .query_map([], |row| {
                Ok((
                    SessionKey::new(
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ),
                    row.get::<_, u64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(sessions);

        for check_id in check_ids {
            for (key, incarnation, generation, cursor) in &rows {
                transaction.execute(
                    "INSERT INTO burn_check_assessment (
                         environment_key, agent, session_id, check_id, incarnation,
                         boundary_generation, boundary_activity_cursor, boundary_at_epoch,
                         status, created_at_epoch, updated_at_epoch)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'idle', ?8, ?8)
                     ON CONFLICT(environment_key, agent, session_id, check_id) DO UPDATE SET
                         incarnation = excluded.incarnation,
                         boundary_generation = excluded.boundary_generation,
                         boundary_activity_cursor = excluded.boundary_activity_cursor,
                         boundary_at_epoch = excluded.boundary_at_epoch,
                         input_revision = NULL,
                         evaluator_revision = NULL,
                         source_generation = NULL,
                         source_fingerprint = NULL,
                         published_fence = NULL,
                         status = 'idle',
                         progress_json = '{}',
                         result_json = NULL,
                         result_revision = NULL,
                         request_count = 0,
                         updated_at_epoch = excluded.updated_at_epoch,
                         next_attempt_at_epoch = NULL,
                         lease_expires_at_epoch = NULL,
                         last_error_category = NULL",
                    rusqlite::params![
                        key.environment_key,
                        key.agent,
                        key.session_id,
                        check_id,
                        incarnation,
                        generation,
                        cursor,
                        now_epoch,
                    ],
                )?;
            }
        }
        transaction.commit()?;
        Ok(rows.len().saturating_mul(check_ids.len()))
    }

    /// Stop new work while preserving completed assessment results.
    pub fn disable_burn_checks(&self) -> anyhow::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM setting WHERE key = ?1", [ENABLED_AT_KEY])?;
        transaction.execute(
            "UPDATE burn_check_assessment
                SET status = 'superseded', progress_json = '{}',
                    lease_expires_at_epoch = NULL, next_attempt_at_epoch = NULL
              WHERE status IN ('queued', 'running')",
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Read quiet sessions whose source cursor changed after enablement.
    pub fn burn_check_candidates(
        &self,
        check_id: &str,
        now_epoch: i64,
        idle_secs: i64,
        limit: usize,
    ) -> anyhow::Result<Vec<BurnCheckCandidate>> {
        let connection = self.lock();
        let Some(enabled_at) = internal_value_in(&connection, ENABLED_AT_KEY)?
            .and_then(|value| value.parse::<i64>().ok())
        else {
            return Ok(Vec::new());
        };
        let sql = "SELECT s.environment_key, s.agent, s.session_id, s.source_kind,
                    s.source_label, s.wsl_distro, s.title, s.title_source, s.cwd,
                    s.surface, s.updated_at_epoch, s.activity_cursor, s.activity_source,
                    s.subagent_count,
                    (SELECT related_id FROM session_relation r
                      WHERE r.environment_key = s.environment_key AND r.agent = s.agent
                        AND r.session_id = s.session_id AND r.kind = 'forkParent' LIMIT 1),
                    s.source_fingerprint,
                    s.incarnation, s.source_generation, s.source_fingerprint,
                    s.activity_cursor, evidence.published_fence, assessment.boundary_at_epoch
               FROM session s
             LEFT JOIN burn_check_assessment AS assessment
               ON assessment.environment_key = s.environment_key
              AND assessment.agent = s.agent
              AND assessment.session_id = s.session_id
              AND assessment.check_id = ?1
             JOIN session_evidence AS evidence
               ON evidence.environment_key = s.environment_key
              AND evidence.agent = s.agent
              AND evidence.session_id = s.session_id
              AND evidence.status = 'ready'
              AND evidence.analyzed_generation = s.source_generation
              AND evidence.processed_fingerprint IS s.source_fingerprint
              AND evidence.parser_revision = ?2
              AND evidence.evidence_schema_revision = ?3
              AND evidence.published_fence IS NOT NULL
             WHERE s.updated_at_epoch IS NOT NULL
               AND s.updated_at_epoch <= ?4 - ?5
               AND (
                     (assessment.boundary_generation IS NOT NULL
                      AND (assessment.boundary_generation < 0
                           OR s.activity_cursor <> assessment.boundary_activity_cursor)
                      AND s.updated_at_epoch >= assessment.boundary_at_epoch)
                    OR
                    (assessment.boundary_generation IS NULL
                     AND s.updated_at_epoch >= ?6)
               )
               AND (assessment.status IS NULL
                    OR (assessment.status <> 'queued'
                        AND (assessment.status <> 'running'
                             OR assessment.lease_expires_at_epoch <= ?4)))
               AND (assessment.next_attempt_at_epoch IS NULL
                    OR assessment.next_attempt_at_epoch <= ?4)
             ORDER BY s.updated_at_epoch, s.environment_key, s.agent, s.session_id
             LIMIT ?7";
        let mut statement = connection.prepare(sql)?;
        let candidates = statement
            .query_map(
                rusqlite::params![
                    check_id,
                    antiburn_local::analysis::PARSER_REVISION,
                    antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                    now_epoch,
                    idle_secs.max(0),
                    enabled_at,
                    i64::try_from(limit.min(256)).unwrap_or(256),
                ],
                |row| {
                    let session = session_from_row(row)?;
                    let incarnation = row.get(16)?;
                    let source_generation = row.get(17)?;
                    let source_fingerprint = row.get(18)?;
                    let activity_cursor = row.get(19)?;
                    let published_fence = row.get(20)?;
                    let boundary_at_epoch = row.get::<_, Option<i64>>(21)?.unwrap_or(enabled_at);
                    Ok(BurnCheckCandidate {
                        session,
                        incarnation,
                        source_generation,
                        source_fingerprint,
                        activity_cursor,
                        published_fence,
                        boundary_at_epoch,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(candidates)
    }

    /// Queue a fresh immutable input, preserving only progress for that exact revision.
    pub fn queue_burn_check_assessment(
        &self,
        input: &BurnCheckInput,
        now_epoch: i64,
        idle_secs: i64,
    ) -> anyhow::Result<bool> {
        if input.check_id.is_empty()
            || input.check_id.len() > 256
            || input.input_revision.is_empty()
            || input.evaluator_revision.is_empty()
        {
            anyhow::bail!("Burn Check assessment identity is invalid");
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        if internal_value_in(&transaction, ENABLED_AT_KEY)?.is_none() {
            transaction.commit()?;
            return Ok(false);
        }
        let current: Option<CurrentAssessmentSource> = transaction
            .query_row(
                "SELECT s.incarnation, s.source_generation, s.source_fingerprint,
                        s.activity_cursor, e.published_fence, s.updated_at_epoch
                   FROM session AS s
                   JOIN session_evidence AS e
                     ON e.environment_key = s.environment_key AND e.agent = s.agent
                    AND e.session_id = s.session_id
                    AND e.status = 'ready'
                    AND e.analyzed_generation = s.source_generation
                    AND e.processed_fingerprint IS s.source_fingerprint
                    AND e.parser_revision = ?4
                    AND e.evidence_schema_revision = ?5
                  WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3",
                rusqlite::params![
                    input.key.environment_key,
                    input.key.agent,
                    input.key.session_id,
                    antiburn_local::analysis::PARSER_REVISION,
                    antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                ],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((incarnation, generation, fingerprint, cursor, fence, updated_at)) = current
        else {
            transaction.commit()?;
            return Ok(false);
        };
        if incarnation != input.incarnation
            || generation != input.source_generation
            || fingerprint != input.source_fingerprint
            || cursor != input.activity_cursor
            || fence != Some(input.published_fence)
            || updated_at < input.boundary_at_epoch
            || updated_at > now_epoch.saturating_sub(idle_secs.max(0))
        {
            transaction.commit()?;
            return Ok(false);
        }

        let old: Option<ExistingAssessmentState> = transaction
            .query_row(
                "SELECT input_revision, status, next_attempt_at_epoch, lease_expires_at_epoch
                   FROM burn_check_assessment
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = ?4",
                rusqlite::params![
                    input.key.environment_key,
                    input.key.agent,
                    input.key.session_id,
                    input.check_id,
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if let Some((Some(old_revision), status, next_attempt, lease)) = &old
            && old_revision == &input.input_revision
            && (status == "queued"
                || status == "completed"
                || (status == "running" && lease.is_some_and(|lease| lease > now_epoch))
                || next_attempt.is_some_and(|attempt| attempt > now_epoch))
        {
            transaction.commit()?;
            return Ok(false);
        }

        let initial_boundary = old.is_none();
        transaction.execute(
            "INSERT INTO burn_check_assessment (
                 environment_key, agent, session_id, check_id, incarnation,
                 boundary_generation, boundary_activity_cursor, boundary_at_epoch,
                 input_revision, evaluator_revision, source_generation, source_fingerprint,
                 published_fence, status, progress_json, result_json, result_revision,
                 request_count, created_at_epoch, updated_at_epoch)
             VALUES (?1, ?2, ?3, ?4, ?5,
                     CASE WHEN ?15 THEN -1 ELSE ?6 END,
                     CASE WHEN ?15 THEN '' ELSE ?7 END,
                     ?8, ?9, ?10, ?11, ?12, ?13,
                     'queued', '{}', NULL, NULL, 0, ?14, ?14)
             ON CONFLICT(environment_key, agent, session_id, check_id) DO UPDATE SET
                 incarnation = excluded.incarnation,
                 boundary_generation = CASE WHEN ?15 THEN -1 ELSE burn_check_assessment.boundary_generation END,
                 boundary_activity_cursor = CASE WHEN ?15 THEN '' ELSE burn_check_assessment.boundary_activity_cursor END,
                 boundary_at_epoch = COALESCE(burn_check_assessment.boundary_at_epoch, ?8),
                 input_revision = excluded.input_revision,
                 evaluator_revision = excluded.evaluator_revision,
                 source_generation = excluded.source_generation,
                 source_fingerprint = excluded.source_fingerprint,
                 published_fence = excluded.published_fence,
                 status = 'queued',
                 progress_json = CASE
                     WHEN burn_check_assessment.input_revision = excluded.input_revision
                     THEN burn_check_assessment.progress_json ELSE '{}' END,
                 result_json = burn_check_assessment.result_json,
                 result_revision = burn_check_assessment.result_revision,
                 request_count = CASE
                     WHEN burn_check_assessment.input_revision = excluded.input_revision
                     THEN burn_check_assessment.request_count ELSE 0 END,
                 updated_at_epoch = excluded.updated_at_epoch,
                 next_attempt_at_epoch = NULL,
                 lease_expires_at_epoch = NULL,
                 last_error_category = NULL",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id,
                input.incarnation,
                input.source_generation,
                input.activity_cursor,
                input.boundary_at_epoch,
                input.input_revision,
                input.evaluator_revision,
                input.source_generation,
                input.source_fingerprint,
                input.published_fence,
                now_epoch,
                initial_boundary,
            ],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    /// Claim or recover one exact assessment revision.
    pub fn claim_burn_check_assessment(
        &self,
        input: &BurnCheckInput,
        now_epoch: i64,
        lease_secs: i64,
        idle_secs: i64,
    ) -> anyhow::Result<bool> {
        let connection = self.lock();
        let updated = connection.execute(
            "UPDATE burn_check_assessment
                SET status = 'running', lease_expires_at_epoch = ?8,
                    updated_at_epoch = ?7
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND check_id = ?4 AND input_revision = ?5
                AND (status = 'queued' OR
                     (status = 'running' AND lease_expires_at_epoch <= ?7))
                AND (next_attempt_at_epoch IS NULL OR next_attempt_at_epoch <= ?7)
                AND EXISTS (
                    SELECT 1 FROM session AS s
                    JOIN session_evidence AS e
                      ON e.environment_key = s.environment_key AND e.agent = s.agent
                     AND e.session_id = s.session_id
                      AND e.status = 'ready'
                      AND e.analyzed_generation = s.source_generation
                      AND e.processed_fingerprint IS s.source_fingerprint
                      AND e.published_fence = ?6
                      AND e.parser_revision = ?9
                      AND e.evidence_schema_revision = ?10
                    WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
                      AND s.incarnation = ?11
                      AND s.source_generation = ?12
                      AND s.source_fingerprint IS ?13
                      AND s.activity_cursor = ?14
                      AND s.updated_at_epoch <= ?7 - ?15
                )",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id,
                input.input_revision,
                input.published_fence,
                now_epoch,
                now_epoch.saturating_add(lease_secs.max(1)),
                antiburn_local::analysis::PARSER_REVISION,
                antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                input.incarnation,
                input.source_generation,
                input.source_fingerprint,
                input.activity_cursor,
                idle_secs.max(0),
            ],
        )?;
        Ok(updated == 1)
    }

    /// Renew a lease only while the session and publication remain unchanged and idle.
    pub fn renew_burn_check_assessment(
        &self,
        input: &BurnCheckInput,
        now_epoch: i64,
        lease_secs: i64,
        idle_secs: i64,
    ) -> anyhow::Result<bool> {
        let connection = self.lock();
        let updated = connection.execute(
            "UPDATE burn_check_assessment
                SET lease_expires_at_epoch = ?8, updated_at_epoch = ?7
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND check_id = ?4 AND input_revision = ?5 AND status = 'running'
                AND lease_expires_at_epoch > ?7
                AND EXISTS (
                    SELECT 1 FROM session AS s
                    JOIN session_evidence AS e
                      ON e.environment_key = s.environment_key AND e.agent = s.agent
                     AND e.session_id = s.session_id
                     AND e.status = 'ready'
                      AND e.analyzed_generation = s.source_generation
                      AND e.processed_fingerprint IS s.source_fingerprint
                      AND e.published_fence = ?6
                      AND e.parser_revision = ?14
                      AND e.evidence_schema_revision = ?15
                    WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
                      AND s.incarnation = ?9 AND s.source_generation = ?10
                      AND s.source_fingerprint IS ?11 AND s.activity_cursor = ?12
                      AND s.updated_at_epoch <= ?7 - ?13
                )",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id,
                input.input_revision,
                input.published_fence,
                now_epoch,
                now_epoch.saturating_add(lease_secs.max(1)),
                input.incarnation,
                input.source_generation,
                input.source_fingerprint,
                input.activity_cursor,
                idle_secs.max(0),
                antiburn_local::analysis::PARSER_REVISION,
                antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
            ],
        )?;
        Ok(updated == 1)
    }

    /// Save typed, source-free progress only for a still-current active revision.
    pub fn save_burn_check_progress(
        &self,
        input: &BurnCheckInput,
        progress_json: &str,
        now_epoch: i64,
        lease_secs: i64,
        idle_secs: i64,
    ) -> anyhow::Result<bool> {
        if progress_json.len() > 512 * 1024 || serde_json::from_str::<Value>(progress_json).is_err()
        {
            anyhow::bail!("Burn Check progress is invalid or too large");
        }
        let connection = self.lock();
        let updated = connection.execute(
            "UPDATE burn_check_assessment
                SET progress_json = ?6, updated_at_epoch = ?7,
                    lease_expires_at_epoch = ?8
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND check_id = ?4 AND input_revision = ?5 AND status = 'running'
                AND lease_expires_at_epoch > ?7
                AND EXISTS (
                    SELECT 1 FROM session AS s
                    JOIN session_evidence AS e
                      ON e.environment_key = s.environment_key AND e.agent = s.agent
                     AND e.session_id = s.session_id
                     AND e.status = 'ready'
                     AND e.analyzed_generation = s.source_generation
                     AND e.processed_fingerprint IS s.source_fingerprint
                     AND e.published_fence = ?9
                     AND e.parser_revision = ?15
                     AND e.evidence_schema_revision = ?16
                    WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
                      AND s.incarnation = ?10 AND s.source_generation = ?11
                      AND s.source_fingerprint IS ?12 AND s.activity_cursor = ?13
                      AND s.updated_at_epoch <= ?7 - ?14
                )",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id,
                input.input_revision,
                progress_json,
                now_epoch,
                now_epoch.saturating_add(lease_secs.max(1)),
                input.published_fence,
                input.incarnation,
                input.source_generation,
                input.source_fingerprint,
                input.activity_cursor,
                idle_secs.max(0),
                antiburn_local::analysis::PARSER_REVISION,
                antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
            ],
        )?;
        Ok(updated == 1)
    }

    /// Publish a compact result only while the exact source snapshot is still idle.
    pub fn complete_burn_check_assessment(
        &self,
        input: &BurnCheckInput,
        result_json: &str,
        now_epoch: i64,
        idle_secs: i64,
    ) -> anyhow::Result<bool> {
        if result_json.len() > 512 * 1024 || serde_json::from_str::<Value>(result_json).is_err() {
            anyhow::bail!("Burn Check result is invalid or too large");
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let updated = transaction.execute(
            "UPDATE burn_check_assessment
                SET status = 'completed', result_json = ?6, result_revision = ?5,
                    progress_json = '{}', boundary_generation = ?9,
                    boundary_activity_cursor = ?10, lease_expires_at_epoch = NULL,
                    next_attempt_at_epoch = NULL, last_error_category = NULL,
                    updated_at_epoch = ?7
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND check_id = ?4 AND input_revision = ?5 AND status = 'running'
                AND lease_expires_at_epoch > ?7
                AND EXISTS (
                    SELECT 1 FROM session AS s
                    JOIN session_evidence AS e
                      ON e.environment_key = s.environment_key AND e.agent = s.agent
                     AND e.session_id = s.session_id
                     AND e.status = 'ready'
                     AND e.analyzed_generation = s.source_generation
                     AND e.processed_fingerprint IS s.source_fingerprint
                     AND e.published_fence = ?8
                      AND e.parser_revision = ?15
                      AND e.evidence_schema_revision = ?16
                    WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
                      AND s.incarnation = ?11 AND s.source_generation = ?12
                      AND s.source_fingerprint IS ?13 AND s.activity_cursor = ?10
                      AND s.updated_at_epoch <= ?7 - ?14
                )",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id,
                input.input_revision,
                result_json,
                now_epoch,
                input.published_fence,
                input.source_generation,
                input.activity_cursor,
                input.incarnation,
                input.source_generation,
                input.source_fingerprint,
                idle_secs.max(0),
                antiburn_local::analysis::PARSER_REVISION,
                antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
            ],
        )?;
        if updated == 0 {
            transaction.execute(
                "UPDATE burn_check_assessment
                    SET status = 'superseded', progress_json = '{}',
                        lease_expires_at_epoch = NULL, updated_at_epoch = ?6
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                    AND check_id = ?4 AND input_revision = ?5 AND status = 'running'",
                rusqlite::params![
                    input.key.environment_key,
                    input.key.agent,
                    input.key.session_id,
                    input.check_id,
                    input.input_revision,
                    now_epoch,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(updated == 1)
    }

    /// Retain a typed partial result and exact progress after a remote failure.
    pub fn fail_burn_check_assessment_with_result(
        &self,
        input: &BurnCheckInput,
        failure: &BurnCheckFailure<'_>,
        now_epoch: i64,
        idle_secs: i64,
    ) -> anyhow::Result<bool> {
        if failure.error_category.is_empty()
            || failure.error_category.len() > 64
            || failure.result_json.len() > 512 * 1024
            || failure.progress_json.len() > 512 * 1024
            || serde_json::from_str::<Value>(failure.result_json).is_err()
            || serde_json::from_str::<Value>(failure.progress_json).is_err()
        {
            anyhow::bail!("Burn Check failure state is invalid or too large");
        }
        let connection = self.lock();
        let updated = connection.execute(
            "UPDATE burn_check_assessment
                SET status = 'failed', next_attempt_at_epoch = ?6,
                    result_json = ?7, result_revision = ?5, progress_json = ?8,
                    lease_expires_at_epoch = NULL, last_error_category = ?9,
                    updated_at_epoch = ?10
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND check_id = ?4 AND input_revision = ?5 AND status = 'running'
                AND lease_expires_at_epoch > ?10
                AND EXISTS (
                    SELECT 1 FROM session AS s
                    JOIN session_evidence AS e
                      ON e.environment_key = s.environment_key AND e.agent = s.agent
                     AND e.session_id = s.session_id
                     AND e.status = 'ready'
                     AND e.analyzed_generation = s.source_generation
                     AND e.processed_fingerprint IS s.source_fingerprint
                     AND e.published_fence = ?11
                     AND e.parser_revision = ?17
                     AND e.evidence_schema_revision = ?18
                    WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
                      AND s.incarnation = ?12 AND s.source_generation = ?13
                      AND s.source_fingerprint IS ?14 AND s.activity_cursor = ?15
                      AND s.updated_at_epoch <= ?10 - ?16
                )",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id,
                input.input_revision,
                failure.retry_at_epoch,
                failure.result_json,
                failure.progress_json,
                failure.error_category,
                now_epoch,
                input.published_fence,
                input.incarnation,
                input.source_generation,
                input.source_fingerprint,
                input.activity_cursor,
                idle_secs.max(0),
                antiburn_local::analysis::PARSER_REVISION,
                antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
            ],
        )?;
        Ok(updated == 1)
    }

    /// Stop active dispatch without storing a result for resumed or disabled work.
    pub fn supersede_burn_check_assessment(
        &self,
        input: &BurnCheckInput,
        now_epoch: i64,
    ) -> anyhow::Result<bool> {
        let connection = self.lock();
        let updated = connection.execute(
            "UPDATE burn_check_assessment
                SET status = 'superseded', lease_expires_at_epoch = NULL,
                    next_attempt_at_epoch = NULL, last_error_category = 'cancelled',
                    updated_at_epoch = ?6
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND check_id = ?4 AND input_revision = ?5
                AND status IN ('queued', 'running')",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id,
                input.input_revision,
                now_epoch,
            ],
        )?;
        Ok(updated == 1)
    }

    /// Read one check's durable progress/result without reading private source content.
    pub fn burn_check_assessment(
        &self,
        key: &SessionKey,
        check_id: &str,
    ) -> anyhow::Result<Option<BurnCheckAssessment>> {
        let connection = self.lock();
        Ok(connection
            .query_row(
                "SELECT input_revision, status, progress_json, result_json,
                        result_revision, request_count
                   FROM burn_check_assessment
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = ?4",
                rusqlite::params![key.environment_key, key.agent, key.session_id, check_id],
                |row| {
                    Ok(BurnCheckAssessment {
                        key: key.clone(),
                        check_id: check_id.to_owned(),
                        input_revision: row.get(0)?,
                        status: row.get(1)?,
                        progress_json: row.get(2)?,
                        result_json: row.get(3)?,
                        result_revision: row.get(4)?,
                        request_count: usize::try_from(row.get::<_, i64>(5)?).unwrap_or_default(),
                    })
                },
            )
            .optional()?)
    }
}

fn digest_parts<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    let mut output = String::with_capacity(64);
    for byte in digest.finalize() {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn internal_value_in(
    connection: &rusqlite::Connection,
    key: &str,
) -> anyhow::Result<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT value FROM setting WHERE key = ?1",
            rusqlite::params![key],
            |row| row.get(0),
        )
        .optional()?)
}

fn write_json_setting(
    connection: &rusqlite::Connection,
    key: &str,
    value: &impl Serialize,
) -> anyhow::Result<()> {
    let serialized = serde_json::to_string(value)?;
    connection.execute(
        "INSERT INTO setting (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, serialized],
    )?;
    Ok(())
}

pub(super) fn clear_local_burn_check_state(
    connection: &rusqlite::Connection,
    now_epoch: i64,
) -> anyhow::Result<()> {
    let enabled = internal_value_in(connection, ENABLED_AT_KEY)?.is_some();
    connection.execute("DELETE FROM burn_check_assessment", [])?;
    connection.execute(
        "DELETE FROM setting WHERE key IN (?1, ?2)",
        rusqlite::params![USAGE_LEDGER_KEY, RESPONSE_CACHE_KEY],
    )?;
    if enabled {
        connection.execute(
            "UPDATE setting SET value = ?2 WHERE key = ?1",
            rusqlite::params![ENABLED_AT_KEY, now_epoch.to_string()],
        )?;
    }
    Ok(())
}
