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
    #[serde(default)]
    removed_parts: BTreeMap<String, RemovedPart>,
    bytes: usize,
    incomplete: bool,
    unsafe_identity: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RemovedPart {
    owner: String,
    kind: Option<String>,
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
        while self.messages.len() + self.parts.len() + self.tombstone_count() > records
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

    pub(super) fn is_removed_part(&self, id: &str) -> bool {
        self.removed_parts.contains_key(id)
    }

    pub(super) fn owns_non_tool_part(&self, id: &str, owner: &str) -> bool {
        self.parts.get(id).is_some_and(|(_, part)| {
            part["messageID"] == owner && part["type"].as_str().is_some_and(|kind| kind != "tool")
        }) || self.removed_parts.get(id).is_some_and(|part| {
            part.owner == owner && part.kind.as_deref().is_some_and(|kind| kind != "tool")
        })
    }

    pub(super) fn retained_user_identities(
        &self,
        frontier: u64,
    ) -> impl Iterator<Item = (&str, u64)> {
        self.messages.keys().filter_map(move |id| {
            self.retained_user_identity_position(id, frontier)
                .map(|position| (id.as_str(), position))
        })
    }

    pub(super) fn retained_user_identity_position(&self, id: &str, frontier: u64) -> Option<u64> {
        let (header_sequence, info) = self.messages.get(id)?;
        if info["role"] != "user" || *header_sequence > frontier {
            return None;
        }
        let position = self
            .parts
            .values()
            .filter(|(sequence, part)| part["messageID"] == id && *sequence <= frontier)
            .map(|(sequence, _)| (*sequence).max(*header_sequence))
            .min()
            .unwrap_or(*header_sequence);
        Some(position)
    }

    pub(super) fn remove_part(
        &mut self,
        id: &str,
        owner: &str,
        known_kind: Option<&str>,
        records: usize,
        bytes: usize,
    ) {
        self.incomplete = true;
        let kind = if let Some((_, part)) = self.parts.get(id) {
            if part["messageID"] != owner {
                self.unsafe_identity = true;
                return;
            }
            part["type"].as_str().map(str::to_owned)
        } else {
            known_kind.map(str::to_owned)
        };
        if let Some((_, part)) = self.parts.remove(id) {
            self.bytes = self.bytes.saturating_sub(part.to_string().len());
        }
        if let Some(previous) = self.removed_parts.get(id) {
            self.unsafe_identity |= previous.owner != owner;
            return;
        }
        if self.unsafe_identity {
            return;
        }
        let size = id.len() + owner.len() + kind.as_ref().map_or(0, String::len);
        if self.tombstone_count() >= records || self.tombstone_bytes().saturating_add(size) > bytes
        {
            self.unsafe_identity = true;
            return;
        }
        self.removed_parts.insert(
            id.to_owned(),
            RemovedPart {
                owner: owner.to_owned(),
                kind,
            },
        );
        self.bytes = self.bytes.saturating_add(size);
        self.bound(records, bytes);
    }

    fn tombstone_count(&self) -> usize {
        self.removed_messages.len() + self.removed_parts.len()
    }

    fn tombstone_bytes(&self) -> usize {
        self.removed_messages.iter().map(String::len).sum::<usize>()
            + self
                .removed_parts
                .iter()
                .map(|(id, part)| {
                    id.len() + part.owner.len() + part.kind.as_ref().map_or(0, String::len)
                })
                .sum::<usize>()
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
        if self.tombstone_count() >= records
            || self.tombstone_bytes().saturating_add(id.len()) > bytes
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

    pub(super) fn admission_failure(&self) -> Option<crate::analysis::ToolResultCompleteness> {
        if self.unsafe_identity {
            Some(crate::analysis::ToolResultCompleteness::UnsafeIdentity)
        } else if !self.root {
            Some(crate::analysis::ToolResultCompleteness::UnsafeOwnership)
        } else {
            None
        }
    }

    pub(super) fn tool_facts(
        &self,
        session: &str,
        message: &str,
        name: &str,
        arguments: Option<&Value>,
    ) -> crate::analysis::NativeToolFacts {
        let info = self
            .messages
            .get(message)
            .filter(|(_, info)| info["role"] == "assistant")
            .map(|(_, info)| info);
        let request = info.filter(|info| {
            !self.unsafe_identity
                && info["id"] == message
                && info["sessionID"] == session
                && self.parts.values().any(|(_, part)| {
                    part["type"] == "step-start"
                        && part["messageID"] == message
                        && part["sessionID"] == session
                })
        });
        crate::analysis::NativeToolFacts::for_call(
            SourceFormat::OpenCodeSqliteV1,
            name,
            arguments,
            None,
            crate::analysis::NativeRequestAttribution::observed(
                request.and_then(|info| info["providerID"].as_str()),
                request.and_then(|info| info["modelID"].as_str()),
            ),
        )
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
                if self.unsafe_identity
                    || part["id"]
                        .as_str()
                        .is_some_and(|id| self.is_removed_part(id))
                    || part["messageID"]
                        .as_str()
                        .is_some_and(|id| self.is_removed_message(id))
                {
                    return;
                }
                if part["type"] == "compaction" {
                    self.incomplete = true;
                }
                let user_part = part["type"] != "tool"
                    && part["messageID"].as_str().is_some_and(|id| {
                        self.messages
                            .get(id)
                            .is_some_and(|(_, info)| info["role"] == "user")
                    });
                let request_start = part["type"] == "step-start"
                    && part["sessionID"] == session
                    && part["messageID"]
                        .as_str()
                        .is_some_and(|id| !id.is_empty() && id.len() <= EVIDENCE_STRING_CAP);
                if (user_part
                    || request_start
                    || part["id"]
                        .as_str()
                        .is_some_and(|id| self.parts.contains_key(id)))
                    && let Some(id) = part["id"]
                        .as_str()
                        .filter(|id| !id.is_empty() && id.len() <= EVIDENCE_STRING_CAP)
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
                if let (Some(id), Some(owner)) =
                    (bounded_id(&data["partID"]), bounded_id(&data["messageID"]))
                {
                    self.remove_part(
                        id,
                        owner,
                        None,
                        MAX_USER_MESSAGE_OBSERVATIONS,
                        MAX_USER_MESSAGE_COLLECTION_BYTES,
                    );
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
            && (self.messages.len() + self.parts.len() + self.tombstone_count()
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
            && self.messages.len() + self.parts.len() + self.tombstone_count()
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

fn bounded_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= EVIDENCE_STRING_CAP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn part_tombstone_overflow_keeps_request_history_fail_closed_after_restart() {
        let mut history = RequestHistory {
            root: true,
            ..RequestHistory::default()
        };
        let header =
            json!({"sessionID":"root","info":{"id":"user","sessionID":"root","role":"user"}});
        let part = json!({"sessionID":"root","part":{"id":"part-a","sessionID":"root","messageID":"user","type":"text","text":"Deleted input"}});
        history.observe("message.updated.1", &header, "root", 1);
        history.observe("message.part.updated.1", &part, "root", 2);
        history.remove_part("part-a", "user", None, 2, 1024);
        history.remove_part("part-b", "user", None, 2, 1024);
        history.remove_part("overflow", "user", None, 2, 1024);
        assert_eq!(history.removed_parts.len(), 2);
        assert!(!history.main_session());
        let mut restored: RequestHistory =
            serde_json::from_slice(&serde_json::to_vec(&history).unwrap()).unwrap();
        restored.observe("message.updated.1", &header, "root", 5);
        restored.observe("message.part.updated.1", &part, "root", 6);
        assert!(!restored.main_session());
        assert!(!restored.parts.contains_key("part-a"));
        assert_eq!(restored.removed_parts, history.removed_parts);
        assert!(restored.freeze("root", "", 6).user_messages.is_empty());
    }

    #[test]
    fn evicted_step_start_cannot_be_replaced_by_retained_assistant_metadata() {
        let mut history = RequestHistory::default();
        let assistant = json!({"sessionID":"facts","info":{
            "id":"assistant-1","sessionID":"facts","role":"assistant",
            "providerID":"openai","modelID":"synthetic-model",
            "path":{"cwd":"/synthetic/project"}
        }});
        history.observe("message.updated.1", &assistant, "facts", 1);
        history.observe(
            "message.part.updated.1",
            &json!({"sessionID":"facts","part":{
                "id":"step-1","messageID":"assistant-1","sessionID":"facts","type":"step-start"
            }}),
            "facts",
            2,
        );
        let mut history: RequestHistory =
            serde_json::from_value(serde_json::to_value(history).unwrap()).unwrap();
        assert_eq!(
            history
                .tool_facts("facts", "assistant-1", "write", None)
                .requester
                .model_key()
                .as_deref(),
            Some("openai/synthetic-model")
        );
        history.bound(0, 0);
        assert_eq!(history.bytes, 0);
        assert!(history.messages.is_empty());
        assert!(history.parts.is_empty());
        history.observe("message.updated.1", &assistant, "facts", 3);
        assert_eq!(
            history
                .tool_facts("facts", "assistant-1", "write", None)
                .requester,
            crate::analysis::NativeRequestAttribution::default()
        );
    }
}
