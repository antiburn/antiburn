//! Native V3 file results retain local linear order and exact direct-parent joins.

use super::*;
use crate::analysis::resume::identity_filter::IdentityFilter;
use std::collections::BTreeSet;

const MAX_RECORD_IDS: usize = 4096;
const MAX_RECORD_ID_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct State {
    parser: PiStreamState,
    retired_calls: IdentityFilter,
    records: IdentityFilter,
    last: Option<String>,
    root: bool,
    suffix_only: bool,
    record_ids: BTreeSet<String>,
    record_id_bytes: usize,
    /// A summary hit can leave order unproved without proving a branch change.
    order_unproven: bool,
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
            record_ids: BTreeSet::new(),
            record_id_bytes: 0,
            order_unproven: false,
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
        let summary_hit = self.records.contains(&id);
        let retained_record = self.record_ids.contains(&id)
            || self.last.as_deref() == Some(id.as_str())
            || self
                .parser
                .native_tool_calls
                .values()
                .any(|call| call.record_id == id)
            || summary_hit
                && sink
                    .native_context(
                        crate::analysis::SourceFormat::PiV3Jsonl,
                        self.parser.native_session_id.as_deref().unwrap_or_default(),
                        ToolResultPosition::Jsonl {
                            record_index: index,
                            item_index: u32::MAX,
                        },
                        self.root,
                    )
                    .user_messages
                    .iter()
                    .any(|message| message.message_id == id);
        if retained_record || summary_hit {
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
                        + call.facts.retained_bytes()
                })
                .sum();
            if retained_record {
                sink.native_admission_failure(crate::analysis::NativeAdmissionFailure {
                    source_format: crate::analysis::SourceFormat::PiV3Jsonl,
                    session_id: self.parser.native_session_id.clone().unwrap_or_default(),
                    position: ToolResultPosition::Jsonl {
                        record_index: index,
                        item_index: u32::MAX,
                    },
                    reason: ToolResultCompleteness::UnsafeIdentity,
                });
                return;
            }
            sink.tool_result_gap();
            self.retain_record_id(&id);
            self.order_unproven = !self.continues_root(&value);
            if !self.order_unproven {
                self.last = Some(id);
            }
            return;
        }
        self.records.insert(&id);
        self.retain_record_id(&id);
        if self.order_unproven && !self.continues_root(&value) {
            sink.tool_result_gap();
            return;
        }
        self.order_unproven = false;
        if self.root
            && self.last.as_ref().is_some_and(|last| {
                value
                    .get("parentId")
                    .is_some_and(|parent| parent.as_str() != Some(last))
            })
        {
            sink.native_admission_failure(crate::analysis::NativeAdmissionFailure {
                source_format: crate::analysis::SourceFormat::PiV3Jsonl,
                session_id: self.parser.native_session_id.clone().unwrap_or_default(),
                position: ToolResultPosition::Jsonl {
                    record_index: index,
                    item_index: u32::MAX,
                },
                reason: ToolResultCompleteness::UnsafeOrder,
            });
        }
        self.root = self.continues_root(&value);
        self.last = Some(id);
        if self.root
            && matches!(
                value["type"].as_str(),
                Some("compaction" | "custom_message")
            )
        {
            sink.tool_result_gap();
        }
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
                for block in blocks {
                    let reject = block["type"] == "toolCall"
                        && block["id"].as_str().is_some_and(|id| {
                            if duplicates.contains(id)
                                || self.parser.native_tool_calls.remove(id).is_some()
                            {
                                self.retired_calls.insert(id);
                                self.parser.native_tool_calls.remove(id);
                                true
                            } else {
                                self.retired_calls.contains(id)
                            }
                        });
                    if reject {
                        *block = Value::Null;
                    }
                }
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
                        + call.facts.retained_bytes()
                })
                .sum();
            self.parser.observe_native_tool_calls(&value);
            self.parser.publication_root = self.root;
            self.parser.emit_public_messages(&value, index, sink);
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
            self.parser
                .emit_native_tool_results_with_admission(sink, false);
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
                            + call.facts.retained_bytes()
                    })
                    .sum();
            }
        } else if role == Some("user") && self.root {
            let parts = extract_content_parts(&value, Role::User);
            self.parser.emit_user_message(&value, index, &parts, sink);
        }
        if self.root && value["type"] == "model_change" {
            let mut selection = crate::analysis::NativeRequestAttribution::observed(
                value["provider"].as_str(),
                value["modelId"].as_str(),
            );
            selection.fast_mode = recorded_model_suffix_fast_mode(value["modelId"].as_str());
            sink.session_selection(crate::analysis::NativeSessionSelectionObservation {
                source_format: crate::analysis::SourceFormat::PiV3Jsonl,
                session_id: self.parser.native_session_id.clone().unwrap_or_default(),
                position: ToolResultPosition::Jsonl {
                    record_index: index,
                    item_index: 0,
                },
                selection,
            });
        }
        if self.root
            && value["type"] == "message"
            && matches!(
                value["message"]["role"].as_str(),
                Some("user" | "assistant" | "toolResult")
            )
        {
            crate::analysis::public_events::emit_activity(
                sink,
                crate::analysis::SourceFormat::PiV3Jsonl,
                self.parser.native_session_id.as_deref().unwrap_or_default(),
                index,
                value["id"].as_str().unwrap_or("message"),
                value.get("timestamp").and_then(parse_ts),
            );
        }
    }

    pub fn main_session(&self) -> bool {
        self.root
    }

    fn continues_root(&self, value: &Value) -> bool {
        self.root
            && native::supports_publication_entry(value)
            && matches!(value.get("parentId"), Some(Value::Null | Value::String(_)))
            && (self.last.is_some() || self.suffix_only || value["parentId"].is_null())
            && self
                .last
                .as_deref()
                .is_none_or(|last| value["parentId"] == last)
    }

    pub fn invalidate(&mut self) {
        self.root = false;
    }

    fn retain_record_id(&mut self, id: &str) {
        if self.record_ids.len() < MAX_RECORD_IDS
            && id.len() <= MAX_RECORD_ID_BYTES.saturating_sub(self.record_id_bytes)
            && self.record_ids.insert(id.to_owned())
        {
            self.record_id_bytes += id.len();
        }
    }

    pub(crate) fn valid(&self) -> bool {
        self.record_ids.len() <= MAX_RECORD_IDS
            && self.record_id_bytes <= MAX_RECORD_ID_BYTES
            && self.record_id_bytes == self.record_ids.iter().map(String::len).sum::<usize>()
    }
}

/// Exact revision-4 layout. Conversion preserves parser progress and joins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StateV4 {
    parser: PiStreamState,
    retired_calls: IdentityFilter,
    records: IdentityFilter,
    last: Option<String>,
    root: bool,
    suffix_only: bool,
}

impl StateV4 {
    pub(crate) fn convert(self, history_incomplete: bool) -> State {
        let mut state = State {
            parser: self.parser,
            retired_calls: self.retired_calls,
            records: self.records,
            last: self.last,
            root: self.root,
            suffix_only: self.suffix_only,
            record_ids: BTreeSet::new(),
            record_id_bytes: 0,
            order_unproven: history_incomplete,
        };
        if let Some(last) = state.last.clone() {
            state.retain_record_id(&last);
        }
        let call_records = state
            .parser
            .native_tool_calls
            .values()
            .map(|call| call.record_id.clone())
            .collect::<BTreeSet<_>>();
        for id in call_records {
            state.retain_record_id(&id);
        }
        state
    }
}

#[cfg(test)]
impl State {
    pub(crate) fn as_v4(&self) -> StateV4 {
        StateV4 {
            parser: self.parser.clone(),
            retired_calls: self.retired_calls.clone(),
            records: self.records.clone(),
            last: self.last.clone(),
            root: self.root,
            suffix_only: self.suffix_only,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::SourceFormat;
    use crate::analysis::interface::{RawSource, SessionCollector, SessionInput};
    use serde_json::json;

    #[derive(Default)]
    struct NativeSink {
        users: Vec<UserMessage>,
        results: Vec<ToolResultObservation>,
        activity: Vec<crate::analysis::NativeActivityObservation>,
        gaps: usize,
    }

    impl RecordSink for NativeSink {
        fn record(&mut self, record: NormalizedRecord) {
            if let NormalizedRecord::ToolResult(result) = record {
                self.results.push(*result);
            }
        }

        fn user_message(&mut self, message: UserMessage) {
            self.users.push(message);
        }

        fn native_activity(&mut self, activity: crate::analysis::NativeActivityObservation) {
            self.activity.push(activity);
        }

        fn tool_result_gap(&mut self) {
            self.gaps += 1;
        }

        fn finish(&mut self, _: SessionSummary) {}
    }

    #[test]
    fn exact_record_identity_cache_stops_at_both_retention_limits() {
        let input = SessionInput {
            agent: "pi".into(),
            session_id: "facts".into(),
            source_format: crate::analysis::SourceFormat::PiV3Jsonl,
            source: RawSource::Jsonl(String::new()),
            fork_parent_session_id: None,
        };
        for id_bytes in [16, 4096] {
            let mut state = State::new(
                &input,
                &json!({"type":"session","version":3,"id":"facts","timestamp":100}),
                false,
            );
            let count = (MAX_RECORD_ID_BYTES / id_bytes).min(MAX_RECORD_IDS);
            for index in 0..count * 2 {
                state.retain_record_id(&format!("{index:08}{}", "x".repeat(id_bytes - 8)));
            }
            assert_eq!(state.record_ids.len(), count);
            assert_eq!(state.record_id_bytes, count * id_bytes);
            assert!(state.valid());
            let encoded = postcard::to_allocvec(&state).unwrap();
            let mut state: State = postcard::from_bytes(&encoded).unwrap();
            for index in count * 2..count * 3 {
                state.retain_record_id(&format!("{index:08}{}", "x".repeat(id_bytes - 8)));
            }
            assert!(postcard::to_allocvec(&state).unwrap() == encoded);
            let bound = MAX_RECORD_ID_BYTES
                + MAX_RECORD_IDS * postcard::to_allocvec(&MAX_RECORD_ID_BYTES).unwrap().len()
                + postcard::to_allocvec(&MAX_RECORD_IDS).unwrap().len();
            assert!(postcard::to_allocvec(&state.record_ids).unwrap().len() <= bound);
        }
    }
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
            json!({"type":"message","id":"assistant","parentId":"antiburn","timestamp":"2026-01-01T00:00:09Z","message":{"role":"assistant","content":[{"type":"text","text":"I received the advisory."},{"type":"toolCall","id":"next-call-id","name":"bash","arguments":{"command":"true"}}]}}),
            json!({"type":"message","id":"next-result","parentId":"assistant","timestamp":"2026-01-01T00:00:10Z","message":{"role":"toolResult","toolCallId":"next-call-id","toolName":"bash","isError":false,"content":[{"type":"text","text":"synthetic command succeeded"}]}}),
        ];
        let mut sink = NativeSink::default();
        for (index, row) in rows.iter().cloned().enumerate() {
            state.observe(row, index as u64, &mut sink);
        }

        assert!(state.main_session());
        assert_eq!(
            sink.users
                .iter()
                .map(|message| message.message_id.as_str())
                .collect::<Vec<_>>(),
            ["user", "next-user"]
        );
        assert!(sink.activity.iter().all(|activity| !matches!(
            activity.position,
            ToolResultPosition::Jsonl {
                record_index: 8,
                ..
            }
        )));
        assert_eq!(sink.results.len(), 2);
        assert_eq!(sink.results[1].call_id.as_deref(), Some("next-call-id"));
        assert_eq!(sink.results[1].status, ToolResultStatus::Success);
        assert!(sink.gaps > 0);

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
