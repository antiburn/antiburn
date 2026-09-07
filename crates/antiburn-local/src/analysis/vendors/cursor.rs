//! Cursor adapter for CLI transcripts and synthesized IDE/store sessions.

use std::io::BufReader;

use anyhow::Context;
use serde_json::Value;
use time::{Date, Month, PrimitiveDateTime, Time, UtcOffset};

use super::read_source;
use crate::analysis::framing::{BoundedJsonlReader, FramedRecord, RecordSkip};
use crate::analysis::interface::{
    ContextWindowSource, NormalizedRecord, RawSource, RecordSink, SessionInput, SessionReader,
    SessionSummary, VisitOutcome,
};
use crate::analysis::model::{NormalizedEvent, NormalizedSession};
use crate::analysis::records::{RecordShape, parse_record, parse_ts};
use crate::analysis::source_validity::{AppendOnlyGuarantee, PinnedSource, SourceClaim};

pub struct CursorSessionReader;

impl SessionReader for CursorSessionReader {
    fn agent(&self) -> &'static str {
        "cursor"
    }

    fn capabilities(
        &self,
        source: &crate::analysis::RawSource,
    ) -> crate::analysis::SourceCapabilities {
        cursor_capabilities(source)
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        let content = read_source(&input.source)
            .with_context(|| format!("reading Cursor session {}", input.session_id))?;
        let (events, model) = parse_cursor(&content);
        Ok(NormalizedSession {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            events,
            cache_write_tokens_available: false,
            context_window: None,
            context_window_source: ContextWindowSource::Inferred,
            model,
        })
    }

    fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        guarantee: AppendOnlyGuarantee,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        let RawSource::File(path) = &input.source else {
            anyhow::bail!("a claimed Cursor source must be a file");
        };
        let mut pinned = match PinnedSource::open(path, claim.clone())? {
            Ok(pinned) => pinned,
            Err(reason) => return Ok(VisitOutcome::SourceChanged(reason)),
        };
        let limit = match guarantee {
            AppendOnlyGuarantee::Evidenced => claim.boundary,
            AppendOnlyGuarantee::Absent => u64::MAX,
        };
        let model = visit_cursor_reader(BufReader::new(pinned.reader(limit)), cancel, sink)?;
        let outcome = match guarantee {
            AppendOnlyGuarantee::Evidenced => pinned.recheck_prefix()?.map_or(
                VisitOutcome::AcceptedPrefix {
                    boundary: claim.boundary,
                },
                VisitOutcome::SourceChanged,
            ),
            AppendOnlyGuarantee::Absent => pinned
                .recheck_full()?
                .map_or(VisitOutcome::AcceptedFull, VisitOutcome::SourceChanged),
        };
        if !matches!(outcome, VisitOutcome::SourceChanged(_)) {
            sink.finish(SessionSummary {
                model,
                ..SessionSummary::default()
            });
        }
        Ok(outcome)
    }
}

fn cursor_capabilities(source: &RawSource) -> crate::analysis::SourceCapabilities {
    use crate::analysis::{SourceCapabilities, SourceFormat};

    let format = match source {
        RawSource::Sqlite(_) => SourceFormat::CursorCliStoreDb,
        RawSource::File(path)
            if path.extension().and_then(|value| value.to_str()) == Some("json") =>
        {
            SourceFormat::CursorLegacyChatJson
        }
        RawSource::File(_) => SourceFormat::CursorCliAgentJsonl,
        RawSource::Jsonl(content)
            if content.contains("\"cursor_source\":\"desktop_state_vscdb\"") =>
        {
            SourceFormat::CursorIdeComposer
        }
        RawSource::Jsonl(content) if content.contains("\"cursor_source\":\"store_db\"") => {
            SourceFormat::CursorCliStoreDb
        }
        RawSource::Jsonl(content) if content.contains("\"cursor_source\":\"agent_transcript\"") => {
            SourceFormat::CursorCliAgentJsonl
        }
        RawSource::Jsonl(_) => SourceFormat::CursorJsonl,
    };
    if matches!(format, SourceFormat::CursorLegacyChatJson) {
        SourceCapabilities::uncharacterized(format)
    } else {
        SourceCapabilities {
            source_format: format,
            ..SourceCapabilities::cursor()
        }
    }
}

fn visit_cursor_reader(
    reader: impl std::io::BufRead,
    cancel: &dyn Fn() -> bool,
    sink: &mut dyn RecordSink,
) -> anyhow::Result<Option<String>> {
    let mut reader = BoundedJsonlReader::new(reader);
    let mut session_model = None;
    while let Some(record) = reader.next_record(cancel) {
        match record {
            FramedRecord::Complete { bytes, .. } => {
                let record = std::str::from_utf8(bytes).context("Cursor record is not UTF-8")?;
                let Ok(value) = serde_json::from_str::<Value>(record) else {
                    sink.record(NormalizedRecord::Unusable(
                        crate::analysis::PartialReason::MalformedRecord,
                    ));
                    continue;
                };
                if session_model.is_none() {
                    session_model = model_from(&value).map(str::to_owned);
                }
                let Some(mut event) = parse_record(&value, RecordShape::Cursor) else {
                    sink.record(NormalizedRecord::Unusable(
                        crate::analysis::PartialReason::UnrecognizedRecordType,
                    ));
                    continue;
                };
                if event.ts_ms.is_none() {
                    event.ts_ms = embedded_timestamp(&value);
                }
                if event.model.is_none() {
                    event.model = session_model.clone();
                }
                if event.uuid.is_none() {
                    event.uuid = cursor_record_id(&value).map(str::to_owned);
                }
                sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
            }
            FramedRecord::Skipped(RecordSkip::ReadFailed { index, kind }) => {
                anyhow::bail!("Cursor record {index} read failed: {kind:?}");
            }
            FramedRecord::Skipped(RecordSkip::Cancelled { index }) => {
                anyhow::bail!("Cursor record {index} read was cancelled");
            }
            FramedRecord::Skipped(skip) => {
                sink.record(NormalizedRecord::Unusable(skip.partial_reason()));
            }
        }
    }
    Ok(session_model)
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
        let Some(mut event) = parse_record(&value, RecordShape::Cursor) else {
            continue;
        };
        if event.ts_ms.is_none() {
            event.ts_ms = embedded_timestamp(&value);
        }
        if event.model.is_none() {
            event.model = session_model.clone();
        }
        if event.uuid.is_none() {
            event.uuid = cursor_record_id(&value).map(str::to_owned);
        }
        events.push(event);
    }
    (events, session_model)
}

fn cursor_record_id(value: &Value) -> Option<&str> {
    ["bubbleId", "messageId", "id"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
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
