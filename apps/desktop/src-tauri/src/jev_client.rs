//! Bounded TypeSafe System One transport.

#[cfg(test)]
use std::io::Read;
use std::time::{Duration, Instant};

use crate::jev_config::system_one_endpoint;
use antiburn_local::analysis::jev::{
    JevAnswer, JevError, JevRequest, JevResponse, MAX_RESPONSE_BYTES, highest_probability_choice,
    validate_jev_request, validate_jev_response,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

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
        validate_jev_request(request)?;
        static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
        let client = CLIENT.get_or_init(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
            reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("a client with no custom TLS material always builds")
        });
        let send_started = Instant::now();
        let mut response = client
            .post(endpoint)
            .bearer_auth(self.api_key.as_ref())
            .json(request)
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
            return Err(match response.status().as_u16() {
                401 | 403 => JevError::AuthenticationRejected,
                400 | 422 => JevError::InvalidRequestSchema,
                429 => JevError::RateLimited { retry_after },
                529 => JevError::ProviderOverloaded { retry_after },
                408 | 500..=599 => JevError::RequestOutcomeUnknown,
                _ => JevError::ProviderUnavailable,
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
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
            if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(body.len()) {
                return Err(JevError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let body_elapsed_ms = body_started.elapsed().as_millis();
        let validation_started = Instant::now();
        let response = serde_json::from_slice(&body).map_err(|_| JevError::ResponseDecode)?;
        validate_jev_response(&response, request)?;
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
        validate_jev_request(request)?;
        let send_started = Instant::now();
        let response = client()
            .post(system_one_endpoint())
            .bearer_auth(self.api_key.as_ref())
            .json(request)
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
            return Err(match status.as_u16() {
                401 | 403 => JevError::AuthenticationRejected,
                400 | 422 => JevError::InvalidRequestSchema,
                429 => JevError::RateLimited { retry_after },
                529 => JevError::ProviderOverloaded { retry_after },
                408 | 500..=599 => JevError::RequestOutcomeUnknown,
                _ => JevError::ProviderUnavailable,
            });
        }
        let body_started = Instant::now();
        let response = read_bounded_response(response)?;
        let body_elapsed_ms = body_started.elapsed().as_millis();
        let validation_started = Instant::now();
        let response: JevResponse =
            serde_json::from_slice(&response).map_err(|_| JevError::ResponseDecode)?;
        validate_jev_response(&response, request)?;
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
fn read_bounded_response(mut response: reqwest::blocking::Response) -> Result<Vec<u8>, JevError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(JevError::ResponseTooLarge);
    }
    let mut body = Vec::with_capacity(MAX_RESPONSE_BYTES.min(16 * 1024));
    response
        .by_ref()
        .take(MAX_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|_| JevError::RequestOutcomeUnknown)?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(JevError::ResponseTooLarge);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use antiburn_local::analysis::jev::{JevAnswer, JevQuestion, JevUsage};
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
