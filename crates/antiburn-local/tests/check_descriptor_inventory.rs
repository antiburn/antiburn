use std::collections::BTreeSet;
use std::sync::Arc;

use antiburn_local::analysis::jev::capabilities::ModelCapabilities;
use antiburn_local::analysis::jev::{
    JevAnswer, JevCheck, JevCheckPlan, JevError, JevResponse, JevUsage,
    pack_work_items_with_capabilities, pack_work_items_with_shared_context, unpack_jev_response,
};
use antiburn_local::analysis::jev_evidence::{
    SessionContentEvidence, content_action_digest, prepare_session_content, select_session_content,
};
use antiburn_local::analysis::session_scope::{
    SessionScopeBoundary, SessionScopeBranch, SessionScopeBuilder, SessionScopeSnapshot,
};
use antiburn_local::analysis::{
    ContentKind, ContentPart, PublishedContent, PublishedContentPart, SourceFormat,
};
use antiburn_local::checks::sampling::{SamplingLimits, SamplingProgress};
use antiburn_local::checks::scope_creep::{
    ScopeCreepCheck, ScopeCreepInput, ScopeCreepStatus, ScopeDescriptorInventory,
};
use antiburn_local::checks::skill_opportunities::{
    SKILL_USE_SELECTION, SkillDefinition, SkillDescriptorInventory, SkillOpportunitiesCheck,
    SkillOpportunityOutcome, SkillOpportunitySnapshot, SkillScope, SkillUseBoundary,
    SkillUseSnapshot,
};
use antiburn_local::model::AgentKind;

fn source(
    count: usize,
) -> (
    SessionContentEvidence,
    Arc<SessionScopeSnapshot>,
    SessionScopeBoundary,
) {
    let mut parts = Vec::new();
    for index in 0..count {
        for (offset, kind, text) in [
            (0, ContentKind::UserText, "Fix the worker shutdown bug."),
            (
                1,
                ContentKind::ToolInput,
                r#"{"command":"cargo test worker_shutdown"}"#,
            ),
            (2, ContentKind::ToolResult, "FAILED: lock order mismatch"),
        ] {
            let turn = (index * 3 + offset) as u64;
            let tool = kind != ContentKind::UserText;
            parts.push(PublishedContentPart {
                source_key: "transcript".into(),
                thread_id: "branch".into(),
                turn_index: turn,
                role: match kind {
                    ContentKind::UserText => "user",
                    ContentKind::ToolResult => "tool",
                    _ => "assistant",
                },
                scope: "main".into(),
                ts_ms: Some(turn as i64),
                uuid: Some(format!("record-{turn}")),
                message_id: None,
                part_index: 0,
                part: ContentPart::new(kind, text).with_tool_identity(
                    tool.then(|| "Bash".into()),
                    tool.then(|| format!("call-{index}")),
                ),
                context_only: false,
                stable_event_identity: true,
            });
        }
    }
    let boundary = SessionScopeBoundary {
        source_key: "transcript".into(),
        thread_id: "branch".into(),
        turn_index: (count * 3 - 1) as u64,
        part_index: 0,
        branch: SessionScopeBranch::ProvenLinear,
    };
    let page = PublishedContent {
        publication_fence: 4,
        source_generation: Some(3),
        parts,
        ..Default::default()
    };
    let mut builder =
        SessionScopeBuilder::new(SourceFormat::ClaudeJsonl, boundary.clone(), 4, 3, true).unwrap();
    builder.push_page(page.clone(), false).unwrap();
    (
        prepare_session_content("session", SourceFormat::ClaudeJsonl, page, vec![]),
        Arc::new(builder.finish().unwrap()),
        boundary,
    )
}

fn scope_check(count: usize) -> ScopeCreepCheck {
    let (content, scope, boundary) = source(count);
    ScopeCreepCheck::new(ScopeCreepInput {
        scope,
        content,
        boundary,
        source_generation: 3,
        ignored_instruction_work_ids: BTreeSet::new(),
    })
    .unwrap()
}

fn skill_check(count: usize, skills: usize) -> SkillOpportunitiesCheck {
    let (content, scope, _) = source(count);
    skill_check_from_source(&content, &scope, skills)
}

fn skill_check_from_source(
    content: &SessionContentEvidence,
    scope: &SessionScopeSnapshot,
    skills: usize,
) -> SkillOpportunitiesCheck {
    let skill_scope = SkillScope {
        agent: AgentKind::Claude,
        project_identity: Some("project".into()),
        environment_identity: "native".into(),
    };
    let inventory = SkillOpportunitySnapshot::new(skill_scope.clone(), (0..skills).map(|index| SkillDefinition {
        identity: format!("skill-{index}"), revision: "definition".into(), name: format!("review-{index}"), aliases: vec![],
        description: "Review Rust lock order and cancellation.".into(), frontmatter: serde_json::json!({"description": "Review Rust lock order and cancellation."}), scope: skill_scope.clone(), enabled: true, created_at_ms: None,
    }).collect(), true).unwrap();
    let usage = SkillUseSnapshot::from_selected_content(
        &select_session_content(content, SKILL_USE_SELECTION),
        &SkillUseBoundary {
            session_identity: content.session_identity_digest.clone(),
            native_session_id: "session".into(),
            publication_fence: 4,
            scope: skill_scope,
        },
        &[],
    )
    .unwrap();
    SkillOpportunitiesCheck::new(content, &inventory, &usage, scope).unwrap()
}

fn large_source(
    tool: &str,
    request: &str,
    result: &str,
) -> (
    SessionContentEvidence,
    Arc<SessionScopeSnapshot>,
    SessionScopeBoundary,
) {
    let mut parts = Vec::new();
    for (turn, kind, text) in [
        (
            0,
            ContentKind::UserText,
            "Fix worker_shutdown cancellation and preserve lock order.",
        ),
        (1, ContentKind::ToolInput, request),
        (2, ContentKind::ToolResult, result),
        (3, ContentKind::ToolInput, request),
        (4, ContentKind::ToolResult, result),
    ] {
        let is_tool = kind != ContentKind::UserText;
        parts.push(PublishedContentPart {
            source_key: "transcript".into(),
            thread_id: "branch".into(),
            turn_index: turn,
            role: match kind {
                ContentKind::UserText => "user",
                ContentKind::ToolResult => "tool",
                _ => "assistant",
            },
            scope: "main".into(),
            ts_ms: Some(turn as i64),
            uuid: Some(format!("large-{turn}")),
            message_id: None,
            part_index: 0,
            part: ContentPart::new(kind, text).with_tool_identity(
                is_tool.then(|| tool.into()),
                is_tool.then(|| format!("call-{}", (turn - 1) / 2)),
            ),
            context_only: false,
            stable_event_identity: true,
        });
    }
    let boundary = SessionScopeBoundary {
        source_key: "transcript".into(),
        thread_id: "branch".into(),
        turn_index: 4,
        part_index: 0,
        branch: SessionScopeBranch::ProvenLinear,
    };
    let page = PublishedContent {
        publication_fence: 4,
        source_generation: Some(3),
        parts,
        ..Default::default()
    };
    let mut builder =
        SessionScopeBuilder::new(SourceFormat::ClaudeJsonl, boundary.clone(), 4, 3, true).unwrap();
    builder.push_page(page.clone(), false).unwrap();
    (
        prepare_session_content("session", SourceFormat::ClaudeJsonl, page, vec![]),
        Arc::new(builder.finish().unwrap()),
        boundary,
    )
}

fn smaller_provider() -> ModelCapabilities {
    let mut capabilities = ModelCapabilities::jev_default();
    capabilities.total_input_tokens.value = Some(16 * 1024);
    capabilities.state_and_longest_question_tokens.value = Some(16 * 1024);
    capabilities.rendering_reserve_tokens = 1024;
    capabilities
}

fn semantic_results<P>(
    plan: &JevCheckPlan<P>,
    choice: &str,
    choices: &[&str],
) -> Vec<antiburn_local::analysis::jev::JevWorkItemResult> {
    let choices = choices
        .iter()
        .copied()
        .chain(
            (plan.check_id == "skill_opportunities")
                .then_some(["specialist_check", "already_covered"])
                .into_iter()
                .flatten(),
        )
        .collect::<BTreeSet<_>>();
    let packing = match &plan.shared_context {
        Some(shared) => {
            pack_work_items_with_shared_context(&plan.work_items, &plan.capabilities, shared)
        }
        None => pack_work_items_with_capabilities(&plan.work_items, &plan.capabilities),
    };
    packing
        .batches
        .iter()
        .flat_map(|batch| {
            unpack_jev_response(
                batch,
                &JevResponse {
                    model: plan.capabilities.model.clone(),
                    usage: JevUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                    },
                    answers: batch
                        .answer_owners
                        .keys()
                        .map(|key| {
                            (
                                key.clone(),
                                JevAnswer::Choice {
                                    choice: choice.into(),
                                    confidence: 1.0,
                                    probabilities: choices
                                        .iter()
                                        .map(|candidate| {
                                            (
                                                (*candidate).into(),
                                                if *candidate == choice { 1.0 } else { 0.0 },
                                            )
                                        })
                                        .collect(),
                                },
                            )
                        })
                        .collect(),
                },
            )
            .unwrap()
        })
        .collect()
}

fn assert_source_chunks(content: &serde_json::Value, source: &str) {
    assert_eq!(content["total_bytes"], source.len());
    assert_eq!(content["partial"], true);
    let chunks = content["chunks"].as_array().unwrap();
    assert!(chunks.len() <= 4);
    assert_eq!(chunks[0]["start_byte"], 0);
    assert_eq!(chunks.last().unwrap()["end_byte"], source.len());
    for chunk in chunks {
        let start = chunk["start_byte"].as_u64().unwrap() as usize;
        let end = chunk["end_byte"].as_u64().unwrap() as usize;
        assert_eq!(chunk["text"], &source[start..end]);
    }
}

#[test]
fn selected_large_scope_edit_and_result_pack_with_exact_ranges_and_partial_negative() {
    let body = format!(
        "early\n{}\nworker_shutdown cancellation\n{}\nmiddle\n{}\nlate\n",
        "filler 新\n".repeat(2500),
        "filler 新\n".repeat(2500),
        "filler 新\n".repeat(5000)
    );
    let request = serde_json::json!({"file_path": "src/worker.rs", "content": body}).to_string();
    let (content, scope, boundary) = large_source("Write", &request, &body);
    let selected_source = select_session_content(
        &content,
        antiburn_local::checks::scope_creep::INPUT_SELECTION,
    );
    let check = ScopeCreepCheck::new(ScopeCreepInput {
        content,
        scope,
        boundary,
        source_generation: 3,
        ignored_instruction_work_ids: BTreeSet::new(),
    })
    .unwrap();
    let mut inventory = ScopeDescriptorInventory::default();
    check.enumerate_descriptors(&mut inventory).unwrap();
    assert_eq!(inventory.groups.len(), 2);
    let capabilities = smaller_provider();
    let candidates = check
        .descriptor_candidates(&inventory, &capabilities)
        .unwrap();
    let plan = check
        .prepare_descriptors(
            &inventory,
            &capabilities,
            &BTreeSet::from([candidates[0].id]),
        )
        .unwrap();
    assert_eq!(plan.work_items.len(), 1);
    assert_eq!(plan.coverage.not_selected_items, 1);
    let work = &plan.work_items[0].window.fields["bound_work"];
    assert_source_chunks(&work[0]["content"], &selected_source.actions[1].text);
    assert_source_chunks(&work[1]["content"], &selected_source.actions[2].text);
    assert_source_chunks(
        &work[0]["normalized_fields"]["values"]["file_edit_content"],
        &selected_source.actions[1]
            .normalized_fields
            .as_ref()
            .unwrap()
            .values[&antiburn_local::analysis::jev::JevInputField::FileEditContent],
    );
    assert!(
        work[0]["normalized_fields"]
            .to_string()
            .contains("worker_shutdown cancellation")
    );
    let results = semantic_results(
        &plan,
        "no_issue",
        &["likely_scope_expansion", "no_issue", "uncertain"],
    );
    assert_eq!(results.len(), 1);
    let reduced = check.reduce(&plan, &results, true).unwrap();
    assert_eq!(reduced.decisions[0].status, ScopeCreepStatus::Uncertain);
    assert_eq!(reduced.assessed_candidates, 1);
    assert!(
        reduced
            .coverage
            .limitations
            .iter()
            .any(|limit| limit == "activity_content_partial")
    );
}

#[test]
fn large_scope_command_preserves_exact_prefix_and_marks_sampled_body() {
    let command = format!(
        "python --isolated - <<'PY'\n{}\nPY\n",
        "print('worker_shutdown 新')\n".repeat(4000)
    );
    let request = serde_json::json!({"command": command}).to_string();
    let (content, scope, boundary) = large_source("Bash", &request, &"result 新\n".repeat(5000));
    let check = ScopeCreepCheck::new(ScopeCreepInput {
        content,
        scope,
        boundary,
        source_generation: 3,
        ignored_instruction_work_ids: BTreeSet::new(),
    })
    .unwrap();
    let mut inventory = ScopeDescriptorInventory::default();
    check.enumerate_descriptors(&mut inventory).unwrap();
    let capabilities = smaller_provider();
    let candidates = check
        .descriptor_candidates(&inventory, &capabilities)
        .unwrap();
    let plan = check
        .prepare_descriptors(
            &inventory,
            &capabilities,
            &BTreeSet::from([candidates[0].id]),
        )
        .unwrap();
    assert_eq!(plan.work_items.len(), 1);
    let input = &plan.work_items[0].window.fields["bound_work"][0]["normalized_fields"]["values"]["bash_command_input"];
    assert_source_chunks(input, &command);
    assert_eq!(
        input["exact_command_prefix"]["text"],
        "python --isolated - <<'PY'\n"
    );
    assert_eq!(
        input["exact_command_prefix"]["governed_options_complete"],
        false
    );
}

#[test]
fn selected_large_skill_work_retains_parts_and_partial_citations_without_clean_negative() {
    let body = format!(
        "early\n{}\nworker_shutdown cancellation\n{}\nlate\n",
        "filler 新\n".repeat(3000),
        "filler 新\n".repeat(5000)
    );
    let request = serde_json::json!({"file_path": "src/worker.rs", "content": body}).to_string();
    let (content, scope, _) = large_source("Write", &request, &body);
    let check = skill_check_from_source(&content, &scope, 1);
    let selected_source = select_session_content(
        &content,
        antiburn_local::checks::skill_opportunities::SKILL_OPPORTUNITIES_INPUT_SELECTION,
    );
    let mut inventory = SkillDescriptorInventory::default();
    check.enumerate_descriptors(&mut inventory).unwrap();
    let candidates = check.descriptor_candidates(&inventory).unwrap();
    let mut sampling = SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: 4096,
        answers_per_candidate: 1,
        judgments_per_run: 4,
    })
    .unwrap();
    sampling
        .synchronize_ordered(
            check.sampling_identity(),
            check.sampling_epoch(),
            &candidates,
            &check.descriptor_chronology(&inventory).unwrap(),
        )
        .unwrap();
    sampling.begin_run();
    let job = sampling.choose_job().unwrap();
    let plan = check
        .prepare_inventory_sampled(
            &inventory,
            &check.session_context(),
            &smaller_provider(),
            std::slice::from_ref(&job),
        )
        .unwrap();
    assert_eq!(plan.work_items.len(), 1);
    let comparison = &plan.prepared.comparisons[0];
    assert_eq!(comparison.work.len(), 2);
    assert!(!comparison.work_context_assessable);
    for (index, citation) in comparison.work.iter().enumerate() {
        let action = selected_source
            .actions
            .iter()
            .find(|action| action.reference == citation.reference)
            .unwrap();
        let selected = citation
            .ranges
            .iter()
            .map(|&(start, end)| &action.text[start..end])
            .collect::<String>();
        assert_eq!(citation.text, selected);
        assert_eq!(citation.source_digest, content_action_digest(action));
        assert!(citation.matches_action(action));
        let mut altered = action.clone();
        let gap = citation
            .ranges
            .windows(2)
            .find(|ranges| ranges[0].1 < ranges[1].0)
            .unwrap();
        let hidden = gap[0].1 + action.text[gap[0].1..gap[1].0].find("filler").unwrap();
        altered.text.replace_range(hidden..hidden + 6, "change");
        assert_eq!(altered.text.len(), action.text.len());
        assert!(!citation.matches_action(&altered));
        assert!(citation.partial);
        assert_source_chunks(
            &plan.work_items[0].window.fields["work"][index]["content"],
            &action.text,
        );
    }
    let results = semantic_results(
        &plan,
        "no_opportunity",
        &["useful_opportunity", "no_opportunity", "uncertain"],
    );
    assert_eq!(results.len(), 1);
    let reduced = check.reduce(&plan, &results, true).unwrap();
    assert_eq!(
        reduced.decisions[0].outcome,
        SkillOpportunityOutcome::Uncertain
    );
    assert!(!reduced.complete);
    check
        .record_sampling_result(&mut sampling, &job, &reduced)
        .unwrap();
    assert!(
        sampling
            .completed_ids(check.sampling_identity())
            .contains(&job.candidate)
    );
}

#[test]
fn scope_descriptor_resume_keeps_all_groups_and_hydrates_only_selection() {
    let check = scope_check(300);
    let capabilities = ModelCapabilities::jev_default();
    let mut inventory = ScopeDescriptorInventory::default();
    check.enumerate_descriptors(&mut inventory).unwrap();
    assert!(inventory.groups.len() <= 256);
    assert!(!inventory.complete);
    assert!(
        inventory
            .groups
            .iter()
            .all(|group| group.window_ids.is_empty() && group.task_scope.is_empty())
    );
    let first = inventory.groups.clone();
    let partial_candidates = check
        .descriptor_candidates(&inventory, &capabilities)
        .unwrap();
    let partial_selection = BTreeSet::from([partial_candidates[0].id]);
    let partial_plan = check
        .prepare_descriptors(&inventory, &capabilities, &partial_selection)
        .unwrap();
    assert!(
        partial_plan
            .coverage
            .limitations
            .iter()
            .any(|limit| limit == "descriptor_enumeration_incomplete")
    );
    assert!(
        check
            .reduce(&partial_plan, &[], false)
            .unwrap()
            .coverage
            .processing_limit_reached
    );
    let mut inventory: ScopeDescriptorInventory =
        serde_json::from_slice(&serde_json::to_vec(&inventory).unwrap()).unwrap();
    while !inventory.complete {
        check.enumerate_descriptors(&mut inventory).unwrap();
    }
    assert_eq!(inventory.groups.len(), 300);
    assert_eq!(&inventory.groups[..first.len()], &first);
    let candidates = check
        .descriptor_candidates(&inventory, &capabilities)
        .unwrap();
    let selected = candidates
        .iter()
        .rev()
        .take(4)
        .map(|candidate| candidate.id)
        .collect();
    let plan = check
        .prepare_descriptors(&inventory, &capabilities, &selected)
        .unwrap();
    assert_eq!(plan.prepared.groups.len(), 4);
    assert_eq!(plan.work_items.len(), 4);
    assert_eq!(plan.coverage.not_selected_items, 296);
    for candidate in ScopeCreepCheck::sampling_candidates(&plan) {
        assert_eq!(
            &candidate,
            candidates
                .iter()
                .find(|item| item.id == candidate.id)
                .unwrap()
        );
    }
    let reduced = check.reduce(&plan, &[], false).unwrap();
    assert_eq!(reduced.remaining_candidates, 300);
    let mut altered = plan.clone();
    altered.prepared.groups[0].context.clear();
    assert!(matches!(
        check.reduce(&altered, &[], false),
        Err(JevError::InvalidCheckPlan)
    ));
}

#[test]
fn scope_unavailable_descriptor_remains_eligible_and_can_be_terminal() {
    let (mut content, scope, boundary) = source(1);
    content
        .actions
        .iter_mut()
        .filter(|action| action.kind == "tool_input")
        .for_each(|action| action.reference.stable = false);
    let check = ScopeCreepCheck::new(ScopeCreepInput {
        content,
        scope,
        boundary,
        source_generation: 3,
        ignored_instruction_work_ids: BTreeSet::new(),
    })
    .unwrap();
    let mut inventory = ScopeDescriptorInventory::default();
    check.enumerate_descriptors(&mut inventory).unwrap();
    let capabilities = ModelCapabilities::jev_default();
    let candidates = check
        .descriptor_candidates(&inventory, &capabilities)
        .unwrap();
    assert_eq!(candidates.len(), 1);
    let mut sampling = SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: 4096,
        answers_per_candidate: 1,
        judgments_per_run: 4,
    })
    .unwrap();
    sampling
        .synchronize_ordered(
            ScopeCreepCheck::check_identity(),
            check.semantic_epoch(&capabilities).unwrap(),
            &candidates,
            &ScopeCreepCheck::descriptor_chronology(&inventory),
        )
        .unwrap();
    sampling.begin_run();
    let job = sampling.choose_job().unwrap();
    let plan = check
        .prepare_descriptors(&inventory, &capabilities, &BTreeSet::from([job.candidate]))
        .unwrap();
    assert!(plan.work_items.is_empty());
    assert_eq!(plan.skipped_item_ids.len(), 1);
    sampling.terminate_candidate(&job).unwrap();
    assert_eq!(
        sampling.runnable_count(ScopeCreepCheck::check_identity()),
        0
    );
    assert!(
        sampling
            .completed_ids(ScopeCreepCheck::check_identity())
            .is_empty()
    );
    assert_eq!(inventory.groups.len(), 1);
}

#[test]
fn skill_descriptor_resume_preserves_product_and_source_chronology() {
    let check = skill_check(80, 4);
    assert_eq!(check.descriptor_count(), 320);
    let mut inventory = SkillDescriptorInventory::default();
    check.enumerate_descriptors(&mut inventory).unwrap();
    assert!(inventory.descriptors.len() <= 256);
    assert!(!inventory.complete);
    let first = inventory.descriptors.clone();
    let mut inventory: SkillDescriptorInventory =
        serde_json::from_slice(&serde_json::to_vec(&inventory).unwrap()).unwrap();
    while !inventory.complete {
        check.enumerate_descriptors(&mut inventory).unwrap();
    }
    assert_eq!(inventory.descriptors.len(), 320);
    assert_eq!(&inventory.descriptors[..first.len()], &first);
    let candidates = check.descriptor_candidates(&inventory).unwrap();
    let chronology = check.descriptor_chronology(&inventory).unwrap();
    assert_eq!(
        chronology,
        candidates
            .iter()
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>()
    );
    let mut sampling = SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: 4096,
        answers_per_candidate: 1,
        judgments_per_run: 4,
    })
    .unwrap();
    sampling
        .synchronize_ordered(
            check.sampling_identity(),
            check.sampling_epoch(),
            &candidates,
            &chronology,
        )
        .unwrap();
    sampling.begin_run();
    let jobs = (0..4)
        .map(|_| sampling.choose_job().unwrap())
        .collect::<Vec<_>>();
    let plan = check
        .prepare_inventory_sampled(
            &inventory,
            &check.session_context(),
            &ModelCapabilities::jev_default(),
            &jobs,
        )
        .unwrap();
    assert_eq!(plan.prepared.comparisons.len(), 4);
    assert_eq!(plan.coverage.not_selected_items, 316);
    let result = check.reduce(&plan, &[], false).unwrap();
    assert!(!result.complete);
    let results = pack_work_items_with_shared_context(
        &plan.work_items,
        &plan.capabilities,
        plan.shared_context.as_ref().unwrap(),
    )
    .batches
    .iter()
    .flat_map(|batch| {
        unpack_jev_response(
            batch,
            &JevResponse {
                model: plan.capabilities.model.clone(),
                usage: JevUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                },
                answers: batch
                    .answer_owners
                    .keys()
                    .map(|key| {
                        (
                            key.clone(),
                            JevAnswer::Choice {
                                choice: "uncertain".into(),
                                confidence: 1.0,
                                probabilities: [
                                    ("useful_opportunity".into(), 0.0),
                                    ("specialist_check".into(), 0.0),
                                    ("no_opportunity".into(), 0.0),
                                    ("already_covered".into(), 0.0),
                                    ("uncertain".into(), 1.0),
                                ]
                                .into(),
                            },
                        )
                    })
                    .collect(),
            },
        )
        .unwrap()
    })
    .collect::<Vec<_>>();
    let accepted = check.reduce(&plan, &results, true).unwrap();
    for job in &jobs {
        check
            .record_sampling_result(&mut sampling, job, &accepted)
            .unwrap();
    }
    assert_eq!(sampling.completed_ids(check.sampling_identity()).len(), 4);
    sampling
        .synchronize_ordered(
            check.sampling_identity(),
            check.sampling_epoch(),
            &candidates,
            &chronology,
        )
        .unwrap();
    assert_eq!(sampling.completed_ids(check.sampling_identity()).len(), 4);
    assert_eq!(sampling.runnable_count(check.sampling_identity()), 316);
    check.enumerate_descriptors(&mut inventory).unwrap();
    assert_eq!(inventory.descriptors.len(), 320);
}

#[test]
fn invalid_skill_descriptor_indices_and_missing_accumulated_entries_are_rejected() {
    let check = skill_check(1, 2);
    let mut inventory = SkillDescriptorInventory::default();
    check.enumerate_descriptors(&mut inventory).unwrap();
    let mut invalid = inventory.clone();
    invalid.descriptors[0].1 = usize::MAX;
    assert!(matches!(
        check.descriptor_chronology(&invalid),
        Err(JevError::InvalidCheckContext)
    ));
    invalid = inventory;
    invalid.descriptors.remove(0);
    assert!(matches!(
        check.descriptor_candidates(&invalid),
        Err(JevError::InvalidCheckContext)
    ));
}

#[test]
fn changed_sources_reset_descriptor_cursor_and_empty_skill_product_completes() {
    let mut scope_inventory = ScopeDescriptorInventory::default();
    scope_check(2)
        .enumerate_descriptors(&mut scope_inventory)
        .unwrap();
    scope_check(1)
        .enumerate_descriptors(&mut scope_inventory)
        .unwrap();
    assert_eq!(scope_inventory.groups.len(), 1);
    assert!(scope_inventory.complete);
    let mut skill_inventory = SkillDescriptorInventory::default();
    skill_check(2, 2)
        .enumerate_descriptors(&mut skill_inventory)
        .unwrap();
    let empty = skill_check(1, 0);
    empty.enumerate_descriptors(&mut skill_inventory).unwrap();
    assert!(skill_inventory.descriptors.is_empty());
    assert!(skill_inventory.complete);
    assert!(
        empty
            .descriptor_candidates(&skill_inventory)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn scope_unrelated_work_append_keeps_exact_descriptor_answer_identity() {
    let (content, scope, boundary) = source(1);
    let original = ScopeCreepCheck::new(ScopeCreepInput {
        content: content.clone(),
        scope: scope.clone(),
        boundary: boundary.clone(),
        source_generation: 3,
        ignored_instruction_work_ids: BTreeSet::new(),
    })
    .unwrap();
    let mut appended = content.clone();
    for action in content
        .actions
        .iter()
        .filter(|action| action.kind == "tool_input" || action.kind == "tool_result")
    {
        let mut action = action.clone();
        action.reference.id = format!("appended-{}", action.reference.id);
        action.reference.turn_index += 10;
        action.tool_call_id = Some("appended-call".into());
        appended.actions.push(action);
    }
    let mut boundary = boundary;
    boundary.turn_index += 10;
    let appended = ScopeCreepCheck::new(ScopeCreepInput {
        content: appended,
        scope,
        boundary,
        source_generation: 3,
        ignored_instruction_work_ids: BTreeSet::new(),
    })
    .unwrap();
    let mut before = ScopeDescriptorInventory::default();
    let mut after = ScopeDescriptorInventory::default();
    original.enumerate_descriptors(&mut before).unwrap();
    appended.enumerate_descriptors(&mut after).unwrap();
    let capabilities = ModelCapabilities::jev_default();
    assert_ne!(before.source_revision, after.source_revision);
    assert_eq!(
        original.semantic_epoch(&capabilities).unwrap(),
        appended.semantic_epoch(&capabilities).unwrap()
    );
    let before = original
        .descriptor_candidates(&before, &capabilities)
        .unwrap();
    let after = appended
        .descriptor_candidates(&after, &capabilities)
        .unwrap();
    assert_eq!(after.len(), 2);
    assert_eq!(before[0], after[0]);
}
