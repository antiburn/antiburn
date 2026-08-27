//! Cursor adapter for CLI transcripts and synthesized IDE/store sessions.

use anyhow::Context;
use serde_json::Value;
use time::{Date, Month, PrimitiveDateTime, Time, UtcOffset};

use super::jsonl::{parse_record, parse_ts};
use super::read_source;
use crate::analysis::interface::{SessionInput, VendorAdapter};
use crate::analysis::model::{NormalizedEvent, NormalizedSession};

pub struct CursorAdapter;

impl VendorAdapter for CursorAdapter {
    fn agent(&self) -> &'static str {
        "cursor"
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        let content = read_source(&input.source)
            .with_context(|| format!("reading Cursor session {}", input.session_id))?;
        let (events, model) = parse_cursor(&content);
        Ok(NormalizedSession {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            events,
            cache_write_tokens_available: true,
            context_window: None,
            model,
        })
    }
}

fn parse_cursor(content: &str) -> (Vec<NormalizedEvent>, Option<String>) {
    let mut events = Vec::new();
    let mut session_model = None;
    for line in content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if session_model.is_none() {
            session_model = model_from(&value).map(str::to_string);
        }
        let Some(mut event) = parse_record(&value) else {
            continue;
        };
        if event.ts_ms.is_none() {
            event.ts_ms = embedded_timestamp(&value);
        }
        if event.model.is_none() {
            event.model = session_model.clone();
        }
        events.push(event);
    }
    (events, session_model)
}

fn model_from(value: &Value) -> Option<&str> {
    value
        .pointer("/message/model")
        .or_else(|| value.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("default"))
}

fn embedded_timestamp(value: &Value) -> Option<i64> {
    let content = value
        .pointer("/message/content")
        .or_else(|| value.get("content"))?;
    let text = match content {
        Value::String(text) => Some(text.as_str()),
        Value::Array(blocks) => blocks.iter().find_map(|block| {
            block
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| block.as_str())
        }),
        _ => None,
    }?;
    let raw = text
        .split_once("<timestamp>")?
        .1
        .split_once("</timestamp>")?
        .0
        .trim();
    parse_ts(&Value::String(raw.to_string())).or_else(|| parse_cursor_verbose_timestamp(raw))
}

fn parse_cursor_verbose_timestamp(raw: &str) -> Option<i64> {
    let (_, rest) = raw.trim().split_once(", ")?;
    let (datetime, offset) = rest.rsplit_once(" (")?;
    let offset = parse_utc_offset(offset.strip_suffix(')')?)?;
    let mut parts = datetime.split(", ");
    let month_day = parts.next()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    let clock = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let (month, day) = month_day.split_once(' ')?;
    let month = parse_month(month)?;
    let day = day.parse::<u8>().ok()?;
    let (time, period) = clock.split_once(' ')?;
    let (hour, minute) = time.split_once(':')?;
    let mut hour = hour.parse::<u8>().ok()?;
    let minute = minute.parse::<u8>().ok()?;
    match period {
        "AM" if hour == 12 => hour = 0,
        "PM" if hour < 12 => hour += 12,
        "AM" | "PM" => {}
        _ => return None,
    }
    let local = PrimitiveDateTime::new(
        Date::from_calendar_date(year, month, day).ok()?,
        Time::from_hms(hour, minute, 0).ok()?,
    );
    Some(local.assume_offset(offset).unix_timestamp_nanos() as i64 / 1_000_000)
}

fn parse_month(value: &str) -> Option<Month> {
    Some(match value {
        "Jan" | "January" => Month::January,
        "Feb" | "February" => Month::February,
        "Mar" | "March" => Month::March,
        "Apr" | "April" => Month::April,
        "May" => Month::May,
        "Jun" | "June" => Month::June,
        "Jul" | "July" => Month::July,
        "Aug" | "August" => Month::August,
        "Sep" | "September" => Month::September,
        "Oct" | "October" => Month::October,
        "Nov" | "November" => Month::November,
        "Dec" | "December" => Month::December,
        _ => return None,
    })
}

fn parse_utc_offset(value: &str) -> Option<UtcOffset> {
    let value = value.strip_prefix("UTC")?;
    if value.is_empty() {
        return UtcOffset::from_hms(0, 0, 0).ok();
    }
    let sign = match value.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let value = &value[1..];
    let (hours, minutes) = value.split_once(':').unwrap_or((value, "0"));
    UtcOffset::from_hms(
        sign * hours.parse::<i8>().ok()?,
        sign * minutes.parse::<i8>().ok()?,
        0,
    )
    .ok()
}
