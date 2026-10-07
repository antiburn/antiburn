//! Select normalized human and command-result facts for an instruction episode.

use super::ContentAction;

pub(super) fn human_text(action: &ContentAction) -> bool {
    action
        .metadata
        .human_text
        .as_ref()
        .is_some_and(|fact| fact.matches_action(action))
}

pub(super) fn command_result(action: &ContentAction, actions: &[ContentAction]) -> bool {
    action.metadata.command_result.as_ref().is_some_and(|fact| {
        fact.matches_action(action) && actions.iter().any(|request| fact.matches_request(request))
    })
}

pub(super) fn supported(action: &ContentAction, actions: &[ContentAction]) -> bool {
    (action.authority == "assistant"
        && (super::action_context::is_assistant_text(&action.kind) || action.kind == "tool_input"))
        || human_text(action)
        || command_result(action, actions)
}

#[cfg(test)]
pub(crate) mod tests;
