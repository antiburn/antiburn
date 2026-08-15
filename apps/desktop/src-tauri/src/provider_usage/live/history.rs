// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Readings kept so later ones have something to compare against.
//!
//! [`super::metrics`] needs a series; a source produces one reading. This
//! module is the bit in between, and it is deliberately the smallest thing
//! that can be.
//!
//! # Why a stamp, not a tick
//!
//! The offline source reads a file another application wrote. Polling it
//! every five minutes yields the *same* reading five minutes later — the
//! provider's figures only move when the agent refetches them. So a sample is
//! keyed on the moment the provider stated it, and a reading whose stamp we
//! already hold is discarded rather than appended.
//!
//! The consequence is worth stating plainly: history grows when the agent
//! runs, not when antiburn looks. A reader who has not used their agent for
//! an hour has one sample, and every forecast will honestly say there is not
//! enough history. That is better than a series of duplicated points, which
//! would compute a confident rate of exactly zero.
//!
//! # Where it lives
//!
//! One row in the app's own key-value table, through
//! [`Store::internal_value`](crate::store::Store::internal_value) — the same
//! seam the anomaly monitor uses for its last-fired stamp. Not a new file, not
//! a new table: this is a small bounded cache that a reader clearing their
//! index should lose along with everything else derived.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::metrics::UsageSample;
use super::model::{Freshness, ProviderUsageSnapshot};
use crate::store::Store;

/// Where the series live.
const KEY: &str = "internal:liveUsageHistory";

/// How far back samples are kept.
///
/// A week, because the longest window a provider reports is a week: history
/// older than the window it describes cannot inform anything about it.
const RETENTION_SECS: i64 = 7 * 24 * 60 * 60;

/// A ceiling on the whole store, whatever the retention says.
///
/// Guards the pathological case — many providers, many windows, an agent
/// refetching constantly — from turning a convenience cache into an unbounded
/// one. Oldest go first.
const MAX_SAMPLES: usize = 2_000;

/// One stored reading. Field names are short because there may be a couple of
/// thousand of them in one JSON value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Stored {
    /// Unix seconds the provider stated this.
    at: i64,
    /// Consumed capacity, `0..=100`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pct: Option<f64>,
    /// Whether the reading described the present when it was taken.
    fresh: bool,
}

/// Every series, keyed by [`window_key`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct History {
    #[serde(flatten)]
    series: BTreeMap<String, Vec<Stored>>,
}

impl History {
    /// The samples for one window, oldest first.
    pub fn samples(&self, key: &str) -> Vec<UsageSample> {
        self.series
            .get(key)
            .map(|stored| {
                stored
                    .iter()
                    .filter_map(|sample| {
                        Some(UsageSample {
                            observed_at: OffsetDateTime::from_unix_timestamp(sample.at).ok()?,
                            used_percent: sample.pct,
                            freshness: if sample.fresh {
                                Freshness::Fresh
                            } else {
                                Freshness::Stale
                            },
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The identity a series is kept under.
///
/// Provider, account, and window, so two accounts at one provider never share
/// a series and the five-hour window never contaminates the weekly one. The
/// account is the provider's own local id — nothing is uploaded, so there is
/// nothing to hash it for — and it never leaves this process.
pub fn window_key(snapshot: &ProviderUsageSnapshot, window_id: &str) -> String {
    format!(
        "{}:{}:{window_id}",
        snapshot.provider,
        snapshot.account.as_deref().unwrap_or_default()
    )
}

/// Read the stored series.
///
/// An unreadable or unparseable value reads as no history rather than as an
/// error: this is a cache, and the worst case of losing it is that every
/// forecast honestly reports sparse history until it refills.
pub fn load(store: &Store) -> History {
    store
        .internal_value(KEY)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Append anything new in `snapshots`, prune, and write back.
///
/// Returns the history *including* the new readings, so a caller that is
/// about to compute a forecast does not have to read it back.
pub fn record(store: &Store, snapshots: &[ProviderUsageSnapshot], now: i64) -> History {
    let mut history = load(store);
    let mut changed = false;

    for snapshot in snapshots {
        let at = snapshot.observed_at.unix_timestamp();
        // A reading older than retention is not worth storing even once.
        if now.saturating_sub(at) > RETENTION_SECS {
            continue;
        }
        for window in &snapshot.windows {
            let series = history
                .series
                .entry(window_key(snapshot, &window.id))
                .or_default();
            // The same stated moment is the same reading, however many times
            // we looked at the file. Appending it would compute a confident
            // rate of zero out of one observation.
            if series.iter().any(|sample| sample.at == at) {
                continue;
            }
            series.push(Stored {
                at,
                pct: window.used_percent,
                fresh: snapshot.source.freshness == Freshness::Fresh,
            });
            series.sort_by_key(|sample| sample.at);
            changed = true;
        }
    }

    if prune(&mut history, now) {
        changed = true;
    }
    if changed {
        // A failed write is not worth reporting: the next pass tries again,
        // and the only cost in the meantime is a forecast that says it does
        // not have enough history, which is a state the surfaces already have.
        let _ = serde_json::to_string(&history).map(|raw| store.set_internal_value(KEY, &raw));
    }
    history
}

/// Drop what is too old, then what is over the ceiling. Returns whether
/// anything went.
fn prune(history: &mut History, now: i64) -> bool {
    let horizon = now - RETENTION_SECS;
    let before: usize = history.series.values().map(Vec::len).sum();

    for series in history.series.values_mut() {
        series.retain(|sample| sample.at >= horizon);
    }
    history.series.retain(|_, series| !series.is_empty());

    let mut total: usize = history.series.values().map(Vec::len).sum();
    while total > MAX_SAMPLES {
        // Oldest first, across every series: the ceiling is a property of the
        // whole cache, so one busy window must not evict a quiet window's
        // entire (and more recent) history.
        let Some(oldest) = history
            .series
            .iter()
            .filter_map(|(key, series)| series.first().map(|sample| (sample.at, key.clone())))
            .min()
        else {
            break;
        };
        if let Some(series) = history.series.get_mut(&oldest.1) {
            series.remove(0);
            if series.is_empty() {
                history.series.remove(&oldest.1);
            }
        }
        total -= 1;
    }

    history.series.values().map(Vec::len).sum::<usize>() != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_usage::live::model::{
        Confidence, SupportTier, UsageScope, UsageSource, UsageWindow, UsageWindowKind, WindowRole,
    };

    const NOW: i64 = 1_800_000_000;

    fn snapshot(observed: i64, percent: f64) -> ProviderUsageSnapshot {
        ProviderUsageSnapshot {
            provider: crate::provider_usage::providers::ANTHROPIC,
            account: Some("account-a".into()),
            plan: None,
            observed_at: OffsetDateTime::from_unix_timestamp(observed).unwrap(),
            source: UsageSource {
                id: "fixture",
                label: "fixture".into(),
                confidence: Confidence::High,
                freshness: Freshness::Fresh,
            },
            support: SupportTier::Live,
            windows: vec![UsageWindow {
                id: "five-hour".into(),
                role: WindowRole::PrimaryShort,
                kind: UsageWindowKind::Rolling,
                scope: UsageScope::Account,
                used_percent: Some(percent),
                starts_at: None,
                resets_at: None,
                authoritative: true,
            }],
            supplemental: None,
        }
    }

    fn store() -> Store {
        Store::open_in_memory(std::path::Path::new("/tmp/antiburn-test-state"))
            .expect("in-memory store")
    }

    #[test]
    fn a_reading_already_held_is_not_appended_again() {
        // The file only changes when the agent refetches, so polling it must
        // not manufacture a series out of one observation.
        let store = store();
        let key = window_key(&snapshot(NOW - 60, 40.0), "five-hour");

        record(&store, &[snapshot(NOW - 60, 40.0)], NOW);
        record(&store, &[snapshot(NOW - 60, 40.0)], NOW);
        let history = record(&store, &[snapshot(NOW - 60, 40.0)], NOW);

        assert_eq!(history.samples(&key).len(), 1);
    }

    #[test]
    fn a_new_stated_moment_extends_the_series_in_order() {
        let store = store();
        let key = window_key(&snapshot(NOW, 40.0), "five-hour");

        // Recorded out of order on purpose.
        record(&store, &[snapshot(NOW - 60, 45.0)], NOW);
        record(&store, &[snapshot(NOW - 3_600, 40.0)], NOW);
        let history = record(&store, &[snapshot(NOW - 30, 50.0)], NOW);

        let samples = history.samples(&key);
        assert_eq!(samples.len(), 3);
        assert_eq!(
            samples
                .iter()
                .map(|sample| sample.used_percent)
                .collect::<Vec<_>>(),
            [Some(40.0), Some(45.0), Some(50.0)]
        );
    }

    #[test]
    fn the_series_survives_a_reload_through_the_store() {
        let store = store();
        let key = window_key(&snapshot(NOW, 40.0), "five-hour");
        record(&store, &[snapshot(NOW - 60, 40.0)], NOW);
        assert_eq!(load(&store).samples(&key).len(), 1);
    }

    #[test]
    fn readings_past_the_horizon_are_dropped_and_never_stored() {
        let store = store();
        let key = window_key(&snapshot(NOW, 40.0), "five-hour");

        // Too old to store at all.
        record(&store, &[snapshot(NOW - RETENTION_SECS - 1, 10.0)], NOW);
        assert!(load(&store).samples(&key).is_empty());

        // Stored, then aged out by a later pass.
        record(&store, &[snapshot(NOW - 60, 40.0)], NOW);
        assert_eq!(load(&store).samples(&key).len(), 1);
        let later = record(&store, &[], NOW + RETENTION_SECS + 120);
        assert!(later.samples(&key).is_empty());
    }

    #[test]
    fn two_accounts_at_one_provider_keep_separate_series() {
        let mut other = snapshot(NOW - 60, 90.0);
        other.account = Some("account-b".into());
        let store = store();
        let history = record(&store, &[snapshot(NOW - 60, 40.0), other.clone()], NOW);

        assert_eq!(
            history.samples(&window_key(&snapshot(NOW, 0.0), "five-hour"))[0].used_percent,
            Some(40.0)
        );
        assert_eq!(
            history.samples(&window_key(&other, "five-hour"))[0].used_percent,
            Some(90.0)
        );
    }

    #[test]
    fn the_ceiling_evicts_the_oldest_across_every_series_not_the_busiest() {
        let mut history = History::default();
        // One long-running series, and one quiet series whose single sample is
        // newer than everything in the first.
        history.series.insert(
            "busy".into(),
            (0..MAX_SAMPLES + 10)
                .map(|index| Stored {
                    at: NOW - 100_000 + index as i64,
                    pct: Some(1.0),
                    fresh: true,
                })
                .collect(),
        );
        history.series.insert(
            "quiet".into(),
            vec![Stored {
                at: NOW - 60,
                pct: Some(2.0),
                fresh: true,
            }],
        );

        prune(&mut history, NOW);

        assert_eq!(
            history.series.values().map(Vec::len).sum::<usize>(),
            MAX_SAMPLES
        );
        // The quiet window's recent reading survived.
        assert_eq!(history.series.get("quiet").map(Vec::len), Some(1));
    }
}
