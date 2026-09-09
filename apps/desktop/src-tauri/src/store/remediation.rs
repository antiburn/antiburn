use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};

use super::{
    Remediation, RemediationEvidenceGuard, RemediationRecord, RemediationResult, RemediationState,
    Store,
};

const MAX_JSON_BYTES: usize = 32_768;
const REMEDIATION_COLUMNS: &str = "remediation_id, target_key, environment_key, agent,
    scope_kind, scope_key, state, dirty_revision, evaluated_revision, definition_json,
    result_json, created_at_epoch, updated_at_epoch, effective_boundary_ms,
    verified_at_epoch, recurred_at_epoch";

impl Store {
    /// Creates or reuses one exact watch after all source identities pass in one transaction.
    pub fn create_or_reuse_remediation(
        &self,
        remediation: &Remediation,
        guards: &[RemediationEvidenceGuard],
    ) -> Result<Option<RemediationRecord>> {
        validate_json("definition_json", &remediation.definition_json)?;
        validate_json("result_json", &remediation.result_json)?;
        ensure!(
            !guards.is_empty(),
            "a remediation requires current evidence"
        );
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        for guard in guards {
            let current: bool = transaction.query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM session s JOIN session_evidence e USING (environment_key, agent, session_id)
                     WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
                       AND s.source_generation = ?4 AND e.published_fence = ?5
                       AND s.source_fingerprint IS ?6 AND e.processed_fingerprint IS ?7
                       AND e.parser_revision = ?8 AND e.analyzer_revision = ?9
                       AND e.evidence_schema_revision = ?10 AND e.status = 'ready'
                       AND e.analyzed_generation = s.source_generation)",
                params![guard.environment_key, guard.agent, guard.session_id,
                    guard.source_generation, guard.published_fence, guard.source_fingerprint,
                    guard.processed_fingerprint, guard.parser_revision, guard.analyzer_revision,
                    guard.evidence_schema_revision],
                |row| row.get(0),
            )?;
            if !current {
                return Ok(None);
            }
        }
        let existing = transaction.query_row(
            &format!("SELECT {REMEDIATION_COLUMNS} FROM remediation
                WHERE environment_key = ?1 AND agent = ?2 AND target_key = ?3 AND state != 'recurred'"),
            params![remediation.environment_key, remediation.agent, remediation.target_key],
            remediation_from_row,
        ).optional()?;
        if let Some(existing) = existing {
            if remediation.state == RemediationState::Reserved
                && existing.state == RemediationState::Watching
            {
                let prior_result: serde_json::Value = serde_json::from_str(&existing.result_json)?;
                let reserved_result = serde_json::json!({
                    "version": 1,
                    "verification": {"status": "reserved"},
                    "savings": {"status": "pending"},
                    "reservationKind": "upgraded",
                    "priorResult": prior_result,
                    "priorBoundaryMs": existing.effective_boundary_ms,
                });
                transaction.execute(
                    "UPDATE remediation SET state = 'reserved', result_json = ?2,
                        definition_json = ?3, effective_boundary_ms = NULL,
                        updated_at_epoch = MAX(updated_at_epoch, ?4)
                      WHERE remediation_id = ?1 AND state = 'watching'",
                    params![
                        existing.remediation_id,
                        reserved_result.to_string(),
                        remediation.definition_json,
                        remediation.created_at_epoch
                    ],
                )?;
            }
            transaction.commit()?;
            drop(connection);
            return self.remediation(&existing.remediation_id);
        }
        let result_json = if remediation.state == RemediationState::Reserved {
            serde_json::json!({
                "version": 1,
                "verification": {"status": "reserved"},
                "savings": {"status": "pending"},
                "reservationKind": "new",
            })
            .to_string()
        } else {
            remediation.result_json.clone()
        };
        transaction.execute(
            "INSERT INTO remediation (remediation_id, target_key, environment_key, agent,
                scope_kind, scope_key, state, definition_json, result_json, created_at_epoch,
                updated_at_epoch, effective_boundary_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11)",
            params![
                remediation.remediation_id,
                remediation.target_key,
                remediation.environment_key,
                remediation.agent,
                remediation.scope_kind,
                remediation.scope_key,
                remediation.state.as_str(),
                remediation.definition_json,
                result_json,
                remediation.created_at_epoch,
                remediation.effective_boundary_ms
            ],
        )?;
        transaction.commit()?;
        drop(connection);
        self.remediation(&remediation.remediation_id)
    }

    pub fn begin_remediation_write(&self, remediation_id: &str, now: i64) -> Result<bool> {
        Ok(self.lock().execute(
            "UPDATE remediation SET state = 'writing', updated_at_epoch = MAX(updated_at_epoch, ?2)
              WHERE remediation_id = ?1 AND state = 'reserved'",
            params![remediation_id, now],
        )? == 1)
    }

    pub fn finalize_remediation_write(
        &self,
        remediation_id: &str,
        boundary_ms: i64,
        now: i64,
    ) -> Result<bool> {
        Ok(self.lock().execute(
            "UPDATE remediation SET state = 'watching', effective_boundary_ms = ?2,
                result_json = '{\"version\":1,\"verification\":{\"status\":\"watching\"},\"savings\":{\"status\":\"pending\"}}',
                dirty_revision = dirty_revision + 1, updated_at_epoch = MAX(updated_at_epoch, ?3)
              WHERE remediation_id = ?1 AND state IN ('writing', 'recoveryNeeded')",
            params![remediation_id, boundary_ms, now],
        )? == 1)
    }

    pub fn cancel_remediation_reservation(&self, remediation_id: &str) -> Result<bool> {
        self.restore_or_delete_reservation(remediation_id, "reserved")
    }

    pub fn cancel_pre_replacement_write(&self, remediation_id: &str) -> Result<bool> {
        self.restore_or_delete_reservation(remediation_id, "writing")
    }

    pub fn mark_remediation_recovery_needed(
        &self,
        remediation_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<bool> {
        let result = serde_json::json!({
            "version": 1,
            "verification": {"status": "recoveryNeeded", "reason": reason},
            "savings": {"status": "pending"}
        })
        .to_string();
        Ok(self.lock().execute(
            "UPDATE remediation SET state = 'recoveryNeeded', result_json = ?2,
                updated_at_epoch = MAX(updated_at_epoch, ?3)
              WHERE remediation_id = ?1 AND state IN ('writing', 'recoveryNeeded')",
            params![remediation_id, result, now],
        )? == 1)
    }

    pub(crate) fn mark_remediation_recovery_checked(
        &self,
        remediation_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<bool> {
        let result = serde_json::json!({
            "version": 1,
            "verification": {"status": "recoveryNeeded", "reason": reason, "checkedAtEpoch": now},
            "savings": {"status": "pending"}
        })
        .to_string();
        Ok(self.lock().execute(
            "UPDATE remediation SET state = 'recoveryNeeded', result_json = ?2,
                updated_at_epoch = MAX(updated_at_epoch, ?3)
              WHERE remediation_id = ?1 AND state IN ('writing', 'recoveryNeeded')",
            params![remediation_id, result, now],
        )? == 1)
    }

    pub(crate) fn defer_remediation_recovery(
        &self,
        remediation_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<bool> {
        let result = serde_json::json!({
            "version": 1,
            "verification": {
                "status": "recoveryNeeded",
                "reason": reason,
                "retryAfterEpoch": now.saturating_add(60)
            },
            "savings": {"status": "pending"}
        })
        .to_string();
        Ok(self.lock().execute(
            "UPDATE remediation SET state = 'recoveryNeeded', result_json = ?2,
                updated_at_epoch = MAX(updated_at_epoch, ?3)
              WHERE remediation_id = ?1 AND state IN ('writing', 'recoveryNeeded')",
            params![remediation_id, result, now],
        )? == 1)
    }

    pub fn remediation(&self, remediation_id: &str) -> Result<Option<RemediationRecord>> {
        self.lock()
            .query_row(
                &format!("SELECT {REMEDIATION_COLUMNS} FROM remediation WHERE remediation_id = ?1"),
                [remediation_id],
                remediation_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn latest_remediation_for_target(
        &self,
        environment: &str,
        agent: &str,
        target_key: &str,
    ) -> Result<Option<RemediationRecord>> {
        self.lock()
            .query_row(
                &format!(
                    "SELECT {REMEDIATION_COLUMNS} FROM remediation
                      WHERE environment_key = ?1 AND agent = ?2 AND target_key = ?3
                      ORDER BY (state != 'recurred') DESC, updated_at_epoch DESC,
                               remediation_id DESC LIMIT 1"
                ),
                params![environment, agent, target_key],
                remediation_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn next_remediation_write_recovery(&self, now: i64) -> Result<Option<RemediationRecord>> {
        self.lock()
            .query_row(
                &format!(
                    "SELECT {REMEDIATION_COLUMNS} FROM remediation
                WHERE state IN ('writing', 'recoveryNeeded')
                  AND json_type(result_json, '$.verification.checkedAtEpoch') IS NULL
                  AND COALESCE(json_extract(result_json, '$.verification.retryAfterEpoch'), 0) <= ?1
                ORDER BY updated_at_epoch, remediation_id LIMIT 1"
                ),
                [now],
                remediation_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn next_dirty_remediation(&self) -> Result<Option<RemediationRecord>> {
        self.lock()
            .query_row(
                &format!(
                    "SELECT {REMEDIATION_COLUMNS} FROM remediation
                WHERE evaluated_revision < dirty_revision AND state IN ('watching', 'fixed')
                ORDER BY updated_at_epoch, remediation_id LIMIT 1"
                ),
                [],
                remediation_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn replace_remediation_result(
        &self,
        remediation_id: &str,
        observed_revision: i64,
        result: &RemediationResult,
    ) -> Result<bool> {
        validate_json("result_json", &result.result_json)?;
        Ok(self.lock().execute(
            "UPDATE remediation SET state = ?3, result_json = ?4, evaluated_revision = ?2,
                updated_at_epoch = MAX(updated_at_epoch, ?5),
                verified_at_epoch = CASE WHEN ?3 = 'fixed' THEN COALESCE(verified_at_epoch, ?6) ELSE verified_at_epoch END,
                recurred_at_epoch = CASE WHEN ?3 = 'recurred' THEN COALESCE(recurred_at_epoch, ?6) ELSE recurred_at_epoch END
              WHERE remediation_id = ?1 AND dirty_revision = ?2 AND evaluated_revision < ?2
                AND ((state = 'watching' AND ?3 IN ('watching', 'fixed'))
                  OR (state = 'fixed' AND ?3 IN ('fixed', 'recurred')))",
            params![remediation_id, observed_revision, result.state.as_str(), result.result_json, result.evaluated_at_epoch, result.transition_at_epoch],
        )? == 1)
    }

    /// Reconciles crash states and makes active watches eligible after startup.
    pub fn reconcile_remediations(&self, now: i64) -> Result<usize> {
        let connection = self.lock();
        connection.execute(
            "UPDATE remediation SET state = 'watching',
                result_json = json_extract(result_json, '$.priorResult'),
                effective_boundary_ms = json_extract(result_json, '$.priorBoundaryMs')
              WHERE state = 'reserved' AND json_extract(result_json, '$.reservationKind') = 'upgraded'",
            [],
        )?;
        connection.execute("DELETE FROM remediation WHERE state = 'reserved'", [])?;
        connection.execute(
            "UPDATE remediation SET state = 'recoveryNeeded',
                result_json = '{\"version\":1,\"verification\":{\"status\":\"recoveryNeeded\",\"reason\":\"writeOutcomeUnknown\"},\"savings\":{\"status\":\"pending\"}}',
                updated_at_epoch = MAX(updated_at_epoch, ?1) WHERE state = 'writing'",
            [now],
        )?;
        connection.execute(
            "UPDATE remediation SET
                result_json = '{\"version\":1,\"verification\":{\"status\":\"recoveryNeeded\",\"reason\":\"writeOutcomeUnknown\"},\"savings\":{\"status\":\"pending\"}}'
              WHERE state = 'recoveryNeeded'",
            [],
        )?;
        Ok(connection.execute(
            "UPDATE remediation SET dirty_revision = dirty_revision + 1,
                definition_json = json_set(definition_json, '$.verificationMethodRevision', ?1)
              WHERE state IN ('watching', 'fixed')
                AND COALESCE(json_extract(definition_json, '$.verificationMethodRevision'), 0) != ?1",
            [i64::from(antiburn_local::remediation::VERIFICATION_METHOD_REVISION)],
        )?)
    }

    fn restore_or_delete_reservation(&self, remediation_id: &str, state: &str) -> Result<bool> {
        let connection = self.lock();
        let restored = connection.execute(
            "UPDATE remediation SET state = 'watching',
                result_json = json_extract(result_json, '$.priorResult'),
                effective_boundary_ms = json_extract(result_json, '$.priorBoundaryMs')
              WHERE remediation_id = ?1 AND state = ?2
                AND json_extract(result_json, '$.reservationKind') = 'upgraded'",
            params![remediation_id, state],
        )?;
        if restored == 1 {
            return Ok(true);
        }
        Ok(connection.execute(
            "DELETE FROM remediation WHERE remediation_id = ?1 AND state = ?2
                AND json_extract(result_json, '$.reservationKind') = 'new'",
            params![remediation_id, state],
        )? == 1)
    }
}

pub(super) fn mark_remediations_dirty_in(
    connection: &rusqlite::Connection,
    environment: &str,
    agent: &str,
    now: i64,
) -> Result<usize> {
    Ok(connection.execute(
        "UPDATE remediation SET dirty_revision = dirty_revision + 1,
            updated_at_epoch = MAX(updated_at_epoch, ?3)
          WHERE environment_key = ?1 AND agent = ?2 AND state IN ('watching', 'fixed')",
        params![environment, agent, now],
    )?)
}

fn validate_json(name: &str, value: &str) -> Result<()> {
    ensure!(
        value.len() <= MAX_JSON_BYTES,
        "{name} exceeds {MAX_JSON_BYTES} bytes"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(value).with_context(|| format!("{name} is not valid JSON"))?;
    ensure!(
        parsed
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|value| value > 0),
        "{name} has no version"
    );
    Ok(())
}

fn remediation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RemediationRecord> {
    let state = row
        .get::<_, String>(6)?
        .parse()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(RemediationRecord {
        remediation_id: row.get(0)?,
        target_key: row.get(1)?,
        environment_key: row.get(2)?,
        agent: row.get(3)?,
        scope_kind: row.get(4)?,
        scope_key: row.get(5)?,
        state,
        dirty_revision: row.get(7)?,
        evaluated_revision: row.get(8)?,
        definition_json: row.get(9)?,
        result_json: row.get(10)?,
        created_at_epoch: row.get(11)?,
        updated_at_epoch: row.get(12)?,
        effective_boundary_ms: row.get(13)?,
        verified_at_epoch: row.get(14)?,
        recurred_at_epoch: row.get(15)?,
    })
}
