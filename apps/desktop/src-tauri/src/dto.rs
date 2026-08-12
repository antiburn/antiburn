// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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

use antiburn_local::analysis::{ActiveSessionsSummary, SessionCost, SkillUse};
use serde::Serialize;

/// One row of the popover's activity list.
///
/// Mirrors the fields `LocalActivityEntry` needs, minus the presentation the
/// renderer derives itself (the cost pill's wording, the time label).
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
    /// Whether the transcript is still being written (heartbeat inside the
    /// engine's active-session window).
    pub is_active: bool,
    /// `cli`, `ide_desktop`, or `unknown`.
    pub surface: String,
    pub wsl_distro: Option<String>,
    pub title: Option<String>,
    /// Whether this session was branched from another local session.
    pub has_fork_parent: bool,
    /// How many local sessions were branched from this one.
    pub fork_child_count: u32,
    /// Sub-agents this session launched.
    pub subagent_count: u32,
    /// On-device cost estimate, or absent when no model in the session could be
    /// priced. Never a partial total.
    pub cost: Option<SessionCost>,
    /// Every model that contributed billable tokens, for the cost tooltip.
    pub models: Vec<String>,
    /// Idle-capped working time, when the session has been analyzed.
    pub active_secs: Option<u64>,
    pub duration_secs: Option<u64>,
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
    pub pattern_score: u8,
    /// 0..1 position on the orchestrator's active-time axis, when the spawn
    /// instant could be mapped onto it.
    pub spawn_progress: Option<f32>,
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

/// Everything the session-analytics surface needs for one session.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAnalytics {
    /// The engine's analysis, shaped as a one-session summary. `None` when the
    /// transcript could not be read at all.
    pub summary: Option<ActiveSessionsSummary>,
    /// False when the engine has only its generic adapter for this agent, which
    /// changes the empty state from "nothing happened" to "we cannot read this".
    pub supports_analytics: bool,
    pub title: Option<String>,
    pub wsl_distro: Option<String>,
    pub is_active: bool,
    pub cost: Option<SessionCost>,
    pub models: Vec<String>,
    pub skills: Vec<SkillUse>,
    pub orchestration: Option<OrchestrationStatus>,
    pub relations: Option<SessionRelations>,
    /// The transcript's own path, for the reveal action. Absent for sessions
    /// held in a vendor database rather than a file.
    pub source_path: Option<String>,
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
    /// Why the last scan failed, when it did.
    pub error: Option<String>,
    /// Per-agent bookkeeping, filled when the status is read through the
    /// command rather than pushed as an event (an event fires per agent, so
    /// re-reading the table for each would be pure noise).
    pub agents: Vec<AgentScanState>,
}

/// Where the app came from and what it is running against.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub app_version: String,
    /// Review date of the engine's bundled pricing catalog.
    pub pricing_catalog_version: String,
    /// Applied schema version of the local database.
    pub schema_version: i64,
    /// Absolute path of the app data directory, so a reader can find their own
    /// data without being told where it "should" be.
    pub data_dir: String,
    /// False in development builds, where the updater plugin is not installed.
    pub updates_supported: bool,
}
