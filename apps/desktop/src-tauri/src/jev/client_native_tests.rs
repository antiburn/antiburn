use super::*;
use antiburn_local::analysis::jev::{JevQuestion, NATIVE_MODEL_ALIAS};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use tracing::instrument::WithSubscriber;

use crate::trace_capture;

fn request() -> JevRequest {
    JevRequest {
        model: NATIVE_MODEL_ALIAS.into(),
        state: json!({"text":"PRIVATE_EVIDENCE"}),
        questions: BTreeMap::from([(
            "q".into(),
            JevQuestion::Choice {
                instructions: json!("PRIVATE_QUESTION"),
                criteria: BTreeMap::from([
                    ("yes".into(), json!("Yes")),
                    ("no".into(), json!("No")),
                ]),
            },
        )]),
    }
}

fn body(model: &str, mismatch: bool) -> String {
    json!({"model":model,"answers":{"q":{"type":"choice","choice":if mismatch {"no"} else {"yes"},"probabilities":{"yes":0.9,"no":0.1},"confidence":0.8}},"usage":{"input_tokens":12,"output_tokens":1}}).to_string()
}

async fn loopback(
    body: String,
    request: &JevRequest,
    observation: impl Fn(JevResponseObservation) -> Result<(), JevError>,
) -> Result<JevResponse, JevError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
    let expected = serde_json::to_vec(request).unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0; 8192];
            let count = socket.read(&mut chunk).unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n")
                && bytes.len() >= end + 4 + expected.len()
            {
                let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                assert!(
                    headers
                        .lines()
                        .any(|line| line == "typesafe-danger-zone-long-context: true")
                );
                assert_eq!(&bytes[end + 4..], expected);
                break;
            }
        }
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let result = TypeSafeClient::new("PRIVATE_KEY".into())
        .unwrap()
        .evaluate_at_with_capabilities_and_usage(
            request,
            &endpoint,
            &ModelCapabilities::jev_native(),
            observation,
        )
        .await;
    server.join().unwrap();
    result
}

#[tokio::test]
async fn native_current_future_and_alias_responses_keep_observed_identity_and_safe_logs() {
    for model in [
        "jev-1.14.0",
        "PRIVATE_FUTURE_RELEASE-2027.2",
        NATIVE_MODEL_ALIAS,
    ] {
        for mismatch in [false, true] {
            let capture = trace_capture::LogCapture::default();
            let observations = std::cell::RefCell::new(Vec::new());
            let response = loopback(body(model, mismatch), &request(), |observation| {
                observations.borrow_mut().push(observation);
                Ok(())
            })
            .with_subscriber(capture.clone())
            .await
            .unwrap();
            assert_eq!(response.model, model);
            let observed = observations.borrow();
            assert_eq!(observed.len(), 1);
            assert_eq!(observed[0].usage.input_tokens, 12);
            assert_eq!(
                observed[0].served_model.as_deref(),
                (model != NATIVE_MODEL_ALIAS).then_some(model)
            );
            let timing = capture.events("typesafe_request_timing");
            assert_eq!(timing.len(), 1);
            assert_eq!(timing[0].level, tracing::Level::DEBUG);
            assert_eq!(
                timing[0]
                    .fields
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                [
                    "body_elapsed_ms",
                    "event",
                    "input_tokens",
                    "output_tokens",
                    "question_count",
                    "send_elapsed_ms",
                    "validation_elapsed_ms"
                ]
            );
            let warning = capture.events("typesafe_choice_probability_mismatch");
            assert_eq!(warning.len(), usize::from(mismatch));
            if mismatch {
                assert_eq!(
                    warning[0]
                        .fields
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    [
                        "event",
                        "input_tokens",
                        "largest_probability_gap",
                        "mismatch_count",
                        "output_tokens",
                        "question_count"
                    ]
                );
            }
            let native_logs = format!("{timing:?} {warning:?}");
            for private in [
                model,
                "PRIVATE_EVIDENCE",
                "PRIVATE_QUESTION",
                "PRIVATE_KEY",
                "127.0.0.1",
            ] {
                assert!(!native_logs.contains(private), "{private}");
            }
            assert!(!capture.text().contains("PRIVATE_FUTURE_RELEASE"));
        }
    }
}

#[tokio::test]
async fn native_invalid_answers_and_ambiguous_models_preserve_independent_usage() {
    let valid = body("jev-1.14.0", false);
    for (body, expected_model) in [
        (
            valid.replace("\"choice\":\"yes\"", "\"choice\":\"undeclared\""),
            Some("jev-1.14.0"),
        ),
        (
            valid.replace(
                "\"model\":\"jev-1.14.0\"",
                "\"model\":\"jev-1.14.0\",\"model\":\"other\"",
            ),
            None,
        ),
        (valid.replace("\"model\":\"jev-1.14.0\",", ""), None),
        (valid.replace("jev-1.14.0", "unsafe model"), None),
        (valid.replace("jev-1.14.0", ""), None),
    ] {
        let observations = std::cell::RefCell::new(Vec::new());
        assert!(
            loopback(body, &request(), |observation| {
                observations.borrow_mut().push(observation);
                Ok(())
            })
            .await
            .is_err()
        );
        let observations = observations.borrow();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].usage.input_tokens, 12);
        assert_eq!(observations[0].served_model.as_deref(), expected_model);
    }
    let observations = std::cell::Cell::new(0);
    let duplicate_usage = valid.replace(
        "\"input_tokens\":12",
        "\"input_tokens\":12,\"input_tokens\":13",
    );
    assert!(
        loopback(duplicate_usage, &request(), |_| {
            observations.set(observations.get() + 1);
            Ok(())
        })
        .await
        .is_err()
    );
    assert_eq!(observations.get(), 0);
}

#[test]
fn native_header_does_not_leak_to_custom_official_or_envelope_request_builders() {
    let native = TypeSafeClient::new("synthetic".into())
        .unwrap()
        .request_builder(&system_one_endpoint(), vec![])
        .build()
        .unwrap();
    assert_eq!(
        native.headers()["TypeSafe-Danger-Zone-Long-Context"],
        "true"
    );
    for endpoint in [
        system_one_endpoint(),
        "http://localhost:11434/v1/systemone".into(),
        "https://api.cloudflare.com/client/v4/accounts/account/ai/run/@cf/cloudflare/clef".into(),
    ] {
        for credential in [None, Some("synthetic")] {
            let custom = custom_request_builder(&endpoint, credential, vec![])
                .build()
                .unwrap();
            assert!(
                !custom
                    .headers()
                    .contains_key("TypeSafe-Danger-Zone-Long-Context")
            );
        }
    }
}

#[tokio::test]
async fn native_large_request_reaches_loopback_without_shortening() {
    let mut request = request();
    request.state = json!({"text":"a".repeat(320_000)});
    let capabilities = ModelCapabilities::jev_native();
    let bytes = validate_jev_request_with_capabilities(&request, &capabilities).unwrap();
    assert!(bytes > 256 * 1024);
    assert!(bytes * 2 + MAX_RESPONSE_BYTES > 512 * 1024);
    assert!(bytes * 2 + MAX_RESPONSE_BYTES < 832 * 1024);
    loopback(body("jev-1.14.0", false), &request, |_| Ok(()))
        .await
        .unwrap();
}
