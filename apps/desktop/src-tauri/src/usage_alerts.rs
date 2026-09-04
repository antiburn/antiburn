//! The background live-usage and milestone monitor.
//!
//! The milestone engine ([`crate::provider_usage::live::milestones`])
//! evaluates whatever the registered sources report — and those now report
//! something: the Usage surface shows a provider's own limits, asked for
//! directly with the reader's own credentials.
//!
//! Milestone notifications are gated on `AppSettings::live_usage_active` —
//! the Settings → Usage switch (on by default) *and* onboarding having
//! finished — and that pairing is deliberate rather than leftover. Both
//! consequences of the switch depend on the same traffic: a milestone is a
//! statement about a threshold being *crossed*, which needs readings that
//! keep moving, and only the sources this switch unlocks ever make a request
//! to find out whether one has. So the one switch buys both, and its copy
//! names both. The onboarding half of the gate holds even while the switch
//! itself defaults on: no credential is read, and no request or subprocess
//! runs, until the reader has actually seen this app once.

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::dto::{LiveUsageFreshness, LiveUsageSummary};
use crate::provider_usage;
use crate::store::Store;

/// Emitted after a provider refresh replaces the cached live-usage snapshot.
pub const EVENT_CHANGED: &str = "live-usage:changed";

/// Where the last complete view payload survives an application restart.
const SNAPSHOT_KEY: &str = "internal:liveUsageSnapshotV2";

/// The last webview-reported UTC offset used to derive local-day metrics.
const UTC_OFFSET_KEY: &str = "internal:liveUsageUtcOffsetMinutes";

/// How often the monitor refreshes its snapshot and checks milestones.
const TICK: Duration = Duration::from_secs(300);

/// Let the first scans land before judging anything.
const STARTUP_DELAY: Duration = Duration::from_secs(120);

/// How fresh a reading the background pass asks each source's cooldown for.
///
/// Match the monitor tick so each pass can fetch when no newer foreground
/// reading exists. The shared source cooldown still caps background traffic
/// at one provider request per tick. See
/// `crate::commands::POPOVER_LIVE_USAGE_MAX_AGE` for the visible refresh budget.
const BACKGROUND_MAX_AGE: Duration = TICK;

/// Spawn the monitor loop; the handle joins the shell's scheduler registry.
pub fn spawn_scheduler(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let app = app.clone();
            blocking::run(move |blocking| run_pass(&app, blocking)).await;
        }
    })
}

/// The hop off the async runtime, and the proof that it happened.
///
/// A pass is blocking work end to end: it reads the store, and — through
/// [`provider_usage::live::sources`] — it asks a provider's endpoint with
/// `reqwest::blocking`. The second of those is not merely impolite on a
/// runtime worker, it is a panic: a blocking `reqwest` call builds and drops
/// a `tokio` runtime on the calling thread, and dropping a runtime from
/// inside an asynchronous context aborts with "Cannot drop a runtime in a
/// context where blocking is not allowed". Called inline, that panic unwound
/// the spawned task itself, so one tick took the whole milestone monitor
/// down for the rest of the process while the app carried on looking healthy.
///
/// So the hop is what makes a pass *correct*, not just what keeps a
/// fifteen-second request from stalling a worker — which is why it is a type
/// and not a convention. [`blocking::Thread`] has a private field and this
/// module is the only place that fills it, so [`run_pass`] cannot be reached
/// from the scheduler's task without going through [`blocking::run`] first.
/// A later refactor that drops the hop does not reintroduce the panic
/// quietly; it stops the crate compiling.
mod blocking {
    /// Proof that the holder is running where blocking is allowed.
    pub struct Thread(());

    /// Run `pass` on a blocking thread, and survive it failing.
    ///
    /// Awaiting the handle keeps ticks serialized the way calling the pass
    /// inline did, and folding the join error away means a pass that panics
    /// anyway costs one pass rather than every pass after it.
    pub async fn run<F: FnOnce(Thread) + Send + 'static>(pass: F) {
        if let Err(error) = tauri::async_runtime::spawn_blocking(move || pass(Thread(()))).await {
            ::tracing::error!(event = "usage_alert_pass_failed", error = %error);
        }
    }
}

/// The registered live usage sources and the milestone engine's ledger.
///
/// Held as app state so both the milestone pass below and
/// [`crate::commands::refresh_live_usage`] read the same registry — one place
/// that knows what this build can prove.
pub struct LiveUsage {
    pub sources: Vec<Box<dyn provider_usage::live::LiveUsageSource>>,
    ledger: std::sync::Mutex<provider_usage::live::MilestoneLedger>,
    summarizing: std::sync::Mutex<()>,
    snapshot: std::sync::Mutex<LiveUsageSummary>,
    utc_offset_minutes: std::sync::Mutex<i32>,
}

impl LiveUsage {
    pub fn new() -> LiveUsage {
        LiveUsage {
            sources: provider_usage::live::sources::registered(),
            ledger: std::sync::Mutex::default(),
            summarizing: std::sync::Mutex::default(),
            snapshot: std::sync::Mutex::default(),
            utc_offset_minutes: std::sync::Mutex::default(),
        }
    }

    /// Start with the last snapshot that the local store can read.
    pub fn from_store(store: &Store) -> LiveUsage {
        let live = LiveUsage::new();
        let mut snapshot: LiveUsageSummary = store
            .internal_value(SNAPSHOT_KEY)
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        for provider in &mut snapshot.providers {
            provider.freshness = LiveUsageFreshness::Stale;
        }
        live.replace_snapshot(snapshot, None);
        *live
            .utc_offset_minutes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = store
            .internal_value(UTC_OFFSET_KEY)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        live
    }

    /// Return the cached snapshot without reading a provider.
    pub fn snapshot(&self) -> LiveUsageSummary {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Replace and persist the snapshot produced by one complete refresh.
    pub fn replace_snapshot(&self, snapshot: LiveUsageSummary, store: Option<&Store>) {
        *self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot.clone();
        if let Some(store) = store
            && let Ok(raw) = serde_json::to_string(&snapshot)
        {
            store.set_internal_value(SNAPSHOT_KEY, &raw);
        }
    }

    /// Return the last offset reported by a webview.
    pub fn utc_offset_minutes(&self) -> i32 {
        *self
            .utc_offset_minutes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Remember the offset that local-day metrics must use.
    pub fn set_utc_offset_minutes(&self, value: i32, store: Option<&Store>) {
        let mut current = self
            .utc_offset_minutes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *current == value {
            return;
        }
        *current = value;
        if let Some(store) = store {
            store.set_internal_value(UTC_OFFSET_KEY, &value.to_string());
        }
    }

    /// Hold this for one whole summarization, and no two can overlap.
    ///
    /// `provider_usage::live::history::record` appends a pass's readings by
    /// loading the stored series, adding to it, and writing the whole thing
    /// back. That is safe while only one caller is ever mid-pass. The old
    /// synchronous command guaranteed this because it ran on the IPC thread.
    /// Now that the refresh command hands its work to a blocking thread,
    /// they can — the popover and Settings → Usage each ask on their own
    /// schedule — and an interleaved read-modify-write silently drops one
    /// pass's samples. This restores the guarantee explicitly, and spares the
    /// providers a second identical request while it is at it.
    pub fn summarizing(&self) -> std::sync::MutexGuard<'_, ()> {
        self.summarizing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for LiveUsage {
    fn default() -> LiveUsage {
        LiveUsage::new()
    }
}

fn run_pass(app: &AppHandle, _blocking: blocking::Thread) {
    let Some(store) = app.try_state::<Store>() else {
        return;
    };
    // Read fresh each pass, and default to not acting: an unreadable
    // preference is not permission (same rule as every notifier).
    let Ok(settings) = store.settings() else {
        return;
    };
    background_pass(app, &settings);
}

fn background_pass(app: &AppHandle, settings: &crate::store::AppSettings) {
    if !settings.live_usage_active() {
        return;
    }
    let _ = refresh_publish_and_evaluate(app, BACKGROUND_MAX_AGE, None);
}

/// Collect, publish, and evaluate one live-usage reading.
///
/// Every app-level refresh uses this path. This keeps milestone evaluation at
/// the collection boundary, including when the visible popover finds a crossing
/// between background ticks.
pub(crate) fn refresh_publish_and_evaluate(
    app: &AppHandle,
    max_age: Duration,
    utc_offset_minutes: Option<i32>,
) -> LiveUsageSummary {
    let Some(live) = app.try_state::<LiveUsage>() else {
        let now = crate::scan::unix_now();
        return LiveUsageSummary {
            generated_at: crate::store::iso_from_epoch(Some(now)),
            ..LiveUsageSummary::default()
        };
    };
    let _summarizing = live.summarizing();
    // Read after the lock because another collection can hold it through a request.
    let now = crate::scan::unix_now();
    let store = app.try_state::<Store>();
    if let Some(offset) = utc_offset_minutes {
        live.set_utc_offset_minutes(offset, store.as_deref());
    }
    let settings = store.as_deref().and_then(|store| store.settings().ok());
    let online = settings
        .as_ref()
        .is_some_and(|settings| settings.live_usage_active());
    let hidden = settings
        .as_ref()
        .map(|settings| &settings.live_usage_hidden_providers)
        .cloned()
        .unwrap_or_default();
    let collected = provider_usage::live::sources::collect(&live.sources, online, &hidden, max_age);
    let snapshots: Vec<provider_usage::live::milestones::LiveUsageSnapshot> =
        collected.snapshots.iter().map(milestone_snapshot).collect();
    let summary = provider_usage::live::summarize_collected(
        collected,
        provider_usage::live::roster(&live.sources, &hidden),
        store.as_deref(),
        now,
        live.utc_offset_minutes(),
    );
    live.replace_snapshot(summary.clone(), store.as_deref());
    let _ = app.emit(EVENT_CHANGED, &summary);
    if let Some(settings) = settings.as_ref() {
        evaluate_milestones(app, &live, settings, &snapshots, now);
    }
    summary
}

fn evaluate_milestones(
    app: &AppHandle,
    live: &LiveUsage,
    settings: &crate::store::AppSettings,
    snapshots: &[provider_usage::live::milestones::LiveUsageSnapshot],
    evaluated_at_epoch: i64,
) {
    // Gate before evaluation because selection records a crossing as delivered.
    // A disabled notification must remain available if the reader enables it.
    if !crate::notifications::allowed(settings, crate::notifications::Kind::UsageMilestone) {
        return;
    }
    let mut ledger = live
        .ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(content) = provider_usage::live::milestone_content(
        &mut ledger,
        snapshots,
        &settings.milestones_5h,
        &settings.milestones_weekly,
    ) {
        let crossing = &content.crossing;
        let cache_age_seconds = evaluated_at_epoch
            .saturating_sub(crossing.observed_at_epoch)
            .max(0);
        ::tracing::info!(
            event = "usage_milestone_selected",
            provider = %content.provider,
            window = %crossing.window_label,
            threshold_percent = crossing.threshold,
            used_percent = crossing.used_percent,
            elapsed_percent = crossing.elapsed_percent,
            observed_at_epoch = crossing.observed_at_epoch,
            evaluated_at_epoch,
            cache_age_seconds,
        );
        let _ = crate::notifications::note_usage_milestone(app, &content);
    }
}

/// The share of the window that has passed at `observed_at`, as a percentage.
///
/// A stated start defines the span. Without one, the known five-hour or
/// seven-day duration is measured backward from the reset.
fn window_elapsed_percent(
    observed_at: time::OffsetDateTime,
    starts_at: Option<time::OffsetDateTime>,
    resets_at: time::OffsetDateTime,
    fallback_duration: time::Duration,
) -> Option<f64> {
    let starts_at = starts_at.unwrap_or(resets_at - fallback_duration);
    let span_seconds = (resets_at - starts_at).whole_seconds();
    if span_seconds <= 0 {
        return None;
    }
    let elapsed_seconds = (observed_at - starts_at).whole_seconds();
    Some((elapsed_seconds as f64 / span_seconds as f64 * 100.0).clamp(0.0, 100.0))
}

/// Narrow a collected snapshot down to what the milestone engine needs.
///
/// The engine deals in two window classes, quota use, and elapsed time. The
/// collected model carries more than that. Three rules do the narrowing:
///
/// - A window with no stated reset is dropped. The reset epoch is part of the
///   window's identity, and without it a crossing could never re-arm — the
///   notification would fire once and then go quiet forever.
/// - A supplemental weekly window (a per-model limit) counts as weekly, so it
///   follows the weekly preference row rather than inventing a third one.
/// - Anything else — a daily limit, a provider-specific bucket — is dropped
///   rather than forced into the nearer of two classes it does not belong to.
fn milestone_snapshot(
    snapshot: &provider_usage::live::ProviderUsageSnapshot,
) -> provider_usage::live::milestones::LiveUsageSnapshot {
    use provider_usage::live::milestones::{LiveUsageSnapshot, LiveUsageWindow, UsageWindowClass};
    use provider_usage::live::{Freshness, UsageScope, UsageWindowKind, WindowRole};

    LiveUsageSnapshot {
        provider: snapshot.provider.to_string(),
        account: snapshot.account.clone(),
        fresh: snapshot.source.freshness == Freshness::Fresh,
        observed_at_epoch: snapshot.observed_at.unix_timestamp(),
        windows: snapshot
            .windows
            .iter()
            .filter_map(|window| {
                let class = match (&window.role, &window.kind) {
                    (WindowRole::PrimaryShort, _) => UsageWindowClass::Short,
                    (WindowRole::PrimaryLong, _)
                    | (WindowRole::Supplemental, UsageWindowKind::Weekly) => {
                        UsageWindowClass::Weekly
                    }
                    _ => return None,
                };
                let resets_at = window.resets_at?;
                let duration = match class {
                    UsageWindowClass::Short => time::Duration::hours(5),
                    UsageWindowClass::Weekly => time::Duration::days(7),
                };
                let elapsed_percent = window_elapsed_percent(
                    snapshot.observed_at,
                    window.starts_at,
                    resets_at,
                    duration,
                )?;
                Some(LiveUsageWindow {
                    id: window.id.clone(),
                    class,
                    label: match (&class, &window.scope) {
                        (UsageWindowClass::Short, _) => "5-hour limit".to_string(),
                        (UsageWindowClass::Weekly, UsageScope::Model(model)) => {
                            format!("{model} weekly limit")
                        }
                        (UsageWindowClass::Weekly, _) => "weekly limit".to_string(),
                    },
                    used_percent: window.used_percent?,
                    elapsed_percent,
                    resets_at_epoch: resets_at.unix_timestamp(),
                    authoritative: window.authoritative,
                })
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{
        LiveProviderUsage, LiveUsageFreshness, LiveUsageSourceError, LiveUsageSummary,
        LiveUsageSupport,
    };

    fn at(epoch: i64) -> time::OffsetDateTime {
        time::OffsetDateTime::from_unix_timestamp(epoch).unwrap()
    }

    #[test]
    fn elapsed_window_percentage_uses_a_stated_start_or_the_known_duration() {
        let reset = at(10 * 60 * 60);
        assert_eq!(
            window_elapsed_percent(at(7 * 60 * 60), None, reset, time::Duration::hours(5)),
            Some(40.0)
        );
        assert_eq!(
            window_elapsed_percent(
                at(4 * 60 * 60),
                Some(at(0)),
                reset,
                time::Duration::hours(5),
            ),
            Some(40.0)
        );
    }

    #[test]
    fn the_latest_live_usage_snapshot_survives_a_restart() {
        let store = Store::open_in_memory(std::path::Path::new("/tmp/antiburn-live-usage-cache"))
            .expect("in-memory store");
        let stored = LiveUsageSummary {
            meters: Vec::new(),
            providers: vec![LiveProviderUsage {
                provider: "openai".into(),
                account_key: None,
                display_name: "Codex".into(),
                support: LiveUsageSupport::Live,
                freshness: LiveUsageFreshness::Fresh,
                source_label: "fixture".into(),
                observed_at: "2026-08-20T00:00:00Z".into(),
                windows: vec![],
                extra_usage: None,
                reset_credits: None,
                plan: None,
                account_uuid: None,
                account_email: None,
            }],
            errors: vec![LiveUsageSourceError {
                source: "fixture".into(),
                provider: "openai".into(),
                display_name: "Codex".into(),
                category: "unavailable".into(),
            }],
            generated_at: "2026-08-20T00:00:00Z".into(),
        };

        LiveUsage::new().replace_snapshot(stored.clone(), Some(&store));

        let mut expected = stored;
        expected.providers[0].freshness = LiveUsageFreshness::Stale;
        assert_eq!(LiveUsage::from_store(&store).snapshot(), expected);
    }

    #[test]
    fn the_latest_utc_offset_survives_a_restart() {
        let store = Store::open_in_memory(std::path::Path::new("/tmp/antiburn-live-usage-offset"))
            .expect("in-memory store");
        let live = LiveUsage::new();

        live.set_utc_offset_minutes(630, Some(&store));

        assert_eq!(LiveUsage::from_store(&store).utc_offset_minutes(), 630);
    }

    #[test]
    fn an_unreadable_live_usage_snapshot_starts_empty() {
        let store = Store::open_in_memory(std::path::Path::new("/tmp/antiburn-live-usage-cache"))
            .expect("in-memory store");
        store.set_internal_value(SNAPSHOT_KEY, "not json");

        assert_eq!(
            LiveUsage::from_store(&store).snapshot(),
            LiveUsageSummary::default()
        );
    }

    /// A pass must land somewhere blocking is allowed.
    ///
    /// The type system already refuses a pass that skips [`blocking::run`] —
    /// [`blocking::Thread`] cannot be built outside that module — so what is
    /// left to prove is that the destination is real. The closure does
    /// exactly what `reqwest::blocking` does on the calling thread before a
    /// request: build a runtime, and drop it. That is the step that panics on
    /// a runtime worker, so a pass that gets through it is a pass that can
    /// reach a provider.
    #[test]
    fn a_pass_runs_where_blocking_is_allowed() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime with no driver enabled always builds");
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        runtime.block_on(blocking::run(move |_thread| {
            drop(
                tokio::runtime::Builder::new_current_thread()
                    .build()
                    .expect("a runtime with no driver enabled always builds"),
            );
            flag.store(true, Ordering::SeqCst);
        }));

        assert!(ran.load(Ordering::SeqCst));
    }
}
