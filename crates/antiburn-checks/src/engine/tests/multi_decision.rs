use super::*;
use crate::check_store::{DecisionAttemptOutcome, DecisionProgressTerminal, SavedDecisionAttempt};
use std::collections::VecDeque;

const TWO_BLOCKS: &str = "name: Sequential\non:\n  tool_result:\n  cooldown: 10s\ndecisions:\n  first:\n    input: {activity: '${{ message.tool.result.status }}'}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Useful., no: Not useful.}\n  second:\n    input: {activity: '${{ message.tool.result.status }}'}\n    output:\n      verdict:\n        type: choice\n        question: Is this complete?\n        criteria: {yes: Complete., no: Incomplete.}\nactions:\n  - type: status\n    if: decisions.first.outputs.verdict == 'yes' && decisions.second.outputs.verdict == 'yes'\n    input: {result: success}\n";

#[derive(Default)]
struct SequentialEvaluator {
    outcomes: Mutex<VecDeque<Result<(), AttemptFailure>>>,
    requests: Mutex<Vec<(DecisionAttempt, Value)>>,
    preparations: AtomicUsize,
    publication_only: bool,
    reject_after_calls: Option<usize>,
    pending_second: bool,
    failure_diagnostic: Option<EvaluationDiagnostic>,
}

impl SequentialEvaluator {
    fn scripted(outcomes: Vec<Result<(), AttemptFailure>>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into()),
            ..Default::default()
        }
    }
}

impl DecisionEvaluator for SequentialEvaluator {
    fn prepare(
        &self,
        activation: &EffectiveActivation,
        inputs: Vec<DecisionInput>,
        budget: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        assert!(!self.publication_only, "publication must not prepare");
        self.preparations.fetch_add(1, Ordering::SeqCst);
        test_prepare(activation, inputs, budget)
    }

    fn validate_prepared(
        &self,
        activation: &EffectiveActivation,
        prepared: &PreparedAssessment,
        purpose: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        if self.publication_only {
            assert_eq!(purpose, PreparationValidationPurpose::Publication);
        }
        if purpose == PreparationValidationPurpose::Dispatch
            && self
                .reject_after_calls
                .is_some_and(|limit| self.requests.lock().unwrap().len() >= limit)
        {
            return Err(incompatible_preparation("The selected profile changed."));
        }
        test_validate(activation, prepared, purpose)
    }

    fn evaluate_prepared<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        prepared: &'a PreparedAssessment,
        attempt: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        assert!(!self.publication_only, "publication must not infer");
        let block = prepared
            .decisions
            .iter()
            .find(|block| block.decision == attempt.decision)
            .unwrap();
        self.requests
            .lock()
            .unwrap()
            .push((attempt.clone(), block.request.clone()));
        if self.pending_second && attempt.decision == "second" {
            return Box::pin(std::future::pending());
        }
        let next = self
            .outcomes
            .lock()
            .unwrap()
            .pop_front()
            .expect("one response per dispatch");
        let diagnostic = self
            .failure_diagnostic
            .clone()
            .unwrap_or_else(|| EvaluationDiagnostic {
                code: EvaluationFailureCode::ProviderUnavailable,
                detail: "Synthetic provider failure.".into(),
            });
        Box::pin(async move {
            next.map_err(|kind| EvaluationFailure {
                code: diagnostic.code,
                kind,
                detail: diagnostic.detail,
                attempts: 1,
            })?;
            Ok(test_answer(activation, attempt, Some("yes")))
        })
    }
}

#[test]
fn permanent_terminal_failures_keep_the_original_diagnostic_and_safe_detail() {
    for (code, detail) in [
        (
            EvaluationFailureCode::AuthenticationRejected,
            "The provider rejected authentication. Update the saved credential.",
        ),
        (
            EvaluationFailureCode::ContextRejected,
            "The provider rejected the prepared request context.",
        ),
        (
            EvaluationFailureCode::InvalidResponse,
            "The provider response does not match the declared answer schema.",
        ),
    ] {
        let active = active();
        let diagnostic = EvaluationDiagnostic {
            code,
            detail: detail.into(),
        };
        let evaluator = Arc::new(SequentialEvaluator {
            outcomes: Mutex::new(vec![Err(AttemptFailure::Permanent)].into()),
            failure_diagnostic: Some(diagnostic.clone()),
            ..Default::default()
        });
        let (store, engine) = setup(&active, evaluator.clone());
        let settled = run_task(start(&engine, &active, u64::MAX));
        assert_eq!(settled.outcome, RunOutcome::Failed(detail.into()));
        assert_eq!((settled.attempts, settled.new_intents), (1, 1));
        assert_eq!(*store.evaluation_failures.lock().unwrap(), [diagnostic]);
        assert_eq!(evaluator.requests.lock().unwrap().len(), 1);
        assert!(store.accepted.lock().unwrap().is_empty());
    }
}

#[test]
fn exhausted_rate_limit_keeps_the_original_terminal_diagnostic_after_live_or_resumed_retry() {
    for resumed in [false, true] {
        let active = active();
        let diagnostic = EvaluationDiagnostic {
            code: EvaluationFailureCode::RateLimited,
            detail: "The provider rate limit still rejects this request. Try again later.".into(),
        };
        let evaluator = Arc::new(SequentialEvaluator {
            outcomes: Mutex::new(
                vec![
                    Err(AttemptFailure::SafeTransient),
                    Err(AttemptFailure::SafeTransient),
                ]
                .into(),
            ),
            failure_diagnostic: Some(diagnostic.clone()),
            ..Default::default()
        });
        let (store, engine) = setup(&active, evaluator.clone());
        let settled = if resumed {
            let mut task = start(&engine, &active, u64::MAX);
            poll_boundary(&mut task);
            drop(task);
            let reopened = CheckEngine::new(store.clone(), evaluator.clone());
            assert_eq!(reopened.recover_pending().unwrap().candidates, 1);
            run_task(start(&reopened, &active, u64::MAX))
        } else {
            run_task(start(&engine, &active, u64::MAX))
        };
        assert_eq!(
            settled.outcome,
            RunOutcome::Failed(diagnostic.detail.clone())
        );
        assert_eq!(
            (settled.attempts, settled.new_intents),
            (2, if resumed { 1 } else { 2 })
        );
        assert_eq!(*store.evaluation_failures.lock().unwrap(), [diagnostic]);
        let requests = evaluator.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .map(|(attempt, _)| (attempt.decision.as_str(), attempt.number))
                .collect::<Vec<_>>(),
            [("first", 1), ("first", 2)]
        );
        assert!(store.accepted.lock().unwrap().is_empty());
    }
}

#[test]
fn unknown_delivery_keeps_the_safe_timeout_detail_without_another_send() {
    let active = active();
    let diagnostic = EvaluationDiagnostic {
        code: EvaluationFailureCode::DeliveryUnknown,
        detail: "The decision request timed out. Delivery is unknown.".into(),
    };
    let evaluator = Arc::new(SequentialEvaluator {
        outcomes: Mutex::new(vec![Err(AttemptFailure::Unknown)].into()),
        failure_diagnostic: Some(diagnostic.clone()),
        ..Default::default()
    });
    let (store, engine) = setup(&active, evaluator.clone());
    let settled = run_task(start(&engine, &active, u64::MAX));
    assert_eq!(
        settled.outcome,
        RunOutcome::Failed(diagnostic.detail.clone())
    );
    assert_eq!((settled.attempts, settled.new_intents), (1, 1));
    assert_eq!(*store.evaluation_failures.lock().unwrap(), [diagnostic]);
    assert_eq!(evaluator.requests.lock().unwrap().len(), 1);
    assert!(store.accepted.lock().unwrap().is_empty());
}

fn active() -> EffectiveActivation {
    let mut active = activation();
    active.compiled = Arc::new(compile(TWO_BLOCKS).unwrap());
    active
}

fn setup(
    active: &EffectiveActivation,
    evaluator: Arc<SequentialEvaluator>,
) -> (Arc<FakeStore>, CheckEngine<FakeStore, SequentialEvaluator>) {
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), evaluator);
    engine
        .persist_activation(
            &active.id.definition_id,
            &crate::definition_lifecycle::LifecycleTransition::Activated(active.clone()),
            Some(active),
        )
        .unwrap();
    engine.enqueue(active, candidate(active, 1)).unwrap();
    (store, engine)
}

fn start<'a>(
    engine: &'a CheckEngine<FakeStore, SequentialEvaluator>,
    active: &EffectiveActivation,
    allowance: u64,
) -> EvaluationTask<'a> {
    let StartOutcome::Started(task) = engine
        .start_one_due_with_attempt_limit(active, 100, allowance)
        .unwrap()
    else {
        panic!("candidate must start");
    };
    task
}

fn poll_boundary(task: &mut EvaluationTask<'_>) {
    assert!(
        task.as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
}

fn task_error(mut task: EvaluationTask<'_>) -> EvaluationTaskError {
    let mut context = Context::from_waker(Waker::noop());
    for _ in 0..16 {
        match task.as_mut().poll(&mut context) {
            Poll::Ready(Err(error)) => return error,
            Poll::Ready(Ok(result)) => panic!("expected a Store error: {result:?}"),
            Poll::Pending => {}
        }
    }
    panic!("task did not settle");
}

#[test]
fn sequential_retry_reuses_all_prepared_requests_and_publishes_once() {
    let active = active();
    let evaluator = Arc::new(SequentialEvaluator::scripted(vec![
        Ok(()),
        Err(AttemptFailure::SafeTransient),
        Ok(()),
    ]));
    let (store, engine) = setup(&active, evaluator.clone());
    let settled = run_task(start(&engine, &active, u64::MAX));
    assert_eq!(
        settled,
        RunSettlement {
            outcome: RunOutcome::Accepted,
            attempts: 3,
            new_intents: 3
        }
    );
    let requests = evaluator.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|(attempt, _)| (attempt.decision.as_str(), attempt.number))
            .collect::<Vec<_>>(),
        [("first", 1), ("second", 1), ("second", 2)]
    );
    assert_eq!(requests[1].1, requests[2].1);
    assert_eq!(evaluator.preparations.load(Ordering::SeqCst), 1);
    let accepted = store.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].evaluated.attempts(), 3);
    assert_eq!(
        accepted[0].status_write(),
        Some(crate::compiler::StatusResult::Success)
    );
}

#[test]
fn mixed_question_types_stay_in_one_block_request_and_complete_result() {
    let mut active = active();
    let source = TWO_BLOCKS.replace("  second:\n", "      score:\n        type: score\n        question: How complete is this?\n        criteria: [Incomplete., Complete.]\n      confidence:\n        type: noul\n        question: Is the evidence sufficient?\n  second:\n");
    active.compiled = Arc::new(compile(&source).unwrap());
    let evaluator = Arc::new(SequentialEvaluator::scripted(vec![Ok(()), Ok(())]));
    let (store, engine) = setup(&active, evaluator.clone());
    assert_eq!(
        run_task(start(&engine, &active, 2)).outcome,
        RunOutcome::Accepted
    );
    assert_eq!(evaluator.requests.lock().unwrap().len(), 2);
    let accepted = store.accepted.lock().unwrap();
    let answers = accepted[0].evaluated.decisions()[0].answers();
    assert_eq!(answers.len(), 3);
    assert!(matches!(
        answers[0].result(),
        crate::results::QuestionResult::Choice(_)
    ));
    assert!(matches!(
        answers[1].result(),
        crate::results::QuestionResult::Score(_)
    ));
    assert!(matches!(
        answers[2].result(),
        crate::results::QuestionResult::Noul(_)
    ));
}

#[test]
fn restart_after_safe_second_failure_sends_only_second_attempt_two() {
    let active = active();
    let evaluator = Arc::new(SequentialEvaluator::scripted(vec![
        Ok(()),
        Err(AttemptFailure::SafeTransient),
        Ok(()),
    ]));
    let (store, engine) = setup(&active, evaluator.clone());
    let mut task = start(&engine, &active, u64::MAX);
    poll_boundary(&mut task);
    poll_boundary(&mut task);
    drop(task);
    assert!(store.accepted.lock().unwrap().is_empty());
    let prepared = store.preparations.lock().unwrap().clone();
    let reopened = CheckEngine::new(store.clone(), evaluator.clone());
    assert_eq!(reopened.recover_pending().unwrap().candidates, 1);
    let settled = run_task(start(&reopened, &active, 1));
    assert_eq!((settled.attempts, settled.new_intents), (3, 1));
    assert_eq!(settled.outcome, RunOutcome::Accepted);
    assert_eq!(*store.preparations.lock().unwrap(), prepared);
    assert_eq!(evaluator.preparations.load(Ordering::SeqCst), 1);
    assert_eq!(evaluator.requests.lock().unwrap().len(), 3);
}

#[test]
fn unknown_second_never_resends_a_sibling_or_publishes_actions() {
    let active = active();
    let evaluator = Arc::new(SequentialEvaluator::scripted(vec![
        Ok(()),
        Err(AttemptFailure::Unknown),
    ]));
    let (store, engine) = setup(&active, evaluator.clone());
    let settled = run_task(start(&engine, &active, u64::MAX));
    assert!(matches!(settled.outcome, RunOutcome::Failed(_)));
    assert_eq!((settled.attempts, settled.new_intents), (2, 2));
    assert!(store.accepted.lock().unwrap().is_empty());
    let reopened = CheckEngine::new(store.clone(), evaluator.clone());
    reopened.enqueue(&active, candidate(&active, 1)).unwrap();
    let settled = run_task(start(&reopened, &active, u64::MAX));
    assert_eq!((settled.attempts, settled.new_intents), (2, 0));
    assert_eq!(evaluator.requests.lock().unwrap().len(), 2);
}

#[test]
fn later_input_failure_prevents_all_provider_preparation_and_intents() {
    let mut active = active();
    active.compiled = Arc::new(
        compile(&TWO_BLOCKS.replacen(
            "  second:\n    input: {activity: '${{ message.tool.result.status }}'}",
            "  second:\n    input: {activity: '${{ message.tool.arguments.missing }}'}",
            1,
        ))
        .unwrap(),
    );
    let evaluator = Arc::new(SequentialEvaluator::default());
    let (store, engine) = setup(&active, evaluator.clone());
    let settled = run_task(start(&engine, &active, 2));
    assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
    assert_eq!((settled.attempts, settled.new_intents), (0, 0));
    assert_eq!(evaluator.preparations.load(Ordering::SeqCst), 0);
    assert_eq!(*store.dispatches.lock().unwrap(), 0);
}

#[test]
fn finite_allowance_reserves_unfinished_blocks_and_is_terminal() {
    for (allowance, outcomes, expected) in [
        (1, vec![], 0),
        (2, vec![Err(AttemptFailure::SafeTransient), Ok(())], 2),
    ] {
        let active = active();
        let evaluator = Arc::new(SequentialEvaluator::scripted(outcomes));
        let (store, engine) = setup(&active, evaluator.clone());
        let settled = run_task(start(&engine, &active, allowance));
        assert_eq!(settled.outcome, RunOutcome::LimitReached);
        assert_eq!(
            (settled.attempts, settled.new_intents),
            (expected, expected)
        );
        assert_eq!(evaluator.requests.lock().unwrap().len() as u64, expected);
        assert!(store.accepted.lock().unwrap().is_empty());
        assert_eq!(engine.recover_pending().unwrap().candidates, 0);
        engine.enqueue(&active, candidate(&active, 1)).unwrap();
        assert!(matches!(
            engine.start_one_due(&active, 100_000).unwrap(),
            StartOutcome::Stale
        ));
    }
}

#[test]
fn dropping_before_an_intent_refunds_all_sends_and_starts_no_cooldown() {
    let active = active();
    let evaluator = Arc::new(SequentialEvaluator::scripted(vec![Ok(()), Ok(())]));
    let (store, engine) = setup(&active, evaluator.clone());
    drop(start(&engine, &active, 2));
    assert_eq!(*store.dispatches.lock().unwrap(), 0);
    assert_eq!(*store.cooldown_until_unix_ms.lock().unwrap(), 0);
    let settled = run_task(start(&engine, &active, 2));
    assert_eq!((settled.attempts, settled.new_intents), (2, 2));
    assert_eq!(evaluator.preparations.load(Ordering::SeqCst), 1);
}

#[test]
fn fully_answered_restart_uses_only_provider_free_publication_validation() {
    let active = active();
    let evaluator = Arc::new(SequentialEvaluator::scripted(vec![Ok(()), Ok(())]));
    let (store, engine) = setup(&active, evaluator);
    let mut task = start(&engine, &active, 2);
    poll_boundary(&mut task);
    poll_boundary(&mut task);
    drop(task);
    let publication = Arc::new(SequentialEvaluator {
        publication_only: true,
        ..Default::default()
    });
    let reopened = CheckEngine::new(store.clone(), publication.clone());
    reopened.recover_pending().unwrap();
    reopened.recover_pending().unwrap();
    assert_eq!(store.accepted.lock().unwrap().len(), 1);
    assert_eq!(*store.dispatches.lock().unwrap(), 2);
    assert_eq!(publication.preparations.load(Ordering::SeqCst), 0);
}

#[test]
fn complete_saved_progress_reports_durable_total_and_zero_new_intents() {
    let active = active();
    let evaluator = Arc::new(SequentialEvaluator::scripted(vec![Ok(()), Ok(())]));
    let (store, engine) = setup(&active, evaluator);
    let mut task = start(&engine, &active, 2);
    poll_boundary(&mut task);
    poll_boundary(&mut task);
    drop(task);
    store.local_answers.lock().unwrap().clear();
    let publication = Arc::new(SequentialEvaluator {
        publication_only: true,
        ..Default::default()
    });
    let reopened = CheckEngine::new(store.clone(), publication);
    reopened.enqueue(&active, candidate(&active, 1)).unwrap();
    let settled = run_task(start(&reopened, &active, 0));
    assert_eq!(
        settled,
        RunSettlement {
            outcome: RunOutcome::Accepted,
            attempts: 2,
            new_intents: 0
        }
    );
    assert_eq!(*store.dispatches.lock().unwrap(), 2);
}

#[test]
fn invalid_saved_model_budget_is_row_local_during_answer_recovery() {
    let mut active = active();
    Arc::make_mut(&mut active.compiled).trigger.cooldown = Duration::ZERO;
    let evaluator = Arc::new(SequentialEvaluator::scripted(vec![
        Ok(()),
        Ok(()),
        Ok(()),
        Ok(()),
    ]));
    let (store, engine) = setup(&active, evaluator.clone());
    for trigger in 1..=2 {
        if trigger == 2 {
            engine
                .enqueue(&active, candidate_on_branch(&active, trigger, "other"))
                .unwrap();
        }
        let mut task = start(&engine, &active, 2);
        poll_boundary(&mut task);
        poll_boundary(&mut task);
        drop(task);
    }
    let id = CandidateId::from(&candidate(&active, 1));
    let mut preparations = store.preparations.lock().unwrap();
    let PreparationRecord::Ready(invalid) = preparations.get_mut(&id).unwrap() else {
        panic!("model preparation");
    };
    invalid.evaluation_budget = EvaluationBudget::new(
        crate::expression_eval::EvaluationLimits {
            max_steps: 1,
            ..Default::default()
        },
        1000,
        8 * 1024 * 1024,
    );
    drop(preparations);
    let reopened = CheckEngine::new(
        store.clone(),
        Arc::new(SequentialEvaluator {
            publication_only: true,
            ..Default::default()
        }),
    );
    reopened.recover_pending().unwrap();
    assert_eq!(store.accepted.lock().unwrap().len(), 1);
    assert_eq!(*store.dispatches.lock().unwrap(), 4);
    assert!(store.terminal.lock().unwrap().contains(&id));
}

#[test]
fn deferred_action_replanning_uses_the_same_saved_post_input_budget() {
    let source = TWO_BLOCKS.replace("activity: '${{ message.tool.result.status }}'", "activity: '${{ message.tool.arguments.items.*.value }}'")
        .replace("if: decisions.first.outputs.verdict == 'yes' && decisions.second.outputs.verdict == 'yes'", "if: message.tool.arguments.items.*.value");
    let mut active = active();
    active.compiled = Arc::new(compile(&source).unwrap());
    let evaluator = Arc::new(SequentialEvaluator::scripted(vec![Ok(()), Ok(())]));
    let (store, engine) = setup(&active, evaluator.clone());
    let mut frozen = candidate(&active, 1);
    frozen.context.value["message"]["tool"]["arguments"] =
        json!({"items": (0..300).map(|_| json!({"value": "selected"})).collect::<Vec<_>>()});
    engine.state.lock().unwrap().pending.clear();
    engine.enqueue(&active, frozen).unwrap();
    *store.acceptance_override.lock().unwrap() = Some(AcceptanceOutcome::Deferred);
    assert_eq!(
        run_task(start(&engine, &active, 2)).outcome,
        RunOutcome::Deferred
    );
    let original = store.preparations.lock().unwrap().clone();
    for _ in 0..3 {
        engine.recover_pending().unwrap();
    }
    assert!(store.local_failures.lock().unwrap().is_empty());
    assert_eq!(*store.preparations.lock().unwrap(), original);
    *store.acceptance_override.lock().unwrap() = None;
    engine.recover_pending().unwrap();
    assert_eq!(store.accepted.lock().unwrap().len(), 1);
    assert_eq!(evaluator.requests.lock().unwrap().len(), 2);
}

#[test]
fn all_block_inputs_share_one_expression_budget_before_provider_preparation() {
    let mut active = active();
    active.compiled = Arc::new(
        compile(&TWO_BLOCKS.replace(
            "activity: '${{ message.tool.result.status }}'",
            "activity: '${{ message.tool.arguments.items.*.value }}'",
        ))
        .unwrap(),
    );
    let evaluator = Arc::new(SequentialEvaluator::default());
    let (store, engine) = setup(&active, evaluator.clone());
    engine.state.lock().unwrap().pending.clear();
    let mut frozen = candidate(&active, 1);
    frozen.context.value["message"]["tool"]["arguments"] =
        json!({"items": (0..600).map(|_| json!({"value": "selected"})).collect::<Vec<_>>()});
    engine.enqueue(&active, frozen).unwrap();
    assert!(matches!(
        run_task(start(&engine, &active, 2)).outcome,
        RunOutcome::NotAssessed(_)
    ));
    assert_eq!(evaluator.preparations.load(Ordering::SeqCst), 0);
    assert_eq!(*store.dispatches.lock().unwrap(), 0);
}

#[test]
fn partial_rolling_work_survives_newer_candidates_and_safe_cancellation() {
    let mut active = active();
    Arc::make_mut(&mut active.compiled).trigger.window = Some(crate::compiler::RollingWindow {
        size: 1,
        minimum: 1,
    });
    let evaluator = Arc::new(SequentialEvaluator::scripted(vec![Ok(()), Ok(())]));
    let (_, engine) = setup(&active, evaluator);
    let mut task = start(&engine, &active, 2);
    poll_boundary(&mut task);
    drop(task);
    assert_eq!(
        engine.enqueue(&active, candidate(&active, 2)).unwrap(),
        EnqueueOutcome::Queued
    );
    assert_eq!(
        engine
            .state
            .lock()
            .unwrap()
            .pending
            .values()
            .map(BTreeMap::len)
            .sum::<usize>(),
        2
    );
    assert_eq!(
        run_task(start(&engine, &active, 1)).outcome,
        RunOutcome::Accepted
    );
}

#[test]
fn selected_profile_recheck_and_stale_settlement_stop_the_next_block() {
    for stale in [false, true] {
        let active = active();
        let evaluator = Arc::new(SequentialEvaluator {
            outcomes: Mutex::new(vec![Ok(())].into()),
            reject_after_calls: (!stale).then_some(1),
            ..Default::default()
        });
        let (store, engine) = setup(&active, evaluator.clone());
        *store.stale_answer.lock().unwrap() = stale;
        let settled = run_task(start(&engine, &active, 2));
        assert_eq!((settled.attempts, settled.new_intents), (1, 1));
        assert_eq!(evaluator.requests.lock().unwrap().len(), 1);
        if stale {
            assert_eq!(settled.outcome, RunOutcome::Stale);
        } else {
            assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
        }
        assert!(store.accepted.lock().unwrap().is_empty());
    }
}

#[test]
fn task_store_errors_keep_committed_counts_and_uncertain_intent_fact() {
    for (intent_failure, expected) in [(true, (2, true)), (false, (1, false))] {
        let active = active();
        let evaluator = Arc::new(SequentialEvaluator::scripted(vec![
            Ok(()),
            Err(AttemptFailure::SafeTransient),
        ]));
        let (store, engine) = setup(&active, evaluator);
        if intent_failure {
            *store.fail_intent_on_call.lock().unwrap() = Some(3);
        } else {
            *store.fail_answer.lock().unwrap() = true;
        }
        let error = task_error(start(&engine, &active, 3));
        assert_eq!(
            (
                error.attempts,
                error.new_intents,
                error.intent_commit_uncertain
            ),
            (expected.0, expected.0, expected.1)
        );
        assert!(store.accepted.lock().unwrap().is_empty());
        assert!(!engine.state.lock().unwrap().in_flight.is_empty());
    }
}

#[test]
fn answered_model_store_error_releases_the_branch_and_recovers_without_sends() {
    let active = active();
    let evaluator = Arc::new(SequentialEvaluator::scripted(vec![Ok(()), Ok(())]));
    let (store, engine) = setup(&active, evaluator.clone());
    *store.fail_acceptance.lock().unwrap() = true;
    let error = task_error(start(&engine, &active, 2));
    assert_eq!(
        (
            error.attempts,
            error.new_intents,
            error.intent_commit_uncertain
        ),
        (2, 2, false)
    );
    assert!(engine.state.lock().unwrap().in_flight.is_empty());
    assert!(engine.state.lock().unwrap().pending.is_empty());
    *store.fail_acceptance.lock().unwrap() = false;
    engine.recover_pending().unwrap();
    assert_eq!(store.accepted.lock().unwrap().len(), 1);
    assert_eq!(evaluator.requests.lock().unwrap().len(), 2);
}

#[test]
fn unresolved_second_intent_keeps_the_branch_reserved_after_drop() {
    let active = active();
    let evaluator = Arc::new(SequentialEvaluator {
        outcomes: Mutex::new(vec![Ok(())].into()),
        pending_second: true,
        ..Default::default()
    });
    let (_, engine) = setup(&active, evaluator);
    let mut task = start(&engine, &active, 2);
    poll_boundary(&mut task);
    poll_boundary(&mut task);
    drop(task);
    engine.enqueue(&active, candidate(&active, 2)).unwrap();
    assert!(matches!(
        engine.start_one_due(&active, 100_000).unwrap(),
        StartOutcome::NotDue
    ));
}

#[test]
fn progress_validation_rejects_holes_names_duplicates_and_false_answer_counts() {
    let active = active();
    let row = |name: &str, number, outcome| SavedDecisionAttempt {
        attempt: DecisionAttempt::new(name, number).unwrap(),
        outcome,
    };
    let first = test_answer(&active, &DecisionAttempt::new("first", 1).unwrap(), None);
    let invalid = vec![
        vec![row("first", 2, DecisionAttemptOutcome::SafeTransient)],
        vec![row("unknown", 1, DecisionAttemptOutcome::Unknown)],
        vec![
            row("first", 1, DecisionAttemptOutcome::SafeTransient),
            row("first", 1, DecisionAttemptOutcome::SafeTransient),
        ],
        vec![row("second", 1, DecisionAttemptOutcome::Unknown)],
        vec![
            row("first", 1, DecisionAttemptOutcome::Unknown),
            row("second", 1, DecisionAttemptOutcome::Unknown),
        ],
        vec![
            row("first", 1, DecisionAttemptOutcome::Answer(first.clone())),
            row("first", 2, DecisionAttemptOutcome::Unknown),
        ],
        vec![
            row("first", 1, DecisionAttemptOutcome::SafeTransient),
            row("first", 2, DecisionAttemptOutcome::Answer(first)),
        ],
    ];
    for attempts in invalid {
        assert!(
            validate_decision_progress(
                &active.compiled.decisions,
                &active.compiled.question_requirements(),
                &DecisionProgress { attempts }
            )
            .is_err()
        );
    }
    for (outcome, terminal) in [
        (
            DecisionAttemptOutcome::DispatchIntent,
            DecisionProgressTerminal::UnresolvedIntent,
        ),
        (
            DecisionAttemptOutcome::Unknown,
            DecisionProgressTerminal::Unknown,
        ),
        (
            DecisionAttemptOutcome::Permanent,
            DecisionProgressTerminal::Permanent,
        ),
    ] {
        let validated = validate_decision_progress(
            &active.compiled.decisions,
            &active.compiled.question_requirements(),
            &DecisionProgress {
                attempts: vec![row("first", 1, outcome)],
            },
        )
        .unwrap();
        assert_eq!(validated.attempts, 1);
        assert_eq!(validated.state, DecisionProgressState::Terminal(terminal));
    }
}

#[test]
fn durable_progress_totals_exceed_u8_without_losing_exact_block_attempts() {
    let mut active = active();
    let block = active.compiled.decisions[0].clone();
    Arc::make_mut(&mut active.compiled).decisions = (0..130)
        .map(|index| {
            let mut block = block.clone();
            block.name = format!("block_{index}");
            block
        })
        .collect();
    let mut rows = Vec::new();
    for block in &active.compiled.decisions {
        rows.push(SavedDecisionAttempt {
            attempt: DecisionAttempt::new(&block.name, 1).unwrap(),
            outcome: DecisionAttemptOutcome::SafeTransient,
        });
        let attempt = DecisionAttempt::new(&block.name, 2).unwrap();
        rows.push(SavedDecisionAttempt {
            outcome: DecisionAttemptOutcome::Answer(test_answer(&active, &attempt, None)),
            attempt,
        });
    }
    rows.reverse();
    let requirements = active.compiled.question_requirements();
    let validated = validate_decision_progress(
        &active.compiled.decisions,
        &requirements,
        &DecisionProgress {
            attempts: rows.clone(),
        },
    )
    .unwrap();
    assert_eq!(validated.attempts, 260);
    let DecisionProgressState::Complete(assessment) = validated.state else {
        panic!("all blocks answered");
    };
    assert_eq!(assessment.attempts(), 260);
    rows.remove(0);
    let validated = validate_decision_progress(
        &active.compiled.decisions,
        &requirements,
        &DecisionProgress { attempts: rows },
    )
    .unwrap();
    assert_eq!(validated.attempts, 259);
    assert_eq!(
        validated.state,
        DecisionProgressState::Next {
            attempt: DecisionAttempt::new("block_129", 2).unwrap(),
            unfinished_decisions: 1
        }
    );
}

#[test]
fn normal_live_start_persists_256_independent_blocks_without_wrapping_counts() {
    let mut source = "name: Many small decisions\non:\n  tool_result:\ndecisions:\n".to_owned();
    for index in 0..256 {
        source.push_str(&format!("  block_{index}:\n    input: {{activity: selected}}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {{yes: Useful., no: Not useful.}}\n"));
    }
    source.push_str("actions:\n  - type: status\n    input: {result: success}\n");
    let mut active = active();
    active.compiled = Arc::new(compile(&source).unwrap());
    let evaluator = Arc::new(SequentialEvaluator::scripted(vec![Ok(()); 256]));
    let (store, engine) = setup(&active, evaluator.clone());
    let StartOutcome::Started(mut task) = engine.start_one_due(&active, 100).unwrap() else {
        panic!("the normal live start must accept all admitted blocks");
    };
    let mut settlement = None;
    for _ in 0..=256 {
        match task.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(result) => {
                settlement = Some(result.unwrap());
                break;
            }
            Poll::Pending => {}
        }
    }
    assert_eq!(
        settlement.expect("all synthetic blocks must settle"),
        RunSettlement {
            outcome: RunOutcome::Accepted,
            attempts: 256,
            new_intents: 256,
        }
    );
    let rows = store.decision_progress.lock().unwrap();
    let progress = rows.values().next().unwrap();
    assert_eq!(progress.attempts.len(), 256);
    assert!(
        progress.attempts.iter().all(|row| row.attempt.number == 1
            && matches!(row.outcome, DecisionAttemptOutcome::Answer(_)))
    );
    let accepted = store.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].evaluated.decisions().len(), 256);
    assert_eq!(accepted[0].evaluated.attempts(), 256);
    assert_eq!(evaluator.requests.lock().unwrap().len(), 256);
    let preparations = store.preparations.lock().unwrap();
    let PreparationRecord::Ready(prepared) = preparations.values().next().unwrap() else {
        panic!("saved model preparation");
    };
    prepared.validate().unwrap();
    assert_eq!(prepared.decisions.len(), 256);
    crate::results::measure_results(&accepted[0].evaluated).unwrap();
}
