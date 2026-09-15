//! Copilot CLI v1 `events.jsonl` adapter.
//!
//! The accepted source is one persisted event chain under
//! `session-state/<uuid>/events.jsonl`. The reader intentionally does not read
//! message content, reasoning, tool arguments, or tool results.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor};
use std::path::Path;

use serde_json::Value;

use super::generic_jsonl::GenericJsonlSessionReader;
use crate::analysis::framing::{BoundedJsonlReader, FramedRecord, PartialReason};
use crate::analysis::interface::{
    EvidenceObservation, NormalizedRecord, RawSource, RecordSink, RelationProvenance,
    SessionCollector, SessionInput, SessionReader, SessionSummary, VisitOutcome,
};
use crate::analysis::model::{NormalizedEvent, NormalizedSession, Role, Usage};
use crate::analysis::records::parse_ts;
use crate::analysis::source_validity::{AppendOnlyGuarantee, PinnedSource, SourceClaim};
use crate::analysis::{SourceCapabilities, SourceFormat};

/// Parses the public Copilot SDK v1 event envelope persisted by Copilot CLI.
pub struct CopilotSessionReader;

impl SessionReader for CopilotSessionReader {
    fn agent(&self) -> &'static str {
        "copilot"
    }

    fn capabilities(&self, input: &SessionInput) -> SourceCapabilities {
        if input.source_format_or(SourceFormat::CopilotCliJsonl) != SourceFormat::CopilotCliJsonl {
            return SourceCapabilities::uncharacterized(input.source_format);
        }
        SourceCapabilities {
            source_format: SourceFormat::CopilotCliJsonl,
            timestamps_and_order: true,
            model_identity: true,
            token_classes: true,
            subagent_relationships: true,
            subagent_models: true,
            ..SourceCapabilities::generic()
        }
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
        if input.source_format_or(SourceFormat::CopilotCliJsonl) != SourceFormat::CopilotCliJsonl {
            return GenericJsonlSessionReader.visit(input, sink);
        }
        let state = match &input.source {
            RawSource::File(path) => {
                self.visit_reader(BufReader::new(File::open(path)?), input, &|| false, sink)?
            }
            RawSource::Jsonl(content) => {
                self.visit_reader(BufReader::new(Cursor::new(content)), input, &|| false, sink)?
            }
            RawSource::Sqlite(_) => anyhow::bail!("Copilot CLI source must be JSONL"),
            RawSource::ClineBundle { .. } => anyhow::bail!("Copilot CLI source must be JSONL"),
            RawSource::KiroCliV2Bundle { .. } => anyhow::bail!("Copilot CLI source must be JSONL"),
        };
        sink.finish(state.finish());
        Ok(VisitOutcome::Unvalidated)
    }

    fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        guarantee: AppendOnlyGuarantee,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        if input.source_format_or(SourceFormat::CopilotCliJsonl) != SourceFormat::CopilotCliJsonl {
            return GenericJsonlSessionReader.visit_claimed(input, claim, guarantee, cancel, sink);
        }
        let RawSource::File(path) = &input.source else {
            anyhow::bail!("a claimed Copilot source must be a file");
        };
        let mut pinned = match PinnedSource::open(path, claim.clone())? {
            Ok(pinned) => pinned,
            Err(reason) => return Ok(VisitOutcome::SourceChanged(reason)),
        };
        let limit = match guarantee {
            AppendOnlyGuarantee::Evidenced => claim.boundary,
            AppendOnlyGuarantee::Absent => u64::MAX,
        };
        let state = self.visit_reader(BufReader::new(pinned.reader(limit)), input, cancel, sink)?;
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
            sink.finish(state.finish());
        }
        Ok(outcome)
    }
}

impl CopilotSessionReader {
    fn visit_reader<R: BufRead>(
        &self,
        reader: R,
        input: &SessionInput,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<CopilotState> {
        if input.source_format_or(SourceFormat::CopilotCliJsonl) != SourceFormat::CopilotCliJsonl {
            anyhow::bail!("Copilot CLI reader requires CopilotCliJsonl admission");
        }
        let directory_id = match &input.source {
            RawSource::File(path) => cli_directory_id(path).map(str::to_owned),
            RawSource::Jsonl(_) => Some(input.session_id.clone()),
            RawSource::Sqlite(_) => None,
            RawSource::ClineBundle { .. } => None,
            RawSource::KiroCliV2Bundle { .. } => None,
        };
        let mut state = CopilotState {
            directory_id,
            ..CopilotState::default()
        };
        let mut framed = BoundedJsonlReader::new(reader);
        while let Some(record) = framed.next_record(cancel) {
            match record {
                FramedRecord::Skipped(skip) => {
                    sink.record(NormalizedRecord::Unusable(skip.partial_reason()))
                }
                FramedRecord::Complete { bytes, .. } => {
                    match serde_json::from_slice::<Value>(bytes) {
                        Ok(value) => state.observe(value, sink),
                        Err(_) => {
                            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord))
                        }
                    }
                }
            }
        }
        Ok(state)
    }
}

#[derive(Default)]
struct CopilotState {
    directory_id: Option<String>,
    previous_event_id: Option<String>,
    started: bool,
    shutdown: bool,
    invalid: bool,
    current_model: Option<String>,
    subagents: HashMap<String, Option<String>>,
    started_at_ms: Option<i64>,
}

impl CopilotState {
    fn observe(&mut self, value: Value, sink: &mut dyn RecordSink) {
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            return self.invalid(sink);
        };
        let Some(id) = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| is_uuid(id))
        else {
            return self.invalid(sink);
        };
        let Some(timestamp) = value.get("timestamp").and_then(parse_ts) else {
            return self.invalid(sink);
        };
        let parent = value.get("parentId");
        if (!self.started && (kind != "session.start" || parent != Some(&Value::Null)))
            || (self.started && parent.and_then(Value::as_str) != self.previous_event_id.as_deref())
            || self.shutdown
        {
            return self.invalid(sink);
        }
        self.previous_event_id = Some(id.to_owned());
        match kind {
            "session.start" => {
                let data = value.get("data");
                let Some(session_id) = data
                    .and_then(|data| data.get("sessionId"))
                    .and_then(Value::as_str)
                    .filter(|id| is_uuid(id))
                else {
                    return self.invalid(sink);
                };
                if data
                    .and_then(|data| data.get("version"))
                    .and_then(Value::as_u64)
                    != Some(1)
                    || self
                        .directory_id
                        .as_deref()
                        .is_some_and(|directory_id| directory_id != session_id)
                {
                    return self.invalid(sink);
                }
                self.current_model = data
                    .and_then(|data| data.get("selectedModel"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.started_at_ms = Some(timestamp);
                self.started = true;
            }
            "session.model_change" => {
                let Some(model) = value
                    .pointer("/data/newModel")
                    .and_then(Value::as_str)
                    .filter(|model| !model.is_empty())
                else {
                    return self.invalid(sink);
                };
                self.current_model = Some(model.to_owned());
            }
            "subagent.started" => {
                let Some(call_id) = value
                    .pointer("/data/toolCallId")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                else {
                    return self.invalid(sink);
                };
                if self
                    .subagents
                    .insert(
                        call_id.to_owned(),
                        value
                            .pointer("/data/model")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    )
                    .is_some()
                {
                    return self.invalid(sink);
                }
                sink.record(NormalizedRecord::Observation(Box::new(
                    EvidenceObservation::SubagentSpawn {
                        ts_ms: Some(timestamp),
                        parent_model: self.current_model.clone(),
                        parent_call_id: Some(call_id.to_owned()),
                        child_model: None,
                        provenance: RelationProvenance::SpawnAgentCall,
                    },
                )));
            }
            "subagent.completed" | "subagent.failed" => {
                let Some(call_id) = value.pointer("/data/toolCallId").and_then(Value::as_str)
                else {
                    return self.invalid(sink);
                };
                let Some(start_model) = self.subagents.remove(call_id) else {
                    return self.invalid(sink);
                };
                let model = value
                    .pointer("/data/model")
                    .and_then(Value::as_str)
                    .or(start_model.as_deref());
                let Some(model) = model.filter(|model| !model.is_empty()) else {
                    return self.invalid(sink);
                };
                sink.record(NormalizedRecord::Observation(Box::new(
                    EvidenceObservation::SubagentModel {
                        parent_call_id: call_id.to_owned(),
                        model: model.to_owned(),
                    },
                )));
            }
            "session.shutdown" => self.shutdown(value.get("data"), timestamp, sink),
            _ => {}
        }
    }

    fn shutdown(&mut self, data: Option<&Value>, timestamp: i64, sink: &mut dyn RecordSink) {
        let Some(data) = data else {
            return self.invalid(sink);
        };
        if !matches!(
            data.get("shutdownType").and_then(Value::as_str),
            Some("routine") | Some("error")
        ) || data
            .get("sessionStartTime")
            .and_then(Value::as_i64)
            .is_none()
            || !data.get("modelMetrics").is_some_and(Value::is_object)
            || !self.subagents.is_empty()
        {
            return self.invalid(sink);
        }
        for (model, metric) in data["modelMetrics"].as_object().into_iter().flatten() {
            let usage = &metric["usage"];
            let Some(input_tokens) = usage.get("inputTokens").and_then(Value::as_u64) else {
                return self.invalid(sink);
            };
            let Some(output_tokens) = usage.get("outputTokens").and_then(Value::as_u64) else {
                return self.invalid(sink);
            };
            let Some(cache_read_tokens) = usage.get("cacheReadTokens").and_then(Value::as_u64)
            else {
                return self.invalid(sink);
            };
            let Some(cache_creation_tokens) = usage.get("cacheWriteTokens").and_then(Value::as_u64)
            else {
                return self.invalid(sink);
            };
            sink.record(NormalizedRecord::MetricsEvent(Box::new(NormalizedEvent {
                ts_ms: Some(timestamp),
                usage_ts_ms: None,
                role: Role::Assistant,
                source: Default::default(),
                usage: Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_creation_tokens,
                    cache_creation_1h_tokens: 0,
                },
                tools: Vec::new(),
                model: Some(model.to_owned()),
                provider: None,
                api: None,
                thinking_mode: None,
                speed: None,
                has_thinking: false,
                message_id: None,
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
        self.shutdown = true;
    }

    fn invalid(&mut self, sink: &mut dyn RecordSink) {
        self.invalid = true;
        sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
    }

    fn finish(self) -> SessionSummary {
        let mut summary = SessionSummary {
            started_at_ms: self.started_at_ms,
            model: self.current_model,
            ..SessionSummary::default()
        };
        if !self.started || !self.shutdown || self.invalid || !self.subagents.is_empty() {
            summary.coverage_gaps.push(PartialReason::MalformedRecord);
        }
        summary
    }
}

fn cli_directory_id(path: &Path) -> Option<&str> {
    (path.file_name()?.to_str()? == "events.jsonl").then_some(())?;
    let id = path.parent()?.file_name()?.to_str()?;
    (path.parent()?.parent()?.file_name()?.to_str()? == "session-state").then_some(id)
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::interface::SessionCollector;
    use crate::analysis::source_validity::SourceClaim;
    use crate::discovery::source_version::head_hash_of;
    use crate::discovery::{FingerprintInputs, SourceStat};
    use tempfile::TempDir;

    const LIFECYCLE: &str =
        include_str!("../../../tests/fixtures/copilot_characterization/subagent_lifecycle.jsonl");
    const MALFORMED: &str =
        include_str!("../../../tests/fixtures/copilot_characterization/malformed_partial.jsonl");

    fn input(content: &str) -> SessionInput {
        SessionInput {
            agent: "copilot".to_owned(),
            session_id: "11111111-1111-4111-8111-111111111111".to_owned(),
            source: RawSource::Jsonl(content.to_owned()),
            source_format: SourceFormat::CopilotCliJsonl,
            fork_parent_session_id: None,
        }
    }

    #[test]
    fn v1_lifecycle_retains_only_model_usage_and_subagent_relation() {
        let reader = CopilotSessionReader;
        let mut sink = SessionCollector::new("copilot", "session");
        reader.visit(&input(LIFECYCLE), &mut sink).unwrap();
        let session = sink.into_session().unwrap();
        assert_eq!(session.events.len(), 1);
        assert_eq!(session.events[0].model.as_deref(), Some("gpt-5"));
        assert_eq!(session.events[0].usage.input_tokens, 10);
        assert!(session.events[0].tools.is_empty());
    }

    #[test]
    fn malformed_or_unfinished_source_stays_partial() {
        let reader = CopilotSessionReader;
        let mut sink = SessionCollector::new("copilot", "session");
        reader.visit(&input(MALFORMED), &mut sink).unwrap();
        assert_eq!(
            sink.coverage(),
            crate::analysis::interface::RecordCoverage::Partial
        );
    }

    #[test]
    fn file_path_requires_the_session_uuid_directory_to_match_start() {
        let dir = TempDir::new().unwrap();
        let path = dir
            .path()
            .join("session-state")
            .join("22222222-2222-4222-8222-222222222222")
            .join("events.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, LIFECYCLE).unwrap();
        let input = SessionInput {
            source: RawSource::File(path),
            ..input("")
        };
        let mut sink = SessionCollector::new("copilot", "session");
        CopilotSessionReader.visit(&input, &mut sink).unwrap();
        assert_eq!(
            sink.coverage(),
            crate::analysis::interface::RecordCoverage::Partial
        );
    }

    #[test]
    fn source_change_rejects_the_claimed_read() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        std::fs::write(&path, LIFECYCLE).unwrap();
        let file = File::open(&path).unwrap();
        let stat = SourceStat::from_open_std_file(&file).unwrap();
        let claim = SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
            stat,
            head_hash: Some(head_hash_of(LIFECYCLE.as_bytes())),
        });
        std::fs::write(&path, format!("{LIFECYCLE}\n")).unwrap();
        let input = SessionInput {
            source: RawSource::File(path),
            ..input("")
        };
        let mut sink = SessionCollector::new("copilot", "session");
        assert!(matches!(
            CopilotSessionReader
                .visit_claimed(
                    &input,
                    &claim,
                    AppendOnlyGuarantee::Absent,
                    &|| false,
                    &mut sink
                )
                .unwrap(),
            VisitOutcome::SourceChanged(_)
        ));
    }
}
