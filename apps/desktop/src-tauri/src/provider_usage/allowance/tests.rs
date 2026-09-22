use super::*;

const DAY: i64 = 24 * 60 * 60;

fn period(start: i64, resets: i64, estimated: Option<f64>) -> QuotaPeriodPayload {
    QuotaPeriodPayload {
        period_id: None,
        starts_at_epoch: start,
        resets_at_epoch: resets,
        start_source: "turnGap".to_string(),
        reset_source: "derived".to_string(),
        samples: Vec::new(),
        contributions: Vec::new(),
        sessions: Vec::new(),
        unattributed: crate::dto::QuotaUnattributedPayload {
            usd: 0.0,
            percent: None,
            session_count: 0,
        },
        unattributed_buckets: Vec::new(),
        estimated_percent: estimated,
        unexplained_buckets: Vec::new(),
        unexplained_percent: None,
    }
}

fn contribution(bucket_start_epoch: i64, percent: f64) -> crate::dto::QuotaContributionPayload {
    crate::dto::QuotaContributionPayload {
        agent: "claude".to_string(),
        session_id: "one".to_string(),
        wsl_distro: None,
        bucket_start_epoch,
        usd: 1.0,
        percent: Some(percent),
    }
}

fn bucket(bucket_start_epoch: i64, percent: f64) -> crate::dto::QuotaBucketTotalPayload {
    crate::dto::QuotaBucketTotalPayload {
        bucket_start_epoch,
        usd: 0.0,
        percent: Some(percent),
    }
}

/// A pool window at [`SHORT_POOL_WEIGHT`], a 5-hour window's own weight.
fn window(start: i64, resets: i64, percent: f64, open: bool) -> PoolWindow {
    PoolWindow {
        starts_at_epoch: start,
        resets_at_epoch: resets,
        percent,
        open,
        weight: SHORT_POOL_WEIGHT,
    }
}

/// A pool window at [`WEEKLY_POOL_WEIGHT`], for a weekly or model-scoped
/// weekly window.
fn weekly(start: i64, resets: i64, percent: f64, open: bool) -> PoolWindow {
    PoolWindow {
        starts_at_epoch: start,
        resets_at_epoch: resets,
        percent,
        open,
        weight: WEEKLY_POOL_WEIGHT,
    }
}

#[test]
fn weekly_levels_returns_none_without_an_estimate() {
    assert_eq!(weekly_levels(&period(0, 3_600, None), 3_600), None);
}

#[test]
fn weekly_levels_groups_into_hourly_steps_from_a_zero_start() {
    let mut p = period(0, 2 * 3_600, Some(30.0));
    p.contributions = vec![contribution(0, 20.0), contribution(1_800, 5.0)];
    p.unattributed_buckets = vec![bucket(3_600, 5.0)];
    let levels = weekly_levels(&p, 2 * 3_600).expect("has an estimate");
    assert_eq!(
        levels,
        vec![
            LevelPoint {
                at_epoch: 0,
                percent: 0.0
            },
            LevelPoint {
                at_epoch: 3_600,
                percent: 25.0
            },
            LevelPoint {
                at_epoch: 7_200,
                percent: 30.0
            },
        ]
    );
    // A closed period: the last point equals `estimated_percent`, because
    // both sum the same contribution, unattributed, and unexplained
    // buckets, and a closed period's buckets already stay under 100.
    assert_eq!(levels.last().unwrap().percent, p.estimated_percent.unwrap());
}

#[test]
fn weekly_levels_caps_each_point_at_100() {
    let mut p = period(0, 3_600, Some(150.0));
    p.contributions = vec![contribution(0, 150.0)];
    let levels = weekly_levels(&p, 3_600).expect("has an estimate");
    assert_eq!(levels.last().unwrap().percent, 100.0);
}

#[test]
fn weekly_levels_clips_an_open_window_at_now() {
    let mut p = period(0, 3 * 3_600, Some(70.0));
    p.contributions = vec![contribution(0, 20.0), contribution(2 * 3_600, 50.0)];
    // now falls inside the window, before its reset: the tail bucket past
    // now must not appear.
    let levels = weekly_levels(&p, 3_600).expect("has an estimate");
    assert_eq!(
        levels,
        vec![
            LevelPoint {
                at_epoch: 0,
                percent: 0.0
            },
            LevelPoint {
                at_epoch: 3_600,
                percent: 20.0
            },
        ]
    );
}

#[test]
fn window_peak_caps_at_100_and_passes_through_none() {
    assert_eq!(window_peak(&period(0, 3_600, Some(150.0))), Some(100.0));
    assert_eq!(window_peak(&period(0, 3_600, Some(42.0))), Some(42.0));
    assert_eq!(window_peak(&period(0, 3_600, None)), None);
}

#[test]
fn pooled_at_bounds_a_closed_window_to_the_trailing_28_day_span() {
    let now = 100 * DAY;
    let t = now;
    // Exactly 28 days before t: the span is `(t - 28d, t]`, so this close is
    // outside it by one second.
    assert!(!pooled_at(t - POOL_SPAN_SECS, false, t, now));
    assert!(pooled_at(t - POOL_SPAN_SECS + 1, false, t, now));
    assert!(pooled_at(t, false, t, now));
    // A window that has not closed by t is not yet in the pool.
    assert!(!pooled_at(t + 1, false, t, now));
}

#[test]
fn pooled_at_counts_an_open_window_only_at_now() {
    let now = 100 * DAY;
    let earlier = now - DAY;
    // An open window's close sits after `now`, so it can never satisfy the
    // closed-in-span rule; it counts only when `t` is `now` itself.
    assert!(!pooled_at(now + DAY, true, earlier, now));
    assert!(pooled_at(now + DAY, true, now, now));
}

#[test]
fn rolling_utilization_of_one_window_equals_its_own_value() {
    let windows = [window(0, 10 * DAY, 42.0, false)];
    let points = rolling_utilization(&windows, 10 * DAY, 0);
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].at_epoch, 10 * DAY);
    assert_eq!(points[0].percent, Some(42.0));
}

#[test]
fn rolling_utilization_of_ten_windows_is_the_nearest_rank_value() {
    let reset = 8 * DAY;
    let windows: Vec<PoolWindow> = (1..=10)
        .map(|n| window(0, reset, n as f64 * 10.0, false))
        .collect();
    let points = rolling_utilization(&windows, reset, 0);
    assert_eq!(points.len(), 1);
    // Nearest rank at the 50th percentile of ten sorted values [10..100]:
    // index = ceil(0.5 * 10) - 1 = 4, the 5th smallest.
    assert_eq!(points[0].percent, Some(50.0));
}

#[test]
fn rolling_utilization_of_eight_windows_is_the_fourth_smallest() {
    // n=8 -> index = ceil(0.5 * 8) - 1 = 3, the 4th smallest.
    let reset = 8 * DAY;
    let windows: Vec<PoolWindow> = (1..=8)
        .map(|n| window(0, reset, n as f64 * 10.0, false))
        .collect();
    let points = rolling_utilization(&windows, reset, 0);
    assert_eq!(points.last().unwrap().percent, Some(40.0));
}

#[test]
fn rolling_utilization_more_than_half_maxed_reads_100() {
    let reset = 8 * DAY;
    let windows = [
        window(0, reset, 100.0, false),
        window(0, reset, 100.0, false),
        window(0, reset, 10.0, false),
    ];
    let points = rolling_utilization(&windows, reset, 0);
    // n=3 -> index = ceil(0.5 * 3) - 1 = 1, the 2nd smallest: 100.
    assert_eq!(points.last().unwrap().percent, Some(100.0));
}

#[test]
fn rolling_utilization_of_one_period_equals_the_peak() {
    let reset = 8 * DAY;
    let windows = [window(0, reset, 63.0, false)];
    let points = rolling_utilization(&windows, reset, 0);
    assert_eq!(points.last().unwrap().percent, Some(63.0));
}

#[test]
fn rolling_utilization_emits_only_when_the_pooled_value_changes() {
    let windows = [
        window(0, 8 * DAY, 30.0, false),
        window(0, 9 * DAY, 10.0, false),
    ];
    let points = rolling_utilization(&windows, 9 * DAY, 0);
    // The 30-only pool at day 8 (n=1, median 30) and the 10-and-30 pool at
    // day 9 (n=2 -> index = ceil(0.5 * 2) - 1 = 0, the 1st smallest: 10)
    // give two different median values, so both evaluation times emit a
    // point.
    assert_eq!(
        points,
        vec![
            RollingPoint {
                at_epoch: 8 * DAY,
                percent: Some(30.0)
            },
            RollingPoint {
                at_epoch: 9 * DAY,
                percent: Some(10.0)
            },
        ]
    );
}

#[test]
fn rolling_utilization_drops_a_window_when_it_leaves_the_trailing_span() {
    // The lower-valued window closes at day 8 and leaves the 28-day span at
    // day 36. No window closes at day 36, so only the leave time can move
    // the line. While both windows are pooled, nearest rank at n=2 always
    // reads the smaller of the two (index 0), so dropping the smaller one
    // raises the figure to the higher window's own value.
    let windows = [
        window(0, 8 * DAY, 30.0, false),
        window(0, 20 * DAY, 90.0, false),
    ];
    let points = rolling_utilization(&windows, 40 * DAY, 0);
    assert!(points.contains(&RollingPoint {
        at_epoch: 36 * DAY,
        percent: Some(90.0)
    }));
}

#[test]
fn rolling_utilization_starts_no_earlier_than_seven_days_of_window_history() {
    // Every window is younger than seven days at `now`, so the line has not
    // reached its start yet.
    let windows = [window(0, 3 * DAY, 40.0, false)];
    assert_eq!(rolling_utilization(&windows, 3 * DAY, 0), Vec::new());
}

#[test]
fn rolling_utilization_pools_an_open_window_only_at_the_final_point() {
    let now = 8 * DAY;
    let windows = [
        window(0, now, 77.0, false),
        // Still open at `now`: its close sits well past it.
        window(DAY, 100 * DAY, 20.0, true),
    ];
    let points = rolling_utilization(&windows, now, 0);
    // With the closed window alone the pool holds one value, 77. The final
    // point also pools the open window at its present level, so the sorted
    // pool is [20, 77]; n=2 -> index = ceil(0.5 * 2) - 1 = 0, the 1st
    // smallest: 20. Only the open window's inclusion can move the figure
    // there.
    assert_eq!(points.last().unwrap().percent, Some(20.0));
}

#[test]
fn rolling_utilization_is_empty_with_no_windows() {
    assert_eq!(rolling_utilization(&[], 10 * DAY, 0), Vec::new());
}

#[test]
fn rolling_utilization_counts_a_weekly_window_by_its_pool_weight() {
    // Three 5-hour windows at 10, 20, 30 and one weekly window at 90.
    // Counted once each, the sorted pool [10, 20, 30, 90] has n=4 ->
    // index = ceil(0.5 * 4) - 1 = 1: 20. At WEEKLY_POOL_WEIGHT (5), the
    // weekly window's 90 appears five times: [10, 20, 30, 90, 90, 90, 90,
    // 90] has n=8 -> index = ceil(0.5 * 8) - 1 = 3: 90.
    let reset = 8 * DAY;
    let windows = [
        window(0, reset, 10.0, false),
        window(0, reset, 20.0, false),
        window(0, reset, 30.0, false),
        weekly(0, reset, 90.0, false),
    ];
    let points = rolling_utilization(&windows, reset, 0);
    assert_eq!(points.last().unwrap().percent, Some(90.0));

    // The same four windows, but the weekly one counted once instead of
    // WEEKLY_POOL_WEIGHT times: the pool falls back to [10, 20, 30, 90] and
    // the figure falls back to 20 — proof the weight, not the value, moves
    // the result above.
    let unweighted = [
        window(0, reset, 10.0, false),
        window(0, reset, 20.0, false),
        window(0, reset, 30.0, false),
        window(0, reset, 90.0, false),
    ];
    let unweighted_points = rolling_utilization(&unweighted, reset, 0);
    assert_eq!(unweighted_points.last().unwrap().percent, Some(20.0));
}

#[test]
fn rolling_utilization_marks_expired_history_and_resumes_after_a_gap() {
    let windows = [window(0, 8 * DAY, 30.0, false)];
    let points = rolling_utilization(&windows, 40 * DAY, 0);
    assert_eq!(
        points.last(),
        Some(&RollingPoint {
            at_epoch: 36 * DAY,
            percent: None
        })
    );

    let windows = [windows[0], window(40 * DAY, 41 * DAY, 30.0, false)];
    let points = rolling_utilization(&windows, 42 * DAY, 0);
    assert!(points.contains(&RollingPoint {
        at_epoch: 36 * DAY,
        percent: None
    }));
    assert!(points.contains(&RollingPoint {
        at_epoch: 41 * DAY,
        percent: Some(30.0)
    }));
    assert_eq!(points.last().unwrap().percent, Some(30.0));
}
