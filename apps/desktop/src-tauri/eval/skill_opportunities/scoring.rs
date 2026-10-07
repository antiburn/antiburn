use super::fixtures::{Case, Label};
use crate::support::scoring::{inventory_errors, ratio};
use antiburn_local::analysis::jev::JevWorkItem;
use antiburn_local::checks::skill_opportunities::{
    SkillOpportunitiesResult, SkillOpportunityOutcome,
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
            skill.identity == case.skill.identity
                && skill.definition_revision == case.skill.revision
                && skill.name == case.skill.name
                && skill.description == case.skill.description
                && skill.created_at_ms == case.skill.created_at_ms
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
        .filter(|decision| decision.outcome == SkillOpportunityOutcome::Unassessed)
        .count();
    let clean = result
        .decisions
        .iter()
        .filter(|decision| decision.outcome == SkillOpportunityOutcome::NoOpportunity)
        .count();
    let unsafe_publications = if case.authority {
        result.findings.len()
            + if case.label == Label::Abstain {
                clean
            } else {
                0
            }
    } else {
        0
    };
    let historical_claims = result.findings.iter().filter(|finding| finding.message != "This work matches a skill you have installed."
        || finding.absence_limit != "No matching use is recorded in the selected evidence. Other session use and historical access are not established.").count();
    json!({"tp":tp,"fp":result.findings.len()-tp,"fn":usize::from(case.label == Label::Advisory)-tp,
        "missing":missing,"failed_cases":0,"abstentions":abstentions,"clean":clean,
        "scheduled":scheduled.len(),"mechanical_skips":skipped,"assessed":scheduled.len()-missing,
        "unsafe_publications":unsafe_publications,"historical_claims":historical_claims})
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
fn mechanical_skips_do_not_remove_positive_labels() {
    use antiburn_local::analysis::jev::JevCheck;
    let mut case = super::fixtures::cases("development")
        .into_iter()
        .find(|case| case.family == "missing_history")
        .unwrap();
    case.label = Label::Advisory;
    let plan = case.check.prepare(&case.check.session_context()).unwrap();
    let result = case.check.reduce(&plan, &[], true).unwrap();
    assert!(plan.work_items.is_empty());
    let row = json!({"id":case.id,"failure":null,"score":score(&case,&result,&plan.work_items,plan.skipped_item_ids.len())});
    assert_eq!(metrics(&[case], &[row])["recall"], 0.0);
}
