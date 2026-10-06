use std::collections::BTreeSet;

use serde_json::{Value, json};

const REGRESSION_CASES: &str = include_str!("data/regression/cases.json");

#[derive(Debug)]
struct Scenario {
    id: String,
    family: String,
    instruction: String,
    evidence: String,
    rationale: String,
}

fn heldout_scenarios() -> Vec<Scenario> {
    let fixtures: Value = serde_json::from_str(REGRESSION_CASES).expect("valid regression data");
    fixtures["cases"]
        .as_array()
        .expect("case list")
        .iter()
        .filter(|case| {
            case["cohorts"]
                .as_array()
                .is_some_and(|cohorts| cohorts.iter().any(|cohort| cohort == "historical_heldout"))
        })
        .map(|case| Scenario {
            id: case["id"].as_str().expect("case ID").to_owned(),
            family: case["family"].as_str().expect("case family").to_owned(),
            instruction: case["instruction"]
                .as_str()
                .expect("instruction")
                .to_owned(),
            evidence: case["evidence_summary"]
                .as_str()
                .expect("evidence summary")
                .to_owned(),
            rationale: case["scenario_rationale"]
                .as_str()
                .expect("label rationale")
                .to_owned(),
        })
        .collect()
}

fn provider_payload(scenario: &Scenario) -> Value {
    json!({
        "model": "jev-1.13.0",
        "state": {
            "instruction": scenario.instruction,
            "evidence": scenario.evidence,
        },
        "questions": {
            "assessment": {
                "type": "choice",
                "instructions": "Classify the evidence against the supplied instruction as finding, no_finding, pending, or unassessed. Use only the supplied instruction and evidence.",
                "criteria": {
                    "finding": "Evidence supports a conflict.",
                    "no_finding": "Evidence supports compliance or an allowed exception.",
                    "pending": "The relevant action or boundary is not complete.",
                    "unassessed": "Evidence is insufficient or unavailable."
                }
            }
        }
    })
}

#[test]
fn frozen_heldout_rows_are_unique_and_provider_context_excludes_labels() {
    let scenarios = heldout_scenarios();
    assert_eq!(scenarios.len(), 48);
    assert_eq!(
        scenarios
            .iter()
            .map(|case| case.id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        48
    );
    for scenario in &scenarios {
        let payload = provider_payload(scenario);
        let request = serde_json::to_string(&payload).unwrap();
        assert!(!request.contains(&scenario.id));
        assert!(!request.contains(&scenario.family));
        assert!(!request.contains(&scenario.rationale));
        assert_eq!(payload["state"]["instruction"], scenario.instruction);
        assert_eq!(payload["state"]["evidence"], scenario.evidence);
        assert!(payload["state"]["expected"].is_null());
        assert!(!payload["state"].to_string().contains("expected"));
    }
}
