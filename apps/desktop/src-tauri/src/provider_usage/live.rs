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
//! `usage-milestone-engine`); the source that fills it is the per-feature
//! online opt-in, and until one is connected the engine simply never sees a
//! window.
//!
//! # Delivery semantics, in order of importance
//!
//! - **Priming.** The first time a window (identified by provider, account,
//!   window id, and reset epoch) is seen, every threshold already crossed is
//!   recorded silently. Connecting the source at 80% used must not fire 50
//!   and 75 — those crossings are old news.
//! - **Once per crossing.** A delivered threshold never fires again for the
//!   same window instance; delivering a threshold supersedes every lower one.
//! - **Re-arm on reset.** The reset epoch is part of the window identity, so
//!   a quota reset produces a fresh identity and the thresholds arm again.
//! - **Highest wins.** When several thresholds cross in one evaluation, the
//!   highest is delivered and the rest are folded into it.
//!
//! The ledger is in-memory on purpose. Priming makes a relaunch safe (already
//! crossed thresholds are re-primed, not re-fired); the price is that a
//! crossing which happens entirely while the app is closed is never
//! announced, which matches priming's own contract: the notification is for
//! the moment of crossing, not the state of being crossed.

use std::collections::BTreeMap;

use crate::store::Milestones;

/// The window classes milestones are tracked for. Mirrors the two milestone
/// preference rows: a short primary window and a weekly one.
// Constructed only by live-source implementations, of which this build ships
// none (deviations D-19). The allow, not the enum, is what the first source
// deletes.
#[allow(dead_code)]
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

/// A source of live usage snapshots.
///
/// Implementations call a provider endpoint with a credential the reader
/// supplied, under the D-023 network policy: per-feature opt-in, default off,
/// never a private-app endpoint. No implementation ships in this build — see
/// the deviations register — so the engine below runs against nothing until
/// one lands.
pub trait LiveUsageSource: Send + Sync {
    fn fetch(&self) -> Vec<LiveUsageSnapshot>;
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
    pub resets_at_epoch: i64,
}

/// Everything the notification needs to describe the crossings of one
/// provider account.
#[derive(Debug, Clone, PartialEq)]
pub struct MilestoneContent {
    pub provider: String,
    pub crossings: Vec<MilestoneCrossing>,
}

const THRESHOLDS: [u8; 3] = [50, 75, 90];

fn milestone_enabled(
    prefs_short: Milestones,
    prefs_weekly: Milestones,
    class: UsageWindowClass,
    threshold: u8,
) -> bool {
    let prefs = match class {
        UsageWindowClass::Short => prefs_short,
        UsageWindowClass::Weekly => prefs_weekly,
    };
    match threshold {
        50 => prefs.at50,
        75 => prefs.at75,
        90 => prefs.at90,
        _ => false,
    }
}

/// Evaluate one round of snapshots against the ledger.
///
/// Destructive by design: crossings selected here are recorded before the
/// caller attempts delivery, so the caller must only invoke this when it
/// intends to deliver. Returns at most one content — the account with the
/// highest new crossing — because two milestone notifications at once read
/// as a malfunction, and the monitor will pick the other account up on its
/// next pass.
pub fn milestone_content(
    ledger: &mut MilestoneLedger,
    snapshots: &[LiveUsageSnapshot],
    prefs_short: Milestones,
    prefs_weekly: Milestones,
) -> Option<MilestoneContent> {
    if !prefs_short.any() && !prefs_weekly.any() {
        return None;
    }

    let mut grouped: BTreeMap<String, Vec<(String, MilestoneCrossing)>> = BTreeMap::new();
    for snapshot in snapshots {
        if !snapshot.fresh {
            continue;
        }
        for window in &snapshot.windows {
            if !window.authoritative || !window.used_percent.is_finite() {
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
                for threshold in THRESHOLDS {
                    if window.used_percent >= f64::from(threshold) {
                        ledger
                            .delivered
                            .insert(format!("milestone:{base}:{threshold}"), "primed");
                    }
                }
                continue;
            }

            let crossed = THRESHOLDS
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
                grouped.entry(snapshot.provider.clone()).or_default().push((
                    format!("milestone:{base}:{threshold}"),
                    MilestoneCrossing {
                        window_label: window.label.clone(),
                        threshold,
                        used_percent: window.used_percent,
                        resets_at_epoch: window.resets_at_epoch,
                    },
                ));
            }
        }
    }

    let (provider, crossings) = grouped
        .into_iter()
        .max_by_key(|(_, crossings)| crossings.iter().map(|(_, c)| c.threshold).max())?;

    // Record before returning: a delivered threshold supersedes every lower
    // one on the same window, so a climb through 50 and 75 in one gap between
    // evaluations produces one notification, not a backlog.
    for (key, _) in &crossings {
        ledger.delivered.insert(key.clone(), "delivered");
        if let Some((base, threshold)) = key.rsplit_once(':')
            && let Ok(threshold) = threshold.parse::<u8>()
        {
            for lower in THRESHOLDS.into_iter().filter(|value| *value <= threshold) {
                ledger
                    .delivered
                    .entry(format!("{base}:{lower}"))
                    .or_insert("superseded");
            }
        }
    }

    Some(MilestoneContent {
        provider,
        crossings: crossings.into_iter().map(|(_, c)| c).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: &str, class: UsageWindowClass, used: f64, reset: i64) -> LiveUsageWindow {
        LiveUsageWindow {
            id: id.into(),
            class,
            label: match class {
                UsageWindowClass::Short => "5-hour limit".into(),
                UsageWindowClass::Weekly => "weekly limit".into(),
            },
            used_percent: used,
            resets_at_epoch: reset,
            authoritative: true,
        }
    }

    fn snapshot(used: f64, reset: i64) -> LiveUsageSnapshot {
        LiveUsageSnapshot {
            provider: "anthropic".into(),
            account: None,
            fresh: true,
            windows: vec![window("five_hour", UsageWindowClass::Short, used, reset)],
        }
    }

    fn all() -> Milestones {
        Milestones {
            at50: true,
            at75: true,
            at90: true,
        }
    }

    #[test]
    fn the_first_sight_of_a_window_primes_and_stays_silent() {
        let mut ledger = MilestoneLedger::default();
        // Connected at 80% used: 50 and 75 are old news.
        assert_eq!(
            milestone_content(&mut ledger, &[snapshot(80.0, 100)], all(), all()),
            None
        );
        // Still 80 on the next pass: nothing new crossed.
        assert_eq!(
            milestone_content(&mut ledger, &[snapshot(80.0, 100)], all(), all()),
            None
        );
        // 90 crosses live: that one fires.
        let content = milestone_content(&mut ledger, &[snapshot(91.0, 100)], all(), all()).unwrap();
        assert_eq!(content.crossings.len(), 1);
        assert_eq!(content.crossings[0].threshold, 90);
    }

    #[test]
    fn a_crossing_fires_once_and_supersedes_lower_thresholds() {
        let mut ledger = MilestoneLedger::default();
        assert!(milestone_content(&mut ledger, &[snapshot(10.0, 100)], all(), all()).is_none());
        // Climbed through 50 and 75 between evaluations: one notification, at
        // the highest crossing.
        let content = milestone_content(&mut ledger, &[snapshot(78.0, 100)], all(), all()).unwrap();
        assert_eq!(content.crossings[0].threshold, 75);
        // Neither 75 nor the superseded 50 fires again.
        assert!(milestone_content(&mut ledger, &[snapshot(79.0, 100)], all(), all()).is_none());
    }

    #[test]
    fn a_reset_epoch_change_rearms_the_window() {
        let mut ledger = MilestoneLedger::default();
        assert!(milestone_content(&mut ledger, &[snapshot(10.0, 100)], all(), all()).is_none());
        assert!(milestone_content(&mut ledger, &[snapshot(60.0, 100)], all(), all()).is_some());
        // New reset epoch = new window identity: primed fresh, silent at 10%.
        assert!(milestone_content(&mut ledger, &[snapshot(10.0, 200)], all(), all()).is_none());
        // And 50 can fire again for the new instance.
        assert!(milestone_content(&mut ledger, &[snapshot(55.0, 200)], all(), all()).is_some());
    }

    #[test]
    fn disabled_thresholds_and_unauthoritative_windows_stay_silent() {
        let mut ledger = MilestoneLedger::default();
        let none = Milestones {
            at50: false,
            at75: false,
            at90: false,
        };
        assert!(milestone_content(&mut ledger, &[snapshot(10.0, 100)], none, none).is_none());
        assert!(milestone_content(&mut ledger, &[snapshot(95.0, 100)], none, none).is_none());

        let mut ledger = MilestoneLedger::default();
        let mut stale = snapshot(10.0, 100);
        stale.fresh = false;
        assert!(milestone_content(&mut ledger, &[stale.clone()], all(), all()).is_none());
        stale.fresh = true;
        stale.windows[0].authoritative = false;
        assert!(milestone_content(&mut ledger, &[stale], all(), all()).is_none());
    }

    #[test]
    fn weekly_and_short_windows_follow_their_own_preference_rows() {
        let mut ledger = MilestoneLedger::default();
        let weekly_only = Milestones {
            at50: true,
            at75: true,
            at90: true,
        };
        let short_none = Milestones {
            at50: false,
            at75: false,
            at90: false,
        };
        let mut snap = snapshot(10.0, 100);
        snap.windows
            .push(window("weekly", UsageWindowClass::Weekly, 10.0, 500));
        assert!(milestone_content(&mut ledger, &[snap.clone()], short_none, weekly_only).is_none());

        snap.windows[0].used_percent = 60.0; // short window crosses, but its row is off
        snap.windows[1].used_percent = 60.0; // weekly crosses, and its row is on
        let content = milestone_content(&mut ledger, &[snap], short_none, weekly_only).unwrap();
        assert_eq!(content.crossings.len(), 1);
        assert_eq!(content.crossings[0].window_label, "weekly limit");
    }
}
