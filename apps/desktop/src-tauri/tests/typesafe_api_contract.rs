use std::time::Duration;

use serde_json::{Value, json};

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const MODEL: &str = "jev-1.13.0";

#[test]
#[ignore = "makes one billable TypeSafe request using synthetic text"]
fn typesafe_system_one_contract_uses_synthetic_evidence() {
    let api_key = std::env::var("TYPESAFE_API_KEY")
        .expect("set TYPESAFE_API_KEY to an authorized test key to run this test");
    assert!(
        !api_key.trim().is_empty(),
        "TYPESAFE_API_KEY must not be empty"
    );

    let state = json!({
        "instruction": {
            "section_id": "section-validation",
            "section": "Before committing",
            "text": "Run the focused tests before committing."
        },
        "session": {
            "actions": [
                {
                  "event_id": "action-commit",
                  "sequence": 1,
                  "kind": "commit",
                  "text": "Committed the changes."
                },
                {
                  "event_id": "action-test",
                  "sequence": 2,
                  "kind": "test",
                  "text": "Ran the focused tests successfully."
                }
            ]
        }
    });
    let questions = json!({
        "has_conflicting_action": {
            "type": "noul",
            "instructions": "Does `session.actions` show that the commit happened before the focused tests required by `instruction.text`? Judge only the supplied actions and their sequence numbers.",
            "criteria": {
                "true": "A commit occurred before the required focused tests.",
                "false": "The tests occurred before the commit, or the supplied evidence does not show a conflict."
            }
        },
        "supporting_event": {
            "type": "choice",
            "instructions": "Which supplied event directly shows the conflict with `instruction.text`? Select `none` when the supplied evidence does not show a conflict.",
            "criteria": {
                "action-commit": "The commit occurred before the required focused tests.",
                "action-test": "The focused tests ran successfully.",
                "none": "No supplied event directly supports a conflict."
            }
        }
    });
    let payload = json!({
        "model": MODEL,
        "state": state,
        "questions": questions
    });

    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build TypeSafe contract test client");
    let response = client
        .post(ENDPOINT)
        .bearer_auth(api_key)
        .json(&payload)
        .send()
        .expect("send synthetic TypeSafe contract request");

    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "TypeSafe must accept the documented System One request"
    );
    let response: Value = response.json().expect("decode TypeSafe JSON response");
    assert_eq!(response["model"].as_str(), Some(MODEL));
    assert_eq!(
        response["answers"]["has_conflicting_action"]["type"],
        "noul"
    );
    let noul = response["answers"]["has_conflicting_action"]["noul"]
        .as_f64()
        .expect("Noul response must contain a numeric judgment");
    assert!(noul > 0.5, "the synthetic recorded order is a conflict");

    assert_eq!(response["answers"]["supporting_event"]["type"], "choice");
    let cited_event = response["answers"]["supporting_event"]["choice"]
        .as_str()
        .expect("Choice response must contain a selected evidence ID");
    assert_eq!(cited_event, "action-commit");
    let probabilities = response["answers"]["supporting_event"]["probabilities"]
        .as_object()
        .expect("Choice response must contain probabilities");
    assert_eq!(probabilities.len(), 3);
    assert!(probabilities.values().all(Value::is_number));

    let input_tokens = response["usage"]["input_tokens"]
        .as_u64()
        .expect("usage must include input token count");
    assert!(input_tokens <= 8_192);
    assert!(response["usage"]["output_tokens"].as_u64().is_some());
}
