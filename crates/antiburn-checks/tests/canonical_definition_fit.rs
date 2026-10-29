use antiburn_checks::fit::{
    FitStrategy, FitValidationError, OMISSION_MARKER, PreparationBudget, PreparationBudgetKind,
    PreparationFailureCode, fit_input,
};
use serde_json::{Value, json};

mod canonical_support;
use canonical_support::{DEFINITIONS, check, context, input};

fn over_budget() -> FitValidationError {
    FitValidationError::OverBudget(PreparationBudget {
        kind: PreparationBudgetKind::EstimatedTokens,
        observed: 2,
        maximum: 1,
    })
}

#[test]
fn every_definition_keeps_its_exact_fit_rules_without_dropping_evidence_records() {
    for (id, _) in DEFINITIONS {
        let check = check(id);
        let fit = &check.decisions[0].fit;
        let expected = if id == "ignored-instructions" {
            vec![
                "current_global_scope.skills[*].description",
                "current_repo_scope.skills[*].description",
                "activity[*].tool.result.content",
            ]
        } else {
            vec!["activity[*].tool.result.content"]
        };
        assert_eq!(
            fit.iter()
                .map(|rule| rule.path.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{id}"
        );
        assert!(
            fit.iter()
                .all(|rule| rule.strategy == FitStrategy::Truncate),
            "{id}"
        );
        let context = context(
            &check,
            "Fix the parser. Keep the exception: do not change generated files.",
            &"result output ".repeat(400),
        );
        let original = input(&check, &context);
        let (mut fitted, report) = fit_input(original.clone(), fit, |_, report| {
            if report.omissions.len() == original["activity"].as_array().unwrap().len() {
                Ok(())
            } else {
                Err(over_budget())
            }
        })
        .unwrap();
        assert_eq!(fitted["requests"], original["requests"], "{id}");
        for (index, omission) in report.omissions.iter().enumerate() {
            assert_eq!(
                omission.path,
                format!("activity[{index}].tool.result.content")
            );
            assert!(omission.retained_count < omission.original_count);
            assert!(
                fitted["activity"][index]["tool"]["result"]["content"]
                    .as_str()
                    .unwrap()
                    .contains(OMISSION_MARKER)
            );
            fitted["activity"][index]["tool"]["result"]["content"] =
                original["activity"][index]["tool"]["result"]["content"].clone();
        }
        assert_eq!(fitted, original, "only result text may change: {id}");
    }
}

#[test]
fn oversized_requests_reject_instead_of_losing_corrections_or_approvals() {
    for (id, _) in DEFINITIONS {
        let check = check(id);
        let context = context(
            &check,
            &format!(
                "{} Keep the correction and approval exception.",
                "request ".repeat(500)
            ),
            &"output ".repeat(600),
        );
        let original = input(&check, &context);
        let failure = fit_input(original.clone(), &check.decisions[0].fit, |fitted, _| {
            assert_eq!(fitted["requests"], original["requests"], "{id}");
            assert_eq!(
                fitted["activity"].as_array().unwrap().len(),
                check.trigger.window.unwrap().minimum as usize
            );
            if fitted["requests"][0]["text"].as_str().unwrap().len() < 512 {
                Ok(())
            } else {
                Err(over_budget())
            }
        })
        .unwrap_err();
        assert_eq!(failure.code, PreparationFailureCode::NoFit, "{id}");
        assert!(
            failure
                .report
                .unwrap()
                .omissions
                .iter()
                .all(|entry| entry.path.starts_with("activity["))
        );
    }
}

#[test]
fn oversized_instructions_reject_while_only_unused_descriptions_can_shrink() {
    for id in ["ignored-instructions", "skill-opportunities"] {
        let check = check(id);
        let mut context = context(&check, "Review the parser.", "read output");
        context["repo"]["instructions"] = json!([{
            "path": "/repo/AGENTS.md", "content": "rule and exception ".repeat(400),
            "scope": {"root": "/repo", "globs": ["src/**"]}, "observed_at": "2026-10-23T12:00:00Z"
        }]);
        context["global"]["skills"] = json!([{
            "name": "review", "description": "review step ".repeat(400), "path": "/config/review/SKILL.md"
        }]);
        let original = input(&check, &context);
        let failure = fit_input(original.clone(), &check.decisions[0].fit, |fitted, _| {
            let mut restored = fitted.clone();
            if id == "ignored-instructions" {
                restored["current_global_scope"]["skills"][0]["description"] =
                    original["current_global_scope"]["skills"][0]["description"].clone();
            }
            assert_eq!(restored, original, "{id}");
            Err(over_budget())
        })
        .unwrap_err();
        assert_eq!(failure.code, PreparationFailureCode::NoFit, "{id}");
        let omissions = failure.report.unwrap().omissions;
        if id == "ignored-instructions" {
            assert_eq!(omissions.len(), 1);
            assert_eq!(
                omissions[0].path,
                "current_global_scope.skills[0].description"
            );
            assert_eq!(omissions[0].retained_count, 256);
        } else {
            assert!(omissions.is_empty());
        }
    }
}

fn ignored_input() -> (antiburn_checks::CompiledCheck, Value) {
    let check = check("ignored-instructions");
    let mut context = context(&check, "Fix the parser.", "completed output");
    for scope in ["global", "repo"] {
        context[scope] = json!({
            "instructions": [{
                "path": format!("/synthetic/{scope}/AGENTS.md"),
                "content": "Keep generated files unchanged, except src/parser.rs.",
                "scope": {"root": "/synthetic/repo", "globs": ["src/**"]},
                "observed_at": "2026-10-27T00:00:00Z"
            }],
            "instructions_complete": false, "instructions_limits": ["unresolved-source"],
            "skills": [{"name": "review", "description": "x".repeat(8192),
                "path": format!("/synthetic/{scope}/review/SKILL.md")}],
            "skills_complete": false, "skills_limits": ["unsupported-entry"],
            "snapshot_revision": format!("{scope}-1")
        });
    }
    context["session"]["requests"] = json!([
        {"id": "request-1", "source": "synthetic", "kind": "user_message", "text": "Fix the parser only."},
        {"id": "request-2", "source": "synthetic", "kind": "user_message", "text": "Correction: do not edit generated files."},
        {"id": "request-3", "source": "synthetic", "kind": "user_message", "text": "Exception: src/parser.rs may change."}
    ]);
    context["session"]["requests_complete"] = json!(false);
    context["session"]["source_coverage"] = json!("partial");
    let message = context["window"]["messages"][0].clone();
    context["window"]["messages"] = (0..20)
        .map(|index| {
            let mut message = message.clone();
            message["id"] = json!(format!("result-{index}"));
            message["native_order"] = json!(format!("native:{index}"));
            message["quality"]["request_history_complete"] = json!(false);
            message["tool"]["arguments"] =
                json!({"path": "src/parser.rs", "patch": "Keep this exact argument.\né"});
            message
        })
        .collect();
    let original = input(&check, &context);
    (check, original)
}

#[test]
fn ignored_instructions_already_fitting_input_is_byte_exact() {
    let (check, original) = ignored_input();
    let (fitted, report) =
        fit_input(original.clone(), &check.decisions[0].fit, |_, _| Ok(())).unwrap();
    assert_eq!(
        serde_json::to_vec(&fitted).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
    assert!(report.omissions.is_empty());
}

#[test]
fn ignored_instructions_single_description_turn_preserves_all_twenty_results_and_requests() {
    let (check, mut original) = ignored_input();
    original["current_global_scope"]["skills"][0]["description"] = json!("s".repeat(16 * 1024));
    original["current_repo_scope"]["skills"] = json!([]);
    let maximum = serde_json::to_vec(&original).unwrap().len() - 512;
    let (mut fitted, report) = fit_input(original.clone(), &check.decisions[0].fit, |fitted, _| {
        if serde_json::to_vec(fitted).unwrap().len() <= maximum {
            Ok(())
        } else {
            Err(over_budget())
        }
    })
    .unwrap();
    assert_eq!(report.omissions.len(), 1);
    assert_eq!(
        report.omissions[0].path,
        "current_global_scope.skills[0].description"
    );
    assert_eq!(report.omissions[0].original_count, 16 * 1024);
    assert_eq!(report.omissions[0].retained_count, 15 * 1024);
    assert_eq!(fitted["activity"].as_array().unwrap().len(), 20);
    assert_eq!(fitted["activity"], original["activity"]);
    assert_eq!(fitted["requests"], original["requests"]);
    fitted["current_global_scope"]["skills"][0]["description"] =
        original["current_global_scope"]["skills"][0]["description"].clone();
    assert_eq!(
        serde_json::to_vec(&fitted).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
}

#[test]
fn ignored_instructions_description_truncation_keeps_unicode_edges_and_scope_metadata() {
    let (check, mut original) = ignored_input();
    let scopes = ["current_global_scope", "current_repo_scope"];
    for scope in scopes {
        original[scope]["skills"][0]["description"] = json!("aé漢".repeat(2048));
    }
    let (mut fitted, report) = fit_input(original.clone(), &check.decisions[0].fit, |fitted, _| {
        if scopes.iter().all(|scope| {
            fitted[scope]["skills"][0]["description"]
                .as_str()
                .unwrap()
                .len()
                <= 4096
        }) {
            Ok(())
        } else {
            Err(over_budget())
        }
    })
    .unwrap();
    assert_eq!(report.omissions.len(), 2);
    for (scope, omission) in scopes.into_iter().zip(&report.omissions) {
        assert_eq!(omission.path, format!("{scope}.skills[0].description"));
        assert_eq!(omission.strategy, FitStrategy::Truncate);
        let description = original[scope]["skills"][0]["description"]
            .as_str()
            .unwrap();
        let retained = fitted[scope]["skills"][0]["description"].as_str().unwrap();
        let (head, tail) = retained.split_once(OMISSION_MARKER).unwrap();
        assert!(description.starts_with(head));
        assert!(description.ends_with(tail));
        assert_eq!(retained.matches(OMISSION_MARKER).count(), 1);
        assert_eq!(omission.original_count, description.len());
        assert_eq!(omission.retained_count, head.len() + tail.len());
        assert!(omission.retained_count >= 256);
        fitted[scope]["skills"][0]["description"] =
            original[scope]["skills"][0]["description"].clone();
    }
    assert_eq!(
        serde_json::to_vec(&fitted).unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
}

#[test]
fn ignored_instructions_skips_missing_targets_and_keeps_known_empty_and_partial_scopes() {
    let (check, original) = ignored_input();
    for scope in ["current_global_scope", "current_repo_scope"] {
        for skills in [
            None,
            Some(json!([])),
            Some(
                json!([{"name": "review", "description": "", "path": "/synthetic/review/SKILL.md"}]),
            ),
            Some(json!([{"name": "review", "path": "/synthetic/review/SKILL.md"}])),
        ] {
            let mut original = original.clone();
            let object = original[scope].as_object_mut().unwrap();
            object.remove("instructions");
            match skills {
                Some(skills) => {
                    object.insert("skills".into(), skills);
                }
                None => {
                    object.remove("skills");
                }
            }
            let (fitted, report) =
                fit_input(original.clone(), &check.decisions[0].fit, |_, _| Ok(())).unwrap();
            assert_eq!(
                serde_json::to_vec(&fitted).unwrap(),
                serde_json::to_vec(&original).unwrap()
            );
            assert_eq!(fitted["request_history_complete"], false);
            assert!(report.omissions.is_empty());
        }
    }
}

#[test]
fn ignored_instructions_rejects_present_wrong_skill_types_before_request_validation() {
    let (check, original) = ignored_input();
    for scope in ["current_global_scope", "current_repo_scope"] {
        for skills in [
            json!(null),
            json!({}),
            json!([null]),
            json!([42]),
            json!([{"description": null}]),
            json!([{"description": 42}]),
            json!([{"description": {}}]),
        ] {
            let mut original = original.clone();
            original[scope]["skills"] = skills;
            let failure = fit_input(original, &check.decisions[0].fit, |_, _| {
                panic!("Wrong types must fail before request validation")
            })
            .unwrap_err();
            assert_eq!(failure.code, PreparationFailureCode::InvalidFit);
        }
    }
}

#[test]
fn missing_required_requests_do_not_become_an_empty_history() {
    for (id, _) in DEFINITIONS {
        let check = check(id);
        let mut context = context(&check, "Review the parser.", "read output");
        context["session"]
            .as_object_mut()
            .unwrap()
            .remove("requests");
        let evaluated = antiburn_checks::preparation::evaluate_decision_input_with_budget(
            &check.decisions[0],
            &context,
            &mut antiburn_checks::expression_eval::EvaluationBudget::default(),
        )
        .unwrap();
        assert_eq!(
            evaluated.value,
            antiburn_checks::expression_eval::EvaluationValue::Unavailable,
            "{id}"
        );
    }
}
