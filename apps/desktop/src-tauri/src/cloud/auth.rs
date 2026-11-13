use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::time::Duration;
use url::Url;

#[derive(Clone)]
pub struct Config {
    pub authorization: Url,
    pub token: Url,
    pub profile: Url,
    pub revocation: Url,
    pub client_id: String,
    pub callback: String,
}

impl Config {
    pub fn from_build(scheme: &str) -> Option<Self> {
        if option_env!("ANTIBURN_CLOUD_AUTH_VERIFIED") != Some("enabled") {
            return None;
        }
        Self::parse(
            option_env!("ANTIBURN_CLOUD_AUTHORIZATION_URL")?,
            option_env!("ANTIBURN_CLOUD_TOKEN_URL")?,
            option_env!("ANTIBURN_CLOUD_PROFILE_URL")?,
            option_env!("ANTIBURN_CLOUD_REVOCATION_URL")?,
            option_env!("ANTIBURN_CLOUD_CLIENT_ID")?,
            option_env!("ANTIBURN_CLOUD_CALLBACK_URI")?,
            scheme,
        )
    }

    pub fn parse(
        authorization: &str,
        token: &str,
        profile: &str,
        revocation: &str,
        client_id: &str,
        callback: &str,
        scheme: &str,
    ) -> Option<Self> {
        let endpoint = |value: &str| {
            let url = Url::parse(value).ok()?;
            (url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
                && url.query().is_none())
            .then_some(url)
        };
        let authorization = endpoint(authorization)?;
        let token = endpoint(token)?;
        let profile = endpoint(profile)?;
        let revocation = endpoint(revocation)?;
        if [&token, &profile, &revocation]
            .iter()
            .any(|url| url.origin() != authorization.origin())
            || client_id.is_empty()
            || client_id.len() > 256
            || callback != format!("{scheme}://cloud/auth")
        {
            return None;
        }
        Some(Self {
            authorization,
            token,
            profile,
            revocation,
            client_id: client_id.to_owned(),
            callback: callback.to_owned(),
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    Offline,
    Rejected,
    InvalidResponse,
    CredentialFailed,
    Cancelled,
    Unavailable,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub id: String,
    pub login: String,
}

#[derive(Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub token_type: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub state: String,
    pub verifier: Option<String>,
    pub expires_at: i64,
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Secrets {
    pub pending: Option<Attempt>,
    pub refresh_token: Option<String>,
}

pub trait Vault: Send + Sync {
    fn read(&self) -> Result<Secrets, Failure>;
    fn write(&self, secrets: &Secrets) -> Result<(), Failure>;
}

pub struct KeyringVault(pub String);
impl Vault for KeyringVault {
    fn read(&self) -> Result<Secrets, Failure> {
        let entry = keyring::Entry::new(&self.0, "cloud-session-v1")
            .map_err(|_| Failure::CredentialFailed)?;
        match entry.get_password() {
            Ok(value) => serde_json::from_str(&value).map_err(|_| Failure::CredentialFailed),
            Err(keyring::Error::NoEntry) => Ok(Secrets::default()),
            Err(_) => Err(Failure::CredentialFailed),
        }
    }
    fn write(&self, secrets: &Secrets) -> Result<(), Failure> {
        let entry = keyring::Entry::new(&self.0, "cloud-session-v1")
            .map_err(|_| Failure::CredentialFailed)?;
        if secrets.pending.is_none() && secrets.refresh_token.is_none() {
            return match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err(Failure::CredentialFailed),
            };
        }
        let value = serde_json::to_string(secrets).map_err(|_| Failure::CredentialFailed)?;
        entry
            .set_password(&value)
            .map_err(|_| Failure::CredentialFailed)
    }
}

pub trait Transport: Send + Sync {
    fn exchange(&self, config: &Config, fields: &[(&str, &str)]) -> Result<Tokens, Failure>;
    fn profile(&self, config: &Config, access: &str) -> Result<Identity, Failure>;
    fn revoke(&self, config: &Config, refresh: &str) -> Result<(), Failure>;
}

pub struct HttpTransport;
fn client() -> Result<reqwest::blocking::Client, Failure> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| Failure::Offline)
}
fn decode<T: serde::de::DeserializeOwned>(
    response: reqwest::blocking::Response,
) -> Result<T, Failure> {
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::BAD_REQUEST
    {
        return Err(Failure::Rejected);
    }
    if !status.is_success() {
        return Err(Failure::Offline);
    }
    let mut bytes = Vec::new();
    response
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure::Offline)?;
    if bytes.len() > 65_536 {
        return Err(Failure::InvalidResponse);
    }
    serde_json::from_slice(&bytes).map_err(|_| Failure::InvalidResponse)
}
impl Transport for HttpTransport {
    fn exchange(&self, config: &Config, fields: &[(&str, &str)]) -> Result<Tokens, Failure> {
        let tokens: Tokens = decode(
            client()?
                .post(config.token.clone())
                .form(fields)
                .send()
                .map_err(|_| Failure::Offline)?,
        )?;
        if !tokens.token_type.eq_ignore_ascii_case("bearer")
            || tokens.access_token.is_empty()
            || tokens.refresh_token.is_empty()
            || tokens.access_token.len() > 16_384
            || tokens.refresh_token.len() > 16_384
            || !(60..=86_400).contains(&tokens.expires_in)
        {
            return Err(Failure::InvalidResponse);
        }
        Ok(tokens)
    }
    fn profile(&self, config: &Config, access: &str) -> Result<Identity, Failure> {
        let identity: Identity = decode(
            client()?
                .get(config.profile.clone())
                .bearer_auth(access)
                .send()
                .map_err(|_| Failure::Offline)?,
        )?;
        if identity.id.is_empty()
            || identity.id.len() > 256
            || identity.login.is_empty()
            || identity.login.len() > 100
            || identity.login.chars().any(char::is_control)
        {
            return Err(Failure::InvalidResponse);
        }
        Ok(identity)
    }
    fn revoke(&self, config: &Config, refresh: &str) -> Result<(), Failure> {
        let response = client()?
            .post(config.revocation.clone())
            .form(&[
                ("token", refresh),
                ("token_type_hint", "refresh_token"),
                ("client_id", &config.client_id),
            ])
            .send()
            .map_err(|_| Failure::Offline)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Failure::Offline)
        }
    }
}

pub fn random() -> Result<String, Failure> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| Failure::Unavailable)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub enum Callback {
    Marketing,
    Code(String),
    Cancelled,
}
pub fn validate_callback(raw: &str, scheme: &str, attempt: &Attempt, now: i64) -> Option<Callback> {
    if raw.len() > 8192 || now >= attempt.expires_at {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    let path = if attempt.verifier.is_some() {
        "/auth"
    } else {
        "/return"
    };
    if url.scheme() != scheme
        || url.host_str() != Some("cloud")
        || url.path() != path
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let mut fields = std::collections::HashMap::new();
    for (key, value) in url.query_pairs() {
        if !["state", "code", "error"].contains(&key.as_ref())
            || fields
                .insert(key.into_owned(), value.into_owned())
                .is_some()
        {
            return None;
        }
    }
    if fields.get("state") != Some(&attempt.state) {
        return None;
    }
    if attempt.verifier.is_none() {
        return (fields.len() == 1).then_some(Callback::Marketing);
    }
    match (fields.get("code"), fields.get("error")) {
        (Some(code), None)
            if !code.is_empty() && code.len() <= 2048 && !code.chars().any(char::is_control) =>
        {
            Some(Callback::Code(code.clone()))
        }
        (None, Some(error)) if error == "access_denied" => Some(Callback::Cancelled),
        _ => None,
    }
}
