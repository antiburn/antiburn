//! The event payload: every field that may ever leave this machine, named once.
//!
//! This module enforces the promise the Privacy pane makes. Almost every
//! [`Event`] field is a `&'static str` the caller passes in. No caller can put
//! a path, a repository name, a session title, or a credential here. There
//! are two bounded exceptions, not a free-form map or a `serde_json::Value`.
//! `properties.resourceUsage` is a closed nested struct.
//! `properties.unrecognizedTypes` is a sanitized `Vec<String>` of transcript
//! record type names. `analytics::sanitize_unrecognized_types` caps its count
//! and length and filters it to a fixed character set before it reaches this
//! struct. Adding a field here is the only way to widen what antiburn sends.
//! That makes widening it a visible act in review, not an accident at a call
//! site.
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
    /// A private CLI operation completed.
    CliOperationCompleted,
    /// The application started.
    AppLaunched,
    /// A discovery pass completed, with a bucketed count of what it found.
    #[cfg(feature = "analytics")]
    ScanCompleted,
    /// A preference changed. The key travels; the value never does.
    SettingToggled,
    /// The reader withdrew analytics consent.
    #[cfg(feature = "analytics")]
    AnalyticsOptedOut,
    /// A session was opened from the activity list.
    #[cfg(feature = "analytics")]
    SessionOpened,
    /// Something failed, by category. No message, no path, no backtrace.
    #[cfg(feature = "analytics")]
    ErrorOccurred,
    /// An assessed cohort contains unknown record vocabulary.
    UnrecognizedRecordsObserved,
    /// Claude's limit-reset diagnostic changed during this run.
    #[cfg(feature = "analytics")]
    ClaudeLimitResetObserved,
    /// A Claude login check or CLI refresh reached a changed closed outcome.
    #[cfg(feature = "analytics")]
    ClaudeLoginObserved,
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
    /// An Auto Fix review reached a closed preparation outcome.
    #[cfg(feature = "analytics")]
    BurnCheckAutoFixReviewed,
    /// The reader confirmed one reviewed Auto Fix operation.
    #[cfg(feature = "analytics")]
    BurnCheckAutoFixConfirmed,
    /// A confirmed Auto Fix operation reached a closed result.
    #[cfg(feature = "analytics")]
    BurnCheckAutoFixCompleted,
    /// A fix prompt request reached a closed preparation outcome.
    #[cfg(feature = "analytics")]
    BurnCheckPromptPrepared,
    /// A prepared fix prompt was copied to the clipboard.
    #[cfg(feature = "analytics")]
    BurnCheckPromptCopied,
    /// A verified or recurred result appeared during a deliberate exposure.
    #[cfg(feature = "analytics")]
    BurnCheckOutcomeObserved,
    /// A new installed-check input preparation was saved.
    BurnCheckInputPrepared,
    /// An explicit installed-check definition open request settled.
    BurnCheckDefinitionOpened,
    /// An explicit installed-check directory open request settled.
    BurnCheckDirectoryOpened,
    /// The Sessions sidebar filter changed to a different selection.
    #[cfg(feature = "analytics")]
    SessionFilterSelected,
    /// The contextual Sessions filters changed.
    #[cfg(feature = "analytics")]
    SessionFiltersChanged,
    /// A remote-host prerequisite check reached a closed result.
    #[cfg(feature = "analytics")]
    RemoteHostConnectionChecked,
    /// A configured remote host was added, edited, or removed.
    #[cfg(feature = "analytics")]
    RemoteHostChanged,
    /// A remote synchronization pass reached a terminal result.
    #[cfg(feature = "analytics")]
    RemoteSyncCompleted,
    /// An explicit project folder action completed.
    #[cfg(feature = "analytics")]
    ProjectFolderAction,
    /// A memory action in the Memories view settled.
    #[cfg(feature = "analytics")]
    MemoryAction,
    /// An assessed cohort's quota-pressure section has assessed incidents,
    /// with a bucketed hit count per limit kind.
    QuotaIncidentsObserved,
    /// An assessed cohort's provider-incidents section has assessed
    /// incidents, with a bucketed hit count per incident kind.
    ProviderIncidentsObserved,
    /// The evidence worker published a session whose transcript gained an
    /// incident within the last two hours that its previous published
    /// evidence did not carry.
    ProviderIncidentsIngested,
    /// A closed quota window reported its coarse estimate-accuracy facts.
    #[cfg(feature = "analytics")]
    QuotaWindowClosed,
    #[cfg(feature = "analytics")]
    NavigationHistoryMoved,
    #[cfg(feature = "analytics")]
    AppSearchOpened,
    #[cfg(feature = "analytics")]
    AppSearchResultOpened,
    /// A production promotion panel was viewed or used.
    #[cfg(feature = "analytics")]
    PromotionInteraction,
    /// A reader opened or used the pinned usage limits controls.
    #[cfg(feature = "analytics")]
    UsageLimitsInteraction,
    /// The reader saved a different application interface size preset.
    InterfaceScaleChanged,
    /// A fixed onboarding screen became visible.
    #[cfg(feature = "analytics")]
    OnboardingScreenViewed,
    /// The reader took an explicit action during onboarding.
    #[cfg(feature = "analytics")]
    OnboardingActionTaken,
    /// The requested Cloud setup reached its unavailable state.
    #[cfg(feature = "analytics")]
    OnboardingCloudOutcome,
    /// The reader completed the local onboarding flow.
    OnboardingCompleted,
    /// The first-run Overview reached a fixed funnel step.
    #[cfg(feature = "analytics")]
    FirstRunStepReached,
    /// A deliberate action in the first-run Overview completed.
    #[cfg(feature = "analytics")]
    FirstRunAction,
    /// The first-run Overview's result first showed.
    #[cfg(feature = "analytics")]
    FirstRunFinished,
    /// A model-check master setting or provider operation completed.
    #[cfg(feature = "analytics")]
    ModelCheckLifecycle,
    /// One check's enabled preference completed a persisted state transition.
    #[cfg(feature = "analytics")]
    CheckEnablementSaved,
    /// A progress step's settings became visible: the first-run "Show
    /// settings" disclosure opened.
    #[cfg(feature = "analytics")]
    StepSettingsViewed,
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
    EventName::CliOperationCompleted,
    EventName::AppLaunched,
    EventName::ScanCompleted,
    EventName::SettingToggled,
    EventName::AnalyticsOptedOut,
    EventName::SessionOpened,
    EventName::ErrorOccurred,
    EventName::UnrecognizedRecordsObserved,
    EventName::ClaudeLimitResetObserved,
    EventName::ClaudeLoginObserved,
    EventName::SurfaceViewed,
    EventName::SettingsPaneViewed,
    EventName::SurfaceStateObserved,
    EventName::LiveUsageStateObserved,
    EventName::UsageObserved,
    EventName::LimitFactorObserved,
    EventName::ResourceUsageObserved,
    EventName::BurnCheckAutoFixReviewed,
    EventName::BurnCheckAutoFixConfirmed,
    EventName::BurnCheckAutoFixCompleted,
    EventName::BurnCheckPromptPrepared,
    EventName::BurnCheckPromptCopied,
    EventName::BurnCheckOutcomeObserved,
    EventName::BurnCheckInputPrepared,
    EventName::BurnCheckDefinitionOpened,
    EventName::BurnCheckDirectoryOpened,
    EventName::SessionFilterSelected,
    EventName::SessionFiltersChanged,
    EventName::RemoteHostConnectionChecked,
    EventName::RemoteHostChanged,
    EventName::RemoteSyncCompleted,
    EventName::ProjectFolderAction,
    EventName::MemoryAction,
    EventName::QuotaIncidentsObserved,
    EventName::ProviderIncidentsObserved,
    EventName::ProviderIncidentsIngested,
    EventName::QuotaWindowClosed,
    EventName::NavigationHistoryMoved,
    EventName::AppSearchOpened,
    EventName::AppSearchResultOpened,
    EventName::PromotionInteraction,
    EventName::UsageLimitsInteraction,
    EventName::InterfaceScaleChanged,
    EventName::OnboardingScreenViewed,
    EventName::OnboardingActionTaken,
    EventName::OnboardingCloudOutcome,
    EventName::OnboardingCompleted,
    EventName::FirstRunStepReached,
    EventName::FirstRunAction,
    EventName::FirstRunFinished,
    EventName::ModelCheckLifecycle,
    EventName::CheckEnablementSaved,
    EventName::StepSettingsViewed,
];

#[cfg(feature = "analytics")]
impl EventName {
    pub fn as_str(self) -> &'static str {
        match self {
            EventName::CliOperationCompleted => "antiburn.cli_operation_completed",
            EventName::AppLaunched => "antiburn.app_launched",
            EventName::ScanCompleted => "antiburn.scan_completed",
            EventName::SettingToggled => "antiburn.setting_toggled",
            EventName::AnalyticsOptedOut => "antiburn.analytics_opted_out",
            EventName::SessionOpened => "antiburn.session_opened",
            EventName::ErrorOccurred => "antiburn.error_occurred",
            EventName::UnrecognizedRecordsObserved => "antiburn.unrecognized_records_observed",
            EventName::ClaudeLimitResetObserved => "antiburn.claude_limit_reset_observed",
            EventName::ClaudeLoginObserved => "antiburn.claude_login_observed",
            EventName::SurfaceViewed => "antiburn.surface_viewed",
            EventName::SettingsPaneViewed => "antiburn.settings_pane_viewed",
            EventName::SurfaceStateObserved => "antiburn.surface_state_observed",
            EventName::LiveUsageStateObserved => "antiburn.live_usage_state_observed",
            EventName::UsageObserved => "antiburn.usage_observed",
            EventName::LimitFactorObserved => "antiburn.limit_factor_observed",
            EventName::ResourceUsageObserved => "antiburn.resource_usage_observed",
            EventName::BurnCheckAutoFixReviewed => "antiburn.burn_check_auto_fix_reviewed",
            EventName::BurnCheckAutoFixConfirmed => "antiburn.burn_check_auto_fix_confirmed",
            EventName::BurnCheckAutoFixCompleted => "antiburn.burn_check_auto_fix_completed",
            EventName::BurnCheckPromptPrepared => "antiburn.burn_check_prompt_prepared",
            EventName::BurnCheckPromptCopied => "antiburn.burn_check_prompt_copied",
            EventName::BurnCheckOutcomeObserved => "antiburn.burn_check_outcome_observed",
            EventName::BurnCheckInputPrepared => "antiburn.burn_check_input_prepared",
            EventName::BurnCheckDefinitionOpened => "antiburn.burn_check_definition_opened",
            EventName::BurnCheckDirectoryOpened => "antiburn.burn_check_directory_opened",
            EventName::ProjectFolderAction => "antiburn.project_folder_action",
            EventName::MemoryAction => "antiburn.memory_action",
            EventName::SessionFilterSelected => "antiburn.session_filter_selected",
            EventName::SessionFiltersChanged => "antiburn.session_filters_changed",
            EventName::RemoteHostConnectionChecked => "antiburn.remote_host_connection_checked",
            EventName::RemoteHostChanged => "antiburn.remote_host_changed",
            EventName::RemoteSyncCompleted => "antiburn.remote_sync_completed",
            EventName::QuotaIncidentsObserved => "antiburn.quota_incidents_observed",
            EventName::ProviderIncidentsObserved => "antiburn.provider_incidents_observed",
            EventName::ProviderIncidentsIngested => "antiburn.provider_incidents_ingested",
            EventName::QuotaWindowClosed => "antiburn.quota_window_closed",
            EventName::NavigationHistoryMoved => "antiburn.navigation_history_moved",
            EventName::AppSearchOpened => "antiburn.app_search_opened",
            EventName::AppSearchResultOpened => "antiburn.app_search_result_opened",
            EventName::PromotionInteraction => "antiburn.promotion_interaction",
            EventName::UsageLimitsInteraction => "antiburn.usage_limits_interaction",
            EventName::InterfaceScaleChanged => "antiburn.interface_scale_changed",
            EventName::OnboardingScreenViewed => "antiburn.onboarding_screen_viewed",
            EventName::OnboardingActionTaken => "antiburn.onboarding_action_taken",
            EventName::OnboardingCloudOutcome => "antiburn.onboarding_cloud_outcome",
            EventName::OnboardingCompleted => "antiburn.onboarding_completed",
            EventName::FirstRunStepReached => "antiburn.first_run_step_reached",
            EventName::FirstRunAction => "antiburn.first_run_action",
            EventName::FirstRunFinished => "antiburn.first_run_finished",
            EventName::ModelCheckLifecycle => "antiburn.ignored_instruction_lifecycle",
            EventName::CheckEnablementSaved => "antiburn.check_enablement_saved",
            EventName::StepSettingsViewed => "antiburn.step_settings_viewed",
        }
    }

    /// Whether this event represents deliberate UI use and refreshes the session timeout.
    ///
    /// Keep this match exhaustive. New events must make an explicit choice instead of
    /// letting background telemetry extend an interaction session by default.
    pub fn is_user_oriented(self, facts: &Facts) -> bool {
        match self {
            EventName::CliOperationCompleted => matches!(facts.label, Some("test" | "repair")),
            EventName::SettingToggled
            | EventName::AnalyticsOptedOut
            | EventName::SessionOpened
            | EventName::SettingsPaneViewed
            | EventName::BurnCheckAutoFixReviewed
            | EventName::BurnCheckAutoFixConfirmed
            | EventName::BurnCheckPromptPrepared
            | EventName::BurnCheckPromptCopied
            | EventName::BurnCheckDefinitionOpened
            | EventName::BurnCheckDirectoryOpened
            | EventName::SessionFilterSelected
            | EventName::SessionFiltersChanged
            | EventName::RemoteHostConnectionChecked
            | EventName::RemoteHostChanged
            | EventName::ProjectFolderAction
            | EventName::MemoryAction
            | EventName::NavigationHistoryMoved
            | EventName::AppSearchOpened
            | EventName::AppSearchResultOpened
            | EventName::UsageLimitsInteraction
            | EventName::InterfaceScaleChanged
            | EventName::OnboardingActionTaken
            | EventName::FirstRunAction
            | EventName::CheckEnablementSaved
            | EventName::StepSettingsViewed => true,
            EventName::PromotionInteraction => matches!(
                facts.label,
                Some("primary_activated" | "learn_more_activated" | "command_copied" | "dismissed")
            ),
            EventName::SurfaceViewed => facts.detail == Some("user"),
            EventName::RemoteSyncCompleted => facts.detail == Some("manual"),
            // `started`, `found`, `read`, and `checked` are reported as the
            // first-run work completes on its own. Only `result` follows a
            // deliberate Next press.
            EventName::FirstRunStepReached => facts.label == Some("result"),
            EventName::ModelCheckLifecycle => matches!(
                facts.detail,
                Some(
                    "enabled"
                        | "disabled"
                        | "provider_saved"
                        | "provider_switched"
                        | "connection_removed"
                        | "credential_removed"
                        | "test_succeeded"
                        | "test_failed"
                )
            ),
            EventName::AppLaunched
            | EventName::ScanCompleted
            | EventName::ErrorOccurred
            | EventName::UnrecognizedRecordsObserved
            | EventName::ClaudeLimitResetObserved
            | EventName::ClaudeLoginObserved
            | EventName::SurfaceStateObserved
            | EventName::LiveUsageStateObserved
            | EventName::UsageObserved
            | EventName::LimitFactorObserved
            | EventName::ResourceUsageObserved
            | EventName::BurnCheckAutoFixCompleted
            | EventName::BurnCheckOutcomeObserved
            | EventName::BurnCheckInputPrepared
            | EventName::QuotaIncidentsObserved
            | EventName::ProviderIncidentsObserved
            | EventName::ProviderIncidentsIngested
            | EventName::QuotaWindowClosed
            | EventName::OnboardingScreenViewed
            | EventName::OnboardingCloudOutcome
            | EventName::OnboardingCompleted
            | EventName::FirstRunFinished => false,
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
    /// into another run. It rolls after 30 minutes without a user-oriented event;
    /// background telemetry does not refresh that timeout. Queued event payloads
    /// include the captured value until delivery or withdrawal. The rotating
    /// [`Event::anonymous_id`] remains the longest-lived generator state here.
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
    /// The closed origin for a surface state or visible Burn Check result.
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
    /// A closed window's dollars-only estimate against its last reading,
    /// banded and signed. Optional and present only on
    /// `antiburn.quota_window_closed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimate_bias: Option<&'static str>,
    /// A closed window's unexplained meter rise as a share of its last
    /// reading, banded. Optional and present only on
    /// `antiburn.quota_window_closed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unexplained_band: Option<&'static str>,
    /// Whether a closed window's last reading covered its own end. Optional
    /// and present only on `antiburn.quota_window_closed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reading_coverage: Option<&'static str>,
    /// These bands describe process and local-store resource use for one bounded window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_usage: Option<super::resources::schema::ResourceUsageSummary>,
    /// Sanitized transcript record type names. This field appears only on
    /// `antiburn.unrecognized_records_observed`.
    /// `analytics::sanitize_unrecognized_types` checks each name before it
    /// reaches this struct. It keeps at most 16 names. Each name must be
    /// non-empty, ASCII, at most 64 bytes long, and made only of characters
    /// in `[A-Za-z0-9_.:/-]`. A name that fails this check becomes the fixed
    /// sentinel `<rejected>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unrecognized_types: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy)]
pub enum CliOperation {
    Startup,
    Test,
    Repair,
}

pub fn cli_operation_facts(operation: CliOperation, succeeded: bool) -> Facts {
    Facts {
        label: Some(match operation {
            CliOperation::Startup => "startup",
            CliOperation::Test => "test",
            CliOperation::Repair => "repair",
        }),
        detail: Some(if succeeded { "succeeded" } else { "failed" }),
        ..Facts::default()
    }
}

/// What a caller may attach to an event.
///
/// A struct rather than three positional `Option<&'static str>` arguments,
/// which are indistinguishable to the compiler and so silently swappable at a
/// call site. Naming them makes a mix-up a compile error instead of a wrong
/// value arriving in a dashboard nobody cross-checks.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// A bucketed magnitude, never an exact count.
    pub bucket: Option<&'static str>,
    /// The primary dimension.
    pub label: Option<&'static str>,
    /// The secondary dimension, where the event has one.
    pub detail: Option<&'static str>,
    /// The closed origin for a surface state or visible Burn Check result.
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
    pub estimate_bias: Option<&'static str>,
    pub unexplained_band: Option<&'static str>,
    pub reading_coverage: Option<&'static str>,
    #[cfg(feature = "analytics")]
    pub resource_usage: Option<super::resources::schema::ResourceUsageSummary>,
    /// Sanitized transcript record type names. See [`Properties::unrecognized_types`].
    pub unrecognized_types: Option<Vec<String>>,
}

/// A saved local preparation outcome. No authored value enters this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BurnCheckPreparationOutcome {
    Unchanged,
    Shortened,
    InputTooLarge,
    InvalidFit,
    InputUnavailable,
    ResourceLimit,
    InvalidInput,
    InvalidRequest,
}

impl BurnCheckPreparationOutcome {
    pub(crate) fn from_record(
        preparation: &antiburn_checks::fit::PreparationRecord,
    ) -> Option<Self> {
        use antiburn_checks::fit::{PreparationFailureCode, PreparationRecord};
        Some(match preparation {
            PreparationRecord::Local(_) => return None,
            PreparationRecord::Ready(prepared)
                if prepared
                    .decisions
                    .iter()
                    .all(|decision| decision.report.omissions.is_empty()) =>
            {
                Self::Unchanged
            }
            PreparationRecord::Ready(_) => Self::Shortened,
            PreparationRecord::Rejected(error) => match error.code {
                PreparationFailureCode::ConditionFalse => return None,
                PreparationFailureCode::InputUnavailable
                | PreparationFailureCode::ProviderUnavailable
                | PreparationFailureCode::IncompatibleProfile => Self::InputUnavailable,
                PreparationFailureCode::InvalidInput => Self::InvalidInput,
                PreparationFailureCode::InvalidRequest => Self::InvalidRequest,
                PreparationFailureCode::NoFit => Self::InputTooLarge,
                PreparationFailureCode::PreparationResourceLimit => Self::ResourceLimit,
                PreparationFailureCode::InvalidFit => Self::InvalidFit,
            },
        })
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Shortened => "shortened",
            Self::InputTooLarge => "input_too_large",
            Self::InvalidFit => "invalid_fit",
            Self::InputUnavailable => "input_unavailable",
            Self::ResourceLimit => "resource_limit",
            Self::InvalidInput => "invalid_input",
            Self::InvalidRequest => "invalid_request",
        }
    }

    pub(crate) fn facts(self) -> Facts {
        Facts {
            detail: Some(self.as_str()),
            ..Facts::default()
        }
    }
}

/// Native completion categories do not accept a path, reference, or private error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BurnCheckDefinitionOpenOutcome {
    Succeeded,
    Missing,
    Unavailable,
    OpenFailed,
}

impl BurnCheckDefinitionOpenOutcome {
    pub(crate) fn facts(self) -> Facts {
        Facts {
            detail: Some(match self {
                Self::Succeeded => "succeeded",
                Self::Missing => "missing",
                Self::Unavailable => "unavailable",
                Self::OpenFailed => "open_failed",
            }),
            ..Facts::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BurnCheckDirectoryOpenOutcome {
    Succeeded,
    Missing,
    Unavailable,
    OpenFailed,
}

impl BurnCheckDirectoryOpenOutcome {
    pub(crate) fn facts(self) -> Facts {
        Facts {
            detail: Some(match self {
                Self::Succeeded => "succeeded",
                Self::Missing => "missing",
                Self::Unavailable => "unavailable",
                Self::OpenFailed => "open_failed",
            }),
            ..Facts::default()
        }
    }
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
    /// A project folder action completed without transmitting its path.
    ProjectFolderAction {
        action: ProjectFolderAction,
        outcome: ProjectFolderOutcome,
    },
    /// A memory action settled. The event carries no path, name, content or
    /// error text.
    MemoryAction {
        action: MemoryActionKind,
        outcome: MemoryActionOutcome,
    },
    /// A session was opened from the activity list. `agent` deserializes into
    /// the engine's own closed enum, so an unrecognised slug is a rejected
    /// command rather than a new value appearing in the data.
    SessionOpened {
        agent: AgentKind,
        environment: Environment,
    },
    /// A fixed product surface became visible.
    SurfaceViewed {
        surface: Surface,
        origin: Origin,
    },
    /// A visible surface presented a data state.
    SurfaceStateObserved {
        surface: StateSurface,
        state: SurfaceState,
        origin: Origin,
    },
    /// A fixed Settings pane became visible.
    SettingsPaneViewed {
        pane: SettingsPane,
    },
    /// A provider state appeared on a visible usage surface.
    LiveUsageStateObserved {
        provider: LiveUsageProvider,
        state: LiveUsageState,
        origin: Origin,
    },
    /// An Auto Fix review request completed.
    BurnCheckAutoFixReviewed {
        outcome: AutoFixReviewOutcome,
    },
    /// The reader confirmed the operation shown in an Auto Fix review.
    BurnCheckAutoFixConfirmed,
    /// A confirmed Auto Fix operation completed.
    BurnCheckAutoFixCompleted {
        outcome: AutoFixOutcome,
    },
    /// A fix prompt request completed.
    BurnCheckPromptPrepared {
        outcome: PromptPreparationOutcome,
    },
    /// A prepared fix prompt reached the clipboard.
    BurnCheckPromptCopied {},
    /// A later result appeared in the visible Burn Checks workspace.
    BurnCheckOutcomeObserved {
        outcome: BurnCheckOutcome,
        origin: BurnCheckOrigin,
    },
    /// The Sessions sidebar filter changed to a different selection. `agent`
    /// deserializes into the engine's own closed enum, so an unrecognized
    /// slug is a rejected command rather than a new value appearing in the
    /// data; the renderer omits it rather than send one.
    SessionFilterSelected {
        filter: SessionFilterKind,
        agent: Option<AgentKind>,
    },
    NavigationHistoryMoved {
        direction: HistoryDirection,
    },
    AppSearchOpened {},
    AppSearchResultOpened {
        category: SearchCategory,
    },
    /// The contextual Sessions filters changed.
    SessionFiltersChanged {
        action: SessionFilterAction,
        agent: Option<AgentKind>,
    },
    OnboardingScreenViewed {
        screen: OnboardingScreen,
    },
    OnboardingActionTaken {
        screen: OnboardingScreen,
        action: OnboardingAction,
    },
    OnboardingCloudUnavailable {},
    /// The first-run Overview reached a fixed funnel step.
    FirstRunStepReached {
        step: FirstRunStep,
        /// The discovery pass's total session count. Present only when
        /// `step` is `found`; bucketed before it reaches [`Facts`].
        sessions: Option<u32>,
        /// Which result first showed. Present only when `step` is `result`.
        result: Option<FirstRunResult>,
    },
    /// A deliberate action in the first-run Overview completed.
    FirstRunAction {
        action: FirstRunActionKind,
    },
    /// The first-run Overview's result first showed.
    FirstRunFinished {},
    /// A progress step's settings became visible in the first-run
    /// disclosure. After the first run, the settings live in Settings
    /// panes, which report their own `settings_pane_viewed` instead.
    StepSettingsViewed {
        label: StepSettingsLabel,
        detail: StepSettingsDetail,
    },
    /// A production promotion panel was viewed or used.
    #[serde(rename = "promotionInteraction")]
    PromotionUsed {
        campaign: PromotionCampaign,
        action: PromotionAction,
        origin: Origin,
    },
    #[serde(rename = "usageLimitsInteraction")]
    UsageLimitsUsed {
        action: UsageLimitsAction,
    },
}

/// A stable, closed analytics identity for a check.
///
/// Keep this separate from arbitrary serialized keys so a future detector does
/// not enter analytics until its measurement contract is reviewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckEnablementId {
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

impl From<antiburn_local::checks::DetectorId> for CheckEnablementId {
    fn from(detector: antiburn_local::checks::DetectorId) -> Self {
        use antiburn_local::checks::DetectorId;

        match detector {
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

impl CheckEnablementId {
    #[cfg(feature = "analytics")]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SessionsOverDepth => "sessions_over_depth",
            Self::ModelOverthinking => "model_overthinking",
            Self::OverpoweredSubagents => "overpowered_subagents",
            Self::UnusedMcpServers => "unused_mcp_servers",
            Self::UnusedBuiltInTools => "unused_built_in_tools",
            Self::UnusedSkills => "unused_skills",
            Self::OldModelUsage => "old_model_usage",
            Self::OveruseOfFastMode => "overuse_of_fast_mode",
            Self::CacheChurn => "cache_churn",
        }
    }
}

#[cfg(feature = "analytics")]
pub(crate) fn check_enablement_facts(
    detector: antiburn_local::checks::DetectorId,
    enabled: bool,
) -> Facts {
    Facts {
        label: Some(CheckEnablementId::from(detector).as_str()),
        detail: Some(if enabled { "enabled" } else { "disabled" }),
        ..Facts::default()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum DecisionProvider {
    Jev,
    Cloudflare,
    Ollama,
    Custom,
}

#[derive(Debug, Clone, Copy)]
pub enum ProviderSetupOutcome {
    Saved,
    Switched,
    ConnectionRemoved,
    CredentialRemoved,
}

#[derive(Debug, Clone, Copy)]
pub enum ProviderTestOutcome {
    Succeeded,
    Failed,
}

/// Native operation boundaries own lifecycle events.
#[derive(Debug, Clone, Copy)]
pub enum ModelCheckLifecycle {
    Enablement {
        enabled: bool,
    },
    ProviderSetup {
        provider: DecisionProvider,
        outcome: ProviderSetupOutcome,
    },
    ProviderTest {
        provider: DecisionProvider,
        outcome: ProviderTestOutcome,
    },
}

impl From<crate::jev::config::SystemOneProvider> for DecisionProvider {
    fn from(provider: crate::jev::config::SystemOneProvider) -> Self {
        use crate::jev::config::SystemOneProvider;
        match provider {
            SystemOneProvider::Jev => Self::Jev,
            SystemOneProvider::Cloudflare => Self::Cloudflare,
            SystemOneProvider::Ollama => Self::Ollama,
            SystemOneProvider::Custom => Self::Custom,
        }
    }
}

#[cfg(feature = "analytics")]
impl ModelCheckLifecycle {
    pub fn resolve(self) -> (EventName, Facts) {
        let (label, detail) = match self {
            Self::Enablement { enabled } => {
                ("enablement", if enabled { "enabled" } else { "disabled" })
            }
            Self::ProviderSetup { provider, outcome } => (
                provider.as_str(),
                match outcome {
                    ProviderSetupOutcome::Saved => "provider_saved",
                    ProviderSetupOutcome::Switched => "provider_switched",
                    ProviderSetupOutcome::ConnectionRemoved => "connection_removed",
                    ProviderSetupOutcome::CredentialRemoved => "credential_removed",
                },
            ),
            Self::ProviderTest { provider, outcome } => (
                provider.as_str(),
                match outcome {
                    ProviderTestOutcome::Succeeded => "test_succeeded",
                    ProviderTestOutcome::Failed => "test_failed",
                },
            ),
        };
        (
            EventName::ModelCheckLifecycle,
            Facts {
                label: Some(label),
                detail: Some(detail),
                ..Facts::default()
            },
        )
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromotionCampaign {
    ClaudeMod,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromotionAction {
    AnnouncementViewed,
    SetupViewed,
    PrimaryActivated,
    LearnMoreActivated,
    CommandCopied,
    Dismissed,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageLimitsAction {
    Opened,
    Closed,
    PinAdded,
    PinRemoved,
    AppearanceDials,
    AppearanceBars,
    RefreshSucceeded,
    RefreshFailed,
    EnableSucceeded,
    EnableFailed,
    SettingsOpened,
    SettingsOpenFailed,
}

#[cfg(feature = "analytics")]
impl UsageLimitsAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Opened => "opened",
            Self::Closed => "closed",
            Self::PinAdded => "pin_added",
            Self::PinRemoved => "pin_removed",
            Self::AppearanceDials => "appearance_dials",
            Self::AppearanceBars => "appearance_bars",
            Self::RefreshSucceeded => "refresh_succeeded",
            Self::RefreshFailed => "refresh_failed",
            Self::EnableSucceeded => "enable_succeeded",
            Self::EnableFailed => "enable_failed",
            Self::SettingsOpened => "settings_opened",
            Self::SettingsOpenFailed => "settings_open_failed",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectFolderAction {
    Open,
    Copy,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectFolderOutcome {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryActionKind {
    Reveal,
    Archive,
    Restore,
    RemoveIndexLine,
    OpenFromSession,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryActionOutcome {
    Succeeded,
    Failed,
    ChangedOnDisk,
    Unsupported,
}

/// A product surface whose visibility is measured.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Agents,
    Activity,
    SessionDetail,
    ProviderPreview,
    ChecksPreview,
    Hud,
    HudDetail,
    Settings,
    BurnChecks,
    Quota,
    Memories,
}

/// A surface that can present a measured data state.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StateSurface {
    Agents,
    Activity,
    SessionDetail,
    ProviderPreview,
    ChecksPreview,
    Hud,
    HudDetail,
    Settings,
    BurnChecks,
    Quota,
    Memories,
}

/// A closed Auto Fix review preparation result.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutoFixReviewOutcome {
    Ready,
    Stale,
    Expired,
    Conflict,
    Unavailable,
    Failed,
}

/// A closed result from one confirmed Auto Fix operation.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutoFixOutcome {
    AppliedAwaitingVerification,
    AppliedVerificationUnavailable,
    RecoveryNeeded,
    Stale,
    Expired,
    Conflict,
    Unavailable,
    Failed,
}

/// A closed result from one prompt preparation request.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromptPreparationOutcome {
    Ready,
    Stale,
    Expired,
    Unavailable,
    Failed,
}

/// A later Burn Check result that the workspace can present.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BurnCheckOutcome {
    Verified,
    Recurred,
}

/// Whether the measured verification watch started passively or from an action.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BurnCheckOrigin {
    Passive,
    Action,
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
    Agents,
    Sessions,
    Checks,
    Appearance,
    Privacy,
    Notifications,
    Usage,
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
    /// The login expired, but it can recover without a new sign-in.
    LoginRecovering,
    RateLimited,
    Unavailable,
    NoCredentials,
    /// A Claude login exists, but only a new sign-in can make it usable.
    SignInRequired,
    /// The tool is installed, but it holds no login.
    NotSignedIn,
}

/// Where an agent ran. Two values, and neither names anything: a WSL
/// distribution's *name* is chosen by the reader and is deliberately not
/// carried, unlike the contract's own client, which sends it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    Native,
    Wsl,
    Remote,
}

/// A Sessions sidebar filter kind. `Agent` covers every harness item; the
/// harness itself travels separately, as `Interaction::SessionFilterSelected`'s
/// own `agent` field.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionFilterKind {
    Notable,
    Material,
    Agent,
    Failing,
    Passing,
    All,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryDirection {
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchCategory {
    View,
    Setting,
    Check,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionFilterAction {
    AgentAdded,
    AgentRemoved,
    AgentsAll,
    ResultFailing,
    ResultPassing,
    ResultAll,
    SpendNotable,
    SpendMaterial,
    SpendAll,
    SourceAll,
    SourceLocalAdded,
    SourceLocalRemoved,
    SourceRemoteAllAdded,
    SourceRemoteAllRemoved,
    SourceRemoteHostAdded,
    SourceRemoteHostRemoved,
    ClearedAll,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnboardingScreen {
    Welcome,
    Sessions,
    Limits,
    Hud,
    Checks,
    Cloud,
    CloudUnavailable,
    CloudHandoff,
    CloudIdentity,
    CloudFolders,
    CloudSync,
    CloudComplete,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingAction {
    GetStarted,
    Continue,
    Back,
    UseLocally,
    SetUpCloud,
    LearnMore,
    SignIn,
    Cancel,
    Retry,
    AddFolder,
    SelectFolder,
    StartSync,
    CheckStatus,
    CancelSync,
    Done,
    SignOut,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingCloudStatus {
    Opening,
    Waiting,
    Returned,
    Cancelled,
    Expired,
    BrowserFailed,
    Offline,
    CredentialFailed,
    SignedIn,
    SignedOut,
    Unavailable,
    Accepted,
    Partial,
    Failed,
}

#[cfg(feature = "analytics")]
pub(super) fn onboarding_dismissed_facts(state: crate::store::OnboardingState) -> Option<Facts> {
    use crate::store::OnboardingState;

    let screen = match state {
        OnboardingState::Welcome => OnboardingScreen::Welcome,
        OnboardingState::Sessions => OnboardingScreen::Sessions,
        OnboardingState::Limits => OnboardingScreen::Limits,
        OnboardingState::Hud => OnboardingScreen::Hud,
        OnboardingState::Checks => OnboardingScreen::Checks,
        OnboardingState::Cloud => OnboardingScreen::Cloud,
        OnboardingState::CloudUnavailable => OnboardingScreen::CloudUnavailable,
        OnboardingState::Completed => return None,
    };
    Some(Facts {
        label: Some(screen.as_str()),
        detail: Some("dismissed"),
        ..Facts::default()
    })
}

/// A fixed step in the first-run Overview's funnel.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FirstRunStep {
    Started,
    Found,
    Read,
    Checked,
    Result,
}

/// Which result the first-run Overview showed. Present only alongside
/// [`FirstRunStep::Result`].
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FirstRunResult {
    Empty,
    ChecksDisabled,
    NotAssessed,
    Clean,
    FixesFound,
}

/// A deliberate action the first-run Overview can report.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FirstRunActionKind {
    FolderAccessRequested,
    FolderAccessGranted,
    IncludeNonRepoFolders,
    LiveUsageStarted,
    LiveUsageSkipped,
    EnhanceOpened,
}

/// A progress step whose settings a reader can view. Narrower than the
/// renderer's own `StepSettingsStep`: `fixes` has no settings to report and
/// never reaches this enum.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepSettingsLabel {
    Agents,
    Limits,
    Sessions,
    Checks,
}

/// Which surface showed the step's settings. Settings → Agents, Sessions,
/// and Checks report `settings_pane_viewed` instead.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepSettingsDetail {
    /// The first-run takeover's "Show settings" disclosure opened.
    FirstRun,
}

/// A privacy-safe result from checking one remote host connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteConnectionOutcome {
    Ready,
    Authentication,
    HostKey,
    HelperMissing,
    Incompatible,
    Connection,
    Invalid,
}

/// A configured-host lifecycle operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteHostChange {
    Added,
    Edited,
    Removed,
    SyncEnabled,
    SyncDisabled,
}

/// A terminal remote synchronization result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSyncOutcome {
    Succeeded,
    Failed,
}

/// Why a remote synchronization pass started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSyncOrigin {
    Manual,
    Automatic,
}

#[cfg(feature = "analytics")]
impl Interaction {
    /// Keep historical decoding without emitting retired setup events.
    #[cfg(feature = "analytics")]
    pub fn is_retired(self) -> bool {
        matches!(
            self,
            Self::FirstRunStepReached { .. }
                | Self::FirstRunAction { .. }
                | Self::FirstRunFinished { .. }
                | Self::StepSettingsViewed { .. }
        )
    }

    /// The event and the facts this interaction becomes.
    pub fn resolve(self) -> (EventName, Facts) {
        match self {
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
            Interaction::BurnCheckAutoFixReviewed { outcome } => (
                EventName::BurnCheckAutoFixReviewed,
                Facts {
                    detail: Some(outcome.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::BurnCheckAutoFixConfirmed => {
                (EventName::BurnCheckAutoFixConfirmed, Facts::default())
            }
            Interaction::BurnCheckAutoFixCompleted { outcome } => (
                EventName::BurnCheckAutoFixCompleted,
                Facts {
                    detail: Some(outcome.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::BurnCheckPromptPrepared { outcome } => (
                EventName::BurnCheckPromptPrepared,
                Facts {
                    detail: Some(outcome.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::BurnCheckPromptCopied {} => {
                (EventName::BurnCheckPromptCopied, Facts::default())
            }
            Interaction::BurnCheckOutcomeObserved { outcome, origin } => (
                EventName::BurnCheckOutcomeObserved,
                Facts {
                    detail: Some(outcome.as_str()),
                    origin: Some(origin.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::ProjectFolderAction { action, outcome } => (
                EventName::ProjectFolderAction,
                Facts {
                    label: Some(match action {
                        ProjectFolderAction::Open => "open",
                        ProjectFolderAction::Copy => "copy",
                    }),
                    detail: Some(match outcome {
                        ProjectFolderOutcome::Succeeded => "succeeded",
                        ProjectFolderOutcome::Failed => "failed",
                    }),
                    ..Facts::default()
                },
            ),
            Interaction::MemoryAction { action, outcome } => (
                EventName::MemoryAction,
                Facts {
                    label: Some(match action {
                        MemoryActionKind::Reveal => "reveal",
                        MemoryActionKind::Archive => "archive",
                        MemoryActionKind::Restore => "restore",
                        MemoryActionKind::RemoveIndexLine => "remove_index_line",
                        MemoryActionKind::OpenFromSession => "open_from_session",
                    }),
                    detail: Some(match outcome {
                        MemoryActionOutcome::Succeeded => "succeeded",
                        MemoryActionOutcome::Failed => "failed",
                        MemoryActionOutcome::ChangedOnDisk => "changed_on_disk",
                        MemoryActionOutcome::Unsupported => "unsupported",
                    }),
                    ..Facts::default()
                },
            ),
            Interaction::SessionFilterSelected { filter, agent } => (
                EventName::SessionFilterSelected,
                Facts {
                    label: Some(filter.as_str()),
                    detail: agent.map(AgentKind::slug),
                    ..Facts::default()
                },
            ),
            Interaction::NavigationHistoryMoved { direction } => (
                EventName::NavigationHistoryMoved,
                Facts {
                    label: Some(direction.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::AppSearchOpened {} => (EventName::AppSearchOpened, Facts::default()),
            Interaction::AppSearchResultOpened { category } => (
                EventName::AppSearchResultOpened,
                Facts {
                    label: Some(category.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::SessionFiltersChanged { action, agent } => (
                EventName::SessionFiltersChanged,
                Facts {
                    label: Some(action.as_str()),
                    detail: match action {
                        SessionFilterAction::AgentAdded | SessionFilterAction::AgentRemoved => {
                            agent.map(AgentKind::slug)
                        }
                        _ => None,
                    },
                    ..Facts::default()
                },
            ),
            Interaction::OnboardingScreenViewed { screen } => (
                EventName::OnboardingScreenViewed,
                Facts {
                    label: Some(screen.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::OnboardingActionTaken { screen, action } => (
                EventName::OnboardingActionTaken,
                Facts {
                    label: Some(screen.as_str()),
                    detail: Some(action.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::OnboardingCloudUnavailable {} => (
                EventName::OnboardingCloudOutcome,
                Facts {
                    detail: Some("unavailable"),
                    ..Facts::default()
                },
            ),
            Interaction::FirstRunStepReached {
                step,
                sessions,
                result,
            } => (
                EventName::FirstRunStepReached,
                Facts {
                    label: Some(step.as_str()),
                    bucket: sessions.map(|count| bucket(count as u64)),
                    detail: result.map(FirstRunResult::as_str),
                    ..Facts::default()
                },
            ),
            Interaction::FirstRunAction { action } => (
                EventName::FirstRunAction,
                Facts {
                    label: Some(action.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::FirstRunFinished {} => (EventName::FirstRunFinished, Facts::default()),
            Interaction::StepSettingsViewed { label, detail } => (
                EventName::StepSettingsViewed,
                Facts {
                    label: Some(label.as_str()),
                    detail: Some(detail.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::PromotionUsed {
                campaign,
                action,
                origin,
            } => (
                EventName::PromotionInteraction,
                Facts {
                    detail: Some(campaign.as_str()),
                    label: Some(action.as_str()),
                    origin: Some(origin.as_str()),
                    ..Facts::default()
                },
            ),
            Interaction::UsageLimitsUsed { action } => (
                EventName::UsageLimitsInteraction,
                Facts {
                    label: Some(action.as_str()),
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
wire_values!(DecisionProvider, {
    DecisionProvider::Jev => "jev",
    DecisionProvider::Cloudflare => "cloudflare",
    DecisionProvider::Ollama => "ollama",
    DecisionProvider::Custom => "custom",
});

#[cfg(feature = "analytics")]
wire_values!(Surface, {
    Surface::Activity => "activity",
    Surface::SessionDetail => "session_detail",
    Surface::ProviderPreview => "provider_preview",
    Surface::ChecksPreview => "checks_preview",
    Surface::Hud => "hud",
    Surface::HudDetail => "hud_detail",
    Surface::Settings => "settings",
    Surface::BurnChecks => "burn_checks",
    Surface::Quota => "quota",
    Surface::Agents => "agents",
    Surface::Memories => "memories",
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
    StateSurface::BurnChecks => "burn_checks",
    StateSurface::Quota => "quota",
    StateSurface::Agents => "agents",
    StateSurface::Memories => "memories",
});

#[cfg(feature = "analytics")]
wire_values!(SessionFilterKind, {
    SessionFilterKind::Notable => "notable",
    SessionFilterKind::Material => "material",
    SessionFilterKind::Agent => "agent",
    SessionFilterKind::Failing => "failing",
    SessionFilterKind::Passing => "passing",
    SessionFilterKind::All => "all",
});
#[cfg(feature = "analytics")]
wire_values!(HistoryDirection, { HistoryDirection::Back => "back", HistoryDirection::Forward => "forward" });
#[cfg(feature = "analytics")]
wire_values!(SearchCategory, { SearchCategory::View => "view", SearchCategory::Setting => "setting", SearchCategory::Check => "check" });
#[cfg(feature = "analytics")]
wire_values!(SessionFilterAction, {
    SessionFilterAction::AgentAdded => "agent_added",
    SessionFilterAction::AgentRemoved => "agent_removed",
    SessionFilterAction::AgentsAll => "agents_all",
    SessionFilterAction::ResultFailing => "result_failing",
    SessionFilterAction::ResultPassing => "result_passing",
    SessionFilterAction::ResultAll => "result_all",
    SessionFilterAction::SpendNotable => "spend_notable",
    SessionFilterAction::SpendMaterial => "spend_material",
    SessionFilterAction::SpendAll => "spend_all",
    SessionFilterAction::SourceAll => "source_all",
    SessionFilterAction::SourceLocalAdded => "source_local_added",
    SessionFilterAction::SourceLocalRemoved => "source_local_removed",
    SessionFilterAction::SourceRemoteAllAdded => "source_remote_all_added",
    SessionFilterAction::SourceRemoteAllRemoved => "source_remote_all_removed",
    SessionFilterAction::SourceRemoteHostAdded => "source_remote_host_added",
    SessionFilterAction::SourceRemoteHostRemoved => "source_remote_host_removed",
    SessionFilterAction::ClearedAll => "cleared_all",
});

#[cfg(feature = "analytics")]
wire_values!(OnboardingScreen, {
    OnboardingScreen::Welcome => "welcome",
    OnboardingScreen::Sessions => "sessions",
    OnboardingScreen::Limits => "limits",
    OnboardingScreen::Hud => "hud",
    OnboardingScreen::Checks => "checks",
    OnboardingScreen::Cloud => "cloud",
    OnboardingScreen::CloudUnavailable => "cloud_unavailable",
    OnboardingScreen::CloudHandoff => "cloud_handoff",
    OnboardingScreen::CloudIdentity => "cloud_identity",
    OnboardingScreen::CloudFolders => "cloud_folders",
    OnboardingScreen::CloudSync => "cloud_sync",
    OnboardingScreen::CloudComplete => "cloud_complete",

});

#[cfg(feature = "analytics")]
wire_values!(OnboardingAction, {
    OnboardingAction::GetStarted => "get_started",
    OnboardingAction::Continue => "continue",
    OnboardingAction::Back => "back",
    OnboardingAction::UseLocally => "use_locally",
    OnboardingAction::SetUpCloud => "set_up_cloud",
    OnboardingAction::LearnMore => "learn_more",
    OnboardingAction::SignIn => "sign_in",
    OnboardingAction::Cancel => "cancel",
    OnboardingAction::Retry => "retry",
    OnboardingAction::AddFolder => "add_folder",
    OnboardingAction::SelectFolder => "select_folder",
    OnboardingAction::StartSync => "start_sync",
    OnboardingAction::CheckStatus => "check_status",
    OnboardingAction::CancelSync => "cancel_sync",
    OnboardingAction::Done => "done",
    OnboardingAction::SignOut => "sign_out",

});

#[cfg(feature = "analytics")]
wire_values!(OnboardingCloudStatus, {
    OnboardingCloudStatus::Opening => "opening",
    OnboardingCloudStatus::Waiting => "waiting",
    OnboardingCloudStatus::Returned => "returned",
    OnboardingCloudStatus::Cancelled => "cancelled",
    OnboardingCloudStatus::Expired => "expired",
    OnboardingCloudStatus::BrowserFailed => "browser_failed",
    OnboardingCloudStatus::Offline => "offline",
    OnboardingCloudStatus::CredentialFailed => "credential_failed",
    OnboardingCloudStatus::SignedIn => "signed_in",
    OnboardingCloudStatus::SignedOut => "signed_out",
    OnboardingCloudStatus::Unavailable => "unavailable",
    OnboardingCloudStatus::Accepted => "accepted",
    OnboardingCloudStatus::Partial => "partial",
    OnboardingCloudStatus::Failed => "failed",
});

#[cfg(feature = "analytics")]
wire_values!(FirstRunStep, {
    FirstRunStep::Started => "started",
    FirstRunStep::Found => "found",
    FirstRunStep::Read => "read",
    FirstRunStep::Checked => "checked",
    FirstRunStep::Result => "result",
});

#[cfg(feature = "analytics")]
wire_values!(FirstRunResult, {
    FirstRunResult::Empty => "empty",
    FirstRunResult::ChecksDisabled => "checks_disabled",
    FirstRunResult::NotAssessed => "not_assessed",
    FirstRunResult::Clean => "clean",
    FirstRunResult::FixesFound => "fixes_found",
});

#[cfg(feature = "analytics")]
wire_values!(FirstRunActionKind, {
    FirstRunActionKind::FolderAccessRequested => "folder_access_requested",
    FirstRunActionKind::FolderAccessGranted => "folder_access_granted",
    FirstRunActionKind::IncludeNonRepoFolders => "include_non_repo_folders",
    FirstRunActionKind::LiveUsageStarted => "live_usage_started",
    FirstRunActionKind::LiveUsageSkipped => "live_usage_skipped",
    FirstRunActionKind::EnhanceOpened => "enhance_opened",
});

#[cfg(feature = "analytics")]
wire_values!(PromotionCampaign, { PromotionCampaign::ClaudeMod => "claude_mod" });
#[cfg(feature = "analytics")]
wire_values!(PromotionAction, {
    PromotionAction::AnnouncementViewed => "announcement_viewed",
    PromotionAction::SetupViewed => "setup_viewed",
    PromotionAction::PrimaryActivated => "primary_activated",
    PromotionAction::LearnMoreActivated => "learn_more_activated",
    PromotionAction::CommandCopied => "command_copied",
    PromotionAction::Dismissed => "dismissed",
});

#[cfg(feature = "analytics")]
wire_values!(AutoFixReviewOutcome, {
    AutoFixReviewOutcome::Ready => "ready",
    AutoFixReviewOutcome::Stale => "stale",
    AutoFixReviewOutcome::Expired => "expired",
    AutoFixReviewOutcome::Conflict => "conflict",
    AutoFixReviewOutcome::Unavailable => "unavailable",
    AutoFixReviewOutcome::Failed => "failed",
});

#[cfg(feature = "analytics")]
wire_values!(AutoFixOutcome, {
    AutoFixOutcome::AppliedAwaitingVerification => "applied_awaiting_verification",
    AutoFixOutcome::AppliedVerificationUnavailable => "applied_verification_unavailable",
    AutoFixOutcome::RecoveryNeeded => "recovery_needed",
    AutoFixOutcome::Stale => "stale",
    AutoFixOutcome::Expired => "expired",
    AutoFixOutcome::Conflict => "conflict",
    AutoFixOutcome::Unavailable => "unavailable",
    AutoFixOutcome::Failed => "failed",
});

#[cfg(feature = "analytics")]
wire_values!(PromptPreparationOutcome, {
    PromptPreparationOutcome::Ready => "ready",
    PromptPreparationOutcome::Stale => "stale",
    PromptPreparationOutcome::Expired => "expired",
    PromptPreparationOutcome::Unavailable => "unavailable",
    PromptPreparationOutcome::Failed => "failed",
});

#[cfg(feature = "analytics")]
wire_values!(BurnCheckOutcome, {
    BurnCheckOutcome::Verified => "verified",
    BurnCheckOutcome::Recurred => "recurred",
});

#[cfg(feature = "analytics")]
wire_values!(BurnCheckOrigin, {
    BurnCheckOrigin::Passive => "passive",
    BurnCheckOrigin::Action => "action",
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
    SettingsPane::Agents => "agents",
    SettingsPane::Sessions => "sessions",
    SettingsPane::Checks => "checks",
    SettingsPane::Appearance => "appearance",
    SettingsPane::Privacy => "privacy",
    SettingsPane::Notifications => "notifications",
    SettingsPane::Usage => "usage",
    SettingsPane::About => "about",
});

#[cfg(feature = "analytics")]
wire_values!(StepSettingsLabel, {
    StepSettingsLabel::Agents => "agents",
    StepSettingsLabel::Limits => "limits",
    StepSettingsLabel::Sessions => "sessions",
    StepSettingsLabel::Checks => "checks",
});

#[cfg(feature = "analytics")]
wire_values!(StepSettingsDetail, {
    StepSettingsDetail::FirstRun => "first_run",
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
    LiveUsageState::LoginRecovering => "login_recovering",
    LiveUsageState::RateLimited => "rate_limited",
    LiveUsageState::Unavailable => "unavailable",
    LiveUsageState::NoCredentials => "no_credentials",
    LiveUsageState::SignInRequired => "sign_in_required",
    LiveUsageState::NotSignedIn => "not_signed_in",
});

/// The closed outcomes `antiburn.claude_login_observed` may carry in
/// `label`.
#[cfg(feature = "analytics")]
pub const CLAUDE_LOGIN_OUTCOMES: &[&str] = &[
    "refreshed",
    "unchanged",
    "sign_in_required",
    "cli_missing",
    "gated",
    "keychain_metadata_failed",
    "keychain_secret_missing",
];

/// The closed refresh triggers `antiburn.claude_login_observed` may carry
/// in `detail`.
#[cfg(feature = "analytics")]
pub const CLAUDE_LOGIN_TRIGGERS: &[&str] = &["expired", "pre_expiry", "cannot_refresh"];

/// The facts of one Claude login observation, or `None` for a value outside
/// the closed vocabularies.
#[cfg(feature = "analytics")]
pub fn claude_login_facts(
    observation: crate::provider_usage::live::LoginObservation,
) -> Option<Facts> {
    let label = CLAUDE_LOGIN_OUTCOMES
        .iter()
        .find(|value| **value == observation.label)?;
    let detail = match observation.detail {
        Some(detail) => Some(
            *CLAUDE_LOGIN_TRIGGERS
                .iter()
                .find(|value| **value == detail)?,
        ),
        None => None,
    };
    Some(Facts {
        label: Some(label),
        detail,
        ..Facts::default()
    })
}

#[cfg(feature = "analytics")]
impl Environment {
    fn as_str(self) -> &'static str {
        match self {
            Environment::Native => "native",
            Environment::Wsl => "wsl",
            Environment::Remote => "remote",
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

/// Map the plan and tier to fixed analytics values.
///
/// Missing or blank plans map to `unknown`.
/// Unlisted plans map to `other`.
/// Specific Max tiers and Pro Lite require the matching provider.
/// Raw plan and tier strings stay on this machine.
#[cfg(feature = "analytics")]
pub fn map_plan(
    provider: LiveUsageProvider,
    plan: Option<&str>,
    plan_tier: Option<&str>,
) -> &'static str {
    let Some(plan) = plan else {
        return "unknown";
    };
    let plan = plan.trim().to_ascii_lowercase();
    if plan.is_empty() {
        return "unknown";
    }
    if provider == LiveUsageProvider::Anthropic && plan == "max" {
        match plan_tier
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("default_claude_max_5x") => return "max_5x",
            Some("default_claude_max_20x") => return "max_20x",
            _ => {}
        }
    }
    if provider == LiveUsageProvider::Openai && plan == "prolite" {
        return "prolite";
    }
    match plan.as_str() {
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

/// Reduce a closed quota window's dollars-only estimate against its last
/// reading to a signed, coarse band.
///
/// `residual` is `(meter_percent, estimated_percent)` for the window's last
/// reading, the same pair shape [`residual_band`] takes. `None` (no reading,
/// or no factor point to price the estimate from) is `unknown`. Otherwise the
/// band names the *direction* of the disagreement as well as its size: `low`
/// means the dollars-only estimate landed below the meter, `high` means it
/// landed above.
#[cfg(feature = "analytics")]
pub fn estimate_bias(residual: Option<(f64, f64)>) -> &'static str {
    let Some((meter_percent, estimated_percent)) = residual else {
        return "unknown";
    };
    let difference = estimated_percent - meter_percent;
    let magnitude = difference.abs();
    if magnitude <= 5.0 {
        "within_5"
    } else if magnitude <= 20.0 {
        if difference < 0.0 {
            "low_5_to_20"
        } else {
            "high_5_to_20"
        }
    } else if difference < 0.0 {
        "low_over_20"
    } else {
        "high_over_20"
    }
}

/// Reduce a closed quota window's unexplained meter rise to a coarse band,
/// as a share of the window's last reading.
///
/// `unexplained_total_percent` is the sum of every
/// `SharedPeriod::unexplained` entry's own percent. `last_reading_percent`
/// is `None` when the window carries no reading at all, which bands as
/// `unknown` rather than `none`: a share of nothing is not a clean window,
/// it is an unmeasured one. A reading of exactly zero percent bands as
/// `none`: nothing rose, so nothing can be unexplained either.
#[cfg(feature = "analytics")]
pub fn unexplained_band(
    unexplained_total_percent: f64,
    last_reading_percent: Option<f64>,
) -> &'static str {
    let Some(last_reading_percent) = last_reading_percent else {
        return "unknown";
    };
    if last_reading_percent <= 0.0 {
        return "none";
    }
    let share_percent = (unexplained_total_percent / last_reading_percent) * 100.0;
    if share_percent < 1.0 {
        "none"
    } else if share_percent < 10.0 {
        "under_10"
    } else if share_percent < 30.0 {
        "10_to_under_30"
    } else {
        "30_and_over"
    }
}

/// One hour, in seconds: how close a closed window's last reading must sit
/// to the window's own end to count as [covering][reading_coverage] it.
#[cfg(feature = "analytics")]
const READING_COVERAGE_TOLERANCE_SECS: i64 = 3_600;

/// Whether a closed quota window's last reading covered its own end.
///
/// `coverage_until` is [`crate::provider_usage::quota::share::SharedPeriod::coverage_until`],
/// `None` when the window carries no reading at all. `window_end_epoch` is
/// the window's own (possibly truncated) reset. A reading within
/// [`READING_COVERAGE_TOLERANCE_SECS`] of that reset is `covered`; an older
/// one is `stale`.
#[cfg(feature = "analytics")]
pub fn reading_coverage(coverage_until: Option<i64>, window_end_epoch: i64) -> &'static str {
    let Some(coverage_until) = coverage_until else {
        return "none";
    };
    if window_end_epoch - coverage_until <= READING_COVERAGE_TOLERANCE_SECS {
        "covered"
    } else {
        "stale"
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
mod tests;
