//! Codex adapter — Codex Desktop / CLI "rollout" transcripts.
//!
//! Codex writes one JSONL file per session
//! (`~/.codex/sessions/<date>/rollout-*.jsonl`). Every line is an envelope
//! `{timestamp, type, payload}`:
//!
//! - `response_item` carries the model-API conversation — `message`
//!   (role + `content[].text`), direct `function_call` / `custom_tool_call`
//!   invocations, their outputs, and `reasoning` (thinking). Current Codex
//!   Desktop sessions wrap real tool calls in a `custom_tool_call` named `exec`
//!   whose input is JavaScript (`tools.apply_patch(...)`,
//!   `tools.exec_command(...)`, …); this adapter lexes those calls as data and
//!   never evaluates the script.
//! - `event_msg` carries UI-layer events. `token_count` gives usage: its
//!   `info.last_token_usage` is the latest turn's usage (its `input_tokens`
//!   is the live prompt size = context-window occupancy) and its
//!   `info.model_context_window` gives the model's real window. (The duplicate
//!   `user_message` / `agent_message` echoes of `response_item` turns are
//!   skipped so turns aren't double-counted.) `thread_settings_applied` gives
//!   the thread's speed: its `thread_settings.service_tier` applies to every
//!   assistant turn after it, until the next `thread_settings_applied` record;
//!   see `service_tier_speed`.
//! - `compacted` is a top-level envelope (not an `event_msg`) that newer Codex
//!   rollouts write when a compaction finishes. Older rollouts instead (or
//!   also) emit `{"type":"event_msg","payload":{"type":"context_compacted"}}`.
//!   Both mark the same event; see `compaction_event` for how the parser
//!   avoids double-counting when a rollout emits both.
//!
//! The shared `parse_record` only understands `role`/`content` at the top level
//! or under `message`, so it drops every Codex line — the data is nested under
//! `payload` and the top-level `type` is `response_item`. This adapter unwraps
//! the envelope.
//!
//! A `(type, payload.type)` combination none of the readers above models does
//! not fail the session closed by default (#229 parity). `is_inert_codex_record`
//! proves a record carries none of the keys a reader consumes before the
//! record is skipped with `Complete` coverage; see its doc comment and
//! `is_recognized_eventless`'s for the allowlist and the structural proof.

use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read};

use anyhow::Context;
use serde_json::{Map, Value};

use super::read_source;
use crate::analysis::framing::{BoundedJsonlReader, FramedRecord, PartialReason, RecordSkip};
use crate::analysis::initial_context::CodexContextAccumulator;
use crate::analysis::interface::{
    ContentKind, ContentPart, EvidenceObservation, NormalizedRecord, RawSource, RecordSink,
    RelationProvenance, SessionInput, SessionSummary, TurnContent, VendorAdapter, VisitOutcome,
};
use crate::analysis::model::{NormalizedEvent, NormalizedSession, Role, ToolCall, Usage};
use crate::analysis::records::{
    compact_json_text, concatenated_text, extract_content_parts_from_container, parse_ts,
    tool_call_from_input,
};
use crate::analysis::source_validity::{AppendOnlyGuarantee, PinnedSource, SourceClaim};

const MAX_PENDING_FORK_ROWS: usize = 256;
const MAX_PENDING_FORK_BYTES: usize = 1024 * 1024;

pub struct CodexAdapter;

impl VendorAdapter for CodexAdapter {
    fn agent(&self) -> &'static str {
        "codex"
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        let content = read_source(&input.source)
            .with_context(|| format!("reading codex session {}", input.session_id))?;
        let (events, context_window, model) = parse_codex(&content);
        Ok(NormalizedSession {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            events,
            cache_write_tokens_available: false,
            context_window,
            model,
        })
    }

    fn visit(
        &self,
        input: &SessionInput,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        (|| -> anyhow::Result<VisitOutcome> {
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
                    anyhow::bail!(
                        "sqlite source must be handled by the sqlite adapter: {}",
                        path.display()
                    )
                }
            };
            sink.finish(summary);
            Ok(VisitOutcome::Unvalidated)
        })()
        .with_context(|| format!("reading codex session {}", input.session_id))
    }

    fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        guarantee: AppendOnlyGuarantee,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        CodexAdapter::visit_claimed(self, input, claim, guarantee, cancel, sink)
    }
}

impl CodexAdapter {
    pub fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        guarantee: AppendOnlyGuarantee,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        (|| -> anyhow::Result<VisitOutcome> {
            let RawSource::File(path) = &input.source else {
                anyhow::bail!("a claimed Codex source must be a file");
            };
            let mut pinned = match PinnedSource::open(path, claim.clone())? {
                Ok(pinned) => pinned,
                Err(reason) => return Ok(VisitOutcome::SourceChanged(reason)),
            };
            let limit = match guarantee {
                AppendOnlyGuarantee::Evidenced => claim.boundary,
                AppendOnlyGuarantee::Absent => u64::MAX,
            };
            let summary = self.visit_reader(BufReader::new(pinned.reader(limit)), cancel, sink)?;
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
        .with_context(|| format!("reading claimed Codex session {}", input.session_id))
    }

    fn visit_reader(
        &self,
        reader: impl BufRead,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<SessionSummary> {
        let mut reader = BoundedJsonlReader::new(reader);
        let mut state = CodexStreamState::default();

        while let Some(record) = reader.next_record(cancel) {
            match record {
                FramedRecord::Skipped(skip) => match skip {
                    RecordSkip::Oversized { .. } | RecordSkip::IncompleteTail { .. } => {
                        sink.record(NormalizedRecord::Unusable(skip.partial_reason()));
                    }
                    RecordSkip::ReadFailed { index, kind } => {
                        anyhow::bail!("Codex record {index} read failed: {kind:?}");
                    }
                    RecordSkip::Cancelled { index } => {
                        anyhow::bail!("Codex record {index} read was cancelled");
                    }
                },
                FramedRecord::Complete { bytes, .. } => {
                    let record = std::str::from_utf8(bytes)
                        .context("Codex transcript record is not valid UTF-8")?;
                    let Ok(value) = serde_json::from_str::<Value>(record) else {
                        sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
                        continue;
                    };
                    state.observe(value, bytes.len(), sink);
                }
            }
        }

        Ok(state.finish(sink))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ForkOwnership {
    #[default]
    TopLevel,
    Pending,
    Owned,
}

#[derive(Default)]
struct CodexStreamState {
    ownership: ForkOwnership,
    agent_path: Option<String>,
    pending_rows: Vec<Value>,
    pending_bytes: usize,
    pending_owned_start: Option<usize>,
    fork_attribution_incomplete: bool,
    previous_token_count_key: Option<TokenCountKey>,
    previous_event_was_boundary: bool,
    previous_boundary_ts: Option<i64>,
    context_window: Option<u64>,
    model: Option<String>,
    current_model: Option<String>,
    current_thinking_mode: Option<String>,
    current_speed: Option<String>,
    started_at_ms: Option<i64>,
    owned_token_count_seen: bool,
    effort_seen: bool,
    context: CodexContextAccumulator,
}

impl CodexStreamState {
    fn observe(&mut self, value: Value, record_bytes: usize, sink: &mut dyn RecordSink) {
        if record_to_event(&value).is_some_and(|event| event.ts_ms.is_none()) {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            return;
        }

        self.context.observe(&value);
        let record_type = value.get("type").and_then(Value::as_str);

        if record_type == Some("session_meta") {
            if self.started_at_ms.is_none() {
                self.started_at_ms = value
                    .pointer("/payload/timestamp")
                    .and_then(parse_ts)
                    .or_else(|| value.get("timestamp").and_then(parse_ts));
            }
            if value
                .pointer("/payload/thread_source")
                .and_then(Value::as_str)
                == Some("subagent")
            {
                self.ownership = ForkOwnership::Pending;
                self.agent_path = value
                    .pointer("/payload/agent_path")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
        }

        if self.ownership == ForkOwnership::Pending {
            self.observe_pending(value, record_bytes, sink);
        } else {
            self.process_value(value, true, sink);
        }
    }

    fn observe_pending(&mut self, value: Value, record_bytes: usize, sink: &mut dyn RecordSink) {
        let record_type = value.get("type").and_then(Value::as_str);
        let payload_type = value.pointer("/payload/type").and_then(Value::as_str);

        if record_type == Some("event_msg") && payload_type == Some("task_started") {
            self.pending_owned_start = Some(
                self.pending_rows
                    .len()
                    .checked_sub(1)
                    .filter(|index| is_developer_message(&self.pending_rows[*index]))
                    .unwrap_or(self.pending_rows.len()),
            );
        }

        let addressed_to_child = record_type == Some("response_item")
            && payload_type == Some("agent_message")
            && value
                .pointer("/payload/recipient")
                .and_then(Value::as_str)
                .zip(self.agent_path.as_deref())
                .is_some_and(|(recipient, path)| recipient == path);
        if self.fork_attribution_incomplete {
            self.pending_owned_start = None;
            self.process_value(value, false, sink);
            if addressed_to_child {
                self.ownership = ForkOwnership::Owned;
            }
            return;
        }

        if self.pending_rows.len() == MAX_PENDING_FORK_ROWS
            || self.pending_bytes.saturating_add(record_bytes) > MAX_PENDING_FORK_BYTES
        {
            for pending in std::mem::take(&mut self.pending_rows) {
                self.process_value(pending, false, sink);
            }
            self.pending_bytes = 0;
            self.pending_owned_start = None;
            self.fork_attribution_incomplete = true;
            self.process_value(value, false, sink);
            if addressed_to_child {
                self.ownership = ForkOwnership::Owned;
            }
            return;
        }

        self.pending_bytes = self.pending_bytes.saturating_add(record_bytes);
        self.pending_rows.push(value);

        if addressed_to_child {
            let owned_start = self
                .pending_owned_start
                .unwrap_or(self.pending_rows.len() - 1);
            let pending_rows = std::mem::take(&mut self.pending_rows);
            for (index, value) in pending_rows.into_iter().enumerate() {
                self.process_value(value, index >= owned_start, sink);
            }
            self.pending_bytes = 0;
            self.pending_owned_start = None;
            self.ownership = ForkOwnership::Owned;
        }
    }

    fn process_value(&mut self, value: Value, usage_is_owned: bool, sink: &mut dyn RecordSink) {
        let record_type = value.get("type").and_then(Value::as_str);
        let payload_type = value.pointer("/payload/type").and_then(Value::as_str);
        let is_token_count =
            record_type == Some("event_msg") && payload_type == Some("token_count");
        // A thread-level setting: update it whether or not this record's own
        // usage belongs to this rollout, so a child rollout's owned turns
        // still inherit the tier a `thread_settings_applied` record set in
        // the replayed parent history that precedes them.
        if let Some(speed) = service_tier_speed(&value) {
            self.current_speed = Some(speed);
        }
        if is_token_count {
            if let Some(key) = token_count_key(&value) {
                let duplicate = self.previous_token_count_key.as_ref() == Some(&key);
                self.previous_token_count_key = Some(key);
                if duplicate {
                    return;
                }
            }
            if !usage_is_owned {
                return;
            }
        }

        if usage_is_owned {
            self.observe_model_and_effort(&value);
            if self.context_window.is_none() {
                self.context_window = value
                    .pointer("/payload/info/model_context_window")
                    .and_then(Value::as_u64)
                    .filter(|window| *window > 0);
            }
            if is_spawn_agent_call(record_type, payload_type, &value) {
                sink.record(NormalizedRecord::Observation(Box::new(
                    EvidenceObservation::SubagentSpawn {
                        ts_ms: value.get("timestamp").and_then(parse_ts),
                        parent_model: self.current_model.clone(),
                        provenance: RelationProvenance::SpawnAgentCall,
                    },
                )));
            }
        }

        if let Some(mut event) = record_to_event(&value) {
            if usage_is_owned {
                event.model = event.model.or_else(|| self.current_model.clone());
                event.thinking_mode = self.current_thinking_mode.clone();
                apply_thread_speed(&mut event, &self.current_speed);
            }
            if is_token_count {
                self.owned_token_count_seen = true;
            }
            if self.is_duplicate_boundary(&event) {
                return;
            }
            let content_parts = content_parts_for_record(&value);
            sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
            if !content_parts.is_empty() {
                sink.record(NormalizedRecord::TurnContent(Box::new(TurnContent {
                    parts: content_parts,
                })));
            }
        } else {
            let allowlisted = is_recognized_eventless(record_type, payload_type);
            let inert = if !allowlisted {
                is_inert_codex_record(&value, true)
            } else if is_proven_echo(record_type, payload_type) {
                is_inert_codex_record(&value, false)
            } else {
                true
            };
            if !inert || !allowlisted {
                sink.record(NormalizedRecord::Observation(Box::new(
                    EvidenceObservation::UnrecognizedType {
                        discriminator: codex_discriminator(&value),
                        inert,
                    },
                )));
            }
            if !inert {
                sink.record(NormalizedRecord::Unusable(
                    PartialReason::UnrecognizedRecordType,
                ));
            }
        }
    }

    fn observe_model_and_effort(&mut self, value: &Value) {
        if let Some(next_model) = [
            "/payload/model",
            "/payload/info/model",
            "/payload/turn_context/model",
        ]
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .map(str::trim)
        .filter(|model| !model.is_empty())
        {
            self.current_model = Some(next_model.to_owned());
            if self.model.is_none() {
                self.model = self.current_model.clone();
            }
        }
        if let Some(next_mode) = [
            "/payload/effort",
            "/payload/reasoning_effort",
            "/payload/turn_context/effort",
            "/payload/turn_context/reasoning_effort",
            "/payload/thread_settings/reasoning_effort",
            "/payload/collaboration_mode/settings/reasoning_effort",
        ]
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
        {
            self.current_thinking_mode = Some(next_mode.to_owned());
            self.effort_seen = true;
        }
    }

    fn is_duplicate_boundary(&mut self, event: &NormalizedEvent) -> bool {
        let duplicate = event.is_compaction_boundary
            && self.previous_event_was_boundary
            && event
                .ts_ms
                .zip(self.previous_boundary_ts)
                .is_none_or(|(current, previous)| {
                    (current - previous).abs() <= COMPACTION_DEDUPE_WINDOW_MS
                });
        if !duplicate {
            self.previous_event_was_boundary = event.is_compaction_boundary;
            if event.is_compaction_boundary {
                self.previous_boundary_ts = event.ts_ms;
            }
        }
        duplicate
    }

    fn finish(mut self, sink: &mut dyn RecordSink) -> SessionSummary {
        if self.ownership == ForkOwnership::Pending {
            for value in std::mem::take(&mut self.pending_rows) {
                self.process_value(value, true, sink);
            }
        }
        let coverage_gaps = if self.fork_attribution_incomplete
            || (self.owned_token_count_seen && !self.effort_seen)
        {
            vec![PartialReason::AttributionIncomplete]
        } else {
            Vec::new()
        };
        let (initial_context, skill_descriptions) = self.context.finish();
        SessionSummary {
            cache_write_tokens_available: false,
            context_window: self.context_window,
            model: self.model,
            started_at_ms: self.started_at_ms,
            coverage_gaps,
            late_tools: Vec::new(),
            initial_context,
            skill_descriptions,
        }
    }
}

fn is_developer_message(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("response_item")
        && value.pointer("/payload/type").and_then(Value::as_str) == Some("message")
        && value.pointer("/payload/role").and_then(Value::as_str) == Some("developer")
}

/// True for a `spawn_agent` function call: the record a Codex parent emits
/// to start a subagent.
fn is_spawn_agent_call(
    record_type: Option<&str>,
    payload_type: Option<&str>,
    value: &Value,
) -> bool {
    record_type == Some("response_item")
        && payload_type == Some("function_call")
        && value.pointer("/payload/name").and_then(Value::as_str) == Some("spawn_agent")
}

/// Returns true for a Codex `(type, payload.type)` pair this adapter treats as
/// carrying no per-turn signal.
///
/// `session_meta`, `turn_context`, `world_state`, the listed `event_msg`
/// payloads, and `response_item`/`agent_message` are proven eventless by
/// shape alone: their own evidence-bearing fields — `turn_context.model` /
/// `.effort`, `thread_settings_applied.thread_settings.service_tier` — are
/// read by `observe_model_and_effort` / `service_tier_speed` on every record,
/// before this predicate ever runs, so nothing about them is left unproven.
///
/// `item_completed` and top-level `inter_agent_communication_metadata` are
/// different: #229-parity measurement (1,034 local rollouts) found
/// `item_completed` is a completion echo of a `response_item` this adapter
/// already models — its `item.type` is one of `Reasoning`, `AgentMessage`,
/// `CommandExecution`, `FileChange`, `UserMessage`, `Extension`,
/// `SubAgentActivity`, `CollabAgentToolCall`, `ContextCompaction`,
/// `McpToolCall`, or `ImageView`, and no sampled record carried usage, a
/// model, or an effort. Its `McpToolCall` and `CommandExecution` items do
/// carry tool-like keys (`tool`, `server`, `arguments`, `command`), so a
/// strict scan rejects them; `is_proven_echo` names below route these two
/// through the light structural check instead (`is_inert_codex_record`'s
/// `reject_nested = false` pass), so a record that starts carrying real
/// evidence still fails closed. `inter_agent_communication_metadata` carries
/// only a `trigger_turn` link id.
fn is_recognized_eventless(record_type: Option<&str>, payload_type: Option<&str>) -> bool {
    matches!(
        record_type,
        Some(
            "session_meta" | "turn_context" | "world_state" | "inter_agent_communication_metadata"
        )
    ) || matches!(
        (record_type, payload_type),
        (
            Some("event_msg"),
            Some(
                "task_started"
                    | "task_complete"
                    | "turn_started"
                    | "turn_complete"
                    | "user_message"
                    | "agent_message"
                    | "turn_aborted"
                    | "thread_settings_applied"
                    | "item_completed"
            )
        )
    ) || matches!(
        (record_type, payload_type),
        (Some("response_item"), Some("agent_message"))
    )
}

/// The subset of `is_recognized_eventless` names that must still pass the
/// light structural check (`is_inert_codex_record`'s `reject_nested = false`
/// pass) before an unrecognized-record observation is skipped. See
/// `is_recognized_eventless`'s doc comment for why the rest of the allowlist
/// does not need this: their fields are already read elsewhere.
fn is_proven_echo(record_type: Option<&str>, payload_type: Option<&str>) -> bool {
    record_type == Some("inter_agent_communication_metadata")
        || matches!(
            (record_type, payload_type),
            (Some("event_msg"), Some("item_completed"))
        )
}

/// Scalar keys a CODEX reader reads directly, at the top level of a record or
/// inside its `payload` object: `token_count_event` / `codex_usage` (usage
/// buckets), `observe_model_and_effort` (model, effort), `service_tier_speed`
/// (service tier), and `message_event` (role). Presence of any of these at a
/// location [`is_inert_codex_record`]'s light check covers blocks the record
/// from clearing that check, no matter the value.
const CODEX_SCALAR_EVIDENCE_KEYS: &[&str] = &[
    "usage",
    "last_token_usage",
    "total_token_usage",
    "info",
    "input_tokens",
    "output_tokens",
    "cached_input_tokens",
    "reasoning_output_tokens",
    "model",
    "effort",
    "reasoning_effort",
    "service_tier",
    "thread_settings",
    "role",
];

/// `payload.type` values `record_to_event` dispatches to a reader, plus the
/// top-level `compacted` envelope type. A record carrying one of these values
/// as its own `type` field proves it holds a shape a reader consumes, even
/// through a `(type, payload.type)` combination `record_to_event` does not
/// (yet) match.
const CODEX_DISPATCHED_TYPES: &[&str] = &[
    "message",
    "reasoning",
    "function_call_output",
    "custom_tool_call_output",
    "tool_search_output",
    "mcp_tool_call_output",
    "custom_tool_call",
    "local_shell_call",
    "tool_search_call",
    "web_search_call",
    "image_generation_call",
    "compaction",
    "context_compaction",
    "context_compacted",
    "token_count",
    "compacted",
];

/// Returns true when an object carries the `function_call_event` /
/// `custom_tool_call_event` tool shape: a non-empty `name`, together with
/// `arguments`, `input`, or `call_id`. A `name` with no non-empty value, or no
/// sibling of the three, is inert — `push_named_tool_str`'s Claude/generic
/// counterpart likewise ignores an empty tool name.
fn has_named_tool_shape(object: &Map<String, Value>) -> bool {
    object
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|name| !name.is_empty())
        && (object.contains_key("arguments")
            || object.contains_key("input")
            || object.contains_key("call_id"))
}

/// Returns true when an unrecognized Codex envelope carries no evidence any
/// CODEX reader consumes.
///
/// Mirrors `records::is_inert_record`, but for the Codex envelope shape
/// (`payload` in place of Claude's `message`) and the readers this file
/// defines. `reject_nested` selects the strict any-depth scan, for a
/// genuinely unrecognized `(type, payload.type)` pair, or the light scan that
/// only reads the root and the root's `payload` object, for a name
/// `is_proven_echo` names (an echo record whose nested `item` cannot carry
/// evidence a reader reads — see `is_recognized_eventless`). A non-object
/// record fails closed.
fn is_inert_codex_record(value: &Value, reject_nested: bool) -> bool {
    if !value.is_object() {
        return false;
    }

    let mut pending = vec![(value, true)];
    while let Some((value, reads_scalar_keys)) = pending.pop() {
        match value {
            Value::Object(object) => {
                if reads_scalar_keys
                    && (CODEX_SCALAR_EVIDENCE_KEYS
                        .iter()
                        .any(|key| object.contains_key(*key))
                        || object
                            .get("type")
                            .and_then(Value::as_str)
                            .is_some_and(|kind| CODEX_DISPATCHED_TYPES.contains(&kind))
                        || has_named_tool_shape(object))
                {
                    return false;
                }

                pending.extend(object.iter().map(|(key, child)| {
                    let reads_scalar_keys =
                        reject_nested || (reads_scalar_keys && key == "payload");
                    (child, reads_scalar_keys)
                }));
            }
            Value::Array(items) => {
                pending.extend(items.iter().map(|item| (item, reject_nested)));
            }
            _ => {}
        }
    }

    true
}

/// A Codex record's discriminator: `<type>` alone, or `<type>.<payload.type>`
/// when the record carries a `payload.type`. Codex discriminators are enum
/// names the vendor writes, never user content, so unlike Claude's
/// `record_discriminator` this need not fall back to a fixed placeholder.
/// `evidence_sink.rs`'s `observe_observation` bounds and caps the returned
/// string the same way regardless of vendor (`EVIDENCE_STRING_CAP`,
/// `MAX_UNRECOGNIZED_TYPES`).
fn codex_discriminator(value: &Value) -> String {
    let record_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("<missing>");
    match value.pointer("/payload/type").and_then(Value::as_str) {
        Some(payload_type) => format!("{record_type}.{payload_type}"),
        None => record_type.to_owned(),
    }
}

/// Read the per-turn speed a `thread_settings_applied` record sets, from its
/// `payload.thread_settings.service_tier`. Returns `None` for any other
/// record type, and for a missing or empty tier (the caller then leaves the
/// current speed unchanged). `"priority"` maps to `"fast"` and `"default"`
/// maps to `"standard"`, matching Claude's speed vocabulary; any other
/// non-empty tier is kept as its own raw label, so an unreviewed tier still
/// shows up as its own key in `fast_modes` instead of vanishing.
fn service_tier_speed(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) != Some("event_msg")
        || value.pointer("/payload/type").and_then(Value::as_str) != Some("thread_settings_applied")
    {
        return None;
    }
    let tier = value
        .pointer("/payload/thread_settings/service_tier")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|tier| !tier.is_empty())?;
    Some(match tier {
        "priority" => "fast".to_owned(),
        "default" => "standard".to_owned(),
        other => other.to_owned(),
    })
}

/// Attach the thread's current speed to an assistant event that carries none
/// of its own. Every Codex event starts with no speed today, but the guard
/// keeps a future record type that reports its own per-turn speed from being
/// overwritten.
fn apply_thread_speed(event: &mut NormalizedEvent, current_speed: &Option<String>) {
    if event.role == Role::Assistant && event.speed.is_none() {
        event.speed = current_speed.clone();
    }
}

fn parse_codex(content: &str) -> (Vec<NormalizedEvent>, Option<u64>, Option<String>) {
    let mut events = Vec::new();
    // A forked rollout starts by replaying its parent's history. Keep those
    // records available to the desktop analysis view, but do not attribute
    // their already-billed token_count events to the child. The first task
    // addressed to the child's agent path marks the owned usage boundary.
    let owned_usage_start = codex_fork_owned_offset(content);
    // The model's context-window size, reported on each `token_count` event's
    // `info.model_context_window`. Constant per model; take the first seen.
    let mut context_window = None;
    // The model id, reported on the `session_meta` / `turn_context` envelope.
    // Best-effort across the pointers Codex has used; first non-empty wins. `None`
    // is fine — cost then has no local estimate for the session.
    let mut model: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut current_thinking_mode: Option<String> = None;
    // The thread-level speed a `thread_settings_applied` record last set, from
    // its `service_tier`. Tracked regardless of `usage_is_owned` — see
    // `service_tier_speed` — so a child rollout's owned turns still inherit
    // the tier a replayed parent record set before the owned window starts.
    let mut current_speed: Option<String> = None;
    // Dedupe state for compaction boundaries: some rollouts write a
    // `context_compacted` event_msg and a top-level `compacted` record
    // back-to-back for the same compaction (see `compaction_event`).
    let mut previous_event_was_boundary = false;
    let mut previous_boundary_ts: Option<i64> = None;
    // Dedupe state for `token_count` rows (see `token_count_key`).
    let mut previous_token_count_key: Option<TokenCountKey> = None;
    let mut offset = 0;
    for line_with_ending in content.split_inclusive('\n') {
        let line_offset = offset;
        offset += line_with_ending.len();
        let line = line_with_ending.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            let usage_is_owned = owned_usage_start.is_none_or(|start| line_offset >= start);
            if let Some(speed) = service_tier_speed(&value) {
                current_speed = Some(speed);
            }
            if usage_is_owned && context_window.is_none() {
                context_window = value
                    .pointer("/payload/info/model_context_window")
                    .and_then(Value::as_u64)
                    .filter(|&w| w > 0);
            }
            if usage_is_owned {
                if let Some(next_model) = [
                    "/payload/model",
                    "/payload/info/model",
                    "/payload/turn_context/model",
                ]
                .iter()
                .find_map(|p| value.pointer(p).and_then(Value::as_str))
                .map(str::trim)
                .filter(|model| !model.is_empty())
                {
                    current_model = Some(next_model.to_string());
                    if model.is_none() {
                        model = current_model.clone();
                    }
                }
                if let Some(next_mode) = [
                    "/payload/effort",
                    "/payload/reasoning_effort",
                    "/payload/turn_context/effort",
                    "/payload/turn_context/reasoning_effort",
                    "/payload/thread_settings/reasoning_effort",
                    "/payload/collaboration_mode/settings/reasoning_effort",
                ]
                .iter()
                .find_map(|p| value.pointer(p).and_then(Value::as_str))
                .map(str::trim)
                .filter(|mode| !mode.is_empty())
                {
                    current_thinking_mode = Some(next_mode.to_string());
                }
            }
            let inherited_token_count = !usage_is_owned
                && value.get("type").and_then(Value::as_str) == Some("event_msg")
                && value.pointer("/payload/type").and_then(Value::as_str) == Some("token_count");
            if let Some(key) = token_count_key(&value) {
                let duplicate_token_count = previous_token_count_key.as_ref() == Some(&key);
                previous_token_count_key = Some(key);
                if duplicate_token_count {
                    continue;
                }
            }
            if !inherited_token_count && let Some(mut ev) = record_to_event(&value) {
                if usage_is_owned {
                    ev.model = ev.model.or_else(|| current_model.clone());
                    ev.thinking_mode = current_thinking_mode.clone();
                    apply_thread_speed(&mut ev, &current_speed);
                }
                let duplicate_boundary = ev.is_compaction_boundary
                    && previous_event_was_boundary
                    && ev
                        .ts_ms
                        .zip(previous_boundary_ts)
                        .is_none_or(|(cur, prev)| {
                            (cur - prev).abs() <= COMPACTION_DEDUPE_WINDOW_MS
                        });
                if !duplicate_boundary {
                    previous_event_was_boundary = ev.is_compaction_boundary;
                    if ev.is_compaction_boundary {
                        previous_boundary_ts = ev.ts_ms;
                    }
                    events.push(ev);
                }
            }
        }
    }
    (events, context_window, model)
}

/// Locate the first row whose token usage belongs to a Codex fork itself.
///
/// Codex rehydrates the parent's rollout into a child file before the child's
/// first task. Those inherited rows are useful context, but their token_count
/// events describe requests already made by the parent. A task addressed to the
/// child's agent path ends the replay; include its preceding developer message
/// when Codex emits one immediately before task_started.
fn codex_fork_owned_offset(content: &str) -> Option<usize> {
    let mut agent_path: Option<String> = None;
    let mut last_task_started_offset: Option<usize> = None;
    let mut previous_row: Option<(usize, bool)> = None;
    let mut offset = 0;

    for line_with_ending in content.split_inclusive('\n') {
        let line = line_with_ending.trim();
        let value = serde_json::from_str::<Value>(line).ok();

        if let Some(value) = value.as_ref() {
            let row_type = value.get("type").and_then(Value::as_str);
            let payload_type = value.pointer("/payload/type").and_then(Value::as_str);

            if row_type == Some("session_meta")
                && value
                    .pointer("/payload/thread_source")
                    .and_then(Value::as_str)
                    == Some("subagent")
            {
                agent_path = value
                    .pointer("/payload/agent_path")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }

            if row_type == Some("event_msg") && payload_type == Some("task_started") {
                last_task_started_offset = Some(
                    previous_row
                        .filter(|(_, is_developer_message)| *is_developer_message)
                        .map(|(previous_offset, _)| previous_offset)
                        .unwrap_or(offset),
                );
            }

            let addressed_to_child = row_type == Some("response_item")
                && payload_type == Some("agent_message")
                && value
                    .pointer("/payload/recipient")
                    .and_then(Value::as_str)
                    .zip(agent_path.as_deref())
                    .is_some_and(|(recipient, path)| recipient == path);
            if addressed_to_child {
                return Some(last_task_started_offset.unwrap_or(offset));
            }

            let is_developer_message = row_type == Some("response_item")
                && payload_type == Some("message")
                && value.pointer("/payload/role").and_then(Value::as_str) == Some("developer");
            previous_row = Some((offset, is_developer_message));
        }

        offset += line_with_ending.len();
    }

    None
}

/// Map one rollout envelope record to a normalized event, or `None` for framing
/// / bookkeeping records that carry no analyzable signal (`session_meta`,
/// `turn_context`, `task_started`, and the `user_message` / `agent_message` UI
/// echoes of `response_item` turns).
fn record_to_event(record: &Value) -> Option<NormalizedEvent> {
    let obj = record.as_object()?;
    let ts = obj.get("timestamp").and_then(parse_ts);
    let rec_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let payload = obj.get("payload").and_then(|p| p.as_object())?;
    let payload_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match (rec_type, payload_type) {
        ("response_item", "message") => message_event(payload, ts),
        ("response_item", "reasoning") => Some(reasoning_event(payload, ts)),
        ("response_item", "function_call_output")
        | ("response_item", "custom_tool_call_output")
        | ("response_item", "tool_search_output")
        | ("response_item", "mcp_tool_call_output") => Some(tool_output_event(payload, ts)),
        ("response_item", "custom_tool_call") => custom_tool_call_event(payload, ts),
        ("response_item", "local_shell_call") => {
            Some(named_tool_event("local_shell", payload.get("action"), ts))
        }
        ("response_item", "tool_search_call") => Some(named_tool_event(
            "tool_search",
            payload.get("arguments"),
            ts,
        )),
        ("response_item", "web_search_call") => Some(named_tool_event("web_search", None, ts)),
        ("response_item", "image_generation_call") => {
            Some(named_tool_event("image_generation", None, ts))
        }
        ("response_item", "compaction" | "context_compaction") => Some(compaction_event(ts)),
        ("response_item", _) if payload.contains_key("name") => function_call_event(payload, ts),
        ("event_msg", "token_count") => token_count_event(payload, ts),
        ("event_msg", "context_compacted") => Some(compaction_event(ts)),
        ("compacted", _) => Some(compaction_event(ts)),
        _ => None,
    }
}

/// Extract one rollout envelope record's message content as
/// [`ContentPart`]s, for the `turn_content` capture. Mirrors the
/// `(rec_type, payload_type)` dispatch [`record_to_event`] uses, but the
/// tool-call shapes [`record_to_event`] does not turn into a `NormalizedEvent`
/// on their own (`local_shell_call`, `tool_search_call`, `web_search_call`,
/// `image_generation_call`) are not captured here.
fn content_parts_for_record(record: &Value) -> Vec<ContentPart> {
    let Some(obj) = record.as_object() else {
        return Vec::new();
    };
    let rec_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
    let Some(payload) = obj.get("payload").and_then(Value::as_object) else {
        return Vec::new();
    };
    let payload_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
    match (rec_type, payload_type) {
        ("response_item", "message") => message_content_parts(payload),
        ("response_item", "reasoning") => reasoning_content_parts(payload),
        ("response_item", "function_call_output")
        | ("response_item", "custom_tool_call_output")
        | ("response_item", "tool_search_output")
        | ("response_item", "mcp_tool_call_output") => tool_output_content_parts(payload),
        ("response_item", "custom_tool_call") => function_call_content_parts(payload),
        ("response_item", _) if payload.contains_key("name") => {
            function_call_content_parts(payload)
        }
        _ => Vec::new(),
    }
}

/// A `message` response_item's `content[]` (Codex's OpenAI-shaped
/// `input_text` / `output_text` blocks), captured through the shared JSONL
/// content extractor.
fn message_content_parts(payload: &Map<String, Value>) -> Vec<ContentPart> {
    let role = match payload.get("role").and_then(Value::as_str) {
        Some("assistant") => Role::Assistant,
        // `user`, `system`, and `developer` all capture as user-side text —
        // `ContentKind` has no separate system kind.
        _ => Role::User,
    };
    extract_content_parts_from_container(payload, role)
}

/// A `reasoning` response_item's `summary[]` text, concatenated into one
/// `Thinking` part. Empty when the transcript carries no summary text (Codex
/// often logs reasoning with an empty summary and encrypted content only).
fn reasoning_content_parts(payload: &Map<String, Value>) -> Vec<ContentPart> {
    let Some(summary) = payload.get("summary").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut text = String::new();
    for item in summary {
        if let Some(part) = item.get("text").and_then(Value::as_str) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(part);
        }
    }
    if text.is_empty() {
        Vec::new()
    } else {
        vec![ContentPart::new(ContentKind::Thinking, text)]
    }
}

/// A tool call's `name` + `arguments`/`input`, as one `ToolInput` part.
/// Covers both `function_call`-shaped records and `custom_tool_call` (whose
/// `exec` wrapper input is a JavaScript string, kept as-is).
fn function_call_content_parts(payload: &Map<String, Value>) -> Vec<ContentPart> {
    let input = payload.get("arguments").or_else(|| payload.get("input"));
    input
        .and_then(compact_json_text)
        .into_iter()
        .map(|text| ContentPart::new(ContentKind::ToolInput, text))
        .collect()
}

/// A tool call output's plain `output` string (`function_call_output` and
/// its siblings), or the concatenated text of a `content[]` array when the
/// output is block-shaped instead.
fn tool_output_content_parts(payload: &Map<String, Value>) -> Vec<ContentPart> {
    let text = payload
        .get("output")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .or_else(|| concatenated_text(payload.get("content")));
    text.into_iter()
        .map(|text| ContentPart::new(ContentKind::ToolResult, text))
        .collect()
}

fn message_event(payload: &Map<String, Value>, ts: Option<i64>) -> Option<NormalizedEvent> {
    let role = match payload.get("role").and_then(|r| r.as_str()) {
        Some("assistant") => Role::Assistant,
        Some("user") => Role::User,
        // Codex injects instructions as `developer` / `system` turns.
        Some("system") | Some("developer") => Role::System,
        _ => return None,
    };
    let mut ev = NormalizedEvent::new(role);
    ev.ts_ms = ts;
    Some(ev)
}

fn reasoning_event(_payload: &Map<String, Value>, ts: Option<i64>) -> NormalizedEvent {
    let mut ev = NormalizedEvent::new(Role::Assistant);
    ev.ts_ms = ts;
    // A `reasoning` response_item is Codex's chain-of-thought turn, the
    // vendor equivalent of a Claude `thinking` content block.
    ev.has_thinking = true;
    ev
}

fn function_call_event(payload: &Map<String, Value>, ts: Option<i64>) -> Option<NormalizedEvent> {
    let name = payload
        .get("name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.is_empty())?;
    let mut ev = NormalizedEvent::new(Role::Assistant);
    ev.ts_ms = ts;
    // `arguments` is a JSON-encoded string; the shared builder digs out the shell
    // command (so a Bash-class call that runs tests reclassifies to Testing) and
    // the skill name (when this is a `Skill` call) from the same input.
    let input = payload.get("arguments").or_else(|| payload.get("input"));
    ev.tools.push(tool_call_from_input(name, input));
    Some(ev)
}

/// Normalize a Codex custom tool call.
///
/// Current Codex Desktop wraps one or more actual tool calls in an outer `exec`
/// script. When that bounded shape is recognized, expose the nested tools and
/// omit the wrapper from `tools` so tool-mix accounting reflects the work
/// itself; `wrapper_tool` still names the wrapper, so its own use as a
/// built-in tool is not lost. Unknown/malformed scripts retain the outer
/// `exec` Bash fallback (in `tools`, with no `wrapper_tool`).
fn custom_tool_call_event(
    payload: &Map<String, Value>,
    ts: Option<i64>,
) -> Option<NormalizedEvent> {
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())?;
    let input = payload.get("input").or_else(|| payload.get("arguments"));
    let mut ev = NormalizedEvent::new(Role::Assistant);
    ev.ts_ms = ts;

    if name == "exec"
        && let Some(script) = input.and_then(Value::as_str)
    {
        ev.tools = nested_exec_tool_calls(script);
    }
    if ev.tools.is_empty() {
        ev.tools.push(tool_call_from_input(name, input));
    } else {
        // The wrapper itself is the real built-in tool whose definition costs
        // tokens. Record its use separately from `tools`, so tool-mix
        // accounting still reflects only the nested work it did.
        ev.wrapper_tool = Some(name.to_string());
    }
    Some(ev)
}

/// Lex `tools.<identifier>(...)` calls from a Codex `exec` script without
/// executing or fully parsing JavaScript. String/template contents and comments
/// are skipped so examples or command output cannot masquerade as invocations.
/// A JSON-compatible object first argument is retained for command-aware
/// classification. For the JavaScript-object form Codex also emits
/// (`exec_command({cmd:"cargo test"})`), the string-valued `cmd` property is
/// extracted lexically.
fn nested_exec_tool_calls(script: &str) -> Vec<ToolCall> {
    let bytes = script.as_bytes();
    let mut tools = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = skip_javascript_string(bytes, index);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = skip_line_comment(bytes, index + 2);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index + 2);
                continue;
            }
            _ => {}
        }

        let Some(mut cursor) = match_identifier(bytes, index, b"tools") else {
            index += 1;
            continue;
        };
        cursor = skip_ascii_whitespace(bytes, cursor);
        if bytes.get(cursor) != Some(&b'.') {
            index += 1;
            continue;
        }
        cursor = skip_ascii_whitespace(bytes, cursor + 1);
        let name_start = cursor;
        while bytes
            .get(cursor)
            .is_some_and(|byte| is_javascript_identifier_continue(*byte))
        {
            cursor += 1;
        }
        if cursor == name_start {
            index += 1;
            continue;
        }
        let name = &script[name_start..cursor];
        cursor = skip_ascii_whitespace(bytes, cursor);
        if bytes.get(cursor) != Some(&b'(') {
            index += 1;
            continue;
        }

        let argument_start = skip_ascii_whitespace(bytes, cursor + 1);
        let parsed_argument = (bytes.get(argument_start) == Some(&b'{'))
            .then(|| balanced_object_end(bytes, argument_start))
            .flatten()
            .and_then(|end| parse_object_argument(&script[argument_start..end]));
        tools.push(tool_call_from_input(name, parsed_argument.as_ref()));
        index = cursor + 1;
    }

    tools
}

fn parse_object_argument(argument: &str) -> Option<Value> {
    serde_json::from_str(argument).ok().or_else(|| {
        let command = javascript_object_string_property(argument, b"cmd")?;
        let mut object = Map::new();
        object.insert("cmd".to_string(), Value::String(command));
        Some(Value::Object(object))
    })
}

/// Extract a top-level, string-valued property from a JavaScript object literal.
/// This intentionally supports only the bounded shape needed for command
/// classification; expressions and nested properties are ignored.
fn javascript_object_string_property(argument: &str, property: &[u8]) -> Option<String> {
    let bytes = argument.as_bytes();
    let mut object_depth = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = skip_javascript_string(bytes, index);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = skip_line_comment(bytes, index + 2);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index + 2);
                continue;
            }
            b'{' => object_depth += 1,
            b'}' => object_depth = object_depth.checked_sub(1)?,
            _ if object_depth == 1 => {
                let Some(mut cursor) = match_identifier(bytes, index, property) else {
                    index += 1;
                    continue;
                };
                cursor = skip_ascii_whitespace(bytes, cursor);
                if bytes.get(cursor) != Some(&b':') {
                    index += 1;
                    continue;
                }
                cursor = skip_ascii_whitespace(bytes, cursor + 1);
                let quote = *bytes.get(cursor)?;
                if !matches!(quote, b'\'' | b'"') {
                    index += 1;
                    continue;
                }
                let end = skip_javascript_string(bytes, cursor);
                if end <= cursor + 1 || bytes.get(end - 1) != Some(&quote) {
                    return None;
                }
                return Some(argument[cursor + 1..end - 1].to_string());
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn match_identifier(bytes: &[u8], start: usize, expected: &[u8]) -> Option<usize> {
    let end = start.checked_add(expected.len())?;
    if bytes.get(start..end) != Some(expected)
        || start
            .checked_sub(1)
            .and_then(|index| bytes.get(index))
            .is_some_and(|byte| is_javascript_identifier_continue(*byte))
        || bytes
            .get(end)
            .is_some_and(|byte| is_javascript_identifier_continue(*byte))
    {
        return None;
    }
    Some(end)
}

fn is_javascript_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

fn skip_ascii_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}

fn skip_javascript_string(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut index = start + 1;
    while let Some(byte) = bytes.get(index) {
        if *byte == b'\\' {
            index = (index + 2).min(bytes.len());
        } else if *byte == quote {
            return index + 1;
        } else {
            index += 1;
        }
    }
    bytes.len()
}

fn skip_line_comment(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(|byte| *byte != b'\n') {
        index += 1;
    }
    index
}

fn skip_block_comment(bytes: &[u8], mut index: usize) -> usize {
    while index + 1 < bytes.len() {
        if bytes[index] == b'*' && bytes[index + 1] == b'/' {
            return index + 2;
        }
        index += 1;
    }
    bytes.len()
}

/// Return the exclusive end of a balanced object beginning at `start`.
fn balanced_object_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                index = skip_javascript_string(bytes, index);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = skip_line_comment(bytes, index + 2);
                continue;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index + 2);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn named_tool_event(name: &str, input: Option<&Value>, ts: Option<i64>) -> NormalizedEvent {
    let mut event = NormalizedEvent::new(Role::Assistant);
    event.ts_ms = ts;
    event.tools.push(tool_call_from_input(name, input));
    event
}

fn tool_output_event(_payload: &Map<String, Value>, ts: Option<i64>) -> NormalizedEvent {
    let mut ev = NormalizedEvent::new(Role::Tool);
    ev.ts_ms = ts;
    ev
}

fn token_count_event(payload: &Map<String, Value>, ts: Option<i64>) -> Option<NormalizedEvent> {
    let info = payload.get("info").and_then(|i| i.as_object())?;
    // `last_token_usage` is the latest turn's usage; its `input_tokens` is the
    // full prompt that turn — i.e. the live context-window occupancy (it climbs
    // as history accumulates and drops on compaction). `total_token_usage` is the
    // lifetime cumulative and must not be used for occupancy. It grows beyond
    // the context window and would peg the chart at 100%.
    let usage_obj = info
        .get("last_token_usage")
        .or_else(|| info.get("total_token_usage"))
        .and_then(|u| u.as_object())?;
    let usage = codex_usage(usage_obj);
    if usage == Usage::default() {
        return None;
    }
    let mut ev = NormalizedEvent::new(Role::Assistant);
    ev.ts_ms = ts;
    ev.usage = usage;
    Some(ev)
}

/// The usage pair that identifies one `token_count` row.
type TokenCountKey = (Value, Value);

/// Codex writes the last `token_count` row again when it resumes a rollout.
/// The copy repeats both `last_token_usage` and `total_token_usage` exactly.
/// Both Codex parsing paths drop a row when this pair matches the prior row.
/// This prevents a resumed rollout from adding duplicate usage.
fn token_count_key(value: &Value) -> Option<TokenCountKey> {
    if value.get("type").and_then(Value::as_str) != Some("event_msg")
        || value.pointer("/payload/type").and_then(Value::as_str) != Some("token_count")
    {
        return None;
    }
    let info = value.pointer("/payload/info")?;
    Some((
        info.get("last_token_usage").cloned().unwrap_or(Value::Null),
        info.get("total_token_usage")
            .cloned()
            .unwrap_or(Value::Null),
    ))
}

/// Both Codex parsing paths treat compaction records within this window as one compaction.
/// Distinct compactions have intervening turns and much larger gaps.
const COMPACTION_DEDUPE_WINDOW_MS: i64 = 5_000;

/// Codex marks a completed compaction with a top-level `compacted` record or an `event_msg` `context_compacted` record.
/// Some rollouts write both forms for one compaction.
/// Both parsing paths deduplicate adjacent boundary events within `COMPACTION_DEDUPE_WINDOW_MS`.
fn compaction_event(ts: Option<i64>) -> NormalizedEvent {
    let mut ev = NormalizedEvent::new(Role::System);
    ev.ts_ms = ts;
    ev.is_compaction_boundary = true;
    ev
}

fn codex_usage(u: &Map<String, Value>) -> Usage {
    let get = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let input = get("input_tokens");
    let cached = get("cached_input_tokens");
    Usage {
        // Codex reports cached tokens *inside* `input_tokens`; split them out so
        // context occupancy (input + cache_read) isn't double-counted.
        input_tokens: input.saturating_sub(cached),
        output_tokens: get("output_tokens"),
        cache_read_tokens: cached,
        cache_creation_tokens: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::SessionCollector;

    /// One synthetic record per CODEX reader `is_inert_codex_record` mirrors,
    /// each asserted evidence-bearing under the strict (any-depth) scan, plus
    /// the exemptions the scan deliberately leaves inert. Mirrors
    /// `records.rs`'s `INERTNESS_MIRROR_CASES`.
    const CODEX_INERTNESS_MIRROR_CASES: &[(&str, bool)] = &[
        (r#"{"type":"new_event","payload":{"usage":{}}}"#, false),
        (
            r#"{"type":"new_event","payload":{"last_token_usage":{}}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"total_token_usage":{}}}"#,
            false,
        ),
        (r#"{"type":"new_event","payload":{"info":{}}}"#, false),
        (
            r#"{"type":"new_event","payload":{"input_tokens":1}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"output_tokens":1}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"cached_input_tokens":1}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"reasoning_output_tokens":1}}"#,
            false,
        ),
        (r#"{"type":"new_event","payload":{"model":"m"}}"#, false),
        (r#"{"type":"new_event","payload":{"effort":"high"}}"#, false),
        (
            r#"{"type":"new_event","payload":{"reasoning_effort":"high"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"service_tier":"priority"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"thread_settings":{}}}"#,
            false,
        ),
        (r#"{"type":"new_event","payload":{"role":"agent"}}"#, false),
        (
            r#"{"type":"new_event","payload":{"type":"message"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"reasoning"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"function_call_output"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"custom_tool_call_output"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"tool_search_output"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"mcp_tool_call_output"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"custom_tool_call"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"local_shell_call"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"tool_search_call"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"web_search_call"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"image_generation_call"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"compaction"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"context_compaction"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"context_compacted"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"type":"token_count"}}"#,
            false,
        ),
        (r#"{"type":"compacted"}"#, false),
        (
            r#"{"type":"new_event","payload":{"name":"Bash","arguments":{}}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"name":"Bash","input":{}}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"name":"Bash","call_id":"c1"}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"nested":{"usage":{}}}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"items":[{"name":"Bash","arguments":{}}]}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","timestamp":"2026-01-01T00:00:00Z"}"#,
            true,
        ),
        (
            r#"{"type":"new_event","payload":{"name":"","arguments":{}}}"#,
            true,
        ),
        (r#"{"type":"new_event","payload":{"call_id":"c1"}}"#, true),
        (
            r#"{"type":"new_event","payload":{"note":"free text"}}"#,
            true,
        ),
    ];

    #[test]
    fn every_key_record_to_event_reads_appears_in_the_codex_inertness_table() {
        for (record, expected_inert) in CODEX_INERTNESS_MIRROR_CASES {
            let value: Value = serde_json::from_str(record).unwrap();
            assert_eq!(
                is_inert_codex_record(&value, true),
                *expected_inert,
                "unexpected strict classification for {record}"
            );
        }
    }

    #[test]
    fn non_object_codex_records_fail_closed() {
        for record in [
            serde_json::json!([]),
            serde_json::json!(7),
            serde_json::json!("text"),
            Value::Null,
        ] {
            assert!(!is_inert_codex_record(&record, true));
            assert!(!is_inert_codex_record(&record, false));
        }
    }

    #[test]
    fn record_to_event_changes_require_an_inertness_review() {
        const EXPECTED_FINGERPRINT: u64 = 1_469_656_163_493_354_995;
        let source = include_str!("codex.rs").replace("\r\n", "\n");
        let start = source.find("fn observe_model_and_effort").unwrap();
        let end = source.find("\n#[cfg(test)]\nmod tests").unwrap();
        let fingerprint = source.as_bytes()[start..end]
            .iter()
            .fold(0xcbf29ce484222325_u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
            });

        assert_eq!(fingerprint, EXPECTED_FINGERPRINT);
    }

    /// An allowlisted `item_completed` whose own payload also carries a
    /// root-level `model` key is NOT inert: the light check still guards the
    /// root and root `payload` object, exactly as it guards a genuine
    /// `item_completed` echo's absence of one.
    #[test]
    fn item_completed_with_a_root_level_model_key_is_not_inert() {
        let record = serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "item_completed",
                "model": "m",
                "item": {"type": "UserMessage"}
            }
        });
        assert!(is_proven_echo(Some("event_msg"), Some("item_completed")));
        assert!(!is_inert_codex_record(&record, false));
    }

    /// A nested `name` + `arguments` pair inside `item_completed.item` is
    /// inert under the light check (the echoed item cannot carry evidence a
    /// reader reads — see `is_recognized_eventless`) but NOT inert under the
    /// strict check (a genuinely unrecognized record stays conservative at
    /// any depth).
    #[test]
    fn nested_arguments_inside_item_completed_item_is_inert_only_under_the_light_check() {
        let record = serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "item_completed",
                "item": {
                    "type": "McpToolCall",
                    "name": "search",
                    "arguments": {"query": "synthetic"}
                }
            }
        });
        assert!(is_inert_codex_record(&record, false));
        assert!(!is_inert_codex_record(&record, true));
    }

    /// `session_meta`, `turn_context`, and `thread_settings_applied` carry
    /// their own model / effort / service-tier fields by design — that data
    /// is read by `observe_model_and_effort` / `service_tier_speed` on every
    /// record before classification runs, so these names bypass the
    /// structural check entirely and never emit an observation or `Unusable`,
    /// exactly as before this change.
    #[test]
    fn old_allowlisted_names_bypass_the_structural_check_even_with_model_or_service_tier() {
        for (record_type, payload_type) in [
            (Some("session_meta"), None),
            (Some("turn_context"), None),
            (Some("world_state"), None),
            (Some("event_msg"), Some("thread_settings_applied")),
        ] {
            assert!(is_recognized_eventless(record_type, payload_type));
            assert!(!is_proven_echo(record_type, payload_type));
        }

        let jsonl = concat!(
            r#"{"timestamp":"2026-08-10T10:00:00Z","type":"turn_context","payload":{"model":"gpt-test","effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-10T10:00:01Z","type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"service_tier":"priority"}}}"#,
            "\n",
        );
        let input = SessionInput {
            agent: "codex".to_string(),
            session_id: "old-allowlist-with-signal".to_string(),
            source: RawSource::Jsonl(jsonl.to_string()),
        };
        let mut sink = ObservationCapturingSink::default();

        CodexAdapter
            .visit(&input, &mut sink)
            .expect("visit old-allowlist-with-signal session");

        assert!(
            sink.observations.is_empty(),
            "unexpected observations: {:?}",
            sink.observations
        );
    }

    #[test]
    fn unresolved_fork_rows_stop_accumulating_at_the_cap() {
        let mut state = CodexStreamState::default();
        let mut sink = SessionCollector::new("codex", "large-unresolved-fork");
        state.observe(
            serde_json::json!({
                "type": "session_meta",
                "payload": {
                    "thread_source": "subagent",
                    "agent_path": "agent-a"
                }
            }),
            100,
            &mut sink,
        );

        for index in 0..=MAX_PENDING_FORK_ROWS {
            state.observe(
                serde_json::json!({"type": "world_state", "payload": {"index": index}}),
                100,
                &mut sink,
            );
            assert!(state.pending_rows.len() <= MAX_PENDING_FORK_ROWS);
        }

        assert!(state.fork_attribution_incomplete);
        assert!(state.pending_rows.is_empty());
        assert_eq!(state.pending_bytes, 0);
        let summary = state.finish(&mut sink);
        sink.finish(summary);
        assert_eq!(
            sink.partial_reasons(),
            &std::collections::BTreeSet::from([PartialReason::AttributionIncomplete])
        );
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
    fn content_capture_maps_message_reasoning_tool_call_and_output() {
        let message_record = serde_json::json!({
            "timestamp": "2026-08-01T10:00:00Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "hello there"}]
            }
        })
        .to_string();
        let reasoning_record = serde_json::json!({
            "timestamp": "2026-08-01T10:00:01Z",
            "type": "response_item",
            "payload": {
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "pondering"}]
            }
        })
        .to_string();
        let function_call_record = serde_json::json!({
            "timestamp": "2026-08-01T10:00:02Z",
            "type": "response_item",
            "payload": {"type": "function_call", "name": "bash", "arguments": "{\"command\":\"ls\"}"}
        })
        .to_string();
        let function_output_record = serde_json::json!({
            "timestamp": "2026-08-01T10:00:03Z",
            "type": "response_item",
            "payload": {"type": "function_call_output", "call_id": "c1", "output": "ok"}
        })
        .to_string();
        let jsonl = format!(
            "{message_record}\n{reasoning_record}\n{function_call_record}\n{function_output_record}\n"
        );
        let input = SessionInput {
            agent: "codex".to_string(),
            session_id: "content-session".to_string(),
            source: RawSource::Jsonl(jsonl),
        };
        let mut sink = ContentCapturingSink::default();

        CodexAdapter
            .visit(&input, &mut sink)
            .expect("visit content session");

        assert_eq!(sink.contents.len(), 4, "one TurnContent per turn");
        assert_eq!(sink.contents[0].parts[0].kind, ContentKind::AssistantText);
        assert_eq!(sink.contents[0].parts[0].text, "hello there");
        assert_eq!(sink.contents[1].parts[0].kind, ContentKind::Thinking);
        assert_eq!(sink.contents[1].parts[0].text, "pondering");
        assert_eq!(sink.contents[2].parts[0].kind, ContentKind::ToolInput);
        assert_eq!(sink.contents[2].parts[0].text, r#"{"command":"ls"}"#);
        assert_eq!(sink.contents[3].parts[0].kind, ContentKind::ToolResult);
        assert_eq!(sink.contents[3].parts[0].text, "ok");
    }

    fn thread_settings_applied(service_tier: &str) -> Value {
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:00Z",
            "type": "event_msg",
            "payload": {
                "type": "thread_settings_applied",
                "thread_settings": {"service_tier": service_tier}
            }
        })
    }

    #[test]
    fn service_tier_maps_priority_to_fast_and_default_to_standard() {
        assert_eq!(
            service_tier_speed(&thread_settings_applied("priority")).as_deref(),
            Some("fast")
        );
        assert_eq!(
            service_tier_speed(&thread_settings_applied("default")).as_deref(),
            Some("standard")
        );
    }

    #[test]
    fn service_tier_keeps_an_unreviewed_tier_as_its_own_label() {
        assert_eq!(
            service_tier_speed(&thread_settings_applied("economy")).as_deref(),
            Some("economy")
        );
    }

    #[test]
    fn service_tier_ignores_an_empty_or_missing_value() {
        assert_eq!(service_tier_speed(&thread_settings_applied("")), None);
        assert_eq!(
            service_tier_speed(&serde_json::json!({
                "type": "event_msg",
                "payload": {"type": "thread_settings_applied", "thread_settings": {}}
            })),
            None
        );
        assert_eq!(
            service_tier_speed(&serde_json::json!({
                "type": "event_msg",
                "payload": {"type": "token_count"}
            })),
            None
        );
    }

    #[test]
    fn an_event_that_already_carries_a_speed_keeps_it() {
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.speed = Some("preexisting".to_owned());

        apply_thread_speed(&mut event, &Some("fast".to_owned()));

        assert_eq!(event.speed.as_deref(), Some("preexisting"));
    }

    #[test]
    fn a_non_assistant_event_never_receives_the_thread_speed() {
        let mut event = NormalizedEvent::new(Role::Tool);

        apply_thread_speed(&mut event, &Some("fast".to_owned()));

        assert_eq!(event.speed, None);
    }

    /// A `thread_settings_applied` record in the copied parent-history prefix
    /// of a subagent rollout — the part `ForkOwnership::Pending` buffers and
    /// attributes to no one — must still set the speed the child's later,
    /// owned turns inherit. See `service_tier_speed`'s call in `process_value`.
    #[test]
    fn a_tier_set_in_the_replayed_parent_prefix_reaches_the_child_s_owned_turns() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-08-05T10:00:00Z","type":"session_meta","payload":{"id":"synthetic-child","thread_source":"subagent","agent_path":"worker","source":"cli"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-05T10:00:01Z","type":"turn_context","payload":{"model":"gpt-parent","effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-05T10:00:02Z","type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"service_tier":"priority"}}}"#,
            "\n",
            r#"{"timestamp":"2026-08-05T10:00:03Z","type":"event_msg","payload":{"type":"task_started"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-05T10:00:04Z","type":"response_item","payload":{"type":"agent_message","author":"parent","recipient":"worker","content":[{"type":"input_text","text":"Handle the synthetic task."}]}}"#,
            "\n",
            r#"{"timestamp":"2026-08-05T10:00:05Z","type":"turn_context","payload":{"model":"gpt-child","effort":"low"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-05T10:00:06Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":300,"cached_input_tokens":100,"output_tokens":40,"total_tokens":340},"total_token_usage":{"input_tokens":300,"cached_input_tokens":100,"output_tokens":40,"total_tokens":340},"model_context_window":100000}}}"#,
            "\n",
        );
        let input = SessionInput {
            agent: "codex".to_string(),
            session_id: "fork-speed".to_string(),
            source: RawSource::Jsonl(jsonl.to_string()),
        };
        let mut sink = SessionCollector::new("codex", "fork-speed");

        CodexAdapter
            .visit(&input, &mut sink)
            .expect("visit fork speed session");
        let session = sink.into_session().expect("fork speed session finishes");

        let owned_event = session
            .events
            .iter()
            .find(|event| event.model.as_deref() == Some("gpt-child"))
            .expect("the child's owned token_count turn is emitted");
        assert_eq!(owned_event.speed.as_deref(), Some("fast"));
    }

    /// Collects every `EvidenceObservation` a visit emits, in order.
    #[derive(Default)]
    struct ObservationCapturingSink {
        observations: Vec<EvidenceObservation>,
    }

    impl RecordSink for ObservationCapturingSink {
        fn record(&mut self, record: NormalizedRecord) {
            if let NormalizedRecord::Observation(observation) = record {
                self.observations.push(*observation);
            }
        }

        fn finish(&mut self, _summary: SessionSummary) {}
    }

    fn spawn_agent_observations(sink: &ObservationCapturingSink) -> Vec<&EvidenceObservation> {
        sink.observations
            .iter()
            .filter(|observation| matches!(observation, EvidenceObservation::SubagentSpawn { .. }))
            .collect()
    }

    #[test]
    fn a_spawn_agent_call_owned_by_the_parent_emits_a_spawn() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-08-06T10:00:00Z","type":"turn_context","payload":{"model":"gpt-parent","effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:00:01Z","type":"response_item","payload":{"type":"function_call","name":"spawn_agent","arguments":"{\"agent_type\":\"worker\"}","call_id":"call-1"}}"#,
            "\n",
        );
        let input = SessionInput {
            agent: "codex".to_string(),
            session_id: "spawn-owned".to_string(),
            source: RawSource::Jsonl(jsonl.to_string()),
        };
        let mut sink = ObservationCapturingSink::default();

        CodexAdapter
            .visit(&input, &mut sink)
            .expect("visit spawn-owned session");

        let spawns = spawn_agent_observations(&sink);
        assert_eq!(spawns.len(), 1);
        let EvidenceObservation::SubagentSpawn {
            parent_model,
            provenance,
            ..
        } = spawns[0]
        else {
            unreachable!("filtered to SubagentSpawn observations");
        };
        assert_eq!(parent_model.as_deref(), Some("gpt-parent"));
        assert_eq!(*provenance, RelationProvenance::SpawnAgentCall);
    }

    /// A `spawn_agent` call inside the copied parent-history prefix of a
    /// subagent rollout (`ForkOwnership::Pending` replay, before the owned
    /// usage boundary) is a spawn the parent's own rollout already reports.
    /// It must not also emit a spawn from the child's file.
    #[test]
    fn a_spawn_agent_call_in_the_replayed_parent_prefix_emits_no_spawn() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-08-06T10:00:00Z","type":"session_meta","payload":{"id":"synthetic-child","thread_source":"subagent","agent_path":"worker","source":"cli"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:00:01Z","type":"turn_context","payload":{"model":"gpt-parent","effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:00:02Z","type":"response_item","payload":{"type":"function_call","name":"spawn_agent","arguments":"{\"agent_type\":\"worker\"}","call_id":"call-1"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:00:03Z","type":"event_msg","payload":{"type":"task_started"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:00:04Z","type":"response_item","payload":{"type":"agent_message","author":"parent","recipient":"worker","content":[{"type":"input_text","text":"Handle the synthetic task."}]}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:00:05Z","type":"turn_context","payload":{"model":"gpt-child","effort":"low"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:00:06Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":300,"cached_input_tokens":100,"output_tokens":40,"total_tokens":340},"total_token_usage":{"input_tokens":300,"cached_input_tokens":100,"output_tokens":40,"total_tokens":340},"model_context_window":100000}}}"#,
            "\n",
        );
        let input = SessionInput {
            agent: "codex".to_string(),
            session_id: "spawn-replayed-prefix".to_string(),
            source: RawSource::Jsonl(jsonl.to_string()),
        };
        let mut sink = ObservationCapturingSink::default();

        CodexAdapter
            .visit(&input, &mut sink)
            .expect("visit spawn-replayed-prefix session");

        assert!(spawn_agent_observations(&sink).is_empty());
    }

    #[test]
    fn a_function_call_with_another_name_emits_no_spawn() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-08-06T10:00:00Z","type":"turn_context","payload":{"model":"gpt-parent","effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-06T10:00:01Z","type":"response_item","payload":{"type":"function_call","name":"bash","arguments":"{\"command\":\"ls\"}","call_id":"call-1"}}"#,
            "\n",
        );
        let input = SessionInput {
            agent: "codex".to_string(),
            session_id: "spawn-other-name".to_string(),
            source: RawSource::Jsonl(jsonl.to_string()),
        };
        let mut sink = ObservationCapturingSink::default();

        CodexAdapter
            .visit(&input, &mut sink)
            .expect("visit spawn-other-name session");

        assert!(spawn_agent_observations(&sink).is_empty());
    }
}
