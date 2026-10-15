use super::*;
use crate::analysis::jev::{
    JevRunProgress, pack_work_items_with_capabilities, unpack_jev_response,
};
use crate::analysis::jev_evidence::prepare_session_content;
use crate::analysis::session_scope::{
    SessionScopeBoundary, SessionScopeBranch, SessionScopeBuilder,
};
use crate::analysis::{
    ContentKind, ContentPart, ContentQueryCoverage, PublishedContent, PublishedContentPart,
    SourceFormat,
};
use crate::checks::skill_opportunities::{SKILL_USE_SELECTION, SkillScope, SkillUseBoundary};
use crate::model::AgentKind;

fn scope() -> SkillScope {
    SkillScope {
        agent: AgentKind::Claude,
        project_identity: Some("project".into()),
        environment_identity: "native".into(),
    }
}

fn skill() -> SkillDefinition {
    let description = "1. Run `cargo test parser`.\n2. Run `cargo check`, inspect failures, and check lock ordering before changing code.\n";
    SkillDefinition {
        identity: "parser-review".into(),
        revision: "definition".into(),
        name: "parser-review".into(),
        aliases: vec![],
        description: description.into(),
        frontmatter: json!({"description": description}),
        scope: scope(),
        enabled: true,
        created_at_ms: Some(10000),
    }
}

fn part(index: u64, kind: ContentKind, text: &str, call: Option<&str>) -> PublishedContentPart {
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
            .with_tool_identity(call.map(|_| "Bash".into()), call.map(str::to_owned)),
        context_only: false,
        stable_event_identity: true,
    }
}

fn parts() -> Vec<PublishedContentPart> {
    vec![
        part(
            0,
            ContentKind::UserText,
            "Investigate the parser failure.",
            None,
        ),
        part(
            1,
            ContentKind::ToolInput,
            r#"{"command":"cargo test parser"}"#,
            Some("test"),
        ),
        part(2, ContentKind::ToolResult, "Tests failed.", Some("test")),
        part(
            3,
            ContentKind::ToolInput,
            r#"{"command":"cargo check"}"#,
            Some("check"),
        ),
        part(4, ContentKind::ToolResult, "Check passed.", Some("check")),
    ]
}

fn check(
    mut parts: Vec<PublishedContentPart>,
    skills: Vec<SkillDefinition>,
    partial: bool,
) -> SkillOpportunitiesCheck {
    parts.push(part(100, ContentKind::Thinking, "private marker", None));
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
    let usage = SkillUseSnapshot::from_selected_content(
        &select_session_content(&content, SKILL_USE_SELECTION),
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

fn response(
    batch: &crate::analysis::jev::JevRequestBatch,
    choice: &str,
    probability: f64,
) -> JevResponse {
    JevResponse {
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
                        probabilities: [
                            "actionable_help",
                            "already_adequate_or_unrelated",
                            "uncertain",
                        ]
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
    }
}

fn results(
    plan: &JevCheckPlan<PreparedSkillOpportunities>,
    choice: &str,
    probability: f64,
) -> Vec<JevWorkItemResult> {
    pack_work_items_with_capabilities(&plan.work_items, &plan.capabilities)
        .batches
        .iter()
        .flat_map(|batch| {
            unpack_jev_response(batch, &response(batch, choice, probability)).unwrap()
        })
        .collect()
}

#[test]
fn complete_workflow_publishes_current_procedure_with_exact_bindings() {
    let definition = skill();
    let check = check(parts(), vec![definition.clone()], false);
    let plan = check.prepare(&check.session_context()).unwrap();
    assert_eq!(plan.work_items.len(), 1, "{:?}", plan.coverage);
    assert!(skill_request_fits(&plan.work_items[0], &plan.capabilities));
    let result = check
        .reduce(&plan, &results(&plan, "actionable_help", 1.0), true)
        .unwrap();
    assert_eq!(result.findings.len(), 1);
    assert!(
        result.findings[0]
            .comparison
            .skill
            .matches_definition(&definition)
    );
    assert_eq!(result.findings[0].comparison.work.len(), 4);
    assert!(!result.findings[0].absence_limit.contains("was available"));
    let restored: SkillOpportunityFinding =
        serde_json::from_value(json!(&result.findings[0])).unwrap();
    assert_eq!(restored, result.findings[0]);
}

#[test]
fn threshold_unknown_and_missing_answers_remain_distinct() {
    let check = check(parts(), vec![skill()], false);
    let plan = check.prepare(&check.session_context()).unwrap();
    for (choice, probability, expected) in [
        ("actionable_help", 0.95, SkillOpportunityOutcome::Advisory),
        ("actionable_help", 0.94, SkillOpportunityOutcome::Uncertain),
        (
            "already_adequate_or_unrelated",
            0.99,
            SkillOpportunityOutcome::NoOpportunity,
        ),
        ("uncertain", 1.0, SkillOpportunityOutcome::Uncertain),
    ] {
        let result = check
            .reduce(&plan, &results(&plan, choice, probability), true)
            .unwrap();
        assert_eq!(result.decisions[0].outcome, expected);
    }
    let result = check.reduce(&plan, &[], false).unwrap();
    assert!(!result.complete);
    assert_eq!(
        result.decisions[0].outcome,
        SkillOpportunityOutcome::Unassessed
    );
    let mut forged = plan.clone();
    forged.prepared.comparisons[0]
        .skill
        .description
        .push_str("invented");
    assert!(
        check
            .reduce(&forged, &results(&plan, "actionable_help", 1.0), true)
            .is_err()
    );
    let mut bad = results(&plan, "actionable_help", 1.0);
    bad[0].evidence.clear();
    assert!(check.reduce(&plan, &bad, true).is_err());
}

#[tokio::test]
async fn minimum_fit_and_incomplete_evidence_make_zero_calls() {
    use crate::analysis::jev::capabilities::{CapabilityLimit, CapabilitySource};
    let mut caps = ModelCapabilities::jev_default();
    caps.runtime_context_tokens = CapabilityLimit::known(32, CapabilitySource::RuntimeMetadata);
    for (mut source, partial) in [(parts(), false), (parts(), true)] {
        if partial {
            source.remove(2);
        }
        let check = check(source, vec![skill()], partial);
        let outcome = crate::analysis::jev::run_jev_check_with_capabilities(
            &check,
            &check.session_context(),
            JevRunProgress::default(),
            caps.clone(),
            |_| async { panic!("invalid evidence must not dispatch") },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert!(outcome.result.findings.is_empty());
        assert!(outcome.progress.results.is_empty());
        assert!(!outcome.result.complete);
    }
}

#[tokio::test]
async fn context_rejection_stops_without_a_second_skill_request() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let check = check(parts(), vec![skill()], false);
    let calls = AtomicUsize::new(0);
    let outcome = crate::analysis::jev::run_jev_check(
        &check,
        &check.session_context(),
        JevRunProgress::default(),
        |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(JevError::ContextRejected) }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!outcome.complete);
    assert!(outcome.result.findings.is_empty());
    assert!(
        outcome
            .result
            .decisions
            .iter()
            .all(|decision| decision.outcome == SkillOpportunityOutcome::Unassessed)
    );
}

#[test]
fn rendered_fit_keeps_unicode_procedure_and_respects_runtime_and_scoring_reserves() {
    use crate::analysis::jev::capabilities::{
        CapabilityLimit, CapabilitySource, ModelLimitOverride,
    };
    let mut definition = skill();
    definition
        .description
        .push_str("Keep the \"café\" and 🦀 labels when changing locks.\n");
    definition.frontmatter = json!({"description": definition.description});
    let check = check(parts(), vec![definition], false);
    let mut caps = ModelCapabilities::jev_default();
    caps.use_ollama_tev1_accounting("chat envelope", "system text");
    caps.runtime_context_tokens = CapabilityLimit::known(2050, CapabilitySource::RuntimeMetadata);
    let plan = check
        .prepare_with_capabilities(&check.session_context(), &caps)
        .unwrap();
    assert_eq!(plan.work_items.len(), 1, "{:?}", plan.coverage);
    let packed = pack_work_items_with_capabilities(&plan.work_items, &caps);
    assert_eq!(packed.batches.len(), 1);
    let batch = &packed.batches[0];
    assert!(batch.serialized_bytes <= 4096);
    assert!(
        crate::analysis::jev::validate_jev_request_with_capabilities(&batch.request, &caps).is_ok()
    );
    let rendered =
        crate::analysis::jev::estimate_jev_rendered_question_tokens(&batch.request, &caps).unwrap();
    assert!(rendered <= 1024);
    assert!(rendered <= caps.usable_input_tokens().unwrap());
    assert!(rendered <= caps.usable_state_tokens().unwrap());
    assert!(batch.request.state.to_string().contains("café"));
    assert!(batch.request.state.to_string().contains("🦀"));
    for limits in [
        ModelLimitOverride {
            runtime_context_tokens: Some(128),
            ..Default::default()
        },
        ModelLimitOverride {
            total_input_tokens: Some(128),
            ..Default::default()
        },
        ModelLimitOverride {
            state_and_longest_question_tokens: Some(128),
            ..Default::default()
        },
    ] {
        assert!(
            check
                .prepare_with_capabilities(&check.session_context(), &caps.with_override(&limits))
                .unwrap()
                .work_items
                .is_empty()
        );
    }
}

#[test]
fn routine_command_and_generic_inventory_do_not_create_a_product() {
    let mut inventory = Vec::new();
    for index in 0..512 {
        let mut definition = skill();
        definition.identity = format!("skill-{index}");
        definition.name = format!("skill-{index}");
        definition.description = "Generic testing help.".into();
        definition.frontmatter = json!({"description": definition.description});
        inventory.push(definition);
    }
    let check = check(parts()[..3].to_vec(), inventory, false);
    assert_eq!(check.descriptor_count(), 0);
    let mut inventory = SkillDescriptorInventory::default();
    check.enumerate_descriptors(&mut inventory).unwrap();
    assert!(inventory.complete);
    assert!(inventory.descriptors.is_empty());
}

#[tokio::test]
async fn ambiguous_identity_and_recorded_equivalent_or_unknown_use_make_zero_calls() {
    let mut duplicate = skill();
    duplicate.identity = "other-definition".into();
    let ambiguous = check(parts(), vec![skill(), duplicate], false);
    let mut checks = vec![ambiguous];
    for (identity, definitions) in [
        (json!({"skill":"parser-review"}), vec![skill()]),
        (json!({"skill":"equivalent-review"}), {
            let mut equivalent = skill();
            equivalent.identity = "equivalent-review".into();
            equivalent.name = "equivalent-review".into();
            vec![skill(), equivalent]
        }),
        (json!({}), vec![skill()]),
    ] {
        let mut source = parts();
        let mut used = part(
            5,
            ContentKind::ToolInput,
            &identity.to_string(),
            Some("skill-use"),
        );
        used.part = used
            .part
            .with_tool_identity(Some("Skill".into()), Some("skill-use".into()));
        source.push(used);
        checks.push(check(source, definitions, false));
    }
    for check in checks {
        assert_eq!(check.descriptor_count(), 0);
        let outcome = crate::analysis::jev::run_jev_check(
            &check,
            &check.session_context(),
            JevRunProgress::default(),
            |_| async { panic!("use or identity uncertainty must block dispatch") },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert!(outcome.result.findings.is_empty());
        assert!(!outcome.result.complete);
    }
}

#[test]
fn capability_constraints_do_not_split_or_clip_procedures() {
    use crate::analysis::jev::capabilities::{CapabilityLimit, CapabilitySource};
    let check = check(parts(), vec![skill()], false);
    let normal = check.prepare(&check.session_context()).unwrap();
    let packing = pack_work_items_with_capabilities(&normal.work_items, &normal.capabilities);
    let bytes = packing.batches[0].serialized_bytes;
    let mut caps = normal.capabilities.clone();
    caps.request_body_bytes = CapabilityLimit::known((bytes - 1) as u64, CapabilitySource::Manual);
    let reduced = check
        .prepare_with_capabilities(&check.session_context(), &caps)
        .unwrap();
    assert_eq!(reduced.work_items.len(), 1, "{:?}", reduced.coverage);
    assert_eq!(reduced.prepared.comparisons, normal.prepared.comparisons);
    assert!(skill_request_fits(&reduced.work_items[0], &caps));
    let reduced_bytes =
        pack_work_items_with_capabilities(&reduced.work_items, &caps).batches[0].serialized_bytes;
    assert!(reduced_bytes < bytes);
    caps.questions_per_request.value = Some(0);
    assert!(
        check
            .prepare_with_capabilities(&check.session_context(), &caps)
            .unwrap()
            .work_items
            .is_empty()
    );
    caps.questions_per_request.value = Some(1);
    caps.criteria_per_question.value = Some(2);
    assert!(
        check
            .prepare_with_capabilities(&check.session_context(), &caps)
            .unwrap()
            .work_items
            .is_empty()
    );
}

#[test]
fn policy_omitted_pairs_do_not_become_clean_or_page_on_later_turns() {
    let mut other = skill();
    other.identity = "other-review".into();
    other.name = "other-review".into();
    let check = check(parts(), vec![skill(), other], false);
    assert_eq!(check.descriptor_count(), 1);
    assert_eq!(check.omitted_comparison_count(), 1);
    let plan = check.prepare(&check.session_context()).unwrap();
    assert_eq!(plan.work_items.len(), 1);
    assert_eq!(plan.coverage.not_selected_items, 1);
    let result = check
        .reduce(
            &plan,
            &results(&plan, "already_adequate_or_unrelated", 1.0),
            true,
        )
        .unwrap();
    assert!(result.findings.is_empty());
    assert!(!result.complete);
    let mut sampling = SamplingProgress::new(crate::checks::sampling::SamplingLimits {
        checks: 1,
        candidates_per_check: 1,
        answers_per_candidate: 1,
        judgments_per_run: 1,
    })
    .unwrap();
    check.synchronize_sampling(&mut sampling).unwrap();
    sampling.begin_run();
    let job = sampling.choose_job().unwrap();
    check
        .record_sampling_result(&mut sampling, &job, &result)
        .unwrap();
    sampling.begin_run();
    assert!(sampling.choose_job().is_none());
}

#[test]
fn repeated_workflow_keeps_both_bindings_when_only_one_example_fits() {
    let mut source = parts();
    for (index, command, call) in [
        (5, "cargo test parser", "test-again"),
        (7, "cargo check", "check-again"),
    ] {
        source.push(part(
            index,
            ContentKind::ToolInput,
            &json!({"command":command}).to_string(),
            Some(call),
        ));
        source.push(part(
            index + 1,
            ContentKind::ToolResult,
            "Repeated outcome.",
            Some(call),
        ));
    }
    let check = check(source, vec![skill()], false);
    let plan = check.prepare(&check.session_context()).unwrap();
    assert_eq!(plan.work_items.len(), 1, "{:?}", plan.coverage);
    assert_eq!(plan.work_items[0].window.fields["occurrences"], 2);
    assert_eq!(plan.prepared.comparisons[0].work.len(), 8);
    let result = check
        .reduce(&plan, &results(&plan, "actionable_help", 1.0), true)
        .unwrap();
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].comparison.work.len(), 8);
}

#[test]
fn single_complete_multiline_operation_can_offer_a_procedure() {
    let command = "cargo test parser\ncargo check";
    let mut definition = skill();
    let procedure_command = command
        .lines()
        .map(|line| format!("   {line}\n"))
        .collect::<String>();
    definition.description = format!(
        "1. Run this recorded workflow:\n   ```sh\n{procedure_command}   ```\n2. Inspect lock ordering and add a deadlock regression before accepting changes.\n"
    );
    definition.frontmatter = json!({"description": definition.description});
    let check = check(
        vec![
            part(
                0,
                ContentKind::UserText,
                "Investigate the parser deadlock.",
                None,
            ),
            part(
                1,
                ContentKind::ToolInput,
                &json!({"command":command}).to_string(),
                Some("workflow"),
            ),
            part(
                2,
                ContentKind::ToolResult,
                "Tests failed; check passed.",
                Some("workflow"),
            ),
        ],
        vec![definition],
        false,
    );
    let plan = check.prepare(&check.session_context()).unwrap();
    assert_eq!(
        plan.work_items.len(),
        1,
        "{:?} {:?}",
        plan.coverage,
        plan.prepared
    );
    assert_eq!(
        check
            .reduce(&plan, &results(&plan, "actionable_help", 1.0), true)
            .unwrap()
            .findings
            .len(),
        1
    );
}

#[test]
fn exact_path_and_command_anchors_bind_one_complete_workflow() {
    let mut definition = skill();
    definition.description = "1. Inspect `src/parser.rs`.\n2. Run `cargo test parser`, record failures, and inspect boundary conditions.\n".into();
    definition.frontmatter = json!({"description": definition.description});
    let mut read = part(
        1,
        ContentKind::ToolInput,
        r#"{"file_path":"src/parser.rs","offset":1,"limit":3}"#,
        Some("read"),
    );
    read.part = read
        .part
        .with_tool_identity(Some("Read".into()), Some("read".into()));
    let mut output = part(
        2,
        ContentKind::ToolResult,
        "Parser source returned.",
        Some("read"),
    );
    output.part = output
        .part
        .with_tool_identity(Some("Read".into()), Some("read".into()));
    let source = vec![
        part(
            0,
            ContentKind::UserText,
            "Investigate the parser failure.",
            None,
        ),
        read,
        output,
        part(
            3,
            ContentKind::ToolInput,
            r#"{"command":"cargo test parser"}"#,
            Some("test"),
        ),
        part(4, ContentKind::ToolResult, "Tests failed.", Some("test")),
    ];
    let check = check(source, vec![definition], false);
    let plan = check.prepare(&check.session_context()).unwrap();
    assert_eq!(plan.work_items.len(), 1, "{:?}", plan.coverage);
    assert_eq!(
        check
            .reduce(&plan, &results(&plan, "actionable_help", 1.0), true)
            .unwrap()
            .findings
            .len(),
        1
    );
}

#[tokio::test]
async fn accepted_response_resume_sends_no_second_request() {
    let check = check(parts(), vec![skill()], false);
    let first = crate::analysis::jev::run_jev_check(
        &check,
        &check.session_context(),
        JevRunProgress::default(),
        |batch| async move { Ok(response(&batch, "actionable_help", 1.0)) },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(first.result.findings.len(), 1);
    let resumed = crate::analysis::jev::run_jev_check(
        &check,
        &check.session_context(),
        first.progress,
        |_| async { panic!("accepted answer must be reused") },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(resumed.result.findings.len(), 1);
}

#[tokio::test]
async fn stored_plan_recovers_accepted_answers_after_capability_shrink() {
    let check = check(parts(), vec![skill()], false);
    let context = check.session_context();
    let mut plan = check.prepare(&context).unwrap();
    let first = crate::analysis::jev::run_jev_check_prepared(
        &check,
        &context,
        &mut plan,
        JevRunProgress::default(),
        crate::analysis::jev::admit_jev_orchestration()
            .await
            .unwrap(),
        |batch| async move { Ok(response(&batch, "actionable_help", 1.0)) },
        |_| Ok(()),
    )
    .await
    .unwrap();
    let stored = serde_json::to_vec(&plan).unwrap();
    let mut changed = plan.capabilities.clone();
    changed.runtime_context_tokens.value = Some(1);
    assert!(
        check
            .prepare_with_capabilities(&context, &changed)
            .unwrap()
            .work_items
            .is_empty()
    );
    let mut restored: JevCheckPlan<PreparedSkillOpportunities> =
        serde_json::from_slice(&stored).unwrap();
    let recovered = crate::analysis::jev::run_jev_check_prepared(
        &check,
        &context,
        &mut restored,
        first.progress,
        crate::analysis::jev::admit_jev_orchestration()
            .await
            .unwrap(),
        |_| async { panic!("accepted request must not be sent again") },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(recovered.result.findings.len(), 1);
    assert_eq!(restored.capabilities, plan.capabilities);
}
