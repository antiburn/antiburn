//! Shared durable state for session-scoped Burn Check assessments.

mod cache;
mod usage;

pub use cache::CachedAssessmentResponse;
use cache::RESPONSE_CACHE_KEY;
pub use usage::BurnCheckUsageSummary;
use usage::UsageLedgerSummary;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{SessionKey, SessionRecord, Store, session_from_row};

const ENABLED_AT_KEY: &str = "internal:burnChecksEnabledAtEpochV1";
const CHECK_ENABLED_AT_PREFIX: &str = "internal:burnCheckEnabledAtEpochV1:";
const HISTORY_BATCH_KEY: &str = "internal:jevBurnCheckHistoryBatchEpochV1";
const USAGE_LEDGER_KEY: &str = "internal:burnCheckUsageLedgerV1";
const USAGE_WINDOW_SECS: i64 = 24 * 60 * 60;
const MAX_RESULT_BYTES: usize = 1024 * 1024;
const MAX_PROGRESS_BYTES: usize = 2 * 1024 * 1024;

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
    pub boundary_positions: std::collections::BTreeMap<String, u64>,
    pub historical: bool,
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
pub struct BurnCheckSampledPair {
    pub comparison_id: String,
    pub dependency_digest: String,
    pub incarnation: u64,
    pub action_id: String,
    pub action_digest: String,
    pub instruction_digest: String,
    pub selector_revision: u32,
    pub round: u32,
    pub assessed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnCheckSampleOrigin {
    pub incarnation: u64,
    pub boundary_positions: std::collections::BTreeMap<String, u64>,
    pub boundary_at_epoch: i64,
    pub historical: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BurnCheckHistoryStatus {
    pub total: usize,
    pub waiting_for_data: usize,
    pub waiting_for_idle: usize,
    pub ready: usize,
    pub queued: usize,
    pub running: usize,
    pub completed: usize,
    pub skipped: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BurnCheckReservation {
    Reserved(String),
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct UsageLedger {
    reservations: Vec<UsageReservation>,
    #[serde(default)]
    summary: UsageLedgerSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UsageReservation {
    id: String,
    session_key: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    check_id: String,
    #[serde(default)]
    model: String,
    input_tokens: u64,
    expires_at_epoch: i64,
    #[serde(default)]
    settled: bool,
    #[serde(default)]
    unknown_recorded: bool,
}

impl Store {
    pub fn burn_check_sample_origin(
        &self,
        key: &SessionKey,
        check_id: &str,
    ) -> anyhow::Result<Option<BurnCheckSampleOrigin>> {
        let connection = self.lock();
        let row: Option<(u64, String, i64, bool)> = connection
            .query_row(
                "SELECT incarnation, boundary_positions_json, boundary_at_epoch, historical
               FROM burn_check_sample_origin
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = ?4",
                rusqlite::params![key.environment_key, key.agent, key.session_id, check_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        row.map(|(incarnation, positions, boundary_at_epoch, historical)| {
            Ok(BurnCheckSampleOrigin {
                incarnation,
                boundary_positions: serde_json::from_str(&positions)?,
                boundary_at_epoch,
                historical,
            })
        })
        .transpose()
    }

    pub fn observe_burn_check_sample_origin(
        &self,
        candidate: &BurnCheckCandidate,
        check_id: &str,
    ) -> anyhow::Result<BurnCheckSampleOrigin> {
        let connection = self.lock();
        connection.execute(
            "INSERT INTO burn_check_sample_origin
                (environment_key, agent, session_id, check_id, incarnation,
                 boundary_positions_json, boundary_at_epoch, historical)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(environment_key, agent, session_id, check_id) DO UPDATE SET
                 incarnation = excluded.incarnation,
                 boundary_positions_json = excluded.boundary_positions_json,
                 boundary_at_epoch = excluded.boundary_at_epoch,
                 historical = excluded.historical
             WHERE burn_check_sample_origin.incarnation <> excluded.incarnation",
            rusqlite::params![
                candidate.session.key.environment_key,
                candidate.session.key.agent,
                candidate.session.key.session_id,
                check_id,
                candidate.incarnation,
                serde_json::to_string(&candidate.boundary_positions)?,
                candidate.boundary_at_epoch,
                candidate.historical
            ],
        )?;
        drop(connection);
        self.burn_check_sample_origin(&candidate.session.key, check_id)?
            .ok_or_else(|| anyhow::anyhow!("sample origin is unavailable"))
    }

    pub(crate) fn enrolled_burn_check_candidate(
        &self,
        candidate: &BurnCheckCandidate,
        check_id: &str,
    ) -> anyhow::Result<BurnCheckCandidate> {
        let origin = self.observe_burn_check_sample_origin(candidate, check_id)?;
        let mut enrolled = candidate.clone();
        if !candidate.historical {
            enrolled.boundary_positions = origin.boundary_positions;
            enrolled.boundary_at_epoch = origin.boundary_at_epoch;
        }
        Ok(enrolled)
    }

    pub fn burn_check_sampled_pairs(
        &self,
        key: &SessionKey,
        check_id: &str,
    ) -> anyhow::Result<Vec<BurnCheckSampledPair>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT comparison_id, dependency_digest, incarnation, action_id, action_digest,
                    instruction_digest, selector_revision, round, assessed
               FROM burn_check_sampled_pair
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = ?4",
        )?;
        let rows = statement.query_map(
            rusqlite::params![key.environment_key, key.agent, key.session_id, check_id],
            |row| {
                Ok(BurnCheckSampledPair {
                    comparison_id: row.get(0)?,
                    dependency_digest: row.get(1)?,
                    incarnation: row.get(2)?,
                    action_id: row.get(3)?,
                    action_digest: row.get(4)?,
                    instruction_digest: row.get(5)?,
                    selector_revision: row.get(6)?,
                    round: row.get(7)?,
                    assessed: row.get(8)?,
                })
            },
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Record a page only while its published result belongs to this revision.
    pub fn save_burn_check_sampled_pairs(
        &self,
        input: &BurnCheckInput,
        pairs: &[BurnCheckSampledPair],
    ) -> anyhow::Result<bool> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let current: bool = transaction.query_row(
            "SELECT EXISTS (SELECT 1 FROM burn_check_assessment
               WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = ?4
                 AND input_revision = ?5 AND incarnation = ?6 AND source_generation = ?7
                 AND source_fingerprint IS ?8 AND published_fence = ?9
                 AND status IN ('completed', 'failed') AND result_revision = ?5)",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id,
                input.input_revision,
                input.incarnation,
                input.source_generation,
                input.source_fingerprint,
                input.published_fence
            ],
            |row| row.get(0),
        )?;
        if !current {
            return Ok(false);
        }
        for pair in pairs {
            transaction.execute(
                "INSERT INTO burn_check_sampled_pair
                    (environment_key, agent, session_id, check_id, comparison_id,
                     dependency_digest, incarnation, action_id, action_digest, instruction_digest,
                     selector_revision, round, assessed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(environment_key, agent, session_id, check_id, comparison_id, dependency_digest)
                 DO UPDATE SET selector_revision = excluded.selector_revision,
                     round = excluded.round, assessed = max(assessed, excluded.assessed)",
                rusqlite::params![input.key.environment_key, input.key.agent, input.key.session_id,
                    input.check_id, pair.comparison_id, pair.dependency_digest, pair.incarnation,
                    pair.action_id, pair.action_digest, pair.instruction_digest,
                    pair.selector_revision, pair.round, pair.assessed],
            )?;
        }
        transaction.commit()?;
        Ok(true)
    }

    /// Fence current-file rules to turns added after this observed instruction version.
    /// The first observation cannot establish when a file became active.
    pub fn observe_burn_check_instruction_epoch(
        &self,
        key: &SessionKey,
        incarnation: u64,
        generation: i64,
        digest: &str,
        now_ms: i64,
    ) -> anyhow::Result<(std::collections::BTreeMap<String, u64>, i64)> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let current: Option<(String, u64, String, i64)> = transaction
            .query_row(
                "SELECT instruction_digest, incarnation, positions_json, observed_at_ms
                   FROM burn_check_instruction_epoch
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                rusqlite::params![key.environment_key, key.agent, key.session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let observed_positions: String = transaction.query_row(
            "SELECT COALESCE(json_group_object(source_key, last_index), '{}')
               FROM (SELECT source_key, MAX(turn_index) AS last_index FROM turn
                      WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                      GROUP BY source_key)",
            rusqlite::params![key.environment_key, key.agent, key.session_id],
            |row| row.get(0),
        )?;
        let mut positions: std::collections::BTreeMap<String, u64> =
            serde_json::from_str(&observed_positions)?;
        let observed_at_ms = if let Some((old_digest, old_incarnation, previous, at)) = &current
            && old_digest == digest
            && *old_incarnation == incarnation
        {
            let previous: std::collections::BTreeMap<String, u64> = serde_json::from_str(previous)?;
            positions.retain(|source, _| !previous.contains_key(source));
            positions.extend(previous);
            *at
        } else {
            now_ms
        };
        if let Some((old_digest, old_incarnation, previous, at)) = current
            && old_digest == digest
            && old_incarnation == incarnation
            && at == observed_at_ms
            && serde_json::from_str::<std::collections::BTreeMap<String, u64>>(&previous)?
                == positions
        {
            return Ok((positions, observed_at_ms));
        }
        let positions_json = serde_json::to_string(&positions)?;
        transaction.execute(
            "INSERT INTO burn_check_instruction_epoch
                 (environment_key, agent, session_id, instruction_digest, incarnation,
                   source_generation, positions_json, observed_at_ms)
              VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
                 instruction_digest = excluded.instruction_digest,
                 incarnation = excluded.incarnation,
                 source_generation = excluded.source_generation,
                  positions_json = excluded.positions_json,
                  observed_at_ms = excluded.observed_at_ms",
            rusqlite::params![
                key.environment_key,
                key.agent,
                key.session_id,
                digest,
                incarnation,
                generation,
                positions_json,
                observed_at_ms
            ],
        )?;
        transaction.commit()?;
        Ok((positions, observed_at_ms))
    }

    #[cfg(test)]
    pub fn save_burn_check_work_answers(
        &self,
        input: &BurnCheckInput,
        progress: &antiburn_local::analysis::jev::JevRunProgress,
        now_epoch: i64,
    ) -> anyhow::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        insert_work_answers(&transaction, input, progress, now_epoch)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn burn_check_work_answers(
        &self,
        input: &BurnCheckInput,
    ) -> anyhow::Result<
        Vec<(
            String,
            String,
            antiburn_local::analysis::jev::JevWorkItemResult,
        )>,
    > {
        let connection = self.lock();
        let mut query = connection.prepare(
            "SELECT reuse_scope, item_marker, result_json FROM burn_check_work_answer
             WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = ?4",
        )?;
        let rows = query.query_map(
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        rows.map(|row| {
            let (scope, marker, result) = row?;
            Ok((scope, marker, serde_json::from_str(&result)?))
        })
        .collect()
    }
    /// Freeze the selected historical window at click time.
    pub fn enqueue_burn_checks(&self, now: i64, days: u8) -> anyhow::Result<usize> {
        self.enqueue_burn_checks_for_revisions(
            &[(
                "ignored_instructions",
                antiburn_local::analysis::ignored_instructions::evaluator_revision(),
            )],
            now,
            days,
        )
    }

    /// Count jobs by check and session, not by distinct session.
    pub fn enqueue_burn_checks_for_revisions(
        &self,
        checks: &[(&str, String)],
        now: i64,
        days: u8,
    ) -> anyhow::Result<usize> {
        if !matches!(days, 7 | 30) {
            anyhow::bail!("Burn Check history window must be 7 or 30 days");
        }
        if checks
            .iter()
            .any(|(id, revision)| id.is_empty() || id.len() > 256 || revision.is_empty())
        {
            anyhow::bail!("Burn Check history identity is invalid");
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        if internal_value_in(&transaction, ENABLED_AT_KEY)?.is_none() {
            anyhow::bail!("TypeSafe checks are not enabled");
        }
        let ids = serde_json::to_string(&checks.iter().map(|(id, _)| id).collect::<Vec<_>>())?;
        let active: bool = transaction.query_row(
            "SELECT EXISTS (
                SELECT 1 FROM burn_check_assessment
                  WHERE check_id IN (SELECT value FROM json_each(?1))
                   AND boundary_generation = -2
                    AND (status IN ('queued', 'running')
                         OR (status = 'failed' AND last_error_category = 'continuing')))",
            [&ids],
            |row| row.get(0),
        )?;
        if active {
            transaction.commit()?;
            return Ok(0);
        }
        let batch_epoch = internal_value_in(&transaction, HISTORY_BATCH_KEY)?
            .map(|value| value.parse::<i64>())
            .transpose()?
            .map_or(now, |previous| now.max(previous.saturating_add(1)));
        transaction.execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![HISTORY_BATCH_KEY, batch_epoch.to_string()],
        )?;
        let mut total = 0;
        for (check_id, evaluator_revision) in checks {
            transaction.execute(
                "UPDATE burn_check_assessment
                SET history_batch_epoch = ?4
              WHERE check_id = ?3
                AND boundary_generation = -2
                AND EXISTS (
                    SELECT 1 FROM session s
                     WHERE s.environment_key = burn_check_assessment.environment_key
                       AND s.agent = burn_check_assessment.agent
                       AND s.session_id = burn_check_assessment.session_id
                       AND s.updated_at_epoch >= ?2 AND s.updated_at_epoch <= ?1
                       AND EXISTS (
                           SELECT 1 FROM turn t
                           JOIN turn_content c ON c.turn_rowid = t.rowid
                            WHERE t.environment_key = s.environment_key
                              AND t.agent = s.agent AND t.session_id = s.session_id
                              AND c.kind <> 'thinking' AND length(c.content) > 0))",
                rusqlite::params![
                    now,
                    now.saturating_sub(i64::from(days) * 24 * 60 * 60),
                    check_id,
                    batch_epoch
                ],
            )?;
            loop {
                let count = transaction.execute(
                "INSERT INTO burn_check_assessment (
                 environment_key, agent, session_id, check_id, incarnation,
                  boundary_generation, boundary_activity_cursor, boundary_at_epoch,
                  boundary_positions_json, status, created_at_epoch, updated_at_epoch, history_batch_epoch)
             SELECT s.environment_key, s.agent, s.session_id, ?4,
                     s.incarnation, -2, '', ?1,
                       json_object('*', 0), 'idle', ?2, ?2, ?5
               FROM session s
               LEFT JOIN burn_check_assessment a
                 ON a.environment_key = s.environment_key AND a.agent = s.agent
                 AND a.session_id = s.session_id AND a.check_id = ?4
               WHERE s.updated_at_epoch >= ?1 AND s.updated_at_epoch <= ?2
                  AND EXISTS (
                      SELECT 1 FROM turn AS t
                      JOIN turn_content AS c ON c.turn_rowid = t.rowid
                      WHERE t.environment_key = s.environment_key
                        AND t.agent = s.agent AND t.session_id = s.session_id
                        AND c.kind <> 'thinking' AND length(c.content) > 0
                  )
                   AND (a.status IS NULL
                       OR (a.boundary_generation IS NOT -2
                            AND a.status NOT IN ('queued', 'running'))
                       OR (a.boundary_generation = -2 AND (
                            a.status = 'superseded'
                              OR (a.status = 'failed' AND
                                  (a.next_attempt_at_epoch IS NULL OR a.next_attempt_at_epoch <= ?2))
                              OR (a.status = 'completed' AND (
                                  a.incarnation <> s.incarnation
                                 OR a.source_generation IS NOT s.source_generation
                                 OR a.source_fingerprint IS NOT s.source_fingerprint
                                 OR a.boundary_activity_cursor <> s.activity_cursor
                                 OR a.evaluator_revision IS NOT ?3)))))
                   AND true
              ORDER BY s.updated_at_epoch, s.environment_key, s.agent, s.session_id
              LIMIT 256
              ON CONFLICT(environment_key, agent, session_id, check_id) DO UPDATE SET
                 incarnation = excluded.incarnation, boundary_generation = -2,
                  boundary_activity_cursor = CASE WHEN burn_check_assessment.boundary_generation = -2
                      THEN burn_check_assessment.boundary_activity_cursor ELSE '' END,
                    boundary_at_epoch = CASE WHEN burn_check_assessment.boundary_generation = -2
                        THEN burn_check_assessment.boundary_at_epoch ELSE excluded.boundary_at_epoch END,
                    boundary_positions_json = CASE WHEN burn_check_assessment.boundary_generation = -2
                        THEN burn_check_assessment.boundary_positions_json ELSE json_object('*', 0) END,
                   status = excluded.status,
                   input_revision = CASE WHEN burn_check_assessment.status = 'failed'
                       THEN burn_check_assessment.input_revision ELSE NULL END,
                   progress_json = CASE WHEN burn_check_assessment.status = 'failed'
                       THEN burn_check_assessment.progress_json ELSE '{}' END,
                   updated_at_epoch = excluded.updated_at_epoch,
                  history_batch_epoch = excluded.history_batch_epoch",
                  rusqlite::params![
                      now.saturating_sub(i64::from(days) * 24 * 60 * 60),
                      now,
                       evaluator_revision,
                       check_id,
                       batch_epoch,
                  ],
            )?;
                total += count;
                if count < 256 {
                    break;
                }
            }
        }
        transaction.commit()?;
        Ok(total)
    }

    pub fn historical_burn_check_status(
        &self,
        now_epoch: i64,
        idle_secs: i64,
    ) -> anyhow::Result<BurnCheckHistoryStatus> {
        self.historical_burn_check_status_for_checks(
            &[(
                "ignored_instructions",
                idle_secs,
                antiburn_local::analysis::ignored_instructions::evaluator_revision(),
            )],
            now_epoch,
        )
    }

    pub fn historical_burn_check_status_for_checks(
        &self,
        checks: &[(&str, i64, String)],
        now_epoch: i64,
    ) -> anyhow::Result<BurnCheckHistoryStatus> {
        let connection = self.lock();
        let checks = serde_json::to_string(checks)?;
        let evidence_current = super::revision_sql::current_evidence("evidence", "session");
        let e_current = super::revision_sql::current_evidence("e", "s");
        connection
            .query_row(
                &format!("WITH current_assessments AS (
                    SELECT assessment.*, CASE WHEN assessment.status = 'completed' AND NOT COALESCE((
                        assessment.evaluator_revision = json_extract(registered.value, '$[2]')
                        AND assessment.incarnation = session.incarnation
                        AND assessment.source_generation = session.source_generation
                        AND assessment.source_fingerprint IS session.source_fingerprint
                        AND assessment.published_fence = evidence.published_fence
                        AND assessment.result_revision = assessment.input_revision
                        AND evidence.status = 'ready'
                        AND evidence.processed_fingerprint IS session.source_fingerprint
                        AND {evidence_current}), 0)
                        THEN 'idle' ELSE assessment.status END AS progress_status
                    FROM burn_check_assessment assessment
                     JOIN json_each(:checks) registered ON assessment.check_id = json_extract(registered.value, '$[0]')
                    JOIN session USING (environment_key, agent, session_id)
                    LEFT JOIN session_evidence evidence USING (environment_key, agent, session_id)
                ) SELECT
                    count(*) FILTER (WHERE a.progress_status = 'idle' AND NOT COALESCE((
                        e.status = 'ready'
                        AND e.processed_fingerprint IS s.source_fingerprint
                        AND {e_current}
                        AND e.published_fence IS NOT NULL), 0)
                        AND COALESCE(e.status, '') NOT IN ('failed', 'unsupported')),
                    count(*) FILTER (WHERE a.progress_status = 'idle'
                        AND e.status = 'ready'
                        AND e.processed_fingerprint IS s.source_fingerprint
                        AND {e_current}
                        AND e.published_fence IS NOT NULL
                         AND s.updated_at_epoch > :now_epoch - max(0, json_extract(registered.value, '$[1]'))),
                    count(*) FILTER (WHERE a.progress_status = 'idle'
                        AND e.status = 'ready'
                        AND e.processed_fingerprint IS s.source_fingerprint
                        AND {e_current}
                        AND e.published_fence IS NOT NULL
                         AND s.updated_at_epoch <= :now_epoch - max(0, json_extract(registered.value, '$[1]'))),
                    count(*) FILTER (WHERE a.progress_status = 'queued'
                        AND COALESCE(e.status, '') NOT IN ('failed', 'unsupported')),
                    count(*) FILTER (WHERE (a.progress_status = 'running'
                        OR (a.progress_status = 'failed' AND a.last_error_category = 'continuing'))
                        AND COALESCE(e.status, '') NOT IN ('failed', 'unsupported')),
                    count(*) FILTER (WHERE a.progress_status = 'completed'),
                    count(*) FILTER (WHERE a.progress_status = 'superseded'
                        OR (a.progress_status <> 'completed' AND e.status = 'unsupported')),
                    count(*) FILTER (WHERE a.progress_status NOT IN ('completed', 'superseded')
                        AND COALESCE(e.status, '') <> 'unsupported'
                         AND ((a.progress_status = 'failed' AND a.last_error_category IS NOT 'continuing')
                             OR e.status = 'failed')),
                    count(*)
                FROM current_assessments a
                JOIN json_each(:checks) AS registered ON a.check_id = json_extract(registered.value, '$[0]')
                JOIN session s USING (environment_key, agent, session_id)
                LEFT JOIN session_evidence e USING (environment_key, agent, session_id)
               WHERE a.boundary_generation = -2
                 AND a.history_batch_epoch = CAST((
                       SELECT value FROM setting WHERE key = :history_batch_key) AS INTEGER)
                  AND EXISTS (
                     SELECT 1 FROM turn_content AS content
                     JOIN turn AS content_turn ON content_turn.rowid = content.turn_rowid
                     WHERE content_turn.environment_key = s.environment_key
                       AND content_turn.agent = s.agent
                       AND content_turn.session_id = s.session_id
                       AND content.kind <> 'thinking' AND length(content.content) > 0
                  )"),
                rusqlite::named_params![
                    ":now_epoch": now_epoch,
                    ":checks": checks,
                    ":parser_revision": antiburn_local::analysis::PARSER_REVISION,
                    ":analyzer_revision": antiburn_local::analysis::ANALYZER_REVISION,
                    ":evidence_schema_revision": antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                    ":history_batch_key": HISTORY_BATCH_KEY,
                ],
                |row| {
                    Ok(BurnCheckHistoryStatus {
                        waiting_for_data: row.get(0)?,
                        waiting_for_idle: row.get(1)?,
                        ready: row.get(2)?,
                        queued: row.get(3)?,
                        running: row.get(4)?,
                        completed: row.get(5)?,
                        skipped: row.get(6)?,
                        failed: row.get(7)?,
                        total: row.get(8)?,
                    })
                },
            )
            .map_err(Into::into)
    }

    pub fn burn_check_in_progress_count(&self, check_id: &str) -> anyhow::Result<usize> {
        let connection = self.lock();
        connection
            .query_row(
                "SELECT count(*) FROM burn_check_assessment
                  WHERE check_id = ?1
                    AND (status IN ('queued', 'running')
                         OR (status = 'failed' AND last_error_category = 'continuing'))",
                [check_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

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
             ON CONFLICT(key) DO NOTHING",
            rusqlite::params![ENABLED_AT_KEY, now_epoch.to_string()],
        )?;
        let mut total = 0;
        for check_id in check_ids {
            let enabled_key = format!("{CHECK_ENABLED_AT_PREFIX}{check_id}");
            if internal_value_in(&transaction, &enabled_key)?.is_some() {
                continue;
            }
            let enabled_at = if *check_id == "ignored_instructions" {
                internal_value_in(&transaction, ENABLED_AT_KEY)?
                    .ok_or_else(|| anyhow::anyhow!("Burn Check enable epoch is unavailable"))?
            } else {
                now_epoch.to_string()
            };
            transaction.execute(
                "INSERT INTO setting (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO NOTHING",
                rusqlite::params![enabled_key, enabled_at],
            )?;
            loop {
                let count = transaction.execute(
                    "INSERT INTO burn_check_assessment (
                         environment_key, agent, session_id, check_id, incarnation,
                           boundary_generation, boundary_activity_cursor, boundary_at_epoch,
                           boundary_positions_json,
                           status, created_at_epoch, updated_at_epoch)
                      SELECT s.environment_key, s.agent, s.session_id, ?1, s.incarnation,
                          s.source_generation, s.activity_cursor, ?2,
                          (SELECT COALESCE(json_group_object(source_key, last_index), '{}')
                              FROM (SELECT source_key, MAX(turn_index) AS last_index FROM turn
                                     WHERE environment_key = s.environment_key
                                       AND agent = s.agent AND session_id = s.session_id
                                     GROUP BY source_key)), 'idle', ?2, ?2
                        FROM session s
                       WHERE NOT EXISTS (SELECT 1 FROM burn_check_assessment a
                           WHERE a.environment_key = s.environment_key AND a.agent = s.agent
                             AND a.session_id = s.session_id AND a.check_id = ?1)
                       ORDER BY s.environment_key, s.agent, s.session_id
                       LIMIT 256
                     ON CONFLICT(environment_key, agent, session_id, check_id) DO NOTHING",
                    rusqlite::params![check_id, now_epoch],
                )?;
                total += count;
                if count < 256 {
                    break;
                }
            }
        }
        transaction.commit()?;
        Ok(total)
    }

    /// Stop new work while preserving completed assessment results.
    pub fn disable_burn_checks(&self) -> anyhow::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM setting WHERE key = ?1", [ENABLED_AT_KEY])?;
        transaction.execute(
            "UPDATE burn_check_assessment
                SET status = 'superseded', last_error_category = 'cancelled',
                    lease_expires_at_epoch = NULL, next_attempt_at_epoch = NULL
              WHERE status IN ('queued', 'running')",
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Make rejected-key assessments eligible after a replacement credential is saved.
    pub fn retry_rejected_burn_checks(&self) -> anyhow::Result<()> {
        self.lock().execute(
            "UPDATE burn_check_assessment SET next_attempt_at_epoch = NULL
              WHERE status = 'failed' AND last_error_category = 'authentication_rejected'",
            [],
        )?;
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
        self.burn_check_candidates_for_revision(
            check_id,
            &antiburn_local::analysis::ignored_instructions::evaluator_revision(),
            now_epoch,
            idle_secs,
            limit,
        )
    }

    pub fn burn_check_candidates_for_revision(
        &self,
        check_id: &str,
        evaluator_revision: &str,
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
        let enabled_at =
            internal_value_in(&connection, &format!("{CHECK_ENABLED_AT_PREFIX}{check_id}"))?
                .map(|value| value.parse::<i64>())
                .transpose()?
                .unwrap_or(enabled_at);
        let current_evidence = super::revision_sql::current_evidence("evidence", "s");
        let sql = format!("SELECT s.environment_key, s.agent, s.session_id, s.source_kind,
                    s.source_label, s.wsl_distro, s.title, s.title_source, s.cwd,
                    s.surface, s.updated_at_epoch, s.activity_cursor, s.activity_source,
                    s.subagent_count,
                    (SELECT related_id FROM session_relation r
                      WHERE r.environment_key = s.environment_key AND r.agent = s.agent
                        AND r.session_id = s.session_id AND r.kind = 'forkParent' LIMIT 1),
                    s.source_fingerprint,
                    s.incarnation, s.source_generation, s.source_fingerprint,
                     s.activity_cursor, evidence.published_fence,
                      CASE WHEN assessment.boundary_generation = -2 AND assessment.status = 'completed'
                                 AND assessment.evaluator_revision IS :evaluator_revision
                           THEN assessment.updated_at_epoch ELSE assessment.boundary_at_epoch END,
                      assessment.boundary_generation, assessment.boundary_positions_json
               FROM session s
             LEFT JOIN burn_check_assessment AS assessment
               ON assessment.environment_key = s.environment_key
              AND assessment.agent = s.agent
              AND assessment.session_id = s.session_id
                AND assessment.check_id = :check_id
             JOIN session_evidence AS evidence
               ON evidence.environment_key = s.environment_key
              AND evidence.agent = s.agent
              AND evidence.session_id = s.session_id
              AND evidence.status = 'ready'
               AND evidence.processed_fingerprint IS s.source_fingerprint
               AND {current_evidence}
               AND evidence.published_fence IS NOT NULL
               WHERE s.updated_at_epoch IS NOT NULL
                  AND s.updated_at_epoch <= :now_epoch - :idle_secs
                AND EXISTS (
                    SELECT 1 FROM turn_content AS content
                    JOIN turn AS content_turn ON content_turn.rowid = content.turn_rowid
                    WHERE content_turn.environment_key = s.environment_key
                      AND content_turn.agent = s.agent
                      AND content_turn.session_id = s.session_id
                      AND content.kind <> 'thinking' AND length(content.content) > 0
                )
                AND (
                       (assessment.boundary_generation IS NOT NULL
                         AND assessment.boundary_generation <> -2
                         AND (assessment.boundary_generation = -1
                            OR s.activity_cursor <> assessment.boundary_activity_cursor)
                       AND s.updated_at_epoch >= assessment.boundary_at_epoch)
                       OR (assessment.status = 'failed'
                           AND (assessment.last_error_category = 'continuing'
                                  OR assessment.next_attempt_at_epoch <= :now_epoch))
                     OR assessment.status = 'queued'
                     OR (assessment.status = 'running'
                           AND assessment.lease_expires_at_epoch <= :now_epoch)
                     OR (assessment.status = 'superseded'
                         AND assessment.last_error_category = 'cancelled')
                     OR (assessment.status = 'superseded'
                         AND assessment.last_error_category = 'unsupported_format'
                         AND (assessment.source_generation IS NOT s.source_generation
                              OR assessment.source_fingerprint IS NOT s.source_fingerprint
                              OR assessment.published_fence IS NOT evidence.published_fence))
                    OR
                     (assessment.boundary_generation IS NULL
                        AND s.updated_at_epoch >= :enabled_at)
                      OR (assessment.boundary_generation = -2
                          AND assessment.status IN ('idle', 'queued', 'running'))
                       OR (assessment.boundary_generation = -2
                           AND assessment.status = 'failed'
                           AND (assessment.next_attempt_at_epoch IS NULL
                                  OR assessment.next_attempt_at_epoch <= :now_epoch))
                       OR (assessment.boundary_generation = -2
                           AND assessment.status IN ('completed', 'superseded')
                           AND s.activity_cursor <> assessment.boundary_activity_cursor
                           AND s.updated_at_epoch > assessment.updated_at_epoch)
                       OR (assessment.status = 'completed'
                           AND assessment.boundary_generation <> -2
                           AND (assessment.incarnation <> s.incarnation
                                OR assessment.source_generation IS NOT s.source_generation
                                OR assessment.source_fingerprint IS NOT s.source_fingerprint
                                OR assessment.published_fence IS NOT evidence.published_fence))
                        OR (assessment.status = 'completed'
                              AND assessment.evaluator_revision IS NOT :evaluator_revision)
                       OR (assessment.status = 'failed'
                           AND assessment.boundary_generation <> -2
                             AND assessment.evaluator_revision IS NOT :evaluator_revision)
               )
                AND (assessment.status IS NULL
                     OR assessment.status <> 'running'
                       OR assessment.lease_expires_at_epoch <= :now_epoch)
                 AND (assessment.next_attempt_at_epoch IS NULL
                        OR assessment.next_attempt_at_epoch <= :now_epoch
                       OR (assessment.boundary_generation <> -2
                            AND (assessment.evaluator_revision IS NOT :evaluator_revision
                                OR assessment.last_error_category IN
                                   ('usage_limit', 'request_limit', 'assessment_page_limit'))))
              ORDER BY row_number() OVER (
                  PARTITION BY COALESCE(assessment.boundary_generation = -2, 0)
                   ORDER BY s.updated_at_epoch DESC, COALESCE(assessment.updated_at_epoch, 0),
                           s.environment_key, s.agent, s.session_id),
                  COALESCE(assessment.boundary_generation = -2, 0),
                  s.environment_key, s.agent, s.session_id
               LIMIT :limit");
        let mut statement = connection.prepare(&sql)?;
        let candidates = statement
            .query_map(
                rusqlite::named_params![
                    ":check_id": check_id,
                    ":parser_revision": antiburn_local::analysis::PARSER_REVISION,
                    ":analyzer_revision": antiburn_local::analysis::ANALYZER_REVISION,
                    ":evidence_schema_revision": antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                    ":now_epoch": now_epoch,
                    ":idle_secs": idle_secs.max(0),
                    ":enabled_at": enabled_at,
                    ":limit": i64::try_from(limit.min(256)).unwrap_or(256),
                    ":evaluator_revision": evaluator_revision,
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
                        boundary_positions: if row.get::<_, Option<i64>>(22)?.is_none() {
                            std::collections::BTreeMap::from([("*".to_owned(), 0)])
                        } else {
                            row.get::<_, Option<String>>(23)?
                                .and_then(|json| serde_json::from_str(&json).ok())
                                .unwrap_or_default()
                        },
                        historical: row.get::<_, Option<i64>>(22)? == Some(-2),
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(candidates)
    }

    pub(crate) fn skill_observation_candidates(
        &self,
        after: Option<(&str, &str)>,
        limit: usize,
    ) -> anyhow::Result<Vec<BurnCheckCandidate>> {
        let connection = self.lock();
        let mut statement = connection.prepare("SELECT s.environment_key, s.agent, s.session_id, s.source_kind,
            s.source_label, s.wsl_distro, s.title, s.title_source, s.cwd, s.surface,
            s.updated_at_epoch, s.activity_cursor, s.activity_source, s.subagent_count,
            (SELECT related_id FROM session_relation r WHERE r.environment_key = s.environment_key
                AND r.agent = s.agent AND r.session_id = s.session_id AND r.kind = 'forkParent' LIMIT 1),
            s.source_fingerprint, s.incarnation, s.source_generation, e.published_fence,
            a.boundary_at_epoch, a.boundary_positions_json, a.boundary_generation
            FROM session s JOIN session_evidence e ON e.environment_key = s.environment_key
                AND e.agent = s.agent AND e.session_id = s.session_id
            JOIN burn_check_assessment a ON a.environment_key = s.environment_key
                AND a.agent = s.agent AND a.session_id = s.session_id AND a.check_id = 'skill_opportunities'
            WHERE s.environment_key = 'native' AND s.agent IN ('opencode', 'codex', 'claude', 'claude-code', 'pi') AND e.status = 'ready'
                AND e.analyzed_generation = s.source_generation AND e.processed_fingerprint IS s.source_fingerprint
                AND (?1 IS NULL OR (s.agent, s.session_id) > (?1, ?2))
            ORDER BY s.agent, s.session_id LIMIT ?3")?;
        let rows = statement
            .query_map(
                rusqlite::params![
                    after.map(|(agent, _)| agent),
                    after.map(|(_, session)| session),
                    limit.min(256)
                ],
                |row| {
                    let session = session_from_row(row)?;
                    Ok(BurnCheckCandidate {
                        incarnation: row.get(16)?,
                        source_generation: row.get(17)?,
                        source_fingerprint: session.source_fingerprint.clone(),
                        activity_cursor: session.activity_cursor.clone(),
                        published_fence: row.get(18)?,
                        boundary_at_epoch: row.get::<_, Option<i64>>(19)?.unwrap_or(0),
                        boundary_positions: row
                            .get::<_, Option<String>>(20)?
                            .and_then(|json| serde_json::from_str(&json).ok())
                            .unwrap_or_default(),
                        historical: row.get::<_, Option<i64>>(21)? == Some(-2),
                        session,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn record_burn_check_candidate_issue_for_check(
        &self,
        check_id: &str,
        candidate: &BurnCheckCandidate,
        unsupported: bool,
        retry_at_epoch: i64,
        now_epoch: i64,
    ) -> anyhow::Result<()> {
        let status = if unsupported { "superseded" } else { "failed" };
        let category = if unsupported {
            "unsupported_format"
        } else {
            "evidence_unavailable"
        };
        let positions = serde_json::to_string(&candidate.boundary_positions)?;
        let boundary_generation = if candidate.historical {
            -2
        } else {
            candidate.source_generation
        };
        let connection = self.lock();
        connection.execute(
            "INSERT INTO burn_check_assessment (
                 environment_key, agent, session_id, check_id, incarnation,
                 boundary_generation, boundary_activity_cursor, boundary_at_epoch,
                 boundary_positions_json, source_generation, source_fingerprint,
                 published_fence, status, progress_json, created_at_epoch,
                 updated_at_epoch, next_attempt_at_epoch, last_error_category)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, '{}', ?14, ?14, ?15, ?16)
             ON CONFLICT(environment_key, agent, session_id, check_id) DO UPDATE SET
                 incarnation = excluded.incarnation,
                 boundary_generation = CASE
                     WHEN burn_check_assessment.boundary_generation = -2 THEN -2
                     ELSE excluded.boundary_generation END,
                 boundary_activity_cursor = CASE
                     WHEN burn_check_assessment.boundary_generation = -2 THEN ''
                     ELSE excluded.boundary_activity_cursor END,
                 boundary_at_epoch = COALESCE(
                     burn_check_assessment.boundary_at_epoch, excluded.boundary_at_epoch),
                 boundary_positions_json = CASE
                     WHEN burn_check_assessment.boundary_generation = -2
                     THEN burn_check_assessment.boundary_positions_json
                     ELSE excluded.boundary_positions_json END,
                 input_revision = CASE
                     WHEN excluded.last_error_category = 'unsupported_format' THEN NULL
                     WHEN burn_check_assessment.source_generation IS excluded.source_generation
                          AND burn_check_assessment.source_fingerprint IS excluded.source_fingerprint
                          AND burn_check_assessment.published_fence IS excluded.published_fence
                     THEN burn_check_assessment.input_revision ELSE NULL END,
                 source_generation = excluded.source_generation,
                 source_fingerprint = excluded.source_fingerprint,
                 published_fence = excluded.published_fence,
                 status = excluded.status,
                 progress_json = CASE
                     WHEN excluded.last_error_category = 'unsupported_format' THEN '{}'
                     WHEN burn_check_assessment.source_generation IS excluded.source_generation
                          AND burn_check_assessment.source_fingerprint IS excluded.source_fingerprint
                          AND burn_check_assessment.published_fence IS excluded.published_fence
                     THEN burn_check_assessment.progress_json ELSE '{}' END,
                 updated_at_epoch = excluded.updated_at_epoch,
                 next_attempt_at_epoch = excluded.next_attempt_at_epoch,
                 lease_expires_at_epoch = NULL,
                 last_error_category = excluded.last_error_category",
            rusqlite::params![
                candidate.session.key.environment_key,
                candidate.session.key.agent,
                candidate.session.key.session_id,
                check_id,
                candidate.incarnation,
                boundary_generation,
                candidate.activity_cursor,
                candidate.boundary_at_epoch,
                positions,
                candidate.source_generation,
                candidate.source_fingerprint,
                candidate.published_fence,
                status,
                now_epoch,
                (!unsupported).then_some(retry_at_epoch),
                category,
            ],
        )?;
        Ok(())
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
        let current_evidence = super::revision_sql::current_evidence("e", "s");
        let current_sql = format!(
                "SELECT s.incarnation, s.source_generation, s.source_fingerprint,
                        s.activity_cursor, e.published_fence, s.updated_at_epoch
                   FROM session AS s
                   JOIN session_evidence AS e
                     ON e.environment_key = s.environment_key AND e.agent = s.agent
                    AND e.session_id = s.session_id
                    AND e.status = 'ready'
                     AND e.processed_fingerprint IS s.source_fingerprint
                     AND {current_evidence}
                   WHERE s.environment_key = :environment_key AND s.agent = :agent AND s.session_id = :session_id"
        );
        let current: Option<CurrentAssessmentSource> = transaction
            .query_row(
                &current_sql,
                rusqlite::named_params![
                    ":environment_key": input.key.environment_key,
                    ":agent": input.key.agent,
                    ":session_id": input.key.session_id,
                    ":parser_revision": antiburn_local::analysis::PARSER_REVISION,
                    ":analyzer_revision": antiburn_local::analysis::ANALYZER_REVISION,
                    ":evidence_schema_revision": antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
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
        let historical: bool = transaction.query_row(
            "SELECT EXISTS (SELECT 1 FROM burn_check_assessment
                WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                  AND check_id = ?4 AND boundary_generation = -2)",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id
            ],
            |row| row.get(0),
        )?;
        if incarnation != input.incarnation
            || generation != input.source_generation
            || fingerprint != input.source_fingerprint
            || cursor != input.activity_cursor
            || fence != Some(input.published_fence)
            || (!historical && updated_at < input.boundary_at_epoch)
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
        if historical
            && old.as_ref().is_some_and(|(_, _, retry_at, _)| {
                retry_at.is_some_and(|retry_at| retry_at > now_epoch)
            })
        {
            return Ok(false);
        }
        if let Some((Some(old_revision), status, next_attempt, lease)) = &old
            && old_revision == &input.input_revision
            && (status == "completed"
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
                  boundary_generation = CASE WHEN ?15 THEN -1
                       WHEN burn_check_assessment.boundary_generation = -2
                            AND burn_check_assessment.status = 'completed'
                            AND burn_check_assessment.evaluator_revision IS excluded.evaluator_revision THEN ?6
                      ELSE burn_check_assessment.boundary_generation END,
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
        let current_evidence = super::revision_sql::current_evidence("e", "s");
        let updated = connection.execute(
            &format!("UPDATE burn_check_assessment
                SET status = 'running', lease_expires_at_epoch = :lease_expires_at,
                    updated_at_epoch = :now_epoch
              WHERE environment_key = :environment_key AND agent = :agent AND session_id = :session_id
                AND check_id = :check_id AND input_revision = :input_revision
                AND (status = 'queued' OR
                     (status = 'running' AND lease_expires_at_epoch <= :now_epoch))
                AND (next_attempt_at_epoch IS NULL OR next_attempt_at_epoch <= :now_epoch)
                AND EXISTS (
                    SELECT 1 FROM session AS s
                    JOIN session_evidence AS e
                      ON e.environment_key = s.environment_key AND e.agent = s.agent
                     AND e.session_id = s.session_id
                      AND e.status = 'ready'
                      AND e.processed_fingerprint IS s.source_fingerprint
                        AND e.published_fence = :published_fence
                        AND {current_evidence}
                    WHERE s.environment_key = :environment_key AND s.agent = :agent AND s.session_id = :session_id
                      AND s.incarnation = :incarnation
                      AND s.source_generation = :source_generation
                      AND s.source_fingerprint IS :source_fingerprint
                      AND s.activity_cursor = :activity_cursor
                      AND s.updated_at_epoch <= :now_epoch - :idle_secs
                )"),
            rusqlite::named_params![
                ":environment_key": input.key.environment_key,
                ":agent": input.key.agent,
                ":session_id": input.key.session_id,
                ":check_id": input.check_id,
                ":input_revision": input.input_revision,
                ":published_fence": input.published_fence,
                ":now_epoch": now_epoch,
                ":lease_expires_at": now_epoch.saturating_add(lease_secs.max(1)),
                ":parser_revision": antiburn_local::analysis::PARSER_REVISION,
                ":analyzer_revision": antiburn_local::analysis::ANALYZER_REVISION,
                ":evidence_schema_revision": antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                ":incarnation": input.incarnation,
                ":source_generation": input.source_generation,
                ":source_fingerprint": input.source_fingerprint,
                ":activity_cursor": input.activity_cursor,
                ":idle_secs": idle_secs.max(0),
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
        let current_evidence = super::revision_sql::current_evidence("e", "s");
        let updated = connection.execute(
            &format!("UPDATE burn_check_assessment
                SET lease_expires_at_epoch = :lease_expires_at, updated_at_epoch = :now_epoch
              WHERE environment_key = :environment_key AND agent = :agent AND session_id = :session_id
                AND check_id = :check_id AND input_revision = :input_revision AND status = 'running'
                AND lease_expires_at_epoch > :now_epoch
                AND EXISTS (
                    SELECT 1 FROM session AS s
                    JOIN session_evidence AS e
                      ON e.environment_key = s.environment_key AND e.agent = s.agent
                     AND e.session_id = s.session_id
                      AND e.status = 'ready'
                      AND e.processed_fingerprint IS s.source_fingerprint
                        AND e.published_fence = :published_fence
                        AND {current_evidence}
                    WHERE s.environment_key = :environment_key AND s.agent = :agent AND s.session_id = :session_id
                      AND s.incarnation = :incarnation AND s.source_generation = :source_generation
                      AND s.source_fingerprint IS :source_fingerprint AND s.activity_cursor = :activity_cursor
                      AND s.updated_at_epoch <= :now_epoch - :idle_secs
                )"),
            rusqlite::named_params![
                ":environment_key": input.key.environment_key,
                ":agent": input.key.agent,
                ":session_id": input.key.session_id,
                ":check_id": input.check_id,
                ":input_revision": input.input_revision,
                ":published_fence": input.published_fence,
                ":now_epoch": now_epoch,
                ":lease_expires_at": now_epoch.saturating_add(lease_secs.max(1)),
                ":incarnation": input.incarnation,
                ":source_generation": input.source_generation,
                ":source_fingerprint": input.source_fingerprint,
                ":activity_cursor": input.activity_cursor,
                ":idle_secs": idle_secs.max(0),
                ":parser_revision": antiburn_local::analysis::PARSER_REVISION,
                ":analyzer_revision": antiburn_local::analysis::ANALYZER_REVISION,
                ":evidence_schema_revision": antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
            ],
        )?;
        Ok(updated == 1)
    }

    /// Save typed, source-free progress only for a still-current active revision.
    #[cfg(test)]
    pub fn save_burn_check_progress(
        &self,
        input: &BurnCheckInput,
        progress_json: &str,
        now_epoch: i64,
        lease_secs: i64,
        idle_secs: i64,
    ) -> anyhow::Result<bool> {
        self.save_burn_check_checkpoint(
            input,
            progress_json,
            None,
            now_epoch,
            lease_secs,
            idle_secs,
        )
    }

    /// Commit answers and progress together only for the exact active source and evaluator.
    pub fn save_burn_check_checkpoint(
        &self,
        input: &BurnCheckInput,
        progress_json: &str,
        progress: Option<&antiburn_local::analysis::jev::JevRunProgress>,
        now_epoch: i64,
        lease_secs: i64,
        idle_secs: i64,
    ) -> anyhow::Result<bool> {
        if progress_json.len() > MAX_PROGRESS_BYTES
            || serde_json::from_str::<Value>(progress_json).is_err()
        {
            anyhow::bail!("Burn Check progress is invalid or too large");
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let current_evidence = super::revision_sql::current_evidence("e", "s");
        let updated = transaction.execute(
            &format!("UPDATE burn_check_assessment
                SET progress_json = :progress_json, updated_at_epoch = :now_epoch,
                    lease_expires_at_epoch = :lease_expires_at
              WHERE environment_key = :environment_key AND agent = :agent AND session_id = :session_id
                AND check_id = :check_id AND input_revision = :input_revision AND status = 'running'
                AND evaluator_revision = :expected_evaluator_revision AND incarnation = :incarnation
                AND source_generation = :source_generation AND source_fingerprint IS :source_fingerprint
                AND published_fence = :published_fence
                AND lease_expires_at_epoch > :now_epoch
                AND EXISTS (
                    SELECT 1 FROM session AS s
                    JOIN session_evidence AS e
                      ON e.environment_key = s.environment_key AND e.agent = s.agent
                     AND e.session_id = s.session_id
                      AND e.status = 'ready'
                      AND e.processed_fingerprint IS s.source_fingerprint
                       AND e.published_fence = :published_fence
                       AND {current_evidence}
                    WHERE s.environment_key = :environment_key AND s.agent = :agent AND s.session_id = :session_id
                      AND s.incarnation = :incarnation AND s.source_generation = :source_generation
                      AND s.source_fingerprint IS :source_fingerprint AND s.activity_cursor = :activity_cursor
                      AND s.updated_at_epoch <= :now_epoch - :idle_secs
                )"),
            rusqlite::named_params![
                ":environment_key": input.key.environment_key,
                ":agent": input.key.agent,
                ":session_id": input.key.session_id,
                ":check_id": input.check_id,
                ":input_revision": input.input_revision,
                ":progress_json": progress_json,
                ":now_epoch": now_epoch,
                ":lease_expires_at": now_epoch.saturating_add(lease_secs.max(1)),
                ":published_fence": input.published_fence,
                ":incarnation": input.incarnation,
                ":source_generation": input.source_generation,
                ":source_fingerprint": input.source_fingerprint,
                ":activity_cursor": input.activity_cursor,
                ":idle_secs": idle_secs.max(0),
                ":parser_revision": antiburn_local::analysis::PARSER_REVISION,
                ":analyzer_revision": antiburn_local::analysis::ANALYZER_REVISION,
                ":evidence_schema_revision": antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                ":expected_evaluator_revision": input.evaluator_revision,
            ],
        )?;
        if updated == 1
            && let Some(progress) = progress
        {
            insert_work_answers(&transaction, input, progress, now_epoch)?;
        }
        transaction.commit()?;
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
        if result_json.len() > MAX_RESULT_BYTES
            || serde_json::from_str::<Value>(result_json).is_err()
        {
            anyhow::bail!("Burn Check result is invalid or too large");
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let current_evidence = super::revision_sql::current_evidence("e", "s");
        let updated = transaction.execute(
            &format!("UPDATE burn_check_assessment
                 SET status = 'completed', result_json = :result_json, result_revision = :input_revision,
                     progress_json = '{{}}', boundary_generation = CASE
                         WHEN boundary_generation = -2 THEN -2 ELSE :source_generation END,
                     boundary_activity_cursor = :activity_cursor,
                     boundary_positions_json =
                          (SELECT COALESCE(json_group_object(source_key, last_index), '{{}}')
                            FROM (SELECT source_key, MAX(turn_index) AS last_index FROM turn
                                   WHERE environment_key = :environment_key AND agent = :agent AND session_id = :session_id
                                  GROUP BY source_key)),
                     lease_expires_at_epoch = NULL,
                    next_attempt_at_epoch = NULL, last_error_category = NULL,
                     updated_at_epoch = :now_epoch
              WHERE environment_key = :environment_key AND agent = :agent AND session_id = :session_id
                AND check_id = :check_id AND input_revision = :input_revision AND status = 'running'
                AND lease_expires_at_epoch > :now_epoch
                AND EXISTS (
                    SELECT 1 FROM session AS s
                    JOIN session_evidence AS e
                      ON e.environment_key = s.environment_key AND e.agent = s.agent
                     AND e.session_id = s.session_id
                     AND e.status = 'ready'
                      AND e.processed_fingerprint IS s.source_fingerprint
                        AND e.published_fence = :published_fence
                        AND {current_evidence}
                    WHERE s.environment_key = :environment_key AND s.agent = :agent AND s.session_id = :session_id
                      AND s.incarnation = :incarnation AND s.source_generation = :source_generation
                      AND s.source_fingerprint IS :source_fingerprint AND s.activity_cursor = :activity_cursor
                      AND s.updated_at_epoch <= :now_epoch - :idle_secs
                )"),
            rusqlite::named_params![
                ":environment_key": input.key.environment_key,
                ":agent": input.key.agent,
                ":session_id": input.key.session_id,
                ":check_id": input.check_id,
                ":input_revision": input.input_revision,
                ":result_json": result_json,
                ":now_epoch": now_epoch,
                ":published_fence": input.published_fence,
                ":source_generation": input.source_generation,
                ":activity_cursor": input.activity_cursor,
                ":incarnation": input.incarnation,
                ":source_fingerprint": input.source_fingerprint,
                ":idle_secs": idle_secs.max(0),
                ":parser_revision": antiburn_local::analysis::PARSER_REVISION,
                ":analyzer_revision": antiburn_local::analysis::ANALYZER_REVISION,
                ":evidence_schema_revision": antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
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
            || failure.result_json.len() > MAX_RESULT_BYTES
            || failure.progress_json.len() > MAX_PROGRESS_BYTES
            || serde_json::from_str::<Value>(failure.result_json).is_err()
            || serde_json::from_str::<Value>(failure.progress_json).is_err()
        {
            anyhow::bail!("Burn Check failure state is invalid or too large");
        }
        let connection = self.lock();
        let current_evidence = super::revision_sql::current_evidence("e", "s");
        let updated = connection.execute(
            &format!("UPDATE burn_check_assessment
                SET status = 'failed', next_attempt_at_epoch = :retry_at_epoch,
                    result_json = :result_json, result_revision = :input_revision, progress_json = :progress_json,
                    lease_expires_at_epoch = NULL, last_error_category = :error_category,
                    updated_at_epoch = :now_epoch
              WHERE environment_key = :environment_key AND agent = :agent AND session_id = :session_id
                AND check_id = :check_id AND input_revision = :input_revision AND status = 'running'
                AND lease_expires_at_epoch > :now_epoch
                AND EXISTS (
                    SELECT 1 FROM session AS s
                    JOIN session_evidence AS e
                      ON e.environment_key = s.environment_key AND e.agent = s.agent
                     AND e.session_id = s.session_id
                      AND e.status = 'ready'
                      AND e.processed_fingerprint IS s.source_fingerprint
                       AND e.published_fence = :published_fence
                       AND {current_evidence}
                    WHERE s.environment_key = :environment_key AND s.agent = :agent AND s.session_id = :session_id
                      AND s.incarnation = :incarnation AND s.source_generation = :source_generation
                      AND s.source_fingerprint IS :source_fingerprint AND s.activity_cursor = :activity_cursor
                      AND s.updated_at_epoch <= :now_epoch - :idle_secs
                )"),
            rusqlite::named_params![
                ":environment_key": input.key.environment_key,
                ":agent": input.key.agent,
                ":session_id": input.key.session_id,
                ":check_id": input.check_id,
                ":input_revision": input.input_revision,
                ":retry_at_epoch": failure.retry_at_epoch,
                ":result_json": failure.result_json,
                ":progress_json": failure.progress_json,
                ":error_category": failure.error_category,
                ":now_epoch": now_epoch,
                ":published_fence": input.published_fence,
                ":incarnation": input.incarnation,
                ":source_generation": input.source_generation,
                ":source_fingerprint": input.source_fingerprint,
                ":activity_cursor": input.activity_cursor,
                ":idle_secs": idle_secs.max(0),
                ":parser_revision": antiburn_local::analysis::PARSER_REVISION,
                ":analyzer_revision": antiburn_local::analysis::ANALYZER_REVISION,
                ":evidence_schema_revision": antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
            ],
        )?;
        Ok(updated == 1)
    }

    /// Stop active dispatch without storing a result for resumed or disabled work.
    pub fn invalidate_burn_check_inputs(
        &self,
        key: &SessionKey,
        check_id: &str,
        expected_input_revision: Option<&str>,
        now_epoch: i64,
    ) -> anyhow::Result<bool> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE burn_check_assessment SET status = 'queued', input_revision = NULL,
                result_revision = NULL, result_json = NULL, progress_json = '{}',
                lease_expires_at_epoch = NULL, next_attempt_at_epoch = NULL,
                last_error_category = NULL, updated_at_epoch = ?6
             WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
               AND check_id = ?4 AND input_revision IS ?5",
            rusqlite::params![
                key.environment_key,
                key.agent,
                key.session_id,
                check_id,
                expected_input_revision,
                now_epoch
            ],
        )?;
        if changed != 0 {
            transaction.execute(
                "DELETE FROM burn_check_work_answer WHERE environment_key = ?1 AND agent = ?2
                    AND session_id = ?3 AND check_id = ?4",
                rusqlite::params![key.environment_key, key.agent, key.session_id, check_id],
            )?;
        }
        transaction.commit()?;
        Ok(changed != 0)
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

    pub fn release_failed_burn_check_lease(
        &self,
        input: &BurnCheckInput,
        category: &str,
        retry_at_epoch: i64,
    ) -> anyhow::Result<bool> {
        let updated = self.lock().execute(
            "UPDATE burn_check_assessment
                SET status = 'failed', lease_expires_at_epoch = NULL,
                    next_attempt_at_epoch = ?6, last_error_category = ?7
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND check_id = ?4 AND input_revision = ?5 AND status = 'running'",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id,
                input.input_revision,
                retry_at_epoch,
                category,
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

fn insert_work_answers(
    connection: &rusqlite::Connection,
    input: &BurnCheckInput,
    progress: &antiburn_local::analysis::jev::JevRunProgress,
    now_epoch: i64,
) -> anyhow::Result<()> {
    let Some(scope) = progress
        .completed_batch_ids
        .iter()
        .find(|id| id.starts_with("reuse-scope:"))
    else {
        return Ok(());
    };
    let mut inserted = false;
    for (id, result) in &progress.results {
        let prefix = format!("reuse-item:{id}:");
        let Some(marker) = progress
            .completed_batch_ids
            .iter()
            .find(|marker| marker.starts_with(&prefix))
        else {
            continue;
        };
        inserted |= connection.execute(
            "INSERT OR IGNORE INTO burn_check_work_answer
                (environment_key, agent, session_id, check_id, reuse_scope, item_marker, result_json, updated_at_epoch)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![input.key.environment_key, input.key.agent, input.key.session_id,
                input.check_id, scope, marker, serde_json::to_string(result)?, now_epoch],
        )? > 0;
    }
    if inserted {
        connection.execute(
            "DELETE FROM burn_check_work_answer WHERE rowid IN (
                SELECT rowid FROM burn_check_work_answer
                WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = ?4
                ORDER BY updated_at_epoch DESC, rowid DESC LIMIT -1 OFFSET 8192)",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id
            ],
        )?;
    }
    Ok(())
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
    connection.execute("DELETE FROM burn_check_sampled_pair", [])?;
    connection.execute("DELETE FROM burn_check_sample_origin", [])?;
    connection.execute("DELETE FROM burn_check_instruction_epoch", [])?;
    connection.execute("DELETE FROM burn_check_work_answer", [])?;
    connection.execute("DELETE FROM burn_check_response_cache", [])?;
    connection.execute("DELETE FROM burn_check_request_outcome", [])?;
    connection.execute("DELETE FROM burn_check_usage_reservation", [])?;
    connection.execute(
        "DELETE FROM setting WHERE key IN (?1, ?2, ?3)",
        rusqlite::params![USAGE_LEDGER_KEY, RESPONSE_CACHE_KEY, HISTORY_BATCH_KEY],
    )?;
    if enabled {
        connection.execute(
            "UPDATE setting SET value = ?2 WHERE key = ?1 OR key LIKE ?3",
            rusqlite::params![
                ENABLED_AT_KEY,
                now_epoch.to_string(),
                format!("{CHECK_ENABLED_AT_PREFIX}%")
            ],
        )?;
    } else {
        connection.execute(
            "DELETE FROM setting WHERE key LIKE ?1",
            [format!("{CHECK_ENABLED_AT_PREFIX}%")],
        )?;
    }
    Ok(())
}

pub(super) fn forget_session_usage_in(
    connection: &rusqlite::Connection,
    key: &SessionKey,
) -> anyhow::Result<()> {
    usage::load_ledger(connection)?;
    let session_key = digest_parts([
        key.environment_key.as_str(),
        key.agent.as_str(),
        key.session_id.as_str(),
    ]);
    connection.execute(
        "DELETE FROM burn_check_work_answer WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        rusqlite::params![key.environment_key, key.agent, key.session_id],
    )?;
    connection.execute(
        "UPDATE burn_check_usage_reservation SET session_key = '',
        data = json_set(data, '$.session_key', '') WHERE session_key = ?1",
        [session_key],
    )?;
    Ok(())
}
