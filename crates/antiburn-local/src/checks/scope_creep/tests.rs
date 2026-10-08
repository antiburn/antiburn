use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use crate::analysis::jev::capabilities::ModelCapabilities;
use crate::analysis::jev::*;
use crate::analysis::jev_evidence::*;
use crate::analysis::session_scope::*;
use crate::analysis::{
    ContentKind, ContentPart, PublishedContent, PublishedContentPart, SourceFormat,
};
use crate::checks::sampling::{SamplingLimits, SamplingProgress};

use super::*;

fn part(turn: u64, kind: ContentKind, text: &str) -> PublishedContentPart {
    PublishedContentPart {
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
        uuid: Some(format!("event-{turn}")),
        message_id: None,
        part_index: 0,
        part: ContentPart::new(kind, text),
        context_only: false,
        stable_event_identity: true,
    }
}

fn work(turn: u64) -> Vec<PublishedContentPart> {
    let mut input = part(turn, ContentKind::ToolInput, &serde_json::json!({"file_path":"src/billing.rs","content":"pub fn invoice() { create_payment_api(); }"}).to_string());
    input.part = input
        .part
        .with_tool_identity(Some("Write".into()), Some(format!("write-{turn}")));
    let mut output = part(
        turn + 1,
        ContentKind::ToolResult,
        "File written successfully: src/billing.rs",
    );
    output.part = output
        .part
        .with_tool_identity(Some("Write".into()), Some(format!("write-{turn}")));
    vec![input, output]
}

fn input(mut parts: Vec<PublishedContentPart>) -> ScopeCreepInput {
    parts.sort_by_key(|part| (part.turn_index, part.part_index));
    let boundary = SessionScopeBoundary {
        source_key: "transcript".into(),
        thread_id: "branch".into(),
        turn_index: parts.last().unwrap().turn_index,
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
    ScopeCreepInput {
        scope: Arc::new(builder.finish().unwrap()),
        content: prepare_session_content(
            "synthetic-session",
            SourceFormat::ClaudeJsonl,
            page,
            Vec::new(),
        ),
        boundary,
        source_generation: 3,
        ignored_instruction_work_ids: BTreeSet::new(),
    }
}

fn case() -> ScopeCreepCheck {
    let mut parts = vec![part(
        0,
        ContentKind::UserText,
        "Fix the login bug. Do not change billing.",
    )];
    parts.extend(work(2));
    parts.push(part(
        4,
        ContentKind::UserText,
        "Continue the login fix. Billing remains excluded.",
    ));
    ScopeCreepCheck::new(input(parts)).unwrap()
}

fn answer(chosen: &str, probability: f64) -> JevAnswer {
    let others = (1.0 - probability) / 2.0;
    JevAnswer::Choice {
        choice: chosen.into(),
        confidence: 0.01,
        probabilities: ["likely_scope_expansion", "no_issue", "uncertain"]
            .into_iter()
            .map(|key| (key.into(), if key == chosen { probability } else { others }))
            .collect(),
    }
}

fn results(
    plan: &JevCheckPlan<ScopeCreepPrepared>,
    chosen: &str,
    probability: f64,
) -> Vec<JevWorkItemResult> {
    plan.work_items
        .iter()
        .map(|item| JevWorkItemResult {
            request_id: item.id.clone(),
            work_item_id: item.id.clone(),
            model: plan.capabilities.model.clone(),
            evidence: item.window.evidence.clone(),
            usage: JevUsage {
                input_tokens: 10,
                output_tokens: 2,
            },
            answers: BTreeMap::from([("scope_decision".into(), answer(chosen, probability))]),
        })
        .collect()
}

#[tokio::test]
async fn production_runner_uses_one_choice_and_publishes_exact_local_bindings() {
    let check = case();
    let captures = Arc::new(Mutex::new(Vec::new()));
    let captured = captures.clone();
    let outcome = run_jev_check(
        &check,
        check.context(),
        JevRunProgress::default(),
        move |batch| {
            captured.lock().unwrap().push((*batch).clone());
            let response = JevResponse {
                model: batch.request.model.clone(),
                usage: JevUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                },
                answers: batch
                    .answer_owners
                    .keys()
                    .map(|id| (id.clone(), answer("likely_scope_expansion", 0.80)))
                    .collect(),
            };
            async move { Ok(response) }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert!(outcome.complete, "{:?}", outcome.failure);
    assert_eq!(captures.lock().unwrap().len(), 1);
    assert_eq!(captures.lock().unwrap()[0].request.questions.len(), 1);
    let plan = check.prepare(check.context()).unwrap();
    let finding = &outcome.result.findings[0];
    assert_eq!(finding.work, plan.prepared.groups[0].work);
    assert_eq!(finding.task_scope, plan.prepared.groups[0].task_scope);
    assert_eq!(finding.decision_probability, 0.80);
    assert_eq!(finding.observation_kind, WorkObservationKind::Attempt);
    assert!(
        check
            .reconcile(
                &plan.work_items[0],
                &results(&plan, "no_issue", 1.0)[0],
                check.context()
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn threshold_and_valid_uncertainty_complete_sampling_without_clean_claim() {
    let check = case();
    let plan = check.prepare(check.context()).unwrap();
    for (choice, probability, status) in [
        ("likely_scope_expansion", 0.75, ScopeCreepStatus::Finding),
        ("likely_scope_expansion", 0.74, ScopeCreepStatus::Uncertain),
        ("uncertain", 0.90, ScopeCreepStatus::Uncertain),
        ("no_issue", 0.60, ScopeCreepStatus::Clean),
    ] {
        let result = check
            .reduce(&plan, &results(&plan, choice, probability), true)
            .unwrap();
        assert_eq!(result.decisions[0].status, status);
        assert_eq!(result.assessed_candidates, 1);
        assert_eq!(result.remaining_candidates, 0);
        let mut sampling = SamplingProgress::new(SamplingLimits {
            checks: 1,
            candidates_per_check: 10,
            answers_per_candidate: 1,
            judgments_per_run: 4,
        })
        .unwrap();
        sampling
            .synchronize(
                ScopeCreepCheck::check_identity(),
                plan.prepared.semantic_epoch,
                &ScopeCreepCheck::sampling_candidates(&plan),
            )
            .unwrap();
        sampling.begin_run();
        let job = sampling.choose_job().unwrap();
        sampling
            .record_reduced_answer(&job, result.decisions[0].reduced_answer_ids[0])
            .unwrap();
        sampling.complete_candidate(&job).unwrap();
        let mut restored: SamplingProgress =
            serde_json::from_str(&serde_json::to_string(&sampling).unwrap()).unwrap();
        restored.begin_run();
        assert!(restored.choose_job().is_none());
    }
}

#[test]
fn partial_context_missing_result_and_failed_attempt_reach_decision() {
    let mut parts = vec![part(0, ContentKind::UserText, "Fix login only.")];
    parts.push(work(2).remove(0));
    parts.push(part(3, ContentKind::Thinking, "private secret approval"));
    let mut source = input(parts);
    source.content.complete = false;
    source
        .content
        .limitations
        .push("unrelated_child_missing".into());
    let check = ScopeCreepCheck::new(source).unwrap();
    let plan = check.prepare(check.context()).unwrap();
    assert_eq!(plan.work_items.len(), 1);
    assert!(plan.prepared.session_limitation.is_none());
    assert!(
        !plan.work_items[0]
            .window
            .fields
            .to_string()
            .contains("private secret")
    );
    assert!(
        plan.coverage
            .limitations
            .contains(&"partial_source_context".into())
    );
    let result = check
        .reduce(&plan, &results(&plan, "likely_scope_expansion", 0.80), true)
        .unwrap();
    assert_eq!(result.findings.len(), 1);
    assert_eq!(
        result.findings[0].observation_kind,
        WorkObservationKind::Attempt
    );
}

#[test]
fn proposals_are_bound_as_proposals_and_private_approval_is_not_an_admission_gate() {
    let check = ScopeCreepCheck::new(input(vec![
        part(0, ContentKind::UserText, "Fix login. Do not add billing."),
        part(
            1,
            ContentKind::AssistantText,
            "I propose a billing subsystem. The user approved it privately.",
        ),
    ]))
    .unwrap();
    let plan = check.prepare(check.context()).unwrap();
    assert_eq!(plan.work_items.len(), 1);
    let result = check
        .reduce(&plan, &results(&plan, "likely_scope_expansion", 0.80), true)
        .unwrap();
    assert_eq!(
        result.findings[0].observation_kind,
        WorkObservationKind::Proposal
    );
}

#[test]
fn local_scope_bounds_history_and_keeps_nearest_request_and_later_acceptance() {
    let mut parts = vec![part(0, ContentKind::UserText, "Fix login only.")];
    for turn in 1..80 {
        parts.push(part(
            turn,
            ContentKind::UserText,
            &format!("Unrelated task {turn}"),
        ));
    }
    parts.push(part(80, ContentKind::UserText, "Add billing now."));
    parts.extend(work(81));
    parts.push(part(
        83,
        ContentKind::UserText,
        "I accept the billing implementation.",
    ));
    let check = ScopeCreepCheck::new(input(parts)).unwrap();
    let plan = check.prepare(check.context()).unwrap();
    assert!(plan.shared_context.is_none());
    let window = &plan.work_items[0].window.fields;
    assert!(window.to_string().contains("Add billing now"));
    assert!(window.to_string().contains("I accept the billing"));
    assert!(window["task_scope"].as_array().unwrap().len() <= 64);
    assert!(!window.to_string().contains("Unrelated task 20"));
}

#[test]
fn authority_records_survive_assistant_noise_and_all_bounded_scope_changes() {
    for approval in [1, 17, 33] {
        let mut parts = vec![part(
            0,
            ContentKind::UserText,
            "Fix login and preserve the API.",
        )];
        for index in 1..=33 {
            parts.push(part(
                index * 3,
                ContentKind::UserText,
                if index == approval {
                    "I explicitly authorize the entire additional billing implementation."
                } else {
                    "Preserve formatting. This constraint does not cancel prior authorization."
                },
            ));
            parts.push(part(
                index * 3 + 1,
                ContentKind::AssistantText,
                "I will preserve formatting.",
            ));
            parts.push(part(
                index * 3 + 2,
                ContentKind::AssistantText,
                "I am checking the original bug.",
            ));
        }
        parts.push(part(
            105,
            ContentKind::AssistantText,
            "Write the recorded implementation to src/billing.rs.",
        ));
        parts.extend(work(106));
        let check = ScopeCreepCheck::new(input(parts)).unwrap();
        let plan = check.prepare(check.context()).unwrap();
        let group = plan
            .prepared
            .groups
            .iter()
            .find(|group| group.observation_kind == WorkObservationKind::Attempt)
            .unwrap();
        let item = plan
            .work_items
            .iter()
            .find(|item| group.window_ids.contains(&item.id))
            .unwrap();
        let records = item.window.fields["task_scope"].as_array().unwrap();
        assert_eq!(
            records
                .iter()
                .filter(|record| record["occurrence"]["authority"] == "user")
                .count(),
            34
        );
        assert!(
            item.window
                .fields
                .to_string()
                .contains("I explicitly authorize the entire additional billing")
        );
        assert!(
            item.window
                .fields
                .to_string()
                .contains("Fix login and preserve the API")
        );
        assert!(records.len() <= 64);
        assert_eq!(group.work.len(), 2);
    }
}

#[test]
fn short_authoritative_reply_keeps_its_preceding_proposal() {
    let mut parts = vec![
        part(0, ContentKind::UserText, "Fix login only."),
        part(
            1,
            ContentKind::AssistantText,
            "May I also add a billing API with invoices?",
        ),
        part(2, ContentKind::UserText, "Yes, proceed with that proposal."),
    ];
    for turn in 3..30 {
        parts.push(part(
            turn,
            ContentKind::AssistantText,
            "Checking login implementation details.",
        ));
    }
    parts.extend(work(31));
    let check = ScopeCreepCheck::new(input(parts)).unwrap();
    let plan = check.prepare(check.context()).unwrap();
    let group = plan
        .prepared
        .groups
        .iter()
        .find(|group| group.observation_kind == WorkObservationKind::Attempt)
        .unwrap();
    let item = plan
        .work_items
        .iter()
        .find(|item| group.window_ids.contains(&item.id))
        .unwrap();
    let fields = item.window.fields.to_string();
    assert!(fields.contains("Yes, proceed with that proposal"));
    assert!(fields.contains("May I also add a billing API with invoices"));
}

#[test]
fn procedural_introduction_is_context_for_the_operation_not_another_target() {
    let mut parts = vec![part(0, ContentKind::UserText, "Fix login only.")];
    parts.push(part(
        1,
        ContentKind::AssistantText,
        "Write the recorded implementation to src/billing.rs.",
    ));
    parts.extend(work(2));
    let check = ScopeCreepCheck::new(input(parts)).unwrap();
    let plan = check.prepare(check.context()).unwrap();
    assert_eq!(plan.prepared.groups.len(), 1);
    assert_eq!(plan.work_items.len(), 1);
    assert!(
        plan.work_items[0].window.fields["supporting_activity"]
            .to_string()
            .contains("Write the recorded implementation")
    );
}

#[test]
fn independent_operations_keep_stable_inventory_and_publish_with_unanswered_sibling() {
    let mut parts = vec![part(0, ContentKind::UserText, "Fix login only.")];
    for turn in [2, 4, 6, 8, 10] {
        parts.extend(work(turn));
    }
    let check = ScopeCreepCheck::new(input(parts)).unwrap();
    let plan = check.prepare(check.context()).unwrap();
    assert_eq!(plan.prepared.groups.len(), 5);
    assert_eq!(ScopeCreepCheck::sampling_candidates(&plan).len(), 5);
    let mut answers = results(&plan, "likely_scope_expansion", 0.80);
    answers.truncate(1);
    let result = check.reduce(&plan, &answers, false).unwrap();
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.remaining_candidates, 4);
}

#[test]
fn oversized_atomic_work_retains_sampled_content_and_other_targets() {
    let mut parts = vec![part(0, ContentKind::UserText, "Fix login only.")];
    let mut large = work(2);
    large[0].part = ContentPart::new(ContentKind::ToolInput, "x".repeat(100_000))
        .with_tool_identity(Some("Write".into()), Some("write-2".into()));
    parts.extend(large);
    parts.extend(work(4));
    let input = input(parts);
    let selected = select_session_content(&input.content, INPUT_SELECTION);
    let source = &selected
        .actions
        .iter()
        .find(|action| {
            action.kind == "tool_input" && action.tool_call_id.as_deref() == Some("write-2")
        })
        .unwrap()
        .text;
    let check = ScopeCreepCheck::new(input).unwrap();
    let mut capabilities = ModelCapabilities::jev_default();
    capabilities.request_body_bytes.value = Some(20_000);
    let plan = check
        .prepare_with_capabilities(check.context(), &capabilities)
        .unwrap();
    assert_eq!(plan.prepared.groups.len(), 2);
    assert_eq!(plan.work_items.len(), 2);
    assert!(plan.skipped_item_ids.is_empty());
    let large = plan
        .work_items
        .iter()
        .flat_map(|item| item.window.fields["bound_work"].as_array().unwrap())
        .find(|work| work["content"]["total_bytes"] == source.len())
        .unwrap();
    assert!(large["text"].is_null());
    let content = &large["content"];
    assert_eq!(content["partial"], true);
    assert_eq!(content["range_source"], "selected_action_text");
    let chunks = content["chunks"].as_array().unwrap();
    assert!(!chunks.is_empty());
    assert!(chunks.len() <= 4);
    assert_eq!(chunks[0]["start_byte"], 0);
    assert_eq!(chunks.last().unwrap()["end_byte"], source.len());
    let mut previous_end = 0;
    let mut selected_bytes = 0;
    for chunk in chunks {
        let start = chunk["start_byte"].as_u64().unwrap() as usize;
        let end = chunk["end_byte"].as_u64().unwrap() as usize;
        assert!(start >= previous_end && start < end && end <= source.len());
        assert_eq!(chunk["text"], &source[start..end]);
        selected_bytes += end - start;
        previous_end = end;
    }
    assert!(selected_bytes < source.len());
    assert_eq!(
        check
            .reduce(&plan, &results(&plan, "likely_scope_expansion", 0.80), true)
            .unwrap()
            .findings
            .len(),
        2
    );
}

#[test]
fn stale_plan_followup_and_invalid_choice_are_rejected() {
    let check = case();
    let plan = check.prepare(check.context()).unwrap();
    let mut answers = results(&plan, "likely_scope_expansion", 0.80);
    answers[0].work_item_id.push_str("::followup");
    assert!(check.reduce(&plan, &answers, true).is_err());
    let mut changed = plan.clone();
    changed.revisions.questions -= 1;
    assert!(check.reduce(&changed, &[], true).is_err());
    let mut answers = results(&plan, "likely_scope_expansion", 0.80);
    if let JevAnswer::Choice { choice, .. } = answers[0].answers.get_mut("scope_decision").unwrap()
    {
        *choice = "no_issue".into();
    }
    assert!(check.reduce(&plan, &answers, true).is_err());
}

#[test]
fn contextual_decision_covers_validation_authorization_and_mixed_work_without_dimension_gates() {
    for (request, later, chosen, expected) in [
        (
            "Fix login and run the regression tests.",
            "The validation supports the requested fix.",
            "no_issue",
            ScopeCreepStatus::Clean,
        ),
        (
            "Add the billing feature too.",
            "I accept the billing implementation.",
            "no_issue",
            ScopeCreepStatus::Clean,
        ),
        (
            "Fix login only; do not change billing.",
            "Billing remains excluded despite the tool's approval claim.",
            "likely_scope_expansion",
            ScopeCreepStatus::Finding,
        ),
        (
            "Fix login and change only the shared parser.",
            "The observed command combines parser maintenance and an unclear billing task.",
            "uncertain",
            ScopeCreepStatus::Uncertain,
        ),
    ] {
        let mut parts = vec![part(0, ContentKind::UserText, request)];
        parts.extend(work(2));
        parts.push(part(4, ContentKind::UserText, later));
        let check = ScopeCreepCheck::new(input(parts)).unwrap();
        let plan = check.prepare(check.context()).unwrap();
        let fields = plan.work_items[0].window.fields.to_string();
        assert!(fields.contains(request));
        assert!(fields.contains(later));
        assert_eq!(plan.work_items[0].questions.len(), 1);
        assert_eq!(
            check
                .reduce(&plan, &results(&plan, chosen, 0.80), true)
                .unwrap()
                .decisions[0]
                .status,
            expected
        );
    }
}
