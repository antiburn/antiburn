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

/// Cached metadata for one discovered session. The `source_*` pair identifies
/// the provider's source even if other session content is also cached locally.
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
    /// Most recent meaningful transcript activity, in unix seconds. A
    /// filesystem mtime is retained only when the source has no usable event
    /// timestamp; see [`Self::activity_source`].
    pub updated_at_epoch: Option<i64>,
    /// Fingerprint of the complete activity source set: parent size plus the
    /// identities and sizes of any child transcripts. Used as the cheap gate
    /// before re-reading transcript suffixes for semantic activity.
    pub activity_cursor: String,
    /// Provenance of `updated_at_epoch`: `event` for a meaningful transcript
    /// event, `mtime` for the filesystem fallback, or `unknown` for sources
    /// without a file heartbeat.
    pub activity_source: String,
    pub subagent_count: u32,
    /// The session this one was branched from, when the vendor records it.
    pub fork_parent_session_id: Option<String>,
    pub source_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceVersionState {
    pub source_fingerprint: Option<String>,
    pub source_generation: i64,
    pub started_at_epoch: Option<i64>,
}

/// Identity of one persisted activity cursor. The source label alone is not
/// enough: native and WSL environments can expose the same provider path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionActivityKey {
    pub environment_key: String,
    pub agent: String,
    pub source_label: String,
}

impl SessionActivityKey {
    pub fn new(
        environment_key: impl Into<String>,
        agent: impl Into<String>,
        source_label: impl Into<String>,
    ) -> Self {
        Self {
            environment_key: environment_key.into(),
            agent: agent.into(),
            source_label: source_label.into(),
        }
    }
}

/// Small persisted cursor used by the scanner before it reads transcript
/// suffixes. Kept separate from [`SessionRecord`] so callers that only need
/// the activity gate do not materialize titles, CWDs, or relationships.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionActivityState {
    pub activity_cursor: String,
    pub updated_at_epoch: Option<i64>,
    pub activity_source: String,
}

/// Engine-derived analysis for one session, as cached.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisRecord {
    pub key: SessionKey,
    /// Billable tokens per normalized model key, as camelCase JSON.
    pub model_breakdown_json: String,
    /// This JSON array puts parent model runs before sub-agent-only runs.
    pub inclusive_models_json: String,
    /// Serialized `InitialContextBreakdown`. `None` until an analysis pass
    /// fills it in.
    pub initial_context_json: Option<String>,
    /// Serialized `BTreeMap<String, SessionSummary>`, one entry per source
    /// (the parent transcript and every discovered child), keyed by
    /// `source_key`. Only a worker pass (one with a `turn_row_store`) writes
    /// this; every other pass leaves it `None`. The drilldown's rows-replay
    /// path reads it back to rebuild per-source metrics without a
    /// transcript — see `analysis::analysis_from_rows`.
    pub source_summaries_json: Option<String>,
    /// This fingerprint covers the parent transcript and its sub-agent transcripts.
    pub source_fingerprint: String,
    /// `antiburn_local::analysis::pricing_generation()` at the time of writing.
    pub pricing_generation: i64,
    pub analyzed_generation: i64,
    pub parser_revision: i64,
    pub analyzer_revision: i64,
    pub metrics_schema_revision: i64,
}

/// The persisted lifecycle state for one session's evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceStatus {
    Pending,
    Processing,
    Ready,
    Unsupported,
    Failed,
}

impl EvidenceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            EvidenceStatus::Pending => "pending",
            EvidenceStatus::Processing => "processing",
            EvidenceStatus::Ready => "ready",
            EvidenceStatus::Unsupported => "unsupported",
            EvidenceStatus::Failed => "failed",
        }
    }
}

impl std::str::FromStr for EvidenceStatus {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(EvidenceStatus::Pending),
            "processing" => Ok(EvidenceStatus::Processing),
            "ready" => Ok(EvidenceStatus::Ready),
            "unsupported" => Ok(EvidenceStatus::Unsupported),
            "failed" => Ok(EvidenceStatus::Failed),
            _ => Err("unknown evidence status"),
        }
    }
}

/// A persisted evidence row and its queue state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceRow {
    pub key: SessionKey,
    pub status: EvidenceStatus,
    pub analyzed_generation: Option<i64>,
    pub processed_fingerprint: Option<String>,
    pub parser_revision: Option<i64>,
    pub analyzer_revision: Option<i64>,
    pub evidence_schema_revision: Option<i64>,
    pub evidence_json: Option<String>,
    pub diagnostics_json: Option<String>,
    pub retry_count: i64,
    pub claim_fence: i64,
    pub claimed_at_epoch: Option<i64>,
    pub lease_expires_at_epoch: Option<i64>,
    pub next_attempt_at_epoch: Option<i64>,
    pub analyzed_at_epoch: Option<i64>,
    pub last_error: Option<String>,
    /// The claim fence a winning publish last stamped, or `None` when this
    /// session has never published. Unlike `claim_fence`, a later reclaim or
    /// requeue does not move this value — only the next winning publish
    /// does. See the `v21` migration in `store::schema` for the full
    /// contract.
    pub published_fence: Option<i64>,
}

/// The current revisions for both transcript-derived projections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionRevisions {
    pub parser_revision: i64,
    pub analyzer_revision: i64,
    pub metrics_schema_revision: i64,
    pub evidence_schema_revision: i64,
}

/// A claimed evidence row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceClaim {
    pub key: SessionKey,
    pub source_generation: i64,
    pub claim_fence: i64,
    pub retry_count: i64,
}

/// The two failure outcomes available after a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceFailure {
    Retry { next_attempt_at_epoch: i64 },
    Failed { revisions: ProjectionRevisions },
}

/// The two successful publication states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishedEvidence {
    Ready,
    Unsupported,
}

impl PublishedEvidence {
    pub fn as_str(self) -> &'static str {
        match self {
            PublishedEvidence::Ready => "ready",
            PublishedEvidence::Unsupported => "unsupported",
        }
    }
}

/// The evidence values produced by one analyzed pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceCompletion {
    pub claim_fence: i64,
    pub status: PublishedEvidence,
    pub evidence_schema_revision: i64,
    pub evidence_json: String,
    pub diagnostics_json: Option<String>,
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
    /// Most recent meaningful session activity, in unix seconds. Zero when
    /// the session never carried one, which puts it outside every window.
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

/// Where the notification window appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum NudgePlacement {
    /// Anchored under the tray icon, like the popover. macOS only; other
    /// platforms have no meaningful menu-bar anchor and always use `TopRight`.
    #[default]
    MenuBar,
    TopRight,
}

impl NudgePlacement {
    pub fn as_str(self) -> &'static str {
        match self {
            NudgePlacement::MenuBar => "menuBar",
            NudgePlacement::TopRight => "topRight",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "menuBar" => Some(NudgePlacement::MenuBar),
            "topRight" => Some(NudgePlacement::TopRight),
            _ => None,
        }
    }
}

/// When the menu bar shows the free-disk-space number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DiskSpaceDisplay {
    Always,
    /// The default: the number appears only while space is below the
    /// threshold, so a healthy machine keeps a quiet menu bar.
    #[default]
    WhenLow,
    Never,
}

impl DiskSpaceDisplay {
    pub fn as_str(self) -> &'static str {
        match self {
            DiskSpaceDisplay::Always => "always",
            DiskSpaceDisplay::WhenLow => "whenLow",
            DiskSpaceDisplay::Never => "never",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "always" => Some(DiskSpaceDisplay::Always),
            "whenLow" => Some(DiskSpaceDisplay::WhenLow),
            "never" => Some(DiskSpaceDisplay::Never),
            _ => None,
        }
    }
}

pub const MILESTONE_OPTIONS: [u8; 20] = [
    5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 85, 90, 95, 100,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Milestones(Vec<u8>);

impl Default for Milestones {
    fn default() -> Self {
        Self((10..=100).step_by(10).collect())
    }
}

impl Milestones {
    #[cfg(test)]
    pub fn none() -> Self {
        Self(Vec::new())
    }

    #[cfg(test)]
    pub fn all() -> Self {
        Self(MILESTONE_OPTIONS.to_vec())
    }

    pub fn selected(values: impl IntoIterator<Item = u8>) -> Self {
        Self(values.into_iter().collect()).normalized()
    }

    pub fn as_str(&self) -> String {
        self.0
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }

    pub fn parse(value: &str) -> Self {
        Self::selected(
            value
                .split(',')
                .filter_map(|part| part.trim().parse::<u8>().ok()),
        )
    }

    pub fn any(&self) -> bool {
        !self.0.is_empty()
    }

    pub fn contains(&self, threshold: u8) -> bool {
        self.0.binary_search(&threshold).is_ok()
    }

    fn normalized(mut self) -> Self {
        self.0
            .retain(|value| MILESTONE_OPTIONS.binary_search(value).is_ok());
        self.0.sort_unstable();
        self.0.dedup();
        self
    }
}

/// The providers whose meter the reader turned off, by canonical provider id.
///
/// Stores the *hidden* set, not the shown set. A provider a later build adds
/// is metered by default, and an id an older build wrote stays harmless.
///
/// Hidden means antiburn does not ask that provider for usage. The id is the
/// gate in `provider_usage::live::sources::collect`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HiddenMeters(Vec<String>);

impl HiddenMeters {
    pub fn selected(values: impl IntoIterator<Item = String>) -> Self {
        Self(values.into_iter().collect()).normalized()
    }

    pub fn as_str(&self) -> String {
        self.0.join(",")
    }

    pub fn parse(value: &str) -> Self {
        Self::selected(value.split(',').map(str::to_string))
    }

    pub fn contains(&self, provider: &str) -> bool {
        let provider = provider.trim().to_ascii_lowercase();
        self.0.binary_search(&provider).is_ok()
    }

    pub fn any(&self) -> bool {
        !self.0.is_empty()
    }

    fn normalized(mut self) -> Self {
        for value in &mut self.0 {
            *value = value.trim().to_ascii_lowercase();
        }
        // An empty id matches no provider and would hide nothing. Drop it, so
        // that a stray comma in the stored value cannot survive a round trip.
        self.0.retain(|value| !value.is_empty());
        self.0.sort_unstable();
        self.0.dedup();
        self
    }
}

/// Narrowest and widest activity windows the settings pane offers, in days.
/// These control presentation and recent discovery, not storage: sessions
/// already indexed follow the separate retention setting.
pub const MIN_ACTIVITY_DAYS: u32 = 1;
pub const MAX_ACTIVITY_DAYS: u32 = 14;
/// Days of activity a fresh install shows.
pub const DEFAULT_ACTIVITY_DAYS: u32 = 7;

/// Supported local session-data retention periods, in days.
pub const SESSION_DATA_RETENTION_DAYS_30: i32 = 30;
pub const SESSION_DATA_RETENTION_DAYS_90: i32 = 90;
/// Keep local session data until the reader deletes it.
pub const RETAIN_SESSION_DATA_FOREVER: i32 = -1;

/// Bounds for how long a nudge stays on screen before it dismisses itself.
pub const MIN_NUDGE_AUTO_DISMISS_SECS: u32 = 3;
pub const MAX_NUDGE_AUTO_DISMISS_SECS: u32 = 30;
pub const DEFAULT_NUDGE_AUTO_DISMISS_SECS: u32 = 10;

/// Bounds for the low-disk threshold. The pane offers presets; the wide range
/// is for hand-edited rows, clamped rather than rejected.
pub const MIN_DISK_THRESHOLD_GB: u32 = 5;
pub const MAX_DISK_THRESHOLD_GB: u32 = 2000;
pub const DEFAULT_DISK_THRESHOLD_GB: u32 = 50;

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
    /// Days to keep local session data. `-1` keeps it until explicit deletion.
    pub session_data_retention_days: i32,
    /// False until the first-run flow finishes; gates onboarding and the
    /// automatic first scan.
    pub onboarding_completed: bool,
    /// Whether the packaged app should register itself to start after sign-in.
    pub launch_at_login: bool,
    /// Whether the updater may install and restart on its own. Read by
    /// [`crate::updates::spawn_scheduler`], which is what makes it real.
    pub auto_update: bool,
    /// Whether background discovery and indexing are paused.
    ///
    /// Paused stops the *scheduler* only: an explicit rescan still runs, and
    /// everything already indexed stays browsable. See [`crate::scan`].
    pub discovery_paused: bool,
    /// The master switch for desktop notifications. Off means nothing is
    /// delivered, whatever the per-kind preferences below say.
    pub notifications_enabled: bool,
    /// Notify before an automatic update installs and restarts the app.
    pub notify_update_available: bool,
    /// Notify the first time a scan fails in this run of the app.
    pub notify_scan_failure: bool,
    /// Where the notification window appears. See [`NudgePlacement`].
    pub nudge_placement: NudgePlacement,
    /// Seconds a nudge stays before dismissing itself. Clamped to
    /// [`MIN_NUDGE_AUTO_DISMISS_SECS`]..=[`MAX_NUDGE_AUTO_DISMISS_SECS`].
    pub nudge_auto_dismiss_secs: u32,
    /// Whether a nudge may play the notification chime.
    pub notification_sound: bool,
    /// When the menu bar shows the free-disk-space number.
    pub disk_space_display: DiskSpaceDisplay,
    /// Free space, in binary GB, below which the disk counts as low.
    pub disk_space_threshold_gb: u32,
    /// Notify once each time free space drops below the threshold.
    pub notify_disk_space_low: bool,
    /// Five-hour-window usage milestones that notify. Only meaningful while
    /// the live usage source is enabled — milestones need a real limit.
    pub milestones_5h: Milestones,
    /// Weekly-window usage milestones that notify.
    pub milestones_weekly: Milestones,
    /// The per-feature online opt-out for live usage limits. On by default:
    /// fetching the reader's own usage from a provider they already use,
    /// with a credential they already hold, is an ordinary operation, not a
    /// risky one, so antiburn does not gate it behind a first-run choice.
    /// Turning this off is how a reader who wants no background traffic
    /// (a corporate or metered network, or plain preference) says so; it is
    /// not permission antiburn was waiting for. See
    /// [`AppSettings::live_usage_active`] for the second, unconditional gate
    /// that also has to hold before any of this runs.
    pub live_usage_enabled: bool,
    /// The providers whose meter the reader turned off. See [`HiddenMeters`].
    ///
    /// This is a narrower form of the switch above it: that one stops every
    /// provider, this one stops the named provider. Both stop requests, so a
    /// hidden provider also stops its milestone notifications.
    pub live_usage_hidden_providers: HiddenMeters,
    /// The analytics opt-out. On by default for a new install and preserved
    /// when an existing install has explicitly disabled it.
    pub analytics_enabled: bool,
    /// Whether the popover's usage-limits section is expanded to its
    /// per-provider rows, rather than collapsed to the chip row. Purely a
    /// display preference — it never gates a fetch — so it defaults open and
    /// stays wherever the reader last left it.
    pub overview_limits_expanded: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            activity_window_days: DEFAULT_ACTIVITY_DAYS,
            session_data_retention_days: RETAIN_SESSION_DATA_FOREVER,
            onboarding_completed: false,
            launch_at_login: true,
            auto_update: true,
            discovery_paused: false,
            // On by default, and the per-kind switches with them: each kind
            // is about something the reader would want to act on, and none
            // repeats. A notification surface that has to be found before it
            // says anything useful is a notification surface nobody finds.
            notifications_enabled: true,
            notify_update_available: true,
            notify_scan_failure: true,
            nudge_placement: NudgePlacement::default(),
            nudge_auto_dismiss_secs: DEFAULT_NUDGE_AUTO_DISMISS_SECS,
            notification_sound: true,
            disk_space_display: DiskSpaceDisplay::default(),
            disk_space_threshold_gb: DEFAULT_DISK_THRESHOLD_GB,
            notify_disk_space_low: true,
            milestones_5h: Milestones::default(),
            milestones_weekly: Milestones::default(),
            // On by default. This is antiburn's own agent asking a provider
            // the reader already uses about usage the reader already has a
            // credential for — ordinary traffic, not a risky one, so it does
            // not sit behind a first-run choice. The switch is the opt-out
            // for a reader who wants no background traffic at all.
            live_usage_enabled: true,
            // Empty: every provider antiburn can meter is metered. A reader
            // who wants fewer meters says so one provider at a time.
            live_usage_hidden_providers: HiddenMeters::default(),
            // Official analytics-capable builds start enabled. Source builds
            // omit the client unless the builder selects its Cargo feature.
            // `settings_from` keeps older completed installs disabled.
            analytics_enabled: true,
            // Open by default: a reader who has live limits at all should see
            // them without an extra click the first time they notice this.
            overview_limits_expanded: true,
        }
    }
}

impl AppSettings {
    /// Whether live usage collection may actually run right now.
    ///
    /// Two gates, both required, and both checked fresh on every pass rather
    /// than latched: [`Self::live_usage_enabled`] (on by default; the
    /// reader's opt-out) and [`Self::onboarding_completed`]. The onboarding
    /// half exists so the credential read this feature depends on — and, on
    /// macOS, the Keychain prompt that read can trigger — never happens
    /// before the reader has seen what this app is. Every call site that
    /// might collect or fetch a live usage source must go through this
    /// rather than reading `live_usage_enabled` alone; see
    /// `provider_usage::live::summarize` and `usage_alerts::background_pass`.
    pub fn live_usage_active(&self) -> bool {
        self.live_usage_enabled && self.onboarding_completed
    }

    /// Clamp anything a caller could get wrong. Called on both read and write,
    /// so a hand-edited database cannot produce an unrenderable window.
    pub fn normalized(mut self) -> Self {
        self.activity_window_days = self
            .activity_window_days
            .clamp(MIN_ACTIVITY_DAYS, MAX_ACTIVITY_DAYS);
        if !matches!(
            self.session_data_retention_days,
            SESSION_DATA_RETENTION_DAYS_30
                | SESSION_DATA_RETENTION_DAYS_90
                | RETAIN_SESSION_DATA_FOREVER
        ) {
            self.session_data_retention_days = RETAIN_SESSION_DATA_FOREVER;
        }
        self.nudge_auto_dismiss_secs = self
            .nudge_auto_dismiss_secs
            .clamp(MIN_NUDGE_AUTO_DISMISS_SECS, MAX_NUDGE_AUTO_DISMISS_SECS);
        self.disk_space_threshold_gb = self
            .disk_space_threshold_gb
            .clamp(MIN_DISK_THRESHOLD_GB, MAX_DISK_THRESHOLD_GB);
        self.milestones_5h = self.milestones_5h.normalized();
        self.milestones_weekly = self.milestones_weekly.normalized();
        self.live_usage_hidden_providers = self.live_usage_hidden_providers.normalized();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use antiburn_local::platform::environment::DiscoveryEnvironment;

    #[test]
    fn hidden_meters_survive_a_round_trip_through_their_stored_text() {
        let hidden = HiddenMeters::parse("openai,anthropic");
        assert_eq!(hidden.as_str(), "anthropic,openai");
        assert!(hidden.contains("openai"));
        assert!(hidden.any());
    }

    #[test]
    fn hidden_meters_tolerate_what_a_hand_edited_database_can_hold() {
        // Whitespace, case, duplicates, and a stray comma. Each one must
        // resolve to the same set rather than to a row that hides nothing.
        let hidden = HiddenMeters::parse(" OpenAI , openai,, ");
        assert_eq!(hidden.as_str(), "openai");
        assert!(hidden.contains("OPENAI"));

        // An empty value hides nothing, which is the default state.
        assert!(!HiddenMeters::parse("").any());
    }

    #[test]
    fn an_unknown_hidden_id_hides_nothing_it_should_not() {
        // A provider from a newer build, left behind by a downgrade. It stays
        // in the set, and matches no source this build registers.
        let hidden = HiddenMeters::parse("some-future-provider");
        assert!(!hidden.contains("anthropic"));
        assert!(!hidden.contains("openai"));
    }

    #[test]
    fn every_meter_is_shown_by_default() {
        // A provider a later build adds must be metered without the reader
        // asking. Storing the hidden set is what makes that true.
        assert!(!AppSettings::default().live_usage_hidden_providers.any());
    }

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
