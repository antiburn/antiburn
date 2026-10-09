//! Explicit context selection for development ablation through production preparation.

use super::{CandidateComparison, ContentAction, EvidenceIdentity, PrerequisiteEpisode};
use crate::analysis::jev::capabilities::ModelCapabilities;
use crate::analysis::jev::{JevError, JevSessionContext};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrerequisiteContextPolicy {
    SelectedEvents,
    #[default]
    CoherentEpisode,
}

impl PrerequisiteContextPolicy {
    pub(super) fn from_context(context: &JevSessionContext) -> Result<Self, JevError> {
        context
            .check_context
            .get("prerequisite_context_policy")
            .map(|value| {
                serde_json::from_value(value.clone()).map_err(|_| JevError::InvalidCheckContext)
            })
            .transpose()
            .map(|value| value.unwrap_or_default())
    }

    pub(super) fn select(
        self,
        comparison: &CandidateComparison,
        actions: &[ContentAction],
        capabilities: &ModelCapabilities,
        source_complete: bool,
    ) -> PrerequisiteEpisode {
        match self {
            Self::CoherentEpisode => {
                super::decisions::episode(comparison, actions, capabilities, source_complete)
            }
            Self::SelectedEvents => selected_events(comparison, actions, source_complete),
        }
    }
}

fn selected_events(
    comparison: &CandidateComparison,
    actions: &[ContentAction],
    source_complete: bool,
) -> PrerequisiteEpisode {
    let position = (
        comparison.source_turn_index,
        comparison
            .source_binding
            .as_ref()
            .map(|binding| binding.source.part_index)
            .unwrap_or(0),
    );
    let mut earlier = actions
        .iter()
        .filter(|action| {
            action.reference.thread_digest == comparison.source_thread_digest
                && action.turn_scope == comparison.source_turn_scope
                && (action.reference.turn_index, action.reference.part_index) < position
        })
        .collect::<Vec<_>>();
    earlier.sort_by_key(|action| (action.reference.turn_index, action.reference.part_index));
    let selected = comparison.context.iter().chain(&comparison.counterevidence);
    let events = earlier
        .iter()
        .filter_map(|action| {
            selected
                .clone()
                .find(|event| event.action_id == action.reference.id)
                .cloned()
        })
        .collect::<Vec<_>>();
    let identities = earlier
        .iter()
        .filter_map(|action| {
            events
                .iter()
                .find(|event| event.action_id == action.reference.id)
                .map(|event| EvidenceIdentity {
                    source: action.reference.clone(),
                    content_digest: super::content_action_digest(action),
                    start_byte: 0,
                    end_byte: event.text.len(),
                })
        })
        .collect::<Vec<_>>();
    let complete = source_complete
        && comparison.prior_history_complete
        && earlier.iter().all(|action| {
            action.reference.stable
                && action.authority == "assistant"
                && !action.truncated
                && comparison.source_binding.as_ref().is_some_and(|binding| {
                    binding.source.source_key_digest == action.reference.source_key_digest
                })
                && selected.clone().any(|event| {
                    event.action_id == action.reference.id
                        && !event.truncated
                        && event.text == action.text
                })
        });
    let revision = super::sha256_hex(
        &serde_json::to_vec(&(&events, &identities, complete, Vec::<ContentAction>::new()))
            .expect("serialize selected prerequisite context"),
    );
    PrerequisiteEpisode {
        events,
        identities,
        complete_selected_history: complete,
        selected_actions: Vec::new(),
        revision,
    }
}
