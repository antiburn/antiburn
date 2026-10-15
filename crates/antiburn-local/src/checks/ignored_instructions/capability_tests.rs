use super::tests::{event, input};
use super::*;
use crate::analysis::jev::capabilities::{
    CapabilityLimit, CapabilitySource, ModelCapabilities, OLLAMA_TEV1_ESTIMATOR, TokenizerIdentity,
};
use crate::analysis::jev::{
    pack_work_items_with_capabilities, validate_jev_request_with_capabilities,
};
use crate::checks::ignored_instructions::planning::{
    PreparedAssessmentInput, SamplingLedger, SavedComparison, build_jev_context_with_capabilities,
};

fn limits() -> ModelCapabilities {
    let mut limits = ModelCapabilities::jev_default();
    limits.rendering_reserve_tokens = 304;
    limits.tokenizer = Some(TokenizerIdentity::ConservativeEstimator(
        OLLAMA_TEV1_ESTIMATOR.to_owned(),
    ));
    limits.runtime_context_tokens = CapabilityLimit::known(2048, CapabilitySource::Manual);
    limits
}

fn command(id: &str, order: i64, text: &str) -> ContentAction {
    let mut action = event(id, order, "assistant", "main", text);
    action.kind = "tool_input".to_owned();
    action.tool_name = Some("Bash".to_owned());
    action.tool_call_id = Some(id.to_owned());
    action
}

fn prepare(source: &AssessmentInput, limits: &ModelCapabilities) -> JevCheckPlan<AssessmentPlan> {
    let context =
        build_jev_context_with_capabilities(source, &SamplingLedger::default(), limits).unwrap();
    IgnoredInstructionsCheck
        .prepare_with_capabilities(&context, limits)
        .unwrap()
}

fn assert_fit(plan: &JevCheckPlan<AssessmentPlan>) -> usize {
    let packed = pack_work_items_with_capabilities(&plan.work_items, &plan.capabilities);
    assert!(packed.skipped_item_ids.is_empty());
    assert_eq!(packed.batches.len(), 1);
    let batch = &packed.batches[0];
    assert_eq!(batch.work_item_ids.len(), plan.work_items.len());
    assert!(batch.serialized_bytes <= 6144);
    assert!(
        plan.work_items
            .iter()
            .all(|item| serde_json::to_vec(&item.window.fields).unwrap().len() <= 1536)
    );
    validate_jev_request_with_capabilities(&batch.request, &plan.capabilities).unwrap();
    assert!(
        crate::analysis::jev::estimate_jev_rendered_question_tokens(
            &batch.request,
            &plan.capabilities
        )
        .unwrap()
            <= 1536
    );
    batch.serialized_bytes
}

fn positive_results(plan: &JevCheckPlan<AssessmentPlan>) -> Vec<JevWorkItemResult> {
    plan.work_items
        .iter()
        .map(|item| JevWorkItemResult {
            request_id: "bound-positive".to_owned(),
            work_item_id: item.id.clone(),
            answers: item
                .questions
                .keys()
                .map(|key| {
                    (
                        key.clone(),
                        JevAnswer::Choice {
                            choice: "conflict".to_owned(),
                            probabilities: BTreeMap::from([
                                ("conflict".to_owned(), 1.0),
                                ("no_conflict".to_owned(), 0.0),
                                ("uncertain".to_owned(), 0.0),
                            ]),
                            confidence: 1.0,
                        },
                    )
                })
                .collect(),
            evidence: item.window.evidence.clone(),
            model: plan.capabilities.model.clone(),
            usage: crate::analysis::jev::JevUsage {
                input_tokens: 17,
                output_tokens: 0,
            },
        })
        .collect()
}

#[test]
fn runtime_reduction_counts_rendered_questions_and_preserves_mandatory_authority() {
    let user = crate::checks::ignored_instructions::selected_context::tests::human();
    let source = input(
        vec![
            user,
            event(
                "optional",
                2,
                "assistant",
                "main",
                "Inspect the ordinary release workflow.",
            ),
            command("publish", 3, "npm publish --access public"),
        ],
        "Do not request `npm publish`, except when the user requests a release.",
    );
    let capacity = limits();
    let normal = prepare(&source, &capacity);
    assert_eq!(normal.work_items.len(), 1);
    let normal_bytes = assert_fit(&normal);
    let packed = pack_work_items_with_capabilities(&normal.work_items, &capacity);
    let tokens = crate::analysis::jev::estimate_jev_rendered_question_tokens(
        &packed.batches[0].request,
        &capacity,
    )
    .unwrap();
    let mut smaller = capacity.clone();
    smaller.runtime_context_tokens = CapabilityLimit::known(
        tokens + capacity.rendering_reserve_tokens - 1,
        CapabilitySource::RuntimeMetadata,
    );
    assert!(validate_jev_request_with_capabilities(&packed.batches[0].request, &smaller).is_err());
    let reduced = prepare(&source, &smaller);
    assert_eq!(reduced.work_items.len(), 1);
    assert!(assert_fit(&reduced) < normal_bytes);
    let fields = &reduced.work_items[0].window.fields;
    assert_eq!(
        fields["instruction_targets"][0]["instruction"]["text"],
        source.content.instructions[0].sections[0].text
    );
    let earlier = fields["instruction_targets"][0]["earlier_counterevidence"]
        .as_array()
        .unwrap();
    assert_eq!(earlier.len(), 1);
    assert_eq!(earlier[0]["text"], source.content.actions[0].text);
    assert_eq!(earlier[0]["native_context"]["evidence_kind"], "human_text");
    assert!(!fields.to_string().contains("ordinary release workflow"));
    let result = IgnoredInstructionsCheck
        .reduce(&reduced, &positive_results(&reduced), true)
        .unwrap();
    assert_eq!(result.findings.len(), 1);
    assert!(result.findings[0].decision_record().is_some());
}

#[test]
fn positive_probability_cannot_publish_changed_source_or_episode_bindings() {
    let source = input(
        vec![command("push", 2, "git push --force")],
        "Do not run `git push --force`.",
    );
    let valid = prepare(&source, &limits());
    assert_eq!(valid.work_items.len(), 1);
    let results = positive_results(&valid);
    assert_eq!(
        IgnoredInstructionsCheck
            .reduce(&valid, &results, true)
            .unwrap()
            .findings
            .len(),
        1
    );
    for mutation in 0..6 {
        let mut invalid = valid.clone();
        let comparison = &mut invalid.prepared.comparisons[0];
        match mutation {
            0 => comparison.source_binding = None,
            1 => comparison
                .source_binding
                .as_mut()
                .unwrap()
                .content_digest
                .push('x'),
            2 => comparison.action.text.push_str(" origin other"),
            3 => comparison
                .prerequisite_episode
                .as_mut()
                .unwrap()
                .revision
                .push('x'),
            4 => comparison.rule_text_end -= 1,
            _ => comparison.reference.scope = InstructionScope::Conditional,
        }
        let rejected = IgnoredInstructionsCheck
            .reduce(&invalid, &results, true)
            .unwrap();
        assert!(rejected.findings.is_empty(), "mutation={mutation}");
        assert_eq!(
            rejected.unassessed_comparisons.len(),
            1,
            "mutation={mutation}"
        );
        assert_eq!(rejected.input_tokens, 17);
    }
}

#[test]
fn persisted_findings_require_a_direct_decision_and_valid_probability() {
    let source = input(
        vec![command("push", 1, "git push --force")],
        "Do not run `git push --force`.",
    );
    let plan = prepare(&source, &limits());
    let result = IgnoredInstructionsCheck
        .reduce(&plan, &positive_results(&plan), true)
        .unwrap();
    assert_eq!(result.findings.len(), 1);
    for mutation in 0..5 {
        let mut invalid = result.findings[0].clone();
        match mutation {
            0 => invalid.decision = None,
            1 => invalid.decision.as_mut().unwrap().explanation_basis = None,
            2 => invalid.decision.as_mut().unwrap().action_is_request = false,
            3 => invalid.composite_probability = 1.1,
            _ => invalid.composite_probability = 0.94,
        }
        assert!(invalid.decision_record().is_none(), "mutation={mutation}");
    }
}

#[test]
fn oversized_authority_and_late_rule_exceptions_abstain_before_dispatch() {
    let mut user = crate::checks::ignored_instructions::selected_context::tests::human();
    user.text = format!(
        "{}\nExcept for a recovery push, do not push the branch.",
        "Complete release context. ".repeat(12)
    );
    user.metadata.bindings[0].end = user.text.len();
    let source = input(
        vec![user, command("push", 2, "git push --force")],
        "Do not run `git push --force`, except when the user requests recovery.",
    );
    let plan = prepare(&source, &limits());
    assert_eq!(plan.prepared.comparisons.len(), 1);
    assert!(plan.work_items.is_empty());
    let result = IgnoredInstructionsCheck.reduce(&plan, &[], true).unwrap();
    assert_eq!(result.unassessed_comparisons.len(), 1);
    assert_eq!(result.request_count, 0);
    let rule = format!(
        "Do not run `git push --force`.\n{}\nException: a user request permits recovery.",
        "The rule applies to ordinary release work. ".repeat(12)
    );
    assert!(rule.len() > 512);
    let oversized = prepare(
        &input(vec![command("push", 1, "git push --force")], &rule),
        &limits(),
    );
    assert!(oversized.prepared.comparisons.is_empty());
    assert!(oversized.work_items.is_empty());
    assert!(!oversized.prepared.coverage.skipped_rules.is_empty());
}

#[test]
fn incompatible_preparation_and_reducer_revisions_are_rejected() {
    let source = input(
        vec![command("push", 1, "git push --force")],
        "Do not run `git push --force`.",
    );
    let capacity = limits();
    let mut context =
        build_jev_context_with_capabilities(&source, &SamplingLedger::default(), &capacity)
            .unwrap();
    context.check_context["assessment_plan"]["question_revision"] =
        json!(ASSESSMENT_QUESTION_REVISION - 1);
    assert!(matches!(
        IgnoredInstructionsCheck.prepare_with_capabilities(&context, &capacity),
        Err(JevError::InvalidCheckContext)
    ));
    let mut plan = prepare(&source, &capacity);
    let results = positive_results(&plan);
    plan.prepared.reducer_revision -= 1;
    assert!(matches!(
        IgnoredInstructionsCheck.reduce(&plan, &results, true),
        Err(JevError::InvalidCheckPlan)
    ));
}

#[test]
fn normal_capacity_keeps_two_newest_direct_actions_in_one_request() {
    let source = input(
        (0..50)
            .map(|index| {
                command(
                    &format!("push-{index}"),
                    index,
                    "git push --force origin main",
                )
            })
            .collect(),
        "Do not run `git push --force`.",
    );
    let mut capacity = limits();
    capacity.runtime_context_tokens.value = Some(8192);
    let plan = prepare(&source, &capacity);
    assert_eq!(plan.work_items.len(), 2);
    assert_eq!(plan.prepared.comparisons[0].action.action_id, "push-49");
    assert_eq!(plan.prepared.comparisons[1].action.action_id, "push-48");
    assert!(plan.prepared.next_comparison_cursor.is_none());
    assert_fit(&plan);
    assert!(plan.prepared.coverage.unselected_pairs > 0);
}

#[test]
fn compatible_terminal_reuse_does_not_admit_policy_omitted_actions() {
    use crate::checks::ignored_instructions::planning::{
        take_preparation_counts, take_selection_counts,
    };
    take_preparation_counts();
    take_selection_counts();
    let source = input(
        (0..50)
            .map(|index| {
                command(
                    &format!("push-{index}"),
                    index,
                    "git push --force origin main",
                )
            })
            .collect(),
        "Do not run `git push --force`.",
    );
    let capacity = limits();
    let mut prepared = PreparedAssessmentInput::new(&source);
    let context = prepared
        .build_context(&SamplingLedger::default(), &capacity)
        .unwrap();
    let first = IgnoredInstructionsCheck
        .prepare_with_capabilities(&context, &capacity)
        .unwrap();
    assert_eq!(first.prepared.comparisons.len(), 2);
    assert_eq!(take_preparation_counts().2, 2);
    assert!(take_selection_counts().1 <= 2);
    let saved = first
        .prepared
        .comparisons
        .iter()
        .map(SavedComparison::from)
        .collect::<Vec<_>>();
    let ledger = SamplingLedger {
        comparison_ids: saved
            .iter()
            .map(|comparison| comparison.id.clone())
            .collect(),
        comparisons: saved
            .iter()
            .map(|comparison| (comparison.id.clone(), comparison.clone()))
            .collect(),
        ..Default::default()
    };
    let resumed = prepared.build_context(&ledger, &capacity).unwrap();
    assert!(
        IgnoredInstructionsCheck
            .prepare_with_capabilities(&resumed, &capacity)
            .unwrap()
            .work_items
            .is_empty()
    );
    let bound = prepared.dependency_comparisons(&saved, &capacity);
    assert_eq!(bound.comparisons.len(), 2);
    let mut corrupt = saved;
    corrupt[0].coordinate.as_mut().unwrap().action_range.1 += 1;
    assert_eq!(
        prepared
            .dependency_comparisons(&corrupt, &capacity)
            .comparisons
            .len(),
        1
    );
}

#[test]
fn lower_runtime_and_question_limits_drop_the_last_candidate_without_splitting() {
    let source = input(
        vec![
            command("older", 1, "git push --force origin main"),
            command("newer", 2, "git push --force origin main"),
        ],
        "Do not run `git push --force`.",
    );
    let mut capacity = limits();
    capacity.questions_per_request.value = Some(1);
    let plan = prepare(&source, &capacity);
    assert_eq!(plan.work_items.len(), 1);
    assert!(
        plan.work_items[0]
            .window
            .fields
            .to_string()
            .contains("git push")
    );
    assert_fit(&plan);
    assert!(
        plan.prepared
            .coverage
            .limitations
            .contains(&"evidence_fit_omitted".to_owned())
    );
}

#[test]
fn constrained_singleton_reduces_optional_history_and_keeps_complete_rule() {
    let rule =
        "Do not run `git push --force`, except when the user explicitly requests a recovery push.";
    let source = input(
        vec![
            crate::checks::ignored_instructions::selected_context::tests::human(),
            event(
                "explanation",
                2,
                "assistant",
                "main",
                "Investigated ordinary supporting changes.",
            ),
            command("push", 3, "git push --force origin main"),
        ],
        rule,
    );
    let normal = prepare(&source, &limits());
    assert_eq!(normal.work_items.len(), 1);
    let normal_bytes = assert_fit(&normal);
    let mut constrained = limits();
    constrained.request_body_bytes.value = Some(normal_bytes as u64 - 1);
    let reduced = prepare(&source, &constrained);
    assert_eq!(reduced.work_items.len(), 1);
    assert!(assert_fit(&reduced) < normal_bytes);
    assert!(
        reduced.work_items[0]
            .window
            .fields
            .to_string()
            .contains("except when")
    );
    assert!(
        reduced.work_items[0]
            .window
            .fields
            .to_string()
            .contains("git push --force origin main")
    );
}

#[test]
fn minimum_evidence_below_capacity_abstains_without_requests() {
    let source = input(
        vec![command("push", 1, "git push --force origin main")],
        "Do not run `git push --force`.",
    );
    for tokens in [1, 64, 128] {
        let mut capacity = limits();
        capacity.runtime_context_tokens.value = Some(tokens);
        let plan = prepare(&source, &capacity);
        assert!(plan.work_items.is_empty(), "{tokens}");
        assert!(
            pack_work_items_with_capabilities(&plan.work_items, &capacity)
                .batches
                .is_empty()
        );
        let result = IgnoredInstructionsCheck.reduce(&plan, &[], true).unwrap();
        assert!(!result.unassessed_comparisons.is_empty());
    }
}

#[test]
fn complete_rule_and_action_caps_do_not_grow_with_model_capacity() {
    for tokens in [2048, 65_536, u64::MAX] {
        let mut capacity = limits();
        capacity.runtime_context_tokens.value = Some(tokens);
        for source in [
            input(
                vec![command(
                    "push",
                    1,
                    &format!("git push --force {}", "界é🦀 ".repeat(100)),
                )],
                "Do not run `git push --force`.",
            ),
            input(
                vec![command("push", 1, "git push --force")],
                &format!(
                    "Do not run `git push --force`. {} Except when recovery is explicitly approved.",
                    "Keep this prerequisite complete. ".repeat(30)
                ),
            ),
        ] {
            let plan = prepare(&source, &capacity);
            assert!(plan.prepared.comparisons.is_empty());
            assert!(plan.work_items.is_empty());
            assert!(!plan.prepared.coverage.limitations.is_empty());
        }
    }
}

#[test]
fn unicode_and_json_escaping_use_final_request_bytes() {
    let source = input(
        vec![command("push", 1, "git push --force -m \"界é🦀\\quoted\"")],
        "Do not run `git push --force`.",
    );
    let plan = prepare(&source, &limits());
    assert_eq!(plan.work_items.len(), 1);
    let bytes = assert_fit(&plan);
    let mut capacity = limits();
    capacity.request_body_bytes.value = Some(bytes as u64 - 1);
    let rejected = prepare(&source, &capacity);
    assert!(rejected.work_items.is_empty());
}

#[test]
fn state_total_criteria_and_rendering_reserve_limits_are_independent() {
    let source = input(
        vec![command("push", 1, "git push --force")],
        "Do not run `git push --force`.",
    );
    for field in 0..5 {
        let mut capacity = limits();
        match field {
            0 => capacity.state_and_longest_question_bytes.value = Some(64),
            1 => capacity.total_input_tokens.value = Some(64),
            2 => capacity.state_and_longest_question_tokens.value = Some(64),
            3 => capacity.criteria_per_question.value = Some(2),
            _ => capacity.rendering_reserve_tokens = 2048,
        }
        assert!(prepare(&source, &capacity).work_items.is_empty(), "{field}");
    }
}

#[test]
fn saved_targets_bind_exact_sources_after_capability_changes() {
    let source = input(
        vec![command("push", 1, "git push --force")],
        "Do not run `git push --force`.",
    );
    let plan = prepare(&source, &limits());
    assert_eq!(plan.prepared.comparisons.len(), 1);
    let saved = plan
        .prepared
        .comparisons
        .iter()
        .map(SavedComparison::from)
        .collect::<Vec<_>>();
    let mut smaller = limits();
    smaller.runtime_context_tokens.value = Some(64);
    let restored = PreparedAssessmentInput::new(&source).dependency_comparisons(&saved, &smaller);
    assert_eq!(restored.comparisons[0].id, plan.prepared.comparisons[0].id);
    let mut changed = source;
    changed.content.actions[0].text.push_str(" origin other");
    changed.content.actions[0].normalized_fields =
        Some(crate::analysis::jev_evidence::normalize_tool_input(
            "Bash",
            &changed.content.actions[0].text,
        ));
    assert!(
        PreparedAssessmentInput::new(&changed)
            .dependency_comparisons(&saved, &smaller)
            .comparisons
            .is_empty()
    );
}

#[test]
fn prose_and_quoted_commands_do_not_create_direct_candidates() {
    for action in [
        event("prose", 1, "assistant", "main", "I ran git push --force."),
        command("quote", 1, "printf '%s' 'git push --force'"),
    ] {
        let plan = prepare(
            &input(vec![action], "Do not run `git push --force`."),
            &limits(),
        );
        assert!(plan.work_items.is_empty());
        assert!(plan.prepared.comparisons.is_empty());
    }
}

#[tokio::test]
async fn positive_answers_recover_without_duplicate_dispatch_and_partial_answers_abstain() {
    use crate::analysis::jev::{
        JevResponse, JevRunProgress, JevUsage, run_jev_check_with_capabilities,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    let source = input(
        vec![command("push", 1, "git push --force")],
        "Do not run `git push --force`.",
    );
    let capacity = limits();
    let context =
        build_jev_context_with_capabilities(&source, &SamplingLedger::default(), &capacity)
            .unwrap();
    let calls = AtomicUsize::new(0);
    let first = run_jev_check_with_capabilities(
        &IgnoredInstructionsCheck,
        &context,
        JevRunProgress::default(),
        capacity.clone(),
        |batch| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move {
                Ok(JevResponse {
                    model: batch.request.model.clone(),
                    answers: batch
                        .request
                        .questions
                        .keys()
                        .map(|key| {
                            (
                                key.clone(),
                                JevAnswer::Choice {
                                    choice: "conflict".to_owned(),
                                    probabilities: BTreeMap::from([
                                        ("conflict".to_owned(), 1.0),
                                        ("no_conflict".to_owned(), 0.0),
                                        ("uncertain".to_owned(), 0.0),
                                    ]),
                                    confidence: 1.0,
                                },
                            )
                        })
                        .collect(),
                    usage: JevUsage {
                        input_tokens: 10,
                        output_tokens: 2,
                    },
                })
            }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(first.result.findings.len(), 1);
    assert!(first.result.findings[0].decision_record().is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let restored = run_jev_check_with_capabilities(
        &IgnoredInstructionsCheck,
        &context,
        first.progress.clone(),
        capacity.clone(),
        |_| async { panic!("Accepted answer must recover without dispatch") },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(restored.result.findings, first.result.findings);
    let plan = prepare(&source, &capacity);
    for mutation in 0..2 {
        let mut invalid = first.progress.results.values().cloned().collect::<Vec<_>>();
        if mutation == 0 {
            invalid[0].model = "different-model".to_owned();
        } else {
            invalid[0].evidence.clear();
        }
        let rejected = IgnoredInstructionsCheck
            .reduce(&plan, &invalid, true)
            .unwrap();
        assert!(rejected.findings.is_empty());
        assert_eq!(rejected.unassessed_comparisons.len(), 1);
    }
    let mut partial = first.progress.results.values().cloned().collect::<Vec<_>>();
    partial[0].answers.clear();
    let result = IgnoredInstructionsCheck
        .reduce(&plan, &partial, true)
        .unwrap();
    assert!(result.findings.is_empty());
    assert_eq!(result.unassessed_comparisons.len(), 1);
}

#[tokio::test]
async fn provider_context_rejection_does_not_split_the_snapshot() {
    use crate::analysis::jev::{JevRunProgress, run_jev_check_with_capabilities};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let source = input(
        vec![
            command("older", 1, "git push --force"),
            command("newer", 2, "git push --force"),
        ],
        "Do not run `git push --force`.",
    );
    let capacity = limits();
    let context =
        build_jev_context_with_capabilities(&source, &SamplingLedger::default(), &capacity)
            .unwrap();
    let calls = AtomicUsize::new(0);
    let result = run_jev_check_with_capabilities(
        &IgnoredInstructionsCheck,
        &context,
        JevRunProgress::default(),
        capacity.clone(),
        |batch| {
            assert_eq!(batch.work_item_ids.len(), 2);
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(JevError::ContextRejected) }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!result.complete);
    assert!(result.result.findings.is_empty());
    assert_eq!(result.result.unassessed_comparisons.len(), 2);
}

#[test]
fn probability_one_requires_native_user_authority_for_approval_rules() {
    let native = crate::checks::ignored_instructions::selected_context::tests::human();
    for origin in 0..3 {
        let mut user = native.clone();
        if origin == 1 {
            user.metadata.user_text_history = None;
        } else if origin == 2 {
            user.kind = "other_tool_output".to_owned();
            user.authority = "tool".to_owned();
        }
        let source = input(
            vec![user, command("push", 2, "git push --force")],
            "Do not run `git push --force` without user approval.",
        );
        let plan = prepare(&source, &limits());
        assert_eq!(plan.work_items.len(), usize::from(origin == 0));
        let results = plan
            .work_items
            .iter()
            .map(|item| JevWorkItemResult {
                request_id: "positive".to_owned(),
                work_item_id: item.id.clone(),
                answers: item
                    .questions
                    .keys()
                    .map(|key| {
                        (
                            key.clone(),
                            JevAnswer::Choice {
                                choice: "conflict".to_owned(),
                                probabilities: BTreeMap::from([
                                    ("conflict".to_owned(), 1.0),
                                    ("no_conflict".to_owned(), 0.0),
                                    ("uncertain".to_owned(), 0.0),
                                ]),
                                confidence: 1.0,
                            },
                        )
                    })
                    .collect(),
                evidence: item.window.evidence.clone(),
                model: plan.capabilities.model.clone(),
                usage: crate::analysis::jev::JevUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            })
            .collect::<Vec<_>>();
        let result = IgnoredInstructionsCheck
            .reduce(&plan, &results, true)
            .unwrap();
        assert_eq!(result.findings.len(), usize::from(origin == 0), "{origin}");
        assert_eq!(
            result.unassessed_comparisons.len(),
            usize::from(origin != 0),
            "{origin}"
        );
    }
}

#[test]
fn required_markdown_scopes_bind_actual_input_slices_and_do_not_clip_exceptions() {
    for rule in [
        "# Push rules\n- Do not run `git push --force`.\n- Exception: the user can request a recovery push.",
        "# Shared branch\n- Do not run `git push --force`.\n  - Except when the user requests recovery.",
        "Push rules\r\n----------\r\nDo not run `git push --force`, except for requested recovery.\r\n",
        "---\nscope: shared-branch\n---\n# Push rules\nDo not run `git push --force`, except for requested recovery.",
        "Only on the shared branch.\n# Push rules\nDo not run `git push --force`, except for requested recovery.",
    ] {
        let source = input(
            vec![
                crate::checks::ignored_instructions::selected_context::tests::human(),
                command("push", 2, "git push --force"),
            ],
            rule,
        );
        let plan = prepare(&source, &limits());
        assert!(!plan.work_items.is_empty(), "{rule}");
        for comparison in &plan.prepared.comparisons {
            for range in &comparison.instruction_context {
                assert_eq!(
                    rule.get(range.start_byte..range.end_byte),
                    Some(range.text.as_str())
                );
                assert!(range.enclosing_section_complete && !range.context_clipped);
            }
        }
        let request = plan.work_items[0].window.fields.to_string();
        assert!(request.contains("recover"), "{request}");
        if rule.starts_with("---") {
            assert!(request.contains("scope: shared-branch"));
        }
        if rule.starts_with("Only") {
            assert!(request.contains("Only on the shared branch"));
        }
        assert_fit(&plan);
    }
}

#[test]
fn quoted_and_fenced_rules_and_unknown_secret_values_abstain() {
    for rule in [
        "> # Example\n> Do not run `git push --force`.",
        "```markdown\nDo not run `git push --force`.\n~~~\n# Still inside the unmatched code fence\n",
    ] {
        let plan = prepare(
            &input(vec![command("push", 1, "git push --force")], rule),
            &limits(),
        );
        assert!(plan.prepared.comparisons.is_empty());
        assert!(plan.work_items.is_empty());
    }
    let source = input(
        vec![command("upload", 1, "upload \"$TOKEN\"")],
        "Do not pass a secret to an upload command.",
    );
    assert!(prepare(&source, &limits()).work_items.is_empty());
}

#[test]
fn nested_scope_requires_a_known_matching_action_path() {
    for (path, admitted) in [
        ("src/service.rs", true),
        ("other/service.rs", false),
        ("src/../other/service.rs", false),
    ] {
        let mut edit = event(
            "edit",
            1,
            "assistant",
            "main",
            &format!(r#"{{"file_path":"{path}"}}"#),
        );
        edit.kind = "tool_input".to_owned();
        edit.tool_name = Some("Edit".to_owned());
        edit.tool_call_id = Some("edit".to_owned());
        let rule = format!("Do not edit `{path}`.");
        let mut source = input(vec![edit], &rule);
        source.content.instructions[0] = crate::checks::ignored_instructions::snapshot_from_text(
            "project:src/AGENTS.md",
            rule,
            InstructionProvenance::CurrentFileComparison,
            InstructionScope::Nested,
        )
        .unwrap();
        let plan = prepare(&source, &limits());
        assert_eq!(!plan.work_items.is_empty(), admitted, "{path}");
        if admitted {
            assert_eq!(
                plan.work_items[0].window.fields["instruction_targets"][0]["instruction"]["scope_directory"],
                "src"
            );
            assert_fit(&plan);
        }
    }
}
