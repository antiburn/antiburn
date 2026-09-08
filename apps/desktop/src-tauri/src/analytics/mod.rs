//! Anonymised product events behind build and opt-out gates.
//!
//! This is the one place antiburn sends anything of its own beyond the update
//! check. The properties below define its privacy boundary.
//!
//! - **Official builds start enabled.** App launch and fixed onboarding-step
//!   events can be sent before setup finishes. Settings and
//!   `ANTIBURN_ANALYTICS_ENABLED=false` provide independent opt-outs.
//! - **A build with no endpoint sends nothing.** See [`config`]; every build
//!   from a clean checkout of this repository is in that state.
//! - **The payload cannot carry the reader's work.** See [`event`], where the
//!   closed struct is the enforcement rather than a review convention.
//! - **The identifier cannot build a longitudinal profile.** It is random and
//!   rotates on [`IDENTITY_LIFETIME_DAYS`]; opting out destroys it, so opting
//!   back in is a new identity that cannot be joined to the old one. The
//!   `sessionId` generator state is weaker still — held in memory and gone
//!   when the process exits. Queued payloads include that value until delivery.
//!
//! What this module cannot enforce is what happens after delivery. Retention
//! and IP handling belong to whoever operates the endpoint, are stated in the
//! privacy policy rather than in the app, and are deliberately not claimed by
//! any copy this repository ships.

#[cfg(feature = "analytics")]
pub mod config;
#[cfg(feature = "analytics")]
mod delivery;
pub mod event;

#[cfg(not(feature = "analytics"))]
pub fn available() -> bool {
    false
}

#[cfg(not(feature = "analytics"))]
pub fn environment_disabled() -> bool {
    false
}

#[cfg(not(feature = "analytics"))]
pub fn operator() -> Option<&'static str> {
    None
}

#[cfg(not(feature = "analytics"))]
pub fn install(_app: &tauri::AppHandle) {}

#[cfg(not(feature = "analytics"))]
pub fn record(_app: &tauri::AppHandle, _name: event::EventName, facts: event::Facts) {
    let _ = (
        facts.bucket,
        facts.label,
        facts.detail,
        facts.origin,
        facts.usage_band,
        facts.response_shape,
        facts.eligibility,
        facts.ineligible_reason,
        facts.experiment,
        facts.reset_arm,
        facts.reset_availability,
        facts.resets_per_week,
        facts.next_reset_available,
        facts.plan,
        facts.factor_band,
        facts.residual_band,
    );
}

#[cfg(not(feature = "analytics"))]
pub fn record_interaction(_app: &tauri::AppHandle, interaction: event::Interaction) {
    match interaction {
        event::Interaction::OnboardingStepViewed { step } => {
            let _ = step;
        }
        event::Interaction::SessionOpened { agent, environment } => {
            let _ = (agent, environment);
        }
        event::Interaction::SurfaceViewed { surface, origin } => {
            let _ = (surface, origin);
        }
        event::Interaction::SurfaceStateObserved {
            surface,
            state,
            origin,
        } => {
            let _ = (surface, state, origin);
        }
        event::Interaction::SettingsPaneViewed { pane } => {
            let _ = pane;
        }
        event::Interaction::LiveUsageStateObserved {
            provider,
            state,
            origin,
        } => {
            let _ = (provider, state, origin);
        }
    }
}

#[cfg(not(feature = "analytics"))]
pub fn prepare_onboarding_restart() {}

#[cfg(not(feature = "analytics"))]
pub fn record_onboarding_started(_app: &tauri::AppHandle) {}

#[cfg(not(feature = "analytics"))]
pub fn record_onboarding_finished(_app: &tauri::AppHandle) {}

#[cfg(not(feature = "analytics"))]
pub fn prepare_hud_exposure(_origin: event::Origin) {}

#[cfg(not(feature = "analytics"))]
pub fn cancel_hud_exposure() {}

#[cfg(not(feature = "analytics"))]
pub fn take_hud_exposure_origin(_exposed: bool) -> Option<event::Origin> {
    None
}

#[cfg(not(feature = "analytics"))]
pub fn record_scan(_app: &tauri::AppHandle, _sessions: Option<u64>) {}

#[cfg(not(feature = "analytics"))]
pub fn record_unrecognized_records(
    _app: &tauri::AppHandle,
    _summary: &antiburn_local::insights::UnrecognizedRecords,
) {
    let _ = event::EventName::UnrecognizedRecordsObserved;
}

#[cfg(not(feature = "analytics"))]
pub fn record_usage_observed(
    _app: &tauri::AppHandle,
    _snapshots: &[crate::provider_usage::live::ProviderUsageSnapshot],
) {
}

#[cfg(not(feature = "analytics"))]
pub fn record_limit_factor_observed(
    _app: &tauri::AppHandle,
    _learned: &[crate::provider_usage::factor::LearnedFactor],
) {
}

#[cfg(not(feature = "analytics"))]
pub fn handle_settings_transition(
    _app: &tauri::AppHandle,
    _previous: &crate::store::AppSettings,
    _saved: &crate::store::AppSettings,
) {
}

#[cfg(feature = "analytics")]
mod enabled {

    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    use antiburn_local::insights::UnrecognizedRecords;
    use tauri::Manager as _;

    use crate::provider_usage::factor::LearnedFactor;
    use crate::provider_usage::live::{ProviderUsageSnapshot, WindowRole, band_for_percent};

    use super::delivery::{DeliverySchedule, FlushOutcome};
    use super::event::{
        Event, EventName, Facts, Interaction, LiveUsageProvider, LiveUsageState, OnboardingFlow,
        Origin, SettingsPane, Surface,
    };
    use super::{config, delivery, event};
    use crate::store::{AppSettings, Store};

    /// How long an installation identifier lives before it is replaced.
    pub const IDENTITY_LIFETIME_DAYS: i64 = 30;

    /// How long a run identifier survives without an analytics event.
    ///
    /// The collector's contract specifies this window, and matching it is the
    /// point — a client that invented its own would make its rows incomparable
    /// with every other surface reporting to the same place.
    pub const SESSION_TIMEOUT: Duration = Duration::from_secs(30 * 60);

    /// How many failures a queued event survives before it is given up on.
    const MAX_ATTEMPTS: u32 = 5;

    /// How long one delivery may take before it is abandoned.
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

    /// Deliberate events separated by this gap belong to different visits.
    const DELIBERATE_VISIT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

    /// The floor between two `antiburn.limit_factor_observed` events for the
    /// same `(provider, lane)` pair, even across a genuine band change.
    const LIMIT_FACTOR_MIN_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

    /// Wakes the bounded delivery scheduler after a queue depth change.
    #[derive(Default)]
    struct DeliveryWake(tokio::sync::Notify);

    /// Whether this build could report at all, regardless of the reader's choice.
    ///
    /// Surfaces ask this to decide whether to offer the control as a live switch
    /// or as a disabled row that says why. Re-exported so callers do not have to
    /// know the configuration lives one module down.
    pub fn available() -> bool {
        config::configured()
    }

    /// Whether the process environment overrides the stored preference.
    pub fn environment_disabled() -> bool {
        std::env::var("ANTIBURN_ANALYTICS_ENABLED").is_ok_and(|value| disables_analytics(&value))
    }

    fn disables_analytics(value: &str) -> bool {
        value.trim().eq_ignore_ascii_case("false")
    }

    /// Who receives the events, for copy that names them.
    pub fn operator() -> Option<&'static str> {
        config::operator()
    }

    /// Whether an event may be recorded right now.
    ///
    /// Read fresh, and default to *not* acting: an unreadable preference is not
    /// permission, the same rule every notifier in this app follows. The
    pub fn allowed(app: &tauri::AppHandle) -> bool {
        if !available() || environment_disabled() {
            return false;
        }
        app.try_state::<Store>()
            .and_then(|store| store.settings().ok())
            .is_some_and(|settings| settings.analytics_enabled)
    }

    /// Record one event, if the reader has allowed it.
    ///
    /// Failure is silent by design. Analytics that interrupt the reader, or that
    /// fail an operation the reader actually asked for, would be the tail wagging
    /// the dog; a dropped event is not worth a single line of user-facing text.
    pub fn record(app: &tauri::AppHandle, name: EventName, facts: Facts) {
        let _ = record_event(app, name, facts);
    }

    fn record_event(app: &tauri::AppHandle, name: EventName, facts: Facts) -> bool {
        let _capture = CAPTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !allowed(app) {
            return false;
        }
        let Some(store) = app.try_state::<Store>() else {
            return false;
        };
        let Some((install_id, session_id)) = current_identity_pair(&store) else {
            return false;
        };
        let payload = Event {
            platform: event::PLATFORM,
            // Generated at capture, not at send: it is the collector's dedup key,
            // so it has to survive a retry unchanged or a redelivered batch counts
            // twice.
            message_id: random_identifier(),
            anonymous_id: install_id,
            session_id,
            event: name.as_str().to_string(),
            original_timestamp: crate::store::now_rfc3339(),
            properties: event::Properties {
                arch: event::arch(),
                bucket: facts.bucket,
                label: facts.label,
                detail: facts.detail,
                origin: facts.origin,
                usage_band: facts.usage_band,
                response_shape: facts.response_shape,
                eligibility: facts.eligibility,
                ineligible_reason: facts.ineligible_reason,
                experiment: facts.experiment,
                reset_arm: facts.reset_arm,
                reset_availability: facts.reset_availability,
                resets_per_week: facts.resets_per_week,
                next_reset_available: facts.next_reset_available,
                plan: facts.plan,
                factor_band: facts.factor_band,
                residual_band: facts.residual_band,
            },
            context: event::Context {
                app_version: format!("antiburn:{}", app.package_info().version),
                os: event::os_family(),
            },
        };
        if let Ok(json) = serde_json::to_string(&payload)
            && store.queue_analytics_event(name.as_str(), &json).is_ok()
        {
            if let Some(wake) = app.try_state::<DeliveryWake>() {
                wake.0.notify_one();
            }
            return true;
        }
        false
    }

    /// Record a changed Claude reset observation after all consent checks pass.
    pub fn record_claude_limit_reset(
        app: &tauri::AppHandle,
        diagnostic: crate::provider_usage::live::anthropic::LimitResetDiagnostic,
    ) {
        if !allowed(app) {
            return;
        }
        let mut last = LAST_CLAUDE_LIMIT_RESET
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *last == Some(diagnostic) {
            return;
        }
        if record_event(
            app,
            EventName::ClaudeLimitResetObserved,
            event::claude_limit_reset_facts(diagnostic),
        ) {
            *last = Some(diagnostic);
        }
    }

    /// Record a coarse usage band for every provider window an ordinary
    /// live-usage refresh just published, when analytics allows it.
    ///
    /// `snapshots` is expected to already carry only online, visible
    /// providers — see [`crate::provider_usage::live::sources::collect`] —
    /// so no further gate is applied here beyond [`allowed`]. A provider that
    /// failed this pass has no snapshot in the slice, so it contributes no
    /// observation and its last reported band is left exactly as it was.
    pub fn record_usage_observed(app: &tauri::AppHandle, snapshots: &[ProviderUsageSnapshot]) {
        if !allowed(app) {
            return;
        }
        let candidates = usage_observed_candidates(snapshots);
        if candidates.is_empty() {
            return;
        }
        let mut last = LAST_USAGE_OBSERVED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (label, detail, band) in candidates {
            if !usage_observed_is_new(&last, label, detail, band) {
                continue;
            }
            if record_event(
                app,
                EventName::UsageObserved,
                Facts {
                    label: Some(label),
                    detail: Some(detail),
                    usage_band: Some(band),
                    ..Facts::default()
                },
            ) {
                last.insert((label, detail), band);
            }
        }
    }

    /// Every `(provider, window role, band)` this build can report for one
    /// collected pass.
    ///
    /// Only the two windows that make up a provider's primary allowance
    /// count: the short rolling window and the long weekly or billing-period
    /// one. A supplemental or provider-named window is not part of this
    /// coarse picture, the same narrowing the milestone engine applies. A
    /// provider id this build does not recognize is skipped rather than
    /// given an invented label. `authoritative` is not checked — the band is
    /// coarse enough that even a derived figure stays useful here.
    fn usage_observed_candidates(
        snapshots: &[ProviderUsageSnapshot],
    ) -> Vec<(&'static str, &'static str, &'static str)> {
        snapshots
            .iter()
            .filter_map(|snapshot| {
                let label = LiveUsageProvider::from_provider_id(snapshot.provider)?.as_str();
                Some(snapshot.windows.iter().filter_map(move |window| {
                    let detail = match &window.role {
                        WindowRole::PrimaryShort => "short",
                        WindowRole::PrimaryLong => "long",
                        WindowRole::Supplemental | WindowRole::Other(_) => return None,
                    };
                    Some((label, detail, band_for_percent(window.used_percent)))
                }))
            })
            .flatten()
            .collect()
    }

    /// Whether `(label, detail)`'s band differs from the last one reported.
    ///
    /// A pure lookup rather than a mutating check, so the duplicate rule can
    /// be tested without the process-wide static behind it.
    fn usage_observed_is_new(
        last: &BTreeMap<(&'static str, &'static str), &'static str>,
        label: &'static str,
        detail: &'static str,
        band: &'static str,
    ) -> bool {
        last.get(&(label, detail)) != Some(&band)
    }

    /// A limit-factor pair's `(plan, factor band, residual band)` tuple.
    type LimitFactorTuple = (&'static str, &'static str, &'static str);

    /// The last tuple reported for each `(provider, lane)` pair, and when.
    type LastLimitFactorObserved =
        BTreeMap<(&'static str, &'static str), (LimitFactorTuple, Instant)>;

    /// One `(provider, lane)` pair's coarse dimensions, computed from a
    /// learning pass's [`LearnedFactor`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct LimitFactorObservation {
        label: &'static str,
        detail: &'static str,
        plan: &'static str,
        factor_band: &'static str,
        residual_band: &'static str,
    }

    /// The lane detail `antiburn.limit_factor_observed` reports, matching the
    /// vocabulary `antiburn.usage_observed` already uses for the same window
    /// roles.
    fn limit_factor_lane_detail(lane: &str) -> Option<&'static str> {
        match lane {
            crate::store::provider_limit::LANE_FIVE_HOUR => Some("short"),
            crate::store::provider_limit::LANE_WEEKLY => Some("long"),
            _ => None,
        }
    }

    /// Every `(provider, lane)` observation this build can report for one
    /// learning pass.
    ///
    /// A provider id this build does not recognize, or a lane outside the two
    /// this app tracks a factor for, is skipped rather than given an invented
    /// label — the same narrowing `usage_observed_candidates` applies.
    fn limit_factor_observed_candidates(learned: &[LearnedFactor]) -> Vec<LimitFactorObservation> {
        learned
            .iter()
            .filter_map(|factor| {
                let label = LiveUsageProvider::from_provider_id(&factor.provider)?.as_str();
                let detail = limit_factor_lane_detail(factor.lane)?;
                Some(LimitFactorObservation {
                    label,
                    detail,
                    plan: event::map_plan(factor.plan.as_deref()),
                    factor_band: event::factor_band(factor.usd_per_percent),
                    residual_band: event::residual_band(factor.residual),
                })
            })
            .collect()
    }

    /// Whether one `(label, detail)` pair's dimensions are worth a second
    /// event: the pair is new to this run, or its tuple changed and at least
    /// 24 hours have passed since the last fire. The 24-hour floor applies
    /// even to a genuine change, so a factor bouncing between two bands
    /// cannot report more than once a day.
    ///
    /// A pure lookup rather than a mutating check, so the rule can be tested
    /// without the process-wide static behind it.
    fn limit_factor_observed_is_new(
        last: &LastLimitFactorObserved,
        key: (&'static str, &'static str),
        tuple: LimitFactorTuple,
        now: Instant,
    ) -> bool {
        match last.get(&key) {
            None => true,
            Some((last_tuple, last_fired_at)) => {
                *last_tuple != tuple
                    && now.duration_since(*last_fired_at) >= LIMIT_FACTOR_MIN_INTERVAL
            }
        }
    }

    /// Record a coarse limit-factor observation for every `(provider, lane)`
    /// pair one learning pass touched, when analytics allows it.
    ///
    /// Multiple accounts on one provider collapse onto the same
    /// `(provider, lane)` key — the payload carries no account dimension, by
    /// design, so there is nothing to key a second observation on. Within one
    /// pass, only the first account processed for a pair can report; see
    /// `docs/plans/limit-factor-estimation.md`'s Phase 3 decisions.
    pub fn record_limit_factor_observed(app: &tauri::AppHandle, learned: &[LearnedFactor]) {
        if !allowed(app) {
            return;
        }
        let candidates = limit_factor_observed_candidates(learned);
        if candidates.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut last = LAST_LIMIT_FACTOR_OBSERVED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for observation in candidates {
            let key = (observation.label, observation.detail);
            let tuple = (
                observation.plan,
                observation.factor_band,
                observation.residual_band,
            );
            if !limit_factor_observed_is_new(&last, key, tuple, now) {
                continue;
            }
            if record_event(
                app,
                EventName::LimitFactorObserved,
                Facts {
                    label: Some(observation.label),
                    detail: Some(observation.detail),
                    plan: Some(observation.plan),
                    factor_band: Some(observation.factor_band),
                    residual_band: Some(observation.residual_band),
                    ..Facts::default()
                },
            ) {
                last.insert(key, (tuple, now));
            }
        }
    }

    /// Record an interaction reported by the renderer.
    ///
    /// The renderer names a shape, not an event. See [`Interaction`] for why.
    pub fn record_interaction(app: &tauri::AppHandle, interaction: Interaction) {
        if let Some((provider, state)) = deliberate_live_usage_observation(interaction) {
            record_live_usage_state(app, provider, state);
            return;
        }
        if matches!(interaction, Interaction::LiveUsageStateObserved { .. }) {
            return;
        }
        let (name, facts) = interaction.resolve();
        if !record_event(app, name, facts) {
            return;
        }
        match interaction {
            Interaction::SurfaceViewed {
                surface,
                origin: Origin::User,
            } if surface != Surface::Settings => note_deliberate_activity(Instant::now()),
            Interaction::SettingsPaneViewed {
                pane: SettingsPane::Insights,
            } => note_deliberate_activity(Instant::now()),
            _ => {}
        }
    }

    fn deliberate_live_usage_observation(
        interaction: Interaction,
    ) -> Option<(LiveUsageProvider, LiveUsageState)> {
        match interaction {
            Interaction::LiveUsageStateObserved {
                provider,
                state,
                origin: Origin::User,
            } => Some((provider, state)),
            _ => None,
        }
    }

    #[derive(Default)]
    struct DeliberateVisit {
        last_activity: Option<Instant>,
        live_usage_states: Vec<(LiveUsageProvider, LiveUsageState)>,
    }

    static DELIBERATE_VISIT: std::sync::Mutex<DeliberateVisit> =
        std::sync::Mutex::new(DeliberateVisit {
            last_activity: None,
            live_usage_states: Vec::new(),
        });

    fn advance_deliberate_visit(visit: &mut DeliberateVisit, now: Instant) {
        if visit
            .last_activity
            .is_none_or(|last| now.duration_since(last) >= DELIBERATE_VISIT_TIMEOUT)
        {
            visit.live_usage_states.clear();
        }
        visit.last_activity = Some(now);
    }

    fn note_deliberate_activity(now: Instant) {
        let mut visit = DELIBERATE_VISIT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        advance_deliberate_visit(&mut visit, now);
    }

    fn record_live_usage_state(
        app: &tauri::AppHandle,
        provider: LiveUsageProvider,
        state: LiveUsageState,
    ) {
        if !allowed(app) {
            return;
        }
        let now = Instant::now();
        let mut visit = DELIBERATE_VISIT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !visit_accepts_live_usage_state(&mut visit, provider, state, now) {
            return;
        }
        let (name, facts) = Interaction::LiveUsageStateObserved {
            provider,
            state,
            origin: Origin::User,
        }
        .resolve();
        if record_event(app, name, facts) {
            visit.live_usage_states.push((provider, state));
        }
    }

    fn visit_accepts_live_usage_state(
        visit: &mut DeliberateVisit,
        provider: LiveUsageProvider,
        state: LiveUsageState,
        now: Instant,
    ) -> bool {
        let Some(last_activity) = visit.last_activity else {
            return false;
        };
        if now.duration_since(last_activity) >= DELIBERATE_VISIT_TIMEOUT {
            visit.live_usage_states.clear();
            visit.last_activity = None;
            return false;
        }
        !visit.live_usage_states.contains(&(provider, state))
    }

    /// The last scan outcome reported in this run.
    ///
    /// In memory, like [`SESSION`], and for the same reason: it is a suppression
    /// hint, not a fact about the reader, and it has no business on their disk.
    static LAST_SCAN: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

    /// The last unknown-record outcome reported in this run.
    static LAST_UNRECOGNIZED: std::sync::Mutex<Option<UnrecognizedOutcome>> =
        std::sync::Mutex::new(None);

    /// The last Claude limit-reset observation queued during this run.
    static LAST_CLAUDE_LIMIT_RESET: std::sync::Mutex<
        Option<crate::provider_usage::live::anthropic::LimitResetDiagnostic>,
    > = std::sync::Mutex::new(None);

    /// The last band reported for each `(provider, window role)` pair queued
    /// during this run. In memory only, for the same reason every other
    /// suppression hint here is: it is a dedup key, not a fact worth keeping
    /// past this process.
    static LAST_USAGE_OBSERVED: std::sync::Mutex<
        BTreeMap<(&'static str, &'static str), &'static str>,
    > = std::sync::Mutex::new(BTreeMap::new());

    /// The last `(plan, factor band, residual band)` tuple reported for each
    /// `(provider, lane)` pair during this run, and when it was reported. In
    /// memory only, for the same reason [`LAST_USAGE_OBSERVED`] is: it is a
    /// dedup and rate-limit key, not a fact worth keeping past this process.
    static LAST_LIMIT_FACTOR_OBSERVED: std::sync::Mutex<LastLimitFactorObserved> =
        std::sync::Mutex::new(BTreeMap::new());

    #[derive(Debug, Clone, Copy, Default)]
    struct OnboardingCapture {
        flow: Option<OnboardingFlow>,
        started: bool,
        finished: bool,
    }

    static ONBOARDING_CAPTURE: std::sync::Mutex<OnboardingCapture> =
        std::sync::Mutex::new(OnboardingCapture {
            flow: None,
            started: false,
            finished: false,
        });

    static HUD_EXPOSURE_ORIGIN: std::sync::Mutex<Option<Origin>> = std::sync::Mutex::new(None);

    /// Begin a distinct restart flow after its pending state persists.
    pub fn prepare_onboarding_restart() {
        *ONBOARDING_CAPTURE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = OnboardingCapture {
            flow: Some(OnboardingFlow::Restart),
            started: false,
            finished: false,
        };
    }

    fn onboarding_flow(app: &tauri::AppHandle) -> OnboardingFlow {
        if app
            .try_state::<Store>()
            .is_some_and(|store| store.onboarding_flow_is_restart())
        {
            OnboardingFlow::Restart
        } else {
            OnboardingFlow::New
        }
    }

    /// Record the first successful reveal of the active setup flow.
    pub fn record_onboarding_started(app: &tauri::AppHandle) {
        let flow = onboarding_flow(app);
        let mut capture = ONBOARDING_CAPTURE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if capture.flow != Some(flow) {
            *capture = OnboardingCapture {
                flow: Some(flow),
                started: false,
                finished: false,
            };
        }
        if capture.started {
            return;
        }
        if record_event(
            app,
            EventName::OnboardingStarted,
            Facts {
                label: Some(flow.as_str()),
                ..Facts::default()
            },
        ) {
            capture.started = true;
        }
    }

    /// Record the committed completion of the active setup flow once.
    pub fn record_onboarding_finished(app: &tauri::AppHandle) {
        let flow = onboarding_flow(app);
        let mut capture = ONBOARDING_CAPTURE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if capture.flow != Some(flow) {
            *capture = OnboardingCapture {
                flow: Some(flow),
                started: false,
                finished: false,
            };
        }
        if capture.finished {
            return;
        }
        if record_event(
            app,
            EventName::OnboardingFinished,
            Facts {
                label: Some(flow.as_str()),
                ..Facts::default()
            },
        ) {
            capture.finished = true;
        }
    }

    /// Hold the origin until the HUD confirms an actual reveal.
    pub fn prepare_hud_exposure(origin: Origin) {
        *HUD_EXPOSURE_ORIGIN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(origin);
    }

    /// Cancel a pending HUD exposure when reveal cannot complete.
    pub fn cancel_hud_exposure() {
        *HUD_EXPOSURE_ORIGIN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    /// Take the origin for one confirmed HUD reveal.
    pub fn take_hud_exposure_origin(exposed: bool) -> Option<Origin> {
        if !exposed {
            return None;
        }
        HUD_EXPOSURE_ORIGIN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum UnrecognizedOutcome {
        None,
        Observed {
            label: &'static str,
            bucket: &'static str,
        },
    }

    impl UnrecognizedOutcome {
        fn facts(self) -> Option<Facts> {
            let Self::Observed { label, bucket } = self else {
                return None;
            };
            Some(Facts {
                bucket: Some(bucket),
                label: Some(label),
                ..Facts::default()
            })
        }
    }

    /// Record a discovery pass, if it says anything the previous one did not.
    ///
    /// `Some(count)` is a completed pass; `None` is a failed one. Call this
    /// only for a full pass. [`crate::scan::scan_report`] keeps a scoped
    /// pass — a watcher-burst retry of a handful of agents — from reaching
    /// this function at all: a scoped pass counts only its named agents, not
    /// the whole install, so its count is not comparable to a full pass's
    /// count and would flap the reported bucket on every burst.
    ///
    /// The scheduler runs a full pass every [`crate::scan::TICK`]. Reporting
    /// each one would put far more events into a channel whose other events
    /// are counted in ones, swamping the queue's own bound and every other
    /// event with it. It would also be the wrong measurement: what is worth
    /// knowing is roughly how large an install's history is and whether
    /// scanning works at all, and a repetition answers neither better than
    /// the first report did. A machine stuck failing every pass would
    /// additionally report that same failure over and over, which is not
    /// more information about one broken install.
    ///
    /// So the bucket, or the failure category, is compared against the last one
    /// reported and an unchanged outcome is dropped. What survives is the first
    /// pass of each run, every crossing of a bucket boundary, and every transition
    /// into or out of failure.
    pub fn record_scan(app: &tauri::AppHandle, sessions: Option<u64>) {
        // Ahead of the suppression check, not after it. A pass during onboarding,
        // or while the switch is off, must not leave a mark that then suppresses
        // the first pass the reader actually consented to.
        if !allowed(app) {
            return;
        }
        let (name, facts) = match sessions {
            Some(count) => (
                EventName::ScanCompleted,
                Facts {
                    bucket: Some(event::bucket(count)),
                    ..Facts::default()
                },
            ),
            None => (
                EventName::ErrorOccurred,
                Facts {
                    label: Some("scan_failed"),
                    ..Facts::default()
                },
            ),
        };
        if !scan_outcome_is_new(facts.bucket.or(facts.label)) {
            return;
        }
        record(app, name, facts);
    }

    /// Whether this outcome differs from the last one reported, remembering it if
    /// it does. Buckets and failure categories share the comparison deliberately:
    /// recovering from a failure is a change worth one event.
    fn scan_outcome_is_new(outcome: Option<&'static str>) -> bool {
        let mut guard = LAST_SCAN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *guard == outcome {
            return false;
        }
        *guard = outcome;
        true
    }

    /// Record a safe summary when an Insights cohort contains unknown types.
    pub fn record_unrecognized_records(app: &tauri::AppHandle, summary: &UnrecognizedRecords) {
        if !allowed(app) {
            return;
        }
        let outcome = unrecognized_records_outcome(summary);
        if !unrecognized_outcome_is_new(outcome) {
            return;
        }
        let Some(facts) = outcome.facts() else {
            return;
        };
        record(app, EventName::UnrecognizedRecordsObserved, facts);
    }

    fn unrecognized_records_outcome(summary: &UnrecognizedRecords) -> UnrecognizedOutcome {
        if summary.sessions_with_types == 0 {
            return UnrecognizedOutcome::None;
        }
        let label = if summary.evidence_bearing_sessions > 0 {
            "evidence_bearing"
        } else if summary.capped_sessions > 0 || summary.truncated_sessions > 0 {
            "inert_capped"
        } else {
            "inert_only"
        };
        UnrecognizedOutcome::Observed {
            label,
            bucket: event::bucket(summary.sessions_with_types),
        }
    }

    fn unrecognized_outcome_is_new(outcome: UnrecognizedOutcome) -> bool {
        let mut guard = LAST_UNRECOGNIZED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *guard == Some(outcome) {
            return false;
        }
        *guard = Some(outcome);
        true
    }

    /// Clear all in-memory suppression hints after consent withdrawal.
    fn reset_suppression() {
        *LAST_SCAN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *LAST_UNRECOGNIZED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *LAST_CLAUDE_LIMIT_RESET
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        LAST_USAGE_OBSERVED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        LAST_LIMIT_FACTOR_OBSERVED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        *DELIBERATE_VISIT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = DeliberateVisit::default();
        cancel_hud_exposure();
    }

    /// The identifier to stamp on an event, minting or rotating it as needed.
    fn current_install_id(store: &Store) -> Option<String> {
        let existing = store.analytics_identity().ok()?;
        if let Some((id, minted_at)) = existing
            && !older_than_lifetime(&minted_at)
        {
            return Some(id);
        }
        let fresh = random_identifier();
        store.set_analytics_identity(&fresh).ok()?;
        // A session identifier must not join the old and new installation IDs.
        reset_session();
        Some(fresh)
    }

    /// Serialize the two identifiers so rotation cannot produce a mixed pair.
    fn current_identity_pair(store: &Store) -> Option<(String, String)> {
        let _guard = IDENTITY_SESSION_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Some((current_install_id(store)?, current_session_id()))
    }

    /// Whether a mint stamp is old enough that the identifier should roll over.
    ///
    /// An unparsable stamp rotates rather than persisting: erring towards a new
    /// identifier loses a little continuity, and erring the other way would keep
    /// one alive indefinitely on a clock this code could not read.
    fn older_than_lifetime(minted_at: &str) -> bool {
        let Ok(minted) =
            time::OffsetDateTime::parse(minted_at, &time::format_description::well_known::Rfc3339)
        else {
            return true;
        };
        (time::OffsetDateTime::now_utc() - minted).whole_days() >= IDENTITY_LIFETIME_DAYS
    }

    /// The current run identifier, and when it was last touched.
    ///
    /// The generator state stays in memory. A restart mints a new value even
    /// inside the window. Queued event payloads keep their captured value.
    static SESSION: std::sync::Mutex<Option<(String, std::time::Instant)>> =
        std::sync::Mutex::new(None);

    static IDENTITY_SESSION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Serializes consent checks, capture, and consent withdrawal.
    static CAPTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The run identifier to stamp on an event, minting or rolling it as needed.
    fn current_session_id() -> String {
        let now = std::time::Instant::now();
        let mut guard = match SESSION.lock() {
            Ok(guard) => guard,
            // A poisoned lock means some earlier holder panicked. Taking the
            // value anyway is right here: the worst outcome is one run identifier
            // reused or replaced a little early, which nothing observes and which
            // is strictly less harmful than dropping the event.
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some((id, last_activity)) = guard.as_ref()
            && now.duration_since(*last_activity) < SESSION_TIMEOUT
        {
            let id = id.clone();
            *guard = Some((id.clone(), now));
            return id;
        }
        let fresh = random_identifier();
        *guard = Some((fresh.clone(), now));
        fresh
    }

    /// Forget the current run identifier, so the next event starts a new one.
    fn reset_session() {
        match SESSION.lock() {
            Ok(mut guard) => *guard = None,
            Err(poisoned) => *poisoned.into_inner() = None,
        }
    }

    /// A version-4 UUID from the system's randomness.
    ///
    /// Hand-formatted rather than pulling a crate for it: `getrandom` is already
    /// in this tree, and the whole requirement is sixteen unpredictable bytes that
    /// are not derived from anything about the machine.
    fn random_identifier() -> String {
        let mut bytes = [0u8; 16];
        if getrandom::fill(&mut bytes).is_err() {
            // Randomness being unavailable is not a reason to fall back to
            // something guessable or machine-derived; a nil identifier is
            // honestly useless, which is the correct failure here.
            return "00000000-0000-4000-8000-000000000000".to_string();
        }
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// React to a settings change. Called from the one transition hub in
    /// `commands.rs` so the queue can never drift out of step with the switch.
    ///
    /// Only withdrawal does anything here. Opting *in* needs no work — the
    /// identifier is minted lazily at the first event — and open windows learn
    /// the new state from `SETTINGS_CHANGED_EVENT`, which the same hub emits with
    /// the saved settings a moment later.
    pub fn handle_settings_transition(
        app: &tauri::AppHandle,
        previous: &AppSettings,
        saved: &AppSettings,
    ) {
        if saved.analytics_enabled || !previous.analytics_enabled {
            return;
        }
        let Some(store) = app.try_state::<Store>() else {
            reset_suppression();
            return;
        };
        clear_local_state(&store);
    }

    /// Remove all analytics state after either opt-out mechanism is used.
    fn clear_local_state(store: &Store) {
        // Opting out is immediate and total: anything already queued is withdrawn,
        // not merely paused, and both identifiers go with it. The run identifier
        // lives in memory rather than in the store, so it has to be dropped
        // separately or opting out and back in inside the same launch would resume
        // the session that was just withdrawn.
        {
            let _capture = CAPTURE_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let _ = store.clear_analytics();
            reset_session();
        }
        reset_suppression();
    }

    /// Install the TLS crypto provider this crate's client needs.
    ///
    /// `reqwest` is taken with `rustls-no-provider` so the provider is a decision
    /// rather than a default (see `Cargo.toml`), which means one has to be
    /// installed before any client is built. Idempotent: a second call, or one
    /// racing another, returns `Err` because a provider is already present, and
    /// that is the success case as much as `Ok` is.
    fn ensure_crypto_provider() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    /// Start the background flusher. A no-op in any build that cannot transmit.
    pub fn install(app: &tauri::AppHandle) {
        if environment_disabled() {
            if let Some(store) = app.try_state::<Store>() {
                clear_local_state(&store);
            }
            return;
        }
        if !available() {
            return;
        }
        ensure_crypto_provider();
        app.manage(DeliveryWake::default());
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let started = Instant::now();
            let initial_depth = handle
                .try_state::<Store>()
                .and_then(|store| store.analytics_event_count().ok())
                .unwrap_or(0);
            let mut schedule = DeliverySchedule::new(Duration::ZERO, initial_depth);
            loop {
                let now = started.elapsed();
                let Some(delay) = schedule.next_delay(now) else {
                    let wake = handle.state::<DeliveryWake>();
                    wake.0.notified().await;
                    let depth = handle
                        .try_state::<Store>()
                        .and_then(|store| store.analytics_event_count().ok())
                        .unwrap_or(0);
                    schedule.queued(started.elapsed(), depth);
                    continue;
                };
                let wake = handle.state::<DeliveryWake>();
                tokio::select! {
                    () = tokio::time::sleep(delay) => {
                        let outcome = flush_once(&handle).await;
                        schedule.flush_completed(started.elapsed(), outcome);
                    }
                    () = wake.0.notified() => {
                        let depth = handle
                            .try_state::<Store>()
                            .and_then(|store| store.analytics_event_count().ok())
                            .unwrap_or(0);
                        schedule.queued(started.elapsed(), depth);
                    }
                }
            }
        });
    }

    /// Deliver what is queued, if the reader still allows it.
    ///
    /// One request per event, which is the contract's shape, but drained from a
    /// local queue rather than fired at the call site: an event captured while the
    /// machine was offline is still worth sending later, and a call site that
    /// blocks on the network to report on itself has its priorities inverted.
    async fn flush_once(app: &tauri::AppHandle) -> FlushOutcome {
        if !allowed(app) {
            return FlushOutcome::Suspended;
        }
        let Some(base) = config::endpoint() else {
            return FlushOutcome::Suspended;
        };
        let Some(store) = app.try_state::<Store>() else {
            return FlushOutcome::Suspended;
        };
        let Ok(pending) = store.pending_analytics_events(delivery::REQUEST_BUDGET) else {
            return FlushOutcome::Suspended;
        };
        if pending.is_empty() {
            return FlushOutcome::Empty;
        }
        ensure_crypto_provider();
        let Ok(client) = reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() else {
            return FlushOutcome::Failed {
                remaining: store.analytics_event_count().unwrap_or(0),
            };
        };

        let track = format!("{}/v1/track", base.trim_end_matches('/'));
        let mut delivered = Vec::new();
        let mut failed = Vec::new();
        let mut suspended = false;
        for (id, payload) in pending {
            // Re-checked every iteration, not once before the loop. A drain of 50
            // events with a 10-second timeout each can outlive the reader's
            // decision, and the Privacy pane promises that switching the control
            // off withdraws what is queued — a batch that kept posting after the
            // switch moved would make that promise false in exactly the moment it
            // matters most.
            if !allowed(app) {
                suspended = true;
                break;
            }
            match stamp_sent_at(&payload) {
                // A row that cannot be parsed will never become parseable, so it
                // is dropped rather than retried until it ages out.
                None => delivered.push(id),
                Some(body) => {
                    let sent = client
                        .post(&track)
                        .header("content-type", "application/json")
                        .body(body)
                        .send()
                        .await
                        .is_ok_and(|response| response.status().is_success());
                    if sent {
                        delivered.push(id);
                    } else {
                        failed.push(id);
                        // One unreachable collector means the rest of this drain
                        // will fail too; stop rather than burning the attempt
                        // budget of every queued row on the same outage.
                        break;
                    }
                }
            }
        }

        if !delivered.is_empty() {
            let _ = store.drop_analytics_events(&delivered);
        }
        if !failed.is_empty() {
            let _ = store.fail_analytics_events(&failed, MAX_ATTEMPTS);
        }
        let remaining = store.analytics_event_count().unwrap_or(0);
        if remaining == 0 {
            FlushOutcome::Empty
        } else if suspended {
            FlushOutcome::Suspended
        } else if failed.is_empty() {
            FlushOutcome::Delivered { remaining }
        } else {
            FlushOutcome::Failed { remaining }
        }
    }

    /// Add the delivery stamp the collector uses to correct for clock skew.
    ///
    /// Kept out of the stored payload so a queued event reports when it was
    /// captured and, separately, when it actually went — which is the whole point
    /// of the pair.
    fn stamp_sent_at(payload: &str) -> Option<String> {
        let mut value: serde_json::Value = serde_json::from_str(payload).ok()?;
        value
            .as_object_mut()?
            .insert("sentAt".into(), crate::store::now_rfc3339().into());
        serde_json::to_string(&value).ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        static SUPPRESSION_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

        fn unrecognized_summary(
            sessions: u64,
            evidence_bearing: u64,
            capped: u64,
            truncated: u64,
        ) -> UnrecognizedRecords {
            UnrecognizedRecords {
                sessions_with_types: sessions,
                evidence_bearing_sessions: evidence_bearing,
                capped_sessions: capped,
                truncated_sessions: truncated,
                ..UnrecognizedRecords::default()
            }
        }

        /// The delivery client must actually build.
        ///
        /// `flush_once` swallows a builder error and returns, so a TLS
        /// misconfiguration would not crash, log, or fail any other test — it
        /// would simply mean nothing is ever sent, silently, in release builds
        /// only. That is the worst failure this module could have, and this is
        /// the cheapest possible guard against it.
        #[test]
        fn a_delivery_client_can_actually_be_built() {
            ensure_crypto_provider();
            reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("the analytics delivery client must build");
        }

        #[test]
        fn availability_requires_a_permitted_endpoint_and_an_operator() {
            assert_eq!(
                available(),
                config::endpoint().is_some() && config::operator().is_some()
            );
        }

        #[test]
        fn only_false_disables_analytics_from_the_environment() {
            for value in ["false", "FALSE", " false "] {
                assert!(disables_analytics(value), "{value}");
            }
            for value in ["", "0", "off", "true"] {
                assert!(!disables_analytics(value), "{value}");
            }
        }

        #[test]
        fn automatic_live_usage_observations_are_not_reportable() {
            let automatic = Interaction::LiveUsageStateObserved {
                provider: LiveUsageProvider::Anthropic,
                state: LiveUsageState::Fresh,
                origin: Origin::Automatic,
            };
            assert_eq!(deliberate_live_usage_observation(automatic), None);

            let user = Interaction::LiveUsageStateObserved {
                provider: LiveUsageProvider::Anthropic,
                state: LiveUsageState::Fresh,
                origin: Origin::User,
            };
            assert_eq!(
                deliberate_live_usage_observation(user),
                Some((LiveUsageProvider::Anthropic, LiveUsageState::Fresh))
            );
        }

        #[test]
        fn provider_polling_cannot_extend_a_deliberate_visit() {
            let started = Instant::now();
            let mut visit = DeliberateVisit::default();
            advance_deliberate_visit(&mut visit, started);
            assert!(visit_accepts_live_usage_state(
                &mut visit,
                LiveUsageProvider::Openai,
                LiveUsageState::Fresh,
                started + Duration::from_secs(1),
            ));
            visit
                .live_usage_states
                .push((LiveUsageProvider::Openai, LiveUsageState::Fresh));

            for elapsed in [60, 15 * 60, 29 * 60] {
                assert!(!visit_accepts_live_usage_state(
                    &mut visit,
                    LiveUsageProvider::Openai,
                    LiveUsageState::Fresh,
                    started + Duration::from_secs(elapsed),
                ));
                assert_eq!(visit.last_activity, Some(started));
            }

            assert!(!visit_accepts_live_usage_state(
                &mut visit,
                LiveUsageProvider::Openai,
                LiveUsageState::Stale,
                started + DELIBERATE_VISIT_TIMEOUT,
            ));
            assert_eq!(visit.last_activity, None);
            assert!(visit.live_usage_states.is_empty());
        }

        /// A minimal snapshot for one provider, with one window per
        /// `(role, used_percent)` pair given.
        fn usage_snapshot(
            provider: &'static str,
            windows: Vec<(WindowRole, Option<f64>)>,
        ) -> ProviderUsageSnapshot {
            ProviderUsageSnapshot {
                provider,
                account: None,
                account_uuid: None,
                account_email: None,
                plan: None,
                plan_tier: None,
                observed_at: time::OffsetDateTime::UNIX_EPOCH,
                source: crate::provider_usage::live::model::UsageSource {
                    id: "fixture",
                    label: "fixture".into(),
                    confidence: crate::provider_usage::live::Confidence::High,
                    freshness: crate::provider_usage::live::Freshness::Fresh,
                },
                windows: windows
                    .into_iter()
                    .enumerate()
                    .map(
                        |(index, (role, used_percent))| crate::provider_usage::live::UsageWindow {
                            id: format!("window-{index}"),
                            role,
                            kind: crate::provider_usage::live::UsageWindowKind::Rolling,
                            scope: crate::provider_usage::live::UsageScope::Account,
                            used_percent,
                            starts_at: None,
                            resets_at: None,
                            authoritative: true,
                        },
                    )
                    .collect(),
                supplemental: None,
                reset_credits: None,
            }
        }

        #[test]
        fn a_window_at_eighty_five_percent_reports_the_under_hundred_band() {
            let snapshot = usage_snapshot(
                crate::provider_usage::providers::ANTHROPIC,
                vec![(WindowRole::PrimaryShort, Some(85.0))],
            );
            assert_eq!(
                usage_observed_candidates(std::slice::from_ref(&snapshot)),
                vec![("anthropic", "short", "80_to_under_100")]
            );
        }

        #[test]
        fn short_and_long_windows_each_report_their_own_role() {
            let snapshot = usage_snapshot(
                crate::provider_usage::providers::OPENAI,
                vec![
                    (WindowRole::PrimaryShort, Some(10.0)),
                    (WindowRole::PrimaryLong, Some(100.0)),
                ],
            );
            assert_eq!(
                usage_observed_candidates(std::slice::from_ref(&snapshot)),
                vec![
                    ("openai", "short", "below_80"),
                    ("openai", "long", "at_limit"),
                ]
            );
        }

        #[test]
        fn a_supplemental_or_unrecognized_window_role_is_not_part_of_the_coarse_picture() {
            let snapshot = usage_snapshot(
                crate::provider_usage::providers::ANTHROPIC,
                vec![
                    (WindowRole::Supplemental, Some(95.0)),
                    (WindowRole::Other("daily".into()), Some(50.0)),
                ],
            );
            assert!(usage_observed_candidates(std::slice::from_ref(&snapshot)).is_empty());
        }

        #[test]
        fn a_provider_this_build_does_not_recognize_reports_nothing() {
            let snapshot = usage_snapshot(
                "some-future-provider",
                vec![(WindowRole::PrimaryShort, Some(50.0))],
            );
            assert!(usage_observed_candidates(std::slice::from_ref(&snapshot)).is_empty());
        }

        #[test]
        fn a_missing_percentage_is_unknown_rather_than_zero() {
            let snapshot = usage_snapshot(
                crate::provider_usage::providers::GOOGLE,
                vec![(WindowRole::PrimaryShort, None)],
            );
            assert_eq!(
                usage_observed_candidates(std::slice::from_ref(&snapshot)),
                vec![("google", "short", "unknown")]
            );
        }

        #[test]
        fn a_failed_providers_absent_snapshot_contributes_no_candidate() {
            // `collect` never puts a failed source's provider in `snapshots` —
            // see `provider_usage::live::sources::collect`. An empty slice is
            // what a pass where every source failed looks like here.
            assert!(usage_observed_candidates(&[]).is_empty());
        }

        #[test]
        fn a_repeated_band_is_not_worth_a_second_event_but_a_changed_one_is() {
            let mut last = BTreeMap::new();
            assert!(usage_observed_is_new(
                &last,
                "anthropic",
                "short",
                "below_80"
            ));
            last.insert(("anthropic", "short"), "below_80");
            assert!(!usage_observed_is_new(
                &last,
                "anthropic",
                "short",
                "below_80"
            ));
            assert!(usage_observed_is_new(
                &last,
                "anthropic",
                "short",
                "80_to_under_100"
            ));
            // A second provider's pair is judged independently of the first.
            assert!(usage_observed_is_new(&last, "openai", "short", "below_80"));
        }

        fn learned_factor(
            provider: &'static str,
            lane: &'static str,
            usd_per_percent: f64,
            plan: Option<&str>,
            residual: Option<(f64, f64)>,
        ) -> LearnedFactor {
            LearnedFactor {
                provider: provider.to_string(),
                lane,
                usd_per_percent,
                plan: plan.map(str::to_string),
                residual,
            }
        }

        #[test]
        fn a_learned_factor_reports_its_provider_lane_plan_and_bands() {
            let learned = vec![learned_factor(
                crate::provider_usage::providers::ANTHROPIC,
                crate::store::provider_limit::LANE_FIVE_HOUR,
                5.0,
                Some("Max"),
                Some((50.0, 48.0)),
            )];
            assert_eq!(
                limit_factor_observed_candidates(&learned),
                vec![LimitFactorObservation {
                    label: "anthropic",
                    detail: "short",
                    plan: "max",
                    factor_band: "2_to_under_8",
                    residual_band: "within_5",
                }]
            );
        }

        #[test]
        fn a_missing_residual_reports_the_unknown_band() {
            let learned = vec![learned_factor(
                crate::provider_usage::providers::OPENAI,
                crate::store::provider_limit::LANE_WEEKLY,
                40.0,
                None,
                None,
            )];
            let candidates = limit_factor_observed_candidates(&learned);
            assert_eq!(candidates[0].detail, "long");
            assert_eq!(candidates[0].plan, "unknown");
            assert_eq!(candidates[0].factor_band, "32_and_over");
            assert_eq!(candidates[0].residual_band, "unknown");
        }

        #[test]
        fn an_unrecognized_provider_or_lane_reports_nothing() {
            let unrecognized_provider = vec![learned_factor(
                "some-future-provider",
                crate::store::provider_limit::LANE_WEEKLY,
                5.0,
                None,
                None,
            )];
            assert!(limit_factor_observed_candidates(&unrecognized_provider).is_empty());

            let unrecognized_lane = vec![learned_factor(
                crate::provider_usage::providers::ANTHROPIC,
                "supplemental",
                5.0,
                None,
                None,
            )];
            assert!(limit_factor_observed_candidates(&unrecognized_lane).is_empty());
        }

        #[test]
        fn a_pair_fires_first_then_only_on_a_changed_tuple_at_least_a_day_later() {
            let mut last = BTreeMap::new();
            let key = ("anthropic", "short");
            let tuple_a = ("max", "2_to_under_8", "within_5");
            let tuple_b = ("max", "8_to_under_32", "within_5");
            let start = Instant::now();

            assert!(
                limit_factor_observed_is_new(&last, key, tuple_a, start),
                "the first observation for a pair always fires"
            );
            last.insert(key, (tuple_a, start));

            assert!(
                !limit_factor_observed_is_new(&last, key, tuple_a, start),
                "an unchanged tuple is not worth a second event"
            );
            assert!(
                !limit_factor_observed_is_new(
                    &last,
                    key,
                    tuple_b,
                    start + Duration::from_secs(3_600)
                ),
                "a changed tuple inside the 24-hour floor is still suppressed"
            );
            assert!(
                limit_factor_observed_is_new(
                    &last,
                    key,
                    tuple_b,
                    start + LIMIT_FACTOR_MIN_INTERVAL
                ),
                "a changed tuple past the floor fires again"
            );
            // A different pair is judged independently.
            assert!(limit_factor_observed_is_new(
                &last,
                ("openai", "short"),
                tuple_a,
                start
            ));
        }

        #[test]
        fn a_hidden_hud_cannot_consume_its_pending_origin() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            cancel_hud_exposure();
            prepare_hud_exposure(Origin::Automatic);

            assert_eq!(take_hud_exposure_origin(false), None);
            assert_eq!(take_hud_exposure_origin(true), Some(Origin::Automatic));
            assert_eq!(take_hud_exposure_origin(true), None);
        }

        #[test]
        fn an_environment_opt_out_clears_local_analytics_state() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            let directory = tempfile::tempdir().unwrap();
            let store = Store::open_in_memory(directory.path()).unwrap();
            store
                .set_analytics_identity("11111111-1111-4111-8111-111111111111")
                .unwrap();
            store.queue_analytics_event("app_launched", "{}").unwrap();

            clear_local_state(&store);

            assert!(store.pending_analytics_events(10).unwrap().is_empty());
            assert!(store.analytics_identity().unwrap().is_none());
        }

        #[test]
        fn identifiers_are_version_4_and_do_not_repeat() {
            let first = random_identifier();
            let second = random_identifier();
            assert_ne!(first, second);
            assert_eq!(first.len(), 36);
            assert_eq!(&first[14..15], "4");
            assert!(matches!(&first[19..20], "8" | "9" | "a" | "b"));
        }

        #[test]
        fn the_delivery_stamp_is_added_at_send_and_leaves_capture_time_alone() {
            let captured =
                r#"{"event":"antiburn.app_launched","originalTimestamp":"2026-08-18T00:00:00Z"}"#;
            let stamped = stamp_sent_at(captured).expect("stamps");
            let value: serde_json::Value = serde_json::from_str(&stamped).unwrap();
            assert_eq!(value["originalTimestamp"], "2026-08-18T00:00:00Z");
            assert!(value["sentAt"].is_string());
        }

        #[test]
        fn an_unparsable_queued_row_is_not_stamped() {
            assert!(stamp_sent_at("not json").is_none());
            // A JSON array is valid JSON but not an event; it has no object to
            // stamp, so it is dropped rather than retried forever.
            assert!(stamp_sent_at("[]").is_none());
        }

        /// The run identifier holds still for the length of a run and is dropped
        /// when consent is withdrawn.
        ///
        /// The first half is what makes the collector's rows coherent; the second
        /// is what stops opting out and straight back in from continuing the
        /// session that was just withdrawn. It is the only identifier here with
        /// no separate persisted generator state, so nothing else can assert this.
        #[test]
        fn a_run_identifier_is_stable_within_a_run_and_dropped_on_opt_out() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            reset_session();
            let first = current_session_id();
            assert_eq!(first, current_session_id(), "stable inside one run");
            assert_eq!(first.len(), 36);
            assert_eq!(&first[14..15], "4");

            reset_session();
            assert_ne!(first, current_session_id(), "withdrawn, not resumed");
            reset_session();
        }

        #[test]
        fn installation_rotation_cannot_reuse_or_mix_the_previous_session() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            let directory = tempfile::tempdir().unwrap();
            let store = std::sync::Arc::new(Store::open_in_memory(directory.path()).unwrap());
            let old_install = "11111111-1111-4111-8111-111111111111";
            store
                .set_analytics_identity_at(old_install, "2000-01-01T00:00:00Z")
                .unwrap();
            let old_payload = r#"{"anonymousId":"old","sessionId":"old-run"}"#;
            store
                .queue_analytics_event("antiburn.app_launched", old_payload)
                .unwrap();
            reset_session();
            let old_session = current_session_id();

            let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    let store = store.clone();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        current_identity_pair(&store).unwrap()
                    })
                })
                .collect();
            barrier.wait();
            let pairs: Vec<_> = workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect();

            assert_eq!(pairs[0], pairs[1]);
            assert_ne!(pairs[0].0, old_install);
            assert_ne!(pairs[0].1, old_session);
            assert_eq!(
                store.pending_analytics_events(10).unwrap()[0].1,
                old_payload
            );
            reset_session();
        }

        /// A frequent pass is the scheduler's business; a frequent event in the
        /// payload is not. Only a changed outcome survives, and recovering from a
        /// failure counts as a change.
        #[test]
        fn only_a_changed_scan_outcome_is_worth_an_event() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            *LAST_SCAN.lock().unwrap() = None;

            assert!(
                scan_outcome_is_new(Some("10-49")),
                "the first pass of a run"
            );
            assert!(!scan_outcome_is_new(Some("10-49")), "the same pass again");
            assert!(!scan_outcome_is_new(Some("10-49")));
            assert!(scan_outcome_is_new(Some("50-199")), "a bucket boundary");
            assert!(scan_outcome_is_new(Some("scan_failed")), "into failure");
            assert!(!scan_outcome_is_new(Some("scan_failed")), "still failing");
            assert!(scan_outcome_is_new(Some("50-199")), "out of failure");

            *LAST_SCAN.lock().unwrap() = None;
        }

        #[test]
        fn an_unrecognized_records_report_becomes_a_bucketed_event() {
            let inert = unrecognized_records_outcome(&unrecognized_summary(7, 0, 0, 0))
                .facts()
                .unwrap();
            assert_eq!(inert.label, Some("inert_only"));
            assert_eq!(inert.bucket, Some("1-9"));

            let capped = unrecognized_records_outcome(&unrecognized_summary(12, 0, 1, 0))
                .facts()
                .unwrap();
            assert_eq!(capped.label, Some("inert_capped"));
            assert_eq!(capped.bucket, Some("10-49"));

            let evidence = unrecognized_records_outcome(&unrecognized_summary(200, 1, 1, 1))
                .facts()
                .unwrap();
            assert_eq!(evidence.label, Some("evidence_bearing"));
            assert_eq!(evidence.bucket, Some("200-999"));
            assert!(
                unrecognized_records_outcome(&unrecognized_summary(0, 0, 0, 0))
                    .facts()
                    .is_none()
            );
        }

        #[test]
        fn only_a_changed_unrecognized_outcome_is_worth_an_event() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            reset_suppression();
            let inert = UnrecognizedOutcome::Observed {
                label: "inert_only",
                bucket: "1-9",
            };

            assert!(unrecognized_outcome_is_new(inert));
            assert!(!unrecognized_outcome_is_new(inert));
            assert!(unrecognized_outcome_is_new(UnrecognizedOutcome::None));
            assert!(!unrecognized_outcome_is_new(UnrecognizedOutcome::None));
            assert!(unrecognized_outcome_is_new(inert));
            reset_suppression();
        }

        #[test]
        fn withdrawing_consent_clears_every_suppression_hint() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            reset_suppression();
            let inert = UnrecognizedOutcome::Observed {
                label: "inert_only",
                bucket: "1-9",
            };
            assert!(scan_outcome_is_new(Some("1-9")));
            assert!(unrecognized_outcome_is_new(inert));
            *LAST_CLAUDE_LIMIT_RESET.lock().unwrap() = Some(
                crate::provider_usage::live::anthropic::empty_limit_reset_diagnostic(
                    "success", "null",
                ),
            );
            LAST_USAGE_OBSERVED
                .lock()
                .unwrap()
                .insert(("anthropic", "short"), "below_80");
            LAST_LIMIT_FACTOR_OBSERVED.lock().unwrap().insert(
                ("anthropic", "short"),
                (("max", "2_to_under_8", "within_5"), Instant::now()),
            );

            reset_suppression();

            assert!(scan_outcome_is_new(Some("1-9")));
            assert!(unrecognized_outcome_is_new(inert));
            assert_eq!(*LAST_CLAUDE_LIMIT_RESET.lock().unwrap(), None);
            assert!(LAST_USAGE_OBSERVED.lock().unwrap().is_empty());
            assert!(LAST_LIMIT_FACTOR_OBSERVED.lock().unwrap().is_empty());
            reset_suppression();
        }

        #[test]
        fn an_unreadable_mint_stamp_rotates_rather_than_persisting() {
            assert!(older_than_lifetime("not a timestamp"));
            assert!(older_than_lifetime("2000-01-01T00:00:00Z"));
            assert!(!older_than_lifetime(&crate::store::now_rfc3339()));
        }
    }
}

#[cfg(feature = "analytics")]
pub use enabled::*;
