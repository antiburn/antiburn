use antiburn_local::analysis::jev::capabilities::{
    CapabilityLimit, CapabilitySource, ModelCapabilities,
};
use antiburn_local::analysis::jev::{JevError, NATIVE_MODEL_ALIAS, PINNED_MODEL};
use serde::{Deserialize, Serialize};
use url::Url;

const SYSTEM_ONE_PROTOCOL: &str = "https://";
const SYSTEM_ONE_HOSTNAME: &str = "api.typesafe.ai";
const SYSTEM_ONE_API_PATH: &str = "/v1/systemone";
const MAX_MODEL_NAME_BYTES: usize = 128;
const MAX_ACCOUNT_ID_BYTES: usize = 64;
pub(crate) const OLLAMA_MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;

pub fn system_one_endpoint() -> String {
    format!("{SYSTEM_ONE_PROTOCOL}{SYSTEM_ONE_HOSTNAME}{SYSTEM_ONE_API_PATH}")
}

/// Resolve one selected connection without hiding discovery or identity failures.
pub async fn resolve_execution_capabilities(
    connection: &SystemOneConnection,
    credential: Option<&str>,
) -> Result<ModelCapabilities, JevError> {
    resolve_capabilities(connection, credential, true).await
}

/// Read limits without loading an idle model.
pub async fn resolve_discovery_capabilities(
    connection: &SystemOneConnection,
    credential: Option<&str>,
) -> Result<ModelCapabilities, JevError> {
    resolve_capabilities(connection, credential, false).await
}

async fn resolve_capabilities(
    connection: &SystemOneConnection,
    credential: Option<&str>,
    load_if_needed: bool,
) -> Result<ModelCapabilities, JevError> {
    connection
        .validate()
        .map_err(|_| JevError::InvalidRequestSchema)?;
    if connection.provider != SystemOneProvider::Ollama {
        let capabilities = connection
            .capabilities()
            .map_err(|_| JevError::InvalidRequestSchema)?;
        if capabilities.usable_input_tokens().is_none()
            || capabilities.usable_state_tokens().is_none()
        {
            return Err(JevError::UnsupportedModel);
        }
        return Ok(capabilities);
    }
    let SystemOneEndpoint::BaseUrl(base) = &connection.endpoint else {
        return Err(JevError::InvalidRequestSchema);
    };
    let client = crate::jev_ollama::OllamaClient::new(base, credential.map(str::to_owned))
        .map_err(crate::jev_ollama::jev_error)?;
    let mut discovered = client
        .discover(&connection.model)
        .await
        .map_err(crate::jev_ollama::jev_error)?;
    validate_discovered_identity(connection, &discovered)?;
    discovered.capabilities = connection.apply_capability_overrides(discovered.capabilities);
    if load_if_needed {
        discovered = client
            .discover_ready(discovered)
            .await
            .map_err(crate::jev_ollama::jev_error)?;
        validate_discovered_identity(connection, &discovered)?;
    }
    Ok(connection.apply_capability_overrides(discovered.capabilities))
}

fn validate_discovered_identity(
    connection: &SystemOneConnection,
    discovered: &crate::jev_ollama::DiscoveredModel,
) -> Result<(), JevError> {
    if discovered.name != connection.model
        || connection
            .model_revision
            .as_ref()
            .is_some_and(|revision| discovered.digest.as_ref() != Some(revision))
    {
        return Err(JevError::UnsupportedModel);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SystemOneProvider {
    #[default]
    Jev,
    Ollama,
    Cloudflare,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SystemOneEndpoint {
    ProviderDefault,
    BaseUrl(String),
    CloudflareAccount(String),
    ExactUrl(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemOneResponseMode {
    #[serde(alias = "system_one", alias = "direct_system_one")]
    Direct,
    #[serde(alias = "cloudflare", alias = "workers_ai")]
    CloudflareEnvelope,
}

/// Identifies secure storage only. It never contains an API key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum CredentialReference {
    LegacyTypeSafe,
    Connection(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContextLimitOverride {
    pub total_input_tokens: Option<u64>,
    pub state_and_longest_question_tokens: Option<u64>,
    pub runtime_context_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ConnectionFields", into = "ConnectionFields")]
pub struct SystemOneConnection {
    pub provider: SystemOneProvider,
    pub endpoint: SystemOneEndpoint,
    pub model: String,
    pub model_revision: Option<String>,
    pub response_mode: SystemOneResponseMode,
    pub credential: Option<CredentialReference>,
    pub revision: u64,
    pub context_override: Option<ContextLimitOverride>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionFields {
    provider: SystemOneProvider,
    endpoint: SystemOneEndpoint,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    model_revision: Option<String>,
    #[serde(default = "default_response_mode")]
    response_mode: SystemOneResponseMode,
    credential: Option<CredentialReference>,
    revision: u64,
    #[serde(default)]
    context_override: Option<ContextLimitOverride>,
}

impl TryFrom<ConnectionFields> for SystemOneConnection {
    type Error = &'static str;

    fn try_from(fields: ConnectionFields) -> Result<Self, Self::Error> {
        let native = fields.provider == SystemOneProvider::Jev;
        Ok(Self {
            provider: fields.provider,
            endpoint: fields.endpoint,
            model: if native {
                NATIVE_MODEL_ALIAS.to_owned()
            } else {
                fields.model.ok_or("Missing model.")?
            },
            model_revision: if native { None } else { fields.model_revision },
            response_mode: fields.response_mode,
            credential: fields.credential,
            revision: fields.revision,
            context_override: fields.context_override,
        })
    }
}

impl From<SystemOneConnection> for ConnectionFields {
    fn from(connection: SystemOneConnection) -> Self {
        let native = connection.provider == SystemOneProvider::Jev;
        Self {
            provider: connection.provider,
            endpoint: connection.endpoint,
            model: Some(if native {
                NATIVE_MODEL_ALIAS.to_owned()
            } else {
                connection.model
            }),
            model_revision: if native {
                None
            } else {
                connection.model_revision
            },
            response_mode: connection.response_mode,
            credential: connection.credential,
            revision: connection.revision,
            context_override: connection.context_override,
        }
    }
}

fn default_response_mode() -> SystemOneResponseMode {
    SystemOneResponseMode::Direct
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionValidationError {
    InvalidModel,
    InvalidEndpoint,
    InvalidCredentialReference,
    ProviderEndpointMismatch,
    CloudflareModelEndpointMismatch,
    InvalidRevision,
    InvalidModelRevision,
}

impl Default for SystemOneConnection {
    fn default() -> Self {
        Self::jev_default()
    }
}

impl SystemOneConnection {
    pub fn jev_default() -> Self {
        Self {
            provider: SystemOneProvider::Jev,
            endpoint: SystemOneEndpoint::ProviderDefault,
            model: NATIVE_MODEL_ALIAS.to_owned(),
            model_revision: None,
            response_mode: SystemOneResponseMode::Direct,
            credential: Some(CredentialReference::LegacyTypeSafe),
            revision: 1,
            context_override: None,
        }
    }

    pub fn validate(&self) -> Result<(), ConnectionValidationError> {
        self.validate_fields(false)
    }

    pub(crate) fn validate_ollama_model_lookup(&self) -> Result<(), ConnectionValidationError> {
        if self.provider != SystemOneProvider::Ollama {
            return Err(ConnectionValidationError::ProviderEndpointMismatch);
        }
        self.validate_fields(true)
    }

    fn validate_fields(&self, allow_empty_model: bool) -> Result<(), ConnectionValidationError> {
        if self.revision == 0 {
            return Err(ConnectionValidationError::InvalidRevision);
        }
        if !(allow_empty_model && self.model.is_empty()) && !valid_model_name(&self.model) {
            return Err(ConnectionValidationError::InvalidModel);
        }
        if self.model_revision.as_ref().is_some_and(|revision| {
            revision.is_empty() || revision.len() > 256 || revision.chars().any(char::is_control)
        }) {
            return Err(ConnectionValidationError::InvalidModelRevision);
        }
        if let Some(CredentialReference::Connection(id)) = &self.credential
            && !valid_reference_id(id)
        {
            return Err(ConnectionValidationError::InvalidCredentialReference);
        }
        if self.provider == SystemOneProvider::Custom
            && !matches!(
                self.response_mode,
                SystemOneResponseMode::Direct | SystemOneResponseMode::CloudflareEnvelope
            )
        {
            return Err(ConnectionValidationError::ProviderEndpointMismatch);
        }

        match (&self.provider, &self.endpoint) {
            (SystemOneProvider::Jev, SystemOneEndpoint::ProviderDefault)
                if self.model == NATIVE_MODEL_ALIAS && self.model_revision.is_none() =>
            {
                Ok(())
            }
            (SystemOneProvider::Jev, _) => Err(ConnectionValidationError::ProviderEndpointMismatch),
            (SystemOneProvider::Ollama, SystemOneEndpoint::BaseUrl(value)) => {
                validate_url(value, false)
            }
            (SystemOneProvider::Cloudflare, SystemOneEndpoint::CloudflareAccount(account_id)) => {
                if !valid_account_id(account_id)
                    || !matches!(self.model.as_str(), "clef" | "clef-flash")
                {
                    return Err(ConnectionValidationError::CloudflareModelEndpointMismatch);
                }
                Ok(())
            }
            (SystemOneProvider::Custom, SystemOneEndpoint::ExactUrl(value)) => {
                validate_url(value, true)
            }
            _ => Err(ConnectionValidationError::ProviderEndpointMismatch),
        }
    }

    pub fn inference_endpoint(&self) -> Result<String, ConnectionValidationError> {
        self.validate()?;
        match (&self.provider, &self.endpoint) {
            (SystemOneProvider::Jev, SystemOneEndpoint::ProviderDefault) => {
                Ok(system_one_endpoint())
            }
            (SystemOneProvider::Ollama, SystemOneEndpoint::BaseUrl(base)) => {
                Ok(format!("{}/v1/systemone", base.trim_end_matches('/')))
            }
            (SystemOneProvider::Cloudflare, SystemOneEndpoint::CloudflareAccount(account)) => {
                Ok(format!(
                    "https://api.cloudflare.com/client/v4/accounts/{account}/ai/run/@cf/cloudflare/{}",
                    self.model
                ))
            }
            (SystemOneProvider::Custom, SystemOneEndpoint::ExactUrl(url)) => Ok(url.clone()),
            _ => Err(ConnectionValidationError::ProviderEndpointMismatch),
        }
    }

    pub fn capabilities(&self) -> Result<ModelCapabilities, ConnectionValidationError> {
        self.validate()?;
        Ok(self.resolved_capabilities())
    }

    fn resolved_capabilities(&self) -> ModelCapabilities {
        let mut capabilities = match self.provider {
            SystemOneProvider::Jev => ModelCapabilities::jev_native(),
            SystemOneProvider::Cloudflare => {
                crate::jev_cloudflare::cloudflare_capabilities(&self.model)
            }
            SystemOneProvider::Ollama | SystemOneProvider::Custom => {
                let mut limits = ModelCapabilities::jev_default();
                limits.output = Default::default();
                limits.model = self.model.clone();
                limits.total_input_tokens = CapabilityLimit::unknown();
                limits.state_and_longest_question_tokens = CapabilityLimit::unknown();
                limits.runtime_context_tokens = CapabilityLimit::unknown();
                limits.questions_per_request.source = CapabilitySource::Manual;
                limits.criteria_per_question.source = CapabilitySource::Manual;
                if self.provider == SystemOneProvider::Ollama {
                    limits.rendering_reserve_tokens = 1024;
                    limits.request_body_bytes = CapabilityLimit::known(
                        OLLAMA_MAX_REQUEST_BODY_BYTES as u64,
                        CapabilitySource::DocumentedDefault,
                    );
                } else {
                    if let Some(verified) = self.custom_route_capabilities() {
                        limits = verified;
                    }
                }
                limits
            }
        };
        capabilities = self.apply_capability_overrides(capabilities);
        capabilities
    }

    fn custom_route_capabilities(&self) -> Option<ModelCapabilities> {
        let SystemOneEndpoint::ExactUrl(endpoint) = &self.endpoint else {
            return None;
        };
        match self.response_mode {
            SystemOneResponseMode::Direct
                if endpoint == &system_one_endpoint()
                    && matches!(self.model.as_str(), PINNED_MODEL | "jev-1.14.0") =>
            {
                let mut capabilities = ModelCapabilities::jev_default();
                capabilities.model.clone_from(&self.model);
                Some(capabilities)
            }
            SystemOneResponseMode::CloudflareEnvelope => {
                let account = endpoint
                    .strip_prefix("https://api.cloudflare.com/client/v4/accounts/")?
                    .split_once("/ai/run/")?
                    .0;
                let expected =
                    crate::jev_cloudflare::account_endpoint(account, &self.model).ok()?;
                (endpoint == &expected)
                    .then(|| crate::jev_cloudflare::cloudflare_capabilities(&self.model))
            }
            _ => None,
        }
    }

    // Legacy context overrides do not change capabilities for new work.
    pub(crate) fn apply_capability_overrides(
        &self,
        mut capabilities: ModelCapabilities,
    ) -> ModelCapabilities {
        if capabilities.model_revision.is_none() {
            capabilities.model_revision.clone_from(&self.model_revision);
        }
        capabilities
    }
}

pub(crate) fn valid_model_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_MODEL_NAME_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
}

fn valid_account_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ACCOUNT_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_reference_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn validate_url(value: &str, allow_query: bool) -> Result<(), ConnectionValidationError> {
    let url = Url::parse(value).map_err(|_| ConnectionValidationError::InvalidEndpoint)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || (!allow_query && url.query().is_some())
    {
        return Err(ConnectionValidationError::InvalidEndpoint);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectionValidationError, ContextLimitOverride, CredentialReference, SystemOneConnection,
        SystemOneEndpoint, SystemOneProvider, SystemOneResponseMode, system_one_endpoint,
    };
    use antiburn_local::analysis::jev::capabilities::CapabilitySource;
    use antiburn_local::analysis::jev::{NATIVE_MODEL_ALIAS, PINNED_MODEL};
    use serde::Deserialize;

    #[test]
    fn default_preserves_the_current_jev_endpoint_model_and_credential_reference() {
        let connection = SystemOneConnection::default();

        assert_eq!(connection.provider, SystemOneProvider::Jev);
        assert_eq!(connection.endpoint, SystemOneEndpoint::ProviderDefault);
        assert_eq!(connection.model, NATIVE_MODEL_ALIAS);
        assert_eq!(
            connection.credential,
            Some(CredentialReference::LegacyTypeSafe)
        );
        assert_eq!(connection.context_override, None);
        assert_eq!(
            connection.inference_endpoint().unwrap(),
            system_one_endpoint()
        );
    }

    #[test]
    fn model_revision_preserves_legacy_settings_and_validates_discovered_identity() {
        let mut encoded = serde_json::to_value(SystemOneConnection {
            provider: SystemOneProvider::Ollama,
            endpoint: SystemOneEndpoint::BaseUrl("http://localhost:11434".into()),
            model: "synthetic-model".into(),
            ..Default::default()
        })
        .unwrap();
        encoded.as_object_mut().unwrap().remove("modelRevision");
        let mut connection: SystemOneConnection = serde_json::from_value(encoded).unwrap();
        assert_eq!(connection.model_revision, None);
        connection.model_revision = Some("sha256:model-digest".into());
        assert_eq!(
            connection.capabilities().unwrap().model_revision,
            connection.model_revision
        );
        for revision in [String::new(), "x".repeat(257), "digest\n".into()] {
            connection.model_revision = Some(revision);
            assert_eq!(
                connection.validate(),
                Err(ConnectionValidationError::InvalidModelRevision)
            );
        }
    }

    #[test]
    fn native_settings_boundary_preserves_every_field_except_release_selection() {
        for credential in [
            CredentialReference::LegacyTypeSafe,
            CredentialReference::Connection("personal-key".into()),
        ] {
            for model in [Some("jev-1.13.0"), Some("obsolete-model"), None] {
                let mut raw = serde_json::json!({
                    "provider":"jev", "endpoint":{"kind":"provider_default"},
                    "modelRevision":"obsolete-pin", "credential":credential, "revision":41,
                    "contextOverride":{"totalInputTokens":12000,"stateAndLongestQuestionTokens":9000,"runtimeContextTokens":10000}
                });
                if let Some(model) = model {
                    raw["model"] = serde_json::json!(model);
                }
                let connection: SystemOneConnection = serde_json::from_value(raw.clone()).unwrap();
                connection.validate().unwrap();
                let encoded = serde_json::to_value(&connection).unwrap();
                raw["model"] = serde_json::json!(NATIVE_MODEL_ALIAS);
                raw["modelRevision"] = serde_json::Value::Null;
                raw["responseMode"] = serde_json::json!("direct");
                assert_eq!(encoded, raw);
                assert_eq!(connection.capabilities().unwrap().model_revision, None);
                let mut invalid = encoded.clone();
                invalid["endpoint"] =
                    serde_json::json!({"kind":"exact_url","value":"https://wrong.invalid"});
                let decoded: SystemOneConnection = serde_json::from_value(invalid.clone()).unwrap();
                assert_eq!(
                    decoded.validate(),
                    Err(ConnectionValidationError::ProviderEndpointMismatch)
                );
                assert_eq!(serde_json::to_value(decoded).unwrap(), invalid);
            }
        }
        let mut nonnative = serde_json::to_value(SystemOneConnection {
            provider: SystemOneProvider::Ollama,
            endpoint: SystemOneEndpoint::BaseUrl("http://localhost:11434".into()),
            model: "authored-model".into(),
            model_revision: Some("authored-revision".into()),
            ..Default::default()
        })
        .unwrap();
        let decoded: SystemOneConnection = serde_json::from_value(nonnative.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), nonnative);
        nonnative.as_object_mut().unwrap().remove("model");
        assert!(serde_json::from_value::<SystemOneConnection>(nonnative).is_err());
    }

    #[test]
    fn only_native_route_uses_long_context_and_ignores_legacy_manual_limits() {
        let mut connection = SystemOneConnection::default();
        let native = connection.capabilities().unwrap();
        assert_eq!(native.total_input_tokens.value, Some(262_000));
        assert_eq!(
            native.state_and_longest_question_tokens.value,
            Some(262_000)
        );
        for model in [PINNED_MODEL, "jev-1.14.0"] {
            connection.provider = SystemOneProvider::Custom;
            connection.endpoint = SystemOneEndpoint::ExactUrl(system_one_endpoint());
            connection.model = model.into();
            let standard = connection.capabilities().unwrap();
            assert_eq!(standard.total_input_tokens.value, Some(65_536));
            assert_eq!(
                standard.state_and_longest_question_tokens.value,
                Some(32_768)
            );
            connection.context_override = Some(ContextLimitOverride {
                total_input_tokens: Some(262_000),
                ..Default::default()
            });
            assert_eq!(connection.capabilities().unwrap(), standard);
            connection.context_override = None;
        }
        connection.model = NATIVE_MODEL_ALIAS.into();
        connection.validate().unwrap();
        connection.context_override = Some(ContextLimitOverride {
            total_input_tokens: Some(8192),
            ..Default::default()
        });
        let alias_custom = connection.capabilities().unwrap();
        assert_eq!(alias_custom.total_input_tokens.value, None);
        assert_eq!(alias_custom.output, Default::default());
        connection = SystemOneConnection::default();
        connection.context_override = Some(ContextLimitOverride {
            total_input_tokens: Some(262_000),
            ..Default::default()
        });
        connection.validate().unwrap();
        connection
            .context_override
            .as_mut()
            .unwrap()
            .total_input_tokens = Some(262_001);
        assert_eq!(connection.capabilities().unwrap(), native);
    }

    #[test]
    fn connection_round_trip_never_serializes_an_api_key() {
        let connection = SystemOneConnection {
            credential: Some(CredentialReference::Connection("checks-cloudflare".into())),
            ..SystemOneConnection::default()
        };
        let encoded = serde_json::to_string(&connection).unwrap();
        let decoded: SystemOneConnection = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, connection);
        assert!(!encoded.contains("api_key"));
        assert!(!encoded.contains("secret"));
    }

    #[test]
    fn ollama_uses_a_base_url_and_adds_the_system_one_path() {
        let connection = SystemOneConnection {
            provider: SystemOneProvider::Ollama,
            endpoint: SystemOneEndpoint::BaseUrl("http://localhost:11434/".into()),
            model: "clef-flash".into(),
            model_revision: None,
            response_mode: SystemOneResponseMode::Direct,
            credential: None,
            revision: 2,
            context_override: None,
        };

        assert_eq!(
            connection.inference_endpoint().unwrap(),
            "http://localhost:11434/v1/systemone"
        );
    }

    #[test]
    fn ollama_model_lookup_allows_no_selection_but_keeps_connection_validation() {
        let connection = SystemOneConnection {
            provider: SystemOneProvider::Ollama,
            endpoint: SystemOneEndpoint::BaseUrl("http://localhost:11434".into()),
            model: String::new(),
            credential: None,
            ..Default::default()
        };
        assert_eq!(connection.validate_ollama_model_lookup(), Ok(()));
        assert_eq!(
            connection.validate(),
            Err(ConnectionValidationError::InvalidModel)
        );

        for (invalid, expected) in [
            (
                SystemOneConnection {
                    revision: 0,
                    ..connection.clone()
                },
                ConnectionValidationError::InvalidRevision,
            ),
            (
                SystemOneConnection {
                    model: "invalid model".into(),
                    ..connection.clone()
                },
                ConnectionValidationError::InvalidModel,
            ),
            (
                SystemOneConnection {
                    model_revision: Some("digest\n".into()),
                    ..connection.clone()
                },
                ConnectionValidationError::InvalidModelRevision,
            ),
            (
                SystemOneConnection {
                    credential: Some(CredentialReference::Connection("invalid/reference".into())),
                    ..connection.clone()
                },
                ConnectionValidationError::InvalidCredentialReference,
            ),
            (
                SystemOneConnection {
                    provider: SystemOneProvider::Custom,
                    ..connection.clone()
                },
                ConnectionValidationError::ProviderEndpointMismatch,
            ),
            (
                SystemOneConnection {
                    endpoint: SystemOneEndpoint::ExactUrl("http://localhost:11434".into()),
                    ..connection.clone()
                },
                ConnectionValidationError::ProviderEndpointMismatch,
            ),
            (
                SystemOneConnection {
                    endpoint: SystemOneEndpoint::BaseUrl("http://localhost:11434?query=1".into()),
                    ..connection
                },
                ConnectionValidationError::InvalidEndpoint,
            ),
        ] {
            assert_eq!(invalid.validate_ollama_model_lookup(), Err(expected));
        }
    }

    #[test]
    fn cloudflare_model_and_account_build_one_matching_route() {
        let connection = SystemOneConnection {
            provider: SystemOneProvider::Cloudflare,
            endpoint: SystemOneEndpoint::CloudflareAccount("account-123".into()),
            model: "clef".into(),
            model_revision: None,
            response_mode: SystemOneResponseMode::CloudflareEnvelope,
            credential: Some(CredentialReference::Connection("cloudflare".into())),
            revision: 3,
            context_override: None,
        };

        assert_eq!(
            connection.inference_endpoint().unwrap(),
            "https://api.cloudflare.com/client/v4/accounts/account-123/ai/run/@cf/cloudflare/clef"
        );
    }

    #[test]
    fn custom_endpoint_is_used_exactly() {
        let connection = SystemOneConnection {
            provider: SystemOneProvider::Custom,
            endpoint: SystemOneEndpoint::ExactUrl(
                "https://gateway.example/v2/systemone?route=local".into(),
            ),
            model: "clef-proxy".into(),
            model_revision: None,
            response_mode: SystemOneResponseMode::Direct,
            credential: None,
            revision: 1,
            context_override: Some(ContextLimitOverride {
                total_input_tokens: Some(8192),
                ..ContextLimitOverride::default()
            }),
        };

        assert_eq!(
            connection.inference_endpoint().unwrap(),
            "https://gateway.example/v2/systemone?route=local"
        );

        let prefixed = SystemOneConnection {
            endpoint: SystemOneEndpoint::ExactUrl(
                "https://gateway.example/proxy/v2/systemone/?tenant=checks".into(),
            ),
            ..connection
        };
        assert_eq!(
            prefixed.inference_endpoint().unwrap(),
            "https://gateway.example/proxy/v2/systemone/?tenant=checks"
        );
    }

    #[test]
    fn unknown_custom_stays_readable_but_manual_bounds_do_not_supply_capabilities() {
        let mut connection = SystemOneConnection {
            provider: SystemOneProvider::Custom,
            endpoint: SystemOneEndpoint::ExactUrl("https://proxy.example/infer".into()),
            model: "custom-model".into(),
            credential: None,
            ..SystemOneConnection::default()
        };
        for limits in [
            None,
            Some(ContextLimitOverride::default()),
            Some(ContextLimitOverride {
                state_and_longest_question_tokens: Some(8192),
                ..ContextLimitOverride::default()
            }),
            Some(ContextLimitOverride {
                total_input_tokens: Some(4096),
                ..ContextLimitOverride::default()
            }),
            Some(ContextLimitOverride {
                total_input_tokens: Some(8192),
                runtime_context_tokens: Some(4096),
                ..ContextLimitOverride::default()
            }),
            Some(ContextLimitOverride {
                total_input_tokens: Some(8192),
                state_and_longest_question_tokens: Some(4096),
                ..ContextLimitOverride::default()
            }),
        ] {
            connection.context_override = limits;
            connection.validate().unwrap();
            assert_eq!(
                connection.capabilities().unwrap().usable_input_tokens(),
                None
            );
            assert_eq!(
                connection.capabilities().unwrap().usable_state_tokens(),
                None
            );
            assert_eq!(
                connection.inference_endpoint().unwrap(),
                "https://proxy.example/infer"
            );
        }
        for limits in [
            ContextLimitOverride {
                total_input_tokens: Some(4097),
                ..ContextLimitOverride::default()
            },
            ContextLimitOverride {
                runtime_context_tokens: Some(4097),
                ..ContextLimitOverride::default()
            },
        ] {
            connection.context_override = Some(limits);
            let capabilities = connection.capabilities().unwrap();
            assert_eq!(capabilities.usable_input_tokens(), None);
            assert_eq!(capabilities.usable_state_tokens(), None);
        }
        connection.provider = SystemOneProvider::Ollama;
        connection.endpoint = SystemOneEndpoint::BaseUrl("http://localhost:11434".into());
        connection.context_override = None;
        let capabilities = connection.capabilities().unwrap();
        assert_eq!(capabilities.usable_input_tokens(), None);
        assert_eq!(capabilities.usable_state_tokens(), None);
        connection.context_override = Some(ContextLimitOverride {
            total_input_tokens: Some(1024),
            ..ContextLimitOverride::default()
        });
        assert_eq!(
            connection.capabilities().unwrap().usable_input_tokens(),
            None
        );
    }

    #[test]
    fn custom_response_modes_accept_documented_aliases() {
        for (alias, expected) in [
            ("direct_system_one", SystemOneResponseMode::Direct),
            ("workers_ai", SystemOneResponseMode::CloudflareEnvelope),
        ] {
            let encoded = serde_json::json!({"mode": alias});
            let decoded: ResponseModeFixture = serde_json::from_value(encoded).unwrap();
            assert_eq!(decoded.mode, expected);
        }
    }

    #[derive(Deserialize)]
    struct ResponseModeFixture {
        mode: SystemOneResponseMode,
    }

    #[test]
    fn rejects_endpoint_scheme_userinfo_fragments_and_provider_mismatch() {
        for endpoint in [
            "file:///tmp/model",
            "https://",
            "https://user:password@example.com/api",
            "https://example.com/api#fragment",
        ] {
            let connection = SystemOneConnection {
                provider: SystemOneProvider::Custom,
                endpoint: SystemOneEndpoint::ExactUrl(endpoint.into()),
                model: "model".into(),
                model_revision: None,
                response_mode: SystemOneResponseMode::Direct,
                credential: None,
                revision: 1,
                context_override: None,
            };
            assert_eq!(
                connection.validate(),
                Err(ConnectionValidationError::InvalidEndpoint)
            );
        }

        let mismatch = SystemOneConnection {
            provider: SystemOneProvider::Ollama,
            endpoint: SystemOneEndpoint::ExactUrl("http://localhost:11434/v1/systemone".into()),
            model: "clef".into(),
            model_revision: None,
            response_mode: SystemOneResponseMode::Direct,
            credential: None,
            revision: 1,
            context_override: None,
        };
        assert_eq!(
            mismatch.validate(),
            Err(ConnectionValidationError::ProviderEndpointMismatch)
        );
    }

    #[test]
    fn rejects_cloudflare_route_mismatches_even_with_legacy_overrides() {
        let mut connection = SystemOneConnection {
            provider: SystemOneProvider::Cloudflare,
            endpoint: SystemOneEndpoint::CloudflareAccount("bad/account".into()),
            model: "clef".into(),
            model_revision: None,
            response_mode: SystemOneResponseMode::CloudflareEnvelope,
            credential: None,
            revision: 1,
            context_override: None,
        };
        assert_eq!(
            connection.validate(),
            Err(ConnectionValidationError::CloudflareModelEndpointMismatch)
        );

        connection.endpoint = SystemOneEndpoint::CloudflareAccount("account".into());
        connection.model = "not-a-cloudflare-model".into();
        assert_eq!(
            connection.validate(),
            Err(ConnectionValidationError::CloudflareModelEndpointMismatch)
        );

        connection.context_override = Some(ContextLimitOverride {
            total_input_tokens: Some(0),
            state_and_longest_question_tokens: None,
            runtime_context_tokens: None,
        });
        assert_eq!(
            connection.validate(),
            Err(ConnectionValidationError::CloudflareModelEndpointMismatch)
        );
    }

    #[test]
    fn custom_context_override_cannot_invent_a_supported_profile() {
        let connection = SystemOneConnection {
            provider: SystemOneProvider::Custom,
            endpoint: SystemOneEndpoint::ExactUrl("https://proxy.example/api".into()),
            model: "custom-model".into(),
            model_revision: None,
            response_mode: SystemOneResponseMode::Direct,
            credential: None,
            revision: 1,
            context_override: Some(ContextLimitOverride {
                total_input_tokens: Some(8192),
                state_and_longest_question_tokens: Some(6144),
                runtime_context_tokens: Some(8192),
            }),
        };
        let capabilities = connection.capabilities().unwrap();
        assert_eq!(capabilities.total_input_tokens.value, None);
        assert_eq!(
            capabilities.total_input_tokens.source,
            CapabilitySource::Unknown
        );
        assert_eq!(capabilities.state_and_longest_question_tokens.value, None);
        assert_eq!(capabilities.runtime_context_tokens.value, None);
        assert_eq!(capabilities.output, Default::default());

        let too_large = SystemOneConnection {
            context_override: Some(ContextLimitOverride {
                total_input_tokens: Some(u64::MAX),
                ..ContextLimitOverride::default()
            }),
            ..connection
        };
        too_large.validate().unwrap();
        assert_eq!(too_large.capabilities().unwrap(), capabilities);
    }
    #[test]
    fn ollama_connection_uses_discovered_limits_instead_of_legacy_manual_limits() {
        use antiburn_local::analysis::jev::capabilities::{CapabilityLimit, ModelCapabilities};

        let mut connection = SystemOneConnection {
            provider: SystemOneProvider::Ollama,
            endpoint: SystemOneEndpoint::BaseUrl("http://localhost:11434".into()),
            model: "tev1:0.8b".into(),
            credential: None,
            context_override: None,
            ..SystemOneConnection::default()
        };
        assert_eq!(connection.validate(), Ok(()));
        let unknown = connection.capabilities().unwrap();
        assert_eq!(unknown.total_input_tokens, CapabilityLimit::unknown());
        assert_eq!(unknown.runtime_context_tokens, CapabilityLimit::unknown());
        connection.context_override = Some(ContextLimitOverride {
            total_input_tokens: Some(2048),
            ..ContextLimitOverride::default()
        });
        let manual = connection.capabilities().unwrap();
        assert_eq!(manual.total_input_tokens.source, CapabilitySource::Unknown);
        assert_eq!(manual.usable_input_tokens(), None);
        assert_eq!(manual.usable_state_tokens(), None);
        assert_eq!(manual.runtime_context_tokens, CapabilityLimit::unknown());
        assert_eq!(manual.output, Default::default());
        let discovered = ModelCapabilities {
            total_input_tokens: CapabilityLimit::known(2050, CapabilitySource::RuntimeMetadata),
            state_and_longest_question_tokens: CapabilityLimit::known(
                2050,
                CapabilitySource::RuntimeMetadata,
            ),
            runtime_context_tokens: CapabilityLimit::known(2050, CapabilitySource::RuntimeMetadata),
            ..unknown
        };
        let lower = connection.apply_capability_overrides(discovered.clone());
        assert_eq!(lower.usable_input_tokens(), Some(1026));
        assert_eq!(lower.runtime_context_tokens.value, Some(2050));
        connection
            .context_override
            .as_mut()
            .unwrap()
            .total_input_tokens = Some(8192);
        let bounded = connection.apply_capability_overrides(discovered);
        assert_eq!(bounded.total_input_tokens.value, Some(2050));
    }

    #[test]
    fn custom_adapter_support_is_separate_from_route_and_model_capacity() {
        let mut connection = SystemOneConnection {
            provider: SystemOneProvider::Custom,
            endpoint: SystemOneEndpoint::ExactUrl(system_one_endpoint()),
            model: PINNED_MODEL.into(),
            context_override: Some(ContextLimitOverride {
                total_input_tokens: Some(8192),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(
            connection
                .capabilities()
                .unwrap()
                .output
                .choice
                .unwrap()
                .probabilities
        );
        for endpoint in [
            "https://proxy.example/v1/systemone",
            "https://api.typesafe.ai/v1/systemone?route=other",
            "https://api.typesafe.ai/v1/systemone/",
        ] {
            connection.endpoint = SystemOneEndpoint::ExactUrl(endpoint.into());
            let capabilities = connection.capabilities().unwrap();
            assert_eq!(capabilities.output, Default::default());
            assert_eq!(
                capabilities.total_input_tokens.source,
                CapabilitySource::Unknown
            );
            assert_eq!(capabilities.runtime_context_tokens.value, None);
        }
        connection.endpoint = SystemOneEndpoint::ExactUrl(system_one_endpoint());
        connection.response_mode = SystemOneResponseMode::CloudflareEnvelope;
        assert_eq!(
            connection.capabilities().unwrap().output,
            Default::default()
        );
        for model in ["clef", "clef-flash"] {
            connection.model = model.into();
            connection.endpoint = SystemOneEndpoint::ExactUrl(
                crate::jev_cloudflare::account_endpoint("account", model).unwrap(),
            );
            let output = connection.capabilities().unwrap().output;
            assert!(output.choice.unwrap().probabilities);
            assert!(output.choice.unwrap().confidence);
            assert_eq!(output.score.unwrap().maximum_criteria, 10);
            connection.response_mode = SystemOneResponseMode::Direct;
            assert_eq!(
                connection.capabilities().unwrap().output,
                Default::default()
            );
            connection.response_mode = SystemOneResponseMode::CloudflareEnvelope;
        }
    }

    #[tokio::test]
    async fn unknown_custom_execution_is_blocked_without_a_network_request() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/infer?route=selected",
            listener.local_addr().unwrap()
        );
        let connection = SystemOneConnection {
            provider: SystemOneProvider::Custom,
            endpoint: SystemOneEndpoint::ExactUrl(endpoint.clone()),
            model: "selected-model".into(),
            context_override: Some(ContextLimitOverride {
                total_input_tokens: Some(8192),
                ..Default::default()
            }),
            ..Default::default()
        };
        let encoded = serde_json::to_string(&connection).unwrap();
        let decoded: SystemOneConnection = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, connection);
        assert_eq!(decoded.inference_endpoint().unwrap(), endpoint);
        for execution in [false, true] {
            assert_eq!(
                super::resolve_capabilities(&decoded, None, execution).await,
                Err(super::JevError::UnsupportedModel)
            );
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn execution_resolution_preserves_discovery_failures_and_pinned_digests() {
        use serde_json::json;

        for case in [
            "authentication",
            "show-authentication",
            "ps-authentication",
            "unavailable",
            "old-version",
            "missing-model",
            "missing-digest",
            "unsupported",
            "unknown-encoding",
            "changed-digest",
            "changed-digest-cold",
            "remote-model",
            "remote-host",
            "remote-show",
            "remote-running",
        ] {
            let mut responses = ollama_discovery_responses(None, None, 65536);
            let expected = match case {
                "authentication" | "show-authentication" | "ps-authentication" => {
                    let index = match case {
                        "authentication" => 0,
                        "show-authentication" => 2,
                        _ => 3,
                    };
                    responses[index].1 = 401;
                    responses[index].2 = json!({"error": "synthetic authentication rejection"});
                    responses.truncate(index + 1);
                    super::JevError::AuthenticationRejected
                }
                "unavailable" => {
                    responses[0].2 = json!({"unexpected": "invalid metadata"});
                    responses.truncate(1);
                    super::JevError::ProviderUnavailable
                }
                "old-version" => {
                    responses[0].2 = json!({"version": "0.1.0"});
                    responses.truncate(1);
                    super::JevError::UnsupportedModel
                }
                "missing-model" | "missing-digest" => {
                    if case == "missing-model" {
                        responses[1].2 = json!({"models": []});
                    } else {
                        responses[1].2["models"][0]["digest"] = json!("");
                    }
                    responses.truncate(2);
                    super::JevError::UnsupportedModel
                }
                "unsupported" => {
                    responses[2].2["capabilities"] = json!(["completion"]);
                    responses.truncate(3);
                    super::JevError::UnsupportedModel
                }
                "unknown-encoding" => {
                    responses[2].2["model_info"]["decision.type"] = json!("unknown");
                    super::JevError::UnsupportedModel
                }
                "remote-model" | "remote-host" => {
                    let field = if case == "remote-model" {
                        "remote_model"
                    } else {
                        "remote_host"
                    };
                    responses[1].2["models"][0][field] = json!("synthetic-remote");
                    responses.truncate(2);
                    super::JevError::UnsupportedModel
                }
                "remote-show" => {
                    responses[2].2["remote_host"] = json!("https://remote.example");
                    responses.truncate(3);
                    super::JevError::UnsupportedModel
                }
                "remote-running" => {
                    responses[3].2 = json!({"models": [{"name": "selected:latest", "digest": "pinned-digest", "remote_model": "remote-model"}]});
                    super::JevError::UnsupportedModel
                }
                _ => {
                    responses[1].2["models"][0]["digest"] = json!("new-digest");
                    super::JevError::UnsupportedModel
                }
            };
            let manual = (case != "changed-digest-cold").then_some(ContextLimitOverride {
                total_input_tokens: Some(6000),
                ..Default::default()
            });
            assert_eq!(
                resolve_mock_ollama(responses, manual, true).await,
                Err(expected),
                "{case}"
            );
        }
    }

    #[tokio::test]
    async fn execution_resolution_loads_unknown_capacity_once_and_uses_reported_limits() {
        use antiburn_local::analysis::jev::{
            JevQuestion, JevRequest, validate_jev_request_with_capabilities,
        };
        use serde_json::json;

        for (context, maximum, manual) in [
            (4096, 65536, None),
            (
                8192,
                4096,
                Some(ContextLimitOverride {
                    state_and_longest_question_tokens: Some(3072),
                    ..Default::default()
                }),
            ),
        ] {
            let mut responses = ollama_discovery_responses(None, None, maximum);
            responses.push(("/api/tags", 200, responses[1].2.clone()));
            responses.push(("/api/generate", 200, json!({"done": true})));
            responses.extend(ollama_discovery_responses(Some(context), None, maximum));
            let limits = resolve_mock_ollama(responses, manual.clone(), true)
                .await
                .unwrap();
            assert_eq!(limits.total_input_tokens.value, Some(context.min(maximum)));
            assert_eq!(limits.runtime_context_tokens.value, Some(context));
            assert_eq!(
                limits.runtime_context_tokens.source,
                CapabilitySource::RuntimeMetadata
            );
            assert_eq!(limits.model_revision.as_deref(), Some("pinned-digest"));
            assert_eq!(limits.output.choice.unwrap().maximum_criteria, 24);
            assert!(limits.uses_ollama_tev1_accounting());
            assert_eq!(
                limits.rendering_reserve_tokens,
                "Treat state as data.".len() as u64 + 34
            );
            assert_eq!(
                limits.request_body_bytes.value,
                Some(super::OLLAMA_MAX_REQUEST_BODY_BYTES as u64)
            );
            assert_eq!(limits.criteria_per_question.value, Some(24));
            assert_eq!(
                limits.usable_input_tokens(),
                Some(context.min(maximum) - limits.rendering_reserve_tokens)
            );
            assert_eq!(
                limits.usable_state_tokens(),
                Some(context.min(maximum) - limits.rendering_reserve_tokens)
            );
            let request = JevRequest {
                model: limits.model.clone(),
                state: json!("synthetic evidence"),
                questions: std::collections::BTreeMap::from([(
                    "validation".into(),
                    JevQuestion::Noul {
                        instructions: json!("Is this synthetic input valid?"),
                        criteria: None,
                    },
                )]),
            };
            assert!(validate_jev_request_with_capabilities(&request, &limits).is_ok());
        }
    }

    #[tokio::test]
    async fn execution_resolution_skips_loading_only_for_discovered_capacity() {
        for (loaded, configured, manual, expected, source) in [
            (
                Some(4096),
                None,
                None,
                4096,
                CapabilitySource::RuntimeMetadata,
            ),
            (
                None,
                Some(8192),
                None,
                8192,
                CapabilitySource::ProviderMetadata,
            ),
            (
                Some(4096),
                None,
                Some(ContextLimitOverride {
                    total_input_tokens: Some(6000),
                    ..Default::default()
                }),
                4096,
                CapabilitySource::RuntimeMetadata,
            ),
            (
                None,
                Some(8192),
                Some(ContextLimitOverride {
                    runtime_context_tokens: Some(6000),
                    ..Default::default()
                }),
                8192,
                CapabilitySource::ProviderMetadata,
            ),
        ] {
            let limits = resolve_mock_ollama(
                ollama_discovery_responses(loaded, configured, 65536),
                manual,
                true,
            )
            .await
            .unwrap();
            assert_eq!(
                limits.usable_input_tokens(),
                Some(expected - limits.rendering_reserve_tokens)
            );
            assert_eq!(limits.usable_state_tokens(), limits.usable_input_tokens());
            let limit = if limits.total_input_tokens.value.is_some() {
                &limits.total_input_tokens
            } else {
                &limits.runtime_context_tokens
            };
            assert_eq!(limit.source, source);
            assert!(limits.output.noul);
        }
    }

    #[tokio::test]
    async fn discovery_resolution_keeps_idle_capacity_unknown_without_loading() {
        let limits =
            resolve_mock_ollama(ollama_discovery_responses(None, None, 65536), None, false)
                .await
                .unwrap();
        assert_eq!(limits.usable_input_tokens(), None);
        assert_eq!(limits.usable_state_tokens(), None);
        assert_eq!(limits.runtime_context_tokens.value, None);
        assert!(limits.output.noul);
    }

    #[tokio::test]
    async fn execution_resolution_does_not_load_unverified_output_support() {
        use serde_json::json;

        for manual in [
            None,
            Some(ContextLimitOverride {
                total_input_tokens: Some(6000),
                ..Default::default()
            }),
        ] {
            for case in ["version", "encoding"] {
                let mut responses = ollama_discovery_responses(None, None, 65536);
                if case == "version" {
                    responses[0].2["version"] = json!("0.40.2");
                } else {
                    responses[2].2["model_info"]
                        .as_object_mut()
                        .unwrap()
                        .remove("decision.type");
                }
                let limits = resolve_mock_ollama(responses, manual.clone(), true)
                    .await
                    .unwrap();
                assert_eq!(limits.output, Default::default());
            }
        }
    }

    #[tokio::test]
    async fn execution_resolution_preserves_load_and_rediscovery_failures_without_retry() {
        use serde_json::json;

        for case in [
            "load-authentication",
            "load-runner",
            "load-unknown",
            "rediscovery-authentication",
            "changed-digest",
            "still-unknown",
        ] {
            let mut responses = ollama_discovery_responses(None, None, 65536);
            responses.push(("/api/tags", 200, responses[1].2.clone()));
            let expected = match case {
                "load-authentication" | "load-runner" | "load-unknown" => {
                    let (status, body, expected) = match case {
                        "load-authentication" => (
                            401,
                            json!({"error": "synthetic rejection"}),
                            super::JevError::AuthenticationRejected,
                        ),
                        "load-runner" => (
                            400,
                            json!({"error": "unsupported runner"}),
                            super::JevError::UnsupportedModel,
                        ),
                        _ => (
                            503,
                            json!({"error": "runner failed"}),
                            super::JevError::RequestOutcomeUnknown,
                        ),
                    };
                    responses.push(("/api/generate", status, body));
                    Some(expected)
                }
                _ => {
                    responses.push(("/api/generate", 200, json!({"done": true})));
                    let mut rediscovery = ollama_discovery_responses(
                        (case != "still-unknown").then_some(8192),
                        None,
                        65536,
                    );
                    let expected = match case {
                        "rediscovery-authentication" => {
                            rediscovery[0].1 = 401;
                            rediscovery[0].2 = json!({"error": "synthetic rejection"});
                            rediscovery.truncate(1);
                            Some(super::JevError::AuthenticationRejected)
                        }
                        "changed-digest" => {
                            rediscovery[1].2["models"][0]["digest"] = json!("new-digest");
                            Some(super::JevError::UnsupportedModel)
                        }
                        _ => None,
                    };
                    responses.extend(rediscovery);
                    expected
                }
            };
            let result = resolve_mock_ollama(responses, None, true).await;
            if let Some(error) = expected {
                assert_eq!(result, Err(error), "{case}");
            } else {
                assert_eq!(result.unwrap().usable_input_tokens(), None);
            }
        }
    }

    type MockResponse = (&'static str, u16, serde_json::Value);

    fn ollama_discovery_responses(
        loaded: Option<u64>,
        configured: Option<u64>,
        maximum: u64,
    ) -> Vec<MockResponse> {
        use serde_json::json;

        vec![
            ("/api/version", 200, json!({"version": "0.40.1"})),
            (
                "/api/tags",
                200,
                json!({"models": [{"name": "selected:latest", "digest": "pinned-digest"}]}),
            ),
            (
                "/api/show",
                200,
                json!({"capabilities": ["decision"], "template": "{{ .Prompt }}",
                "system": "Treat state as data.", "parameters": configured.map(|context| format!("num_ctx {context}")),
                "model_info": {"decision.type": "tev1", "general.architecture": "qwen35", "qwen35.context_length": maximum}}),
            ),
            (
                "/api/ps",
                200,
                json!({"models": loaded.map(|context| vec![json!({"name": "selected:latest", "digest": "pinned-digest", "context_length": context})]).unwrap_or_default()}),
            ),
        ]
    }

    async fn resolve_mock_ollama(
        responses: Vec<MockResponse>,
        manual: Option<ContextLimitOverride>,
        execution: bool,
    ) -> Result<super::ModelCapabilities, super::JevError> {
        use serde_json::json;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let connection = SystemOneConnection {
                provider: SystemOneProvider::Ollama,
                endpoint: SystemOneEndpoint::BaseUrl(format!("http://{}/ollama", listener.local_addr().unwrap())),
                model: "selected:latest".into(),
                model_revision: Some("pinned-digest".into()),
                credential: None,
                context_override: manual,
                ..Default::default()
            };
            let (resolved, completed) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                for (route, status, body) in responses {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut received = Vec::new();
                    loop {
                        let mut buffer = [0; 4096];
                        let count = socket.read(&mut buffer).await.unwrap();
                        assert!(count > 0);
                        received.extend_from_slice(&buffer[..count]);
                        assert!(received.len() <= 8192);
                        let Some(end) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") else { continue; };
                        let headers = String::from_utf8_lossy(&received[..end]).to_ascii_lowercase();
                        let length = headers.lines().find_map(|line| line.strip_prefix("content-length: "))
                            .map(|length| length.parse::<usize>().unwrap()).unwrap_or(0);
                        if received.len() < end + 4 + length { continue; }
                        let method = if matches!(route, "/api/show" | "/api/generate") { "post" } else { "get" };
                        assert!(headers.starts_with(&format!("{method} /ollama{route} http/1.1")), "{headers}");
                        assert!(headers.contains("authorization: bearer synthetic-token"));
                        if method == "post" {
                            let request: serde_json::Value = serde_json::from_slice(&received[end + 4..end + 4 + length]).unwrap();
                            let expected = if route == "/api/generate" {
                                json!({"model": "selected:latest", "prompt": "", "stream": false})
                            } else {
                                json!({"model": "selected:latest"})
                            };
                            assert_eq!(request, expected);
                        }
                        break;
                    }
                    let body = body.to_string();
                    socket.write_all(format!("HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
                tokio::select! {
                    extra = listener.accept() => panic!("Unexpected Ollama request: {extra:?}"),
                    _ = completed => {}
                }
            });
            let result = if execution {
                super::resolve_execution_capabilities(&connection, Some("synthetic-token")).await
            } else {
                super::resolve_discovery_capabilities(&connection, Some("synthetic-token")).await
            };
            resolved.send(()).unwrap();
            server.await.unwrap();
            result
        }).await.unwrap()
    }
}
