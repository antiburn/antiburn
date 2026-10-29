use super::*;

fn budget_activation() -> EffectiveActivation {
    let mut activation = activation();
    activation.compiled = Arc::new(
        compile(
            "name: Shared budget\non:\n  tool_result:\ndecisions:\n  review:\n    input:\n      selected: ${{ message.tool.arguments.items.*.value }}\n    output:\n      verdict:\n        type: choice\n        question: Is the work useful?\n        criteria: {yes: Useful., no: Not useful.}\nactions:\n  - type: status\n    input: {result: failure}\n  - type: status\n    if: message.tool.arguments.items.*.value\n    input: {result: success}\n",
        )
        .unwrap(),
    );
    activation
}

fn budget_candidate(activation: &EffectiveActivation) -> FrozenCandidate {
    let mut candidate = candidate(activation, 1);
    candidate.context.value["message"]["tool"]["arguments"] = json!({
        "items": (0..600).map(|_| json!({"value": "selected"})).collect::<Vec<_>>()
    });
    candidate
}

#[test]
fn preparation_and_actions_share_the_iteration_budget_before_acceptance() {
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let activation = budget_activation();
    engine
        .enqueue(&activation, budget_candidate(&activation))
        .unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&activation, 0).unwrap() else {
        panic!("the complete candidate must start");
    };
    let settlement = run_task(task);
    assert!(matches!(settlement.outcome, RunOutcome::Failed(_)));
    assert_eq!(settlement.attempts, 1);
    assert_eq!(*store.dispatches.lock().unwrap(), 1);
    assert!(store.accepted.lock().unwrap().is_empty());
}

#[test]
fn saved_recovery_keeps_input_costs_and_rejects_missing_budgets() {
    let activation = budget_activation();
    let candidate = budget_candidate(&activation);
    assert!(
        plan_status_actions_with_budget(
            &activation.compiled.actions,
            &candidate.context.value,
            &EvaluatedAssessment::local(),
            None,
            &mut EvaluationBudget::default(),
        )
        .is_ok()
    );
    let mut budget = EvaluationBudget::default();
    let PreparationOutcome::Ready(input) = prepare_decision_input_with_budget(
        &activation.compiled.decisions[0],
        &candidate,
        &mut budget,
    ) else {
        panic!("the input alone fits the run budget");
    };
    let prepared = PreparationRecord::Ready(
        test_prepare(
            &activation,
            vec![DecisionInput {
                decision: "review".into(),
                input,
            }],
            budget,
        )
        .unwrap(),
    );
    let saved =
        crate::preparation::decode_preparation_record(&serde_json::to_vec(&prepared).unwrap())
            .unwrap();
    let mut budget = restored_evaluation_budget(&activation.compiled, Some(&saved)).unwrap();
    assert!(
        plan_status_actions_with_budget(
            &activation.compiled.actions,
            &candidate.context.value,
            &EvaluatedAssessment::local(),
            None,
            &mut budget
        )
        .is_err()
    );
    assert!(restored_evaluation_budget(&activation.compiled, None).is_err());
}

#[test]
fn recovery_rejects_changed_or_exhausted_saved_evaluation_limits() {
    let activation = budget_activation();
    let changed = EvaluationBudget::new(
        crate::expression_eval::EvaluationLimits {
            max_steps: 1,
            ..Default::default()
        },
        1_000,
        8 * 1024 * 1024,
    );
    let mut exhausted = EvaluationBudget::default();
    assert!(exhausted.visit_items(1_001).is_err());
    for budget in [changed, exhausted] {
        let mut prepared = test_prepare(
            &activation,
            vec![DecisionInput {
                decision: "review".into(),
                input: json!({}),
            }],
            EvaluationBudget::default(),
        )
        .unwrap();
        prepared.evaluation_budget = budget;
        let preparation = PreparationRecord::Ready(prepared);
        assert!(
            crate::preparation::decode_preparation_record(
                &serde_json::to_vec(&preparation).unwrap()
            )
            .is_err()
        );
        assert!(restored_evaluation_budget(&activation.compiled, Some(&preparation)).is_err());
    }
}
