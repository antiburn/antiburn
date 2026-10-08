//! Cloudflare Workers AI transport for Clef and Clef-flash.

use std::time::Duration;

use antiburn_local::analysis::jev::{
    JevError, JevRequest, JevResponse, MAX_RESPONSE_BYTES,
    capabilities::{CapabilityLimit, CapabilitySource, ModelCapabilities, TokenizerIdentity},
    validate_jev_request_with_capabilities, validate_jev_response_with_capabilities,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const CLOUDFLARE_CONTEXT_TOKENS: u64 = 65_536;
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
        let endpoint = account_endpoint(&self.account_id, &self.model)?;
        let mut response = client
            .post(endpoint)
            .bearer_auth(self.api_token.as_ref())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|error| {
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
                .and_then(parse_retry_after);
            return Err(status_error(response.status().as_u16(), retry_after));
        }
        let maximum = usize::try_from(
            capabilities
                .response_body_bytes
                .value
                .unwrap_or(MAX_RESPONSE_BYTES as u64),
        )
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
        CloudflareAdapter::decode_response(&body, request, &self.model, capabilities)
    }
}

/// Return limits from published Clef metadata plus explicit conservative local guards.
/// Cloudflare's model pages publish the 65,536-token context and 64-question cap.
/// Their API schemas do not publish a tokenizer or state-specific token limit.
pub fn cloudflare_capabilities(model: &str) -> ModelCapabilities {
    ModelCapabilities {
        total_input_tokens: CapabilityLimit::known(
            CLOUDFLARE_CONTEXT_TOKENS,
            CapabilitySource::DocumentedDefault,
        ),
        state_and_longest_question_tokens: CapabilityLimit::known(
            32_768,
            CapabilitySource::DocumentedDefault,
        ),
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
        criteria_per_question: CapabilityLimit::known(26, CapabilitySource::DocumentedDefault),
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
    fn encode_request(
        request: &JevRequest,
        route_model: &str,
        capabilities: &ModelCapabilities,
    ) -> Result<Vec<u8>, JevError> {
        if request.model != route_model {
            return Err(JevError::UnsupportedModel);
        }
        validate_jev_request_with_capabilities(request, capabilities)?;
        let mut value =
            serde_json::to_value(request).map_err(|_| JevError::RequestSerialization)?;
        value["model"] = serde_json::Value::String(route_model.to_owned());
        serde_json::to_vec(&value).map_err(|_| JevError::RequestSerialization)
    }

    pub(crate) fn decode_response(
        body: &[u8],
        request: &JevRequest,
        route_model: &str,
        capabilities: &ModelCapabilities,
    ) -> Result<JevResponse, JevError> {
        let envelope: CloudflareEnvelope =
            serde_json::from_slice(body).map_err(|_| JevError::ResponseDecode)?;
        if !envelope.success {
            return Err(JevError::ProviderUnavailable);
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
        400 | 422 => JevError::InvalidRequestSchema,
        429 => JevError::RateLimited { retry_after },
        408 | 500..=599 => JevError::RequestOutcomeUnknown,
        _ => JevError::ProviderUnavailable,
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

    #[test]
    fn account_routes_and_short_body_model_match_both_supported_models() {
        for model in ["clef", "clef-flash"] {
            let request = request(model);
            let body =
                CloudflareAdapter::encode_request(&request, model, &cloudflare_capabilities(model))
                    .unwrap();
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["model"], model);
            assert_eq!(
                account_endpoint("account_123", model).unwrap(),
                format!(
                    "https://api.cloudflare.com/client/v4/accounts/account_123/ai/run/@cf/cloudflare/{model}"
                )
            );
        }
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
            Err(JevError::ProviderUnavailable)
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
            assert_eq!(
                metadata["models"][model]["context_window_tokens"],
                CLOUDFLARE_CONTEXT_TOKENS
            );
            assert_eq!(
                metadata["models"][model]["maximum_questions"],
                CLOUDFLARE_MAX_QUESTIONS
            );
            assert_eq!(capabilities.total_input_tokens.value, Some(65_536));
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
