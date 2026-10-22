use serde::{Deserialize, Serialize};

use super::{JevInputSelection, JevNativeFieldContainer, JevNativeFieldRange};
use crate::analysis::SourceFormat;
use crate::checks::ignored_instructions::sha256_hex;

pub(super) fn opencode_v2_source(source: SourceFormat) -> bool {
    matches!(
        source,
        SourceFormat::OpenCodeSqliteV2 | SourceFormat::OpenCodeV2SessionExport
    )
}

pub(super) fn opencode_v2_action(action: &super::ContentAction) -> bool {
    opencode_v2_reference(action)
        && action
            .tool_call_id
            .as_deref()
            .is_some_and(native_skill_identity_is_valid)
        && !action.truncated
}

fn opencode_v2_reference(action: &super::ContentAction) -> bool {
    let identity = |value: &str| {
        !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
    };
    let digest = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    action.reference.stable
        && digest(&action.reference.id)
        && action
            .reference
            .native_record_id
            .as_deref()
            .is_some_and(identity)
        && digest(&action.reference.source_key_digest)
        && digest(&action.reference.thread_digest)
        && action.reference.id
            == sha256_hex(
                format!(
                    "{}\0{}\0{}\0{}",
                    action.reference.source_key_digest,
                    action
                        .reference
                        .native_record_id
                        .as_deref()
                        .unwrap_or_default(),
                    action.reference.turn_index,
                    action.reference.part_index
                )
                .as_bytes(),
            )
        && action.text.len() <= crate::analysis::interface::MAX_CONTENT_PART_BYTES
        && action.metadata.bindings.len() <= 16
}

pub(super) fn opencode_v2_index(value: &str) -> bool {
    value
        .parse::<usize>()
        .is_ok_and(|index| index < 1024 && index.to_string() == value)
}

/// Adapter-validated skill evidence. Completion does not mean task success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevRecordedSkillResult {
    pub identity: JevRecordedSkillIdentityKind,
    pub name: Option<String>,
    pub location: Option<String>,
    pub state: super::JevOperationState,
    pub status: JevRecordedSkillStatus,
    pub source_format: SourceFormat,
    pub session_id: Option<String>,
    pub message_id: Option<String>,
    pub part_index: u32,
    pub call_id: Option<String>,
    pub request_message_id: Option<String>,
    pub request_part_index: Option<u32>,
    pub field: super::JevInputField,
    pub ranges: Vec<JevNativeFieldRange>,
    pub text_digest: String,
    pub complete: bool,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_skill: Option<JevNativeSkillProof>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevNativeSkillProducer {
    OpenCodeV2Skill843a6c51,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevNativeSkillOrigin {
    ToolRequest,
    ToolResult,
    PromptAttachment { index: u32 },
    SkillMessage,
}

impl JevNativeSkillOrigin {
    pub fn is_injected(self) -> bool {
        matches!(self, Self::PromptAttachment { .. } | Self::SkillMessage)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevNativeSkillBody {
    NotApplicable,
    Recorded,
    Unavailable,
    Truncated,
}

/// Native IDs and display names are separate. Injected text does not prove a call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevNativeSkillProof {
    pub producer: JevNativeSkillProducer,
    pub origin: JevNativeSkillOrigin,
    pub id: String,
    pub display_name: Option<String>,
    pub body: JevNativeSkillBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevRecordedSkillIdentityKind {
    Name,
    InferredName,
    Document,
    Unknown,
    NativeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevRecordedSkillStatus {
    Requested,
    DocumentSelected,
    Failed,
    Unknown,
}

pub fn skill_text_digest(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

pub fn is_recorded_skill_selection(action: &super::ContentAction) -> bool {
    if let Some(fact) = &action.metadata.recorded_skill_result
        && fact
            .native_skill
            .as_ref()
            .is_some_and(|proof| proof.origin.is_injected())
    {
        return fact.matches_action(action, fact.source_format);
    }
    action.turn_role == "user"
        && action.turn_scope == "main"
        && action
            .metadata
            .recorded_skill_result
            .as_ref()
            .is_some_and(|fact| {
                fact.field == super::JevInputField::UserMessage
                    && fact.complete
                    && !fact.truncated
                    && matches!(
                        fact.status,
                        JevRecordedSkillStatus::DocumentSelected
                            | JevRecordedSkillStatus::Requested
                    )
                    && fact.matches_action(action, fact.source_format)
            })
}

impl JevRecordedSkillResult {
    pub fn matches_action(&self, action: &super::ContentAction, source: SourceFormat) -> bool {
        if opencode_v2_source(source) || self.native_skill.is_some() {
            return self.matches_opencode_v2_action(action, source);
        }
        self.source_format == source
            && self.message_id == action.reference.native_record_id
            && self.part_index == action.reference.part_index
            && self.call_id == action.tool_call_id
            && self.text_digest == skill_text_digest(skill_action_text(action))
            && (action.kind != "tool_input"
                || action.text.is_empty()
                || action
                    .normalized_fields
                    .as_ref()
                    .and_then(|fields| fields.values.get(&super::JevInputField::OtherToolInput))
                    .is_none_or(|text| text == &action.text))
            && self.ranges.len() <= 16
            && self.ranges.iter().all(|range| {
                action.metadata.bindings.contains(range)
                    && range.pointer.len() <= 256
                    && range.start < range.end
                    && range.end <= crate::analysis::interface::MAX_CONTENT_PART_BYTES
            })
            && self.name.as_deref().is_none_or(skill_name_is_valid)
            && self
                .location
                .as_ref()
                .is_none_or(|path| !path.is_empty() && path.len() <= 4096)
            && self
                .session_id
                .as_ref()
                .is_none_or(|id| !id.is_empty() && id.len() <= 256)
            && self
                .request_message_id
                .as_ref()
                .is_none_or(|id| !id.is_empty() && id.len() <= 256)
            && self.request_message_id.is_some() == self.request_part_index.is_some()
            && matches!(
                (self.field, action.kind.as_str()),
                (super::JevInputField::UserMessage, "user" | "user_text")
                    | (super::JevInputField::OtherToolInput, "tool_input")
                    | (super::JevInputField::OtherToolOutput, "tool_result")
            )
            && match self.field {
                super::JevInputField::OtherToolInput => {
                    self.status == JevRecordedSkillStatus::Requested
                }
                super::JevInputField::UserMessage => {
                    self.identity == JevRecordedSkillIdentityKind::Document
                        && matches!(
                            self.status,
                            JevRecordedSkillStatus::DocumentSelected
                                | JevRecordedSkillStatus::Requested
                        )
                }
                super::JevInputField::OtherToolOutput => true,
                _ => false,
            }
            && match self.status {
                JevRecordedSkillStatus::DocumentSelected => {
                    self.identity == JevRecordedSkillIdentityKind::Document
                        && self.state == super::JevOperationState::Completed
                }
                JevRecordedSkillStatus::Failed => self.state == super::JevOperationState::Error,
                JevRecordedSkillStatus::Requested | JevRecordedSkillStatus::Unknown => true,
            }
            && match self.identity {
                JevRecordedSkillIdentityKind::Unknown => {
                    self.name.is_none() && self.location.is_none()
                }
                JevRecordedSkillIdentityKind::Name | JevRecordedSkillIdentityKind::InferredName => {
                    self.name.is_some() && self.location.is_none()
                }
                JevRecordedSkillIdentityKind::Document => {
                    self.name.is_some()
                        && self.location.is_some()
                        && self.session_id.is_some()
                        && self.message_id.is_some()
                        && self.ranges.len() == 1
                        && self.ranges[0].native_record_id == self.message_id
                        && self.ranges[0].field == self.field
                        && self.ranges[0].start == 0
                        && self.ranges[0].end == action.text.len()
                }
                JevRecordedSkillIdentityKind::NativeId => false,
            }
    }

    pub fn opencode_v2(
        part: &crate::analysis::ContentPart,
        source: SourceFormat,
        session_id: &str,
        message_id: &str,
        part_index: u32,
        proof: JevNativeSkillProof,
    ) -> Self {
        let field = match proof.origin {
            JevNativeSkillOrigin::ToolRequest => super::JevInputField::OtherToolInput,
            JevNativeSkillOrigin::ToolResult => super::JevInputField::OtherToolOutput,
            JevNativeSkillOrigin::PromptAttachment { .. } | JevNativeSkillOrigin::SkillMessage => {
                super::JevInputField::UserMessage
            }
        };
        Self {
            identity: JevRecordedSkillIdentityKind::NativeId,
            name: None,
            location: None,
            state: part.metadata.state,
            status: match proof.origin {
                JevNativeSkillOrigin::ToolRequest => JevRecordedSkillStatus::Requested,
                JevNativeSkillOrigin::ToolResult
                    if part.metadata.state == super::JevOperationState::Error =>
                {
                    JevRecordedSkillStatus::Failed
                }
                _ => JevRecordedSkillStatus::Unknown,
            },
            source_format: source,
            session_id: Some(session_id.to_owned()),
            message_id: Some(message_id.to_owned()),
            part_index,
            call_id: part.tool_call_id.clone(),
            request_message_id: None,
            request_part_index: None,
            field,
            ranges: part
                .metadata
                .bindings
                .iter()
                .filter(|range| range.field == field)
                .cloned()
                .collect(),
            text_digest: skill_text_digest(&part.text),
            complete: !part.truncated
                && matches!(
                    proof.body,
                    JevNativeSkillBody::NotApplicable | JevNativeSkillBody::Recorded
                ),
            truncated: part.truncated,
            native_skill: Some(proof),
        }
    }

    fn matches_opencode_v2_action(
        &self,
        action: &super::ContentAction,
        source: SourceFormat,
    ) -> bool {
        let Some(proof) = &self.native_skill else {
            return false;
        };
        if !opencode_v2_source(source)
            || self.source_format != source
            || !opencode_v2_reference(action)
            || self.identity != JevRecordedSkillIdentityKind::NativeId
            || self.name.is_some()
            || self.location.is_some()
            || self.message_id != action.reference.native_record_id
            || self.part_index != action.reference.part_index
            || self.call_id != action.tool_call_id
            || self.text_digest != skill_text_digest(skill_action_text(action))
            || self.state != action.metadata.state
            || self.session_id.as_ref().is_none_or(|session| {
                !native_skill_identity_is_valid(session)
                    || action.reference.thread_digest != sha256_hex(session.as_bytes())
            })
            || proof.producer != JevNativeSkillProducer::OpenCodeV2Skill843a6c51
            || !native_skill_identity_is_valid(&proof.id)
            || proof
                .display_name
                .as_deref()
                .is_some_and(|name| !native_skill_identity_is_valid(name))
            || self.ranges.iter().any(|range| {
                range.native_record_id != self.message_id
                    || range.container != JevNativeFieldContainer::Record
                    || range.field != self.field
                    || range.start != 0
                    || range.end > crate::analysis::interface::MAX_CONTENT_PART_BYTES
                    || !action.metadata.bindings.contains(range)
            })
            || self.request_message_id.is_some() != self.request_part_index.is_some()
            || self
                .request_message_id
                .as_deref()
                .is_some_and(|id| !native_skill_identity_is_valid(id))
            || !native_skill_body_matches(self, action, proof)
        {
            return false;
        }
        match proof.origin {
            JevNativeSkillOrigin::ToolRequest => {
                self.field == super::JevInputField::OtherToolInput
                    && action.tool_name.as_deref() == Some("skill")
                    && action
                        .tool_call_id
                        .as_deref()
                        .is_some_and(native_skill_identity_is_valid)
                    && self.status == JevRecordedSkillStatus::Requested
                    && self.request_message_id.is_none()
                    && opencode_v2_skill_request_id(action).as_deref() == Some(proof.id.as_str())
                    && proof.display_name.is_none()
                    && self.ranges.len() == 1
                    && self.ranges[0].end == proof.id.len()
                    && opencode_v2_skill_pointer(&self.ranges[0].pointer, "/input/id")
            }
            JevNativeSkillOrigin::ToolResult => self.matches_opencode_v2_result(action, proof),
            JevNativeSkillOrigin::PromptAttachment { index } => {
                index < 64
                    && action.turn_role == "user"
                    && self.matches_opencode_v2_injection(
                        action,
                        proof,
                        &format!("/skills/{index}/id"),
                        &format!("/skills/{index}/name"),
                        &format!("/skills/{index}/text"),
                    )
            }
            JevNativeSkillOrigin::SkillMessage => {
                action.turn_role == "system"
                    && self.matches_opencode_v2_injection(action, proof, "/skill", "/name", "/text")
            }
        }
    }

    fn matches_opencode_v2_result(
        &self,
        action: &super::ContentAction,
        proof: &JevNativeSkillProof,
    ) -> bool {
        let Some(prefix) = self.ranges.iter().find_map(|range| {
            range
                .pointer
                .strip_suffix("/input/id")
                .filter(|_| opencode_v2_skill_pointer(&range.pointer, "/input/id"))
        }) else {
            return false;
        };
        self.field == super::JevInputField::OtherToolOutput
            && action.kind == "tool_result"
            && action.authority == "tool"
            && action.turn_role == "assistant"
            && action.tool_name.as_deref() == Some("skill")
            && action
                .tool_call_id
                .as_deref()
                .is_some_and(native_skill_identity_is_valid)
            && native_skill_range(self, &format!("{prefix}/input/id"), proof.id.len())
            && proof.display_name.as_ref().is_none_or(|name| {
                native_skill_range(self, &format!("{prefix}/metadata/name"), name.len())
            })
            && self.ranges.len()
                == 1 + usize::from(proof.display_name.is_some())
                    + usize::from(matches!(
                        proof.body,
                        JevNativeSkillBody::Recorded | JevNativeSkillBody::Truncated
                    ))
            && match self.state {
                super::JevOperationState::Error => self.status == JevRecordedSkillStatus::Failed,
                super::JevOperationState::Completed => {
                    self.status == JevRecordedSkillStatus::Unknown
                }
                _ => false,
            }
            && self.ranges.iter().all(|range| {
                if range.pointer == format!("{prefix}/input/id") {
                    range.end == proof.id.len()
                } else if let Some(name) = &proof.display_name
                    && range.pointer == format!("{prefix}/metadata/name")
                {
                    range.end == name.len()
                } else if range.pointer == format!("{prefix}/error/message") {
                    range.end == action.text.len() && self.state == super::JevOperationState::Error
                } else {
                    range.end == action.text.len()
                        && range
                            .pointer
                            .strip_prefix(&format!("{prefix}/content/"))
                            .and_then(|rest| rest.strip_suffix("/text"))
                            .is_some_and(opencode_v2_index)
                }
            })
            && match proof.body {
                JevNativeSkillBody::Recorded | JevNativeSkillBody::Truncated => {
                    self.ranges
                        .iter()
                        .filter(|range| {
                            range.pointer.starts_with(&format!("{prefix}/content/"))
                                || range.pointer == format!("{prefix}/error/message")
                        })
                        .count()
                        == 1
                }
                JevNativeSkillBody::Unavailable | JevNativeSkillBody::NotApplicable => true,
            }
    }

    fn matches_opencode_v2_injection(
        &self,
        action: &super::ContentAction,
        proof: &JevNativeSkillProof,
        id_pointer: &str,
        name_pointer: &str,
        body_pointer: &str,
    ) -> bool {
        self.field == super::JevInputField::UserMessage
            && matches!(action.kind.as_str(), "user" | "user_text")
            && action.authority == "unknown"
            && action.turn_scope == "main"
            && action.tool_name.is_none()
            && action.tool_call_id.is_none()
            && action.metadata.human_text.is_none()
            && action.metadata.user_text_history.is_none()
            && self.status == JevRecordedSkillStatus::Unknown
            && self.state == super::JevOperationState::Unknown
            && self.request_message_id.is_none()
            && proof
                .display_name
                .as_ref()
                .is_some_and(|name| native_skill_range(self, name_pointer, name.len()))
            && native_skill_range(self, id_pointer, proof.id.len())
            && self.ranges.len()
                == 2 + usize::from(matches!(
                    proof.body,
                    JevNativeSkillBody::Recorded | JevNativeSkillBody::Truncated
                ))
            && (!matches!(
                proof.body,
                JevNativeSkillBody::Recorded | JevNativeSkillBody::Truncated
            ) || native_skill_range(self, body_pointer, action.text.len()))
    }
}

fn native_skill_range(fact: &JevRecordedSkillResult, pointer: &str, len: usize) -> bool {
    fact.ranges
        .iter()
        .filter(|range| range.pointer == pointer && range.end == len)
        .count()
        == 1
}

fn native_skill_body_matches(
    fact: &JevRecordedSkillResult,
    action: &super::ContentAction,
    proof: &JevNativeSkillProof,
) -> bool {
    fact.truncated == action.truncated
        && match proof.body {
            JevNativeSkillBody::NotApplicable => {
                proof.origin == JevNativeSkillOrigin::ToolRequest
                    && fact.complete
                    && !fact.truncated
            }
            JevNativeSkillBody::Recorded => {
                proof.origin != JevNativeSkillOrigin::ToolRequest
                    && fact.complete
                    && !fact.truncated
                    && !action.text.is_empty()
            }
            JevNativeSkillBody::Unavailable => {
                proof.origin != JevNativeSkillOrigin::ToolRequest
                    && !fact.complete
                    && !fact.truncated
                    && action.text.is_empty()
            }
            JevNativeSkillBody::Truncated => {
                proof.origin != JevNativeSkillOrigin::ToolRequest
                    && !fact.complete
                    && fact.truncated
                    && !action.text.is_empty()
            }
        }
}

fn native_skill_identity_is_valid(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn opencode_v2_skill_pointer(pointer: &str, suffix: &str) -> bool {
    pointer
        .strip_prefix("/content/")
        .and_then(|rest| rest.strip_suffix(suffix))
        .and_then(|rest| rest.strip_suffix("/state"))
        .is_some_and(opencode_v2_index)
}

fn opencode_v2_skill_request_id(action: &super::ContentAction) -> Option<String> {
    if action.kind != "tool_input"
        || action.authority != "assistant"
        || action.turn_role != "assistant"
    {
        return None;
    }
    let fields = action.normalized_fields.as_ref()?;
    if fields.malformed
        || fields.category != Some(crate::analysis::jev::JevNormalizedCategory::OtherTool)
    {
        return None;
    }
    let selected = fields.values.get(&super::JevInputField::OtherToolInput)?;
    if !action.text.is_empty() && selected != &action.text {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(selected).ok()?;
    let input = value.as_object()?;
    let id = input.get("id")?.as_str()?;
    if input.len() != 1 || !native_skill_identity_is_valid(id) {
        return None;
    }
    action
        .metadata
        .bindings
        .iter()
        .any(|range| {
            range.field == super::JevInputField::OtherToolInput
                && range.native_record_id == action.reference.native_record_id
                && range.container == JevNativeFieldContainer::Record
                && range.start == 0
                && range.end == id.len()
                && opencode_v2_skill_pointer(&range.pointer, "/input/id")
        })
        .then(|| id.to_owned())
}

pub fn skill_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && !name.chars().any(|ch| {
            ch.is_whitespace() || ch.is_control() || matches!(ch, '/' | '\\' | '<' | '>' | '"')
        })
}

/// Convert adapter evidence before smart checks receive actions.
pub fn normalize_recorded_skill(action: &mut super::ContentAction, source: SourceFormat) {
    if action
        .metadata
        .recorded_skill_result
        .as_ref()
        .and_then(|fact| fact.native_skill.as_ref())
        .is_some_and(|proof| proof.origin.is_injected())
    {
        action.authority = "unknown".to_owned();
        action.metadata.user_text_history = None;
        action.metadata.human_text = None;
    }
    if let Some(proof) = action.metadata.selected_skill.take() {
        action.metadata.recorded_skill_result = normalize_selected_skill(action, source, &proof);
        return;
    }
    if action.metadata.recorded_skill_result.is_some() {
        if !action
            .metadata
            .recorded_skill_result
            .as_ref()
            .is_some_and(|fact| fact.matches_action(action, source))
        {
            action.metadata.recorded_skill_result = None;
        }
        return;
    }
    if opencode_v2_source(source) {
        return;
    }
    let tool = match source {
        SourceFormat::ClaudeJsonl => action.tool_name.as_deref() == Some("Skill"),
        SourceFormat::OpenCodeSqliteV1 => action.tool_name.as_deref() == Some("skill"),
        SourceFormat::PiV3Jsonl => matches!(action.tool_name.as_deref(), Some("Skill" | "skill")),
        _ => false,
    };
    if !tool || !matches!(action.kind.as_str(), "tool_input" | "tool_result") {
        return;
    }
    let mut fact = skill_fact(action, source);
    if action.kind == "tool_input" {
        let (identity, name) = request_identity(skill_action_text(action), source);
        fact.identity = identity;
        fact.name = name;
        fact.field = super::JevInputField::OtherToolInput;
        fact.status = JevRecordedSkillStatus::Requested;
    } else if source == SourceFormat::ClaudeJsonl
        && action.metadata.state == super::JevOperationState::Error
        && action.reference.native_record_id.is_some()
        && action.metadata.bindings.iter().any(|range| {
            range.native_record_id == action.reference.native_record_id
                && range.field == super::JevInputField::OtherToolOutput
                && range.container == JevNativeFieldContainer::Record
                && range.pointer
                    == format!("/message/content/{}/content", action.reference.part_index)
                && range.start == 0
                && range.end == action.text.len()
        })
    {
        fact.status = JevRecordedSkillStatus::Failed;
    }
    action.metadata.recorded_skill_result = Some(fact);
}

fn skill_fact(action: &super::ContentAction, source: SourceFormat) -> JevRecordedSkillResult {
    JevRecordedSkillResult {
        identity: JevRecordedSkillIdentityKind::Unknown,
        name: None,
        location: None,
        state: action.metadata.state,
        status: JevRecordedSkillStatus::Unknown,
        source_format: source,
        session_id: None,
        message_id: action.reference.native_record_id.clone(),
        part_index: action.reference.part_index,
        call_id: action.tool_call_id.clone(),
        request_message_id: None,
        request_part_index: None,
        field: super::JevInputField::OtherToolOutput,
        ranges: action.metadata.bindings.clone(),
        text_digest: skill_text_digest(skill_action_text(action)),
        complete: !action.truncated,
        truncated: action.truncated,
        native_skill: None,
    }
}

fn skill_action_text(action: &super::ContentAction) -> &str {
    if action.kind == "tool_input"
        && action.text.is_empty()
        && let Some(text) = action
            .normalized_fields
            .as_ref()
            .and_then(|fields| fields.values.get(&super::JevInputField::OtherToolInput))
    {
        text
    } else {
        &action.text
    }
}

fn request_identity(
    text: &str,
    source: SourceFormat,
) -> (JevRecordedSkillIdentityKind, Option<String>) {
    use JevRecordedSkillIdentityKind as Identity;
    let Ok(mut input) = serde_json::from_str::<serde_json::Value>(text) else {
        return (Identity::Unknown, None);
    };
    if let Some(encoded) = input.as_str() {
        let Ok(decoded) = serde_json::from_str(encoded) else {
            return (Identity::Unknown, None);
        };
        input = decoded;
    }
    let keys: &[&str] = if source == SourceFormat::OpenCodeSqliteV1 {
        &["name"]
    } else {
        &["skill", "name", "skill_name", "skillName"]
    };
    let names: Vec<_> = keys
        .iter()
        .filter_map(|key| input.get(*key).and_then(serde_json::Value::as_str))
        .collect();
    if let Some(name) = names.first() {
        if skill_name_is_valid(name) && names.iter().all(|other| other == name) {
            let identity = if input.get(keys[0]).is_some() {
                Identity::Name
            } else {
                Identity::InferredName
            };
            return (identity, Some((*name).into()));
        }
        return (Identity::Unknown, None);
    }
    let command = input
        .get("command")
        .and_then(serde_json::Value::as_str)
        .and_then(|command| command.split_whitespace().next())
        .map(|name| name.trim_start_matches('/'));
    let path = input
        .get("path")
        .and_then(serde_json::Value::as_str)
        .and_then(|path| {
            let mut parts = path.rsplit(['/', '\\']);
            parts
                .next()
                .filter(|file| file.eq_ignore_ascii_case("SKILL.md"))?;
            parts.next()
        });
    if let Some(name) = command.or(path).filter(|name| skill_name_is_valid(name)) {
        (Identity::InferredName, Some(name.into()))
    } else {
        (Identity::Unknown, None)
    }
}

fn normalize_selected_skill(
    action: &super::ContentAction,
    source: SourceFormat,
    proof: &JevSelectedSkillProof,
) -> Option<JevRecordedSkillResult> {
    if !proof.is_bounded()
        || proof.source_format != source
        || Some(proof.message_id.as_str()) != action.reference.native_record_id.as_deref()
        || proof.normalization_revision != 1
        || !proof.complete
        || action.truncated
        || !matches!(action.kind.as_str(), "user" | "user_text")
        || action.turn_role != "user"
        || action.turn_scope != "main"
    {
        return None;
    }
    let range = &proof.ranges[0];
    if range.field != super::JevInputField::UserMessage
        || range.start != 0
        || range.end != action.text.len()
        || !action.metadata.bindings.contains(range)
    {
        return None;
    }
    match (source, proof.producer, proof.normalization) {
        (
            SourceFormat::CodexRolloutJsonl,
            JevSelectedSkillProducer::CodexSelectedSkillInstructions,
            JevSelectedSkillNormalization::CodexSkillDocument,
        ) => {
            if range.pointer != format!("/payload/content/{}/text", action.reference.part_index)
                || codex_document(&action.text)
                    != Some((proof.name.as_str(), proof.location.as_str()))
            {
                return None;
            }
        }
        (
            SourceFormat::PiV3Jsonl,
            JevSelectedSkillProducer::PiSkillWrapper,
            JevSelectedSkillNormalization::PiSkillWrapper,
        ) => {
            let pointer = range.pointer.as_str();
            let content_pointer = pointer == "/message/content"
                || pointer
                    .strip_prefix("/message/content/")
                    .and_then(|rest| rest.strip_suffix("/text"))
                    .is_some_and(|index| index.parse::<u32>().is_ok());
            if !content_pointer || !pi_document(&action.text, &proof.name, &proof.location) {
                return None;
            }
        }
        _ => return None,
    }
    let mut fact = skill_fact(action, source);
    fact.identity = JevRecordedSkillIdentityKind::Document;
    fact.name = Some(proof.name.clone());
    fact.location = Some(proof.location.clone());
    fact.session_id = Some(proof.session_id.clone());
    fact.state = super::JevOperationState::Completed;
    fact.status = match proof.status {
        JevSelectedSkillStatus::DocumentSelected => JevRecordedSkillStatus::DocumentSelected,
        JevSelectedSkillStatus::Requested => JevRecordedSkillStatus::Requested,
    };
    fact.field = super::JevInputField::UserMessage;
    fact.ranges = proof.ranges.clone();
    Some(fact)
}

fn codex_document(text: &str) -> Option<(&str, &str)> {
    let body = text
        .strip_prefix("<skill>\n<name>")?
        .strip_suffix("\n</skill>")?;
    let (name, body) = body.split_once("</name>\n<path>")?;
    if !skill_name_is_valid(name) {
        return None;
    }
    let (path, mut document) = body.split_once("</path>\n")?;
    if path.is_empty() || path.contains(['\n', '<', '>']) {
        return None;
    }
    if let Some(metadata) = document.strip_prefix("<resource_access>") {
        let (metadata, rest) = metadata.split_once("</resource_access>\n")?;
        if !serde_json::from_str::<serde_json::Value>(metadata)
            .ok()?
            .is_object()
        {
            return None;
        }
        document = rest;
    }
    (!document.trim().is_empty() && !document.contains("</skill>")).then_some((name, path))
}

fn pi_document(text: &str, name: &str, location: &str) -> bool {
    if location.contains(['"', '<', '>']) {
        return false;
    }
    let Some(directory) = location.strip_suffix("/SKILL.md") else {
        return false;
    };
    let prefix = format!(
        "<skill name=\"{name}\" location=\"{location}\">\nReferences are relative to {directory}.\n\n"
    );
    text.strip_prefix(&prefix)
        .and_then(|body| body.strip_suffix("\n</skill>"))
        .is_some_and(|body| {
            !body.trim().is_empty() && !body.contains("<skill") && !body.contains("</skill>")
        })
}

/// Selection evidence does not establish human authority or successful execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevSelectedSkillProof {
    pub source_format: SourceFormat,
    pub session_id: String,
    pub message_id: String,
    pub name: String,
    pub location: String,
    pub producer: JevSelectedSkillProducer,
    pub normalization: JevSelectedSkillNormalization,
    pub normalization_revision: u32,
    pub ranges: Vec<JevNativeFieldRange>,
    pub status: JevSelectedSkillStatus,
    pub complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevSelectedSkillProducer {
    CodexSelectedSkillInstructions,
    PiSkillWrapper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevSelectedSkillNormalization {
    CodexSkillDocument,
    PiSkillWrapper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevSelectedSkillStatus {
    DocumentSelected,
    Requested,
}

impl JevSelectedSkillProof {
    pub fn is_bounded(&self) -> bool {
        let identity = |value: &str| {
            !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
        };
        identity(&self.session_id)
            && identity(&self.message_id)
            && identity(&self.name)
            && !self
                .name
                .chars()
                .any(|ch| ch.is_whitespace() || matches!(ch, '/' | '\\' | '<' | '>' | '"'))
            && !self.location.is_empty()
            && self.location.len() <= 4096
            && !self.location.chars().any(char::is_control)
            && self.ranges.len() == 1
            && self.ranges.iter().all(|range| {
                range.native_record_id.as_deref() == Some(self.message_id.as_str())
                    && range.container == JevNativeFieldContainer::Record
                    && !range.pointer.is_empty()
                    && range.pointer.len() <= 256
                    && range.start < range.end
                    && range.end <= crate::analysis::interface::MAX_CONTENT_PART_BYTES
            })
    }

    pub fn selected(&self, selection: JevInputSelection) -> Option<Self> {
        (self.is_bounded()
            && self
                .ranges
                .iter()
                .all(|range| selection.includes(range.field)))
        .then(|| self.clone())
    }
}
