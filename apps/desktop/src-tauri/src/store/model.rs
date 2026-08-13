// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The records the store reads and writes.
//!
//! These are storage shapes, not wire shapes: the IPC layer maps them into the
//! camelCase payloads in [`crate::dto`]. Keeping the two apart means a schema
//! change never silently alters what the webview receives.

use serde::{Deserialize, Serialize};

/// Identity of one local session: the execution environment it ran in, the
/// agent that produced it, and that agent's own id for it.
///
/// All three are needed. An agent's session ids are unique only within that
/// agent, and only within one environment — a WSL install of an agent may
/// deliberately reuse the ids of its native counterpart.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub environment_key: String,
    pub agent: String,
    pub session_id: String,
}

impl SessionKey {
    pub fn new(
        environment_key: impl Into<String>,
        agent: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            environment_key: environment_key.into(),
            agent: agent.into(),
            session_id: session_id.into(),
        }
    }

    /// The key for a session the webview named by agent, id, and (optionally)
    /// WSL distribution — which is all the identity a view ever carries.
    pub fn for_session(agent: &str, session_id: &str, wsl_distro: Option<&str>) -> Self {
        SessionKey::new(environment_key(wsl_distro), agent, session_id)
    }
}

/// The environment key discovery stamps a session with.
///
/// Mirrors `DiscoveryEnvironment::key`, which is what the scan actually writes;
/// a test pins the two together so a divergence cannot silently split a
/// session's rows in half.
pub fn environment_key(wsl_distro: Option<&str>) -> String {
    match wsl_distro
        .map(str::trim)
        .filter(|distro| !distro.is_empty())
    {
        Some(distro) => format!("wsl:{}", distro.to_ascii_lowercase()),
        None => "native".to_string(),
    }
}

/// Cached metadata for one discovered session. No transcript content: the
/// `source_*` pair points at the provider's own file rather than copying it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRecord {
    pub key: SessionKey,
    /// `file`, `inline`, or `providerDb` — how `source_label` should be read.
    pub source_kind: String,
    /// The transcript's path, or the vendor-store label for a non-file source.
    pub source_label: String,
    pub wsl_distro: Option<String>,
    pub title: Option<String>,
    pub title_source: Option<String>,
    pub cwd: Option<String>,
    /// `cli`, `ide_desktop`, or `unknown`.
    pub surface: String,
    /// Transcript heartbeat, in unix seconds.
    pub updated_at_epoch: Option<i64>,
    pub subagent_count: u32,
    /// The session this one was branched from, when the vendor records it.
    pub fork_parent_session_id: Option<String>,
}

/// Engine-derived analysis for one session, as cached.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisRecord {
    pub key: SessionKey,
    /// `SessionMetrics` as camelCase JSON — derived counts only.
    pub metrics_json: String,
    /// `SessionCost` components as camelCase JSON, or `None` when unpriced.
    pub cost_json: Option<String>,
    /// Billable tokens per normalized model key, as camelCase JSON.
    pub model_breakdown_json: String,
    pub active_secs: i64,
    pub duration_secs: i64,
    pub pattern_score: i64,
    /// `mtime:size` of the transcript the analysis was computed from.
    pub source_fingerprint: String,
    /// `antiburn_local::analysis::pricing_generation()` at the time of writing.
    pub pricing_generation: i64,
}

/// One session's token evidence, as the provider-usage aggregation reads it.
///
/// A projection rather than a record: the aggregation needs three columns out
/// of a two-table join and nothing else, and materializing whole
/// [`SessionRecord`]s and [`AnalysisRecord`]s to reach them would read
/// megabytes of metrics JSON per pass.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEvidenceRecord {
    /// The agent's discovery slug.
    pub agent: String,
    /// Transcript heartbeat, in unix seconds. Zero when the session never
    /// carried one, which puts it outside every window.
    pub updated_at_epoch: i64,
    /// Billable tokens per normalized model key, or `None` when the session has
    /// not been analyzed. Absence is "we do not know yet", never "zero".
    pub model_breakdown_json: Option<String>,
}

/// One local relationship between two sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationRecord {
    pub kind: RelationKind,
    /// The related session's id — a sub-agent transcript id, or a fork parent's
    /// session id.
    pub related_id: String,
    pub label: Option<String>,
}

/// What a [`RelationRecord`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    /// A sub-agent this session launched.
    Subagent,
    /// The session this one was branched from.
    ForkParent,
}

impl RelationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RelationKind::Subagent => "subagent",
            RelationKind::ForkParent => "forkParent",
        }
    }

    /// Parse the stored spelling. Named `parse` rather than `from_str` so it
    /// does not read as an unimplemented `FromStr`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "subagent" => Some(RelationKind::Subagent),
            "forkParent" => Some(RelationKind::ForkParent),
            _ => None,
        }
    }
}

/// A repository located on this machine, plus the user's include choice.
#[derive(Debug, Clone, PartialEq)]
pub struct RepositoryRecord {
    /// Stable identity — the canonical repository root, folded per platform.
    pub key: String,
    pub repo_name: String,
    pub full_name: String,
    /// `accessible`, `permission_denied`, `not_cloned`, or `disabled`.
    pub status: String,
    pub repo_root: Option<String>,
    pub suspected_path: Option<String>,
    pub worktree_count: u32,
    pub session_count: u32,
    pub wsl_distro: Option<String>,
    pub enabled: bool,
}

/// How the app renders itself. `System` follows the OS appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    pub fn as_str(self) -> &'static str {
        match self {
            ThemePreference::System => "system",
            ThemePreference::Light => "light",
            ThemePreference::Dark => "dark",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(ThemePreference::System),
            "light" => Some(ThemePreference::Light),
            "dark" => Some(ThemePreference::Dark),
            _ => None,
        }
    }
}

/// Narrowest and widest activity windows the settings pane offers, in days.
pub const MIN_ACTIVITY_DAYS: u32 = 1;
pub const MAX_ACTIVITY_DAYS: u32 = 30;
/// Days of activity a fresh install shows.
pub const DEFAULT_ACTIVITY_DAYS: u32 = 7;

/// Every user preference the app persists, as one value.
///
/// Stored key-by-key so adding a preference is additive; read and written as a
/// whole because that is how the settings window uses it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub theme: ThemePreference,
    /// Calendar days of activity the popover list shows.
    pub activity_window_days: u32,
    /// False until the first-run flow finishes; gates onboarding and the
    /// automatic first scan.
    pub onboarding_completed: bool,
    /// Recorded, and applied by the platform at next launch. See
    /// [`crate::commands::set_settings`] for why it is inert today.
    pub launch_at_login: bool,
    /// Whether the updater may check on its own. Read by
    /// [`crate::updates::spawn_scheduler`], which is what makes it real.
    pub auto_update: bool,
    /// Whether background discovery and indexing are paused.
    ///
    /// Paused stops the *scheduler* only: an explicit rescan still runs, and
    /// everything already indexed stays browsable. See [`crate::scan`].
    pub discovery_paused: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            activity_window_days: DEFAULT_ACTIVITY_DAYS,
            onboarding_completed: false,
            launch_at_login: false,
            auto_update: true,
            discovery_paused: false,
        }
    }
}

impl AppSettings {
    /// Clamp anything a caller could get wrong. Called on both read and write,
    /// so a hand-edited database cannot produce an unrenderable window.
    pub fn normalized(mut self) -> Self {
        self.activity_window_days = self
            .activity_window_days
            .clamp(MIN_ACTIVITY_DAYS, MAX_ACTIVITY_DAYS);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use antiburn_local::platform::environment::DiscoveryEnvironment;

    #[test]
    fn the_environment_key_matches_the_one_discovery_stamps_sessions_with() {
        assert_eq!(environment_key(None), DiscoveryEnvironment::Native.key());

        let wsl = DiscoveryEnvironment::Wsl {
            distribution: "Ubuntu".to_string(),
            user: "avery".to_string(),
        };
        assert_eq!(environment_key(Some("Ubuntu")), wsl.key());
        // Distribution names are case-insensitive in practice, and both sides
        // fold them the same way.
        assert_eq!(environment_key(Some("UBUNTU")), wsl.key());
    }

    #[test]
    fn a_blank_distribution_is_the_native_environment() {
        assert_eq!(environment_key(Some("")), "native");
        assert_eq!(environment_key(Some("   ")), "native");
    }

    #[test]
    fn a_session_key_carries_all_three_identity_parts() {
        let key = SessionKey::for_session("claude-code", "abc", Some("Ubuntu"));
        assert_eq!(key.environment_key, "wsl:ubuntu");
        assert_eq!(key.agent, "claude-code");
        assert_eq!(key.session_id, "abc");
        assert_ne!(key, SessionKey::for_session("claude-code", "abc", None));
    }
}
