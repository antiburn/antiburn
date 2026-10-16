use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use antiburn_checks::check_store::*;
use antiburn_checks::compiler::{CompiledCheck, StatusResult};
use antiburn_checks::definition_lifecycle::{
    EffectiveActivation, InactiveKind, LifecycleState, LifecycleTransition,
};
use antiburn_checks::discovery::DefinitionScope;
use antiburn_checks::domain::{
    ActivationId, ActivationScope, FrozenCandidate, Generation, ObservationPosition, RunScope,
};
use antiburn_checks::engine::EvaluationDiagnostic;
use antiburn_checks::fit::{PreparationFailure, PreparationRecord};
use antiburn_checks::source::{CommittedSourceObservation, PublicationCursor, SourceBatch};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use super::BurnCheckCandidateClaim;
use crate::store::Store;

#[path = "runtime_attempts.rs"]
mod attempts;
#[path = "runtime_page.rs"]
mod page;
#[path = "runtime_recovery.rs"]
mod recovery;
#[path = "runtime_startup.rs"]
mod startup;
#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;

/// SQLite persistence for one environment, session CWD, and live or replay run.
pub struct StoreCheckStore {
    store: Arc<Store>,
    owner: String,
    scope: ActivationScope,
    activation_scope_key: String,
    activations: Mutex<BTreeMap<ActivationId, EffectiveActivation>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct LifecycleRecord {
    version: u32,
    generation: u64,
    revision: [u8; 32],
    state: String,
    definition_scope: String,
    source_paths: Vec<PathBuf>,
    project_root: Option<PathBuf>,
    bootstrap_observation_position: u64,
    bootstrap_publication: u64,
}

impl StoreCheckStore {
    /// Recovers this scope under the application's exclusive profile owner.
    pub fn new(
        store: Arc<Store>,
        owner: String,
        scope: ActivationScope,
    ) -> Result<Self, StoreError> {
        if owner.is_empty()
            || scope.environment.is_empty()
            || scope.session_cwd.is_empty()
            || matches!(&scope.run, RunScope::Replay(id) if id.is_empty())
        {
            return Err(StoreError {
                message:
                    "check store requires owner, environment, session CWD, and nonempty replay identity"
                        .into(),
            });
        }
        let adapter = Self {
            store,
            owner,
            activation_scope_key: Self::activation_scope_key(&scope),
            scope,
            activations: Mutex::new(BTreeMap::new()),
        };
        adapter.recover_abandoned().map_err(store_error)?;
        Ok(adapter)
    }

    /// Returns the exact key shared by activation, history, and report queries.
    pub fn activation_scope_key(scope: &ActivationScope) -> String {
        serde_json::to_string(scope).expect("activation scope serializes")
    }

    /// Lists persisted IDs so discovery can also reconcile removed definitions.
    pub fn definition_ids(&self) -> Result<Vec<String>, StoreError> {
        let connection = self.store.lock();
        let mut statement = connection.prepare("SELECT check_id FROM burn_check_activation WHERE activation_scope_key = ?1 ORDER BY check_id")
            .map_err(|error| store_error(error.into()))?;
        statement
            .query_map([&self.activation_scope_key], |row| row.get(0))
            .map_err(|error| store_error(error.into()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| store_error(error.into()))
    }

    /// Loads pause transitions without reading or compiling definition files.
    pub fn active_pause_transitions(
        &self,
    ) -> Result<Vec<(String, LifecycleTransition)>, StoreError> {
        let connection = self.store.lock();
        let mut statement = connection
            .prepare(
                "SELECT check_id, generation, lifecycle_json FROM burn_check_activation
             WHERE activation_scope_key = ?1 AND enabled = 1 ORDER BY check_id",
            )
            .map_err(|error| store_error(error.into()))?;
        let records = statement
            .query_map([&self.activation_scope_key], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|error| store_error(error.into()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| store_error(error.into()))?;
        records
            .into_iter()
            .map(|(id, generation, saved)| {
                let record: LifecycleRecord = serde_json::from_str(&saved)?;
                ensure!(
                    record.version == 1
                        && record.state == "active"
                        && record.generation == generation,
                    "active lifecycle does not match durable activation"
                );
                Ok((
                    id,
                    LifecycleTransition::Deactivated {
                        revision: record.revision,
                        project_root: record.project_root,
                        scope: definition_scope(&record.definition_scope)?,
                        source_paths: record.source_paths,
                        inactive_kind: InactiveKind::Paused,
                        next_generation: Generation(
                            generation
                                .checked_add(1)
                                .context("activation generation is exhausted")?,
                        ),
                    },
                ))
            })
            .collect::<Result<Vec<_>>>()
            .map_err(store_error)
    }

    /// Loads prior lifecycle metadata. Compilation stays at the host boundary.
    pub fn load_lifecycle(
        &self,
        definition_id: &str,
        compiled: impl Into<Option<Arc<CompiledCheck>>>,
    ) -> Result<LifecycleState, StoreError> {
        self.read_lifecycle(definition_id, compiled.into())
            .map_err(store_error)
    }

    fn read_lifecycle(
        &self,
        definition_id: &str,
        compiled: Option<Arc<CompiledCheck>>,
    ) -> Result<LifecycleState> {
        let saved: Option<String> = self.store.lock().query_row(
            "SELECT lifecycle_json FROM burn_check_activation WHERE check_id = ?1 AND activation_scope_key = ?2",
            params![definition_id, self.activation_scope_key], |row| row.get(0),
        ).optional()?.flatten();
        let Some(saved) = saved else {
            return Ok(LifecycleState::Removed {
                generation: Generation(0),
            });
        };
        let record: LifecycleRecord = serde_json::from_str(&saved)?;
        ensure!(record.version == 1, "unsupported lifecycle version");
        let scope = definition_scope(&record.definition_scope)?;
        let generation = Generation(record.generation);
        if record.state == "active" && compiled.is_none() {
            return Ok(LifecycleState::Uncompiled { generation });
        }
        Ok(match record.state.as_str() {
            "active" => LifecycleState::Active(EffectiveActivation {
                id: ActivationId {
                    scope: self.scope.clone(),
                    definition_id: definition_id.into(),
                    generation,
                },
                revision: record.revision,
                definition_scope: scope,
                source_paths: record.source_paths,
                project_root: record.project_root,
                compiled: compiled.context("active lifecycle requires a compiled definition")?,
                bootstrap_observation_position: ObservationPosition(
                    record.bootstrap_observation_position,
                ),
                bootstrap_publication: PublicationCursor(record.bootstrap_publication),
            }),
            "invalid" | "paused" => LifecycleState::Inactive {
                revision: record.revision,
                project_root: record.project_root,
                generation,
                scope,
                source_paths: record.source_paths,
                inactive_kind: if record.state == "invalid" {
                    InactiveKind::Invalid
                } else {
                    InactiveKind::Paused
                },
            },
            "removed" => LifecycleState::Removed { generation },
            _ => anyhow::bail!("invalid lifecycle state"),
        })
    }

    /// Validates the durable identity before exposing compiled work to recovery.
    pub fn register_activation(&self, activation: &EffectiveActivation) -> Result<(), StoreError> {
        self.validate_activation(activation).map_err(store_error)?;
        let saved: Option<(i64, String, bool)> = self.store.lock().query_row(
            "SELECT generation, definition_revision, enabled FROM burn_check_activation WHERE check_id = ?1 AND activation_scope_key = ?2",
            params![activation.id.definition_id, self.activation_scope_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional().map_err(|error| store_error(error.into()))?;
        if saved
            != Some((
                integer(activation.id.generation.0).map_err(store_error)?,
                hex(&activation.revision),
                true,
            ))
        {
            return Err(StoreError {
                message: "activation does not match durable generation and revision".into(),
            });
        }
        self.activations
            .lock()
            .expect("activation mutex is not poisoned")
            .insert(activation.id.clone(), activation.clone());
        Ok(())
    }

    /// Reads a publication boundary without converting it to observation order.
    pub fn publication_head(&self) -> Result<PublicationCursor, StoreError> {
        self.store
            .lock()
            .query_row(
                "SELECT COALESCE(MAX(publication_sequence), 0) FROM native_tool_result_publication",
                [],
                |row| row.get::<_, u64>(0),
            )
            .map(PublicationCursor)
            .map_err(|error| store_error(error.into()))
    }

    fn validate_activation(&self, activation: &EffectiveActivation) -> Result<()> {
        ensure!(
            activation.id.scope == self.scope,
            "activation belongs to another scope"
        );
        ensure!(
            activation.id.generation.0 > 0,
            "activation generation must be positive"
        );
        Ok(())
    }

    fn persist_lifecycle(
        &self,
        definition_id: &str,
        transition: &LifecycleTransition,
        activation: Option<&EffectiveActivation>,
    ) -> Result<()> {
        if !matches!(
            transition,
            LifecycleTransition::Activated(_) | LifecycleTransition::Unchanged(_)
        ) {
            ensure!(
                activation.is_none(),
                "inactive transition cannot register an activation"
            );
        }
        let (record, changed) = match transition {
            LifecycleTransition::Activated(active) | LifecycleTransition::Unchanged(active) => {
                self.validate_activation(active)?;
                ensure!(
                    active.id.definition_id == definition_id && activation == Some(active),
                    "activation transition does not match its definition"
                );
                (
                    LifecycleRecord {
                        version: 1,
                        generation: active.id.generation.0,
                        revision: active.revision,
                        state: "active".into(),
                        definition_scope: scope_name(active.definition_scope).into(),
                        source_paths: active.source_paths.clone(),
                        project_root: active.project_root.clone(),
                        bootstrap_observation_position: active.bootstrap_observation_position.0,
                        bootstrap_publication: active.bootstrap_publication.0,
                    },
                    matches!(transition, LifecycleTransition::Activated(_)),
                )
            }
            LifecycleTransition::Deactivated {
                revision,
                project_root,
                scope,
                source_paths,
                inactive_kind,
                next_generation,
            } => (
                LifecycleRecord {
                    version: 1,
                    generation: next_generation.0,
                    revision: *revision,
                    state: match inactive_kind {
                        InactiveKind::Invalid => "invalid",
                        InactiveKind::Paused => "paused",
                    }
                    .into(),
                    definition_scope: scope_name(*scope).into(),
                    source_paths: source_paths.clone(),
                    project_root: project_root.clone(),
                    bootstrap_observation_position: 0,
                    bootstrap_publication: 0,
                },
                true,
            ),
            LifecycleTransition::Removed { next_generation } => (
                LifecycleRecord {
                    version: 1,
                    generation: next_generation.0,
                    revision: [0; 32],
                    state: "removed".into(),
                    definition_scope: "global".into(),
                    source_paths: vec![],
                    project_root: None,
                    bootstrap_observation_position: 0,
                    bootstrap_publication: 0,
                },
                true,
            ),
            LifecycleTransition::UnchangedInactive => return Ok(()),
        };
        let mut connection = self.store.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<(u64, Option<String>)> = tx.query_row(
            "SELECT generation, lifecycle_json FROM burn_check_activation WHERE check_id = ?1 AND activation_scope_key = ?2",
            params![definition_id, self.activation_scope_key], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        let encoded = serde_json::to_string(&record)?;
        if record.state == "removed" {
            if previous.is_none() && record.generation == 0 {
                return Ok(());
            }
            if let Some((generation, Some(saved))) = &previous {
                let previous_record: LifecycleRecord = serde_json::from_str(saved)?;
                if *generation == record.generation && previous_record.state == "removed" {
                    return Ok(());
                }
            }
        }
        if previous
            .as_ref()
            .is_some_and(|(_, saved)| saved.as_deref() == Some(&encoded))
        {
            return Ok(());
        }
        ensure!(
            changed
                && record.generation
                    == previous
                        .as_ref()
                        .map_or(1, |(generation, _)| generation.saturating_add(1)),
            "lifecycle generation compare-and-swap failed"
        );
        let checkpoint = SourceProgress {
            publication_cursor: PublicationCursor(record.bootstrap_publication),
            observation_position: ObservationPosition(record.bootstrap_observation_position),
            ..SourceProgress::default()
        }
        .encode()
        .map_err(|error| anyhow::anyhow!(error.message))?;
        tx.execute("UPDATE burn_check_candidate SET state = 'cancelled' WHERE check_id = ?1 AND activation_scope_key = ?2 AND state IN ('pending', 'claimed', 'dispatching', 'answered')", params![definition_id, self.activation_scope_key])?;
        tx.execute(
            "DELETE FROM burn_check_current_status WHERE check_id = ?1 AND activation_scope_key = ?2",
            params![definition_id, self.activation_scope_key],
        )?;
        tx.execute(
            "DELETE FROM burn_check_observation WHERE check_id = ?1 AND activation_scope_key = ?2",
            params![definition_id, self.activation_scope_key],
        )?;
        tx.execute(
            "DELETE FROM burn_check_session_attachment WHERE check_id = ?1 AND activation_scope_key = ?2",
            params![definition_id, self.activation_scope_key],
        )?;
        tx.execute("INSERT INTO burn_check_activation (check_id, activation_scope_key, definition_revision, enabled, generation, bootstrap_sequence, applied_sequence, updated_at_epoch, checkpoint_json, lifecycle_json)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, ?9)
            ON CONFLICT(check_id, activation_scope_key) DO UPDATE SET definition_revision = excluded.definition_revision,
            enabled = excluded.enabled, generation = excluded.generation, bootstrap_sequence = excluded.bootstrap_sequence,
            applied_sequence = excluded.applied_sequence, updated_at_epoch = excluded.updated_at_epoch,
            checkpoint_json = excluded.checkpoint_json, lifecycle_json = excluded.lifecycle_json",
            params![definition_id, self.activation_scope_key, hex(&record.revision), record.state == "active", integer(record.generation)?, integer(record.bootstrap_publication)?, now_epoch(), checkpoint, encoded])?;
        tx.commit()?;
        Ok(())
    }

    fn claim_record(&self, claimed: &ClaimedCandidate) -> Result<BurnCheckCandidateClaim> {
        ensure!(
            claimed.candidate.activation.scope == self.scope
                && claimed.id == CandidateId::from(&claimed.candidate),
            "claim belongs to another candidate or scope"
        );
        Ok(BurnCheckCandidateClaim {
            candidate_id: hex(&claimed.id.0),
            owner: self.owner.clone(),
            fence: integer(claimed.fence)?,
        })
    }
}

impl CheckStore for StoreCheckStore {
    fn activate(
        &self,
        definition_id: &str,
        transition: &LifecycleTransition,
        activation: Option<&EffectiveActivation>,
    ) -> Result<(), StoreError> {
        self.persist_lifecycle(definition_id, transition, activation)
            .map_err(store_error)?;
        self.activations
            .lock()
            .expect("activation mutex is not poisoned")
            .retain(|id, _| id.definition_id != definition_id);
        if let Some(activation) = activation {
            self.register_activation(activation)?;
        }
        Ok(())
    }
    fn load_source_progress(
        &self,
        activation: &EffectiveActivation,
    ) -> Result<SourceProgress, StoreError> {
        self.progress(activation).map_err(store_error)
    }
    fn apply_source_page(
        &self,
        activation: &EffectiveActivation,
        expected: SourceProgressHead,
        page: &SourceBatch,
        observations: &[CommittedSourceObservation],
        next: &SourceProgress,
        candidates: &[FrozenCandidate],
    ) -> Result<(), StoreError> {
        self.persist_page(activation, expected, page, observations, next, candidates)
            .map_err(store_error)
    }
    fn record_overflow(&self, candidate: &FrozenCandidate) -> Result<(), StoreError> {
        if candidate.activation.scope != self.scope {
            return Err(StoreError {
                message: "overflow candidate belongs to another scope".into(),
            });
        }
        self.store
            .record_burn_check_overflow(
                &hex(&candidate.fingerprint),
                "pending candidate limit reached",
                now_epoch(),
            )
            .map(|_| ())
            .map_err(store_error)
    }
    fn load_recovery(
        &self,
        after: Option<&CandidateId>,
        limit: usize,
    ) -> Result<Vec<RecoveryRecord>, StoreError> {
        self.recovery_page(after, limit).map_err(store_error)
    }
    fn claim_due(
        &self,
        candidate: &FrozenCandidate,
        now_unix_ms: u64,
        bypass_check_cooldown: bool,
    ) -> Result<CandidateClaim, StoreError> {
        self.claim(candidate, now_unix_ms, bypass_check_cooldown)
            .map_err(store_error)
    }
    fn record_dispatch_intent(
        &self,
        claimed: &ClaimedCandidate,
        attempted_at_unix_ms: u64,
        cooldown: Duration,
        attempt_number: u8,
    ) -> Result<ClaimOutcome, StoreError> {
        self.dispatch(claimed, attempted_at_unix_ms, cooldown, attempt_number)
            .map_err(store_error)
    }
    fn release_pre_dispatch_claim(
        &self,
        claimed: &ClaimedCandidate,
    ) -> Result<ClaimOutcome, StoreError> {
        self.release_unattempted_claim(claimed).map_err(store_error)
    }
    fn load_preparation(
        &self,
        claimed: &ClaimedCandidate,
    ) -> Result<Option<PreparationRecord>, StoreError> {
        self.saved_preparation(claimed).map_err(store_error)
    }
    fn record_preparation(
        &self,
        claimed: &ClaimedCandidate,
        preparation: &PreparationRecord,
    ) -> Result<ClaimOutcome, StoreError> {
        self.save_preparation(claimed, preparation)
            .map_err(store_error)
    }
    fn record_preparation_rejection(
        &self,
        claimed: &ClaimedCandidate,
        failure: &PreparationFailure,
    ) -> Result<ClaimOutcome, StoreError> {
        self.terminal_with_diagnostic(
            claimed,
            &failure.detail,
            true,
            Some(&PreparationRecord::Rejected(failure.clone())),
            None,
        )
        .map_err(store_error)
    }
    fn record_evaluation_failure(
        &self,
        claimed: &ClaimedCandidate,
        diagnostic: &EvaluationDiagnostic,
    ) -> Result<ClaimOutcome, StoreError> {
        self.terminal_with_diagnostic(claimed, &diagnostic.detail, false, None, Some(diagnostic))
            .map_err(store_error)
    }
    fn record_attempt_failure(
        &self,
        claimed: &ClaimedCandidate,
        attempt_number: u8,
        outcome: AttemptFailure,
        diagnostic: &EvaluationDiagnostic,
    ) -> Result<ClaimOutcome, StoreError> {
        self.settle_failure(claimed, attempt_number, outcome, diagnostic)
            .map_err(store_error)
    }
    fn record_answer(
        &self,
        claimed: &ClaimedCandidate,
        evaluated: &EvaluatedCandidate,
    ) -> Result<(), StoreError> {
        self.save_answer(claimed, evaluated).map_err(store_error)
    }
    fn accept(&self, assessment: &AssessmentAcceptance) -> Result<AcceptanceOutcome, StoreError> {
        let claim = self
            .claim_record(&assessment.claimed)
            .map_err(store_error)?;
        let answer = serde_json::to_value(&assessment.evaluated)
            .map_err(|error| store_error(error.into()))?;
        let actions =
            serde_json::to_value(&assessment.actions).map_err(|error| store_error(error.into()))?;
        self.store
            .try_accept_burn_check_answer(
                &claim,
                &format!("assessment:{}", claim.candidate_id),
                &answer,
                &actions,
                assessment.status_write().map(status_name),
                now_epoch(),
            )
            .map_err(store_error)
    }
    fn record_failure(
        &self,
        claimed: &ClaimedCandidate,
        detail: &str,
    ) -> Result<ClaimOutcome, StoreError> {
        self.terminal(claimed, detail, false).map_err(store_error)
    }
    fn record_not_assessed(
        &self,
        claimed: &ClaimedCandidate,
        detail: &str,
    ) -> Result<ClaimOutcome, StoreError> {
        self.terminal(claimed, detail, true).map_err(store_error)
    }
}

fn definition_scope(value: &str) -> Result<DefinitionScope> {
    match value {
        "global" => Ok(DefinitionScope::Global),
        "project" => Ok(DefinitionScope::Project),
        _ => anyhow::bail!("invalid definition scope"),
    }
}
fn scope_name(scope: DefinitionScope) -> &'static str {
    match scope {
        DefinitionScope::Global => "global",
        DefinitionScope::Project => "project",
    }
}
fn status_name(status: StatusResult) -> &'static str {
    match status {
        StatusResult::Success => "success",
        StatusResult::Failure => "failure",
    }
}
fn parse_status(status: Option<String>) -> Result<Option<StatusResult>> {
    status
        .map(|status| match status.as_str() {
            "success" => Ok(StatusResult::Success),
            "failure" => Ok(StatusResult::Failure),
            _ => anyhow::bail!("invalid saved status"),
        })
        .transpose()
}
fn integer(value: u64) -> Result<i64> {
    i64::try_from(value).context("runtime value exceeds SQLite integer range")
}
fn hex(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn now_epoch() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after Unix epoch")
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}
fn store_error(error: anyhow::Error) -> StoreError {
    StoreError {
        message: format!("Burn Check store: {error}"),
    }
}
