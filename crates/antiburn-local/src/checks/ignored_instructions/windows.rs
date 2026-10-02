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
        "earlier_counterevidence": ordered_request_counter_evidence(&comparison.counterevidence, comparison),
        "assessment_limits": {
            "candidate_action_truncated": comparison.action.truncated,
            "earlier_history_truncated": comparison.earlier_history_truncated,
            "prior_history_complete": comparison.prior_history_complete,
        }
    })).collect::<Vec<_>>();
    json!({
        "instruction_targets": targets,
        "candidate_action": &initial["candidate_action"],
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
                .counterevidence
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
    json!({
        "role": &event.role, "kind": &event.kind, "timestamp_ms": event.timestamp_ms,
        "tool_name": &event.tool_name, "text": &event.text, "truncated": event.truncated,
        "recorded_order": recorded_order(&comparison.source_thread_digest, &comparison.source_turn_scope, event.source_order,
            &comparison.source_thread_digest, &comparison.source_turn_scope, comparison.source_turn_index),
    })
}

fn ordered_request_counter_evidence(
    events: &[CounterEvidence],
    comparison: &CandidateComparison,
) -> Vec<Value> {
    let mut ordered = events.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        left.source_order
            .cmp(&right.source_order)
            .then_with(|| left.action_id.cmp(&right.action_id))
    });
    ordered
        .into_iter()
        .map(|event| request_counter_evidence(event, comparison))
        .collect()
}
