use std::collections::BTreeMap;

use antiburn_checks::actions::{ActionOutcomeKind, plan_status_actions};
use antiburn_checks::compiler::{Condition, StatusAction, StatusResult};
use antiburn_checks::expression::parse_expression;
use antiburn_checks::results::{EvaluatedAssessment, decode_results};
use serde_json::json;

pub mod support;
use support::assessed_choice;

fn labels() -> Vec<String> {
    vec!["stuck".into(), "clear".into(), "uncertain".into()]
}

fn label_only(output: &str) -> EvaluatedAssessment {
    assessed_choice("review", "verdict", &labels(), output, None).unwrap()
}

fn decode(
    saved: &serde_json::Value,
    check: &antiburn_checks::compiler::CompiledCheck,
) -> Result<EvaluatedAssessment, antiburn_checks::results::ResultError> {
    decode_results(&serde_json::to_vec(saved).unwrap())?
        .validate_for_declarations(&check.decisions, &check.question_requirements())
}

fn check() -> antiburn_checks::compiler::CompiledCheck {
    antiburn_checks::compile(include_str!("fixtures/retry-review.yml")).unwrap()
}

#[test]
fn label_only_results_validate_labels_without_adding_details() {
    let choice = label_only("stuck");
    assert_eq!(
        serde_json::to_value(&choice).unwrap()["decisions"][0]["answers"][0],
        json!({"name": "verdict", "result": {"type": "choice", "output": "stuck"}})
    );
    let mut label_check = check();
    label_check.actions.clear();
    choice
        .validate_for_declarations(&label_check.decisions, &label_check.question_requirements())
        .unwrap();
    for (decision, question, declared, output) in [
        ("", "verdict", labels(), "stuck"),
        ("review", "", labels(), "stuck"),
        ("review", "verdict", labels(), ""),
        ("review", "verdict", labels(), "other"),
        ("review", "verdict", vec!["stuck".into()], "stuck"),
        (
            "review",
            "verdict",
            vec!["stuck".into(), "stuck".into()],
            "stuck",
        ),
    ] {
        assert!(assessed_choice(decision, question, &declared, output, None).is_err());
    }
    for (decision, question) in [("other", "verdict"), ("review", "other")] {
        let choice = assessed_choice(decision, question, &labels(), "stuck", None).unwrap();
        assert!(
            choice
                .validate_for_declarations(
                    &label_check.decisions,
                    &label_check.question_requirements()
                )
                .is_err()
        );
    }
}

#[test]
fn present_probability_maps_remain_strict() {
    for probabilities in [
        BTreeMap::new(),
        BTreeMap::from([("stuck".into(), 1.0)]),
        BTreeMap::from([("stuck".into(), 0.5), ("clear".into(), 0.5)]),
        BTreeMap::from([
            ("stuck".into(), f64::NAN),
            ("clear".into(), 0.0),
            ("uncertain".into(), 0.0),
        ]),
        BTreeMap::from([
            ("stuck".into(), 1.1),
            ("clear".into(), -0.1),
            ("uncertain".into(), 0.0),
        ]),
        BTreeMap::from([
            ("stuck".into(), 0.1),
            ("clear".into(), 0.1),
            ("uncertain".into(), 0.1),
        ]),
    ] {
        assert!(
            assessed_choice("review", "verdict", &labels(), "stuck", Some(probabilities)).is_err()
        );
    }
}

#[test]
fn absent_details_do_not_create_null_matches_or_fallback_success() {
    let conditions = [
        "decisions.review.details.verdict.probabilities.stuck >= 0.95",
        "!(decisions.review.details.verdict.probabilities.stuck >= 0.95)",
        "decisions.review.details.verdict.probabilities.stuck == null",
        "decisions.review.details.verdict.probabilities.stuck || true",
    ];
    let actions = conditions.map(|condition| StatusAction {
        condition: Some(Condition::Expression(parse_expression(condition).unwrap())),
        result: StatusResult::Success.into(),
        message: None,
    });
    for prior in [
        None,
        Some(StatusResult::Failure),
        Some(StatusResult::Success),
    ] {
        let plan = plan_status_actions(&actions, &json!({}), &label_only("stuck"), prior).unwrap();
        assert_eq!(plan.current_status, prior);
        assert!(
            plan.outcomes
                .iter()
                .all(|action| { action.outcome == ActionOutcomeKind::SkippedConditionUnavailable })
        );
    }
}

#[test]
fn stored_label_only_answers_require_declaration_and_action_compatibility() {
    let probability_check = check();
    let mut label_check = check();
    label_check.actions = vec![StatusAction {
        condition: Some(Condition::Expression(
            parse_expression("decisions.review.outputs.verdict == 'stuck'").unwrap(),
        )),
        result: StatusResult::Failure.into(),
        message: None,
    }];
    assert!(!label_check.question_requirements()[0].probabilities);
    for output in labels() {
        let answer = label_only(&output);
        let bytes = serde_json::to_vec(&answer).unwrap();
        let restored = decode_results(&bytes)
            .unwrap()
            .validate_for_declarations(&label_check.decisions, &label_check.question_requirements())
            .unwrap();
        assert_eq!(restored, answer);
        restored
            .validate_for_declarations(&label_check.decisions, &label_check.question_requirements())
            .unwrap();
        let error = restored
            .validate_for_declarations(
                &probability_check.decisions,
                &probability_check.question_requirements(),
            )
            .unwrap_err();
        assert!(error.message.contains("probability"));
        let plan = plan_status_actions(&label_check.actions, &json!({}), &restored, None).unwrap();
        assert_eq!(
            plan.current_status,
            (output == "stuck").then_some(StatusResult::Failure),
        );
    }
    for field in ["decision", "name", "output"] {
        let mut saved = serde_json::to_value(label_only("stuck")).unwrap();
        match field {
            "decision" => saved["decisions"][0][field] = json!("unknown"),
            "name" => saved["decisions"][0]["answers"][0][field] = json!("unknown"),
            _ => saved["decisions"][0]["answers"][0]["result"][field] = json!("unknown"),
        }
        assert!(decode(&saved, &label_check).is_err(), "{field}");
    }
}

#[test]
fn stored_absence_is_distinct_from_null_empty_or_partial_maps() {
    let base = serde_json::to_value(label_only("stuck")).unwrap();
    let mut label_check = check();
    label_check.actions.clear();
    for probabilities in [
        json!(null),
        json!({}),
        json!({"stuck": 1.0}),
        json!({"stuck": 0.1, "clear": 0.1, "uncertain": 0.1}),
        json!({"stuck": 1.1, "clear": -0.1, "uncertain": 0.0}),
    ] {
        let mut saved = base.clone();
        saved["decisions"][0]["answers"][0]["result"]["probabilities"] = probabilities;
        assert!(decode(&saved, &label_check).is_err());
    }
    for probabilities in [
        json!({"stuck": 0.5, "clear": 0.5}),
        json!({"stuck": 0.5, "clear": 0.2, "uncertain": 0.2, "extra": 0.1}),
    ] {
        let mut saved = base.clone();
        saved["decisions"][0]["answers"][0]["result"]["probabilities"] = probabilities;
        assert!(decode(&saved, &check()).is_err());
    }
    for field in ["decision", "name", "output"] {
        let mut saved = base.clone();
        match field {
            "decision" => saved["decisions"][0][field] = json!(""),
            "name" => saved["decisions"][0]["answers"][0][field] = json!(""),
            _ => saved["decisions"][0]["answers"][0]["result"][field] = json!(""),
        }
        assert!(decode(&saved, &label_check).is_err());
    }
    let mut confidence = base;
    confidence["decisions"][0]["answers"][0]["result"]["confidence"] = json!(0.99);
    assert!(decode(&confidence, &label_check).is_ok());
    confidence["decisions"][0]["answers"][0]["result"]["confidence"] = json!(1.01);
    assert!(decode(&confidence, &label_check).is_err());
}
