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

use antiburn_local::analysis::{
    ActiveSessionsSummary, EfficiencyTotals, EvidenceValue, FAST_SPEED_KEY, ModelRun,
    QuotaLimitKind, RepeatedContextAccounting, SessionCost, SessionEvidence,
};
use antiburn_local::insights::{
    BadgeId, BadgeStatus, DetectorId, DetectorStatus, EfficiencyReport, NotAssessedReason,
    QuotaPressureSection, ReportCatalogs, SessionBadge, model_family,
};
use antiburn_local::pricing::canonical_model_key;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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
    /// This is the session's total cost. The activity list and the export
    /// document show this figure.
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

/// One session's estimated share of a provider-reported allowance.
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
    pub window_id: String,
    pub resets_at: String,
    pub percent: f64,
}

/// Current per-session estimates, computed from local turns and live limits.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLimitAllocationSummary {
    pub allocations: Vec<SessionLimitAllocation>,
    pub generated_at: String,
}

/* -------------------------------------------------------------------------
 * Local insights report
 *
 * Mirrors of `antiburn_local::insights` report types. The payloads carry
 * counts, statuses, and structured reasons only — no transcript content,
 * no session identifiers, no evidence text. The category and reason names
 * are identifiers; the pane owns every reader-facing word.
 * ---------------------------------------------------------------------- */

/// Coverage of the report window: every discovered session, partitioned
/// by why it is or is not in the assessed cohort (FR-12).
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsCoveragePayload {
    /// Every session the window covers — the coverage denominator. It is
    /// always at least as large as the assessed cohort.
    pub discovered: u64,
    pub unknown_start: u64,
    pub pending: u64,
    pub processing: u64,
    pub failed: u64,
    pub unsupported: u64,
    pub stale: u64,
    pub ready: u64,
    pub actively_growing: u64,
    pub awaiting_provider_support: u64,
}

/// The exclusive status of one report category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InsightsCategoryStatus {
    Findings,
    Clean,
    NotAssessed,
}

/// One of the nine report categories, with its status and denominators.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsCategoryPayload {
    /// Stable category identifier, e.g. `sessionsOverDepth`.
    pub id: &'static str,
    /// Sessions whose capabilities let this category assess them.
    pub eligible: u64,
    /// Sessions this category actually assessed.
    pub assessed: u64,
    pub status: InsightsCategoryStatus,
    /// Sessions with at least one finding. `None` unless the status is
    /// `findings`.
    pub finding_sessions: Option<u64>,
    /// Structured reason identifier. `None` unless the status is
    /// `notAssessed`.
    pub not_assessed_reason: Option<&'static str>,
}

/// Deduplicated hits for one quota limit kind.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsQuotaLimitPayload {
    /// Stable limit-kind identifier, e.g. `rollingWindow`.
    pub kind: &'static str,
    pub hits: u64,
}

/// Bounded quota-pressure findings from transcript-attributable incidents.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsQuotaFindingsPayload {
    pub total_hits: u64,
    pub hard_hits: u64,
    pub warnings: u64,
    pub affected_session_count: u64,
    pub hits_by_limit_kind: Vec<InsightsQuotaLimitPayload>,
    /// Bounded set of transcript-attributed model names.
    pub affected_models: Vec<String>,
    pub affected_models_truncated: bool,
    pub first_observed_ts_ms: i64,
    pub last_observed_ts_ms: i64,
}

/// The quota-pressure section, outside the nine-category contract.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsQuotaPressurePayload {
    /// False exactly when the transcripts carry no quota evidence.
    pub assessed: bool,
    pub findings: Option<InsightsQuotaFindingsPayload>,
}

/// Bounded unknown record vocabulary from the local evidence cohort.
///
/// Type discriminators are schema vocabulary, not transcript content.
/// The engine limits each value to 256 bytes and each report to 16 values.
/// The counts are not exclusive. The engine bounds the diagnostic markers for both limit counts.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsUnrecognizedRecordsPayload {
    pub types: Vec<String>,
    pub types_truncated: bool,
    pub sessions_with_types: u64,
    pub inert_sessions: u64,
    pub evidence_bearing_sessions: u64,
    pub capped_sessions: u64,
    pub truncated_sessions: u64,
}

/// The thirty-day insights report, as the pane renders it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsReportPayload {
    /// The one environment scope this report covers (`native`, or
    /// `wsl:<distro>`). A report never combines scopes.
    pub environment_key: String,
    pub window_start_epoch: i64,
    pub window_end_epoch: i64,
    pub computed_at_epoch: i64,
    pub coverage: InsightsCoveragePayload,
    /// Size of the assessed cohort. Presented separately from the
    /// coverage denominator, never in its place.
    pub assessed_sessions: u64,
    pub categories: Vec<InsightsCategoryPayload>,
    pub quota_pressure: InsightsQuotaPressurePayload,
    pub unrecognized_records: InsightsUnrecognizedRecordsPayload,
    pub catalog_revision: i64,
}

/// Report calculation state plus the evidence backlog counts.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsightsStatusPayload {
    /// True while a report reduction runs.
    pub calculating: bool,
    /// Evidence rows that wait for processing in this report's scope.
    pub pending: u64,
    /// Evidence rows a worker is processing now, in this report's scope.
    pub processing: u64,
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

fn detector_id_str(id: DetectorId) -> &'static str {
    match id {
        DetectorId::SessionsOverDepth => "sessionsOverDepth",
        DetectorId::ModelOverthinking => "modelOverthinking",
        DetectorId::OverpoweredSubagents => "overpoweredSubagents",
        DetectorId::UnusedMcpServers => "unusedMcpServers",
        DetectorId::UnusedBuiltInTools => "unusedBuiltInTools",
        DetectorId::UnusedSkills => "unusedSkills",
        DetectorId::OldModelUsage => "oldModelUsage",
        DetectorId::OveruseOfFastMode => "overuseOfFastMode",
        DetectorId::CacheChurn => "cacheChurn",
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

fn quota_limit_kind_str(kind: QuotaLimitKind) -> &'static str {
    match kind {
        QuotaLimitKind::RollingWindow => "rollingWindow",
        QuotaLimitKind::Weekly => "weekly",
        QuotaLimitKind::ModelSpecific => "modelSpecific",
        QuotaLimitKind::WeightedUsage => "weightedUsage",
        QuotaLimitKind::RateLimit => "rateLimit",
    }
}

impl From<EfficiencyReport> for InsightsReportPayload {
    fn from(report: EfficiencyReport) -> Self {
        let coverage = &report.context.coverage;
        let categories = DetectorId::ALL
            .iter()
            .map(|&id| {
                let counts = report.detectors[id.index()];
                let (status, finding_sessions, not_assessed_reason) =
                    match &report.detector_statuses[id.index()] {
                        DetectorStatus::Findings(findings) => (
                            InsightsCategoryStatus::Findings,
                            Some(findings.finding_sessions),
                            None,
                        ),
                        DetectorStatus::Clean => (InsightsCategoryStatus::Clean, None, None),
                        DetectorStatus::NotAssessed(reason) => (
                            InsightsCategoryStatus::NotAssessed,
                            None,
                            Some(not_assessed_reason_str(*reason)),
                        ),
                    };
                InsightsCategoryPayload {
                    id: detector_id_str(id),
                    eligible: counts.eligible,
                    assessed: counts.assessed,
                    status,
                    finding_sessions,
                    not_assessed_reason,
                }
            })
            .collect();
        let quota_pressure = match &report.quota_pressure {
            QuotaPressureSection::NotAssessed => InsightsQuotaPressurePayload {
                assessed: false,
                findings: None,
            },
            QuotaPressureSection::Findings(findings) => InsightsQuotaPressurePayload {
                assessed: true,
                findings: Some(InsightsQuotaFindingsPayload {
                    total_hits: findings.total_hits,
                    hard_hits: findings.hard_hits,
                    warnings: findings.warnings,
                    affected_session_count: findings.affected_session_count,
                    hits_by_limit_kind: findings
                        .hits_by_limit_kind
                        .iter()
                        .map(|(&kind, &hits)| InsightsQuotaLimitPayload {
                            kind: quota_limit_kind_str(kind),
                            hits,
                        })
                        .collect(),
                    affected_models: findings.affected_models.iter().cloned().collect(),
                    affected_models_truncated: findings.affected_models_truncated,
                    first_observed_ts_ms: findings.first_observed_ts_ms,
                    last_observed_ts_ms: findings.last_observed_ts_ms,
                }),
            },
        };
        Self {
            environment_key: report.context.environment_key,
            window_start_epoch: report.context.window.start_epoch,
            window_end_epoch: report.context.window.end_epoch,
            computed_at_epoch: report.context.computed_at_epoch,
            coverage: InsightsCoveragePayload {
                discovered: coverage.discovered,
                unknown_start: coverage.unknown_start,
                pending: coverage.pending,
                processing: coverage.processing,
                failed: coverage.failed,
                unsupported: coverage.unsupported,
                stale: coverage.stale,
                ready: coverage.ready,
                actively_growing: coverage.actively_growing,
                awaiting_provider_support: coverage.awaiting_provider_support,
            },
            assessed_sessions: report.assessed_sessions,
            categories,
            quota_pressure,
            unrecognized_records: InsightsUnrecognizedRecordsPayload {
                types: report.unrecognized_records.types.into_iter().collect(),
                types_truncated: report.unrecognized_records.types_truncated,
                sessions_with_types: report.unrecognized_records.sessions_with_types,
                inert_sessions: report.unrecognized_records.inert_sessions,
                evidence_bearing_sessions: report.unrecognized_records.evidence_bearing_sessions,
                capped_sessions: report.unrecognized_records.capped_sessions,
                truncated_sessions: report.unrecognized_records.truncated_sessions,
            },
            catalog_revision: report.catalog_revision,
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

    mod insights {
        use std::collections::{BTreeMap, BTreeSet};

        use antiburn_local::analysis::{
            ContextEvidence, EvidenceSource, ModelTokens, RelationConfidence, RelationProvenance,
            RepeatedContext, SessionEvidenceAccumulator, SourceCapabilities, SourceKind,
            SubagentChild, TurnCounts, TurnFacts,
        };
        use antiburn_local::insights::{
            CoverageCounts, DetectorFindings, EfficiencyReportAccumulator, QuotaPressureFindings,
            ReportContext, ReportWindow, session_badges,
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

        /// The wire shape is the privacy contract: the payload names
        /// exactly these keys, and none of them can carry transcript
        /// content, session identifiers, or evidence text.
        #[test]
        fn the_report_payload_serializes_camel_case_counts_and_nothing_else() {
            let mut report = report();
            report.detector_statuses[0] = DetectorStatus::Findings(DetectorFindings {
                finding_sessions: 2,
                examples: Vec::new(),
            });
            report.quota_pressure = QuotaPressureSection::Findings(QuotaPressureFindings {
                hits_by_limit_kind: BTreeMap::from([(QuotaLimitKind::Weekly, 3)]),
                total_hits: 3,
                hard_hits: 1,
                warnings: 2,
                affected_session_count: 1,
                affected_session_examples: Vec::new(),
                affected_models: BTreeSet::from(["claude-3-5-haiku-20241022".to_owned()]),
                affected_models_truncated: false,
                first_observed_ts_ms: 1_000,
                last_observed_ts_ms: 2_000,
                observed_times_ms: vec![1_000, 2_000],
            });

            let value = serde_json::to_value(InsightsReportPayload::from(report)).unwrap();

            // `serde_json` maps iterate alphabetically, so the expected
            // lists are sorted.
            let top_keys: Vec<&str> = value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                top_keys,
                [
                    "assessedSessions",
                    "catalogRevision",
                    "categories",
                    "computedAtEpoch",
                    "coverage",
                    "environmentKey",
                    "quotaPressure",
                    "unrecognizedRecords",
                    "windowEndEpoch",
                    "windowStartEpoch",
                ]
            );
            assert_eq!(value["environmentKey"], "native");
            assert_eq!(value["coverage"]["unknownStart"], 0);

            let categories = value["categories"].as_array().unwrap();
            assert_eq!(categories.len(), 9);
            for category in categories {
                let keys: Vec<&str> = category
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect();
                assert_eq!(
                    keys,
                    [
                        "assessed",
                        "eligible",
                        "findingSessions",
                        "id",
                        "notAssessedReason",
                        "status",
                    ]
                );
            }
            assert_eq!(categories[0]["id"], "sessionsOverDepth");
            assert_eq!(categories[0]["status"], "findings");
            assert_eq!(categories[0]["findingSessions"], 2);
            assert_eq!(categories[8]["id"], "cacheChurn");
            assert_eq!(categories[8]["status"], "notAssessed");
            assert_eq!(categories[8]["notAssessedReason"], "noSessionsInWindow");

            let quota = value["quotaPressure"].as_object().unwrap();
            let quota_keys: Vec<&str> = quota.keys().map(String::as_str).collect();
            assert_eq!(quota_keys, ["assessed", "findings"]);
            let findings = quota["findings"].as_object().unwrap();
            let finding_keys: Vec<&str> = findings.keys().map(String::as_str).collect();
            assert_eq!(
                finding_keys,
                [
                    "affectedModels",
                    "affectedModelsTruncated",
                    "affectedSessionCount",
                    "firstObservedTsMs",
                    "hardHits",
                    "hitsByLimitKind",
                    "lastObservedTsMs",
                    "totalHits",
                    "warnings",
                ]
            );
            assert_eq!(findings["hitsByLimitKind"][0]["kind"], "weekly");

            let unrecognized = value["unrecognizedRecords"].as_object().unwrap();
            let unrecognized_keys: Vec<&str> = unrecognized.keys().map(String::as_str).collect();
            assert_eq!(
                unrecognized_keys,
                [
                    "cappedSessions",
                    "evidenceBearingSessions",
                    "inertSessions",
                    "sessionsWithTypes",
                    "truncatedSessions",
                    "types",
                    "typesTruncated",
                ]
            );
        }

        #[test]
        fn unrecognized_types_survive_dto_conversion() {
            let mut report = report();
            report.unrecognized_records.types =
                BTreeSet::from(["zeta".to_owned(), "alpha".to_owned()]);
            report.unrecognized_records.types_truncated = true;
            report.unrecognized_records.sessions_with_types = 4;
            report.unrecognized_records.inert_sessions = 3;
            report.unrecognized_records.evidence_bearing_sessions = 2;
            report.unrecognized_records.capped_sessions = 1;
            report.unrecognized_records.truncated_sessions = 1;

            let value = serde_json::to_value(InsightsReportPayload::from(report)).unwrap();
            assert_eq!(
                value["unrecognizedRecords"],
                serde_json::json!({
                    "types": ["alpha", "zeta"],
                    "typesTruncated": true,
                    "sessionsWithTypes": 4,
                    "inertSessions": 3,
                    "evidenceBearingSessions": 2,
                    "cappedSessions": 1,
                    "truncatedSessions": 1,
                })
            );
        }

        /// A quota section with no evidence serializes as not assessed,
        /// never as an empty findings shape a view could read as clean.
        #[test]
        fn an_unassessed_quota_section_serializes_with_null_findings() {
            let value = serde_json::to_value(InsightsReportPayload::from(report())).unwrap();
            assert_eq!(value["quotaPressure"]["assessed"], false);
            assert!(value["quotaPressure"]["findings"].is_null());
        }

        #[test]
        fn the_status_payload_serializes_camel_case() {
            let value = serde_json::to_value(InsightsStatusPayload {
                calculating: true,
                pending: 4,
                processing: 1,
            })
            .unwrap();
            let keys: Vec<&str> = value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(keys, ["calculating", "pending", "processing"]);
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
                    "evidenceState": "ready"
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
            models.fast_modes.insert(
                FAST_SPEED_KEY.to_owned(),
                TurnCounts {
                    main_loop: 0,
                    delegated: 2,
                },
            );

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
}
