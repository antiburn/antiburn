//! Native rollout results use the existing output and completed-item parsers.

use super::*;
use crate::analysis::resume::identity_filter::IdentityFilter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct State {
    parser: CodexStreamState,
    retired_calls: IdentityFilter,
    replaced_calls: IdentityFilter,
    completed_ids: IdentityFilter,
    root: bool,
    paginated: bool,
    ordinal: Option<u64>,
    suffix_only: bool,
    public: crate::analysis::public_events::JsonState<super::public::PublicState>,
}

impl State {
    pub fn new(input: &SessionInput, header: &Value, suffix_only: bool) -> Self {
        let meta = &header["payload"];
        let root = header["type"] == "session_meta"
            && meta["id"] == input.session_id
            && meta["cli_version"] == "0.160.1"
            && meta["thread_source"] == "user"
            && matches!(meta["history_mode"].as_str(), Some("legacy" | "paginated"))
            && (meta["history_mode"] != "paginated" || header["ordinal"] == 0)
            && ["forked_from_id", "parent_thread_id", "history_base"]
                .iter()
                .all(|key| meta.get(key).is_none_or(Value::is_null))
            && input.fork_parent_session_id.is_none();
        let mut parser = CodexStreamState::for_input(input);
        parser.observe_native_request(header, root);
        parser.completed.observe(header, true, &mut HeaderSink);
        Self {
            parser,
            retired_calls: IdentityFilter::default(),
            replaced_calls: IdentityFilter::default(),
            completed_ids: IdentityFilter::default(),
            root,
            paginated: meta["history_mode"] == "paginated",
            ordinal: None,
            suffix_only,
            public: Default::default(),
        }
    }

    pub fn observe(&mut self, value: Value, index: u64, sink: &mut dyn RecordSink) {
        let mut public = std::mem::take(&mut self.public.0);
        public.begin(&value, index, &self.parser.native_turn_id, sink);
        {
            let mut capture = super::public::CycleSink::new(
                &mut public,
                &value,
                self.parser.native_turn_id.as_deref(),
                sink,
            );
            self.observe_inner(value.clone(), index, &mut capture);
        }
        if self.replaces_exec_content(&value) {
            public.close_replaced_output(&value);
        }
        self.parser.completed.retire_live_replacement(&value);
        public.end(
            &value,
            index,
            self.root,
            &self.parser.session_id,
            (
                &self.parser.native_request,
                self.parser.native_turn_id.as_deref(),
            ),
            sink,
        );
        self.public.0 = public;
    }

    fn observe_inner(&mut self, value: Value, index: u64, sink: &mut dyn RecordSink) {
        if !has_required_timestamp(&value) {
            self.parser.scope.invalidate();
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            return;
        }
        self.parser.observe_native_request(&value, self.root);
        if value["type"] == "session_meta" {
            // The first physical header also initializes the parser on full-window startup.
            if index != 0 || self.suffix_only {
                self.parser.report_ordinal_regression(&value, index, sink);
                self.root = false;
            }
            return;
        }
        self.parser.report_ordinal_regression(&value, index, sink);
        if self.paginated {
            let ordinal = value["ordinal"].as_u64();
            if self.ordinal.is_none() {
                self.parser.completed.live_seam(ordinal);
            }
            self.root &= ordinal.is_some()
                && (self.ordinal.is_some() || self.suffix_only || ordinal == Some(1))
                && self
                    .ordinal
                    .is_none_or(|last| ordinal == last.checked_add(1));
            self.ordinal = ordinal;
        }
        if value["type"] == "compacted"
            || value["type"] == "history_base"
            || value["payload"]["type"] == "thread_rolled_back"
        {
            self.root = false;
        }
        let payload = &value["payload"];
        let mut capture = CompletedCapture {
            content: None,
            gap: false,
        };
        self.parser
            .completed
            .observe(&value, self.root, &mut capture);
        self.root &= self.parser.completed.admits(&self.parser.session_id);
        self.parser.completed.compact_live_identities();
        if capture.gap {
            sink.tool_result_gap();
        }
        let call_id = payload["call_id"]
            .as_str()
            .or_else(|| payload["callId"].as_str());
        let is_call = matches!(
            payload["type"].as_str(),
            Some("function_call" | "custom_tool_call" | "tool_search_call" | "web_search_call")
        );
        let is_output = matches!(
            payload["type"].as_str(),
            Some(
                "function_call_output"
                    | "custom_tool_call_output"
                    | "tool_search_output"
                    | "mcp_tool_call_output"
                    | "web_search_output"
            )
        );
        let replaced = self.replaces_exec_content(&value);
        let uncertain_replacement = is_output
            && !replaced
            && completed::replaces_exec_content(&value, self.paginated, |id| {
                self.replaced_calls.contains(id)
            });
        if uncertain_replacement {
            // A filter match cannot prove that this output replaces an exec call.
            sink.tool_result_gap();
        }
        if is_call
            && replaced
            && let Some(id) = call_id
        {
            self.replaced_calls.insert(id);
        }
        let retired = call_id.is_some_and(|id| {
            self.retired_calls.contains(id) && !self.parser.native_calls.contains_key(id)
        });
        if !(is_call && retired || is_output && (replaced || uncertain_replacement)) {
            self.parser.capture_native_tool_result(&value, index, sink);
        }
        self.parser.native_calls.retain(|_, call| call.is_some());
        self.parser.native_call_bytes = self
            .parser
            .native_calls
            .iter()
            .map(|(id, call)| id.len() + call.as_ref().map_or(0, |call| call.retained_bytes))
            .sum();
        if is_call && let Some(id) = call_id {
            self.retired_calls.insert(id);
        }
        if is_output && let Some(id) = call_id {
            self.retired_calls.insert(id);
            self.parser.native_calls.remove(id);
            self.parser.native_call_bytes = self
                .parser
                .native_calls
                .iter()
                .map(|(id, call)| id.len() + call.as_ref().map_or(0, |call| call.retained_bytes))
                .sum();
        }
        if value["type"] == "response_item" && payload["type"] == "message" {
            let mut parts = content_parts_for_record(&value);
            self.parser.completed.capture_message(&value, &mut parts);
            if self.root {
                emit_user_message(index, &parts, sink);
                let known_injection = payload
                    .pointer("/internal_chat_message_metadata_passthrough/content_item_kinds")
                    .and_then(Value::as_array)
                    .is_some_and(|kinds| {
                        !kinds.is_empty()
                            && kinds
                                .iter()
                                .all(|kind| kind == "skills.selected_skill_instructions")
                    });
                if payload["role"] == "user"
                    && !known_injection
                    && (parts.is_empty()
                        || parts.iter().any(|part| {
                            part.truncated || part.metadata.user_text_history.is_none()
                        }))
                {
                    sink.tool_result_gap();
                }
            }
        }
        if value["type"] == "event_msg" && payload["type"] == "item_completed" {
            let Some(id) = payload["item"]["id"].as_str() else {
                return;
            };
            if self.completed_ids.contains(id) {
                sink.tool_result_gap();
                return;
            }
            self.completed_ids.insert(id);
            if let Some(content) = capture.content
                && let Some(mut result) = completed_native_result(
                    &content,
                    Some(id),
                    self.parser.source_format,
                    &self.parser.session_id,
                    index,
                    codex_host_status(payload["item"]["status"].as_str()),
                    self.parser.completed_facts(&value),
                )
            {
                result.context = Some(crate::analysis::ToolResultContext::pending(
                    result.position,
                    self.root,
                ));
                sink.record(NormalizedRecord::ToolResult(Box::new(result)));
            }
        }
    }

    fn replaces_exec_content(&self, record: &Value) -> bool {
        self.parser.completed.replaces_exec_content(record)
    }

    pub fn main_session(&self) -> bool {
        self.root
    }

    pub fn invalidate(&mut self) {
        self.root = false;
        self.public.0.invalidate();
    }
}

/// Exact revision-4 layout. Conversion preserves every retained join and guard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StateV4 {
    parser: CodexStreamState,
    retired_calls: IdentityFilter,
    replaced_calls: IdentityFilter,
    completed_ids: IdentityFilter,
    root: bool,
    paginated: bool,
    ordinal: Option<u64>,
    suffix_only: bool,
}

impl StateV4 {
    pub(crate) fn convert(self) -> State {
        State {
            parser: self.parser,
            retired_calls: self.retired_calls,
            replaced_calls: self.replaced_calls,
            completed_ids: self.completed_ids,
            root: self.root,
            paginated: self.paginated,
            ordinal: self.ordinal,
            suffix_only: self.suffix_only,
            public: Default::default(),
        }
    }
}

#[cfg(test)]
impl State {
    pub(crate) fn as_v4(&self) -> StateV4 {
        StateV4 {
            parser: self.parser.clone(),
            retired_calls: self.retired_calls.clone(),
            replaced_calls: self.replaced_calls.clone(),
            completed_ids: self.completed_ids.clone(),
            root: self.root,
            paginated: self.paginated,
            ordinal: self.ordinal,
            suffix_only: self.suffix_only,
        }
    }
}

struct HeaderSink;
impl RecordSink for HeaderSink {
    fn record(&mut self, _: NormalizedRecord) {}
    fn finish(&mut self, _: SessionSummary) {}
}

struct CompletedCapture {
    content: Option<Box<TurnContent>>,
    gap: bool,
}
impl RecordSink for CompletedCapture {
    fn record(&mut self, record: NormalizedRecord) {
        match record {
            NormalizedRecord::TurnContent(content) => self.content = Some(content),
            NormalizedRecord::Unusable(_) => self.gap = true,
            _ => {}
        }
    }
    fn finish(&mut self, _: SessionSummary) {}
}
