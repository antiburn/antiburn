use std::collections::BTreeSet;
use std::sync::Arc;

use antiburn_local::analysis::jev::capabilities::ModelCapabilities;
use antiburn_local::analysis::jev::{JevCheck, JevError};
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
    ScopeCreepCheck, ScopeCreepInput, ScopeDescriptorInventory,
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
                if index % 2 == 0 {
                    r#"{"command":"cargo test worker_shutdown"}"#
                } else {
                    r#"{"command":"cargo test lock_order"}"#
                },
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
        description: "1. Run `cargo test worker_shutdown`.\n2. Run `cargo test lock_order`.".into(), frontmatter: serde_json::json!({"description": "1. Run `cargo test worker_shutdown`.\n2. Run `cargo test lock_order`."}), scope: skill_scope.clone(), enabled: true, created_at_ms: None,
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

#[test]
fn oversized_scope_edit_is_skipped_without_claiming_assessment() {
    let body = format!(
        "early\n{}\nworker_shutdown cancellation\n{}\nmiddle\n{}\nlate\n",
        "filler 新\n".repeat(2500),
        "filler 新\n".repeat(2500),
        "filler 新\n".repeat(5000)
    );
    let request = serde_json::json!({"file_path": "src/worker.rs", "content": body}).to_string();
    let (content, scope, boundary) = large_source("Write", &request, &body);
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
    assert!(plan.work_items.is_empty());
    assert_eq!(plan.coverage.skipped_items, 1);
    assert!(plan.coverage.processing_limit_reached);
    assert!(!plan.coverage.limitations.is_empty());
    assert!(plan.prepared.groups[0].window_ids.is_empty());
    let reduced = check.reduce(&plan, &[], true).unwrap();
    assert!(reduced.findings.is_empty());
    assert_eq!(reduced.assessed_candidates, 0);
    assert!(reduced.coverage.processing_limit_reached);
}

#[test]
fn oversized_scope_command_is_skipped_without_claiming_assessment() {
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
    assert!(plan.work_items.is_empty());
    assert_eq!(plan.coverage.skipped_items, 1);
    assert!(!plan.coverage.limitations.is_empty());
    let reduced = check.reduce(&plan, &[], true).unwrap();
    assert!(reduced.findings.is_empty());
    assert_eq!(reduced.assessed_candidates, 0);
    assert!(reduced.coverage.processing_limit_reached);
}

#[test]
fn selected_large_skill_work_is_safely_skipped_with_source_bound_partial_citations() {
    let body = format!(
        "early\n{}\nworker_shutdown cancellation\n{}\nlate\n",
        "filler 新\n".repeat(3000),
        "filler 新\n".repeat(5000)
    );
    let request =
        serde_json::json!({"command": "cargo test worker_shutdown\ncargo test lock_order"})
            .to_string();
    let (content, scope, _) = large_source("Bash", &request, &body);
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
    assert!(plan.work_items.is_empty());
    assert_eq!(plan.coverage.skipped_items, 1);
    assert!(plan.coverage.processing_limit_reached);
    assert_eq!(plan.skipped_item_ids.len(), 1);
    let comparison = &plan.prepared.comparisons[0];
    assert_eq!(comparison.work.len(), 4);
    assert!(comparison.work_context_assessable);
    for citation in &comparison.work {
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
        if citation.partial {
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
        } else {
            assert_eq!(citation.ranges, vec![(0, action.text.len())]);
        }
    }
    let reduced = check.reduce(&plan, &[], true).unwrap();
    assert!(reduced.findings.is_empty());
    assert!(
        reduced
            .decisions
            .iter()
            .all(|decision| { decision.outcome == SkillOpportunityOutcome::Unassessed })
    );
    assert!(!reduced.complete);
}

#[test]
fn scope_descriptor_resume_keeps_all_groups_and_hydrates_only_selection() {
    let check = scope_check(300);
    let capabilities = ModelCapabilities::jev_default();
    let mut inventory = ScopeDescriptorInventory::default();
    check.enumerate_descriptors(&mut inventory).unwrap();
    assert_eq!(inventory.groups.len(), 2);
    assert!(inventory.complete);
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
        !partial_plan
            .coverage
            .limitations
            .iter()
            .any(|limit| limit == "descriptor_enumeration_incomplete")
    );
    let inventory: ScopeDescriptorInventory =
        serde_json::from_slice(&serde_json::to_vec(&inventory).unwrap()).unwrap();
    assert_eq!(inventory.groups.len(), 2);
    assert_eq!(&inventory.groups[..first.len()], &first);
    let candidates = check
        .descriptor_candidates(&inventory, &capabilities)
        .unwrap();
    let selected = candidates
        .iter()
        .rev()
        .take(1)
        .map(|candidate| candidate.id)
        .collect();
    let plan = check
        .prepare_descriptors(&inventory, &capabilities, &selected)
        .unwrap();
    assert_eq!(plan.prepared.groups.len(), 1);
    assert_eq!(plan.work_items.len(), 0);
    assert_eq!(plan.coverage.skipped_items, 1);
    assert!(plan.coverage.processing_limit_reached);
    assert!(plan.prepared.groups[0].window_ids.is_empty());
    assert_eq!(plan.coverage.not_selected_items, 1);
    let reduced = check.reduce(&plan, &[], false).unwrap();
    assert_eq!(reduced.remaining_candidates, candidates.len());
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
fn skill_descriptor_serialization_preserves_single_candidate_chronology() {
    let check = skill_check(80, 4);
    assert_eq!(check.descriptor_count(), 1);
    let mut inventory = SkillDescriptorInventory::default();
    check.enumerate_descriptors(&mut inventory).unwrap();
    assert_eq!(inventory.descriptors.len(), 1);
    assert!(inventory.complete);
    let first = inventory.descriptors.clone();
    let mut inventory: SkillDescriptorInventory =
        serde_json::from_slice(&serde_json::to_vec(&inventory).unwrap()).unwrap();
    assert_eq!(inventory.descriptors.len(), 1);
    assert_eq!(&inventory.descriptors[..first.len()], &first);
    let candidates = check.descriptor_candidates(&inventory).unwrap();
    assert_eq!(candidates.len(), check.descriptor_count());
    assert!(!candidates.is_empty());
    let chronology = check.descriptor_chronology(&inventory).unwrap();
    assert_eq!(
        chronology,
        candidates
            .iter()
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>()
    );
    check.enumerate_descriptors(&mut inventory).unwrap();
    assert_eq!(inventory.descriptors.len(), 1);
    assert_eq!(check.descriptor_candidates(&inventory).unwrap(), candidates);
    assert_eq!(check.descriptor_chronology(&inventory).unwrap(), chronology);
}

#[test]
fn invalid_skill_descriptor_indices_and_missing_accumulated_entries_are_rejected() {
    let check = skill_check(2, 2);
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
        action.kind = "other_tool_input".into();
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
    assert_eq!(after.len(), 1);
    assert_eq!(before[0], after[0]);
}
