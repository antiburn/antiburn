use std::collections::BTreeMap;

use antiburn_local::analysis::jev::{JevQuestion, JevRequest};
use serde_json::json;

pub const CASE_ID: &str = "completed-task";

pub fn request(model: &str) -> JevRequest {
    JevRequest {
        model: model.into(),
        state: json!({"text":"The task is to read one configuration file. The assistant read it and reported its contents."}),
        questions: BTreeMap::from([
            (
                "sufficient".into(),
                JevQuestion::Noul {
                    instructions: json!("Was the requested task completed?"),
                    criteria: Some(json!({"true":"Completed","false":"Not completed"})),
                },
            ),
            (
                "completion".into(),
                JevQuestion::Score {
                    instructions: json!("Rate how completely the requested task was done."),
                    criteria: vec![
                        json!("The requested file was not read."),
                        json!("The requested file was read but its contents were not reported."),
                        json!("The requested file was read and its contents were reported."),
                    ],
                },
            ),
            (
                "scope".into(),
                JevQuestion::Choice {
                    instructions: json!("Classify the work against the task."),
                    criteria: BTreeMap::from([
                        ("within".into(), json!("The work stays within the task.")),
                        ("outside".into(), json!("The work exceeds the task.")),
                        (
                            "unknown".into(),
                            json!("The evidence does not resolve scope."),
                        ),
                    ]),
                },
            ),
        ]),
    }
}

#[test]
fn protocol_fixture_has_one_mixed_request_and_exact_typed_answers() {
    use antiburn_local::analysis::jev::{
        JevResponse, PINNED_MODEL, validate_jev_request, validate_jev_response,
    };
    let request = request(PINNED_MODEL);
    assert_eq!(request.questions.len(), 3);
    validate_jev_request(&request).unwrap();
    let response: JevResponse = serde_json::from_value(json!({
        "model": PINNED_MODEL,
        "answers": expected_answers(),
        "usage": {"input_tokens":100,"output_tokens":3}
    }))
    .unwrap();
    validate_jev_response(&response, &request).unwrap();
    for name in request.questions.keys() {
        let mut missing = response.clone();
        missing.answers.remove(name);
        assert!(validate_jev_response(&missing, &request).is_err());
    }
}

pub fn expected_answers() -> serde_json::Value {
    json!({
        "scope": {"type":"choice","choice":"within","probabilities":{"within":0.9,"outside":0.05,"unknown":0.05},"confidence":0.9},
        "completion": {"type":"score","score":1.9,"legend":{"0":"The requested file was not read.","1":"The requested file was read but its contents were not reported.","2":"The requested file was read and its contents were reported."},"probabilities":{"0":0.02,"1":0.06,"2":0.92},"confidence":0.8},
        "sufficient": {"type":"noul","noul":0.9}
    })
}
