use std::collections::BTreeMap;

use antiburn_checks::actions::{ActionOutcomeKind, plan_status_actions};
use antiburn_checks::compile;
use antiburn_checks::compiler::{CompiledCheck, InputTemplate, StatusResult};
use antiburn_checks::decision::{Question, QuestionKind};
use antiburn_checks::diagnostics::DiagnosticCode;
use antiburn_checks::results::{
    ChoiceResult, DecisionResult, EvaluatedAssessment, NamedAnswer, NoulResult, QuestionResult,
    ScoreResult,
};
use serde_json::{Value, json};

const DECISIONS: &str = r#"decisions:
  review:
    input:
      work: "${{ message.tool.result.content }}"
    output:
      verdict:
        type: choice
        question: Does this need review?
        criteria: {yes: Review is needed., no: Review is not needed.}
      severity:
        type: score
        question: How severe is the issue?
        criteria: [Low., Medium., High.]
      applies:
        type: noul
        question: Does the policy apply?
  audit:
    input: {work: "${{ message.tool.result.content }}"}
    output:
      verdict:
        type: choice
        question: Should the work stop?
        criteria: {stop: Stop for review., allow: Continue the work.}
"#;

fn definition(decisions: &str, actions: &str) -> String {
    format!(
        "name: Typed language test\non: {{tool_result: null}}\n{decisions}actions:\n{actions}\n"
    )
}

fn check_for_expression(expression: &str, placement: &str) -> CompiledCheck {
    let quoted = |text: &str| serde_json::to_string(text).unwrap();
    let action = match placement {
        "condition" => format!(
            "  - type: status\n    if: {}\n    input: {{result: failure}}",
            quoted(expression)
        ),
        "result" => format!(
            "  - type: status\n    input: {{result: {}}}",
            quoted(&format!(
                "${{{{ {expression} && 'failure' || 'success' }}}}"
            ))
        ),
        "message" => format!(
            "  - type: status\n    if: false\n    input: {{result: failure, message: {}}}",
            quoted(&format!("${{{{ toJSON({expression}) }}}}"))
        ),
        _ => panic!("Unknown test placement."),
    };
    compile(&definition(DECISIONS, &action))
        .unwrap_or_else(|errors| panic!("{expression}, {placement}: {errors:?}"))
}

fn evaluated(check: &CompiledCheck) -> EvaluatedAssessment {
    let requirements = check.question_requirements();
    let decisions = check
        .decisions
        .iter()
        .map(|decision| {
            let answers = decision
                .outputs
                .iter()
                .map(|question| {
                    let result = match &question.question {
                        Question::Choice { criteria, .. } => QuestionResult::Choice(
                            ChoiceResult::new(
                                &question.question,
                                &criteria[0].0,
                                Some(BTreeMap::from([
                                    (criteria[0].0.clone(), 0.95),
                                    (criteria[1].0.clone(), 0.05),
                                ])),
                                Some(0.83),
                            )
                            .unwrap(),
                        ),
                        Question::Score { .. } => QuestionResult::Score(
                            ScoreResult::new(
                                &question.question,
                                1.4,
                                Some(vec![0.1, 0.4, 0.5]),
                                Some(0.72),
                            )
                            .unwrap(),
                        ),
                        Question::Noul { .. } => {
                            QuestionResult::Noul(NoulResult::new(0.75).unwrap())
                        }
                    };
                    NamedAnswer::new(&question.name, result).unwrap()
                })
                .collect();
            DecisionResult::new(decision, &requirements, answers, 1).unwrap()
        })
        .collect();
    EvaluatedAssessment::new(&check.decisions, &requirements, decisions).unwrap()
}

#[test]
fn compiles_named_mixed_questions_and_retains_all_authored_order() {
    let check = check_for_expression("decisions.review.outputs.applies >= 0.5", "condition");
    assert_eq!(
        check
            .decisions
            .iter()
            .map(|decision| decision.name.as_str())
            .collect::<Vec<_>>(),
        ["review", "audit"]
    );
    assert_eq!(
        check.decisions[0]
            .outputs
            .iter()
            .map(|question| question.name.as_str())
            .collect::<Vec<_>>(),
        ["verdict", "severity", "applies"]
    );
    let Question::Score { criteria, .. } = &check.decisions[0].outputs[1].question else {
        panic!("Expected Score.");
    };
    assert_eq!(criteria, &["Low.", "Medium.", "High."]);
    assert!(matches!(check.decisions[0].input, InputTemplate::Object(_)));
    let required = check.question_requirements();
    assert_eq!(
        required
            .iter()
            .map(|question| (
                question.decision.as_str(),
                question.question.as_str(),
                question.kind,
                question.criteria_count
            ))
            .collect::<Vec<_>>(),
        [
            ("review", "verdict", QuestionKind::Choice, 2),
            ("review", "severity", QuestionKind::Score, 3),
            ("review", "applies", QuestionKind::Noul, 0),
            ("audit", "verdict", QuestionKind::Choice, 2),
        ]
    );
}

#[test]
fn detail_requirements_bind_each_question_through_indirect_reads() {
    for (expression, expected) in [
        ("decisions.review.outputs.verdict", [(false, false); 4]),
        ("decisions.review.outputs.severity", [(false, false); 4]),
        ("decisions.review.outputs.applies", [(false, false); 4]),
        ("decisions.review.details.applies", [(false, false); 4]),
        (
            "decisions.review.details.verdict.probabilities.yes",
            [
                (true, false),
                (false, false),
                (false, false),
                (false, false),
            ],
        ),
        (
            "decisions.review.details.verdict.probabilities[message.tool.arguments.label]",
            [
                (true, false),
                (false, false),
                (false, false),
                (false, false),
            ],
        ),
        (
            "decisions.review.details.verdict.confidence",
            [
                (false, true),
                (false, false),
                (false, false),
                (false, false),
            ],
        ),
        (
            "decisions.review.details.severity.probabilities[0]",
            [
                (false, false),
                (true, false),
                (false, false),
                (false, false),
            ],
        ),
        (
            "decisions.review.details.severity.probabilities[1.0]",
            [
                (false, false),
                (true, false),
                (false, false),
                (false, false),
            ],
        ),
        (
            "decisions.review.details.severity.probabilities[message.tool.arguments.index]",
            [
                (false, false),
                (true, false),
                (false, false),
                (false, false),
            ],
        ),
        (
            "decisions.review.details.severity.probabilities.length",
            [
                (false, false),
                (true, false),
                (false, false),
                (false, false),
            ],
        ),
        (
            "(decisions.review.details.severity.probabilities.*).length",
            [
                (false, false),
                (true, false),
                (false, false),
                (false, false),
            ],
        ),
        (
            "decisions.review.details.severity.confidence",
            [
                (false, false),
                (false, true),
                (false, false),
                (false, false),
            ],
        ),
        (
            "decisions",
            [(true, true), (true, true), (false, false), (true, true)],
        ),
        (
            "decisions.review",
            [(true, true), (true, true), (false, false), (false, false)],
        ),
        ("decisions.review.outputs", [(false, false); 4]),
        (
            "decisions.review.details",
            [(true, true), (true, true), (false, false), (false, false)],
        ),
        (
            "decisions.review.details.verdict",
            [(true, true), (false, false), (false, false), (false, false)],
        ),
        (
            "decisions.review.details.severity",
            [(false, false), (true, true), (false, false), (false, false)],
        ),
        (
            "contains(toJSON(decisions.audit.details.verdict), 'stop')",
            [(false, false), (false, false), (false, false), (true, true)],
        ),
        (
            "(true && decisions.review.details.verdict).probabilities.yes",
            [
                (true, false),
                (false, false),
                (false, false),
                (false, false),
            ],
        ),
        (
            "(decisions.review.details.verdict && message.tool.arguments).value",
            [(true, true), (false, false), (false, false), (false, false)],
        ),
        (
            "message.tool.arguments[decisions.review.details.severity.confidence]",
            [
                (false, false),
                (false, true),
                (false, false),
                (false, false),
            ],
        ),
        (
            "(decisions.*.details)[message.tool.arguments.index]",
            [(true, true), (true, true), (false, false), (true, true)],
        ),
        (
            "toJSON((decisions.*.details)[0])",
            [(true, true), (true, true), (false, false), (true, true)],
        ),
        (
            "(decisions.review.details.*).length",
            [(true, true), (true, true), (false, false), (false, false)],
        ),
        ("toJSON(decisions.*.outputs)", [(false, false); 4]),
    ] {
        for placement in ["condition", "result", "message"] {
            let check = check_for_expression(expression, placement);
            let actual = check
                .question_requirements()
                .iter()
                .map(|question| (question.probabilities, question.confidence))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{expression}, {placement}");
        }
    }
}

#[test]
fn false_branches_cannot_hide_unsupported_typed_properties() {
    for expression in [
        "decisions.unknown.outputs.verdict",
        "decisions.review.outputs.unknown",
        "decisions.audit.details.severity",
        "decisions.review.details.applies.confidence",
        "decisions.review.details.applies.probabilities",
        "decisions.review.details.applies.*.confidence",
        "(decisions.review.details.applies.*)[0].probabilities",
        "decisions.review.details.severity.legend",
        "decisions.review.details.severity.probabilities.yes",
        "decisions.review.details.severity.probabilities['0']",
        "decisions.review.details.severity.probabilities[3]",
        "decisions.review.details.severity.probabilities[1.5]",
        "decisions.review.details.verdict.probabilities[0]",
        "decisions.review.details.verdict.probabilities.unknown",
        "decisions.review.details.*.probabilities",
        "decisions.review.outputs.severity.value",
        "decisions.review.outputs.applies[0]",
        "decisions[message.tool.arguments.decision].outputs.verdict",
    ] {
        let source = definition(
            DECISIONS,
            &format!(
                "  - type: status\n    if: \"false && {expression}\"\n    input: {{result: success}}"
            ),
        );
        let errors = compile(&source).expect_err(expression);
        assert!(
            errors.iter().all(|error| error.span.is_some()),
            "{expression}"
        );
    }
}

#[test]
fn mixed_typed_results_drive_actions_and_expose_exact_details() {
    let check = compile(&definition(DECISIONS, r#"  - type: status
    if: decisions.review.outputs.verdict == 'yes' && decisions.review.details.verdict.probabilities.yes >= 0.95 && decisions.review.details.verdict.confidence > 0.8 && decisions.review.outputs.severity == 1.4 && decisions.review.details.severity.probabilities[2] == 0.5 && decisions.review.details.severity.confidence > 0.7 && decisions.review.outputs.applies == 0.75 && decisions.audit.outputs.verdict == 'stop'
    input:
      result: failure
      message: "${{ toJSON(decisions) }}"
  - type: status
    if: false
    input: {result: success}"#)).unwrap();
    let assessed = evaluated(&check);
    assessed
        .validate_for_declarations(&check.decisions, &check.question_requirements())
        .unwrap();
    let plan = plan_status_actions(&check.actions, &json!({}), &assessed, None).unwrap();
    assert_eq!(plan.current_status, Some(StatusResult::Failure));
    let ActionOutcomeKind::Applied {
        message: Some(message),
        ..
    } = &plan.outcomes[0].outcome
    else {
        panic!("Expected an applied action.");
    };
    let context: Value = serde_json::from_str(message).unwrap();
    assert_eq!(
        context["review"]["outputs"],
        json!({"verdict": "yes", "severity": 1.4, "applies": 0.75})
    );
    assert_eq!(
        context["review"]["details"]["severity"],
        json!({"probabilities": [0.1, 0.4, 0.5], "confidence": 0.72})
    );
    assert_eq!(
        context["review"]["details"]["verdict"],
        json!({"probabilities": {"yes": 0.95, "no": 0.05}, "confidence": 0.83})
    );
    assert_eq!(context["review"]["details"]["applies"], json!({}));
    assert_eq!(
        plan.outcomes[1].outcome,
        ActionOutcomeKind::SkippedConditionFalse
    );
}

#[test]
fn score_and_choice_criteria_keep_language_limits_separate_from_routes() {
    for kind in ["choice", "score"] {
        let criteria = if kind == "choice" {
            (0..27)
                .map(|index| format!("label_{index}: Description."))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            std::iter::repeat_n("Description.", 27)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let criteria = if kind == "choice" {
            format!("{{{criteria}}}")
        } else {
            format!("[{criteria}]")
        };
        let source = definition(
            &format!(
                "decisions:\n  review:\n    input: {{work: true}}\n    output:\n      result: {{type: {kind}, question: Which level?, criteria: {criteria}}}\n"
            ),
            "  - type: status\n    input: {result: success}",
        );
        let check = compile(&source).unwrap();
        assert_eq!(check.question_requirements()[0].criteria_count, 27);
    }
}

#[test]
fn each_block_allows_eight_questions_without_a_language_block_limit() {
    for questions in [8, 9] {
        let outputs = (0..questions)
            .map(|index| format!("      q{index}: {{type: noul, question: Does this apply?}}\n"))
            .collect::<String>();
        let source = definition(
            &format!("decisions:\n  review:\n    input: {{work: true}}\n    output:\n{outputs}"),
            "  - type: status\n    input: {result: success}",
        );
        assert_eq!(compile(&source).is_ok(), questions == 8);
    }
    let blocks = (0..100).map(|index| format!("  d{index}:\n    input: {{work: true}}\n    output: {{answer: {{type: noul, question: Does this apply?}}}}\n")).collect::<String>();
    let check = compile(&definition(
        &format!("decisions:\n{blocks}"),
        "  - type: status\n    input: {result: success}",
    ))
    .unwrap();
    assert_eq!(check.decisions.len(), 100);
    assert_eq!(check.question_requirements().len(), 100);
}

#[test]
fn question_shapes_and_fixed_text_reject_invalid_authored_data() {
    for output in [
        "{}",
        "{answer: {type: choice, question: Which label?, criteria: [One., Two.]}}",
        "{answer: {type: score, question: Which level?, criteria: {one: One., two: Two.}}}",
        "{answer: {type: score, question: Which level?, criteria: [Only.]}}",
        "{answer: {type: score, question: Which level?, criteria: [One., '']}}",
        "{answer: {type: score, question: Which level?, criteria: [One., '${{ true }}']}}",
        "{answer: {type: score, question: Which level?}}",
        "{answer: {type: noul, question: Does it apply?, criteria: []}}",
        "{answer: {type: noul, question: ''}}",
        "{answer: {type: noul, question: '${{ true }}'}}",
        "{answer: {type: unknown, question: Does it apply?}}",
        "{Invalid: {type: noul, question: Does it apply?}}",
        "{answer: {type: noul, question: Does it apply?}, answer: {type: noul, question: Does it apply?}}",
    ] {
        let source = definition(
            &format!("decisions:\n  review:\n    input: {{work: true}}\n    output: {output}\n"),
            "  - type: status\n    if: false\n    input: {result: success}",
        );
        assert!(compile(&source).is_err(), "{output}");
    }
}

#[test]
fn inputs_and_map_aliases_cannot_read_any_decision_result() {
    for input in [
        "      work: '${{ decisions.review.outputs.verdict }}'",
        "      work: '${{ false && decisions.audit.outputs.verdict }}'",
        "      work: {map: '${{ message.tool.arguments.items }}', as: item, if: false, value: '${{ decisions.review.outputs.verdict }}'}",
        "      work: {map: '${{ decisions.review.details.severity.probabilities }}', as: item, value: '${{ item }}'}",
    ] {
        let decisions =
            DECISIONS.replace("      work: \"${{ message.tool.result.content }}\"", input);
        let source = definition(&decisions, "  - type: status\n    input: {result: success}");
        let errors = compile(&source).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.code == DiagnosticCode::CompileUnsupportedFeature),
            "{input}: {errors:?}"
        );
    }
}

#[test]
fn fixed_score_bounds_do_not_change_general_dynamic_array_admission() {
    for expression in [
        "message.tool.arguments.items[1.5]",
        "session.requests[1.5].text",
    ] {
        let source = definition(
            DECISIONS,
            &format!("  - type: status\n    if: {expression}\n    input: {{result: success}}"),
        );
        compile(&source).unwrap();
    }
}
