//! Native evidence from one ordered Cursor Desktop database snapshot.

use serde_json::Value;

use crate::analysis::{
    EVIDENCE_STRING_CAP, MAX_CONTENT_PART_BYTES, NormalizedRecord, RecordSink, SessionInput,
    SourceFormat, ToolResultChange, ToolResultCompleteness, ToolResultContext,
    ToolResultObservation, ToolResultPosition, ToolResultStatus, UserMessage, UserMessageChange,
    UserMessageCompleteness, UserMessageOrigin, UserMessagePart, UserMessagePosition,
    UserMessageScope,
};

pub(super) struct DesktopEvidence<'a> {
    input: &'a SessionInput,
    metadata_seen: bool,
    enabled: bool,
    ordered: bool,
    root: bool,
}

impl<'a> DesktopEvidence<'a> {
    pub(super) fn new(input: &'a SessionInput) -> Self {
        Self {
            input,
            metadata_seen: false,
            enabled: false,
            ordered: false,
            root: false,
        }
    }

    pub(super) fn metadata(&mut self, value: &Value, index: u64, sink: &mut dyn RecordSink) {
        if self.metadata_seen || index != 0 {
            self.enabled = false;
            sink.tool_result_gap();
            sink.tool_result_admission_gap(crate::analysis::ToolResultAdmission::UnsafeIdentity);
            return;
        }
        self.metadata_seen = true;
        self.enabled = self.input.source_format == SourceFormat::CursorIdeComposer
            && value.get("cursor_source").and_then(Value::as_str) == Some("desktop_state_vscdb")
            && value.get("session_id").and_then(Value::as_str) == Some(&self.input.session_id)
            && bounded_id(&self.input.session_id).is_some();
        if !self.enabled {
            sink.tool_result_gap();
        }
        self.ordered =
            value.get("cursor_scope_ordering").and_then(Value::as_str) == Some("recorded_headers");
        self.root = self.input.fork_parent_session_id.is_none()
            && value
                .get(crate::discovery::FORK_OBSERVATION_KEY)
                .is_some_and(Value::is_null);
    }

    pub(super) fn finish(&self, sink: &mut dyn RecordSink) {
        if self.input.source_format != SourceFormat::CursorIdeComposer {
            return;
        }
        let admission = if !self.metadata_seen || !self.enabled {
            Some(crate::analysis::ToolResultAdmission::UnsafeIdentity)
        } else if !self.ordered {
            Some(crate::analysis::ToolResultAdmission::UnsafeOrder)
        } else if !self.root {
            Some(crate::analysis::ToolResultAdmission::UnsafeOwnership)
        } else {
            None
        };
        if let Some(admission) = admission {
            sink.tool_result_gap();
            sink.tool_result_admission_gap(admission);
        }
    }

    pub(super) fn observe(&self, value: &Value, index: u64, sink: &mut dyn RecordSink) {
        if !self.enabled {
            return;
        }
        let id = value
            .get("bubbleId")
            .and_then(Value::as_str)
            .and_then(bounded_id);
        if self.root && self.ordered && value.get("role").and_then(Value::as_str) == Some("user") {
            let text = value.get("content").and_then(Value::as_str);
            if let (Some(id), Some(text)) = (
                id,
                text.filter(|text| !text.is_empty() && text.len() <= MAX_CONTENT_PART_BYTES),
            ) {
                sink.user_message(UserMessage {
                    source_format: SourceFormat::CursorIdeComposer,
                    session_id: self.input.session_id.clone(),
                    message_id: id.into(),
                    position: UserMessagePosition::Jsonl {
                        record_index: index,
                    },
                    origin: UserMessageOrigin::CursorDesktopUserContent,
                    scope: UserMessageScope::MainSession,
                    change: UserMessageChange::Present,
                    completeness: UserMessageCompleteness::Complete,
                    parts: vec![UserMessagePart {
                        item_index: 0,
                        native_part_id: None,
                        text: text.into(),
                    }],
                    proof_revision: 1,
                });
            } else {
                sink.record(NormalizedRecord::Unusable(
                    crate::analysis::PartialReason::AttributionIncomplete,
                ));
            }
        }
        if value.get("role").and_then(Value::as_str) != Some("assistant") {
            return;
        }
        let Some(tool) = value
            .get("cursor_tool_result")
            .filter(|tool| tool.is_object())
        else {
            return;
        };
        let call_id = tool
            .get("toolCallId")
            .and_then(Value::as_str)
            .and_then(bounded_id);
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .and_then(bounded_id);
        let arguments = parse_object(tool.get("params"));
        let result = parse_object(tool.get("result"));
        let output = result
            .as_ref()
            .and_then(|result| result.get("output"))
            .and_then(Value::as_str)
            .filter(|text| text.len() <= MAX_CONTENT_PART_BYTES);
        let exit = result
            .as_ref()
            .and_then(|result| result.get("exitCode"))
            .and_then(Value::as_i64);
        let terminal = tool.get("status").and_then(Value::as_str) == Some("completed");
        let shell = name == Some("run_terminal_command_v2");
        let status = if shell && terminal {
            if result
                .as_ref()
                .and_then(|result| result.get("rejected"))
                .and_then(Value::as_bool)
                == Some(true)
                || result
                    .as_ref()
                    .and_then(|result| result.get("notInterrupted"))
                    .and_then(Value::as_bool)
                    == Some(false)
            {
                ToolResultStatus::Interrupted
            } else {
                match exit {
                    Some(0) => ToolResultStatus::Success,
                    Some(_) => ToolResultStatus::Error,
                    None if result
                        .as_ref()
                        .is_some_and(|result| result.get("exitCode").is_none()) =>
                    {
                        match tool.get("native_status").and_then(Value::as_str) {
                            Some("success") => ToolResultStatus::Success,
                            Some("error") => ToolResultStatus::Error,
                            _ => ToolResultStatus::Unknown,
                        }
                    }
                    None => ToolResultStatus::Unknown,
                }
            }
        } else {
            ToolResultStatus::Unknown
        };
        let position = ToolResultPosition::Jsonl {
            record_index: index,
            item_index: 0,
        };
        let complete = shell
            && terminal
            && arguments.is_some()
            && output.is_some()
            && (exit.is_some()
                || result
                    .as_ref()
                    .is_some_and(|result| result.get("exitCode").is_none()))
            && status != ToolResultStatus::Unknown;
        sink.record(NormalizedRecord::ToolResult(Box::new(
            ToolResultObservation {
                source_format: SourceFormat::CursorIdeComposer,
                session_id: self.input.session_id.clone(),
                record_id: id.map(str::to_owned),
                call_id: call_id.map(str::to_owned),
                result_id: id.map(str::to_owned),
                position,
                revision: 1,
                change: ToolResultChange::Update,
                tool_name: name.map(str::to_owned),
                tool_category: name.map(|name| {
                    crate::analysis::records::tool_call_from_input(name, arguments.as_ref())
                        .category
                }),
                arguments,
                content: output.map(str::to_owned),
                status,
                finality: terminal,
                completeness: if !self.root {
                    ToolResultCompleteness::UnsafeOwnership
                } else if !self.ordered {
                    ToolResultCompleteness::UnsafeOrder
                } else if id.is_none() || call_id.is_none() || name.is_none() {
                    ToolResultCompleteness::UnsafeIdentity
                } else if complete {
                    ToolResultCompleteness::Complete
                } else {
                    ToolResultCompleteness::Incomplete
                },
                context: Some(ToolResultContext::pending(
                    position,
                    self.root && self.ordered,
                )),
            },
        )));
    }
}

fn bounded_id(value: &str) -> Option<&str> {
    (!value.is_empty() && value.len() <= EVIDENCE_STRING_CAP).then_some(value)
}

fn parse_object(value: Option<&Value>) -> Option<Value> {
    let text = value?
        .as_str()
        .filter(|text| text.len() <= MAX_CONTENT_PART_BYTES)?;
    serde_json::from_str::<Value>(text)
        .ok()
        .filter(Value::is_object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{
        CompositeSink, EvidenceSource, RawSource, SessionEvidenceAccumulator,
        SessionMetricsAccumulator, SourceCapabilities, SourceKind, ToolResultAdmission, reader_for,
    };
    use serde_json::json;

    fn read(
        ordered: bool,
        fork: bool,
        missing_call: bool,
        source_format: SourceFormat,
    ) -> CompositeSink {
        read_result(
            ordered,
            fork,
            missing_call,
            source_format,
            json!({"output":"Parser failed", "exitCode":1,"notInterrupted":true,"rejected":false}),
            None,
        )
    }

    fn read_result(
        ordered: bool,
        fork: bool,
        missing_call: bool,
        source_format: SourceFormat,
        result: Value,
        native_status: Option<&str>,
    ) -> CompositeSink {
        let values = [
            json!({"session_id":"synthetic", "cursor_source":"desktop_state_vscdb", "cursor_scope_ordering":if ordered {"recorded_headers"} else {"unproven"}, "local_fork_observation":null}),
            json!({"role":"user", "bubbleId":"request", "timestamp":"2026-01-01T00:00:00Z", "content":"Fix the parser failure."}),
            json!({"role":"assistant", "bubbleId":"bubble/t:call\nresult", "model":"synthetic", "timestamp":"2026-01-01T00:00:01Z", "content":"", "cursor_tool_result":{
                "toolCallId": if missing_call {Value::Null} else {json!("call\nresult")}, "name":"run_terminal_command_v2", "status":"completed", "params":json!({"command":"cargo test parser"}).to_string(), "result":result.to_string(), "native_status":native_status
            }}),
        ];
        let input = SessionInput {
            agent: "cursor".into(),
            session_id: "synthetic".into(),
            source: RawSource::Jsonl(
                values
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            source_format,
            fork_parent_session_id: fork.then(|| "parent".into()),
        };
        let mut sink = CompositeSink::new(
            SessionMetricsAccumulator::new("cursor", "synthetic"),
            SessionEvidenceAccumulator::new(EvidenceSource {
                agent: "cursor".into(),
                session_id: "synthetic".into(),
                kind: SourceKind::Jsonl,
                capabilities: SourceCapabilities {
                    source_format,
                    ..SourceCapabilities::cursor()
                },
            }),
        );
        reader_for("cursor").visit(&input, &mut sink).unwrap();
        sink
    }

    #[test]
    fn desktop_shell_uses_recorded_status_when_exit_code_is_absent() {
        for (native, expected) in [
            (Some("success"), ToolResultStatus::Success),
            (Some("error"), ToolResultStatus::Error),
            (None, ToolResultStatus::Unknown),
        ] {
            let sink = read_result(
                true,
                false,
                false,
                SourceFormat::CursorIdeComposer,
                json!({"output":"native result", "rejected":false,"notInterrupted":true}),
                native,
            );
            let result = &sink.tool_result_observations()[0];
            assert_eq!(result.status, expected);
            assert_eq!(
                result.completeness,
                if native.is_some() {
                    ToolResultCompleteness::Complete
                } else {
                    ToolResultCompleteness::Incomplete
                }
            );
        }
    }

    #[test]
    fn malformed_exit_code_does_not_use_status_fallback() {
        let sink = read_result(
            true,
            false,
            false,
            SourceFormat::CursorIdeComposer,
            json!({"output":"native result", "exitCode":"invalid"}),
            Some("success"),
        );
        let result = &sink.tool_result_observations()[0];
        assert_eq!(result.status, ToolResultStatus::Unknown);
        assert_eq!(result.completeness, ToolResultCompleteness::Incomplete);
        let sink = read_result(
            true,
            false,
            false,
            SourceFormat::CursorIdeComposer,
            json!({"output":"native result", "exitCode":"invalid", "rejected":true}),
            Some("success"),
        );
        let result = &sink.tool_result_observations()[0];
        assert_eq!(result.status, ToolResultStatus::Interrupted);
        assert_eq!(result.completeness, ToolResultCompleteness::Incomplete);
    }

    #[test]
    fn missing_desktop_metadata_cannot_claim_complete_membership() {
        let source_format = SourceFormat::CursorIdeComposer;
        let input = SessionInput {
            agent: "cursor".into(),
            session_id: "synthetic".into(),
            source: RawSource::Jsonl(
                json!({"role":"user","bubbleId":"user","content":"Fix the parser."}).to_string(),
            ),
            source_format,
            fork_parent_session_id: None,
        };
        let mut sink = CompositeSink::new(
            SessionMetricsAccumulator::new("cursor", "synthetic"),
            SessionEvidenceAccumulator::new(EvidenceSource {
                agent: "cursor".into(),
                session_id: "synthetic".into(),
                kind: SourceKind::Jsonl,
                capabilities: SourceCapabilities {
                    source_format,
                    ..SourceCapabilities::cursor()
                },
            }),
        );
        reader_for("cursor").visit(&input, &mut sink).unwrap();
        assert!(!sink.tool_result_observations_complete());
        assert!(sink.tool_result_observations().is_empty());
    }

    #[test]
    fn desktop_shell_result_keeps_native_identity_status_and_user_prefix() {
        let sink = read(true, false, false, SourceFormat::CursorIdeComposer);
        let result = &sink.tool_result_observations()[0];
        assert_eq!(result.admission(), ToolResultAdmission::Admitted);
        assert_eq!(result.completeness, ToolResultCompleteness::Complete);
        assert_eq!(result.status, ToolResultStatus::Error);
        assert_eq!(result.record_id.as_deref(), Some("bubble/t:call\nresult"));
        assert_eq!(
            result.arguments.as_ref().unwrap()["command"],
            "cargo test parser"
        );
        assert_eq!(result.content.as_deref(), Some("Parser failed"));
        let context = result.context.as_ref().unwrap();
        assert!(context.complete_through_event);
        assert_eq!(context.user_messages.len(), 1);
        assert!(context.user_messages[0].is_bounded());
        assert_eq!(
            context.user_messages[0].parts[0].text,
            "Fix the parser failure."
        );
    }

    #[test]
    fn desktop_results_reject_unsafe_order_forks_and_missing_identity() {
        for (ordered, fork, missing, expected) in [
            (false, false, false, ToolResultAdmission::UnsafeOrder),
            (true, true, false, ToolResultAdmission::UnsafeOwnership),
            (true, false, true, ToolResultAdmission::UnsafeIdentity),
        ] {
            let sink = read(ordered, fork, missing, SourceFormat::CursorIdeComposer);
            assert_eq!(sink.tool_result_observations()[0].admission(), expected);
        }
    }

    #[test]
    fn desktop_marker_does_not_enable_other_cursor_source_contracts() {
        for format in [
            SourceFormat::CursorJsonl,
            SourceFormat::CursorCliAgentJsonl,
            SourceFormat::CursorChatStoreDb,
        ] {
            assert!(
                read(true, false, false, format)
                    .tool_result_observations()
                    .is_empty()
            );
        }
    }
}
