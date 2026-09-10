//! Candidate selection and checkpoint bookkeeping for the bounded Codex
//! rollout history reader.
//!
//! [`crate::provider_usage::codex_rollout_history`] is this module's only
//! caller. A candidate is a Codex file session whose account resolves under
//! the same two-step rule [`super::provider_limit`] uses for attribution
//! (`resolve_bound_account`, falling back to `provider_known_accounts`), so
//! an unbound session on a single-account machine is eligible the same way
//! it is for factor learning. The checkpoint itself is keyed by session and
//! provider, not by account: it tracks how far a rollout file has been
//! read, which does not depend on which account a later pass resolves the
//! session to.

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use super::provider_limit::resolve_bound_account;
use super::{SessionKey, Store};

/// Codex's provider id, as recorded on `session_provider_account` and
/// `provider_account_seen`.
const OPENAI: &str = "openai";

/// Raw candidate rows one scan may return before account resolution filters
/// some of them out. Wide enough that a scan still yields a full batch of
/// resolvable candidates even when a few rows in it do not resolve.
const MAX_CANDIDATE_SCAN_ROWS: usize = 64;

/// One Codex rollout file eligible for a bounded read, with the account its
/// session resolves to and how far it has already been read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RolloutCandidate {
    pub key: SessionKey,
    pub source_label: String,
    pub account_key: String,
    pub cursor_bytes: u64,
    pub source_bytes: u64,
    pub source_modified_epoch: Option<i64>,
    pub source_identity: String,
    pub complete: bool,
}

/// The metadata needed to resume one rollout file on its next pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RolloutCheckpoint {
    pub cursor_bytes: u64,
    pub source_bytes: u64,
    pub source_modified_epoch: Option<i64>,
    pub source_identity: String,
    pub complete: bool,
}

struct RawCandidateRow {
    key: SessionKey,
    source_label: String,
    cursor_bytes: u64,
    source_bytes: u64,
    source_modified_epoch: Option<i64>,
    source_identity: String,
    complete: bool,
}

impl Store {
    /// Codex file sessions eligible for a rollout read, newest incomplete
    /// work first, up to `limit` resolvable candidates.
    ///
    /// `cutoff_epoch` excludes a session with no turn activity at or after
    /// it — the same retention cutoff the importer uses for readings, so a
    /// session whose only activity has already aged out of retention is not
    /// scanned for no reason. `now_epoch` excludes a source still in its
    /// retry backoff window.
    pub(crate) fn provider_usage_rollout_candidates(
        &self,
        cutoff_epoch: i64,
        now_epoch: i64,
        limit: usize,
    ) -> Result<Vec<RolloutCandidate>> {
        let raw = self.rollout_candidate_rows(cutoff_epoch, now_epoch)?;
        let keys: Vec<SessionKey> = raw.iter().map(|row| row.key.clone()).collect();
        let bound = self.session_bound_accounts(&keys)?;
        let known = self.provider_known_accounts(OPENAI)?;

        let mut candidates = Vec::with_capacity(limit.min(MAX_CANDIDATE_SCAN_ROWS));
        for row in raw {
            let bound_set = bound.get(&(row.key.clone(), OPENAI.to_string()));
            let known_set = known.get(&row.key.agent);
            let Some(account_key) = resolve_bound_account(bound_set, known_set) else {
                continue;
            };
            candidates.push(RolloutCandidate {
                key: row.key,
                source_label: row.source_label,
                account_key,
                cursor_bytes: row.cursor_bytes,
                source_bytes: row.source_bytes,
                source_modified_epoch: row.source_modified_epoch,
                source_identity: row.source_identity,
                complete: row.complete,
            });
            if candidates.len() >= limit {
                break;
            }
        }
        Ok(candidates)
    }

    fn rollout_candidate_rows(
        &self,
        cutoff_epoch: i64,
        now_epoch: i64,
    ) -> Result<Vec<RawCandidateRow>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT s.environment_key, s.agent, s.session_id, s.source_label,
                    COALESCE(checkpoint.cursor_bytes, 0),
                    COALESCE(checkpoint.source_bytes, 0),
                    checkpoint.source_modified_epoch,
                    COALESCE(checkpoint.source_identity, ''),
                    COALESCE(checkpoint.status = 'complete', 0)
               FROM session s
               JOIN session_evidence e
                 ON e.environment_key = s.environment_key
                AND e.agent = s.agent AND e.session_id = s.session_id
               LEFT JOIN provider_usage_rollout_checkpoint checkpoint
                 ON checkpoint.environment_key = s.environment_key
                AND checkpoint.agent = s.agent
                AND checkpoint.session_id = s.session_id
                AND checkpoint.provider = ?4
              WHERE s.agent = 'codex'
                AND s.source_kind = 'file'
                AND EXISTS (
                    SELECT 1 FROM turn t
                     WHERE t.environment_key = s.environment_key
                       AND t.agent = s.agent AND t.session_id = s.session_id
                       AND t.claim_fence = e.published_fence
                       AND t.ts_ms >= ?1
                )
                AND COALESCE(checkpoint.next_attempt_epoch, 0) <= ?2
              ORDER BY CASE checkpoint.status WHEN 'complete' THEN 1 ELSE 0 END,
                       COALESCE(checkpoint.updated_at_epoch, 0),
                       COALESCE(s.updated_at_epoch, 0) DESC
              LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![
                cutoff_epoch.saturating_mul(1_000),
                now_epoch,
                MAX_CANDIDATE_SCAN_ROWS as i64,
                OPENAI,
            ],
            |row| {
                Ok(RawCandidateRow {
                    key: SessionKey::new(
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ),
                    source_label: row.get(3)?,
                    cursor_bytes: row.get::<_, i64>(4)?.max(0) as u64,
                    source_bytes: row.get::<_, i64>(5)?.max(0) as u64,
                    source_modified_epoch: row.get(6)?,
                    source_identity: row.get(7)?,
                    complete: row.get::<_, i64>(8)? != 0,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Return the next due local retry without reading a rollout file.
    pub(crate) fn rollout_checkpoint_next_retry_epoch(
        &self,
        now_epoch: i64,
    ) -> Result<Option<i64>> {
        let connection = self.lock();
        Ok(connection
            .query_row(
                "SELECT MIN(next_attempt_epoch)
                   FROM provider_usage_rollout_checkpoint
                  WHERE status = 'retry' AND next_attempt_epoch > ?1",
                params![now_epoch],
                |row| row.get(0),
            )
            .optional()?
            .flatten())
    }

    /// Save a checked offset after a bounded rollout file read.
    pub(crate) fn upsert_rollout_checkpoint(
        &self,
        key: &SessionKey,
        source_label: &str,
        checkpoint: &RolloutCheckpoint,
        now_epoch: i64,
    ) -> Result<()> {
        let cursor_bytes = i64::try_from(checkpoint.cursor_bytes).unwrap_or(i64::MAX);
        let connection = self.lock();
        connection.execute(
            "INSERT INTO provider_usage_rollout_checkpoint (
                    environment_key, agent, session_id, provider, source_label,
                    cursor_bytes, source_bytes, source_modified_epoch, source_identity,
                    status, retry_count, next_attempt_epoch, updated_at_epoch, completed_at_epoch
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, 0, ?11, ?12)
             ON CONFLICT (environment_key, agent, session_id, provider)
             DO UPDATE SET source_label = excluded.source_label,
                           cursor_bytes = excluded.cursor_bytes,
                           source_bytes = excluded.source_bytes,
                           source_modified_epoch = excluded.source_modified_epoch,
                           source_identity = excluded.source_identity,
                           status = excluded.status,
                           retry_count = 0,
                           next_attempt_epoch = 0,
                           updated_at_epoch = excluded.updated_at_epoch,
                           completed_at_epoch = excluded.completed_at_epoch",
            params![
                key.environment_key,
                key.agent,
                key.session_id,
                OPENAI,
                source_label,
                cursor_bytes,
                i64::try_from(checkpoint.source_bytes).unwrap_or(i64::MAX),
                checkpoint.source_modified_epoch,
                checkpoint.source_identity,
                if checkpoint.complete {
                    "complete"
                } else {
                    "pending"
                },
                now_epoch,
                checkpoint.complete.then_some(now_epoch),
            ],
        )?;
        Ok(())
    }

    /// Record a completed source inspection without reopening its import.
    pub(crate) fn touch_rollout_checkpoint(&self, key: &SessionKey, now_epoch: i64) -> Result<()> {
        let connection = self.lock();
        connection.execute(
            "UPDATE provider_usage_rollout_checkpoint
                SET updated_at_epoch = ?1, completed_at_epoch = ?1
              WHERE environment_key = ?2 AND agent = ?3 AND session_id = ?4
                AND provider = ?5 AND status = 'complete'",
            params![
                now_epoch,
                key.environment_key,
                key.agent,
                key.session_id,
                OPENAI,
            ],
        )?;
        Ok(())
    }

    /// Delay one failed source without retaining its error text.
    pub(crate) fn defer_rollout_checkpoint(
        &self,
        key: &SessionKey,
        source_label: &str,
        cursor_bytes: u64,
        now_epoch: i64,
    ) -> Result<i64> {
        let connection = self.lock();
        let retries = connection
            .query_row(
                "SELECT retry_count FROM provider_usage_rollout_checkpoint
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND provider = ?4",
                params![key.environment_key, key.agent, key.session_id, OPENAI],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0)
            .clamp(0, 6) as u32;
        let delay = 60_i64.saturating_mul(2_i64.pow(retries)).min(3_600);
        let next_attempt_epoch = now_epoch.saturating_add(delay);
        connection.execute(
            "INSERT INTO provider_usage_rollout_checkpoint (
                    environment_key, agent, session_id, provider, source_label,
                    cursor_bytes, status, retry_count, next_attempt_epoch,
                    updated_at_epoch, completed_at_epoch
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'retry', 1, ?7, ?8, NULL)
             ON CONFLICT (environment_key, agent, session_id, provider)
             DO UPDATE SET source_label = excluded.source_label,
                           status = 'retry',
                           retry_count = MIN(retry_count + 1, 8),
                           next_attempt_epoch = ?7,
                           updated_at_epoch = excluded.updated_at_epoch,
                           completed_at_epoch = NULL",
            params![
                key.environment_key,
                key.agent,
                key.session_id,
                OPENAI,
                source_label,
                i64::try_from(cursor_bytes).unwrap_or(i64::MAX),
                next_attempt_epoch,
                now_epoch,
            ],
        )?;
        Ok(next_attempt_epoch)
    }
}

#[cfg(test)]
mod tests;
