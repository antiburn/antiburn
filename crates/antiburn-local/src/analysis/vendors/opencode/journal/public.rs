//! Bounded public boundaries beside the result journal state.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Event, identity, request_context};
use crate::analysis::{
    NativeActivityObservation, NativeEventContext, NativeEvidenceEvent, NativeEvidenceFact,
    NativeMessageObservation, NativeMessageRemoval, NativeToolInput, PublicMessageObservation,
    PublicMessagePayload, SourceFormat, ToolResultChange, ToolResultCompleteness,
    ToolResultPosition, UserMessageChange, UserMessageObservation, UserMessagePart,
};

const MAX_ROWS: usize = 128;
const MAX_BYTES: usize = 192 * 1024;
const MAX_TEXT: usize = 16 * 1024;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PublicState {
    messages: BTreeMap<String, Assistant>,
    parts: BTreeMap<String, Part>,
    users: BTreeMap<String, UserMessageObservation>,
    #[serde(default)]
    historical_users: BTreeMap<String, u64>,
    calls: BTreeMap<String, PublicMessageObservation>,
    removed_parts: BTreeSet<String>,
    disabled: bool,
    omitted: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Assistant {
    begun: bool,
    invalid: bool,
    unsafe_identity: bool,
    completed: bool,
    published: Option<PublicMessageObservation>,
    next_ordinal: u32,
    first_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Part {
    owner: String,
    kind: String,
    ordinal: u32,
    text: Option<String>,
    final_text: bool,
    excluded: bool,
}

impl PublicState {
    pub(super) fn seed_historical_users(
        &mut self,
        requests: &request_context::RequestHistory,
        frontier: u64,
    ) {
        for (id, position) in requests.retained_user_identities(frontier) {
            self.remember_historical_user(id, position);
            if self.disabled {
                break;
            }
        }
        self.bound();
    }

    fn remember_historical_user(&mut self, id: &str, position: u64) {
        if self.disabled || self.historical_users.contains_key(id) {
            return;
        }
        if self.historical_users.len() + self.removed_parts.len() >= MAX_ROWS {
            self.disabled = true;
            return;
        }
        self.historical_users.insert(id.to_owned(), position);
    }

    pub(super) fn is_removed_part(&self, id: &str) -> bool {
        self.removed_parts.contains(id)
    }

    pub(super) fn non_tool_part_kind(&self, id: &str, owner: &str) -> Option<&str> {
        self.parts
            .get(id)
            .filter(|part| part.owner == owner && part.kind != "tool")
            .map(|part| part.kind.as_str())
    }

    pub(super) fn history_complete(&self) -> bool {
        !self.disabled && !self.omitted
    }

    pub(super) fn observe(
        &mut self,
        session: &str,
        event: &Event,
        requests: &request_context::RequestHistory,
        safe: bool,
        baseline: u64,
    ) -> Vec<NativeEvidenceEvent> {
        let Some(data) = event
            .data
            .as_deref()
            .and_then(|data| serde_json::from_str::<Value>(data).ok())
        else {
            return Vec::new();
        };
        if data["sessionID"].as_str() != Some(session) {
            return Vec::new();
        }
        if event.sequence > baseline {
            let owner = match event.kind.as_str() {
                "message.updated.1" => identity(&data["info"]["id"]),
                "message.part.updated.1" => identity(&data["part"]["messageID"]),
                _ => None,
            };
            if let Some(id) = owner
                && let Some(position) = requests.retained_user_identity_position(id, baseline)
            {
                self.remember_historical_user(id, position);
            }
        }
        let mut facts = Vec::new();
        if event.kind == "message.removed.1" {
            if let Some(message) = identity(&data["messageID"]) {
                self.messages.remove(message);
                self.users.remove(message);
                self.historical_users.remove(message);
                self.parts.retain(|_, part| part.owner != message);
                self.calls.retain(|_, call| call.record_id != message);
                facts.push(NativeEvidenceFact::MessageRemoval(NativeMessageRemoval {
                    source_format: SourceFormat::OpenCodeSqliteV1,
                    session_id: session.to_owned(),
                    message_id: message.to_owned(),
                    position: position(event.sequence),
                    revision: event.sequence + 1,
                }));
            }
        } else if !self.disabled {
            self.observe_message(session, event, &data, requests, safe, &mut facts);
            self.observe_part(session, event, &data, requests, safe, &mut facts);
            self.observe_user(session, event, &data, requests, safe, &mut facts);
        }
        let completed_message = facts.iter().any(|fact| matches!(fact,
            NativeEvidenceFact::Message(NativeMessageObservation::Public(public))
                if public.eligible && matches!(public.payload, PublicMessagePayload::AssistantText { .. })
        ));
        if safe && requests.main_session() && (native_work(event, &data) || completed_message) {
            facts.push(NativeEvidenceFact::Activity(NativeActivityObservation {
                source_format: SourceFormat::OpenCodeSqliteV1,
                session_id: session.to_owned(),
                position: position(event.sequence),
                token: event.id.clone(),
                native_at_ms: None,
            }));
        }
        self.bound();
        facts
            .into_iter()
            .map(|fact| NativeEvidenceEvent {
                fact,
                live_eligible: false,
            })
            .collect()
    }

    fn bound(&mut self) {
        while self.messages.len()
            + self.parts.len()
            + self.users.len()
            + self.historical_users.len()
            + self.calls.len()
            + self.removed_parts.len()
            > MAX_ROWS
            || serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > MAX_BYTES)
        {
            self.omitted = true;
            if let Some(owner) = self
                .messages
                .iter()
                .min_by_key(|(_, message)| message.first_sequence)
                .map(|(id, _)| id.clone())
            {
                self.messages.remove(&owner);
                self.parts.retain(|_, part| part.owner != owner);
                self.calls.retain(|_, call| call.record_id != owner);
            } else if let Some(id) = self
                .users
                .iter()
                .min_by_key(|(_, user)| user.revision)
                .map(|(id, _)| id.clone())
            {
                self.users.remove(&id);
                self.parts.retain(|_, part| part.owner != id);
            } else if let Some(id) = self.parts.keys().next().cloned() {
                self.parts.remove(&id);
            } else {
                self.disabled = true;
                break;
            }
        }
    }

    fn observe_message(
        &mut self,
        session: &str,
        event: &Event,
        data: &Value,
        requests: &request_context::RequestHistory,
        safe: bool,
        facts: &mut Vec<NativeEvidenceFact>,
    ) {
        if event.kind != "message.updated.1" {
            return;
        }
        let info = &data["info"];
        let Some(id) = identity(&info["id"]).filter(|id| !requests.is_removed_message(id)) else {
            return;
        };
        if info["role"] != "assistant" {
            if let Some(message) = self.messages.get_mut(id) {
                message.invalid = true;
                message.unsafe_identity = true;
            }
            return;
        }
        let completed = info
            .pointer("/time/completed")
            .and_then(Value::as_i64)
            .is_some();
        let ordinal = self
            .parts
            .values()
            .filter(|part| part.owner == id)
            .map(|part| part.ordinal)
            .max()
            .map_or(0, |ordinal| ordinal.saturating_add(1));
        let message = self
            .messages
            .entry(id.to_owned())
            .or_insert_with(|| Assistant {
                first_sequence: event.sequence,
                ..Assistant::default()
            });
        message.next_ordinal = message.next_ordinal.max(ordinal);
        message.begun |= !completed;
        message.completed = completed;
        message.invalid |= info["sessionID"]
            .as_str()
            .is_some_and(|owner| owner != session)
            || info["summary"] == true
            || info.get("error").is_some_and(|error| !error.is_null());
        message.unsafe_identity |= info["sessionID"]
            .as_str()
            .is_some_and(|owner| owner != session);
        if !completed
            && let Some(previous) = &mut message.published
            && previous.eligible
        {
            previous.eligible = false;
            previous.position = position(event.sequence);
            previous.revision = event.sequence + 1;
            facts.push(NativeEvidenceFact::Message(
                NativeMessageObservation::Public(previous.clone()),
            ));
        }
        if completed && message.begun {
            self.publish_text(session, id, event.sequence, requests, safe, facts);
        }
    }

    fn observe_part(
        &mut self,
        session: &str,
        event: &Event,
        data: &Value,
        requests: &request_context::RequestHistory,
        safe: bool,
        facts: &mut Vec<NativeEvidenceFact>,
    ) {
        if !matches!(
            event.kind.as_str(),
            "message.part.updated.1" | "message.part.removed.1"
        ) {
            return;
        }
        let removal = event.kind == "message.part.removed.1";
        let part = &data["part"];
        let Some(id) = identity(if removal {
            &data["partID"]
        } else {
            &part["id"]
        }) else {
            return;
        };
        if removal {
            let owner = identity(&data["messageID"]);
            if self.removed_parts.len() >= MAX_ROWS {
                self.disabled = true;
                return;
            }
            self.removed_parts.insert(id.to_owned());
            if self
                .parts
                .get(id)
                .is_some_and(|part| owner != Some(part.owner.as_str()))
            {
                self.invalidate_part(session, id, event.sequence, requests, facts);
                return;
            }
            if let Some(mut call) = self.calls.remove(id)
                && owner == Some(call.record_id.as_str())
            {
                call.change = ToolResultChange::Removal;
                call.eligible = false;
                call.position = position(event.sequence);
                call.revision = event.sequence + 1;
                facts.push(NativeEvidenceFact::Message(
                    NativeMessageObservation::Public(call),
                ));
            }
            if let Some(removed) = self.parts.remove(id)
                && owner == Some(removed.owner.as_str())
            {
                self.publish_text(
                    session,
                    &removed.owner,
                    event.sequence,
                    requests,
                    safe,
                    facts,
                );
            }
            return;
        }
        if self.removed_parts.contains(id) {
            return;
        }
        let Some(owner) =
            identity(&part["messageID"]).filter(|owner| !requests.is_removed_message(owner))
        else {
            return;
        };
        if part["sessionID"]
            .as_str()
            .is_some_and(|value| value != session)
        {
            return;
        }
        let Some(kind) = identity(&part["type"]) else {
            return;
        };
        if self
            .parts
            .get(id)
            .is_some_and(|part| part.owner != owner || part.kind != kind)
        {
            self.invalidate_part(session, id, event.sequence, requests, facts);
            self.removed_parts.insert(id.to_owned());
            return;
        }
        if kind == "tool" && self.calls.get(id).is_some_and(|previous| {
            previous.call_id.as_deref() != identity(&part["callID"])
                || matches!(&previous.payload, PublicMessagePayload::ToolCall { tool_name, .. } if Some(tool_name.as_str()) != identity(&part["tool"]))
        }) {
            self.invalidate_part(session, id, event.sequence, requests, facts);
            self.removed_parts.insert(id.to_owned());
            return;
        }
        let ordinal = if let Some(existing) = self.parts.get(id) {
            existing.ordinal
        } else if let Some(message) = self.messages.get_mut(owner) {
            let ordinal = message.next_ordinal;
            message.next_ordinal = message.next_ordinal.saturating_add(1);
            ordinal
        } else {
            self.parts
                .values()
                .filter(|part| part.owner == owner)
                .count() as u32
        };
        let current = self.parts.entry(id.to_owned()).or_insert_with(|| Part {
            owner: owner.to_owned(),
            kind: kind.to_owned(),
            ordinal,
            text: None,
            final_text: false,
            excluded: false,
        });
        if kind == "text" {
            current.excluded = !part.get("synthetic").is_none_or(|value| value == false)
                || !part.get("ignored").is_none_or(|value| value == false);
            current.text = part["text"]
                .as_str()
                .filter(|text| text.len() <= MAX_TEXT)
                .map(str::to_owned);
            current.final_text = part.pointer("/time/end").and_then(Value::as_i64).is_some();
            if self
                .messages
                .get(owner)
                .is_some_and(|message| message.published.is_some())
            {
                self.publish_text(session, owner, event.sequence, requests, safe, facts);
            }
        } else if kind == "compaction" {
            if let Some(message) = self.messages.get_mut(owner) {
                message.invalid = true;
            }
        } else if kind == "tool" && part["state"]["status"] == "running" {
            let (Some(call_id), Some(name), Some(input)) = (
                identity(&part["callID"]),
                identity(&part["tool"]),
                part["state"].get("input").filter(|input| input.is_object()),
            ) else {
                return;
            };
            if !self
                .messages
                .get(owner)
                .is_some_and(|message| message.begun && !message.invalid)
                || input.to_string().len() > MAX_TEXT
            {
                return;
            }
            let mut context = context(requests, session, owner, event.sequence);
            let mut tool_facts = requests.tool_facts(session, owner, name, Some(input));
            tool_facts.operation_paths = None;
            if part
                .pointer("/metadata/providerExecuted")
                .is_some_and(|value| value != false)
            {
                tool_facts.category = Some(crate::analysis::CheckToolCategory::Other);
            }
            let mut observation = PublicMessageObservation {
                source_format: SourceFormat::OpenCodeSqliteV1,
                session_id: session.to_owned(),
                record_id: owner.to_owned(),
                part_id: Some(id.to_owned()),
                call_id: Some(call_id.to_owned()),
                position: position(event.sequence),
                revision: event.sequence + 1,
                change: ToolResultChange::Update,
                eligible: safe && requests.main_session(),
                completeness: completeness(requests, safe),
                context: None,
                payload: PublicMessagePayload::ToolCall {
                    tool_name: name.to_owned(),
                    input: Some(NativeToolInput::Json(input.clone())),
                    facts: Some(tool_facts),
                    provider_executed: None,
                },
            };
            if let Some(previous) = self.calls.get(id) {
                context = previous.context.clone().unwrap_or(context);
                if previous.payload == observation.payload {
                    return;
                }
            }
            observation.context = Some(context);
            self.calls.insert(id.to_owned(), observation.clone());
            facts.push(NativeEvidenceFact::Message(
                NativeMessageObservation::Public(observation),
            ));
        }
    }

    fn invalidate_part(
        &mut self,
        session: &str,
        id: &str,
        sequence: u64,
        requests: &request_context::RequestHistory,
        facts: &mut Vec<NativeEvidenceFact>,
    ) {
        if let Some(call) = self.calls.get_mut(id) {
            call.eligible = false;
            call.completeness = ToolResultCompleteness::UnsafeIdentity;
            call.position = position(sequence);
            call.revision = sequence + 1;
            facts.push(NativeEvidenceFact::Message(
                NativeMessageObservation::Public(call.clone()),
            ));
        }
        if let Some(owner) = self.parts.get(id).map(|part| part.owner.clone()) {
            if let Some(message) = self.messages.get_mut(&owner) {
                message.invalid = true;
                message.unsafe_identity = true;
            }
            self.publish_text(session, &owner, sequence, requests, false, facts);
        }
    }

    fn publish_text(
        &mut self,
        session: &str,
        id: &str,
        sequence: u64,
        requests: &request_context::RequestHistory,
        safe: bool,
        facts: &mut Vec<NativeEvidenceFact>,
    ) {
        let Some(message) = self.messages.get(id) else {
            return;
        };
        if !message.completed || !message.begun {
            return;
        }
        let mut parts: Vec<_> = self
            .parts
            .iter()
            .filter(|(_, part)| part.owner == id && part.kind == "text" && !part.excluded)
            .collect();
        parts.sort_by_key(|(_, part)| part.ordinal);
        let eligible = safe
            && requests.main_session()
            && !message.invalid
            && !parts.is_empty()
            && parts
                .iter()
                .all(|(_, part)| part.final_text && part.text.is_some());
        if !eligible && message.published.is_none() {
            return;
        }
        let parts = parts
            .into_iter()
            .filter_map(|(part_id, part)| {
                Some(UserMessagePart {
                    item_index: part.ordinal,
                    native_part_id: Some(part_id.clone()),
                    text: part.text.clone()?,
                })
            })
            .collect();
        let generator = requests.tool_facts(session, id, "", None).requester;
        let observation = PublicMessageObservation {
            source_format: SourceFormat::OpenCodeSqliteV1,
            session_id: session.to_owned(),
            record_id: id.to_owned(),
            part_id: None,
            call_id: None,
            position: position(sequence),
            revision: sequence + 1,
            change: ToolResultChange::Update,
            eligible,
            completeness: if message.unsafe_identity {
                ToolResultCompleteness::UnsafeIdentity
            } else {
                completeness(requests, safe)
            },
            context: message
                .published
                .as_ref()
                .and_then(|previous| previous.context.clone())
                .or_else(|| Some(context(requests, session, id, sequence))),
            payload: PublicMessagePayload::AssistantText { parts, generator },
        };
        if message.published.as_ref().is_some_and(|previous| {
            previous.payload == observation.payload && previous.eligible == observation.eligible
        }) {
            return;
        }
        self.messages.get_mut(id).expect("message exists").published = Some(observation.clone());
        facts.push(NativeEvidenceFact::Message(
            NativeMessageObservation::Public(observation),
        ));
    }

    fn observe_user(
        &mut self,
        session: &str,
        event: &Event,
        data: &Value,
        requests: &request_context::RequestHistory,
        safe: bool,
        facts: &mut Vec<NativeEvidenceFact>,
    ) {
        let owner = if event.kind == "message.updated.1" {
            identity(&data["info"]["id"])
        } else if event.kind == "message.part.updated.1" {
            identity(&data["part"]["messageID"])
        } else if event.kind == "message.part.removed.1" {
            identity(&data["messageID"])
        } else {
            None
        };
        let Some(owner) = owner.filter(|owner| requests.is_user_message(owner)) else {
            return;
        };
        let prefix = requests.freeze(session, "", event.sequence);
        let current = prefix
            .user_messages
            .iter()
            .find(|message| message.message_id == owner)
            .cloned();
        let mut current = if let Some(current) = current {
            current
        } else if let Some(previous) = self.users.get(owner) {
            let mut message = previous.message.clone();
            message.change = UserMessageChange::Removal;
            message.parts.clear();
            message
        } else {
            return;
        };
        if !safe || !requests.main_session() {
            current.change = UserMessageChange::Conflict;
        }
        let previous = self.users.get(owner);
        if previous.is_some_and(|previous| {
            previous.message.parts == current.parts && previous.message.change == current.change
        }) {
            return;
        }
        let observation = UserMessageObservation {
            message: current,
            revision: event.sequence + 1,
            context: previous.map(|previous| previous.context.clone()).unwrap_or(
                NativeEventContext {
                    first_eligible_position: position(
                        self.historical_users
                            .get(owner)
                            .copied()
                            .unwrap_or(event.sequence),
                    ),
                    main_session: safe && requests.main_session(),
                    complete_through_event: false,
                    user_messages: prefix.user_messages,
                },
            ),
        };
        self.users.insert(owner.to_owned(), observation.clone());
        facts.push(NativeEvidenceFact::Message(NativeMessageObservation::User(
            observation,
        )));
    }
}

fn position(sequence: u64) -> ToolResultPosition {
    ToolResultPosition::OpenCodeEvent {
        sequence,
        item_index: 0,
    }
}

fn completeness(
    requests: &request_context::RequestHistory,
    ordered: bool,
) -> ToolResultCompleteness {
    if !requests.main_session() {
        ToolResultCompleteness::UnsafeOwnership
    } else if !ordered {
        ToolResultCompleteness::UnsafeOrder
    } else {
        ToolResultCompleteness::Complete
    }
}

fn context(
    requests: &request_context::RequestHistory,
    session: &str,
    owner: &str,
    sequence: u64,
) -> NativeEventContext {
    let context = requests.freeze(session, owner, sequence);
    NativeEventContext {
        first_eligible_position: position(sequence),
        main_session: context.main_session,
        complete_through_event: context.complete_through_event,
        user_messages: context.user_messages,
    }
}

fn native_work(event: &Event, data: &Value) -> bool {
    match event.kind.as_str() {
        // A header can change only usage. Parts and proven completion report work.
        "message.updated.1" => false,
        "message.part.updated.1" => {
            matches!(
                data["part"]["type"].as_str(),
                Some("text" | "reasoning" | "tool" | "step-start" | "step-finish" | "compaction")
            ) && data["part"]
                .get("synthetic")
                .is_none_or(|value| value == false)
                && data["part"]
                    .get("ignored")
                    .is_none_or(|value| value == false)
        }
        "message.removed.1" | "message.part.removed.1" => true,
        _ => false,
    }
}
