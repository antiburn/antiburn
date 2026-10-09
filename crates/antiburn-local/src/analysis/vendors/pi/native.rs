//! Retained linear V3 evidence. The journal does not prove external retention or producer origin.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::analysis::framing::PartialReason;
use crate::analysis::interface::{
    ContentAuthority, ContentKind, ContentPart, NormalizedRecord, RecordSink,
};
use crate::analysis::jev::JevInputField;
use crate::analysis::jev_evidence::{
    JevNativeFieldContainer, JevNativeFieldRange, JevOperationState, JevSelectedSkillNormalization,
    JevSelectedSkillProducer, JevSelectedSkillProof, JevSelectedSkillStatus, UserTextHistoryProof,
};

mod reads;

const MAX_CALLS: usize = 4096;
const MAX_CALL_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub(super) struct PiNativeState {
    active: bool,
    invalid: bool,
    last_id: Option<String>,
    session_id: Option<String>,
    calls: HashMap<String, NativeCall>,
    call_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NativeCall {
    name: String,
    record_id: String,
    index: usize,
    arguments: String,
    resolved: bool,
    ambiguous: bool,
}

impl PiNativeState {
    pub(super) fn enable(&mut self, enabled: bool) {
        self.active = enabled;
    }

    pub(super) fn start(&mut self, header: &Value, version: u8) {
        self.active &= version == 3;
        self.session_id = identity(header, "id").map(str::to_owned);
        self.invalid |= header.get("parentSession").is_some() || self.session_id.is_none();
    }

    pub(super) fn invalidate(&mut self) {
        self.invalid = true;
    }

    pub(super) fn incomplete(&self) -> bool {
        self.active && self.invalid
    }

    pub(super) fn observe_entry(&mut self, value: &Value, sink: &mut dyn RecordSink) {
        if !self.active {
            return;
        }
        let id = identity(value, "id");
        let parent_matches = match (&self.last_id, value.get("parentId")) {
            (None, Some(Value::Null)) => true,
            (Some(last), Some(Value::String(parent))) => last == parent,
            _ => false,
        };
        let kind = value["type"].as_str();
        let supported = matches!(
            kind,
            Some("model_change" | "thinking_level_change" | "message" | "session_info" | "label")
        );
        if id.is_none()
            || !parent_matches
            || id == self.last_id.as_deref()
            || !supported
            || !complete(value)
            || (value["message"]["role"] == "user" && !text_only_user(value))
            || value["message"]["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|block| !complete(block)))
        {
            self.invalid = true;
            sink.record(NormalizedRecord::Unusable(
                PartialReason::AttributionIncomplete,
            ));
        }
        self.last_id = id.map(str::to_owned);
    }

    pub(super) fn bind_message(&mut self, value: &Value, parts: &mut Vec<ContentPart>) {
        let message = &value["message"];
        match message["role"].as_str() {
            Some("user") => bind_user(
                value,
                parts,
                self.session_id
                    .as_deref()
                    .filter(|_| self.active && !self.invalid),
            ),
            Some("assistant") => self.bind_calls(value, parts),
            Some("toolResult") => self.bind_result(value, parts),
            _ => {}
        }
        for part in parts {
            for binding in &mut part.metadata.bindings {
                if binding.native_record_id.is_none() {
                    binding.native_record_id = identity(value, "id").map(str::to_owned);
                }
            }
            part.truncated |= !complete(value);
            if self.active
                && !self.invalid
                && !part.truncated
                && message["role"] == "user"
                && part.authority == ContentAuthority::User
                && let (Some(session_id), Some(message_id)) =
                    (&self.session_id, identity(value, "id"))
            {
                part.metadata.user_text_history = Some(UserTextHistoryProof {
                    source_format: crate::analysis::SourceFormat::PiV3Jsonl,
                    session_id: session_id.clone(),
                    message_id: message_id.to_owned(),
                    revision: 1,
                });
            }
        }
    }

    fn bind_calls(&mut self, value: &Value, parts: &mut [ContentPart]) {
        if !self.active || self.invalid {
            return;
        }
        let Some(record_id) = identity(value, "id") else {
            return;
        };
        for (index, block) in value["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            if block["type"] != "toolCall" {
                continue;
            }
            let (Some(id), Some(name)) = (identity(block, "id"), identity(block, "name")) else {
                self.invalid = true;
                continue;
            };
            if let Some(call) = self.calls.get_mut(id) {
                call.ambiguous = true;
                self.invalid = true;
                continue;
            }
            let arguments = block["arguments"].to_string();
            let ambiguous = !complete(block)
                || arguments.len() > crate::analysis::interface::MAX_CONTENT_PART_BYTES;
            let bytes = arguments.len() + record_id.len() + id.len() + name.len();
            if self.calls.len() >= MAX_CALLS || bytes > MAX_CALL_BYTES - self.call_bytes {
                self.invalid = true;
                continue;
            }
            self.call_bytes += bytes;
            self.calls.insert(
                id.to_owned(),
                NativeCall {
                    name: name.to_owned(),
                    record_id: record_id.to_owned(),
                    index,
                    arguments,
                    resolved: false,
                    ambiguous,
                },
            );
        }
        for part in parts {
            if part.kind == ContentKind::ToolInput {
                part.metadata.state = JevOperationState::Pending;
            }
        }
    }

    fn bind_result(&mut self, value: &Value, parts: &mut [ContentPart]) {
        if !self.active || self.invalid {
            return;
        }
        let message = &value["message"];
        let Some(call) = identity(message, "toolCallId").and_then(|id| self.calls.get_mut(id))
        else {
            return;
        };
        if call.ambiguous || call.resolved {
            self.invalid = true;
            return;
        }
        if message["toolName"].as_str() != Some(&call.name) {
            return;
        }
        call.resolved = true;
        if !complete(value) {
            return;
        }
        let state = match message["isError"].as_bool() {
            Some(false) => JevOperationState::Completed,
            Some(true) => JevOperationState::Error,
            None => JevOperationState::Unknown,
        };
        // One native text block is required for an exact result-text binding.
        let Some(blocks) = message["content"].as_array() else {
            return;
        };
        if let [block] = blocks.as_slice()
            && block["type"] == "text"
            && let Some(text) = block["text"].as_str()
            && let [part] = parts
        {
            if part.truncated || part.text != text || !complete(block) {
                return;
            }
            part.metadata.state = state;
            if let Ok(arguments) = serde_json::from_str::<Value>(&call.arguments) {
                let fields = crate::analysis::jev_evidence::normalize_tool_input(
                    &call.name,
                    &call.arguments,
                );
                let mut bindings = crate::analysis::jev_evidence::native_input_bindings(
                    &arguments,
                    &format!("/message/content/{}/arguments", call.index),
                    &fields,
                    JevNativeFieldContainer::Record,
                );
                for binding in &mut bindings {
                    binding.native_record_id = Some(call.record_id.clone());
                }
                part.metadata.bindings.extend(bindings);
            }
            let field = match call.name.as_str() {
                "read" => JevInputField::ReadFileOutput,
                "bash" => JevInputField::BashCommandOutput,
                _ => JevInputField::OtherToolOutput,
            };
            part.metadata.bindings.push(binding(
                value,
                field,
                "/message/content/0/text",
                0,
                text.len(),
            ));
            if call.name == "read" {
                reads::bind_read(value, &call.arguments, part);
            } else if message
                .pointer("/details/truncation/truncated")
                .and_then(Value::as_bool)
                == Some(true)
            {
                part.truncated = true;
            }
        }
    }
}

fn identity<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value[key]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= crate::analysis::EVIDENCE_STRING_CAP)
}

fn complete(value: &Value) -> bool {
    ![value, &value["message"]].iter().any(|value| {
        value["truncated"] == true || value["incomplete"] == true || value["complete"] == false
    })
}

fn text_only_user(value: &Value) -> bool {
    let content = &value["message"]["content"];
    content.is_string()
        || content.as_array().is_some_and(|blocks| {
            !blocks.is_empty()
                && blocks
                    .iter()
                    .all(|block| block["type"] == "text" && block["text"].is_string())
        })
}

fn binding(
    value: &Value,
    field: JevInputField,
    pointer: &str,
    start: usize,
    end: usize,
) -> JevNativeFieldRange {
    JevNativeFieldRange {
        native_record_id: identity(value, "id").map(str::to_owned),
        field,
        container: JevNativeFieldContainer::Record,
        pointer: pointer.to_owned(),
        start,
        end,
    }
}

fn bind_user(value: &Value, parts: &mut Vec<ContentPart>, session_id: Option<&str>) {
    let content = &value["message"]["content"];
    let strings: Vec<_> = if let Some(text) = content.as_str() {
        vec![("/message/content".to_owned(), text)]
    } else {
        content
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(index, block)| {
                (block["type"] == "text")
                    .then(|| block["text"].as_str())
                    .flatten()
                    .map(|text| (format!("/message/content/{index}/text"), text))
            })
            .collect()
    };
    parts.clear();
    for (pointer, text) in strings {
        let wrapper = skill_wrapper(text);
        let wrapper_like = text.contains("<skill") || text.contains("</skill>");
        let (end, args_start) = wrapper
            .as_ref()
            .map_or((text.len(), text.len()), |wrapper| {
                (wrapper.end, wrapper.args_start)
            });
        let authority = if wrapper_like {
            ContentAuthority::Unknown
        } else {
            ContentAuthority::User
        };
        let mut part =
            ContentPart::new(ContentKind::UserText, &text[..end]).with_authority(authority);
        part.metadata
            .bindings
            .push(binding(value, JevInputField::UserMessage, &pointer, 0, end));
        if !part.truncated
            && let (Some(wrapper), Some(session_id), Some(message_id)) =
                (wrapper, session_id, identity(value, "id"))
        {
            let proof = JevSelectedSkillProof {
                source_format: crate::analysis::SourceFormat::PiV3Jsonl,
                session_id: session_id.to_owned(),
                message_id: message_id.to_owned(),
                name: wrapper.name.to_owned(),
                location: wrapper.location.to_owned(),
                producer: JevSelectedSkillProducer::PiSkillWrapper,
                normalization: JevSelectedSkillNormalization::PiSkillWrapper,
                normalization_revision: 1,
                ranges: part.metadata.bindings.clone(),
                status: JevSelectedSkillStatus::DocumentSelected,
                complete: true,
            };
            if proof.is_bounded() {
                part.metadata.selected_skill = Some(proof);
            }
        }
        parts.push(part);
        if args_start < text.len() {
            let mut args = ContentPart::new(ContentKind::UserText, &text[args_start..]);
            args.metadata.bindings.push(binding(
                value,
                JevInputField::UserMessage,
                &pointer,
                args_start,
                text.len(),
            ));
            parts.push(args);
        }
    }
}

/// Match the core expansion, not a system skill listing or an arbitrary XML fragment.
fn skill_wrapper(text: &str) -> Option<SkillWrapper<'_>> {
    let rest = text.strip_prefix("<skill name=\"")?;
    let (name, rest) = rest.split_once("\" location=\"")?;
    super::pi_skill_identity(name)?;
    let (location, body) = rest.split_once("\">\n")?;
    if location.contains(['"', '\n']) || !location.ends_with("/SKILL.md") {
        return None;
    }
    let (references, _) = body.split_once("\n\n")?;
    let directory = location.strip_suffix("/SKILL.md")?;
    if references != format!("References are relative to {directory}.") {
        return None;
    }
    let (body, args) = text.split_once("\n</skill>")?;
    if body.matches("<skill").count() != 1 || args.contains("</skill>") || args.contains("<skill") {
        return None;
    }
    let end = body.len() + "\n</skill>".len();
    let args_start = if args.is_empty() {
        end
    } else {
        let args = args.strip_prefix("\n\n")?;
        if args.is_empty() {
            return None;
        }
        end + 2
    };
    Some(SkillWrapper {
        name,
        location,
        end,
        args_start,
    })
}

struct SkillWrapper<'a> {
    name: &'a str,
    location: &'a str,
    end: usize,
    args_start: usize,
}
