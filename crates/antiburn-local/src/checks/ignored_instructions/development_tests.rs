use super::tests::{event, input};
use super::*;
use crate::analysis::jev::{JevUsage, capabilities::ModelCapabilities};
use crate::checks::ignored_instructions::{
    PrerequisiteContextPolicy, SamplingLedger, build_jev_context_with_context_policy,
};

#[test]
fn selected_native_context_controls_result_and_authorization_publication() {
    use crate::analysis::jev::obligations::{ConditionEvidence, PermissionRequirement};
    use crate::checks::ignored_instructions::selected_context::tests::{human, test_pair};
    for permission in [
        PermissionRequirement::Independent,
        PermissionRequirement::AuthoritativeApproval,
    ] {
        for complete_proof in [true, false] {
            for relationship in ["conflict", "follows"] {
                let mut actions = vec![human()];
                actions.extend(test_pair());
                if !complete_proof {
                    if permission == PermissionRequirement::Independent {
                        actions[2].truncated = true;
                    } else {
                        actions[0].metadata.user_text_history = None;
                    }
                }
                actions.push(event(
                    "release",
                    4,
                    "assistant",
                    "main",
                    "Published the release.",
                ));
                let source = input(
                    actions,
                    "Publish only after the focused tests pass and the user approves.",
                );
                let (_, plan) = classified(
                    &source,
                    PrerequisiteContextPolicy::CoherentEpisode,
                    "prerequisite",
                    "not_read_order",
                    "other_path",
                );
                let mut assessment = plan.prepared.clone();
                assessment
                    .comparisons
                    .retain(|comparison| comparison.reference.action_id == "release");
                let comparison = &assessment.comparisons[0];
                let obligation = assessment
                    .observable_obligations
                    .get_mut(&comparison.id)
                    .unwrap();
                obligation.permission = permission;
                obligation.condition_evidence = ConditionEvidence::Result;
                let answer = comparison_response(comparison, relationship, "self_contained");
                let result =
                    reduce_assessment(&assessment, &[(comparison.id.clone(), answer)].into(), true);
                assert_eq!(result.unassessed_comparisons.is_empty(), complete_proof);
                assert_eq!(
                    result.findings.len(),
                    usize::from(complete_proof && relationship == "conflict")
                );
                if let Some(finding) = result.findings.first() {
                    let decision = finding.decision_record().unwrap();
                    assert!(!decision.coverage.results_excluded);
                    assert!(!decision.coverage.user_authority_excluded);
                    assert!(
                        decision
                            .selected_evidence
                            .iter()
                            .any(|evidence| evidence.source.id == "test-result")
                    );
                }
                let item = plan
                    .work_items
                    .iter()
                    .find(|item| {
                        item.window.fields["candidate_action"]["text"] == "Published the release."
                    })
                    .unwrap();
                let context =
                    item.window.fields["instruction_targets"][0]["earlier_counterevidence"]
                        .as_array()
                        .unwrap();
                assert_eq!(
                    context
                        .iter()
                        .any(|event| event["native_context"]["evidence_kind"] == "human_text"),
                    permission == PermissionRequirement::Independent || complete_proof
                );
                assert_eq!(
                    context
                        .iter()
                        .any(|event| event["native_context"]["evidence_kind"] == "command_result"),
                    permission != PermissionRequirement::Independent || complete_proof
                );
            }
        }
    }
}

#[test]
fn command_result_window_uses_the_selected_request_after_raw_argument_normalization() {
    use crate::checks::ignored_instructions::selected_context::tests::{human, test_pair};
    let mut actions = vec![human()];
    let mut pair = test_pair();
    pair[0].text =
        json!({"command": "cargo test --lib", "description": "Native-only display text"})
            .to_string();
    actions.extend(pair);
    actions.push(event(
        "report",
        4,
        "assistant",
        "main",
        "All focused tests passed.",
    ));
    let source = input(actions, "Report test results accurately.");
    let (_, plan) = classified(
        &source,
        PrerequisiteContextPolicy::CoherentEpisode,
        "action",
        "not_read_order",
        "other_path",
    );
    let item = plan
        .work_items
        .iter()
        .find(|item| item.window.fields["candidate_action"]["text"] == "All focused tests passed.")
        .unwrap();
    let context = item.window.fields["instruction_targets"][0]["earlier_counterevidence"]
        .as_array()
        .unwrap();
    let result = context
        .iter()
        .find(|event| event["native_context"]["evidence_kind"] == "command_result")
        .unwrap();
    assert_eq!(
        result["native_context"]["matched_request"],
        "cargo test --lib"
    );
    assert_eq!(result["native_context"]["operation_state"], "completed");
    assert!(
        !serde_json::to_string(&item.window.fields)
            .unwrap()
            .contains("Native-only display text")
    );
}

fn answer(question: &JevQuestion, selected: &str) -> JevAnswer {
    let JevQuestion::Choice { criteria, .. } = question else {
        panic!("expected choice")
    };
    assert!(criteria.contains_key(selected), "{selected}");
    JevAnswer::Choice {
        choice: selected.to_owned(),
        confidence: 0.98,
        probabilities: criteria
            .keys()
            .map(|key| {
                (
                    key.clone(),
                    if key == selected {
                        0.98
                    } else {
                        0.02 / (criteria.len() - 1) as f64
                    },
                )
            })
            .collect(),
    }
}

fn classified(
    source: &AssessmentInput,
    policy: PrerequisiteContextPolicy,
    obligation: &str,
    read_requirement: &str,
    path_role: &str,
) -> (JevSessionContext, JevCheckPlan<AssessmentPlan>) {
    classified_with_path_policy(
        source,
        policy,
        obligation,
        read_requirement,
        path_role,
        "other_path",
    )
}

fn classified_with_path_policy(
    source: &AssessmentInput,
    policy: PrerequisiteContextPolicy,
    obligation: &str,
    read_requirement: &str,
    path_role: &str,
    path_policy: &str,
) -> (JevSessionContext, JevCheckPlan<AssessmentPlan>) {
    let context = build_jev_context_with_context_policy(
        source,
        &SamplingLedger::default(),
        &ModelCapabilities::jev_default(),
        policy,
    )
    .unwrap();
    let mut plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    let results = matching::rule_classifications(&plan.prepared)
        .unwrap()
        .into_iter()
        .map(|item| {
            let result = JevWorkItemResult {
                request_id: "development-classification".to_owned(),
                work_item_id: item.id.clone(),
                answers: item
                    .questions
                    .iter()
                    .map(|(key, question)| {
                        let choice = match key.as_str() {
                            "condition_evidence" => "selected",
                            "permission" => "independent",
                            "action_family" => "any",
                            "obligation" => obligation,
                            "read_prerequisite" => read_requirement,
                            "path_change_policy" => path_policy,
                            "read_trigger" => {
                                if read_requirement == "not_read_order" {
                                    "not_read_rule"
                                } else {
                                    "edit_request"
                                }
                            }
                            _ if key.starts_with("read_path_") => path_role,
                            _ if key.starts_with("literal_policy_") => "literal_other",
                            _ if key.starts_with("literal_qualification_") => "qualified",
                            _ => panic!("unexpected question {key}"),
                        };
                        (key.clone(), answer(question, choice))
                    })
                    .collect(),
                evidence: item.window.evidence,
                model: ASSESSMENT_MODEL.to_owned(),
                usage: JevUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            };
            (item.id, result)
        })
        .collect();
    matching::apply_rule_matching(&mut plan, &results, &context).unwrap();
    (context, plan)
}

fn tool(id: &str, order: i64, name: &str, value: Value) -> ContentAction {
    let mut action = event(id, order, "assistant", "main", &value.to_string());
    action.kind = "tool_input".to_owned();
    action.tool_name = Some(name.to_owned());
    action.tool_call_id = Some(format!("call-{id}"));
    action
}

fn comparison_response(
    comparison: &CandidateComparison,
    relationship: &str,
    basis: &str,
) -> JevWorkItemResult {
    JevWorkItemResult {
        request_id: "development-candidate".to_owned(),
        work_item_id: comparison.id.clone(),
        answers: comparison_questions(0)
            .iter()
            .map(|(key, question)| {
                let choice = match key.as_str() {
                    QUESTION_APPLICABILITY => "applies",
                    QUESTION_RELATIONSHIP => relationship,
                    QUESTION_EVIDENCE_BASIS => basis,
                    QUESTION_COMPLETION => "not_completion_obligation",
                    _ => panic!("unexpected question"),
                };
                (key.clone(), answer(question, choice))
            })
            .collect(),
        evidence: Vec::new(),
        model: ASSESSMENT_MODEL.to_owned(),
        usage: JevUsage {
            input_tokens: 0,
            output_tokens: 0,
        },
    }
}

#[test]
fn literal_report_proof_accepts_native_kind_and_window_clipping_but_not_source_loss() {
    use crate::analysis::jev::exact_facts::{LiteralPolicy, LiteralPolicyBinding};
    for kind in ["assistant", "assistant_text"] {
        for source_truncated in [false, true] {
            let text = format!(
                "{}I added disableLeaseGuard.",
                "Recorded UTF-8 context é🦀.\n".repeat(650)
            );
            let mut action = event("native-report", 1, "assistant", "main", &text);
            action.kind = kind.to_owned();
            action.truncated = source_truncated;
            let source = input(vec![action], "Do not add `disableLeaseGuard`.");
            let (_, mut plan) = classified(
                &source,
                PrerequisiteContextPolicy::CoherentEpisode,
                "action",
                "not_read_order",
                "other_path",
            );
            plan.prepared
                .comparisons
                .retain(|comparison| comparison.action.text.contains("I added disableLeaseGuard"));
            let comparison = &plan.prepared.comparisons[0];
            assert!(comparison.action.truncated);
            plan.prepared
                .observable_obligations
                .get_mut(&comparison.id)
                .unwrap()
                .literal_policies = vec![LiteralPolicyBinding {
                identifier: "disableLeaseGuard".to_owned(),
                policy: LiteralPolicy::ConstructBan,
                exact_match: None,
            }];
            for probability in [0.89, 0.90] {
                let mut response =
                    comparison_response(comparison, "conflict", "evidence_incomplete");
                response.answers.insert(
                    "literal_report_0".to_owned(),
                    JevAnswer::Choice {
                        choice: "reports_addition".to_owned(),
                        confidence: probability,
                        probabilities: BTreeMap::from([
                            ("reports_addition".to_owned(), probability),
                            ("no_addition_report".to_owned(), 1.0 - probability),
                        ]),
                    },
                );
                let result = reduce_assessment(
                    &plan.prepared,
                    &BTreeMap::from([(comparison.id.clone(), response)]),
                    true,
                );
                assert_eq!(
                    result.findings.len(),
                    usize::from(!source_truncated && probability >= 0.90)
                );
                if !result.findings.is_empty() {
                    assert_eq!(result.findings[0].reference.action_id, "native-report");
                    assert!(result.findings[0].decision_record().is_some());
                }
            }
        }
    }
}

#[test]
fn uncertain_coverage_still_blocks_an_otherwise_confident_conflict() {
    let source = input(
        vec![tool(
            "install",
            1,
            "Bash",
            serde_json::json!({"command":"npm install"}),
        )],
        "Do not run npm install.",
    );
    let context = build_jev_context(&source).unwrap();
    let mut plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    let classifications = IgnoredInstructionsCheck
        .classifications(&context)
        .unwrap()
        .into_iter()
        .map(|item| {
            let mut answers = item
                .questions
                .iter()
                .map(|(key, question)| {
                    let selected = match key.as_str() {
                        "condition_evidence" => "selected",
                        "permission" => "independent",
                        "action_family" => "bash",
                        "obligation" => "action",
                        "read_prerequisite" => "not_read_order",
                        "path_change_policy" => "other_path",
                        "read_trigger" => "not_read_rule",
                        _ if key.starts_with("literal_policy_") => "literal_other",
                        _ if key.starts_with("literal_qualification_") => "unqualified",
                        _ => panic!("unexpected classification {key}"),
                    };
                    (key.clone(), answer(question, selected))
                })
                .collect::<BTreeMap<_, _>>();
            let JevAnswer::Choice {
                confidence,
                probabilities,
                ..
            } = answers.get_mut("condition_evidence").unwrap()
            else {
                panic!("choice required")
            };
            *confidence = 0.79;
            *probabilities = BTreeMap::from([
                ("selected".to_owned(), 0.79),
                ("result".to_owned(), 0.21),
                ("undefined".to_owned(), 0.0),
                ("unknown".to_owned(), 0.0),
            ]);
            let result = JevWorkItemResult {
                request_id: "coverage-test".to_owned(),
                work_item_id: item.id.clone(),
                answers,
                evidence: item.window.evidence,
                model: ASSESSMENT_MODEL.to_owned(),
                usage: JevUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            };
            (item.id, result)
        })
        .collect();
    IgnoredInstructionsCheck
        .apply_classifications(&mut plan, &classifications, &context)
        .unwrap();
    let comparison = &plan.prepared.comparisons[0];
    let result = reduce_assessment(
        &plan.prepared,
        &BTreeMap::from([(
            comparison.id.clone(),
            comparison_response(comparison, "conflict", "self_contained"),
        )]),
        true,
    );
    assert!(result.findings.is_empty());
    assert_eq!(result.unassessed_comparisons.len(), 1);
}

#[test]
fn recursive_directory_scope_binds_move_paths_without_resolving_unknown_roots() {
    for (from, to, unknown) in [
        ("src/secure/key.rs", "src/key.rs", false),
        ("src/key.rs", "src/secure/key.rs", false),
        ("/workspace/src/secure/key.rs", "src/key.rs", true),
    ] {
        let source = input(
            vec![tool(
                "move",
                1,
                "apply_patch",
                json!({"patchText":format!("*** Begin Patch\n*** Update File: {from}\n*** Move to: {to}\n*** End Patch")}),
            )],
            "Do not move files in `src/secure/**`.",
        );
        let (_, plan) = classified(
            &source,
            PrerequisiteContextPolicy::CoherentEpisode,
            "action",
            "not_read_order",
            "edit_directory",
        );
        let comparison = &plan.prepared.comparisons[0];
        let obligation = &plan.prepared.observable_obligations[&comparison.id];
        assert_eq!(obligation.edit_scope_unknown, unknown);
        if !unknown {
            assert_eq!(obligation.edit_scope_matches, Some(true));
        }
        let result = reduce_assessment(
            &plan.prepared,
            &BTreeMap::from([(
                comparison.id.clone(),
                comparison_response(comparison, "conflict", "self_contained"),
            )]),
            true,
        );
        assert_eq!(result.findings.is_empty(), unknown);
    }
}

#[test]
fn context_ablation_keeps_every_other_preparation_dimension_fixed() {
    let mut actions = vec![event(
        "earlier",
        1,
        "assistant",
        "main",
        &format!("{}Requested validation.", "Recorded context. ".repeat(80)),
    )];
    actions.extend((2..12).map(|order| {
        event(
            &format!("note-{order}"),
            order,
            "assistant",
            "main",
            "Reviewed a note.",
        )
    }));
    actions.push(event(
        "publish",
        12,
        "assistant",
        "main",
        "Requested publication.",
    ));
    let source = input(actions, "Request validation before publication.");
    let (selected_context, selected) = classified(
        &source,
        PrerequisiteContextPolicy::SelectedEvents,
        "prerequisite",
        "not_read_order",
        "other_path",
    );
    let (episode_context, episode) = classified(
        &source,
        PrerequisiteContextPolicy::CoherentEpisode,
        "prerequisite",
        "not_read_order",
        "other_path",
    );
    assert_ne!(
        selected_context.input_revision,
        episode_context.input_revision
    );
    assert_ne!(
        IgnoredInstructionsCheck.incremental_identity(&selected_context),
        IgnoredInstructionsCheck.incremental_identity(&episode_context)
    );
    assert_eq!(selected.revisions, episode.revisions);
    assert_eq!(selected.capabilities, episode.capabilities);
    assert_eq!(selected.prepared.coverage, episode.prepared.coverage);
    assert_eq!(
        selected.prepared.observable_obligations,
        episode.prepared.observable_obligations
    );
    assert_eq!(
        selected
            .prepared
            .comparisons
            .iter()
            .map(|comparison| (&comparison.id, &comparison.reference))
            .collect::<Vec<_>>(),
        episode
            .prepared
            .comparisons
            .iter()
            .map(|comparison| (&comparison.id, &comparison.reference))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        selected
            .work_items
            .iter()
            .map(|item| (&item.id, &item.questions))
            .collect::<Vec<_>>(),
        episode
            .work_items
            .iter()
            .map(|item| (&item.id, &item.questions))
            .collect::<Vec<_>>()
    );
    let selected_publish = selected
        .prepared
        .comparisons
        .iter()
        .find(|comparison| comparison.action.action_id == "publish")
        .unwrap();
    let episode_publish = episode
        .prepared
        .comparisons
        .iter()
        .find(|comparison| comparison.action.action_id == "publish")
        .unwrap();
    assert!(
        !selected_publish
            .prerequisite_episode
            .as_ref()
            .unwrap()
            .complete_selected_history
    );
    assert!(
        episode_publish
            .prerequisite_episode
            .as_ref()
            .unwrap()
            .complete_selected_history
    );
    assert!(
        selected_publish
            .prerequisite_episode
            .as_ref()
            .unwrap()
            .events
            .len()
            < episode_publish
                .prerequisite_episode
                .as_ref()
                .unwrap()
                .events
                .len()
    );
    let default = crate::checks::ignored_instructions::build_jev_context_with_capabilities(
        &source,
        &SamplingLedger::default(),
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    assert_eq!(default.check_context, episode_context.check_context);
}

#[test]
fn a_small_complete_selected_baseline_uses_the_same_prerequisite_proof() {
    let source = input(
        vec![
            event(
                "earlier",
                1,
                "assistant",
                "main",
                "Requested the first check.",
            ),
            event(
                "publish",
                2,
                "assistant",
                "main",
                "Requested publication without the second check.",
            ),
        ],
        "Request both checks before publication.",
    );
    for policy in [
        PrerequisiteContextPolicy::SelectedEvents,
        PrerequisiteContextPolicy::CoherentEpisode,
    ] {
        let (_, mut plan) = classified(
            &source,
            policy,
            "prerequisite",
            "not_read_order",
            "other_path",
        );
        plan.prepared
            .comparisons
            .retain(|comparison| comparison.action.action_id == "publish");
        let comparison = &plan.prepared.comparisons[0];
        assert!(
            comparison
                .prerequisite_episode
                .as_ref()
                .unwrap()
                .complete_selected_history
        );
        let result = reduce_assessment(
            &plan.prepared,
            &BTreeMap::from([(
                comparison.id.clone(),
                comparison_response(comparison, "conflict", "self_contained"),
            )]),
            true,
        );
        assert_eq!(result.findings.len(), 1);
        assert!(result.findings[0].decision_record().is_some());
    }
}

#[test]
fn path_identity_gates_conflict_and_clean_even_without_a_read_requirement() {
    for path in [
        "/workspace/protected/config.rs",
        "protected\\config.rs",
        "./protected/config.rs",
        "protected/../protected/config.rs",
        "$ROOT/protected/config.rs",
    ] {
        let source = input(
            vec![tool(
                "edit",
                1,
                "Edit",
                json!({"file_path": path, "new_string":"EXCLUDED_EDIT_BODY"}),
            )],
            "Do not change `protected/` files.",
        );
        let (_, plan) = classified(
            &source,
            PrerequisiteContextPolicy::CoherentEpisode,
            "action",
            "not_read_order",
            "edit_directory",
        );
        let comparison = &plan.prepared.comparisons[0];
        assert!(plan.prepared.observable_obligations[&comparison.id].edit_scope_unknown);
        for relationship in ["conflict", "follows"] {
            let result = reduce_assessment(
                &plan.prepared,
                &BTreeMap::from([(
                    comparison.id.clone(),
                    comparison_response(comparison, relationship, "self_contained"),
                )]),
                true,
            );
            assert!(result.findings.is_empty());
            assert_eq!(result.unassessed_comparisons.len(), 1);
            assert!(
                result
                    .coverage
                    .limitations
                    .contains(&"edit_path_identity_unavailable".to_owned())
            );
        }
        assert!(
            !plan.work_items[0]
                .window
                .fields
                .to_string()
                .contains("EXCLUDED_EDIT_BODY")
        );
    }
    let source = input(
        vec![tool(
            "edit",
            1,
            "Edit",
            json!({"file_path":"protected/config.rs"}),
        )],
        "Do not change `protected/` files.",
    );
    let (_, plan) = classified(
        &source,
        PrerequisiteContextPolicy::CoherentEpisode,
        "action",
        "not_read_order",
        "edit_directory",
    );
    assert!(
        !plan
            .prepared
            .observable_obligations
            .values()
            .next()
            .unwrap()
            .edit_scope_unknown
    );
}

#[test]
fn patch_operations_reach_requests_without_edit_bodies_or_execution_claims() {
    let source = input(
        vec![tool(
            "patch",
            1,
            "apply_patch",
            json!({"patchText":"*** Begin Patch\n*** Update File: protected/old.rs\n*** Move to: src/new.rs\n@@\n-EXCLUDED_OLD_BODY\n+EXCLUDED_NEW_BODY\n*** Delete File: protected/obsolete.rs\n*** End Patch"}),
        )],
        "Do not change `protected/` files.",
    );
    let (_, plan) = classified(
        &source,
        PrerequisiteContextPolicy::CoherentEpisode,
        "action",
        "not_read_order",
        "edit_directory",
    );
    let item = &plan.work_items[0];
    let changes = &item.window.fields["requested_path_changes"];
    assert_eq!(
        changes[0],
        json!({"path":"protected/old.rs", "change":"move_from"})
    );
    assert_eq!(changes[1], json!({"path":"src/new.rs", "change":"move_to"}));
    assert_eq!(
        changes[2],
        json!({"path":"protected/obsolete.rs", "change":"delete"})
    );
    assert!(!item.window.fields.to_string().contains("EXCLUDED_OLD_BODY"));
    assert!(!item.window.fields.to_string().contains("EXCLUDED_NEW_BODY"));
    assert_eq!(
        item.window
            .evidence
            .iter()
            .filter(|reference| reference.role == JevEvidenceRole::Candidate)
            .count(),
        1
    );
}

#[test]
fn long_here_document_fragments_keep_their_own_source_bound_header() {
    let command = format!(
        "format-check --stdin <<'END_INPUT'\n{}\nSECRET_MARKER\nEND_INPUT\n",
        "recorded input\n".repeat(500)
    );
    let source = input(
        vec![tool(
            "shell",
            1,
            "Bash",
            json!({"command":command, "output":"EXCLUDED_OUTPUT"}),
        )],
        "Do not pass `SECRET_MARKER` to format-check.",
    );
    let (_, plan) = classified(
        &source,
        PrerequisiteContextPolicy::CoherentEpisode,
        "action",
        "not_read_order",
        "other_path",
    );
    assert!(plan.work_items.len() > 1);
    for item in &plan.work_items {
        let context = &item.window.fields["command_input_context"];
        assert_eq!(context["header"], "format-check --stdin <<'END_INPUT'");
        assert_eq!(context["body_expands"], false);
        assert!(!context.to_string().contains("SECRET_MARKER"));
        assert!(!item.window.fields.to_string().contains("EXCLUDED_OUTPUT"));
        assert!(
            item.window
                .evidence
                .iter()
                .any(|reference| reference.part_id == "command_input_context"
                    && reference.source_id == "shell"
                    && reference.role == JevEvidenceRole::SupportingContext)
        );
        assert_eq!(
            item.window
                .evidence
                .iter()
                .filter(|reference| reference.role == JevEvidenceRole::Candidate)
                .count(),
            1
        );
    }
}

#[test]
fn complete_missing_read_proof_is_distinct_from_unknown_read_success() {
    for earlier_read in [false, true] {
        let mut actions = Vec::new();
        if earlier_read {
            actions.push(tool(
                "read",
                1,
                "Read",
                json!({"file_path":"docs/policy.md"}),
            ));
        }
        actions.push(tool("edit", 2, "Edit", json!({"file_path":"src/main.rs"})));
        let source = input(actions, "Read `docs/policy.md` before editing any file.");
        let (_, mut plan) = classified(
            &source,
            PrerequisiteContextPolicy::CoherentEpisode,
            "prerequisite",
            "read_success",
            "required_path",
        );
        plan.prepared
            .comparisons
            .retain(|comparison| comparison.action.action_id == "edit");
        let comparison = &plan.prepared.comparisons[0];
        let result = reduce_assessment(
            &plan.prepared,
            &BTreeMap::from([(
                comparison.id.clone(),
                comparison_response(comparison, "follows", "evidence_incomplete"),
            )]),
            true,
        );
        assert_eq!(result.findings.len(), usize::from(!earlier_read));
        assert_eq!(
            result.unassessed_comparisons.len(),
            usize::from(earlier_read)
        );
        if !earlier_read {
            assert!(
                result.findings[0]
                    .decision_record()
                    .unwrap()
                    .coverage
                    .read_request_inventory_complete
            );
        }
    }
}

#[test]
fn preparation_preserves_operation_roles_across_reselection_and_serialization() {
    let source = input(
        vec![tool(
            "patch",
            1,
            "apply_patch",
            json!({"patchText":"*** Begin Patch\n*** Update File: protected/source.rs\n*** Move to: src/destination.rs\n*** End Patch"}),
        )],
        "Do not change `protected/` files.",
    );
    let (first, first_plan) = classified(
        &source,
        PrerequisiteContextPolicy::CoherentEpisode,
        "action",
        "not_read_order",
        "edit_directory",
    );
    let mut preselected = source.clone();
    preselected.content = crate::checks::ignored_instructions::select_session_content(
        &source.content,
        INPUT_SELECTION,
    );
    let (_, preselected_plan) = classified(
        &preselected,
        PrerequisiteContextPolicy::CoherentEpisode,
        "action",
        "not_read_order",
        "edit_directory",
    );
    assert_eq!(
        first_plan.work_items[0].window.fields,
        preselected_plan.work_items[0].window.fields
    );
    assert_eq!(
        first_plan.prepared.comparisons[0].reference,
        preselected_plan.prepared.comparisons[0].reference
    );
    let mut selected_source = source.clone();
    selected_source.content.actions =
        serde_json::from_value(first.check_context["episode_actions"].clone()).unwrap();
    let selected_source: AssessmentInput =
        serde_json::from_slice(&serde_json::to_vec(&selected_source).unwrap()).unwrap();
    let (_, selected_plan) = classified(
        &selected_source,
        PrerequisiteContextPolicy::CoherentEpisode,
        "action",
        "not_read_order",
        "edit_directory",
    );
    assert_eq!(
        first_plan.work_items[0].window.fields,
        selected_plan.work_items[0].window.fields
    );
    assert_eq!(
        first_plan.prepared.comparisons[0].reference.action_digest,
        selected_plan.prepared.comparisons[0]
            .reference
            .action_digest
    );
}

#[test]
fn neither_context_policy_claims_complete_history_from_an_unbound_source() {
    let mut earlier = event("earlier", 1, "assistant", "main", "Requested the check.");
    earlier.reference.source_key_digest = "different-source".to_owned();
    let source = input(
        vec![
            earlier,
            event("publish", 2, "assistant", "main", "Requested publication."),
        ],
        "Request a check before publication.",
    );
    for policy in [
        PrerequisiteContextPolicy::SelectedEvents,
        PrerequisiteContextPolicy::CoherentEpisode,
    ] {
        let (_, mut plan) = classified(
            &source,
            policy,
            "prerequisite",
            "not_read_order",
            "other_path",
        );
        plan.prepared
            .comparisons
            .retain(|comparison| comparison.action.action_id == "publish");
        let comparison = &plan.prepared.comparisons[0];
        assert!(
            !comparison
                .prerequisite_episode
                .as_ref()
                .unwrap()
                .complete_selected_history
        );
        let result = reduce_assessment(
            &plan.prepared,
            &BTreeMap::from([(
                comparison.id.clone(),
                comparison_response(comparison, "follows", "self_contained"),
            )]),
            true,
        );
        assert!(result.findings.is_empty());
        assert_eq!(result.unassessed_comparisons.len(), 1);
    }
}

#[test]
fn context_policy_is_explicit_and_invalid_policy_is_not_a_product_fallback() {
    let source = input(
        vec![event(
            "candidate",
            1,
            "assistant",
            "main",
            "Requested publication.",
        )],
        "Request a check before publication.",
    );
    let mut context = build_jev_context_with_context_policy(
        &source,
        &SamplingLedger::default(),
        &ModelCapabilities::jev_default(),
        PrerequisiteContextPolicy::CoherentEpisode,
    )
    .unwrap();
    assert_eq!(
        PrerequisiteContextPolicy::default(),
        PrerequisiteContextPolicy::CoherentEpisode
    );
    context.check_context["prerequisite_context_policy"] = json!("unsupported_policy");
    assert_eq!(
        PrerequisiteContextPolicy::from_context(&context),
        Err(JevError::InvalidCheckContext)
    );
    context
        .check_context
        .as_object_mut()
        .unwrap()
        .remove("prerequisite_context_policy");
    assert_eq!(
        PrerequisiteContextPolicy::from_context(&context).unwrap(),
        PrerequisiteContextPolicy::CoherentEpisode
    );
}

#[test]
fn a_read_from_another_source_cannot_satisfy_or_disprove_the_prerequisite() {
    let mut read = tool("read", 1, "Read", json!({"file_path":"docs/policy.md"}));
    read.reference.source_key_digest = "unbound-source".to_owned();
    let source = input(
        vec![
            read,
            tool("edit", 2, "Edit", json!({"file_path":"src/main.rs"})),
        ],
        "Request a read of `docs/policy.md` before editing.",
    );
    let (_, mut plan) = classified(
        &source,
        PrerequisiteContextPolicy::CoherentEpisode,
        "prerequisite",
        "request_order",
        "required_path",
    );
    plan.prepared
        .comparisons
        .retain(|comparison| comparison.action.action_id == "edit");
    let comparison = &plan.prepared.comparisons[0];
    let obligation = &plan.prepared.observable_obligations[&comparison.id];
    let order = obligation.read_request_order.as_ref().unwrap();
    assert!(!order.paths_known);
    assert!(order.earlier_request_id.is_none());
    assert!(!obligation.read_prerequisite_absent);
    let result = reduce_assessment(
        &plan.prepared,
        &BTreeMap::from([(
            comparison.id.clone(),
            comparison_response(comparison, "follows", "self_contained"),
        )]),
        true,
    );
    assert!(result.findings.is_empty());
    assert_eq!(result.unassessed_comparisons.len(), 1);
}

#[test]
fn source_binding_mismatch_blocks_conflict_and_clean_publication() {
    let source = input(
        vec![event(
            "candidate",
            1,
            "assistant",
            "main",
            "Recorded communication.",
        )],
        "Use the required format.",
    );
    for mismatch in ["digest", "thread", "source", "anchor", "order"] {
        let (_, mut plan) = classified(
            &source,
            PrerequisiteContextPolicy::CoherentEpisode,
            "action",
            "not_read_order",
            "other_path",
        );
        let comparison = &mut plan.prepared.comparisons[0];
        let binding = comparison.source_binding.as_mut().unwrap();
        match mismatch {
            "digest" => binding.content_digest = "different-digest".to_owned(),
            "thread" => binding.source.thread_digest = "different-thread".to_owned(),
            "source" => binding.source.source_key_digest.clear(),
            "anchor" => binding.source.id = "different-anchor".to_owned(),
            "order" => binding.source.turn_index += 1,
            _ => unreachable!(),
        }
        let comparison = &plan.prepared.comparisons[0];
        for relationship in ["conflict", "follows"] {
            let result = reduce_assessment(
                &plan.prepared,
                &BTreeMap::from([(
                    comparison.id.clone(),
                    comparison_response(comparison, relationship, "self_contained"),
                )]),
                true,
            );
            assert!(result.findings.is_empty());
            assert_eq!(result.unassessed_comparisons.len(), 1);
        }
    }
}

#[test]
fn changed_episode_proof_blocks_both_conflict_and_clean_publication() {
    let source = input(
        vec![
            event("check", 1, "assistant", "main", "Requested a check."),
            event("publish", 2, "assistant", "main", "Requested publication."),
        ],
        "Request a check before publication.",
    );
    let (_, mut plan) = classified(
        &source,
        PrerequisiteContextPolicy::CoherentEpisode,
        "prerequisite",
        "not_read_order",
        "other_path",
    );
    plan.prepared
        .comparisons
        .retain(|comparison| comparison.action.action_id == "publish");
    plan.prepared.comparisons[0]
        .prerequisite_episode
        .as_mut()
        .unwrap()
        .events[0]
        .text = "Changed prerequisite evidence.".to_owned();
    let comparison = &plan.prepared.comparisons[0];
    for relationship in ["conflict", "follows"] {
        let result = reduce_assessment(
            &plan.prepared,
            &BTreeMap::from([(
                comparison.id.clone(),
                comparison_response(comparison, relationship, "self_contained"),
            )]),
            true,
        );
        assert!(result.findings.is_empty());
        assert_eq!(
            result.unassessed_comparisons.as_slice(),
            std::slice::from_ref(&comparison.id)
        );
    }
}

#[test]
fn typed_path_ban_uses_requested_changes_and_keeps_unknown_identity_unassessed() {
    for (patch, policy, expected_finding, expected_unassessed) in [
        (
            "*** Delete File: protected/old.rs",
            "path_change_ban",
            true,
            false,
        ),
        (
            "*** Update File: protected/old.rs\n*** Move to: src/new.rs",
            "path_change_ban",
            true,
            false,
        ),
        (
            "*** Update File: src/old.rs\n*** Move to: protected/new.rs",
            "path_change_ban",
            true,
            false,
        ),
        (
            "*** Delete File: src/old.rs",
            "path_change_ban",
            false,
            false,
        ),
        (
            "*** Delete File: /workspace/protected/old.rs",
            "path_change_ban",
            false,
            true,
        ),
        (
            "*** Delete File: protected/old.rs",
            "other_path",
            false,
            false,
        ),
    ] {
        let source = input(
            vec![tool(
                "patch",
                1,
                "apply_patch",
                json!({
                    "patchText": format!("*** Begin Patch\n{patch}\n*** End Patch")
                }),
            )],
            "Do not change `protected/` files.",
        );
        let (_, plan) = classified_with_path_policy(
            &source,
            PrerequisiteContextPolicy::CoherentEpisode,
            "action",
            "not_read_order",
            "edit_directory",
            policy,
        );
        let comparison = &plan.prepared.comparisons[0];
        let result = reduce_assessment(
            &plan.prepared,
            &BTreeMap::from([(
                comparison.id.clone(),
                comparison_response(comparison, "follows", "self_contained"),
            )]),
            true,
        );
        assert_eq!(
            result.findings.len(),
            usize::from(expected_finding),
            "{patch}, {policy}"
        );
        assert_eq!(
            result.unassessed_comparisons.len(),
            usize::from(expected_unassessed)
        );
        if expected_finding {
            assert_eq!(result.findings[0].reference.action_id, "patch");
            assert!(result.findings[0].decision_record().is_some());
        }
    }
}
#[test]
fn request_prerequisite_with_two_literal_paths_uses_source_order_not_nearby_text() {
    for (read_before, read_after, expected_finding) in [
        (true, false, false),
        (false, true, true),
        (false, false, true),
    ] {
        let mut actions = vec![event(
            "note",
            1,
            "assistant",
            "main",
            "A separate task uses another file.",
        )];
        if read_before {
            actions.push(tool(
                "read",
                2,
                "Read",
                json!({"file_path":"docs/transfer-policy.md"}),
            ));
        }
        actions.extend((3..18).map(|order| {
            event(
                &format!("note-{order}"),
                order,
                "assistant",
                "main",
                "Recorded unrelated note.",
            )
        }));
        actions.push(tool(
            "edit",
            20,
            "Edit",
            json!({"file_path":"src/transfer.rs"}),
        ));
        if read_after {
            actions.push(tool(
                "later-read",
                21,
                "Read",
                json!({"file_path":"docs/transfer-policy.md"}),
            ));
        }
        let source = input(
            actions,
            "Request a read of `docs/transfer-policy.md` before editing `src/transfer.rs`.",
        );
        let (_, mut plan) = classified(
            &source,
            PrerequisiteContextPolicy::CoherentEpisode,
            "prerequisite",
            "request_order",
            "other_path",
        );
        // These bindings represent accepted independent rule classifications.
        let context = build_jev_context_with_context_policy(
            &source,
            &SamplingLedger::default(),
            &ModelCapabilities::jev_default(),
            PrerequisiteContextPolicy::CoherentEpisode,
        )
        .unwrap();
        let classification_items = matching::rule_classifications(&plan.prepared).unwrap();
        let results = classification_items
            .into_iter()
            .map(|item| {
                let answers = item
                    .questions
                    .iter()
                    .map(|(key, question)| {
                        let selected = match key.as_str() {
                            "condition_evidence" => "selected",
                            "permission" => "independent",
                            "action_family" => "any",
                            "obligation" => "prerequisite",
                            "read_prerequisite" => "request_order",
                            "read_trigger" => "edit_request",
                            "read_path_0" => "required_path",
                            "read_path_1" => "edit_file",
                            "path_change_policy" => "other_path",
                            _ if key.starts_with("literal_policy_") => "literal_other",
                            _ if key.starts_with("literal_qualification_") => "qualified",
                            _ => panic!("unexpected rule classification {key}"),
                        };
                        (key.clone(), answer(question, selected))
                    })
                    .collect();
                let result = JevWorkItemResult {
                    request_id: "independent-rule-shape".to_owned(),
                    work_item_id: item.id.clone(),
                    answers,
                    evidence: item.window.evidence,
                    model: ASSESSMENT_MODEL.to_owned(),
                    usage: JevUsage {
                        input_tokens: 0,
                        output_tokens: 0,
                    },
                };
                (item.id, result)
            })
            .collect();
        matching::apply_rule_matching(&mut plan, &results, &context).unwrap();
        plan.prepared
            .comparisons
            .retain(|comparison| comparison.reference.action_id == "edit");
        let comparison = &plan.prepared.comparisons[0];
        let order = plan.prepared.observable_obligations[&comparison.id]
            .read_request_order
            .as_ref()
            .unwrap();
        assert_eq!(order.earlier_request_id.is_some(), read_before);
        assert_eq!(order.later_request_id.is_some(), read_after);
        assert!(order.paths_known && order.history_complete);
        let result = reduce_assessment(
            &plan.prepared,
            &BTreeMap::from([(
                comparison.id.clone(),
                comparison_response(comparison, "insufficient_evidence", "evidence_incomplete"),
            )]),
            true,
        );
        assert_eq!(result.findings.len(), usize::from(expected_finding));
        assert!(result.unassessed_comparisons.is_empty());
        if expected_finding {
            assert_eq!(result.findings[0].reference.action_id, "edit");
            assert!(result.findings[0].decision_record().is_some());
        }
    }
}
#[test]
fn opaque_earlier_requests_cannot_prove_read_absence() {
    let source = input(
        vec![
            tool(
                "shell-read",
                1,
                "Bash",
                json!({"command":"cat docs/transfer-policy.md"}),
            ),
            tool("edit", 2, "Edit", json!({"file_path":"src/transfer.rs"})),
        ],
        "Request a read of `docs/transfer-policy.md` before editing.",
    );
    let (_, mut plan) = classified(
        &source,
        PrerequisiteContextPolicy::CoherentEpisode,
        "prerequisite",
        "request_order",
        "required_path",
    );
    plan.prepared
        .comparisons
        .retain(|comparison| comparison.reference.action_id == "edit");
    let comparison = &plan.prepared.comparisons[0];
    let order = plan.prepared.observable_obligations[&comparison.id]
        .read_request_order
        .as_ref()
        .unwrap();
    assert!(!order.paths_known);
    for relationship in ["conflict", "follows"] {
        let result = reduce_assessment(
            &plan.prepared,
            &BTreeMap::from([(
                comparison.id.clone(),
                comparison_response(comparison, relationship, "self_contained"),
            )]),
            true,
        );
        assert!(result.findings.is_empty());
        assert_eq!(result.unassessed_comparisons.len(), 1);
    }
}
