mod active;
mod cache_miss;
mod slots;
mod tally;

#[cfg(test)]
mod reference;
#[cfg(test)]
mod tests;

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::mem::size_of;

use serde::{Deserialize, Serialize};

use active::ActiveSegments;
use cache_miss::{CacheInput, CachePatch, CacheReducer};
use slots::{CompactionMark, ProgressSlots, ReorderWindow, SlotAggregate, SlotAxis, StampedName};
use tally::{
    IdentityKey, Interner, LateToolCandidate, MAX_BUILTIN_LATE_CANDIDATES, MAX_LATE_CANDIDATES,
    MAX_MCP_SERVERS, MAX_MODEL_RUNS, MAX_MODELS, MAX_SKILL_NAMES, MAX_SKILL_USES, MAX_SPEEDS,
    MAX_THINKING_MODES, MAX_TOOL_NAMES, NameId, SkillMark, SkillNameInterner, add_model_tokens,
    add_usage,
};

use crate::analysis::efficiency::{EfficiencyInput, EfficiencyReducer};
use crate::analysis::engine::{
    BUCKETS, Bucket, CONTEXT_WINDOW, IDLE_GAP_MS, SessionMetrics, SkillUse,
};
use crate::analysis::initial_context::{
    InitialContextBreakdown, InitialContextSourceCount, InitialContextTokenSource, SourceOrigin,
};
use crate::analysis::interface::{NormalizedRecord, RecordSink, SessionSummary};
use crate::analysis::model::{EventSource, ModelRun, NormalizedEvent, Role, Usage};
use crate::pricing::ModelTokens;

const CONTEXT_WINDOW_TIERS: [u64; 2] = [200_000, 1_000_000];
/// Tests enforce this ceiling for reducer-owned derived state.
/// Exact caller-provided identity strings are additional.
pub const RETAINED_METRICS_BYTES_BOUND: usize = 640 * 1_024;
/// Descriptions match the desktop excerpt limit.
const MAX_DESCRIPTION_CHARS: usize = 300;
/// Initial-context rows retain the largest named rows and overflow totals.
const MAX_INITIAL_CONTEXT_SOURCES: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct MetricsIdentity {
    pub(crate) agent: String,
    pub(crate) session_id: String,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
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
    fn observe(&mut self, event: &NormalizedEvent) {
        self.tokens_in = self
            .tokens_in
            .saturating_add(event.usage.effective_input_tokens());
        self.tokens_out = self.tokens_out.saturating_add(event.usage.output_tokens);
        self.billable_input_tokens = self
            .billable_input_tokens
            .saturating_add(event.usage.input_tokens);
        self.billable_output_tokens = self
            .billable_output_tokens
            .saturating_add(event.usage.output_tokens);
        self.billable_cache_read_tokens = self
            .billable_cache_read_tokens
            .saturating_add(event.usage.cache_read_tokens);
        self.billable_cache_creation_tokens = self
            .billable_cache_creation_tokens
            .saturating_add(event.usage.cache_creation_tokens);
        if event.source == EventSource::Parent {
            self.peak_context_tokens = self.peak_context_tokens.max(event.usage.context_tokens());
            self.compaction_count = self
                .compaction_count
                .saturating_add(u64::from(event.is_compaction_boundary));
        }
        self.event_count = self.event_count.saturating_add(1);
    }
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
struct StoredSummary {
    context_window: Option<u64>,
    model: Option<String>,
    started_at_ms: Option<i64>,
    initial_context: Option<InitialContextBreakdown>,
    skill_descriptions: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct ModelRunMark {
    effective_ts: i64,
    ordinal: u64,
    model: Option<NameId>,
    thinking_mode: Option<NameId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMetricsAccumulator {
    identity: MetricsIdentity,
    slots: ProgressSlots,
    reorder: ReorderWindow,
    active: ActiveSegments,
    interner: Interner,
    mcp_interner: Interner,
    model_interner: Interner,
    bucket_model_interner: Interner,
    thinking_interner: Interner,
    speed_interner: Interner,
    last_tool_interner: Interner,
    skill_names: SkillNameInterner,
    tallies: OnlineTallies,
    summary: Option<StoredSummary>,
    efficiency: EfficiencyReducer,
    cache: CacheReducer,
    skill_marks: Vec<SkillMark>,
    skill_duration_heap: BinaryHeap<Reverse<(i64, usize)>>,
    late_candidates: Vec<LateToolCandidate>,
    late_duration_heap: BinaryHeap<Reverse<(i64, usize)>>,
    tool_calls_by_name: Vec<(NameId, u32)>,
    tool_match_counts: Vec<(String, u32)>,
    mcp_tool_calls: Vec<(NameId, u32)>,
    skill_match_counts: Vec<(String, u32)>,
    model_breakdown: Vec<(NameId, ModelTokens)>,
    unattributed_model_tokens: ModelTokens,
    model_runs: Vec<ModelRunMark>,
    last_effective_ts: i64,
    folded_last_ts: Option<i64>,
    folded_active_position: u64,
    active_model: Option<IdentityKey>,
    observed_turns: u64,
    tool_names_truncated: u64,
    mcp_servers_truncated: u64,
    models_truncated: u64,
    model_runs_truncated: u64,
    skill_uses_truncated: u64,
    late_candidates_truncated: u64,
}

impl SessionMetricsAccumulator {
    pub fn new(agent: impl Into<String>, session_id: impl Into<String>) -> Self {
        let agent = agent.into();
        Self {
            identity: MetricsIdentity {
                agent: agent.clone(),
                session_id: session_id.into(),
            },
            slots: ProgressSlots::default(),
            reorder: ReorderWindow::default(),
            active: ActiveSegments::default(),
            interner: Interner::with_limit(MAX_TOOL_NAMES),
            mcp_interner: Interner::with_limit(MAX_MCP_SERVERS),
            model_interner: Interner::with_limit(MAX_MODELS),
            bucket_model_interner: Interner::with_limit(MAX_MODELS),
            thinking_interner: Interner::with_limit(MAX_THINKING_MODES),
            speed_interner: Interner::with_limit(MAX_SPEEDS),
            last_tool_interner: Interner::with_limit(MAX_TOOL_NAMES),
            skill_names: SkillNameInterner::default(),
            tallies: OnlineTallies::default(),
            summary: None,
            efficiency: EfficiencyReducer::default(),
            cache: CacheReducer::new(&agent),
            skill_marks: Vec::new(),
            skill_duration_heap: BinaryHeap::new(),
            late_candidates: Vec::new(),
            late_duration_heap: BinaryHeap::new(),
            tool_calls_by_name: Vec::new(),
            tool_match_counts: Vec::new(),
            mcp_tool_calls: Vec::new(),
            skill_match_counts: Vec::new(),
            model_breakdown: Vec::new(),
            unattributed_model_tokens: ModelTokens::default(),
            model_runs: Vec::new(),
            last_effective_ts: i64::MIN,
            folded_last_ts: None,
            folded_active_position: 0,
            active_model: None,
            observed_turns: 0,
            tool_names_truncated: 0,
            mcp_servers_truncated: 0,
            models_truncated: 0,
            model_runs_truncated: 0,
            skill_uses_truncated: 0,
            late_candidates_truncated: 0,
        }
    }

    pub fn metrics(&self) -> SessionMetrics {
        let mut axis = self.active.clone();
        axis.rebuild_prefix();
        self.project(&axis)
    }

    /// A restorable snapshot of this accumulator's full internal state:
    /// every reducer, interner, and pending window, not just the projected
    /// [`SessionMetrics`]. Serializing this and feeding the result to
    /// [`Self::restore`] reproduces this exact accumulator.
    pub fn snapshot(&self) -> Self {
        self.clone()
    }

    /// Restores an accumulator [`Self::snapshot`] produced. Rebuilds the
    /// index caches serialization skips (see [`tally::Interner`],
    /// [`active::ActiveSegments`]) so the result keeps observing records as
    /// if it never stopped.
    pub fn restore(snapshot: Self) -> Self {
        let mut accumulator = snapshot;
        accumulator.rebuild_caches();
        accumulator
    }

    /// Rebuilds every index a snapshot restore leaves empty or invalid.
    fn rebuild_caches(&mut self) {
        self.interner.rebuild_index();
        self.mcp_interner.rebuild_index();
        self.model_interner.rebuild_index();
        self.bucket_model_interner.rebuild_index();
        self.thinking_interner.rebuild_index();
        self.speed_interner.rebuild_index();
        self.last_tool_interner.rebuild_index();
        self.skill_names.rebuild_index();
        self.active.rebuild_prefix();
    }

    pub fn earliest_ts_ms(&self) -> Option<i64> {
        self.active.earliest_ts_ms()
    }

    pub fn started_at_ms(&self) -> Option<i64> {
        self.summary
            .as_ref()
            .and_then(|summary| summary.started_at_ms)
            .or_else(|| self.earliest_ts_ms())
    }

    pub fn observed_turns(&self) -> usize {
        usize::try_from(self.observed_turns).unwrap_or(usize::MAX)
    }

    pub fn retained_bytes(&self) -> usize {
        self.slots
            .retained_bytes()
            .saturating_add(self.reorder.retained_bytes())
            .saturating_add(self.active.retained_bytes())
            .saturating_add(self.interner.retained_bytes())
            .saturating_add(self.mcp_interner.retained_bytes())
            .saturating_add(self.model_interner.retained_bytes())
            .saturating_add(self.bucket_model_interner.retained_bytes())
            .saturating_add(self.thinking_interner.retained_bytes())
            .saturating_add(self.speed_interner.retained_bytes())
            .saturating_add(self.last_tool_interner.retained_bytes())
            .saturating_add(self.skill_names.retained_bytes())
            .saturating_add(self.efficiency.retained_bytes())
            .saturating_add(self.cache.retained_bytes())
            .saturating_add(
                self.skill_marks
                    .capacity()
                    .saturating_mul(size_of::<SkillMark>()),
            )
            .saturating_add(
                self.skill_duration_heap
                    .capacity()
                    .saturating_mul(size_of::<Reverse<(i64, usize)>>()),
            )
            .saturating_add(
                self.late_candidates
                    .capacity()
                    .saturating_mul(size_of::<LateToolCandidate>()),
            )
            .saturating_add(
                self.late_duration_heap
                    .capacity()
                    .saturating_mul(size_of::<Reverse<(i64, usize)>>()),
            )
            .saturating_add(
                self.tool_calls_by_name
                    .capacity()
                    .saturating_mul(size_of::<(NameId, u32)>()),
            )
            .saturating_add(string_counts_retained_bytes(
                &self.tool_match_counts,
                self.tool_match_counts.capacity(),
            ))
            .saturating_add(
                self.mcp_tool_calls
                    .capacity()
                    .saturating_mul(size_of::<(NameId, u32)>()),
            )
            .saturating_add(string_counts_retained_bytes(
                &self.skill_match_counts,
                self.skill_match_counts.capacity(),
            ))
            .saturating_add(
                self.model_breakdown
                    .capacity()
                    .saturating_mul(size_of::<(NameId, ModelTokens)>()),
            )
            .saturating_add(
                self.model_runs
                    .capacity()
                    .saturating_mul(size_of::<ModelRunMark>()),
            )
            .saturating_add(self.summary_retained_bytes())
    }

    fn summary_retained_bytes(&self) -> usize {
        self.summary.as_ref().map_or(0, |summary| {
            summary.model.as_ref().map_or(0, String::capacity)
                + summary
                    .skill_descriptions
                    .capacity()
                    .saturating_mul(size_of::<(String, String)>())
                + summary
                    .skill_descriptions
                    .iter()
                    .map(|(name, description)| name.capacity() + description.capacity())
                    .sum::<usize>()
                + summary
                    .initial_context
                    .as_ref()
                    .map_or(0, initial_context_retained_bytes)
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

    fn observe_event(&mut self, event: NormalizedEvent) {
        let ordinal = self.observed_turns;
        self.observed_turns = self.observed_turns.saturating_add(1);
        self.tallies.observe(&event);
        if let Some(timestamp) = event.ts_ms {
            self.active.observe(timestamp);
            self.last_effective_ts = timestamp;
        }
        let effective_ts = event.ts_ms.unwrap_or(self.last_effective_ts);
        self.efficiency.observe(EfficiencyInput {
            ordinal,
            ts_ms: event.ts_ms,
            role: event.role,
            source: event.source,
            message_id: event.message_id.as_deref(),
            model: event.model.as_deref(),
            usage: event.usage,
        });
        let mut slot = SlotAggregate::new(ordinal, effective_ts);
        slot.timestamp = event.ts_ms;
        let model = event
            .model
            .as_deref()
            .filter(|value| !value.is_empty())
            .and_then(|value| self.bucket_model_interner.intern(value));

        if event.source == EventSource::Subagent {
            slot.subagent_tokens = event
                .usage
                .effective_input_tokens()
                .saturating_add(event.usage.output_tokens);
        } else {
            if let Some(parent_model) = event.model.as_deref().filter(|value| !value.is_empty()) {
                self.active_model = Some(IdentityKey::new(parent_model));
            }
            self.observe_parent_fields(&event, ordinal, model, &mut slot);
        }
        self.observe_tools(&event, ordinal, effective_ts);
        self.observe_model_usage(&event, effective_ts, ordinal);

        if event.source == EventSource::Parent {
            if event.role == Role::User {
                self.cache.observe_user_prompt(event.ts_ms);
            }
            if event.is_compaction_boundary {
                self.cache.mark_compaction();
            }
            let context_tokens = event.usage.context_tokens();
            if context_tokens > 0 {
                let (mode_1, mode_2, gap) = self.cache.observe(CacheInput {
                    key: (effective_ts, ordinal),
                    timestamp: event.ts_ms,
                    context_tokens,
                    cache_read_tokens: event.usage.cache_read_tokens,
                    cache_write_tokens: event.usage.cache_creation_tokens,
                    model: self.active_model,
                });
                slot.cache_mode_1 = mode_1;
                slot.first_gap = gap.map(|gap| (ordinal, gap));
                if let Some(patch) = mode_2 {
                    self.apply_cache_patch(patch);
                }
            }
        }
        if event.may_resolve_late_tool || event.late_tool_candidate_is_builtin {
            self.reserve_late_candidate(&event, ordinal, effective_ts);
        }
        if let Some(ready) =
            self.reorder
                .push(slot, &self.identity.agent, &self.identity.session_id)
        {
            self.fold_ready_slot(ready);
        }
    }

    fn apply_cache_patch(&mut self, patch: CachePatch) {
        if !self.reorder.merge_cache_patch(patch.key.1, patch.slot) {
            let patched = self.slots.merge_cache_patch(patch.key, patch.slot);
            if !patched {
                tracing::debug!(event = "metrics_cache_patch_target_missing");
            }
        }
    }

    fn fold_ready_slot(&mut self, ready: SlotAggregate) {
        if let Some(timestamp) = ready.timestamp {
            resolve_skill_durations(
                timestamp,
                &mut self.skill_duration_heap,
                &mut self.skill_marks,
            );
            resolve_late_durations(
                timestamp,
                &mut self.late_duration_heap,
                &mut self.late_candidates,
            );
        }
        for mark in self
            .skill_marks
            .iter_mut()
            .filter(|mark| mark.ordinal == ready.first_ordinal)
        {
            mark.effective_ts = ready.first_ts;
        }
        if let Some(candidate) = self
            .late_candidates
            .iter_mut()
            .find(|candidate| candidate.ordinal == ready.first_ordinal)
        {
            candidate.effective_ts = ready.first_ts;
        }
        self.cache
            .update_key(ready.first_ordinal, (ready.first_ts, ready.first_ordinal));
        let active_position = self.advance_active_position(ready.first_ts);
        if self.slots.axis() == SlotAxis::Ordinal && self.active.active_ms() > 0 {
            let mut active = self.active.clone();
            active.rebuild_prefix();
            self.slots.flip_to_active(|timestamp| {
                u64::try_from(active.cumulative_ms(timestamp).max(0)).unwrap_or(u64::MAX)
            });
        }
        let position = match self.slots.axis() {
            SlotAxis::Active => active_position,
            SlotAxis::Ordinal => ready.first_ordinal,
        };
        self.slots.push(ready, position);
    }

    fn advance_active_position(&mut self, timestamp: i64) -> u64 {
        if timestamp == i64::MIN {
            return 0;
        }
        if let Some(previous) = self.folded_last_ts {
            let gap = timestamp.saturating_sub(previous).clamp(0, IDLE_GAP_MS);
            self.folded_active_position = self
                .folded_active_position
                .saturating_add(u64::try_from(gap).unwrap_or(0));
        }
        self.folded_last_ts = Some(timestamp);
        self.folded_active_position
    }

    fn observe_parent_fields(
        &mut self,
        event: &NormalizedEvent,
        ordinal: u64,
        model: Option<NameId>,
        slot: &mut SlotAggregate,
    ) {
        slot.tokens_in = event.usage.effective_input_tokens();
        slot.tokens_out = event.usage.output_tokens;
        slot.cache_read_tokens = event.usage.cache_read_tokens;
        slot.cache_write_tokens = event.usage.cache_creation_tokens;
        slot.context_tokens = event.usage.context_tokens();
        slot.user_prompts = u32::from(event.role == Role::User);
        slot.has_thinking = event.has_thinking;
        slot.model = model.map(|name| StampedName { ordinal, name });
        slot.thinking_mode = event
            .thinking_mode
            .as_deref()
            .filter(|value| !value.is_empty())
            .and_then(|value| self.thinking_interner.intern(value))
            .map(|name| StampedName { ordinal, name });
        slot.speed = event
            .speed
            .as_deref()
            .filter(|value| !value.is_empty())
            .and_then(|value| self.speed_interner.intern(value))
            .map(|name| StampedName { ordinal, name });
        slot.last_tool = event.tools.last().and_then(|tool| {
            self.last_tool_interner
                .intern(&tool.name)
                .map(|name| StampedName { ordinal, name })
        });
        slot.subagent_launches = event
            .tools
            .iter()
            .filter(|tool| tool.name.eq_ignore_ascii_case("task"))
            .count()
            .try_into()
            .unwrap_or(u32::MAX);
        if event.is_compaction_boundary {
            let mark = CompactionMark {
                effective_ts: slot.first_ts,
                ordinal,
                trigger: event.compaction_trigger,
                pre_tokens: event.compaction_pre_tokens,
                post_tokens: event.compaction_post_tokens,
            };
            slot.first_compaction_key = Some((mark.effective_ts, mark.ordinal));
            slot.compaction = Some(mark);
        }
    }

    fn observe_tools(&mut self, event: &NormalizedEvent, ordinal: u64, effective_ts: i64) {
        for (tool_index, tool) in event.tools.iter().enumerate() {
            self.count_tool(&tool.name);
            if tool.name.eq_ignore_ascii_case("skill") {
                self.push_skill_mark(
                    ordinal,
                    tool_index.try_into().unwrap_or(u16::MAX),
                    tool.detail.as_deref().unwrap_or("skill"),
                    effective_ts,
                    (event.ts_ms, None),
                    event.usage,
                );
            } else if let Some(server) = mcp_server_name(&tool.name) {
                self.count_mcp(server);
            }
        }
        if let Some(wrapper) = &event.wrapper_tool {
            self.count_tool(wrapper);
        }
    }

    fn reserve_late_candidate(&mut self, event: &NormalizedEvent, ordinal: u64, effective_ts: i64) {
        let provisional_builtin_count = self
            .late_candidates
            .iter()
            .filter(|candidate| candidate.provisional_builtin)
            .count();
        if self.late_candidates.len() >= MAX_LATE_CANDIDATES
            || (event.late_tool_candidate_is_builtin
                && provisional_builtin_count >= MAX_BUILTIN_LATE_CANDIDATES)
        {
            self.late_candidates_truncated = self.late_candidates_truncated.saturating_add(1);
            tracing::debug!(event = "metrics_late_candidates_capped");
            return;
        }
        let candidate = LateToolCandidate {
            ordinal,
            source: event.source,
            provisional_builtin: event.late_tool_candidate_is_builtin,
            usage: event.usage,
            effective_ts,
            timestamp: event.ts_ms,
            next_timestamp: None,
            next_tool_index: u16::try_from(event.tools.len()).unwrap_or(u16::MAX),
            late_subagent_launches: 0,
            late_last_tool: None,
        };
        let index = self.late_candidates.len();
        if let Some(timestamp) = candidate.timestamp {
            self.late_duration_heap.push(Reverse((timestamp, index)));
        }
        self.late_candidates.push(candidate);
    }

    fn push_skill_mark(
        &mut self,
        ordinal: u64,
        tool_index: u16,
        name: &str,
        effective_ts: i64,
        timing: (Option<i64>, Option<i64>),
        usage: Usage,
    ) {
        if self.skill_marks.len() >= MAX_SKILL_USES {
            self.skill_uses_truncated = self.skill_uses_truncated.saturating_add(1);
            tracing::debug!(event = "metrics_skill_uses_capped");
            return;
        }
        let raw_name = name;
        let Some(name) = self.skill_names.intern(name) else {
            self.skill_uses_truncated = self.skill_uses_truncated.saturating_add(1);
            tracing::debug!(event = "metrics_skill_names_capped");
            return;
        };
        let mark = SkillMark {
            ordinal,
            tool_index,
            name,
            effective_ts,
            timestamp: timing.0,
            next_timestamp: timing.1,
            tokens_out: usage.output_tokens,
            context_tokens: usage.context_tokens(),
        };
        let index = self.skill_marks.len();
        if let Some(timestamp) = mark.timestamp
            && mark.next_timestamp.is_none()
        {
            self.skill_duration_heap.push(Reverse((timestamp, index)));
        }
        increment_matching_name(
            &mut self.skill_match_counts,
            raw_name,
            MAX_SKILL_NAMES,
            tally::MAX_SKILL_NAME_BYTES,
            tally::bounded_skill_name,
        );
        self.skill_marks.push(mark);
    }

    fn observe_model_usage(&mut self, event: &NormalizedEvent, effective_ts: i64, ordinal: u64) {
        let usage = event.usage;
        let has_tokens = usage.input_tokens != 0
            || usage.output_tokens != 0
            || usage.cache_read_tokens != 0
            || usage.cache_creation_tokens != 0;
        if !has_tokens {
            return;
        }
        let model = match event.model.as_deref() {
            Some(value) => {
                let value = crate::analysis::pricing::strip_window_tag(value).trim();
                if value.is_empty() {
                    return;
                }
                let model = self.model_interner.intern(value);
                if model.is_none() {
                    self.models_truncated = self.models_truncated.saturating_add(1);
                    tracing::debug!(event = "metrics_models_capped");
                }
                model
            }
            None => None,
        };
        let thinking_mode = event
            .thinking_mode
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| self.thinking_interner.intern(value));
        let run = ModelRunMark {
            effective_ts,
            ordinal,
            model,
            thinking_mode,
        };
        if !self
            .model_runs
            .iter()
            .any(|current| current.model == run.model && current.thinking_mode == run.thinking_mode)
        {
            if self.model_runs.len() < MAX_MODEL_RUNS {
                self.model_runs.push(run);
            } else {
                self.model_runs_truncated = self.model_runs_truncated.saturating_add(1);
                tracing::debug!(event = "metrics_model_runs_capped");
            }
        }
        let Some(model) = model else {
            add_usage(&mut self.unattributed_model_tokens, usage);
            return;
        };
        if let Some((_, tokens)) = self
            .model_breakdown
            .iter_mut()
            .find(|(current, _)| *current == model)
        {
            add_usage(tokens, usage);
        } else if self.model_breakdown.len() < MAX_MODELS {
            let mut tokens = ModelTokens::default();
            add_usage(&mut tokens, usage);
            self.model_breakdown.push((model, tokens));
        } else {
            add_usage(&mut self.unattributed_model_tokens, usage);
            self.models_truncated = self.models_truncated.saturating_add(1);
            tracing::debug!(event = "metrics_model_breakdown_capped");
        }
    }

    fn count_tool(&mut self, name: &str) {
        let raw_name = name;
        let Some(name) = self.interner.intern(name) else {
            self.tool_names_truncated = self.tool_names_truncated.saturating_add(1);
            return;
        };
        increment_capped(
            &mut self.tool_calls_by_name,
            name,
            MAX_TOOL_NAMES,
            &mut self.tool_names_truncated,
        );
        increment_matching_name(
            &mut self.tool_match_counts,
            raw_name,
            MAX_TOOL_NAMES,
            tally::MAX_NAME_BYTES,
            tally::truncate_name,
        );
    }

    fn count_mcp(&mut self, server: &str) {
        let lower = server.to_ascii_lowercase();
        let Some(server) = self.mcp_interner.intern(&lower) else {
            self.mcp_servers_truncated = self.mcp_servers_truncated.saturating_add(1);
            return;
        };
        increment_capped(
            &mut self.mcp_tool_calls,
            server,
            MAX_MCP_SERVERS,
            &mut self.mcp_servers_truncated,
        );
    }

    fn finish_summary(&mut self, mut summary: SessionSummary) {
        self.active.rebuild_prefix();
        self.efficiency.flush();
        let fallback = summary.model.as_deref().map(IdentityKey::new);
        for patch in self.cache.resolve_deferred(fallback) {
            self.apply_cache_patch(patch);
        }
        for slot in self
            .reorder
            .drain_sorted(&self.identity.agent, &self.identity.session_id)
        {
            self.fold_ready_slot(slot);
        }
        for (ordinal, tool) in summary.late_tools.drain(..) {
            if ordinal >= self.observed_turns() {
                continue;
            }
            self.count_tool(&tool.name);
            if let Some(server) = mcp_server_name(&tool.name) {
                self.count_mcp(server);
            }
            let Some(index) = self
                .late_candidates
                .iter()
                .position(|candidate| candidate.ordinal == ordinal as u64)
            else {
                self.late_candidates_truncated = self.late_candidates_truncated.saturating_add(1);
                continue;
            };
            let candidate = self.late_candidates[index].clone();
            let tool_name = self.last_tool_interner.intern(&tool.name);
            if candidate.source == EventSource::Parent {
                if tool.name.eq_ignore_ascii_case("task") {
                    self.late_candidates[index].late_subagent_launches = self.late_candidates
                        [index]
                        .late_subagent_launches
                        .saturating_add(1);
                }
                if let Some(name) = tool_name {
                    self.late_candidates[index].late_last_tool = Some(name);
                }
            }
            if tool.name.eq_ignore_ascii_case("skill") {
                self.push_skill_mark(
                    candidate.ordinal,
                    candidate.next_tool_index,
                    tool.detail.as_deref().unwrap_or("skill"),
                    candidate.effective_ts,
                    (candidate.timestamp, candidate.next_timestamp),
                    candidate.usage,
                );
                self.late_candidates[index].next_tool_index = self.late_candidates[index]
                    .next_tool_index
                    .saturating_add(1);
            }
        }
        let observed_skill_names = string_count_map(&self.skill_match_counts);
        let mut skill_descriptions = summary
            .skill_descriptions
            .into_iter()
            .map(|(name, description)| {
                (
                    tally::bounded_skill_name(&name),
                    bounded_excerpt(&description, MAX_DESCRIPTION_CHARS),
                )
            })
            .filter(|(name, _)| {
                observed_skill_names
                    .keys()
                    .any(|observed| observed.eq_ignore_ascii_case(name))
            })
            .collect::<Vec<_>>();
        skill_descriptions.sort_by(|left, right| {
            let left_count = case_insensitive_count(&observed_skill_names, &left.0);
            let right_count = case_insensitive_count(&observed_skill_names, &right.0);
            right_count
                .cmp(&left_count)
                .then_with(|| left.0.cmp(&right.0))
        });
        skill_descriptions.truncate(MAX_SKILL_NAMES);
        skill_descriptions.shrink_to_fit();
        let observed_mcp_names = count_map(&self.mcp_tool_calls, &self.mcp_interner);
        let observed_tool_names = string_count_map(&self.tool_match_counts);
        self.summary = Some(StoredSummary {
            context_window: summary.context_window,
            model: summary.model.map(|model| tally::truncate_name(&model)),
            started_at_ms: summary.started_at_ms,
            initial_context: summary.initial_context.map(|breakdown| {
                bound_initial_context(
                    breakdown,
                    &observed_skill_names,
                    &observed_mcp_names,
                    &observed_tool_names,
                )
            }),
            skill_descriptions,
        });
    }

    fn project(&self, axis: &ActiveSegments) -> SessionMetrics {
        let empty = StoredSummary::default();
        let summary = self.summary.as_ref().unwrap_or(&empty);
        let use_explicit_cache_writes = self.cache.has_explicit_cache_writes();
        let active_ms = axis.active_ms();
        let mut buckets = vec![Bucket::default(); BUCKETS];
        let mut bucket_state = vec![BucketState::default(); BUCKETS];
        for slot in self.slots.iter().chain(self.reorder.iter()) {
            let index = bucket_index(
                slot.first_ts,
                slot.first_ordinal,
                active_ms,
                axis,
                self.observed_turns,
            );
            fold_slot(
                &mut buckets[index],
                &mut bucket_state[index],
                slot,
                use_explicit_cache_writes,
            );
            fold_slot_compactions(
                &mut buckets,
                &mut bucket_state,
                slot,
                active_ms,
                axis,
                self.observed_turns,
            );
        }
        let mut projected_cache = self.cache.clone();
        if !use_explicit_cache_writes {
            let fallback = summary.model.as_deref().map(IdentityKey::new);
            for patch in projected_cache.resolve_deferred(fallback) {
                let index = bucket_index(
                    patch.key.0,
                    patch.key.1,
                    active_ms,
                    axis,
                    self.observed_turns,
                );
                fold_cache_slot(&mut buckets[index], &mut bucket_state[index], patch.slot);
            }
        }
        for candidate in &self.late_candidates {
            if candidate.late_subagent_launches == 0 && candidate.late_last_tool.is_none() {
                continue;
            }
            let index = bucket_index(
                candidate.effective_ts,
                candidate.ordinal,
                active_ms,
                axis,
                self.observed_turns,
            );
            buckets[index].subagent_launches = buckets[index]
                .subagent_launches
                .saturating_add(candidate.late_subagent_launches);
            if let Some(name) = candidate.late_last_tool {
                let incoming = StampedName {
                    ordinal: candidate.ordinal,
                    name,
                };
                if bucket_state[index]
                    .last_tool
                    .is_none_or(|current| incoming.ordinal >= current.ordinal)
                {
                    bucket_state[index].last_tool = Some(incoming);
                }
            }
        }
        finish_bucket_state(
            &mut buckets,
            &bucket_state,
            &self.bucket_model_interner,
            &self.thinking_interner,
            &self.speed_interner,
            &self.last_tool_interner,
        );
        for mark in self.efficiency.clone().rewrite_marks() {
            let index = bucket_index(mark.key.0, mark.key.1, active_ms, axis, self.observed_turns);
            buckets[index].rewrite_tokens =
                buckets[index].rewrite_tokens.saturating_add(mark.tokens);
        }

        let mut skill_marks = self.skill_marks.clone();
        for timestamp in self.reorder.iter().filter_map(|slot| slot.timestamp) {
            for mark in &mut skill_marks {
                mark.observe_timestamp(timestamp);
            }
        }
        skill_marks.sort_by_key(|mark| (mark.ordinal, mark.tool_index));
        let skill_uses = skill_marks
            .into_iter()
            .map(|mark| {
                let progress = progress(
                    mark.effective_ts,
                    mark.ordinal,
                    active_ms,
                    axis,
                    self.observed_turns,
                );
                let name = self.skill_names.get(mark.name).to_string();
                let description = summary
                    .skill_descriptions
                    .iter()
                    .find(|(current, _)| current == &name)
                    .map(|(_, description)| description.clone());
                SkillUse {
                    name,
                    progress,
                    description,
                    duration_ms: mark
                        .timestamp
                        .zip(mark.next_timestamp)
                        .map(|(start, next)| next.saturating_sub(start).clamp(0, IDLE_GAP_MS)),
                    tokens_out: mark.tokens_out,
                    context_tokens: mark.context_tokens,
                }
            })
            .collect::<Vec<_>>();

        let mut model_breakdown = self.model_breakdown_map(summary.model.as_deref());
        let cost = crate::analysis::pricing::price_breakdown(&model_breakdown);
        let model_runs = self.model_runs(summary.model.as_deref());
        let tool_calls_by_name = count_map(&self.tool_calls_by_name, &self.interner);
        let mcp_tool_calls = count_map(&self.mcp_tool_calls, &self.mcp_interner);
        let context_window = resolve_context_window(
            summary.context_window.unwrap_or(CONTEXT_WINDOW),
            self.tallies.peak_context_tokens,
        );
        let cache_rehydration_count = if use_explicit_cache_writes {
            self.cache.mode_1_rehydrations
        } else {
            projected_cache.mode_2_rehydrations
        };
        let cache_routing_miss_count = if use_explicit_cache_writes {
            self.cache.mode_1_routing_misses
        } else {
            projected_cache.mode_2_routing_misses
        };
        if model_breakdown.is_empty() {
            model_breakdown.shrink_to_fit();
        }

        SessionMetrics {
            agent: self.identity.agent.clone(),
            session_id: self.identity.session_id.clone(),
            duration_secs: axis.duration_secs(),
            active_secs: (active_ms / 1_000).max(0) as u64,
            event_count: self.tallies.event_count,
            tokens_in: self.tallies.tokens_in,
            tokens_out: self.tallies.tokens_out,
            peak_context_tokens: self.tallies.peak_context_tokens,
            compaction_count: self.tallies.compaction_count,
            cache_routing_miss_count,
            cache_rehydration_count,
            context_available: self.identity.agent != "claude" || summary.context_window.is_some(),
            context_window,
            buckets,
            initial_context: summary.initial_context.clone(),
            model: summary.model.clone(),
            model_runs,
            billable_input_tokens: self.tallies.billable_input_tokens,
            billable_output_tokens: self.tallies.billable_output_tokens,
            billable_cache_read_tokens: self.tallies.billable_cache_read_tokens,
            billable_cache_creation_tokens: self.tallies.billable_cache_creation_tokens,
            model_breakdown,
            cost,
            efficiency: self.efficiency.clone().finish(summary.model.as_deref()),
            skill_uses,
            mcp_tool_calls,
            tool_calls_by_name,
        }
    }

    fn model_breakdown_map(&self, fallback: Option<&str>) -> HashMap<String, ModelTokens> {
        let mut result = HashMap::new();
        for (model, tokens) in &self.model_breakdown {
            result.insert(self.model_interner.get(*model).to_string(), tokens.clone());
        }
        if has_model_tokens(&self.unattributed_model_tokens)
            && let Some(model) = fallback
                .map(crate::analysis::pricing::strip_window_tag)
                .map(str::trim)
                .filter(|model| !model.is_empty())
        {
            add_model_tokens(
                result.entry(model.to_string()).or_default(),
                &self.unattributed_model_tokens,
            );
        }
        result
    }

    fn model_runs(&self, fallback: Option<&str>) -> Vec<ModelRun> {
        let mut result = Vec::new();
        for mark in &self.model_runs {
            let Some(run) = resolved_model_run(
                mark,
                fallback,
                &self.model_interner,
                &self.thinking_interner,
            ) else {
                continue;
            };
            if !result.contains(&run) && result.len() < MAX_MODEL_RUNS {
                result.push(run);
            }
        }
        result
    }
}

impl RecordSink for SessionMetricsAccumulator {
    fn record(&mut self, record: NormalizedRecord) {
        if let NormalizedRecord::MetricsEvent(event) = record {
            self.observe_event(*event);
        }
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.finish_summary(summary);
    }
}

#[derive(Clone, Default)]
struct BucketState {
    model: Option<StampedName>,
    thinking_mode: Option<StampedName>,
    speed: Option<StampedName>,
    last_tool: Option<StampedName>,
    compaction: Option<CompactionMark>,
    first_gap: Option<(u64, u64)>,
    rehydration_gap: Option<(u64, Option<u64>)>,
    rehydration: Option<slots::CacheRehydrationMark>,
}

fn fold_slot(
    bucket: &mut Bucket,
    state: &mut BucketState,
    slot: &SlotAggregate,
    use_explicit_cache_writes: bool,
) {
    bucket.tokens_in = bucket.tokens_in.saturating_add(slot.tokens_in);
    bucket.tokens_out = bucket.tokens_out.saturating_add(slot.tokens_out);
    bucket.cache_read_tokens = bucket
        .cache_read_tokens
        .saturating_add(slot.cache_read_tokens);
    bucket.cache_write_tokens = bucket
        .cache_write_tokens
        .saturating_add(slot.cache_write_tokens);
    bucket.subagent_tokens = bucket.subagent_tokens.saturating_add(slot.subagent_tokens);
    bucket.context_tokens = bucket.context_tokens.max(slot.context_tokens);
    bucket.user_prompts = bucket.user_prompts.saturating_add(slot.user_prompts);
    bucket.subagent_launches = bucket
        .subagent_launches
        .saturating_add(slot.subagent_launches);
    bucket.has_thinking |= slot.has_thinking;
    merge_stamped(&mut state.model, slot.model);
    merge_stamped(&mut state.thinking_mode, slot.thinking_mode);
    merge_stamped(&mut state.speed, slot.speed);
    merge_stamped(&mut state.last_tool, slot.last_tool);
    if slot
        .first_gap
        .is_some_and(|value| state.first_gap.is_none_or(|current| value.0 < current.0))
    {
        state.first_gap = slot.first_gap;
    }
    let cache = if use_explicit_cache_writes {
        slot.cache_mode_1
    } else {
        slot.cache_mode_2
    };
    fold_cache_slot(bucket, state, cache);
}

fn fold_slot_compactions(
    buckets: &mut [Bucket],
    states: &mut [BucketState],
    slot: &SlotAggregate,
    active_ms: i64,
    axis: &ActiveSegments,
    observed_turns: u64,
) {
    if let Some((timestamp, ordinal)) = slot.first_compaction_key {
        let index = bucket_index(timestamp, ordinal, active_ms, axis, observed_turns);
        buckets[index].is_compaction_boundary = true;
    }
    if let Some(mark) = slot.compaction {
        let index = bucket_index(
            mark.effective_ts,
            mark.ordinal,
            active_ms,
            axis,
            observed_turns,
        );
        buckets[index].is_compaction_boundary = true;
        merge_compaction(&mut states[index].compaction, Some(mark));
    }
}

fn fold_cache_slot(bucket: &mut Bucket, state: &mut BucketState, cache: slots::CacheSlot) {
    bucket.is_cache_rehydration |= cache.is_rehydration;
    bucket.is_cache_routing_miss |= cache.is_routing_miss;
    if cache.rehydration_gap.is_some_and(|value| {
        state
            .rehydration_gap
            .is_none_or(|current| value.0 > current.0)
    }) {
        state.rehydration_gap = cache.rehydration_gap;
    }
    if cache.rehydration.is_some_and(|value| {
        state
            .rehydration
            .is_none_or(|current| value.ordinal > current.ordinal)
    }) {
        state.rehydration = cache.rehydration;
    }
}

fn finish_bucket_state(
    buckets: &mut [Bucket],
    states: &[BucketState],
    models: &Interner,
    thinking_modes: &Interner,
    speeds: &Interner,
    last_tools: &Interner,
) {
    for (bucket, state) in buckets.iter_mut().zip(states) {
        bucket.model = state.model.map(|value| models.get(value.name).to_string());
        bucket.thinking_mode = state
            .thinking_mode
            .map(|value| thinking_modes.get(value.name).to_string());
        bucket.speed = state.speed.map(|value| speeds.get(value.name).to_string());
        bucket.last_tool = state
            .last_tool
            .map(|value| last_tools.get(value.name).to_string());
        if bucket.is_compaction_boundary {
            bucket.context_tokens = 0;
        }
        if let Some(compaction) = state.compaction {
            bucket.is_compaction_boundary = true;
            bucket.compaction_trigger = compaction.trigger;
            bucket.compaction_pre_tokens = compaction.pre_tokens;
            bucket.compaction_post_tokens = compaction.post_tokens;
        }
        bucket.secs_since_prior_turn = state
            .rehydration_gap
            .map(|(_, gap)| gap)
            .unwrap_or_else(|| state.first_gap.map(|(_, gap)| gap));
        bucket.cache_rehydration = state.rehydration.map(public_cache_rehydration);
    }
}

fn public_cache_rehydration(
    mark: slots::CacheRehydrationMark,
) -> crate::analysis::engine::CacheRehydration {
    crate::analysis::engine::CacheRehydration {
        context_tokens: mark.context_tokens,
        still_cached_tokens: mark.still_cached_tokens,
        rewritten_tokens: mark.rewritten_tokens,
        growth_tokens: mark.growth_tokens,
        user_inactive_secs: mark.user_inactive_secs,
    }
}

fn merge_stamped(target: &mut Option<StampedName>, incoming: Option<StampedName>) {
    if incoming.is_some_and(|value| target.is_none_or(|current| value.ordinal > current.ordinal)) {
        *target = incoming;
    }
}

fn merge_compaction(target: &mut Option<CompactionMark>, incoming: Option<CompactionMark>) {
    if incoming.is_some_and(|value| target.is_none_or(|current| value.ordinal > current.ordinal)) {
        *target = incoming;
    }
}

fn progress(
    effective_ts: i64,
    ordinal: u64,
    active_ms: i64,
    active: &ActiveSegments,
    observed_turns: u64,
) -> f32 {
    let value = if active_ms > 0 {
        active.cumulative_ms(effective_ts) as f32 / active_ms as f32
    } else if observed_turns > 1 {
        ordinal as f32 / (observed_turns - 1) as f32
    } else {
        0.0
    };
    value.clamp(0.0, 1.0)
}

fn bucket_index(
    effective_ts: i64,
    ordinal: u64,
    active_ms: i64,
    active: &ActiveSegments,
    observed_turns: u64,
) -> usize {
    ((progress(effective_ts, ordinal, active_ms, active, observed_turns) * BUCKETS as f32) as usize)
        .min(BUCKETS - 1)
}

fn increment_capped(
    values: &mut Vec<(NameId, u32)>,
    name: NameId,
    cap: usize,
    truncated: &mut u64,
) {
    if let Some((_, count)) = values.iter_mut().find(|(current, _)| *current == name) {
        *count = count.saturating_add(1);
    } else if values.len() < cap {
        values.push((name, 1));
    } else {
        *truncated = truncated.saturating_add(1);
        tracing::debug!(event = "metrics_name_map_capped");
    }
}

fn count_map(values: &[(NameId, u32)], interner: &Interner) -> HashMap<String, u32> {
    values
        .iter()
        .map(|(name, count)| (interner.get(*name).to_string(), *count))
        .collect()
}

fn string_count_map(values: &[(String, u32)]) -> HashMap<String, u32> {
    values.iter().cloned().collect()
}

fn increment_matching_name(
    values: &mut Vec<(String, u32)>,
    name: &str,
    cap: usize,
    byte_limit: usize,
    bound: fn(&str) -> String,
) {
    if name.len() <= byte_limit {
        if let Some((_, count)) = values
            .iter_mut()
            .find(|(current, _)| current.eq_ignore_ascii_case(name))
        {
            *count = count.saturating_add(1);
        } else if values.len() < cap {
            values.push((name.to_ascii_lowercase(), 1));
        }
        return;
    }
    let bounded = bound(&name.to_ascii_lowercase());
    if let Some((_, count)) = values.iter_mut().find(|(current, _)| current == &bounded) {
        *count = count.saturating_add(1);
    } else if values.len() < cap {
        values.push((bounded, 1));
    }
}

fn string_counts_retained_bytes(values: &[(String, u32)], capacity: usize) -> usize {
    capacity
        .saturating_mul(size_of::<(String, u32)>())
        .saturating_add(
            values
                .iter()
                .map(|(name, _)| name.capacity())
                .sum::<usize>(),
        )
}

fn resolve_skill_durations(
    timestamp: i64,
    heap: &mut BinaryHeap<Reverse<(i64, usize)>>,
    marks: &mut [SkillMark],
) {
    while heap
        .peek()
        .is_some_and(|Reverse((start, _))| *start < timestamp)
    {
        let Some(Reverse((_, index))) = heap.pop() else {
            break;
        };
        if let Some(mark) = marks.get_mut(index) {
            mark.observe_timestamp(timestamp);
        }
    }
}

fn resolve_late_durations(
    timestamp: i64,
    heap: &mut BinaryHeap<Reverse<(i64, usize)>>,
    candidates: &mut [LateToolCandidate],
) {
    while heap
        .peek()
        .is_some_and(|Reverse((start, _))| *start < timestamp)
    {
        let Some(Reverse((_, index))) = heap.pop() else {
            break;
        };
        if let Some(candidate) = candidates.get_mut(index) {
            candidate.observe_timestamp(timestamp);
        }
    }
}

fn bound_initial_context(
    mut breakdown: InitialContextBreakdown,
    observed_skill_names: &HashMap<String, u32>,
    observed_mcp_names: &HashMap<String, u32>,
    observed_tool_names: &HashMap<String, u32>,
) -> InitialContextBreakdown {
    for source in &mut breakdown.sources {
        source.source = tally::truncate_name(&source.source);
        let is_skill = source.source == InitialContextTokenSource::Skill.as_str();
        let is_mcp = source.source == InitialContextTokenSource::Mcp.as_str();
        let original_name = source.source_name.take();
        let match_name = original_name.as_deref().map(|name| {
            if is_skill {
                tally::bounded_skill_name(&name.to_ascii_lowercase())
            } else {
                tally::truncate_name(&name.to_ascii_lowercase())
            }
        });
        source.source_name = original_name.map(|name| {
            if is_skill {
                tally::bounded_skill_name(&name)
            } else {
                tally::truncate_name(&name)
            }
        });
        source.use_count = if is_skill {
            match_name
                .as_deref()
                .map_or(0, |name| case_insensitive_count(observed_skill_names, name))
        } else if is_mcp {
            match_name
                .as_deref()
                .map_or(0, |name| case_insensitive_count(observed_mcp_names, name))
        } else if source.source == InitialContextTokenSource::BuiltinTool.as_str() {
            observed_tool_names
                .iter()
                .filter(|(observed, _)| {
                    match_name
                        .as_deref()
                        .is_some_and(|name| observed.as_str() == name)
                        || source.match_names.iter().any(|name| {
                            observed.as_str()
                                == tally::truncate_name(&name.to_ascii_lowercase()).as_str()
                        })
                })
                .fold(0_u32, |count, (_, value)| count.saturating_add(*value))
        } else {
            0
        };
        source.match_names.clear();
        source.match_names.shrink_to_fit();
    }
    if breakdown.sources.len() > MAX_INITIAL_CONTEXT_SOURCES {
        const OVERFLOW_ROWS: usize = 3;
        let named_limit = MAX_INITIAL_CONTEXT_SOURCES - OVERFLOW_ROWS;
        let mut retained = (0..breakdown.sources.len()).collect::<Vec<_>>();
        retained.sort_by_key(|index| Reverse(breakdown.sources[*index].token_count));
        retained.truncate(named_limit);
        retained.sort_unstable();

        let mut named = Vec::with_capacity(MAX_INITIAL_CONTEXT_SOURCES);
        let mut overflow: HashMap<String, InitialContextSourceCount> = HashMap::new();
        for (index, source) in breakdown.sources.drain(..).enumerate() {
            if retained.binary_search(&index).is_ok() {
                named.push(source);
                continue;
            }
            let row = overflow.entry(source.source.clone()).or_insert_with(|| {
                InitialContextSourceCount {
                    source: source.source.clone(),
                    source_name: Some(initial_context_overflow_name(&source.source).to_string()),
                    token_count: 0,
                    use_count: 0,
                    origin: SourceOrigin::Unknown,
                    deferred: false,
                    match_names: Vec::new(),
                }
            });
            row.token_count = row.token_count.saturating_add(source.token_count);
            row.use_count = row.use_count.saturating_add(source.use_count);
            row.deferred |= source.deferred;
        }
        let mut overflow = overflow.into_values().collect::<Vec<_>>();
        overflow.sort_by(|left, right| left.source.cmp(&right.source));
        named.extend(overflow);
        breakdown.sources = named;
    }
    breakdown.sources.shrink_to_fit();
    breakdown
}

fn initial_context_overflow_name(source: &str) -> &'static str {
    if source == InitialContextTokenSource::Skill.as_str() {
        "Other skills"
    } else if source == InitialContextTokenSource::Mcp.as_str() {
        "Other MCP servers"
    } else {
        "Other built-in tools"
    }
}

fn case_insensitive_count(values: &HashMap<String, u32>, name: &str) -> u32 {
    values
        .iter()
        .filter(|(current, _)| current.eq_ignore_ascii_case(name))
        .fold(0_u32, |total, (_, count)| total.saturating_add(*count))
}

fn initial_context_retained_bytes(breakdown: &InitialContextBreakdown) -> usize {
    breakdown
        .sources
        .capacity()
        .saturating_mul(size_of::<
            crate::analysis::initial_context::InitialContextSourceCount,
        >())
        .saturating_add(
            breakdown
                .sources
                .iter()
                .map(|source| {
                    source.source.capacity()
                        + source.source_name.as_ref().map_or(0, String::capacity)
                        + source
                            .match_names
                            .capacity()
                            .saturating_mul(size_of::<String>())
                        + source
                            .match_names
                            .iter()
                            .map(String::capacity)
                            .sum::<usize>()
                })
                .sum::<usize>(),
        )
}

fn bounded_excerpt(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_string();
    }
    let mut excerpt = value
        .chars()
        .take(maximum.saturating_sub(1))
        .collect::<String>();
    excerpt.push('…');
    excerpt
}

/// Extracts the server segment from `mcp__<server>__<tool>`.
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

fn has_model_tokens(tokens: &ModelTokens) -> bool {
    tokens.input_tokens != 0
        || tokens.output_tokens != 0
        || tokens.cache_read_tokens != 0
        || tokens.cache_creation_tokens != 0
        || tokens.cache_creation_1h_tokens != 0
}

fn resolved_model_run(
    mark: &ModelRunMark,
    fallback: Option<&str>,
    models: &Interner,
    thinking_modes: &Interner,
) -> Option<ModelRun> {
    let model = mark
        .model
        .map(|id| models.get(id))
        .or(fallback)
        .map(crate::analysis::pricing::strip_window_tag)
        .map(str::trim)
        .filter(|model| !model.is_empty())?;
    Some(ModelRun {
        model: model.to_string(),
        thinking_mode: mark
            .thinking_mode
            .map(|id| thinking_modes.get(id).trim().to_string()),
    })
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

/// Merges derived slot facts on one shared active-time axis.
///
/// The merge keeps each parent-transcript source tag. Every child input
/// contributes as a subagent stream. Efficiency is the sum of thread totals.
pub fn merge_metrics(
    parent: &SessionMetricsAccumulator,
    subagents: &[SessionMetricsAccumulator],
) -> SessionMetrics {
    let mut active = parent.active.clone();
    for subagent in subagents {
        active.merge(&subagent.active);
    }
    active.rebuild_prefix();
    let mut merged = parent.project(&active);
    merged.initial_context = None;
    let active_ms = active.active_ms();
    reproject_exact_merged_cache(parent, &active, active_ms, &mut merged);
    let parent_fallback = parent
        .summary
        .as_ref()
        .and_then(|summary| summary.model.as_deref());
    let mut merged_runs = Vec::new();
    collect_merged_runs(
        &mut merged_runs,
        parent,
        parent_fallback,
        &active,
        active_ms,
        0,
    );
    for subagent in subagents {
        for slot in subagent.slots.iter().chain(subagent.reorder.iter()) {
            let index = bucket_index(
                slot.first_ts,
                slot.first_ordinal,
                active_ms,
                &active,
                subagent.observed_turns,
            );
            let tokens = slot
                .tokens_in
                .saturating_add(slot.tokens_out)
                .saturating_add(slot.subagent_tokens);
            merged.buckets[index].subagent_tokens =
                merged.buckets[index].subagent_tokens.saturating_add(tokens);
        }
    }
    merged.duration_secs = active.duration_secs();
    merged.active_secs = (active_ms / 1_000).max(0) as u64;
    for (stream_index, subagent) in subagents.iter().enumerate() {
        let subagent_metrics = subagent.metrics();
        collect_merged_runs(
            &mut merged_runs,
            subagent,
            parent_fallback,
            &active,
            active_ms,
            stream_index.saturating_add(1),
        );
        merged.event_count = merged
            .event_count
            .saturating_add(subagent.tallies.event_count);
        merged.tokens_in = merged.tokens_in.saturating_add(subagent.tallies.tokens_in);
        merged.tokens_out = merged
            .tokens_out
            .saturating_add(subagent.tallies.tokens_out);
        merged.billable_input_tokens = merged
            .billable_input_tokens
            .saturating_add(subagent.tallies.billable_input_tokens);
        merged.billable_output_tokens = merged
            .billable_output_tokens
            .saturating_add(subagent.tallies.billable_output_tokens);
        merged.billable_cache_read_tokens = merged
            .billable_cache_read_tokens
            .saturating_add(subagent.tallies.billable_cache_read_tokens);
        merged.billable_cache_creation_tokens = merged
            .billable_cache_creation_tokens
            .saturating_add(subagent.tallies.billable_cache_creation_tokens);
        merged.efficiency.add(subagent_metrics.efficiency);
        merge_count_map(
            &mut merged.tool_calls_by_name,
            subagent_metrics.tool_calls_by_name,
            MAX_TOOL_NAMES,
        );
        merge_count_map(
            &mut merged.mcp_tool_calls,
            subagent_metrics.mcp_tool_calls,
            MAX_MCP_SERVERS,
        );
        for mark in &subagent.skill_marks {
            if merged.skill_uses.len() >= MAX_SKILL_USES {
                break;
            }
            let name = subagent.skill_names.get(mark.name).to_string();
            let description = subagent.summary.as_ref().and_then(|summary| {
                summary
                    .skill_descriptions
                    .iter()
                    .find(|(current, _)| current == &name)
                    .map(|(_, description)| description.clone())
            });
            merged.skill_uses.push(SkillUse {
                name,
                progress: progress(
                    mark.effective_ts,
                    mark.ordinal,
                    active_ms,
                    &active,
                    subagent.observed_turns,
                ),
                description,
                duration_ms: mark
                    .timestamp
                    .zip(mark.next_timestamp)
                    .map(|(start, next)| next.saturating_sub(start).clamp(0, IDLE_GAP_MS)),
                tokens_out: mark.tokens_out,
                context_tokens: mark.context_tokens,
            });
        }
        for (model, tokens) in subagent.model_breakdown_map(parent_fallback) {
            if merged.model_breakdown.len() < MAX_MODELS
                || merged.model_breakdown.contains_key(&model)
            {
                add_model_tokens(merged.model_breakdown.entry(model).or_default(), &tokens);
            } else if let Some(fallback) = merged
                .model
                .as_deref()
                .map(crate::analysis::pricing::strip_window_tag)
                .map(str::trim)
                .filter(|model| !model.is_empty())
            {
                add_model_tokens(
                    merged
                        .model_breakdown
                        .entry(fallback.to_string())
                        .or_default(),
                    &tokens,
                );
            }
        }
    }
    if !subagents.is_empty() {
        merged_runs.sort_by_key(|(position, stream_index, ordinal, _)| {
            (*position, *stream_index, *ordinal)
        });
        merged.model_runs.clear();
        for (_, _, _, run) in merged_runs {
            if merged.model_runs.len() >= MAX_MODEL_RUNS {
                break;
            }
            if !merged.model_runs.contains(&run) {
                merged.model_runs.push(run);
            }
        }
    }
    merged
        .skill_uses
        .sort_by(|left, right| left.progress.total_cmp(&right.progress));
    merged.cost = crate::analysis::pricing::price_breakdown(&merged.model_breakdown);
    merged
}

fn reproject_exact_merged_cache(
    parent: &SessionMetricsAccumulator,
    active: &ActiveSegments,
    active_ms: i64,
    metrics: &mut SessionMetrics,
) {
    let mut slots = parent
        .slots
        .iter()
        .chain(parent.reorder.iter())
        .collect::<Vec<_>>();
    if slots
        .iter()
        .any(|slot| slot.first_ordinal != slot.last_ordinal)
    {
        return;
    }
    let use_explicit_cache_writes = slots.iter().any(|slot| slot.cache_write_tokens > 0);
    for bucket in &mut metrics.buckets {
        bucket.is_cache_rehydration = false;
        bucket.cache_rehydration = None;
        bucket.is_cache_routing_miss = false;
        bucket.secs_since_prior_turn = None;
    }
    let mut first_gaps = vec![None::<(u64, u64)>; BUCKETS];
    let mut rehydration_gaps = vec![None::<(u64, Option<u64>)>; BUCKETS];
    slots.sort_by_key(|slot| slot.first_key);
    let empty_summary = StoredSummary::default();
    let summary = parent.summary.as_ref().unwrap_or(&empty_summary);
    let fallback = summary.model.as_deref().map(IdentityKey::new);
    let mut active_model = fallback;
    let mut reducer = CacheReducer::new(&parent.identity.agent);
    let mut mode_2_patches = Vec::new();

    for slot in slots {
        if slot.user_prompts > 0 {
            reducer.observe_user_prompt(slot.timestamp);
        }
        if let Some(model) = slot.model {
            let value = parent.bucket_model_interner.get(model.name);
            if !value.is_empty() {
                active_model = Some(IdentityKey::new(value));
            }
        }
        if slot.first_compaction_key.is_some() {
            reducer.mark_compaction();
        }
        if slot.context_tokens == 0 {
            continue;
        }
        let key = (slot.first_ts, slot.first_ordinal);
        let (mode_1, mode_2, gap) = reducer.observe(CacheInput {
            key,
            timestamp: slot.timestamp,
            context_tokens: slot.context_tokens,
            cache_read_tokens: slot.cache_read_tokens,
            cache_write_tokens: slot.cache_write_tokens,
            model: active_model,
        });
        let index = bucket_index(key.0, key.1, active_ms, active, parent.observed_turns);
        if let Some(gap) = gap
            && first_gaps[index].is_none_or(|current| key.1 < current.0)
        {
            first_gaps[index] = Some((key.1, gap));
        }
        if use_explicit_cache_writes {
            apply_projected_cache_slot(
                &mut metrics.buckets[index],
                &mut rehydration_gaps[index],
                mode_1,
            );
        }
        if let Some(patch) = mode_2 {
            mode_2_patches.push(patch);
        }
    }
    mode_2_patches.extend(reducer.resolve_deferred(fallback));
    if !use_explicit_cache_writes {
        for patch in mode_2_patches {
            let index = bucket_index(
                patch.key.0,
                patch.key.1,
                active_ms,
                active,
                parent.observed_turns,
            );
            apply_projected_cache_slot(
                &mut metrics.buckets[index],
                &mut rehydration_gaps[index],
                patch.slot,
            );
        }
    }
    for (index, bucket) in metrics.buckets.iter_mut().enumerate() {
        bucket.secs_since_prior_turn = rehydration_gaps[index]
            .map(|(_, gap)| gap)
            .unwrap_or_else(|| first_gaps[index].map(|(_, gap)| gap));
    }
    metrics.cache_rehydration_count = if use_explicit_cache_writes {
        reducer.mode_1_rehydrations
    } else {
        reducer.mode_2_rehydrations
    };
    metrics.cache_routing_miss_count = if use_explicit_cache_writes {
        reducer.mode_1_routing_misses
    } else {
        reducer.mode_2_routing_misses
    };
}

fn apply_projected_cache_slot(
    bucket: &mut Bucket,
    rehydration_gap: &mut Option<(u64, Option<u64>)>,
    cache: slots::CacheSlot,
) {
    bucket.is_cache_rehydration |= cache.is_rehydration;
    bucket.is_cache_routing_miss |= cache.is_routing_miss;
    let replaces_rehydration = cache
        .rehydration_gap
        .is_some_and(|incoming| rehydration_gap.is_none_or(|current| incoming.0 > current.0));
    if replaces_rehydration {
        *rehydration_gap = cache.rehydration_gap;
        bucket.cache_rehydration = cache.rehydration.map(public_cache_rehydration);
    }
}

fn collect_merged_runs(
    output: &mut Vec<(i64, usize, u64, ModelRun)>,
    accumulator: &SessionMetricsAccumulator,
    fallback: Option<&str>,
    active: &ActiveSegments,
    active_ms: i64,
    stream_index: usize,
) {
    for mark in &accumulator.model_runs {
        let Some(run) = resolved_model_run(
            mark,
            fallback,
            &accumulator.model_interner,
            &accumulator.thinking_interner,
        ) else {
            continue;
        };
        let position = if active_ms > 0 {
            active.cumulative_ms(mark.effective_ts)
        } else {
            i64::try_from(mark.ordinal).unwrap_or(i64::MAX)
        };
        output.push((position, stream_index, mark.ordinal, run));
    }
}

fn merge_count_map(target: &mut HashMap<String, u32>, incoming: HashMap<String, u32>, cap: usize) {
    for (name, count) in incoming {
        if let Some(current) = target.get_mut(&name) {
            *current = current.saturating_add(count);
        } else if target.len() < cap {
            target.insert(name, count);
        }
    }
}
