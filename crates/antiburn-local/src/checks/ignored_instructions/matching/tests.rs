use super::super::tests::{event, input};
use super::*;
use crate::analysis::jev::{
    JevExecutionOutcome, JevResponse, JevRunProgress, JevUsage, run_jev_check,
};
use std::sync::{Arc, Mutex};

#[test]
fn episode_context_is_selected_only_after_prerequisite_classification() {
    let mut actions = vec![event(
        "validation",
        1,
        "assistant",
        "main",
        "Requested validation for the package.",
    )];
    actions.extend((2..14).map(|index| {
        event(
            &format!("note-{index}"),
            index,
            "assistant",
            "main",
            "Reviewed a note.",
        )
    }));
    actions.push(event(
        "publish",
        14,
        "assistant",
        "main",
        "Requested package publication.",
    ));
    let context =
        build_jev_context(&input(actions, "Request validation before publication.")).unwrap();
    for obligation in ["action", "prerequisite"] {
        let mut plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
        assert!(
            plan.prepared
                .comparisons
                .iter()
                .all(|comparison| comparison.prerequisite_episode.is_none())
        );
        let items = rule_classifications(&plan.prepared).unwrap();
        let results = items
            .into_iter()
            .map(|item| {
                let result = JevWorkItemResult {
                    request_id: "classification".to_owned(),
                    work_item_id: item.id.clone(),
                    answers: item
                        .questions
                        .iter()
                        .map(|(id, question)| {
                            let option = match id.as_str() {
                                "permission" => "independent",
                                "condition_evidence" => "selected",
                                "action_family" => "any",
                                "obligation" => obligation,
                                "read_prerequisite" => "not_read_order",
                                "read_trigger" => "not_read_rule",
                                _ => panic!("unexpected question {id}"),
                            };
                            (id.clone(), answer(question, option))
                        })
                        .collect(),
                    evidence: item.window.evidence,
                    model: ASSESSMENT_MODEL.to_owned(),
                    usage: JevUsage {
                        input_tokens: 0,
                        output_tokens: 0,
                    },
                };
                (item.id, result)
            })
            .collect();
        apply_rule_matching(&mut plan, &results, &context).unwrap();
        let comparison = plan
            .prepared
            .comparisons
            .iter()
            .find(|comparison| comparison.reference.action_id == "publish")
            .unwrap();
        assert_eq!(
            comparison.prerequisite_episode.is_some(),
            obligation == "prerequisite"
        );
        let item = plan
            .work_items
            .iter()
            .find(|item| {
                item.window.evidence.iter().any(|evidence| {
                    evidence.role == JevEvidenceRole::Candidate && evidence.source_id == "publish"
                })
            })
            .unwrap();
        assert_eq!(
            item.window
                .evidence
                .iter()
                .filter(|evidence| evidence.role == JevEvidenceRole::Candidate)
                .count(),
            1
        );
        if obligation == "prerequisite" {
            let events = item.window.fields["instruction_targets"][0]["earlier_counterevidence"]
                .as_array()
                .unwrap();
            assert_eq!(events.len(), 13);
            assert_eq!(events[0]["text"], "Requested validation for the package.");
            assert!(item.window.fields["instruction_targets"][0]["assessment_limits"]["prerequisite_episode_complete"].as_bool().unwrap());
            assert!(!item.window.fields.to_string().contains("source_key_digest"));
            assert!(!item.window.fields.to_string().contains("validation\""));
        }
    }
}

#[test]
fn ambiguous_read_path_cannot_prove_a_missing_prerequisite() {
    for path in [
        "/workspace/docs/guide.md",
        "docs\\guide.md",
        "$ROOT/docs/guide.md",
        "./docs/guide.md",
    ] {
        let mut read = event(
            "read",
            1,
            "assistant",
            "main",
            &json!({"file_path": path}).to_string(),
        );
        read.kind = "tool_input".to_owned();
        read.tool_name = Some("Read".to_owned());
        read.tool_call_id = Some("read-call".to_owned());
        let mut edit = event(
            "edit",
            2,
            "assistant",
            "main",
            r#"{"file_path":"src/main.rs"}"#,
        );
        edit.kind = "tool_input".to_owned();
        edit.tool_name = Some("Edit".to_owned());
        edit.tool_call_id = Some("edit-call".to_owned());
        let source = input(
            vec![read, edit],
            "Request a read of `docs/guide.md` before editing.",
        );
        let plan = build_assessment_plan(source.clone());
        let orders = exact_read_orders(&plan.comparisons, &source.content, true);
        let candidate = plan
            .comparisons
            .iter()
            .find(|comparison| comparison.reference.action_id == "edit")
            .unwrap();
        assert!(orders[&candidate.id].iter().all(|order| !order.paths_known));
        assert!(
            orders[&candidate.id]
                .iter()
                .all(|order| order.state() == ObligationState::Pending)
        );
    }
}

#[test]
fn edit_classification_keeps_shell_edit_requests_in_executed_candidates() {
    let mut shell = event(
        "shell-edit",
        10,
        "assistant",
        "main",
        r#"{"command":"sed -i '' 's/old/new/' /etc/service.conf"}"#,
    );
    shell.kind = "tool_input".to_owned();
    shell.tool_name = Some("Bash".to_owned());
    let context =
        build_jev_context(&input(vec![shell], "Do not edit `/etc/service.conf`.")).unwrap();
    let mut plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    let item = rule_classifications(&plan.prepared).unwrap().remove(0);
    let result = JevWorkItemResult {
        request_id: "classification".to_owned(),
        work_item_id: item.id.clone(),
        answers: item
            .questions
            .iter()
            .map(|(id, question)| {
                let option = match id.as_str() {
                    "permission" => "independent",
                    "condition_evidence" => "selected",
                    "action_family" => "edit",
                    "obligation" => "action",
                    "read_prerequisite" => "not_read_order",
                    "read_trigger" => "not_read_rule",
                    _ if id.starts_with("literal_qualification_") => "qualified",
                    _ if id.starts_with("literal_policy_") => "literal_other",
                    _ => "other_path",
                };
                (id.clone(), answer(question, option))
            })
            .collect(),
        evidence: item.window.evidence,
        model: ASSESSMENT_MODEL.to_owned(),
        usage: JevUsage {
            input_tokens: 0,
            output_tokens: 0,
        },
    };
    apply_rule_matching(&mut plan, &BTreeMap::from([(item.id, result)]), &context).unwrap();
    assert!(
        plan.work_items
            .iter()
            .any(|item| item.window.evidence.iter().any(|binding| {
                binding.role == JevEvidenceRole::Candidate && binding.source_id == "shell-edit"
            }))
    );
    assert!(plan.prepared.coverage.skipped_actions.is_empty());
}

#[test]
fn checkpoint_reduction_requires_permission_classification_even_on_an_unmatched_plan() {
    let context = build_jev_context(&input(
        vec![event(
            "report",
            10,
            "assistant",
            "main",
            "I deployed without approval.",
        )],
        "Do not deploy without owner approval.",
    ))
    .unwrap();
    let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    let item = &plan.work_items[0];
    let mut initial = JevWorkItemResult {
        request_id: "screen".to_owned(),
        work_item_id: item.id.clone(),
        answers: item
            .questions
            .iter()
            .map(|(id, question)| (id.clone(), answer(question, "applies")))
            .collect(),
        evidence: item.window.evidence.clone(),
        model: ASSESSMENT_MODEL.to_owned(),
        usage: JevUsage {
            input_tokens: 0,
            output_tokens: 0,
        },
    };
    let followup = IgnoredInstructionsCheck
        .reconcile(item, &initial, &context)
        .unwrap()
        .unwrap();
    initial
        .answers
        .extend(followup.questions.iter().map(|(id, question)| {
            let JevQuestion::Choice { criteria, .. } = question else {
                panic!("expected Choice")
            };
            let selected = if criteria.contains_key("conflict") {
                "conflict"
            } else if criteria.contains_key("self_contained") {
                "self_contained"
            } else {
                "not_completion_obligation"
            };
            (id.clone(), answer(question, selected))
        }));
    let rule_item = rule_classifications(&plan.prepared).unwrap().remove(0);
    let classification = JevWorkItemResult {
        request_id: "classification".to_owned(),
        work_item_id: rule_item.id,
        answers: rule_item
            .questions
            .iter()
            .map(|(id, question)| {
                let option = match id.as_str() {
                    "permission" => "authoritative_approval",
                    "condition_evidence" => "selected",
                    "read_prerequisite" => "not_read_order",
                    "read_trigger" => "not_read_rule",
                    _ if id.starts_with("literal_qualification_") => "qualified",
                    "action_family" => "any",
                    "obligation" => "action",
                    _ if id.starts_with("literal_policy_") => "literal_other",
                    _ => panic!("unexpected classification"),
                };
                (id.clone(), answer(question, option))
            })
            .collect(),
        evidence: rule_item.window.evidence,
        model: ASSESSMENT_MODEL.to_owned(),
        usage: JevUsage {
            input_tokens: 0,
            output_tokens: 0,
        },
    };
    for results in [vec![initial.clone()], vec![initial, classification]] {
        let reduced = IgnoredInstructionsCheck
            .reduce(&plan, &results, false)
            .unwrap();
        assert!(reduced.findings.is_empty());
        assert_eq!(reduced.unassessed_comparisons.len(), 1);
    }
}

#[test]
fn exact_read_order_accepts_selected_paths_with_and_without_native_normalization() {
    use crate::analysis::jev::obligations::ObligationState;
    for normalized in [false, true] {
        let mut read = event(
            "read-call",
            10,
            "assistant",
            "main",
            "{\"path\":\"docs/policy.md\"}",
        );
        read.kind = "tool_input".to_owned();
        read.tool_name = Some("Read".to_owned());
        read.tool_call_id = Some("native-read-call".to_owned());
        if normalized {
            read.normalized_fields = Some(crate::analysis::jev_evidence::normalize_tool_input(
                "Read", &read.text,
            ));
        }
        let mut edit = event(
            "edit-call",
            20,
            "assistant",
            "main",
            "{\"path\":\"src/ui/a.rs\"}",
        );
        edit.kind = "tool_input".to_owned();
        edit.tool_name = Some("Edit".to_owned());
        edit.tool_call_id = Some("native-edit-call".to_owned());
        let mut assessment = input(
            vec![read, edit],
            "Request a read of `docs/policy.md` before requesting edits under `src/ui/`.",
        );
        assessment.content = crate::checks::ignored_instructions::select_session_content(
            &assessment.content,
            INPUT_SELECTION,
        );
        let plan = build_assessment_plan(assessment.clone());
        let candidate = plan
            .comparisons
            .iter()
            .find(|comparison| comparison.action.action_id == "edit-call")
            .unwrap();
        let order = plan.read_request_orders[&candidate.id]
            .iter()
            .find(|order| order.required_path == "docs/policy.md")
            .unwrap();
        assert_eq!(order.state(), ObligationState::Satisfied);
        assert_eq!(order.earlier_request_id.as_deref(), Some("read-call"));
        let mut with_unrelated = assessment.clone();
        let mut malformed_shell = event("malformed-shell", 15, "assistant", "main", "{}");
        malformed_shell.kind = "tool_input".to_owned();
        malformed_shell.tool_name = Some("Bash".to_owned());
        malformed_shell.tool_call_id = Some("shell-call".to_owned());
        with_unrelated.content.actions.insert(1, malformed_shell);
        let plan = build_assessment_plan(with_unrelated);
        let edit = plan
            .comparisons
            .iter()
            .find(|comparison| comparison.action.action_id == "edit-call")
            .unwrap();
        let order = plan.read_request_orders[&edit.id]
            .iter()
            .find(|order| order.required_path == "docs/policy.md")
            .unwrap();
        assert_eq!(order.state(), ObligationState::Satisfied);
        for (read_part, edit_part, expected) in [
            (0, 1, ObligationState::Satisfied),
            (1, 0, ObligationState::Violated),
        ] {
            let mut same_record = assessment.clone();
            same_record.content.actions[0].reference.turn_index = 10;
            same_record.content.actions[0].reference.part_index = read_part;
            same_record.content.actions[1].reference.turn_index = 10;
            same_record.content.actions[1].reference.part_index = edit_part;
            same_record.content.actions[0].timestamp_ms = Some(200);
            same_record.content.actions[1].timestamp_ms = Some(100);
            let plan = build_assessment_plan(same_record);
            let candidate = plan
                .comparisons
                .iter()
                .find(|comparison| comparison.action.action_id == "edit-call")
                .unwrap();
            let order = plan.read_request_orders[&candidate.id]
                .iter()
                .find(|order| order.required_path == "docs/policy.md")
                .unwrap();
            assert_eq!(order.state(), expected);
        }
        assessment.content.actions[0].reference.thread_digest = "sibling".to_owned();
        let plan = build_assessment_plan(assessment);
        let candidate = plan
            .comparisons
            .iter()
            .find(|comparison| comparison.action.action_id == "edit-call")
            .unwrap();
        let order = plan.read_request_orders[&candidate.id]
            .iter()
            .find(|order| order.required_path == "docs/policy.md")
            .unwrap();
        assert_eq!(order.state(), ObligationState::Violated);
    }
}

#[test]
fn classification_keeps_provenance_and_scope_contexts_separate() {
    let mut assessment = input(
        vec![event(
            "action",
            10,
            "assistant",
            "main",
            "Requested the command.",
        )],
        "Never request the command.",
    );
    let mut current = assessment.content.instructions[0].clone();
    current.provenance = InstructionProvenance::CurrentFileComparison;
    assessment.content.instructions.push(current);
    let mut global = assessment.content.instructions[0].clone();
    global.scope = InstructionScope::Global;
    assessment.content.instructions.push(global);
    let plan = build_assessment_plan(assessment);
    assert_eq!(
        plan.comparisons
            .iter()
            .map(|comparison| &comparison.id)
            .collect::<BTreeSet<_>>()
            .len(),
        3
    );
    let items = rule_classifications(&plan).unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(
        items
            .iter()
            .map(|item| &item.id)
            .collect::<BTreeSet<_>>()
            .len(),
        3
    );
}

#[test]
fn oversized_unclassified_rules_keep_explicit_unknown_observations() {
    let rule = format!(
        "Never deploy without approval. {}",
        "Additional policy context. ".repeat(1000)
    );
    let context = build_jev_context(&input(
        vec![event("report", 10, "assistant", "main", "I deployed.")],
        &rule,
    ))
    .unwrap();
    let mut plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    assert!(rule_classifications(&plan.prepared).unwrap().is_empty());
    apply_rule_matching(&mut plan, &BTreeMap::new(), &context).unwrap();
    assert_eq!(
        plan.prepared.observable_obligations.len(),
        plan.prepared.comparisons.len()
    );
    assert!(
        plan.prepared
            .observable_obligations
            .values()
            .all(|observation| observation.permission == PermissionRequirement::Unknown)
    );
}

#[tokio::test]
async fn prerequisite_questions_use_recorded_order_and_do_not_repeat_rule_classification() {
    let mut assessment = input(
        vec![event(
            "trigger",
            20,
            "assistant",
            "main",
            "I committed the change.",
        )],
        "Run tests before committing.",
    );
    assessment.prior_history_complete = false;
    let context = build_jev_context(&assessment).unwrap();
    let outcome = run_jev_check(
        &IgnoredInstructionsCheck,
        &context,
        JevRunProgress::default(),
        |batch| async move {
            let answers = batch
                .request
                .questions
                .iter()
                .map(|(id, question)| {
                    let JevQuestion::Choice { criteria, .. } = question else {
                        panic!("expected Choice")
                    };
                    let option = if criteria.contains_key("not_read_rule") {
                        "not_read_rule"
                    } else if criteria.contains_key("unqualified") {
                        "qualified"
                    } else if criteria.contains_key("literal_other") {
                        "literal_other"
                    } else if criteria.contains_key("selected") {
                        "selected"
                    } else if criteria.contains_key("independent") {
                        "independent"
                    } else if criteria.contains_key("not_read_order") {
                        "not_read_order"
                    } else if criteria.contains_key("other_path") {
                        "other_path"
                    } else if criteria.contains_key("any") {
                        "any"
                    } else if criteria.contains_key("prerequisite") {
                        "prerequisite"
                    } else if criteria.contains_key("applies") {
                        "applies"
                    } else if criteria.contains_key("insufficient_evidence") {
                        "insufficient_evidence"
                    } else if criteria.contains_key("evidence_incomplete") {
                        "evidence_incomplete"
                    } else {
                        panic!("completion classification repeats at an action boundary")
                    };
                    (id.clone(), answer(question, option))
                })
                .collect();
            Ok(JevResponse {
                model: ASSESSMENT_MODEL.to_owned(),
                answers,
                usage: JevUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            })
        },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert!(outcome.result.findings.is_empty());
    assert_eq!(outcome.result.unassessed_comparisons.len(), 1);
    assert!(outcome.result.pending_rules.is_empty());
    assert_eq!(
        outcome
            .progress
            .results
            .values()
            .find(|result| result.work_item_id.ends_with("::followup"))
            .unwrap()
            .answers
            .len(),
        2
    );
}

fn answer(question: &JevQuestion, option: &str) -> JevAnswer {
    let JevQuestion::Choice { criteria, .. } = question else {
        panic!("expected Choice")
    };
    assert!(criteria.contains_key(option));
    JevAnswer::Choice {
        choice: option.to_owned(),
        probabilities: criteria
            .keys()
            .map(|key| {
                (
                    key.clone(),
                    if key == option {
                        0.97
                    } else {
                        0.03 / (criteria.len() - 1) as f64
                    },
                )
            })
            .collect(),
        confidence: 0.97,
    }
}

async fn run(
    context: &JevSessionContext,
    progress: JevRunProgress,
    family: &str,
    obligation: &str,
) -> (JevExecutionOutcome<AssessmentResult>, Vec<String>) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let calls = sent.clone();
    let outcome = run_jev_check(
        &IgnoredInstructionsCheck,
        context,
        progress,
        |batch| {
            calls.lock().unwrap().extend(batch.work_item_ids.clone());
            async move {
                let answers = batch
                    .request
                    .questions
                    .iter()
                    .map(|(id, question)| {
                        let JevQuestion::Choice { criteria, .. } = question else {
                            panic!("expected Choice")
                        };
                        let option = if criteria.contains_key("not_read_rule") {
                            "not_read_rule"
                        } else if criteria.contains_key("unqualified") {
                            "qualified"
                        } else if criteria.contains_key("literal_other") {
                            "literal_other"
                        } else if criteria.contains_key("selected") {
                            "selected"
                        } else if criteria.contains_key("independent") {
                            "independent"
                        } else if criteria.contains_key("not_read_order") {
                            "not_read_order"
                        } else if criteria.contains_key("other_path") {
                            "other_path"
                        } else if criteria.contains_key("any") {
                            family
                        } else if criteria.contains_key("prerequisite") {
                            obligation
                        } else if criteria.contains_key("not_applicable") {
                            "not_applicable"
                        } else if criteria.contains_key("unrelated") {
                            "unrelated"
                        } else if criteria.contains_key("self_contained") {
                            "self_contained"
                        } else if criteria.contains_key("not_completion_obligation") {
                            "not_completion_obligation"
                        } else {
                            panic!("unexpected follow-up")
                        };
                        (id.clone(), answer(question, option))
                    })
                    .collect();
                Ok(JevResponse {
                    model: ASSESSMENT_MODEL.to_owned(),
                    answers,
                    usage: JevUsage {
                        input_tokens: 10,
                        output_tokens: 2,
                    },
                })
            }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();
    let sent = sent.lock().unwrap().clone();
    (outcome, sent)
}

#[tokio::test]
async fn append_reuses_unchanged_judgments_and_classification() {
    let original = input(
        vec![event(
            "first",
            10,
            "assistant",
            "main",
            "Used the documented format.",
        )],
        "Use the documented format.",
    );
    let first_context = build_jev_context(&original).unwrap();
    let (first, _) = run(&first_context, JevRunProgress::default(), "any", "action").await;
    let first_id = first
        .progress
        .results
        .keys()
        .find(|id| !id.starts_with("classification-"))
        .unwrap()
        .clone();
    let saved = first.progress.results[&first_id].clone();
    let mut appended = original.clone();
    appended.content.publication_fence += 1;
    appended.content.selected_input_digest = "appended-selected-input".to_owned();
    appended.content.actions.push(event(
        "second",
        20,
        "assistant",
        "other-branch",
        "Used the documented format again.",
    ));
    let appended_context = build_jev_context(&appended).unwrap();
    let (second, sent) = run(&appended_context, first.progress.clone(), "any", "action").await;
    assert!(second.complete);
    assert_eq!(sent.len(), 1);
    assert!(
        !sent
            .iter()
            .any(|id| id == &first_id || id.starts_with("classification-"))
    );
    assert_eq!(second.progress.results[&first_id], saved);
    let (resumed, sent) = run(&appended_context, second.progress, "any", "action").await;
    assert!(sent.is_empty());
    assert_eq!(resumed.result, second.result);
    appended.incarnation += 1;
    let (_, sent) = run(
        &build_jev_context(&appended).unwrap(),
        resumed.progress,
        "any",
        "action",
    )
    .await;
    assert!(sent.iter().any(|id| id.starts_with("classification-")));
    assert!(sent.contains(&first_id));
}

#[tokio::test]
async fn distant_episode_content_invalidates_the_anchored_semantic_answer() {
    let mut actions = vec![event(
        "earlier",
        1,
        "assistant",
        "main",
        &format!(
            "{}\nRequested validation for this package.",
            "Recorded context. ".repeat(80)
        ),
    )];
    actions.extend((2..12).map(|index| {
        event(
            &format!("note-{index}"),
            index,
            "assistant",
            "main",
            "Reviewed a note.",
        )
    }));
    actions.push(event(
        "publish",
        12,
        "assistant",
        "main",
        "Requested package publication.",
    ));
    let mut source = input(actions, "Request validation before publication.");
    let first_context = build_jev_context(&source).unwrap();
    let (first, _) = run(
        &first_context,
        JevRunProgress::default(),
        "any",
        "prerequisite",
    )
    .await;
    let plan = IgnoredInstructionsCheck.prepare(&first_context).unwrap();
    let publish_id = plan
        .work_items
        .iter()
        .find(|item| {
            item.window.evidence.iter().any(|binding| {
                binding.role == JevEvidenceRole::Candidate && binding.source_id == "publish"
            })
        })
        .unwrap()
        .id
        .clone();
    let comparison_id = plan
        .prepared
        .comparisons
        .iter()
        .find(|comparison| comparison.reference.action_id == "publish")
        .unwrap()
        .id
        .clone();
    source.content.actions[0].text = source.content.actions[0]
        .text
        .replace("Requested validation", "Planned validation");
    let changed = build_jev_context(&source).unwrap();
    let changed_plan = IgnoredInstructionsCheck.prepare(&changed).unwrap();
    assert!(
        changed_plan
            .prepared
            .comparisons
            .iter()
            .any(|comparison| comparison.id == comparison_id
                && comparison.reference.action_id == "publish")
    );
    let (_, sent) = run(&changed, first.progress, "any", "prerequisite").await;
    assert!(sent.contains(&publish_id));
}

#[tokio::test]
async fn legacy_candidate_without_source_binding_cannot_publish_a_new_clean_result() {
    let mut context = build_jev_context(&input(
        vec![event(
            "candidate",
            1,
            "assistant",
            "main",
            "Recorded communication.",
        )],
        "Use the required response format.",
    ))
    .unwrap();
    context.check_context["assessment_plan"]["comparisons"][0]["source_binding"] = Value::Null;
    let (outcome, _) = run(&context, JevRunProgress::default(), "any", "action").await;
    assert!(outcome.result.findings.is_empty());
    assert_eq!(outcome.result.unassessed_comparisons.len(), 1);
    assert!(
        outcome
            .result
            .coverage
            .limitations
            .contains(&"action_source_binding_unavailable".to_owned())
    );
}

#[tokio::test]
async fn changed_context_and_reference_invalidate_affected_work() {
    let original = input(
        vec![event(
            "first",
            10,
            "assistant",
            "main",
            "Used the documented format.",
        )],
        "Use the documented format.",
    );
    let (first, _) = run(
        &build_jev_context(&original).unwrap(),
        JevRunProgress::default(),
        "any",
        "action",
    )
    .await;
    let first_id = first
        .progress
        .results
        .keys()
        .find(|id| !id.starts_with("classification-"))
        .unwrap()
        .clone();
    let mut changed = original;
    changed.content.actions.push(event(
        "context",
        20,
        "assistant",
        "main",
        "This step changes the relevant context.",
    ));
    changed.content.selected_input_digest = "changed-context".to_owned();
    let (_, sent) = run(
        &build_jev_context(&changed).unwrap(),
        first.progress.clone(),
        "any",
        "action",
    )
    .await;
    assert!(sent.contains(&first_id));
    changed.content.instructions = input(Vec::new(), "Use a different documented format.")
        .content
        .instructions;
    let (_, sent) = run(
        &build_jev_context(&changed).unwrap(),
        first.progress,
        "any",
        "action",
    )
    .await;
    assert!(sent.iter().any(|id| id.starts_with("classification-")));
}

#[tokio::test]
async fn completion_obligations_stay_pending_once_without_a_recorded_boundary() {
    let context = build_jev_context(&input(
        vec![
            event("plan", 10, "assistant", "main", "I will include a summary."),
            event("pause", 20, "assistant", "main", "Still working."),
        ],
        "Include a summary in the final response.",
    ))
    .unwrap();
    let (outcome, sent) = run(
        &context,
        JevRunProgress::default(),
        "assistant",
        "completion",
    )
    .await;
    assert_eq!(sent.len(), 1);
    assert_eq!(outcome.result.pending_rules.len(), 1);
    assert_eq!(
        outcome.result.pending_rules[0].reason,
        "completion_boundary_unavailable"
    );
    assert_eq!(outcome.result.unassessed_comparisons.len(), 2);
    assert!(outcome.result.findings.is_empty());
}

#[test]
fn classification_is_once_per_rule_and_unknown_properties_keep_all_candidates() {
    let context = build_jev_context(&input(
        (0..20)
            .map(|index| {
                event(
                    &format!("action-{index}"),
                    index,
                    "assistant",
                    "main",
                    "A low-overlap paraphrase.",
                )
            })
            .collect(),
        "Do not request the prohibited command.",
    ))
    .unwrap();
    let mut plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    assert_eq!(rule_classifications(&plan.prepared).unwrap().len(), 1);
    let original = plan.work_items.clone();
    apply_rule_matching(&mut plan, &BTreeMap::new(), &context).unwrap();
    assert_eq!(plan.work_items.len(), original.len());
    assert_eq!(plan.prepared.coverage.skipped_actions.len(), 0);
}

#[tokio::test]
async fn typed_matching_uses_selected_fields_and_keeps_omissions_unassessed() {
    let mut read = event("read", 10, "assistant", "main", "{\"path\":\"a.rs\"}");
    read.kind = "tool_input".to_owned();
    read.tool_name = Some("Read".to_owned());
    read.normalized_fields = Some(crate::analysis::jev::JevNormalizedFields {
        category: Some(crate::analysis::jev::JevNormalizedCategory::ReadFile),
        values: BTreeMap::from([(JevInputField::ReadFilePath, read.text.clone())]),
        malformed: false,
    });
    let context = build_jev_context(&input(vec![read], "Never request a shell command.")).unwrap();
    let (outcome, sent) = run(&context, JevRunProgress::default(), "bash", "action").await;
    assert_eq!(sent.len(), 1);
    assert_eq!(outcome.result.unassessed_comparisons.len(), 1);
    assert_eq!(outcome.result.coverage.skipped_actions.len(), 1);
    assert!(outcome.result.findings.is_empty());
}
