use serde_json::{Value, json};

use super::*;
use crate::actions::{ActionOutcomeKind, plan_status_actions_with_budget};
use crate::compiler::{StatusResult, compile};
use crate::expression::parse_expression;
use crate::preparation::evaluate_decision_input_with_budget;
use crate::results::EvaluatedAssessment;

const TRANSFORMED_COUNTS: &[&str] = &[
    "fromJSON(format('{0}', toJSON(session.requests))).length",
    "fromJSON(format('{{\"wrapped\":{0}}}', toJSON(session.requests))).wrapped.length",
    "fromJSON(format('{{\"wrapped\":{0}}}', toJSON(session))).wrapped.requests.length",
    "fromJSON(join(toJSON(session.requests), '')).length",
    "fromJSON(join(fromJSON('[\"\",\"\"]'), toJSON(session.requests))).length",
    "fromJSON(fromJSON(format('{{\"encoded\":{0}}}', toJSON(toJSON(session.requests)))).encoded).length",
    "fromJSON(fromJSON(toJSON(format('{0}', toJSON(session.requests))))).length",
    "fromJSON(format('{0}', join(fromJSON('[\"\",\"\"]'), format('{0}', toJSON(session.requests))))).length",
    "fromJSON(format('{0}', toJSON(fromJSON(format('{0}', toJSON(session.requests)))))).length",
    "fromJSON((true && format('{0}', toJSON(session.requests))) || '[]').length",
    "fromJSON(join(fromJSON(format('[{0}]', toJSON(toJSON(session.requests)))), '')).length",
];

fn context(requests: Value, complete: bool) -> Value {
    json!({
        "session": {"id": "[]", "requests": requests, "requests_complete": complete},
        "message": {"tool": {"arguments": {"items": []}, "result": {"status": "completed"}}},
        "window": {"messages": []},
    })
}

fn evaluate(source: &str, context: &Value) -> EvaluatedValue {
    evaluate_expression_with_budget(
        &parse_expression(source).unwrap(),
        context,
        &EvaluationBindings::new(),
        &mut EvaluationBudget::default(),
        1,
    )
    .unwrap()
}

fn selects_request_membership(trace: &EvaluationTrace) -> bool {
    trace.dependencies.iter().any(|dependency| {
        dependency.kind == EvaluationDependencyKind::Membership
            && dependency.path
                == [
                    PathSegment::Property("session".to_owned()),
                    PathSegment::Property("requests".to_owned()),
                ]
    })
}

fn definition(input: &str, actions: &str) -> String {
    format!(
        "name: Text provenance\non: {{tool_result: null, window: 3}}\ndecisions:\n  review:\n    input:\n      value: {input}\n    output:\n      verdict: {{type: choice, question: Is this useful?, criteria: {{yes: Useful., no: Not useful.}}}}\nactions:\n{actions}\n"
    )
}

fn expression_input(expression: &str) -> String {
    serde_json::to_string(&format!("${{{{ {expression} }}}}")).unwrap()
}

#[test]
fn text_transforms_retain_selected_collection_sources_before_membership_reads() {
    for requests in [json!([]), json!([{"text": "known"}])] {
        let context = context(requests, false);
        for expression in TRANSFORMED_COUNTS {
            let value = evaluate(expression, &context);
            assert!(
                selects_request_membership(&value.trace),
                "{expression}: {:?}",
                value.trace
            );
        }
    }
    let context = context(json!([]), false);
    let value = evaluate(
        "fromJSON(format(toJSON(session.requests), 'unused')).length",
        &context,
    );
    assert!(selects_request_membership(&value.trace));
    for suffix in ["[0]", ".*", "[0].*.length"] {
        let expression = format!(
            "{}{suffix}",
            "fromJSON(format('{{\"wrapped\":{0}}}', toJSON(session.requests))).wrapped"
        );
        let value = evaluate(&expression, &context);
        assert!(
            selects_request_membership(&value.trace),
            "{suffix}: {:?}",
            value.trace
        );
    }
}

#[test]
fn production_input_and_map_boundaries_reject_tainted_partial_membership() {
    let action = "  - type: status\n    input: {result: failure}";
    for expression in TRANSFORMED_COUNTS {
        let check = compile(&definition(&expression_input(expression), action)).unwrap();
        for complete in [false, true] {
            let mut budget = EvaluationBudget::default();
            let value = evaluate_decision_input_with_budget(
                &check.decisions[0],
                &context(json!([]), complete),
                &mut budget,
            )
            .unwrap();
            assert_eq!(
                matches!(value.value, EvaluationValue::Available(_)),
                complete,
                "{expression}"
            );
            assert!(!budget.exhausted());
        }
    }
    for expression in [
        "fromJSON(format('{0}', toJSON(session.requests)))",
        "fromJSON(format('{{\"wrapped\":{0}}}', toJSON(session))).wrapped.requests",
        "fromJSON(join(fromJSON('[\"\",\"\"]'), toJSON(session.requests)))",
    ] {
        let input = format!(
            "{{map: {}, as: request, if: false, value: unused}}",
            expression_input(expression)
        );
        let check = compile(&definition(&input, action)).unwrap();
        let value = evaluate_decision_input_with_budget(
            &check.decisions[0],
            &context(json!([]), false),
            &mut EvaluationBudget::default(),
        )
        .unwrap();
        assert_eq!(value.value, EvaluationValue::Unavailable, "{expression}");
    }
}

#[test]
fn tainted_action_conditions_results_and_messages_skip_without_blocking_independent_actions() {
    for expression in TRANSFORMED_COUNTS {
        let condition = serde_json::to_string(&format!("{expression} == 0")).unwrap();
        let result = expression_input(&format!("{expression} == 0 && 'success' || 'failure'"));
        let message = serde_json::to_string(&format!("Count: ${{{{ {expression} }}}}")).unwrap();
        let actions = format!(
            "  - type: status\n    if: {condition}\n    input: {{result: success}}\n  - type: status\n    input: {{result: {result}}}\n  - type: status\n    input: {{result: success, message: {message}}}\n  - type: status\n    if: message.tool.result.status == 'completed'\n    input: {{result: failure}}"
        );
        let check = compile(&format!(
            "name: Text provenance\non: {{tool_result: null, window: 3}}\nactions:\n{actions}\n"
        ))
        .unwrap();
        let evaluated = EvaluatedAssessment::local();
        evaluated
            .validate_for_declarations(&check.decisions, &check.question_requirements())
            .unwrap();
        let plan = plan_status_actions_with_budget(
            &check.actions,
            &context(json!([]), false),
            &evaluated,
            Some(StatusResult::Failure),
            &mut EvaluationBudget::default(),
        )
        .unwrap();
        assert_eq!(
            plan.outcomes[0].outcome,
            ActionOutcomeKind::SkippedConditionUnavailable,
            "{expression}"
        );
        assert_eq!(
            plan.outcomes[1].outcome,
            ActionOutcomeKind::SkippedInputUnavailable,
            "{expression}"
        );
        assert_eq!(
            plan.outcomes[2].outcome,
            ActionOutcomeKind::SkippedInputUnavailable,
            "{expression}"
        );
        assert!(matches!(
            plan.outcomes[3].outcome,
            ActionOutcomeKind::Applied {
                result: StatusResult::Failure,
                ..
            }
        ));
        assert_eq!(plan.current_status, Some(StatusResult::Failure));
        let complete = plan_status_actions_with_budget(
            &check.actions,
            &context(json!([]), true),
            &evaluated,
            Some(StatusResult::Failure),
            &mut EvaluationBudget::default(),
        )
        .unwrap();
        assert!(
            complete.outcomes[..3].iter().all(|outcome| matches!(
                outcome.outcome,
                ActionOutcomeKind::Applied {
                    result: StatusResult::Success,
                    ..
                }
            )),
            "{expression}"
        );
    }
}

#[test]
fn string_join_retains_eager_separator_membership_in_inputs_and_local_actions() {
    for (separator, requires_membership, reads_requests) in [
        ("session.requests.length", true, true),
        ("fromJSON(toJSON(session)).requests.length", true, true),
        ("true || session.requests.length", false, false),
        ("toJSON(session.requests)", false, true),
    ] {
        let result = expression_input(&format!("join('success', {separator})"));
        let check = compile(&definition(
            &result,
            "  - type: status\n    input: {result: failure}",
        ))
        .unwrap();
        let condition =
            serde_json::to_string(&format!("join('yes', {separator}) == 'yes'")).unwrap();
        let message = expression_input(&format!("join('Ready.', {separator})"));
        let local = compile(&format!(
            "name: String join\non: {{tool_result: null}}\nactions:\n  - type: status\n    if: {condition}\n    input: {{result: success}}\n  - type: status\n    input: {{result: {result}}}\n  - type: status\n    input: {{result: success, message: {message}}}\n"
        ))
        .unwrap();
        assert!(local.is_local());
        for requests in [Some(json!([])), Some(json!([{"text": "known"}])), None] {
            for complete in [Some(false), None, Some(true)] {
                let mut context = context(
                    requests.clone().unwrap_or_else(|| json!([])),
                    complete == Some(true),
                );
                if requests.is_none() {
                    context["session"]
                        .as_object_mut()
                        .unwrap()
                        .remove("requests");
                }
                if complete.is_none() {
                    context["session"]
                        .as_object_mut()
                        .unwrap()
                        .remove("requests_complete");
                }
                let available = (!reads_requests || requests.is_some())
                    && (!requires_membership || complete == Some(true));
                let input = evaluate_decision_input_with_budget(
                    &check.decisions[0],
                    &context,
                    &mut EvaluationBudget::default(),
                )
                .unwrap();
                assert_eq!(
                    input.value,
                    if available {
                        EvaluationValue::Available(json!({"value": "success"}))
                    } else {
                        EvaluationValue::Unavailable
                    },
                    "{separator}, {complete:?}"
                );
                let plan = plan_status_actions_with_budget(
                    &local.actions,
                    &context,
                    &EvaluatedAssessment::local(),
                    Some(StatusResult::Failure),
                    &mut EvaluationBudget::default(),
                )
                .unwrap();
                assert_eq!(
                    plan.current_status,
                    Some(if available {
                        StatusResult::Success
                    } else {
                        StatusResult::Failure
                    }),
                    "{separator}, {complete:?}"
                );
                if available {
                    assert!(plan.outcomes.iter().all(|outcome| matches!(
                        outcome.outcome,
                        ActionOutcomeKind::Applied {
                            result: StatusResult::Success,
                            ..
                        }
                    )));
                } else {
                    assert_eq!(
                        plan.outcomes[0].outcome,
                        ActionOutcomeKind::SkippedConditionUnavailable
                    );
                    for outcome in &plan.outcomes[1..] {
                        assert_eq!(outcome.outcome, ActionOutcomeKind::SkippedInputUnavailable);
                    }
                }
            }
        }
    }
}

#[test]
fn string_join_preserves_first_argument_identity_and_text_provenance() {
    let context = context(json!([]), false);
    for source in [
        "session.id",
        "toJSON(session)",
        "format('{0}', toJSON(session.requests))",
    ] {
        let original = evaluate(source, &context);
        let joined = evaluate(
            &format!("join({source}, toJSON(session.requests))"),
            &context,
        );
        assert_eq!(joined.value, original.value, "{source}");
        assert_eq!(
            joined.metadata.identity, original.metadata.identity,
            "{source}"
        );
        assert_eq!(
            joined.metadata.encoded_provenance, original.metadata.encoded_provenance,
            "{source}"
        );
        assert_eq!(
            joined.metadata.text_sources, original.metadata.text_sources,
            "{source}"
        );
        assert!(joined.trace.dependencies.contains(&EvaluationDependency {
            path: vec![
                PathSegment::Property("session".into()),
                PathSegment::Property("requests".into()),
            ],
            kind: EvaluationDependencyKind::Value,
        }));
    }
}

#[test]
fn whole_transformed_copies_and_unrelated_or_unused_values_remain_available() {
    let context = context(json!([]), false);
    for expression in [
        "format('{0}', toJSON(session.requests))",
        "format('{{\"wrapped\":{0}}}', toJSON(session))",
        "fromJSON(format('{0}', toJSON(session.requests)))",
        "fromJSON(format('{{\"wrapped\":{0}}}', toJSON(session)))",
        "fromJSON(join(fromJSON('[\"\",\"\"]'), toJSON(session.requests)))",
        "fromJSON(format('{0}', toJSON(message.tool.arguments.items))).length",
        "fromJSON(format('{0}', toJSON(window.messages))).length",
        "fromJSON(format('[]', toJSON(session.requests))).length",
        "fromJSON(join('[]', toJSON(session.requests))).length",
        "fromJSON(fromJSON(join(toJSON(session), '')).id).length",
        "fromJSON(fromJSON(toJSON(session)).id).length",
        "fromJSON(fromJSON(toJSON(toJSON(session)))).id",
        "fromJSON(true && '[]' || toJSON(session.requests)).length",
    ] {
        let value = evaluate(expression, &context);
        assert!(
            !selects_request_membership(&value.trace),
            "{expression}: {:?}",
            value.trace
        );
        let check = compile(&definition(
            &expression_input(expression),
            "  - type: status\n    input: {result: failure}",
        ))
        .unwrap();
        let input = evaluate_decision_input_with_budget(
            &check.decisions[0],
            &context,
            &mut EvaluationBudget::default(),
        )
        .unwrap();
        assert!(
            matches!(input.value, EvaluationValue::Available(_)),
            "{expression}"
        );
    }
    let input = format!(
        "{{map: {}, as: row, if: false, value: {}}}",
        expression_input("message.tool.arguments.items"),
        expression_input(TRANSFORMED_COUNTS[0])
    );
    let check = compile(&definition(
        &input,
        "  - type: status\n    input: {result: failure}",
    ))
    .unwrap();
    let value = evaluate_decision_input_with_budget(
        &check.decisions[0],
        &context,
        &mut EvaluationBudget::default(),
    )
    .unwrap();
    assert_eq!(
        value.value,
        EvaluationValue::Available(json!({"value": []}))
    );
    assert!(!value.trace.dependencies.iter().any(|dependency| matches!(dependency.path.first(), Some(PathSegment::Property(root)) if root == "session")));
}

#[test]
fn composed_text_sources_keep_taints_without_restoring_frozen_identity() {
    let context = context(json!([]), false);
    let source = evaluate("toJSON(session.requests)", &context);
    let mut composed = EvaluatedValue::available(json!("[]"));
    composed.merge_text_provenance(&source);
    assert!(!selects_request_membership(&composed.trace));
    let bindings = EvaluationBindings::from([("text".to_owned(), composed)]);
    let value = evaluate_expression_with_budget(
        &parse_expression("fromJSON(text).length").unwrap(),
        &context,
        &bindings,
        &mut EvaluationBudget::default(),
        1,
    )
    .unwrap();
    assert!(selects_request_membership(&value.trace));
    let parsed = evaluate(
        "fromJSON(format('{0}', toJSON(session.requests)))",
        &context,
    );
    let bindings = EvaluationBindings::from([("parsed".to_owned(), parsed)]);
    for (expression, expected) in [
        ("parsed == parsed", true),
        ("parsed == session.requests", false),
    ] {
        let value = evaluate_expression_with_budget(
            &parse_expression(expression).unwrap(),
            &context,
            &bindings,
            &mut EvaluationBudget::default(),
            1,
        )
        .unwrap();
        assert_eq!(value.value, EvaluationValue::Available(json!(expected)));
    }
}
