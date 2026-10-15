use super::super::tests::{event, input};
use super::*;
use crate::analysis::jev::JevUsage;

fn command_action(id: &str, command: &str) -> crate::checks::ignored_instructions::ContentAction {
    let mut action = event(id, 10, "assistant", "main", command);
    action.kind = "tool_input".into();
    action.tool_name = Some("Bash".into());
    action.tool_call_id = Some("push".into());
    action.normalized_fields = Some(crate::analysis::jev::JevNormalizedFields {
        category: Some(crate::analysis::jev::JevNormalizedCategory::BashCommand),
        values: [(
            crate::analysis::jev::JevInputField::BashCommandInput,
            command.to_owned(),
        )]
        .into(),
        malformed: false,
    });
    action
}

fn assessment() -> AssessmentPlan {
    let action = command_action("report", "git push --force");
    let plan = build_assessment_plan(input(vec![action], "Never run `git push --force`."));
    assert_eq!(plan.comparisons.len(), 1, "{plan:#?}");
    plan
}

fn response(comparison: &CandidateComparison, choice: &str, probability: f64) -> JevWorkItemResult {
    JevWorkItemResult {
        request_id: "synthetic-request".to_owned(),
        work_item_id: comparison.id.clone(),
        answers: BTreeMap::from([(
            QUESTION_DECISION.to_owned(),
            JevAnswer::Choice {
                choice: choice.to_owned(),
                probabilities: ["conflict", "no_conflict", "uncertain"]
                    .into_iter()
                    .map(|option| {
                        (
                            option.to_owned(),
                            if option == choice {
                                probability
                            } else {
                                (1.0 - probability) / 2.0
                            },
                        )
                    })
                    .collect(),
                confidence: probability,
            },
        )]),
        evidence: Vec::new(),
        model: ASSESSMENT_MODEL.to_owned(),
        usage: JevUsage {
            input_tokens: 10,
            output_tokens: 2,
        },
    }
}

fn reduce(plan: &AssessmentPlan, choice: &str, probability: f64) -> AssessmentResult {
    let comparison = &plan.comparisons[0];
    reduce_assessment(
        plan,
        &BTreeMap::from([(
            comparison.id.clone(),
            response(comparison, choice, probability),
        )]),
        true,
    )
}

#[test]
fn composite_probability_has_one_publication_threshold() {
    let plan = assessment();
    for probability in [0.94, 0.95, 0.98] {
        let result = reduce(&plan, "conflict", probability);
        assert_eq!(result.findings.len(), usize::from(probability >= 0.95));
        if let Some(finding) = result.findings.first() {
            assert_eq!(finding.composite_probability, probability);
            let json = serde_json::to_value(finding).unwrap();
            assert!(json.get("applicability_probability").is_none());
            assert!(json.get("evidence_basis_probability").is_none());
            assert!(json.get("conflict_probability").is_none());
        }
        assert_eq!(
            result.coverage.reassessed_comparison_ids,
            [plan.comparisons[0].id.clone()]
        );
    }
}

#[test]
fn probability_one_cannot_publish_missing_or_invalid_source_bindings() {
    for mutation in 0..5 {
        let mut plan = assessment();
        let comparison = &mut plan.comparisons[0];
        match mutation {
            0 => comparison.source_binding = None,
            1 => comparison.source_binding.as_mut().unwrap().content_digest = "changed".to_owned(),
            2 => comparison.reference.scope = InstructionScope::Unknown,
            3 => comparison.action.truncated = true,
            _ => comparison.instruction_context[0].context_clipped = true,
        }
        let result = reduce(&plan, "conflict", 1.0);
        assert!(result.findings.is_empty(), "{mutation}");
        assert_eq!(result.unassessed_comparisons.len(), 1, "{mutation}");
        assert!(result.coverage.reassessed_finding_ids.is_empty());
    }
}

#[test]
fn a_finding_cannot_keep_an_invalid_decision_record() {
    let result = reduce(&assessment(), "conflict", 1.0);
    assert_eq!(result.findings.len(), 1);
    let finding = &result.findings[0];
    assert!(finding.decision_record().is_some());
    let mut missing = finding.clone();
    missing.decision = None;
    assert!(missing.decision_record().is_none());
    let mut changed = finding.clone();
    changed.decision.as_mut().unwrap().action_anchor.source.id = "unbound".to_owned();
    assert!(changed.decision_record().is_none());
}

#[test]
fn low_probability_no_issue_resolves_only_its_target_on_partial_input() {
    let mut plan = assessment();
    plan.complete_input = false;
    plan.comparisons[0].prior_history_complete = false;
    let resolved = plan.comparisons[0].clone();
    let mut missing = resolved.clone();
    missing.id = "missing-pair".to_owned();
    missing.reference.rule_id = "missing-rule".to_owned();
    plan.comparisons.push(missing.clone());
    let result = reduce_assessment(
        &plan,
        &BTreeMap::from([(
            resolved.id.clone(),
            response(&resolved, "no_conflict", 0.53),
        )]),
        true,
    );
    assert!(result.findings.is_empty());
    assert!(result.pending_rules.is_empty());
    assert_eq!(
        result.coverage.reassessed_comparison_ids,
        std::slice::from_ref(&resolved.id)
    );
    assert_eq!(
        result.coverage.reassessed_rule_ids,
        std::slice::from_ref(&resolved.reference.rule_id)
    );
    assert_eq!(
        result.coverage.reassessed_finding_ids,
        [finding_id_for_reference(&resolved.reference)]
    );
    assert_eq!(result.unassessed_comparisons, [missing.id]);
    assert!(
        result
            .coverage
            .limitations
            .contains(&"source_evidence_is_partial".to_owned())
    );
    assert!(
        result
            .coverage
            .limitations
            .contains(&"some_comparisons_unassessed".to_owned())
    );
    assert!(
        !result
            .coverage
            .limitations
            .contains(&"semantic_decision_uncertain".to_owned())
    );
}

#[test]
fn legacy_pending_completion_answer_is_rejected() {
    let plan = assessment();
    let result = reduce(&plan, "pending_completion", 0.40);
    assert!(result.findings.is_empty());
    assert_eq!(
        result.unassessed_comparisons,
        [plan.comparisons[0].id.clone()]
    );
    assert!(result.pending_rules.is_empty());
    assert!(result.coverage.reassessed_comparison_ids.is_empty());
    assert!(result.coverage.reassessed_finding_ids.is_empty());
    assert!(!result.coverage.processing_limit_reached);
    assert!(
        !result
            .coverage
            .limitations
            .contains(&"semantic_decision_uncertain".to_owned())
    );
}

#[test]
fn uncertain_answers_are_processed_without_resolving_the_comparison() {
    let plan = assessment();
    for probability in [0.97, 0.40] {
        let result = reduce(&plan, "uncertain", probability);
        assert!(result.findings.is_empty());
        assert_eq!(
            result.coverage.reassessed_comparison_ids,
            [plan.comparisons[0].id.clone()]
        );
        assert!(result.pending_rules.is_empty());
        assert_eq!(
            result.unassessed_comparisons,
            [plan.comparisons[0].id.clone()]
        );
        assert!(result.coverage.reassessed_finding_ids.is_empty());
        assert!(!result.coverage.processing_limit_reached);
    }
}

#[test]
fn missing_and_invalid_answers_are_not_processed() {
    let plan = assessment();
    let comparison = &plan.comparisons[0];
    for invalid in [
        "missing",
        "old_schema",
        "unknown_choice",
        "nan",
        "missing_option",
        "bad_sum",
    ] {
        let mut answer = response(comparison, "conflict", 0.97);
        match invalid {
            "missing" => answer.answers.clear(),
            "old_schema" => {
                let choice = answer.answers.remove(QUESTION_DECISION).unwrap();
                answer.answers.insert("relationship".to_owned(), choice);
            }
            _ => {
                let JevAnswer::Choice {
                    choice,
                    probabilities,
                    ..
                } = answer.answers.get_mut(QUESTION_DECISION).unwrap()
                else {
                    unreachable!()
                };
                match invalid {
                    "unknown_choice" => *choice = "applies".to_owned(),
                    "nan" => {
                        probabilities.insert("conflict".to_owned(), f64::NAN);
                    }
                    "missing_option" => {
                        probabilities.remove("uncertain");
                    }
                    "bad_sum" => {
                        probabilities.insert("conflict".to_owned(), 0.5);
                    }
                    _ => unreachable!(),
                }
            }
        }
        let result = reduce_assessment(&plan, &[(comparison.id.clone(), answer)].into(), true);
        assert!(result.findings.is_empty(), "{invalid}");
        assert!(
            result.coverage.reassessed_comparison_ids.is_empty(),
            "{invalid}"
        );
        assert_eq!(
            result.unassessed_comparisons,
            std::slice::from_ref(&comparison.id)
        );
    }
}

#[test]
fn partial_input_supports_direct_advisory_without_exact_history() {
    let action = command_action("report", "git push --force");
    let mut source = input(vec![action], "Never run `git push --force`.");
    source.content.complete = false;
    source.prior_history_complete = false;
    let plan = build_assessment_plan(source);
    let result = reduce(&plan, "conflict", 0.97);
    assert_eq!(result.findings.len(), 1);
    assert!(result.findings[0].decision_record().is_some());
    assert!(
        result.findings[0]
            .limitations
            .contains(&"prior_history_incomplete".to_owned())
    );
    assert!(
        result.findings[0]
            .limitations
            .contains(&"source_evidence_is_partial".to_owned())
    );
}

#[test]
fn incomplete_episode_does_not_assert_an_unseen_prerequisite_absence() {
    let source = input(
        vec![event(
            "publish",
            2,
            "assistant",
            "main",
            "Reported skipping the second check.",
        )],
        "Request both checks before publication.",
    );
    let plan = build_assessment_plan(source);
    assert!(plan.comparisons.is_empty());
    let result = reduce_assessment(&plan, &BTreeMap::new(), true);
    assert!(result.findings.is_empty());
}

#[test]
fn observed_completion_keeps_the_exact_local_rule_and_action_binding() {
    let plan = assessment();
    let result = reduce(&plan, "conflict", 0.97);
    let finding = &result.findings[0];
    assert_eq!(finding.reference, plan.comparisons[0].reference);
    assert_eq!(
        finding.instruction_excerpt,
        rule_text_fragment(&plan.comparisons[0])
    );
    assert_eq!(finding.action_excerpt, plan.comparisons[0].action.text);
    let saved = serde_json::to_value(finding).unwrap();
    let restored: AssessmentFinding = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(&restored, finding);
    assert!(restored.decision_record().is_some(), "{:#?}", restored);
    let mut mismatch = restored;
    mismatch.reference.action_id = "different-anchor".to_owned();
    assert!(mismatch.decision_record().is_none());
    let mut legacy = saved;
    legacy.as_object_mut().unwrap().remove("decision");
    let legacy: AssessmentFinding = serde_json::from_value(legacy).unwrap();
    assert!(legacy.decision.is_none());
}

#[test]
fn complete_records_do_not_override_semantic_uncertainty() {
    let plan = assessment();
    assert!(plan.complete_input);
    let result = reduce(&plan, "uncertain", 0.97);
    assert!(result.findings.is_empty());
    assert_eq!(
        result.unassessed_comparisons,
        [plan.comparisons[0].id.clone()]
    );
    assert!(result.coverage.reassessed_finding_ids.is_empty());
}

#[test]
fn direct_no_issue_can_use_partial_rule_and_action_ranges() {
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
    assert!(plan.comparisons.is_empty());
    plan.complete_input = false;
    let result = reduce_assessment(&plan, &BTreeMap::new(), true);
    assert!(result.findings.is_empty());
    assert!(!result.coverage.limitations.is_empty());
}

#[test]
fn stronger_overlapping_finding_keeps_probability_and_context_together() {
    let mut plan = assessment();
    let mut stronger = plan.comparisons[0].clone();
    stronger.id = "second-range".to_owned();
    stronger.context.clear();
    let weaker_answer = response(&plan.comparisons[0], "conflict", 0.80);
    let stronger_answer = response(&stronger, "conflict", 0.99);
    plan.comparisons.push(stronger.clone());
    let answers = BTreeMap::from([
        (weaker_answer.work_item_id.clone(), weaker_answer),
        (stronger_answer.work_item_id.clone(), stronger_answer),
    ]);
    let result = reduce_assessment(&plan, &answers, true);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].composite_probability, 0.99);
    assert_eq!(result.findings[0].reference, stronger.reference);
    plan.comparisons.reverse();
    assert_eq!(
        reduce_assessment(&plan, &answers, true).findings,
        result.findings
    );
}

#[test]
fn current_file_advisory_keeps_its_provenance_limit() {
    let mut plan = assessment();
    plan.comparisons[0].reference.provenance = InstructionProvenance::CurrentFileComparison;
    let result = reduce(&plan, "conflict", 0.97);
    assert_eq!(result.findings[0].certainty, FindingCertainty::Possible);
    assert!(
        result.findings[0]
            .limitations
            .contains(&"current_file_not_historical_proof".to_owned())
    );
}

#[test]
fn shared_requests_count_usage_once() {
    let mut plan = assessment();
    let mut second = plan.comparisons[0].clone();
    second.id = "second-pair".to_owned();
    let first = response(&plan.comparisons[0], "no_conflict", 0.97);
    let second_answer = response(&second, "no_conflict", 0.97);
    plan.comparisons.push(second);
    let result = reduce_assessment(
        &plan,
        &[
            (first.work_item_id.clone(), first),
            (second_answer.work_item_id.clone(), second_answer),
        ]
        .into(),
        true,
    );
    assert_eq!(result.request_count, 1);
    assert_eq!(result.input_tokens, 10);
    assert_eq!(result.output_tokens, 2);
}
