use super::*;
use crate::store::provider_limit::LANE_WEEKLY;

const DAY: i64 = 86_400;
const WEEK: i64 = 7 * DAY;
const FIVE_HOURS: i64 = 5 * 3_600;

fn period(
    id: i64,
    starts: Option<i64>,
    resets: Option<i64>,
    last_observed: i64,
) -> ProviderUsagePeriod {
    ProviderUsagePeriod {
        id,
        provider: "anthropic".to_string(),
        account_key: "a".repeat(64),
        window_id: "weekly".to_string(),
        window_kind: "weekly".to_string(),
        window_role: "primaryLong".to_string(),
        scope_key: "account".to_string(),
        scope_label: "account".to_string(),
        duration_seconds: resets.zip(starts).map(|(reset, start)| reset - start),
        starts_at_epoch: starts,
        resets_at_epoch: resets,
        first_observed_epoch: last_observed,
        last_observed_epoch: last_observed,
    }
}

/// The resolved period backed by the observed row `period_id`, ignoring any
/// cadence-extrapolated slots the same call also returned.
fn find(periods: &[QuotaPeriod], period_id: i64) -> QuotaPeriod {
    *periods
        .iter()
        .find(|candidate| candidate.period_id == Some(period_id))
        .expect("expected the observed period to survive resolution")
}

#[test]
fn a_fully_reported_period_carries_reported_on_both_ends() {
    let observed = [period(1, Some(1_000), Some(1_000 + WEEK), 1_000 + WEEK)];
    let periods = resolve_periods(LANE_WEEKLY, WEEK, &observed, &[], 0, 2 * WEEK, 1_000 + WEEK);
    let resolved = find(&periods, 1);
    assert_eq!(resolved.start_source, BoundarySource::Reported);
    assert_eq!(resolved.reset_source, BoundarySource::Reported);
    assert_eq!(resolved.starts_at_epoch, 1_000);
    assert_eq!(resolved.resets_at_epoch, 1_000 + WEEK);
}

#[test]
fn a_period_missing_its_start_derives_it_from_the_reset_and_lane_duration() {
    let observed = [period(1, None, Some(1_000 + WEEK), 1_000 + WEEK)];
    let periods = resolve_periods(LANE_WEEKLY, WEEK, &observed, &[], 0, 2 * WEEK, 1_000 + WEEK);
    let resolved = find(&periods, 1);
    assert_eq!(resolved.start_source, BoundarySource::Derived);
    assert_eq!(resolved.reset_source, BoundarySource::Reported);
    assert_eq!(resolved.starts_at_epoch, 1_000);
}

#[test]
fn a_period_missing_its_reset_derives_it_from_the_start_and_lane_duration() {
    let observed = [period(1, Some(1_000), None, 1_000)];
    let periods = resolve_periods(LANE_WEEKLY, WEEK, &observed, &[], 0, 2 * WEEK, 1_000);
    let resolved = find(&periods, 1);
    assert_eq!(resolved.start_source, BoundarySource::Reported);
    assert_eq!(resolved.reset_source, BoundarySource::Derived);
    assert_eq!(resolved.resets_at_epoch, 1_000 + WEEK);
}

#[test]
fn a_period_with_neither_boundary_is_skipped() {
    let observed = [period(1, None, None, 1_000)];
    let periods = resolve_periods(LANE_WEEKLY, WEEK, &observed, &[], 0, 2 * WEEK, 1_000);
    assert!(periods.is_empty());
}

#[test]
fn cadence_extrapolates_forward_and_backward_from_one_observed_reset() {
    let anchor_reset = 10 * WEEK;
    let observed = [period(
        1,
        Some(anchor_reset - WEEK),
        Some(anchor_reset),
        anchor_reset,
    )];
    let range_start = anchor_reset - 3 * WEEK;
    let range_end = anchor_reset + 3 * WEEK;
    let now = anchor_reset + WEEK / 2;
    let periods = resolve_periods(
        LANE_WEEKLY,
        WEEK,
        &observed,
        &[],
        range_start,
        range_end,
        now,
    );

    // The observed period plus cadence slots both before and after it.
    assert!(periods.len() > 1, "expected cadence slots on both sides");
    assert!(
        periods
            .iter()
            .any(|p| p.resets_at_epoch < anchor_reset && p.reset_source == BoundarySource::Cadence),
        "expected a backward cadence slot"
    );
    assert!(
        periods
            .iter()
            .any(|p| p.starts_at_epoch > anchor_reset && p.start_source == BoundarySource::Cadence),
        "expected a forward cadence slot"
    );
    // Every slot is contiguous and none duplicates the observed period.
    let observed_slot_count = periods.iter().filter(|p| p.period_id == Some(1)).count();
    assert_eq!(observed_slot_count, 1);
}

#[test]
fn an_observed_slot_suppresses_the_cadence_slot_it_would_otherwise_generate() {
    let anchor_reset = 10 * WEEK;
    // A second, later reset one week on: cadence would otherwise invent this
    // exact slot from the anchor alone.
    let observed = [
        period(
            1,
            Some(anchor_reset - WEEK),
            Some(anchor_reset),
            anchor_reset,
        ),
        period(
            2,
            Some(anchor_reset),
            Some(anchor_reset + WEEK),
            anchor_reset + WEEK,
        ),
    ];
    let now = anchor_reset + WEEK / 2;
    let periods = resolve_periods(
        LANE_WEEKLY,
        WEEK,
        &observed,
        &[],
        anchor_reset - WEEK,
        anchor_reset + 2 * WEEK,
        now,
    );
    let forward_cadence_duplicates = periods
        .iter()
        .filter(|p| p.starts_at_epoch == anchor_reset && p.period_id.is_none())
        .count();
    assert_eq!(
        forward_cadence_duplicates, 0,
        "the observed period at this slot should suppress the cadence guess"
    );
}

#[test]
fn does_not_extrapolate_a_weekly_lane_with_no_observed_period_at_all() {
    let periods = resolve_periods(LANE_WEEKLY, WEEK, &[], &[], 0, 4 * WEEK, 2 * WEEK);
    assert!(periods.is_empty());
}

#[test]
fn turn_gap_opens_two_windows_from_turns_six_hours_apart() {
    let first_turn = 1_000;
    let second_turn = first_turn + 6 * 3_600;
    let periods = resolve_periods(
        LANE_FIVE_HOUR,
        FIVE_HOURS,
        &[],
        &[first_turn, second_turn],
        0,
        second_turn + FIVE_HOURS,
        second_turn,
    );
    assert_eq!(periods.len(), 2);
    assert_eq!(periods[0].starts_at_epoch, first_turn);
    assert_eq!(periods[0].start_source, BoundarySource::TurnGap);
    assert_eq!(periods[0].reset_source, BoundarySource::TurnGap);
    assert_eq!(periods[1].starts_at_epoch, second_turn);
}

#[test]
fn turn_gap_does_not_open_a_window_for_a_turn_inside_an_observed_period() {
    let observed = [period(
        1,
        Some(1_000),
        Some(1_000 + FIVE_HOURS),
        1_000 + FIVE_HOURS,
    )];
    let turn_inside = 1_000 + 100;
    let periods = resolve_periods(
        LANE_FIVE_HOUR,
        FIVE_HOURS,
        &observed,
        &[turn_inside],
        0,
        2 * FIVE_HOURS,
        1_000 + FIVE_HOURS,
    );
    assert_eq!(periods.len(), 1);
    assert_eq!(periods[0].period_id, Some(1));
}

#[test]
fn turn_gap_caps_inferred_windows_at_four_hundred() {
    let turns: Vec<i64> = (0..500).map(|index| index * FIVE_HOURS).collect();
    let periods = resolve_periods(
        LANE_FIVE_HOUR,
        FIVE_HOURS,
        &[],
        &turns,
        0,
        500 * FIVE_HOURS,
        500 * FIVE_HOURS,
    );
    assert_eq!(periods.len(), MAX_INFERRED_WINDOWS);
}

#[test]
fn empty_inputs_resolve_to_no_periods() {
    assert!(resolve_periods(LANE_WEEKLY, WEEK, &[], &[], 0, WEEK, 0).is_empty());
    assert!(resolve_periods(LANE_FIVE_HOUR, FIVE_HOURS, &[], &[], 0, FIVE_HOURS, 0).is_empty());
}
