//! Normalize human text and exact command-result bindings before check preparation.

use super::{ContentAction, ContentEventReference, JevNativeFieldRange, JevOperationState};
use crate::analysis::{SourceFormat, jev::JevInputField};
use crate::checks::ignored_instructions::sha256_hex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevHumanText {
    pub source: ContentEventReference,
    pub turn_scope: String,
    pub text_digest: String,
    pub range: JevNativeFieldRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevBoundCommandResult {
    pub source: ContentEventReference,
    pub turn_scope: String,
    pub text_digest: String,
    pub range: JevNativeFieldRange,
    pub request: ContentEventReference,
    pub request_range: JevNativeFieldRange,
    pub request_text_digest: String,
    pub call_id: String,
    pub tool_name: String,
    pub state: JevOperationState,
}

/// Validate decoded native bytes while the adapter still has the native string.
pub fn native_text_matches(
    range: &JevNativeFieldRange,
    native_text: &str,
    selected_text: &str,
) -> bool {
    !selected_text.is_empty() && native_text.get(range.start..range.end) == Some(selected_text)
}

impl JevHumanText {
    pub fn matches_action(&self, action: &ContentAction) -> bool {
        matches!(action.kind.as_str(), "user" | "user_text")
            && action.authority == "user"
            && action.turn_role == "user"
            && self.source == action.reference
            && self.turn_scope == action.turn_scope
            && !action.truncated
            && self.text_digest == text_digest(&action.text)
            && action.metadata.bindings.contains(&self.range)
    }
}

impl JevBoundCommandResult {
    pub fn matches_action(&self, action: &ContentAction) -> bool {
        action.kind == "tool_result"
            && action.authority == "tool"
            && self.source == action.reference
            && self.turn_scope == action.turn_scope
            && !action.truncated
            && self.text_digest == text_digest(&action.text)
            && action.metadata.bindings.contains(&self.range)
            && action.tool_call_id.as_deref() == Some(self.call_id.as_str())
            && action.tool_name.as_deref() == Some(self.tool_name.as_str())
            && action.metadata.state == self.state
            && matches!(
                self.state,
                JevOperationState::Completed | JevOperationState::Error
            )
    }

    pub fn matches_request(&self, request: &ContentAction) -> bool {
        request.kind == "tool_input"
            && request.authority == "assistant"
            && self.request == request.reference
            && self.turn_scope == request.turn_scope
            && !request.truncated
            && self.request_text_digest == text_digest(&request.text)
            && request.metadata.bindings.contains(&self.request_range)
            && request.tool_call_id.as_deref() == Some(self.call_id.as_str())
            && request.tool_name.as_deref() == Some(self.tool_name.as_str())
    }
}

/// Native adapters bind the selected decoded text before storage. Pointer spelling
/// is provenance; normalization does not infer a field from a pointer name.
pub fn normalize_context(actions: &mut [ContentAction], source_format: SourceFormat) {
    for action in actions.iter_mut() {
        action.metadata.human_text = normalize_human_text(action, source_format);
        action.metadata.command_result = None;
    }
    let results = actions
        .iter()
        .map(|action| normalize_command_result(action, actions))
        .collect::<Vec<_>>();
    for (action, result) in actions.iter_mut().zip(results) {
        action.metadata.command_result = result;
    }
}

fn normalize_human_text(
    action: &ContentAction,
    source_format: SourceFormat,
) -> Option<JevHumanText> {
    let proof = action.metadata.user_text_history.as_ref()?;
    if !matches!(action.kind.as_str(), "user" | "user_text")
        || action.authority != "user"
        || action.turn_role != "user"
        || proof.source_format != source_format
        || proof.session_id.is_empty()
        || proof.message_id.is_empty()
        || proof.revision == 0
    {
        return None;
    }
    Some(JevHumanText {
        source: action.reference.clone(),
        turn_scope: action.turn_scope.clone(),
        text_digest: text_digest(&action.text),
        range: selected_range(action, JevInputField::UserMessage, &action.text)?.clone(),
    })
}

fn normalize_command_result(
    action: &ContentAction,
    actions: &[ContentAction],
) -> Option<JevBoundCommandResult> {
    if action.kind != "tool_result"
        || action.authority != "tool"
        || !matches!(
            action.metadata.state,
            JevOperationState::Completed | JevOperationState::Error
        )
    {
        return None;
    }
    let call_id = action.tool_call_id.as_deref().filter(|id| !id.is_empty())?;
    let tool_name = action
        .tool_name
        .as_deref()
        .filter(|name| !name.is_empty())?;
    let same_call = |other: &&ContentAction| {
        other.reference.source_key_digest == action.reference.source_key_digest
            && other.reference.thread_digest == action.reference.thread_digest
            && other.turn_scope == action.turn_scope
            && other.tool_call_id.as_deref() == Some(call_id)
    };
    let mut requests = actions
        .iter()
        .filter(|request| request.kind == "tool_input")
        .filter(same_call);
    let request = requests.next()?;
    if requests.next().is_some()
        || request.authority != "assistant"
        || request.tool_name.as_deref() != Some(tool_name)
        || position(request) >= position(action)
        || actions
            .iter()
            .filter(|result| result.kind == "tool_result")
            .filter(same_call)
            .count()
            != 1
    {
        return None;
    }
    let fields = request.normalized_fields.as_ref()?;
    if fields.malformed
        || fields.category != Some(crate::analysis::jev::JevNormalizedCategory::BashCommand)
    {
        return None;
    }
    let selected = fields.values.get(&JevInputField::BashCommandInput)?;
    let command = match serde_json::from_str::<serde_json::Value>(selected) {
        Ok(serde_json::Value::Object(value)) => value.get("command")?.as_str()?.to_owned(),
        _ => selected.clone(),
    };
    let request_range = selected_range(request, JevInputField::BashCommandInput, &command)?.clone();
    Some(JevBoundCommandResult {
        source: action.reference.clone(),
        turn_scope: action.turn_scope.clone(),
        text_digest: text_digest(&action.text),
        range: selected_range(action, JevInputField::BashCommandOutput, &action.text)?.clone(),
        request: request.reference.clone(),
        request_range,
        request_text_digest: text_digest(selected),
        call_id: call_id.to_owned(),
        tool_name: tool_name.to_owned(),
        state: action.metadata.state,
    })
}

fn selected_range<'a>(
    action: &'a ContentAction,
    field: JevInputField,
    text: &str,
) -> Option<&'a JevNativeFieldRange> {
    if !action.reference.stable
        || action.truncated
        || text.is_empty()
        || action.reference.source_key_digest.is_empty()
        || action.reference.thread_digest.is_empty()
        || action
            .reference
            .native_record_id
            .as_ref()
            .is_none_or(String::is_empty)
    {
        return None;
    }
    action.metadata.bindings.iter().find(|range| {
        range.field == field
            && range.end.checked_sub(range.start) == Some(text.len())
            && !range.pointer.is_empty()
            && range.native_record_id == action.reference.native_record_id
    })
}

fn position(action: &ContentAction) -> (u64, u32) {
    (action.reference.turn_index, action.reference.part_index)
}

fn text_digest(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

#[cfg(test)]
mod tests;
