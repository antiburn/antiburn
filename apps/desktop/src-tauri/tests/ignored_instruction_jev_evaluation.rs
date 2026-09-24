use std::time::Duration;

use serde_json::{Map, Value, json};

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const MODEL: &str = "jev-1.13.0";
const CASES: &str = include_str!(
    "../../../../crates/antiburn-local/tests/fixtures/ignored_instructions/cases.json"
);
const MINIMUM_ACCURACY: usize = 6;

#[test]
#[ignore = "makes one billable TypeSafe request using synthetic labeled evidence"]
fn jev_evaluates_synthetic_ignored_instruction_cases_without_running_the_app() {
    let api_key = std::env::var("TYPESAFE_API_KEY")
        .expect("set TYPESAFE_API_KEY to an authorized test key to run this evaluation");
    assert!(
        !api_key.trim().is_empty(),
        "TYPESAFE_API_KEY must not be empty"
    );

    let fixture: Value = serde_json::from_str(CASES).expect("synthetic case fixture is valid");
    let cases = fixture["cases"].as_array().expect("cases array");
    let state_cases: Vec<Value> = cases
        .iter()
        .map(|case| {
            json!({
                "id": case["id"],
                "instruction": case["instruction"],
                "evidence_chunks": case["evidence_chunks"]
            })
        })
        .collect();

    let mut questions = Map::new();
    for case in cases {
        let id = case["id"].as_str().expect("case ID");
        let assessment_id = format!("assessment_{id}");
        let citation_id = format!("citation_{id}");
        questions.insert(
            assessment_id,
            json!({
                "type": "choice",
                "instructions": format!("Assess case {id}. Does its evidence show a likely instruction conflict, show no conflict in the checked content, or lack enough evidence to assess? Use only this case's instruction and evidence."),
                "criteria": {
                    "likely_issue": "The supplied evidence directly supports a conflict with an applicable instruction.",
                    "no_issue_in_checked_content": "The supplied evidence supports compliance or an allowed exception, and the checked content is sufficient for this decision.",
                    "unassessed": "The source is not historical proof, the relevant history is missing, or the evidence cannot establish a conflict or compliance."
                }
            }),
        );

        let mut citation_criteria = Map::new();
        let mut event_ids = Vec::new();
        for event in case["evidence_chunks"]
            .as_array()
            .expect("evidence chunks")
            .iter()
            .flat_map(|chunk| chunk["events"].as_array().expect("events"))
        {
            let event_id = event["id"].as_str().expect("event ID");
            event_ids.push(event_id.to_owned());
            citation_criteria.insert(
                event_id.to_owned(),
                Value::String(format!("Event {event_id} is the most direct support for a likely conflict in case {id}.")),
            );
        }
        citation_criteria.insert(
            "none".to_owned(),
            Value::String(
                "No event directly supports a likely conflict, or the case is unassessed."
                    .to_owned(),
            ),
        );
        questions.insert(
            citation_id,
            json!({
                "type": "choice",
                "instructions": format!("For case {id}, select the one event that directly supports a likely instruction conflict. Select none when the case is no_issue_in_checked_content or unassessed, or when no event directly supports a likely conflict. An event that only provides context or shows an action is not enough."),
                "criteria": citation_criteria
            }),
        );
        assert!(
            !event_ids.is_empty(),
            "{id} must provide evaluation evidence"
        );
    }

    let payload = json!({
        "model": MODEL,
        "state": {"cases": state_cases},
        "questions": questions
    });
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build TypeSafe evaluation client");
    let response = client
        .post(ENDPOINT)
        .bearer_auth(api_key)
        .json(&payload)
        .send()
        .expect("send synthetic ignored-instruction evaluation");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "TypeSafe must accept the synthetic evaluation request"
    );
    let response: Value = response.json().expect("decode TypeSafe response");
    assert_eq!(response["model"].as_str(), Some(MODEL));
    let answers = response["answers"].as_object().expect("answers object");

    let mut correct_assessments = 0;
    let mut correct_citations = 0;
    for case in cases {
        let id = case["id"].as_str().expect("case ID");
        let expected_assessment = case["expected"]["assessment"]
            .as_str()
            .expect("expected assessment");
        let expected_citation = case["expected"]["finding_event_ids"]
            .as_array()
            .and_then(|ids| ids.first())
            .and_then(Value::as_str)
            .unwrap_or("none");
        let assessment = answers[&format!("assessment_{id}")]["choice"]
            .as_str()
            .expect("typed assessment answer");
        let citation = answers[&format!("citation_{id}")]["choice"]
            .as_str()
            .expect("typed citation answer");
        correct_assessments += usize::from(assessment == expected_assessment);
        correct_citations += usize::from(citation == expected_citation);
        eprintln!(
            "{id}: assessment={assessment} (expected {expected_assessment}); citation={citation} (expected {expected_citation})"
        );
    }

    let total = cases.len();
    eprintln!(
        "Jev synthetic evaluation: {correct_assessments}/{total} assessments, {correct_citations}/{total} citations"
    );
    assert!(
        correct_assessments >= MINIMUM_ACCURACY,
        "assessment accuracy was below the tuning floor of {MINIMUM_ACCURACY}/{total}"
    );
    assert!(
        correct_citations >= MINIMUM_ACCURACY,
        "citation accuracy was below the tuning floor of {MINIMUM_ACCURACY}/{total}"
    );
    let input_tokens = response["usage"]["input_tokens"]
        .as_u64()
        .expect("usage includes input token count");
    assert!(
        input_tokens <= 8_192,
        "evaluation exceeded the input-token budget"
    );
    assert!(response["usage"]["output_tokens"].as_u64().is_some());
}
