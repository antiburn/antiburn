//! Amp full-export v39 reader.
//!
//! This reader accepts explicit whole-thread exports only. File-change records
//! and live CLI state do not satisfy this contract and remain unavailable.

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::analysis::framing::MAX_RECORD_BYTES;
use crate::analysis::interface::{
    EvidenceObservation, NormalizedRecord, RecordSink, SessionCollector, SessionInput,
    SessionReader, SessionSummary, VisitOutcome,
};
use crate::analysis::model::{NormalizedEvent, Role, ToolCall, Usage};
use crate::analysis::records::parse_ts;
use crate::analysis::source_validity::{AppendOnlyGuarantee, PinnedSource, SourceClaim};
use crate::analysis::{SourceCapabilities, SourceFormat};

pub struct AmpSessionReader;

impl SessionReader for AmpSessionReader {
    fn agent(&self) -> &'static str {
        "amp-code"
    }

    fn capabilities(&self, input: &SessionInput) -> SourceCapabilities {
        if input.source_format != SourceFormat::AmpThreadJson {
            return SourceCapabilities::uncharacterized(input.source_format);
        }
        SourceCapabilities {
            source_format: SourceFormat::AmpThreadJson,
            request_context_tokens: true,
            timestamps_and_order: true,
            model_identity: true,
            token_classes: true,
            tool_invocations: true,
            ..SourceCapabilities::generic()
        }
    }

    fn normalize(
        &self,
        input: &SessionInput,
    ) -> anyhow::Result<crate::analysis::NormalizedSession> {
        let mut collector = SessionCollector::new(input.agent.clone(), input.session_id.clone());
        self.visit(input, &mut collector)?;
        collector.into_session()
    }

    fn visit(
        &self,
        input: &SessionInput,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        if input.source_format != SourceFormat::AmpThreadJson {
            anyhow::bail!("Amp reader requires AmpThreadJson admission");
        }
        let value = match &input.source {
            crate::analysis::RawSource::File(path) => read_json(path)?,
            crate::analysis::RawSource::Jsonl(content) => {
                if content.len() > MAX_RECORD_BYTES {
                    anyhow::bail!("Amp export is too large");
                }
                serde_json::from_str(content)?
            }
            _ => anyhow::bail!("Amp full export must be a file or explicit export"),
        };
        let summary = parse_export(&value, &input.session_id, sink)?;
        sink.finish(summary);
        Ok(VisitOutcome::Unvalidated)
    }

    fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        _guarantee: AppendOnlyGuarantee,
        _cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        let crate::analysis::RawSource::File(path) = &input.source else {
            return self.visit(input, sink);
        };
        let mut pinned = match PinnedSource::open(path, claim.clone())? {
            Ok(pinned) => pinned,
            Err(reason) => return Ok(VisitOutcome::SourceChanged(reason)),
        };
        let value = read_json_reader(pinned.reader((MAX_RECORD_BYTES + 1) as u64))?;
        let summary = parse_export(&value, &input.session_id, sink)?;
        if let Some(reason) = pinned.recheck_full()? {
            return Ok(VisitOutcome::SourceChanged(reason));
        }
        sink.finish(summary);
        Ok(VisitOutcome::AcceptedFull)
    }
}

fn parse_export(
    value: &Value,
    session_id: &str,
    sink: &mut dyn RecordSink,
) -> anyhow::Result<SessionSummary> {
    if value.get("version").and_then(Value::as_u64) != Some(39)
        || value.get("threadId").and_then(Value::as_str) != Some(session_id)
    {
        anyhow::bail!("unsupported Amp export envelope");
    }
    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Amp export has no messages"))?;
    let mut summary = SessionSummary::default();
    let mut assistant_usage_ids = HashSet::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str);
        let usage = message.get("usage");
        if usage.is_some() && role != Some("assistant") {
            anyhow::bail!("Amp usage is attached to a non-assistant message");
        }
        if role != Some("assistant") {
            continue;
        }
        let message_id = message
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Amp assistant message has no stable id"))?;
        if usage.is_some() && !assistant_usage_ids.insert(message_id) {
            anyhow::bail!("Amp assistant usage record is duplicated");
        }
        let model = message
            .get("model")
            .and_then(Value::as_str)
            .filter(|model| !model.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Amp assistant message has no model"))?;
        let timestamp = message
            .get("timestamp")
            .and_then(parse_ts)
            .ok_or_else(|| anyhow::anyhow!("Amp assistant message has no timestamp"))?;
        summary.started_at_ms.get_or_insert(timestamp);
        let (usage, total_input_tokens, max_input_tokens) = parse_usage(usage)?;
        if total_input_tokens != usage.context_tokens() {
            anyhow::bail!("Amp total input tokens do not reconcile");
        }
        summary.context_window = Some(max_input_tokens);
        for feature in message
            .get("features")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = feature
                .as_str()
                .or_else(|| feature.get("name").and_then(Value::as_str))
                .filter(|name| !name.is_empty())
                .ok_or_else(|| anyhow::anyhow!("invalid Amp feature identity"))?;
            sink.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::SkillInjection {
                    name: name.to_owned(),
                    invoked: true,
                },
            )));
        }
        let tools = message
            .get("toolCalls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .map(ToolCall::new)
                    .ok_or_else(|| anyhow::anyhow!("invalid Amp tool identity"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        sink.record(NormalizedRecord::MetricsEvent(Box::new(NormalizedEvent {
            ts_ms: Some(timestamp),
            usage_ts_ms: Some(timestamp),
            role: Role::Assistant,
            source: Default::default(),
            usage,
            tools,
            model: Some(model.to_owned()),
            provider: None,
            api: None,
            thinking_mode: None,
            speed: None,
            has_thinking: false,
            message_id: Some(message_id.to_owned()),
            is_compaction_boundary: false,
            compaction_trigger: None,
            compaction_pre_tokens: None,
            compaction_post_tokens: None,
            wrapper_tool: None,
            may_resolve_late_tool: false,
            late_tool_candidate_is_builtin: false,
            uuid: None,
            parent_uuid: None,
            logical_parent_uuid: None,
            thread_id: None,
        })));
    }
    Ok(summary)
}

fn parse_usage(value: Option<&Value>) -> anyhow::Result<(Usage, u64, u64)> {
    let value = value.ok_or_else(|| anyhow::anyhow!("Amp assistant message has no usage"))?;
    let input_tokens = metric(value, "inputTokens")?;
    let cache_creation_tokens = metric(value, "cacheCreationTokens")?;
    let usage = Usage {
        input_tokens: input_tokens
            .checked_sub(cache_creation_tokens)
            .ok_or_else(|| anyhow::anyhow!("Amp cache creation exceeds input tokens"))?,
        output_tokens: metric(value, "outputTokens")?,
        cache_read_tokens: metric(value, "cacheReadTokens")?,
        cache_creation_tokens,
        cache_creation_1h_tokens: 0,
    };
    Ok((
        usage,
        metric(value, "totalInputTokens")?,
        metric(value, "maxInputTokens")?,
    ))
}

fn metric(value: &Value, key: &str) -> anyhow::Result<u64> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("Amp usage is missing {key}"))
}

fn read_json(path: &Path) -> anyhow::Result<Value> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_RECORD_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_RECORD_BYTES {
        anyhow::bail!("Amp export is too large");
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn read_json_reader(mut reader: impl Read) -> anyhow::Result<Value> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    if bytes.len() > MAX_RECORD_BYTES {
        anyhow::bail!("Amp export is too large");
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{RawSource, SourceFormat};

    fn input(content: &str) -> SessionInput {
        SessionInput {
            agent: "amp-code".to_owned(),
            session_id: "T-synthetic-39".to_owned(),
            source: RawSource::Jsonl(content.to_owned()),
            source_format: SourceFormat::AmpThreadJson,
            fork_parent_session_id: None,
        }
    }

    #[test]
    fn v39_export_retains_main_usage_tools_skills_and_context_window() {
        let content = include_str!("../../../tests/fixtures/source_contracts/amp_export_v39.json");
        let mut sink = SessionCollector::new("amp-code", "T-synthetic-39");
        AmpSessionReader.visit(&input(content), &mut sink).unwrap();
        let session = sink.into_session().unwrap();
        assert_eq!(session.events.len(), 1);
        assert_eq!(
            session.events[0].message_id.as_deref(),
            Some("msg-synthetic-1")
        );
        assert_eq!(session.events[0].usage.input_tokens, 11);
        assert_eq!(session.events[0].usage.cache_creation_tokens, 1);
    }

    #[test]
    fn unsupported_version_and_file_changes_fail_closed() {
        let content = include_str!("../../../tests/fixtures/source_contracts/amp_export_v39.json")
            .replace("\"version\":39", "\"version\":38");
        let mut sink = SessionCollector::new("amp-code", "T-synthetic-39");
        assert!(AmpSessionReader.visit(&input(&content), &mut sink).is_err());
    }

    #[test]
    fn invalid_totals_and_oversized_exports_fail_closed() {
        for (field, replacement) in [
            ("\"totalInputTokens\":14", "\"totalInputTokens\":15"),
            ("\"cacheCreationTokens\":1", "\"cacheCreationTokens\":13"),
        ] {
            let content =
                include_str!("../../../tests/fixtures/source_contracts/amp_export_v39.json")
                    .replace(field, replacement);
            let mut sink = SessionCollector::new("amp-code", "T-synthetic-39");
            assert!(AmpSessionReader.visit(&input(&content), &mut sink).is_err());
        }

        let oversized = "{".to_owned() + &" ".repeat(MAX_RECORD_BYTES) + "}";
        let mut sink = SessionCollector::new("amp-code", "T-synthetic-39");
        assert!(
            AmpSessionReader
                .visit(&input(&oversized), &mut sink)
                .is_err()
        );

        let file_changes = SessionInput {
            source: RawSource::File("file-changes.json".into()),
            source_format: SourceFormat::AmpFileChanges,
            ..input("")
        };
        let mut sink = SessionCollector::new("amp-code", "T-synthetic-39");
        assert!(AmpSessionReader.visit(&file_changes, &mut sink).is_err());
    }

    #[test]
    fn duplicate_usage_and_missing_identity_fail_closed() {
        let content = include_str!("../../../tests/fixtures/source_contracts/amp_export_v39.json")
            .replace(
                "}],\"tools\"",
                "},{\"role\":\"assistant\",\"id\":\"msg-synthetic-1\",\"timestamp\":\"2026-09-01T10:01:00Z\",\"model\":\"synthetic-model\",\"usage\":{\"inputTokens\":12,\"outputTokens\":4,\"cacheReadTokens\":2,\"cacheCreationTokens\":1,\"totalInputTokens\":14,\"maxInputTokens\":32}}],\"tools\"",
            );
        let mut sink = SessionCollector::new("amp-code", "T-synthetic-39");
        assert!(AmpSessionReader.visit(&input(&content), &mut sink).is_err());

        let missing_id =
            include_str!("../../../tests/fixtures/source_contracts/amp_export_v39.json")
                .replace("\"id\":\"msg-synthetic-1\",", "");
        let mut sink = SessionCollector::new("amp-code", "T-synthetic-39");
        assert!(
            AmpSessionReader
                .visit(&input(&missing_id), &mut sink)
                .is_err()
        );
    }
}
