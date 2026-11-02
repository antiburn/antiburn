//! This contract proves a retained linear root, not the original historical conversation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::analysis::SourceFormat;
use crate::analysis::interface::{ContentAuthority, ContentKind, ContentPart};
use crate::analysis::jev::JevInputField;
use crate::analysis::jev_evidence::{
    JevNativeFieldContainer, JevNativeFieldRange, UserTextHistoryProof,
};

const CHARACTERIZED_VERSIONS: &[&str] = &["2.1.278", "2.1.309"];
const MAX_RECORDS: usize = 16_384;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ClaudeRetainedRoot {
    session: Option<String>,
    last: Option<String>,
    fingerprints: BTreeMap<String, [u8; 32]>,
    invalid: bool,
    characterized: bool,
    user_slot: bool,
}

impl ClaudeRetainedRoot {
    pub(super) fn for_user_slot() -> Self {
        Self {
            user_slot: true,
            ..Self::default()
        }
    }

    pub(super) fn is_pristine_user_slot(&self) -> bool {
        self.user_slot
            && !self.invalid
            && !self.characterized
            && self.session.is_none()
            && self.last.is_none()
            && self.fingerprints.is_empty()
    }

    pub(super) fn observe_user_slot(&mut self, record: &Value) {
        self.user_slot = true;
        self.observe(record);
    }

    pub(super) fn observe_retained_start(&mut self, record: &Value) {
        // The live reader proves the owner from the file header, not the omitted parent.
        if self.last.is_some() || self.invalid {
            self.invalidate();
            return;
        }
        self.user_slot = true;
        self.observe_record(record, true);
    }

    pub(super) fn admits(&self, session_id: &str) -> bool {
        self.characterized && !self.invalid && self.session.as_deref() == Some(session_id)
    }

    pub(super) fn invalidate(&mut self) {
        self.invalid = true;
    }

    pub(super) fn has_known_seam(&self, record: &Value) -> bool {
        self.last.as_deref().is_some_and(|last| {
            retained_parent(record) == Some(last)
                && (self.user_slot || !is_compaction_boundary(record))
        })
    }

    pub(super) fn has_gap(&self) -> bool {
        self.characterized && self.invalid
    }

    pub(super) fn check_replay(&mut self, record: &Value) {
        if self.session.is_some()
            && record["uuid"]
                .as_str()
                .and_then(|id| self.fingerprints.get(id))
                != Some(&digest(record))
        {
            self.invalidate();
        }
    }

    pub(super) fn conflicting_record(&self, record: &Value) -> bool {
        self.characterized
            && record["uuid"]
                .as_str()
                .and_then(|id| self.fingerprints.get(id))
                .is_some_and(|prior| prior != &digest(record))
    }

    pub(super) fn has_record(&self, record: &Value) -> bool {
        record["uuid"]
            .as_str()
            .is_some_and(|id| self.fingerprints.contains_key(id))
    }

    pub(super) fn exact_record_replay(&self, record: &Value) -> bool {
        record["uuid"]
            .as_str()
            .and_then(|id| self.fingerprints.get(id))
            .is_some_and(|prior| prior == &digest(record))
    }

    pub(super) fn observe(&mut self, record: &Value) {
        self.observe_record(record, false);
    }

    fn observe_record(&mut self, record: &Value, retained_start: bool) {
        self.characterized |= is_characterized_cli(record);
        if ((record["subtype"] == "compact_boundary" || record["isCompactSummary"] == true)
            && !self.user_slot)
            || record["type"] == "fork-context-ref"
        {
            self.invalidate();
        }
        if !matches!(record["type"].as_str(), Some("user" | "assistant"))
            && record.get("uuid").is_none()
        {
            return;
        }
        let id = bounded_id(&record["uuid"]);
        let session = bounded_id(&record["sessionId"]);
        let root = self.last.is_none();
        let parent_matches = if root {
            (retained_start && retained_parent(record).is_some() && retained_parent(record) != id)
                || (record.get("parentUuid") == Some(&Value::Null)
                    && record.get("logicalParentUuid").is_none_or(Value::is_null)
                    && ((if self.user_slot {
                        is_user_content(record)
                    } else {
                        is_human(record)
                    }) || is_informational_root(record)))
        } else {
            self.has_known_seam(record)
        };
        if !is_characterized_cli(record)
            || record["isSidechain"] != false
            || record.get("agentId").is_some()
            || id.is_none()
            || session.is_none()
            || self
                .session
                .as_deref()
                .is_some_and(|previous| Some(previous) != session)
            || !parent_matches
            || matches!(record["type"].as_str(), Some("user" | "assistant"))
                && record.pointer("/message/role") != record.get("type")
            || self.fingerprints.len() >= MAX_RECORDS
        {
            self.invalidate();
            return;
        }
        let id = id.expect("validated record identity");
        self.session = session.map(str::to_owned);
        self.last = Some(id.to_owned());
        self.fingerprints.insert(id.to_owned(), digest(record));
    }

    pub(super) fn bind_user_text(&mut self, record: &Value, parts: &mut [ContentPart]) {
        if self.invalid || !is_human(record) || self.session.is_none() {
            return;
        }
        let texts = match record.pointer("/message/content") {
            Some(Value::String(text)) => Some(vec![("/message/content".into(), text.as_str())]),
            Some(Value::Array(blocks)) if !blocks.is_empty() => blocks
                .iter()
                .enumerate()
                .map(|(index, block)| {
                    is_user_text_block(block).then_some(())?;
                    Some((
                        format!("/message/content/{index}/text"),
                        block["text"].as_str()?,
                    ))
                })
                .collect::<Option<Vec<_>>>(),
            _ => None,
        };
        let Some(texts) = texts.filter(|texts| texts.len() == parts.len()) else {
            self.invalidate();
            return;
        };
        if parts.iter().zip(&texts).any(|(part, (_, text))| {
            part.kind != ContentKind::UserText
                || part.authority != ContentAuthority::User
                || part.truncated
                || part.text != *text
        }) {
            self.invalidate();
            return;
        }
        let record_id = record["uuid"].as_str().expect("validated identity");
        for (part, (pointer, text)) in parts.iter_mut().zip(texts) {
            part.metadata.user_text_history = Some(UserTextHistoryProof {
                source_format: SourceFormat::ClaudeJsonl,
                session_id: self.session.clone().expect("validated session"),
                message_id: record_id.into(),
                revision: 1,
            });
            part.metadata.bindings.push(JevNativeFieldRange {
                native_record_id: Some(record_id.into()),
                field: JevInputField::UserMessage,
                container: JevNativeFieldContainer::Record,
                pointer,
                start: 0,
                end: text.len(),
            });
        }
    }
}

fn bounded_id(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 512)
}

fn digest(record: &Value) -> [u8; 32] {
    Sha256::digest(record.to_string().as_bytes()).into()
}

fn is_human(record: &Value) -> bool {
    record["type"] == "user"
        && record.pointer("/message/role").and_then(Value::as_str) == Some("user")
        && record.pointer("/origin/kind").and_then(Value::as_str) == Some("human")
        && record["promptSource"] == "typed"
        && record["turnOrigin"] == "human"
        && !has_injection_flags(record)
}

pub(super) fn is_characterized_cli(record: &Value) -> bool {
    record["entrypoint"] == "cli"
        && record["version"]
            .as_str()
            .is_some_and(|version| CHARACTERIZED_VERSIONS.contains(&version))
}

pub(super) fn is_user_content(record: &Value) -> bool {
    record["type"] == "user"
        && record.pointer("/message/role").and_then(Value::as_str) == Some("user")
        && !excludes_user_text(record)
        && match record.pointer("/message/content") {
            Some(Value::String(_)) => true,
            Some(Value::Array(blocks)) => blocks.iter().any(is_user_text_block),
            _ => false,
        }
}

pub(super) fn is_user_text_block(block: &Value) -> bool {
    block["type"] == "text" && block["text"].is_string() && !excludes_user_text(block)
}

pub(super) fn excludes_user_text(record: &Value) -> bool {
    has_injection_flags(record)
        || (record.get("origin").is_some()
            && !matches!(
                record.pointer("/origin/kind").and_then(Value::as_str),
                Some("human" | "api" | "extension" | "sdk")
            ))
        || record.get("promptSource").is_some_and(|source| {
            !matches!(source.as_str(), Some("typed" | "api" | "extension" | "sdk"))
        })
        || record.get("turnOrigin").is_some_and(|origin| {
            !matches!(origin.as_str(), Some("human" | "api" | "extension" | "sdk"))
        })
}

pub(super) fn is_compaction_boundary(record: &Value) -> bool {
    record["type"] == "system"
        && record["subtype"] == "compact_boundary"
        && record.get("parentUuid") == Some(&Value::Null)
        && bounded_id(&record["logicalParentUuid"]).is_some()
}

pub(super) fn retained_parent(record: &Value) -> Option<&str> {
    if is_compaction_boundary(record) {
        bounded_id(&record["logicalParentUuid"])
    } else if record.get("logicalParentUuid").is_none_or(Value::is_null)
        && record["subtype"] != "compact_boundary"
    {
        bounded_id(&record["parentUuid"])
    } else {
        None
    }
}

pub(super) fn is_informational_root(record: &Value) -> bool {
    record["type"] == "system"
        && record["subtype"] == "informational"
        && record["level"] == "notice"
        && record["isMeta"] == false
        && record["content"].is_string()
        && !has_injection_flags(record)
}

fn has_injection_flags(record: &Value) -> bool {
    record.get("planContent").is_some()
        || ["isMeta", "isSynthetic", "isCompactSummary"]
            .iter()
            .any(|key| record.get(key).is_some_and(|value| value != false))
}

fn denies_human_authority(record: &Value) -> bool {
    has_injection_flags(record)
        || (record.get("origin").is_some()
            && record.pointer("/origin/kind").and_then(Value::as_str) != Some("human"))
        || record
            .get("promptSource")
            .is_some_and(|source| source != "typed")
        || record
            .get("turnOrigin")
            .is_some_and(|origin| origin != "human")
}

pub(super) fn set_user_authority(record: &Value, parts: &mut [ContentPart]) {
    let injected = denies_human_authority(record);
    let unproven = is_characterized_cli(record) && !is_human(record);
    let injected_block = record
        .pointer("/message/content")
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks
                .iter()
                .any(|block| block["type"] == "text" && denies_human_authority(block))
        });
    if record["type"] == "user" && (injected || unproven || injected_block) {
        for part in parts {
            if part.kind == ContentKind::UserText {
                part.authority = ContentAuthority::Unknown;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(version: &str, id: &str, parent: Value) -> Value {
        serde_json::json!({
            "type": "user",
            "version": version,
            "entrypoint": "cli",
            "sessionId": "session",
            "isSidechain": false,
            "uuid": id,
            "parentUuid": parent,
            "origin": {"kind": "human"},
            "promptSource": "typed",
            "turnOrigin": "human",
            "message": {"role": "user", "content": format!("Request {id}.")}
        })
    }

    #[test]
    fn characterized_cli_upgrade_keeps_a_linear_root() {
        let mut root = ClaudeRetainedRoot::default();
        let first = user("2.1.278", "root", Value::Null);
        root.observe(&first);
        let bridge = serde_json::json!({
            "type": "system",
            "subtype": "bridge_status",
            "version": "2.1.309",
            "entrypoint": "cli",
            "sessionId": "session",
            "isSidechain": false,
            "uuid": "bridge",
            "parentUuid": "root",
            "isMeta": false,
            "content": "Connected"
        });
        root.observe(&bridge);
        let resumed = user("2.1.309", "resumed", Value::String("bridge".into()));
        root.observe(&resumed);

        assert!(root.admits("session"));
        let mut parts = vec![
            ContentPart::new(ContentKind::UserText, "Request resumed.")
                .with_authority(ContentAuthority::User),
        ];
        root.bind_user_text(&resumed, &mut parts);
        assert!(parts[0].metadata.user_text_history.is_some());
    }

    #[test]
    fn unsupported_cli_version_invalidates_the_retained_root() {
        let mut root = ClaudeRetainedRoot::default();
        root.observe(&user("2.1.278", "root", Value::Null));
        root.observe(&user("2.1.310", "resumed", Value::String("root".into())));

        assert!(!root.admits("session"));
    }

    #[test]
    fn current_cli_user_without_origin_does_not_gain_human_authority() {
        let mut record = user("2.1.309", "user", Value::Null);
        record
            .as_object_mut()
            .expect("test record must be an object")
            .remove("origin");
        let mut parts = [ContentPart::new(ContentKind::UserText, "Request user.")
            .with_authority(ContentAuthority::User)];

        set_user_authority(&record, &mut parts);

        assert_eq!(parts[0].authority, ContentAuthority::Unknown);
    }

    #[test]
    fn characterized_cli_injected_text_never_gains_human_history() {
        for version in CHARACTERIZED_VERSIONS {
            for case in 0..5 {
                let mut record = user(version, "user", Value::Null);
                match case {
                    0 => record["planContent"] = Value::String("Recorded plan.".into()),
                    1 => record["isMeta"] = Value::Bool(true),
                    2 => record["isSynthetic"] = Value::Bool(true),
                    3 => record["origin"]["kind"] = Value::String("api".into()),
                    _ => {
                        record["message"]["content"] = serde_json::json!([
                            {"type":"text","text":"Request user.","isSynthetic":true}
                        ]);
                    }
                }
                let mut root = ClaudeRetainedRoot::default();
                root.observe(&record);
                let mut parts = vec![
                    ContentPart::new(ContentKind::UserText, "Request user.")
                        .with_authority(ContentAuthority::User),
                ];
                set_user_authority(&record, &mut parts);
                root.bind_user_text(&record, &mut parts);

                assert_eq!(parts[0].authority, ContentAuthority::Unknown);
                assert!(parts[0].metadata.user_text_history.is_none());
            }
        }
    }
}
