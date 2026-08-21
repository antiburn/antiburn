// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The analysis engine. Completely vendor-agnostic: it only ever consumes
//! [`NormalizedEvent`]s, so adding a new vendor never touches this file.
//!
//! It classifies each turn into a [`Phase`] (the hypnogram bands), resamples
//! every session onto a shared progress grid so multiple live sessions can be
//! averaged, and derives the token / tool / context / pattern-score metrics the
//! sleep-style UI renders.

use std::collections::{HashMap, HashSet};

use crate::pricing::ModelTokens;
// Re-exported through `engine` (and in turn the `analysis` module) so consumers
// reach it as `analysis::SkillUse`. The type itself lives in `crate::model::skill`
// alongside the other local-only value types, so the enrichment layer that
// flattens it stays in lockstep with what the engine emits.
pub use crate::model::skill::SkillUse;
use serde::{Deserialize, Serialize};

use crate::analysis::initial_context::InitialContextBreakdown;
use crate::analysis::model::{ModelRun, NormalizedEvent, NormalizedSession, Role, ToolCategory};

/// Number of progress buckets each session is resampled onto (0% → 100%).
/// Higher = finer time resolution, so fewer turns land in the same bucket and
/// each bucket is more likely a single mode (closer to one lane per slice).
pub const BUCKETS: usize = 180;
/// Reference context window used to normalize context-drift (Claude-class).
pub const CONTEXT_WINDOW: u64 = 200_000;
/// Standard context-window tiers, smallest first. Occupancy can never exceed the
/// real window, so a session's peak is a hard lower bound on it — used to snap a
/// nominal window up when the observed peak tops it (e.g. a 1M-beta model we
/// didn't recognize).
const CONTEXT_WINDOW_TIERS: [u64; 2] = [200_000, 1_000_000];
/// Inter-event gaps at or above this are treated as "away" time and excluded
/// from active time; shorter gaps count as active work. Tunable.
pub const IDLE_GAP_MS: i64 = 5 * 60 * 1000;
/// Floor weight for a turn's contribution to its bucket's phase mix. Turns are
/// weighted by output tokens so the per-bucket dominant reflects produced work
/// rather than turn count; this floor keeps zero-output turns (user/tool-result
/// turns, many errors) from vanishing entirely.
pub const MIN_PHASE_WEIGHT: f32 = 1.0;

/// The hypnogram bands, deepest → shallowest, mirroring sleep stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Deep work: editing/writing files.
    Implementing,
    /// Verifying: running tests / builds / checks (a passing test run).
    Testing,
    /// Light activity: reading, searching, running commands.
    Exploring,
    /// REM-like: reasoning / planning with no tool calls.
    Thinking,
    /// Wake-ups: errors, failed tools, corrections (the "bad pattern" signal).
    Disruption,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseDistribution {
    pub implementing: f32,
    pub testing: f32,
    pub exploring: f32,
    pub thinking: f32,
    pub disruption: f32,
}

impl PhaseDistribution {
    fn add_phase(&mut self, phase: Phase, weight: f32) {
        match phase {
            Phase::Implementing => self.implementing += weight,
            Phase::Testing => self.testing += weight,
            Phase::Exploring => self.exploring += weight,
            Phase::Thinking => self.thinking += weight,
            Phase::Disruption => self.disruption += weight,
        }
    }

    fn total(&self) -> f32 {
        self.implementing + self.testing + self.exploring + self.thinking + self.disruption
    }

    fn normalized(&self) -> PhaseDistribution {
        let total = self.total();
        if total <= 0.0 {
            return PhaseDistribution::default();
        }
        PhaseDistribution {
            implementing: self.implementing / total,
            testing: self.testing / total,
            exploring: self.exploring / total,
            thinking: self.thinking / total,
            disruption: self.disruption / total,
        }
    }

    fn dominant(&self) -> Phase {
        let entries = [
            (Phase::Implementing, self.implementing),
            (Phase::Testing, self.testing),
            (Phase::Exploring, self.exploring),
            (Phase::Thinking, self.thinking),
            (Phase::Disruption, self.disruption),
        ];
        entries
            .iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(phase, _)| *phase)
            .unwrap_or(Phase::Thinking)
    }
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

    fn total(&self) -> u64 {
        self.edit + self.read + self.search + self.test + self.bash + self.other
    }
}

/// One point on the shared 0→100% progress grid.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub dominant_phase: Option<Phase>,
    pub distribution: PhaseDistribution,
    /// Effective input (fresh + cache-written) / generated output throughput in
    /// this bucket — the Tokens area chart. Cache reads are excluded;
    /// `context_tokens` is the cache-read-inclusive occupancy high-water mark that
    /// drives Context Drift instead.
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub context_tokens: u64,
    /// True when a real compaction boundary (Claude `compact_boundary`, Codex
    /// `context_compacted`) landed in this bucket — where the context-drift
    /// high-water mark resets instead of forward-filling.
    pub is_compaction_boundary: bool,
}

/// One contiguous run of a single mode on the active-time axis. The per-turn
/// hypnogram is mutually exclusive by construction — each segment is exactly one
/// phase — with consecutive same-phase turns merged so it doesn't fragment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseSegment {
    pub phase: Phase,
    /// Active time this run occupied (idle gaps capped), in milliseconds.
    pub active_ms: i64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub context_tokens: u64,
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
    /// Earliest event timestamp (epoch ms), or `None` when the session carries no
    /// timestamps. Lets a caller place this session on another session's timeline
    /// — e.g. a sub-agent's spawn instant on its orchestrator's hypnogram — from
    /// the analysis output alone, without re-parsing the transcript.
    #[serde(default)]
    pub first_ts_ms: Option<i64>,
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
    pub context_fraction: f32,
    /// Whether the model context window is known well enough to present
    /// occupancy. Unknown Claude model ids deliberately leave this unavailable.
    pub context_available: bool,
    /// The model's context-window size used to normalize occupancy for this
    /// session (Codex's reported window, else the reference `CONTEXT_WINDOW`).
    /// Aggregation rescales each session's occupancy into the shared reference.
    pub context_window: u64,
    pub tool_mix: ToolMix,
    pub grep_count: u64,
    pub disruption_count: u64,
    pub phase_distribution: PhaseDistribution,
    pub pattern_score: u8,
    pub signals: Vec<String>,
    pub buckets: Vec<Bucket>,
    /// Per-turn mode runs on the active-time axis (single-session hypnogram).
    pub segments: Vec<PhaseSegment>,
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
    /// call, in event order. Each carries its active-time position (the hypnogram
    /// x-axis), the invoking turn's token figures, and an idle-capped duration;
    /// `description` is grafted from the raw transcript later in `analyze_sources`.
    /// Empty when the session invoked no skills. Phase classification, `ToolMix`,
    /// scoring, and cost are unaffected — skills stay in `ToolCategory::Other`.
    #[serde(default)]
    pub skill_uses: Vec<SkillUse>,
}

/// Averaged view across all live sessions — what the hypnogram + cards render.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSessionsSummary {
    pub session_count: usize,
    pub avg_duration_secs: u64,
    pub avg_active_secs: u64,
    pub avg_pattern_score: u8,
    pub phase_distribution: PhaseDistribution,
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
    pub signals: Vec<String>,
    pub sessions: Vec<SessionMetrics>,
}

impl ActiveSessionsSummary {
    pub fn empty() -> ActiveSessionsSummary {
        ActiveSessionsSummary {
            session_count: 0,
            avg_duration_secs: 0,
            avg_active_secs: 0,
            avg_pattern_score: 0,
            phase_distribution: PhaseDistribution::default(),
            tool_mix: ToolMix::default(),
            grep_total: 0,
            tokens_in_total: 0,
            tokens_out_total: 0,
            peak_context_tokens: 0,
            context_available: false,
            context_window: CONTEXT_WINDOW,
            cost_total_usd: None,
            buckets: vec![Bucket::default(); BUCKETS],
            signals: Vec::new(),
            sessions: Vec::new(),
        }
    }
}

/// Classify a single event into a phase, or `None` if it carries no signal
/// (e.g. a bare user prompt with no tools/error).
fn classify(ev: &NormalizedEvent) -> Option<Phase> {
    if ev.is_error {
        return Some(Phase::Disruption);
    }
    if ev.tools.iter().any(|t| t.category == ToolCategory::Edit) {
        return Some(Phase::Implementing);
    }
    if ev.tools.iter().any(|t| t.category == ToolCategory::Test) {
        return Some(Phase::Testing);
    }
    if ev.tools.iter().any(|t| {
        matches!(
            t.category,
            ToolCategory::Read | ToolCategory::Search | ToolCategory::Bash
        )
    }) {
        return Some(Phase::Exploring);
    }
    if ev.role == Role::Assistant && (ev.thinking || ev.text_len > 0) {
        return Some(Phase::Thinking);
    }
    None
}

struct ClassifiedEvent<'a> {
    event_index: usize,
    phase: Phase,
    progress: f32,
    event: &'a NormalizedEvent,
}

/// Compute metrics for one normalized session.
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

    // Cumulative active time at each distinct timestamp (idle gaps capped). The
    // hypnogram's x-axis is ACTIVE time — matching `active_secs` and the phase
    // donut — rather than wall-clock, which would smear events toward the start
    // and leave long empty stretches wherever the session went idle.
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

    // Classify and assign a 0..1 progress (in active time) to each signal-bearing event.
    let mut classified: Vec<ClassifiedEvent> = Vec::new();
    // Skill invocations, collected alongside classification (see below). The
    // parallel `skill_event_idx` records each use's source event so a post-pass
    // can resolve its idle-capped duration from the sorted timestamps.
    let mut skill_uses: Vec<SkillUse> = Vec::new();
    let mut skill_event_idx: Vec<usize> = Vec::new();
    // Progress buckets where a compaction boundary landed, collected during the
    // same loop (before the classify early-`continue`, like skill markers).
    let mut compaction_bucket_indices: Vec<usize> = Vec::new();
    let n = session.events.len();
    // Active-time progress of the most recent timestamped turn. Timestamp-less
    // events reuse it so they sit at the same active-time position as the turn
    // before them, rather than dropping onto a turn-order estimate that ignores
    // the active-time axis.
    let mut last_progress = 0.0f32;
    for (idx, ev) in session.events.iter().enumerate() {
        if active_ms > 0
            && let Some(ts) = ev.ts_ms
        {
            last_progress = active_progress(ts);
        }
        // This event's 0..1 active-time position: `last_progress` is its own
        // position when it has a timestamp (set just above), or the preceding
        // timestamped turn's when it doesn't; the `idx/(n-1)` fallback applies
        // only to timestamp-less sessions. Computed once so skill markers and
        // classified turns share the same axis position.
        let progress = if active_ms > 0 {
            last_progress
        } else if n > 1 {
            idx as f32 / (n - 1) as f32
        } else {
            0.0
        }
        .clamp(0.0, 1.0);

        // Collect skill invocations BEFORE the classify early-`continue`: a bare
        // `Skill` tool_use is `ToolCategory::Other`, so `classify` returns `None`
        // and the event is skipped — collecting after would drop the marker. The
        // skill name comes from `ToolCall::detail` (filled by the vendor-neutral
        // `tool_call_from_input`), falling back to a generic "skill".
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

        // Record compaction boundaries BEFORE the classify early-`continue`: the
        // marker record itself carries no tools/text, so `classify` returns
        // `None` and collecting after would drop it.
        if ev.is_compaction_boundary {
            let bi = ((progress * BUCKETS as f32) as usize).min(BUCKETS - 1);
            compaction_bucket_indices.push(bi);
        }

        let Some(phase) = classify(ev) else { continue };
        classified.push(ClassifiedEvent {
            event_index: idx,
            phase,
            progress,
            event: ev,
        });
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
    // for the cost estimate.
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
        peak_context = peak_context.max(ev.usage.context_tokens());
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

    // Phase distribution + buckets.
    let mut distribution = PhaseDistribution::default();
    let mut buckets = vec![Bucket::default(); BUCKETS];
    let mut bucket_dist = vec![PhaseDistribution::default(); BUCKETS];
    for c in &classified {
        // Session-global mix stays turn-count-based (feeds the pattern score).
        distribution.add_phase(c.phase, 1.0);
        let bi = ((c.progress * BUCKETS as f32) as usize).min(BUCKETS - 1);
        // Per-bucket mix (drives the hypnogram's dominant lane) is weighted by
        // output tokens so a long, productive turn outweighs brief ones.
        let weight = (c.event.usage.output_tokens as f32).max(MIN_PHASE_WEIGHT);
        bucket_dist[bi].add_phase(c.phase, weight);
    }
    for (bi, dist) in bucket_dist.iter().enumerate() {
        if dist.total() > 0.0 {
            buckets[bi].distribution = dist.normalized();
            buckets[bi].dominant_phase = Some(dist.dominant());
        }
    }
    for &bi in &compaction_bucket_indices {
        buckets[bi].is_compaction_boundary = true;
    }

    // Usage is sometimes reported as a standalone bookkeeping event (Codex
    // `token_count`) rather than on the message/tool event that produced it.
    // Anchor those token deltas to the nearest active bucket so token/context
    // charts stay visible without treating bookkeeping as a session phase.
    if !classified.is_empty() {
        let mut anchor_idx = 0usize;
        for (idx, ev) in session.events.iter().enumerate() {
            let tin = ev.usage.effective_input_tokens();
            let tout = ev.usage.output_tokens;
            let ctx = ev.usage.context_tokens();
            if tin == 0 && tout == 0 && ctx == 0 {
                continue;
            }
            while anchor_idx + 1 < classified.len() && classified[anchor_idx + 1].event_index <= idx
            {
                anchor_idx += 1;
            }
            let anchor = if classified[anchor_idx].event_index <= idx {
                &classified[anchor_idx]
            } else {
                &classified[0]
            };
            let bi = ((anchor.progress * BUCKETS as f32) as usize).min(BUCKETS - 1);
            buckets[bi].tokens_in = buckets[bi].tokens_in.saturating_add(tin);
            buckets[bi].tokens_out = buckets[bi].tokens_out.saturating_add(tout);
            buckets[bi].context_tokens = buckets[bi].context_tokens.max(ctx);
        }
    }

    // Carry context occupancy forward as a high-water mark across active
    // buckets, resetting at real compaction boundaries. Between compactions,
    // occupancy only grows as history accumulates, so a bucket that recorded a
    // lower (or zero) peak than an earlier one is a sampling gap, not a real
    // drop — filling forward keeps the drift curve a "how full has the window
    // gotten" line, distinct from per-bucket token throughput (which otherwise
    // collapses to the same curve when buckets hold a single event: Codex
    // `token_count`, OpenCode per-message rows). But when the vendor marked a
    // genuine compaction in a bucket, the drop is real: zero both the running
    // mark AND that bucket's own `context_tokens` there, so the curve
    // sawtooths back down instead of smoothing the compaction away. Zeroing
    // the bucket's own reading (not just `continue`-ing past it) matters when
    // the boundary shares a bucket with the last pre-compaction turn — the
    // anchoring pass above may have already stamped that turn's high reading
    // onto this exact bucket, and without clearing it, the very next line
    // (`running_context.max(b.context_tokens)`) would immediately resurrect
    // the pre-compaction peak and defeat the reset for every bucket after it.
    let mut running_context = 0u64;
    for b in &mut buckets {
        if b.is_compaction_boundary {
            running_context = 0;
            b.context_tokens = 0;
        }
        if b.dominant_phase.is_none() {
            continue;
        }
        running_context = running_context.max(b.context_tokens);
        b.context_tokens = running_context;
    }

    // Per-turn segments: each classified turn is one mode (mutually exclusive),
    // sized by the active time until the next turn (idle gaps capped). The last
    // turn runs to session end. Consecutive same-phase turns are merged so the
    // ribbon doesn't shatter into noise.
    let mut segments: Vec<PhaseSegment> = Vec::new();
    for (k, c) in classified.iter().enumerate() {
        let dwell = match (
            c.event.ts_ms,
            classified.get(k + 1).and_then(|nx| nx.event.ts_ms),
        ) {
            (Some(t0), Some(t1)) => (t1 - t0).clamp(0, IDLE_GAP_MS),
            (Some(t0), None) => (last_ts - t0).clamp(0, IDLE_GAP_MS),
            _ => 1, // no usable timestamps: equal unit widths
        };
        let tin = c.event.usage.effective_input_tokens();
        let tout = c.event.usage.output_tokens;
        let ctx = c.event.usage.context_tokens();
        match segments.last_mut() {
            Some(seg) if seg.phase == c.phase => {
                seg.active_ms += dwell;
                seg.tokens_in = seg.tokens_in.saturating_add(tin);
                seg.tokens_out = seg.tokens_out.saturating_add(tout);
                seg.context_tokens = seg.context_tokens.max(ctx);
            }
            _ => segments.push(PhaseSegment {
                phase: c.phase,
                active_ms: dwell,
                tokens_in: tin,
                tokens_out: tout,
                context_tokens: ctx,
            }),
        }
    }

    let disruption_count = classified
        .iter()
        .filter(|c| c.phase == Phase::Disruption)
        .count() as u64;
    // The model's real context window (Codex reports it; Claude infers it from
    // recognized model ids), else the internal reference default. Unknown
    // Claude ids retain the fallback for score compatibility but mark context
    // unavailable so occupancy is never presented as authoritative.
    let context_available = session.agent != "claude" || session.context_window.is_some();
    let context_window = resolve_context_window(
        session.context_window.unwrap_or(CONTEXT_WINDOW),
        peak_context,
    );
    let context_fraction = if context_available {
        peak_context as f32 / context_window as f32
    } else {
        0.0
    };
    let phase_distribution = distribution.normalized();
    let (pattern_score, signals) =
        score_pattern(&phase_distribution, &tool_mix, grep_count, context_fraction);

    // On-device cost estimate, priced per-model from the breakdown accumulated above.
    let cost = crate::analysis::pricing::price_breakdown(&cost_breakdown);

    SessionMetrics {
        agent: session.agent.clone(),
        session_id: session.session_id.clone(),
        // `timestamps` is sorted ascending, so the first entry is the earliest.
        first_ts_ms: timestamps.first().copied(),
        duration_secs,
        active_secs,
        event_count: n,
        tokens_in,
        tokens_out,
        peak_context_tokens: peak_context,
        context_fraction,
        context_available,
        context_window,
        tool_mix,
        grep_count,
        disruption_count,
        phase_distribution,
        pattern_score,
        signals,
        buckets,
        segments,
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

/// Active-time position (0..1) of an arbitrary wall-clock `ts` along this
/// session's timeline, on the **same idle-capped active-time axis the hypnogram
/// uses** — not wall-clock. Inter-event gaps are capped at `IDLE_GAP_MS` before
/// accumulating, so the fraction matches `active_secs` and the segment/bucket
/// widths rather than smearing toward whichever side a long "away" gap fell on.
///
/// Used to place a sub-agent "spawn dot" on its orchestrator's hypnogram: a
/// sub-agent's creation time is taken as its *first activity* (the earliest
/// event ts in its own transcript), and this maps that wall-clock instant onto
/// the orchestrator's active-time axis.
///
/// Unlike the exact-match `active_progress` closure inside [`analyze_session`],
/// this **interpolates** between the two surrounding events (a spawn ts rarely
/// lands exactly on an orchestrator event) and clamps to `0.0`/`1.0` outside the
/// session span. Returns `None` when the session has no usable active time (no
/// timestamps, or every event at one instant) — there is nothing to map onto.
///
/// It builds its own cumulative-active table rather than sharing
/// `analyze_session`'s — a deliberate isolation choice: a one-shot, non-hot call
/// kept fully clear of the analysis hot path (the ~10 lines are duplicated on
/// purpose, not refactored into it).
pub fn active_time_fraction(session: &NormalizedSession, ts: i64) -> Option<f32> {
    let mut timestamps: Vec<i64> = session.events.iter().filter_map(|e| e.ts_ms).collect();
    timestamps.sort_unstable();
    if timestamps.len() < 2 {
        return None;
    }

    // Cumulative capped active-ms at each distinct timestamp, plus the total —
    // the same construction the hypnogram's active-time axis uses.
    let mut cum: Vec<(i64, i64)> = Vec::with_capacity(timestamps.len());
    let mut acc = 0i64;
    for (k, &t) in timestamps.iter().enumerate() {
        if k > 0 {
            acc += (t - timestamps[k - 1]).clamp(0, IDLE_GAP_MS);
        }
        if cum.last().map(|&(seen, _)| seen) != Some(t) {
            cum.push((t, acc));
        }
    }
    let total = acc;
    if total <= 0 {
        return None;
    }

    // Outside the span → clamp to the ends.
    if ts <= cum[0].0 {
        return Some(0.0);
    }
    if ts >= cum[cum.len() - 1].0 {
        return Some(1.0);
    }

    // Interpolate the capped active-ms within the bracketing [lo, hi] pair.
    // `binary_search` yields the exact hit or the insertion gap; on a gap, `i` is
    // the first entry past `ts`, so `[i-1, i]` brackets it. The bracket's active
    // contribution is `hi_a - lo_a` (already idle-capped), advanced by the
    // wall-clock fraction across the bracket.
    let active_at = match cum.binary_search_by(|&(t, _)| t.cmp(&ts)) {
        Ok(i) => cum[i].1,
        Err(i) => {
            let (lo_t, lo_a) = cum[i - 1];
            let (hi_t, hi_a) = cum[i];
            let span = hi_t - lo_t;
            if span <= 0 {
                lo_a
            } else {
                let frac = (ts - lo_t) as f64 / span as f64;
                lo_a + ((hi_a - lo_a) as f64 * frac).round() as i64
            }
        }
    };

    Some((active_at as f32 / total as f32).clamp(0.0, 1.0))
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

/// Rescale a token count measured against `window` into the shared reference
/// `CONTEXT_WINDOW`, so occupancy from models with different context sizes
/// (e.g. Codex's 258k vs the 200k reference) lands on one comparable axis. This
/// lets the summary keep a single `context_window` denominator for the frontend.
fn to_reference_window(tokens: u64, window: u64) -> u64 {
    if window == 0 || window == CONTEXT_WINDOW {
        return tokens;
    }
    ((tokens as u128 * CONTEXT_WINDOW as u128) / window as u128) as u64
}

/// Aggregate per-session metrics into the averaged live summary.
pub fn aggregate(sessions: &[NormalizedSession]) -> ActiveSessionsSummary {
    let metrics: Vec<SessionMetrics> = sessions
        .iter()
        .map(analyze_session)
        .filter(|m| m.event_count > 0)
        .collect();

    aggregate_metrics(metrics)
}

/// Aggregate already-analyzed sessions without retaining their raw transcripts.
pub fn aggregate_metrics(metrics: Vec<SessionMetrics>) -> ActiveSessionsSummary {
    if metrics.is_empty() {
        return ActiveSessionsSummary::empty();
    }

    let count = metrics.len();
    let mut pooled_dist = PhaseDistribution::default();
    let mut tool_mix = ToolMix::default();
    let mut grep_total = 0u64;
    let mut tokens_in_total = 0u64;
    let mut tokens_out_total = 0u64;
    let mut peak_context = 0u64;
    let mut duration_sum = 0u64;
    let mut active_sum = 0u64;
    let mut score_sum = 0u32;
    // Sum cost only over priceable sessions; stays `None` if none had a known model.
    let mut cost_total_usd: Option<f64> = None;
    for m in &metrics {
        pooled_dist.implementing += m.phase_distribution.implementing;
        pooled_dist.testing += m.phase_distribution.testing;
        pooled_dist.exploring += m.phase_distribution.exploring;
        pooled_dist.thinking += m.phase_distribution.thinking;
        pooled_dist.disruption += m.phase_distribution.disruption;
        tool_mix.merge(m.tool_mix);
        grep_total += m.grep_count;
        tokens_in_total += m.tokens_in;
        tokens_out_total += m.tokens_out;
        if m.context_available {
            peak_context =
                peak_context.max(to_reference_window(m.peak_context_tokens, m.context_window));
        }
        duration_sum += m.duration_secs;
        active_sum += m.active_secs;
        score_sum += m.pattern_score as u32;
        if let Some(c) = m.cost {
            cost_total_usd = Some(cost_total_usd.unwrap_or(0.0) + c.total_usd);
        }
    }

    // Average the per-bucket distributions/tokens over sessions with data there.
    let mut buckets = vec![Bucket::default(); BUCKETS];
    for (bi, bucket) in buckets.iter_mut().enumerate() {
        let mut dist = PhaseDistribution::default();
        let mut contributors = 0f32;
        let mut tin = 0u64;
        let mut tout = 0u64;
        // Sessions that actually carried token activity in this bucket. Averaging
        // over these (not the full session count) keeps the token volume undiluted
        // where sessions don't overlap — mirroring `ctx_contributors` below and the
        // phase mix's `contributors`.
        let mut tok_contributors = 0u64;
        let mut ctx_sum = 0u64;
        let mut ctx_contributors = 0u64;
        // A compaction in any contributing session marks the summary bucket.
        let mut is_compaction_boundary = false;
        for m in &metrics {
            let b = &m.buckets[bi];
            is_compaction_boundary |= b.is_compaction_boundary;
            if b.distribution.total() > 0.0 {
                dist.implementing += b.distribution.implementing;
                dist.testing += b.distribution.testing;
                dist.exploring += b.distribution.exploring;
                dist.thinking += b.distribution.thinking;
                dist.disruption += b.distribution.disruption;
                contributors += 1.0;
            }
            tin = tin.saturating_add(b.tokens_in);
            tout = tout.saturating_add(b.tokens_out);
            if b.tokens_in > 0 || b.tokens_out > 0 {
                tok_contributors += 1;
            }
            // Normalize each session's occupancy into the shared reference window
            // before averaging, so mixed-vendor summaries stay on one % axis.
            let ctx = if m.context_available {
                to_reference_window(b.context_tokens, m.context_window)
            } else {
                0
            };
            if ctx > 0 {
                ctx_sum = ctx_sum.saturating_add(ctx);
                ctx_contributors += 1;
            }
        }
        if contributors > 0.0 {
            bucket.distribution = PhaseDistribution {
                implementing: dist.implementing / contributors,
                testing: dist.testing / contributors,
                exploring: dist.exploring / contributors,
                thinking: dist.thinking / contributors,
                disruption: dist.disruption / contributors,
            };
            bucket.dominant_phase = Some(bucket.distribution.dominant());
        }
        bucket.tokens_in = tin.checked_div(tok_contributors).unwrap_or(0);
        bucket.tokens_out = tout.checked_div(tok_contributors).unwrap_or(0);
        bucket.context_tokens = ctx_sum.checked_div(ctx_contributors).unwrap_or(0);
        bucket.is_compaction_boundary = is_compaction_boundary;
    }

    let phase_distribution = pooled_dist.normalized();
    let context_fraction = peak_context as f32 / CONTEXT_WINDOW as f32;
    let (_, signals) = score_pattern(&phase_distribution, &tool_mix, grep_total, context_fraction);

    ActiveSessionsSummary {
        session_count: count,
        avg_duration_secs: duration_sum / count as u64,
        avg_active_secs: active_sum / count as u64,
        avg_pattern_score: (score_sum / count as u32) as u8,
        phase_distribution,
        tool_mix,
        grep_total,
        tokens_in_total,
        tokens_out_total,
        peak_context_tokens: peak_context,
        context_available: metrics.iter().any(|m| m.context_available),
        context_window: CONTEXT_WINDOW,
        cost_total_usd,
        buckets,
        signals,
        sessions: metrics,
    }
}

/// 0–100 "session health" score plus the top contributing signals. Higher is
/// healthier. Penalties model the bad-pattern drift the user cares about.
fn score_pattern(
    dist: &PhaseDistribution,
    tools: &ToolMix,
    grep_count: u64,
    context_fraction: f32,
) -> (u8, Vec<String>) {
    let mut penalties: Vec<(f32, String)> = Vec::new();

    // Frequent disruptions (errors / retries / corrections).
    if dist.disruption > 0.0 {
        let p = dist.disruption * 60.0;
        penalties.push((p, format!("{}% disruptions", pct(dist.disruption))));
    }

    // Exploring without productive work — thrash. Edits and test/verify runs
    // both count as productive; only read/search/bash are "exploring".
    let productive = (tools.edit + tools.test) as f32;
    let explores = (tools.read + tools.search + tools.bash) as f32;
    if productive + explores > 0.0 {
        let productive_ratio = productive / (productive + explores);
        if productive_ratio < 0.5 {
            let p = (0.5 - productive_ratio) * 40.0;
            penalties.push((p, "Exploring-heavy, few edits".to_string()));
        }
    }

    // Grep-heavy search relative to total tool use.
    let total_tools = tools.total();
    if total_tools > 0 {
        let grep_rate = grep_count as f32 / total_tools as f32;
        if grep_rate > 0.3 {
            let p = (grep_rate - 0.3) * 30.0;
            penalties.push((p, format!("{}% of tools are searches", pct(grep_rate))));
        }
    }

    // Context bloat past ~60% of the window.
    if context_fraction > 0.6 {
        let p = ((context_fraction - 0.6) / 0.4).min(1.0) * 30.0;
        penalties.push((
            p,
            format!("Context at {}% of window", pct(context_fraction)),
        ));
    }

    let total_penalty: f32 = penalties.iter().map(|(p, _)| p).sum();
    let score = (100.0 - total_penalty).clamp(0.0, 100.0) as u8;

    penalties.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let signals = penalties
        .into_iter()
        .filter(|(p, _)| *p >= 3.0)
        .take(3)
        .map(|(_, label)| label)
        .collect();

    (score, signals)
}

fn pct(fraction: f32) -> u32 {
    (fraction * 100.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a session whose events carry the given epoch-ms timestamps.
    fn session_at(ts: &[i64]) -> NormalizedSession {
        NormalizedSession {
            agent: "claude".to_string(),
            session_id: "s".to_string(),
            events: ts
                .iter()
                .map(|&t| {
                    let mut e = NormalizedEvent::new(Role::User);
                    e.ts_ms = Some(t);
                    e
                })
                .collect(),
            context_window: None,
            model: None,
        }
    }

    #[test]
    fn fraction_clamps_at_the_session_ends() {
        let s = session_at(&[0, 1000, 2000, 3000]);
        assert_eq!(active_time_fraction(&s, -100), Some(0.0)); // before first
        assert_eq!(active_time_fraction(&s, 0), Some(0.0)); // at first
        assert_eq!(active_time_fraction(&s, 3000), Some(1.0)); // at last
        assert_eq!(active_time_fraction(&s, 9999), Some(1.0)); // after last
    }

    #[test]
    fn fraction_interpolates_between_events() {
        // Even 1s gaps, no idle: active time is linear in wall-clock.
        let s = session_at(&[0, 1000, 2000, 3000]);
        assert_eq!(active_time_fraction(&s, 1500), Some(0.5)); // mid-gap
        let third = active_time_fraction(&s, 1000).unwrap();
        assert!((third - 1.0 / 3.0).abs() < 1e-6); // on an event
    }

    #[test]
    fn fraction_is_monotonic_nondecreasing() {
        let s = session_at(&[0, 1000, 2000, 3000]);
        let mut prev = 0.0;
        for t in 0..=3000 {
            let f = active_time_fraction(&s, t).unwrap();
            assert!(f >= prev - 1e-6, "regressed at {t}: {f} < {prev}");
            prev = f;
        }
    }

    #[test]
    fn idle_gaps_are_capped_like_the_axis() {
        // One huge gap (> cap): total active time collapses to the cap, but the
        // midpoint still maps to the middle of the (capped) active span.
        let s = session_at(&[0, IDLE_GAP_MS * 4]);
        assert_eq!(active_time_fraction(&s, 0), Some(0.0));
        assert_eq!(active_time_fraction(&s, IDLE_GAP_MS * 4), Some(1.0));
        assert_eq!(active_time_fraction(&s, IDLE_GAP_MS * 2), Some(0.5));
    }

    #[test]
    fn no_usable_active_time_is_none() {
        assert_eq!(active_time_fraction(&session_at(&[]), 0), None); // no events
        assert_eq!(active_time_fraction(&session_at(&[5]), 5), None); // one event
        assert_eq!(active_time_fraction(&session_at(&[5, 5]), 5), None); // zero span
    }
}
