//! Source-bound decision records for deterministic finding explanations.

use serde::{Deserialize, Serialize};

use super::assessment::{AssessmentPlan, ObservableObligation};
use super::instructions::sha256_hex;
use super::{
    CandidateComparison, ContentAction, ContentEventReference, CounterEvidence, RuleActionRef,
};
use crate::analysis::jev::capabilities::ModelCapabilities;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionSourceBinding {
    pub source: ContentEventReference,
    pub authority: String,
    pub content_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<ActionExcerptBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionExcerptBinding {
    pub start_byte: usize,
    pub end_byte: usize,
    pub source_bytes: usize,
    pub source_truncated: bool,
    pub text_digest: String,
}

impl ActionSourceBinding {
    pub(super) fn matches(&self, comparison: &CandidateComparison) -> bool {
        self.authority == "assistant"
            && self.source.stable
            && comparison.reference.action_stable
            && !self.source.source_key_digest.is_empty()
            && !self.source.thread_digest.is_empty()
            && self.source.thread_digest == comparison.source_thread_digest
            && self.source.turn_index == comparison.source_turn_index
            && self.source.id == comparison.reference.action_id
            && self.source.id == comparison.action.action_id
            && !self.content_digest.is_empty()
            && self.content_digest == comparison.reference.action_digest
            && self.excerpt.as_ref().map_or_else(
                || {
                    comparison.action_text_start == 0
                        && comparison.action_text_end == comparison.action.text.len()
                },
                |excerpt| {
                    excerpt.start_byte == comparison.action_text_start
                        && excerpt.end_byte == comparison.action_text_end
                        && excerpt.start_byte < excerpt.end_byte
                        && excerpt.end_byte <= excerpt.source_bytes
                        && excerpt.end_byte - excerpt.start_byte == comparison.action.text.len()
                        && excerpt.text_digest == sha256_hex(comparison.action.text.as_bytes())
                },
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceIdentity {
    pub source: ContentEventReference,
    pub content_digest: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrerequisiteEpisode {
    pub events: Vec<CounterEvidence>,
    pub identities: Vec<EvidenceIdentity>,
    pub complete_selected_history: bool,
    #[serde(default)]
    pub selected_actions: Vec<ContentAction>,
    pub revision: String,
}

impl PrerequisiteEpisode {
    pub(super) fn authorization_available(&self) -> bool {
        self.selected_actions
            .iter()
            .any(super::selected_context::human_text)
    }

    pub(super) fn results_available(&self) -> bool {
        self.selected_actions
            .iter()
            .any(|action| super::selected_context::command_result(action, &self.selected_actions))
    }

    pub(super) fn has_source_bindings(&self, anchor: &ActionSourceBinding) -> bool {
        self.events
            .iter()
            .map(|event| &event.action_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == self.events.len()
            && self.events.iter().all(|event| {
                self.identities
                    .iter()
                    .any(|identity| identity.source.id == event.action_id)
            })
            && (self.selected_actions.is_empty()
                || (self.selected_actions.len() == self.events.len()
                    && self
                        .selected_actions
                        .iter()
                        .zip(&self.events)
                        .all(|(action, event)| {
                            let identities = self
                                .identities
                                .iter()
                                .filter(|identity| identity.source.id == event.action_id)
                                .collect::<Vec<_>>();
                            let ranges = identities
                                .iter()
                                .map(|identity| (identity.start_byte, identity.end_byte))
                                .collect::<Vec<_>>();
                            !identities.is_empty()
                                && identities.iter().all(|identity| {
                                    action.reference == identity.source
                                        && super::content_action_digest(action)
                                            == identity.content_digest
                                })
                                && selected_counter_event(action, &ranges).as_ref() == Some(event)
                                && super::selected_context::supported(
                                    action,
                                    &self.selected_actions,
                                )
                        })))
            && self.revision
                == sha256_hex(
                    &serde_json::to_vec(&(
                        &self.events,
                        &self.identities,
                        self.complete_selected_history,
                        &self.selected_actions,
                    ))
                    .expect("serialize prerequisite episode"),
                )
            && self.identities.iter().all(|identity| {
                let Some(event) = self
                    .events
                    .iter()
                    .find(|event| event.action_id == identity.source.id)
                else {
                    return false;
                };
                event.action_id == identity.source.id
                    && event.source_order == identity.source.turn_index
                    && ((event.role == "assistant"
                        && (super::action_context::is_assistant_text(&event.kind)
                            || event.kind == "tool_input"))
                        || !self.selected_actions.is_empty())
                    && identity.source.stable
                    && !identity.source.id.is_empty()
                    && !identity.content_digest.is_empty()
                    && identity.source.source_key_digest == anchor.source.source_key_digest
                    && identity.source.thread_digest == anchor.source.thread_digest
                    && (identity.source.turn_index, identity.source.part_index)
                        < (anchor.source.turn_index, anchor.source.part_index)
                    && identity.start_byte < identity.end_byte
                    && (!self.selected_actions.is_empty()
                        || (identity.start_byte == 0 && identity.end_byte == event.text.len()))
                    && (!self.complete_selected_history || !event.truncated)
            })
            && self
                .identities
                .windows(2)
                .all(|pair| evidence_is_ordered(&pair[0], &pair[1]))
            && self
                .identities
                .iter()
                .map(|identity| (&identity.source.id, identity.start_byte, identity.end_byte))
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.identities.len()
    }
}

fn selected_counter_event(
    action: &ContentAction,
    ranges: &[(usize, usize)],
) -> Option<CounterEvidence> {
    let parts = ranges
        .iter()
        .map(|(start, end)| action.text.get(*start..*end))
        .collect::<Option<Vec<_>>>()?;
    let mut event = super::planning::counter_event(action, action.text.len());
    event.text = parts.join("\n");
    event.truncated |= ranges != [(0, action.text.len())];
    Some(event)
}

fn evidence_source_ids(identities: &[EvidenceIdentity]) -> Vec<String> {
    let mut ids = identities
        .iter()
        .map(|identity| identity.source.id.clone())
        .collect::<Vec<_>>();
    ids.dedup();
    ids
}

fn evidence_is_ordered(left: &EvidenceIdentity, right: &EvidenceIdentity) -> bool {
    if left.source.id == right.source.id {
        left.source == right.source
            && left.content_digest == right.content_digest
            && left.end_byte <= right.start_byte
    } else {
        (left.source.turn_index, left.source.part_index)
            < (right.source.turn_index, right.source.part_index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrerequisiteOutcome {
    NotRequired,
    EarlierRequestAbsent,
    SelectedHistoryConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CitationClaim {
    RuleRequirement,
    AnchoredAction,
    PrerequisiteContrast,
    ObservedContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationProof {
    pub claim: CitationClaim,
    pub source_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionCoverage {
    pub source_complete: bool,
    pub selected_history_complete: bool,
    pub read_request_inventory_complete: bool,
    pub results_excluded: bool,
    pub user_authority_excluded: bool,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub schema_revision: u32,
    pub source_generation: i64,
    pub source_fingerprint: Option<String>,
    pub publication_fence: i64,
    pub rule_action: RuleActionRef,
    pub rule_start_byte: usize,
    pub rule_end_byte: usize,
    pub action_anchor: EvidenceIdentity,
    pub action_authority: String,
    pub action_is_request: bool,
    pub prerequisite: PrerequisiteOutcome,
    pub selected_evidence: Vec<EvidenceIdentity>,
    pub coverage: DecisionCoverage,
    pub citations: Vec<CitationProof>,
    pub context_revision: String,
    pub evaluator_revision: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation_basis: Option<InstructionExplanationBasis>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionExplanationBasis {
    pub schema_revision: u32,
    pub relationship: InstructionMismatch,
    pub instruction: String,
    pub action: String,
    pub instruction_context: Vec<super::instructions::InstructionContextRange>,
    pub earlier_events: Vec<CounterEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionMismatch {
    RequirementConflict,
}

impl DecisionRecord {
    /// Validate source bindings before a renderer uses the decision template.
    pub fn has_citation_proof(&self) -> bool {
        let rule_id = format!(
            "{}:{}",
            self.rule_action.instruction_id, self.rule_action.rule_id
        );
        self.schema_revision == 1
            && self.action_authority == "assistant"
            && !self.context_revision.is_empty()
            && !self.evaluator_revision.is_empty()
            && !self.rule_action.instruction_digest.is_empty()
            && !self.action_anchor.content_digest.is_empty()
            && self.action_anchor.source.id == self.rule_action.action_id
            && self.action_anchor.source.stable
            && !self.action_anchor.source.id.is_empty()
            && !self.action_anchor.source.source_key_digest.is_empty()
            && !self.action_anchor.source.thread_digest.is_empty()
            && self.action_anchor.content_digest == self.rule_action.action_digest
            && self.action_anchor.start_byte < self.action_anchor.end_byte
            && self.rule_start_byte < self.rule_end_byte
            && self.selected_evidence.iter().all(|evidence| {
                evidence.source.stable
                    && !evidence.source.id.is_empty()
                    && !evidence.source.thread_digest.is_empty()
                    && !evidence.content_digest.is_empty()
                    && evidence.source.source_key_digest
                        == self.action_anchor.source.source_key_digest
                    && evidence.source.thread_digest == self.action_anchor.source.thread_digest
                    && (evidence.source.turn_index, evidence.source.part_index)
                        < (
                            self.action_anchor.source.turn_index,
                            self.action_anchor.source.part_index,
                        )
                    && evidence.start_byte < evidence.end_byte
            })
            && self
                .selected_evidence
                .windows(2)
                .all(|pair| evidence_is_ordered(&pair[0], &pair[1]))
            && self
                .selected_evidence
                .iter()
                .map(|evidence| (&evidence.source.id, evidence.start_byte, evidence.end_byte))
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.selected_evidence.len()
            && [
                CitationClaim::RuleRequirement,
                CitationClaim::AnchoredAction,
            ]
            .into_iter()
            .all(|claim| {
                self.citations.iter().any(|proof| {
                    proof.claim == claim
                        && proof.source_ids
                            == [if claim == CitationClaim::RuleRequirement {
                                rule_id.clone()
                            } else {
                                self.rule_action.action_id.clone()
                            }]
                })
            })
            && (self.prerequisite != PrerequisiteOutcome::NotRequired
                || self.selected_evidence.is_empty()
                || self.citations.iter().any(|proof| {
                    proof.claim == CitationClaim::ObservedContext
                        && proof.source_ids == evidence_source_ids(&self.selected_evidence)
                }))
            && (self.prerequisite == PrerequisiteOutcome::NotRequired
                || self.citations.iter().any(|proof| {
                    proof.claim == CitationClaim::PrerequisiteContrast
                        && self.coverage.source_complete
                        && (self.coverage.selected_history_complete
                            || (self.prerequisite == PrerequisiteOutcome::EarlierRequestAbsent
                                && self.coverage.read_request_inventory_complete))
                        && proof.source_ids
                            == std::iter::once(self.rule_action.action_id.clone())
                                .chain(evidence_source_ids(&self.selected_evidence))
                                .collect::<Vec<_>>()
                }))
    }

    pub fn contrast_template(&self) -> Option<&'static str> {
        self.has_citation_proof().then_some(match (self.prerequisite, self.action_is_request) {
            (PrerequisiteOutcome::EarlierRequestAbsent, _) =>
                "No required read request precedes this action in the complete recorded request history.",
            (PrerequisiteOutcome::SelectedHistoryConflict, _) =>
                "The selected earlier events and this action conflict with the prerequisite rule.",
            (_, true) => "The recorded tool request conflicts with this instruction.",
            (_, false) => "The recorded assistant text conflicts with this instruction.",
        })
    }
}

pub(super) fn episode(
    comparison: &CandidateComparison,
    actions: &[ContentAction],
    capabilities: &ModelCapabilities,
    source_complete: bool,
) -> PrerequisiteEpisode {
    episode_with_witnesses(comparison, actions, capabilities, source_complete, &[])
}

pub(super) fn episode_with_witnesses(
    comparison: &CandidateComparison,
    actions: &[ContentAction],
    capabilities: &ModelCapabilities,
    source_complete: bool,
    witnesses: &[String],
) -> PrerequisiteEpisode {
    let available = [
        capabilities.usable_state_tokens(),
        capabilities.request_body_bytes.value,
        capabilities.state_and_longest_question_bytes.value,
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(16 * 1024);
    let budget = if available <= 8192 {
        available.saturating_sub(640).min(2048) as usize
    } else {
        available.saturating_sub(4096).saturating_div(4).min(4096) as usize
    };
    let mut earlier = actions
        .iter()
        .filter(|action| {
            action.reference.thread_digest == comparison.source_thread_digest
                && action.turn_scope == comparison.source_turn_scope
                && (action.reference.turn_index, action.reference.part_index)
                    < (
                        comparison.source_turn_index,
                        comparison
                            .source_binding
                            .as_ref()
                            .map(|binding| binding.source.part_index)
                            .unwrap_or(0),
                    )
        })
        .collect::<Vec<_>>();
    earlier.sort_by(|left, right| {
        left.reference
            .turn_index
            .cmp(&right.reference.turn_index)
            .then_with(|| left.reference.part_index.cmp(&right.reference.part_index))
            .then_with(|| left.reference.id.cmp(&right.reference.id))
    });
    let earlier_count = earlier.len();
    earlier.retain(|action| super::selected_context::supported(action, actions));
    earlier.sort_by_key(|action| {
        (
            !witnesses.contains(&action.reference.id),
            std::cmp::Reverse((action.reference.turn_index, action.reference.part_index)),
        )
    });
    let mut complete = source_complete && comparison.prior_history_complete;
    let mut bytes = 0usize;
    let mut selected = Vec::new();
    let mut selected_ranges = std::collections::BTreeMap::new();
    for action in earlier {
        let available = budget.saturating_sub(bytes).saturating_sub(256);
        let ranges = if super::planning::atomic_command(action) {
            if action.text.len() <= available {
                vec![(0, action.text.len())]
            } else {
                Vec::new()
            }
        } else {
            super::selected_context::supporting_text_ranges(
                &action.text,
                available,
                super::planning::rule_text_fragment(comparison),
            )
        };
        let selected_bytes = ranges.iter().map(|(start, end)| end - start).sum::<usize>()
            + ranges.len().saturating_sub(1);
        let next_bytes = bytes.saturating_add(selected_bytes).saturating_add(256);
        if selected.len() == 64 || next_bytes > budget || ranges.is_empty() {
            complete = false;
            continue;
        }
        bytes = next_bytes;
        complete &= ranges == [(0, action.text.len())]
            && !action.truncated
            && action.reference.stable
            && super::selected_context::supported(action, actions)
            && comparison.source_binding.as_ref().is_some_and(|binding| {
                binding.source.source_key_digest == action.reference.source_key_digest
            });
        selected_ranges.insert(action.reference.id.clone(), ranges);
        selected.push(action);
    }
    complete &= selected.len() == earlier_count;
    selected.sort_by_key(|action| (action.reference.turn_index, action.reference.part_index));
    // Keep a result only when its exact request also fits in this episode.
    let selected_actions = selected
        .iter()
        .map(|action| (*action).clone())
        .collect::<Vec<_>>();
    selected.retain(|action| {
        action.kind != "tool_result"
            || super::selected_context::command_result(action, &selected_actions)
    });
    complete &= selected.len() == earlier_count;
    let selected_actions = selected
        .iter()
        .map(|action| (*action).clone())
        .collect::<Vec<_>>();
    let events = selected
        .iter()
        .map(|action| {
            selected_counter_event(action, &selected_ranges[&action.reference.id])
                .expect("selected ranges bind to the retained action text")
        })
        .collect();
    let identities = selected
        .iter()
        .flat_map(|action| {
            selected_ranges[&action.reference.id]
                .iter()
                .map(|(start, end)| EvidenceIdentity {
                    source: action.reference.clone(),
                    content_digest: super::content_action_digest(action),
                    start_byte: *start,
                    end_byte: *end,
                })
        })
        .collect();
    let revision = sha256_hex(
        &serde_json::to_vec(&(&events, &identities, complete, &selected_actions))
            .expect("serialize prerequisite episode"),
    );
    PrerequisiteEpisode {
        events,
        identities,
        complete_selected_history: complete,
        selected_actions,
        revision,
    }
}

pub(super) fn record(
    plan: &AssessmentPlan,
    comparison: &CandidateComparison,
    obligation: Option<&ObservableObligation>,
    limitations: Vec<String>,
) -> Option<DecisionRecord> {
    let binding = comparison.source_binding.as_ref()?;
    if binding.authority != "assistant" {
        return None;
    }
    comparison
        .rule_text
        .get(comparison.rule_text_start..comparison.rule_text_end)?;
    if !binding.matches(comparison) {
        return None;
    }
    let required = obligation.is_some_and(|value| value.prerequisite_required);
    let episode = comparison.prerequisite_episode.as_ref();
    if episode.is_some_and(|value| !value.has_source_bindings(binding)) {
        return None;
    }
    let prerequisite = if !required || !episode.is_some_and(|value| value.complete_selected_history)
    {
        PrerequisiteOutcome::NotRequired
    } else if obligation.is_some_and(|value| {
        value.read_prerequisite_absent
            || value.read_request_order.as_ref().is_some_and(|order| {
                order.state() == crate::analysis::jev::obligations::ObligationState::Violated
            })
    }) {
        PrerequisiteOutcome::EarlierRequestAbsent
    } else {
        PrerequisiteOutcome::SelectedHistoryConflict
    };
    let selected_evidence = episode
        .map(|value| value.identities.clone())
        .unwrap_or_default();
    let mut citations = vec![
        CitationProof {
            claim: CitationClaim::RuleRequirement,
            source_ids: vec![format!(
                "{}:{}",
                comparison.reference.instruction_id, comparison.reference.rule_id
            )],
        },
        CitationProof {
            claim: CitationClaim::AnchoredAction,
            source_ids: vec![comparison.reference.action_id.clone()],
        },
    ];
    if required {
        citations.push(CitationProof {
            claim: CitationClaim::PrerequisiteContrast,
            source_ids: std::iter::once(comparison.reference.action_id.clone())
                .chain(evidence_source_ids(&selected_evidence))
                .collect(),
        });
    } else if !selected_evidence.is_empty() {
        citations.push(CitationProof {
            claim: CitationClaim::ObservedContext,
            source_ids: evidence_source_ids(&selected_evidence),
        });
    }
    let record = DecisionRecord {
        schema_revision: 1,
        source_generation: plan.source_generation,
        source_fingerprint: plan.source_fingerprint.clone(),
        publication_fence: plan.publication_fence,
        rule_action: comparison.reference.clone(),
        rule_start_byte: comparison.rule_text_start,
        rule_end_byte: comparison.rule_text_end,
        action_anchor: EvidenceIdentity {
            source: binding.source.clone(),
            content_digest: binding.content_digest.clone(),
            start_byte: comparison.action_text_start,
            end_byte: comparison.action_text_end,
        },
        action_is_request: comparison.action.kind == "tool_input",
        action_authority: binding.authority.clone(),
        prerequisite,
        selected_evidence,
        coverage: DecisionCoverage {
            source_complete: plan.complete_input,
            selected_history_complete: episode.is_some_and(|value| value.complete_selected_history),
            read_request_inventory_complete: obligation.is_some_and(|value| {
                value.read_prerequisite_absent
                    || value
                        .read_request_order
                        .as_ref()
                        .is_some_and(|order| order.history_complete && order.paths_known)
            }),
            results_excluded: !episode.is_some_and(PrerequisiteEpisode::results_available),
            user_authority_excluded: !episode
                .is_some_and(PrerequisiteEpisode::authorization_available),
            limitations,
        },
        citations,
        context_revision: sha256_hex(
            &serde_json::to_vec(comparison).expect("serialize comparison context"),
        ),
        evaluator_revision: super::evaluator_revision(),
        model: plan.model_version.clone(),
        explanation_basis: Some(InstructionExplanationBasis {
            schema_revision: 1,
            relationship: InstructionMismatch::RequirementConflict,
            instruction: super::planning::rule_text_fragment(comparison).to_owned(),
            action: comparison.action.text.clone(),
            instruction_context: comparison.instruction_context.clone(),
            earlier_events: episode
                .map(|episode| episode.events.clone())
                .unwrap_or_default(),
        }),
    };
    record.has_citation_proof().then_some(record)
}
