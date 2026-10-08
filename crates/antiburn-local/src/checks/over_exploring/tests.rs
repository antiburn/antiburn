use std::collections::{BTreeMap, BTreeSet};

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
    task_text: &str,
    paths: &[&str],
    body: &str,
    later: &str,
) -> (SessionContentEvidence, SessionScopeSnapshot, EpisodeSpan) {
    fixture_with_later_kind(task_text, paths, body, later, ContentKind::AssistantText)
}

fn fixture_with_later_kind(
    task_text: &str,
    paths: &[&str],
    body: &str,
    later: &str,
    later_kind: ContentKind,
) -> (SessionContentEvidence, SessionScopeSnapshot, EpisodeSpan) {
    let mut parts = vec![
        part(0, ContentKind::UserText, task_text, None),
        part(
            1,
            ContentKind::AssistantText,
            "Investigate the task and its dependencies.",
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
        let lines = body
            .lines()
            .enumerate()
            .map(|(index, line)| format!("{}: {line}", index + 1))
            .collect::<Vec<_>>();
        let output = format!(
            "<path>{path}</path>\n<type>file</type>\n<content>\n{}\n\n(End of file - total {} lines)\n</content>",
            lines.join("\n"),
            lines.len()
        );
        parts.push(part(
            order + 1,
            ContentKind::ToolResult,
            &output,
            Some(&call),
        ));
    }
    let last = parts.last().unwrap().turn_index;
    for (index, (start, end)) in crate::analysis::jev::text_ranges::text_ranges(later, 2048, 0)
        .into_iter()
        .enumerate()
    {
        parts.push(part(
            last + 1 + u64::try_from(index).unwrap(),
            later_kind,
            &later[start..end],
            None,
        ));
    }
    let boundary_turn = parts.last().unwrap().turn_index;
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
            turn_index: boundary_turn,
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
    content.actions[0].context_only = true;
    let span = EpisodeSpan {
        first_event_id: content.actions[1].reference.id.clone(),
        last_event_id: content.actions[usize::try_from(last).unwrap()]
            .reference
            .id
            .clone(),
        state: EpisodeState::Complete,
    };
    (content, task, span)
}

fn input(paths: &[&str]) -> OverExploringInput {
    let (content, task, span) = fixture(
        "Fix the parser.",
        paths,
        "recorded content",
        "The investigation is complete.",
    );
    build_episodes(&content, &task, &[span]).unwrap()
}

#[test]
fn short_approval_does_not_establish_complete_task_context() {
    let parts = vec![
        part(
            0,
            ContentKind::UserText,
            "Audit the parser before editing. Keep billing unchanged.",
            None,
        ),
        part(
            1,
            ContentKind::AssistantText,
            "I propose to audit the parser and its tests, then report the risks.",
            None,
        ),
        part(2, ContentKind::UserText, "Yes, continue.", None),
        part(
            3,
            ContentKind::ToolInput,
            r#"{"filePath":"parser.rs","limit":2}"#,
            Some("audit"),
        ),
        part(4, ContentKind::ToolResult, "1: parser code", Some("audit")),
    ];
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
            turn_index: 4,
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
    let span = EpisodeSpan {
        first_event_id: content.actions[2].reference.id.clone(),
        last_event_id: content.actions[4].reference.id.clone(),
        state: EpisodeState::Complete,
    };
    let input = build_episodes(&content, &task, &[span]).unwrap();
    assert_eq!(input.task_context.fields["partial"], true);
    assert_eq!(
        input.task_context.fields["antecedent_context_omitted"],
        true
    );
    let plan = plan(&input);
    let assessment = OverExploringCheck
        .reduce(&plan, &results(&plan, "justified_or_minor", 0.9), true)
        .unwrap();
    assert!(assessment.clean_episode_ids.is_empty());
    assert!(
        assessment
            .unassessed
            .iter()
            .any(|item| item.limitation == Abstention::SourceLimited)
    );
}

#[test]
fn selected_plans_share_the_large_source_inventory() {
    let (content, task, span) = fixture(
        "Fix the parser.",
        &["parser.rs", "tests.rs", "library.rs"],
        &"recorded line\n".repeat(5000),
        "Investigation complete.",
    );
    let input = build_episodes(&content, &task, &[span]).unwrap();
    let plan = plan(&input);
    assert!(
        plan.prepared
            .events
            .iter()
            .map(|event| event.text.len())
            .sum::<usize>()
            > 128 * 1024
    );
    let mut sampling = SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: MAX_SAMPLING_CANDIDATES,
        answers_per_candidate: 1,
        judgments_per_run: 3,
    })
    .unwrap();
    synchronize_sampling(&plan, &mut sampling).unwrap();
    sampling.begin_run();
    while let Some(job) = sampling.choose_job() {
        let mut selected = plan.clone();
        PreparedAssessment::select_jobs(&mut selected, std::slice::from_ref(&job)).unwrap();
        let checkpoint = selected.clone();
        assert!(std::sync::Arc::ptr_eq(
            &plan.prepared.events,
            &selected.prepared.events
        ));
        assert!(std::sync::Arc::ptr_eq(
            &selected.prepared.events,
            &checkpoint.prepared.events
        ));
        assert!(std::sync::Arc::ptr_eq(
            &plan.prepared.episodes,
            &selected.prepared.episodes
        ));
        assert!(std::sync::Arc::ptr_eq(
            &plan.prepared.targets,
            &selected.prepared.targets
        ));
        assert!(std::sync::Arc::ptr_eq(
            &plan.prepared.task_contexts,
            &selected.prepared.task_contexts
        ));
    }
}

#[test]
fn small_reads_fit_default_capabilities_with_large_irrelevant_scope() {
    let irrelevant = (0..20_000)
        .map(|index| format!("Unrelated recorded discussion {index}. "))
        .collect::<String>();
    let (content, task, span) = fixture_with_later_kind(
        "Fix the parser.",
        &["parser.rs"],
        "small parser",
        &irrelevant,
        ContentKind::UserText,
    );
    let input = build_episodes(&content, &task, &[span]).unwrap();
    let started = std::time::Instant::now();
    let plan = plan(&input);
    let prepare_us = started.elapsed().as_micros();
    assert!(!plan.work_items.is_empty());
    assert!(plan.skipped_item_ids.is_empty());
    let shared_bytes = serde_json::to_vec(plan.shared_context.as_ref().unwrap())
        .unwrap()
        .len();
    let old_bytes = serde_json::to_vec(&task.user_context()).unwrap().len();
    assert!(shared_bytes * 100 < old_bytes);
    let old_packing = pack_work_items_with_shared_context(
        &plan.work_items,
        &plan.capabilities,
        &task.user_context(),
    );
    assert!(old_packing.batches.is_empty());
    assert_eq!(old_packing.skipped_item_ids.len(), plan.work_items.len());
    assert!(
        plan.shared_context
            .as_ref()
            .unwrap()
            .fields
            .to_string()
            .contains("Fix the parser")
    );
    let packed = pack_work_items_with_shared_context(
        &plan.work_items,
        &plan.capabilities,
        plan.shared_context.as_ref().unwrap(),
    );
    assert!(!packed.batches.is_empty());
    assert!(packed.skipped_item_ids.is_empty());
    eprintln!(
        "over_exploring large scope: old_shared_bytes={old_bytes} selected_shared_bytes={shared_bytes} prepare_us={prepare_us} requests={}",
        packed.batches.len()
    );
}

fn cache_age_source() -> String {
    let mut source = "pub fn accepts(value: usize, maximum: usize) -> bool {\n    value < maximum\n}\n#[test]\nfn endpoint_is_valid() { assert!(accepts(10, 10)); }\n#[test]\nfn overflow_is_invalid() { assert!(!accepts(11, 10)); }\n".to_owned();
    for index in 0..28 {
        source.push_str(&format!("fn render_panel_{index}() -> &'static str {{\n    \"Panel {index}: choose a display theme\"\n}}\n\n"));
    }
    source
}

fn plan(input: &OverExploringInput) -> JevCheckPlan<PreparedAssessment> {
    OverExploringCheck
        .prepare(&build_jev_context(input).unwrap())
        .unwrap()
}

fn answer(choice: &str, probability: f64) -> JevAnswer {
    JevAnswer::Choice {
        choice: choice.into(),
        confidence: 0.5,
        probabilities: ["likely_excess", "justified_or_minor", "uncertain"]
            .into_iter()
            .map(|value| {
                (
                    value.into(),
                    if value == choice {
                        probability
                    } else {
                        (1.0 - probability) / 2.0
                    },
                )
            })
            .collect(),
    }
}

fn results(
    plan: &JevCheckPlan<PreparedAssessment>,
    choice: &str,
    probability: f64,
) -> Vec<JevWorkItemResult> {
    plan.work_items
        .iter()
        .map(|item| {
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
                answers: BTreeMap::from([(QUESTION_ID.into(), answer(choice, probability))]),
            }
        })
        .collect()
}

fn progress() -> SamplingProgress {
    SamplingProgress::new(SamplingLimits {
        checks: 4,
        candidates_per_check: MAX_SAMPLING_CANDIDATES,
        answers_per_candidate: 1024,
        judgments_per_run: 3,
    })
    .unwrap()
}

#[test]
fn native_bindings_keep_observed_extent_separate_from_request() {
    let input = input(&["parser.rs", "library.rs"]);
    let episode = &input.episodes[0];
    assert_eq!(input.events[episode.before[0]].text, "Fix the parser.");
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
    let plan = plan(&input);
    assert_eq!(plan.work_items.len(), MAX_TARGETS_PER_TURN);
    assert_eq!(plan.prepared.targets.len(), 5);
    assert_eq!(plan.coverage.not_selected_items, 2);
    assert!(
        !plan
            .coverage
            .limitations
            .contains(&"SampledEvidence".into())
    );
    for item in &plan.work_items {
        assert_eq!(item.questions.len(), 1);
        let JevQuestion::Choice { criteria, .. } = &item.questions[QUESTION_ID] else {
            panic!("Expected Choice");
        };
        assert_eq!(
            criteria.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["justified_or_minor", "likely_excess", "uncertain"]
        );
    }
    assert_eq!(
        plan.revisions,
        JevCheckRevisions {
            projection: 5,
            chunking: 6,
            questions: 11,
            reducer: 7
        }
    );
}

#[test]
fn selected_probability_gates_each_reason_without_a_confidence_gate() {
    let input = input(&["library.rs", "parser.rs"]);
    let plan = plan(&input);
    assert_eq!(
        plan.work_items
            .iter()
            .map(|item| plan.prepared.targets[&item.id].reason)
            .collect::<BTreeSet<_>>()
            .len(),
        3
    );
    for (probability, expected) in [(0.749, false), (0.75, true), (0.98, true)] {
        let reduced = OverExploringCheck
            .reduce(&plan, &results(&plan, "likely_excess", probability), true)
            .unwrap();
        assert_eq!(reduced.findings.len(), if expected { 3 } else { 0 });
        assert_eq!(reduced.completed_work_item_ids.len(), 3);
        assert!(reduced.clean_episode_ids.is_empty());
        for decision in reduced.findings {
            assert_eq!(decision.outcome, SemanticOutcome::LikelyExcess);
            assert_eq!(decision.probability, probability);
            assert_eq!(
                decision.reads,
                plan.prepared.targets[&decision.work_item_id].bindings
            );
            assert_eq!(decision.task_evidence, input.task_context.evidence);
            assert!(!decision.source_evidence.is_empty());
        }
    }
}

#[test]
fn uncertain_and_justified_answers_are_distinct_terminal_results() {
    let plan = plan(&input(&["parser.rs"]));
    for choice in ["uncertain", "justified_or_minor"] {
        let reduced = OverExploringCheck
            .reduce(&plan, &results(&plan, choice, 0.98), true)
            .unwrap();
        assert!(reduced.findings.is_empty());
        assert_eq!(
            reduced.completed_episode_ids,
            vec![plan.prepared.episodes[0].id]
        );
        assert_eq!(reduced.clean_episode_ids.is_empty(), choice == "uncertain");
        let mut progress = progress();
        synchronize_sampling(&plan, &mut progress).unwrap();
        progress.begin_run();
        while let Some(job) = progress.choose_job() {
            plan.prepared
                .record_completion(&reduced, &job, &mut progress)
                .unwrap();
        }
        let check = crate::checks::sampling::StableId::new("smart-check", &[b"over_exploring"]);
        assert_eq!(progress.coverage(check).unwrap().completed, 2);
    }
}

#[test]
fn partial_history_deferred_and_truncated_inputs_remain_usable() {
    let (mut content, task, mut span) = fixture(
        "Fix the parser.",
        &["parser.rs"],
        "recorded content",
        "Investigation continues.",
    );
    content.complete = false;
    content.limitations = vec!["history_loss".into(), "open_investigation".into()];
    content.actions[0].truncated = true;
    span.state = EpisodeState::Deferred;
    let input = build_episodes(&content, &task, &[span]).unwrap();
    let plan = plan(&input);
    assert!(!plan.work_items.is_empty());
    for item in &plan.work_items {
        assert_eq!(item.window.fields["history_complete"], false);
        assert_eq!(item.window.fields["episode_state"], "deferred");
        assert_eq!(
            item.window.fields["source_limits"],
            json!(["history_loss", "open_investigation"])
        );
        assert_eq!(item.window.fields["before"][0]["source_truncated"], true);
    }
    let reduced = OverExploringCheck
        .reduce(&plan, &results(&plan, "uncertain", 0.98), true)
        .unwrap();
    assert_eq!(reduced.completed_work_item_ids.len(), plan.work_items.len());
    assert!(reduced.clean_episode_ids.is_empty());
}

#[test]
fn no_issue_on_partial_sources_keeps_durable_limits_without_full_clean() {
    for (variant, expected) in [
        ("history", Abstention::IncompleteHistory),
        ("deferred", Abstention::DeferredEpisode),
        ("source", Abstention::SourceLimited),
        ("task", Abstention::SourceLimited),
        ("truncated", Abstention::TruncatedEvidence),
        ("read_request", Abstention::TruncatedEvidence),
        ("read_result", Abstention::TruncatedEvidence),
    ] {
        let mut input = input(&["parser.rs"]);
        match variant {
            "history" => input.complete = false,
            "deferred" => input.episodes[0].state = EpisodeState::Deferred,
            "source" => input.limitations.push("history_loss".into()),
            "task" => input.task_context.fields["limitations"] = json!(["history_loss"]),
            "truncated" => input.events[0].truncated = true,
            "read_request" => {
                input.events[2]
                    .metadata
                    .read_request
                    .as_mut()
                    .unwrap()
                    .truncated = true;
                input.episodes[0].reads[0].request.truncated = true;
            }
            "read_result" => {
                input.events[3]
                    .metadata
                    .read_result
                    .as_mut()
                    .unwrap()
                    .truncated = true;
                input.episodes[0].reads[0]
                    .result
                    .as_mut()
                    .unwrap()
                    .truncated = true;
            }
            _ => unreachable!(),
        }
        let mut plan = plan(&input);
        let mut progress = progress();
        synchronize_sampling(&plan, &mut progress).unwrap();
        progress.begin_run();
        let jobs = (0..MAX_TARGETS_PER_TURN)
            .filter_map(|_| progress.choose_job())
            .collect::<Vec<_>>();
        PreparedAssessment::select_jobs(&mut plan, &jobs).unwrap();
        let reduced = OverExploringCheck
            .reduce(&plan, &results(&plan, "justified_or_minor", 0.98), true)
            .unwrap();
        let restored: Assessment =
            serde_json::from_slice(&serde_json::to_vec(&reduced).unwrap()).unwrap();
        assert!(restored.findings.is_empty(), "{variant}");
        assert!(restored.clean_episode_ids.is_empty(), "{variant}");
        assert_eq!(
            restored.completed_work_item_ids.len(),
            plan.work_items.len()
        );
        assert!(
            restored
                .unassessed
                .iter()
                .any(|item| item.limitation == expected),
            "{variant}"
        );
        assert!(
            restored
                .coverage
                .limitations
                .contains(&format!("{expected:?}")),
            "{variant}"
        );
        for job in &jobs {
            plan.prepared
                .record_completion(&restored, job, &mut progress)
                .unwrap();
        }
    }
}

#[test]
fn a_thousand_events_pack_with_bounded_support_and_exact_target_indexes() {
    let (mut content, task, span) = fixture(
        "Fix the parser.",
        &["parser.rs"],
        "recorded content",
        "The explanation uses the source.",
    );
    let mut later = content.actions.pop().unwrap();
    let mut result = content.actions.pop().unwrap();
    let mut request = content.actions.pop().unwrap();
    request.reference.turn_index = 500;
    result.reference.turn_index = 501;
    let template = content.actions[1].clone();
    for index in 2..999 {
        if index == 500 {
            content.actions.push(request.clone());
            continue;
        }
        if index == 501 {
            content.actions.push(result.clone());
            continue;
        }
        let mut event = template.clone();
        event.reference.id = format!("noise-{index}");
        event.reference.turn_index = index;
        event.text = "An unrelated progress update.".into();
        content.actions.push(event);
    }
    later.reference.turn_index = 999;
    content.actions.push(later);
    let input = build_episodes(&content, &task, &[span]).unwrap();
    assert_eq!(input.events.len(), 1000);
    let plan = plan(&input);
    assert_eq!(plan.work_items.len(), 2);
    assert!(plan.skipped_item_ids.is_empty());
    for item in &plan.work_items {
        let fields = &item.window.fields;
        let records = ["before", "events", "subsequent"]
            .into_iter()
            .flat_map(|key| fields[key].as_array().unwrap())
            .collect::<Vec<_>>();
        assert!(records.len() <= MAX_SUPPORTING_EVENTS + 2);
        assert_eq!(fields["event_selection"]["source_events"], 1000);
        assert_eq!(fields["event_selection"]["partial"], true);
        let read = &fields["reads"][0];
        let request_index = usize::try_from(read["request_event_index"].as_u64().unwrap()).unwrap();
        let result_index = usize::try_from(read["result_event_index"].as_u64().unwrap()).unwrap();
        assert_eq!(fields["events"][request_index]["source_index"], 500);
        assert_eq!(fields["events"][result_index]["source_index"], 501);
        assert_ne!(request_index, 499);
        assert!(
            serde_json::to_string(fields)
                .unwrap()
                .contains("The explanation uses the source.")
        );
        assert_eq!(fields["before"][0]["source_index"], 0);
        let citation = item
            .window
            .evidence
            .iter()
            .find(|citation| citation.part_id == format!("events[{result_index}].ranges[0].text"))
            .unwrap();
        assert_eq!(citation.source_id, input.events[501].reference.id);
    }
    let reduced = OverExploringCheck
        .reduce(&plan, &results(&plan, "justified_or_minor", 0.98), true)
        .unwrap();
    assert!(reduced.clean_episode_ids.is_empty());
    assert!(
        reduced
            .coverage
            .limitations
            .contains(&"SampledEvidence".into())
    );
}

#[test]
fn a_128_path_inventory_synchronizes_all_257_targets_with_the_engine_bound() {
    let paths = (0..128)
        .map(|index| format!("file-{index}.rs"))
        .collect::<Vec<_>>();
    let input = input(&paths.iter().map(String::as_str).collect::<Vec<_>>());
    let plan = plan(&input);
    assert_eq!(plan.prepared.targets.len(), 257);
    assert_eq!(plan.prepared.candidates.len(), 257);
    let mut progress = progress();
    synchronize_sampling(&plan, &mut progress).unwrap();
    let check = crate::checks::sampling::StableId::new("smart-check", &[b"over_exploring"]);
    assert_eq!(progress.coverage(check).unwrap().eligible, 257);
    assert_eq!(progress.coverage(check).unwrap().remaining, 257);
}

#[test]
fn missing_results_bind_only_actual_request_ids_and_never_inherit_extent() {
    let (mut content, task, span) = fixture(
        "Fix the parser.",
        &["parser.rs"],
        "recorded content",
        "Complete.",
    );
    content.actions[3]
        .metadata
        .read_result
        .as_mut()
        .unwrap()
        .request_reference_id = None;
    let input = build_episodes(&content, &task, &[span]).unwrap();
    assert!(input.episodes[0].reads[0].result.is_none());
    let plan = plan(&input);
    assert_eq!(plan.work_items.len(), 1);
    assert!(
        plan.prepared
            .targets
            .values()
            .all(|target| target.reason == Reason::UnrelatedFiles)
    );
    let reduced = OverExploringCheck
        .reduce(&plan, &results(&plan, "likely_excess", 0.98), true)
        .unwrap();
    let binding = &reduced.findings[0].reads[0];
    assert_eq!(binding.request_id, input.events[2].reference.id);
    assert_eq!(binding.result_id, None);
    assert_eq!(binding.output_digest, None);
    assert_eq!(
        plan.work_items[0].window.fields["reads"][0]["observed"],
        json!(null)
    );
    assert_eq!(
        plan.work_items[0].window.fields["reads"][0]["result_event_index"],
        json!(null)
    );
    assert!(reduced.clean_episode_ids.is_empty());
}

#[test]
fn unknown_observed_extent_does_not_block_other_reasons() {
    let (mut content, task, span) = fixture(
        "Fix the parser.",
        &["parser.rs"],
        "recorded content",
        "Complete.",
    );
    content.actions[3]
        .metadata
        .read_result
        .as_mut()
        .unwrap()
        .returned_extent = None;
    let input = build_episodes(&content, &task, &[span]).unwrap();
    let plan = plan(&input);
    assert_eq!(plan.work_items.len(), 1);
    let reduced = OverExploringCheck
        .reduce(&plan, &results(&plan, "likely_excess", 0.98), true)
        .unwrap();
    assert_eq!(reduced.findings.len(), 1);
    assert_eq!(reduced.findings[0].reason, Reason::UnrelatedFiles);
    assert_eq!(
        plan.work_items[0].window.fields["reads"][0]["observed"]["extent"],
        json!(null)
    );
    assert!(
        reduced
            .unassessed
            .iter()
            .any(|item| item.limitation == Abstention::UnknownObservedExtent)
    );
}

#[test]
fn continuation_rematerializes_unselected_descriptors_without_losing_inventory() {
    let mut plan = plan(&input(&["a.rs", "b.rs", "c.rs", "d.rs"]));
    let inventory = plan.prepared.candidates.clone();
    assert_eq!(inventory.len(), 9);
    let mut progress = progress();
    synchronize_sampling(&plan, &mut progress).unwrap();
    let mut previous = BTreeSet::new();
    for _ in 0..2 {
        progress.begin_run();
        let jobs = (0..3)
            .map(|_| progress.choose_job().unwrap())
            .collect::<Vec<_>>();
        PreparedAssessment::select_jobs(&mut plan, &jobs).unwrap();
        assert_eq!(plan.work_items.len(), 3);
        for item in &plan.work_items {
            assert!(previous.insert(item.id.clone()));
        }
        let reduced = OverExploringCheck
            .reduce(&plan, &results(&plan, "uncertain", 0.98), true)
            .unwrap();
        for job in &jobs {
            plan.prepared
                .record_completion(&reduced, job, &mut progress)
                .unwrap();
        }
        assert_eq!(plan.prepared.candidates, inventory);
        assert!(progress.choose_job().is_none());
    }
    assert_eq!(previous.len(), 6);
}

#[test]
fn oversized_utf8_events_use_exact_structural_representative_ranges() {
    let body = "fn boundary() { /* é 🦀 */ }\n".repeat(4000);
    let (content, task, span) = fixture("Fix the parser.", &["parser.rs"], &body, "Complete.");
    let input = build_episodes(&content, &task, &[span]).unwrap();
    let plan = plan(&input);
    assert_eq!(plan.work_items.len(), 2);
    assert!(
        plan.coverage
            .limitations
            .contains(&"SampledEvidence".into())
    );
    let reduced = OverExploringCheck
        .reduce(&plan, &results(&plan, "justified_or_minor", 0.98), true)
        .unwrap();
    assert!(reduced.clean_episode_ids.is_empty());
    assert_eq!(reduced.completed_work_item_ids.len(), 2);
    assert!(
        reduced
            .unassessed
            .iter()
            .all(|item| item.limitation == Abstention::SampledEvidence)
    );
    for item in &plan.work_items {
        let event = &item.window.fields["events"][2];
        assert_eq!(event["partial"], true);
        let ranges = event["ranges"].as_array().unwrap();
        assert_eq!(ranges.len(), MAX_EVENT_RANGES);
        let source = &input.events[3].text;
        let all = crate::analysis::jev::text_ranges::text_ranges(source, EVENT_RANGE_BYTES, 0);
        for (range, expected) in ranges
            .iter()
            .zip([all[0], all[all.len() / 2], all[all.len() - 1]])
        {
            let start = usize::try_from(range["start"].as_u64().unwrap()).unwrap();
            let end = usize::try_from(range["end"].as_u64().unwrap()).unwrap();
            assert_eq!((start, end), expected);
            assert!(end - start <= EVENT_RANGE_BYTES);
            assert_eq!(range["text"].as_str().unwrap(), &source[start..end]);
        }
        for (index, _) in ranges.iter().enumerate() {
            let citation = item
                .window
                .evidence
                .iter()
                .find(|citation| citation.part_id == format!("events[2].ranges[{index}].text"))
                .unwrap();
            assert_eq!(citation.source_id, input.events[3].reference.id);
            assert_eq!(citation.role, JevEvidenceRole::Candidate);
        }
        let text_bytes: usize = ["before", "events", "subsequent"]
            .into_iter()
            .flat_map(|key| item.window.fields[key].as_array().unwrap())
            .flat_map(|event| event["ranges"].as_array().unwrap())
            .map(|range| range["text"].as_str().unwrap().len())
            .sum();
        assert!(text_bytes <= MAX_WINDOW_TEXT_BYTES);
    }
}

#[test]
fn source_text_is_retained_once_across_many_episode_descriptors() {
    let (content, task, _) = fixture(
        "Audit dependencies.",
        &["a.rs", "b.rs", "c.rs", "d.rs"],
        &"recorded content\n".repeat(1000),
        "Complete.",
    );
    let spans = (0..4)
        .map(|index| EpisodeSpan {
            first_event_id: content.actions[index * 2 + 2].reference.id.clone(),
            last_event_id: content.actions[index * 2 + 3].reference.id.clone(),
            state: EpisodeState::Complete,
        })
        .collect::<Vec<_>>();
    let input = build_episodes(&content, &task, &spans).unwrap();
    assert_eq!(input.events, content.actions);
    let bytes = serde_json::to_vec(&input).unwrap().len();
    let source_bytes = serde_json::to_vec(&content.actions).unwrap().len();
    assert!(bytes < source_bytes * 2);
    let mut plan = plan(&input);
    assert_eq!(plan.prepared.episodes.len(), 4);
    assert_eq!(plan.prepared.events.len(), content.actions.len());
    assert_eq!(plan.work_items.len(), 3);
    assert_eq!(plan.prepared.targets.len(), 8);
    assert!(
        plan.coverage
            .limitations
            .contains(&"SampledEvidence".into())
    );
    let inventory = plan.prepared.candidates.clone();
    let mut progress = progress();
    synchronize_sampling(&plan, &mut progress).unwrap();
    let mut completed = BTreeSet::new();
    for _ in 0..3 {
        progress.begin_run();
        let jobs = (0..MAX_TARGETS_PER_TURN)
            .filter_map(|_| progress.choose_job())
            .collect::<Vec<_>>();
        PreparedAssessment::select_jobs(&mut plan, &jobs).unwrap();
        assert_eq!(plan.work_items.len(), jobs.len());
        assert!(
            plan.coverage
                .limitations
                .contains(&"SampledEvidence".into())
        );
        let reduced = OverExploringCheck
            .reduce(&plan, &results(&plan, "justified_or_minor", 0.98), true)
            .unwrap();
        assert!(reduced.clean_episode_ids.is_empty());
        for job in &jobs {
            assert!(completed.insert(job.candidate));
            plan.prepared
                .record_completion(&reduced, job, &mut progress)
                .unwrap();
        }
        assert_eq!(plan.prepared.candidates, inventory);
        assert_eq!(plan.prepared.events.as_ref(), &content.actions);
    }
    assert_eq!(completed.len(), inventory.len());
    progress.begin_run();
    assert!(progress.choose_job().is_none());
}

#[test]
fn source_binding_and_projection_corruption_are_rejected() {
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
        let (mut content, task, span) = fixture(
            "Fix the parser.",
            &["parser.rs"],
            "recorded content",
            "Complete.",
        );
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
    let mut input = input(&["parser.rs"]);
    input.episodes[0].reads[0]
        .result
        .as_mut()
        .unwrap()
        .reference_id = "invented".into();
    assert!(
        OverExploringCheck
            .prepare(&build_jev_context(&input).unwrap())
            .is_err()
    );
}

#[test]
fn invalid_results_and_legacy_plans_are_rejected() {
    let plan = plan(&input(&["parser.rs"]));
    for variant in [
        "model",
        "source",
        "role",
        "work",
        "duplicate",
        "missing",
        "extra",
        "nan",
        "sum",
        "confidence",
    ] {
        let mut results = results(&plan, "likely_excess", 0.98);
        match variant {
            "model" => results[0].model = "other-model".into(),
            "source" => results[0].evidence[0].source_id = "another-task".into(),
            "role" => results[0].evidence[0].role = JevEvidenceRole::Candidate,
            "work" => results[0].work_item_id = "different-work".into(),
            "duplicate" => results.push(results[0].clone()),
            "missing" => {
                results[0].answers.clear();
            }
            "extra" => {
                results[0]
                    .answers
                    .insert("relevance".into(), answer("likely_excess", 0.98));
            }
            "nan" | "sum" | "confidence" => {
                let JevAnswer::Choice {
                    probabilities,
                    confidence,
                    ..
                } = results[0].answers.get_mut(QUESTION_ID).unwrap()
                else {
                    unreachable!()
                };
                if variant == "confidence" {
                    *confidence = 2.0;
                } else {
                    probabilities.insert(
                        "likely_excess".into(),
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
    let mut stale = plan.clone();
    stale.revisions.questions = 7;
    assert!(OverExploringCheck.reduce(&stale, &[], true).is_err());
}

#[test]
fn source_and_task_changes_invalidate_target_answers() {
    let initial = input(&["parser.rs"]);
    let first = plan(&initial);
    for variant in ["task", "later", "limits", "state"] {
        let mut changed = initial.clone();
        match variant {
            "task" => changed.task_context.fields["values"][0] = json!("Audit all files."),
            "later" => {
                changed.events.last_mut().unwrap().text = "This explanation uses the source.".into()
            }
            "limits" => changed.limitations.push("history_loss".into()),
            "state" => changed.episodes[0].state = EpisodeState::Deferred,
            _ => unreachable!(),
        }
        let changed = plan(&changed);
        assert_eq!(
            first.prepared.episodes[0].id,
            changed.prepared.episodes[0].id
        );
        assert_ne!(
            first.prepared.candidates[0].required_answers,
            changed.prepared.candidates[0].required_answers
        );
        assert!(
            OverExploringCheck
                .reduce(&changed, &results(&first, "likely_excess", 0.98), true)
                .is_err()
        );
    }
}

#[test]
fn capability_failure_preserves_full_task_and_inventory() {
    let (content, task, span) = fixture(
        &"ordered task context ".repeat(200),
        &["parser.rs"],
        "recorded content",
        "Complete.",
    );
    let input = build_episodes(&content, &task, &[span]).unwrap();
    let mut small = ModelCapabilities::jev_default();
    small.request_body_bytes.value = Some(1024);
    let plan = OverExploringCheck
        .prepare_with_capabilities(&build_jev_context(&input).unwrap(), &small)
        .unwrap();
    assert!(plan.work_items.is_empty());
    assert_eq!(plan.prepared.targets.len(), 2);
    assert_eq!(plan.shared_context.as_ref().unwrap(), &input.task_context);
    assert_eq!(plan.skipped_item_ids.len(), 2);
}

#[test]
fn production_payload_keeps_the_explicit_audit_request_for_each_named_reason() {
    let request = "The cache_age endpoint is inclusive: accepts(10, 10) must be true and accepts(11, 10) must be false. Correct src/cache_age.rs. Audit the relevant validation paths and supply a written report before editing.";
    let source = cache_age_source();
    let report = "requested_audit result: the original source condition excludes 10 and accepts 9. The dependency must validate inclusive endpoints; caller limits 10 through 15 must stay inclusive. The source observations establish the condition, overflow test, unchanged caller contracts and absence of cleanup side effects. Test design: assert every caller endpoint and assert endpoint+1 is rejected. These observations support the requested explanation and risk report.";
    let (content, task, span) = fixture(request, &["src/cache_age.rs"; 3], &source, report);
    let input = build_episodes(&content, &task, &[span]).unwrap();
    let plan = plan(&input);
    let packed = pack_work_items_with_shared_context(
        &plan.work_items,
        &plan.capabilities,
        plan.shared_context.as_ref().unwrap(),
    );
    assert!(packed.skipped_item_ids.is_empty());
    let mut reasons = BTreeSet::new();
    for batch in packed.batches {
        validate_jev_request_with_capabilities(&batch.request, &plan.capabilities).unwrap();
        assert!(
            batch.request.state["shared_context"]["values"]
                .as_array()
                .unwrap()
                .contains(&json!(request))
        );
        for work in batch.request.state["work_items"].as_array().unwrap() {
            let reason: Reason = serde_json::from_value(work["context"]["reason"].clone()).unwrap();
            reasons.insert(reason);
            assert!(
                work["context"]["reads"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|read| read["requested"]["paths"] == json!(["src/cache_age.rs"]))
            );
            assert!(
                serde_json::to_string(&work["context"])
                    .unwrap()
                    .contains(report)
            );
        }
        for question in batch.request.questions.values() {
            let JevQuestion::Choice {
                instructions,
                criteria,
            } = question
            else {
                panic!("Expected Choice");
            };
            assert!(
                instructions["question"]["requested_coverage"]
                    .as_str()
                    .unwrap()
                    .contains("Diagnosis does not cancel the audit.")
            );
            assert!(instructions["question"]["reason_boundary"].as_str().unwrap().contains(
                "A task-relevant file with excessive regions or repeats is not an unrelated file"
            ));
            assert_eq!(criteria.len(), 3);
        }
    }
    assert_eq!(
        reasons,
        BTreeSet::from([Reason::UnrelatedFiles, Reason::ExcessiveWithinFileReading])
    );
    assert!(
        plan.shared_context
            .as_ref()
            .unwrap()
            .evidence
            .iter()
            .any(
                |reference| reference.source_id == content.actions[0].reference.id
                    && reference.role == JevEvidenceRole::Instruction
            )
    );
}

#[tokio::test]
async fn mock_payload_separates_initial_diagnosis_from_later_repeat_targets() {
    let request = "The cache_age endpoint is inclusive: accepts(10, 10) must be true and accepts(11, 10) must be false. Correct src/cache_age.rs. Change only the inclusive endpoint comparison. No audit, refactor, or alternative implementation is requested.";
    let diagnosis = "The recorded accepts function uses <; the failing endpoint test requires <=. I will replace that operator at src/cache_age.rs:2.";
    for timestamps_present in [true, false] {
        let (mut content, task, _) = fixture(
            request,
            &["src/cache_age.rs"; 5],
            &cache_age_source(),
            "The endpoint correction is complete: value <= maximum.",
        );
        let mut established = content.actions[1].clone();
        established.reference.id = "established-cause".into();
        established.text = diagnosis.into();
        content.actions.insert(4, established);
        for (index, event) in content.actions.iter_mut().enumerate() {
            event.reference.turn_index = u64::try_from(index).unwrap();
            event.timestamp_ms = timestamps_present.then(|| i64::try_from(index).unwrap());
        }
        let spans = [
            EpisodeSpan {
                first_event_id: content.actions[1].reference.id.clone(),
                last_event_id: content.actions[4].reference.id.clone(),
                state: EpisodeState::Complete,
            },
            EpisodeSpan {
                first_event_id: content.actions[5].reference.id.clone(),
                last_event_id: content.actions[12].reference.id.clone(),
                state: EpisodeState::Complete,
            },
        ];
        let input = build_episodes(&content, &task, &spans).unwrap();
        let prepared = plan(&input);
        let initial = prepared
            .prepared
            .targets
            .values()
            .find(|target| {
                target.reason == Reason::ExcessiveWithinFileReading
                    && target.episode_id == input.episodes[0].id
            })
            .unwrap();
        let later = prepared
            .prepared
            .targets
            .values()
            .find(|target| {
                target.reason == Reason::ExcessiveWithinFileReading
                    && target.episode_id == input.episodes[1].id
            })
            .unwrap();
        assert_eq!(initial.bindings.len(), 1);
        assert_eq!(later.bindings.len(), 4);
        assert_eq!(
            initial.bindings[0].output_digest,
            later.bindings[0].output_digest
        );
        assert!(
            later
                .bindings
                .iter()
                .all(|read| read.request_id != initial.bindings[0].request_id)
        );
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<JevRequest>::new()));
        let capture = captured.clone();
        let outcome = run_jev_check(
            &OverExploringCheck,
            &build_jev_context(&input).unwrap(),
            JevRunProgress::default(),
            move |batch| {
                let capture = capture.clone();
                async move {
                    capture.lock().unwrap().push(batch.request.clone());
                    Ok(JevResponse {
                        model: batch.request.model.clone(),
                        answers: batch
                            .request
                            .questions
                            .keys()
                            .map(|id| (id.clone(), answer("uncertain", 0.98)))
                            .collect(),
                        usage: JevUsage {
                            input_tokens: 10,
                            output_tokens: 1,
                        },
                    })
                }
            },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert!(outcome.complete, "{:?}", outcome.failure);
        let captured = captured.lock().unwrap();
        let (batch, context) = captured
            .iter()
            .flat_map(|batch| {
                batch.state["work_items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(move |work| (batch, &work["context"]))
            })
            .find(|(_, context)| {
                context["reason"] == json!("excessive_within_file_reading")
                    && context["reads"][0]["request_source_index"] == 2
            })
            .unwrap();
        assert_eq!(context["target_read_indexes"], json!([0]));
        assert_eq!(context["reads"].as_array().unwrap().len(), 1);
        let read = &context["reads"][0];
        assert_eq!(read["request_source_index"], 2);
        assert_eq!(read["result_source_index"], 3);
        assert_eq!(
            read["request_timestamp_ms"],
            json!(timestamps_present.then_some(2))
        );
        assert_eq!(
            read["result_timestamp_ms"],
            json!(timestamps_present.then_some(3))
        );
        let cause = context["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["source_index"] == 4)
            .unwrap();
        assert!(serde_json::to_string(cause).unwrap().contains(diagnosis));
        assert_eq!(
            cause["timestamp_ms"],
            json!(timestamps_present.then_some(4))
        );
        assert!(
            context["before"]
                .as_array()
                .unwrap()
                .iter()
                .all(|event| event["source_index"].as_u64().unwrap() < 2)
        );
        assert!(
            context["subsequent"]
                .as_array()
                .unwrap()
                .iter()
                .all(|event| event["source_index"].as_u64().unwrap() > 4)
        );
        assert_eq!(
            context["subsequent"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|event| event["kind"] == "tool_input")
                .count(),
            4
        );
        assert!(batch.state["shared_context"].to_string().contains(request));
        for question in batch.questions.values() {
            let JevQuestion::Choice { instructions, .. } = question else {
                panic!("Expected Choice");
            };
            assert!(
                instructions["question"]["temporal_scope"]
                    .as_str()
                    .unwrap()
                    .contains("Do not borrow later repetition")
            );
        }
    }
}

#[test]
fn legitimate_investigation_context_and_proportionality_rules_reach_the_question() {
    for (task_text, later) in [
        (
            "Audit all files for security risks.",
            "The audit needs this evidence.",
        ),
        (
            "Fix the dependency failure.",
            "The imported dependency explains its caller contract.",
        ),
        (
            "Find the failing code path.",
            "The investigation eliminates a reasonable hypothesis.",
        ),
        ("Change the cross-cutting API.", "All callers need review."),
        ("Verify concurrent file changes.", "Reread changed content."),
        ("Fix the small file.", "One line gives the needed context."),
        (
            "Explain the failure without edits.",
            "The explanation uses the source.",
        ),
    ] {
        let (content, task, span) =
            fixture(task_text, &["parser.rs"; 12], "recorded content", later);
        let input = build_episodes(&content, &task, &[span]).unwrap();
        let plan = plan(&input);
        let reduced = OverExploringCheck
            .reduce(&plan, &results(&plan, "justified_or_minor", 0.98), true)
            .unwrap();
        assert!(reduced.findings.is_empty());
        assert_eq!(reduced.clean_episode_ids.len(), 1);
        assert!(
            serde_json::to_string(&plan.shared_context)
                .unwrap()
                .contains(task_text)
        );
        let fields = serde_json::to_string(&plan.work_items[0].window.fields).unwrap();
        assert!(fields.contains(later));
        let question = serde_json::to_string(&plan.work_items[0].questions).unwrap();
        assert!(question.contains("proportionality"));
        assert!(question.contains("reasonable hypothesis elimination"));
    }
}

#[tokio::test]
async fn offline_runner_validates_one_choice_and_keeps_failures_pending() {
    let context = build_jev_context(&input(&["parser.rs"])).unwrap();
    let outcome = run_jev_check(
        &OverExploringCheck,
        &context,
        JevRunProgress::default(),
        |batch| async move {
            validate_jev_request_with_capabilities(
                &batch.request,
                &ModelCapabilities::jev_default(),
            )?;
            Ok(JevResponse {
                model: batch.request.model.clone(),
                answers: batch
                    .request
                    .questions
                    .keys()
                    .map(|id| (id.clone(), answer("likely_excess", 0.98)))
                    .collect(),
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
    assert_eq!(outcome.result.findings.len(), 2);
    let failed = run_jev_check(
        &OverExploringCheck,
        &context,
        JevRunProgress::default(),
        |_| async { Err(JevError::ProviderUnavailable) },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert!(!failed.complete);
    assert!(failed.result.completed_work_item_ids.is_empty());
    assert!(failed.result.clean_episode_ids.is_empty());
    assert!(!failed.result.unassessed.is_empty());
}
