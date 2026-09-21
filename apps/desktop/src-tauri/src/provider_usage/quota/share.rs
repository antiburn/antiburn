//! Share a quota period's meter rise across the priced buckets inside it.
//!
//! The provider's meter states the truth at each reading, but readings land
//! minutes to hours apart. Between two readings, this module assumes the
//! meter's own rise split across whatever local spend fell inside that span,
//! in proportion to each bucket's own dollars: the "shared" regime. A bucket
//! after the last reading, or every bucket in a period with no reading at
//! all, keeps the older rule instead: price it from the learned
//! dollars-per-percent factor, the "estimated" regime. See
//! `spec-quota-shared-meter.md` for the product rule this encodes.
//!
//! [`share_period`] is pure: it takes already-loaded readings, bucket
//! dollars, and factor points, and returns a percent per bucket, so it needs
//! no store access and stays unit-testable on its own.
//!
//! Turn buckets sit on absolute 15-minute boundaries, but a period starts
//! and resets at any minute, so a bucket at either edge of the period only
//! partly overlaps it. A bucket's dollars come only from turns inside the
//! period, so this module spreads them over that overlap alone, not over
//! the bucket's whole 15 minutes: see [`bucket_span_in_period`].

use crate::provider_usage::quota::factor_point_at_or_earliest;
use crate::store::provider_limit::{CONTRIBUTION_BUCKET_SECS, FactorPoint};

/// A segment closes once the meter has risen this many points since its own
/// start. Codex reports integer percent, so a single one-point tick is
/// noise; merging until the meter has moved this far stops one session
/// being credited with a neighbour's tick.
const MIN_SHARE_SEGMENT_PERCENT: f64 = 2.0;

/// A segment closes once it spans this long, whatever the meter did.
const MAX_SHARE_SEGMENT_SECS: i64 = 3 * 3_600;

/// One quota period's inputs for sharing its meter rise across its buckets.
pub struct ShareInput<'a> {
    pub start: i64,
    pub reset: i64,
    /// Authoritative readings with a known percent, inside `[start, reset)`,
    /// sorted ascending by time.
    pub readings: &'a [(i64, f64)],
    /// Every bucket's own dollars inside the period: `(bucket_start_epoch,
    /// usd)`, bound and unbound alike, one entry per session per bucket.
    pub buckets: &'a [(i64, f64)],
    /// The lane's factor points, ordered by `effective_at_epoch`, for the
    /// estimated regime.
    pub points: &'a [FactorPoint],
}

/// One shared segment: the meter's own rise across `[from_t, to_t)`, already
/// clamped to zero when the reading that closed it read lower than the one
/// that opened it.
struct Segment {
    from_t: i64,
    to_t: i64,
    rise: f64,
}

/// The result of sharing one period's meter rise across its buckets.
pub struct SharedPeriod {
    /// Parallel to [`ShareInput::buckets`]: this bucket's shared or
    /// estimated percent. `None` only for a bucket in the tail with no
    /// factor point to price it from.
    pub bucket_percent: Vec<Option<f64>>,
    /// One entry per segment whose rise had no dollars to share it across:
    /// `(to_t, percent)`, the whole rise credited to nothing local.
    pub unexplained: Vec<(i64, f64)>,
    /// The last reading's own time, the point past which buckets fall into
    /// the estimated regime. `None` when the period carries no reading.
    pub coverage_until: Option<i64>,
    /// How many segments closed on a reading lower than the one that opened
    /// them: the meter regressed, so their rise clamped to zero rather than
    /// going negative.
    pub meter_regressions: u32,
}

/// The part of a bucket's own 15 minutes that lies inside the period,
/// `[from, to)`, or `None` when the bucket is entirely outside it. Turn
/// buckets sit on absolute 15-minute boundaries and a period starts at any
/// minute, so a bucket at either edge of the period only partly belongs to
/// it; its dollars all come from turns inside the period, so they spread
/// over this part alone.
fn bucket_span_in_period(input: &ShareInput<'_>, bucket_start: i64) -> Option<(i64, i64)> {
    let from = bucket_start.max(input.start);
    let to = (bucket_start + CONTRIBUTION_BUCKET_SECS).min(input.reset);
    (to > from).then_some((from, to))
}

/// A bucket's percent under the estimated regime: its dollars divided by the
/// factor point in effect at the last second before `at`, the clipped end
/// of the bucket's span from `bucket_span_in_period`, or `None` with no
/// factor point yet. A period is `[start, reset)`, so a factor point that
/// takes effect exactly at the reset belongs to the next window and must
/// not price this one.
fn estimated_percent(points: &[FactorPoint], at: i64, usd: f64) -> Option<f64> {
    factor_point_at_or_earliest(points, at - 1).map(|point| usd / point.usd_per_percent)
}

/// Close one segment `[from_t, to_t)` from its boundary readings' percents.
/// A reading lower than the one that opened its segment is a meter
/// regression: count it and clamp the segment's rise to zero rather than
/// let it go negative.
fn close_segment(
    from_t: i64,
    from_pct: f64,
    to_t: i64,
    to_pct: f64,
    meter_regressions: &mut u32,
) -> Segment {
    let raw = to_pct - from_pct;
    if raw < 0.0 {
        *meter_regressions += 1;
    }
    Segment {
        from_t,
        to_t,
        rise: raw.max(0.0),
    }
}

/// Seconds that a bucket's own span inside the period, `[from, to)`, shares
/// with `[from_t, to_t)`. Zero when the two spans do not overlap. Codex
/// writes a reading on every turn, so a segment can close in a few minutes,
/// shorter than a bucket; a bucket must split across every segment it
/// touches, not just the one holding its start, or dollars pile onto one
/// segment while its neighbours wrongly show a rise with nothing behind it.
fn overlap_secs(from: i64, to: i64, from_t: i64, to_t: i64) -> i64 {
    (to.min(to_t) - from.max(from_t)).max(0)
}

/// Share one quota period's meter rise across its buckets. See the module
/// doc comment for the shared-versus-estimated split this implements.
pub fn share_period(input: &ShareInput<'_>) -> SharedPeriod {
    let readings = input.readings;
    if readings.is_empty() {
        let bucket_percent = input
            .buckets
            .iter()
            .map(|&(bucket_start, usd)| {
                bucket_span_in_period(input, bucket_start)
                    .and_then(|(_, to)| estimated_percent(input.points, to, usd))
            })
            .collect();
        return SharedPeriod {
            bucket_percent,
            unexplained: Vec::new(),
            coverage_until: None,
            meter_regressions: 0,
        };
    }

    let mut segments: Vec<Segment> = Vec::new();
    let mut meter_regressions: u32 = 0;

    // The first segment always starts at the window start with a level of
    // zero percent and runs to the first reading.
    let (first_t, first_pct) = readings[0];
    segments.push(close_segment(
        input.start,
        0.0,
        first_t,
        first_pct,
        &mut meter_regressions,
    ));

    let mut seg_start = readings[0];
    for &(t, pct) in &readings[1..] {
        let moved_enough = pct - seg_start.1 >= MIN_SHARE_SEGMENT_PERCENT;
        let waited_long_enough = t - seg_start.0 >= MAX_SHARE_SEGMENT_SECS;
        if moved_enough || waited_long_enough {
            segments.push(close_segment(
                seg_start.0,
                seg_start.1,
                t,
                pct,
                &mut meter_regressions,
            ));
            seg_start = (t, pct);
        }
    }
    // The last reading always closes a final segment, whatever its size.
    let last = *readings.last().expect("checked non-empty above");
    if seg_start.0 != last.0 {
        segments.push(close_segment(
            seg_start.0,
            seg_start.1,
            last.0,
            last.1,
            &mut meter_regressions,
        ));
    }

    // Spread each bucket's dollars evenly across the part of its own 15
    // minutes that lies inside the period: a bucket that straddles a
    // segment boundary feeds each side in proportion to the time it spends
    // there, not only the segment holding its start. A bucket outside the
    // period contributes nothing, as today.
    let mut segment_usd = vec![0.0; segments.len()];
    for &(bucket_start, usd) in input.buckets {
        let Some((from, to)) = bucket_span_in_period(input, bucket_start) else {
            continue;
        };
        let span_secs = (to - from) as f64;
        for (segment, seg_usd) in segments.iter().zip(segment_usd.iter_mut()) {
            let overlap = overlap_secs(from, to, segment.from_t, segment.to_t);
            if overlap > 0 {
                *seg_usd += usd * (overlap as f64 / span_secs);
            }
        }
    }

    let unexplained: Vec<(i64, f64)> = segments
        .iter()
        .zip(&segment_usd)
        .filter(|&(segment, usd)| segment.rise > 0.0 && *usd == 0.0)
        // The rise is known only at the reading that ends the segment, so
        // stamp it there; the chart ramps up to it instead of jumping in
        // at the segment's start.
        .map(|(segment, _)| (segment.to_t, segment.rise))
        .collect();

    // Past the last reading, a bucket leaves the shared regime for the
    // estimated one; `coverage_until` is where that split falls.
    let coverage_until = last.0;

    let bucket_percent = input
        .buckets
        .iter()
        .map(|&(bucket_start, usd)| {
            let (from, to) = bucket_span_in_period(input, bucket_start)?;
            let span_secs = (to - from) as f64;
            // Sum this bucket's share of every segment it overlaps, and
            // track how much of its own in-period span fell in the shared
            // regime at all (a zero-dollar segment still counts as
            // covered: its rise is unexplained, not this bucket's fault).
            let mut shared_percent = 0.0;
            let mut shared_secs: i64 = 0;
            for (segment, seg_usd) in segments.iter().zip(&segment_usd) {
                let overlap = overlap_secs(from, to, segment.from_t, segment.to_t);
                if overlap == 0 {
                    continue;
                }
                shared_secs += overlap;
                if *seg_usd > 0.0 {
                    let frac = overlap as f64 / span_secs;
                    shared_percent += segment.rise * usd * frac / seg_usd;
                }
            }
            // The rest of the bucket's own in-period span, if any, lies at
            // or past the last reading: price that part from the factor.
            let tail_secs = (to - from.max(coverage_until)).max(0);
            if tail_secs == 0 {
                return Some(shared_percent);
            }
            let tail_frac = tail_secs as f64 / span_secs;
            match estimated_percent(input.points, to, usd * tail_frac) {
                Some(tail_percent) => Some(shared_percent + tail_percent),
                // No factor for the tail part: keep the shared part alone
                // when the bucket had one, else stay unpriced as today.
                None if shared_secs > 0 => Some(shared_percent),
                None => None,
            }
        })
        .collect();

    SharedPeriod {
        bucket_percent,
        unexplained,
        coverage_until: Some(coverage_until),
        meter_regressions,
    }
}

/// Like [`share_period`], but a closed window (`reset <= now`) whose stack
/// would end above 100 percent closes on a synthetic reading of 100 at its
/// reset, so the estimated tail shares `100 - last reading` among its
/// buckets by dollars instead of overshooting a value the meter cannot
/// reach. An open window is never capped: its factor-priced tail can still
/// move before the window closes, so this function returns `share_period`'s
/// own result unchanged.
///
/// A meter regression clamps its segment's rise to zero instead of going
/// negative. That clamp can double-count the real rise across the segments
/// on either side of the regression, so even after the synthetic reading the
/// capped stack can still exceed 100. As a last step, this function scales
/// the whole capped stack down to 100 whenever that happens.
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

    // A regression's clamp can double-count the real rise, so even the
    // capped stack can still run past 100. Scale every priced bucket and
    // every unexplained entry down to a stack of exactly 100 when that
    // happens; each one keeps its own share of the total.
    let capped_total: f64 = capped.bucket_percent.iter().flatten().sum::<f64>()
        + capped
            .unexplained
            .iter()
            .map(|&(_, percent)| percent)
            .sum::<f64>();
    let (bucket_percent, unexplained) = if capped_total > 100.0 {
        let scale = 100.0 / capped_total;
        (
            capped
                .bucket_percent
                .into_iter()
                .map(|percent| percent.map(|value| value * scale))
                .collect(),
            capped
                .unexplained
                .into_iter()
                .map(|(t, percent)| (t, percent * scale))
                .collect(),
        )
    } else {
        (capped.bucket_percent, capped.unexplained)
    };

    // `coverage_until` and `meter_regressions` describe the real meter, not
    // the synthetic close, so keep the uncapped run's own values: the
    // synthetic reading can never itself regress (100 is the meter's
    // maximum), but state the uncapped count explicitly rather than assume
    // it.
    SharedPeriod {
        bucket_percent,
        unexplained,
        coverage_until: uncapped.coverage_until,
        meter_regressions: uncapped.meter_regressions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// True when two percents match to within floating-point noise. The
    /// time-weighted split divides by sums with no round number, so a test
    /// bucket that straddles a segment boundary yields a percent with no
    /// exact decimal form.
    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn point(usd_per_percent: f64) -> FactorPoint {
        FactorPoint {
            id: 0,
            provider: "anthropic".to_string(),
            account_key: "a".repeat(64),
            lane: "fiveHour".to_string(),
            effective_at_epoch: 0,
            usd_per_percent,
            method: "delta".to_string(),
            sample_count: 1,
            plan: None,
            plan_tier: None,
        }
    }

    /// With no readings at all, every bucket prices by the factor, and the
    /// period carries no meter coverage.
    #[test]
    fn no_readings_prices_every_bucket_by_the_factor() {
        let points = [point(2.0)];
        let input = ShareInput {
            start: 0,
            reset: 100_000,
            readings: &[],
            buckets: &[(0, 10.0), (900, 5.0)],
            points: &points,
        };
        let shared = share_period(&input);
        assert_eq!(shared.bucket_percent, vec![Some(5.0), Some(2.5)]);
        assert_eq!(shared.coverage_until, None);
        assert_eq!(shared.meter_regressions, 0);
        assert!(shared.unexplained.is_empty());
    }

    /// A closed period with no readings and a reset at 5,100, off the
    /// 15-minute grid: the last absolute bucket, 4,500 to 5,400, only 600 of
    /// whose seconds belong to this period. A second factor point takes
    /// effect exactly at the reset — the next window's own factor, seeded
    /// right at its start. Pricing must stop short of the reset, or the
    /// tail bucket wrongly borrows the next window's much bigger factor
    /// (0.9 instead of 90).
    #[test]
    fn estimated_tail_prices_at_the_period_own_end_not_the_next_window_factor() {
        let points = [
            point(1.0),
            FactorPoint {
                effective_at_epoch: 5_100,
                ..point(100.0)
            },
        ];
        let input = ShareInput {
            start: 0,
            reset: 5_100,
            readings: &[],
            buckets: &[(4_500, 90.0)],
            points: &points,
        };
        let shared = share_period(&input);
        assert_eq!(shared.bucket_percent, vec![Some(90.0)]);
    }

    /// Two readings, 0 to 10 to 20 percent, with two "sessions" each
    /// contributing dollars at a 30:70 ratio. Each bucket's own 15 minutes
    /// (CONTRIBUTION_BUCKET_SECS) is 900 seconds, so the bucket started at
    /// 500 and the one started at 1,500 each straddle a boundary (the
    /// reading at 1,000, and the last reading at 2,000 respectively): part
    /// of their dollars land in the neighbouring segment, or past the last
    /// reading, by the time they spend there. The two sessions' percents,
    /// summed across every bucket, still add to exactly the last reading's
    /// 20 percent.
    #[test]
    fn a_rise_splits_between_sessions_by_their_own_dollars_within_a_segment() {
        let points: [FactorPoint; 0] = [];
        let input = ShareInput {
            start: 0,
            reset: 5_000,
            readings: &[(1_000, 10.0), (2_000, 20.0)],
            buckets: &[(100, 30.0), (500, 70.0), (1_100, 30.0), (1_500, 70.0)],
            points: &points,
        };
        let shared = share_period(&input);
        let percents: Vec<f64> = shared.bucket_percent.iter().map(|p| p.unwrap()).collect();
        // Bucket at 100 sits wholly inside the first segment [0, 1,000).
        assert!(approx(percents[0], 4.354_838_709_677_42));
        // Bucket at 500 spans [500, 1,400): part first segment, part second.
        assert!(approx(percents[1], 8.756_272_401_433_69));
        // Bucket at 1,100 sits wholly inside the second segment [1,000, 2,000).
        assert_eq!(percents[2], 3.0);
        // Bucket at 1,500 spans [1,500, 2,400): part second segment, part
        // past the last reading, dropped there for lack of a factor point.
        assert!(approx(percents[3], 3.888_888_888_888_889));
        let session_a = percents[0] + percents[2];
        let session_b = percents[1] + percents[3];
        assert!(approx(session_a + session_b, 20.0));
        assert_eq!(shared.coverage_until, Some(2_000));
    }

    /// Readings at 1, 2, 3, 4 percent ten minutes apart merge into one
    /// segment, [600, 1,800), until the 3-percent reading (no single tick
    /// moves the meter the required 2 points); the final tick then closes
    /// its own segment, [1,800, 2,400). The bucket started at 1,300 spans
    /// [1,300, 2,200), straddling that boundary, so it feeds both segments
    /// by the time it spends in each. The earlier segment, [0, 600), holds
    /// no bucket at all, so its own 1-point rise goes unexplained, stamped
    /// at 600, the reading that closes that segment.
    #[test]
    fn integer_ticks_below_the_threshold_merge_into_one_segment() {
        let points: [FactorPoint; 0] = [];
        let input = ShareInput {
            start: 0,
            reset: 10_000,
            readings: &[(600, 1.0), (1_200, 2.0), (1_800, 3.0), (2_400, 4.0)],
            buckets: &[(700, 40.0), (1_300, 60.0)],
            points: &points,
        };
        let shared = share_period(&input);
        let percents: Vec<f64> = shared.bucket_percent.iter().map(|p| p.unwrap()).collect();
        // Bucket at 700 sits wholly inside [600, 1,800): the merged
        // segment's own 2-point rise is its only dollars, so it takes all
        // of it in proportion to its own share of that segment's usd.
        assert!(approx(percents[0], 12.0 / 11.0));
        // Bucket at 1,300 spans both [600, 1,800) and [1,800, 2,400).
        assert!(approx(percents[1], 21.0 / 11.0));
        assert_eq!(shared.unexplained, vec![(600, 1.0)]);
    }

    /// A segment whose rise has no dollars behind it emits one unexplained
    /// entry for the whole rise, at the segment's own end (the reading
    /// that closed it).
    #[test]
    fn a_rise_with_no_dollars_is_entirely_unexplained() {
        let points: [FactorPoint; 0] = [];
        let input = ShareInput {
            start: 0,
            reset: 5_000,
            readings: &[(1_000, 15.0)],
            buckets: &[],
            points: &points,
        };
        let shared = share_period(&input);
        assert_eq!(shared.unexplained, vec![(1_000, 15.0)]);
        assert_eq!(shared.coverage_until, Some(1_000));
    }

    /// A bucket after the last reading falls in the estimated regime and
    /// prices from the factor; `coverage_until` still names the last
    /// reading.
    #[test]
    fn a_bucket_after_the_last_reading_is_priced_by_the_factor() {
        let points = [point(5.0)];
        let input = ShareInput {
            start: 0,
            reset: 5_000,
            readings: &[(1_000, 10.0)],
            buckets: &[(2_000, 25.0)],
            points: &points,
        };
        let shared = share_period(&input);
        assert_eq!(shared.bucket_percent, vec![Some(5.0)]);
        assert_eq!(shared.coverage_until, Some(1_000));
    }

    /// A regression (10 -> 8 -> 15), each reading far enough apart in time
    /// to close its own segment regardless of the percent move: the middle
    /// segment's rise clamps to zero and counts as one regression, and its
    /// bucket never carries a negative percent.
    #[test]
    fn a_meter_regression_counts_once_and_never_goes_negative() {
        let points: [FactorPoint; 0] = [];
        let four_hours = 4 * 3_600;
        let r0 = (10_000, 10.0);
        let r1 = (r0.0 + four_hours, 8.0);
        let r2 = (r1.0 + four_hours, 15.0);
        let input = ShareInput {
            start: 0,
            reset: r2.0 + 1,
            readings: &[r0, r1, r2],
            buckets: &[(r0.0 + 5_000, 50.0)],
            points: &points,
        };
        let shared = share_period(&input);
        assert_eq!(shared.meter_regressions, 1);
        assert_eq!(shared.bucket_percent, vec![Some(0.0)]);
    }

    /// With every segment's rise fully explained by its own dollars, the
    /// sum of every bucket's percent equals the last reading's percent
    /// exactly: the stack matches the meter there.
    #[test]
    fn the_stack_at_the_last_reading_equals_the_meter() {
        let points: [FactorPoint; 0] = [];
        let input = ShareInput {
            start: 0,
            reset: 3_000,
            readings: &[(1_000, 12.0), (2_000, 20.0)],
            buckets: &[(500, 100.0), (1_500, 50.0)],
            points: &points,
        };
        let shared = share_period(&input);
        let bucket_sum: f64 = shared.bucket_percent.iter().flatten().sum();
        let unexplained_sum: f64 = shared.unexplained.iter().map(|&(_, percent)| percent).sum();
        assert!(shared.unexplained.is_empty());
        assert_eq!(bucket_sum + unexplained_sum, 20.0);
    }

    /// Four readings inside one bucket's own 15 minutes (Codex writes a
    /// reading on every turn, so a burst can close several segments faster
    /// than a bucket is wide): every segment those readings form still
    /// overlaps the bucket's whole window, so each one has dollars behind
    /// it, and the meter's whole rise splits between the two sessions by
    /// their own dollar ratio, not by which segment happens to hold the
    /// bucket's start.
    #[test]
    fn a_burst_of_readings_inside_one_bucket_splits_by_dollars_not_by_start() {
        let points: [FactorPoint; 0] = [];
        let input = ShareInput {
            start: 0,
            reset: 10_000,
            readings: &[(200, 1.0), (400, 2.0), (600, 3.0), (800, 4.0)],
            // Two sessions' dollars, both in the one bucket [0, 900).
            buckets: &[(0, 30.0), (0, 70.0)],
            points: &points,
        };
        let shared = share_period(&input);
        assert!(shared.unexplained.is_empty());
        let session_a = shared.bucket_percent[0].unwrap();
        let session_b = shared.bucket_percent[1].unwrap();
        assert!(approx(session_a, 1.2));
        assert!(approx(session_b, 2.8));
        assert!(approx(session_a + session_b, 4.0));
    }

    /// A bucket that straddles a segment boundary splits between the two
    /// segments by the time it spends in each, not only the one holding
    /// its own start: bucket B, started at 450, spans [450, 1,350) evenly
    /// across [0, 900) and [900, 1,800), so half its dollars weigh each
    /// segment. Under the old whole-bucket assignment, all of bucket B's
    /// dollars would fall in the first segment and the second segment's
    /// own rise would be wrongly unexplained; here it is not.
    #[test]
    fn a_straddling_bucket_splits_by_time_between_segments() {
        let points: [FactorPoint; 0] = [];
        let input = ShareInput {
            start: 0,
            reset: 5_000,
            readings: &[(900, 10.0), (1_800, 19.0)],
            buckets: &[(0, 100.0), (450, 90.0)],
            points: &points,
        };
        let shared = share_period(&input);
        assert!(shared.unexplained.is_empty());
        let bucket_a = shared.bucket_percent[0].unwrap();
        let bucket_b = shared.bucket_percent[1].unwrap();
        assert!(approx(bucket_a, 1_000.0 / 145.0));
        assert!(approx(bucket_b, 450.0 / 145.0 + 9.0));
        assert!(approx(bucket_a + bucket_b, 19.0));
    }

    /// A bucket whose own 15 minutes straddles the last reading is priced
    /// two ways at once: the part before the reading shares the segment's
    /// rise as usual, and the part at or after it prices from the factor,
    /// same as a bucket wholly in the tail.
    #[test]
    fn a_bucket_straddling_the_last_reading_is_priced_both_ways() {
        let points = [point(5.0)];
        let input = ShareInput {
            start: 0,
            reset: 5_000,
            readings: &[(500, 10.0)],
            buckets: &[(200, 90.0)],
            points: &points,
        };
        let shared = share_period(&input);
        // 300 of the bucket's 900 seconds fall in [0, 500), a third of its
        // dollars (30), the segment's only dollars: it takes the whole
        // 10-point rise. The other 600 seconds (60 dollars) fall at or
        // after the last reading and price from the 5-dollars-per-point
        // factor: 60 / 5 = 12.
        assert!(approx(shared.bucket_percent[0].unwrap(), 22.0));
        assert_eq!(shared.coverage_until, Some(500));
    }

    /// A window that starts at 1,020 (a 7:27-style offset, not on a
    /// 15-minute boundary), with a single reading of 20 at 2,700 and two
    /// buckets, each with $10, at the absolute 15-minute boundaries 900 and
    /// 1,800: the first straddles the period start, so only 780 of its own
    /// 900 seconds belong to the period, and the second sits wholly inside
    /// it. Both buckets carry the same dollars, so the reading's 20-point
    /// rise splits evenly between them, 10 each, not 0 and 20: the
    /// straddling bucket keeps all its dollars instead of losing the part
    /// that falls before the period start.
    #[test]
    fn bucket_straddling_the_period_start_keeps_all_its_dollars() {
        let points: [FactorPoint; 0] = [];
        let input = ShareInput {
            start: 1_020,
            reset: 100_000,
            readings: &[(2_700, 20.0)],
            buckets: &[(900, 10.0), (1_800, 10.0)],
            points: &points,
        };
        let shared = share_period(&input);
        let percents: Vec<f64> = shared.bucket_percent.iter().map(|p| p.unwrap()).collect();
        assert!(approx(percents[0], 10.0));
        assert!(approx(percents[1], 10.0));
    }

    /// A closed period with no readings at all and a reset at 5,100, inside
    /// the last absolute bucket, 4,500 to 5,400: only 600 of that bucket's
    /// 900 seconds belong to the period. Its factor-priced dollars, together
    /// with an earlier bucket's, overshoot 100 before capping. Capping still
    /// sums to exactly 100, and the straddling bucket's own share of that
    /// 100 equals its own share of the total dollars, not a share inflated
    /// by minutes that lie outside the period.
    #[test]
    fn capped_period_never_exceeds_100_with_a_bucket_straddling_the_reset() {
        let points = [point(1.0)];
        let input = ShareInput {
            start: 0,
            reset: 5_100,
            readings: &[],
            buckets: &[(0, 30.0), (4_500, 90.0)],
            points: &points,
        };
        let uncapped = share_period(&input);
        let uncapped_percents: Vec<f64> =
            uncapped.bucket_percent.iter().map(|p| p.unwrap()).collect();
        assert_eq!(uncapped_percents, vec![30.0, 90.0]);
        assert!(uncapped_percents.iter().sum::<f64>() > 100.0);

        let capped = share_period_capped(&input, 20_000);
        let percents: Vec<f64> = capped.bucket_percent.iter().map(|p| p.unwrap()).collect();
        assert!(approx(percents.iter().sum(), 100.0));
        // The straddling bucket holds 90 of the 120 total dollars: its
        // share of the capped 100 is that same 75 percent.
        assert!(approx(percents[1], 75.0));
    }

    /// A real reading at 4,800, with the period resetting at 5,100: the
    /// bucket at 4,500 spans only 600 of its own 900 seconds inside the
    /// period, of which 300 lie at or after the reading. Its $9 tail prices
    /// half of itself, $4.50, from the $1-per-percent factor: 4.5 percent,
    /// not the 6 percent a 600-of-900 fraction would give, and not the 9
    /// percent pricing the whole bucket would give.
    #[test]
    fn tail_after_a_real_reading_stops_at_the_reset() {
        let points = [point(1.0)];
        let input = ShareInput {
            start: 0,
            reset: 5_100,
            readings: &[(4_800, 0.0)],
            buckets: &[(4_500, 9.0)],
            points: &points,
        };
        let shared = share_period(&input);
        assert!(approx(shared.bucket_percent[0].unwrap(), 4.5));
        assert_eq!(shared.coverage_until, Some(4_800));
    }

    /// A closed window with no readings at all, whose factor prices its two
    /// buckets to 100 and 150, a total of 250: capping scales the whole
    /// window to a stack of exactly 100, keeping the two buckets' 2:3
    /// dollar ratio.
    #[test]
    fn share_period_capped_scales_the_whole_stack_to_100_with_no_readings() {
        let points = [point(1.0)];
        let input = ShareInput {
            start: 0,
            reset: 10_000,
            readings: &[],
            buckets: &[(0, 100.0), (900, 150.0)],
            points: &points,
        };
        let uncapped = share_period(&input);
        assert_eq!(uncapped.bucket_percent, vec![Some(100.0), Some(150.0)]);

        let capped = share_period_capped(&input, 20_000);
        let percents: Vec<f64> = capped.bucket_percent.iter().map(|p| p.unwrap()).collect();
        assert!(approx(percents[0], 40.0));
        assert!(approx(percents[1], 60.0));
        assert!(approx(percents[0] + percents[1], 100.0));
        assert_eq!(capped.coverage_until, None);
        assert_eq!(capped.meter_regressions, 0);
    }

    /// A closed window with a real reading of 40 partway through, and a
    /// tail the factor prices to 90: capping shares only the leftover 60
    /// points across the tail's own dollars. The buckets before the real
    /// reading are untouched, and `coverage_until` still names the real
    /// reading, not the synthetic close.
    #[test]
    fn share_period_capped_shares_only_the_leftover_across_the_tail() {
        let points = [point(1.0)];
        let input = ShareInput {
            start: 0,
            reset: 10_000,
            readings: &[(4_000, 40.0)],
            buckets: &[(1_000, 30.0), (4_000, 90.0)],
            points: &points,
        };
        let uncapped = share_period(&input);
        let uncapped_percents: Vec<f64> =
            uncapped.bucket_percent.iter().map(|p| p.unwrap()).collect();
        assert_eq!(uncapped_percents[0], 40.0);
        assert_eq!(uncapped_percents[1], 90.0);

        let capped = share_period_capped(&input, 20_000);
        let percents: Vec<f64> = capped.bucket_percent.iter().map(|p| p.unwrap()).collect();
        // The bucket before the real reading is unchanged from the uncapped
        // run.
        assert_eq!(percents[0], uncapped_percents[0]);
        // The tail bucket is the only dollars in its segment, so it takes
        // the whole leftover 60-point rise instead of the factor's 90.
        assert!(approx(percents[1], 60.0));
        assert_eq!(capped.coverage_until, Some(4_000));
        assert_eq!(capped.meter_regressions, 0);
        assert!(
            capped.unexplained.is_empty(),
            "the tail has dollars behind it, so no unexplained entry appears"
        );
    }

    /// An open window (`reset` after `now`) is never capped, however far
    /// past 100 its factor-priced stack runs: the window can still gather
    /// more readings before it closes.
    #[test]
    fn share_period_capped_leaves_an_open_window_unchanged() {
        let points = [point(1.0)];
        let input = ShareInput {
            start: 0,
            reset: 10_000,
            readings: &[],
            buckets: &[(0, 100.0), (900, 150.0)],
            points: &points,
        };
        let uncapped = share_period(&input);
        let capped = share_period_capped(&input, 5_000);
        assert_eq!(capped.bucket_percent, uncapped.bucket_percent);
        assert_eq!(capped.unexplained, uncapped.unexplained);
        assert_eq!(capped.coverage_until, uncapped.coverage_until);
        assert_eq!(capped.meter_regressions, uncapped.meter_regressions);
    }

    /// A regression (80 -> 20 -> 80) inside a closed window, each reading
    /// four hours apart so every one closes its own segment regardless of
    /// the percent move (as in
    /// [`a_meter_regression_counts_once_and_never_goes_negative`]): the
    /// middle segment's rise clamps to zero, but the segments before and
    /// after it still carry their own full rises, 80 and 60, and a tail
    /// bucket past the last reading adds another 10 from the factor: an
    /// uncapped stack of 150. The synthetic close then closes its own
    /// segment too (its 20-point rise clears the merge threshold on its
    /// own), so the capped run's stack, 160, is still past the meter's own
    /// maximum. Scaling by `100 / 160` keeps every bucket's ratio to the
    /// others.
    #[test]
    fn share_period_capped_scales_a_regressed_window_to_100() {
        let points = [point(1.0)];
        let four_hours = 4 * 3_600;
        let r0 = (10_000, 80.0);
        let r1 = (r0.0 + four_hours, 20.0);
        let r2 = (r1.0 + four_hours, 80.0);
        let reset = r2.0 + 2_000;
        let input = ShareInput {
            start: 0,
            reset,
            readings: &[r0, r1, r2],
            buckets: &[(0, 10.0), (10_800, 10.0), (25_000, 10.0), (39_600, 10.0)],
            points: &points,
        };
        let uncapped = share_period(&input);
        let uncapped_total: f64 = uncapped.bucket_percent.iter().flatten().sum::<f64>()
            + uncapped
                .unexplained
                .iter()
                .map(|&(_, percent)| percent)
                .sum::<f64>();
        assert!(approx(uncapped_total, 150.0));

        let capped = share_period_capped(&input, reset + 10_000);
        let percents: Vec<f64> = capped.bucket_percent.iter().map(|p| p.unwrap()).collect();
        assert!(approx(percents[0], 50.0));
        assert!(approx(percents[1], 0.0));
        assert!(approx(percents[2], 37.5));
        assert!(approx(percents[3], 12.5));
        assert!(approx(percents.iter().sum(), 100.0));
        assert_eq!(capped.meter_regressions, 1);
        assert_eq!(capped.coverage_until, Some(r2.0));
    }

    /// A closed window whose stack stays at or below 100 is never capped.
    #[test]
    fn share_period_capped_leaves_a_closed_window_at_or_below_100_unchanged() {
        let points = [point(1.0)];
        let input = ShareInput {
            start: 0,
            reset: 10_000,
            readings: &[],
            buckets: &[(0, 30.0), (900, 50.0)],
            points: &points,
        };
        let uncapped = share_period(&input);
        let capped = share_period_capped(&input, 20_000);
        assert_eq!(capped.bucket_percent, uncapped.bucket_percent);
        assert_eq!(capped.unexplained, uncapped.unexplained);
        assert_eq!(capped.coverage_until, uncapped.coverage_until);
        assert_eq!(capped.meter_regressions, uncapped.meter_regressions);
    }
}
