use super::*;
use crate::actions::ActionOutcomeKind;
use crate::compiler::StatusResult;
use crate::fit::LocalPreparation;
use std::sync::atomic::AtomicBool;

const ACTIONS: &str = "  - type: status\n    input: {result: failure, message: Local evidence.}\n";

struct LocalEvaluator {
    enabled: AtomicBool,
    checks: AtomicUsize,
    blocked_check: usize,
}

impl Default for LocalEvaluator {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(true),
            checks: AtomicUsize::new(0),
            blocked_check: 0,
        }
    }
}

impl DecisionEvaluator for LocalEvaluator {
    fn local_execution_is_current(&self, _: &EffectiveActivation) -> bool {
        let check = self.checks.fetch_add(1, Ordering::SeqCst) + 1;
        self.enabled.load(Ordering::SeqCst) && check != self.blocked_check
    }

    fn prepare(
        &self,
        _: &EffectiveActivation,
        _: Vec<DecisionInput>,
        _: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        panic!("local execution must not prepare a model");
    }

    fn preparation_recorded(&self, _: &PreparationRecord) {
        panic!("local execution must not report model preparation");
    }

    fn validate_prepared(
        &self,
        _: &EffectiveActivation,
        _: &PreparedAssessment,
        _: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        panic!("local execution must not validate model preparation");
    }

    fn evaluate_prepared<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        _: &'a PreparedAssessment,
        _: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        panic!("local execution must not evaluate model preparation");
    }
}

fn local_activation(on: &str, actions: &str) -> EffectiveActivation {
    let mut active = activation();
    active.compiled = Arc::new(
        compile(&format!(
            "name: Local\non:\n  tool_result:\n{on}actions:\n{actions}"
        ))
        .unwrap(),
    );
    active
}

fn local_engine(
    active: &EffectiveActivation,
    evaluator: Arc<LocalEvaluator>,
) -> (Arc<FakeStore>, CheckEngine<FakeStore, LocalEvaluator>) {
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), evaluator);
    engine
        .persist_activation(
            &active.id.definition_id,
            &crate::definition_lifecycle::LifecycleTransition::Activated(active.clone()),
            Some(active),
        )
        .unwrap();
    (store, engine)
}

fn start(
    engine: &CheckEngine<FakeStore, LocalEvaluator>,
    active: &EffectiveActivation,
    now: u64,
) -> RunSettlement {
    let StartOutcome::Started(task) = engine
        .start_one_due_with_attempt_limit(active, now, 0)
        .unwrap()
    else {
        panic!("local candidate must start without provider attempts");
    };
    run_task(task)
}

#[test]
fn local_start_saves_zero_attempt_answer_without_any_model_hook() {
    let active = local_activation("", ACTIONS);
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    let frozen = candidate(&active, 1);
    engine.enqueue(&active, frozen.clone()).unwrap();
    let settlement = start(&engine, &active, 1001);
    assert_eq!(
        settlement,
        RunSettlement {
            outcome: RunOutcome::Accepted,
            attempts: 0,
            new_intents: 0,
        }
    );
    assert_eq!(*store.dispatches.lock().unwrap(), 0);
    assert_eq!(
        *store.local_starts.lock().unwrap(),
        [(1001, Duration::ZERO)]
    );
    let accepted = store.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].claimed.candidate, frozen);
    assert_eq!(
        serde_json::to_value(&accepted[0].evaluated).unwrap(),
        json!({"version": 1, "decisions": [], "attempts": 0})
    );
    assert_eq!(accepted[0].status_write(), Some(StatusResult::Failure));
    let saved = store.local_answers.lock().unwrap();
    let preparation = &saved.values().next().unwrap().preparation;
    assert_eq!(
        serde_json::to_value(preparation).unwrap()["status"],
        "local"
    );
}

#[test]
fn local_gate_before_start_retains_work_without_starting_cooldown() {
    let active = local_activation("  cooldown: 10s\n", ACTIONS);
    let evaluator = Arc::new(LocalEvaluator::default());
    evaluator.enabled.store(false, Ordering::SeqCst);
    let (store, engine) = local_engine(&active, evaluator.clone());
    engine.enqueue(&active, candidate(&active, 1)).unwrap();
    assert!(matches!(
        engine.start_one_due(&active, 1001).unwrap(),
        StartOutcome::NotDue
    ));
    assert!(store.local_starts.lock().unwrap().is_empty());
    assert_eq!(*store.cooldown_until_unix_ms.lock().unwrap(), 0);
    evaluator.enabled.store(true, Ordering::SeqCst);
    assert_eq!(start(&engine, &active, 1001).outcome, RunOutcome::Accepted);
}

#[test]
fn local_answer_recovers_once_without_restarting_cooldown_or_budget() {
    let active = local_activation("  cooldown: 10s\n", ACTIONS);
    let evaluator = Arc::new(LocalEvaluator::default());
    let (store, engine) = local_engine(&active, evaluator.clone());
    engine.enqueue(&active, candidate(&active, 1)).unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 1001).unwrap() else {
        panic!("start");
    };
    let saved = store
        .local_answers
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .preparation
        .clone();
    evaluator.enabled.store(false, Ordering::SeqCst);
    assert_eq!(run_task(task).outcome, RunOutcome::Deferred);
    let reopened = CheckEngine::new(store.clone(), evaluator.clone());
    for _ in 0..3 {
        reopened.recover_pending().unwrap();
    }
    assert!(store.accepted.lock().unwrap().is_empty());
    assert_eq!(*store.cooldown_until_unix_ms.lock().unwrap(), 11_001);
    evaluator.enabled.store(true, Ordering::SeqCst);
    reopened.recover_pending().unwrap();
    reopened.recover_pending().unwrap();
    assert_eq!(store.accepted.lock().unwrap().len(), 1);
    assert_eq!(
        store
            .local_answers
            .lock()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .preparation,
        saved
    );
    assert_eq!(store.local_starts.lock().unwrap().len(), 1);
    assert_eq!(*store.dispatches.lock().unwrap(), 0);
}

#[test]
fn local_runtime_is_checked_again_after_action_planning() {
    let active = local_activation("", ACTIONS);
    let evaluator = Arc::new(LocalEvaluator {
        blocked_check: 3,
        ..Default::default()
    });
    let (store, engine) = local_engine(&active, evaluator);
    engine.enqueue(&active, candidate(&active, 1)).unwrap();
    assert_eq!(start(&engine, &active, 0).outcome, RunOutcome::Deferred);
    assert!(store.accepted.lock().unwrap().is_empty());
    engine.recover_pending().unwrap();
    assert_eq!(store.accepted.lock().unwrap().len(), 1);
    assert_eq!(store.local_starts.lock().unwrap().len(), 1);
}

#[test]
fn local_recovery_checks_runtime_again_before_acceptance() {
    let active = local_activation("", ACTIONS);
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    engine.enqueue(&active, candidate(&active, 1)).unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 0).unwrap() else {
        panic!("start");
    };
    drop(task);
    let evaluator = Arc::new(LocalEvaluator {
        blocked_check: 2,
        ..Default::default()
    });
    let reopened = CheckEngine::new(store.clone(), evaluator);
    reopened.recover_pending().unwrap();
    assert!(store.accepted.lock().unwrap().is_empty());
    reopened.recover_pending().unwrap();
    assert_eq!(store.accepted.lock().unwrap().len(), 1);
}

#[test]
fn local_positive_cooldown_and_zero_default_keep_single_flight() {
    for (on, next_start) in [("", 1001), ("  cooldown: 10s\n", 11_001)] {
        let active = local_activation(on, ACTIONS);
        let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
        engine.enqueue(&active, candidate(&active, 1)).unwrap();
        let StartOutcome::Started(first) = engine.start_one_due(&active, 1001).unwrap() else {
            panic!("start");
        };
        engine.enqueue(&active, candidate(&active, 2)).unwrap();
        assert!(matches!(
            engine.start_one_due(&active, next_start).unwrap(),
            StartOutcome::NotDue
        ));
        assert_eq!(run_task(first).outcome, RunOutcome::Accepted);
        if next_start > 1001 {
            assert!(matches!(
                engine.start_one_due(&active, next_start - 1).unwrap(),
                StartOutcome::NotDue
            ));
        }
        assert_eq!(
            start(&engine, &active, next_start).outcome,
            RunOutcome::Accepted
        );
        assert_eq!(store.accepted.lock().unwrap().len(), 2);
        assert_eq!(store.local_starts.lock().unwrap().len(), 2);
    }
}

#[test]
fn local_shared_action_budget_failure_cannot_publish_an_earlier_status() {
    let actions = format!(
        "{ACTIONS}  - type: status\n    if: message.tool.arguments.items.*.value\n    input: {{result: success}}\n  - type: status\n    if: message.tool.arguments.items.*.value\n    input: {{result: success}}\n"
    );
    let active = local_activation("  cooldown: 10s\n", &actions);
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    let mut frozen = candidate(&active, 1);
    frozen.context.value["message"]["tool"]["arguments"] =
        json!({"items": (0..600).map(|_| json!({"value": "selected"})).collect::<Vec<_>>()});
    engine.enqueue(&active, frozen).unwrap();
    let settlement = start(&engine, &active, 1001);
    assert!(matches!(settlement.outcome, RunOutcome::Failed(_)));
    assert_eq!(settlement.attempts, 0);
    assert!(store.accepted.lock().unwrap().is_empty());
    assert_eq!(store.local_failures.lock().unwrap().len(), 1);
    assert_eq!(*store.cooldown_until_unix_ms.lock().unwrap(), 11_001);
}

#[test]
fn local_many_small_action_templates_cannot_reset_the_shared_budget() {
    let repeated = "  - type: status\n    input:\n      result: success\n      message: \"${{ join(message.tool.arguments.items, ',') }}\"\n";
    let actions = format!("{ACTIONS}{}", repeated.repeat(600));
    let active = local_activation("", &actions);
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    let mut frozen = candidate(&active, 1);
    frozen.context.value["message"]["tool"]["arguments"] = json!({"items": ["first", "second"]});
    engine.enqueue(&active, frozen).unwrap();
    let result = start(&engine, &active, 0);
    assert!(matches!(result.outcome, RunOutcome::Failed(_)));
    assert_eq!(result.attempts, 0);
    assert!(store.accepted.lock().unwrap().is_empty());
    assert_eq!(store.local_failures.lock().unwrap().len(), 1);
}

#[test]
fn local_selected_membership_and_independent_actions_keep_existing_rules() {
    let actions = "  - type: status\n    if: session.requests[0].text == 'task'\n    input: {result: success}\n  - type: status\n    input: {result: '${{ message.tool.arguments.invalid }}'}\n  - type: status\n    input: {result: failure}\n";
    let active = local_activation("", actions);
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    let mut frozen = candidate(&active, 1);
    frozen.context.value["session"] =
        json!({"requests": [{"text": "task"}], "requests_complete": false});
    frozen.context.value["message"]["tool"]["arguments"] = json!({"invalid": "other"});
    engine.enqueue(&active, frozen).unwrap();
    assert_eq!(start(&engine, &active, 0).outcome, RunOutcome::Accepted);
    let accepted = store.accepted.lock().unwrap();
    assert_eq!(
        accepted[0].actions.outcomes[0].outcome,
        ActionOutcomeKind::SkippedConditionUnavailable
    );
    assert!(matches!(
        accepted[0].actions.outcomes[1].outcome,
        ActionOutcomeKind::Failed(_)
    ));
    assert_eq!(accepted[0].status_write(), Some(StatusResult::Failure));
}

#[test]
fn local_preparation_decode_and_resume_do_not_repair_missing_state() {
    let record = PreparationRecord::Local(LocalPreparation::new(1001));
    let value = serde_json::to_value(&record).unwrap();
    let restored: PreparationRecord = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(restored, record);
    for key in ["version", "started_at_unix_ms", "evaluation_budget"] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<PreparationRecord>(missing).is_err(),
            "{key}"
        );
    }
    let mut extra = value;
    extra["provider"] = json!("invented");
    assert!(serde_json::from_value::<PreparationRecord>(extra).is_err());
    let mut invalid = LocalPreparation::new(0);
    invalid.version = 2;
    assert_eq!(
        invalid.validate_for_resume().unwrap_err().code,
        PreparationFailureCode::IncompatibleProfile
    );
    invalid.version = 1;
    assert!(invalid.evaluation_budget.visit_items(1001).is_err());
    assert!(invalid.validate_for_resume().is_err());
}

#[test]
fn local_and_model_results_and_preparations_cannot_cross() {
    let local = local_activation("", ACTIONS);
    let model = activation();
    let empty = EvaluatedAssessment::local();
    empty
        .validate_for_declarations(
            &local.compiled.decisions,
            &local.compiled.question_requirements(),
        )
        .unwrap();
    assert!(
        empty
            .validate_for_declarations(
                &model.compiled.decisions,
                &model.compiled.question_requirements()
            )
            .is_err()
    );
    let answer = test_answer(&model, &DecisionAttempt::new("review", 1).unwrap(), None);
    assert!(
        EvaluatedAssessment::new(
            &local.compiled.decisions,
            &local.compiled.question_requirements(),
            vec![answer]
        )
        .is_err()
    );
    for invalid in [
        json!({"version": 1, "decisions": [], "attempts": 1}),
        json!({"version": 1, "decisions": [], "attempts": 2}),
    ] {
        assert!(crate::results::decode_results(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    let ready = PreparationRecord::Ready(
        test_prepare(
            &model,
            vec![DecisionInput {
                decision: "review".into(),
                input: json!({}),
            }],
            EvaluationBudget::default(),
        )
        .unwrap(),
    );
    let local_record = PreparationRecord::Local(LocalPreparation::new(1));
    for record in [None, Some(&ready)] {
        assert!(restored_evaluation_budget(&local.compiled, record).is_err());
    }
    assert!(restored_evaluation_budget(&model.compiled, Some(&local_record)).is_err());
    assert!(local.compiled.decisions.is_empty());
}

#[test]
fn local_recovery_isolates_invalid_saved_records_without_model_calls() {
    let active = local_activation("", ACTIONS);
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    for position in 1..=2 {
        engine
            .enqueue(
                &active,
                candidate_on_branch(&active, position, &format!("branch-{position}")),
            )
            .unwrap();
        let StartOutcome::Started(task) = engine.start_one_due(&active, 0).unwrap() else {
            panic!("start");
        };
        drop(task);
    }
    {
        let mut saved = store.local_answers.lock().unwrap();
        let invalid = saved.values_mut().next().unwrap();
        let PreparationRecord::Local(preparation) = &mut invalid.preparation else {
            panic!("local");
        };
        preparation.version = 2;
    }
    let reopened = CheckEngine::new(store.clone(), Arc::new(LocalEvaluator::default()));
    reopened.recover_pending().unwrap();
    assert_eq!(store.accepted.lock().unwrap().len(), 1);
    assert_eq!(store.local_failures.lock().unwrap().len(), 1);
    assert_eq!(store.local_starts.lock().unwrap().len(), 2);
    assert_eq!(*store.dispatches.lock().unwrap(), 0);
}

#[test]
fn local_recovery_restores_spent_budget_instead_of_resetting_it() {
    let actions = format!(
        "{ACTIONS}  - type: status\n    if: message.tool.arguments.items.*.value\n    input: {{result: success}}\n"
    );
    let active = local_activation("", &actions);
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    let mut frozen = candidate(&active, 1);
    frozen.context.value["message"]["tool"]["arguments"] =
        json!({"items": (0..600).map(|_| json!({"value": "selected"})).collect::<Vec<_>>()});
    engine.enqueue(&active, frozen).unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 0).unwrap() else {
        panic!("start");
    };
    drop(task);
    {
        let mut saved = store.local_answers.lock().unwrap();
        let PreparationRecord::Local(preparation) =
            &mut saved.values_mut().next().unwrap().preparation
        else {
            panic!("local");
        };
        preparation.evaluation_budget.visit_items(600).unwrap();
    }
    let reopened = CheckEngine::new(store.clone(), Arc::new(LocalEvaluator::default()));
    reopened.recover_pending().unwrap();
    assert!(store.accepted.lock().unwrap().is_empty());
    assert_eq!(store.local_failures.lock().unwrap().len(), 1);
}

#[test]
fn local_start_storage_failure_restores_unstarted_work() {
    let active = local_activation("  cooldown: 10s\n", ACTIONS);
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    engine.enqueue(&active, candidate(&active, 1)).unwrap();
    *store.fail_local_start.lock().unwrap() = true;
    assert!(engine.start_one_due(&active, 1001).is_err());
    assert!(store.local_answers.lock().unwrap().is_empty());
    assert_eq!(*store.cooldown_until_unix_ms.lock().unwrap(), 0);
    *store.fail_local_start.lock().unwrap() = false;
    assert_eq!(start(&engine, &active, 2001).outcome, RunOutcome::Accepted);
    assert_eq!(
        *store.local_starts.lock().unwrap(),
        [(2001, Duration::from_secs(10))]
    );
}

#[test]
fn stale_local_start_and_acceptance_cannot_publish_status() {
    let active = local_activation("", ACTIONS);
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    engine.enqueue(&active, candidate(&active, 1)).unwrap();
    *store.stale_local_start.lock().unwrap() = true;
    assert!(matches!(
        engine.start_one_due(&active, 0).unwrap(),
        StartOutcome::Stale
    ));
    assert!(store.local_starts.lock().unwrap().is_empty());
    assert!(store.accepted.lock().unwrap().is_empty());
    for (acceptance, outcome) in [
        (AcceptanceOutcome::Stale, RunOutcome::Stale),
        (AcceptanceOutcome::Deferred, RunOutcome::Deferred),
        (AcceptanceOutcome::AlreadyAccepted, RunOutcome::Accepted),
    ] {
        let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
        engine.enqueue(&active, candidate(&active, 1)).unwrap();
        *store.acceptance_override.lock().unwrap() = Some(acceptance);
        assert_eq!(
            start(&engine, &active, 0),
            RunSettlement {
                outcome,
                attempts: 0,
                new_intents: 0,
            }
        );
        assert!(store.accepted.lock().unwrap().is_empty());
    }
}

#[test]
fn local_recovery_propagates_storage_read_errors_without_restarting() {
    let active = local_activation("", ACTIONS);
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    engine.enqueue(&active, candidate(&active, 1)).unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 0).unwrap() else {
        panic!("start");
    };
    drop(task);
    let reopened = CheckEngine::new(store.clone(), Arc::new(LocalEvaluator::default()));
    *store.fail_preparation_load.lock().unwrap() = true;
    assert!(
        reopened
            .recover_pending()
            .unwrap_err()
            .message
            .contains("read failure")
    );
    assert!(store.local_failures.lock().unwrap().is_empty());
    assert!(store.accepted.lock().unwrap().is_empty());
    *store.fail_preparation_load.lock().unwrap() = false;
    reopened.recover_pending().unwrap();
    assert_eq!(store.accepted.lock().unwrap().len(), 1);
    assert_eq!(store.local_starts.lock().unwrap().len(), 1);
}

#[test]
fn local_condition_rejection_does_not_start_cooldown_and_storage_failure_retries() {
    for (condition, expected) in [
        ("false", RunOutcome::Skipped),
        (
            "message.tool.arguments.missing",
            RunOutcome::NotAssessed("Top-level condition depends on unavailable evidence.".into()),
        ),
    ] {
        let mut active = local_activation("  cooldown: 10s\n", ACTIONS);
        Arc::make_mut(&mut active.compiled).condition =
            Some(crate::compiler::Condition::Expression(
                crate::expression::parse_expression(condition).unwrap(),
            ));
        let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
        engine.enqueue(&active, candidate(&active, 1)).unwrap();
        *store.fail_preparation_rejection.lock().unwrap() = true;
        assert!(engine.start_one_due(&active, 0).is_err());
        *store.fail_preparation_rejection.lock().unwrap() = false;
        assert_eq!(
            start(&engine, &active, 0),
            RunSettlement {
                outcome: expected,
                attempts: 0,
                new_intents: 0,
            }
        );
        assert!(store.local_starts.lock().unwrap().is_empty());
        assert_eq!(*store.cooldown_until_unix_ms.lock().unwrap(), 0);
    }
}

fn local_task_error(mut task: EvaluationTask<'_>) -> StoreError {
    let mut context = Context::from_waker(Waker::noop());
    match task.as_mut().poll(&mut context) {
        Poll::Ready(Err(error)) => {
            assert_eq!(
                (
                    error.attempts,
                    error.new_intents,
                    error.intent_commit_uncertain
                ),
                (0, 0, false)
            );
            error.source
        }
        _ => panic!("local storage failure must return an error"),
    }
}

fn assert_local_reservation_released(engine: &CheckEngine<FakeStore, LocalEvaluator>) {
    let state = engine.state.lock().unwrap();
    assert!(state.in_flight.is_empty());
    assert_eq!(state.active_count, 0);
}

fn saved_local_preparation(store: &FakeStore, candidate: &FrozenCandidate) -> PreparationRecord {
    store.local_answers.lock().unwrap()[&CandidateId::from(candidate)]
        .preparation
        .clone()
}

#[test]
fn local_acceptance_error_recovers_on_same_engine_and_releases_branch() {
    let active = local_activation("  cooldown: 10s\n", ACTIONS);
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    let first = candidate(&active, 1);
    let second = candidate(&active, 2);
    engine.enqueue(&active, first.clone()).unwrap();
    *store.fail_acceptance.lock().unwrap() = true;
    let StartOutcome::Started(task) = engine.start_one_due(&active, 1001).unwrap() else {
        panic!("start");
    };
    let preparation = saved_local_preparation(&store, &first);
    assert!(
        local_task_error(task)
            .message
            .contains("acceptance write failure")
    );
    assert_local_reservation_released(&engine);
    assert!(store.accepted.lock().unwrap().is_empty());
    engine.enqueue(&active, second.clone()).unwrap();
    assert!(matches!(
        store.claim_due(&second, 11_001, false).unwrap(),
        CandidateClaim::NotDue { .. }
    ));
    assert_eq!(*store.cooldown_until_unix_ms.lock().unwrap(), 11_001);
    *store.fail_acceptance.lock().unwrap() = false;
    engine.recover_pending().unwrap();
    assert_eq!(store.accepted.lock().unwrap().len(), 1);
    assert_eq!(
        store.accepted.lock().unwrap()[0].claimed.id,
        CandidateId::from(&first)
    );
    assert_eq!(store.local_starts.lock().unwrap().len(), 1);
    assert_eq!(saved_local_preparation(&store, &first), preparation);
    assert_eq!(
        start(&engine, &active, 11_001).outcome,
        RunOutcome::Accepted
    );
    let accepted = store.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 2);
    assert!(
        accepted
            .iter()
            .all(|answer| answer.evaluated.decisions().is_empty()
                && answer.evaluated.attempts() == 0)
    );
    assert_eq!(
        *store.local_starts.lock().unwrap(),
        [
            (1001, Duration::from_secs(10)),
            (11_001, Duration::from_secs(10))
        ]
    );
    assert_eq!(*store.dispatches.lock().unwrap(), 0);
}

#[test]
fn local_unpolled_task_drop_recovers_on_same_engine_and_releases_branch() {
    let active = local_activation("  cooldown: 10s\n", ACTIONS);
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    let first = candidate(&active, 1);
    let second = candidate(&active, 2);
    engine.enqueue(&active, first.clone()).unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 1001).unwrap() else {
        panic!("start");
    };
    let preparation = saved_local_preparation(&store, &first);
    drop(task);
    assert_local_reservation_released(&engine);
    engine.enqueue(&active, second.clone()).unwrap();
    assert!(matches!(
        store.claim_due(&second, 11_001, false).unwrap(),
        CandidateClaim::NotDue { .. }
    ));
    assert!(store.accepted.lock().unwrap().is_empty());
    engine.recover_pending().unwrap();
    engine.recover_pending().unwrap();
    assert_eq!(store.accepted.lock().unwrap().len(), 1);
    assert_eq!(
        store.accepted.lock().unwrap()[0].claimed.id,
        CandidateId::from(&first)
    );
    assert_eq!(store.local_starts.lock().unwrap().len(), 1);
    assert_eq!(*store.cooldown_until_unix_ms.lock().unwrap(), 11_001);
    assert_eq!(saved_local_preparation(&store, &first), preparation);
    assert_eq!(
        start(&engine, &active, 11_001).outcome,
        RunOutcome::Accepted
    );
    let accepted = store.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 2);
    assert!(
        accepted
            .iter()
            .all(|answer| answer.evaluated.decisions().is_empty()
                && answer.evaluated.attempts() == 0)
    );
    assert_eq!(
        *store.local_starts.lock().unwrap(),
        [
            (1001, Duration::from_secs(10)),
            (11_001, Duration::from_secs(10))
        ]
    );
    assert_eq!(*store.dispatches.lock().unwrap(), 0);
}

#[test]
fn local_failure_write_error_recovers_on_same_engine_and_releases_branch() {
    let actions = format!(
        "{ACTIONS}  - type: status\n    if: message.tool.arguments.items.*.value\n    input: {{result: success}}\n  - type: status\n    if: message.tool.arguments.items.*.value\n    input: {{result: success}}\n"
    );
    let active = local_activation("  cooldown: 10s\n", &actions);
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    let mut first = candidate(&active, 1);
    first.context.value["message"]["tool"]["arguments"] =
        json!({"items": (0..600).map(|_| json!({"value": "selected"})).collect::<Vec<_>>()});
    let second = candidate(&active, 2);
    engine.enqueue(&active, first.clone()).unwrap();
    *store.fail_local_failure_write.lock().unwrap() = true;
    let StartOutcome::Started(task) = engine.start_one_due(&active, 1001).unwrap() else {
        panic!("start");
    };
    let preparation = saved_local_preparation(&store, &first);
    assert!(
        local_task_error(task)
            .message
            .contains("local failure write failure")
    );
    assert_local_reservation_released(&engine);
    assert!(store.accepted.lock().unwrap().is_empty());
    assert!(store.local_failures.lock().unwrap().is_empty());
    engine.enqueue(&active, second.clone()).unwrap();
    assert!(matches!(
        store.claim_due(&second, 11_001, false).unwrap(),
        CandidateClaim::NotDue { .. }
    ));
    *store.fail_local_failure_write.lock().unwrap() = false;
    engine.recover_pending().unwrap();
    assert!(store.accepted.lock().unwrap().is_empty());
    assert_eq!(store.local_failures.lock().unwrap().len(), 1);
    assert!(
        store
            .terminal
            .lock()
            .unwrap()
            .contains(&CandidateId::from(&first))
    );
    assert_eq!(store.local_starts.lock().unwrap().len(), 1);
    assert_eq!(*store.cooldown_until_unix_ms.lock().unwrap(), 11_001);
    assert_eq!(saved_local_preparation(&store, &first), preparation);
    assert_eq!(
        start(&engine, &active, 11_001).outcome,
        RunOutcome::Accepted
    );
    let accepted = store.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].claimed.id, CandidateId::from(&second));
    assert!(accepted[0].evaluated.decisions().is_empty());
    assert_eq!(accepted[0].evaluated.attempts(), 0);
    assert_eq!(
        *store.local_starts.lock().unwrap(),
        [
            (1001, Duration::from_secs(10)),
            (11_001, Duration::from_secs(10))
        ]
    );
    assert_eq!(*store.dispatches.lock().unwrap(), 0);
}

#[test]
fn model_task_drop_keeps_unresolved_dispatch_reserved() {
    let active = activation();
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(
        store.clone(),
        Arc::new(PendingEvaluator {
            calls: AtomicUsize::new(0),
        }),
    );
    engine.enqueue(&active, candidate(&active, 1)).unwrap();
    let StartOutcome::Started(mut task) = engine.start_one_due(&active, 1001).unwrap() else {
        panic!("start");
    };
    assert!(
        task.as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    drop(task);
    engine.enqueue(&active, candidate(&active, 2)).unwrap();
    assert!(matches!(
        engine.start_one_due(&active, 11_001).unwrap(),
        StartOutcome::NotDue
    ));
    let state = engine.state.lock().unwrap();
    assert_eq!(state.in_flight.len(), 1);
    assert_eq!(state.active_count, 0);
    assert_eq!(*store.dispatches.lock().unwrap(), 1);
}

#[test]
fn local_top_level_condition_filters_before_window_selection() {
    let active = EffectiveActivation {
        compiled: Arc::new(compile("name: Local window\nif: message.tool.result.status == 'error'\non:\n  tool_result:\n  window: {size: 2, min: 2}\nactions:\n  - type: status\n    input: {result: failure}\n").unwrap()),
        ..local_activation("", ACTIONS)
    };
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    let mut excluded = source_event(1, "excluded");
    let SourceEvent::Observation { observation, .. } = &mut excluded else {
        panic!("observation");
    };
    let ObservationKind::ToolResult {
        message, status, ..
    } = &mut observation.kind
    else {
        panic!("tool");
    };
    message.value["tool"]["result"]["status"] = json!("completed");
    *status = "completed".into();
    let first = engine
        .apply_source_page(
            &active,
            &SourceBatch {
                events: vec![excluded, source_event(2, "included-1")],
                next: PublicationCursor(2),
                has_more: false,
            },
        )
        .unwrap();
    assert_eq!(first.candidates, 0);
    assert!(
        first
            .condition_outcomes
            .iter()
            .any(|(_, condition)| *condition == crate::preparation::ConditionOutcome::False)
    );
    let second = engine
        .apply_source_page(&active, &source_page(3, "included-2"))
        .unwrap();
    assert_eq!(second.candidates, 1);
    assert_eq!(start(&engine, &active, 0).outcome, RunOutcome::Accepted);
    let accepted = store.accepted.lock().unwrap();
    assert_eq!(
        accepted[0]
            .claimed
            .candidate
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["included-1", "included-2"]
    );
}

#[test]
fn local_top_level_condition_keeps_a_separate_bounded_budget() {
    let actions = "  - type: status\n    if: message.tool.arguments.items.*.value\n    input: {result: failure}\n";
    let mut active = local_activation("", actions);
    Arc::make_mut(&mut active.compiled).condition = Some(crate::compiler::Condition::Expression(
        crate::expression::parse_expression("message.tool.arguments.items.*.value").unwrap(),
    ));
    let (store, engine) = local_engine(&active, Arc::new(LocalEvaluator::default()));
    let mut frozen = candidate(&active, 1);
    frozen.context.value["message"]["tool"]["arguments"] =
        json!({"items": (0..600).map(|_| json!({"value": "selected"})).collect::<Vec<_>>()});
    engine.enqueue(&active, frozen).unwrap();
    assert_eq!(start(&engine, &active, 0).outcome, RunOutcome::Accepted);
    assert_eq!(store.accepted.lock().unwrap().len(), 1);
}
