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
    ActiveSessionsSummary, EfficiencyTotals, ModelRun, QuotaLimitKind, SessionCost,
};
use antiburn_local::insights::{
    DetectorId, DetectorStatus, EfficiencyReport, NotAssessedReason, QuotaPressureSection,
};
use serde::{Deserialize, Serialize};

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
/// Only three of the five are producible from session observations, which is
/// all v1 has. [`Live`](Self::Live) and [`Detected`](Self::Detected) are
/// declared because they are part of the ratified presentation contract and a
/// view must render them correctly the day a separately-approved passive
/// evidence source starts emitting them — but nothing in this build ever does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
    /// **Reserved.** Same rationale as [`Live`](Self::Live): distinguishing
    /// "installed and signed in" from "used" needs evidence beyond a
    /// transcript.
    #[allow(dead_code)]
    Detected,
    /// Sessions were attributed to this provider, but they carry no token
    /// evidence at all — unanalyzed, or analyzed to nothing.
    Unknown,
}

/// Whether a provider's newest local evidence is recent enough to describe now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
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
    /// Sessions that contributed to this window. A session that used two
    /// providers is counted once under each.
    pub session_count: u32,
}

/// The three windows every provider is summarized over.
///
/// Independent, not nested: `week` is the trailing seven calendar days and
/// `month` starts at the first of the current month, so early in a month the
/// week reaches back further than the month does.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageWindows {
    pub today: ProviderUsageWindow,
    pub week: ProviderUsageWindow,
    pub month: ProviderUsageWindow,
}

/// Everything the usage surfaces show about one provider.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    /// Canonical provider id (`anthropic`, `openai`, `unknown`, …).
    pub provider: String,
    pub display_name: String,
    pub state: ProviderUsageState,
    pub staleness: ProviderUsageStaleness,
    pub windows: ProviderUsageWindows,
    /// ISO-8601 stamp of the newest session attributed to this provider.
    pub last_activity_at: Option<String>,
}

/// Local provider usage, as one snapshot.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageSummary {
    /// Providers with at least one session in the covered span, newest first.
    /// A provider the reader has not used lately is absent rather than zeroed.
    pub providers: Vec<ProviderUsage>,
    /// ISO-8601 stamp of the moment this snapshot was computed.
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
    /// Review date of the engine's bundled pricing catalog.
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
    /// Whether this build can send consented analytics at all. False in every
    /// development build and every build from a clean checkout, because the
    /// endpoint is injected and this tree has none — so the Privacy pane
    /// offers a disabled row that says why, rather than a live switch over
    /// nothing.
    pub analytics_supported: bool,
    /// Who receives those events, in the reader's own words. `None` when the
    /// build has no endpoint. Injected with the endpoint, never a literal in
    /// this repository.
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

/// Live provider usage, as one snapshot.
///
/// An empty `providers` list is the ordinary state — no source configured, or
/// none with anything to say — and the views render nothing rather than an
/// empty frame. `errors` is separate so that "nothing found" and "something
/// broke" never look alike.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveUsageSummary {
    pub providers: Vec<LiveProviderUsage>,
    pub errors: Vec<LiveUsageSourceError>,
    /// ISO-8601 stamp of the moment this snapshot was collected.
    pub generated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    mod insights {
        use std::collections::{BTreeMap, BTreeSet};

        use antiburn_local::insights::{
            CoverageCounts, DetectorFindings, EfficiencyReportAccumulator, QuotaPressureFindings,
            ReportContext, ReportWindow,
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
