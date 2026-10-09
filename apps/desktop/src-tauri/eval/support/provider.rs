use super::{client, config, jev_cloudflare, jev_ollama};
use antiburn_local::analysis::jev::{
    JevError, JevRequest, JevResponse, capabilities::ModelCapabilities,
};
use config::{
    ContextLimitOverride, SystemOneConnection, SystemOneEndpoint, SystemOneProvider,
    SystemOneResponseMode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::OnceLock;

const LOCAL_BASE: &str = "http://127.0.0.1:11434";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ProviderPreset {
    Jev,
    OllamaNimble,
    OllamaClefFlash,
    CloudflareClef,
    CloudflareClefFlash,
    CustomJevDirect,
    CustomOllamaNimbleDirect,
    CustomOllamaClefFlashDirect,
    CustomCloudflareClefEnvelope,
    CustomCloudflareClefFlashEnvelope,
}

impl ProviderPreset {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        serde_json::from_value(json!(value))
            .map_err(|_| "Unknown ANTIBURN_EVAL_PROVIDER preset".into())
    }

    fn local(self) -> bool {
        matches!(
            self,
            Self::OllamaNimble
                | Self::OllamaClefFlash
                | Self::CustomOllamaNimbleDirect
                | Self::CustomOllamaClefFlashDirect
        )
    }

    fn cloudflare(self) -> bool {
        matches!(
            self,
            Self::CloudflareClef
                | Self::CloudflareClefFlash
                | Self::CustomCloudflareClefEnvelope
                | Self::CustomCloudflareClefFlashEnvelope
        )
    }

    fn model(self) -> &'static str {
        match self {
            Self::Jev | Self::CustomJevDirect => antiburn_local::analysis::jev::PINNED_MODEL,
            Self::OllamaNimble | Self::CustomOllamaNimbleDirect => "nimble:latest",
            Self::OllamaClefFlash | Self::CustomOllamaClefFlashDirect => "clef-flash:latest",
            Self::CloudflareClef | Self::CustomCloudflareClefEnvelope => "clef",
            Self::CloudflareClefFlash | Self::CustomCloudflareClefFlashEnvelope => "clef-flash",
        }
    }

    fn connection(self, account: Option<&str>) -> Result<SystemOneConnection, String> {
        let mut connection = SystemOneConnection::jev_default();
        connection.model = self.model().into();
        connection.credential = None;
        match self {
            Self::Jev => {}
            Self::OllamaNimble | Self::OllamaClefFlash => {
                connection.provider = SystemOneProvider::Ollama;
                connection.endpoint = SystemOneEndpoint::BaseUrl(LOCAL_BASE.into());
            }
            Self::CloudflareClef | Self::CloudflareClefFlash => {
                connection.provider = SystemOneProvider::Cloudflare;
                connection.endpoint = SystemOneEndpoint::CloudflareAccount(
                    account.ok_or("CLOUDFLARE_ACCOUNT_ID is required")?.into(),
                );
            }
            _ => {
                connection.provider = SystemOneProvider::Custom;
                connection.context_override = Some(ContextLimitOverride {
                    total_input_tokens: Some(if self.local() { 8192 } else { 65_536 }),
                    ..ContextLimitOverride::default()
                });
                let endpoint = if self.local() {
                    format!("{LOCAL_BASE}/v1/systemone")
                } else if self.cloudflare() {
                    connection.response_mode = SystemOneResponseMode::CloudflareEnvelope;
                    jev_cloudflare::account_endpoint(
                        account.ok_or("CLOUDFLARE_ACCOUNT_ID is required")?,
                        self.model(),
                    )
                    .map_err(|error| error.to_string())?
                } else {
                    config::system_one_endpoint()
                };
                connection.endpoint = SystemOneEndpoint::ExactUrl(endpoint);
            }
        }
        connection
            .validate()
            .map_err(|_| "Invalid provider preset connection")?;
        Ok(connection)
    }
}

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

fn account(preset: ProviderPreset) -> Option<String> {
    preset
        .cloudflare()
        .then(|| std::env::var("CLOUDFLARE_ACCOUNT_ID").expect("CLOUDFLARE_ACCOUNT_ID is required"))
}

fn discover_configuration() -> EvalConfiguration {
    let preset = selected_preset();
    let account = account(preset);
    let mut connection = preset.connection(account.as_deref()).expect("valid preset");
    let capabilities = if preset.local() {
        let discovered = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Discovery runtime builds");
            runtime.block_on(async {
                jev_ollama::OllamaClient::new(LOCAL_BASE, None)?
                    .discover(preset.model())
                    .await
            })
        })
        .join()
        .expect("Ollama discovery does not panic")
        .expect("Production Ollama discovery must succeed");
        assert_eq!(
            discovered.name,
            preset.model(),
            "Discovery must bind the selected model"
        );
        connection.model_revision = discovered.digest;
        discovered.capabilities
    } else if preset.cloudflare() {
        jev_cloudflare::cloudflare_capabilities(preset.model())
    } else {
        ModelCapabilities::jev_default()
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

fn credential(name: &str) -> Result<String, JevError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or(JevError::AuthenticationRejected)
}

impl EvalClient {
    pub(crate) async fn from_environment() -> Result<Self, JevError> {
        let configuration = configuration().clone();
        let transport = match configuration.connection.provider {
            SystemOneProvider::Jev => Transport::Jev(client::TypeSafeClient::new(credential(
                "TYPESAFE_API_KEY",
            )?)?),
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
                    credential("CLOUDFLARE_AUTH_TOKEN")?,
                    configuration.connection.model.clone(),
                )?)
            }
            SystemOneProvider::Custom => Transport::Custom {
                endpoint: configuration
                    .connection
                    .inference_endpoint()
                    .map_err(|_| JevError::InvalidRequestSchema)?,
                credential: if configuration.preset.local() {
                    None
                } else {
                    Some(
                        credential(if configuration.preset.cloudflare() {
                            "CLOUDFLARE_AUTH_TOKEN"
                        } else {
                            "TYPESAFE_API_KEY"
                        })?
                        .into(),
                    )
                },
                mode: configuration.connection.response_mode,
            },
        };
        Ok(Self {
            configuration,
            transport,
        })
    }

    pub(crate) async fn evaluate(
        &self,
        request: &JevRequest,
        blocking: bool,
    ) -> Result<JevResponse, JevError> {
        let capabilities = &self.configuration.capabilities;
        match &self.transport {
            Transport::Jev(client) if blocking => {
                let client = client.clone();
                let request = request.clone();
                tokio::task::spawn_blocking(move || client.evaluate(&request))
                    .await
                    .expect("Blocking production transport must not panic")
            }
            Transport::Jev(client) => client.evaluate_async(request).await,
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
        OllamaError::InvalidRequest | OllamaError::RequestBodyTooLarge => {
            JevError::InvalidRequestSchema
        }
        _ => JevError::ProviderUnavailable,
    }
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
