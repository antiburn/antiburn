use std::time::Duration;

use antiburn_checks::compile;
use antiburn_checks::compiler::{
    Condition, DecisionModel, InputTemplate, RollingWindow, StatusResult, ToolCategory,
    ToolResultStatus,
};
use antiburn_checks::config::{ConfigValueKind, parse_yaml};
use antiburn_checks::diagnostics::DiagnosticCode;
use antiburn_checks::expression::{BinaryOperator, Expression, PathSegment, parse_expression};

fn valid_check() -> &'static str {
    r#"
name: Failed Retry Loop
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
    model: typesafe/jev-1.13.0
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
    if: "decisions.review.outputs.verdict == 'clear' && decisions.review.details.verdict.probabilities.clear >= 0.95"
    input:
      result: success
"#
}

#[test]
fn compiles_every_example_supported_by_this_release() {
    let examples = [
        (
            "Failed Retry Loop",
            include_str!("../../../docs/burn-check/examples/failed-retry-loop.yml"),
        ),
        (
            "Over-exploring",
            include_str!("../../../docs/burn-check/examples/over-exploring.yml"),
        ),
        (
            "Scope Creep",
            include_str!("../../../docs/burn-check/examples/scope-creep.yml"),
        ),
    ];

    for (name, source) in examples {
        let result = compile(source);
        assert!(result.is_ok(), "{name}: {result:?}");
    }
}

fn diagnostic(source: &str) -> antiburn_checks::Diagnostic {
    compile(source).expect_err("fixture must fail").remove(0)
}

#[test]
fn pinned_choice_criteria_limit_is_located_and_accepts_the_boundary() {
    let old = "          stuck: Repeated attempts show the same unresolved failure.\n          clear: The attempts show recovery or unrelated work.\n          uncertain: The selected evidence is ambiguous.";
    for count in [26, 27] {
        let extra = (3..count)
            .map(|index| format!("          label_{index}: Synthetic label."))
            .collect::<Vec<_>>()
            .join("\n");
        let source = valid_check().replace(old, &format!("{old}\n{extra}"));
        if count == 26 {
            assert_eq!(compile(&source).unwrap().decision.criteria.len(), 26);
        } else {
            let error = diagnostic(&source);
            assert_eq!(error.code, DiagnosticCode::ConfigInvalidValue);
            assert_eq!(
                error.config_path.as_deref(),
                Some("decisions.review.output.verdict.criteria")
            );
            assert!(error.span.is_some());
            assert!(error.message.contains("26"));
        }
    }
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
    assert_eq!(check.name, "Failed Retry Loop");
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
    assert_eq!(check.decision.model, DecisionModel::TypeSafeJev1130);
    assert_eq!(check.decision.question_id, "verdict");
    assert_eq!(check.decision.criteria.len(), 3);
    assert_eq!(check.actions.len(), 2);
    assert_eq!(check.actions[0].result, StatusResult::Failure);
    assert_eq!(check.actions[1].result, StatusResult::Success);
    assert!(matches!(check.condition, Some(Condition::Expression(_))));
    assert!(matches!(
        check.decision.input[1].1,
        InputTemplate::Expression(_)
    ));
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
            valid_check().replace("typesafe/jev-1.13.0", "other/model"),
            DiagnosticCode::CompileUnsupportedFeature,
        ),
        (
            valid_check().replace("type: choice", "type: score"),
            DiagnosticCode::CompileUnsupportedFeature,
        ),
        (
            valid_check().replace("  tools: [read, shell]\n", "  models: [provider/model]\n"),
            DiagnosticCode::CompileUnsupportedFeature,
        ),
        (
            valid_check().replace(
                "      result: failure",
                "      result: \"${{ message.tool.result.status }}\"",
            ),
            DiagnosticCode::CompileUnsupportedFeature,
        ),
        (
            valid_check().replace(
                "      requests: \"${{ session.requests }}\"",
                "      requests: {map: \"${{ session.requests }}\", as: item, value: item}",
            ),
            DiagnosticCode::CompileUnsupportedFeature,
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
                "if: \"${{ contains(message.kind, 'tool_result') }}\"",
            ),
            DiagnosticCode::CompileUnsupportedFeature,
        ),
        (
            valid_check().replace(
                "if: \"message.tool.result.status != 'cancelled'\"",
                "if: \"${{ message.*.kind }}\"",
            ),
            DiagnosticCode::CompileUnsupportedFeature,
        ),
        (
            valid_check().replace(
                "if: \"message.tool.result.status != 'cancelled'\"",
                "if: \"${{ message[session.id].kind }}\"",
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
        "if: \"global.instructions.length > 0\"",
    );
    assert_eq!(
        diagnostic(&unsupported_root).code,
        DiagnosticCode::CompileUnsupportedFeature
    );
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
        "name: Failed Retry Loop",
        "name: Failed Retry Loop\nextra: true",
    );
    assert_eq!(
        diagnostic(&unknown).code,
        DiagnosticCode::ConfigUnknownField
    );

    let missing = "name: Incomplete\non: {tool_result: null}\nactions: []\n";
    let errors = compile(missing).expect_err("a decision is required");
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::ConfigMissingField)
    );
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
fn expression_parser_rejects_functions_projection_and_dynamic_access() {
    for expression in [
        "contains(items, 'x')",
        "items.*.name",
        "items[session.index]",
        "items[-1]",
        "message.kind + 'x'",
        "${{ message.kind }}",
        "message.kind == \"quoted\"",
    ] {
        assert!(parse_expression(expression).is_err(), "{expression}");
    }
}
