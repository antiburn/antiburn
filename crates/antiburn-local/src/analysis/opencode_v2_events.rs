//! Bounded join core for the pinned OpenCode V2 durable event payloads.
//!
//! This module deliberately returns an intermediate observation. It does not
//! prove that a desktop event belongs to a server-side session database row.

use std::collections::{BTreeMap, HashMap, VecDeque};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::analysis::{
    EVIDENCE_STRING_CAP, NativeToolOutcome, SourceFormat, ToolCategory, ToolResultChange,
    ToolResultCompleteness, ToolResultContext, ToolResultObservation, ToolResultPosition,
    ToolResultStatus, UserMessage, UserMessageChange, UserMessageCompleteness, UserMessageOrigin,
    UserMessagePart, UserMessagePosition, UserMessageScope,
};

pub const MAX_RETAINED_CALLS: usize = 256;
pub const MAX_RETAINED_USERS: usize = 128;
pub const MAX_STRING_BYTES: usize = 16 * 1024;
pub const MAX_RETAINED_PAYLOAD_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gap {
    MissingSequence,
    SequenceDiscontinuity,
    ContinuityLost,
    MissingJoin,
    ConflictingDuplicate,
    CapacityExceeded,
    UnknownShape,
    UnsupportedVersion,
    OptionalContentOmitted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalStatus {
    Success,
    Failed,
}

/// A conservative terminal tool observation joined to V2 durable events.
#[derive(Debug, Clone, PartialEq)]
pub struct JoinedToolResult {
    pub session_id: String,
    pub assistant_message_id: String,
    pub call_id: String,
    pub event_id: String,
    pub aggregate_id: String,
    pub sequence: u64,
    pub created: Option<f64>,
    pub tool_name: String,
    pub input: Option<Value>,
    pub request_message_id: Option<String>,
    pub request_sequence: Option<u64>,
    pub request_text: Option<String>,
    pub user_messages: Vec<JoinedUserMessage>,
    pub status: TerminalStatus,
    /// Text content is populated only when every content item is text.
    pub content: Option<String>,
    pub outcome: Option<NativeToolOutcome>,
    pub provider_executed: Option<bool>,
    pub gaps: Vec<Gap>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinedUserMessage {
    pub message_id: String,
    pub sequence: u64,
    pub text: Option<String>,
    pub complete: bool,
}

impl JoinedToolResult {
    /// Convert a joined V2 terminal event to the shared native-result contract.
    pub fn into_observation(
        self,
        complete_from_boundary: bool,
        main_session: bool,
    ) -> ToolResultObservation {
        let user_messages_complete = !self.user_messages.is_empty()
            && self.session_id.len() <= EVIDENCE_STRING_CAP
            && self.user_messages.iter().all(|message| {
                message.complete
                    && message.message_id.len() <= EVIDENCE_STRING_CAP
                    && message
                        .text
                        .as_ref()
                        .is_some_and(|text| !text.is_empty() && text.len() <= MAX_STRING_BYTES)
            });
        let user_messages: Vec<_> = self
            .user_messages
            .iter()
            .filter(|_| main_session)
            .filter_map(|message| {
                let text = message.text.as_ref()?;
                let user_message = UserMessage {
                    source_format: SourceFormat::OpenCodeSqliteV2,
                    session_id: self.session_id.clone(),
                    message_id: message.message_id.clone(),
                    position: UserMessagePosition::OpenCodeV2Event {
                        sequence: message.sequence,
                    },
                    origin: UserMessageOrigin::OpenCodeUserText,
                    scope: UserMessageScope::MainSession,
                    change: UserMessageChange::Present,
                    completeness: if message.complete {
                        UserMessageCompleteness::Complete
                    } else {
                        UserMessageCompleteness::Incomplete
                    },
                    parts: vec![UserMessagePart {
                        item_index: 0,
                        native_part_id: None,
                        text: text.clone(),
                    }],
                    proof_revision: 1,
                };
                user_message.is_bounded().then_some(user_message)
            })
            .collect();
        let complete_through_event = main_session
            && complete_from_boundary
            && self.gaps.is_empty()
            && user_messages_complete
            && user_messages
                .iter()
                .all(|message| message.completeness == UserMessageCompleteness::Complete);
        let request_is_complete = self.request_message_id.is_some()
            && self.request_sequence.is_some()
            && self
                .request_text
                .as_ref()
                .is_some_and(|text| !text.is_empty());
        let identity_is_bounded = self.session_id.len() <= EVIDENCE_STRING_CAP
            && self.assistant_message_id.len() <= EVIDENCE_STRING_CAP
            && self.call_id.len() <= EVIDENCE_STRING_CAP
            && self.event_id.len() <= EVIDENCE_STRING_CAP
            && self.tool_name.len() <= EVIDENCE_STRING_CAP;
        let completeness = if !identity_is_bounded {
            ToolResultCompleteness::UnsafeIdentity
        } else if !main_session {
            ToolResultCompleteness::UnsafeOwnership
        } else if self.gaps.is_empty()
            && request_is_complete
            && complete_through_event
            && self.input.is_some()
            && self.content.is_some()
        {
            ToolResultCompleteness::Complete
        } else {
            ToolResultCompleteness::Incomplete
        };
        let context = ToolResultContext {
            result_position: ToolResultPosition::OpenCodeV2Event {
                sequence: self.sequence,
                item_index: 0,
            },
            main_session,
            complete_through_event,
            user_messages,
        };
        let tool_category = if self.provider_executed == Some(true) {
            ToolCategory::Other
        } else {
            match self.tool_name.as_str() {
                "execute" => ToolCategory::Other,
                "patch" => ToolCategory::Edit,
                "shell" => {
                    crate::analysis::ToolCall::with_command(
                        &self.tool_name,
                        self.input
                            .as_ref()
                            .and_then(|input| input.get("command"))
                            .and_then(Value::as_str),
                    )
                    .category
                }
                _ => ToolCategory::from_tool_name(&self.tool_name),
            }
        };
        ToolResultObservation {
            source_format: SourceFormat::OpenCodeSqliteV2,
            session_id: self.session_id,
            record_id: Some(self.assistant_message_id),
            call_id: Some(self.call_id),
            result_id: Some(self.event_id),
            position: ToolResultPosition::OpenCodeV2Event {
                sequence: self.sequence,
                item_index: 0,
            },
            revision: 0,
            change: ToolResultChange::Update,
            tool_category: Some(tool_category),
            tool_name: Some(self.tool_name),
            arguments: self.input,
            content: self.content,
            status: match self.status {
                TerminalStatus::Success => ToolResultStatus::Success,
                TerminalStatus::Failed => ToolResultStatus::Error,
            },
            finality: true,
            completeness,
            context: Some(context),
            outcome: self.outcome,
            provider_executed: self.provider_executed,
        }
    }
}

#[derive(Debug, Clone)]
struct UserRequest {
    sequence: u64,
    message_id: String,
    text: Option<String>,
    complete: bool,
}

#[derive(Debug, Clone)]
enum PendingInbox {
    User(UserRequest),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StepContext {
    request_message_id: Option<String>,
    request_sequence: Option<u64>,
    request_text: Option<String>,
    gaps: Vec<Gap>,
    user_messages: Vec<JoinedUserMessage>,
}

#[derive(Debug, Clone)]
struct Call {
    name: Option<String>,
    input: Option<Value>,
    request_message_id: Option<String>,
    request_sequence: Option<u64>,
    request_text: Option<String>,
    gaps: Vec<Gap>,
    user_messages: Vec<JoinedUserMessage>,
    terminal: Option<TerminalFingerprint>,
    provider_executed: Option<bool>,
}

#[derive(Debug, Clone)]
struct TerminalFingerprint {
    status: TerminalStatus,
    content: Option<[u8; 32]>,
    outcome: Option<NativeToolOutcome>,
    provider_executed: Option<bool>,
}

struct TerminalEvent<'a> {
    event: &'a Value,
    event_id: &'a str,
    aggregate_id: &'a str,
    sequence: u64,
    session_id: &'a str,
    data: &'a serde_json::Map<String, Value>,
    status: TerminalStatus,
}

/// Joins the durable V2 event envelope by native session/message/call IDs.
/// Feed events in delivery order. Sequence numbers are authoritative for
/// ordering; timestamps are retained for display only.
#[derive(Debug, Default)]
pub struct OpenCodeV2EventJoiner {
    session_id: Option<String>,
    last_sequence: Option<u64>,
    request_history_gap: bool,
    seen_event_ids: HashMap<String, [u8; 32]>,
    seen_event_id_order: VecDeque<String>,
    calls: BTreeMap<(String, String), Call>,
    pending_inbox: HashMap<String, PendingInbox>,
    users: Vec<UserRequest>,
    steps: HashMap<String, StepContext>,
    gaps: Vec<Gap>,
}

impl OpenCodeV2EventJoiner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one event. Returns a result only for the first terminal event
    /// that can be joined to a known call. Unknown or unsafe input is retained
    /// as an explicit gap and never guessed into a result.
    pub fn observe(&mut self, event: &Value) -> Option<JoinedToolResult> {
        let result = self.observe_event(event);
        if self.retained_payload_bytes() > MAX_RETAINED_PAYLOAD_BYTES {
            self.calls.clear();
            self.pending_inbox.clear();
            self.users.clear();
            self.steps.clear();
            self.seen_event_ids.clear();
            self.seen_event_id_order.clear();
            self.request_history_gap = true;
            self.gap(Gap::CapacityExceeded);
            return None;
        }
        result
    }

    fn observe_event(&mut self, event: &Value) -> Option<JoinedToolResult> {
        let (Some(event_id), Some(event_type), Some(data)) = (
            string(event, "id"),
            string(event, "type"),
            event.get("data").and_then(Value::as_object),
        ) else {
            self.gap(Gap::UnknownShape);
            return None;
        };
        let Some(expected_version) = event_schema_version(event_type) else {
            self.gap(Gap::UnknownShape);
            return None;
        };
        if event.pointer("/durable/version").and_then(Value::as_u64) != Some(expected_version) {
            self.gap(Gap::UnsupportedVersion);
            return None;
        }
        if event_id.len() > MAX_STRING_BYTES {
            self.gap(Gap::UnknownShape);
            return None;
        }
        let Ok(encoded_event) = serde_json::to_vec(event) else {
            self.gap(Gap::UnknownShape);
            return None;
        };
        if encoded_event.len() > 256 * 1024 {
            self.gap(Gap::CapacityExceeded);
            return None;
        }
        let digest: [u8; 32] = Sha256::digest(&encoded_event).into();
        if let Some(previous) = self.seen_event_ids.get(event_id) {
            if previous != &digest {
                self.gap(Gap::ConflictingDuplicate);
            }
            return None;
        }
        // Keep exact duplicate checks bounded. Sequence checks still reject older replays.
        while self.seen_event_ids.len() >= MAX_RETAINED_CALLS * 8 {
            let Some(expired) = self.seen_event_id_order.pop_front() else {
                break;
            };
            self.seen_event_ids.remove(&expired);
        }
        self.seen_event_ids.insert(event_id.to_owned(), digest);
        self.seen_event_id_order.push_back(event_id.to_owned());

        let Some(sequence) = event.pointer("/durable/seq").and_then(Value::as_u64) else {
            self.gap(Gap::MissingSequence);
            return None;
        };
        let Some(aggregate_id) = event
            .pointer("/durable/aggregateID")
            .and_then(Value::as_str)
        else {
            self.gap(Gap::UnknownShape);
            return None;
        };
        if aggregate_id.len() > MAX_STRING_BYTES {
            self.gap(Gap::UnknownShape);
            return None;
        }
        if self
            .last_sequence
            .is_some_and(|previous| sequence <= previous)
        {
            self.gap(Gap::SequenceDiscontinuity);
        }
        self.last_sequence = Some(sequence);

        let Some(session_id) = bounded_field(data, "sessionID") else {
            self.gap(Gap::UnknownShape);
            return None;
        };
        if let Some(known) = &self.session_id {
            if known != &session_id || aggregate_id != known {
                self.gap(Gap::UnknownShape);
                return None;
            }
        } else {
            self.session_id = Some(session_id.clone());
            if aggregate_id != session_id {
                self.gap(Gap::UnknownShape);
                return None;
            }
        }

        match event_type {
            "session.updated" => {
                if data
                    .get("info")
                    .and_then(|info| info.get("id"))
                    .and_then(Value::as_str)
                    != Some(session_id.as_str())
                {
                    self.gap(Gap::UnknownShape);
                }
                None
            }
            "session.inbox.enqueued" => {
                self.observe_user_enqueued(sequence, data);
                None
            }
            "session.inbox.delivered" => {
                self.observe_user_delivered(sequence, data);
                None
            }
            "session.inbox.cancelled" => {
                self.observe_inbox_cancelled(data);
                None
            }
            "session.step.started" => {
                self.observe_step_started(sequence, data);
                None
            }
            "session.tool.input.started" => {
                self.observe_tool_started(data);
                None
            }
            "session.tool.input.ended" => {
                self.observe_tool_input_ended(data);
                None
            }
            "session.tool.called" => {
                self.observe_tool_called(sequence, data);
                None
            }
            "session.tool.success" => self.observe_terminal(TerminalEvent {
                event,
                event_id,
                aggregate_id,
                sequence,
                session_id: &session_id,
                data,
                status: TerminalStatus::Success,
            }),
            "session.tool.failed" => self.observe_terminal(TerminalEvent {
                event,
                event_id,
                aggregate_id,
                sequence,
                session_id: &session_id,
                data,
                status: TerminalStatus::Failed,
            }),
            "session.deleted"
            | "session.moved"
            | "session.forked"
            | "session.message.content.updated"
            | "session.revert.staged"
            | "session.revert.cleared"
            | "session.revert.committed" => {
                self.gap(Gap::ContinuityLost);
                None
            }
            "session.created"
            | "session.agent.selected"
            | "session.model.selected"
            | "session.renamed"
            | "session.metadata.updated"
            | "session.permissions"
            | "session.viewed"
            | "session.usage.recorded"
            | "session.usage.updated"
            | "session.inbox.delivery.changed"
            | "session.execution.started"
            | "session.execution.succeeded"
            | "session.execution.failed"
            | "session.execution.interrupted"
            | "session.instructions.updated"
            | "session.synthetic"
            | "session.skill.activated"
            | "session.shell.started"
            | "session.shell.ended"
            | "session.step.streamed"
            | "session.step.ended"
            | "session.step.failed"
            | "session.text.started"
            | "session.text.delta"
            | "session.text.ended"
            | "session.reasoning.started"
            | "session.reasoning.delta"
            | "session.reasoning.ended"
            | "session.tool.input.delta"
            | "session.tool.progress"
            | "session.retry.scheduled"
            | "session.compaction.started"
            | "session.compaction.delta"
            | "session.compaction.ended"
            | "session.compaction.failed" => None,
            _ => {
                self.gap(Gap::UnknownShape);
                None
            }
        }
    }

    fn retained_payload_bytes(&self) -> usize {
        let users = |messages: &[JoinedUserMessage]| {
            messages
                .iter()
                .map(|message| {
                    message.message_id.len() + message.text.as_ref().map_or(0, String::len)
                })
                .sum::<usize>()
        };
        let mut bytes = self.session_id.as_ref().map_or(0, String::len)
            + self
                .seen_event_ids
                .keys()
                .map(|id| id.len() + 32)
                .sum::<usize>()
            + self.pending_inbox.keys().map(String::len).sum::<usize>();
        for request in self
            .users
            .iter()
            .chain(
                self.pending_inbox
                    .values()
                    .filter_map(|pending| match pending {
                        PendingInbox::User(user) => Some(user),
                        PendingInbox::Other => None,
                    }),
            )
        {
            bytes += request.message_id.len() + request.text.as_ref().map_or(0, String::len);
        }
        for (id, context) in &self.steps {
            bytes += id.len()
                + context.request_message_id.as_ref().map_or(0, String::len)
                + context.request_text.as_ref().map_or(0, String::len)
                + users(&context.user_messages);
        }
        for ((message_id, call_id), call) in &self.calls {
            bytes += message_id.len()
                + call_id.len()
                + call.name.as_ref().map_or(0, String::len)
                + call.input.as_ref().map_or(0, json_payload_bytes)
                + call.request_message_id.as_ref().map_or(0, String::len)
                + call.request_text.as_ref().map_or(0, String::len)
                + users(&call.user_messages);
            bytes += usize::from(call.terminal.is_some()) * 32;
            if let Some(outcome) = call
                .terminal
                .as_ref()
                .and_then(|terminal| terminal.outcome.as_ref())
            {
                bytes += outcome.retained_string_bytes();
            }
        }
        bytes
    }

    pub fn gaps(&self) -> &[Gap] {
        &self.gaps
    }

    /// Mark transport continuity loss. Later joined results retain this gap
    /// until the caller creates a new joiner for a separately proven segment.
    pub fn mark_gap(&mut self) {
        self.gap(Gap::ContinuityLost);
    }

    fn observe_step_started(&mut self, sequence: u64, data: &serde_json::Map<String, Value>) {
        let Some(assistant_message_id) = bounded_field(data, "assistantMessageID") else {
            self.gap(Gap::UnknownShape);
            return;
        };
        if self.steps.len() >= MAX_RETAINED_CALLS && !self.steps.contains_key(&assistant_message_id)
        {
            self.gap(Gap::CapacityExceeded);
            return;
        }
        let user_messages: Vec<_> = self
            .users
            .iter()
            .filter(|user| user.sequence < sequence)
            .map(|user| JoinedUserMessage {
                message_id: user.message_id.clone(),
                sequence: user.sequence,
                text: user.text.clone(),
                complete: user.complete,
            })
            .collect();
        let request = self
            .users
            .iter()
            .filter(|user| user.sequence < sequence)
            .max_by_key(|user| user.sequence)
            .map(|user| StepContext {
                request_message_id: Some(user.message_id.clone()),
                request_sequence: Some(user.sequence),
                request_text: user.text.clone(),
                gaps: if user.complete {
                    Vec::new()
                } else {
                    vec![Gap::MissingJoin]
                },
                user_messages: user_messages.clone(),
            });
        if self.request_history_gap
            || request.as_ref().is_none_or(|context| {
                context.request_text.as_ref().is_none_or(String::is_empty)
                    || context.gaps.contains(&Gap::MissingJoin)
            })
        {
            self.gap(Gap::MissingJoin);
        }
        let mut context = request.unwrap_or(StepContext {
            request_message_id: None,
            request_sequence: None,
            request_text: None,
            gaps: Vec::new(),
            user_messages: Vec::new(),
        });
        if context.request_message_id.is_none()
            || context.request_text.as_ref().is_none_or(String::is_empty)
            || self.request_history_gap
        {
            push_gap(&mut context.gaps, Gap::MissingJoin);
        }
        if let Some(existing) = self.steps.get(&assistant_message_id) {
            if existing != &context {
                self.gap(Gap::ConflictingDuplicate);
            }
            return;
        }
        self.steps.insert(assistant_message_id, context);
    }

    fn observe_user_enqueued(&mut self, sequence: u64, data: &serde_json::Map<String, Value>) {
        let Some(inbox_id) = bounded_field(data, "inboxID") else {
            self.gap(Gap::UnknownShape);
            return;
        };
        if self.pending_inbox.len() + self.users.len() >= MAX_RETAINED_USERS {
            self.gap(Gap::CapacityExceeded);
            return;
        }
        let item = data.get("item").and_then(Value::as_object);
        let pending = match item
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
        {
            Some("user") => {
                let payload = item
                    .and_then(|item| item.get("payload"))
                    .and_then(Value::as_object);
                let text = payload
                    .and_then(|payload| payload.get("text"))
                    .and_then(Value::as_str);
                let complete = text
                    .is_some_and(|text| !text.is_empty() && text.len() <= MAX_STRING_BYTES)
                    && ["files", "agents", "skills"].iter().all(|field| {
                        payload
                            .and_then(|payload| payload.get(*field))
                            .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty))
                    });
                if text.is_none_or(|text| text.is_empty() || text.len() > MAX_STRING_BYTES) {
                    self.gap(Gap::MissingJoin);
                }
                PendingInbox::User(UserRequest {
                    sequence,
                    message_id: inbox_id.clone(),
                    text: text
                        .filter(|text| text.len() <= MAX_STRING_BYTES)
                        .map(str::to_owned),
                    complete,
                })
            }
            Some("synthetic" | "compaction" | "move") => PendingInbox::Other,
            _ => {
                self.gap(Gap::UnknownShape);
                PendingInbox::Other
            }
        };
        if self.pending_inbox.insert(inbox_id, pending).is_some() {
            self.gap(Gap::ConflictingDuplicate);
        }
    }

    fn observe_user_delivered(&mut self, sequence: u64, data: &serde_json::Map<String, Value>) {
        let Some(message_id) = bounded_field(data, "inboxID") else {
            self.gap(Gap::UnknownShape);
            return;
        };
        let mut request = match self.pending_inbox.remove(&message_id) {
            Some(PendingInbox::User(request)) => request,
            Some(PendingInbox::Other) => return,
            None => {
                self.gap(Gap::MissingJoin);
                self.request_history_gap = true;
                return;
            }
        };
        request.sequence = sequence;
        if self.request_history_gap || !request.complete {
            self.users.clear();
        }
        if self.request_history_gap && request.complete {
            self.request_history_gap = false;
        } else if !request.complete {
            self.request_history_gap = true;
        }
        if self.users.len() >= MAX_RETAINED_USERS {
            self.gap(Gap::CapacityExceeded);
            return;
        }
        self.users.push(request);
    }

    fn observe_inbox_cancelled(&mut self, data: &serde_json::Map<String, Value>) {
        let Some(message_id) = bounded_field(data, "inboxID") else {
            self.gap(Gap::UnknownShape);
            return;
        };
        self.pending_inbox.remove(&message_id);
    }

    fn observe_tool_started(&mut self, data: &serde_json::Map<String, Value>) {
        let (Some(message_id), Some(call_id), Some(name)) = (
            bounded_field(data, "assistantMessageID"),
            bounded_field(data, "id"),
            bounded_field(data, "name"),
        ) else {
            self.gap(Gap::UnknownShape);
            return;
        };
        if !self.ensure_call(message_id.clone(), call_id.clone()) {
            return;
        }
        let call = self
            .calls
            .get_mut(&(message_id, call_id))
            .expect("inserted call");
        if call.name.as_ref().is_some_and(|existing| existing != &name) {
            self.gap(Gap::ConflictingDuplicate);
            return;
        }
        call.name = Some(name);
    }

    fn observe_tool_input_ended(&mut self, data: &serde_json::Map<String, Value>) {
        let (Some(message_id), Some(call_id), Some(text)) = (
            bounded_field(data, "assistantMessageID"),
            bounded_field(data, "id"),
            data.get("text").and_then(Value::as_str),
        ) else {
            self.gap(Gap::UnknownShape);
            return;
        };
        if text.len() > MAX_STRING_BYTES {
            self.gap(Gap::OptionalContentOmitted);
            return;
        }
        if let Ok(input) = serde_json::from_str::<Value>(text)
            && self.ensure_call(message_id.clone(), call_id.clone())
        {
            self.calls
                .get_mut(&(message_id, call_id))
                .expect("inserted call")
                .input = Some(input);
        }
    }

    fn observe_tool_called(&mut self, _sequence: u64, data: &serde_json::Map<String, Value>) {
        if data
            .get("executed")
            .is_some_and(|value| !value.is_boolean())
        {
            self.gap(Gap::UnknownShape);
            return;
        }
        let (Some(message_id), Some(call_id), Some(input)) = (
            bounded_field(data, "assistantMessageID"),
            bounded_field(data, "id"),
            data.get("input"),
        ) else {
            self.gap(Gap::UnknownShape);
            return;
        };
        if input.to_string().len() > MAX_STRING_BYTES
            || !self.ensure_call(message_id.clone(), call_id.clone())
        {
            self.gap(Gap::CapacityExceeded);
            return;
        }
        let context = self.steps.get(&message_id).cloned();
        let mut call_gaps = context
            .as_ref()
            .map(|context| context.gaps.clone())
            .unwrap_or_default();
        if context.as_ref().is_none_or(|context| {
            context.request_message_id.is_none()
                || context.request_sequence.is_none()
                || context.request_text.as_ref().is_none_or(String::is_empty)
        }) {
            self.gap(Gap::MissingJoin);
            push_gap(&mut call_gaps, Gap::MissingJoin);
        }
        let call = self
            .calls
            .get_mut(&(message_id, call_id))
            .expect("inserted call");
        call.input = Some(input.clone());
        call.provider_executed = data.get("executed").and_then(Value::as_bool);
        call.gaps.extend(call_gaps);
        if let Some(context) = context {
            call.request_message_id = context.request_message_id;
            call.request_sequence = context.request_sequence;
            call.request_text = context.request_text;
            call.user_messages = context.user_messages;
        }
    }

    fn observe_terminal(&mut self, terminal: TerminalEvent<'_>) -> Option<JoinedToolResult> {
        let TerminalEvent {
            event,
            event_id,
            aggregate_id,
            sequence,
            session_id,
            data,
            status,
        } = terminal;
        let (Some(message_id), Some(call_id)) = (
            bounded_field(data, "assistantMessageID"),
            bounded_field(data, "id"),
        ) else {
            self.gap(Gap::UnknownShape);
            return None;
        };
        let Some(call) = self.calls.get(&(message_id.clone(), call_id.clone())) else {
            self.gap(Gap::MissingJoin);
            return None;
        };
        let Some(name) = call.name.clone() else {
            self.gap(Gap::MissingJoin);
            return None;
        };
        let input = call.input.clone();
        let request_message_id = call.request_message_id.clone();
        let request_sequence = call.request_sequence;
        let request_text = call.request_text.clone();
        let call_gaps = call.gaps.clone();
        let user_messages = call.user_messages.clone();
        let mut result_gaps: Vec<_> = self
            .gaps
            .iter()
            .filter(|gap| **gap != Gap::MissingJoin)
            .cloned()
            .collect();
        for gap in call_gaps {
            push_gap(&mut result_gaps, gap);
        }
        let prior_terminal = call.terminal.clone();
        if data
            .get("executed")
            .is_some_and(|value| !value.is_boolean())
        {
            self.gap(Gap::UnknownShape);
            return None;
        }
        let terminal_executed = data.get("executed").and_then(Value::as_bool);
        if call
            .provider_executed
            .zip(terminal_executed)
            .is_some_and(|(called, terminal)| called && !terminal)
        {
            self.gap(Gap::ConflictingDuplicate);
            return None;
        }
        let provider_executed = terminal_executed.or(call.provider_executed);
        let outcome = NativeToolOutcome::opencode_v2_tool(
            &name,
            data.get("metadata"),
            provider_executed,
            status == TerminalStatus::Failed,
        );
        let content = decode_content(data.get("content"));
        if data.get("content").is_some() && content.is_none() {
            push_gap(&mut result_gaps, Gap::OptionalContentOmitted);
        }
        if data
            .get("metadata")
            .and_then(|metadata| metadata.get("truncated"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            push_gap(&mut result_gaps, Gap::OptionalContentOmitted);
        }
        let created = event.get("created").and_then(Value::as_f64);
        let result = JoinedToolResult {
            session_id: session_id.to_owned(),
            assistant_message_id: message_id.clone(),
            call_id: call_id.clone(),
            event_id: event_id.to_owned(),
            aggregate_id: aggregate_id.to_owned(),
            sequence,
            created,
            tool_name: name,
            input,
            request_message_id,
            request_sequence,
            request_text,
            user_messages,
            status,
            content,
            outcome,
            provider_executed,
            gaps: result_gaps,
        };
        if let Some(first) = &prior_terminal {
            if first.status != result.status
                || first.outcome != result.outcome
                || first.provider_executed != result.provider_executed
                || first.content
                    != result
                        .content
                        .as_ref()
                        .map(|text| Sha256::digest(text.as_bytes()).into())
            {
                self.gap(Gap::ConflictingDuplicate);
            }
            return None;
        }
        self.calls
            .get_mut(&(message_id, call_id))
            .expect("call was checked")
            .terminal = Some(TerminalFingerprint {
            status: result.status.clone(),
            content: result
                .content
                .as_ref()
                .map(|text| Sha256::digest(text.as_bytes()).into()),
            outcome: result.outcome.clone(),
            provider_executed: result.provider_executed,
        });
        Some(result)
    }

    fn ensure_call(&mut self, message_id: String, call_id: String) -> bool {
        let key = (message_id.clone(), call_id.clone());
        if self.calls.contains_key(&key) {
            return true;
        }
        if self.calls.len() >= MAX_RETAINED_CALLS {
            self.gap(Gap::CapacityExceeded);
            return false;
        }
        self.calls.insert(
            key,
            Call {
                name: None,
                input: None,
                request_message_id: None,
                request_sequence: None,
                request_text: None,
                gaps: Vec::new(),
                user_messages: Vec::new(),
                terminal: None,
                provider_executed: None,
            },
        );
        true
    }

    fn gap(&mut self, gap: Gap) {
        if !self.gaps.contains(&gap) {
            self.gaps.push(gap);
        }
    }
}

fn json_payload_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len(),
        Value::Array(values) => values.iter().map(json_payload_bytes).sum(),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| key.len() + json_payload_bytes(value))
            .sum(),
        _ => std::mem::size_of::<Value>(),
    }
}

fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn event_schema_version(event_type: &str) -> Option<u64> {
    let version = match event_type {
        "session.deleted"
        | "session.forked"
        | "session.tool.success"
        | "session.tool.failed"
        | "session.instructions.updated" => 2,
        "session.updated"
        | "session.created"
        | "session.agent.selected"
        | "session.model.selected"
        | "session.moved"
        | "session.renamed"
        | "session.metadata.updated"
        | "session.permissions"
        | "session.viewed"
        | "session.message.content.updated"
        | "session.usage.recorded"
        | "session.usage.updated"
        | "session.inbox.enqueued"
        | "session.inbox.delivered"
        | "session.inbox.cancelled"
        | "session.inbox.delivery.changed"
        | "session.execution.started"
        | "session.execution.succeeded"
        | "session.execution.failed"
        | "session.execution.interrupted"
        | "session.synthetic"
        | "session.skill.activated"
        | "session.shell.started"
        | "session.shell.ended"
        | "session.step.started"
        | "session.step.streamed"
        | "session.step.ended"
        | "session.step.failed"
        | "session.text.started"
        | "session.text.delta"
        | "session.text.ended"
        | "session.reasoning.started"
        | "session.reasoning.delta"
        | "session.reasoning.ended"
        | "session.tool.input.started"
        | "session.tool.input.delta"
        | "session.tool.input.ended"
        | "session.tool.called"
        | "session.tool.progress"
        | "session.retry.scheduled"
        | "session.compaction.started"
        | "session.compaction.delta"
        | "session.compaction.ended"
        | "session.compaction.failed"
        | "session.revert.staged"
        | "session.revert.cleared"
        | "session.revert.committed" => 1,
        _ => return None,
    };
    Some(version)
}

fn bounded_field(value: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= MAX_STRING_BYTES)
        .map(str::to_owned)
}

fn push_gap(gaps: &mut Vec<Gap>, gap: Gap) {
    if !gaps.contains(&gap) {
        gaps.push(gap);
    }
}

pub(super) fn decode_content(value: Option<&Value>) -> Option<String> {
    let parts = value?.as_array()?;
    let mut output = String::new();
    for part in parts {
        if part.get("type").and_then(Value::as_str) != Some("text") {
            return None;
        }
        let text = part.get("text")?.as_str()?;
        if output.len().saturating_add(text.len()) > MAX_STRING_BYTES {
            return None;
        }
        output.push_str(text);
    }
    Some(output)
}
