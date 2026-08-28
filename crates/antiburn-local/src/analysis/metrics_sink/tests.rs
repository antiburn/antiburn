use super::*;
use crate::analysis::model::{CompactionTrigger, ToolCall, ToolCategory};

fn event(timestamp: Option<i64>, role: Role, input: u64, output: u64) -> NormalizedEvent {
    let mut event = NormalizedEvent::new(role);
    event.ts_ms = timestamp;
    event.usage.input_tokens = input;
    event.usage.output_tokens = output;
    event
}

fn finished(events: Vec<NormalizedEvent>, summary: SessionSummary) -> SessionMetricsAccumulator {
    SessionMetricsAccumulator::from_parts(
        "synthetic".to_string(),
        "session".to_string(),
        events,
        summary,
    )
}

#[test]
fn bounded_reducer_matches_the_retained_reference_for_small_streams() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    for seed in 0_usize..200 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let turn_count = usize::try_from(state % 401).expect("the count fits");
        let mut events = Vec::with_capacity(turn_count);
        for ordinal in 0..turn_count {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let base = i64::try_from(ordinal).expect("the ordinal fits") * 1_000;
            let disorder = i64::try_from(state % 32).expect("the disorder fits") * 1_000;
            let timestamp = if seed.is_multiple_of(20) {
                Some(0)
            } else if state.is_multiple_of(23) {
                None
            } else {
                Some(base.saturating_sub(disorder))
            };
            let mut current = event(
                timestamp,
                if state.is_multiple_of(7) {
                    Role::User
                } else {
                    Role::Assistant
                },
                state % 300,
                state % 40,
            );
            current.model = if state.is_multiple_of(31) {
                None
            } else if state.is_multiple_of(43) {
                Some(String::new())
            } else {
                Some("claude-opus-4-6".to_string())
            };
            current.source = if state.is_multiple_of(29) {
                EventSource::Subagent
            } else {
                EventSource::Parent
            };
            current.message_id =
                (current.role == Role::Assistant).then(|| format!("message-{}", ordinal / 2));
            current.usage.cache_read_tokens = state % 2_000;
            current.usage.cache_creation_tokens = state % 500;
            current.thinking_mode = state.is_multiple_of(5).then(|| "high".to_string());
            current.speed = state.is_multiple_of(11).then(|| "fast".to_string());
            current.has_thinking = state.is_multiple_of(13);
            current.is_compaction_boundary = state.is_multiple_of(37);
            if state.is_multiple_of(17) {
                current.tools.push(ToolCall::new("Task"));
            }
            if state.is_multiple_of(19) {
                let mut skill = ToolCall::new("Skill");
                skill.detail = Some("synthetic-skill".to_string());
                current.tools.push(skill);
            }
            current.wrapper_tool = state.is_multiple_of(41).then(|| "exec".to_string());
            events.push(current);
        }
        let late_tools = if turn_count > 0 && seed.is_multiple_of(10) {
            events[0].may_resolve_late_tool = true;
            let mut skill = ToolCall::new("Skill");
            skill.detail = Some("late-synthetic-skill".to_string());
            vec![(0, skill)]
        } else {
            Vec::new()
        };
        let make_summary = || SessionSummary {
            cache_write_tokens_available: seed % 2 == 0,
            context_window: Some(200_000),
            model: (!seed.is_multiple_of(17)).then(|| "claude-opus-4-6".to_string()),
            late_tools: late_tools.clone(),
            ..SessionSummary::default()
        };
        let summary = make_summary();
        let bounded = SessionMetricsAccumulator::from_parts(
            "synthetic".to_string(),
            format!("seed-{seed}"),
            events.clone(),
            summary,
        );
        let reference = super::reference::SessionMetricsAccumulator::from_parts(
            "synthetic".to_string(),
            format!("seed-{seed}"),
            events,
            make_summary(),
        );
        let bounded_metrics = bounded.metrics();
        let reference_metrics = reference.metrics();
        assert_eq!(
            bounded_metrics.efficiency, reference_metrics.efficiency,
            "efficiency seed {seed}"
        );
        assert_eq!(bounded_metrics, reference_metrics, "seed {seed}");
        assert_eq!(bounded.earliest_ts_ms(), reference.earliest_ts_ms());
        assert_eq!(reference.retained_turns(), turn_count);
        if turn_count > 0 {
            assert!(reference.retained_bytes() > 0);
        }
    }
}

#[test]
fn bounded_reducer_matches_the_reference_across_idle_gaps() {
    let mut first_assistant = event(Some(0), Role::Assistant, 10, 2);
    first_assistant.model = Some("claude-opus-4-6".to_string());
    first_assistant.tools.push(ToolCall::new("Read"));
    let mut first_user = event(Some(IDLE_GAP_MS / 2), Role::User, 3, 0);
    first_user.thinking_mode = Some("high".to_string());
    let mut second_assistant = event(Some(IDLE_GAP_MS * 2), Role::Assistant, 20, 4);
    second_assistant.model = Some("claude-sonnet-4-6".to_string());
    second_assistant.usage.cache_read_tokens = 5;
    second_assistant.tools.push(ToolCall::new("Edit"));
    let mut second_user = event(Some(IDLE_GAP_MS * 2 + IDLE_GAP_MS / 3), Role::User, 7, 0);
    second_user.is_compaction_boundary = true;
    let events = vec![first_assistant, first_user, second_assistant, second_user];
    let summary = || SessionSummary {
        context_window: Some(200_000),
        cache_write_tokens_available: true,
        ..SessionSummary::default()
    };
    let bounded = finished(events.clone(), summary()).metrics();
    let reference = super::reference::SessionMetricsAccumulator::from_parts(
        "synthetic".to_string(),
        "session".to_string(),
        events,
        summary(),
    )
    .metrics();

    assert_eq!(bounded.active_secs, 550);
    assert_eq!(bounded, reference);
}

#[test]
fn bounded_reducer_drift_tier_preserves_exact_totals() {
    let mut events = Vec::new();
    for ordinal in 0_usize..6_000 {
        let timestamp = if ordinal < 5_700 {
            ordinal as i64
        } else {
            10_000_000 + ordinal as i64 * 60_000
        };
        let mut current = event(Some(timestamp), Role::Assistant, 2, 3);
        current.model = Some("claude-opus-4-6".to_string());
        current.is_compaction_boundary = ordinal.is_multiple_of(503);
        events.push(current);
    }
    let bounded = finished(events.clone(), SessionSummary::default()).metrics();
    let reference = super::reference::SessionMetricsAccumulator::from_parts(
        "synthetic".to_string(),
        "session".to_string(),
        events,
        SessionSummary::default(),
    )
    .metrics();
    assert_eq!(bounded.tokens_in, reference.tokens_in);
    assert_eq!(bounded.tokens_out, reference.tokens_out);
    assert_eq!(bounded.compaction_count, reference.compaction_count);
    assert_eq!(
        bounded
            .buckets
            .iter()
            .map(|bucket| bucket.tokens_in)
            .sum::<u64>(),
        bounded.tokens_in
    );
    assert_eq!(
        bounded
            .buckets
            .iter()
            .map(|bucket| bucket.tokens_out)
            .sum::<u64>(),
        bounded.tokens_out
    );
}

#[test]
#[ignore = "extended bounded-state stress tier"]
fn bounded_reducer_drift_tier_extended() {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "extended");
    for ordinal in 0_usize..100_000 {
        let mut current = event(Some(ordinal as i64 * 1_000), Role::Assistant, 1, 1);
        current.model = Some("claude-opus-4-6".to_string());
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(current)));
    }
    accumulator.finish(SessionSummary::default());
    let metrics = accumulator.metrics();
    assert_eq!(metrics.tokens_in, 100_000);
    assert!(accumulator.retained_bytes() <= RETAINED_METRICS_BYTES_BOUND);
}

#[test]
fn reference_merge_without_sidechains_matches_reference_metrics() {
    let reference = super::reference::SessionMetricsAccumulator::from_parts(
        "synthetic".to_string(),
        "reference-merge".to_string(),
        vec![event(Some(0), Role::Assistant, 1, 1)],
        SessionSummary::default(),
    );
    assert_eq!(
        super::reference::merge_metrics(&reference, &[]),
        reference.metrics()
    );
}

#[test]
fn retained_bytes_excludes_exact_identity_strings() {
    let short = SessionMetricsAccumulator::new("a", "s");
    let long = SessionMetricsAccumulator::new("a".repeat(10_000), "s".repeat(10_000));
    assert_eq!(short.retained_bytes(), long.retained_bytes());
    assert_eq!(long.metrics().agent.len(), 10_000);
    assert_eq!(long.metrics().session_id.len(), 10_000);
}

#[test]
fn empty_metrics_are_repeatable() {
    let accumulator = SessionMetricsAccumulator::new("synthetic", "empty");
    assert_eq!(accumulator.metrics(), accumulator.metrics());
    assert_eq!(accumulator.observed_turns(), 0);
}

#[test]
fn metrics_before_finish_include_every_observed_turn() {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "unfinished");
    for index in 0..17 {
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event(
            Some(index),
            Role::Assistant,
            2,
            3,
        ))));
    }
    let first = accumulator.metrics();
    let retained = accumulator.retained_bytes();
    let second = accumulator.metrics();
    assert_eq!(first, second);
    assert_eq!(first.tokens_in, 34);
    assert_eq!(first.tokens_out, 51);
    assert_eq!(accumulator.retained_bytes(), retained);
}

#[test]
fn repeated_and_out_of_order_timestamps_keep_exact_positions() {
    let metrics = finished(
        vec![
            event(Some(10_000), Role::User, 1, 0),
            event(Some(5_000), Role::Assistant, 2, 1),
            event(Some(5_000), Role::Assistant, 3, 1),
            event(Some(20_000), Role::Assistant, 4, 1),
        ],
        SessionSummary::default(),
    )
    .metrics();
    assert_eq!(metrics.buckets[0].tokens_in, 5);
    assert_eq!(metrics.buckets[60].user_prompts, 1);
    assert_eq!(metrics.buckets[179].tokens_in, 4);
}

#[test]
fn disorder_inside_the_reorder_window_matches_the_reference() {
    for displacement in [1, slots::REORDER_WINDOW - 1] {
        let mut events = (1..=displacement)
            .map(|timestamp| event(Some(timestamp as i64), Role::Assistant, 1, 1))
            .collect::<Vec<_>>();
        events.push(event(Some(0), Role::Assistant, 2, 3));
        let accumulator = finished(events.clone(), SessionSummary::default());
        let reference = super::reference::SessionMetricsAccumulator::from_parts(
            "synthetic".to_string(),
            "session".to_string(),
            events,
            SessionSummary::default(),
        );
        assert_eq!(accumulator.reorder.overflow, 0);
        assert_eq!(accumulator.metrics(), reference.metrics());
    }
}

#[test]
fn disorder_past_the_reorder_window_is_counted() {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "disorder");
    for timestamp in 1..=slots::REORDER_WINDOW {
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event(
            Some(timestamp as i64),
            Role::Assistant,
            1,
            1,
        ))));
    }
    accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event(
        Some(0),
        Role::Assistant,
        1,
        1,
    ))));
    assert_eq!(accumulator.reorder.overflow, 1);
    assert_eq!(accumulator.metrics().tokens_in, 65);
}

#[test]
fn the_last_compaction_keeps_its_metadata_as_one_tuple() {
    let mut context = event(Some(0), Role::Assistant, 90_000, 1);
    context.usage.cache_read_tokens = 100_000;
    let mut first = event(Some(0), Role::System, 0, 0);
    first.is_compaction_boundary = true;
    first.compaction_trigger = Some(CompactionTrigger::Manual);
    first.compaction_pre_tokens = Some(190_000);
    first.compaction_post_tokens = Some(30_000);
    let mut second = event(Some(0), Role::System, 0, 0);
    second.is_compaction_boundary = true;
    let metrics = finished(
        vec![
            context,
            first,
            second,
            event(Some(10_000), Role::User, 0, 0),
        ],
        SessionSummary::default(),
    )
    .metrics();
    let bucket = &metrics.buckets[0];
    assert!(bucket.is_compaction_boundary);
    assert_eq!(bucket.context_tokens, 0);
    assert_eq!(bucket.compaction_trigger, None);
    assert_eq!(bucket.compaction_pre_tokens, None);
    assert_eq!(bucket.compaction_post_tokens, None);
    assert_eq!(metrics.compaction_count, 2);
}

fn inferred_cache_turn(timestamp: i64, read: u64, fresh: u64) -> NormalizedEvent {
    let mut current = event(Some(timestamp), Role::Assistant, fresh, 1);
    current.usage.cache_read_tokens = read;
    current
}

#[test]
fn metrics_before_finish_resolve_deferred_cache_with_the_default_summary() {
    let previous = inferred_cache_turn(0, 25_000, 5_000);
    let mut current = inferred_cache_turn(1_000, 0, 30_000);
    current.model = Some("claude-sonnet-4-6".to_string());
    let mut next = inferred_cache_turn(2_000, 20_000, 10_000);
    next.model = Some("claude-sonnet-4-6".to_string());
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "cache-before-finish");
    for event in [previous, current, next] {
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }
    assert_eq!(accumulator.metrics().cache_routing_miss_count, 1);
}

#[test]
fn summary_model_resolves_the_first_cache_model_transition() {
    let previous = inferred_cache_turn(0, 25_000, 5_000);
    let mut current = inferred_cache_turn(1_000, 0, 30_000);
    current.model = Some("claude-sonnet-4-6".to_string());
    let mut next = inferred_cache_turn(2_000, 20_000, 10_000);
    next.model = Some("claude-sonnet-4-6".to_string());
    let metrics = finished(
        vec![previous, current, next],
        SessionSummary {
            model: Some("claude-opus-4-6".to_string()),
            ..SessionSummary::default()
        },
    )
    .metrics();
    assert_eq!(metrics.cache_rehydration_count, 0);
    assert_eq!(metrics.cache_routing_miss_count, 0);
}

#[test]
fn sidechain_models_do_not_change_the_parent_cache_model() {
    let mut previous = inferred_cache_turn(0, 25_000, 5_000);
    previous.model = Some("claude-opus-4-6".to_string());
    let mut sidechain = event(Some(500), Role::Assistant, 1, 1);
    sidechain.source = EventSource::Subagent;
    sidechain.model = Some("claude-sonnet-4-6".to_string());
    let current = inferred_cache_turn(1_000, 0, 30_000);
    let next = inferred_cache_turn(2_000, 20_000, 10_000);
    let metrics = finished(
        vec![previous, sidechain, current, next],
        SessionSummary::default(),
    )
    .metrics();
    assert_eq!(metrics.cache_routing_miss_count, 1);
}

#[test]
fn cache_gap_keeps_values_larger_than_u32() {
    let mut first = event(Some(0), Role::Assistant, 1_000, 1);
    first.usage.cache_read_tokens = 34_000;
    let mut second = event(Some(i64::MAX), Role::Assistant, 1_000, 1);
    second.usage.cache_creation_tokens = 40_000;
    let metrics = finished(
        vec![first, second],
        SessionSummary {
            cache_write_tokens_available: true,
            ..SessionSummary::default()
        },
    )
    .metrics();
    assert_eq!(
        metrics.buckets[179].secs_since_prior_turn,
        Some((i64::MAX / 1_000) as u64)
    );
}

#[test]
fn cache_modes_preserve_rehydration_gap_priority() {
    let mut first = event(Some(0), Role::Assistant, 1_000, 1);
    first.usage.cache_read_tokens = 29_000;
    let mut second = event(Some(120_000), Role::Assistant, 1_000, 1);
    second.usage.cache_read_tokens = 9_000;
    second.usage.cache_creation_tokens = 20_000;
    let metrics = finished(
        vec![first, second],
        SessionSummary {
            cache_write_tokens_available: true,
            ..SessionSummary::default()
        },
    )
    .metrics();
    assert_eq!(metrics.cache_rehydration_count, 1);
    assert!(metrics.buckets[179].is_cache_rehydration);
    assert_eq!(metrics.buckets[179].secs_since_prior_turn, Some(120));
}

#[test]
fn model_mode_speed_and_last_tool_use_arrival_order() {
    let mut first = event(Some(0), Role::Assistant, 1, 1);
    first.model = Some("model-a".to_string());
    first.thinking_mode = Some("low".to_string());
    first.speed = Some("standard".to_string());
    first.tools.push(ToolCall::new("Read"));
    let mut second = event(Some(0), Role::Assistant, 1, 1);
    second.model = Some("model-b".to_string());
    second.thinking_mode = Some("high".to_string());
    second.speed = Some("fast".to_string());
    second.tools.push(ToolCall::new("Edit"));
    let metrics = finished(
        vec![first, second, event(Some(10_000), Role::User, 0, 0)],
        SessionSummary::default(),
    )
    .metrics();
    assert_eq!(metrics.buckets[0].model.as_deref(), Some("model-b"));
    assert_eq!(metrics.buckets[0].thinking_mode.as_deref(), Some("high"));
    assert_eq!(metrics.buckets[0].speed.as_deref(), Some("fast"));
    assert_eq!(metrics.buckets[0].last_tool.as_deref(), Some("Edit"));
}

#[test]
fn long_skill_names_keep_their_description() {
    let name = "very-long-skill-name-".repeat(5);
    let mut current = event(Some(0), Role::Assistant, 1, 1);
    let mut skill = ToolCall::new("Skill");
    skill.detail = Some(name.clone());
    current.tools.push(skill);
    let metrics = finished(
        vec![current],
        SessionSummary {
            skill_descriptions: [(name.clone(), "does a thing".to_string())]
                .into_iter()
                .collect(),
            ..SessionSummary::default()
        },
    )
    .metrics();
    assert_eq!(metrics.skill_uses[0].name, name);
    assert_eq!(
        metrics.skill_uses[0].description.as_deref(),
        Some("does a thing")
    );
}

#[test]
fn metrics_before_finish_use_the_nearest_disordered_skill_successor() {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "skill-residue");
    let mut skill_event = event(Some(50_000), Role::Assistant, 1, 1);
    let mut skill = ToolCall::new("Skill");
    skill.detail = Some("synthetic-skill".to_string());
    skill_event.tools.push(skill);
    for current in [
        skill_event,
        event(Some(60_000), Role::Assistant, 1, 1),
        event(Some(55_000), Role::Assistant, 1, 1),
    ] {
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(current)));
    }
    let metrics = accumulator.metrics();
    assert_eq!(metrics.skill_uses[0].duration_ms, Some(5_000));
}

#[test]
fn a_late_skill_keeps_position_duration_and_name_counts() {
    let mut command = event(Some(0), Role::User, 0, 0);
    command.may_resolve_late_tool = true;
    let mut skill = ToolCall::new("skill");
    skill.detail = Some("synthetic-skill".to_string());
    let metrics = finished(
        vec![command, event(Some(5_000), Role::Assistant, 1, 2)],
        SessionSummary {
            late_tools: vec![(0, skill)],
            ..SessionSummary::default()
        },
    )
    .metrics();
    assert_eq!(metrics.tool_calls_by_name.get("skill"), Some(&1));
    assert_eq!(metrics.skill_uses.len(), 1);
    assert_eq!(metrics.skill_uses[0].progress, 0.0);
    assert_eq!(metrics.skill_uses[0].duration_ms, Some(5_000));
}

#[test]
fn a_late_task_updates_the_parent_bucket() {
    let mut command = event(Some(0), Role::Assistant, 1, 1);
    command.may_resolve_late_tool = true;
    let metrics = finished(
        vec![command, event(Some(1_000), Role::Assistant, 1, 1)],
        SessionSummary {
            late_tools: vec![(0, ToolCall::new("Task"))],
            ..SessionSummary::default()
        },
    )
    .metrics();
    assert_eq!(metrics.buckets[0].subagent_launches, 1);
    assert_eq!(metrics.buckets[0].last_tool.as_deref(), Some("Task"));
}

#[test]
fn an_empty_model_blocks_summary_model_attribution() {
    let mut current = event(Some(0), Role::Assistant, 10, 2);
    current.model = Some("   ".to_string());
    let metrics = finished(
        vec![current],
        SessionSummary {
            model: Some("claude-opus-4-6".to_string()),
            ..SessionSummary::default()
        },
    )
    .metrics();
    assert!(metrics.model_breakdown.is_empty());
    assert!(metrics.model_runs.is_empty());
    assert_eq!(metrics.efficiency.unpriced_turns, 1);
}

#[test]
fn bounded_merge_matches_reference_chronology_for_small_streams() {
    let make_event = |timestamp, model: &str| {
        let mut current = event(Some(timestamp), Role::Assistant, 10, 2);
        current.model = Some(model.to_string());
        current
    };
    let parent_events = vec![
        make_event(0, "claude-opus-4-6"),
        make_event(10_000, "claude-opus-4-6"),
    ];
    let child_events = vec![make_event(5_000, "claude-sonnet-4-6")];
    let parent = finished(parent_events.clone(), SessionSummary::default());
    let child = finished(child_events.clone(), SessionSummary::default());
    let reference_parent = super::reference::SessionMetricsAccumulator::from_parts(
        "synthetic".to_string(),
        "session".to_string(),
        parent_events,
        SessionSummary::default(),
    );
    let reference_child = super::reference::SessionMetricsAccumulator::from_parts(
        "synthetic".to_string(),
        "child".to_string(),
        child_events,
        SessionSummary::default(),
    );
    let actual = merge_metrics(&parent, &[child]);
    let mut expected = super::reference::merge_metrics(&reference_parent, &[reference_child]);
    expected.efficiency = actual.efficiency;
    assert_eq!(actual, expected);
}

#[test]
fn bounded_merge_without_children_matches_parent_projection() {
    let events = vec![event(Some(0), Role::Assistant, 10, 2)];
    let parent = finished(events.clone(), SessionSummary::default());
    let reference = super::reference::SessionMetricsAccumulator::from_parts(
        "synthetic".to_string(),
        "session".to_string(),
        events,
        SessionSummary::default(),
    );
    let mut expected = super::reference::merge_metrics(&reference, &[]);
    expected.efficiency = parent.metrics().efficiency;
    expected.initial_context = None;
    assert_eq!(merge_metrics(&parent, &[]), expected);
}

#[test]
fn merged_cache_facts_match_a_large_parent_projection() {
    let events = (0..1_000)
        .map(|ordinal| inferred_cache_turn(ordinal * 3_000, 25_000, 5_000))
        .collect::<Vec<_>>();
    let parent = finished(
        events,
        SessionSummary {
            cache_write_tokens_available: true,
            ..SessionSummary::default()
        },
    );
    assert!(parent.slots.compactions > 0);
    let parent_metrics = parent.metrics();
    let merged = merge_metrics(&parent, &[]);
    assert_eq!(
        merged.cache_rehydration_count,
        parent_metrics.cache_rehydration_count
    );
    assert_eq!(
        merged.cache_routing_miss_count,
        parent_metrics.cache_routing_miss_count
    );
    assert_eq!(merged.buckets, parent_metrics.buckets);
}

#[test]
fn merged_cache_detection_uses_chronological_parent_order() {
    let mut previous = inferred_cache_turn(0, 25_000, 5_000);
    previous.model = Some("claude-sonnet-4-6".to_string());
    let mut current = inferred_cache_turn(1_000, 0, 30_000);
    current.model = Some("claude-sonnet-4-6".to_string());
    let mut next = inferred_cache_turn(2_000, 20_000, 10_000);
    next.model = Some("claude-sonnet-4-6".to_string());
    let events = vec![previous, next, current];
    let summary = || SessionSummary {
        cache_write_tokens_available: false,
        model: Some("claude-sonnet-4-6".to_string()),
        ..SessionSummary::default()
    };
    let parent = finished(events.clone(), summary());
    let reference = super::reference::SessionMetricsAccumulator::from_parts(
        "synthetic".to_string(),
        "session".to_string(),
        events,
        summary(),
    );
    let actual = merge_metrics(&parent, &[]);
    let mut expected = super::reference::merge_metrics(&reference, &[]);
    expected.efficiency = actual.efficiency;
    expected.initial_context = None;
    assert_eq!(actual.cache_rehydration_count, 0);
    assert_eq!(actual.cache_routing_miss_count, 1);
    assert_eq!(actual, expected);
}

#[test]
fn merged_unattributed_child_tokens_use_the_parent_model() {
    let mut parent_event = event(Some(0), Role::Assistant, 10, 2);
    parent_event.model = Some("claude-opus-4-6".to_string());
    let parent = finished(
        vec![parent_event],
        SessionSummary {
            model: Some("claude-opus-4-6".to_string()),
            ..SessionSummary::default()
        },
    );
    let child = finished(
        vec![event(Some(1_000), Role::Assistant, 20, 3)],
        SessionSummary {
            model: Some("claude-sonnet-4-6".to_string()),
            ..SessionSummary::default()
        },
    );
    let merged = merge_metrics(&parent, &[child]);
    assert_eq!(
        merged
            .model_breakdown
            .get("claude-opus-4-6")
            .expect("parent attribution")
            .input_tokens,
        30
    );
    assert!(!merged.model_breakdown.contains_key("claude-sonnet-4-6"));
}

#[test]
fn merged_model_overflow_uses_a_normalized_fallback_key() {
    let mut parent_event = event(Some(0), Role::Assistant, 1, 1);
    parent_event.model = Some("claude-opus-4-6[1m]".to_string());
    let parent = finished(
        vec![parent_event],
        SessionSummary {
            model: Some("claude-opus-4-6[1m]".to_string()),
            ..SessionSummary::default()
        },
    );
    let children = (0..MAX_MODELS)
        .map(|index| {
            let mut child_event = event(Some(index as i64 + 1), Role::Assistant, 1, 1);
            child_event.model = Some(format!("synthetic-child-model-{index}"));
            finished(vec![child_event], SessionSummary::default())
        })
        .collect::<Vec<_>>();
    let merged = merge_metrics(&parent, &children);
    assert!(merged.model_breakdown.contains_key("claude-opus-4-6"));
    assert!(!merged.model_breakdown.contains_key("claude-opus-4-6[1m]"));
    assert_eq!(merged.model_breakdown.len(), MAX_MODELS);
}

#[test]
fn merged_skill_uses_follow_shared_chronology() {
    let make_skill = |timestamp, name: &str| {
        let mut current = event(Some(timestamp), Role::Assistant, 1, 1);
        let mut skill = ToolCall::new("Skill");
        skill.detail = Some(name.to_string());
        current.tools.push(skill);
        current
    };
    let parent = finished(
        vec![make_skill(10_000, "parent-skill")],
        SessionSummary::default(),
    );
    let child = finished(
        vec![make_skill(5_000, "child-skill")],
        SessionSummary::default(),
    );
    let merged = merge_metrics(&parent, &[child]);
    assert_eq!(merged.skill_uses[0].name, "child-skill");
    assert_eq!(merged.skill_uses[1].name, "parent-skill");
    assert!(merged.skill_uses[0].progress <= merged.skill_uses[1].progress);
}

#[test]
fn merge_without_children_keeps_parent_model_run_order() {
    let mut first = event(Some(10_000), Role::Assistant, 1, 1);
    first.model = Some("claude-opus-4-6".to_string());
    let mut second = event(Some(5_000), Role::Assistant, 1, 1);
    second.model = Some("claude-sonnet-4-6".to_string());
    let parent = finished(vec![first, second], SessionSummary::default());
    assert_eq!(
        merge_metrics(&parent, &[]).model_runs,
        parent.metrics().model_runs
    );
}

#[test]
fn merged_model_runs_follow_shared_chronology() {
    let mut parent_event = event(Some(10_000), Role::Assistant, 1, 1);
    parent_event.model = Some("claude-opus-4-6".to_string());
    let mut child_event = event(Some(5_000), Role::Assistant, 1, 1);
    child_event.model = Some("claude-sonnet-4-6".to_string());
    let parent = finished(vec![parent_event], SessionSummary::default());
    let child = finished(vec![child_event], SessionSummary::default());
    let merged = merge_metrics(&parent, &[child]);
    assert_eq!(merged.model_runs[0].model, "claude-sonnet-4-6");
    assert_eq!(merged.model_runs[1].model, "claude-opus-4-6");
}

#[test]
fn parent_sidechain_records_remain_subagent_tokens() {
    let parent = event(Some(0), Role::Assistant, 10, 2);
    let mut sidechain = event(Some(1_000), Role::Assistant, 20, 3);
    sidechain.source = EventSource::Subagent;
    let accumulator = finished(vec![parent, sidechain], SessionSummary::default());
    let metrics = merge_metrics(&accumulator, &[]);
    assert_eq!(
        metrics
            .buckets
            .iter()
            .map(|bucket| bucket.tokens_in)
            .sum::<u64>(),
        10
    );
    assert_eq!(
        metrics
            .buckets
            .iter()
            .map(|bucket| bucket.subagent_tokens)
            .sum::<u64>(),
        23
    );
}

#[test]
fn uniform_large_session_populates_the_complete_chart() {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "uniform-large");
    for ordinal in 0..40_000 {
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event(
            Some(ordinal * 1_000),
            Role::Assistant,
            10,
            0,
        ))));
    }
    accumulator.finish(SessionSummary::default());
    let metrics = accumulator.metrics();
    let populated = metrics
        .buckets
        .iter()
        .filter(|bucket| bucket.tokens_in > 0)
        .count();
    assert_eq!(populated, BUCKETS);
    assert!(metrics.buckets[0].tokens_in < metrics.tokens_in / 100);
}

#[test]
fn large_session_keeps_every_visible_compaction_boundary() {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "many-compactions");
    for ordinal in 0..40_000 {
        let mut current = event(Some(ordinal * 1_000), Role::Assistant, 1, 0);
        current.is_compaction_boundary = ordinal.rem_euclid(400) == 0;
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(current)));
    }
    accumulator.finish(SessionSummary::default());
    let metrics = accumulator.metrics();
    assert_eq!(metrics.compaction_count, 100);
    assert_eq!(
        metrics
            .buckets
            .iter()
            .filter(|bucket| bucket.is_compaction_boundary)
            .count(),
        100
    );
}

#[test]
fn large_session_keeps_cache_markers_across_the_chart() {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "many-cache-markers");
    accumulator.observed_turns = 99_400;
    for ordinal in 0..99_400_u64 {
        accumulator.active.observe(ordinal as i64 * 1_000);
    }
    accumulator.active.rebuild_prefix();
    accumulator
        .slots
        .flip_to_active(|timestamp| timestamp.max(0) as u64);
    for ordinal in 0..99_400_u64 {
        let mut slot = SlotAggregate::new(ordinal, ordinal as i64 * 1_000);
        slot.tokens_in = 1;
        if ordinal.is_multiple_of(142) {
            slot.cache_mode_2.is_routing_miss = true;
        }
        accumulator.slots.push(slot, ordinal * 1_000);
    }
    let metrics = accumulator.metrics();
    assert_eq!(
        metrics
            .buckets
            .iter()
            .filter(|bucket| bucket.is_cache_routing_miss)
            .count(),
        BUCKETS
    );
}

#[test]
fn bounded_compaction_keeps_totals_and_boundaries() {
    let mut events = Vec::new();
    for index in 0..(slots::SLOTS + 300) {
        let mut current = event(Some(index as i64 * 1_000), Role::Assistant, 2, 3);
        if index == slots::SLOTS + 299 {
            current.is_compaction_boundary = true;
        }
        events.push(current);
    }
    let accumulator = finished(events, SessionSummary::default());
    let metrics = accumulator.metrics();
    assert_eq!(metrics.tokens_in, ((slots::SLOTS + 300) * 2) as u64);
    assert_eq!(
        metrics
            .buckets
            .iter()
            .map(|bucket| bucket.tokens_in)
            .sum::<u64>(),
        metrics.tokens_in
    );
    assert_eq!(metrics.compaction_count, 1);
    assert!(
        metrics
            .buckets
            .iter()
            .any(|bucket| bucket.is_compaction_boundary)
    );
    assert!(accumulator.retained_bytes() <= RETAINED_METRICS_BYTES_BOUND);
}

#[test]
fn summary_storage_stays_bounded_and_identity_stays_exact() {
    use crate::analysis::initial_context::{
        InitialContextSourceCount, InitialContextTokenSource, SourceOrigin,
    };

    let sources = (0..500)
        .map(|index| InitialContextSourceCount {
            source: InitialContextTokenSource::Skill.as_str().repeat(20),
            source_name: Some(format!("{}-{index}", "name".repeat(100))),
            token_count: 1,
            use_count: 0,
            origin: SourceOrigin::Unknown,
            deferred: false,
            match_names: (0..100)
                .map(|item| format!("{}-{index}-{item}", "alias".repeat(20)))
                .collect(),
        })
        .collect();
    let mut accumulator = SessionMetricsAccumulator::new("a".repeat(1_000), "s".repeat(1_000));
    for index in 0..1_440 {
        let mut current = event(Some(index as i64 * IDLE_GAP_MS * 2), Role::Assistant, 1, 1);
        current.model = Some(format!("model-{index}"));
        current.thinking_mode = Some(format!("thinking-{index}"));
        current.speed = Some(format!("speed-{index}"));
        current.may_resolve_late_tool = true;
        let mut skill = ToolCall::new("Skill");
        skill.detail = Some(format!("{}-{index}", "skill".repeat(100)));
        current.tools.push(skill);
        current
            .tools
            .push(ToolCall::new(format!("mcp__server-{index}__tool")));
        current.tools.push(ToolCall::new(format!("tool-{index}")));
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(current)));
    }
    let skill_descriptions = (0..MAX_SKILL_NAMES)
        .map(|index| {
            (
                format!("{}-{index}", "skill".repeat(100)),
                "description".repeat(100),
            )
        })
        .collect();
    accumulator.finish(SessionSummary {
        model: Some("m".repeat(1_000)),
        initial_context: Some(InitialContextBreakdown { sources }),
        skill_descriptions,
        ..SessionSummary::default()
    });
    let metrics = accumulator.metrics();
    assert_eq!(metrics.agent, "a".repeat(1_000));
    assert_eq!(metrics.session_id, "s".repeat(1_000));
    assert_eq!(
        metrics.model.as_ref().map(String::len),
        Some(tally::MAX_NAME_BYTES)
    );
    let initial_context = metrics.initial_context.expect("bounded context");
    assert!(initial_context.sources.len() <= MAX_INITIAL_CONTEXT_SOURCES);
    assert_eq!(
        initial_context
            .sources
            .iter()
            .map(|source| source.token_count)
            .sum::<u64>(),
        500
    );
    assert!(accumulator.active.segments_merged > 0);
    assert!(
        accumulator.retained_bytes() <= RETAINED_METRICS_BYTES_BOUND,
        "retained {} bytes",
        accumulator.retained_bytes()
    );
}

#[test]
fn initial_context_overflow_preserves_tokens_across_source_categories() {
    use crate::analysis::initial_context::{
        InitialContextSourceCount, InitialContextTokenSource, SourceOrigin,
    };

    let categories = [
        InitialContextTokenSource::Skill,
        InitialContextTokenSource::Mcp,
        InitialContextTokenSource::BuiltinTool,
    ];
    let sources = (0..68)
        .map(|index| InitialContextSourceCount {
            source: categories[index % categories.len()].as_str().to_string(),
            source_name: Some(format!("source-{index}")),
            token_count: 1,
            use_count: 0,
            origin: SourceOrigin::Unknown,
            deferred: false,
            match_names: Vec::new(),
        })
        .collect();
    let metrics = finished(
        Vec::new(),
        SessionSummary {
            initial_context: Some(InitialContextBreakdown { sources }),
            ..SessionSummary::default()
        },
    )
    .metrics();
    let sources = metrics.initial_context.expect("initial context").sources;
    assert_eq!(sources.len(), MAX_INITIAL_CONTEXT_SOURCES);
    assert_eq!(
        sources.iter().map(|source| source.token_count).sum::<u64>(),
        68
    );
    assert!(sources.iter().all(|source| source.source_name.is_some()));
    assert!(
        sources
            .iter()
            .any(|source| source.source_name.as_deref() == Some("Other skills"))
    );
    assert!(
        sources
            .iter()
            .any(|source| source.source_name.as_deref() == Some("Other MCP servers"))
    );
    assert!(
        sources
            .iter()
            .any(|source| { source.source_name.as_deref() == Some("Other built-in tools") })
    );
}

#[test]
fn initial_context_names_keep_use_count_matching_after_bounding() {
    use crate::analysis::initial_context::{
        InitialContextSourceCount, InitialContextTokenSource, SourceOrigin,
    };

    let skill_name = "long-skill-name-".repeat(8);
    let mcp_name = "Long-MCP-Server-".repeat(8);
    let mut current = event(Some(0), Role::Assistant, 1, 1);
    let mut skill = ToolCall::new("Skill");
    skill.detail = Some(skill_name.clone());
    current.tools.push(skill);
    let mut case_variant = ToolCall::new("Skill");
    case_variant.detail = Some(skill_name.to_ascii_uppercase());
    current.tools.push(case_variant);
    current
        .tools
        .push(ToolCall::new(format!("mcp__{mcp_name}__search")));
    let sources = vec![
        InitialContextSourceCount {
            source: InitialContextTokenSource::Skill.as_str().to_string(),
            source_name: Some(skill_name),
            token_count: 10,
            use_count: 0,
            origin: SourceOrigin::Unknown,
            deferred: false,
            match_names: Vec::new(),
        },
        InitialContextSourceCount {
            source: InitialContextTokenSource::Mcp.as_str().to_string(),
            source_name: Some(mcp_name),
            token_count: 20,
            use_count: 0,
            origin: SourceOrigin::Unknown,
            deferred: false,
            match_names: Vec::new(),
        },
    ];
    let metrics = finished(
        vec![current],
        SessionSummary {
            initial_context: Some(InitialContextBreakdown { sources }),
            ..SessionSummary::default()
        },
    )
    .metrics();
    let counts = metrics
        .initial_context
        .expect("initial context")
        .sources
        .into_iter()
        .map(|source| source.use_count)
        .collect::<Vec<_>>();
    assert_eq!(counts, vec![2, 1]);
}

#[test]
fn observed_builtin_aliases_survive_internal_filtering() {
    use crate::analysis::initial_context::{
        InitialContextSourceCount, InitialContextTokenSource, SourceOrigin,
    };

    let mut current = event(Some(0), Role::Assistant, 1, 1);
    current.tools.push(ToolCall::new("alias-7"));
    let source = InitialContextSourceCount {
        source: InitialContextTokenSource::BuiltinTool.as_str().to_string(),
        source_name: Some("canonical".to_string()),
        token_count: 10,
        use_count: 0,
        origin: SourceOrigin::Unknown,
        deferred: false,
        match_names: (1..=7).map(|index| format!("alias-{index}")).collect(),
    };
    let metrics = finished(
        vec![current],
        SessionSummary {
            initial_context: Some(InitialContextBreakdown {
                sources: vec![source],
            }),
            ..SessionSummary::default()
        },
    )
    .metrics();
    assert_eq!(
        metrics.initial_context.expect("initial context").sources[0].use_count,
        1
    );
}

#[test]
fn observed_builtin_aliases_do_not_remain_in_summary_state() {
    use crate::analysis::initial_context::{
        InitialContextSourceCount, InitialContextTokenSource, SourceOrigin,
    };

    let mut current = event(Some(0), Role::Assistant, 1, 1);
    for index in 0..MAX_TOOL_NAMES {
        current.tools.push(ToolCall::new(format!("alias-{index}")));
    }
    let source = InitialContextSourceCount {
        source: InitialContextTokenSource::BuiltinTool.as_str().to_string(),
        source_name: Some("canonical".to_string()),
        token_count: 10,
        use_count: 0,
        origin: SourceOrigin::Unknown,
        deferred: false,
        match_names: (0..MAX_TOOL_NAMES + 100)
            .map(|index| format!("alias-{index}"))
            .collect(),
    };
    let accumulator = finished(
        vec![current],
        SessionSummary {
            initial_context: Some(InitialContextBreakdown {
                sources: vec![source],
            }),
            ..SessionSummary::default()
        },
    );
    let summary = accumulator.summary.as_ref().expect("summary");
    let row = &summary
        .initial_context
        .as_ref()
        .expect("initial context")
        .sources[0];
    assert_eq!(row.use_count, MAX_TOOL_NAMES as u32);
    assert!(row.match_names.is_empty());
    assert!(accumulator.retained_bytes() <= RETAINED_METRICS_BYTES_BOUND);
}

#[test]
fn skill_descriptions_prioritize_invoked_skills() {
    let mut current = event(Some(0), Role::Assistant, 1, 1);
    let mut skill = ToolCall::new("Skill");
    skill.detail = Some("zzz-invoked".to_string());
    current.tools.push(skill);
    let mut skill_descriptions = (0..80)
        .map(|index| (format!("aaa-{index:02}"), format!("unused {index}")))
        .collect::<HashMap<_, _>>();
    skill_descriptions.insert("zzz-invoked".to_string(), "used description".to_string());
    let metrics = finished(
        vec![current],
        SessionSummary {
            skill_descriptions,
            ..SessionSummary::default()
        },
    )
    .metrics();
    assert_eq!(
        metrics.skill_uses[0].description.as_deref(),
        Some("used description")
    );
}

#[test]
fn skill_descriptions_use_the_desktop_character_limit() {
    let name = "synthetic-skill";
    let mut current = event(Some(0), Role::Assistant, 1, 1);
    let mut skill = ToolCall::new("Skill");
    skill.detail = Some(name.to_string());
    current.tools.push(skill);
    let metrics = finished(
        vec![current],
        SessionSummary {
            skill_descriptions: [(name.to_string(), "界".repeat(400))].into_iter().collect(),
            ..SessionSummary::default()
        },
    )
    .metrics();
    let description = metrics.skill_uses[0]
        .description
        .as_deref()
        .expect("description");
    assert_eq!(description.chars().count(), MAX_DESCRIPTION_CHARS);
    assert!(description.ends_with('…'));
}

#[test]
fn tagged_models_do_not_share_the_breakdown_interner_budget() {
    let events = (0..MAX_MODELS)
        .map(|index| {
            let mut current = event(Some(index as i64), Role::Assistant, 1, 1);
            current.model = Some(format!("synthetic-model-{index}[1m]"));
            current
        })
        .collect();
    let accumulator = finished(events, SessionSummary::default());
    let metrics = accumulator.metrics();
    assert_eq!(metrics.model_breakdown.len(), MAX_MODELS);
    assert_eq!(accumulator.models_truncated, 0);
}

#[test]
fn repeated_model_runs_do_not_increment_the_truncation_count() {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "model-runs");
    for index in 0..MAX_MODEL_RUNS {
        let mut current = event(Some(index as i64), Role::Assistant, 1, 1);
        current.model = Some("synthetic-model".to_string());
        current.thinking_mode = Some(format!("mode-{index}"));
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(current)));
    }
    let mut repeated = event(Some(100), Role::Assistant, 1, 1);
    repeated.model = Some("synthetic-model".to_string());
    repeated.thinking_mode = Some("mode-0".to_string());
    accumulator.record(NormalizedRecord::MetricsEvent(Box::new(repeated)));
    assert_eq!(accumulator.model_runs_truncated, 0);

    let mut dropped = event(Some(101), Role::Assistant, 1, 1);
    dropped.model = Some("synthetic-model".to_string());
    dropped.thinking_mode = Some("new-mode".to_string());
    accumulator.record(NormalizedRecord::MetricsEvent(Box::new(dropped)));
    assert_eq!(accumulator.model_runs_truncated, 1);
}

#[test]
fn tool_name_saturation_does_not_hide_model_or_mode_transitions() {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "separate-interners");
    for index in 0..MAX_TOOL_NAMES {
        let mut current = event(Some(index as i64), Role::Assistant, 0, 0);
        current.tools.push(ToolCall::new(format!("tool-{index}")));
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(current)));
    }
    let mut transition = event(Some(10_000), Role::Assistant, 1, 1);
    transition.model = Some("claude-opus-4-6".to_string());
    transition.thinking_mode = Some("high".to_string());
    transition.speed = Some("fast".to_string());
    accumulator.record(NormalizedRecord::MetricsEvent(Box::new(transition)));
    accumulator.finish(SessionSummary::default());
    let metrics = accumulator.metrics();
    assert!(metrics.buckets.iter().any(|bucket| {
        bucket.model.as_deref() == Some("claude-opus-4-6")
            && bucket.thinking_mode.as_deref() == Some("high")
            && bucket.speed.as_deref() == Some("fast")
    }));
}

#[test]
fn capped_names_do_not_change_observed_turn_counts() {
    let mut accumulator = SessionMetricsAccumulator::new("synthetic", "caps");
    for index in 0..1_000 {
        let mut current = event(Some(index), Role::Assistant, 1, 1);
        current.model = Some(format!("model-{index}"));
        current.may_resolve_late_tool = true;
        let mut skill = ToolCall::new("Skill");
        skill.detail = Some(format!("skill-{index}"));
        current.tools.push(skill);
        current.tools.push(ToolCall {
            name: format!("mcp__server-{index}__tool"),
            category: ToolCategory::Other,
            detail: None,
        });
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(current)));
    }
    assert_eq!(accumulator.observed_turns(), 1_000);
    assert!(accumulator.interner.truncated > 0);
    assert!(accumulator.tool_names_truncated > 0);
    assert!(accumulator.mcp_servers_truncated > 0);
    assert!(accumulator.models_truncated > 0);
    assert!(accumulator.model_runs_truncated > 0);
    assert!(accumulator.skill_uses_truncated > 0);
    assert!(accumulator.late_candidates_truncated > 0);
    assert!(accumulator.slots.compactions > 0);
}

#[test]
fn earliest_timestamp_is_a_minimum() {
    let accumulator = finished(
        vec![
            event(Some(10), Role::User, 0, 0),
            event(Some(5), Role::Assistant, 1, 1),
        ],
        SessionSummary::default(),
    );
    assert_eq!(accumulator.earliest_ts_ms(), Some(5));
}
