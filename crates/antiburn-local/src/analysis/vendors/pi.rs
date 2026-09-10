//! Pi v3 JSONL adapter.
//!
//! Pi writes one record per line. Semantic rows are `message`, `model_change`,
//! `thinking_level_change`, and `compaction`. The adapter recognizes the
//! `session` and `session_info` housekeeping rows by shape. It also recognizes
//! inert `custom` and `custom_message` records only when shared parser fields
//! cannot carry analysis signals. It recognizes `bashExecution` as a Pi
//! housekeeping role under the same rule.
//!
//! The top-level row timestamp controls ordering. Assistant messages can also
//! carry a request-start timestamp inside `message.timestamp`; usage uses that
//! position while event ordering keeps the top-level timestamp. Usage contains four disjoint
//! buckets: `input`, `output`, `cacheRead`, and `cacheWrite`. Extra usage fields
//! do not contribute to accounting. A linked child excludes rows whose
//! timestamps precede the session header. The adapter checks only parent-link
//! key presence and never reads the private path value.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::analysis::evidence::MAX_SUBAGENT_CHILDREN;
use crate::analysis::framing::{BoundedJsonlReader, FramedRecord, PartialReason, RecordSkip};
use crate::analysis::interface::{
    ContentPart, ContextWindowSource, EvidenceObservation, NormalizedRecord, ProviderHint,
    RawSource, RecordSink, ResumedVisit, SessionCollector, SessionInput, SessionReader,
    SessionSummary, TurnContent, VisitOutcome, bounded_provider_hint_value, push_provider_hint,
};
use crate::analysis::model::{NormalizedEvent, NormalizedSession, Role};
use crate::analysis::records::{
    RecordShape, extract_content_parts, parse_record, parse_ts, thread_identity_field,
};
use crate::analysis::resume::{AdapterResume, StreamSnapshot};
use crate::analysis::source_validity::{AppendOnlyGuarantee, PinnedSource, SourceClaim};
use crate::analysis::threads::ThreadResolver;

/// Parses Pi transcript files without retaining transcript content.
pub struct PiSessionReader;

impl SessionReader for PiSessionReader {
    fn agent(&self) -> &'static str {
        "pi"
    }

    fn capabilities(&self, _source: &RawSource) -> crate::analysis::SourceCapabilities {
        crate::analysis::SourceCapabilities::pi()
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
            let state = match &input.source {
                RawSource::File(path) => self.visit_reader(
                    BufReader::new(File::open(path)?),
                    &|| false,
                    sink,
                    PiStreamState::default(),
                )?,
                RawSource::Jsonl(content) => {
                    let suffix: &[u8] = if content.ends_with('\n') { b"" } else { b"\n" };
                    let source = Cursor::new(content.as_bytes()).chain(suffix);
                    self.visit_reader(
                        BufReader::new(source),
                        &|| false,
                        sink,
                        PiStreamState::default(),
                    )?
                }
                RawSource::Sqlite(_) => {
                    anyhow::bail!("sqlite source must be handled by the sqlite adapter")
                }
            };
            sink.finish(state.finish());
            Ok(VisitOutcome::Unvalidated)
        })()
        .context("reading Pi session")
    }

    fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        guarantee: AppendOnlyGuarantee,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        PiSessionReader::visit_claimed(self, input, claim, guarantee, cancel, sink)
    }

    fn visit_claimed_resumed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        resume: &StreamSnapshot,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<ResumedVisit> {
        PiSessionReader::visit_claimed_resumed(self, input, claim, resume, cancel, sink)
    }

    fn empty_resume_state(&self) -> Option<crate::analysis::resume::AdapterSnapshot> {
        Some(PiSessionReader::empty_adapter_snapshot())
    }
}

impl PiSessionReader {
    /// A fresh [`PiStreamState`], serialized. Mirrors
    /// [`crate::analysis::vendors::claude::ClaudeSessionReader::empty_adapter_snapshot`]:
    /// pairs with a [`StreamSnapshot`] whose [`ResumePoint`][rp] offset is
    /// zero to start the first resumable pass over a source.
    ///
    /// [rp]: crate::analysis::source_validity::ResumePoint
    pub fn empty_adapter_snapshot() -> crate::analysis::resume::AdapterSnapshot {
        crate::analysis::resume::AdapterSnapshot(
            postcard::to_allocvec(&PiStreamState::default())
                .expect("a default PiStreamState always encodes"),
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
                anyhow::bail!("a claimed Pi source must be a file");
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
                PiStreamState::default(),
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
            sink.finish(state.finish());
            Ok(outcome)
        })()
        .context("reading claimed Pi session")
    }

    /// Streams a file from a verified [`StreamSnapshot`], restoring
    /// [`PiStreamState`] from `resume.adapter` and reading only the bytes
    /// past `resume.resume.offset`. Mirrors
    /// [`crate::analysis::vendors::claude::ClaudeSessionReader::visit_claimed_resumed`]
    /// exactly; see its doc comment for the full read/recheck/snapshot shape.
    ///
    /// The snapshot retains branch policies so a resumed pass can follow earlier parent links.
    /// This method's `resume` is `None` only when `outcome` is
    /// [`VisitOutcome::SourceChanged`].
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
                anyhow::bail!("a claimed Pi source must be a file");
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
            let initial_state: PiStreamState =
                postcard::from_bytes(&resume.adapter.0).context("decoding Pi adapter snapshot")?;
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
            let adapter = postcard::to_allocvec(&state).context("encoding Pi adapter snapshot")?;
            let new_resume = pinned.resume_point()?;
            sink.finish(state.finish());
            Ok(ResumedVisit {
                outcome,
                resume: Some(AdapterResume {
                    point: new_resume,
                    adapter: crate::analysis::resume::AdapterSnapshot(adapter),
                }),
            })
        })()
        .context("reading resumed Pi session")
    }

    /// Streams `reader` starting from `state`, so a resumed pass can carry
    /// forward the state a prior pass left off with. A first pass starts
    /// from `PiStreamState::default()`. Returns the state at the end of the
    /// stream, not yet reduced to a [`SessionSummary`]: the caller decides
    /// whether to snapshot it before calling [`PiStreamState::finish`].
    fn visit_reader(
        &self,
        reader: impl BufRead,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
        mut state: PiStreamState,
    ) -> anyhow::Result<PiStreamState> {
        let mut reader = BoundedJsonlReader::new(reader);

        while let Some(record) = reader.next_record(cancel) {
            match record {
                FramedRecord::Skipped(skip) => match skip {
                    RecordSkip::Oversized { .. } | RecordSkip::IncompleteTail { .. } => {
                        sink.record(NormalizedRecord::Unusable(skip.partial_reason()));
                    }
                    RecordSkip::ReadFailed { index, kind } => {
                        anyhow::bail!("Pi record {index} read failed: {kind:?}");
                    }
                    RecordSkip::Cancelled { index } => {
                        anyhow::bail!("Pi record {index} read was cancelled");
                    }
                },
                FramedRecord::Complete { bytes, .. } => {
                    let record = std::str::from_utf8(bytes)
                        .context("Pi transcript record is not valid UTF-8")?;
                    let Ok(value) = serde_json::from_str::<Value>(record) else {
                        sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
                        continue;
                    };
                    state.observe(value, sink);
                }
            }
        }

        Ok(state)
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
struct PiPolicy {
    model: Option<String>,
    provider: Option<String>,
    thinking_mode: Option<String>,
}

const MAX_SUBAGENT_CALLS: usize = 1024;
const MAX_WORKER_MESSAGES: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PiSubagentCall {
    ordinal: usize,
    parent_model: Option<String>,
    ts_ms: Option<i64>,
    resolved: bool,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
struct PiStreamState {
    model: Option<String>,
    current_model: Option<String>,
    current_provider: Option<String>,
    provider_hints: Vec<ProviderHint>,
    current_thinking_mode: Option<String>,
    started_at_ms: Option<i64>,
    cache_write_tokens_available: Option<bool>,
    fork_header_present: bool,
    fork_start_ms: Option<i64>,
    fork_attribution_incomplete: bool,
    /// Derives each row's thread from the `id` / `parentId` chain. Every row
    /// after the session header carries both fields, so this resolves over
    /// message rows and non-message rows (`model_change`,
    /// `thinking_level_change`, `compaction`, …) alike — a message whose
    /// `parentId` names a `model_change` row still joins that row's thread.
    threads: ThreadResolver,
    policy_by_id: HashMap<String, PiPolicy>,
    subagent_calls: HashMap<String, PiSubagentCall>,
    subagent_incomplete: bool,
}

impl PiStreamState {
    fn observe(&mut self, value: Value, sink: &mut dyn RecordSink) {
        let id = thread_identity_field(&value, "id");
        let parent_id = thread_identity_field(&value, "parentId");
        let thread_id = self.threads.resolve(id.as_deref(), parent_id.as_deref());
        // Rows without lineage fields retain the existing headerless input behavior.
        if id.is_some() || value.get("parentId").is_some() {
            let policy = parent_id
                .as_ref()
                .and_then(|parent| self.policy_by_id.get(parent))
                .cloned()
                .unwrap_or_default();
            self.current_model = policy.model;
            self.current_provider = policy.provider;
            self.current_thinking_mode = policy.thinking_mode;
        }
        self.observe_row(&value, thread_id, sink);
        // Use the thread resolver's bound and keep each identity's first policy.
        if !self.threads.capped()
            && let Some(id) = id
        {
            self.policy_by_id.entry(id).or_insert_with(|| PiPolicy {
                model: self.current_model.clone(),
                provider: self.current_provider.clone(),
                thinking_mode: self.current_thinking_mode.clone(),
            });
        }
    }

    fn observe_row(&mut self, value: &Value, thread_id: Option<String>, sink: &mut dyn RecordSink) {
        let row_type = value.get("type").and_then(Value::as_str);
        let id = thread_identity_field(value, "id");
        let parent_id = thread_identity_field(value, "parentId");
        if let Some(observation) = thread_link_observation(id.as_deref(), parent_id.as_deref()) {
            sink.record(NormalizedRecord::Observation(Box::new(observation)));
        }
        if row_type != Some("session") && self.fork_header_present {
            match (
                self.fork_start_ms,
                value.get("timestamp").and_then(parse_ts),
            ) {
                (Some(start_ms), Some(ts_ms)) if ts_ms < start_ms => {
                    match row_type {
                        Some("model_change") => self.observe_model_change(value, sink, true),
                        Some("thinking_level_change") => {
                            self.observe_thinking_level_change(value, sink, true);
                        }
                        Some("message")
                            if value.pointer("/message/role").and_then(Value::as_str)
                                == Some("assistant") =>
                        {
                            self.observe_assistant_metadata(value, true);
                        }
                        _ => {}
                    }
                    sink.record(NormalizedRecord::Observation(Box::new(
                        EvidenceObservation::InheritedRecord,
                    )));
                    return;
                }
                (Some(_), Some(_)) => {}
                _ => {
                    self.current_model = None;
                    self.current_provider = None;
                    self.current_thinking_mode = None;
                    self.fork_attribution_incomplete = true;
                    sink.record(NormalizedRecord::Unusable(
                        PartialReason::AttributionIncomplete,
                    ));
                    return;
                }
            }
        }
        let has_metric_timestamp = row_type == Some("compaction")
            || (row_type == Some("message")
                && matches!(
                    value.pointer("/message/role").and_then(Value::as_str),
                    Some("user" | "assistant" | "toolResult")
                ));
        if !has_metric_timestamp && let Some(ts_ms) = value.get("timestamp").and_then(parse_ts) {
            sink.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::RecordTimestamp { ts_ms },
            )));
        }
        match row_type {
            Some("session") if is_inert_shape(value) => self.observe_session(value, sink),
            Some("message") => self.observe_message(value, thread_id, sink),
            Some("model_change") => self.observe_model_change(value, sink, false),
            Some("thinking_level_change") => self.observe_thinking_level_change(value, sink, false),
            Some("compaction") => self.observe_compaction(value, thread_id, sink),
            Some("session_info") if is_inert_shape(value) => observe_inert(value, sink),
            Some("custom" | "custom_message") if is_inert_shape(value) => {
                observe_inert(value, sink)
            }
            Some(discriminator) => unrecognized(discriminator, sink),
            None => unrecognized("<missing>", sink),
        }
    }

    fn observe_session(&mut self, value: &Value, sink: &mut dyn RecordSink) {
        let has_parent = value
            .as_object()
            .is_some_and(|header| header.contains_key("parentSession"));
        if has_parent {
            self.fork_header_present = true;
            self.fork_start_ms = value.get("timestamp").and_then(parse_ts);
            self.fork_attribution_incomplete = self.fork_start_ms.is_none();
        }

        let supported = value
            .get("version")
            .is_some_and(|version| version.as_u64() == Some(3) || version.as_str() == Some("3"));
        if !supported {
            unrecognized("session", sink);
            return;
        }
        let Some(timestamp) = value.get("timestamp").and_then(parse_ts) else {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            return;
        };
        if self.started_at_ms.is_none() {
            self.started_at_ms = Some(timestamp);
        }
    }

    fn observe_message(
        &mut self,
        value: &Value,
        thread_id: Option<String>,
        sink: &mut dyn RecordSink,
    ) {
        let role = value
            .pointer("/message/role")
            .and_then(Value::as_str)
            .unwrap_or("<missing>");
        if role == "bashExecution" {
            if is_inert_shape(value) {
                observe_inert(value, sink);
            } else {
                unrecognized(role, sink);
            }
            return;
        }
        if !matches!(role, "user" | "assistant" | "toolResult") {
            unrecognized(role, sink);
            return;
        }

        let Some(timestamp) = value.get("timestamp").and_then(parse_ts) else {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            return;
        };
        let Some(mut event) = parse_record(value, RecordShape::Pi) else {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            return;
        };
        event.ts_ms = Some(timestamp);
        event.usage_ts_ms = (role == "assistant")
            .then(|| value.pointer("/message/timestamp").and_then(Value::as_i64))
            .flatten()
            .filter(|usage_ts| {
                self.started_at_ms
                    .is_some_and(|started| *usage_ts >= started && *usage_ts <= timestamp)
            });
        event.speed = None;
        event.thread_id = thread_id;

        if role == "assistant" {
            self.observe_assistant_metadata(value, false);
            event.model = event.model.or_else(|| self.current_model.clone());
            if self.model.is_none() {
                self.model = event.model.clone();
            }
            event.provider = value
                .pointer("/message/provider")
                .and_then(Value::as_str)
                .and_then(bounded_provider_hint_value);
            event.api = value
                .pointer("/message/api")
                .and_then(Value::as_str)
                .and_then(bounded_provider_hint_value);
            if let Some(provider) = value
                .pointer("/message/provider")
                .and_then(Value::as_str)
                .or(self.current_provider.as_deref())
            {
                push_provider_hint(&mut self.provider_hints, provider, event.model.as_deref());
            }
            // Keep the agent policy separate from message.providerThinkingLevel. The latter is not an agent-selected level.
            event.thinking_mode = self.current_thinking_mode.clone();
            self.observe_subagent_calls(value, &event, sink);
        }
        // Read explicit identities only. The shared parser also infers skill names from paths and commands.
        event.tools = value
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("toolCall"))
            .filter_map(|block| {
                let name = block
                    .get("name")?
                    .as_str()
                    .filter(|name| !name.is_empty())?;
                let mut tool =
                    crate::analysis::records::tool_call_from_input(name, block.get("arguments"));
                if name.eq_ignore_ascii_case("skill") {
                    tool.detail = block
                        .pointer("/arguments/skill")
                        .and_then(Value::as_str)
                        .and_then(pi_skill_identity)
                        .map(str::to_owned);
                }
                Some(tool)
            })
            .collect();

        let unknown_blocks = unknown_content_blocks(value);
        let content_parts = if role == "toolResult"
            && value.pointer("/message/toolName").and_then(Value::as_str) == Some("subagent")
        {
            Vec::new()
        } else if role == "assistant"
            && let Some(blocks) = value.pointer("/message/content").and_then(Value::as_array)
            && blocks
                .iter()
                .any(|block| block["type"] == "toolCall" && block["name"] == "subagent")
        {
            let content: Vec<_> = blocks
                .iter()
                .filter(|block| !(block["type"] == "toolCall" && block["name"] == "subagent"))
                .collect();
            extract_content_parts(&serde_json::json!({"content": content}), event.role)
        } else {
            extract_content_parts(value, event.role)
        };
        self.emit_event(event, content_parts, sink);
        if role == "toolResult"
            && value.pointer("/message/toolName").and_then(Value::as_str) == Some("subagent")
        {
            self.observe_subagent_result(&value["message"], sink);
        }
        for discriminator in &unknown_blocks {
            sink.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::UnrecognizedType {
                    discriminator: discriminator.clone(),
                    inert: false,
                },
            )));
        }
        if !unknown_blocks.is_empty() {
            sink.record(NormalizedRecord::Unusable(
                PartialReason::UnrecognizedRecordType,
            ));
        }
    }

    fn observe_subagent_calls(
        &mut self,
        value: &Value,
        event: &NormalizedEvent,
        sink: &mut dyn RecordSink,
    ) {
        for block in value
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if block.get("type").and_then(Value::as_str) != Some("toolCall")
                || block.get("name").and_then(Value::as_str) != Some("subagent")
            {
                continue;
            }
            let Some(id) = pi_subagent_id(block, "id") else {
                self.subagent_incomplete = true;
                continue;
            };
            if self.subagent_calls.contains_key(id) {
                continue;
            }
            if self.subagent_calls.len() == MAX_SUBAGENT_CALLS {
                self.subagent_incomplete = true;
                continue;
            }
            self.subagent_calls.insert(
                id.to_owned(),
                PiSubagentCall {
                    ordinal: self.subagent_calls.len(),
                    parent_model: event.model.as_deref().and_then(bounded_provider_hint_value),
                    ts_ms: event.ts_ms,
                    resolved: false,
                },
            );
            sink.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::SubagentSpawn {
                    ts_ms: event.ts_ms,
                    parent_model: event.model.as_deref().and_then(bounded_provider_hint_value),
                    parent_call_id: Some(format!("{id}:0")),
                    child_model: None,
                    provenance: crate::analysis::interface::RelationProvenance::TaskToolUse,
                },
            )));
        }
    }

    fn observe_subagent_result(&mut self, message: &Value, sink: &mut dyn RecordSink) {
        let Some((id, call)) = pi_subagent_id(message, "toolCallId")
            .and_then(|id| self.subagent_calls.get_mut(id).map(|call| (id, call)))
        else {
            self.subagent_incomplete = true;
            return;
        };
        if call.resolved {
            return;
        }
        let details = &message["details"];
        if !matches!(
            details["mode"].as_str(),
            Some("single" | "parallel" | "chain")
        ) {
            self.subagent_incomplete = true;
            return;
        }
        let Some(results) = details["results"].as_array() else {
            self.subagent_incomplete = true;
            return;
        };
        // Empty results can indicate cancellation, not a worker with a known model.
        if results.is_empty() || (details["mode"] == "single" && results.len() != 1) {
            self.subagent_incomplete = true;
            return;
        }
        call.resolved = true;
        let call = call.clone();
        if call.parent_model.is_none() {
            self.subagent_incomplete = true;
        }
        if results.len() > MAX_SUBAGENT_CHILDREN {
            self.subagent_incomplete = true;
        }
        for (worker_index, result) in results.iter().take(MAX_SUBAGENT_CHILDREN).enumerate() {
            // The call already reports the first worker; parallel and chain results can add more.
            if worker_index > 0 {
                sink.record(NormalizedRecord::Observation(Box::new(
                    EvidenceObservation::SubagentSpawn {
                        ts_ms: call.ts_ms,
                        parent_model: call.parent_model.clone(),
                        parent_call_id: Some(format!("{id}:{worker_index}")),
                        child_model: None,
                        provenance: crate::analysis::interface::RelationProvenance::TaskToolUse,
                    },
                )));
            }
            let Some(messages) = result["messages"].as_array() else {
                self.subagent_incomplete = true;
                continue;
            };
            if result["exitCode"].as_i64().is_none_or(|code| code < 0) {
                self.subagent_incomplete = true;
            }
            if messages.len() > MAX_WORKER_MESSAGES {
                self.subagent_incomplete = true;
            }
            let thread = format!("pi-subagent:{}:{worker_index}", call.ordinal);
            let mut assistant_seen = false;
            for nested in messages.iter().take(MAX_WORKER_MESSAGES) {
                if nested["role"] != "assistant" {
                    if !matches!(nested["role"].as_str(), Some("user" | "toolResult")) {
                        self.subagent_incomplete = true;
                    }
                    continue;
                }
                assistant_seen = true;
                // Parse only assistant accounting fields. Never retain tasks, prompts, or tool arguments.
                let row = serde_json::json!({"message": {
                    "role": "assistant", "model": nested["model"], "usage": nested["usage"]
                }});
                let Some(mut event) = parse_record(&row, RecordShape::Pi) else {
                    self.subagent_incomplete = true;
                    continue;
                };
                event.model = nested["model"]
                    .as_str()
                    .and_then(bounded_provider_hint_value);
                event.provider = nested["provider"]
                    .as_str()
                    .and_then(bounded_provider_hint_value);
                event.api = nested["api"].as_str().and_then(bounded_provider_hint_value);
                // Pi assistant messages store epoch milliseconds, not the outer entry's timestamp format.
                event.ts_ms = nested["timestamp"].as_i64();
                event.source = crate::analysis::model::EventSource::Subagent;
                event.thread_id = Some(thread.clone());
                event.has_thinking = nested["content"]
                    .as_array()
                    .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "thinking"));
                if event.model.is_none() || event.ts_ms.is_none() {
                    self.subagent_incomplete = true;
                }
                if nested["model"]
                    .as_str()
                    .is_some_and(|model| model.len() > crate::analysis::EVIDENCE_STRING_CAP)
                    || !["input", "output", "cacheRead", "cacheWrite"]
                        .iter()
                        .all(|key| nested["usage"][key].as_u64().is_some())
                {
                    self.subagent_incomplete = true;
                }
                if let Some(provider) = &event.provider {
                    push_provider_hint(&mut self.provider_hints, provider, event.model.as_deref());
                }
                // Nested calls need their own result join, which this bounded adapter does not yet support.
                if nested["content"].as_array().is_some_and(|blocks| {
                    blocks
                        .iter()
                        .any(|block| block["type"] == "toolCall" && block["name"] == "subagent")
                }) {
                    self.subagent_incomplete = true;
                }
                if let Some(model) = nested["model"].as_str() {
                    sink.record(NormalizedRecord::Observation(Box::new(
                        EvidenceObservation::SubagentModel {
                            parent_call_id: format!("{id}:{worker_index}"),
                            model: model.to_owned(),
                        },
                    )));
                }
                self.emit_event(event, Vec::new(), sink);
            }
            if !assistant_seen {
                self.subagent_incomplete = true;
            }
        }
    }

    fn observe_assistant_metadata(&mut self, value: &Value, inherited: bool) {
        if let Some(model) = value
            .pointer("/message/model")
            .and_then(Value::as_str)
            .and_then(bounded_provider_hint_value)
        {
            if !inherited && self.model.is_none() {
                self.model = Some(model.clone());
            }
            self.current_model = Some(model);
        }
        if let Some(provider) = value
            .pointer("/message/provider")
            .and_then(Value::as_str)
            .and_then(bounded_provider_hint_value)
        {
            self.current_provider = Some(provider);
        }

        if !inherited
            && let Some(api) = value
                .pointer("/message/api")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|api| !api.is_empty())
        {
            let reports_cache_writes = api == "anthropic-messages";
            self.cache_write_tokens_available =
                Some(self.cache_write_tokens_available.unwrap_or(true) && reports_cache_writes);
        }
    }

    fn observe_model_change(&mut self, value: &Value, sink: &mut dyn RecordSink, inherited: bool) {
        let next = value
            .get("modelId")
            .or_else(|| value.get("model"))
            .and_then(Value::as_str)
            .and_then(bounded_provider_hint_value);
        if value.get("timestamp").and_then(parse_ts).is_none() || next.is_none() {
            self.current_model = None;
            self.current_provider = None;
            if !inherited {
                sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            }
            return;
        }
        if !inherited && self.model.is_none() {
            self.model = next.clone();
        }
        let provider = value
            .get("provider")
            .and_then(Value::as_str)
            .and_then(bounded_provider_hint_value);
        if let Some(provider) = provider {
            self.current_provider = Some(provider.clone());
            if !inherited {
                push_provider_hint(&mut self.provider_hints, &provider, next.as_deref());
            }
        }
        self.current_model = next;
    }

    fn observe_thinking_level_change(
        &mut self,
        value: &Value,
        sink: &mut dyn RecordSink,
        inherited: bool,
    ) {
        let next = value
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|level| {
                !level.is_empty() && level.len() <= crate::analysis::EVIDENCE_STRING_CAP
            })
            .map(str::to_owned);
        if value.get("timestamp").and_then(parse_ts).is_none() || next.is_none() {
            self.current_thinking_mode = None;
            if !inherited {
                sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            }
            return;
        }
        self.current_thinking_mode = next;
    }

    fn observe_compaction(
        &mut self,
        value: &Value,
        thread_id: Option<String>,
        sink: &mut dyn RecordSink,
    ) {
        let Some(timestamp) = value.get("timestamp").and_then(parse_ts) else {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            return;
        };
        let mut event = NormalizedEvent::new(Role::System);
        event.ts_ms = Some(timestamp);
        event.thread_id = thread_id;
        event.uuid = thread_identity_field(value, "id");
        event.parent_uuid = thread_identity_field(value, "parentId");
        event.is_compaction_boundary = true;
        event.compaction_pre_tokens = value.get("tokensBefore").and_then(Value::as_u64);
        event.model = self.current_model.clone();
        event.thinking_mode = self.current_thinking_mode.clone();
        self.emit_event(event, Vec::new(), sink);
    }

    fn emit_event(
        &mut self,
        event: NormalizedEvent,
        content_parts: Vec<ContentPart>,
        sink: &mut dyn RecordSink,
    ) {
        sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
        if !content_parts.is_empty() {
            sink.record(NormalizedRecord::TurnContent(Box::new(TurnContent {
                parts: content_parts,
            })));
        }
    }

    fn finish(self) -> SessionSummary {
        let mut coverage_gaps: Vec<PartialReason> = self
            .fork_attribution_incomplete
            .then_some(PartialReason::AttributionIncomplete)
            .into_iter()
            .collect();
        // A capped thread resolver means some records past the cap could not
        // be linked into their real thread: the same kind of attribution
        // loss the cache group's unresolved-parent-link check reports. See
        // `ClaudeStreamState::into_summary` in `claude.rs`.
        if self.threads.capped() {
            coverage_gaps.push(PartialReason::AttributionIncomplete);
        }
        if self.subagent_incomplete || self.subagent_calls.values().any(|call| !call.resolved) {
            coverage_gaps.push(PartialReason::AttributionIncomplete);
        }
        SessionSummary {
            cache_write_tokens_available: self.cache_write_tokens_available.unwrap_or(true),
            context_window: None,
            context_window_source: ContextWindowSource::Inferred,
            model: self.model,
            provider_hints: self.provider_hints,
            started_at_ms: self.started_at_ms,
            coverage_gaps,
            late_tools: Vec::new(),
            initial_context: None,
            skill_descriptions: HashMap::new(),
        }
    }
}

fn pi_subagent_id<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)?
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= crate::analysis::EVIDENCE_STRING_CAP)
}

// Pi permits invalid skill names with warnings. Retain only the bounded, path-free specification subset.
fn pi_skill_identity(name: &str) -> Option<&str> {
    (!name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
    .then_some(name)
}

/// This row's `ThreadLink` observation (Pi's `id` / `parentId`), when either
/// field is present. Mirrors `records::evidence_observations`'s Claude
/// `ThreadLink` emission (`uuid` / `parentUuid`) with Pi's own field names,
/// since that helper reads only Claude's shape and Pi has no `message`-
/// nested id to fall back on.
fn thread_link_observation(
    id: Option<&str>,
    parent_id: Option<&str>,
) -> Option<EvidenceObservation> {
    (id.is_some() || parent_id.is_some()).then(|| EvidenceObservation::ThreadLink {
        uuid: id.map(str::to_owned),
        parent_uuid: parent_id.map(str::to_owned),
    })
}

fn observe_inert(value: &Value, sink: &mut dyn RecordSink) {
    if value.get("timestamp").and_then(parse_ts).is_none() {
        sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
    }
}

fn unrecognized(discriminator: &str, sink: &mut dyn RecordSink) {
    sink.record(NormalizedRecord::Observation(Box::new(
        EvidenceObservation::UnrecognizedType {
            discriminator: discriminator.to_owned(),
            inert: false,
        },
    )));
    sink.record(NormalizedRecord::Unusable(
        PartialReason::UnrecognizedRecordType,
    ));
}

fn is_inert_shape(value: &Value) -> bool {
    let allowed_role =
        (value.get("type").and_then(Value::as_str) == Some("message")).then_some("bashExecution");
    !has_shared_parser_signal(value, allowed_role)
}

fn has_shared_parser_signal(value: &Value, allowed_role: Option<&str>) -> bool {
    let Some(row) = value.as_object() else {
        return false;
    };
    object_has_shared_parser_signal(row, None)
        || row
            .get("message")
            .and_then(Value::as_object)
            .is_some_and(|message| object_has_shared_parser_signal(message, allowed_role))
}

fn object_has_shared_parser_signal(
    object: &serde_json::Map<String, Value>,
    allowed_role: Option<&str>,
) -> bool {
    const SIGNAL_KEYS: &[&str] = &[
        "usage",
        "model",
        "modelId",
        "thinkingLevel",
        "thinking",
        "reasoning",
        "effort",
        "reasoning_effort",
        "reasoningEffort",
        "speed",
        "tool_calls",
        "toolCalls",
        "tool_use",
        "toolUse",
        "tool_result",
        "toolResult",
        "toolCallId",
        "toolName",
        "isError",
        "compactMetadata",
        "tokensBefore",
    ];
    if SIGNAL_KEYS.iter().any(|key| object.contains_key(*key)) {
        return true;
    }
    if object
        .get("role")
        .and_then(Value::as_str)
        .is_some_and(|role| Some(role) != allowed_role)
    {
        return true;
    }
    object
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks.iter().any(|block| {
                block
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| {
                        matches!(
                            kind,
                            "toolCall"
                                | "tool_use"
                                | "tool_result"
                                | "function_call"
                                | "function_call_output"
                                | "thinking"
                                | "reasoning"
                                | "compaction"
                                | "compact_boundary"
                        )
                    })
            })
        })
}

fn unknown_content_blocks(value: &Value) -> Vec<String> {
    value
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| {
            let block_type = block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("<missing>");
            (!matches!(block_type, "text" | "thinking" | "toolCall" | "image"))
                .then(|| block_type.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::*;
    use crate::analysis::evidence::{EvidenceSource, SourceCapabilities, SourceKind};
    use crate::analysis::evidence_sink::{EvidenceResumeState, SessionEvidenceAccumulator};
    use crate::analysis::interface::ContentKind;
    use crate::analysis::metrics_sink::SessionMetricsAccumulator;
    use crate::analysis::model::ToolCategory;
    use crate::analysis::resume::EvidenceSnapshot;
    use crate::analysis::source_validity::ResumePoint;
    use crate::analysis::{RESUME_SNAPSHOT_REVISION, SourceChangedReason};
    use crate::discovery::source_version::head_hash_of;
    use crate::discovery::{FingerprintInputs, SourceStat};
    use tempfile::TempDir;

    const FIRST_RECORD: &str = concat!(
        r#"{"type":"message","timestamp":"2026-01-01T00:00:00Z","message":{"role":"assistant","content":[{"type":"text","text":"first"}]}}"#,
        "\n",
    );
    const SECOND_RECORD: &str = concat!(
        r#"{"type":"message","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"second"}]}}"#,
        "\n",
    );

    fn file_input(path: &Path) -> SessionInput {
        SessionInput {
            agent: "pi".to_string(),
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

    /// A full [`StreamSnapshot`] around `resume` (the adapter's own half),
    /// with fresh metrics/evidence/index state. Mirrors
    /// `claude.rs`'s test helper of the same name.
    fn snapshot_from(resume: AdapterResume) -> StreamSnapshot {
        let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "pi".to_owned(),
            session_id: "claimed-session".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::pi(),
        });
        StreamSnapshot {
            revision: RESUME_SNAPSHOT_REVISION,
            resume: resume.point,
            adapter: resume.adapter,
            metrics: SessionMetricsAccumulator::new("pi", "claimed-session"),
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
            adapter: PiSessionReader::empty_adapter_snapshot(),
        })
    }

    #[derive(Default, Debug)]
    struct SubagentSink {
        events: Vec<NormalizedEvent>,
        observations: Vec<EvidenceObservation>,
        content: Vec<ContentPart>,
        gaps: Vec<PartialReason>,
    }

    impl RecordSink for SubagentSink {
        fn record(&mut self, record: NormalizedRecord) {
            match record {
                NormalizedRecord::MetricsEvent(event) => self.events.push(*event),
                NormalizedRecord::Observation(observation) => self.observations.push(*observation),
                NormalizedRecord::TurnContent(content) => self.content.extend(content.parts),
                _ => {}
            }
        }

        fn finish(&mut self, summary: SessionSummary) {
            self.gaps = summary.coverage_gaps;
        }
    }

    fn subagent_call() -> Value {
        json!({"type":"message","timestamp":1,"message":{
            "role":"assistant","model":"parent-model","usage":{"input":11},
            "content":[{"type":"toolCall","name":"subagent","id":"call-1",
                "arguments":{"task":"PRIVATE TASK","agent":"PRIVATE AGENT"}}]
        }})
    }

    fn subagent_result(mode: &str, workers: usize) -> Value {
        json!({"type":"message","timestamp":4,"message":{
            "role":"toolResult","toolName":"subagent","toolCallId":"call-1",
            "content":[{"type":"text","text":"PRIVATE RESULT"}],"details":{"mode":mode,"results":(0..workers).map(|index| json!({
                "model":"dispatch-alias","task":"PRIVATE TASK","exitCode":0,
                "usage":{"input":999999},
                "messages":[{"role":"user","content":"PRIVATE PROMPT"},
                    {"role":"assistant","provider":"anthropic","api":"anthropic-messages",
                     "model":format!("worker-{index}"),"timestamp":2,
                     "content":[{"type":"text","text":"PRIVATE OUTPUT"}],
                     "usage":{"input":3,"output":5,"cacheRead":7,"cacheWrite":2}},
                    {"role":"assistant","provider":"openai","model":"worker-switch","timestamp":3,
                     "usage":{"input":1,"output":2,"cacheRead":0,"cacheWrite":0}}]
            })).collect::<Vec<_>>()}
        }})
    }

    #[test]
    fn official_subagent_modes_join_actual_workers_without_parent_usage_or_policy_changes() {
        for (mode, workers) in [("single", 1), ("parallel", 3), ("chain", 2)] {
            let mut state = PiStreamState::default();
            let mut sink = SubagentSink::default();
            state.observe(subagent_call(), &mut sink);
            state.observe(subagent_result(mode, workers), &mut sink);
            state.observe(subagent_result(mode, workers), &mut sink);
            assert_eq!(state.current_model.as_deref(), Some("parent-model"));
            assert_eq!(state.model.as_deref(), Some("parent-model"));
            assert!(state.finish().coverage_gaps.is_empty());
            let delegated: Vec<_> = sink
                .events
                .iter()
                .filter(|event| event.source == crate::analysis::model::EventSource::Subagent)
                .collect();
            assert_eq!(delegated.len(), workers * 2);
            assert_eq!(
                sink.events
                    .iter()
                    .filter(|event| event.source == crate::analysis::model::EventSource::Parent)
                    .map(|event| event.usage.input_tokens)
                    .sum::<u64>(),
                11
            );
            for (index, turns) in delegated.chunks_exact(2).enumerate() {
                assert_eq!(turns[0].model, Some(format!("worker-{index}")));
                assert_eq!(turns[1].model.as_deref(), Some("worker-switch"));
                assert_eq!(turns[0].thread_id, Some(format!("pi-subagent:0:{index}")));
                assert_eq!(turns[0].thread_id, turns[1].thread_id);
                assert!(turns[0].uuid.is_none());
                assert_eq!(turns[0].ts_ms, Some(2));
                assert_eq!(turns[0].usage.input_tokens, 3);
                assert_eq!(turns[0].usage.output_tokens, 5);
                assert_eq!(turns[0].usage.cache_read_tokens, 7);
                assert_eq!(turns[0].usage.cache_creation_tokens, 2);
                assert_eq!(turns[0].provider.as_deref(), Some("anthropic"));
                let row = crate::analysis::rows::turn_row_from_event(turns[0], "parent", 0);
                assert_eq!(row.scope, crate::analysis::rows::TurnScope::Delegated);
                assert_eq!(row.child_id, turns[0].thread_id);
                assert_eq!(row.provider, turns[0].provider);
                assert_eq!(row.model, turns[0].model);
                assert_eq!(row.input_tokens, 3);
                assert!(row.content.is_empty());
            }
            assert_eq!(sink.observations.iter().filter(|observation| matches!(observation,
                EvidenceObservation::SubagentSpawn { parent_model: Some(model), .. } if model == "parent-model"
            )).count(), workers);
            assert!(!format!("{sink:?}").contains("PRIVATE"));
        }
    }

    #[test]
    fn official_subagent_join_keeps_dispatch_parent_after_a_model_change() {
        let mut state = PiStreamState::default();
        let mut sink = SubagentSink::default();
        state.observe(subagent_call(), &mut sink);
        state.observe(
            json!({"type":"model_change","timestamp":2,"modelId":"new-parent"}),
            &mut sink,
        );
        state.observe(subagent_result("single", 1), &mut sink);
        assert_eq!(state.current_model.as_deref(), Some("new-parent"));
        assert!(sink.observations.iter().any(|observation| matches!(observation,
            EvidenceObservation::SubagentSpawn { parent_model: Some(model), .. } if model == "parent-model"
        )));
    }

    #[test]
    fn official_subagent_missing_and_unrecognized_evidence_stays_partial() {
        let valid = subagent_result("single", 1);
        let mut cases = vec![subagent_result("unknown", 1), subagent_result("single", 0)];
        for pointer in [
            "/message/details/results/0/messages",
            "/message/details/results/0/messages/1/model",
            "/message/details/results/0/messages/1/usage",
            "/message/toolCallId",
            "/message/details",
        ] {
            let mut result = valid.clone();
            *result.pointer_mut(pointer).unwrap() = Value::Null;
            cases.push(result);
        }
        for result in cases {
            let mut state = PiStreamState::default();
            let mut sink = SubagentSink::default();
            state.observe(subagent_call(), &mut sink);
            state.observe(result, &mut sink);
            assert!(
                state
                    .finish()
                    .coverage_gaps
                    .contains(&PartialReason::AttributionIncomplete)
            );
            assert!(
                !sink
                    .events
                    .iter()
                    .any(|event| event.model.as_deref() == Some("dispatch-alias"))
            );
        }
        let mut state = PiStreamState::default();
        state.observe(subagent_call(), &mut SubagentSink::default());
        assert!(
            state
                .finish()
                .coverage_gaps
                .contains(&PartialReason::AttributionIncomplete)
        );
    }

    #[test]
    fn official_subagent_caps_bound_state_and_keep_partial() {
        let mut oversized = subagent_call();
        oversized["message"]["content"][0]["id"] =
            json!("x".repeat(crate::analysis::EVIDENCE_STRING_CAP + 1));
        let mut state = PiStreamState::default();
        state.observe(oversized, &mut SubagentSink::default());
        assert!(state.subagent_calls.is_empty());
        assert!(
            state
                .finish()
                .coverage_gaps
                .contains(&PartialReason::AttributionIncomplete)
        );
        let mut state = PiStreamState::default();
        let mut sink = SubagentSink::default();
        for index in 0..=MAX_SUBAGENT_CALLS {
            let mut call = subagent_call();
            call["message"]["content"][0]["id"] = json!(format!("call-{index}"));
            state.observe(call, &mut sink);
        }
        assert_eq!(state.subagent_calls.len(), MAX_SUBAGENT_CALLS);
        let serialized = postcard::to_allocvec(&state).unwrap();
        assert!(!serialized.windows(7).any(|bytes| bytes == b"PRIVATE"));
        assert!(
            state
                .finish()
                .coverage_gaps
                .contains(&PartialReason::AttributionIncomplete)
        );
        for result in [subagent_result("parallel", MAX_SUBAGENT_CHILDREN + 1), {
            let mut result = subagent_result("single", 1);
            let worker = result["message"]["details"]["results"][0]["messages"][1].clone();
            result["message"]["details"]["results"][0]["messages"] =
                json!(vec![worker; MAX_WORKER_MESSAGES + 1]);
            result
        }] {
            let mut state = PiStreamState::default();
            let mut sink = SubagentSink::default();
            state.observe(subagent_call(), &mut sink);
            state.observe(result, &mut sink);
            assert!(
                state
                    .finish()
                    .coverage_gaps
                    .contains(&PartialReason::AttributionIncomplete)
            );
        }
    }

    #[test]
    fn official_subagent_result_after_resume_matches_full_read() {
        let source = format!(
            "{}\n{}\n{}\n",
            subagent_call(),
            subagent_result("parallel", 2),
            subagent_result("parallel", 2)
        );
        let directory = TempDir::new().unwrap();
        let path = write_source(&directory, source.as_bytes());
        let input = file_input(&path);
        let mut full = SubagentSink::default();
        PiSessionReader.visit(&input, &mut full).unwrap();
        let mut offset = 0;
        for line in source.split_inclusive('\n') {
            offset += line.len();
            std::fs::write(&path, &source[..offset]).unwrap();
            let mut resumed = SubagentSink::default();
            let visit = PiSessionReader
                .visit_claimed_resumed(
                    &input,
                    &claim_for_path(&path),
                    &fresh_snapshot(),
                    &|| false,
                    &mut resumed,
                )
                .unwrap();
            OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(&source.as_bytes()[offset..])
                .unwrap();
            PiSessionReader
                .visit_claimed_resumed(
                    &input,
                    &claim_for_path(&path),
                    &snapshot_from(visit.resume.unwrap()),
                    &|| false,
                    &mut resumed,
                )
                .unwrap();
            assert_eq!(resumed.events, full.events);
            assert_eq!(resumed.observations, full.observations);
            assert_eq!(resumed.gaps, full.gaps);
        }
    }

    #[test]
    fn official_subagent_composite_resume_preserves_rows_metrics_and_evidence() {
        use std::sync::Arc;

        use crate::analysis::{
            CompositeSink, FenceScope, MemoryTurnRowStore, TurnRowSink, TurnRowStore,
            TurnSessionKey, query_turn_facts, query_turn_rows,
        };

        let directory = TempDir::new().unwrap();
        let path = write_source(&directory, b"");
        let input = file_input(&path);
        let store = MemoryTurnRowStore::new("pi", "claimed-session");
        let mut snapshot = fresh_snapshot();
        let mut source = String::new();
        let composite = |store: &Arc<MemoryTurnRowStore>, snapshot: &StreamSnapshot| {
            CompositeSink::with_turn_rows(
                SessionMetricsAccumulator::restore(snapshot.metrics.clone()),
                SessionEvidenceAccumulator::from_coverage_record_with_resume(
                    snapshot.evidence.record.clone(),
                    snapshot.evidence.resume.clone(),
                ),
                TurnRowSink::new(
                    Arc::clone(store) as Arc<dyn TurnRowStore>,
                    "claimed-session".to_owned(),
                    None,
                )
                .with_start_index(snapshot.next_turn_index),
            )
        };
        for row in [subagent_call(), subagent_result("parallel", 2)] {
            source.push_str(&format!("{row}\n"));
            std::fs::write(&path, &source).unwrap();
            let mut resumed = composite(&store, &snapshot);
            let visit = PiSessionReader
                .visit_claimed_resumed(
                    &input,
                    &claim_for_path(&path),
                    &snapshot,
                    &|| false,
                    &mut resumed,
                )
                .unwrap();
            resumed.observe_source_outcome(visit.outcome);
            let full_store = MemoryTurnRowStore::new("pi", "claimed-session");
            let mut full = composite(&full_store, &fresh_snapshot());
            let outcome = PiSessionReader
                .visit_claimed(
                    &input,
                    &claim_for_path(&path),
                    AppendOnlyGuarantee::Absent,
                    &|| false,
                    &mut full,
                )
                .unwrap();
            full.observe_source_outcome(outcome);
            let key = TurnSessionKey {
                environment_key: "native",
                agent: "pi",
                session_id: "claimed-session",
            };
            let rows_and_facts = |store: &MemoryTurnRowStore| {
                store.with_connection(|conn| {
                    (
                        query_turn_rows(conn, &key, &FenceScope::single(1)).unwrap(),
                        query_turn_facts(conn, &key, &FenceScope::single(1)).unwrap(),
                    )
                })
            };
            assert_eq!(rows_and_facts(&store), rows_and_facts(&full_store));
            assert_eq!(resumed.metrics().unwrap(), full.metrics().unwrap());
            assert_eq!(resumed.evidence().unwrap(), full.evidence().unwrap());
            assert_eq!(resumed.summary(), full.summary());
            store.with_connection(|conn| {
                let count: u64 = conn
                    .query_row("SELECT count(*) FROM turn_content", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(count, 0);
            });
            snapshot = resumed.snapshot(visit.resume.unwrap()).unwrap();
        }
    }

    #[test]
    fn branch_and_inherited_policies_follow_parents_with_resume_parity() {
        for (source, expected) in [
            (
                include_str!("../../../tests/fixtures/pi_characterization/branch_policy.jsonl"),
                vec![
                    (Some("model-b"), Some("high")),
                    (Some("model-a"), Some("low")),
                    (Some("model-a"), Some("low")),
                    (None, None),
                    (None, None),
                    (None, None),
                    (Some("model-b"), Some("high")),
                ],
            ),
            (
                include_str!(
                    "../../../tests/fixtures/pi_characterization/fork_inherited_policy.jsonl"
                ),
                vec![
                    (Some("model-a"), Some("low")),
                    (Some("model-b"), Some("high")),
                ],
            ),
        ] {
            let directory = TempDir::new().unwrap();
            let path = write_source(&directory, source.as_bytes());
            let input = file_input(&path);
            let full = PiSessionReader.normalize(&input).unwrap();
            assert_eq!(
                full.events
                    .iter()
                    .map(|event| (event.model.as_deref(), event.thinking_mode.as_deref()))
                    .collect::<Vec<_>>(),
                expected,
            );
            if source.contains("parentSession") {
                assert_eq!(
                    full.events
                        .iter()
                        .map(|event| event.usage.input_tokens)
                        .sum::<u64>(),
                    3
                );
                let mut sink = SummarySink::default();
                PiSessionReader.visit(&input, &mut sink).unwrap();
                assert!(sink.summary.unwrap().cache_write_tokens_available);
            }
            let mut offset = 0;
            for line in source.split_inclusive('\n') {
                offset += line.len();
                std::fs::write(&path, &source[..offset]).unwrap();
                let mut first = SessionCollector::new("pi", "claimed-session");
                let visit = PiSessionReader
                    .visit_claimed_resumed(
                        &input,
                        &claim_for_path(&path),
                        &fresh_snapshot(),
                        &|| false,
                        &mut first,
                    )
                    .unwrap();
                let mut events = first.into_session().unwrap().events;
                OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap()
                    .write_all(&source.as_bytes()[offset..])
                    .unwrap();
                let mut second = SessionCollector::new("pi", "claimed-session");
                PiSessionReader
                    .visit_claimed_resumed(
                        &input,
                        &claim_for_path(&path),
                        &snapshot_from(visit.resume.unwrap()),
                        &|| false,
                        &mut second,
                    )
                    .unwrap();
                events.extend(second.into_session().unwrap().events);
                assert_eq!(events, full.events, "resume offset {offset}");
            }
        }
    }

    #[test]
    fn policy_storage_stops_at_the_thread_cap_without_borrowing_untracked_policy() {
        let mut state = PiStreamState::default();
        let mut sink = SummarySink::default();
        state.observe(json!({"type":"thinking_level_change","id":"low","parentId":null,"timestamp":1,"thinkingLevel":"low"}), &mut sink);
        let mut index = 0;
        while !state.threads.capped() {
            state.observe(
                json!({"type":"custom","id":format!("row-{index}"),"parentId":"low","timestamp":2}),
                &mut sink,
            );
            index += 1;
            assert!(index <= 50_000);
        }
        let retained = state.policy_by_id.len();
        assert_eq!(retained, 50_000);
        let mut resumed: PiStreamState =
            postcard::from_bytes(&postcard::to_allocvec(&state).unwrap()).unwrap();
        for state in [&mut state, &mut resumed] {
            state.observe(json!({"type":"thinking_level_change","id":"untracked","parentId":"low","timestamp":3,"thinkingLevel":"max"}), &mut sink);
            assert_eq!(state.current_thinking_mode.as_deref(), Some("max"));
            state.observe(json!({"type":"message","id":"child","parentId":"untracked","timestamp":4,"message":{"role":"assistant","content":[]}}), &mut sink);
            assert_eq!(state.current_thinking_mode, None);
            state.observe(json!({"type":"message","id":"known-child","parentId":"low","timestamp":5,"message":{"role":"assistant","content":[]}}), &mut sink);
            assert_eq!(state.current_thinking_mode.as_deref(), Some("low"));
            assert_eq!(state.policy_by_id.len(), retained);
        }
        let gaps = state.finish().coverage_gaps;
        assert_eq!(gaps, vec![PartialReason::AttributionIncomplete]);
        assert_eq!(gaps, resumed.finish().coverage_gaps);
    }

    #[test]
    fn a_resumed_read_from_offset_zero_matches_a_full_read() {
        let directory = TempDir::new().expect("tempdir");
        let path = write_source(&directory, FIRST_RECORD.as_bytes());
        let claim = claim_for_path(&path);
        let input = file_input(&path);
        let mut collector = SessionCollector::new("pi", "claimed-session");

        let visit = PiSessionReader
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
        let mut first_pass = SessionCollector::new("pi", "claimed-session");
        let first_claim = claim_for_path(&path);
        let first_visit = PiSessionReader
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
        let mut second_pass = SessionCollector::new("pi", "claimed-session");

        let second_visit = PiSessionReader
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
    fn a_rewritten_tail_fails_a_resumed_read_without_a_snapshot() {
        let directory = TempDir::new().expect("tempdir");
        let path = write_source(&directory, FIRST_RECORD.as_bytes());
        let input = file_input(&path);
        let first_claim = claim_for_path(&path);
        let mut first_pass = SessionCollector::new("pi", "claimed-session");
        let first_visit = PiSessionReader
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
        let mut second_pass = SessionCollector::new("pi", "claimed-session");

        let visit = PiSessionReader
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

    #[derive(Default)]
    struct SummarySink {
        summary: Option<SessionSummary>,
    }

    impl RecordSink for SummarySink {
        fn record(&mut self, _record: NormalizedRecord) {}

        fn finish(&mut self, summary: SessionSummary) {
            self.summary = Some(summary);
        }
    }

    #[test]
    fn provider_hints_include_zero_token_messages_and_model_changes() {
        let content = concat!(
            r#"{"type":"model_change","timestamp":"2026-01-01T00:00:00Z","provider":"anthropic","modelId":"claude-sonnet"}"#,
            "\n",
            r#"{"type":"message","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","model":"claude-sonnet","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"content":[]}}"#,
            "\n",
            r#"{"type":"message","timestamp":"2026-01-01T00:00:02Z","message":{"role":"assistant","model":"gpt-5","provider":"openai-codex","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"content":[]}}"#,
            "\n"
        );
        let input = SessionInput {
            agent: "pi".to_owned(),
            session_id: "providers".to_owned(),
            source: RawSource::Jsonl(content.to_owned()),
            fork_parent_session_id: None,
        };
        let mut sink = SummarySink::default();

        PiSessionReader.visit(&input, &mut sink).unwrap();

        assert_eq!(
            sink.summary.unwrap().provider_hints,
            vec![
                ProviderHint {
                    provider: "anthropic".to_owned(),
                    model: Some("claude-sonnet".to_owned()),
                },
                ProviderHint {
                    provider: "openai-codex".to_owned(),
                    model: Some("gpt-5".to_owned()),
                },
            ]
        );
    }

    #[test]
    fn provider_hints_are_unique_and_bounded() {
        let long_provider = format!("{}é", "p".repeat(crate::analysis::EVIDENCE_STRING_CAP));
        let long_model = format!("{}é", "m".repeat(crate::analysis::EVIDENCE_STRING_CAP));
        let mut content = format!(
            "{{\"type\":\"model_change\",\"timestamp\":0,\"provider\":\"{long_provider}\",\"modelId\":\"{long_model}\"}}\n"
        );
        for index in 0..(crate::analysis::MAX_PROVIDER_HINTS + 10) {
            content.push_str(&format!(
                "{{\"type\":\"model_change\",\"timestamp\":{},\"provider\":\"provider-{index}\",\"modelId\":\"model-{index}\"}}\n",
                index + 1
            ));
        }
        let input = SessionInput {
            agent: "pi".to_owned(),
            session_id: "bounded-providers".to_owned(),
            source: RawSource::Jsonl(content),
            fork_parent_session_id: None,
        };
        let mut sink = SummarySink::default();

        PiSessionReader.visit(&input, &mut sink).unwrap();

        let hints = sink.summary.unwrap().provider_hints;
        assert_eq!(hints.len(), crate::analysis::MAX_PROVIDER_HINTS);
        assert_eq!(
            hints[0].provider,
            "p".repeat(crate::analysis::EVIDENCE_STRING_CAP)
        );
        assert_eq!(
            hints[0].model,
            Some("m".repeat(crate::analysis::EVIDENCE_STRING_CAP))
        );
        assert!(hints.iter().all(|hint| {
            hint.provider.len() <= crate::analysis::EVIDENCE_STRING_CAP
                && hint
                    .model
                    .as_ref()
                    .is_none_or(|model| model.len() <= crate::analysis::EVIDENCE_STRING_CAP)
        }));
    }

    #[test]
    fn retained_provider_and_model_state_is_utf8_safely_bounded() {
        let long_provider = format!("{}é", "p".repeat(crate::analysis::EVIDENCE_STRING_CAP));
        let long_model = format!("{}é", "m".repeat(crate::analysis::EVIDENCE_STRING_CAP));
        let mut state = PiStreamState::default();
        let mut sink = SummarySink::default();

        state.observe(
            serde_json::json!({
                "type": "message",
                "timestamp": 1,
                "message": {
                    "role": "assistant",
                    "provider": long_provider,
                    "model": long_model,
                    "usage": {},
                    "content": []
                }
            }),
            &mut sink,
        );

        assert_eq!(
            state.current_provider.as_deref(),
            Some("p".repeat(crate::analysis::EVIDENCE_STRING_CAP).as_str())
        );
        assert_eq!(
            state.current_model.as_deref(),
            Some("m".repeat(crate::analysis::EVIDENCE_STRING_CAP).as_str())
        );
    }

    #[test]
    fn invalid_thinking_changes_clear_the_previous_policy_and_bound_resume_state() {
        for invalid in [
            Value::Null,
            json!(""),
            json!("x".repeat(crate::analysis::EVIDENCE_STRING_CAP + 1)),
        ] {
            let mut state = PiStreamState::default();
            let mut sink = SummarySink::default();
            state.observe(
                json!({"type": "thinking_level_change", "timestamp": 1, "thinkingLevel": "low"}),
                &mut sink,
            );
            state.observe(
                json!({"type": "thinking_level_change", "timestamp": 2, "thinkingLevel": invalid}),
                &mut sink,
            );
            assert_eq!(state.current_thinking_mode, None);
        }
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
    fn content_capture_maps_text_thinking_tool_call_and_tool_result() {
        let assistant_record = json!({
            "type": "message",
            "timestamp": "2026-01-01T00:00:01Z",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "hello there"},
                    {"type": "thinking", "thinking": "pondering"},
                    {"type": "toolCall", "id": "call-1", "name": "bash", "arguments": {"command": "ls"}},
                ]
            }
        })
        .to_string();
        let tool_result_record = json!({
            "type": "message",
            "timestamp": "2026-01-01T00:00:02Z",
            "message": {
                "role": "toolResult",
                "toolCallId": "call-1",
                "toolName": "bash",
                "content": [{"type": "text", "text": "ok"}]
            }
        })
        .to_string();
        let input = SessionInput {
            agent: "pi".to_string(),
            session_id: "content-session".to_string(),
            source: RawSource::Jsonl(format!("{assistant_record}\n{tool_result_record}\n")),
            fork_parent_session_id: None,
        };
        let mut sink = ContentCapturingSink::default();

        PiSessionReader
            .visit(&input, &mut sink)
            .expect("visit content session");

        assert_eq!(sink.contents.len(), 2, "one TurnContent per turn");
        let assistant_parts = &sink.contents[0].parts;
        assert_eq!(assistant_parts.len(), 3);
        assert_eq!(assistant_parts[0].kind, ContentKind::AssistantText);
        assert_eq!(assistant_parts[0].text, "hello there");
        assert_eq!(assistant_parts[1].kind, ContentKind::Thinking);
        assert_eq!(assistant_parts[1].text, "pondering");
        assert_eq!(assistant_parts[2].kind, ContentKind::ToolInput);
        assert_eq!(assistant_parts[2].text, r#"{"command":"ls"}"#);

        let tool_result_parts = &sink.contents[1].parts;
        assert_eq!(tool_result_parts.len(), 1);
        assert_eq!(tool_result_parts[0].kind, ContentKind::ToolResult);
        assert_eq!(tool_result_parts[0].text, "ok");
    }

    #[test]
    fn inert_shape_checks_only_shared_parser_locations() {
        let nested_extension = json!({
            "type": "custom_message",
            "timestamp": "2026-01-01T00:00:00Z",
            "data": {
                "content": [{"type": "thinking"}],
                "details": {"usage": {"input": 1}},
                "display": {"model": "extension-model"}
            }
        });
        assert!(is_inert_shape(&nested_extension));

        for key in [
            "usage",
            "model",
            "modelId",
            "thinkingLevel",
            "reasoning",
            "toolCalls",
            "toolResult",
            "compactMetadata",
            "tokensBefore",
        ] {
            let top_level = json!({
                "type": "custom",
                "timestamp": "2026-01-01T00:00:00Z",
                (key): true,
            });
            assert!(!is_inert_shape(&top_level), "top-level signal {key}");

            let message = json!({
                "type": "custom_message",
                "timestamp": "2026-01-01T00:00:00Z",
                "message": {(key): true},
            });
            assert!(!is_inert_shape(&message), "message signal {key}");
        }
    }

    #[test]
    fn bash_execution_is_an_explicit_usage_free_housekeeping_role() {
        let inert = json!({
            "type": "message",
            "timestamp": "2026-01-01T00:00:00Z",
            "message": {
                "role": "bashExecution",
                "command": "synthetic command",
                "output": "synthetic output"
            }
        });
        assert!(is_inert_shape(&inert));

        let evidence_bearing = json!({
            "type": "message",
            "timestamp": "2026-01-01T00:00:00Z",
            "message": {
                "role": "bashExecution",
                "usage": {"input": 1}
            }
        });
        assert!(!is_inert_shape(&evidence_bearing));
    }

    #[test]
    fn pi_tool_call_content_block_yields_named_tool() {
        let record = json!({
            "type": "message",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "toolCall",
                    "id": "call_1",
                    "name": "read",
                    "arguments": {"path": "src/lib.rs"}
                }]
            }
        });

        let ev = parse_record(&record, RecordShape::Pi).expect("record should parse");
        assert_eq!(ev.role, Role::Assistant);
        assert_eq!(ev.tools.len(), 1);
        assert_eq!(ev.tools[0].name, "read");
        assert_eq!(ev.tools[0].category, ToolCategory::Read);
    }

    #[test]
    fn pi_tool_call_arguments_feed_bash_test_classification() {
        let record = json!({
            "type": "message",
            "message": {
                "role": "assistant",
                "content": [{
                    "type": "toolCall",
                    "id": "call_1",
                    "name": "bash",
                    "arguments": {"command": "cargo test --workspace"}
                }]
            }
        });

        let ev = parse_record(&record, RecordShape::Pi).expect("record should parse");
        assert_eq!(ev.tools.len(), 1);
        assert_eq!(ev.tools[0].category, ToolCategory::Test);
    }

    #[test]
    fn pi_tool_result_message_role_is_parsed() {
        let errored = json!({
            "type": "message",
            "message": {
                "role": "toolResult",
                "toolCallId": "call_1",
                "toolName": "bash",
                "isError": true,
                "content": [{"type": "text", "text": "boom"}]
            }
        });
        let ok = json!({
            "type": "message",
            "message": {
                "role": "toolResult",
                "toolCallId": "call_2",
                "toolName": "bash",
                "content": [{"type": "text", "text": "ok"}]
            }
        });

        let errored = parse_record(&errored, RecordShape::Pi).expect("tool result should parse");
        assert_eq!(errored.role, Role::Tool);

        let ok = parse_record(&ok, RecordShape::Pi).expect("tool result should parse");
        assert_eq!(ok.role, Role::Tool);
    }
}
