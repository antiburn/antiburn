// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The usage-alert monitor: spend anomalies now, live milestones when a
//! source exists.
//!
//! # The anomaly rule
//!
//! "Unusually fast" is defined against the machine's own history, not a
//! tuned constant: the last hour's estimated spend must be at least a
//! quarter of the trailing week's total, *and* clear an absolute floor.
//! Someone whose hour equals a quarter of their week is having a remarkable
//! hour by construction — the rule normalizes itself to heavy and light
//! users alike — and the floor keeps a quiet machine (where $2 can be a
//! quarter of the week) from being interrupted over pocket change.
//!
//! Every figure is a local estimate priced from the bundled catalog
//! ([`crate::provider_usage::spend_between`]); the copy says so. Episodes
//! repeat at most once per [`EPISODE_SECS`], persisted through
//! [`Store::internal_value`] so a relaunch mid-episode stays quiet.
//!
//! # Milestones
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

use tauri::{AppHandle, Manager};

use crate::dto::{LiveUsageFreshness, LiveUsageSummary};
use crate::provider_usage;
use crate::store::Store;

/// Emitted after a provider refresh replaces the cached live-usage snapshot.
pub const EVENT_CHANGED: &str = "live-usage:changed";

/// Where the last complete view payload survives an application restart.
const SNAPSHOT_KEY: &str = "internal:liveUsageSnapshot";

/// How often the monitor looks. Coarser than the scan tick on purpose: an
/// anomaly is a trend, and a trend does not change by the minute.
const TICK: Duration = Duration::from_secs(300);

/// Let the first scans land before judging anything.
const STARTUP_DELAY: Duration = Duration::from_secs(120);

/// The trailing window the anomaly is measured over, and its baseline.
const HOUR_SECS: i64 = 60 * 60;
const WEEK_SECS: i64 = 7 * 24 * 60 * 60;

/// The last hour must be at least this fraction of the trailing week.
const WEEK_FRACTION: f64 = 0.25;

/// …and at least this many estimated dollars, so a quiet machine's quarter
/// is not an interruption.
const FLOOR_USD: f64 = 10.0;

/// Minimum quiet time between anomaly notifications.
const EPISODE_SECS: i64 = 6 * 60 * 60;

/// Where the last-fired moment survives a relaunch.
const FIRED_KEY: &str = "internal:usageAnomalyFiredEpoch";

/// How fresh a reading the milestone pass asks each source's cooldown for.
///
/// This pass is not shown to anyone — it runs on [`TICK`], whether or not the
/// popover is even open — so there is no reason to pay for a fetch more
/// often than the pass itself runs, and every reason not to: it is exactly
/// the traffic the sources' cooldown (`provider_usage::live::sources::cooldown`)
/// exists to keep to a small, predictable trickle. Ten minutes matches what
/// every source's cooldown fixed unconditionally before `max_age` existed,
/// so this pass's background traffic is unchanged by the popover's own,
/// much shorter, `max_age` — see `crate::commands::POPOVER_LIVE_USAGE_MAX_AGE`.
const MILESTONE_MAX_AGE: Duration = Duration::from_secs(600);

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
/// the spawned task itself, so one tick took the whole monitor — anomalies
/// and milestones alike — down for the rest of the process while the app
/// carried on looking healthy.
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
        if tauri::async_runtime::spawn_blocking(move || pass(Thread(())))
            .await
            .is_err()
        {
            eprintln!("antiburn: a usage-alert pass failed; the monitor continues");
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
}

impl LiveUsage {
    pub fn new() -> LiveUsage {
        LiveUsage {
            sources: provider_usage::live::sources::registered(),
            ledger: std::sync::Mutex::default(),
            summarizing: std::sync::Mutex::default(),
            snapshot: std::sync::Mutex::default(),
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
    let now = crate::scan::unix_now();
    anomaly_pass(app, &store, &settings, now);
    milestone_pass(app, &settings);
}

fn anomaly_pass(app: &AppHandle, store: &Store, settings: &crate::store::AppSettings, now: i64) {
    if !settings.notifications_enabled || !settings.notify_usage_anomalies {
        return;
    }

    let last_fired: i64 = store
        .internal_value(FIRED_KEY)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if now.saturating_sub(last_fired) < EPISODE_SECS {
        return;
    }

    let Ok(evidence) = store.usage_evidence(now - WEEK_SECS) else {
        return;
    };
    let Some((hour_usd, week_usd)) = anomaly(&evidence, now) else {
        return;
    };

    if crate::notifications::note_usage_anomaly(app, hour_usd, week_usd) {
        store.set_internal_value(FIRED_KEY, &now.to_string());
    }
}

fn milestone_pass(app: &AppHandle, settings: &crate::store::AppSettings) {
    // Gate *before* evaluating: selection is destructive (a chosen crossing
    // is recorded as delivered), so a crossing selected while notifications
    // were off would be silently consumed. `live_usage_active` folds in both
    // the reader's switch and onboarding having finished, so this can never
    // fire — and never even collect, which is what would read a credential —
    // before onboarding is done, whatever the switch's default is.
    if !settings.live_usage_active()
        || !crate::notifications::allowed(settings, crate::notifications::Kind::UsageMilestone)
    {
        return;
    }
    let Some(live) = app.try_state::<LiveUsage>() else {
        return;
    };
    let snapshots: Vec<provider_usage::live::milestones::LiveUsageSnapshot> =
        provider_usage::live::sources::collect(
            &live.sources,
            settings.live_usage_active(),
            MILESTONE_MAX_AGE,
        )
        .snapshots
        .iter()
        .map(milestone_snapshot)
        .collect();
    let mut ledger = live
        .ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(content) = provider_usage::live::milestone_content(
        &mut ledger,
        &snapshots,
        settings.milestones_5h,
        settings.milestones_weekly,
    ) {
        let _ = crate::notifications::note_usage_milestone(app, &content);
    }
}

/// Narrow a collected snapshot down to what the milestone engine needs.
///
/// The engine deals in two window classes and an integer percentage; the
/// collected model carries more than that, and most of the extra is exactly
/// what a *notification* should not try to say. Three rules do the narrowing:
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
                    resets_at_epoch: window.resets_at?.unix_timestamp(),
                    authoritative: window.authoritative,
                })
            })
            .collect(),
    }
}

/// The pure half of the rule, separated so the arithmetic is testable
/// without a store or a clock.
fn anomaly(evidence: &[crate::store::UsageEvidenceRecord], now: i64) -> Option<(f64, f64)> {
    let hour_usd = provider_usage::spend_between(evidence, now - HOUR_SECS, now + 1);
    if hour_usd < FLOOR_USD {
        return None;
    }
    let week_usd = provider_usage::spend_between(evidence, now - WEEK_SECS, now + 1);
    (hour_usd >= week_usd * WEEK_FRACTION).then_some((hour_usd, week_usd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{
        LiveProviderUsage, LiveUsageFreshness, LiveUsageSourceError, LiveUsageSummary,
        LiveUsageSupport,
    };
    use crate::store::UsageEvidenceRecord;

    #[test]
    fn the_latest_live_usage_snapshot_survives_a_restart() {
        let store = Store::open_in_memory(std::path::Path::new("/tmp/antiburn-live-usage-cache"))
            .expect("in-memory store");
        let stored = LiveUsageSummary {
            providers: vec![LiveProviderUsage {
                provider: "openai".into(),
                display_name: "Codex".into(),
                support: LiveUsageSupport::Live,
                freshness: LiveUsageFreshness::Fresh,
                source_label: "fixture".into(),
                observed_at: "2026-08-20T00:00:00Z".into(),
                windows: vec![],
                extra_usage: None,
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

    fn record(epoch: i64, output_tokens: u64) -> UsageEvidenceRecord {
        UsageEvidenceRecord {
            agent: "claude-code".into(),
            updated_at_epoch: epoch,
            model_breakdown_json: Some(format!(
                "{{\"claude-sonnet-4-5\":{{\"input_tokens\":0,\"output_tokens\":{output_tokens},\
                 \"cache_read_tokens\":0,\"cache_creation_tokens\":0}}}}"
            )),
        }
    }

    const NOW: i64 = 1_700_000_000;

    /// Output tokens that price above the floor with the bundled catalog.
    /// (Sonnet output is $15/M at the time of writing; a million tokens is
    /// comfortably past any plausible floor without depending on the exact
    /// price.)
    const BIG: u64 = 5_000_000;

    #[test]
    fn a_quiet_hour_is_no_anomaly_however_quiet_the_week() {
        // Small hour, small week: under the floor, silent.
        let evidence = vec![record(NOW - 100, 1_000)];
        assert_eq!(anomaly(&evidence, NOW), None);
    }

    #[test]
    fn a_heavy_hour_against_a_heavier_week_is_ordinary() {
        // The hour clears the floor but is a sliver of the week.
        let mut evidence = vec![record(NOW - 100, BIG)];
        for day in 1..7 {
            evidence.push(record(NOW - day * 24 * 60 * 60, BIG * 4));
        }
        assert_eq!(anomaly(&evidence, NOW), None);
    }

    #[test]
    fn a_quarter_of_the_week_in_one_hour_is_the_anomaly() {
        // One big burst now, modest history: the hour dominates the week.
        let evidence = vec![
            record(NOW - 100, BIG),
            record(NOW - 3 * 24 * 60 * 60, BIG / 2),
        ];
        let (hour, week) = anomaly(&evidence, NOW).expect("fires");
        assert!(hour > 0.0 && week >= hour);
    }
}
