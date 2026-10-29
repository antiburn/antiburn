//! Bounded candidate scheduling and single-flight assessment coordination.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::Value;

use crate::actions::{ActionPlanningError, plan_status_actions_with_budget};
use crate::check_store::{
    AcceptanceOutcome, AssessmentAcceptance, AttemptFailure, CandidateClaim, CandidateId,
    CheckStore, ClaimOutcome, ClaimedCandidate, DecisionProgress, DecisionProgressState,
    EvaluatedAssessment, RecoveryWork, SourceProgress, StoreError, validate_decision_progress,
};
use crate::decision::DecisionAttempt;
pub use crate::decision::DecisionInput;
use crate::definition_lifecycle::EffectiveActivation;
use crate::domain::{
    ActivationId, BranchId, EvidenceVersion, FrozenCandidate, Observation, ObservationPosition,
};
use crate::expression_eval::EvaluationBudget;
use crate::fit::{
    PreparationFailure, PreparationFailureCode, PreparationFailureContext, PreparationRecord,
};
use crate::preparation::{PreparationOutcome, prepare_decision_input_with_budget};
use crate::prepared_assessment::PreparedAssessment;
use crate::results::DecisionResult;
use crate::selection::SelectionOutcome;
use crate::source::{CommittedSourceObservation, PublicationCursor, SourceBatch, SourceEvent};

#[path = "engine_local.rs"]
mod local;
#[path = "engine/model.rs"]
mod model;
#[path = "engine_source_refresh.rs"]
mod source_refresh;
#[cfg(test)]
#[path = "engine/test_support.rs"]
pub(crate) mod test_support;
use model::{bounded_failure, incompatible_preparation, prepare_all_inputs};

/// Maximum number of pending candidates retained in memory.
pub const MAX_PENDING_CANDIDATES: usize = 128;
/// Maximum number of evaluations active across all checks in this coordinator.
pub const MAX_IN_FLIGHT_EVALUATIONS: usize = 4;

/// Scheduling differences that the shared engine applies to live and replay runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExecutionPolicy {
    /// Honor authored cooldown and coalesce only rolling-window candidates.
    #[default]
    Live,
    /// Drain one consumed event at a time without check cooldown.
    SequentialReplay,
}

/// A boxed evaluator future. The future owns all admission waits and provider I/O.
pub type DecisionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DecisionResult, EvaluationFailure>> + Send + 'a>>;

/// One bounded evaluation task returned to the coordinator for concurrent polling.
pub type EvaluationTask<'a> =
    Pin<Box<dyn Future<Output = Result<RunSettlement, EvaluationTaskError>> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluationTaskError {
    pub source: StoreError,
    pub attempts: u64,
    pub new_intents: u64,
    pub intent_commit_uncertain: bool,
}

impl std::fmt::Display for EvaluationTaskError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(formatter)
    }
}
impl std::error::Error for EvaluationTaskError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationValidationPurpose {
    Dispatch,
    Publication,
}

/// A model-backed evaluator for one already-frozen candidate.
pub trait DecisionEvaluator: Send + Sync {
    /// Checks local runtime gates without provider preparation or discovery.
    fn local_execution_is_current(&self, _activation: &EffectiveActivation) -> bool {
        true
    }

    /// Validates and freezes provider input before durable dispatch intent.
    fn prepare(
        &self,
        activation: &EffectiveActivation,
        inputs: Vec<DecisionInput>,
        evaluation_budget: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure>;

    /// Observes a new preparation after its current claim is saved.
    /// Saved retries and recovery do not call this hook.
    fn preparation_recorded(&self, _preparation: &PreparationRecord) {}

    /// Fences incompatible saved provider profiles before dispatch or recovery.
    fn validate_prepared(
        &self,
        activation: &EffectiveActivation,
        prepared: &PreparedAssessment,
        purpose: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure>;

    /// Evaluates the exact persisted preparation. Simple adapters use its input.
    fn evaluate_prepared<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        candidate: &'a FrozenCandidate,
        prepared: &'a PreparedAssessment,
        attempt: &'a DecisionAttempt,
    ) -> DecisionFuture<'a>;
}

/// A provider or response-validation failure that cannot produce a status write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluationFailure {
    /// Typed diagnostic classification, independent of retry policy.
    pub code: EvaluationFailureCode,
    /// Retry and unknown-delivery policy supplied by the transport boundary.
    pub kind: AttemptFailure,
    /// Safe diagnostic detail.
    pub detail: String,
    /// Provider attempts already sent for this result.
    pub attempts: u8,
}

/// Safe, closed provider and engine failure categories for local reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationFailureCode {
    AuthenticationRejected,
    RateLimited,
    ProviderOverloaded,
    ProviderUnavailable,
    DeliveryUnknown,
    InvalidResponse,
    ContextRejected,
    InvalidRequest,
    Cancelled,
    StorageFailure,
    InvalidDecision,
    ActionFailed,
}

/// Persisted diagnostic detail contains no provider body or evidence payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationDiagnostic {
    pub code: EvaluationFailureCode,
    pub detail: String,
}

/// Settled engine outcome with the actual provider attempt count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunSettlement {
    /// Semantic engine outcome.
    pub outcome: RunOutcome,
    /// Provider attempts consumed by the evaluator.
    pub attempts: u64,
    /// Committed intents created by this task, excluding saved attempts.
    pub new_intents: u64,
}

/// Explicit result of adding a frozen candidate to bounded pending state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// The candidate became the latest pending candidate for this branch.
    Queued,
    /// A previous pending candidate was replaced by this later trigger.
    Coalesced,
    /// The global pending bound is full and the candidate was not retained.
    Overflow,
}

/// Result of attempting to run one due candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    /// A candidate was evaluated and accepted.
    Accepted,
    /// The answer is durable and waits for source normalization; do not resend.
    Deferred,
    /// Publication rights were revoked; the answer remains historical.
    Stale,
    /// A known false condition skipped provider evaluation.
    Skipped,
    /// Required input was unavailable and no provider call was made.
    NotAssessed(String),
    /// The candidate failed without writing a status.
    Failed(String),
    /// The finite replay allowance ended before another intent.
    LimitReached,
}

/// Result of reserving a due candidate for asynchronous evaluation.
#[must_use = "poll each started evaluation through completion; dropping it leaves dispatch unresolved"]
pub enum StartOutcome<'a> {
    /// No candidate can run yet or the durable cooldown has not expired.
    NotDue,
    /// The Store rejected a candidate whose generation or evidence is stale.
    Stale,
    /// The evaluator future is ready for bounded concurrent polling.
    Started(EvaluationTask<'a>),
}

/// Outcome for applying a source batch to the Store and runtime scheduler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchOutcome {
    /// Number of observations accepted by the selector.
    pub observations: usize,
    /// Number of complete candidates durably created.
    pub candidates: usize,
    /// Number of candidates replaced by a newer pending candidate.
    pub coalesced: usize,
    /// Number of candidates rejected by the runtime queue bound.
    pub overflowed: usize,
    /// Conditions that filtered observations before window counting or queue mutation.
    pub condition_outcomes: Vec<(ObservationPosition, crate::preparation::ConditionOutcome)>,
    /// Model, mode, and path rejections for committed observations.
    pub filter_outcomes: Vec<(ObservationPosition, crate::filters::FilterDiagnostic)>,
}

/// A bounded, thread-safe scheduler. Persistence remains the source of truth.
pub struct CheckEngine<S, E> {
    store: Arc<S>,
    evaluator: Arc<E>,
    state: Arc<Mutex<SchedulerState>>,
    selectors: Mutex<BTreeMap<ActivationId, SourceProgress>>,
    policy: ExecutionPolicy,
}

struct InFlightGuard {
    state: Arc<Mutex<SchedulerState>>,
    key: ScheduleKey,
    finished: bool,
    release_branch_on_drop: bool,
    safe_release: Option<Box<dyn FnOnce() + Send>>,
}

impl InFlightGuard {
    fn finish(&mut self) {
        if self.finished {
            return;
        }
        let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
        state.in_flight.remove(&self.key);
        state.active_count = state.active_count.saturating_sub(1);
        self.finished = true;
        self.safe_release = None;
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if self.release_branch_on_drop {
            if !self.finished
                && let Some(release) = self.safe_release.take()
            {
                release();
            }
            self.finish();
            return;
        }
        if !self.finished {
            let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
            // Keep this branch reserved after cancellation. The Store has no settled
            // outcome for this intent, so this engine must not dispatch it again.
            state.active_count = state.active_count.saturating_sub(1);
        }
    }
}

#[derive(Default)]
struct SchedulerState {
    pending: BTreeMap<ScheduleKey, BTreeMap<ObservationPosition, PendingCandidate>>,
    in_flight: BTreeSet<ScheduleKey>,
    cooldown_until_unix_ms: BTreeMap<ScheduleKey, u64>,
    service_tickets: BTreeMap<ScheduleKey, u64>,
    next_service_ticket: u64,
    active_count: usize,
    recent_dispatches: u8,
    pending_releases: BTreeMap<ScheduleKey, ClaimedCandidate>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ScheduleKey {
    activation: ActivationId,
    branch: BranchId,
}

#[derive(Clone)]
struct PendingCandidate {
    candidate: FrozenCandidate,
    compiled: Arc<crate::compiler::CompiledCheck>,
    resumed: bool,
}

enum PreflightOutcome {
    Ready(PreparedAssessment),
    Settled(RunSettlement),
    Stale,
}

enum PageInvalidation {
    Upsert {
        identity: String,
        version: EvidenceVersion,
        source_evidence_version: Option<EvidenceVersion>,
    },
    NativeUpsert {
        identity: String,
        version: EvidenceVersion,
    },
    Removal {
        identity: String,
    },
    Branch,
}

impl<S: CheckStore + 'static, E: DecisionEvaluator> CheckEngine<S, E> {
    /// Creates a coordinator over the desktop Store and decision evaluator.
    #[must_use]
    pub fn new(store: Arc<S>, evaluator: Arc<E>) -> Self {
        Self::with_policy(store, evaluator, ExecutionPolicy::Live)
    }

    /// Creates a coordinator with an explicit scheduling policy.
    #[must_use]
    pub fn with_policy(store: Arc<S>, evaluator: Arc<E>, policy: ExecutionPolicy) -> Self {
        Self {
            store,
            evaluator,
            state: Arc::new(Mutex::new(SchedulerState::default())),
            selectors: Mutex::new(BTreeMap::new()),
            policy,
        }
    }

    /// Persists an activation transition and drops in-memory work from older generations.
    pub fn persist_activation(
        &self,
        definition_id: &str,
        transition: &crate::definition_lifecycle::LifecycleTransition,
        activation: Option<&EffectiveActivation>,
    ) -> Result<(), StoreError> {
        self.store.activate(definition_id, transition, activation)?;
        {
            let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
            state.pending.retain(|key, _| {
                key.activation.definition_id != definition_id
                    || activation.is_some_and(|value| key.activation == value.id)
            });
            state.cooldown_until_unix_ms.retain(|key, _| {
                key.activation.definition_id != definition_id
                    || activation.is_some_and(|value| key.activation == value.id)
            });
            state.pending_releases.retain(|key, _| {
                key.activation.definition_id != definition_id
                    || activation.is_some_and(|value| key.activation == value.id)
            });
        }
        let mut selectors = self
            .selectors
            .lock()
            .expect("selector mutex is not poisoned");
        selectors.retain(|id, _| {
            id.definition_id != definition_id || activation.is_some_and(|value| id == &value.id)
        });
        Ok(())
    }

    /// Applies and acknowledges one source page with durable selector progress.
    ///
    /// Observation positions come from the persisted engine head, not publication
    /// cursors. The in-memory selector changes only after the Store commits the
    /// page, its acknowledgment, the new selector checkpoint, and candidates.
    pub fn apply_source_page(
        &self,
        activation: &EffectiveActivation,
        page: &SourceBatch,
    ) -> Result<BatchOutcome, StoreError> {
        self.apply_source_page_with_inventories(activation, page, &mut |_, _| Ok(None))
    }

    /// Freezes demanded current inventories without changing native observations.
    pub fn apply_source_page_with_inventories(
        &self,
        activation: &EffectiveActivation,
        page: &SourceBatch,
        capture: &mut impl FnMut(
            PublicationCursor,
            &Observation,
        )
            -> Result<Option<crate::context::CurrentInventories>, StoreError>,
    ) -> Result<BatchOutcome, StoreError> {
        let mut selectors = self
            .selectors
            .lock()
            .expect("selector mutex is not poisoned");
        if !selectors.contains_key(&activation.id) {
            let progress = self.store.load_source_progress(activation)?;
            selectors.insert(activation.id.clone(), progress);
        }
        let progress = selectors
            .get_mut(&activation.id)
            .expect("source progress was loaded");
        validate_source_page(progress, page)?;

        let expected = progress.head();
        let mut next = progress.clone();
        let mut committed_observations = Vec::new();
        let mut candidates = Vec::new();
        let mut condition_outcomes = Vec::new();
        let mut filter_outcomes = Vec::new();
        let mut invalidations = Vec::<(usize, BranchId, PageInvalidation)>::new();
        let mut reset_branches = BTreeSet::new();
        let mut applied_page = page.clone();
        for (page_index, event) in applied_page.events.iter_mut().enumerate() {
            match event {
                SourceEvent::Ignored { .. } => {}
                SourceEvent::Observation {
                    cursor,
                    source_evidence_version,
                    identity,
                    revision,
                    observation: draft,
                } => {
                    if !next.selection.accepts_update(
                        &draft.branch,
                        identity,
                        *revision,
                        &draft.version,
                    ) {
                        // A native binding can change without a new public projection.
                        if let Some(version) = source_evidence_version
                            && next.selection.is_normalized_duplicate(
                                &draft.branch,
                                identity,
                                *revision,
                                &draft.version,
                            )
                        {
                            invalidations.push((
                                page_index,
                                draft.branch.clone(),
                                PageInvalidation::NativeUpsert {
                                    identity: identity.clone(),
                                    version: version.clone(),
                                },
                            ));
                        }
                        *event = SourceEvent::Ignored { cursor: *cursor };
                        continue;
                    }
                    let position = ObservationPosition(
                        next.observation_position
                            .0
                            .checked_add(1)
                            .ok_or_else(|| StoreError {
                                message: "observation position is exhausted".to_owned(),
                            })?,
                    );
                    next.observation_position = position;
                    let observation = (**draft).clone().commit(position);
                    invalidations.push((
                        page_index,
                        observation.branch.clone(),
                        PageInvalidation::Upsert {
                            identity: identity.clone(),
                            version: observation.version.clone(),
                            source_evidence_version: source_evidence_version.clone(),
                        },
                    ));
                    let selection = next.selection.apply_published_with_inventories(
                        activation,
                        identity,
                        *revision,
                        &observation,
                        &mut || capture(*cursor, &observation),
                    )?;
                    let window_invalidated = selection == SelectionOutcome::WindowInvalidated;
                    match selection {
                        SelectionOutcome::Candidate(candidate) => {
                            candidates.push((*candidate, page_index, identity.clone()))
                        }
                        SelectionOutcome::ConditionFiltered(condition) => {
                            condition_outcomes.push((position, condition))
                        }
                        SelectionOutcome::FilterRejected(diagnostic) => {
                            filter_outcomes.push((position, diagnostic))
                        }
                        SelectionOutcome::WindowInvalidated => {
                            invalidations.push((
                                page_index,
                                observation.branch.clone(),
                                PageInvalidation::Branch,
                            ));
                        }
                        _ => {}
                    }
                    committed_observations.push(CommittedSourceObservation {
                        cursor: *cursor,
                        identity: identity.clone(),
                        revision: *revision,
                        observation,
                        window_invalidated,
                    });
                }
                SourceEvent::Removal {
                    cursor,
                    identity,
                    revision,
                    branch,
                } => {
                    if !next.selection.remove_result(branch, identity, *revision) {
                        *event = SourceEvent::Ignored { cursor: *cursor };
                        continue;
                    }
                    invalidations.push((
                        page_index,
                        branch.clone(),
                        PageInvalidation::Removal {
                            identity: identity.clone(),
                        },
                    ));
                }
                SourceEvent::Gap { branch, .. } => {
                    next.selection.mark_coverage_gap(branch);
                }
                SourceEvent::UnsafeUpdate { branch, .. }
                | SourceEvent::SegmentReset { branch, .. } => {
                    invalidations.push((page_index, branch.clone(), PageInvalidation::Branch));
                    reset_branches.insert(branch.clone());
                    next.selection.reset_segment(branch);
                }
                SourceEvent::CoverageRestored { branch, .. } => {
                    next.selection.restore_coverage(branch);
                }
            }
        }
        candidates.retain(|(candidate, candidate_index, candidate_identity)| {
            !invalidations.iter().any(|(change_index, branch, change)| {
                change_index > candidate_index
                    && branch == &candidate.branch
                    && match change {
                        PageInvalidation::Upsert { identity, version, source_evidence_version } => candidate.dependencies.iter().any(|dependency| {
                            match dependency {
                                crate::domain::EvidenceDependency::Message { id, version: saved_version } if id == identity => saved_version != version,
                                crate::domain::EvidenceDependency::SourceEvidence { id, version: saved_version } if id == identity => {
                                    if id == candidate_identity { saved_version != version } else { source_evidence_version.as_ref().is_some_and(|native_version| saved_version != native_version) }
                                },
                                _ => false,
                            }
                        }),
                        PageInvalidation::NativeUpsert { identity, version } => candidate.dependencies.iter().any(|dependency| {
                            matches!(dependency, crate::domain::EvidenceDependency::SourceEvidence { id, version: saved_version } if id == identity && id != candidate_identity && saved_version != version)
                        }),
                        PageInvalidation::Removal { identity } => candidate.dependencies.iter().any(|dependency| {
                            matches!(dependency, crate::domain::EvidenceDependency::Message { id, .. } | crate::domain::EvidenceDependency::SourceEvidence { id, .. } if id == identity)
                        }),
                        PageInvalidation::Branch => true,
                    }
            })
        });
        let candidates = candidates
            .into_iter()
            .map(|(candidate, _, _)| candidate)
            .collect::<Vec<_>>();
        next.publication_cursor = page.next;
        if let Err(error) = self.store.apply_source_page(
            activation,
            expected,
            &applied_page,
            &committed_observations,
            &next,
            &candidates,
        ) {
            selectors.remove(&activation.id);
            return Err(error);
        }
        *progress = next;
        drop(selectors);

        if !reset_branches.is_empty() {
            let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
            state.pending.retain(|key, _| {
                key.activation != activation.id || !reset_branches.contains(&key.branch)
            });
        }

        let mut outcome = BatchOutcome {
            observations: committed_observations.len(),
            candidates: candidates.len(),
            coalesced: 0,
            overflowed: 0,
            condition_outcomes,
            filter_outcomes,
        };
        for candidate in candidates {
            match self.enqueue(activation, candidate.clone())? {
                EnqueueOutcome::Queued => {}
                EnqueueOutcome::Coalesced => outcome.coalesced += 1,
                EnqueueOutcome::Overflow => {
                    self.store.record_overflow(&candidate)?;
                    outcome.overflowed += 1;
                }
            }
        }
        Ok(outcome)
    }

    /// Removes proven terminal queue entries and reloads unfinished work in bounded pages.
    pub fn recover_pending(&self) -> Result<BatchOutcome, StoreError> {
        self.prune_terminal_pending()?;
        let mut outcome = BatchOutcome {
            observations: 0,
            candidates: 0,
            coalesced: 0,
            overflowed: 0,
            condition_outcomes: Vec::new(),
            filter_outcomes: Vec::new(),
        };
        let mut after = None;
        loop {
            let records = self
                .store
                .load_recovery(after.as_ref(), MAX_PENDING_CANDIDATES)?;
            if records.is_empty() {
                break;
            }
            for record in records {
                let activation = record.activation;
                let (candidate, resumed) = match record.work {
                    RecoveryWork::Pending { candidate, resumed } => (candidate, resumed),
                    RecoveryWork::Answered { claimed, evaluated } => {
                        after = Some(claimed.id.clone());
                        if activation.compiled.is_local()
                            && !self.evaluator.local_execution_is_current(&activation)
                        {
                            continue;
                        }
                        if let Err(error) = evaluated.validate_for_declarations(
                            &activation.compiled.decisions,
                            &activation.compiled.question_requirements(),
                        ) {
                            self.store.record_evaluation_failure(
                                &claimed,
                                &EvaluationDiagnostic {
                                    code: EvaluationFailureCode::InvalidDecision,
                                    detail: error.to_string(),
                                },
                            )?;
                            continue;
                        }
                        let preparation = self.store.load_preparation(&claimed)?;
                        if !activation.compiled.is_local() {
                            let progress = self.store.load_decision_progress(&claimed)?;
                            let validated = validate_decision_progress(
                                &activation.compiled.decisions,
                                &activation.compiled.question_requirements(),
                                &progress,
                            );
                            let complete = matches!(&validated, Ok(validated) if matches!(&validated.state, DecisionProgressState::Complete(saved) if saved == &evaluated));
                            let prepared = match preparation.as_ref() {
                                Some(PreparationRecord::Ready(prepared))
                                    if complete
                                        && prepared
                                            .validate_for_declarations(
                                                &activation.compiled.decisions,
                                            )
                                            .is_ok() =>
                                {
                                    prepared
                                }
                                _ => {
                                    self.store.record_preparation_rejection(&claimed, &incompatible_preparation("The saved answer does not match complete durable decision progress and preparation."))?;
                                    continue;
                                }
                            };
                            model::publish_model(
                                self.store.as_ref(),
                                self.evaluator.as_ref(),
                                &activation,
                                &claimed,
                                prepared,
                                evaluated,
                            )?;
                            continue;
                        }
                        let mut budget = match restored_evaluation_budget(
                            &activation.compiled,
                            preparation.as_ref(),
                        ) {
                            Ok(budget) => budget,
                            Err(error) => {
                                self.store.record_evaluation_failure(&claimed, &error)?;
                                continue;
                            }
                        };
                        let actions = match plan_status_actions_with_budget(
                            &activation.compiled.actions,
                            &claimed.candidate.context.value,
                            &evaluated,
                            claimed.prior_status,
                            &mut budget,
                        ) {
                            Ok(actions) => actions,
                            Err(error) => {
                                self.store.record_evaluation_failure(
                                    &claimed,
                                    &EvaluationDiagnostic {
                                        code: EvaluationFailureCode::ActionFailed,
                                        detail: error.message,
                                    },
                                )?;
                                continue;
                            }
                        };
                        if activation.compiled.is_local()
                            && !self.evaluator.local_execution_is_current(&activation)
                        {
                            continue;
                        }
                        self.store.accept(&AssessmentAcceptance {
                            claimed,
                            evaluated,
                            actions,
                        })?;
                        continue;
                    }
                };
                after = Some(CandidateId::from(&candidate));
                outcome.candidates += 1;
                match self.enqueue_with_resume(&activation, candidate.clone(), resumed)? {
                    EnqueueOutcome::Queued => {}
                    EnqueueOutcome::Coalesced => outcome.coalesced += 1,
                    EnqueueOutcome::Overflow => {
                        self.store.record_overflow(&candidate)?;
                        outcome.overflowed += 1;
                    }
                }
            }
        }
        Ok(outcome)
    }

    /// Coalesces windows and retains separate non-window candidates in source order.
    ///
    /// # Errors
    /// Returns a Store error if terminal-state lookup fails before queue admission.
    pub fn enqueue(
        &self,
        activation: &EffectiveActivation,
        candidate: FrozenCandidate,
    ) -> Result<EnqueueOutcome, StoreError> {
        self.enqueue_with_resume(activation, candidate, false)
    }

    fn enqueue_with_resume(
        &self,
        activation: &EffectiveActivation,
        candidate: FrozenCandidate,
        resumed: bool,
    ) -> Result<EnqueueOutcome, StoreError> {
        let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
        if state.pending.values().map(BTreeMap::len).sum::<usize>() >= MAX_PENDING_CANDIDATES {
            drop(state);
            self.prune_terminal_pending()?;
            state = self.state.lock().expect("scheduler mutex is not poisoned");
        }
        let key = ScheduleKey {
            activation: candidate.activation.clone(),
            branch: candidate.branch.clone(),
        };
        let coalesces = !resumed && activation.compiled.trigger.window.is_some();
        if let Some(existing) = state
            .pending
            .get_mut(&key)
            .and_then(|queue| queue.get_mut(&candidate.trigger_position))
        {
            existing.resumed |= resumed;
            return Ok(EnqueueOutcome::Queued);
        }
        let existing = state.pending.get(&key);
        let replaces = coalesces
            && existing.is_some_and(|queue| queue.values().any(|pending| !pending.resumed));
        let needs_ticket = existing.is_none_or(BTreeMap::is_empty);
        if !replaces
            && state.pending.values().map(BTreeMap::len).sum::<usize>() >= MAX_PENDING_CANDIDATES
        {
            return Ok(EnqueueOutcome::Overflow);
        }
        let queue = state.pending.entry(key.clone()).or_default();
        if replaces {
            if queue.last_key_value().is_some_and(|(position, pending)| {
                !pending.resumed && *position > candidate.trigger_position
            }) {
                return Ok(EnqueueOutcome::Coalesced);
            }
            queue.retain(|_, pending| pending.resumed);
        }
        queue.insert(
            candidate.trigger_position,
            PendingCandidate {
                candidate,
                compiled: Arc::clone(&activation.compiled),
                resumed,
            },
        );
        if needs_ticket {
            let ticket = state.next_service_ticket;
            state.next_service_ticket = state.next_service_ticket.saturating_add(1);
            state.service_tickets.insert(key.clone(), ticket);
        }
        if replaces {
            Ok(EnqueueOutcome::Coalesced)
        } else {
            Ok(EnqueueOutcome::Queued)
        }
    }

    fn prune_terminal_pending(&self) -> Result<(), StoreError> {
        let candidate_ids = self
            .state
            .lock()
            .expect("scheduler mutex is not poisoned")
            .pending
            .values()
            .flat_map(BTreeMap::values)
            .map(|pending| CandidateId::from(&pending.candidate))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if candidate_ids.is_empty() {
            return Ok(());
        }
        let mut terminal = BTreeSet::new();
        for batch in candidate_ids.chunks(MAX_PENDING_CANDIDATES) {
            let found = self.store.load_terminal_candidate_ids(batch)?;
            if found.iter().any(|id| !batch.contains(id)) {
                return Err(StoreError {
                    message: "terminal candidate lookup returned an unrequested identity".into(),
                });
            }
            terminal.extend(found);
        }
        let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
        let mut empty_keys = Vec::new();
        state.pending.retain(|key, queue| {
            queue.retain(|_, pending| !terminal.contains(&CandidateId::from(&pending.candidate)));
            if queue.is_empty() {
                empty_keys.push(key.clone());
                false
            } else {
                true
            }
        });
        for key in empty_keys {
            state.service_tickets.remove(&key);
        }
        Ok(())
    }

    /// Reserves the earliest due candidate and returns its evaluation future.
    ///
    /// The caller must poll returned tasks in a bounded set beside source work
    /// and cancellation. Do not await a task inline on the coordinator. The
    /// Store's durable cooldown is reapplied after restart before task creation.
    pub fn start_one_due(
        &self,
        activation: &EffectiveActivation,
        now_unix_ms: u64,
    ) -> Result<StartOutcome<'_>, StoreError> {
        self.start_one_due_with_attempt_limit(activation, now_unix_ms, u64::MAX)
    }

    /// Reserves one candidate with a caller-supplied remaining attempt budget.
    pub fn start_one_due_with_attempt_limit(
        &self,
        activation: &EffectiveActivation,
        now_unix_ms: u64,
        maximum_new_intents: u64,
    ) -> Result<StartOutcome<'_>, StoreError> {
        self.start_one_due_matching(
            activation,
            now_unix_ms,
            maximum_new_intents,
            |_| true,
            Arc::clone(&self.evaluator),
        )
    }

    /// Starts only branches whose startup attachment has committed.
    pub fn start_one_due_for_branches(
        &self,
        activation: &EffectiveActivation,
        now_unix_ms: u64,
        branches: &BTreeSet<BranchId>,
    ) -> Result<StartOutcome<'_>, StoreError> {
        self.start_one_due_matching(
            activation,
            now_unix_ms,
            u64::MAX,
            |branch| branches.contains(branch),
            Arc::clone(&self.evaluator),
        )
    }

    /// Starts ready branches with one evaluator for the complete assessment.
    ///
    /// Preparation and validation run before this method returns. The returned
    /// task owns the evaluator, including every allowed retry, and does not borrow
    /// the engine. The caller can run this method on a blocking preparation worker.
    pub fn start_one_due_for_branches_with_evaluator(
        &self,
        activation: &EffectiveActivation,
        now_unix_ms: u64,
        branches: &BTreeSet<BranchId>,
        evaluator: Arc<E>,
    ) -> Result<StartOutcome<'static>, StoreError>
    where
        S: 'static,
        E: 'static,
    {
        self.start_one_due_matching(
            activation,
            now_unix_ms,
            u64::MAX,
            |branch| branches.contains(branch),
            evaluator,
        )
    }

    fn start_one_due_matching<'task>(
        &self,
        activation: &EffectiveActivation,
        now_unix_ms: u64,
        maximum_new_intents: u64,
        ready: impl Fn(&BranchId) -> bool,
        evaluator: Arc<E>,
    ) -> Result<StartOutcome<'task>, StoreError>
    where
        S: 'task,
        E: 'task,
    {
        let (key, pending) = {
            let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
            if state.active_count >= MAX_IN_FLIGHT_EVALUATIONS {
                return Ok(StartOutcome::NotDue);
            }
            let due = state
                .pending
                .iter()
                .filter(|(key, _)| key.activation == activation.id)
                .filter(|(key, _)| ready(&key.branch))
                .filter(|(key, _)| {
                    !state.in_flight.contains(*key)
                        && (self.policy == ExecutionPolicy::SequentialReplay
                            || state.pending.get(*key).is_some_and(|queue| {
                                queue
                                    .first_key_value()
                                    .is_some_and(|(_, pending)| pending.resumed)
                            })
                            || state
                                .cooldown_until_unix_ms
                                .get(*key)
                                .is_none_or(|until| now_unix_ms >= *until))
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            let ranked = due
                .iter()
                .filter_map(|key| {
                    state.pending.get(key).and_then(|queue| {
                        let (newest, _) = queue.last_key_value()?;
                        Some((key, *newest))
                    })
                })
                .collect::<Vec<_>>();
            let next_key = if self.policy == ExecutionPolicy::SequentialReplay {
                ranked
                    .iter()
                    .min_by_key(|(key, position)| (*position, *key))
                    .map(|(key, _)| *key)
            } else if state.recent_dispatches >= 3 {
                due.iter()
                    .min_by_key(|key| (state.service_tickets.get(*key).copied(), *key))
            } else {
                ranked
                    .iter()
                    .max_by_key(|(key, position)| (*position, *key))
                    .map(|(key, _)| *key)
            };
            let next = next_key.and_then(|key| {
                state
                    .pending
                    .get(key)
                    .and_then(|queue| queue.first_key_value())
                    .map(|(_, pending)| (key.clone(), pending.clone()))
            });
            let Some((key, pending)) = next else {
                return Ok(StartOutcome::NotDue);
            };
            let queue = state.pending.get_mut(&key).expect("pending queue exists");
            queue.remove(&pending.candidate.trigger_position);
            if queue.is_empty() {
                state.pending.remove(&key);
            }
            state.in_flight.insert(key.clone());
            state.active_count += 1;
            (key, pending)
        };

        let guard = InFlightGuard {
            state: Arc::clone(&self.state),
            key: key.clone(),
            finished: false,
            release_branch_on_drop: pending.compiled.is_local(),
            safe_release: None,
        };
        let pending_release = self
            .state
            .lock()
            .expect("scheduler mutex is not poisoned")
            .pending_releases
            .get(&key)
            .cloned();
        if let Some(previous) = pending_release {
            if let Err(error) = self.store.release_pre_dispatch_claim(&previous) {
                self.restore_candidate(key, pending);
                let mut guard = guard;
                guard.finish();
                return Err(error);
            }
            self.state
                .lock()
                .expect("scheduler mutex is not poisoned")
                .pending_releases
                .remove(&key);
        }
        let claimed = match self.store.claim_due(
            &pending.candidate,
            now_unix_ms,
            self.policy == ExecutionPolicy::SequentialReplay,
        ) {
            Ok(CandidateClaim::Claimed(claimed)) => *claimed,
            Ok(CandidateClaim::NotDue { retry_at_unix_ms }) => {
                self.defer_candidate(key, pending, retry_at_unix_ms);
                let mut guard = guard;
                guard.finish();
                return Ok(StartOutcome::NotDue);
            }
            Ok(CandidateClaim::Stale) => {
                self.clear_ticket_if_idle(&key);
                let mut guard = guard;
                guard.finish();
                return Ok(StartOutcome::Stale);
            }
            Err(error) => {
                self.restore_candidate(key, pending);
                let mut guard = guard;
                guard.finish();
                return Err(error);
            }
        };
        if pending.compiled.is_local() {
            return self.start_local(activation, now_unix_ms, pending, claimed, guard, evaluator);
        }
        self.start_model(
            activation,
            now_unix_ms,
            maximum_new_intents,
            pending,
            claimed,
            guard,
            evaluator,
        )
    }

    fn prepare_claimed(
        &self,
        activation: &EffectiveActivation,
        pending: &PendingCandidate,
        claimed: &ClaimedCandidate,
        evaluator: &E,
        attempts: u64,
        purpose: PreparationValidationPurpose,
    ) -> Result<PreflightOutcome, StoreError> {
        let preparation = if let Some(saved) = self.store.load_preparation(claimed)? {
            saved
        } else if attempts > 0 || purpose == PreparationValidationPurpose::Publication {
            PreparationRecord::Rejected(incompatible_preparation(
                "The original preparation is unavailable.",
            ))
        } else {
            let prepared = prepare_all_inputs(&pending.compiled, &pending.candidate).and_then(
                |(inputs, budget)| {
                    let saved_budget = budget.clone();
                    let prepared = evaluator.prepare(activation, inputs, budget)?;
                    if prepared.evaluation_budget != saved_budget {
                        return Err(incompatible_preparation(
                            "Provider preparation changed the shared expression budget.",
                        ));
                    }
                    prepared
                        .validate_for_declarations(&pending.compiled.decisions)
                        .map_err(|error| incompatible_preparation(&error.to_string()))?;
                    Ok(prepared)
                },
            );
            let record = crate::preparation::bounded_preparation_record(match prepared {
                Ok(prepared) => PreparationRecord::Ready(prepared),
                Err(error) => PreparationRecord::Rejected(error),
            });
            if self.store.record_preparation(claimed, &record)? == ClaimOutcome::Stale {
                return Ok(PreflightOutcome::Stale);
            }
            evaluator.preparation_recorded(&record);
            record
        };
        let prepared = match preparation {
            PreparationRecord::Ready(prepared) => prepared
                .validate_for_declarations(&pending.compiled.decisions)
                .map_err(|error| incompatible_preparation(&error.to_string()))
                .and_then(|()| evaluator.validate_prepared(activation, &prepared, purpose))
                .map(|()| prepared),
            PreparationRecord::Rejected(error) => Err(error),
            PreparationRecord::Local(_) => Err(incompatible_preparation(
                "A local preparation cannot evaluate a model decision.",
            )),
        };
        match prepared {
            Ok(prepared) => Ok(PreflightOutcome::Ready(prepared)),
            Err(error) => {
                let error = bounded_failure(error);
                let recorded = self.store.record_preparation_rejection(claimed, &error)?;
                Ok(PreflightOutcome::Settled(RunSettlement {
                    outcome: if recorded == ClaimOutcome::Stale {
                        RunOutcome::Stale
                    } else if error.code == PreparationFailureCode::ConditionFalse {
                        RunOutcome::Skipped
                    } else {
                        RunOutcome::NotAssessed(error.detail)
                    },
                    attempts,
                    new_intents: 0,
                }))
            }
        }
    }

    fn mark_started(&self, key: &ScheduleKey, now_unix_ms: u64, cooldown: std::time::Duration) {
        mark_scheduler_started(&self.state, key, now_unix_ms, cooldown, self.policy);
    }

    fn restore_pre_dispatch(
        &self,
        key: ScheduleKey,
        pending: PendingCandidate,
        claimed: ClaimedCandidate,
        mut guard: InFlightGuard,
        mut error: StoreError,
    ) -> StoreError {
        match self.store.release_pre_dispatch_claim(&claimed) {
            Ok(ClaimOutcome::Current) => self.restore_candidate(key, pending),
            Ok(ClaimOutcome::Stale) => {}
            Err(release_error) => {
                let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
                state.pending_releases.insert(key.clone(), claimed);
                restore_pending(&mut state, key, pending);
                error.message = format!(
                    "{}; pre-dispatch claim release: {}",
                    error.message, release_error.message
                );
            }
        }
        guard.finish();
        error
    }

    fn defer_candidate(&self, key: ScheduleKey, pending: PendingCandidate, retry_at_unix_ms: u64) {
        let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
        state
            .cooldown_until_unix_ms
            .insert(key.clone(), retry_at_unix_ms);
        restore_pending(&mut state, key, pending);
    }

    fn restore_candidate(&self, key: ScheduleKey, pending: PendingCandidate) {
        let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
        restore_pending(&mut state, key, pending);
    }

    fn clear_ticket_if_idle(&self, key: &ScheduleKey) {
        let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
        if state.pending.get(key).is_none_or(BTreeMap::is_empty) {
            state.service_tickets.remove(key);
        }
    }
}

fn mark_scheduler_started(
    state: &Mutex<SchedulerState>,
    key: &ScheduleKey,
    now_unix_ms: u64,
    cooldown: std::time::Duration,
    policy: ExecutionPolicy,
) {
    let mut state = state.lock().expect("scheduler mutex is not poisoned");
    if state
        .pending
        .get(key)
        .is_some_and(|queue| !queue.is_empty())
    {
        let ticket = state.next_service_ticket;
        state.next_service_ticket = state.next_service_ticket.saturating_add(1);
        state.service_tickets.insert(key.clone(), ticket);
    } else {
        state.service_tickets.remove(key);
    }
    if policy == ExecutionPolicy::Live {
        state.recent_dispatches = if state.recent_dispatches >= 3 {
            0
        } else {
            state.recent_dispatches + 1
        };
    }
    let cooldown_ms = u64::try_from(cooldown.as_millis()).unwrap_or(u64::MAX);
    state
        .cooldown_until_unix_ms
        .insert(key.clone(), now_unix_ms.saturating_add(cooldown_ms));
}

fn restored_evaluation_budget(
    compiled: &crate::compiler::CompiledCheck,
    preparation: Option<&PreparationRecord>,
) -> Result<EvaluationBudget, EvaluationDiagnostic> {
    if compiled.is_local() {
        let Some(PreparationRecord::Local(prepared)) = preparation else {
            return Err(EvaluationDiagnostic {
                code: EvaluationFailureCode::ActionFailed,
                detail: "A saved local answer requires its original local preparation.".into(),
            });
        };
        prepared
            .validate_for_resume()
            .map_err(|error| EvaluationDiagnostic {
                code: EvaluationFailureCode::ActionFailed,
                detail: error.detail,
            })?;
        return Ok(prepared.evaluation_budget.clone());
    }
    let Some(PreparationRecord::Ready(prepared)) = preparation else {
        return Err(EvaluationDiagnostic {
            code: EvaluationFailureCode::ActionFailed,
            detail: "A saved model answer requires its original model preparation.".into(),
        });
    };
    prepared
        .validate_for_declarations(&compiled.decisions)
        .map_err(|error| EvaluationDiagnostic {
            code: EvaluationFailureCode::ActionFailed,
            detail: error.to_string(),
        })?;
    Ok(prepared.evaluation_budget.clone())
}

fn restore_pending(state: &mut SchedulerState, key: ScheduleKey, pending: PendingCandidate) {
    let queue = state.pending.entry(key).or_default();
    if !pending.resumed
        && pending.compiled.trigger.window.is_some()
        && queue.values().any(|entry| !entry.resumed)
    {
        return;
    }
    queue
        .entry(pending.candidate.trigger_position)
        .or_insert(pending);
}

fn validate_source_page(progress: &SourceProgress, page: &SourceBatch) -> Result<(), StoreError> {
    let mut previous = progress.publication_cursor;
    for event in &page.events {
        let cursor = match event {
            SourceEvent::Ignored { cursor }
            | SourceEvent::Observation { cursor, .. }
            | SourceEvent::Removal { cursor, .. }
            | SourceEvent::Gap { cursor, .. }
            | SourceEvent::UnsafeUpdate { cursor, .. }
            | SourceEvent::SegmentReset { cursor, .. }
            | SourceEvent::CoverageRestored { cursor, .. } => *cursor,
        };
        if cursor <= previous {
            return Err(StoreError {
                message: "source page does not advance publication progress".to_owned(),
            });
        }
        previous = cursor;
    }
    if page.next < previous || (!page.events.is_empty() && page.next != previous) {
        return Err(StoreError {
            message: "source page cursor does not match its final event".to_owned(),
        });
    }
    Ok(())
}

impl From<ActionPlanningError> for EvaluationFailure {
    fn from(value: ActionPlanningError) -> Self {
        Self {
            code: EvaluationFailureCode::ActionFailed,
            kind: AttemptFailure::Permanent,
            detail: value.message,
            attempts: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    mod assessment_budget;
    mod inventory_freeze;
    mod local;
    mod multi_decision;
    mod preparation_limits;
    mod queue_pruning;
    mod selected_evaluator;
    mod selection_live;

    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    use serde_json::json;

    use super::test_support::{test_answer, test_prepare, test_validate};
    use super::*;
    use crate::check_store::{CandidateClaim, CandidateId, ClaimedCandidate};
    use crate::compiler::compile;
    use crate::definition_lifecycle::EffectiveActivation;
    use crate::domain::{
        EvidenceDependency, EvidenceVersion, FrozenContext, Generation, Message, NativeOrderKey,
        ObservationDraft, ObservationKind, ObservationPosition,
    };
    use crate::fit::FitReport;
    use crate::source::{PublicationCursor, SourceBatch, SourceEvent};

    #[derive(Clone)]
    struct PendingCandidate {
        activation: EffectiveActivation,
        candidate: FrozenCandidate,
    }

    struct FakeStore {
        accepted: Mutex<Vec<AssessmentAcceptance>>,
        dispatches: Mutex<usize>,
        progress: Mutex<SourceProgress>,
        stored_candidates: Mutex<Vec<FrozenCandidate>>,
        committed: Mutex<Vec<CommittedSourceObservation>>,
        applied_events: Mutex<Vec<SourceEvent>>,
        pending: Mutex<Vec<PendingCandidate>>,
        fail_next_page: Mutex<bool>,
        load_barrier: Mutex<Option<Arc<Barrier>>>,
        applied_pages: Mutex<usize>,
        cooldown_until_unix_ms: Mutex<u64>,
        terminal: Mutex<BTreeSet<CandidateId>>,
        overflowed: Mutex<Vec<CandidateId>>,
        fail_terminal_lookup: Mutex<bool>,
        claim_barriers: Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
        terminal_lookup_calls: Mutex<Vec<Vec<CandidateId>>>,
        fail_terminal_lookup_on_call: Mutex<Option<usize>>,
        terminal_lookup_extra_id: Mutex<Option<CandidateId>>,
        activations: Mutex<BTreeMap<ActivationId, EffectiveActivation>>,
        local_answers: Mutex<BTreeMap<CandidateId, SavedLocalAnswer>>,
        local_starts: Mutex<Vec<(u64, Duration)>>,
        local_failures: Mutex<Vec<String>>,
        fail_local_start: Mutex<bool>,
        stale_local_start: Mutex<bool>,
        fail_preparation_rejection: Mutex<bool>,
        fail_preparation_load: Mutex<bool>,
        fail_acceptance: Mutex<bool>,
        fail_local_failure_write: Mutex<bool>,
        acceptance_override: Mutex<Option<AcceptanceOutcome>>,
        preparations: Mutex<BTreeMap<CandidateId, PreparationRecord>>,
        decision_progress: Mutex<BTreeMap<CandidateId, DecisionProgress>>,
        fail_intent_on_call: Mutex<Option<usize>>,
        fail_answer: Mutex<bool>,
        stale_answer: Mutex<bool>,
        evaluation_failures: Mutex<Vec<EvaluationDiagnostic>>,
    }

    #[derive(Clone)]
    struct SavedLocalAnswer {
        activation: EffectiveActivation,
        claimed: ClaimedCandidate,
        preparation: PreparationRecord,
        evaluated: EvaluatedAssessment,
    }

    impl Default for FakeStore {
        fn default() -> Self {
            Self {
                accepted: Mutex::new(Vec::new()),
                dispatches: Mutex::new(0),
                progress: Mutex::new(SourceProgress::default()),
                stored_candidates: Mutex::new(Vec::new()),
                committed: Mutex::new(Vec::new()),
                applied_events: Mutex::new(Vec::new()),
                pending: Mutex::new(Vec::new()),
                fail_next_page: Mutex::new(false),
                load_barrier: Mutex::new(None),
                applied_pages: Mutex::new(0),
                cooldown_until_unix_ms: Mutex::new(0),
                terminal: Mutex::new(BTreeSet::new()),
                overflowed: Mutex::new(Vec::new()),
                fail_terminal_lookup: Mutex::new(false),
                claim_barriers: Mutex::new(None),
                terminal_lookup_calls: Mutex::new(Vec::new()),
                fail_terminal_lookup_on_call: Mutex::new(None),
                terminal_lookup_extra_id: Mutex::new(None),
                activations: Mutex::new(BTreeMap::new()),
                local_answers: Mutex::new(BTreeMap::new()),
                local_starts: Mutex::new(Vec::new()),
                local_failures: Mutex::new(Vec::new()),
                fail_local_start: Mutex::new(false),
                stale_local_start: Mutex::new(false),
                fail_preparation_rejection: Mutex::new(false),
                fail_preparation_load: Mutex::new(false),
                fail_acceptance: Mutex::new(false),
                fail_local_failure_write: Mutex::new(false),
                acceptance_override: Mutex::new(None),
                preparations: Mutex::new(BTreeMap::new()),
                decision_progress: Mutex::new(BTreeMap::new()),
                fail_intent_on_call: Mutex::new(None),
                fail_answer: Mutex::new(false),
                stale_answer: Mutex::new(false),
                evaluation_failures: Mutex::new(Vec::new()),
            }
        }
    }

    impl CheckStore for FakeStore {
        fn release_pre_dispatch_claim(
            &self,
            claimed: &ClaimedCandidate,
        ) -> Result<ClaimOutcome, StoreError> {
            if let Some(progress) = self.decision_progress.lock().unwrap().get(&claimed.id) {
                if progress.attempts.iter().any(|row| {
                    matches!(
                        row.outcome,
                        crate::check_store::DecisionAttemptOutcome::DispatchIntent
                    )
                }) {
                    return Err(StoreError {
                        message: "cannot release an unresolved intent".into(),
                    });
                }
                if !progress.attempts.is_empty()
                    && !self.terminal.lock().unwrap().contains(&claimed.id)
                    && !self.local_answers.lock().unwrap().contains_key(&claimed.id)
                    && let Some(activation) = self
                        .activations
                        .lock()
                        .unwrap()
                        .get(&claimed.candidate.activation)
                        .cloned()
                {
                    let mut pending = self.pending.lock().unwrap();
                    if !pending
                        .iter()
                        .any(|record| record.candidate.fingerprint == claimed.candidate.fingerprint)
                    {
                        pending.push(PendingCandidate {
                            activation,
                            candidate: claimed.candidate.clone(),
                        });
                    }
                }
            }
            Ok(ClaimOutcome::Current)
        }
        fn activate(
            &self,
            _: &str,
            _: &crate::definition_lifecycle::LifecycleTransition,
            activation: Option<&EffectiveActivation>,
        ) -> Result<(), StoreError> {
            if let Some(activation) = activation {
                self.activations
                    .lock()
                    .unwrap()
                    .insert(activation.id.clone(), activation.clone());
            }
            Ok(())
        }

        fn load_source_progress(
            &self,
            _: &EffectiveActivation,
        ) -> Result<SourceProgress, StoreError> {
            let progress = self.progress.lock().unwrap().clone();
            let barrier = self.load_barrier.lock().unwrap().clone();
            if let Some(barrier) = barrier {
                barrier.wait();
            }
            Ok(progress)
        }

        fn apply_source_page(
            &self,
            activation: &EffectiveActivation,
            expected: crate::check_store::SourceProgressHead,
            page: &SourceBatch,
            committed: &[crate::source::CommittedSourceObservation],
            next: &SourceProgress,
            candidates: &[FrozenCandidate],
        ) -> Result<(), StoreError> {
            let mut fail_next = self.fail_next_page.lock().unwrap();
            if *fail_next {
                *fail_next = false;
                return Err(StoreError {
                    message: "injected page failure".to_owned(),
                });
            }
            drop(fail_next);
            let mut progress = self.progress.lock().unwrap();
            if progress.head() != expected {
                return Err(StoreError {
                    message: "source progress changed".to_owned(),
                });
            }
            if next.publication_cursor != page.next {
                return Err(StoreError {
                    message: "next progress does not acknowledge the page".to_owned(),
                });
            }
            *progress = next.clone();
            self.committed.lock().unwrap().extend_from_slice(committed);
            self.applied_events
                .lock()
                .unwrap()
                .extend_from_slice(&page.events);
            self.stored_candidates
                .lock()
                .unwrap()
                .extend_from_slice(candidates);
            self.pending
                .lock()
                .unwrap()
                .retain(|record| {
                    record.activation.id != activation.id || !page.events.iter().any(|event| {
                        matches!(event, SourceEvent::UnsafeUpdate { branch, .. } | SourceEvent::SegmentReset { branch, .. } if branch == &record.candidate.branch)
                    })
                });
            self.pending
                .lock()
                .unwrap()
                .extend(
                    candidates
                        .iter()
                        .cloned()
                        .map(|candidate| PendingCandidate {
                            activation: activation.clone(),
                            candidate,
                        }),
                );
            *self.applied_pages.lock().unwrap() += 1;
            Ok(())
        }

        fn record_overflow(&self, candidate: &FrozenCandidate) -> Result<(), StoreError> {
            let id = CandidateId::from(candidate);
            self.overflowed.lock().unwrap().push(id.clone());
            self.terminal.lock().unwrap().insert(id.clone());
            self.pending
                .lock()
                .unwrap()
                .retain(|record| CandidateId::from(&record.candidate) != id);
            Ok(())
        }

        fn claim_due(
            &self,
            candidate: &FrozenCandidate,
            now_unix_ms: u64,
            bypass_check_cooldown: bool,
        ) -> Result<CandidateClaim, StoreError> {
            let barriers = self.claim_barriers.lock().unwrap().take();
            if let Some((entered, release)) = barriers {
                entered.wait();
                release.wait();
            }
            if self
                .terminal
                .lock()
                .unwrap()
                .contains(&CandidateId::from(candidate))
            {
                return Ok(CandidateClaim::Stale);
            }
            let local_busy = {
                let answers = self.local_answers.lock().unwrap();
                let terminal = self.terminal.lock().unwrap();
                answers.iter().any(|(id, saved)| {
                    saved.claimed.candidate.activation == candidate.activation
                        && saved.claimed.candidate.branch == candidate.branch
                        && !terminal.contains(id)
                })
            };
            if local_busy {
                return Ok(CandidateClaim::NotDue {
                    retry_at_unix_ms: now_unix_ms.saturating_add(1000),
                });
            }
            let retry_at = *self.cooldown_until_unix_ms.lock().unwrap();
            let resumed = self
                .decision_progress
                .lock()
                .unwrap()
                .get(&CandidateId::from(candidate))
                .is_some_and(|progress| !progress.attempts.is_empty());
            if !bypass_check_cooldown && !resumed && now_unix_ms < retry_at {
                return Ok(CandidateClaim::NotDue {
                    retry_at_unix_ms: retry_at,
                });
            }
            Ok(CandidateClaim::Claimed(Box::new(ClaimedCandidate {
                fence: 1,
                id: CandidateId::from(candidate),
                candidate: candidate.clone(),
                prior_status: None,
            })))
        }

        fn record_dispatch_intent(
            &self,
            claimed: &ClaimedCandidate,
            attempted_at_unix_ms: u64,
            cooldown: Duration,
            attempt: &DecisionAttempt,
        ) -> Result<ClaimOutcome, StoreError> {
            *self.dispatches.lock().unwrap() += 1;
            if *self.fail_intent_on_call.lock().unwrap() == Some(*self.dispatches.lock().unwrap()) {
                return Err(StoreError {
                    message: "injected intent write failure".into(),
                });
            }
            self.decision_progress
                .lock()
                .unwrap()
                .entry(claimed.id.clone())
                .or_default()
                .attempts
                .push(crate::check_store::SavedDecisionAttempt {
                    attempt: attempt.clone(),
                    outcome: crate::check_store::DecisionAttemptOutcome::DispatchIntent,
                });
            *self.cooldown_until_unix_ms.lock().unwrap() = attempted_at_unix_ms
                .saturating_add(u64::try_from(cooldown.as_millis()).unwrap_or(u64::MAX));
            self.pending
                .lock()
                .unwrap()
                .retain(|record| record.candidate.fingerprint != claimed.candidate.fingerprint);
            Ok(ClaimOutcome::Current)
        }

        fn load_preparation(
            &self,
            claimed: &ClaimedCandidate,
        ) -> Result<Option<PreparationRecord>, StoreError> {
            if *self.fail_preparation_load.lock().unwrap() {
                return Err(StoreError {
                    message: "injected preparation read failure".into(),
                });
            }
            Ok(self
                .preparations
                .lock()
                .unwrap()
                .get(&claimed.id)
                .cloned()
                .or_else(|| {
                    self.local_answers
                        .lock()
                        .unwrap()
                        .get(&claimed.id)
                        .map(|saved| saved.preparation.clone())
                }))
        }

        fn load_decision_progress(
            &self,
            claimed: &ClaimedCandidate,
        ) -> Result<DecisionProgress, StoreError> {
            Ok(self
                .decision_progress
                .lock()
                .unwrap()
                .get(&claimed.id)
                .cloned()
                .unwrap_or_default())
        }

        fn record_preparation(
            &self,
            claimed: &ClaimedCandidate,
            preparation: &PreparationRecord,
        ) -> Result<ClaimOutcome, StoreError> {
            self.preparations
                .lock()
                .unwrap()
                .insert(claimed.id.clone(), preparation.clone());
            Ok(ClaimOutcome::Current)
        }

        fn record_local_start(
            &self,
            claimed: &ClaimedCandidate,
            now_unix_ms: u64,
            cooldown: Duration,
            preparation: &crate::fit::LocalPreparation,
        ) -> Result<ClaimOutcome, StoreError> {
            if *self.fail_local_start.lock().unwrap() {
                return Err(StoreError {
                    message: "injected local start failure".into(),
                });
            }
            if *self.stale_local_start.lock().unwrap() {
                self.terminal.lock().unwrap().insert(claimed.id.clone());
                return Ok(ClaimOutcome::Stale);
            }
            preparation
                .validate_for_resume()
                .map_err(|error| StoreError {
                    message: error.detail,
                })?;
            let activation = self
                .activations
                .lock()
                .unwrap()
                .get(&claimed.candidate.activation)
                .cloned()
                .expect("register local activation");
            assert!(activation.compiled.is_local());
            let mut answers = self.local_answers.lock().unwrap();
            if self.terminal.lock().unwrap().contains(&claimed.id) {
                return Ok(ClaimOutcome::Stale);
            }
            if let Some(saved) = answers.get(&claimed.id) {
                assert_eq!(saved.claimed, *claimed);
                assert_eq!(
                    saved.preparation,
                    PreparationRecord::Local(preparation.clone())
                );
                return Ok(ClaimOutcome::Current);
            }
            assert_eq!(preparation.started_at_unix_ms, now_unix_ms);
            answers.insert(
                claimed.id.clone(),
                SavedLocalAnswer {
                    activation,
                    claimed: claimed.clone(),
                    preparation: PreparationRecord::Local(preparation.clone()),
                    evaluated: EvaluatedAssessment::local(),
                },
            );
            self.local_starts
                .lock()
                .unwrap()
                .push((now_unix_ms, cooldown));
            *self.cooldown_until_unix_ms.lock().unwrap() =
                now_unix_ms.saturating_add(u64::try_from(cooldown.as_millis()).unwrap());
            self.pending
                .lock()
                .unwrap()
                .retain(|pending| CandidateId::from(&pending.candidate) != claimed.id);
            Ok(ClaimOutcome::Current)
        }

        fn record_decision_answer(
            &self,
            claimed: &ClaimedCandidate,
            attempt: &DecisionAttempt,
            answer: &DecisionResult,
        ) -> Result<ClaimOutcome, StoreError> {
            if *self.fail_answer.lock().unwrap() {
                return Err(StoreError {
                    message: "injected answer write failure".into(),
                });
            }
            self.decision_progress
                .lock()
                .unwrap()
                .get_mut(&claimed.id)
                .unwrap()
                .attempts
                .iter_mut()
                .find(|row| row.attempt == *attempt)
                .unwrap()
                .outcome = crate::check_store::DecisionAttemptOutcome::Answer(answer.clone());
            if let Some(activation) = self
                .activations
                .lock()
                .unwrap()
                .get(&claimed.candidate.activation)
                .cloned()
            {
                let progress = self
                    .decision_progress
                    .lock()
                    .unwrap()
                    .get(&claimed.id)
                    .cloned()
                    .unwrap();
                let validated = validate_decision_progress(
                    &activation.compiled.decisions,
                    &activation.compiled.question_requirements(),
                    &progress,
                )?;
                if let DecisionProgressState::Complete(evaluated) = validated.state {
                    self.local_answers.lock().unwrap().insert(
                        claimed.id.clone(),
                        SavedLocalAnswer {
                            activation,
                            claimed: claimed.clone(),
                            preparation: self
                                .preparations
                                .lock()
                                .unwrap()
                                .get(&claimed.id)
                                .cloned()
                                .unwrap(),
                            evaluated,
                        },
                    );
                }
            }
            Ok(if *self.stale_answer.lock().unwrap() {
                ClaimOutcome::Stale
            } else {
                ClaimOutcome::Current
            })
        }

        fn accept(
            &self,
            assessment: &AssessmentAcceptance,
        ) -> Result<AcceptanceOutcome, StoreError> {
            if *self.fail_acceptance.lock().unwrap() {
                return Err(StoreError {
                    message: "injected acceptance write failure".into(),
                });
            }
            if let Some(outcome) = *self.acceptance_override.lock().unwrap() {
                return Ok(outcome);
            }
            self.accepted.lock().unwrap().push(assessment.clone());
            if self
                .local_answers
                .lock()
                .unwrap()
                .contains_key(&assessment.claimed.id)
            {
                self.terminal
                    .lock()
                    .unwrap()
                    .insert(assessment.claimed.id.clone());
            }
            Ok(AcceptanceOutcome::Accepted)
        }

        fn load_recovery(
            &self,
            after: Option<&CandidateId>,
            limit: usize,
        ) -> Result<Vec<crate::check_store::RecoveryRecord>, StoreError> {
            let mut records = self.pending.lock().unwrap().clone();
            records.sort_by_key(|record| CandidateId::from(&record.candidate));
            let mut recovery = records
                .into_iter()
                .filter(|record| {
                    after.is_none_or(|after| CandidateId::from(&record.candidate) > *after)
                })
                .map(|record| crate::check_store::RecoveryRecord {
                    activation: record.activation,
                    work: RecoveryWork::Pending {
                        resumed: self
                            .decision_progress
                            .lock()
                            .unwrap()
                            .get(&CandidateId::from(&record.candidate))
                            .is_some_and(|progress| !progress.attempts.is_empty()),
                        candidate: record.candidate,
                    },
                })
                .collect::<Vec<_>>();
            let terminal = self.terminal.lock().unwrap().clone();
            recovery.extend(
                self.local_answers
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(id, _)| {
                        after.is_none_or(|after| *id > after) && !terminal.contains(*id)
                    })
                    .map(|(_, saved)| crate::check_store::RecoveryRecord {
                        activation: saved.activation.clone(),
                        work: RecoveryWork::Answered {
                            claimed: saved.claimed.clone(),
                            evaluated: saved.evaluated.clone(),
                        },
                    }),
            );
            recovery.sort_by_key(|record| match &record.work {
                RecoveryWork::Pending { candidate, .. } => CandidateId::from(candidate),
                RecoveryWork::Answered { claimed, .. } => claimed.id.clone(),
            });
            recovery.truncate(limit);
            Ok(recovery)
        }

        fn load_terminal_candidate_ids(
            &self,
            candidate_ids: &[CandidateId],
        ) -> Result<Vec<CandidateId>, StoreError> {
            assert!(candidate_ids.len() <= MAX_PENDING_CANDIDATES);
            let call = {
                let mut calls = self.terminal_lookup_calls.lock().unwrap();
                calls.push(candidate_ids.to_vec());
                calls.len()
            };
            if *self.fail_terminal_lookup.lock().unwrap()
                || *self.fail_terminal_lookup_on_call.lock().unwrap() == Some(call)
            {
                return Err(StoreError {
                    message: "injected terminal candidate lookup failure".into(),
                });
            }
            let terminal = self.terminal.lock().unwrap();
            let mut found = candidate_ids
                .iter()
                .filter(|id| terminal.contains(*id))
                .cloned()
                .collect::<Vec<_>>();
            if let Some(id) = self.terminal_lookup_extra_id.lock().unwrap().take() {
                found.push(id);
            }
            Ok(found)
        }

        fn record_attempt_failure(
            &self,
            claimed: &ClaimedCandidate,
            attempt: &DecisionAttempt,
            outcome: AttemptFailure,
            _: &EvaluationDiagnostic,
        ) -> Result<ClaimOutcome, StoreError> {
            self.decision_progress
                .lock()
                .unwrap()
                .get_mut(&claimed.id)
                .unwrap()
                .attempts
                .iter_mut()
                .find(|row| row.attempt == *attempt)
                .unwrap()
                .outcome = match outcome {
                AttemptFailure::SafeTransient => {
                    crate::check_store::DecisionAttemptOutcome::SafeTransient
                }
                AttemptFailure::Unknown => crate::check_store::DecisionAttemptOutcome::Unknown,
                AttemptFailure::Permanent => crate::check_store::DecisionAttemptOutcome::Permanent,
            };
            Ok(ClaimOutcome::Current)
        }

        fn record_failure(
            &self,
            claimed: &ClaimedCandidate,
            detail: &str,
        ) -> Result<ClaimOutcome, StoreError> {
            if *self.fail_local_failure_write.lock().unwrap() {
                return Err(StoreError {
                    message: "injected local failure write failure".into(),
                });
            }
            if self.local_answers.lock().unwrap().contains_key(&claimed.id) {
                self.terminal.lock().unwrap().insert(claimed.id.clone());
                self.local_failures.lock().unwrap().push(detail.into());
            }
            Ok(ClaimOutcome::Current)
        }

        fn record_evaluation_failure(
            &self,
            claimed: &ClaimedCandidate,
            diagnostic: &EvaluationDiagnostic,
        ) -> Result<ClaimOutcome, StoreError> {
            self.evaluation_failures
                .lock()
                .unwrap()
                .push(diagnostic.clone());
            self.record_failure(claimed, &diagnostic.detail)
        }

        fn record_not_assessed(
            &self,
            claimed: &ClaimedCandidate,
            _: &str,
        ) -> Result<ClaimOutcome, StoreError> {
            self.terminal.lock().unwrap().insert(claimed.id.clone());
            self.pending
                .lock()
                .unwrap()
                .retain(|pending| pending.candidate.fingerprint != claimed.candidate.fingerprint);
            Ok(ClaimOutcome::Current)
        }

        fn record_preparation_rejection(
            &self,
            claimed: &ClaimedCandidate,
            failure: &PreparationFailure,
        ) -> Result<ClaimOutcome, StoreError> {
            if *self.fail_preparation_rejection.lock().unwrap() {
                return Err(StoreError {
                    message: "injected rejection failure".into(),
                });
            }
            self.record_not_assessed(claimed, &failure.detail)
        }
    }

    struct FakeEvaluator;

    impl DecisionEvaluator for FakeEvaluator {
        fn prepare(
            &self,
            activation: &EffectiveActivation,
            inputs: Vec<DecisionInput>,
            budget: EvaluationBudget,
        ) -> Result<PreparedAssessment, PreparationFailure> {
            test_prepare(activation, inputs, budget)
        }
        fn validate_prepared(
            &self,
            activation: &EffectiveActivation,
            prepared: &PreparedAssessment,
            purpose: PreparationValidationPurpose,
        ) -> Result<(), PreparationFailure> {
            test_validate(activation, prepared, purpose)
        }
        fn evaluate_prepared<'a>(
            &'a self,
            activation: &'a EffectiveActivation,
            _: &'a FrozenCandidate,
            _: &'a PreparedAssessment,
            attempt: &'a DecisionAttempt,
        ) -> DecisionFuture<'a> {
            Box::pin(async move { Ok(test_answer(activation, attempt, None)) })
        }
    }

    struct PendingEvaluator {
        calls: AtomicUsize,
    }

    impl DecisionEvaluator for PendingEvaluator {
        fn prepare(
            &self,
            activation: &EffectiveActivation,
            inputs: Vec<DecisionInput>,
            budget: EvaluationBudget,
        ) -> Result<PreparedAssessment, PreparationFailure> {
            test_prepare(activation, inputs, budget)
        }
        fn validate_prepared(
            &self,
            activation: &EffectiveActivation,
            prepared: &PreparedAssessment,
            purpose: PreparationValidationPurpose,
        ) -> Result<(), PreparationFailure> {
            test_validate(activation, prepared, purpose)
        }
        fn evaluate_prepared<'a>(
            &'a self,
            _: &'a EffectiveActivation,
            _: &'a FrozenCandidate,
            _: &'a PreparedAssessment,
            _: &'a DecisionAttempt,
        ) -> DecisionFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::pending())
        }
    }

    fn activation() -> EffectiveActivation {
        EffectiveActivation {
            id: ActivationId {
                scope: Default::default(),
                definition_id: "retry".to_owned(),
                generation: Generation(1),
            },
            revision: [1; 32],
            definition_scope: crate::discovery::DefinitionScope::Global,
            source_paths: vec![],
            project_root: None,
            compiled: Arc::new(
                compile("name: Retry\non:\n  tool_result:\n  cooldown: 10s\ndecisions:\n  review:\n    input: {activity: \"${{ message.tool.result.status }}\"}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    input: {result: failure}\n").unwrap(),
            ),
            bootstrap_observation_position: ObservationPosition(0),
            bootstrap_publication: Default::default(),
        }
    }

    fn candidate(activation: &EffectiveActivation, trigger: u64) -> FrozenCandidate {
        FrozenCandidate {
            activation: activation.id.clone(),
            branch: BranchId("branch-a".to_owned()),
            trigger_position: ObservationPosition(trigger),
            messages: vec![Message {
                id: format!("message-{trigger}"),
                value: json!({"tool": {"result": {"status": "error"}}}),
            }],
            context: FrozenContext {
                value: json!({
                    "session": {"id": "session-a", "requests": []},
                    "message": {"tool": {"result": {"status": "error"}}},
                }),
                dependencies: vec![EvidenceDependency::Message {
                    id: format!("message-{trigger}"),
                    version: crate::domain::EvidenceVersion(format!("version-{trigger}")),
                }],
            },
            dependencies: vec![],
            fingerprint: [trigger as u8; 32],
        }
    }

    fn candidate_on_branch(
        activation: &EffectiveActivation,
        trigger: u64,
        branch: &str,
    ) -> FrozenCandidate {
        let mut candidate = candidate(activation, trigger);
        candidate.branch = BranchId(branch.to_owned());
        candidate
    }

    fn window_activation() -> EffectiveActivation {
        EffectiveActivation {
            compiled: Arc::new(
                compile("name: Retry\non:\n  tool_result:\n  window: {size: 2, min: 2}\ndecisions:\n  review:\n    input: {activity: \"${{ window.messages }}\"}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    input: {result: failure}\n").unwrap(),
            ),
            ..activation()
        }
    }

    fn source_event(cursor: u64, identity: &str) -> SourceEvent {
        source_event_revision(cursor, identity, 1)
    }

    fn source_event_revision(cursor: u64, identity: &str, revision: u64) -> SourceEvent {
        SourceEvent::Observation {
            source_evidence_version: None,
            cursor: PublicationCursor(cursor),
            identity: identity.to_owned(),
            revision,
            observation: Box::new(ObservationDraft {
                first_eligible_publication: None,
                branch: BranchId("branch-a".to_owned()),
                native_order: NativeOrderKey(format!("native-{identity}")),
                first_terminal_publication: None,
                version: EvidenceVersion(format!("version-{identity}-{revision}")),
                kind: ObservationKind::ToolResult {
                    message: Message {
                        id: identity.to_owned(),
                        value: json!({"tool": {"result": {"status": "error"}}}),
                    },
                    category: "shell".to_owned(),
                    status: "error".to_owned(),
                },
                context: FrozenContext {
                    value: json!({"session": {"requests": []}}),
                    dependencies: vec![],
                },
                complete: true,
            }),
        }
    }

    fn source_page(cursor: u64, identity: &str) -> SourceBatch {
        SourceBatch {
            events: vec![source_event(cursor, identity)],
            next: PublicationCursor(cursor),
            has_more: false,
        }
    }

    fn revised_source_page(cursor: u64, identity: &str, revision: u64) -> SourceBatch {
        SourceBatch {
            events: vec![source_event_revision(cursor, identity, revision)],
            next: PublicationCursor(cursor),
            has_more: false,
        }
    }

    fn run_task(mut task: EvaluationTask<'_>) -> RunSettlement {
        let mut context = Context::from_waker(Waker::noop());
        for _ in 0..32 {
            match task.as_mut().poll(&mut context) {
                Poll::Ready(Ok(outcome)) => return outcome,
                Poll::Ready(Err(error)) => panic!("evaluation task failed: {error}"),
                Poll::Pending => {}
            }
        }
        panic!("immediate evaluator did not settle within its bounded attempts");
    }

    #[test]
    fn latest_pending_coalesces_and_dispatch_cooldown_begins_at_attempt() {
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(Arc::clone(&store), Arc::new(FakeEvaluator));
        let mut activation = activation();
        Arc::make_mut(&mut activation.compiled).trigger.window =
            Some(crate::compiler::RollingWindow {
                size: 1,
                minimum: 1,
            });
        assert_eq!(
            engine
                .enqueue(&activation, candidate(&activation, 1))
                .unwrap(),
            EnqueueOutcome::Queued
        );
        let ticket = engine
            .state
            .lock()
            .unwrap()
            .service_tickets
            .get(&ScheduleKey {
                activation: activation.id.clone(),
                branch: BranchId("branch-a".to_owned()),
            })
            .copied();
        assert_eq!(
            engine
                .enqueue(&activation, candidate(&activation, 2))
                .unwrap(),
            EnqueueOutcome::Coalesced
        );
        assert_eq!(
            engine
                .state
                .lock()
                .unwrap()
                .service_tickets
                .get(&ScheduleKey {
                    activation: activation.id.clone(),
                    branch: BranchId("branch-a".to_owned()),
                })
                .copied(),
            ticket
        );
        let StartOutcome::Started(task) = engine.start_one_due(&activation, 50_000).unwrap() else {
            panic!("first candidate must start");
        };
        assert_eq!(run_task(task).outcome, RunOutcome::Accepted);
        assert_eq!(*store.dispatches.lock().unwrap(), 1);

        assert_eq!(
            engine
                .enqueue(&activation, candidate(&activation, 3))
                .unwrap(),
            EnqueueOutcome::Queued
        );
        assert!(matches!(
            engine.start_one_due(&activation, 59_000).unwrap(),
            StartOutcome::NotDue
        ));
        assert_eq!(*store.dispatches.lock().unwrap(), 1);
        let StartOutcome::Started(task) = engine.start_one_due(&activation, 60_000).unwrap() else {
            panic!("candidate must start after cooldown");
        };
        assert_eq!(run_task(task).outcome, RunOutcome::Accepted);
        assert_eq!(*store.dispatches.lock().unwrap(), 2);
        let accepted = store.accepted.lock().unwrap();
        assert_eq!(accepted.len(), 2);
        assert_eq!(
            accepted[0].claimed.candidate.trigger_position,
            ObservationPosition(2)
        );
    }

    #[test]
    fn live_dispatch_prefers_recent_work_and_gives_old_work_a_fair_turn() {
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(Arc::clone(&store), Arc::new(FakeEvaluator));
        let mut activation = activation();
        Arc::make_mut(&mut activation.compiled).trigger.cooldown = Duration::ZERO;
        for (trigger, branch) in [(1, "old-a"), (2, "old-b"), (3, "old-c"), (4, "recent-d")] {
            assert_eq!(
                engine
                    .enqueue(
                        &activation,
                        candidate_on_branch(&activation, trigger, branch)
                    )
                    .unwrap(),
                EnqueueOutcome::Queued
            );
        }

        for (now, trigger, branch) in [
            (10_000, 4, "recent-d"),
            (20_000, 5, "recent-e"),
            (30_000, 6, "recent-f"),
        ] {
            if trigger > 4 {
                assert_eq!(
                    engine
                        .enqueue(
                            &activation,
                            candidate_on_branch(&activation, trigger, branch)
                        )
                        .unwrap(),
                    EnqueueOutcome::Queued
                );
            }
            let StartOutcome::Started(task) = engine.start_one_due(&activation, now).unwrap()
            else {
                panic!("recent candidate must start");
            };
            assert_eq!(run_task(task).outcome, RunOutcome::Accepted);
            assert_eq!(
                store
                    .accepted
                    .lock()
                    .unwrap()
                    .last()
                    .unwrap()
                    .claimed
                    .candidate
                    .branch,
                BranchId(branch.to_owned())
            );
        }

        let StartOutcome::Started(task) = engine.start_one_due(&activation, 40_000).unwrap() else {
            panic!("fair candidate must start");
        };
        assert_eq!(run_task(task).outcome, RunOutcome::Accepted);
        assert_eq!(
            store
                .accepted
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .claimed
                .candidate
                .branch,
            BranchId("old-a".to_owned())
        );
    }

    #[test]
    fn one_branch_never_has_two_active_evaluations() {
        let store = Arc::new(FakeStore::default());
        let evaluator = Arc::new(PendingEvaluator {
            calls: AtomicUsize::new(0),
        });
        let engine = CheckEngine::new(Arc::clone(&store), evaluator.clone());
        let activation = activation();
        engine
            .enqueue(&activation, candidate(&activation, 1))
            .unwrap();
        let StartOutcome::Started(mut task) = engine.start_one_due(&activation, 10_000).unwrap()
        else {
            panic!("candidate must start");
        };
        assert_eq!(evaluator.calls.load(Ordering::SeqCst), 0);
        let mut context = Context::from_waker(Waker::noop());
        assert!(task.as_mut().poll(&mut context).is_pending());
        assert_eq!(evaluator.calls.load(Ordering::SeqCst), 1);
        engine
            .enqueue(&activation, candidate(&activation, 2))
            .unwrap();
        assert!(matches!(
            engine.start_one_due(&activation, 10_000).unwrap(),
            StartOutcome::NotDue
        ));
        drop(task);
        assert!(matches!(
            engine.start_one_due(&activation, 20_000).unwrap(),
            StartOutcome::NotDue
        ));
        assert_eq!(*store.dispatches.lock().unwrap(), 1);
    }

    #[test]
    fn selector_progress_and_observation_positions_resume_after_restart() {
        let store = Arc::new(FakeStore::default());
        let activation = window_activation();
        let first_engine = CheckEngine::new(Arc::clone(&store), Arc::new(FakeEvaluator));
        first_engine
            .apply_source_page(&activation, &source_page(100, "result-1"))
            .unwrap();

        let restarted_engine = CheckEngine::new(Arc::clone(&store), Arc::new(FakeEvaluator));
        let outcome = restarted_engine
            .apply_source_page(&activation, &source_page(250, "result-2"))
            .unwrap();

        assert_eq!(outcome.candidates, 1);
        let progress = store.progress.lock().unwrap();
        assert_eq!(progress.publication_cursor, PublicationCursor(250));
        assert_eq!(progress.observation_position, ObservationPosition(2));
        let stored = store.stored_candidates.lock().unwrap();
        assert_eq!(stored[0].trigger_position, ObservationPosition(2));
        assert_eq!(
            stored[0]
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["result-1", "result-2"]
        );
    }

    #[test]
    fn correction_advances_observation_order_without_creating_a_trigger() {
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(Arc::clone(&store), Arc::new(FakeEvaluator));
        let activation = window_activation();

        assert_eq!(
            engine
                .apply_source_page(&activation, &source_page(100, "result-1"))
                .unwrap()
                .candidates,
            0
        );
        assert_eq!(
            engine
                .apply_source_page(&activation, &revised_source_page(200, "result-1", 2),)
                .unwrap()
                .candidates,
            0
        );
        assert_eq!(
            engine
                .apply_source_page(&activation, &source_page(300, "result-2"))
                .unwrap()
                .candidates,
            1
        );

        let progress = store.progress.lock().unwrap();
        assert_eq!(progress.observation_position, ObservationPosition(3));
        let candidate = &store.stored_candidates.lock().unwrap()[0];
        assert_eq!(candidate.trigger_position, ObservationPosition(3));
        assert_eq!(candidate.messages[0].id, "result-1");
        assert!(
            candidate
                .dependencies
                .contains(&EvidenceDependency::Message {
                    id: "result-1".to_owned(),
                    version: EvidenceVersion("version-result-1-2".to_owned()),
                })
        );
    }

    #[test]
    fn later_same_page_correction_invalidates_earlier_candidate_before_dispatch() {
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(Arc::clone(&store), Arc::new(FakeEvaluator));
        let activation = window_activation();
        let page = SourceBatch {
            events: vec![
                source_event(100, "result-1"),
                source_event(200, "result-2"),
                source_event_revision(300, "result-1", 2),
            ],
            next: PublicationCursor(300),
            has_more: false,
        };

        let outcome = engine.apply_source_page(&activation, &page).unwrap();
        assert_eq!(outcome.candidates, 0);
        assert!(store.stored_candidates.lock().unwrap().is_empty());
    }

    #[test]
    fn later_same_page_unsafe_update_invalidates_earlier_candidate_before_dispatch() {
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(Arc::clone(&store), Arc::new(FakeEvaluator));
        let activation = window_activation();
        let page = SourceBatch {
            events: vec![
                source_event(100, "result-1"),
                source_event(200, "result-2"),
                SourceEvent::UnsafeUpdate {
                    cursor: PublicationCursor(300),
                    branch: BranchId("branch-a".to_owned()),
                    reason: "result order is unavailable".to_owned(),
                },
            ],
            next: PublicationCursor(300),
            has_more: false,
        };

        let outcome = engine.apply_source_page(&activation, &page).unwrap();
        assert_eq!(outcome.candidates, 0);
        assert!(store.stored_candidates.lock().unwrap().is_empty());
    }

    #[test]
    fn same_page_quality_gap_keeps_frozen_candidate_and_marks_later_candidates_partial() {
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
        let activation = window_activation();
        let outcome = engine
            .apply_source_page(
                &activation,
                &SourceBatch {
                    events: vec![
                        source_event(1, "a"),
                        source_event(2, "b"),
                        SourceEvent::Gap {
                            cursor: PublicationCursor(3),
                            branch: BranchId("branch-a".into()),
                            reason: "request-history-incomplete".into(),
                        },
                        source_event(4, "c"),
                    ],
                    next: PublicationCursor(4),
                    has_more: false,
                },
            )
            .unwrap();

        assert_eq!(outcome.candidates, 2);
        let candidates = store.stored_candidates.lock().unwrap();
        assert_eq!(candidates.len(), 2);
        for (candidate, expected_messages, coverage) in [
            (&candidates[0], ["a", "b"], "complete"),
            (&candidates[1], ["b", "c"], "partial"),
        ] {
            assert_eq!(
                candidate
                    .messages
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>(),
                expected_messages
            );
            assert_eq!(
                candidate.context.value["session"]["source_coverage"],
                coverage
            );
        }
    }

    #[test]
    fn quality_gap_keeps_pending_work_and_current_window() {
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
        let activation = window_activation();
        engine
            .apply_source_page(
                &activation,
                &SourceBatch {
                    events: vec![source_event(1, "a"), source_event(2, "b")],
                    next: PublicationCursor(2),
                    has_more: false,
                },
            )
            .unwrap();
        let pending = store.pending.lock().unwrap()[0].candidate.clone();
        let before = store.progress.lock().unwrap().selection.clone();
        engine
            .apply_source_page(
                &activation,
                &SourceBatch {
                    events: vec![SourceEvent::Gap {
                        cursor: PublicationCursor(3),
                        branch: BranchId("branch-a".into()),
                        reason: "request-history-incomplete".into(),
                    }],
                    next: PublicationCursor(3),
                    has_more: false,
                },
            )
            .unwrap();

        assert_eq!(store.pending.lock().unwrap()[0].candidate, pending);
        let key = ScheduleKey {
            activation: activation.id.clone(),
            branch: BranchId("branch-a".into()),
        };
        assert_eq!(
            engine.state.lock().unwrap().pending[&key][&pending.trigger_position].candidate,
            pending
        );
        let progress = store.progress.lock().unwrap();
        let branch = &progress.selection.branches[&key.branch];
        assert_eq!(branch.results, before.branches[&key.branch].results);
        assert!(!branch.coverage_complete);
        assert_eq!(progress.selection.seen_results, before.seen_results);
        drop(progress);

        assert_eq!(
            engine
                .apply_source_page(&activation, &source_page(4, "c"))
                .unwrap()
                .candidates,
            1
        );
        let candidates = store.stored_candidates.lock().unwrap();
        let latest = candidates.last().unwrap();
        assert_eq!(
            latest
                .messages
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["b", "c"]
        );
        assert_eq!(
            latest.context.value["session"]["source_coverage"],
            "partial"
        );
    }

    #[test]
    fn unsafe_update_discards_pending_work_and_prevents_old_result_reuse() {
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
        let activation = window_activation();
        engine
            .apply_source_page(
                &activation,
                &SourceBatch {
                    events: vec![source_event(1, "a"), source_event(2, "b")],
                    next: PublicationCursor(2),
                    has_more: false,
                },
            )
            .unwrap();
        let reset = SourceBatch {
            events: vec![SourceEvent::UnsafeUpdate {
                cursor: PublicationCursor(3),
                branch: BranchId("branch-a".into()),
                reason: "result ownership is unavailable".into(),
            }],
            next: PublicationCursor(3),
            has_more: false,
        };
        *store.fail_next_page.lock().unwrap() = true;
        assert!(engine.apply_source_page(&activation, &reset).is_err());
        assert_eq!(engine.state.lock().unwrap().pending.len(), 1);
        assert_eq!(store.pending.lock().unwrap().len(), 1);
        engine.apply_source_page(&activation, &reset).unwrap();
        assert!(engine.state.lock().unwrap().pending.is_empty());
        assert!(store.pending.lock().unwrap().is_empty());
        let progress = store.progress.lock().unwrap();
        let branch = BranchId("branch-a".into());
        assert!(progress.selection.branches[&branch].results.is_empty());
        assert!(!progress.selection.branches[&branch].coverage_complete);
        assert_eq!(progress.selection.seen_results.len(), 2);
        assert!(progress.selection.seen_results.values().all(|seen| {
            seen.terminal_seen && seen.membership == crate::selection::ResultMembership::Retired
        }));
        drop(progress);

        let restarted = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
        assert_eq!(restarted.recover_pending().unwrap().candidates, 0);
        let replay = restarted
            .apply_source_page(&activation, &source_page(4, "a"))
            .unwrap();
        assert_eq!(replay.observations, 0);
        assert_eq!(replay.candidates, 0);
        assert_eq!(
            restarted
                .apply_source_page(&activation, &revised_source_page(5, "b", 2))
                .unwrap()
                .candidates,
            0
        );
        assert_eq!(
            restarted
                .apply_source_page(&activation, &source_page(6, "c"))
                .unwrap()
                .candidates,
            0
        );
        assert_eq!(
            restarted
                .apply_source_page(&activation, &source_page(7, "d"))
                .unwrap()
                .candidates,
            1
        );
        let candidates = store.stored_candidates.lock().unwrap();
        assert_eq!(candidates.len(), 2);
        let latest = candidates.last().unwrap();
        assert_eq!(
            latest
                .messages
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["c", "d"]
        );
        assert_eq!(
            latest.context.value["session"]["source_coverage"],
            "partial"
        );
    }

    #[test]
    fn segment_reset_clears_pending_work_only_after_commit_and_keeps_history() {
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
        let activation = window_activation();
        let page = SourceBatch {
            events: vec![source_event(1, "a"), source_event(2, "b")],
            next: PublicationCursor(2),
            has_more: false,
        };
        engine.apply_source_page(&activation, &page).unwrap();
        let StartOutcome::Started(task) = engine.start_one_due(&activation, 100_000).unwrap()
        else {
            panic!("the first window must run")
        };
        assert_eq!(run_task(task).outcome, RunOutcome::Accepted);
        let history = store.accepted.lock().unwrap().clone();
        engine
            .apply_source_page(&activation, &source_page(3, "c"))
            .unwrap();
        let mut other = source_event(4, "other-a");
        let SourceEvent::Observation { observation, .. } = &mut other else {
            unreachable!()
        };
        observation.branch = BranchId("branch-b".into());
        engine
            .apply_source_page(
                &activation,
                &SourceBatch {
                    events: vec![other],
                    next: PublicationCursor(4),
                    has_more: false,
                },
            )
            .unwrap();
        let mut other = source_event(5, "other-b");
        let SourceEvent::Observation { observation, .. } = &mut other else {
            unreachable!()
        };
        observation.branch = BranchId("branch-b".into());
        engine
            .apply_source_page(
                &activation,
                &SourceBatch {
                    events: vec![other],
                    next: PublicationCursor(5),
                    has_more: false,
                },
            )
            .unwrap();
        let before =
            store.progress.lock().unwrap().selection.branches[&BranchId("branch-b".into())].clone();
        let reset = SourceBatch {
            events: vec![SourceEvent::SegmentReset {
                cursor: PublicationCursor(6),
                branch: BranchId("branch-a".into()),
                reason: "capture-lost".into(),
            }],
            next: PublicationCursor(6),
            has_more: false,
        };
        *store.fail_next_page.lock().unwrap() = true;
        assert!(engine.apply_source_page(&activation, &reset).is_err());
        assert_eq!(engine.state.lock().unwrap().pending.len(), 2);
        engine.apply_source_page(&activation, &reset).unwrap();
        assert_eq!(engine.state.lock().unwrap().pending.len(), 1);
        assert_eq!(store.pending.lock().unwrap().len(), 1);
        assert_eq!(
            store.pending.lock().unwrap()[0].candidate.branch,
            BranchId("branch-b".into())
        );
        assert_eq!(
            store.accepted.lock().unwrap()[0].claimed.candidate,
            history[0].claimed.candidate
        );
        assert_eq!(
            store.accepted.lock().unwrap()[0].evaluated,
            history[0].evaluated
        );
        let progress = store.progress.lock().unwrap();
        assert_eq!(
            progress.selection.branches[&BranchId("branch-b".into())],
            before
        );
        assert!(
            progress.selection.branches[&BranchId("branch-a".into())]
                .results
                .is_empty()
        );
        drop(progress);

        let restarted = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
        assert_eq!(restarted.recover_pending().unwrap().candidates, 1);
        assert_eq!(
            restarted
                .apply_source_page(&activation, &source_page(7, "d"))
                .unwrap()
                .candidates,
            0
        );
        assert_eq!(
            restarted
                .apply_source_page(&activation, &source_page(8, "e"))
                .unwrap()
                .candidates,
            1
        );
        let candidates = store.stored_candidates.lock().unwrap();
        let latest = candidates.last().unwrap();
        assert_eq!(
            latest
                .messages
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["d", "e"]
        );
        assert_eq!(
            latest.context.value["session"]["source_coverage"],
            "partial"
        );
        assert_eq!(candidates[0], history[0].claimed.candidate);
    }

    #[test]
    fn same_page_reset_discards_earlier_candidate_but_keeps_later_safe_window() {
        for reset in [
            SourceEvent::SegmentReset {
                cursor: PublicationCursor(3),
                branch: BranchId("branch-a".into()),
                reason: "capture-lost".into(),
            },
            SourceEvent::UnsafeUpdate {
                cursor: PublicationCursor(3),
                branch: BranchId("branch-a".into()),
                reason: "result order is unavailable".into(),
            },
        ] {
            let store = Arc::new(FakeStore::default());
            let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
            let activation = window_activation();
            let outcome = engine
                .apply_source_page(
                    &activation,
                    &SourceBatch {
                        events: vec![
                            source_event(1, "a"),
                            source_event(2, "b"),
                            reset,
                            source_event(4, "c"),
                            source_event(5, "d"),
                        ],
                        next: PublicationCursor(5),
                        has_more: false,
                    },
                )
                .unwrap();
            assert_eq!(outcome.candidates, 1);
            assert_eq!(
                store.stored_candidates.lock().unwrap()[0]
                    .messages
                    .iter()
                    .map(|item| item.id.as_str())
                    .collect::<Vec<_>>(),
                ["c", "d"]
            );
            assert_eq!(
                store.stored_candidates.lock().unwrap()[0].context.value["session"]["source_coverage"],
                "partial"
            );
        }
    }

    #[test]
    fn reset_keeps_cooldown_and_does_not_release_an_uncertain_provider_attempt() {
        for reset in [
            SourceEvent::SegmentReset {
                cursor: PublicationCursor(2),
                branch: BranchId("branch-a".into()),
                reason: "capture-lost".into(),
            },
            SourceEvent::UnsafeUpdate {
                cursor: PublicationCursor(2),
                branch: BranchId("branch-a".into()),
                reason: "result identity is unavailable".into(),
            },
        ] {
            let store = Arc::new(FakeStore::default());
            let evaluator = Arc::new(PendingEvaluator {
                calls: AtomicUsize::new(0),
            });
            let engine = CheckEngine::new(store.clone(), evaluator.clone());
            let activation = activation();
            engine
                .apply_source_page(&activation, &source_page(1, "a"))
                .unwrap();
            let StartOutcome::Started(mut task) =
                engine.start_one_due(&activation, 100_000).unwrap()
            else {
                panic!("the first candidate must dispatch")
            };
            let waker = Waker::noop();
            assert!(matches!(
                task.as_mut().poll(&mut Context::from_waker(waker)),
                Poll::Pending
            ));
            engine
                .apply_source_page(
                    &activation,
                    &SourceBatch {
                        events: vec![reset],
                        next: PublicationCursor(2),
                        has_more: false,
                    },
                )
                .unwrap();
            drop(task);
            let key = ScheduleKey {
                activation: activation.id.clone(),
                branch: BranchId("branch-a".into()),
            };
            assert_eq!(
                engine.state.lock().unwrap().cooldown_until_unix_ms[&key],
                110_000
            );
            assert_eq!(*store.cooldown_until_unix_ms.lock().unwrap(), 110_000);
            engine
                .apply_source_page(&activation, &source_page(3, "b"))
                .unwrap();
            assert!(matches!(
                engine.start_one_due(&activation, 200_000).unwrap(),
                StartOutcome::NotDue
            ));
            assert_eq!(evaluator.calls.load(Ordering::SeqCst), 1);
            assert_eq!(*store.dispatches.lock().unwrap(), 1);
        }
    }

    #[test]
    fn failed_source_page_does_not_advance_selector_or_acknowledgment() {
        let store = Arc::new(FakeStore::default());
        *store.fail_next_page.lock().unwrap() = true;
        let engine = CheckEngine::new(Arc::clone(&store), Arc::new(FakeEvaluator));
        let activation = window_activation();
        let first_page = source_page(17, "result-1");

        assert!(engine.apply_source_page(&activation, &first_page).is_err());
        assert_eq!(*store.progress.lock().unwrap(), SourceProgress::default());
        assert!(
            !engine
                .selectors
                .lock()
                .unwrap()
                .contains_key(&activation.id)
        );
        assert_eq!(
            engine
                .apply_source_page(&activation, &first_page)
                .unwrap()
                .candidates,
            0
        );
        let outcome = engine
            .apply_source_page(&activation, &source_page(99, "result-2"))
            .unwrap();

        assert_eq!(outcome.candidates, 1);
        assert_eq!(
            store.stored_candidates.lock().unwrap()[0].trigger_position,
            ObservationPosition(2)
        );
        assert_eq!(*store.applied_pages.lock().unwrap(), 2);
    }

    #[test]
    fn concurrent_engines_cannot_commit_the_same_source_page_twice() {
        let store = Arc::new(FakeStore::default());
        *store.load_barrier.lock().unwrap() = Some(Arc::new(Barrier::new(2)));
        let activation = activation();
        let page = source_page(40, "result-1");
        let engines = [
            CheckEngine::new(Arc::clone(&store), Arc::new(FakeEvaluator)),
            CheckEngine::new(Arc::clone(&store), Arc::new(FakeEvaluator)),
        ];
        let handles = engines.map(|engine| {
            let activation = activation.clone();
            let page = page.clone();
            std::thread::spawn(move || engine.apply_source_page(&activation, &page))
        });
        let results = handles.map(|handle| handle.join().unwrap());

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        assert_eq!(*store.applied_pages.lock().unwrap(), 1);
        assert_eq!(
            store.progress.lock().unwrap().publication_cursor,
            PublicationCursor(40)
        );
    }

    #[test]
    fn restart_respects_store_cooldown_without_dropping_pending_work() {
        let store = Arc::new(FakeStore::default());
        let activation = activation();
        let first_engine = CheckEngine::new(Arc::clone(&store), Arc::new(FakeEvaluator));
        first_engine
            .enqueue(&activation, candidate(&activation, 1))
            .unwrap();
        let StartOutcome::Started(task) = first_engine.start_one_due(&activation, 50_000).unwrap()
        else {
            panic!("first candidate must start");
        };
        assert_eq!(run_task(task).outcome, RunOutcome::Accepted);

        let next_candidate = candidate(&activation, 2);
        store.pending.lock().unwrap().push(PendingCandidate {
            activation: activation.clone(),
            candidate: next_candidate,
        });
        let restarted_engine = CheckEngine::new(Arc::clone(&store), Arc::new(FakeEvaluator));
        assert_eq!(restarted_engine.recover_pending().unwrap().candidates, 1);
        assert!(matches!(
            restarted_engine.start_one_due(&activation, 59_999).unwrap(),
            StartOutcome::NotDue
        ));
        let StartOutcome::Started(task) =
            restarted_engine.start_one_due(&activation, 60_000).unwrap()
        else {
            panic!("pending candidate must survive the durable cooldown");
        };
        assert_eq!(run_task(task).outcome, RunOutcome::Accepted);
        assert_eq!(*store.dispatches.lock().unwrap(), 2);
    }
}
