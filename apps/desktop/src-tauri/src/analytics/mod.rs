//! Anonymised events about the application, behind build and opt-out gates.
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
//!   `sessionId` the contract also requires is weaker still — held in memory,
//!   never written to disk, and gone when the process exits.
//!
//! What this module cannot enforce is what happens after delivery. Retention
//! and IP handling belong to whoever operates the endpoint, are stated in the
//! privacy policy rather than in the app, and are deliberately not claimed by
//! any copy this repository ships.

#[cfg(feature = "analytics")]
pub mod config;
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
    let _ = (facts.bucket, facts.label, facts.detail);
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
        event::Interaction::UsageViewed {
            providers,
            evidence,
        } => {
            let _ = (providers, evidence);
        }
    }
}

#[cfg(not(feature = "analytics"))]
pub fn record_scan(_app: &tauri::AppHandle, _sessions: Option<u64>) {}

#[cfg(not(feature = "analytics"))]
pub fn record_unrecognized_records(
    _app: &tauri::AppHandle,
    _summary: &antiburn_local::insights::UnrecognizedRecords,
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

    use std::time::Duration;

    use antiburn_local::insights::UnrecognizedRecords;
    use tauri::Manager as _;

    use super::event::{Event, EventName, Facts, Interaction};
    use super::{config, event};
    use crate::store::{AppSettings, Store};

    /// How long an installation identifier lives before it is replaced.
    pub const IDENTITY_LIFETIME_DAYS: i64 = 30;

    /// How long a run identifier survives without activity before it is replaced.
    ///
    /// The collector's contract specifies this window, and matching it is the
    /// point — a client that invented its own would make its rows incomparable
    /// with every other surface reporting to the same place.
    pub const SESSION_TIMEOUT: Duration = Duration::from_secs(30 * 60);

    /// How many events one delivery attempt carries.
    const BATCH_SIZE: u32 = 50;

    /// How often the flusher wakes.
    const FLUSH_INTERVAL: Duration = Duration::from_secs(15 * 60);

    /// A short settling delay so launch does not compete with the first scan.
    const FIRST_FLUSH_DELAY: Duration = Duration::from_secs(60);

    /// How many failures a queued event survives before it is given up on.
    const MAX_ATTEMPTS: u32 = 5;

    /// How long one delivery may take before it is abandoned.
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

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
    fn allowed(app: &tauri::AppHandle) -> bool {
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
        if !allowed(app) {
            return;
        }
        let Some(store) = app.try_state::<Store>() else {
            return;
        };
        let Some(install_id) = current_install_id(&store) else {
            return;
        };
        let payload = Event {
            platform: event::PLATFORM,
            // Generated at capture, not at send: it is the collector's dedup key,
            // so it has to survive a retry unchanged or a redelivered batch counts
            // twice.
            message_id: random_identifier(),
            anonymous_id: install_id,
            session_id: current_session_id(),
            event: name.as_str().to_string(),
            original_timestamp: crate::store::now_rfc3339(),
            properties: event::Properties {
                arch: event::arch(),
                bucket: facts.bucket,
                label: facts.label,
                detail: facts.detail,
            },
            context: event::Context {
                app_version: format!("antiburn:{}", app.package_info().version),
                os: event::os_family(),
            },
        };
        if let Ok(json) = serde_json::to_string(&payload) {
            let _ = store.queue_analytics_event(name.as_str(), &json);
        }
    }

    /// Record an interaction reported by the renderer.
    ///
    /// The renderer names a shape, not an event. See [`Interaction`] for why.
    pub fn record_interaction(app: &tauri::AppHandle, interaction: Interaction) {
        let (name, facts) = interaction.resolve();
        record(app, name, facts);
    }

    /// The last scan outcome reported in this run.
    ///
    /// In memory, like [`SESSION`], and for the same reason: it is a suppression
    /// hint, not a fact about the reader, and it has no business on their disk.
    static LAST_SCAN: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

    /// The last unknown-record outcome reported in this run.
    static LAST_UNRECOGNIZED: std::sync::Mutex<Option<UnrecognizedOutcome>> =
        std::sync::Mutex::new(None);

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
                detail: None,
            })
        }
    }

    /// Record a discovery pass, if it says anything the previous one did not.
    ///
    /// `Some(count)` is a completed pass; `None` is a failed one.
    ///
    /// The scheduler runs a pass every [`crate::scan::TICK`] for as long as the
    /// popover is open, so reporting each one would put an event a minute, all
    /// day, into a channel whose other events are counted in ones — swamping the
    /// queue's own bound and every other event with it. It would also be the wrong
    /// measurement twice over: what is worth knowing is roughly how large an
    /// install's history is and whether scanning works at all, and a repetition
    /// answers neither better than the first report did. A machine stuck failing
    /// every pass would additionally report that failure some six hundred times a
    /// day, which is not more information about one broken install.
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
        Some(fresh)
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
    /// Deliberately in memory and nowhere else. A restart mints a new one even
    /// inside the window, which is the same behaviour the contract describes for
    /// a desktop client and is the weaker of the two options — a persisted
    /// session id would be a second durable identifier, and one is enough.
    static SESSION: std::sync::Mutex<Option<(String, std::time::Instant)>> =
        std::sync::Mutex::new(None);

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
        let _ = store.clear_analytics();
        reset_session();
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
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(FIRST_FLUSH_DELAY).await;
            loop {
                flush_once(&handle).await;
                tokio::time::sleep(FLUSH_INTERVAL).await;
            }
        });
    }

    /// Deliver what is queued, if the reader still allows it.
    ///
    /// One request per event, which is the contract's shape, but drained from a
    /// local queue rather than fired at the call site: an event captured while the
    /// machine was offline is still worth sending later, and a call site that
    /// blocks on the network to report on itself has its priorities inverted.
    async fn flush_once(app: &tauri::AppHandle) {
        if !allowed(app) {
            return;
        }
        let Some(base) = config::endpoint() else {
            return;
        };
        let Some(store) = app.try_state::<Store>() else {
            return;
        };
        let Ok(pending) = store.pending_analytics_events(BATCH_SIZE) else {
            return;
        };
        if pending.is_empty() {
            return;
        }
        ensure_crypto_provider();
        let Ok(client) = reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() else {
            return;
        };

        let track = format!("{}/v1/track", base.trim_end_matches('/'));
        let mut delivered = Vec::new();
        let mut failed = Vec::new();
        for (id, payload) in pending {
            // Re-checked every iteration, not once before the loop. A drain of 50
            // events with a 10-second timeout each can outlive the reader's
            // decision, and the Privacy pane promises that switching the control
            // off withdraws what is queued — a batch that kept posting after the
            // switch moved would make that promise false in exactly the moment it
            // matters most.
            if !allowed(app) {
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
        /// no on-disk representation, so nothing else can assert this.
        #[test]
        fn a_run_identifier_is_stable_within_a_run_and_dropped_on_opt_out() {
            reset_session();
            let first = current_session_id();
            assert_eq!(first, current_session_id(), "stable inside one run");
            assert_eq!(first.len(), 36);
            assert_eq!(&first[14..15], "4");

            reset_session();
            assert_ne!(first, current_session_id(), "withdrawn, not resumed");
            reset_session();
        }

        /// A pass a minute is the scheduler's business; a pass a minute in the
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

            reset_suppression();

            assert!(scan_outcome_is_new(Some("1-9")));
            assert!(unrecognized_outcome_is_new(inert));
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
