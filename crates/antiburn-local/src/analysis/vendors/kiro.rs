//! Kiro CLI V2 bundle reader.
//!
//! This reader retains only model, token, and tool identity facts. It does not
//! retain prompts, messages, tool input, tool results, paths, or permissions.

use std::fs::File;
use std::io::BufReader;

use serde_json::Value;

use crate::analysis::framing::{BoundedJsonlReader, FramedRecord, PartialReason};
use crate::analysis::interface::{
    RawSource, RecordSink, SessionCollector, SessionInput, SessionReader, SessionSummary,
    VisitOutcome,
};
use crate::analysis::model::{
    NormalizedEvent, NormalizedSession, Role, ToolCall, ToolCategory, Usage,
};
use crate::analysis::{SourceCapabilities, SourceFormat};

pub struct KiroSessionReader;

impl SessionReader for KiroSessionReader {
    fn agent(&self) -> &'static str {
        "kiro"
    }

    fn capabilities(&self, input: &SessionInput) -> SourceCapabilities {
        SourceCapabilities::uncharacterized(input.source_format_or(SourceFormat::KiroSessionJson))
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
        if input.source_format == SourceFormat::KiroCrewJsonl {
            return visit_crew(input, sink);
        }
        if input.source_format != SourceFormat::KiroCliV2Bundle {
            sink.record(crate::analysis::NormalizedRecord::Unusable(
                PartialReason::MalformedRecord,
            ));
            sink.finish(SessionSummary::default());
            return Ok(VisitOutcome::Unvalidated);
        }
        let RawSource::KiroCliV2Bundle {
            metadata_path,
            messages_path,
        } = &input.source
        else {
            anyhow::bail!("Kiro CLI V2 requires a metadata and journal bundle");
        };
        let metadata: Value = serde_json::from_reader(File::open(metadata_path)?)?;
        let model = validate_metadata(&metadata, &input.session_id)?;
        observe_turn_usage(&metadata, model.as_deref(), sink);
        let mut invalid = false;
        let mut framed = BoundedJsonlReader::new(BufReader::new(File::open(messages_path)?));
        while let Some(record) = framed.next_record(&|| false) {
            match record {
                FramedRecord::Skipped(skip) => {
                    invalid = true;
                    sink.record(crate::analysis::NormalizedRecord::Unusable(
                        skip.partial_reason(),
                    ));
                }
                FramedRecord::Complete { bytes, .. } => {
                    match serde_json::from_slice::<Value>(bytes) {
                        Ok(value) => {
                            if !observe_envelope(&value, model.as_deref(), sink) {
                                invalid = true;
                                sink.record(crate::analysis::NormalizedRecord::Unusable(
                                    PartialReason::MalformedRecord,
                                ));
                            }
                        }
                        Err(_) => {
                            invalid = true;
                            sink.record(crate::analysis::NormalizedRecord::Unusable(
                                PartialReason::MalformedRecord,
                            ));
                        }
                    }
                }
            }
        }
        let mut summary = SessionSummary {
            model,
            ..SessionSummary::default()
        };
        if invalid {
            summary.coverage_gaps.push(PartialReason::MalformedRecord);
        }
        sink.finish(summary);
        Ok(VisitOutcome::Unvalidated)
    }
}

/// Read a Kiro Crew session log: one `.jsonl` whose first line is a
/// `{"_type":"metadata",…}` header and whose remaining lines are
/// `{"role":"user"|"assistant"|"tool","content":…}` records.
///
/// Crew logs record no per-turn token usage, so this emits model identity only
/// (one assistant event per assistant record carrying the session model). Tool
/// identity is not recoverable: a `role:"tool"` record is a tool *result* with
/// no typed tool name, so no `ToolCall` is fabricated. Prompts, message bodies,
/// and tool content are never retained.
fn visit_crew(input: &SessionInput, sink: &mut dyn RecordSink) -> anyhow::Result<VisitOutcome> {
    let RawSource::File(path) = &input.source else {
        anyhow::bail!("Kiro Crew source must be a single JSONL file");
    };
    let mut invalid = false;
    let mut model: Option<String> = None;
    let mut framed = BoundedJsonlReader::new(BufReader::new(File::open(path)?));
    let mut first = true;
    while let Some(record) = framed.next_record(&|| false) {
        match record {
            FramedRecord::Skipped(skip) => {
                invalid = true;
                sink.record(crate::analysis::NormalizedRecord::Unusable(
                    skip.partial_reason(),
                ));
            }
            FramedRecord::Complete { bytes, .. } => {
                let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
                    invalid = true;
                    sink.record(crate::analysis::NormalizedRecord::Unusable(
                        PartialReason::MalformedRecord,
                    ));
                    first = false;
                    continue;
                };
                if first {
                    first = false;
                    if value.get("_type").and_then(Value::as_str) == Some("metadata") {
                        model = value
                            .get("model")
                            .and_then(Value::as_str)
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned);
                        continue;
                    }
                    // A Crew log always opens with its metadata header; without
                    // one the session model is unknown, so the read is partial.
                    invalid = true;
                    sink.record(crate::analysis::NormalizedRecord::Unusable(
                        PartialReason::MalformedRecord,
                    ));
                }
                match value.get("role").and_then(Value::as_str) {
                    Some("assistant") => {
                        sink.record(crate::analysis::NormalizedRecord::MetricsEvent(Box::new(
                            crew_assistant_event(model.as_deref()),
                        )));
                    }
                    Some(role) if CREW_NON_MODEL_ROLES.contains(&role) => {}
                    // An unknown role (or no role) could be a model turn in a
                    // newer shape; report it rather than dropping it silently.
                    _ => {
                        invalid = true;
                        sink.record(crate::analysis::NormalizedRecord::Unusable(
                            PartialReason::MalformedRecord,
                        ));
                    }
                }
            }
        }
    }
    let mut summary = SessionSummary {
        model,
        ..SessionSummary::default()
    };
    if invalid {
        summary.coverage_gaps.push(PartialReason::MalformedRecord);
    }
    sink.finish(summary);
    Ok(VisitOutcome::Unvalidated)
}

/// Crew record roles that are known and carry no model turn: the user's
/// prompt, tool results, and dashboard-side events (errors, attached files,
/// injected context, cards, system notices, MCP OAuth prompts).
const CREW_NON_MODEL_ROLES: &[&str] = &[
    "user",
    "tool",
    "error",
    "file",
    "inject",
    "card",
    "system",
    "notice",
    "mcp_oauth",
];

/// One assistant turn in a Crew log, carrying model identity and no usage.
fn crew_assistant_event(model: Option<&str>) -> NormalizedEvent {
    NormalizedEvent {
        accounting: None,
        ts_ms: None,
        usage_ts_ms: None,
        role: Role::Assistant,
        source: Default::default(),
        usage: Usage::default(),
        tools: Vec::new(),
        model: model.map(str::to_owned),
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
        proven_subagent_launches: None,
    }
}

fn observe_turn_usage(metadata: &Value, default_model: Option<&str>, sink: &mut dyn RecordSink) {
    let Some(turns) = metadata
        .pointer("/session_state/conversation_metadata/user_turn_metadatas")
        .and_then(Value::as_array)
    else {
        return;
    };
    for turn in turns {
        let token = |name| turn.get(name).and_then(Value::as_u64).unwrap_or(0);
        let usage = Usage {
            input_tokens: token("input_token_count"),
            output_tokens: token("output_token_count"),
            cache_read_tokens: token("cache_read_input_token_count"),
            cache_creation_tokens: token("cache_write_input_token_count"),
            cache_creation_1h_tokens: 0,
        };
        if usage == Usage::default() {
            continue;
        }
        sink.record(crate::analysis::NormalizedRecord::MetricsEvent(Box::new(
            NormalizedEvent {
                accounting: None,
                ts_ms: None,
                usage_ts_ms: None,
                role: Role::Assistant,
                source: Default::default(),
                usage,
                tools: Vec::new(),
                model: turn
                    .get("model")
                    .and_then(Value::as_str)
                    .or(default_model)
                    .map(str::to_owned),
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
                proven_subagent_launches: None,
            },
        )));
    }
}

fn validate_metadata(metadata: &Value, session_id: &str) -> anyhow::Result<Option<String>> {
    let object = metadata
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Kiro metadata is not an object"))?;
    if object.get("session_id").and_then(Value::as_str) != Some(session_id)
        || !object.get("cwd").is_some_and(Value::is_string)
        || !object.get("created_at").is_some_and(Value::is_string)
        || !object.get("updated_at").is_some_and(Value::is_string)
        || !object.get("session_state").is_some_and(Value::is_object)
    {
        anyhow::bail!("Kiro metadata does not match the V2 contract");
    }
    let state = &metadata["session_state"];
    if state.get("version").and_then(Value::as_str) != Some("v1")
        || !state.get("rts_model_state").is_some_and(Value::is_object)
    {
        anyhow::bail!("Kiro metadata has an unsupported session state");
    }
    Ok(state
        .pointer("/rts_model_state/model_info/model_id")
        .and_then(Value::as_str)
        .map(str::to_owned))
}

fn observe_envelope(value: &Value, model: Option<&str>, sink: &mut dyn RecordSink) -> bool {
    let Some(kind) = value.get("kind").and_then(Value::as_str) else {
        return false;
    };
    if value.get("version").and_then(Value::as_str) != Some("v1")
        || !matches!(kind, "Prompt" | "AssistantMessage" | "ToolResults")
    {
        return false;
    }
    let Some(data) = value.get("data").and_then(Value::as_object) else {
        return false;
    };
    let Some(content) = data.get("content").and_then(Value::as_array) else {
        return false;
    };
    let mut tools = Vec::new();
    for part in content {
        let Some(part_kind) = part.get("kind").and_then(Value::as_str) else {
            return false;
        };
        match part_kind {
            "text" => {
                if !part.get("data").is_some_and(Value::is_string) {
                    return false;
                }
            }
            "thinking" => {
                if !part.pointer("/data/modelId").is_some_and(Value::is_string) {
                    return false;
                }
            }
            "toolUse" => {
                let Some(name) = part
                    .pointer("/data/name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                else {
                    return false;
                };
                if !part.pointer("/data/input").is_some_and(Value::is_object) {
                    return false;
                }
                tools.push(ToolCall {
                    name: name.to_owned(),
                    category: ToolCategory::Other,
                    detail: None,
                });
            }
            "toolResult" => {
                if !part
                    .pointer("/data/toolUseId")
                    .is_some_and(Value::is_string)
                {
                    return false;
                }
            }
            _ => return false,
        }
    }
    if kind == "AssistantMessage" && (!tools.is_empty() || model.is_some()) {
        sink.record(crate::analysis::NormalizedRecord::MetricsEvent(Box::new(
            NormalizedEvent {
                accounting: None,
                ts_ms: None,
                usage_ts_ms: None,
                role: Role::Assistant,
                source: Default::default(),
                usage: Usage::default(),
                tools,
                model: model.map(str::to_owned),
                provider: None,
                api: None,
                thinking_mode: None,
                speed: None,
                has_thinking: false,
                message_id: data
                    .get("message_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
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
                proven_subagent_launches: None,
            },
        )));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::interface::RecordCoverage;
    use tempfile::TempDir;

    const ROOT_ID: &str = "11111111-1111-4111-8111-111111111111";

    fn root_bundle(temp: &TempDir) -> SessionInput {
        let metadata_path = temp.path().join(format!("{ROOT_ID}.json"));
        let messages_path = temp.path().join(format!("{ROOT_ID}.jsonl"));
        std::fs::write(
            &metadata_path,
            include_str!("../../../tests/fixtures/kiro_cli_v2_bundle/root.json"),
        )
        .unwrap();
        std::fs::write(
            &messages_path,
            include_str!("../../../tests/fixtures/kiro_cli_v2_bundle/root.jsonl"),
        )
        .unwrap();
        SessionInput {
            agent: "kiro".to_owned(),
            session_id: ROOT_ID.to_owned(),
            source: RawSource::KiroCliV2Bundle {
                metadata_path,
                messages_path,
            },
            source_format: SourceFormat::KiroCliV2Bundle,
            fork_parent_session_id: None,
        }
    }

    #[test]
    fn v2_root_and_child_bundles_retain_only_safe_facts() {
        let temp = TempDir::new().unwrap();
        for (id, metadata, journal) in [
            (
                "11111111-1111-4111-8111-111111111111",
                include_str!("../../../tests/fixtures/kiro_cli_v2_bundle/root.json"),
                include_str!("../../../tests/fixtures/kiro_cli_v2_bundle/root.jsonl"),
            ),
            (
                "22222222-2222-4222-8222-222222222222",
                include_str!("../../../tests/fixtures/kiro_cli_v2_bundle/child.json"),
                include_str!("../../../tests/fixtures/kiro_cli_v2_bundle/child.jsonl"),
            ),
        ] {
            let metadata_path = temp.path().join(format!("{id}.json"));
            let messages_path = temp.path().join(format!("{id}.jsonl"));
            std::fs::write(&metadata_path, metadata).unwrap();
            std::fs::write(&messages_path, journal).unwrap();
            let input = SessionInput {
                agent: "kiro".to_owned(),
                session_id: id.to_owned(),
                source: RawSource::KiroCliV2Bundle {
                    metadata_path,
                    messages_path,
                },
                source_format: SourceFormat::KiroCliV2Bundle,
                fork_parent_session_id: None,
            };
            let session = KiroSessionReader.normalize(&input).unwrap();
            assert_eq!(session.events.len(), 2);
            assert_eq!(session.model.as_deref(), Some("safe-model"));
            assert_eq!(
                KiroSessionReader.capabilities(&input),
                SourceCapabilities::uncharacterized(SourceFormat::KiroCliV2Bundle)
            );
        }
    }

    #[test]
    fn unsupported_metadata_or_journal_version_fails_closed() {
        for path_kind in ["metadata", "journal"] {
            let temp = TempDir::new().unwrap();
            let input = root_bundle(&temp);
            let RawSource::KiroCliV2Bundle {
                metadata_path,
                messages_path,
            } = &input.source
            else {
                unreachable!()
            };
            let path = if path_kind == "metadata" {
                metadata_path
            } else {
                messages_path
            };
            let content = std::fs::read_to_string(path)
                .unwrap()
                .replacen("\"v1\"", "\"v2\"", 1);
            std::fs::write(path, content).unwrap();

            let mut sink = SessionCollector::new("kiro", ROOT_ID);
            let result = KiroSessionReader.visit(&input, &mut sink);
            if path_kind == "metadata" {
                assert!(result.is_err());
            } else {
                result.unwrap();
                assert_eq!(sink.coverage(), RecordCoverage::Partial);
            }
        }
    }

    #[test]
    fn missing_journal_companion_is_rejected() {
        let temp = TempDir::new().unwrap();
        let input = root_bundle(&temp);
        let RawSource::KiroCliV2Bundle { messages_path, .. } = &input.source else {
            unreachable!()
        };
        std::fs::remove_file(messages_path).unwrap();
        assert!(KiroSessionReader.normalize(&input).is_err());
    }

    fn crew_input(temp: &TempDir) -> SessionInput {
        let path = temp.path().join("dashboard_chat-1-1793312743.jsonl");
        std::fs::write(
            &path,
            include_str!("../../../tests/fixtures/kiro_crew_bundle/session.jsonl"),
        )
        .unwrap();
        SessionInput {
            agent: "kiro".to_owned(),
            session_id: "dashboard_chat-1-1793312743".to_owned(),
            source: RawSource::File(path),
            source_format: SourceFormat::KiroCrewJsonl,
            fork_parent_session_id: None,
        }
    }

    #[test]
    fn crew_log_emits_model_identity_per_assistant_turn_without_usage() {
        let temp = TempDir::new().unwrap();
        let input = crew_input(&temp);
        let session = KiroSessionReader.normalize(&input).unwrap();

        // Two assistant records → two events; the user and tool records add none.
        assert_eq!(session.events.len(), 2);
        assert_eq!(session.model.as_deref(), Some("safe-model"));
        for event in &session.events {
            assert_eq!(event.model.as_deref(), Some("safe-model"));
            // Crew logs carry no per-turn usage and no typed tool-call identity.
            assert_eq!(event.usage, Usage::default());
            assert!(event.tools.is_empty());
        }
        assert_eq!(
            KiroSessionReader.capabilities(&input),
            SourceCapabilities::uncharacterized(SourceFormat::KiroCrewJsonl)
        );
    }

    #[test]
    fn crew_log_without_a_model_is_still_read() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("dashboard_chat-2-1793312744.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"_type":"metadata","title":"no model","agent":"kirocrew","model":""}"#,
                "\n",
                r#"{"role":"user","content":"hi"}"#,
                "\n",
                r#"{"role":"assistant","content":"hello"}"#,
                "\n",
            ),
        )
        .unwrap();
        let input = SessionInput {
            agent: "kiro".to_owned(),
            session_id: "dashboard_chat-2-1793312744".to_owned(),
            source: RawSource::File(path),
            source_format: SourceFormat::KiroCrewJsonl,
            fork_parent_session_id: None,
        };
        let session = KiroSessionReader.normalize(&input).unwrap();
        assert_eq!(session.events.len(), 1);
        assert_eq!(session.model, None);
        assert_eq!(session.events[0].model, None);
    }

    fn crew_coverage(temp: &TempDir, body: &str) -> RecordCoverage {
        let path = temp.path().join("dashboard_chat-3-1793312745.jsonl");
        std::fs::write(&path, body).unwrap();
        let input = SessionInput {
            agent: "kiro".to_owned(),
            session_id: "dashboard_chat-3-1793312745".to_owned(),
            source: RawSource::File(path),
            source_format: SourceFormat::KiroCrewJsonl,
            fork_parent_session_id: None,
        };
        let mut sink = SessionCollector::new("kiro", "dashboard_chat-3-1793312745");
        KiroSessionReader.visit(&input, &mut sink).unwrap();
        sink.coverage()
    }

    #[test]
    fn crew_log_with_known_roles_is_complete() {
        let temp = TempDir::new().unwrap();
        let body = concat!(
            r#"{"_type":"metadata","model":"safe-model"}"#,
            "\n",
            r#"{"role":"user","content":"hi"}"#,
            "\n",
            r#"{"role":"inject","content":"x"}"#,
            "\n",
            r#"{"role":"assistant","content":"hello"}"#,
            "\n",
            r#"{"role":"notice","content":"y"}"#,
            "\n",
        );
        assert_eq!(crew_coverage(&temp, body), RecordCoverage::Complete);
    }

    #[test]
    fn crew_log_without_a_metadata_header_is_partial() {
        let temp = TempDir::new().unwrap();
        let body = concat!(
            r#"{"role":"user","content":"hi"}"#,
            "\n",
            r#"{"role":"assistant","content":"hello"}"#,
            "\n",
        );
        assert_eq!(crew_coverage(&temp, body), RecordCoverage::Partial);
    }

    #[test]
    fn crew_log_with_an_unknown_role_is_partial() {
        let temp = TempDir::new().unwrap();
        for record in [
            r#"{"role":"agent","content":"hello"}"#,
            r#"{"content":"no role"}"#,
        ] {
            let body = format!("{{\"_type\":\"metadata\",\"model\":\"m\"}}\n{record}\n");
            assert_eq!(crew_coverage(&temp, &body), RecordCoverage::Partial);
        }
    }
}
