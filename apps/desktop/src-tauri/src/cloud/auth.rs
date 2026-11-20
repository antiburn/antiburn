//! First-party wire validation and the OS credential boundary for the desktop engine.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{ffi::OsStr, io::Read, time::Duration};
use url::Url;

const DEFAULT_DEVICE_NAME: &str = "Antiburn Desktop";

/// Returns display metadata for the browser consent page, not a device ID.
/// Run on a blocking worker. The lookup uses local OS settings, not DNS.
pub(super) fn device_name() -> String {
    hostname::get()
        .ok()
        .as_deref()
        .and_then(short_hostname)
        .unwrap_or(DEFAULT_DEVICE_NAME)
        .to_owned()
}

/// Shares only the first hostname segment, and never a domain suffix or IP.
fn short_hostname(hostname: &OsStr) -> Option<&str> {
    let label = hostname.to_str()?.split('.').next()?;
    let bytes = label.as_bytes();
    (bytes.len() <= 63
        && bytes.first()?.is_ascii_alphanumeric()
        && bytes.last()?.is_ascii_alphanumeric()
        && bytes.iter().any(u8::is_ascii_alphabetic)
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
        && !label.eq_ignore_ascii_case("localhost"))
    .then_some(label)
}
/// Scope for existing capture-only account grants.
pub const CAPTURE_SCOPE: &str = "capture:write";
/// Scope for account grants that can also search and read sessions.
pub const CAPTURE_AND_SESSION_SCOPE: &str = "capture:write sessions:list sessions:read";

/// Accept only the service's exact scope strings. Do not normalize or split them.
fn supported_scope(scope: &str) -> bool {
    matches!(scope, CAPTURE_SCOPE | CAPTURE_AND_SESSION_SCOPE)
}

/// Maximum opaque access lifetime in seconds; refresh does not extend the grant deadline.
pub const MAX_ACCESS_SECONDS: u64 = 900;
/// Maximum initial fixed grant lifetime in seconds.
pub const MAX_GRANT_SECONDS: i64 = 365 * 24 * 60 * 60;

/// Pin canonical service origins. Builds with debug assertions admit literal loopback HTTP.
/// Resource owns request routes; issuer binds callbacks and responses.
/// Neither value can contain a path or trailing slash.
#[derive(Clone)]
pub struct Config {
    pub issuer: String,
    pub resource: String,
    pub authorization: Url,
    pub token: Url,
    pub session: Url,
    pub workspaces: Url,
    pub revocation: Url,
}

impl Config {
    /// Use local defaults with debug assertions only when both origins are absent.
    /// Partial or invalid overrides disable authentication. Other builds need explicit HTTPS.
    pub fn from_build() -> Option<Self> {
        Self::from_origins(
            option_env!("ANTIBURN_CLOUD_ISSUER"),
            option_env!("ANTIBURN_CLOUD_RESOURCE"),
        )
    }

    /// Keep explicit origins as one complete pair; never fill a missing override.
    fn from_origins(issuer: Option<&str>, resource: Option<&str>) -> Option<Self> {
        match (issuer, resource) {
            (None, None) if cfg!(debug_assertions) => {
                Self::parse("http://localhost:3000", "http://localhost:3000")
            }
            (Some(issuer), Some(resource)) => Self::parse(issuer, resource),
            _ => None,
        }
    }

    /// Reject aliases, credentials, paths, and remote HTTP rather than normalizing authority.
    pub fn parse(issuer: &str, resource: &str) -> Option<Self> {
        /// Require the exact public origin string used by callback and response proofs.
        fn canonical(value: &str) -> Option<Url> {
            let url = Url::parse(value).ok()?;
            ((url.scheme() == "https" || debug_loopback_http(&url))
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/"
                && url.origin().ascii_serialization() == value)
                .then_some(url)
        }
        canonical(issuer)?;
        let resource_url = canonical(resource)?;
        Some(Self {
            issuer: issuer.to_owned(),
            resource: resource.to_owned(),
            authorization: resource_url.join("devices/authorize/account").ok()?,
            token: resource_url.join("v1/devices/token").ok()?,
            revocation: resource_url.join("v1/devices/logout").ok()?,
            session: resource_url.join("v1/devices/session").ok()?,
            workspaces: resource_url.join("v1/captures/destinations").ok()?,
        })
    }
}

/// Admit plaintext only for exact loopback hosts when debug assertions are enabled.
fn debug_loopback_http(url: &Url) -> bool {
    cfg!(debug_assertions)
        && url.scheme() == "http"
        && match url.host() {
            Some(url::Host::Domain("localhost")) => true,
            Some(url::Host::Ipv4(address)) => address == std::net::Ipv4Addr::LOCALHOST,
            Some(url::Host::Ipv6(address)) => address == std::net::Ipv6Addr::LOCALHOST,
            _ => false,
        }
}

/// Closed errors safe for renderer state. Never attach tokens or raw provider errors.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    Offline,
    /// Token exchange failed during connection setup before sending HTTP proof.
    /// Timeouts and failures after dispatch cannot use this retry-safe outcome.
    OfflineBeforeSend,
    Rejected,
    InvalidResponse,
    CredentialFailed,
    Cancelled,
    Unavailable,
    ReauthRequired,
}

/// Internal account binding plus display metadata; the display name never binds a grant.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct User {
    pub id: String,
    pub display_name: String,
}

/// The kind of a verified workspace or an available destination in its list.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    Personal,
    Team,
}

/// Destination and display metadata. This is not a grant or upload fallback.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub kind: WorkspaceKind,
}

impl Workspace {
    /// Validate display data before it reaches grant selection or the renderer.
    pub(super) fn valid(&self) -> bool {
        bounded(&self.id, 256) && bounded(&self.name, 256)
    }
}

/// One destination in the server's current eligible list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EligibleWorkspace {
    pub workspace: Workspace,
    /// Opaque membership fence for Team; Personal has no membership generation.
    pub membership_generation: Option<String>,
}

/// Account session plus the local Personal anchor used by older desktop adapters.
/// The anchor comes from the eligible list, not from the token or session wire.
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
    /// Require live access inside the validated fixed grant deadline.
    pub fn validate(&self, config: &Config, now: i64) -> Result<(), Failure> {
        self.validate_grant(config, now)?;
        if self.access_expires_at <= now || self.access_expires_at > self.expires_at {
            return Err(Failure::InvalidResponse);
        }
        Ok(())
    }

    /// Validate the saved account binding and its local Personal anchor.
    pub fn validate_grant(&self, config: &Config, now: i64) -> Result<(), Failure> {
        if !bounded(&self.user.id, 256)
            || !bounded(&self.user.display_name, 256)
            || !bounded(&self.grant_id, 256)
            || !bounded(&self.device_name, 100)
            || self.workspace.kind != WorkspaceKind::Personal
            || !self.workspace.valid()
            || self.resource != config.resource
            || !supported_scope(&self.scope)
            || self.iss != config.issuer
            || self.expires_at <= now
            || self.expires_at > now.saturating_add(MAX_GRANT_SECONDS)
        {
            return Err(Failure::InvalidResponse);
        }
        Ok(())
    }

    /// Pin account grant authority and the local Personal anchor across rotation.
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

/// Nonsecret account display data permitted to cross the renderer IPC boundary.
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

/// Reject empty, oversized, or control-bearing wire strings without logging their values.
fn bounded(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

/// Parse the wire deadline to UTC seconds; malformed input cannot restore access.
fn deadline(value: &str) -> Result<i64, Failure> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map(|value| value.unix_timestamp())
        .map_err(|_| Failure::InvalidResponse)
}

/// Untrusted token response. Decoding is not acceptance; retain bounded refresh
/// proof for cleanup even when another field fails authority validation.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub token_type: String,
    pub grant_id: String,
    pub user_id: String,
    // Decode old responses only so their bounded refresh proof can be revoked.
    #[serde(rename = "workspace_id", default, deserialize_with = "legacy_claim")]
    pub(super) legacy_workspace_claim: bool,
    pub grant_expires_at: String,
    pub scope: String,
    pub resource: String,
    pub iss: String,
}

fn legacy_claim<'de, D: serde::Deserializer<'de>>(value: D) -> Result<bool, D::Error> {
    serde::de::IgnoredAny::deserialize(value).map(|_| true)
}

impl Tokens {
    /// Validate secret bounds and access lifetime before sending any access bearer.
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

    /// Reject authority outside the pinned first-party resource, issuer, scope, or deadline.
    pub fn validate(&self, config: &Config, now: i64) -> Result<(), Failure> {
        self.validate_secrets()?;
        let expires_at = deadline(&self.grant_expires_at)?;
        if !bounded(&self.grant_id, 256)
            || !bounded(&self.user_id, 256)
            || self.legacy_workspace_claim
            || !supported_scope(&self.scope)
            || self.resource != config.resource
            || self.iss != config.issuer
            || expires_at <= now
            || expires_at > now.saturating_add(MAX_GRANT_SECONDS)
        {
            return Err(Failure::InvalidResponse);
        }
        Ok(())
    }

    /// Require the independently verified session to confirm every token grant binding.
    pub fn matches_session(&self, session: &AccountSession) -> bool {
        self.grant_id == session.grant_id
            && self.user_id == session.user_id
            && deadline(&self.grant_expires_at)
                .is_ok_and(|expires_at| deadline(&session.grant_expires_at) == Ok(expires_at))
            && self.scope == session.scope
            && self.resource == session.resource
            && self.iss == session.iss
    }
}

/// Exact account session wire shape. It contains no destination claims.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountSession {
    pub user_id: String,
    pub display_name: Option<String>,
    pub grant_id: String,
    pub device_name: String,
    pub scope: String,
    pub resource: String,
    pub grant_expires_at: String,
    pub access_expires_at: String,
    pub iss: String,
}

impl AccountSession {
    /// Attach the verified Personal adapter destination. The caller validates the result.
    pub(super) fn with_personal(self, workspace: Workspace) -> Result<Session, Failure> {
        Ok(Session {
            user: User {
                id: self.user_id,
                display_name: self.display_name.unwrap_or_else(|| "Cloud account".into()),
            },
            grant_id: self.grant_id,
            device_name: self.device_name,
            workspace,
            resource: self.resource,
            scope: self.scope,
            iss: self.iss,
            expires_at: deadline(&self.grant_expires_at)?,
            access_expires_at: deadline(&self.access_expires_at)?,
        })
    }
}

#[cfg(test)]
impl From<Session> for AccountSession {
    fn from(session: Session) -> Self {
        fn format(seconds: i64) -> String {
            time::OffsetDateTime::from_unix_timestamp(seconds)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()
        }
        Self {
            user_id: session.user.id,
            display_name: Some(session.user.display_name),
            grant_id: session.grant_id,
            device_name: session.device_name,
            scope: session.scope,
            resource: session.resource,
            grant_expires_at: format(session.expires_at),
            access_expires_at: format(session.access_expires_at),
            iss: session.iss,
        }
    }
}

/// Ephemeral browser proof. Never persist its verifier or restore it after restart.
#[derive(Clone)]
pub struct Attempt {
    pub state: String,
    pub verifier: String,
    pub callback: String,
    pub expires_at: i64,
    pub epoch: String,
    pub generation: u64,
}

/// OS-only refresh record. `session: None` permits logout only, never authentication;
/// cleanup can retain one extra returned proof without replacing the original.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    pub epoch: String,
    pub issuer: String,
    pub resource: String,
    pub refresh_token: String,
    #[serde(default)]
    pub cleanup_refresh_tokens: Vec<String>,
    /// Old keyring records omit this bit. Their proofs remain logout-only.
    #[serde(default)]
    pub account_grant: bool,
    pub session: Option<Session>,
}

/// Store one engine's rotating refresh credential and logout-only proofs.
///
/// The controller serializes calls and holds the state-directory owner lock.
/// Implementors must use secure storage, replace the whole record on write, and
/// return errors without secrets. A read failure is not an absent credential.
/// A successful write must be durable before it returns. Failed writes must not
/// claim success; the engine closes access and keeps revocation uncertainty.
/// These synchronous calls require the Cloud blocking-work permit.
pub trait Vault: Send + Sync {
    /// Return absence only for a missing entry; malformed or inaccessible storage is an error.
    fn read(&self) -> Result<Option<Credential>, Failure>;
    /// Delete only when given `None`; deleting an absent credential succeeds.
    /// The engine records confirmed revocation before requesting deletion.
    fn write(&self, credential: Option<&Credential>) -> Result<(), Failure>;
}

/// OS credential storage bound to the app identifier and state-directory namespace.
/// Synchronous keyring calls require admitted blocking work and have no plaintext fallback.
pub struct KeyringVault {
    service: String,
    account: String,
}
impl KeyringVault {
    /// Derive the namespace in memory without opening the keyring during UI setup.
    pub fn new(service: String, directory: &std::path::Path) -> Self {
        let namespace =
            URL_SAFE_NO_PAD.encode(Sha256::digest(directory.as_os_str().as_encoded_bytes()));
        Self {
            service,
            account: format!("cloud-grant-v2-{namespace}"),
        }
    }
}

/// Derive an isolated slot from exact authority IDs, never from display labels.
pub(super) fn workspace_namespace(config: &Config, account: &str, workspace: &str) -> String {
    let binding = serde_json::to_vec(&(&config.issuer, &config.resource, account, workspace))
        .expect("string tuple serialization");
    URL_SAFE_NO_PAD.encode(Sha256::digest(binding))
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

/// Exact first-party JSON proof variants, without cookies or access bearer proof.
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

/// Send the desktop engine's first-party requests without retries or redirects.
///
/// Implementors must bound responses and keep cookies, Origin, and raw errors
/// out of this boundary. Only reads of session and workspace state send access bearer proof. The controller
/// serializes credential operations; transport success does not grant access.
/// Implementors can block only inside admitted Cloud blocking work.
pub trait Transport: Send + Sync {
    /// Return decoded tokens with a bounded refresh proof, even when other fields
    /// are invalid. The engine validates authority and saves rejected proof for
    /// logout. Return `OfflineBeforeSend` only when no HTTP proof was sent.
    /// An uncertain exchange must not retry the predecessor.
    fn exchange(&self, config: &Config, request: &TokenRequest<'_>) -> Result<Tokens, Failure>;
    /// Read current server authority; the engine checks all returned bindings.
    fn session(&self, config: &Config, access: &str) -> Result<AccountSession, Failure>;
    /// Return the current eligible list. A missing route or invalid list is an error.
    fn workspaces(&self, config: &Config, access: &str) -> Result<Vec<EligibleWorkspace>, Failure>;
    /// Return success only for synchronous HTTP 204 confirming this proof's grant.
    fn revoke(&self, config: &Config, refresh: &str) -> Result<(), Failure>;
}

/// Short blocking HTTP transport, used only inside the bounded Cloud worker.
/// The engine's synchronous proof/write ordering stays intact; this is not async HTTP.
pub struct HttpTransport;
/// Check each request URL before attaching proof. Debug loopback HTTP bypasses
/// proxies so plaintext credentials stay on the local connection.
pub(super) fn client(url: &Url) -> Result<reqwest::blocking::Client, Failure> {
    let local_http = debug_loopback_http(url);
    if (url.scheme() != "https" && !local_http)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Failure::Unavailable);
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut builder = reqwest::blocking::Client::builder()
        .https_only(!local_http)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(15));
    let loopback = match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    };
    if loopback {
        builder = builder.no_proxy();
    }
    builder.build().map_err(|_| Failure::Offline)
}

/// Bound successful JSON and map status/read errors to closed safe failures.
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

/// Permit a later exchange only when connection setup failed before HTTP dispatch.
/// Keep timeouts uncertain even when the connector reports them.
/// The transport must keep retries and redirects disabled.
fn exchange_failure(error: reqwest::Error) -> Failure {
    if error.is_connect() && !error.is_timeout() {
        Failure::OfflineBeforeSend
    } else {
        Failure::Offline
    }
}

impl Transport for HttpTransport {
    fn exchange(&self, config: &Config, request: &TokenRequest<'_>) -> Result<Tokens, Failure> {
        let tokens: Tokens = decode(
            client(&config.token)?
                .post(config.token.clone())
                .json(request)
                .send()
                .map_err(exchange_failure)?,
        )?;
        // Keep usable logout proof until the engine decides whether to accept it.
        if !bounded(&tokens.refresh_token, 16_384) {
            return Err(Failure::InvalidResponse);
        }
        Ok(tokens)
    }
    fn session(&self, config: &Config, access: &str) -> Result<AccountSession, Failure> {
        let session: AccountSession = decode(
            client(&config.session)?
                .get(config.session.clone())
                .bearer_auth(access)
                .send()
                .map_err(|_| Failure::Offline)?,
        )?;
        deadline(&session.grant_expires_at)?;
        deadline(&session.access_expires_at)?;
        Ok(session)
    }
    fn workspaces(&self, config: &Config, access: &str) -> Result<Vec<EligibleWorkspace>, Failure> {
        let response = client(&config.workspaces)?
            .get(config.workspaces.clone())
            .bearer_auth(access)
            .send()
            .map_err(|_| Failure::Offline)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Failure::Unavailable);
        }
        decode::<WorkspaceResponse>(response)?.try_into()
    }
    fn revoke(&self, config: &Config, refresh: &str) -> Result<(), Failure> {
        let response = client(&config.revocation)?
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceResponse {
    workspaces: Vec<WorkspaceEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceEntry {
    workspace_id: String,
    name: String,
    #[serde(rename = "type")]
    kind: WorkspaceKind,
    // This field is required, including when its value is null.
    #[serde(deserialize_with = "Option::deserialize")]
    membership_generation: Option<String>,
}

impl TryFrom<WorkspaceResponse> for Vec<EligibleWorkspace> {
    type Error = Failure;

    fn try_from(response: WorkspaceResponse) -> Result<Self, Failure> {
        if response.workspaces.len() > 100 {
            return Err(Failure::InvalidResponse);
        }
        let mut seen = std::collections::BTreeSet::new();
        response
            .workspaces
            .into_iter()
            .map(|entry| {
                let workspace = Workspace {
                    id: entry.workspace_id,
                    name: entry.name,
                    kind: entry.kind,
                };
                if !workspace.valid()
                    || !workspace.id.bytes().enumerate().all(|(index, byte)| {
                        if matches!(index, 8 | 13 | 18 | 23) {
                            byte == b'-'
                        } else {
                            byte.is_ascii_hexdigit()
                        }
                    })
                    || workspace.id.len() != 36
                    || !seen.insert(workspace.id.to_ascii_lowercase())
                    || !match (&workspace.kind, &entry.membership_generation) {
                        (WorkspaceKind::Personal, None) => true,
                        (WorkspaceKind::Team, Some(value)) => bounded(value, 256),
                        _ => false,
                    }
                {
                    return Err(Failure::InvalidResponse);
                }
                Ok(EligibleWorkspace {
                    workspace,
                    membership_generation: entry.membership_generation,
                })
            })
            .collect()
    }
}

/// Generate independent 32-byte proof or epoch material with OS randomness.
pub fn random() -> Result<String, Failure> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| Failure::Unavailable)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Derive the S256 PKCE challenge without exposing the verifier to the browser.
pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// A guarded browser result, still subject to pending epoch and generation checks.
pub enum Callback {
    Code(String),
    Cancelled,
}

/// Admit only the exact callback route, Host, state, issuer, and one result.
/// Invalid requests cannot consume an attempt or disclose the expected proof.
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

#[cfg(test)]
mod tests;
