use super::*;
use crate::analysis::jev::capabilities::{CapabilityLimit, CapabilitySource, ModelCapabilities};
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
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn absolute_source_paths_do_not_depend_on_the_host_platform() {
    for path in [
        "/repo/src/parser.rs",
        "C:\\repo\\parser.rs",
        "D:/repo/parser.rs",
        "\\\\server\\share\\parser.rs",
    ] {
        assert!(is_absolute_source_path(path), "{path}");
    }
    for path in [
        "src/parser.rs",
        "C:parser.rs",
        "\\parser.rs",
        "",
        "1:/parser.rs",
    ] {
        assert!(!is_absolute_source_path(path), "{path}");
    }
}

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

fn source(parts: Vec<PublishedContentPart>) -> (SessionContentEvidence, SessionScopeSnapshot) {
    let boundary_turn = parts.last().unwrap().turn_index;
    let page = PublishedContent {
        publication_fence: 4,
        source_generation: Some(3),
        parts,
        ..Default::default()
    };
    let mut builder = SessionScopeBuilder::new(
        SourceFormat::OpenCodeSqliteV1,
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
        prepare_session_content("session", SourceFormat::OpenCodeSqliteV1, page, Vec::new());
    content.complete = true;
    (content, task)
}

fn fixture(task: &str, paths: &[&str], body: &str) -> OverExploringInput {
    let mut parts = vec![
        part(0, ContentKind::UserText, task, None),
        part(
            1,
            ContentKind::AssistantText,
            "Inspect the parser and its dependencies.",
            None,
        ),
    ];
    for (index, path) in paths.iter().enumerate() {
        let order = u64::try_from(index * 2 + 2).unwrap();
        let call = format!("call-{index}");
        parts.push(part(
            order,
            ContentKind::ToolInput,
            &json!({"filePath":path, "offset":1, "limit":2000}).to_string(),
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
    let (content, scope) = source(parts);
    let span = EpisodeSpan {
        first_event_id: content.actions[1].reference.id.clone(),
        last_event_id: content.actions.last().unwrap().reference.id.clone(),
        state: EpisodeState::Complete,
    };
    build_episodes(&content, &scope, &[span]).unwrap()
}

fn repeat_input(count: usize) -> OverExploringInput {
    fixture(
        "Explain the parser boundary. Reuse available results.",
        &vec!["/repo/src/parser.rs"; count],
        &"validate the boundary\n".repeat(500),
    )
}

fn closed_repeat_after_open_unrelated_input() -> OverExploringInput {
    let mut parts = vec![
        part(
            0,
            ContentKind::UserText,
            "Explain the parser boundary.",
            None,
        ),
        part(1, ContentKind::AssistantText, "Inspect the parser.", None),
    ];
    for (index, path) in [
        "/repo/notes/garden.txt",
        "/repo/src/parser.rs",
        "/repo/src/parser.rs",
    ]
    .into_iter()
    .enumerate()
    {
        let order = u64::try_from(index * 2 + 2).unwrap();
        let call = format!("call-{index}");
        parts.push(part(
            order,
            ContentKind::ToolInput,
            &json!({"filePath":path,"offset":1,"limit":500}).to_string(),
            Some(&call),
        ));
        let lines = (1..=300)
            .map(|line| format!("{line}: Garden guide: plant seeds and water seedlings."))
            .collect::<Vec<_>>()
            .join("\n");
        let output = format!(
            "<path>{path}</path>\n<type>file</type>\n<content>\n{lines}\n\n(End of file - total 300 lines)\n</content>"
        );
        parts.push(part(
            order + 1,
            ContentKind::ToolResult,
            &output,
            Some(&call),
        ));
    }
    let (content, task) = source(parts);
    let spans = [
        EpisodeSpan {
            first_event_id: content.actions[1].reference.id.clone(),
            last_event_id: content.actions[3].reference.id.clone(),
            state: EpisodeState::Deferred,
        },
        EpisodeSpan {
            first_event_id: content.actions[4].reference.id.clone(),
            last_event_id: content.actions[7].reference.id.clone(),
            state: EpisodeState::Complete,
        },
    ];
    build_episodes(&content, &task, &spans).unwrap()
}

fn plan(input: &OverExploringInput) -> JevCheckPlan<PreparedAssessment> {
    OverExploringCheck
        .prepare(&build_jev_context(input).unwrap())
        .unwrap()
}

fn results(
    plan: &JevCheckPlan<PreparedAssessment>,
    choice: &str,
    probability: f64,
) -> Vec<JevWorkItemResult> {
    plan.work_items
        .iter()
        .map(|item| {
            let mut evidence = plan
                .shared_context
                .as_ref()
                .map_or_else(Vec::new, |shared| shared.evidence.clone());
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
                answers: BTreeMap::from([(
                    QUESTION_ID.into(),
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
                    },
                )]),
            }
        })
        .collect()
}

fn packed(plan: &JevCheckPlan<PreparedAssessment>) -> JevPackingResult {
    match &plan.shared_context {
        Some(shared) => {
            pack_work_items_with_shared_context(&plan.work_items, &plan.capabilities, shared)
        }
        None => pack_work_items_with_capabilities(&plan.work_items, &plan.capabilities),
    }
}

#[test]
fn fifty_reads_keep_complete_bindings_and_constant_size_aggregate_facts() {
    let small = plan(&repeat_input(2));
    let large = plan(&repeat_input(50));
    assert_eq!(large.work_items.len(), 1);
    assert_eq!(
        large
            .prepared
            .targets
            .values()
            .next()
            .unwrap()
            .bindings
            .len(),
        50
    );
    assert_eq!(large.work_items[0].window.fields["repeat"]["count"], 50);
    assert!(
        large.work_items[0].window.fields["repeat"]
            .get("reads")
            .is_none()
    );
    assert!(
        serde_json::to_vec(&large.work_items[0].window.fields)
            .unwrap()
            .len()
            < serde_json::to_vec(&small.work_items[0].window.fields)
                .unwrap()
                .len()
                + 20
    );
    assert_eq!(packed(&large).batches.len(), 1);
    let result = OverExploringCheck
        .reduce(&large, &results(&large, "likely_excess", 1.0), true)
        .unwrap();
    assert_eq!(result.findings[0].reads.len(), 50);
    let basis = result.findings[0].explanation.as_ref().unwrap();
    assert_eq!(basis.version, 3);
    assert_eq!(basis.whole_output_equal, Some(true));
    assert!(result.findings[0].has_valid_explanation());
    let sample = basis.compared.fields["repeat"]["header_sample"]["text"]
        .as_str()
        .unwrap();
    assert!(sample.starts_with("1: validate the boundary"));
    assert!(sample.len() <= 64);
    assert!(serde_json::to_vec(&basis.compared.fields).unwrap().len() < 1024);
}

#[test]
fn global_selection_prefers_two_runs_then_one_substantial_unrelated_read() {
    let input = fixture(
        "Explain the parser.",
        &[
            "/repo/a.rs",
            "/repo/a.rs",
            "/repo/b.rs",
            "/repo/b.rs",
            "/repo/c.rs",
            "/repo/c.rs",
            "/repo/garden.txt",
        ],
        &"garden guide: plant seeds and water seedlings\n".repeat(300),
    );
    let plan = plan(&input);
    assert_eq!(plan.prepared.targets.len(), 3);
    assert_eq!(plan.prepared.candidates.len(), 1);
    let mut selected_paths = plan
        .prepared
        .targets
        .values()
        .map(|target| {
            let episode = plan
                .prepared
                .episodes
                .iter()
                .find(|episode| episode.id == target.episode_id)
                .unwrap();
            episode.reads[target.read_indexes[0]].request.paths[0].as_str()
        })
        .collect::<Vec<_>>();
    selected_paths.sort_unstable();
    assert_eq!(
        selected_paths,
        ["/repo/a.rs", "/repo/b.rs", "/repo/garden.txt"]
    );
    assert_eq!(
        plan.prepared
            .targets
            .values()
            .filter(|target| target.reason == Reason::ExcessiveWithinFileReading)
            .count(),
        2
    );
    assert_eq!(
        plan.prepared
            .targets
            .values()
            .find(|target| target.reason == Reason::UnrelatedFiles)
            .unwrap()
            .read_indexes,
        [6]
    );
    assert!(
        plan.prepared
            .unassessed
            .iter()
            .any(|item| item.limitation == Abstention::SampledEvidence)
    );
    assert!(packed(&plan).batches.len() <= 1);
    let mut sampling = SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: 1,
        answers_per_candidate: 3,
        judgments_per_run: 3,
    })
    .unwrap();
    synchronize_sampling(&plan, &mut sampling).unwrap();
    sampling.begin_run();
    let job = sampling.choose_job().unwrap();
    let mut selected = plan.clone();
    PreparedAssessment::select_jobs(&mut selected, std::slice::from_ref(&job)).unwrap();
    assert_eq!(selected.work_items, plan.work_items);
    let result = OverExploringCheck
        .reduce(
            &selected,
            &results(&selected, "justified_or_minor", 1.0),
            true,
        )
        .unwrap();
    selected
        .prepared
        .record_completion(&result, &job, &mut sampling)
        .unwrap();
    assert!(sampling.choose_job().is_none());
    assert!(std::sync::Arc::ptr_eq(
        &plan.prepared.events,
        &selected.prepared.events
    ));
}

#[test]
fn global_priority_selects_larger_later_runs_across_episodes() {
    let mut input = fixture(
        "Explain the parser.",
        &[
            "/repo/a.rs",
            "/repo/a.rs",
            "/repo/b.rs",
            "/repo/b.rs",
            "/repo/c.rs",
            "/repo/c.rs",
        ],
        &"validate the boundary\n".repeat(500),
    );
    for index in [4, 5] {
        let read = &mut input.episodes[0].reads[index];
        let result = read.result.as_mut().unwrap();
        let action = input
            .events
            .iter_mut()
            .find(|action| action.reference.id == result.reference_id)
            .unwrap();
        action.text.push_str(&"more recorded output\n".repeat(500));
        result.recorded_output_digest =
            crate::checks::ignored_instructions::sha256_hex(action.text.as_bytes());
        result.recorded_output_bytes = u64::try_from(action.text.len()).unwrap();
        action.metadata.read_result = Some(result.clone());
    }
    let original = input.episodes.remove(0);
    let task = input.task_contexts.remove(&original.id).unwrap();
    for run in 0_usize..3 {
        let start = 2 + run * 4;
        let end = start + 4;
        let id = crate::checks::sampling::StableId::new("test-episode", &[&run.to_le_bytes()]);
        input.episodes.push(InvestigationEpisode {
            id,
            state: EpisodeState::Complete,
            before: (0..start).collect(),
            events: (start..end).collect(),
            subsequent: (end..input.events.len()).collect(),
            reads: original.reads[run * 2..run * 2 + 2].to_vec(),
        });
        input.task_contexts.insert(id, task.clone());
    }
    let plan = plan(&input);
    let mut paths = plan
        .prepared
        .targets
        .values()
        .map(|target| {
            input
                .episodes
                .iter()
                .find(|episode| episode.id == target.episode_id)
                .unwrap()
                .reads[target.read_indexes[0]]
                .request
                .paths[0]
                .as_str()
        })
        .collect::<Vec<_>>();
    paths.sort_unstable();
    assert!(paths.contains(&"/repo/c.rs"));
    assert!(paths.contains(&"/repo/a.rs"));
    assert!(!paths.contains(&"/repo/b.rs"));
    assert_eq!(paths.len(), 2);
    let first_request_path = plan.prepared.candidates[0]
        .work_item_ids
        .first()
        .map(|id| &plan.prepared.targets[id])
        .map(|target| {
            input
                .episodes
                .iter()
                .find(|episode| episode.id == target.episode_id)
                .unwrap()
                .reads[target.read_indexes[0]]
                .request
                .paths[0]
                .as_str()
        })
        .unwrap();
    assert_eq!(first_request_path, "/repo/c.rs");
    assert!(packed(&plan).batches.len() <= 1);
}

#[test]
fn unrelated_selection_prefers_larger_output_then_earlier_source_order() {
    let mut input = fixture(
        "Explain the parser boundary.",
        &[
            "/repo/notes/a.txt",
            "/repo/notes/b.txt",
            "/repo/notes/c.txt",
        ],
        &"Garden guide: prepare compost and water seedlings.\n".repeat(300),
    );
    for read_index in [1, 2] {
        let read = &mut input.episodes[0].reads[read_index];
        let result = read.result.as_mut().unwrap();
        let action = input
            .events
            .iter_mut()
            .find(|action| action.reference.id == result.reference_id)
            .unwrap();
        action
            .text
            .push_str(&"Additional garden notes.\n".repeat(100));
        result.recorded_output_digest =
            crate::checks::ignored_instructions::sha256_hex(action.text.as_bytes());
        result.recorded_output_bytes = u64::try_from(action.text.len()).unwrap();
        action.metadata.read_result = Some(result.clone());
    }
    let plan = plan(&input);
    assert_eq!(plan.prepared.targets.len(), 1);
    let target = plan.prepared.targets.values().next().unwrap();
    assert_eq!(target.reason, Reason::UnrelatedFiles);
    assert_eq!(target.read_indexes, [1]);
}

#[test]
fn positive_answer_cannot_bypass_local_binding_task_or_payload_gates() {
    let valid = plan(&repeat_input(3));
    let answers = results(&valid, "likely_excess", 1.0);
    for corruption in 0..7 {
        let mut invalid = valid.clone();
        match corruption {
            0 => {
                std::sync::Arc::make_mut(&mut invalid.prepared.episodes)[0].reads[1]
                    .result
                    .as_mut()
                    .unwrap()
                    .recorded_output_digest = "changed".into()
            }
            1 => {
                std::sync::Arc::make_mut(&mut invalid.prepared.events)[2]
                    .reference
                    .stable = false
            }
            2 => invalid.prepared.history_complete = false,
            3 => std::sync::Arc::make_mut(&mut invalid.prepared.task_contexts)
                .values_mut()
                .next()
                .unwrap()
                .evidence
                .clear(),
            4 => invalid.work_items[0].window.fields["repeat"]["count"] = json!(1),
            5 => invalid.shared_context.as_mut().unwrap().fields["task"] = json!("Changed task"),
            6 => {
                std::sync::Arc::make_mut(&mut invalid.prepared.task_contexts)
                    .values_mut()
                    .next()
                    .unwrap()
                    .fields["values"] = json!([])
            }
            _ => unreachable!(),
        }
        assert!(
            OverExploringCheck.reduce(&invalid, &answers, true).is_err(),
            "corruption {corruption}"
        );
    }
}

#[test]
fn incomplete_history_and_truncated_evidence_cannot_publish_or_be_clean() {
    for mode in 0..3 {
        let mut input = repeat_input(2);
        match mode {
            0 => input.complete = false,
            1 => input.limitations.push("context_loss".into()),
            2 => {
                input.events[3].truncated = true;
            }
            _ => unreachable!(),
        }
        let plan = plan(&input);
        if mode == 2 {
            assert!(
                plan.prepared
                    .targets
                    .values()
                    .all(|target| target.reason != Reason::ExcessiveWithinFileReading)
            );
        } else {
            assert!(plan.work_items.is_empty());
        }
        let result = OverExploringCheck.reduce(&plan, &[], true).unwrap();
        assert!(result.findings.is_empty());
        assert!(result.clean_episode_ids.is_empty());
        assert!(!result.unassessed.is_empty());
    }
}

#[test]
fn unknown_relative_cwd_different_extent_and_unequal_full_outputs_block_repeat_proof() {
    for mode in 0..9 {
        let mut input = repeat_input(2);
        if mode == 0 || mode == 1 {
            for read in &mut input.episodes[0].reads {
                read.request.paths = vec!["src/parser.rs".into()];
            }
        }
        if mode == 1 {
            input.episodes[0].reads[0].request.cwd = Some("/repo/one".into());
            input.episodes[0].reads[1].request.cwd = Some("/repo/two".into());
        }
        if mode == 2 {
            input.episodes[0].reads[1].request.extent.limit = Some(501);
        }
        if mode == 3 {
            input.episodes[0].reads[1]
                .result
                .as_mut()
                .unwrap()
                .returned_extent
                .as_mut()
                .unwrap()
                .end_inclusive = Some(501);
        }
        if mode == 4 {
            input.episodes[0].reads[1]
                .result
                .as_mut()
                .unwrap()
                .returned_extent = None;
        }
        if mode == 5 {
            let result_id = input.episodes[0].reads[1]
                .result
                .as_ref()
                .unwrap()
                .reference_id
                .clone();
            let action = input
                .events
                .iter_mut()
                .find(|action| action.reference.id == result_id)
                .unwrap();
            action.text.push_str("different full output");
            let result = input.episodes[0].reads[1].result.as_mut().unwrap();
            result.recorded_output_digest =
                crate::checks::ignored_instructions::sha256_hex(action.text.as_bytes());
            result.recorded_output_bytes = u64::try_from(action.text.len()).unwrap();
        }
        if mode == 6 {
            input.episodes[0].reads[1]
                .result
                .as_mut()
                .unwrap()
                .truncated = true;
        }
        if mode == 7 {
            input.episodes[0].reads[1].request.truncated = true;
        }
        if mode == 8 {
            let request_id = input.episodes[0].reads[1].request.reference_id.clone();
            input
                .events
                .iter_mut()
                .find(|action| action.reference.id == request_id)
                .unwrap()
                .truncated = true;
        }
        for read in &input.episodes[0].reads {
            for action in &mut input.events {
                if action.reference.id == read.request.reference_id {
                    action.metadata.read_request = Some(read.request.clone());
                }
                if read
                    .result
                    .as_ref()
                    .is_some_and(|result| result.reference_id == action.reference.id)
                {
                    action.metadata.read_result = read.result.clone();
                }
            }
        }
        let plan = plan(&input);
        assert!(
            plan.prepared
                .targets
                .values()
                .all(|target| target.reason != Reason::ExcessiveWithinFileReading),
            "mode {mode}"
        );
    }
}

#[test]
fn an_earlier_result_after_the_next_request_is_not_available_for_reuse() {
    let mut input = repeat_input(2);
    let later_request_id = input.episodes[0].reads[1].request.reference_id.clone();
    let earlier_result_id = input.episodes[0].reads[0]
        .result
        .as_ref()
        .unwrap()
        .reference_id
        .clone();
    let request_index = input
        .events
        .iter()
        .position(|event| event.reference.id == later_request_id)
        .unwrap();
    let result_index = input
        .events
        .iter()
        .position(|event| event.reference.id == earlier_result_id)
        .unwrap();
    input.events.swap(request_index, result_index);
    for (index, event) in input.events.iter_mut().enumerate() {
        event.reference.turn_index = u64::try_from(index).unwrap();
        event.reference.part_index = 0;
    }
    input.episodes[0].events = (1..input.events.len()).collect();
    let plan = plan(&input);
    assert!(
        plan.prepared
            .targets
            .values()
            .all(|target| target.reason != Reason::ExcessiveWithinFileReading)
    );
}

#[test]
fn visible_context_between_reads_breaks_the_repeat_interval() {
    for (kind, text, context_only, authority, turn_role) in [
        ("user", "Continue the task.", false, "user", "user"),
        (
            "assistant",
            "I will inspect this again.",
            false,
            "assistant",
            "assistant",
        ),
        ("assistant", "", true, "assistant", "assistant"),
        (
            "file_edit",
            "Changed the parser.",
            false,
            "assistant",
            "assistant",
        ),
        (
            "tool_input",
            "Run a different command.",
            false,
            "assistant",
            "assistant",
        ),
        (
            "tool_result",
            "Command completed.",
            false,
            "assistant",
            "assistant",
        ),
    ] {
        let mut input = repeat_input(2);
        let request_id = input.episodes[0].reads[1].request.reference_id.clone();
        let insertion = input
            .events
            .iter()
            .position(|event| event.reference.id == request_id)
            .unwrap();
        let mut breaker = input.events[insertion - 1].clone();
        breaker.reference.id = format!("interval-breaker-{kind}");
        breaker.reference.part_index += 1;
        breaker.kind = kind.into();
        breaker.authority = authority.into();
        breaker.turn_role = turn_role.into();
        breaker.context_only = context_only;
        breaker.text = text.into();
        breaker.metadata.read_request = None;
        breaker.metadata.read_result = None;
        for event in &mut input.events[insertion..] {
            event.reference.part_index += 1;
        }
        input.events.insert(insertion, breaker);
        input.episodes[0].events = (1..input.events.len()).collect();
        let plan = plan(&input);
        assert!(
            plan.prepared
                .targets
                .values()
                .all(|target| target.reason != Reason::ExcessiveWithinFileReading),
            "{kind}"
        );
    }
}

#[test]
fn a_closed_repeat_at_the_publication_fence_survives_an_open_unrelated_episode() {
    let input = closed_repeat_after_open_unrelated_input();
    assert_eq!(input.episodes[0].state, EpisodeState::Deferred);
    assert_eq!(input.episodes[1].state, EpisodeState::Complete);
    assert_eq!(
        input.events[*input.episodes[1].events.last().unwrap()]
            .reference
            .id,
        input.events.last().unwrap().reference.id
    );
    let plan = plan(&input);
    assert_eq!(plan.prepared.targets.len(), 1);
    assert_eq!(
        plan.prepared.targets.values().next().unwrap().reason,
        Reason::ExcessiveWithinFileReading
    );
    assert!(
        plan.prepared
            .unassessed
            .iter()
            .any(|item| item.episode_id == input.episodes[0].id
                && item.limitation == Abstention::DeferredEpisode)
    );
    let result = OverExploringCheck
        .reduce(&plan, &results(&plan, "likely_excess", 1.0), true)
        .unwrap();
    assert_eq!(result.findings.len(), 1);
    assert!(result.clean_episode_ids.is_empty());
}

#[test]
fn closed_repeat_interval_does_not_admit_open_unrelated_work() {
    let mut input = repeat_input(2);
    input.episodes[0].state = EpisodeState::Deferred;
    let plan = plan(&input);
    assert_eq!(plan.work_items.len(), 1);
    assert!(
        plan.prepared
            .targets
            .values()
            .all(|target| target.reason == Reason::ExcessiveWithinFileReading)
    );
    let result = OverExploringCheck
        .reduce(&plan, &results(&plan, "likely_excess", 1.0), true)
        .unwrap();
    assert_eq!(result.findings.len(), 1);
    assert!(result.clean_episode_ids.is_empty());
}

#[test]
fn probability_threshold_uncertainty_and_partial_answers_remain_distinct() {
    let plan = plan(&repeat_input(2));
    for (choice, probability, findings, clean) in [
        ("likely_excess", 0.94, 0, false),
        ("likely_excess", 0.95, 1, false),
        ("justified_or_minor", 0.96, 0, true),
        ("uncertain", 1.0, 0, false),
    ] {
        let result = OverExploringCheck
            .reduce(&plan, &results(&plan, choice, probability), true)
            .unwrap();
        assert_eq!(result.findings.len(), findings);
        assert_eq!(!result.clean_episode_ids.is_empty(), clean);
        assert_eq!(result.completed_work_item_ids.len(), 1);
    }
    let partial = OverExploringCheck.reduce(&plan, &[], false).unwrap();
    assert!(partial.completed_work_item_ids.is_empty());
    assert!(
        partial
            .unassessed
            .iter()
            .any(|item| item.limitation == Abstention::PartialAssessment)
    );
}

#[test]
fn malformed_answers_duplicate_results_and_old_revisions_are_rejected() {
    let mut plan = plan(&repeat_input(2));
    let valid = results(&plan, "likely_excess", 1.0);
    let mut duplicate = valid.clone();
    duplicate.extend(valid.clone());
    assert!(OverExploringCheck.reduce(&plan, &duplicate, true).is_err());
    let mut invalid = valid.clone();
    invalid[0].evidence.clear();
    assert!(OverExploringCheck.reduce(&plan, &invalid, true).is_err());
    let mut invalid = valid.clone();
    invalid[0].model = "different".into();
    assert!(OverExploringCheck.reduce(&plan, &invalid, true).is_err());
    plan.revisions.reducer -= 1;
    assert!(OverExploringCheck.reduce(&plan, &valid, true).is_err());
}

#[test]
fn fit_respects_question_criteria_bytes_state_and_runtime_limits() {
    let context = build_jev_context(&repeat_input(3)).unwrap();
    for mode in 0..5 {
        let mut capabilities = ModelCapabilities::jev_default();
        match mode {
            0 => {
                capabilities.questions_per_request =
                    CapabilityLimit::known(0, CapabilitySource::Manual)
            }
            1 => {
                capabilities.criteria_per_question =
                    CapabilityLimit::known(2, CapabilitySource::Manual)
            }
            2 => {
                capabilities.request_body_bytes =
                    CapabilityLimit::known(64, CapabilitySource::Manual)
            }
            3 => {
                capabilities.runtime_context_tokens =
                    CapabilityLimit::known(32, CapabilitySource::RuntimeMetadata)
            }
            4 => {
                capabilities.state_and_longest_question_tokens =
                    CapabilityLimit::known(32, CapabilitySource::Manual)
            }
            _ => unreachable!(),
        }
        let plan = OverExploringCheck
            .prepare_with_capabilities(&context, &capabilities)
            .unwrap();
        assert!(plan.work_items.is_empty(), "mode {mode}");
        assert!(plan.prepared.candidates.is_empty());
        assert!(packed(&plan).batches.is_empty());
        assert!(
            plan.prepared
                .unassessed
                .iter()
                .any(|item| item.limitation == Abstention::ContextTooLarge)
        );
    }
}

#[test]
fn verified_ollama_fit_includes_rendering_reserve_and_runtime_override() {
    let context = build_jev_context(&repeat_input(3)).unwrap();
    let mut capabilities = ModelCapabilities::jev_default();
    capabilities.use_ollama_tev1_accounting("chat envelope", "system text");
    capabilities.runtime_context_tokens =
        CapabilityLimit::known(2048, CapabilitySource::RuntimeMetadata);
    let plan = OverExploringCheck
        .prepare_with_capabilities(&context, &capabilities)
        .unwrap();
    assert_eq!(plan.work_items.len(), 1);
    let packed = packed(&plan);
    assert_eq!(packed.batches.len(), 1);
    let request = &packed.batches[0].request;
    validate_jev_request_with_capabilities(request, &capabilities).unwrap();
    let fit = estimate_jev_rendered_question_tokens(request, &capabilities).unwrap();
    assert!(fit <= MAX_ESTIMATED_INPUT_TOKENS);
    assert!(fit <= capabilities.usable_input_tokens().unwrap());
    let limits =
        capabilities.with_override(&crate::analysis::jev::capabilities::ModelLimitOverride {
            runtime_context_tokens: Some(128),
            ..Default::default()
        });
    assert!(
        OverExploringCheck
            .prepare_with_capabilities(&context, &limits)
            .unwrap()
            .work_items
            .is_empty()
    );
}

#[test]
fn tev1_repeat_reduction_fits_verified_limits_and_preserves_duplicate_finding() {
    let context = build_jev_context(&repeat_input(3)).unwrap();
    let mut capabilities = ModelCapabilities::jev_default();
    capabilities.use_ollama_tev1_accounting("chat envelope", "system text");
    capabilities.total_input_tokens =
        CapabilityLimit::known(2048, CapabilitySource::ProviderMetadata);
    capabilities.runtime_context_tokens =
        CapabilityLimit::known(2048, CapabilitySource::RuntimeMetadata);
    capabilities.state_and_longest_question_tokens =
        CapabilityLimit::known(2048, CapabilitySource::ProviderMetadata);
    capabilities.rendering_reserve_tokens = 1024;

    let plan = OverExploringCheck
        .prepare_with_capabilities(&context, &capabilities)
        .unwrap();
    assert_eq!(
        plan.work_items.len(),
        1,
        "unassessed={:?}",
        plan.prepared.unassessed
    );
    assert!(
        plan.work_items[0].window.fields["repeat"]["redundant_output_bytes"]
            .as_u64()
            .unwrap()
            >= 8192
    );
    assert_eq!(plan.work_items[0].window.fields["repeat"]["count"], 3);

    let packed = packed(&plan);
    assert_eq!(packed.batches.len(), 1);
    let request = &packed.batches[0].request;
    validate_jev_request_with_capabilities(request, &capabilities).unwrap();
    let request_bytes = serde_json::to_vec(request).unwrap().len();
    let request_tokens = estimate_jev_rendered_question_tokens(request, &capabilities).unwrap();
    assert!(
        request_tokens <= 1024,
        "bytes={request_bytes}, tokens={request_tokens}"
    );
    assert!(request_bytes < MAX_PACKED_REQUEST_BYTES);

    let assessment = OverExploringCheck
        .reduce(&plan, &results(&plan, "likely_excess", 1.0), true)
        .unwrap();
    assert_eq!(assessment.findings.len(), 1);
    assert_eq!(assessment.findings[0].reads.len(), 3);
    assert!(plan.prepared.validates_decision(&assessment.findings[0]));
}

#[test]
fn compact_tev1_repeat_keeps_purpose_and_exact_bindings() {
    for (task, purpose, choice, positive) in [
        (
            "Explain the parser boundary. Reuse available results.",
            "Inspect the parser.",
            "likely_excess",
            true,
        ),
        (
            "Read the parser three times to verify that the recorded outputs match.",
            "Compare the requested three outputs.",
            "justified_or_minor",
            false,
        ),
    ] {
        let mut input = fixture(
            task,
            &["/synthetic/workspace/src/parser.rs"; 3],
            &"pub fn boundary(value: usize) -> bool { value <= 10 }\n".repeat(200),
        );
        input
            .events
            .iter_mut()
            .find(|event| event.kind == "assistant")
            .unwrap()
            .text = purpose.into();
        let context = build_jev_context(&input).unwrap();
        let mut capabilities = ModelCapabilities::jev_default();
        capabilities.use_ollama_tev1_accounting("chat envelope", "system text");
        capabilities.total_input_tokens =
            CapabilityLimit::known(2048, CapabilitySource::ProviderMetadata);
        capabilities.runtime_context_tokens =
            CapabilityLimit::known(2048, CapabilitySource::RuntimeMetadata);
        capabilities.state_and_longest_question_tokens =
            CapabilityLimit::known(2048, CapabilitySource::ProviderMetadata);
        capabilities.rendering_reserve_tokens = 0;

        let initial = OverExploringCheck
            .prepare_with_capabilities(&context, &capabilities)
            .unwrap();
        assert_eq!(initial.work_items.len(), 1);
        let initial_request = &packed(&initial).batches[0].request;
        let initial_bytes = serde_json::to_vec(initial_request).unwrap().len();
        capabilities.rendering_reserve_tokens = 1024;
        capabilities.request_body_bytes = CapabilityLimit::known(
            u64::try_from(initial_bytes - 1).unwrap(),
            CapabilitySource::Manual,
        );
        let plan = OverExploringCheck
            .prepare_with_capabilities(&context, &capabilities)
            .unwrap();
        assert_eq!(plan.work_items.len(), 1, "{:?}", plan.prepared.unassessed);
        let item = &plan.work_items[0];
        assert_eq!(item.window.fields["purpose"]["text"], purpose);
        let purpose_reference = item
            .window
            .evidence
            .iter()
            .find(|reference| reference.part_id == "purpose.text")
            .unwrap();
        assert_eq!(purpose_reference.role, JevEvidenceRole::SupportingContext);
        assert_eq!(
            purpose_reference.source_id,
            input
                .events
                .iter()
                .find(|event| event.kind == "assistant")
                .unwrap()
                .reference
                .id
        );
        let JevQuestion::Choice {
            instructions,
            criteria,
        } = &item.questions[QUESTION_ID]
        else {
            panic!("repeat questions must be choices");
        };
        assert!(
            instructions["question"]
                .as_str()
                .unwrap()
                .contains("earlier identical result")
        );
        assert_eq!(criteria.len(), 3);

        let packed = packed(&plan);
        assert_eq!(packed.batches.len(), 1);
        let request = &packed.batches[0].request;
        validate_jev_request_with_capabilities(request, &capabilities).unwrap();
        let request_bytes = serde_json::to_vec(request).unwrap().len();
        let request_tokens = estimate_jev_rendered_question_tokens(request, &capabilities).unwrap();
        assert!(
            request_tokens <= 1024,
            "bytes={request_bytes}, tokens={request_tokens}"
        );
        assert!(request_bytes <= capabilities.request_body_bytes.value.unwrap() as usize);
        assert!(request.state["work_items"][0]["context"]["purpose"]["text"] == purpose);

        let response = results(&plan, choice, 1.0);
        let mut expected_evidence = plan.shared_context.as_ref().unwrap().evidence.clone();
        expected_evidence.extend(item.window.evidence.clone());
        assert_eq!(response[0].evidence, expected_evidence);
        let assessment = OverExploringCheck.reduce(&plan, &response, true).unwrap();
        let target = &plan.prepared.targets[&item.id];
        assert_eq!(target.bindings.len(), 3);
        let episode = plan
            .prepared
            .episodes
            .iter()
            .find(|episode| episode.id == target.episode_id)
            .unwrap();
        for (binding, read_index) in target.bindings.iter().zip(&target.read_indexes) {
            let read = &episode.reads[*read_index];
            assert_eq!(binding.request_id, read.request.reference_id);
            assert_eq!(
                binding.result_id,
                read.result
                    .as_ref()
                    .map(|result| result.reference_id.clone())
            );
            assert_eq!(
                binding.output_digest,
                read.result
                    .as_ref()
                    .map(|result| result.recorded_output_digest.clone())
            );
        }
        assert_eq!(assessment.findings.len(), usize::from(positive));
        assert_eq!(assessment.clean_episode_ids.is_empty(), positive);
        if positive {
            assert_eq!(assessment.findings[0].reads, target.bindings);
            assert!(plan.prepared.validates_decision(&assessment.findings[0]));
        }
    }
}

#[test]
fn constrained_singleton_shortens_header_and_preserves_required_task() {
    let input = fixture(
        "Explain the parser boundary.",
        &["/repo/notes/garden.txt"],
        &"Garden guide: plant seeds and water the seedlings.\n".repeat(300),
    );
    let normal = plan(&input);
    let normal_packed = packed(&normal);
    let mut capabilities = normal.capabilities.clone();
    capabilities.request_body_bytes = CapabilityLimit::known(
        u64::try_from(normal_packed.batches[0].serialized_bytes - 150).unwrap(),
        CapabilitySource::Manual,
    );
    let reduced = OverExploringCheck
        .prepare_with_capabilities(&build_jev_context(&input).unwrap(), &capabilities)
        .unwrap();
    assert_eq!(reduced.work_items.len(), 1);
    assert!(
        reduced.work_items[0].window.fields["read"]["sample"]["text"]
            .as_str()
            .unwrap()
            .len()
            < normal.work_items[0].window.fields["read"]["sample"]["text"]
                .as_str()
                .unwrap()
                .len()
    );
    assert_eq!(reduced.shared_context, normal.shared_context);
    let packed = packed(&reduced);
    assert_eq!(packed.batches.len(), 1);
    assert!(packed.skipped_item_ids.is_empty());
    validate_jev_request_with_capabilities(&packed.batches[0].request, &capabilities).unwrap();
    assert_eq!(
        OverExploringCheck
            .reduce(&reduced, &results(&reduced, "likely_excess", 1.0), true)
            .unwrap()
            .findings
            .len(),
        1
    );
}

#[test]
fn reduced_header_uses_measured_capacity_for_a_complete_line_over_128_bytes() {
    let first = format!("Garden manual: {}\n", "soil ".repeat(30));
    assert!(first.len() > 128);
    let body = format!("{first}{}", "Plant seeds, prepare compost, water seedlings, and label each vegetable bed for the growing season.\n".repeat(200));
    let input = fixture("Explain the parser.", &["/repo/garden.txt"], &body);
    let normal = plan(&input);
    let mut limits = normal.capabilities.clone();
    limits.request_body_bytes = CapabilityLimit::known(
        u64::try_from(packed(&normal).batches[0].serialized_bytes - 180).unwrap(),
        CapabilitySource::Manual,
    );
    let reduced = OverExploringCheck
        .prepare_with_capabilities(&build_jev_context(&input).unwrap(), &limits)
        .unwrap();
    assert_eq!(reduced.work_items.len(), 1);
    let sample = reduced.work_items[0].window.fields["read"]["sample"]["text"]
        .as_str()
        .unwrap();
    assert!(sample.len() > 128);
    assert!(sample.ends_with('\n'));
    assert!(
        sample.len()
            < normal.work_items[0].window.fields["read"]["sample"]["text"]
                .as_str()
                .unwrap()
                .len()
    );
    let request = &packed(&reduced).batches[0].request;
    validate_jev_request_with_capabilities(request, &limits).unwrap();
    let result = OverExploringCheck
        .reduce(&reduced, &results(&reduced, "likely_excess", 1.0), true)
        .unwrap();
    assert!(reduced.prepared.validates_decision(&result.findings[0]));
}

#[test]
fn unicode_and_escaped_samples_keep_complete_lines_and_exact_source_ranges() {
    let input = fixture(
        "Explain the parser.",
        &["/repo/notes/garden.txt"],
        &"\"é界\\sample\"\n".repeat(900),
    );
    let plan = plan(&input);
    assert_eq!(plan.work_items.len(), 1);
    let result = OverExploringCheck
        .reduce(&plan, &results(&plan, "likely_excess", 1.0), true)
        .unwrap();
    let snippets = &result.findings[0].explanation.as_ref().unwrap().snippets;
    assert!(result.findings[0].has_valid_explanation());
    assert_eq!(snippets.len(), 1);
    assert!(snippets[0].text.len() <= 512);
    assert!(snippets[0].text.ends_with('\n'));
    assert!(
        input
            .events
            .iter()
            .any(|action| snippets[0].matches_action(action))
    );
    let packed = packed(&plan);
    assert_eq!(packed.batches.len(), 1);
    assert!(packed.batches[0].serialized_bytes <= MAX_PACKED_REQUEST_BYTES);
    validate_jev_request_with_capabilities(&packed.batches[0].request, &plan.capabilities).unwrap();
    assert!(result.clean_episode_ids.is_empty());
}

#[test]
fn oversized_complete_line_and_task_units_are_not_clipped() {
    for (task, body) in [
        (
            "Explain the parser.".to_owned(),
            format!("{}\n", "界".repeat(4000)),
        ),
        (
            "Explain the parser. ".repeat(40),
            "garden guide\n".repeat(1000),
        ),
    ] {
        let input = fixture(&task, &["/repo/garden.txt"], &body);
        let plan = plan(&input);
        assert!(plan.work_items.is_empty());
        assert!(packed(&plan).batches.is_empty());
        assert!(
            OverExploringCheck
                .reduce(&plan, &[], true)
                .unwrap()
                .clean_episode_ids
                .is_empty()
        );
    }
}

#[test]
fn small_repeats_make_no_requests() {
    let input = fixture(
        "Explain the parser.",
        &["/repo/parser.rs"; 3],
        "small parser\n",
    );
    let plan = plan(&input);
    assert!(plan.work_items.is_empty());
    assert!(plan.prepared.candidates.is_empty());
    assert!(packed(&plan).batches.is_empty());
    assert!(
        OverExploringCheck
            .reduce(&plan, &[], true)
            .unwrap()
            .clean_episode_ids
            .is_empty()
    );
}

#[test]
fn escaped_task_metadata_obeys_serialized_byte_caps() {
    let task = format!("Explain the parser. {}", "\\\"".repeat(130));
    assert!(task.len() < 384);
    let input = fixture(
        &task,
        &["/repo/parser.rs"; 2],
        &"validate the boundary\n".repeat(500),
    );
    assert!(
        serde_json::to_vec(&input.task_context.fields)
            .unwrap()
            .len()
            > 384
    );
    let plan = plan(&input);
    assert!(plan.work_items.is_empty());
    assert!(packed(&plan).batches.is_empty());
    assert!(
        plan.prepared
            .unassessed
            .iter()
            .any(|item| item.limitation == Abstention::SourceLimited)
    );
}

#[test]
fn file_beginning_uses_the_recorded_extent_unit() {
    use crate::analysis::jev_evidence::JevReadUnit;
    let mut result = repeat_input(2).episodes[0].reads[0].result.clone().unwrap();
    for (unit, offset, expected) in [
        (JevReadUnit::Lines, 1, true),
        (JevReadUnit::Lines, 0, false),
        (JevReadUnit::Bytes, 0, true),
        (JevReadUnit::Bytes, 1, false),
        (JevReadUnit::Unknown, 0, false),
    ] {
        let extent = result.returned_extent.as_mut().unwrap();
        extent.unit = unit;
        extent.offset = Some(offset);
        assert_eq!(
            super::episodes::read_starts_at_file_beginning(&result),
            expected
        );
        assert_eq!(
            has_known_returned_extent(&result),
            unit != JevReadUnit::Unknown
        );
    }
}

#[test]
fn tag_like_text_in_an_unframed_result_is_not_removed() {
    let mut input = fixture(
        "Explain the parser.",
        &["/repo/garden.txt"],
        &"Garden manual.\n".repeat(1000),
    );
    let result = input.episodes[0].reads[0].result.as_mut().unwrap();
    let action = input
        .events
        .iter_mut()
        .find(|action| action.reference.id == result.reference_id)
        .unwrap();
    action.text = format!(
        "Original file content contains tag examples.\n<content>\n{}",
        "Garden guide: plant seeds.\n".repeat(500)
    );
    result.extent_contract = None;
    result.recorded_output_digest =
        crate::checks::ignored_instructions::sha256_hex(action.text.as_bytes());
    result.recorded_output_bytes = u64::try_from(action.text.len()).unwrap();
    action.metadata.read_result = Some(result.clone());
    let plan = plan(&input);
    let sample = plan.work_items[0].window.fields["read"]["sample"]["text"]
        .as_str()
        .unwrap();
    assert!(sample.starts_with("Original file content"));
    let finding = OverExploringCheck
        .reduce(&plan, &results(&plan, "likely_excess", 1.0), true)
        .unwrap()
        .findings
        .remove(0);
    assert_eq!(finding.explanation.as_ref().unwrap().snippets[0].range.0, 0);
    assert!(plan.prepared.validates_decision(&finding));
}

#[test]
fn task_purpose_and_header_samples_remain_source_bound() {
    let input = fixture(
        "Explain the parser boundary in src/parser.rs. Do not research gardening.",
        &["/repo/notes/garden.txt"],
        &"Garden guide: prepare compost and water seedlings.\n".repeat(500),
    );
    let plan = plan(&input);
    let finding = OverExploringCheck
        .reduce(&plan, &results(&plan, "likely_excess", 1.0), true)
        .unwrap()
        .findings
        .remove(0);
    let basis = finding.explanation.as_ref().unwrap();
    let task = basis.task.evidence.first().unwrap();
    let task_action = input
        .events
        .iter()
        .find(|action| action.reference.id == task.source_id)
        .unwrap();
    assert_eq!(task_action.kind, "user");
    assert!(basis.task.fields.to_string().contains(&task_action.text));

    let purpose = basis.purpose.as_ref().unwrap();
    assert_eq!(purpose.text, "Inspect the parser and its dependencies.");
    assert!(
        purpose.matches_action(
            input
                .events
                .iter()
                .find(|action| action.reference.id == purpose.reference.id)
                .unwrap()
        )
    );
    assert!(basis.compared.evidence.iter().any(|reference| {
        reference.part_id == "purpose.text"
            && reference.source_id == purpose.reference.id
            && reference.role == JevEvidenceRole::SupportingContext
    }));

    let sample = &basis.snippets[0];
    assert!(!sample.text.is_empty());
    assert!(sample.text.ends_with('\n'));
    assert!(
        sample.matches_action(
            input
                .events
                .iter()
                .find(|action| action.reference.id == sample.reference.id)
                .unwrap()
        )
    );
    assert!(basis.compared.evidence.iter().any(|reference| {
        reference.part_id == "read.sample.text" && reference.source_id == sample.reference.id
    }));
    assert!(finding.source_evidence.iter().any(|reference| {
        reference.part_id == "purpose.text" && reference.source_id == purpose.reference.id
    }));
    assert!(plan.prepared.validates_decision(&finding));
}

#[test]
fn overlong_or_truncated_read_purpose_is_not_clipped_into_a_candidate() {
    for reason in [Reason::ExcessiveWithinFileReading, Reason::UnrelatedFiles] {
        for mode in 0..2 {
            let mut input = if reason == Reason::ExcessiveWithinFileReading {
                repeat_input(2)
            } else {
                fixture(
                    "Explain the parser boundary.",
                    &["/repo/notes/garden.txt"],
                    &"Garden guide: prepare compost and water seedlings.\n".repeat(300),
                )
            };
            let purpose = input
                .events
                .iter_mut()
                .find(|action| action.kind == "assistant")
                .unwrap();
            if mode == 0 {
                purpose.text = "Inspect the parser. ".repeat(12);
                assert!(purpose.text.len() > 192);
            } else {
                purpose.truncated = true;
            }
            let plan = plan(&input);
            assert!(plan.work_items.is_empty(), "reason={reason:?}, mode={mode}");
            assert!(packed(&plan).batches.is_empty());
            assert!(
                OverExploringCheck
                    .reduce(&plan, &[], true)
                    .unwrap()
                    .clean_episode_ids
                    .is_empty()
            );
        }
    }
}

#[test]
fn short_approval_cannot_replace_its_missing_antecedent_task() {
    let (content, task) = source(vec![
        part(
            0,
            ContentKind::UserText,
            "Audit the parser. Keep billing unchanged.",
            None,
        ),
        part(
            1,
            ContentKind::AssistantText,
            "Inspect the parser and tests?",
            None,
        ),
        part(2, ContentKind::UserText, "Yes, continue.", None),
        part(
            3,
            ContentKind::ToolInput,
            r#"{"filePath":"/repo/parser.rs","limit":2}"#,
            Some("audit"),
        ),
        part(4, ContentKind::ToolResult, "1: parser code", Some("audit")),
    ]);
    let span = EpisodeSpan {
        first_event_id: content.actions[2].reference.id.clone(),
        last_event_id: content.actions[4].reference.id.clone(),
        state: EpisodeState::Complete,
    };
    let input = build_episodes(&content, &task, &[span]).unwrap();
    assert_eq!(
        input.task_context.fields["antecedent_context_omitted"],
        true
    );
    let plan = plan(&input);
    assert!(plan.work_items.is_empty());
    assert!(
        plan.prepared
            .unassessed
            .iter()
            .any(|item| item.limitation == Abstention::SourceLimited)
    );
}

#[test]
fn source_and_task_changes_invalidate_target_identity() {
    let input = repeat_input(2);
    let original = plan(&input);
    let mut changed = input.clone();
    changed.task_contexts.values_mut().next().unwrap().fields["values"] =
        json!(["Verify both reads explicitly."]);
    changed.task_context = changed.task_contexts.values().next().unwrap().clone();
    let changed = plan(&changed);
    assert_ne!(original.work_items[0].id, changed.work_items[0].id);
    assert_ne!(original.prepared.epoch, changed.prepared.epoch);
}

#[test]
fn explanation_contract_rejects_changed_bindings_sample_and_run_facts() {
    for input in [
        repeat_input(3),
        fixture(
            "Explain the parser.",
            &["/repo/garden.txt"],
            &"Garden guide: plant seeds.\n".repeat(500),
        ),
    ] {
        let plan = plan(&input);
        let finding = OverExploringCheck
            .reduce(&plan, &results(&plan, "likely_excess", 1.0), true)
            .unwrap()
            .findings
            .remove(0);
        assert!(finding.has_valid_explanation());
        for mode in 0..5 {
            let mut changed = finding.clone();
            let basis = changed.explanation.as_mut().unwrap();
            match mode {
                0 => basis.version = 1,
                1 => basis.reads[0].request.reference_id = "wrong".into(),
                2 => basis.task.fields["partial"] = json!(true),
                3 if changed.reason == Reason::UnrelatedFiles => {
                    basis.compared.fields["read"]["sample"] = json!("different sample")
                }
                3 => basis.compared.fields["repeat"]["count"] = json!(1),
                4 => basis.compared.fields["purpose"]["text"] = json!("different purpose"),
                _ => unreachable!(),
            }
            assert!(!changed.has_valid_explanation(), "mode {mode}");
        }
    }
}

#[tokio::test]
async fn shared_task_snapshot_runs_once_and_reuses_accepted_answers() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let context = build_jev_context(&repeat_input(3)).unwrap();
    let calls = AtomicUsize::new(0);
    let outcome = run_jev_check(
        &OverExploringCheck,
        &context,
        JevRunProgress::default(),
        |batch| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move {
                assert!(batch.serialized_bytes <= MAX_PACKED_REQUEST_BYTES);
                let answers = batch
                    .request
                    .questions
                    .keys()
                    .map(|id| {
                        (
                            id.clone(),
                            JevAnswer::Choice {
                                choice: "likely_excess".into(),
                                confidence: 1.0,
                                probabilities: BTreeMap::from([
                                    ("likely_excess".into(), 1.0),
                                    ("justified_or_minor".into(), 0.0),
                                    ("uncertain".into(), 0.0),
                                ]),
                            },
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
            }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(outcome.result.findings.len(), 1);
    let resumed = run_jev_check(
        &OverExploringCheck,
        &context,
        outcome.progress,
        |_| async { panic!("accepted answer must not dispatch again") },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(resumed.result.findings.len(), 1);
}
