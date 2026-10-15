//! Shared preset connections and credentials for evaluation and benchmarks.

use super::{config, jev_cloudflare};
use antiburn_local::analysis::jev::JevError;
use config::{
    ContextLimitOverride, SystemOneConnection, SystemOneEndpoint, SystemOneProvider,
    SystemOneResponseMode,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub(crate) const LOCAL_BASE: &str = "http://127.0.0.1:11434";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ProviderPreset {
    Jev,
    OllamaNimble,
    OllamaClefFlash,
    OllamaTevSmall,
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
        serde_json::from_value(json!(value)).map_err(|_| "Unknown provider preset".into())
    }

    pub(crate) fn local(self) -> bool {
        matches!(
            self,
            Self::OllamaNimble
                | Self::OllamaClefFlash
                | Self::OllamaTevSmall
                | Self::CustomOllamaNimbleDirect
                | Self::CustomOllamaClefFlashDirect
        )
    }

    pub(crate) fn cloudflare(self) -> bool {
        matches!(
            self,
            Self::CloudflareClef
                | Self::CloudflareClefFlash
                | Self::CustomCloudflareClefEnvelope
                | Self::CustomCloudflareClefFlashEnvelope
        )
    }

    pub(crate) fn model(self) -> &'static str {
        match self {
            Self::Jev | Self::CustomJevDirect => antiburn_local::analysis::jev::PINNED_MODEL,
            Self::OllamaNimble | Self::CustomOllamaNimbleDirect => "nimble:latest",
            Self::OllamaClefFlash | Self::CustomOllamaClefFlashDirect => "clef-flash:latest",
            Self::OllamaTevSmall => "tev1:0.8b",
            Self::CloudflareClef | Self::CustomCloudflareClefEnvelope => "clef",
            Self::CloudflareClefFlash | Self::CustomCloudflareClefFlashEnvelope => "clef-flash",
        }
    }

    #[cfg(test)]
    pub(crate) fn connection(self, account: Option<&str>) -> Result<SystemOneConnection, String> {
        self.connection_with_model(account, None)
    }

    pub(crate) fn connection_with_model(
        self,
        account: Option<&str>,
        model: Option<&str>,
    ) -> Result<SystemOneConnection, String> {
        let mut connection = SystemOneConnection::jev_default();
        connection.model = model.unwrap_or_else(|| self.model()).into();
        connection.credential = None;
        match self {
            Self::Jev => {}
            Self::OllamaNimble | Self::OllamaClefFlash | Self::OllamaTevSmall => {
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
                        &connection.model,
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

    pub(crate) fn environment_connection(
        self,
        model: Option<&str>,
    ) -> Result<SystemOneConnection, String> {
        let account = if self.cloudflare() {
            Some(
                std::env::var("CLOUDFLARE_ACCOUNT_ID")
                    .map_err(|_| "CLOUDFLARE_ACCOUNT_ID is required")?,
            )
        } else {
            None
        };
        self.connection_with_model(account.as_deref(), model)
    }

    pub(crate) fn environment_credential(self) -> Result<Option<String>, JevError> {
        if self.local() {
            return Ok(None);
        }
        std::env::var(if self.cloudflare() {
            "CLOUDFLARE_AUTH_TOKEN"
        } else {
            "TYPESAFE_API_KEY"
        })
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(Some)
        .ok_or(JevError::AuthenticationRejected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_override_precedes_validation_and_endpoint_binding() {
        assert!(
            ProviderPreset::Jev
                .connection_with_model(None, Some("other-model"))
                .is_err()
        );
        let local = ProviderPreset::OllamaNimble
            .connection_with_model(None, Some("other-model:latest"))
            .unwrap();
        assert_eq!(local.model, "other-model:latest");
        let hosted = ProviderPreset::CustomCloudflareClefEnvelope
            .connection_with_model(Some("synthetic-account"), Some("clef-flash"))
            .unwrap();
        assert!(
            hosted
                .inference_endpoint()
                .unwrap()
                .ends_with("/clef-flash")
        );
        assert!(
            ProviderPreset::OllamaNimble
                .connection_with_model(None, Some(""))
                .is_err()
        );
        assert!(ProviderPreset::CloudflareClef.connection(None).is_err());
    }
}
