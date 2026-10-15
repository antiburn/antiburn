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
    let bindings_correct = failure.is_none() && complete && observed == case.expected;
    let outcome_correct = outcome == case.expected_outcome;
    let correct = bindings_correct && outcome_correct;
    json!({"id":case.id,"reason":case.reason,"family":case.family,"expected":case.expected,"expected_outcome":case.expected_outcome,
        "expected_semantic":case.expected_semantic,"expected_bindings":case.expected_bindings,"policy_admission":case.policy_admission,
        "observed":observed,"outcome":outcome,"clean":clean,"correct":correct,"bindings_correct":bindings_correct,"outcome_correct":outcome_correct,"authority":case.authority,
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
    let mut unsupported = 0;
    let mut policy_limited = 0;
    let mut wrong_findings = 0;
    let mut wrong_bindings = 0;
    let mut missed_supported_positives = 0;
    let mut unsupported_clean = 0;
    let mut unsupported_publications = 0;
    for case in cases {
        let scheduled_here = reason.is_none_or(|reason| case.reason == reason);
        let mut remaining = case
            .expected
            .iter()
            .filter(|finding| reason.is_none_or(|reason| finding.reason == reason))
            .cloned()
            .collect::<Vec<_>>();
        positives += remaining.len();
        unsupported += usize::from(scheduled_here && case.policy_admission == "unsupported");
        policy_limited += usize::from(scheduled_here && case.policy_admission == "policy_limited");
        let Some(row) = rows.iter().find(|row| row["id"] == case.id) else {
            if case.policy_admission == "admitted" {
                missed_supported_positives += remaining.len();
            }
            missing += usize::from(scheduled_here);
            failures += usize::from(scheduled_here);
            continue;
        };
        let observed: Vec<ExpectedFinding> = serde_json::from_value(row["observed"].clone())
            .expect("Scoring row contains typed observed findings");
        unsupported_publications += usize::from(
            scheduled_here && case.policy_admission != "admitted" && !observed.is_empty(),
        );
        let mut unmatched = Vec::new();
        for finding in observed
            .into_iter()
            .filter(|finding| reason.is_none_or(|reason| finding.reason == reason))
        {
            publications += 1;
            if let Some(index) = remaining.iter().position(|expected| *expected == finding) {
                remaining.remove(index);
                tp += 1;
            } else {
                unmatched.push(finding);
            }
        }
        for finding in unmatched {
            if let Some(index) = remaining
                .iter()
                .position(|expected| expected.reason == finding.reason)
            {
                remaining.remove(index);
                wrong_bindings += 1;
            } else {
                wrong_findings += 1;
            }
        }
        missed_supported_positives += if case.policy_admission == "admitted" {
            remaining.len()
        } else {
            0
        };
        unsupported_clean +=
            usize::from(scheduled_here && row["clean"] == true && case.expected_outcome != "clean");
        abstentions += usize::from(scheduled_here && row["outcome"] == "abstention");
        clean += usize::from(scheduled_here && row["clean"] == true);
        failures +=
            usize::from(scheduled_here && (!row["failure"].is_null() || row["complete"] != true));
    }
    let precision = ratio(tp, publications).as_f64();
    let recall = ratio(tp, positives).as_f64();
    json!({"scheduled":cases.iter().filter(|case|reason.is_none_or(|reason|case.reason==reason)).count(),"expected_findings":positives,"published_findings":publications,"joint_true_positives":tp,
        "joint_false_positives":publications-tp,"joint_false_negatives":positives-tp,"precision":precision,"recall":recall,
        "missing":missing,"failures":failures,"abstentions":abstentions,"clean":clean,"unsupported":unsupported,"policy_limited":policy_limited,
        "wrong_findings":wrong_findings,"wrong_bindings":wrong_bindings,"missed_supported_positives":missed_supported_positives,"unsupported_clean":unsupported_clean,
        "unsupported_publications":unsupported_publications,
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
    assert_eq!(result["overall"]["wrong_bindings"], 1);
    assert_eq!(result["overall"]["wrong_findings"], 0);
}

#[test]
fn wrong_findings_missing_supported_positives_and_unsupported_clean_are_separate() {
    let cases = super::compact::cases();
    let positive = cases
        .iter()
        .find(|case| case.id == "compact-large-repeat")
        .unwrap()
        .clone();
    let negative = cases
        .iter()
        .find(|case| case.id == "compact-explicit-verification")
        .unwrap()
        .clone();
    let mut unsupported = cases
        .into_iter()
        .find(|case| case.id == "compact-generic-header")
        .unwrap();
    unsupported.policy_admission = "policy_limited".into();
    unsupported.expected_outcome = "abstention".into();
    let rows = [
        json!({"id":negative.id,"reason":negative.reason,"observed":negative.expected_bindings,"outcome":"finding","clean":false,"complete":true,"failure":null}),
        json!({"id":unsupported.id,"reason":unsupported.reason,"observed":[],"outcome":"clean","clean":true,"complete":true,"failure":null}),
    ];
    let report = report(&[positive, negative, unsupported], &rows);
    assert_eq!(report["overall"]["wrong_findings"], 1);
    assert_eq!(report["overall"]["wrong_bindings"], 0);
    assert_eq!(report["overall"]["missed_supported_positives"], 1);
    assert_eq!(report["overall"]["unsupported_clean"], 1);
    assert_eq!(report["overall"]["policy_limited"], 1);
}
