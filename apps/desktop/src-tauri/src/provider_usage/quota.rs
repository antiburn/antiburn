//! Derive the quota windows a lane ran through a time range.
//!
//! A provider periodically states a window's start and reset. Between those
//! statements, this module fills gaps: it extrapolates a weekly lane's
//! future and past windows from one observed reset, and it infers a
//! five-hour lane's windows from the local turn timeline when the provider
//! never reported one. [`resolve_periods`] is pure: it takes already-loaded
//! rows and epochs and returns [`QuotaPeriod`] values, so it needs no store
//! access and stays unit-testable on its own.

use crate::provider_usage::factor::window_start_epoch;
use crate::store::provider_limit::{
    FactorPoint, LANE_FIVE_HOUR, MODEL_LANE_PREFIX, lane_duration_seconds,
};
use crate::store::provider_usage_history::{ProviderUsagePeriod, RESET_JITTER_SECS};

/// How the resolver learned one boundary of a [`QuotaPeriod`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundarySource {
    /// The provider stated it directly.
    Reported,
    /// Computed from the other boundary and the lane's nominal duration.
    Derived,
    /// Extrapolated from another observed weekly reset, by whole weeks.
    Cadence,
    /// Inferred from the first local turn after a gap of at least the
    /// lane's duration.
    TurnGap,
    /// The next observed window began before this one's stated reset, so
    /// the provider ended this window early.
    Truncated,
}

/// One quota window: a span with a source for each end.
///
/// `period_id` is `Some` only for a window backed by an observed
/// [`ProviderUsagePeriod`] row. A cadence-extrapolated or turn-gap-inferred
/// window carries `None`: it exists only in this resolver's output, not in
/// the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaPeriod {
    pub period_id: Option<i64>,
    pub starts_at_epoch: i64,
    pub resets_at_epoch: i64,
    pub start_source: BoundarySource,
    pub reset_source: BoundarySource,
}

/// Five-hour windows this resolver infers from turn gaps, per call.
const MAX_INFERRED_WINDOWS: usize = 400;

/// Every quota window a lane ran through `[range_start, range_end)`, plus the
/// window that is open right now even when it reaches past `range_end`.
///
/// `observed` is the account's stored periods for this lane: rows overlapping
/// the range, plus the most recent one before it, which anchors weekly
/// cadence extrapolation. `turn_epochs` is every local turn epoch attributed
/// to the account inside the range, ascending, used only for a five-hour
/// lane. Returned periods are sorted by start; the caller clips to the range
/// itself, since a window can straddle a boundary.
pub fn resolve_periods(
    lane: &str,
    lane_duration: i64,
    observed: &[ProviderUsagePeriod],
    turn_epochs: &[i64],
    range_start: i64,
    range_end: i64,
    now: i64,
) -> Vec<QuotaPeriod> {
    let mut resolved_observed: Vec<QuotaPeriod> = observed
        .iter()
        .filter_map(|period| resolve_observed(period, lane))
        .collect();

    let is_weekly_like =
        lane == crate::store::provider_limit::LANE_WEEKLY || lane.starts_with(MODEL_LANE_PREFIX);
    if is_weekly_like {
        resolved_observed = clip_truncated_weekly_windows(resolved_observed);
    }

    let mut periods: Vec<QuotaPeriod> = resolved_observed
        .iter()
        .copied()
        .filter(|period| in_range_or_open(period, range_start, range_end, now))
        .collect();

    if lane == LANE_FIVE_HOUR {
        let inferred = infer_five_hour_windows(&resolved_observed, turn_epochs);
        periods.extend(
            inferred
                .into_iter()
                .filter(|period| in_range_or_open(period, range_start, range_end, now)),
        );
    } else {
        let anchor_reset = resolved_observed
            .iter()
            .map(|period| period.resets_at_epoch)
            .max()
            .filter(|_| is_weekly_like);
        if let Some(anchor_reset) = anchor_reset {
            for slot in
                weekly_cadence_slots(anchor_reset, lane_duration, range_start, range_end, now)
            {
                if !overlaps_any_by_more_than_jitter(&slot, &resolved_observed) {
                    periods.push(slot);
                }
            }
        }
    }

    periods.sort_by_key(|period| period.starts_at_epoch);
    periods
}

/// Turn one observed period row into a [`QuotaPeriod`], deriving whichever
/// boundary the provider did not state from the other one and the lane's
/// nominal duration. `None` when neither boundary is known.
fn resolve_observed(period: &ProviderUsagePeriod, lane: &str) -> Option<QuotaPeriod> {
    let starts_at_epoch = window_start_epoch(period, lane)?;
    let start_source = if period.starts_at_epoch.is_some() {
        BoundarySource::Reported
    } else {
        BoundarySource::Derived
    };
    let (resets_at_epoch, reset_source) = match period.resets_at_epoch {
        Some(reset) => (reset, BoundarySource::Reported),
        None => (
            starts_at_epoch + lane_duration_seconds(lane),
            BoundarySource::Derived,
        ),
    };
    Some(QuotaPeriod {
        period_id: Some(period.id),
        starts_at_epoch,
        resets_at_epoch,
        start_source,
        reset_source,
    })
}

/// Merge duplicate weekly readings, and cut short a window the provider
/// ended early.
///
/// A weekly lane can restart before its stated reset. Codex does this: it
/// never reports a window's start, so the resolver derives one from that
/// window's own reset, seven days back. When Codex ends a window early, the
/// next window's own start, derived the same way, falls before the old
/// window's stated reset. That next start marks where the old window really
/// ended, so this function moves the old window's reset there and marks it
/// [`BoundarySource::Truncated`]. It also merges two readings of the same
/// window whose reported reset drifted by at most [`RESET_JITTER_SECS`],
/// keeping the one with the later reset as the more complete reading.
fn clip_truncated_weekly_windows(periods: Vec<QuotaPeriod>) -> Vec<QuotaPeriod> {
    let mut sorted = periods;
    sorted.sort_by_key(|period| (period.starts_at_epoch, period.resets_at_epoch));

    let mut merged: Vec<QuotaPeriod> = Vec::with_capacity(sorted.len());
    for period in sorted {
        if let Some(last) = merged.last_mut()
            && (period.starts_at_epoch - last.starts_at_epoch).abs() <= RESET_JITTER_SECS
        {
            if period.resets_at_epoch > last.resets_at_epoch {
                *last = period;
            }
            continue;
        }
        merged.push(period);
    }

    for index in 0..merged.len().saturating_sub(1) {
        let next_start = merged[index + 1].starts_at_epoch;
        if next_start < merged[index].resets_at_epoch {
            merged[index].resets_at_epoch = next_start;
            merged[index].reset_source = BoundarySource::Truncated;
        }
    }
    merged.retain(|period| period.resets_at_epoch > period.starts_at_epoch);
    merged
}

/// Whether a period belongs in the resolver's output: it overlaps the
/// half-open query range, or it is the window running right now, even when
/// that window reaches past `range_end`.
fn in_range_or_open(period: &QuotaPeriod, range_start: i64, range_end: i64, now: i64) -> bool {
    let overlaps_range = period.starts_at_epoch < range_end && period.resets_at_epoch > range_start;
    let is_open = period.starts_at_epoch <= now && period.resets_at_epoch > now;
    overlaps_range || is_open
}

/// Seconds two periods' spans share, zero when they do not overlap.
fn overlap_seconds(left: &QuotaPeriod, right: &QuotaPeriod) -> i64 {
    (left.resets_at_epoch.min(right.resets_at_epoch)
        - left.starts_at_epoch.max(right.starts_at_epoch))
    .max(0)
}

fn overlaps_any_by_more_than_jitter(slot: &QuotaPeriod, observed: &[QuotaPeriod]) -> bool {
    observed
        .iter()
        .any(|period| overlap_seconds(slot, period) > RESET_JITTER_SECS)
}

/// Floor division, since Rust's `/` truncates toward zero and a slot index
/// can be negative (a cadence anchor's past windows).
fn div_floor(numerator: i64, denominator: i64) -> i64 {
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder != 0 && (remainder < 0) != (denominator < 0) {
        quotient - 1
    } else {
        quotient
    }
}

/// Every weekly slot, anchored on `anchor_reset` and stepped by whole weeks,
/// whose span could matter to the query: it overlaps the range, or it is the
/// slot running now.
fn weekly_cadence_slots(
    anchor_reset: i64,
    lane_duration: i64,
    range_start: i64,
    range_end: i64,
    now: i64,
) -> Vec<QuotaPeriod> {
    let window_start = range_start.min(now) - lane_duration;
    let window_end = range_end.max(now) + lane_duration;
    let k_min = div_floor(window_start - anchor_reset, lane_duration);
    let k_max = div_floor(window_end - anchor_reset, lane_duration) + 1;
    let mut slots = Vec::new();
    let mut k = k_min;
    while k <= k_max {
        let resets_at_epoch = anchor_reset + k * lane_duration;
        let starts_at_epoch = resets_at_epoch - lane_duration;
        let slot = QuotaPeriod {
            period_id: None,
            starts_at_epoch,
            resets_at_epoch,
            start_source: BoundarySource::Cadence,
            reset_source: BoundarySource::Cadence,
        };
        if in_range_or_open(&slot, range_start, range_end, now) {
            slots.push(slot);
        }
        k += 1;
    }
    slots
}

/// Open a five-hour window at the first turn that falls inside no observed
/// period and no already-inferred window, ascending through `turn_epochs`.
///
/// `turn_epochs` and the windows this opens are both processed in order, so
/// checking a new turn against only the most recently opened window's end is
/// enough to know whether it lands inside it: no earlier inferred window can
/// reach further forward.
fn infer_five_hour_windows(observed: &[QuotaPeriod], turn_epochs: &[i64]) -> Vec<QuotaPeriod> {
    let mut inferred = Vec::new();
    let mut open_until: Option<i64> = None;
    for &turn_epoch in turn_epochs {
        if inferred.len() >= MAX_INFERRED_WINDOWS {
            break;
        }
        if let Some(until) = open_until
            && turn_epoch < until
        {
            continue;
        }
        if observed.iter().any(|period| {
            turn_epoch >= period.starts_at_epoch && turn_epoch < period.resets_at_epoch
        }) {
            continue;
        }
        let natural_reset = turn_epoch + lane_duration_seconds(LANE_FIVE_HOUR);
        // An inferred window must not run over an observed period that
        // starts inside it: otherwise a bucket in that overlap would credit
        // to this inferred window instead of the observed one that actually
        // covers it. Clip to the earliest such start, when one exists.
        let resets_at_epoch = observed
            .iter()
            .map(|period| period.starts_at_epoch)
            .filter(|&start| start > turn_epoch && start < natural_reset)
            .min()
            .unwrap_or(natural_reset);
        open_until = Some(resets_at_epoch);
        inferred.push(QuotaPeriod {
            period_id: None,
            starts_at_epoch: turn_epoch,
            resets_at_epoch,
            start_source: BoundarySource::TurnGap,
            reset_source: BoundarySource::TurnGap,
        });
    }
    inferred
}

/// The factor point in effect at `at_epoch`, from a lane's whole point
/// series ordered by `effective_at_epoch` ascending: the latest point at or
/// before it, else the earliest point of all. `None` when the lane has no
/// factor point yet.
///
/// Mirrors [`crate::store::Store::factor_point_at`] without a query per
/// bucket: the quota screen loads a lane's points once per call and
/// binary-searches this instead.
pub fn factor_point_at_or_earliest(points: &[FactorPoint], at_epoch: i64) -> Option<&FactorPoint> {
    if points.is_empty() {
        return None;
    }
    let split = points.partition_point(|point| point.effective_at_epoch <= at_epoch);
    Some(if split == 0 {
        &points[0]
    } else {
        &points[split - 1]
    })
}

/// The pure algorithm that shares a period's meter rise across its buckets.
/// Kept in its own module since `commands::quota` and this module's own
/// tests both need it.
pub(crate) mod share;

#[cfg(test)]
mod tests;
