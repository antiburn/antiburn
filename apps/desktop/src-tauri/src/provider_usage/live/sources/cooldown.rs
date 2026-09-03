//! Shared "retry after a cooldown, but never blank a good reading" state for
//! the direct-fetch sources.
//!
//! A fetch here is never free: it is at least one HTTPS round trip, and for
//! the Codex fallback it can mean spawning a whole process. Two callers ask
//! every source to [`fetch`](super::super::LiveUsageSource::fetch) while the
//! opt-in is on: the background monitor, every five minutes, and
//! [`crate::commands::refresh_live_usage`], every time the popover — while it is
//! visible — polls for a reading. Those two callers do not want the same
//! thing. The monitor is not shown to anyone and can accept a five-minute-old
//! figure; the popover is on screen right now, being polled
//! *because* someone is looking at it, and a fixed ten-minute cooldown would
//! mean up to ten minutes of that polling doing nothing.
//!
//! So the cooldown is not fixed. Each call to [`Cooldown::poll`] states its
//! own `max_age` — how old a successful reading that particular caller is
//! willing to accept — and a source built on this type only actually calls
//! its own network code once `max_age` (or the failure cooldown derived from
//! it; see [`MIN_FAILURE_COOLDOWN`] and [`FAILURE_COOLDOWN`]) has elapsed
//! since the last attempt. Every other call reports the same outcome again —
//! the last good snapshot, restamped for how old it now is, and the last
//! attempt's error if that attempt failed. Never a blank while a good
//! reading exists, and never one pretending to be fresher than it is.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use time::OffsetDateTime;

use super::super::SourceOutcome;
use super::super::model::{Freshness, ProviderUsageError, ProviderUsageSnapshot};

/// The floor under a caller-requested failure cooldown.
///
/// A caller may ask for a `max_age` far below a minute — the popover, once
/// shown, does — but a failure is not a success arriving late, and retrying
/// a broken endpoint on every visible tick teaches nobody anything and
/// answers nobody sooner. A minute is short enough that a reader who just
/// fixed whatever was wrong finds out quickly, and long enough that an open
/// panel does not turn one bad response into a request storm.
const MIN_FAILURE_COOLDOWN: Duration = Duration::from_secs(60);

/// The ceiling on a caller-requested failure cooldown — what a caller with no
/// particular urgency, asking for a `max_age` well past this, actually gets.
/// Matches the fixed cooldown this type used to apply to every failure
/// unconditionally; it is now a ceiling rather than the whole rule because a
/// caller who wants fresher successes should not be made to wait as long for
/// a retry after a failure either.
pub const FAILURE_COOLDOWN: Duration = Duration::from_secs(300);

/// How old a fetched reading may be and still describe the present.
///
/// An hour, the same budget the file-based source this replaces used: past
/// that, a five-hour figure has had time to move without a reader hearing
/// about it. This is unrelated to a caller's `max_age` above — it governs
/// when a reading already on screen is marked stale, not when the next fetch
/// is allowed to run.
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
    /// `max_age` is this call's own statement of how old a successful
    /// reading it is willing to accept — the popover asks for something close
    /// to its own polling interval, the background monitor asks for
    /// something close to its own tick, and both share the same cached state
    /// underneath. A failed attempt's retry is derived from `max_age` too,
    /// clamped to [`MIN_FAILURE_COOLDOWN`, [`FAILURE_COOLDOWN`]] — see
    /// [`off_cooldown`].
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
        max_age: Duration,
        fetch: impl FnOnce() -> Result<Option<ProviderUsageSnapshot>, ProviderUsageError>,
    ) -> SourceOutcome {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if off_cooldown(inner.last_attempt, max_age) {
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

/// Whether the last attempt is old enough that `poll` should try again.
///
/// A success is measured against `max_age` exactly as the caller stated it —
/// that is the whole point of threading it through. A failure is measured
/// against `max_age` too, but clamped: never less than
/// [`MIN_FAILURE_COOLDOWN`], so an eager caller cannot turn a broken endpoint
/// into a request on every tick, and never more than [`FAILURE_COOLDOWN`], so
/// a caller with no urgency still finds out about a fixed problem within five
/// minutes.
fn off_cooldown(last: Option<(Instant, bool)>, max_age: Duration) -> bool {
    match last {
        None => true,
        Some((at, succeeded)) => {
            at.elapsed()
                >= if succeeded {
                    max_age
                } else {
                    max_age.clamp(MIN_FAILURE_COOLDOWN, FAILURE_COOLDOWN)
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

    /// A background-caller-shaped `max_age`, for tests that only care that
    /// *some* cooldown applies and not about its exact length.
    const DEFAULT_MAX_AGE: Duration = Duration::from_secs(600);

    /// A popover-shaped `max_age` — well under a minute, the way an open
    /// panel polling every tick asks for something close to its own
    /// interval.
    const SHORT_MAX_AGE: Duration = Duration::from_secs(50);

    fn snapshot(observed_at: OffsetDateTime, percent: f64) -> ProviderUsageSnapshot {
        ProviderUsageSnapshot {
            provider: crate::provider_usage::providers::ANTHROPIC,
            account: None,
            plan: None,
            plan_tier: None,
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
            reset_credits: None,
        }
    }

    #[test]
    fn a_fresh_cooldown_calls_the_fetch_function() {
        let cooldown = Cooldown::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let outcome = cooldown.poll(at(1_000), DEFAULT_MAX_AGE, move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Some(snapshot(at(1_000), 40.0)))
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(outcome.error.is_none());
        assert_eq!(outcome.snapshots[0].windows[0].used_percent, Some(40.0));
    }

    #[test]
    fn a_second_poll_inside_the_requested_max_age_never_calls_fetch_again() {
        let cooldown = Cooldown::new();
        let calls = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let counted = Arc::clone(&calls);
            cooldown.poll(at(1_000), DEFAULT_MAX_AGE, move || {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(Some(snapshot(at(1_000), 40.0)))
            });
        }
        // Both calls landed on the same in-memory cooldown, so only the first
        // one actually reached the network.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_caller_asking_for_a_shorter_max_age_refetches_where_a_fixed_cooldown_would_have_cached() {
        // This is the popover's whole reason for threading `max_age` through:
        // against the old fixed ten-minute success cooldown, a poll seconds
        // later would have been served the cache. A caller stating a shorter
        // `max_age` gets a real refetch instead.
        let cooldown = Cooldown::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        cooldown.poll(at(1_000), SHORT_MAX_AGE, move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Some(snapshot(at(1_000), 40.0)))
        });

        // Backdate past `SHORT_MAX_AGE` but nowhere near the old fixed
        // `SUCCESS_COOLDOWN` (ten minutes) this replaces.
        {
            let mut inner = cooldown.inner.lock().unwrap();
            inner.last_attempt = Some((
                Instant::now() - SHORT_MAX_AGE - Duration::from_secs(1),
                true,
            ));
        }
        let counted = Arc::clone(&calls);
        cooldown.poll(at(1_000), SHORT_MAX_AGE, move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Some(snapshot(at(1_000), 90.0)))
        });

        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_last_good_snapshot_survives_a_failed_retry() {
        let cooldown = Cooldown::new();
        cooldown.poll(at(1_000), DEFAULT_MAX_AGE, || {
            Ok(Some(snapshot(at(1_000), 40.0)))
        });

        // Force the cooldown open by backdating the last attempt, the same
        // way `off_cooldown`'s own unit test below does — a real wait would
        // make this suite minutes long for no more coverage.
        {
            let mut inner = cooldown.inner.lock().unwrap();
            inner.last_attempt = Some((Instant::now() - FAILURE_COOLDOWN, false));
        }
        let outcome = cooldown.poll(at(2_000), DEFAULT_MAX_AGE, || {
            Err(ProviderUsageError::Unavailable)
        });

        assert_eq!(outcome.error, Some(ProviderUsageError::Unavailable));
        assert_eq!(outcome.snapshots[0].windows[0].used_percent, Some(40.0));
    }

    #[test]
    fn a_snapshot_past_the_age_budget_restamps_as_stale_without_moving_its_observed_at() {
        let cooldown = Cooldown::new();
        cooldown.poll(at(1_000), DEFAULT_MAX_AGE, || {
            Ok(Some(snapshot(at(1_000), 40.0)))
        });
        {
            let mut inner = cooldown.inner.lock().unwrap();
            // Keep the cooldown closed so this poll cannot trigger a refetch;
            // only the passage of `now` should change the answer.
            inner.last_attempt = Some((Instant::now(), true));
        }
        let far_future = at(1_000 + 3 * 60 * 60);
        let outcome = cooldown.poll(far_future, DEFAULT_MAX_AGE, || {
            Ok(Some(snapshot(far_future, 90.0)))
        });

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
        cooldown.poll(at(1_000), DEFAULT_MAX_AGE, || {
            Ok(Some(snapshot(at(1_000), 40.0)))
        });
        {
            let mut inner = cooldown.inner.lock().unwrap();
            inner.last_attempt = Some((Instant::now() - DEFAULT_MAX_AGE, true));
        }
        // "Asked, and there is nothing to report" — an account type with no
        // rate limit, say — must not leave the previous account's reading on
        // screen, and it is not an error either.
        let outcome = cooldown.poll(at(2_000), DEFAULT_MAX_AGE, || Ok(None));

        assert!(outcome.snapshots.is_empty());
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn off_cooldown_reopens_once_its_own_budget_has_elapsed() {
        assert!(off_cooldown(None, DEFAULT_MAX_AGE));
        assert!(!off_cooldown(Some((Instant::now(), true)), DEFAULT_MAX_AGE));
        assert!(off_cooldown(
            Some((Instant::now() - DEFAULT_MAX_AGE, true)),
            DEFAULT_MAX_AGE
        ));
        assert!(!off_cooldown(
            Some((Instant::now() - FAILURE_COOLDOWN, true)),
            DEFAULT_MAX_AGE
        ));
        assert!(off_cooldown(
            Some((Instant::now() - FAILURE_COOLDOWN, false)),
            DEFAULT_MAX_AGE
        ));
    }

    #[test]
    fn a_failed_retry_never_waits_less_than_the_failure_floor_however_short_max_age_is() {
        // A caller asking for a ten-second `max_age` still gets
        // `MIN_FAILURE_COOLDOWN` for a failure, not ten seconds — see
        // `MIN_FAILURE_COOLDOWN`'s doc for why.
        let eager = Duration::from_secs(10);
        assert!(!off_cooldown(
            Some((Instant::now() - eager - Duration::from_secs(1), false)),
            eager
        ));
        assert!(off_cooldown(
            Some((Instant::now() - MIN_FAILURE_COOLDOWN, false)),
            eager
        ));
    }

    #[test]
    fn a_failed_retry_never_waits_longer_than_the_failure_ceiling_however_patient_max_age_is() {
        // A background caller asking for a `max_age` well past
        // `FAILURE_COOLDOWN` still gets `FAILURE_COOLDOWN` for a failure —
        // today's behaviour, preserved as a ceiling rather than the whole
        // rule.
        let patient = Duration::from_secs(3_600);
        assert!(!off_cooldown(
            Some((
                Instant::now() - FAILURE_COOLDOWN + Duration::from_secs(1),
                false
            )),
            patient
        ));
        assert!(off_cooldown(
            Some((Instant::now() - FAILURE_COOLDOWN, false)),
            patient
        ));
    }
}
