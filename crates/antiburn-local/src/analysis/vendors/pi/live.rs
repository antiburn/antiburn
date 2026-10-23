//! Native V3 file results retain local linear order and exact direct-parent joins.

use super::*;
use crate::analysis::resume::identity_filter::IdentityFilter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct State {
    parser: PiStreamState,
    retired_calls: IdentityFilter,
    records: IdentityFilter,
    last: Option<String>,
    root: bool,
    suffix_only: bool,
}

impl State {
    pub fn new(input: &SessionInput, header: &Value, suffix_only: bool) -> Self {
        let root = header["type"] == "session"
            && header["version"] == 3
            && header["id"] == input.session_id
            && header.get("parentSession").is_none()
            && header.get("timestamp").and_then(parse_ts).is_some()
            && input.fork_parent_session_id.is_none();
        let mut parser = PiStreamState::default().with_native_contract(input, PiDialect::PI);
        parser.native_session_id = Some(input.session_id.clone());
        parser.publication_root = root;
        Self {
            parser,
            retired_calls: IdentityFilter::default(),
            records: IdentityFilter::default(),
            last: None,
            root,
            suffix_only,
        }
    }

    pub fn observe(&mut self, mut value: Value, index: u64, sink: &mut dyn RecordSink) {
        if value["type"] == "session" {
            if index != 0 || self.suffix_only {
                self.root = false;
            }
            return;
        }
        let Some(id) = thread_identity_field(&value, "id") else {
            self.root = false;
            return;
        };
        if self.records.contains(&id) {
            for block in value["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "toolCall"
                    && let Some(call_id) = block["id"].as_str()
                {
                    self.retired_calls.insert(call_id);
                    self.parser.native_tool_calls.remove(call_id);
                }
            }
            self.parser
                .native_tool_calls
                .retain(|_, call| call.record_id != id);
            self.parser.native_tool_bytes = self
                .parser
                .native_tool_calls
                .iter()
                .map(|(id, call)| {
                    id.len()
                        + call.record_id.len()
                        + call.name.len()
                        + call.arguments.as_ref().map_or(0, String::len)
                })
                .sum();
            sink.tool_result_admission_gap(crate::analysis::ToolResultAdmission::UnsafeIdentity);
            return;
        }
        self.records.insert(&id);
        self.root &= matches!(
            value["type"].as_str(),
            Some("model_change" | "thinking_level_change" | "message" | "session_info" | "label")
        ) || (value["type"] == "custom_message"
            && native_record_complete(&value)
            && is_inert_shape(&value));
        self.root &= matches!(value.get("parentId"), Some(Value::Null | Value::String(_)))
            && (self.last.is_some() || self.suffix_only || value["parentId"].is_null())
            && self
                .last
                .as_deref()
                .is_none_or(|last| value["parentId"] == last);
        self.last = Some(id);
        let role = value["message"]["role"].as_str();
        if role == Some("assistant") {
            if let Some(blocks) = value
                .pointer_mut("/message/content")
                .and_then(Value::as_array_mut)
            {
                let mut seen = HashSet::new();
                let duplicates: HashSet<String> = blocks
                    .iter()
                    .filter(|block| block["type"] == "toolCall")
                    .filter_map(|block| block["id"].as_str())
                    .filter(|id| !seen.insert((*id).to_owned()))
                    .map(str::to_owned)
                    .collect();
                blocks.retain(|block| {
                    block["type"] != "toolCall"
                        || block["id"].as_str().is_none_or(|id| {
                            if duplicates.contains(id)
                                || self.parser.native_tool_calls.remove(id).is_some()
                            {
                                self.retired_calls.insert(id);
                                self.parser.native_tool_calls.remove(id);
                                false
                            } else {
                                !self.retired_calls.contains(id)
                            }
                        })
                });
            }
            self.parser.native_tool_bytes = self
                .parser
                .native_tool_calls
                .iter()
                .map(|(id, call)| {
                    id.len()
                        + call.record_id.len()
                        + call.name.len()
                        + call.arguments.as_ref().map_or(0, String::len)
                })
                .sum();
            self.parser.observe_native_tool_calls(&value);
            for block in value["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "toolCall"
                    && let Some(id) = block["id"].as_str()
                {
                    self.retired_calls.insert(id);
                }
            }
        } else if role == Some("toolResult") {
            // Keep result staging separate from the unresolved call budget.
            let call_bytes = std::mem::take(&mut self.parser.native_tool_bytes);
            self.parser.observe_native_tool_result(&value, index);
            self.parser.publication_root = self.root;
            self.parser.emit_native_tool_results(sink);
            self.parser.native_tool_bytes = call_bytes;
            if value["message"]["isError"].is_boolean()
                && let Some(id) = value["message"]["toolCallId"].as_str()
            {
                self.retired_calls.insert(id);
                self.parser.native_tool_calls.remove(id);
                self.parser.native_tool_bytes = self
                    .parser
                    .native_tool_calls
                    .iter()
                    .map(|(id, call)| {
                        id.len()
                            + call.record_id.len()
                            + call.name.len()
                            + call.arguments.as_ref().map_or(0, String::len)
                    })
                    .sum();
            }
        } else if role == Some("user") && self.root {
            let parts = extract_content_parts(&value, Role::User);
            self.parser.emit_user_message(&value, index, &parts, sink);
        }
    }

    pub fn main_session(&self) -> bool {
        self.root
    }

    pub fn invalidate(&mut self) {
        self.root = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::SourceFormat;
    use crate::analysis::interface::{RawSource, SessionCollector, SessionInput};
    use serde_json::json;

    #[test]
    fn inert_antiburn_custom_message_preserves_native_continuation_root() {
        let input = SessionInput {
            agent: "pi".to_owned(),
            session_id: "synthetic-pi-session".to_owned(),
            source: RawSource::Jsonl(String::new()),
            fork_parent_session_id: None,
            source_format: SourceFormat::PiV3Jsonl,
        };
        let header = json!({
            "type": "session",
            "version": 3,
            "id": "synthetic-pi-session",
            "timestamp": "2026-01-01T00:00:00Z"
        });
        let mut state = State::new(&input, &header, false);
        let rows = [
            json!({"type":"session","version":3,"id":"synthetic-pi-session","timestamp":"2026-01-01T00:00:00Z"}),
            json!({"type":"model_change","id":"model","parentId":null,"timestamp":"2026-01-01T00:00:01Z","provider":"synthetic","modelId":"model"}),
            json!({"type":"message","id":"user","parentId":"model","timestamp":"2026-01-01T00:00:02Z","message":{"role":"user","content":"Run the fixture check."}}),
            json!({"type":"message","id":"call","parentId":"user","timestamp":"2026-01-01T00:00:03Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"call-id","name":"bash","arguments":{"command":"false"}}]}}),
            json!({"type":"message","id":"result","parentId":"call","timestamp":"2026-01-01T00:00:04Z","message":{"role":"toolResult","toolCallId":"call-id","toolName":"bash","isError":true,"content":[{"type":"text","text":"synthetic command failed"}]}}),
            json!({"type":"thinking_level_change","id":"thinking","parentId":"result","timestamp":"2026-01-01T00:00:05Z","thinkingLevel":"low"}),
            json!({"type":"message","id":"system","parentId":"thinking","timestamp":"2026-01-01T00:00:06Z","message":{"role":"system","content":""},"sections":[]}),
            json!({"type":"message","id":"next-user","parentId":"system","timestamp":"2026-01-01T00:00:07Z","message":{"role":"user","content":"Report the result."}}),
            json!({"type":"custom_message","id":"antiburn","parentId":"next-user","timestamp":"2026-01-01T00:00:08Z","customType":"antiburn-session-checks","display":false,"content":"Synthetic advisory."}),
            json!({"type":"message","id":"assistant","parentId":"antiburn","timestamp":"2026-01-01T00:00:09Z","message":{"role":"assistant","content":[{"type":"text","text":"I received the advisory."}]}}),
        ];
        let mut sink = SessionCollector::new("pi", "synthetic-pi-session");
        for (index, row) in rows.iter().cloned().enumerate() {
            state.observe(row, index as u64, &mut sink);
        }

        assert!(state.main_session());

        let mut signal_state = State::new(&input, &header, false);
        let mut signal_sink = SessionCollector::new("pi", "synthetic-pi-session");
        for (index, mut row) in rows.iter().cloned().enumerate() {
            if index == 8 {
                row.as_object_mut()
                    .expect("synthetic row is an object")
                    .insert("usage".to_owned(), json!({"input": 1}));
            }
            signal_state.observe(row, index as u64, &mut signal_sink);
        }

        assert!(!signal_state.main_session());

        let mut partial_state = State::new(&input, &header, false);
        let mut partial_sink = SessionCollector::new("pi", "synthetic-pi-session");
        for (index, mut row) in rows.iter().cloned().enumerate() {
            if index == 8 {
                row.as_object_mut()
                    .expect("synthetic row is an object")
                    .insert("incomplete".to_owned(), json!(true));
            }
            partial_state.observe(row, index as u64, &mut partial_sink);
        }

        assert!(!partial_state.main_session());
    }
}
