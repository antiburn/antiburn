use std::collections::BTreeMap;

use serde_json::json;

use super::*;
use crate::analysis::jev::capabilities::ModelCapabilities;
use crate::analysis::jev::*;
use crate::analysis::jev_evidence::{
    JevOperationState, SessionContentEvidence, prepare_session_content,
};
use crate::analysis::session_scope::{
    SessionScopeBoundary, SessionScopeBranch, SessionScopeBuilder, SessionScopeSnapshot,
};
use crate::analysis::{
    ContentKind, ContentPart, PublishedContent, PublishedContentPart, SourceFormat,
};
use crate::checks::sampling::{SamplingLimits, SamplingProgress};

fn part(index: u64, kind: ContentKind, text: &str, call: Option<&str>) -> PublishedContentPart {
    let mut part = ContentPart::new(kind, text);
    if let Some(call) = call {
        part = part.with_tool_identity(Some("read".into()), Some(call.into()));
        part.metadata.state = JevOperationState::Completed;
    }
    PublishedContentPart {
        source_key: "transcript".into(),
        thread_id: "branch".into(),
        turn_index: index,
        role: if kind == ContentKind::UserText {
            "user"
        } else {
            "assistant"
        },
        scope: "main".into(),
        ts_ms: Some(i64::try_from(index).unwrap()),
        uuid: Some(format!("native-{index}")),
        message_id: None,
        part_index: 0,
        part,
        context_only: false,
        stable_event_identity: true,
    }
}

fn fixture(
    task: &str,
    paths: &[&str],
    later: &str,
) -> (SessionContentEvidence, SessionScopeSnapshot, EpisodeSpan) {
    let mut parts = vec![
        part(0, ContentKind::UserText, task, None),
        part(
            1,
            ContentKind::AssistantText,
            "Investigate the task and check the relevant dependencies.",
            None,
        ),
    ];
    for (index, path) in paths.iter().enumerate() {
        let order = u64::try_from(index * 2 + 2).unwrap();
        let call = format!("call-{index}");
        parts.push(part(
            order,
            ContentKind::ToolInput,
            &json!({"filePath": path, "limit": 2000}).to_string(),
            Some(&call),
        ));
        parts.push(part(order + 1, ContentKind::ToolResult, &format!("<path>{path}</path>\n<type>file</type>\n<content>\n1: recorded content\n\n(End of file - total 1 lines)\n</content>"), Some(&call)));
    }
    let end = parts.last().unwrap().turn_index;
    parts.push(part(end + 1, ContentKind::AssistantText, later, None));
    fixture_from_parts(parts, 1, usize::try_from(end).unwrap())
}

fn fixture_from_parts(
    parts: Vec<PublishedContentPart>,
    first: usize,
    last: usize,
) -> (SessionContentEvidence, SessionScopeSnapshot, EpisodeSpan) {
    let boundary = parts.last().unwrap().turn_index;
    let page = PublishedContent {
        publication_fence: 4,
        source_generation: Some(3),
        parts,
        ..Default::default()
    };
    let mut builder = SessionScopeBuilder::new(
        SourceFormat::OpenCodeSqliteV2,
        SessionScopeBoundary {
            source_key: "transcript".into(),
            thread_id: "branch".into(),
            turn_index: boundary,
            part_index: 0,
            branch: SessionScopeBranch::ProvenLinear,
        },
        4,
        3,
        true,
    )
    .unwrap();
    builder.push_page(page.clone(), false).unwrap();
    let task = builder.finish().unwrap();
    let mut content =
        prepare_session_content("session", SourceFormat::OpenCodeSqliteV2, page, Vec::new());
    content.complete = true;
    for event in &mut content.actions[..first] {
        event.context_only = true;
    }
    let span = EpisodeSpan {
        first_event_id: content.actions[first].reference.id.clone(),
        last_event_id: content.actions[last].reference.id.clone(),
        state: EpisodeState::Complete,
    };
    (content, task, span)
}

fn input(task_text: &str, paths: &[&str], later: &str) -> OverExploringInput {
    let (content, task, span) = fixture(task_text, paths, later);
    build_episodes(&content, &task, &[span]).unwrap()
}

fn plan(input: &OverExploringInput) -> JevCheckPlan<PreparedAssessment> {
    OverExploringCheck
        .prepare(&build_jev_context(input).unwrap())
        .unwrap()
}

fn answer(choice: &str) -> JevAnswer {
    JevAnswer::Choice {
        choice: choice.into(),
        confidence: 0.98,
        probabilities: BTreeMap::from([
            (
                "supported".into(),
                if choice == "supported" { 0.98 } else { 0.01 },
            ),
            (
                "justified".into(),
                if choice == "justified" { 0.98 } else { 0.01 },
            ),
            (
                "unknown".into(),
                if choice == "unknown" { 0.98 } else { 0.01 },
            ),
        ]),
    }
}

fn results(
    plan: &JevCheckPlan<PreparedAssessment>,
    positive: Option<Reason>,
) -> Vec<JevWorkItemResult> {
    plan.work_items
        .iter()
        .map(|item| {
            let target = &plan.prepared.targets[&item.id];
            let mut evidence = plan.shared_context.as_ref().unwrap().evidence.clone();
            evidence.extend(item.window.evidence.clone());
            JevWorkItemResult {
                request_id: "request".into(),
                work_item_id: item.id.clone(),
                model: plan.capabilities.model.clone(),
                usage: JevUsage {
                    input_tokens: 10,
                    output_tokens: 1,
                },
                evidence,
                answers: super::questions::GATES
                    .into_iter()
                    .map(|gate| {
                        (
                            gate.into(),
                            answer(choice_for(gate, target.reason, positive)),
                        )
                    })
                    .collect(),
            }
        })
        .collect()
}

fn choice_for(gate: &str, reason: Reason, positive: Option<Reason>) -> &'static str {
    if gate == "substantial" && positive != Some(reason)
        || gate == "relevance" && reason == Reason::ExcessiveWithinFileReading
    {
        "justified"
    } else {
        "supported"
    }
}

#[test]
fn native_read_shapes_reach_stable_episodes_with_observed_not_requested_ranges() {
    let input = input(
        "Fix the parser.",
        &["parser.rs", "library.rs"],
        "The investigation is complete.",
    );
    assert_eq!(input.episodes.len(), 1);
    let episode = &input.episodes[0];
    assert_eq!(episode.reads.len(), 2);
    assert_eq!(episode.before[0].text, "Fix the parser.");
    assert_eq!(episode.subsequent.len(), 1);
    assert_eq!(episode.reads[0].request.extent.offset, None);
    assert_eq!(episode.reads[0].request.extent.limit, Some(2000));
    assert_eq!(
        episode.reads[0]
            .result
            .as_ref()
            .unwrap()
            .returned_extent
            .as_ref()
            .unwrap()
            .limit,
        Some(1)
    );
    assert_eq!(episode.id, input.episodes[0].id);
    let plan = plan(&input);
    assert!(!plan.work_items.is_empty());
    let serialized = serde_json::to_string(&plan.work_items[0].window.fields).unwrap();
    assert!(!serialized.contains(&episode.reads[0].request.reference_id));
    assert!(!serialized.contains("whole_file"));
    assert!(serialized.contains("recorded content"));
}

#[test]
fn every_reason_publishes_only_its_exact_read_bindings() {
    let input = input(
        "Fix the parser.",
        &["parser.rs", "other.rs", "parser.rs"],
        "Investigation complete.",
    );
    let plan = plan(&input);
    for reason in [
        Reason::UnrelatedFiles,
        Reason::ExcessiveFileBreadth,
        Reason::ExcessiveWithinFileReading,
    ] {
        let reduced = OverExploringCheck
            .reduce(&plan, &results(&plan, Some(reason)), true)
            .unwrap();
        assert!(!reduced.findings.is_empty());
        assert!(
            reduced
                .findings
                .iter()
                .all(|finding| finding.reason == reason)
        );
        for finding in &reduced.findings {
            let target = &plan.prepared.targets[&finding.work_item_id];
            assert_eq!(finding.reads, target.bindings);
            assert_eq!(finding.episode_id, input.episodes[0].id);
            assert_eq!(finding.task_evidence, input.task_context.evidence);
            assert_eq!(finding.model, plan.capabilities.model);
            assert_eq!(finding.semantic_revision, plan.input_revision);
            for (binding, index) in finding.reads.iter().zip(&target.read_indexes) {
                let read = &input.episodes[0].reads[*index];
                assert_eq!(binding.request_id, read.request.reference_id);
                assert_eq!(
                    binding.result_id,
                    read.result.as_ref().unwrap().reference_id
                );
            }
        }
        assert_eq!(reduced.completed_episode_ids, vec![input.episodes[0].id]);
        assert!(reduced.clean_episode_ids.is_empty());
    }
}

#[test]
fn every_semantic_gate_is_required_and_unknown_is_not_clean() {
    let plan = plan(&input("Fix the parser.", &["parser.rs"], "Complete."));
    for gate in [
        "relevance",
        "useful_information",
        "later_use",
        "substantial",
        "sufficiency",
    ] {
        for choice in ["justified", "unknown"] {
            let mut results = results(&plan, Some(Reason::UnrelatedFiles));
            for result in &mut results {
                result.answers.insert(gate.into(), answer(choice));
            }
            let assessment = OverExploringCheck.reduce(&plan, &results, true).unwrap();
            assert!(assessment.findings.is_empty(), "{gate}: {choice}");
            if choice == "unknown" || gate == "sufficiency" {
                assert!(assessment.clean_episode_ids.is_empty());
                assert!(!assessment.unassessed.is_empty());
            }
        }
    }
}

#[test]
fn diligence_negative_controls_survive_all_reason_gates() {
    let cases = [
        (
            "Audit all repository files for security risks.",
            "The risk audit needs this evidence.",
            "useful_information",
        ),
        (
            "Fix the parser dependency failure.",
            "Discovered an imported dependency and used its contract.",
            "useful_information",
        ),
        (
            "Find the failing code path.",
            "These reads eliminate the initial hypothesis.",
            "useful_information",
        ),
        (
            "Change the cross-cutting API.",
            "All callers require review.",
            "useful_information",
        ),
        (
            "Verify concurrent file changes.",
            "Reread the file after its version changed.",
            "useful_information",
        ),
        (
            "Fix the small file.",
            "The one-line file supplies needed surrounding context.",
            "substantial",
        ),
        (
            "Explain the failure without edits.",
            "The final explanation uses the returned text.",
            "later_use",
        ),
    ];
    for (task, later, gate) in cases {
        let input = input(task, &["parser.rs", "dependency.rs", "parser.rs"], later);
        let plan = plan(&input);
        for reason in [
            Reason::UnrelatedFiles,
            Reason::ExcessiveFileBreadth,
            Reason::ExcessiveWithinFileReading,
        ] {
            let mut outcomes = results(&plan, Some(reason));
            for outcome in &mut outcomes {
                outcome.answers.insert(gate.into(), answer("justified"));
            }
            let reduced = OverExploringCheck.reduce(&plan, &outcomes, true).unwrap();
            assert!(reduced.findings.is_empty(), "{task}: {reason:?}");
            assert_eq!(reduced.clean_episode_ids, vec![input.episodes[0].id]);
            let fields = serde_json::to_string(&plan.work_items[0].window.fields).unwrap();
            assert!(fields.contains(later));
            assert!(
                serde_json::to_string(&plan.shared_context)
                    .unwrap()
                    .contains(task)
            );
        }
    }
}

#[test]
fn counts_ranges_and_no_edit_cannot_override_a_semantic_negative() {
    let input = input(
        "Audit dependencies.",
        &["parser.rs"; 12],
        "No changes are needed.",
    );
    let plan = plan(&input);
    assert!(
        !plan
            .prepared
            .targets
            .values()
            .any(|target| target.reason == Reason::ExcessiveFileBreadth)
    );
    let reduced = OverExploringCheck
        .reduce(&plan, &results(&plan, None), true)
        .unwrap();
    assert!(reduced.findings.is_empty());
    assert_eq!(reduced.clean_episode_ids.len(), 1);
}

#[test]
fn incomplete_failed_truncated_directory_and_deferred_reads_abstain_without_calls() {
    for variant in [
        "history",
        "deferred",
        "missing",
        "failed",
        "status",
        "directory",
        "truncated",
        "request",
        "before",
        "after",
    ] {
        let mut input = input("Fix the parser.", &["parser.rs"], "Complete.");
        let episode = &mut input.episodes[0];
        match variant {
            "history" => input.complete = false,
            "deferred" => episode.state = EpisodeState::Deferred,
            "missing" => episode.reads[0].result = None,
            "failed" => {
                episode.reads[0].result.as_mut().unwrap().status =
                    crate::analysis::jev_evidence::JevReadStatus::Failed
            }
            "status" => {
                episode.reads[0].result.as_mut().unwrap().status =
                    crate::analysis::jev_evidence::JevReadStatus::Unknown
            }
            "directory" => {
                episode.reads[0].result.as_mut().unwrap().kind =
                    crate::analysis::jev_evidence::JevReadResultKind::Directory
            }
            "truncated" => episode.reads[0].result.as_mut().unwrap().truncated = true,
            "request" => episode.reads[0].request.truncated = true,
            "before" => episode.before[0].truncated = true,
            "after" => episode.subsequent[0].truncated = true,
            _ => unreachable!(),
        }
        let plan = plan(&input);
        assert!(plan.work_items.is_empty(), "{variant}");
        let reduced = OverExploringCheck.reduce(&plan, &[], true).unwrap();
        assert!(reduced.findings.is_empty());
        assert!(reduced.clean_episode_ids.is_empty());
        assert!(!reduced.unassessed.is_empty());
    }
}

#[test]
fn unknown_extent_never_inherits_requested_limits_or_completes_episode() {
    let mut input = input("Fix the parser.", &["parser.rs"], "Complete.");
    input.episodes[0].reads[0]
        .result
        .as_mut()
        .unwrap()
        .returned_extent = None;
    let plan = plan(&input);
    assert!(
        plan.prepared
            .targets
            .values()
            .all(|target| target.reason != Reason::ExcessiveWithinFileReading)
    );
    let reduced = OverExploringCheck
        .reduce(&plan, &results(&plan, None), true)
        .unwrap();
    assert!(reduced.clean_episode_ids.is_empty());
    assert!(reduced.completed_episode_ids.is_empty());
    assert!(
        reduced
            .unassessed
            .iter()
            .any(|item| item.limitation == Abstention::UnknownObservedExtent)
    );
}

#[test]
fn interrupted_execution_preserves_supported_targets_without_completing_episode() {
    let plan = plan(&input(
        "Fix the parser.",
        &["parser.rs", "other.rs"],
        "Complete.",
    ));
    let outcomes = results(&plan, Some(Reason::UnrelatedFiles));
    for (outcomes, complete) in [(&outcomes[..1], true), (&outcomes[..], false)] {
        let reduced = OverExploringCheck
            .reduce(&plan, outcomes, complete)
            .unwrap();
        assert!(!reduced.findings.is_empty());
        assert!(reduced.clean_episode_ids.is_empty());
        assert!(reduced.completed_episode_ids.is_empty());
    }
}

#[test]
fn mismatched_models_evidence_work_ids_and_distributions_are_rejected() {
    let plan = plan(&input("Fix the parser.", &["parser.rs"], "Complete."));
    for variant in [
        "model",
        "source",
        "role",
        "work",
        "duplicate",
        "missing",
        "nan",
        "sum",
        "confidence",
    ] {
        let mut results = results(&plan, Some(Reason::UnrelatedFiles));
        match variant {
            "model" => results[0].model = "other-model".into(),
            "source" => results[0].evidence[0].source_id = "another-task".into(),
            "role" => results[0].evidence[0].role = JevEvidenceRole::Candidate,
            "work" => results[0].work_item_id = "different-work".into(),
            "duplicate" => results.push(results[0].clone()),
            "missing" => {
                results[0].answers.remove("relevance");
            }
            "nan" | "sum" | "confidence" => {
                let JevAnswer::Choice {
                    probabilities,
                    confidence,
                    ..
                } = results[0].answers.get_mut("relevance").unwrap()
                else {
                    unreachable!()
                };
                if variant == "confidence" {
                    *confidence = 2.0;
                } else {
                    probabilities.insert(
                        "supported".into(),
                        if variant == "nan" { f64::NAN } else { 0.5 },
                    );
                }
            }
            _ => unreachable!(),
        }
        assert!(
            OverExploringCheck.reduce(&plan, &results, true).is_err(),
            "{variant}"
        );
    }
}

#[test]
fn builder_rejects_wrong_branch_order_call_result_digest_and_overlapping_spans() {
    for variant in [
        "branch",
        "order",
        "call",
        "digest",
        "bytes",
        "result_id",
        "request_id",
        "unstable",
        "thinking",
        "publication",
        "overlap",
    ] {
        let (mut content, task, span) = fixture("Fix the parser.", &["parser.rs"], "Complete.");
        let mut spans = vec![span.clone()];
        match variant {
            "branch" => content.actions[2].reference.thread_digest = "sibling".into(),
            "order" => content.actions.swap(2, 3),
            "call" => content.actions[3].tool_call_id = Some("different-call".into()),
            "digest" => {
                content.actions[3]
                    .metadata
                    .read_result
                    .as_mut()
                    .unwrap()
                    .recorded_output_digest = "wrong".into()
            }
            "bytes" => {
                content.actions[3]
                    .metadata
                    .read_result
                    .as_mut()
                    .unwrap()
                    .recorded_output_bytes = 0
            }
            "result_id" => {
                content.actions[3]
                    .metadata
                    .read_result
                    .as_mut()
                    .unwrap()
                    .reference_id = "wrong".into()
            }
            "request_id" => {
                content.actions[2]
                    .metadata
                    .read_request
                    .as_mut()
                    .unwrap()
                    .reference_id = "wrong".into()
            }
            "unstable" => content.actions[2].reference.stable = false,
            "thinking" => content.actions[1].kind = "thinking".into(),
            "publication" => content.publication_fence += 1,
            "overlap" => spans.push(span),
            _ => unreachable!(),
        }
        assert_eq!(
            build_episodes(&content, &task, &spans),
            Err(JevError::InvalidCheckContext),
            "{variant}"
        );
    }
}

#[test]
fn changed_task_results_and_later_use_invalidate_semantics_not_episode_identity() {
    let initial = input("Fix the parser.", &["parser.rs"], "Complete.");
    let first = plan(&initial);
    for variant in ["task", "output", "later"] {
        let mut changed = initial.clone();
        match variant {
            "task" => {
                changed.task_context.fields["values"][0] = json!("Audit the whole repository.")
            }
            "output" => {
                changed.episodes[0].reads[0]
                    .result
                    .as_mut()
                    .unwrap()
                    .recorded_output_digest = "new-output".into()
            }
            "later" => {
                changed.episodes[0].subsequent[0].text = "The explanation uses these reads.".into()
            }
            _ => unreachable!(),
        }
        let second = plan(&changed);
        assert_eq!(
            first.prepared.candidates[0].episode_id,
            second.prepared.candidates[0].episode_id
        );
        assert_ne!(
            first.prepared.candidates[0].required_answers,
            second.prepared.candidates[0].required_answers
        );
        assert!(
            OverExploringCheck
                .reduce(
                    &second,
                    &results(&first, Some(Reason::UnrelatedFiles)),
                    true
                )
                .is_err()
        );
    }
}

#[test]
fn sampler_restart_completion_and_context_invalidation_use_shared_progress() {
    let input = input("Fix the parser.", &["parser.rs"], "Complete.");
    let mut plan = plan(&input);
    let mut progress = SamplingProgress::new(SamplingLimits {
        checks: 4,
        candidates_per_check: 256,
        answers_per_candidate: 1024,
        judgments_per_run: 1,
    })
    .unwrap();
    synchronize_sampling(&plan, &mut progress).unwrap();
    progress.begin_run();
    let job = progress.choose_job().unwrap();
    PreparedAssessment::select_jobs(&mut plan, std::slice::from_ref(&job)).unwrap();
    let partial = OverExploringCheck.reduce(&plan, &[], false).unwrap();
    assert!(
        plan.prepared
            .record_completion(&partial, &job, &mut progress)
            .is_err()
    );
    let encoded = serde_json::to_vec(&progress).unwrap();
    progress = serde_json::from_slice(&encoded).unwrap();
    let reduced = OverExploringCheck
        .reduce(&plan, &results(&plan, None), true)
        .unwrap();
    plan.prepared
        .record_completion(&reduced, &job, &mut progress)
        .unwrap();
    assert_eq!(progress.coverage(job.check).unwrap().completed, 1);
    assert!(progress.choose_job().is_none());
    let mut changed = input;
    changed.task_context.fields["values"][0] = json!("Audit all files.");
    let changed = super::tests::plan(&changed);
    synchronize_sampling(&changed, &mut progress).unwrap();
    assert_eq!(progress.coverage(job.check).unwrap().completed, 0);
    assert!(progress.choose_job().is_none());
    progress.begin_run();
    assert_eq!(progress.choose_job().unwrap().candidate, job.candidate);
}

#[test]
fn capability_fit_keeps_full_task_and_abstains_instead_of_clipping() {
    let input = input(
        &format!(
            "Do not change billing. {}",
            "ordered task context ".repeat(200)
        ),
        &["parser.rs"],
        "Complete.",
    );
    let context = build_jev_context(&input).unwrap();
    let large = OverExploringCheck.prepare(&context).unwrap();
    assert!(!large.work_items.is_empty());
    let mut small = ModelCapabilities::jev_default();
    small.request_body_bytes.value = Some(1024);
    let small = OverExploringCheck
        .prepare_with_capabilities(&context, &small)
        .unwrap();
    assert!(small.work_items.is_empty());
    assert_eq!(small.shared_context, large.shared_context);
    assert!(
        small
            .prepared
            .unassessed
            .iter()
            .any(|item| item.limitation == Abstention::ContextTooLarge)
    );
    let packed = pack_work_items_with_shared_context(
        &large.work_items,
        &large.capabilities,
        large.shared_context.as_ref().unwrap(),
    );
    assert!(!packed.batches.is_empty());
    for batch in packed.batches {
        assert_eq!(
            batch.request.state["shared_context"],
            input.task_context.fields
        );
    }
}

#[tokio::test]
async fn shared_runner_packs_validates_and_reduces_all_three_reasons_offline() {
    let input = input("Fix the parser.", &["parser.rs", "other.rs"], "Complete.");
    let context = build_jev_context(&input).unwrap();
    for reason in [
        Reason::UnrelatedFiles,
        Reason::ExcessiveFileBreadth,
        Reason::ExcessiveWithinFileReading,
    ] {
        let outcome = run_jev_check(
            &OverExploringCheck,
            &context,
            JevRunProgress::default(),
            move |batch| async move {
                validate_jev_request_with_capabilities(
                    &batch.request,
                    &ModelCapabilities::jev_default(),
                )?;
                let answers = batch
                    .request
                    .questions
                    .keys()
                    .map(|id| {
                        let (work, gate) = &batch.answer_owners[id];
                        let index = batch
                            .work_item_ids
                            .iter()
                            .position(|item| item == work)
                            .unwrap();
                        let item_reason: Reason = serde_json::from_value(
                            batch.request.state["work_items"][index]["context"]["reason"].clone(),
                        )
                        .unwrap();
                        (
                            id.clone(),
                            answer(choice_for(gate, item_reason, Some(reason))),
                        )
                    })
                    .collect();
                Ok(JevResponse {
                    model: batch.request.model.clone(),
                    answers,
                    usage: JevUsage {
                        input_tokens: 10,
                        output_tokens: 1,
                    },
                })
            },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert!(outcome.complete, "{:?}", outcome.failure);
        assert!(!outcome.result.findings.is_empty());
        assert!(
            outcome
                .result
                .findings
                .iter()
                .all(|finding| finding.reason == reason)
        );
    }
}

#[tokio::test]
async fn runner_failure_preserves_abstention_and_never_completes_or_cleans() {
    let context =
        build_jev_context(&input("Fix the parser.", &["parser.rs"], "Complete.")).unwrap();
    let outcome = run_jev_check(
        &OverExploringCheck,
        &context,
        JevRunProgress::default(),
        |_| async { Err(JevError::ProviderUnavailable) },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert!(!outcome.complete);
    assert!(!outcome.progress.failed_item_ids.is_empty());
    assert!(outcome.result.findings.is_empty());
    assert!(outcome.result.clean_episode_ids.is_empty());
    assert!(!outcome.result.unassessed.is_empty());
}

#[test]
fn sampling_and_binding_keep_disjoint_episodes_separate() {
    let (content, task, _) = fixture("Fix the parser.", &["parser.rs", "other.rs"], "Complete.");
    let spans = vec![
        EpisodeSpan {
            first_event_id: content.actions[1].reference.id.clone(),
            last_event_id: content.actions[3].reference.id.clone(),
            state: EpisodeState::Complete,
        },
        EpisodeSpan {
            first_event_id: content.actions[4].reference.id.clone(),
            last_event_id: content.actions[5].reference.id.clone(),
            state: EpisodeState::Complete,
        },
    ];
    let input = build_episodes(&content, &task, &spans).unwrap();
    assert_eq!(input.episodes.len(), 2);
    assert_ne!(input.episodes[0].id, input.episodes[1].id);
    let mut plan = plan(&input);
    let inventory = plan.prepared.candidates.clone();
    let mut progress = SamplingProgress::new(SamplingLimits {
        checks: 4,
        candidates_per_check: 256,
        answers_per_candidate: 1024,
        judgments_per_run: 1,
    })
    .unwrap();
    synchronize_sampling(&plan, &mut progress).unwrap();
    progress.begin_run();
    let job = progress.choose_job().unwrap();
    PreparedAssessment::select_jobs(&mut plan, std::slice::from_ref(&job)).unwrap();
    assert_eq!(plan.coverage.not_selected_items, 1);
    let reduced = OverExploringCheck
        .reduce(&plan, &results(&plan, Some(Reason::UnrelatedFiles)), true)
        .unwrap();
    assert_eq!(reduced.completed_episode_ids, vec![job.candidate]);
    assert!(
        reduced
            .findings
            .iter()
            .all(|finding| finding.episode_id == job.candidate)
    );
    plan.prepared
        .record_completion(&reduced, &job, &mut progress)
        .unwrap();
    assert_eq!(progress.coverage(job.check).unwrap().remaining, 1);
    assert_eq!(plan.prepared.candidates, inventory);
    assert!(progress.choose_job().is_none());
}

#[test]
fn unbound_reads_and_omitted_inventory_do_not_become_clean() {
    let (mut content, task, span) = fixture("Fix the parser.", &["parser.rs"], "Complete.");
    assert!(build_episodes(&content, &task, &[]).is_err());
    content.actions[3]
        .metadata
        .read_result
        .as_mut()
        .unwrap()
        .request_reference_id = None;
    let input = build_episodes(&content, &task, &[span]).unwrap();
    let plan = plan(&input);
    assert!(plan.work_items.is_empty());
    let reduced = OverExploringCheck.reduce(&plan, &[], true).unwrap();
    assert!(reduced.clean_episode_ids.is_empty());
    assert!(!reduced.unassessed.is_empty());
}

#[test]
fn late_user_context_requires_latest_snapshot_and_cannot_reuse_earlier_authority() {
    let (mut content, task, span) = fixture("Fix the parser.", &["parser.rs"], "Complete.");
    let mut user = content.actions[0].clone();
    user.reference.id = "later-user".into();
    user.reference.turn_index = 6;
    user.text = "Actually audit every file.".into();
    content.actions.push(user);
    assert!(build_episodes(&content, &task, &[span]).is_err());
}

#[test]
fn completion_from_old_subsequent_work_is_rejected_after_requeue() {
    let input = input("Fix the parser.", &["parser.rs"], "Complete.");
    let initial = plan(&input);
    let reduced = OverExploringCheck
        .reduce(&initial, &results(&initial, None), true)
        .unwrap();
    let mut changed = input;
    changed.episodes[0].subsequent[0].text = "The later explanation uses this evidence.".into();
    let changed = plan(&changed);
    assert_eq!(initial.prepared.epoch, changed.prepared.epoch);
    let mut progress = SamplingProgress::new(SamplingLimits {
        checks: 4,
        candidates_per_check: 256,
        answers_per_candidate: 1024,
        judgments_per_run: 1,
    })
    .unwrap();
    synchronize_sampling(&changed, &mut progress).unwrap();
    progress.begin_run();
    let job = progress.choose_job().unwrap();
    assert!(
        changed
            .prepared
            .record_completion(&reduced, &job, &mut progress)
            .is_err()
    );
    assert_eq!(progress.coverage(job.check).unwrap().completed, 0);
}

#[test]
fn same_relative_path_in_different_directories_is_not_one_file() {
    let mut input = input(
        "Fix both services.",
        &["config.rs", "config.rs"],
        "Complete.",
    );
    input.episodes[0].reads[0].request.cwd = Some("service-a".into());
    input.episodes[0].reads[1].request.cwd = Some("service-b".into());
    let plan = plan(&input);
    let extent: Vec<_> = plan
        .prepared
        .targets
        .values()
        .filter(|target| target.reason == Reason::ExcessiveWithinFileReading)
        .collect();
    assert_eq!(extent.len(), 2);
    assert!(extent.iter().all(|target| target.read_indexes.len() == 1));
    assert!(
        plan.prepared
            .targets
            .values()
            .any(|target| target.reason == Reason::ExcessiveFileBreadth)
    );
}

#[test]
fn large_repeated_windows_stop_at_local_preparation_bound() {
    let mut input = input("Audit dependencies.", &["parser.rs"; 24], "Complete.");
    input.episodes[0].before[0].text = "recorded context ".repeat(70_000);
    let plan = plan(&input);
    assert!(plan.work_items.is_empty());
    assert!(plan.coverage.processing_limit_reached);
    assert!(
        plan.prepared
            .unassessed
            .iter()
            .any(|item| item.limitation == Abstention::PreparationLimitReached)
    );
    let result = OverExploringCheck.reduce(&plan, &[], true).unwrap();
    assert!(result.clean_episode_ids.is_empty());
}

fn recorded_read(parts: &mut Vec<PublishedContentPart>, path: &str, body: &str) {
    let order = u64::try_from(parts.len()).unwrap();
    let call = format!("call-{order}");
    parts.push(part(
        order,
        ContentKind::ToolInput,
        &json!({"filePath":path}).to_string(),
        Some(&call),
    ));
    let lines: Vec<_> = body
        .lines()
        .enumerate()
        .map(|(index, line)| format!("{}: {line}", index + 1))
        .collect();
    parts.push(part(order + 1, ContentKind::ToolResult, &format!("<path>{path}</path>\n<type>file</type>\n<content>\n{}\n\n(End of file - total {} lines)\n</content>", lines.join("\n"), lines.len()), Some(&call)));
}

fn recorded_tool(
    parts: &mut Vec<PublishedContentPart>,
    kind: ContentKind,
    tool: &str,
    call: &str,
    text: &str,
) {
    let mut record = part(u64::try_from(parts.len()).unwrap(), kind, text, None);
    record.part = record
        .part
        .with_tool_identity(Some(tool.into()), Some(call.into()));
    record.part.metadata.state = JevOperationState::Completed;
    parts.push(record);
}

fn diagnosed_input(reason: Reason, useful_later: bool) -> OverExploringInput {
    let mut parts = vec![part(
        0,
        ContentKind::UserText,
        "Fix expires_at(10, 10): an entry expires when now equals its deadline. Change only the boundary condition and test it. Do not audit unrelated documentation or archived implementations.",
        None,
    )];
    recorded_tool(
        &mut parts,
        ContentKind::ToolInput,
        "bash",
        "diagnostic-test",
        r#"{"command":"cargo test expires_at_deadline"}"#,
    );
    recorded_tool(
        &mut parts,
        ContentKind::ToolResult,
        "bash",
        "diagnostic-test",
        "test expires_at_deadline FAILED\nassertion failed: expired(10, 10)\nactual: false; expected: true\nexit code: 101",
    );
    recorded_read(
        &mut parts,
        "src/lib.rs",
        "mod expiry;\npub use expiry::expired;",
    );
    recorded_read(
        &mut parts,
        "src/expiry.rs",
        "pub fn expired(now: u64, deadline: u64) -> bool {\n    now > deadline\n}",
    );
    let first = parts.len();
    parts.push(part(u64::try_from(first).unwrap(), ContentKind::AssistantText, "The failing boundary test and expired implementation show that > must be >=. The active library imports expiry, not archive modules.", None));
    match reason {
        Reason::UnrelatedFiles => recorded_read(&mut parts, "docs/garden.txt", &"Plant tulips in autumn; use loose soil and water after planting.\nPrune roses after winter; remove dead branches before new growth.\n".repeat(24)),
        Reason::ExcessiveFileBreadth => {
            for index in 0..6 {
                recorded_read(&mut parts, &format!("archive/expiry_{index}.rs"), "// Archived example; not part of the library modules.\npub fn expired(now: u64, deadline: u64) -> bool {\n    now > deadline\n}\n#[test]\nfn old_example() { assert!(expired(11, 10)); }");
            }
        }
        Reason::ExcessiveWithinFileReading => {
            let body = "pub fn expired(now: u64, deadline: u64) -> bool {\n    now > deadline\n}\n";
            for _ in 0..8 { recorded_read(&mut parts, "src/expiry.rs", body); }
        }
    }
    let last = parts.len() - 1;
    recorded_tool(
        &mut parts,
        ContentKind::ToolInput,
        "edit",
        "correction",
        r#"{"filePath":"src/expiry.rs","oldString":"now > deadline","newString":"now >= deadline"}"#,
    );
    recorded_tool(
        &mut parts,
        ContentKind::ToolResult,
        "edit",
        "correction",
        "Updated src/expiry.rs: now >= deadline",
    );
    recorded_tool(
        &mut parts,
        ContentKind::ToolInput,
        "bash",
        "verification",
        r#"{"command":"cargo test expires_at_deadline"}"#,
    );
    recorded_tool(
        &mut parts,
        ContentKind::ToolResult,
        "bash",
        "verification",
        "test expires_at_deadline ... ok\n1 passed; 0 failed; exit code: 0",
    );
    if useful_later {
        parts.push(part(
            u64::try_from(parts.len()).unwrap(),
            ContentKind::UserText,
            "Also explain the archived expiry examples and how their old boundary behaves.",
            None,
        ));
        parts.push(part(u64::try_from(parts.len()).unwrap(), ContentKind::AssistantText, "The recorded archived examples return false at now == deadline and true at now > deadline. Their existing test covers only the latter. The explanation uses that source content; the current implementation now expires at equality.", None));
    }
    let (content, task, span) = fixture_from_parts(parts, first, last);
    build_episodes(&content, &task, &[span]).unwrap()
}

#[test]
fn concrete_diagnosis_and_performed_correction_reach_each_reason_target() {
    for reason in [
        Reason::UnrelatedFiles,
        Reason::ExcessiveFileBreadth,
        Reason::ExcessiveWithinFileReading,
    ] {
        let input = diagnosed_input(reason, false);
        let plan = plan(&input);
        let reduced = OverExploringCheck
            .reduce(&plan, &results(&plan, Some(reason)), true)
            .unwrap();
        assert!(!reduced.findings.is_empty());
        for finding in reduced.findings {
            let item = plan
                .work_items
                .iter()
                .find(|item| item.id == finding.work_item_id)
                .unwrap();
            let fields = serde_json::to_string(&item.window.fields).unwrap();
            assert!(fields.contains("actual: false; expected: true"));
            assert!(fields.contains("mod expiry"));
            assert!(fields.contains("Updated src/expiry.rs: now >= deadline"));
            assert!(fields.contains("1 passed; 0 failed"));
            for index in &plan.prepared.targets[&item.id].read_indexes {
                let read = &item.window.fields["reads"][index];
                assert_eq!(read["read_index"], *index);
                assert_eq!(read["is_target"], true);
                let result_index = read["result_event_index"].as_u64().unwrap();
                assert!(
                    item.window.fields["events"][usize::try_from(result_index).unwrap()]["text"]
                        .as_str()
                        .unwrap()
                        .contains("<content>")
                );
            }
        }
    }
}

#[test]
fn useful_later_explanation_vetoes_breadth_even_when_other_semantic_gates_support() {
    let initial = diagnosed_input(Reason::ExcessiveFileBreadth, false);
    let later = diagnosed_input(Reason::ExcessiveFileBreadth, true);
    assert_eq!(initial.episodes[0].id, later.episodes[0].id);
    let plan = plan(&later);
    let mut outcomes = results(&plan, Some(Reason::ExcessiveFileBreadth));
    for outcome in &mut outcomes {
        outcome
            .answers
            .insert("later_use".into(), answer("justified"));
    }
    let reduced = OverExploringCheck.reduce(&plan, &outcomes, true).unwrap();
    assert!(reduced.findings.is_empty());
    assert_eq!(reduced.clean_episode_ids, vec![later.episodes[0].id]);
    assert!(
        OverExploringCheck
            .reduce(
                &plan,
                &results(
                    &super::tests::plan(&initial),
                    Some(Reason::ExcessiveFileBreadth)
                ),
                true
            )
            .is_err()
    );
}

#[test]
fn shared_validator_and_reducer_accept_identical_rounding_without_renormalizing() {
    let plan = plan(&diagnosed_input(Reason::ExcessiveFileBreadth, false));
    for probabilities in [
        BTreeMap::from([
            ("supported".into(), 0.93),
            ("justified".into(), 0.02),
            ("unknown".into(), 0.04),
        ]),
        BTreeMap::from([
            ("supported".into(), 0.94),
            ("justified".into(), 0.03),
            ("unknown".into(), 0.04),
        ]),
    ] {
        let mut outcomes = results(&plan, Some(Reason::ExcessiveFileBreadth));
        let target = outcomes
            .iter_mut()
            .find(|outcome| {
                plan.prepared.targets[&outcome.work_item_id].reason == Reason::ExcessiveFileBreadth
            })
            .unwrap();
        target.answers.insert(
            "justified_breadth".into(),
            JevAnswer::Choice {
                choice: "supported".into(),
                confidence: 0.90,
                probabilities: probabilities.clone(),
            },
        );
        let item = plan
            .work_items
            .iter()
            .find(|item| item.id == target.work_item_id)
            .unwrap();
        let response = JevResponse {
            model: target.model.clone(),
            answers: target.answers.clone(),
            usage: target.usage,
        };
        let request = JevRequest {
            model: target.model.clone(),
            state: item.window.fields.clone(),
            questions: item.questions.clone(),
        };
        validate_jev_response(&response, &request).unwrap();
        assert!(
            OverExploringCheck
                .reduce(&plan, &outcomes, true)
                .unwrap()
                .findings
                .iter()
                .any(|finding| finding.reason == Reason::ExcessiveFileBreadth)
        );
        let JevAnswer::Choice {
            probabilities: retained,
            ..
        } = &response.answers["justified_breadth"]
        else {
            unreachable!()
        };
        assert_eq!(retained, &probabilities);
    }
    let mut outcomes = results(&plan, Some(Reason::ExcessiveFileBreadth));
    let target = &mut outcomes[0];
    target.answers.insert(
        "justified_breadth".into(),
        JevAnswer::Choice {
            choice: "supported".into(),
            confidence: 0.90,
            probabilities: BTreeMap::from([
                ("supported".into(), 0.90),
                ("justified".into(), 0.02),
                ("unknown".into(), 0.04),
            ]),
        },
    );
    let item = plan
        .work_items
        .iter()
        .find(|item| item.id == target.work_item_id)
        .unwrap();
    let response = JevResponse {
        model: target.model.clone(),
        answers: target.answers.clone(),
        usage: target.usage,
    };
    let request = JevRequest {
        model: target.model.clone(),
        state: item.window.fields.clone(),
        questions: item.questions.clone(),
    };
    assert!(validate_jev_response(&response, &request).is_err());
    assert!(OverExploringCheck.reduce(&plan, &outcomes, true).is_err());
}

#[test]
fn distribution_concentration_is_not_a_second_semantic_probability_gate() {
    let plan = plan(&diagnosed_input(Reason::ExcessiveWithinFileReading, false));
    for (probability, confidence, expected) in [
        (0.90, 0.85, true),
        (0.92, 0.89, true),
        (0.89, 0.835, false),
        (0.89, 0.99, false),
        (0.71, 0.565, false),
    ] {
        let mut outcomes = results(&plan, Some(Reason::ExcessiveWithinFileReading));
        for outcome in &mut outcomes {
            if plan.prepared.targets[&outcome.work_item_id].reason
                != Reason::ExcessiveWithinFileReading
            {
                continue;
            }
            for gate in super::questions::GATES {
                let choice = choice_for(
                    gate,
                    Reason::ExcessiveWithinFileReading,
                    Some(Reason::ExcessiveWithinFileReading),
                );
                outcome.answers.insert(
                    gate.into(),
                    JevAnswer::Choice {
                        choice: choice.into(),
                        confidence,
                        probabilities: BTreeMap::from([
                            (
                                "supported".into(),
                                if choice == "supported" {
                                    probability
                                } else {
                                    (1.0 - probability) / 2.0
                                },
                            ),
                            (
                                "justified".into(),
                                if choice == "justified" {
                                    probability
                                } else {
                                    (1.0 - probability) / 2.0
                                },
                            ),
                            ("unknown".into(), (1.0 - probability) / 2.0),
                        ]),
                    },
                );
            }
        }
        let reduced = OverExploringCheck.reduce(&plan, &outcomes, true).unwrap();
        assert_eq!(
            reduced
                .findings
                .iter()
                .any(|finding| finding.reason == Reason::ExcessiveWithinFileReading),
            expected
        );
        if !expected {
            assert!(reduced.clean_episode_ids.is_empty());
        }
    }
}

#[test]
fn uncertain_sibling_reasons_do_not_block_exactly_bound_supported_breadth() {
    let input = diagnosed_input(Reason::ExcessiveFileBreadth, false);
    let plan = plan(&input);
    let mut outcomes = results(&plan, Some(Reason::ExcessiveFileBreadth));
    for outcome in &mut outcomes {
        if plan.prepared.targets[&outcome.work_item_id].reason != Reason::ExcessiveFileBreadth {
            for gate in super::questions::GATES {
                outcome.answers.insert(gate.into(), answer("unknown"));
            }
        }
    }
    let reduced = OverExploringCheck.reduce(&plan, &outcomes, true).unwrap();
    assert_eq!(reduced.findings.len(), 1);
    assert_eq!(reduced.findings[0].reason, Reason::ExcessiveFileBreadth);
    assert_eq!(
        reduced.findings[0].reads,
        plan.prepared.targets[&reduced.findings[0].work_item_id].bindings
    );
    assert!(reduced.clean_episode_ids.is_empty());
    assert!(reduced.completed_episode_ids.is_empty());
    assert!(reduced.completed_work_item_ids.is_empty());
    assert!(reduced.unassessed.iter().all(
        |item| item.work_item_id.is_some() && item.reason != Some(Reason::ExcessiveFileBreadth)
    ));
}

#[test]
fn semantic_unknown_and_recorded_justification_are_distinct_outcomes() {
    let plan = plan(&diagnosed_input(Reason::UnrelatedFiles, false));
    let mut outcomes = results(&plan, Some(Reason::UnrelatedFiles));
    for outcome in &mut outcomes {
        outcome
            .answers
            .insert("useful_information".into(), answer("unknown"));
    }
    let reduced = OverExploringCheck.reduce(&plan, &outcomes, true).unwrap();
    assert!(reduced.findings.is_empty());
    assert!(reduced.clean_episode_ids.is_empty());
    for outcome in &mut outcomes {
        outcome
            .answers
            .insert("substantial".into(), answer("justified"));
    }
    let reduced = OverExploringCheck.reduce(&plan, &outcomes, true).unwrap();
    assert!(reduced.findings.is_empty());
    assert_eq!(reduced.clean_episode_ids.len(), 1);
}

#[test]
fn uncertainty_on_another_target_of_the_same_reason_does_not_expand_or_block_a_finding() {
    let input = input(
        "Fix the parser.",
        &["garden.txt", "library.rs"],
        "Complete.",
    );
    let plan = plan(&input);
    let mut outcomes = results(&plan, Some(Reason::UnrelatedFiles));
    let unrelated: Vec<_> = plan
        .prepared
        .targets
        .iter()
        .filter(|(_, target)| target.reason == Reason::UnrelatedFiles)
        .map(|(id, _)| id.clone())
        .collect();
    assert_eq!(unrelated.len(), 2);
    let uncertain = outcomes
        .iter_mut()
        .find(|outcome| outcome.work_item_id == unrelated[0])
        .unwrap();
    for gate in super::questions::GATES {
        uncertain.answers.insert(gate.into(), answer("unknown"));
    }
    let reduced = OverExploringCheck.reduce(&plan, &outcomes, true).unwrap();
    assert_eq!(reduced.findings.len(), 1);
    assert_eq!(reduced.findings[0].work_item_id, unrelated[1]);
    assert_eq!(
        reduced.findings[0].reads,
        plan.prepared.targets[&unrelated[1]].bindings
    );
    assert!(
        reduced
            .unassessed
            .iter()
            .any(|item| item.work_item_id.as_ref() == Some(&unrelated[0]))
    );
    assert!(reduced.completed_episode_ids.is_empty());
    assert!(reduced.clean_episode_ids.is_empty());
}

#[test]
fn unknown_observed_extent_does_not_block_supported_unrelated_work_or_invent_extent() {
    let mut input = diagnosed_input(Reason::UnrelatedFiles, false);
    input.episodes[0].reads[0]
        .result
        .as_mut()
        .unwrap()
        .returned_extent = None;
    let plan = plan(&input);
    assert!(
        plan.prepared
            .targets
            .values()
            .all(|target| target.reason != Reason::ExcessiveWithinFileReading)
    );
    let reduced = OverExploringCheck
        .reduce(&plan, &results(&plan, Some(Reason::UnrelatedFiles)), true)
        .unwrap();
    assert_eq!(reduced.findings.len(), 1);
    assert_eq!(reduced.findings[0].reason, Reason::UnrelatedFiles);
    assert!(
        reduced
            .unassessed
            .iter()
            .any(|item| item.limitation == Abstention::UnknownObservedExtent)
    );
    assert!(reduced.completed_episode_ids.is_empty());
    assert!(reduced.clean_episode_ids.is_empty());
}

#[test]
fn revised_semantics_reject_legacy_plans_and_overlapping_question_answers() {
    let plan = plan(&diagnosed_input(Reason::UnrelatedFiles, false));
    assert_eq!(
        plan.revisions,
        JevCheckRevisions {
            projection: 2,
            chunking: 1,
            questions: 7,
            reducer: 4
        }
    );
    let mut stale = plan.clone();
    stale.revisions = JevCheckRevisions {
        projection: 2,
        chunking: 1,
        questions: 3,
        reducer: 3,
    };
    assert_eq!(
        OverExploringCheck.reduce(&stale, &results(&plan, Some(Reason::UnrelatedFiles)), true),
        Err(JevError::InvalidCheckPlan)
    );
    let mut old_answers = results(&plan, Some(Reason::UnrelatedFiles));
    old_answers[0]
        .answers
        .insert("reason".into(), answer("supported"));
    assert!(
        OverExploringCheck
            .reduce(&plan, &old_answers, true)
            .is_err()
    );
    assert!(plan.work_items.iter().all(|item| {
        item.questions
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            == [
                "justified_breadth",
                "justified_extent",
                "later_use",
                "relevance",
                "substantial",
                "sufficiency",
                "useful_information",
            ]
    }));
}

#[test]
fn within_file_substantial_threshold_keeps_independent_questions_strict() {
    for reason in [Reason::UnrelatedFiles, Reason::ExcessiveWithinFileReading] {
        let plan = plan(&diagnosed_input(reason, false));
        let mut answers = results(&plan, Some(reason));
        for result in &mut answers {
            if plan.prepared.targets[&result.work_item_id].reason == reason {
                result.answers.insert(
                    "substantial".into(),
                    JevAnswer::Choice {
                        choice: "supported".into(),
                        confidence: 0.2,
                        probabilities: BTreeMap::from([
                            ("supported".into(), 0.71),
                            ("justified".into(), 0.14),
                            ("unknown".into(), 0.15),
                        ]),
                    },
                );
            }
        }
        let reduced = OverExploringCheck.reduce(&plan, &answers, true).unwrap();
        assert_eq!(
            reduced
                .findings
                .iter()
                .any(|finding| finding.reason == reason),
            reason == Reason::ExcessiveWithinFileReading
        );
        for gate in ["useful_information", "later_use", "sufficiency"] {
            let mut below = answers.clone();
            for result in &mut below {
                if plan.prepared.targets[&result.work_item_id].reason == reason {
                    result.answers.insert(
                        gate.into(),
                        JevAnswer::Choice {
                            choice: "supported".into(),
                            confidence: 0.99,
                            probabilities: BTreeMap::from([
                                ("supported".into(), 0.89),
                                ("justified".into(), 0.05),
                                ("unknown".into(), 0.06),
                            ]),
                        },
                    );
                }
            }
            assert!(
                !OverExploringCheck
                    .reduce(&plan, &below, true)
                    .unwrap()
                    .findings
                    .iter()
                    .any(|finding| finding.reason == reason)
            );
        }
    }
}

#[test]
fn pre_enrollment_reads_supply_context_without_becoming_finding_targets() {
    let (mut content, task, span) =
        fixture("Fix the parser.", &["parser.rs", "other.rs"], "Complete.");
    content.actions[2].context_only = true;
    let earlier_request = content.actions[2].reference.id.clone();
    let input = build_episodes(&content, &task, &[span]).unwrap();
    assert_eq!(input.episodes[0].reads.len(), 1);
    let plan = plan(&input);
    assert!(!plan.work_items.is_empty());
    let reduced = OverExploringCheck
        .reduce(&plan, &results(&plan, Some(Reason::UnrelatedFiles)), true)
        .unwrap();
    assert!(!reduced.findings.is_empty());
    assert!(
        reduced
            .findings
            .iter()
            .flat_map(|finding| &finding.reads)
            .all(|read| read.request_id != earlier_request)
    );
}

#[test]
fn reason_specific_questions_do_not_copy_or_veto_other_dimensions() {
    use SemanticOutcome::{Justified, Supported};
    for reason in [
        Reason::UnrelatedFiles,
        Reason::ExcessiveFileBreadth,
        Reason::ExcessiveWithinFileReading,
    ] {
        let plan = plan(&diagnosed_input(reason, false));
        for gate in super::questions::GATES {
            for choice in ["justified", "unknown"] {
                let mut outcomes = results(&plan, Some(reason));
                for outcome in &mut outcomes {
                    if plan.prepared.targets[&outcome.work_item_id].reason != reason {
                        continue;
                    }
                    outcome.answers.insert(gate.into(), answer(choice));
                }
                let result = OverExploringCheck.reduce(&plan, &outcomes, true).unwrap();
                let ignored = match reason {
                    Reason::UnrelatedFiles => {
                        ["justified_breadth", "justified_extent"].contains(&gate)
                    }
                    Reason::ExcessiveFileBreadth => {
                        ["relevance", "justified_extent"].contains(&gate)
                    }
                    Reason::ExcessiveWithinFileReading => gate == "justified_breadth",
                };
                let still_supported = ignored
                    || reason == Reason::ExcessiveWithinFileReading
                        && gate == "relevance"
                        && choice == "justified";
                assert_eq!(
                    result
                        .findings
                        .iter()
                        .any(|finding| finding.reason == reason),
                    still_supported,
                    "{reason:?}: {gate}: {choice}"
                );
                if still_supported {
                    let finding = result
                        .findings
                        .iter()
                        .find(|finding| finding.reason == reason)
                        .unwrap();
                    assert_eq!(finding.judgments.sufficiency, Supported);
                    assert_eq!(finding.judgments.useful_information, Supported);
                    assert_eq!(finding.judgments.later_use, Supported);
                    assert_eq!(finding.judgments.substantial, Supported);
                    if reason == Reason::ExcessiveWithinFileReading {
                        assert_eq!(finding.judgments.relevance, Justified);
                        assert_eq!(finding.judgments.justified_extent, Supported);
                    }
                    let saved: Decision =
                        serde_json::from_value(serde_json::to_value(finding).unwrap()).unwrap();
                    assert_eq!(saved, *finding);
                } else if choice == "unknown" || gate == "sufficiency" {
                    assert!(result.clean_episode_ids.is_empty(), "{reason:?}: {gate}");
                    assert!(result.completed_episode_ids.is_empty());
                }
            }
        }
    }
}

#[test]
fn insufficient_semantic_evidence_blocks_clean_even_with_recorded_justification() {
    for reason in [
        Reason::UnrelatedFiles,
        Reason::ExcessiveFileBreadth,
        Reason::ExcessiveWithinFileReading,
    ] {
        let plan = plan(&diagnosed_input(reason, true));
        for sufficiency in ["justified", "unknown"] {
            for veto in ["useful_information", "later_use", "substantial"] {
                let mut outcomes = results(&plan, None);
                for outcome in &mut outcomes {
                    outcome
                        .answers
                        .insert("sufficiency".into(), answer(sufficiency));
                    outcome.answers.insert(veto.into(), answer("justified"));
                }
                let result = OverExploringCheck.reduce(&plan, &outcomes, true).unwrap();
                assert!(result.findings.is_empty());
                assert!(result.clean_episode_ids.is_empty());
                assert!(result.completed_episode_ids.is_empty());
                assert!(result.completed_work_item_ids.is_empty());
                assert!(
                    result
                        .unassessed
                        .iter()
                        .all(|item| item.limitation == Abstention::UncertainDecision)
                );
            }
        }
    }
}

#[test]
fn within_file_reading_requires_relevant_files_even_with_excessive_extent() {
    let plan = plan(&diagnosed_input(Reason::ExcessiveWithinFileReading, false));
    let mut outcomes = results(&plan, Some(Reason::ExcessiveWithinFileReading));
    for outcome in &mut outcomes {
        if plan.prepared.targets[&outcome.work_item_id].reason == Reason::ExcessiveWithinFileReading
        {
            outcome
                .answers
                .insert("relevance".into(), answer("supported"));
        }
    }
    let result = OverExploringCheck.reduce(&plan, &outcomes, true).unwrap();
    assert!(result.findings.is_empty());
    assert_eq!(result.clean_episode_ids.len(), 1);
}
