use super::fixtures::{Case, Label};
use crate::support::scoring::{inventory_errors, ratio};
use antiburn_local::analysis::jev::JevWorkItem;
use antiburn_local::checks::skill_opportunities::{
    SkillOpportunitiesResult, SkillOpportunityFinding, SkillOpportunityLimit,
    SkillOpportunityOutcome,
};
use serde_json::{Value, json};

pub(crate) fn score(
    case: &Case,
    result: &SkillOpportunitiesResult,
    scheduled: &[JevWorkItem],
    skipped: usize,
) -> Value {
    let exact = result
        .findings
        .iter()
        .filter(|finding| {
            let skill = &finding.comparison.skill;
            skill.matches_definition(&case.skill)
                && finding
                    .comparison
                    .work
                    .iter()
                    .map(|work| &work.reference)
                    .eq(case.work.iter())
                && finding
                    .evidence
                    .iter()
                    .any(|reference| reference.source_id == case.skill.identity)
                && case.work.iter().all(|work| {
                    finding
                        .evidence
                        .iter()
                        .any(|reference| reference.source_id == work.id)
                })
        })
        .count();
    let tp = usize::from(case.label == Label::Advisory && exact == 1);
    let missing = scheduled
        .iter()
        .filter(|item| {
            !result
                .decisions
                .iter()
                .any(|decision| decision.comparison.id == item.id && decision.judgments.is_some())
        })
        .count();
    let abstentions = result
        .decisions
        .iter()
        .filter(|decision| {
            matches!(
                decision.outcome,
                SkillOpportunityOutcome::Unassessed | SkillOpportunityOutcome::Uncertain
            )
        })
        .count();
    let clean = result
        .decisions
        .iter()
        .filter(|decision| decision.outcome == SkillOpportunityOutcome::NoOpportunity)
        .count();
    let unsafe_publications = result.findings.len().saturating_sub(exact);
    let historical_claims = result
        .findings
        .iter()
        .filter(|finding| !has_current_advisory_claim(finding))
        .count();
    json!({"tp":tp,"fp":result.findings.len()-tp,"fn":usize::from(case.label == Label::Advisory)-tp,
        "missing":missing,"failed_cases":0,"abstentions":abstentions,"clean":clean,
        "scheduled":scheduled.len(),"mechanical_skips":skipped,"assessed":scheduled.len()-missing,
        "unsafe_publications":unsafe_publications,"historical_claims":historical_claims,
        "legacy_admission_case":case.authority})
}

fn has_current_advisory_claim(finding: &SkillOpportunityFinding) -> bool {
    let mut expected = "This recommendation uses current skill information and observed work. Unknown use does not establish non-use or historical access.".to_owned();
    let limits = &finding.comparison.limitations;
    if limits.contains(&SkillOpportunityLimit::ReferenceContentPartial) {
        expected.push_str(" Only selected skill reference ranges are available.");
    }
    if limits.contains(&SkillOpportunityLimit::TaskContextPartial)
        || limits.contains(&SkillOpportunityLimit::KnownUseContextPartial)
    {
        expected.push_str(" Task or known-use context is partial.");
    }
    finding.message == "This current skill could help with the observed work."
        && finding.absence_limit == expected
}

pub(crate) fn failed(case: &Case, scheduled: usize, skipped: usize) -> Value {
    json!({"tp":0,"fp":0,"fn":usize::from(case.label == Label::Advisory),"missing":scheduled,
        "failed_cases":1,"abstentions":0,"clean":0,"scheduled":scheduled,"mechanical_skips":skipped,
        "assessed":0,"unsafe_publications":0,"historical_claims":0})
}

pub(crate) fn metrics(cases: &[Case], rows: &[Value]) -> Value {
    let schedule = cases
        .iter()
        .map(|case| json!({"id":case.id}))
        .collect::<Vec<_>>();
    let sum = |key: &str| {
        rows.iter()
            .map(|row| row["score"][key].as_u64().expect("Typed score count"))
            .sum::<u64>()
    };
    let tp = sum("tp");
    let fp = sum("fp");
    let positives = cases
        .iter()
        .filter(|case| case.label == Label::Advisory)
        .count();
    json!({"cases":cases.len(),"executed_cases":rows.len(),"tp":tp,"fp":fp,"fn":positives as u64-tp,
        "precision":ratio(tp as usize,(tp+fp) as usize),"recall":ratio(tp as usize,positives),
        "missing_cases":cases.iter().filter(|case|!rows.iter().any(|row|row["id"] == case.id)).count(),
        "missing_answers":sum("missing"),"failed_cases":rows.iter().filter(|row|!row["failure"].is_null()).count(),
        "abstentions":sum("abstentions"),"clean":sum("clean"),"scheduled_items":sum("scheduled"),
        "mechanical_skips":sum("mechanical_skips"),"unsafe_publications":sum("unsafe_publications"),
        "historical_claims":sum("historical_claims"),"identity_errors":inventory_errors(&schedule,rows)})
}

#[test]
fn missing_positive_cases_remain_in_recall() {
    let cases = super::fixtures::cases("development");
    let result = metrics(&cases, &[]);
    assert_eq!(result["recall"], 0.0);
    assert!(result["fn"].as_u64().unwrap() > 0);
    assert_eq!(result["missing_cases"], cases.len());
}

#[test]
fn missing_history_reaches_the_model_without_removing_positive_labels() {
    use antiburn_local::analysis::jev::JevCheck;
    let mut case = super::fixtures::cases("development")
        .into_iter()
        .find(|case| case.family == "missing_history")
        .unwrap();
    case.label = Label::Advisory;
    let plan = case.check.prepare(&case.check.session_context()).unwrap();
    let result = case.check.reduce(&plan, &[], true).unwrap();
    assert!(!plan.work_items.is_empty());
    let row = json!({"id":case.id,"failure":null,"score":score(&case,&result,&plan.work_items,plan.skipped_item_ids.len())});
    assert_eq!(metrics(&[case], &[row])["recall"], 0.0);
}

#[cfg(test)]
fn saved_choice_result(
    case: &Case,
    probability: f64,
) -> (
    antiburn_local::analysis::jev::JevCheckPlan<
        antiburn_local::checks::skill_opportunities::PreparedSkillOpportunities,
    >,
    SkillOpportunitiesResult,
) {
    use antiburn_local::analysis::jev::{JevAnswer, JevCheck, JevUsage, JevWorkItemResult};
    use std::collections::BTreeMap;
    let plan = case.check.prepare(&case.check.session_context()).unwrap();
    let answers = plan
        .work_items
        .iter()
        .map(|item| {
            let target = plan
                .prepared
                .comparisons
                .iter()
                .find(|comparison| comparison.id == item.id)
                .unwrap()
                .skill
                .identity
                == case.skill.identity;
            let probability = if target { probability } else { 0.52 };
            JevWorkItemResult {
                request_id: item.id.clone(),
                work_item_id: item.id.clone(),
                model: plan.capabilities.model.clone(),
                answers: BTreeMap::from([(
                    "opportunity".into(),
                    JevAnswer::Choice {
                        choice: "useful_opportunity".into(),
                        confidence: if target { 0.67 } else { 0.28 },
                        probabilities: BTreeMap::from([
                            ("useful_opportunity".into(), probability),
                            ("no_opportunity".into(), 0.99 - probability),
                            ("uncertain".into(), 0.01),
                        ]),
                    },
                )]),
                evidence: plan
                    .shared_context
                    .as_ref()
                    .unwrap()
                    .evidence
                    .iter()
                    .chain(&item.window.evidence)
                    .cloned()
                    .collect(),
                usage: JevUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            }
        })
        .collect::<Vec<_>>();
    let result = case.check.reduce(&plan, &answers, true).unwrap();
    (plan, result)
}

#[test]
fn unknown_use_saved_choices_score_a_current_advisory_without_historical_claims() {
    for suite in ["development", "controls"] {
        for case in super::fixtures::cases(suite)
            .into_iter()
            .filter(|case| case.family == "unknown_use")
        {
            assert_eq!(case.label, Label::Advisory);
            let (plan, result) = saved_choice_result(&case, 0.78);
            assert_eq!(result.findings.len(), 1);
            let finding = &result.findings[0];
            assert!(finding.comparison.skill.matches_definition(&case.skill));
            assert!(!finding.comparison.absence_assessable);
            assert!(
                finding
                    .absence_limit
                    .contains("Unknown use does not establish non-use or historical access.")
            );
            assert!(
                finding
                    .absence_limit
                    .ends_with(" Task or known-use context is partial.")
            );
            let scored = score(
                &case,
                &result,
                &plan.work_items,
                plan.skipped_item_ids.len(),
            );
            for (metric, expected) in [
                ("tp", 1),
                ("fp", 0),
                ("fn", 0),
                ("historical_claims", 0),
                ("unsafe_publications", 0),
                ("abstentions", 1),
            ] {
                assert_eq!(scored[metric], expected, "{}: {metric}", case.id);
            }
            let (plan, uncertain) = saved_choice_result(&case, 0.74);
            assert!(uncertain.findings.is_empty());
            assert_eq!(
                score(
                    &case,
                    &uncertain,
                    &plan.work_items,
                    plan.skipped_item_ids.len()
                )["fn"],
                1
            );
        }
    }
}

#[test]
fn unknown_use_does_not_hide_routine_work_or_unrelated_skill_false_positives() {
    for suite in ["development", "controls"] {
        for case in super::fixtures::cases(suite).into_iter().filter(|case| {
            matches!(
                case.family,
                "unknown_use_direct_work" | "unknown_use_irrelevant"
            )
        }) {
            assert_eq!(case.label, Label::NoOpportunity);
            let (plan, result) = saved_choice_result(&case, 0.78);
            let score = score(
                &case,
                &result,
                &plan.work_items,
                plan.skipped_item_ids.len(),
            );
            assert_eq!(score["tp"], 0);
            assert_eq!(score["fp"], 1, "{}", case.id);
            assert_eq!(score["historical_claims"], 0);
        }
    }
}

#[test]
fn claim_contract_accepts_only_typed_context_not_nonuse_or_history_accusations() {
    let case = super::fixtures::cases("development")
        .into_iter()
        .find(|case| case.family == "unknown_use")
        .unwrap();
    let (plan, result) = saved_choice_result(&case, 0.78);
    let original = &result.findings[0];
    for variant in 0..3 {
        let mut changed = result.clone();
        match variant {
            0 => changed.findings[0]
                .absence_limit
                .push_str(" No matching skill was used."),
            1 => {
                changed.findings[0].message =
                    "The agent had this skill available but did not use it.".into()
            }
            _ => changed.findings[0].comparison.limitations.retain(|limit| {
                *limit != SkillOpportunityLimit::KnownUseContextPartial
                    && *limit != SkillOpportunityLimit::TaskContextPartial
            }),
        }
        assert_eq!(
            score(
                &case,
                &changed,
                &plan.work_items,
                plan.skipped_item_ids.len()
            )["historical_claims"],
            1
        );
    }
    let mut wrong_reference = result.clone();
    wrong_reference.findings[0].comparison.skill.description = "Prepare travel itineraries.".into();
    assert_eq!(
        score(
            &case,
            &wrong_reference,
            &plan.work_items,
            plan.skipped_item_ids.len()
        )["unsafe_publications"],
        1
    );
    assert!(has_current_advisory_claim(original));
}
