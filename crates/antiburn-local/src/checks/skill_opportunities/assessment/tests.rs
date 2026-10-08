use super::*;
use crate::analysis::jev::{JevRunProgress, run_jev_check, unpack_jev_response};
use crate::analysis::jev_evidence::prepare_session_content;
use crate::analysis::session_scope::{
    SessionScopeBoundary, SessionScopeBranch, SessionScopeBuilder,
};
use crate::analysis::{
    ContentKind, ContentPart, ContentQueryCoverage, PublishedContent, PublishedContentPart,
    SourceFormat,
};
use crate::checks::sampling::SamplingLimits;
use crate::checks::skill_opportunities::{
    SKILL_USE_SELECTION, SkillDefinition, SkillScope, SkillUseBoundary,
};
use crate::model::AgentKind;

fn scope() -> SkillScope {
    SkillScope {
        agent: AgentKind::Claude,
        project_identity: Some("project".into()),
        environment_identity: "native".into(),
    }
}

#[test]
fn short_approval_does_not_establish_complete_task_context() {
    let source = vec![
        part(
            0,
            ContentKind::UserText,
            "Audit the parser before editing. Keep billing unchanged.",
            None,
            None,
        ),
        part(
            1,
            ContentKind::AssistantText,
            "I propose to audit the parser and its tests, then report the risks.",
            None,
            None,
        ),
        part(2, ContentKind::UserText, "Yes, continue.", None, None),
        part(
            3,
            ContentKind::ToolInput,
            r#"{"command":"cargo test parser"}"#,
            Some("Bash"),
            Some("audit"),
        ),
        part(
            4,
            ContentKind::ToolResult,
            "Parser tests pass.",
            Some("Bash"),
            Some("audit"),
        ),
    ];
    let check = check(source, 1, false);
    let plan = check.prepare(&check.session_context()).unwrap();
    assert!(check.task_contexts[0].fields["partial"] == true);
    assert!(check.task_contexts[0].fields["antecedent_context_omitted"] == true);
    assert!(
        plan.prepared.comparisons[0]
            .limitations
            .contains(&SkillOpportunityLimit::TaskContextPartial)
    );
    let result = check
        .reduce(&plan, &results(&plan, "no_opportunity", 0.9), true)
        .unwrap();
    assert!(!result.complete);
}

#[test]
fn settled_targets_preserve_the_plan_inventory_invariant() {
    let check = check(parts(), 4, false);
    let mut sampling = SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: 4096,
        answers_per_candidate: 1,
        judgments_per_run: 4,
    })
    .unwrap();
    check.synchronize_sampling(&mut sampling).unwrap();
    sampling.begin_run();
    let jobs = (0..4)
        .map(|_| sampling.choose_job().unwrap())
        .collect::<Vec<_>>();
    let mut plan = check
        .prepare_sampled(
            &check.session_context(),
            &ModelCapabilities::jev_default(),
            &jobs,
        )
        .unwrap();
    let mut broken = plan.clone();
    broken.work_items.remove(0);
    assert!(matches!(
        check.reduce(&broken, &[], false),
        Err(JevError::InvalidCheckPlan)
    ));
    check.retain_plan_jobs(&mut plan, &jobs[1..]).unwrap();
    assert_eq!(plan.prepared.comparisons.len(), 3);
    assert_eq!(plan.work_items.len() + plan.skipped_item_ids.len(), 3);
    check
        .reduce(&plan, &results(&plan, "no_opportunity", 0.9), true)
        .unwrap();
    let mut altered = plan.clone();
    altered.work_items[0].window.fields["work"] = json!([]);
    assert!(matches!(
        check.reduce(&altered, &[], false),
        Err(JevError::InvalidCheckPlan)
    ));
}

#[tokio::test]
async fn missing_task_is_unassessed_without_invalid_plan_or_dispatch() {
    let mut source = parts();
    source[0].turn_index = 3;
    source[0].uuid = Some("later-task".into());
    source.sort_by_key(|part| part.turn_index);
    let check = check(source, 1, false);
    let plan = check.prepare(&check.session_context()).unwrap();
    assert!(plan.work_items.is_empty());
    assert_eq!(plan.skipped_item_ids.len(), 1);
    let outcome = run_jev_check(
        &check,
        &check.session_context(),
        JevRunProgress::default(),
        |_| async { panic!("missing task must not dispatch") },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert!(!outcome.result.complete);
    assert!(
        outcome
            .result
            .decisions
            .iter()
            .all(|decision| decision.judgments.is_none())
    );
}
fn skill(index: usize) -> SkillDefinition {
    SkillDefinition {
        identity: format!("skill-{index}"),
        revision: "definition".into(),
        name: format!("review-{index}"),
        aliases: vec![],
        description: "Review Rust lock order, cancellation, and resource lifetime defects.".into(),
        frontmatter: json!({"description": "Review Rust lock order, cancellation, and resource lifetime defects."}),
        scope: scope(),
        enabled: true,
        created_at_ms: Some(10000),
    }
}
fn part(
    index: u64,
    kind: ContentKind,
    text: &str,
    tool: Option<&str>,
    call: Option<&str>,
) -> PublishedContentPart {
    PublishedContentPart {
        source_key: "transcript".into(),
        thread_id: "branch".into(),
        turn_index: index,
        role: match kind {
            ContentKind::UserText => "user",
            ContentKind::ToolResult => "tool",
            _ => "assistant",
        },
        scope: "main".into(),
        ts_ms: Some(index as i64 * 100),
        uuid: Some(format!("record-{index}")),
        message_id: None,
        part_index: 0,
        part: ContentPart::new(kind, text)
            .with_tool_identity(tool.map(str::to_owned), call.map(str::to_owned)),
        context_only: false,
        stable_event_identity: true,
    }
}
fn parts() -> Vec<PublishedContentPart> {
    vec![
        part(
            0,
            ContentKind::UserText,
            "Investigate the worker shutdown deadlock.",
            None,
            None,
        ),
        part(
            1,
            ContentKind::ToolInput,
            r#"{"command":"cargo test worker_shutdown"}"#,
            Some("Bash"),
            Some("test"),
        ),
        part(
            2,
            ContentKind::ToolResult,
            "FAILED: queue -> lease and lease -> queue",
            Some("Bash"),
            Some("test"),
        ),
    ]
}
fn check(parts: Vec<PublishedContentPart>, count: usize, partial: bool) -> SkillOpportunitiesCheck {
    check_with_skills(parts, (0..count).map(skill).collect(), partial)
}

fn check_with_skills(
    mut parts: Vec<PublishedContentPart>,
    skills: Vec<SkillDefinition>,
    partial: bool,
) -> SkillOpportunitiesCheck {
    parts.push(part(
        100,
        ContentKind::Thinking,
        "private unique marker",
        None,
        None,
    ));
    let page = PublishedContent {
        publication_fence: 4,
        source_generation: Some(3),
        parts,
        coverage: ContentQueryCoverage::default(),
        next_offset: 0,
    };
    let mut builder = SessionScopeBuilder::new(
        SourceFormat::ClaudeJsonl,
        SessionScopeBoundary {
            source_key: "transcript".into(),
            thread_id: "branch".into(),
            turn_index: 100,
            part_index: 0,
            branch: SessionScopeBranch::ProvenLinear,
        },
        4,
        3,
        true,
    )
    .unwrap();
    builder.push_page(page.clone(), false).unwrap();
    let scope_snapshot = builder.finish().unwrap();
    let mut content = prepare_session_content("session", SourceFormat::ClaudeJsonl, page, vec![]);
    if partial {
        content.complete = false;
        content.limitations.push("selected_input_gap".into());
    }
    let selected = select_session_content(&content, SKILL_USE_SELECTION);
    let usage = SkillUseSnapshot::from_selected_content(
        &selected,
        &SkillUseBoundary {
            session_identity: content.session_identity_digest.clone(),
            native_session_id: "native-session".into(),
            publication_fence: 4,
            scope: scope(),
        },
        &[],
    )
    .unwrap();
    SkillOpportunitiesCheck::new(
        &content,
        &SkillOpportunitySnapshot::new(scope(), skills, true).unwrap(),
        &usage,
        &scope_snapshot,
    )
    .unwrap()
}
fn results(
    plan: &JevCheckPlan<PreparedSkillOpportunities>,
    choice: &str,
    probability: f64,
) -> Vec<JevWorkItemResult> {
    pack_work_items_with_shared_context(
        &plan.work_items,
        &plan.capabilities,
        plan.shared_context.as_ref().unwrap(),
    )
    .batches
    .iter()
    .flat_map(|batch| {
        let response = JevResponse {
            model: batch.request.model.clone(),
            usage: JevUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            answers: batch
                .request
                .questions
                .keys()
                .map(|id| {
                    (
                        id.clone(),
                        JevAnswer::Choice {
                            choice: choice.into(),
                            probabilities: ["useful_opportunity", "no_opportunity", "uncertain"]
                                .into_iter()
                                .map(|key| {
                                    (
                                        key.into(),
                                        if key == choice {
                                            probability
                                        } else {
                                            (1.0 - probability) / 2.0
                                        },
                                    )
                                })
                                .collect(),
                            confidence: 0.1,
                        },
                    )
                })
                .collect(),
        };
        unpack_jev_response(batch, &response).unwrap()
    })
    .collect()
}

#[test]
fn one_decision_uses_selected_choice_and_positive_threshold() {
    let check = check(parts(), 1, false);
    let plan = check.prepare(&check.session_context()).unwrap();
    assert_eq!(plan.work_items[0].questions.len(), 1);
    for (choice, probability, expected) in [
        (
            "useful_opportunity",
            0.75,
            SkillOpportunityOutcome::Advisory,
        ),
        (
            "useful_opportunity",
            0.74,
            SkillOpportunityOutcome::Uncertain,
        ),
        (
            "no_opportunity",
            0.6,
            SkillOpportunityOutcome::NoOpportunity,
        ),
        ("uncertain", 0.8, SkillOpportunityOutcome::Uncertain),
    ] {
        let result = check
            .reduce(&plan, &results(&plan, choice, probability), true)
            .unwrap();
        assert_eq!(result.decisions[0].outcome, expected);
        assert_eq!(
            result.findings.len(),
            usize::from(expected == SkillOpportunityOutcome::Advisory)
        );
        assert_eq!(
            result.complete,
            expected != SkillOpportunityOutcome::Uncertain
        );
    }
}

#[test]
fn partial_work_and_unknown_use_reach_model_with_bound_attempts() {
    let mut activity = parts();
    activity.pop();
    activity.push(part(
        3,
        ContentKind::ToolInput,
        r#"{"skill":"review-0","name":"conflicting"}"#,
        Some("Skill"),
        Some("unknown"),
    ));
    activity.extend([
        part(
            4,
            ContentKind::ToolInput,
            r#"{"path":"worker.rs"}"#,
            Some("Read"),
            Some("read"),
        ),
        part(
            5,
            ContentKind::ToolResult,
            "queue.lock(); lease.lock();",
            Some("Read"),
            Some("read"),
        ),
    ]);
    let check = check(activity, 1, true);
    let plan = check.prepare(&check.session_context()).unwrap();
    assert_eq!(plan.work_items.len(), 1);
    assert_eq!(plan.prepared.comparisons[0].work.len(), 3);
    assert!(!plan.prepared.comparisons[0].absence_assessable);
    let payload = plan.work_items[0].window.fields.to_string();
    assert!(payload.contains("cargo test worker_shutdown"));
    assert!(!payload.contains("private unique marker"));
    let result = check
        .reduce(&plan, &results(&plan, "useful_opportunity", 0.9), true)
        .unwrap();
    assert_eq!(result.findings.len(), 1);
    assert!(!result.complete);
    assert!(!result.findings[0].absence_limit.contains("No matching use"));
    for work in &result.findings[0].comparison.work {
        assert!(
            result.findings[0]
                .evidence
                .iter()
                .any(|evidence| evidence.source_id == work.reference.id)
        );
    }
}

#[test]
fn descriptor_inventory_hydrates_only_selected_pairs_and_batches_them() {
    let mut activity = parts();
    activity.extend([
        part(
            3,
            ContentKind::UserText,
            "Now inspect cancellation.",
            None,
            None,
        ),
        part(
            4,
            ContentKind::ToolInput,
            r#"{"path":"cancel.rs"}"#,
            Some("Read"),
            Some("cancel"),
        ),
    ]);
    let check = check(activity, 260, false);
    assert_eq!(check.descriptors.len(), 520);
    assert_eq!(check.items.len(), 260);
    assert!(
        check
            .prepared
            .comparisons
            .iter()
            .all(|comparison| comparison.work.is_empty())
    );
    let mut sampling = SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: 4096,
        answers_per_candidate: 1,
        judgments_per_run: 4,
    })
    .unwrap();
    check.synchronize_sampling(&mut sampling).unwrap();
    sampling.begin_run();
    let jobs: Vec<_> = (0..4).map(|_| sampling.choose_job().unwrap()).collect();
    let plan = check
        .prepare_sampled(
            &check.session_context(),
            &ModelCapabilities::jev_default(),
            &jobs,
        )
        .unwrap();
    assert_eq!(plan.work_items.len(), 4);
    assert_eq!(plan.coverage.not_selected_items, 516);
    let packed = pack_work_items_with_shared_context(
        &plan.work_items,
        &plan.capabilities,
        plan.shared_context.as_ref().unwrap(),
    );
    assert_eq!(packed.batches.len(), 1);
    assert_eq!(packed.batches[0].request.questions.len(), 4);
    let result = check
        .reduce(&plan, &results(&plan, "uncertain", 0.9), true)
        .unwrap();
    for job in &jobs {
        check
            .record_sampling_result(&mut sampling, job, &result)
            .unwrap();
    }
    let mut restored: SamplingProgress =
        serde_json::from_value(serde_json::to_value(sampling).unwrap()).unwrap();
    check.synchronize_sampling(&mut restored).unwrap();
    assert_eq!(restored.coverage(check_identity()).unwrap().completed, 4);
    restored.begin_run();
    assert!(!jobs.contains(&restored.choose_job().unwrap()));
}

#[test]
fn equivalent_recorded_use_is_context_for_the_single_decision() {
    let mut activity = parts();
    activity.push(part(
        3,
        ContentKind::ToolInput,
        r#"{"skill":"review-0"}"#,
        Some("Skill"),
        Some("review"),
    ));
    let check = check(activity, 2, false);
    let plan = check.prepare(&check.session_context()).unwrap();
    assert_eq!(plan.work_items.len(), 2);
    assert!(
        plan.shared_context.as_ref().unwrap().fields["skill_references"]
            .to_string()
            .contains("lock order")
    );
    assert!(
        check
            .reduce(&plan, &results(&plan, "no_opportunity", 0.9), true)
            .unwrap()
            .findings
            .is_empty()
    );
}

#[test]
fn packed_skill_references_exclude_metadata_and_body_when_description_exists() {
    let mut definition = skill(0);
    definition.description = "UNIQUE_SKILL_DESCRIPTION_MARKER".into();
    definition.frontmatter = json!({
        "description": definition.description,
        "metadata": {"private": "UNIQUE_SKILL_BODY_MARKER"},
        "body": "UNIQUE_SKILL_BODY_MARKER"
    });
    for used in [false, true] {
        let mut activity = parts();
        if used {
            activity.push(part(
                3,
                ContentKind::ToolInput,
                r#"{"skill":"review-0"}"#,
                Some("Skill"),
                Some("review"),
            ));
        }
        let check = check_with_skills(activity, vec![definition.clone()], false);
        let plan = check.prepare(&check.session_context()).unwrap();
        let packed = pack_work_items_with_shared_context(
            &plan.work_items,
            &plan.capabilities,
            plan.shared_context.as_ref().unwrap(),
        );
        assert!(!packed.batches.is_empty());
        assert!(packed.skipped_item_ids.is_empty());
        for batch in packed.batches {
            let request = serde_json::to_string(&batch.request).unwrap();
            assert!(!request.contains("UNIQUE_SKILL_BODY_MARKER"));
            assert_eq!(
                request.matches("UNIQUE_SKILL_DESCRIPTION_MARKER").count(),
                1
            );
            assert!(request.contains("description"));
            assert!(!request.contains("markdown_fallback"));
        }
    }
}

#[test]
fn packed_fallback_references_include_markdown_and_preserve_partial_ranges() {
    for frontmatter in [
        json!({}),
        json!({"description": null}),
        json!({"description": "  "}),
    ] {
        for used in [false, true] {
            let mut definition = skill(0);
            definition.frontmatter = frontmatter.clone();
            definition.description = format!(
                "# Skill\n\nUNIQUE_FALLBACK_BODY_MARKER\n{}",
                "Review ownership.\n".repeat(2000)
            );
            let mut activity = parts();
            if used {
                activity.push(part(
                    3,
                    ContentKind::ToolInput,
                    r#"{"skill":"review-0"}"#,
                    Some("Skill"),
                    Some("review"),
                ));
            }
            let check = check_with_skills(activity, vec![definition.clone()], false);
            let plan = check.prepare(&check.session_context()).unwrap();
            let packed = pack_work_items_with_shared_context(
                &plan.work_items,
                &plan.capabilities,
                plan.shared_context.as_ref().unwrap(),
            );
            assert!(!packed.batches.is_empty());
            assert!(packed.skipped_item_ids.is_empty());
            for batch in packed.batches {
                let request = serde_json::to_string(&batch.request).unwrap();
                assert_eq!(request.matches("UNIQUE_FALLBACK_BODY_MARKER").count(), 1);
                assert!(request.contains("markdown_fallback"));
                assert!(request.contains("\"partial\":true"));
                assert!(request.contains("start_byte"));
                assert!(request.contains("end_byte"));
            }
        }
    }
}

#[test]
fn missing_invalid_and_foreign_answers_do_not_complete_sampling() {
    let check = check(parts(), 1, false);
    let plan = check.prepare(&check.session_context()).unwrap();
    let missing = check.reduce(&plan, &[], false).unwrap();
    assert_eq!(
        missing.decisions[0].outcome,
        SkillOpportunityOutcome::Unassessed
    );
    let mut answers = results(&plan, "useful_opportunity", 0.9);
    answers[0].evidence[0].source_id = "foreign".into();
    assert_eq!(
        check.reduce(&plan, &answers, true).unwrap_err(),
        JevError::InvalidCheckPlan
    );
    let mut stale = plan.clone();
    stale.revisions.questions -= 1;
    assert_eq!(
        check.reduce(&stale, &[], true).unwrap_err(),
        JevError::InvalidCheckPlan
    );
}

fn large_fallback() -> SkillDefinition {
    let mut definition = skill(0);
    definition.frontmatter = json!({});
    definition.description = (0..256)
        .map(|index| {
            let marker = match index {
                0 => "EARLY_REFERENCE_MARKER",
                127 => "MIDDLE_REFERENCE_MARKER",
                255 => "LATE_REFERENCE_MARKER",
                220 => "UNSELECTED_REFERENCE_MARKER",
                _ => "Review resource ownership.",
            };
            format!("{marker}{}\n", "x".repeat(4095 - marker.len()))
        })
        .collect();
    definition
}

#[test]
fn requests_and_durable_results_sample_whole_fallback_without_full_document() {
    let definition = large_fallback();
    let check = check_with_skills(parts(), vec![definition.clone()], false);
    let mut plan = check.prepare(&check.session_context()).unwrap();
    let citation = &plan.prepared.comparisons[0].skill;
    assert!(citation.matches_definition(&definition));
    assert_eq!(citation.description.len(), 4 * 4096);
    assert_eq!(citation.reference.total_bytes, definition.description.len());
    assert!(citation.reference.partial);
    assert_eq!(
        citation.reference.ranges.last().unwrap().1,
        definition.description.len()
    );
    let packed = pack_work_items_with_shared_context(
        &plan.work_items,
        &plan.capabilities,
        plan.shared_context.as_ref().unwrap(),
    );
    assert!(!packed.batches.is_empty());
    assert!(packed.skipped_item_ids.is_empty());
    for batch in packed.batches {
        let request = serde_json::to_string(&batch.request).unwrap();
        for marker in [
            "EARLY_REFERENCE_MARKER",
            "MIDDLE_REFERENCE_MARKER",
            "LATE_REFERENCE_MARKER",
        ] {
            assert!(request.contains(marker));
        }
        assert!(!request.contains("UNSELECTED_REFERENCE_MARKER"));
        assert!(request.len() < 32 * 1024);
    }
    plan.coverage.limitations.clear();
    for choice in ["useful_opportunity", "no_opportunity"] {
        let result = check
            .reduce(&plan, &results(&plan, choice, 0.9), true)
            .unwrap();
        assert!(!result.complete);
        assert!(
            result
                .coverage
                .limitations
                .iter()
                .any(|limit| limit == "skill_reference_content_partial")
        );
        let durable = serde_json::to_string(&result).unwrap();
        if choice == "useful_opportunity" {
            assert!(
                result.findings[0]
                    .absence_limit
                    .contains("Only selected skill reference ranges")
            );
        }
        assert!(durable.len() < 64 * 1024);
        assert!(!durable.contains("UNSELECTED_REFERENCE_MARKER"));
        assert!(durable.contains("LATE_REFERENCE_MARKER"));
    }
}

#[test]
fn many_large_used_skills_and_old_tasks_do_not_block_an_exact_alias_match() {
    let mut activity = (0..20)
        .map(|index| {
            part(
                index,
                ContentKind::UserText,
                &format!("OLD_TASK_PRIVATE_MARKER{}", "x".repeat(20 * 1024)),
                None,
                None,
            )
        })
        .collect::<Vec<_>>();
    activity.extend([
        part(
            20,
            ContentKind::UserText,
            "CURRENT_TASK_MARKER Investigate the worker shutdown deadlock.",
            None,
            None,
        ),
        part(
            21,
            ContentKind::ToolInput,
            r#"{"command":"cargo test worker_shutdown"}"#,
            Some("Bash"),
            Some("test"),
        ),
        part(
            22,
            ContentKind::ToolResult,
            "FAILED: lock order",
            Some("Bash"),
            Some("test"),
        ),
    ]);
    let mut definitions = (0..32).map(skill).collect::<Vec<_>>();
    definitions[0].aliases = vec!["review-alias".into()];
    for (index, definition) in definitions.iter_mut().enumerate().skip(1) {
        definition.frontmatter = json!({});
        definition.description = format!(
            "UNRELATED_SKILL_PRIVATE_MARKER_{index}\n{}",
            "Garden seedlings and irrigation.\n".repeat(2000)
        );
        activity.push(part(
            22 + index as u64,
            ContentKind::ToolInput,
            &format!(r#"{{"skill":"review-{index}"}}"#),
            Some("Skill"),
            Some(&format!("use-{index}")),
        ));
    }
    activity.push(part(
        55,
        ContentKind::ToolInput,
        r#"{"skill":"review-alias"}"#,
        Some("Skill"),
        Some("exact-use"),
    ));
    let check = check_with_skills(activity, definitions, false);
    let plan = check.prepare(&check.session_context()).unwrap();
    let exact = plan
        .prepared
        .comparisons
        .iter()
        .find(|comparison| comparison.skill.identity == "skill-0")
        .unwrap();
    assert!(
        exact
            .used_current_skills
            .iter()
            .any(|skill| skill.identity == "skill-0")
    );
    assert!(!exact.use_citations.is_empty());
    assert!(
        plan.coverage
            .limitations
            .iter()
            .any(|limit| limit == "selected_known_use_context_only")
    );
    let exact_item = plan
        .work_items
        .iter()
        .find(|item| item.id == exact.id)
        .unwrap();
    let shared = check.shared_context(&BTreeSet::from([exact.id.as_str()]));
    let packed = pack_work_items_with_shared_context(
        std::slice::from_ref(exact_item),
        &plan.capabilities,
        &shared,
    );
    assert_eq!(packed.batches.len(), 1);
    assert!(packed.skipped_item_ids.is_empty());
    let request = serde_json::to_string(&packed.batches[0].request).unwrap();
    assert!(request.contains("CURRENT_TASK_MARKER"));
    assert!(!request.contains("OLD_TASK_PRIVATE_MARKER"));
    assert!(!request.contains("UNRELATED_SKILL_PRIVATE_MARKER"));
    assert_eq!(request.matches("Review Rust lock order").count(), 1);
    assert_eq!(request.matches("review-alias").count(), 1);
    assert!(request.len() < 16 * 1024);
    let result = check
        .reduce(&plan, &results(&plan, "no_opportunity", 0.9), true)
        .unwrap();
    assert!(!result.complete);
}

#[test]
fn choice_must_have_a_highest_probability_and_ties_remain_valid() {
    let check = check(parts(), 1, false);
    let plan = check.prepare(&check.session_context()).unwrap();
    for choice in ["useful_opportunity", "no_opportunity", "uncertain"] {
        let answers = results(&plan, choice, 0.2);
        assert_eq!(
            check.reduce(&plan, &answers, true).unwrap_err(),
            JevError::InvalidChoiceDistribution
        );
    }
    let mut answers = results(&plan, "no_opportunity", 0.4);
    if let JevAnswer::Choice { probabilities, .. } =
        answers[0].answers.get_mut("opportunity").unwrap()
    {
        probabilities.insert("useful_opportunity".into(), 0.4);
        probabilities.insert("uncertain".into(), 0.2);
    }
    assert!(check.reduce(&plan, &answers, true).is_ok());
}

#[test]
fn shared_context_deduplicates_use_facts_and_equal_reference_text() {
    let mut definitions = (0..3).map(skill).collect::<Vec<_>>();
    for definition in &mut definitions[..2] {
        definition.description = "SHARED_REFERENCE_MARKER Review lock order.".into();
        definition.frontmatter = json!({"description": definition.description});
    }
    let mut activity = parts();
    for (index, name) in [(3, "review-0"), (4, "review-0"), (5, "review-1")] {
        let mut event = part(
            index,
            ContentKind::ToolInput,
            &format!(r#"{{"skill":"{name}"}}"#),
            Some("Skill"),
            Some(&format!("use-{index}")),
        );
        event.ts_ms = Some(300);
        activity.push(event);
    }
    let check = check_with_skills(activity, definitions, false);
    let item = check.hydrate(&check.descriptors[2]).1;
    let plan = check
        .build_plan(
            &check.session_context(),
            &ModelCapabilities::jev_default(),
            &[item],
        )
        .unwrap();
    let shared = &plan.shared_context.as_ref().unwrap().fields;
    assert_eq!(shared["used_current_skills"].as_array().unwrap().len(), 2);
    assert_eq!(shared["skill_references"].as_object().unwrap().len(), 1);
    assert_eq!(shared["use"].as_array().unwrap().len(), 2);
    assert!(
        shared["use"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["source_ids"].as_array().unwrap().len() == 2)
    );
    let packed = pack_work_items_with_shared_context(
        &plan.work_items,
        &plan.capabilities,
        plan.shared_context.as_ref().unwrap(),
    );
    assert_eq!(packed.batches.len(), 1);
    assert_eq!(
        serde_json::to_string(&packed.batches[0].request)
            .unwrap()
            .matches("SHARED_REFERENCE_MARKER")
            .count(),
        1
    );
}

#[test]
fn exact_use_is_selected_before_a_large_equivalent_use_history() {
    let mut definitions = vec![skill(0), skill(1)];
    definitions[0].aliases = vec!["review-alias".into()];
    let mut activity = parts();
    for index in 3..40 {
        activity.push(part(
            index,
            ContentKind::ToolInput,
            r#"{"skill":"review-1"}"#,
            Some("Skill"),
            Some(&format!("use-{index}")),
        ));
    }
    activity.push(part(
        40,
        ContentKind::ToolInput,
        r#"{"skill":"review-alias"}"#,
        Some("Skill"),
        Some("exact"),
    ));
    let check = check_with_skills(activity, definitions, false);
    let (comparison, item) = check.hydrate(&check.descriptors[0]);
    assert!(
        comparison
            .used_current_skills
            .iter()
            .any(|skill| skill.identity == "skill-0")
    );
    assert!(
        comparison
            .use_citations
            .iter()
            .any(|reference| reference.turn_index == 40)
    );
    assert_eq!(comparison.use_citations.len(), 32);
    assert_eq!(item.window.fields["current_skill_used"], true);
    let mut plan = check.prepare(&check.session_context()).unwrap();
    assert_eq!(
        plan.shared_context.as_ref().unwrap().fields["tasks"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    plan.coverage.limitations.clear();
    let result = check
        .reduce(&plan, &results(&plan, "no_opportunity", 0.9), true)
        .unwrap();
    assert!(!result.complete);
    assert!(
        result
            .coverage
            .limitations
            .iter()
            .any(|limit| limit == "selected_known_use_context_only")
    );
}

#[test]
fn partial_task_context_cannot_produce_a_complete_negative() {
    let mut activity = parts();
    activity[0] = part(
        0,
        ContentKind::UserText,
        &"Investigate the worker shutdown deadlock.\n".repeat(200),
        None,
        None,
    );
    let check = check(activity, 1, false);
    let mut plan = check.prepare(&check.session_context()).unwrap();
    assert!(
        plan.prepared.comparisons[0]
            .limitations
            .contains(&SkillOpportunityLimit::TaskContextPartial)
    );
    plan.coverage.limitations.clear();
    let result = check
        .reduce(&plan, &results(&plan, "no_opportunity", 0.9), true)
        .unwrap();
    assert!(!result.complete);
    assert!(
        result
            .coverage
            .limitations
            .iter()
            .any(|limit| limit == "selected_task_context_only")
    );
}

#[tokio::test]
async fn runner_dispatches_one_question_per_pair_without_followup() {
    let check = check(parts(), 1, false);
    let outcome = run_jev_check(
        &check,
        &check.session_context(),
        JevRunProgress::default(),
        |batch| async move {
            assert_eq!(batch.request.questions.len(), 1);
            let response = JevResponse {
                model: batch.request.model.clone(),
                usage: JevUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                },
                answers: batch
                    .request
                    .questions
                    .keys()
                    .map(|id| {
                        (
                            id.clone(),
                            JevAnswer::Choice {
                                choice: "useful_opportunity".into(),
                                probabilities: BTreeMap::from([
                                    ("useful_opportunity".into(), 0.9),
                                    ("no_opportunity".into(), 0.05),
                                    ("uncertain".into(), 0.05),
                                ]),
                                confidence: 0.1,
                            },
                        )
                    })
                    .collect(),
            };
            Ok(response)
        },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(outcome.result.findings.len(), 1);
}
