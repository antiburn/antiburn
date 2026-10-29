use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context as TaskContext, Poll, Waker};

use antiburn_checks::actions::plan_status_actions;
use antiburn_checks::compiler::compile;
use antiburn_checks::decision::DecisionInput;
use antiburn_checks::domain::{
    BranchId, EvidenceVersion, FrozenContext, Message, NativeOrderKey, ObservationDraft,
    ObservationKind,
};
use antiburn_checks::engine::{
    CheckEngine, DecisionEvaluator, DecisionFuture, EvaluationFailure,
    PreparationValidationPurpose, RunOutcome, StartOutcome,
};
use antiburn_checks::expression_eval::EvaluationBudget;
use antiburn_checks::prepared_assessment::{PreparedAssessment, PreparedBlock};
use antiburn_checks::results::{ChoiceResult, NamedAnswer, QuestionResult};
use antiburn_checks::source::SourceEvent;
use serde_json::json;

use super::*;

#[path = "runtime_tracing_tests.rs"]
mod tracing_tests;

#[path = "runtime_admission_tests.rs"]
mod admission;
#[path = "runtime_connection_tests.rs"]
mod connections;
#[path = "runtime_decision_tests.rs"]
mod decisions;
#[path = "runtime_events_tests.rs"]
mod events;
#[path = "runtime_expression_tests.rs"]
mod expressions;
#[path = "runtime_failure_tests.rs"]
mod failures;
#[path = "runtime_gap_tests.rs"]
mod gaps;
#[path = "runtime_local_tests.rs"]
mod local;
#[path = "runtime_preparation_tests.rs"]
mod preparation;
#[path = "runtime_progress_tests.rs"]
mod progress;
#[path = "runtime_recovery_validation_tests.rs"]
mod recovery_validation;
#[path = "runtime_registry_tests.rs"]
mod registry;
#[path = "runtime_selection_tests.rs"]
mod selection;
#[path = "runtime_snapshot_tests.rs"]
mod snapshots;
#[path = "runtime_unsafe_update_tests.rs"]
mod unsafe_updates;

const CHECK: &str = "name: Retry\non:\n  tool_result:\ndecisions:\n  review:\n    input: {activity: \"${{ message.tool.result.status }}\"}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    if: ${{ decisions.review.details.verdict.probabilities.yes > 0.9 }}\n    input: {result: failure}\n";

struct NoEvaluator;
impl DecisionEvaluator for NoEvaluator {
    fn prepare(
        &self,
        _: &EffectiveActivation,
        _: Vec<DecisionInput>,
        _: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        panic!("store source tests do not prepare provider requests")
    }
    fn validate_prepared(
        &self,
        _: &EffectiveActivation,
        _: &PreparedAssessment,
        _: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        panic!("store source tests do not validate provider requests")
    }
    fn evaluate_prepared<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        _: &'a PreparedAssessment,
        _: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        panic!("store tests do not send provider requests")
    }
}

fn fixture() -> (Arc<Store>, Arc<StoreCheckStore>, EffectiveActivation) {
    let store = Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-runtime-test")).unwrap());
    fixture_with_store(store)
}

fn fixture_with_store(
    store: Arc<Store>,
) -> (Arc<Store>, Arc<StoreCheckStore>, EffectiveActivation) {
    fixture_with_definition(store, CHECK)
}

fn fixture_with_definition(
    store: Arc<Store>,
    source: &str,
) -> (Arc<Store>, Arc<StoreCheckStore>, EffectiveActivation) {
    store
        .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "true")
        .unwrap();
    let scope = ActivationScope {
        environment: "native".into(),
        session_cwd: "/repo".into(),
        run: RunScope::Live,
    };
    let adapter =
        Arc::new(StoreCheckStore::new(store.clone(), "owner-1".into(), scope.clone()).unwrap());
    let active = EffectiveActivation {
        id: ActivationId {
            scope,
            definition_id: "retry".into(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: DefinitionScope::Global,
        source_paths: vec![],
        project_root: Some(PathBuf::from("/repo")),
        compiled: Arc::new(compile(source).unwrap()),
        bootstrap_observation_position: ObservationPosition(50),
        bootstrap_publication: PublicationCursor(0),
    };
    adapter
        .activate(
            "retry",
            &LifecycleTransition::Activated(active.clone()),
            Some(&active),
        )
        .unwrap();
    antiburn_checks_store::replace_runtime_definitions(
        &mut store.lock(),
        &[antiburn_checks_store::RuntimeDefinition {
            check_id: active.id.definition_id.clone(),
            activation_scope_key: adapter.activation_scope_key.clone(),
            generation: Some(active.id.generation.0.try_into().unwrap()),
            definition_revision: hex(&active.revision),
            name: active.compiled.name.clone(),
            availability: antiburn_checks_store::DefinitionAvailability::Available,
        }],
    )
    .unwrap();
    (store, adapter, active)
}

fn event(store: &Store, sequence: u64, id: &str, branch: &str, revision: u64) -> SourceBatch {
    store.lock().execute("INSERT INTO session (environment_key, agent, session_id, source_kind, source_label, first_seen_at, last_seen_at, cwd)
        VALUES ('native', 'fixture', ?1, 'file', 'fixture', 'now', 'now', '/repo') ON CONFLICT DO NOTHING", [branch]).unwrap();
    store.lock().execute("INSERT INTO native_tool_result_publication (publication_sequence, outcome, environment_key, agent, session_id, source_format, native_identity, native_position_json, native_revision, payload_json, complete, source_generation, claim_fence)
        VALUES (?1, 'update', 'native', 'fixture', ?2, 'fixture', ?3, '{}', ?4, '{}', 1, 1, 1)", params![sequence, branch, id, revision]).unwrap();
    SourceBatch {
        events: vec![SourceEvent::Observation {
            source_evidence_version: None,
            cursor: PublicationCursor(sequence),
            identity: id.into(),
            revision,
            observation: Box::new(ObservationDraft {
                first_eligible_publication: None,
                first_terminal_publication: None,
                branch: BranchId(branch.into()),
                native_order: NativeOrderKey(id.into()),
                version: EvidenceVersion(format!("{id}:{revision}")),
                kind: ObservationKind::ToolResult {
                    message: Message {
                        id: id.into(),
                        value: json!({"tool": {"result": {"status": "error"}}}),
                    },
                    category: "shell".into(),
                    status: "error".into(),
                },
                context: FrozenContext {
                    value: json!({"session": {"requests": []}}),
                    dependencies: vec![],
                },
                complete: true,
            }),
        }],
        next: PublicationCursor(sequence),
        has_more: false,
    }
}

fn apply(
    adapter: &Arc<StoreCheckStore>,
    active: &EffectiveActivation,
    page: &SourceBatch,
) -> FrozenCandidate {
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    engine.apply_source_page(active, page).unwrap();
    let position = adapter
        .load_source_progress(active)
        .unwrap()
        .observation_position;
    adapter
        .load_recovery(None, 10)
        .unwrap()
        .into_iter()
        .find_map(|record| match record.work {
            RecoveryWork::Pending { candidate, .. } if candidate.trigger_position == position => {
                Some(candidate)
            }
            _ => None,
        })
        .unwrap()
}

fn claim(adapter: &StoreCheckStore, candidate: &FrozenCandidate) -> ClaimedCandidate {
    let CandidateClaim::Claimed(claimed) = adapter.claim_due(candidate, 100_000, true).unwrap()
    else {
        panic!("candidate must be claimable");
    };
    *claimed
}

fn transient_diagnostic() -> EvaluationDiagnostic {
    EvaluationDiagnostic {
        code: antiburn_checks::engine::EvaluationFailureCode::RateLimited,
        detail: "Synthetic rate limit.".into(),
    }
}

fn review_attempt(number: u8) -> DecisionAttempt {
    DecisionAttempt::new("review", number).unwrap()
}

fn prepared_review(
    profile: &str,
    input: serde_json::Value,
    request: serde_json::Value,
    report: antiburn_checks::fit::FitReport,
    budget: EvaluationBudget,
) -> PreparedAssessment {
    PreparedAssessment::new(
        profile.into(),
        budget,
        0,
        vec![PreparedBlock {
            decision: "review".into(),
            original_input: input.clone(),
            input,
            request,
            report,
        }],
    )
    .unwrap()
}

fn prepare_inputs(
    inputs: Vec<DecisionInput>,
    budget: EvaluationBudget,
    profile: &str,
) -> PreparedAssessment {
    PreparedAssessment::new(
        profile.into(),
        budget,
        0,
        inputs
            .into_iter()
            .map(|input| PreparedBlock {
                decision: input.decision,
                original_input: input.input.clone(),
                input: input.input,
                request: json!({"exact": "saved"}),
                report: Default::default(),
            })
            .collect(),
    )
    .unwrap()
}

fn validate_fixture_preparation(
    active: &EffectiveActivation,
    prepared: &PreparedAssessment,
    profile: &str,
) -> Result<(), PreparationFailure> {
    let invalid = || PreparationFailure {
        code: antiburn_checks::fit::PreparationFailureCode::IncompatibleProfile,
        detail: "The saved synthetic preparation does not match its construction policy.".into(),
        report: None,
        context: Default::default(),
    };
    prepared
        .validate_for_declarations(&active.compiled.decisions)
        .map_err(|_| invalid())?;
    if prepared.profile != profile
        || prepared
            .decisions
            .iter()
            .any(|block| block.request != json!({"exact": "saved"}))
    {
        return Err(invalid());
    }
    Ok(())
}

fn review_result(active: &EffectiveActivation, number: u8, yes: f64) -> DecisionResult {
    let block = active
        .compiled
        .decisions
        .iter()
        .find(|block| block.name == "review")
        .unwrap();
    let question = block
        .outputs
        .iter()
        .find(|question| question.name == "verdict")
        .unwrap();
    DecisionResult::new(
        block,
        &active.compiled.question_requirements(),
        vec![
            NamedAnswer::new(
                "verdict",
                QuestionResult::Choice(
                    ChoiceResult::new(
                        &question.question,
                        if yes > 0.5 { "yes" } else { "no" },
                        Some(BTreeMap::from([
                            ("yes".into(), yes),
                            ("no".into(), 1.0 - yes),
                        ])),
                        None,
                    )
                    .unwrap(),
                ),
            )
            .unwrap(),
        ],
        number,
    )
    .unwrap()
}

fn next_review_attempt(adapter: &StoreCheckStore, claimed: &ClaimedCandidate) -> u8 {
    let active = adapter
        .registered_activation(&claimed.candidate.activation)
        .unwrap();
    let progress = adapter.load_decision_progress(claimed).unwrap();
    let DecisionProgressState::Next { attempt, .. } = validate_decision_progress(
        &active.compiled.decisions,
        &active.compiled.question_requirements(),
        &progress,
    )
    .unwrap()
    .state
    else {
        panic!("review must have a next attempt")
    };
    assert_eq!(attempt.decision, "review");
    attempt.number
}

impl StoreCheckStore {
    fn dispatch_review(
        &self,
        claimed: &ClaimedCandidate,
        now_ms: u64,
        cooldown: Duration,
        number: u8,
    ) -> Result<ClaimOutcome, StoreError> {
        let current: bool = self.store.lock().query_row("SELECT EXISTS(SELECT 1 FROM burn_check_candidate WHERE candidate_id = ?1 AND claim_owner = ?2 AND claim_fence = ?3 AND state = 'claimed')", params![hex(&claimed.id.0), self.owner, claimed.fence], |row| row.get(0)).unwrap();
        if !current {
            return self.record_dispatch_intent(claimed, now_ms, cooldown, &review_attempt(number));
        }
        if self.load_preparation(claimed)?.is_none() {
            self.record_preparation(
                claimed,
                &PreparationRecord::Ready(prepared_review(
                    "fixture-v1",
                    json!({}),
                    json!({"exact": "saved"}),
                    Default::default(),
                    EvaluationBudget::default(),
                )),
            )?;
        }
        self.record_dispatch_intent(claimed, now_ms, cooldown, &review_attempt(number))
    }

    fn fail_review(
        &self,
        claimed: &ClaimedCandidate,
        number: u8,
        outcome: AttemptFailure,
        diagnostic: &EvaluationDiagnostic,
    ) -> Result<ClaimOutcome, StoreError> {
        self.record_attempt_failure(claimed, &review_attempt(number), outcome, diagnostic)
    }
}

fn answer(
    adapter: &StoreCheckStore,
    active: &EffectiveActivation,
    claimed: ClaimedCandidate,
    yes: f64,
) -> AssessmentAcceptance {
    let number = next_review_attempt(adapter, &claimed);
    adapter
        .dispatch_review(&claimed, 100_000, Duration::from_secs(60), number)
        .unwrap();
    let result = review_result(active, number, yes);
    adapter
        .record_decision_answer(&claimed, &review_attempt(number), &result)
        .unwrap();
    let evaluated = EvaluatedAssessment::new(
        &active.compiled.decisions,
        &active.compiled.question_requirements(),
        vec![result],
    )
    .unwrap();
    let actions = plan_status_actions(
        &active.compiled.actions,
        &claimed.candidate.context.value,
        &evaluated,
        claimed.prior_status,
    )
    .unwrap();
    AssessmentAcceptance {
        claimed,
        evaluated,
        actions,
    }
}

#[test]
fn page_checkpoint_coalescing_and_generation_are_durable_and_atomic() {
    let (store, adapter, mut active) = fixture();
    Arc::make_mut(&mut active.compiled).trigger.window =
        Some(antiburn_checks::compiler::RollingWindow {
            size: 1,
            minimum: 1,
        });
    adapter.register_activation(&active).unwrap();
    let page = event(&store, 11, "one", "branch", 1);
    store.lock().execute_batch("CREATE TRIGGER reject_candidate BEFORE INSERT ON burn_check_candidate BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    assert!(engine.apply_source_page(&active, &page).is_err());
    assert_eq!(
        adapter
            .load_source_progress(&active)
            .unwrap()
            .observation_position,
        ObservationPosition(50)
    );
    assert_eq!(
        store
            .lock()
            .query_row("SELECT COUNT(*) FROM burn_check_observation", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_candidate")
        .unwrap();
    let first = apply(&adapter, &active, &page);
    let second = apply(&adapter, &active, &event(&store, 29, "two", "branch", 1));
    assert_eq!(second.trigger_position, ObservationPosition(52));
    assert_eq!(
        adapter.claim_due(&first, 100_000, false).unwrap(),
        CandidateClaim::Stale
    );
    assert_eq!(adapter.load_recovery(None, 10).unwrap().len(), 1);
    let restarted =
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap();
    assert_eq!(
        restarted
            .load_lifecycle("retry", active.compiled.clone())
            .unwrap(),
        LifecycleState::Active(active.clone())
    );
    restarted.register_activation(&active).unwrap();
    assert_eq!(
        restarted.load_source_progress(&active).unwrap(),
        adapter.load_source_progress(&active).unwrap()
    );
    let mut wrong = active.clone();
    wrong.id.generation = Generation(7);
    assert!(
        restarted
            .activate(
                "retry",
                &LifecycleTransition::Activated(wrong.clone()),
                Some(&wrong)
            )
            .is_err()
    );
    assert_eq!(claim(&restarted, &second).candidate, second);
}

#[test]
fn answered_recovery_defers_without_resend_and_preserves_last_status_write() {
    let source = CHECK.replace(
        "input: {result: failure}",
        "input: {result: failure, message: Initial failure explanation.}",
    );
    let store = Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-runtime-test")).unwrap());
    let (store, adapter, active) = fixture_with_definition(store, &source);
    let first = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let assessment = answer(&adapter, &active, claim(&adapter, &first), 0.99);
    let next_page = event(&store, 2, "two", "branch", 1);
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Deferred
    );
    let restarted = Arc::new(
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap(),
    );
    restarted.register_activation(&active).unwrap();
    let RecoveryWork::Answered { claimed, evaluated } =
        restarted.load_recovery(None, 10).unwrap().remove(0).work
    else {
        panic!("saved answer must recover");
    };
    assert_eq!(evaluated, assessment.evaluated);
    let second = apply(&restarted, &active, &next_page);
    let recovered = AssessmentAcceptance {
        claimed,
        evaluated,
        actions: assessment.actions,
    };
    store.lock().execute_batch("CREATE TRIGGER reject_status BEFORE INSERT ON burn_check_current_status BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
    assert!(restarted.accept(&recovered).is_err());
    for table in [
        "burn_check_assessment_history",
        "burn_check_acceptance_watermark",
    ] {
        let count: i64 = store
            .lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "failed acceptance must roll back {table}");
    }
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_status")
        .unwrap();
    assert_eq!(
        restarted.accept(&recovered).unwrap(),
        AcceptanceOutcome::Accepted
    );
    assert_eq!(
        restarted.accept(&recovered).unwrap(),
        AcceptanceOutcome::AlreadyAccepted
    );
    let original: (String, i64, String, String) = store
        .lock()
        .query_row(
            "SELECT s.assessment_id, s.updated_at_epoch,
                json_extract(h.actions_json, '$.outcomes[0].outcome.Applied.message'), s.status
             FROM burn_check_current_status s JOIN burn_check_assessment_history h USING(assessment_id)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(original.2, "Initial failure explanation.");
    assert_eq!(original.3, "failure");
    let first_watermark: (u64, u64) = store
        .lock()
        .query_row(
            "SELECT generation, position FROM burn_check_acceptance_watermark",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        first_watermark,
        (active.id.generation.0, first.trigger_position.0)
    );
    let claimed = claim(&restarted, &second);
    assert_eq!(claimed.prior_status, Some(StatusResult::Failure));
    let no_write = answer(&restarted, &active, claimed, 0.4);
    assert_eq!(no_write.status_write(), None);
    assert_eq!(
        restarted.accept(&no_write).unwrap(),
        AcceptanceOutcome::Accepted
    );
    let retained: (String, i64, String, String) = store
        .lock()
        .query_row(
            "SELECT s.assessment_id, s.updated_at_epoch,
                json_extract(h.actions_json, '$.outcomes[0].outcome.Applied.message'), s.status
             FROM burn_check_current_status s JOIN burn_check_assessment_history h USING(assessment_id)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(original, retained);
    let no_write_watermark: (u64, u64) = store
        .lock()
        .query_row(
            "SELECT generation, position FROM burn_check_acceptance_watermark",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        no_write_watermark,
        (active.id.generation.0, second.trigger_position.0)
    );
    assert!(no_write_watermark.1 > first_watermark.1);
    let (state, position, actions): (String, u64, String) = store
        .lock()
        .query_row(
            "SELECT c.state, h.trigger_position, h.actions_json FROM burn_check_candidate c
            JOIN burn_check_assessment_history h USING(candidate_id) WHERE c.candidate_id = ?1",
            [hex(&second.fingerprint)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, "accepted");
    assert_eq!(position, second.trigger_position.0);
    let actions: serde_json::Value = serde_json::from_str(&actions).unwrap();
    assert_eq!(actions["outcomes"][0]["outcome"], "SkippedConditionFalse");
    assert_eq!(actions["current_status"], json!(StatusResult::Failure));
    assert_eq!(
        restarted.accept(&no_write).unwrap(),
        AcceptanceOutcome::AlreadyAccepted
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
}

#[test]
fn safe_retry_number_survives_restart_and_unknown_dispatch_never_resends() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let first = claim(&adapter, &candidate);
    adapter
        .dispatch_review(&first, 100_000, Duration::from_secs(60), 1)
        .unwrap();
    adapter
        .fail_review(
            &first,
            1,
            AttemptFailure::SafeTransient,
            &transient_diagnostic(),
        )
        .unwrap();
    let restarted =
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap();
    restarted.register_activation(&active).unwrap();
    let CandidateClaim::Claimed(retry) = restarted.claim_due(&candidate, 120_000, false).unwrap()
    else {
        panic!("resumed work bypasses its own started cooldown");
    };
    assert_eq!(next_review_attempt(&restarted, &retry), 2);
    assert_eq!(
        restarted
            .dispatch_review(&first, 100_000, Duration::ZERO, 2)
            .unwrap(),
        ClaimOutcome::Stale
    );
    restarted
        .dispatch_review(&retry, 100_000, Duration::ZERO, 2)
        .unwrap();
    let recovered =
        StoreCheckStore::new(store.clone(), "owner-3".into(), active.id.scope.clone()).unwrap();
    recovered.register_activation(&active).unwrap();
    assert!(recovered.load_recovery(None, 10).unwrap().is_empty());
    assert_eq!(
        recovered.claim_due(&candidate, 200_000, true).unwrap(),
        CandidateClaim::Stale
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT outcome FROM burn_check_evaluation_attempt WHERE attempt_number = 2",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "unknown"
    );
    let saved: String = store
        .lock()
        .query_row(
            "SELECT diagnostic_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<EvaluationDiagnostic>(&saved)
            .unwrap()
            .code,
        antiburn_checks::engine::EvaluationFailureCode::DeliveryUnknown
    );
}

#[test]
fn correction_revokes_answer_and_exact_claim_does_not_substitute_branches() {
    let (store, adapter, active) = fixture();
    let first = apply(&adapter, &active, &event(&store, 1, "one", "branch-a", 1));
    let second = apply(&adapter, &active, &event(&store, 2, "two", "branch-b", 1));
    let second_claim = claim(&adapter, &second);
    assert_eq!(second_claim.id, CandidateId::from(&second));
    let assessment = answer(&adapter, &active, claim(&adapter, &first), 0.99);
    let mut correction = event(&store, 3, "one", "branch-a", 1);
    let SourceEvent::Observation { observation, .. } = &mut correction.events[0] else {
        unreachable!()
    };
    observation.version = EvidenceVersion("same-revision-correction".into());
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    assert_eq!(
        engine
            .apply_source_page(&active, &correction)
            .unwrap()
            .candidates,
        0
    );
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Stale
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_current_status",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn same_revision_correction_or_removal_invalidates_accepted_evidence() {
    for removed in [false, true] {
        let (store, adapter, active) = fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted
        );
        let page = if removed {
            store.lock().execute("INSERT INTO native_tool_result_publication (publication_sequence, outcome, environment_key, agent, session_id, source_format, native_identity, native_position_json, native_revision, payload_json, complete, source_generation, claim_fence)
                SELECT 2, 'removal', environment_key, agent, session_id, source_format, native_identity, native_position_json, native_revision, '{}', 1, source_generation, claim_fence
                FROM native_tool_result_publication WHERE publication_sequence = 1", []).unwrap();
            SourceBatch {
                events: vec![SourceEvent::Removal {
                    cursor: PublicationCursor(2),
                    identity: "one".into(),
                    revision: 1,
                    branch: BranchId("branch".into()),
                }],
                next: PublicationCursor(2),
                has_more: false,
            }
        } else {
            let mut page = event(&store, 2, "one", "branch", 1);
            let SourceEvent::Observation { observation, .. } = &mut page.events[0] else {
                unreachable!()
            };
            let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
                unreachable!()
            };
            message.value["tool"]["result"]["status"] = json!("corrected");
            observation.version = EvidenceVersion("same-revision-corrected-content".into());
            page
        };
        CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
            .apply_source_page(&active, &page)
            .unwrap();
        let valid: (bool, bool) = store.lock().query_row(
            "SELECT s.evidence_valid, h.evidence_valid FROM burn_check_current_status s JOIN burn_check_assessment_history h USING(assessment_id)",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(valid, (false, false));
        let mut next = active.clone();
        next.id.generation = Generation(2);
        next.revision = [2; 32];
        adapter
            .activate(
                "retry",
                &LifecycleTransition::Activated(next.clone()),
                Some(&next),
            )
            .unwrap();
        assert_eq!(
            adapter.claim_due(&candidate, 200_000, true).unwrap(),
            CandidateClaim::Stale
        );
        assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_current_status",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}

#[test]
fn later_correction_in_one_page_prevents_storing_the_earlier_candidate() {
    let (store, adapter, active) = fixture();
    let mut page = event(&store, 1, "one", "branch", 1);
    let mut correction = event(&store, 2, "one", "branch", 1);
    let SourceEvent::Observation { observation, .. } = &mut correction.events[0] else {
        unreachable!()
    };
    observation.version = EvidenceVersion("corrected".into());
    page.events.extend(correction.events);
    page.next = PublicationCursor(2);
    let outcome = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
        .apply_source_page(&active, &page)
        .unwrap();
    assert_eq!(outcome.candidates, 0);
    assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
    assert_eq!(
        store
            .lock()
            .query_row("SELECT COUNT(*) FROM burn_check_candidate", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        0
    );
}

#[test]
fn unapplied_publication_for_another_session_does_not_block_acceptance() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
    event(&store, 2, "two", "unrelated-session", 1);
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
}

#[test]
fn overflow_and_failure_keep_history_without_status_or_watermark() {
    let (store, adapter, active) = fixture();
    let overflow = apply(&adapter, &active, &event(&store, 1, "one", "a", 1));
    adapter.record_overflow(&overflow).unwrap();
    let failed = apply(&adapter, &active, &event(&store, 2, "two", "b", 1));
    adapter
        .record_failure(&claim(&adapter, &failed), "provider failed")
        .unwrap();
    let outcomes = store
        .lock()
        .prepare("SELECT outcome FROM burn_check_assessment_history ORDER BY trigger_position")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(outcomes, ["not_assessed", "execution_failed"]);
    for table in [
        "burn_check_current_status",
        "burn_check_acceptance_watermark",
        "burn_check_evaluation_attempt",
    ] {
        assert_eq!(
            store
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn retention_prunes_unused_frozen_inputs_and_preserves_current_status_evidence() {
    let (store, adapter, active) = fixture();
    for (sequence, branch, yes) in [(1, "no-status", 0.4), (2, "failure", 0.99)] {
        let candidate = apply(
            &adapter,
            &active,
            &event(&store, sequence, "one", branch, 1),
        );
        let claimed = claim(&adapter, &candidate);
        let prepared = prepared_review(
            "fixture-v1",
            json!({"text": "retained evidence"}),
            json!({"exact": "provider request"}),
            antiburn_checks::fit::FitReport {
                omissions: vec![antiburn_checks::fit::FitOmission {
                    path: "text".into(),
                    strategy: antiburn_checks::fit::FitStrategy::Truncate,
                    original_count: 2048,
                    retained_count: 1024,
                }],
            },
            EvaluationBudget::default(),
        );
        adapter
            .record_preparation(&claimed, &PreparationRecord::Ready(prepared))
            .unwrap();
        let assessment = answer(&adapter, &active, claimed, yes);
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted
        );
    }
    Store::prune_burn_check_candidate_payloads_in(&store.lock(), now_epoch() + 1).unwrap();
    let payloads = store.lock().prepare("SELECT branch_key, frozen_input_json <> '{}' FROM burn_check_candidate ORDER BY branch_key").unwrap()
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert_eq!(
        payloads,
        [("failure".into(), true), ("no-status".into(), false)]
    );
    let prepared = store
        .lock()
        .prepare(
            "SELECT branch_key, preparation_json FROM burn_check_candidate ORDER BY branch_key",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for (branch, saved) in prepared {
        let prepared: serde_json::Value = serde_json::from_str(&saved).unwrap();
        assert_eq!(
            prepared["decisions"][0]["report"]["omissions"][0]["retained_count"],
            1024
        );
        if branch == "failure" {
            assert!(prepared["decisions"][0].get("request").is_some());
            assert_eq!(
                prepared["decisions"][0]["input"]["text"],
                "retained evidence"
            );
            assert_eq!(
                prepared["decisions"][0]["original_input"]["text"],
                "retained evidence"
            );
        } else {
            for payload in ["original_input", "input", "request"] {
                assert!(prepared["decisions"][0].get(payload).is_none());
            }
        }
    }
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_assessment_history",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
}

#[test]
fn source_page_read_before_deletion_cannot_restore_deleted_check_data() {
    let (store, adapter, active) = fixture();
    let page = event(&store, 1, "one", "branch", 1);
    store
        .delete_session(&crate::store::SessionKey::new(
            "native", "fixture", "branch",
        ))
        .unwrap()
        .unwrap();
    assert!(
        CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
            .apply_source_page(&active, &page)
            .is_err()
    );
    assert_eq!(
        adapter
            .load_source_progress(&active)
            .unwrap()
            .publication_cursor,
        PublicationCursor(0)
    );
    for table in ["burn_check_observation", "burn_check_candidate"] {
        assert_eq!(
            store
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn v75_migration_preserves_existing_data_and_supports_runtime_acceptance() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &migration in &crate::store::schema::MIGRATIONS[..75] {
        connection.execute_batch(migration).unwrap();
    }
    connection.pragma_update(None, "user_version", 75).unwrap();
    connection.execute_batch(
        "INSERT INTO setting (key, value) VALUES ('legacy-setting', 'preserved');
         INSERT INTO session (environment_key, agent, session_id, source_kind, source_label, first_seen_at, last_seen_at)
           VALUES ('native', 'claude-code', 'migration-session', 'file', 'fixture', 'now', 'now');
         INSERT INTO burn_check_assessment (environment_key, agent, session_id, check_id, incarnation, created_at_epoch, updated_at_epoch)
           VALUES ('native', 'claude-code', 'migration-session', 'existing-detector', 1, 1, 1);",
    ).unwrap();
    let store =
        Store::from_connection(connection, PathBuf::from("/tmp/antiburn-runtime-migration"))
            .unwrap();
    assert_eq!(
        store.schema_version().unwrap(),
        i64::try_from(crate::store::schema::MIGRATIONS.len()).unwrap()
    );
    let (store, adapter, active) = fixture_with_store(Arc::new(store));
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
    let connection = store.lock();
    assert_eq!(
        connection
            .query_row(
                "SELECT value FROM setting WHERE key = 'legacy-setting'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "preserved"
    );
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'burn_check_assessment'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    assert!(
        connection
            .execute(
                "UPDATE native_tool_result_publication SET source_generation = 2",
                []
            )
            .is_err()
    );
    assert!(
        connection
            .execute("DELETE FROM native_tool_result_publication", [])
            .is_err()
    );
    drop(connection);
    store
        .delete_session(&crate::store::SessionKey::new(
            "native", "fixture", "branch",
        ))
        .unwrap()
        .unwrap();
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn recovery_uses_keyset_pages_and_replay_has_an_independent_scope() {
    let (store, adapter, active) = fixture();
    for (sequence, branch) in [(1, "a"), (2, "b"), (3, "c")] {
        apply(
            &adapter,
            &active,
            &event(&store, sequence, "same-native-id", branch, 1),
        );
    }
    let mut after = None;
    let mut ids = Vec::new();
    loop {
        let records = adapter.load_recovery(after.as_ref(), 1).unwrap();
        if records.is_empty() {
            break;
        }
        let RecoveryWork::Pending { candidate, .. } = &records[0].work else {
            panic!("pending candidate expected");
        };
        let id = CandidateId::from(candidate);
        assert!(after.as_ref().is_none_or(|after| after < &id));
        after = Some(id.clone());
        ids.push(id);
    }
    assert_eq!(ids.len(), 3);
    let mut replay = active.clone();
    replay.id.scope.run = RunScope::Replay("isolated-run".into());
    replay.bootstrap_observation_position = ObservationPosition(0);
    let replay_store = Arc::new(
        StoreCheckStore::new(
            store.clone(),
            "replay-owner".into(),
            replay.id.scope.clone(),
        )
        .unwrap(),
    );
    replay_store
        .activate(
            "retry",
            &LifecycleTransition::Activated(replay.clone()),
            Some(&replay),
        )
        .unwrap();
    let page = SourceBatch {
        events: vec![SourceEvent::Observation {
            source_evidence_version: None,
            cursor: PublicationCursor(1),
            identity: "same-native-id".into(),
            revision: 1,
            observation: Box::new(ObservationDraft {
                first_eligible_publication: None,
                first_terminal_publication: None,
                branch: BranchId("a".into()),
                native_order: NativeOrderKey("one".into()),
                version: EvidenceVersion("one".into()),
                kind: ObservationKind::ToolResult {
                    message: Message {
                        id: "same-native-id".into(),
                        value: json!({"tool": {"result": {"status": "error"}}}),
                    },
                    category: "shell".into(),
                    status: "error".into(),
                },
                context: FrozenContext {
                    value: json!({"session": {"requests": []}}),
                    dependencies: vec![],
                },
                complete: true,
            }),
        }],
        next: PublicationCursor(1),
        has_more: false,
    };
    let candidate = apply(&replay_store, &replay, &page);
    let assessment = answer(
        &replay_store,
        &replay,
        claim(&replay_store, &candidate),
        0.99,
    );
    assert_eq!(
        replay_store.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
    assert_ne!(
        StoreCheckStore::activation_scope_key(&replay.id.scope),
        StoreCheckStore::activation_scope_key(&active.id.scope)
    );
    assert_eq!(adapter.load_recovery(None, 10).unwrap().len(), 3);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_current_status WHERE activation_scope_key = ?1",
                [&adapter.activation_scope_key],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn clear_and_session_deletion_remove_frozen_selector_payloads() {
    let (store, adapter, active) = fixture();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    let RecoveryWork::Pending { candidate, .. } =
        adapter.load_recovery(None, 10).unwrap().remove(0).work
    else {
        panic!("pending candidate expected");
    };
    let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
    store
        .delete_session(&crate::store::SessionKey::new(
            "native", "fixture", "branch",
        ))
        .unwrap();
    let progress = adapter.load_source_progress(&active).unwrap();
    assert!(progress.selection.branches.is_empty());
    assert!(progress.selection.seen_results.is_empty());
    for table in [
        "burn_check_observation",
        "burn_check_candidate",
        "burn_check_candidate_dependency",
        "burn_check_evaluation_attempt",
        "burn_check_assessment_history",
        "burn_check_current_status",
        "burn_check_acceptance_watermark",
        "burn_check_cooldown",
    ] {
        let count: i64 = store
            .lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "session deletion must clear {table}");
    }
    let next_page = event(&store, 2, "two", "new-branch", 1);
    assert!(engine.apply_source_page(&active, &next_page).is_err());
    engine.apply_source_page(&active, &next_page).unwrap();
    assert_eq!(
        adapter
            .load_source_progress(&active)
            .unwrap()
            .selection
            .branches
            .len(),
        1
    );
    store.clear_local_session_data().unwrap();
    assert!(adapter.register_activation(&active).is_err());
    assert_eq!(
        adapter.load_lifecycle("retry", None).unwrap(),
        LifecycleState::Removed {
            generation: Generation(1)
        }
    );
    let mut next = active.clone();
    next.id.generation = Generation(2);
    adapter
        .activate(
            "retry",
            &LifecycleTransition::Activated(next.clone()),
            Some(&next),
        )
        .unwrap();
    assert_eq!(
        adapter.load_lifecycle("retry", None).unwrap(),
        LifecycleState::Uncompiled {
            generation: Generation(2)
        }
    );
}

struct RecordingEvaluator(Mutex<Vec<u8>>);

impl DecisionEvaluator for RecordingEvaluator {
    fn prepare(
        &self,
        _: &EffectiveActivation,
        inputs: Vec<DecisionInput>,
        budget: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        Ok(prepare_inputs(inputs, budget, "fixture-v1"))
    }
    fn validate_prepared(
        &self,
        activation: &EffectiveActivation,
        prepared: &PreparedAssessment,
        _: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        validate_fixture_preparation(activation, prepared, "fixture-v1")
    }
    fn evaluate_prepared<'a>(
        &'a self,
        active: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        _: &'a PreparedAssessment,
        attempt: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        self.0.lock().unwrap().push(attempt.number);
        Box::pin(async move { Ok(review_result(active, attempt.number, 0.99)) })
    }
}

struct ControlledEvaluator {
    released: AtomicBool,
    failure: Option<AttemptFailure>,
    inputs: Mutex<Vec<(u64, serde_json::Value, u8)>>,
}

impl DecisionEvaluator for ControlledEvaluator {
    fn prepare(
        &self,
        _: &EffectiveActivation,
        inputs: Vec<DecisionInput>,
        budget: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        Ok(prepare_inputs(inputs, budget, "fixture-v1"))
    }
    fn validate_prepared(
        &self,
        active: &EffectiveActivation,
        prepared: &PreparedAssessment,
        purpose: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        RecordingEvaluator(Mutex::new(Vec::new())).validate_prepared(active, prepared, purpose)
    }
    fn evaluate_prepared<'a>(
        &'a self,
        active: &'a EffectiveActivation,
        candidate: &'a FrozenCandidate,
        prepared: &'a PreparedAssessment,
        attempt: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        let number = attempt.number;
        self.inputs.lock().unwrap().push((
            candidate.trigger_position.0,
            prepared.decisions[0].input.clone(),
            number,
        ));
        let first = candidate.trigger_position == ObservationPosition(51);
        Box::pin(std::future::poll_fn(move |_| {
            if first && !self.released.load(Ordering::SeqCst) {
                return Poll::Pending;
            }
            if first && let Some(kind) = self.failure {
                return Poll::Ready(Err(EvaluationFailure {
                    code: antiburn_checks::engine::EvaluationFailureCode::ProviderUnavailable,
                    kind,
                    detail: "scripted provider failure".into(),
                    attempts: number,
                }));
            }
            Poll::Ready(Ok(review_result(active, number, 0.99)))
        }))
    }
}

fn poll_settlement(
    task: antiburn_checks::engine::EvaluationTask<'_>,
) -> antiburn_checks::engine::RunSettlement {
    poll_task_result(task).unwrap()
}

fn poll_task_result(
    mut task: antiburn_checks::engine::EvaluationTask<'_>,
) -> Result<antiburn_checks::engine::RunSettlement, antiburn_checks::engine::EvaluationTaskError> {
    for _ in 0..1024 {
        if let Poll::Ready(result) = task
            .as_mut()
            .poll(&mut TaskContext::from_waker(Waker::noop()))
        {
            return result;
        }
    }
    panic!("scripted evaluator must settle within the test poll bound")
}

fn assert_start_error<E: DecisionEvaluator + 'static>(
    engine: &CheckEngine<StoreCheckStore, E>,
    active: &EffectiveActivation,
) {
    match engine.start_one_due(active, 100_000) {
        Err(_) => {}
        Ok(StartOutcome::Started(task)) => assert!(poll_task_result(task).is_err()),
        _ => panic!("the injected storage error must reach the caller"),
    }
}

#[test]
fn provider_failures_after_source_correction_settle_stale_and_release_single_flight() {
    for failure in [
        AttemptFailure::Permanent,
        AttemptFailure::Unknown,
        AttemptFailure::SafeTransient,
    ] {
        let (store, adapter, active) = fixture();
        let evaluator = Arc::new(ControlledEvaluator {
            released: AtomicBool::new(false),
            failure: Some(failure),
            inputs: Mutex::new(vec![]),
        });
        let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
        engine
            .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
            .unwrap();
        let StartOutcome::Started(mut task) = engine.start_one_due(&active, 100_000).unwrap()
        else {
            panic!("first candidate must start")
        };
        assert!(
            task.as_mut()
                .poll(&mut TaskContext::from_waker(Waker::noop()))
                .is_pending()
        );
        engine
            .apply_source_page(&active, &event(&store, 2, "one", "branch", 2))
            .unwrap();
        engine
            .apply_source_page(&active, &event(&store, 3, "two", "branch", 1))
            .unwrap();
        evaluator.released.store(true, Ordering::SeqCst);
        let settled = poll_settlement(task);
        assert_eq!(settled.outcome, RunOutcome::Stale, "{failure:?}");
        assert_eq!(settled.attempts, 1);
        assert_eq!(
            evaluator.inputs.lock().unwrap().len(),
            1,
            "stale transient work must not retry"
        );
        let StartOutcome::Started(task) = engine.start_one_due(&active, 160_000).unwrap() else {
            panic!("settled stale work must release its branch")
        };
        assert_eq!(poll_settlement(task).outcome, RunOutcome::Accepted);
        assert_eq!(
            evaluator
                .inputs
                .lock()
                .unwrap()
                .iter()
                .map(|item| (item.0, item.2))
                .collect::<Vec<_>>(),
            [(51, 1), (53, 1)]
        );
        let attempts: (i64, i64) = store.lock().query_row(
            "SELECT COUNT(*), SUM(state = 'dispatch_intent') FROM burn_check_evaluation_attempt", [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(attempts, (2, 0));
        let outcomes = store
            .lock()
            .prepare("SELECT outcome FROM burn_check_assessment_history ORDER BY trigger_position")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(outcomes, ["stale", "assessed"]);
    }
}

#[test]
fn correction_between_safe_failure_and_retry_intent_prevents_a_second_send() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    assert_eq!(
        adapter
            .dispatch_review(&claimed, 0, Duration::ZERO, 1)
            .unwrap(),
        ClaimOutcome::Current
    );
    assert_eq!(
        adapter
            .fail_review(
                &claimed,
                1,
                AttemptFailure::SafeTransient,
                &transient_diagnostic()
            )
            .unwrap(),
        ClaimOutcome::Current
    );
    CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
        .apply_source_page(&active, &event(&store, 2, "one", "branch", 2))
        .unwrap();
    assert_eq!(
        adapter
            .dispatch_review(&claimed, 0, Duration::ZERO, 2)
            .unwrap(),
        ClaimOutcome::Stale
    );
    let restarted =
        StoreCheckStore::new(store.clone(), "restarted".into(), active.id.scope.clone()).unwrap();
    restarted.register_activation(&active).unwrap();
    assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

fn content_event(store: &Store, sequence: u64, content: Option<&str>) -> SourceBatch {
    let mut page = event(store, sequence, &format!("result-{sequence}"), "branch", 1);
    let SourceEvent::Observation { observation, .. } = &mut page.events[0] else {
        unreachable!()
    };
    let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
        unreachable!()
    };
    if let Some(content) = content {
        message.value["tool"]["result"]["content"] = json!(content);
    }
    page
}

#[test]
fn trigger_condition_precedes_window_counting_and_cannot_replace_ready_work() {
    use antiburn_checks::preparation::ConditionOutcome;

    let source = CHECK
        .replace(
            "  tool_result:\n",
            "  tool_result:\n  window: {size: 2, min: 2}\n",
        )
        .replace(
            "decisions:\n",
            "if: message.tool.result.content == 'keep'\ndecisions:\n",
        )
        .replace("message.tool.result.status", "window.messages");
    let store =
        Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-runtime-condition")).unwrap());
    let (store, adapter, active) = fixture_with_definition(store, &source);
    let evaluator = Arc::new(ControlledEvaluator {
        released: AtomicBool::new(true),
        failure: None,
        inputs: Mutex::new(vec![]),
    });
    let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
    for (sequence, content, expected) in [
        (1, Some("keep"), None),
        (2, Some("drop"), Some(ConditionOutcome::False)),
        (3, None, Some(ConditionOutcome::Unavailable)),
        (4, Some("keep"), None),
    ] {
        let outcome = engine
            .apply_source_page(&active, &content_event(&store, sequence, content))
            .unwrap();
        assert_eq!(outcome.candidates, usize::from(sequence == 4));
        assert_eq!(
            outcome
                .condition_outcomes
                .into_iter()
                .map(|(_, condition)| condition)
                .collect::<Vec<_>>(),
            expected.into_iter().collect::<Vec<_>>()
        );
    }
    for (sequence, content) in [(5, Some("drop")), (6, None)] {
        assert_eq!(
            engine
                .apply_source_page(&active, &content_event(&store, sequence, content))
                .unwrap()
                .coalesced,
            0
        );
    }
    let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
        panic!("valid pending work must survive filtered events")
    };
    assert_eq!(poll_settlement(task).outcome, RunOutcome::Accepted);
    let inputs = evaluator.inputs.lock().unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].0, 54);
    assert_eq!(inputs[0].1["activity"].as_array().unwrap().len(), 2);
    assert!(
        inputs[0].1["activity"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message["tool"]["result"]["content"] == "keep")
    );
    assert_eq!(
        store
            .lock()
            .query_row("SELECT COUNT(*) FROM burn_check_candidate", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        1
    );
}

#[test]
fn non_window_candidates_survive_restart_and_dispatch_in_source_order() {
    let (store, adapter, active) = fixture();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    for sequence in 1..=3 {
        let outcome = engine
            .apply_source_page(
                &active,
                &event(&store, sequence, &format!("result-{sequence}"), "branch", 1),
            )
            .unwrap();
        assert_eq!(outcome.coalesced, 0);
    }
    assert_eq!(adapter.load_recovery(None, 10).unwrap().len(), 3);
    let restarted = Arc::new(
        StoreCheckStore::new(store, "restart-owner".into(), active.id.scope.clone()).unwrap(),
    );
    restarted.register_activation(&active).unwrap();
    let evaluator = Arc::new(ControlledEvaluator {
        released: AtomicBool::new(true),
        failure: None,
        inputs: Mutex::new(vec![]),
    });
    let engine = CheckEngine::new(restarted, evaluator.clone());
    assert_eq!(engine.recover_pending().unwrap().candidates, 3);
    for now in [100_000, 160_000, 220_000] {
        let StartOutcome::Started(task) = engine.start_one_due(&active, now).unwrap() else {
            panic!("each event must retain a separate candidate")
        };
        assert_eq!(poll_settlement(task).outcome, RunOutcome::Accepted);
    }
    assert_eq!(
        evaluator
            .inputs
            .lock()
            .unwrap()
            .iter()
            .map(|input| input.0)
            .collect::<Vec<_>>(),
        [51, 52, 53]
    );
}

#[test]
fn non_window_overflow_is_durable_and_keeps_earlier_pending_events() {
    let (store, adapter, active) = fixture();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    let mut overflowed = 0;
    for sequence in 1..=antiburn_checks::engine::MAX_PENDING_CANDIDATES as u64 + 2 {
        overflowed += engine
            .apply_source_page(
                &active,
                &event(&store, sequence, &format!("result-{sequence}"), "branch", 1),
            )
            .unwrap()
            .overflowed;
    }
    assert_eq!(overflowed, 2);
    assert_eq!(
        adapter.load_recovery(None, 500).unwrap().len(),
        antiburn_checks::engine::MAX_PENDING_CANDIDATES
    );
    let rows = store.lock().prepare("SELECT state, COUNT(*), MIN(trigger_position), MAX(trigger_position) FROM burn_check_candidate GROUP BY state ORDER BY state").unwrap()
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?, row.get::<_, u64>(2)?, row.get::<_, u64>(3)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert_eq!(
        rows,
        [
            ("not_assessed".into(), 2, 179, 180),
            ("pending".into(), 128, 51, 178)
        ]
    );
}

#[test]
fn startup_admission_refills_oldest_eligible_sessions_after_the_cap() {
    use antiburn_local::analysis::file_live::{NativeFileCursor, read_native_file_page};
    use antiburn_local::analysis::{RawSource, SessionInput, SourceFormat};

    let (store, adapter, active) = fixture();
    let directory = tempfile::tempdir().unwrap();
    for index in 0..130 {
        let session_id = format!("startup-{index:03}");
        let path = directory.path().join(format!("{session_id}.jsonl"));
        let header = json!({"type": "session", "version": 3, "id": session_id, "cwd": "/repo"});
        std::fs::write(&path, format!("{header}\n")).unwrap();
        let page = read_native_file_page(
            &SessionInput {
                agent: "pi".into(),
                session_id: session_id.clone(),
                source: RawSource::File(path.clone()),
                source_format: SourceFormat::PiV3Jsonl,
                fork_parent_session_id: None,
            },
            None,
            &|| false,
        )
        .unwrap();
        assert!(!page.has_more);
        let cursor = NativeFileCursor::decode(&page.cursor.encode()).unwrap();
        store.lock().execute(
            "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
                cwd, updated_at_epoch, first_seen_at, last_seen_at, source_format)
             VALUES ('native', 'pi', ?1, 'file', ?2, '/repo', ?3, 'now', 'now', ?4)",
            params![session_id, path.to_string_lossy(), index,
                serde_json::to_string(&SourceFormat::PiV3Jsonl).unwrap()],
        ).unwrap();
        store.lock().execute(
            "INSERT INTO native_file_delivery (environment_key, agent, session_id, incarnation,
                source_label, cwd, parser_revision, cursor_json, observed_generation, pending, active_until_epoch)
             SELECT environment_key, agent, session_id, incarnation, source_label, cwd, ?2, ?3,
                source_generation, 0, 0 FROM session WHERE environment_key = 'native'
                AND agent = 'pi' AND session_id = ?1",
            params![session_id, antiburn_local::analysis::PARSER_REVISION,
                serde_json::to_string(&cursor).unwrap()],
        ).unwrap();
    }

    let first = adapter.startup_sessions(&active, -1).unwrap();
    assert_eq!(first.len(), 128);
    assert_eq!(first.first().unwrap().key.session_id, "startup-129");
    assert!(
        first
            .iter()
            .any(|session| session.key.session_id == "startup-000")
    );
    assert!(
        !first
            .iter()
            .any(|session| session.key.session_id == "startup-001")
    );

    for session in &first {
        store
            .lock()
            .execute(
                "INSERT INTO burn_check_session_attachment
                (check_id, activation_scope_key, generation, branch_key, incarnation)
             VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    active.id.definition_id,
                    adapter.activation_scope_key,
                    active.id.generation.0,
                    session.key.to_identity_json(),
                    session.incarnation
                ],
            )
            .unwrap();
    }
    let refill = adapter.startup_sessions(&active, -1).unwrap();
    assert_eq!(
        refill
            .iter()
            .map(|session| session.key.session_id.as_str())
            .collect::<Vec<_>>(),
        ["startup-002", "startup-001"]
    );
}

struct PublishedReader(antiburn_checks::source::PublishedEvidenceBatch);

impl antiburn_checks::source::PublishedEvidenceReader for PublishedReader {
    fn read_batch(
        &self,
        _: antiburn_checks::source::PublicationRead,
    ) -> Result<antiburn_checks::source::PublishedEvidenceBatch, antiburn_checks::source::SourceError>
    {
        Ok(self.0.clone())
    }
}

#[test]
fn normalized_partial_result_exposes_authored_identity_and_remains_assessable() {
    use antiburn_checks::source::*;

    let source = CHECK.replace("decisions:\n", "if: message.id == 'one' && message.source == 'fixture' && message.kind == 'tool_result'\ndecisions:\n")
        .replace("input: {activity: \"${{ message.tool.result.status }}\"}", "input: {activity: '${{ message }}', requests: '${{ session.requests }}'}");
    let store =
        Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-runtime-normalization")).unwrap());
    let (store, adapter, active) = fixture_with_definition(store, &source);
    event(&store, 1, "one", "branch", 1);
    let page = PublishedEvidenceSource::new(PublishedReader(PublishedEvidenceBatch {
        events: vec![PublishedEvidenceEvent {
            source_evidence_version: None,
            cursor: PublicationCursor(1), branch: "branch".into(),
            context: Some(PublishedContext {
                session_selection: None,
                inventories: Default::default(),
                requests: vec![("request".into(), json!({"source": "fixture", "kind": "user_message", "text": "Review available results."}))],
                session_id: "branch".into(), agent_name: "fixture".into(), project_root: Some("/repo".into()),
                version: "request-prefix-1".into(), main_session: true, complete_through_event: false,
            }),
            change: PublishedEvidenceChange::Upsert(PublishedToolResult {
                metadata: Box::default(),
                first_terminal_publication: None,
                identity: "one".into(), native_order: NativeOrderKey("native-one".into()), revision: 1,
                tool_name: None, category: None, arguments: None, content: Some("Useful partial result".into()),
                outcome: None, provider_executed: None,
                status: PublishedResultStatus::Error, finality: true, payload_complete: false, validity: EvidenceValidity::Valid,
            }),
        }], next: PublicationCursor(1), has_more: false,
    }), 1).unwrap().read_batch(PublicationCursor(0)).unwrap();
    let evaluator = Arc::new(ControlledEvaluator {
        released: AtomicBool::new(true),
        failure: None,
        inputs: Mutex::new(vec![]),
    });
    let engine = CheckEngine::new(adapter, evaluator.clone());
    assert_eq!(
        engine.apply_source_page(&active, &page).unwrap().candidates,
        1
    );
    let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
        panic!("safe partial evidence must run")
    };
    assert_eq!(poll_settlement(task).outcome, RunOutcome::Accepted);
    let inputs = evaluator.inputs.lock().unwrap();
    let message = &inputs[0].1["activity"];
    assert_eq!(message["id"], "one");
    assert_eq!(message["source"], "fixture");
    assert_eq!(message["kind"], "tool_result");
    assert_eq!(message["quality"]["category_available"], false);
    assert_eq!(message["quality"]["request_history_complete"], false);
    assert!(message["tool"].get("category").is_none());
    assert!(message["tool"].get("arguments").is_none());
    assert_eq!(
        inputs[0].1["requests"][0]["text"],
        "Review available results."
    );
}

#[test]
fn saved_answers_reject_invalid_probabilities_and_wrong_choice_declarations() {
    for corruption in ["probabilities", "declaration"] {
        let (store, adapter, active) = fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
        let mut saved = serde_json::to_value(assessment.evaluated).unwrap();
        if corruption == "probabilities" {
            saved["decisions"][0]["answers"][0]["result"]["probabilities"] =
                json!({"yes": 1.5, "no": -0.5});
        } else {
            saved["decisions"][0]["answers"][0]["name"] = json!("other_question");
        }
        store
            .lock()
            .execute(
                "UPDATE burn_check_candidate SET answer_json = ?1 WHERE candidate_id = ?2",
                params![
                    serde_json::to_string(&saved).unwrap(),
                    hex(&candidate.fingerprint)
                ],
            )
            .unwrap();
        let restarted = StoreCheckStore::new(
            store.clone(),
            "restart-owner".into(),
            active.id.scope.clone(),
        )
        .unwrap();
        restarted.register_activation(&active).unwrap();
        let restarted = Arc::new(restarted);
        let engine = CheckEngine::new(restarted.clone(), Arc::new(NoEvaluator));
        assert_eq!(engine.recover_pending().unwrap().candidates, 0);
        let diagnostic: String = store.lock().query_row(
            "SELECT diagnostic_json FROM burn_check_candidate WHERE candidate_id = ?1 AND state = 'execution_failed'",
            [hex(&candidate.fingerprint)],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(
            serde_json::from_str::<EvaluationDiagnostic>(&diagnostic)
                .unwrap()
                .code,
            antiburn_checks::engine::EvaluationFailureCode::InvalidDecision,
            "{corruption}",
        );
        assert_eq!(
            restarted.claim_due(&candidate, 200_000, true).unwrap(),
            CandidateClaim::Stale
        );
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_current_status",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }
}

#[test]
fn recovered_safe_retry_passes_the_durable_number_to_the_evaluator() {
    use antiburn_checks::engine::{ExecutionPolicy, RunOutcome, StartOutcome};

    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    let evaluator = Arc::new(RecordingEvaluator(Mutex::new(vec![])));
    let antiburn_checks::preparation::PreparationOutcome::Ready(input) =
        antiburn_checks::preparation::prepare_decision_input(
            &active.compiled.decisions[0],
            &candidate,
        )
    else {
        panic!("fixture input must be available");
    };
    adapter
        .record_preparation(
            &claimed,
            &PreparationRecord::Ready(
                evaluator
                    .prepare(
                        &active,
                        vec![DecisionInput {
                            decision: "review".into(),
                            input,
                        }],
                        EvaluationBudget::default(),
                    )
                    .unwrap(),
            ),
        )
        .unwrap();
    adapter
        .dispatch_review(&claimed, 0, Duration::ZERO, 1)
        .unwrap();
    adapter
        .fail_review(
            &claimed,
            1,
            AttemptFailure::SafeTransient,
            &transient_diagnostic(),
        )
        .unwrap();
    let restarted =
        Arc::new(StoreCheckStore::new(store, "owner-2".into(), active.id.scope.clone()).unwrap());
    restarted.register_activation(&active).unwrap();
    let engine = CheckEngine::with_policy(
        restarted,
        evaluator.clone(),
        ExecutionPolicy::SequentialReplay,
    );
    assert_eq!(engine.recover_pending().unwrap().candidates, 1);
    let StartOutcome::Started(task) = engine.start_one_due(&active, 1).unwrap() else {
        panic!("recovered retry must start");
    };
    assert_eq!(poll_settlement(task).outcome, RunOutcome::Accepted);
    assert_eq!(*evaluator.0.lock().unwrap(), [2]);
}
