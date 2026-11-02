//! Ollama's direct System One transport and conservative local discovery.

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
use serde::{Deserialize, Serialize};

pub const MAX_REQUEST_BODY_BYTES: usize = crate::jev::config::OLLAMA_MAX_REQUEST_BODY_BYTES;
pub const DEFAULT_LOCAL_CONCURRENCY: usize = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const IDENTITY_CHECK_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DISCOVERY_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_INSTALLED_MODELS: usize = 1024;

fn inference_slots() -> &'static tokio::sync::Semaphore {
    static SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| tokio::sync::Semaphore::new(DEFAULT_LOCAL_CONCURRENCY))
}

async fn acquire_inference_slot() -> Result<tokio::sync::SemaphorePermit<'static>, OllamaError> {
    let acquire = inference_slots().acquire();
    #[cfg(test)]
    let acquire = test_hooks::observe_inference_slot_pending(acquire);
    acquire.await.map_err(|_| OllamaError::ProviderUnavailable)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OllamaError {
    InvalidBaseUrl,
    OldVersion,
    MissingModel,
    UnsupportedRunner,
    ContextRejected,
    RequestBodyTooLarge,
    AuthenticationRejected,
    ProviderUnavailable,
    RequestOutcomeUnknown,
    InvalidRequest,
    ResponseDecode,
    ResponseTooLarge,
}

impl std::fmt::Display for OllamaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidBaseUrl => "Invalid Ollama base URL",
            Self::OldVersion => "Ollama must be version 0.35.0 or newer",
            Self::MissingModel => "The selected Ollama model is not installed",
            Self::UnsupportedRunner => {
                "This Ollama model is not supported. Select an installed local System One model"
            }
            Self::ContextRejected => "The Ollama model rejected the request context",
            Self::RequestBodyTooLarge => "Ollama text requests cannot exceed 64 KiB",
            Self::AuthenticationRejected => "Ollama rejected the configured authentication",
            Self::ProviderUnavailable => "Ollama is unavailable",
            Self::RequestOutcomeUnknown => "The Ollama request outcome is unknown",
            Self::InvalidRequest => "The Ollama request is invalid",
            Self::ResponseDecode => "Ollama returned an invalid System One response",
            Self::ResponseTooLarge => "The Ollama response exceeded the local limit",
        })
    }
}

impl std::error::Error for OllamaError {}

#[derive(Clone)]
pub struct OllamaClient {
    base_url: String,
    api_key: Option<std::sync::Arc<str>>,
    http: reqwest::Client,
}

impl std::fmt::Debug for OllamaClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OllamaClient")
            .field("connection", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiscoveredModel {
    pub name: String,
    pub digest: Option<String>,
    pub capabilities: ModelCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledOllamaModel {
    pub id: String,
    pub label: String,
}

impl OllamaClient {
    pub fn new(base_url: &str, api_key: Option<String>) -> Result<Self, OllamaError> {
        Self::with_http(base_url, api_key, shared_http_client().clone())
    }

    #[cfg(test)]
    fn with_timeouts(
        base_url: &str,
        api_key: Option<String>,
        connect_timeout: Duration,
        request_timeout: Duration,
    ) -> Result<Self, OllamaError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .timeout(request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| OllamaError::ProviderUnavailable)?;
        Self::with_http(base_url, api_key, http)
    }

    fn with_http(
        base_url: &str,
        api_key: Option<String>,
        http: reqwest::Client,
    ) -> Result<Self, OllamaError> {
        let parsed = url::Url::parse(base_url).map_err(|_| OllamaError::InvalidBaseUrl)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(OllamaError::InvalidBaseUrl);
        }
        let base_url = base_url.trim_end_matches('/').to_owned();
        Ok(Self {
            base_url,
            api_key: api_key.filter(|key| !key.trim().is_empty()).map(Into::into),
            http,
        })
    }

    pub fn inference_endpoint(&self) -> String {
        format!("{}/v1/systemone", self.base_url)
    }

    pub async fn list_installed_models(&self) -> Result<Vec<InstalledOllamaModel>, OllamaError> {
        let tags: TagsResponse = self.get_json("/api/tags").await?;
        if tags.models.len() > MAX_INSTALLED_MODELS {
            return Err(OllamaError::ResponseTooLarge);
        }
        let mut names = std::collections::BTreeSet::new();
        let mut models = Vec::new();
        for tag in tags.models {
            if remote_model(
                &tag.name,
                tag.remote_model.as_deref(),
                tag.remote_host.as_deref(),
            ) {
                continue;
            }
            if !valid_model_digest(&tag.digest)
                || !crate::jev::config::valid_model_name(&tag.name)
                || !names.insert(tag.name.clone())
            {
                return Err(OllamaError::ResponseDecode);
            }
            models.push(InstalledOllamaModel {
                id: tag.name.clone(),
                label: tag.name,
            });
        }
        Ok(models)
    }

    pub async fn evaluate(
        &self,
        request: &JevRequest,
        capabilities: &ModelCapabilities,
    ) -> Result<JevResponse, OllamaError> {
        let bytes = self.evaluate_bytes(request, capabilities).await?;
        let decoded: JevResponse =
            decode_jev_json_with_usage(&bytes, JevUsageLocation::Direct, |_| Ok(()))
                .map_err(|_| OllamaError::ResponseDecode)?;
        validate_jev_response_with_capabilities(&decoded, request, capabilities)
            .map_err(|_| OllamaError::ResponseDecode)?;
        Ok(decoded)
    }

    /// Records reported usage before decoding or validating typed answers.
    pub async fn evaluate_with_usage(
        &self,
        request: &JevRequest,
        capabilities: &ModelCapabilities,
        record_usage: impl Fn(JevUsage) -> Result<(), JevError>,
    ) -> Result<JevResponse, JevError> {
        self.evaluate_with_usage_cancellable(request, capabilities, || false, || {}, record_usage)
            .await
    }

    /// Checks cancellation before inference send.
    /// Reports possible inference delivery after local admission and identity checks.
    pub(crate) async fn evaluate_with_usage_cancellable(
        &self,
        request: &JevRequest,
        capabilities: &ModelCapabilities,
        is_cancelled: impl Fn() -> bool,
        on_send: impl FnOnce(),
        record_usage: impl Fn(JevUsage) -> Result<(), JevError>,
    ) -> Result<JevResponse, JevError> {
        if is_cancelled() {
            return Err(JevError::Cancelled);
        }
        let _permit = acquire_inference_slot().await.map_err(jev_error)?;
        if is_cancelled() {
            return Err(JevError::Cancelled);
        }
        let builder = self
            .evaluation_request(request, capabilities)
            .map_err(jev_error)?;
        if is_cancelled() {
            return Err(JevError::Cancelled);
        }
        let identity = self
            .verify_pre_send_identity(&request.model, capabilities)
            .await;
        if is_cancelled() {
            return Err(JevError::Cancelled);
        }
        identity.map_err(jev_error)?;
        on_send();
        let bytes = Self::send_evaluation(builder, capabilities)
            .await
            .map_err(jev_error)?;
        let decoded = decode_jev_json_with_usage(&bytes, JevUsageLocation::Direct, record_usage)?;
        validate_jev_response_with_capabilities(&decoded, request, capabilities)?;
        Ok(decoded)
    }

    async fn evaluate_bytes(
        &self,
        request: &JevRequest,
        capabilities: &ModelCapabilities,
    ) -> Result<Vec<u8>, OllamaError> {
        let _permit = acquire_inference_slot().await?;
        let builder = self.evaluation_request(request, capabilities)?;
        self.verify_pre_send_identity(&request.model, capabilities)
            .await?;
        Self::send_evaluation(builder, capabilities).await
    }

    // This read rejects known tag changes. A tag can still change after the read.
    async fn verify_pre_send_identity(
        &self,
        model: &str,
        capabilities: &ModelCapabilities,
    ) -> Result<(), OllamaError> {
        let expected = capabilities
            .model_revision
            .as_deref()
            .filter(|digest| valid_model_digest(digest))
            .ok_or(OllamaError::UnsupportedRunner)?;
        if capabilities.model != model || !crate::jev::config::valid_model_name(model) {
            return Err(OllamaError::UnsupportedRunner);
        }
        let read_tags = async {
            let mut request = self.http.get(format!("{}/api/tags", self.base_url));
            if let Some(key) = &self.api_key {
                request = request.bearer_auth(key.as_ref());
            }
            let response = request
                .send()
                .await
                .map_err(|_| OllamaError::ProviderUnavailable)?;
            if matches!(response.status().as_u16(), 401 | 403) {
                return Err(OllamaError::AuthenticationRejected);
            }
            // A metadata failure cannot make inference delivery or usage uncertain.
            decode_json::<TagsResponse>(response)
                .await
                .map_err(|_| OllamaError::ProviderUnavailable)
        };
        let tags = tokio::time::timeout(IDENTITY_CHECK_TIMEOUT, read_tags)
            .await
            .map_err(|_| OllamaError::ProviderUnavailable)??;
        if tags.models.len() > MAX_INSTALLED_MODELS {
            return Err(OllamaError::ProviderUnavailable);
        }
        let identity = canonical_model_name(model);
        let mut matching = tags
            .models
            .iter()
            .filter(|tag| canonical_model_name(&tag.name) == identity);
        let tag = matching.next().ok_or(OllamaError::MissingModel)?;
        if matching.next().is_some()
            || !crate::jev::config::valid_model_name(&tag.name)
            || !valid_model_digest(&tag.digest)
        {
            return Err(OllamaError::ProviderUnavailable);
        }
        if tag.digest != expected
            || remote_model(
                &tag.name,
                tag.remote_model.as_deref(),
                tag.remote_host.as_deref(),
            )
        {
            return Err(OllamaError::UnsupportedRunner);
        }
        Ok(())
    }

    fn evaluation_request(
        &self,
        request: &JevRequest,
        capabilities: &ModelCapabilities,
    ) -> Result<reqwest::RequestBuilder, OllamaError> {
        validate_jev_request_with_capabilities(request, capabilities)
            .map_err(|_| OllamaError::InvalidRequest)?;
        let body = serde_json::to_vec(request).map_err(|_| OllamaError::InvalidRequest)?;
        if body.len() > MAX_REQUEST_BODY_BYTES {
            return Err(OllamaError::RequestBodyTooLarge);
        }
        let mut builder = self
            .http
            .post(self.inference_endpoint())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key.as_ref());
        }
        Ok(builder)
    }

    async fn send_evaluation(
        builder: reqwest::RequestBuilder,
        capabilities: &ModelCapabilities,
    ) -> Result<Vec<u8>, OllamaError> {
        let response = builder.send().await.map_err(|error| {
            if error.is_builder() {
                OllamaError::InvalidRequest
            } else if error.is_connect() {
                OllamaError::ProviderUnavailable
            } else {
                OllamaError::RequestOutcomeUnknown
            }
        })?;
        let status = response.status();
        let maximum = capabilities
            .response_body_bytes
            .value
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(MAX_RESPONSE_BYTES)
            .min(MAX_RESPONSE_BYTES);
        let bytes = read_bounded(response, maximum).await?;
        if !status.is_success() {
            return Err(classify_error(status.as_u16(), &bytes));
        }
        Ok(bytes)
    }

    /// Load a verified model once if its effective input limits remain unknown.
    pub async fn discover_ready(
        &self,
        discovered: DiscoveredModel,
    ) -> Result<DiscoveredModel, OllamaError> {
        if (discovered.capabilities.usable_input_tokens().is_some()
            && discovered.capabilities.usable_state_tokens().is_some())
            || discovered.capabilities.output == SystemOneOutputCapabilities::default()
        {
            return Ok(discovered);
        }
        let _permit = acquire_inference_slot().await?;
        self.verify_pre_send_identity(&discovered.name, &discovered.capabilities)
            .await?;
        let _: serde_json::Value = self
            .post_json(
                "/api/generate",
                &serde_json::json!({"model": discovered.name, "prompt": "", "stream": false}),
            )
            .await?;
        let ready = self.discover(&discovered.name).await?;
        if ready.name != discovered.name || ready.digest != discovered.digest {
            return Err(OllamaError::UnsupportedRunner);
        }
        Ok(ready)
    }

    pub async fn discover(&self, model: &str) -> Result<DiscoveredModel, OllamaError> {
        let version: VersionResponse = self.get_json("/api/version").await?;
        if !version_at_least(&version.version, (0, 35, 0)) {
            return Err(OllamaError::OldVersion);
        }
        let tags: TagsResponse = self.get_json("/api/tags").await?;
        let identity = canonical_model_name(model);
        let Some(tag) = tags
            .models
            .into_iter()
            .find(|candidate| canonical_model_name(&candidate.name) == identity)
        else {
            return Err(OllamaError::MissingModel);
        };
        if remote_model(
            &tag.name,
            tag.remote_model.as_deref(),
            tag.remote_host.as_deref(),
        ) {
            return Err(OllamaError::UnsupportedRunner);
        }
        if tag.digest.trim().is_empty() {
            return Err(OllamaError::UnsupportedRunner);
        }
        let show: ShowResponse = self
            .post_json("/api/show", &serde_json::json!({"model": model}))
            .await?;
        if remote_model(
            model,
            show.remote_model.as_deref(),
            show.remote_host.as_deref(),
        ) {
            return Err(OllamaError::UnsupportedRunner);
        }
        if !show
            .capabilities
            .iter()
            .any(|capability| capability == "decision")
        {
            return Err(OllamaError::UnsupportedRunner);
        }
        let ps: PsResponse = self.get_json("/api/ps").await?;
        if ps.models.iter().any(|entry| {
            canonical_model_name(&entry.name) == identity
                && remote_model(
                    &entry.name,
                    entry.remote_model.as_deref(),
                    entry.remote_host.as_deref(),
                )
        }) {
            return Err(OllamaError::UnsupportedRunner);
        }
        let loaded = ps.models.iter().find(|entry| {
            canonical_model_name(&entry.name) == identity
                && entry
                    .digest
                    .as_ref()
                    .is_some_and(|digest| digest == &tag.digest)
        });
        let configured_context = show.parameters.as_deref().and_then(parse_num_ctx);
        let loaded_context = loaded.and_then(|entry| entry.context_length);
        let runtime_context_tokens = loaded_context
            .map_or_else(CapabilityLimit::unknown, |value| {
                CapabilityLimit::known(value, CapabilitySource::RuntimeMetadata)
            });
        let model_context = show
            .model_info
            .as_ref()
            .and_then(|info| {
                info.get("general.architecture")
                    .and_then(serde_json::Value::as_str)
            })
            .and_then(|arch| {
                show.model_info
                    .as_ref()?
                    .get(format!("{arch}.context_length"))?
                    .as_u64()
            });
        let selected_context = loaded_context.or(configured_context);
        let cap = selected_context
            .map(|context| model_context.map_or(context, |maximum| maximum.min(context)));
        let source = if loaded_context.is_some() && cap == loaded_context {
            CapabilitySource::RuntimeMetadata
        } else {
            CapabilitySource::ProviderMetadata
        };
        let mut capabilities = ModelCapabilities {
            output: verified_output_support(&version.version, &show)?,
            total_input_tokens: cap.map_or_else(CapabilityLimit::unknown, |value| {
                CapabilityLimit::known(value, source)
            }),
            state_and_longest_question_tokens: cap.map_or_else(CapabilityLimit::unknown, |value| {
                CapabilityLimit::known(value, source)
            }),
            state_and_longest_question_bytes: CapabilityLimit::unknown(),
            request_body_bytes: CapabilityLimit::known(
                MAX_REQUEST_BODY_BYTES as u64,
                CapabilitySource::DocumentedDefault,
            ),
            response_body_bytes: CapabilityLimit::known(
                MAX_RESPONSE_BYTES as u64,
                CapabilitySource::DocumentedDefault,
            ),
            runtime_context_tokens,
            questions_per_request: CapabilityLimit::known(64, CapabilitySource::DocumentedDefault),
            criteria_per_question: CapabilityLimit::known(26, CapabilitySource::DocumentedDefault),
            rendering_reserve_tokens: 1024,
            tokenizer: Some(TokenizerIdentity::ConservativeEstimator(
                antiburn_local::analysis::jev::capabilities::ASCII_WEIGHTED_ESTIMATOR.into(),
            )),
            model: model.to_owned(),
            model_revision: Some(tag.digest.clone()),
        };
        if let Some(envelope) = verified_tev1_chat_envelope(&version.version, &show) {
            capabilities.use_ollama_tev1_accounting(envelope, show.system.as_deref().unwrap_or(""));
        } else if let Some(envelope) = verified_generic_chat_envelope(&version.version, &show) {
            capabilities
                .use_ollama_generic_accounting(envelope, show.system.as_deref().unwrap_or(""));
        }
        Ok(DiscoveredModel {
            name: model.to_owned(),
            digest: Some(tag.digest),
            capabilities,
        })
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, OllamaError> {
        let mut request = self.http.get(format!("{}{path}", self.base_url));
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key.as_ref());
        }
        let response = request
            .send()
            .await
            .map_err(|_| OllamaError::ProviderUnavailable)?;
        decode_json(response).await
    }

    async fn post_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, OllamaError> {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base_url))
            .json(body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key.as_ref());
        }
        decode_json(
            request
                .send()
                .await
                .map_err(|_| OllamaError::ProviderUnavailable)?,
        )
        .await
    }
}

fn shared_http_client() -> &'static reqwest::Client {
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

async fn decode_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, OllamaError> {
    let status = response.status();
    let body = read_bounded(response, MAX_DISCOVERY_BODY_BYTES).await?;
    if !status.is_success() {
        return Err(classify_error(status.as_u16(), &body));
    }
    serde_json::from_slice(&body).map_err(|_| OllamaError::ProviderUnavailable)
}

async fn read_bounded(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, OllamaError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(OllamaError::ResponseTooLarge);
    }
    let mut body = Vec::with_capacity(limit.min(16 * 1024));
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| OllamaError::RequestOutcomeUnknown)?
    {
        if chunk.len() > limit.saturating_sub(body.len()) {
            return Err(OllamaError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn classify_error(status: u16, body: &[u8]) -> OllamaError {
    let message = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(serde_json::Value::as_str)
                .map(str::to_ascii_lowercase)
        })
        .unwrap_or_default();
    if status == 401 || status == 403 {
        OllamaError::AuthenticationRejected
    } else if status == 404 && (message.contains("not found") || message.contains("pull")) {
        OllamaError::MissingModel
    } else if status == 400
        && (message.contains("decision")
            || message.contains("runner")
            || message.contains("unsupported"))
    {
        OllamaError::UnsupportedRunner
    } else if status == 400
        && ((message.contains("context")
            && (message.contains("requires")
                || message.contains("exceed")
                || message.contains("too long")))
            || (message.starts_with("prompt ")
                && message.contains(" tokens; expected 1–")
                && message.contains("input is never truncated")))
    {
        OllamaError::ContextRejected
    } else if status == 413 {
        OllamaError::RequestBodyTooLarge
    } else if status == 400 || status == 422 {
        OllamaError::InvalidRequest
    } else {
        OllamaError::RequestOutcomeUnknown
    }
}

pub(crate) fn jev_error(error: OllamaError) -> JevError {
    match error {
        OllamaError::AuthenticationRejected => JevError::AuthenticationRejected,
        OllamaError::RequestOutcomeUnknown => JevError::RequestOutcomeUnknown,
        OllamaError::ResponseTooLarge => JevError::ResponseTooLarge,
        OllamaError::ResponseDecode => JevError::ResponseDecode,
        OllamaError::ContextRejected => JevError::ContextRejected,
        OllamaError::InvalidBaseUrl
        | OllamaError::InvalidRequest
        | OllamaError::RequestBodyTooLarge => JevError::InvalidRequestSchema,
        OllamaError::OldVersion | OllamaError::MissingModel | OllamaError::UnsupportedRunner => {
            JevError::UnsupportedModel
        }
        OllamaError::ProviderUnavailable => JevError::ProviderUnavailable,
    }
}

fn parse_num_ctx(parameters: &str) -> Option<u64> {
    parameters.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        (fields.next()? == "num_ctx")
            .then(|| fields.next()?.parse().ok())
            .flatten()
    })
}

fn canonical_model_name(name: &str) -> String {
    let (path, model) = name.rsplit_once('/').unwrap_or(("", name));
    let (model, tag) = model.split_once(':').unwrap_or((model, "latest"));
    let (host, namespace) = match path.rsplit_once('/') {
        Some(parts) => parts,
        None => (
            "registry.ollama.ai",
            if path.is_empty() { "library" } else { path },
        ),
    };
    format!("{host}/{namespace}/{model}:{tag}")
}

fn valid_model_digest(digest: &str) -> bool {
    !digest.trim().is_empty() && digest.len() <= 256 && !digest.chars().any(char::is_control)
}

fn remote_model(name: &str, remote_model: Option<&str>, remote_host: Option<&str>) -> bool {
    remote_model.is_some_and(|value| !value.trim().is_empty())
        || remote_host.is_some_and(|value| !value.trim().is_empty())
        || name
            .rsplit_once(':')
            .is_some_and(|(_, tag)| tag == "cloud" || tag.ends_with("-cloud"))
}

fn version_at_least(version: &str, minimum: (u64, u64, u64)) -> bool {
    let Some(version) = version.strip_prefix('v').or(Some(version)) else {
        return false;
    };
    let mut components = version
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok());
    let found = (components.next(), components.next(), components.next());
    match found {
        (Some(a), Some(b), Some(c)) => (a, b, c) >= minimum,
        _ => false,
    }
}

#[derive(Deserialize)]
struct VersionResponse {
    version: String,
}
#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<TagModel>,
}
#[derive(Deserialize)]
struct TagModel {
    name: String,
    digest: String,
    #[serde(default)]
    remote_model: Option<String>,
    #[serde(default)]
    remote_host: Option<String>,
}
#[derive(Deserialize)]
struct PsResponse {
    models: Vec<PsModel>,
}
#[derive(Deserialize)]
struct PsModel {
    name: String,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    remote_model: Option<String>,
    #[serde(default)]
    remote_host: Option<String>,
}
#[derive(Deserialize)]
struct ShowResponse {
    #[serde(default)]
    remote_model: Option<String>,
    #[serde(default)]
    remote_host: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    parameters: Option<String>,
    #[serde(default)]
    model_info: Option<serde_json::Value>,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    system: Option<String>,
    #[serde(default)]
    modelfile: Option<String>,
}

fn verified_output_support(
    version: &str,
    show: &ShowResponse,
) -> Result<SystemOneOutputCapabilities, OllamaError> {
    if version.strip_prefix('v').unwrap_or(version) != "0.40.1" {
        return Ok(SystemOneOutputCapabilities::default());
    }
    let info = show.model_info.as_ref();
    let encoding = info.and_then(|info| {
        info.get("decision.type").or_else(|| {
            let architecture = info.get("general.architecture")?.as_str()?;
            info.get(format!("{architecture}.decision.type"))
        })
    });
    if encoding.is_some_and(|value| !value.is_string()) {
        return Ok(SystemOneOutputCapabilities::default());
    }
    let encoding = encoding.and_then(serde_json::Value::as_str);
    let renderer = show.modelfile.as_deref().and_then(|modelfile| {
        modelfile.lines().find_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next()? == "RENDERER")
                .then(|| fields.next())
                .flatten()
        })
    });
    let encoding = encoding
        .filter(|encoding| !encoding.is_empty())
        .or(renderer);
    let maximum =
        match encoding {
            Some("tev1") => 24,
            Some("clef") => 26,
            Some("strands") => return Ok(SystemOneOutputCapabilities::default()),
            Some(_) => return Err(OllamaError::UnsupportedRunner),
            None if show.modelfile.as_deref().is_some_and(|modelfile| {
                modelfile.lines().any(|line| line.starts_with("FROM "))
            }) && info
                .and_then(|info| info.get("general.architecture"))
                .and_then(serde_json::Value::as_str)
                .is_some() =>
            {
                26
            }
            None => return Ok(SystemOneOutputCapabilities::default()),
        };
    let categorical = SystemOneCategoricalOutputCapabilities {
        probabilities: true,
        confidence: true,
        minimum_criteria: 2,
        maximum_criteria: maximum,
    };
    Ok(SystemOneOutputCapabilities {
        choice: Some(categorical),
        score: Some(categorical),
        noul: true,
    })
}

// Pin the native Qwen3.5 template returned for the characterized Tev GGUF.
// The text-only envelope follows Ollama v0.40.1 model/renderers/qwen35.go
// and the native template's non-thinking generation prefix.
const TEV1_QWEN35_TEMPLATE_SHA256: &str =
    "d78de6bee4c952ca3145eb161921560a6ede7b59a34e7e4be815f3c5386b4364";
const TEV1_QWEN35_CHAT_ENVELOPE: &str = "<|im_start|>system\n<|im_end|>\n<|im_start|>user\n<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n";

fn verified_tev1_chat_envelope(version: &str, show: &ShowResponse) -> Option<&'static str> {
    if version.strip_prefix('v').unwrap_or(version) != "0.40.1" {
        return None;
    }
    let encoding = show.model_info.as_ref().and_then(|info| {
        info.get("decision.type")
            .or_else(|| {
                let architecture = info.get("general.architecture")?.as_str()?;
                info.get(format!("{architecture}.decision.type"))
            })
            .and_then(serde_json::Value::as_str)
    });
    if encoding != Some("tev1") {
        return None;
    }
    verified_chat_envelope(show)
}

fn verified_generic_chat_envelope(version: &str, show: &ShowResponse) -> Option<&'static str> {
    if version.strip_prefix('v').unwrap_or(version) != "0.40.1" {
        return None;
    }
    // routes.go reads decision.type, then Config.Renderer. Require the full
    // Modelfile before treating absent decision metadata as the generic encoding.
    let info = show.model_info.as_ref()?.as_object()?;
    let architecture = info.get("general.architecture")?.as_str()?;
    if info
        .get("decision.type")
        .is_some_and(|value| value.as_str() != Some(""))
        || info
            .get(&format!("{architecture}.decision.type"))
            .is_some_and(|value| value.as_str() != Some(""))
    {
        return None;
    }
    let modelfile = show.modelfile.as_deref()?;
    if !modelfile.lines().any(|line| line.starts_with("FROM "))
        || modelfile.lines().any(|line| {
            line.trim_start().starts_with("RENDERER ") || line.trim_start().starts_with("PARSER ")
        })
    {
        return None;
    }
    verified_chat_envelope(show)
}

fn verified_chat_envelope(show: &ShowResponse) -> Option<&'static str> {
    if show.modelfile.as_deref().is_some_and(|modelfile| {
        modelfile.lines().any(|line| {
            line.trim_start().starts_with("RENDERER ") || line.trim_start().starts_with("PARSER ")
        })
    }) {
        return None;
    }
    let template = show.template.as_deref()?;
    match template.trim() {
        "{{ .Prompt }}" | "{{.Prompt}}" => Some(""),
        _ => {
            use sha2::{Digest, Sha256};
            let digest: String = Sha256::digest(template.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            (digest == TEV1_QWEN35_TEMPLATE_SHA256).then_some(TEV1_QWEN35_CHAT_ENVELOPE)
        }
    }
}

#[cfg(test)]
#[path = "jev_ollama/preflight_tests.rs"]
mod preflight_tests;

#[cfg(test)]
pub(crate) mod test_hooks {
    use std::future::{Future, poll_fn};
    use std::sync::Arc;

    tokio::task_local! {
        static INFERENCE_SLOT_PENDING: Arc<dyn Fn() + Send + Sync>;
    }

    pub(crate) async fn hold_inference_slots() -> tokio::sync::SemaphorePermit<'static> {
        super::inference_slots()
            .acquire_many(super::DEFAULT_LOCAL_CONCURRENCY as u32)
            .await
            .unwrap()
    }

    pub(crate) async fn with_inference_slot_pending<T>(
        on_pending: impl Fn() + Send + Sync + 'static,
        run: impl Future<Output = T>,
    ) -> T {
        INFERENCE_SLOT_PENDING
            .scope(Arc::new(on_pending), run)
            .await
    }

    pub(super) async fn observe_inference_slot_pending<T>(acquire: impl Future<Output = T>) -> T {
        let mut on_pending = INFERENCE_SLOT_PENDING.try_with(Arc::clone).ok();
        tokio::pin!(acquire);
        poll_fn(|cx| {
            let result = acquire.as_mut().poll(cx);
            if result.is_pending()
                && let Some(on_pending) = on_pending.take()
            {
                on_pending();
                // Test the permit-release race before the outer cancellation timer can poll.
                return acquire.as_mut().poll(cx);
            }
            result
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use antiburn_local::analysis::jev::{JevQuestion, PINNED_MODEL};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn transport_capabilities() -> ModelCapabilities {
        ModelCapabilities {
            model_revision: Some("synthetic-digest".into()),
            ..ModelCapabilities::jev_default()
        }
    }

    fn reply_to_identity_check(listener: &std::net::TcpListener, credential: Option<&str>) {
        use std::io::{Read, Write};

        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut received = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).unwrap();
            assert!(count > 0);
            received.extend_from_slice(&buffer[..count]);
            assert!(received.len() < 8192);
            if received.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                break;
            }
        }
        let headers = String::from_utf8(received).unwrap().to_ascii_lowercase();
        assert!(headers.starts_with("get /api/tags http/1.1\r\n"));
        match credential {
            Some(credential) => {
                assert!(headers.contains(&format!("authorization: bearer {credential}")))
            }
            None => assert!(!headers.contains("authorization:")),
        }
        let body =
            json!({"models": [{"name": PINNED_MODEL, "digest": "synthetic-digest"}]}).to_string();
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    }

    #[test]
    fn remote_model_classification_uses_metadata_and_explicit_cloud_tags() {
        for name in ["clef:cloud", "library/clef:cloud", "model:120b-cloud"] {
            assert!(remote_model(name, None, None));
        }
        assert!(remote_model("renamed:latest", Some("clef:cloud"), None));
        assert!(remote_model(
            "renamed:latest",
            None,
            Some("https://remote.example")
        ));
        for name in ["clef", "clef:latest", "clef:9b", "cloud-local:latest"] {
            assert!(!remote_model(name, None, None));
            assert!(!remote_model(name, Some(""), Some("")));
        }
        assert_eq!(
            jev_error(OllamaError::UnsupportedRunner),
            JevError::UnsupportedModel
        );
    }

    #[tokio::test]
    async fn installed_model_lookup_reads_only_tags_and_excludes_remote_models() {
        let body = json!({"models": [
            {"name": "selected:9b", "digest": "local-digest"},
            {"name": "completion-model:latest", "digest": "other-local-digest"},
            {"name": "clef:cloud", "digest": "remote-digest"},
            {"name": "model:120b-cloud", "digest": "remote-digest"},
            {"name": "renamed:latest", "digest": "remote-digest", "remote_model": "clef:cloud"},
            {"name": "hosted:latest", "digest": "remote-digest", "remote_host": "https://remote.example"}
        ]}).to_string();
        for credential in [None, Some("synthetic-token")] {
            let models = lookup_mock_models(200, body.clone(), None, credential, false)
                .await
                .unwrap();
            assert_eq!(
                models,
                vec![
                    InstalledOllamaModel {
                        id: "selected:9b".into(),
                        label: "selected:9b".into()
                    },
                    InstalledOllamaModel {
                        id: "completion-model:latest".into(),
                        label: "completion-model:latest".into()
                    },
                ]
            );
        }
    }

    #[tokio::test]
    async fn installed_model_lookup_distinguishes_empty_metadata_and_bounded_failures() {
        for body in [
            json!({"models": []}),
            json!({"models": [{"name": "clef:cloud", "digest": "remote"}]}),
        ] {
            assert!(
                lookup_mock_models(200, body.to_string(), None, None, false)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        for (body, expected) in [
            ("not json".into(), OllamaError::ProviderUnavailable),
            (json!({"models": {}}).to_string(), OllamaError::ProviderUnavailable),
            (json!({"models": [{"name": "local:latest"}]}).to_string(), OllamaError::ProviderUnavailable),
            (json!({"models": [{"name": "local:latest", "digest": ""}]}).to_string(), OllamaError::ResponseDecode),
            (json!({"models": [{"name": "bad name", "digest": "digest"}]}).to_string(), OllamaError::ResponseDecode),
            (json!({"models": [{"name": "local:latest", "digest": "digest\n"}]}).to_string(), OllamaError::ResponseDecode),
            (json!({"models": [{"name": "local:latest", "digest": "digest"}, {"name": "local:latest", "digest": "other"}]}).to_string(), OllamaError::ResponseDecode),
        ] {
            assert_eq!(lookup_mock_models(200, body, None, None, false).await, Err(expected));
        }
        assert_eq!(
            lookup_mock_models(401, "{}".into(), None, Some("synthetic-token"), false).await,
            Err(OllamaError::AuthenticationRejected)
        );
        assert_eq!(
            lookup_mock_models(
                200,
                String::new(),
                Some(MAX_DISCOVERY_BODY_BYTES + 1),
                None,
                false
            )
            .await,
            Err(OllamaError::ResponseTooLarge)
        );
        assert_eq!(
            lookup_mock_models(200, String::new(), None, None, true).await,
            Err(OllamaError::ProviderUnavailable)
        );
        for count in [MAX_INSTALLED_MODELS, MAX_INSTALLED_MODELS + 1] {
            let body = json!({"models": (0..count).map(|index| json!({"name": format!("local:{index}"), "digest": "synthetic-digest"})).collect::<Vec<_>>()}).to_string();
            let result = lookup_mock_models(200, body, None, None, false).await;
            if count == MAX_INSTALLED_MODELS {
                assert_eq!(result.unwrap().len(), count);
            } else {
                assert_eq!(result, Err(OllamaError::ResponseTooLarge));
            }
        }
    }

    async fn lookup_mock_models(
        status: u16,
        body: String,
        content_length: Option<usize>,
        credential: Option<&str>,
        stall: bool,
    ) -> Result<Vec<InstalledOllamaModel>, OllamaError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        tokio::time::timeout(Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = OllamaClient::with_timeouts(
                &format!("http://{}/ollama", listener.local_addr().unwrap()),
                credential.map(str::to_owned), Duration::from_secs(1),
                if stall { Duration::from_millis(50) } else { Duration::from_secs(2) },
            ).unwrap();
            let credential = credential.map(str::to_owned);
            let (done, completed) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                    assert!(request.len() <= 8192);
                    if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") { break; }
                }
                let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                assert!(request.starts_with("get /ollama/api/tags http/1.1\r\n"));
                assert!(!request.contains("typesafe-danger-zone-long-context:"));
                match credential {
                    Some(secret) => assert!(request.contains(&format!("authorization: bearer {secret}"))),
                    None => assert!(!request.contains("authorization:")),
                }
                if stall {
                    completed.await.unwrap();
                    return;
                }
                socket.write_all(format!("HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", content_length.unwrap_or(body.len())).as_bytes()).await.unwrap();
                tokio::select! {
                    extra = listener.accept() => panic!("Model lookup sent another request: {extra:?}"),
                    _ = completed => {}
                }
            });
            let result = client.list_installed_models().await;
            done.send(()).unwrap();
            server.await.unwrap();
            result
        }).await.unwrap()
    }

    #[tokio::test]
    async fn mixed_blocks_use_one_unchanged_send_and_strict_decode_with_or_without_usage() {
        use antiburn_local::analysis::jev::JevAnswer;
        use std::io::{Read, Write};

        let caps = transport_capabilities();
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
        for with_usage in [true, false] {
            for (body, expected, known_usage) in [
                (valid.clone(), Ok(()), true),
                (valid.replace(r#""noul":{"type":"noul","noul":0.2}"#, r#""noul":{"type":"noul","noul":0.2},"noul":{"type":"noul","noul":0.9}"#), Err(JevError::ResponseDecode), true),
                (valid.replace(r#""0":0.9"#, r#""0":0.9,"0":0.1"#), Err(JevError::ResponseDecode), true),
                (valid.replace(r#""0":"low""#, r#""0":"low","0":"high""#), Err(JevError::ResponseDecode), true),
                (valid.replace(r#""input_tokens":12"#, r#""input_tokens":12,"input_tokens":99"#), Err(JevError::ResponseDecode), false),
                (valid.replace(r#""usage":{"input_tokens":12,"output_tokens":1}"#, r#""usage":{"input_tokens":12,"output_tokens":1},"usage":{"input_tokens":99,"output_tokens":9}"#), Err(JevError::ResponseDecode), false),
                (valid.replace(r#""noul":0.2"#, r#""noul":0.2,"confidence":0.9"#), Err(JevError::ResponseDecode), true),
                (valid.replace(r#""0":0.9"#, r#""00":0.9"#), Err(JevError::InvalidScoreDistribution), true),
                (valid.replace(r#""0":"low","1":"high""#, r#""0":"high","1":"low""#), Err(JevError::InvalidScoreDistribution), true),
                (valid.replace(r#""score":0.3"#, r#""score":1.1"#), Err(JevError::InvalidScoreDistribution), true),
            ] {
                let expected_request = serde_json::to_vec(&request).unwrap();
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let endpoint = format!("http://{}", listener.local_addr().unwrap());
                let server = std::thread::spawn(move || {
                    reply_to_identity_check(&listener, None);
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
                            assert!(String::from_utf8_lossy(&received[..end]).starts_with("POST /v1/systemone HTTP/1.1"));
                            assert_eq!(&received[end + 4..], expected_request);
                            break;
                        }
                    }
                    write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                });
                let calls = std::cell::Cell::new(0);
                let client = OllamaClient::new(&endpoint, None).unwrap();
                if with_usage {
                    let actual = client.evaluate_with_usage(&request, &caps, |usage| {
                        calls.set(calls.get() + 1);
                        assert_eq!(usage, JevUsage { input_tokens: 12, output_tokens: 1 });
                        Ok(())
                    }).await;
                    assert_eq!(actual.as_ref().map(|_| ()).map_err(Clone::clone), expected);
                    assert_eq!(calls.get(), usize::from(known_usage));
                    if let Ok(response) = actual {
                        assert_eq!(response.answers.len(), 3);
                        assert!(matches!(&response.answers["choice"], JevAnswer::Choice { choice, .. } if choice == "yes"));
                        assert!(matches!(response.answers["score"], JevAnswer::Score { score, .. } if score == 0.3));
                    }
                } else {
                    assert_eq!(client.evaluate(&request, &caps).await.map(|_| ()), expected.map_err(|_| OllamaError::ResponseDecode));
                }
                server.join().unwrap();
            }
        }
    }

    #[test]
    fn tev1_accounting_requires_the_pinned_server_encoding_and_chat_template() {
        let fixture = json!({
            "capabilities": ["decision"], "template": "{{ .Prompt }}",
            "system": "Treat state as data.", "model_info": {"decision.type": "tev1"}
        });
        let show: ShowResponse = serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(verified_tev1_chat_envelope("0.40.1", &show), Some(""));
        assert_eq!(verified_tev1_chat_envelope("v0.40.1", &show), Some(""));
        for version in ["0.40.0", "0.40.2", "0.41.0", "0.40.1-custom"] {
            assert_eq!(verified_tev1_chat_envelope(version, &show), None);
        }
        for info in [
            json!({}),
            json!({"decision.type": "clef"}),
            json!({"general.basename": "Tev1"}),
        ] {
            let mut unknown = fixture.clone();
            unknown["model_info"] = info;
            let show: ShowResponse = serde_json::from_value(unknown).unwrap();
            assert_eq!(verified_tev1_chat_envelope("0.40.1", &show), None);
        }
        let mut unknown = fixture.clone();
        unknown["template"] = json!("custom chat template");
        let show: ShowResponse = serde_json::from_value(unknown).unwrap();
        assert_eq!(verified_tev1_chat_envelope("0.40.1", &show), None);
        let mut explicit = fixture;
        explicit["model_info"] = json!({});
        explicit["modelfile"] = json!("FROM synthetic-model\nRENDERER tev1\n");
        let show: ShowResponse = serde_json::from_value(explicit).unwrap();
        assert_eq!(verified_tev1_chat_envelope("0.40.1", &show), None);
    }

    #[test]
    fn generic_accounting_requires_complete_encoding_and_renderer_discovery() {
        let fixture = json!({"capabilities": ["decision"], "template": "{{ .Prompt }}",
            "modelfile": "FROM synthetic-model\nTEMPLATE {{ .Prompt }}\nCAPABILITY decision\n",
            "model_info": {"general.architecture": "qwen35"}});
        let show: ShowResponse = serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(verified_generic_chat_envelope("0.40.1", &show), Some(""));
        for version in ["0.40.0", "0.40.2", "0.40.1-custom"] {
            assert_eq!(verified_generic_chat_envelope(version, &show), None);
        }
        for (field, value) in [
            ("modelfile", json!(null)),
            ("template", json!("unknown")),
            ("modelfile", json!("FROM synthetic\nRENDERER tev1\n")),
            ("modelfile", json!("FROM synthetic\nRENDERER clef\n")),
            ("modelfile", json!("FROM synthetic\nPARSER unknown\n")),
            (
                "model_info",
                json!({"general.architecture": "qwen35", "decision.type": "tev1"}),
            ),
            (
                "model_info",
                json!({"general.architecture": "qwen35", "qwen35.decision.type": "clef"}),
            ),
        ] {
            let mut changed = fixture.clone();
            changed[field] = value;
            let show: ShowResponse = serde_json::from_value(changed).unwrap();
            assert_eq!(verified_generic_chat_envelope("0.40.1", &show), None);
        }
    }

    #[test]
    fn output_support_requires_a_pinned_runner_and_verified_encoding() {
        for (encoding, maximum) in [("tev1", 24), ("clef", 26)] {
            let show: ShowResponse = serde_json::from_value(json!({
                "capabilities": ["decision"], "model_info": {"decision.type": encoding}
            }))
            .unwrap();
            let output = verified_output_support("0.40.1", &show).unwrap();
            assert!(output.noul);
            for categorical in [output.choice.unwrap(), output.score.unwrap()] {
                assert!(categorical.probabilities && categorical.confidence);
                assert_eq!(
                    (categorical.minimum_criteria, categorical.maximum_criteria),
                    (2, maximum)
                );
            }
            for version in ["0.35.0", "0.40.2", "0.40.1-custom"] {
                assert_eq!(
                    verified_output_support(version, &show).unwrap(),
                    Default::default()
                );
            }
        }
        let show: ShowResponse = serde_json::from_value(json!({
            "capabilities": ["decision"], "model_info": {"decision.type": "unknown"}
        }))
        .unwrap();
        assert_eq!(
            verified_output_support("0.40.1", &show),
            Err(OllamaError::UnsupportedRunner)
        );
        let show: ShowResponse =
            serde_json::from_value(json!({"capabilities": ["decision"]})).unwrap();
        assert_eq!(
            verified_output_support("0.40.1", &show).unwrap(),
            Default::default()
        );
        for encoding in [json!(null), json!(42), json!({"name": "tev1"})] {
            let show: ShowResponse = serde_json::from_value(json!({
                "capabilities": ["decision"], "modelfile": "FROM synthetic\n",
                "model_info": {"general.architecture": "qwen35", "decision.type": encoding}
            }))
            .unwrap();
            assert_eq!(
                verified_output_support("0.40.1", &show).unwrap(),
                Default::default()
            );
        }
    }

    #[tokio::test]
    async fn reported_usage_precedes_ollama_answer_decode_and_validation() {
        use std::io::{Read, Write};

        let caps = transport_capabilities();
        let request = JevRequest {
            model: caps.model.clone(),
            state: json!("synthetic"),
            questions: BTreeMap::from([(
                "q".into(),
                JevQuestion::Noul {
                    instructions: json!("Is the evidence sufficient?"),
                    criteria: None,
                },
            )]),
        };
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
            let body = json!({"model": caps.model, "answers": {"q": answer},
                "usage": {"input_tokens": 12, "output_tokens": 1}})
            .to_string();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                reply_to_identity_check(&listener, None);
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
            let actual = OllamaClient::new(&endpoint, None)
                .unwrap()
                .evaluate_with_usage(&request, &caps, |usage| {
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
                })
                .await;
            assert_eq!(actual, Err(expected));
            assert_eq!(recorded.get(), 1);
            server.join().unwrap();
        }
    }

    #[test]
    fn uncertain_server_errors_are_not_safe_retries_and_debug_is_redacted() {
        for status in [307, 408, 500, 503] {
            for body in [
                br#"{"error":"out of memory"}"#.as_slice(),
                br#"{"error":"runner failed after scoring"}"#,
            ] {
                assert_eq!(
                    classify_error(status, body),
                    OllamaError::RequestOutcomeUnknown
                );
            }
        }
        let client =
            OllamaClient::new("http://private.example/local", Some("secret".into())).unwrap();
        assert!(!format!("{client:?}").contains("secret"));
        assert!(!format!("{client:?}").contains("private.example"));
    }

    #[tokio::test]
    async fn tev1_discovery_uses_runtime_context_and_measured_reserve_without_model_name_rules() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client =
            OllamaClient::new(&format!("http://{}", listener.local_addr().unwrap()), None).unwrap();
        let server = tokio::spawn(async move {
            for (context, generic) in [(2048, false), (2050, false), (2048, true), (2050, true)] {
                for (route, body) in [
                    ("/api/version", json!({"version": "0.40.1"})),
                    (
                        "/api/tags",
                        json!({"models": [{"name": "renamed-decision:latest", "digest": "synthetic-digest"}]}),
                    ),
                    (
                        "/api/show",
                        json!({"capabilities": ["decision"], "template": "{{ .Prompt }}",
                        "modelfile": "FROM synthetic-model\nTEMPLATE {{ .Prompt }}\nCAPABILITY decision\n",
                        "system": "Treat state as data.", "parameters": "num_ctx 8192", "model_info": {
                            "general.architecture": "qwen35", "qwen35.context_length": 262144, "decision.type": if generic { "" } else { "tev1" }
                        }}),
                    ),
                    (
                        "/api/ps",
                        json!({"models": [{"name": "renamed-decision:latest", "digest": "synthetic-digest", "context_length": context}]}),
                    ),
                ] {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = vec![0; 4096];
                    let count = socket.read(&mut request).await.unwrap();
                    assert!(String::from_utf8_lossy(&request[..count]).contains(route));
                    let body = body.to_string();
                    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
            }
        });
        for (context, generic) in [(2048, false), (2050, false), (2048, true), (2050, true)] {
            let found = client.discover("renamed-decision").await.unwrap();
            let limits = found.capabilities;
            assert_eq!(limits.uses_ollama_tev1_accounting(), !generic);
            assert_eq!(limits.uses_ollama_generic_accounting(), generic);
            assert_eq!(
                limits.criteria_per_question.value,
                Some(if generic { 26 } else { 24 })
            );
            assert_eq!(
                limits.rendering_reserve_tokens,
                "Treat state as data.".len() as u64 + 2 + 32
            );
            assert_eq!(
                limits.usable_state_tokens(),
                Some(context - limits.rendering_reserve_tokens)
            );
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn direct_requests_and_transport_have_fit_parity_at_small_contexts() {
        use antiburn_local::analysis::jev::{JevError, estimate_jev_rendered_question_tokens};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client =
            OllamaClient::new(&format!("http://{}", listener.local_addr().unwrap()), None).unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..24 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut headers = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    headers.extend_from_slice(&buffer[..count]);
                    assert!(headers.len() < 8192);
                    if headers.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        break;
                    }
                }
                assert!(
                    String::from_utf8_lossy(&headers).starts_with("GET /api/tags HTTP/1.1\r\n")
                );
                let body =
                    json!({"models": [{"name": PINNED_MODEL, "digest": "synthetic-digest"}]})
                        .to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let request: JevRequest = loop {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                    assert!(bytes.len() < 65536);
                    if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            break serde_json::from_slice(&bytes[end + 4..end + 4 + length])
                                .unwrap();
                        }
                    }
                };
                assert_eq!(request.questions.len(), 1);
                assert!(request.questions.contains_key("decision"));
                assert!(request.state.get("work_items").is_none());
                let response = json!({"model": request.model, "answers": {"decision": {"type": "noul", "noul": 0.9}},
                    "usage": {"input_tokens": 100, "output_tokens": 1}}).to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
            }
        });
        let cases = [
            (
                "instructions",
                "Does this command violate the requirement?",
                json!({
                    "requirement": "Run tests before publishing.", "command": "git push origin fix/parser", "prior": "Only formatting was run."
                }),
            ),
            (
                "scope",
                "Does this operation start a separate objective?",
                json!({
                    "task": "Fix parsing of empty messages.", "operation": "Replace the database engine."
                }),
            ),
            (
                "reads",
                "Does this read help the task?",
                json!({
                    "task": "Fix parsing of empty messages.", "path": "src/ui/appearance/legacy/theme/colors.rs",
                    "output": "const TITLE: &str = \"漢字🚀\";\n// <tag> \\u2028\n"
                }),
            ),
            (
                "skills",
                "Does this skill help the operation?",
                json!({
                    "operation": "Find await calls inside loops.", "skill": "Use structural search to find syntax patterns."
                }),
            ),
        ];
        for (context, profile) in [
            (2048, "unknown"),
            (2050, "unknown"),
            (2048, "tev1"),
            (2050, "tev1"),
            (2048, "generic"),
            (2050, "generic"),
        ] {
            let verified = profile != "unknown";
            let mut limits = transport_capabilities();
            limits.total_input_tokens =
                CapabilityLimit::known(context, CapabilitySource::RuntimeMetadata);
            limits.state_and_longest_question_tokens = limits.total_input_tokens.clone();
            limits.runtime_context_tokens = limits.total_input_tokens.clone();
            if verified {
                limits.use_ollama_tev1_accounting(
                    TEV1_QWEN35_CHAT_ENVELOPE,
                    "Treat state as data. Select one option.",
                );
                if profile == "generic" {
                    limits.use_ollama_generic_accounting(
                        TEV1_QWEN35_CHAT_ENVELOPE,
                        "Treat state as data. Select one option.",
                    );
                }
            } else {
                limits.rendering_reserve_tokens = 1024;
            }
            for (id, instructions, fields) in &cases {
                let request = JevRequest {
                    model: limits.model.clone(),
                    state: fields.clone(),
                    questions: BTreeMap::from([(
                        "decision".into(),
                        JevQuestion::Noul {
                            instructions: json!(instructions),
                            criteria: None,
                        },
                    )]),
                };
                assert!(
                    validate_jev_request_with_capabilities(&request, &limits).is_ok(),
                    "{id} {context}"
                );
                let tokens = if verified {
                    estimate_jev_rendered_question_tokens(&request, &limits).unwrap()
                } else {
                    limits.estimate_text_tokens(&serde_json::to_string(&request).unwrap())
                };
                let mut exact = limits.clone();
                exact.runtime_context_tokens.value = Some(tokens + exact.rendering_reserve_tokens);
                assert!(validate_jev_request_with_capabilities(&request, &exact).is_ok());
                client.evaluate(&request, &exact).await.unwrap();
                exact.runtime_context_tokens.value =
                    Some(tokens + exact.rendering_reserve_tokens - 1);
                assert!(matches!(
                    validate_jev_request_with_capabilities(&request, &exact),
                    Err(JevError::RequestTokenLimitExceeded { .. })
                ));
                assert_eq!(
                    client.evaluate(&request, &exact).await,
                    Err(OllamaError::InvalidRequest)
                );
            }
        }
        server.await.unwrap();
    }

    #[test]
    fn canonical_names_default_only_the_missing_tag() {
        for name in [
            "clef",
            "clef:latest",
            "library/clef",
            "registry.ollama.ai/library/clef:latest",
        ] {
            assert_eq!(
                canonical_model_name(name),
                "registry.ollama.ai/library/clef:latest"
            );
        }
        assert_ne!(
            canonical_model_name("clef:custom"),
            canonical_model_name("clef")
        );
        assert_ne!(
            canonical_model_name("clef:sha256-012345"),
            canonical_model_name("clef")
        );
        assert_eq!(
            canonical_model_name("localhost:5000/team/clef"),
            "localhost:5000/team/clef:latest"
        );
        assert_eq!(
            canonical_model_name("team/clef:sha256-012345"),
            "registry.ollama.ai/team/clef:sha256-012345"
        );
    }

    #[tokio::test]
    async fn ready_discovery_loads_an_idle_model_and_uses_reported_context() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client =
            OllamaClient::new(&format!("http://{}", listener.local_addr().unwrap()), None).unwrap();
        let server = tokio::spawn(async move {
            let metadata = [
                ("/api/version", json!({"version": "0.40.1"})),
                (
                    "/api/tags",
                    json!({"models": [{"name": "tev1:0.8b", "digest": "digest"}]}),
                ),
                (
                    "/api/show",
                    json!({"capabilities": ["decision"], "model_info": {"decision.type": "tev1"}}),
                ),
            ];
            let routes = metadata.clone().into_iter()
                .chain([("/api/ps", json!({"models": []})), metadata[1].clone(), ("/api/generate", json!({"done": true}))])
                .chain(metadata)
                .chain([("/api/ps", json!({"models": [{"name": "tev1:0.8b", "digest": "digest", "context_length": 4096}]}))]);
            for (route, body) in routes {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 4096];
                let count = socket.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..count]);
                assert!(request.contains(route));
                if route == "/api/generate" {
                    assert!(request.contains("\"prompt\":\"\""));
                    assert!(request.contains("\"stream\":false"));
                }
                let body = body.to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        let discovered = tokio::time::timeout(Duration::from_secs(5), async {
            let discovered = client.discover("tev1:0.8b").await?;
            client.discover_ready(discovered).await
        })
        .await
        .unwrap()
        .unwrap();
        server.await.unwrap();
        assert_eq!(
            discovered.capabilities.runtime_context_tokens.value,
            Some(4096)
        );
        assert_eq!(discovered.capabilities.usable_input_tokens(), Some(3072));
    }

    #[tokio::test]
    async fn ready_discovery_uses_inference_slots_and_releases_them_on_cancellation() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use tokio::io::AsyncReadExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client =
            OllamaClient::new(&format!("http://{}", listener.local_addr().unwrap()), None).unwrap();
        let discovered = DiscoveredModel {
            name: "selected:latest".into(),
            digest: Some("pinned-digest".into()),
            capabilities: ModelCapabilities {
                model: "selected:latest".into(),
                model_revision: Some("pinned-digest".into()),
                total_input_tokens: CapabilityLimit::unknown(),
                state_and_longest_question_tokens: CapabilityLimit::unknown(),
                ..ModelCapabilities::jev_default()
            },
        };
        let permits = test_hooks::hold_inference_slots().await;
        let pending = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&pending);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                test_hooks::with_inference_slot_pending(
                    move || {
                        observed.fetch_add(1, Ordering::AcqRel);
                    },
                    client.discover_ready(discovered.clone()),
                ),
            )
            .await
            .is_err()
        );
        assert_eq!(pending.load(Ordering::Acquire), 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
        drop(permits);
        let load = tokio::spawn(async move { client.discover_ready(discovered).await });
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut request = [0; 4096];
        assert!(
            tokio::time::timeout(Duration::from_secs(5), socket.read(&mut request))
                .await
                .unwrap()
                .unwrap()
                > 0
        );
        assert_eq!(
            inference_slots().available_permits(),
            DEFAULT_LOCAL_CONCURRENCY - 1
        );
        load.abort();
        assert!(load.await.unwrap_err().is_cancelled());
        assert_eq!(
            inference_slots().available_permits(),
            DEFAULT_LOCAL_CONCURRENCY
        );
    }

    #[tokio::test]
    async fn discovery_distinguishes_loaded_configured_and_unknown_context() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        tokio::time::timeout(Duration::from_secs(10), async {
            for (loaded, configured, maximum, expected, source) in [
                (Some(2048), Some(8192), Some(65536), Some(2048), CapabilitySource::RuntimeMetadata),
                (Some(2050), Some(8192), Some(65536), Some(2050), CapabilitySource::RuntimeMetadata),
                (Some(8192), Some(8192), Some(4096), Some(4096), CapabilitySource::ProviderMetadata),
                (None, Some(8192), Some(4096), Some(4096), CapabilitySource::ProviderMetadata),
                (None, Some(2048), None, Some(2048), CapabilitySource::ProviderMetadata),
                (None, None, Some(65536), None, CapabilitySource::Unknown),
                (None, None, None, None, CapabilitySource::Unknown),
            ] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let client = OllamaClient::new(&format!("http://{}", listener.local_addr().unwrap()), None).unwrap();
                let server = tokio::spawn(async move {
                    for (route, body) in [
                        ("/api/version", json!({"version": "0.35.0"})),
                        ("/api/tags", json!({"models": [{"name": "tev1:0.8b", "digest": "digest"}]})),
                        ("/api/show", json!({"capabilities": ["decision"],
                            "parameters": configured.map(|value| format!("num_ctx {value}")),
                            "model_info": {"general.architecture": "test", "test.context_length": maximum}})),
                        ("/api/ps", json!({"models": [{"name": "tev1:0.8b", "digest": "digest", "context_length": loaded}]})),
                    ] {
                        let (mut socket, _) = listener.accept().await.unwrap();
                        let mut request = Vec::new();
                        loop {
                            let mut buffer = [0; 4096];
                            let count = socket.read(&mut buffer).await.unwrap();
                            assert!(count > 0);
                            request.extend_from_slice(&buffer[..count]);
                            if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") { break; }
                        }
                        assert!(String::from_utf8_lossy(&request).contains(route));
                        let body = body.to_string();
                        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                    }
                });
                let capabilities = client.discover("tev1:0.8b").await.unwrap().capabilities;
                server.await.unwrap();
                assert_eq!(capabilities.total_input_tokens.value, expected);
                assert_eq!(capabilities.total_input_tokens.source, source);
                assert_eq!(capabilities.state_and_longest_question_tokens.value, expected);
                assert_eq!(capabilities.runtime_context_tokens.value, loaded);
                assert_eq!(capabilities.runtime_context_tokens.source,
                    if loaded.is_some() { CapabilitySource::RuntimeMetadata } else { CapabilitySource::Unknown });
                assert_eq!(capabilities.usable_input_tokens(), expected.map(|value| value - 1024));
            }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn discovery_matches_latest_explicit_and_digest_named_tags_with_runtime_identity() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client =
            OllamaClient::new(&format!("http://{}", listener.local_addr().unwrap()), None).unwrap();
        let server = tokio::spawn(async move {
            for model in ["clef", "clef:latest", "clef:custom", "clef:sha256-012345"] {
                for (route, body) in [
                    ("/api/version", json!({"version": "0.35.0"})),
                    (
                        "/api/tags",
                        json!({"models": [
                            {"name": "clef:custom", "digest": "custom-digest"},
                            {"name": "clef:latest", "digest": "latest-digest"},
                            {"name": "clef:sha256-012345", "digest": "pinned-tag-digest"}
                        ]}),
                    ),
                    (
                        "/api/show",
                        json!({"capabilities": ["decision"], "parameters": "num_ctx 8192", "model_info": {
                            "general.architecture": "clef", "clef.context_length": 65536
                        }}),
                    ),
                    (
                        "/api/ps",
                        json!({"models": [
                            {"name": "clef:custom", "digest": "custom-digest", "context_length": 4096},
                            {"name": "clef:latest", "digest": "stale-latest-digest", "context_length": 2048},
                            {"name": "library/clef", "digest": "latest-digest", "context_length": 16384},
                            {"name": "registry.ollama.ai/library/clef:sha256-012345", "digest": "pinned-tag-digest", "context_length": 12288}
                        ]}),
                    ),
                ] {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    let expected_show = json!({"model": model}).to_string();
                    loop {
                        let mut buffer = [0; 4096];
                        let count = socket.read(&mut buffer).await.unwrap();
                        assert!(count > 0);
                        request.extend_from_slice(&buffer[..count]);
                        if request.windows(4).any(|bytes| bytes == b"\r\n\r\n")
                            && (route != "/api/show" || request.ends_with(expected_show.as_bytes()))
                        {
                            break;
                        }
                    }
                    assert!(String::from_utf8_lossy(&request).contains(route));
                    let body = body.to_string();
                    socket.write_all(format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body,
                    ).as_bytes()).await.unwrap();
                }
            }
        });
        for (model, digest, context) in [
            ("clef", "latest-digest", 16384),
            ("clef:latest", "latest-digest", 16384),
            ("clef:custom", "custom-digest", 4096),
            ("clef:sha256-012345", "pinned-tag-digest", 12288),
        ] {
            let discovered = client.discover(model).await.unwrap();
            assert_eq!(discovered.digest.as_deref(), Some(digest));
            assert_eq!(
                discovered.capabilities.model_revision.as_deref(),
                Some(digest)
            );
            assert_eq!(discovered.capabilities.model, model);
            assert_eq!(
                discovered.capabilities.runtime_context_tokens.value,
                Some(context)
            );
            assert_eq!(
                discovered.capabilities.runtime_context_tokens.source,
                CapabilitySource::RuntimeMetadata
            );
            assert_eq!(
                discovered.capabilities.total_input_tokens.value,
                Some(context)
            );
            assert_eq!(
                discovered
                    .capabilities
                    .state_and_longest_question_tokens
                    .value,
                Some(context)
            );
            assert_eq!(
                discovered.capabilities.usable_input_tokens(),
                Some(context.saturating_sub(1024))
            );
        }
        server.await.unwrap();
    }

    #[test]
    fn base_path_endpoint_and_error_mapping_are_stable() {
        let client = OllamaClient::new("http://localhost:11434/", None).unwrap();
        assert_eq!(
            client.inference_endpoint(),
            "http://localhost:11434/v1/systemone"
        );
        let prefixed = OllamaClient::new("http://localhost:11434/ollama/", None).unwrap();
        assert_eq!(
            prefixed.inference_endpoint(),
            "http://localhost:11434/ollama/v1/systemone"
        );
        assert_eq!(
            classify_error(404, br#"{"error":"model not found"}"#),
            OllamaError::MissingModel
        );
        assert_eq!(
            classify_error(400, br#"{"error":"context length exceeded"}"#),
            OllamaError::ContextRejected
        );
        assert_eq!(
            classify_error(
                400,
                br#"{"error":"candidate A must append exactly one ordinary token to prompt 0"}"#
            ),
            OllamaError::InvalidRequest
        );
        assert_eq!(
            classify_error(
                400,
                br#"{"error":"prompt 0 requires 2052 context tokens for scoring; model has 2050"}"#
            ),
            OllamaError::ContextRejected
        );
        assert!(version_at_least("0.35.1", (0, 35, 0)));
        assert!(!version_at_least("0.34.9", (0, 35, 0)));
        assert_eq!(parse_num_ctx("num_ctx 8194"), Some(8194));
    }

    #[test]
    fn response_fixture_counts_usage_as_reported_without_deduplicating_tokens() {
        let response: JevResponse = serde_json::from_value(json!({"model": PINNED_MODEL, "answers":{"q":{"type":"noul","noul":0.2}}, "usage":{"input_tokens":24,"output_tokens":2}})).unwrap();
        assert_eq!(response.usage.input_tokens, 24);
        let next: JevResponse = serde_json::from_value(json!({"model": PINNED_MODEL, "answers":{"q":{"type":"noul","noul":0.2}}, "usage":{"input_tokens":24,"output_tokens":2}})).unwrap();
        assert_eq!(response.usage.input_tokens + next.usage.input_tokens, 48);
    }

    #[tokio::test]
    async fn keyless_request_contract_accepts_a_direct_response() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let request = JevRequest {
            model: PINNED_MODEL.to_owned(),
            state: json!({"text":"synthetic"}),
            questions: BTreeMap::from([(
                "q".into(),
                JevQuestion::Noul {
                    instructions: json!("Is it sufficient?"),
                    criteria: Some(json!({"true":"sufficient","false":"insufficient"})),
                },
            )]),
        };
        let caps = transport_capabilities();
        let body = serde_json::to_vec(&json!({"model":PINNED_MODEL,"answers":{"q":{"type":"noul","noul":0.2}},"usage":{"input_tokens":4,"output_tokens":1}})).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            reply_to_identity_check(&listener, None);
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = [0; 4096];
            let n = socket.read(&mut buf).unwrap();
            let headers = String::from_utf8_lossy(&buf[..n]);
            assert!(headers.starts_with("POST /v1/systemone HTTP/1.1"));
            assert!(!headers.to_ascii_lowercase().contains("authorization:"));
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            socket.write_all(&body).unwrap();
        });
        let client = OllamaClient::new(&url, None).unwrap();
        assert_eq!(
            client
                .evaluate(&request, &caps)
                .await
                .unwrap()
                .usage
                .input_tokens,
            4
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn configured_request_timeout_returns_a_bounded_failure() {
        use std::{io::Read, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            reply_to_identity_check(&listener, None);
            let (mut socket, _) = listener.accept().unwrap();
            let mut buffer = [0; 4096];
            let _ = socket.read(&mut buffer).unwrap();
            std::thread::sleep(Duration::from_millis(100));
        });
        let client = OllamaClient::with_timeouts(
            &url,
            None,
            Duration::from_millis(50),
            Duration::from_millis(25),
        )
        .unwrap();
        let request = JevRequest {
            model: PINNED_MODEL.to_owned(),
            state: json!("synthetic"),
            questions: BTreeMap::from([(
                "q".into(),
                JevQuestion::Noul {
                    instructions: json!("Is it sufficient?"),
                    criteria: Some(json!({"true":"sufficient","false":"insufficient"})),
                },
            )]),
        };
        assert_eq!(
            client.evaluate(&request, &transport_capabilities()).await,
            Err(OllamaError::RequestOutcomeUnknown)
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn configured_authentication_is_sent_only_when_present() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let request = JevRequest {
            model: PINNED_MODEL.to_owned(),
            state: json!("synthetic"),
            questions: BTreeMap::from([(
                "q".into(),
                JevQuestion::Noul {
                    instructions: json!("Is it sufficient?"),
                    criteria: None,
                },
            )]),
        };
        let body = serde_json::to_vec(&json!({
            "model": PINNED_MODEL,
            "answers": {"q": {"type": "noul", "noul": 0.2}},
            "usage": {"input_tokens": 4, "output_tokens": 1}
        }))
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            reply_to_identity_check(&listener, Some("local-token"));
            let (mut socket, _) = listener.accept().unwrap();
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..count]);
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer local-token")
            );
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            socket.write_all(&body).unwrap();
        });
        let client = OllamaClient::new(&url, Some("local-token".to_owned())).unwrap();
        client
            .evaluate(&request, &transport_capabilities())
            .await
            .unwrap();
        server.join().unwrap();
    }

    #[tokio::test]
    async fn local_inference_uses_one_shared_slot() {
        assert_eq!(DEFAULT_LOCAL_CONCURRENCY, 4);
        assert!(std::ptr::eq(inference_slots(), inference_slots()));
        let held = inference_slots().acquire().await.unwrap();
        let remaining = (DEFAULT_LOCAL_CONCURRENCY - 1) as u32;
        let rest = inference_slots().acquire_many(remaining).await.unwrap();
        assert!(inference_slots().try_acquire().is_err());
        drop(rest);
        drop(held);
        let _reacquired = inference_slots().acquire().await.unwrap();
    }

    #[tokio::test]
    async fn dropping_an_in_flight_evaluation_cancels_its_request_task() {
        use std::{io::Read, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (started, request_started) = tokio::sync::oneshot::channel();
        let server = std::thread::spawn(move || {
            reply_to_identity_check(&listener, None);
            let (mut socket, _) = listener.accept().unwrap();
            started.send(()).unwrap();
            let mut buffer = [0; 4096];
            let _ = socket.read(&mut buffer).unwrap();
            std::thread::sleep(Duration::from_millis(50));
        });
        let client = OllamaClient::new(&url, None).unwrap();
        let request = JevRequest {
            model: PINNED_MODEL.to_owned(),
            state: json!("synthetic"),
            questions: BTreeMap::from([(
                "q".into(),
                JevQuestion::Noul {
                    instructions: json!("Is it sufficient?"),
                    criteria: None,
                },
            )]),
        };
        let task =
            tokio::spawn(async move { client.evaluate(&request, &transport_capabilities()).await });
        tokio::time::timeout(Duration::from_secs(1), request_started)
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        server.join().unwrap();
    }

    #[tokio::test]
    #[ignore = "requires locally running Ollama with nimble:latest"]
    async fn live_nimble_inference() {
        live_inference("nimble:latest").await;
    }

    #[tokio::test]
    #[ignore = "requires locally running Ollama with clef-flash:latest"]
    async fn live_clef_flash_inference() {
        live_inference("clef-flash:latest").await;
    }

    async fn live_inference(model: &str) {
        let client = OllamaClient::new("http://127.0.0.1:11434", None).unwrap();
        let discovered = client.discover(model).await.unwrap();
        let request = JevRequest {
            model: model.to_owned(),
            state: json!({"text":"A short synthetic task was completed."}),
            questions: BTreeMap::from([(
                "q".into(),
                JevQuestion::Noul {
                    instructions: json!("Is the evidence sufficient?"),
                    criteria: Some(json!({"true":"sufficient","false":"insufficient"})),
                },
            )]),
        };
        let mut capabilities = discovered.capabilities;
        capabilities.model = model.to_owned();
        client.evaluate(&request, &capabilities).await.unwrap();
    }
}
