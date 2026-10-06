use super::super::tests::{event, input};
use super::*;
use crate::analysis::jev::JevUsage;

fn response(
    comparison: &CandidateComparison,
    relationship: &str,
    basis: &str,
    completion: &str,
) -> JevWorkItemResult {
    let answers = [
        (
            QUESTION_APPLICABILITY,
            "applies",
            vec!["applies", "not_applicable", "uncertain"],
        ),
        (
            QUESTION_RELATIONSHIP,
            relationship,
            vec!["conflict", "follows", "unrelated", "insufficient_evidence"],
        ),
        (
            QUESTION_EVIDENCE_BASIS,
            basis,
            vec!["self_contained", "evidence_incomplete", "uncertain"],
        ),
        (
            QUESTION_COMPLETION,
            completion,
            vec![
                "not_completion_obligation",
                "completion_not_observed",
                "completion_observed",
                "uncertain",
            ],
        ),
    ]
    .into_iter()
    .map(|(id, choice, options)| {
        let probabilities = options
            .iter()
            .map(|option| {
                (
                    (*option).to_owned(),
                    if *option == choice {
                        0.97
                    } else {
                        0.03 / (options.len() - 1) as f64
                    },
                )
            })
            .collect();
        (
            id.to_owned(),
            JevAnswer::Choice {
                choice: choice.to_owned(),
                probabilities,
                confidence: 0.97,
            },
        )
    })
    .collect();
    JevWorkItemResult {
        request_id: "synthetic-request".to_owned(),
        work_item_id: comparison.id.clone(),
        answers,
        evidence: Vec::new(),
        model: ASSESSMENT_MODEL.to_owned(),
        usage: JevUsage {
            input_tokens: 0,
            output_tokens: 0,
        },
    }
}

fn assessment() -> AssessmentPlan {
    build_assessment_plan(input(
        vec![event(
            "report",
            10,
            "assistant",
            "main",
            "I finished the update.",
        )],
        "Include a validation summary when the update is complete.",
    ))
}

#[test]
fn partial_response_and_rule_ranges_cannot_clear_a_whole_response_finding() {
    let mut plan = build_assessment_plan(input(
        vec![event(
            "reply",
            10,
            "assistant",
            "main",
            &format!("{} required", "a".repeat(3000)),
        )],
        "Include `required` in every response.",
    ));
    assert!(plan.comparisons.len() > 1);
    let comparison = plan.comparisons[0].clone();
    let mut answer = response(
        &comparison,
        "unrelated",
        "self_contained",
        "not_completion_obligation",
    );
    answer
        .answers
        .retain(|question, _| question == QUESTION_APPLICABILITY);
    set_probability(&mut answer, QUESTION_APPLICABILITY, "not_applicable", 0.97);
    plan.comparisons = vec![comparison.clone()];
    let result = reduce_assessment(
        &plan,
        &BTreeMap::from([(comparison.id.clone(), answer.clone())]),
        true,
    );
    assert_eq!(
        result.unassessed_comparisons,
        std::slice::from_ref(&comparison.id)
    );
    assert!(result.coverage.reassessed_finding_ids.is_empty());

    let mut partial_rule = comparison;
    partial_rule.action.truncated = false;
    partial_rule.rule_text_end = partial_rule.rule_text.len() - 1;
    plan.comparisons = vec![partial_rule.clone()];
    let result = reduce_assessment(
        &plan,
        &BTreeMap::from([(partial_rule.id.clone(), answer)]),
        true,
    );
    assert!(result.coverage.reassessed_finding_ids.is_empty());
}

#[test]
fn observable_development_scenarios_require_decisive_candidate_answers() {
    for (rule, action, relationship, expected_findings) in [
        (
            "Answer the user's question with one file path.",
            "Done.",
            "conflict",
            1,
        ),
        (
            "Do not run destructive commands.",
            "Example only: `wipe-cache --all`. I did not run it.",
            "follows",
            0,
        ),
    ] {
        let plan = build_assessment_plan(input(
            vec![event("reply", 10, "assistant", "main", action)],
            rule,
        ));
        let comparison = &plan.comparisons[0];
        let answer = response(
            comparison,
            relationship,
            "self_contained",
            "not_completion_obligation",
        );
        let result = reduce_assessment(
            &plan,
            &BTreeMap::from([(comparison.id.clone(), answer.clone())]),
            true,
        );
        assert_eq!(result.findings.len(), expected_findings, "{rule}");
        assert!(result.unassessed_comparisons.is_empty(), "{rule}");

        let mut uncertain = answer;
        set_probability(&mut uncertain, QUESTION_RELATIONSHIP, relationship, 0.80);
        let result = reduce_assessment(
            &plan,
            &BTreeMap::from([(comparison.id.clone(), uncertain)]),
            true,
        );
        assert!(result.findings.is_empty(), "{rule}");
        assert_eq!(result.unassessed_comparisons.len(), 1, "{rule}");
        assert_eq!(result.unassessed_comparisons[0], comparison.id, "{rule}");
    }
}

#[test]
fn reassessment_finding_identity_requires_a_conclusive_judgment() {
    let plan = assessment();
    let comparison = &plan.comparisons[0];
    let expected_id = finding_id_for_reference(&comparison.reference);
    let compliant = reduce_assessment(
        &plan,
        &BTreeMap::from([(
            comparison.id.clone(),
            response(
                comparison,
                "follows",
                "self_contained",
                "not_completion_obligation",
            ),
        )]),
        true,
    );
    assert_eq!(compliant.coverage.reassessed_finding_ids, [expected_id]);

    let uncertain = reduce_assessment(
        &plan,
        &BTreeMap::from([(
            comparison.id.clone(),
            response(
                comparison,
                "insufficient_evidence",
                "evidence_incomplete",
                "uncertain",
            ),
        )]),
        true,
    );
    assert!(uncertain.coverage.reassessed_finding_ids.is_empty());
}

#[test]
fn authority_gate_rejects_confident_conflicts_and_confident_clean_answers() {
    use crate::analysis::jev::obligations::PermissionRequirement;
    for permission in [
        PermissionRequirement::AuthoritativeApproval,
        PermissionRequirement::Unknown,
        PermissionRequirement::ApprovalClaim,
    ] {
        for relationship in ["conflict", "follows"] {
            let mut plan = assessment();
            let id = plan.comparisons[0].id.clone();
            plan.observable_obligations.insert(
                id.clone(),
                ObservableObligation {
                    literal_policies: Vec::new(),
                    condition_evidence:
                        crate::analysis::jev::obligations::ConditionEvidence::Selected,
                    prerequisite_required: false,
                    permission,
                    read_request_order: None,
                    read_order_required: false,
                    read_order_unknown: false,
                    read_success_required: false,
                    read_prerequisite_absent: false,
                    candidate_family: "assistant".to_owned(),
                    edit_scope_matches: None,
                    recorded_edit_only: false,
                    edit_scope_unknown: false,
                },
            );
            let answer = response(
                &plan.comparisons[0],
                relationship,
                "self_contained",
                "not_completion_obligation",
            );
            let result = reduce_assessment(&plan, &BTreeMap::from([(id, answer)]), true);
            if permission == PermissionRequirement::ApprovalClaim {
                assert!(result.unassessed_comparisons.is_empty());
                assert_eq!(
                    result.findings.len(),
                    usize::from(relationship == "conflict")
                );
            } else {
                assert!(result.findings.is_empty());
                assert_eq!(result.unassessed_comparisons.len(), 1);
            }
        }
    }
}

#[test]
fn unavailable_condition_blocks_conflict_and_clean_independently_of_model_answers() {
    use crate::analysis::jev::obligations::{ConditionEvidence, PermissionRequirement};
    for condition in [
        ConditionEvidence::Result,
        ConditionEvidence::Undefined,
        ConditionEvidence::Unknown,
    ] {
        for relationship in ["conflict", "follows", "unrelated"] {
            let mut plan = assessment();
            let id = plan.comparisons[0].id.clone();
            plan.observable_obligations.insert(
                id.clone(),
                ObservableObligation {
                    literal_policies: Vec::new(),
                    condition_evidence: condition,
                    prerequisite_required: false,
                    permission: PermissionRequirement::Independent,
                    read_request_order: None,
                    read_order_required: false,
                    read_order_unknown: false,
                    read_success_required: false,
                    read_prerequisite_absent: false,
                    candidate_family: "assistant".to_owned(),
                    edit_scope_matches: None,
                    recorded_edit_only: false,
                    edit_scope_unknown: false,
                },
            );
            let answer = response(
                &plan.comparisons[0],
                relationship,
                "self_contained",
                "not_completion_obligation",
            );
            let result = reduce_assessment(&plan, &BTreeMap::from([(id, answer)]), true);
            assert!(result.findings.is_empty());
            assert_eq!(result.unassessed_comparisons.len(), 1);
        }
    }
}

#[test]
fn exact_request_order_overrides_model_order_but_never_missing_history() {
    use crate::analysis::jev::obligations::{PermissionRequirement, ReadRequestOrder};
    for (earlier, history_complete, paths_known, expected_finding, expected_unassessed) in [
        (false, true, true, 1, 0),
        (true, true, true, 0, 0),
        (false, false, true, 0, 1),
        (false, true, false, 0, 1),
        (true, false, true, 0, 0),
    ] {
        for relationship in ["follows", "conflict", "insufficient_evidence"] {
            let mut plan = assessment();
            let id = plan.comparisons[0].id.clone();
            plan.observable_obligations.insert(
                id.clone(),
                ObservableObligation {
                    literal_policies: Vec::new(),
                    condition_evidence:
                        crate::analysis::jev::obligations::ConditionEvidence::Selected,
                    prerequisite_required: true,
                    permission: PermissionRequirement::Independent,
                    read_request_order: Some(ReadRequestOrder {
                        required_path: "docs/policy.md".to_owned(),
                        earlier_request_id: earlier.then(|| "earlier-read-call".to_owned()),
                        later_request_id: Some("later-read-call".to_owned()),
                        history_complete,
                        paths_known,
                    }),
                    read_order_required: true,
                    read_order_unknown: false,
                    read_success_required: false,
                    read_prerequisite_absent: false,
                    candidate_family: "edit".to_owned(),
                    edit_scope_matches: None,
                    recorded_edit_only: false,
                    edit_scope_unknown: false,
                },
            );
            let answer = response(
                &plan.comparisons[0],
                relationship,
                "evidence_incomplete",
                "not_completion_obligation",
            );
            let result = reduce_assessment(&plan, &BTreeMap::from([(id, answer)]), true);
            assert_eq!(result.findings.len(), expected_finding);
            assert_eq!(result.unassessed_comparisons.len(), expected_unassessed);
            if expected_finding == 1 {
                assert!(
                    result.findings[0]
                        .limitations
                        .contains(&"recorded_read_request_order_only".to_owned())
                );
            }
        }
    }
}

#[test]
fn selected_record_completeness_does_not_override_missing_semantic_evidence() {
    let plan = assessment();
    assert!(plan.complete_input);
    for relationship in ["conflict", "follows", "unrelated"] {
        for basis in ["evidence_incomplete", "uncertain"] {
            let answer = response(
                &plan.comparisons[0],
                relationship,
                basis,
                "not_completion_obligation",
            );
            let result = reduce_assessment(
                &plan,
                &BTreeMap::from([(answer.work_item_id.clone(), answer)]),
                true,
            );
            assert!(result.findings.is_empty(), "{relationship}/{basis}");
            assert_eq!(
                result.unassessed_comparisons,
                vec![plan.comparisons[0].id.clone()]
            );
        }
    }
}

#[test]
fn unresolved_completion_cannot_publish_a_finding_or_clean_comparison() {
    let plan = assessment();
    for relationship in ["conflict", "follows", "unrelated"] {
        for completion in ["completion_not_observed", "uncertain"] {
            let answer = response(
                &plan.comparisons[0],
                relationship,
                "self_contained",
                completion,
            );
            let result = reduce_assessment(
                &plan,
                &BTreeMap::from([(answer.work_item_id.clone(), answer)]),
                true,
            );
            assert!(result.findings.is_empty(), "{relationship}/{completion}");
            assert_eq!(result.unassessed_comparisons.len(), 1);
            assert_eq!(
                result.pending_rules.len(),
                usize::from(completion == "completion_not_observed")
            );
        }
    }
}

#[test]
fn observed_completion_keeps_the_exact_local_rule_and_action_binding() {
    let plan = assessment();
    let comparison = &plan.comparisons[0];
    let answer = response(
        comparison,
        "conflict",
        "self_contained",
        "completion_observed",
    );
    let result = reduce_assessment(
        &plan,
        &BTreeMap::from([(answer.work_item_id.clone(), answer)]),
        true,
    );
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].reference, comparison.reference);
    assert_eq!(
        result.findings[0].instruction_excerpt,
        super::super::super::planning::rule_text_fragment(comparison)
    );
    assert_eq!(
        result.findings[0].action_excerpt,
        comparison
            .action
            .text
            .get(comparison.action_text_start..comparison.action_text_end)
            .unwrap()
    );
    assert!(result.unassessed_comparisons.is_empty());
    assert!(result.pending_rules.is_empty());
}

#[test]
fn classified_action_obligation_does_not_need_a_repeated_completion_answer() {
    let plan = assessment();
    let comparison = &plan.comparisons[0];
    let mut answer = response(
        comparison,
        "conflict",
        "self_contained",
        "not_completion_obligation",
    );
    answer.answers.remove(QUESTION_COMPLETION);
    let answers = BTreeMap::from([(answer.work_item_id.clone(), answer)]);
    let completion = BTreeMap::from([(comparison.id.clone(), CompletionCoverage::NotObligation)]);
    let result = reduce_with_completion(&plan, &answers, true, &completion);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].reference, comparison.reference);
}

fn set_probability(answer: &mut JevWorkItemResult, question: &str, option: &str, probability: f64) {
    let JevAnswer::Choice { probabilities, .. } = answer.answers.get_mut(question).unwrap() else {
        panic!("expected Choice")
    };
    let remaining = (1.0 - probability) / (probabilities.len() - 1) as f64;
    for (key, value) in probabilities {
        *value = if key == option {
            probability
        } else {
            remaining
        };
    }
}

#[test]
fn supplied_missing_evidence_cannot_be_overridden_by_not_applicable() {
    let plan = assessment();
    let mut answer = response(
        &plan.comparisons[0],
        "unrelated",
        "evidence_incomplete",
        "not_completion_obligation",
    );
    set_probability(&mut answer, QUESTION_APPLICABILITY, "not_applicable", 0.97);
    let result = reduce_assessment(
        &plan,
        &BTreeMap::from([(answer.work_item_id.clone(), answer)]),
        true,
    );
    assert!(result.findings.is_empty());
    assert_eq!(result.unassessed_comparisons.len(), 1);
}

#[test]
fn clean_requires_independent_relationship_and_complete_evidence() {
    let plan = assessment();
    for relationship in ["unrelated", "follows", "conflict", "insufficient_evidence"] {
        for missing_question in [
            None,
            Some(QUESTION_RELATIONSHIP),
            Some(QUESTION_EVIDENCE_BASIS),
        ] {
            let mut answer = response(
                &plan.comparisons[0],
                relationship,
                "self_contained",
                "not_completion_obligation",
            );
            set_probability(&mut answer, QUESTION_APPLICABILITY, "not_applicable", 0.97);
            if let Some(question) = missing_question {
                answer.answers.remove(question);
            }
            let result = reduce_assessment(
                &plan,
                &BTreeMap::from([(answer.work_item_id.clone(), answer)]),
                true,
            );
            assert!(result.findings.is_empty());
            assert_eq!(
                result.unassessed_comparisons.len(),
                usize::from(
                    !matches!(relationship, "unrelated" | "follows") || missing_question.is_some()
                ),
                "{relationship}/{missing_question:?}",
            );
        }
    }
}

#[test]
fn every_publication_gate_limits_finding_certainty() {
    let plan = assessment();
    for (question, option) in [
        (QUESTION_EVIDENCE_BASIS, "self_contained"),
        (QUESTION_COMPLETION, "completion_observed"),
    ] {
        let mut answer = response(
            &plan.comparisons[0],
            "conflict",
            "self_contained",
            "completion_observed",
        );
        set_probability(&mut answer, question, option, 0.87);
        let result = reduce_assessment(
            &plan,
            &BTreeMap::from([(answer.work_item_id.clone(), answer)]),
            true,
        );
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.findings[0].certainty, FindingCertainty::Possible);
    }
}

#[test]
fn stronger_overlapping_finding_keeps_its_probabilities_and_context_together() {
    let mut plan = assessment();
    let mut stronger = plan.comparisons[0].clone();
    stronger.id = "second-range".to_owned();
    stronger.context.clear();
    let mut weaker = response(
        &plan.comparisons[0],
        "conflict",
        "self_contained",
        "not_completion_obligation",
    );
    set_probability(&mut weaker, QUESTION_RELATIONSHIP, "conflict", 0.87);
    let mut stronger_answer = response(
        &stronger,
        "conflict",
        "self_contained",
        "not_completion_obligation",
    );
    set_probability(
        &mut stronger_answer,
        QUESTION_RELATIONSHIP,
        "conflict",
        0.99,
    );
    plan.comparisons.push(stronger.clone());
    let answers = BTreeMap::from([
        (weaker.work_item_id.clone(), weaker),
        (stronger_answer.work_item_id.clone(), stronger_answer),
    ]);
    let result = reduce_assessment(&plan, &answers, true);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].certainty, FindingCertainty::Likely);
    assert_eq!(result.findings[0].conflict_probability, 0.99);
    assert_eq!(result.findings[0].reference, stronger.reference);
    plan.comparisons.reverse();
    assert_eq!(
        reduce_assessment(&plan, &answers, true).findings,
        result.findings
    );
}
