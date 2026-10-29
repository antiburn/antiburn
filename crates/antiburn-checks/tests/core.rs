use std::collections::BTreeMap;
use std::time::Duration;

use antiburn_checks::actions::{ActionOutcomeKind, plan_status_actions};
use antiburn_checks::compile;
use antiburn_checks::compiler::{
    Condition, InputTemplate, RollingWindow, StatusResult, StatusResultTemplate, ToolCategory,
    ToolResultStatus,
};
use antiburn_checks::config::{ConfigValueKind, parse_yaml};
use antiburn_checks::decision::{QuestionKind, QuestionRequirements};
use antiburn_checks::diagnostics::DiagnosticCode;
use antiburn_checks::expression::{BinaryOperator, Expression, PathSegment, parse_expression};

pub mod support;
use support::{choice_for_check, choice_labels};

fn valid_check() -> &'static str {
    r#"
name: Synthetic retry review
description: Detect repeated attempts at an unresolved failure.
on:
  tool_result:
    tools: [read, shell]
    status: [error, completed]
  window: {size: 5, min: 3}
  cooldown: 2m
if: "message.tool.result.status != 'cancelled'"
decisions:
  review:
    input:
      requests: "${{ session.requests }}"
      activity: "${{ window.messages[0].tool.result.status }}"
    output:
      verdict:
        type: choice
        question: Are these repeated attempts unresolved?
        criteria:
          stuck: Repeated attempts show the same unresolved failure.
          clear: The attempts show recovery or unrelated work.
          uncertain: The selected evidence is ambiguous.
actions:
  - type: status
    if: "decisions.review.outputs.verdict == 'stuck' && decisions.review.details.verdict.probabilities.stuck >= 0.95"
    input:
      result: failure
      message: Review the repeated failed operation.
  - type: status
    if: "!(decisions.review.outputs.verdict == 'stuck' && decisions.review.details.verdict.probabilities.stuck >= 0.95)"
    input:
      result: success
"#
}

#[test]
fn compiles_all_five_canonical_definitions() {
    let definitions = [
        (
            "Over-exploring",
            include_str!("../../../.antiburn/checks/over-exploring.yml"),
        ),
        (
            "Scope creep",
            include_str!("../../../.antiburn/checks/scope-creep.yml"),
        ),
        (
            "Ignored instructions",
            include_str!("../../../.antiburn/checks/ignored-instructions.yml"),
        ),
        (
            "Skill opportunities",
            include_str!("../../../.antiburn/checks/skill-opportunities.yml"),
        ),
        (
            "Fast mode value",
            include_str!("../../../.antiburn/checks/fast-mode-value.yml"),
        ),
    ];

    for (name, source) in definitions {
        let result = compile(source);
        assert!(result.is_ok(), "{name}: {result:?}");
        assert_eq!(result.unwrap().name, name);
    }
}

#[test]
fn maintained_rules_write_only_when_their_authored_status_gates_match() {
    let examples = [
        (
            include_str!("../../../.antiburn/checks/over-exploring.yml"),
            &["likely_excess"][..],
            None,
        ),
        (
            include_str!("../../../.antiburn/checks/scope-creep.yml"),
            &["unauthorized_extra"][..],
            None,
        ),
        (
            include_str!("../../../.antiburn/checks/ignored-instructions.yml"),
            &["recorded_correction_conflict", "current_file_mismatch"][..],
            Some("no_conflict"),
        ),
        (
            include_str!("../../../.antiburn/checks/skill-opportunities.yml"),
            &["actionable_help"][..],
            None,
        ),
        (
            include_str!("../../../.antiburn/checks/fast-mode-value.yml"),
            &["try_standard_mode"][..],
            None,
        ),
    ];
    for (source, failure_labels, success_label) in examples {
        let check = compile(source).unwrap();
        let labels = choice_labels(&check);
        for output in &labels {
            for probability in [0.94, 0.95, 0.99] {
                let probabilities = labels
                    .iter()
                    .map(|label| {
                        let value = if label == output {
                            probability
                        } else {
                            (1.0 - probability) / (labels.len() - 1) as f64
                        };
                        (label.clone(), value)
                    })
                    .collect::<BTreeMap<_, _>>();
                let choice = choice_for_check(&check, output, Some(probabilities)).unwrap();
                let expected = if failure_labels.contains(&output.as_str()) && probability >= 0.95 {
                    Some(StatusResult::Failure)
                } else if success_label.is_none_or(|label| output == label && probability >= 0.95) {
                    Some(StatusResult::Success)
                } else {
                    None
                };
                for prior in [
                    None,
                    Some(StatusResult::Failure),
                    Some(StatusResult::Success),
                ] {
                    let plan =
                        plan_status_actions(&check.actions, &serde_json::json!({}), &choice, prior)
                            .unwrap();
                    assert_eq!(
                        plan.current_status,
                        expected.or(prior),
                        "{}: {output} at {probability}, prior {prior:?}",
                        check.name
                    );
                    assert_eq!(
                        plan.outcomes
                            .iter()
                            .filter(|outcome| matches!(
                                outcome.outcome,
                                ActionOutcomeKind::Applied { .. }
                            ))
                            .count(),
                        usize::from(expected.is_some()),
                        "{} must write only when an authored status gate matches",
                        check.name
                    );
                }
            }
        }
    }
}

fn diagnostic(source: &str) -> antiburn_checks::Diagnostic {
    compile(source).expect_err("fixture must fail").remove(0)
}

#[test]
fn choice_criteria_counts_are_not_limited_by_a_provider() {
    let old = "          stuck: Repeated attempts show the same unresolved failure.\n          clear: The attempts show recovery or unrelated work.\n          uncertain: The selected evidence is ambiguous.";
    for count in [26, 27, 100] {
        let extra = (3..count)
            .map(|index| format!("          label_{index}: Synthetic label."))
            .collect::<Vec<_>>()
            .join("\n");
        let source = valid_check().replace(old, &format!("{old}\n{extra}"));
        let check = compile(&source).unwrap();
        assert_eq!(choice_labels(&check).len(), count);
        assert_eq!(check.question_requirements()[0].criteria_count, count);
    }
}

#[test]
fn choice_criteria_still_require_labels_and_fixed_descriptions() {
    let old = "          stuck: Repeated attempts show the same unresolved failure.\n          clear: The attempts show recovery or unrelated work.\n          uncertain: The selected evidence is ambiguous.";
    for criteria in [
        "          only: One label.",
        "          yes: ''\n          no: Nonblank.",
        "          yes: Valid.\n          Invalid: Invalid identifier.",
    ] {
        let source = valid_check().replace(old, criteria);
        let error = diagnostic(&source);
        assert_eq!(error.code, DiagnosticCode::ConfigInvalidValue);
        assert!(error.span.is_some());
        assert!(
            error
                .config_path
                .unwrap()
                .starts_with("decisions.review.output.verdict.criteria")
        );
    }
}

#[test]
fn decision_routing_is_not_an_authored_field() {
    assert!(compile(valid_check()).is_ok());
    for value in ["typesafe/jev-1.13.0", "other/model", "null", "{}"] {
        let source =
            valid_check().replace("  review:\n", &format!("  review:\n    model: {value}\n"));
        let errors = compile(&source).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        let error = &errors[0];
        assert_eq!(error.code, DiagnosticCode::ConfigUnknownField);
        assert_eq!(error.config_path.as_deref(), Some("decisions.review.model"));
        let span = error.span.unwrap();
        assert_eq!(span.column, 5);
        assert_eq!(
            source.lines().nth(span.line - 1).unwrap().trim(),
            format!("model: {value}")
        );
        assert_eq!(&source[span.start..span.end], "model");
    }
}

#[test]
fn observed_models_use_the_selected_event_scope_and_separate_current_selection() {
    let source = valid_check().replace(
        "      activity: \"${{ window.messages[0].tool.result.status }}\"",
        "      activity: \"${{ message.model }}\"",
    );
    assert!(compile(&source).is_ok());
    assert!(compile(&source.replace("message.model", "session.model")).is_ok());
    let error = diagnostic(&source.replace("message.model", "turn.model"));
    assert_eq!(error.code, DiagnosticCode::CompileUnsupportedFeature);
    assert!(error.span.is_some());
    let source = valid_check().replace(
        "    tools: [read, shell]",
        "    models: [provider/observed-model]",
    );
    assert_eq!(
        compile(&source).unwrap().trigger.models.unwrap(),
        ["provider/observed-model"]
    );
}

#[test]
fn choice_requirements_inspect_all_action_branches_and_literal_brackets() {
    let check = compile(valid_check()).unwrap();
    assert_eq!(
        check.question_requirements()[0],
        QuestionRequirements {
            decision: "review".into(),
            question: "verdict".into(),
            kind: QuestionKind::Choice,
            criteria_count: 3,
            probabilities: true,
            confidence: false,
        }
    );

    let mut check = check;
    check.actions.clear();
    assert!(!check.question_requirements()[0].probabilities);
    for condition in [
        "false && decisions.review.details.verdict.probabilities.stuck >= 0.95",
        "true || !(decisions['review']['details']['verdict']['probabilities']['stuck'] >= 0.95)",
    ] {
        let mut candidate = check.clone();
        candidate
            .actions
            .push(antiburn_checks::compiler::StatusAction {
                condition: Some(Condition::Expression(parse_expression(condition).unwrap())),
                result: StatusResult::Failure.into(),
                message: None,
            });
        assert!(
            candidate.question_requirements()[0].probabilities,
            "{condition}"
        );
    }
    for condition in [
        "decisions.review.outputs.verdict == 'stuck'",
        "decisions.other.details.verdict.probabilities.stuck >= 0.95",
        "decisions.review.details.other.probabilities.stuck >= 0.95",
        "decisions.review.details.verdict.probabilities.unknown >= 0.95",
        "message.tool.arguments.probabilities.stuck >= 0.95",
    ] {
        let mut candidate = check.clone();
        candidate
            .actions
            .push(antiburn_checks::compiler::StatusAction {
                condition: Some(Condition::Expression(parse_expression(condition).unwrap())),
                result: StatusResult::Failure.into(),
                message: None,
            });
        assert!(
            !candidate.question_requirements()[0].probabilities,
            "{condition}"
        );
        assert!(
            !candidate.question_requirements()[0].confidence,
            "{condition}"
        );
    }
    check.actions.push(antiburn_checks::compiler::StatusAction {
        condition: Some(Condition::Expression(
            parse_expression("decisions.review.details.verdict.confidence >= 0.95").unwrap(),
        )),
        result: StatusResult::Failure.into(),
        message: None,
    });
    assert!(check.question_requirements()[0].confidence);
    assert!(!check.question_requirements()[0].probabilities);

    let source = valid_check().replace("probabilities.stuck", "confidence");
    assert!(compile(&source).unwrap().question_requirements()[0].confidence);
}

#[test]
fn yaml_core_resolution_preserves_yaml_12_strings_and_explicit_types() {
    let document = parse_yaml(
        "on: yes\noff: no\nflag: true\ntext: !!str 12\nexplicit: !!bool true\nzero: !!float 0\nnegative: !!float -1\ninteger: 2\n",
    )
    .expect("valid restricted YAML");
    let entries = document.root().as_mapping().expect("root mapping");
    assert_eq!(entries[0].key, "on");
    assert!(matches!(&entries[0].value.kind, ConfigValueKind::String(value) if value == "yes"));
    assert!(matches!(&entries[1].value.kind, ConfigValueKind::String(value) if value == "no"));
    assert!(matches!(
        entries[2].value.kind,
        ConfigValueKind::Boolean(true)
    ));
    assert!(matches!(&entries[3].value.kind, ConfigValueKind::String(value) if value == "12"));
    assert!(matches!(
        entries[4].value.kind,
        ConfigValueKind::Boolean(true)
    ));
    assert!(matches!(entries[5].value.kind, ConfigValueKind::Number(_)));
    assert!(matches!(entries[6].value.kind, ConfigValueKind::Number(_)));
    assert!(matches!(entries[7].value.kind, ConfigValueKind::Number(_)));
}

#[test]
fn yaml_restrictions_report_codes_and_spans() {
    let cases = [
        (
            "name: first\nname: second\n",
            DiagnosticCode::YamlDuplicateKey,
        ),
        (
            "outer: {key: first, key: second}\n",
            DiagnosticCode::YamlDuplicateKey,
        ),
        ("1: value\n", DiagnosticCode::YamlNonStringKey),
        ("true: value\n", DiagnosticCode::YamlNonStringKey),
        ("? [a, b]\n: value\n", DiagnosticCode::YamlNonStringKey),
        ("value: .inf\n", DiagnosticCode::YamlNonFiniteNumber),
        ("value: -.Inf\n", DiagnosticCode::YamlNonFiniteNumber),
        ("value: .nan\n", DiagnosticCode::YamlNonFiniteNumber),
        ("value: 1e999\n", DiagnosticCode::YamlNonFiniteNumber),
        ("value: !custom text\n", DiagnosticCode::YamlUnsupportedTag),
        ("value: !custom {}\n", DiagnosticCode::YamlUnsupportedTag),
        (
            "value: !!set {a: null}\n",
            DiagnosticCode::YamlUnsupportedTag,
        ),
        ("value: &shared text\n", DiagnosticCode::YamlAnchor),
        (
            "value: &shared text\ncopy: *shared\n",
            DiagnosticCode::YamlAnchor,
        ),
        ("value: *missing\n", DiagnosticCode::YamlAlias),
        ("value: {<<: {a: b}}\n", DiagnosticCode::YamlMergeKey),
        ("value: {\"<<\": {a: b}}\n", DiagnosticCode::YamlMergeKey),
        (
            "name: first\n---\nname: second\n",
            DiagnosticCode::YamlDocumentCount,
        ),
        ("", DiagnosticCode::YamlRootType),
        ("- value\n", DiagnosticCode::YamlRootType),
        ("value\n", DiagnosticCode::YamlRootType),
        (
            "%YAML 1.1\n---\nname: Example\n",
            DiagnosticCode::YamlDirective,
        ),
        (
            "%TAG !e! tag:example.com,2020:\n---\nname: Example\n",
            DiagnosticCode::YamlDirective,
        ),
        ("value: !!float 2x\n", DiagnosticCode::YamlInvalidScalar),
        (
            "value: !!float 1e-9999\n",
            DiagnosticCode::YamlInvalidScalar,
        ),
    ];
    for (source, expected) in cases {
        let errors = parse_yaml(source).expect_err(source);
        assert_eq!(errors[0].code, expected, "{source}");
        assert!(errors[0].span.is_some(), "{source}");
    }
}

#[test]
fn duplicate_key_diagnostic_points_to_the_second_key() {
    let error = parse_yaml("name: first\nname: second\n").expect_err("duplicate key");
    let span = error[0].span.expect("duplicate key span");
    assert_eq!(span.line, 2);
    assert_eq!(span.column, 1);
}

#[test]
fn standard_non_specific_tag_keeps_a_plain_scalar_as_string() {
    let document = parse_yaml("name: ! Plain\n").expect("standard non-specific tag");
    let value = &document.root().as_mapping().expect("mapping")[0].value;
    assert!(matches!(&value.kind, ConfigValueKind::String(text) if text == "Plain"));
}

#[test]
fn compiles_a_typed_check_with_rolling_window_and_choice_probabilities() {
    let check = compile(valid_check()).expect("valid supported check");
    assert_eq!(check.name, "Synthetic retry review");
    assert_eq!(
        check.trigger.window,
        Some(RollingWindow {
            size: 5,
            minimum: 3
        })
    );
    assert_eq!(check.trigger.cooldown, Duration::from_secs(120));
    assert_eq!(
        check.trigger.tools,
        Some(vec![ToolCategory::Read, ToolCategory::Shell])
    );
    assert_eq!(
        check.trigger.statuses,
        Some(vec![ToolResultStatus::Error, ToolResultStatus::Completed])
    );
    assert_eq!(check.decisions[0].outputs[0].name, "verdict");
    assert_eq!(choice_labels(&check).len(), 3);
    assert_eq!(check.actions.len(), 2);
    assert_eq!(
        check.actions[0].result,
        StatusResultTemplate::Literal(StatusResult::Failure)
    );
    assert_eq!(
        check.actions[1].result,
        StatusResultTemplate::Literal(StatusResult::Success)
    );
    assert!(matches!(check.condition, Some(Condition::Expression(_))));
    let InputTemplate::Object(input) = &check.decisions[0].input else {
        panic!("expected input object")
    };
    assert!(matches!(input[1].1, InputTemplate::Expression(_)));
}

#[test]
fn applies_window_and_cooldown_defaults() {
    let source = valid_check()
        .replace("window: {size: 5, min: 3}\n", "window: 8\n")
        .replace("  cooldown: 2m\n", "");
    let check = compile(&source).expect("valid defaulted check");
    assert_eq!(
        check.trigger.window,
        Some(RollingWindow {
            size: 8,
            minimum: 1
        })
    );
    assert_eq!(check.trigger.cooldown, Duration::from_secs(60));
}

#[test]
fn rejects_unsupported_v1_features_with_source_spans() {
    let cases = [
        (
            valid_check().replace("  cooldown: 2m\n", "  cooldown: 01m\n"),
            DiagnosticCode::CompileInvalidCooldown,
        ),
        (
            valid_check().replace("{size: 5, min: 3}", "{size: 5, min: 6}"),
            DiagnosticCode::CompileInvalidWindow,
        ),
        (
            valid_check().replace("{size: 5, min: 3}", "{size: 41, min: 1}"),
            DiagnosticCode::CompileInvalidWindow,
        ),
        (
            valid_check().replace("type: choice", "type: score"),
            DiagnosticCode::ConfigInvalidType,
        ),
        (
            valid_check().replace(
                "  - type: status\n    if:",
                "  - type: message\n    if: false\n    input:\n      message: Notice\n  - type: status\n    if:",
            ),
            DiagnosticCode::CompileUnsupportedFeature,
        ),
        (
            valid_check().replace(
                "if: \"message.tool.result.status != 'cancelled'\"",
                "if: \"${{ hashFiles(message.kind) }}\"",
            ),
            DiagnosticCode::CompileUnsupportedFeature,
        ),
        (
            valid_check().replace(
                "if: \"message.tool.result.status != 'cancelled'\"",
                "if: \"${{ message.kind[0:2] }}\"",
            ),
            DiagnosticCode::CompileInvalidExpression,
        ),
        (
            valid_check().replace(
                "if: \"message.tool.result.status != 'cancelled'\"",
                "if: \"${{ message.kind.toLowerCase() }}\"",
            ),
            DiagnosticCode::CompileUnsupportedFeature,
        ),
    ];
    for (source, expected) in cases {
        let errors = compile(&source).expect_err("unsupported feature");
        assert!(
            errors.iter().any(|error| error.code == expected),
            "{source}\n{errors:?}"
        );
        assert!(errors.iter().all(|error| error.span.is_some()), "{source}");
    }
}

#[test]
fn validates_all_actions_even_when_their_conditions_are_false() {
    let source = valid_check().replace(
        "  - type: status\n    if:",
        "  - type: message\n    if: false\n    input:\n      message: Notice\n  - type: status\n    if:",
    );
    let errors = compile(&source).expect_err("message action is not supported yet");
    let error = errors
        .iter()
        .find(|error| error.code == DiagnosticCode::CompileUnsupportedFeature)
        .expect("unsupported action diagnostic");
    assert_eq!(error.config_path.as_deref(), Some("actions[0]"));
}

#[test]
fn rejects_bad_expression_syntax_and_closed_field_references() {
    let syntax = valid_check().replace(
        "if: \"message.tool.result.status != 'cancelled'\"",
        "if: \"${{ message.tool.result.status + }}\"",
    );
    assert_eq!(
        diagnostic(&syntax).code,
        DiagnosticCode::CompileInvalidExpression
    );

    let unknown = valid_check().replace(
        "if: \"message.tool.result.status != 'cancelled'\"",
        "if: \"message.tool.result.missing == null\"",
    );
    assert_eq!(
        diagnostic(&unknown).code,
        DiagnosticCode::CompileUnknownReference
    );

    let unsupported_root = valid_check().replace(
        "if: \"message.tool.result.status != 'cancelled'\"",
        "if: \"agent.skills.length > 0\"",
    );
    assert_eq!(
        diagnostic(&unsupported_root).code,
        DiagnosticCode::CompileUnsupportedFeature
    );
}

#[test]
fn rejects_left_associative_expressions_beyond_the_evaluation_depth() {
    let expression = |terms: usize| {
        std::iter::repeat_n("true", terms)
            .collect::<Vec<_>>()
            .join(" && ")
    };
    let at_limit = expression(antiburn_checks::expression_eval::DEFAULT_MAX_EVALUATION_DEPTH);
    parse_expression(&at_limit).expect("expression at the evaluator depth limit");

    let over_limit = expression(antiburn_checks::expression_eval::DEFAULT_MAX_EVALUATION_DEPTH + 1);
    let source = valid_check().replace(
        "if: \"message.tool.result.status != 'cancelled'\"",
        &format!("if: \"{over_limit}\""),
    );
    let error = compile(&source).expect_err("over-depth expression must fail compilation");
    let error = error
        .iter()
        .find(|error| error.code == DiagnosticCode::CompileInvalidExpression)
        .expect("expression depth diagnostic");
    assert_eq!(error.config_path.as_deref(), Some("if"));
    assert!(error.message.contains("maximum evaluation depth of 32"));
}

#[test]
fn compiles_partial_source_quality_and_native_context_fields() {
    let source = valid_check().replace(
        "      requests: \"${{ session.requests }}\"\n      activity:",
        "      session_id: \"${{ session.id }}\"\n      agent: \"${{ agent.name }}\"\n      repo: \"${{ repo.root }}\"\n      request: \"${{ session.requests[0].text }}\"\n      coverage: \"${{ session.source_coverage }}\"\n      quality: \"${{ message.quality }}\"\n      activity:",
    );
    compile(&source).expect("partial data fields remain available to checks");
}

#[test]
fn reports_structural_fields_and_missing_requirements() {
    let unknown = valid_check().replace(
        "name: Synthetic retry review",
        "name: Synthetic retry review\nextra: true",
    );
    assert_eq!(
        diagnostic(&unknown).code,
        DiagnosticCode::ConfigUnknownField
    );

    let missing = "name: Incomplete\non: {tool_result: null}\n";
    let errors = compile(missing).expect_err("actions are required for local checks too");
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::ConfigMissingField)
    );
    assert!(compile("name: Incomplete\non: {tool_result: null}\nactions: []\n").is_err());
}

#[test]
fn expression_parser_handles_precedence_and_fixed_access() {
    let expression =
        parse_expression("a || b && c == 'x' && items[0].name").expect("expression syntax");
    let Expression::Binary {
        operator: BinaryOperator::Or,
        right,
        ..
    } = expression
    else {
        panic!("expected disjunction at the root");
    };
    let Expression::Binary {
        operator: BinaryOperator::And,
        right,
        ..
    } = *right
    else {
        panic!("expected conjunction on the right");
    };
    let mut paths = Vec::new();
    right.visit_paths(&mut |path| paths.push(path.to_vec()));
    assert_eq!(
        paths,
        vec![vec![
            PathSegment::Property("items".to_owned()),
            PathSegment::Index(0),
            PathSegment::Property("name".to_owned()),
        ]]
    );
}

#[test]
fn expression_parser_rejects_unsupported_access_and_code() {
    for expression in [
        "items[0:2]",
        "hashFiles('x')",
        "message.kind + 'x'",
        "${{ message.kind }}",
        "message.kind == \"quoted\"",
    ] {
        assert!(parse_expression(expression).is_err(), "{expression}");
    }
}
