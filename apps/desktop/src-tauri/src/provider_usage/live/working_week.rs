//! Time arithmetic that counts only the days the reader works.
//!
//! The elapsed marker, the pace verdict, and the runway all compare usage
//! against time. A reader who stops on the weekend does not want the weekend
//! counted. These functions measure a span in working time, not clock time.
//!
//! Every function takes the reader's UTC offset. A day starts at the reader's
//! local midnight, not at UTC midnight.
//!
//! A [`WorkingWeek::Seven`] week keeps the plain clock arithmetic. The result
//! is then the same as the behaviour before this setting existed.

use time::{Duration, OffsetDateTime, UtcOffset};

use crate::store::WorkingWeek;

/// The largest number of day steps one walk makes.
///
/// A weekly window needs seven steps. A runway projection can reach further.
/// The limit stops a corrupt timestamp from starting a very long loop.
const MAX_DAY_STEPS: u32 = 400;

/// Make a `UtcOffset` from whole minutes. An unusable value gives UTC.
pub fn offset_from_minutes(minutes: i32) -> UtcOffset {
    UtcOffset::from_whole_seconds(minutes.saturating_mul(60)).unwrap_or(UtcOffset::UTC)
}

/// The working week and the local offset that one calculation measures with.
///
/// A caller that must not apply the preference uses [`WorkingClock::every_day`]
/// instead of an `Option`. A five-hour window is an example: the reader's week
/// says nothing about a period that rolls several times in one day.
#[derive(Debug, Clone, Copy)]
pub struct WorkingClock {
    week: WorkingWeek,
    offset: UtcOffset,
}

impl WorkingClock {
    /// A clock that applies the reader's preference.
    pub fn new(week: WorkingWeek, offset: UtcOffset) -> Self {
        Self { week, offset }
    }

    /// A clock that counts every day, whatever the reader chose.
    pub fn every_day(offset: UtcOffset) -> Self {
        Self {
            week: WorkingWeek::Seven,
            offset,
        }
    }

    /// See [`working_span`].
    pub fn span(self, from: OffsetDateTime, to: OffsetDateTime) -> Duration {
        working_span(from, to, self.week, self.offset)
    }

    /// See [`elapsed_fraction`].
    pub fn elapsed_fraction(
        self,
        start: OffsetDateTime,
        end: OffsetDateTime,
        now: OffsetDateTime,
    ) -> Option<f64> {
        elapsed_fraction(start, end, now, self.week, self.offset)
    }

    /// See [`advance_working`].
    pub fn advance(self, from: OffsetDateTime, burn: Duration) -> Option<OffsetDateTime> {
        advance_working(from, burn, self.week, self.offset)
    }
}

/// The first local midnight after `at`.
fn next_local_midnight(at: OffsetDateTime, offset: UtcOffset) -> Option<OffsetDateTime> {
    let local = at.to_offset(offset);
    Some(local.date().next_day()?.midnight().assume_offset(offset))
}

/// The part of the span from `from` to `to` that falls on a working day.
///
/// The result is never negative. A `Seven` week returns the plain difference.
pub fn working_span(
    from: OffsetDateTime,
    to: OffsetDateTime,
    week: WorkingWeek,
    offset: UtcOffset,
) -> Duration {
    if to <= from {
        return Duration::ZERO;
    }
    if week.is_every_day() {
        return to - from;
    }

    let mut total = Duration::ZERO;
    let mut cursor = from;
    for _ in 0..MAX_DAY_STEPS {
        if cursor >= to {
            break;
        }
        // Each segment ends at the next local midnight. The segment stays
        // inside one local day, so one weekday test covers all of it.
        let Some(boundary) = next_local_midnight(cursor, offset) else {
            break;
        };
        let segment_end = boundary.min(to);
        if week.includes(cursor.to_offset(offset).weekday()) {
            total += segment_end - cursor;
        }
        cursor = segment_end;
    }
    total
}

/// The share of a window's working time that has passed at `now`, from 0 to 1.
///
/// Returns `None` when the window has no length, and when the window holds no
/// working day. An unknown stays unknown. A marker drawn from a guess is worse
/// than no marker.
pub fn elapsed_fraction(
    start: OffsetDateTime,
    end: OffsetDateTime,
    now: OffsetDateTime,
    week: WorkingWeek,
    offset: UtcOffset,
) -> Option<f64> {
    if end <= start {
        return None;
    }
    let total = working_span(start, end, week, offset).as_seconds_f64();
    if total <= 0.0 {
        return None;
    }
    let at = now.clamp(start, end);
    let elapsed = working_span(start, at, week, offset).as_seconds_f64();
    Some((elapsed / total).clamp(0.0, 1.0))
}

/// The instant that is `burn` of working time after `from`.
///
/// A non-working day does not use the budget. Returns `None` when the answer
/// is further away than the walk limit reaches.
pub fn advance_working(
    from: OffsetDateTime,
    burn: Duration,
    week: WorkingWeek,
    offset: UtcOffset,
) -> Option<OffsetDateTime> {
    if burn <= Duration::ZERO {
        return Some(from);
    }
    if week.is_every_day() {
        return from.checked_add(burn);
    }

    let mut remaining = burn;
    let mut cursor = from;
    for _ in 0..MAX_DAY_STEPS {
        let boundary = next_local_midnight(cursor, offset)?;
        if week.includes(cursor.to_offset(offset).weekday()) {
            let available = boundary - cursor;
            if available >= remaining {
                return cursor.checked_add(remaining);
            }
            remaining -= available;
        }
        cursor = boundary;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month};

    /// Make an instant from a local date and time at `offset_minutes`.
    fn at(
        year: i32,
        month: Month,
        day: u8,
        hour: u8,
        minute: u8,
        offset_minutes: i32,
    ) -> OffsetDateTime {
        Date::from_calendar_date(year, month, day)
            .expect("valid date")
            .with_hms(hour, minute, 0)
            .expect("valid time")
            .assume_offset(offset_from_minutes(offset_minutes))
    }

    /// The test window runs from Monday 2026-09-21 to the Monday after it.
    fn week_bounds(offset_minutes: i32) -> (OffsetDateTime, OffsetDateTime) {
        (
            at(2026, Month::September, 21, 0, 0, offset_minutes),
            at(2026, Month::September, 28, 0, 0, offset_minutes),
        )
    }

    #[test]
    fn seven_day_week_keeps_plain_clock_arithmetic() {
        let (start, end) = week_bounds(0);
        let offset = offset_from_minutes(0);
        // Saturday noon, five and a half days into a seven-day window.
        let now = at(2026, Month::September, 26, 12, 0, 0);

        assert_eq!(
            working_span(start, end, WorkingWeek::Seven, offset),
            end - start
        );
        let plain = (now - start).as_seconds_f64() / (end - start).as_seconds_f64();
        let measured =
            elapsed_fraction(start, end, now, WorkingWeek::Seven, offset).expect("a fraction");
        assert!((measured - plain).abs() < 1e-9, "{measured} != {plain}");
    }

    #[test]
    fn a_shorter_week_counts_fewer_days() {
        let (start, end) = week_bounds(0);
        let offset = offset_from_minutes(0);

        assert_eq!(
            working_span(start, end, WorkingWeek::Five, offset),
            Duration::days(5)
        );
        assert_eq!(
            working_span(start, end, WorkingWeek::Six, offset),
            Duration::days(6)
        );
    }

    #[test]
    fn friday_evening_on_a_five_day_week_reads_near_the_end() {
        let (start, end) = week_bounds(0);
        let offset = offset_from_minutes(0);
        // Friday 18:00 is four days and eighteen hours of working time.
        let now = at(2026, Month::September, 25, 18, 0, 0);

        let measured =
            elapsed_fraction(start, end, now, WorkingWeek::Five, offset).expect("a fraction");
        assert!((measured - 0.95).abs() < 1e-9, "{measured}");

        // The same moment on a seven-day week looks much further from the end.
        let linear =
            elapsed_fraction(start, end, now, WorkingWeek::Seven, offset).expect("a fraction");
        assert!(linear < 0.69, "{linear}");
    }

    #[test]
    fn the_marker_holds_still_through_the_weekend() {
        let (start, end) = week_bounds(0);
        let offset = offset_from_minutes(0);
        let saturday = at(2026, Month::September, 26, 9, 0, 0);
        let sunday = at(2026, Month::September, 27, 21, 0, 0);

        assert_eq!(
            elapsed_fraction(start, end, saturday, WorkingWeek::Five, offset),
            Some(1.0)
        );
        assert_eq!(
            elapsed_fraction(start, end, sunday, WorkingWeek::Five, offset),
            Some(1.0)
        );
    }

    #[test]
    fn a_local_offset_moves_the_day_boundary() {
        // Sydney runs ten hours ahead in September. The window starts at the
        // local Monday midnight, so the last working day ends Friday there.
        let offset = offset_from_minutes(600);
        let (start, end) = week_bounds(600);
        let saturday_morning = at(2026, Month::September, 26, 9, 0, 600);

        assert_eq!(
            elapsed_fraction(start, end, saturday_morning, WorkingWeek::Five, offset),
            Some(1.0)
        );
    }

    #[test]
    fn advance_skips_the_weekend() {
        let offset = offset_from_minutes(0);
        let friday_noon = at(2026, Month::September, 25, 12, 0, 0);

        let reached = advance_working(friday_noon, Duration::hours(24), WorkingWeek::Five, offset)
            .expect("an instant");
        assert_eq!(reached, at(2026, Month::September, 28, 12, 0, 0));

        // A seven-day week lands on the next calendar day instead.
        let linear = advance_working(friday_noon, Duration::hours(24), WorkingWeek::Seven, offset)
            .expect("an instant");
        assert_eq!(linear, at(2026, Month::September, 26, 12, 0, 0));
    }

    #[test]
    fn a_span_with_no_working_day_stays_unknown() {
        let offset = offset_from_minutes(0);
        let saturday = at(2026, Month::September, 26, 0, 0, 0);
        let monday = at(2026, Month::September, 28, 0, 0, 0);
        let sunday = at(2026, Month::September, 27, 0, 0, 0);

        assert_eq!(
            working_span(saturday, monday, WorkingWeek::Five, offset),
            Duration::ZERO
        );
        assert_eq!(
            elapsed_fraction(saturday, monday, sunday, WorkingWeek::Five, offset),
            None
        );
    }

    #[test]
    fn a_reversed_span_is_empty() {
        let offset = offset_from_minutes(0);
        let (start, end) = week_bounds(0);

        assert_eq!(
            working_span(end, start, WorkingWeek::Five, offset),
            Duration::ZERO
        );
        assert_eq!(
            elapsed_fraction(end, start, end, WorkingWeek::Five, offset),
            None
        );
    }
}
