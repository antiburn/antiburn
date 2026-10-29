use super::*;
use crate::compiler::compile;
use serde_json::json;

fn over_budget() -> FitValidationError {
    FitValidationError::OverBudget(PreparationBudget {
        kind: PreparationBudgetKind::EstimatedTokens,
        observed: 2,
        maximum: 1,
    })
}

fn rule(path: &str, strategy: FitStrategy) -> FitRule {
    FitRule {
        path: path.into(),
        segments: parse_fit_path(path).unwrap(),
        strategy,
    }
}

fn source(fit: &str) -> String {
    format!(
        "name: Fit\non:\n  tool_result:\ndecisions:\n  review:\n    input: {{z: [], a: []}}\n    fit:\n{fit}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {{yes: Yes., no: No.}}\nactions:\n  - type: status\n    input: {{result: failure}}\n"
    )
}

#[test]
fn yaml_mapping_order_survives_compilation_and_controls_turns() {
    let check = compile(&source("      z: recent\n      a: recent")).unwrap();
    assert_eq!(
        check.decisions[0]
            .fit
            .iter()
            .map(|rule| rule.path.as_str())
            .collect::<Vec<_>>(),
        ["z", "a"]
    );
    let mut turns = Vec::new();
    let (input, report) = fit_input(
        json!({"a": [1,2,3], "z": [4,5,6]}),
        &check.decisions[0].fit,
        |input, _| {
            turns.push((
                input["z"].as_array().unwrap().len(),
                input["a"].as_array().unwrap().len(),
            ));
            if turns.len() == 4 {
                Ok(())
            } else {
                Err(over_budget())
            }
        },
    )
    .unwrap();
    assert_eq!(turns, [(3, 3), (2, 3), (2, 2), (1, 2)]);
    assert_eq!(input, json!({"a": [2,3], "z": [6]}));
    assert_eq!(
        report
            .omissions
            .iter()
            .map(|target| target.path.as_str())
            .collect::<Vec<_>>(),
        ["z", "a"]
    );
}

#[test]
fn wildcard_expansion_uses_original_array_order_and_equal_turns() {
    let input = json!({"rows": [{"items": ["a1", "a2", "a3"]}, {"items": ["b1", "b2", "b3"]}]});
    let mut turns = Vec::new();
    let (result, report) = fit_input(
        input,
        &[rule("rows[*].items", FitStrategy::Recent)],
        |input, _| {
            turns.push(input.clone());
            if turns.len() == 4 {
                Ok(())
            } else {
                Err(over_budget())
            }
        },
    )
    .unwrap();
    assert_eq!(turns[1]["rows"][0]["items"], json!(["a2", "a3"]));
    assert_eq!(turns[1]["rows"][1]["items"], json!(["b1", "b2", "b3"]));
    assert_eq!(result["rows"][0]["items"], json!(["a3"]));
    assert_eq!(result["rows"][1]["items"], json!(["b2", "b3"]));
    assert_eq!(
        report
            .omissions
            .iter()
            .map(|target| target.path.as_str())
            .collect::<Vec<_>>(),
        ["rows[0].items", "rows[1].items"]
    );
}

#[test]
fn wildcard_targets_keep_numeric_array_order_past_ten_items() {
    let rows = (0..12)
        .map(|index| json!({"items": [index, index + 100]}))
        .collect::<Vec<_>>();
    let (input, report) = fit_input(
        json!({"rows": rows}),
        &[rule("rows[*].items", FitStrategy::Recent)],
        |_, report| {
            assert_eq!(
                report
                    .omissions
                    .iter()
                    .map(|target| target.path.clone())
                    .collect::<Vec<_>>(),
                (0..report.omissions.len())
                    .map(|index| format!("rows[{index}].items"))
                    .collect::<Vec<_>>()
            );
            if report.omissions.len() == 12 {
                Ok(())
            } else {
                Err(over_budget())
            }
        },
    )
    .unwrap();
    assert_eq!(report.omissions.len(), 12);
    for index in 0..12 {
        assert_eq!(input["rows"][index]["items"], json!([index + 100]));
    }
}

#[test]
fn mixed_strategies_have_no_shared_cap_and_stop_after_the_first_fitting_turn() {
    let input = json!({"text": "x".repeat(4096), "rows": [1,2,3], "untouched": "y".repeat(1000)});
    let mut turns = Vec::new();
    let (result, report) = fit_input(
        input.clone(),
        &[
            rule("rows", FitStrategy::Recent),
            rule("text", FitStrategy::Truncate),
        ],
        |input, report| {
            turns.push(report.clone());
            if input["rows"].as_array().unwrap().len() == 1 {
                Ok(())
            } else {
                Err(over_budget())
            }
        },
    )
    .unwrap();
    assert_eq!(turns.len(), 4);
    assert_eq!(result["rows"], json!([3]));
    assert_eq!(
        result["text"].as_str().unwrap().len(),
        3072 + OMISSION_MARKER.len()
    );
    assert_eq!(result["untouched"], input["untouched"]);
    assert_eq!(report.omissions[0].retained_count, 1);
    assert_eq!(report.omissions[1].retained_count, 3072);
}

#[test]
fn fitting_input_is_unchanged_when_the_complete_request_already_fits() {
    let original = json!({"rows": [1,2,3], "text": "text".repeat(1000)});
    let bytes = serde_json::to_vec(&original).unwrap();
    let mut calls = 0;
    let (input, report) = fit_input(
        original,
        &[
            rule("text", FitStrategy::Truncate),
            rule("rows", FitStrategy::Recent),
        ],
        |_, _| {
            calls += 1;
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(serde_json::to_vec(&input).unwrap(), bytes);
    assert_eq!(calls, 1);
    assert!(report.omissions.is_empty());
}

#[test]
fn unicode_truncation_keeps_exact_head_tail_minimum_and_one_marker() {
    let original = "aé漢🚀".repeat(700);
    let mut retained = original.len();
    let mut final_input = Value::Null;
    let failure = fit_input(
        json!({"text": original}),
        &[rule("text", FitStrategy::Truncate)],
        |input, report| {
            final_input = input.clone();
            if let Some(omission) = report.omissions.first() {
                assert_eq!(omission.original_count, original.len());
                assert!(omission.retained_count >= MINIMUM_TEXT_BYTES);
                assert!(retained - omission.retained_count <= TRUNCATE_CHUNK_BYTES);
                assert!(omission.retained_count < retained);
                retained = omission.retained_count;
                let text = input["text"].as_str().unwrap();
                let (head, tail) = text.split_once(OMISSION_MARKER).unwrap();
                assert!(original.starts_with(head));
                assert!(original.ends_with(tail));
                assert_eq!(head.len() + tail.len(), retained);
                assert_eq!(text.matches(OMISSION_MARKER).count(), 1);
            }
            Err(over_budget())
        },
    )
    .unwrap_err();
    assert_eq!(failure.code, PreparationFailureCode::NoFit);
    assert!((256..=262).contains(&retained));
    assert_eq!(
        failure.report.unwrap().omissions[0].retained_count,
        retained
    );
    assert!(
        final_input["text"]
            .as_str()
            .unwrap()
            .contains(OMISSION_MARKER)
    );
}

#[test]
fn present_wrong_types_fail_even_when_no_reduction_is_needed() {
    for (path, strategy, input) in [
        ("rows", FitStrategy::Recent, json!({"rows": null})),
        ("rows[*].text", FitStrategy::Truncate, json!({"rows": {}})),
        (
            "rows[*].text",
            FitStrategy::Truncate,
            json!({"rows": [{"text": 42}]}),
        ),
        (
            "rows[*].text",
            FitStrategy::Truncate,
            json!({"rows": [null]}),
        ),
    ] {
        let failure = fit_input(input, &[rule(path, strategy)], |_, _| {
            panic!("wrong types fail before request validation")
        })
        .unwrap_err();
        assert_eq!(failure.code, PreparationFailureCode::InvalidFit);
    }
}

#[test]
fn absent_optional_fields_and_empty_arrays_are_skipped() {
    let input = json!({"rows": [{}, {"nested": []}], "empty": []});
    let (result, report) = fit_input(
        input.clone(),
        &[
            rule("rows[*].optional.text", FitStrategy::Truncate),
            rule("rows[*].nested[*].text", FitStrategy::Truncate),
            rule("empty", FitStrategy::Recent),
        ],
        |_, _| Ok(()),
    )
    .unwrap();
    assert_eq!(result, input);
    assert!(report.omissions.is_empty());
}

#[test]
fn no_fit_retains_one_record_and_preserves_unselected_values() {
    let mut last = Value::Null;
    let failure = fit_input(
        json!({"rows": [{"id": 1}, {"id": 2}], "fixed": "required"}),
        &[rule("rows", FitStrategy::Recent)],
        |input, _| {
            last = input.clone();
            Err(over_budget())
        },
    )
    .unwrap_err();
    assert_eq!(last, json!({"rows": [{"id": 2}], "fixed": "required"}));
    assert_eq!(failure.code, PreparationFailureCode::NoFit);
    assert_eq!(failure.report.unwrap().omissions[0].retained_count, 1);
}

#[test]
fn malformed_overlapping_unknown_and_unsupported_rules_fail_compilation() {
    for fit in [
        "      z[0]: recent",
        "      z..field: recent",
        "      z.*: recent",
        "      z.: recent",
        "      '[*]': recent",
        "      z: shorten",
        "      unknown: recent",
        "      z: recent\n      z[*].text: truncate",
    ] {
        let errors = compile(&source(fit)).unwrap_err();
        assert!(
            errors.iter().any(|error| error
                .config_path
                .as_deref()
                .is_some_and(|path| path.contains(".fit."))),
            "{errors:?}"
        );
    }
}

#[test]
fn unavailable_provider_validation_does_not_authorize_reductions() {
    let error = fit_input(
        json!({"rows": [1,2]}),
        &[rule("rows", FitStrategy::Recent)],
        |_, report| {
            assert!(report.omissions.is_empty());
            Err(FitValidationError::Unavailable(
                "Provider limits are unavailable.".into(),
            ))
        },
    )
    .unwrap_err();
    assert_eq!(error.code, PreparationFailureCode::ProviderUnavailable);
}

#[test]
fn no_fit_reasons_distinguish_rules_matches_floors_and_exhausted_reductions() {
    for (input, rules, reason) in [
        (json!({}), vec![], NoFitReason::NoRules),
        (
            json!({"rows": [{}]}),
            vec![rule("rows[*].text", FitStrategy::Truncate)],
            NoFitReason::NoMatchingTargets,
        ),
        (
            json!({"rows": []}),
            vec![rule("rows", FitStrategy::Recent)],
            NoFitReason::TargetsAtMinimum,
        ),
        (
            json!({"text": "x".repeat(256)}),
            vec![rule("text", FitStrategy::Truncate)],
            NoFitReason::TargetsAtMinimum,
        ),
        (
            json!({"rows": [1, 2]}),
            vec![rule("rows", FitStrategy::Recent)],
            NoFitReason::ReductionsExhausted,
        ),
    ] {
        let error = fit_input(input, &rules, |_, _| Err(over_budget())).unwrap_err();
        assert_eq!(error.code, PreparationFailureCode::NoFit);
        assert_eq!(error.context.no_fit_reason, Some(reason));
        assert_eq!(
            error.context.budget.unwrap().kind,
            PreparationBudgetKind::EstimatedTokens
        );
    }
}

#[test]
fn oversized_source_is_rejected_before_expansion_or_request_validation() {
    let input = json!({"rows": vec![Value::String("x".into()); 200_000]});
    let error = fit_input(input, &[rule("rows[*]", FitStrategy::Truncate)], |_, _| {
        panic!("source admission must run first")
    })
    .unwrap_err();
    assert_eq!(error.code, PreparationFailureCode::PreparationResourceLimit);
    let budget = error.context.budget.unwrap();
    assert_eq!(budget.kind, PreparationBudgetKind::InputNodes);
    assert_eq!(
        budget.observed,
        u64::try_from(MAX_FIT_INPUT_NODES + 1).unwrap()
    );
    assert_eq!(error.context.field.as_deref(), Some("input"));
    assert!(error.report.is_none());
}

#[test]
fn expansion_and_traversal_have_independent_pre_request_limits() {
    let error = fit_input(
        json!({"rows": vec!["x"; MAX_FIT_TARGETS + 1]}),
        &[rule("rows[*]", FitStrategy::Truncate)],
        |_, _| panic!("expanded targets are bounded first"),
    )
    .unwrap_err();
    assert_eq!(error.code, PreparationFailureCode::PreparationResourceLimit);
    assert_eq!(
        error.context.budget.unwrap().kind,
        PreparationBudgetKind::ExpandedTargets
    );
    assert_eq!(error.context.input_path.as_deref(), Some("rows[*]"));

    let rules = (0..MAX_FIT_RULES)
        .map(|index| rule(&format!("rows[*].optional_{index}"), FitStrategy::Truncate))
        .collect::<Vec<_>>();
    let error = fit_input(json!({"rows": vec![json!({}); 2000]}), &rules, |_, _| {
        panic!("wildcard visits are bounded first")
    })
    .unwrap_err();
    let budget = error.context.budget.unwrap();
    assert_eq!(budget.kind, PreparationBudgetKind::TraversalNodes);
    assert_eq!(
        budget.observed,
        u64::try_from(MAX_FIT_TRAVERSAL_NODES + 1).unwrap()
    );
}

#[test]
fn input_and_path_depth_are_checked_before_recursive_expansion() {
    let mut nested = Value::Null;
    for _ in 0..=MAX_FIT_INPUT_DEPTH {
        nested = Value::Array(vec![nested]);
    }
    let error = fit_input(nested, &[], |_, _| {
        panic!("input depth admission runs first")
    })
    .unwrap_err();
    assert_eq!(
        error.context.budget.unwrap().kind,
        PreparationBudgetKind::InputDepth
    );
    let path = std::iter::repeat_n("field", MAX_FIT_PATH_SEGMENTS + 1)
        .collect::<Vec<_>>()
        .join(".");
    let error = fit_input(json!({}), &[rule(&path, FitStrategy::Truncate)], |_, _| {
        panic!("path depth admission runs first")
    })
    .unwrap_err();
    assert_eq!(
        error.context.budget.unwrap().kind,
        PreparationBudgetKind::PathDepth
    );
}

#[test]
fn serialized_byte_admission_counts_escaping_and_stops_without_a_body_buffer() {
    let value = json!({"text": "\0".repeat(1000)});
    let error =
        measure_preparation_bytes(&value, 100, PreparationBudgetKind::InputBytes).unwrap_err();
    assert_eq!(error.code, PreparationFailureCode::PreparationResourceLimit);
    let budget = error.context.budget.unwrap();
    assert_eq!(budget.kind, PreparationBudgetKind::InputBytes);
    assert!(budget.observed > budget.maximum);
    assert!(budget.observed <= 106);
}

#[test]
fn turn_exhaustion_does_not_claim_that_all_permitted_reductions_are_exhausted() {
    let original_count = MAX_FIT_TURNS + 2;
    let mut validations = 0;
    let error = fit_input(
        json!({"rows": vec![Value::Null; original_count]}),
        &[rule("rows", FitStrategy::Recent)],
        |_, _| {
            validations += 1;
            Err(over_budget())
        },
    )
    .unwrap_err();
    assert_eq!(error.code, PreparationFailureCode::PreparationResourceLimit);
    assert!(error.context.no_fit_reason.is_none());
    assert_eq!(
        error.context.budget.unwrap().kind,
        PreparationBudgetKind::Turns
    );
    assert_eq!(validations, MAX_FIT_TURNS + 1);
    assert_eq!(error.report.unwrap().omissions[0].retained_count, 2);
}

#[test]
fn first_marker_turn_must_shorten_the_serialized_string() {
    let marker_bytes = serde_json::to_string(OMISSION_MARKER).unwrap().len() - 2;
    for removed in [1, marker_bytes, marker_bytes + 1] {
        let original = "x".repeat(MINIMUM_TEXT_BYTES + removed);
        let result = fit_input(
            json!({"text": original}),
            &[rule("text", FitStrategy::Truncate)],
            |_, report| {
                if report.omissions.is_empty() {
                    Err(over_budget())
                } else {
                    Ok(())
                }
            },
        );
        if removed <= marker_bytes {
            let error = result.unwrap_err();
            assert_eq!(
                error.context.no_fit_reason,
                Some(NoFitReason::TargetsAtMinimum)
            );
            assert!(error.report.unwrap().omissions.is_empty());
        } else {
            let (input, report) = result.unwrap();
            assert_eq!(report.omissions[0].retained_count, MINIMUM_TEXT_BYTES);
            assert!(
                serde_json::to_string(input["text"].as_str().unwrap())
                    .unwrap()
                    .len()
                    < serde_json::to_string(&original).unwrap().len()
            );
        }
    }
    let original = format!("{}{}{}", "a".repeat(128), "\0".repeat(4), "b".repeat(128));
    let (input, report) = fit_input(
        json!({"text": original}),
        &[rule("text", FitStrategy::Truncate)],
        |_, report| {
            if report.omissions.is_empty() {
                Err(over_budget())
            } else {
                Ok(())
            }
        },
    )
    .unwrap();
    assert_eq!(report.omissions[0].original_count, 260);
    assert_eq!(report.omissions[0].retained_count, 256);
    assert!(
        serde_json::to_string(&input["text"]).unwrap().len()
            < serde_json::to_string(&original).unwrap().len()
    );
}

#[test]
fn unicode_alignment_never_removes_more_than_one_chunk() {
    let original = "🚀".repeat(325);
    let (input, report) = fit_input(
        json!({"text": original}),
        &[rule("text", FitStrategy::Truncate)],
        |_, report| {
            if report.omissions.is_empty() {
                Err(over_budget())
            } else {
                Ok(())
            }
        },
    )
    .unwrap();
    assert_eq!(report.omissions[0].original_count, 1300);
    assert_eq!(report.omissions[0].retained_count, 280);
    let (head, tail) = input["text"]
        .as_str()
        .unwrap()
        .split_once(OMISSION_MARKER)
        .unwrap();
    assert!(original.starts_with(head));
    assert!(original.ends_with(tail));
    assert_eq!(head.len() + tail.len(), 280);
}

#[test]
fn compiler_bounds_rule_count_and_path_size_before_pairwise_validation() {
    let rules = (0..=MAX_FIT_RULES)
        .map(|index| format!("      z.field_{index}: truncate"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(compile(&source(&rules)).is_err());
    let path = format!("z.{}", "a".repeat(MAX_FIT_PATH_BYTES));
    assert!(parse_fit_path(&path).is_none());
    assert!(compile(&source(&format!("      {path}: truncate"))).is_err());
}
