//! Read-only candidate lookup for bounded provider-usage backfill work.

use anyhow::Result;
use rusqlite::params;

use super::{SessionKey, Store};

const BACKFILL_STATE_KEY: &str = "internal:providerUsageBackfillV1";

/// One directly attributed Codex rollout that has published token evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderUsageBackfillCandidate {
    pub key: SessionKey,
    pub source_label: String,
    pub account_key: String,
    pub cursor_bytes: u64,
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

    /// List directly attributed rollout files after a durable association cursor.
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
                    COALESCE(checkpoint.cursor_bytes, 0)
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
                AND (checkpoint.status IS NULL OR checkpoint.status <> 'complete')
                AND COALESCE(checkpoint.next_attempt_epoch, 0) <= ?2
              ORDER BY COALESCE(checkpoint.updated_at_epoch, 0), spa.rowid
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
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Save a checked offset after a bounded source read.
    pub(crate) fn update_provider_usage_backfill_checkpoint(
        &self,
        candidate: &ProviderUsageBackfillCandidate,
        cursor_bytes: u64,
        now_epoch: i64,
        complete: bool,
    ) -> Result<()> {
        let cursor_bytes = i64::try_from(cursor_bytes).unwrap_or(i64::MAX);
        let connection = self.lock();
        connection.execute(
            "INSERT INTO provider_usage_backfill_checkpoint (
                    environment_key, agent, session_id, provider, account_key,
                    source_label, cursor_bytes, status, retry_count, next_attempt_epoch,
                    updated_at_epoch, completed_at_epoch
                ) VALUES (?1, ?2, ?3, 'openai', ?4, ?5, ?6, ?7, 0, 0, ?8, ?9)
             ON CONFLICT (environment_key, agent, session_id, provider, account_key)
             DO UPDATE SET source_label = excluded.source_label,
                           cursor_bytes = excluded.cursor_bytes,
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
                if complete { "complete" } else { "pending" },
                now_epoch,
                complete.then_some(now_epoch),
            ],
        )?;
        Ok(())
    }

    /// Delay one failed source without retaining its error text.
    pub(crate) fn defer_provider_usage_backfill_candidate(
        &self,
        candidate: &ProviderUsageBackfillCandidate,
        now_epoch: i64,
    ) -> Result<()> {
        let connection = self.lock();
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
                now_epoch.saturating_add(60),
                now_epoch,
            ],
        )?;
        Ok(())
    }
}
