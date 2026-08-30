//! Bounded OpenCode message and part analysis.
//!
//! OpenCode stores one JSON message blob and zero or more JSON part blobs in
//! SQLite. Discovery exports the same records as JSONL with each message
//! immediately followed by its parts. This adapter retains only one normalized
//! message while it consumes either representation.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read};
use std::path::Path;

use anyhow::Context;
use rusqlite::{Connection, OpenFlags, Statement, params};
use serde_json::{Map, Value};

use crate::analysis::EVIDENCE_STRING_CAP;
use crate::analysis::SourceChangedReason;
use crate::analysis::framing::{
    BoundedJsonlReader, FramedRecord, MAX_RECORD_BYTES, PartialReason, RecordSkip,
};
use crate::analysis::interface::{
    ContentKind, ContentPart, EvidenceObservation, NormalizedRecord, RawSource, RecordSink,
    RelationProvenance, SessionCollector, SessionInput, SessionSummary, TurnContent, VendorAdapter,
    VisitOutcome,
};
use crate::analysis::model::{
    CompactionTrigger, EventSource, NormalizedEvent, NormalizedSession, Role, ToolCall,
    ToolCategory, Usage,
};
use crate::analysis::records::{compact_json_text, parse_ts, tool_call_from_input};
use crate::discovery::agents::opencode::{
    db_session_fingerprint_connection, db_session_has_parent_id, opencode_fork_parent_title,
};
use crate::discovery::source_version::provider_db_fingerprint;

const MAX_MESSAGE_PART_BYTES: usize = MAX_RECORD_BYTES;

/// Caps the descendant sessions [`OpenCodeStreamState`] tracks by identity,
/// mirroring [`crate::analysis::threads::ThreadResolver`]'s bound for a
/// pathologically wide fan-out of subagents.
const MAX_TRACKED_CHILD_SESSIONS: usize = 50_000;

pub struct OpenCodeAdapter;

impl VendorAdapter for OpenCodeAdapter {
    fn agent(&self) -> &'static str {
        "opencode"
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
        let summary = match &input.source {
            RawSource::File(path) => {
                self.visit_reader(BufReader::new(File::open(path)?), &|| false, sink)?
            }
            RawSource::Jsonl(content) => {
                let suffix: &[u8] = if content.ends_with('\n') { b"" } else { b"\n" };
                let source = Cursor::new(content.as_bytes()).chain(suffix);
                self.visit_reader(BufReader::new(source), &|| false, sink)?
            }
            RawSource::Sqlite(path) => {
                self.visit_database(path, &input.session_id, &|| false, sink)?
            }
        };
        sink.finish(summary);
        Ok(VisitOutcome::Unvalidated)
    }

    fn visit_db_claimed(
        &self,
        input: &SessionInput,
        claimed_fingerprint: &str,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        let RawSource::Sqlite(path) = &input.source else {
            anyhow::bail!("a claimed OpenCode database source must be SQLite");
        };
        let conn = open_database(path)?;
        conn.execute_batch("BEGIN")?;
        let actual = db_session_fingerprint_connection(&conn, &input.session_id)
            .map(|(latest, rows)| provider_db_fingerprint(latest, rows));
        if actual.as_deref() != Some(claimed_fingerprint) {
            return Ok(VisitOutcome::SourceChanged(
                SourceChangedReason::FingerprintMismatch,
            ));
        }
        let summary = visit_database_connection(&conn, &input.session_id, cancel, sink)?;
        conn.execute_batch("COMMIT")?;
        sink.finish(summary);
        Ok(VisitOutcome::AcceptedFull)
    }
}

impl OpenCodeAdapter {
    fn visit_reader(
        &self,
        reader: impl BufRead,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<SessionSummary> {
        let mut reader = BoundedJsonlReader::new(reader);
        let mut state = OpenCodeStreamState::default();
        while let Some(record) = reader.next_record(cancel) {
            match record {
                FramedRecord::Skipped(skip) => match skip {
                    RecordSkip::Oversized { .. } | RecordSkip::IncompleteTail { .. } => {
                        state.flush(sink);
                        sink.record(NormalizedRecord::Unusable(skip.partial_reason()));
                    }
                    RecordSkip::ReadFailed { index, kind } => {
                        anyhow::bail!("OpenCode record {index} read failed: {kind:?}");
                    }
                    RecordSkip::Cancelled { index } => {
                        anyhow::bail!("OpenCode record {index} read was cancelled");
                    }
                },
                FramedRecord::Complete { bytes, .. } => {
                    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
                        state.flush(sink);
                        sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
                        continue;
                    };
                    state.observe_export(value, bytes.len(), sink);
                }
            }
        }
        state.flush(sink);
        Ok(state.finish())
    }

    fn visit_database(
        &self,
        path: &Path,
        session_id: &str,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<SessionSummary> {
        let conn = open_database(path)?;
        conn.execute_batch("BEGIN")?;
        let summary = visit_database_connection(&conn, session_id, cancel, sink)?;
        conn.execute_batch("COMMIT")?;
        Ok(summary)
    }
}

fn open_database(path: &Path) -> anyhow::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening OpenCode database {}", path.display()))
}

fn visit_database_connection(
    conn: &Connection,
    root_session_id: &str,
    cancel: &dyn Fn() -> bool,
    sink: &mut dyn RecordSink,
) -> anyhow::Result<SessionSummary> {
    let mut state = OpenCodeStreamState::default();
    let cluster = if db_session_has_parent_id(conn) {
        "WITH RECURSIVE cluster(id) AS (
             SELECT id FROM session WHERE id = ?1
             UNION
             SELECT session.id FROM session JOIN cluster ON session.parent_id = cluster.id
         )"
    } else {
        "WITH cluster(id) AS (SELECT id FROM session WHERE id = ?1)"
    };
    state.descendant_sessions = query_db_descendant_sessions(conn, cluster, root_session_id)?;
    let mut messages = conn.prepare(&format!(
        "{cluster}
         SELECT message.id, message.time_created, message.time_updated,
                CASE WHEN length(CAST(message.data AS BLOB)) <= ?2 THEN message.data END,
                length(CAST(message.data AS BLOB)),
                message.session_id
         FROM message JOIN cluster ON message.session_id = cluster.id
         ORDER BY COALESCE(message.time_created, message.time_updated, 0),
                   message.session_id, message.id"
    ))?;
    let mut parts = prepare_db_parts(conn)?;
    let mut rows = messages.query(params![root_session_id, MAX_RECORD_BYTES as i64])?;
    while let Some(row) = rows.next()? {
        if cancel() {
            anyhow::bail!("OpenCode database read was cancelled");
        }
        let message_id: String = row.get(0)?;
        let created: Option<i64> = row.get(1).ok();
        let updated: Option<i64> = row.get(2).ok();
        let data: Option<String> = row.get(3).ok().flatten();
        let data_len = row.get::<_, Option<i64>>(4)?.unwrap_or(0).max(0) as usize;
        let session_id: String = row.get(5)?;
        let fallback_ts = created.or(updated).and_then(parse_db_ts);

        if data_len > MAX_RECORD_BYTES {
            sink.record(NormalizedRecord::Unusable(PartialReason::Oversized));
            drain_message_parts(&mut parts, &message_id, cancel, sink)?;
            continue;
        }
        let Some(data) = data else {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            drain_message_parts(&mut parts, &message_id, cancel, sink)?;
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            drain_message_parts(&mut parts, &message_id, cancel, sink)?;
            continue;
        };
        let Some(mut event) = message_event(&value, fallback_ts) else {
            report_invalid_message(&value, sink);
            drain_message_parts(&mut parts, &message_id, cancel, sink)?;
            continue;
        };
        state.apply_session(&mut event, &session_id, session_id != root_session_id, sink);
        state.observe_model(&event);
        let mut pending = PendingMessage {
            id: message_id,
            event,
            content: Vec::new(),
            part_bytes: 0,
            parts_oversized: false,
        };
        visit_db_parts(&mut parts, &mut pending, cancel, sink)?;
        sink.record(NormalizedRecord::MetricsEvent(Box::new(pending.event)));
        if !pending.content.is_empty() {
            sink.record(NormalizedRecord::TurnContent(Box::new(TurnContent {
                parts: pending.content,
            })));
        }
    }
    Ok(state.finish())
}

/// Loads each descendant session's own creation timestamp and fork-title
/// signal in one query, so a message row never needs its own join back to
/// `session`. One row per session, not per message; forks (null
/// `parent_id`) never join `cluster` and never appear here.
fn query_db_descendant_sessions(
    conn: &Connection,
    cluster: &str,
    root_session_id: &str,
) -> rusqlite::Result<HashMap<String, DescendantSessionMeta>> {
    let mut statement = conn.prepare(&format!(
        "{cluster}
         SELECT session.id, session.time_created, session.title
         FROM session JOIN cluster ON session.id = cluster.id
         WHERE session.id != ?1"
    ))?;
    let mut rows = statement.query(params![root_session_id])?;
    let mut out = HashMap::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let created: Option<i64> = row.get(1).ok();
        let title: Option<String> = row.get(2).ok();
        out.insert(
            id,
            DescendantSessionMeta {
                ts_ms: created.and_then(parse_db_ts),
                looks_like_fork: title
                    .as_deref()
                    .is_some_and(|title| opencode_fork_parent_title(title).is_some()),
            },
        );
    }
    Ok(out)
}

fn prepare_db_parts(conn: &Connection) -> rusqlite::Result<Statement<'_>> {
    conn.prepare(
        "SELECT id, time_created, time_updated,
                CASE
                    WHEN length(CAST(data AS BLOB)) <= ?2
                     AND SUM(length(CAST(data AS BLOB))) OVER (
                             ORDER BY COALESCE(time_created, time_updated, 0), id
                             ROWS UNBOUNDED PRECEDING
                         ) <= ?2
                    THEN data
                END,
                length(CAST(data AS BLOB)),
                SUM(length(CAST(data AS BLOB))) OVER (
                    ORDER BY COALESCE(time_created, time_updated, 0), id
                    ROWS UNBOUNDED PRECEDING
                )
           FROM part
          WHERE message_id = ?1
          ORDER BY COALESCE(time_created, time_updated, 0), id",
    )
}

fn visit_db_parts(
    statement: &mut Statement<'_>,
    pending: &mut PendingMessage,
    cancel: &dyn Fn() -> bool,
    sink: &mut dyn RecordSink,
) -> anyhow::Result<()> {
    let mut rows = statement.query(params![pending.id, MAX_RECORD_BYTES as i64])?;
    while let Some(row) = rows.next()? {
        if cancel() {
            anyhow::bail!("OpenCode database read was cancelled");
        }
        let created: Option<i64> = row.get(1).ok();
        let updated: Option<i64> = row.get(2).ok();
        let data: Option<String> = row.get(3).ok().flatten();
        let data_len = row.get::<_, Option<i64>>(4)?.unwrap_or(0).max(0) as usize;
        pending.part_bytes = row.get::<_, Option<i64>>(5)?.unwrap_or(0).max(0) as usize;
        if data_len > MAX_RECORD_BYTES || pending.part_bytes > MAX_MESSAGE_PART_BYTES {
            if !pending.parts_oversized {
                sink.record(NormalizedRecord::Unusable(PartialReason::Oversized));
                pending.parts_oversized = true;
            }
            continue;
        }
        let Some(data) = data else {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            continue;
        };
        apply_part(
            &value,
            created.or(updated).and_then(parse_db_ts),
            pending,
            sink,
        );
    }
    Ok(())
}

fn drain_message_parts(
    statement: &mut Statement<'_>,
    message_id: &str,
    cancel: &dyn Fn() -> bool,
    sink: &mut dyn RecordSink,
) -> anyhow::Result<()> {
    let mut pending = PendingMessage {
        id: message_id.to_owned(),
        event: NormalizedEvent::new(Role::Assistant),
        content: Vec::new(),
        part_bytes: 0,
        parts_oversized: false,
    };
    visit_db_parts(statement, &mut pending, cancel, sink)
}

/// One descendant session's own creation timestamp and fork-title signal,
/// known from `session` (database) or `session_member` (export) before its
/// first message streams.
#[derive(Default)]
struct DescendantSessionMeta {
    ts_ms: Option<i64>,
    looks_like_fork: bool,
}

#[derive(Default)]
struct OpenCodeStreamState {
    pending: Option<PendingMessage>,
    model: Option<String>,
    /// The root session id, known from the export's `session_meta` row.
    /// `None` for a synthetic stream with no session wrapper; every message
    /// then stays main-scope, matching the pre-relationship behavior.
    root_session_id: Option<String>,
    /// The root's own most recently observed model, reported as
    /// `SubagentSpawn::parent_model`.
    root_model: Option<String>,
    /// Spawn timestamp and fork-title signal per descendant session,
    /// prefetched (database) or accumulated from `session_member` rows
    /// (export) ahead of that session's first message.
    descendant_sessions: HashMap<String, DescendantSessionMeta>,
    /// Descendant sessions already reported through `SubagentSpawn`, capped
    /// like [`crate::analysis::threads::ThreadResolver`].
    seen_child_sessions: HashSet<String>,
    /// True once [`MAX_TRACKED_CHILD_SESSIONS`] is reached and a further
    /// descendant session goes unreported.
    children_capped: bool,
    /// True once a `parent_id` descendant's title also looks like an
    /// OpenCode fork title (`opencode_fork_parent_title`). Discovery's
    /// `infer_db_fork_observation` owns real fork detection; this only
    /// degrades attribution for the ambiguous shape.
    saw_fork_titled_child: bool,
}

struct PendingMessage {
    id: String,
    event: NormalizedEvent,
    /// Content captured from this message's `text`, `reasoning`, and `tool`
    /// parts, in part order. Emitted as one `TurnContent` record right after
    /// this message's `MetricsEvent`.
    content: Vec<ContentPart>,
    part_bytes: usize,
    parts_oversized: bool,
}

impl OpenCodeStreamState {
    fn observe_export(&mut self, value: Value, bytes: usize, sink: &mut dyn RecordSink) {
        let row_type = value.get("type").and_then(Value::as_str);
        match row_type {
            Some("message") => {
                self.flush(sink);
                let Some(id) = value.get("messageID").and_then(Value::as_str) else {
                    sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
                    return;
                };
                let payload = value.get("payload").unwrap_or(&value);
                let fallback_ts = value.pointer("/time/created").and_then(parse_ts);
                let Some(mut event) = message_event(payload, fallback_ts) else {
                    report_invalid_message(payload, sink);
                    return;
                };
                if let Some(session_id) = value.get("sessionID").and_then(Value::as_str) {
                    let session_role = value.get("sessionRole").and_then(Value::as_str);
                    let is_child = session_role == Some("child")
                        || self
                            .root_session_id
                            .as_deref()
                            .is_some_and(|root| root != session_id);
                    self.apply_session(&mut event, session_id, is_child, sink);
                }
                self.observe_model(&event);
                self.pending = Some(PendingMessage {
                    id: id.to_owned(),
                    event,
                    content: Vec::new(),
                    part_bytes: 0,
                    parts_oversized: false,
                });
            }
            Some("part") => {
                let Some(pending) = self.pending.as_mut() else {
                    unrecognized("part_without_message", sink);
                    return;
                };
                if value.get("messageID").and_then(Value::as_str) != Some(pending.id.as_str()) {
                    self.flush(sink);
                    unrecognized("noncontiguous_part", sink);
                    return;
                }
                pending.part_bytes = pending.part_bytes.saturating_add(bytes);
                if pending.part_bytes > MAX_MESSAGE_PART_BYTES {
                    if !pending.parts_oversized {
                        sink.record(NormalizedRecord::Unusable(PartialReason::Oversized));
                        pending.parts_oversized = true;
                    }
                    return;
                }
                let payload = value.get("payload").unwrap_or(&value);
                let fallback_ts = value.pointer("/time/created").and_then(parse_ts);
                apply_part(payload, fallback_ts, pending, sink);
            }
            Some("session_meta") => {
                self.flush(sink);
                if self.root_session_id.is_none() {
                    self.root_session_id = value
                        .pointer("/payload/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                self.observe_record_timestamp(&value, sink);
            }
            Some("session_member") => {
                self.flush(sink);
                if let Some(child_id) = value.get("originSessionID").and_then(Value::as_str) {
                    let ts_ms = value.pointer("/time/created").and_then(parse_ts);
                    let title = value.pointer("/payload/title").and_then(Value::as_str);
                    self.descendant_sessions.insert(
                        child_id.to_owned(),
                        DescendantSessionMeta {
                            ts_ms,
                            looks_like_fork: title
                                .is_some_and(|title| opencode_fork_parent_title(title).is_some()),
                        },
                    );
                }
                self.observe_record_timestamp(&value, sink);
            }
            Some(discriminator) => {
                self.flush(sink);
                unrecognized(discriminator, sink);
            }
            None => {
                self.flush(sink);
                unrecognized("<missing>", sink);
            }
        }
    }

    fn observe_record_timestamp(&self, value: &Value, sink: &mut dyn RecordSink) {
        if let Some(ts_ms) = value
            .pointer("/time/created")
            .and_then(parse_ts)
            .or_else(|| value.pointer("/time/updated").and_then(parse_ts))
        {
            sink.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::RecordTimestamp { ts_ms },
            )));
        }
    }

    /// Applies one message's session identity to its event: sets
    /// `thread_id` to the message's own session and, for a descendant
    /// session, `EventSource::Subagent`. Otherwise updates the root's
    /// current model, reported by a later spawn as `parent_model`.
    fn apply_session(
        &mut self,
        event: &mut NormalizedEvent,
        session_id: &str,
        is_child: bool,
        sink: &mut dyn RecordSink,
    ) {
        event.thread_id = Some(session_id.to_owned());
        if !is_child {
            if event.model.is_some() {
                self.root_model = event.model.clone();
            }
            return;
        }
        event.source = EventSource::Subagent;
        if self
            .descendant_sessions
            .get(session_id)
            .is_some_and(|meta| meta.looks_like_fork)
        {
            self.saw_fork_titled_child = true;
        }
        self.spawn_child(session_id, sink);
    }

    /// Emits one `SubagentSpawn` the first time `session_id` streams, up to
    /// [`MAX_TRACKED_CHILD_SESSIONS`]. Past the cap, the session goes
    /// untracked and [`Self::finish`] reports `AttributionIncomplete`.
    fn spawn_child(&mut self, session_id: &str, sink: &mut dyn RecordSink) {
        if self.seen_child_sessions.contains(session_id) {
            return;
        }
        if self.seen_child_sessions.len() >= MAX_TRACKED_CHILD_SESSIONS {
            self.children_capped = true;
            return;
        }
        self.seen_child_sessions.insert(session_id.to_owned());
        let ts_ms = self
            .descendant_sessions
            .get(session_id)
            .and_then(|meta| meta.ts_ms);
        sink.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::SubagentSpawn {
                ts_ms,
                parent_model: self.root_model.clone(),
                provenance: RelationProvenance::SessionParentLink,
            },
        )));
    }

    fn observe_model(&mut self, event: &NormalizedEvent) {
        if self.model.is_none() {
            self.model = event.model.clone();
        }
    }

    fn flush(&mut self, sink: &mut dyn RecordSink) {
        if let Some(pending) = self.pending.take() {
            sink.record(NormalizedRecord::MetricsEvent(Box::new(pending.event)));
            if !pending.content.is_empty() {
                sink.record(NormalizedRecord::TurnContent(Box::new(TurnContent {
                    parts: pending.content,
                })));
            }
        }
    }

    fn finish(&self) -> SessionSummary {
        // A capped or fork-titled descendant means some records could not
        // be attributed to their real relationship: the same kind of
        // attribution loss the Claude adapter's capped thread resolver
        // reports.
        let mut coverage_gaps = Vec::new();
        if self.children_capped || self.saw_fork_titled_child {
            coverage_gaps.push(PartialReason::AttributionIncomplete);
        }
        SessionSummary {
            cache_write_tokens_available: true,
            context_window: None,
            model: self.model.clone(),
            started_at_ms: None,
            coverage_gaps,
            late_tools: Vec::new(),
            initial_context: None,
            skill_descriptions: HashMap::new(),
        }
    }
}

fn message_event(value: &Value, fallback_ts: Option<i64>) -> Option<NormalizedEvent> {
    let object = value.as_object()?;
    let role = role_of(object.get("role").and_then(Value::as_str))?;
    let mut event = NormalizedEvent::new(role);
    event.ts_ms = object
        .get("time")
        .and_then(|time| time.get("created"))
        .and_then(parse_ts)
        .or(fallback_ts);
    event.ts_ms?;
    event.model = string_field(object, &["modelID", "modelId", "model"]);
    event.thinking_mode = string_field(object, &["variant"]);
    event.usage = object
        .get("tokens")
        .and_then(Value::as_object)
        .map(opencode_usage)
        .unwrap_or_default();
    Some(event)
}

fn apply_part(
    value: &Value,
    fallback_ts: Option<i64>,
    pending: &mut PendingMessage,
    sink: &mut dyn RecordSink,
) {
    let Some(object) = value.as_object() else {
        sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
        return;
    };
    if pending.event.ts_ms.is_none() {
        pending.event.ts_ms = object
            .get("time")
            .and_then(|time| time.get("created"))
            .and_then(parse_ts)
            .or(fallback_ts);
    }
    let part_type = object.get("type").and_then(Value::as_str).unwrap_or("");
    match part_type {
        "text" => {
            if let Some(text) = object
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                let kind = if pending.event.role == Role::Assistant {
                    ContentKind::AssistantText
                } else {
                    ContentKind::UserText
                };
                pending.content.push(ContentPart::new(kind, text));
            }
        }
        "file" | "snapshot" | "step-start" | "step-finish" | "agent" | "retry" => {}
        "reasoning" => {
            pending.event.has_thinking = true;
            if let Some(text) = object
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                pending
                    .content
                    .push(ContentPart::new(ContentKind::Thinking, text));
            }
        }
        "tool" => apply_tool_part(object, pending),
        "patch" => pending.event.tools.push(ToolCall {
            name: "patch".to_owned(),
            category: ToolCategory::Edit,
            detail: None,
        }),
        "compaction" => {
            pending.event.is_compaction_boundary = true;
            pending.event.compaction_trigger =
                object.get("auto").and_then(Value::as_bool).map(|auto| {
                    if auto {
                        CompactionTrigger::Auto
                    } else {
                        CompactionTrigger::Manual
                    }
                });
        }
        discriminator if !discriminator.is_empty() => unrecognized(discriminator, sink),
        _ => unrecognized("<missing_part_type>", sink),
    }
}

fn apply_tool_part(part: &Map<String, Value>, pending: &mut PendingMessage) {
    let name = part
        .get("tool")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("tool");
    let state = part.get("state");
    let input = state.and_then(|state| state.get("input"));
    pending.event.tools.push(tool_call_from_input(name, input));
    if let Some(text) = input.and_then(compact_json_text) {
        pending
            .content
            .push(ContentPart::new(ContentKind::ToolInput, text));
    }
    if let Some(output) = state
        .and_then(|state| state.get("output"))
        .and_then(Value::as_str)
        .filter(|output| !output.is_empty())
    {
        pending
            .content
            .push(ContentPart::new(ContentKind::ToolResult, output));
    }
}

fn unrecognized(discriminator: &str, sink: &mut dyn RecordSink) {
    let discriminator = discriminator
        .chars()
        .take(EVIDENCE_STRING_CAP)
        .collect::<String>();
    sink.record(NormalizedRecord::Observation(Box::new(
        EvidenceObservation::UnrecognizedType {
            discriminator,
            inert: false,
        },
    )));
    sink.record(NormalizedRecord::Unusable(
        PartialReason::UnrecognizedRecordType,
    ));
}

fn message_discriminator(value: &Value) -> &str {
    value
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("<missing_message_role>")
}

fn report_invalid_message(value: &Value, sink: &mut dyn RecordSink) {
    if role_of(value.get("role").and_then(Value::as_str)).is_none() {
        unrecognized(message_discriminator(value), sink);
    } else {
        sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
    }
}

fn role_of(role: Option<&str>) -> Option<Role> {
    match role {
        Some("user") => Some(Role::User),
        Some("assistant") => Some(Role::Assistant),
        Some("system") => Some(Role::System),
        Some("tool" | "toolResult") => Some(Role::Tool),
        _ => None,
    }
}

fn string_field(object: &Map<String, Value>, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        object
            .get(*name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn opencode_usage(tokens: &Map<String, Value>) -> Usage {
    let number = |name: &str| tokens.get(name).and_then(Value::as_u64).unwrap_or(0);
    let cache = tokens.get("cache").and_then(Value::as_object);
    let cache_number = |name: &str| {
        cache
            .and_then(|cache| cache.get(name))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Usage {
        input_tokens: number("input"),
        output_tokens: number("output").saturating_add(number("reasoning")),
        cache_read_tokens: cache_number("read"),
        cache_creation_tokens: cache_number("write"),
    }
}

fn parse_db_ts(value: i64) -> Option<i64> {
    parse_ts(&Value::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CountingSink(u64);

    impl RecordSink for CountingSink {
        fn record(&mut self, record: NormalizedRecord) {
            if matches!(record, NormalizedRecord::MetricsEvent(_)) {
                self.0 += 1;
            }
        }

        fn finish(&mut self, _summary: SessionSummary) {}
    }

    #[test]
    fn retained_state_never_exceeds_one_message() {
        let mut state = OpenCodeStreamState::default();
        let mut sink = CountingSink(0);

        for index in 0..10_000 {
            state.observe_export(
                serde_json::json!({
                    "type": "message",
                    "messageID": format!("m{index}"),
                    "time": {"created": index as i64},
                    "payload": {"role": "user"}
                }),
                64,
                &mut sink,
            );
            assert!(state.pending.iter().count() <= 1);
        }
        state.flush(&mut sink);

        assert!(state.pending.is_none());
        assert_eq!(sink.0, 10_000);
    }

    /// Collects every `TurnContent` record a visit emits, in order.
    #[derive(Default)]
    struct ContentCapturingSink {
        contents: Vec<TurnContent>,
    }

    impl RecordSink for ContentCapturingSink {
        fn record(&mut self, record: NormalizedRecord) {
            if let NormalizedRecord::TurnContent(content) = record {
                self.contents.push(*content);
            }
        }

        fn finish(&mut self, _summary: SessionSummary) {}
    }

    #[test]
    fn content_capture_maps_text_reasoning_and_tool_parts() {
        let mut state = OpenCodeStreamState::default();
        let mut sink = ContentCapturingSink::default();

        state.observe_export(
            serde_json::json!({
                "type": "message",
                "messageID": "m1",
                "time": {"created": 1000},
                "payload": {"role": "assistant", "modelID": "model-a"}
            }),
            64,
            &mut sink,
        );
        state.observe_export(
            serde_json::json!({
                "type": "part",
                "messageID": "m1",
                "payload": {"type": "text", "text": "hello there"}
            }),
            64,
            &mut sink,
        );
        state.observe_export(
            serde_json::json!({
                "type": "part",
                "messageID": "m1",
                "payload": {"type": "reasoning", "text": "pondering"}
            }),
            64,
            &mut sink,
        );
        state.observe_export(
            serde_json::json!({
                "type": "part",
                "messageID": "m1",
                "payload": {
                    "type": "tool",
                    "tool": "bash",
                    "state": {"input": {"command": "ls"}, "output": "ok"}
                }
            }),
            64,
            &mut sink,
        );
        state.flush(&mut sink);

        assert_eq!(
            sink.contents.len(),
            1,
            "one TurnContent for the one message"
        );
        let parts = &sink.contents[0].parts;
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0].kind, ContentKind::AssistantText);
        assert_eq!(parts[0].text, "hello there");
        assert_eq!(parts[1].kind, ContentKind::Thinking);
        assert_eq!(parts[1].text, "pondering");
        assert_eq!(parts[2].kind, ContentKind::ToolInput);
        assert_eq!(parts[2].text, r#"{"command":"ls"}"#);
        assert_eq!(parts[3].kind, ContentKind::ToolResult);
        assert_eq!(parts[3].text, "ok");
    }
}
