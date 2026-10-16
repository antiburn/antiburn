use std::collections::BTreeMap;

use antiburn_checks::actions::{ActionOutcomeKind, ChoiceResult, plan_status_actions};
use antiburn_checks::compiler::{Condition, StatusAction, StatusResult};
use antiburn_checks::diagnostics::DiagnosticCode;
use antiburn_checks::expression::parse_expression;
use antiburn_checks::expression_eval::{
    EvaluationLimits, EvaluationValue, evaluate_expression, evaluate_expression_with_limits,
};
use serde_json::{Value, json};

fn eval(source: &str, context: &Value) -> EvaluationValue {
    evaluate_expression(
        &parse_expression(source).expect("valid expression"),
        context,
    )
    .expect("expression evaluates")
}

fn status(condition: Option<Condition>, result: StatusResult) -> StatusAction {
    StatusAction {
        condition,
        result,
        message: None,
    }
}

fn expression_condition(source: &str) -> Condition {
    Condition::Expression(parse_expression(source).expect("valid condition"))
}

#[test]
fn distinguishes_missing_paths_from_explicit_null() {
    let context = json!({"event": {"present": null}});
    assert_eq!(
        eval("event.present", &context),
        EvaluationValue::Available(Value::Null)
    );
    assert_eq!(
        eval("event.missing", &context),
        EvaluationValue::Unavailable
    );
    assert_eq!(
        eval("event.present == null", &context),
        EvaluationValue::Available(json!(true))
    );
    assert_eq!(
        eval("event.missing == null", &context),
        EvaluationValue::Unavailable
    );
}

#[test]
fn logical_operators_short_circuit_and_return_the_selected_operand() {
    let context = json!({});
    assert_eq!(
        eval("'selected' || missing.path", &context),
        EvaluationValue::Available(json!("selected"))
    );
    assert_eq!(
        eval("false && missing.path", &context),
        EvaluationValue::Available(json!(false))
    );
    assert_eq!(
        eval("'' || 'fallback'", &context),
        EvaluationValue::Available(json!("fallback"))
    );
    assert_eq!(
        eval("missing.path || 'fallback'", &context),
        EvaluationValue::Unavailable
    );
}

#[test]
fn compares_truthiness_case_and_cross_type_values_by_contract() {
    let context = json!({});
    assert_eq!(
        eval("null == false", &context),
        EvaluationValue::Available(json!(true))
    );
    assert_eq!(
        eval("true == '1'", &context),
        EvaluationValue::Available(json!(true))
    );
    assert_eq!(
        eval("'2.5' != 3", &context),
        EvaluationValue::Available(json!(true))
    );
    assert_eq!(
        eval("'not a number' == 0", &context),
        EvaluationValue::Available(json!(false))
    );
    assert_eq!(
        eval("'Alpha' == 'aLPHa'", &context),
        EvaluationValue::Available(json!(true))
    );
    assert_eq!(
        eval("'Alpha' < 'beta'", &context),
        EvaluationValue::Available(json!(true))
    );
    assert_eq!(
        eval("1 < true", &context),
        EvaluationValue::Available(json!(false))
    );
}

#[test]
fn compares_composite_values_by_frozen_path_identity() {
    let context = json!({"data": {"first": [1], "second": [1]}});
    assert_eq!(
        eval("data.first == data.first", &context),
        EvaluationValue::Available(json!(true))
    );
    assert_eq!(
        eval("data.first == data.second", &context),
        EvaluationValue::Available(json!(false))
    );
    assert_eq!(
        eval("data.first != data.second", &context),
        EvaluationValue::Available(json!(true))
    );
}

#[test]
fn compares_large_integers_and_integer_float_boundaries_exactly() {
    let context: Value = serde_json::from_str(
        r#"{"values":{"lower":9007199254740992,"upper":9007199254740993,"float":9007199254740992.0}}"#,
    )
    .unwrap();
    assert_eq!(
        eval("values.lower == values.upper", &context),
        EvaluationValue::Available(json!(false))
    );
    assert_eq!(
        eval("values.lower < values.upper", &context),
        EvaluationValue::Available(json!(true))
    );
    assert_eq!(
        eval("values.upper > values.float", &context),
        EvaluationValue::Available(json!(true))
    );
    assert_eq!(
        eval("values.lower == values.float", &context),
        EvaluationValue::Available(json!(true))
    );
    assert_eq!(
        eval("'9007199254740993' == values.upper", &context),
        EvaluationValue::Available(json!(true))
    );
}

#[test]
fn accesses_only_safe_json_paths_and_supports_array_length() {
    let context =
        json!({"data": {"length": "actual", "items": [4, 5], "constructor": "own value"}});
    assert_eq!(
        eval("data.length", &context),
        EvaluationValue::Available(json!("actual"))
    );
    assert_eq!(
        eval("data.items.length", &context),
        EvaluationValue::Available(json!(2))
    );
    assert_eq!(
        eval("data.items[1]", &context),
        EvaluationValue::Available(json!(5))
    );
    assert_eq!(
        eval("data.items[2]", &context),
        EvaluationValue::Unavailable
    );
    assert_eq!(
        eval("data.constructor", &context),
        EvaluationValue::Available(json!("own value"))
    );
    for path in [
        "data.items.length.foo",
        "data.items.length[0]",
        "data.items.length.length",
    ] {
        assert_eq!(eval(path, &context), EvaluationValue::Unavailable, "{path}");
    }
    assert_eq!(
        eval("data.length.length", &context),
        EvaluationValue::Unavailable
    );
}

#[test]
fn enforces_explicit_evaluation_depth_and_step_limits() {
    let expression = parse_expression("!(!(!true))").unwrap();
    let depth_error = evaluate_expression_with_limits(
        &expression,
        &json!({}),
        EvaluationLimits {
            max_depth: 2,
            max_steps: 100,
        },
    )
    .expect_err("depth limit");
    assert!(depth_error.message.contains("depth limit"));
    assert_eq!(depth_error.code, DiagnosticCode::EvaluationLimitExceeded);
    let step_error = evaluate_expression_with_limits(
        &parse_expression("root.branch.value").unwrap(),
        &json!({"root": {"branch": {"value": 1}}}),
        EvaluationLimits {
            max_depth: 32,
            max_steps: 2,
        },
    )
    .expect_err("step limit");
    assert!(step_error.message.contains("step limit"));
}

#[test]
fn validates_choice_labels_and_probability_maps_without_label_semantics() {
    let labels = vec!["uncertain".to_owned(), "clear".to_owned()];
    let probabilities =
        BTreeMap::from([("uncertain".to_owned(), 0.25), ("clear".to_owned(), 0.75)]);
    let choice = ChoiceResult::validate("review", "verdict", &labels, "uncertain", probabilities)
        .expect("declared label and probabilities are valid");
    let actions = [status(
        Some(expression_condition(
            "decisions.review.outputs.verdict == 'uncertain'",
        )),
        StatusResult::Failure,
    )];
    let plan = plan_status_actions(&actions, &json!({}), &[choice], None).unwrap();
    assert_eq!(plan.current_status, Some(StatusResult::Failure));

    let invalid = BTreeMap::from([("uncertain".to_owned(), 0.2), ("clear".to_owned(), 0.2)]);
    assert!(ChoiceResult::validate("review", "verdict", &labels, "clear", invalid).is_err());
    let invalid_label =
        BTreeMap::from([("uncertain".to_owned(), 0.25), ("clear".to_owned(), 0.75)]);
    assert!(ChoiceResult::validate("review", "verdict", &labels, "other", invalid_label).is_err());
}

#[test]
fn persisted_choice_results_cannot_bypass_probability_validation() {
    let valid = json!({"decision": "review", "question": "verdict", "output": "yes", "probabilities": {"yes": 0.99, "no": 0.01}});
    assert!(serde_json::from_value::<ChoiceResult>(valid.clone()).is_ok());
    for (field, value) in [
        ("probabilities", json!({"yes": 1.2, "no": -0.2})),
        ("probabilities", json!({"yes": 0.1, "no": 0.1})),
        ("probabilities", json!({"yes": 1.0})),
        ("output", json!("undeclared")),
        ("decision", json!("")),
    ] {
        let mut saved = valid.clone();
        saved[field] = value;
        assert!(
            serde_json::from_value::<ChoiceResult>(saved).is_err(),
            "{field}"
        );
    }
}

#[test]
fn plans_each_action_against_one_context_and_last_valid_write_wins() {
    let labels = vec!["yes".to_owned(), "no".to_owned()];
    let choice = ChoiceResult::validate(
        "review",
        "answer",
        &labels,
        "yes",
        BTreeMap::from([("yes".to_owned(), 0.9), ("no".to_owned(), 0.1)]),
    )
    .unwrap();
    let actions = [
        status(
            Some(expression_condition(
                "decisions.review.details.answer.probabilities.yes >= 0.8",
            )),
            StatusResult::Failure,
        ),
        status(
            Some(expression_condition("missing.value")),
            StatusResult::Success,
        ),
        status(Some(Condition::Boolean(false)), StatusResult::Success),
        status(
            Some(expression_condition(
                "decisions.review.outputs.answer == 'yes'",
            )),
            StatusResult::Success,
        ),
    ];
    let plan = plan_status_actions(&actions, &json!({"frozen": true}), &[choice], None).unwrap();
    assert_eq!(plan.current_status, Some(StatusResult::Success));
    assert_eq!(plan.outcomes.len(), actions.len());
    assert_eq!(
        plan.outcomes[0].outcome,
        ActionOutcomeKind::Applied {
            result: StatusResult::Failure,
            message: None,
        }
    );
    assert_eq!(
        plan.outcomes[1].outcome,
        ActionOutcomeKind::SkippedConditionUnavailable
    );
    assert_eq!(
        plan.outcomes[2].outcome,
        ActionOutcomeKind::SkippedConditionFalse
    );
    assert_eq!(
        plan.outcomes[3].outcome,
        ActionOutcomeKind::Applied {
            result: StatusResult::Success,
            message: None,
        }
    );
}

#[test]
fn preserves_prior_status_when_every_action_skips() {
    let actions = [
        status(Some(Condition::Boolean(false)), StatusResult::Success),
        status(
            Some(expression_condition("unknown.field")),
            StatusResult::Failure,
        ),
    ];
    let plan = plan_status_actions(&actions, &json!({}), &[], Some(StatusResult::Failure)).unwrap();
    assert_eq!(plan.current_status, Some(StatusResult::Failure));
    assert!(plan.outcomes.iter().all(|outcome| matches!(
        outcome.outcome,
        ActionOutcomeKind::SkippedConditionFalse | ActionOutcomeKind::SkippedConditionUnavailable
    )));
}

#[test]
fn action_evaluation_errors_are_recorded_and_do_not_block_later_actions() {
    let nested = format!("{}true{}", "!".repeat(40), "");
    let too_deep = status(Some(expression_condition(&nested)), StatusResult::Failure);
    let later = status(Some(Condition::Boolean(true)), StatusResult::Success);
    let actions = [too_deep, later];
    let plan = plan_status_actions(&actions, &json!({}), &[], None).unwrap();
    assert_eq!(plan.current_status, Some(StatusResult::Success));
    assert_eq!(plan.outcomes.len(), 2);
    assert!(matches!(
        plan.outcomes[0].outcome,
        ActionOutcomeKind::Failed(_)
    ));
    assert!(matches!(
        plan.outcomes[1].outcome,
        ActionOutcomeKind::Applied { .. }
    ));
}

#[test]
fn rejects_duplicate_decision_results() {
    let labels = vec!["yes".to_owned(), "no".to_owned()];
    let create_choice = || {
        ChoiceResult::validate(
            "review",
            "answer",
            &labels,
            "yes",
            BTreeMap::from([("yes".to_owned(), 1.0), ("no".to_owned(), 0.0)]),
        )
        .unwrap()
    };
    assert!(
        plan_status_actions(&[], &json!({}), &[create_choice(), create_choice()], None).is_err()
    );
}

#[test]
fn rejects_invalid_action_context_roots() {
    assert!(plan_status_actions(&[], &json!(null), &[], None).is_err());
    assert!(plan_status_actions(&[], &json!({"session": {}, "decisions": {}}), &[], None).is_err());
}
