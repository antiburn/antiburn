use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Read, time::Duration};
use url::Url;

pub const CLIENT_ID: &str = "antiburn-desktop";
pub const SCOPE: &str = "capture:write";
pub const MAX_ACCESS_SECONDS: u64 = 900;
pub const MAX_GRANT_SECONDS: i64 = 365 * 24 * 60 * 60;

#[derive(Clone)]
pub struct Config {
    pub issuer: String,
    pub resource: String,
    pub authorization: Url,
    pub token: Url,
    pub session: Url,
    pub revocation: Url,
}

impl Config {
    pub fn from_build() -> Option<Self> {
        Self::parse(
            option_env!("ANTIBURN_CLOUD_ISSUER")?,
            option_env!("ANTIBURN_CLOUD_RESOURCE")?,
        )
    }

    pub fn parse(issuer: &str, resource: &str) -> Option<Self> {
        fn canonical(value: &str) -> Option<Url> {
            let url = Url::parse(value).ok()?;
            (url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/"
                && url.as_str() == value)
                .then_some(url)
        }
        let issuer_url = canonical(issuer)?;
        let resource_url = canonical(resource)?;
        Some(Self {
            issuer: issuer.to_owned(),
            resource: resource.to_owned(),
            authorization: issuer_url.join("oauth/authorize").ok()?,
            token: issuer_url.join("oauth/token").ok()?,
            revocation: issuer_url.join("oauth/revoke").ok()?,
            session: resource_url.join("v1/desktop/session").ok()?,
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
    ReauthRequired,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct User {
    pub id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    Personal,
    Team,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub kind: WorkspaceKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub user: User,
    pub grant_id: String,
    pub device_id: String,
    pub workspace: Workspace,
    pub audience: String,
    pub actions: Vec<String>,
    pub expires_at: i64,
}

impl Session {
    pub fn validate(&self, config: &Config, now: i64) -> Result<(), Failure> {
        let bounded = |value: &str, max: usize| {
            !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
        };
        if !bounded(&self.user.id, 256)
            || !bounded(&self.user.display_name, 100)
            || !bounded(&self.grant_id, 256)
            || !bounded(&self.device_id, 256)
            || !bounded(&self.workspace.id, 256)
            || !bounded(&self.workspace.name, 100)
            || self.audience != config.resource
            || self.actions != [SCOPE]
            || self.expires_at <= now
            || self.expires_at > now.saturating_add(MAX_GRANT_SECONDS)
        {
            return Err(Failure::InvalidResponse);
        }
        Ok(())
    }

    pub fn same_grant(&self, other: &Self) -> bool {
        self.user.id == other.user.id
            && self.grant_id == other.grant_id
            && self.device_id == other.device_id
            && self.workspace.id == other.workspace.id
            && self.workspace.kind == other.workspace.kind
            && self.audience == other.audience
            && self.actions == other.actions
            && self.expires_at == other.expires_at
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub id: String,
    pub login: String,
}

impl From<&Session> for Identity {
    fn from(value: &Session) -> Self {
        Self {
            id: value.user.id.clone(),
            login: value.user.display_name.clone(),
        }
    }
}

#[derive(Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub token_type: String,
}

#[derive(Clone)]
pub struct Attempt {
    pub state: String,
    pub verifier: String,
    pub callback: String,
    pub expires_at: i64,
    pub epoch: String,
    pub generation: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    pub epoch: String,
    pub issuer: String,
    pub resource: String,
    pub refresh_token: String,
    pub session: Option<Session>,
}

pub trait Vault: Send + Sync {
    fn read(&self) -> Result<Option<Credential>, Failure>;
    fn write(&self, credential: Option<&Credential>) -> Result<(), Failure>;
}

pub struct KeyringVault {
    service: String,
    account: String,
}
impl KeyringVault {
    pub fn new(service: String, directory: &std::path::Path) -> Self {
        let namespace =
            URL_SAFE_NO_PAD.encode(Sha256::digest(directory.as_os_str().as_encoded_bytes()));
        Self {
            service,
            account: format!("cloud-grant-v2-{namespace}"),
        }
    }
}
impl Vault for KeyringVault {
    fn read(&self) -> Result<Option<Credential>, Failure> {
        let entry = keyring::Entry::new(&self.service, &self.account)
            .map_err(|_| Failure::CredentialFailed)?;
        match entry.get_password() {
            Ok(value) => serde_json::from_str(&value)
                .map(Some)
                .map_err(|_| Failure::CredentialFailed),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(Failure::CredentialFailed),
        }
    }
    fn write(&self, credential: Option<&Credential>) -> Result<(), Failure> {
        let entry = keyring::Entry::new(&self.service, &self.account)
            .map_err(|_| Failure::CredentialFailed)?;
        match credential {
            Some(credential) => entry
                .set_password(
                    &serde_json::to_string(credential).map_err(|_| Failure::CredentialFailed)?,
                )
                .map_err(|_| Failure::CredentialFailed),
            None => match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err(Failure::CredentialFailed),
            },
        }
    }
}

pub trait Transport: Send + Sync {
    fn exchange(&self, config: &Config, fields: &[(&str, &str)]) -> Result<Tokens, Failure>;
    fn session(&self, config: &Config, access: &str) -> Result<Session, Failure>;
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
    if [400, 401, 403].contains(&status.as_u16()) {
        return Err(Failure::Rejected);
    }
    if status != reqwest::StatusCode::OK {
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
            || !(1..=MAX_ACCESS_SECONDS).contains(&tokens.expires_in)
        {
            return Err(Failure::InvalidResponse);
        }
        Ok(tokens)
    }
    fn session(&self, config: &Config, access: &str) -> Result<Session, Failure> {
        decode(
            client()?
                .get(config.session.clone())
                .bearer_auth(access)
                .send()
                .map_err(|_| Failure::Offline)?,
        )
    }
    fn revoke(&self, config: &Config, refresh: &str) -> Result<(), Failure> {
        let response = client()?
            .post(config.revocation.clone())
            .form(&[
                ("token", refresh),
                ("token_type_hint", "refresh_token"),
                ("client_id", CLIENT_ID),
            ])
            .send()
            .map_err(|_| Failure::Offline)?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
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
    Code(String),
    Cancelled,
}

pub fn validate_callback(
    target: &str,
    host: &str,
    config: &Config,
    attempt: &Attempt,
    now: i64,
) -> Option<Callback> {
    if target.len() > 8192
        || now >= attempt.expires_at
        || !target.starts_with("/cloud/callback?")
        || target.contains('#')
        || host
            != attempt
                .callback
                .strip_prefix("http://")?
                .strip_suffix("/cloud/callback")?
    {
        return None;
    }
    let query = target.strip_prefix("/cloud/callback?")?;
    let mut fields = std::collections::HashMap::new();
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if !["state", "code", "error", "iss"].contains(&key.as_ref())
            || fields
                .insert(key.into_owned(), value.into_owned())
                .is_some()
        {
            return None;
        }
    }
    if fields.get("state") != Some(&attempt.state)
        || fields.get("iss") != Some(&config.issuer)
        || fields.len() != 3
    {
        return None;
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
