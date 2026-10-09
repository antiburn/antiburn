//! Ollama's direct System One transport and conservative local discovery.

use std::time::Duration;

use antiburn_local::analysis::jev::{
    JevRequest, JevResponse, MAX_RESPONSE_BYTES,
    capabilities::{CapabilityLimit, CapabilitySource, ModelCapabilities, TokenizerIdentity},
    validate_jev_request_with_capabilities, validate_jev_response_with_capabilities,
};
use serde::Deserialize;

pub const MAX_REQUEST_BODY_BYTES: usize = crate::jev::config::OLLAMA_MAX_REQUEST_BODY_BYTES;
pub const DEFAULT_LOCAL_CONCURRENCY: usize = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_DISCOVERY_BODY_BYTES: usize = 2 * 1024 * 1024;

fn inference_slots() -> &'static tokio::sync::Semaphore {
    static SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| tokio::sync::Semaphore::new(DEFAULT_LOCAL_CONCURRENCY))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OllamaError {
    InvalidBaseUrl,
    OldVersion,
    MissingModel,
    UnsupportedRunner,
    ColdLoad,
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
            Self::UnsupportedRunner => "The Ollama model does not support System One decisions",
            Self::ColdLoad => "The Ollama model is not loaded; retry after it loads",
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

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    api_key: Option<std::sync::Arc<str>>,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiscoveredModel {
    pub name: String,
    pub digest: Option<String>,
    pub capabilities: ModelCapabilities,
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

    pub async fn evaluate(
        &self,
        request: &JevRequest,
        capabilities: &ModelCapabilities,
    ) -> Result<JevResponse, OllamaError> {
        let _permit = inference_slots()
            .acquire()
            .await
            .map_err(|_| OllamaError::ProviderUnavailable)?;
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
        let response = builder.send().await.map_err(|error| {
            if error.is_connect() {
                OllamaError::ProviderUnavailable
            } else {
                OllamaError::RequestOutcomeUnknown
            }
        })?;
        let status = response.status();
        let bytes = read_bounded(response, MAX_RESPONSE_BYTES).await?;
        if !status.is_success() {
            return Err(classify_error(status.as_u16(), &bytes));
        }
        let decoded: JevResponse =
            serde_json::from_slice(&bytes).map_err(|_| OllamaError::ResponseDecode)?;
        validate_jev_response_with_capabilities(&decoded, request, capabilities)
            .map_err(|_| OllamaError::ResponseDecode)?;
        Ok(decoded)
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
        let show: ShowResponse = self
            .post_json("/api/show", &serde_json::json!({"model": model}))
            .await?;
        if !show
            .capabilities
            .iter()
            .any(|capability| capability == "decision")
        {
            return Err(OllamaError::UnsupportedRunner);
        }
        let ps: PsResponse = self.get_json("/api/ps").await?;
        let loaded = ps.models.iter().find(|entry| {
            canonical_model_name(&entry.name) == identity
                && entry
                    .digest
                    .as_ref()
                    .is_none_or(|digest| digest == &tag.digest)
        });
        let configured_context = show.parameters.as_deref().and_then(parse_num_ctx);
        let loaded_context = loaded.and_then(|entry| entry.context_length);
        let (runtime_context_tokens, runtime_source) = match (loaded_context, configured_context) {
            (Some(value), _) => (
                CapabilityLimit::known(value, CapabilitySource::RuntimeMetadata),
                CapabilitySource::RuntimeMetadata,
            ),
            (None, Some(value)) => (
                CapabilityLimit::known(value, CapabilitySource::ProviderMetadata),
                CapabilitySource::ProviderMetadata,
            ),
            (None, None) => (
                CapabilityLimit::known(8_192, CapabilitySource::DocumentedDefault),
                CapabilitySource::DocumentedDefault,
            ),
        };
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
        let cap = model_context
            .map(|max| max.min(runtime_context_tokens.value.unwrap_or(8_192)))
            .unwrap_or(runtime_context_tokens.value.unwrap_or(8_192));
        let source = if model_context.is_some() && cap == model_context.unwrap_or_default() {
            CapabilitySource::ProviderMetadata
        } else {
            runtime_source
        };
        let mut capabilities = ModelCapabilities {
            total_input_tokens: CapabilityLimit::known(cap, source),
            state_and_longest_question_tokens: CapabilityLimit::known(cap, source),
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
    } else if message.contains("not found") || message.contains("pull") {
        OllamaError::MissingModel
    } else if message.contains("decision")
        || message.contains("runner")
        || message.contains("unsupported")
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
    } else if message.contains("load") || message.contains("memory") {
        OllamaError::ColdLoad
    } else if status == 413 {
        OllamaError::RequestBodyTooLarge
    } else if status == 400 || status == 422 {
        OllamaError::InvalidRequest
    } else {
        OllamaError::ProviderUnavailable
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
}
#[derive(Deserialize)]
struct ShowResponse {
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
mod tests {
    use super::*;
    use antiburn_local::analysis::jev::{JevQuestion, PINNED_MODEL};
    use serde_json::json;
    use std::collections::BTreeMap;

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
    async fn tev1_compact_comparisons_have_packer_and_transport_fit_parity_at_small_contexts() {
        use antiburn_local::analysis::jev::{
            JevInputWindow, JevWorkItem, estimate_jev_rendered_question_tokens,
            pack_work_items_with_capabilities,
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client =
            OllamaClient::new(&format!("http://{}", listener.local_addr().unwrap()), None).unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..24 {
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
                assert!(request.questions.contains_key("q0_0"));
                let response = json!({"model": request.model, "answers": {"q0_0": {"type": "noul", "noul": 0.9}},
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
            let mut limits = ModelCapabilities::jev_default();
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
                let work = JevWorkItem {
                    id: (*id).into(),
                    window: JevInputWindow {
                        fields: fields.clone(),
                        evidence: vec![],
                    },
                    questions: BTreeMap::from([(
                        "decision".into(),
                        JevQuestion::Noul {
                            instructions: json!(instructions),
                            criteria: None,
                        },
                    )]),
                };
                let packed =
                    pack_work_items_with_capabilities(std::slice::from_ref(&work), &limits);
                assert!(packed.skipped_item_ids.is_empty(), "{id} {context}");
                let request = &packed.batches[0].request;
                let tokens = if verified {
                    estimate_jev_rendered_question_tokens(request, &limits).unwrap()
                } else {
                    limits.estimate_text_tokens(&serde_json::to_string(request).unwrap())
                };
                let mut exact = limits.clone();
                exact.runtime_context_tokens.value = Some(tokens + exact.rendering_reserve_tokens);
                assert!(validate_jev_request_with_capabilities(request, &exact).is_ok());
                assert_eq!(
                    pack_work_items_with_capabilities(std::slice::from_ref(&work), &exact)
                        .batches
                        .len(),
                    1
                );
                client.evaluate(request, &exact).await.unwrap();
                exact.runtime_context_tokens.value =
                    Some(tokens + exact.rendering_reserve_tokens - 1);
                assert!(
                    pack_work_items_with_capabilities(std::slice::from_ref(&work), &exact)
                        .batches
                        .is_empty()
                );
                assert_eq!(
                    client.evaluate(request, &exact).await,
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
        let caps = ModelCapabilities::jev_default();
        let body = serde_json::to_vec(&json!({"model":PINNED_MODEL,"answers":{"q":{"type":"noul","noul":0.2}},"usage":{"input_tokens":4,"output_tokens":1}})).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
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
            client
                .evaluate(&request, &ModelCapabilities::jev_default())
                .await,
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
            .evaluate(&request, &ModelCapabilities::jev_default())
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
        let task = tokio::spawn(async move {
            client
                .evaluate(&request, &ModelCapabilities::jev_default())
                .await
        });
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
