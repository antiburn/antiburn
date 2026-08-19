// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared "retry after a cooldown, but never blank a good reading" state for
//! the direct-fetch sources.
//!
//! A fetch here is never free: it is at least one HTTPS round trip, and for
//! the Codex fallback it can mean spawning a whole process. The scheduler
//! asks every source to [`fetch`](super::super::LiveUsageSource::fetch) every
//! five minutes while the opt-in is on, so a source built on this type only
//! actually calls its own network code once [`SUCCESS_COOLDOWN`] or
//! [`FAILURE_COOLDOWN`] has elapsed since the last attempt. Every other tick
//! it reports the same outcome again — the last good snapshot, restamped for
//! how old it now is, and the last attempt's error if that attempt failed.
//! Never a blank while a good reading exists, and never one pretending to be
//! fresher than it is.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use time::OffsetDateTime;

use super::super::SourceOutcome;
use super::super::model::{Freshness, ProviderUsageError, ProviderUsageSnapshot};

/// How long to leave a source alone after a reading succeeded.
///
/// Ten minutes: against a five-hour window that is indistinguishable from
/// current, and it keeps every source's traffic to a small, predictable
/// trickle rather than one request every five-minute scheduler tick.
pub const SUCCESS_COOLDOWN: Duration = Duration::from_secs(600);

/// …and after one failed. Shorter than a success's cooldown, so a reader who
/// just fixed whatever was wrong — signed back in, restored their connection
/// — does not wait as long to find out.
pub const FAILURE_COOLDOWN: Duration = Duration::from_secs(300);

/// How old a fetched reading may be and still describe the present.
///
/// An hour, the same budget the file-based source this replaces used: past
/// that, a five-hour figure has had time to move without a reader hearing
/// about it.
pub const MAX_AGE: time::Duration = time::Duration::minutes(60);

/// Cooldown-gated retry state for one direct-fetch source.
#[derive(Default)]
pub struct Cooldown {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    snapshot: Option<ProviderUsageSnapshot>,
    error: Option<ProviderUsageError>,
    last_attempt: Option<(Instant, bool)>,
}

impl Cooldown {
    pub fn new() -> Cooldown {
        Cooldown::default()
    }

    /// Ask `fetch` for a fresh reading if the cooldown has elapsed; otherwise
    /// reuse the last outcome without calling it at all.
    ///
    /// `fetch` returns `Ok(None)` for a real, negative answer — "asked, and
    /// there is nothing to report" (an account type with no rate limit, say)
    /// — which this treats as a success for cooldown purposes and clears
    /// whatever snapshot came before it: that prior reading no longer
    /// describes this account, so keeping it on screen would be the one
    /// thing worse than reporting nothing.
    ///
    /// Returns the outcome this source should report this pass: the best
    /// snapshot it can currently vouch for — restamped against `now` — and
    /// the error from the most recent attempt, when that attempt is the one
    /// that failed. The two can coexist in the returned [`SourceOutcome`]: a
    /// failed refresh after an earlier success still has a snapshot worth
    /// showing, alongside the error that says it did not just get fresher.
    pub fn poll(
        &self,
        now: OffsetDateTime,
        fetch: impl FnOnce() -> Result<Option<ProviderUsageSnapshot>, ProviderUsageError>,
    ) -> SourceOutcome {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if off_cooldown(inner.last_attempt) {
            match fetch() {
                Ok(snapshot) => {
                    inner.snapshot = snapshot;
                    inner.error = None;
                    inner.last_attempt = Some((Instant::now(), true));
                }
                Err(error) => {
                    inner.error = Some(error);
                    inner.last_attempt = Some((Instant::now(), false));
                }
            }
        }
        let snapshot = inner
            .snapshot
            .clone()
            .map(|snapshot| restamp(snapshot, now));
        match (snapshot, inner.error) {
            (Some(snapshot), None) => SourceOutcome::found(vec![snapshot]),
            (Some(snapshot), Some(error)) => SourceOutcome {
                snapshots: vec![snapshot],
                error: Some(error),
            },
            (None, Some(error)) => SourceOutcome::failed(error),
            (None, None) => SourceOutcome::absent(),
        }
    }
}

fn off_cooldown(last: Option<(Instant, bool)>) -> bool {
    match last {
        None => true,
        Some((at, succeeded)) => {
            at.elapsed()
                >= if succeeded {
                    SUCCESS_COOLDOWN
                } else {
                    FAILURE_COOLDOWN
                }
        }
    }
}

/// Recompute freshness for `now` without touching when the figure was
/// actually observed — `observed_at` is the provider's own moment, and a
/// cooldown-skipped tick must not quietly advance it.
fn restamp(mut snapshot: ProviderUsageSnapshot, now: OffsetDateTime) -> ProviderUsageSnapshot {
    snapshot.source.freshness = Freshness::at(snapshot.observed_at, now, MAX_AGE);
    snapshot
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::provider_usage::live::model::{
        Confidence, UsageScope, UsageSource, UsageWindow, UsageWindowKind, WindowRole,
    };

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(seconds).expect("valid timestamp")
    }

    fn snapshot(observed_at: OffsetDateTime, percent: f64) -> ProviderUsageSnapshot {
        ProviderUsageSnapshot {
            provider: crate::provider_usage::providers::ANTHROPIC,
            account: None,
            plan: None,
            observed_at,
            source: UsageSource {
                id: "fixture",
                label: "fixture".into(),
                confidence: Confidence::High,
                freshness: Freshness::Fresh,
            },
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

    #[test]
    fn a_fresh_cooldown_calls_the_fetch_function() {
        let cooldown = Cooldown::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let outcome = cooldown.poll(at(1_000), move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Some(snapshot(at(1_000), 40.0)))
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(outcome.error.is_none());
        assert_eq!(outcome.snapshots[0].windows[0].used_percent, Some(40.0));
    }

    #[test]
    fn a_second_poll_inside_the_success_cooldown_never_calls_fetch_again() {
        let cooldown = Cooldown::new();
        let calls = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let counted = Arc::clone(&calls);
            cooldown.poll(at(1_000), move || {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(Some(snapshot(at(1_000), 40.0)))
            });
        }
        // Both calls landed on the same in-memory cooldown, so only the first
        // one actually reached the network.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_last_good_snapshot_survives_a_failed_retry() {
        let cooldown = Cooldown::new();
        cooldown.poll(at(1_000), || Ok(Some(snapshot(at(1_000), 40.0))));

        // Force the cooldown open by backdating the last attempt, the same
        // way `off_cooldown`'s own unit test below does — a real wait would
        // make this suite minutes long for no more coverage.
        {
            let mut inner = cooldown.inner.lock().unwrap();
            inner.last_attempt = Some((Instant::now() - FAILURE_COOLDOWN, false));
        }
        let outcome = cooldown.poll(at(2_000), || Err(ProviderUsageError::Unavailable));

        assert_eq!(outcome.error, Some(ProviderUsageError::Unavailable));
        // The stale snapshot is still there — a failed refresh must never
        // blank a reading that used to be good.
        assert_eq!(outcome.snapshots[0].windows[0].used_percent, Some(40.0));
    }

    #[test]
    fn a_snapshot_past_the_age_budget_restamps_as_stale_without_moving_its_observed_at() {
        let cooldown = Cooldown::new();
        cooldown.poll(at(1_000), || Ok(Some(snapshot(at(1_000), 40.0))));
        {
            let mut inner = cooldown.inner.lock().unwrap();
            // Keep the cooldown closed so this poll cannot trigger a refetch;
            // only the passage of `now` should change the answer.
            inner.last_attempt = Some((Instant::now(), true));
        }
        let far_future = at(1_000 + 3 * 60 * 60);
        let outcome = cooldown.poll(far_future, || Ok(Some(snapshot(far_future, 90.0))));

        let snapshot = outcome
            .snapshots
            .into_iter()
            .next()
            .expect("the cached reading is still there");
        assert_eq!(snapshot.source.freshness, Freshness::Stale);
        // Still the original reading, not the one the closure would have
        // returned had the cooldown actually let it run.
        assert_eq!(snapshot.windows[0].used_percent, Some(40.0));
    }

    #[test]
    fn a_real_negative_answer_clears_a_previous_snapshot_without_an_error() {
        let cooldown = Cooldown::new();
        cooldown.poll(at(1_000), || Ok(Some(snapshot(at(1_000), 40.0))));
        {
            let mut inner = cooldown.inner.lock().unwrap();
            inner.last_attempt = Some((Instant::now() - SUCCESS_COOLDOWN, true));
        }
        // "Asked, and there is nothing to report" — an account type with no
        // rate limit, say — must not leave the previous account's reading on
        // screen, and it is not an error either.
        let outcome = cooldown.poll(at(2_000), || Ok(None));

        assert!(outcome.snapshots.is_empty());
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn a_failure_cooldown_is_shorter_than_a_success_cooldown() {
        // A reader who just fixed whatever was wrong should not wait as long
        // as a routine, working refresh does.
        assert!(FAILURE_COOLDOWN < SUCCESS_COOLDOWN);
    }

    #[test]
    fn off_cooldown_reopens_once_its_own_budget_has_elapsed() {
        assert!(off_cooldown(None));
        assert!(!off_cooldown(Some((Instant::now(), true))));
        assert!(off_cooldown(Some((
            Instant::now() - SUCCESS_COOLDOWN,
            true
        ))));
        assert!(!off_cooldown(Some((
            Instant::now() - FAILURE_COOLDOWN,
            true
        ))));
        assert!(off_cooldown(Some((
            Instant::now() - FAILURE_COOLDOWN,
            false
        ))));
    }
}
