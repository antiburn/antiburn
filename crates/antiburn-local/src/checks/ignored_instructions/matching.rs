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
        let targets = window.comparisons;
        if targets
            .iter()
            .any(|comparison| !reference_matches(comparison, context))
        {
            continue;
        }
        if targets
            .iter()
            .any(|comparison| !super::reduction::authority_evidence_valid(comparison))
        {
            continue;
        }
        let id = sha256_hex(
            &serde_json::to_vec(&(
                &window.id,
                targets.iter().map(|pair| &pair.id).collect::<Vec<_>>(),
            ))
            .expect("serialize window target identities"),
        );
        let mut item = JevWorkItem {
            id,
            window: window_with_facts(&targets, context),
            questions: window_questions(&targets),
        };
        item.window.fields = direct_window_fields(&item.window.fields, false);
        output.push(item);
    }
    while !output.is_empty() && !fits_one_request(&output, capabilities) {
        if output.len() > 1 {
            output.pop();
        } else {
            output[0].window.fields = direct_window_fields(&output[0].window.fields, true);
            if !fits_one_request(&output, capabilities) {
                output.clear();
            }
            break;
        }
    }
    output
}

fn reference_matches(comparison: &CandidateComparison, context: &JevSessionContext) -> bool {
    let Some(action) = context.check_context["episode_actions"]
        .as_array()
        .and_then(|actions| {
            actions
                .iter()
                .find(|action| action["reference"]["id"] == comparison.reference.action_id)
        })
    else {
        return false;
    };
    let Ok(action) = serde_json::from_value::<super::super::ContentAction>(action.clone()) else {
        return false;
    };
    if super::super::content_action_digest(&action) != comparison.reference.action_digest {
        return false;
    }
    let authority_retained = context.check_context["episode_actions"]
        .as_array()
        .is_some_and(|actions| {
            actions
                .iter()
                .filter(|event| {
                    event["authority"] == "user"
                        && event["reference"]["thread_digest"] == comparison.source_thread_digest
                        && event["turn_scope"] == comparison.source_turn_scope
                        && event["reference"]["turn_index"]
                            .as_u64()
                            .zip(event["reference"]["part_index"].as_u64())
                            .is_some_and(|order| {
                                order
                                    < (
                                        action.reference.turn_index,
                                        u64::from(action.reference.part_index),
                                    )
                            })
                })
                .all(|event| {
                    let Ok(source) =
                        serde_json::from_value::<super::super::ContentAction>(event.clone())
                    else {
                        return false;
                    };
                    !super::super::selected_context::human_text(&source)
                        || comparison
                            .prerequisite_episode
                            .as_ref()
                            .is_some_and(|episode| {
                                episode.events.iter().any(|retained| {
                                    retained.action_id == source.reference.id
                                        && retained.text == source.text
                                        && !retained.truncated
                                })
                            })
                })
        });
    if !authority_retained {
        return false;
    }
    context
        .reference_snapshots
        .iter()
        .filter(|snapshot| snapshot.kind == "instruction_snapshot")
        .any(|snapshot| {
            let Ok(instructions) = serde_json::from_value::<Vec<super::super::InstructionSnapshot>>(
                snapshot.fields.clone(),
            ) else {
                return false;
            };
            instructions.iter().any(|instruction| {
                instruction.id == comparison.reference.instruction_id
                    && instruction.digest == comparison.reference.instruction_digest
                    && instruction.scope == comparison.reference.scope
                    && instruction.source == comparison.reference.source
                    && instruction.digest == sha256_hex(instruction.text.as_bytes())
                    && instruction.provenance == comparison.reference.provenance
                    && super::super::planning::instruction_scope_matches(instruction, &action)
                    && instruction.sections.iter().any(|rule| {
                        rule.id == comparison.reference.rule_id
                            && rule.text == comparison.rule_text
                            && rule.context == comparison.instruction_context
                    })
                    && comparison.instruction_context.iter().all(|range| {
                        instruction.text.get(range.start_byte..range.end_byte)
                            == Some(range.text.as_str())
                    })
            })
        })
}

fn fits_one_request(items: &[JevWorkItem], capabilities: &ModelCapabilities) -> bool {
    if items.iter().any(|item| {
        serde_json::to_vec(&item.window.fields)
            .expect("state")
            .len()
            > 1536
    }) {
        return false;
    }
    crate::analysis::jev::work_items_fit_one_request(
        items,
        capabilities,
        None,
        crate::analysis::jev::JevRequestLimits {
            request_bytes: 6144,
            estimated_input_tokens: 1536,
        },
    )
    .unwrap_or(false)
}

fn direct_window_fields(fields: &Value, reduced: bool) -> Value {
    let compact_event = |event: &Value| {
        json!({
            "role": event["role"], "kind": event["kind"], "text": event["text"],
            "tool_name": event["tool_name"],
            "recorded_order": event["recorded_order"], "truncated": event["truncated"],
            "native_context": event["native_context"],
        })
    };
    let targets = fields["instruction_targets"].as_array().expect("instruction targets").iter().map(|target| {
        let instruction = &target["instruction"];
        let text = instruction["text"].as_str().expect("instruction text");
        let rule = instruction["complete_text"].as_str().unwrap_or(text);
        let mut optional_bytes = if reduced { 0 } else { 192 };
        let mut optional_events = usize::from(!reduced);
        let events = target["earlier_counterevidence"].as_array().expect("earlier evidence").iter().filter(|event| {
            if !event["native_context"].is_null() { return true; }
            if !matches!(event["kind"].as_str(), Some("assistant" | "assistant_text")) { return false; }
            let bytes = event["text"].as_str().map_or(0, str::len);
            if optional_events == 0 || event["truncated"] == true || bytes > optional_bytes { return false; }
            optional_bytes -= bytes;
            optional_events -= 1;
            true
        }).map(&compact_event).collect::<Vec<_>>();
        json!({"instruction": {"text": rule, "basis": instruction.get("basis").unwrap_or(&instruction["provenance"]), "scope_directory": instruction["scope_directory"]},
            "earlier_counterevidence": events})
    }).collect::<Vec<_>>();
    let mut action = compact_event(&fields["candidate_action"]);
    let path_changes = fields
        .get("requested_path_changes")
        .unwrap_or(&fields["candidate_action"]["requested_path_changes"]);
    if path_changes
        .as_array()
        .is_some_and(|changes| !changes.is_empty())
    {
        action["requested_path_changes"] = path_changes.clone();
    }
    json!({"instruction_targets": targets, "candidate_action": action,
        "command_input_context": fields["command_input_context"]})
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
        comparison.prerequisite_episode = Some(select_comparison_episode(
            comparison,
            &actions,
            capabilities,
            plan.complete_input,
            policy,
        ));
    }
    Ok(())
}

pub(in crate::checks::ignored_instructions) fn select_comparison_episode(
    comparison: &CandidateComparison,
    actions: &[super::super::ContentAction],
    capabilities: &ModelCapabilities,
    complete: bool,
    policy: super::super::PrerequisiteContextPolicy,
) -> super::super::PrerequisiteEpisode {
    let mut selected = comparison.clone();
    selected.prerequisite_episode =
        Some(policy.select(comparison, actions, capabilities, complete));
    selected.prerequisite_episode.expect("selected episode")
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
