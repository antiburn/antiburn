//! Native result delivery for the pinned CLI shape, without metric accumulation.

use super::*;
use crate::analysis::resume::identity_filter::IdentityFilter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct State {
    parser: ClaudeStreamState,
    retired_calls: IdentityFilter,
    records: IdentityFilter,
    last: Option<String>,
    root: bool,
}

impl State {
    pub fn new(input: &SessionInput, header: &Value) -> Self {
        Self {
            parser: ClaudeStreamState::for_input(input),
            retired_calls: IdentityFilter::default(),
            records: IdentityFilter::default(),
            last: None,
            root: is_root_input(input)
                && root_record(header, &input.session_id)
                && header.get("parentUuid") == Some(&Value::Null)
                && header.get("logicalParentUuid").is_none_or(Value::is_null)
                && (retained_root::is_user_content(header)
                    || retained_root::is_informational_root(header)),
        }
    }

    pub fn observe(&mut self, mut value: Value, index: u64, sink: &mut dyn RecordSink) {
        let Some(id) = thread_identity_field(&value, "uuid") else {
            if matches!(value["type"].as_str(), Some("user" | "assistant")) {
                self.root = false;
            }
            return;
        };
        if self.records.contains(&id) {
            for block in value["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_use"
                    && let Some(call_id) = block["id"].as_str()
                {
                    self.retired_calls.insert(call_id);
                    self.parser.native_calls.remove(call_id);
                }
            }
            self.parser.native_calls.retain(|_, call| {
                call.as_ref()
                    .is_none_or(|call| call.record_id.as_deref() != Some(&id))
            });
            self.parser.native_call_bytes = self
                .parser
                .native_calls
                .iter()
                .map(|(id, call)| id.len() + call.as_ref().map_or(0, |call| call.retained_bytes))
                .sum();
            sink.tool_result_admission_gap(crate::analysis::ToolResultAdmission::UnsafeIdentity);
            return;
        }
        self.records.insert(&id);
        self.root &= root_record(&value, &self.parser.session_id)
            && self
                .last
                .as_deref()
                .is_none_or(|last| value["parentUuid"] == last)
            && value.get("logicalParentUuid").is_none_or(Value::is_null)
            && value["subtype"] != "compact_boundary"
            && value["isCompactSummary"] != true;
        self.last = Some(id);
        if self.root {
            emit_user_message(&value, index, &self.parser.session_id, sink);
            if value["type"] == "user"
                && !retained_root::is_user_content(&value)
                && value["message"]["content"]
                    .as_array()
                    .is_none_or(|blocks| blocks.iter().any(|block| block["type"] != "tool_result"))
            {
                sink.tool_result_gap();
            }
        }
        if value["type"] == "assistant"
            && let Some(blocks) = value
                .pointer_mut("/message/content")
                .and_then(Value::as_array_mut)
        {
            blocks.retain(|block| {
                block["type"] != "tool_use"
                    || block["id"].as_str().is_none_or(|id| {
                        self.parser.native_calls.contains_key(id)
                            || !self.retired_calls.contains(id)
                    })
            });
        }
        self.parser.capture_native_tool_results(&value, index, sink);
        self.parser.native_calls.retain(|_, call| call.is_some());
        self.parser.native_call_bytes = self
            .parser
            .native_calls
            .iter()
            .map(|(id, call)| id.len() + call.as_ref().map_or(0, |call| call.retained_bytes))
            .sum();
        if value["type"] == "assistant" {
            for block in value["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_use"
                    && let Some(id) = block["id"].as_str()
                {
                    self.retired_calls.insert(id);
                }
            }
        }
        if value["type"] == "user" {
            for block in value["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_result"
                    && let Some(id) = block["tool_use_id"].as_str()
                {
                    self.retired_calls.insert(id);
                    self.parser.native_calls.remove(id);
                }
            }
            self.parser.native_call_bytes = self
                .parser
                .native_calls
                .iter()
                .map(|(id, call)| id.len() + call.as_ref().map_or(0, |call| call.retained_bytes))
                .sum();
        }
    }

    pub fn main_session(&self) -> bool {
        self.root
    }

    pub fn invalidate(&mut self) {
        self.root = false;
    }
}

fn root_record(record: &Value, session: &str) -> bool {
    retained_root::is_characterized_cli(record)
        && record["sessionId"] == session
        && record["isSidechain"] == false
        && record.get("agentId").is_none()
        && (!matches!(record["type"].as_str(), Some("user" | "assistant"))
            || record.pointer("/message/role") == record.get("type"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::interface::{
        NormalizedRecord, ToolResultCompleteness, ToolResultObservation, ToolResultStatus,
    };

    #[derive(Default)]
    struct ResultSink {
        results: Vec<ToolResultObservation>,
    }

    impl RecordSink for ResultSink {
        fn record(&mut self, record: NormalizedRecord) {
            if let NormalizedRecord::ToolResult(result) = record {
                self.results.push(*result);
            }
        }

        fn finish(&mut self, _summary: SessionSummary) {}
    }

    fn record(kind: &str, id: &str, parent: Value, message: Value) -> Value {
        serde_json::json!({
            "type": kind,
            "version": "2.1.309",
            "entrypoint": "cli",
            "sessionId": "session",
            "isSidechain": false,
            "uuid": id,
            "parentUuid": parent,
            "message": message
        })
    }

    #[test]
    fn current_cli_resume_keeps_native_result_delivery() {
        let input = SessionInput {
            agent: "claude".to_owned(),
            session_id: "session".to_owned(),
            source: RawSource::Jsonl(String::new()),
            fork_parent_session_id: None,
            source_format: crate::analysis::SourceFormat::ClaudeJsonl,
        };
        let header = serde_json::json!({
            "type": "user",
            "version": "2.1.278",
            "entrypoint": "cli",
            "sessionId": "session",
            "isSidechain": false,
            "uuid": "root",
            "parentUuid": null,
            "message": {"role": "user", "content": "Original request."}
        });
        let mut state = State::new(&input, &header);
        let mut sink = ResultSink::default();

        state.observe(
            record(
                "system",
                "bridge",
                Value::String("root".into()),
                Value::Null,
            ),
            1,
            &mut sink,
        );
        state.observe(
            record(
                "assistant",
                "call-row",
                Value::String("bridge".into()),
                serde_json::json!({
                    "role": "assistant",
                    "content": [{"type": "tool_use", "id": "call", "name": "Bash", "input": {"command": "false"}}]
                }),
            ),
            2,
            &mut sink,
        );
        state.observe(
            record(
                "user",
                "result-row",
                Value::String("call-row".into()),
                serde_json::json!({
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": "call", "content": "synthetic failure", "is_error": true}]
                }),
            ),
            3,
            &mut sink,
        );

        assert!(state.main_session());
        assert_eq!(sink.results.len(), 1);
        assert_eq!(sink.results[0].status, ToolResultStatus::Error);
        assert_eq!(
            sink.results[0].completeness,
            ToolResultCompleteness::Complete
        );
    }
}
