use std::collections::{HashMap, HashSet};
use std::mem::size_of;

use crate::analysis::efficiency::EfficiencyTotals;
use crate::analysis::engine::{
    BUCKETS, Bucket, CONTEXT_WINDOW, CacheRehydration, IDLE_GAP_MS, SessionMetrics, SkillUse,
};
use crate::analysis::interface::{NormalizedRecord, RecordSink, SessionSummary};
use crate::analysis::model::{
    CompactionTrigger, EventSource, ModelRun, NormalizedEvent, Role, ToolCall, Usage,
};
use crate::analysis::pricing::{lookup_pricing, strip_window_tag};
use crate::pricing::ModelTokens;

const CONTEXT_WINDOW_TIERS: [u64; 2] = [200_000, 1_000_000];
const CACHE_REHYDRATION_MIN_CONTEXT_TOKENS: u64 = 20_000;
const CACHE_REHYDRATION_PRIOR_READ_RATIO: f64 = 0.5;
const CACHE_REHYDRATION_CONTEXT_RETENTION_RATIO: f64 = 0.8;
const CACHE_REHYDRATION_RECOVERY_READ_RATIO: f64 = 0.5;
const CLAUDE_REHYDRATION_MIN_USER_INACTIVITY_SECS: u64 = 60 * 60;
const CODEX_REHYDRATION_MIN_USER_INACTIVITY_SECS: u64 = 30 * 60;

pub(crate) struct MetricsIdentity {
    pub(crate) agent: String,
    pub(crate) session_id: String,
}

pub(crate) struct MetricTurn {
    ts_ms: Option<i64>,
    role: Role,
    message_id: Option<String>,
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
    /// See `NormalizedEvent::wrapper_tool`.
    wrapper_tool: Option<String>,
}

impl From<NormalizedEvent> for MetricTurn {
    fn from(event: NormalizedEvent) -> Self {
        Self {
            ts_ms: event.ts_ms,
            role: event.role,
            message_id: event.message_id,
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
            wrapper_tool: event.wrapper_tool,
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
        // Fill `use_count` here too, from this same session's `skill_uses` and
        // `mcp_tool_calls`, so the streaming path matches the batch path in
        // `analyze_sources_with` (both compute the same breakdown, independently).
        if let Some(breakdown) = metrics.initial_context.as_mut() {
            crate::analysis::initial_context::fill_use_counts(
                breakdown,
                &metrics.skill_uses,
                &metrics.mcp_tool_calls,
                &metrics.tool_calls_by_name,
            );
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
                .saturating_add(turn.message_id.as_ref().map_or(0, String::capacity))
                .saturating_add(turn.model.as_ref().map_or(0, String::capacity))
                .saturating_add(turn.thinking_mode.as_ref().map_or(0, String::capacity))
                .saturating_add(turn.speed.as_ref().map_or(0, String::capacity))
                .saturating_add(turn.wrapper_tool.as_ref().map_or(0, String::capacity))
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
    cache_read_tokens: u64,
    first_turn_after_compaction: bool,
    user_inactive_secs: Option<u64>,
    model: Option<&'a str>,
}

/// The material cache events and their user-inactivity classification.
#[derive(Default)]
struct CacheMissEvents {
    rehydrations: HashSet<usize>,
    provider_misses: HashSet<usize>,
    compositions: HashMap<usize, CacheRehydration>,
}

impl CacheMissEvents {
    fn insert(
        &mut self,
        previous_context: u64,
        turn: CacheTurn<'_>,
        minimum_user_inactivity_secs: Option<u64>,
    ) {
        let is_rehydration = minimum_user_inactivity_secs.is_some_and(|minimum| {
            turn.user_inactive_secs
                .is_some_and(|inactivity| inactivity >= minimum)
        });
        if is_rehydration {
            self.rehydrations.insert(turn.event_index);
        } else {
            self.provider_misses.insert(turn.event_index);
        }
        self.compositions
            .insert(turn.event_index, cache_rehydration(previous_context, turn));
    }
}

fn cache_ratio(tokens: u64, context_tokens: u64) -> f64 {
    if context_tokens == 0 {
        return 0.0;
    }
    tokens as f64 / context_tokens as f64
}

fn cache_rehydration(previous_context: u64, turn: CacheTurn<'_>) -> CacheRehydration {
    let context_tokens = turn.context_tokens;
    let still_cached_tokens = turn.cache_read_tokens.min(context_tokens);
    let uncached_tokens = context_tokens.saturating_sub(still_cached_tokens);
    let growth_tokens = context_tokens
        .saturating_sub(previous_context)
        .min(uncached_tokens);
    CacheRehydration {
        context_tokens,
        still_cached_tokens,
        rewritten_tokens: uncached_tokens.saturating_sub(growth_tokens),
        growth_tokens,
        user_inactive_secs: turn.user_inactive_secs,
    }
}

fn same_known_model(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

fn is_direct_cache_event(
    previous: CacheTurn<'_>,
    current: CacheTurn<'_>,
    cache_write_tokens: u64,
) -> bool {
    cache_write_tokens >= CACHE_REHYDRATION_MIN_CONTEXT_TOKENS
        && is_material_cache_event(previous, current)
}

fn inferred_cache_rehydration_turn(previous: CacheTurn<'_>, current: CacheTurn<'_>) -> bool {
    is_material_cache_event(previous, current)
}

fn is_material_cache_event(previous: CacheTurn<'_>, current: CacheTurn<'_>) -> bool {
    if current.first_turn_after_compaction
        || current.context_tokens < CACHE_REHYDRATION_MIN_CONTEXT_TOKENS
        || cache_ratio(previous.cache_read_tokens, previous.context_tokens)
            < CACHE_REHYDRATION_PRIOR_READ_RATIO
        || !same_known_model(previous.model, current.model)
    {
        return false;
    }
    let retained_context_ratio = current.context_tokens as f64 / previous.context_tokens as f64;
    if retained_context_ratio < CACHE_REHYDRATION_CONTEXT_RETENTION_RATIO {
        return false;
    }
    cache_rehydration(previous.context_tokens, current).rewritten_tokens
        >= CACHE_REHYDRATION_MIN_CONTEXT_TOKENS
}

fn rehydration_min_user_inactivity_secs(agent: &str) -> Option<u64> {
    match agent {
        "claude" => Some(CLAUDE_REHYDRATION_MIN_USER_INACTIVITY_SECS),
        "codex" => Some(CODEX_REHYDRATION_MIN_USER_INACTIVITY_SECS),
        _ => None,
    }
}

fn cache_miss_events(
    turns: &[(EventSource, &MetricTurn)],
    summary: &SessionSummary,
    agent: &str,
) -> CacheMissEvents {
    let mut events = CacheMissEvents::default();
    let mut cache_turns = Vec::new();
    let mut first_turn_after_compaction = false;
    let mut has_explicit_cache_writes = false;
    let mut active_model = summary.model.as_deref();
    let mut previous_turn_ts: Option<i64> = None;
    let mut pending_user_prompt = false;
    let mut pending_user_prompt_ts = None;
    let minimum_user_inactivity_secs = rehydration_min_user_inactivity_secs(agent);

    for (event_index, (source, turn)) in turns.iter().enumerate() {
        if *source != EventSource::Parent {
            continue;
        }
        if turn.role == Role::User {
            pending_user_prompt = true;
            if let Some(timestamp) = turn.ts_ms {
                pending_user_prompt_ts.get_or_insert(timestamp);
            }
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
        let cache_turn = CacheTurn {
            event_index,
            context_tokens,
            cache_read_tokens: turn.usage.cache_read_tokens,
            first_turn_after_compaction,
            user_inactive_secs: pending_user_prompt
                .then(|| {
                    pending_user_prompt_ts
                        .zip(previous_turn_ts)
                        .map(|(prompt, previous)| {
                            u64::try_from((prompt - previous).max(0) / 1_000).unwrap_or(0)
                        })
                })
                .flatten(),
            model: active_model,
        };
        pending_user_prompt = false;
        pending_user_prompt_ts = None;
        has_explicit_cache_writes |= turn.usage.cache_creation_tokens > 0;
        if let Some(previous) = cache_turns.last().copied()
            && is_direct_cache_event(previous, cache_turn, turn.usage.cache_creation_tokens)
        {
            events.insert(
                previous.context_tokens,
                cache_turn,
                minimum_user_inactivity_secs,
            );
        }
        cache_turns.push(cache_turn);
        previous_turn_ts = turn.ts_ms;
        first_turn_after_compaction = false;
    }

    if has_explicit_cache_writes {
        return events;
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
            events.insert(
                previous.context_tokens,
                *current,
                minimum_user_inactivity_secs,
            );
        }
    }
    events
}

struct ReferenceEfficiencyTurn<'a> {
    event_index: usize,
    ts: i64,
    source: EventSource,
    model: Option<&'a str>,
    usage: Usage,
}

/// One missing-model turn's raw usage, aggregated for the fallback model.
///
/// This mirrors `crate::analysis::efficiency`'s `FallbackOverflow`, but as
/// an independent sum: it does not call the reducer's helpers. The op
/// order matters for bit-exactness with the reducer. Per turn, compute
/// `input_share = input / fresh`. Then add, in this order,
/// `new_tokens * input_share`, `new_tokens * (1.0 - input_share)`,
/// `rewrite_tokens * input_share`, and `rewrite_tokens * (1.0 - input_share)`
/// to the four running token totals. Add cache-read tokens, growth, and
/// output tokens as integer sums.
#[derive(Default)]
struct ReferenceFallbackAggregate {
    growth_tokens: u64,
    output_tokens: u64,
    new_input_tokens: f64,
    new_cache_tokens: f64,
    rewrite_input_tokens: f64,
    rewrite_cache_tokens: f64,
    cache_read_tokens: u64,
    turns: u64,
}

struct ReferenceRewrite {
    event_index: usize,
    ts: i64,
    tokens: u64,
}

/// Returns the independent reference computation of efficiency totals.
///
/// This keeps its own turn-merge and pricing logic instead of calling into
/// `crate::analysis::efficiency`, so it can catch bugs the reducer shares
/// with itself. It still folds turns in the reducer's canonical order. A
/// turn with a known model prices and adds to the totals right away, in
/// fold (timestamp) order. A turn with no model of its own — `model` is
/// `None`, not an empty or unpriceable string — joins one fallback
/// aggregate instead. That aggregate prices once, under the session
/// fallback model, after every priced turn above has already summed.
fn reference_efficiency(
    turns: &[(EventSource, &MetricTurn)],
    fallback_model: Option<&str>,
) -> (EfficiencyTotals, Vec<ReferenceRewrite>) {
    let mut merged = Vec::new();
    let mut index_by_id = HashMap::new();
    let mut last_ts = i64::MIN;
    for (event_index, (source, event)) in turns.iter().enumerate() {
        if let Some(timestamp) = event.ts_ms {
            last_ts = timestamp;
        }
        if event.role != Role::Assistant {
            continue;
        }
        if let Some(&index) = event
            .message_id
            .as_deref()
            .and_then(|id| index_by_id.get(id))
        {
            let current: &mut ReferenceEfficiencyTurn<'_> = &mut merged[index];
            current.usage = current.usage.saturating_add(event.usage);
            if current.model.is_none() {
                current.model = event.model.as_deref();
            }
            continue;
        }
        if let Some(id) = event.message_id.as_deref() {
            index_by_id.insert(id, merged.len());
        }
        merged.push(ReferenceEfficiencyTurn {
            event_index,
            ts: last_ts,
            source: *source,
            model: event.model.as_deref(),
            usage: event.usage,
        });
    }
    merged.retain(|turn| turn.usage.output_tokens > 0);
    merged.sort_by_key(|turn| turn.ts);

    let mut totals = EfficiencyTotals::default();
    let mut fallback = ReferenceFallbackAggregate::default();
    let mut rewrites = Vec::new();
    let mut previous_context = None;
    let mut previous_parent_context = None;
    for turn in merged {
        let usage = turn.usage;
        let context = usage.context_tokens();
        let growth = previous_context.map_or(context, |prior: u64| context.saturating_sub(prior));
        previous_context = Some(context);
        if turn.source == EventSource::Parent {
            let parent_growth =
                previous_parent_context.map_or(context, |prior: u64| context.saturating_sub(prior));
            previous_parent_context = Some(context);
            let rewrite_tokens = usage.effective_input_tokens().saturating_sub(parent_growth);
            if rewrite_tokens > 0 {
                rewrites.push(ReferenceRewrite {
                    event_index: turn.event_index,
                    ts: turn.ts,
                    tokens: rewrite_tokens,
                });
            }
        }

        let Some(model) = turn.model else {
            let fresh = usage
                .input_tokens
                .saturating_add(usage.cache_creation_tokens);
            let new_tokens = fresh.min(growth);
            let rewrite_tokens = fresh.saturating_sub(new_tokens);
            let input_share = if fresh == 0 {
                0.0
            } else {
                usage.input_tokens as f64 / fresh as f64
            };
            fallback.growth_tokens = fallback.growth_tokens.saturating_add(growth);
            fallback.output_tokens = fallback.output_tokens.saturating_add(usage.output_tokens);
            fallback.new_input_tokens += new_tokens as f64 * input_share;
            fallback.new_cache_tokens += new_tokens as f64 * (1.0 - input_share);
            fallback.rewrite_input_tokens += rewrite_tokens as f64 * input_share;
            fallback.rewrite_cache_tokens += rewrite_tokens as f64 * (1.0 - input_share);
            fallback.cache_read_tokens = fallback
                .cache_read_tokens
                .saturating_add(usage.cache_read_tokens);
            fallback.turns = fallback.turns.saturating_add(1);
            continue;
        };

        let model = strip_window_tag(model).trim();
        if model.is_empty() {
            totals.unpriced_turns = totals.unpriced_turns.saturating_add(1);
            continue;
        }
        let Some(price) = lookup_pricing(model) else {
            totals.unpriced_turns = totals.unpriced_turns.saturating_add(1);
            continue;
        };
        let fresh = usage
            .input_tokens
            .saturating_add(usage.cache_creation_tokens);
        let new_tokens = fresh.min(growth);
        let rewrite_tokens = fresh.saturating_sub(new_tokens);
        let fresh_rate = if fresh == 0 {
            0.0
        } else {
            (usage.input_tokens as f64 * price.input_cost_per_token
                + usage.cache_creation_tokens as f64 * price.cache_write_cost_per_token)
                / fresh as f64
        };
        let new_work = usage.output_tokens as f64 * price.output_cost_per_token
            + new_tokens as f64 * fresh_rate;
        let carry = usage.cache_read_tokens as f64 * price.cache_read_cost_per_token;
        let rewrite = rewrite_tokens as f64 * fresh_rate;
        totals.new_work_usd += new_work;
        totals.carry_usd += carry;
        totals.rewrite_usd += rewrite;
        totals.total_usd += new_work + carry + rewrite;
        totals.growth_tokens = totals.growth_tokens.saturating_add(growth);
        totals.output_tokens = totals.output_tokens.saturating_add(usage.output_tokens);
        totals.priced_turns = totals.priced_turns.saturating_add(1);
    }

    // The fallback aggregate prices once, after every priced turn above has
    // already summed. This matches `EfficiencyReducer::finish`'s order.
    let priced_fallback = fallback_model
        .map(|name| strip_window_tag(name).trim())
        .filter(|name| !name.is_empty())
        .and_then(lookup_pricing);
    match priced_fallback {
        None => {
            totals.unpriced_turns = totals.unpriced_turns.saturating_add(fallback.turns);
        }
        Some(price) => {
            let new_work = fallback.output_tokens as f64 * price.output_cost_per_token
                + fallback.new_input_tokens * price.input_cost_per_token
                + fallback.new_cache_tokens * price.cache_write_cost_per_token;
            let carry = fallback.cache_read_tokens as f64 * price.cache_read_cost_per_token;
            let rewrite = fallback.rewrite_input_tokens * price.input_cost_per_token
                + fallback.rewrite_cache_tokens * price.cache_write_cost_per_token;
            totals.new_work_usd += new_work;
            totals.carry_usd += carry;
            totals.rewrite_usd += rewrite;
            totals.total_usd += new_work + carry + rewrite;
            totals.growth_tokens = totals.growth_tokens.saturating_add(fallback.growth_tokens);
            totals.output_tokens = totals.output_tokens.saturating_add(fallback.output_tokens);
            totals.priced_turns = totals.priced_turns.saturating_add(fallback.turns);
        }
    }
    (totals, rewrites)
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
    let mut mcp_tool_calls: HashMap<String, u32> = HashMap::new();
    let mut tool_calls_by_name: HashMap<String, u32> = HashMap::new();
    let mut buckets = vec![Bucket::default(); BUCKETS];
    let cache_miss_events = cache_miss_events(turns, summary, &identity.agent);
    let mut last_progress = 0.0f32;
    let mut previous_turn_ts: Option<i64> = None;
    let mut cache_rehydration_count = 0u64;
    let mut provider_cache_miss_count = 0u64;

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
                let is_cache_rehydration = cache_miss_events.rehydrations.contains(&index);
                let is_provider_cache_miss = cache_miss_events.provider_misses.contains(&index);
                if is_cache_rehydration {
                    bucket.is_cache_rehydration = true;
                    cache_rehydration_count = cache_rehydration_count.saturating_add(1);
                }
                if is_provider_cache_miss {
                    bucket.is_cache_routing_miss = true;
                    provider_cache_miss_count = provider_cache_miss_count.saturating_add(1);
                }
                if let Some(composition) = cache_miss_events.compositions.get(&index) {
                    bucket.cache_rehydration = Some(*composition);
                }
                if is_cache_rehydration
                    || is_provider_cache_miss
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
            *tool_calls_by_name.entry(tool.name.clone()).or_insert(0) += 1;
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
            } else if let Some(server) = mcp_server_name(&tool.name) {
                *mcp_tool_calls
                    .entry(server.to_ascii_lowercase())
                    .or_insert(0) += 1;
            }
        }
        // A Codex `exec` wrapper unwraps into its nested calls in `tools`
        // (real tool-mix accounting), but the wrapper itself is the built-in
        // tool whose definition costs tokens, so its use counts here too.
        if let Some(wrapper) = &turn.wrapper_tool {
            *tool_calls_by_name.entry(wrapper.clone()).or_insert(0) += 1;
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
    let (efficiency, rewrites) = reference_efficiency(turns, summary.model.as_deref());
    for rewrite in rewrites {
        let progress = if active_ms > 0 {
            active_progress(rewrite.ts)
        } else if turns.len() > 1 {
            rewrite.event_index as f32 / (turns.len() - 1) as f32
        } else {
            0.0
        }
        .clamp(0.0, 1.0);
        let bucket_index = ((progress * BUCKETS as f32) as usize).min(BUCKETS - 1);
        buckets[bucket_index].rewrite_tokens = buckets[bucket_index]
            .rewrite_tokens
            .saturating_add(rewrite.tokens);
    }

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
        cache_routing_miss_count: provider_cache_miss_count,
        cache_rehydration_count,
        context_available,
        context_window,
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
        efficiency,
        skill_uses,
        mcp_tool_calls,
        tool_calls_by_name,
    }
}

/// Extract the server-name segment from an MCP tool name shaped
/// `mcp__<server>__<tool>`. The `mcp__` prefix check is case-insensitive.
/// Returns `None` for a name that is not an MCP tool call.
fn mcp_server_name(name: &str) -> Option<&str> {
    if name.len() < 5 || !name.as_bytes()[..5].eq_ignore_ascii_case(b"mcp__") {
        return None;
    }
    let rest = &name[5..];
    if rest.is_empty() {
        return None;
    }
    Some(rest.split("__").next().unwrap_or(rest))
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
