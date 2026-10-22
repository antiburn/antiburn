//! Bounded candidate scheduling and single-flight assessment coordination.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::actions::{ActionPlanningError, plan_status_actions};
use crate::check_store::{
    AcceptanceOutcome, AssessmentAcceptance, AttemptFailure, CandidateClaim, CandidateId,
    CheckStore, ClaimOutcome, ClaimedCandidate, EvaluatedCandidate, RecoveryWork, SourceProgress,
    StoreError,
};
use crate::definition_lifecycle::EffectiveActivation;
use crate::domain::{ActivationId, BranchId, FrozenCandidate, ObservationPosition};
use crate::fit::{
    FitReport, PreparationFailure, PreparationFailureCode, PreparationFailureContext,
    PreparationRecord, PreparedDecision,
};
use crate::preparation::{PreparationOutcome, prepare_decision_input};
use crate::selection::SelectionOutcome;
use crate::source::{CommittedSourceObservation, SourceBatch, SourceEvent};

#[path = "engine_source_refresh.rs"]
mod source_refresh;

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
    Pin<Box<dyn Future<Output = Result<EvaluatedCandidate, EvaluationFailure>> + Send + 'a>>;

/// One bounded evaluation task returned to the coordinator for concurrent polling.
pub type EvaluationTask<'a> =
    Pin<Box<dyn Future<Output = Result<RunSettlement, StoreError>> + Send + 'a>>;

/// A model-backed evaluator for one already-frozen candidate.
pub trait DecisionEvaluator: Send + Sync {
    /// Validates and freezes provider input before durable dispatch intent.
    fn prepare(
        &self,
        _activation: &EffectiveActivation,
        input: Value,
    ) -> Result<PreparedDecision, PreparationFailure> {
        Ok(PreparedDecision {
            profile: "unbounded-v1".into(),
            input,
            request: None,
            report: FitReport::default(),
        })
    }

    /// Observes a new preparation after its current claim is saved.
    /// Saved retries and recovery do not call this hook.
    fn preparation_recorded(&self, _preparation: &PreparationRecord) {}

    /// Fences incompatible saved provider profiles before dispatch or recovery.
    fn validate_prepared(
        &self,
        _activation: &EffectiveActivation,
        _prepared: &PreparedDecision,
    ) -> Result<(), PreparationFailure> {
        Ok(())
    }

    /// Evaluates the exact persisted preparation. Simple adapters use its input.
    fn evaluate_prepared<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        candidate: &'a FrozenCandidate,
        prepared: &'a PreparedDecision,
        attempt_number: u8,
    ) -> DecisionFuture<'a> {
        self.evaluate(activation, candidate, &prepared.input, attempt_number)
    }

    /// Evaluates the compiled Choice with one durable, numbered provider attempt.
    ///
    /// Implementations must return promptly. Provider admission and transport
    /// waits belong inside the returned future so the coordinator can poll this
    /// task beside source pages, other evaluations, and cancellation.
    /// `attempt_number` is the persisted dispatch number, including after restart.
    /// The engine owns retries; each call must send at most one attempt.
    fn evaluate<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        candidate: &'a FrozenCandidate,
        prepared_input: &'a Value,
        attempt_number: u8,
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
    pub attempts: u8,
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
}

/// A bounded, thread-safe scheduler. Persistence remains the source of truth.
pub struct CheckEngine<S, E> {
    store: Arc<S>,
    evaluator: Arc<E>,
    state: Arc<Mutex<SchedulerState>>,
    selectors: Mutex<BTreeMap<ActivationId, SourceProgress>>,
    policy: ExecutionPolicy,
    maximum_attempts: u8,
}

struct InFlightGuard {
    state: Arc<Mutex<SchedulerState>>,
    key: ScheduleKey,
    finished: bool,
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
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
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
}

enum PreflightOutcome {
    Ready(PreparedDecision),
    Settled(RunSettlement),
    Stale,
}

impl<S: CheckStore, E: DecisionEvaluator> CheckEngine<S, E> {
    /// Creates a coordinator over the desktop Store and decision evaluator.
    #[must_use]
    pub fn new(store: Arc<S>, evaluator: Arc<E>) -> Self {
        Self::with_policy(store, evaluator, ExecutionPolicy::Live)
    }

    /// Creates a coordinator with an explicit scheduling policy.
    #[must_use]
    pub fn with_policy(store: Arc<S>, evaluator: Arc<E>, policy: ExecutionPolicy) -> Self {
        Self::with_policy_and_attempt_limit(store, evaluator, policy, 2)
    }

    /// Creates a coordinator with an explicit per-candidate attempt bound.
    #[must_use]
    pub fn with_policy_and_attempt_limit(
        store: Arc<S>,
        evaluator: Arc<E>,
        policy: ExecutionPolicy,
        maximum_attempts: u8,
    ) -> Self {
        Self {
            store,
            evaluator,
            state: Arc::new(Mutex::new(SchedulerState::default())),
            selectors: Mutex::new(BTreeMap::new()),
            policy,
            maximum_attempts,
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
        let mut invalidations = Vec::<(usize, crate::domain::BranchId, Option<String>)>::new();
        let mut reset_branches = BTreeSet::new();
        let mut applied_page = page.clone();
        for (page_index, event) in applied_page.events.iter_mut().enumerate() {
            match event {
                SourceEvent::Ignored { .. } => {}
                SourceEvent::Observation {
                    cursor,
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
                    let seen_key = (observation.branch.clone(), identity.clone());
                    if next
                        .selection
                        .seen_results
                        .get(&seen_key)
                        .is_some_and(|previous| {
                            *revision > previous.revision
                                || (*revision == previous.revision
                                    && observation.version != previous.version)
                        })
                    {
                        invalidations.push((
                            page_index,
                            observation.branch.clone(),
                            Some(identity.clone()),
                        ));
                    }
                    let selection = next.selection.apply_published(
                        activation,
                        identity,
                        *revision,
                        &observation,
                    );
                    let window_invalidated = selection == SelectionOutcome::WindowInvalidated;
                    match selection {
                        SelectionOutcome::Candidate(candidate) => {
                            candidates.push((*candidate, page_index))
                        }
                        SelectionOutcome::ConditionFiltered(condition) => {
                            condition_outcomes.push((position, condition))
                        }
                        SelectionOutcome::WindowInvalidated => {
                            invalidations.push((page_index, observation.branch.clone(), None));
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
                    invalidations.push((page_index, branch.clone(), Some(identity.clone())));
                }
                SourceEvent::Gap { branch, .. } => {
                    next.selection.mark_coverage_gap(branch);
                }
                SourceEvent::UnsafeUpdate { branch, .. }
                | SourceEvent::SegmentReset { branch, .. } => {
                    invalidations.push((page_index, branch.clone(), None));
                    reset_branches.insert(branch.clone());
                    next.selection.reset_segment(branch);
                }
                SourceEvent::CoverageRestored { branch, .. } => {
                    next.selection.restore_coverage(branch);
                }
            }
        }
        candidates.retain(|(candidate, candidate_index)| {
            !invalidations.iter().any(|(change_index, branch, identity)| {
                change_index > candidate_index
                    && branch == &candidate.branch
                    && identity.as_ref().is_none_or(|identity| {
                        candidate.dependencies.iter().any(|dependency| {
                            matches!(dependency, crate::domain::EvidenceDependency::Message { id, .. } if id == identity)
                        })
                    })
            })
        });
        let candidates = candidates
            .into_iter()
            .map(|(candidate, _)| candidate)
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
        };
        for candidate in candidates {
            match self.enqueue(activation, candidate.clone()) {
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

    /// Reloads durable pending work after restart in bounded pages.
    pub fn recover_pending(&self) -> Result<BatchOutcome, StoreError> {
        let mut outcome = BatchOutcome {
            observations: 0,
            candidates: 0,
            coalesced: 0,
            overflowed: 0,
            condition_outcomes: Vec::new(),
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
                let candidate = match record.work {
                    RecoveryWork::Pending(candidate) => candidate,
                    RecoveryWork::Answered { claimed, evaluated } => {
                        after = Some(claimed.id.clone());
                        evaluated
                            .validate(&activation.compiled)
                            .map_err(|error| StoreError {
                                message: error.message,
                            })?;
                        let actions = plan_status_actions(
                            &activation.compiled.actions,
                            &claimed.candidate.context.value,
                            &evaluated.choices,
                            claimed.prior_status,
                        )
                        .map_err(|error| StoreError {
                            message: error.message,
                        })?;
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
                match self.enqueue(&activation, candidate.clone()) {
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
    pub fn enqueue(
        &self,
        activation: &EffectiveActivation,
        candidate: FrozenCandidate,
    ) -> EnqueueOutcome {
        let key = ScheduleKey {
            activation: candidate.activation.clone(),
            branch: candidate.branch.clone(),
        };
        let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
        let coalesces = activation.compiled.trigger.window.is_some();
        let existing = state.pending.get(&key);
        if existing.is_some_and(|queue| queue.contains_key(&candidate.trigger_position)) {
            return EnqueueOutcome::Queued;
        }
        let replaces = coalesces && existing.is_some_and(|queue| !queue.is_empty());
        let needs_ticket = existing.is_none_or(BTreeMap::is_empty);
        if !replaces
            && state.pending.values().map(BTreeMap::len).sum::<usize>() >= MAX_PENDING_CANDIDATES
        {
            return EnqueueOutcome::Overflow;
        }
        let queue = state.pending.entry(key.clone()).or_default();
        if replaces {
            if queue
                .last_key_value()
                .is_some_and(|(position, _)| *position > candidate.trigger_position)
            {
                return EnqueueOutcome::Coalesced;
            }
            queue.clear();
        }
        queue.insert(
            candidate.trigger_position,
            PendingCandidate {
                candidate,
                compiled: Arc::clone(&activation.compiled),
            },
        );
        if needs_ticket {
            let ticket = state.next_service_ticket;
            state.next_service_ticket = state.next_service_ticket.saturating_add(1);
            state.service_tickets.insert(key.clone(), ticket);
        }
        if replaces {
            EnqueueOutcome::Coalesced
        } else {
            EnqueueOutcome::Queued
        }
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
        self.start_one_due_with_attempt_limit(activation, now_unix_ms, self.maximum_attempts)
    }

    /// Reserves one candidate with a caller-supplied remaining attempt budget.
    pub fn start_one_due_with_attempt_limit(
        &self,
        activation: &EffectiveActivation,
        now_unix_ms: u64,
        maximum_attempts: u8,
    ) -> Result<StartOutcome<'_>, StoreError> {
        self.start_one_due_matching(activation, now_unix_ms, maximum_attempts, |_| true)
    }

    /// Starts only branches whose startup attachment has committed.
    pub fn start_one_due_for_branches(
        &self,
        activation: &EffectiveActivation,
        now_unix_ms: u64,
        branches: &BTreeSet<BranchId>,
    ) -> Result<StartOutcome<'_>, StoreError> {
        self.start_one_due_matching(activation, now_unix_ms, self.maximum_attempts, |branch| {
            branches.contains(branch)
        })
    }

    fn start_one_due_matching(
        &self,
        activation: &EffectiveActivation,
        now_unix_ms: u64,
        maximum_attempts: u8,
        ready: impl Fn(&BranchId) -> bool,
    ) -> Result<StartOutcome<'_>, StoreError> {
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
        let prepared_input = match self.prepare_claimed(activation, &pending, &claimed) {
            Ok(PreflightOutcome::Ready(prepared)) => prepared,
            Ok(PreflightOutcome::Settled(settlement)) => {
                self.clear_ticket_if_idle(&key);
                let mut guard = guard;
                guard.finish();
                return Ok(StartOutcome::Started(Box::pin(
                    async move { Ok(settlement) },
                )));
            }
            Ok(PreflightOutcome::Stale) => {
                let mut guard = guard;
                guard.finish();
                return Ok(StartOutcome::Stale);
            }
            Err(error) => {
                return Err(self.restore_pre_dispatch(key, pending, claimed, guard, error));
            }
        };
        let cooldown = if self.policy == ExecutionPolicy::SequentialReplay {
            std::time::Duration::ZERO
        } else {
            pending.compiled.trigger.cooldown
        };
        match self.store.record_dispatch_intent(
            &claimed,
            now_unix_ms,
            cooldown,
            claimed.next_attempt,
        ) {
            Ok(ClaimOutcome::Current) => {}
            Ok(ClaimOutcome::Stale) => {
                self.clear_ticket_if_idle(&key);
                let mut guard = guard;
                guard.finish();
                return Ok(StartOutcome::Stale);
            }
            Err(error) => {
                return Err(self.restore_pre_dispatch(key, pending, claimed, guard, error));
            }
        }
        {
            let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
            if state
                .pending
                .get(&key)
                .is_some_and(|queue| !queue.is_empty())
            {
                let ticket = state.next_service_ticket;
                state.next_service_ticket = state.next_service_ticket.saturating_add(1);
                state.service_tickets.insert(key.clone(), ticket);
            } else {
                state.service_tickets.remove(&key);
            }
            if self.policy == ExecutionPolicy::Live {
                state.recent_dispatches = if state.recent_dispatches >= 3 {
                    0
                } else {
                    state.recent_dispatches + 1
                };
            }
        }
        {
            let mut state = self.state.lock().expect("scheduler mutex is not poisoned");
            let cooldown_ms = u64::try_from(cooldown.as_millis()).unwrap_or(u64::MAX);
            state
                .cooldown_until_unix_ms
                .insert(key.clone(), now_unix_ms.saturating_add(cooldown_ms));
        }
        let evaluator = Arc::clone(&self.evaluator);
        let store = Arc::clone(&self.store);
        let activation = activation.clone();
        let task: EvaluationTask<'_> = Box::pin(async move {
            let mut guard = guard;
            let mut attempt_number = claimed.next_attempt;
            let evaluated = loop {
                match evaluator
                    .evaluate_prepared(
                        &activation,
                        &pending.candidate,
                        &prepared_input,
                        attempt_number,
                    )
                    .await
                {
                    Ok(mut evaluated) => {
                        evaluated.attempts = attempt_number;
                        break Ok(evaluated);
                    }
                    Err(mut error) => {
                        if store.record_attempt_failure(
                            &claimed,
                            attempt_number,
                            error.kind,
                            &EvaluationDiagnostic {
                                code: error.code,
                                detail: error.detail.clone(),
                            },
                        )? == ClaimOutcome::Stale
                        {
                            guard.finish();
                            return Ok(RunSettlement {
                                outcome: RunOutcome::Stale,
                                attempts: attempt_number,
                            });
                        }
                        if error.kind == AttemptFailure::SafeTransient
                            && attempt_number < maximum_attempts.min(2)
                        {
                            attempt_number += 1;
                            if store.record_dispatch_intent(
                                &claimed,
                                now_unix_ms,
                                cooldown,
                                attempt_number,
                            )? == ClaimOutcome::Stale
                            {
                                guard.finish();
                                return Ok(RunSettlement {
                                    outcome: RunOutcome::Stale,
                                    attempts: attempt_number - 1,
                                });
                            }
                        } else {
                            error.attempts = attempt_number;
                            break Err(error);
                        }
                    }
                }
            };
            let outcome = match evaluated {
                Ok(evaluated) => {
                    let attempts = evaluated.attempts;
                    if let Err(error) = evaluated.validate(&pending.compiled) {
                        let recorded = store.record_attempt_failure(
                            &claimed,
                            attempts,
                            AttemptFailure::Permanent,
                            &EvaluationDiagnostic {
                                code: EvaluationFailureCode::InvalidDecision,
                                detail: error.message.clone(),
                            },
                        )?;
                        let terminal = store.record_evaluation_failure(
                            &claimed,
                            &EvaluationDiagnostic {
                                code: EvaluationFailureCode::InvalidDecision,
                                detail: error.message.clone(),
                            },
                        )?;
                        guard.finish();
                        return Ok(RunSettlement {
                            outcome: if recorded == ClaimOutcome::Stale
                                || terminal == ClaimOutcome::Stale
                            {
                                RunOutcome::Stale
                            } else {
                                RunOutcome::Failed(error.message)
                            },
                            attempts,
                        });
                    }
                    store.record_answer(&claimed, &evaluated)?;
                    match plan_status_actions(
                        &pending.compiled.actions,
                        &pending.candidate.context.value,
                        &evaluated.choices,
                        claimed.prior_status,
                    ) {
                        Ok(actions) => {
                            let accepted = store.accept(&AssessmentAcceptance {
                                claimed,
                                evaluated,
                                actions,
                            })?;
                            RunSettlement {
                                outcome: match accepted {
                                    AcceptanceOutcome::Accepted
                                    | AcceptanceOutcome::AlreadyAccepted => RunOutcome::Accepted,
                                    AcceptanceOutcome::Deferred => RunOutcome::Deferred,
                                    AcceptanceOutcome::Stale => RunOutcome::Stale,
                                },
                                attempts,
                            }
                        }
                        Err(error) => {
                            let recorded = store.record_evaluation_failure(
                                &claimed,
                                &EvaluationDiagnostic {
                                    code: EvaluationFailureCode::ActionFailed,
                                    detail: error.message.clone(),
                                },
                            )?;
                            RunSettlement {
                                outcome: if recorded == ClaimOutcome::Stale {
                                    RunOutcome::Stale
                                } else {
                                    RunOutcome::Failed(error.message)
                                },
                                attempts,
                            }
                        }
                    }
                }
                Err(error) => {
                    let recorded = store.record_evaluation_failure(
                        &claimed,
                        &EvaluationDiagnostic {
                            code: error.code,
                            detail: error.detail.clone(),
                        },
                    )?;
                    RunSettlement {
                        outcome: if recorded == ClaimOutcome::Stale {
                            RunOutcome::Stale
                        } else {
                            RunOutcome::Failed(error.detail)
                        },
                        attempts: error.attempts,
                    }
                }
            };
            guard.finish();
            Ok(outcome)
        });
        Ok(StartOutcome::Started(task))
    }

    fn prepare_claimed(
        &self,
        activation: &EffectiveActivation,
        pending: &PendingCandidate,
        claimed: &ClaimedCandidate,
    ) -> Result<PreflightOutcome, StoreError> {
        let context = PreparationFailureContext {
            decision: Some(pending.compiled.decision.name.clone()),
            field: Some(format!("decisions.{}", pending.compiled.decision.name)),
            ..Default::default()
        };
        let preparation = if let Some(saved) = self.store.load_preparation(claimed)? {
            saved
        } else if claimed.next_attempt > 1 {
            PreparationRecord::Rejected(PreparationFailure {
                code: PreparationFailureCode::IncompatibleProfile,
                detail: "The original provider request is unavailable for this retry.".into(),
                report: None,
                context: context.clone().into(),
            })
        } else {
            let prepared = match prepare_decision_input(&pending.compiled, &pending.candidate) {
                PreparationOutcome::Ready(input) => self.evaluator.prepare(activation, input),
                PreparationOutcome::Skipped => Err(PreparationFailure {
                    code: PreparationFailureCode::ConditionFalse,
                    detail: "condition was false".into(),
                    report: None,
                    context: context.clone().into(),
                }),
                PreparationOutcome::Unavailable(detail) => Err(PreparationFailure {
                    code: PreparationFailureCode::InputUnavailable,
                    detail,
                    report: None,
                    context: context.clone().into(),
                }),
                PreparationOutcome::Failed(detail) => Err(PreparationFailure {
                    code: PreparationFailureCode::InvalidInput,
                    detail,
                    report: None,
                    context: context.into(),
                }),
            };
            let record = match prepared {
                Ok(prepared) => PreparationRecord::Ready(prepared),
                Err(error) => PreparationRecord::Rejected(error),
            };
            if self.store.record_preparation(claimed, &record)? == ClaimOutcome::Stale {
                return Ok(PreflightOutcome::Stale);
            }
            self.evaluator.preparation_recorded(&record);
            record
        };
        let prepared = match preparation {
            PreparationRecord::Ready(prepared) => self
                .evaluator
                .validate_prepared(activation, &prepared)
                .map(|()| prepared),
            PreparationRecord::Rejected(error) => Err(error),
        };
        match prepared {
            Ok(prepared) => Ok(PreflightOutcome::Ready(prepared)),
            Err(error) => {
                let recorded = self.store.record_preparation_rejection(claimed, &error)?;
                Ok(PreflightOutcome::Settled(RunSettlement {
                    outcome: if recorded == ClaimOutcome::Stale {
                        RunOutcome::Stale
                    } else if error.code == PreparationFailureCode::ConditionFalse {
                        RunOutcome::Skipped
                    } else {
                        RunOutcome::NotAssessed(error.detail)
                    },
                    attempts: claimed.next_attempt - 1,
                }))
            }
        }
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

fn restore_pending(state: &mut SchedulerState, key: ScheduleKey, pending: PendingCandidate) {
    let queue = state.pending.entry(key).or_default();
    if pending.compiled.trigger.window.is_some() && !queue.is_empty() {
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
    if page.next != previous {
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
    mod selection_live;

    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::actions::ChoiceResult;
    use crate::check_store::{CandidateClaim, CandidateId, ClaimedCandidate};
    use crate::compiler::compile;
    use crate::definition_lifecycle::EffectiveActivation;
    use crate::domain::{
        EvidenceDependency, EvidenceVersion, FrozenContext, Generation, Message, NativeOrderKey,
        ObservationDraft, ObservationKind, ObservationPosition,
    };
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
            }
        }
    }

    impl CheckStore for FakeStore {
        fn release_pre_dispatch_claim(
            &self,
            _: &ClaimedCandidate,
        ) -> Result<ClaimOutcome, StoreError> {
            Ok(ClaimOutcome::Current)
        }
        fn activate(
            &self,
            _: &str,
            _: &crate::definition_lifecycle::LifecycleTransition,
            _: Option<&EffectiveActivation>,
        ) -> Result<(), StoreError> {
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

        fn record_overflow(&self, _: &FrozenCandidate) -> Result<(), StoreError> {
            Ok(())
        }

        fn claim_due(
            &self,
            candidate: &FrozenCandidate,
            now_unix_ms: u64,
            bypass_check_cooldown: bool,
        ) -> Result<CandidateClaim, StoreError> {
            let retry_at = *self.cooldown_until_unix_ms.lock().unwrap();
            if !bypass_check_cooldown && now_unix_ms < retry_at {
                return Ok(CandidateClaim::NotDue {
                    retry_at_unix_ms: retry_at,
                });
            }
            Ok(CandidateClaim::Claimed(Box::new(ClaimedCandidate {
                fence: 1,
                next_attempt: 1,
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
            _: u8,
        ) -> Result<ClaimOutcome, StoreError> {
            *self.dispatches.lock().unwrap() += 1;
            *self.cooldown_until_unix_ms.lock().unwrap() = attempted_at_unix_ms
                .saturating_add(u64::try_from(cooldown.as_millis()).unwrap_or(u64::MAX));
            self.pending
                .lock()
                .unwrap()
                .retain(|record| record.candidate.fingerprint != claimed.candidate.fingerprint);
            Ok(ClaimOutcome::Current)
        }

        fn record_answer(
            &self,
            _: &ClaimedCandidate,
            _: &EvaluatedCandidate,
        ) -> Result<(), StoreError> {
            Ok(())
        }

        fn accept(
            &self,
            assessment: &AssessmentAcceptance,
        ) -> Result<AcceptanceOutcome, StoreError> {
            self.accepted.lock().unwrap().push(assessment.clone());
            Ok(AcceptanceOutcome::Accepted)
        }

        fn load_recovery(
            &self,
            after: Option<&CandidateId>,
            limit: usize,
        ) -> Result<Vec<crate::check_store::RecoveryRecord>, StoreError> {
            let mut records = self.pending.lock().unwrap().clone();
            records.sort_by_key(|record| CandidateId::from(&record.candidate));
            Ok(records
                .into_iter()
                .filter(|record| {
                    after.is_none_or(|after| CandidateId::from(&record.candidate) > *after)
                })
                .take(limit)
                .map(|record| crate::check_store::RecoveryRecord {
                    activation: record.activation,
                    work: RecoveryWork::Pending(record.candidate),
                })
                .collect())
        }

        fn record_attempt_failure(
            &self,
            _: &ClaimedCandidate,
            _: u8,
            _: AttemptFailure,
            _: &EvaluationDiagnostic,
        ) -> Result<ClaimOutcome, StoreError> {
            Ok(ClaimOutcome::Current)
        }

        fn record_failure(
            &self,
            _: &ClaimedCandidate,
            _: &str,
        ) -> Result<ClaimOutcome, StoreError> {
            Ok(ClaimOutcome::Current)
        }

        fn record_not_assessed(
            &self,
            _: &ClaimedCandidate,
            _: &str,
        ) -> Result<ClaimOutcome, StoreError> {
            Ok(ClaimOutcome::Current)
        }
    }

    struct FakeEvaluator;

    impl DecisionEvaluator for FakeEvaluator {
        fn evaluate<'a>(
            &'a self,
            _: &'a EffectiveActivation,
            _: &'a FrozenCandidate,
            _: &'a Value,
            _: u8,
        ) -> DecisionFuture<'a> {
            Box::pin(async {
                let probabilities =
                    BTreeMap::from([("yes".to_owned(), 0.9), ("no".to_owned(), 0.1)]);
                let choice = ChoiceResult::validate(
                    "review",
                    "verdict",
                    &["yes".to_owned(), "no".to_owned()],
                    "yes",
                    probabilities,
                )
                .map_err(|error| EvaluationFailure {
                    code: EvaluationFailureCode::InvalidDecision,
                    kind: AttemptFailure::Permanent,
                    detail: error.message,
                    attempts: 0,
                })?;
                Ok(EvaluatedCandidate {
                    choices: vec![choice],
                    attempts: 1,
                })
            })
        }
    }

    struct PendingEvaluator {
        calls: AtomicUsize,
    }

    impl DecisionEvaluator for PendingEvaluator {
        fn evaluate<'a>(
            &'a self,
            _: &'a EffectiveActivation,
            _: &'a FrozenCandidate,
            _: &'a Value,
            _: u8,
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
                compile("name: Retry\non:\n  tool_result:\n  cooldown: 10s\ndecisions:\n  review:\n    model: typesafe/jev-1.13.0\n    input: {activity: \"${{ message.tool.result.status }}\"}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    input: {result: failure}\n").unwrap(),
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
                compile("name: Retry\non:\n  tool_result:\n  window: {size: 2, min: 2}\ndecisions:\n  review:\n    model: typesafe/jev-1.13.0\n    input: {activity: \"${{ window.messages }}\"}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    input: {result: failure}\n").unwrap(),
            ),
            ..activation()
        }
    }

    fn source_event(cursor: u64, identity: &str) -> SourceEvent {
        source_event_revision(cursor, identity, 1)
    }

    fn source_event_revision(cursor: u64, identity: &str, revision: u64) -> SourceEvent {
        SourceEvent::Observation {
            cursor: PublicationCursor(cursor),
            identity: identity.to_owned(),
            revision,
            observation: Box::new(ObservationDraft {
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
        match task.as_mut().poll(&mut context) {
            Poll::Ready(Ok(outcome)) => outcome,
            Poll::Ready(Err(error)) => panic!("evaluation task failed: {}", error.message),
            Poll::Pending => panic!("immediate evaluator must finish in one poll"),
        }
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
            engine.enqueue(&activation, candidate(&activation, 1)),
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
            engine.enqueue(&activation, candidate(&activation, 2)),
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
            engine.enqueue(&activation, candidate(&activation, 3)),
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
                engine.enqueue(
                    &activation,
                    candidate_on_branch(&activation, trigger, branch)
                ),
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
                    engine.enqueue(
                        &activation,
                        candidate_on_branch(&activation, trigger, branch)
                    ),
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
        engine.enqueue(&activation, candidate(&activation, 1));
        let StartOutcome::Started(mut task) = engine.start_one_due(&activation, 10_000).unwrap()
        else {
            panic!("candidate must start");
        };
        assert_eq!(evaluator.calls.load(Ordering::SeqCst), 0);
        let mut context = Context::from_waker(Waker::noop());
        assert!(task.as_mut().poll(&mut context).is_pending());
        assert_eq!(evaluator.calls.load(Ordering::SeqCst), 1);
        engine.enqueue(&activation, candidate(&activation, 2));
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
        first_engine.enqueue(&activation, candidate(&activation, 1));
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
