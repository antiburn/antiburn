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
            projection: 9,
            chunking: 8,
            questions: 19,
            reducer: 8
        }
    );
}

fn small_capabilities(tokens: u64) -> ModelCapabilities {
    let mut capabilities = ModelCapabilities::jev_default();
    capabilities.runtime_context_tokens.value = Some(tokens);
    capabilities.rendering_reserve_tokens = 1024;
    capabilities
}

#[tokio::test]
async fn compact_selected_pairs_run_through_the_production_boundary() {
    for tokens in [2048, 2050] {
        let input = input(&["parser.rs", "parser.rs"]);
        let context = build_jev_context(&input).unwrap();
        let capabilities = small_capabilities(tokens);
        let orchestration = admit_jev_orchestration().await.unwrap();
        let mut plan = OverExploringCheck
            .prepare_with_capabilities(&context, &capabilities)
            .unwrap();
        let pair_id = plan
            .prepared
            .targets
            .iter()
            .find(|(_, target)| {
                target.reason == Reason::ExcessiveWithinFileReading && target.bindings.len() == 2
            })
            .unwrap()
            .0
            .clone();
        let mut progress = SamplingProgress::new(SamplingLimits {
            checks: 1,
            candidates_per_check: MAX_SAMPLING_CANDIDATES,
            answers_per_candidate: 1,
            judgments_per_run: 8,
        })
        .unwrap();
        synchronize_sampling(&plan, &mut progress).unwrap();
        progress.begin_run();
        while let Some(job) = progress.choose_job() {
            if plan
                .prepared
                .candidates
                .iter()
                .find(|candidate| candidate.candidate_id == job.candidate)
                .unwrap()
                .work_item_ids
                .contains(&pair_id)
            {
                PreparedAssessment::select_jobs(&mut plan, &[job]).unwrap();
                break;
            }
        }
        assert!(plan.shared_context.is_none());
        let outcome = run_jev_check_prepared(
            &OverExploringCheck,
            &context,
            &mut plan,
            JevRunProgress::default(),
            orchestration,
            |batch| async move {
                assert!(batch.request.state.get("shared_context").is_none());
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
        for finding in &outcome.result.findings {
            for binding in &finding.reads {
                assert!(
                    finding
                        .source_evidence
                        .iter()
                        .any(|reference| reference.source_id == binding.request_id)
                );
                assert!(
                    finding
                        .source_evidence
                        .iter()
                        .any(|reference| Some(&reference.source_id) == binding.result_id.as_ref())
                );
            }
            for snippet in &finding.explanation.as_ref().unwrap().snippets {
                assert!(
                    snippet.matches_action(
                        input
                            .events
                            .iter()
                            .find(|action| action.reference == snippet.reference)
                            .unwrap()
                    )
                );
            }
        }
    }
}

#[tokio::test]
async fn compact_relevance_extent_and_breadth_use_the_production_runner() {
    for tokens in [2048, 2050] {
        for paths in [vec!["parser.rs"], vec!["parser.rs", "library.rs"]] {
            let input = input(&paths);
            let outcome = run_jev_check_with_capabilities(
                &OverExploringCheck,
                &build_jev_context(&input).unwrap(),
                JevRunProgress::default(),
                small_capabilities(tokens),
                |batch| async move {
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
            if paths.len() > 1 {
                assert!(
                    outcome
                        .result
                        .findings
                        .iter()
                        .any(|finding| finding.reason == Reason::UnrelatedFiles)
                );
            }
            assert!(
                outcome
                    .result
                    .findings
                    .iter()
                    .any(|finding| finding.reason == Reason::ExcessiveWithinFileReading)
            );
            if paths.len() > 1 {
                assert!(
                    outcome
                        .result
                        .findings
                        .iter()
                        .any(|finding| finding.reason == Reason::ExcessiveFileBreadth)
                );
            }
        }
    }
}

#[test]
fn initial_requested_seven_line_source_stays_separate_from_later_read_targets() {
    let text = "fn accepts(value: u32, maximum: u32) -> bool {\n    value < maximum\n}\n#[test]\nfn boundary() {\n    assert!(accepts(10, 10));\n}";
    let (mut content, task, span) = fixture(
        "Fix src/cache_age.rs so accepts(10, 10) is true.",
        &["src/cache_age.rs"; 3],
        text,
        "Correct the comparison after diagnosis.",
    );
    let mut diagnosis = content.actions[1].clone();
    diagnosis.reference.id = "diagnosis-after-first-read".into();
    diagnosis.text = "Diagnosis: change < to <=.".into();
    content.actions.insert(4, diagnosis);
    for (index, action) in content.actions.iter_mut().enumerate() {
        action.reference.turn_index = index as u64;
    }
    let input = build_episodes(&content, &task, &[span]).unwrap();
    let mut capabilities = small_capabilities(2048);
    capabilities.use_ollama_generic_accounting("", "");
    capabilities.rendering_reserve_tokens = 302;
    capabilities.runtime_context_tokens.value = Some(2200);
    let plan = OverExploringCheck
        .prepare_with_capabilities(&build_jev_context(&input).unwrap(), &capabilities)
        .unwrap();
    {
        let reason = Reason::ExcessiveWithinFileReading;
        let initial = plan
            .prepared
            .targets
            .iter()
            .find(|(_, target)| target.reason == reason && target.read_indexes == vec![0])
            .unwrap();
        let item = plan
            .work_items
            .iter()
            .find(|item| item.id == *initial.0)
            .unwrap();
        assert_eq!(item.window.fields["reads"].as_array().unwrap().len(), 1);
        assert!(item.window.fields.to_string().contains("fn accepts"));
        let question = serde_json::to_string(&item.questions).unwrap();
        assert!(question.contains("task objective") || question.contains("requested extent"));
        assert!(
            question.contains("no diagnosis hindsight") || reason == Reason::ExcessiveFileBreadth
        );
    }
    let repeat_id = plan
        .prepared
        .targets
        .iter()
        .find(|(_, target)| {
            target.reason == Reason::ExcessiveWithinFileReading && target.read_indexes.len() == 2
        })
        .unwrap()
        .0
        .clone();
    let mut progress = SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: MAX_SAMPLING_CANDIDATES,
        answers_per_candidate: 1,
        judgments_per_run: 32,
    })
    .unwrap();
    synchronize_sampling(&plan, &mut progress).unwrap();
    progress.begin_run();
    let mut plan = plan;
    while let Some(job) = progress.choose_job() {
        if plan
            .prepared
            .candidates
            .iter()
            .find(|candidate| candidate.candidate_id == job.candidate)
            .unwrap()
            .work_item_ids
            .contains(&repeat_id)
        {
            PreparedAssessment::select_jobs(&mut plan, &[job]).unwrap();
            break;
        }
    }
    let item = plan
        .work_items
        .iter()
        .find(|item| item.id == repeat_id)
        .unwrap();
    assert!(
        item.window.fields["intervening"]
            .to_string()
            .contains("Diagnosis")
    );
    assert!(
        serde_json::to_string(&item.questions)
            .unwrap()
            .contains("LATER read")
    );
}

#[test]
fn every_read_reason_uses_the_explicit_task_objective_without_blanket_first_read_protection() {
    let input = input(&[
        "src/cache_age.rs",
        "docs/unrelated-guide.md",
        "src/cache_age.rs",
    ]);
    let plan = OverExploringCheck
        .prepare(&build_jev_context(&input).unwrap())
        .unwrap();
    for reason in [
        Reason::UnrelatedFiles,
        Reason::ExcessiveFileBreadth,
        Reason::ExcessiveWithinFileReading,
    ] {
        let target = plan
            .prepared
            .targets
            .iter()
            .find(|(_, target)| target.reason == reason)
            .expect("reason has a selected target");
        let item = plan.work_items.iter().find(|item| item.id == *target.0);
        if reason == Reason::ExcessiveFileBreadth && item.is_none() {
            continue;
        }
        let Some(item) = item else {
            continue;
        };
        let prompt = serde_json::to_string(&item.questions).unwrap();
        assert!(
            prompt.contains("relevance baseline") || reason == Reason::ExcessiveFileBreadth,
            "{reason:?}: {prompt}"
        );
        assert!(
            prompt.contains("not every first read") || reason == Reason::ExcessiveFileBreadth,
            "{reason:?}: {prompt}"
        );
        assert!(
            prompt.contains("Never infer scope") || reason == Reason::ExcessiveFileBreadth,
            "{reason:?}: {prompt}"
        );
        if reason == Reason::UnrelatedFiles {
            let first = plan
                .prepared
                .targets
                .iter()
                .find(|(_, candidate)| {
                    candidate.reason == reason && candidate.read_indexes == vec![0]
                })
                .unwrap();
            let first_item = plan.work_items.iter().find(|item| item.id == *first.0);
            if let Some(first_item) = first_item {
                assert!(
                    serde_json::to_string(&first_item.window.fields)
                        .unwrap()
                        .contains("src/cache_age.rs")
                );
            }
        }
    }
}

#[test]
fn task_named_initial_cache_age_read_cannot_be_unrelated_but_detours_and_extent_remain_assessable()
{
    let source = "fn accepts_age(value: u32, maximum: u32) -> bool {\n    value < maximum\n}\n#[test]\nfn equal_age_is_accepted() {\n    assert!(accepts_age(10, 10));\n}";
    let (content, task, span) = fixture(
        "Fix src/cache_age.rs so accepts_age(10, 10) returns true.",
        &[
            "src/cache_age.rs",
            "docs/unrelated-guide.md",
            "src/cache_age.rs",
        ],
        source,
        "Change the boundary comparison after diagnosis.",
    );
    let input = build_episodes(&content, &task, &[span]).unwrap();
    let context = build_jev_context(&input).unwrap();
    let plan = OverExploringCheck
        .prepare_with_capabilities(&context, &small_capabilities(2050))
        .unwrap();
    let first_named = plan
        .prepared
        .targets
        .iter()
        .find(|(_, target)| {
            target.reason == Reason::ExcessiveWithinFileReading && target.read_indexes == vec![0]
        })
        .unwrap();
    let first_named_id = first_named.0.clone();
    let mut plan = plan;
    let mut progress = SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: MAX_SAMPLING_CANDIDATES,
        answers_per_candidate: 1,
        judgments_per_run: 16,
    })
    .unwrap();
    synchronize_sampling(&plan, &mut progress).unwrap();
    progress.begin_run();
    while let Some(job) = progress.choose_job() {
        if plan
            .prepared
            .candidates
            .iter()
            .find(|candidate| candidate.candidate_id == job.candidate)
            .unwrap()
            .work_item_ids
            .contains(&first_named_id)
        {
            PreparedAssessment::select_jobs(&mut plan, &[job]).unwrap();
            break;
        }
    }
    assert!(plan.work_items.iter().any(|item| item.id == first_named_id));
    assert!(!plan.prepared.targets.values().any(|target| target.reason == Reason::UnrelatedFiles && target.read_indexes == vec![0]));
    assert!(plan.prepared.targets.values().any(|target| target.reason == Reason::UnrelatedFiles && target.read_indexes == vec![1]));
    let first_item = plan
        .work_items
        .iter()
        .find(|item| item.id == first_named_id)
        .unwrap();
    assert_eq!(
        first_item.window.fields["reads"][0]["task_named_path"],
        json!(true)
    );
    let repeat = plan
        .prepared
        .targets
        .iter()
        .find(|(_, target)| {
            target.reason == Reason::ExcessiveWithinFileReading && target.read_indexes == vec![0, 2]
        })
        .unwrap();
    assert_eq!(repeat.1.bindings.len(), 2);
    let mut answers = results(&plan, "likely_excess", 0.98);
    let named_extent = answers
        .iter_mut()
        .find(|answer| answer.work_item_id == first_named_id)
        .unwrap();
    named_extent
        .answers
        .insert(QUESTION_ID.into(), answer("likely_excess", 0.89));
    let findings = OverExploringCheck
        .reduce(&plan, &answers, false)
        .unwrap()
        .findings;
    assert!(
        findings
            .iter()
            .all(|finding| !(finding.reason == Reason::UnrelatedFiles
                && finding.reads[0].request_id == input.episodes[0].reads[0].request.reference_id))
    );
    assert!(
        findings
            .iter()
            .all(|finding| finding.work_item_id != first_named_id)
    );
    let strong = OverExploringCheck
        .reduce(&plan, &results(&plan, "likely_excess", 0.98), false)
        .unwrap();
    assert!(
        strong
            .findings
            .iter()
            .any(|finding| finding.work_item_id == first_named_id
                && finding.reason == Reason::ExcessiveWithinFileReading)
    );
}

#[test]
fn intervening_edit_references_address_filtered_entries_and_reconstruct_exact_source() {
    let (mut content, task, span) = fixture(
        "Fix parser.rs.",
        &["parser.rs", "parser.rs"],
        "fn parse() {}",
        "Verify the edit.",
    );
    let mut edit = content.actions[2].clone();
    edit.reference.id = "intervening-parser-edit".into();
    edit.metadata.read_request = None;
    edit.text = r#"{"filePath":"parser.rs","oldString":"fn parse() {}","newString":"fn parse() { validate(); }"}"#.into();
    let edit_reference = edit.reference.id.clone();
    content.actions.insert(4, edit);
    for (index, action) in content.actions.iter_mut().enumerate() {
        action.reference.turn_index = index as u64;
    }
    let input = build_episodes(&content, &task, &[span]).unwrap();
    let mut capabilities = small_capabilities(2050);
    capabilities.use_ollama_generic_accounting("", "");
    capabilities.rendering_reserve_tokens = 302;
    capabilities.runtime_context_tokens.value = Some(2200);
    let mut plan = OverExploringCheck
        .prepare_with_capabilities(&build_jev_context(&input).unwrap(), &capabilities)
        .unwrap();
    let pair_id = plan
        .prepared
        .targets
        .iter()
        .find(|(_, target)| {
            target.reason == Reason::ExcessiveWithinFileReading && target.read_indexes == vec![0, 1]
        })
        .unwrap()
        .0
        .clone();
    let mut progress = SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: MAX_SAMPLING_CANDIDATES,
        answers_per_candidate: 1,
        judgments_per_run: 8,
    })
    .unwrap();
    synchronize_sampling(&plan, &mut progress).unwrap();
    progress.begin_run();
    while let Some(job) = progress.choose_job() {
        if plan
            .prepared
            .candidates
            .iter()
            .find(|candidate| candidate.candidate_id == job.candidate)
            .unwrap()
            .work_item_ids
            .contains(&pair_id)
        {
            PreparedAssessment::select_jobs(&mut plan, &[job]).unwrap();
            break;
        }
    }
    let restored: JevCheckPlan<PreparedAssessment> = serde_json::from_value(json!(&plan)).unwrap();
    let item = restored
        .work_items
        .iter()
        .find(|item| item.id == pair_id)
        .unwrap();
    let reference = item
        .window
        .evidence
        .iter()
        .find(|reference| reference.source_id == edit_reference)
        .unwrap();
    assert_eq!(reference.part_id, "intervening[0]");
    let selected = &item.window.fields["intervening"][0];
    assert_eq!(selected["at"], json!(4));
    let start = selected["range"][0].as_u64().unwrap() as usize;
    let end = selected["range"][1].as_u64().unwrap() as usize;
    assert_eq!(selected["text"], json!(&input.events[4].text[start..end]));
    let assessment = OverExploringCheck
        .reduce(&restored, &results(&restored, "likely_excess", 0.9), false)
        .unwrap();
    let finding = &assessment.findings[0];
    assert!(finding.source_evidence.contains(reference));
    let snippet = finding
        .explanation
        .as_ref()
        .unwrap()
        .snippets
        .iter()
        .find(|snippet| snippet.reference.id == edit_reference)
        .unwrap();
    assert!(snippet.matches_action(&input.events[4]));
    assert_eq!(snippet.range, (start, end));
    assert_eq!(finding.reads.len(), 2);
}

#[test]
fn small_windows_dispatch_and_persist_paired_exact_excerpts() {
    for tokens in [2048, 2050] {
        let input = input(&["parser.rs"]);
        let plan = OverExploringCheck
            .prepare_with_capabilities(
                &build_jev_context(&input).unwrap(),
                &small_capabilities(tokens),
            )
            .unwrap();
        assert_eq!(
            plan.work_items.len(),
            2,
            "{tokens}: {:?}",
            plan.skipped_item_ids
        );
        let assessment = OverExploringCheck
            .reduce(&plan, &results(&plan, "likely_excess", 0.9), false)
            .unwrap();
        assert_eq!(assessment.findings.len(), 2);
        for decision in &assessment.findings {
            let explanation = decision.explanation.as_ref().unwrap();
            for snippet in &explanation.snippets {
                let action = input
                    .events
                    .iter()
                    .find(|action| action.reference == snippet.reference)
                    .unwrap();
                assert!(snippet.matches_action(action));
            }
            assert_eq!(explanation.reads.len(), 1);
            assert_eq!(
                explanation.reads[0].request.reference_id,
                decision.reads[0].request_id
            );
            assert!(!decision.task_evidence.is_empty());
            let result = &explanation.compared.fields["reads"][0]["result"];
            let source_index = result["at"].as_u64().unwrap() as usize;
            let start = result["range"][0].as_u64().unwrap() as usize;
            let end = result["range"][1].as_u64().unwrap() as usize;
            assert_eq!(
                result["text"],
                json!(&input.events[source_index].text[start..end])
            );
            let persisted: Decision = serde_json::from_value(json!(decision)).unwrap();
            assert_eq!(&persisted, decision);
        }
        assert!(assessment.clean_episode_ids.is_empty());
    }
}

#[test]
fn known_extent_sibling_and_pair_survive_an_unknown_extent() {
    let mut input = input(&["parser.rs", "parser.rs", "parser.rs"]);
    let last = input.episodes[0].reads[2].result.as_mut().unwrap();
    last.returned_extent = None;
    let id = last.reference_id.clone();
    input
        .events
        .iter_mut()
        .find(|event| event.reference.id == id)
        .unwrap()
        .metadata
        .read_result = Some(last.clone());
    let plan = plan(&input);
    let extents = plan
        .prepared
        .targets
        .values()
        .filter(|target| target.reason == Reason::ExcessiveWithinFileReading)
        .collect::<Vec<_>>();
    assert_eq!(extents.len(), 3);
    assert!(
        extents
            .iter()
            .all(|target| !target.read_indexes.contains(&2))
    );
    assert!(
        extents
            .iter()
            .any(|target| target.read_indexes == vec![0, 1])
    );
}

#[test]
fn small_window_pair_keeps_whole_output_equality_separate_from_samples() {
    for tokens in [2048, 2050] {
        let input = input(&["parser.rs", "parser.rs"]);
        let mut plan = OverExploringCheck
            .prepare_with_capabilities(
                &build_jev_context(&input).unwrap(),
                &small_capabilities(tokens),
            )
            .unwrap();
        let pair_id = plan
            .prepared
            .targets
            .iter()
            .find(|(_, target)| {
                target.reason == Reason::ExcessiveWithinFileReading && target.bindings.len() == 2
            })
            .unwrap()
            .0
            .clone();
        let mut progress = SamplingProgress::new(SamplingLimits {
            checks: 1,
            candidates_per_check: MAX_SAMPLING_CANDIDATES,
            answers_per_candidate: 1,
            judgments_per_run: 8,
        })
        .unwrap();
        synchronize_sampling(&plan, &mut progress).unwrap();
        progress.begin_run();
        while let Some(job) = progress.choose_job() {
            let candidate = plan
                .prepared
                .candidates
                .iter()
                .find(|candidate| candidate.candidate_id == job.candidate)
                .unwrap();
            if candidate.work_item_ids.contains(&pair_id) {
                PreparedAssessment::select_jobs(&mut plan, &[job]).unwrap();
                break;
            }
        }
        if plan.work_items.is_empty() {
            assert_eq!(plan.skipped_item_ids, vec![pair_id]);
            continue;
        }
        let result = OverExploringCheck
            .reduce(&plan, &results(&plan, "likely_excess", 0.9), false)
            .unwrap();
        let basis = result.findings[0].explanation.as_ref().unwrap();
        assert_eq!(
            basis.relationship,
            ReadRelationship::LaterReadRepeatsEarlier
        );
        assert_eq!(basis.whole_output_equal, Some(true));
        assert_eq!(basis.reads.len(), 2);
    }
}

#[test]
fn small_breadth_uses_distinct_files_and_a_direct_set_judgment() {
    let input = input(&["parser.rs", "library.rs", "parser.rs"]);
    let plan = OverExploringCheck
        .prepare_with_capabilities(
            &build_jev_context(&input).unwrap(),
            &small_capabilities(2048),
        )
        .unwrap();
    let breadth = plan
        .prepared
        .targets
        .iter()
        .find(|(_, target)| target.reason == Reason::ExcessiveFileBreadth)
        .unwrap();
    assert_eq!(breadth.1.bindings.len(), 2);
    assert!(
        plan.work_items.iter().any(|item| item.id == *breadth.0),
        "{:?}",
        plan.skipped_item_ids
    );
}

#[test]
fn bounded_eight_file_breadth_fits_without_repeating_returned_bodies() {
    let paths = [
        "parser.rs",
        "lexer.rs",
        "tokens.rs",
        "errors.rs",
        "types.rs",
        "format.rs",
        "config.rs",
        "tests.rs",
    ];
    let input = input(&paths);
    for tokens in [2048, 2050] {
        let plan = OverExploringCheck
            .prepare_with_capabilities(
                &build_jev_context(&input).unwrap(),
                &small_capabilities(tokens),
            )
            .unwrap();
        let breadth_id = plan
            .prepared
            .targets
            .iter()
            .find(|(_, target)| target.reason == Reason::ExcessiveFileBreadth)
            .unwrap()
            .0
            .clone();
        let mut plan = plan;
        let mut progress = SamplingProgress::new(SamplingLimits {
            checks: 1,
            candidates_per_check: MAX_SAMPLING_CANDIDATES,
            answers_per_candidate: 1,
            judgments_per_run: 16,
        })
        .unwrap();
        synchronize_sampling(&plan, &mut progress).unwrap();
        progress.begin_run();
        while let Some(job) = progress.choose_job() {
            if plan
                .prepared
                .candidates
                .iter()
                .find(|candidate| candidate.candidate_id == job.candidate)
                .unwrap()
                .work_item_ids
                .contains(&breadth_id)
            {
                PreparedAssessment::select_jobs(&mut plan, &[job]).unwrap();
                break;
            }
        }
        let item = plan.work_items.iter().find(|item| item.id == breadth_id);
        let Some(item) = item else {
            assert!(
                plan.skipped_item_ids.contains(&breadth_id) || plan.coverage.not_selected_items > 0
            );
            continue;
        };
        assert_eq!(item.window.fields["reads"].as_array().unwrap().len(), 8);
        assert!(!item.window.fields.to_string().contains("recorded content"));
        let result = OverExploringCheck
            .reduce(&plan, &results(&plan, "likely_excess", 0.9), false)
            .unwrap();
        let finding = result
            .findings
            .iter()
            .find(|finding| finding.reason == Reason::ExcessiveFileBreadth)
            .unwrap();
        assert_eq!(
            finding.explanation.as_ref().unwrap().relationship,
            ReadRelationship::DistinctFileSetTooBroad
        );
        assert!(
            finding
                .explanation
                .as_ref()
                .unwrap()
                .whole_output_equal
                .is_none()
        );
    }
}

#[test]
fn small_read_request_fits_escaped_unicode_and_retains_content_not_its_wrapper() {
    for tokens in [2048, 2050] {
        let (content, task, span) = fixture(
            "Fix parser encoding. Preserve UTF-8 input.",
            &["src/escaped \"folder\"/parser-🦀.rs"],
            "quoted \"text\" and \\slashes 🦀\nValidate Unicode.",
            "Done.",
        );
        let input = build_episodes(&content, &task, &[span]).unwrap();
        let plan = OverExploringCheck
            .prepare_with_capabilities(
                &build_jev_context(&input).unwrap(),
                &small_capabilities(tokens),
            )
            .unwrap();
        assert_eq!(plan.work_items.len(), 2);
        let packing = pack_work_items_with_capabilities(&plan.work_items, &plan.capabilities);
        assert!(packing.skipped_item_ids.is_empty());
        for batch in packing.batches {
            validate_jev_request_with_capabilities(&batch.request, &plan.capabilities).unwrap();
        }
        for item in &plan.work_items {
            let text = item.window.fields["reads"][0]["result"]["text"]
                .as_str()
                .unwrap();
            assert!(text.contains("quoted"));
            assert!(!text.contains("<path>"));
        }
    }
}

#[test]
fn missing_first_task_does_not_veto_later_episode() {
    let (content, task, _) = fixture(
        "Fix the parser.",
        &["parser.rs", "library.rs"],
        "code",
        "Done.",
    );
    let spans = [
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
    let mut input = build_episodes(&content, &task, &spans).unwrap();
    let unavailable = JevSharedRequestContext {
        fields: json!({}),
        evidence: vec![],
    };
    input.task_context = unavailable.clone();
    input
        .task_contexts
        .insert(input.episodes[0].id, unavailable);
    let plan = OverExploringCheck
        .prepare_with_capabilities(
            &build_jev_context(&input).unwrap(),
            &small_capabilities(2050),
        )
        .unwrap();
    assert!(!plan.work_items.is_empty());
    assert!(
        plan.work_items
            .iter()
            .all(|item| plan.prepared.targets[&item.id].episode_id == input.episodes[1].id)
    );
}

#[test]
fn local_positive_does_not_override_negative_distinct_file_set() {
    let input = input(&["parser.rs", "library.rs"]);
    let plan = plan(&input);
    let mut judgments = results(&plan, "likely_excess", 0.9);
    let negative = results(&plan, "justified_or_minor", 0.9);
    for result in &mut judgments {
        if plan.prepared.targets[&result.work_item_id].reason == Reason::ExcessiveFileBreadth {
            *result = negative
                .iter()
                .find(|other| other.work_item_id == result.work_item_id)
                .unwrap()
                .clone();
        }
    }
    let assessment = OverExploringCheck.reduce(&plan, &judgments, true).unwrap();
    assert!(!assessment.findings.is_empty());
    assert!(
        assessment
            .findings
            .iter()
            .all(|finding| finding.reason != Reason::ExcessiveFileBreadth)
    );
}

#[test]
fn fresh_reduction_retracts_breadth_after_accepted_counterevidence() {
    let plan = plan(&input(&["parser.rs", "library.rs"]));
    let prior = OverExploringCheck
        .reduce(&plan, &results(&plan, "likely_excess", 0.9), true)
        .unwrap();
    assert!(
        prior
            .findings
            .iter()
            .any(|finding| finding.reason == Reason::ExcessiveFileBreadth)
    );
    let mut latest_answers = results(&plan, "likely_excess", 0.9);
    let negatives = results(&plan, "justified_or_minor", 0.9);
    for result in &mut latest_answers {
        if plan.prepared.targets[&result.work_item_id].reason == Reason::ExcessiveFileBreadth {
            *result = negatives
                .iter()
                .find(|negative| negative.work_item_id == result.work_item_id)
                .unwrap()
                .clone();
        }
    }
    let latest = OverExploringCheck
        .reduce(&plan, &latest_answers, true)
        .unwrap();
    assert!(
        latest
            .findings
            .iter()
            .any(|finding| finding.reason != Reason::ExcessiveFileBreadth)
    );
    assert!(
        latest
            .findings
            .iter()
            .all(|finding| finding.reason != Reason::ExcessiveFileBreadth)
    );
    assert_eq!(
        latest.completed_work_item_ids,
        prior.completed_work_item_ids
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
fn a_128_path_inventory_keeps_local_targets_and_bounds_breadth() {
    let paths = (0..128)
        .map(|index| format!("file-{index}.rs"))
        .collect::<Vec<_>>();
    let input = input(&paths.iter().map(String::as_str).collect::<Vec<_>>());
    let plan = plan(&input);
    assert_eq!(plan.prepared.targets.len(), 256);
    assert_eq!(plan.prepared.candidates.len(), 256);
    let mut progress = progress();
    synchronize_sampling(&plan, &mut progress).unwrap();
    let check = crate::checks::sampling::StableId::new("smart-check", &[b"over_exploring"]);
    assert_eq!(progress.coverage(check).unwrap().eligible, 256);
    assert_eq!(progress.coverage(check).unwrap().remaining, 256);
}

#[test]
fn selected_jobs_preserve_the_set_level_breadth_gap_without_duplicates() {
    let paths = (0..128)
        .map(|index| format!("file-{index}.rs"))
        .collect::<Vec<_>>();
    let input = input(&paths.iter().map(String::as_str).collect::<Vec<_>>());
    let mut plan = plan(&input);
    let mut progress = progress();
    synchronize_sampling(&plan, &mut progress).unwrap();
    progress.begin_run();
    let job = progress.choose_job().unwrap();
    for _ in 0..2 {
        PreparedAssessment::select_jobs(&mut plan, std::slice::from_ref(&job)).unwrap();
        let gaps = plan
            .prepared
            .unassessed
            .iter()
            .filter(|item| {
                item.work_item_id.is_none()
                    && item.reason == Some(Reason::ExcessiveFileBreadth)
                    && item.limitation == Abstention::SampledEvidence
            })
            .count();
        assert_eq!(gaps, 1);
        assert!(
            plan.coverage
                .limitations
                .contains(&"SampledEvidence".into())
        );
        let reduced = OverExploringCheck
            .reduce(&plan, &results(&plan, "justified_or_minor", 0.98), true)
            .unwrap();
        assert!(reduced.clean_episode_ids.is_empty());
        assert!(
            reduced
                .unassessed
                .iter()
                .any(|item| item.work_item_id.is_none()
                    && item.reason == Some(Reason::ExcessiveFileBreadth)
                    && item.limitation == Abstention::SampledEvidence)
        );
    }
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
fn source_identity_corruption_is_rejected_and_read_dependencies_are_isolated() {
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
        let result = build_episodes(&content, &task, &spans);
        match variant {
            "request_id" | "unstable" => assert!(result.unwrap().episodes.is_empty(), "{variant}"),
            "call" | "digest" | "bytes" | "result_id" => {
                let input = result.unwrap();
                assert_eq!(input.episodes[0].reads.len(), 1);
                assert!(input.episodes[0].reads[0].result.is_none(), "{variant}");
                assert!(
                    input
                        .limitations
                        .contains(&"read_result_binding_invalid".into())
                );
            }
            _ => assert_eq!(result, Err(JevError::InvalidCheckContext), "{variant}"),
        }
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
                instructions["question"]["rules"]
                    .as_str()
                    .unwrap()
                    .contains("Task")
            );
            assert!(instructions["question"]["question"].as_str().is_some());
            assert_eq!(criteria.len(), 3);
        }
    }
    assert_eq!(
        reasons,
        BTreeSet::from([Reason::ExcessiveWithinFileReading])
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
        assert!((1..=2).contains(&later.bindings.len()));
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
                instructions["question"]["rules"]
                    .as_str()
                    .unwrap()
                    .contains("no diagnosis hindsight")
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
        assert!(reduced.clean_episode_ids.is_empty());
        assert!(
            serde_json::to_string(&plan.shared_context)
                .unwrap()
                .contains(task_text)
        );
        let fields = serde_json::to_string(&plan.work_items[0].window.fields).unwrap();
        assert!(fields.contains(later));
        let question = serde_json::to_string(&plan.work_items[0].questions).unwrap();
        assert!(question.contains("against the objective"));
        assert!(question.contains("dependencies") || question.contains("objective"));
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
