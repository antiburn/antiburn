//! Public records and complete task intervals for the pinned 0.160.1 rollout.

use super::*;
use crate::analysis::public_events::{JsonState, user_observation};
use crate::analysis::{
    NativeCycleKey, NativeEventContext, NativeMessageObservation, NativeRequestAttribution,
    PublicMessageObservation, PublicMessagePayload, ResponseCycleObservation, ResponseCycleStatus,
    UserMessagePart,
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_CYCLE_MESSAGES: usize = 4096;
const MAX_CYCLE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PENDING_INPUTS: usize = 256;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ReadState {
    pub parser: CodexStreamState,
    pub public: JsonState<PublicState>,
}

impl From<CodexStreamState> for ReadState {
    fn from(parser: CodexStreamState) -> Self {
        Self {
            parser,
            public: Default::default(),
        }
    }
}

impl std::ops::Deref for ReadState {
    type Target = CodexStreamState;
    fn deref(&self) -> &Self::Target {
        &self.parser
    }
}

impl std::ops::DerefMut for ReadState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.parser
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct PublicState {
    active: Option<Cycle>,
    closed: BTreeMap<String, (String, ResponseCycleObservation)>,
    closed_bytes: usize,
    finished: BTreeSet<String>,
    cycles_exhausted: bool,
    projection: Option<Projection>,
    call_turns: BTreeMap<String, String>,
    pending_inputs: Vec<(String, NativeMessageObservation)>,
    pending_input_bytes: usize,
    #[serde(skip)]
    record_public_seen: bool,
    #[serde(skip)]
    record_user_seen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Projection {
    id: String,
    turn: String,
    texts: Vec<String>,
    generator: NativeRequestAttribution,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cycle {
    turn: String,
    start: ToolResultPosition,
    started_at_ms: Option<i64>,
    valid: bool,
    inputs: usize,
    messages: Vec<NativeMessageObservation>,
    bytes: usize,
    requester: Option<NativeRequestAttribution>,
    generation_observed: bool,
    open_calls: BTreeSet<String>,
}

impl Cycle {
    fn invalidate(&mut self) {
        self.valid = false;
        self.messages = Vec::new();
        self.open_calls.clear();
        self.bytes = 0;
        self.inputs = 0;
        self.requester = None;
        self.generation_observed = false;
    }
}

impl PublicState {
    pub(super) fn invalidate(&mut self) {
        if let Some(cycle) = &mut self.active {
            cycle.invalidate();
        }
        self.projection = None;
        self.pending_inputs.clear();
        self.pending_input_bytes = 0;
    }

    pub(super) fn begin(
        &mut self,
        record: &Value,
        index: u64,
        context_turn: &Option<String>,
        sink: &mut dyn RecordSink,
    ) {
        self.record_public_seen = false;
        self.record_user_seen = false;
        let payload = &record["payload"];
        if record["type"] == "event_msg"
            && matches!(
                payload["type"].as_str(),
                Some("task_started" | "turn_started")
            )
        {
            let Some(turn) = payload["turn_id"]
                .as_str()
                .and_then(crate::analysis::native_tool_facts::identity)
            else {
                self.invalidate();
                return;
            };
            if self.active.is_some()
                || self.closed.contains_key(&turn)
                || self
                    .finished
                    .contains(&crate::digest::sha256_hex(turn.as_bytes()))
                || self.cycles_exhausted
                || self.closed.len() >= MAX_CYCLE_MESSAGES
            {
                self.invalidate();
                return;
            }
            let position = ToolResultPosition::Jsonl {
                record_index: index,
                item_index: 0,
            };
            let context = context_at(sink, position, "", true);
            let mut cycle = Cycle {
                turn: turn.clone(),
                start: context.first_eligible_position,
                started_at_ms: record.get("timestamp").and_then(parse_ts),
                valid: true,
                inputs: 0,
                messages: Vec::new(),
                bytes: 0,
                requester: None,
                generation_observed: false,
                open_calls: BTreeSet::new(),
            };
            for (input_turn, message) in self.pending_inputs.drain(..) {
                if input_turn == turn {
                    cycle.inputs += 1;
                    add_member(&mut cycle, message, true);
                }
            }
            self.pending_input_bytes = 0;
            self.active = Some(cycle);
        }
        if record["type"] == "response_item"
            && matches!(
                payload["type"].as_str(),
                Some("function_call" | "custom_tool_call" | "tool_search_call" | "web_search_call")
            )
            && let (Some(call), Some(turn)) = (
                payload["call_id"]
                    .as_str()
                    .or_else(|| payload["callId"].as_str()),
                context_turn.as_ref(),
            )
        {
            if self.call_turns.len() < MAX_CYCLE_MESSAGES && call.len() <= 4096 {
                if self
                    .call_turns
                    .insert(
                        crate::digest::sha256_hex(call.as_bytes()),
                        crate::digest::sha256_hex(turn.as_bytes()),
                    )
                    .is_some()
                {
                    self.invalidate();
                }
            } else {
                self.invalidate();
            }
        }
        if matches!(record["type"].as_str(), Some("compacted" | "history_base"))
            || payload["type"] == "thread_rolled_back"
        {
            self.invalidate();
        }
    }

    fn member(
        &mut self,
        message: NativeMessageObservation,
        turn: Option<&str>,
        result_content_complete: bool,
    ) {
        self.record_public_seen |= matches!(&message, NativeMessageObservation::Public(_));
        self.record_user_seen |= matches!(&message, NativeMessageObservation::User(_));
        if let Some(active) = &mut self.active {
            if !active.valid {
                return;
            }
            if turn != Some(active.turn.as_str()) {
                active.invalidate();
                return;
            }
            if matches!(&message, NativeMessageObservation::User(_)) {
                active.inputs += 1;
            }
            match &message {
                NativeMessageObservation::Public(value)
                    if matches!(&value.payload, PublicMessagePayload::ToolCall { .. }) =>
                {
                    active.generation_observed = true;
                    if let PublicMessagePayload::ToolCall { facts, .. } = &value.payload {
                        observe_request(
                            active,
                            &facts
                                .as_ref()
                                .map(|facts| facts.requester.clone())
                                .unwrap_or_default(),
                        );
                    }
                    if let Some(id) = &value.call_id {
                        active
                            .open_calls
                            .insert(crate::digest::sha256_hex(id.as_bytes()));
                    }
                }
                NativeMessageObservation::ToolResult(value) => {
                    if let Some(id) = &value.call_id {
                        active
                            .open_calls
                            .remove(&crate::digest::sha256_hex(id.as_bytes()));
                    }
                }
                NativeMessageObservation::Public(value) => {
                    active.generation_observed = true;
                    if let PublicMessagePayload::AssistantText { generator, .. } = &value.payload {
                        observe_request(active, generator);
                    }
                }
                _ => {}
            }
            add_member(active, message, result_content_complete);
        } else if let (Some(turn), NativeMessageObservation::User(_)) = (turn, &message) {
            let size = serde_json::to_vec(&message)
                .expect("native message encodes")
                .len();
            if self.pending_inputs.len() < MAX_PENDING_INPUTS
                && size <= MAX_CYCLE_BYTES.saturating_sub(self.pending_input_bytes)
            {
                self.pending_input_bytes += size;
                self.pending_inputs.push((turn.to_owned(), message));
            }
        }
    }

    pub(super) fn close_replaced_output(&mut self, record: &Value) {
        let payload = &record["payload"];
        if record["type"] != "response_item"
            || !matches!(
                payload["type"].as_str(),
                Some("custom_tool_call_output" | "function_call_output")
            )
        {
            return;
        }
        let Some(active) = &mut self.active else {
            return;
        };
        if !active.valid {
            return;
        }
        let Some(call) = payload["call_id"]
            .as_str()
            .or_else(|| payload["callId"].as_str())
        else {
            active.invalidate();
            return;
        };
        let call = crate::digest::sha256_hex(call.as_bytes());
        let turn = payload
            .pointer("/internal_chat_message_metadata_passthrough/turn_id")
            .and_then(Value::as_str)
            .or_else(|| payload["turn_id"].as_str());
        let complete = match payload.get("output").or_else(|| payload.get("content")) {
            Some(Value::String(_)) => true,
            Some(Value::Array(parts)) => parts.iter().all(|part| {
                matches!(
                    part["type"].as_str(),
                    Some("input_text" | "output_text" | "text")
                ) && part["text"].is_string()
            }),
            _ => false,
        };
        if self.call_turns.get(&call) != Some(&crate::digest::sha256_hex(active.turn.as_bytes()))
            || turn.is_some_and(|turn| turn != active.turn)
            || !complete
        {
            active.invalidate();
            return;
        }
        // The wrapper closes its own call. Native item IDs do not bind nested work to it.
        active.open_calls.remove(&call);
    }

    pub(super) fn end(
        &mut self,
        record: &Value,
        index: u64,
        admitted: bool,
        session: &str,
        request: (&NativeRequestAttribution, Option<&str>),
        sink: &mut dyn RecordSink,
    ) {
        let (requester, request_turn) = request;
        if !admitted {
            self.invalidate();
            return;
        }
        let payload = &record["payload"];
        if !matches!(
            record["type"].as_str(),
            Some("session_meta" | "turn_context" | "response_item" | "event_msg")
        ) {
            self.invalidate();
            let recognized =
                super::is_recognized_eventless(record["type"].as_str(), payload["type"].as_str());
            let inert = super::is_inert_codex_record(
                record,
                !super::is_proven_echo(record["type"].as_str(), payload["type"].as_str()),
            );
            if !recognized || !inert {
                sink.tool_result_gap();
            }
        }
        if record["type"] == "event_msg"
            && !matches!(
                payload["type"].as_str(),
                Some(
                    "task_started"
                        | "turn_started"
                        | "task_complete"
                        | "turn_complete"
                        | "turn_aborted"
                        | "thread_settings_applied"
                        | "thread_goal_updated"
                        | "item_completed"
                        | "token_count"
                )
            )
        {
            self.invalidate();
            if !super::is_recognized_eventless(record["type"].as_str(), payload["type"].as_str()) {
                sink.tool_result_gap();
            }
        }
        if record["type"] == "response_item" {
            if !matches!(
                payload["type"].as_str(),
                Some(
                    "message"
                        | "reasoning"
                        | "function_call"
                        | "custom_tool_call"
                        | "tool_search_call"
                        | "web_search_call"
                        | "function_call_output"
                        | "custom_tool_call_output"
                        | "tool_search_output"
                        | "mcp_tool_call_output"
                        | "web_search_output"
                )
            ) {
                self.invalidate();
                if !super::is_recognized_eventless(
                    record["type"].as_str(),
                    payload["type"].as_str(),
                ) {
                    sink.tool_result_gap();
                }
            }
            if matches!(
                payload["type"].as_str(),
                Some("function_call" | "custom_tool_call" | "tool_search_call" | "web_search_call")
            ) && !self.record_public_seen
            {
                self.invalidate();
            }
            if payload["type"] == "message"
                && payload["role"] == "user"
                && payload
                    .pointer("/internal_chat_message_metadata_passthrough/content_item_kinds")
                    .and_then(Value::as_array)
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "user.text"))
                && !self.record_user_seen
            {
                self.invalidate();
            }
        }
        if record["type"] == "turn_context"
            && let Some(active) = &mut self.active
        {
            if payload["turn_id"] != active.turn {
                active.invalidate();
            }
            observe_request(active, requester);
        }
        if record["type"] == "response_item"
            && payload["type"] == "reasoning"
            && let Some(active) = &mut self.active
        {
            active.generation_observed = true;
            if request_turn != Some(active.turn.as_str()) {
                active.invalidate();
            }
            observe_request(active, requester);
        }
        if record["type"] == "event_msg"
            && payload["type"] == "item_completed"
            && payload["item"]["type"] == "Reasoning"
            && let Some(active) = &mut self.active
        {
            active.generation_observed = true;
            if payload["thread_id"] != session
                || payload["turn_id"] != active.turn
                || request_turn != Some(active.turn.as_str())
            {
                active.invalidate();
            }
            observe_request(active, requester);
        }
        if record["type"] == "event_msg"
            && payload["type"] == "thread_settings_applied"
            && payload["thread_id"] == session
        {
            let settings = &payload["thread_settings"];
            let mut selection = NativeRequestAttribution::observed(
                settings["model_provider_id"].as_str(),
                settings["model"].as_str(),
            );
            selection.fast_mode = match settings["service_tier"].as_str() {
                Some("priority") => Some(true),
                Some("default") => Some(false),
                _ => None,
            };
            sink.session_selection(crate::analysis::NativeSessionSelectionObservation {
                source_format: crate::analysis::SourceFormat::CodexRolloutJsonl,
                session_id: session.to_owned(),
                position: ToolResultPosition::Jsonl {
                    record_index: index,
                    item_index: 0,
                },
                selection,
            });
        }
        if record["type"] == "event_msg"
            && payload["type"] == "item_completed"
            && payload["item"]["type"] == "AgentMessage"
        {
            if self.projection.is_some() {
                self.invalidate();
            }
            let texts: Option<Vec<_>> = payload["item"]["content"].as_array().and_then(|parts| {
                parts
                    .iter()
                    .map(|part| {
                        (part["type"] == "Text")
                            .then(|| part["text"].as_str().map(str::to_owned))
                            .flatten()
                    })
                    .collect()
            });
            if let (Some(id), Some(turn), Some(texts)) = (
                payload["item"]["id"].as_str(),
                payload["turn_id"].as_str(),
                texts,
            ) {
                if payload["thread_id"] == session
                    && id.len() <= 4096
                    && turn.len() <= 4096
                    && texts.iter().map(String::len).sum::<usize>() <= MAX_CYCLE_BYTES
                {
                    self.projection = Some(Projection {
                        id: id.to_owned(),
                        turn: turn.to_owned(),
                        texts,
                        generator: if request_turn == Some(turn) {
                            requester.clone()
                        } else {
                            NativeRequestAttribution::default()
                        },
                    });
                } else {
                    self.invalidate();
                }
            } else {
                self.invalidate();
            }
        } else if let Some(projection) = self.projection.take() {
            let turn = payload
                .pointer("/internal_chat_message_metadata_passthrough/turn_id")
                .and_then(Value::as_str);
            let parts: Option<Vec<_>> = payload["content"].as_array().and_then(|parts| {
                parts
                    .iter()
                    .enumerate()
                    .map(|(index, part)| {
                        if part["type"] != "output_text" {
                            return None;
                        }
                        Some(UserMessagePart {
                            item_index: index as u32,
                            native_part_id: None,
                            text: part["text"].as_str()?.to_owned(),
                        })
                    })
                    .collect()
            });
            if record["type"] == "response_item"
                && payload["type"] == "message"
                && payload["role"] == "assistant"
                && payload["id"] == projection.id
                && turn == Some(projection.turn.as_str())
                && parts.as_ref().is_some_and(|parts| {
                    parts
                        .iter()
                        .map(|part| &part.text)
                        .eq(projection.texts.iter())
                })
            {
                let parts = parts.expect("matching public parts exist");
                let position = ToolResultPosition::Jsonl {
                    record_index: index,
                    item_index: parts.len() as u32,
                };
                let public = PublicMessageObservation {
                    source_format: crate::analysis::SourceFormat::CodexRolloutJsonl,
                    session_id: session.to_owned(),
                    record_id: projection.id,
                    part_id: None,
                    call_id: None,
                    position,
                    revision: index.saturating_add(1),
                    change: ToolResultChange::Update,
                    eligible: true,
                    completeness: ToolResultCompleteness::Complete,
                    context: Some(NativeEventContext::pending(position, true)),
                    payload: PublicMessagePayload::AssistantText {
                        parts,
                        generator: projection.generator,
                    },
                };
                let finalized =
                    sink.finalize_native_message(NativeMessageObservation::Public(public));
                self.member(finalized.clone(), Some(&projection.turn), true);
                if let NativeMessageObservation::Public(public) = finalized {
                    sink.record(NormalizedRecord::PublicMessage(Box::new(public)));
                }
            } else {
                self.invalidate();
            }
        } else if record["type"] == "response_item"
            && payload["type"] == "message"
            && payload["role"] == "assistant"
        {
            // A raw assistant row without its exact completion cannot complete a cycle.
            self.invalidate();
        }
        let terminal_digest = (record["type"] == "event_msg"
            && matches!(
                payload["type"].as_str(),
                Some("task_complete" | "turn_complete" | "turn_aborted")
            ))
        .then(|| {
            let mut terminal_payload = payload.clone();
            if terminal_payload["type"] == "turn_complete" {
                terminal_payload["type"] = Value::String("task_complete".to_owned());
            }
            crate::digest::sha256_hex(terminal_payload.to_string().as_bytes())
        });
        if terminal_digest.as_ref().is_some_and(|digest| {
            payload["turn_id"]
                .as_str()
                .and_then(|turn| self.closed.get(turn))
                .is_some_and(|(prior_digest, _)| terminal_digest_matches(prior_digest, digest))
        }) {
            return;
        }
        if real_activity(record) {
            crate::analysis::public_events::emit_activity(
                sink,
                crate::analysis::SourceFormat::CodexRolloutJsonl,
                session,
                index,
                payload["type"].as_str().unwrap_or("record"),
                record.get("timestamp").and_then(parse_ts),
            );
        }
        if let Some(digest) = terminal_digest {
            self.terminal(record, index, session, digest, sink);
        }
    }

    fn terminal(
        &mut self,
        record: &Value,
        index: u64,
        session: &str,
        digest: String,
        sink: &mut dyn RecordSink,
    ) {
        let payload = &record["payload"];
        let Some(turn) = payload["turn_id"].as_str() else {
            self.invalidate();
            return;
        };
        if let Some((prior_digest, prior)) = self.closed.get_mut(turn) {
            if terminal_digest_matches(prior_digest, &digest) {
                return;
            }
            retain_terminal_digest(prior_digest, &digest);
            if prior.change != ToolResultChange::Removal {
                prior.change = ToolResultChange::Removal;
                prior.position = context_at(
                    sink,
                    ToolResultPosition::Jsonl {
                        record_index: index,
                        item_index: u32::MAX,
                    },
                    session,
                    true,
                )
                .first_eligible_position;
                prior.revision = match prior.position {
                    ToolResultPosition::FileBytes { record_end, .. } => record_end,
                    _ => index.saturating_add(1),
                };
                sink.record(NormalizedRecord::ResponseCycle(Box::new(prior.clone())));
            }
            return;
        }
        if self.finished.len() < MAX_CYCLE_MESSAGES {
            self.finished
                .insert(crate::digest::sha256_hex(turn.as_bytes()));
        } else {
            self.cycles_exhausted = true;
        }
        let Some(active) = self.active.take() else {
            return;
        };
        if active.turn != turn {
            self.active = Some(active);
            self.invalidate();
            return;
        }
        let status = if payload["type"] == "turn_aborted" {
            if !matches!(
                payload["reason"].as_str(),
                Some("interrupted" | "replaced" | "review_ended" | "budget_limited")
            ) {
                return;
            }
            ResponseCycleStatus::Cancelled
        } else if payload.get("error").is_some_and(|value| !value.is_null()) {
            ResponseCycleStatus::Error
        } else {
            ResponseCycleStatus::Completed
        };
        if !active.valid
            || active.inputs == 0
            || self.projection.is_some()
            || !active.open_calls.is_empty()
        {
            return;
        }
        let position = ToolResultPosition::Jsonl {
            record_index: index,
            item_index: u32::MAX,
        };
        let context = context_at(sink, position, session, true);
        let requester = if active.generation_observed {
            active.requester.unwrap_or_default()
        } else {
            NativeRequestAttribution::default()
        };
        let cycle = ResponseCycleObservation {
            source_format: crate::analysis::SourceFormat::CodexRolloutJsonl,
            session_id: session.to_owned(),
            key: NativeCycleKey::CodexTurn {
                turn_id: turn.to_owned(),
            },
            start_position: active.start,
            position: context.first_eligible_position,
            revision: match context.first_eligible_position {
                ToolResultPosition::FileBytes { record_end, .. } => record_end,
                _ => index.saturating_add(1),
            },
            change: ToolResultChange::Update,
            status,
            native_reason: payload["reason"]
                .as_str()
                .filter(|reason| reason.len() <= 4096)
                .map(str::to_owned),
            native_error_code: native_error_code(payload),
            started_at_ms: active.started_at_ms,
            ended_at_ms: record.get("timestamp").and_then(parse_ts),
            requester,
            context,
            messages: active.messages,
        };
        if serde_json::to_vec(&cycle)
            .expect("native cycle encodes")
            .len()
            > MAX_CYCLE_BYTES
        {
            return;
        }
        let mut removal = cycle.clone();
        removal.messages = Vec::new();
        removal.context.user_messages = Vec::new();
        let size = serde_json::to_vec(&(turn, &digest, &removal))
            .expect("cycle removal encodes")
            .len();
        if self.closed.len() >= MAX_CYCLE_MESSAGES
            || size > MAX_CYCLE_BYTES.saturating_sub(self.closed_bytes)
        {
            return;
        }
        self.closed_bytes += size;
        self.closed.insert(turn.to_owned(), (digest, removal));
        sink.record(NormalizedRecord::ResponseCycle(Box::new(cycle)));
    }
}

fn terminal_digest_matches(retained: &str, digest: &str) -> bool {
    let (original, current) = retained.split_once(':').unwrap_or((retained, retained));
    original == digest || current == digest
}

fn retain_terminal_digest(retained: &mut String, digest: &str) {
    // Keep both proofs in the existing string field so old cursors still decode.
    let original = retained
        .split_once(':')
        .map_or(retained.as_str(), |(original, _)| original);
    *retained = format!("{original}:{digest}");
}

fn native_error_code(payload: &Value) -> Option<String> {
    let info = payload.pointer("/error/codex_error_info")?;
    let code = match info {
        Value::String(code) => code.as_str(),
        Value::Object(fields) if fields.len() == 1 => fields.keys().next()?.as_str(),
        _ => return None,
    };
    (code.len() <= 4096).then(|| code.to_owned())
}

fn observe_request(cycle: &mut Cycle, request: &NativeRequestAttribution) {
    if !cycle.valid {
        return;
    }
    let Some(prior) = &mut cycle.requester else {
        cycle.requester = Some(request.clone());
        return;
    };
    if prior.provider != request.provider || prior.model != request.model {
        prior.provider = None;
        prior.model = None;
    }
    if prior.fast_mode != request.fast_mode {
        prior.fast_mode = None;
    }
}

fn add_member(cycle: &mut Cycle, message: NativeMessageObservation, result_content_complete: bool) {
    if !cycle.valid {
        return;
    }
    let admitted = match &message {
        NativeMessageObservation::User(value) => {
            value.message.change == crate::analysis::UserMessageChange::Present
                && value.message.completeness == crate::analysis::UserMessageCompleteness::Complete
        }
        NativeMessageObservation::Public(value) => {
            value.eligible
                && value.change == ToolResultChange::Update
                && value.completeness == ToolResultCompleteness::Complete
        }
        NativeMessageObservation::ToolResult(value) => {
            result_content_complete
                && value.finality
                && value.change == ToolResultChange::Update
                && matches!(
                    value.completeness,
                    ToolResultCompleteness::Complete | ToolResultCompleteness::Incomplete
                )
                && value.content.is_some()
                && value.tool_name.is_some()
                && value.arguments.is_some()
        }
    };
    let size = serde_json::to_vec(&message)
        .expect("native member encodes")
        .len();
    if !admitted
        || cycle.messages.len() >= MAX_CYCLE_MESSAGES
        || size > MAX_CYCLE_BYTES.saturating_sub(cycle.bytes)
    {
        cycle.invalidate();
        return;
    }
    cycle.bytes += size;
    cycle.messages.push(message);
}

fn context_at(
    sink: &mut dyn RecordSink,
    position: ToolResultPosition,
    session: &str,
    main: bool,
) -> NativeEventContext {
    sink.native_context(
        crate::analysis::SourceFormat::CodexRolloutJsonl,
        session,
        position,
        main,
    )
}

fn real_activity(record: &Value) -> bool {
    if record["type"] == "response_item"
        && record["payload"]["type"] == "message"
        && !matches!(
            record["payload"]["role"].as_str(),
            Some("user" | "assistant")
        )
    {
        return false;
    }
    if record["type"] == "response_item"
        && record["payload"]["type"] == "message"
        && record["payload"]["role"] == "user"
    {
        return record["metadata"]["retained_source"]["complete"] == true
            && record
                .pointer("/payload/internal_chat_message_metadata_passthrough/content_item_kinds")
                .and_then(Value::as_array)
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "user.text"));
    }
    match record["type"].as_str() {
        Some("response_item") => matches!(
            record["payload"]["type"].as_str(),
            Some(
                "message"
                    | "reasoning"
                    | "function_call"
                    | "custom_tool_call"
                    | "function_call_output"
                    | "custom_tool_call_output"
                    | "tool_search_call"
                    | "web_search_call"
                    | "tool_search_output"
                    | "web_search_output"
            )
        ),
        Some("event_msg") => matches!(
            record["payload"]["type"].as_str(),
            Some(
                "task_started"
                    | "turn_started"
                    | "task_complete"
                    | "turn_complete"
                    | "turn_aborted"
                    | "item_completed"
                    | "agent_message"
                    | "agent_reasoning"
                    | "exec_command_begin"
                    | "exec_command_end"
            )
        ),
        _ => false,
    }
}

pub(super) struct CycleSink<'a> {
    state: &'a mut PublicState,
    sink: &'a mut dyn RecordSink,
    turn: Option<String>,
    result_content_complete: Option<bool>,
}

impl<'a> CycleSink<'a> {
    pub(super) fn new(
        state: &'a mut PublicState,
        record: &Value,
        context_turn: Option<&str>,
        sink: &'a mut dyn RecordSink,
    ) -> Self {
        let payload = &record["payload"];
        let turn = payload
            .pointer("/internal_chat_message_metadata_passthrough/turn_id")
            .and_then(Value::as_str)
            .or_else(|| payload["turn_id"].as_str())
            .or_else(|| {
                payload["call_id"]
                    .as_str()
                    .and_then(|call| {
                        state
                            .call_turns
                            .get(&crate::digest::sha256_hex(call.as_bytes()))
                    })
                    .and_then(|turn| {
                        state
                            .active
                            .as_ref()
                            .filter(|active| {
                                crate::digest::sha256_hex(active.turn.as_bytes()) == *turn
                            })
                            .map(|active| active.turn.as_str())
                    })
            })
            .or_else(|| {
                (record["type"] == "response_item" && payload["role"] != "user")
                    .then_some(context_turn)
                    .flatten()
            })
            .map(str::to_owned);
        let result_content_complete = (record["type"] == "response_item"
            && matches!(
                payload["type"].as_str(),
                Some(
                    "function_call_output"
                        | "custom_tool_call_output"
                        | "tool_search_output"
                        | "mcp_tool_call_output"
                        | "web_search_output"
                )
            ))
        .then(|| codex_result_content(payload).1);
        Self {
            state,
            sink,
            turn,
            result_content_complete,
        }
    }
}

impl RecordSink for CycleSink<'_> {
    fn record(&mut self, record: NormalizedRecord) {
        let message = match record {
            NormalizedRecord::ToolResult(value) => {
                Some(NativeMessageObservation::ToolResult(*value))
            }
            NormalizedRecord::PublicMessage(value) => {
                Some(NativeMessageObservation::Public(*value))
            }
            other => {
                if matches!(other, NormalizedRecord::Unusable(_)) {
                    self.state.invalidate();
                }
                self.sink.record(other);
                return;
            }
        };
        if let Some(message) = message {
            let result_content_complete = match &message {
                NativeMessageObservation::ToolResult(value) => self
                    .result_content_complete
                    .unwrap_or(value.completeness == ToolResultCompleteness::Complete),
                _ => true,
            };
            let message = self.sink.finalize_native_message(message);
            self.state.member(
                message.clone(),
                self.turn.as_deref(),
                result_content_complete,
            );
            match message {
                NativeMessageObservation::ToolResult(value) => self
                    .sink
                    .record(NormalizedRecord::ToolResult(Box::new(value))),
                NativeMessageObservation::Public(value) => self
                    .sink
                    .record(NormalizedRecord::PublicMessage(Box::new(value))),
                NativeMessageObservation::User(_) => unreachable!(),
            }
        }
    }
    fn user_message(&mut self, message: UserMessage) {
        if let Some(value) = user_observation(message.clone()) {
            let value = self
                .sink
                .finalize_native_message(NativeMessageObservation::User(value));
            self.state.member(value, self.turn.as_deref(), true);
        }
        self.sink.user_message(message);
    }
    fn tool_result_gap(&mut self) {
        self.state.invalidate();
        self.sink.tool_result_gap();
    }
    fn tool_result_admission_gap(&mut self, admission: crate::analysis::ToolResultAdmission) {
        self.state.invalidate();
        self.sink.tool_result_admission_gap(admission);
    }
    fn native_admission_failure(&mut self, failure: crate::analysis::NativeAdmissionFailure) {
        self.state.invalidate();
        self.sink.native_admission_failure(failure);
    }
    fn finish(&mut self, summary: SessionSummary) {
        self.sink.finish(summary);
    }
}

#[cfg(test)]
mod memory_tests;
