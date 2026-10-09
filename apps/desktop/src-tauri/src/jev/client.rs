//! Bounded TypeSafe System One transport.

#[cfg(test)]
use std::io::Read;
use std::time::{Duration, Instant};

use crate::jev::config::SystemOneResponseMode;
use crate::jev::config::system_one_endpoint;
use antiburn_local::analysis::jev::{
    JevAnswer, JevError, JevRequest, JevResponse, MAX_RESPONSE_BYTES,
    capabilities::ModelCapabilities, highest_probability_choice,
    validate_jev_request_with_capabilities, validate_jev_response_with_capabilities,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

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
    pub(crate) async fn evaluate_async(
        &self,
        request: &JevRequest,
    ) -> Result<JevResponse, JevError> {
        self.evaluate_at(request, &system_one_endpoint()).await
    }

    async fn evaluate_at(
        &self,
        request: &JevRequest,
        endpoint: &str,
    ) -> Result<JevResponse, JevError> {
        self.evaluate_at_with_capabilities(request, endpoint, &ModelCapabilities::jev_default())
            .await
    }

    pub(crate) async fn evaluate_at_with_capabilities(
        &self,
        request: &JevRequest,
        endpoint: &str,
        capabilities: &ModelCapabilities,
    ) -> Result<JevResponse, JevError> {
        let body = JevSystemOneAdapter::encode_request(request, capabilities)?;
        let send_started = Instant::now();
        let mut response = async_client()
            .post(endpoint)
            .bearer_auth(self.api_key.as_ref())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
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
            .unwrap_or(MAX_RESPONSE_BYTES);
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
        let response = JevSystemOneAdapter::decode_response(&body, request, capabilities)?;
        let (mismatch_count, largest_gap) = choice_mismatches(&response);
        if mismatch_count > 0 {
            ::tracing::warn!(event = "typesafe_choice_probability_mismatch", model = %response.model,
                question_count = request.questions.len(), mismatch_count, largest_probability_gap = largest_gap,
                input_tokens = response.usage.input_tokens, output_tokens = response.usage.output_tokens);
        }
        ::tracing::debug!(event = "typesafe_request_timing", model = %response.model,
            question_count = request.questions.len(), input_tokens = response.usage.input_tokens,
            output_tokens = response.usage.output_tokens, send_elapsed_ms, body_elapsed_ms,
            validation_elapsed_ms = validation_started.elapsed().as_millis());
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

    #[cfg(test)]
    pub(crate) fn evaluate(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        let capabilities = ModelCapabilities::jev_default();
        let request_body = JevSystemOneAdapter::encode_request(request, &capabilities)?;
        let send_started = Instant::now();
        let response = client()
            .post(system_one_endpoint())
            .bearer_auth(self.api_key.as_ref())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_body)
            .send()
            .map_err(|error| {
                if error.is_builder() {
                    JevError::InvalidRequestSchema
                } else if error.is_timeout() {
                    JevError::RequestOutcomeUnknown
                } else if error.is_connect() {
                    JevError::ProviderUnavailable
                } else {
                    JevError::RequestOutcomeUnknown
                }
            })?;
        let send_elapsed_ms = send_started.elapsed().as_millis();
        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| {
                    retry_after_delay(value, time::OffsetDateTime::now_utc().unix_timestamp())
                });
            return Err(JevSystemOneAdapter::status_error(
                status.as_u16(),
                retry_after,
            ));
        }
        let body_started = Instant::now();
        let maximum_response_bytes = capabilities
            .response_body_bytes
            .value
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(MAX_RESPONSE_BYTES);
        let response = read_bounded_response(response, maximum_response_bytes)?;
        let body_elapsed_ms = body_started.elapsed().as_millis();
        let validation_started = Instant::now();
        let response = JevSystemOneAdapter::decode_response(&response, request, &capabilities)?;
        let validation_elapsed_ms = validation_started.elapsed().as_millis();
        let (mismatch_count, largest_gap) = choice_mismatches(&response);
        if mismatch_count > 0 {
            ::tracing::warn!(
                event = "typesafe_choice_probability_mismatch",
                model = %response.model,
                question_count = request.questions.len(),
                mismatch_count,
                largest_probability_gap = largest_gap,
                input_tokens = response.usage.input_tokens,
                output_tokens = response.usage.output_tokens,
            );
        }
        ::tracing::debug!(
            event = "typesafe_request_timing",
            model = %response.model,
            question_count = request.questions.len(),
            input_tokens = response.usage.input_tokens,
            output_tokens = response.usage.output_tokens,
            send_elapsed_ms,
            body_elapsed_ms,
            validation_elapsed_ms,
        );
        Ok(response)
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

    pub(crate) fn decode_response(
        body: &[u8],
        request: &JevRequest,
        capabilities: &ModelCapabilities,
    ) -> Result<JevResponse, JevError> {
        let response = serde_json::from_slice(body).map_err(|_| JevError::ResponseDecode)?;
        validate_jev_response_with_capabilities(&response, request, capabilities)?;
        Ok(response)
    }

    fn status_error(status: u16, retry_after: Option<Duration>) -> JevError {
        match status {
            401 | 403 => JevError::AuthenticationRejected,
            400 | 422 => JevError::InvalidRequestSchema,
            429 => JevError::RateLimited { retry_after },
            529 => JevError::ProviderOverloaded { retry_after },
            408 | 500..=599 => JevError::RequestOutcomeUnknown,
            _ => JevError::ProviderUnavailable,
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
    let body = JevSystemOneAdapter::encode_request(request, capabilities)?;
    let mut builder = async_client()
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body);
    if let Some(credential) = credential {
        builder = builder.bearer_auth(credential);
    }
    let mut response = builder.send().await.map_err(|error| {
        if error.is_connect() {
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
        return Err(JevSystemOneAdapter::status_error(
            response.status().as_u16(),
            retry_after,
        ));
    }
    let maximum = capabilities
        .response_body_bytes
        .value
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(MAX_RESPONSE_BYTES);
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
        SystemOneResponseMode::Direct => {
            JevSystemOneAdapter::decode_response(&body, request, capabilities)
        }
        SystemOneResponseMode::CloudflareEnvelope => {
            crate::jev_cloudflare::CloudflareAdapter::decode_response(
                &body,
                request,
                &request.model,
                capabilities,
            )
        }
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
        assert_eq!(error, JevError::ProviderUnavailable);
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
fn client() -> &'static reqwest::blocking::Client {
    static CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("a client with no custom TLS material always builds")
    })
}

#[cfg(test)]
fn read_bounded_response(
    mut response: reqwest::blocking::Response,
    maximum_bytes: usize,
) -> Result<Vec<u8>, JevError> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum_bytes as u64)
    {
        return Err(JevError::ResponseTooLarge);
    }
    let mut body = Vec::with_capacity(maximum_bytes.min(16 * 1024));
    response
        .by_ref()
        .take(maximum_bytes as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|_| JevError::RequestOutcomeUnknown)?;
    if body.len() > maximum_bytes {
        return Err(JevError::ResponseTooLarge);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use antiburn_local::analysis::jev::{JevAnswer, JevQuestion, JevUsage, validate_jev_response};
    use serde_json::json;
    use std::collections::BTreeMap;

    use super::*;

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
            model: antiburn_local::analysis::jev::PINNED_MODEL.to_owned(),
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
            model: antiburn_local::analysis::jev::PINNED_MODEL.to_owned(),
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
            "model": antiburn_local::analysis::jev::PINNED_MODEL,
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
            model: antiburn_local::analysis::jev::PINNED_MODEL.to_owned(),
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

    #[tokio::test]
    async fn blocking_transport_is_created_on_a_blocking_thread() {
        let client = TypeSafeClient::new("synthetic-key".to_owned()).unwrap();
        tokio::task::spawn_blocking(move || {
            drop(client);
            assert!(std::ptr::eq(super::client(), super::client()));
        })
        .await
        .unwrap();
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
