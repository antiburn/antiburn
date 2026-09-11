use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};

use super::{
    Remediation, RemediationEvidenceGuard, RemediationRecord, RemediationResult, RemediationState,
    Store,
};

const MAX_JSON_BYTES: usize = 32_768;
const MAX_AGGREGATE_WINS: usize = 1_000;
const MAX_PASSIVE_ENROLLMENTS_PER_PUBLICATION: usize = 100;
const MAX_DURABLE_REMEDIATIONS: usize = 1_000;
const MAX_DURABLE_CONTRIBUTIONS: usize = 1_000;
const REMEDIATION_COLUMNS: &str = "remediation_id, target_key, environment_key, agent,
    scope_kind, scope_key, state, dirty_revision, evaluated_revision, definition_json,
    result_json, created_at_epoch, updated_at_epoch, effective_boundary_ms,
    verified_at_epoch, recurred_at_epoch, action_joined_at_ms";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PassiveRemediation {
    pub remediation_id: String,
    pub target_key: String,
    pub environment_key: String,
    pub agent: String,
    pub scope_kind: String,
    pub scope_key: String,
    pub definition_json: String,
    pub result_json: String,
    pub display_snapshot_json: String,
    pub boundary_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationDisplaySnapshot {
    pub remediation_id: String,
    pub origin: String,
    pub display_snapshot_json: String,
    pub effective_boundary_ms: i64,
    pub verified_boundary_ms: Option<i64>,
    pub recurred_boundary_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationContribution {
    pub owner_key: String,
    pub remediation_id: String,
    pub detector_id: String,
    pub origin: String,
    pub display_snapshot_json: String,
    pub facts_json: String,
    pub starts_at_ms: i64,
    pub ends_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Store {
    pub(crate) fn mark_remediation_action_joined(
        &self,
        remediation_id: &str,
        action_at_ms: i64,
    ) -> Result<bool> {
        Ok(self.lock().execute(
            "UPDATE remediation SET action_joined_at_ms = COALESCE(action_joined_at_ms, ?2)
              WHERE remediation_id = ?1 AND effective_boundary_ms IS NOT NULL
                AND ?2 >= effective_boundary_ms",
            params![remediation_id, action_at_ms],
        )? == 1)
    }
    /// Stores the bounded presentation facts and exact evidence boundaries for one watch.
    pub fn upsert_remediation_display_snapshot(
        &self,
        snapshot: &RemediationDisplaySnapshot,
    ) -> Result<bool> {
        validate_origin(&snapshot.origin)?;
        validate_safe_json("display_snapshot_json", &snapshot.display_snapshot_json)?;
        validate_boundaries(
            snapshot.effective_boundary_ms,
            snapshot.verified_boundary_ms,
            snapshot.recurred_boundary_ms,
        )?;
        Ok(self.lock().execute(
            "UPDATE remediation
                SET origin = ?2, display_snapshot_json = ?3,
                    effective_boundary_ms = CASE
                        WHEN state IN ('reserved', 'writing', 'recoveryNeeded')
                        THEN effective_boundary_ms ELSE ?4 END,
                    verified_boundary_ms = ?5, recurred_boundary_ms = ?6
              WHERE remediation_id = ?1",
            params![
                snapshot.remediation_id,
                snapshot.origin,
                snapshot.display_snapshot_json,
                snapshot.effective_boundary_ms,
                snapshot.verified_boundary_ms,
                snapshot.recurred_boundary_ms,
            ],
        )? == 1)
    }

    pub fn remediation_display_snapshot(
        &self,
        remediation_id: &str,
    ) -> Result<Option<RemediationDisplaySnapshot>> {
        self.lock()
            .query_row(
                "SELECT remediation_id, origin, display_snapshot_json, effective_boundary_ms,
                        verified_boundary_ms, recurred_boundary_ms
                   FROM remediation
                  WHERE remediation_id = ?1 AND origin IS NOT NULL
                    AND display_snapshot_json IS NOT NULL AND effective_boundary_ms IS NOT NULL",
                [remediation_id],
                display_snapshot_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Inserts or replaces one durable owner contribution with equal or newer facts.
    pub fn upsert_remediation_contribution(
        &self,
        contribution: &RemediationContribution,
    ) -> Result<bool> {
        validate_contribution(contribution)?;
        let connection = self.lock();
        upsert_remediation_contribution_in(&connection, contribution)
    }

    /// Reads the newest aggregate wins with a fixed upper bound.
    pub fn remediation_contributions(&self, limit: usize) -> Result<Vec<RemediationContribution>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit =
            i64::try_from(limit.min(MAX_AGGREGATE_WINS)).expect("the aggregate win limit fits i64");
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT owner_key, remediation_id, detector_id, origin, display_snapshot_json,
                    facts_json, starts_at_ms, ends_at_ms, updated_at_ms
               FROM remediation_contribution
              ORDER BY ends_at_ms DESC, owner_key DESC
              LIMIT ?1",
        )?;
        Ok(statement
            .query_map([limit], contribution_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

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
                && transaction.query_row(
                    "SELECT COALESCE(origin = 'passive', 0)
                       FROM remediation WHERE remediation_id = ?1",
                    [&existing.remediation_id],
                    |row| row.get::<_, bool>(0),
                )?
            {
                let prior_pins = transaction.query_row(
                    "SELECT origin, display_snapshot_json, verified_boundary_ms,
                            recurred_boundary_ms, joined_boundary_ms
                       FROM remediation WHERE remediation_id = ?1",
                    [&existing.remediation_id],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<i64>>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                        ))
                    },
                )?;
                let reserved_result = serde_json::json!({
                    "version": 1,
                    "verification": {"status": "reserved"},
                    "savings": {"status": "pending"},
                    "reservationKind": "upgraded",
                    "priorDefinition": existing.definition_json,
                    "priorResult": existing.result_json,
                    "priorBoundaryMs": existing.effective_boundary_ms,
                    "priorOrigin": prior_pins.0,
                    "priorDisplaySnapshot": prior_pins.1,
                    "priorVerifiedBoundaryMs": prior_pins.2,
                    "priorRecurredBoundaryMs": prior_pins.3,
                    "priorActionJoinedAtMs": existing.action_joined_at_ms,
                    "priorJoinedBoundaryMs": prior_pins.4,
                });
                let reserved_result = reserved_result.to_string();
                validate_json("result_json", &reserved_result)?;
                transaction.execute(
                    "UPDATE remediation SET state = 'reserved', result_json = ?2,
                        definition_json = ?3, effective_boundary_ms = NULL,
                        action_joined_at_ms = COALESCE(action_joined_at_ms, ?5),
                        joined_boundary_ms = COALESCE(joined_boundary_ms, ?6),
                        updated_at_epoch = MAX(updated_at_epoch, ?4)
                      WHERE remediation_id = ?1 AND state = 'watching'",
                    params![
                        existing.remediation_id,
                        reserved_result,
                        remediation.definition_json,
                        remediation.created_at_epoch,
                        remediation.created_at_epoch.saturating_mul(1_000),
                        existing.effective_boundary_ms,
                    ],
                )?;
            }
            transaction.commit()?;
            drop(connection);
            return self.remediation(&existing.remediation_id);
        }
        prune_archivable_fixed_in(&transaction)?;
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
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11
              WHERE (SELECT COUNT(*) FROM remediation WHERE state != 'recurred') < ?12",
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
                remediation.effective_boundary_ms,
                i64::try_from(MAX_DURABLE_REMEDIATIONS).expect("the remediation bound fits i64"),
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
            "UPDATE remediation SET state = 'watching',
                effective_boundary_ms = COALESCE(joined_boundary_ms, ?2),
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
        let verification = serde_json::json!({"status": "recoveryNeeded", "reason": reason});
        Ok(self.lock().execute(
            "UPDATE remediation SET state = 'recoveryNeeded',
                result_json = json_set(result_json, '$.verification', json(?2)),
                updated_at_epoch = MAX(updated_at_epoch, ?3)
              WHERE remediation_id = ?1 AND state IN ('writing', 'recoveryNeeded')",
            params![remediation_id, verification.to_string(), now],
        )? == 1)
    }

    pub(crate) fn mark_remediation_recovery_checked(
        &self,
        remediation_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<bool> {
        let verification = serde_json::json!({
            "status": "recoveryNeeded",
            "reason": reason,
            "checkedAtEpoch": now
        });
        Ok(self.lock().execute(
            "UPDATE remediation SET state = 'recoveryNeeded',
                result_json = json_set(result_json, '$.verification', json(?2)),
                updated_at_epoch = MAX(updated_at_epoch, ?3)
              WHERE remediation_id = ?1 AND state IN ('writing', 'recoveryNeeded')",
            params![remediation_id, verification.to_string(), now],
        )? == 1)
    }

    pub(crate) fn defer_remediation_recovery(
        &self,
        remediation_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<bool> {
        let verification = serde_json::json!({
            "status": "recoveryNeeded",
            "reason": reason,
            "retryAfterEpoch": now.saturating_add(60)
        });
        Ok(self.lock().execute(
            "UPDATE remediation SET state = 'recoveryNeeded',
                result_json = json_set(result_json, '$.verification', json(?2)),
                updated_at_epoch = MAX(updated_at_epoch, ?3)
              WHERE remediation_id = ?1 AND state IN ('writing', 'recoveryNeeded')",
            params![remediation_id, verification.to_string(), now],
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
                WHERE evaluated_revision < dirty_revision
                  AND state IN ('watching', 'fixed', 'recurred')
                ORDER BY updated_at_epoch, remediation_id LIMIT 1"
                ),
                [],
                remediation_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub(crate) fn mark_remediation_evaluated(
        &self,
        remediation_id: &str,
        observed_revision: i64,
        now: i64,
    ) -> Result<bool> {
        Ok(self.lock().execute(
            "UPDATE remediation SET evaluated_revision = ?2,
                updated_at_epoch = MAX(updated_at_epoch, ?3)
              WHERE remediation_id = ?1 AND dirty_revision = ?2 AND evaluated_revision < ?2",
            params![remediation_id, observed_revision, now],
        )? == 1)
    }

    pub fn replace_remediation_result(
        &self,
        remediation_id: &str,
        observed_revision: i64,
        result: &RemediationResult,
    ) -> Result<bool> {
        self.replace_remediation_result_with_contribution(
            remediation_id,
            observed_revision,
            result,
            None,
        )
    }

    pub(crate) fn replace_remediation_result_with_contribution(
        &self,
        remediation_id: &str,
        observed_revision: i64,
        result: &RemediationResult,
        contribution: Option<&RemediationContribution>,
    ) -> Result<bool> {
        validate_json("result_json", &result.result_json)?;
        if let Some(contribution) = contribution {
            validate_contribution(contribution)?;
            ensure!(
                contribution.remediation_id == remediation_id,
                "contribution belongs to another remediation"
            );
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut updated = transaction.execute(
            "UPDATE remediation SET state = ?3, result_json = ?4, evaluated_revision = ?2,
                updated_at_epoch = MAX(updated_at_epoch, ?5),
                verified_at_epoch = CASE WHEN ?3 = 'fixed' THEN COALESCE(verified_at_epoch, ?6 / 1000) ELSE verified_at_epoch END,
                recurred_at_epoch = CASE WHEN ?3 = 'recurred' THEN COALESCE(recurred_at_epoch, ?6 / 1000) ELSE recurred_at_epoch END,
                verified_boundary_ms = CASE WHEN ?3 = 'fixed' THEN COALESCE(verified_boundary_ms, ?6) ELSE verified_boundary_ms END,
                recurred_boundary_ms = CASE WHEN ?3 = 'recurred' THEN COALESCE(recurred_boundary_ms, ?6) ELSE recurred_boundary_ms END
              WHERE remediation_id = ?1 AND dirty_revision = ?2 AND evaluated_revision < ?2
                AND ((state = 'watching' AND ?3 IN ('watching', 'fixed'))
                  OR (state = 'fixed' AND ?3 IN ('fixed', 'recurred')))",
            params![remediation_id, observed_revision, result.state.as_str(), result.result_json, result.evaluated_at_epoch, result.transition_at_ms],
        )? == 1;
        let mut terminal_correction = false;
        let mut terminal_boundary_ms = None;
        if !updated {
            updated = transaction.execute(
                "UPDATE remediation SET result_json = ?3, evaluated_revision = ?2,
                    updated_at_epoch = MAX(updated_at_epoch, ?4),
                    recurred_at_epoch = CASE WHEN ?5 = 'recurred'
                        THEN COALESCE(?6 / 1000, recurred_at_epoch) ELSE recurred_at_epoch END,
                    recurred_boundary_ms = CASE WHEN ?5 = 'recurred'
                        THEN COALESCE(?6, recurred_boundary_ms) ELSE recurred_boundary_ms END
                  WHERE remediation_id = ?1 AND state = 'recurred'
                    AND ?5 = 'recurred'
                    AND dirty_revision = ?2 AND evaluated_revision < ?2",
                params![
                    remediation_id,
                    observed_revision,
                    result.result_json,
                    result.evaluated_at_epoch,
                    result.state.as_str(),
                    result.transition_at_ms,
                ],
            )? == 1;
            if updated {
                terminal_correction = true;
                terminal_boundary_ms = transaction.query_row(
                    "SELECT recurred_boundary_ms FROM remediation WHERE remediation_id = ?1",
                    [remediation_id],
                    |row| row.get::<_, Option<i64>>(0),
                )?;
            }
        }
        if updated {
            transaction.execute(
                "DELETE FROM remediation_contribution WHERE remediation_id = ?1",
                [remediation_id],
            )?;
            // A corrected terminal attempt cannot accrue beyond its corrected recurrence boundary.
            if let Some(contribution) = contribution.filter(|contribution| {
                !terminal_correction
                    || terminal_boundary_ms
                        .is_some_and(|boundary| contribution.ends_at_ms <= boundary)
            }) {
                upsert_remediation_contribution_in(&transaction, contribution)?;
            }
        }
        transaction.commit()?;
        Ok(updated)
    }

    /// Reconciles crash states and makes active watches eligible after startup.
    pub fn reconcile_remediations(&self, now: i64) -> Result<usize> {
        let connection = self.lock();
        connection.execute(
            "UPDATE remediation SET state = 'watching',
                definition_json = json_extract(result_json, '$.priorDefinition'),
                result_json = json_extract(result_json, '$.priorResult'),
                effective_boundary_ms = json_extract(result_json, '$.priorBoundaryMs'),
                origin = json_extract(result_json, '$.priorOrigin'),
                display_snapshot_json = json_extract(result_json, '$.priorDisplaySnapshot'),
                verified_boundary_ms = json_extract(result_json, '$.priorVerifiedBoundaryMs'),
                recurred_boundary_ms = json_extract(result_json, '$.priorRecurredBoundaryMs'),
                action_joined_at_ms = json_extract(result_json, '$.priorActionJoinedAtMs'),
                joined_boundary_ms = json_extract(result_json, '$.priorJoinedBoundaryMs')
              WHERE state = 'reserved' AND json_extract(result_json, '$.reservationKind') = 'upgraded'",
            [],
        )?;
        connection.execute("DELETE FROM remediation WHERE state = 'reserved'", [])?;
        connection.execute(
            "UPDATE remediation SET state = 'recoveryNeeded',
                result_json = json_set(result_json, '$.verification',
                    json('{\"status\":\"recoveryNeeded\",\"reason\":\"writeOutcomeUnknown\"}')),
                updated_at_epoch = MAX(updated_at_epoch, ?1) WHERE state = 'writing'",
            [now],
        )?;
        connection.execute(
            "UPDATE remediation SET
                result_json = json_set(result_json, '$.verification',
                    json('{\"status\":\"recoveryNeeded\",\"reason\":\"writeOutcomeUnknown\"}'))
              WHERE state = 'recoveryNeeded'",
            [],
        )?;
        Ok(connection.execute(
            "UPDATE remediation SET dirty_revision = dirty_revision + 1
              WHERE state IN ('watching', 'fixed')",
            [],
        )?)
    }

    fn restore_or_delete_reservation(&self, remediation_id: &str, state: &str) -> Result<bool> {
        let connection = self.lock();
        let restored = connection.execute(
            "UPDATE remediation SET state = 'watching',
                definition_json = json_extract(result_json, '$.priorDefinition'),
                result_json = json_extract(result_json, '$.priorResult'),
                effective_boundary_ms = json_extract(result_json, '$.priorBoundaryMs'),
                origin = json_extract(result_json, '$.priorOrigin'),
                display_snapshot_json = json_extract(result_json, '$.priorDisplaySnapshot'),
                verified_boundary_ms = json_extract(result_json, '$.priorVerifiedBoundaryMs'),
                recurred_boundary_ms = json_extract(result_json, '$.priorRecurredBoundaryMs'),
                action_joined_at_ms = json_extract(result_json, '$.priorActionJoinedAtMs'),
                joined_boundary_ms = json_extract(result_json, '$.priorJoinedBoundaryMs')
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

fn upsert_remediation_contribution_in(
    connection: &rusqlite::Connection,
    contribution: &RemediationContribution,
) -> Result<bool> {
    let changed = connection.execute(
        "INSERT INTO remediation_contribution (
            owner_key, remediation_id, detector_id, origin, display_snapshot_json,
            facts_json, starts_at_ms, ends_at_ms, updated_at_ms)
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
           FROM remediation WHERE remediation_id = ?2
         ON CONFLICT(owner_key) DO UPDATE SET
            detector_id = excluded.detector_id, origin = excluded.origin,
            display_snapshot_json = excluded.display_snapshot_json,
            facts_json = excluded.facts_json, starts_at_ms = excluded.starts_at_ms,
            ends_at_ms = excluded.ends_at_ms, updated_at_ms = excluded.updated_at_ms
         WHERE remediation_contribution.remediation_id = excluded.remediation_id
           AND remediation_contribution.updated_at_ms <= excluded.updated_at_ms",
        params![
            contribution.owner_key,
            contribution.remediation_id,
            contribution.detector_id,
            contribution.origin,
            contribution.display_snapshot_json,
            contribution.facts_json,
            contribution.starts_at_ms,
            contribution.ends_at_ms,
            contribution.updated_at_ms,
        ],
    )? == 1;
    if changed {
        bound_contributions_in(connection)?;
    }
    Ok(changed)
}

fn bound_contributions_in(connection: &rusqlite::Connection) -> Result<()> {
    connection.execute(
        "DELETE FROM remediation_contribution
          WHERE owner_key IN (
            SELECT owner_key FROM remediation_contribution
             ORDER BY ends_at_ms DESC, owner_key DESC
             LIMIT -1 OFFSET ?1)",
        [i64::try_from(MAX_DURABLE_CONTRIBUTIONS).expect("the contribution bound fits i64")],
    )?;
    Ok(())
}

pub(super) fn mark_remediations_dirty_in(
    connection: &rusqlite::Connection,
    environment: &str,
    agent: &str,
    now: i64,
    correction_replay: bool,
) -> Result<usize> {
    Ok(connection.execute(
        "UPDATE remediation SET dirty_revision = dirty_revision + 1,
            updated_at_epoch = MAX(updated_at_epoch, ?3)
          WHERE environment_key = ?1 AND agent = ?2
            AND (state IN ('watching', 'fixed') OR (?4 AND state = 'recurred'))",
        params![environment, agent, now, correction_replay],
    )?)
}

pub(super) fn enroll_passive_remediations_in(
    connection: &rusqlite::Connection,
    candidates: &[PassiveRemediation],
    created_at_epoch: i64,
) -> Result<usize> {
    let candidate_keys = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            serde_json::json!({
                "index": index,
                "remediationId": candidate.remediation_id,
                "targetKey": candidate.target_key,
                "environmentKey": candidate.environment_key,
                "agent": candidate.agent,
            })
        })
        .collect::<Vec<_>>();
    let candidate_keys = serde_json::to_string(&candidate_keys)?;
    let mut statement = connection.prepare(
        "SELECT CAST(json_extract(candidate.value, '$.index') AS INTEGER)
           FROM json_each(?1) candidate
          WHERE NOT EXISTS (
                    SELECT 1 FROM remediation
                     WHERE remediation_id = json_extract(candidate.value, '$.remediationId'))
            AND NOT EXISTS (
                    SELECT 1 FROM remediation
                     WHERE environment_key = json_extract(candidate.value, '$.environmentKey')
                       AND agent = json_extract(candidate.value, '$.agent')
                       AND target_key = json_extract(candidate.value, '$.targetKey')
                       AND state != 'recurred')
          ORDER BY CAST(json_extract(candidate.value, '$.index') AS INTEGER)
          LIMIT ?2",
    )?;
    let selected = statement
        .query_map(
            params![
                candidate_keys,
                i64::try_from(MAX_PASSIVE_ENROLLMENTS_PER_PUBLICATION)
                    .expect("the passive enrollment bound fits i64")
            ],
            |row| row.get::<_, usize>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    let mut inserted = 0;
    for index in selected {
        prune_archivable_fixed_in(connection)?;
        let candidate = &candidates[index];
        validate_json("definition_json", &candidate.definition_json)?;
        validate_json("result_json", &candidate.result_json)?;
        validate_safe_json("display_snapshot_json", &candidate.display_snapshot_json)?;
        ensure!(candidate.boundary_ms >= 0, "invalid passive boundary");
        inserted += connection.execute(
            "INSERT INTO remediation (
                remediation_id, target_key, environment_key, agent, scope_kind, scope_key,
                state, definition_json, result_json, created_at_epoch, updated_at_epoch,
                effective_boundary_ms, origin, display_snapshot_json)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, 'watching', ?7, ?8, ?9, ?9, ?10,
                    'passive', ?11
              WHERE (SELECT COUNT(*) FROM remediation WHERE state != 'recurred') < ?12
                AND NOT EXISTS (
                SELECT 1 FROM remediation
                 WHERE environment_key = ?3 AND agent = ?4 AND target_key = ?2
                   AND state != 'recurred')
             ON CONFLICT(remediation_id) DO NOTHING",
            params![
                candidate.remediation_id,
                candidate.target_key,
                candidate.environment_key,
                candidate.agent,
                candidate.scope_kind,
                candidate.scope_key,
                candidate.definition_json,
                candidate.result_json,
                created_at_epoch,
                candidate.boundary_ms,
                candidate.display_snapshot_json,
                i64::try_from(MAX_DURABLE_REMEDIATIONS).expect("the remediation bound fits i64"),
            ],
        )?;
    }
    connection.execute(
        "DELETE FROM remediation
          WHERE remediation_id IN (
            SELECT remediation_id FROM remediation WHERE state = 'recurred'
             ORDER BY updated_at_epoch DESC, remediation_id DESC
             LIMIT -1 OFFSET ?1)",
        [i64::try_from(MAX_DURABLE_REMEDIATIONS).expect("the remediation bound fits i64")],
    )?;
    Ok(inserted)
}

/// Removes old fixed watch rows only after their derived contribution is durable.
fn prune_archivable_fixed_in(connection: &rusqlite::Connection) -> Result<usize> {
    let active: usize = connection.query_row(
        "SELECT COUNT(*) FROM remediation WHERE state != 'recurred'",
        [],
        |row| row.get(0),
    )?;
    let remove = active.saturating_sub(MAX_DURABLE_REMEDIATIONS.saturating_sub(1));
    if remove == 0 {
        return Ok(0);
    }
    Ok(connection.execute(
        "DELETE FROM remediation
          WHERE remediation_id IN (
            SELECT r.remediation_id FROM remediation r
             WHERE r.state = 'fixed' AND EXISTS (
                SELECT 1 FROM remediation_contribution c
                 WHERE c.remediation_id = r.remediation_id)
             ORDER BY r.updated_at_epoch, r.remediation_id
             LIMIT ?1)",
        [i64::try_from(remove).expect("the remediation prune count fits i64")],
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
        parsed.get("version").and_then(serde_json::Value::as_u64) == Some(1),
        "{name} has an unsupported version"
    );
    Ok(())
}

fn validate_safe_json(name: &str, value: &str) -> Result<()> {
    validate_json(name, value)?;
    let parsed: serde_json::Value = serde_json::from_str(value)?;
    ensure!(
        parsed.get("version").and_then(serde_json::Value::as_u64) == Some(1),
        "{name} has an unsupported version"
    );
    ensure!(
        has_only_safe_keys(&parsed),
        "{name} contains private fields"
    );
    Ok(())
}

fn has_only_safe_keys(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(values) => values.iter().all(has_only_safe_keys),
        serde_json::Value::Object(values) => values.iter().all(|(key, value)| {
            !matches!(
                key.as_str(),
                "callId"
                    | "config"
                    | "credential"
                    | "definition"
                    | "evidence"
                    | "path"
                    | "prompt"
                    | "selector"
                    | "sessionId"
                    | "sessionIds"
                    | "sourceLabel"
            ) && has_only_safe_keys(value)
        }),
        _ => true,
    }
}

fn validate_origin(origin: &str) -> Result<()> {
    ensure!(matches!(origin, "passive" | "action"), "invalid origin");
    Ok(())
}

fn validate_boundaries(
    effective_boundary_ms: i64,
    verified_boundary_ms: Option<i64>,
    recurred_boundary_ms: Option<i64>,
) -> Result<()> {
    ensure!(effective_boundary_ms >= 0, "invalid effective boundary");
    ensure!(
        verified_boundary_ms.is_none_or(|value| value >= effective_boundary_ms),
        "verified boundary precedes the effective boundary"
    );
    ensure!(
        recurred_boundary_ms
            .zip(verified_boundary_ms)
            .is_none_or(|(recurred, verified)| recurred >= verified)
            && (recurred_boundary_ms.is_none() || verified_boundary_ms.is_some()),
        "recurred boundary has no prior verified boundary"
    );
    Ok(())
}

fn validate_contribution(contribution: &RemediationContribution) -> Result<()> {
    validate_origin(&contribution.origin)?;
    validate_safe_json("display_snapshot_json", &contribution.display_snapshot_json)?;
    validate_safe_json("facts_json", &contribution.facts_json)?;
    ensure!(contribution.starts_at_ms >= 0, "invalid contribution start");
    ensure!(
        contribution.ends_at_ms >= contribution.starts_at_ms,
        "contribution ends before it starts"
    );
    ensure!(
        contribution.updated_at_ms >= contribution.ends_at_ms,
        "contribution update precedes its end"
    );
    Ok(())
}

fn display_snapshot_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RemediationDisplaySnapshot> {
    let display_snapshot_json: String = row.get(2)?;
    validate_envelope_from_row("display_snapshot_json", &display_snapshot_json, 2)?;
    Ok(RemediationDisplaySnapshot {
        remediation_id: row.get(0)?,
        origin: row.get(1)?,
        display_snapshot_json,
        effective_boundary_ms: row.get(3)?,
        verified_boundary_ms: row.get(4)?,
        recurred_boundary_ms: row.get(5)?,
    })
}

fn contribution_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RemediationContribution> {
    let display_snapshot_json: String = row.get(4)?;
    let facts_json: String = row.get(5)?;
    validate_envelope_from_row("display_snapshot_json", &display_snapshot_json, 4)?;
    validate_envelope_from_row("facts_json", &facts_json, 5)?;
    Ok(RemediationContribution {
        owner_key: row.get(0)?,
        remediation_id: row.get(1)?,
        detector_id: row.get(2)?,
        origin: row.get(3)?,
        display_snapshot_json,
        facts_json,
        starts_at_ms: row.get(6)?,
        ends_at_ms: row.get(7)?,
        updated_at_ms: row.get(8)?,
    })
}

fn remediation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RemediationRecord> {
    let state = row
        .get::<_, String>(6)?
        .parse()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let definition_json: String = row.get(9)?;
    let result_json: String = row.get(10)?;
    validate_envelope_from_row("definition_json", &definition_json, 9)?;
    validate_envelope_from_row("result_json", &result_json, 10)?;
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
        definition_json,
        result_json,
        created_at_epoch: row.get(11)?,
        updated_at_epoch: row.get(12)?,
        effective_boundary_ms: row.get(13)?,
        verified_at_epoch: row.get(14)?,
        recurred_at_epoch: row.get(15)?,
        action_joined_at_ms: row.get(16)?,
    })
}

fn validate_envelope_from_row(name: &str, value: &str, column: usize) -> rusqlite::Result<()> {
    validate_json(name, value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, error.into())
    })
}
