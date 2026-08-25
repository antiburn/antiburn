// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::{HashMap, HashSet};
use std::mem::size_of;

use crate::analysis::engine::{
    BUCKETS, Bucket, CONTEXT_WINDOW, IDLE_GAP_MS, SessionMetrics, SkillUse, ToolMix,
};
use crate::analysis::interface::{NormalizedRecord, RecordSink, SessionSummary};
use crate::analysis::model::{
    CompactionTrigger, EventSource, ModelRun, NormalizedEvent, Role, ToolCall, Usage,
};
use crate::pricing::ModelTokens;

const CONTEXT_WINDOW_TIERS: [u64; 2] = [200_000, 1_000_000];
const CACHE_REHYDRATION_MIN_CONTEXT_TOKENS: u64 = 20_000;
const CACHE_REHYDRATION_WRITE_RATIO: f64 = 0.5;
const CACHE_REHYDRATION_PRIOR_READ_RATIO: f64 = 0.5;
const CACHE_REHYDRATION_MISS_READ_RATIO: f64 = 0.2;
const CACHE_REHYDRATION_CONTEXT_RETENTION_RATIO: f64 = 0.8;
const CACHE_REHYDRATION_REPLAY_RATIO: f64 = 0.5;
const CACHE_REHYDRATION_RECOVERY_READ_RATIO: f64 = 0.5;

pub(crate) struct MetricsIdentity {
    pub(crate) agent: String,
    pub(crate) session_id: String,
}

pub(crate) struct MetricTurn {
    ts_ms: Option<i64>,
    role: Role,
    source: EventSource,
    usage: Usage,
    tools: Vec<ToolCall>,
    model: Option<String>,
    thinking_mode: Option<String>,
    speed: Option<String>,
    has_thinking: bool,
    is_compaction_boundary: bool,
    compaction_trigger: Option<CompactionTrigger>,
    compaction_pre_tokens: Option<u64>,
    compaction_post_tokens: Option<u64>,
}

impl From<NormalizedEvent> for MetricTurn {
    fn from(event: NormalizedEvent) -> Self {
        Self {
            ts_ms: event.ts_ms,
            role: event.role,
            source: event.source,
            usage: event.usage,
            tools: event.tools,
            model: event.model,
            thinking_mode: event.thinking_mode,
            speed: event.speed,
            has_thinking: event.has_thinking,
            is_compaction_boundary: event.is_compaction_boundary,
            compaction_trigger: event.compaction_trigger,
            compaction_pre_tokens: event.compaction_pre_tokens,
            compaction_post_tokens: event.compaction_post_tokens,
        }
    }
}

#[derive(Default)]
pub(crate) struct OnlineTallies {
    tokens_in: u64,
    tokens_out: u64,
    billable_input_tokens: u64,
    billable_output_tokens: u64,
    billable_cache_read_tokens: u64,
    billable_cache_creation_tokens: u64,
    peak_context_tokens: u64,
    compaction_count: u64,
    tool_mix: ToolMix,
    grep_count: u64,
    event_count: usize,
}

impl OnlineTallies {
    pub(crate) fn observe(&mut self, turn: &MetricTurn, source: EventSource) {
        self.tokens_in = self
            .tokens_in
            .saturating_add(turn.usage.effective_input_tokens());
        self.tokens_out = self.tokens_out.saturating_add(turn.usage.output_tokens);
        self.billable_input_tokens = self
            .billable_input_tokens
            .saturating_add(turn.usage.input_tokens);
        self.billable_output_tokens = self
            .billable_output_tokens
            .saturating_add(turn.usage.output_tokens);
        self.billable_cache_read_tokens = self
            .billable_cache_read_tokens
            .saturating_add(turn.usage.cache_read_tokens);
        self.billable_cache_creation_tokens = self
            .billable_cache_creation_tokens
            .saturating_add(turn.usage.cache_creation_tokens);
        if source == EventSource::Parent {
            self.peak_context_tokens = self.peak_context_tokens.max(turn.usage.context_tokens());
            self.compaction_count = self
                .compaction_count
                .saturating_add(u64::from(turn.is_compaction_boundary));
        }
        for tool in &turn.tools {
            self.tool_mix.add(tool.category);
            if crate::analysis::model::ToolCategory::is_grep(&tool.name) {
                self.grep_count = self.grep_count.saturating_add(1);
            }
        }
        self.event_count = self.event_count.saturating_add(1);
    }
}

pub struct SessionMetricsAccumulator {
    identity: MetricsIdentity,
    turns: Vec<MetricTurn>,
    tallies: OnlineTallies,
    summary: Option<SessionSummary>,
}

impl SessionMetricsAccumulator {
    pub fn new(agent: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            identity: MetricsIdentity {
                agent: agent.into(),
                session_id: session_id.into(),
            },
            turns: Vec::new(),
            tallies: OnlineTallies::default(),
            summary: None,
        }
    }

    pub fn metrics(&self) -> SessionMetrics {
        let empty_summary = SessionSummary::default();
        let summary = self.summary.as_ref().unwrap_or(&empty_summary);
        let turns = self
            .turns
            .iter()
            .map(|turn| (turn.source, turn))
            .collect::<Vec<_>>();
        let mut metrics = finalize_metrics(
            MetricsIdentity {
                agent: self.identity.agent.clone(),
                session_id: self.identity.session_id.clone(),
            },
            summary,
            &turns,
            &self.tallies,
        );
        metrics.initial_context = summary.initial_context.clone();
        for skill_use in &mut metrics.skill_uses {
            if let Some(description) = summary.skill_descriptions.get(&skill_use.name) {
                skill_use.description = Some(description.clone());
            }
        }
        metrics
    }

    pub fn earliest_ts_ms(&self) -> Option<i64> {
        self.turns.iter().filter_map(|turn| turn.ts_ms).min()
    }

    pub fn retained_turns(&self) -> usize {
        self.turns.len()
    }

    pub fn retained_bytes(&self) -> usize {
        self.turns.iter().fold(0usize, |total, turn| {
            total
                .saturating_add(size_of::<MetricTurn>())
                .saturating_add(turn.tools.capacity() * size_of::<ToolCall>())
                .saturating_add(turn.model.as_ref().map_or(0, String::capacity))
                .saturating_add(turn.thinking_mode.as_ref().map_or(0, String::capacity))
                .saturating_add(turn.speed.as_ref().map_or(0, String::capacity))
                .saturating_add(
                    turn.tools
                        .iter()
                        .map(|tool| {
                            tool.name.capacity() + tool.detail.as_ref().map_or(0, String::capacity)
                        })
                        .sum::<usize>(),
                )
        })
    }

    pub(crate) fn from_parts(
        agent: String,
        session_id: String,
        events: Vec<NormalizedEvent>,
        summary: SessionSummary,
    ) -> Self {
        let mut accumulator = Self::new(agent, session_id);
        for event in events {
            accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
        }
        accumulator.finish(summary);
        accumulator
    }
}

impl RecordSink for SessionMetricsAccumulator {
    fn record(&mut self, record: NormalizedRecord) {
        if let NormalizedRecord::MetricsEvent(event) = record {
            let turn = MetricTurn::from(*event);
            self.tallies.observe(&turn, turn.source);
            self.turns.push(turn);
        }
    }

    fn finish(&mut self, summary: SessionSummary) {
        for (ordinal, tool) in &summary.late_tools {
            if let Some(turn) = self.turns.get_mut(*ordinal) {
                self.tallies.tool_mix.add(tool.category);
                if crate::analysis::model::ToolCategory::is_grep(&tool.name) {
                    self.tallies.grep_count = self.tallies.grep_count.saturating_add(1);
                }
                turn.tools.push(tool.clone());
            }
        }
        self.summary = Some(summary);
    }
}

pub fn merge_metrics(
    parent: &SessionMetricsAccumulator,
    subagents: &[SessionMetricsAccumulator],
) -> SessionMetrics {
    let capacity = parent.turns.len()
        + subagents
            .iter()
            .map(|subagent| subagent.turns.len())
            .sum::<usize>();
    let mut turns = Vec::with_capacity(capacity);
    push_stream(&parent.turns, EventSource::Parent, &mut turns);
    for subagent in subagents {
        push_stream(&subagent.turns, EventSource::Subagent, &mut turns);
    }
    turns.sort_by_key(|(timestamp, _, _)| *timestamp);
    let sourced_turns = turns
        .into_iter()
        .map(|(_, source, turn)| (source, turn))
        .collect::<Vec<_>>();
    let mut tallies = OnlineTallies::default();
    for (source, turn) in &sourced_turns {
        tallies.observe(turn, *source);
    }
    let empty_summary = SessionSummary::default();
    finalize_metrics(
        MetricsIdentity {
            agent: parent.identity.agent.clone(),
            session_id: parent.identity.session_id.clone(),
        },
        parent.summary.as_ref().unwrap_or(&empty_summary),
        &sourced_turns,
        &tallies,
    )
}

fn push_stream<'a>(
    stream: &'a [MetricTurn],
    source: EventSource,
    turns: &mut Vec<(i64, EventSource, &'a MetricTurn)>,
) {
    let mut last_ts = i64::MIN;
    for turn in stream {
        if let Some(timestamp) = turn.ts_ms {
            last_ts = timestamp;
        }
        turns.push((last_ts, source, turn));
    }
}

#[derive(Clone, Copy)]
struct CacheTurn<'a> {
    event_index: usize,
    context_tokens: u64,
    fresh_input_tokens: u64,
    cache_read_tokens: u64,
    first_turn_after_compaction: bool,
    model: Option<&'a str>,
}

fn cache_ratio(tokens: u64, context_tokens: u64) -> f64 {
    if context_tokens == 0 {
        return 0.0;
    }
    tokens as f64 / context_tokens as f64
}

fn same_known_model(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

fn is_cache_rehydration_turn(
    context_tokens: u64,
    cache_write_tokens: u64,
    previous_context_tokens: u64,
    previous_cache_read_tokens: u64,
    first_turn_after_compaction: bool,
) -> bool {
    if first_turn_after_compaction
        || context_tokens < CACHE_REHYDRATION_MIN_CONTEXT_TOKENS
        || previous_context_tokens == 0
    {
        return false;
    }
    cache_ratio(cache_write_tokens, context_tokens) >= CACHE_REHYDRATION_WRITE_RATIO
        && cache_ratio(previous_cache_read_tokens, previous_context_tokens)
            >= CACHE_REHYDRATION_PRIOR_READ_RATIO
}

fn inferred_cache_rehydration_turn(previous: CacheTurn<'_>, current: CacheTurn<'_>) -> bool {
    if current.first_turn_after_compaction
        || current.context_tokens < CACHE_REHYDRATION_MIN_CONTEXT_TOKENS
        || cache_ratio(previous.cache_read_tokens, previous.context_tokens)
            < CACHE_REHYDRATION_PRIOR_READ_RATIO
        || cache_ratio(current.cache_read_tokens, current.context_tokens)
            > CACHE_REHYDRATION_MISS_READ_RATIO
        || !same_known_model(previous.model, current.model)
    {
        return false;
    }
    let retained_context_ratio = current.context_tokens as f64 / previous.context_tokens as f64;
    if retained_context_ratio < CACHE_REHYDRATION_CONTEXT_RETENTION_RATIO {
        return false;
    }
    let context_growth = current
        .context_tokens
        .saturating_sub(previous.context_tokens);
    let replayed_input = current.fresh_input_tokens.saturating_sub(context_growth);
    cache_ratio(replayed_input, current.context_tokens) >= CACHE_REHYDRATION_REPLAY_RATIO
}

fn cache_rehydration_event_indices(
    turns: &[(EventSource, &MetricTurn)],
    summary: &SessionSummary,
) -> HashSet<usize> {
    let mut indices = HashSet::new();
    let mut cache_turns = Vec::new();
    let mut previous_context = 0u64;
    let mut previous_cache_read = 0u64;
    let mut first_turn_after_compaction = false;
    let mut active_model = summary.model.as_deref();

    for (event_index, (source, turn)) in turns.iter().enumerate() {
        if *source != EventSource::Parent {
            continue;
        }
        if let Some(model) = turn.model.as_deref().filter(|model| !model.is_empty()) {
            active_model = Some(model);
        }
        if turn.is_compaction_boundary {
            first_turn_after_compaction = true;
        }
        let context_tokens = turn.usage.context_tokens();
        if context_tokens == 0 {
            continue;
        }
        if summary.cache_write_tokens_available
            && is_cache_rehydration_turn(
                context_tokens,
                turn.usage.cache_creation_tokens,
                previous_context,
                previous_cache_read,
                first_turn_after_compaction,
            )
        {
            indices.insert(event_index);
        }
        cache_turns.push(CacheTurn {
            event_index,
            context_tokens,
            fresh_input_tokens: turn.usage.input_tokens,
            cache_read_tokens: turn.usage.cache_read_tokens,
            first_turn_after_compaction,
            model: active_model,
        });
        previous_context = context_tokens;
        previous_cache_read = turn.usage.cache_read_tokens;
        first_turn_after_compaction = false;
    }

    if summary.cache_write_tokens_available {
        return indices;
    }
    for window in cache_turns.windows(3) {
        let [previous, current, next] = window else {
            continue;
        };
        let recovery_ratio = cache_ratio(next.cache_read_tokens, next.context_tokens);
        let recovery_retention = next.context_tokens as f64 / current.context_tokens as f64;
        if inferred_cache_rehydration_turn(*previous, *current)
            && !next.first_turn_after_compaction
            && recovery_ratio >= CACHE_REHYDRATION_RECOVERY_READ_RATIO
            && recovery_retention >= CACHE_REHYDRATION_CONTEXT_RETENTION_RATIO
            && same_known_model(current.model, next.model)
        {
            indices.insert(current.event_index);
        }
    }
    indices
}

pub(crate) fn finalize_metrics(
    identity: MetricsIdentity,
    summary: &SessionSummary,
    turns: &[(EventSource, &MetricTurn)],
    tallies: &OnlineTallies,
) -> SessionMetrics {
    let mut timestamps = turns
        .iter()
        .filter_map(|(_, turn)| turn.ts_ms)
        .collect::<Vec<_>>();
    timestamps.sort_unstable();
    let (first_ts, last_ts) = match (timestamps.first(), timestamps.last()) {
        (Some(&first), Some(&last)) => (first, last),
        _ => (0, 0),
    };
    let duration_secs = ((last_ts - first_ts).max(0) / 1000) as u64;
    let active_ms = timestamps
        .windows(2)
        .map(|window| (window[1] - window[0]).clamp(0, IDLE_GAP_MS))
        .sum::<i64>();
    let active_secs = (active_ms / 1000) as u64;

    let mut cumulative_active = Vec::with_capacity(timestamps.len());
    let mut accumulated = 0i64;
    for (index, &timestamp) in timestamps.iter().enumerate() {
        if index > 0 {
            accumulated += (timestamp - timestamps[index - 1]).clamp(0, IDLE_GAP_MS);
        }
        if cumulative_active.last().map(|&(value, _)| value) != Some(timestamp) {
            cumulative_active.push((timestamp, accumulated));
        }
    }
    let active_progress = |timestamp: i64| {
        if active_ms <= 0 {
            return 0.0;
        }
        match cumulative_active.binary_search_by(|&(value, _)| value.cmp(&timestamp)) {
            Ok(index) => cumulative_active[index].1 as f32 / active_ms as f32,
            Err(_) => 0.0,
        }
    };

    let mut skill_uses = Vec::new();
    let mut skill_event_indices = Vec::new();
    let mut buckets = vec![Bucket::default(); BUCKETS];
    let cache_rehydration_events = cache_rehydration_event_indices(turns, summary);
    let mut last_progress = 0.0f32;
    let mut previous_turn_ts: Option<i64> = None;
    let mut cache_rehydration_count = 0u64;

    for (index, (source, turn)) in turns.iter().enumerate() {
        if active_ms > 0
            && let Some(timestamp) = turn.ts_ms
        {
            last_progress = active_progress(timestamp);
        }
        let progress = if active_ms > 0 {
            last_progress
        } else if turns.len() > 1 {
            index as f32 / (turns.len() - 1) as f32
        } else {
            0.0
        }
        .clamp(0.0, 1.0);
        let bucket_index = ((progress * BUCKETS as f32) as usize).min(BUCKETS - 1);
        let bucket = &mut buckets[bucket_index];

        if *source == EventSource::Subagent {
            bucket.subagent_tokens = bucket
                .subagent_tokens
                .saturating_add(turn.usage.effective_input_tokens())
                .saturating_add(turn.usage.output_tokens);
        }
        if *source == EventSource::Parent {
            bucket.tokens_in = bucket
                .tokens_in
                .saturating_add(turn.usage.effective_input_tokens());
            bucket.tokens_out = bucket.tokens_out.saturating_add(turn.usage.output_tokens);
            bucket.cache_read_tokens = bucket
                .cache_read_tokens
                .saturating_add(turn.usage.cache_read_tokens);
            bucket.cache_write_tokens = bucket
                .cache_write_tokens
                .saturating_add(turn.usage.cache_creation_tokens);
            bucket.context_tokens = bucket.context_tokens.max(turn.usage.context_tokens());
            bucket.is_compaction_boundary |= turn.is_compaction_boundary;
            if turn.is_compaction_boundary {
                bucket.compaction_trigger = turn.compaction_trigger;
                bucket.compaction_pre_tokens = turn.compaction_pre_tokens;
                bucket.compaction_post_tokens = turn.compaction_post_tokens;
            }
            if turn.usage.context_tokens() > 0 {
                let secs_since_prior_turn =
                    turn.ts_ms.zip(previous_turn_ts).map(|(current, prior)| {
                        u64::try_from((current - prior).max(0) / 1000).unwrap_or(0)
                    });
                let is_cache_rehydration = cache_rehydration_events.contains(&index);
                if is_cache_rehydration {
                    bucket.is_cache_rehydration = true;
                    cache_rehydration_count = cache_rehydration_count.saturating_add(1);
                }
                if is_cache_rehydration
                    || (!bucket.is_cache_rehydration && bucket.secs_since_prior_turn.is_none())
                {
                    bucket.secs_since_prior_turn = secs_since_prior_turn;
                }
                previous_turn_ts = turn.ts_ms;
            }
            let launches = turn
                .tools
                .iter()
                .filter(|tool| tool.name.eq_ignore_ascii_case("task"))
                .count() as u32;
            bucket.subagent_launches = bucket.subagent_launches.saturating_add(launches);
            if turn.role == Role::User {
                bucket.user_prompts = bucket.user_prompts.saturating_add(1);
            }
            if let Some(model) = turn.model.as_deref().filter(|model| !model.is_empty()) {
                bucket.model = Some(model.to_string());
            }
            if let Some(mode) = turn
                .thinking_mode
                .as_deref()
                .filter(|mode| !mode.is_empty())
            {
                bucket.thinking_mode = Some(mode.to_string());
            }
            if let Some(speed) = turn.speed.as_deref().filter(|speed| !speed.is_empty()) {
                bucket.speed = Some(speed.to_string());
            }
            bucket.has_thinking |= turn.has_thinking;
            if let Some(tool) = turn.tools.last() {
                bucket.last_tool = Some(tool.name.clone());
            }
        }

        for tool in &turn.tools {
            if tool.name.eq_ignore_ascii_case("skill") {
                skill_uses.push(SkillUse {
                    name: tool.detail.clone().unwrap_or_else(|| "skill".to_string()),
                    progress,
                    description: None,
                    duration_ms: None,
                    tokens_out: turn.usage.output_tokens,
                    context_tokens: turn.usage.context_tokens(),
                });
                skill_event_indices.push(index);
            }
        }
    }

    for bucket in &mut buckets {
        if bucket.is_compaction_boundary {
            bucket.context_tokens = 0;
        }
    }
    for (skill_use, &event_index) in skill_uses.iter_mut().zip(&skill_event_indices) {
        if let Some(timestamp) = turns[event_index].1.ts_ms
            && let Some(&next_timestamp) = timestamps.iter().find(|&&value| value > timestamp)
        {
            skill_use.duration_ms = Some((next_timestamp - timestamp).clamp(0, IDLE_GAP_MS));
        }
    }

    let mut model_breakdown: HashMap<String, ModelTokens> = HashMap::new();
    let mut model_runs = Vec::new();
    let mut seen_model_runs = HashSet::new();
    for (_, turn) in turns {
        let usage = turn.usage;
        let has_tokens = usage.input_tokens != 0
            || usage.output_tokens != 0
            || usage.cache_read_tokens != 0
            || usage.cache_creation_tokens != 0;
        if has_tokens && let Some(model) = turn.model.as_deref().or(summary.model.as_deref()) {
            let model = crate::analysis::pricing::strip_window_tag(model)
                .trim()
                .to_string();
            if !model.is_empty() {
                let run = ModelRun {
                    model: model.clone(),
                    thinking_mode: turn
                        .thinking_mode
                        .as_deref()
                        .map(str::trim)
                        .filter(|mode| !mode.is_empty())
                        .map(str::to_string),
                };
                if seen_model_runs.insert(run.clone()) {
                    model_runs.push(run);
                }
                let entry = model_breakdown.entry(model).or_default();
                entry.input_tokens = entry.input_tokens.saturating_add(usage.input_tokens);
                entry.output_tokens = entry.output_tokens.saturating_add(usage.output_tokens);
                entry.cache_read_tokens = entry
                    .cache_read_tokens
                    .saturating_add(usage.cache_read_tokens);
                entry.cache_creation_tokens = entry
                    .cache_creation_tokens
                    .saturating_add(usage.cache_creation_tokens);
            }
        }
    }

    let context_available = identity.agent != "claude" || summary.context_window.is_some();
    let context_window = resolve_context_window(
        summary.context_window.unwrap_or(CONTEXT_WINDOW),
        tallies.peak_context_tokens,
    );
    let cost = crate::analysis::pricing::price_breakdown(&model_breakdown);

    SessionMetrics {
        agent: identity.agent,
        session_id: identity.session_id,
        duration_secs,
        active_secs,
        event_count: tallies.event_count,
        tokens_in: tallies.tokens_in,
        tokens_out: tallies.tokens_out,
        peak_context_tokens: tallies.peak_context_tokens,
        compaction_count: tallies.compaction_count,
        cache_rehydration_count,
        context_available,
        context_window,
        tool_mix: tallies.tool_mix,
        grep_count: tallies.grep_count,
        buckets,
        initial_context: None,
        model: summary.model.clone(),
        model_runs,
        billable_input_tokens: tallies.billable_input_tokens,
        billable_output_tokens: tallies.billable_output_tokens,
        billable_cache_read_tokens: tallies.billable_cache_read_tokens,
        billable_cache_creation_tokens: tallies.billable_cache_creation_tokens,
        model_breakdown,
        cost,
        skill_uses,
    }
}

fn resolve_context_window(reported: u64, peak: u64) -> u64 {
    let base = reported.max(1);
    if peak <= base {
        return base;
    }
    CONTEXT_WINDOW_TIERS
        .iter()
        .copied()
        .find(|&tier| tier >= peak)
        .unwrap_or(peak)
}
