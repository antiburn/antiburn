//! Pi v3 JSONL adapter.
//!
//! Pi writes one record per line. Semantic rows are `message`, `model_change`,
//! `thinking_level_change`, and `compaction`. The adapter recognizes the
//! `session` and `session_info` housekeeping rows by shape. It also recognizes
//! inert `custom` and `custom_message` records only when shared parser fields
//! cannot carry analysis signals. It recognizes `bashExecution` as a Pi
//! housekeeping role under the same rule.
//!
//! The top-level row timestamp controls ordering. Usage contains four disjoint
//! buckets: `input`, `output`, `cacheRead`, and `cacheWrite`. Extra usage fields
//! do not contribute to accounting. A linked child excludes rows whose
//! timestamps precede the session header. The adapter checks only parent-link
//! key presence and never reads the private path value.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read};

use anyhow::Context;
use serde_json::Value;

use crate::analysis::framing::{BoundedJsonlReader, FramedRecord, PartialReason, RecordSkip};
use crate::analysis::interface::{
    ContentPart, EvidenceObservation, NormalizedRecord, ProviderHint, RawSource, RecordSink,
    SessionCollector, SessionInput, SessionSummary, TurnContent, VendorAdapter, VisitOutcome,
    bounded_provider_hint_value, push_provider_hint,
};
use crate::analysis::model::{NormalizedEvent, NormalizedSession, Role};
use crate::analysis::records::{
    RecordShape, extract_content_parts, parse_record, parse_ts, thread_identity_field,
};
use crate::analysis::source_validity::{AppendOnlyGuarantee, PinnedSource, SourceClaim};
use crate::analysis::threads::ThreadResolver;

/// Parses Pi transcript files without retaining transcript content.
pub struct PiAdapter;

impl VendorAdapter for PiAdapter {
    fn agent(&self) -> &'static str {
        "pi"
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
                    self.visit_reader(BufReader::new(File::open(path)?), &|| false, sink)?
                }
                RawSource::Jsonl(content) => {
                    let suffix: &[u8] = if content.ends_with('\n') { b"" } else { b"\n" };
                    let source = Cursor::new(content.as_bytes()).chain(suffix);
                    self.visit_reader(BufReader::new(source), &|| false, sink)?
                }
                RawSource::Sqlite(_) => {
                    anyhow::bail!("sqlite source must be handled by the sqlite adapter")
                }
            };
            sink.finish(summary);
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
        PiAdapter::visit_claimed(self, input, claim, guarantee, cancel, sink)
    }
}

impl PiAdapter {
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
        .context("reading claimed Pi session")
    }

    fn visit_reader(
        &self,
        reader: impl BufRead,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<SessionSummary> {
        let mut reader = BoundedJsonlReader::new(reader);
        let mut state = PiStreamState::default();

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

        Ok(state.finish())
    }
}

#[derive(Default)]
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
}

impl PiStreamState {
    fn observe(&mut self, value: Value, sink: &mut dyn RecordSink) {
        let row_type = value.get("type").and_then(Value::as_str);
        // Every row carrying `id` / `parentId` joins the thread chain, in
        // file order, before the row-type dispatch below — including a row
        // about to be dropped as inherited. That keeps an owned row's
        // `parentId` resolving to a seen id even when the row it names was
        // itself inherited, so a fork file stays one thread. A row with no
        // `id` resolves to the resolver's current thread (rule c) without
        // recording anything new.
        let id = thread_identity_field(&value, "id");
        let parent_id = thread_identity_field(&value, "parentId");
        let thread_id = self.threads.resolve(id.as_deref(), parent_id.as_deref());
        if let Some(observation) = thread_link_observation(id.as_deref(), parent_id.as_deref()) {
            sink.record(NormalizedRecord::Observation(Box::new(observation)));
        }
        if row_type != Some("session") && self.fork_header_present {
            match (
                self.fork_start_ms,
                value.get("timestamp").and_then(parse_ts),
            ) {
                (Some(start_ms), Some(ts_ms)) if ts_ms < start_ms => {
                    sink.record(NormalizedRecord::Observation(Box::new(
                        EvidenceObservation::InheritedRecord,
                    )));
                    return;
                }
                (Some(_), Some(_)) => {}
                _ => {
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
            Some("session") if is_inert_shape(&value) => self.observe_session(&value, sink),
            Some("message") => self.observe_message(&value, thread_id, sink),
            Some("model_change") => self.observe_model_change(&value, sink),
            Some("thinking_level_change") => self.observe_thinking_level_change(&value, sink),
            Some("compaction") => self.observe_compaction(&value, thread_id, sink),
            Some("session_info") if is_inert_shape(&value) => observe_inert(&value, sink),
            Some("custom" | "custom_message") if is_inert_shape(&value) => {
                observe_inert(&value, sink)
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
        event.speed = None;
        event.thread_id = thread_id;

        if role == "assistant" {
            self.observe_assistant_metadata(value);
            event.model = event.model.or_else(|| self.current_model.clone());
            if let Some(provider) = value
                .pointer("/message/provider")
                .and_then(Value::as_str)
                .or(self.current_provider.as_deref())
            {
                push_provider_hint(&mut self.provider_hints, provider, event.model.as_deref());
            }
            event.thinking_mode = self.current_thinking_mode.clone();
        }
        for tool in &mut event.tools {
            if tool.name.eq_ignore_ascii_case("skill") {
                tool.detail = None;
            }
        }

        let unknown_blocks = unknown_content_blocks(value);
        let content_parts = extract_content_parts(value, event.role);
        self.emit_event(event, content_parts, sink);
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

    fn observe_assistant_metadata(&mut self, value: &Value) {
        if let Some(model) = value
            .pointer("/message/model")
            .and_then(Value::as_str)
            .and_then(bounded_provider_hint_value)
        {
            if self.model.is_none() {
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

        if let Some(api) = value
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

    fn observe_model_change(&mut self, value: &Value, sink: &mut dyn RecordSink) {
        let next = value
            .get("modelId")
            .or_else(|| value.get("model"))
            .and_then(Value::as_str)
            .and_then(bounded_provider_hint_value);
        if value.get("timestamp").and_then(parse_ts).is_none() || next.is_none() {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            return;
        }
        if self.model.is_none() {
            self.model = next.clone();
        }
        let provider = value
            .get("provider")
            .and_then(Value::as_str)
            .and_then(bounded_provider_hint_value);
        if let Some(provider) = provider {
            self.current_provider = Some(provider.clone());
            push_provider_hint(&mut self.provider_hints, &provider, next.as_deref());
        }
        self.current_model = next;
    }

    fn observe_thinking_level_change(&mut self, value: &Value, sink: &mut dyn RecordSink) {
        let next = value
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|level| !level.is_empty())
            .map(str::to_owned);
        if value.get("timestamp").and_then(parse_ts).is_none() || next.is_none() {
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
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
        SessionSummary {
            cache_write_tokens_available: self.cache_write_tokens_available.unwrap_or(true),
            context_window: None,
            model: self.model.or(self.current_model),
            provider_hints: self.provider_hints,
            started_at_ms: self.started_at_ms,
            coverage_gaps,
            late_tools: Vec::new(),
            initial_context: None,
            skill_descriptions: HashMap::new(),
        }
    }
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
    use serde_json::json;

    use super::*;
    use crate::analysis::interface::ContentKind;
    use crate::analysis::model::ToolCategory;

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
        };
        let mut sink = SummarySink::default();

        PiAdapter.visit(&input, &mut sink).unwrap();

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
        };
        let mut sink = SummarySink::default();

        PiAdapter.visit(&input, &mut sink).unwrap();

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
        };
        let mut sink = ContentCapturingSink::default();

        PiAdapter
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
