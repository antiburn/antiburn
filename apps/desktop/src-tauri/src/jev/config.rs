use antiburn_local::analysis::jev::PINNED_MODEL;
use antiburn_local::analysis::jev::capabilities::{
    CapabilityLimit, ModelCapabilities, ModelLimitOverride,
};
use serde::{Deserialize, Serialize};
use url::Url;

const SYSTEM_ONE_PROTOCOL: &str = "https://";
const SYSTEM_ONE_HOSTNAME: &str = "api.typesafe.ai";
const SYSTEM_ONE_API_PATH: &str = "/v1/systemone";
const MAX_MODEL_NAME_BYTES: usize = 128;
const MAX_ACCOUNT_ID_BYTES: usize = 64;
const MAX_MANUAL_CONTEXT_TOKENS: u64 = antiburn_local::analysis::jev::MAX_REQUEST_TOKENS;
pub(crate) const OLLAMA_MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;

pub fn system_one_endpoint() -> String {
    format!("{SYSTEM_ONE_PROTOCOL}{SYSTEM_ONE_HOSTNAME}{SYSTEM_ONE_API_PATH}")
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
#[serde(rename_all = "camelCase")]
pub struct SystemOneConnection {
    pub provider: SystemOneProvider,
    pub endpoint: SystemOneEndpoint,
    pub model: String,
    #[serde(default)]
    pub model_revision: Option<String>,
    #[serde(default = "default_response_mode")]
    pub response_mode: SystemOneResponseMode,
    pub credential: Option<CredentialReference>,
    pub revision: u64,
    #[serde(default)]
    pub context_override: Option<ContextLimitOverride>,
}

fn default_response_mode() -> SystemOneResponseMode {
    SystemOneResponseMode::Direct
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionValidationError {
    InvalidModel,
    InvalidEndpoint,
    InvalidCredentialReference,
    InvalidContextOverride,
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
    /// Keep current installations on their existing TypeSafe endpoint and model.
    pub fn jev_default() -> Self {
        Self {
            provider: SystemOneProvider::Jev,
            endpoint: SystemOneEndpoint::ProviderDefault,
            model: PINNED_MODEL.to_owned(),
            model_revision: None,
            response_mode: SystemOneResponseMode::Direct,
            credential: Some(CredentialReference::LegacyTypeSafe),
            revision: 1,
            context_override: None,
        }
    }

    pub fn validate(&self) -> Result<(), ConnectionValidationError> {
        if self.revision == 0 {
            return Err(ConnectionValidationError::InvalidRevision);
        }
        if !valid_model_name(&self.model) {
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
        if let Some(override_limits) = &self.context_override
            && !valid_context_override(override_limits)
        {
            return Err(ConnectionValidationError::InvalidContextOverride);
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
                if self.model == PINNED_MODEL =>
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
        let mut capabilities = match self.provider {
            SystemOneProvider::Jev => ModelCapabilities::jev_default(),
            SystemOneProvider::Cloudflare => {
                crate::jev_cloudflare::cloudflare_capabilities(&self.model)
            }
            SystemOneProvider::Ollama | SystemOneProvider::Custom => {
                let mut limits = ModelCapabilities::jev_default();
                limits.model = self.model.clone();
                limits.total_input_tokens = CapabilityLimit::unknown();
                limits.state_and_longest_question_tokens = CapabilityLimit::unknown();
                limits.runtime_context_tokens = CapabilityLimit::unknown();
                if self.provider == SystemOneProvider::Ollama {
                    use antiburn_local::analysis::jev::capabilities::CapabilitySource;
                    limits.total_input_tokens =
                        CapabilityLimit::known(8192, CapabilitySource::DocumentedDefault);
                    limits.runtime_context_tokens =
                        CapabilityLimit::known(8192, CapabilitySource::DocumentedDefault);
                    limits.rendering_reserve_tokens = 1024;
                    limits.request_body_bytes = CapabilityLimit::known(
                        OLLAMA_MAX_REQUEST_BODY_BYTES as u64,
                        CapabilitySource::DocumentedDefault,
                    );
                }
                limits
            }
        };
        capabilities = self.apply_capability_overrides(capabilities);
        Ok(capabilities)
    }

    pub(crate) fn apply_capability_overrides(
        &self,
        mut capabilities: ModelCapabilities,
    ) -> ModelCapabilities {
        if let Some(override_limits) = &self.context_override {
            capabilities = capabilities.with_override(&ModelLimitOverride {
                total_input_tokens: override_limits.total_input_tokens,
                state_and_longest_question_tokens: override_limits
                    .state_and_longest_question_tokens,
                runtime_context_tokens: override_limits.runtime_context_tokens,
            });
        }
        if capabilities.model_revision.is_none() {
            capabilities.model_revision.clone_from(&self.model_revision);
        }
        capabilities
    }
}

fn valid_model_name(value: &str) -> bool {
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

fn valid_context_override(value: &ContextLimitOverride) -> bool {
    [
        value.total_input_tokens,
        value.state_and_longest_question_tokens,
        value.runtime_context_tokens,
    ]
    .into_iter()
    .flatten()
    .all(|tokens| (1..=MAX_MANUAL_CONTEXT_TOKENS).contains(&tokens))
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
    use super::MAX_MANUAL_CONTEXT_TOKENS;
    use super::{
        ConnectionValidationError, ContextLimitOverride, CredentialReference, SystemOneConnection,
        SystemOneEndpoint, SystemOneProvider, SystemOneResponseMode, system_one_endpoint,
    };
    use antiburn_local::analysis::jev::PINNED_MODEL;
    use antiburn_local::analysis::jev::capabilities::CapabilitySource;
    use serde::Deserialize;

    #[test]
    fn default_preserves_the_current_jev_endpoint_model_and_credential_reference() {
        let connection = SystemOneConnection::default();

        assert_eq!(connection.provider, SystemOneProvider::Jev);
        assert_eq!(connection.endpoint, SystemOneEndpoint::ProviderDefault);
        assert_eq!(connection.model, PINNED_MODEL);
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
        let mut encoded = serde_json::to_value(SystemOneConnection::default()).unwrap();
        encoded.as_object_mut().unwrap().remove("model_revision");
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
            context_override: None,
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
    fn rejects_cloudflare_route_and_context_override_mismatches() {
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
            Err(ConnectionValidationError::InvalidContextOverride)
        );
    }

    #[test]
    fn custom_context_override_is_applied_as_a_bounded_manual_limit() {
        let connection = SystemOneConnection {
            provider: SystemOneProvider::Custom,
            endpoint: SystemOneEndpoint::ExactUrl("https://proxy.example/api".into()),
            model: "custom-model".into(),
            model_revision: None,
            response_mode: SystemOneResponseMode::Direct,
            credential: None,
            revision: 1,
            context_override: Some(ContextLimitOverride {
                total_input_tokens: Some(4096),
                state_and_longest_question_tokens: Some(2048),
                runtime_context_tokens: Some(8192),
            }),
        };
        let capabilities = connection.capabilities().unwrap();
        assert_eq!(capabilities.total_input_tokens.value, Some(4096));
        assert_eq!(
            capabilities.total_input_tokens.source,
            CapabilitySource::Manual
        );
        assert_eq!(
            capabilities.state_and_longest_question_tokens.value,
            Some(2048)
        );
        assert_eq!(capabilities.runtime_context_tokens.value, Some(8192));

        let too_large = SystemOneConnection {
            context_override: Some(ContextLimitOverride {
                total_input_tokens: Some(MAX_MANUAL_CONTEXT_TOKENS + 1),
                ..ContextLimitOverride::default()
            }),
            ..connection
        };
        assert_eq!(
            too_large.validate(),
            Err(ConnectionValidationError::InvalidContextOverride)
        );
    }
}
