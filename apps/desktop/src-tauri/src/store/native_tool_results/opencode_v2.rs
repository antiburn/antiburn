//! Fenced capture progress for OpenCode V2 event sessions.

use std::path::PathBuf;

use antiburn_local::analysis::opencode_v2_capture::{
    CaptureReconciliation, MAX_RECONCILIATION_BINDINGS, MAX_RECONCILIATION_BYTES,
};
use antiburn_local::analysis::{
    SourceFormat, ToolResultChange, ToolResultObservation, ToolResultPosition,
};
use antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS;
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::super::publications::{
    PublicationPolicy, ToolResultBatch, publish_coverage_in, reconcile_with_policy,
};
use crate::store::native_tool_results::opencode_v2_status::OpenCodeV2SourceStatus;
use crate::store::{Incarnation, SessionKey, SessionRecord, Store};

const MAX_CAPTURE_STARTS: usize = 256;
const MAX_EPOCH_BYTES: usize = 128;
const MAX_GAP_REASON_BYTES: usize = 64;
const MAX_RETIREMENT_RESULTS: usize = 256;
// Reserve space for optional fields that older JSON omits.
const RECONCILIATION_FIELD_RESERVE_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenCodeV2ReconciliationPreparation {
    Ready,
    Progress,
    Fenced,
}

#[derive(Debug, Clone)]
pub(crate) struct OpenCodeV2CaptureStart {
    pub key: SessionKey,
    pub incarnation: Incarnation,
    pub source_generation: i64,
    pub source_label: String,
    pub cwd: String,
    pub admitted_source: OpenCodeV2AdmittedSource,
    pub baseline_sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OpenCodeV2AdmittedSource {
    pub database_path: PathBuf,
    pub file_identity: String,
}

impl OpenCodeV2AdmittedSource {
    pub(crate) fn from_activity_cursor(cursor: &str, session_id: &str) -> Option<Self> {
        let cursor = cursor.strip_prefix("opencode-admission-v2:")?;
        let (database_path, admitted_session, file_identity, scope): (
            PathBuf,
            String,
            String,
            String,
        ) = serde_json::from_str(cursor).ok()?;
        (database_path.is_absolute()
            && admitted_session == session_id
            && !file_identity.is_empty()
            && file_identity.len() <= 512
            && !scope.is_empty())
        .then_some(Self {
            database_path,
            file_identity,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OpenCodeV2CaptureClaim {
    pub key: SessionKey,
    pub incarnation: Incarnation,
    pub source_generation: i64,
    pub source_label: String,
    pub cwd: String,
    pub admitted_source: Option<OpenCodeV2AdmittedSource>,
    pub capture_epoch: String,
    pub claim_fence: i64,
    pub baseline_sequence: Option<u64>,
    pub last_sequence: Option<u64>,
    pub history_gap: bool,
}

impl Store {
    /// Load the current persisted claims for exact native OpenCode sessions.
    pub(crate) fn opencode_v2_capture_claims(
        &self,
        keys: &[SessionKey],
    ) -> Result<Vec<OpenCodeV2CaptureClaim>> {
        ensure!(
            keys.len() <= MAX_CAPTURE_STARTS,
            "invalid V2 claim lookup size"
        );
        let connection = self.lock();
        let mut claims = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for key in keys {
            if key.environment_key != "native"
                || key.agent != "opencode"
                || !seen.insert(key.clone())
            {
                continue;
            }
            let claim = connection
                .query_row(
                    "SELECT d.incarnation, d.source_generation, d.source_label, d.cwd,
                            d.capture_epoch, d.claim_fence, d.baseline_sequence, d.last_sequence,
                            d.history_gap, d.admitted_source_json
                       FROM opencode_v2_capture d JOIN session s
                         USING (environment_key, agent, session_id)
                      WHERE d.environment_key = ?1 AND d.agent = ?2 AND d.session_id = ?3
                        AND d.incarnation = s.incarnation",
                    params![key.environment_key, key.agent, key.session_id],
                    |row| {
                        let admitted_source_json: Option<String> = row.get(9)?;
                        Ok(OpenCodeV2CaptureClaim {
                            key: key.clone(),
                            incarnation: Incarnation(row.get(0)?),
                            source_generation: row.get(1)?,
                            source_label: row.get(2)?,
                            cwd: row.get(3)?,
                            capture_epoch: row.get(4)?,
                            claim_fence: row.get(5)?,
                            baseline_sequence: row.get(6)?,
                            last_sequence: row.get(7)?,
                            history_gap: row.get(8)?,
                            admitted_source: admitted_source_json
                                .map(|json| {
                                    serde_json::from_str(&json).map_err(|error| {
                                        rusqlite::Error::FromSqlConversionFailure(
                                            9,
                                            rusqlite::types::Type::Text,
                                            Box::new(error),
                                        )
                                    })
                                })
                                .transpose()?,
                        })
                    },
                )
                .optional()?;
            if let Some(claim) = claim {
                claims.push(claim);
            }
        }
        Ok(claims)
    }

    /// Return lease-free prior claims whose admitted source or scope changed.
    pub(crate) fn detached_opencode_v2_capture_claims(
        &self,
        records: &[SessionRecord],
        now: i64,
    ) -> Result<Vec<OpenCodeV2CaptureClaim>> {
        ensure!(
            records.len() <= MAX_CAPTURE_STARTS,
            "invalid V2 detached scope size"
        );
        let connection = self.lock();
        let mut claims = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for record in records {
            if record.key.environment_key != "native"
                || record.key.agent != "opencode"
                || !seen.insert(record.key.clone())
            {
                continue;
            }
            let Some(current_source) = OpenCodeV2AdmittedSource::from_activity_cursor(
                &record.activity_cursor,
                &record.key.session_id,
            ) else {
                continue;
            };
            let row = connection
                .query_row(
                    "SELECT d.incarnation, d.source_generation, d.source_label, d.cwd,
                            d.capture_epoch, d.claim_fence, d.baseline_sequence, d.last_sequence,
                            d.history_gap, d.admitted_source_json
                       FROM opencode_v2_capture d JOIN session s
                         USING (environment_key, agent, session_id)
                      WHERE d.environment_key = ?1 AND d.agent = ?2 AND d.session_id = ?3
                        AND d.incarnation = s.incarnation AND d.lease_expires_at_epoch IS NULL",
                    params![
                        record.key.environment_key,
                        record.key.agent,
                        record.key.session_id
                    ],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, Option<i64>>(6)?,
                            row.get::<_, Option<i64>>(7)?,
                            row.get::<_, bool>(8)?,
                            row.get::<_, Option<String>>(9)?,
                        ))
                    },
                )
                .optional()?;
            let Some((
                incarnation,
                source_generation,
                source_label,
                cwd,
                capture_epoch,
                claim_fence,
                baseline_sequence,
                last_sequence,
                history_gap,
                admitted_source_json,
            )) = row
            else {
                continue;
            };
            if source_label != record.source_label
                || record
                    .cwd
                    .as_deref()
                    .is_none_or(|current_cwd| current_cwd != cwd)
            {
                // A moved source can only retire data while its original label still exists.
                if source_label != format!("opencode:{}:open_code_v2_sqlite", record.key.session_id)
                {
                    continue;
                }
            }
            let Some(old_source) = admitted_source_json
                .as_deref()
                .map(serde_json::from_str::<OpenCodeV2AdmittedSource>)
                .transpose()?
            else {
                continue;
            };
            if old_source == current_source
                && source_label == record.source_label
                && record.cwd.as_deref() == Some(cwd.as_str())
            {
                continue;
            }
            claims.push(OpenCodeV2CaptureClaim {
                key: record.key.clone(),
                incarnation: Incarnation(incarnation as u64),
                source_generation,
                source_label,
                cwd,
                admitted_source: Some(old_source),
                capture_epoch,
                claim_fence,
                baseline_sequence: baseline_sequence.map(|value| value as u64),
                last_sequence: last_sequence.map(|value| value as u64),
                history_gap,
            });
        }
        ensure!(now >= 0, "V2 detached-claim time must be non-negative");
        Ok(claims)
    }

    /// Bound current evidence before hydration. Keep evicted identities as tombstones.
    pub(crate) fn prepare_opencode_v2_reconciliation(
        &self,
        records: &[SessionRecord],
        now: i64,
    ) -> Result<OpenCodeV2ReconciliationPreparation> {
        ensure!(
            !records.is_empty() && records.len() <= MAX_CAPTURE_STARTS,
            "invalid V2 reconciliation scope size"
        );
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut claims = Vec::with_capacity(records.len());
        let mut seen = std::collections::HashSet::new();
        for record in records {
            ensure!(
                seen.insert(record.key.clone()),
                "duplicate V2 reconciliation scope"
            );
            let Some(claim) = preparation_claim(&tx, record, now)? else {
                return Ok(OpenCodeV2ReconciliationPreparation::Fenced);
            };
            claims.push(claim);
        }
        for claim in &claims {
            if let Some((pending, reason)) = pending_scope_retirement(&tx, claim)? {
                let mut remaining = MAX_RETIREMENT_RESULTS;
                if !retire_scope_bindings(&tx, &pending, &reason, &mut remaining)? {
                    clear_scope_retirement(&tx, &pending)?;
                }
                tx.commit()?;
                return Ok(OpenCodeV2ReconciliationPreparation::Progress);
            }
        }
        let mut count = 0usize;
        let mut bytes = 0usize;
        for claim in &claims {
            let mut statement = tx.prepare(
                "SELECT rowid, length(CAST(payload_json AS BLOB))
                   FROM native_tool_result_current INDEXED BY native_tool_result_current_session
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                    AND is_removed = 0 AND source_format = ?4 ORDER BY rowid LIMIT ?5",
            )?;
            let mut rows = statement.query(params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                serde_json::to_string(&SourceFormat::OpenCodeSqliteV2)?,
                MAX_RECONCILIATION_BINDINGS + 1 - count
            ])?;
            let mut oldest = Vec::new();
            let mut overflow = false;
            while let Some(row) = rows.next()? {
                if oldest.len() < MAX_RETIREMENT_RESULTS {
                    oldest.push(row.get::<_, i64>(0)?);
                }
                count += 1;
                bytes = bytes
                    .saturating_add(row.get::<_, usize>(1)?)
                    .saturating_add(RECONCILIATION_FIELD_RESERVE_BYTES);
                if count > MAX_RECONCILIATION_BINDINGS || bytes > MAX_RECONCILIATION_BYTES {
                    overflow = true;
                    break;
                }
            }
            drop(rows);
            drop(statement);
            if overflow {
                for rowid in oldest {
                    retire_stored_binding(&tx, claim, rowid, "v2-current-retention")?;
                }
                publish_gap(&tx, claim, "v2-current-retention")?;
                save_capture_status(&tx, claim, OpenCodeV2SourceStatus::CaptureGap, now)?;
                tx.commit()?;
                return Ok(OpenCodeV2ReconciliationPreparation::Progress);
            }
        }
        tx.commit()?;
        Ok(OpenCodeV2ReconciliationPreparation::Ready)
    }

    /// Continue exact scope retirement after the capture lease is released.
    pub(crate) fn continue_opencode_v2_scope_retirement(
        &self,
        claims: &[OpenCodeV2CaptureClaim],
        reason: &str,
        now: i64,
    ) -> Result<OpenCodeV2ReconciliationPreparation> {
        ensure!(
            scope_retirement_reason(reason),
            "invalid V2 scope retirement reason"
        );
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !capture_claims_match(&tx, claims, now, false)? {
            return Ok(OpenCodeV2ReconciliationPreparation::Fenced);
        }
        let mut pending_claims = Vec::new();
        for claim in claims {
            if !retirement_session_matches(&tx, claim)? {
                return Ok(OpenCodeV2ReconciliationPreparation::Fenced);
            }
            let saved_reason: Option<String> = tx.query_row(
                "SELECT retirement_reason FROM opencode_v2_capture
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                params![
                    claim.key.environment_key,
                    claim.key.agent,
                    claim.key.session_id
                ],
                |row| row.get(0),
            )?;
            match saved_reason.as_deref() {
                Some(saved) if saved == reason => pending_claims.push(claim),
                Some(_) => return Ok(OpenCodeV2ReconciliationPreparation::Fenced),
                None => {}
            }
        }
        let mut remaining = MAX_RETIREMENT_RESULTS;
        let mut pending = false;
        for claim in pending_claims {
            let more = retire_scope_bindings(&tx, claim, reason, &mut remaining)?;
            if !more {
                clear_scope_retirement(&tx, claim)?;
            }
            pending |= more;
        }
        tx.commit()?;
        Ok(if pending {
            OpenCodeV2ReconciliationPreparation::Progress
        } else {
            OpenCodeV2ReconciliationPreparation::Ready
        })
    }

    /// Read exact current bindings before taking the source snapshot.
    pub(crate) fn opencode_v2_reconciliation_targets(
        &self,
        records: &[SessionRecord],
    ) -> Result<Vec<ToolResultObservation>> {
        ensure!(
            records.len() <= MAX_CAPTURE_STARTS,
            "invalid V2 reconciliation scope size"
        );
        let connection = self.lock();
        let mut targets = Vec::new();
        let mut bytes = 0usize;
        let mut sessions = std::collections::HashSet::new();
        for record in records {
            ensure!(
                record.key.environment_key == "native"
                    && record.key.agent == "opencode"
                    && record.source_label
                        == format!("opencode:{}:open_code_v2_sqlite", record.key.session_id)
                    && sessions.insert(record.key.clone()),
                "invalid V2 reconciliation scope"
            );
            let mut statement = connection.prepare(
                "SELECT length(CAST(c.payload_json AS BLOB)), c.payload_json
                   FROM native_tool_result_current c JOIN session s
                     USING (environment_key, agent, session_id)
                  WHERE c.environment_key = ?1 AND c.agent = ?2 AND c.session_id = ?3
                    AND c.source_format = ?4 AND c.is_removed = 0
                    AND s.source_kind = 'providerDb' AND s.source_label = ?5 AND s.cwd = ?6
                    AND s.wsl_distro IS NULL ORDER BY c.native_identity LIMIT ?7",
            )?;
            let mut rows = statement.query(params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id,
                serde_json::to_string(&SourceFormat::OpenCodeSqliteV2)?,
                record.source_label,
                record.cwd,
                MAX_RECONCILIATION_BINDINGS + 1
            ])?;
            while let Some(row) = rows.next()? {
                bytes = bytes.saturating_add(row.get::<_, usize>(0)?);
                ensure!(
                    targets.len() < MAX_RECONCILIATION_BINDINGS
                        && bytes <= MAX_RECONCILIATION_BYTES,
                    antiburn_local::analysis::OpenCodeV2SnapshotError::SourceLimit
                );
                targets.push(serde_json::from_str(row.get_ref(1)?.as_str()?)?);
            }
        }
        Ok(targets)
    }

    /// Clears capture leases left by a worker that stopped.
    pub(crate) fn recover_opencode_v2_capture_claims(&self, now: i64) -> Result<usize> {
        ensure!(now >= 0, "V2 capture recovery time must be non-negative");
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let claims = {
            let mut statement = tx.prepare(
                "SELECT environment_key, agent, session_id, incarnation, source_generation,
                        source_label, cwd, capture_epoch, claim_fence, baseline_sequence,
                        last_sequence, history_gap, lease_expires_at_epoch, admitted_source_json
                   FROM opencode_v2_capture
                  WHERE lease_expires_at_epoch IS NOT NULL
                  ORDER BY environment_key, agent, session_id",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        OpenCodeV2CaptureClaim {
                            key: SessionKey::new(
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                            ),
                            incarnation: Incarnation(row.get(3)?),
                            source_generation: row.get(4)?,
                            source_label: row.get(5)?,
                            cwd: row.get(6)?,
                            admitted_source: row
                                .get::<_, Option<String>>(13)?
                                .map(|json| {
                                    serde_json::from_str(&json).map_err(|error| {
                                        rusqlite::Error::FromSqlConversionFailure(
                                            13,
                                            rusqlite::types::Type::Text,
                                            Box::new(error),
                                        )
                                    })
                                })
                                .transpose()?,
                            capture_epoch: row.get(7)?,
                            claim_fence: row.get(8)?,
                            baseline_sequence: row.get(9)?,
                            last_sequence: row.get(10)?,
                            history_gap: row.get(11)?,
                        },
                        row.get::<_, i64>(12)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (claim, lease_expires_at_epoch) in &claims {
            if capture_session_matches(&tx, claim)? {
                publish_gap(
                    &tx,
                    claim,
                    if *lease_expires_at_epoch <= now {
                        "capture-expired"
                    } else {
                        "capture-recovered"
                    },
                )?;
            }
            tx.execute(
                "UPDATE opencode_v2_capture SET history_gap = 1,
                        lease_expires_at_epoch = NULL
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                    AND capture_epoch = ?4 AND claim_fence = ?5
                    AND lease_expires_at_epoch IS NOT NULL",
                params![
                    claim.key.environment_key,
                    claim.key.agent,
                    claim.key.session_id,
                    claim.capture_epoch,
                    claim.claim_fence
                ],
            )?;
        }
        tx.commit()?;
        Ok(claims.len())
    }

    /// Returns recent or previously admitted V2 sessions in an enabled Live scope.
    pub(crate) fn opencode_v2_live_candidates(
        &self,
        now: i64,
        limit: usize,
    ) -> Result<Vec<SessionRecord>> {
        ensure!(limit <= 1024, "candidate limit exceeds 1024");
        if limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT s.environment_key, s.agent, s.session_id, s.source_kind,
                    s.source_label, s.wsl_distro, s.title, s.title_source, s.cwd,
                    s.surface, s.updated_at_epoch, s.activity_cursor,
                    s.activity_source, s.subagent_count, s.source_fingerprint
               FROM session s
              WHERE s.environment_key = 'native' AND s.agent = 'opencode'
                AND s.source_kind = 'providerDb' AND s.wsl_distro IS NULL
                AND s.source_label = 'opencode:' || s.session_id || ':open_code_v2_sqlite'
                AND s.cwd IS NOT NULL AND s.cwd <> ''
                AND (s.updated_at_epoch >= ?1 - ?2 OR EXISTS (
                    SELECT 1 FROM opencode_v2_capture d
                     WHERE d.environment_key = s.environment_key AND d.agent = s.agent
                       AND d.session_id = s.session_id AND d.incarnation = s.incarnation
                        AND d.source_label = s.source_label AND d.cwd = s.cwd
                        AND d.capture_epoch <> '' AND d.claim_fence > 0))
                AND NOT EXISTS (
                    SELECT 1 FROM session_relation r
                     WHERE r.environment_key = s.environment_key AND r.agent = s.agent
                       AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
                         OR (r.related_id = s.session_id AND r.kind = 'subagent')))
                AND EXISTS (
                    SELECT 1 FROM burn_check_activation a
                     WHERE a.enabled = 1
                       AND json_extract(a.activation_scope_key, '$.environment') = s.environment_key
                       AND json_extract(a.activation_scope_key, '$.session_cwd') = s.cwd
                       AND json_extract(a.activation_scope_key, '$.run') = 'Live')
              ORDER BY s.updated_at_epoch DESC, s.session_id ASC LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![now, ACTIVE_SESSION_WINDOW_SECS, limit as i64],
            |row| {
                Ok(SessionRecord {
                    key: SessionKey::new(
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ),
                    source_kind: row.get(3)?,
                    source_label: row.get(4)?,
                    wsl_distro: row.get(5)?,
                    title: row.get(6)?,
                    title_source: row.get(7)?,
                    cwd: row.get(8)?,
                    surface: row.get(9)?,
                    updated_at_epoch: row.get(10)?,
                    activity_cursor: row.get(11)?,
                    activity_source: row.get(12)?,
                    subagent_count: row.get(13)?,
                    fork_parent_session_id: None,
                    source_fingerprint: row.get(14)?,
                })
            },
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    #[cfg(test)]
    pub(crate) fn start_opencode_v2_capture_epoch(
        &self,
        starts: &[OpenCodeV2CaptureStart],
        capture_epoch: &str,
        now: i64,
        lease_secs: i64,
    ) -> Result<Option<Vec<OpenCodeV2CaptureClaim>>> {
        self.start_opencode_v2_capture_epoch_inner(starts, None, capture_epoch, now, lease_secs)
    }

    pub(crate) fn start_opencode_v2_capture_epoch_reconciled(
        &self,
        starts: &[OpenCodeV2CaptureStart],
        snapshot: &CaptureReconciliation,
        capture_epoch: &str,
        now: i64,
        lease_secs: i64,
    ) -> Result<Option<Vec<OpenCodeV2CaptureClaim>>> {
        ensure!(
            snapshot.baseline.len() == starts.len()
                && starts
                    .iter()
                    .all(|start| snapshot.baseline.get(&start.key.session_id)
                        == Some(&start.baseline_sequence)),
            "V2 reconciliation watermark does not match capture start"
        );
        ensure!(
            snapshot.bindings.len() <= MAX_RECONCILIATION_BINDINGS,
            "V2 reconciliation exceeds its binding limit"
        );
        self.start_opencode_v2_capture_epoch_inner(
            starts,
            Some(snapshot),
            capture_epoch,
            now,
            lease_secs,
        )
    }

    fn start_opencode_v2_capture_epoch_inner(
        &self,
        starts: &[OpenCodeV2CaptureStart],
        snapshot: Option<&CaptureReconciliation>,
        capture_epoch: &str,
        now: i64,
        lease_secs: i64,
    ) -> Result<Option<Vec<OpenCodeV2CaptureClaim>>> {
        ensure!(
            !starts.is_empty() && starts.len() <= MAX_CAPTURE_STARTS,
            "invalid V2 capture set size"
        );
        ensure!(
            !capture_epoch.is_empty() && capture_epoch.len() <= MAX_EPOCH_BYTES,
            "invalid V2 capture epoch"
        );
        ensure!(lease_secs > 0, "V2 capture lease must be positive");
        let lease_expiry = now
            .checked_add(lease_secs)
            .context("V2 capture lease overflow")?;
        ensure!(starts.iter().all(valid_start), "invalid V2 capture start");
        let mut seen = std::collections::HashSet::new();
        ensure!(
            starts.iter().all(|start| seen.insert(start.key.clone())),
            "duplicate V2 capture session"
        );

        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for start in starts {
            ensure!(
                valid_admitted_source(&start.admitted_source),
                "invalid V2 admitted database identity"
            );
            if !scope_matches(
                &tx,
                &start.key,
                start.incarnation,
                Some(start.source_generation),
                &start.source_label,
                &start.cwd,
            )? || read_admitted_source(
                &tx,
                &start.key,
                start.incarnation,
                &start.source_label,
                &start.cwd,
            )? != Some(start.admitted_source.clone())
                || tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM opencode_v2_capture
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                    AND incarnation = ?4 AND source_label = ?5 AND cwd = ?6
                    AND lease_expires_at_epoch > ?7)",
                    params![
                        start.key.environment_key,
                        start.key.agent,
                        start.key.session_id,
                        start.incarnation.0,
                        start.source_label,
                        start.cwd,
                        now,
                    ],
                    |row| row.get::<_, bool>(0),
                )?
            {
                return Ok(None);
            }
        }
        let mut claims = Vec::with_capacity(starts.len());
        for start in starts {
            let previous_fence: Option<i64> = tx
                .query_row(
                    "SELECT claim_fence FROM opencode_v2_capture
                      WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                    params![
                        start.key.environment_key,
                        start.key.agent,
                        start.key.session_id
                    ],
                    |row| row.get(0),
                )
                .optional()?;
            let fence = previous_fence
                .unwrap_or(0)
                .checked_add(1)
                .context("V2 capture fence overflow")?;
            let baseline = sqlite_sequence(start.baseline_sequence)?;
            tx.execute(
                "INSERT INTO opencode_v2_capture (
                    environment_key, agent, session_id, incarnation, source_generation,
                    source_label, cwd, capture_epoch, claim_fence, baseline_sequence,
                    last_sequence, history_gap, lease_expires_at_epoch,
                    admitted_source_json, retirement_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, 1, ?11, ?12, NULL)
                 ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
                    incarnation = excluded.incarnation,
                    source_generation = excluded.source_generation,
                    source_label = excluded.source_label, cwd = excluded.cwd,
                    capture_epoch = excluded.capture_epoch, claim_fence = excluded.claim_fence,
                    baseline_sequence = excluded.baseline_sequence,
                    last_sequence = excluded.last_sequence, history_gap = 1,
                    lease_expires_at_epoch = excluded.lease_expires_at_epoch,
                    admitted_source_json = excluded.admitted_source_json,
                    retirement_reason = NULL",
                params![
                    start.key.environment_key,
                    start.key.agent,
                    start.key.session_id,
                    start.incarnation.0,
                    start.source_generation,
                    start.source_label,
                    start.cwd,
                    capture_epoch,
                    fence,
                    baseline,
                    lease_expiry,
                    serde_json::to_string(&start.admitted_source)?
                ],
            )?;
            let claim = OpenCodeV2CaptureClaim {
                key: start.key.clone(),
                incarnation: start.incarnation,
                source_generation: start.source_generation,
                source_label: start.source_label.clone(),
                cwd: start.cwd.clone(),
                admitted_source: Some(start.admitted_source.clone()),
                capture_epoch: capture_epoch.to_owned(),
                claim_fence: fence,
                baseline_sequence: start.baseline_sequence,
                last_sequence: start.baseline_sequence,
                history_gap: true,
            };
            publish_gap(&tx, &claim, "cold-attach")?;
            save_capture_status(&tx, &claim, OpenCodeV2SourceStatus::Live, now)?;
            claims.push(claim);
        }
        if let Some(snapshot) = snapshot
            && !reconcile_capture_bindings(&tx, &claims, snapshot)?
        {
            return Ok(None);
        }
        tx.commit()?;
        Ok(Some(claims))
    }

    pub(crate) fn renew_opencode_v2_capture_epoch(
        &self,
        claims: &[OpenCodeV2CaptureClaim],
        now: i64,
        lease_secs: i64,
    ) -> Result<bool> {
        ensure!(lease_secs > 0, "V2 capture lease must be positive");
        let lease_expiry = now
            .checked_add(lease_secs)
            .context("V2 capture lease overflow")?;
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !claims_valid(&tx, claims, now, true)? {
            return Ok(false);
        }
        for claim in claims {
            tx.execute(
                "UPDATE opencode_v2_capture SET lease_expires_at_epoch = ?5
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                    AND capture_epoch = ?4 AND claim_fence = ?6",
                params![
                    claim.key.environment_key,
                    claim.key.agent,
                    claim.key.session_id,
                    claim.capture_epoch,
                    lease_expiry,
                    claim.claim_fence
                ],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn advance_opencode_v2_capture(
        &self,
        claim: &OpenCodeV2CaptureClaim,
        sequence: u64,
        result: Option<&ToolResultObservation>,
        now: i64,
        lease_secs: i64,
    ) -> Result<bool> {
        ensure!(lease_secs > 0, "V2 capture lease must be positive");
        let lease_expiry = now
            .checked_add(lease_secs)
            .context("V2 capture lease overflow")?;
        let sequence_sql = sqlite_sequence(Some(sequence))?.context("missing V2 event sequence")?;
        if result.is_some_and(|_| {
            claim
                .baseline_sequence
                .is_some_and(|baseline| sequence <= baseline)
        }) {
            return Ok(false);
        }
        if let Some(result) = result
            && (result.source_format != SourceFormat::OpenCodeSqliteV2
                || !matches!(result.position, ToolResultPosition::OpenCodeV2Event { sequence: n, .. } if n == sequence)
                || result.session_id != claim.key.session_id
                || result.record_id.as_deref().is_none_or(str::is_empty)
                || result.call_id.as_deref().is_none_or(str::is_empty)
                || !result.finality
                || result.change != ToolResultChange::Update)
        {
            return Ok(false);
        }
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !claims_valid(&tx, std::slice::from_ref(claim), now, true)? {
            return Ok(false);
        }
        let (baseline_sequence, last_sequence): (Option<i64>, Option<i64>) = tx.query_row(
            "SELECT baseline_sequence, last_sequence FROM opencode_v2_capture
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND capture_epoch = ?4 AND claim_fence = ?5",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                claim.capture_epoch,
                claim.claim_fence
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if baseline_sequence != sqlite_sequence(claim.baseline_sequence)?
            || sequence_sql <= baseline_sequence.unwrap_or(-1)
            || last_sequence.is_some_and(|last| sequence_sql <= last)
        {
            return Ok(false);
        }
        let publish_result = if let Some(result) = result {
            match existing_terminal_matches(&tx, &claim.key, result)? {
                Some(false) => return Ok(false),
                Some(true) => false,
                None => true,
            }
        } else {
            false
        };
        tx.execute(
            "UPDATE opencode_v2_capture SET last_sequence = ?6,
                    lease_expires_at_epoch = ?7
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND capture_epoch = ?4 AND claim_fence = ?5",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                claim.capture_epoch,
                claim.claim_fence,
                sequence_sql,
                lease_expiry
            ],
        )?;
        if let Some(result) = result.filter(|_| publish_result) {
            let batch = ToolResultBatch {
                source_format: SourceFormat::OpenCodeSqliteV2,
                results: vec![result.clone()],
                complete: false,
            };
            let complete = result
                .context
                .as_ref()
                .is_some_and(|context| context.main_session && context.complete_through_event);
            reconcile_with_policy(
                &tx,
                &claim.key,
                current_source_generation(&tx, &claim.key)?,
                claim.claim_fence,
                &batch,
                PublicationPolicy {
                    baseline: claim.baseline_sequence,
                    history_complete: Some(complete),
                    gap_reason: (!complete).then_some("v2-history-incomplete"),
                    live_eligible: Some(true),
                    ..PublicationPolicy::default()
                },
            )?;
            tx.execute(
                "UPDATE opencode_v2_capture SET history_gap = ?6
                      WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                        AND capture_epoch = ?4 AND claim_fence = ?5",
                params![
                    claim.key.environment_key,
                    claim.key.agent,
                    claim.key.session_id,
                    claim.capture_epoch,
                    claim.claim_fence,
                    i64::from(!complete)
                ],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn mark_opencode_v2_capture_gap(
        &self,
        claims: &[OpenCodeV2CaptureClaim],
        reason: &str,
        now: i64,
    ) -> Result<usize> {
        ensure!(valid_gap_reason(reason), "invalid V2 capture gap reason");
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut released = 0;
        let mut retirement_remaining = MAX_RETIREMENT_RESULTS;
        for claim in claims {
            if !capture_claims_match(&tx, std::slice::from_ref(claim), now, false)? {
                continue;
            }
            let retire_scope = scope_retirement_reason(reason);
            let session_matches = if retire_scope {
                retirement_session_matches(&tx, claim)?
            } else {
                capture_session_matches(&tx, claim)?
            };
            if session_matches {
                if retire_scope {
                    retire_scope_bindings(&tx, claim, reason, &mut retirement_remaining)?;
                }
                publish_gap(&tx, claim, reason)?;
                let status = match reason {
                    "event-version-unsupported" => OpenCodeV2SourceStatus::VersionUnsupported,
                    "source-limit" | "recovery-capacity" => OpenCodeV2SourceStatus::SourceLimit,
                    _ => OpenCodeV2SourceStatus::CaptureGap,
                };
                save_capture_status(&tx, claim, status, now)?;
            }
            tx.execute(
                "UPDATE opencode_v2_capture SET history_gap = 1,
                        lease_expires_at_epoch = NULL,
                        retirement_reason = CASE WHEN ?6 THEN ?7 ELSE retirement_reason END
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                    AND capture_epoch = ?4 AND claim_fence = ?5",
                params![
                    claim.key.environment_key,
                    claim.key.agent,
                    claim.key.session_id,
                    claim.capture_epoch,
                    claim.claim_fence,
                    scope_retirement_reason(reason),
                    reason
                ],
            )?;
            released += 1;
        }
        tx.commit()?;
        Ok(released)
    }
}

fn result_identity(result: &ToolResultObservation) -> Result<String> {
    Ok(serde_json::to_string(&(
        &result.session_id,
        &result.record_id,
        &result.call_id,
        &result.result_id,
    ))?)
}

fn scope_retirement_reason(reason: &str) -> bool {
    matches!(
        reason,
        "session-moved" | "session-deleted" | "source-scope-changed"
    )
}

fn retirement_session_matches(
    tx: &Transaction<'_>,
    claim: &OpenCodeV2CaptureClaim,
) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM session WHERE environment_key = ?1 AND agent = ?2
          AND session_id = ?3 AND incarnation = ?4 AND source_label = ?5
          AND source_kind = 'providerDb' AND wsl_distro IS NULL)",
        params![
            claim.key.environment_key,
            claim.key.agent,
            claim.key.session_id,
            claim.incarnation.0,
            claim.source_label
        ],
        |row| row.get(0),
    )?)
}

fn pending_scope_retirement(
    tx: &Transaction<'_>,
    current: &OpenCodeV2CaptureClaim,
) -> Result<Option<(OpenCodeV2CaptureClaim, String)>> {
    Ok(tx
        .query_row(
            "SELECT cwd, capture_epoch, claim_fence, retirement_reason FROM opencode_v2_capture
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
            AND incarnation = ?4 AND source_label = ?5 AND retirement_reason IS NOT NULL",
            params![
                current.key.environment_key,
                current.key.agent,
                current.key.session_id,
                current.incarnation.0,
                current.source_label
            ],
            |row| {
                let mut pending = current.clone();
                pending.cwd = row.get(0)?;
                pending.capture_epoch = row.get(1)?;
                pending.claim_fence = row.get(2)?;
                Ok((pending, row.get(3)?))
            },
        )
        .optional()?)
}

fn clear_scope_retirement(tx: &Transaction<'_>, claim: &OpenCodeV2CaptureClaim) -> Result<()> {
    let cursor: Option<String> = tx
        .query_row(
            "SELECT activity_cursor FROM session
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND incarnation = ?4 AND source_kind = 'providerDb' AND wsl_distro IS NULL",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                claim.incarnation.0
            ],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let admitted_source = cursor
        .as_deref()
        .and_then(|cursor| {
            OpenCodeV2AdmittedSource::from_activity_cursor(cursor, &claim.key.session_id)
        })
        .map(|source| serde_json::to_string(&source))
        .transpose()?;
    tx.execute(
        "UPDATE opencode_v2_capture SET retirement_reason = NULL,
                admitted_source_json = COALESCE(?6, admitted_source_json)
      WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
        AND capture_epoch = ?4 AND claim_fence = ?5",
        params![
            claim.key.environment_key,
            claim.key.agent,
            claim.key.session_id,
            claim.capture_epoch,
            claim.claim_fence,
            admitted_source
        ],
    )?;
    Ok(())
}

fn preparation_claim(
    tx: &Transaction<'_>,
    record: &SessionRecord,
    now: i64,
) -> Result<Option<OpenCodeV2CaptureClaim>> {
    if record.key.environment_key != "native"
        || record.key.agent != "opencode"
        || record.source_label != format!("opencode:{}:open_code_v2_sqlite", record.key.session_id)
    {
        return Ok(None);
    }
    let Some(admitted_source) = OpenCodeV2AdmittedSource::from_activity_cursor(
        &record.activity_cursor,
        &record.key.session_id,
    ) else {
        return Ok(None);
    };
    let state = tx.query_row(
        "SELECT s.incarnation, s.source_generation, COALESCE(d.claim_fence, 0),
                COALESCE(d.capture_epoch, 'current-retention'), d.admitted_source_json
           FROM session s LEFT JOIN opencode_v2_capture d
             ON d.environment_key = s.environment_key AND d.agent = s.agent AND d.session_id = s.session_id
               AND d.incarnation = s.incarnation AND d.source_label = s.source_label AND d.cwd = s.cwd
          WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
            AND s.source_kind = 'providerDb' AND s.source_label = ?4 AND s.cwd = ?5
            AND s.activity_cursor IS ?6 AND s.wsl_distro IS NULL
            AND (d.lease_expires_at_epoch IS NULL OR d.lease_expires_at_epoch <= ?7)",
        params![record.key.environment_key, record.key.agent, record.key.session_id,
            record.source_label, record.cwd, record.activity_cursor, now],
         |row| Ok((Incarnation(row.get(0)?), row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, String>(3)?, row.get::<_, Option<String>>(4)?)),
    ).optional()?;
    let Some((incarnation, source_generation, claim_fence, capture_epoch, stored_source)) = state
    else {
        return Ok(None);
    };
    if claim_fence > 0
        && stored_source.as_deref() != Some(serde_json::to_string(&admitted_source)?.as_str())
    {
        return Ok(None);
    }
    let Some(cwd) = record.cwd.as_ref() else {
        return Ok(None);
    };
    if !scope_matches(
        tx,
        &record.key,
        incarnation,
        Some(source_generation),
        &record.source_label,
        cwd,
    )? {
        return Ok(None);
    }
    Ok(Some(OpenCodeV2CaptureClaim {
        key: record.key.clone(),
        incarnation,
        source_generation,
        source_label: record.source_label.clone(),
        cwd: cwd.clone(),
        admitted_source: Some(admitted_source),
        capture_epoch,
        claim_fence,
        baseline_sequence: None,
        last_sequence: None,
        history_gap: true,
    }))
}

fn retire_scope_bindings(
    tx: &Transaction<'_>,
    claim: &OpenCodeV2CaptureClaim,
    reason: &str,
    remaining: &mut usize,
) -> Result<bool> {
    let mut statement = tx.prepare(
        "SELECT c.rowid FROM native_tool_result_current c INDEXED BY native_tool_result_current_session
           JOIN native_tool_result_publication p ON p.publication_sequence = c.publication_sequence
          WHERE c.environment_key = ?1 AND c.agent = ?2 AND c.session_id = ?3
            AND c.source_format = ?4 AND c.is_removed = 0
            AND (p.project_cwd = ?6 OR (p.project_cwd IS NULL AND ?6 = ?7))
            AND (p.session_incarnation = ?8 OR p.session_incarnation IS NULL)
          ORDER BY c.rowid LIMIT ?5",
    )?;
    let rowids = statement
        .query_map(
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                serde_json::to_string(&SourceFormat::OpenCodeSqliteV2)?,
                *remaining,
                claim.cwd,
                current_cwd(tx, &claim.key)?,
                claim.incarnation.0
            ],
            |row| row.get::<_, i64>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    *remaining -= rowids.len();
    for rowid in rowids {
        retire_stored_binding(tx, claim, rowid, reason)?;
    }
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_tool_result_current c
           JOIN native_tool_result_publication p ON p.publication_sequence = c.publication_sequence
          WHERE c.environment_key = ?1 AND c.agent = ?2 AND c.session_id = ?3
            AND c.source_format = ?4 AND c.is_removed = 0
            AND (p.project_cwd = ?5 OR (p.project_cwd IS NULL AND ?5 = ?6))
            AND (p.session_incarnation = ?7 OR p.session_incarnation IS NULL))",
        params![
            claim.key.environment_key,
            claim.key.agent,
            claim.key.session_id,
            serde_json::to_string(&SourceFormat::OpenCodeSqliteV2)?,
            claim.cwd,
            current_cwd(tx, &claim.key)?,
            claim.incarnation.0
        ],
        |row| row.get(0),
    )?)
}

fn retire_stored_binding(
    tx: &Transaction<'_>,
    claim: &OpenCodeV2CaptureClaim,
    rowid: i64,
    reason: &str,
) -> Result<()> {
    let generation = current_source_generation(tx, &claim.key)?;
    let inserted = tx.execute(
        "INSERT INTO native_tool_result_publication (
           outcome, environment_key, agent, session_id, source_format, native_identity,
           native_position_json, native_revision, payload_json, complete, source_generation,
           claim_fence, live_eligible, gap_reason, project_cwd, session_incarnation)
         SELECT 'removal', c.environment_key, c.agent, c.session_id, c.source_format,
            c.native_identity, c.native_position_json, c.native_revision, c.payload_json, 0, ?6, ?7, 0, ?8,
           COALESCE(p.project_cwd, ?9), COALESCE(p.session_incarnation, ?10)
           FROM native_tool_result_current c
           JOIN native_tool_result_publication p ON p.publication_sequence = c.publication_sequence
          WHERE c.rowid = ?1 AND c.environment_key = ?2
             AND c.agent = ?3 AND c.session_id = ?4 AND c.source_format = ?5 AND c.is_removed = 0",
        params![
            rowid,
            claim.key.environment_key,
            claim.key.agent,
            claim.key.session_id,
            serde_json::to_string(&SourceFormat::OpenCodeSqliteV2)?,
            generation,
            claim.claim_fence,
            reason,
            claim.cwd,
            claim.incarnation.0
        ],
    )?;
    ensure!(inserted == 1, "V2 retirement lost its exact binding");
    tx.execute(
        "UPDATE native_tool_result_current SET is_removed = 1, complete = 0,
        publication_sequence = ?2, source_generation = ?3, claim_fence = ?4 WHERE rowid = ?1",
        params![rowid, tx.last_insert_rowid(), generation, claim.claim_fence],
    )?;
    Ok(())
}

fn current_cwd(tx: &Transaction<'_>, key: &SessionKey) -> Result<Option<String>> {
    Ok(tx.query_row(
        "SELECT cwd FROM session WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![key.environment_key, key.agent, key.session_id],
        |row| row.get(0),
    )?)
}

fn reconcile_capture_bindings(
    tx: &Transaction<'_>,
    claims: &[OpenCodeV2CaptureClaim],
    snapshot: &CaptureReconciliation,
) -> Result<bool> {
    let format = serde_json::to_string(&SourceFormat::OpenCodeSqliteV2)?;
    let mut seen = std::collections::HashSet::new();
    let mut bytes = 0usize;
    for binding in &snapshot.bindings {
        let result = &binding.observation;
        let Some(claim) = claims
            .iter()
            .find(|claim| claim.key.session_id == result.session_id)
        else {
            return Ok(false);
        };
        let identity = result_identity(result)?;
        if result.source_format != SourceFormat::OpenCodeSqliteV2
            || !seen.insert((result.session_id.clone(), identity.clone()))
        {
            return Ok(false);
        }
        bytes = bytes.saturating_add(serde_json::to_vec(result)?.len());
        ensure!(
            bytes <= MAX_RECONCILIATION_BYTES,
            "V2 reconciliation exceeds its byte limit"
        );
        let saved: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM native_tool_result_current
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND source_format = ?4 AND native_identity = ?5 AND is_removed = 0",
                params![
                    claim.key.environment_key,
                    claim.key.agent,
                    claim.key.session_id,
                    format,
                    identity
                ],
                |row| row.get(0),
            )
            .optional()?;
        if saved
            .as_deref()
            .map(serde_json::from_str::<ToolResultObservation>)
            .transpose()?
            .as_ref()
            != Some(result)
        {
            return Ok(false);
        }
    }
    for claim in claims {
        let count: usize = tx.query_row(
            "SELECT COUNT(*) FROM (SELECT 1 FROM native_tool_result_current
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND source_format = ?4 AND is_removed = 0 LIMIT ?5)",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                format,
                MAX_RECONCILIATION_BINDINGS + 1
            ],
            |row| row.get(0),
        )?;
        if count
            != snapshot
                .bindings
                .iter()
                .filter(|binding| binding.observation.session_id == claim.key.session_id)
                .count()
        {
            return Ok(false);
        }
    }
    for binding in snapshot.bindings.iter().filter(|binding| !binding.retained) {
        let claim = claims
            .iter()
            .find(|claim| claim.key.session_id == binding.observation.session_id)
            .context("missing V2 reconciliation claim")?;
        let mut removal = binding.observation.clone();
        removal.change = ToolResultChange::Removal;
        removal.revision = removal
            .revision
            .checked_add(1)
            .context("V2 retirement revision overflow")?;
        reconcile_with_policy(
            tx,
            &claim.key,
            current_source_generation(tx, &claim.key)?,
            claim.claim_fence,
            &ToolResultBatch {
                source_format: SourceFormat::OpenCodeSqliteV2,
                results: vec![removal],
                complete: false,
            },
            PublicationPolicy {
                suppress_coverage: true,
                live_eligible: Some(false),
                ..PublicationPolicy::default()
            },
        )?;
    }
    Ok(true)
}

fn existing_terminal_matches(
    tx: &Transaction<'_>,
    key: &SessionKey,
    result: &ToolResultObservation,
) -> Result<Option<bool>> {
    let mut prefix = serde_json::to_string(&(&result.session_id, &result.record_id))?;
    prefix.pop();
    let lower = format!("{prefix},");
    let upper = format!("{prefix}-");
    let mut statement = tx.prepare(
        "SELECT CASE WHEN is_removed = 0 THEN payload_json END, is_removed,
                json_extract(native_identity, '$[2]') FROM native_tool_result_current
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND source_format = ?4
            AND native_identity >= ?5 AND native_identity < ?6 ORDER BY native_identity",
    )?;
    let mut rows = statement.query(params![
        key.environment_key,
        key.agent,
        key.session_id,
        serde_json::to_string(&SourceFormat::OpenCodeSqliteV2)?,
        lower,
        upper
    ])?;
    let mut found = false;
    while let Some(row) = rows.next()? {
        if row.get::<_, Option<String>>(2)? != result.call_id {
            continue;
        }
        found = true;
        if row.get::<_, bool>(1)? {
            continue;
        }
        let old: ToolResultObservation = serde_json::from_str(row.get_ref(0)?.as_str()?)?;
        if old.tool_name != result.tool_name
            || old.tool_category != result.tool_category
            || old.arguments != result.arguments
            || old.content != result.content
            || old.status != result.status
            || old.outcome != result.outcome
            || old.provider_executed != result.provider_executed
        {
            return Ok(Some(false));
        }
    }
    Ok(found.then_some(true))
}

fn valid_start(start: &OpenCodeV2CaptureStart) -> bool {
    start.key.environment_key == "native"
        && start.key.agent == "opencode"
        && start.source_label == format!("opencode:{}:open_code_v2_sqlite", start.key.session_id)
        && !start.cwd.is_empty()
        && start.cwd.len() <= 4096
        && start.source_generation >= 0
        && valid_admitted_source(&start.admitted_source)
        && start
            .baseline_sequence
            .is_none_or(|sequence| i64::try_from(sequence).is_ok())
}

fn valid_admitted_source(source: &OpenCodeV2AdmittedSource) -> bool {
    source.database_path.is_absolute()
        && source.database_path.as_os_str().to_string_lossy().len() <= 4096
        && !source.file_identity.is_empty()
        && source.file_identity.len() <= 512
        && !source.file_identity.chars().any(char::is_control)
}

fn current_admitted_source(
    tx: &Transaction<'_>,
    claim: &OpenCodeV2CaptureClaim,
) -> Result<Option<OpenCodeV2AdmittedSource>> {
    read_admitted_source(
        tx,
        &claim.key,
        claim.incarnation,
        &claim.source_label,
        &claim.cwd,
    )
}

fn read_admitted_source(
    tx: &Transaction<'_>,
    key: &SessionKey,
    incarnation: Incarnation,
    source_label: &str,
    cwd: &str,
) -> Result<Option<OpenCodeV2AdmittedSource>> {
    let cursor: Option<String> = tx
        .query_row(
            "SELECT activity_cursor FROM session
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND incarnation = ?4 AND source_kind = 'providerDb' AND source_label = ?5
                AND cwd = ?6 AND wsl_distro IS NULL",
            params![
                key.environment_key,
                key.agent,
                key.session_id,
                incarnation.0,
                source_label,
                cwd
            ],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    Ok(cursor.and_then(|cursor| {
        OpenCodeV2AdmittedSource::from_activity_cursor(&cursor, &key.session_id)
    }))
}

fn sqlite_sequence(sequence: Option<u64>) -> Result<Option<i64>> {
    sequence
        .map(|value| i64::try_from(value).context("V2 sequence exceeds SQLite range"))
        .transpose()
}

fn scope_matches(
    tx: &Transaction<'_>,
    key: &SessionKey,
    incarnation: Incarnation,
    generation: Option<i64>,
    source_label: &str,
    cwd: &str,
) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM session s
          WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
             AND s.incarnation = ?4 AND (?5 IS NULL OR s.source_generation = ?5)
            AND s.source_kind = 'providerDb' AND s.source_label = ?6 AND s.cwd = ?7
            AND s.cwd <> '' AND s.wsl_distro IS NULL
            AND NOT EXISTS (SELECT 1 FROM session_relation r
              WHERE r.environment_key = s.environment_key AND r.agent = s.agent
                AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
                  OR (r.related_id = s.session_id AND r.kind = 'subagent')))
            AND EXISTS (SELECT 1 FROM burn_check_activation a WHERE a.enabled = 1
              AND json_extract(a.activation_scope_key, '$.environment') = s.environment_key
              AND json_extract(a.activation_scope_key, '$.session_cwd') = s.cwd
              AND json_extract(a.activation_scope_key, '$.run') = 'Live'))",
        params![
            key.environment_key,
            key.agent,
            key.session_id,
            incarnation.0,
            generation,
            source_label,
            cwd
        ],
        |row| row.get(0),
    )?)
}

fn claims_valid(
    tx: &Transaction<'_>,
    claims: &[OpenCodeV2CaptureClaim],
    now: i64,
    require_lease: bool,
) -> Result<bool> {
    if !capture_claims_match(tx, claims, now, require_lease)? {
        return Ok(false);
    }
    for claim in claims {
        if !scope_matches(
            tx,
            &claim.key,
            claim.incarnation,
            None,
            &claim.source_label,
            &claim.cwd,
        )? {
            return Ok(false);
        }
        let Some(admitted_source) = claim.admitted_source.as_ref() else {
            return Ok(false);
        };
        if current_admitted_source(tx, claim)? != Some(admitted_source.clone()) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn capture_claims_match(
    tx: &Transaction<'_>,
    claims: &[OpenCodeV2CaptureClaim],
    now: i64,
    require_lease: bool,
) -> Result<bool> {
    if claims.is_empty() || claims.len() > MAX_CAPTURE_STARTS {
        return Ok(false);
    }
    let mut seen = std::collections::HashSet::new();
    for claim in claims {
        if !seen.insert(claim.key.clone()) {
            return Ok(false);
        }
        let valid: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM opencode_v2_capture
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND incarnation = ?4 AND source_generation = ?5 AND source_label = ?6
                 AND cwd = ?7 AND capture_epoch = ?8 AND claim_fence = ?9
                 AND (?10 = 0 OR lease_expires_at_epoch > ?11)
                 AND admitted_source_json IS ?12)",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                claim.incarnation.0,
                claim.source_generation,
                claim.source_label,
                claim.cwd,
                claim.capture_epoch,
                claim.claim_fence,
                i64::from(require_lease),
                now,
                claim
                    .admitted_source
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?
            ],
            |row| row.get(0),
        )?;
        if !valid {
            return Ok(false);
        }
    }
    Ok(true)
}

fn capture_session_matches(tx: &Transaction<'_>, claim: &OpenCodeV2CaptureClaim) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM session s
          WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
             AND s.incarnation = ?4
             AND s.source_kind = 'providerDb' AND s.source_label = ?5 AND s.cwd = ?6
            AND s.wsl_distro IS NULL)",
        params![
            claim.key.environment_key,
            claim.key.agent,
            claim.key.session_id,
            claim.incarnation.0,
            claim.source_label,
            claim.cwd
        ],
        |row| row.get(0),
    )?)
}

fn publish_gap(tx: &Transaction<'_>, claim: &OpenCodeV2CaptureClaim, reason: &str) -> Result<()> {
    let batch = ToolResultBatch {
        source_format: SourceFormat::OpenCodeSqliteV2,
        results: Vec::new(),
        complete: false,
    };
    publish_coverage_in(
        tx,
        &claim.key,
        current_source_generation(tx, &claim.key)?,
        claim.claim_fence,
        &batch,
        false,
        Some(reason),
    )
}

fn current_source_generation(tx: &Transaction<'_>, key: &SessionKey) -> Result<i64> {
    Ok(tx.query_row(
        "SELECT source_generation FROM session
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![key.environment_key, key.agent, key.session_id],
        |row| row.get(0),
    )?)
}

fn valid_gap_reason(reason: &str) -> bool {
    !reason.is_empty()
        && reason.len() <= MAX_GAP_REASON_BYTES
        && reason
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn save_capture_status(
    tx: &Transaction<'_>,
    claim: &OpenCodeV2CaptureClaim,
    status: OpenCodeV2SourceStatus,
    now: i64,
) -> Result<()> {
    tx.execute(
        "INSERT INTO opencode_v2_source_status
          (environment_key, agent, session_id, incarnation, source_generation,
           source_label, cwd, code, observed_at_epoch)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
           incarnation = excluded.incarnation, source_generation = excluded.source_generation,
           source_label = excluded.source_label, cwd = excluded.cwd,
           code = excluded.code, observed_at_epoch = excluded.observed_at_epoch",
        params![
            claim.key.environment_key,
            claim.key.agent,
            claim.key.session_id,
            claim.incarnation.0,
            current_source_generation(tx, &claim.key)?,
            claim.source_label,
            claim.cwd,
            status.code(),
            now
        ],
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "opencode_v2_tests.rs"]
mod tests;
