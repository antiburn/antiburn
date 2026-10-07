//! Source-qualified generated context cannot supply human authorization.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::analysis::SourceFormat;
use crate::analysis::jev::JevInputField;
use crate::analysis::jev_evidence::{
    ContentAction, ContentEventReference, JevNativeFieldContainer, JevNativeFieldRange,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevNonAuthorizingContextKind {
    Environment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevNonAuthorizingContextProducer {
    CodexEnvironmentContext01601,
}

/// The adapter validates producer origin and native syntax before it emits this proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevNonAuthorizingContextProof {
    pub kind: JevNonAuthorizingContextKind,
    pub producer: JevNonAuthorizingContextProducer,
    pub source_format: SourceFormat,
    pub session_id: String,
    pub message_id: String,
    pub range: JevNativeFieldRange,
    pub text_digest: String,
    pub complete: bool,
    pub normalization_revision: u32,
}

/// Checks and the desktop consume this fact without native-schema parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevNonAuthorizingContext {
    pub kind: JevNonAuthorizingContextKind,
    pub source_format: SourceFormat,
    pub session_id: String,
    pub message_id: String,
    pub source: ContentEventReference,
    pub turn_scope: String,
    pub range: JevNativeFieldRange,
    pub text_digest: String,
    pub complete: bool,
    pub normalization_revision: u32,
}

pub fn non_authorizing_context_digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

impl JevNonAuthorizingContextProof {
    pub fn is_bounded(&self) -> bool {
        valid_identity(&self.session_id)
            && valid_identity(&self.message_id)
            && self.complete
            && self.normalization_revision == 1
            && self.source_format == SourceFormat::CodexRolloutJsonl
            && self.producer == JevNonAuthorizingContextProducer::CodexEnvironmentContext01601
            && valid_range(&self.range, &self.message_id)
            && valid_digest(&self.text_digest)
    }
}

impl JevNonAuthorizingContext {
    pub fn matches_action(&self, action: &ContentAction, source: SourceFormat) -> bool {
        self.complete
            && self.normalization_revision == 1
            && self.source_format == source
            && source == SourceFormat::CodexRolloutJsonl
            && valid_identity(&self.session_id)
            && valid_identity(&self.message_id)
            && self.source == action.reference
            && self.turn_scope == action.turn_scope
            && self.source.native_record_id.as_deref() == Some(self.message_id.as_str())
            && action_is_non_authorizing(action)
            && valid_range(&self.range, &self.message_id)
            && self.range.end == action.text.len()
            && action.metadata.bindings.contains(&self.range)
            && valid_digest(&self.text_digest)
            && self.text_digest == non_authorizing_context_digest(&action.text)
    }

    pub fn matches_session(
        &self,
        action: &ContentAction,
        source: SourceFormat,
        session_id: &str,
    ) -> bool {
        self.session_id == session_id && self.matches_action(action, source)
    }
}

/// Bind an adapter proof to the published action, source key, thread, and bytes.
pub fn normalize_non_authorizing_context(
    action: &ContentAction,
    source: SourceFormat,
) -> Option<JevNonAuthorizingContext> {
    let Some(proof) = action.metadata.non_authorizing_context_proof.as_ref() else {
        return action
            .metadata
            .non_authorizing_context
            .as_ref()
            .filter(|fact| fact.matches_action(action, source))
            .cloned();
    };
    if !proof.is_bounded()
        || proof.source_format != source
        || proof.message_id != action.reference.native_record_id.as_deref()?
        || proof.range.pointer != format!("/payload/content/{}/text", action.reference.part_index)
        || proof.range.end != action.text.len()
        || !action.metadata.bindings.contains(&proof.range)
        || proof.text_digest != non_authorizing_context_digest(&action.text)
        || !action_is_non_authorizing(action)
    {
        return None;
    }
    Some(JevNonAuthorizingContext {
        kind: proof.kind,
        source_format: source,
        session_id: proof.session_id.clone(),
        message_id: proof.message_id.clone(),
        source: action.reference.clone(),
        turn_scope: action.turn_scope.clone(),
        range: proof.range.clone(),
        text_digest: proof.text_digest.clone(),
        complete: true,
        normalization_revision: 1,
    })
}

fn action_is_non_authorizing(action: &ContentAction) -> bool {
    matches!(action.kind.as_str(), "user" | "user_text")
        && action.turn_role == "user"
        && action.turn_scope == "main"
        && action.authority == "unknown"
        && action.reference.stable
        && !action.reference.id.is_empty()
        && !action.reference.source_key_digest.is_empty()
        && !action.reference.thread_digest.is_empty()
        && !action.truncated
        && !action.text.is_empty()
        && action.metadata.user_text_history.is_none()
        && action.metadata.human_text.is_none()
        && action.metadata.selected_skill.is_none()
        && action.metadata.recorded_skill_result.is_none()
        && action.tool_name.is_none()
        && action.tool_call_id.is_none()
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_range(range: &JevNativeFieldRange, message_id: &str) -> bool {
    range.native_record_id.as_deref() == Some(message_id)
        && range.field == JevInputField::UserMessage
        && range.container == JevNativeFieldContainer::Record
        && !range.pointer.is_empty()
        && range.pointer.len() <= 256
        && range.start == 0
        && range.end > 0
        && range.end <= crate::analysis::MAX_CONTENT_PART_BYTES
}
