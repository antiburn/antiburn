//! Sequential replay through the shared check engine.

use std::sync::Arc;

use crate::check_store::{CheckStore, StoreError};
use crate::definition_lifecycle::EffectiveActivation;
use crate::engine::{CheckEngine, DecisionEvaluator, ExecutionPolicy, RunOutcome, StartOutcome};
use crate::source::{PublicationCursor, SourceBatch, SourceEvent};

/// Stable caller-assigned identity for one isolated replay run.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ReplayRunId(pub String);

/// A finite ordered source that distinguishes end-of-input from an empty live page.
pub trait ReplayEventSource {
    /// Returns the next committed event, or `None` only after finite input is exhausted.
    fn next_event(&mut self) -> Result<Option<SourceEvent>, String>;
}

/// Hard bounds for one replay invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayLimits {
    /// Maximum number of applied source events. One extra read can probe for EOF.
    pub events: usize,
    /// Maximum number of engine evaluations.
    pub evaluations: usize,
    /// Maximum provider attempts, including the one known-safe transient retry.
    pub attempts: usize,
}

/// Terminal state of the finite replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayEnd {
    /// The source reported finite end-of-input and pending work was drained.
    Complete,
    /// A configured event, evaluation, or attempt limit was reached.
    LimitReached,
    /// Cancellation was observed between source events.
    Cancelled,
    /// The source failed before finite end-of-input.
    SourceError(String),
    /// The shared Store or engine failed.
    ExecutionError(String),
}

/// One applied event and its optional settled engine outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayStep {
    /// Source publication cursor for this event.
    pub cursor: PublicationCursor,
    /// Number of candidates created by this event.
    pub candidates: usize,
    /// Settled outcome when this event created a candidate.
    pub outcome: Option<RunOutcome>,
}

/// Bounded semantic result for one isolated replay run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayReport {
    /// Caller-assigned isolated run identity.
    pub run_id: ReplayRunId,
    /// Logical definition identity used for this run.
    pub definition_id: String,
    /// Content revision of the compiled definition.
    pub definition_revision: String,
    /// Hard bounds applied to this invocation.
    pub limits: ReplayLimits,
    /// Terminal replay state.
    pub end: ReplayEnd,
    /// Source events successfully applied before termination.
    pub events: usize,
    /// Engine evaluations started.
    pub evaluations: usize,
    /// Provider attempts consumed by this runner.
    pub attempts: usize,
    /// Bounded event-level diagnostics.
    pub steps: Vec<ReplayStep>,
}

/// Replays one finite stream through the same selector, evaluator, actions, and Store port.
///
/// The runner consumes one event, applies it, and settles any resulting
/// candidate before asking the source for the next event. This prevents a
/// later request, correction, or gap from changing an earlier trigger's
/// frozen context. The caller must provide a replay-only Store instance; this
/// function never accepts or mutates live scheduling policy.
/// At the event bound, one source read distinguishes EOF from additional input.
/// The runner does not apply or evaluate that extra event.
///
/// # Errors
/// Returns a Store error if creating the replay engine fails before a report
/// can be produced. Runtime source and engine errors are retained in the report.
pub async fn replay<S, E, R>(
    run_id: ReplayRunId,
    mut activation: EffectiveActivation,
    store: Arc<S>,
    evaluator: Arc<E>,
    source: &mut R,
    limits: ReplayLimits,
    is_cancelled: impl Fn() -> bool,
) -> Result<ReplayReport, StoreError>
where
    S: CheckStore + 'static,
    E: DecisionEvaluator,
    R: ReplayEventSource,
{
    if run_id.0.trim().is_empty() {
        return Err(StoreError {
            message: "replay run ID must not be empty".to_owned(),
        });
    }
    activation.id.scope.run = crate::domain::RunScope::Replay(run_id.0.clone());
    let definition_id = activation.id.definition_id.clone();
    let definition_revision = activation
        .revision
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    activation.bootstrap_observation_position = crate::domain::ObservationPosition(0);
    activation.bootstrap_publication = PublicationCursor(0);
    let engine = CheckEngine::with_policy(store, evaluator, ExecutionPolicy::SequentialReplay);
    let mut report = ReplayReport {
        run_id,
        definition_id,
        definition_revision,
        limits,
        end: ReplayEnd::Complete,
        events: 0,
        evaluations: 0,
        attempts: 0,
        steps: Vec::new(),
    };
    engine.persist_activation(
        &activation.id.definition_id,
        &crate::definition_lifecycle::LifecycleTransition::Activated(activation.clone()),
        Some(&activation),
    )?;

    loop {
        if is_cancelled() {
            report.end = ReplayEnd::Cancelled;
            break;
        }
        let event = match source.next_event() {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(error) => {
                report.end = ReplayEnd::SourceError(error);
                break;
            }
        };
        if report.events >= limits.events {
            report.end = ReplayEnd::LimitReached;
            break;
        }
        let cursor = event_cursor(&event);
        let page = SourceBatch {
            events: vec![event],
            next: cursor,
            has_more: true,
        };
        let applied = match engine.apply_source_page(&activation, &page) {
            Ok(applied) => applied,
            Err(error) => {
                report.end = ReplayEnd::ExecutionError(error.message);
                break;
            }
        };
        report.events += 1;
        let mut outcome = None;
        if applied.candidates > 0 {
            if report.evaluations >= limits.evaluations {
                report.end = ReplayEnd::LimitReached;
                report.steps.push(ReplayStep {
                    cursor,
                    candidates: applied.candidates,
                    outcome: None,
                });
                break;
            }
            let remaining_attempts = limits.attempts.saturating_sub(report.attempts);
            let maximum_attempts = u64::try_from(remaining_attempts).map_err(|_| StoreError {
                message: "replay attempt allowance is too large".into(),
            })?;
            match engine.start_one_due_with_attempt_limit(&activation, 0, maximum_attempts) {
                Ok(StartOutcome::Started(task)) => {
                    report.evaluations += 1;
                    match task.await {
                        Ok(settlement) => {
                            report.attempts = report
                                .attempts
                                .checked_add(usize::try_from(settlement.new_intents).map_err(
                                    |_| StoreError {
                                        message: "replay attempt total is too large".into(),
                                    },
                                )?)
                                .ok_or_else(|| StoreError {
                                    message: "replay attempt total is exhausted".into(),
                                })?;
                            match &settlement.outcome {
                                RunOutcome::Failed(detail) => {
                                    report.end = ReplayEnd::ExecutionError(detail.clone());
                                }
                                RunOutcome::Deferred => {
                                    report.end = ReplayEnd::ExecutionError(
                                        "replay answer acceptance is deferred".to_owned(),
                                    );
                                }
                                RunOutcome::Stale => {
                                    report.end = ReplayEnd::ExecutionError(
                                        "replay answer lost publication rights".to_owned(),
                                    );
                                }
                                RunOutcome::LimitReached => report.end = ReplayEnd::LimitReached,
                                _ => {}
                            }
                            outcome = Some(settlement.outcome);
                        }
                        Err(error) => {
                            report.attempts = report
                                .attempts
                                .checked_add(usize::try_from(error.new_intents).map_err(|_| {
                                    StoreError {
                                        message: "replay attempt total is too large".into(),
                                    }
                                })?)
                                .ok_or_else(|| StoreError {
                                    message: "replay attempt total is exhausted".into(),
                                })?;
                            report.end =
                                ReplayEnd::ExecutionError(if error.intent_commit_uncertain {
                                    format!(
                                        "{}; the last dispatch intent commit is uncertain",
                                        error.source.message
                                    )
                                } else {
                                    error.source.message
                                });
                        }
                    }
                }
                Ok(StartOutcome::NotDue | StartOutcome::Stale) => {
                    report.end = ReplayEnd::ExecutionError(
                        "sequential replay candidate was not dispatched".to_owned(),
                    );
                }
                Err(error) => report.end = ReplayEnd::ExecutionError(error.message),
            }
        }
        report.steps.push(ReplayStep {
            cursor,
            candidates: applied.candidates,
            outcome,
        });
        if !matches!(report.end, ReplayEnd::Complete) {
            break;
        }
    }
    Ok(report)
}

fn event_cursor(event: &SourceEvent) -> PublicationCursor {
    match event {
        SourceEvent::Ignored { cursor }
        | SourceEvent::Observation { cursor, .. }
        | SourceEvent::Removal { cursor, .. }
        | SourceEvent::Gap { cursor, .. }
        | SourceEvent::UnsafeUpdate { cursor, .. }
        | SourceEvent::SegmentReset { cursor, .. }
        | SourceEvent::CoverageRestored { cursor, .. } => *cursor,
    }
}

#[cfg(test)]
mod tests {
    mod label_only;
    mod local;
    mod multi_decision;
    mod public_events;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::task::{Context, Poll, Waker};

    use serde_json::json;

    use super::*;
    use crate::check_store::{
        AcceptanceOutcome, AssessmentAcceptance, AttemptFailure, CandidateClaim, CandidateId,
        ClaimedCandidate, DecisionAttemptOutcome, DecisionProgress, RecoveryRecord,
        SavedDecisionAttempt, SourceProgress,
    };
    use crate::decision::{DecisionAttempt, DecisionInput};
    use crate::definition_lifecycle::EffectiveActivation;
    use crate::domain::{
        ActivationId, BranchId, EvidenceVersion, FrozenContext, Generation, Message,
        NativeOrderKey, ObservationDraft, ObservationKind, ObservationPosition,
    };
    use crate::engine::PreparationValidationPurpose;
    use crate::engine::test_support::{test_answer, test_prepare, test_validate};
    use crate::engine::{DecisionFuture, EvaluationFailure};
    use crate::expression_eval::EvaluationBudget;
    use crate::fit::PreparationFailure;
    use crate::prepared_assessment::PreparedAssessment;
    use crate::results::{DecisionResult, EvaluatedAssessment};
    use crate::source::CommittedSourceObservation;

    #[derive(Default)]
    struct MemoryStore {
        progress: Mutex<SourceProgress>,
        accepted: Mutex<Vec<AssessmentAcceptance>>,
        answers: Mutex<Vec<EvaluatedAssessment>>,
        activations: Mutex<Vec<EffectiveActivation>>,
        dispatches: Mutex<Vec<u8>>,
        failures: Mutex<Vec<AttemptFailure>>,
        evaluation_failures: Mutex<Vec<crate::engine::EvaluationDiagnostic>>,
        acceptance: Mutex<Option<AcceptanceOutcome>>,
        recovery: Mutex<Vec<RecoveryRecord>>,
        preparations: Mutex<BTreeMap<CandidateId, crate::fit::PreparationRecord>>,
        local_starts: Mutex<Vec<(u64, std::time::Duration)>>,
        attempt_rows: Mutex<BTreeMap<CandidateId, DecisionProgress>>,
        fail_intent_on_call: Mutex<Option<usize>>,
        fail_accept: Mutex<bool>,
    }

    impl CheckStore for MemoryStore {
        fn release_pre_dispatch_claim(
            &self,
            _: &ClaimedCandidate,
        ) -> Result<crate::check_store::ClaimOutcome, StoreError> {
            Ok(crate::check_store::ClaimOutcome::Current)
        }
        fn activate(
            &self,
            _: &str,
            _: &crate::definition_lifecycle::LifecycleTransition,
            activation: Option<&EffectiveActivation>,
        ) -> Result<(), StoreError> {
            if let Some(activation) = activation {
                self.activations.lock().unwrap().push(activation.clone());
            }
            Ok(())
        }

        fn load_source_progress(
            &self,
            _: &EffectiveActivation,
        ) -> Result<SourceProgress, StoreError> {
            Ok(self.progress.lock().unwrap().clone())
        }

        fn apply_source_page(
            &self,
            _: &EffectiveActivation,
            expected: crate::check_store::SourceProgressHead,
            page: &SourceBatch,
            _: &[CommittedSourceObservation],
            next: &SourceProgress,
            _: &[crate::domain::FrozenCandidate],
        ) -> Result<(), StoreError> {
            let mut progress = self.progress.lock().unwrap();
            if progress.head() != expected || next.publication_cursor != page.next {
                return Err(StoreError {
                    message: "progress conflict".into(),
                });
            }
            *progress = next.clone();
            Ok(())
        }

        fn record_overflow(&self, _: &crate::domain::FrozenCandidate) -> Result<(), StoreError> {
            Ok(())
        }

        fn load_recovery(
            &self,
            after: Option<&CandidateId>,
            limit: usize,
        ) -> Result<Vec<RecoveryRecord>, StoreError> {
            fn candidate_id(record: &RecoveryRecord) -> CandidateId {
                match &record.work {
                    crate::check_store::RecoveryWork::Pending { candidate, .. } => {
                        CandidateId::from(candidate)
                    }
                    crate::check_store::RecoveryWork::Answered { claimed, .. } => {
                        claimed.id.clone()
                    }
                }
            }
            let mut records = self.recovery.lock().unwrap().clone();
            records.sort_by_key(candidate_id);
            Ok(records
                .into_iter()
                .filter(|record| after.is_none_or(|after| candidate_id(record) > *after))
                .take(limit)
                .collect())
        }

        fn load_terminal_candidate_ids(
            &self,
            candidate_ids: &[CandidateId],
        ) -> Result<Vec<CandidateId>, StoreError> {
            let accepted = self.accepted.lock().unwrap();
            Ok(candidate_ids
                .iter()
                .filter(|id| {
                    accepted
                        .iter()
                        .any(|assessment| assessment.claimed.id == **id)
                })
                .cloned()
                .collect())
        }

        fn claim_due(
            &self,
            candidate: &crate::domain::FrozenCandidate,
            _: u64,
            _: bool,
        ) -> Result<CandidateClaim, StoreError> {
            Ok(CandidateClaim::Claimed(Box::new(ClaimedCandidate {
                id: CandidateId::from(candidate),
                candidate: candidate.clone(),
                prior_status: self
                    .accepted
                    .lock()
                    .unwrap()
                    .iter()
                    .rev()
                    .find(|assessment| {
                        assessment.claimed.candidate.branch == candidate.branch
                            && assessment.claimed.candidate.activation == candidate.activation
                    })
                    .and_then(|assessment| assessment.actions.current_status),
                fence: 1,
            })))
        }

        fn record_dispatch_intent(
            &self,
            claimed: &ClaimedCandidate,
            _: u64,
            _: std::time::Duration,
            attempt: &DecisionAttempt,
        ) -> Result<crate::check_store::ClaimOutcome, StoreError> {
            self.dispatches.lock().unwrap().push(attempt.number);
            if *self.fail_intent_on_call.lock().unwrap()
                == Some(self.dispatches.lock().unwrap().len())
            {
                return Err(StoreError {
                    message: "injected dispatch intent write failure".into(),
                });
            }
            self.attempt_rows
                .lock()
                .unwrap()
                .entry(claimed.id.clone())
                .or_default()
                .attempts
                .push(SavedDecisionAttempt {
                    attempt: attempt.clone(),
                    outcome: DecisionAttemptOutcome::DispatchIntent,
                });
            Ok(crate::check_store::ClaimOutcome::Current)
        }

        fn load_preparation(
            &self,
            claimed: &ClaimedCandidate,
        ) -> Result<Option<crate::fit::PreparationRecord>, StoreError> {
            Ok(self.preparations.lock().unwrap().get(&claimed.id).cloned())
        }

        fn load_decision_progress(
            &self,
            claimed: &ClaimedCandidate,
        ) -> Result<DecisionProgress, StoreError> {
            Ok(self
                .attempt_rows
                .lock()
                .unwrap()
                .get(&claimed.id)
                .cloned()
                .unwrap_or_default())
        }

        fn record_preparation(
            &self,
            claimed: &ClaimedCandidate,
            record: &crate::fit::PreparationRecord,
        ) -> Result<crate::check_store::ClaimOutcome, StoreError> {
            self.preparations
                .lock()
                .unwrap()
                .insert(claimed.id.clone(), record.clone());
            Ok(crate::check_store::ClaimOutcome::Current)
        }

        fn record_local_start(
            &self,
            claimed: &ClaimedCandidate,
            now_unix_ms: u64,
            cooldown: std::time::Duration,
            preparation: &crate::fit::LocalPreparation,
        ) -> Result<crate::check_store::ClaimOutcome, StoreError> {
            if self
                .accepted
                .lock()
                .unwrap()
                .iter()
                .any(|saved| saved.claimed.id == claimed.id)
            {
                return Ok(crate::check_store::ClaimOutcome::Stale);
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
                .iter()
                .find(|activation| activation.id == claimed.candidate.activation)
                .cloned()
                .expect("registered activation");
            assert!(activation.compiled.is_local());
            let record = crate::fit::PreparationRecord::Local(preparation.clone());
            let mut preparations = self.preparations.lock().unwrap();
            if let Some(saved) = preparations.get(&claimed.id) {
                assert_eq!(saved, &record);
                return Ok(crate::check_store::ClaimOutcome::Current);
            }
            assert_eq!(preparation.started_at_unix_ms, now_unix_ms);
            let evaluated = EvaluatedAssessment::local();
            preparations.insert(claimed.id.clone(), record);
            self.answers.lock().unwrap().push(evaluated.clone());
            self.local_starts
                .lock()
                .unwrap()
                .push((now_unix_ms, cooldown));
            self.recovery.lock().unwrap().push(RecoveryRecord {
                activation,
                work: crate::check_store::RecoveryWork::Answered {
                    claimed: claimed.clone(),
                    evaluated,
                },
            });
            Ok(crate::check_store::ClaimOutcome::Current)
        }

        fn record_attempt_failure(
            &self,
            claimed: &ClaimedCandidate,
            attempt: &DecisionAttempt,
            outcome: AttemptFailure,
            _: &crate::engine::EvaluationDiagnostic,
        ) -> Result<crate::check_store::ClaimOutcome, StoreError> {
            self.failures.lock().unwrap().push(outcome);
            self.attempt_rows
                .lock()
                .unwrap()
                .get_mut(&claimed.id)
                .unwrap()
                .attempts
                .iter_mut()
                .find(|row| row.attempt == *attempt)
                .unwrap()
                .outcome = match outcome {
                AttemptFailure::SafeTransient => DecisionAttemptOutcome::SafeTransient,
                AttemptFailure::Unknown => DecisionAttemptOutcome::Unknown,
                AttemptFailure::Permanent => DecisionAttemptOutcome::Permanent,
            };
            Ok(crate::check_store::ClaimOutcome::Current)
        }

        fn record_decision_answer(
            &self,
            claimed: &ClaimedCandidate,
            attempt: &DecisionAttempt,
            evaluated: &DecisionResult,
        ) -> Result<crate::check_store::ClaimOutcome, StoreError> {
            let mut rows = self.attempt_rows.lock().unwrap();
            let progress = rows.get_mut(&claimed.id).unwrap();
            progress
                .attempts
                .iter_mut()
                .find(|row| row.attempt == *attempt)
                .unwrap()
                .outcome = DecisionAttemptOutcome::Answer(evaluated.clone());
            let activation = self
                .activations
                .lock()
                .unwrap()
                .iter()
                .find(|activation| activation.id == claimed.candidate.activation)
                .cloned()
                .unwrap();
            let checked = crate::check_store::validate_decision_progress(
                &activation.compiled.decisions,
                &activation.compiled.question_requirements(),
                progress,
            )?;
            if let crate::check_store::DecisionProgressState::Complete(evaluated) = checked.state {
                self.answers.lock().unwrap().push(evaluated);
            }
            Ok(crate::check_store::ClaimOutcome::Current)
        }

        fn accept(
            &self,
            assessment: &AssessmentAcceptance,
        ) -> Result<AcceptanceOutcome, StoreError> {
            if *self.fail_accept.lock().unwrap() {
                return Err(StoreError {
                    message: "injected acceptance failure".into(),
                });
            }
            if let Some(outcome) = *self.acceptance.lock().unwrap() {
                return Ok(outcome);
            }
            self.accepted.lock().unwrap().push(assessment.clone());
            Ok(AcceptanceOutcome::Accepted)
        }

        fn record_failure(
            &self,
            _: &ClaimedCandidate,
            _: &str,
        ) -> Result<crate::check_store::ClaimOutcome, StoreError> {
            Ok(crate::check_store::ClaimOutcome::Current)
        }

        fn record_evaluation_failure(
            &self,
            _: &ClaimedCandidate,
            diagnostic: &crate::engine::EvaluationDiagnostic,
        ) -> Result<crate::check_store::ClaimOutcome, StoreError> {
            self.evaluation_failures
                .lock()
                .unwrap()
                .push(diagnostic.clone());
            Ok(crate::check_store::ClaimOutcome::Current)
        }

        fn record_not_assessed(
            &self,
            _: &ClaimedCandidate,
            _: &str,
        ) -> Result<crate::check_store::ClaimOutcome, StoreError> {
            Ok(crate::check_store::ClaimOutcome::Current)
        }
    }

    struct ScriptedEvaluator {
        inputs: Mutex<Vec<serde_json::Value>>,
    }

    impl DecisionEvaluator for ScriptedEvaluator {
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
            _: &'a crate::domain::FrozenCandidate,
            prepared: &'a PreparedAssessment,
            attempt: &'a DecisionAttempt,
        ) -> DecisionFuture<'a> {
            self.inputs.lock().unwrap().push(
                prepared
                    .decisions
                    .iter()
                    .find(|block| block.decision == attempt.decision)
                    .unwrap()
                    .input
                    .clone(),
            );
            Box::pin(async move { Ok(test_answer(activation, attempt, Some("uncertain"))) })
        }
    }

    struct Events(std::vec::IntoIter<SourceEvent>);

    impl ReplayEventSource for Events {
        fn next_event(&mut self) -> Result<Option<SourceEvent>, String> {
            Ok(self.0.next())
        }
    }

    fn activation() -> EffectiveActivation {
        let source = include_str!("../tests/fixtures/retry-review.yml");
        activation_for(source)
    }

    fn activation_for(source: &str) -> EffectiveActivation {
        EffectiveActivation {
            id: ActivationId {
                scope: crate::domain::ActivationScope::default(),
                definition_id: "replay-check".into(),
                generation: Generation(1),
            },
            revision: [1; 32],
            definition_scope: crate::discovery::DefinitionScope::Global,
            source_paths: Vec::new(),
            project_root: None,
            compiled: Arc::new(crate::compiler::compile(source).unwrap()),
            bootstrap_observation_position: ObservationPosition(0),
            bootstrap_publication: PublicationCursor(0),
        }
    }

    fn event(position: u64) -> SourceEvent {
        SourceEvent::Observation {
            source_evidence_version: None,
            cursor: PublicationCursor(position),
            identity: format!("result-{position}"),
            revision: 1,
            observation: Box::new(ObservationDraft {
                first_eligible_publication: None,
                branch: BranchId("main".into()),
                native_order: NativeOrderKey(format!("native-{position}")),
                first_terminal_publication: None,
                version: EvidenceVersion(format!("v{position}")),
                kind: ObservationKind::ToolResult {
                    message: Message {
                        id: format!("result-{position}"),
                        value: json!({"tool": {"name": "shell", "result": {"status": "error", "content": "failed"}}}),
                    },
                    category: "shell".into(),
                    status: "error".into(),
                },
                context: FrozenContext {
                    value: json!({"session": {"requests": [{"id": format!("request-{position}"), "text": "Retry"}], "requests_complete": true, "source_coverage": "complete"}}),
                    dependencies: Vec::new(),
                },
                complete: true,
            }),
        }
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn synthetic_definition_replays_each_trigger_before_reading_later_requests() {
        let store = Arc::new(MemoryStore::default());
        let evaluator = Arc::new(ScriptedEvaluator {
            inputs: Mutex::new(Vec::new()),
        });
        let mut events = Events((1..=4).map(event).collect::<Vec<_>>().into_iter());
        let report = block_on(replay(
            ReplayRunId("test-run".into()),
            activation(),
            Arc::clone(&store),
            Arc::clone(&evaluator),
            &mut events,
            ReplayLimits {
                events: 10,
                evaluations: 10,
                attempts: 10,
            },
            || false,
        ))
        .unwrap();

        assert_eq!(report.end, ReplayEnd::Complete);
        assert_eq!(report.events, 4);
        assert_eq!(report.evaluations, 2);
        assert_eq!(report.attempts, 2);
        assert_eq!(store.accepted.lock().unwrap().len(), 2);
        assert_eq!(store.answers.lock().unwrap().len(), 2);
        let inputs = evaluator.inputs.lock().unwrap();
        assert_eq!(inputs[0]["requests"][0]["id"], "request-3");
        assert_eq!(inputs[1]["requests"][0]["id"], "request-4");
        assert_eq!(inputs[0]["activity"].as_array().unwrap().len(), 3);
        assert_eq!(inputs[1]["activity"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn the_same_runner_accepts_an_independent_compiled_definition() {
        let definition = "name: Independent\non:\n  tool_result:\n  window: {size: 2, min: 1}\n  cooldown: 30s\ndecisions:\n  review:\n    input:\n      requests: \"${{ session.requests }}\"\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria:\n          stuck: Concern.\n          clear: Clear.\n          uncertain: Unclear.\nactions:\n  - type: status\n    input: {result: failure}\n";
        let store = Arc::new(MemoryStore::default());
        let evaluator = Arc::new(ScriptedEvaluator {
            inputs: Mutex::new(Vec::new()),
        });
        let mut events = Events(vec![event(1)].into_iter());
        let report = block_on(replay(
            ReplayRunId("independent-run".into()),
            activation_for(definition),
            store.clone(),
            evaluator,
            &mut events,
            ReplayLimits {
                events: 2,
                evaluations: 2,
                attempts: 2,
            },
            || false,
        ))
        .unwrap();

        assert_eq!(report.end, ReplayEnd::Complete);
        assert_eq!(report.evaluations, 1);
        assert_eq!(store.accepted.lock().unwrap().len(), 1);
    }

    #[test]
    fn cancellation_is_reported_before_consuming_the_next_event() {
        let store = Arc::new(MemoryStore::default());
        let evaluator = Arc::new(ScriptedEvaluator {
            inputs: Mutex::new(Vec::new()),
        });
        let mut events = Events(vec![event(1)].into_iter());
        let report = block_on(replay(
            ReplayRunId("cancelled-run".into()),
            activation(),
            store,
            evaluator,
            &mut events,
            ReplayLimits {
                events: 10,
                evaluations: 10,
                attempts: 10,
            },
            || true,
        ))
        .unwrap();
        assert_eq!(report.end, ReplayEnd::Cancelled);
        assert_eq!(report.events, 0);
    }

    #[test]
    fn attempt_limit_stops_before_dispatching_the_next_trigger() {
        let store = Arc::new(MemoryStore::default());
        let evaluator = Arc::new(ScriptedEvaluator {
            inputs: Mutex::new(Vec::new()),
        });
        let mut events = Events((1..=4).map(event).collect::<Vec<_>>().into_iter());
        let report = block_on(replay(
            ReplayRunId("bounded-run".into()),
            activation(),
            store.clone(),
            evaluator,
            &mut events,
            ReplayLimits {
                events: 10,
                evaluations: 10,
                attempts: 1,
            },
            || false,
        ))
        .unwrap();
        assert_eq!(report.end, ReplayEnd::LimitReached);
        assert_eq!(report.evaluations, 2);
        assert_eq!(report.attempts, 1);
        assert_eq!(store.accepted.lock().unwrap().len(), 1);
        assert_eq!(
            report.steps.last().unwrap().outcome,
            Some(RunOutcome::LimitReached)
        );
    }

    struct SequenceEvaluator(
        Mutex<std::collections::VecDeque<Result<&'static str, AttemptFailure>>>,
    );

    impl DecisionEvaluator for SequenceEvaluator {
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
            _: &'a crate::domain::FrozenCandidate,
            _: &'a PreparedAssessment,
            attempt: &'a DecisionAttempt,
        ) -> DecisionFuture<'a> {
            let next = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted attempt");
            Box::pin(async move {
                let output = next.map_err(|kind| EvaluationFailure {
                    code: crate::engine::EvaluationFailureCode::ProviderUnavailable,
                    kind,
                    detail: "scripted provider failure".into(),
                    attempts: 1,
                })?;
                Ok(test_answer(activation, attempt, Some(output)))
            })
        }
    }

    fn run_sequence(
        store: Arc<MemoryStore>,
        outcomes: Vec<Result<&'static str, AttemptFailure>>,
        limits: ReplayLimits,
    ) -> ReplayReport {
        block_on(replay(
            ReplayRunId("sequence".into()),
            activation(),
            store,
            Arc::new(SequenceEvaluator(Mutex::new(outcomes.into()))),
            &mut Events((1..=5).map(event).collect::<Vec<_>>().into_iter()),
            limits,
            || false,
        ))
        .unwrap()
    }

    fn small_limits() -> ReplayLimits {
        ReplayLimits {
            events: 8,
            evaluations: 3,
            attempts: 6,
        }
    }

    #[test]
    fn status_actions_clear_failure_on_uncertainty() {
        let store = Arc::new(MemoryStore::default());
        let report = run_sequence(
            store.clone(),
            vec![Ok("stuck"), Ok("uncertain"), Ok("clear")],
            small_limits(),
        );
        assert_eq!(report.end, ReplayEnd::Complete);
        let accepted = store.accepted.lock().unwrap();
        assert_eq!(accepted.len(), 3);
        assert_eq!(
            accepted[0].status_write(),
            Some(crate::compiler::StatusResult::Failure)
        );
        assert_eq!(
            accepted[1].status_write(),
            Some(crate::compiler::StatusResult::Success)
        );
        assert_eq!(
            accepted[1].actions.current_status,
            Some(crate::compiler::StatusResult::Success)
        );
        assert_eq!(
            accepted[2].status_write(),
            Some(crate::compiler::StatusResult::Success)
        );
    }

    #[test]
    fn unknown_delivery_stops_replay_without_resend_or_status() {
        let store = Arc::new(MemoryStore::default());
        let report = run_sequence(
            store.clone(),
            vec![Err(AttemptFailure::Unknown)],
            small_limits(),
        );
        assert!(matches!(report.end, ReplayEnd::ExecutionError(_)));
        assert_eq!(
            (report.events, report.evaluations, report.attempts),
            (3, 1, 1)
        );
        assert_eq!(*store.dispatches.lock().unwrap(), vec![1]);
        assert_eq!(
            *store.failures.lock().unwrap(),
            vec![AttemptFailure::Unknown]
        );
        assert!(store.answers.lock().unwrap().is_empty());
        assert!(store.accepted.lock().unwrap().is_empty());
    }

    #[test]
    fn safe_retry_is_settled_and_bounded_by_remaining_attempt_budget() {
        for (budget, outcomes, expected_dispatches) in [
            (1, vec![Err(AttemptFailure::SafeTransient)], vec![1]),
            (
                2,
                vec![Err(AttemptFailure::SafeTransient), Ok("clear")],
                vec![1, 2],
            ),
        ] {
            let store = Arc::new(MemoryStore::default());
            let report = run_sequence(
                store.clone(),
                outcomes,
                ReplayLimits {
                    attempts: budget,
                    ..small_limits()
                },
            );
            assert_eq!(report.attempts, budget);
            assert_eq!(*store.dispatches.lock().unwrap(), expected_dispatches);
            assert_eq!(
                *store.failures.lock().unwrap(),
                vec![AttemptFailure::SafeTransient]
            );
            assert_eq!(
                store.accepted.lock().unwrap().len(),
                usize::from(budget == 2)
            );
        }
    }

    #[test]
    fn deferred_acceptance_keeps_the_answer_and_stops_before_another_call() {
        let store = Arc::new(MemoryStore::default());
        *store.acceptance.lock().unwrap() = Some(AcceptanceOutcome::Deferred);
        let report = run_sequence(store.clone(), vec![Ok("stuck")], small_limits());
        assert!(matches!(report.end, ReplayEnd::ExecutionError(_)));
        assert_eq!(
            (report.events, report.evaluations, report.attempts),
            (3, 1, 1)
        );
        assert_eq!(store.answers.lock().unwrap().len(), 1);
        assert!(store.accepted.lock().unwrap().is_empty());
    }

    #[test]
    fn replay_binds_run_scope_and_resets_both_bootstrap_orders() {
        let store = Arc::new(MemoryStore::default());
        let mut activation = activation();
        activation.bootstrap_observation_position = ObservationPosition(30);
        activation.bootstrap_publication = PublicationCursor(90);
        activation.id.scope.environment = "local".into();
        activation.id.scope.session_cwd = "project".into();
        let report = block_on(replay(
            ReplayRunId("isolated".into()),
            activation,
            store.clone(),
            Arc::new(ScriptedEvaluator {
                inputs: Mutex::new(Vec::new()),
            }),
            &mut Events((1..=3).map(event).collect::<Vec<_>>().into_iter()),
            small_limits(),
            || false,
        ))
        .unwrap();
        assert_eq!(report.evaluations, 1);
        let activations = store.activations.lock().unwrap();
        assert_eq!(
            activations[0].id.scope.run,
            crate::domain::RunScope::Replay("isolated".into())
        );
        assert_eq!(activations[0].id.scope.environment, "local");
        assert_eq!(activations[0].id.scope.session_cwd, "project");
        assert_eq!(
            activations[0].bootstrap_observation_position,
            ObservationPosition(0)
        );
        assert_eq!(activations[0].bootstrap_publication, PublicationCursor(0));
    }

    #[test]
    fn event_and_evaluation_limits_bound_work_before_dispatch() {
        for (limits, expected) in [
            (
                ReplayLimits {
                    events: 2,
                    ..small_limits()
                },
                (2, 0, 0),
            ),
            (
                ReplayLimits {
                    evaluations: 0,
                    ..small_limits()
                },
                (3, 0, 0),
            ),
            (
                ReplayLimits {
                    attempts: 0,
                    ..small_limits()
                },
                (3, 1, 0),
            ),
        ] {
            let store = Arc::new(MemoryStore::default());
            let report = run_sequence(store.clone(), Vec::new(), limits);
            assert_eq!(report.end, ReplayEnd::LimitReached);
            assert_eq!(
                (report.events, report.evaluations, report.attempts),
                expected
            );
            assert!(store.dispatches.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn exact_event_budget_distinguishes_eof_from_unapplied_input() {
        for (count, budget, end) in [
            (0, 0, ReplayEnd::Complete),
            (2, 2, ReplayEnd::Complete),
            (3, 2, ReplayEnd::LimitReached),
        ] {
            let store = Arc::new(MemoryStore::default());
            let mut events = Events((1..=count).map(event).collect::<Vec<_>>().into_iter());
            let report = block_on(replay(
                ReplayRunId("exact-event-budget".into()),
                activation(),
                store.clone(),
                Arc::new(ScriptedEvaluator {
                    inputs: Mutex::new(Vec::new()),
                }),
                &mut events,
                ReplayLimits {
                    events: budget,
                    ..small_limits()
                },
                || false,
            ))
            .unwrap();
            assert_eq!(report.end, end);
            assert_eq!(report.events, budget);
            assert_eq!(report.steps.len(), budget);
            assert_eq!(report.evaluations, 0);
            assert!(store.dispatches.lock().unwrap().is_empty());
            assert_eq!(
                store.progress.lock().unwrap().publication_cursor.0,
                u64::try_from(budget).unwrap()
            );
        }
    }
}
