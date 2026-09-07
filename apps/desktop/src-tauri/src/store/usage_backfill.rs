//! Read-only candidate lookup for bounded provider-usage backfill work.

use anyhow::Result;
use rusqlite::params;

use super::Store;

const BACKFILL_STATE_KEY: &str = "internal:providerUsageBackfillV1";

/// One directly attributed Codex rollout that has published token evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderUsageBackfillCandidate {
    pub association_id: i64,
    pub source_label: String,
    pub account_key: String,
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
        after_association_id: i64,
        limit: usize,
    ) -> Result<Vec<ProviderUsageBackfillCandidate>> {
        let connection = self.lock();
        let limit = i64::try_from(limit.clamp(1, 16)).expect("bounded limit fits i64");
        let mut statement = connection.prepare(
            "SELECT spa.rowid, s.source_label, spa.account_key
               FROM session_provider_account spa
               JOIN session s
                 ON s.environment_key = spa.environment_key
                AND s.agent = spa.agent
                AND s.session_id = spa.session_id
               JOIN session_evidence e
                 ON e.environment_key = s.environment_key
                AND e.agent = s.agent
                AND e.session_id = s.session_id
              WHERE spa.rowid > ?1
                AND spa.provider = 'openai'
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
                )
                AND 1 = (
                    SELECT COUNT(*) FROM session_provider_account peer
                     WHERE peer.environment_key = spa.environment_key
                       AND peer.agent = spa.agent
                       AND peer.session_id = spa.session_id
                       AND peer.provider = spa.provider
                )
              ORDER BY spa.rowid
              LIMIT ?2",
        )?;
        let rows = statement.query_map(params![after_association_id.max(0), limit], |row| {
            Ok(ProviderUsageBackfillCandidate {
                association_id: row.get(0)?,
                source_label: row.get(1)?,
                account_key: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
