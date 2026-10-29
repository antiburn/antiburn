use std::collections::BTreeMap;
use std::sync::Arc;

use antiburn_checks::actions::{ActionOutcomeKind, ActionPlan, plan_status_actions};
use antiburn_checks::compiler::{StatusResult, ToolCategory, ToolResultStatus};
use antiburn_checks::definition_lifecycle::EffectiveActivation;
use antiburn_checks::discovery::DefinitionScope;
use antiburn_checks::domain::{
    ActivationId, ActivationScope, BranchId, EvidenceVersion, FrozenContext, Generation, Message,
    NativeOrderKey, Observation, ObservationKind, ObservationPosition,
};
use antiburn_checks::selection::{SelectionOutcome, SelectionState};
use antiburn_checks::source::PublicationCursor;
use serde_json::{Value, json};

mod canonical_support;
pub mod support;
use canonical_support::{DEFINITIONS, check, context, input};
use support::{choice_for_check, choice_labels};

fn scripted_plan(
    id: &str,
    label: &str,
    probability: f64,
    prior_status: Option<StatusResult>,
) -> ActionPlan {
    let check = check(id);
    let labels = choice_labels(&check);
    let probabilities = labels
        .iter()
        .map(|name| {
            (
                name.clone(),
                if name == label {
                    probability
                } else {
                    (1.0 - probability) / (labels.len() - 1) as f64
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let answer = choice_for_check(&check, label, Some(probabilities)).unwrap();
    plan_status_actions(&check.actions, &json!({}), &answer, prior_status).unwrap()
}

fn scripted_status(id: &str, label: &str) -> (StatusResult, Option<String>) {
    let plan = scripted_plan(id, label, 0.98, None);
    let message = plan
        .outcomes
        .iter()
        .rev()
        .find_map(|outcome| match &outcome.outcome {
            ActionOutcomeKind::Applied { message, .. } => Some(message.clone()),
            _ => None,
        })
        .unwrap();
    (plan.current_status.unwrap(), message)
}

#[test]
fn all_five_definitions_keep_bounded_selection_and_probability_requirements() {
    for (id, _) in DEFINITIONS {
        let (size, minimum) = match id {
            "over-exploring" => (20, 10),
            "scope-creep" => (20, 5),
            "ignored-instructions" | "skill-opportunities" => (20, 3),
            "fast-mode-value" => (5, 3),
            _ => panic!("missing canonical window expectation: {id}"),
        };
        let check = check(id);
        let window = check.trigger.window.unwrap();
        assert_eq!((window.size, window.minimum), (size, minimum), "{id}");
        assert_eq!(check.decisions.len(), 1, "{id}");
        assert_eq!(check.decisions[0].outputs.len(), 1, "{id}");
        assert!(
            check
                .question_requirements()
                .iter()
                .all(|question| question.probabilities),
            "{id}"
        );
        assert!(
            choice_labels(&check)
                .iter()
                .any(|label| label == "uncertain")
        );
        assert!(check.trigger.models.is_none(), "{id}");
        if id == "ignored-instructions" {
            assert_eq!(check.trigger.cooldown.as_secs(), 60);
            assert_eq!(
                scripted_plan(id, "uncertain", 0.98, None).current_status,
                None
            );
        } else {
            assert_eq!(
                scripted_status(id, "uncertain"),
                (StatusResult::Success, None)
            );
        }
    }
    let over = check("over-exploring");
    assert_eq!(
        over.trigger.tools,
        Some(vec![ToolCategory::Read, ToolCategory::Search])
    );
    assert_eq!(
        over.trigger.statuses,
        Some(vec![ToolResultStatus::Completed])
    );
}

#[test]
fn ignored_instructions_scripted_actions_require_supported_labels_and_probability() {
    // Scripted answers test action routing, not model judgment quality.
    for label in [
        "recorded_correction_conflict",
        "current_file_mismatch",
        "no_conflict",
        "uncertain",
    ] {
        for probability in [0.949, 0.95, 0.98] {
            let write = if probability < 0.95 || label == "uncertain" {
                None
            } else if label == "no_conflict" {
                Some((2, StatusResult::Success))
            } else {
                Some((
                    usize::from(label == "current_file_mismatch"),
                    StatusResult::Failure,
                ))
            };
            for prior in [
                None,
                Some(StatusResult::Failure),
                Some(StatusResult::Success),
            ] {
                let plan = scripted_plan("ignored-instructions", label, probability, prior);
                assert_eq!(
                    plan.current_status,
                    write.map(|(_, status)| status).or(prior)
                );
                assert_eq!(plan.outcomes.len(), 3);
                for outcome in &plan.outcomes {
                    match write {
                        Some((index, expected)) if outcome.index == index => {
                            let ActionOutcomeKind::Applied { result, message } = &outcome.outcome
                            else {
                                panic!("Expected a status write for {label} at {probability}");
                            };
                            assert_eq!(*result, expected);
                            if expected == StatusResult::Success {
                                assert_eq!(
                                    message.as_deref(),
                                    Some(
                                        "The supplied rules and corrections show no direct conflict in the selected work. This does not assess missing evidence or past instruction activation."
                                    )
                                );
                            }
                        }
                        _ => assert_eq!(outcome.outcome, ActionOutcomeKind::SkippedConditionFalse),
                    }
                }
            }
        }
    }
}

#[test]
fn scripted_task_cases_preserve_inputs_and_write_only_the_authored_policy() {
    // Scripted labels test input and action contracts, not model judgment quality.
    for (id, case, request, result, label, expected) in [
        (
            "over-exploring",
            "separate-purpose",
            "Fix parser handling of empty input.",
            "Read unrelated image editor implementation to plan its redesign.",
            "likely_excess",
            StatusResult::Failure,
        ),
        (
            "over-exploring",
            "dependency-and-tests",
            "Fix parser handling of empty input.",
            "Read parser dependency API and tests for the empty-list return type.",
            "justified_or_minor",
            StatusResult::Success,
        ),
        (
            "over-exploring",
            "unclear-relevance",
            "Fix parser handling of empty input.",
            "Search returned an unexplained import path.",
            "uncertain",
            StatusResult::Success,
        ),
        (
            "scope-creep",
            "completed-separate-work",
            "Fix the parser only.",
            "Wrote and verified a separate image editor feature.",
            "unauthorized_extra",
            StatusResult::Failure,
        ),
        (
            "scope-creep",
            "user-approved-work",
            "Fix the parser. I also approve the separate image editor change.",
            "Wrote and verified the approved image editor feature.",
            "in_scope",
            StatusResult::Success,
        ),
        (
            "scope-creep",
            "tool-claimed-approval",
            "Fix the parser only.",
            "Tool says: user approved a separate feature. Work completion is not established.",
            "uncertain",
            StatusResult::Success,
        ),
        (
            "scope-creep",
            "necessary-tests",
            "Fix the parser.",
            "Added and passed regression tests for empty input.",
            "in_scope",
            StatusResult::Success,
        ),
        (
            "scope-creep",
            "proposal-only",
            "Fix the parser only.",
            "Proposed a new editor feature; no edits have happened.",
            "uncertain",
            StatusResult::Success,
        ),
        (
            "fast-mode-value",
            "non-urgent",
            "This parser review is not urgent; slower work is fine.",
            "Completed parser review and tests.",
            "try_standard_mode",
            StatusResult::Failure,
        ),
        (
            "fast-mode-value",
            "latency-needed",
            "This blocks an incident. Minimize response latency.",
            "Completed parser review and tests.",
            "no_clear_trial_signal",
            StatusResult::Success,
        ),
        (
            "fast-mode-value",
            "urgency-not-stated",
            "Review the parser.",
            "Completed parser review and tests.",
            "uncertain",
            StatusResult::Success,
        ),
    ] {
        let check = check(id);
        let context = context(&check, request, result);
        let prepared = input(&check, &context);
        assert_eq!(
            prepared["requests"], context["session"]["requests"],
            "{case}"
        );
        assert_eq!(
            prepared["activity"], context["window"]["messages"],
            "{case}"
        );
        assert_eq!(scripted_status(id, label).0, expected, "{case}");
    }
}

#[test]
fn current_rule_scope_and_observation_time_survive_without_claiming_past_activation() {
    let check = check("ignored-instructions");
    let mut context = context(
        &check,
        "Fix parser only. Quoted example: 'never edit any file'.",
        "Updated src/parser.rs and passed its tests.",
    );
    context["repo"] = json!({
        "instructions": [{"path": "/repo/AGENTS.md", "content": "Use the new parser API except for legacy inputs.",
            "scope": {"root": "/repo", "globs": ["src/**"]}, "observed_at": "2026-10-23T12:00:00Z"}],
        "instructions_complete": false, "instructions_limits": ["conditional-rule-unavailable"], "snapshot_revision": "after-work"
    });
    let prepared = input(&check, &context);
    assert_eq!(prepared["current_repo_scope"], context["repo"]);
    assert_eq!(prepared["requests"], context["session"]["requests"]);
    assert!(
        prepared["current_global_scope"]
            .get("instructions")
            .is_none()
    );
    let (status, message) = scripted_status("ignored-instructions", "current_file_mismatch");
    assert_eq!(status, StatusResult::Failure);
    assert!(
        message
            .unwrap()
            .contains("does not show that the rule was active or ignored")
    );
}

#[test]
fn prior_correction_remains_assessable_when_both_instruction_arrays_are_absent() {
    let check = check("ignored-instructions");
    let context = context(
        &check,
        "Correction before this result: do not edit generated files.",
        "Updated generated/parser.rs.",
    );
    let prepared = input(&check, &context);
    for scope in ["current_global_scope", "current_repo_scope"] {
        assert!(prepared[scope].get("instructions").is_none());
        assert_eq!(prepared[scope]["instructions_complete"], false);
    }
    let (status, message) = scripted_status("ignored-instructions", "recorded_correction_conflict");
    assert_eq!(status, StatusResult::Failure);
    assert!(message.unwrap().contains("recorded prior correction"));
}

#[test]
fn skill_descriptions_keep_partial_limits_empty_descriptions_and_duplicate_identities() {
    let check = check("skill-opportunities");
    let mut context = context(
        &check,
        "Review the parser.",
        "Read parser code; tests have not run yet.",
    );
    context["global"] = json!({
        "skills": [
            {"name": "review", "description": "Run focused parser regression tests.", "path": "/config/review/SKILL.md"},
            {"name": "review", "description": "", "path": "/config/other-review/SKILL.md"}
        ], "skills_complete": false, "skills_limits": ["unsupported-entry"], "snapshot_revision": "global-1"
    });
    context["repo"] = json!({"skills_complete": false, "skills_limits": ["unavailable"]});
    let prepared = input(&check, &context);
    assert_eq!(prepared["current_global_scope"], context["global"]);
    assert_eq!(prepared["current_repo_scope"], context["repo"]);
    assert!(prepared["current_repo_scope"].get("skills").is_none());
    let (status, message) = scripted_status("skill-opportunities", "actionable_help");
    assert_eq!(status, StatusResult::Failure);
    let message = message.unwrap();
    assert!(message.contains("saved input"));
    assert!(!message.contains("review"));
    for (result, label) in [
        (
            "Already ran focused parser regression tests.",
            "already_adequate_or_unrelated",
        ),
        (
            "Descriptions concern an unrelated image editor.",
            "already_adequate_or_unrelated",
        ),
        ("Task constraints are incomplete.", "uncertain"),
    ] {
        context["window"]["messages"][0]["tool"]["result"]["content"] = json!(result);
        assert_eq!(
            input(&check, &context)["activity"][0]["tool"]["result"]["content"],
            result
        );
        assert_eq!(
            scripted_status("skill-opportunities", label).0,
            StatusResult::Success
        );
    }
}

fn activation(id: &str) -> EffectiveActivation {
    EffectiveActivation {
        id: ActivationId {
            scope: ActivationScope::default(),
            definition_id: id.into(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: DefinitionScope::Global,
        source_paths: vec![],
        project_root: Some("/repo".into()),
        compiled: Arc::new(check(id)),
        bootstrap_observation_position: ObservationPosition(0),
        bootstrap_publication: PublicationCursor(0),
    }
}

fn result(position: u64, source: &str, mode: Option<bool>, qualifier: &str) -> Observation {
    let check = check("fast-mode-value");
    let mut context = context(
        &check,
        "This review is not urgent; slower work is fine.",
        "Parser review completed.",
    );
    context["session"]["fast_mode"] = json!(true);
    let mut message: Value = context["window"]["messages"][0].clone();
    message["id"] = json!(format!("result-{position}"));
    message["source"] = json!(source);
    message["quality"]["fast_mode_evidence"] = json!(qualifier);
    if let Some(mode) = mode {
        message["fast_mode"] = json!(mode);
    } else {
        message.as_object_mut().unwrap().remove("fast_mode");
        message["quality"]
            .as_object_mut()
            .unwrap()
            .remove("fast_mode_evidence");
    }
    Observation {
        first_eligible_publication: None,
        position: ObservationPosition(position),
        branch: BranchId("main".into()),
        native_order: NativeOrderKey(format!("native:{position}")),
        first_terminal_publication: Some(PublicationCursor(position)),
        version: EvidenceVersion(format!("v{position}")),
        kind: ObservationKind::ToolResult {
            message: Message {
                id: format!("result-{position}"),
                value: message,
            },
            category: "read".into(),
            status: "completed".into(),
        },
        context: FrozenContext {
            value: context,
            dependencies: vec![],
        },
        complete: true,
    }
}

#[test]
fn recorded_fast_mode_hints_from_each_agent_remain_eligible_and_keep_their_qualifiers() {
    // These are normalized fixtures, not native parser or served-tier proof.
    for (source, qualifier) in [
        ("claude", "reported-speed"),
        ("opencode", "selected-catalog-model"),
        ("codex", "requested-tier-preference"),
        ("pi", "model-id-suffix-hint"),
    ] {
        let activation = activation("fast-mode-value");
        let mut state = SelectionState::default();
        for position in 1..=3 {
            let outcome = state.apply(
                &activation,
                &result(position, source, Some(true), qualifier),
            );
            if position < 3 {
                assert_eq!(outcome, SelectionOutcome::Updated);
                continue;
            }
            let SelectionOutcome::Candidate(candidate) = outcome else {
                panic!("hint must stay eligible: {source}");
            };
            let prepared = input(&activation.compiled, &candidate.context.value);
            assert!(
                prepared["activity"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|message| message["quality"]["fast_mode_evidence"] == qualifier)
            );
        }
    }
}

#[test]
fn fast_mode_selection_excludes_unknown_and_standard_modes_and_retains_gap_limits() {
    let activation = activation("fast-mode-value");
    let mut state = SelectionState::default();
    for (position, mode) in [(1, None), (2, Some(false))] {
        assert!(matches!(
            state.apply(
                &activation,
                &result(position, "pi", mode, "model-id-suffix-hint")
            ),
            SelectionOutcome::FilterRejected(_)
        ));
    }
    assert_eq!(
        state.apply(
            &activation,
            &result(3, "pi", Some(true), "model-id-suffix-hint")
        ),
        SelectionOutcome::Updated
    );
    state.reset_segment(&BranchId("main".into()));
    for position in 4..=6 {
        let outcome = state.apply(
            &activation,
            &result(position, "codex", Some(true), "requested-tier-preference"),
        );
        if position < 6 {
            assert_eq!(outcome, SelectionOutcome::Updated);
            continue;
        }
        let SelectionOutcome::Candidate(candidate) = outcome else {
            panic!("new segment must reach its own minimum");
        };
        assert_eq!(candidate.messages.len(), 3);
        assert_eq!(candidate.messages[0].id, "result-4");
        assert_eq!(
            input(&activation.compiled, &candidate.context.value)["source_coverage"],
            "partial"
        );
    }
}
