//! Reduce observed quota windows to the Overview's subscription chart.
//!
//! The Overview shows one account's rolling utilization: the
//! [`UTILIZATION_PERCENTILE`] percentile, by nearest rank, of every
//! account-wide quota window's estimated share (5-hour, weekly, and any
//! model-scoped weekly window), pooled over a trailing 28-day span. A
//! weekly-scale window's percent counts [`WEEKLY_POOL_WEIGHT`] times in the
//! pool; a 5-hour window's percent counts [`SHORT_POOL_WEIGHT`] time. This
//! module holds the pure reductions the chart and its headline share: a
//! weekly window's cumulative level, a window's own capped peak, and the
//! pooled rolling line both read from.

use std::collections::BTreeMap;

use crate::dto::QuotaPeriodPayload;

/// The maximum percentage a window's estimate shows as.
pub const MAXED_PERCENT: f64 = 100.0;

/// How far back the rolling pool looks for a closed window.
pub const POOL_SPAN_SECS: i64 = 28 * 24 * 60 * 60;

/// How many times a weekly-scale window's percent appears in the pool.
/// A weekly limit constrains work more than a 5-hour limit, so the
/// figure weights it more.
pub const WEEKLY_POOL_WEIGHT: usize = 5;

/// How many times a 5-hour window's percent appears in the pool.
pub const SHORT_POOL_WEIGHT: usize = 1;

/// How much window history the rolling line needs behind it before its
/// first point.
const LINE_START_LEAD_SECS: i64 = 7 * 24 * 60 * 60;

/// The weekly area's sample step.
const LEVEL_STEP_SECS: i64 = 60 * 60;

/// One point of a weekly-style window's cumulative level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelPoint {
    pub at_epoch: i64,
    pub percent: f64,
}

/// A weekly (or model-scoped weekly) window's cumulative level, sampled
/// hourly from zero at its start to its own estimate.
///
/// The level at each hour is the running sum of every contribution,
/// unattributed, and unexplained bucket percent up to that hour — the same
/// three sources [`crate::commands::quota::quota_usage_for_store`] sums into
/// `estimated_percent`, so a closed window's last point equals
/// `estimated_percent.min(100)` exactly: both figures sum the same buckets,
/// and a closed window's buckets already stay under 100 by
/// [`crate::provider_usage::quota::share::share_period_capped`]'s own
/// contract. Capping each point defends an open window's tail, which can
/// still run past 100 before it closes.
///
/// `None` when the period has no estimate at all. The single point at the
/// window's own start when it has not yet run a full hour, or has not yet
/// started relative to `now`.
pub fn weekly_levels(period: &QuotaPeriodPayload, now: i64) -> Option<Vec<LevelPoint>> {
    period.estimated_percent?;
    let start = period.starts_at_epoch;
    let end = period.resets_at_epoch.min(now);
    let mut points = vec![LevelPoint {
        at_epoch: start,
        percent: 0.0,
    }];
    if end <= start {
        return Some(points);
    }

    let mut by_bucket: BTreeMap<i64, f64> = BTreeMap::new();
    for bucket in &period.contributions {
        if let Some(percent) = bucket.percent {
            *by_bucket.entry(bucket.bucket_start_epoch).or_insert(0.0) += percent;
        }
    }
    for bucket in period
        .unattributed_buckets
        .iter()
        .chain(&period.unexplained_buckets)
    {
        if let Some(percent) = bucket.percent {
            *by_bucket.entry(bucket.bucket_start_epoch).or_insert(0.0) += percent;
        }
    }

    let steps = ((end - start) as f64 / LEVEL_STEP_SECS as f64).ceil() as i64;
    let mut cumulative = 0.0;
    for step in 1..=steps {
        let step_start = start + (step - 1) * LEVEL_STEP_SECS;
        let step_end = (start + step * LEVEL_STEP_SECS).min(end);
        let rise: f64 = by_bucket
            .range(step_start..step_end)
            .map(|(_, percent)| *percent)
            .sum();
        cumulative += rise;
        points.push(LevelPoint {
            at_epoch: step_end,
            percent: cumulative.min(MAXED_PERCENT),
        });
    }
    Some(points)
}

/// A window's own capped peak: its `estimated_percent`, capped at
/// [`MAXED_PERCENT`]. `None` when the window has no estimate.
pub fn window_peak(period: &QuotaPeriodPayload) -> Option<f64> {
    period
        .estimated_percent
        .map(|percent| percent.min(MAXED_PERCENT))
}

/// Whether the window that closes at `resets_at_epoch` (open when `open` is
/// true) belongs to the pool a headline or a rolling point reduces at time
/// `t`.
///
/// A closed window counts when its close falls in the trailing 28-day span
/// ending at `t`, `(t − 28 days, t]`. An open window counts only at the
/// pool's final evaluation, `t == now`, at its present level — `t` never
/// exceeds `now`, so an open window's close never falls at or before any
/// other `t` and this rule alone decides it.
pub fn pooled_at(resets_at_epoch: i64, open: bool, t: i64, now: i64) -> bool {
    let closed_in_span = resets_at_epoch > t - POOL_SPAN_SECS && resets_at_epoch <= t;
    closed_in_span || (t == now && open)
}

/// One point of the rolling utilization step line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RollingPoint {
    pub at_epoch: i64,
    pub percent: Option<f64>,
}

/// One window as the rolling pool reduces it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoolWindow {
    pub starts_at_epoch: i64,
    pub resets_at_epoch: i64,
    /// The window's own [`window_peak`].
    pub percent: f64,
    /// True when the window has not closed at `now`.
    pub open: bool,
    /// How many times `percent` appears in the pool.
    pub weight: usize,
}

/// The trailing 28-day pooled utilization, as a step line.
///
/// `windows` is every account-wide window (5-hour, weekly, and any
/// model-scoped weekly window) this account has. Each window's `percent`
/// appears `weight` times in the pool at every time it is counted — a
/// weekly-scale window carries [`WEEKLY_POOL_WEIGHT`], a 5-hour window
/// carries [`SHORT_POOL_WEIGHT`] — so a weekly limit shapes the figure more
/// than a 5-hour limit does. The line evaluates at every window close and at
/// every time a closed window leaves the span, from its start to `now`, plus
/// `now` itself, so the last point also pools every window still open, each
/// at its present level — the same rule [`pooled_at`] states. It emits a
/// point only when the pooled value changes, or at the first and the last
/// evaluation. A null value ends the line when the pool becomes empty.
///
/// The line starts at the first evaluation time with at least seven days of
/// window history behind it: `max(from_epoch, earliest window start + 7
/// days)`. It emits nothing when `now` falls before that start, or when
/// `windows` is empty.
pub fn rolling_utilization(windows: &[PoolWindow], now: i64, from_epoch: i64) -> Vec<RollingPoint> {
    let Some(earliest_start) = windows.iter().map(|w| w.starts_at_epoch).min() else {
        return Vec::new();
    };
    let line_start = from_epoch.max(earliest_start + LINE_START_LEAD_SECS);
    if now < line_start {
        return Vec::new();
    }

    // The pool changes when a window closes and when a closed window leaves
    // the trailing span, so the line evaluates at both times.
    let mut times: Vec<i64> = windows
        .iter()
        .flat_map(|w| [w.resets_at_epoch, w.resets_at_epoch + POOL_SPAN_SECS])
        .filter(|&t| t >= line_start && t <= now)
        .collect();
    times.push(line_start);
    times.push(now);
    times.sort_unstable();
    times.dedup();

    let mut points = Vec::new();
    let mut last_value: Option<f64> = None;
    let last_index = times.len() - 1;
    for (index, &t) in times.iter().enumerate() {
        let mut pooled: Vec<f64> = windows
            .iter()
            .filter(|w| pooled_at(w.resets_at_epoch, w.open, t, now))
            .flat_map(|w| std::iter::repeat_n(w.percent, w.weight))
            .collect();
        pooled.sort_by(f64::total_cmp);
        let value = (!pooled.is_empty()).then(|| percentile_by_rank(&pooled, |percent| *percent));
        let is_edge = index == 0 || index == last_index;
        if (is_edge && value.is_some()) || value != last_value {
            points.push(RollingPoint {
                at_epoch: t,
                percent: value,
            });
            last_value = value;
        }
    }
    points
}

/// The rank the pooled utilization figure reads. Tune the figure by
/// changing this constant alone.
const UTILIZATION_PERCENTILE: f64 = 0.5;

/// The [`UTILIZATION_PERCENTILE`] percentile of an already-sorted,
/// non-empty list.
///
/// Nearest-rank method: index = ceil(UTILIZATION_PERCENTILE * n) - 1. The
/// figure is always a value a real window reached, never an interpolation
/// between two. With one period the index resolves to that period, so the
/// figure equals its own value.
fn percentile_by_rank<T>(sorted: &[T], value: impl Fn(&T) -> f64) -> f64 {
    let n = sorted.len();
    let rank = (UTILIZATION_PERCENTILE * n as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(n - 1);
    value(&sorted[index])
}

#[cfg(test)]
mod tests;
