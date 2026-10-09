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
        "instruction": instruction_fields(comparison),
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
        evidence.extend(comparison.instruction_context.iter().enumerate().map(|(index, _)| JevEvidenceReference {
            part_id: format!("instruction_targets[{target_index}].instruction.surrounding_context[{index}]"),
            source_id: comparison.reference.instruction_id.clone(),
            content_kind: "instruction_section_context".to_owned(),
            role: JevEvidenceRole::SupportingContext,
        }));
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
        "instruction": instruction_fields(comparison),
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

fn instruction_fields(comparison: &CandidateComparison) -> Value {
    json!({
        "section": comparison.reference.rule_heading,
        "text": rule_text_fragment(comparison),
        "provenance": comparison.reference.provenance,
        "scope": comparison.reference.scope,
        "target_source_lines": {"start": comparison.reference.start_line, "end": comparison.reference.end_line},
        "target_range_bytes": {"start": comparison.rule_text_start, "end": comparison.rule_text_end, "total": comparison.rule_text.len()},
        "surrounding_context": comparison.instruction_context,
        "context_limits": {
            "target_is_fragment": comparison.rule_text_start != 0 || comparison.rule_text_end != comparison.rule_text.len(),
            "surrounding_context_unavailable": comparison.instruction_context.is_empty(),
            "surrounding_context_clipped": comparison.instruction_context.iter().any(|range| range.context_clipped || !range.enclosing_section_complete),
            "context_is_source_text": true,
        },
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
    let source = comparison
        .prerequisite_episode
        .as_ref()
        .and_then(|episode| {
            episode
                .selected_actions
                .iter()
                .find(|action| action.reference.id == event.action_id)
        });
    let supporting = event.action_id != comparison.action.action_id;
    let retained_event = comparison
        .prerequisite_episode
        .as_ref()
        .and_then(|episode| {
            episode
                .events
                .iter()
                .find(|selected| selected.action_id == event.action_id)
        });
    let ranges = if supporting && retained_event.is_some() {
        comparison
            .prerequisite_episode
            .as_ref()
            .expect("the retained event has an episode")
            .identities
            .iter()
            .filter(|identity| identity.source.id == event.action_id)
            .map(|identity| (identity.start_byte, identity.end_byte))
            .collect::<Vec<_>>()
    } else {
        vec![(0, event.text.len())]
    };
    let selected_event = retained_event.unwrap_or(event);
    let mut selected_offset = 0;
    let selected_ranges = ranges.iter().map(|(start, end)| {
        let selected_start = selected_offset;
        selected_offset += end - start;
        let range = json!({"start": start, "end": end, "selected_text_start": selected_start, "selected_text_end": selected_offset});
        selected_offset += 1;
        range
    }).collect::<Vec<_>>();
    json!({
        "role": &event.role, "kind": &event.kind, "timestamp_ms": event.timestamp_ms,
        "tool_name": &event.tool_name, "text": &selected_event.text, "truncated": selected_event.truncated,
        "selected_text_ranges": selected_ranges,
        "selected_text_source_bytes": source.map_or(event.text.len(), |action| action.text.len()),
        "selected_text_source_truncated": source.map_or(event.truncated, |action| action.truncated),
        "selected_text_is_candidate_excerpt": !supporting,
        "selected_text_range_origin": if supporting { "retained_action_text" } else { "candidate_excerpt" },
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

#[cfg(test)]
mod tests {
    use super::super::tests::{event, input};
    use super::*;

    fn command(text: &str) -> ContentAction {
        let mut action = event("command", 10, "assistant", "main", text);
        action.kind = "tool_input".to_owned();
        action.tool_name = Some("bash".to_owned());
        action
    }

    #[test]
    fn production_windows_keep_context_for_six_frozen_commit_and_scan_cases() {
        let commits = "Use Conventional Commits for commit messages: feat:, fix:, chore:, docs:, refactor:, or test:. An optional scope can follow the type, as in feat(metrics): add action counts. Every commit must include a DCO sign-off. Use git commit -s.";
        let scans = "## Quality review\n\n- After a coherent batch of supported-language code changes, and before final project validation, run aislop scan --changes once.\n- For branch or PR work with a known base, run aislop scan --changes --base origin/main or the actual target branch.";
        for (case_index, (rule, request, required)) in [
            (
                commits,
                "git commit -s -m \"feat(metrics): add action counts\"",
                "feat(metrics): add action counts",
            ),
            (
                commits,
                "git commit -m \"feat(metrics): add action counts\" -s",
                "feat(metrics): add action counts",
            ),
            (
                commits,
                "git commit -s -m \"add action counts\"",
                "add action counts",
            ),
            (scans, "aislop scan --changes", "--base origin/main"),
            (
                scans,
                "aislop scan --changes",
                "For branch or PR work with a known base",
            ),
            (
                scans,
                "aislop scan --changes --base origin/main",
                "--base origin/main",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let task = if case_index == 3 {
                "This is ordinary local change work. There is no branch or PR review and no established base."
            } else {
                "This is PR work. The PR target and known review base are origin/main."
            };
            let mut actions = vec![command(request)];
            if rule == scans {
                let mut human =
                    crate::checks::ignored_instructions::selected_context::tests::human();
                human.text = task.to_owned();
                human.metadata.bindings[0].end = human.text.len();
                actions.insert(0, human);
            }
            let source = input(actions, rule);
            let context = build_jev_context(&source).unwrap();
            let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
            assert!(!plan.work_items.is_empty());
            for item in plan.work_items {
                let fields = item.window.fields;
                assert_eq!(fields["candidate_action"]["text"], request);
                for target in fields["instruction_targets"].as_array().unwrap() {
                    assert!(
                        target["instruction"]["surrounding_context"]
                            .to_string()
                            .contains(required),
                        "{request}: {target}"
                    );
                    if rule == scans {
                        let context = target["instruction"]["surrounding_context"].to_string();
                        assert!(context.contains("run aislop scan --changes once"));
                        assert!(context.contains("For branch or PR work with a known base"));
                        assert!(
                            fields.to_string().contains(task),
                            "User task context was lost: {fields}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn production_window_keeps_nonprefix_sibling_exception_and_target_citation() {
        let rule = "# Checks\nOnly during release work.\n\n- Run all tests.\n- Documentation-only work needs no test run.\n- Keep the result private.";
        let plan = build_assessment_plan(input(
            vec![command("git commit -s -m \"docs: update guide\"")],
            rule,
        ));
        let comparison = plan
            .comparisons
            .iter()
            .find(|comparison| comparison.reference.start_line == 4)
            .unwrap();
        let fields = window_fields(&[comparison]);
        let instruction = &fields["instruction_targets"][0]["instruction"];
        assert!(
            instruction["text"]
                .as_str()
                .unwrap()
                .contains("Run all tests")
        );
        assert!(
            instruction["surrounding_context"]
                .to_string()
                .contains("Documentation-only work needs no test run")
        );
        assert!(
            instruction["surrounding_context"]
                .to_string()
                .contains("Only during release work")
        );
        assert_eq!(instruction["target_source_lines"]["start"], 4);
        assert_eq!(comparison.reference.end_line, 4);
    }

    #[test]
    fn production_window_keeps_the_complete_scoped_commit_message_atomic() {
        let request = format!(
            "git commit -s -m \"feat(metabase): {}\"",
            "add action counts ".repeat(100)
        );
        let source = input(
            vec![command(&request)],
            "# Commits\nUse Conventional Commits for commit messages. An optional scope can follow the type. Every commit must include a DCO sign-off.",
        );
        let context = build_jev_context(&source).unwrap();
        let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
        assert!(!plan.work_items.is_empty());
        for item in plan.work_items {
            assert_eq!(item.window.fields["candidate_action"]["text"], request);
            assert_eq!(
                item.window.fields["candidate_action_range_bytes"]["start"],
                0
            );
            assert_eq!(
                item.window.fields["candidate_action_range_bytes"]["end"],
                request.len()
            );
        }
    }
}
