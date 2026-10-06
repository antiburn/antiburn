use std::collections::BTreeSet;

use serde_json::{Value, json};

const POLICY: &str = include_str!("../data/gates.json");

pub(crate) fn score_v2(inventory: &[Value], rows: &[Value]) -> Value {
    let mut by_id = std::collections::BTreeMap::new();
    let mut joint_passes = 0usize;
    let mut observable_bindings = 0usize;
    let mut recalled_bindings = 0usize;
    let mut published_bindings = 0usize;
    let mut correct_published_bindings = 0usize;
    let mut errors = Vec::new();

    for row in rows {
        let Some(id) = row["id"].as_str() else {
            errors.push(
                json!({"case_id":null,"category":"invalid_result_identity","observed":row["id"]}),
            );
            continue;
        };
        if by_id.contains_key(id) {
            errors.push(
                json!({"case_id":id,"category":"duplicate_result","policy_version":2,
                "response_reference":row["response_reference"],"disposition":"unaccepted"}),
            );
        } else {
            by_id.insert(id, row);
        }
    }
    let mut scheduled_ids = BTreeSet::new();

    for scheduled in inventory {
        let id = scheduled["id"].as_str().unwrap_or("<missing-id>");
        if !scheduled_ids.insert(id) {
            errors.push(
                json!({"case_id":id,"category":"duplicate_scheduled_case","policy_version":2,
                "disposition":"unaccepted"}),
            );
            continue;
        }
        let expected = scheduled["expected"].as_str().unwrap_or("<missing-label>");
        let resolution_available = scheduled["binding_resolution"].is_null()
            || (expected == "finding" && scheduled["binding_resolution"] == "resolved")
            || (expected != "finding" && scheduled["binding_resolution"] == "not_required");
        let labels_available = scheduled["expected_bindings"].is_array()
            && matches!(
                expected,
                "finding" | "no_finding" | "pending" | "unassessed"
            )
            && resolution_available;
        let expected_bindings = string_set(&scheduled["expected_bindings"]);
        let bindings_well_formed =
            scheduled["expected_bindings"]
                .as_array()
                .is_some_and(|bindings| {
                    bindings.iter().all(Value::is_string)
                        && bindings.len() == expected_bindings.len()
                        && (expected != "finding" || !bindings.is_empty())
                });
        if expected == "finding" && scheduled["label_review"].is_null() && labels_available {
            observable_bindings += expected_bindings.len();
        }
        let Some(row) = by_id.get(id) else {
            errors.push(json!({"case_id":id,"category":"missing_result","family":scheduled["family"],
                "source_format":scheduled["source_format"],"fixture_identity":scheduled["fixture_identity"],
                "policy_version":2,"expected":{"outcome":expected,"bindings":expected_bindings},
                "expected_binding_labels_available":labels_available,
                "observed":null,"response_reference":null,"revision":null,
                "cause_hypothesis":"No row was captured for this scheduled case.","disposition":"unaccepted"}));
            continue;
        };
        let observed = row["observed"].as_str().unwrap_or("unassessed");
        let observed_bindings = string_set(&row["observed_bindings"]);
        let observed_bindings_valid = row["observed_bindings"].as_array().is_some_and(|values| {
            values.iter().all(Value::is_string) && values.len() == observed_bindings.len()
        });
        let binding_match = labels_available
            && bindings_well_formed
            && observed_bindings_valid
            && row["citation_binding_integrity"] != false
            && expected_bindings == observed_bindings;
        let outcome_match = row["failure"].is_null() && expected == observed;
        if outcome_match && binding_match {
            joint_passes += 1;
        } else {
            errors.push(json!({"case_id":id,"category":if row["failure"].is_null() {"semantic_mismatch"} else {"provider_or_execution_failure"},
                "family":scheduled["family"],"source_format":scheduled["source_format"],
                "fixture_identity":scheduled["fixture_identity"],"policy_version":2,
                "expected":{"outcome":expected,"bindings":expected_bindings},
                "expected_binding_labels_available":labels_available,
                "observed":{"outcome":observed,"bindings":observed_bindings},
                "failure":row["failure"],"response_reference":row["response_reference"],
                "revision":row["revisions"],"cause_hypothesis":row["cause"],
                "disposition":"unaccepted"}));
        }
        if expected == "finding"
            && scheduled["label_review"].is_null()
            && labels_available
            && row["failure"].is_null()
            && observed_bindings_valid
            && row["citation_binding_integrity"] != false
        {
            recalled_bindings += expected_bindings.intersection(&observed_bindings).count();
        }
        if observed == "finding" {
            published_bindings += observed_bindings.len();
            if expected == "finding"
                && labels_available
                && row["failure"].is_null()
                && observed_bindings_valid
                && row["citation_binding_integrity"] != false
            {
                correct_published_bindings +=
                    observed_bindings.intersection(&expected_bindings).count();
            }
        }
    }
    let expected_ids = scheduled_ids;
    for row in rows {
        let Some(id) = row["id"].as_str() else {
            errors.push(
                json!({"case_id":null,"category":"invalid_result_identity","observed":row["id"]}),
            );
            continue;
        };
        if !expected_ids.contains(id) {
            let bindings = string_set(&row["observed_bindings"]);
            if row["observed"] == "finding" {
                published_bindings += bindings.len();
            }
            errors.push(json!({"case_id":id,"category":"unexpected_result","expected":null,
                "observed":{"outcome":row["observed"],"bindings":bindings},
                "policy_version":2,"response_reference":row["response_reference"],
                "cause_hypothesis":"Result ID is outside the scheduled inventory.","disposition":"unaccepted"}));
        }
    }
    let completed = errors.iter().all(|error| {
        !matches!(
            error["category"].as_str(),
            Some("duplicate_result" | "invalid_result_identity" | "duplicate_scheduled_case")
        )
    }) && expected_ids.iter().all(|id| by_id.contains_key(id))
        && by_id.len() == expected_ids.len();
    let missing_binding_labels = inventory
        .iter()
        .filter(|case| {
            let bindings = &case["expected_bindings"];
            let expected = case["expected"].as_str().unwrap_or("<missing-label>");
            !bindings.as_array().is_some_and(|bindings| {
                let unique = bindings
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<BTreeSet<_>>();
                bindings.iter().all(Value::is_string)
                    && unique.len() == bindings.len()
                    && (expected != "finding" || !bindings.is_empty())
            }) || !matches!(
                expected,
                "finding" | "no_finding" | "pending" | "unassessed"
            ) || (!case["binding_resolution"].is_null()
                && ((expected == "finding" && case["binding_resolution"] != "resolved")
                    || (expected != "finding" && case["binding_resolution"] != "not_required")))
        })
        .count();
    json!({
        "policy_version":2,
        "scheduled_cases":inventory.len(),
        "evaluated_cases":rows.len(),
        "joint_passes":joint_passes,
        "joint_accuracy":ratio(joint_passes, inventory.len()),
        "complete":completed && rows.len() == expected_ids.len(),
        "observable_expected_bindings":observable_bindings,
        "observable_recalled_bindings":recalled_bindings,
        "observable_binding_recall":ratio(recalled_bindings, observable_bindings),
        "cases_missing_exact_binding_labels":missing_binding_labels,
        "published_bindings":published_bindings,
        "correct_published_bindings":correct_published_bindings,
        "binding_precision":if missing_binding_labels == 0 { ratio(correct_published_bindings, published_bindings) } else { Value::Null },
        "errors":errors
    })
}

pub(crate) fn passes_v2(score: &Value) -> bool {
    let policy: Value = serde_json::from_str(POLICY).expect("valid v2 evaluation policy");
    let thresholds = &policy["gates"];
    let joint = (thresholds["joint_case_accuracy"]
        .as_f64()
        .expect("policy joint accuracy gate is numeric")
        * 100.0) as u64;
    let recall = (thresholds["observable_binding_recall"]
        .as_f64()
        .expect("policy binding recall gate is numeric")
        * 100.0) as u64;
    let precision = (thresholds["binding_precision"]
        .as_f64()
        .expect("policy binding precision gate is numeric")
        * 100.0) as u64;
    let scheduled = score["scheduled_cases"].as_u64().unwrap_or(0);
    let bindings = score["observable_expected_bindings"].as_u64().unwrap_or(0);
    let published = score["published_bindings"].as_u64().unwrap_or(0);
    score["complete"] == true
        && scheduled > 0
        && bindings > 0
        && published > 0
        && score["cases_missing_exact_binding_labels"] == 0
        && score["joint_passes"].as_u64().unwrap_or(0) * 100 >= scheduled * joint
        && score["observable_recalled_bindings"].as_u64().unwrap_or(0) * 100 >= bindings * recall
        && score["correct_published_bindings"].as_u64().unwrap_or(0) * 100 >= published * precision
}

fn string_set(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn ratio(numerator: usize, denominator: usize) -> Value {
    if denominator == 0 {
        Value::Null
    } else {
        json!(numerator as f64 / denominator as f64)
    }
}
