//! What the history of a window says that any single reading cannot.
//!
//! A percentage answers "how much is gone". These functions answer the two
//! questions a reader actually has — *how fast* is it going, and *will it
//! last* — from a series of readings of the same window.
//!
//! # Everything here can decline to answer
//!
//! And it does so far more often than it answers, which is the design rather
//! than a shortfall. [`UsageForecast`] carries an explicit
//! [`ForecastUnavailableReason`] instead of a zero or a null, because "we have
//! not seen enough of your week to say" and "you are on track" are different
//! answers and only one of them is reassuring. The guards, in the order they
//! fire:
//!
//! - **Stale.** A reading that no longer describes the present cannot
//!   describe a rate either.
//! - **Transition.** A window whose percentage just *dropped* has reset (or
//!   the provider corrected itself); the pair spanning that drop describes two
//!   different weeks and the rate across it is meaningless. Only the segment
//!   since the most recent drop is used.
//! - **Sparse history.** Fewer than two comparable readings in the last two
//!   hours, *or* readings that span less than [`MIN_SPAN`]. This is the common
//!   case with a source that only updates when an agent runs, and it is why
//!   the surfaces all have a "not enough history" branch that is not an error
//!   state.
//!
//! The span floor is the one that was learnt rather than designed. Two
//! readings thirty seconds apart satisfy every count-based check and produce
//! an authoritative-looking rate that is pure noise: observed live, the same
//! window reported "At risk, 30%/hour, runs out in 3h" and then, minutes
//! later, "Comfortable, 6.8%/hour, lasts past the reset". A verdict that can
//! invert while a reader watches is worse than no verdict, so a rate now
//! needs a span long enough for the underlying figure to have plausibly
//! moved.
//!
//! # Deliberate data model choices
//!
//! - **No account and plan fields on a sample.** A sample's key already contains
//!   the provider, the account, and the window id, so two samples in one
//!   series cannot differ in either — a field that can only hold one value is
//!   not a guard, it is furniture.
//! - **No reset-crossing helper.** This application has no anomaly engine
//!   because usage rates are not predictable or steady enough. The simpler
//!   percentage-drop test below is what the forecast actually needs.

use time::{Duration, OffsetDateTime};

use super::model::{Confidence, Freshness};

/// The shortest span of readings that can support a rate.
///
/// Fifteen minutes. Below this the denominator is small enough that ordinary
/// jitter in when the provider recomputed its own figure dominates the
/// result — and the surfaces present that result as a verdict, in colour.
const MIN_SPAN: Duration = Duration::minutes(15);

/// One reading of one window, kept so later readings have something to
/// compare against.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSample {
    /// When the *provider* stated this, not when it was read.
    pub observed_at: OffsetDateTime,
    /// Consumed capacity in `0..=100`.
    pub used_percent: Option<f64>,
    /// Whether the reading described the present when it was taken. Stale
    /// samples are kept — they are still history — but never contribute to a
    /// rate.
    pub freshness: Freshness,
}

/// A rate over comparable readings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConsumptionRate {
    /// Percentage points consumed per hour. Never remaining per hour.
    pub percent_per_hour: f64,
    /// How many pairs actually moved. Used to suppress a trend computed
    /// entirely from readings that did not change.
    pub changed_samples: usize,
}

/// Why a window has no forecast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForecastUnavailableReason {
    /// The current reading no longer describes the present.
    Stale,
    /// The newest pair spans a reset or a correction.
    Transition,
    /// Too few comparable readings to say anything.
    SparseHistory,
}

impl ForecastUnavailableReason {
    /// The wire spelling the views branch on.
    pub fn as_str(self) -> &'static str {
        match self {
            ForecastUnavailableReason::Stale => "stale",
            ForecastUnavailableReason::Transition => "transition",
            ForecastUnavailableReason::SparseHistory => "sparseHistory",
        }
    }
}

/// What the history of one window supports saying.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageForecast {
    /// How far to trust the values below, when there are any.
    pub confidence: Option<Confidence>,
    /// Why there are none, when there are none. Exactly one of this and
    /// `confidence` is set.
    pub unavailable_reason: Option<ForecastUnavailableReason>,
    /// Percentage points per hour over the recent window.
    pub consumption_rate: Option<f64>,
    /// The current rate over the rate that would exactly exhaust at reset.
    /// Above 1 means the allowance runs out first.
    pub pace_ratio: Option<f64>,
    /// The last half hour's rate over the last two hours'. Above 1 means
    /// speeding up.
    pub pace_trend: Option<f64>,
    /// When the allowance runs out at the recent rate.
    pub estimated_exhaustion_at: Option<OffsetDateTime>,
}

impl UsageForecast {
    fn unavailable(reason: ForecastUnavailableReason) -> UsageForecast {
        UsageForecast {
            confidence: None,
            unavailable_reason: Some(reason),
            consumption_rate: None,
            pace_ratio: None,
            pace_trend: None,
            estimated_exhaustion_at: None,
        }
    }
}

/// The average rate across every comparable adjacent pair.
///
/// A pair is comparable when neither reading is stale, both carry a
/// percentage, time moved forward, and the percentage did not go *down*. That
/// last condition drops reset and correction pairs, which is why a rate can
/// be computed across a series that contains one.
pub fn consumption_rate(samples: &[UsageSample]) -> Option<ConsumptionRate> {
    let mut total_delta = 0.0;
    let mut total_hours = 0.0;
    let mut changed = 0;
    let mut comparable = 0;
    for pair in samples.windows(2) {
        let Some((delta, hours)) = comparable_delta(&pair[0], &pair[1]) else {
            continue;
        };
        comparable += 1;
        total_delta += delta;
        total_hours += hours;
        if delta > 0.0 {
            changed += 1;
        }
    }
    (comparable > 0 && total_hours > 0.0).then_some(ConsumptionRate {
        percent_per_hour: total_delta / total_hours,
        changed_samples: changed,
    })
}

fn comparable_delta(a: &UsageSample, b: &UsageSample) -> Option<(f64, f64)> {
    if a.freshness == Freshness::Stale || b.freshness == Freshness::Stale {
        return None;
    }
    let delta = b.used_percent? - a.used_percent?;
    let hours = (b.observed_at - a.observed_at).as_seconds_f64() / 3600.0;
    (delta >= 0.0 && hours > 0.0).then_some((delta, hours))
}

/// Everything the history of one window supports, or why it supports nothing.
pub fn usage_forecast(
    samples: &[UsageSample],
    used_percent: Option<f64>,
    resets_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
    current_freshness: Freshness,
    current_confidence: Confidence,
) -> UsageForecast {
    if current_freshness == Freshness::Stale {
        return UsageForecast::unavailable(ForecastUnavailableReason::Stale);
    }
    if latest_pair_is_transition(samples) {
        return UsageForecast::unavailable(ForecastUnavailableReason::Transition);
    }

    let stable = latest_stable_segment(samples);
    let recent: Vec<UsageSample> = stable
        .iter()
        .filter(|sample| sample.observed_at >= now - Duration::hours(2))
        .cloned()
        .collect();
    if recent.len() < 2 || span(&recent).is_none_or(|span| span < MIN_SPAN) {
        return UsageForecast::unavailable(ForecastUnavailableReason::SparseHistory);
    }
    let Some(rate) = consumption_rate(&recent) else {
        return UsageForecast::unavailable(ForecastUnavailableReason::SparseHistory);
    };
    let short: Vec<UsageSample> = recent
        .iter()
        .filter(|sample| sample.observed_at >= now - Duration::minutes(30))
        .cloned()
        .collect();

    UsageForecast {
        // Six readings is where a two-hour rate stops being two points and a
        // straight line between them.
        confidence: Some(
            if recent.len() >= 6 && current_confidence == Confidence::High {
                Confidence::High
            } else {
                Confidence::Medium
            },
        ),
        unavailable_reason: None,
        consumption_rate: Some(rate.percent_per_hour),
        pace_ratio: pace_ratio(rate, used_percent, resets_at, now),
        pace_trend: pace_trend(&short, &recent),
        estimated_exhaustion_at: estimated_exhaustion_at(now, used_percent, rate),
    }
}

/// How much time the readings cover, oldest to newest.
fn span(samples: &[UsageSample]) -> Option<Duration> {
    let first = samples.first()?;
    let last = samples.last()?;
    Some(last.observed_at - first.observed_at)
}

fn latest_pair_is_transition(samples: &[UsageSample]) -> bool {
    match samples.last_chunk::<2>() {
        Some([previous, current]) => crosses_transition(previous, current),
        None => false,
    }
}

/// The tail of the series since the most recent reset or correction.
fn latest_stable_segment(samples: &[UsageSample]) -> &[UsageSample] {
    let start = samples
        .windows(2)
        .rposition(|pair| crosses_transition(&pair[0], &pair[1]))
        .map_or(0, |index| index + 1);
    &samples[start..]
}

/// Whether the percentage went down between two readings.
///
/// Either the window reset or the provider corrected itself. We do not need
/// to know which: in both cases the two readings describe different periods
/// and no rate across them means anything.
fn crosses_transition(previous: &UsageSample, current: &UsageSample) -> bool {
    matches!(
        (previous.used_percent, current.used_percent),
        (Some(old), Some(new)) if new < old
    )
}

/// The current rate over the rate that would land exactly at the reset.
///
/// One is on track. Two is twice as fast as the allowance can afford.
pub fn pace_ratio(
    recent_rate: ConsumptionRate,
    used_percent: Option<f64>,
    resets_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> Option<f64> {
    let remaining = 100.0 - used_percent?;
    let hours = (resets_at? - now).as_seconds_f64() / 3600.0;
    if !(0.0..=100.0).contains(&remaining) || hours <= 0.0 {
        return None;
    }
    let required = remaining / hours;
    if required == 0.0 {
        // Nothing left and time still to run: any consumption at all is
        // infinitely faster than an allowance that is already gone.
        return Some(f64::INFINITY);
    }
    Some(recent_rate.percent_per_hour / required)
}

/// The short-horizon rate over the long one, when both are worth comparing.
///
/// The sample-count floors are what stop a trend being reported from readings
/// that did not actually move, and the rate floor stops a near-zero
/// denominator turning a rounding difference into "3× faster".
pub fn pace_trend(short: &[UsageSample], long: &[UsageSample]) -> Option<f64> {
    let short = consumption_rate(short)?;
    let long = consumption_rate(long)?;
    if short.changed_samples < 1 || long.changed_samples < 2 || long.percent_per_hour <= 0.01 {
        return None;
    }
    Some(short.percent_per_hour / long.percent_per_hour)
}

/// When the allowance runs out at the recent rate.
pub fn estimated_exhaustion_at(
    now: OffsetDateTime,
    used_percent: Option<f64>,
    recent_rate: ConsumptionRate,
) -> Option<OffsetDateTime> {
    let remaining = 100.0 - used_percent?;
    if !(0.0..=100.0).contains(&remaining) || recent_rate.percent_per_hour <= 0.0 {
        return None;
    }
    now.checked_add(Duration::seconds_f64(
        remaining / recent_rate.percent_per_hour * 3600.0,
    ))
}

/// Whether trustworthy history shows non-zero usage anywhere in a window's
/// current allowance period.
///
/// This exists for one purpose: a supplemental, model-scoped weekly window
/// (a per-model limit alongside the account-wide one) sits at 0% for most
/// readers, who never touch that model. The views hide such a window until
/// it has something to say and, once it does, keep showing it for the rest
/// of that period even if a later reading comes back unknown — see
/// `liveUsage.ts::isUsageWindowVisible` on the frontend, which this feeds.
///
/// "Current period" reuses exactly what [`usage_forecast`] already means by
/// it — the tail of the series since the most recent percentage drop
/// ([`latest_stable_segment`]) — rather than inventing a second definition.
/// That choice is deliberate: a [`UsageSample`] here carries no reset
/// timestamp of its own, so there is only one place "which period is this"
/// can be answered, and no second copy to drift out of sync with it. (A
/// sibling app that shipped this same behaviour kept a `reset_epoch` on each
/// stored sample and matched it against the window's `resets_at`; the two
/// independently-derived answers disagreed and hid windows that were
/// genuinely in use.)
///
/// `resets_at` still earns a role, as a sanity check rather than a boundary:
/// a reset already behind `now` means the provider's own account of this
/// window has rolled, or gone stale, since anything below was read, and no
/// percentage-drop in the history can be trusted to describe the period that
/// is current *now*. Missing reset metadata is treated the same way, for the
/// same reason — there is no period here to bound.
pub fn has_nonzero_usage_in_current_period(
    samples: &[UsageSample],
    resets_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> bool {
    if resets_at.is_none_or(|reset| reset <= now) {
        return false;
    }
    latest_stable_segment(samples)
        .iter()
        .any(|sample| sample.used_percent.is_some_and(|percent| percent > 0.0))
}

/// How many percentage points of a window were consumed since local midnight.
///
/// Only fresh, non-decreasing pairs count. A pair spanning a reset
/// contributes nothing rather than an inferred first-post-reset figure:
/// guessing that number over-counts exactly when the provider's reset
/// metadata is least stable, which is the worst possible time.
pub fn used_since(samples: &[UsageSample], local_midnight: OffsetDateTime) -> Option<f64> {
    let relevant: Vec<&UsageSample> = samples
        .iter()
        .filter(|sample| sample.observed_at >= local_midnight)
        .filter(|sample| sample.freshness == Freshness::Fresh)
        .collect();
    if relevant.len() < 2 {
        return None;
    }
    let mut total = 0.0;
    let mut evidence = false;
    for pair in relevant.windows(2) {
        let (Some(start), Some(end)) = (pair[0].used_percent, pair[1].used_percent) else {
            continue;
        };
        if end >= start {
            total += end - start;
            evidence = true;
        }
    }
    evidence.then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn at(offset_secs: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(NOW + offset_secs).expect("valid timestamp")
    }

    fn sample(offset_secs: i64, percent: f64) -> UsageSample {
        UsageSample {
            observed_at: at(offset_secs),
            used_percent: Some(percent),
            freshness: Freshness::Fresh,
        }
    }

    fn forecast(samples: &[UsageSample], used: f64, resets_in_secs: i64) -> UsageForecast {
        usage_forecast(
            samples,
            Some(used),
            Some(at(resets_in_secs)),
            at(0),
            Freshness::Fresh,
            Confidence::High,
        )
    }

    #[test]
    fn a_rate_is_the_total_change_over_the_total_elapsed_time() {
        // 10 points over two hours.
        let samples = [sample(-7_200, 20.0), sample(-3_600, 25.0), sample(0, 30.0)];
        let rate = consumption_rate(&samples).expect("comparable");
        assert!((rate.percent_per_hour - 5.0).abs() < 1e-9);
        assert_eq!(rate.changed_samples, 2);
    }

    #[test]
    fn a_stale_reading_never_contributes_to_a_rate() {
        let mut samples = [sample(-3_600, 20.0), sample(0, 30.0)];
        samples[0].freshness = Freshness::Stale;
        assert_eq!(consumption_rate(&samples), None);
    }

    #[test]
    fn a_reset_splits_the_series_rather_than_poisoning_the_rate() {
        // Climbs to 90, resets to 5, climbs again. A rate across the drop
        // would be negative and meaningless; only the tail is used.
        let samples = [
            sample(-14_400, 80.0),
            sample(-10_800, 90.0),
            sample(-7_200, 5.0),
            sample(-3_600, 10.0),
            sample(0, 20.0),
        ];
        let result = forecast(&samples, 20.0, 3_600);
        assert_eq!(result.unavailable_reason, None);
        // 15 points over two hours from the post-reset segment alone.
        assert!((result.consumption_rate.unwrap() - 7.5).abs() < 1e-9);
    }

    #[test]
    fn a_reset_in_the_newest_pair_suppresses_the_forecast_entirely() {
        // Not enough of the new period has been seen to say anything yet.
        let samples = [sample(-3_600, 90.0), sample(0, 5.0)];
        assert_eq!(
            forecast(&samples, 5.0, 3_600).unavailable_reason,
            Some(ForecastUnavailableReason::Transition)
        );
    }

    #[test]
    fn a_stale_current_reading_suppresses_the_forecast_before_anything_else() {
        let samples = [sample(-3_600, 20.0), sample(0, 30.0)];
        let result = usage_forecast(
            &samples,
            Some(30.0),
            Some(at(3_600)),
            at(0),
            Freshness::Stale,
            Confidence::High,
        );
        assert_eq!(
            result.unavailable_reason,
            Some(ForecastUnavailableReason::Stale)
        );
    }

    #[test]
    fn one_reading_in_the_recent_window_is_sparse_history_not_a_zero_rate() {
        // The common case for a source that only updates when an agent runs.
        // "Not enough history" and "you are using nothing" must not look alike.
        let samples = [sample(-10_800, 20.0), sample(0, 30.0)];
        assert_eq!(
            forecast(&samples, 30.0, 3_600).unavailable_reason,
            Some(ForecastUnavailableReason::SparseHistory)
        );
    }

    #[test]
    fn two_readings_moments_apart_are_not_a_rate() {
        // The defect this floor exists for, reproduced. Two readings thirty
        // seconds apart pass every count-based check and yield 360%/hour,
        // which the surfaces would render as a red "At risk" verdict and then
        // contradict minutes later.
        let moments = [sample(-30, 7.0), sample(0, 10.0)];
        assert_eq!(
            forecast(&moments, 10.0, 18_000).unavailable_reason,
            Some(ForecastUnavailableReason::SparseHistory)
        );

        // The same two readings far enough apart to mean something.
        let spread = [sample(-3_600, 7.0), sample(0, 10.0)];
        let result = forecast(&spread, 10.0, 18_000);
        assert_eq!(result.unavailable_reason, None);
        assert!((result.consumption_rate.unwrap() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn the_span_floor_is_measured_end_to_end_not_per_pair() {
        // Many readings inside one short burst are still one short burst.
        let burst: Vec<UsageSample> = (0..8)
            .map(|index| sample(-700 + index * 100, 10.0 + index as f64))
            .collect();
        assert_eq!(
            forecast(&burst, 18.0, 18_000).unavailable_reason,
            Some(ForecastUnavailableReason::SparseHistory)
        );
    }

    #[test]
    fn confidence_rises_only_once_the_recent_window_is_more_than_a_straight_line() {
        let two = [sample(-3_600, 20.0), sample(0, 30.0)];
        assert_eq!(
            forecast(&two, 30.0, 3_600).confidence,
            Some(Confidence::Medium)
        );

        let many: Vec<UsageSample> = (0..6)
            .map(|index| sample(-6_000 + index * 1_000, 20.0 + index as f64))
            .collect();
        assert_eq!(
            forecast(&many, 25.0, 3_600).confidence,
            Some(Confidence::High)
        );
    }

    #[test]
    fn pace_compares_the_current_rate_with_the_one_the_allowance_can_afford() {
        let rate = ConsumptionRate {
            percent_per_hour: 10.0,
            changed_samples: 3,
        };
        // 50 left, 10 hours to reset: 5/hour affordable against 10/hour actual.
        assert_eq!(
            pace_ratio(rate, Some(50.0), Some(at(36_000)), at(0)),
            Some(2.0)
        );
        // A reset already past cannot anchor a pace.
        assert_eq!(pace_ratio(rate, Some(50.0), Some(at(-1)), at(0)), None);
        // Nothing left and time still running.
        assert_eq!(
            pace_ratio(rate, Some(100.0), Some(at(3_600)), at(0)),
            Some(f64::INFINITY)
        );
    }

    #[test]
    fn a_trend_needs_readings_that_actually_moved() {
        let flat = [sample(-1_800, 20.0), sample(-900, 20.0), sample(0, 20.0)];
        assert_eq!(pace_trend(&flat, &flat), None);

        // Long: 12.5 points over two hours = 6.25/hour. Short: 5 points over
        // the last half hour = 10/hour. Speeding up.
        let long = [
            sample(-7_200, 20.0),
            sample(-3_600, 25.0),
            sample(-1_800, 27.5),
            sample(0, 32.5),
        ];
        let short = [sample(-1_800, 27.5), sample(0, 32.5)];
        assert_eq!(pace_trend(&short, &long), Some(1.6));
    }

    #[test]
    fn runway_is_the_remaining_capacity_at_the_current_rate() {
        let rate = ConsumptionRate {
            percent_per_hour: 10.0,
            changed_samples: 3,
        };
        // 40 left at 10/hour is four hours.
        assert_eq!(
            estimated_exhaustion_at(at(0), Some(60.0), rate),
            Some(at(14_400))
        );
        // A rate of nothing never exhausts anything.
        assert_eq!(
            estimated_exhaustion_at(
                at(0),
                Some(60.0),
                ConsumptionRate {
                    percent_per_hour: 0.0,
                    changed_samples: 0
                }
            ),
            None
        );
    }

    #[test]
    fn today_counts_only_what_was_observed_since_midnight() {
        let samples = [
            sample(-86_400, 10.0), // yesterday
            sample(-3_600, 40.0),
            sample(0, 55.0),
        ];
        assert_eq!(used_since(&samples, at(-7_200)), Some(15.0));
        // One reading inside the day proves no change at all.
        assert_eq!(used_since(&samples, at(-1_800)), None);
    }

    #[test]
    fn a_reset_inside_the_day_contributes_nothing_rather_than_an_inferred_figure() {
        let samples = [sample(-7_200, 90.0), sample(-3_600, 5.0), sample(0, 12.0)];
        // Only the 5 → 12 pair counts. The drop is skipped, not inferred as
        // "10 more then a reset", which would over-count exactly when the
        // provider's reset metadata is least stable.
        assert_eq!(used_since(&samples, at(-10_800)), Some(7.0));
    }

    /* ---------------------------------------------------------------------
     * Whether a window's current period has shown any usage
     * ------------------------------------------------------------------- */

    #[test]
    fn hides_a_window_whose_current_period_never_showed_usage() {
        let samples = [sample(-7_200, 0.0), sample(-3_600, 0.0), sample(0, 0.0)];
        assert!(!has_nonzero_usage_in_current_period(
            &samples,
            Some(at(3_600)),
            at(0)
        ));
    }

    #[test]
    fn shows_a_window_once_its_current_period_has_any_nonzero_reading() {
        let samples = [sample(-7_200, 0.0), sample(-3_600, 4.0), sample(0, 4.0)];
        assert!(has_nonzero_usage_in_current_period(
            &samples,
            Some(at(3_600)),
            at(0)
        ));
    }

    #[test]
    fn a_reset_since_the_nonzero_reading_drops_it_from_the_current_period() {
        // Used 20 points last period, then reset to 0. This period alone has
        // shown nothing yet, so last period's usage must not leak forward.
        let samples = [sample(-7_200, 20.0), sample(-3_600, 0.0), sample(0, 0.0)];
        assert!(!has_nonzero_usage_in_current_period(
            &samples,
            Some(at(3_600)),
            at(0)
        ));
    }

    #[test]
    fn stays_visible_when_the_latest_reading_in_the_period_is_unknown() {
        // The sticky case this feature exists for: a later reading with no
        // percentage must not erase what an earlier one in the same period
        // already proved.
        let samples = [
            sample(-3_600, 12.0),
            UsageSample {
                observed_at: at(0),
                used_percent: None,
                freshness: Freshness::Fresh,
            },
        ];
        assert!(has_nonzero_usage_in_current_period(
            &samples,
            Some(at(3_600)),
            at(0)
        ));
    }

    #[test]
    fn a_reset_already_behind_now_cannot_be_trusted() {
        // The provider's own account of this window has rolled (or gone
        // stale) since the sample was read; no drop in the history can prove
        // which period is current now.
        let samples = [sample(-3_600, 40.0)];
        assert!(!has_nonzero_usage_in_current_period(
            &samples,
            Some(at(-1)),
            at(0)
        ));
    }

    #[test]
    fn missing_reset_metadata_cannot_be_trusted_either() {
        let samples = [sample(-3_600, 40.0)];
        assert!(!has_nonzero_usage_in_current_period(&samples, None, at(0)));
    }
}
