//! Cloudflare Workers AI transport for Clef and Clef-flash.

use std::time::Duration;

use antiburn_local::analysis::jev::{
    JevError, JevRequest, JevResponse, JevUsage, JevUsageLocation, MAX_RESPONSE_BYTES,
    capabilities::{
        CapabilityLimit, CapabilitySource, ModelCapabilities,
        SystemOneCategoricalOutputCapabilities, SystemOneOutputCapabilities, TokenizerIdentity,
    },
    decode_jev_json_with_usage, validate_jev_request_with_capabilities,
    validate_jev_response_with_capabilities,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CLEF_CONTEXT_TOKENS: u64 = 65_536;
const CLEF_FLASH_CONTEXT_TOKENS: u64 = 24_576;
const CLOUDFLARE_MAX_QUESTIONS: u32 = 64;
const SAFE_STATE_BYTES: u64 = 30 * 1024;
const SAFE_BODY_BYTES: u64 = 60 * 1024;

/// Cloudflare credentials are kept private and are not included in debug output.
#[derive(Clone)]
pub struct CloudflareClient {
    account_id: std::sync::Arc<str>,
    api_token: std::sync::Arc<str>,
    model: String,
}

impl std::fmt::Debug for CloudflareClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CloudflareClient")
            .field("model", &self.model)
            .field("credentials", &"[redacted]")
            .finish()
    }
}

impl CloudflareClient {
    pub fn new(account_id: String, api_token: String, model: String) -> Result<Self, JevError> {
        if !valid_account_id(&account_id) || !valid_model(&model) {
            return Err(JevError::InvalidRequestSchema);
        }
        if api_token.trim().is_empty() {
            return Err(JevError::AuthenticationRejected);
        }
        Ok(Self {
            account_id: account_id.into(),
            api_token: api_token.into(),
            model,
        })
    }

    pub async fn evaluate(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        let capabilities = cloudflare_capabilities(&self.model);
        self.evaluate_with_capabilities(request, &capabilities)
            .await
    }

    pub(crate) async fn evaluate_with_capabilities(
        &self,
        request: &JevRequest,
        capabilities: &ModelCapabilities,
    ) -> Result<JevResponse, JevError> {
        self.evaluate_with_capabilities_and_usage(request, capabilities, |_| Ok(()))
            .await
    }

    pub(crate) async fn evaluate_with_capabilities_and_usage(
        &self,
        request: &JevRequest,
        capabilities: &ModelCapabilities,
        record_usage: impl Fn(JevUsage) -> Result<(), JevError>,
    ) -> Result<JevResponse, JevError> {
        let endpoint = account_endpoint(&self.account_id, &self.model)?;
        self.evaluate_at_with_usage(request, capabilities, &endpoint, record_usage)
            .await
    }

    async fn evaluate_at_with_usage(
        &self,
        request: &JevRequest,
        capabilities: &ModelCapabilities,
        endpoint: &str,
        record_usage: impl Fn(JevUsage) -> Result<(), JevError>,
    ) -> Result<JevResponse, JevError> {
        let body = CloudflareAdapter::encode_request(request, &self.model, capabilities)?;
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
        let mut response = client
            .post(endpoint)
            .bearer_auth(self.api_token.as_ref())
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
        if !response.status().is_success() {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_retry_after);
            return Err(status_error(response.status().as_u16(), retry_after));
        }
        let maximum = usize::try_from(
            capabilities
                .response_body_bytes
                .value
                .unwrap_or(MAX_RESPONSE_BYTES as u64),
        )
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
        CloudflareAdapter::decode_response_with_usage(
            &body,
            request,
            &self.model,
            capabilities,
            record_usage,
        )
    }
}

/// Return limits from published Clef metadata plus explicit conservative local guards.
/// Cloudflare publishes each route's context and a 64-question cap.
/// Their API schemas do not publish a tokenizer or state-specific token limit.
pub fn cloudflare_capabilities(model: &str) -> ModelCapabilities {
    let context = match model {
        "clef" => Some(CLEF_CONTEXT_TOKENS),
        "clef-flash" => Some(CLEF_FLASH_CONTEXT_TOKENS),
        _ => None,
    };
    ModelCapabilities {
        output: if valid_model(model) {
            SystemOneOutputCapabilities {
                choice: Some(SystemOneCategoricalOutputCapabilities {
                    probabilities: true,
                    confidence: true,
                    minimum_criteria: 2,
                    maximum_criteria: 26,
                }),
                score: Some(SystemOneCategoricalOutputCapabilities {
                    probabilities: true,
                    confidence: true,
                    minimum_criteria: 2,
                    maximum_criteria: 10,
                }),
                noul: true,
            }
        } else {
            SystemOneOutputCapabilities::default()
        },
        total_input_tokens: context.map_or_else(CapabilityLimit::unknown, |tokens| {
            CapabilityLimit::known(tokens, CapabilitySource::DocumentedDefault)
        }),
        state_and_longest_question_tokens: context
            .map_or_else(CapabilityLimit::unknown, |tokens| {
                CapabilityLimit::known(tokens.min(32_768), CapabilitySource::Manual)
            }),
        state_and_longest_question_bytes: CapabilityLimit::known(
            SAFE_STATE_BYTES,
            CapabilitySource::Manual,
        ),
        request_body_bytes: CapabilityLimit::known(SAFE_BODY_BYTES, CapabilitySource::Manual),
        response_body_bytes: CapabilityLimit::known(
            MAX_RESPONSE_BYTES as u64,
            CapabilitySource::Manual,
        ),
        runtime_context_tokens: CapabilityLimit::unknown(),
        questions_per_request: CapabilityLimit::known(
            CLOUDFLARE_MAX_QUESTIONS,
            CapabilitySource::DocumentedDefault,
        ),
        criteria_per_question: CapabilityLimit::known(26, CapabilitySource::Manual),
        rendering_reserve_tokens: 4_096,
        tokenizer: Some(TokenizerIdentity::ConservativeEstimator(
            antiburn_local::analysis::jev::capabilities::ASCII_WEIGHTED_ESTIMATOR.to_owned(),
        )),
        model: model.to_owned(),
        model_revision: None,
    }
}

pub fn account_endpoint(account_id: &str, model: &str) -> Result<String, JevError> {
    if !valid_account_id(account_id) || !valid_model(model) {
        return Err(JevError::InvalidRequestSchema);
    }
    Ok(format!(
        "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/run/@cf/cloudflare/{model}"
    ))
}

pub(crate) struct CloudflareAdapter;

impl CloudflareAdapter {
    pub(crate) fn encode_request(
        request: &JevRequest,
        route_model: &str,
        capabilities: &ModelCapabilities,
    ) -> Result<Vec<u8>, JevError> {
        Self::validate_request(request, route_model, capabilities)?;
        serde_json::to_vec(request).map_err(|_| JevError::RequestSerialization)
    }

    /// Checks the complete adapter contract without encoding or sending a request.
    pub(crate) fn validate_request(
        request: &JevRequest,
        route_model: &str,
        capabilities: &ModelCapabilities,
    ) -> Result<usize, JevError> {
        if request.model != route_model {
            return Err(JevError::UnsupportedModel);
        }
        let bytes = validate_jev_request_with_capabilities(request, capabilities)?;
        if request.questions.iter().any(|(id, question)| {
            let instructions = match question {
                antiburn_local::analysis::jev::JevQuestion::Choice { instructions, .. }
                | antiburn_local::analysis::jev::JevQuestion::Score { instructions, .. }
                | antiburn_local::analysis::jev::JevQuestion::Noul { instructions, .. } => {
                    instructions
                }
            };
            id.len() > 100
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
                || instructions.is_null()
        }) {
            return Err(JevError::InvalidRequestSchema);
        }
        Ok(bytes)
    }

    #[cfg(test)]
    fn decode_response(
        body: &[u8],
        request: &JevRequest,
        route_model: &str,
        capabilities: &ModelCapabilities,
    ) -> Result<JevResponse, JevError> {
        Self::decode_response_with_usage(body, request, route_model, capabilities, |_| Ok(()))
    }

    pub(crate) fn decode_response_with_usage(
        body: &[u8],
        request: &JevRequest,
        route_model: &str,
        capabilities: &ModelCapabilities,
        record_usage: impl Fn(JevUsage) -> Result<(), JevError>,
    ) -> Result<JevResponse, JevError> {
        let envelope: CloudflareEnvelope =
            decode_jev_json_with_usage(body, JevUsageLocation::CloudflareEnvelope, record_usage)?;
        if !envelope.success {
            return Err(JevError::RequestOutcomeUnknown);
        }
        let result = envelope.result.ok_or(JevError::ResponseDecode)?;
        if result.model != route_model || request.model != route_model {
            return Err(JevError::ResponseModelMismatch);
        }
        validate_jev_response_with_capabilities(&result, request, capabilities)?;
        Ok(result)
    }
}

#[derive(serde::Deserialize)]
struct CloudflareEnvelope {
    success: bool,
    #[serde(default)]
    result: Option<JevResponse>,
}

fn status_error(status: u16, retry_after: Option<Duration>) -> JevError {
    match status {
        401 | 403 => JevError::AuthenticationRejected,
        400 | 413 | 415 | 422 => JevError::InvalidRequestSchema,
        429 => JevError::RateLimited { retry_after },
        _ => JevError::RequestOutcomeUnknown,
    }
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let deadline =
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc2822).ok()?;
    Some(Duration::from_secs(
        u64::try_from(
            deadline
                .unix_timestamp()
                .saturating_sub(time::OffsetDateTime::now_utc().unix_timestamp())
                .max(0),
        )
        .ok()?,
    ))
}

fn valid_model(model: &str) -> bool {
    matches!(model, "clef" | "clef-flash")
}

fn valid_account_id(account_id: &str) -> bool {
    !account_id.is_empty()
        && account_id.len() <= 64
        && account_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use antiburn_local::analysis::jev::{JevQuestion, JevUsage};
    use serde_json::{Value, json};

    use super::*;

    fn request(model: &str) -> JevRequest {
        JevRequest {
            model: model.to_owned(),
            state: json!({"text": "synthetic evidence"}),
            questions: BTreeMap::from([(
                "q".to_owned(),
                JevQuestion::Noul {
                    instructions: json!("Is the evidence sufficient?"),
                    criteria: None,
                },
            )]),
        }
    }

    fn result(model: &str) -> Value {
        json!({
            "model": model,
            "answers": {"q": {"type": "noul", "noul": 0.2}},
            "usage": {"input_tokens": 12, "output_tokens": 1}
        })
    }

    #[tokio::test]
    async fn mixed_block_transport_keeps_one_request_and_rejects_ambiguous_json() {
        use std::io::{Read, Write};

        let mut request = request("clef");
        request.questions.insert(
            "choice".into(),
            JevQuestion::Choice {
                instructions: json!("Pick one"),
                criteria: BTreeMap::from([
                    ("yes".into(), json!("Yes")),
                    ("no".into(), json!("No")),
                ]),
            },
        );
        request.questions.insert(
            "score".into(),
            JevQuestion::Score {
                instructions: json!("Rate it"),
                criteria: vec![json!("low"), json!("high")],
            },
        );
        let result = r#"{"model":"clef","answers":{"choice":{"type":"choice","choice":"yes","probabilities":{"yes":0.1,"no":0.9},"confidence":0.8},"score":{"type":"score","score":0.3,"legend":{"0":"low","1":"high"},"probabilities":{"0":0.9,"1":0.1},"confidence":0.8},"q":{"type":"noul","noul":0.2}},"usage":{"input_tokens":12,"output_tokens":1}}"#;
        let valid = format!(r#"{{"success":true,"result":{result}}}"#);
        for (body, expected, known_usage) in [
            (valid.clone(), Ok(()), true),
            (valid.replace(r#""q":{"type":"noul","noul":0.2}"#, r#""q":{"type":"noul","noul":0.2},"q":{"type":"noul","noul":0.8}"#), Err(JevError::ResponseDecode), true),
            (valid.replace(r#""0":0.9"#, r#""0":0.9,"0":0.1"#), Err(JevError::ResponseDecode), true),
            (valid.replace(r#""0":"low""#, r#""0":"low","0":"high""#), Err(JevError::ResponseDecode), true),
            (valid.replace(r#""input_tokens":12"#, r#""input_tokens":12,"input_tokens":99"#), Err(JevError::ResponseDecode), false),
            (valid.replace(r#""usage":{"input_tokens":12,"output_tokens":1}"#, r#""usage":{"input_tokens":12,"output_tokens":1},"usage":{"input_tokens":99,"output_tokens":9}"#), Err(JevError::ResponseDecode), false),
            (format!(r#"{{"success":true,"result":{result},"result":{result}}}"#), Err(JevError::ResponseDecode), false),
            (format!(r#"{{"success":true,"success":false,"result":{result}}}"#), Err(JevError::ResponseDecode), false),
            (valid.replace(r#""noul":0.2"#, r#""noul":0.2,"confidence":0.9"#), Err(JevError::ResponseDecode), true),
            (valid.replace(r#""0":0.9"#, r#""01":0.9"#), Err(JevError::InvalidScoreDistribution), true),
            (valid.replace(r#""0":"low","1":"high""#, r#""0":"high","1":"low""#), Err(JevError::InvalidScoreDistribution), true),
            (valid.replace(r#""score":0.3"#, r#""score":-0.1"#), Err(JevError::InvalidScoreDistribution), true),
        ] {
            let expected_request = serde_json::to_vec(&request).unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut received = Vec::new();
                loop {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).unwrap();
                    assert!(count > 0);
                    received.extend_from_slice(&chunk[..count]);
                    if let Some(end) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                        && received.len() >= end + 4 + expected_request.len() {
                        let headers = String::from_utf8_lossy(&received[..end]).to_ascii_lowercase();
                        assert!(!headers.contains("typesafe-danger-zone-long-context:"));
                        assert_eq!(&received[end + 4..], expected_request);
                        break;
                    }
                }
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let calls = std::cell::Cell::new(0);
            let client = CloudflareClient::new("account".into(), "synthetic".into(), "clef".into()).unwrap();
            let actual = client.evaluate_at_with_usage(&request, &cloudflare_capabilities("clef"), &endpoint, |usage| {
                calls.set(calls.get() + 1);
                assert_eq!(usage, JevUsage { input_tokens: 12, output_tokens: 1 });
                Ok(())
            }).await;
            assert_eq!(actual.as_ref().map(|_| ()).map_err(Clone::clone), expected);
            assert_eq!(calls.get(), usize::from(known_usage));
            if let Ok(response) = actual {
                use antiburn_local::analysis::jev::JevAnswer;
                assert_eq!(response.answers.len(), 3);
                assert!(matches!(&response.answers["choice"], JevAnswer::Choice { choice, .. } if choice == "yes"));
                assert!(matches!(response.answers["score"], JevAnswer::Score { score, .. } if score == 0.3));
            }
            server.join().unwrap();
        }
    }

    #[test]
    fn account_routes_and_short_body_model_match_both_supported_models() {
        for model in ["clef", "clef-flash"] {
            let request = request(model);
            let body =
                CloudflareAdapter::encode_request(&request, model, &cloudflare_capabilities(model))
                    .unwrap();
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["model"], model);
            assert_eq!(body, serde_json::to_vec(&request).unwrap());
            assert_eq!(
                account_endpoint("account_123", model).unwrap(),
                format!(
                    "https://api.cloudflare.com/client/v4/accounts/account_123/ai/run/@cf/cloudflare/{model}"
                )
            );
        }
    }

    #[test]
    fn pure_validation_and_encoding_share_the_question_identifier_boundary() {
        for model in ["clef", "clef-flash"] {
            let capabilities = cloudflare_capabilities(model);
            for length in [100, 101] {
                let mut request = request(model);
                let question = request.questions.remove("q").unwrap();
                request.questions.insert("a".repeat(length), question);
                let validation =
                    CloudflareAdapter::validate_request(&request, model, &capabilities);
                let encoded = CloudflareAdapter::encode_request(&request, model, &capabilities);
                if length == 100 {
                    let encoded = encoded.unwrap();
                    assert_eq!(validation.unwrap(), encoded.len());
                    assert_eq!(encoded, serde_json::to_vec(&request).unwrap());
                } else {
                    assert_eq!(validation, Err(JevError::InvalidRequestSchema));
                    assert_eq!(encoded, Err(JevError::InvalidRequestSchema));
                }
            }
        }
    }

    #[test]
    fn pure_validation_and_encoding_share_all_adapter_request_rules() {
        let capabilities = cloudflare_capabilities("clef");
        let base = request("clef");
        let mut requests = Vec::new();
        for id in ["invalid/id", "non_ascii_é"] {
            let mut request = base.clone();
            let question = request.questions.remove("q").unwrap();
            request.questions.insert(id.into(), question);
            requests.push(request);
        }
        for question in [
            JevQuestion::Noul {
                instructions: Value::Null,
                criteria: None,
            },
            JevQuestion::Choice {
                instructions: Value::Null,
                criteria: BTreeMap::from([
                    ("yes".into(), json!("Yes.")),
                    ("no".into(), json!("No.")),
                ]),
            },
            JevQuestion::Score {
                instructions: Value::Null,
                criteria: vec![json!("Low."), json!("High.")],
            },
        ] {
            let mut request = base.clone();
            request.questions.insert("q".into(), question);
            requests.push(request);
        }
        for request in requests {
            assert_eq!(
                CloudflareAdapter::validate_request(&request, "clef", &capabilities),
                Err(JevError::InvalidRequestSchema)
            );
            assert_eq!(
                CloudflareAdapter::encode_request(&request, "clef", &capabilities),
                Err(JevError::InvalidRequestSchema)
            );
        }
        assert_eq!(
            CloudflareAdapter::validate_request(&base, "clef-flash", &capabilities),
            Err(JevError::UnsupportedModel)
        );
        let mut small = capabilities;
        small.request_body_bytes.value = Some(1);
        assert_eq!(
            CloudflareAdapter::validate_request(&base, "clef", &small).unwrap_err(),
            CloudflareAdapter::encode_request(&base, "clef", &small).unwrap_err()
        );
    }

    #[test]
    fn rejects_route_model_mismatch_before_send_and_after_response() {
        let request = request("clef");
        assert_eq!(
            CloudflareAdapter::encode_request(
                &request,
                "clef-flash",
                &cloudflare_capabilities("clef-flash")
            ),
            Err(JevError::UnsupportedModel)
        );
        let body = json!({"success": true, "result": result("clef-flash")}).to_string();
        assert_eq!(
            CloudflareAdapter::decode_response(
                body.as_bytes(),
                &request,
                "clef",
                &cloudflare_capabilities("clef")
            ),
            Err(JevError::ResponseModelMismatch)
        );
    }

    #[test]
    fn requires_valid_cloudflare_envelopes_and_strict_answers() {
        let request = request("clef");
        for body in ["not-json", "{}", r#"{"success":true,"result":{}}"#] {
            assert_eq!(
                CloudflareAdapter::decode_response(
                    body.as_bytes(),
                    &request,
                    "clef",
                    &cloudflare_capabilities("clef")
                ),
                Err(JevError::ResponseDecode)
            );
        }
        let failed =
            json!({"success": false, "errors": [{"message": "secret provider text"}] }).to_string();
        assert_eq!(
            CloudflareAdapter::decode_response(
                failed.as_bytes(),
                &request,
                "clef",
                &cloudflare_capabilities("clef")
            ),
            Err(JevError::RequestOutcomeUnknown)
        );
        let mut invalid = result("clef");
        invalid["answers"]["q"]["noul"] = json!(1.5);
        let body = json!({"success": true, "result": invalid}).to_string();
        assert_eq!(
            CloudflareAdapter::decode_response(
                body.as_bytes(),
                &request,
                "clef",
                &cloudflare_capabilities("clef")
            ),
            Err(JevError::InvalidNoulProbability)
        );
    }

    #[test]
    fn maps_authentication_rate_limits_and_safe_provider_errors() {
        assert_eq!(status_error(401, None), JevError::AuthenticationRejected);
        assert_eq!(status_error(403, None), JevError::AuthenticationRejected);
        assert_eq!(
            status_error(429, Some(Duration::from_secs(7))),
            JevError::RateLimited {
                retry_after: Some(Duration::from_secs(7))
            }
        );
        assert_eq!(status_error(500, None), JevError::RequestOutcomeUnknown);
        let error = status_error(401, None);
        assert!(!format!("{error:?} {error}").contains("token"));
    }

    #[test]
    fn defaults_track_published_metadata_and_guard_against_state_truncation() {
        let metadata: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/cloudflare/clef-model-metadata.json"
        ))
        .unwrap();
        for model in ["clef", "clef-flash"] {
            let capabilities = cloudflare_capabilities(model);
            let context = if model == "clef" { 65_536 } else { 24_576 };
            assert_eq!(metadata["models"][model]["context_window_tokens"], context);
            assert_eq!(
                metadata["models"][model]["maximum_questions"],
                CLOUDFLARE_MAX_QUESTIONS
            );
            assert_eq!(capabilities.total_input_tokens.value, Some(context));
            assert_eq!(
                capabilities.state_and_longest_question_tokens.value,
                Some(context.min(32_768))
            );
            assert_eq!(capabilities.usable_input_tokens(), Some(context - 4_096));
            assert_eq!(
                capabilities.usable_state_tokens(),
                Some(context.min(32_768) - 4_096)
            );
            assert_eq!(
                capabilities.total_input_tokens.source,
                CapabilitySource::DocumentedDefault
            );
            assert_eq!(capabilities.questions_per_request.value, Some(64));
            assert_eq!(
                capabilities.state_and_longest_question_bytes.value,
                Some(SAFE_STATE_BYTES)
            );
            assert_eq!(capabilities.request_body_bytes.value, Some(SAFE_BODY_BYTES));
            assert_eq!(capabilities.model_revision, None);
            let output = capabilities.output;
            let choice = output.choice.unwrap();
            assert_eq!(
                choice.maximum_criteria,
                metadata["output_contract"]["local_choice_criteria"][1]
            );
            assert!(choice.probabilities && choice.confidence);
            let score = output.score.unwrap();
            assert_eq!(
                score.maximum_criteria,
                metadata["output_contract"]["local_score_criteria"][1]
            );
            assert!(score.probabilities && score.confidence);
        }
        let too_many_questions = (0..=CLOUDFLARE_MAX_QUESTIONS)
            .map(|index| {
                (
                    format!("q{index}"),
                    JevQuestion::Noul {
                        instructions: json!("Synthetic check"),
                        criteria: None,
                    },
                )
            })
            .collect();
        let request = JevRequest {
            model: "clef".to_owned(),
            state: json!("synthetic"),
            questions: too_many_questions,
        };
        assert_eq!(
            validate_jev_request_with_capabilities(&request, &cloudflare_capabilities("clef")),
            Err(JevError::QuestionLimitExceeded)
        );

        let oversized = JevRequest {
            model: "clef".to_owned(),
            state: json!("x".repeat(SAFE_STATE_BYTES as usize)),
            questions: BTreeMap::from([(
                "q".to_owned(),
                JevQuestion::Noul {
                    instructions: json!("Synthetic check"),
                    criteria: None,
                },
            )]),
        };
        assert!(matches!(
            validate_jev_request_with_capabilities(&oversized, &cloudflare_capabilities("clef")),
            Err(JevError::RequestTooLarge { .. })
        ));
    }

    #[test]
    fn flash_rejects_input_that_fits_clef_but_exceeds_its_documented_context() {
        for (model, fits) in [("clef", true), ("clef-flash", false)] {
            let request = JevRequest {
                model: model.into(),
                state: json!("x".repeat(28 * 1024)),
                questions: BTreeMap::from([(
                    "q".into(),
                    JevQuestion::Noul {
                        instructions: json!("Synthetic check"),
                        criteria: None,
                    },
                )]),
            };
            let capabilities = cloudflare_capabilities(model);
            let result = CloudflareAdapter::encode_request(&request, model, &capabilities);
            if fits {
                assert!(result.is_ok());
            } else {
                assert!(matches!(
                    result,
                    Err(JevError::RequestTokenLimitExceeded {
                        maximum: 20_480,
                        ..
                    })
                ));
            }
        }
    }

    #[test]
    fn valid_result_preserves_normalized_usage() {
        let request = request("clef-flash");
        let body =
            json!({"success": true, "result": result("clef-flash"), "errors": []}).to_string();
        let response = CloudflareAdapter::decode_response(
            body.as_bytes(),
            &request,
            "clef-flash",
            &cloudflare_capabilities("clef-flash"),
        )
        .unwrap();
        assert_eq!(
            response.usage,
            JevUsage {
                input_tokens: 12,
                output_tokens: 1
            }
        );
    }

    #[tokio::test]
    async fn reported_usage_precedes_cloudflare_answer_decode_and_validation() {
        use std::io::{Read, Write};

        for (answer, callback_error, expected) in [
            (
                json!({"type": "noul", "noul": "invalid"}),
                false,
                JevError::ResponseDecode,
            ),
            (
                json!({"type": "noul", "noul": 1.5}),
                false,
                JevError::InvalidNoulProbability,
            ),
            (
                json!({"type": "noul", "noul": 0.2}),
                true,
                JevError::ProgressStorageFailure,
            ),
        ] {
            let body = json!({"success": true, "result": {
                "model": "clef", "answers": {"q": answer},
                "usage": {"input_tokens": 12, "output_tokens": 1}
            }})
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
            let client =
                CloudflareClient::new("account".into(), "synthetic".into(), "clef".into()).unwrap();
            let actual = client
                .evaluate_at_with_usage(
                    &request("clef"),
                    &cloudflare_capabilities("clef"),
                    &endpoint,
                    |usage| {
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
                    },
                )
                .await;
            assert_eq!(actual, Err(expected));
            assert_eq!(recorded.get(), 1);
            server.join().unwrap();
        }
    }

    #[test]
    fn output_claims_are_explicit_and_unknown_routes_have_no_claim() {
        for model in ["clef", "clef-flash"] {
            let output = cloudflare_capabilities(model).output;
            assert!(output.noul);
            let choice = output.choice.unwrap();
            assert!(choice.probabilities && choice.confidence);
            assert_eq!((choice.minimum_criteria, choice.maximum_criteria), (2, 26));
            let score = output.score.unwrap();
            assert!(score.probabilities && score.confidence);
            assert_eq!((score.minimum_criteria, score.maximum_criteria), (2, 10));
        }
        assert_eq!(cloudflare_capabilities("other").output, Default::default());
        for status in [301, 307, 408, 500, 503, 529] {
            assert_eq!(status_error(status, None), JevError::RequestOutcomeUnknown);
        }
    }

    #[test]
    fn rejects_missing_credentials_and_does_not_debug_print_credentials() {
        assert!(matches!(
            CloudflareClient::new("account".into(), "  ".into(), "clef".into()),
            Err(JevError::AuthenticationRejected)
        ));
        let client =
            CloudflareClient::new("account".into(), "synthetic-secret".into(), "clef".into())
                .unwrap();
        assert!(!format!("{client:?}").contains("synthetic-secret"));
        let _transport = CloudflareClient::evaluate;
        assert_eq!(parse_retry_after("9"), Some(Duration::from_secs(9)));
    }

    #[tokio::test]
    #[ignore = "requires CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_AUTH_TOKEN"]
    async fn live_clef_models_accept_system_one_requests() {
        let account_id = std::env::var("CLOUDFLARE_ACCOUNT_ID").unwrap();
        let api_token = std::env::var("CLOUDFLARE_AUTH_TOKEN").unwrap();
        for model in ["clef", "clef-flash"] {
            let client =
                CloudflareClient::new(account_id.clone(), api_token.clone(), model.to_owned())
                    .unwrap();
            let request = request(model);
            let response = client.evaluate(&request).await.unwrap();
            assert_eq!(response.model, model);
            assert!(response.answers.contains_key("q"));
        }
    }
}
