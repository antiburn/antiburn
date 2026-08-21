// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The analysis engine. Completely vendor-agnostic: it only ever consumes
//! [`NormalizedEvent`]s, so adding a new vendor never touches this file.
//!
//! It resamples observed token and context data onto a shared progress grid and
//! derives the usage, tool, context, and cost metrics the UI renders.

use std::collections::{HashMap, HashSet};

use crate::pricing::ModelTokens;
// Re-exported through `engine` (and in turn the `analysis` module) so consumers
// reach it as `analysis::SkillUse`. The type itself lives in `crate::model::skill`
// alongside the other local-only value types, so the enrichment layer that
// flattens it stays in lockstep with what the engine emits.
pub use crate::model::skill::SkillUse;
use serde::{Deserialize, Serialize};

use crate::analysis::initial_context::InitialContextBreakdown;
use crate::analysis::model::{EventSource, ModelRun, NormalizedSession, ToolCategory};

/// Number of progress buckets each session is resampled onto (0% → 100%).
pub const BUCKETS: usize = 180;
/// Reference context window used to normalize context occupancy.
pub const CONTEXT_WINDOW: u64 = 200_000;
/// Standard context-window tiers, smallest first. Occupancy can never exceed the
/// real window, so a session's peak is a hard lower bound on it — used to snap a
/// nominal window up when the observed peak tops it (e.g. a 1M-beta model we
/// didn't recognize).
const CONTEXT_WINDOW_TIERS: [u64; 2] = [200_000, 1_000_000];
/// Inter-event gaps at or above this are treated as "away" time and excluded
/// from active time; shorter gaps count as active work. Tunable.
pub const IDLE_GAP_MS: i64 = 5 * 60 * 1000;

/// A prompt-cache entry expires after its provider's TTL. When a turn arrives
/// after the cache has lapsed, the whole context re-writes to the cache in one
/// turn instead of only the new suffix. This "cache rehydration" turn costs
/// far more than a normal cache hit, so the engine flags it separately from
/// ordinary cache writes. A turn counts as a rehydration only when all of
/// these hold, each guarded by a named threshold below:
/// - the turn's context is large enough that a full rewrite is not noise;
/// - most of the turn's context is a fresh cache write, not a cache hit;
/// - the turn right before it was mostly served from cache, so the drop to a
///   full rewrite is a real TTL lapse and not just the session's first turn;
/// - the turn is not the first turn after a compaction boundary, since a
///   compaction always rewrites the (now smaller) context and that is not a
///   TTL lapse.
///
/// Ignore contexts under this size; a full rewrite of a small context is
/// cheap and not worth flagging as a rehydration event.
const CACHE_REHYDRATION_MIN_CONTEXT_TOKENS: u64 = 20_000;
/// A turn's cache write must reach this share of its context to count as a
/// full rewrite rather than an incremental cache write.
const CACHE_REHYDRATION_WRITE_RATIO: f64 = 0.8;
/// The previous turn's cache read must reach this share of its context so the
/// rehydration is a real TTL lapse, not the first turn after a fresh session.
const CACHE_REHYDRATION_PRIOR_READ_RATIO: f64 = 0.5;

/// Pure decision function for [`CACHE_REHYDRATION_*`] rehydration detection,
/// factored out so the rule is unit-testable without a full event fixture.
fn is_cache_rehydration_turn(
    context_tokens: u64,
    cache_write_tokens: u64,
    prev_context_tokens: u64,
    prev_cache_read_tokens: u64,
    first_turn_after_compaction: bool,
) -> bool {
    if first_turn_after_compaction
        || context_tokens < CACHE_REHYDRATION_MIN_CONTEXT_TOKENS
        || prev_context_tokens == 0
    {
        return false;
    }
    let write_ratio = cache_write_tokens as f64 / context_tokens as f64;
    let prev_read_ratio = prev_cache_read_tokens as f64 / prev_context_tokens as f64;
    write_ratio >= CACHE_REHYDRATION_WRITE_RATIO
        && prev_read_ratio >= CACHE_REHYDRATION_PRIOR_READ_RATIO
}
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
    fn add(&mut self, category: ToolCategory) {
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
    /// Effective input (fresh + cache-written) / generated output throughput in
    /// this bucket. Cache reads are excluded. `context_tokens` records the
    /// cache-read-inclusive occupancy observed in this bucket.
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub context_tokens: u64,
    /// True when a real compaction boundary lands in this bucket.
    pub is_compaction_boundary: bool,
    /// Cache-read tokens summed over this bucket's turns. Cost-only; already
    /// folded into `context_tokens` occupancy, never into `tokens_in`.
    pub cache_read_tokens: u64,
    /// Cache-write (cache-creation) tokens summed over this bucket's turns.
    /// This is a breakdown of `tokens_in`, not an addition to it — `tokens_in`
    /// already includes cache writes as effective input.
    pub cache_write_tokens: u64,
    /// True when a turn landing in this bucket is a detected cache
    /// rehydration (see `CACHE_REHYDRATION_WRITE_RATIO`).
    pub is_cache_rehydration: bool,
    /// Count of `Task` tool calls in this bucket: how many sub-agents the
    /// parent session launched at this point. Parent turns only — a
    /// sub-agent does not itself launch sub-agents in this count.
    pub subagent_launches: u32,
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
    // Time span (prefer real timestamps; fall back to event ordering).
    let mut timestamps: Vec<i64> = session.events.iter().filter_map(|e| e.ts_ms).collect();
    timestamps.sort_unstable();
    let (first_ts, last_ts) = match (timestamps.first(), timestamps.last()) {
        (Some(&lo), Some(&hi)) => (lo, hi),
        _ => (0, 0),
    };
    let span_ms = (last_ts - first_ts).max(0);
    let duration_secs = (span_ms / 1000) as u64;

    // Active time: sum inter-event gaps, capping each at the idle threshold so
    // long "away" gaps don't inflate the total while a lone real step still
    // contributes its plausible work time.
    let active_ms: i64 = timestamps
        .windows(2)
        .map(|w| (w[1] - w[0]).clamp(0, IDLE_GAP_MS))
        .sum();
    let active_secs = (active_ms / 1000) as u64;

    let mut cum_active: Vec<(i64, i64)> = Vec::with_capacity(timestamps.len());
    let mut acc = 0i64;
    for (k, &t) in timestamps.iter().enumerate() {
        if k > 0 {
            acc += (t - timestamps[k - 1]).clamp(0, IDLE_GAP_MS);
        }
        // Keep the first occurrence per timestamp (its active time so far).
        if cum_active.last().map(|&(ts, _)| ts) != Some(t) {
            cum_active.push((t, acc));
        }
    }
    let active_progress = |t: i64| -> f32 {
        if active_ms <= 0 {
            return 0.0;
        }
        match cum_active.binary_search_by(|&(ts, _)| ts.cmp(&t)) {
            Ok(i) => cum_active[i].1 as f32 / active_ms as f32,
            Err(_) => 0.0,
        }
    };

    // Collect observed usage and skill invocations on the same active-time grid.
    let mut skill_uses: Vec<SkillUse> = Vec::new();
    let mut skill_event_idx: Vec<usize> = Vec::new();
    let mut buckets = vec![Bucket::default(); BUCKETS];
    let n = session.events.len();
    // Timestamp-less events use the position of the preceding timestamped event.
    let mut last_progress = 0.0f32;
    // Cache-rehydration detection state, carried turn to turn (a "turn" is an
    // event with non-zero context occupancy; events with no usage, like tool
    // results, don't move this state).
    let mut prev_turn_context = 0u64;
    let mut prev_turn_cache_read = 0u64;
    let mut awaiting_first_turn_after_compaction = false;
    for (idx, ev) in session.events.iter().enumerate() {
        if active_ms > 0
            && let Some(ts) = ev.ts_ms
        {
            last_progress = active_progress(ts);
        }
        // Use event order only when the session has no usable active-time span.
        let progress = if active_ms > 0 {
            last_progress
        } else if n > 1 {
            idx as f32 / (n - 1) as f32
        } else {
            0.0
        }
        .clamp(0.0, 1.0);
        let bi = ((progress * BUCKETS as f32) as usize).min(BUCKETS - 1);
        let bucket = &mut buckets[bi];
        // Tokens sum across every stream: a sub-agent's spend counts toward
        // this session's total, per the product rule that a sub-agent is an
        // implementation detail of its parent.
        bucket.tokens_in = bucket
            .tokens_in
            .saturating_add(ev.usage.effective_input_tokens());
        bucket.tokens_out = bucket.tokens_out.saturating_add(ev.usage.output_tokens);
        bucket.cache_read_tokens = bucket
            .cache_read_tokens
            .saturating_add(ev.usage.cache_read_tokens);
        bucket.cache_write_tokens = bucket
            .cache_write_tokens
            .saturating_add(ev.usage.cache_creation_tokens);

        // Context occupancy, compaction, and cache-rehydration read the
        // parent's own turns only. A sub-agent has its own context window,
        // so folding its occupancy into the parent's would not mean anything.
        if ev.source == EventSource::Parent {
            bucket.context_tokens = bucket.context_tokens.max(ev.usage.context_tokens());
            bucket.is_compaction_boundary |= ev.is_compaction_boundary;
            if ev.is_compaction_boundary {
                awaiting_first_turn_after_compaction = true;
            }
            let context_tokens = ev.usage.context_tokens();
            if context_tokens > 0 {
                if is_cache_rehydration_turn(
                    context_tokens,
                    ev.usage.cache_creation_tokens,
                    prev_turn_context,
                    prev_turn_cache_read,
                    awaiting_first_turn_after_compaction,
                ) {
                    bucket.is_cache_rehydration = true;
                }
                prev_turn_context = context_tokens;
                prev_turn_cache_read = ev.usage.cache_read_tokens;
                awaiting_first_turn_after_compaction = false;
            }
            // A `Task` tool call launches a sub-agent. Mark the bucket so a
            // reader can see it in the tooltip; this never draws a chart line.
            let launches = ev
                .tools
                .iter()
                .filter(|tool| tool.name.eq_ignore_ascii_case("task"))
                .count() as u32;
            bucket.subagent_launches = bucket.subagent_launches.saturating_add(launches);

            // Mode signals: the last parent event seen in this bucket wins.
            // Sub-agents run their own model/effort/speed, which says nothing
            // about the parent session's mode at this point, so they never
            // reach here.
            if let Some(model) = ev.model.as_deref().filter(|m| !m.is_empty()) {
                bucket.model = Some(model.to_string());
            }
            if let Some(mode) = ev.thinking_mode.as_deref().filter(|m| !m.is_empty()) {
                bucket.thinking_mode = Some(mode.to_string());
            }
            if let Some(speed) = ev.speed.as_deref().filter(|s| !s.is_empty()) {
                bucket.speed = Some(speed.to_string());
            }
            bucket.has_thinking |= ev.has_thinking;
        }

        // Collect skill invocations for exports.
        for tool in &ev.tools {
            if tool.name.eq_ignore_ascii_case("skill") {
                skill_uses.push(SkillUse {
                    name: tool.detail.clone().unwrap_or_else(|| "skill".to_string()),
                    progress,
                    description: None, // grafted from the raw transcript later
                    duration_ms: None, // resolved in the post-pass below
                    tokens_out: ev.usage.output_tokens,
                    context_tokens: ev.usage.context_tokens(),
                });
                skill_event_idx.push(idx);
            }
        }
    }

    // A compaction boundary is an observed reset, so its bucket starts at zero.
    for bucket in &mut buckets {
        if bucket.is_compaction_boundary {
            bucket.context_tokens = 0;
        }
    }

    // Resolve each skill use's `duration_ms`: the idle-capped gap from its event's
    // timestamp to the next distinct timestamp in the session. `None` when the
    // event (or the whole session) carries no timestamp, or it's the last activity.
    for (use_, &ev_idx) in skill_uses.iter_mut().zip(skill_event_idx.iter()) {
        if let Some(t0) = session.events[ev_idx].ts_ms
            && let Some(&t1) = timestamps.iter().find(|&&t| t > t0)
        {
            use_.duration_ms = Some((t1 - t0).clamp(0, IDLE_GAP_MS));
        }
    }

    // Token / tool / context tallies. `tokens_in`/`tokens_out` are the displayed
    // effective input (fresh + cache writes, matching the web app) / generated
    // output. Cache reads are excluded and surface as occupancy via `peak_context` /
    // `context_tokens`. The `bill_*` accumulators carry the four per-rate components
    // for the cost estimate. These tallies (and `tool_mix`/`grep_count` below) sum
    // over every event, including any sub-agent merged in by
    // `merge_subagent_events` — only `peak_context` stays parent-only, guarded below.
    let mut tokens_in = 0u64;
    let mut tokens_out = 0u64;
    let mut bill_in = 0u64;
    let mut bill_out = 0u64;
    let mut bill_cr = 0u64;
    let mut bill_cw = 0u64;
    let mut peak_context = 0u64;
    let mut tool_mix = ToolMix::default();
    let mut grep_count = 0u64;
    // Per-model token breakdown for cost: attribute each turn's billable tokens to
    // the model that produced it so cost prices per-model (a session that mixes
    // Opus + Haiku is no longer billed entirely at Opus). The aggregate `bill_*`
    // totals above are unchanged and the breakdown sums to them.
    let mut cost_breakdown: HashMap<String, ModelTokens> = HashMap::new();
    let mut model_runs = Vec::new();
    let mut seen_model_runs = HashSet::new();
    for ev in &session.events {
        tokens_in = tokens_in.saturating_add(ev.usage.effective_input_tokens());
        tokens_out = tokens_out.saturating_add(ev.usage.output_tokens);
        bill_in = bill_in.saturating_add(ev.usage.input_tokens);
        bill_out = bill_out.saturating_add(ev.usage.output_tokens);
        bill_cr = bill_cr.saturating_add(ev.usage.cache_read_tokens);
        bill_cw = bill_cw.saturating_add(ev.usage.cache_creation_tokens);
        // Peak occupancy is a parent-only figure: a sub-agent's own context
        // window does not describe the parent's, so it never raises this.
        if ev.source == EventSource::Parent {
            peak_context = peak_context.max(ev.usage.context_tokens());
        }
        let u = ev.usage;
        let has_tokens = u.input_tokens != 0
            || u.output_tokens != 0
            || u.cache_read_tokens != 0
            || u.cache_creation_tokens != 0;
        // Resolve the owning model: the event's own model, else the session
        // headline. Tokens with no resolvable model are unpriceable, so skip them.
        if has_tokens && let Some(model) = ev.model.as_deref().or(session.model.as_deref()) {
            let model = crate::analysis::pricing::strip_window_tag(model)
                .trim()
                .to_string();
            if !model.is_empty() {
                let run = ModelRun {
                    model: model.clone(),
                    thinking_mode: ev
                        .thinking_mode
                        .as_deref()
                        .map(str::trim)
                        .filter(|mode| !mode.is_empty())
                        .map(str::to_string),
                };
                if seen_model_runs.insert(run.clone()) {
                    model_runs.push(run);
                }
                let entry = cost_breakdown.entry(model).or_default();
                entry.input_tokens = entry.input_tokens.saturating_add(u.input_tokens);
                entry.output_tokens = entry.output_tokens.saturating_add(u.output_tokens);
                entry.cache_read_tokens =
                    entry.cache_read_tokens.saturating_add(u.cache_read_tokens);
                entry.cache_creation_tokens = entry
                    .cache_creation_tokens
                    .saturating_add(u.cache_creation_tokens);
            }
        }
        for tool in &ev.tools {
            tool_mix.add(tool.category);
            if ToolCategory::is_grep(&tool.name) {
                grep_count += 1;
            }
        }
    }

    // The model's real context window (Codex reports it; Claude infers it from
    // recognized model ids), else the internal reference default.
    let context_available = session.agent != "claude" || session.context_window.is_some();
    let context_window = resolve_context_window(
        session.context_window.unwrap_or(CONTEXT_WINDOW),
        peak_context,
    );
    // On-device cost estimate, priced per-model from the breakdown accumulated above.
    let cost = crate::analysis::pricing::price_breakdown(&cost_breakdown);

    SessionMetrics {
        agent: session.agent.clone(),
        session_id: session.session_id.clone(),
        duration_secs,
        active_secs,
        event_count: n,
        tokens_in,
        tokens_out,
        peak_context_tokens: peak_context,
        context_available,
        context_window,
        tool_mix,
        grep_count,
        buckets,
        initial_context: None,
        model: session.model.clone(),
        model_runs,
        billable_input_tokens: bill_in,
        billable_output_tokens: bill_out,
        billable_cache_read_tokens: bill_cr,
        billable_cache_creation_tokens: bill_cw,
        model_breakdown: cost_breakdown,
        cost,
        skill_uses,
    }
}

/// Resolve a session's context window: the reported/inferred size, snapped up to
/// the smallest standard tier that still fits the observed peak occupancy. A
/// session can never hold more tokens than its window, so a peak above the
/// nominal window means the real one is larger (an undetected 1M-beta model).
fn resolve_context_window(reported: u64, peak: u64) -> u64 {
    let base = reported.max(1);
    if peak <= base {
        return base;
    }
    CONTEXT_WINDOW_TIERS
        .iter()
        .copied()
        .find(|&t| t >= peak)
        .unwrap_or(peak)
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
    // Sum cost only over priceable sessions; stays `None` if none had a known model.
    let mut cost_total_usd: Option<f64> = None;
    let reference = reference_window(&metrics);
    for m in &metrics {
        tool_mix.merge(m.tool_mix);
        grep_total += m.grep_count;
        tokens_in_total += m.tokens_in;
        tokens_out_total += m.tokens_out;
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
        for m in &metrics {
            let b = &m.buckets[bi];
            is_compaction_boundary |= b.is_compaction_boundary;
            is_cache_rehydration |= b.is_cache_rehydration;
            subagent_launches = subagent_launches.saturating_add(b.subagent_launches);
            tin = tin.saturating_add(b.tokens_in);
            tout = tout.saturating_add(b.tokens_out);
            cache_read = cache_read.saturating_add(b.cache_read_tokens);
            cache_write = cache_write.saturating_add(b.cache_write_tokens);
            if b.tokens_in > 0 || b.tokens_out > 0 {
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
        bucket.cache_read_tokens = cache_read.checked_div(tok_contributors).unwrap_or(0);
        bucket.cache_write_tokens = cache_write.checked_div(tok_contributors).unwrap_or(0);
        bucket.context_tokens = ctx_sum.checked_div(ctx_contributors).unwrap_or(0);
        bucket.is_compaction_boundary = is_compaction_boundary;
        bucket.is_cache_rehydration = is_cache_rehydration;
        bucket.subagent_launches = subagent_launches;
        // `model`, `thinking_mode`, `speed`, and `has_thinking` stay at their
        // default (`None`/`false`) here. Each contributing session can be a
        // different agent with its own model and mode, so one bucket cannot
        // carry a single true value across a multi-session summary — the
        // mode-change chart annotation is a single-session feature only.
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
        context_available: metrics.iter().any(|m| m.context_available),
        context_window: reference,
        cost_total_usd,
        buckets,
        sessions: metrics,
    }
}
