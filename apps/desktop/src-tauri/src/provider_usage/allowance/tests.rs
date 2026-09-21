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

fn period(start: i64, estimated: Option<f64>) -> QuotaPeriodPayload {
    QuotaPeriodPayload {
        period_id: None,
        starts_at_epoch: start,
        resets_at_epoch: start + 7 * 86_400,
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

#[test]
fn shared_estimates_include_inferred_periods_and_exclude_unknown_periods() {
    let periods = vec![
        period(0, Some(12.0)),
        period(604_800, None),
        period(1_209_600, Some(80.0)),
    ];
    let result = utilization(&periods, "weekly").expect("two known periods");
    assert_eq!(result.period_count, 2);
    assert_eq!(result.average_percent, 46.0);
    assert_eq!(result.peak_percent, 80.0);
    assert_eq!(result.first_period_at_epoch, 0);
    assert_eq!(result.last_period_at_epoch, 1_209_600);
    assert_eq!(utilization(&[period(0, None)], "weekly"), None);
}

#[test]
fn six_shared_periods_state_a_typical_and_maxed_count() {
    let periods: Vec<_> = [18.0, 40.0, 44.0, 50.0, 60.0, 100.0]
        .into_iter()
        .enumerate()
        .map(|(i, percent)| period(i as i64 * 604_800, Some(percent)))
        .collect();
    let result = utilization(&periods, "rolling").expect("six known periods");
    assert_eq!(result.typical_percent, Some(47.0));
    assert_eq!(result.maxed_period_count, 1);
    assert_eq!(result.window_kind, "rolling");
}

#[test]
fn open_period_overshoot_is_capped_for_utilization_statistics() {
    let result = utilization(&[period(0, Some(130.0))], "weekly").expect("known period");
    assert_eq!(result.peak_percent, 100.0);
    assert_eq!(result.average_percent, 100.0);
    assert_eq!(result.maxed_period_count, 1);
}

#[test]
fn daily_consumption_sums_the_shared_buckets_on_their_own_days() {
    let day = 86_400;
    let mut period = period(0, Some(60.0));
    period.samples.push(crate::dto::QuotaSamplePayload {
        observed_at_epoch: 3 * day,
        used_percent: Some(60.0),
        fresh: true,
        authoritative: true,
    });
    period
        .contributions
        .push(crate::dto::QuotaContributionPayload {
            agent: "claude".to_string(),
            session_id: "one".to_string(),
            wsl_distro: None,
            bucket_start_epoch: day,
            usd: 1.0,
            percent: Some(20.0),
        });
    period
        .unattributed_buckets
        .push(crate::dto::QuotaBucketTotalPayload {
            bucket_start_epoch: day,
            usd: 1.0,
            percent: Some(10.0),
        });
    period
        .unexplained_buckets
        .push(crate::dto::QuotaBucketTotalPayload {
            bucket_start_epoch: 3 * day,
            usd: 0.0,
            percent: Some(30.0),
        });
    let result = consumption(&[period]);
    assert_eq!(
        result
            .consumed
            .iter()
            .filter(|entry| entry.at_epoch == day)
            .map(|entry| entry.percent)
            .sum::<f64>(),
        30.0
    );
    assert_eq!(
        result
            .consumed
            .iter()
            .filter(|entry| entry.at_epoch == 3 * day)
            .map(|entry| entry.percent)
            .sum::<f64>(),
        30.0
    );
    assert!(!result.covers(2 * day, 3 * day - 1));
    assert!(!result.covers(4 * day, 5 * day - 1));
}

#[test]
fn sparse_inferred_buckets_leave_days_between_them_unknown() {
    let day = 86_400;
    let mut inferred = period(0, Some(30.0));
    for at in [day, 6 * day] {
        inferred
            .contributions
            .push(crate::dto::QuotaContributionPayload {
                agent: "codex".to_string(),
                session_id: "sparse".to_string(),
                wsl_distro: None,
                bucket_start_epoch: at,
                usd: 1.0,
                percent: Some(15.0),
            });
    }
    let result = consumption(&[inferred]);
    assert!(result.covers(day, 2 * day - 1));
    assert!(!result.covers(3 * day, 4 * day - 1));
    assert!(result.covers(6 * day, 7 * day - 1));
}

#[test]
fn authoritative_sample_span_does_not_join_a_later_estimated_bucket() {
    let day = 86_400;
    let mut observed = period(0, Some(35.0));
    for at in [day, 3 * day] {
        observed.samples.push(crate::dto::QuotaSamplePayload {
            observed_at_epoch: at,
            used_percent: Some(20.0),
            fresh: true,
            authoritative: true,
        });
    }
    observed
        .contributions
        .push(crate::dto::QuotaContributionPayload {
            agent: "claude".to_string(),
            session_id: "tail".to_string(),
            wsl_distro: None,
            bucket_start_epoch: 6 * day,
            usd: 1.0,
            percent: Some(15.0),
        });
    let result = consumption(&[observed]);
    assert!(result.covers(2 * day, 3 * day - 1));
    assert!(!result.covers(4 * day, 5 * day - 1));
    assert!(result.covers(6 * day, 7 * day - 1));
}

#[test]
fn inferred_period_bucket_estimate_makes_its_day_known() {
    let mut inferred = period(0, Some(15.0));
    inferred
        .contributions
        .push(crate::dto::QuotaContributionPayload {
            agent: "codex".to_string(),
            session_id: "two".to_string(),
            wsl_distro: None,
            bucket_start_epoch: 86_400,
            usd: 2.0,
            percent: Some(15.0),
        });
    let result = consumption(&[inferred]);
    assert!(result.covers(86_400, 2 * 86_400 - 1));
    assert!(!result.covers(0, 86_399));
    assert_eq!(result.consumed[0].percent, 15.0);
}

#[test]
fn truncated_period_sample_outside_its_bounds_does_not_cover_a_day() {
    let mut truncated = period(0, None);
    truncated.resets_at_epoch = 86_400;
    truncated.samples.push(crate::dto::QuotaSamplePayload {
        observed_at_epoch: 2 * 86_400,
        used_percent: Some(50.0),
        fresh: true,
        authoritative: true,
    });
    assert_eq!(consumption(&[truncated]), AccountConsumption::default());
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
