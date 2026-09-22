//! The shapes that cross the IPC boundary.
//!
//! Every type here serializes as camelCase and mirrors a type the webview
//! already declares in `src/lib/types`. They are deliberately separate from the
//! store's own records: the database's shape is an implementation detail, and a
//! migration must never be able to change what the views receive.
//!
//! Nothing here carries a figure the shell invented. Costs come from the
//! engine's pricing table, metrics from its analysis engine, and the wording
//! around them belongs to the views — so these payloads carry values and facts,
//! never labels.

use crate::provider_usage::live::{Detection, LoginCarrier, SourceErrorDetail};
use antiburn_local::analysis::tool_catalog::{comparable_tool_name, situational_tools};
use antiburn_local::analysis::{
    ActiveSessionsSummary, EfficiencyTotals, EvidenceValue, FAST_SPEED_KEY, LoadedSource,
    ModelEvidence, ModelRun, RepeatedContextAccounting, SessionCost, SessionEvidence, SourceFormat,
    ToolDefinition, lookup_pricing,
};
use antiburn_local::insights::{
    BadgeId, BadgeStatus, DetectorId, EfficiencyReport, NotAssessedReason, ReportCatalogs,
    SessionBadge, model_family,
};
use antiburn_local::pricing::canonical_model_key;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One row of the popover's activity list.
///
/// Mirrors the fields `SessionListEntry` needs, minus the cost pill text.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    /// The agent's discovery slug (`claude-code`, `codex`, …).
    pub agent: String,
    pub session_id: String,
    /// Repository the session ran in; empty when it could not be resolved.
    pub repo: String,
    /// ISO-8601 stamp of the session's most recent transcript activity.
    pub timestamp: String,
    /// Whether meaningful session activity fell inside the engine's
    /// active-session window.
    pub is_active: bool,
    /// `cli`, `ide_desktop`, or `unknown`.
    pub surface: String,
    pub wsl_distro: Option<String>,
    pub title: Option<String>,
    /// Whether this session was branched from another local session.
    pub has_fork_parent: bool,
    /// How many local sessions were branched from this one.
    pub fork_child_count: u32,
    /// On-device cost estimate. The estimate covers every sub-agent this
    /// session launched. The value is absent when no model in the combined
    /// breakdown has a price. This field never holds a partial total.
    pub cost: Option<SessionCost>,
    /// Input, cache-creation, output, and cache-read tokens, summed across
    /// every model. The count covers every sub-agent this session launched.
    pub total_tokens: u64,
    /// Every model that contributed billable tokens.
    pub models: Vec<String>,
    /// Parent model runs come before runs used only by sub-agents.
    pub model_runs: Vec<ModelRun>,
}

/// Identity of one local session, as the views key on it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionIdentity {
    pub agent: String,
    pub session_id: String,
    pub wsl_distro: Option<String>,
}

/// One end of a local fork relation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRelation {
    pub identity: SessionIdentity,
    pub title: Option<String>,
    /// False when the related transcript is no longer on this machine.
    pub available: bool,
}

/// Direct fork relations for one session.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRelations {
    pub title: Option<String>,
    pub parent: Option<SessionRelation>,
    pub children: Vec<SessionRelation>,
}

impl SessionRelations {
    /// True when there is nothing to render, so the command can send `null`
    /// rather than an empty shape the view would still draw chrome for.
    pub fn is_empty(&self) -> bool {
        self.parent.is_none() && self.children.is_empty()
    }
}

/// One sub-agent an orchestrator launched.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentMember {
    pub agent: String,
    pub subagent_id: String,
    pub label: String,
    /// This sub-agent's own cost. `None` when the sub-agent has no metrics,
    /// or when a model in its breakdown has no price.
    pub cost: Option<SessionCost>,
    /// Billable token counts for this sub-agent alone. `None` when unknown.
    pub tokens: Option<BillableTokens>,
    /// Distinct model runs this sub-agent used. Empty when unknown.
    pub model_runs: Vec<ModelRun>,
    /// Unix seconds of this sub-agent's earliest transcript event. `None`
    /// when the child transcript could not be analyzed this pass.
    pub started_at_epoch: Option<i64>,
}

/// The sub-agent picture for one session.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationStatus {
    /// At least two sub-agents — genuine fan-out rather than one delegated task.
    pub orchestrating: bool,
    pub orchestrator_agent: String,
    pub orchestrator_session_id: String,
    pub subagent_count: u32,
    pub members: Vec<SubagentMember>,
}

/// Billable token counts, summed across one or more models.
///
/// This struct mirrors the `billable_*` fields on `SessionMetrics`. A single
/// session already carries those fields. This struct exists for a subject
/// that spans more than one transcript, such as every sub-agent combined, or
/// a parent plus every sub-agent. That subject has no single `SessionMetrics`
/// of its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillableTokens {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

/// Everything the session-analysis surface needs for one session.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAnalysis {
    /// The engine's analysis, shaped as a one-session summary. `None` when the
    /// transcript could not be read at all.
    pub summary: Option<ActiveSessionsSummary>,
    /// False when the engine has only its generic adapter for this agent, which
    /// changes the empty state from "nothing happened" to "we cannot read this".
    pub supports_analysis: bool,
    pub title: Option<String>,
    pub wsl_distro: Option<String>,
    pub is_active: bool,
    /// Cost of the parent transcript plus every sub-agent it launched.
    ///
    /// This is the session's total cost. The activity list shows this figure.
    ///
    /// The value is `None` when a model in the combined breakdown has no
    /// price. A partial total hides real cost.
    pub cost: Option<SessionCost>,
    /// Cost of the parent transcript, without any sub-agent.
    pub top_level_cost: Option<SessionCost>,
    /// Cost of every sub-agent this session launched, combined.
    ///
    /// The value is `None` when the session has no sub-agent, or when no
    /// sub-agent could be priced.
    pub subagents_cost: Option<SessionCost>,
    /// Billable token counts that back [`Self::cost`]. The count sums the
    /// parent transcript and every sub-agent.
    pub inclusive_tokens: Option<BillableTokens>,
    /// Billable token counts that back [`Self::subagents_cost`]. The count
    /// sums every sub-agent. The value is `None` when the session has no
    /// sub-agent.
    pub subagents_tokens: Option<BillableTokens>,
    /// Where the spend went: new work, carry, or rewrite. The totals sum the
    /// parent thread and every sub-agent thread, the same subject as
    /// [`Self::cost`]. `None` when the transcript could not be read.
    pub efficiency: Option<EfficiencyTotals>,
    /// Every model that contributed billable tokens. The list covers the
    /// parent transcript and every sub-agent. It matches [`Self::cost`].
    pub models: Vec<String>,
    /// Parent model runs come before runs used only by sub-agents.
    pub model_runs: Vec<ModelRun>,
    pub orchestration: Option<OrchestrationStatus>,
    pub relations: Option<SessionRelations>,
    /// Unix seconds of the earliest event in the parent or any sub-agent.
    /// `None` when the transcript could not be read.
    pub started_at_epoch: Option<i64>,
    /// The transcript's own path, for the reveal action. Absent for sessions
    /// held in a vendor database rather than a file.
    pub source_path: Option<String>,
    /// The stored absolute working directory, including the specific worktree.
    pub project_path: Option<String>,
    /// True when no published row set exists yet for this session, so every
    /// other field above is [`SessionAnalysis::unavailable`]'s placeholder
    /// rather than a real read. The worker fills the gap on its own; the
    /// view should show an indexing state, not an empty-transcript state.
    ///
    /// [`SessionAnalysis::unavailable`]: crate::analysis::SessionAnalysis::unavailable
    pub analysis_pending: bool,
    /// True when the fields above come from a published fence that a fresher
    /// pass is already queued or running behind, or whose transcript has
    /// since moved on. The data on screen is real, just not the latest —
    /// unlike [`Self::analysis_pending`], which means there is nothing to
    /// show yet. The view keeps polling and swaps in the fresh pass once the
    /// worker publishes it.
    pub analysis_stale: bool,
}

/// A protected directory the last pass declined to read, and how many working
/// directories are waiting behind it.
///
/// One entry per directory rather than per path: the operating system grants
/// access at that granularity, so it is the only granularity worth asking about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredPermissionDir {
    /// The protected directory's name, for example `Documents`.
    pub dir: String,
    /// How many known working directories sit inside it.
    pub path_count: u32,
}

/// One repository row in the sources pane.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryItem {
    /// Stable list identity — the canonical repository root.
    pub key: String,
    pub repo_name: String,
    pub full_name: String,
    pub status: String,
    pub repo_root: Option<String>,
    pub suspected_path: Option<String>,
    pub worktree_count: u32,
    pub session_count: u32,
    pub wsl_distro: Option<String>,
    pub enabled: bool,
}

/// What one agent's last pass saw.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentScanState {
    /// The agent's discovery slug.
    pub agent: String,
    /// ISO-8601 stamp of the last pass that included this agent.
    pub last_completed_at: Option<String>,
    pub sessions_seen: i64,
}

/// What a scan is doing, or last did.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    pub running: bool,
    /// Agents whose pass has finished, out of the total.
    pub completed_agents: usize,
    pub total_agents: usize,
    /// Sessions the current or last pass persisted.
    pub sessions: usize,
    /// ISO-8601 stamp of the last completed scan.
    pub finished_at: Option<String>,
    /// True when the last pass stopped because it was asked to. Distinct from
    /// [`Self::error`]: a cancelled pass did nothing wrong, it just did less.
    pub cancelled: bool,
    /// Why the last scan failed, when it did.
    pub error: Option<String>,
    /// Per-agent bookkeeping, filled when the status is read through the
    /// command rather than pushed as an event (an event fires per agent, so
    /// re-reading the table for each would be pure noise).
    pub agents: Vec<AgentScanState>,
    /// True when this pass indexed a session the list has never shown, or
    /// evicted a rejected one. A reader's list refetches on this rather than
    /// on every pass, since an unchanged pass patches rows in place instead.
    pub list_changed: bool,
    /// R5: how many session rows the last pass added or refreshed.
    /// This lets a reader detect a productive pass without `list_changed`.
    pub re_described: usize,
}

/* -------------------------------------------------------------------------
 * Local provider usage
 * ---------------------------------------------------------------------- */

/// How well the app can describe one provider's usage.
///
/// The ladder is a *capability* statement, not a quality score: it says what
/// kind of evidence produced the numbers, so a view can never dress a rough
/// figure up as a precise one.
///
/// Four states are producible from session observations. [`Live`](Self::Live)
/// remains reserved for provider-owned allowance data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderUsageState {
    /// The provider itself reported a current allowance and how much of it is
    /// gone.
    ///
    /// **Reserved.** A transcript records what was spent, never what remains,
    /// so no amount of session evidence can reach this state. Producing it
    /// needs a passive provider-owned source that has not passed review.
    #[allow(dead_code)]
    Live,
    /// Every model that contributed tokens could be priced, so the cost is a
    /// complete on-device estimate of what those tokens are worth.
    Estimated,
    /// Tokens were observed, but at least one model has no price in the
    /// bundled catalog — so the cost, when present at all, is a floor rather
    /// than a total.
    Observed,
    /// The provider is present but nothing is quantified.
    ///
    /// Explicit transcript metadata names the provider, but reports no tokens.
    Detected,
    /// Sessions were attributed to this provider, but they carry no token
    /// evidence at all — unanalyzed, or analyzed to nothing.
    Unknown,
}

/// Whether a provider's newest local evidence is recent enough to describe now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderUsageStaleness {
    /// Evidence inside the freshness threshold.
    Fresh,
    /// The newest session attributed to this provider predates the threshold,
    /// so these totals describe past work rather than current work.
    Stale,
    /// No activity timestamp at all.
    Unknown,
}

/// One provider's totals over one window.
///
/// There is deliberately no percentage, allowance, remaining balance, or reset
/// field anywhere in this type. Session evidence records what was *spent*; a
/// denominator would have to be invented, and an invented denominator is the
/// one thing this surface must never show.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageWindow {
    /// Effective input: fresh prompt tokens plus prompt-cache writes, matching
    /// the engine's own `tokens_in`.
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Prompt-cache reads, kept separate because they are billed at their own
    /// rate and are not "input the reader wrote".
    pub cache_read: u64,
    /// On-device cost estimate for the models in this window that could be
    /// priced. Absent when none could. A partial estimate is possible and is
    /// signalled by [`ProviderUsageState::Observed`], never by this field.
    pub estimated_usd: Option<f64>,
    /// True when every token-bearing model in this window has a catalog price.
    /// Empty windows are complete because they contain no unknown cost.
    pub cost_complete: bool,
    /// Sessions that contributed to this window. A session that used two
    /// providers is counted once under each.
    pub session_count: u32,
}

impl Default for ProviderUsageWindow {
    fn default() -> Self {
        Self {
            tokens_in: 0,
            tokens_out: 0,
            cache_read: 0,
            estimated_usd: None,
            cost_complete: true,
            session_count: 0,
        }
    }
}

/// The three windows every provider is summarized over.
///
/// Independent, not nested: `week` is the trailing seven calendar days and
/// `month_to_date` starts at the first of the current month, so early in a
/// month the week reaches back further than the month does.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageWindows {
    pub today: ProviderUsageWindow,
    pub week: ProviderUsageWindow,
    pub month_to_date: ProviderUsageWindow,
    /// The trailing thirty local calendar days, including today.
    pub last_30_days: ProviderUsageWindow,
}

/// One source agent's contribution to a provider account group.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAgentUsage {
    pub agent: String,
    pub windows: ProviderUsageWindows,
}

/// Everything the usage surfaces show about one provider.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    /// Canonical provider id (`anthropic`, `openai`, `unknown`, …).
    pub provider: String,
    /// Installation-scoped opaque key, or `None` when the account is unknown.
    pub account_key: Option<String>,
    pub display_name: String,
    pub state: ProviderUsageState,
    pub staleness: ProviderUsageStaleness,
    pub windows: ProviderUsageWindows,
    pub agents: Vec<ProviderAgentUsage>,
    /// ISO-8601 stamp of the newest session attributed to this provider.
    pub last_activity_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAgentDayUsage {
    pub agent: String,
    #[serde(flatten)]
    pub usage: ProviderUsageWindow,
}

/// Totals for one local calendar day, across every attributed provider.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageDay {
    #[serde(default)]
    pub agents: Vec<ProviderAgentDayUsage>,
    /// The reader's calendar date, `YYYY-MM-DD`.
    pub local_date: String,
    #[serde(flatten)]
    pub usage: ProviderUsageWindow,
}

/// Local provider usage, as one snapshot.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageSummary {
    /// Providers with at least one session in the covered span, newest first.
    /// A provider the reader has not used lately is absent rather than zeroed.
    pub providers: Vec<ProviderUsage>,
    /// Totals across every attributed provider and account.
    pub totals: ProviderUsageWindows,
    /// Totals per source agent across every attributed provider and account.
    pub agents: Vec<ProviderAgentUsage>,
    /// One entry per day of the trailing thirty, oldest first, today last.
    /// Days with no session are present and empty. The sum equals
    /// `totals.last_30_days`.
    pub days: Vec<ProviderUsageDay>,
    /// The thirty days before `days`, in the same shape. They feed no total
    /// and no provider row: they exist so a view can compare like with like.
    pub previous_days: Vec<ProviderUsageDay>,
    /// ISO-8601 stamp of the moment this snapshot was computed.
    pub generated_at: String,
}

/// The trailing 28-day pooled utilization: the tuned percentile, by nearest
/// rank, of every account-wide quota window's capped estimate that pools at
/// `now` — the rolling chart's own last point, so the two never drift.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceUtilization {
    pub utilization_percent: f64,
    /// How many weekly windows pooled into this figure.
    pub weekly_window_count: u32,
    /// How many 5-hour windows pooled into this figure.
    pub short_window_count: u32,
    /// How many model-scoped weekly windows pooled into this figure.
    pub model_window_count: u32,
}

/// One provider account's subscription chart.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceUsageAccount {
    pub provider: String,
    pub display_name: String,
    pub account_key: String,
    /// The plan the account's newest observation names, or `None` before any
    /// observation names one.
    pub plan: Option<LiveProviderPlan>,
    /// The headline figure. `None` when no window in the trailing span has
    /// an estimate.
    pub utilization: Option<AllowanceUtilization>,
    pub chart: AllowanceChart,
}

/// The three drawn layers of one account's allowance chart.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceChart {
    /// 5-hour windows, ascending by start.
    pub short_windows: Vec<AllowanceWindowPeak>,
    /// Weekly and model-scoped weekly windows, ascending by start.
    pub weekly_windows: Vec<AllowanceWindowLevels>,
    /// The rolling utilization step line, ascending by time.
    pub rolling: Vec<AllowanceRollingPoint>,
}

/// One 5-hour window's column: its span, and the highest level it reached.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceWindowPeak {
    pub starts_at_epoch: i64,
    pub resets_at_epoch: i64,
    /// The window's own `estimated_percent`, capped at 100.
    pub peak_percent: f64,
}

/// One weekly (or model-scoped weekly) window's rising area.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceWindowLevels {
    /// `"weekly"` for the account-wide window, or `"model:<slug>"` for a
    /// model-scoped one.
    pub lane: String,
    pub starts_at_epoch: i64,
    pub resets_at_epoch: i64,
    /// Hourly, cumulative, capped at 100. The first point is always zero at
    /// `starts_at_epoch`.
    pub points: Vec<AllowanceLevelPoint>,
}

/// One point of a weekly window's cumulative level.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceLevelPoint {
    pub at_epoch: i64,
    pub percent: f64,
}

/// One point of the rolling utilization step line.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceRollingPoint {
    pub at_epoch: i64,
    /// Null ends the line when no window remains in the pool.
    pub percent: Option<f64>,
}

/// The allowance chart for every account, as one snapshot.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowanceUsageSummary {
    /// One entry for each provider account this app has quota evidence for.
    pub accounts: Vec<AllowanceUsageAccount>,
    /// The trailing span the headline `utilization` pools.
    pub utilization_span_days: u32,
    /// Start of the visible 30-day chart range.
    pub range_start_epoch: i64,
    /// End of the visible chart range: `now`.
    pub range_end_epoch: i64,
    /// ISO-8601 stamp of the moment this snapshot was computed.
    pub generated_at: String,
}

/// The provider allowance represented by one session estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionLimitMetric {
    Weekly,
    FiveHour,
}

/// One session's estimated share of a provider account's learned
/// dollars-per-percent limit factor.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLimitAllocation {
    pub agent: String,
    pub session_id: String,
    pub wsl_distro: Option<String>,
    pub metric: SessionLimitMetric,
    pub provider: String,
    pub display_name: String,
    pub account_key: Option<String>,
    /// The lane the factor belongs to (`weekly` or `fiveHour`). No longer a
    /// specific provider window: the factor is a standing property of the
    /// account and lane, not of one allowance period.
    pub window_id: String,
    pub percent: f64,
    /// `learned` when the factor point came from a meter delta, `seeded`
    /// when it came from a single first-reading estimate. The percentage
    /// remains an estimate, never a bill, either way.
    pub confidence: String,
}

/// Materialized per-session allowance estimates.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLimitAllocationSummary {
    pub allocations: Vec<SessionLimitAllocation>,
    pub generated_at: String,
}

/// One quota window's derived start or end, in terms a reader can trust or
/// distrust: `"reported"` came from the provider directly, `"derived"` was
/// computed from the other boundary and the lane's nominal duration,
/// `"cadence"` was extrapolated from another observed weekly reset,
/// `"turnGap"` was inferred from a gap in local turn activity, and
/// `"truncated"` marks a reset moved earlier because the next window began
/// before the provider's stated reset for this one.
pub type QuotaBoundarySource = String;

/// A lane's currently open window, when one exists. Mirrors Rust
/// `store::provider_limit::QuotaAccountLane::current_period`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaCurrentPeriodPayload {
    pub starts_at_epoch: i64,
    pub resets_at_epoch: i64,
}

/// One lane a quota account carries.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaLanePayload {
    /// `weekly`, `fiveHour`, or `model:<slug>`.
    pub lane: String,
    /// `"Weekly"`, `"5-hour"`, or the model-scoped window's own label
    /// (Anthropic's is currently "Fable").
    pub label: String,
    /// The lane's open window, derived the same way the period resolver
    /// derives a boundary the provider did not state. `None` when every
    /// known period for the lane has already reset.
    pub current_period: Option<QuotaCurrentPeriodPayload>,
    /// The earliest reading the lane holds. A range that ends before it has
    /// no data.
    pub first_observed_epoch: i64,
}

/// One `(provider, account)` this app has observed at least one quota period
/// for.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaAccountPayload {
    pub provider: String,
    pub display_name: String,
    pub account_key: String,
    pub lanes: Vec<QuotaLanePayload>,
}

/// Response for `get_quota_accounts`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaAccountsPayload {
    pub accounts: Vec<QuotaAccountPayload>,
    pub generated_at: String,
}

/// Request for `get_quota_usage`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaUsageRequest {
    pub provider: String,
    pub account_key: String,
    pub lane: String,
    pub range_start_epoch: i64,
    pub range_end_epoch: i64,
}

/// One meter reading inside a quota period.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSamplePayload {
    pub observed_at_epoch: i64,
    pub used_percent: Option<f64>,
    pub fresh: bool,
    pub authoritative: bool,
}

/// One session's estimated dollars inside one 15-minute bucket of a quota
/// period.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaContributionPayload {
    pub agent: String,
    pub session_id: String,
    pub wsl_distro: Option<String>,
    pub bucket_start_epoch: i64,
    pub usd: f64,
    pub percent: Option<f64>,
}

/// One session's estimated total inside a quota period.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSessionTotalPayload {
    pub agent: String,
    pub session_id: String,
    pub wsl_distro: Option<String>,
    pub title: Option<String>,
    pub usd: f64,
    pub percent: Option<f64>,
}

/// Spend inside a quota period this app could not credit to any session.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaUnattributedPayload {
    pub usd: f64,
    pub percent: Option<f64>,
    pub session_count: u32,
}

/// Unattributed spend inside one 15-minute bucket of a quota period.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaBucketTotalPayload {
    pub bucket_start_epoch: i64,
    pub usd: f64,
    pub percent: Option<f64>,
}

/// One quota window, its meter readings, and the sessions estimated to have
/// contributed to it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaPeriodPayload {
    /// `None` for a period this app derived rather than observed directly:
    /// a cadence-extrapolated or turn-gap-inferred window.
    pub period_id: Option<i64>,
    pub starts_at_epoch: i64,
    pub resets_at_epoch: i64,
    pub start_source: QuotaBoundarySource,
    pub reset_source: QuotaBoundarySource,
    pub samples: Vec<QuotaSamplePayload>,
    pub contributions: Vec<QuotaContributionPayload>,
    /// Descending by `usd`.
    pub sessions: Vec<QuotaSessionTotalPayload>,
    pub unattributed: QuotaUnattributedPayload,
    /// Ascending by bucket. Holds one entry for each bucket with an
    /// unbound row, so a chart can plot unattributed spend over time
    /// instead of a single period total.
    pub unattributed_buckets: Vec<QuotaBucketTotalPayload>,
    /// The sum of every bound session's, unattributed's, and unexplained
    /// percent, so at the period's last reading it equals the meter. A
    /// closed period never exceeds 100: its factor-priced tail scales down
    /// to fit under that cap instead of overshooting a value the meter
    /// cannot reach. An open period can still overshoot, since it may
    /// gather more readings before it closes.
    pub estimated_percent: Option<f64>,
    /// One entry per meter-rise segment that had no local dollars to share
    /// it across: the whole segment's rise, at its own end (the reading
    /// that closed it), so the chart can ramp up to it. `usd` is always
    /// `0.0`; `percent` is always `Some`.
    pub unexplained_buckets: Vec<QuotaBucketTotalPayload>,
    /// The sum of every unexplained segment's percent. `None` only when the
    /// period carries no meter reading at all.
    pub unexplained_percent: Option<f64>,
}

/// Response for `get_quota_usage`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaUsagePayload {
    pub provider: String,
    pub account_key: String,
    pub lane: String,
    pub lane_label: String,
    pub range_start_epoch: i64,
    pub range_end_epoch: i64,
    pub periods: Vec<QuotaPeriodPayload>,
    pub generated_at: String,
}

/// Request for `get_session_quota`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionQuotaRequest {
    pub agent: String,
    pub session_id: String,
    pub wsl_distro: Option<String>,
}

/// The quota period one [`SessionQuotaEntryPayload`] falls in, without the
/// per-session breakdown [`QuotaPeriodPayload`] carries: a session already
/// knows which session it is.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionQuotaPeriodPayload {
    pub period_id: Option<i64>,
    pub starts_at_epoch: i64,
    pub resets_at_epoch: i64,
    pub start_source: QuotaBoundarySource,
    pub reset_source: QuotaBoundarySource,
}

/// One `(provider, lane, period)` a session's turns fell in.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionQuotaEntryPayload {
    pub provider: String,
    pub display_name: String,
    /// `None` when the session has no resolved account for this provider.
    pub account_key: Option<String>,
    /// `None` only when `confidence` is `"unbound"`: an entry with no
    /// resolved account has no lane to name either.
    pub lane: Option<String>,
    /// `None` only when `confidence` is `"unbound"`.
    pub lane_label: Option<String>,
    /// `None` only when `confidence` is `"unbound"`.
    pub period: Option<SessionQuotaPeriodPayload>,
    pub usd: f64,
    pub percent: Option<f64>,
    /// `"measured"` when every one of the session's buckets fell in a
    /// shared meter segment, `"learned"` or `"seeded"` from the factor
    /// otherwise, or `"unbound"` when the session has no resolved account
    /// for the provider its usage attributes to. Also `"measured"` when the
    /// lane has no factor point yet: `percent` then covers only the buckets
    /// inside a shared meter segment, while `usd` still covers every bucket
    /// in the window.
    pub confidence: String,
    /// The plan the account's newest observation reported, or `None` before
    /// any reading names one.
    pub plan: Option<LiveProviderPlan>,
}

/// Response for `get_session_quota`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionQuotaPayload {
    pub entries: Vec<SessionQuotaEntryPayload>,
    pub generated_at: String,
}

/// One detector rendered by All checks.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecksCategoryPayload {
    pub id: BurnCheckDetectorId,
    pub finding: u64,
    /// Agents with findings, or complete clean results when no finding exists.
    pub agents: Vec<String>,
    pub clean: u64,
    pub unavailable: u64,
    /// Hundredths of one percent, bounded to `0..=10000`.
    pub estimated_token_burn_basis_points: Option<u16>,
}

/// The bounded subset of the local report needed by the Checks feature.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecksReportPayload {
    pub evidence_settled: bool,
    /// Sessions with evidence that is queued or processing for this report window.
    pub pending_evidence: u64,
    /// Hundredths of one percent, bounded to `0..=10000`.
    pub estimated_token_burn_basis_points: Option<u16>,
    pub categories: Vec<ChecksCategoryPayload>,
}

/// One accepted detector identifier on the remediation IPC boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckDetectorId {
    SessionsOverDepth,
    ModelOverthinking,
    OverpoweredSubagents,
    UnusedMcpServers,
    UnusedBuiltInTools,
    UnusedSkills,
    OldModelUsage,
    OveruseOfFastMode,
    CacheChurn,
}

/// A reader-owned suppression for one entire burn check.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnCheckSnoozePayload {
    pub detector: BurnCheckDetectorId,
    /// This release supports the whole check. The field reserves target scope.
    pub scope: BurnCheckSnoozeScope,
    /// Milliseconds since the Unix epoch. `None` means the reader chose forever.
    pub until: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckSnoozeScope {
    Check,
}

impl From<BurnCheckDetectorId> for DetectorId {
    fn from(value: BurnCheckDetectorId) -> Self {
        match value {
            BurnCheckDetectorId::SessionsOverDepth => Self::SessionsOverDepth,
            BurnCheckDetectorId::ModelOverthinking => Self::ModelOverthinking,
            BurnCheckDetectorId::OverpoweredSubagents => Self::OverpoweredSubagents,
            BurnCheckDetectorId::UnusedMcpServers => Self::UnusedMcpServers,
            BurnCheckDetectorId::UnusedBuiltInTools => Self::UnusedBuiltInTools,
            BurnCheckDetectorId::UnusedSkills => Self::UnusedSkills,
            BurnCheckDetectorId::OldModelUsage => Self::OldModelUsage,
            BurnCheckDetectorId::OveruseOfFastMode => Self::OveruseOfFastMode,
            BurnCheckDetectorId::CacheChurn => Self::CacheChurn,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckSourceFormat {
    ClaudeJsonl,
    CodexRolloutJsonl,
    OpenCodeJsonl,
    OpenCodeSqliteV2,
    PiV3Jsonl,
    CursorJsonl,
    CursorCliAgentJsonl,
    CursorCliStoreDb,
    CursorChatStoreDb,
    CursorIdeComposer,
    CursorLegacyChatJson,
    AntigravityJson,
    AntigravityBrainJsonl,
    AntigravityCascadeJson,
    AntigravityWorkspaceChatJson,
    AntigravitySqlite,
    CopilotCliJsonl,
    CopilotIdeChatJson,
    ClineSessionJson,
    ClineMessagesContractV1,
    KiroSessionJson,
    KiroChat,
    KiroCliV2Bundle,
    KiroCliV3Bundle,
    KiroChatSaveExport,
    AmpThreadJson,
    AmpFileChanges,
    WindsurfWorkspaceJson,
    WindsurfMirrorJson,
    WindsurfCascadeProtobuf,
    DevinLocalSqlite,
    Uncharacterized,
}

impl From<SourceFormat> for BurnCheckSourceFormat {
    fn from(value: SourceFormat) -> Self {
        match value {
            SourceFormat::ClaudeJsonl => Self::ClaudeJsonl,
            SourceFormat::CodexRolloutJsonl => Self::CodexRolloutJsonl,
            SourceFormat::OpenCodeJsonl => Self::OpenCodeJsonl,
            SourceFormat::OpenCodeSqliteV2 => Self::OpenCodeSqliteV2,
            SourceFormat::PiV3Jsonl => Self::PiV3Jsonl,
            SourceFormat::CursorJsonl => Self::CursorJsonl,
            SourceFormat::CursorCliAgentJsonl => Self::CursorCliAgentJsonl,
            SourceFormat::CursorCliStoreDb => Self::CursorCliStoreDb,
            SourceFormat::CursorChatStoreDb => Self::CursorChatStoreDb,
            SourceFormat::CursorIdeComposer => Self::CursorIdeComposer,
            SourceFormat::CursorLegacyChatJson => Self::CursorLegacyChatJson,
            SourceFormat::AntigravityJson => Self::AntigravityJson,
            SourceFormat::AntigravityBrainJsonl => Self::AntigravityBrainJsonl,
            SourceFormat::AntigravityCascadeJson => Self::AntigravityCascadeJson,
            SourceFormat::AntigravityWorkspaceChatJson => Self::AntigravityWorkspaceChatJson,
            SourceFormat::AntigravitySqlite => Self::AntigravitySqlite,
            SourceFormat::CopilotCliJsonl => Self::CopilotCliJsonl,
            SourceFormat::CopilotIdeChatJson => Self::CopilotIdeChatJson,
            SourceFormat::ClineSessionJson => Self::ClineSessionJson,
            SourceFormat::ClineMessagesContractV1 => Self::ClineMessagesContractV1,
            SourceFormat::KiroSessionJson => Self::KiroSessionJson,
            SourceFormat::KiroChat => Self::KiroChat,
            SourceFormat::KiroCliV2Bundle => Self::KiroCliV2Bundle,
            SourceFormat::KiroCliV3Bundle => Self::KiroCliV3Bundle,
            SourceFormat::KiroChatSaveExport => Self::KiroChatSaveExport,
            SourceFormat::AmpThreadJson => Self::AmpThreadJson,
            SourceFormat::AmpFileChanges => Self::AmpFileChanges,
            SourceFormat::WindsurfWorkspaceJson => Self::WindsurfWorkspaceJson,
            SourceFormat::WindsurfMirrorJson => Self::WindsurfMirrorJson,
            SourceFormat::WindsurfCascadeProtobuf => Self::WindsurfCascadeProtobuf,
            SourceFormat::DevinLocalSqlite => Self::DevinLocalSqlite,
            SourceFormat::Uncharacterized => Self::Uncharacterized,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnCheckFindingPayload {
    pub detector: BurnCheckDetectorId,
    pub agent: String,
    pub source_format: BurnCheckSourceFormat,
    pub observation: String,
    pub labels: Vec<String>,
    pub omitted: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckResourceKind {
    Session,
    Reasoning,
    Worker,
    McpServer,
    BuiltInTool,
    Skill,
    Model,
    Speed,
    Cache,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckScopeKind {
    Global,
    Project,
    Session,
    Worker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckQuantityUnit {
    Tokens,
    Turns,
    Resources,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckEstimateMethod {
    RepeatedContextAboveDepthCap,
    AssumedOutputReduction,
    WorkerModelPriceDifference,
    McpDefinitionExposure,
    BuiltInDefinitionReplication,
    InjectedSkillDocument,
    OldModelPriceDifference,
    FastTierPricePremium,
    CacheRehydrationPriceDifference,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnCheckEstimatedValuePayload {
    pub value: f64,
    pub unit: BurnCheckSavingsUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckSavingsUnit {
    LiteralInputTokens,
    AssumedOutputTokens,
    CacheClassTokens,
    ApiEquivalentUsd,
    Improvements,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckVerificationLimit {
    FreshEvidenceFromSameSourceAndTarget,
    ExactPositiveControlRequired,
    CurrentEvidenceCannotProveFix,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnCheckDisplayFactsPayload {
    pub resource_kind: BurnCheckResourceKind,
    pub resource_identity: Option<String>,
    pub current_value: Option<String>,
    pub replacement_value: Option<String>,
    pub scope_kind: BurnCheckScopeKind,
    pub quantity: Option<u64>,
    pub quantity_unit: Option<BurnCheckQuantityUnit>,
    pub observation_count: u64,
    pub first_observed_at_ms: i64,
    pub last_observed_at_ms: i64,
    pub estimate_method: Option<BurnCheckEstimateMethod>,
    pub estimated_opportunity: Option<BurnCheckEstimatedValuePayload>,
    pub estimated_token_burn_basis_points: Option<u16>,
    pub verification_limit: BurnCheckVerificationLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AutoFixAvailabilityPayload {
    Available,
    Unavailable { reason: AutoFixUnavailableReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PromptFixAvailabilityPayload {
    Available,
    Unavailable { reason: PromptFixUnavailableReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoFixUnavailableReason {
    UnsupportedOrUnprovenTarget,
    ActiveWatch,
    SafetyCheckFailed,
    TargetNotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckCoverageLimit {
    CurrentPublishedEvidenceOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckWatchLifecycle {
    Reserved,
    Writing,
    RecoveryNeeded,
    WaitingForPromptUse,
    Watching,
    Fixed,
    Recurred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckVerificationReason {
    MissingPostBoundaryEvidence,
    WriteOutcomeUnknown,
    VerificationUnavailable,
    UnsupportedAgent,
    HomeUnavailable,
    PhysicalTargetChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BurnCheckVerificationPayload {
    Reserved,
    Watching {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<BurnCheckVerificationReason>,
        #[serde(skip_serializing_if = "Option::is_none")]
        method_revision: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        evidence_revision: Option<String>,
    },
    Fixed {
        method_revision: u32,
        evidence_revision: String,
    },
    StillUnresolved {
        method_revision: u32,
        evidence_revision: String,
    },
    Recurred {
        method_revision: u32,
        evidence_revision: String,
    },
    RecoveryNeeded {
        reason: BurnCheckVerificationReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        checked_at_epoch: Option<i64>,
    },
    VerificationUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckSavingsMethod {
    OldModelPriceDifference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckSavingsUnknownReason {
    MissingRates,
    MissingEvidence,
    MissingRevision,
    ArithmeticOverflow,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BurnCheckSavingsPayload {
    Pending {
        #[serde(skip_serializing_if = "Option::is_none")]
        method_revision: Option<u32>,
    },
    Unavailable,
    Unknown {
        reason: BurnCheckSavingsUnknownReason,
        method_revision: u32,
    },
    Known {
        method: BurnCheckSavingsMethod,
        method_revision: u32,
        pricing_revision: String,
        api_equivalent_cost_avoided_usd: f64,
        measured_through_ms: i64,
        recurrence_ms: Option<i64>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnCheckWatchPayload {
    pub watch_id: String,
    pub origin: AggregateWinOrigin,
    pub lifecycle: BurnCheckWatchLifecycle,
    pub verification: BurnCheckVerificationPayload,
    pub savings: BurnCheckSavingsPayload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnCheckTargetPayload {
    pub finding_id: String,
    pub action_id: String,
    pub finding: BurnCheckFindingPayload,
    pub display: BurnCheckDisplayFactsPayload,
    pub occurrence_count: u64,
    pub affected_session_count: Option<u64>,
    pub project_name: Option<String>,
    pub project_location: Option<String>,
    /// Full local directory for explicit folder actions, excluded from analytics.
    pub project_path: Option<String>,
    /// Local configuration file for a reviewed remediation target, excluded from analytics.
    pub config_file: Option<String>,
    pub auto_fix: AutoFixAvailabilityPayload,
    pub prompt_fix: PromptFixAvailabilityPayload,
    pub watch: Option<BurnCheckWatchPayload>,
    pub coverage_limits: Vec<BurnCheckCoverageLimit>,
    pub samples: Vec<BurnCheckSamplePayload>,
    pub expires_at_epoch: i64,
}

/// Privacy-safe metadata and an opaque route to one local sample session.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnCheckSamplePayload {
    pub navigation_handle: String,
    pub title: String,
    pub agent: String,
    pub surface: BurnCheckSampleSurface,
    pub observed_at_ms: i64,
    pub repo: String,
    pub timestamp: String,
    pub is_active: bool,
    pub has_fork_parent: bool,
    pub fork_child_count: u32,
    pub cost: Option<SessionCost>,
    pub models: Vec<String>,
    pub model_runs: Vec<ModelRun>,
    pub hygiene: SessionHygienePayload,
}

/// Safe source category for a sample session display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BurnCheckSampleSurface {
    Cli,
    IdeDesktop,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum OpenBurnCheckSampleOutcome {
    Opened,
    Deleted,
    Expired,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnCheckTargetListPayload {
    pub targets: Vec<BurnCheckTargetPayload>,
    pub samples: Vec<BurnCheckSamplePayload>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BurnCheckRemediationOutcomePayload {
    Failed,
    Passed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnCheckRemediationAttemptPayload {
    pub detector: BurnCheckDetectorId,
    pub watch_id: String,
    pub display: BurnCheckDisplayFactsPayload,
    pub origin: AggregateWinOrigin,
    pub lifecycle: BurnCheckWatchLifecycle,
    pub outcome: BurnCheckRemediationOutcomePayload,
    pub verification: BurnCheckVerificationPayload,
    pub savings: BurnCheckSavingsPayload,
    pub effective_boundary_ms: Option<i64>,
    pub verified_boundary_ms: Option<i64>,
    pub recurred_boundary_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnCheckRemediationProgressPayload {
    pub attempts: Vec<BurnCheckRemediationAttemptPayload>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PrepareAutoFixBurnCheckTargetOutcome {
    ReviewReady { review: AutoFixReviewPayload },
    Stale,
    Expired,
    Conflict,
    Unavailable { reason: AutoFixUnavailableReason },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoFixReviewPayload {
    pub prepared_operation_id: String,
    pub expires_at_epoch: i64,
    pub agent: String,
    pub scope: BurnCheckScopeKind,
    pub setting: AutoFixSetting,
    pub config_file: String,
    pub selector_label: String,
    pub current_value: String,
    pub proposed_value: String,
    pub behavior_override_warning: bool,
    pub effect: AutoFixEffect,
    pub side_effect: AutoFixSideEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoFixSetting {
    Model,
    Reasoning,
    Compaction,
    SubagentModel,
    McpServer,
    BuiltInTool,
    Skill,
    FastMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoFixEffect {
    ModelSelection,
    ReasoningEffort,
    SessionCompaction,
    WorkerModelSelection,
    McpAvailability,
    ToolAvailability,
    SkillAvailability,
    ServiceTierSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoFixSideEffect {
    ModelBehaviorMayChange,
    ResponsesMayUseLessReasoning,
    EarlierSessionSummarization,
    WorkerBehaviorMayChange,
    ServerWillNotBeAvailable,
    ToolWillNotBeAvailable,
    SkillWillNotBeAvailable,
    ResponsesMayTakeLonger,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ApplyPreparedBurnCheckOperationOutcome {
    AppliedAwaitingVerification { watch_id: String },
    Applied,
    RecoveryNeeded { watch_id: String },
    Stale,
    Expired,
    Conflict,
    Unavailable { reason: AutoFixUnavailableReason },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregateWinsPayload {
    pub wins: Vec<AggregateWinPayload>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregateWinPayload {
    pub finding_id: String,
    pub detector: BurnCheckDetectorId,
    pub origin: AggregateWinOrigin,
    pub display: BurnCheckDisplayFactsPayload,
    pub savings: AggregateSavingsPayload,
    pub starts_at_ms: i64,
    pub ends_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AggregateWinOrigin {
    Passive,
    Action,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregateSavingsPayload {
    pub token_savings: Option<u64>,
    pub api_equivalent_cost_avoided_usd: Option<f64>,
    pub improvement_count: Option<u64>,
    pub method: Option<BurnCheckEstimateMethod>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PromptFixUnavailableReason {
    TargetNotFound,
    PromptSizeLimit,
    EssentialIdentityUnavailable,
    ProtectedBuiltInTool,
    DeferredAgent,
    UnsupportedSourceFormat,
    CheckUnsupportedForAgent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CopyPromptFixBurnCheckTargetOutcome {
    PromptReady {
        prompt: String,
        watch: Option<BurnCheckWatchPayload>,
    },
    Stale,
    Expired,
    Unavailable {
        reason: PromptFixUnavailableReason,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CopyPromptFixBurnCheckOutcome {
    PromptReady { prompt: String },
    Unavailable,
}

/// One session identity requested for a hygiene badge reduction.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHygieneRequest {
    pub agent: String,
    pub session_id: String,
    pub wsl_distro: Option<String>,
}

/// One session hygiene status on the IPC boundary.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionHygieneStatus {
    Finding,
    Clean,
    NotAssessed,
}

/// The stored facts that caused one session hygiene finding.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionHygieneFindingEvidencePayload {
    SessionOverdepth {
        max_request_context_tokens: u64,
        depth_cap_tokens: u64,
    },
    ModelOverthinking {
        tiers: Vec<HygieneEffortTierPayload>,
    },
    OverpoweredSubagents {
        main_models: Vec<String>,
        delegated_models: Vec<String>,
    },
    ObsoleteModel {
        models: Vec<HygieneObsoleteModelPayload>,
    },
    FastModeOveruse {
        delegated_turns: u64,
    },
    ExcessCacheRehydration {
        repeated_tokens: u64,
        paid_tokens: u64,
        threshold_multiple: f64,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HygieneEffortTierPayload {
    pub tier: String,
    pub main_loop_turns: u64,
    pub delegated_turns: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HygieneObsoleteModelPayload {
    pub model: String,
    pub replacement: String,
}

/// One session hygiene badge with the facts behind a finding.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHygieneBadgePayload {
    pub id: &'static str,
    pub status: SessionHygieneStatus,
    pub not_assessed_reason: Option<&'static str>,
    /// Which vendor billing mechanism backs an `excessCacheRehydration`
    /// verdict. Absent for every other badge and for old evidence with no
    /// `repeated_context` marker.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounting: Option<&'static str>,
    /// Present only when stored evidence explains a finding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finding_evidence: Option<SessionHygieneFindingEvidencePayload>,
}

/// The session badge set and its stored evidence state.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHygienePayload {
    pub badges: Vec<SessionHygieneBadgePayload>,
    pub evidence_state: &'static str,
    /// Priced idle context for this one session, present only when
    /// evidence backs it. Informational: it carries no verdict.
    pub unused_resources: Option<SessionUnusedResourcesPayload>,
}

/// Resources that sat in every request's context this session and were
/// never called, with what the session paid to replay each one.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUnusedResourcesPayload {
    pub mcp_servers: Vec<UnusedResourcePayload>,
    pub built_in_tools: Vec<UnusedResourcePayload>,
    pub skills: Vec<UnusedResourcePayload>,
}

/// One unused resource, with its priced replication cost. `cost_usd` is
/// absent when no observed model resolves in the live pricing table.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnusedResourcePayload {
    pub name: String,
    pub cost_usd: Option<f64>,
}

/// The aggregate hygiene numbers for the sessions in the activity window.
///
/// The onboarding Ready step reads this: a progress state while
/// `settled_sessions` trails `total_sessions`, a results card after.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HygieneSummaryPayload {
    /// Sessions in the window, after the disabled-agent display filter.
    pub total_sessions: u64,
    /// Sessions whose analysis reached a terminal state.
    pub settled_sessions: u64,
    /// Sessions with current ready evidence, so the checks ran.
    pub analyzed_sessions: u64,
    /// Analyzed sessions with at least one finding.
    pub failing_sessions: u64,
    /// Badge id of the most frequent finding, when any session fails.
    pub most_common_finding: Option<&'static str>,
}

pub(crate) fn badge_id_str(id: BadgeId) -> &'static str {
    match id {
        BadgeId::SessionOverdepth => "sessionOverdepth",
        BadgeId::ModelOverthinking => "modelOverthinking",
        BadgeId::OverpoweredSubagents => "overpoweredSubagents",
        BadgeId::ObsoleteModel => "obsoleteModel",
        BadgeId::FastModeOveruse => "fastModeOveruse",
        BadgeId::ExcessCacheRehydration => "excessCacheRehydration",
    }
}

impl SessionHygieneBadgePayload {
    fn from_badge(
        badge: SessionBadge,
        accounting: Option<&'static str>,
        finding_evidence: Option<SessionHygieneFindingEvidencePayload>,
    ) -> Self {
        let (status, not_assessed_reason) = match badge.status {
            BadgeStatus::Finding => (SessionHygieneStatus::Finding, None),
            BadgeStatus::Clean => (SessionHygieneStatus::Clean, None),
            BadgeStatus::NotAssessed(reason) => (
                SessionHygieneStatus::NotAssessed,
                Some(not_assessed_reason_str(reason)),
            ),
        };
        // Only `ExcessCacheRehydration` carries repeated-context
        // accounting; every other badge's payload leaves it absent.
        let accounting = if badge.id == BadgeId::ExcessCacheRehydration {
            accounting
        } else {
            None
        };
        Self {
            id: badge_id_str(badge.id),
            status,
            not_assessed_reason,
            accounting,
            finding_evidence,
        }
    }
}

fn observed<T>(evidence: &EvidenceValue<T>) -> Option<&T> {
    match evidence {
        EvidenceValue::Complete(observed) | EvidenceValue::Partial { observed, .. } => {
            Some(observed)
        }
        EvidenceValue::Unsupported => None,
    }
}

fn model_is_premium(model: &str, catalogs: &ReportCatalogs) -> bool {
    let Some(policy) = catalogs.families.get(&model_family(model)) else {
        return false;
    };
    policy.premium.reviewed && policy.premium.is_premium(&canonical_model_key(model))
}

fn finding_evidence(
    id: BadgeId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
) -> Option<SessionHygieneFindingEvidencePayload> {
    match id {
        BadgeId::SessionOverdepth => {
            let context = observed(&evidence.context)?;
            Some(SessionHygieneFindingEvidencePayload::SessionOverdepth {
                max_request_context_tokens: context.max_request_context_tokens,
                depth_cap_tokens: catalogs.depth_cap_tokens,
            })
        }
        BadgeId::ModelOverthinking => {
            let models = observed(&evidence.models)?;
            let families = models
                .by_model
                .keys()
                .map(|model| model_family(model))
                .collect::<BTreeSet<_>>();
            let tiers = models
                .effort_tiers
                .iter()
                .filter_map(|(tier, turns)| {
                    let normalized = tier.trim().to_lowercase();
                    let above_cap = families.iter().any(|family| {
                        catalogs
                            .families
                            .get(family)
                            .is_some_and(|policy| policy.effort.above_cap.contains(&normalized))
                    });
                    above_cap.then(|| HygieneEffortTierPayload {
                        tier: tier.clone(),
                        main_loop_turns: turns.main_loop,
                        delegated_turns: turns.delegated,
                    })
                })
                .collect();
            Some(SessionHygieneFindingEvidencePayload::ModelOverthinking { tiers })
        }
        BadgeId::OverpoweredSubagents => {
            let subagents = observed(&evidence.subagents)?;
            let models = observed(&evidence.models);
            let main_models = models
                .and_then(|models| models.dominant_main_model.as_ref())
                .filter(|model| model_is_premium(model, catalogs))
                .cloned()
                .into_iter()
                .chain(
                    subagents
                        .children
                        .iter()
                        .filter_map(|child| child.parent_model.as_ref())
                        .filter(|model| model_is_premium(model, catalogs))
                        .cloned(),
                )
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let delegated_models = subagents
                .delegated_models
                .iter()
                .filter(|model| model_is_premium(model, catalogs))
                .cloned()
                .collect();
            Some(SessionHygieneFindingEvidencePayload::OverpoweredSubagents {
                main_models,
                delegated_models,
            })
        }
        BadgeId::ObsoleteModel => {
            let models = observed(&evidence.models)?;
            let models = models
                .by_model
                .iter()
                .filter_map(|(model, tokens)| {
                    let replacement = catalogs.model_replacements.lookup(model)?;
                    (tokens.turns > 0 && tokens.last_ts_ms >= replacement.available_since_ts_ms)
                        .then(|| HygieneObsoleteModelPayload {
                            model: model.clone(),
                            replacement: replacement.replacement.clone(),
                        })
                })
                .collect();
            Some(SessionHygieneFindingEvidencePayload::ObsoleteModel { models })
        }
        BadgeId::FastModeOveruse => {
            let models = observed(&evidence.models)?;
            let delegated_turns = models
                .fast_modes
                .iter()
                .filter(|(label, turns)| {
                    label.trim().eq_ignore_ascii_case(FAST_SPEED_KEY)
                        && turns.delegated >= catalogs.fast_mode_delegated_turns_threshold
                })
                .map(|(_, turns)| turns.delegated)
                .sum();
            Some(SessionHygieneFindingEvidencePayload::FastModeOveruse { delegated_turns })
        }
        BadgeId::ExcessCacheRehydration => {
            let cache = observed(&evidence.cache)?;
            let repeated_context = observed(&cache.repeated_context)?;
            let models = observed(&evidence.models)?;
            let model = models
                .dominant_main_model
                .as_ref()
                .or_else(|| models.by_model.keys().next())?;
            let threshold_multiple = catalogs
                .families
                .get(&model_family(model))?
                .cache_overpay_multiple_threshold;
            Some(
                SessionHygieneFindingEvidencePayload::ExcessCacheRehydration {
                    repeated_tokens: repeated_context.repeated_tokens,
                    paid_tokens: repeated_context.paid_tokens,
                    threshold_multiple,
                },
            )
        }
    }
}

/// Sums one resource's replication cost across every observed model:
/// `token_count * turns * cache_read_cost_per_token`, per model in
/// `models.by_model`. `None` when no observed model resolves in the
/// live pricing table, even though the resource still names itself.
fn unused_resource_cost_usd(token_count: u64, models: Option<&ModelEvidence>) -> Option<f64> {
    let models = models?;
    let mut total_usd = 0.0;
    let mut priced_any = false;
    for (model, tokens) in &models.by_model {
        let Some(pricing) = lookup_pricing(model) else {
            continue;
        };
        total_usd += token_count as f64 * tokens.turns as f64 * pricing.cache_read_cost_per_token;
        priced_any = true;
    }
    priced_any.then_some(total_usd)
}

/// Builds one payload entry per injected, never-invoked MCP server or
/// skill, matching `unused_mcp_servers`/`unused_skills`'s own `evaluate`.
fn unused_loaded_source_payloads(
    sources: &BTreeMap<String, LoadedSource>,
    models: Option<&ModelEvidence>,
) -> Vec<UnusedResourcePayload> {
    sources
        .iter()
        .filter(|(_, source)| source.injected && !source.invoked)
        .map(|(name, source)| UnusedResourcePayload {
            name: name.clone(),
            cost_usd: source
                .token_count
                .and_then(|tokens| unused_resource_cost_usd(tokens, models)),
        })
        .collect()
}

/// Builds one payload entry per unused built-in tool definition, matching
/// `unused_built_in_tools::has_unused_definition`: a real, non-deferred,
/// never-invoked, non-situational definition.
fn unused_built_in_tool_payloads(
    agent: &str,
    definitions: &BTreeMap<String, ToolDefinition>,
    models: Option<&ModelEvidence>,
) -> Vec<UnusedResourcePayload> {
    let situational: Vec<String> = situational_tools(agent)
        .iter()
        .map(|name| comparable_tool_name(name))
        .collect();
    definitions
        .iter()
        .filter(|(name, definition)| {
            definition.tokens > 0
                && !definition.deferred
                && !definition.invoked
                && !situational.contains(&comparable_tool_name(name))
        })
        .map(|(name, definition)| UnusedResourcePayload {
            name: name.clone(),
            cost_usd: unused_resource_cost_usd(u64::from(definition.tokens), models),
        })
        .collect()
}

/// Builds the session's priced idle-context section from evidence: every
/// injected-but-unused MCP server, built-in tool, and skill.
fn session_unused_resources(evidence: &SessionEvidence) -> Option<SessionUnusedResourcesPayload> {
    let sources = observed(&evidence.context_sources)?;
    let models = observed(&evidence.models);
    let built_in_tools = match observed(&sources.tool_definitions) {
        Some(definitions) => {
            unused_built_in_tool_payloads(&evidence.identity.agent, definitions, models)
        }
        None => Vec::new(),
    };
    Some(SessionUnusedResourcesPayload {
        mcp_servers: unused_loaded_source_payloads(&sources.mcp_servers, models),
        built_in_tools,
        skills: unused_loaded_source_payloads(&sources.skills, models),
    })
}

/// Reads the accounting `Cache Churn` used for this session's
/// `repeated_context`, or `None` when neither cache-write nor
/// uncached-input accounting applies.
fn repeated_context_accounting_str(evidence: &SessionEvidence) -> Option<&'static str> {
    let cache = match &evidence.cache {
        EvidenceValue::Complete(cache)
        | EvidenceValue::Partial {
            observed: cache, ..
        } => cache,
        EvidenceValue::Unsupported => return None,
    };
    let repeated_context = match &cache.repeated_context {
        EvidenceValue::Complete(observed) | EvidenceValue::Partial { observed, .. } => observed,
        EvidenceValue::Unsupported => return None,
    };
    Some(match repeated_context.accounting {
        RepeatedContextAccounting::CacheWrite => "cacheWrite",
        RepeatedContextAccounting::UncachedInput => "uncachedInput",
    })
}

impl SessionHygienePayload {
    pub fn from_badges(
        badges: [SessionBadge; 6],
        accounting: Option<&'static str>,
        evidence_state: &'static str,
    ) -> Self {
        Self {
            badges: badges
                .into_iter()
                .map(|badge| SessionHygieneBadgePayload::from_badge(badge, accounting, None))
                .collect(),
            evidence_state,
            unused_resources: None,
        }
    }

    pub fn for_evidence(
        badges: [SessionBadge; 6],
        evidence: &SessionEvidence,
        catalogs: &ReportCatalogs,
        evidence_state: &'static str,
    ) -> Self {
        let accounting = repeated_context_accounting_str(evidence);
        Self {
            badges: badges
                .into_iter()
                .map(|badge| {
                    let details = if matches!(badge.status, BadgeStatus::Finding) {
                        finding_evidence(badge.id, evidence, catalogs)
                    } else {
                        None
                    };
                    SessionHygieneBadgePayload::from_badge(badge, accounting, details)
                })
                .collect(),
            evidence_state,
            unused_resources: session_unused_resources(evidence),
        }
    }

    pub fn not_assessed(evidence_state: &'static str, reason: NotAssessedReason) -> Self {
        Self::from_badges(
            BadgeId::ALL.map(|id| SessionBadge {
                id,
                status: BadgeStatus::NotAssessed(reason),
            }),
            None,
            evidence_state,
        )
    }
}

impl From<DetectorId> for BurnCheckDetectorId {
    fn from(value: DetectorId) -> Self {
        match value {
            DetectorId::SessionsOverDepth => Self::SessionsOverDepth,
            DetectorId::ModelOverthinking => Self::ModelOverthinking,
            DetectorId::OverpoweredSubagents => Self::OverpoweredSubagents,
            DetectorId::UnusedMcpServers => Self::UnusedMcpServers,
            DetectorId::UnusedBuiltInTools => Self::UnusedBuiltInTools,
            DetectorId::UnusedSkills => Self::UnusedSkills,
            DetectorId::OldModelUsage => Self::OldModelUsage,
            DetectorId::OveruseOfFastMode => Self::OveruseOfFastMode,
            DetectorId::CacheChurn => Self::CacheChurn,
        }
    }
}

impl From<crate::remediation::AutoFixUnavailableReason> for AutoFixUnavailableReason {
    fn from(value: crate::remediation::AutoFixUnavailableReason) -> Self {
        match value {
            crate::remediation::AutoFixUnavailableReason::UnsupportedOrUnprovenTarget => {
                Self::UnsupportedOrUnprovenTarget
            }
            crate::remediation::AutoFixUnavailableReason::ActiveWatch => Self::ActiveWatch,
            crate::remediation::AutoFixUnavailableReason::SafetyCheckFailed => {
                Self::SafetyCheckFailed
            }
        }
    }
}

impl From<antiburn_local::remediation::RemediationUnavailableReason>
    for PromptFixUnavailableReason
{
    fn from(value: antiburn_local::remediation::RemediationUnavailableReason) -> Self {
        use antiburn_local::remediation::RemediationUnavailableReason;
        match value {
            RemediationUnavailableReason::PromptSizeLimit => Self::PromptSizeLimit,
            RemediationUnavailableReason::EssentialIdentityUnavailable => {
                Self::EssentialIdentityUnavailable
            }
            RemediationUnavailableReason::ProtectedBuiltInTool => Self::ProtectedBuiltInTool,
            RemediationUnavailableReason::DeferredAgent => Self::DeferredAgent,
            RemediationUnavailableReason::UnsupportedSourceFormat => Self::UnsupportedSourceFormat,
            RemediationUnavailableReason::CheckUnsupportedForAgent => {
                Self::CheckUnsupportedForAgent
            }
        }
    }
}

impl From<crate::remediation::BurnCheckScopeKind> for BurnCheckScopeKind {
    fn from(value: crate::remediation::BurnCheckScopeKind) -> Self {
        match value {
            crate::remediation::BurnCheckScopeKind::Global => Self::Global,
            crate::remediation::BurnCheckScopeKind::Project => Self::Project,
            crate::remediation::BurnCheckScopeKind::Session => Self::Session,
            crate::remediation::BurnCheckScopeKind::Worker => Self::Worker,
        }
    }
}

impl From<crate::remediation::BurnCheckEstimateMethod> for BurnCheckEstimateMethod {
    fn from(value: crate::remediation::BurnCheckEstimateMethod) -> Self {
        match value {
            crate::remediation::BurnCheckEstimateMethod::RepeatedContextAboveDepthCap => {
                Self::RepeatedContextAboveDepthCap
            }
            crate::remediation::BurnCheckEstimateMethod::AssumedOutputReduction => {
                Self::AssumedOutputReduction
            }
            crate::remediation::BurnCheckEstimateMethod::WorkerModelPriceDifference => {
                Self::WorkerModelPriceDifference
            }
            crate::remediation::BurnCheckEstimateMethod::McpDefinitionExposure => {
                Self::McpDefinitionExposure
            }
            crate::remediation::BurnCheckEstimateMethod::BuiltInDefinitionReplication => {
                Self::BuiltInDefinitionReplication
            }
            crate::remediation::BurnCheckEstimateMethod::InjectedSkillDocument => {
                Self::InjectedSkillDocument
            }
            crate::remediation::BurnCheckEstimateMethod::OldModelPriceDifference => {
                Self::OldModelPriceDifference
            }
            crate::remediation::BurnCheckEstimateMethod::FastTierPricePremium => {
                Self::FastTierPricePremium
            }
            crate::remediation::BurnCheckEstimateMethod::CacheRehydrationPriceDifference => {
                Self::CacheRehydrationPriceDifference
            }
        }
    }
}

impl From<crate::remediation::BurnCheckDisplayFacts> for BurnCheckDisplayFactsPayload {
    fn from(value: crate::remediation::BurnCheckDisplayFacts) -> Self {
        use crate::remediation::{
            BurnCheckQuantityUnit as Quantity, BurnCheckResourceKind as Resource,
            BurnCheckVerificationLimit as Limit,
        };
        Self {
            resource_kind: match value.resource_kind {
                Resource::Session => BurnCheckResourceKind::Session,
                Resource::Reasoning => BurnCheckResourceKind::Reasoning,
                Resource::Worker => BurnCheckResourceKind::Worker,
                Resource::McpServer => BurnCheckResourceKind::McpServer,
                Resource::BuiltInTool => BurnCheckResourceKind::BuiltInTool,
                Resource::Skill => BurnCheckResourceKind::Skill,
                Resource::Model => BurnCheckResourceKind::Model,
                Resource::Speed => BurnCheckResourceKind::Speed,
                Resource::Cache => BurnCheckResourceKind::Cache,
            },
            resource_identity: value.resource_identity,
            current_value: value.current_value,
            replacement_value: value.replacement_value,
            scope_kind: value.scope_kind.into(),
            quantity: value.quantity,
            quantity_unit: value.quantity_unit.map(|unit| match unit {
                Quantity::Tokens => BurnCheckQuantityUnit::Tokens,
                Quantity::Turns => BurnCheckQuantityUnit::Turns,
                Quantity::Resources => BurnCheckQuantityUnit::Resources,
            }),
            observation_count: value.observation_count,
            first_observed_at_ms: value.first_observed_at_ms,
            last_observed_at_ms: value.last_observed_at_ms,
            estimate_method: value.estimate_method.map(Into::into),
            estimated_opportunity: value.estimated_opportunity.map(|estimate| {
                BurnCheckEstimatedValuePayload {
                    value: estimate.value,
                    unit: match estimate.unit {
                        antiburn_local::remediation::SavingsUnit::LiteralInputTokens => {
                            BurnCheckSavingsUnit::LiteralInputTokens
                        }
                        antiburn_local::remediation::SavingsUnit::AssumedOutputTokens => {
                            BurnCheckSavingsUnit::AssumedOutputTokens
                        }
                        antiburn_local::remediation::SavingsUnit::CacheClassTokens => {
                            BurnCheckSavingsUnit::CacheClassTokens
                        }
                        antiburn_local::remediation::SavingsUnit::ApiEquivalentUsd => {
                            BurnCheckSavingsUnit::ApiEquivalentUsd
                        }
                        antiburn_local::remediation::SavingsUnit::Improvements => {
                            BurnCheckSavingsUnit::Improvements
                        }
                    },
                }
            }),
            estimated_token_burn_basis_points: value.estimated_token_burn_basis_points,
            verification_limit: match value.verification_limit {
                Limit::FreshEvidenceFromSameSourceAndTarget => {
                    BurnCheckVerificationLimit::FreshEvidenceFromSameSourceAndTarget
                }
                Limit::ExactPositiveControlRequired => {
                    BurnCheckVerificationLimit::ExactPositiveControlRequired
                }
                Limit::CurrentEvidenceCannotProveFix => {
                    BurnCheckVerificationLimit::CurrentEvidenceCannotProveFix
                }
            },
        }
    }
}

impl From<crate::store::RemediationState> for BurnCheckWatchLifecycle {
    fn from(value: crate::store::RemediationState) -> Self {
        match value {
            crate::store::RemediationState::Reserved => Self::Reserved,
            crate::store::RemediationState::Writing => Self::Writing,
            crate::store::RemediationState::RecoveryNeeded => Self::RecoveryNeeded,
            crate::store::RemediationState::WaitingForPromptUse => Self::WaitingForPromptUse,
            crate::store::RemediationState::Watching => Self::Watching,
            crate::store::RemediationState::Fixed => Self::Fixed,
            crate::store::RemediationState::Recurred => Self::Recurred,
        }
    }
}

impl From<crate::remediation::VerificationReason> for BurnCheckVerificationReason {
    fn from(value: crate::remediation::VerificationReason) -> Self {
        match value {
            crate::remediation::VerificationReason::MissingPostBoundaryEvidence => {
                Self::MissingPostBoundaryEvidence
            }
            crate::remediation::VerificationReason::WriteOutcomeUnknown => {
                Self::WriteOutcomeUnknown
            }
            crate::remediation::VerificationReason::VerificationUnavailable => {
                Self::VerificationUnavailable
            }
            crate::remediation::VerificationReason::UnsupportedAgent => Self::UnsupportedAgent,
            crate::remediation::VerificationReason::HomeUnavailable => Self::HomeUnavailable,
            crate::remediation::VerificationReason::PhysicalTargetChanged => {
                Self::PhysicalTargetChanged
            }
        }
    }
}

impl From<crate::remediation::VerificationStatus> for BurnCheckVerificationPayload {
    fn from(value: crate::remediation::VerificationStatus) -> Self {
        use crate::remediation::VerificationStatus;

        match value {
            VerificationStatus::Reserved => Self::Reserved,
            VerificationStatus::Watching {
                reason,
                method_revision,
                evidence_revision,
            } => Self::Watching {
                reason: reason.map(Into::into),
                method_revision,
                evidence_revision,
            },
            VerificationStatus::Fixed {
                method_revision,
                evidence_revision,
            } => Self::Fixed {
                method_revision,
                evidence_revision,
            },
            VerificationStatus::StillUnresolved {
                method_revision,
                evidence_revision,
            } => Self::StillUnresolved {
                method_revision,
                evidence_revision,
            },
            VerificationStatus::Recurred {
                method_revision,
                evidence_revision,
            } => Self::Recurred {
                method_revision,
                evidence_revision,
            },
            VerificationStatus::RecoveryNeeded {
                reason,
                checked_at_epoch,
            } => Self::RecoveryNeeded {
                reason: reason.into(),
                checked_at_epoch,
            },
            VerificationStatus::VerificationUnavailable => Self::VerificationUnavailable,
        }
    }
}

impl From<crate::remediation::SavingsUnknownReason> for BurnCheckSavingsUnknownReason {
    fn from(value: crate::remediation::SavingsUnknownReason) -> Self {
        match value {
            crate::remediation::SavingsUnknownReason::MissingRates => Self::MissingRates,
            crate::remediation::SavingsUnknownReason::MissingEvidence => Self::MissingEvidence,
            crate::remediation::SavingsUnknownReason::MissingRevision => Self::MissingRevision,
            crate::remediation::SavingsUnknownReason::ArithmeticOverflow => {
                Self::ArithmeticOverflow
            }
        }
    }
}

impl From<crate::remediation::SavingsStatus> for BurnCheckSavingsPayload {
    fn from(value: crate::remediation::SavingsStatus) -> Self {
        use crate::remediation::SavingsStatus;

        match value {
            SavingsStatus::Pending { method_revision } => Self::Pending { method_revision },
            SavingsStatus::Unavailable => Self::Unavailable,
            SavingsStatus::Unknown {
                reason,
                method_revision,
            } => Self::Unknown {
                reason: reason.into(),
                method_revision,
            },
            SavingsStatus::Known {
                method: crate::remediation::SavingsMethod::OldModelPriceDifference,
                method_revision,
                pricing_revision,
                api_equivalent_cost_avoided_usd,
                measured_through_ms,
                recurrence_ms,
            } => Self::Known {
                method: BurnCheckSavingsMethod::OldModelPriceDifference,
                method_revision,
                pricing_revision,
                api_equivalent_cost_avoided_usd,
                measured_through_ms,
                recurrence_ms,
            },
        }
    }
}

impl From<crate::remediation::WatchStatus> for BurnCheckWatchPayload {
    fn from(value: crate::remediation::WatchStatus) -> Self {
        Self {
            watch_id: value.watch_id,
            origin: match value.origin {
                crate::remediation::RemediationOrigin::Passive => AggregateWinOrigin::Passive,
                crate::remediation::RemediationOrigin::Action => AggregateWinOrigin::Action,
            },
            lifecycle: value.lifecycle.into(),
            verification: value.verification.into(),
            savings: value.savings.into(),
        }
    }
}

impl From<crate::remediation::BurnCheckTarget> for BurnCheckTargetPayload {
    fn from(value: crate::remediation::BurnCheckTarget) -> Self {
        let finding = value.finding;
        Self {
            finding_id: value.finding_id,
            action_id: value.action_id,
            finding: BurnCheckFindingPayload {
                detector: finding.detector.into(),
                agent: finding.agent.slug().to_owned(),
                source_format: finding.source_format.into(),
                observation: finding.observation,
                labels: finding.facts.labels,
                omitted: finding.facts.omitted,
            },
            display: value.display.into(),
            occurrence_count: u64::try_from(value.occurrences).unwrap_or(u64::MAX),
            affected_session_count: value
                .affected_sessions
                .map(|count| u64::try_from(count).unwrap_or(u64::MAX)),
            project_name: value.project_name,
            project_location: value.project_location,
            project_path: value.project_path,
            config_file: value.config_file,
            auto_fix: match value.auto_fix {
                crate::remediation::AutoFixAvailability::Available => {
                    AutoFixAvailabilityPayload::Available
                }
                crate::remediation::AutoFixAvailability::Unavailable(reason) => {
                    AutoFixAvailabilityPayload::Unavailable {
                        reason: reason.into(),
                    }
                }
            },
            prompt_fix: match value.prompt_fix {
                crate::remediation::PromptFixAvailability::Available => {
                    PromptFixAvailabilityPayload::Available
                }
                crate::remediation::PromptFixAvailability::Unavailable(reason) => {
                    PromptFixAvailabilityPayload::Unavailable {
                        reason: reason.into(),
                    }
                }
            },
            watch: value.watch.map(Into::into),
            coverage_limits: value
                .coverage_limits
                .into_iter()
                .map(|limit| match limit {
                    crate::remediation::CoverageLimit::CurrentPublishedEvidenceOnly => {
                        BurnCheckCoverageLimit::CurrentPublishedEvidenceOnly
                    }
                })
                .collect(),
            samples: Vec::new(),
            expires_at_epoch: value.expires_at_epoch,
        }
    }
}

impl From<crate::remediation::AutoFixReview> for AutoFixReviewPayload {
    fn from(value: crate::remediation::AutoFixReview) -> Self {
        Self {
            prepared_operation_id: value.prepared_operation_id,
            expires_at_epoch: value.expires_at_epoch,
            agent: value.agent.slug().to_owned(),
            scope: value.scope.into(),
            setting: match value.setting {
                crate::remediation::AutoFixSetting::Model => AutoFixSetting::Model,
                crate::remediation::AutoFixSetting::Reasoning => AutoFixSetting::Reasoning,
                crate::remediation::AutoFixSetting::Compaction => AutoFixSetting::Compaction,
                crate::remediation::AutoFixSetting::SubagentModel => AutoFixSetting::SubagentModel,
                crate::remediation::AutoFixSetting::McpServer => AutoFixSetting::McpServer,
                crate::remediation::AutoFixSetting::BuiltInTool => AutoFixSetting::BuiltInTool,
                crate::remediation::AutoFixSetting::Skill => AutoFixSetting::Skill,
                crate::remediation::AutoFixSetting::FastMode => AutoFixSetting::FastMode,
            },
            config_file: value.config_file,
            selector_label: value.selector_label,
            current_value: value.current_value,
            proposed_value: value.proposed_value,
            behavior_override_warning: value.behavior_override_warning,
            effect: match value.effect {
                crate::remediation::AutoFixEffect::ModelSelection => AutoFixEffect::ModelSelection,
                crate::remediation::AutoFixEffect::ReasoningEffort => {
                    AutoFixEffect::ReasoningEffort
                }
                crate::remediation::AutoFixEffect::SessionCompaction => {
                    AutoFixEffect::SessionCompaction
                }
                crate::remediation::AutoFixEffect::WorkerModelSelection => {
                    AutoFixEffect::WorkerModelSelection
                }
                crate::remediation::AutoFixEffect::McpAvailability => {
                    AutoFixEffect::McpAvailability
                }
                crate::remediation::AutoFixEffect::ToolAvailability => {
                    AutoFixEffect::ToolAvailability
                }
                crate::remediation::AutoFixEffect::SkillAvailability => {
                    AutoFixEffect::SkillAvailability
                }
                crate::remediation::AutoFixEffect::ServiceTierSelection => {
                    AutoFixEffect::ServiceTierSelection
                }
            },
            side_effect: match value.side_effect {
                crate::remediation::AutoFixSideEffect::ModelBehaviorMayChange => {
                    AutoFixSideEffect::ModelBehaviorMayChange
                }
                crate::remediation::AutoFixSideEffect::ResponsesMayUseLessReasoning => {
                    AutoFixSideEffect::ResponsesMayUseLessReasoning
                }
                crate::remediation::AutoFixSideEffect::EarlierSessionSummarization => {
                    AutoFixSideEffect::EarlierSessionSummarization
                }
                crate::remediation::AutoFixSideEffect::WorkerBehaviorMayChange => {
                    AutoFixSideEffect::WorkerBehaviorMayChange
                }
                crate::remediation::AutoFixSideEffect::ServerWillNotBeAvailable => {
                    AutoFixSideEffect::ServerWillNotBeAvailable
                }
                crate::remediation::AutoFixSideEffect::ToolWillNotBeAvailable => {
                    AutoFixSideEffect::ToolWillNotBeAvailable
                }
                crate::remediation::AutoFixSideEffect::SkillWillNotBeAvailable => {
                    AutoFixSideEffect::SkillWillNotBeAvailable
                }
                crate::remediation::AutoFixSideEffect::ResponsesMayTakeLonger => {
                    AutoFixSideEffect::ResponsesMayTakeLonger
                }
            },
        }
    }
}

impl From<crate::remediation::AggregateWins> for AggregateWinsPayload {
    fn from(value: crate::remediation::AggregateWins) -> Self {
        Self {
            wins: value
                .wins
                .into_iter()
                .map(|win| AggregateWinPayload {
                    finding_id: win.finding_id,
                    detector: win.detector.into(),
                    origin: match win.origin.as_str() {
                        "passive" => AggregateWinOrigin::Passive,
                        "action" => AggregateWinOrigin::Action,
                        _ => unreachable!("the store validates contribution origins"),
                    },
                    display: win.display.into(),
                    savings: AggregateSavingsPayload {
                        token_savings: win.savings.token_savings,
                        api_equivalent_cost_avoided_usd: win
                            .savings
                            .api_equivalent_cost_avoided_usd,
                        improvement_count: win.savings.improvement_count,
                        method: win.savings.method.map(Into::into),
                    },
                    starts_at_ms: win.starts_at_ms,
                    ends_at_ms: win.ends_at_ms,
                })
                .collect(),
        }
    }
}

impl From<crate::remediation::BurnCheckTargetList> for BurnCheckTargetListPayload {
    fn from(value: crate::remediation::BurnCheckTargetList) -> Self {
        Self {
            targets: value.targets.into_iter().map(Into::into).collect(),
            samples: Vec::new(),
            truncated: value.truncated,
        }
    }
}

impl From<crate::remediation::BurnCheckRemediationProgress>
    for BurnCheckRemediationProgressPayload
{
    fn from(value: crate::remediation::BurnCheckRemediationProgress) -> Self {
        Self {
            attempts: value
                .attempts
                .into_iter()
                .map(|attempt| BurnCheckRemediationAttemptPayload {
                    detector: attempt.detector.into(),
                    watch_id: attempt.watch_id,
                    display: attempt.display.into(),
                    origin: match attempt.origin {
                        crate::remediation::RemediationOrigin::Passive => {
                            AggregateWinOrigin::Passive
                        }
                        crate::remediation::RemediationOrigin::Action => AggregateWinOrigin::Action,
                    },
                    lifecycle: attempt.lifecycle.into(),
                    outcome: match attempt.outcome {
                        crate::remediation::BurnCheckRemediationOutcome::Failed => {
                            BurnCheckRemediationOutcomePayload::Failed
                        }
                        crate::remediation::BurnCheckRemediationOutcome::Passed => {
                            BurnCheckRemediationOutcomePayload::Passed
                        }
                    },
                    verification: attempt.verification.into(),
                    savings: attempt.savings.into(),
                    effective_boundary_ms: attempt.effective_boundary_ms,
                    verified_boundary_ms: attempt.verified_boundary_ms,
                    recurred_boundary_ms: attempt.recurred_boundary_ms,
                })
                .collect(),
        }
    }
}

fn not_assessed_reason_str(reason: NotAssessedReason) -> &'static str {
    match reason {
        NotAssessedReason::NoSessionsInWindow => "noSessionsInWindow",
        NotAssessedReason::CapabilityMissing => "capabilityMissing",
        NotAssessedReason::IncompleteEvidence => "incompleteEvidence",
        NotAssessedReason::EvidenceContractIncomplete => "evidenceContractIncomplete",
        NotAssessedReason::SignalMissing => "signalMissing",
    }
}

impl ChecksReportPayload {
    pub(crate) fn from_reduced_report(report: &crate::insights_report::ReducedReport) -> Self {
        let mut payload = Self::from_report(
            &report.report,
            report.evidence_settled,
            report.pending_evidence,
        );
        for detector in [
            DetectorId::UnusedMcpServers,
            DetectorId::UnusedBuiltInTools,
            DetectorId::UnusedSkills,
        ] {
            let Some(assessment) = report.resources.detector(detector) else {
                continue;
            };
            let category = &mut payload.categories[detector.index()];
            category.finding = assessment.unused_count;
            category.clean = u64::from(assessment.clean);
            category.unavailable = u64::from(assessment.unavailable);
            category.agents = if assessment.unused_count > 0 {
                &assessment.finding_agents
            } else {
                &assessment.clean_agents
            }
            .iter()
            .cloned()
            .collect();
            category.estimated_token_burn_basis_points =
                assessment.estimated_token_burn_basis_points;
        }
        let resource_tokens = report.resources.measured_finding_tokens_by_session();
        payload.estimated_token_burn_basis_points = report
            .report
            .estimated_token_burn_with_resource_tokens_by_session(resource_tokens.as_deref());
        payload
    }

    pub fn from_report(
        report: &EfficiencyReport,
        evidence_settled: bool,
        pending_evidence: u64,
    ) -> Self {
        let categories = DetectorId::ALL
            .iter()
            .map(|&id| {
                let counts = report.detectors[id.index()];
                ChecksCategoryPayload {
                    id: id.into(),
                    finding: counts.finding,
                    agents: if counts.finding > 0 {
                        &report.finding_agents[id.index()]
                    } else {
                        &report.clean_agents[id.index()]
                    }
                    .iter()
                    .cloned()
                    .collect(),
                    clean: counts.clean,
                    unavailable: counts.unavailable,
                    estimated_token_burn_basis_points: report
                        .detector_estimated_token_burn_basis_points[id.index()],
                }
            })
            .collect();
        Self {
            evidence_settled,
            pending_evidence,
            estimated_token_burn_basis_points: report.estimated_token_burn_basis_points,
            categories,
        }
    }
}

/// Where the app came from and what it is running against.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub app_version: String,
    /// True when Rust enables debug assertions for this binary.
    pub debug_build: bool,
    /// CPU architecture this binary was compiled for, e.g. `aarch64`.
    pub arch: String,
    /// Version of the active runtime pricing catalog.
    pub pricing_catalog_version: String,
    /// Applied schema version of the local database.
    pub schema_version: i64,
    /// Absolute path of the app data directory, so a reader can find their own
    /// data without being told where it "should" be.
    pub data_dir: String,
    /// Sessions currently in the local index.
    pub indexed_sessions: u32,
    /// Size of the local database on disk, in bytes. Zero when it has not been
    /// written yet — a fresh install, or a store held in memory.
    pub database_bytes: u64,
    /// False in development builds, where the updater plugin is not installed.
    pub updates_supported: bool,
    /// Whether this build includes a configured analytics client.
    pub analytics_supported: bool,
    /// True when the process environment disables an analytics-capable build.
    pub analytics_environment_disabled: bool,
    /// Who receives those events, in the reader's own words. `None` when the
    /// build has no complete analytics configuration.
    pub analytics_operator: Option<String>,
}

/* -------------------------------------------------------------------------
 * Live provider usage — the provider's own figures.
 *
 * A separate payload from `ProviderUsageSummary` on purpose. That type's
 * guarantee is that it contains no percentage, allowance, or reset anywhere,
 * and a test proves it by serializing the whole thing and grepping. Adding a
 * limit field to it would end that guarantee for the estimate path as well as
 * the limit path, and the views can layer two payloads perfectly well.
 * ---------------------------------------------------------------------- */

/// Marks figures stated directly by a provider rather than locally estimated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LiveUsageSupport {
    /// The provider stated this allowance. A determinate meter is honest.
    Live,
}

/// Whether a reading still describes the present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LiveUsageFreshness {
    Fresh,
    Stale,
}

/// One provider-reported allowance.
///
/// Every field through `resets_at` is either something the provider stated or
/// `null`. Nothing there is derived, interpolated, or defaulted — in
/// particular `used_percent` is `null` rather than `0.0` when the provider did
/// not say, because a meter reading empty and a meter reading unknown are
/// different facts. The last two fields are the exception, and both are
/// derived from this window's own sample history rather than stated by
/// anyone.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveUsageWindow {
    /// Stable id within the provider: `five-hour`, `seven-day`, `weekly-<model>`.
    pub id: String,
    /// `primaryShort`, `primaryLong`, `supplemental`, or the provider's own word.
    pub role: String,
    /// `rolling`, `weekly`, `daily`, `monthly`, `billingCycle`, or the provider's own word.
    pub kind: String,
    /// The model a scoped window covers, when it covers one.
    pub scope_model: Option<String>,
    /// Consumed capacity in `0..=100`. Never remaining.
    pub used_percent: Option<f64>,
    /// ISO-8601 start of the current window, when the provider stated one.
    pub starts_at: Option<String>,
    /// ISO-8601 reset, when the provider stated one.
    pub resets_at: Option<String>,
    /// How far into its own period this window has travelled, from 0 to 1.
    ///
    /// This is what the marker on each bar shows. `None` when the period is
    /// unknown, and the views then draw no marker.
    ///
    /// A weekly window measures this against the days the reader works, so a
    /// five-day week reads 100% from Friday midnight until the reset.
    pub elapsed_fraction: Option<f64>,
    /// Whether trustworthy history shows non-zero usage anywhere in this
    /// window's current allowance period. The views consult this only for a
    /// supplemental, model-scoped window — most readers never touch that
    /// model, so it stays hidden until this turns true, then stays visible
    /// for the rest of the period even past a reading that comes back with
    /// no percentage at all.
    pub has_nonzero_usage_in_current_period: bool,
    /// What this window's own history supports saying about it.
    pub forecast: LiveUsageForecast,
}

/// The derived half of a window: what its history says, or why it says
/// nothing.
///
/// Exactly one of `unavailable_reason` and the value fields is populated.
/// That is not a formality — "we have not seen enough of your week to say"
/// and "you are on track" are different answers, and only one of them is
/// reassuring. A null here always means the former.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveUsageForecast {
    /// `stale`, `transition`, or `sparseHistory`. Null when there *is* a
    /// forecast.
    pub unavailable_reason: Option<String>,
    /// `low`, `medium`, or `high`, for the values below.
    pub confidence: Option<String>,
    /// Percentage points of the allowance consumed per hour.
    pub consumption_rate: Option<f64>,
    /// The current rate over the rate that would land exactly at the reset.
    /// Above 1 means the allowance runs out first.
    pub pace_ratio: Option<f64>,
    /// The last half hour's rate over the last two hours'. Above 1 is
    /// speeding up.
    pub pace_trend: Option<f64>,
    /// ISO-8601 moment the allowance runs out at the current rate.
    pub runway_at: Option<String>,
    /// Percentage points of this window consumed since the reader's local
    /// midnight. Only meaningful on a window longer than a day.
    pub used_today: Option<f64>,
}

/// One provider account's live usage.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveProviderUsage {
    /// Canonical provider id, matching [`ProviderUsage::provider`] so the
    /// views can join the two payloads without a translation table.
    pub provider: String,
    /// A stable opaque key for this provider account. This value is `null`
    /// when the source does not identify an account.
    #[serde(default)]
    pub account_key: Option<String>,
    pub display_name: String,
    pub support: LiveUsageSupport,
    pub freshness: LiveUsageFreshness,
    /// A short description of where the figures came from, safe to display.
    /// Carries no account identifier.
    pub source_label: String,
    /// ISO-8601 stamp of when the *provider fact* was observed — not when the
    /// app read it. The difference is the whole point of showing it.
    pub observed_at: String,
    pub windows: Vec<LiveUsageWindow>,
    /// Metered usage beyond the allowance, when the provider reports it.
    pub extra_usage: Option<LiveExtraUsage>,
    /// The provider reports manual rate-limit resets here.
    #[serde(default)]
    pub reset_credits: Option<LiveUsageResetCredits>,
    /// The subscription plan, when the source stated one. `null` when it did
    /// not. Defaulted on deserialize: a snapshot cached before this field
    /// existed must still load.
    #[serde(default)]
    pub plan: Option<LiveProviderPlan>,
    /// The provider's raw account UUID for local display.
    #[serde(default)]
    pub account_uuid: Option<String>,
    /// The provider's account email for local display.
    #[serde(default)]
    pub account_email: Option<String>,
}

/// The provider's own plan label, raw. The frontend maps these strings to
/// display text; nothing here is a display string already.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveProviderPlan {
    /// The plan, for example `"max"` or `"plus"`.
    pub name: String,
    /// A finer-grained tier within `name`, when the source stated one, for
    /// example `"default_claude_max_5x"`.
    pub tier: Option<String>,
}

/// Provider credits that manually reset rate limits.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveUsageResetCredits {
    pub available_count: u64,
}

/// Metered spend alongside the allowance.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveExtraUsage {
    /// Whether the account permits this path. `false` differs from unknown.
    pub enabled: bool,
    pub used_percent: Option<f64>,
    pub used: Option<f64>,
    pub remaining: Option<f64>,
    pub limit: Option<f64>,
    /// Currency code for the three amounts above, when they are monetary.
    pub currency: Option<String>,
}

/// A source that failed, in terms a reader can act on.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveUsageSourceError {
    /// The source's stable id.
    pub source: String,
    /// The canonical id of the provider the source answers for.
    ///
    /// A failed source contributes no entry to `providers`, so this field is
    /// the only place the views can learn whose usage is missing. Defaulted
    /// on deserialize: a snapshot cached before this field existed must
    /// still load.
    #[serde(default)]
    pub provider: String,
    /// The provider's display name, for example "Claude". Defaulted like
    /// `provider`.
    #[serde(default)]
    pub display_name: String,
    /// `authentication`, `rateLimited`, `schema`, or `unavailable`.
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<SourceErrorDetail>,
}

/// One provider antiburn can meter, and whether the reader shows it.
///
/// The roster comes from the registered sources, not from the readings. A
/// hidden provider is never asked for usage, so it is absent from
/// `LiveUsageSummary::providers`. Only this list keeps its switch on screen.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveUsageMeter {
    /// The canonical provider id, for example `anthropic`.
    pub provider: String,
    /// The provider's display name, for example "Claude".
    pub display_name: String,
    /// False when the reader turned this meter off.
    pub shown: bool,
    #[serde(default)]
    pub detection: Detection,
    /// Where the login was found, when a carrier was. Kept through the
    /// `signedIn` upgrade so the note can name the tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carrier: Option<LoginCarrier>,
    /// `carrier`'s display name, so the views never restate the enum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carrier_label: Option<String>,
}

/// Live provider usage, as one snapshot.
///
/// An empty `providers` list is the ordinary state — no source configured, or
/// none with anything to say — and the views render nothing rather than an
/// empty frame. `errors` is separate so that "nothing found" and "something
/// broke" never look alike.
///
/// `meters` says which of the two an empty `providers` list is. A roster with
/// every entry hidden means the reader turned the meters off. A roster with
/// entries still shown means antiburn found nothing to report.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveUsageSummary {
    pub providers: Vec<LiveProviderUsage>,
    pub errors: Vec<LiveUsageSourceError>,
    /// Every provider antiburn can meter, shown or hidden, ordered by id.
    ///
    /// Defaulted on deserialize: a snapshot cached before this field existed
    /// must still load.
    #[serde(default)]
    pub meters: Vec<LiveUsageMeter>,
    /// ISO-8601 stamp of the moment this snapshot was collected.
    pub generated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_usage_payload_serializes_camel_case_fields_and_boundary_source_strings() {
        let payload = QuotaUsagePayload {
            provider: "anthropic".to_string(),
            account_key: "a".repeat(64),
            lane: "fiveHour".to_string(),
            lane_label: "5-hour".to_string(),
            range_start_epoch: 0,
            range_end_epoch: 1_000,
            periods: vec![QuotaPeriodPayload {
                period_id: None,
                starts_at_epoch: 0,
                resets_at_epoch: 1_000,
                start_source: "turnGap".to_string(),
                reset_source: "cadence".to_string(),
                samples: vec![QuotaSamplePayload {
                    observed_at_epoch: 500,
                    used_percent: Some(10.0),
                    fresh: true,
                    authoritative: true,
                }],
                contributions: vec![QuotaContributionPayload {
                    agent: "claude-code".to_string(),
                    session_id: "s1".to_string(),
                    wsl_distro: None,
                    bucket_start_epoch: 0,
                    usd: 1.0,
                    percent: Some(2.0),
                }],
                sessions: vec![QuotaSessionTotalPayload {
                    agent: "claude-code".to_string(),
                    session_id: "s1".to_string(),
                    wsl_distro: None,
                    title: Some("Fix the bug".to_string()),
                    usd: 1.0,
                    percent: Some(2.0),
                }],
                unattributed: QuotaUnattributedPayload {
                    usd: 0.5,
                    percent: Some(1.0),
                    session_count: 1,
                },
                unattributed_buckets: vec![QuotaBucketTotalPayload {
                    bucket_start_epoch: 0,
                    usd: 0.5,
                    percent: Some(1.0),
                }],
                estimated_percent: Some(2.0),
                unexplained_buckets: vec![QuotaBucketTotalPayload {
                    bucket_start_epoch: 0,
                    usd: 0.0,
                    percent: Some(0.5),
                }],
                unexplained_percent: Some(0.5),
            }],
            generated_at: "2026-09-16T00:00:00Z".to_string(),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["accountKey"], "a".repeat(64));
        assert_eq!(json["laneLabel"], "5-hour");
        assert_eq!(json["rangeStartEpoch"], 0);
        assert_eq!(json["rangeEndEpoch"], 1_000);
        let period = &json["periods"][0];
        assert_eq!(period["periodId"], serde_json::Value::Null);
        assert_eq!(period["startsAtEpoch"], 0);
        assert_eq!(period["resetsAtEpoch"], 1_000);
        assert_eq!(period["startSource"], "turnGap");
        assert_eq!(period["resetSource"], "cadence");
        assert_eq!(period["contributions"][0]["bucketStartEpoch"], 0);
        assert_eq!(period["sessions"][0]["sessionId"], "s1");
        assert_eq!(period["unattributed"]["sessionCount"], 1);
        assert_eq!(period["unattributedBuckets"][0]["bucketStartEpoch"], 0);
        assert_eq!(period["unattributedBuckets"][0]["usd"], 0.5);
        assert_eq!(period["unattributedBuckets"][0]["percent"], 1.0);
        assert_eq!(period["estimatedPercent"], 2.0);
        assert_eq!(period["unexplainedBuckets"][0]["usd"], 0.0);
        assert_eq!(period["unexplainedBuckets"][0]["percent"], 0.5);
        assert_eq!(period["unexplainedPercent"], 0.5);
    }

    #[test]
    fn session_quota_payload_serializes_camel_case_fields_and_confidence_strings() {
        let payload = SessionQuotaPayload {
            entries: vec![
                SessionQuotaEntryPayload {
                    provider: "anthropic".to_string(),
                    display_name: "Claude".to_string(),
                    account_key: Some("a".repeat(64)),
                    lane: Some("weekly".to_string()),
                    lane_label: Some("Weekly".to_string()),
                    period: Some(SessionQuotaPeriodPayload {
                        period_id: Some(7),
                        starts_at_epoch: 0,
                        resets_at_epoch: 604_800,
                        start_source: "reported".to_string(),
                        reset_source: "derived".to_string(),
                    }),
                    usd: 1.0,
                    percent: Some(2.0),
                    confidence: "learned".to_string(),
                    plan: Some(LiveProviderPlan {
                        name: "max".to_string(),
                        tier: Some("max_20x".to_string()),
                    }),
                },
                SessionQuotaEntryPayload {
                    provider: "openai".to_string(),
                    display_name: "Codex".to_string(),
                    account_key: None,
                    lane: None,
                    lane_label: None,
                    period: None,
                    usd: 0.5,
                    percent: None,
                    confidence: "unbound".to_string(),
                    plan: None,
                },
            ],
            generated_at: "2026-09-16T00:00:00Z".to_string(),
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["entries"][0]["displayName"], "Claude");
        assert_eq!(json["entries"][0]["accountKey"], "a".repeat(64));
        assert_eq!(json["entries"][0]["laneLabel"], "Weekly");
        assert_eq!(json["entries"][0]["period"]["periodId"], 7);
        assert_eq!(json["entries"][0]["period"]["startSource"], "reported");
        assert_eq!(json["entries"][0]["period"]["resetSource"], "derived");
        assert_eq!(json["entries"][0]["confidence"], "learned");
        assert_eq!(json["entries"][0]["plan"]["name"], "max");
        assert_eq!(json["entries"][0]["plan"]["tier"], "max_20x");
        assert_eq!(json["entries"][1]["accountKey"], serde_json::Value::Null);
        assert_eq!(json["entries"][1]["plan"], serde_json::Value::Null);
        assert_eq!(json["entries"][1]["lane"], serde_json::Value::Null);
        assert_eq!(json["entries"][1]["laneLabel"], serde_json::Value::Null);
        assert_eq!(json["entries"][1]["period"], serde_json::Value::Null);
        assert_eq!(json["entries"][1]["confidence"], "unbound");
    }

    #[test]
    fn legacy_live_errors_round_trip_without_a_detail_field() {
        let json = serde_json::json!({
            "source": "fixture", "provider": "anthropic", "displayName": "Claude", "category": "unavailable"
        });
        let error: LiveUsageSourceError = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(error.detail, None);
        assert_eq!(serde_json::to_value(error).unwrap(), json);
    }

    #[test]
    fn live_error_details_use_closed_camel_case_values() {
        for (detail, wire, provider, category) in [
            (
                SourceErrorDetail::KeychainUnreadable,
                "keychainUnreadable",
                "anthropic",
                "unavailable",
            ),
            (
                SourceErrorDetail::RefreshUnsupported,
                "refreshUnsupported",
                "google",
                "authentication",
            ),
        ] {
            let json = serde_json::json!({
                "source": "fixture", "provider": provider, "displayName": "Fixture",
                "category": category, "detail": wire
            });
            let error: LiveUsageSourceError = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(error.detail, Some(detail));
            assert_eq!(serde_json::to_value(error).unwrap(), json);
        }
    }

    #[test]
    fn a_legacy_live_meter_round_trips_with_unknown_detection() {
        let meter: LiveUsageMeter = serde_json::from_value(serde_json::json!({
            "provider": "anthropic", "displayName": "Claude", "shown": true
        }))
        .unwrap();
        assert_eq!(meter.detection, Detection::Unknown);
        let json = serde_json::to_value(&meter).unwrap();
        assert_eq!(json["detection"], "unknown");
        assert_eq!(
            serde_json::from_value::<LiveUsageMeter>(json).unwrap(),
            meter
        );
    }

    #[test]
    fn live_detection_uses_camel_case_wire_values() {
        for (detection, wire) in [
            (Detection::Unknown, "unknown"),
            (Detection::NotInstalled, "notInstalled"),
            (Detection::InstalledNotSignedIn, "installedNotSignedIn"),
            (Detection::SignedIn, "signedIn"),
        ] {
            let json = serde_json::to_value(detection).unwrap();
            assert_eq!(json, wire);
            assert_eq!(
                serde_json::from_value::<Detection>(json).unwrap(),
                detection
            );
        }
    }

    mod insights {
        use antiburn_local::analysis::{
            ContextEvidence, EvidenceSource, LoadedSource, ModelControlObservation, ModelTokens,
            RelationConfidence, RelationProvenance, RepeatedContext, SessionEvidenceAccumulator,
            SourceCapabilities, SourceKind, SubagentChild, ToolDefinition, TurnCounts, TurnFacts,
        };
        use antiburn_local::insights::{
            CoverageCounts, DetectorCounts, DetectorFindings, DetectorStatus,
            EfficiencyReportAccumulator, ReportContext, ReportWindow, SessionExample,
            session_badges,
        };

        use super::*;

        fn report() -> EfficiencyReport {
            EfficiencyReportAccumulator::new().finish(ReportContext {
                environment_key: "native".to_owned(),
                window: ReportWindow {
                    start_epoch: 100,
                    end_epoch: 200,
                },
                computed_at_epoch: 200,
                parser_revision: 1,
                analyzer_revision: 1,
                evidence_schema_revision: 1,
                coverage: CoverageCounts::default(),
            })
        }

        #[test]
        fn checks_report_serializes_only_display_fields() {
            let mut report = report();
            report.finding_agents[0].extend(["codex".to_owned(), "claude-code".to_owned()]);
            report.clean_agents[0].insert("cursor".to_owned());
            report.clean_agents[1].insert("opencode".to_owned());
            report.estimated_token_burn_basis_points = Some(1_625);
            report.detector_estimated_token_burn_basis_points[0] = Some(500);
            report.detector_statuses[0] = DetectorStatus::Findings(DetectorFindings {
                finding_sessions: 2,
                examples: vec![SessionExample {
                    agent: "claude-code".to_owned(),
                    session_id: "session-1".to_owned(),
                }],
            });
            report.detectors[0] = DetectorCounts {
                eligible: 4,
                assessed: 3,
                finding: 2,
                clean: 1,
                unavailable: 1,
                not_applicable: 0,
            };

            let value =
                serde_json::to_value(ChecksReportPayload::from_report(&report, true, 0)).unwrap();
            assert_eq!(
                value["categories"][0]["agents"],
                serde_json::json!(["claude-code", "codex"])
            );
            assert_eq!(
                value["categories"][1]["agents"],
                serde_json::json!(["opencode"])
            );
            assert!(value["categories"][0].get("examples").is_none());
            assert!(value.get("coverage").is_none());
            assert!(value.get("quotaPressure").is_none());
            assert!(value.get("providerIncidents").is_none());
            assert_eq!(value["evidenceSettled"], true);
            assert_eq!(value["pendingEvidence"], 0);
            assert_eq!(value["estimatedTokenBurnBasisPoints"], 1_625);
            assert_eq!(value["categories"][0]["estimatedTokenBurnBasisPoints"], 500);
            assert_eq!(
                value["categories"][1]["estimatedTokenBurnBasisPoints"],
                serde_json::Value::Null
            );

            let top_keys: Vec<&str> = value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                top_keys,
                [
                    "categories",
                    "estimatedTokenBurnBasisPoints",
                    "evidenceSettled",
                    "pendingEvidence"
                ]
            );
            let category_keys: Vec<&str> = value["categories"][0]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                category_keys,
                [
                    "agents",
                    "clean",
                    "estimatedTokenBurnBasisPoints",
                    "finding",
                    "id",
                    "unavailable",
                ]
            );
            assert_eq!(value["categories"][0]["finding"], 2);
            assert_eq!(value["categories"][0]["clean"], 1);
            assert_eq!(value["categories"][0]["unavailable"], 1);

            let value =
                serde_json::to_value(ChecksReportPayload::from_report(&report, false, 4)).unwrap();
            assert_eq!(value["evidenceSettled"], false);
            assert_eq!(value["pendingEvidence"], 4);
            assert_eq!(value["estimatedTokenBurnBasisPoints"], 1_625);
        }

        #[test]
        fn burn_check_contract_serializes_tagged_states_and_decimal_savings() {
            let payload = BurnCheckWatchPayload {
                watch_id: "opaque-watch".into(),
                origin: AggregateWinOrigin::Action,
                lifecycle: BurnCheckWatchLifecycle::Fixed,
                verification: BurnCheckVerificationPayload::Fixed {
                    method_revision: 3,
                    evidence_revision: "source-7".into(),
                },
                savings: BurnCheckSavingsPayload::Known {
                    method: BurnCheckSavingsMethod::OldModelPriceDifference,
                    method_revision: 4,
                    pricing_revision: "pricing-9".into(),
                    api_equivalent_cost_avoided_usd: -1.25,
                    measured_through_ms: 500,
                    recurrence_ms: Some(450),
                },
            };

            let value = serde_json::to_value(payload).unwrap();
            assert_eq!(value["watchId"], "opaque-watch");
            assert_eq!(value["origin"], "action");
            assert_eq!(value["lifecycle"], "fixed");
            assert_eq!(value["verification"]["status"], "fixed");
            assert_eq!(value["verification"]["methodRevision"], 3);
            assert_eq!(value["savings"]["status"], "known");
            assert_eq!(value["savings"]["method"], "oldModelPriceDifference");
            assert!(value["savings"].get("tokenEquivalentSavings").is_none());
            assert_eq!(value["savings"]["apiEquivalentCostAvoidedUsd"], -1.25);
            assert_eq!(value["savings"]["measuredThroughMs"], 500);
            assert_eq!(value["savings"]["recurrenceMs"], 450);

            let outcome = serde_json::to_value(
                ApplyPreparedBurnCheckOperationOutcome::AppliedAwaitingVerification {
                    watch_id: "opaque-watch".into(),
                },
            )
            .unwrap();
            assert_eq!(
                outcome,
                serde_json::json!({
                    "outcome": "appliedAwaitingVerification",
                    "watchId": "opaque-watch"
                })
            );
            assert_eq!(
                serde_json::to_value(ApplyPreparedBurnCheckOperationOutcome::Applied).unwrap(),
                serde_json::json!({"outcome": "applied"})
            );
        }

        #[test]
        fn burn_check_review_and_display_dtos_expose_only_semantic_facts() {
            let display = BurnCheckDisplayFactsPayload {
                resource_kind: BurnCheckResourceKind::Model,
                resource_identity: Some("old-model".into()),
                current_value: Some("old-model".into()),
                replacement_value: Some("new-model".into()),
                scope_kind: BurnCheckScopeKind::Project,
                quantity: Some(3),
                quantity_unit: Some(BurnCheckQuantityUnit::Turns),
                observation_count: 2,
                first_observed_at_ms: 100,
                last_observed_at_ms: 200,
                estimate_method: Some(BurnCheckEstimateMethod::OldModelPriceDifference),
                estimated_opportunity: Some(BurnCheckEstimatedValuePayload {
                    value: -1.25,
                    unit: BurnCheckSavingsUnit::ApiEquivalentUsd,
                }),
                estimated_token_burn_basis_points: Some(1_250),
                verification_limit:
                    BurnCheckVerificationLimit::FreshEvidenceFromSameSourceAndTarget,
            };
            let value = serde_json::to_value(display).unwrap();
            let keys = value
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(
                keys,
                [
                    "currentValue",
                    "estimateMethod",
                    "estimatedOpportunity",
                    "estimatedTokenBurnBasisPoints",
                    "firstObservedAtMs",
                    "lastObservedAtMs",
                    "observationCount",
                    "quantity",
                    "quantityUnit",
                    "replacementValue",
                    "resourceIdentity",
                    "resourceKind",
                    "scopeKind",
                    "verificationLimit",
                ]
            );
            assert_eq!(value["estimatedOpportunity"]["value"], -1.25);
            assert_eq!(value["estimatedOpportunity"]["unit"], "apiEquivalentUsd");
            assert_eq!(value["estimatedTokenBurnBasisPoints"], 1_250);
            let serialized = value.to_string();
            for private_name in [
                "path",
                "sessionId",
                "callId",
                "evidence",
                "config",
                "prompt",
            ] {
                assert!(!serialized.contains(private_name));
            }

            let review = serde_json::to_value(AutoFixReviewPayload {
                prepared_operation_id: "prepared".into(),
                expires_at_epoch: 300,
                agent: "claude-code".into(),
                scope: BurnCheckScopeKind::Project,
                setting: AutoFixSetting::Model,
                config_file: "~/.claude/settings.json".into(),
                selector_label: "model".into(),
                current_value: "old-model".into(),
                proposed_value: "new-model".into(),
                behavior_override_warning: false,
                effect: AutoFixEffect::ModelSelection,
                side_effect: AutoFixSideEffect::ModelBehaviorMayChange,
            })
            .unwrap();
            assert!(review.get("preparedOperationId").is_some());
            assert_eq!(review["configFile"], "~/.claude/settings.json");
            assert!(review.get("path").is_none());
            assert!(review.get("originalBytes").is_none());
            assert!(review.get("config").is_none());

            assert_eq!(
                serde_json::to_value(AutoFixSetting::Reasoning).unwrap(),
                "reasoning"
            );
            assert_eq!(
                serde_json::to_value(AutoFixEffect::ReasoningEffort).unwrap(),
                "reasoningEffort"
            );
            assert_eq!(
                serde_json::to_value(AutoFixSideEffect::ResponsesMayUseLessReasoning).unwrap(),
                "responsesMayUseLessReasoning"
            );
        }

        #[test]
        fn auto_fix_review_vocabulary_is_exhaustive_and_serialized() {
            let settings = [
                AutoFixSetting::Model,
                AutoFixSetting::Reasoning,
                AutoFixSetting::Compaction,
                AutoFixSetting::SubagentModel,
                AutoFixSetting::McpServer,
                AutoFixSetting::BuiltInTool,
                AutoFixSetting::Skill,
                AutoFixSetting::FastMode,
            ];
            let effects = [
                AutoFixEffect::ModelSelection,
                AutoFixEffect::ReasoningEffort,
                AutoFixEffect::SessionCompaction,
                AutoFixEffect::WorkerModelSelection,
                AutoFixEffect::McpAvailability,
                AutoFixEffect::ToolAvailability,
                AutoFixEffect::SkillAvailability,
                AutoFixEffect::ServiceTierSelection,
            ];
            let side_effects = [
                AutoFixSideEffect::ModelBehaviorMayChange,
                AutoFixSideEffect::ResponsesMayUseLessReasoning,
                AutoFixSideEffect::EarlierSessionSummarization,
                AutoFixSideEffect::WorkerBehaviorMayChange,
                AutoFixSideEffect::ServerWillNotBeAvailable,
                AutoFixSideEffect::ToolWillNotBeAvailable,
                AutoFixSideEffect::SkillWillNotBeAvailable,
                AutoFixSideEffect::ResponsesMayTakeLonger,
            ];
            assert_eq!(settings.len(), effects.len());
            assert_eq!(settings.len(), side_effects.len());
            for value in settings {
                assert!(serde_json::to_value(value).unwrap().is_string());
            }
            for value in effects {
                assert!(serde_json::to_value(value).unwrap().is_string());
            }
            for value in side_effects {
                assert!(serde_json::to_value(value).unwrap().is_string());
            }
        }

        #[test]
        fn burn_check_detector_request_rejects_unknown_values() {
            assert_eq!(
                serde_json::from_str::<BurnCheckDetectorId>("\"oldModelUsage\"").unwrap(),
                BurnCheckDetectorId::OldModelUsage
            );
            assert!(serde_json::from_str::<BurnCheckDetectorId>("\"futureDetector\"").is_err());
        }

        /// The badge wire shape carries identifiers only.
        #[test]
        fn the_session_hygiene_payload_contains_no_free_text() {
            let payload = SessionHygienePayload::from_badges(
                [
                    SessionBadge {
                        id: BadgeId::SessionOverdepth,
                        status: BadgeStatus::Finding,
                    },
                    SessionBadge {
                        id: BadgeId::ModelOverthinking,
                        status: BadgeStatus::Clean,
                    },
                    SessionBadge {
                        id: BadgeId::OverpoweredSubagents,
                        status: BadgeStatus::NotAssessed(NotAssessedReason::IncompleteEvidence),
                    },
                    SessionBadge {
                        id: BadgeId::ObsoleteModel,
                        status: BadgeStatus::Clean,
                    },
                    SessionBadge {
                        id: BadgeId::FastModeOveruse,
                        status: BadgeStatus::Clean,
                    },
                    SessionBadge {
                        id: BadgeId::ExcessCacheRehydration,
                        status: BadgeStatus::Clean,
                    },
                ],
                None,
                "ready",
            );

            assert_eq!(
                serde_json::to_value(payload).unwrap(),
                serde_json::json!({
                    "badges": [
                        {"id": "sessionOverdepth", "status": "finding", "notAssessedReason": null},
                        {"id": "modelOverthinking", "status": "clean", "notAssessedReason": null},
                        {
                            "id": "overpoweredSubagents",
                            "status": "notAssessed",
                            "notAssessedReason": "incompleteEvidence"
                        },
                        {"id": "obsoleteModel", "status": "clean", "notAssessedReason": null},
                        {"id": "fastModeOveruse", "status": "clean", "notAssessedReason": null},
                        {"id": "excessCacheRehydration", "status": "clean", "notAssessedReason": null}
                    ],
                    "evidenceState": "ready",
                    "unusedResources": null
                })
            );
        }

        #[test]
        fn the_session_hygiene_payload_serializes_finding_evidence() {
            let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
                agent: "claude-code".to_owned(),
                session_id: "finding-details".to_owned(),
                kind: SourceKind::File,
                capabilities: SourceCapabilities::claude(),
            })
            .evidence(&TurnFacts::default());
            let catalogs = ReportCatalogs::default();

            evidence.context = EvidenceValue::Complete(ContextEvidence {
                max_request_context_tokens: catalogs.depth_cap_tokens + 50_000,
                top_depth_examples: Vec::new(),
            });
            let EvidenceValue::Complete(models) = &mut evidence.models else {
                panic!("synthetic model evidence must be complete");
            };
            models.dominant_main_model = Some("claude-opus-4-6".to_owned());
            models.by_model.insert(
                "claude-opus-4-6".to_owned(),
                ModelTokens {
                    turns: 2,
                    last_ts_ms: i64::MAX,
                    ..ModelTokens::default()
                },
            );
            models.effort_tiers.insert(
                "max".to_owned(),
                TurnCounts {
                    main_loop: 2,
                    delegated: 0,
                },
            );
            models.control_observations.push(ModelControlObservation {
                provider: None,
                api: None,
                model: "claude-opus-4-6".to_owned(),
                effort: Some("max".to_owned()),
                speed: None,
                last_ts_ms: i64::MAX,
                turns: TurnCounts {
                    main_loop: 2,
                    delegated: 0,
                },
            });
            models.fast_modes.insert(
                FAST_SPEED_KEY.to_owned(),
                TurnCounts {
                    main_loop: 0,
                    delegated: 2,
                },
            );
            models.control_observations.push(ModelControlObservation {
                provider: None,
                api: None,
                model: "claude-opus-4-6".to_owned(),
                effort: None,
                speed: Some(FAST_SPEED_KEY.to_owned()),
                last_ts_ms: i64::MAX,
                turns: TurnCounts {
                    main_loop: 0,
                    delegated: 2,
                },
            });

            let EvidenceValue::Complete(subagents) = &mut evidence.subagents else {
                panic!("synthetic subagent evidence must be complete");
            };
            subagents.spawn_count = 1;
            subagents.delegated_turns = 2;
            subagents
                .delegated_models
                .insert("claude-opus-4-6".to_owned());
            subagents.children.push(SubagentChild {
                ordinal: 1,
                parent_model: Some("claude-opus-4-6".to_owned()),
                parent_call_id: None,
                observed_child_models: subagents.delegated_models.clone(),
                child_model: EvidenceValue::Unsupported,
                confidence: RelationConfidence::Observed,
                provenance: RelationProvenance::TaskToolUse,
            });

            let EvidenceValue::Complete(cache) = &mut evidence.cache else {
                panic!("synthetic cache evidence must be complete");
            };
            cache.repeated_context = EvidenceValue::Complete(RepeatedContext {
                accounting: RepeatedContextAccounting::CacheWrite,
                repeated_tokens: 135,
                paid_tokens: 235,
                pairs_considered: 1,
                pairs_skipped: 0,
            });

            let payload = SessionHygienePayload::for_evidence(
                session_badges(&evidence, &catalogs),
                &evidence,
                &catalogs,
                "ready",
            );
            let value = serde_json::to_value(payload).unwrap();

            assert_eq!(
                value["badges"][0]["findingEvidence"],
                serde_json::json!({
                    "kind": "sessionOverdepth",
                    "maxRequestContextTokens": 450_000,
                    "depthCapTokens": 400_000
                })
            );
            assert_eq!(
                value["badges"][1]["findingEvidence"]["kind"],
                "modelOverthinking"
            );
            assert_eq!(
                value["badges"][1]["findingEvidence"]["tiers"][0]["tier"],
                "max"
            );
            assert_eq!(
                value["badges"][2]["findingEvidence"]["kind"],
                "overpoweredSubagents"
            );
            assert_eq!(
                value["badges"][3]["findingEvidence"]["kind"],
                "obsoleteModel"
            );
            assert_eq!(
                value["badges"][3]["findingEvidence"]["models"][0]["replacement"],
                "claude-opus-5"
            );
            assert_eq!(value["badges"][4]["findingEvidence"]["delegatedTurns"], 2);
            assert_eq!(
                value["badges"][5]["findingEvidence"],
                serde_json::json!({
                    "kind": "excessCacheRehydration",
                    "repeatedTokens": 135,
                    "paidTokens": 235,
                    "thresholdMultiple": 2.35
                })
            );
        }

        /// One unused MCP server, built-in tool, and skill each report a
        /// name and a cost summed across every priced observed model; a
        /// used resource of each kind is absent from the payload.
        #[test]
        fn for_evidence_prices_unused_resources_and_omits_used_ones() {
            let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
                agent: "claude-code".to_owned(),
                session_id: "unused-resources".to_owned(),
                kind: SourceKind::File,
                capabilities: SourceCapabilities::claude(),
            })
            .evidence(&TurnFacts::default());
            let catalogs = ReportCatalogs::default();

            let EvidenceValue::Complete(models) = &mut evidence.models else {
                panic!("synthetic model evidence must be complete");
            };
            models.by_model.insert(
                "claude-sonnet-5".to_owned(),
                ModelTokens {
                    turns: 2,
                    ..ModelTokens::default()
                },
            );
            models.by_model.insert(
                "claude-opus-5".to_owned(),
                ModelTokens {
                    turns: 3,
                    ..ModelTokens::default()
                },
            );

            let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
                panic!("synthetic context source evidence must be complete");
            };
            sources.mcp_servers.insert(
                "unused-server".to_owned(),
                LoadedSource {
                    description: None,
                    configured: true,
                    available: true,
                    injected: true,
                    invoked: false,
                    token_count: Some(100),
                    origin: EvidenceValue::Unsupported,
                },
            );
            sources.mcp_servers.insert(
                "used-server".to_owned(),
                LoadedSource {
                    description: None,
                    configured: true,
                    available: true,
                    injected: true,
                    invoked: true,
                    token_count: Some(100),
                    origin: EvidenceValue::Unsupported,
                },
            );
            sources.skills.insert(
                "unused-skill".to_owned(),
                LoadedSource {
                    description: None,
                    configured: true,
                    available: true,
                    injected: true,
                    invoked: false,
                    token_count: Some(80),
                    origin: EvidenceValue::Unsupported,
                },
            );
            sources.skills.insert(
                "used-skill".to_owned(),
                LoadedSource {
                    description: None,
                    configured: true,
                    available: true,
                    injected: true,
                    invoked: true,
                    token_count: Some(80),
                    origin: EvidenceValue::Unsupported,
                },
            );
            let mut definitions = BTreeMap::new();
            definitions.insert(
                "unused-tool".to_owned(),
                ToolDefinition {
                    tokens: 50,
                    invoked: false,
                    deferred: false,
                },
            );
            definitions.insert(
                "used-tool".to_owned(),
                ToolDefinition {
                    tokens: 50,
                    invoked: true,
                    deferred: false,
                },
            );
            sources.tool_definitions = EvidenceValue::Complete(definitions);

            let payload = SessionHygienePayload::for_evidence(
                session_badges(&evidence, &catalogs),
                &evidence,
                &catalogs,
                "ready",
            );
            let expected_cost = |tokens: f64| tokens * (2.0 * 0.3e-6 + 3.0 * 0.4e-6);
            assert_eq!(
                payload.unused_resources,
                Some(SessionUnusedResourcesPayload {
                    mcp_servers: vec![UnusedResourcePayload {
                        name: "unused-server".to_owned(),
                        cost_usd: Some(expected_cost(100.0)),
                    }],
                    built_in_tools: vec![UnusedResourcePayload {
                        name: "unused-tool".to_owned(),
                        cost_usd: Some(expected_cost(50.0)),
                    }],
                    skills: vec![UnusedResourcePayload {
                        name: "unused-skill".to_owned(),
                        cost_usd: Some(expected_cost(80.0)),
                    }],
                })
            );
        }
    }

    /// The webview's `SubagentMemberPayload` contract names these exact
    /// camelCase keys. A rename here would silently break that contract, so
    /// this test pins the wire shape rather than the Rust field names.
    #[test]
    fn subagent_member_serializes_with_camel_case_cost_tokens_and_model_runs() {
        let member = SubagentMember {
            agent: "claude-code".to_string(),
            subagent_id: "sub-1".to_string(),
            label: "Reviewer".to_string(),
            cost: Some(SessionCost {
                total_usd: 1.5,
                input_usd: 0.5,
                output_usd: 1.0,
                cache_read_usd: 0.0,
                cache_write_usd: 0.0,
            }),
            tokens: Some(BillableTokens {
                input_tokens: 10,
                output_tokens: 20,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            }),
            model_runs: vec![ModelRun {
                model: "claude-3-5-haiku-20241022".to_string(),
                thinking_mode: None,
            }],
            started_at_epoch: Some(1_760_000_000),
        };

        let value = serde_json::to_value(&member).expect("serialize");
        assert_eq!(value["agent"], "claude-code");
        assert_eq!(value["subagentId"], "sub-1");
        assert_eq!(value["label"], "Reviewer");
        assert_eq!(value["cost"]["totalUsd"], 1.5);
        assert_eq!(value["tokens"]["inputTokens"], 10);
        assert_eq!(value["modelRuns"][0]["model"], "claude-3-5-haiku-20241022");
        assert_eq!(value["startedAtEpoch"], 1_760_000_000);
    }

    /// A sub-agent with no metrics reports `null`, never a partial or zeroed
    /// figure — the same rule [`SessionAnalysis::cost`] follows.
    #[test]
    fn subagent_member_with_no_metrics_serializes_cost_and_tokens_as_null() {
        let member = SubagentMember {
            agent: "claude-code".to_string(),
            subagent_id: "sub-2".to_string(),
            label: "Sub-agent".to_string(),
            cost: None,
            tokens: None,
            model_runs: Vec::new(),
            started_at_epoch: None,
        };

        let value = serde_json::to_value(&member).expect("serialize");
        assert!(value["cost"].is_null());
        assert!(value["tokens"].is_null());
        assert_eq!(value["modelRuns"], serde_json::json!([]));
        assert!(value["startedAtEpoch"].is_null());
    }

    #[test]
    fn remediation_progress_preserves_outcome_origin_and_boundaries() {
        let payload = BurnCheckRemediationProgressPayload::from(
            crate::remediation::BurnCheckRemediationProgress {
                attempts: vec![crate::remediation::BurnCheckRemediationAttempt {
                    detector: DetectorId::OldModelUsage,
                    watch_id: "attempt".into(),
                    display: crate::remediation::BurnCheckDisplayFacts {
                        resource_kind: crate::remediation::BurnCheckResourceKind::Model,
                        resource_identity: Some("old".into()),
                        current_value: Some("old".into()),
                        replacement_value: Some("new".into()),
                        scope_kind: crate::remediation::BurnCheckScopeKind::Project,
                        quantity: None,
                        quantity_unit: None,
                        observation_count: 1,
                        first_observed_at_ms: 10,
                        last_observed_at_ms: 20,
                        estimate_method: None,
                        estimated_opportunity: None,
                        estimated_token_burn_basis_points: None,
                        verification_limit: crate::remediation::BurnCheckVerificationLimit::FreshEvidenceFromSameSourceAndTarget,
                    },
                    origin: crate::remediation::RemediationOrigin::Action,
                    lifecycle: crate::store::RemediationState::WaitingForPromptUse,
                    outcome: crate::remediation::BurnCheckRemediationOutcome::Failed,
                    verification: crate::remediation::VerificationStatus::Reserved,
                    savings: crate::remediation::SavingsStatus::Pending {
                        method_revision: None,
                    },
                    effective_boundary_ms: None,
                    verified_boundary_ms: None,
                    recurred_boundary_ms: None,
                }],
            },
        );

        let value = serde_json::to_value(payload).unwrap();
        assert_eq!(value["attempts"][0]["lifecycle"], "waitingForPromptUse");
        assert_eq!(value["attempts"][0]["outcome"], "failed");
        assert_eq!(value["attempts"][0]["origin"], "action");
        assert!(value["attempts"][0]["effectiveBoundaryMs"].is_null());
    }

    #[test]
    fn burn_check_sample_payload_exposes_no_session_identity() {
        let value = serde_json::to_value(BurnCheckSamplePayload {
            navigation_handle: "opaque-handle".to_owned(),
            title: "Sample session".to_owned(),
            agent: "codex".to_owned(),
            surface: BurnCheckSampleSurface::Cli,
            observed_at_ms: 1_760_000_000_000,
            repo: "demo".to_owned(),
            timestamp: "2026-09-14T12:00:00Z".to_owned(),
            is_active: false,
            has_fork_parent: false,
            fork_child_count: 0,
            cost: None,
            models: Vec::new(),
            model_runs: Vec::new(),
            hygiene: SessionHygienePayload {
                evidence_state: "pending",
                badges: Vec::new(),
                unused_resources: None,
            },
        })
        .expect("serialize");
        let encoded = value.to_string();

        assert_eq!(value["navigationHandle"], "opaque-handle");
        assert_eq!(value["surface"], "cli");
        assert!(!encoded.contains("sessionId"));
        assert!(!encoded.contains("environmentKey"));
        assert!(!encoded.contains("wslDistro"));
    }
}
