//! Read-only candidate lookup for bounded provider-usage backfill work.

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use super::{SessionKey, Store};

const BACKFILL_STATE_KEY: &str = "internal:providerUsageBackfillV1";

/// One directly attributed Codex rollout that has published token evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderUsageBackfillCandidate {
    pub key: SessionKey,
    pub source_label: String,
    pub account_key: String,
    pub cursor_bytes: u64,
    pub source_bytes: u64,
    pub source_modified_epoch: Option<i64>,
    pub source_identity: String,
    pub complete: bool,
}

/// The metadata needed to resume one rollout source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderUsageBackfillCheckpoint {
    pub cursor_bytes: u64,
    pub source_bytes: u64,
    pub source_modified_epoch: Option<i64>,
    pub source_identity: String,
    pub complete: bool,
}

impl Store {
    /// Persist a backfill checkpoint and report an unavailable local store.
    pub(crate) fn write_provider_usage_backfill_state(&self, state: &str) -> Result<()> {
        let connection = self.lock();
        connection.execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![BACKFILL_STATE_KEY, state],
        )?;
        Ok(())
    }

    /// List eligible rollout files with direct account evidence.
    pub(crate) fn provider_usage_backfill_candidates(
        &self,
        cutoff_epoch: i64,
        now_epoch: i64,
        limit: usize,
    ) -> Result<Vec<ProviderUsageBackfillCandidate>> {
        let connection = self.lock();
        let limit = i64::try_from(limit.clamp(1, 16)).expect("bounded limit fits i64");
        let mut statement = connection.prepare(
            "SELECT s.environment_key, s.agent, s.session_id, s.source_label, spa.account_key,
                    COALESCE(checkpoint.cursor_bytes, 0),
                    COALESCE(checkpoint.source_bytes, 0),
                    checkpoint.source_modified_epoch,
                    COALESCE(checkpoint.source_identity, ''),
                    COALESCE(checkpoint.status = 'complete', 0)
               FROM session_provider_account spa
               JOIN session s
                 ON s.environment_key = spa.environment_key
                AND s.agent = spa.agent
                AND s.session_id = spa.session_id
               JOIN session_evidence e
                 ON e.environment_key = s.environment_key
                AND e.agent = s.agent
                AND e.session_id = s.session_id
               LEFT JOIN provider_usage_backfill_checkpoint checkpoint
                 ON checkpoint.environment_key = spa.environment_key
                AND checkpoint.agent = spa.agent
                AND checkpoint.session_id = spa.session_id
                AND checkpoint.provider = spa.provider
                AND checkpoint.account_key = spa.account_key
              WHERE spa.provider = 'openai'
                AND spa.provenance = 'provider_live'
                AND spa.confidence = 'direct'
                AND s.agent = 'codex'
                AND s.source_kind = 'file'
                AND length(spa.account_key) = 64
                AND spa.account_key NOT GLOB '*[^0-9A-Fa-f]*'
                AND EXISTS (
                    SELECT 1 FROM turn t
                     WHERE t.environment_key = s.environment_key
                       AND t.agent = s.agent
                       AND t.session_id = s.session_id
                       AND t.claim_fence = e.published_fence
                       AND t.ts_ms >= ?1
                )
                AND 1 = (
                    SELECT COUNT(*) FROM session_provider_account peer
                     WHERE peer.environment_key = spa.environment_key
                       AND peer.agent = spa.agent
                       AND peer.session_id = spa.session_id
                      AND peer.provider = spa.provider
                )
                AND COALESCE(checkpoint.next_attempt_epoch, 0) <= ?2
              ORDER BY CASE checkpoint.status WHEN 'complete' THEN 1 ELSE 0 END,
                       COALESCE(checkpoint.updated_at_epoch, 0),
                       COALESCE(s.updated_at_epoch, 0) DESC, spa.rowid
              LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![cutoff_epoch.saturating_mul(1_000), now_epoch, limit],
            |row| {
                Ok(ProviderUsageBackfillCandidate {
                    key: SessionKey::new(
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ),
                    source_label: row.get(3)?,
                    account_key: row.get(4)?,
                    cursor_bytes: row.get::<_, i64>(5)?.max(0) as u64,
                    source_bytes: row.get::<_, i64>(6)?.max(0) as u64,
                    source_modified_epoch: row.get(7)?,
                    source_identity: row.get(8)?,
                    complete: row.get::<_, i64>(9)? != 0,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Return the next due local retry without reading a rollout file.
    pub(crate) fn provider_usage_backfill_next_retry_epoch(
        &self,
        now_epoch: i64,
    ) -> Result<Option<i64>> {
        let connection = self.lock();
        Ok(connection
            .query_row(
                "SELECT MIN(next_attempt_epoch)
                   FROM provider_usage_backfill_checkpoint
                  WHERE status = 'retry' AND next_attempt_epoch > ?1",
                params![now_epoch],
                |row| row.get(0),
            )
            .optional()?
            .flatten())
    }

    /// Save a checked offset after a bounded source read.
    pub(crate) fn update_provider_usage_backfill_checkpoint(
        &self,
        candidate: &ProviderUsageBackfillCandidate,
        checkpoint: &ProviderUsageBackfillCheckpoint,
        now_epoch: i64,
    ) -> Result<()> {
        let cursor_bytes = i64::try_from(checkpoint.cursor_bytes).unwrap_or(i64::MAX);
        let connection = self.lock();
        connection.execute(
            "INSERT INTO provider_usage_backfill_checkpoint (
                    environment_key, agent, session_id, provider, account_key,
                    source_label, cursor_bytes, source_bytes, source_modified_epoch, source_identity,
                    status, retry_count, next_attempt_epoch,
                    updated_at_epoch, completed_at_epoch
                ) VALUES (?1, ?2, ?3, 'openai', ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, 0, ?11, ?12)
             ON CONFLICT (environment_key, agent, session_id, provider, account_key)
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
                candidate.key.environment_key,
                candidate.key.agent,
                candidate.key.session_id,
                candidate.account_key,
                candidate.source_label,
                cursor_bytes,
                i64::try_from(checkpoint.source_bytes).unwrap_or(i64::MAX),
                checkpoint.source_modified_epoch,
                checkpoint.source_identity,
                if checkpoint.complete { "complete" } else { "pending" },
                now_epoch,
                checkpoint.complete.then_some(now_epoch),
            ],
        )?;
        Ok(())
    }

    /// Record a completed source inspection without reopening its import.
    pub(crate) fn touch_provider_usage_backfill_checkpoint(
        &self,
        candidate: &ProviderUsageBackfillCandidate,
        now_epoch: i64,
    ) -> Result<()> {
        let connection = self.lock();
        connection.execute(
            "UPDATE provider_usage_backfill_checkpoint
                SET updated_at_epoch = ?1, completed_at_epoch = ?1
              WHERE environment_key = ?2 AND agent = ?3 AND session_id = ?4
                AND provider = 'openai' AND account_key = ?5 AND status = 'complete'",
            params![
                now_epoch,
                candidate.key.environment_key,
                candidate.key.agent,
                candidate.key.session_id,
                candidate.account_key,
            ],
        )?;
        Ok(())
    }

    /// Delay one failed source without retaining its error text.
    pub(crate) fn defer_provider_usage_backfill_candidate(
        &self,
        candidate: &ProviderUsageBackfillCandidate,
        now_epoch: i64,
    ) -> Result<i64> {
        let connection = self.lock();
        let retries = connection
            .query_row(
                "SELECT retry_count FROM provider_usage_backfill_checkpoint
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                    AND provider = 'openai' AND account_key = ?4",
                params![
                    candidate.key.environment_key,
                    candidate.key.agent,
                    candidate.key.session_id,
                    candidate.account_key,
                ],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0)
            .clamp(0, 6) as u32;
        let delay = 60_i64.saturating_mul(2_i64.pow(retries)).min(3_600);
        let next_attempt_epoch = now_epoch.saturating_add(delay);
        connection.execute(
            "INSERT INTO provider_usage_backfill_checkpoint (
                    environment_key, agent, session_id, provider, account_key,
                    source_label, cursor_bytes, status, retry_count, next_attempt_epoch,
                    updated_at_epoch, completed_at_epoch
                ) VALUES (?1, ?2, ?3, 'openai', ?4, ?5, ?6, 'retry', 1, ?7, ?8, NULL)
             ON CONFLICT (environment_key, agent, session_id, provider, account_key)
             DO UPDATE SET source_label = excluded.source_label,
                           status = 'retry',
                           retry_count = MIN(retry_count + 1, 8),
                           next_attempt_epoch = ?7,
                           updated_at_epoch = excluded.updated_at_epoch,
                           completed_at_epoch = NULL",
            params![
                candidate.key.environment_key,
                candidate.key.agent,
                candidate.key.session_id,
                candidate.account_key,
                candidate.source_label,
                i64::try_from(candidate.cursor_bytes).unwrap_or(i64::MAX),
                next_attempt_epoch,
                now_epoch,
            ],
        )?;
        Ok(next_attempt_epoch)
    }
}
