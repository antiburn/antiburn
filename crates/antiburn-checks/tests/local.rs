use antiburn_checks::actions::{ActionOutcomeKind, plan_status_actions};
use antiburn_checks::compile;
use antiburn_checks::compiler::StatusResult;
use antiburn_checks::diagnostics::DiagnosticCode;
use antiburn_checks::results::EvaluatedAssessment;
use serde_json::json;
use std::time::Duration;

fn source(actions: &str) -> String {
    format!("name: Local\non:\n  tool_result:\nactions:\n{actions}")
}

const ACTION: &str = "  - type: status\n    input: {result: failure}\n";

#[test]
fn local_declarations_have_no_choice_requirements_and_default_to_zero_cooldown() {
    let check = compile(&source(ACTION)).unwrap();
    assert!(check.is_local());
    assert!(check.decisions.is_empty());
    assert!(check.question_requirements().is_empty());
    assert_eq!(check.trigger.cooldown, Duration::ZERO);
    let explicit = source(ACTION).replace("  tool_result:\n", "  tool_result:\n  cooldown: 5m\n");
    assert_eq!(
        compile(&explicit).unwrap().trigger.cooldown,
        Duration::from_secs(300)
    );
    let model = compile(include_str!("../../../.antiburn/checks/scope-creep.yml")).unwrap();
    assert!(!model.is_local());
    assert!(!model.decisions.is_empty());
    assert!(!model.question_requirements().is_empty());
}

#[test]
fn present_invalid_decisions_never_fall_back_to_local_execution() {
    for decisions in ["{}", "null", "[]", "{review: {}}"] {
        let yaml =
            source(ACTION).replace("actions:\n", &format!("decisions: {decisions}\nactions:\n"));
        assert!(compile(&yaml).is_err(), "{decisions}");
    }
}

#[test]
fn local_actions_cannot_read_undeclared_decisions_or_enable_message_actions() {
    for condition in [
        "decisions.review.outputs.verdict == 'yes'",
        "false && decisions.review.outputs.verdict == 'yes'",
    ] {
        let yaml = source(&format!(
            "  - type: status\n    if: {condition}\n    input: {{result: failure}}\n"
        ));
        assert!(
            compile(&yaml)
                .unwrap_err()
                .iter()
                .any(|error| error.code == DiagnosticCode::CompileUnknownReference)
        );
    }
    let message = source("  - type: message\n    if: false\n    input: {message: Unsupported.}\n");
    assert!(compile(&message).is_err());
}

#[test]
fn local_status_order_and_explanations_use_the_shared_action_planner() {
    let check = compile(&source("  - type: status\n    input: {result: failure, message: First.}\n  - type: status\n    input: {result: '${{ message.tool.result.status == ''completed'' && ''success'' || ''failure'' }}', message: 'Last: ${{ message.tool.result.status }}'}\n")).unwrap();
    let plan = plan_status_actions(
        &check.actions,
        &json!({"message": {"tool": {"result": {"status": "completed"}}}}),
        &EvaluatedAssessment::local(),
        None,
    )
    .unwrap();
    assert_eq!(plan.current_status, Some(StatusResult::Success));
    assert_eq!(
        plan.outcomes[1].outcome,
        ActionOutcomeKind::Applied {
            result: StatusResult::Success,
            message: Some("Last: completed".into())
        }
    );
}

#[test]
fn local_no_write_retains_prior_status_and_omitted_message_clears_explanation() {
    let skipped = compile(&source(
        "  - type: status\n    if: false\n    input: {result: success}\n",
    ))
    .unwrap();
    for prior in [
        None,
        Some(StatusResult::Failure),
        Some(StatusResult::Success),
    ] {
        let plan = plan_status_actions(
            &skipped.actions,
            &json!({}),
            &EvaluatedAssessment::local(),
            prior,
        )
        .unwrap();
        assert_eq!(plan.current_status, prior);
        assert_eq!(
            plan.outcomes[0].outcome,
            ActionOutcomeKind::SkippedConditionFalse
        );
    }
    let writes = compile(&source("  - type: status\n    input: {result: failure, message: First.}\n  - type: status\n    input: {result: failure, message: Replacement.}\n  - type: status\n    input: {result: failure}\n")).unwrap();
    let plan = plan_status_actions(
        &writes.actions,
        &json!({}),
        &EvaluatedAssessment::local(),
        Some(StatusResult::Failure),
    )
    .unwrap();
    assert_eq!(
        plan.outcomes[1].outcome,
        ActionOutcomeKind::Applied {
            result: StatusResult::Failure,
            message: Some("Replacement.".into())
        }
    );
    assert_eq!(
        plan.outcomes[2].outcome,
        ActionOutcomeKind::Applied {
            result: StatusResult::Failure,
            message: None
        }
    );
}
