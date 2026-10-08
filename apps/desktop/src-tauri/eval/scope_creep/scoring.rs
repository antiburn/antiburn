use crate::support::scoring::{inventory_errors, ratio};
use serde_json::{Value, json};

pub fn score(schedule: &[Value], rows: &[Value]) -> Value {
    let mut positives = 0;
    let mut publications = 0;
    let mut true_positives = 0;
    let mut missing = 0;
    let mut unsafe_authority = 0;
    let mut abstentions = 0;
    let mut correct_clean = 0;
    let mut failures = 0;
    for case in schedule {
        positives += usize::from(case["expected"] == "finding");
        let matches: Vec<_> = rows.iter().filter(|row| row["id"] == case["id"]).collect();
        if matches.len() != 1 {
            missing += 1;
            continue;
        }
        let row = matches[0];
        publications += row["finding_count"].as_u64().unwrap_or(0) as usize;
        let exact = row["binding_exact"] == true;
        true_positives += usize::from(
            case["expected"] == "finding"
                && row["observed"] == "finding"
                && exact
                && row["failure"].is_null(),
        );
        correct_clean += usize::from(
            case["expected"] == "clean"
                && row["observed"] == "clean"
                && row["decision_binding_exact"] == true
                && row["failure"].is_null(),
        );
        missing += row["missing_scheduled_answers"].as_u64().unwrap_or(0) as usize;
        failures += usize::from(!row["failure"].is_null());
        abstentions += usize::from(row["observed"] == "unassessed");
        unsafe_authority +=
            usize::from(case["authority_fixture"] == true && row["observed"] != "unassessed");
    }
    let precision = ratio(true_positives, publications);
    let recall = ratio(true_positives, positives);
    let identity_errors = inventory_errors(schedule, rows);
    json!({
          "identity_errors":identity_errors,
        "joint_precision":precision,"joint_recall":recall,"true_positives":true_positives,"expected_positives":positives,
        "finding_publications":publications,"correct_clean":correct_clean,"abstentions":abstentions,
        "unsafe_authority_publications":unsafe_authority,"missing_scheduled_assessments":missing,"failed_cases":failures,
         "scheduled_cases":schedule.len(),"executed_cases":rows.len(),
        "uncertainty":"One run per synthetic case. Shared adapter shapes are conformance controls. This is not a population accuracy estimate."})
}

#[test]
fn wrong_bindings_missing_cases_and_abstentions_cannot_pass_recall() {
    let schedule = vec![
        json!({"id":"p","expected":"finding","eligible":true}),
        json!({"id":"q","expected":"finding","eligible":true}),
    ];
    let row = json!({"id":"p","observed":"finding","finding_count":1,"binding_exact":false,"failure":null,"missing_scheduled_answers":0});
    let result = score(&schedule, &[row]);
    assert_eq!(result["joint_precision"], 0.0);
    assert_eq!(result["joint_recall"], 0.0);
    assert_eq!(result["missing_scheduled_assessments"], 1);
    let abstention = json!({"id":"p","observed":"unassessed","finding_count":0,"failure":null,"missing_scheduled_answers":0});
    assert_eq!(score(&schedule, &[abstention])["joint_recall"], 0.0);
}

#[test]
fn unsupported_clean_is_an_unsafe_authority_publication() {
    let schedule = vec![json!({"id":"a","expected":"unassessed","authority_fixture":true})];
    let row = json!({"id":"a","observed":"clean","finding_count":0,"failure":null,"missing_scheduled_answers":0});
    assert_eq!(score(&schedule, &[row])["unsafe_authority_publications"], 1);
}
