//! Request windows and provider-safe evidence views.

use super::*;

pub(super) struct AssessmentWindow<'a> {
    pub id: String,
    pub comparisons: Vec<&'a CandidateComparison>,
}

pub(super) fn assessment_windows(comparisons: &[CandidateComparison]) -> Vec<AssessmentWindow<'_>> {
    let mut groups = Vec::<(String, Vec<&CandidateComparison>)>::new();
    let mut group_indices = BTreeMap::<String, usize>::new();
    for comparison in comparisons {
        let key = format!(
            "{}:{}:{}",
            comparison.reference.action_id,
            comparison.action_text_start,
            comparison.action_text_end
        );
        let index = *group_indices.entry(key.clone()).or_insert_with(|| {
            groups.push((key, Vec::new()));
            groups.len() - 1
        });
        groups[index].1.push(comparison);
    }
    groups
        .into_iter()
        .flat_map(|(key, targets)| {
            targets
                .chunks(MAX_TARGETS_PER_WINDOW)
                .enumerate()
                .map(move |(chunk_index, comparisons)| AssessmentWindow {
                    id: sha256_hex(format!("{key}\0{chunk_index}").as_bytes()),
                    comparisons: {
                        let mut sorted = comparisons.to_vec();
                        sorted.sort_by(|left, right| left.id.cmp(&right.id));
                        sorted
                    },
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

pub(super) fn window_fields(comparisons: &[&CandidateComparison]) -> Value {
    let first = comparisons
        .first()
        .expect("assessment windows contain at least one target");
    let initial = initial_context(first);
    let targets = comparisons.iter().map(|comparison| json!({
        "comparison_id": &comparison.id,
        "instruction": {
            "section": &comparison.reference.rule_heading,
            "text": rule_text_fragment(comparison),
            "provenance": comparison.reference.provenance,
            "scope": comparison.reference.scope,
        },
        "earlier_counterevidence": ordered_request_counter_evidence(comparison.prerequisite_episode.as_ref().map(|episode| episode.events.as_slice()).unwrap_or(&comparison.counterevidence), comparison),
        "assessment_limits": {
            "candidate_action_truncated": comparison.action.truncated,
            "earlier_history_truncated": comparison.prerequisite_episode.as_ref().map(|episode| !episode.complete_selected_history).unwrap_or(comparison.earlier_history_truncated),
            "prior_history_complete": comparison.prior_history_complete,
            "prerequisite_episode_complete": comparison.prerequisite_episode.as_ref().is_some_and(|episode| episode.complete_selected_history),
        }
    })).collect::<Vec<_>>();
    json!({
        "instruction_targets": targets,
        "candidate_action": &initial["candidate_action"],
        "candidate_action_range_bytes": &initial["candidate_action_range_bytes"],
        "candidate_source_extent": first.source_binding.as_ref().and_then(|binding| binding.excerpt.as_ref()).map(|excerpt| json!({
            "selected_source_bytes":excerpt.source_bytes,"source_truncated":excerpt.source_truncated,
            "window_is_fragment":excerpt.start_byte != 0 || excerpt.end_byte != excerpt.source_bytes,
        })),
        "candidate_action_meaning": action_meaning(&first.action),
        "nearby_context": &initial["nearby_context"],
        "assessment_limits": {
            "context_is_same_branch": true,
            "nearby_context_truncated": first.context_truncated || first.context.iter().any(|event| event.truncated),
            "thinking_content_excluded": true,
        }
    })
}

pub(super) fn window_evidence(comparisons: &[&CandidateComparison]) -> Vec<JevEvidenceReference> {
    let mut evidence = Vec::new();
    if let Some(first) = comparisons.first() {
        evidence.push(JevEvidenceReference {
            part_id: "candidate_action".to_owned(),
            source_id: first.action.action_id.clone(),
            content_kind: first.action.kind.clone(),
            role: JevEvidenceRole::Candidate,
        });
        evidence.extend(first.context.iter().enumerate().map(|(index, event)| {
            JevEvidenceReference {
                part_id: format!("nearby_context[{index}]"),
                source_id: event.action_id.clone(),
                content_kind: event.kind.clone(),
                role: JevEvidenceRole::SupportingContext,
            }
        }));
    }
    for (target_index, comparison) in comparisons.iter().enumerate() {
        evidence.push(JevEvidenceReference {
            part_id: format!("instruction_targets[{target_index}].instruction"),
            source_id: format!(
                "{}:{}",
                comparison.reference.instruction_id, comparison.reference.rule_id
            ),
            content_kind: "instruction_rule".to_owned(),
            role: JevEvidenceRole::Instruction,
        });
        evidence.extend(
            comparison
                .prerequisite_episode
                .as_ref()
                .map(|episode| episode.events.as_slice())
                .unwrap_or(&comparison.counterevidence)
                .iter()
                .enumerate()
                .map(|(index, event)| JevEvidenceReference {
                    part_id: format!(
                        "instruction_targets[{target_index}].earlier_counterevidence[{index}]"
                    ),
                    source_id: event.action_id.clone(),
                    content_kind: event.kind.clone(),
                    role: JevEvidenceRole::SupportingContext,
                }),
        );
    }
    evidence
}

pub(super) fn initial_context(comparison: &CandidateComparison) -> Value {
    json!({
        "instruction": {
            "section": comparison.reference.rule_heading.as_str(),
            "text": rule_text_fragment(comparison),
            "provenance": comparison.reference.provenance,
            "scope": comparison.reference.scope,
        },
        "instruction_range_bytes": {"start": comparison.rule_text_start, "end": comparison.rule_text_end, "total": comparison.rule_text.len()},
        "candidate_action": request_counter_evidence(&comparison.action, comparison),
        "candidate_action_range_bytes": {"start": comparison.action_text_start, "end": comparison.action_text_end},
        "nearby_context": comparison.context.iter().map(|event| request_counter_evidence(event, comparison)).collect::<Vec<_>>(),
        "earlier_counterevidence": ordered_request_counter_evidence(&comparison.counterevidence, comparison),
        "assessment_limits": {
            "context_is_same_branch": true,
            "candidate_action_truncated": comparison.action.truncated,
            "nearby_context_truncated": comparison.context_truncated || comparison.context.iter().any(|event| event.truncated),
            "earlier_history_truncated": comparison.earlier_history_truncated,
            "prior_history_complete": comparison.prior_history_complete,
            "thinking_content_excluded": true,
        }
    })
}

fn request_counter_evidence(event: &CounterEvidence, comparison: &CandidateComparison) -> Value {
    use crate::analysis::jev::exact_facts::recorded_order;
    let order = comparison
        .prerequisite_episode
        .as_ref()
        .and_then(|episode| {
            episode
                .identities
                .iter()
                .find(|identity| identity.source.id == event.action_id)
        })
        .zip(comparison.source_binding.as_ref())
        .map(|(identity, binding)| {
            use crate::analysis::jev::exact_facts::RecordedOrder;
            match (identity.source.turn_index, identity.source.part_index)
                .cmp(&(binding.source.turn_index, binding.source.part_index))
            {
                std::cmp::Ordering::Less => RecordedOrder::Before,
                std::cmp::Ordering::Equal => RecordedOrder::Same,
                std::cmp::Ordering::Greater => RecordedOrder::After,
            }
        })
        .unwrap_or_else(|| {
            recorded_order(
                &comparison.source_thread_digest,
                &comparison.source_turn_scope,
                event.source_order,
                &comparison.source_thread_digest,
                &comparison.source_turn_scope,
                comparison.source_turn_index,
            )
        });
    json!({
        "role": &event.role, "kind": &event.kind, "timestamp_ms": event.timestamp_ms,
        "tool_name": &event.tool_name, "text": &event.text, "truncated": event.truncated,
        "recorded_order": order,
        "native_context": comparison.prerequisite_episode.as_ref().and_then(|episode| {
            let action = episode.selected_actions.iter().find(|action| action.reference.id == event.action_id)?;
            if super::super::selected_context::human_text(action) {
                Some(json!({"evidence_kind": "human_text"}))
            } else if super::super::selected_context::command_result(action, &episode.selected_actions) {
                let fact = action.metadata.command_result.as_ref()?;
                let request = episode.selected_actions.iter().find(|request| {
                    fact.matches_request(request)
                })?;
                Some(json!({"evidence_kind": "command_result", "operation_state": fact.state,
                    "matched_request": request.text}))
            } else {
                None
            }
        }),
    })
}

fn ordered_request_counter_evidence(
    events: &[CounterEvidence],
    comparison: &CandidateComparison,
) -> Vec<Value> {
    let mut ordered = events.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        let part_index = |event: &CounterEvidence| {
            comparison
                .prerequisite_episode
                .as_ref()
                .and_then(|episode| {
                    episode
                        .identities
                        .iter()
                        .find(|identity| identity.source.id == event.action_id)
                })
                .map(|identity| identity.source.part_index)
                .unwrap_or(0)
        };
        left.source_order
            .cmp(&right.source_order)
            .then_with(|| part_index(left).cmp(&part_index(right)))
            .then_with(|| left.action_id.cmp(&right.action_id))
    });
    ordered
        .into_iter()
        .map(|event| request_counter_evidence(event, comparison))
        .collect()
}
