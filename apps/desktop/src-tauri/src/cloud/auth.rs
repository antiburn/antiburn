use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Read, time::Duration};
use url::Url;

pub const DEVICE_NAME: &str = "Antiburn desktop";
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
        canonical(issuer)?;
        let resource_url = canonical(resource)?;
        Some(Self {
            issuer: issuer.to_owned(),
            resource: resource.to_owned(),
            authorization: resource_url.join("desktop/authorize").ok()?,
            token: resource_url.join("v1/desktop/token").ok()?,
            revocation: resource_url.join("v1/desktop/logout").ok()?,
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
    pub device_name: String,
    pub workspace: Workspace,
    pub resource: String,
    pub scope: String,
    pub iss: String,
    pub expires_at: i64,
    pub access_expires_at: i64,
}

impl Session {
    pub fn validate(&self, config: &Config, now: i64) -> Result<(), Failure> {
        self.validate_grant(config, now)?;
        if self.access_expires_at <= now || self.access_expires_at > self.expires_at {
            return Err(Failure::InvalidResponse);
        }
        Ok(())
    }

    pub fn validate_grant(&self, config: &Config, now: i64) -> Result<(), Failure> {
        if !bounded(&self.user.id, 256)
            || !bounded(&self.user.display_name, 256)
            || !bounded(&self.grant_id, 256)
            || !bounded(&self.device_name, 100)
            || !bounded(&self.workspace.id, 256)
            || !bounded(&self.workspace.name, 256)
            || self.workspace.kind != WorkspaceKind::Personal
            || self.resource != config.resource
            || self.scope != SCOPE
            || self.iss != config.issuer
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
            && self.workspace.id == other.workspace.id
            && self.workspace.kind == other.workspace.kind
            && self.resource == other.resource
            && self.scope == other.scope
            && self.iss == other.iss
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

fn bounded(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn deadline(value: &str) -> Result<i64, Failure> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map(|value| value.unix_timestamp())
        .map_err(|_| Failure::InvalidResponse)
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub token_type: String,
    pub grant_id: String,
    pub user_id: String,
    pub workspace_id: String,
    pub grant_expires_at: String,
    pub scope: String,
    pub resource: String,
    pub iss: String,
}

impl Tokens {
    fn validate_secrets(&self) -> Result<(), Failure> {
        if self.token_type != "Bearer"
            || !bounded(&self.access_token, 16_384)
            || !bounded(&self.refresh_token, 16_384)
            || self.access_token == self.refresh_token
            || !(1..=MAX_ACCESS_SECONDS).contains(&self.expires_in)
        {
            return Err(Failure::InvalidResponse);
        }
        Ok(())
    }

    pub fn validate(&self, config: &Config, now: i64) -> Result<(), Failure> {
        self.validate_secrets()?;
        let expires_at = deadline(&self.grant_expires_at)?;
        if !bounded(&self.grant_id, 256)
            || !bounded(&self.user_id, 256)
            || !bounded(&self.workspace_id, 256)
            || self.scope != SCOPE
            || self.resource != config.resource
            || self.iss != config.issuer
            || expires_at <= now
            || expires_at > now.saturating_add(MAX_GRANT_SECONDS)
        {
            return Err(Failure::InvalidResponse);
        }
        Ok(())
    }

    pub fn matches_session(&self, session: &Session) -> bool {
        self.grant_id == session.grant_id
            && self.user_id == session.user.id
            && self.workspace_id == session.workspace.id
            && deadline(&self.grant_expires_at).ok() == Some(session.expires_at)
            && self.scope == session.scope
            && self.resource == session.resource
            && self.iss == session.iss
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionResponse {
    user_id: String,
    display_name: Option<String>,
    workspace_id: String,
    workspace_name: String,
    grant_id: String,
    device_name: String,
    scope: String,
    resource: String,
    grant_expires_at: String,
    access_expires_at: String,
    iss: String,
}

impl TryFrom<SessionResponse> for Session {
    type Error = Failure;

    fn try_from(value: SessionResponse) -> Result<Self, Self::Error> {
        Ok(Self {
            user: User {
                id: value.user_id,
                display_name: value.display_name.unwrap_or_else(|| "Cloud account".into()),
            },
            grant_id: value.grant_id,
            device_name: value.device_name,
            workspace: Workspace {
                id: value.workspace_id,
                name: value.workspace_name,
                kind: WorkspaceKind::Personal,
            },
            resource: value.resource,
            scope: value.scope,
            iss: value.iss,
            expires_at: deadline(&value.grant_expires_at)?,
            access_expires_at: deadline(&value.access_expires_at)?,
        })
    }
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
    #[serde(default)]
    pub cleanup_refresh_tokens: Vec<String>,
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
            Ok(value) => {
                let credential: Credential =
                    serde_json::from_str(&value).map_err(|_| Failure::CredentialFailed)?;
                if credential.cleanup_refresh_tokens.len() > 1
                    || !bounded(&credential.refresh_token, 16_384)
                    || credential
                        .cleanup_refresh_tokens
                        .iter()
                        .any(|proof| !bounded(proof, 16_384))
                {
                    return Err(Failure::CredentialFailed);
                }
                Ok(Some(credential))
            }
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

#[derive(Serialize)]
#[serde(tag = "grant_type", rename_all = "snake_case")]
pub enum TokenRequest<'a> {
    AuthorizationCode {
        code: &'a str,
        code_verifier: &'a str,
    },
    RefreshToken {
        refresh_token: &'a str,
    },
}

pub trait Transport: Send + Sync {
    fn exchange(&self, config: &Config, request: &TokenRequest<'_>) -> Result<Tokens, Failure>;
    fn session(&self, config: &Config, access: &str) -> Result<Session, Failure>;
    fn revoke(&self, config: &Config, refresh: &str) -> Result<(), Failure>;
}

pub struct HttpTransport;
fn client() -> Result<reqwest::blocking::Client, Failure> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
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
    fn exchange(&self, config: &Config, request: &TokenRequest<'_>) -> Result<Tokens, Failure> {
        let tokens: Tokens = decode(
            client()?
                .post(config.token.clone())
                .json(request)
                .send()
                .map_err(|_| Failure::Offline)?,
        )?;
        tokens.validate_secrets()?;
        Ok(tokens)
    }
    fn session(&self, config: &Config, access: &str) -> Result<Session, Failure> {
        let response: SessionResponse = decode(
            client()?
                .get(config.session.clone())
                .bearer_auth(access)
                .send()
                .map_err(|_| Failure::Offline)?,
        )?;
        response.try_into()
    }
    fn revoke(&self, config: &Config, refresh: &str) -> Result<(), Failure> {
        let response = client()?
            .post(config.revocation.clone())
            .json(&serde_json::json!({ "refresh_token": refresh }))
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
