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

const VERSION: &str = "2.1.278";
const MAX_RECORDS: usize = 16_384;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ClaudeRetainedRoot {
    session: Option<String>,
    last: Option<String>,
    fingerprints: BTreeMap<String, [u8; 32]>,
    invalid: bool,
    characterized: bool,
}

impl ClaudeRetainedRoot {
    pub(super) fn invalidate(&mut self) {
        self.invalid = true;
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

    pub(super) fn observe(&mut self, record: &Value) {
        self.characterized |= record["version"] == VERSION && record["entrypoint"] == "cli";
        if record["subtype"] == "compact_boundary"
            || record["isCompactSummary"] == true
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
            record.get("parentUuid") == Some(&Value::Null)
                && record.get("logicalParentUuid").is_none_or(Value::is_null)
                && (is_human(record) || is_informational_root(record))
        } else {
            record["parentUuid"].as_str() == self.last.as_deref()
                && record.get("logicalParentUuid").is_none_or(Value::is_null)
        };
        if record["version"] != VERSION
            || record["entrypoint"] != "cli"
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
                    (block["type"] == "text").then_some(())?;
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

fn is_informational_root(record: &Value) -> bool {
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

pub(super) fn set_user_authority(record: &Value, parts: &mut [ContentPart]) {
    let injected = has_injection_flags(record)
        || record
            .pointer("/origin/kind")
            .is_some_and(|kind| kind != "human")
        || record
            .get("promptSource")
            .is_some_and(|source| source != "typed")
        || record
            .get("turnOrigin")
            .is_some_and(|origin| origin != "human");
    let unproven = record["version"] == VERSION && !is_human(record);
    if record["type"] == "user" && (injected || unproven) {
        for part in parts {
            if part.kind == ContentKind::UserText {
                part.authority = ContentAuthority::Unknown;
            }
        }
    }
}
