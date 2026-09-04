//! Antigravity adapter.
//!
//! Antigravity transcripts are step-based and don't fit the generic JSONL
//! parser: every step carries an uppercase `type` (`USER_INPUT`,
//! `PLANNER_RESPONSE`, `CORTEX_STEP_TYPE_*`) and no lowercase `role`, so the
//! generic `resolve_role` drops every record and the session normalizes to
//! empty. This adapter understands the three shapes Antigravity actually writes:
//!
//! 1. **Brain transcript** (`RawSource::Jsonl` or a `.jsonl` file): steps like
//!    `{"type":"USER_INPUT","created_at":"…","content":"<USER_REQUEST>…"}` and
//!    `{"type":"PLANNER_RESPONSE","content":"…"}`.
//!
//! 2. **API cascade** (`RawSource::File`): a single JSON document
//!    `{"sessionId":…,"source":"antigravity_api","steps":<LSP response>}` whose
//!    steps live at the nested pointer `/steps/steps`, each shaped like
//!    `{"type":"CORTEX_STEP_TYPE_USER_INPUT","status":"ok","userInput":{…}}`.
//!
//! 3. **Native database** (`RawSource::Sqlite`): one SQLite file per session.
//!    The brain transcript supplies activity and tools when present. Bounded
//!    protobuf rows supply usage, timestamps, retries, and models.
//!
//! Cascade documents are capped at 64 MiB. The Serde visitor ignores content
//! bodies and retains at most 256 KiB of analysis fields from one step.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::Context;
use rusqlite::{Connection, OpenFlags, params};
use serde::Deserializer;
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use crate::analysis::SourceChangedReason;
use crate::analysis::framing::{BoundedJsonlReader, FramedRecord, PartialReason, RecordSkip};
use crate::analysis::interface::{
    NormalizedRecord, RawSource, RecordSink, SessionCollector, SessionInput, SessionSummary,
    VendorAdapter, VisitOutcome,
};
use crate::analysis::model::{NormalizedEvent, NormalizedSession, Role, Usage};
use crate::analysis::records::{parse_ts, parse_usage, tool_call_from_input};
use crate::analysis::source_validity::{AppendOnlyGuarantee, PinnedSource, SourceClaim};
use crate::discovery::agents::antigravity::{
    combine_db_fingerprint, db_fingerprint_connection, sibling_brain_transcript,
};
use crate::discovery::source_version::{
    FINGERPRINT_HEAD_BYTES, FingerprintInputs, SourceStat, head_hash_of, provider_db_fingerprint,
};

const MAX_CASCADE_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CASCADE_RETAINED_STEP_BYTES: usize = 256 * 1024;
const MAX_CASCADE_TOOL_CALLS_PER_STEP: usize = 128;
const MAX_CASCADE_STRING_BYTES: usize = 4 * 1024;
const CASCADE_CANCEL_CHECK_BYTES: usize = 8 * 1024;
const CANCELLED_MESSAGE: &str = "Antigravity cascade read was cancelled";
const MAX_GEN_METADATA_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_ID_BYTES: usize = 512;
const MAX_MODEL_BYTES: usize = 256;
const MAX_TRACKED_RESPONSE_IDS: usize = 50_000;
const MAX_PROTO_FIELDS: usize = 16_384;

pub struct AntigravityAdapter;

impl VendorAdapter for AntigravityAdapter {
    fn agent(&self) -> &'static str {
        "antigravity"
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        let mut collector = SessionCollector::new(input.agent.clone(), input.session_id.clone());
        self.visit(input, &mut collector)?;
        collector.into_session()
    }

    fn visit(
        &self,
        input: &SessionInput,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        (|| -> anyhow::Result<VisitOutcome> {
            let summary = match &input.source {
                RawSource::File(path) => {
                    let file = File::open(path)?;
                    if is_jsonl_path(path) {
                        self.visit_jsonl(BufReader::new(file), &|| false, false, sink)?
                    } else {
                        self.visit_cascade(file, &|| false, sink)?
                    }
                }
                RawSource::Jsonl(content) => {
                    if is_cascade_content(content) {
                        self.visit_cascade(Cursor::new(content.as_bytes()), &|| false, sink)?
                    } else {
                        let suffix: &[u8] = if content.ends_with('\n') { b"" } else { b"\n" };
                        self.visit_jsonl(
                            BufReader::new(Cursor::new(content.as_bytes()).chain(suffix)),
                            &|| false,
                            false,
                            sink,
                        )?
                    }
                }
                RawSource::Sqlite(path) => {
                    self.visit_database(path, &input.session_id, &|| false, sink)?
                }
            };
            sink.finish(summary);
            Ok(VisitOutcome::Unvalidated)
        })()
        .with_context(|| format!("reading antigravity session {}", input.session_id))
    }

    fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        guarantee: AppendOnlyGuarantee,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        (|| -> anyhow::Result<VisitOutcome> {
            let RawSource::File(path) = &input.source else {
                anyhow::bail!("a claimed Antigravity file source must be a file");
            };
            let mut pinned = match PinnedSource::open(path, claim.clone())? {
                Ok(pinned) => pinned,
                Err(reason) => return Ok(VisitOutcome::SourceChanged(reason)),
            };
            let limit = match guarantee {
                AppendOnlyGuarantee::Evidenced => claim.boundary,
                AppendOnlyGuarantee::Absent => u64::MAX,
            };
            let summary = if is_jsonl_path(path) {
                self.visit_jsonl(BufReader::new(pinned.reader(limit)), cancel, false, sink)?
            } else {
                let model = self.probe_cascade_model(pinned.reader(limit), cancel)?;
                self.visit_cascade_with_model(pinned.reader(limit), cancel, sink, model)?
            };
            let outcome = match guarantee {
                AppendOnlyGuarantee::Evidenced => match pinned.recheck_prefix()? {
                    Some(reason) => VisitOutcome::SourceChanged(reason),
                    None => VisitOutcome::AcceptedPrefix {
                        boundary: claim.boundary,
                    },
                },
                AppendOnlyGuarantee::Absent => match pinned.recheck_full()? {
                    Some(reason) => VisitOutcome::SourceChanged(reason),
                    None => VisitOutcome::AcceptedFull,
                },
            };
            if matches!(outcome, VisitOutcome::SourceChanged(_)) {
                return Ok(outcome);
            }
            sink.finish(summary);
            Ok(outcome)
        })()
        .with_context(|| format!("reading claimed Antigravity session {}", input.session_id))
    }

    fn visit_db_claimed(
        &self,
        input: &SessionInput,
        claimed_fingerprint: &str,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        let RawSource::Sqlite(path) = &input.source else {
            anyhow::bail!("a claimed Antigravity database source must be SQLite");
        };
        let transcript_claim =
            sibling_brain_transcript(path, &input.session_id).and_then(|transcript| {
                claim_file(&transcript)
                    .ok()
                    .map(|claim| (transcript, claim))
            });
        let mut pinned_transcript = match &transcript_claim {
            Some((transcript, claim)) => match PinnedSource::open(transcript, claim.clone())? {
                Ok(pinned) => Some(pinned),
                Err(reason) => return Ok(VisitOutcome::SourceChanged(reason)),
            },
            None => None,
        };
        let mut summary = SessionSummary::default();
        if let Some(pinned) = pinned_transcript.as_mut() {
            summary =
                self.visit_jsonl(BufReader::new(pinned.reader(u64::MAX)), cancel, true, sink)?;
            if let Some(reason) = pinned.recheck_full()? {
                return Ok(VisitOutcome::SourceChanged(reason));
            }
        }
        let connection = open_database(path)?;
        connection.execute_batch("BEGIN")?;
        let actual = db_fingerprint_connection(&connection)
            .map(|database| {
                combine_db_fingerprint(
                    database,
                    transcript_claim
                        .as_ref()
                        .map(|(_, claim)| claim.fingerprint.as_str()),
                )
            })
            .map(|(latest, rows)| provider_db_fingerprint(latest, rows));
        if actual.as_deref() != Some(claimed_fingerprint) {
            return Ok(VisitOutcome::SourceChanged(
                SourceChangedReason::FingerprintMismatch,
            ));
        }
        let summary = self.visit_database_rows(&connection, summary, cancel, sink)?;
        if let Some(pinned) = pinned_transcript.as_mut()
            && let Some(reason) = pinned.recheck_full()?
        {
            return Ok(VisitOutcome::SourceChanged(reason));
        }
        connection.execute_batch("COMMIT")?;
        sink.finish(summary);
        Ok(VisitOutcome::AcceptedFull)
    }
}

impl AntigravityAdapter {
    fn visit_jsonl(
        &self,
        reader: impl BufRead,
        cancel: &dyn Fn() -> bool,
        suppress_usage: bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<SessionSummary> {
        let mut reader = BoundedJsonlReader::new(reader);
        let mut state = AntigravityStreamState::default();
        while let Some(record) = reader.next_record(cancel) {
            match record {
                FramedRecord::Skipped(skip) => match skip {
                    RecordSkip::Oversized { .. } | RecordSkip::IncompleteTail { .. } => {
                        sink.record(NormalizedRecord::Unusable(skip.partial_reason()));
                    }
                    RecordSkip::ReadFailed { index, kind } => {
                        anyhow::bail!("Antigravity record {index} read failed: {kind:?}");
                    }
                    RecordSkip::Cancelled { index } => {
                        anyhow::bail!("Antigravity record {index} read was cancelled");
                    }
                },
                FramedRecord::Complete { bytes, .. } => {
                    let record = std::str::from_utf8(bytes)
                        .context("Antigravity transcript record is not valid UTF-8")?;
                    let Ok(value) = serde_json::from_str::<Value>(record) else {
                        sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
                        continue;
                    };
                    state.observe(&value, suppress_usage, sink);
                }
            }
        }
        Ok(state.finish())
    }

    fn visit_database(
        &self,
        path: &Path,
        session_id: &str,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<SessionSummary> {
        let summary = match sibling_brain_transcript(path, session_id)
            .and_then(|transcript| File::open(transcript).ok())
        {
            Some(file) => self.visit_jsonl(BufReader::new(file), cancel, true, sink)?,
            None => SessionSummary::default(),
        };
        let connection = open_database(path)?;
        connection.execute_batch("BEGIN")?;
        let summary = self.visit_database_rows(&connection, summary, cancel, sink)?;
        connection.execute_batch("COMMIT")?;
        Ok(summary)
    }

    fn visit_database_rows(
        &self,
        connection: &Connection,
        mut summary: SessionSummary,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<SessionSummary> {
        let fallback_ts = summary.started_at_ms;
        let has_generations = table_exists(connection, "gen_metadata")?;
        let has_steps = table_exists(connection, "steps")?;
        if !has_generations && !has_steps {
            anyhow::bail!("Antigravity database has no usage tables");
        }
        let mut state = DatabaseUsageState::default();
        if has_generations {
            visit_generation_rows(
                connection,
                cancel,
                sink,
                |metadata, row_sink| {
                    for_each_chat_usage(metadata.data, |invocation| {
                        state.record_model(invocation.identity, metadata.model, row_sink);
                    })
                    .is_none_or(|scan| scan.malformed)
                },
                true,
            )?;
        }
        if has_steps {
            visit_step_rows(
                connection,
                fallback_ts,
                cancel,
                sink,
                &mut state,
                &mut summary,
            )?;
        }
        if has_generations {
            visit_generation_rows(
                connection,
                cancel,
                sink,
                |metadata, sink| {
                    for_each_chat_usage(metadata.data, |invocation| {
                        state.emit(
                            invocation,
                            Some(metadata.model),
                            fallback_ts,
                            sink,
                            &mut summary,
                        );
                    })
                    .is_none_or(|scan| scan.malformed)
                },
                false,
            )?;
        }
        summary.cache_write_tokens_available = true;
        Ok(summary)
    }

    fn visit_cascade(
        &self,
        mut reader: impl Read + Seek,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<SessionSummary> {
        let model = self.probe_cascade_model(&mut reader, cancel)?;
        reader.seek(SeekFrom::Start(0))?;
        self.visit_cascade_with_model_and_retained_high_water(reader, cancel, sink, model)
            .map(|(summary, _)| summary)
    }

    fn probe_cascade_model(
        &self,
        reader: impl Read,
        cancel: &dyn Fn() -> bool,
    ) -> anyhow::Result<Option<String>> {
        let cancelled = Cell::new(false);
        let mut limited = reader.take(MAX_CASCADE_DOCUMENT_BYTES + 1);
        let mut checked = CancelReader::new(&mut limited, cancel, &cancelled);
        let mut model = None;
        {
            let mut buffered = BufReader::with_capacity(CASCADE_CANCEL_CHECK_BYTES, &mut checked);
            let mut deserializer = serde_json::Deserializer::from_reader(&mut buffered);
            let _ = ModelProbeSeed { model: &mut model }
                .deserialize(&mut deserializer)
                .and_then(|()| deserializer.end());
        }
        if cancelled.get() {
            anyhow::bail!(CANCELLED_MESSAGE);
        }
        Ok(model)
    }

    fn visit_cascade_with_model(
        &self,
        reader: impl Read,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
        model: Option<String>,
    ) -> anyhow::Result<SessionSummary> {
        self.visit_cascade_with_model_and_retained_high_water(reader, cancel, sink, model)
            .map(|(summary, _)| summary)
    }

    #[cfg(test)]
    fn visit_cascade_with_retained_high_water(
        &self,
        reader: impl Read,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<(SessionSummary, usize)> {
        self.visit_cascade_with_model_and_retained_high_water(reader, cancel, sink, None)
    }

    fn visit_cascade_with_model_and_retained_high_water(
        &self,
        reader: impl Read,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
        model: Option<String>,
    ) -> anyhow::Result<(SessionSummary, usize)> {
        let mut state = AntigravityStreamState {
            model,
            ..AntigravityStreamState::default()
        };
        let retained_high_water = Cell::new(0);
        let cancelled = Cell::new(false);
        let mut limited = reader.take(MAX_CASCADE_DOCUMENT_BYTES + 1);
        let result = {
            let mut checked = CancelReader::new(&mut limited, cancel, &cancelled);
            let mut buffered = BufReader::with_capacity(CASCADE_CANCEL_CHECK_BYTES, &mut checked);
            let mut deserializer = serde_json::Deserializer::from_reader(&mut buffered);
            let result = CascadeSeed {
                state: &mut state,
                sink,
                cancel,
                cancelled: &cancelled,
                retained_high_water: &retained_high_water,
            }
            .deserialize(&mut deserializer);
            result.and_then(|()| deserializer.end())
        };
        if cancelled.get() {
            anyhow::bail!(CANCELLED_MESSAGE);
        }
        if limited.limit() == 0 {
            sink.record(NormalizedRecord::Unusable(PartialReason::Oversized));
        } else if result.is_err() {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
        }
        Ok((state.finish(), retained_high_water.get()))
    }
}

struct CancelReader<'a, R> {
    inner: R,
    cancel: &'a dyn Fn() -> bool,
    cancelled: &'a Cell<bool>,
}

impl<'a, R> CancelReader<'a, R> {
    fn new(inner: R, cancel: &'a dyn Fn() -> bool, cancelled: &'a Cell<bool>) -> Self {
        Self {
            inner,
            cancel,
            cancelled,
        }
    }
}

impl<R: Read> Read for CancelReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.cancelled.get() {
            return Err(io::Error::other(CANCELLED_MESSAGE));
        }
        if (self.cancel)() {
            self.cancelled.set(true);
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                CANCELLED_MESSAGE,
            ));
        }
        let limit = buffer.len().min(CASCADE_CANCEL_CHECK_BYTES);
        self.inner.read(&mut buffer[..limit])
    }
}

#[derive(Default)]
struct AntigravityStreamState {
    model: Option<String>,
    started_at_ms: Option<i64>,
    cascade_partial: bool,
}

impl AntigravityStreamState {
    fn observe(&mut self, value: &Value, suppress_usage: bool, sink: &mut dyn RecordSink) {
        if is_meta_line(value) {
            self.observe_model(value);
            return;
        }
        let Some(mut event) = step_to_event(value) else {
            return;
        };
        if suppress_usage {
            event.usage = Usage::default();
            if event.role == Role::Assistant {
                if event.tools.is_empty() {
                    return;
                }
                event.role = Role::Tool;
            }
        }
        self.observe_model(value);
        event.model = model_from(value).or_else(|| self.model.clone());
        if self.started_at_ms.is_none() {
            self.started_at_ms = event.ts_ms;
        }
        sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }

    fn observe_model(&mut self, value: &Value) {
        if let Some(model) = model_from(value) {
            self.model = Some(model);
        }
    }

    fn finish(self) -> SessionSummary {
        SessionSummary {
            cache_write_tokens_available: false,
            model: self.model,
            started_at_ms: self.started_at_ms,
            coverage_gaps: self
                .cascade_partial
                .then_some(PartialReason::Oversized)
                .into_iter()
                .collect(),
            ..SessionSummary::default()
        }
    }
}

fn model_from(value: &Value) -> Option<String> {
    value
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_owned)
}

fn open_database(path: &Path) -> anyhow::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening Antigravity database {}", path.display()))
}

fn claim_file(path: &Path) -> anyhow::Result<SourceClaim> {
    let mut file = File::open(path)?;
    let stat = SourceStat::from_open_std_file(&file)
        .ok_or_else(|| anyhow::anyhow!("cannot read transcript metadata"))?;
    let mut head = Vec::new();
    file.by_ref()
        .take(FINGERPRINT_HEAD_BYTES as u64)
        .read_to_end(&mut head)?;
    Ok(SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
        stat,
        head_hash: Some(head_hash_of(&head)),
    }))
}

fn table_exists(connection: &Connection, table: &str) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        params![table],
        |row| row.get(0),
    )
}

struct ChatMetadata<'a> {
    data: &'a [u8],
    model: &'a str,
}

#[derive(Clone, Copy)]
struct UsageInvocation {
    usage: Usage,
    identity: u64,
}

#[derive(Default)]
struct DatabaseUsageState {
    models: HashMap<u64, String>,
    seen: HashSet<u64>,
    model_saturated: bool,
    seen_saturated: bool,
    reported_limit: bool,
}

impl DatabaseUsageState {
    fn record_model(&mut self, identity: u64, model: &str, sink: &mut dyn RecordSink) {
        if self.models.contains_key(&identity) || self.model_saturated {
            return;
        }
        if self.models.len() == MAX_TRACKED_RESPONSE_IDS {
            self.model_saturated = true;
            self.report_limit(sink);
            return;
        }
        self.models.insert(identity, model.to_owned());
    }

    fn emit(
        &mut self,
        invocation: UsageInvocation,
        direct_model: Option<&str>,
        ts_ms: Option<i64>,
        sink: &mut dyn RecordSink,
        summary: &mut SessionSummary,
    ) {
        if self.seen_saturated || self.seen.contains(&invocation.identity) {
            return;
        }
        if self.seen.len() == MAX_TRACKED_RESPONSE_IDS {
            self.seen_saturated = true;
            self.report_limit(sink);
            return;
        }
        self.seen.insert(invocation.identity);
        let model = direct_model
            .map(str::to_owned)
            .or_else(|| self.models.get(&invocation.identity).cloned())
            .or_else(|| summary.model.clone());
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.ts_ms = ts_ms;
        event.usage = invocation.usage;
        event.model = model.clone();
        if model.is_some() {
            summary.model = model;
        }
        sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }

    fn report_limit(&mut self, sink: &mut dyn RecordSink) {
        if !self.reported_limit {
            self.reported_limit = true;
            sink.record(NormalizedRecord::Unusable(PartialReason::Oversized));
        }
    }
}

fn visit_generation_rows(
    connection: &Connection,
    cancel: &dyn Fn() -> bool,
    sink: &mut dyn RecordSink,
    mut visit: impl FnMut(ChatMetadata<'_>, &mut dyn RecordSink) -> bool,
    report_errors: bool,
) -> anyhow::Result<()> {
    let mut statement = connection.prepare(
        "SELECT CASE WHEN length(CAST(data AS BLOB)) <= ?1 THEN CAST(data AS BLOB) END,
                length(CAST(data AS BLOB))
           FROM gen_metadata
          ORDER BY idx",
    )?;
    let mut rows = statement.query(params![MAX_GEN_METADATA_BYTES as i64])?;
    while let Some(row) = rows.next()? {
        if cancel() {
            anyhow::bail!("Antigravity database read was cancelled");
        }
        let data_len = row.get::<_, Option<i64>>(1)?.unwrap_or(0).max(0) as usize;
        if data_len > MAX_GEN_METADATA_BYTES {
            if report_errors {
                sink.record(NormalizedRecord::Unusable(PartialReason::Oversized));
            }
            continue;
        }
        let Some(data) = row.get::<_, Option<Vec<u8>>>(0).ok().flatten() else {
            if report_errors {
                sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            }
            continue;
        };
        let Some(metadata) = find_chat_metadata(&data) else {
            if report_errors {
                sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            }
            continue;
        };
        if visit(metadata, sink) && report_errors {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
        }
    }
    Ok(())
}

fn visit_step_rows(
    connection: &Connection,
    fallback_ts: Option<i64>,
    cancel: &dyn Fn() -> bool,
    sink: &mut dyn RecordSink,
    state: &mut DatabaseUsageState,
    summary: &mut SessionSummary,
) -> anyhow::Result<()> {
    let mut statement = connection.prepare(
        "SELECT CASE WHEN length(CAST(metadata AS BLOB)) <= ?1 THEN CAST(metadata AS BLOB) END,
                length(CAST(metadata AS BLOB))
           FROM steps
          ORDER BY idx",
    )?;
    let mut rows = statement.query(params![MAX_GEN_METADATA_BYTES as i64])?;
    while let Some(row) = rows.next()? {
        if cancel() {
            anyhow::bail!("Antigravity database read was cancelled");
        }
        let data_len = row.get::<_, Option<i64>>(1)?.unwrap_or(0).max(0) as usize;
        if data_len > MAX_GEN_METADATA_BYTES {
            sink.record(NormalizedRecord::Unusable(PartialReason::Oversized));
            continue;
        }
        let Some(data) = row.get::<_, Option<Vec<u8>>>(0).ok().flatten() else {
            continue;
        };
        let Some((ts_ms, scan)) = for_each_step_usage(&data, |_| {}) else {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            continue;
        };
        if scan.malformed {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
        }
        let _ = for_each_step_usage(&data, |invocation| {
            state.emit(invocation, None, ts_ms.or(fallback_ts), sink, summary);
        });
    }
    Ok(())
}

fn find_chat_metadata(data: &[u8]) -> Option<ChatMetadata<'_>> {
    let mut fields = ProtoFields::new(data);
    while let Some((_field, wire)) = fields.next_field()? {
        if wire == 2 {
            let candidate = fields.read_bytes()?;
            if let Some(model) = chat_model(candidate)
                && for_each_chat_usage(candidate, |_| {}).is_some_and(|scan| scan.count > 0)
            {
                return Some(ChatMetadata {
                    data: candidate,
                    model,
                });
            }
        } else {
            fields.skip(wire)?;
        }
    }
    None
}

fn chat_model(data: &[u8]) -> Option<&str> {
    let mut fields = ProtoFields::new(data);
    let mut canonical = None;
    let mut fallback = None;
    while let Some((field, wire)) = fields.next_field()? {
        if matches!(field, 19 | 21) && wire == 2 {
            let bytes = fields.read_bytes()?;
            if let Some(model) = plausible_model(bytes) {
                if field == 19 {
                    canonical = Some(model);
                } else {
                    fallback = Some(model);
                }
            }
        } else {
            fields.skip(wire)?;
        }
    }
    canonical.or(fallback)
}

fn plausible_model(bytes: &[u8]) -> Option<&str> {
    if bytes.is_empty()
        || bytes.len() > MAX_MODEL_BYTES
        || !bytes.iter().all(|byte| matches!(byte, 0x20..=0x7e))
        || !bytes.iter().any(u8::is_ascii_alphanumeric)
    {
        return None;
    }
    std::str::from_utf8(bytes)
        .ok()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[derive(Clone, Copy)]
struct UsageScan {
    count: usize,
    malformed: bool,
}

fn for_each_chat_usage(data: &[u8], mut visit: impl FnMut(UsageInvocation)) -> Option<UsageScan> {
    let mut fields = ProtoFields::new(data);
    let mut count = 0;
    let mut malformed = false;
    while let Some((field, wire)) = fields.next_field()? {
        match (field, wire) {
            (4, 2) => {
                let data = fields.read_bytes()?;
                if let Some(invocation) = decode_usage(data) {
                    visit(invocation);
                    count += 1;
                } else {
                    malformed = true;
                }
            }
            (17, 2) => {
                let retry = fields.read_bytes()?;
                if let Some(invocation) = retry_usage(retry) {
                    visit(invocation);
                    count += 1;
                } else {
                    malformed = true;
                }
            }
            (_, wire) => {
                fields.skip(wire)?;
            }
        }
    }
    Some(UsageScan { count, malformed })
}

fn retry_usage(data: &[u8]) -> Option<UsageInvocation> {
    let mut fields = ProtoFields::new(data);
    let mut usage = None;
    while let Some((field, wire)) = fields.next_field()? {
        if field == 2 && wire == 2 {
            usage = decode_usage(fields.read_bytes()?);
        } else {
            fields.skip(wire)?;
        }
    }
    usage
}

fn for_each_step_usage(
    data: &[u8],
    mut visit: impl FnMut(UsageInvocation),
) -> Option<(Option<i64>, UsageScan)> {
    let mut fields = ProtoFields::new(data);
    let mut created_at = None;
    let mut started_at = None;
    let mut count = 0;
    let mut malformed_usage = false;
    while let Some((field, wire)) = fields.next_field()? {
        match (field, wire) {
            (1, 2) | (32, 2) => {
                let timestamp = decode_timestamp(fields.read_bytes()?);
                if timestamp.is_none() {
                    malformed_usage = true;
                }
                if field == 1 {
                    created_at = timestamp;
                } else {
                    started_at = timestamp;
                }
            }
            (9, 2) => match decode_usage(fields.read_bytes()?) {
                Some(invocation) => {
                    visit(invocation);
                    count += 1;
                }
                None => malformed_usage = true,
            },
            (28, 2) => match retry_usage(fields.read_bytes()?) {
                Some(invocation) => {
                    visit(invocation);
                    count += 1;
                }
                None => malformed_usage = true,
            },
            (_, wire) => fields.skip(wire)?,
        }
    }
    Some((
        created_at.or(started_at),
        UsageScan {
            count,
            malformed: malformed_usage,
        },
    ))
}

fn decode_timestamp(data: &[u8]) -> Option<i64> {
    let mut fields = ProtoFields::new(data);
    let mut seconds = None;
    let mut nanos = 0_u64;
    while let Some((field, wire)) = fields.next_field()? {
        if matches!(field, 1 | 2) && wire == 0 {
            let value = fields.read_varint()?;
            if field == 1 {
                seconds = Some(i64::try_from(value).ok()?);
            } else {
                nanos = value;
            }
        } else {
            fields.skip(wire)?;
        }
    }
    if nanos >= 1_000_000_000 {
        return None;
    }
    seconds?
        .checked_mul(1_000)?
        .checked_add(i64::try_from(nanos / 1_000_000).ok()?)
}

fn decode_usage(data: &[u8]) -> Option<UsageInvocation> {
    let mut fields = ProtoFields::new(data);
    let mut input_tokens = 0;
    let mut total_output = None;
    let mut cache_write = 0;
    let mut cache_read = 0;
    let mut thinking = None;
    let mut response = None;
    let mut message_id = None;
    let mut response_id = None;
    let mut provider_message_id = None;
    while let Some((field, wire)) = fields.next_field()? {
        match field {
            1..=5 | 9 | 10 if wire == 0 => {
                let value = fields.read_varint()?;
                match field {
                    1 => {}
                    2 => input_tokens = value,
                    3 => total_output = Some(value),
                    4 => cache_write = value,
                    5 => cache_read = value,
                    9 => thinking = Some(value),
                    10 => response = Some(value),
                    _ => unreachable!(),
                }
            }
            7 | 11 | 12 if wire == 2 => {
                let identity = fields.read_bytes()?;
                if identity.is_empty() || identity.len() > MAX_RESPONSE_ID_BYTES {
                    return None;
                }
                match field {
                    7 => message_id = Some(identity),
                    11 => response_id = Some(identity),
                    12 => provider_message_id = Some(identity),
                    _ => unreachable!(),
                }
            }
            _ => fields.skip(wire)?,
        }
    }
    let split_output = thinking.unwrap_or(0).checked_add(response.unwrap_or(0))?;
    let output_tokens = match total_output {
        Some(total) => {
            if (thinking.is_some() || response.is_some())
                && total != split_output
                && (total != 0 || split_output != 0)
            {
                return None;
            }
            total
        }
        None => split_output,
    };
    let identity = response_id.or(provider_message_id).or(message_id)?;
    Some(UsageInvocation {
        usage: Usage {
            input_tokens,
            output_tokens,
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_write,
        },
        identity: identity_hash(identity),
    })
}

/// A collision suppresses one usage event. It cannot cause duplicate accounting.
fn identity_hash(identity: &[u8]) -> u64 {
    identity.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

struct ProtoFields<'a> {
    data: &'a [u8],
    offset: usize,
    fields: usize,
}

impl<'a> ProtoFields<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            offset: 0,
            fields: 0,
        }
    }

    fn next_field(&mut self) -> Option<Option<(u64, u8)>> {
        if self.offset == self.data.len() {
            return Some(None);
        }
        self.fields = self.fields.checked_add(1)?;
        if self.fields > MAX_PROTO_FIELDS {
            return None;
        }
        let key = self.read_varint()?;
        let field = key >> 3;
        let wire = (key & 7) as u8;
        (field != 0).then_some(Some((field, wire)))
    }

    fn read_varint(&mut self) -> Option<u64> {
        let mut value = 0_u64;
        for shift in (0..=63).step_by(7) {
            let byte = *self.data.get(self.offset)?;
            self.offset += 1;
            if shift == 63 && byte > 1 {
                return None;
            }
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    fn read_bytes(&mut self) -> Option<&'a [u8]> {
        let length = usize::try_from(self.read_varint()?).ok()?;
        let end = self.offset.checked_add(length)?;
        let bytes = self.data.get(self.offset..end)?;
        self.offset = end;
        Some(bytes)
    }

    fn skip(&mut self, wire: u8) -> Option<()> {
        match wire {
            0 => {
                self.read_varint()?;
            }
            1 => self.offset = self.offset.checked_add(8)?,
            2 => {
                self.read_bytes()?;
            }
            3 => {
                let mut depth = 1_usize;
                while depth > 0 {
                    self.fields = self.fields.checked_add(1)?;
                    if self.fields > MAX_PROTO_FIELDS {
                        return None;
                    }
                    let key = self.read_varint()?;
                    if key >> 3 == 0 {
                        return None;
                    }
                    let nested_wire = (key & 7) as u8;
                    match nested_wire {
                        0 => {
                            self.read_varint()?;
                        }
                        1 => self.offset = self.offset.checked_add(8)?,
                        2 => {
                            self.read_bytes()?;
                        }
                        3 => depth = depth.checked_add(1)?,
                        4 => depth -= 1,
                        5 => self.offset = self.offset.checked_add(4)?,
                        _ => return None,
                    }
                    if self.offset > self.data.len() {
                        return None;
                    }
                }
            }
            5 => self.offset = self.offset.checked_add(4)?,
            _ => return None,
        }
        (self.offset <= self.data.len()).then_some(())
    }
}

fn is_jsonl_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
}

fn is_cascade_content(content: &str) -> bool {
    let mut has_steps = false;
    let mut deserializer = serde_json::Deserializer::from_str(content);
    let _ = CascadeShapeSeed(&mut has_steps).deserialize(&mut deserializer);
    has_steps
}

struct CascadeShapeSeed<'a>(&'a mut bool);

impl<'de> DeserializeSeed<'de> for CascadeShapeSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(CascadeShapeVisitor(self.0))
    }
}

struct CascadeShapeVisitor<'a>(&'a mut bool);

impl<'de> Visitor<'de> for CascadeShapeVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an Antigravity cascade object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "steps" => {
                    *self.0 = true;
                    map.next_value::<IgnoredAny>()?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

struct CascadeSeed<'a> {
    state: &'a mut AntigravityStreamState,
    sink: &'a mut dyn RecordSink,
    cancel: &'a dyn Fn() -> bool,
    cancelled: &'a Cell<bool>,
    retained_high_water: &'a Cell<usize>,
}

impl<'de> DeserializeSeed<'de> for CascadeSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(CascadeVisitor(self))
    }
}

struct CascadeVisitor<'a>(CascadeSeed<'a>);

impl<'de> Visitor<'de> for CascadeVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an Antigravity cascade object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        while let Some(key) = map.next_key::<String>()? {
            check_cancel(self.0.cancel, self.0.cancelled)?;
            match key.as_str() {
                "model" => {
                    self.0.state.model = map.next_value_seed(HeaderModelSeed(self.0.state))?;
                }
                "steps" => map.next_value_seed(StepsSeed {
                    state: self.0.state,
                    sink: self.0.sink,
                    cancel: self.0.cancel,
                    cancelled: self.0.cancelled,
                    retained_high_water: self.0.retained_high_water,
                })?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

struct ModelProbeSeed<'a> {
    model: &'a mut Option<String>,
}

impl<'de> DeserializeSeed<'de> for ModelProbeSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ModelProbeVisitor(self))
    }
}

struct ModelProbeVisitor<'a>(ModelProbeSeed<'a>);

impl<'de> Visitor<'de> for ModelProbeVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an Antigravity cascade object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        while let Some(key) = map.next_key::<String>()? {
            if key == "model" {
                let mut model = map.next_value_seed(ModelStringSeed)?;
                if let Some(value) = model.as_mut()
                    && value.len() > MAX_CASCADE_STRING_BYTES
                {
                    *value = bounded_string(value, MAX_CASCADE_STRING_BYTES);
                } else if let Some(value) = model.as_mut() {
                    value.shrink_to_fit();
                }
                *self.0.model = model.filter(|value| !value.trim().is_empty());
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}

struct ModelStringSeed;

impl<'de> DeserializeSeed<'de> for ModelStringSeed {
    type Value = Option<String>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(HeaderModelVisitor)
    }
}

struct HeaderModelSeed<'a>(&'a mut AntigravityStreamState);

impl<'de> DeserializeSeed<'de> for HeaderModelSeed<'_> {
    type Value = Option<String>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut model = deserializer.deserialize_any(HeaderModelVisitor)?;
        if let Some(value) = model.as_mut()
            && value.len() > MAX_CASCADE_STRING_BYTES
        {
            *value = bounded_string(value, MAX_CASCADE_STRING_BYTES);
            self.0.cascade_partial = true;
        } else if let Some(value) = model.as_mut() {
            value.shrink_to_fit();
        }
        Ok(model.filter(|model| !model.trim().is_empty()))
    }
}

struct HeaderModelVisitor;

impl<'de> Visitor<'de> for HeaderModelVisitor {
    type Value = Option<String>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an optional model string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(Some(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(Some(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element::<IgnoredAny>()?.is_some() {}
        Ok(None)
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(None)
    }
}

struct StepsSeed<'a> {
    state: &'a mut AntigravityStreamState,
    sink: &'a mut dyn RecordSink,
    cancel: &'a dyn Fn() -> bool,
    cancelled: &'a Cell<bool>,
    retained_high_water: &'a Cell<usize>,
}

impl<'de> DeserializeSeed<'de> for StepsSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StepsVisitor(self))
    }
}

struct StepsVisitor<'a>(StepsSeed<'a>);

impl<'de> Visitor<'de> for StepsVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an Antigravity steps array or object")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while let Some(step) = sequence.next_element_seed(StepSeed {
            cancel: self.0.cancel,
            cancelled: self.0.cancelled,
        })? {
            check_cancel(self.0.cancel, self.0.cancelled)?;
            self.0
                .retained_high_water
                .set(self.0.retained_high_water.get().max(step.retained_bytes));
            let partial = step.partial;
            if let Some(event) = step.into_event(self.0.state) {
                if self.0.state.started_at_ms.is_none() {
                    self.0.state.started_at_ms = event.ts_ms;
                }
                self.0
                    .sink
                    .record(NormalizedRecord::MetricsEvent(Box::new(event)));
            }
            if partial {
                self.0
                    .sink
                    .record(NormalizedRecord::Unusable(PartialReason::Oversized));
            }
        }
        Ok(())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        while let Some(key) = map.next_key::<String>()? {
            check_cancel(self.0.cancel, self.0.cancelled)?;
            if key == "steps" {
                map.next_value_seed(StepsSeed {
                    state: self.0.state,
                    sink: self.0.sink,
                    cancel: self.0.cancel,
                    cancelled: self.0.cancelled,
                    retained_high_water: self.0.retained_high_water,
                })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}

struct StepSeed<'a> {
    cancel: &'a dyn Fn() -> bool,
    cancelled: &'a Cell<bool>,
}

impl<'de> DeserializeSeed<'de> for StepSeed<'_> {
    type Value = CascadeStep;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StepVisitor {
            cancel: self.cancel,
            cancelled: self.cancelled,
        })
    }
}

struct StepVisitor<'a> {
    cancel: &'a dyn Fn() -> bool,
    cancelled: &'a Cell<bool>,
}

impl<'de> Visitor<'de> for StepVisitor<'_> {
    type Value = CascadeStep;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an Antigravity cascade step")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut step = CascadeStep::default();
        while let Some(key) = map.next_key::<String>()? {
            check_cancel(self.cancel, self.cancelled)?;
            match key.as_str() {
                "type" => step.raw_type = map.next_value_seed(RetainedStringSeed(&mut step))?,
                "created_at" => step.created_at = map.next_value_seed(TimestampSeed(&mut step))?,
                "timestamp" => step.timestamp = map.next_value_seed(TimestampSeed(&mut step))?,
                "createdAt" => {
                    step.created_at_camel = map.next_value_seed(TimestampSeed(&mut step))?
                }
                "metadata" => map.next_value_seed(MetadataSeed(&mut step))?,
                "model" => step.model = map.next_value_seed(RetainedStringSeed(&mut step))?,
                "usage" => {
                    let parsed = map.next_value_seed(UsageSeed)?;
                    step.usage = parsed.usage;
                    step.partial |= parsed.partial;
                }
                "tool_calls" => map.next_value_seed(ToolCallsSeed(&mut step))?,
                "toolName" | "tool_name" | "name" => {
                    let name = map.next_value_seed(RetainedStringSeed(&mut step))?;
                    if step.inline_tool_name.is_none() {
                        step.inline_tool_name = name;
                    }
                }
                "toolCall" | "tool" | "action" => {
                    let name = map.next_value_seed(NamedContainerSeed(&mut step))?;
                    if step.inline_tool_name.is_none() {
                        step.inline_tool_name = name;
                    }
                }
                "args" | "input" => {
                    if step.inline_input.is_none() {
                        step.inline_input = map.next_value_seed(ToolInputSeed(&mut step))?;
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                "content" | "userInput" => {
                    step.has_content = true;
                    map.next_value::<IgnoredAny>()?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(step)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(CascadeStep::default())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(CascadeStep::default())
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(CascadeStep::default())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(CascadeStep::default())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(CascadeStep::default())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(CascadeStep::default())
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(CascadeStep::default())
    }
}

#[derive(Default)]
struct CascadeStep {
    raw_type: Option<String>,
    created_at: Option<Value>,
    timestamp: Option<Value>,
    created_at_camel: Option<Value>,
    metadata_created_at: Option<Value>,
    metadata_started_at: Option<Value>,
    model: Option<String>,
    usage: Usage,
    tools: Vec<PendingTool>,
    inline_tool_name: Option<String>,
    inline_input: Option<Value>,
    has_content: bool,
    retained_bytes: usize,
    partial: bool,
}

impl CascadeStep {
    fn retain(&mut self, mut value: String) -> Option<String> {
        if value.len() > MAX_CASCADE_STRING_BYTES {
            value = bounded_string(&value, MAX_CASCADE_STRING_BYTES);
            self.partial = true;
        } else {
            value.shrink_to_fit();
        }
        if self.retained_bytes.saturating_add(value.capacity()) > MAX_CASCADE_RETAINED_STEP_BYTES {
            self.partial = true;
            return None;
        }
        self.retained_bytes = self.retained_bytes.saturating_add(value.capacity());
        Some(value)
    }

    fn push_tool(&mut self, tool: PendingTool) {
        if self.tools.len() == MAX_CASCADE_TOOL_CALLS_PER_STEP {
            self.partial = true;
            return;
        }
        self.tools.push(tool);
    }

    fn into_event(self, state: &mut AntigravityStreamState) -> Option<NormalizedEvent> {
        let kind = normalize_type(self.raw_type.as_deref().unwrap_or(""));
        let role = match role_for(&kind) {
            Some(role) => role,
            None if self.has_content => Role::Assistant,
            None => return None,
        };
        let mut event = NormalizedEvent::new(role);
        event.ts_ms = self
            .created_at
            .as_ref()
            .or(self.timestamp.as_ref())
            .or(self.created_at_camel.as_ref())
            .or(self.metadata_created_at.as_ref())
            .or(self.metadata_started_at.as_ref())
            .and_then(parse_ts);
        event.usage = self.usage;
        if let Some(model) = self.model.filter(|model| !model.trim().is_empty()) {
            state.model = Some(model.clone());
            event.model = Some(model);
        } else {
            event.model = state.model.clone();
        }
        for tool in self.tools {
            event
                .tools
                .push(tool_call_from_input(&tool.name, tool.input.as_ref()));
        }
        if event.tools.is_empty()
            && role == Role::Tool
            && let Some(name) = self.inline_tool_name
        {
            event
                .tools
                .push(tool_call_from_input(&name, self.inline_input.as_ref()));
        }
        Some(event)
    }
}

fn bounded_string(value: &str, max_bytes: usize) -> String {
    let mut boundary = max_bytes.min(value.len());
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_owned()
}

struct RetainedStringSeed<'a>(&'a mut CascadeStep);

impl<'de> DeserializeSeed<'de> for RetainedStringSeed<'_> {
    type Value = Option<String>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(RetainedStringVisitor(self.0))
    }
}

struct RetainedStringVisitor<'a>(&'a mut CascadeStep);

impl<'de> Visitor<'de> for RetainedStringVisitor<'_> {
    type Value = Option<String>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an optional retained string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(self.0.retain(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(self.0.retain(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        self.0.partial = true;
        Ok(None)
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        self.0.partial = true;
        Ok(None)
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        self.0.partial = true;
        Ok(None)
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        self.0.partial = true;
        Ok(None)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element::<IgnoredAny>()?.is_some() {}
        self.0.partial = true;
        Ok(None)
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        self.0.partial = true;
        Ok(None)
    }
}

struct TimestampSeed<'a>(&'a mut CascadeStep);

impl<'de> DeserializeSeed<'de> for TimestampSeed<'_> {
    type Value = Option<Value>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(TimestampVisitor(self.0))
    }
}

struct TimestampVisitor<'a>(&'a mut CascadeStep);

impl Visitor<'_> for TimestampVisitor<'_> {
    type Value = Option<Value>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a timestamp string or number")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(self.0.retain(value.to_owned()).map(Value::String))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(self.0.retain(value).map(Value::String))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Some(Value::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Some(Value::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        Ok(serde_json::Number::from_f64(value).map(Value::Number))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
}

struct MetadataSeed<'a>(&'a mut CascadeStep);

impl<'de> DeserializeSeed<'de> for MetadataSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(MetadataVisitor(self.0))
    }
}

struct MetadataVisitor<'a>(&'a mut CascadeStep);

impl<'de> Visitor<'de> for MetadataVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Antigravity step metadata")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "createdAt" => {
                    self.0.metadata_created_at = map.next_value_seed(TimestampSeed(self.0))?
                }
                "startedAt" => {
                    self.0.metadata_started_at = map.next_value_seed(TimestampSeed(self.0))?
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element::<IgnoredAny>()?.is_some() {}
        Ok(())
    }
}

#[derive(Default)]
struct UsageFields {
    input_tokens: Option<u64>,
    prompt_tokens: Option<u64>,
    input: Option<u64>,
    output_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    output: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cached_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_read: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_creation_tokens: Option<u64>,
    cache_write: Option<u64>,
}

impl UsageFields {
    fn finish(self) -> Usage {
        let cache_read_tokens = self
            .cache_read_input_tokens
            .or(self.cached_tokens)
            .or(self.cache_read_tokens)
            .or(self.cache_read)
            .unwrap_or(0);
        Usage {
            input_tokens: self
                .input_tokens
                .or_else(|| {
                    self.prompt_tokens
                        .map(|value| value.saturating_sub(cache_read_tokens))
                })
                .or(self.input)
                .unwrap_or(0),
            output_tokens: self
                .output_tokens
                .or(self.completion_tokens)
                .or(self.output)
                .unwrap_or(0),
            cache_read_tokens,
            cache_creation_tokens: self
                .cache_creation_input_tokens
                .or(self.cache_creation_tokens)
                .or(self.cache_write)
                .unwrap_or(0),
        }
    }
}

struct UsageSeed;

impl<'de> DeserializeSeed<'de> for UsageSeed {
    type Value = ParsedUsage;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UsageVisitor)
    }
}

struct UsageVisitor;

struct ParsedUsage {
    usage: Usage,
    partial: bool,
}

impl ParsedUsage {
    fn empty(partial: bool) -> Self {
        Self {
            usage: Usage::default(),
            partial,
        }
    }
}

impl<'de> Visitor<'de> for UsageVisitor {
    type Value = ParsedUsage;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an Antigravity usage object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut usage = UsageFields::default();
        let mut partial = false;
        while let Some(key) = map.next_key::<String>()? {
            let target = match key.as_str() {
                "input_tokens" => &mut usage.input_tokens,
                "prompt_tokens" => &mut usage.prompt_tokens,
                "input" => &mut usage.input,
                "output_tokens" => &mut usage.output_tokens,
                "completion_tokens" => &mut usage.completion_tokens,
                "output" => &mut usage.output,
                "cache_read_input_tokens" => &mut usage.cache_read_input_tokens,
                "cached_tokens" => &mut usage.cached_tokens,
                "cache_read_tokens" => &mut usage.cache_read_tokens,
                "cacheRead" => &mut usage.cache_read,
                "cache_creation_input_tokens" => &mut usage.cache_creation_input_tokens,
                "cache_creation_tokens" => &mut usage.cache_creation_tokens,
                "cacheWrite" => &mut usage.cache_write,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                    continue;
                }
            };
            let token = map.next_value_seed(TokenNumberSeed)?;
            *target = token.value;
            partial |= token.partial;
        }
        Ok(ParsedUsage {
            usage: usage.finish(),
            partial,
        })
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(ParsedUsage::empty(false))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(ParsedUsage::empty(false))
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(ParsedUsage::empty(true))
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(ParsedUsage::empty(true))
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(ParsedUsage::empty(true))
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(ParsedUsage::empty(true))
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(ParsedUsage::empty(true))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element::<IgnoredAny>()?.is_some() {}
        Ok(ParsedUsage::empty(true))
    }
}

struct TokenNumberSeed;

impl<'de> DeserializeSeed<'de> for TokenNumberSeed {
    type Value = ParsedToken;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(TokenNumberVisitor)
    }
}

struct TokenNumberVisitor;

struct ParsedToken {
    value: Option<u64>,
    partial: bool,
}

impl ParsedToken {
    fn valid(value: Option<u64>) -> Self {
        Self {
            value,
            partial: false,
        }
    }

    fn invalid() -> Self {
        Self {
            value: None,
            partial: true,
        }
    }
}

impl<'de> Visitor<'de> for TokenNumberVisitor {
    type Value = ParsedToken;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a non-negative token count")
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(ParsedToken::valid(Some(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        if value < 0 {
            Ok(ParsedToken::invalid())
        } else {
            Ok(ParsedToken::valid(Some(value as u64)))
        }
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> {
        const U64_EXCLUSIVE_UPPER_BOUND: f64 = 18_446_744_073_709_551_616.0;
        if value.is_finite()
            && value >= 0.0
            && value.fract() == 0.0
            && value < U64_EXCLUSIVE_UPPER_BOUND
        {
            Ok(ParsedToken::valid(Some(value as u64)))
        } else {
            Ok(ParsedToken::invalid())
        }
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(ParsedToken::valid(None))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(ParsedToken::valid(None))
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(ParsedToken::invalid())
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(ParsedToken::invalid())
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element::<IgnoredAny>()?.is_some() {}
        Ok(ParsedToken::invalid())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(ParsedToken::invalid())
    }
}

struct PendingTool {
    name: String,
    input: Option<Value>,
}

struct ToolCallsSeed<'a>(&'a mut CascadeStep);

impl<'de> DeserializeSeed<'de> for ToolCallsSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ToolCallsVisitor(self.0))
    }
}

struct ToolCallsVisitor<'a>(&'a mut CascadeStep);

impl<'de> Visitor<'de> for ToolCallsVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an Antigravity tool call array")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while let Some(tool) = sequence.next_element_seed(ToolSeed(self.0))? {
            if let Some(tool) = tool {
                self.0.push_tool(tool);
            }
        }
        Ok(())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(())
    }
}

struct ToolSeed<'a>(&'a mut CascadeStep);

impl<'de> DeserializeSeed<'de> for ToolSeed<'_> {
    type Value = Option<PendingTool>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ToolVisitor(self.0))
    }
}

struct ToolVisitor<'a>(&'a mut CascadeStep);

impl<'de> Visitor<'de> for ToolVisitor<'_> {
    type Value = Option<PendingTool>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an Antigravity tool call")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(self
            .0
            .retain(value.to_owned())
            .map(|name| PendingTool { name, input: None }))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(self
            .0
            .retain(value)
            .map(|name| PendingTool { name, input: None }))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut name = None;
        let mut input = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "name" => name = map.next_value_seed(RetainedStringSeed(self.0))?,
                "function" if name.is_none() => {
                    name = map.next_value_seed(NamedContainerSeed(self.0))?
                }
                "args" | "input" if input.is_none() => {
                    input = map.next_value_seed(ToolInputSeed(self.0))?
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(name.map(|name| PendingTool { name, input }))
    }
}

struct NamedContainerSeed<'a>(&'a mut CascadeStep);

impl<'de> DeserializeSeed<'de> for NamedContainerSeed<'_> {
    type Value = Option<String>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(NamedContainerVisitor(self.0))
    }
}

struct NamedContainerVisitor<'a>(&'a mut CascadeStep);

impl<'de> Visitor<'de> for NamedContainerVisitor<'_> {
    type Value = Option<String>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an object with a tool name")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut name = None;
        while let Some(key) = map.next_key::<String>()? {
            if key == "name" && name.is_none() {
                name = map.next_value_seed(RetainedStringSeed(self.0))?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(name)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(self.0.retain(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(self.0.retain(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        self.0.partial = true;
        Ok(None)
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        self.0.partial = true;
        Ok(None)
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        self.0.partial = true;
        Ok(None)
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        self.0.partial = true;
        Ok(None)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element::<IgnoredAny>()?.is_some() {}
        self.0.partial = true;
        Ok(None)
    }
}

struct ToolInputSeed<'a>(&'a mut CascadeStep);

impl<'de> DeserializeSeed<'de> for ToolInputSeed<'_> {
    type Value = Option<Value>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ToolInputVisitor(self.0))
    }
}

struct ToolInputVisitor<'a>(&'a mut CascadeStep);

impl<'de> Visitor<'de> for ToolInputVisitor<'_> {
    type Value = Option<Value>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded Antigravity tool input")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(self.0.retain(value.to_owned()).map(Value::String))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(self.0.retain(value).map(Value::String))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut object = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if matches!(
                key.as_str(),
                "command" | "cmd" | "skill" | "name" | "skill_name" | "skillName" | "path"
            ) {
                if let Some(value) = map.next_value_seed(RetainedStringSeed(self.0))? {
                    object.insert(key, Value::String(value));
                }
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(Some(Value::Object(object)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        self.0.partial = true;
        Ok(None)
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        self.0.partial = true;
        Ok(None)
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        self.0.partial = true;
        Ok(None)
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        self.0.partial = true;
        Ok(None)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element::<IgnoredAny>()?.is_some() {}
        self.0.partial = true;
        Ok(None)
    }
}

fn check_cancel<E>(cancel: &dyn Fn() -> bool, cancelled: &Cell<bool>) -> Result<(), E>
where
    E: serde::de::Error,
{
    if cancel() {
        cancelled.set(true);
        return Err(E::custom(CANCELLED_MESSAGE));
    }
    Ok(())
}

/// True for the synthesized header / wrapper lines (`{"sessionId":…,
/// "source":"antigravity_brain"…}`) that carry no step payload.
fn is_meta_line(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    if matches!(
        obj.get("source").and_then(|s| s.as_str()),
        Some("antigravity_brain") | Some("antigravity_api")
    ) {
        return true;
    }
    // A session-wrapper object: identifies a session but carries no step shape.
    obj.contains_key("sessionId")
        && !obj.contains_key("type")
        && !obj.contains_key("content")
        && !obj.contains_key("userInput")
}

/// Map one Antigravity step to a normalized event, or `None` when it carries no
/// analyzable signal.
fn step_to_event(step: &Value) -> Option<NormalizedEvent> {
    let obj = step.as_object()?;
    let raw_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let kind = normalize_type(raw_type);

    let has_content = obj.contains_key("content") || obj.contains_key("userInput");
    let role = match role_for(&kind) {
        Some(role) => role,
        None if has_content => Role::Assistant,
        None => return None,
    };

    let mut ev = NormalizedEvent::new(role);

    ev.ts_ms = obj
        .get("created_at")
        .or_else(|| obj.get("timestamp"))
        .or_else(|| obj.get("createdAt"))
        // API-cascade steps (from `GetCascadeTrajectorySteps`) nest the timestamp
        // under `metadata` rather than at the top level — without this every
        // event parses with no time, so the session shows "0 mins active".
        .or_else(|| obj.get("metadata").and_then(|m| m.get("createdAt")))
        .or_else(|| obj.get("metadata").and_then(|m| m.get("startedAt")))
        .and_then(parse_ts);

    // Antigravity attaches tool invocations as a `tool_calls[]` array on the
    // PLANNER_RESPONSE step (each `{name, args}`), with separate action-named
    // result steps (VIEW_FILE, GREP_SEARCH, …). The names live in `tool_calls`,
    // so read them off every step regardless of role.
    push_tool_calls(obj, &mut ev);
    // Fallback: a Tool-role step that names its tool inline (no tool_calls).
    if ev.tools.is_empty()
        && role == Role::Tool
        && let Some(name) = tool_name(obj)
    {
        let input = obj.get("args").or_else(|| obj.get("input"));
        ev.tools.push(tool_call_from_input(&name, input));
    }

    ev.usage = parse_usage(obj.get("usage"));

    Some(ev)
}

/// Strip a `CORTEX_STEP_TYPE_` prefix and uppercase, so the brain
/// (`USER_INPUT`) and cascade (`CORTEX_STEP_TYPE_USER_INPUT`) namings collapse
/// to one key.
fn normalize_type(raw: &str) -> String {
    raw.trim()
        .strip_prefix("CORTEX_STEP_TYPE_")
        .unwrap_or(raw)
        .to_ascii_uppercase()
}

/// Resolve a normalized type key to its role.
fn role_for(kind: &str) -> Option<Role> {
    if kind == "USER_INPUT" {
        return Some(Role::User);
    }
    if kind.contains("PLAN") || kind.contains("THINK") || kind.contains("REASON") {
        return Some(Role::Assistant);
    }
    // Action / tool-result step types: the generic `TOOL`, plus the concrete
    // action names Antigravity uses for tool *results* (VIEW_FILE,
    // LIST_DIRECTORY, GREP_SEARCH, RUN_COMMAND, CODE_ACTION, …). Errors are
    // tool-shaped too. The tool *names* come from `tool_calls[]` on the
    // preceding planner step, so these result steps do not count tools again.
    const TOOL_MARKERS: &[&str] = &[
        "TOOL",
        "ACTION",
        "FILE",
        "DIRECTORY",
        "SEARCH",
        "GREP",
        "VIEW",
        "LIST",
        "COMMAND",
        "RUN",
        "EDIT",
        "WRITE",
        "TERMINAL",
        "BROWSER",
        "PERMISSION",
        "ERROR",
    ];
    if TOOL_MARKERS.iter().any(|m| kind.contains(m)) {
        return Some(Role::Tool);
    }
    if kind.contains("RESPONSE") {
        return Some(Role::Assistant);
    }
    // Framing / bookkeeping steps (CONVERSATION_HISTORY, KNOWLEDGE_ARTIFACTS,
    // EPHEMERAL_MESSAGE, GENERIC, settings, …): keep as System so they're not
    // misread as assistant work, but still carry timestamps for duration.
    if kind.contains("SETTINGS")
        || kind.contains("SYSTEM")
        || kind.contains("META")
        || kind.contains("HISTORY")
        || kind.contains("KNOWLEDGE")
        || kind.contains("ARTIFACT")
        || kind.contains("EPHEMERAL")
        || kind.contains("MESSAGE")
        || kind == "GENERIC"
    {
        return Some(Role::System);
    }
    None
}

/// Push a `ToolCall` for each entry in a step's `tool_calls[]` array. Each entry
/// names the tool at `.name` (Antigravity) or `.function.name` (OpenAI-style),
/// or is a bare string.
fn push_tool_calls(obj: &serde_json::Map<String, Value>, ev: &mut NormalizedEvent) {
    let Some(calls) = obj.get("tool_calls").and_then(|c| c.as_array()) else {
        return;
    };
    for call in calls {
        let name = match call {
            Value::String(s) => Some(s.as_str()),
            _ => call
                .get("name")
                .or_else(|| call.get("function").and_then(|f| f.get("name")))
                .and_then(|n| n.as_str()),
        };
        if let Some(name) = name.filter(|n| !n.is_empty()) {
            let input = call.get("args").or_else(|| call.get("input"));
            ev.tools.push(tool_call_from_input(name, input));
        }
    }
}

/// Pull a tool name from the first present of the likely field locations.
fn tool_name(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let direct = obj
        .get("toolName")
        .or_else(|| obj.get("tool_name"))
        .or_else(|| obj.get("name"))
        .and_then(|v| v.as_str());
    if let Some(name) = direct.filter(|n| !n.is_empty()) {
        return Some(name.to_string());
    }
    for container in ["toolCall", "tool", "action"] {
        if let Some(name) = obj
            .get(container)
            .and_then(|c| c.get("name"))
            .and_then(|v| v.as_str())
            .filter(|n| !n.is_empty())
        {
            return Some(name.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::RecordCoverage;
    use crate::discovery::source_version::{FingerprintInputs, SourceStat, head_hash_of};
    use std::path::PathBuf;
    use std::rc::Rc;
    use tempfile::TempDir;

    struct CountingReader<R> {
        inner: R,
        bytes_read: Rc<Cell<usize>>,
    }

    struct CancellingSink {
        collector: SessionCollector,
        cancelled: Rc<Cell<bool>>,
    }

    struct TranscriptMutatingSink {
        collector: SessionCollector,
        transcript: PathBuf,
        mutated: bool,
    }

    impl RecordSink for CancellingSink {
        fn record(&mut self, record: NormalizedRecord) {
            if matches!(&record, NormalizedRecord::MetricsEvent(event) if event.usage != Usage::default())
            {
                self.cancelled.set(true);
            }
            self.collector.record(record);
        }

        fn finish(&mut self, summary: SessionSummary) {
            self.collector.finish(summary);
        }
    }

    impl RecordSink for TranscriptMutatingSink {
        fn record(&mut self, record: NormalizedRecord) {
            if !self.mutated
                && matches!(&record, NormalizedRecord::MetricsEvent(event) if event.usage != Usage::default())
            {
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&self.transcript)
                    .unwrap();
                std::io::Write::write_all(&mut file, b"{}\n").unwrap();
                self.mutated = true;
            }
            self.collector.record(record);
        }

        fn finish(&mut self, summary: SessionSummary) {
            self.collector.finish(summary);
        }
    }

    impl<R: Read> Read for CountingReader<R> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let read = self.inner.read(buffer)?;
            self.bytes_read.set(self.bytes_read.get() + read);
            Ok(read)
        }
    }

    fn input(source: RawSource) -> SessionInput {
        SessionInput {
            agent: "antigravity".to_owned(),
            session_id: "synthetic-antigravity".to_owned(),
            source,
            fork_parent_session_id: None,
        }
    }

    fn claim_for_path(path: &std::path::Path) -> SourceClaim {
        let file = File::open(path).expect("open source for claim");
        let stat = SourceStat::from_open_std_file(&file).expect("stat source for claim");
        let bytes = std::fs::read(path).expect("read source for claim");
        SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
            stat,
            head_hash: Some(head_hash_of(&bytes)),
        })
    }

    fn protobuf_varint(mut value: u64, out: &mut Vec<u8>) {
        while value >= 0x80 {
            out.push((value as u8 & 0x7f) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    fn protobuf_field_varint(field: u64, value: u64, out: &mut Vec<u8>) {
        protobuf_varint(field << 3, out);
        protobuf_varint(value, out);
    }

    fn protobuf_field_bytes(field: u64, value: &[u8], out: &mut Vec<u8>) {
        protobuf_varint((field << 3) | 2, out);
        protobuf_varint(value.len() as u64, out);
        out.extend_from_slice(value);
    }

    fn usage_blob(
        identity_field: u64,
        identity: &str,
        input: u64,
        total_output: Option<u64>,
        thinking: u64,
        response: u64,
    ) -> Vec<u8> {
        let mut usage = Vec::new();
        protobuf_field_varint(1, 777, &mut usage);
        protobuf_field_varint(2, input, &mut usage);
        if let Some(total_output) = total_output {
            protobuf_field_varint(3, total_output, &mut usage);
        }
        protobuf_field_varint(4, 7, &mut usage);
        protobuf_field_varint(5, 800, &mut usage);
        protobuf_field_bytes(identity_field, identity.as_bytes(), &mut usage);
        protobuf_field_varint(9, thinking, &mut usage);
        protobuf_field_varint(10, response, &mut usage);
        protobuf_field_varint(30, 42, &mut usage);
        protobuf_varint((31 << 3) | 5, &mut usage);
        usage.extend_from_slice(&42_u32.to_le_bytes());
        protobuf_varint((32 << 3) | 3, &mut usage);
        protobuf_field_varint(33, 42, &mut usage);
        protobuf_varint((32 << 3) | 4, &mut usage);
        usage
    }

    fn retry_blob(usage: &[u8]) -> Vec<u8> {
        let mut retry = Vec::new();
        protobuf_field_bytes(2, usage, &mut retry);
        retry
    }

    fn generation_blob(
        wrapper_field: u64,
        model_19: Option<&[u8]>,
        model_21: Option<&[u8]>,
        primary: &[u8],
        retries: &[Vec<u8>],
    ) -> Vec<u8> {
        let mut chat_model = Vec::new();
        protobuf_field_bytes(4, primary, &mut chat_model);
        for retry in retries {
            protobuf_field_bytes(17, &retry_blob(retry), &mut chat_model);
        }
        if let Some(model) = model_19 {
            protobuf_field_bytes(19, model, &mut chat_model);
        }
        if let Some(model) = model_21 {
            protobuf_field_bytes(21, model, &mut chat_model);
        }
        protobuf_varint((23 << 3) | 1, &mut chat_model);
        chat_model.extend_from_slice(&42_u64.to_le_bytes());
        let mut outer = Vec::new();
        protobuf_field_bytes(1, b"unrelated sibling", &mut outer);
        protobuf_field_bytes(wrapper_field, &chat_model, &mut outer);
        outer
    }

    fn timestamp_blob(seconds: u64) -> Vec<u8> {
        let mut timestamp = Vec::new();
        protobuf_field_varint(1, seconds, &mut timestamp);
        protobuf_field_varint(2, 500_000_000, &mut timestamp);
        timestamp
    }

    fn step_blob(
        timestamp_field: u64,
        seconds: u64,
        primary: &[u8],
        retries: &[Vec<u8>],
    ) -> Vec<u8> {
        let mut step = Vec::new();
        protobuf_field_bytes(timestamp_field, &timestamp_blob(seconds), &mut step);
        protobuf_field_bytes(9, primary, &mut step);
        for retry in retries {
            protobuf_field_bytes(28, &retry_blob(retry), &mut step);
        }
        step
    }

    fn sqlite_session(
        subroot_name: &str,
        generations: &[Vec<u8>],
        steps: &[Vec<u8>],
        oversized_generation: bool,
        oversized_step: bool,
    ) -> (TempDir, SessionInput) {
        let directory = TempDir::new().expect("tempdir");
        let session_id = "session-db";
        let subroot = directory.path().join(subroot_name);
        let database_dir = subroot.join("conversations");
        let transcript_dir = subroot
            .join("brain")
            .join(session_id)
            .join(".system_generated")
            .join("logs");
        std::fs::create_dir_all(&database_dir).unwrap();
        std::fs::create_dir_all(&transcript_dir).unwrap();
        std::fs::write(
            transcript_dir.join("transcript.jsonl"),
            concat!(
                r#"{"type":"USER_INPUT","created_at":"2026-01-01T00:00:00Z","content":"hello"}"#,
                "\n",
                r#"{"type":"PLANNER_RESPONSE","created_at":"2026-01-01T00:00:01Z","content":"done","usage":{"input_tokens":999,"output_tokens":999},"tool_calls":[{"name":"read_file"}]}"#,
                "\n"
            ),
        )
        .unwrap();
        let db_path = database_dir.join(format!("{session_id}.db"));
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE trajectory_meta (trajectory_id TEXT PRIMARY KEY, cascade_id TEXT, trajectory_type INTEGER, source INTEGER);
                 CREATE TABLE steps (idx INTEGER PRIMARY KEY, step_type INTEGER NOT NULL DEFAULT 0, status INTEGER NOT NULL DEFAULT 0, has_subtrajectory NUMERIC NOT NULL DEFAULT false, metadata BLOB, error_details BLOB, permissions BLOB, task_details BLOB, render_info BLOB, step_payload BLOB, step_format INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB, size INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE executor_metadata (idx INTEGER PRIMARY KEY, data BLOB);
                 CREATE TABLE parent_references (idx INTEGER PRIMARY KEY, data BLOB);
                 CREATE TABLE trajectory_metadata_blob (id TEXT PRIMARY KEY, data BLOB);
                 CREATE TABLE battle_mode_infos (idx INTEGER PRIMARY KEY, data BLOB);",
            )
            .unwrap();
        if oversized_step {
            connection
                .execute(
                    "INSERT INTO steps(idx, metadata) VALUES (0, zeroblob(?1))",
                    params![MAX_GEN_METADATA_BYTES as i64 + 1],
                )
                .unwrap();
        }
        for (index, row) in steps.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO steps(idx, metadata) VALUES (?1, ?2)",
                    params![index as i64 + 10, row],
                )
                .unwrap();
        }
        if oversized_generation {
            connection
                .execute(
                    "INSERT INTO gen_metadata(idx, data, size) VALUES (0, zeroblob(?1), ?1)",
                    params![MAX_GEN_METADATA_BYTES as i64 + 1],
                )
                .unwrap();
        }
        for (index, row) in generations.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO gen_metadata(idx, data, size) VALUES (?1, ?2, ?3)",
                    params![index as i64 + 10, row, row.len() as i64],
                )
                .unwrap();
        }
        drop(connection);
        (
            directory,
            SessionInput {
                agent: "antigravity".to_owned(),
                session_id: session_id.to_owned(),
                source: RawSource::Sqlite(db_path),
                fork_parent_session_id: None,
            },
        )
    }

    #[test]
    fn role_for_classifies_known_step_types() {
        // Direct user input.
        assert_eq!(role_for("USER_INPUT"), Some(Role::User));

        // Planning and reasoning steps use the assistant role.
        assert_eq!(role_for("PLAN"), Some(Role::Assistant));
        assert_eq!(role_for("CHAIN_OF_THINKING"), Some(Role::Assistant));
        assert_eq!(role_for("REASONING"), Some(Role::Assistant));

        // Tool / action / result step types map to Tool.
        assert_eq!(role_for("TOOL"), Some(Role::Tool));
        assert_eq!(role_for("VIEW_FILE"), Some(Role::Tool));
        assert_eq!(role_for("RUN_COMMAND"), Some(Role::Tool));
        assert_eq!(role_for("GREP_SEARCH"), Some(Role::Tool));
        assert_eq!(role_for("ERROR"), Some(Role::Tool));

        // Plain assistant prose (not a tool or a plan).
        assert_eq!(role_for("MODEL_RESPONSE"), Some(Role::Assistant));

        // Framing / bookkeeping steps stay System.
        assert_eq!(role_for("GENERIC"), Some(Role::System));
        assert_eq!(role_for("CONVERSATION_HISTORY"), Some(Role::System));
        assert_eq!(role_for("EPHEMERAL_MESSAGE"), Some(Role::System));
        assert_eq!(role_for("KNOWLEDGE_ARTIFACTS"), Some(Role::System));

        // Unknown step types are left for the caller to decide.
        assert_eq!(role_for("FLOOP"), None);
    }

    #[test]
    fn brain_stream_extracts_direct_model_and_usage() {
        let content = concat!(
            r#"{"type":"USER_INPUT","created_at":"2026-01-01T00:00:00Z","content":"hello"}"#,
            "\n",
            r#"{"type":"PLANNER_RESPONSE","created_at":"2026-01-01T00:00:01Z","content":"done","model":"MODEL_PLACEHOLDER_M35","usage":{"input_tokens":21,"output_tokens":8}}"#,
            "\n"
        );

        let session = AntigravityAdapter
            .normalize(&input(RawSource::Jsonl(content.to_owned())))
            .expect("brain transcript normalizes");

        assert_eq!(session.model.as_deref(), Some("MODEL_PLACEHOLDER_M35"));
        assert_eq!(
            session.events[1].model.as_deref(),
            Some("MODEL_PLACEHOLDER_M35")
        );
        assert_eq!(session.events[1].usage.input_tokens, 21);
        assert_eq!(session.events[1].usage.output_tokens, 8);
        assert!(!session.cache_write_tokens_available);
    }

    #[test]
    fn cascade_stream_extracts_nested_steps_model_and_usage() {
        let content = r#"{"source":"antigravity_api","model":"MODEL_PLACEHOLDER_M26","steps":{"steps":[{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"done","metadata":{"createdAt":"2026-01-01T00:00:01Z"},"usage":{"input_tokens":34,"output_tokens":13}}]}}"#;

        let session = AntigravityAdapter
            .normalize(&input(RawSource::Jsonl(content.to_owned())))
            .expect("cascade document normalizes");

        assert_eq!(session.events.len(), 1);
        assert_eq!(session.model.as_deref(), Some("MODEL_PLACEHOLDER_M26"));
        assert_eq!(
            session.events[0].model.as_deref(),
            Some("MODEL_PLACEHOLDER_M26")
        );
        assert_eq!(session.events[0].usage.input_tokens, 34);
        assert_eq!(session.events[0].usage.output_tokens, 13);
    }

    #[test]
    fn cascade_top_level_model_applies_before_or_after_steps() {
        let before = r#"{"model":"MODEL_PLACEHOLDER_M26","steps":{"steps":[{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"done"}]}}"#;
        let after = r#"{"steps":{"steps":[{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"done"}]},"model":"MODEL_PLACEHOLDER_M26"}"#;

        for content in [before, after] {
            let session = AntigravityAdapter
                .normalize(&input(RawSource::Jsonl(content.to_owned())))
                .expect("cascade document normalizes");

            assert_eq!(session.model.as_deref(), Some("MODEL_PLACEHOLDER_M26"));
            assert_eq!(
                session.events[0].model.as_deref(),
                Some("MODEL_PLACEHOLDER_M26")
            );
        }
    }

    #[test]
    fn malformed_cascade_marks_partial_after_streamed_steps() {
        let content = r#"{"source":"antigravity_api","steps":{"steps":[{"type":"CORTEX_STEP_TYPE_USER_INPUT","userInput":{"userResponse":"hello"}},broken]}}"#;
        let input = input(RawSource::Jsonl(content.to_owned()));
        let mut collector = SessionCollector::new(&input.agent, &input.session_id);

        AntigravityAdapter
            .visit(&input, &mut collector)
            .expect("malformed cascade returns partial coverage");

        assert_eq!(collector.coverage(), RecordCoverage::Partial);
        assert!(
            collector
                .partial_reasons()
                .contains(&PartialReason::MalformedRecord)
        );
        assert_eq!(collector.into_session().unwrap().events.len(), 1);
    }

    #[test]
    fn large_ignored_cascade_content_does_not_increase_retained_step_bytes() {
        fn retained_high_water(content: &str) -> (usize, usize) {
            let document = format!(
                r#"{{"source":"antigravity_api","steps":{{"steps":[{{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"{content}","model":"MODEL_PLACEHOLDER_M35","usage":{{"input_tokens":21,"output_tokens":8}}}}]}}}}"#
            );
            let mut sink = SessionCollector::new("antigravity", "retained-memory");
            let (summary, retained) = AntigravityAdapter
                .visit_cascade_with_retained_high_water(
                    Cursor::new(document.as_bytes()),
                    &|| false,
                    &mut sink,
                )
                .expect("cascade streams");
            sink.finish(summary);
            let events = sink.into_session().expect("cascade finishes").events.len();
            (retained, events)
        }

        let small = retained_high_water("small ignored body");
        let large_body = "x".repeat(crate::analysis::MAX_RECORD_BYTES - 1024);
        let large = retained_high_water(&large_body);

        assert_eq!(small.1, 1);
        assert_eq!(large.1, 1);
        assert_eq!(large.0, small.0);
        assert!(large.0 < 1024);
    }

    #[test]
    fn cascade_retained_fields_are_bounded_and_mark_partial() {
        let long_model = "m".repeat(MAX_CASCADE_STRING_BYTES + 1);
        let document = format!(
            r#"{{"source":"antigravity_api","steps":{{"steps":[{{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"ignored","model":"{long_model}","usage":{{"input_tokens":1}}}}]}}}}"#
        );
        let mut collector = SessionCollector::new("antigravity", "bounded-fields");

        let (summary, retained) = AntigravityAdapter
            .visit_cascade_with_retained_high_water(
                Cursor::new(document.as_bytes()),
                &|| false,
                &mut collector,
            )
            .expect("cascade streams");
        collector.finish(summary);

        assert_eq!(collector.coverage(), RecordCoverage::Partial);
        assert!(retained <= MAX_CASCADE_RETAINED_STEP_BYTES);
        let session = collector.into_session().expect("cascade finishes");
        assert_eq!(session.events.len(), 1);
        assert_eq!(
            session.events[0].model.as_ref().unwrap().len(),
            MAX_CASCADE_STRING_BYTES
        );
        assert!(session.events[0].model.as_ref().unwrap().capacity() <= MAX_CASCADE_STRING_BYTES);
    }

    #[test]
    fn optional_or_mistyped_cascade_fields_do_not_abort_later_steps() {
        let content = concat!(
            r#"{"source":"antigravity_api","steps":{"steps":[null,"#,
            r#"{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"one","usage":null,"metadata":null,"tool_calls":null},"#,
            r#"{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"two","usage":"bad","metadata":7,"tool_calls":{"bad":true}},"#,
            r#"{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"three","usage":{"input_tokens":4,"output_tokens":2},"metadata":{"createdAt":"2026-01-01T00:00:03Z"}}]}}"#
        );

        let session = AntigravityAdapter
            .normalize(&input(RawSource::Jsonl(content.to_owned())))
            .expect("optional cascade fields do not abort the document");

        assert_eq!(session.events.len(), 3);
        assert_eq!(session.events[2].usage.input_tokens, 4);
        assert!(session.events[2].ts_ms.is_some());
    }

    #[test]
    fn malformed_usage_values_do_not_abort_later_valid_steps() {
        let content = concat!(
            r#"{"steps":{"steps":["#,
            r#"{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"one","usage":{"input_tokens":{"bad":1},"output_tokens":[2]}},"#,
            r#"{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"two","usage":[{"input_tokens":99}]},"#,
            r#"{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"three","usage":{"input_tokens":7,"output_tokens":3}}]}}"#
        );
        let input = input(RawSource::Jsonl(content.to_owned()));
        let mut collector = SessionCollector::new(&input.agent, &input.session_id);

        AntigravityAdapter
            .visit(&input, &mut collector)
            .expect("malformed usage does not abort the cascade");

        assert_eq!(collector.coverage(), RecordCoverage::Partial);
        let session = collector.into_session().expect("cascade finishes");
        assert_eq!(session.events.len(), 3);
        assert_eq!(session.events[0].usage, Usage::default());
        assert_eq!(session.events[1].usage, Usage::default());
        assert_eq!(session.events[2].usage.input_tokens, 7);
        assert_eq!(session.events[2].usage.output_tokens, 3);
    }

    #[test]
    fn unsupported_tool_values_do_not_abort_later_steps() {
        let content = concat!(
            r#"{"steps":{"steps":["#,
            r#"{"type":"CORTEX_STEP_TYPE_TOOL","tool":false,"input":[1,2,3]},"#,
            r#"{"type":"CORTEX_STEP_TYPE_TOOL","tool":"read_file","input":7},"#,
            r#"{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"later","usage":{"input_tokens":7}}]}}"#
        );
        let input = input(RawSource::Jsonl(content.to_owned()));
        let mut collector = SessionCollector::new(&input.agent, &input.session_id);

        AntigravityAdapter.visit(&input, &mut collector).unwrap();

        assert_eq!(collector.coverage(), RecordCoverage::Partial);
        let session = collector.into_session().unwrap();
        assert_eq!(session.events.len(), 3);
        assert_eq!(session.events[1].tools[0].name, "read_file");
        assert_eq!(session.events[2].usage.input_tokens, 7);
    }

    #[test]
    fn invalid_numeric_usage_is_partial_and_does_not_create_tokens() {
        let content = concat!(
            r#"{"steps":{"steps":["#,
            r#"{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"negative","usage":{"input_tokens":-1}},"#,
            r#"{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"fractional","usage":{"input_tokens":1.5}},"#,
            r#"{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"overflow","usage":{"input_tokens":1e40}},"#,
            r#"{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":"valid","usage":{"input_tokens":9}}]}}"#
        );
        let input = input(RawSource::Jsonl(content.to_owned()));
        let mut collector = SessionCollector::new(&input.agent, &input.session_id);

        AntigravityAdapter.visit(&input, &mut collector).unwrap();

        assert_eq!(collector.coverage(), RecordCoverage::Partial);
        let session = collector.into_session().unwrap();
        assert_eq!(session.events.len(), 4);
        assert!(
            session.events[..3]
                .iter()
                .all(|event| event.usage == Usage::default())
        );
        assert_eq!(session.events[3].usage.input_tokens, 9);
    }

    #[test]
    fn structural_inline_cascade_detection_accepts_whitespace_and_newlines() {
        let cascade = r#"
        {
          "source" : "antigravity_api",
          "steps" : { "steps" : [] }
        }
        "#;
        let steps_only = "{\n  \"steps\" : { \"steps\" : [] }\n}";
        let brain = r#"{"type":"USER_INPUT","content":"hello"}"#;

        assert!(is_cascade_content(cascade));
        assert!(is_cascade_content(steps_only));
        assert!(!is_cascade_content(brain));
        assert!(!is_cascade_content("{broken"));
    }

    #[test]
    fn malformed_and_oversized_brain_records_do_not_hide_neighbors() {
        let mut content = String::from(
            "{\"type\":\"USER_INPUT\",\"created_at\":\"2026-01-01T00:00:00Z\",\"content\":\"first\"}\n{broken\n",
        );
        content.push_str(&"x".repeat(crate::analysis::MAX_RECORD_BYTES + 1));
        content.push('\n');
        content.push_str(
            "{\"type\":\"PLANNER_RESPONSE\",\"created_at\":\"2026-01-01T00:00:01Z\",\"content\":\"last\"}\n",
        );
        let input = input(RawSource::Jsonl(content));
        let mut collector = SessionCollector::new(&input.agent, &input.session_id);

        AntigravityAdapter
            .visit(&input, &mut collector)
            .expect("brain transcript streams");

        assert_eq!(collector.coverage(), RecordCoverage::Partial);
        assert!(
            collector
                .partial_reasons()
                .contains(&PartialReason::MalformedRecord)
        );
        assert!(
            collector
                .partial_reasons()
                .contains(&PartialReason::Oversized)
        );
        assert_eq!(collector.into_session().unwrap().events.len(), 2);
    }

    #[test]
    fn claimed_brain_file_is_validated_and_streamed() {
        let directory = TempDir::new().expect("tempdir");
        let path = directory.path().join("transcript.jsonl");
        std::fs::write(
            &path,
            b"{\"type\":\"USER_INPUT\",\"created_at\":\"2026-01-01T00:00:00Z\",\"content\":\"hello\"}\n",
        )
        .expect("write transcript");
        let claim = claim_for_path(&path);
        let input = input(RawSource::File(path));
        let mut collector = SessionCollector::new(&input.agent, &input.session_id);

        let outcome = AntigravityAdapter
            .visit_claimed(
                &input,
                &claim,
                AppendOnlyGuarantee::Absent,
                &|| false,
                &mut collector,
            )
            .expect("claimed transcript streams");

        assert_eq!(outcome, VisitOutcome::AcceptedFull);
        assert_eq!(collector.into_session().unwrap().events.len(), 1);
    }

    #[test]
    fn changed_claimed_brain_file_is_rejected_before_streaming() {
        let directory = TempDir::new().expect("tempdir");
        let path = directory.path().join("transcript.jsonl");
        std::fs::write(&path, b"original\n").expect("write transcript");
        let claim = claim_for_path(&path);
        std::fs::write(&path, b"modified\n").expect("modify transcript");
        let input = input(RawSource::File(path));
        let mut collector = SessionCollector::new(&input.agent, &input.session_id);

        let outcome = AntigravityAdapter
            .visit_claimed(
                &input,
                &claim,
                AppendOnlyGuarantee::Absent,
                &|| false,
                &mut collector,
            )
            .expect("changed source returns an outcome");

        assert!(matches!(outcome, VisitOutcome::SourceChanged(_)));
    }

    #[test]
    fn cancellation_stops_brain_and_cascade_reads() {
        let brain = input(RawSource::Jsonl(
            "{\"type\":\"USER_INPUT\",\"content\":\"hello\"}\n".to_owned(),
        ));
        let cascade = input(RawSource::Jsonl(
            r#"{"source":"antigravity_api","steps":{"steps":[{"type":"CORTEX_STEP_TYPE_USER_INPUT","userInput":{"userResponse":"hello"}}]}}"#
                .to_owned(),
        ));

        for input in [brain, cascade] {
            let mut sink = SessionCollector::new(&input.agent, &input.session_id);
            let result = match &input.source {
                RawSource::Jsonl(content) if is_cascade_content(content) => AntigravityAdapter
                    .visit_cascade(Cursor::new(content.as_bytes()), &|| true, &mut sink),
                RawSource::Jsonl(content) => AntigravityAdapter.visit_jsonl(
                    BufReader::new(Cursor::new(content.as_bytes())),
                    &|| true,
                    false,
                    &mut sink,
                ),
                _ => unreachable!(),
            };
            assert!(result.is_err());
        }
    }

    #[test]
    fn cancellation_interrupts_large_ignored_cascade_content() {
        let prefix = Cursor::new(
            br#"{"steps":{"steps":[{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","content":""#,
        );
        let body = std::io::repeat(b'x').take(8 * 1024 * 1024);
        let suffix = Cursor::new(br#""}]}}"#);
        let bytes_read = Rc::new(Cell::new(0));
        let reader = CountingReader {
            inner: prefix.chain(body).chain(suffix),
            bytes_read: Rc::clone(&bytes_read),
        };
        let mut sink = SessionCollector::new("antigravity", "cancel-ignored-content");

        let result = AntigravityAdapter.visit_cascade_with_model(
            reader,
            &|| bytes_read.get() >= 64 * 1024,
            &mut sink,
            None,
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(CANCELLED_MESSAGE));
        assert!(bytes_read.get() < 128 * 1024);
    }

    #[test]
    fn cascade_size_limit_observes_one_extra_whitespace_byte() {
        let prefix = br#"{"steps":{"steps":[]}}"#;
        let total = usize::try_from(MAX_CASCADE_DOCUMENT_BYTES + 1).unwrap();
        let whitespace = std::io::repeat(b' ').take((total - prefix.len()) as u64);
        let bytes_read = Rc::new(Cell::new(0));
        let reader = CountingReader {
            inner: Cursor::new(prefix).chain(whitespace),
            bytes_read: Rc::clone(&bytes_read),
        };
        let mut sink = SessionCollector::new("antigravity", "oversized-whitespace");

        let summary = AntigravityAdapter
            .visit_cascade_with_model(reader, &|| false, &mut sink, None)
            .expect("oversized valid JSON returns partial coverage");
        sink.finish(summary);

        assert_eq!(bytes_read.get(), total);
        assert_eq!(sink.coverage(), RecordCoverage::Partial);
        assert!(sink.partial_reasons().contains(&PartialReason::Oversized));
    }

    #[test]
    fn protobuf_decoder_uses_descriptor_backed_token_fields() {
        let usage = usage_blob(11, "response-1", 30, Some(50), 40, 10);
        let generation = generation_blob(
            1,
            Some(b"gemini-3.6-flash"),
            Some(b"Gemini fallback"),
            &usage,
            &[],
        );
        let metadata = find_chat_metadata(&generation).expect("valid generation metadata");
        let mut invocations = Vec::new();
        for_each_chat_usage(metadata.data, |invocation| invocations.push(invocation)).unwrap();

        assert_eq!(metadata.model, "gemini-3.6-flash");
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].usage.input_tokens, 30);
        assert_eq!(invocations[0].usage.output_tokens, 50);
        assert_eq!(invocations[0].usage.cache_creation_tokens, 7);
        assert_eq!(invocations[0].usage.cache_read_tokens, 800);
    }

    #[test]
    fn protobuf_decoder_handles_output_rules_and_identity_fallbacks() {
        let split = decode_usage(&usage_blob(12, "provider", 5, None, 4, 3)).unwrap();
        assert_eq!(split.usage.output_tokens, 7);
        assert_eq!(split.identity, identity_hash(b"provider"));

        let message = decode_usage(&usage_blob(7, "message", 5, Some(7), 4, 3)).unwrap();
        assert_eq!(message.identity, identity_hash(b"message"));

        let mut all_identities = usage_blob(7, "message", 5, Some(7), 4, 3);
        protobuf_field_bytes(12, b"provider", &mut all_identities);
        protobuf_field_bytes(11, b"response", &mut all_identities);
        assert_eq!(
            decode_usage(&all_identities).unwrap().identity,
            identity_hash(b"response")
        );

        assert!(decode_usage(&usage_blob(11, "drift", 5, Some(8), 4, 3)).is_none());
    }

    #[test]
    fn protobuf_decoder_scans_wrappers_and_rejects_binary_model_fallback() {
        let usage = usage_blob(11, "response", 5, Some(7), 4, 3);
        let canonical = generation_blob(
            6,
            Some(b"gemini-3.6-flash"),
            Some(b"valid\0binary"),
            &usage,
            &[],
        );
        assert_eq!(
            find_chat_metadata(&canonical).unwrap().model,
            "gemini-3.6-flash"
        );
        let fallback = generation_blob(8, None, Some(b"Gemini 3.6 Flash"), &usage, &[]);
        assert_eq!(
            find_chat_metadata(&fallback).unwrap().model,
            "Gemini 3.6 Flash"
        );
        let binary_only = generation_blob(9, None, Some(b"valid\0binary"), &usage, &[]);
        assert!(find_chat_metadata(&binary_only).is_none());
        assert!(find_chat_metadata(&[0x0f]).is_none());
    }

    #[test]
    fn protobuf_usage_scans_report_malformed_siblings() {
        let valid = usage_blob(11, "valid", 5, Some(7), 4, 3);
        let generation = generation_blob(
            1,
            Some(b"gemini-3.6-flash"),
            None,
            &[0x0f],
            std::slice::from_ref(&valid),
        );
        let metadata = find_chat_metadata(&generation).unwrap();
        let generation_scan = for_each_chat_usage(metadata.data, |_| {}).unwrap();
        assert_eq!(generation_scan.count, 1);
        assert!(generation_scan.malformed);

        let mut step = step_blob(1, 100, &valid, &[]);
        protobuf_field_bytes(28, &[0x0f], &mut step);
        let (_, step_scan) = for_each_step_usage(&step, |_| {}).unwrap();
        assert_eq!(step_scan.count, 1);
        assert!(step_scan.malformed);

        let mut invalid_timestamp = Vec::new();
        protobuf_field_varint(1, 100, &mut invalid_timestamp);
        protobuf_field_varint(2, 1_000_000_000, &mut invalid_timestamp);
        let mut invalid_timestamp_step = Vec::new();
        protobuf_field_bytes(1, &invalid_timestamp, &mut invalid_timestamp_step);
        protobuf_field_bytes(9, &valid, &mut invalid_timestamp_step);
        let (_, invalid_timestamp_scan) =
            for_each_step_usage(&invalid_timestamp_step, |_| {}).unwrap();
        assert_eq!(invalid_timestamp_scan.count, 1);
        assert!(invalid_timestamp_scan.malformed);

        let mut truncated_timestamp_step = Vec::new();
        protobuf_field_bytes(1, &[0x08], &mut truncated_timestamp_step);
        protobuf_field_bytes(9, &valid, &mut truncated_timestamp_step);
        let (_, truncated_timestamp_scan) =
            for_each_step_usage(&truncated_timestamp_step, |_| {}).unwrap();
        assert_eq!(truncated_timestamp_scan.count, 1);
        assert!(truncated_timestamp_scan.malformed);

        let (_directory, input) = sqlite_session(
            "antigravity",
            std::slice::from_ref(&generation),
            std::slice::from_ref(&step),
            false,
            false,
        );
        let mut collector = SessionCollector::new(&input.agent, &input.session_id);
        AntigravityAdapter.visit(&input, &mut collector).unwrap();
        assert_eq!(collector.coverage(), RecordCoverage::Partial);
        assert!(
            collector
                .partial_reasons()
                .contains(&PartialReason::MalformedRecord)
        );
    }

    #[test]
    fn sqlite_streams_steps_retries_and_pruned_generations_without_duplicates() {
        let success = usage_blob(11, "success", 30, Some(50), 40, 10);
        let failed = usage_blob(12, "failed", 9, Some(0), 0, 0);
        let background = usage_blob(7, "background", 11, Some(5), 2, 3);
        let pruned = usage_blob(11, "pruned", 13, Some(6), 1, 5);
        let generations = vec![
            vec![0x0f],
            vec![0x0a, 0x05, 0x01],
            generation_blob(
                6,
                Some(b"gemini-3.6-flash"),
                Some(b"valid\0binary"),
                &success,
                std::slice::from_ref(&failed),
            ),
            generation_blob(1, Some(b"gemini-3.6-flash"), None, &pruned, &[]),
        ];
        let steps = vec![
            vec![0x0f],
            vec![0x0a, 0x05, 0x01],
            step_blob(1, 100, &success, std::slice::from_ref(&failed)),
            step_blob(32, 200, &background, &[]),
        ];
        let (_directory, input) =
            sqlite_session("antigravity-cli", &generations, &steps, true, true);
        let mut collector = SessionCollector::new(&input.agent, &input.session_id);

        AntigravityAdapter.visit(&input, &mut collector).unwrap();

        assert_eq!(collector.coverage(), RecordCoverage::Partial);
        assert!(
            collector
                .partial_reasons()
                .contains(&PartialReason::Oversized)
        );
        assert!(
            collector
                .partial_reasons()
                .contains(&PartialReason::MalformedRecord)
        );
        let session = collector.into_session().unwrap();
        let usage_events = session
            .events
            .iter()
            .filter(|event| event.usage != Usage::default())
            .collect::<Vec<_>>();
        assert_eq!(usage_events.len(), 4);
        assert!(
            usage_events
                .iter()
                .all(|event| event.role == Role::Assistant)
        );
        assert_eq!(usage_events[0].usage.input_tokens, 30);
        assert_eq!(usage_events[0].ts_ms, Some(100_500));
        assert_eq!(usage_events[1].usage.input_tokens, 9);
        assert_eq!(usage_events[1].ts_ms, Some(100_500));
        assert_eq!(usage_events[2].usage.input_tokens, 11);
        assert_eq!(usage_events[2].ts_ms, Some(200_500));
        assert_eq!(usage_events[3].usage.input_tokens, 13);
        assert!(usage_events[3].ts_ms.is_some());
        assert_eq!(session.events[1].usage, Usage::default());
        assert_eq!(session.events[1].role, Role::Tool);
        assert_eq!(session.events[1].tools[0].name, "read_file");
        assert!(session.cache_write_tokens_available);
    }

    #[test]
    fn sqlite_session_produces_nonzero_analyzed_token_and_context_metrics() {
        let usage = usage_blob(11, "response-1", 30, Some(50), 40, 10);
        let valid = generation_blob(1, Some(b"gemini-3.6-flash"), None, &usage, &[]);
        let step = step_blob(1, 100, &usage, &[]);
        let (_directory, input) =
            sqlite_session("antigravity-ide", &[valid], &[step], false, false);

        let summary = crate::analysis::analyze_sources(vec![input]);

        assert_eq!(summary.session_count, 1);
        assert!(summary.tokens_in_total > 0);
        assert_eq!(summary.tokens_out_total, 50);
        assert!(summary.peak_context_tokens > 0);
    }

    #[test]
    fn claimed_database_accepts_its_combined_database_and_transcript_fingerprint() {
        let usage = usage_blob(11, "response-1", 30, Some(50), 40, 10);
        let generation = generation_blob(1, Some(b"gemini-3.6-flash"), None, &usage, &[]);
        let step = step_blob(1, 100, &usage, &[]);
        let (_directory, input) =
            sqlite_session("antigravity-ide", &[generation], &[step], false, false);
        let RawSource::Sqlite(path) = &input.source else {
            unreachable!();
        };
        let (latest, rows) =
            crate::discovery::agents::antigravity::db_fingerprint(path, &input.session_id).unwrap();
        let fingerprint = provider_db_fingerprint(latest, rows);
        let mut collector = SessionCollector::new(&input.agent, &input.session_id);

        let outcome = AntigravityAdapter
            .visit_db_claimed(&input, &fingerprint, &|| false, &mut collector)
            .unwrap();

        assert_eq!(outcome, VisitOutcome::AcceptedFull);
        assert_eq!(collector.into_session().unwrap().events.len(), 3);
    }

    #[test]
    fn claimed_database_rejects_a_transcript_change_during_database_streaming() {
        let usage = usage_blob(11, "response-1", 30, Some(50), 40, 10);
        let generation = generation_blob(1, Some(b"gemini-3.6-flash"), None, &usage, &[]);
        let step = step_blob(1, 100, &usage, &[]);
        let (_directory, input) =
            sqlite_session("antigravity-ide", &[generation], &[step], false, false);
        let RawSource::Sqlite(path) = &input.source else {
            unreachable!();
        };
        let transcript = sibling_brain_transcript(path, &input.session_id).unwrap();
        let (latest, rows) =
            crate::discovery::agents::antigravity::db_fingerprint(path, &input.session_id).unwrap();
        let fingerprint = provider_db_fingerprint(latest, rows);
        let mut sink = TranscriptMutatingSink {
            collector: SessionCollector::new(&input.agent, &input.session_id),
            transcript,
            mutated: false,
        };

        let outcome = AntigravityAdapter
            .visit_db_claimed(&input, &fingerprint, &|| false, &mut sink)
            .unwrap();

        assert!(matches!(outcome, VisitOutcome::SourceChanged(_)));
        assert!(sink.mutated);
    }

    #[test]
    fn sqlite_session_keeps_usage_when_the_brain_transcript_is_absent() {
        let usage = usage_blob(11, "response-1", 30, Some(50), 40, 10);
        let generation = generation_blob(1, Some(b"gemini-3.6-flash"), None, &usage, &[]);
        let step = step_blob(1, 100, &usage, &[]);
        let (_directory, input) =
            sqlite_session("antigravity-ide", &[generation], &[step], false, false);
        let RawSource::Sqlite(path) = &input.source else {
            unreachable!();
        };
        let transcript = sibling_brain_transcript(path, &input.session_id).unwrap();
        std::fs::remove_file(transcript).unwrap();

        let summary = crate::analysis::analyze_sources(vec![input]);

        assert_eq!(summary.session_count, 1);
        assert_eq!(summary.tokens_in_total, 37);
        assert_eq!(summary.tokens_out_total, 50);
        assert_eq!(summary.peak_context_tokens, 837);
    }

    #[test]
    fn database_identity_state_has_a_hard_bound() {
        let mut state = DatabaseUsageState::default();
        let mut collector = SessionCollector::new("antigravity", "bounded-identities");
        for identity in 0..=MAX_TRACKED_RESPONSE_IDS as u64 {
            state.record_model(identity, "gemini-3.6-flash", &mut collector);
        }

        assert_eq!(state.models.len(), MAX_TRACKED_RESPONSE_IDS);
        assert!(state.model_saturated);
        assert_eq!(collector.coverage(), RecordCoverage::Partial);
    }

    #[test]
    fn sqlite_stream_degrades_when_one_usage_table_is_missing() {
        let usage = usage_blob(11, "response", 5, Some(7), 4, 3);
        let generation = generation_blob(1, Some(b"gemini-3.6-flash"), None, &usage, &[]);
        let step = step_blob(1, 100, &usage, &[]);
        for missing in ["steps", "gen_metadata"] {
            let (_directory, input) = sqlite_session(
                "antigravity",
                std::slice::from_ref(&generation),
                std::slice::from_ref(&step),
                false,
                false,
            );
            let RawSource::Sqlite(path) = &input.source else {
                unreachable!();
            };
            Connection::open(path)
                .unwrap()
                .execute_batch(&format!("DROP TABLE {missing}"))
                .unwrap();

            let session = AntigravityAdapter.normalize(&input).unwrap();

            assert_eq!(
                session
                    .events
                    .iter()
                    .filter(|event| event.usage != Usage::default())
                    .count(),
                1
            );
        }
    }

    #[test]
    fn sqlite_stream_rejects_a_database_without_antigravity_tables() {
        let (_directory, input) = sqlite_session("antigravity", &[], &[], false, false);
        let RawSource::Sqlite(path) = &input.source else {
            unreachable!();
        };
        Connection::open(path)
            .unwrap()
            .execute_batch(
                "DROP TABLE steps; DROP TABLE gen_metadata; CREATE TABLE unrelated(id INTEGER);",
            )
            .unwrap();

        assert!(AntigravityAdapter.normalize(&input).is_err());
    }

    #[test]
    fn sqlite_stream_checks_cancellation_between_rows() {
        let first = usage_blob(11, "first", 5, Some(7), 4, 3);
        let second = usage_blob(11, "second", 6, Some(8), 4, 4);
        let steps = vec![
            step_blob(1, 100, &first, &[]),
            step_blob(1, 200, &second, &[]),
        ];
        let (_directory, input) = sqlite_session("antigravity", &[], &steps, false, false);
        let RawSource::Sqlite(path) = &input.source else {
            unreachable!();
        };
        let connection = open_database(path).unwrap();
        connection.execute_batch("BEGIN").unwrap();
        let cancelled = Rc::new(Cell::new(false));
        let mut sink = CancellingSink {
            collector: SessionCollector::new(&input.agent, &input.session_id),
            cancelled: Rc::clone(&cancelled),
        };

        let result = AntigravityAdapter.visit_database_rows(
            &connection,
            SessionSummary::default(),
            &|| cancelled.get(),
            &mut sink,
        );

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("database read was cancelled")
        );
    }
}
