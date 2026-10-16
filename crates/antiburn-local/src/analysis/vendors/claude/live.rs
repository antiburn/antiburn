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
    record["version"] == "2.1.278"
        && record["entrypoint"] == "cli"
        && record["sessionId"] == session
        && record["isSidechain"] == false
        && record.get("agentId").is_none()
        && (!matches!(record["type"].as_str(), Some("user" | "assistant"))
            || record.pointer("/message/role") == record.get("type"))
}
