//! Cheap request facts and classification-independent context selection.

use super::*;
use crate::analysis::jev::capabilities::ModelCapabilities;
use crate::analysis::jev::obligations::ReadRequestOrder;

#[cfg(test)]
#[path = "matching/tests.rs"]
mod tests;

pub(super) fn assessment_work_items(
    comparisons: &[CandidateComparison],
    context: &JevSessionContext,
    capabilities: &ModelCapabilities,
) -> Vec<JevWorkItem> {
    let mut output = Vec::new();
    for window in assessment_windows(comparisons) {
        let mut pending = vec![window.comparisons];
        while let Some(targets) = pending.pop() {
            let id = sha256_hex(
                &serde_json::to_vec(&(
                    &window.id,
                    targets.iter().map(|pair| &pair.id).collect::<Vec<_>>(),
                ))
                .expect("serialize window target identities"),
            );
            let item = JevWorkItem {
                id,
                window: window_with_facts(&targets, context),
                questions: window_questions(&targets),
            };
            let packed = crate::analysis::jev::pack_work_items_with_capabilities(
                std::slice::from_ref(&item),
                capabilities,
            );
            if targets.len() > 1 && !packed.skipped_item_ids.is_empty() {
                let middle = targets.len() / 2;
                pending.push(targets[middle..].to_vec());
                pending.push(targets[..middle].to_vec());
            } else {
                output.push(item);
            }
        }
    }
    output
}

pub(super) fn window_with_facts(
    comparisons: &[&CandidateComparison],
    context: &JevSessionContext,
) -> JevInputWindow {
    let mut fields = window_fields(comparisons);
    let mut evidence = window_evidence(comparisons);
    let first = comparisons[0];
    let facts = crate::analysis::jev::exact_facts::ExactActionFacts::from_store(
        &context.evidence_store,
        &first.action.action_id,
        first.action.tool_name.as_deref(),
    );
    fields["requested_path_changes"] = json!(super::super::action_context::path_changes(
        &facts.edit_operations
    ));
    for (index, comparison) in comparisons.iter().enumerate() {
        fields["instruction_targets"][index]["identifier_facts"] = json!(
            crate::analysis::jev::exact_facts::reference_identifier_facts(
                rule_text_fragment(comparison),
                &facts
            )
        );
    }
    if let Some(command) = facts.command.as_deref()
        && let Some(source_text) = context
            .evidence_store
            .get(&first.action.action_id, JevInputField::BashCommandInput)
        && let Some(header) =
            super::super::action_context::here_document_context(command, source_text, first)
    {
        fields["command_input_context"] = json!(header);
        evidence.push(JevEvidenceReference {
            part_id: "command_input_context".to_owned(),
            source_id: first.action.action_id.clone(),
            content_kind: first.action.kind.clone(),
            role: JevEvidenceRole::SupportingContext,
        });
    }
    JevInputWindow { fields, evidence }
}

pub(super) fn select_context(
    plan: &mut AssessmentPlan,
    context: &JevSessionContext,
    capabilities: &ModelCapabilities,
) -> Result<(), JevError> {
    let actions: Vec<super::super::ContentAction> =
        serde_json::from_value(context.check_context["episode_actions"].clone())
            .map_err(|_| JevError::InvalidCheckContext)?;
    let policy = super::super::PrerequisiteContextPolicy::from_context(context)?;
    for comparison in &mut plan.comparisons {
        comparison.prerequisite_episode =
            Some(policy.select(comparison, &actions, capabilities, plan.complete_input));
    }
    Ok(())
}

pub(super) fn earlier_read_only_actions(
    comparisons: &[CandidateComparison],
    content: &SessionContentEvidence,
) -> BTreeMap<String, usize> {
    let branches = branch_order_index(&content.actions);
    let actions_by_id = content
        .actions
        .iter()
        .map(|action| (action.reference.id.as_str(), action))
        .collect::<BTreeMap<_, _>>();
    comparisons
        .iter()
        .filter_map(|comparison| {
            let candidate = actions_by_id.get(comparison.action.action_id.as_str())?;
            let earlier = branches
                .actions_by_branch
                .get(&(
                    comparison.source_thread_digest.clone(),
                    comparison.source_turn_scope.clone(),
                ))?
                .iter()
                .filter(|event| {
                    (event.reference.turn_index, event.reference.part_index)
                        < (
                            candidate.reference.turn_index,
                            candidate.reference.part_index,
                        )
                })
                .collect::<Vec<_>>();
            let only_reads = earlier.iter().all(|event| {
                let normalized;
                let fields = match event.normalized_fields.as_ref() {
                    Some(fields) => fields,
                    None => {
                        normalized = crate::analysis::jev_evidence::normalize_tool_input(
                            event.tool_name.as_deref().unwrap_or(""),
                            &event.text,
                        );
                        &normalized
                    }
                };
                event.kind == "tool_input"
                    && event.reference.source_key_digest == candidate.reference.source_key_digest
                    && !event.truncated
                    && event.reference.stable
                    && event.tool_call_id.is_some()
                    && !fields.malformed
                    && fields.values.contains_key(&JevInputField::ReadFilePath)
            });
            only_reads.then(|| (comparison.id.clone(), earlier.len()))
        })
        .collect()
}

pub(super) fn exact_read_orders(
    comparisons: &[CandidateComparison],
    content: &SessionContentEvidence,
    prior_history_complete: bool,
) -> BTreeMap<String, Vec<ReadRequestOrder>> {
    let branches = branch_order_index(&content.actions);
    let actions_by_id = content
        .actions
        .iter()
        .map(|action| (action.reference.id.as_str(), action))
        .collect::<BTreeMap<_, _>>();
    comparisons
        .iter()
        .map(|comparison| {
            let candidate = actions_by_id.get(comparison.action.action_id.as_str());
            let candidate_position =
                candidate.map(|event| (event.reference.turn_index, event.reference.part_index));
            let candidate_source =
                candidate.map(|event| event.reference.source_key_digest.as_str());
            let candidate_complete =
                candidate.is_some_and(|event| !event.truncated && event.reference.stable);
            let branch = branches.actions_by_branch.get(&(
                comparison.source_thread_digest.clone(),
                comparison.source_turn_scope.clone(),
            ));
            let orders = super::super::action_context::rule_path_candidates(&comparison.rule_text)
                .into_iter()
                .map(|required_path| {
                    let mut order = ReadRequestOrder {
                        required_path,
                        earlier_request_id: None,
                        later_request_id: None,
                        history_complete: prior_history_complete && content.complete,
                        paths_known: candidate_complete,
                    };
                    let Some(candidate_position) = candidate_position else {
                        order.paths_known = false;
                        return order;
                    };
                    let scope = crate::analysis::jev::obligations::RequestPathScope::File(
                        order.required_path.clone(),
                    );
                    if scope.matches(&order.required_path).is_none() {
                        order.paths_known = false;
                    }
                    if !order.paths_known {
                        return order;
                    }
                    for event in branch.into_iter().flatten() {
                        let position = (event.reference.turn_index, event.reference.part_index);
                        if Some(event.reference.source_key_digest.as_str()) != candidate_source {
                            if position < candidate_position {
                                order.paths_known = false;
                            }
                            continue;
                        }
                        if event.kind != "tool_input" {
                            continue;
                        }
                        let normalized;
                        let fields = match event.normalized_fields.as_ref() {
                            Some(fields) => fields,
                            None => {
                                normalized = crate::analysis::jev_evidence::normalize_tool_input(
                                    event.tool_name.as_deref().unwrap_or(""),
                                    &event.text,
                                );
                                &normalized
                            }
                        };
                        let is_read = fields.values.contains_key(&JevInputField::ReadFilePath);
                        if position < candidate_position
                            && (fields.values.contains_key(&JevInputField::BashCommandInput)
                                || fields.values.contains_key(&JevInputField::OtherToolInput))
                        {
                            order.paths_known = false;
                        }
                        let facts =
                            crate::analysis::jev::exact_facts::ExactActionFacts::from_selected(
                                event.tool_name.as_deref(),
                                Some(fields),
                            );
                        let before = position < candidate_position;
                        if before
                            && is_read
                            && (facts.malformed
                                || facts.paths.is_empty()
                                || !event.reference.stable
                                || event.tool_call_id.is_none()
                                || event.truncated
                                || facts.paths.iter().any(|path| scope.matches(path).is_none()))
                        {
                            order.paths_known = false;
                        }
                        if !is_read
                            || facts.malformed
                            || event.truncated
                            || !event.reference.stable
                            || event.tool_call_id.is_none()
                            || !facts.paths.contains(&order.required_path)
                        {
                            continue;
                        }
                        if before {
                            order.earlier_request_id = Some(event.reference.id.clone());
                        } else if position > candidate_position {
                            order.later_request_id = Some(event.reference.id.clone());
                        } else if event.reference.id != comparison.action.action_id {
                            order.paths_known = false;
                        }
                    }
                    order
                })
                .collect();
            (comparison.id.clone(), orders)
        })
        .collect()
}
