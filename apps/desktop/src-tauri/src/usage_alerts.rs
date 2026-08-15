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
//! something: the Usage surface shows a provider's own limits, read from what
//! an agent cached on this machine.
//!
//! Milestone notifications are gated on `live_usage_enabled` — the Settings →
//! Usage switch, default off — and that pairing is deliberate rather than
//! leftover. A milestone is a statement about a threshold being *crossed*, so
//! it needs readings that keep moving, and only the refresh source makes them
//! move: without it the offline reading sits still until the reader next uses
//! their agent, and a crossing would be announced whenever that happened to
//! be. So the one switch buys both, and its copy names both.

use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::provider_usage;
use crate::store::Store;

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

/// Spawn the monitor loop; the handle joins the shell's scheduler registry.
pub fn spawn_scheduler(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            run_pass(&app);
        }
    })
}

/// The registered live usage sources and the milestone engine's ledger.
///
/// Held as app state so both the milestone pass below and
/// [`crate::commands::get_live_usage`] read the same registry — one place
/// that knows what this build can prove.
pub struct LiveUsage {
    pub sources: Vec<Box<dyn provider_usage::live::LiveUsageSource>>,
    ledger: std::sync::Mutex<provider_usage::live::MilestoneLedger>,
}

impl LiveUsage {
    /// `workspace` is a directory under the app's own data directory, for the
    /// refresh source's private scratch space. Passed in rather than derived
    /// here so the shell keeps one answer to "where does this app write".
    pub fn new(workspace: std::path::PathBuf) -> LiveUsage {
        LiveUsage {
            sources: provider_usage::live::sources::registered(workspace),
            ledger: std::sync::Mutex::default(),
        }
    }
}

fn run_pass(app: &AppHandle) {
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
    // were off would be silently consumed.
    if !settings.live_usage_enabled
        || !crate::notifications::allowed(settings, crate::notifications::Kind::UsageMilestone)
    {
        return;
    }
    let Some(live) = app.try_state::<LiveUsage>() else {
        return;
    };
    let snapshots: Vec<provider_usage::live::milestones::LiveUsageSnapshot> =
        provider_usage::live::sources::collect(&live.sources, settings.live_usage_enabled)
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
    use crate::store::UsageEvidenceRecord;

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
