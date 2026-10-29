//! Bounded TypeSafe System One transport.

use std::time::{Duration, Instant};

use crate::jev::config::SystemOneResponseMode;
use crate::jev::config::system_one_endpoint;
use antiburn_local::analysis::jev::{
    JevAnswer, JevError, JevRequest, JevResponse, JevResponseObservation, JevUsage,
    JevUsageLocation, MAX_RESPONSE_BYTES, capabilities::ModelCapabilities,
    decode_jev_json_with_observation, decode_jev_json_with_usage, highest_probability_choice,
    validate_jev_request_with_capabilities, validate_jev_response_with_capabilities,
    validate_native_jev_response_with_capabilities,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(test)]
#[path = "client_native_tests.rs"]
mod native_tests;

fn async_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("a client with no custom TLS material always builds")
    })
}

/// The client keeps the key private and never includes it in debug output.
#[derive(Clone)]
pub(crate) struct TypeSafeClient {
    api_key: std::sync::Arc<str>,
}

impl TypeSafeClient {
    #[cfg(test)]
    async fn evaluate_at(
        &self,
        request: &JevRequest,
        endpoint: &str,
    ) -> Result<JevResponse, JevError> {
        self.evaluate_at_with_capabilities(request, endpoint, &ModelCapabilities::jev_native())
            .await
    }

    #[cfg(test)]
    pub(crate) async fn evaluate_at_with_capabilities(
        &self,
        request: &JevRequest,
        endpoint: &str,
        capabilities: &ModelCapabilities,
    ) -> Result<JevResponse, JevError> {
        self.evaluate_at_with_capabilities_and_usage(request, endpoint, capabilities, |_| Ok(()))
            .await
    }

    /// Records available provider usage before validating the answer.
    pub(crate) async fn evaluate_at_with_capabilities_and_usage(
        &self,
        request: &JevRequest,
        endpoint: &str,
        capabilities: &ModelCapabilities,
        record_usage: impl Fn(JevResponseObservation) -> Result<(), JevError>,
    ) -> Result<JevResponse, JevError> {
        let body = JevSystemOneAdapter::encode_request(request, capabilities)?;
        let send_started = Instant::now();
        let mut response = self
            .request_builder(endpoint, body)
            .send()
            .await
            .map_err(|error| {
                if error.is_builder() {
                    JevError::InvalidRequestSchema
                } else if error.is_connect() {
                    JevError::ProviderUnavailable
                } else {
                    JevError::RequestOutcomeUnknown
                }
            })?;
        let send_elapsed_ms = send_started.elapsed().as_millis();
        if !response.status().is_success() {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| {
                    retry_after_delay(value, time::OffsetDateTime::now_utc().unix_timestamp())
                });
            return Err(JevSystemOneAdapter::status_error(
                response.status().as_u16(),
                retry_after,
            ));
        }
        let maximum_response_bytes = capabilities
            .response_body_bytes
            .value
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(MAX_RESPONSE_BYTES)
            .min(MAX_RESPONSE_BYTES);
        if response
            .content_length()
            .is_some_and(|length| length > maximum_response_bytes as u64)
        {
            return Err(JevError::ResponseTooLarge);
        }
        let body_started = Instant::now();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| JevError::RequestOutcomeUnknown)?
        {
            if chunk.len() > maximum_response_bytes.saturating_sub(body.len()) {
                return Err(JevError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let body_elapsed_ms = body_started.elapsed().as_millis();
        let validation_started = Instant::now();
        let response: JevResponse =
            decode_jev_json_with_observation(&body, JevUsageLocation::Direct, record_usage)?;
        validate_native_jev_response_with_capabilities(&response, request, capabilities)?;
        let (mismatch_count, largest_gap) = choice_mismatches(&response);
        if mismatch_count > 0 {
            ::tracing::warn!(
                event = "typesafe_choice_probability_mismatch",
                question_count = request.questions.len(),
                mismatch_count,
                largest_probability_gap = largest_gap,
                input_tokens = response.usage.input_tokens,
                output_tokens = response.usage.output_tokens
            );
        }
        ::tracing::debug!(
            event = "typesafe_request_timing",
            question_count = request.questions.len(),
            input_tokens = response.usage.input_tokens,
            output_tokens = response.usage.output_tokens,
            send_elapsed_ms,
            body_elapsed_ms,
            validation_elapsed_ms = validation_started.elapsed().as_millis()
        );
        Ok(response)
    }

    pub(crate) fn new(api_key: String) -> Result<Self, JevError> {
        if api_key.trim().is_empty() {
            return Err(JevError::AuthenticationRejected);
        }
        Ok(Self {
            api_key: api_key.into(),
        })
    }

    fn request_builder(&self, endpoint: &str, body: Vec<u8>) -> reqwest::RequestBuilder {
        async_client()
            .post(endpoint)
            .bearer_auth(self.api_key.as_ref())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("TypeSafe-Danger-Zone-Long-Context", "true")
            .body(body)
    }
}

/// Maps the generic worker contract to TypeSafe's JSON-over-HTTP protocol.
pub(crate) struct JevSystemOneAdapter;

impl JevSystemOneAdapter {
    fn encode_request(
        request: &JevRequest,
        capabilities: &ModelCapabilities,
    ) -> Result<Vec<u8>, JevError> {
        validate_jev_request_with_capabilities(request, capabilities)?;
        serde_json::to_vec(request).map_err(|_| JevError::RequestSerialization)
    }

    fn decode_response_with_usage(
        body: &[u8],
        request: &JevRequest,
        capabilities: &ModelCapabilities,
        record_usage: impl Fn(JevUsage) -> Result<(), JevError>,
    ) -> Result<JevResponse, JevError> {
        let response = decode_jev_json_with_usage(body, JevUsageLocation::Direct, record_usage)?;
        validate_jev_response_with_capabilities(&response, request, capabilities)?;
        Ok(response)
    }

    fn status_error(status: u16, retry_after: Option<Duration>) -> JevError {
        match status {
            401 | 403 => JevError::AuthenticationRejected,
            400 | 413 | 415 | 422 => JevError::InvalidRequestSchema,
            429 => JevError::RateLimited { retry_after },
            529 => JevError::ProviderOverloaded { retry_after },
            _ => JevError::RequestOutcomeUnknown,
        }
    }
}

/// Send a compatible System One request to its configured exact URL.
pub async fn evaluate_custom(
    endpoint: &str,
    credential: Option<&str>,
    mode: SystemOneResponseMode,
    request: &JevRequest,
    capabilities: &ModelCapabilities,
) -> Result<JevResponse, JevError> {
    evaluate_custom_with_usage(
        endpoint,
        credential,
        mode,
        request,
        capabilities,
        |_| Ok(()),
    )
    .await
}

/// Records reported usage before validating a direct or Cloudflare-envelope answer.
pub async fn evaluate_custom_with_usage(
    endpoint: &str,
    credential: Option<&str>,
    mode: SystemOneResponseMode,
    request: &JevRequest,
    capabilities: &ModelCapabilities,
    record_usage: impl Fn(JevUsage) -> Result<(), JevError>,
) -> Result<JevResponse, JevError> {
    let body = match mode {
        SystemOneResponseMode::Direct => {
            JevSystemOneAdapter::encode_request(request, capabilities)?
        }
        SystemOneResponseMode::CloudflareEnvelope => {
            crate::jev_cloudflare::CloudflareAdapter::encode_request(
                request,
                &request.model,
                capabilities,
            )?
        }
    };
    let builder = custom_request_builder(endpoint, credential, body);
    let mut response = builder.send().await.map_err(|error| {
        if error.is_builder() {
            JevError::InvalidRequestSchema
        } else if error.is_connect() {
            JevError::ProviderUnavailable
        } else {
            JevError::RequestOutcomeUnknown
        }
    })?;
    if !response.status().is_success() {
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                retry_after_delay(value, time::OffsetDateTime::now_utc().unix_timestamp())
            });
        let status = response.status().as_u16();
        return Err(if status == 529 && endpoint != system_one_endpoint() {
            JevError::RequestOutcomeUnknown
        } else {
            JevSystemOneAdapter::status_error(status, retry_after)
        });
    }
    let maximum = capabilities
        .response_body_bytes
        .value
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(MAX_RESPONSE_BYTES)
        .min(MAX_RESPONSE_BYTES);
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(JevError::ResponseTooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| JevError::RequestOutcomeUnknown)?
    {
        if chunk.len() > maximum.saturating_sub(body.len()) {
            return Err(JevError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    match mode {
        SystemOneResponseMode::Direct => JevSystemOneAdapter::decode_response_with_usage(
            &body,
            request,
            capabilities,
            record_usage,
        ),
        SystemOneResponseMode::CloudflareEnvelope => {
            crate::jev_cloudflare::CloudflareAdapter::decode_response_with_usage(
                &body,
                request,
                &request.model,
                capabilities,
                record_usage,
            )
        }
    }
}

fn custom_request_builder(
    endpoint: &str,
    credential: Option<&str>,
    body: Vec<u8>,
) -> reqwest::RequestBuilder {
    let builder = async_client()
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body);
    match credential {
        Some(credential) => builder.bearer_auth(credential),
        None => builder,
    }
}

#[cfg(test)]
mod custom_transport_tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};

    fn request(model: &str) -> JevRequest {
        JevRequest {
            model: model.to_owned(),
            state: json!({"text": "synthetic evidence"}),
            questions: BTreeMap::from([(
                "q".to_owned(),
                antiburn_local::analysis::jev::JevQuestion::Noul {
                    instructions: json!("Is the evidence sufficient?"),
                    criteria: None,
                },
            )]),
        }
    }

    #[tokio::test]
    async fn mixed_blocks_use_one_unchanged_send_and_strict_decode_in_all_direct_and_custom_modes()
    {
        use antiburn_local::analysis::jev::JevQuestion;

        let caps = ModelCapabilities::jev_native();
        let request = JevRequest {
            model: caps.model.clone(),
            state: json!("synthetic evidence"),
            questions: BTreeMap::from([
                (
                    "choice".into(),
                    JevQuestion::Choice {
                        instructions: json!("Pick one"),
                        criteria: BTreeMap::from([
                            ("yes".into(), json!("Yes")),
                            ("no".into(), json!("No")),
                        ]),
                    },
                ),
                (
                    "score".into(),
                    JevQuestion::Score {
                        instructions: json!("Rate it"),
                        criteria: vec![json!("low"), json!("high")],
                    },
                ),
                (
                    "noul".into(),
                    JevQuestion::Noul {
                        instructions: json!("Does it apply?"),
                        criteria: None,
                    },
                ),
            ]),
        };
        let answers = r#"{"choice":{"type":"choice","choice":"yes","probabilities":{"yes":0.1,"no":0.9},"confidence":0.8},"score":{"type":"score","score":0.3,"legend":{"0":"low","1":"high"},"probabilities":{"0":0.9,"1":0.1},"confidence":0.8},"noul":{"type":"noul","noul":0.2}}"#;
        let valid = format!(
            r#"{{"model":"{}","answers":{answers},"usage":{{"input_tokens":12,"output_tokens":1}}}}"#,
            caps.model
        );
        for transport in ["typesafe", "custom-direct", "custom-envelope"] {
            let mut cases = vec![
                (valid.clone(), Ok(()), true),
                (valid.replace(r#""noul":{"type":"noul","noul":0.2}"#, r#""noul":{"type":"noul","noul":0.2},"noul":{"type":"noul","noul":0.9}"#), Err(JevError::ResponseDecode), true),
                (valid.replace(r#""yes":0.1"#, r#""yes":0.1,"yes":0.8"#), Err(JevError::ResponseDecode), true),
                (valid.replace(r#""0":"low""#, r#""0":"low","0":"high""#), Err(JevError::ResponseDecode), true),
                (valid.replace(r#""input_tokens":12"#, r#""input_tokens":12,"input_tokens":99"#), Err(JevError::ResponseDecode), false),
                (valid.replace(r#""usage":{"input_tokens":12,"output_tokens":1}"#, r#""usage":{"input_tokens":12,"output_tokens":1},"usage":{"input_tokens":99,"output_tokens":9}"#), Err(JevError::ResponseDecode), false),
                (valid.replace(r#""noul":0.2"#, r#""noul":0.2,"confidence":0.9"#), Err(JevError::ResponseDecode), true),
                (valid.replace(r#""0":0.9"#, r#""00":0.9"#), Err(JevError::InvalidScoreDistribution), true),
                (valid.replace(r#""0":"low","1":"high""#, r#""0":"high","1":"low""#), Err(JevError::InvalidScoreDistribution), true),
                (valid.replace(r#""score":0.3"#, r#""score":1.1"#), Err(JevError::InvalidScoreDistribution), true),
                (valid.replace(r#""0":0.9"#, r#""0":0.4"#), Err(JevError::InvalidProbabilitySum), true),
                (valid.replace(r#""noul":{"type":"noul","noul":0.2}"#, r#""extra":{"type":"noul","noul":0.2}"#), Err(JevError::ResponseAnswerMissing), true),
                (valid.replace(r#""noul":{"type":"noul","noul":0.2}"#, r#""noul":{"type":"choice","choice":"yes","probabilities":{"yes":0.1,"no":0.9},"confidence":0.8}"#), Err(JevError::ResponseAnswerTypeMismatch), true),
            ];
            cases.push((
                valid.replace(&caps.model, "future/jev:release-2099"),
                if transport == "typesafe" {
                    Ok(())
                } else {
                    Err(JevError::ResponseModelMismatch)
                },
                true,
            ));
            if transport == "custom-envelope" {
                for (body, _, _) in &mut cases {
                    *body = format!(r#"{{"success":true,"result":{body}}}"#);
                }
                cases.push((
                    format!(r#"{{"success":true,"result":{valid},"result":{valid}}}"#),
                    Err(JevError::ResponseDecode),
                    false,
                ));
                cases.push((
                    format!(r#"{{"success":true,"success":false,"result":{valid}}}"#),
                    Err(JevError::ResponseDecode),
                    false,
                ));
            }
            for (body, expected, known_usage) in cases {
                let expected_request = serde_json::to_vec(&request).unwrap();
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let endpoint = format!("http://{}/", listener.local_addr().unwrap());
                let server = std::thread::spawn(move || {
                    let (mut socket, _) = listener.accept().unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut received = Vec::new();
                    loop {
                        let mut chunk = [0; 4096];
                        let count = socket.read(&mut chunk).unwrap();
                        assert!(count > 0);
                        received.extend_from_slice(&chunk[..count]);
                        if let Some(end) =
                            received.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                            && received.len() >= end + 4 + expected_request.len()
                        {
                            let headers =
                                String::from_utf8_lossy(&received[..end]).to_ascii_lowercase();
                            assert_eq!(
                                headers.contains("typesafe-danger-zone-long-context: true"),
                                transport == "typesafe"
                            );
                            assert_eq!(&received[end + 4..], expected_request);
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
                let calls = std::cell::Cell::new(0);
                let record_usage = |usage| {
                    calls.set(calls.get() + 1);
                    assert_eq!(
                        usage,
                        JevUsage {
                            input_tokens: 12,
                            output_tokens: 1
                        }
                    );
                    Ok(())
                };
                let actual = if transport == "typesafe" {
                    TypeSafeClient::new("synthetic".into())
                        .unwrap()
                        .evaluate_at_with_capabilities_and_usage(
                            &request,
                            &endpoint,
                            &caps,
                            |observation| record_usage(observation.usage),
                        )
                        .await
                } else {
                    evaluate_custom_with_usage(
                        &endpoint,
                        None,
                        if transport == "custom-direct" {
                            SystemOneResponseMode::Direct
                        } else {
                            SystemOneResponseMode::CloudflareEnvelope
                        },
                        &request,
                        &caps,
                        record_usage,
                    )
                    .await
                };
                assert_eq!(
                    actual.as_ref().map(|_| ()).map_err(Clone::clone),
                    expected,
                    "{transport}"
                );
                assert_eq!(calls.get(), usize::from(known_usage), "{transport}");
                if let Ok(response) = actual {
                    assert_eq!(response.answers.len(), 3);
                    assert!(
                        matches!(&response.answers["choice"], JevAnswer::Choice { choice, .. } if choice == "yes")
                    );
                    assert!(
                        matches!(response.answers["score"], JevAnswer::Score { score, .. } if score == 0.3)
                    );
                }
                server.join().unwrap();
            }
        }
    }

    #[tokio::test]
    async fn usage_survives_invalid_answers_in_typesafe_and_both_custom_modes() {
        let capabilities = ModelCapabilities::jev_native();
        let request = request(&capabilities.model);
        for transport in ["typesafe", "custom-direct", "custom-envelope"] {
            for (answer, usage, callback_error, expected) in [
                (
                    json!({"type": "noul", "noul": "invalid"}),
                    json!({"input_tokens": 12, "output_tokens": 1}),
                    false,
                    JevError::ResponseDecode,
                ),
                (
                    json!({"type": "noul", "noul": 1.5}),
                    json!({"input_tokens": 12, "output_tokens": 1}),
                    false,
                    JevError::InvalidNoulProbability,
                ),
                (
                    json!({"type": "noul", "noul": 0.2}),
                    json!({"input_tokens": 12, "output_tokens": 1}),
                    true,
                    JevError::ProgressStorageFailure,
                ),
                (
                    json!({"type": "noul", "noul": 0.2}),
                    json!({"input_tokens": -1, "output_tokens": 1}),
                    false,
                    JevError::ResponseDecode,
                ),
            ] {
                let valid_usage = usage["input_tokens"].as_u64().is_some();
                let result =
                    json!({"model": capabilities.model, "answers": {"q": answer}, "usage": usage});
                let body = if transport == "custom-envelope" {
                    json!({"success": true, "result": result})
                } else {
                    result
                }
                .to_string();
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let endpoint = format!("http://{}/", listener.local_addr().unwrap());
                let server = std::thread::spawn(move || {
                    let (mut socket, _) = listener.accept().unwrap();
                    let mut bytes = [0; 4096];
                    assert!(socket.read(&mut bytes).unwrap() > 0);
                    write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                });
                let recorded = std::cell::Cell::new(0);
                let record_usage = |usage: JevUsage| {
                    recorded.set(recorded.get() + 1);
                    assert_eq!(
                        usage,
                        JevUsage {
                            input_tokens: 12,
                            output_tokens: 1
                        }
                    );
                    if callback_error {
                        Err(JevError::ProgressStorageFailure)
                    } else {
                        Ok(())
                    }
                };
                let actual = if transport == "typesafe" {
                    TypeSafeClient::new("synthetic".into())
                        .unwrap()
                        .evaluate_at_with_capabilities_and_usage(
                            &request,
                            &endpoint,
                            &capabilities,
                            |observation| record_usage(observation.usage),
                        )
                        .await
                } else {
                    evaluate_custom_with_usage(
                        &endpoint,
                        None,
                        if transport == "custom-direct" {
                            SystemOneResponseMode::Direct
                        } else {
                            SystemOneResponseMode::CloudflareEnvelope
                        },
                        &request,
                        &capabilities,
                        record_usage,
                    )
                    .await
                };
                assert_eq!(actual, Err(expected), "{transport}");
                assert_eq!(recorded.get(), u32::from(valid_usage), "{transport}");
                server.join().unwrap();
            }
        }
    }

    #[tokio::test]
    async fn custom_transport_reuses_connections_without_reusing_credentials() {
        use std::io::BufRead;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = std::io::BufReader::new(socket);
            for authenticated in [true, false] {
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    headers.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let headers = headers.to_ascii_lowercase();
                assert!(!headers.contains("typesafe-danger-zone-long-context:"));
                assert_eq!(
                    headers.contains("authorization: bearer first"),
                    authenticated
                );
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .map(|value| value.trim().parse().unwrap())
                    })
                    .unwrap();
                reader.read_exact(&mut vec![0; length]).unwrap();
                let response = json!({
                    "model": "proxy-model", "answers": {"q": {"type": "noul", "noul": 0.2}},
                    "usage": {"input_tokens": 10, "output_tokens": 1}
                });
                let body = if authenticated {
                    response
                } else {
                    json!({"success": true, "result": response})
                }
                .to_string();
                write!(
                    reader.get_mut(),
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                reader.get_mut().flush().unwrap();
            }
        });
        let mut capabilities = ModelCapabilities::jev_default();
        capabilities.model = "proxy-model".into();
        for (credential, mode) in [
            (Some("first"), SystemOneResponseMode::Direct),
            (None, SystemOneResponseMode::CloudflareEnvelope),
        ] {
            let response = tokio::time::timeout(
                Duration::from_secs(10),
                evaluate_custom(
                    &endpoint,
                    credential,
                    mode,
                    &request("proxy-model"),
                    &capabilities,
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(response.model, "proxy-model");
        }
        server.join().unwrap();
    }

    #[tokio::test]
    async fn custom_transport_decodes_explicit_cloudflare_envelopes() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "http://{}/prefix?tenant=custom",
            listener.local_addr().unwrap()
        );
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut bytes = [0u8; 4096];
            let _ = socket.read(&mut bytes).unwrap();
            let body = json!({"success": true, "result": {
                "model": "proxy-model", "answers": {"q": {"type": "noul", "noul": 0.2}},
                "usage": {"input_tokens": 10, "output_tokens": 1}
            }})
            .to_string();
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let mut capabilities = ModelCapabilities::jev_default();
        capabilities.model = "proxy-model".into();
        let response = evaluate_custom(
            &endpoint,
            None,
            SystemOneResponseMode::CloudflareEnvelope,
            &request("proxy-model"),
            &capabilities,
        )
        .await
        .unwrap();
        assert_eq!(response.model, "proxy-model");
        assert_eq!(response.usage.input_tokens, 10);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn custom_transport_does_not_follow_redirects_with_credentials() {
        let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let target_endpoint = format!("http://{}/", target.local_addr().unwrap());
        let source = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "http://{}/exact/path?route=one",
            source.local_addr().unwrap()
        );
        let redirect = std::thread::spawn(move || {
            let (mut socket, _) = source.accept().unwrap();
            let mut received = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let count = socket.read(&mut buffer).unwrap();
                received.extend_from_slice(&buffer[..count]);
                if received.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            let headers = String::from_utf8_lossy(&received);
            assert!(headers.starts_with("POST /exact/path?route=one HTTP/1.1"));
            assert!(
                headers
                    .lines()
                    .any(|line| line.eq_ignore_ascii_case("authorization: Bearer configured"))
            );
            write!(socket, "HTTP/1.1 307 Temporary Redirect\r\nLocation: {target_endpoint}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let mut capabilities = ModelCapabilities::jev_default();
        capabilities.model = "proxy-model".into();
        let error = evaluate_custom(
            &endpoint,
            Some("configured"),
            SystemOneResponseMode::Direct,
            &request("proxy-model"),
            &capabilities,
        )
        .await
        .unwrap_err();
        assert_eq!(error, JevError::RequestOutcomeUnknown);
        redirect.join().unwrap();
        assert!(
            matches!(target.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }
}

fn choice_mismatches(response: &JevResponse) -> (usize, f64) {
    response
        .answers
        .values()
        .fold((0, 0.0), |(count, gap), answer| {
            let JevAnswer::Choice {
                choice,
                probabilities,
                ..
            } = answer
            else {
                return (count, gap);
            };
            let Some(highest) = highest_probability_choice(choice, probabilities) else {
                return (count, gap);
            };
            if highest == choice {
                return (count, gap);
            }
            let difference = probabilities[highest] - probabilities[choice];
            (count + 1, gap.max(difference))
        })
}

fn retry_after_delay(value: &str, now_epoch: i64) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let deadline =
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc2822).ok()?;
    Some(Duration::from_secs(
        u64::try_from(deadline.unix_timestamp().saturating_sub(now_epoch).max(0)).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use antiburn_local::analysis::jev::{JevAnswer, JevQuestion, JevUsage, validate_jev_response};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::io::Read;

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        assert_eq!(retry_after_delay("12", 0), Some(Duration::from_secs(12)));
        let date = "Wed, 21 Oct 2015 07:28:00 GMT";
        let epoch =
            time::OffsetDateTime::parse(date, &time::format_description::well_known::Rfc2822)
                .unwrap()
                .unix_timestamp();
        assert_eq!(
            retry_after_delay(date, epoch - 12),
            Some(Duration::from_secs(12))
        );
        assert_eq!(retry_after_delay(date, epoch + 12), Some(Duration::ZERO));
        assert_eq!(retry_after_delay("invalid", epoch), None);
    }

    #[tokio::test]
    async fn async_transport_cancellation_closes_a_pending_response() {
        use std::io::Write as _;
        let (headers_sent, headers_received) = tokio::sync::oneshot::channel();
        let request = JevRequest {
            model: antiburn_local::analysis::jev::NATIVE_MODEL_ALIAS.to_owned(),
            state: json!({"text": "synthetic evidence"}),
            questions: BTreeMap::from([(
                "q".to_owned(),
                JevQuestion::Noul {
                    instructions: json!("Is this evidence sufficient?"),
                    criteria: None,
                },
            )]),
        };
        let body_bytes = serde_json::to_vec(&request).unwrap().len();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "{scheme}://{}/v1/systemone",
            listener.local_addr().unwrap(),
            scheme = "http"
        );
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut received = Vec::new();
            loop {
                let mut buffer = [0u8; 4096];
                let read = socket.read(&mut buffer).unwrap();
                assert!(read > 0);
                received.extend_from_slice(&buffer[..read]);
                if let Some(end) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                    && received.len() >= end + 4 + body_bytes
                {
                    break;
                }
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n")
                .unwrap();
            headers_sent.send(()).unwrap();
            let mut byte = [0u8; 1];
            match socket.read(&mut byte) {
                Ok(0) => {}
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                other => panic!("cancellation must close the connection: {other:?}"),
            }
        });
        let client = TypeSafeClient::new("synthetic-key".to_owned()).unwrap();
        let evaluation = tokio::spawn(async move { client.evaluate_at(&request, &endpoint).await });
        tokio::time::timeout(Duration::from_secs(10), headers_received)
            .await
            .expect("server receives request and sends response headers")
            .unwrap();
        assert!(!evaluation.is_finished(), "response body remains pending");
        evaluation.abort();
        assert!(evaluation.await.unwrap_err().is_cancelled());
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn typesafe_http_contract_preserves_the_request_and_validates_the_response() {
        use std::io::{Read as _, Write as _};
        let request = JevRequest {
            model: antiburn_local::analysis::jev::NATIVE_MODEL_ALIAS.to_owned(),
            state: json!({"text": "synthetic evidence"}),
            questions: BTreeMap::from([(
                "q".to_owned(),
                JevQuestion::Noul {
                    instructions: json!("Is this evidence sufficient?"),
                    criteria: None,
                },
            )]),
        };
        let response_body = serde_json::to_vec(&json!({
            "model": "jev-1.14.0",
            "answers": {"q": {"type": "noul", "noul": 0.2}},
            "usage": {"input_tokens": 12, "output_tokens": 1}
        }))
        .unwrap();
        let expected_body = serde_json::to_vec(&request).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/custom", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut received = Vec::new();
            loop {
                let mut buffer = [0u8; 4096];
                let read = socket.read(&mut buffer).unwrap();
                assert_ne!(read, 0);
                received.extend_from_slice(&buffer[..read]);
                let Some(header_end) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&received[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                if received.len() >= header_end + 4 + content_length {
                    assert!(headers.starts_with("POST /custom HTTP/1.1"));
                    assert!(headers.lines().any(|line| {
                        line.eq_ignore_ascii_case("authorization: Bearer synthetic-key")
                    }));
                    assert!(headers.lines().any(|line| line.eq_ignore_ascii_case("content-type: application/json")));
                    assert_eq!(
                        &received[header_end + 4..header_end + 4 + content_length],
                        expected_body
                    );
                    write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        response_body.len()
                    )
                    .unwrap();
                    socket.write_all(&response_body).unwrap();
                    break;
                }
            }
        });
        let client = TypeSafeClient::new("synthetic-key".to_owned()).unwrap();
        let response = client.evaluate_at(&request, &endpoint).await.unwrap();
        assert_eq!(response.usage.input_tokens, 12);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn known_http_errors_do_not_include_response_or_request_secrets() {
        use std::io::{Read as _, Write as _};
        let request = JevRequest {
            model: antiburn_local::analysis::jev::NATIVE_MODEL_ALIAS.to_owned(),
            state: json!({"text": "private synthetic request content"}),
            questions: BTreeMap::from([(
                "q".to_owned(),
                JevQuestion::Noul {
                    instructions: json!("Is this evidence sufficient?"),
                    criteria: None,
                },
            )]),
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request_bytes = [0u8; 4096];
            let _ = socket.read(&mut request_bytes).unwrap();
            socket.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 30\r\nConnection: close\r\n\r\nprovider-secret-response-body").unwrap();
        });
        let client = TypeSafeClient::new("synthetic-key".to_owned()).unwrap();
        let error = client.evaluate_at(&request, &endpoint).await.unwrap_err();
        let safe_error = format!("{error:?} {error}");
        assert!(matches!(error, JevError::AuthenticationRejected));
        assert!(!safe_error.contains("synthetic-key"));
        assert!(!safe_error.contains("private synthetic request content"));
        assert!(!safe_error.contains("provider-secret-response-body"));
        server.join().unwrap();
    }

    #[test]
    fn typed_response_decode_matches_the_validated_request_contract() {
        let request = JevRequest {
            model: "jev-1.13.0".to_owned(),
            state: json!({"event_id": "synthetic-event"}),
            questions: BTreeMap::from([(
                "issue".to_owned(),
                JevQuestion::Choice {
                    instructions: json!("Is this a conflict?"),
                    criteria: BTreeMap::from([
                        ("yes".to_owned(), json!("Conflict")),
                        ("no".to_owned(), json!("No conflict")),
                    ]),
                },
            )]),
        };
        let value = json!({
            "model": "jev-1.13.0",
            "answers": {
                "issue": {
                    "type": "choice",
                    "choice": "yes",
                    "probabilities": {"yes": 0.9, "no": 0.1},
                    "confidence": 0.8
                }
            },
            "usage": {"input_tokens": 12, "output_tokens": 1}
        });
        let response: JevResponse = serde_json::from_value(value).unwrap();
        assert_eq!(validate_jev_response(&response, &request), Ok(()));
        assert_eq!(
            response.usage,
            JevUsage {
                input_tokens: 12,
                output_tokens: 1
            }
        );
        assert!(
            matches!(response.answers.get("issue"), Some(JevAnswer::Choice { choice, .. }) if choice == "yes")
        );
        assert_eq!(choice_mismatches(&response), (0, 0.0));
        let mut inconsistent = response;
        if let Some(JevAnswer::Choice { choice, .. }) = inconsistent.answers.get_mut("issue") {
            *choice = "no".to_owned();
        }
        assert_eq!(validate_jev_response(&inconsistent, &request), Ok(()));
        let (count, gap) = choice_mismatches(&inconsistent);
        assert_eq!(count, 1);
        assert!((gap - 0.8).abs() < 0.000_001);
    }
}
