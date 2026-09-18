use antiburn_local::analysis::{QuotaConfidence, QuotaHitSeverity};

use super::*;

/// 15 Sep 2026, 13:43 in Sydney, as milliseconds.
const REFUSED_AT_MS: i64 = 1_789_443_780_000;

fn incident(ts_ms: i64, limit_kind: QuotaLimitKind) -> QuotaIncident {
    QuotaIncident {
        ts_ms,
        limit_kind,
        severity: QuotaHitSeverity::HardHit,
        model: None,
        reset_ts_ms: None,
        reset_clock: None,
        utilization_pct: None,
        confidence: QuotaConfidence::Observed,
    }
}

/// Every block the refusals make, with no span to bound them.
fn blocks(incidents: &[QuotaIncident]) -> Vec<Block> {
    blocks_since(incidents, i64::MIN)
}

fn with_clock(mut incident: QuotaIncident, hour: u8, minute: u8) -> QuotaIncident {
    incident.reset_clock = Some(QuotaResetClock {
        hour,
        minute,
        zone: "Australia/Sydney".to_string(),
    });
    incident
}

#[test]
fn a_stated_clock_later_today_gives_the_wait_to_it() {
    let refused = with_clock(
        incident(REFUSED_AT_MS, QuotaLimitKind::RollingWindow),
        16,
        0,
    );
    let blocks = blocks(&[refused]);
    assert_eq!(blocks.len(), 1);
    // 13:43 to 16:00 is two hours and seventeen minutes.
    assert_eq!(blocks[0].waited_ms(), Some((2 * 60 + 17) * 60 * 1000));
}

#[test]
fn a_clock_that_has_already_passed_states_no_wait() {
    // 9am is behind 13:43, so the next occurrence is tomorrow. A five-hour
    // window cannot hold a 19-hour wait, so the parse is discarded.
    let refused = with_clock(incident(REFUSED_AT_MS, QuotaLimitKind::RollingWindow), 9, 0);
    assert_eq!(blocks(&[refused])[0].waited_ms(), None);
}

#[test]
fn a_weekly_window_holds_a_wait_a_rolling_window_cannot() {
    let refused = with_clock(incident(REFUSED_AT_MS, QuotaLimitKind::Weekly), 9, 0);
    let waited = blocks(&[refused])[0]
        .waited_ms()
        .expect("a weekly wait fits");
    assert_eq!(waited, (19 * 60 + 17) * 60 * 1000);
}

#[test]
fn an_unknown_zone_states_no_wait() {
    let mut refused = incident(REFUSED_AT_MS, QuotaLimitKind::RollingWindow);
    refused.reset_clock = Some(QuotaResetClock {
        hour: 16,
        minute: 0,
        zone: "Nowhere/Invented".to_string(),
    });
    assert_eq!(blocks(&[refused])[0].waited_ms(), None);
}

#[test]
fn a_stated_instant_wins_over_a_stated_clock() {
    let mut refused = with_clock(
        incident(REFUSED_AT_MS, QuotaLimitKind::RollingWindow),
        16,
        0,
    );
    refused.reset_ts_ms = Some(REFUSED_AT_MS + 60 * 60 * 1000);
    assert_eq!(blocks(&[refused])[0].waited_ms(), Some(60 * 60 * 1000));
}

#[test]
fn a_retry_storm_counts_once_and_keeps_the_stated_wait() {
    let first = incident(REFUSED_AT_MS, QuotaLimitKind::RollingWindow);
    let retry = with_clock(
        incident(REFUSED_AT_MS + 90 * 1000, QuotaLimitKind::RollingWindow),
        16,
        0,
    );
    let blocks = blocks(&[retry, first]);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].started_at_ms, REFUSED_AT_MS);
    assert_eq!(blocks[0].waited_ms(), Some((2 * 60 + 17) * 60 * 1000));
}

#[test]
fn retries_inside_the_gap_stay_one_block_however_long_the_window_stays_shut() {
    // The gap separates two refusals. A window shut for longer than the gap
    // is still one block while the retries keep arriving inside it. Counting
    // from the block start split one outage into two.
    let refusals: Vec<_> = (0..4)
        .map(|step| {
            incident(
                REFUSED_AT_MS + step * (STORM_GAP_MS - 1_000),
                QuotaLimitKind::RollingWindow,
            )
        })
        .collect();
    let blocks = blocks(&refusals);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].started_at_ms, REFUSED_AT_MS);
}

#[test]
fn two_limit_kinds_refusing_together_are_two_blocks() {
    // A five-hour limit and a weekly limit are different windows. One block
    // holding both took the weekly reset as the wait for the five-hour one.
    let rolling = incident(REFUSED_AT_MS, QuotaLimitKind::RollingWindow);
    let weekly = incident(REFUSED_AT_MS + 60 * 1000, QuotaLimitKind::Weekly);
    let blocks = blocks(&[rolling, weekly]);
    assert_eq!(blocks.len(), 2);
}

#[test]
fn a_refusal_past_the_storm_gap_is_its_own_block() {
    let first = incident(REFUSED_AT_MS, QuotaLimitKind::RollingWindow);
    let later = incident(
        REFUSED_AT_MS + STORM_GAP_MS + 1_000,
        QuotaLimitKind::RollingWindow,
    );
    assert_eq!(blocks(&[first, later]).len(), 2);
}

#[test]
fn the_overage_counts_blocks_inside_the_span_and_sums_their_waits() {
    let old = incident(
        REFUSED_AT_MS - 40 * 86_400_000,
        QuotaLimitKind::RollingWindow,
    );
    let counted = with_clock(
        incident(REFUSED_AT_MS, QuotaLimitKind::RollingWindow),
        16,
        0,
    );
    let silent = incident(
        REFUSED_AT_MS + STORM_GAP_MS + 1_000,
        QuotaLimitKind::RollingWindow,
    );

    let overage = overage(&[old, counted, silent], REFUSED_AT_MS - 30 * 86_400_000);

    assert_eq!(overage.block_count, 2);
    assert_eq!(overage.waited_ms, (2 * 60 + 17) * 60 * 1000);
    assert_eq!(overage.blocks_without_wait, 1);
    assert_eq!(
        overage.last_block_at_ms,
        Some(REFUSED_AT_MS + STORM_GAP_MS + 1_000)
    );
}

/// A refusal before the span must not take a refusal inside it away.
#[test]
fn a_refusal_before_the_span_leaves_the_one_inside_it_counted() {
    // The two refusals sit inside one storm gap, so they merge into one
    // block and the block keeps the earlier start. A span test on the block
    // then drops it, and the refusal the reader met inside the span counts
    // for nothing.
    let hour_ms = 60 * 60 * 1000;
    let before = incident(REFUSED_AT_MS - hour_ms, QuotaLimitKind::RollingWindow);
    let inside = incident(REFUSED_AT_MS + hour_ms, QuotaLimitKind::RollingWindow);

    let overage = overage(&[before, inside], REFUSED_AT_MS);

    assert_eq!(overage.block_count, 1);
    assert_eq!(overage.last_block_at_ms, Some(REFUSED_AT_MS + hour_ms));
}

fn rollup(last_observed_epoch: i64, peak_used_percent: Option<f64>) -> ProviderUsagePeriodRollup {
    kinded_rollup(last_observed_epoch, peak_used_percent, "rolling")
}

fn kinded_rollup(
    last_observed_epoch: i64,
    peak_used_percent: Option<f64>,
    window_kind: &str,
) -> ProviderUsagePeriodRollup {
    ProviderUsagePeriodRollup {
        period_id: last_observed_epoch,
        provider: "anthropic".to_string(),
        account_key: "account".to_string(),
        window_kind: window_kind.to_string(),
        window_role: "primaryLong".to_string(),
        scope_key: "account".to_string(),
        starts_at_epoch: Some(last_observed_epoch - 604_800),
        resets_at_epoch: Some(last_observed_epoch),
        first_observed_epoch: last_observed_epoch - 604_800,
        last_observed_epoch,
        peak_used_percent,
        last_used_percent: peak_used_percent,
        observation_count: 1,
        refusal_count: 0,
    }
}

fn weeks(peaks: &[Option<f64>]) -> Vec<ProviderUsagePeriodRollup> {
    peaks
        .iter()
        .enumerate()
        .map(|(index, peak)| rollup(1_800_000_000 + (index as i64) * 604_800, *peak))
        .collect()
}

#[test]
fn a_single_period_states_a_peak_and_no_typical_figure() {
    let utilization = utilization(&weeks(&[Some(62.0)])).expect("one period is enough for a peak");
    assert_eq!(utilization.peak_percent, 62.0);
    assert_eq!(utilization.typical_percent, None);
    assert_eq!(utilization.period_count, 1);
}

#[test]
fn six_periods_state_the_median_as_the_typical_figure() {
    let peaks = [
        Some(18.0),
        Some(40.0),
        Some(44.0),
        Some(50.0),
        Some(60.0),
        Some(62.0),
    ];
    let utilization = utilization(&weeks(&peaks)).expect("six periods reduce");
    assert_eq!(utilization.period_count, 6);
    assert_eq!(utilization.typical_percent, Some(47.0));
    assert_eq!(utilization.peak_percent, 62.0);
    assert_eq!(utilization.maxed_period_count, 0);
}

/// The median and the mean disagree whenever idle periods sit near zero.
/// The median is the one that describes a period the reader recognizes.
#[test]
fn idle_periods_move_the_median_far_less_than_a_mean() {
    let peaks = [
        Some(0.0),
        Some(0.0),
        Some(1.0),
        Some(29.0),
        Some(90.0),
        Some(100.0),
        Some(100.0),
    ];
    let utilization = utilization(&weeks(&peaks)).expect("seven periods reduce");
    assert_eq!(utilization.typical_percent, Some(29.0));
    assert_eq!(utilization.peak_percent, 100.0);
    assert_eq!(utilization.maxed_period_count, 2);
}

#[test]
fn a_period_with_no_reported_figure_is_left_out_and_never_read_as_zero() {
    let utilization =
        utilization(&weeks(&[Some(80.0), None, Some(60.0)])).expect("two periods reduce");
    assert_eq!(utilization.period_count, 2);
    assert_eq!(utilization.peak_percent, 80.0);
}

#[test]
fn periods_with_no_reported_figure_reduce_to_nothing() {
    assert_eq!(utilization(&weeks(&[None, None])), None);
}

/// The window a figure covers must follow the provider's newest word for it.
///
/// A weekly window and a rolling window answer different questions. A reader
/// who sees the older name against the newer figures reads the wrong
/// question.
#[test]
fn the_newest_period_names_the_window() {
    let rollups = vec![
        kinded_rollup(1_000, Some(40.0), "other:fortnightly"),
        kinded_rollup(2_000, Some(62.0), "weekly"),
    ];

    let reduced = utilization(&rollups).expect("two periods report a figure");

    assert_eq!(reduced.window_kind, "weekly");
    assert_eq!(reduced.peak_percent, 62.0);
}

#[test]
fn the_average_covers_every_period_the_store_holds() {
    // The average reads across all of them, so one busy period does not
    // speak for the rest and one idle period is not left out.
    let rollups = vec![
        kinded_rollup(1_000, Some(90.0), "weekly"),
        kinded_rollup(2_000, Some(55.0), "weekly"),
        kinded_rollup(3_000, Some(20.0), "weekly"),
    ];

    let reduced = utilization(&rollups).expect("three periods report a figure");

    assert_eq!(reduced.average_percent, 55.0);
    assert_eq!(reduced.peak_percent, 90.0);
}

fn reading(period_id: i64, observed_at_epoch: i64, used_percent: f64) -> ProviderUsageReading {
    ProviderUsageReading {
        provider: "openai".to_string(),
        account_key: "account".to_string(),
        period_id,
        observed_at_epoch,
        used_percent,
    }
}

fn only_consumption(readings: &[ProviderUsageReading]) -> AccountConsumption {
    consumption(readings)
        .into_values()
        .next()
        .expect("the readings name one account")
}

/// The provider states a running total. What a day consumed is the rise.
#[test]
fn a_reading_consumes_the_rise_since_the_reading_before_it() {
    let readings = vec![
        reading(1, 1_000, 12.0),
        reading(1, 2_000, 30.0),
        reading(1, 3_000, 41.0),
    ];

    let consumed = only_consumption(&readings);

    let percents: Vec<f64> = consumed
        .consumed
        .iter()
        .map(|entry| entry.percent)
        .collect();
    assert_eq!(percents, vec![12.0, 18.0, 11.0]);
}

/// A new period restarts the total. Its first reading is a rise from zero,
/// not a fall from the period before it.
#[test]
fn a_new_period_restarts_the_total() {
    let readings = vec![
        reading(1, 1_000, 80.0),
        reading(2, 2_000, 5.0),
        reading(2, 3_000, 9.0),
    ];

    let consumed = only_consumption(&readings);

    let percents: Vec<f64> = consumed
        .consumed
        .iter()
        .map(|entry| entry.percent)
        .collect();
    assert_eq!(percents, vec![80.0, 5.0, 4.0]);
}

/// A restated lower figure consumes nothing. The reader did not give
/// allowance back.
#[test]
fn a_fall_inside_a_period_consumes_nothing() {
    let readings = vec![reading(1, 1_000, 40.0), reading(1, 2_000, 38.0)];

    let consumed = only_consumption(&readings);

    assert_eq!(consumed.consumed[1].percent, 0.0);
}

/// Two readings that bracket a day speak for it. A day outside every span
/// does not, and reads as unknown rather than as idle.
#[test]
fn readings_speak_for_the_days_between_them_and_for_no_others() {
    let day = 86_400;
    let readings = vec![reading(1, 10 * day, 10.0), reading(1, 13 * day, 25.0)];

    let consumed = only_consumption(&readings);

    assert!(consumed.covers(11 * day, 12 * day - 1));
    assert!(!consumed.covers(14 * day, 15 * day - 1));
    assert!(!consumed.covers(8 * day, 9 * day - 1));
}

/// The chart marks a day the reader met a refusal on, even when a refusal
/// before the span falls inside the same storm gap.
#[test]
fn account_blocks_keep_a_refusal_a_block_before_the_span_would_hide() {
    let hour_ms = 60 * 60 * 1000;
    let record = QuotaIncidentRecord {
        agent: "claude".to_string(),
        incidents_json: serde_json::to_string(&vec![
            incident(REFUSED_AT_MS - hour_ms, QuotaLimitKind::RollingWindow),
            incident(REFUSED_AT_MS + hour_ms, QuotaLimitKind::RollingWindow),
        ])
        .expect("the incidents serialize"),
        provider_accounts_json: r#"[{"provider":"anthropic","accountKey":"account"}]"#.to_string(),
    };

    let blocks = account_blocks(&[record], REFUSED_AT_MS);

    let account = blocks
        .get(&("anthropic".to_string(), "account".to_string()))
        .expect("the record names one account");
    assert_eq!(account.len(), 1);
    assert_eq!(account[0].started_at_ms, REFUSED_AT_MS + hour_ms);
}

/// A block outside the span is not one of the blocks the chart marks.
#[test]
fn account_blocks_keep_only_the_blocks_inside_the_span() {
    let record = QuotaIncidentRecord {
        agent: "claude".to_string(),
        incidents_json: serde_json::to_string(&vec![
            incident(
                REFUSED_AT_MS - 40 * 86_400_000,
                QuotaLimitKind::RollingWindow,
            ),
            incident(REFUSED_AT_MS, QuotaLimitKind::RollingWindow),
        ])
        .expect("the incidents serialize"),
        provider_accounts_json: r#"[{"provider":"anthropic","accountKey":"account"}]"#.to_string(),
    };

    let blocks = account_blocks(&[record], REFUSED_AT_MS - 30 * 86_400_000);

    let account = blocks
        .get(&("anthropic".to_string(), "account".to_string()))
        .expect("the record names one account");
    assert_eq!(account.len(), 1);
    assert_eq!(account[0].started_at_ms, REFUSED_AT_MS);
}
