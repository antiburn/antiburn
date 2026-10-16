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
        ) && matches!(value.get("parentId"), Some(Value::Null | Value::String(_)))
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
