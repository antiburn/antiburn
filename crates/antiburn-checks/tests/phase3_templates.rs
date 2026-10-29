use antiburn_checks::actions::{ActionOutcomeKind, plan_status_actions_with_budget};
use antiburn_checks::compile;
use antiburn_checks::compiler::StatusResult;
use antiburn_checks::diagnostics::DiagnosticCode;
use antiburn_checks::expression::{PathSegment, parse_expression};
use antiburn_checks::expression_eval::{
    EvaluationBudget, EvaluationDependencyKind, EvaluationLimits, EvaluationValue,
};
use antiburn_checks::preparation::evaluate_decision_input_with_budget;
use antiburn_checks::results::EvaluatedAssessment;
use serde_json::{Value, json};

pub mod support;
use support::assessed_choice;

fn definition(input: &str, actions: &str) -> String {
    format!(
        "name: Template test\non:\n  tool_result:\n  window: 3\ndecisions:\n  review:\n    input:\n{input}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {{yes: Useful., no: Not useful.}}\nactions:\n{actions}\n"
    )
}

fn source(input: &str) -> String {
    definition(input, "  - type: status\n    input: {result: failure}")
}

fn prepare(input: &str, context: Value) -> Result<EvaluationValue, DiagnosticCode> {
    let check = compile(&source(input)).unwrap();
    evaluate_decision_input_with_budget(
        &check.decisions[0],
        &context,
        &mut EvaluationBudget::default(),
    )
    .map(|value| value.value)
    .map_err(|error| error.code)
}

#[test]
fn whole_and_embedded_templates_preserve_types_and_evaluate_source_once() {
    let prepared = prepare(
        r#"      typed: "${{ message.tool.arguments }}"
      text: "value=${{ message.tool.arguments.value }}; bool=${{ true }}; null=${{ null }}"
      json: "${{ toJSON(message.tool.arguments) }}"
      braces: "before ${{ '}}' }} after"
      nested: ["${{ 2 }}", {value: "${{ false }}"}]"#,
        json!({"message": {"tool": {"arguments": {"value": "${{ unavailable.path }}"}}}}),
    )
    .unwrap();
    assert_eq!(
        prepared,
        EvaluationValue::Available(json!({
            "typed": {"value": "${{ unavailable.path }}"},
            "text": "value=${{ unavailable.path }}; bool=true; null=",
            "json": "{\"value\":\"${{ unavailable.path }}\"}",
            "braces": "before }} after",
            "nested": [2, {"value": false}],
        }))
    );
    assert_eq!(
        prepare(
            "      value: \"prefix ${{ message.tool.arguments }}\"",
            json!({"message": {"tool": {"arguments": {}}}})
        ),
        Err(DiagnosticCode::RuntimeInvalidContext)
    );
    assert_eq!(
        prepare(
            "      value: \"prefix ${{ message.tool.arguments }}\"",
            json!({})
        ),
        Ok(EvaluationValue::Unavailable)
    );
}

#[test]
fn nested_maps_keep_order_aliases_and_nested_arrays() {
    let prepared = prepare(
        r#"      value:
        map: "${{ message.tool.arguments.groups }}"
        as: group
        value:
          name: "${{ group.name }}"
          items:
            map: "${{ group.items }}"
            as: item
            if: "item.enabled"
            value: "${{ format('{0}:{1}', group.name, item.name) }}""#,
        json!({"message": {"tool": {"arguments": {"groups": [
            {"name": "first", "items": [{"enabled": false}, {"enabled": true, "name": "keep"}]},
            {"name": "second", "items": []},
        ]}}}}),
    )
    .unwrap();
    assert_eq!(
        prepared,
        EvaluationValue::Available(json!({"value": [
            {"name": "first", "items": ["first:keep"]},
            {"name": "second", "items": []},
        ]}))
    );
}

#[test]
fn map_missing_conditions_and_values_fail_instead_of_filtering() {
    let input = r#"      value:
        map: "${{ message.tool.arguments.items }}"
        as: item
        if: "item.keep"
        value: "${{ item.text }}""#;
    for items in [json!([{}]), json!([{"keep": true}])] {
        assert_eq!(
            prepare(
                input,
                json!({"message": {"tool": {"arguments": {"items": items}}}})
            )
            .unwrap(),
            EvaluationValue::Unavailable
        );
    }
    assert_eq!(
        prepare(
            input,
            json!({"message": {"tool": {"arguments": {"items": [{"keep": false}]}}}})
        )
        .unwrap(),
        EvaluationValue::Available(json!({"value": []}))
    );
    assert_eq!(
        prepare(
            input,
            json!({"message": {"tool": {"arguments": {"items": {}}}}})
        ),
        Err(DiagnosticCode::RuntimeInvalidContext)
    );
}

#[test]
fn map_traces_select_membership_conditions_and_emitted_fields_only() {
    let check = compile(&source(
        r#"      value:
        map: "${{ window.messages }}"
        as: item
        if: "item.tool.result.status == 'completed'"
        value: "${{ fromJSON(toJSON(item.tool.result.content)) }}""#,
    ))
    .unwrap();
    let evaluated = evaluate_decision_input_with_budget(
        &check.decisions[0],
        &json!({"window": {"messages": [
            {"tool": {"result": {"status": "error", "content": "unused"}}},
            {"tool": {"result": {"status": "completed", "content": {"text": "selected"}}}},
        ]}}),
        &mut EvaluationBudget::default(),
    )
    .unwrap();
    let dependencies = &evaluated.trace.dependencies;
    let path = |text: &str| {
        let antiburn_checks::expression::Expression::Path(path) = parse_expression(text).unwrap()
        else {
            panic!("static path")
        };
        path
    };
    assert!(
        dependencies
            .iter()
            .any(|entry| entry.kind == EvaluationDependencyKind::Membership
                && entry.path == path("window.messages"))
    );
    for selected in [
        "window.messages[0].tool.result.status",
        "window.messages[1].tool.result.status",
        "window.messages[1].tool.result.content",
    ] {
        assert!(
            dependencies
                .iter()
                .any(|entry| entry.kind == EvaluationDependencyKind::Value
                    && entry.path == path(selected)),
            "missing {selected}: {dependencies:?}"
        );
    }
    assert!(
        !dependencies
            .iter()
            .any(|entry| entry.kind == EvaluationDependencyKind::Value
                && (entry.path == path("window.messages")
                    || entry.path == path("window.messages[0]")
                    || entry.path == path("window.messages[0].tool.result.content")))
    );
    assert_eq!(
        evaluated.value,
        EvaluationValue::Available(json!({"value": [{"text": "selected"}]}))
    );
}

#[test]
fn aliases_cannot_shadow_roots_enclosing_aliases_or_escape_scope() {
    let directive =
        r#"      value: {map: "${{ session.requests }}", as: item, value: "${{ item.text }}"}"#;
    for root in [
        "session",
        "turn",
        "message",
        "window",
        "global",
        "repo",
        "agent",
        "decisions",
        "Bad",
    ] {
        assert!(
            compile(&source(
                &directive.replace("as: item", &format!("as: {root}"))
            ))
            .is_err(),
            "{root}"
        );
    }
    for input in [
        format!("{directive}\n      other: \"${{{{ item.text }}}}\""),
        directive.replace("${{ session.requests }}", "${{ item.items }}"),
        directive.replace(
            "value: \"${{ item.text }}\"",
            "value: {map: \"${{ session.requests }}\", as: item, value: x}",
        ),
        directive.replace("as: item", "as: item, extra: true"),
        directive.replace("${{ session.requests }}", "prefix ${{ session.requests }}"),
        directive.replace(", as: item", ""),
    ] {
        assert!(compile(&source(&input)).is_err(), "{input}");
    }
    assert!(compile(&source(&directive.replace("item", "messages"))).is_ok());
    assert!(
        compile(&source(&directive.replace(
            "${{ session.requests }}",
            "${{ false || session.requests }}"
        )))
        .is_ok()
    );
    assert!(
        compile(&source(&directive.replace(
            "${{ session.requests }}",
            "${{ session.requests || false }}"
        )))
        .is_ok()
    );
}

#[test]
fn closed_record_access_has_dot_bracket_dynamic_and_projection_parity() {
    for expression in [
        "message.quality.source_complete",
        "message['quality']['source_complete']",
        "message.tool.provider_executed",
        "message['tool']['provider_executed']",
        "message.tool.result.outcome.exit",
        "message['tool']['result']['outcome']['exit']",
        "window.messages[session.requests.length].quality.main_session",
        "window.messages.*.tool.result.status",
        "session.requests.*.text",
        "message.tool.arguments[session.id].anything",
        "fromJSON('{}').anything",
        "message.tool.result.content.anything",
        "message.quality.*",
    ] {
        assert!(
            compile(&source(&format!(
                "      value: \"${{{{ {expression} }}}}\""
            )))
            .is_ok(),
            "{expression}"
        );
    }
    for expression in [
        "message.quality.typo",
        "message['quality']['typo']",
        "message.tool.typo",
        "message['tool']['typo']",
        "message.tool.result.outcome.typo",
        "message['tool']['result']['outcome']['typo']",
        "window.messages[session.requests.length].typo",
        "window.messages.*.typo",
        "message[session.id]",
        "message.content.anything",
        "message.tool.result.outcome.exit.anything",
        "toJSON(message).anything",
        "contains('x', 'x').anything",
        "session.requests[0].typo",
        "message.Quality",
        "window.typo",
    ] {
        assert!(
            compile(&source(&format!(
                "      value: \"${{{{ {expression} }}}}\""
            )))
            .is_err(),
            "{expression}"
        );
    }
    assert!(
        compile(&source(
            r#"      value: {map: "${{ session.requests }}", as: item, value: "${{ item.typo }}"}"#
        ))
        .is_err()
    );
}

#[test]
fn phase_rules_apply_inside_calls_dynamic_selectors_and_templates() {
    for expression in [
        "toJSON(decisions.review.outputs.verdict)",
        "message.tool.arguments[decisions.review.outputs.verdict]",
    ] {
        assert!(
            compile(&source(&format!(
                "      value: \"prefix ${{{{ {expression} }}}}\""
            )))
            .is_err()
        );
    }
    let definition = source("      value: true");
    for condition in [
        "window.messages.length > 0",
        "contains(toJSON(window.messages), 'x')",
        "message.tool.arguments[window.messages.length]",
    ] {
        assert!(
            compile(&definition.replace("decisions:", &format!("if: \"{condition}\"\ndecisions:")))
                .is_err()
        );
    }
    assert!(
        compile(
            &definition
                .replace("  window: 3\n", "")
                .replace("value: true", "value: \"${{ window.messages }}\"")
        )
        .is_err()
    );
}

#[test]
fn compiler_admits_every_pure_function_and_rejects_unsupported_functions() {
    for expression in [
        "strictEquals(1, 1)",
        "CONTAINS('text', 'e')",
        "startsWith('text', 't')",
        "endsWith('text', 't')",
        "join(fromJSON('[1,2]'), ':')",
        "format('{0}', true)",
        "toJSON(message)",
        "fromJSON('{}')",
    ] {
        compile(&source(&format!(
            "      value: \"${{{{ {expression} }}}}\""
        )))
        .unwrap();
    }
    for expression in [
        "hashFiles('x')",
        "success()",
        "contains('x')",
        "fromJSON('{}', 1)",
        "format('no values')",
    ] {
        assert!(
            compile(&source(&format!(
                "      value: \"${{{{ {expression} }}}}\""
            )))
            .is_err()
        );
    }
}

#[test]
fn status_results_and_messages_validate_without_blocking_independent_actions() {
    let check = compile(&definition(
        "      value: true",
        r#"  - type: status
    input: {result: failure, message: First write.}
  - type: status
    input: {result: "${{ 'SUCCESS' }}"}
  - type: status
    input: {result: "${{ true }}"}
  - type: status
    input: {result: success, message: "${{ null }}"}
  - type: status
    input: {result: success, message: "${{ '   ' }}"}
  - type: status
    input: {result: "${{ message.tool.arguments.missing }}"}
  - type: status
    input: {result: "${{ 'success' }}", message: "result ${{ message.tool.result.status }}"}
  - type: status
    input: {result: success}"#,
    ))
    .unwrap();
    let plan = plan_status_actions_with_budget(
        &check.actions,
        &json!({"message": {"tool": {"result": {"status": "completed"}}}}),
        &EvaluatedAssessment::local(),
        None,
        &mut EvaluationBudget::default(),
    )
    .unwrap();
    assert_eq!(plan.current_status, Some(StatusResult::Success));
    assert!(
        matches!(&plan.outcomes[0].outcome, ActionOutcomeKind::Applied { result: StatusResult::Failure, message: Some(text) } if text == "First write.")
    );
    for outcome in &plan.outcomes[1..5] {
        assert!(matches!(outcome.outcome, ActionOutcomeKind::Failed(_)));
    }
    assert_eq!(
        plan.outcomes[5].outcome,
        ActionOutcomeKind::SkippedInputUnavailable
    );
    assert!(
        matches!(&plan.outcomes[6].outcome, ActionOutcomeKind::Applied { message: Some(text), .. } if text == "result completed")
    );
    assert!(matches!(
        plan.outcomes[7].outcome,
        ActionOutcomeKind::Applied { message: None, .. }
    ));
}

#[test]
fn capability_requirements_scan_result_message_functions_and_dynamic_access() {
    for input in [
        "result: \"${{ contains(toJSON(decisions.review.details.verdict.probabilities), 'yes') && 'failure' || 'success' }}\"",
        "result: failure, message: \"probability ${{ decisions.review.details.verdict.probabilities[message.tool.arguments.label] }}\"",
        "result: failure, message: \"${{ toJSON(decisions.review.details.*.probabilities) }}\"",
    ] {
        let check = compile(&definition(
            "      value: true",
            &format!("  - type: status\n    if: false\n    input: {{{input}}}"),
        ))
        .unwrap();
        assert!(check.question_requirements()[0].probabilities, "{input}");
        assert!(!check.question_requirements()[0].confidence);
    }
    assert!(compile(&definition("      value: true", "  - type: status\n    if: false\n    input: {result: failure, message: \"${{ toJSON(decisions.review.details.verdict.unknown) }}\"}")).is_err());
    let confidence = compile(&definition("      value: true", "  - type: status\n    if: false\n    input: {result: failure, message: \"${{ toJSON(decisions.review.details.verdict.confidence) }}\"}")).unwrap();
    assert!(confidence.question_requirements()[0].confidence);
    let labels_only = compile(&definition("      value: true", "  - type: status\n    input: {result: failure, message: \"${{ toJSON(decisions.*.outputs.verdict) }}\"}")).unwrap();
    assert!(!labels_only.question_requirements()[0].probabilities);
    assert!(
        compile(&definition(
            "      value: true",
            "  - type: message\n    if: false\n    input: {message: Unsupported.}"
        ))
        .is_err()
    );
}

#[test]
fn capability_requirements_keep_grouped_projection_indexes_and_control_reads() {
    for (expression, probabilities) in [
        ("toJSON((decisions.*.details)[0])", true),
        (
            "toJSON((decisions.*.details)[message.tool.arguments.index])",
            true,
        ),
        ("(decisions.*.details)[0].verdict.probabilities.yes", true),
        (
            "(decisions.*.details)[message.tool.arguments.index].verdict.probabilities.yes",
            true,
        ),
        ("(decisions.*.details.*.probabilities)[0][0].yes", true),
        (
            "(decisions.review.details.verdict.probabilities.*).length > 0",
            true,
        ),
        (
            "(decisions.review.details.*.probabilities).length > 0",
            true,
        ),
        ("(decisions.*.details.*.probabilities)[0].length > 0", true),
        (
            "(decisions.*.details.*.probabilities)[message.tool.arguments.index].length > 0",
            true,
        ),
        (
            "(decisions.review.details.verdict.probabilities.*).length && message.tool.arguments.result",
            true,
        ),
        (
            "message.tool.arguments[(decisions.review.details.verdict.probabilities.*).length]",
            true,
        ),
        (
            "(decisions.review.details.verdict.probabilities && message.tool.arguments).result",
            true,
        ),
        (
            "message.tool.arguments[(decisions.*.details)[0].verdict.probabilities.yes]",
            true,
        ),
        ("toJSON(decisions.*.outputs.verdict)", false),
        ("(decisions.*.outputs.verdict).length > 0", false),
        ("(decisions.*.outputs.*)[0].length > 0", false),
        ("toJSON((decisions.*.outputs)[0])", false),
        (
            "(decisions.*.outputs)[message.tool.arguments.index].verdict",
            false,
        ),
        (
            "decisions.review.outputs.verdict && message.tool.arguments.result",
            false,
        ),
    ] {
        for action in [
            format!(
                "  - type: status\n    if: {}\n    input: {{result: failure}}",
                serde_json::to_string(expression).unwrap()
            ),
            format!(
                "  - type: status\n    input: {{result: {}}}",
                serde_json::to_string(&format!(
                    "${{{{ {expression} && 'success' || 'failure' }}}}"
                ))
                .unwrap()
            ),
            format!(
                "  - type: status\n    input: {{result: failure, message: {}}}",
                serde_json::to_string(&format!("${{{{ {expression} }}}}")).unwrap()
            ),
        ] {
            let check = compile(&definition("      value: true", &action))
                .unwrap_or_else(|errors| panic!("{action}: {errors:?}"));
            assert_eq!(
                check.question_requirements()[0].probabilities,
                probabilities,
                "{action}"
            );
            assert!(
                check.question_requirements()[0].confidence
                    == matches!(
                        expression,
                        "toJSON((decisions.*.details)[0])"
                            | "toJSON((decisions.*.details)[message.tool.arguments.index])"
                    ),
                "{action}"
            );
            let answer = assessed_choice(
                "review",
                "verdict",
                &["yes".into(), "no".into()],
                "yes",
                None,
            )
            .unwrap();
            let validated =
                answer.validate_for_declarations(&check.decisions, &check.question_requirements());
            if probabilities {
                let error = validated.expect_err("a label-only result must fail before acceptance");
                assert!(error.message.contains("probability"), "{action}: {error:?}");
            } else {
                validated.unwrap_or_else(|error| panic!("output-only projection must remain label-only compatible: {action}: {error:?}"));
            }
        }
    }
}

#[test]
fn fixed_input_containers_charge_exact_shared_item_boundaries() {
    for (kind, maximum) in [("sequence", 999), ("object", 999)] {
        for (count, expected) in [(maximum, true), (maximum + 1, false)] {
            let input = if kind == "sequence" {
                format!(
                    "      value: [{}]",
                    std::iter::repeat_n("true", count)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                format!(
                    "      value:\n{}",
                    (0..count)
                        .map(|index| format!("        item_{index}: true"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            };
            let check = compile(&source(&input)).unwrap();
            let mut budget = EvaluationBudget::default();
            let evaluated =
                evaluate_decision_input_with_budget(&check.decisions[0], &json!({}), &mut budget);
            assert_eq!(
                evaluated.is_ok(),
                expected,
                "{kind}, {count}: {evaluated:?}"
            );
            if expected {
                assert_eq!(budget.visited_items(), 1_000);
            } else {
                assert_eq!(
                    evaluated.unwrap_err().code,
                    DiagnosticCode::EvaluationLimitExceeded
                );
                assert!(budget.exhausted());
            }
        }
    }

    let check = compile(&source("      value: [true, {nested: [true, true]}]")).unwrap();
    for maximum in [5, 6] {
        let mut budget = EvaluationBudget::new(EvaluationLimits::default(), maximum, 1024);
        let evaluated =
            evaluate_decision_input_with_budget(&check.decisions[0], &json!({}), &mut budget);
        assert_eq!(evaluated.is_ok(), maximum == 6, "{maximum}: {evaluated:?}");
        if maximum == 6 {
            assert_eq!(budget.visited_items(), 6);
        }
    }
}

#[test]
fn recursive_templates_and_actions_share_steps_items_bytes_and_depth() {
    let check = compile(&source("      value: [true, true, true, true, true]")).unwrap();
    let mut budget = EvaluationBudget::new(
        EvaluationLimits {
            max_depth: 32,
            max_steps: 6,
        },
        1000,
        1024,
    );
    assert_eq!(
        evaluate_decision_input_with_budget(&check.decisions[0], &json!({}), &mut budget)
            .unwrap_err()
            .code,
        DiagnosticCode::EvaluationLimitExceeded
    );

    let check = compile(&source(r#"      value: {map: "${{ message.tool.arguments.items }}", as: item, value: "${{ item }}"}"#)).unwrap();
    let mut budget = EvaluationBudget::new(EvaluationLimits::default(), 2, 1024);
    assert_eq!(
        evaluate_decision_input_with_budget(
            &check.decisions[0],
            &json!({"message": {"tool": {"arguments": {"items": [1, 2, 3]}}}}),
            &mut budget
        )
        .unwrap_err()
        .code,
        DiagnosticCode::EvaluationLimitExceeded
    );

    let check = compile(&definition("      value: true", "  - type: status\n    input: {result: failure}\n  - type: status\n    input: {result: \"${{ 'invalid' }}\"}\n  - type: status\n    input: {result: success, message: This message exceeds the remaining byte budget.}")).unwrap();
    let mut budget = EvaluationBudget::new(EvaluationLimits::default(), 1000, 25);
    evaluate_decision_input_with_budget(&check.decisions[0], &json!({}), &mut budget).unwrap();
    assert_eq!(
        plan_status_actions_with_budget(
            &check.actions,
            &json!({}),
            &EvaluatedAssessment::local(),
            None,
            &mut budget
        )
        .unwrap_err()
        .code,
        DiagnosticCode::EvaluationLimitExceeded
    );
    assert!(budget.exhausted());
    assert!(
        plan_status_actions_with_budget(
            &check.actions[..1],
            &json!({}),
            &EvaluatedAssessment::local(),
            None,
            &mut budget
        )
        .is_err()
    );

    let nested = "[".repeat(8) + "true" + &"]".repeat(8);
    let check = compile(&source(&format!("      value: {nested}"))).unwrap();
    let mut budget = EvaluationBudget::new(
        EvaluationLimits {
            max_depth: 4,
            max_steps: 1000,
        },
        1000,
        1024,
    );
    assert_eq!(
        evaluate_decision_input_with_budget(&check.decisions[0], &json!({}), &mut budget)
            .unwrap_err()
            .code,
        DiagnosticCode::EvaluationLimitExceeded
    );
}

#[test]
fn projection_map_aliases_keep_original_source_indexes() {
    let check = compile(&source(r#"      value: {map: "${{ window.messages.*.tool.result }}", as: item, value: "${{ item.content }}"}"#)).unwrap();
    let evaluated = evaluate_decision_input_with_budget(
        &check.decisions[0],
        &json!({"window": {"messages": [{"tool": {"result": {"content": "selected"}}}]}}),
        &mut EvaluationBudget::default(),
    )
    .unwrap();
    assert!(evaluated.trace.dependencies.iter().any(|entry| entry.path
        == vec![
            PathSegment::Property("window".into()),
            PathSegment::Property("messages".into()),
            PathSegment::Index(0),
            PathSegment::Property("tool".into()),
            PathSegment::Property("result".into()),
            PathSegment::Property("content".into()),
        ]));
}

#[test]
fn map_alias_identity_is_not_conversion_provenance() {
    let prepared = prepare(
        r#"      value:
        map: "${{ window.messages }}"
        as: item
        if: "item == window.messages[0]"
        value:
          same: "${{ item == window.messages[0] }}"
          converted: "${{ fromJSON(toJSON(item)) == item }}""#,
        json!({"window": {"messages": [{"id": "first"}, {"id": "second"}]}}),
    )
    .unwrap();
    assert_eq!(
        prepared,
        EvaluationValue::Available(json!({"value": [{"same": true, "converted": false}]}))
    );
}
