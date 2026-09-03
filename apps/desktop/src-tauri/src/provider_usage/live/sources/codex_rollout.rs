//! Read the newest rate-limit reading out of the Codex CLI's own rollout
//! files.
//!
//! Every turn the Codex CLI runs, it appends a `token_count` event to the
//! session's rollout file, and that event carries the account's rate limits
//! as the server reported them on that turn. [`latest_reading`] finds the
//! newest such event and turns it into a [`RolloutReading`], for
//! [`super::codex_fetch`] to offer as a seed when the endpoint and the
//! app-server fallback both fail.
//!
//! # Where the rollouts live
//!
//! `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<timestamp>-<uuid>.jsonl`, or
//! `~/.codex/sessions/...` when `CODEX_HOME` is unset. One file is one
//! session: one JSON object per line, each with a `timestamp`, an `ordinal`,
//! a `type`, and a `payload`.
//!
//! # A bounded read, not a discovery walk
//!
//! This module does not use the discovery crate's walkers. It looks at the
//! newest day directory (and the day before it, for a session that started
//! before midnight), picks the file in it with the newest modification time,
//! and reads only that file's last 64 KiB. A rollout file can grow to
//! several megabytes over a long session; this module only ever wants the
//! last line that matters.
//!
//! # Why this is a seed, not a source in its own right
//!
//! A rollout event carries only the account-wide window, not the
//! model-scoped `additional_rate_limits` the live endpoint also returns.
//! Swapping between the two shapes on every poll would make rows come and go
//! on screen, so [`super::codex_fetch`] only ever reaches for a rollout
//! reading when both its own attempts have failed. See that module's doc
//! under "Seeding from the session log".

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::cooldown::MAX_AGE;
use crate::provider_usage::live::codex::is_sliding_reset_projection;
use crate::provider_usage::live::model::{UsageScope, UsageWindow, UsageWindowKind, WindowRole};

/// The tail of a rollout file this module reads, in bytes. Wide enough to
/// hold many turns' worth of `token_count` events, small enough that even a
/// multi-megabyte session log costs one bounded read.
const TAIL_BYTES: u64 = 64 * 1024;

/// The newest rate-limit reading a Codex rollout file holds.
#[derive(Debug, Clone, PartialEq)]
pub struct RolloutReading {
    /// When the CLI observed these figures — the reading's own timestamp,
    /// not when this module read the file.
    pub observed_at: OffsetDateTime,
    /// The plan label, when the event stated one.
    pub plan: Option<String>,
    /// The account-wide windows the event reported.
    pub windows: Vec<UsageWindow>,
}

/// The newest rate-limit reading a Codex rollout file on this machine holds,
/// when one is recent enough to describe the present.
///
/// `sessions_root` is `$CODEX_HOME/sessions` or `~/.codex/sessions`.
/// `now` is compared against both the chosen file's modification time and
/// the reading's own timestamp; either one older than `MAX_AGE` (one hour)
/// makes the result `None`.
pub fn latest_reading(sessions_root: &Path, now: OffsetDateTime) -> Option<RolloutReading> {
    let file = newest_rollout_file(sessions_root)?;
    let modified_at = mtime(&file)?;
    if now - modified_at > MAX_AGE {
        return None;
    }
    let tail = read_tail(&file)?;
    let line = find_reading_line(&tail)?;
    let timestamp = line.get("timestamp").and_then(Value::as_str)?;
    let observed_at = OffsetDateTime::parse(timestamp, &Rfc3339).ok()?;
    if now - observed_at > MAX_AGE {
        return None;
    }
    let rate_limits = line.get("payload")?.get("rate_limits")?;
    let windows = parse_windows(rate_limits, observed_at)?;
    if windows.is_empty() {
        return None;
    }
    let plan = rate_limits
        .get("plan_type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(RolloutReading {
        observed_at,
        plan,
        windows,
    })
}

/// A directory's immediate subdirectory names. Missing or unreadable reads
/// as empty, the same as an account with no rollouts yet.
fn subdir_names(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

/// The day directory name one calendar day before `day`, only when the
/// decrement stays inside the same month — a plain digit subtraction, not a
/// calendar computation, matching the bound this module documents.
fn previous_day_name(day: &str) -> Option<String> {
    let number: u32 = day.parse().ok()?;
    number
        .checked_sub(1)
        .filter(|n| *n > 0)
        .map(|n| format!("{n:02}"))
}

/// The day directories to look in: the newest one under `sessions_root`, and
/// the day before it when that directory also exists in the same month.
fn candidate_day_dirs(sessions_root: &Path) -> Vec<PathBuf> {
    let Some(year) = subdir_names(sessions_root).into_iter().max() else {
        return Vec::new();
    };
    let year_dir = sessions_root.join(year);
    let Some(month) = subdir_names(&year_dir).into_iter().max() else {
        return Vec::new();
    };
    let month_dir = year_dir.join(month);
    let days = subdir_names(&month_dir);
    let Some(newest_day) = days.iter().max() else {
        return Vec::new();
    };
    let mut dirs = vec![month_dir.join(newest_day)];
    if let Some(previous) = previous_day_name(newest_day)
        && days.iter().any(|day| day == &previous)
    {
        dirs.push(month_dir.join(previous));
    }
    dirs
}

/// The `rollout-*.jsonl` file with the newest modification time, across the
/// candidate day directories.
fn newest_rollout_file(sessions_root: &Path) -> Option<PathBuf> {
    candidate_day_dirs(sessions_root)
        .iter()
        .flat_map(|dir| fs::read_dir(dir).into_iter().flatten())
        .filter_map(|entry| entry.ok())
        .filter(|entry| is_rollout_file_name(&entry.file_name()))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((entry.path(), modified))
        })
        .max_by_key(|(_, modified)| *modified)
        .map(|(path, _)| path)
}

fn is_rollout_file_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
}

fn mtime(path: &Path) -> Option<OffsetDateTime> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let since_epoch = modified.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    OffsetDateTime::from_unix_timestamp(since_epoch.as_secs() as i64).ok()
}

/// The last [`TAIL_BYTES`] of `path`, with a leading partial line dropped
/// when the read did not start at the file's own beginning.
fn read_tail(path: &Path) -> Option<Vec<u8>> {
    let mut file = fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    if start > 0 {
        match buffer.iter().position(|byte| *byte == b'\n') {
            Some(newline) => buffer.drain(..=newline),
            // No newline in the whole tail: the entire read is one partial
            // line, so there is nothing usable in it.
            None => buffer.drain(..),
        };
    }
    Some(buffer)
}

/// The newest line in `tail` that is an `event_msg` carrying a `token_count`
/// event with a non-null `rate_limits` whose `limit_id` is `"codex"`, null,
/// or absent — the account-wide bucket. A line that is not JSON, or does
/// not match, is skipped rather than treated as an error.
fn find_reading_line(tail: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(tail).ok()?;
    for line in text.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        let payload = value.get("payload");
        if payload
            .and_then(|payload| payload.get("type"))
            .and_then(Value::as_str)
            != Some("token_count")
        {
            continue;
        }
        let Some(rate_limits) = payload
            .and_then(|payload| payload.get("rate_limits"))
            .filter(|value| !value.is_null())
        else {
            continue;
        };
        let limit_id = rate_limits.get("limit_id").and_then(Value::as_str);
        if !matches!(limit_id, None | Some("codex")) {
            continue;
        }
        return Some(value);
    }
    None
}

/// `primary` then `secondary` from a `rate_limits` object, each turned into
/// an account-wide [`UsageWindow`]. Any window missing its duration or
/// percentage, or carrying a percentage outside `0..=100`, fails the whole
/// reading — the same fail-closed rule every other parser here uses.
fn parse_windows(rate_limits: &Value, observed_at: OffsetDateTime) -> Option<Vec<UsageWindow>> {
    let mut windows = Vec::new();
    for key in ["primary", "secondary"] {
        if let Some(window) = rate_limits.get(key).filter(|value| !value.is_null()) {
            windows.push(parse_window(window, observed_at)?);
        }
    }
    Some(windows)
}

fn parse_window(value: &Value, observed_at: OffsetDateTime) -> Option<UsageWindow> {
    let used_percent = value.get("used_percent").and_then(Value::as_f64)?;
    if !(0.0..=100.0).contains(&used_percent) {
        return None;
    }
    let minutes = value
        .get("window_minutes")
        .and_then(Value::as_f64)
        .filter(|minutes| minutes.is_finite() && *minutes > 0.0)
        .map(|minutes| minutes.round() as i64)?;

    let (id, role, kind) = match minutes {
        10_080 => (
            "seven-day".to_string(),
            WindowRole::PrimaryLong,
            UsageWindowKind::Weekly,
        ),
        300 => (
            "five-hour".to_string(),
            WindowRole::PrimaryShort,
            UsageWindowKind::Rolling,
        ),
        _ => (
            format!("rolling-{minutes}m"),
            WindowRole::PrimaryShort,
            UsageWindowKind::Rolling,
        ),
    };

    let resets_at = value
        .get("resets_at")
        .and_then(Value::as_i64)
        .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
        .filter(|reset| {
            !is_sliding_reset_projection(*reset, observed_at, minutes * 60, used_percent)
        });

    Some(UsageWindow {
        id,
        role,
        kind,
        scope: UsageScope::Account,
        used_percent: Some(used_percent),
        starts_at: None,
        resets_at,
        authoritative: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rfc3339(at: OffsetDateTime) -> String {
        at.format(&Rfc3339).expect("format")
    }

    /// One `event_msg`/`token_count` line, with the given `rate_limits`
    /// value (pass [`Value::Null`] for a `token_count` event with none).
    fn token_count_line(timestamp: OffsetDateTime, rate_limits: Value) -> String {
        json!({
            "timestamp": rfc3339(timestamp),
            "ordinal": 1,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {},
                "rate_limits": rate_limits,
            },
        })
        .to_string()
    }

    fn window(used_percent: f64, window_minutes: i64, resets_at: Option<i64>) -> Value {
        json!({
            "used_percent": used_percent,
            "window_minutes": window_minutes,
            "resets_at": resets_at,
        })
    }

    fn rate_limits(limit_id: Option<&str>, primary: Option<Value>, plan: Option<&str>) -> Value {
        json!({
            "limit_id": limit_id,
            "primary": primary,
            "secondary": Value::Null,
            "plan_type": plan,
        })
    }

    /// Writes `sessions_root/<year>/<month>/<day>/<name>` with `contents`,
    /// creating every parent directory.
    fn write_rollout(
        sessions_root: &Path,
        year: &str,
        month: &str,
        day: &str,
        name: &str,
        contents: &str,
    ) -> PathBuf {
        let dir = sessions_root.join(year).join(month).join(day);
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join(name);
        fs::write(&path, contents).expect("write");
        path
    }

    /// `now`'s own year, month, and day, zero-padded — the directory names a
    /// rollout for `now` is filed under.
    fn safe_date(now: OffsetDateTime) -> (String, String, String) {
        (
            format!("{:04}", now.year()),
            format!("{:02}", u8::from(now.month())),
            format!("{:02}", now.day()),
        )
    }

    #[test]
    fn the_newest_qualifying_event_is_read_from_the_newest_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_root = dir.path();
        let now = OffsetDateTime::now_utc();
        let (year, month, day) = safe_date(now);
        let observed_at = now;
        let line = token_count_line(
            observed_at,
            rate_limits(
                Some("codex"),
                Some(window(20.0, 10_080, Some(1_788_758_562))),
                Some("pro"),
            ),
        );
        write_rollout(sessions_root, &year, &month, &day, "rollout-a.jsonl", &line);

        let reading = latest_reading(sessions_root, now).expect("a reading");
        assert_eq!(reading.plan.as_deref(), Some("pro"));
        assert_eq!(reading.observed_at, observed_at);
        assert_eq!(reading.windows.len(), 1);
        assert_eq!(reading.windows[0].id, "seven-day");
        assert_eq!(reading.windows[0].used_percent, Some(20.0));
        assert_eq!(
            reading.windows[0].resets_at,
            OffsetDateTime::from_unix_timestamp(1_788_758_562).ok()
        );
    }

    #[test]
    fn an_event_with_null_rate_limits_after_a_good_one_is_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_root = dir.path();
        let now = OffsetDateTime::now_utc();
        let (year, month, day) = safe_date(now);
        let good = token_count_line(
            now - time::Duration::seconds(10),
            rate_limits(Some("codex"), Some(window(20.0, 10_080, None)), Some("pro")),
        );
        let null_reading = token_count_line(now, Value::Null);
        let contents = format!("{good}\n{null_reading}\n");
        write_rollout(
            sessions_root,
            &year,
            &month,
            &day,
            "rollout-a.jsonl",
            &contents,
        );

        let reading = latest_reading(sessions_root, now).expect("the good reading");
        assert_eq!(reading.windows[0].used_percent, Some(20.0));
    }

    #[test]
    fn an_event_with_a_different_limit_id_is_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_root = dir.path();
        let now = OffsetDateTime::now_utc();
        let (year, month, day) = safe_date(now);
        let good = token_count_line(
            now - time::Duration::seconds(10),
            rate_limits(Some("codex"), Some(window(20.0, 10_080, None)), Some("pro")),
        );
        let other = token_count_line(
            now,
            rate_limits(Some("other"), Some(window(99.0, 10_080, None)), Some("pro")),
        );
        let contents = format!("{good}\n{other}\n");
        write_rollout(
            sessions_root,
            &year,
            &month,
            &day,
            "rollout-a.jsonl",
            &contents,
        );

        let reading = latest_reading(sessions_root, now).expect("the good reading");
        assert_eq!(reading.windows[0].used_percent, Some(20.0));
    }

    #[test]
    fn an_out_of_range_percentage_makes_the_reading_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_root = dir.path();
        let now = OffsetDateTime::now_utc();
        let (year, month, day) = safe_date(now);
        let line = token_count_line(
            now,
            rate_limits(
                Some("codex"),
                Some(window(140.0, 10_080, None)),
                Some("pro"),
            ),
        );
        write_rollout(sessions_root, &year, &month, &day, "rollout-a.jsonl", &line);

        assert_eq!(latest_reading(sessions_root, now), None);
    }

    #[test]
    fn an_event_older_than_one_hour_makes_the_reading_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_root = dir.path();
        let now = OffsetDateTime::now_utc();
        let (year, month, day) = safe_date(now);
        let stale = now - time::Duration::hours(2);
        let line = token_count_line(
            stale,
            rate_limits(Some("codex"), Some(window(20.0, 10_080, None)), Some("pro")),
        );
        write_rollout(sessions_root, &year, &month, &day, "rollout-a.jsonl", &line);

        assert_eq!(latest_reading(sessions_root, now), None);
    }

    #[test]
    fn a_file_whose_token_count_events_never_carry_rate_limits_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_root = dir.path();
        let now = OffsetDateTime::now_utc();
        let (year, month, day) = safe_date(now);
        let line = token_count_line(now, Value::Null);
        write_rollout(sessions_root, &year, &month, &day, "rollout-a.jsonl", &line);

        assert_eq!(latest_reading(sessions_root, now), None);
    }

    #[test]
    fn a_good_event_near_the_end_of_a_long_file_is_still_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_root = dir.path();
        let now = OffsetDateTime::now_utc();
        let (year, month, day) = safe_date(now);
        let filler = filler_lines(200_000);
        let good = token_count_line(
            now,
            rate_limits(Some("codex"), Some(window(33.0, 10_080, None)), Some("pro")),
        );
        let contents = format!("{filler}{good}\n");
        write_rollout(
            sessions_root,
            &year,
            &month,
            &day,
            "rollout-a.jsonl",
            &contents,
        );

        let reading = latest_reading(sessions_root, now).expect("a reading within the tail");
        assert_eq!(reading.windows[0].used_percent, Some(33.0));
    }

    #[test]
    fn a_good_event_outside_the_tail_window_is_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_root = dir.path();
        let now = OffsetDateTime::now_utc();
        let (year, month, day) = safe_date(now);
        let good = token_count_line(
            now,
            rate_limits(Some("codex"), Some(window(33.0, 10_080, None)), Some("pro")),
        );
        let filler = filler_lines(199_000);
        let contents = format!("{good}\n{filler}");
        write_rollout(
            sessions_root,
            &year,
            &month,
            &day,
            "rollout-a.jsonl",
            &contents,
        );

        assert_eq!(latest_reading(sessions_root, now), None);
    }

    #[test]
    fn a_five_hour_primary_window_is_identified_by_its_own_minutes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_root = dir.path();
        let now = OffsetDateTime::now_utc();
        let (year, month, day) = safe_date(now);
        let line = token_count_line(
            now,
            rate_limits(Some("codex"), Some(window(10.0, 300, None)), None),
        );
        write_rollout(sessions_root, &year, &month, &day, "rollout-a.jsonl", &line);

        let reading = latest_reading(sessions_root, now).expect("a reading");
        assert_eq!(reading.windows[0].id, "five-hour");
        assert_eq!(reading.windows[0].role, WindowRole::PrimaryShort);
        assert_eq!(reading.windows[0].kind, UsageWindowKind::Rolling);
    }

    #[test]
    fn an_uncommon_window_length_gets_a_minutes_derived_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_root = dir.path();
        let now = OffsetDateTime::now_utc();
        let (year, month, day) = safe_date(now);
        let line = token_count_line(
            now,
            rate_limits(Some("codex"), Some(window(10.0, 60, None)), None),
        );
        write_rollout(sessions_root, &year, &month, &day, "rollout-a.jsonl", &line);

        let reading = latest_reading(sessions_root, now).expect("a reading");
        assert_eq!(reading.windows[0].id, "rolling-60m");
    }

    #[test]
    fn a_zero_usage_reset_landing_on_the_projected_boundary_is_dropped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions_root = dir.path();
        let now = OffsetDateTime::now_utc();
        let (year, month, day) = safe_date(now);
        let projected = (now + time::Duration::seconds(300 * 60)).unix_timestamp();
        let line = token_count_line(
            now,
            rate_limits(Some("codex"), Some(window(0.0, 300, Some(projected))), None),
        );
        write_rollout(sessions_root, &year, &month, &day, "rollout-a.jsonl", &line);

        let reading = latest_reading(sessions_root, now).expect("a reading");
        assert_eq!(reading.windows[0].used_percent, Some(0.0));
        assert_eq!(reading.windows[0].resets_at, None);
    }

    /// Many small, valid, non-matching lines — enough to push a real event
    /// past this module's tail-read bound in the tests that document it.
    fn filler_lines(total_bytes: usize) -> String {
        let line = json!({
            "timestamp": "2026-01-01T00:00:00Z",
            "ordinal": 0,
            "type": "session_meta",
            "payload": {"pad": "x".repeat(80)},
        })
        .to_string();
        let per_line = line.len() + 1;
        let count = total_bytes.div_ceil(per_line);
        let mut result = String::with_capacity(count * per_line);
        for _ in 0..count {
            result.push_str(&line);
            result.push('\n');
        }
        result
    }

    #[test]
    fn an_empty_sessions_root_yields_no_reading() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(latest_reading(dir.path(), OffsetDateTime::now_utc()), None);
    }

    #[test]
    fn previous_day_name_only_decrements_within_the_month() {
        assert_eq!(previous_day_name("05"), Some("04".to_string()));
        assert_eq!(previous_day_name("01"), None);
    }
}
