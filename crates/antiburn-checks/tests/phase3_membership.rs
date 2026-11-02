use antiburn_checks::actions::{ActionOutcomeKind, plan_status_actions_with_budget};
use antiburn_checks::compile;
use antiburn_checks::compiler::StatusResult;
use antiburn_checks::domain::{
    ActivationId, ActivationScope, BranchId, FrozenCandidate, FrozenContext, Generation,
    ObservationPosition,
};
use antiburn_checks::expression::{PathSegment, parse_expression};
use antiburn_checks::expression_eval::{
    EvaluationBindings, EvaluationBudget, EvaluationDependencyKind, evaluate_expression_with_budget,
};
use antiburn_checks::preparation::{
    ConditionOutcome, PreparationOutcome, evaluate_decision_input_with_budget,
    evaluate_trigger_condition, prepare_decision_input_with_budget,
};
use antiburn_checks::results::EvaluatedAssessment;
use serde_json::{Value, json};

fn definition(input: &str, actions: &str) -> String {
    format!(
        "name: Membership test\non: {{tool_result: null, window: 3}}\ndecisions:\n  review:\n    input:\n{input}\n    output:\n      verdict: {{type: choice, question: Is this useful?, criteria: {{yes: Useful., no: Not useful.}}}}\nactions:\n{actions}\n"
    )
}

fn candidate(context: Value) -> FrozenCandidate {
    FrozenCandidate {
        activation: ActivationId {
            scope: ActivationScope::default(),
            definition_id: "membership-test".into(),
            generation: Generation(1),
        },
        branch: BranchId("main-session".into()),
        trigger_position: ObservationPosition(1),
        messages: Vec::new(),
        context: FrozenContext {
            value: context,
            dependencies: Vec::new(),
        },
        dependencies: Vec::new(),
        fingerprint: [0; 32],
    }
}

fn context(requests: Value, complete: Option<bool>) -> Value {
    let mut context = json!({
        "session": {"id": "session-1", "requests": requests},
        "message": {"tool": {"arguments": {"items": []}, "result": {"status": "completed", "content": "known result"}}},
        "window": {"messages": [{"tool": {"result": {"status": "completed"}}}]},
    });
    if let Some(complete) = complete {
        context["session"]["requests_complete"] = json!(complete);
    }
    context
}

fn input_for(expression: &str) -> String {
    format!(
        "      value: {}",
        serde_json::to_string(&format!("${{{{ {expression} }}}}")).unwrap()
    )
}

#[test]
fn partial_request_operations_are_unavailable_before_ready() {
    for requests in [json!([]), json!([{"text": "known"}])] {
        for complete in [Some(false), None] {
            let frozen = candidate(context(requests.clone(), complete));
            for input in [
                input_for("session.requests.length"),
                input_for("session.requests.*.text"),
                input_for("session.requests[0].text"),
                "      value: {map: '${{ session.requests }}', as: request, if: false, value: '${{ request.text }}'}".into(),
            ] {
                let check = compile(&definition(&input, "  - type: status\n    input: {result: success}")).unwrap();
                let mut budget = EvaluationBudget::default();
                let prepared = prepare_decision_input_with_budget(&check.decisions[0], &frozen, &mut budget);
                assert!(matches!(prepared, PreparationOutcome::Unavailable(_)), "{input}, {complete:?}: {prepared:?}");
                assert!(!budget.exhausted());
            }
        }
    }
}

#[test]
fn conversion_operations_keep_original_request_membership_dependencies() {
    let expressions = [
        "fromJSON(toJSON(session.requests)).length",
        "fromJSON(toJSON(session.requests))[0].text",
        "fromJSON(toJSON(session.requests)).*.text",
        "fromJSON(toJSON(session)).requests.length",
        "fromJSON(toJSON(session)).requests[0].text",
        "fromJSON(toJSON(session)).requests.*.text",
        "toJSON(session.requests.*.text)",
    ];
    for expression in expressions {
        let frozen = candidate(context(json!([{"text": "known"}]), Some(false)));
        let raw = evaluate_expression_with_budget(
            &parse_expression(expression).unwrap(),
            &frozen.context.value,
            &EvaluationBindings::new(),
            &mut EvaluationBudget::default(),
            1,
        )
        .unwrap();
        assert!(raw.trace.dependencies.iter().any(|dependency| dependency.kind == EvaluationDependencyKind::Membership && matches!(dependency.path.as_slice(), [PathSegment::Property(session), PathSegment::Property(requests)] if session == "session" && requests == "requests")), "missing membership for {expression}: {:?}", raw.trace);
        let check = compile(&definition(
            &input_for(expression),
            "  - type: status\n    input: {result: success}",
        ))
        .unwrap();
        assert!(
            matches!(
                prepare_decision_input_with_budget(
                    &check.decisions[0],
                    &frozen,
                    &mut EvaluationBudget::default()
                ),
                PreparationOutcome::Unavailable(_)
            ),
            "{expression}"
        );
    }
    for expression in [
        "fromJSON(toJSON(session.requests))",
        "fromJSON(toJSON(session)).requests",
    ] {
        let check = compile(&definition(&format!("      value: {{map: '${{{{ {expression} }}}}', as: request, if: false, value: unused}}"), "  - type: status\n    input: {result: success}")).unwrap();
        for complete in [Some(false), None] {
            assert!(
                matches!(
                    prepare_decision_input_with_budget(
                        &check.decisions[0],
                        &candidate(context(json!([]), complete)),
                        &mut EvaluationBudget::default()
                    ),
                    PreparationOutcome::Unavailable(_)
                ),
                "map over {expression}, {complete:?}"
            );
        }
    }
}

#[test]
fn partial_whole_copies_and_unrelated_known_values_remain_available() {
    for expression in [
        "session.requests",
        "session",
        "toJSON(session.requests)",
        "fromJSON(toJSON(session.requests))",
        "fromJSON(toJSON(session)).requests",
        "fromJSON(toJSON(session)).id",
        "message.tool.result.content",
        "window.messages.length",
        "window.messages.*.tool.result.status",
        "message.tool.arguments.items.length",
        "fromJSON(toJSON(message.tool.arguments)).items.length",
        "fromJSON('[]').length",
    ] {
        for complete in [Some(false), None] {
            let check = compile(&definition(
                &input_for(expression),
                "  - type: status\n    input: {result: failure}",
            ))
            .unwrap();
            let prepared = prepare_decision_input_with_budget(
                &check.decisions[0],
                &candidate(context(json!([]), complete)),
                &mut EvaluationBudget::default(),
            );
            assert!(
                matches!(prepared, PreparationOutcome::Ready(_)),
                "{expression}, {complete:?}: {prepared:?}"
            );
        }
    }
}

#[test]
fn complete_membership_allows_empty_operations_and_unused_optional_fields() {
    for expression in [
        "session.requests.length",
        "session.requests.*.text",
        "fromJSON(toJSON(session.requests)).length",
    ] {
        let check = compile(&definition(
            &input_for(expression),
            "  - type: status\n    input: {result: failure}",
        ))
        .unwrap();
        assert!(
            matches!(
                prepare_decision_input_with_budget(
                    &check.decisions[0],
                    &candidate(context(json!([]), Some(true))),
                    &mut EvaluationBudget::default()
                ),
                PreparationOutcome::Ready(_)
            ),
            "{expression}"
        );
    }
    let input = "      value: {map: '${{ session.requests }}', as: request, if: false, value: '${{ request.in_reply_to }}'}";
    let check = compile(&definition(
        input,
        "  - type: status\n    input: {result: failure}",
    ))
    .unwrap();
    assert_eq!(
        prepare_decision_input_with_budget(
            &check.decisions[0],
            &candidate(context(json!([{"text": "known"}]), Some(true))),
            &mut EvaluationBudget::default()
        ),
        PreparationOutcome::Ready(json!({"value": []}))
    );
    let evaluated = evaluate_decision_input_with_budget(
        &check.decisions[0],
        &context(json!([{"text": "known"}]), Some(true)),
        &mut EvaluationBudget::default(),
    )
    .unwrap();
    assert!(!evaluated.trace.dependencies.iter().any(|dependency| matches!(dependency.path.last(), Some(PathSegment::Property(field)) if field == "in_reply_to")));
}

#[test]
fn request_membership_skips_only_the_affected_actions() {
    let actions = r#"  - type: status
    if: session.requests.length == 0
    input: {result: success}
  - type: status
    input: {result: "${{ fromJSON(toJSON(session.requests)).length == 0 && 'success' || 'failure' }}"}
  - type: status
    input: {result: success, message: "Count: ${{ fromJSON(toJSON(session)).requests.length }}"}"#;
    let check = compile(&definition("      value: true", actions)).unwrap();
    for complete in [Some(false), None] {
        let frozen = context(json!([]), complete);
        let mut budget = EvaluationBudget::default();
        let plan = plan_status_actions_with_budget(
            &check.actions,
            &frozen,
            &EvaluatedAssessment::local(),
            Some(StatusResult::Failure),
            &mut budget,
        )
        .unwrap();
        assert_eq!(plan.current_status, Some(StatusResult::Failure));
        assert_eq!(
            plan.outcomes[0].outcome,
            ActionOutcomeKind::SkippedConditionUnavailable
        );
        assert_eq!(
            plan.outcomes[1].outcome,
            ActionOutcomeKind::SkippedInputUnavailable
        );
        assert_eq!(
            plan.outcomes[2].outcome,
            ActionOutcomeKind::SkippedInputUnavailable
        );
        assert!(!budget.exhausted());

        let independent = compile(&definition("      value: true", &format!("{actions}\n  - type: status\n    if: message.tool.result.status == 'completed'\n    input: {{result: success}}"))).unwrap();
        let plan = plan_status_actions_with_budget(
            &independent.actions,
            &frozen,
            &EvaluatedAssessment::local(),
            Some(StatusResult::Failure),
            &mut EvaluationBudget::default(),
        )
        .unwrap();
        assert_eq!(plan.current_status, Some(StatusResult::Success));
        assert!(matches!(
            plan.outcomes[3].outcome,
            ActionOutcomeKind::Applied { .. }
        ));
    }
}

#[test]
fn preselection_membership_conditions_use_frozen_quality_facts() {
    let mut check = compile(&definition(
        "      value: true",
        "  - type: status\n    input: {result: success}",
    ))
    .unwrap();
    check.condition = Some(antiburn_checks::compiler::Condition::Expression(
        parse_expression("session.requests.length == 0").unwrap(),
    ));
    for complete in [Some(false), None] {
        assert_eq!(
            evaluate_trigger_condition(&check, &context(json!([]), complete)),
            ConditionOutcome::Unavailable
        );
    }
    assert_eq!(
        evaluate_trigger_condition(&check, &context(json!([]), Some(true))),
        ConditionOutcome::Matched
    );
}
