//! User-slot history from the same ordered journal as tool mutations.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::analysis::{
    EVIDENCE_STRING_CAP, MAX_USER_MESSAGE_COLLECTION_BYTES, MAX_USER_MESSAGE_OBSERVATIONS,
    SourceFormat, ToolResultContext, ToolResultPosition, UserMessage, UserMessageChange,
    UserMessageCompleteness, UserMessageOrigin, UserMessagePart, UserMessagePosition,
    UserMessageScope,
};

#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct RequestHistory {
    root: bool,
    messages: BTreeMap<String, (u64, Value)>,
    parts: BTreeMap<String, (u64, Value)>,
    #[serde(default)]
    removed_messages: BTreeSet<String>,
    bytes: usize,
    incomplete: bool,
    unsafe_identity: bool,
}

impl RequestHistory {
    pub(super) fn partial(root: bool) -> Self {
        Self {
            root,
            incomplete: true,
            ..Self::default()
        }
    }

    pub(super) fn bound(&mut self, records: usize, bytes: usize) {
        while self.messages.len() + self.parts.len() + self.removed_messages.len() > records
            || self.bytes > bytes
        {
            if !self.evict_oldest() {
                break;
            }
        }
    }

    pub(super) fn is_user_message(&self, id: &str) -> bool {
        self.messages
            .get(id)
            .is_some_and(|(_, info)| info["role"] == "user")
    }

    pub(super) fn has_message(&self, id: &str) -> bool {
        self.messages.contains_key(id)
    }

    pub(super) fn is_removed_message(&self, id: &str) -> bool {
        self.removed_messages.contains(id)
    }

    pub(super) fn remove_message(&mut self, id: &str, records: usize, bytes: usize) {
        self.incomplete = true;
        if let Some((_, message)) = self.messages.remove(id) {
            self.bytes = self.bytes.saturating_sub(message.to_string().len());
        }
        self.parts.retain(|_, (_, part)| {
            if part["messageID"] != id {
                return true;
            }
            self.bytes = self.bytes.saturating_sub(part.to_string().len());
            false
        });
        if self.is_removed_message(id) || self.unsafe_identity {
            return;
        }
        let removed_bytes: usize = self.removed_messages.iter().map(String::len).sum();
        if self.removed_messages.len() >= records || removed_bytes.saturating_add(id.len()) > bytes
        {
            // Keep deletion proof. Reject all ownership if another tombstone cannot fit.
            self.unsafe_identity = true;
            return;
        }
        self.removed_messages.insert(id.to_owned());
        self.bytes = self.bytes.saturating_add(id.len());
        self.bound(records, bytes);
    }

    pub(super) fn main_session(&self) -> bool {
        self.root && !self.unsafe_identity
    }

    pub(super) fn observe(&mut self, kind: &str, data: &Value, session: &str, sequence: u64) {
        if data["sessionID"].as_str() != Some(session) {
            self.unsafe_identity = true;
            return;
        }
        match kind {
            "session.created.1" => {
                self.root = data["info"]["id"] == session
                    && data["info"].get("parentID").is_none_or(Value::is_null);
            }
            "session.updated.1" => {
                self.root &= data["info"]["id"] == session
                    && data["info"].get("parentID").is_none_or(Value::is_null);
            }
            "message.updated.1" => {
                self.unsafe_identity |= data["info"]["sessionID"]
                    .as_str()
                    .is_some_and(|id| id != session);
                if let Some(id) = data["info"]["id"].as_str()
                    && !self.is_removed_message(id)
                {
                    self.retain(true, id, &data["info"], sequence);
                }
            }
            "message.part.updated.1" => {
                let part = &data["part"];
                if part["messageID"]
                    .as_str()
                    .is_some_and(|id| self.is_removed_message(id))
                {
                    return;
                }
                if part["type"] == "compaction" {
                    self.incomplete = true;
                }
                if part["type"] != "tool"
                    && part["messageID"].as_str().is_some_and(|id| {
                        self.messages
                            .get(id)
                            .is_some_and(|(_, info)| info["role"] == "user")
                    })
                    && let Some(id) = part["id"].as_str()
                {
                    self.retain(false, id, part, sequence);
                }
            }
            "message.removed.1" => {
                self.incomplete = true;
                if let Some(id) = data["messageID"]
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= EVIDENCE_STRING_CAP)
                {
                    self.remove_message(
                        id,
                        MAX_USER_MESSAGE_OBSERVATIONS,
                        MAX_USER_MESSAGE_COLLECTION_BYTES,
                    );
                }
            }
            "session.deleted.1" => self.incomplete = true,
            "message.part.removed.1" => {
                if let Some(id) = data["partID"].as_str()
                    && let Some((_, part)) = self.parts.remove(id)
                {
                    self.bytes = self.bytes.saturating_sub(part.to_string().len());
                    self.incomplete = true;
                }
            }
            _ => {}
        }
    }

    fn retain(&mut self, message: bool, id: &str, value: &Value, sequence: u64) {
        let size = value.to_string().len();
        let (exists, first_sequence, previous_size, unchanged) = {
            let rows = if message { &self.messages } else { &self.parts };
            let existing = rows.get(id);
            (
                existing.is_some(),
                existing.map_or(sequence, |(sequence, _)| *sequence),
                existing.map_or(0, |(_, previous)| previous.to_string().len()),
                existing.is_some_and(|(_, previous)| previous == value),
            )
        };
        let rows = if message { &self.messages } else { &self.parts };
        if let Some((_, previous)) = rows.get(id) {
            let fields = if message {
                &["id", "role", "parentID", "sessionID"][..]
            } else {
                &["id", "messageID", "sessionID", "type"][..]
            };
            self.unsafe_identity |= fields.iter().any(|field| previous[*field] != value[*field]);
        }
        if unchanged {
            return;
        }
        while !exists
            && (self.messages.len() + self.parts.len() + self.removed_messages.len()
                >= MAX_USER_MESSAGE_OBSERVATIONS
                || self.bytes.saturating_add(size) > MAX_USER_MESSAGE_COLLECTION_BYTES)
        {
            if !self.evict_oldest() {
                break;
            }
        }
        let next_bytes = self
            .bytes
            .saturating_sub(previous_size)
            .saturating_add(size);
        if (!exists
            && self.messages.len() + self.parts.len() + self.removed_messages.len()
                >= MAX_USER_MESSAGE_OBSERVATIONS)
            || next_bytes > MAX_USER_MESSAGE_COLLECTION_BYTES
        {
            self.incomplete = true;
            return;
        }
        let rows = if message {
            &mut self.messages
        } else {
            &mut self.parts
        };
        if rows
            .insert(id.to_owned(), (first_sequence, value.clone()))
            .is_some()
            && !message
        {
            self.incomplete = true;
        }
        self.bytes = next_bytes;
    }

    fn evict_oldest(&mut self) -> bool {
        let oldest_message = self
            .messages
            .iter()
            .min_by_key(|(_, (sequence, _))| *sequence)
            .map(|(id, (sequence, value))| (true, id.clone(), *sequence, value.to_string().len()));
        let oldest_part = self
            .parts
            .iter()
            .min_by_key(|(_, (sequence, _))| *sequence)
            .map(|(id, (sequence, value))| (false, id.clone(), *sequence, value.to_string().len()));
        let Some(oldest) = [oldest_message, oldest_part]
            .into_iter()
            .flatten()
            .min_by_key(|(_, _, sequence, _)| *sequence)
        else {
            return false;
        };
        self.incomplete = true;
        if oldest.0 {
            let message_id = oldest.1;
            self.messages.remove(&message_id);
            let removed = self
                .parts
                .iter()
                .filter(|(_, (_, part))| part["messageID"] == message_id)
                .map(|(id, (_, part))| (id.clone(), part.to_string().len()))
                .collect::<Vec<_>>();
            for (id, bytes) in removed {
                self.parts.remove(&id);
                self.bytes = self.bytes.saturating_sub(bytes);
            }
        } else {
            self.parts.remove(&oldest.1);
        }
        self.bytes = self.bytes.saturating_sub(oldest.3);
        true
    }

    pub(super) fn freeze(&self, session: &str, message: &str, sequence: u64) -> ToolResultContext {
        let assistant = self.messages.get(message);
        let main_session =
            self.main_session() && assistant.is_some_and(|(_, info)| info["role"] == "assistant");
        let parent_retained = assistant.is_some_and(|(_, info)| {
            info["parentID"].as_str().is_some_and(|parent| {
                self.messages
                    .get(parent)
                    .is_some_and(|(_, info)| info["role"] == "user")
            })
        });
        let mut complete = !self.incomplete && main_session && parent_retained;
        let mut user_messages = Vec::new();
        let mut messages: Vec<_> = self.messages.iter().collect();
        messages.sort_by_key(|(_, (position, _))| *position);
        for (id, (position, info)) in messages {
            if info["role"] != "user" {
                continue;
            }
            let mut parts = Vec::new();
            let mut ordered_parts: Vec<_> = self.parts.iter().collect();
            ordered_parts.sort_by_key(|(_, (position, _))| *position);
            for (part_id, (_, part)) in ordered_parts {
                if part["messageID"] != *id {
                    continue;
                }
                if part["type"] == "text"
                    && part.get("ignored").is_none_or(|value| value == false)
                    && part.get("synthetic").is_none_or(|value| value == false)
                {
                    if let Some(text) = part["text"].as_str() {
                        parts.push(UserMessagePart {
                            item_index: u32::try_from(parts.len()).unwrap_or(u32::MAX),
                            native_part_id: Some(part_id.clone()),
                            text: text.to_owned(),
                        });
                    } else {
                        complete = false;
                    }
                } else if part["type"] != "text" {
                    complete = false;
                }
            }
            let request = UserMessage {
                source_format: SourceFormat::OpenCodeSqliteV1,
                session_id: session.to_owned(),
                message_id: id.clone(),
                position: UserMessagePosition::OpenCodeEvent {
                    sequence: *position,
                },
                origin: UserMessageOrigin::OpenCodeUserText,
                scope: UserMessageScope::MainSession,
                change: UserMessageChange::Present,
                completeness: UserMessageCompleteness::Complete,
                parts,
                proof_revision: 2,
            };
            if request.is_bounded() {
                user_messages.push(request);
            } else {
                complete = false;
            }
        }
        ToolResultContext {
            result_position: ToolResultPosition::OpenCodeEvent {
                sequence,
                item_index: 0,
            },
            main_session,
            complete_through_event: complete,
            user_messages,
        }
    }
}
