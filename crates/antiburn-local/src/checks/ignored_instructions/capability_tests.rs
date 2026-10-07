use super::tests::{event, input};
use super::*;
use crate::analysis::jev::capabilities::{
    CapabilityLimit, CapabilitySource, ModelCapabilities, TokenizerIdentity,
};
use crate::analysis::jev::{
    pack_work_items_with_capabilities, validate_jev_request_with_capabilities,
};
use crate::checks::ignored_instructions::planning::{
    SamplingLedger, build_jev_context_with_capabilities,
    extend_jev_context_with_history_and_capabilities,
};

fn hosted_limits(tokens: u64) -> ModelCapabilities {
    let mut limits = ModelCapabilities::jev_default();
    limits.model = "synthetic-hosted".to_owned();
    limits.total_input_tokens = CapabilityLimit::known(tokens, CapabilitySource::Manual);
    limits.state_and_longest_question_tokens = CapabilityLimit::unknown();
    limits.state_and_longest_question_bytes = CapabilityLimit::unknown();
    limits.request_body_bytes = CapabilityLimit::known(1024 * 1024, CapabilitySource::Manual);
    limits.questions_per_request = CapabilityLimit::known(64, CapabilitySource::DocumentedDefault);
    limits
}

fn prepare(input: &AssessmentInput, limits: &ModelCapabilities) -> JevCheckPlan<AssessmentPlan> {
    let context =
        build_jev_context_with_capabilities(input, &SamplingLedger::default(), limits).unwrap();
    IgnoredInstructionsCheck
        .prepare_with_capabilities(&context, limits)
        .unwrap()
}

#[test]
fn larger_limits_supply_more_action_and_context_without_changing_event_selection() {
    let mut actions = (0..7)
        .map(|index| {
            event(
                &format!("earlier-{index}"),
                index,
                "assistant",
                "main",
                &format!(
                    "{}Run the required tests before publishing.",
                    "Recorded work. ".repeat(80)
                ),
            )
        })
        .collect::<Vec<_>>();
    actions.push(event(
        "candidate",
        10,
        "assistant",
        "main",
        &format!(
            "{}I ran the required tests before publishing.",
            "Recorded work. ".repeat(200),
        ),
    ));
    let source = input(actions, "Run the required tests before publishing.");
    let old = build_assessment_plan(source.clone());
    let new = prepare(&source, &hosted_limits(65_536));
    let old_candidate = old
        .comparisons
        .iter()
        .find(|item| item.reference.action_id == "candidate" && item.action_text_start == 0)
        .unwrap();
    let new_candidate = new
        .prepared
        .comparisons
        .iter()
        .find(|item| item.reference.action_id == "candidate")
        .unwrap();
    assert_eq!(new_candidate.action.text, source.content.actions[7].text);
    assert!(!new_candidate.action.truncated);
    assert!(
        !old_candidate
            .action
            .text
            .contains("I ran the required tests")
    );
    assert!(new_candidate.context.iter().all(|item| !item.truncated));
    assert!(
        new_candidate
            .counterevidence
            .iter()
            .all(|item| !item.truncated)
    );
    assert_eq!(
        old_candidate
            .context
            .iter()
            .map(|item| &item.action_id)
            .collect::<Vec<_>>(),
        new_candidate
            .context
            .iter()
            .map(|item| &item.action_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        old_candidate
            .counterevidence
            .iter()
            .map(|item| &item.action_id)
            .collect::<Vec<_>>(),
        new_candidate
            .counterevidence
            .iter()
            .map(|item| &item.action_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(new.capabilities.model, "synthetic-hosted");
    assert_eq!(new.prepared.model_version, "synthetic-hosted");
}

#[test]
fn larger_windows_keep_unproven_context_out_of_requests_and_thinking_out_of_identity() {
    use crate::checks::ignored_instructions::select_session_content;

    let mut user = event("user", 1, "user", "main", "EXCLUDED_USER");
    user.kind = "user_text".to_owned();
    let mut thinking = event("thinking", 2, "assistant", "main", "EXCLUDED_THINKING");
    thinking.kind = "thinking".to_owned();
    let mut output = event("output", 3, "tool", "main", "EXCLUDED_OUTPUT");
    output.kind = "tool_result".to_owned();
    output.tool_name = Some("Bash".to_owned());
    let mut source = input(
        vec![
            user,
            thinking,
            output,
            event(
                "candidate",
                4,
                "assistant",
                "main",
                &"Selected response. ".repeat(200),
            ),
        ],
        "Follow the documented process.",
    );
    source.content.actions[3].authority = "assistant".to_owned();
    let mut changed = source.clone();
    changed.content.actions[1]
        .text
        .push_str("CHANGED_EXCLUDED_TEXT");
    source.content = select_session_content(&source.content, INPUT_SELECTION);
    changed.content = select_session_content(&changed.content, INPUT_SELECTION);
    let limits = hosted_limits(262_144);
    let plan = prepare(&source, &limits);
    let changed_plan = prepare(&changed, &limits);
    assert_eq!(plan.input_revision, changed_plan.input_revision);
    assert_eq!(plan.work_items, changed_plan.work_items);
    assert_eq!(plan.prepared.comparisons.len(), 1);
    let packed = pack_work_items_with_capabilities(&plan.work_items, &limits);
    assert_eq!(packed.batches.len(), 1);
    assert!(
        !serde_json::to_string(&packed.batches[0].request)
            .unwrap()
            .contains("EXCLUDED")
    );
}

#[test]
fn limit_changes_invalidate_range_inventory_and_growth_has_local_bounds() {
    let source = input(
        vec![event(
            "candidate",
            1,
            "assistant",
            "main",
            &"Recorded work. ".repeat(5000),
        )],
        "Follow the documented process.",
    );
    let small = prepare(&source, &hosted_limits(65_536));
    let large = prepare(&source, &hosted_limits(262_144));
    let huge = prepare(&source, &hosted_limits(u64::MAX));
    assert_ne!(small.input_revision, large.input_revision);
    assert_ne!(
        small.prepared.comparisons[0].id,
        large.prepared.comparisons[0].id
    );
    assert_eq!(large.prepared.comparisons, huge.prepared.comparisons);
    assert!(
        huge.prepared
            .comparisons
            .iter()
            .all(|item| item.action.text.len() <= 32 * 1024)
    );
    assert_eq!(huge.prepared.coverage.unselected_pairs, 0);
}

#[test]
fn unicode_ranges_cover_the_full_action_and_keep_source_truncation() {
    let text = "界é🦀\n".repeat(5000);
    let mut source = input(
        vec![event("candidate", 1, "assistant", "main", &text)],
        "Follow the documented process.",
    );
    source.content.actions[0].truncated = true;
    let plan = prepare(&source, &hosted_limits(65_536));
    let mut ranges = plan
        .prepared
        .comparisons
        .iter()
        .map(|item| (item.action_text_start, item.action_text_end))
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    assert_eq!(ranges[0].0, 0);
    assert_eq!(ranges.last().unwrap().1, text.len());
    assert!(ranges.windows(2).all(|pair| pair[0].1 > pair[1].0));
    for comparison in &plan.prepared.comparisons {
        assert_eq!(
            comparison.action.text,
            text[comparison.action_text_start..comparison.action_text_end]
        );
        assert!(comparison.action.truncated);
    }
    let packed = pack_work_items_with_capabilities(&plan.work_items, &plan.capabilities);
    assert!(packed.skipped_item_ids.is_empty());
    assert!(packed.batches.iter().all(|batch| {
        validate_jev_request_with_capabilities(&batch.request, &plan.capabilities).is_ok()
    }));
}

#[test]
fn small_limits_reject_required_rule_context_without_shortening_it() {
    let rule = "Only publish when every required prerequisite is recorded. ".repeat(20);
    let source = input(
        vec![event("candidate", 1, "assistant", "main", "Published.")],
        &rule,
    );
    let mut limits = hosted_limits(512);
    limits.rendering_reserve_tokens = 0;
    limits.request_body_bytes.value = Some(512);
    let plan = prepare(&source, &limits);
    assert_eq!(plan.prepared.comparisons.len(), 1);
    assert_eq!(
        plan.work_items[0].window.fields["instruction_targets"][0]["instruction"]["text"],
        rule
    );
    let packed = pack_work_items_with_capabilities(&plan.work_items, &limits);
    assert!(packed.batches.is_empty());
    assert_eq!(packed.skipped_item_ids, vec![plan.work_items[0].id.clone()]);
}

#[test]
fn history_continuation_uses_the_same_context_limit_and_preserves_branch_isolation() {
    let limits = hosted_limits(65_536);
    let mut source = input(
        vec![event(
            "candidate",
            20,
            "assistant",
            "main",
            "Published the release.",
        )],
        "Run required tests before publishing.",
    );
    source.prior_history_complete = false;
    let mut context =
        build_jev_context_with_capabilities(&source, &SamplingLedger::default(), &limits).unwrap();
    let mut carried = IgnoredInstructionsCheck
        .prepare_with_capabilities(&context, &limits)
        .unwrap()
        .prepared
        .comparisons;
    context.check_context["assessment_plan"]["comparisons"] = json!([]);
    let history = format!("{}Ran required tests.", "Recorded step. ".repeat(80));
    let page = vec![
        event("tests", 10, "assistant", "main", &history),
        event("other-branch", 11, "assistant", "other", "EXCLUDED_BRANCH"),
    ];
    extend_jev_context_with_history_and_capabilities(
        &mut context,
        &mut carried,
        &page,
        true,
        &limits,
    )
    .unwrap();
    assert_eq!(carried[0].counterevidence.len(), 1);
    assert_eq!(carried[0].counterevidence[0].text, history);
    assert!(!carried[0].earlier_history_truncated);
    assert!(carried[0].prior_history_complete);
    let plan = IgnoredInstructionsCheck
        .prepare_with_capabilities(&context, &limits)
        .unwrap();
    assert_eq!(
        plan.prepared.comparisons[0].counterevidence[0].text,
        history
    );
    assert!(
        !plan.work_items[0]
            .window
            .fields
            .to_string()
            .contains("EXCLUDED_BRANCH")
    );
}

#[test]
fn ollama_body_limit_controls_growth_even_with_large_runtime_context() {
    let source = input(
        vec![event(
            "candidate",
            1,
            "assistant",
            "main",
            &"界\"\\\n".repeat(20_000),
        )],
        "Follow the documented process.",
    );
    let mut limits = hosted_limits(1024 * 1024);
    limits.model = "synthetic-ollama".to_owned();
    limits.request_body_bytes.value = Some(64 * 1024);
    let plan = prepare(&source, &limits);
    let larger = prepare(&source, &hosted_limits(1024 * 1024));
    let largest = |plan: &JevCheckPlan<AssessmentPlan>| {
        plan.prepared
            .comparisons
            .iter()
            .map(|item| item.action.text.len())
            .max()
            .unwrap()
    };
    assert!(largest(&plan) < largest(&larger));
    let packed = pack_work_items_with_capabilities(&plan.work_items, &limits);
    assert!(packed.skipped_item_ids.is_empty());
    assert!(
        packed
            .batches
            .iter()
            .all(|batch| serde_json::to_vec(&batch.request).unwrap().len() <= 64 * 1024)
    );
}

#[test]
fn capability_window_performance_regression() {
    let source = input(
        vec![event(
            "candidate",
            1,
            "assistant",
            "main",
            &"Recorded work. ".repeat(3500),
        )],
        &format!("- {}", "Follow the required release process. ".repeat(100)),
    );
    let old_context = build_jev_context(&source).unwrap();
    let mut old = IgnoredInstructionsCheck.prepare(&old_context).unwrap();
    old.capabilities.request_body_bytes.value = Some(60 * 1024);
    old.capabilities.state_and_longest_question_bytes.value = Some(30 * 1024);
    old.capabilities.tokenizer = Some(TokenizerIdentity::ConservativeEstimator(
        "serialized UTF-8 byte upper bound".to_owned(),
    ));
    let hosted = hosted_limits(65_536);
    let large_hosted = hosted_limits(262_144);
    let mut ollama = large_hosted.clone();
    ollama.request_body_bytes.value = Some(64 * 1024);
    let mut old_requests = 0;
    let mut old_pairs = 0;
    for (name, limits) in [
        ("legacy", old.capabilities.clone()),
        ("hosted-64k", hosted),
        ("hosted-256k", large_hosted),
        ("ollama-64KiB", ollama),
    ] {
        let started = std::time::Instant::now();
        let plan = if name == "legacy" {
            old.clone()
        } else {
            prepare(&source, &limits)
        };
        let packed = pack_work_items_with_capabilities(&plan.work_items, &limits);
        assert!(packed.skipped_item_ids.is_empty());
        let action_bytes = plan
            .work_items
            .iter()
            .map(|item| {
                item.window.fields["candidate_action"]["text"]
                    .as_str()
                    .unwrap()
                    .len()
            })
            .sum::<usize>();
        let largest_action = plan
            .prepared
            .comparisons
            .iter()
            .map(|item| item.action.text.len())
            .max()
            .unwrap();
        let state_bytes = packed
            .batches
            .iter()
            .map(|batch| serde_json::to_vec(&batch.request.state).unwrap().len())
            .sum::<usize>();
        let body_bytes = packed
            .batches
            .iter()
            .map(|batch| batch.serialized_bytes)
            .sum::<usize>();
        let retained_bytes = serde_json::to_vec(&plan.prepared).unwrap().len();
        assert!(retained_bytes < 2 * 1024 * 1024);
        eprintln!(
            "{name}: pairs={}, windows={}, requests={}, largest_action={}, supplied_action={}, state_bytes={}, body_bytes={}, retained_plan_bytes={}, elapsed={:?}",
            plan.prepared.comparisons.len(),
            plan.work_items.len(),
            packed.batches.len(),
            largest_action,
            action_bytes,
            state_bytes,
            body_bytes,
            retained_bytes,
            started.elapsed()
        );
        if name == "legacy" {
            old_requests = packed.batches.len();
            old_pairs = plan.prepared.comparisons.len();
        } else {
            assert!(largest_action > 1024);
            assert!(plan.prepared.comparisons.len() < old_pairs);
            assert!(packed.batches.len() < old_requests);
            assert_eq!(plan.prepared.coverage.unselected_pairs, 0);
        }
    }
}
