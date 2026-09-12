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
#[cfg(feature = "analytics")]
mod resources;

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
pub fn install_schedulers(_app: &tauri::AppHandle, _schedulers: &crate::Schedulers) {}

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
        facts.unrecognized_types,
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
        event::Interaction::BurnCheckAutoFixReviewed { outcome } => {
            let _ = outcome;
        }
        event::Interaction::BurnCheckAutoFixConfirmed => {}
        event::Interaction::BurnCheckAutoFixCompleted { outcome } => {
            let _ = outcome;
        }
        event::Interaction::BurnCheckPromptPrepared { outcome } => {
            let _ = outcome;
        }
        event::Interaction::BurnCheckPromptCopied => {}
        event::Interaction::BurnCheckOutcomeObserved { outcome, origin } => {
            let _ = (outcome, origin);
        }
        event::Interaction::SessionFilterSelected { filter, agent } => {
            let _ = (filter, agent);
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

#[cfg(not(feature = "analytics"))]
pub fn prepare_opt_out_in_transaction(
    _app: &tauri::AppHandle,
    _transaction: &rusqlite::Transaction<'_>,
    _previous: &crate::store::AppSettings,
    _saved: &crate::store::AppSettings,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(feature = "analytics"))]
pub struct SettingsTransitionGuard;

#[cfg(not(feature = "analytics"))]
pub fn lock_settings_transition() -> SettingsTransitionGuard {
    SettingsTransitionGuard
}

#[cfg(feature = "analytics")]
mod enabled {

    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::atomic::{AtomicU64, Ordering};
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
    use super::{config, delivery, event, resources};
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

    /// Persist the withdrawal event with the preference change.
    ///
    /// The transaction makes the user's decision and the event durable as one
    /// unit. A crash after commit cannot restore consent or lose the signal.
    pub fn prepare_opt_out_in_transaction(
        app: &tauri::AppHandle,
        transaction: &rusqlite::Transaction<'_>,
        previous: &AppSettings,
        saved: &AppSettings,
    ) -> anyhow::Result<()> {
        prepare_opt_out_payload(
            transaction,
            previous,
            saved,
            &format!("antiburn:{}", app.package_info().version),
        )
    }

    fn prepare_opt_out_payload(
        transaction: &rusqlite::Transaction<'_>,
        previous: &AppSettings,
        saved: &AppSettings,
        app_version: &str,
    ) -> anyhow::Result<()> {
        if !previous.analytics_enabled
            || saved.analytics_enabled
            || !available()
            || environment_disabled()
        {
            return Ok(());
        }
        let Some((anonymous_id, _minted_at)) = Store::analytics_identity_in(transaction)? else {
            return Ok(());
        };
        let event = Event {
            platform: event::PLATFORM,
            message_id: random_identifier(),
            anonymous_id,
            session_id: current_session_id(),
            event: EventName::AnalyticsOptedOut.as_str().to_string(),
            original_timestamp: crate::store::now_rfc3339(),
            properties: event::Properties {
                arch: event::arch(),
                bucket: None,
                label: None,
                detail: None,
                origin: None,
                usage_band: None,
                response_shape: None,
                eligibility: None,
                ineligible_reason: None,
                experiment: None,
                reset_arm: None,
                reset_availability: None,
                resets_per_week: None,
                next_reset_available: None,
                plan: None,
                factor_band: None,
                residual_band: None,
                resource_usage: None,
                unrecognized_types: None,
            },
            context: event::Context {
                app_version: app_version.to_string(),
                os: event::os_family(),
            },
        };
        let payload = serde_json::to_string(&event)?;
        Store::queue_analytics_event_in(
            transaction,
            EventName::AnalyticsOptedOut.as_str(),
            &payload,
        )?;
        Ok(())
    }

    /// Serialize settings writes with opt-out cleanup.
    pub struct SettingsTransitionGuard {
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    pub fn lock_settings_transition() -> SettingsTransitionGuard {
        SettingsTransitionGuard {
            _guard: OPT_OUT_LIFECYCLE_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        }
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
        let _lifecycle = lock_settings_transition();
        let _capture = CAPTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        record_event_locked(app, name, facts)
    }

    fn record_event_locked(app: &tauri::AppHandle, name: EventName, facts: Facts) -> bool {
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
                resource_usage: facts.resource_usage,
                unrecognized_types: facts.unrecognized_types,
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
        let _lifecycle = lock_settings_transition();
        let _capture = CAPTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !allowed(app) {
            return;
        }
        let mut last = LAST_CLAUDE_LIMIT_RESET
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *last == Some(diagnostic) {
            return;
        }
        if record_event_locked(
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
        let _lifecycle = lock_settings_transition();
        let _capture = CAPTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
            if record_event_locked(
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
                let provider = LiveUsageProvider::from_provider_id(&factor.provider)?;
                let label = provider.as_str();
                let detail = limit_factor_lane_detail(factor.lane)?;
                Some(LimitFactorObservation {
                    label,
                    detail,
                    plan: event::map_plan(
                        provider,
                        factor.plan.as_deref(),
                        factor.plan_tier.as_deref(),
                    ),
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
        let _lifecycle = lock_settings_transition();
        let _capture = CAPTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
            if record_event_locked(
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
        let _lifecycle = lock_settings_transition();
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
        let _capture = CAPTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if record_event_locked(app, name, facts) {
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
        let _lifecycle = lock_settings_transition();
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
        let _capture = CAPTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if record_event_locked(
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
        let _lifecycle = lock_settings_transition();
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
        let _capture = CAPTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if record_event_locked(
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

    /// Maximum sanitized type names carried on one
    /// `antiburn.unrecognized_records_observed` event.
    const MAX_UNRECOGNIZED_TYPE_NAMES: usize = 16;

    /// Maximum bytes allowed for one sanitized type name.
    const MAX_UNRECOGNIZED_TYPE_NAME_BYTES: usize = 64;

    /// Stands in for a type name antiburn will not carry verbatim. It makes
    /// a rejection visible without leaking the value.
    const REJECTED_TYPE_NAME: &str = "<rejected>";

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum UnrecognizedOutcome {
        None,
        Observed {
            label: &'static str,
            bucket: &'static str,
            types: Vec<String>,
        },
    }

    impl UnrecognizedOutcome {
        fn facts(self) -> Option<Facts> {
            let Self::Observed {
                label,
                bucket,
                types,
            } = self
            else {
                return None;
            };
            Some(Facts {
                bucket: Some(bucket),
                label: Some(label),
                unrecognized_types: Some(types),
                ..Facts::default()
            })
        }
    }

    /// Reduce a report's unknown record type names to the bounded, sanitized
    /// list an event may carry.
    ///
    /// Keeps at most [`MAX_UNRECOGNIZED_TYPE_NAMES`] names. Each name must be
    /// non-empty, ASCII, and at most [`MAX_UNRECOGNIZED_TYPE_NAME_BYTES`]
    /// bytes long. Each name must use only letters, digits, `_`, `.`, `:`,
    /// `/`, and `-`. A name that fails this check becomes the fixed sentinel
    /// [`REJECTED_TYPE_NAME`]. This keeps a rejection visible without the
    /// value itself. The sentinel appears at most once. The result is
    /// sorted and has no duplicates.
    fn sanitize_unrecognized_types(types: &BTreeSet<String>) -> Vec<String> {
        let mut sanitized = BTreeSet::new();
        for name in types.iter().take(MAX_UNRECOGNIZED_TYPE_NAMES) {
            if is_safe_unrecognized_type_name(name) {
                sanitized.insert(name.clone());
            } else {
                sanitized.insert(REJECTED_TYPE_NAME.to_string());
            }
        }
        sanitized.into_iter().collect()
    }

    /// Whether a type name is safe to carry verbatim on an analytics event.
    fn is_safe_unrecognized_type_name(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= MAX_UNRECOGNIZED_TYPE_NAME_BYTES
            && name.is_ascii()
            && name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'/' | b'-')
            })
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
        if !unrecognized_outcome_is_new(outcome.clone()) {
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
            types: sanitize_unrecognized_types(&summary.types),
        }
    }

    /// Whether this outcome differs from the last one reported. Remembers
    /// the new outcome when it does. A changed sanitized type list counts
    /// as a change, the same as a changed label or bucket. A new unknown
    /// record name is exactly what this event exists to surface.
    fn unrecognized_outcome_is_new(outcome: UnrecognizedOutcome) -> bool {
        let mut guard = LAST_UNRECOGNIZED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.as_ref() == Some(&outcome) {
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

    /// Serializes ordinary and final queue drains.
    static FLUSH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    /// A consent change also resets and wakes the resource sampler. Opt-out
    /// queues one final signal, then withdraws all remaining local state.
    pub fn handle_settings_transition(
        app: &tauri::AppHandle,
        previous: &AppSettings,
        saved: &AppSettings,
    ) {
        if saved.analytics_enabled != previous.analytics_enabled {
            resources::settings_changed(app);
        }
        if saved.analytics_enabled {
            if !previous.analytics_enabled {
                finish_reenabled_opt_out(app);
            }
            return;
        }
        if !previous.analytics_enabled {
            return;
        }
        if app.try_state::<Store>().is_none() {
            reset_suppression();
            return;
        }
        let generation = begin_opt_out();
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = flush_opt_out_once(&handle, generation).await;
            finish_opt_out(&handle, generation);
        });
    }

    static NEXT_OPT_OUT_GENERATION: AtomicU64 = AtomicU64::new(0);
    static ACTIVE_OPT_OUT_GENERATION: AtomicU64 = AtomicU64::new(0);
    static OPT_OUT_LIFECYCLE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn begin_opt_out() -> u64 {
        let generation = NEXT_OPT_OUT_GENERATION
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let generation = if generation == 0 { 1 } else { generation };
        ACTIVE_OPT_OUT_GENERATION.store(generation, Ordering::Release);
        generation
    }

    fn opt_out_is_active(generation: u64) -> bool {
        ACTIVE_OPT_OUT_GENERATION.load(Ordering::Acquire) == generation
    }

    fn cancel_opt_out() {
        ACTIVE_OPT_OUT_GENERATION.store(0, Ordering::Release);
    }

    fn finish_reenabled_opt_out(app: &tauri::AppHandle) {
        cancel_opt_out();
        if let Some(store) = app.try_state::<Store>() {
            clear_local_state(&store);
        } else {
            reset_suppression();
        }
    }

    fn finish_opt_out(app: &tauri::AppHandle, generation: u64) {
        let _lifecycle = lock_settings_transition();
        let Some(store) = app.try_state::<Store>() else {
            if opt_out_is_active(generation) {
                cancel_opt_out();
                reset_suppression();
            }
            return;
        };
        // Re-enable can commit before this worker reaches cleanup. Check the
        // persisted preference while holding the lifecycle lock, so fresh
        // identity state is never removed by an old opt-out task.
        if opt_out_is_active(generation)
            && store
                .settings()
                .ok()
                .is_some_and(|settings| !settings.analytics_enabled)
        {
            clear_local_state(&store);
            cancel_opt_out();
        }
    }

    /// Remove all analytics state after either opt-out mechanism is used.
    fn clear_local_state(store: &Store) {
        // Final cleanup withdraws anything that was not delivered. The run
        // identifier lives in memory, so it also needs a separate reset.
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
        // A crash after the atomic opt-out transaction leaves its signal on
        // disk. The next launch completes withdrawal without retrying it.
        if let Some(store) = app.try_state::<Store>()
            && store.analytics_opt_out_pending().unwrap_or(false)
        {
            clear_local_state(&store);
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

    pub fn install_schedulers(app: &tauri::AppHandle, schedulers: &crate::Schedulers) {
        resources::install(app, schedulers);
    }

    pub(super) fn record_resource_usage(
        app: &tauri::AppHandle,
        generation: u64,
        summary: resources::schema::ResourceUsageSummary,
    ) {
        let _lifecycle = lock_settings_transition();
        let _capture = CAPTURE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(control) = app.try_state::<resources::ResourceSamplerControl>() else {
            return;
        };
        if control.generation() != generation {
            return;
        }
        let _ = record_event_locked(
            app,
            EventName::ResourceUsageObserved,
            Facts {
                resource_usage: Some(summary),
                ..Facts::default()
            },
        );
    }

    /// Deliver what is queued, if the reader still allows it.
    ///
    /// One request per event, which is the contract's shape, but drained from a
    /// local queue rather than fired at the call site: an event captured while the
    /// machine was offline is still worth sending later, and a call site that
    /// blocks on the network to report on itself has its priorities inverted.
    async fn flush_once(app: &tauri::AppHandle) -> FlushOutcome {
        flush_once_with_mode(app, None).await
    }

    /// Deliver one final bounded pass while the durable opt-out is active.
    async fn flush_opt_out_once(app: &tauri::AppHandle, generation: u64) -> FlushOutcome {
        match tokio::time::timeout(
            Duration::from_secs(30),
            flush_once_with_mode(app, Some(generation)),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(_) => FlushOutcome::Failed {
                remaining: app
                    .try_state::<Store>()
                    .and_then(|store| store.analytics_event_count().ok())
                    .unwrap_or(0),
            },
        }
    }

    async fn flush_once_with_mode(
        app: &tauri::AppHandle,
        opt_out_generation: Option<u64>,
    ) -> FlushOutcome {
        let permitted = |app: &tauri::AppHandle| match opt_out_generation {
            Some(generation) => {
                opt_out_is_active(generation)
                    && available()
                    && !environment_disabled()
                    && app
                        .try_state::<Store>()
                        .and_then(|store| store.settings().ok())
                        .is_some_and(|settings| !settings.analytics_enabled)
            }
            None => allowed(app),
        };
        if !permitted(app) {
            return FlushOutcome::Suspended;
        }
        if opt_out_generation.is_some()
            && !app
                .try_state::<Store>()
                .and_then(|store| store.analytics_opt_out_pending().ok())
                .unwrap_or(false)
        {
            return FlushOutcome::Empty;
        }
        let Some(base) = config::endpoint() else {
            return FlushOutcome::Suspended;
        };
        let Some(store) = app.try_state::<Store>() else {
            return FlushOutcome::Suspended;
        };
        flush_pending_events(&store, base, || permitted(app)).await
    }

    async fn flush_pending_events(
        store: &Store,
        base: &str,
        permitted: impl Fn() -> bool,
    ) -> FlushOutcome {
        flush_pending_events_with_timeout(store, base, permitted, REQUEST_TIMEOUT).await
    }

    async fn flush_pending_events_with_timeout(
        store: &Store,
        base: &str,
        permitted: impl Fn() -> bool,
        request_timeout: Duration,
    ) -> FlushOutcome {
        let _flush = FLUSH_LOCK.lock().await;
        let Ok(pending) = store.pending_analytics_events(delivery::REQUEST_BUDGET) else {
            return FlushOutcome::Suspended;
        };
        if pending.is_empty() {
            return FlushOutcome::Empty;
        }
        ensure_crypto_provider();
        let Ok(client) = reqwest::Client::builder().timeout(request_timeout).build() else {
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
            if !permitted() {
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
            unrecognized_summary_with_types(sessions, evidence_bearing, capped, truncated, &[])
        }

        fn unrecognized_summary_with_types(
            sessions: u64,
            evidence_bearing: u64,
            capped: u64,
            truncated: u64,
            types: &[&str],
        ) -> UnrecognizedRecords {
            UnrecognizedRecords {
                sessions_with_types: sessions,
                evidence_bearing_sessions: evidence_bearing,
                capped_sessions: capped,
                truncated_sessions: truncated,
                types: types.iter().map(|name| name.to_string()).collect(),
                ..UnrecognizedRecords::default()
            }
        }

        #[test]
        fn opt_out_generations_do_not_reuse_a_cancelled_token() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            cancel_opt_out();
            let first = begin_opt_out();
            cancel_opt_out();
            let second = begin_opt_out();

            assert_ne!(first, second);
            assert!(!opt_out_is_active(first));
            assert!(opt_out_is_active(second));
            cancel_opt_out();
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
            plan_tier: Option<&str>,
            residual: Option<(f64, f64)>,
        ) -> LearnedFactor {
            LearnedFactor {
                provider: provider.to_string(),
                lane,
                usd_per_percent,
                plan: plan.map(str::to_string),
                plan_tier: plan_tier.map(str::to_string),
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
                Some("default_claude_max_5x"),
                Some((50.0, 48.0)),
            )];
            assert_eq!(
                limit_factor_observed_candidates(&learned),
                vec![LimitFactorObservation {
                    label: "anthropic",
                    detail: "short",
                    plan: "max_5x",
                    factor_band: "4_to_under_8",
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
                None,
            )];
            let candidates = limit_factor_observed_candidates(&learned);
            assert_eq!(candidates[0].detail, "long");
            assert_eq!(candidates[0].plan, "unknown");
            assert_eq!(candidates[0].factor_band, "32_to_under_64");
            assert_eq!(candidates[0].residual_band, "unknown");
        }

        #[test]
        fn provider_tiers_reach_the_candidate_event_as_closed_plan_values() {
            let learned = vec![
                learned_factor(
                    crate::provider_usage::providers::ANTHROPIC,
                    crate::store::provider_limit::LANE_FIVE_HOUR,
                    5.0,
                    Some("max"),
                    Some("default_claude_max_20x"),
                    None,
                ),
                learned_factor(
                    crate::provider_usage::providers::OPENAI,
                    crate::store::provider_limit::LANE_FIVE_HOUR,
                    5.0,
                    Some("prolite"),
                    None,
                    None,
                ),
            ];
            let candidates = limit_factor_observed_candidates(&learned);
            assert_eq!(candidates[0].plan, "max_20x");
            assert_eq!(candidates[1].plan, "prolite");
        }

        #[test]
        fn an_unrecognized_provider_or_lane_reports_nothing() {
            let unrecognized_provider = vec![learned_factor(
                "some-future-provider",
                crate::store::provider_limit::LANE_WEEKLY,
                5.0,
                None,
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
                None,
            )];
            assert!(limit_factor_observed_candidates(&unrecognized_lane).is_empty());
        }

        #[test]
        fn a_pair_fires_first_then_only_on_a_changed_tuple_at_least_a_day_later() {
            let mut last = BTreeMap::new();
            let key = ("anthropic", "short");
            let tuple_a = ("max_5x", "2_to_under_4", "within_5");
            let tuple_b = ("max_20x", "2_to_under_4", "within_5");
            let tuple_c = ("max_5x", "4_to_under_8", "within_5");
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
                !limit_factor_observed_is_new(
                    &last,
                    key,
                    tuple_c,
                    start + Duration::from_secs(3_600)
                ),
                "a changed factor band inside the 24-hour floor is still suppressed"
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
            assert!(limit_factor_observed_is_new(
                &last,
                key,
                tuple_c,
                start + LIMIT_FACTOR_MIN_INTERVAL
            ));
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
            assert_eq!(inert.unrecognized_types, Some(Vec::new()));

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

        /// The event carries the sanitizer's own output: sorted,
        /// deduplicated, and never the raw report order.
        #[test]
        fn the_outcome_carries_the_sanitized_sorted_type_names() {
            let summary = unrecognized_summary_with_types(
                7,
                0,
                0,
                0,
                &["zzz_custom", "aaa_custom", "aaa_custom"],
            );
            let facts = unrecognized_records_outcome(&summary).facts().unwrap();
            assert_eq!(
                facts.unrecognized_types,
                Some(vec!["aaa_custom".to_string(), "zzz_custom".to_string()])
            );
        }

        #[test]
        fn a_normal_type_name_passes_the_sanitizer_unchanged() {
            let types = sanitize_unrecognized_types(&BTreeSet::from(["custom_event".to_string()]));
            assert_eq!(types, vec!["custom_event".to_string()]);
        }

        #[test]
        fn an_unsafe_type_name_becomes_the_rejected_sentinel() {
            let with_space =
                sanitize_unrecognized_types(&BTreeSet::from(["has space".to_string()]));
            assert_eq!(with_space, vec![REJECTED_TYPE_NAME.to_string()]);

            let with_non_ascii = sanitize_unrecognized_types(&BTreeSet::from(["café".to_string()]));
            assert_eq!(with_non_ascii, vec![REJECTED_TYPE_NAME.to_string()]);

            let too_long = sanitize_unrecognized_types(&BTreeSet::from(["a".repeat(65)]));
            assert_eq!(too_long, vec![REJECTED_TYPE_NAME.to_string()]);

            let empty = sanitize_unrecognized_types(&BTreeSet::from([String::new()]));
            assert_eq!(empty, vec![REJECTED_TYPE_NAME.to_string()]);
        }

        /// More than one rejected name still yields one sentinel. The
        /// sentinel shows that a rejection happened. It does not show how
        /// many names were rejected.
        #[test]
        fn multiple_rejected_names_collapse_to_one_sentinel() {
            let types = sanitize_unrecognized_types(&BTreeSet::from([
                "has space".to_string(),
                "café".to_string(),
                "custom_event".to_string(),
            ]));
            assert_eq!(
                types,
                vec![REJECTED_TYPE_NAME.to_string(), "custom_event".to_string()]
            );
        }

        #[test]
        fn the_sanitizer_keeps_at_most_sixteen_names() {
            let many: BTreeSet<String> = (0..20).map(|index| format!("type_{index:02}")).collect();
            let types = sanitize_unrecognized_types(&many);
            assert_eq!(types.len(), MAX_UNRECOGNIZED_TYPE_NAMES);
        }

        #[test]
        fn only_a_changed_unrecognized_outcome_is_worth_an_event() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            reset_suppression();
            let inert = UnrecognizedOutcome::Observed {
                label: "inert_only",
                bucket: "1-9",
                types: Vec::new(),
            };

            assert!(unrecognized_outcome_is_new(inert.clone()));
            assert!(!unrecognized_outcome_is_new(inert.clone()));
            assert!(unrecognized_outcome_is_new(UnrecognizedOutcome::None));
            assert!(!unrecognized_outcome_is_new(UnrecognizedOutcome::None));
            assert!(unrecognized_outcome_is_new(inert.clone()));

            // Same label and bucket, a new type name: still a new outcome.
            let inert_new_type = UnrecognizedOutcome::Observed {
                label: "inert_only",
                bucket: "1-9",
                types: vec!["custom_event".to_string()],
            };
            assert!(unrecognized_outcome_is_new(inert_new_type.clone()));
            assert!(!unrecognized_outcome_is_new(inert_new_type));
            reset_suppression();
        }

        #[test]
        fn withdrawing_consent_clears_every_suppression_hint() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            reset_suppression();
            let inert = UnrecognizedOutcome::Observed {
                label: "inert_only",
                bucket: "1-9",
                types: Vec::new(),
            };
            assert!(scan_outcome_is_new(Some("1-9")));
            assert!(unrecognized_outcome_is_new(inert.clone()));
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
                (("max", "2_to_under_4", "within_5"), Instant::now()),
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

        fn collector(
            expected_requests: usize,
            response_delay: Duration,
        ) -> (
            String,
            std::sync::mpsc::Receiver<Vec<u8>>,
            std::thread::JoinHandle<()>,
        ) {
            use std::io::{Read, Write};
            use std::net::TcpListener;

            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let endpoint = http_endpoint(
                &listener.local_addr().unwrap().ip().to_string(),
                listener.local_addr().unwrap().port(),
            );
            let (sent, received) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                for _ in 0..expected_requests {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0_u8; 4096];
                    let header_end = loop {
                        let read = stream.read(&mut buffer).unwrap();
                        if read == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..read]);
                        if let Some(end) =
                            request.windows(4).position(|window| window == b"\r\n\r\n")
                        {
                            break end + 4;
                        }
                    };
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    while request.len() < header_end + length {
                        let read = stream.read(&mut buffer).unwrap();
                        if read == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..read]);
                    }
                    sent.send(request[header_end..header_end + length].to_vec())
                        .unwrap();
                    if !response_delay.is_zero() {
                        std::thread::sleep(response_delay);
                    }
                    let _ = stream.write_all(
                        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            });
            (endpoint, received, worker)
        }

        fn http_endpoint(host: &str, port: u16) -> String {
            format!("{}://{}:{}", "http", host, port)
        }

        fn opt_out_transition_store() -> (Store, tempfile::TempDir) {
            let directory = tempfile::tempdir().unwrap();
            let store = Store::open_in_memory(directory.path()).unwrap();
            store
                .set_analytics_identity("11111111-1111-4111-8111-111111111111")
                .unwrap();
            store
                .queue_analytics_event("antiburn.app_launched", r#"{"event":"old"}"#)
                .unwrap();
            (store, directory)
        }

        #[test]
        fn the_production_transition_queues_one_fixed_signal_with_existing_identity() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            assert!(
                available(),
                "the enabled test suite needs analytics configuration"
            );
            let (store, _directory) = opt_out_transition_store();
            let mut disabled = store.settings().unwrap();
            disabled.analytics_enabled = false;

            let (previous, saved, ()) = store
                .replace_settings_with_transition(&disabled, |transaction, previous, saved| {
                    prepare_opt_out_payload(transaction, previous, saved, "antiburn:test")
                })
                .unwrap();

            assert!(previous.analytics_enabled);
            assert!(!saved.analytics_enabled);
            assert_eq!(
                store.analytics_identity().unwrap().unwrap().0,
                "11111111-1111-4111-8111-111111111111"
            );
            let pending = store.pending_analytics_events(50).unwrap();
            assert_eq!(pending.len(), 2);
            assert!(pending[0].1.contains("antiburn.analytics_opted_out"));
            let payload: serde_json::Value = serde_json::from_str(&pending[0].1).unwrap();
            assert_eq!(payload["event"], "antiburn.analytics_opted_out");
            assert_eq!(
                payload["anonymousId"],
                "11111111-1111-4111-8111-111111111111"
            );
            let properties = payload["properties"].as_object().unwrap();
            assert_eq!(properties["arch"], event::arch());
            assert!(
                properties
                    .iter()
                    .filter(|(key, _)| key.as_str() != "arch")
                    .all(|(_, value)| value.is_null())
            );
        }

        #[test]
        fn final_transition_delivery_drains_the_signal_and_backlog_before_cleanup() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            assert!(
                available(),
                "the enabled test suite needs analytics configuration"
            );
            let (store, _directory) = opt_out_transition_store();
            let mut disabled = store.settings().unwrap();
            disabled.analytics_enabled = false;
            store
                .replace_settings_with_transition(&disabled, |transaction, previous, saved| {
                    prepare_opt_out_payload(transaction, previous, saved, "antiburn:test")
                })
                .unwrap();
            let (endpoint, received, worker) = collector(2, Duration::ZERO);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let outcome = runtime.block_on(flush_pending_events_with_timeout(
                &store,
                &endpoint,
                || true,
                Duration::from_secs(2),
            ));
            assert_eq!(outcome, FlushOutcome::Empty);
            let first: serde_json::Value =
                serde_json::from_slice(&received.recv().unwrap()).unwrap();
            assert_eq!(first["event"], "antiburn.analytics_opted_out");
            let _second = received.recv().unwrap();
            worker.join().unwrap();
            assert_eq!(store.analytics_event_count().unwrap(), 0);
            assert!(store.analytics_identity().unwrap().is_some());

            clear_local_state(&store);
            assert!(store.analytics_identity().unwrap().is_none());
            assert_eq!(store.analytics_event_count().unwrap(), 0);
        }

        #[test]
        fn failed_or_timed_out_final_delivery_still_cleans_up_without_retry() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            assert!(
                available(),
                "the enabled test suite needs analytics configuration"
            );
            for (delay, timeout) in [
                (Duration::ZERO, Duration::from_secs(2)),
                (Duration::from_millis(100), Duration::from_millis(10)),
            ] {
                let (store, _directory) = opt_out_transition_store();
                let mut disabled = store.settings().unwrap();
                disabled.analytics_enabled = false;
                store
                    .replace_settings_with_transition(&disabled, |transaction, previous, saved| {
                        prepare_opt_out_payload(transaction, previous, saved, "antiburn:test")
                    })
                    .unwrap();
                let endpoint = if delay.is_zero() {
                    // No listener means a refused connection tests a transport failure.
                    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
                    let endpoint = http_endpoint(
                        &listener.local_addr().unwrap().ip().to_string(),
                        listener.local_addr().unwrap().port(),
                    );
                    drop(listener);
                    endpoint
                } else {
                    let (endpoint, _received, worker) = collector(1, delay);
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    let outcome = runtime.block_on(flush_pending_events_with_timeout(
                        &store,
                        &endpoint,
                        || true,
                        timeout,
                    ));
                    assert!(matches!(outcome, FlushOutcome::Failed { .. }));
                    let _ = worker.join();
                    clear_local_state(&store);
                    assert!(store.analytics_identity().unwrap().is_none());
                    assert_eq!(store.analytics_event_count().unwrap(), 0);
                    continue;
                };
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let outcome = runtime.block_on(flush_pending_events_with_timeout(
                    &store,
                    &endpoint,
                    || true,
                    timeout,
                ));
                assert!(matches!(outcome, FlushOutcome::Failed { .. }));
                clear_local_state(&store);
                assert!(store.analytics_identity().unwrap().is_none());
                assert_eq!(store.analytics_event_count().unwrap(), 0);
            }
        }

        #[test]
        fn a_second_disabled_transition_does_not_append_a_duplicate_signal() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            assert!(
                available(),
                "the enabled test suite needs analytics configuration"
            );
            let (store, _directory) = opt_out_transition_store();
            let mut disabled = store.settings().unwrap();
            disabled.analytics_enabled = false;
            store
                .replace_settings_with_transition(&disabled, |transaction, previous, saved| {
                    prepare_opt_out_payload(transaction, previous, saved, "antiburn:test")
                })
                .unwrap();
            let (_, _, ()) = store
                .replace_settings_with_transition(&disabled, |transaction, previous, saved| {
                    prepare_opt_out_payload(transaction, previous, saved, "antiburn:test")
                })
                .unwrap();
            assert_eq!(
                store
                    .pending_analytics_events(50)
                    .unwrap()
                    .iter()
                    .filter(|(_, payload)| payload.contains("antiburn.analytics_opted_out"))
                    .count(),
                1
            );
        }

        #[test]
        fn final_delivery_gate_can_stop_a_pass_before_it_sends_or_removes_rows() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            assert!(
                available(),
                "the enabled test suite needs analytics configuration"
            );
            let (store, _directory) = opt_out_transition_store();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let outcome = runtime.block_on(flush_pending_events_with_timeout(
                &store,
                &http_endpoint("127.0.0.1", 1),
                || false,
                Duration::from_millis(10),
            ));
            assert_eq!(outcome, FlushOutcome::Suspended);
            assert_eq!(store.analytics_event_count().unwrap(), 1);
            clear_local_state(&store);
            assert_eq!(store.analytics_event_count().unwrap(), 0);
            assert!(store.analytics_identity().unwrap().is_none());
        }

        #[test]
        fn opt_out_preparation_without_an_identity_does_not_mint_or_queue() {
            let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
            assert!(
                available(),
                "the enabled test suite needs analytics configuration"
            );
            let directory = tempfile::tempdir().unwrap();
            let store = Store::open_in_memory(directory.path()).unwrap();
            let mut disabled = store.settings().unwrap();
            disabled.analytics_enabled = false;
            store
                .replace_settings_with_transition(&disabled, |transaction, previous, saved| {
                    prepare_opt_out_payload(transaction, previous, saved, "antiburn:test")
                })
                .unwrap();
            assert!(store.analytics_identity().unwrap().is_none());
            assert_eq!(store.analytics_event_count().unwrap(), 0);
        }
    }
}

#[cfg(feature = "analytics")]
pub use enabled::*;
