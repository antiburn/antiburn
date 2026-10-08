use super::fixtures::{Case, ExpectedFinding, REASONS};
use crate::support::scoring::{inventory_errors, ratio};
use antiburn_local::checks::over_exploring::{Assessment, Reason};
use serde_json::{Value, json};

pub fn row(
    case: &Case,
    result: Option<&Assessment>,
    complete: bool,
    failure: Option<String>,
) -> Value {
    let observed: Vec<ExpectedFinding> = result
        .map(|result| {
            result
                .findings
                .iter()
                .map(|finding| ExpectedFinding {
                    reason: finding.reason,
                    reads: finding.reads.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    let clean = complete
        && result.is_some_and(|result| {
            !result.completed_work_item_ids.is_empty()
                && result.coverage.not_selected_items == 0
                && result.completed_work_item_ids.len() == result.coverage.selected_items
                && result.unassessed.is_empty()
                && result.findings.is_empty()
        });
    let outcome = if !observed.is_empty() {
        "finding"
    } else if clean {
        "clean"
    } else {
        "abstention"
    };
    let correct = failure.is_none()
        && complete
        && outcome == case.expected_outcome
        && observed == case.expected;
    json!({"id":case.id,"reason":case.reason,"family":case.family,"expected":case.expected,"expected_outcome":case.expected_outcome,
        "observed":observed,"outcome":outcome,"clean":clean,"correct":correct,"authority":case.authority,
        "unsafe_authority_publication":case.authority && (!observed.is_empty() || clean) && !correct,
        "complete":complete,"failure":failure,"limitations":result.map(|result| &result.unassessed)})
}

fn metrics(cases: &[Case], rows: &[Value], reason: Option<Reason>) -> Value {
    let mut positives = 0;
    let mut publications = 0;
    let mut tp = 0;
    let mut missing = 0;
    let mut abstentions = 0;
    let mut clean = 0;
    let mut failures = 0;
    for case in cases {
        let scheduled_here = reason.is_none_or(|reason| case.reason == reason);
        let mut remaining = case
            .expected
            .iter()
            .filter(|finding| reason.is_none_or(|reason| finding.reason == reason))
            .cloned()
            .collect::<Vec<_>>();
        positives += remaining.len();
        let Some(row) = rows.iter().find(|row| row["id"] == case.id) else {
            missing += usize::from(scheduled_here);
            failures += usize::from(scheduled_here);
            continue;
        };
        let observed: Vec<ExpectedFinding> = serde_json::from_value(row["observed"].clone())
            .expect("Scoring row contains typed observed findings");
        for finding in observed
            .into_iter()
            .filter(|finding| reason.is_none_or(|reason| finding.reason == reason))
        {
            publications += 1;
            if let Some(index) = remaining.iter().position(|expected| *expected == finding) {
                remaining.remove(index);
                tp += 1;
            }
        }
        abstentions += usize::from(scheduled_here && row["outcome"] == "abstention");
        clean += usize::from(scheduled_here && row["clean"] == true);
        failures +=
            usize::from(scheduled_here && (!row["failure"].is_null() || row["complete"] != true));
    }
    let precision = ratio(tp, publications).as_f64();
    let recall = ratio(tp, positives).as_f64();
    json!({"scheduled":cases.iter().filter(|case|reason.is_none_or(|reason|case.reason==reason)).count(),"expected_findings":positives,"published_findings":publications,"joint_true_positives":tp,
        "joint_false_positives":publications-tp,"joint_false_negatives":positives-tp,"precision":precision,"recall":recall,
        "missing":missing,"failures":failures,"abstentions":abstentions,"clean":clean,
        "executed":rows.iter().filter(|row| reason.is_none_or(|reason| row["reason"] == serde_json::to_value(reason).expect("Typed reason serializes"))).count()})
}

pub fn report(cases: &[Case], rows: &[Value]) -> Value {
    let schedule = cases
        .iter()
        .map(|case| json!({"id":case.id}))
        .collect::<Vec<_>>();
    let identity_errors = inventory_errors(&schedule, rows);
    let overall = metrics(cases, rows, None);
    let mut reasons = serde_json::Map::new();
    for reason in REASONS {
        let name = serde_json::to_value(reason)
            .expect("Typed reason serializes")
            .as_str()
            .expect("Reason serializes as a string")
            .to_owned();
        reasons.insert(name, metrics(cases, rows, Some(reason)));
    }
    let unsafe_publications = rows
        .iter()
        .filter(|row| row["unsafe_authority_publication"] == true)
        .count();
    let authority_missing = cases
        .iter()
        .filter(|case| case.authority && !rows.iter().any(|row| row["id"] == case.id))
        .count();
    json!({"overall":overall,"reasons":reasons,"unsafe_authority_publications":unsafe_publications,"missing_authority_cases":authority_missing,"identity_errors":identity_errors,
        "uncertainty":"One actual execution per synthetic case. These counts do not establish population accuracy. Templated cases are correlated; no independence claim.","rows":rows})
}

#[test]
fn a_bound_positive_survives_a_sibling_provider_failure_in_metrics() {
    let case = super::fixtures::cases("development").remove(0);
    let row = json!({"id":case.id,"reason":case.reason,"observed":case.expected,"outcome":"finding","clean":false,"complete":false,"failure":"provider_unavailable"});
    let result = report(&[case], &[row]);
    assert_eq!(result["overall"]["joint_true_positives"], 1);
    assert_eq!(result["overall"]["joint_false_positives"], 0);
    assert_eq!(result["overall"]["failures"], 1);
}

#[test]
fn a_wrong_read_binding_is_not_a_true_positive() {
    let case = super::fixtures::cases("development").remove(0);
    let mut observed = case.expected.clone();
    observed[0].reads[0].output_digest = Some("wrong-binding".into());
    let row = json!({"id":case.id,"reason":case.reason,"observed":observed,"outcome":"finding","clean":false,"complete":true,"failure":null});
    let result = report(&[case], &[row]);
    assert_eq!(result["overall"]["joint_true_positives"], 0);
    assert_eq!(result["overall"]["joint_false_positives"], 1);
}
