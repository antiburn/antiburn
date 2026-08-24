// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Live usage windows and the milestone-selection engine.
//!
//! Everything local in this app estimates what was *spent*; a milestone is a
//! statement about what *remains*, which only a provider can answer. This
//! module holds the shape of that answer ([`LiveUsageSnapshot`]) and the
//! engine that decides when a crossing deserves a notification. The engine is
//! adapted from the private app under D-023 (allowlist rule
//! `usage-milestone-engine`).
//!
//! What fills it is [`super::sources`], narrowed by
//! `usage_alerts::milestone_snapshot` down to the two window classes and the
//! quota and elapsed-window percentages this engine deals in. That narrowing
//! is deliberate: a window with no stated reset never arrives here at all,
//! because the reset epoch is half of a window's identity and a crossing that
//! cannot re-arm would fire once and then be silent for good.
//!
//! # Delivery semantics, in order of importance
//!
//! - **Priming.** The first time a window (identified by provider, account,
//!   window id, and reset epoch) is seen, every threshold already crossed is
//!   recorded silently. Connecting the source at 80% used must not replay old
//!   crossings.
//! - **Once per crossing.** A delivered threshold never fires again for the
//!   same window instance; delivering a threshold supersedes every lower one.
//!   A disabled threshold is also recorded when crossed, so selecting it later
//!   does not replay an old event.
//! - **Re-arm on reset.** The reset epoch is part of the window identity, so
//!   a quota reset produces a fresh identity and the thresholds arm again.
//! - **One limit at a time.** A jump over several thresholds on one limit
//!   reports the highest. If several limits cross together, the limit furthest
//!   ahead of elapsed time reports first and the others wait for later passes.
//!
//! The ledger is in-memory on purpose. Priming makes a relaunch safe (already
//! crossed thresholds are re-primed, not re-fired); the price is that a
//! crossing which happens entirely while the app is closed is never
//! announced, which matches priming's own contract: the notification is for
//! the moment of crossing, not the state of being crossed.

use std::collections::BTreeMap;

use crate::store::{MILESTONE_OPTIONS, Milestones};

/// The window classes milestones are tracked for. Mirrors the two milestone
/// preference rows: a short primary window and a weekly one.
///
/// Two, not more. A per-model weekly limit is a weekly limit as far as a
/// notification is concerned — inventing a third preference row for it would
/// ask the reader a question they have no way to answer differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageWindowClass {
    Short,
    Weekly,
}

/// One provider-reported usage window.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveUsageWindow {
    /// Stable id within the snapshot (e.g. `"five_hour"`).
    pub id: String,
    pub class: UsageWindowClass,
    /// Human label for the notification copy (e.g. `"5-hour limit"`).
    pub label: String,
    /// Percent of the window's allowance already used, `0.0..`.
    pub used_percent: f64,
    /// Percent of the current window that had passed at this observation.
    pub elapsed_percent: f64,
    /// When this window's allowance resets, unix seconds. Part of the window
    /// identity: a new reset epoch re-arms every threshold.
    pub resets_at_epoch: i64,
    /// False when the provider's answer was derived rather than stated;
    /// milestones only fire on stated numbers.
    pub authoritative: bool,
}

/// One provider account's live usage, as fetched by an opted-in source.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveUsageSnapshot {
    /// Canonical provider id (`anthropic`, `openai`, …).
    pub provider: String,
    /// Opaque account discriminator when the source knows one, so two
    /// accounts at one provider track independently.
    pub account: Option<String>,
    /// False when the fetch is stale; stale numbers neither prime nor fire.
    pub fresh: bool,
    pub windows: Vec<LiveUsageWindow>,
}

/// What the engine remembers between evaluations. In-memory by design; see
/// the module doc for why a relaunch is safe.
#[derive(Debug, Default)]
pub struct MilestoneLedger {
    /// Window identities that have been seen at least once.
    primed_windows: BTreeMap<String, ()>,
    /// `"milestone:<base>:<threshold>"` → why it will not fire again
    /// (`"primed"`, `"delivered"`, `"superseded"`).
    delivered: BTreeMap<String, &'static str>,
}

/// One threshold crossing folded into a notification.
#[derive(Debug, Clone, PartialEq)]
pub struct MilestoneCrossing {
    pub window_label: String,
    pub threshold: u8,
    pub used_percent: f64,
    pub elapsed_percent: f64,
    pub resets_at_epoch: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MilestoneContent {
    pub provider: String,
    pub crossing: MilestoneCrossing,
}

fn milestone_enabled(
    prefs_short: &Milestones,
    prefs_weekly: &Milestones,
    class: UsageWindowClass,
    threshold: u8,
) -> bool {
    let prefs = match class {
        UsageWindowClass::Short => prefs_short,
        UsageWindowClass::Weekly => prefs_weekly,
    };
    prefs.contains(threshold)
}

/// Evaluate one round of snapshots against the ledger.
///
/// Destructive by design: crossings selected here are recorded before the
/// caller attempts delivery, so the caller must only invoke this when it
/// intends to deliver. Returns the new crossing furthest ahead of elapsed
/// time. Other limits remain pending for later passes, so several crossings
/// do not arrive as a burst.
pub fn milestone_content(
    ledger: &mut MilestoneLedger,
    snapshots: &[LiveUsageSnapshot],
    prefs_short: &Milestones,
    prefs_weekly: &Milestones,
) -> Option<MilestoneContent> {
    if !prefs_short.any() && !prefs_weekly.any() {
        return None;
    }

    let mut candidates = Vec::new();
    for snapshot in snapshots {
        if !snapshot.fresh {
            continue;
        }
        for window in &snapshot.windows {
            if !window.authoritative
                || !window.used_percent.is_finite()
                || !window.elapsed_percent.is_finite()
            {
                continue;
            }
            let account = snapshot.account.clone().unwrap_or_default();
            let base = format!(
                "{}:{account}:{}:{}",
                snapshot.provider, window.id, window.resets_at_epoch
            );

            // First sight of this window instance: record what is already
            // crossed, announce nothing.
            if !ledger.primed_windows.contains_key(&base) {
                ledger.primed_windows.insert(base.clone(), ());
                for threshold in MILESTONE_OPTIONS {
                    if window.used_percent >= f64::from(threshold) {
                        ledger
                            .delivered
                            .insert(format!("milestone:{base}:{threshold}"), "primed");
                    }
                }
                continue;
            }

            for threshold in MILESTONE_OPTIONS.into_iter().filter(|threshold| {
                !milestone_enabled(prefs_short, prefs_weekly, window.class, *threshold)
                    && window.used_percent >= f64::from(*threshold)
            }) {
                ledger
                    .delivered
                    .entry(format!("milestone:{base}:{threshold}"))
                    .or_insert("disabled");
            }

            let crossed = MILESTONE_OPTIONS
                .into_iter()
                .filter(|threshold| {
                    milestone_enabled(prefs_short, prefs_weekly, window.class, *threshold)
                })
                .filter(|threshold| window.used_percent >= f64::from(*threshold))
                .filter(|threshold| {
                    !ledger
                        .delivered
                        .contains_key(&format!("milestone:{base}:{threshold}"))
                })
                .max();

            if let Some(threshold) = crossed {
                candidates.push((
                    format!("milestone:{base}:{threshold}"),
                    snapshot.provider.clone(),
                    MilestoneCrossing {
                        window_label: window.label.clone(),
                        threshold,
                        used_percent: window.used_percent,
                        elapsed_percent: window.elapsed_percent,
                        resets_at_epoch: window.resets_at_epoch,
                    },
                ));
            }
        }
    }

    let (key, provider, crossing) = candidates.into_iter().max_by(|left, right| {
        let left_lead = left.2.used_percent - left.2.elapsed_percent;
        let right_lead = right.2.used_percent - right.2.elapsed_percent;
        left_lead
            .total_cmp(&right_lead)
            .then_with(|| left.2.threshold.cmp(&right.2.threshold))
    })?;

    ledger.delivered.insert(key.clone(), "delivered");
    if let Some((base, threshold)) = key.rsplit_once(':')
        && let Ok(threshold) = threshold.parse::<u8>()
    {
        for lower in MILESTONE_OPTIONS
            .into_iter()
            .filter(|value| *value <= threshold)
        {
            ledger
                .delivered
                .entry(format!("{base}:{lower}"))
                .or_insert("superseded");
        }
    }

    Some(MilestoneContent { provider, crossing })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(
        id: &str,
        class: UsageWindowClass,
        used: f64,
        elapsed: f64,
        reset: i64,
    ) -> LiveUsageWindow {
        LiveUsageWindow {
            id: id.into(),
            class,
            label: match class {
                UsageWindowClass::Short => "5-hour limit".into(),
                UsageWindowClass::Weekly => "weekly limit".into(),
            },
            used_percent: used,
            elapsed_percent: elapsed,
            resets_at_epoch: reset,
            authoritative: true,
        }
    }

    fn snapshot(used: f64, elapsed: f64, reset: i64) -> LiveUsageSnapshot {
        LiveUsageSnapshot {
            provider: "anthropic".into(),
            account: None,
            fresh: true,
            windows: vec![window(
                "five_hour",
                UsageWindowClass::Short,
                used,
                elapsed,
                reset,
            )],
        }
    }

    #[test]
    fn the_first_sight_of_a_window_primes_and_stays_silent() {
        let mut ledger = MilestoneLedger::default();
        assert_eq!(
            milestone_content(
                &mut ledger,
                &[snapshot(80.0, 40.0, 100)],
                &Milestones::all(),
                &Milestones::all(),
            ),
            None
        );
        assert_eq!(
            milestone_content(
                &mut ledger,
                &[snapshot(80.0, 41.0, 100)],
                &Milestones::all(),
                &Milestones::all(),
            ),
            None
        );
        let content = milestone_content(
            &mut ledger,
            &[snapshot(91.0, 45.0, 100)],
            &Milestones::all(),
            &Milestones::all(),
        )
        .unwrap();
        assert_eq!(content.crossing.threshold, 90);
    }

    #[test]
    fn a_crossing_fires_once_and_supersedes_lower_thresholds() {
        let mut ledger = MilestoneLedger::default();
        assert!(
            milestone_content(
                &mut ledger,
                &[snapshot(10.0, 10.0, 100)],
                &Milestones::all(),
                &Milestones::all(),
            )
            .is_none()
        );
        let content = milestone_content(
            &mut ledger,
            &[snapshot(78.0, 40.0, 100)],
            &Milestones::all(),
            &Milestones::all(),
        )
        .unwrap();
        assert_eq!(content.crossing.threshold, 75);
        assert!(
            milestone_content(
                &mut ledger,
                &[snapshot(79.0, 41.0, 100)],
                &Milestones::all(),
                &Milestones::all(),
            )
            .is_none()
        );
    }

    #[test]
    fn a_reset_epoch_change_rearms_the_window() {
        let mut ledger = MilestoneLedger::default();
        let prefs = Milestones::selected([50]);
        assert!(
            milestone_content(&mut ledger, &[snapshot(10.0, 10.0, 100)], &prefs, &prefs,).is_none()
        );
        assert!(
            milestone_content(&mut ledger, &[snapshot(60.0, 20.0, 100)], &prefs, &prefs,).is_some()
        );
        assert!(
            milestone_content(&mut ledger, &[snapshot(10.0, 10.0, 200)], &prefs, &prefs,).is_none()
        );
        assert!(
            milestone_content(&mut ledger, &[snapshot(55.0, 20.0, 200)], &prefs, &prefs,).is_some()
        );
    }

    #[test]
    fn disabled_thresholds_and_unauthoritative_windows_stay_silent() {
        let mut ledger = MilestoneLedger::default();
        assert!(
            milestone_content(
                &mut ledger,
                &[snapshot(10.0, 10.0, 100)],
                &Milestones::none(),
                &Milestones::none(),
            )
            .is_none()
        );

        let mut ledger = MilestoneLedger::default();
        let mut stale = snapshot(10.0, 10.0, 100);
        stale.fresh = false;
        assert!(
            milestone_content(
                &mut ledger,
                &[stale.clone()],
                &Milestones::all(),
                &Milestones::all(),
            )
            .is_none()
        );
        stale.fresh = true;
        stale.windows[0].authoritative = false;
        assert!(
            milestone_content(
                &mut ledger,
                &[stale],
                &Milestones::all(),
                &Milestones::all(),
            )
            .is_none()
        );
    }

    #[test]
    fn weekly_and_short_windows_follow_their_own_preference_rows() {
        let mut ledger = MilestoneLedger::default();
        let weekly = Milestones::selected([50]);
        let mut snap = snapshot(10.0, 10.0, 100);
        snap.windows
            .push(window("weekly", UsageWindowClass::Weekly, 10.0, 10.0, 500));
        assert!(
            milestone_content(&mut ledger, &[snap.clone()], &Milestones::none(), &weekly).is_none()
        );

        snap.windows[0].used_percent = 60.0;
        snap.windows[1].used_percent = 60.0;
        let content =
            milestone_content(&mut ledger, &[snap], &Milestones::none(), &weekly).unwrap();
        assert_eq!(content.crossing.window_label, "weekly limit");
    }

    #[test]
    fn five_percent_steps_can_be_selected_individually() {
        let mut ledger = MilestoneLedger::default();
        let fifteen = Milestones::selected([15]);
        assert!(
            milestone_content(
                &mut ledger,
                &[snapshot(10.0, 10.0, 100)],
                &fifteen,
                &Milestones::none(),
            )
            .is_none()
        );
        let content = milestone_content(
            &mut ledger,
            &[snapshot(16.0, 12.0, 100)],
            &fifteen,
            &Milestones::none(),
        )
        .unwrap();
        assert_eq!(content.crossing.threshold, 15);
    }

    #[test]
    fn selecting_a_threshold_does_not_replay_a_crossing_from_while_it_was_off() {
        let mut ledger = MilestoneLedger::default();
        let weekly = Milestones::selected([50]);
        assert!(
            milestone_content(
                &mut ledger,
                &[snapshot(10.0, 10.0, 100)],
                &Milestones::none(),
                &weekly,
            )
            .is_none()
        );
        assert!(
            milestone_content(
                &mut ledger,
                &[snapshot(16.0, 12.0, 100)],
                &Milestones::none(),
                &weekly,
            )
            .is_none()
        );
        assert!(
            milestone_content(
                &mut ledger,
                &[snapshot(16.0, 12.0, 100)],
                &Milestones::selected([15]),
                &weekly,
            )
            .is_none()
        );
    }

    #[test]
    fn the_default_selects_ten_percent_steps_only() {
        let defaults = Milestones::default();
        for threshold in MILESTONE_OPTIONS {
            assert_eq!(defaults.contains(threshold), threshold % 10 == 0);
        }
    }

    #[test]
    fn the_window_furthest_ahead_of_time_notifies_first() {
        let mut ledger = MilestoneLedger::default();
        let mut snap = snapshot(10.0, 10.0, 100);
        snap.windows
            .push(window("weekly", UsageWindowClass::Weekly, 10.0, 10.0, 500));
        assert!(
            milestone_content(
                &mut ledger,
                &[snap.clone()],
                &Milestones::default(),
                &Milestones::default(),
            )
            .is_none()
        );

        snap.windows[0].used_percent = 20.0;
        snap.windows[0].elapsed_percent = 40.0;
        snap.windows[1].used_percent = 20.0;
        snap.windows[1].elapsed_percent = 5.0;
        let first = milestone_content(
            &mut ledger,
            &[snap.clone()],
            &Milestones::default(),
            &Milestones::default(),
        )
        .unwrap();
        assert_eq!(first.crossing.window_label, "weekly limit");

        let second = milestone_content(
            &mut ledger,
            &[snap],
            &Milestones::default(),
            &Milestones::default(),
        )
        .unwrap();
        assert_eq!(second.crossing.window_label, "5-hour limit");
    }
}
