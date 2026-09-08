//! The event payload: every field that may ever leave this machine, named once.
//!
//! This module is the enforcement point for the promise the Privacy pane makes.
//! [`Event`] has no free-form field — no map, no `serde_json::Value`, no
//! `String` a caller chooses the contents of — so there is nowhere for a path,
//! a repository name, a session title, or a credential to be put. Adding a
//! field here is the only way to widen what is sent, which makes widening it a
//! visible act in review rather than an accident at a call site.
//!
//! Counts are bucketed for the same reason. An exact session count, reported
//! repeatedly over weeks, is a fingerprint even without an identifier attached
//! to it; a bucket is the answer to "roughly how much is this being used"
//! without also answering "is this the same machine as last week".

use antiburn_local::model::AgentKind;
use serde::Deserialize;
#[cfg(feature = "analytics")]
use serde::Serialize;

/// The events this application may report. A closed set, by design: a
/// `&'static str` a caller passes in would put naming — and therefore scope —
/// back at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventName {
    /// The application started.
    AppLaunched,
    /// A new or explicitly restarted setup flow finished.
    #[cfg(feature = "analytics")]
    OnboardingFinished,
    /// A new or explicitly restarted setup flow became visible.
    #[cfg(feature = "analytics")]
    OnboardingStarted,
    /// One fixed onboarding step became visible.
    #[cfg(feature = "analytics")]
    OnboardingStepViewed,
    /// A discovery pass completed, with a bucketed count of what it found.
    #[cfg(feature = "analytics")]
    ScanCompleted,
    /// A preference changed. The key travels; the value never does.
    SettingToggled,
    /// A session was opened from the activity list.
    #[cfg(feature = "analytics")]
    SessionOpened,
    /// Something failed, by category. No message, no path, no backtrace.
    #[cfg(feature = "analytics")]
    ErrorOccurred,
    /// An Insights cohort contains unknown record vocabulary.
    UnrecognizedRecordsObserved,
    /// Claude's limit-reset diagnostic changed during this run.
    #[cfg(feature = "analytics")]
    ClaudeLimitResetObserved,
    /// A product surface became visible.
    #[cfg(feature = "analytics")]
    SurfaceViewed,
    /// A Settings pane became visible.
    #[cfg(feature = "analytics")]
    SettingsPaneViewed,
    /// A visible surface presented a terminal or timed-out data state.
    #[cfg(feature = "analytics")]
    SurfaceStateObserved,
    /// A provider state appeared on a visible usage surface.
    #[cfg(feature = "analytics")]
    LiveUsageStateObserved,
    /// An ordinary live-usage refresh published a changed coarse usage band.
    #[cfg(feature = "analytics")]
    UsageObserved,
    /// A learning pass produced a first or changed coarse limit factor.
    #[cfg(feature = "analytics")]
    LimitFactorObserved,
    /// One hourly summary describes the shell's coarse resource use.
    #[cfg(feature = "analytics")]
    ResourceUsageObserved,
}

/// Every event this application may send.
///
/// Test-only, because nothing in the running app iterates the catalog — but it
/// is the list the documentation test walks, so a variant missing from here
/// would make that test pass vacuously. `no_variant_escapes_the_catalog` below
/// closes that with an exhaustive match: adding a variant is a *compile* error
/// until it is listed here, and listing it here is what forces it into
/// `docs/analytics.md`.
#[cfg(all(test, feature = "analytics"))]
pub const EVERY_EVENT: &[EventName] = &[
    EventName::AppLaunched,
    EventName::OnboardingFinished,
    EventName::OnboardingStarted,
    EventName::OnboardingStepViewed,
    EventName::ScanCompleted,
    EventName::SettingToggled,
    EventName::SessionOpened,
    EventName::ErrorOccurred,
    EventName::UnrecognizedRecordsObserved,
    EventName::ClaudeLimitResetObserved,
    EventName::SurfaceViewed,
    EventName::SettingsPaneViewed,
    EventName::SurfaceStateObserved,
    EventName::LiveUsageStateObserved,
    EventName::UsageObserved,
    EventName::LimitFactorObserved,
    EventName::ResourceUsageObserved,
];

#[cfg(feature = "analytics")]
impl EventName {
    pub fn as_str(self) -> &'static str {
        match self {
            EventName::AppLaunched => "antiburn.app_launched",
            EventName::OnboardingFinished => "antiburn.onboarding_finished",
            EventName::OnboardingStarted => "antiburn.onboarding_started",
            EventName::OnboardingStepViewed => "antiburn.onboarding_step_viewed",
            EventName::ScanCompleted => "antiburn.scan_completed",
            EventName::SettingToggled => "antiburn.setting_toggled",
            EventName::SessionOpened => "antiburn.session_opened",
            EventName::ErrorOccurred => "antiburn.error_occurred",
            EventName::UnrecognizedRecordsObserved => "antiburn.unrecognized_records_observed",
            EventName::ClaudeLimitResetObserved => "antiburn.claude_limit_reset_observed",
            EventName::SurfaceViewed => "antiburn.surface_viewed",
            EventName::SettingsPaneViewed => "antiburn.settings_pane_viewed",
            EventName::SurfaceStateObserved => "antiburn.surface_state_observed",
            EventName::LiveUsageStateObserved => "antiburn.live_usage_state_observed",
            EventName::UsageObserved => "antiburn.usage_observed",
            EventName::LimitFactorObserved => "antiburn.limit_factor_observed",
            EventName::ResourceUsageObserved => "antiburn.resource_usage_observed",
        }
    }
}

/// One event, in the collector's wire envelope.
///
/// The shape matches the collector's documented `POST /v1/track` contract.
/// antiburn does not send `identify`, `userId`, or `orgId` values.
///
/// `sentAt` is absent here on purpose. It is stamped at delivery, not at
/// capture, so the server can correct for clock skew against
/// `originalTimestamp`; a queued event that waited an hour must not claim it
/// was sent when it was recorded.
#[cfg(feature = "analytics")]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// Always `desktop`, the surface class the collector partitions on.
    pub platform: &'static str,
    /// Per-event UUID. The collector's deduplication key, which is what makes
    /// re-delivering a batch after a failed flush safe.
    pub message_id: String,
    /// The rotating installation identifier. Random, not derived from any
    /// machine fact, and replaced every [`super::IDENTITY_LIFETIME_DAYS`].
    ///
    /// The contract's own client keeps this stable and shares it with device
    /// telemetry so rows join across surfaces. antiburn rotates it instead to
    /// prevent a long-term history from joining across rotation periods.
    pub anonymous_id: String,
    /// The run identifier the contract requires on every track call.
    ///
    /// Required by the collector, not optional: a payload without it is
    /// rejected outright, so this is the contract's floor rather than
    /// something antiburn chose to add. It is also the *least* persistent
    /// thing in the payload — its generator state lives in memory, is gone
    /// when the process exits, and is replaced after
    /// [`super::SESSION_TIMEOUT`] of inactivity. The generator cannot continue
    /// into another run. Queued event payloads include the captured value until
    /// delivery or withdrawal. The rotating [`Event::anonymous_id`] remains the
    /// longest-lived generator state here.
    pub session_id: String,
    /// Event name, in antiburn's own namespace.
    pub event: String,
    /// When it happened, RFC 3339 UTC.
    pub original_timestamp: String,
    /// The closed property set. Not a free-form map: see the module docs.
    pub properties: Properties,
    /// Install context, the two fields the contract stamps on every track.
    pub context: Context,
}

/// Everything an event may carry beyond its name. Closed, for the reason in
/// this module's own docs.
#[cfg(feature = "analytics")]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Properties {
    /// CPU architecture.
    pub arch: &'static str,
    /// A bucketed magnitude, where the event has one. Never an exact count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket: Option<&'static str>,
    /// A bare key or category from a closed vocabulary, never reader text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<&'static str>,
    /// A second dimension, where one event has two things worth separating —
    /// which agent's session was opened *and* whether it ran natively or
    /// under WSL. Same rules as [`Properties::label`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<&'static str>,
    /// Whether a surface exposure followed a user action or automatic restore.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<&'static str>,
    /// The short-window usage position returned with a Claude reset probe.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_band: Option<&'static str>,
    /// Whether the reset member was absent, null, malformed, or an object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_shape: Option<&'static str>,
    /// Claude's reset eligibility boolean, including absent field states.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eligibility: Option<&'static str>,
    /// Claude's ineligibility reason from an allowlisted vocabulary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ineligible_reason: Option<&'static str>,
    /// Claude's experiment-membership boolean, including absent field states.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experiment: Option<&'static str>,
    /// Claude's experiment arm from an allowlisted vocabulary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_arm: Option<&'static str>,
    /// Claude's reset availability boolean, including absent field states.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_availability: Option<&'static str>,
    /// Claude's weekly reset count reduced to a fixed bucket.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_per_week: Option<&'static str>,
    /// Whether Claude returned a next-availability timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_reset_available: Option<&'static str>,
    /// A learned limit factor's mapped plan name, or `unknown` or `other`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<&'static str>,
    /// A learned limit factor's coarse dollars-per-percent band.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub factor_band: Option<&'static str>,
    /// How far the meter and the factor's own estimate disagree, banded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub residual_band: Option<&'static str>,
    /// These bands describe process and local-store resource use for one bounded window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_usage: Option<super::resources::schema::ResourceUsageSummary>,
}

/// What a caller may attach to an event.
///
/// A struct rather than three positional `Option<&'static str>` arguments,
/// which are indistinguishable to the compiler and so silently swappable at a
/// call site. Naming them makes a mix-up a compile error instead of a wrong
/// value arriving in a dashboard nobody cross-checks.
#[derive(Debug, Clone, Copy, Default)]
pub struct Facts {
    /// A bucketed magnitude, never an exact count.
    pub bucket: Option<&'static str>,
    /// The primary dimension.
    pub label: Option<&'static str>,
    /// The secondary dimension, where the event has one.
    pub detail: Option<&'static str>,
    /// Whether a surface exposure followed a user action or automatic restore.
    pub origin: Option<&'static str>,
    pub usage_band: Option<&'static str>,
    pub response_shape: Option<&'static str>,
    pub eligibility: Option<&'static str>,
    pub ineligible_reason: Option<&'static str>,
    pub experiment: Option<&'static str>,
    pub reset_arm: Option<&'static str>,
    pub reset_availability: Option<&'static str>,
    pub resets_per_week: Option<&'static str>,
    pub next_reset_available: Option<&'static str>,
    pub plan: Option<&'static str>,
    pub factor_band: Option<&'static str>,
    pub residual_band: Option<&'static str>,
    #[cfg(feature = "analytics")]
    pub resource_usage: Option<super::resources::schema::ResourceUsageSummary>,
}

#[cfg(feature = "analytics")]
pub fn claude_limit_reset_facts(
    diagnostic: crate::provider_usage::live::anthropic::LimitResetDiagnostic,
) -> Facts {
    Facts {
        label: Some(diagnostic.request_outcome),
        usage_band: Some(diagnostic.usage_band),
        response_shape: Some(diagnostic.response_shape),
        eligibility: diagnostic.eligibility,
        ineligible_reason: diagnostic.ineligible_reason,
        experiment: diagnostic.experiment,
        reset_arm: diagnostic.arm,
        reset_availability: diagnostic.availability,
        resets_per_week: diagnostic.resets_per_week,
        next_reset_available: diagnostic.next_available,
        ..Facts::default()
    }
}

/// The install context the contract stamps on every track event.
#[cfg(feature = "analytics")]
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    /// The canonical `antiburn:<version>` string, the same form the
    /// contract's `desktop:<version>` takes for its own surface.
    pub app_version: String,
    /// Operating-system family — `macos`, `windows`, `linux`.
    pub os: &'static str,
}

/// An interaction the renderer may report.
///
/// This is the enforcement point for events fired from the webview, and it is
/// why there is no general "record an event" command. A command taking a name
/// and a map would move naming — and therefore scope — into TypeScript, where
/// nothing stops a future call site from passing a repository name. Here the
/// renderer may only name a shape that already exists, serde rejects anything
/// outside it, and every string that reaches the payload is a `&'static str`
/// this file wrote. Every doc comment below that says "closed" means this.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Interaction {
    /// A fixed onboarding step became visible.
    OnboardingStepViewed { step: OnboardingStep },
    /// A session was opened from the activity list. `agent` deserializes into
    /// the engine's own closed enum, so an unrecognised slug is a rejected
    /// command rather than a new value appearing in the data.
    SessionOpened {
        agent: AgentKind,
        environment: Environment,
    },
    /// A fixed product surface became visible.
    SurfaceViewed { surface: Surface, origin: Origin },
    /// A visible surface presented a data state.
    SurfaceStateObserved {
        surface: StateSurface,
        state: SurfaceState,
        origin: Origin,
    },
    /// A fixed Settings pane became visible.
    SettingsPaneViewed { pane: SettingsPane },
    /// A provider state appeared on a visible usage surface.
    LiveUsageStateObserved {
        provider: LiveUsageProvider,
        state: LiveUsageState,
        origin: Origin,
    },
}

/// A product surface whose visibility is measured.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Activity,
    SessionDetail,
    ProviderPreview,
    ChecksPreview,
    Hud,
    HudDetail,
    Settings,
}

/// A surface that can present a measured data state.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StateSurface {
    Activity,
    SessionDetail,
    ProviderPreview,
    ChecksPreview,
    Hud,
    HudDetail,
    Settings,
    Insights,
}

/// Why a surface became visible.
#[derive(Debug, Clone, Copy, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    User,
    Automatic,
}

/// A visible surface's coarse presentation state.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceState {
    Ready,
    Empty,
    Error,
    LoadingTimeout,
}

/// A pane in the fixed Settings window.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsPane {
    General,
    Appearance,
    Sources,
    Privacy,
    Notifications,
    Usage,
    Insights,
    About,
}

/// A provider with a supported live-usage source.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveUsageProvider {
    Anthropic,
    Openai,
    Google,
}

/// A provider state that a visible usage surface can present.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveUsageState {
    Fresh,
    Stale,
    Authentication,
    RateLimited,
    Unavailable,
    NoCredentials,
}

/// Which setup lifecycle is active.
#[cfg(feature = "analytics")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingFlow {
    New,
    Restart,
}

/// A screen in the fixed first-run flow.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingStep {
    Welcome,
    AgentsDetected,
    SourcesAndRepos,
    Ready,
}

/// Where an agent ran. Two values, and neither names anything: a WSL
/// distribution's *name* is chosen by the reader and is deliberately not
/// carried, unlike the contract's own client, which sends it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    Native,
    Wsl,
}

#[cfg(feature = "analytics")]
impl Interaction {
    /// The event and the facts this interaction becomes.
    pub fn resolve(self) -> (EventName, Facts) {
        match self {
            Interaction::OnboardingStepViewed { step } => (
                EventName::OnboardingStepViewed,
                Facts {
                    label: Some(step.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::SessionOpened { agent, environment } => (
                EventName::SessionOpened,
                Facts {
                    label: Some(agent.slug()),
                    detail: Some(environment.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::SurfaceViewed { surface, origin } => (
                EventName::SurfaceViewed,
                Facts {
                    label: Some(surface.as_str()),
                    detail: Some(origin.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::SurfaceStateObserved {
                surface,
                state,
                origin,
            } => (
                EventName::SurfaceStateObserved,
                Facts {
                    label: Some(surface.as_str()),
                    detail: Some(state.as_str()),
                    origin: Some(origin.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::SettingsPaneViewed { pane } => (
                EventName::SettingsPaneViewed,
                Facts {
                    label: Some(pane.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::LiveUsageStateObserved {
                provider, state, ..
            } => (
                EventName::LiveUsageStateObserved,
                Facts {
                    label: Some(provider.as_str()),
                    detail: Some(state.as_str()),
                    ..Facts::default()
                },
            ),
        }
    }
}

#[cfg(feature = "analytics")]
macro_rules! wire_values {
    ($type:ty, { $($variant:path => $value:literal),+ $(,)? }) => {
        impl $type {
            pub(crate) fn as_str(self) -> &'static str {
                match self {
                    $($variant => $value),+
                }
            }
        }
    };
}

#[cfg(feature = "analytics")]
wire_values!(Surface, {
    Surface::Activity => "activity",
    Surface::SessionDetail => "session_detail",
    Surface::ProviderPreview => "provider_preview",
    Surface::ChecksPreview => "checks_preview",
    Surface::Hud => "hud",
    Surface::HudDetail => "hud_detail",
    Surface::Settings => "settings",
});

#[cfg(feature = "analytics")]
wire_values!(StateSurface, {
    StateSurface::Activity => "activity",
    StateSurface::SessionDetail => "session_detail",
    StateSurface::ProviderPreview => "provider_preview",
    StateSurface::ChecksPreview => "checks_preview",
    StateSurface::Hud => "hud",
    StateSurface::HudDetail => "hud_detail",
    StateSurface::Settings => "settings",
    StateSurface::Insights => "insights",
});

#[cfg(feature = "analytics")]
wire_values!(Origin, {
    Origin::User => "user",
    Origin::Automatic => "automatic",
});

#[cfg(feature = "analytics")]
wire_values!(SurfaceState, {
    SurfaceState::Ready => "ready",
    SurfaceState::Empty => "empty",
    SurfaceState::Error => "error",
    SurfaceState::LoadingTimeout => "loading_timeout",
});

#[cfg(feature = "analytics")]
wire_values!(SettingsPane, {
    SettingsPane::General => "general",
    SettingsPane::Appearance => "appearance",
    SettingsPane::Sources => "sources",
    SettingsPane::Privacy => "privacy",
    SettingsPane::Notifications => "notifications",
    SettingsPane::Usage => "usage",
    SettingsPane::Insights => "insights",
    SettingsPane::About => "about",
});

#[cfg(feature = "analytics")]
wire_values!(LiveUsageProvider, {
    LiveUsageProvider::Anthropic => "anthropic",
    LiveUsageProvider::Openai => "openai",
    LiveUsageProvider::Google => "google",
});

#[cfg(feature = "analytics")]
impl LiveUsageProvider {
    /// The provider category for a canonical provider id, from
    /// [`crate::provider_usage::providers`].
    ///
    /// `antiburn.usage_observed` reports for the same providers
    /// `antiburn.live_usage_state_observed` does, so both read this one
    /// closed mapping rather than keeping two vocabularies in step by hand.
    /// An id outside the three known providers returns `None`, so a future
    /// source cannot silently widen what a label can say.
    pub fn from_provider_id(provider: &str) -> Option<LiveUsageProvider> {
        match provider {
            id if id == crate::provider_usage::providers::ANTHROPIC => {
                Some(LiveUsageProvider::Anthropic)
            }
            id if id == crate::provider_usage::providers::OPENAI => Some(LiveUsageProvider::Openai),
            id if id == crate::provider_usage::providers::GOOGLE => Some(LiveUsageProvider::Google),
            _ => None,
        }
    }
}

#[cfg(feature = "analytics")]
wire_values!(LiveUsageState, {
    LiveUsageState::Fresh => "fresh",
    LiveUsageState::Stale => "stale",
    LiveUsageState::Authentication => "authentication",
    LiveUsageState::RateLimited => "rate_limited",
    LiveUsageState::Unavailable => "unavailable",
    LiveUsageState::NoCredentials => "no_credentials",
});

#[cfg(feature = "analytics")]
wire_values!(OnboardingFlow, {
    OnboardingFlow::New => "new",
    OnboardingFlow::Restart => "restart",
});

#[cfg(feature = "analytics")]
impl OnboardingStep {
    fn as_str(self) -> &'static str {
        match self {
            OnboardingStep::Welcome => "welcome",
            OnboardingStep::AgentsDetected => "agents_detected",
            OnboardingStep::SourcesAndRepos => "sources_and_repos",
            OnboardingStep::Ready => "ready",
        }
    }
}

#[cfg(feature = "analytics")]
impl Environment {
    fn as_str(self) -> &'static str {
        match self {
            Environment::Native => "native",
            Environment::Wsl => "wsl",
        }
    }
}

/// Collapse a count into the bucket that ships in its place.
#[cfg(feature = "analytics")]
pub fn bucket(count: u64) -> &'static str {
    match count {
        0 => "0",
        1..=9 => "1-9",
        10..=49 => "10-49",
        50..=199 => "50-199",
        200..=999 => "200-999",
        _ => "1000+",
    }
}

/// Map a provider-reported plan name to the closed vocabulary
/// `antiburn.limit_factor_observed` sends.
///
/// The raw string never leaves this machine: it names a plan the reader
/// chose, which is exactly the kind of value this file's own module docs say
/// has nowhere to be put. `None` (no plan reported) and an empty or
/// all-whitespace string both become `unknown`; a plan name outside the
/// listed set becomes `other`, so a provider renaming or adding a plan tier
/// widens no vocabulary a reader was not already told about.
#[cfg(feature = "analytics")]
pub fn map_plan(plan: Option<&str>) -> &'static str {
    let Some(plan) = plan else {
        return "unknown";
    };
    match plan.trim().to_lowercase().as_str() {
        "" => "unknown",
        "free" => "free",
        "pro" => "pro",
        "max" => "max",
        "team" => "team",
        "enterprise" => "enterprise",
        "plus" => "plus",
        "business" => "business",
        "edu" => "edu",
        _ => "other",
    }
}

/// Reduce a learned dollars-per-percent factor to a power-of-two band.
///
/// Each band doubles the band below it, so a reader can group adjacent bands
/// later without a change to this vocabulary. Non-finite and non-positive
/// input map to the lowest band.
#[cfg(feature = "analytics")]
pub fn factor_band(usd_per_percent: f64) -> &'static str {
    if usd_per_percent.is_nan() || usd_per_percent < 1.0 {
        "under_1"
    } else if usd_per_percent < 2.0 {
        "1_to_under_2"
    } else if usd_per_percent < 4.0 {
        "2_to_under_4"
    } else if usd_per_percent < 8.0 {
        "4_to_under_8"
    } else if usd_per_percent < 16.0 {
        "8_to_under_16"
    } else if usd_per_percent < 32.0 {
        "16_to_under_32"
    } else if usd_per_percent < 64.0 {
        "32_to_under_64"
    } else if usd_per_percent < 128.0 {
        "64_to_under_128"
    } else {
        "128_and_over"
    }
}

/// Reduce a period's residual to a coarse band: how far the meter and the
/// factor's own estimate for the same span disagree.
///
/// `None` (no residual computed yet for this lane) is `unknown`, not `0`,
/// so a missing measurement is never read as a perfect one.
#[cfg(feature = "analytics")]
pub fn residual_band(residual: Option<(f64, f64)>) -> &'static str {
    let Some((meter_percent, estimated_percent)) = residual else {
        return "unknown";
    };
    let difference = (meter_percent - estimated_percent).abs();
    if difference <= 5.0 {
        "within_5"
    } else if difference <= 20.0 {
        "within_20"
    } else {
        "over_20"
    }
}

/// The surface class the collector partitions on. antiburn is a desktop
/// application, and the contract's vocabulary has one value for that.
#[cfg(feature = "analytics")]
pub const PLATFORM: &str = "desktop";

/// The operating-system family, as the payload reports it.
#[cfg(feature = "analytics")]
pub fn os_family() -> &'static str {
    std::env::consts::OS
}

/// The CPU architecture, as the payload reports it.
#[cfg(feature = "analytics")]
pub fn arch() -> &'static str {
    std::env::consts::ARCH
}

#[cfg(all(test, feature = "analytics"))]
mod tests {
    use super::super::resources::schema::{
        CoverageBand, CpuBand, IoRateBand, MemoryBand, ResourceUsageSummary,
    };
    use super::*;

    fn resource_summary() -> ResourceUsageSummary {
        ResourceUsageSummary {
            memory_mean: MemoryBand::From100ToUnder250Mib,
            memory_max: MemoryBand::From250ToUnder500Mib,
            memory_coverage: CoverageBand::Full,
            cpu_average: CpuBand::From10ToUnder25Percent,
            cpu_coverage: CoverageBand::Partial,
            read_rate_average: IoRateBand::Zero,
            read_coverage: CoverageBand::Full,
            write_rate_average: IoRateBand::Unavailable,
            write_coverage: CoverageBand::None,
            database_size: MemoryBand::From50ToUnder100Mib,
            database_coverage: CoverageBand::Full,
            wal_size: MemoryBand::Under50Mib,
            wal_coverage: CoverageBand::Partial,
        }
    }

    fn sample() -> Event {
        Event {
            platform: PLATFORM,
            message_id: "11111111-1111-4111-8111-111111111111".into(),
            anonymous_id: "22222222-2222-4222-8222-222222222222".into(),
            session_id: "33333333-3333-4333-8333-333333333333".into(),
            event: EventName::ScanCompleted.as_str().into(),
            original_timestamp: "2026-08-18T00:00:00Z".into(),
            properties: Properties {
                arch: "aarch64",
                bucket: Some("10-49"),
                label: Some("claude-code"),
                detail: Some("native"),
                origin: Some("user"),
                usage_band: Some("80_to_under_100"),
                response_shape: Some("object"),
                eligibility: Some("eligible"),
                ineligible_reason: Some("null"),
                experiment: Some("in_experiment"),
                reset_arm: Some("reset"),
                reset_availability: Some("available"),
                resets_per_week: Some("1"),
                next_reset_available: Some("present"),
                plan: Some("max"),
                factor_band: Some("2_to_under_4"),
                residual_band: Some("within_5"),
                resource_usage: Some(resource_summary()),
            },
            context: Context {
                app_version: "antiburn:1.2.3".into(),
                os: "macos",
            },
        }
    }

    #[test]
    fn buckets_never_reveal_an_exact_count() {
        assert_eq!(bucket(0), "0");
        assert_eq!(bucket(1), "1-9");
        assert_eq!(bucket(9), "1-9");
        assert_eq!(bucket(10), "10-49");
        assert_eq!(bucket(4_000), "1000+");
    }

    /// A recognized plan name maps case- and whitespace-insensitively; an
    /// absent plan is `unknown`; anything else, including an empty string, is
    /// `other` rather than the raw text.
    #[test]
    fn an_unlisted_plan_name_maps_to_other_rather_than_leaking_its_text() {
        assert_eq!(map_plan(None), "unknown");
        assert_eq!(map_plan(Some("")), "unknown");
        assert_eq!(map_plan(Some("   ")), "unknown");
        assert_eq!(map_plan(Some("Max")), "max");
        assert_eq!(map_plan(Some(" pro ")), "pro");
        assert_eq!(map_plan(Some("FREE")), "free");
        assert_eq!(map_plan(Some("team")), "team");
        assert_eq!(map_plan(Some("enterprise")), "enterprise");
        assert_eq!(map_plan(Some("plus")), "plus");
        assert_eq!(map_plan(Some("business")), "business");
        assert_eq!(map_plan(Some("edu")), "edu");
        assert_eq!(map_plan(Some("some-future-plan")), "other");
    }

    #[test]
    fn the_factor_band_boundaries_step_by_powers_of_two() {
        assert_eq!(factor_band(f64::NAN), "under_1");
        assert_eq!(factor_band(f64::NEG_INFINITY), "under_1");
        assert_eq!(factor_band(-1.0), "under_1");
        assert_eq!(factor_band(0.0), "under_1");
        assert_eq!(factor_band(0.99), "under_1");
        assert_eq!(factor_band(1.0), "1_to_under_2");
        assert_eq!(factor_band(1.99), "1_to_under_2");
        assert_eq!(factor_band(2.0), "2_to_under_4");
        assert_eq!(factor_band(3.99), "2_to_under_4");
        assert_eq!(factor_band(4.0), "4_to_under_8");
        assert_eq!(factor_band(7.99), "4_to_under_8");
        assert_eq!(factor_band(8.0), "8_to_under_16");
        assert_eq!(factor_band(15.99), "8_to_under_16");
        assert_eq!(factor_band(16.0), "16_to_under_32");
        assert_eq!(factor_band(31.99), "16_to_under_32");
        assert_eq!(factor_band(32.0), "32_to_under_64");
        assert_eq!(factor_band(63.99), "32_to_under_64");
        assert_eq!(factor_band(64.0), "64_to_under_128");
        assert_eq!(factor_band(127.99), "64_to_under_128");
        assert_eq!(factor_band(128.0), "128_and_over");
        assert_eq!(factor_band(1_000.0), "128_and_over");
        assert_eq!(factor_band(f64::INFINITY), "128_and_over");
    }

    #[test]
    fn the_residual_band_boundaries_match_the_absolute_difference() {
        assert_eq!(residual_band(None), "unknown");
        assert_eq!(residual_band(Some((50.0, 50.0))), "within_5");
        assert_eq!(residual_band(Some((50.0, 45.0))), "within_5");
        assert_eq!(residual_band(Some((50.0, 44.9))), "within_20");
        assert_eq!(residual_band(Some((50.0, 30.0))), "within_20");
        assert_eq!(residual_band(Some((50.0, 29.9))), "over_20");
        // Order does not matter: a factor that under- or over-estimates by
        // the same amount lands in the same band.
        assert_eq!(residual_band(Some((29.9, 50.0))), "over_20");
    }

    /// The complete wire surface, pinned.
    ///
    /// This test cannot read the Privacy pane, so it does not pretend to: it
    /// pins the exact set of keys that go out, and the pane's "Exactly what is
    /// sent" row has to be updated in the same change as any diff here. An
    /// earlier version of this test was named as though it checked the copy,
    /// which is worse than not checking it — the name asserted a guarantee
    /// nothing enforced, and five fields went unnamed in the pane for exactly
    /// that reason. Adding a field below without touching
    /// `apps/desktop/src/views/settings/PrivacyPane.tsx` is the bug this
    /// comment exists to prevent.
    #[test]
    fn the_wire_payload_is_exactly_these_twenty_seven_fields() {
        let json = serde_json::to_value(sample()).expect("serializes");
        let object = json.as_object().expect("an object");
        let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "anonymousId",
                "context",
                "event",
                "messageId",
                "originalTimestamp",
                "platform",
                "properties",
                "sessionId",
            ]
        );

        let mut property_keys: Vec<_> = object["properties"]
            .as_object()
            .expect("properties object")
            .keys()
            .map(String::as_str)
            .collect();
        property_keys.sort_unstable();
        assert_eq!(
            property_keys,
            [
                "arch",
                "bucket",
                "detail",
                "eligibility",
                "experiment",
                "factorBand",
                "ineligibleReason",
                "label",
                "nextResetAvailable",
                "origin",
                "plan",
                "resetArm",
                "resetAvailability",
                "resetsPerWeek",
                "residualBand",
                "resourceUsage",
                "responseShape",
                "usageBand",
            ]
        );

        let mut context_keys: Vec<_> = object["context"]
            .as_object()
            .expect("context object")
            .keys()
            .map(String::as_str)
            .collect();
        context_keys.sort_unstable();
        assert_eq!(context_keys, ["appVersion", "os"]);
    }

    /// `sentAt` belongs to delivery, not to capture. A queued event that
    /// waited an hour must not claim it was sent when it was recorded, so the
    /// stamp is spliced in at the moment of the request instead.
    #[test]
    fn a_captured_event_carries_no_sent_at() {
        let json = serde_json::to_string(&sample()).expect("serializes");
        assert!(!json.contains("sentAt"), "{json}");
    }

    /// Nothing about the reader's account, because there is no account. The
    /// contract's identify half is deliberately unimplemented.
    #[test]
    fn no_user_or_organisation_identity_is_ever_carried() {
        let json = serde_json::to_string(&sample()).expect("serializes");
        // `sessionId` is deliberately absent from this list. It identifies a
        // run of the process rather than a person.
        for forbidden in ["userId", "orgId", "email", "locale"] {
            assert!(!json.contains(forbidden), "{forbidden} in {json}");
        }
    }

    #[test]
    fn absent_optional_fields_are_omitted_rather_than_null() {
        let mut event = sample();
        event.properties.bucket = None;
        event.properties.label = None;
        event.properties.detail = None;
        event.properties.origin = None;
        event.properties.usage_band = None;
        event.properties.response_shape = None;
        event.properties.eligibility = None;
        event.properties.ineligible_reason = None;
        event.properties.experiment = None;
        event.properties.reset_arm = None;
        event.properties.reset_availability = None;
        event.properties.resets_per_week = None;
        event.properties.next_reset_available = None;
        event.properties.plan = None;
        event.properties.factor_band = None;
        event.properties.residual_band = None;
        event.properties.resource_usage = None;
        let json = serde_json::to_string(&event).expect("serializes");
        assert!(!json.contains("bucket"), "{json}");
        assert!(!json.contains("label"), "{json}");
        assert!(!json.contains("detail"), "{json}");
        assert!(!json.contains("\"origin\""), "{json}");
        assert!(!json.contains("\"plan\""), "{json}");
        assert!(!json.contains("factorBand"), "{json}");
        assert!(!json.contains("residualBand"), "{json}");
        assert!(!json.contains("resourceUsage"), "{json}");
    }

    #[test]
    fn resource_usage_has_only_the_typed_nested_allowlist() {
        let mut event = sample();
        event.event = EventName::ResourceUsageObserved.as_str().into();
        event.properties.resource_usage = Some(resource_summary());

        let json = serde_json::to_value(event).expect("serializes");
        let resource = json["properties"]["resourceUsage"]
            .as_object()
            .expect("resourceUsage object");
        let mut keys: Vec<_> = resource.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "cpuAverage",
                "cpuCoverage",
                "databaseCoverage",
                "databaseSize",
                "memoryCoverage",
                "memoryMax",
                "memoryMean",
                "readCoverage",
                "readRateAverage",
                "walCoverage",
                "walSize",
                "writeCoverage",
                "writeRateAverage",
            ]
        );
        assert_eq!(resource["cpuAverage"], "from10_to_under25_percent");
        assert_eq!(resource["readRateAverage"], "zero");
        assert_eq!(resource["writeRateAverage"], "unavailable");
    }

    /// The compiler, not a reviewer, keeps [`EVERY_EVENT`] complete.
    ///
    /// A new variant makes this match non-exhaustive, which fails the build at
    /// this line — the one place that then points at the catalog and at the
    /// document the event has to appear in.
    #[test]
    fn no_variant_escapes_the_catalog() {
        fn listed(event: EventName) -> bool {
            match event {
                EventName::AppLaunched
                | EventName::OnboardingFinished
                | EventName::OnboardingStepViewed
                | EventName::ScanCompleted
                | EventName::SettingToggled
                | EventName::SessionOpened
                | EventName::ErrorOccurred
                | EventName::UnrecognizedRecordsObserved
                | EventName::OnboardingStarted
                | EventName::SurfaceViewed
                | EventName::SettingsPaneViewed
                | EventName::SurfaceStateObserved
                | EventName::LiveUsageStateObserved
                | EventName::ClaudeLimitResetObserved
                | EventName::UsageObserved
                | EventName::LimitFactorObserved
                | EventName::ResourceUsageObserved => true,
            }
        }
        assert_eq!(
            EVERY_EVENT.len(),
            17,
            "a variant was added to the match above but not to EVERY_EVENT"
        );
        assert!(EVERY_EVENT.iter().copied().all(listed));
    }

    /// The public catalog in `docs/analytics.md` is a promise to a
    /// reader who cannot read this file, so it cannot be allowed to drift.
    /// Adding a variant above without documenting it fails here rather than
    /// shipping a document that quietly under-reports what is sent.
    #[test]
    fn the_documented_catalog_matches_the_code() {
        let doc = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/analytics.md"
        ));
        for name in EVERY_EVENT {
            assert!(
                doc.contains(name.as_str()),
                "{} is sent but is not in docs/analytics.md",
                name.as_str()
            );
        }
    }

    /// Every document that counts the fields counts the same number.
    ///
    /// Three of them do, in the reader's own words, and none is generated
    /// from this struct — so the count is a hand-copied number in four
    /// places. It has already drifted once: `detail` was added, three
    /// documents were updated to thirteen, and the privacy policy went on
    /// saying twelve. Counting from the payload here is the only way that
    /// stays true without somebody remembering.
    #[test]
    fn every_document_that_counts_the_fields_counts_the_same_number() {
        let json = serde_json::to_value(sample()).expect("serializes");
        let object = json.as_object().expect("an object");
        let counted = object
            .values()
            .map(|value| match value.as_object() {
                // `properties` and `context` are containers; their leaves are
                // what a reader counts, not the container itself.
                Some(nested) => nested.len(),
                None => 1,
            })
            .sum::<usize>()
            // `sentAt` is stamped at delivery rather than at capture, so it
            // is absent from the struct and present on the wire. A reader
            // counting what arrives counts it.
            + 1;

        let word = match counted {
            12 => "twelve",
            13 => "thirteen",
            14 => "fourteen",
            22 => "twenty-two",
            23 => "twenty-three",
            27 => "twenty-seven",
            other => panic!("no word for {other} fields; add one and update the documents"),
        };

        for path in [
            "/../../../docs/analytics.md",
            "/../../../docs/privacy-policy.md",
            "/../../../docs/support.md",
            "/../src/views/settings/PrivacyPane.tsx",
        ] {
            let doc = std::fs::read_to_string(env!("CARGO_MANIFEST_DIR").to_owned() + path)
                .unwrap_or_else(|error| panic!("{path}: {error}"));
            assert!(
                doc.to_lowercase().contains(&format!("{word} fields")),
                "{path} does not say \"{word} fields\"; the payload now has {counted}"
            );
        }
    }

    /// An interaction resolves to constants this file owns, never to
    /// anything the renderer supplied verbatim.
    #[test]
    fn an_interaction_resolves_to_this_files_own_constants() {
        let (name, facts) = Interaction::OnboardingStepViewed {
            step: OnboardingStep::SourcesAndRepos,
        }
        .resolve();
        assert_eq!(name, EventName::OnboardingStepViewed);
        assert_eq!(facts.label, Some("sources_and_repos"));

        let (name, facts) = Interaction::SessionOpened {
            agent: AgentKind::Claude,
            environment: Environment::Wsl,
        }
        .resolve();
        assert_eq!(name, EventName::SessionOpened);
        assert_eq!(facts.label, Some("claude-code"));
        assert_eq!(facts.detail, Some("wsl"));
        assert_eq!(facts.bucket, None);

        let (name, facts) = Interaction::SurfaceStateObserved {
            surface: StateSurface::ProviderPreview,
            state: SurfaceState::LoadingTimeout,
            origin: Origin::User,
        }
        .resolve();
        assert_eq!(name, EventName::SurfaceStateObserved);
        assert_eq!(facts.label, Some("provider_preview"));
        assert_eq!(facts.detail, Some("loading_timeout"));
        assert_eq!(facts.origin, Some("user"));

        let (name, facts) = Interaction::LiveUsageStateObserved {
            provider: LiveUsageProvider::Google,
            state: LiveUsageState::RateLimited,
            origin: Origin::User,
        }
        .resolve();
        assert_eq!(name, EventName::LiveUsageStateObserved);
        assert_eq!(facts.label, Some("google"));
        assert_eq!(facts.detail, Some("rate_limited"));
        assert_eq!(facts.origin, None);
    }

    /// `usage_observed` reads the same closed vocabulary
    /// `live_usage_state_observed` does, rather than trusting a new source's
    /// provider id outright. A provider this build does not recognize maps to
    /// `None`, so the caller skips it instead of inventing a fourth label.
    #[test]
    fn an_unrecognised_provider_id_has_no_usage_observed_label() {
        assert_eq!(
            LiveUsageProvider::from_provider_id(crate::provider_usage::providers::ANTHROPIC),
            Some(LiveUsageProvider::Anthropic)
        );
        assert_eq!(
            LiveUsageProvider::from_provider_id(crate::provider_usage::providers::OPENAI),
            Some(LiveUsageProvider::Openai)
        );
        assert_eq!(
            LiveUsageProvider::from_provider_id(crate::provider_usage::providers::GOOGLE),
            Some(LiveUsageProvider::Google)
        );
        assert_eq!(
            LiveUsageProvider::from_provider_id("some-future-provider"),
            None
        );
    }

    /// The renderer cannot invent a value. This is the whole reason the IPC
    /// edge takes a closed enum rather than a name and a map: a call site in
    /// TypeScript that passed a repository name would not compile into
    /// anything the collector could receive — it fails at the command
    /// boundary instead.
    #[test]
    fn an_unrecognised_interaction_is_refused_at_the_boundary() {
        let unknown_agent = serde_json::json!({
            "kind": "sessionOpened",
            "agent": "some-repository-name",
            "environment": "native",
        });
        assert!(serde_json::from_value::<Interaction>(unknown_agent).is_err());

        let unknown_shape = serde_json::json!({
            "kind": "somethingElse",
            "path": "/Users/someone/work",
        });
        assert!(serde_json::from_value::<Interaction>(unknown_shape).is_err());

        let unknown_environment = serde_json::json!({
            "kind": "sessionOpened",
            "agent": "claude-code",
            "environment": "Ubuntu-24.04",
        });
        assert!(serde_json::from_value::<Interaction>(unknown_environment).is_err());

        let extra_property = serde_json::json!({
            "kind": "surfaceViewed",
            "surface": "activity",
            "origin": "user",
            "repository": "private-name",
        });
        assert!(serde_json::from_value::<Interaction>(extra_property).is_err());

        let unknown_origin = serde_json::json!({
            "kind": "surfaceViewed",
            "surface": "activity",
            "origin": "background_poll",
        });
        assert!(serde_json::from_value::<Interaction>(unknown_origin).is_err());
    }

    #[test]
    fn every_event_name_sits_in_antiburns_own_namespace() {
        for name in EVERY_EVENT {
            assert!(name.as_str().starts_with("antiburn."), "{}", name.as_str());
        }
    }
}
