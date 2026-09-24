//! Allocate provider meter changes to usage at its recorded timestamps.
//! Chart buckets group the output only. They do not distribute usage in time.

use crate::provider_usage::quota::factor_point_at_or_earliest;
use crate::store::provider_limit::FactorPoint;

const MIN_SHARE_SEGMENT_PERCENT: f64 = 2.0;
const MAX_SHARE_SEGMENT_SECS: i64 = 3 * 3_600;

pub struct ShareInput<'a> {
    pub start: i64,
    pub reset: i64,
    /// Authoritative readings in ascending time order, inside the period.
    pub readings: &'a [(i64, f64)],
    /// Recorded `(timestamp_ms, usd)` usage, grouped by chart bucket and session.
    pub buckets: &'a [&'a [(i64, f64)]],
    pub points: &'a [FactorPoint],
}

struct Segment {
    to_ms: i64,
    rise: f64,
}

pub struct SharedPeriod {
    /// Parallel to `ShareInput::buckets`. None means no priced contribution.
    pub bucket_percent: Vec<Option<f64>>,
    pub unexplained: Vec<(i64, f64)>,
    pub coverage_until: Option<i64>,
    pub meter_regressions: u32,
}

fn close_segment(from_pct: f64, to_t: i64, to_pct: f64, regressions: &mut u32) -> Segment {
    let rise = to_pct - from_pct;
    if rise < 0.0 {
        *regressions += 1;
    }
    Segment {
        to_ms: to_t.saturating_mul(1_000),
        rise: rise.max(0.0),
    }
}

pub fn share_period(input: &ShareInput<'_>) -> SharedPeriod {
    let mut segments = Vec::new();
    let mut meter_regressions = 0;
    if let Some(&(first_t, first_pct)) = input.readings.first() {
        segments.push(close_segment(
            0.0,
            first_t,
            first_pct,
            &mut meter_regressions,
        ));
        let mut from = (first_t, first_pct);
        for &(t, pct) in &input.readings[1..] {
            if pct - from.1 >= MIN_SHARE_SEGMENT_PERCENT || t - from.0 >= MAX_SHARE_SEGMENT_SECS {
                segments.push(close_segment(from.1, t, pct, &mut meter_regressions));
                from = (t, pct);
            }
        }
        let &(t, pct) = input.readings.last().expect("a first reading exists");
        if from.0 != t {
            segments.push(close_segment(from.1, t, pct, &mut meter_regressions));
        }
    }

    let in_period = |ts_ms: i64| {
        ts_ms >= input.start.saturating_mul(1_000) && ts_ms < input.reset.saturating_mul(1_000)
    };
    // A turn at a reading's timestamp belongs to that reading, not the tail.
    let segment_index = |ts_ms| segments.partition_point(|segment| segment.to_ms < ts_ms);
    let mut segment_usd = vec![0.0; segments.len()];
    for &bucket in input.buckets {
        for &(ts_ms, usd) in bucket {
            if in_period(ts_ms)
                && let Some(total) = segment_usd.get_mut(segment_index(ts_ms))
            {
                *total += usd;
            }
        }
    }

    // A downward correction must reduce the allocated total as well.
    let measured_total: f64 = segments.iter().map(|segment| segment.rise).sum();
    let last_percent = input.readings.last().map_or(0.0, |&(_, pct)| pct.max(0.0));
    let scale = if measured_total > last_percent {
        last_percent / measured_total
    } else {
        1.0
    };
    let unexplained = segments
        .iter()
        .zip(&segment_usd)
        .filter(|(segment, usd)| segment.rise > 0.0 && **usd == 0.0)
        .map(|(segment, _)| (segment.to_ms / 1_000, segment.rise * scale))
        .collect();
    let bucket_percent = input
        .buckets
        .iter()
        .map(|bucket| {
            let mut total = 0.0;
            let mut priced = false;
            for &(ts_ms, usd) in *bucket {
                if !in_period(ts_ms) || usd <= 0.0 {
                    continue;
                }
                let index = segment_index(ts_ms);
                if let Some(segment) = segments.get(index) {
                    total += segment.rise * scale * usd / segment_usd[index];
                    priced = true;
                } else if let Some(point) = factor_point_at_or_earliest(input.points, ts_ms / 1_000)
                {
                    total += usd / point.usd_per_percent;
                    priced = true;
                }
            }
            priced.then_some(total)
        })
        .collect();
    SharedPeriod {
        bucket_percent,
        unexplained,
        coverage_until: input.readings.last().map(|&(t, _)| t),
        meter_regressions,
    }
}

/// Close an overshooting historical estimate at 100 percent. Keep open estimates.
pub fn share_period_capped(input: &ShareInput<'_>, now: i64) -> SharedPeriod {
    let uncapped = share_period(input);
    let total: f64 = uncapped.bucket_percent.iter().flatten().sum::<f64>()
        + uncapped
            .unexplained
            .iter()
            .map(|&(_, percent)| percent)
            .sum::<f64>();
    if input.reset > now || total <= 100.0 {
        return uncapped;
    }

    // The real readings are all inside `[start, reset)` by this module's own
    // contract, so appending a synthetic reading of 100 at `reset` keeps the
    // vector sorted ascending: it becomes the new last reading, and the
    // sharing pass below treats it exactly like an authoritative one. With
    // no real readings at all this yields one segment for the whole window,
    // `[start, reset)`, at a rise of 100: every bucket's percent becomes
    // `100 * usd / total_usd`, the "scale the whole stack to 100" case.
    let mut capped_readings: Vec<(i64, f64)> = input.readings.to_vec();
    capped_readings.push((input.reset, 100.0));
    let capped_input = ShareInput {
        start: input.start,
        reset: input.reset,
        readings: &capped_readings,
        buckets: input.buckets,
        points: input.points,
    };
    let capped = share_period(&capped_input);

    SharedPeriod {
        coverage_until: uncapped.coverage_until,
        meter_regressions: uncapped.meter_regressions,
        ..capped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(at: i64, usd_per_percent: f64) -> FactorPoint {
        FactorPoint {
            id: 0,
            provider: "anthropic".into(),
            account_key: "a".repeat(64),
            lane: "fiveHour".into(),
            effective_at_epoch: at,
            usd_per_percent,
            method: "delta".into(),
            sample_count: 1,
            plan: None,
            plan_tier: None,
        }
    }

    fn total(shared: &SharedPeriod) -> f64 {
        shared.bucket_percent.iter().flatten().sum::<f64>()
            + shared.unexplained.iter().map(|&(_, p)| p).sum::<f64>()
    }

    #[test]
    fn reading_inside_bucket_does_not_add_an_estimated_future_tail() {
        let input = ShareInput {
            start: 0,
            reset: 18_000,
            readings: &[(289, 92.0)],
            buckets: &[&[(100_000, 5.125)]],
            points: &[point(0, 1.098)],
        };
        let shared = share_period(&input);
        assert_eq!(shared.bucket_percent, vec![Some(92.0)]);
        assert_eq!(shared.coverage_until, Some(289));
        assert!(shared.unexplained.is_empty());
    }

    #[test]
    fn only_actual_usage_after_reading_adds_an_estimate() {
        let input = ShareInput {
            start: 0,
            reset: 18_000,
            readings: &[(289, 92.0)],
            buckets: &[&[(100_000, 5.0), (300_000, 0.5)]],
            points: &[point(0, 1.0)],
        };
        assert_eq!(share_period(&input).bucket_percent, vec![Some(92.5)]);
        let updated = ShareInput {
            readings: &[(289, 92.0), (310, 93.0)],
            ..input
        };
        assert_eq!(share_period(&updated).bucket_percent, vec![Some(93.0)]);
    }

    #[test]
    fn same_bucket_sessions_share_only_segments_their_turns_belong_to() {
        let input = ShareInput {
            start: 0,
            reset: 18_000,
            readings: &[(200, 20.0), (400, 30.0)],
            buckets: &[&[(100_000, 3.0)], &[(150_000, 1.0), (300_000, 9.0)]],
            points: &[],
        };
        let shared = share_period(&input);
        assert_eq!(shared.bucket_percent, vec![Some(15.0), Some(15.0)]);
        assert_eq!(total(&shared), 30.0);
    }

    #[test]
    fn millisecond_boundaries_assign_exact_reading_time_to_measured_usage() {
        let input = ShareInput {
            start: 100,
            reset: 900,
            readings: &[(200, 20.0)],
            buckets: &[
                &[(99_999, 100.0), (100_000, 1.0), (200_000, 1.0)],
                &[(200_001, 3.0), (900_000, 100.0)],
            ],
            points: &[point(0, 1.0)],
        };
        assert_eq!(
            share_period(&input).bucket_percent,
            vec![Some(20.0), Some(3.0)]
        );
    }

    #[test]
    fn factor_is_selected_at_usage_time_not_bucket_end() {
        let input = ShareInput {
            start: 0,
            reset: 900,
            readings: &[],
            buckets: &[&[(100_000, 4.0), (500_000, 4.0)]],
            points: &[point(0, 1.0), point(400, 2.0), point(900, 100.0)],
        };
        assert_eq!(share_period(&input).bucket_percent, vec![Some(6.0)]);
    }

    #[test]
    fn missing_factor_keeps_the_shared_part_and_leaves_tail_only_unknown() {
        let input = ShareInput {
            start: 0,
            reset: 900,
            readings: &[(200, 20.0)],
            buckets: &[&[(100_000, 1.0), (300_000, 1.0)], &[(400_000, 1.0)]],
            points: &[],
        };
        assert_eq!(share_period(&input).bucket_percent, vec![Some(20.0), None]);
        let no_readings = ShareInput {
            readings: &[],
            ..input
        };
        assert_eq!(share_period(&no_readings).bucket_percent, vec![None, None]);
    }

    #[test]
    fn meter_rise_without_local_usage_stays_unexplained() {
        let input = ShareInput {
            start: 0,
            reset: 900,
            readings: &[(200, 20.0), (400, 30.0)],
            buckets: &[&[(300_000, 1.0)]],
            points: &[],
        };
        let shared = share_period(&input);
        assert_eq!(shared.bucket_percent, vec![Some(10.0)]);
        assert_eq!(shared.unexplained, vec![(200, 20.0)]);
        assert_eq!(total(&shared), 30.0);
    }

    #[test]
    fn small_meter_ticks_merge_before_allocation() {
        let input = ShareInput {
            start: 0,
            reset: 900,
            readings: &[(0, 0.0), (100, 1.0), (200, 2.0)],
            buckets: &[&[(50_000, 1.0)], &[(150_000, 3.0)]],
            points: &[],
        };
        assert_eq!(
            share_period(&input).bucket_percent,
            vec![Some(0.5), Some(1.5)]
        );
    }

    #[test]
    fn downward_meter_correction_reconciles_allocations_and_unexplained_usage() {
        let input = ShareInput {
            start: 0,
            reset: 18_000,
            readings: &[(100, 40.0), (200, 80.0), (300, 60.0)],
            buckets: &[&[(150_000, 1.0)], &[(350_000, 2.0)]],
            points: &[point(0, 1.0)],
        };
        let shared = share_period(&input);
        assert_eq!(shared.bucket_percent, vec![Some(30.0), Some(2.0)]);
        assert_eq!(shared.unexplained, vec![(100, 30.0)]);
        assert_eq!(total(&shared), 62.0);
        assert_eq!(shared.meter_regressions, 1);
    }

    #[test]
    fn zero_meter_produces_zero_contribution() {
        let input = ShareInput {
            start: 0,
            reset: 900,
            readings: &[(200, 0.0)],
            buckets: &[&[(100_000, 1.0)]],
            points: &[],
        };
        assert_eq!(share_period(&input).bucket_percent, vec![Some(0.0)]);
    }

    #[test]
    fn closed_period_caps_tail_but_keeps_real_coverage() {
        let input = ShareInput {
            start: 0,
            reset: 900,
            readings: &[(200, 80.0)],
            buckets: &[&[(100_000, 1.0)], &[(300_000, 50.0)]],
            points: &[point(0, 1.0)],
        };
        let open = share_period_capped(&input, 800);
        assert_eq!(total(&open), 130.0);
        let closed = share_period_capped(&input, 900);
        assert_eq!(closed.bucket_percent, vec![Some(80.0), Some(20.0)]);
        assert_eq!(closed.coverage_until, Some(200));
    }

    #[test]
    fn closed_period_without_readings_scales_by_actual_spend() {
        let input = ShareInput {
            start: 120,
            reset: 1_020,
            readings: &[],
            buckets: &[&[(150_000, 90.0)], &[(1_010_000, 30.0)]],
            points: &[point(0, 1.0)],
        };
        let closed = share_period_capped(&input, 1_020);
        assert_eq!(closed.bucket_percent, vec![Some(75.0), Some(25.0)]);
        assert_eq!(closed.coverage_until, None);
    }
}
