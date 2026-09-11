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
//! - `token_usage_record` is a newer top-level usage envelope. Its
//!   `payload.usage` is the per-response usage. Its
//!   `payload.thread_token_usage` is cumulative. Some rollouts write it beside an equivalent
//!   `event_msg.token_count`; see `usage_record_key` for deduplication.
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
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::read_source;
use crate::analysis::framing::{BoundedJsonlReader, FramedRecord, PartialReason, RecordSkip};
use crate::analysis::initial_context::CodexContextAccumulator;
use crate::analysis::interface::{
    ContentKind, ContentPart, ContextSourceKind, ContextWindowSource, EvidenceObservation,
    NormalizedRecord, RawSource, RecordSink, RelationProvenance, ResumedVisit, SessionInput,
    SessionReader, SessionSummary, TurnContent, VisitOutcome,
};
use crate::analysis::model::{NormalizedEvent, NormalizedSession, Role, ToolCall, Usage};
use crate::analysis::records::{
    compact_json_text, concatenated_text, extract_content_parts_from_container, parse_ts,
    tool_call_from_input,
};
use crate::analysis::resume::{AdapterResume, StreamSnapshot};
use crate::analysis::source_validity::{AppendOnlyGuarantee, PinnedSource, SourceClaim};

const MAX_PENDING_FORK_ROWS: usize = 256;
const MAX_PENDING_FORK_BYTES: usize = 1024 * 1024;

pub struct CodexSessionReader;

impl SessionReader for CodexSessionReader {
    fn agent(&self) -> &'static str {
        "codex"
    }

    fn capabilities(&self, _source: &RawSource) -> crate::analysis::SourceCapabilities {
        crate::analysis::SourceCapabilities::codex()
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        let content = read_source(&input.source)
            .with_context(|| format!("reading codex session {}", input.session_id))?;
        let (events, context_window, model, cache_write_tokens_available) = parse_codex(&content);
        let context_window_source = if context_window.is_some() {
            ContextWindowSource::Reported
        } else {
            ContextWindowSource::Inferred
        };
        Ok(NormalizedSession {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            events,
            cache_write_tokens_available,
            context_window,
            context_window_source,
            model,
        })
    }

    fn visit(
        &self,
        input: &SessionInput,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        (|| -> anyhow::Result<VisitOutcome> {
            let state = match &input.source {
                RawSource::File(path) => self.visit_reader(
                    BufReader::new(File::open(path)?),
                    &|| false,
                    sink,
                    CodexStreamState::default(),
                )?,
                RawSource::Jsonl(content) => {
                    let suffix: &[u8] = if content.ends_with('\n') { b"" } else { b"\n" };
                    let source = Cursor::new(content.as_bytes()).chain(suffix);
                    self.visit_reader(
                        BufReader::new(source),
                        &|| false,
                        sink,
                        CodexStreamState::default(),
                    )?
                }
                RawSource::Sqlite(path) => {
                    anyhow::bail!(
                        "sqlite source must be handled by the sqlite adapter: {}",
                        path.display()
                    )
                }
            };
            let summary = state.finish(sink);
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
        CodexSessionReader::visit_claimed(self, input, claim, guarantee, cancel, sink)
    }

    fn visit_claimed_resumed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        resume: &StreamSnapshot,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<ResumedVisit> {
        CodexSessionReader::visit_claimed_resumed(self, input, claim, resume, cancel, sink)
    }

    fn empty_resume_state(&self) -> Option<crate::analysis::resume::AdapterSnapshot> {
        Some(CodexSessionReader::empty_adapter_snapshot())
    }
}

impl CodexSessionReader {
    /// A fresh [`CodexStreamState`], serialized. Mirrors
    /// [`crate::analysis::vendors::claude::ClaudeSessionReader::empty_adapter_snapshot`]:
    /// pairs with a [`StreamSnapshot`] whose [`ResumePoint`][rp] offset is
    /// zero to start the first resumable pass over a source.
    ///
    /// [rp]: crate::analysis::source_validity::ResumePoint
    pub fn empty_adapter_snapshot() -> crate::analysis::resume::AdapterSnapshot {
        crate::analysis::resume::AdapterSnapshot(
            postcard::to_allocvec(&CodexStreamState::default())
                .expect("a default CodexStreamState always encodes"),
        )
    }

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
            let state = self.visit_reader(
                BufReader::new(pinned.reader(limit)),
                cancel,
                sink,
                CodexStreamState::default(),
            )?;
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
            let summary = state.finish(sink);
            sink.finish(summary);
            Ok(outcome)
        })()
        .with_context(|| format!("reading claimed Codex session {}", input.session_id))
    }

    /// Streams a file from a verified [`StreamSnapshot`], restoring
    /// [`CodexStreamState`] from `resume.adapter` and reading only the bytes
    /// past `resume.resume.offset`. Mirrors
    /// [`crate::analysis::vendors::claude::ClaudeSessionReader::visit_claimed_resumed`]
    /// exactly; see its doc comment for the full read/recheck/snapshot shape.
    ///
    /// "Unsettled" rule: a fork sub-agent rollout's ownership can still be
    /// [`ForkOwnership::Pending`] at end of stream — the rows in
    /// `pending_rows` have no proven owner yet. [`CodexStreamState::finish`]
    /// flushes them as owned so a settled read still publishes every row,
    /// but a later full pass could resolve the same rows differently (an
    /// `agent_message` addressed to the child might still arrive). A
    /// snapshot taken at that point would not reproduce a full pass, so
    /// this method reports `resume: None` whenever ownership is still
    /// `Pending` right before `finish` — even though `outcome` is
    /// [`VisitOutcome::AcceptedFull`] and the sink still finishes normally.
    /// The next change to this source costs one full pass instead of a
    /// resume.
    pub fn visit_claimed_resumed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        resume: &StreamSnapshot,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<ResumedVisit> {
        (|| -> anyhow::Result<ResumedVisit> {
            anyhow::ensure!(
                resume.is_current(),
                "snapshot revision {} is not current",
                resume.revision
            );
            let RawSource::File(path) = &input.source else {
                anyhow::bail!("a claimed Codex source must be a file");
            };
            let mut pinned = match PinnedSource::open_resumed(path, claim.clone(), &resume.resume)?
            {
                Ok(pinned) => pinned,
                Err(reason) => {
                    return Ok(ResumedVisit {
                        outcome: VisitOutcome::SourceChanged(reason),
                        resume: None,
                    });
                }
            };
            let initial_state: CodexStreamState = postcard::from_bytes(&resume.adapter.0)
                .context("decoding Codex adapter snapshot")?;
            let state = self.visit_reader(
                BufReader::new(pinned.reader_from(resume.resume.offset, u64::MAX)),
                cancel,
                sink,
                initial_state,
            )?;
            let outcome = match pinned.recheck_full()? {
                Some(reason) => VisitOutcome::SourceChanged(reason),
                None => VisitOutcome::AcceptedFull,
            };
            if matches!(outcome, VisitOutcome::SourceChanged(_)) {
                return Ok(ResumedVisit {
                    outcome,
                    resume: None,
                });
            }
            // See the "unsettled" rule above: pending fork ownership is not
            // a safe resume point, even though this read still finishes
            // the sink. The `pending_rows` check is defensive: ownership
            // stays `Pending` for as long as any row is buffered, so the
            // two conditions coincide today, but a resume snapshot must
            // never depend on that coincidence holding.
            let pending =
                state.ownership == ForkOwnership::Pending || !state.pending_rows.is_empty();
            let settled_resume = if pending {
                None
            } else {
                let adapter =
                    postcard::to_allocvec(&state).context("encoding Codex adapter snapshot")?;
                let point = pinned.resume_point()?;
                Some(AdapterResume {
                    point,
                    adapter: crate::analysis::resume::AdapterSnapshot(adapter),
                })
            };
            let summary = state.finish(sink);
            sink.finish(summary);
            Ok(ResumedVisit {
                outcome,
                resume: settled_resume,
            })
        })()
        .with_context(|| format!("reading resumed Codex session {}", input.session_id))
    }

    /// Streams `reader` starting from `state`, so a resumed pass can carry
    /// forward the state a prior pass left off with. A first pass starts
    /// from `CodexStreamState::default()`. Returns the state at the end of
    /// the stream, not yet reduced to a [`SessionSummary`]: the caller
    /// decides whether to snapshot it before calling
    /// [`CodexStreamState::finish`].
    fn visit_reader(
        &self,
        reader: impl BufRead,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
        mut state: CodexStreamState,
    ) -> anyhow::Result<CodexStreamState> {
        let mut reader = BoundedJsonlReader::new(reader);

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

        Ok(state)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
enum ForkOwnership {
    #[default]
    TopLevel,
    Pending,
    Owned,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
struct CodexStreamState {
    ownership: ForkOwnership,
    agent_path: Option<String>,
    /// See [`json_text_codec`]: `postcard` cannot decode a `serde_json::Value`
    /// directly, so this field's wire form is JSON text. Always empty at the
    /// point [`CodexSessionReader::visit_claimed_resumed`] encodes a snapshot (see
    /// its "unsettled" rule), but the codec must not depend on that: it round
    /// trips a populated buffer too.
    #[serde(with = "json_text_codec")]
    pending_rows: Vec<Value>,
    pending_bytes: usize,
    pending_owned_start: Option<usize>,
    fork_attribution_incomplete: bool,
    /// See [`json_text_codec`].
    #[serde(with = "json_text_codec")]
    previous_usage_key: Option<UsageRecordSeen>,
    /// See [`json_text_codec`].
    #[serde(with = "json_text_codec")]
    recent_cross_format_usage: Option<UsageRecordSeen>,
    previous_event_was_boundary: bool,
    previous_boundary_ts: Option<i64>,
    context_window: Option<u64>,
    model: Option<String>,
    current_model: Option<String>,
    current_provider: Option<String>,
    current_thinking_mode: Option<String>,
    current_speed: Option<String>,
    started_at_ms: Option<i64>,
    owned_usage_seen: bool,
    effort_seen: bool,
    /// Sticky once true: an owned usage object has carried a
    /// [`CACHE_WRITE_ALIAS_KEYS`] key. See `usage_carries_cache_write`.
    cache_write_tokens_available: bool,
    context: CodexContextAccumulator,
}

/// Snapshot codec for a `CodexStreamState` field whose type carries
/// `serde_json::Value` data at any depth (`pending_rows`,
/// `previous_usage_key`, `recent_cross_format_usage`).
///
/// `serde_json::Value`'s `Deserialize` impl calls `deserialize_any`, a
/// self-describing lookahead `postcard` explicitly does not implement (its
/// wire format carries no type tags). Encoding a `Value` works either way,
/// but decoding one back only works through a self-describing format such
/// as JSON. This module stores the whole field as JSON text on the wire
/// instead, parsed back through `serde_json`'s own self-describing decoder —
/// generic over any `T`, so one codec covers every such field.
mod json_text_codec {
    use serde::de::DeserializeOwned;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<T, S>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        let text = serde_json::to_string(value).map_err(serde::ser::Error::custom)?;
        text.serialize(serializer)
    }

    pub(super) fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
    where
        T: DeserializeOwned,
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        serde_json::from_str(&text).map_err(serde::de::Error::custom)
    }
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
        observe_resource_evidence(&value, usage_is_owned, sink);
        let record_type = value.get("type").and_then(Value::as_str);
        let payload_type = value.pointer("/payload/type").and_then(Value::as_str);
        let is_token_count = is_token_count_record(record_type, payload_type);
        let is_usage_record = is_token_count || record_type == Some("token_usage_record");
        // Child requests inherit the explicit controls from the parent history.
        self.observe_model_and_effort(&value, usage_is_owned);
        if usage_is_owned {
            if self.context_window.is_none() {
                self.context_window = value
                    .pointer("/payload/info/model_context_window")
                    .and_then(Value::as_u64)
                    .filter(|window| *window > 0);
            }
            if !self.cache_write_tokens_available {
                self.cache_write_tokens_available =
                    usage_record_usage_object(&value).is_some_and(usage_carries_cache_write);
            }
            if is_spawn_agent_call(record_type, payload_type, &value) {
                sink.record(NormalizedRecord::Observation(Box::new(
                    EvidenceObservation::SubagentSpawn {
                        ts_ms: value.get("timestamp").and_then(parse_ts),
                        parent_model: self.current_model.clone(),
                        parent_call_id: None,
                        child_model: None,
                        provenance: RelationProvenance::SpawnAgentCall,
                    },
                )));
            }
        }

        if is_usage_record {
            if usage_record_is_duplicate(
                &mut self.previous_usage_key,
                &mut self.recent_cross_format_usage,
                &value,
                usage_is_owned,
            ) {
                return;
            }
            if !usage_is_owned {
                return;
            }
        }

        if let Some(mut event) = record_to_event(&value) {
            event.provider = self.current_provider.clone();
            if self.current_provider.as_deref() == Some("openai") {
                event.api = Some("responses".to_owned());
            }
            if usage_is_owned {
                event.model = event.model.or_else(|| self.current_model.clone());
                event.thinking_mode = self.current_thinking_mode.clone();
                apply_thread_speed(&mut event, &self.current_speed);
            }
            if is_usage_record {
                self.owned_usage_seen = true;
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
            let allowlisted = is_recognized_eventless(record_type, payload_type)
                || (is_usage_record && is_usage_free_record(&value));
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

    fn observe_model_and_effort(&mut self, value: &Value, usage_is_owned: bool) {
        let record_type = value.get("type").and_then(Value::as_str);
        let payload_type = value.pointer("/payload/type").and_then(Value::as_str);
        let settings =
            record_type == Some("event_msg") && payload_type == Some("thread_settings_applied");
        if settings || matches!(record_type, Some("session_meta" | "turn_context")) {
            // A new request context separates equal usage totals from different requests.
            self.previous_usage_key = None;
            self.recent_cross_format_usage = None;
        }
        let provider = if record_type == Some("session_meta") {
            value.pointer("/payload/model_provider")
        } else if settings {
            value.pointer("/payload/thread_settings/model_provider_id")
        } else {
            None
        };
        if let Some(provider) = provider {
            // An invalid explicit provider must not select the catalog's default route.
            self.current_provider =
                Some(codex_control_value(provider).unwrap_or_else(|| "<unknown>".to_owned()));
        }
        if settings
            && value
                .pointer("/payload/thread_settings/service_tier")
                .is_some()
        {
            self.current_speed = service_tier_speed(value);
        }
        if !matches!(
            record_type,
            Some("session_meta" | "turn_context" | "token_usage_record" | "response_item")
        ) && !settings
            && !(record_type == Some("event_msg") && payload_type == Some("token_count"))
        {
            return;
        }
        if let Some(next_model) = [
            "/payload/model",
            "/payload/info/model",
            "/payload/turn_context/model",
            "/payload/thread_settings/model",
        ]
        .iter()
        .find_map(|pointer| value.pointer(pointer))
        {
            self.current_model = codex_control_value(next_model);
            if usage_is_owned && self.model.is_none() {
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
        .find_map(|pointer| value.pointer(pointer))
        {
            self.current_thinking_mode = codex_control_value(next_mode);
        }
        if usage_is_owned && self.current_thinking_mode.is_some() {
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
        let coverage_gaps =
            if self.fork_attribution_incomplete || (self.owned_usage_seen && !self.effort_seen) {
                vec![PartialReason::AttributionIncomplete]
            } else {
                Vec::new()
            };
        if let Some(version) = self.context.harness_version() {
            sink.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::HarnessVersion {
                    version: version.to_owned(),
                },
            )));
        }
        let (initial_context, skill_descriptions) = self.context.finish();
        let context_window_source = if self.context_window.is_some() {
            ContextWindowSource::Reported
        } else {
            ContextWindowSource::Inferred
        };
        SessionSummary {
            cache_write_tokens_available: self.cache_write_tokens_available,
            context_window: self.context_window,
            context_window_source,
            model: self.model,
            provider_hints: Vec::new(),
            started_at_ms: self.started_at_ms,
            coverage_gaps,
            late_tools: Vec::new(),
            initial_context,
            skill_descriptions,
        }
    }
}

fn observe_resource_evidence(value: &Value, owned: bool, sink: &mut dyn RecordSink) {
    if value.get("type").and_then(Value::as_str) != Some("response_item") {
        return;
    }
    let payload = &value["payload"];
    match payload["type"].as_str() {
        Some("tool_search_output") => {
            // The pinned client emits completed results with namespace tool definitions.
            let tools = payload["tools"]
                .as_array()
                .filter(|_| payload["status"] == "completed" && payload["execution"] == "client");
            let Some(tools) = tools else {
                sink.record(NormalizedRecord::Unusable(
                    PartialReason::AttributionIncomplete,
                ));
                return;
            };
            for namespace in tools {
                let Some(name) = namespace["name"].as_str() else {
                    sink.record(NormalizedRecord::Unusable(
                        PartialReason::AttributionIncomplete,
                    ));
                    continue;
                };
                if namespace["type"] != "namespace" {
                    sink.record(NormalizedRecord::Unusable(
                        PartialReason::AttributionIncomplete,
                    ));
                    continue;
                }
                let Some(server) = name.strip_prefix("mcp__") else {
                    continue;
                };
                let valid = resource_name(server)
                    && !server.contains("__")
                    && namespace["tools"].as_array().is_some_and(|tools| {
                        !tools.is_empty()
                            && tools.iter().all(|tool| {
                                tool["type"] == "function"
                                    && tool["name"].as_str().is_some_and(resource_name)
                                    && tool["parameters"].is_object()
                            })
                    });
                if !valid {
                    sink.record(NormalizedRecord::Unusable(
                        PartialReason::AttributionIncomplete,
                    ));
                    continue;
                }
                sink.record(NormalizedRecord::Observation(Box::new(
                    EvidenceObservation::ContextSource {
                        kind: ContextSourceKind::McpServer,
                        name: server.to_owned(),
                        description: None,
                    },
                )));
            }
        }
        Some("message") if payload["role"] == "user" => {
            let Some(content) = payload["content"].as_array() else {
                return;
            };
            for part in content {
                if part["type"] != "input_text" {
                    continue;
                }
                let Some(text) = part["text"].as_str() else {
                    continue;
                };
                if !text.starts_with("<skill>\n") {
                    continue;
                }
                if let Some(name) = selected_skill_name(text) {
                    sink.record(NormalizedRecord::Observation(Box::new(
                        EvidenceObservation::SkillInjection {
                            name: name.to_owned(),
                            invoked: owned,
                        },
                    )));
                } else {
                    sink.record(NormalizedRecord::Unusable(
                        PartialReason::AttributionIncomplete,
                    ));
                }
            }
        }
        _ => {}
    }
}

fn resource_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= crate::analysis::evidence::EVIDENCE_STRING_CAP
        && !name
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '<' | '>' | '/' | '\\'))
}

fn selected_skill_name(text: &str) -> Option<&str> {
    let body = text
        .strip_prefix("<skill>\n<name>")?
        .strip_suffix("\n</skill>")?;
    let (name, body) = body.split_once("</name>\n<path>")?;
    if !resource_name(name) {
        return None;
    }
    let (path, mut document) = body.split_once("</path>\n")?;
    if path.is_empty() || path.contains(['\n', '<', '>']) {
        return None;
    }
    if let Some(metadata) = document.strip_prefix("<resource_access>") {
        let (metadata, rest) = metadata.split_once("</resource_access>\n")?;
        let metadata: Value = serde_json::from_str(metadata).ok()?;
        if !metadata.is_object() {
            return None;
        }
        document = rest;
    }
    (!document.trim().is_empty() && !document.contains("</skill>")).then_some(name)
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
///
/// The ten `collab_agent_*` / `collab_waiting_*` / `collab_close_*` /
/// `collab_resume_*` payload types are a different family: a codex collab
/// (multi-agent) session logs one begin and one end record for each step of
/// an inter-agent call (spawn, interaction, wait, close, resume). Each field
/// repeats data the paired `spawn_agent` function call already carries. A
/// verified read of the public codex protocol source (`openai/codex`,
/// `EventMsg`'s ten `Collab*` variants) found no usage, token, or billing
/// field on any of the ten payload structs. `collab_agent_spawn_begin` and
/// `collab_agent_spawn_end` do carry `model` and `reasoning_effort`, but
/// these fields name the spawned agent's configuration, not billing
/// evidence, so this allowlist entry is a deliberate, verified override of
/// the scalar-key scan for those two names only.
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
        (
            Some("event_msg"),
            Some(
                "collab_agent_spawn_begin"
                    | "collab_agent_spawn_end"
                    | "collab_agent_interaction_begin"
                    | "collab_agent_interaction_end"
                    | "collab_waiting_begin"
                    | "collab_waiting_end"
                    | "collab_close_begin"
                    | "collab_close_end"
                    | "collab_resume_begin"
                    | "collab_resume_end"
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
    "turn_token_usage",
    "thread_token_usage",
    "info",
    "input_tokens",
    "output_tokens",
    "cached_input_tokens",
    "cache_read_input_tokens",
    "cache_read_tokens",
    "cache_write_input_tokens",
    "cache_write_tokens",
    "cache_creation_input_tokens",
    "cache_creation_tokens",
    "reasoning_output_tokens",
    "model",
    "model_provider",
    "model_provider_id",
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
    "token_usage_record",
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

/// Keep control values bounded without converting long identifiers into known catalog values.
fn codex_control_value(value: &Value) -> Option<String> {
    let value = value.as_str()?.trim();
    if value.is_empty() {
        return None;
    }
    if value.len() > crate::analysis::evidence::EVIDENCE_STRING_CAP {
        return Some("<unknown>".to_owned());
    }
    Some(value.to_owned())
}

fn service_tier_speed(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) != Some("event_msg")
        || value.pointer("/payload/type").and_then(Value::as_str) != Some("thread_settings_applied")
    {
        return None;
    }
    let tier = codex_control_value(value.pointer("/payload/thread_settings/service_tier")?)?;
    Some(match tier.as_str() {
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

fn parse_codex(content: &str) -> (Vec<NormalizedEvent>, Option<u64>, Option<String>, bool) {
    let mut events = Vec::new();
    // A forked rollout starts by replaying its parent's history. Keep those
    // records available to the desktop analysis view, but do not attribute
    // their already-billed token_count events to the child. The first task
    // addressed to the child's agent path marks the owned usage boundary.
    let owned_usage_start = codex_fork_owned_offset(content);
    // The model's context-window size, reported on each `token_count` event's
    // `info.model_context_window`. Constant per model; take the first seen.
    let mut context_window = None;
    let mut controls = CodexStreamState::default();
    // Dedupe state for compaction boundaries: some rollouts write a
    // `context_compacted` event_msg and a top-level `compacted` record
    // back-to-back for the same compaction (see `compaction_event`).
    let mut previous_event_was_boundary = false;
    let mut previous_boundary_ts: Option<i64> = None;
    // Sticky once true: an owned usage object has carried a
    // `CACHE_WRITE_ALIAS_KEYS` key.
    let mut cache_write_tokens_available = false;
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
            controls.observe_model_and_effort(&value, usage_is_owned);
            if usage_is_owned && context_window.is_none() {
                context_window = value
                    .pointer("/payload/info/model_context_window")
                    .and_then(Value::as_u64)
                    .filter(|&w| w > 0);
            }
            if usage_is_owned && !cache_write_tokens_available {
                cache_write_tokens_available =
                    usage_record_usage_object(&value).is_some_and(usage_carries_cache_write);
            }
            let record_type = value.get("type").and_then(Value::as_str);
            let payload_type = value.pointer("/payload/type").and_then(Value::as_str);
            let is_usage_record = is_token_count_record(record_type, payload_type)
                || record_type == Some("token_usage_record");
            let inherited_usage = !usage_is_owned && is_usage_record;
            if usage_record_is_duplicate(
                &mut controls.previous_usage_key,
                &mut controls.recent_cross_format_usage,
                &value,
                usage_is_owned,
            ) {
                continue;
            }
            if !inherited_usage && let Some(mut ev) = record_to_event(&value) {
                ev.provider = controls.current_provider.clone();
                if controls.current_provider.as_deref() == Some("openai") {
                    ev.api = Some("responses".to_owned());
                }
                if usage_is_owned {
                    ev.model = ev.model.or_else(|| controls.current_model.clone());
                    ev.thinking_mode = controls.current_thinking_mode.clone();
                    apply_thread_speed(&mut ev, &controls.current_speed);
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
    (
        events,
        context_window,
        controls.model,
        cache_write_tokens_available,
    )
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
        ("token_usage_record", _) => token_usage_record_event(payload, ts),
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
    let qualified = payload
        .get("namespace")
        .and_then(Value::as_str)
        .filter(|namespace| namespace.starts_with("mcp__") && resource_name(namespace))
        .map(|namespace| format!("{namespace}__{name}"));
    ev.tools.push(tool_call_from_input(
        qualified.as_deref().unwrap_or(name),
        input,
    ));
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

fn token_usage_record_event(
    payload: &Map<String, Value>,
    ts: Option<i64>,
) -> Option<NormalizedEvent> {
    let (usage_object, _, _) = token_usage_record_objects(payload)?;
    let usage = codex_usage(usage_object);
    if usage == Usage::default() {
        return None;
    }
    let mut event = NormalizedEvent::new(Role::Assistant);
    event.ts_ms = ts;
    event.usage = usage;
    Some(event)
}

/// The usage keys `codex_usage` reads — `input_tokens`, `output_tokens`,
/// every cache-read alias, and every cache-write alias
/// ([`CACHE_WRITE_ALIAS_KEYS`]) — plus the `total_tokens` sum Codex writes
/// beside them. `is_usage_free_record` uses this list to prove a usage
/// object carries only known, zero-valued keys.
const CODEX_USAGE_KEYS: &[&str] = &[
    "input_tokens",
    "cached_input_tokens",
    "cache_read_input_tokens",
    "cache_read_tokens",
    "cache_write_input_tokens",
    "cache_write_tokens",
    "cache_creation_input_tokens",
    "cache_creation_tokens",
    "output_tokens",
    "reasoning_output_tokens",
    "total_tokens",
];

/// The alias keys `codex_usage` reads for a cache-write token count.
/// Presence of any of these in a usage object — not just a nonzero value —
/// proves this session's Codex CLI reports cache writes; an old CLI omits
/// every one of them. See [`usage_carries_cache_write`].
const CACHE_WRITE_ALIAS_KEYS: &[&str] = &[
    "cache_write_input_tokens",
    "cache_write_tokens",
    "cache_creation_input_tokens",
    "cache_creation_tokens",
];

/// True when a usage object carries at least one [`CACHE_WRITE_ALIAS_KEYS`]
/// key, at any value. Used to set `cache_write_tokens_available`.
fn usage_carries_cache_write(usage: &Map<String, Value>) -> bool {
    CACHE_WRITE_ALIAS_KEYS
        .iter()
        .any(|key| usage.contains_key(*key))
}

/// Returns the per-response usage object from either Codex usage format.
fn usage_record_usage_object(value: &Value) -> Option<&Map<String, Value>> {
    let record_type = value.get("type").and_then(Value::as_str);
    let payload_type = value.pointer("/payload/type").and_then(Value::as_str);
    if record_type == Some("token_usage_record") {
        let payload = value.get("payload")?.as_object()?;
        return token_usage_record_objects(payload).map(|(usage, _, _)| usage);
    }
    if !is_token_count_record(record_type, payload_type) {
        return None;
    }
    let info = value.pointer("/payload/info")?.as_object()?;
    info.get("last_token_usage")
        .or_else(|| info.get("total_token_usage"))
        .and_then(Value::as_object)
}

/// Returns true when a known usage record carries no usage to count.
///
/// This reads the same usage object `token_count_event` reads:
/// `last_token_usage`, falling back to `total_token_usage` only when
/// `last_token_usage` is absent. `total_token_usage` is Codex's lifetime
/// cumulative; once a session has real history it is never all-zero, but it
/// is not what this heartbeat contributes, so a nonzero cumulative in the
/// object this record does not use never disqualifies the record.
///
/// Codex writes three usage-free shapes: `info: null` beside a
/// `rate_limits` object (a rate-limit heartbeat); a selected usage object
/// that holds only zero counts; and a selected usage object that holds zero
/// counts in every component key beside a nonzero derived `total_tokens`
/// (the component sum still reports zero usage). `token_count_event`
/// returns `None` for all three: for the first two, because no usage object
/// is read; for the third, because `codex_usage` reads only the zero-valued
/// components. `process_value` would otherwise treat the record as
/// unrecognized and fail closed on the `info` key. A usage object with a
/// key this adapter does not read is not usage-free: it may be a renamed
/// count, so it still fails closed.
fn is_usage_free_record(value: &Value) -> bool {
    let record_type = value.get("type").and_then(Value::as_str);
    let payload_type = value.pointer("/payload/type").and_then(Value::as_str);
    let usage = if record_type == Some("token_usage_record") {
        let Some(payload) = value.get("payload").and_then(Value::as_object) else {
            return false;
        };
        let Some((usage, _, _)) = token_usage_record_objects(payload) else {
            return false;
        };
        return usage
            .iter()
            .all(|(key, count)| key == "total_tokens" || count.as_u64() == Some(0));
    } else if is_token_count_record(record_type, payload_type) {
        let Some(info) = value.pointer("/payload/info") else {
            return true;
        };
        let Some(info) = info.as_object() else {
            return info.is_null();
        };
        let Some(usage) = info
            .get("last_token_usage")
            .or_else(|| info.get("total_token_usage"))
        else {
            return true;
        };
        usage
    } else {
        return false;
    };
    usage.as_object().is_some_and(|usage| {
        usage.iter().all(|(key, count)| {
            if !CODEX_USAGE_KEYS.contains(&key.as_str()) {
                return false;
            }
            // `total_tokens` is a derived sum: a nonzero value beside
            // all-zero components still reports zero usage.
            key == "total_tokens" || count.as_u64().is_some_and(|count| count == 0)
        })
    })
}

/// The per-response and cumulative usage that identify one usage row.
type UsageRecordKey = (Value, Value);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum UsageRecordFormat {
    LegacyTokenCount,
    TokenUsageRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct UsageRecordSeen {
    key: UsageRecordKey,
    format: UsageRecordFormat,
    identity: Option<Value>,
    owned: bool,
    ts_ms: Option<i64>,
}

type TokenUsageObjects<'a> = (
    &'a Map<String, Value>,
    &'a Map<String, Value>,
    &'a Map<String, Value>,
);

fn token_usage_record_objects(payload: &Map<String, Value>) -> Option<TokenUsageObjects<'_>> {
    let usage = payload.get("usage")?.as_object()?;
    let turn_usage = payload.get("turn_token_usage")?.as_object()?;
    let thread_usage = payload.get("thread_token_usage")?.as_object()?;
    [usage, turn_usage, thread_usage]
        .iter()
        .all(|usage| {
            usage.iter().all(|(key, count)| {
                CODEX_USAGE_KEYS.contains(&key.as_str()) && count.as_u64().is_some()
            })
        })
        .then_some((usage, turn_usage, thread_usage))
}

/// Returns a common deduplication key for both Codex usage formats.
///
/// Codex can write equivalent `token_usage_record` and `token_count` rows.
/// The first value is per-response usage; the second is cumulative usage.
/// The dedupe policy compares these values with format and request identity.
fn usage_record_key(value: &Value) -> Option<UsageRecordKey> {
    let record_type = value.get("type").and_then(Value::as_str);
    let payload_type = value.pointer("/payload/type").and_then(Value::as_str);
    if record_type == Some("token_usage_record") {
        let payload = value.get("payload")?.as_object()?;
        let (usage, _, thread_usage) = token_usage_record_objects(payload)?;
        return Some((
            Value::Object(usage.clone()),
            Value::Object(thread_usage.clone()),
        ));
    }
    if !is_token_count_record(record_type, payload_type) {
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

fn usage_record_format(value: &Value) -> Option<UsageRecordFormat> {
    let record_type = value.get("type").and_then(Value::as_str);
    let payload_type = value.pointer("/payload/type").and_then(Value::as_str);
    if record_type == Some("token_usage_record") {
        Some(UsageRecordFormat::TokenUsageRecord)
    } else if is_token_count_record(record_type, payload_type) {
        Some(UsageRecordFormat::LegacyTokenCount)
    } else {
        None
    }
}

/// A cross-format match needs a timestamp because equal per-response usage
/// can belong to distinct requests. Keep the window short and retain one candidate.
const CROSS_FORMAT_USAGE_DEDUPE_WINDOW_MS: u64 = 5_000;

fn usage_record_is_duplicate(
    previous: &mut Option<UsageRecordSeen>,
    recent: &mut Option<UsageRecordSeen>,
    value: &Value,
    owned: bool,
) -> bool {
    let Some(key) = usage_record_key(value) else {
        return false;
    };
    let Some(format) = usage_record_format(value) else {
        return false;
    };
    let current = UsageRecordSeen {
        key,
        format,
        identity: usage_record_identity(value),
        owned,
        ts_ms: value.get("timestamp").and_then(parse_ts),
    };

    let same_format_repeat = previous.as_ref().is_some_and(|prior| {
        prior.owned == current.owned
            && prior.format == current.format
            && prior.key == current.key
            && prior.identity == current.identity
    });
    let exact_cross_format_repeat = previous.as_ref().is_some_and(|prior| {
        prior.owned == current.owned
            && prior.format != current.format
            && prior.key == current.key
            && prior
                .ts_ms
                .zip(current.ts_ms)
                .is_some_and(|(prior, current)| {
                    prior.abs_diff(current) <= CROSS_FORMAT_USAGE_DEDUPE_WINDOW_MS
                })
    });

    let mismatched_cross_format_repeat = !same_format_repeat
        && !exact_cross_format_repeat
        && recent.as_ref().is_some_and(|prior| {
            prior.owned == current.owned
                && prior.format != current.format
                && prior.key.0 == current.key.0
                && prior.key.1 != current.key.1
                && prior
                    .ts_ms
                    .zip(current.ts_ms)
                    .is_some_and(|(prior, current)| {
                        prior.abs_diff(current) <= CROSS_FORMAT_USAGE_DEDUPE_WINDOW_MS
                    })
        });

    let duplicate =
        same_format_repeat || exact_cross_format_repeat || mismatched_cross_format_repeat;
    if exact_cross_format_repeat || mismatched_cross_format_repeat {
        *recent = None;
    }
    *previous = Some(current.clone());
    if !duplicate {
        *recent = Some(current);
    }
    duplicate
}

fn usage_record_identity(value: &Value) -> Option<Value> {
    if usage_record_format(value) != Some(UsageRecordFormat::TokenUsageRecord) {
        return None;
    }
    let payload = value.get("payload")?.as_object()?;
    let mut identity = Map::new();
    for key in [
        "thread_id",
        "turn_id",
        "session_id",
        "root_turn_id",
        "response_id",
    ] {
        if let Some(value) = payload.get(key) {
            identity.insert(key.to_owned(), value.clone());
        }
    }
    (!identity.is_empty()).then_some(Value::Object(identity))
}

fn is_token_count_record(record_type: Option<&str>, payload_type: Option<&str>) -> bool {
    record_type == Some("event_msg") && payload_type == Some("token_count")
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

/// Splits a Codex `last_token_usage` / `total_token_usage` object into
/// [`Usage`]'s buckets. `cache_read_tokens` is the largest cache-read alias.
/// `cache_creation_tokens` is the largest cache-write alias from
/// [`CACHE_WRITE_ALIAS_KEYS`]. `input_tokens` subtracts both values and
/// floors the result at zero.
/// Codex reports both cache classes *inside* `input_tokens`; leaving them
/// in would double-count context occupancy ([`Usage::context_tokens`]).
fn codex_usage(u: &Map<String, Value>) -> Usage {
    let get = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let cache_read_tokens = get("cached_input_tokens")
        .max(get("cache_read_input_tokens"))
        .max(get("cache_read_tokens"));
    let cache_creation_tokens = CACHE_WRITE_ALIAS_KEYS
        .iter()
        .map(|key| get(key))
        .max()
        .unwrap_or(0);
    Usage {
        input_tokens: get("input_tokens")
            .saturating_sub(cache_read_tokens)
            .saturating_sub(cache_creation_tokens),
        output_tokens: get("output_tokens"),
        cache_read_tokens,
        cache_creation_tokens,
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
        (
            r#"{"type":"new_event","payload":{"turn_token_usage":{}}}"#,
            false,
        ),
        (
            r#"{"type":"new_event","payload":{"thread_token_usage":{}}}"#,
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
        (r#"{"type":"token_usage_record"}"#, false),
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
        for key in ["model_provider", "model_provider_id"] {
            let value = serde_json::json!({"type":"unknown", "payload":{key:"custom"}});
            assert!(!is_inert_codex_record(&value, true));
            assert!(!is_inert_codex_record(&value, false));
        }
    }

    #[test]
    fn retained_request_controls_are_bounded_and_round_trip() {
        let mut state = CodexStreamState::default();
        let long_value = "x".repeat(crate::analysis::evidence::EVIDENCE_STRING_CAP + 1);
        for _ in 0..1000 {
            state.observe_model_and_effort(&serde_json::json!({
                "type":"event_msg", "payload":{"type":"thread_settings_applied", "thread_settings":{
                    "model_provider_id":long_value, "model":long_value,
                    "reasoning_effort":long_value, "service_tier":long_value
                }}
            }), true);
        }
        for value in [
            &state.current_provider,
            &state.current_model,
            &state.current_thinking_mode,
            &state.current_speed,
        ] {
            assert_eq!(value.as_deref(), Some("<unknown>"));
        }
        let snapshot = postcard::to_allocvec(&state).unwrap();
        assert!(snapshot.len() < 1024);
        let restored: CodexStreamState = postcard::from_bytes(&snapshot).unwrap();
        assert_eq!(restored.current_provider, state.current_provider);
        assert_eq!(restored.current_model, state.current_model);
        assert_eq!(restored.current_thinking_mode, state.current_thinking_mode);
        assert_eq!(restored.current_speed, state.current_speed);
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
        const EXPECTED_FINGERPRINT: u64 = 18_370_784_376_499_888_089;
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

    /// A `token_count` heartbeat with `info: null`, one whose usage holds
    /// only zero counts, and one whose selected usage object holds zero
    /// counts in every component key beside a nonzero derived
    /// `total_tokens` carry nothing to count: all three are usage-free, so
    /// `process_value` treats them as recognized-eventless. The third case
    /// mirrors a real Codex 2026-08 heartbeat: `last_token_usage`'s own
    /// component keys are all zero, but `total_token_usage` — not read for
    /// this record — carries the session's real nonzero lifetime
    /// cumulative, and does not disqualify it.
    #[test]
    fn token_count_without_usage_is_usage_free() {
        let heartbeat = serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "token_count", "info": null, "rate_limits": {"primary": {}}}
        });
        let zero = serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "token_count", "rate_limits": {}, "info": {
                "last_token_usage": {"input_tokens": 0, "cached_input_tokens": 0, "output_tokens": 0, "reasoning_output_tokens": 0, "total_tokens": 0},
                "total_token_usage": {"input_tokens": 0, "cached_input_tokens": 0, "output_tokens": 0, "reasoning_output_tokens": 0, "total_tokens": 0},
                "model_context_window": 100000
            }}
        });
        let zero_components_nonzero_total = serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "token_count", "rate_limits": {}, "info": {
                "model_context_window": 272000,
                "last_token_usage": {
                    "input_tokens": 0, "cached_input_tokens": 0, "cache_write_input_tokens": 0,
                    "output_tokens": 0, "reasoning_output_tokens": 0, "total_tokens": 13989
                },
                "total_token_usage": {
                    "input_tokens": 9000000, "cached_input_tokens": 8000000, "cache_write_input_tokens": 0,
                    "output_tokens": 400000, "reasoning_output_tokens": 100000, "total_tokens": 9500000
                }
            }}
        });
        assert!(is_usage_free_record(&heartbeat));
        assert!(is_usage_free_record(&zero));
        assert!(is_usage_free_record(&zero_components_nonzero_total));
        // The strict check still rejects all three on the `info` key. Only
        // the usage-free rule lets them through.
        assert!(!is_inert_codex_record(&heartbeat, true));
        assert!(!is_inert_codex_record(&zero, true));
        assert!(!is_inert_codex_record(&zero_components_nonzero_total, true));
    }

    /// A `token_count` whose usage object names a key this adapter does not
    /// read, or a non-zero count, is not usage-free. This holds even when
    /// the only non-zero count sits in `cache_write_input_tokens` — a known
    /// key `codex_usage` now reads into `cache_creation_tokens`, so this
    /// case reports real usage rather than failing closed (see
    /// `token_count_with_only_a_cache_write_reads_as_real_usage`) — and even
    /// when the unknown key sits beside an otherwise all-zero usage object.
    #[test]
    fn token_count_with_unread_or_non_zero_usage_is_not_usage_free() {
        let renamed = serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {
                "last_token_usage": {"prompt_tokens": 0, "output_tokens": 0}
            }}
        });
        let counted = serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {
                "last_token_usage": {"input_tokens": 12, "output_tokens": 0}
            }}
        });
        let not_an_object = serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {"last_token_usage": 7}}
        });
        let non_zero_cache_write = serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {
                "last_token_usage": {
                    "input_tokens": 0, "cached_input_tokens": 0, "cache_write_input_tokens": 5,
                    "output_tokens": 0, "reasoning_output_tokens": 0, "total_tokens": 0
                }
            }}
        });
        let unknown_key = serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {
                "last_token_usage": {
                    "input_tokens": 0, "cached_input_tokens": 0, "output_tokens": 0,
                    "reasoning_output_tokens": 0, "total_tokens": 0, "speculation_tokens": 5
                }
            }}
        });
        assert!(!is_usage_free_record(&renamed));
        assert!(!is_usage_free_record(&counted));
        assert!(!is_usage_free_record(&not_an_object));
        assert!(!is_usage_free_record(&non_zero_cache_write));
        assert!(!is_usage_free_record(&unknown_key));
    }

    /// A heartbeat whose only non-zero component is `cache_write_input_tokens`
    /// now reads as real usage instead of failing closed. Before `codex_usage`
    /// read the field, this record produced `Usage::default()` from
    /// `token_count_event`, so `record_to_event` returned `None`,
    /// `is_usage_free_record` still reported it as not usage-free (the
    /// field carried a non-zero count), and `process_value` fell through to
    /// the unrecognized-record path and marked the record `Unusable`.
    #[test]
    fn token_count_with_only_a_cache_write_reads_as_real_usage() {
        let record = serde_json::json!({
            "timestamp": "2026-08-28T19:52:18.490Z",
            "type": "event_msg",
            "payload": {"type": "token_count", "info": {
                "last_token_usage": {
                    "input_tokens": 0, "cached_input_tokens": 0, "cache_write_input_tokens": 9000,
                    "output_tokens": 0, "reasoning_output_tokens": 0, "total_tokens": 9000
                }
            }}
        });
        let event = record_to_event(&record).expect("a nonzero cache-write count is real usage");
        assert_eq!(event.usage.cache_creation_tokens, 9000);
        assert_eq!(event.usage.input_tokens, 0);
        assert_ne!(event.usage, Usage::default());
    }

    #[test]
    fn codex_usage_splits_cache_read_and_cache_write_out_of_input() {
        let usage_obj = serde_json::json!({
            "input_tokens": 52000, "cached_input_tokens": 40000,
            "cache_write_input_tokens": 9000, "output_tokens": 1200,
            "reasoning_output_tokens": 300, "total_tokens": 53200
        });
        let usage = codex_usage(usage_obj.as_object().unwrap());
        assert_eq!(usage.input_tokens, 3000);
        assert_eq!(usage.cache_read_tokens, 40000);
        assert_eq!(usage.cache_creation_tokens, 9000);
        assert_eq!(usage.output_tokens, 1200);
    }

    /// `Usage::context_tokens()` (input + cache_read + cache_creation) must
    /// not move for the same record between the old split and the new one.
    /// Before this change, `codex_usage` did not read
    /// `cache_write_input_tokens`, so context occupancy was `input_tokens`
    /// as Codex reports it (52000): cache reads split out, cache writes
    /// folded into `input_tokens`. The new split pulls cache writes into
    /// their own bucket, but the three buckets still sum to the same raw
    /// `input_tokens` figure.
    #[test]
    fn cache_write_split_preserves_context_occupancy() {
        let usage_obj = serde_json::json!({
            "input_tokens": 52000, "cached_input_tokens": 40000,
            "cache_write_input_tokens": 9000, "output_tokens": 1200
        });
        let usage = codex_usage(usage_obj.as_object().unwrap());
        assert_eq!(usage.context_tokens(), 52000);
    }

    fn collect_synthetic_codex(
        jsonl: &str,
    ) -> (crate::analysis::RecordCoverage, NormalizedSession) {
        let input = SessionInput {
            agent: "codex".to_owned(),
            session_id: "synthetic-token-usage".to_owned(),
            source: RawSource::Jsonl(jsonl.to_owned()),
            fork_parent_session_id: None,
        };
        let mut sink = SessionCollector::new("codex", "synthetic-token-usage");
        CodexSessionReader
            .visit(&input, &mut sink)
            .expect("visit synthetic token usage session");
        let coverage = sink.coverage();
        let session = sink.into_session().expect("the session finishes");
        (coverage, session)
    }

    #[test]
    fn matching_new_and_legacy_usage_records_count_once_in_both_paths() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-09-01T00:00:00Z","type":"session_meta","payload":{"model":"gpt-test","effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:00:01Z","ordinal":1,"type":"token_usage_record","payload":{"thread_id":"thread-test","turn_id":"turn-test","session_id":"session-test","root_turn_id":"root-test","response_id":"response-test","usage":{"input_tokens":120,"cached_input_tokens":20,"cache_write_input_tokens":10,"output_tokens":30,"reasoning_output_tokens":5,"total_tokens":150},"turn_token_usage":{"input_tokens":500,"cached_input_tokens":100,"cache_write_input_tokens":20,"output_tokens":60,"reasoning_output_tokens":10,"total_tokens":560},"thread_token_usage":{"input_tokens":900,"cached_input_tokens":300,"cache_write_input_tokens":40,"output_tokens":100,"reasoning_output_tokens":20,"total_tokens":1000}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:00:02Z","type":"response_item","payload":{"type":"local_shell_call","action":{"type":"exec","command":"true"}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":200000,"last_token_usage":{"input_tokens":120,"cached_input_tokens":20,"cache_write_input_tokens":10,"output_tokens":30,"reasoning_output_tokens":5,"total_tokens":150},"total_token_usage":{"input_tokens":900,"cached_input_tokens":300,"cache_write_input_tokens":40,"output_tokens":100,"reasoning_output_tokens":20,"total_tokens":1000}}}}"#,
            "\n",
        );

        let (coverage, streamed) = collect_synthetic_codex(jsonl);
        let (legacy_events, context_window, model, cache_write_available) = parse_codex(jsonl);

        assert_eq!(coverage, crate::analysis::RecordCoverage::Complete);
        assert_eq!(streamed.events, legacy_events);
        assert_eq!(streamed.events.len(), 2);
        assert_eq!(streamed.events[0].usage.input_tokens, 90);
        assert_eq!(streamed.events[0].usage.cache_read_tokens, 20);
        assert_eq!(streamed.events[0].usage.cache_creation_tokens, 10);
        assert_eq!(streamed.events[0].usage.output_tokens, 30);
        assert!(
            streamed.events[1]
                .tools
                .iter()
                .any(|tool| tool.name == "local_shell")
        );
        assert_eq!(streamed.context_window, Some(200_000));
        assert_eq!(context_window, Some(200_000));
        assert_eq!(streamed.model.as_deref(), Some("gpt-test"));
        assert_eq!(model.as_deref(), Some("gpt-test"));
        assert!(streamed.cache_write_tokens_available);
        assert!(cache_write_available);
        assert_eq!(
            streamed.events[0].ts_ms,
            parse_ts(&serde_json::json!("2026-09-01T00:00:01Z"))
        );
    }

    #[test]
    fn mismatched_cumulative_usage_rows_deduplicate_by_per_response_usage() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-09-01T00:00:00Z","type":"session_meta","payload":{"model":"gpt-test","effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:00:01Z","type":"token_usage_record","payload":{"usage":{"input_tokens":60863,"cached_input_tokens":0,"output_tokens":107},"turn_token_usage":{"input_tokens":60863,"cached_input_tokens":0,"output_tokens":107},"thread_token_usage":{"input_tokens":60863,"cached_input_tokens":0,"output_tokens":107}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":200000,"last_token_usage":{"input_tokens":60863,"cached_input_tokens":0,"output_tokens":107},"total_token_usage":{"input_tokens":70000,"cached_input_tokens":50000,"output_tokens":200}}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:00:03Z","type":"token_usage_record","payload":{"usage":{"input_tokens":67617,"cached_input_tokens":60800,"output_tokens":126},"turn_token_usage":{"input_tokens":67617,"cached_input_tokens":60800,"output_tokens":126},"thread_token_usage":{"input_tokens":128480,"cached_input_tokens":60800,"output_tokens":233}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":200000,"last_token_usage":{"input_tokens":67617,"cached_input_tokens":60800,"output_tokens":126},"total_token_usage":{"input_tokens":130000,"cached_input_tokens":110000,"output_tokens":350}}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:00:05Z","type":"token_usage_record","payload":{"usage":{"input_tokens":67756,"cached_input_tokens":67584,"output_tokens":185},"turn_token_usage":{"input_tokens":67756,"cached_input_tokens":67584,"output_tokens":185},"thread_token_usage":{"input_tokens":196236,"cached_input_tokens":128384,"output_tokens":418}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:00:06Z","type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":200000,"last_token_usage":{"input_tokens":67756,"cached_input_tokens":67584,"output_tokens":185},"total_token_usage":{"input_tokens":200000,"cached_input_tokens":180000,"output_tokens":500}}}}"#,
            "\n",
        );

        let (coverage, streamed) = collect_synthetic_codex(jsonl);
        let (legacy_events, context_window, model, _) = parse_codex(jsonl);

        assert_eq!(coverage, crate::analysis::RecordCoverage::Complete);
        assert_eq!(streamed.events, legacy_events);
        assert_eq!(streamed.events.len(), 3);
        let usage = streamed
            .events
            .iter()
            .fold(Usage::default(), |total, event| {
                total.saturating_add(event.usage)
            });
        assert_eq!(usage.input_tokens, 67_852);
        assert_eq!(usage.cache_read_tokens, 128_384);
        assert_eq!(usage.output_tokens, 418);
        assert_eq!(
            streamed
                .events
                .iter()
                .map(|event| event.usage.context_tokens())
                .max(),
            Some(67_756)
        );
        assert_eq!(streamed.context_window, Some(200_000));
        assert_eq!(context_window, Some(200_000));
        assert_eq!(streamed.model.as_deref(), Some("gpt-test"));
        assert_eq!(model.as_deref(), Some("gpt-test"));
    }

    #[test]
    fn equal_usage_requests_with_distinct_response_ids_are_preserved() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-09-01T00:10:00Z","type":"session_meta","payload":{"effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:10:01Z","type":"token_usage_record","payload":{"response_id":"response-a","usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"turn_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"thread_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:10:02Z","type":"token_usage_record","payload":{"response_id":"response-b","usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"turn_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"thread_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5}}}"#,
            "\n",
        );

        let (_, streamed) = collect_synthetic_codex(jsonl);
        let (legacy_events, _, _, _) = parse_codex(jsonl);

        assert_eq!(streamed.events, legacy_events);
        assert_eq!(streamed.events.len(), 2);
        assert_eq!(streamed.events[0].usage, streamed.events[1].usage);
    }

    #[test]
    fn mismatched_cross_format_usage_outside_window_is_preserved() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-09-01T00:20:00Z","type":"session_meta","payload":{"effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:20:01Z","type":"token_usage_record","payload":{"usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"turn_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"thread_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:20:07Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"total_token_usage":{"input_tokens":300,"cached_input_tokens":200,"output_tokens":15}}}}"#,
            "\n",
        );

        let (_, streamed) = collect_synthetic_codex(jsonl);
        let (legacy_events, _, _, _) = parse_codex(jsonl);

        assert_eq!(streamed.events, legacy_events);
        assert_eq!(streamed.events.len(), 2);
    }

    #[test]
    fn adjacent_equal_usage_pairs_are_consumed_independently() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-09-01T00:30:00Z","type":"session_meta","payload":{"effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:30:01Z","type":"token_usage_record","payload":{"usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"turn_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"thread_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:30:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"total_token_usage":{"input_tokens":110,"cached_input_tokens":20,"output_tokens":6}}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:30:03Z","type":"token_usage_record","payload":{"usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"turn_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"thread_token_usage":{"input_tokens":200,"cached_input_tokens":40,"output_tokens":10}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:30:04Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"total_token_usage":{"input_tokens":210,"cached_input_tokens":40,"output_tokens":11}}}}"#,
            "\n",
        );

        let (_, streamed) = collect_synthetic_codex(jsonl);
        let (legacy_events, _, _, _) = parse_codex(jsonl);

        assert_eq!(streamed.events, legacy_events);
        assert_eq!(streamed.events.len(), 2);
    }

    #[test]
    fn a_distinct_usage_row_blocks_an_older_cross_format_match() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-09-01T00:40:00Z","type":"session_meta","payload":{"effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:40:01Z","type":"token_usage_record","payload":{"usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"turn_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"thread_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:40:02Z","type":"token_usage_record","payload":{"usage":{"input_tokens":200,"cached_input_tokens":30,"output_tokens":6},"turn_token_usage":{"input_tokens":200,"cached_input_tokens":30,"output_tokens":6},"thread_token_usage":{"input_tokens":300,"cached_input_tokens":50,"output_tokens":11}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:40:03Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"total_token_usage":{"input_tokens":400,"cached_input_tokens":80,"output_tokens":16}}}}"#,
            "\n",
        );

        let (_, streamed) = collect_synthetic_codex(jsonl);
        let (legacy_events, _, _, _) = parse_codex(jsonl);

        assert_eq!(streamed.events, legacy_events);
        assert_eq!(streamed.events.len(), 3);
    }

    #[test]
    fn reversed_cross_format_usage_rows_deduplicate() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-09-01T00:50:00Z","type":"session_meta","payload":{"effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:50:01Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"total_token_usage":{"input_tokens":300,"cached_input_tokens":200,"output_tokens":15}}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T00:50:02Z","type":"token_usage_record","payload":{"usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"turn_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"thread_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5}}}"#,
            "\n",
        );

        let (_, streamed) = collect_synthetic_codex(jsonl);
        let (legacy_events, _, _, _) = parse_codex(jsonl);

        assert_eq!(streamed.events, legacy_events);
        assert_eq!(streamed.events.len(), 1);
    }

    #[test]
    fn mismatched_cross_format_usage_without_timestamps_is_preserved() {
        let new_record = serde_json::json!({
            "type": "token_usage_record",
            "payload": {
                "usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 5},
                "turn_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 5},
                "thread_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 5}
            }
        });
        let legacy_record = serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 5},
                    "total_token_usage": {"input_tokens": 300, "cached_input_tokens": 200, "output_tokens": 15}
                }
            }
        });
        let mut previous = None;
        let mut recent = None;

        assert!(!usage_record_is_duplicate(
            &mut previous,
            &mut recent,
            &new_record,
            true
        ));
        assert!(!usage_record_is_duplicate(
            &mut previous,
            &mut recent,
            &legacy_record,
            true
        ));
    }

    #[test]
    fn a_standalone_new_usage_record_keeps_source_order_and_counts() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-09-01T01:00:00Z","type":"session_meta","payload":{"effort":"medium"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T01:00:01Z","ordinal":2,"type":"token_usage_record","payload":{"thread_id":"thread-only","turn_id":"turn-only","session_id":"session-only","root_turn_id":"root-only","response_id":"response-only","usage":{"input_tokens":80,"cached_input_tokens":30,"cache_write_input_tokens":0,"output_tokens":20,"reasoning_output_tokens":4,"total_tokens":100},"turn_token_usage":{"input_tokens":80,"cached_input_tokens":30,"cache_write_input_tokens":0,"output_tokens":20,"reasoning_output_tokens":4,"total_tokens":100},"thread_token_usage":{"input_tokens":80,"cached_input_tokens":30,"cache_write_input_tokens":0,"output_tokens":20,"reasoning_output_tokens":4,"total_tokens":100}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T01:00:02Z","type":"response_item","payload":{"type":"web_search_call"}}"#,
            "\n",
        );

        let (coverage, streamed) = collect_synthetic_codex(jsonl);
        let (legacy_events, context_window, _, cache_write_available) = parse_codex(jsonl);

        assert_eq!(coverage, crate::analysis::RecordCoverage::Complete);
        assert_eq!(streamed.events, legacy_events);
        assert_eq!(streamed.events.len(), 2);
        assert_eq!(streamed.events[0].usage.context_tokens(), 80);
        assert_eq!(streamed.events[0].usage.output_tokens, 20);
        assert_eq!(streamed.events[1].tools[0].name, "web_search");
        assert_eq!(context_window, None);
        assert!(streamed.cache_write_tokens_available);
        assert!(cache_write_available);
    }

    #[test]
    fn legacy_first_and_repeated_new_usage_records_count_once() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-09-01T02:00:00Z","type":"turn_context","payload":{"effort":"low"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T02:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":64000,"last_token_usage":{"input_tokens":40,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":45},"total_token_usage":{"input_tokens":40,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":45}}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T02:00:02Z","type":"token_usage_record","payload":{"usage":{"input_tokens":40,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":45},"turn_token_usage":{"input_tokens":40,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":45},"thread_token_usage":{"input_tokens":40,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":45}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T02:00:03Z","type":"token_usage_record","payload":{"usage":{"input_tokens":40,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":45},"turn_token_usage":{"input_tokens":40,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":45},"thread_token_usage":{"input_tokens":40,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":45}}}"#,
            "\n",
        );

        let (coverage, streamed) = collect_synthetic_codex(jsonl);
        let (legacy_events, _, _, _) = parse_codex(jsonl);

        assert_eq!(coverage, crate::analysis::RecordCoverage::Complete);
        assert_eq!(streamed.events, legacy_events);
        assert_eq!(streamed.events.len(), 1);
        assert_eq!(
            streamed.events[0].ts_ms,
            parse_ts(&serde_json::json!("2026-09-01T02:00:01Z"))
        );
    }

    #[test]
    fn malformed_or_unknown_new_usage_records_fail_closed() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-09-01T03:00:00Z","type":"token_usage_record","payload":{"usage":{"input_tokens":4,"future_tokens":2},"turn_token_usage":{"input_tokens":4},"thread_token_usage":{"input_tokens":4}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T03:00:01Z","type":"token_usage_record","payload":{"usage":{"input_tokens":4},"thread_token_usage":{"input_tokens":4}}}"#,
            "\n",
        );

        let (coverage, streamed) = collect_synthetic_codex(jsonl);
        let (legacy_events, _, _, _) = parse_codex(jsonl);

        assert_eq!(coverage, crate::analysis::RecordCoverage::Partial);
        assert_eq!(streamed.events, legacy_events);
        assert!(streamed.events.is_empty());
    }

    #[test]
    fn an_inherited_new_usage_record_does_not_hide_owned_legacy_usage() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-09-01T04:00:00Z","type":"session_meta","payload":{"thread_source":"subagent","agent_path":"worker"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T04:00:01Z","type":"turn_context","payload":{"effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T04:00:02Z","type":"token_usage_record","payload":{"usage":{"input_tokens":60,"cached_input_tokens":20,"cache_write_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":2,"total_tokens":70},"turn_token_usage":{"input_tokens":60,"cached_input_tokens":20,"cache_write_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":2,"total_tokens":70},"thread_token_usage":{"input_tokens":60,"cached_input_tokens":20,"cache_write_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":2,"total_tokens":70}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T04:00:03Z","type":"event_msg","payload":{"type":"task_started"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T04:00:03.500Z","type":"turn_context","payload":{"effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T04:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":128000,"last_token_usage":{"input_tokens":60,"cached_input_tokens":20,"cache_write_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":2,"total_tokens":70},"total_token_usage":{"input_tokens":60,"cached_input_tokens":20,"cache_write_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":2,"total_tokens":70}}}}"#,
            "\n",
            r#"{"timestamp":"2026-09-01T04:00:05Z","type":"response_item","payload":{"type":"agent_message","recipient":"worker","content":[]}}"#,
            "\n",
        );

        let (coverage, streamed) = collect_synthetic_codex(jsonl);
        let (legacy_events, context_window, _, _) = parse_codex(jsonl);

        assert_eq!(coverage, crate::analysis::RecordCoverage::Complete);
        assert_eq!(streamed.events, legacy_events);
        assert_eq!(streamed.events.len(), 1);
        assert_eq!(streamed.events[0].usage.output_tokens, 10);
        assert_eq!(streamed.context_window, Some(128_000));
        assert_eq!(context_window, Some(128_000));
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
            fork_parent_session_id: None,
        };
        let mut sink = ObservationCapturingSink::default();

        CodexSessionReader
            .visit(&input, &mut sink)
            .expect("visit old-allowlist-with-signal session");

        assert!(
            sink.observations.is_empty(),
            "unexpected observations: {:?}",
            sink.observations
        );
    }

    /// A synthetic Codex 2026-08 heartbeat: `last_token_usage` reports zero
    /// in every component key beside a nonzero derived `total_tokens`, and
    /// `total_token_usage` carries a large nonzero lifetime cumulative. A
    /// collector must not mark it unusable, and must not emit a
    /// non-inert `UnrecognizedType` observation for it, or the whole
    /// session degrades to `Partial` coverage over one inert heartbeat.
    #[test]
    fn a_zero_component_heartbeat_beside_a_nonzero_cumulative_is_inert() {
        let jsonl = concat!(
            r#"{"timestamp":"2026-08-28T19:52:18.490Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{},"info":{"model_context_window":272000,"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":13989},"total_token_usage":{"input_tokens":9000000,"cached_input_tokens":8000000,"cache_write_input_tokens":0,"output_tokens":400000,"reasoning_output_tokens":100000,"total_tokens":9500000}}}}"#,
            "\n",
        );
        let input = SessionInput {
            agent: "codex".to_string(),
            session_id: "zero-component-heartbeat".to_string(),
            source: RawSource::Jsonl(jsonl.to_string()),
            fork_parent_session_id: None,
        };
        let mut sink = ObservationCapturingSink::default();

        CodexSessionReader
            .visit(&input, &mut sink)
            .expect("visit zero-component-heartbeat session");

        assert!(
            sink.observations.is_empty(),
            "unexpected observations: {:?}",
            sink.observations
        );
        assert!(
            sink.unusable.is_empty(),
            "unexpected unusable reasons: {:?}",
            sink.unusable
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
            fork_parent_session_id: None,
        };
        let mut sink = ContentCapturingSink::default();

        CodexSessionReader
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
            r#"{"timestamp":"2026-08-05T10:00:07Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":350,"cached_input_tokens":100,"output_tokens":50,"total_tokens":400},"total_token_usage":{"input_tokens":650,"cached_input_tokens":200,"output_tokens":90,"total_tokens":740},"model_context_window":100000}}}"#,
            "\n",
        );
        let input = SessionInput {
            agent: "codex".to_string(),
            session_id: "fork-speed".to_string(),
            source: RawSource::Jsonl(jsonl.to_string()),
            fork_parent_session_id: None,
        };
        let mut sink = SessionCollector::new("codex", "fork-speed");

        CodexSessionReader
            .visit(&input, &mut sink)
            .expect("visit fork speed session");
        let session = sink.into_session().expect("fork speed session finishes");

        let owned_event = session
            .events
            .iter()
            .find(|event| event.model.as_deref() == Some("gpt-child"))
            .expect("the child's owned token_count turn is emitted");
        assert_eq!(owned_event.speed.as_deref(), Some("fast"));
        assert!(
            session
                .events
                .iter()
                .filter(|event| event.usage.output_tokens > 0)
                .all(|event| event.speed.as_deref() == Some("fast"))
        );
    }

    /// Collects every `EvidenceObservation` and `Unusable` reason a visit
    /// emits, in order.
    #[derive(Default)]
    struct ObservationCapturingSink {
        observations: Vec<EvidenceObservation>,
        unusable: Vec<PartialReason>,
    }

    impl RecordSink for ObservationCapturingSink {
        fn record(&mut self, record: NormalizedRecord) {
            match record {
                NormalizedRecord::Observation(observation) => {
                    self.observations.push(*observation);
                }
                NormalizedRecord::Unusable(reason) => self.unusable.push(reason),
                NormalizedRecord::MetricsEvent(_) | NormalizedRecord::TurnContent(_) => {}
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
            fork_parent_session_id: None,
        };
        let mut sink = ObservationCapturingSink::default();

        CodexSessionReader
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
            fork_parent_session_id: None,
        };
        let mut sink = ObservationCapturingSink::default();

        CodexSessionReader
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
            fork_parent_session_id: None,
        };
        let mut sink = ObservationCapturingSink::default();

        CodexSessionReader
            .visit(&input, &mut sink)
            .expect("visit spawn-other-name session");

        assert!(spawn_agent_observations(&sink).is_empty());
    }

    /* ------------------------------------------------------------------
     * Resume snapshots.
     * ------------------------------------------------------------------ */

    mod resume_snapshots {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::path::{Path, PathBuf};

        use tempfile::TempDir;

        use super::*;
        use crate::analysis::evidence::{EvidenceSource, SourceCapabilities, SourceKind};
        use crate::analysis::evidence_sink::{EvidenceResumeState, SessionEvidenceAccumulator};
        use crate::analysis::metrics_sink::SessionMetricsAccumulator;
        use crate::analysis::resume::EvidenceSnapshot;
        use crate::analysis::source_validity::ResumePoint;
        use crate::analysis::{RESUME_SNAPSHOT_REVISION, SourceChangedReason};
        use crate::discovery::source_version::head_hash_of;
        use crate::discovery::{FingerprintInputs, SourceStat};

        const FIRST_RECORD: &str = concat!(
            r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"first"}]}}"#,
            "\n",
        );
        const SECOND_RECORD: &str = concat!(
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"second"}]}}"#,
            "\n",
        );

        fn file_input(path: &Path) -> SessionInput {
            SessionInput {
                agent: "codex".to_string(),
                session_id: "claimed-session".to_string(),
                source: RawSource::File(path.to_path_buf()),
                fork_parent_session_id: None,
            }
        }

        fn claim_for_path(path: &Path) -> SourceClaim {
            let file = File::open(path).expect("open source for claim");
            let stat = SourceStat::from_open_std_file(&file).expect("stat source for claim");
            let bytes = std::fs::read(path).expect("read source for claim");
            SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
                stat,
                head_hash: Some(head_hash_of(&bytes)),
            })
        }

        fn write_source(directory: &TempDir, bytes: &[u8]) -> PathBuf {
            let path = directory.path().join("session.jsonl");
            std::fs::write(&path, bytes).expect("write source");
            path
        }

        /// A full [`StreamSnapshot`] around `resume` (the adapter's own
        /// half), with fresh metrics/evidence/index state. Mirrors
        /// `claude.rs`'s test helper of the same name.
        fn snapshot_from(resume: AdapterResume) -> StreamSnapshot {
            let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
                agent: "codex".to_owned(),
                session_id: "claimed-session".to_owned(),
                kind: SourceKind::Jsonl,
                capabilities: SourceCapabilities::codex(),
            });
            StreamSnapshot {
                revision: RESUME_SNAPSHOT_REVISION,
                resume: resume.point,
                adapter: resume.adapter,
                metrics: SessionMetricsAccumulator::new("codex", "claimed-session"),
                evidence: EvidenceSnapshot {
                    record: evidence.coverage_record(),
                    resume: EvidenceResumeState::default(),
                },
                next_turn_index: 0,
            }
        }

        /// A snapshot at offset zero, ready to resume a whole file from its
        /// start: [`PinnedSource::open_resumed`]'s offset-zero case.
        fn fresh_snapshot() -> StreamSnapshot {
            snapshot_from(AdapterResume {
                point: ResumePoint {
                    offset: 0,
                    tail_hash: head_hash_of(&[]),
                    tail_len: 0,
                },
                adapter: CodexSessionReader::empty_adapter_snapshot(),
            })
        }

        #[test]
        fn resumed_requests_keep_the_explicit_custom_provider() {
            let directory = TempDir::new().unwrap();
            let prefix = concat!(
                r#"{"type":"session_meta","payload":{"model_provider":"custom"}}"#,
                "\n",
                r#"{"type":"turn_context","payload":{"model":"gpt-5.6","effort":"high"}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"service_tier":"priority"}}}"#,
                "\n",
            );
            let path = write_source(&directory, prefix.as_bytes());
            let input = file_input(&path);
            let mut collector = SessionCollector::new("codex", "claimed-session");
            let first = CodexSessionReader
                .visit_claimed_resumed(
                    &input,
                    &claim_for_path(&path),
                    &fresh_snapshot(),
                    &|| false,
                    &mut collector,
                )
                .unwrap();
            OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(SECOND_RECORD.as_bytes())
                .unwrap();
            let mut resumed = SessionCollector::new("codex", "claimed-session");
            CodexSessionReader
                .visit_claimed_resumed(
                    &input,
                    &claim_for_path(&path),
                    &snapshot_from(first.resume.unwrap()),
                    &|| false,
                    &mut resumed,
                )
                .unwrap();
            let session = resumed.into_session().unwrap();
            assert_eq!(session.events.len(), 1);
            let event = &session.events[0];
            assert_eq!(event.provider.as_deref(), Some("custom"));
            assert!(event.api.is_none());
            assert_eq!(event.thinking_mode.as_deref(), Some("high"));
            assert_eq!(event.speed.as_deref(), Some("fast"));
            let mut full = SessionCollector::new("codex", "claimed-session");
            CodexSessionReader.visit(&input, &mut full).unwrap();
            assert_eq!(session, full.into_session().unwrap());
        }

        #[test]
        fn a_resumed_read_from_offset_zero_matches_a_full_read() {
            let directory = TempDir::new().expect("tempdir");
            let path = write_source(&directory, FIRST_RECORD.as_bytes());
            let claim = claim_for_path(&path);
            let input = file_input(&path);
            let mut collector = SessionCollector::new("codex", "claimed-session");

            let visit = CodexSessionReader
                .visit_claimed_resumed(&input, &claim, &fresh_snapshot(), &|| false, &mut collector)
                .expect("resumed visit of a fresh file");

            assert_eq!(visit.outcome, VisitOutcome::AcceptedFull);
            let resume = visit.resume.expect("a settled pass carries a resume");
            assert_eq!(resume.point.offset, FIRST_RECORD.len() as u64);
            assert_eq!(
                collector
                    .into_session()
                    .expect("resumed read must publish")
                    .events
                    .len(),
                1
            );
        }

        #[test]
        fn a_second_resumed_read_continues_from_the_first_snapshot() {
            let directory = TempDir::new().expect("tempdir");
            let path = write_source(&directory, FIRST_RECORD.as_bytes());
            let input = file_input(&path);
            let mut first_pass = SessionCollector::new("codex", "claimed-session");
            let first_claim = claim_for_path(&path);
            let first_visit = CodexSessionReader
                .visit_claimed_resumed(
                    &input,
                    &first_claim,
                    &fresh_snapshot(),
                    &|| false,
                    &mut first_pass,
                )
                .expect("first resumed visit");
            let resume = first_visit.resume.expect("a settled pass carries a resume");
            let snapshot = snapshot_from(resume);

            OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open source for append")
                .write_all(SECOND_RECORD.as_bytes())
                .expect("append second record");
            let second_claim = claim_for_path(&path);
            let mut second_pass = SessionCollector::new("codex", "claimed-session");

            let second_visit = CodexSessionReader
                .visit_claimed_resumed(
                    &input,
                    &second_claim,
                    &snapshot,
                    &|| false,
                    &mut second_pass,
                )
                .expect("second resumed visit");

            assert_eq!(second_visit.outcome, VisitOutcome::AcceptedFull);
            assert_eq!(
                second_pass
                    .into_session()
                    .expect("resumed read must publish")
                    .events
                    .len(),
                1,
                "the resumed pass reads only the newly appended record"
            );
        }

        #[test]
        fn a_resumed_legacy_usage_record_deduplicates_after_a_new_record() {
            let first_records = concat!(
                r#"{"timestamp":"2026-09-02T00:00:00Z","type":"turn_context","payload":{"effort":"high"}}"#,
                "\n",
                r#"{"timestamp":"2026-09-02T00:00:01Z","type":"token_usage_record","payload":{"usage":{"input_tokens":30,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":35},"turn_token_usage":{"input_tokens":30,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":35},"thread_token_usage":{"input_tokens":30,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":35}}}"#,
                "\n",
            );
            let legacy_record = concat!(
                r#"{"timestamp":"2026-09-02T00:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":96000,"last_token_usage":{"input_tokens":30,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":35},"total_token_usage":{"input_tokens":30,"cached_input_tokens":10,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":35}}}}"#,
                "\n",
            );
            let directory = TempDir::new().expect("tempdir");
            let path = write_source(&directory, first_records.as_bytes());
            let input = file_input(&path);
            let first_claim = claim_for_path(&path);
            let mut first_pass = SessionCollector::new("codex", "claimed-session");
            let first_visit = CodexSessionReader
                .visit_claimed_resumed(
                    &input,
                    &first_claim,
                    &fresh_snapshot(),
                    &|| false,
                    &mut first_pass,
                )
                .expect("first resumed visit");
            let first_session = first_pass.into_session().expect("first pass finishes");
            assert_eq!(first_session.events.len(), 1);
            let snapshot = snapshot_from(first_visit.resume.expect("first pass resumes"));

            OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open source for append")
                .write_all(legacy_record.as_bytes())
                .expect("append legacy usage record");
            let second_claim = claim_for_path(&path);
            let mut second_pass = SessionCollector::new("codex", "claimed-session");
            let second_visit = CodexSessionReader
                .visit_claimed_resumed(
                    &input,
                    &second_claim,
                    &snapshot,
                    &|| false,
                    &mut second_pass,
                )
                .expect("second resumed visit");
            let second_session = second_pass.into_session().expect("second pass finishes");

            assert_eq!(second_visit.outcome, VisitOutcome::AcceptedFull);
            assert!(second_session.events.is_empty());
            assert_eq!(second_session.context_window, Some(96_000));
        }

        #[test]
        fn a_resumed_mismatched_cumulative_usage_record_deduplicates() {
            let first_records = concat!(
                r#"{"timestamp":"2026-09-02T01:00:00Z","type":"turn_context","payload":{"effort":"high"}}"#,
                "\n",
                r#"{"timestamp":"2026-09-02T01:00:01Z","type":"token_usage_record","payload":{"usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"turn_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"thread_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5}}}"#,
                "\n",
            );
            let legacy_record = concat!(
                r#"{"timestamp":"2026-09-02T01:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":5},"total_token_usage":{"input_tokens":300,"cached_input_tokens":200,"output_tokens":15}}}}"#,
                "\n",
            );
            let directory = TempDir::new().expect("tempdir");
            let path = write_source(&directory, first_records.as_bytes());
            let input = file_input(&path);
            let first_claim = claim_for_path(&path);
            let mut first_pass = SessionCollector::new("codex", "claimed-session");
            let first_visit = CodexSessionReader
                .visit_claimed_resumed(
                    &input,
                    &first_claim,
                    &fresh_snapshot(),
                    &|| false,
                    &mut first_pass,
                )
                .expect("first resumed visit");
            assert_eq!(first_pass.into_session().unwrap().events.len(), 1);
            let snapshot = snapshot_from(first_visit.resume.expect("first pass resumes"));

            OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open source for append")
                .write_all(legacy_record.as_bytes())
                .expect("append legacy usage record");
            let second_claim = claim_for_path(&path);
            let mut second_pass = SessionCollector::new("codex", "claimed-session");
            let second_visit = CodexSessionReader
                .visit_claimed_resumed(
                    &input,
                    &second_claim,
                    &snapshot,
                    &|| false,
                    &mut second_pass,
                )
                .expect("second resumed visit");
            let resumed_session = second_pass.into_session().unwrap();

            assert_eq!(second_visit.outcome, VisitOutcome::AcceptedFull);
            assert!(resumed_session.events.is_empty());
            let mut full = SessionCollector::new("codex", "claimed-session");
            CodexSessionReader.visit(&input, &mut full).unwrap();
            assert_eq!(full.into_session().unwrap().events.len(), 1);
        }

        #[test]
        fn a_rewritten_tail_fails_a_resumed_read_without_a_snapshot() {
            let directory = TempDir::new().expect("tempdir");
            let path = write_source(&directory, FIRST_RECORD.as_bytes());
            let input = file_input(&path);
            let first_claim = claim_for_path(&path);
            let mut first_pass = SessionCollector::new("codex", "claimed-session");
            let first_visit = CodexSessionReader
                .visit_claimed_resumed(
                    &input,
                    &first_claim,
                    &fresh_snapshot(),
                    &|| false,
                    &mut first_pass,
                )
                .expect("first resumed visit");
            let resume = first_visit.resume.expect("a settled pass carries a resume");
            let snapshot = snapshot_from(resume);

            // Same identity, a rewritten tail: the old snapshot's offset now
            // points past a rewritten byte instead of an append. Re-claiming
            // against the rewritten content isolates that check from the
            // unrelated head-region check `open_resumed` also runs.
            std::fs::write(&path, SECOND_RECORD.as_bytes()).expect("rewrite source");
            let rewritten_claim = claim_for_path(&path);
            let mut second_pass = SessionCollector::new("codex", "claimed-session");

            let visit = CodexSessionReader
                .visit_claimed_resumed(
                    &input,
                    &rewritten_claim,
                    &snapshot,
                    &|| false,
                    &mut second_pass,
                )
                .expect("resumed visit of a rewritten source");

            assert_eq!(
                visit.outcome,
                VisitOutcome::SourceChanged(SourceChangedReason::ResumeTailMismatch)
            );
            assert!(visit.resume.is_none());
            assert!(second_pass.into_session().is_err());
        }

        /// A subagent rollout whose ownership is still `Pending` at EOF (no
        /// `agent_message` ever addresses the child): the "unsettled" rule
        /// on `visit_claimed_resumed`. The read still settles
        /// (`AcceptedFull`, and the sink still publishes the buffered rows
        /// `finish` flushes), but no resume snapshot comes back — a later
        /// full pass could still resolve those rows differently.
        #[test]
        fn a_pending_fork_at_eof_settles_but_carries_no_resume_snapshot() {
            let jsonl = include_str!(
                "../../../tests/fixtures/codex_characterization/unresolved_fork.jsonl"
            );
            let directory = TempDir::new().expect("tempdir");
            let path = write_source(&directory, jsonl.as_bytes());
            let claim = claim_for_path(&path);
            let input = file_input(&path);
            let mut collector = SessionCollector::new("codex", "claimed-session");

            let visit = CodexSessionReader
                .visit_claimed_resumed(&input, &claim, &fresh_snapshot(), &|| false, &mut collector)
                .expect("resumed visit of a pending fork");

            assert_eq!(visit.outcome, VisitOutcome::AcceptedFull);
            assert!(
                visit.resume.is_none(),
                "pending fork ownership at EOF must not carry a resume snapshot"
            );
            assert!(
                !collector
                    .into_session()
                    .expect("a settled pass still publishes")
                    .events
                    .is_empty(),
                "finish still flushes the pending rows as owned"
            );
        }

        /// After a pending fork's first resumed pass reports no snapshot,
        /// a later append that resolves ownership (an `agent_message`
        /// addressed to the child) is picked up by a fresh bootstrap pass
        /// — offset zero, a fresh adapter state — which does produce a
        /// resume snapshot once ownership settles to `Owned`.
        #[test]
        fn a_later_append_that_resolves_ownership_lets_a_fresh_bootstrap_pass_snapshot() {
            let jsonl = include_str!(
                "../../../tests/fixtures/codex_characterization/unresolved_fork.jsonl"
            );
            let directory = TempDir::new().expect("tempdir");
            let path = write_source(&directory, jsonl.as_bytes());
            let claim = claim_for_path(&path);
            let input = file_input(&path);
            let mut first_pass = SessionCollector::new("codex", "claimed-session");
            let first_visit = CodexSessionReader
                .visit_claimed_resumed(
                    &input,
                    &claim,
                    &fresh_snapshot(),
                    &|| false,
                    &mut first_pass,
                )
                .expect("resumed visit of a pending fork");
            assert!(first_visit.resume.is_none());

            let resolving = concat!(
                r#"{"timestamp":"2026-08-06T10:00:03Z","type":"event_msg","payload":{"type":"task_started"}}"#,
                "\n",
                r#"{"timestamp":"2026-08-06T10:00:04Z","type":"response_item","payload":{"type":"agent_message","author":"parent","recipient":"worker","content":[{"type":"input_text","text":"Handle the synthetic task."}]}}"#,
                "\n",
            );
            OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open source for append")
                .write_all(resolving.as_bytes())
                .expect("append resolving records");

            // A fresh bootstrap pass: offset zero, a fresh adapter state —
            // the fallback the desktop worker and the parity harness use
            // whenever a resumed pass carries no snapshot forward.
            let bootstrap_claim = claim_for_path(&path);
            let mut second_pass = SessionCollector::new("codex", "claimed-session");
            let second_visit = CodexSessionReader
                .visit_claimed_resumed(
                    &input,
                    &bootstrap_claim,
                    &fresh_snapshot(),
                    &|| false,
                    &mut second_pass,
                )
                .expect("fresh bootstrap pass over the resolved fork");

            assert_eq!(second_visit.outcome, VisitOutcome::AcceptedFull);
            assert!(
                second_visit.resume.is_some(),
                "ownership resolved to Owned by EOF must carry a resume snapshot"
            );
        }

        /// `pending_rows` must round-trip through a `StreamSnapshot` even
        /// when populated: `visit_claimed_resumed` never encodes a state
        /// whose buffer is non-empty today (see the "unsettled" rule), but
        /// the codec itself must not depend on that invariant holding.
        #[test]
        fn a_populated_pending_rows_buffer_round_trips_through_a_stream_snapshot() {
            let state = CodexStreamState {
                ownership: ForkOwnership::Pending,
                pending_rows: vec![
                    serde_json::json!({
                        "type": "session_meta",
                        "payload": {"id": "synthetic", "thread_source": "subagent"}
                    }),
                    serde_json::json!({
                        "type": "event_msg",
                        "payload": {
                            "type": "token_count",
                            "info": {"last_token_usage": {"input_tokens": 5}}
                        }
                    }),
                ],
                pending_bytes: 42,
                ..Default::default()
            };

            let adapter_bytes =
                postcard::to_allocvec(&state).expect("a populated CodexStreamState always encodes");
            let snapshot = snapshot_from(AdapterResume {
                point: ResumePoint {
                    offset: 0,
                    tail_hash: head_hash_of(&[]),
                    tail_len: 0,
                },
                adapter: crate::analysis::resume::AdapterSnapshot(adapter_bytes),
            });

            let encoded = snapshot.encode();
            let decoded = StreamSnapshot::decode(&encoded).expect("decode stream snapshot");
            let decoded_state: CodexStreamState = postcard::from_bytes(&decoded.adapter.0)
                .expect("decode codex adapter snapshot with a populated pending_rows buffer");

            assert_eq!(decoded_state.ownership, ForkOwnership::Pending);
            assert_eq!(decoded_state.pending_rows, state.pending_rows);
            assert_eq!(decoded_state.pending_bytes, state.pending_bytes);
        }
    }
}
