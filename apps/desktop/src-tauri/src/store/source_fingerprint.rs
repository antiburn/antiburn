//! Persisted strict fingerprint work and completion fences.

use super::*;
use antiburn_local::analysis::SourceFormat;

fn reconciliation_delay(updated_at_epoch: Option<i64>, now: i64) -> i64 {
    match updated_at_epoch.map(|updated| now.saturating_sub(updated).max(0)) {
        None | Some(0..=3600) => 300,
        Some(3601..=86400) => 3600,
        Some(_) => 86400,
    }
}

/// A pending strict hash bound to one committed metadata revision.
#[derive(Debug, Clone)]
pub(crate) struct PendingSourceFingerprint {
    pub(crate) key: SessionKey,
    pub(super) incarnation: u64,
    pub(super) generation: i64,
    pub(super) source_label: String,
    pub(crate) source_format: SourceFormat,
    pub(crate) admission_revision: String,
    pub(crate) source_path: Option<String>,
    pub(super) fingerprint: Option<String>,
    pub(crate) updated_at_epoch: Option<i64>,
    pub(super) global_fence: u64,
}

impl Store {
    #[cfg(test)]
    pub(crate) fn pending_source_fingerprint(
        &self,
        key: &SessionKey,
    ) -> Result<Option<PendingSourceFingerprint>> {
        Ok(self
            .source_fingerprint_snapshot(key)?
            .filter(|pending| pending.fingerprint.is_none()))
    }

    pub(super) fn source_fingerprint_snapshot(
        &self,
        key: &SessionKey,
    ) -> Result<Option<PendingSourceFingerprint>> {
        let connection = self.lock();
        Ok(connection
            .query_row(
                "SELECT incarnation, source_generation, source_label, admission_revision,
                    fingerprint_source_path, source_fingerprint, updated_at_epoch, source_format
                FROM session WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                  AND admission_revision IS NOT NULL AND source_kind = 'providerDb'
                  AND environment_key = 'native' AND agent = 'opencode'
                   AND source_format IN ('\"open_code_sqlite_v2\"', '\"open_code_v2_sqlite\"')",
                params![key.environment_key, key.agent, key.session_id],
                |row| {
                    let source_label: String = row.get(2)?;
                    let source_format =
                        serde_json::from_str(&row.get::<_, String>(7)?).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                7,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?;
                    Ok(PendingSourceFingerprint {
                        key: key.clone(),
                        incarnation: row.get(0)?,
                        generation: row.get(1)?,
                        source_label,
                        source_format,
                        admission_revision: row.get(3)?,
                        source_path: row.get(4)?,
                        fingerprint: row.get(5)?,
                        updated_at_epoch: row.get(6)?,
                        global_fence: self.destructive_write_epoch.load(Ordering::Acquire),
                    })
                },
            )
            .optional()?)
    }

    pub(crate) fn complete_source_fingerprint(
        &self,
        pending: &PendingSourceFingerprint,
        fingerprint: &str,
        now: i64,
    ) -> Result<bool> {
        let mut connection = self.lock();
        if self.destructive_write_epoch.load(Ordering::Acquire) != pending.global_fence {
            return Ok(false);
        }
        let next_attempt = now + reconciliation_delay(pending.updated_at_epoch, now);
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE session SET source_fingerprint = ?8, strict_fingerprint_pending = 0,
                    source_generation = source_generation + CASE
                        WHEN source_fingerprint IS NOT NULL AND source_fingerprint <> ?8 THEN 1 ELSE 0 END,
                    fingerprint_checked_at_epoch = ?10,
                    fingerprint_next_attempt_at_epoch = ?11,
                    fingerprint_retry_count = 0, fingerprint_in_flight = 0
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND incarnation = ?4 AND source_generation = ?5 AND source_label = ?6
                AND source_kind = 'providerDb'
                AND admission_revision = ?7 AND source_fingerprint IS ?9",
            params![pending.key.environment_key, pending.key.agent, pending.key.session_id,
                pending.incarnation, pending.generation, pending.source_label,
                pending.admission_revision, fingerprint, pending.fingerprint, now, next_attempt],
        )? == 1;
        if changed
            && pending
                .fingerprint
                .as_deref()
                .is_some_and(|old| old != fingerprint)
        {
            mark_evidence_pending_in(&transaction, &pending.key)?;
        }
        transaction.commit()?;
        Ok(changed)
    }

    pub(crate) fn next_source_fingerprint_attempt(&self) -> Result<Option<i64>> {
        Ok(self.lock().query_row(
            "SELECT MIN(COALESCE(fingerprint_next_attempt_at_epoch, 0)) FROM session
              WHERE fingerprint_source_path IS NOT NULL AND fingerprint_in_flight = 0
                AND environment_key = 'native' AND agent = 'opencode' AND source_kind = 'providerDb'
                 AND source_format IN ('\"open_code_sqlite_v2\"', '\"open_code_v2_sqlite\"')",
            [],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn claim_source_fingerprints(
        &self,
        now: i64,
        excluded: &HashSet<SessionKey>,
        limit: usize,
        pass_started_at: i64,
    ) -> Result<Vec<PendingSourceFingerprint>> {
        let keys = {
            let connection = self.lock();
            let mut statement = connection.prepare(
                "SELECT environment_key, agent, session_id FROM session
                  WHERE fingerprint_source_path IS NOT NULL AND fingerprint_in_flight = 0
                    AND environment_key = 'native' AND agent = 'opencode' AND source_kind = 'providerDb'
                     AND source_format IN ('\"open_code_sqlite_v2\"', '\"open_code_v2_sqlite\"')
                    AND COALESCE(fingerprint_next_attempt_at_epoch, 0) <= ?1
                    AND (strict_fingerprint_pending = 1 OR fingerprint_checked_at_epoch IS NULL
                         OR fingerprint_checked_at_epoch < ?3)
                  ORDER BY COALESCE(fingerprint_next_attempt_at_epoch, 0), first_seen_at LIMIT ?2"
            )?;
            statement
                .query_map(
                    params![now, limit + excluded.len(), pass_started_at],
                    |row| {
                        Ok(SessionKey::new(
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut work = Vec::new();
        for key in keys
            .into_iter()
            .filter(|key| !excluded.contains(key))
            .take(limit)
        {
            if let Some(pending) = self.source_fingerprint_snapshot(&key)?
                && pending.source_path.is_some()
            {
                let claimed = self.lock().execute(
                    "UPDATE session SET fingerprint_in_flight = 1 WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                       AND incarnation = ?4 AND source_generation = ?5 AND admission_revision = ?6
                       AND source_fingerprint IS ?7 AND fingerprint_in_flight = 0
                       AND source_kind = 'providerDb' AND fingerprint_source_path IS ?9
                       AND source_label = ?10
                       AND COALESCE(fingerprint_next_attempt_at_epoch, 0) <= ?8",
                    params![key.environment_key, key.agent, key.session_id, pending.incarnation,
                        pending.generation, pending.admission_revision, pending.fingerprint, now,
                        pending.source_path, pending.source_label],
                )? == 1;
                if claimed {
                    work.push(pending);
                }
            }
        }
        Ok(work)
    }

    pub(crate) fn retry_source_fingerprint(
        &self,
        pending: &PendingSourceFingerprint,
        now: i64,
        cancelled: bool,
    ) -> Result<()> {
        self.lock().execute(
            "UPDATE session SET fingerprint_in_flight = 0,
                    fingerprint_next_attempt_at_epoch = MAX(COALESCE(fingerprint_next_attempt_at_epoch, 0),
                        ?4 + CASE WHEN ?5 THEN 0 ELSE MIN(300, 1 << MIN(fingerprint_retry_count, 9)) END),
                    fingerprint_retry_count = fingerprint_retry_count + CASE WHEN ?5 THEN 0 ELSE 1 END
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND incarnation = ?6 AND source_label = ?7 AND fingerprint_source_path IS ?8
                AND source_kind = 'providerDb' AND source_generation = ?9
                AND admission_revision = ?10 AND source_fingerprint IS ?11",
            params![pending.key.environment_key, pending.key.agent, pending.key.session_id, now, cancelled,
                pending.incarnation, pending.source_label, pending.source_path,
                pending.generation, pending.admission_revision, pending.fingerprint],
        )?;
        Ok(())
    }

    pub(crate) fn quarantine_missing_source_fingerprint(
        &self,
        pending: &PendingSourceFingerprint,
    ) -> Result<()> {
        self.lock().execute(
            "UPDATE session SET fingerprint_source_path = NULL, source_fingerprint = NULL,
                    strict_fingerprint_pending = 0, fingerprint_in_flight = 0,
                    fingerprint_next_attempt_at_epoch = NULL, fingerprint_retry_count = 0
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND incarnation = ?4 AND source_generation = ?5 AND admission_revision = ?6
                AND source_label = ?7 AND fingerprint_source_path IS ?8
                AND source_kind = 'providerDb'",
            params![
                pending.key.environment_key,
                pending.key.agent,
                pending.key.session_id,
                pending.incarnation,
                pending.generation,
                pending.admission_revision,
                pending.source_label,
                pending.source_path
            ],
        )?;
        Ok(())
    }

    pub(crate) fn defer_source_fingerprint_retry(
        &self,
        pending: &PendingSourceFingerprint,
        now: i64,
    ) -> Result<()> {
        self.lock().execute(
            "UPDATE session SET fingerprint_next_attempt_at_epoch = MAX(
                COALESCE(fingerprint_next_attempt_at_epoch, 0),
                ?4 + MIN(300, 1 << MIN(fingerprint_retry_count - 1, 9)))
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND incarnation = ?5 AND source_label = ?6 AND fingerprint_source_path IS ?7
                AND source_kind = 'providerDb' AND fingerprint_retry_count > 0",
            params![
                pending.key.environment_key,
                pending.key.agent,
                pending.key.session_id,
                now,
                pending.incarnation,
                pending.source_label,
                pending.source_path
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::reconciliation_delay;

    #[test]
    fn strict_reconciliation_backs_off_as_sessions_become_idle() {
        assert_eq!(reconciliation_delay(None, 100_000), 300);
        assert_eq!(reconciliation_delay(Some(99_000), 100_000), 300);
        assert_eq!(reconciliation_delay(Some(96_399), 100_000), 3600);
        assert_eq!(reconciliation_delay(Some(13_599), 100_000), 86400);
        assert_eq!(reconciliation_delay(Some(100_001), 100_000), 300);
    }
}
