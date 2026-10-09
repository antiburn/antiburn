use std::collections::BTreeMap;

use antiburn_local::analysis::ignored_instructions::{
    AssessmentInput, IgnoredInstructionsCheck, InstructionProvenance, InstructionScope,
    SamplingLedger, build_jev_context_with_capabilities, prepare_session_content,
    snapshot_from_text,
};
use antiburn_local::analysis::jev::capabilities::{
    CapabilityLimit, CapabilitySource, ModelCapabilities,
};
use antiburn_local::analysis::jev::*;
use antiburn_local::analysis::{
    ContentKind, ContentPart, PublishedContent, PublishedContentPart, SourceFormat,
};

#[test]
fn offline_2050_instruction_request_and_explanation_use_exact_selected_text() {
    for tokens in [2048, 2050] {
        let mut limits = ModelCapabilities::jev_default();
        limits.total_input_tokens = CapabilityLimit::known(tokens, CapabilitySource::Manual);
        limits.runtime_context_tokens.value = Some(tokens);
        limits.rendering_reserve_tokens = 1024;
        let command = "git commit -m 'fix: login'";
        let rule = "Every commit must include a DCO sign-off. Use git commit -s.";
        let content = prepare_session_content(
            "synthetic",
            SourceFormat::ClaudeJsonl,
            PublishedContent {
                publication_fence: 1,
                parts: vec![PublishedContentPart {
                    source_key: "transcript".into(),
                    thread_id: "main".into(),
                    turn_index: 1,
                    role: "assistant",
                    scope: "main".into(),
                    ts_ms: Some(1),
                    uuid: Some("request".into()),
                    message_id: None,
                    part_index: 0,
                    part: ContentPart::new(
                        ContentKind::ToolInput,
                        serde_json::json!({"command": command}).to_string(),
                    )
                    .with_tool_identity(Some("Bash".into()), Some("commit".into())),
                    context_only: false,
                    stable_event_identity: true,
                }],
                ..Default::default()
            },
            vec![
                snapshot_from_text(
                    "AGENTS.md",
                    rule.into(),
                    InstructionProvenance::RecordedInjection,
                    InstructionScope::Project,
                )
                .unwrap(),
            ],
        );
        let input = AssessmentInput {
            content,
            prior_history_complete: false,
            activity_after_ms: None,
            boundary_positions: BTreeMap::new(),
            source_generation: 1,
            source_fingerprint: None,
            incarnation: 1,
            comparison_after: None,
        };
        let context =
            build_jev_context_with_capabilities(&input, &SamplingLedger::default(), &limits)
                .unwrap();
        let plan = IgnoredInstructionsCheck
            .prepare_with_capabilities(&context, &limits)
            .unwrap();
        let packed = pack_work_items_with_capabilities(&plan.work_items, &limits);
        assert!(!packed.batches.is_empty(), "{:?}", plan.work_items);
        assert!(packed.skipped_item_ids.is_empty());
        let results = plan
            .work_items
            .iter()
            .map(|item| JevWorkItemResult {
                request_id: item.id.clone(),
                work_item_id: item.id.clone(),
                answers: item
                    .questions
                    .keys()
                    .map(|key| {
                        (
                            key.clone(),
                            JevAnswer::Choice {
                                choice: "conflict".into(),
                                probabilities: BTreeMap::from([
                                    ("conflict".into(), 0.97),
                                    ("no_issue".into(), 0.01),
                                    ("pending_completion".into(), 0.01),
                                    ("uncertain".into(), 0.01),
                                ]),
                                confidence: 0.97,
                            },
                        )
                    })
                    .collect(),
                evidence: item.window.evidence.clone(),
                model: limits.model.clone(),
                usage: JevUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            })
            .collect::<Vec<_>>();
        for batch in packed.batches {
            assert!(validate_jev_request_with_capabilities(&batch.request, &limits).is_ok());
            assert_eq!(
                serde_json::to_string(&batch.request)
                    .unwrap()
                    .matches("git commit -m")
                    .count(),
                1
            );
        }
        let result = IgnoredInstructionsCheck
            .reduce(&plan, &results, true)
            .unwrap();
        let finding = &result.findings[0];
        let basis = finding
            .decision_record()
            .unwrap()
            .explanation_basis
            .as_ref()
            .unwrap();
        assert_eq!(basis.instruction, finding.instruction_excerpt);
        assert_eq!(basis.action, finding.action_excerpt);
    }
}
