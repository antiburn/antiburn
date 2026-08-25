// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The analysis engine. Completely vendor-agnostic: it only ever consumes
//! [`NormalizedEvent`]s, so adding a new vendor never touches this file.
//!
//! It resamples observed token and context data onto a shared progress grid and
//! derives the usage, tool, context, and cost metrics the UI renders.

use std::collections::HashMap;

use crate::pricing::ModelTokens;
// Re-exported through `engine` (and in turn the `analysis` module) so consumers
// reach it as `analysis::SkillUse`. The type itself lives in `crate::model::skill`
// alongside the other local-only value types, so the enrichment layer that
// flattens it stays in lockstep with what the engine emits.
pub use crate::model::skill::SkillUse;
use serde::{Deserialize, Serialize};

use crate::analysis::efficiency::EfficiencyTotals;
use crate::analysis::initial_context::InitialContextBreakdown;
use crate::analysis::model::{CompactionTrigger, ModelRun, NormalizedSession, ToolCategory};

/// Number of progress buckets each session is resampled onto (0% → 100%).
pub const BUCKETS: usize = 180;
/// Reference context window used to normalize context occupancy.
pub const CONTEXT_WINDOW: u64 = 200_000;
/// Inter-event gaps at or above this are treated as "away" time and excluded
/// from active time; shorter gaps count as active work. Tunable.
pub const IDLE_GAP_MS: i64 = 5 * 60 * 1000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolMix {
    pub edit: u64,
    pub read: u64,
    pub search: u64,
    pub test: u64,
    pub bash: u64,
    pub other: u64,
}

impl ToolMix {
    pub(crate) fn add(&mut self, category: ToolCategory) {
        match category {
            ToolCategory::Edit => self.edit = self.edit.saturating_add(1),
            ToolCategory::Read => self.read = self.read.saturating_add(1),
            ToolCategory::Search => self.search = self.search.saturating_add(1),
            ToolCategory::Test => self.test = self.test.saturating_add(1),
            ToolCategory::Bash => self.bash = self.bash.saturating_add(1),
            ToolCategory::Other => self.other = self.other.saturating_add(1),
        }
    }

    fn merge(&mut self, other: ToolMix) {
        self.edit = self.edit.saturating_add(other.edit);
        self.read = self.read.saturating_add(other.read);
        self.search = self.search.saturating_add(other.search);
        self.test = self.test.saturating_add(other.test);
        self.bash = self.bash.saturating_add(other.bash);
        self.other = self.other.saturating_add(other.other);
    }
}

/// One point on the shared 0→100% progress grid.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    /// The bucket records parent effective input (fresh + cache-written) and
    /// generated output throughput. Cache reads are excluded. `context_tokens`
    /// records the cache-read-inclusive parent occupancy.
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// The bucket combines effective input and generated output from
    /// sub-agents. A sub-agent has its own context window, so this value stays
    /// separate from the parent token series and `context_tokens`.
    pub subagent_tokens: u64,
    pub context_tokens: u64,
    /// True when a real compaction boundary lands in this bucket.
    pub is_compaction_boundary: bool,
    /// The bucket sums parent cache-read tokens. This cost-only value is
    /// already part of `context_tokens` and is never part of `tokens_in`.
    pub cache_read_tokens: u64,
    /// The bucket sums parent cache-write tokens.
    /// This is a breakdown of `tokens_in`, not an addition to it — `tokens_in`
    /// already includes cache writes as effective input.
    pub cache_write_tokens: u64,
    /// True when a turn landing in this bucket is a detected cache
    /// rehydration (see `cache_rehydration_event_indices`).
    pub is_cache_rehydration: bool,
    /// Wall-clock seconds since the prior parent turn. A rehydration turn takes
    /// priority when multiple turns land in this bucket.
    #[serde(default)]
    pub secs_since_prior_turn: Option<u64>,
    /// Count of `Task` tool calls in this bucket: how many sub-agents the
    /// parent session launched at this point. Parent turns only — a
    /// sub-agent does not itself launch sub-agents in this count.
    pub subagent_launches: u32,
    /// Count of user prompts in this bucket. A gap that ends at a prompt is
    /// the user away, not a tool that runs.
    #[serde(default)]
    pub user_prompts: u32,
    /// The name of the last parent tool call in this bucket, when any. The
    /// tooltip names it for the slices that follow, while the tool runs and
    /// no model call lands.
    #[serde(default)]
    pub last_tool: Option<String>,
    /// The model that produced the last parent event in this bucket. Parent
    /// turns only — a sub-agent runs its own model, which says nothing about
    /// the parent session's mode at this point.
    pub model: Option<String>,
    /// The thinking mode of the last parent event in this bucket. Parent
    /// turns only, for the same reason as `model`.
    pub thinking_mode: Option<String>,
    /// The response speed of the last parent event in this bucket. Parent
    /// turns only, for the same reason as `model`.
    pub speed: Option<String>,
    /// True when any parent event in this bucket carries a `thinking` block
    /// (or its vendor equivalent). Parent turns only.
    pub has_thinking: bool,
    /// Whether the compaction in this bucket was manual or automatic, when
    /// known. Parent turns only, same reason as `model`. When two
    /// compactions land in one bucket, this keeps the last one's trigger.
    pub compaction_trigger: Option<CompactionTrigger>,
    /// The context token count right before the compaction in this bucket,
    /// when known. Parent turns only. Keeps the last compaction's value.
    pub compaction_pre_tokens: Option<u64>,
    /// The context token count right after the compaction in this bucket,
    /// when known. Parent turns only. Keeps the last compaction's value.
    pub compaction_post_tokens: Option<u64>,
}

/// Estimated USD cost of a session, split by billable component. An on-device
/// approximation from the vendored pricing table (`crate::analysis::pricing`); the UI always
/// renders it as `~$`. `None` on `SessionMetrics` when the model is unknown/unpriceable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCost {
    pub total_usd: f64,
    pub input_usd: f64,
    pub output_usd: f64,
    pub cache_read_usd: f64,
    pub cache_write_usd: f64,
}

/// Metrics for a single session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetrics {
    pub agent: String,
    pub session_id: String,
    pub duration_secs: u64,
    /// Wall-clock span minus idle gaps (≥ `IDLE_GAP_MS`); the time the session
    /// was genuinely active. Always ≤ `duration_secs`.
    pub active_secs: u64,
    pub event_count: usize,
    /// Effective input tokens (fresh input + prompt-cache writes) — what the Tokens
    /// header/chart show as "in", matching the web app's "Input tokens". Cache reads
    /// are excluded and instead surface as occupancy in `peak_context_tokens`.
    pub tokens_in: u64,
    /// Generated output tokens ("out").
    pub tokens_out: u64,
    pub peak_context_tokens: u64,
    /// Compaction boundaries in the parent transcript.
    #[serde(default)]
    pub compaction_count: u64,
    /// Turns the engine flags as a cache rehydration.
    #[serde(default)]
    pub cache_rehydration_count: u64,
    /// Whether the model context window is known well enough to present
    /// occupancy. Unknown Claude model ids deliberately leave this unavailable.
    pub context_available: bool,
    /// The model's context-window size used to normalize occupancy for this
    /// session (Codex's reported window, else the reference `CONTEXT_WINDOW`).
    /// Aggregation rescales each session's occupancy into the shared reference.
    pub context_window: u64,
    pub tool_mix: ToolMix,
    pub grep_count: u64,
    pub buckets: Vec<Bucket>,
    /// Where this session's *initial* context window went, by source. `None`
    /// when the agent/session has no reliable signal ("unavailable"). Populated
    /// in `analyze_sources` (it needs the raw payload, not the normalized stream).
    #[serde(default)]
    pub initial_context: Option<InitialContextBreakdown>,
    /// The model id used for this session (most expensive priceable one seen when
    /// mixed). `None` when no adapter could extract it.
    #[serde(default)]
    pub model: Option<String>,
    /// Distinct model and thinking-mode pairs that produced billable tokens.
    #[serde(default)]
    pub model_runs: Vec<ModelRun>,
    /// Billable token components used for the cost estimate: fresh input, generated
    /// output, cache reads, and cache writes — each prices at a different rate.
    /// `tokens_out` mirrors generated output; `tokens_in` is displayed effective input
    /// (fresh + cache writes), while `billable_input_tokens` and
    /// `billable_cache_creation_tokens` keep that split for per-rate pricing. Cache
    /// reads are cost-only and never folded into displayed counts.
    #[serde(default)]
    pub billable_input_tokens: u64,
    #[serde(default)]
    pub billable_output_tokens: u64,
    #[serde(default)]
    pub billable_cache_read_tokens: u64,
    #[serde(default)]
    pub billable_cache_creation_tokens: u64,
    /// Billable token counts retained per normalized model key. This lets IPC
    /// cost results preserve mixed-model attribution instead of inventing a
    /// single-model breakdown after analysis.
    #[serde(default)]
    pub model_breakdown: HashMap<String, ModelTokens>,
    /// On-device cost estimate (`~$`), or `None` when `model` is unknown/unpriceable.
    #[serde(default)]
    pub cost: Option<SessionCost>,
    /// Where the spend went: new work, carry, or rewrite. Computed over this
    /// session's own events as one thread. For a merged parent-plus-sub-agent
    /// stream the caller replaces this with the sum of the per-thread totals,
    /// because the merge collapses the sub-agents' separate contexts.
    #[serde(default)]
    pub efficiency: EfficiencyTotals,
    /// Skill invocations detected in this session — one entry per `Skill` tool
    /// call, in event order. Each carries its active-time position, the invoking
    /// turn's token figures, and an idle-capped duration;
    /// `description` is grafted from the raw transcript later in `analyze_sources`.
    /// Empty when the session invoked no skills. Skills stay in `ToolCategory::Other`.
    #[serde(default, skip_serializing)]
    pub skill_uses: Vec<SkillUse>,
}

/// Averaged view across all analyzed sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSessionsSummary {
    pub session_count: usize,
    pub avg_duration_secs: u64,
    pub avg_active_secs: u64,
    pub tool_mix: ToolMix,
    pub grep_total: u64,
    pub tokens_in_total: u64,
    pub tokens_out_total: u64,
    pub peak_context_tokens: u64,
    /// Compactions summed over the included sessions.
    #[serde(default)]
    pub compaction_count: u64,
    /// Cache rehydrations summed over the included sessions.
    #[serde(default)]
    pub cache_rehydration_count: u64,
    pub context_available: bool,
    pub context_window: u64,
    /// Summed on-device cost estimate (`~$`) across priceable sessions, or `None`
    /// when none of the sessions had a known model.
    #[serde(default)]
    pub cost_total_usd: Option<f64>,
    pub buckets: Vec<Bucket>,
    pub sessions: Vec<SessionMetrics>,
}

impl ActiveSessionsSummary {
    pub fn empty() -> ActiveSessionsSummary {
        ActiveSessionsSummary {
            session_count: 0,
            avg_duration_secs: 0,
            avg_active_secs: 0,
            tool_mix: ToolMix::default(),
            grep_total: 0,
            tokens_in_total: 0,
            tokens_out_total: 0,
            peak_context_tokens: 0,
            compaction_count: 0,
            cache_rehydration_count: 0,
            context_available: false,
            context_window: CONTEXT_WINDOW,
            cost_total_usd: None,
            buckets: vec![Bucket::default(); BUCKETS],
            sessions: Vec::new(),
        }
    }
}

/// Compute metrics for one normalized session.
///
/// `session` may be a plain transcript, or the result of
/// [`crate::analysis::merge_subagent_events`] merging a parent with its
/// sub-agents into one event stream. In the merged case, token, tool, and
/// cost tallies sum over every event regardless of [`crate::analysis::model::EventSource`]
/// (a sub-agent is an implementation detail of its parent), while context
/// occupancy, compaction, and cache-rehydration detection read parent-tagged
/// events only (a sub-agent has its own context window).
pub fn analyze_session(session: &NormalizedSession) -> SessionMetrics {
    let summary = crate::analysis::interface::SessionSummary {
        cache_write_tokens_available: session.cache_write_tokens_available,
        context_window: session.context_window,
        model: session.model.clone(),
        late_tools: Vec::new(),
        initial_context: None,
        skill_descriptions: HashMap::new(),
    };
    crate::analysis::metrics_sink::SessionMetricsAccumulator::from_parts(
        session.agent.clone(),
        session.session_id.clone(),
        session.events.clone(),
        summary,
    )
    .metrics()
}

/// Rescale a token count measured against `window` into the shared
/// `reference` window, so occupancy from models with different context sizes
/// lands on one comparable axis. The summary then keeps a single
/// `context_window` denominator for the frontend. A session whose window is
/// already the reference keeps its raw token counts.
fn to_reference_window(tokens: u64, window: u64, reference: u64) -> u64 {
    if window == 0 || window == reference {
        return tokens;
    }
    ((tokens as u128 * reference as u128) / window as u128) as u64
}

/// The reference window for a summary: the largest window among the sessions
/// that report context. A single session keeps its own window, so the detail
/// view shows real token counts. A mixed summary scales the smaller windows up
/// to the largest one instead of down to a fixed tier.
fn reference_window(metrics: &[SessionMetrics]) -> u64 {
    metrics
        .iter()
        .filter(|m| m.context_available)
        .map(|m| m.context_window)
        .max()
        .unwrap_or(CONTEXT_WINDOW)
}

/// Aggregate per-session metrics into the averaged live summary.
pub fn aggregate(sessions: &[NormalizedSession]) -> ActiveSessionsSummary {
    let metrics: Vec<SessionMetrics> = sessions
        .iter()
        .map(analyze_session)
        .filter(|m| m.event_count > 0)
        .collect();

    let session_count = metrics.len();
    let summary = aggregate_metrics(metrics);
    ::tracing::trace!(event = "analysis_aggregate_done", sessions = session_count);
    summary
}

/// Aggregate already-analyzed sessions without retaining their raw transcripts.
pub fn aggregate_metrics(metrics: Vec<SessionMetrics>) -> ActiveSessionsSummary {
    if metrics.is_empty() {
        return ActiveSessionsSummary::empty();
    }

    let count = metrics.len();
    let mut tool_mix = ToolMix::default();
    let mut grep_total = 0u64;
    let mut tokens_in_total = 0u64;
    let mut tokens_out_total = 0u64;
    let mut peak_context = 0u64;
    let mut duration_sum = 0u64;
    let mut active_sum = 0u64;
    let mut compaction_count = 0u64;
    let mut cache_rehydration_count = 0u64;
    // Sum cost only over priceable sessions; stays `None` if none had a known model.
    let mut cost_total_usd: Option<f64> = None;
    let reference = reference_window(&metrics);
    for m in &metrics {
        tool_mix.merge(m.tool_mix);
        grep_total += m.grep_count;
        tokens_in_total += m.tokens_in;
        tokens_out_total += m.tokens_out;
        compaction_count += m.compaction_count;
        cache_rehydration_count += m.cache_rehydration_count;
        if m.context_available {
            peak_context = peak_context.max(to_reference_window(
                m.peak_context_tokens,
                m.context_window,
                reference,
            ));
        }
        duration_sum += m.duration_secs;
        active_sum += m.active_secs;
        if let Some(c) = m.cost {
            cost_total_usd = Some(cost_total_usd.unwrap_or(0.0) + c.total_usd);
        }
    }

    // Average observed token and context values over sessions with data there.
    let mut buckets = vec![Bucket::default(); BUCKETS];
    for (bi, bucket) in buckets.iter_mut().enumerate() {
        let mut tin = 0u64;
        let mut tout = 0u64;
        let mut subagent_tokens = 0u64;
        let mut cache_read = 0u64;
        let mut cache_write = 0u64;
        // Average only sessions that report values in this bucket.
        let mut tok_contributors = 0u64;
        let mut ctx_sum = 0u64;
        let mut ctx_contributors = 0u64;
        // A compaction (or a rehydration) in any contributing session marks
        // the summary bucket.
        let mut is_compaction_boundary = false;
        let mut is_cache_rehydration = false;
        let mut subagent_launches = 0u32;
        let mut user_prompts = 0u32;
        for m in &metrics {
            let b = &m.buckets[bi];
            is_compaction_boundary |= b.is_compaction_boundary;
            is_cache_rehydration |= b.is_cache_rehydration;
            subagent_launches = subagent_launches.saturating_add(b.subagent_launches);
            user_prompts = user_prompts.saturating_add(b.user_prompts);
            tin = tin.saturating_add(b.tokens_in);
            tout = tout.saturating_add(b.tokens_out);
            subagent_tokens = subagent_tokens.saturating_add(b.subagent_tokens);
            cache_read = cache_read.saturating_add(b.cache_read_tokens);
            cache_write = cache_write.saturating_add(b.cache_write_tokens);
            if b.tokens_in > 0 || b.tokens_out > 0 || b.subagent_tokens > 0 {
                tok_contributors += 1;
            }
            // Normalize each session's occupancy into the shared reference window
            // before averaging, so mixed-vendor summaries stay on one axis.
            let ctx = if m.context_available {
                to_reference_window(b.context_tokens, m.context_window, reference)
            } else {
                0
            };
            if ctx > 0 {
                ctx_sum = ctx_sum.saturating_add(ctx);
                ctx_contributors += 1;
            }
        }
        bucket.tokens_in = tin.checked_div(tok_contributors).unwrap_or(0);
        bucket.tokens_out = tout.checked_div(tok_contributors).unwrap_or(0);
        bucket.subagent_tokens = subagent_tokens.checked_div(tok_contributors).unwrap_or(0);
        bucket.cache_read_tokens = cache_read.checked_div(tok_contributors).unwrap_or(0);
        bucket.cache_write_tokens = cache_write.checked_div(tok_contributors).unwrap_or(0);
        bucket.context_tokens = ctx_sum.checked_div(ctx_contributors).unwrap_or(0);
        bucket.is_compaction_boundary = is_compaction_boundary;
        bucket.is_cache_rehydration = is_cache_rehydration;
        bucket.subagent_launches = subagent_launches;
        bucket.user_prompts = user_prompts;
        // The per-session signals below name one gap, one model, one mode, and
        // one compaction. Each contributing session can run a different agent
        // with its own mode and its own compactions, so a multi-session summary
        // leaves them at their defaults (`None`/`false`). A single-session
        // summary carries them through, so the session detail view can show
        // them.
        if count == 1 {
            let own = &metrics[0].buckets[bi];
            bucket.secs_since_prior_turn = own.secs_since_prior_turn;
            bucket.model = own.model.clone();
            bucket.thinking_mode = own.thinking_mode.clone();
            bucket.speed = own.speed.clone();
            bucket.has_thinking = own.has_thinking;
            bucket.last_tool = own.last_tool.clone();
            bucket.compaction_trigger = own.compaction_trigger;
            bucket.compaction_pre_tokens = own.compaction_pre_tokens;
            bucket.compaction_post_tokens = own.compaction_post_tokens;
        }
    }

    ActiveSessionsSummary {
        session_count: count,
        avg_duration_secs: duration_sum / count as u64,
        avg_active_secs: active_sum / count as u64,
        tool_mix,
        grep_total,
        tokens_in_total,
        tokens_out_total,
        peak_context_tokens: peak_context,
        compaction_count,
        cache_rehydration_count,
        context_available: metrics.iter().any(|m| m.context_available),
        context_window: reference,
        cost_total_usd,
        buckets,
        sessions: metrics,
    }
}
