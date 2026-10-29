use super::{client, config, jev_cloudflare, jev_ollama};
use antiburn_local::analysis::jev::{
    JevError, JevRequest, JevResponse, capabilities::ModelCapabilities,
};
#[cfg(test)]
use config::ContextLimitOverride;
use config::{SystemOneConnection, SystemOneEndpoint, SystemOneProvider, SystemOneResponseMode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::OnceLock;

#[path = "provider_config.rs"]
mod provider_config;
use provider_config::LOCAL_BASE;
pub(crate) use provider_config::ProviderPreset;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EvalConfiguration {
    pub(crate) preset: ProviderPreset,
    pub(crate) connection: SystemOneConnection,
    pub(crate) capabilities: ModelCapabilities,
}

fn selected_preset() -> ProviderPreset {
    let value = match std::env::var("ANTIBURN_EVAL_PROVIDER") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => "jev".into(),
        Err(std::env::VarError::NotUnicode(_)) => panic!("ANTIBURN_EVAL_PROVIDER must be UTF-8"),
    };
    ProviderPreset::parse(&value).expect("Select a closed provider preset")
}

fn discover_configuration() -> EvalConfiguration {
    let preset = selected_preset();
    let model = match std::env::var("ANTIBURN_EVAL_MODEL") {
        Ok(model) => Some(model),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => panic!("ANTIBURN_EVAL_MODEL: {error}"),
    };
    let mut connection = preset
        .environment_connection(model.as_deref())
        .expect("valid preset and model");
    let capabilities = if preset.local() {
        let model = connection.model.clone();
        let expected_model = model.clone();
        let discovered = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Discovery runtime builds");
            runtime.block_on(async {
                jev_ollama::OllamaClient::new(LOCAL_BASE, None)?
                    .discover(&model)
                    .await
            })
        })
        .join()
        .expect("Ollama discovery does not panic")
        .expect("Production Ollama discovery must succeed");
        assert_eq!(
            discovered.name, expected_model,
            "Discovery must bind the selected model"
        );
        connection.model_revision = discovered.digest;
        discovered.capabilities
    } else {
        connection
            .capabilities()
            .expect("Production selected-connection limits must resolve")
    };
    EvalConfiguration {
        preset,
        capabilities: connection.apply_capability_overrides(capabilities),
        connection,
    }
}

pub(crate) fn configuration() -> &'static EvalConfiguration {
    static CONFIGURATION: OnceLock<EvalConfiguration> = OnceLock::new();
    CONFIGURATION.get_or_init(discover_configuration)
}

impl EvalConfiguration {
    pub(crate) fn identity(&self) -> Value {
        json!({"preset":self.preset,"provider":self.connection.provider,"model":self.capabilities.model,
            "model_revision":self.capabilities.model_revision,"response_mode":self.connection.response_mode,
            "capabilities":self.capabilities})
    }
}

#[derive(Clone)]
enum Transport {
    Jev(client::TypeSafeClient),
    Ollama(jev_ollama::OllamaClient),
    Cloudflare(jev_cloudflare::CloudflareClient),
    Custom {
        endpoint: String,
        credential: Option<std::sync::Arc<str>>,
        mode: SystemOneResponseMode,
    },
}

#[derive(Clone)]
pub(crate) struct EvalClient {
    configuration: EvalConfiguration,
    transport: Transport,
}

impl EvalClient {
    pub(crate) fn capabilities(&self) -> &ModelCapabilities {
        &self.configuration.capabilities
    }

    pub(crate) async fn from_environment() -> Result<Self, JevError> {
        let configuration = configuration().clone();
        let credential = configuration.preset.environment_credential()?;
        let transport = match configuration.connection.provider {
            SystemOneProvider::Jev => Transport::Jev(client::TypeSafeClient::new(
                credential.ok_or(JevError::AuthenticationRejected)?,
            )?),
            SystemOneProvider::Ollama => Transport::Ollama(
                jev_ollama::OllamaClient::new(LOCAL_BASE, None).map_err(ollama_error)?,
            ),
            SystemOneProvider::Cloudflare => {
                let SystemOneEndpoint::CloudflareAccount(account) =
                    &configuration.connection.endpoint
                else {
                    return Err(JevError::InvalidRequestSchema);
                };
                Transport::Cloudflare(jev_cloudflare::CloudflareClient::new(
                    account.clone(),
                    credential.ok_or(JevError::AuthenticationRejected)?,
                    configuration.connection.model.clone(),
                )?)
            }
            SystemOneProvider::Custom => Transport::Custom {
                endpoint: configuration
                    .connection
                    .inference_endpoint()
                    .map_err(|_| JevError::InvalidRequestSchema)?,
                credential: credential.map(Into::into),
                mode: configuration.connection.response_mode,
            },
        };
        Ok(Self {
            configuration,
            transport,
        })
    }

    pub(crate) async fn evaluate(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        let capabilities = &self.configuration.capabilities;
        match &self.transport {
            Transport::Jev(client) => {
                client
                    .evaluate_at_with_capabilities_and_usage(
                        request,
                        &config::system_one_endpoint(),
                        capabilities,
                        |_| Ok(()),
                    )
                    .await
            }
            Transport::Ollama(client) => client
                .evaluate(request, capabilities)
                .await
                .map_err(ollama_error),
            Transport::Cloudflare(client) => {
                client
                    .evaluate_with_capabilities(request, capabilities)
                    .await
            }
            Transport::Custom {
                endpoint,
                credential,
                mode,
            } => {
                client::evaluate_custom(
                    endpoint,
                    credential.as_deref(),
                    *mode,
                    request,
                    capabilities,
                )
                .await
            }
        }
    }
}

fn ollama_error(error: jev_ollama::OllamaError) -> JevError {
    use jev_ollama::OllamaError;
    // Match the shared worker's production error categories.
    match error {
        OllamaError::AuthenticationRejected => JevError::AuthenticationRejected,
        OllamaError::RequestOutcomeUnknown => JevError::RequestOutcomeUnknown,
        OllamaError::ResponseTooLarge => JevError::ResponseTooLarge,
        OllamaError::ResponseDecode => JevError::ResponseDecode,
        OllamaError::ContextRejected => JevError::ContextRejected,
        OllamaError::InvalidRequest | OllamaError::RequestBodyTooLarge => {
            JevError::InvalidRequestSchema
        }
        _ => JevError::ProviderUnavailable,
    }
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
