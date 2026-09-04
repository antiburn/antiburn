//! Ask Google for Antigravity subscription usage with provider-owned tokens.
//!
//! The source reads the current `agy` credential from the macOS, Linux, or
//! Windows `gemini` keyring, or its legacy file. It can also read the
//! Antigravity IDE unified OAuth value from `state.vscdb`. Every carrier is
//! read-only and bounded. The source can use a refresh token, but it keeps
//! refreshed access tokens in memory and never changes the provider's store.
//!
//! The cloud flow first calls `loadCodeAssist` and requires its managed project.
//! It then calls the project-scoped `retrieveUserQuotaSummary`. Requiring the
//! project prevents an unscoped availability response from appearing as a real
//! empty or full quota reading.
//!
//! When cloud credentials are absent or cloud retrieval fails, the source asks
//! a running `agy` or Antigravity IDE language server. It reads the shared
//! quota summary and user status from one loopback endpoint candidate.
//! [`super::antigravity_local`] owns the strict process, listener, TLS, timeout,
//! and response bounds. Its client cannot receive a Google OAuth token.

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine as _;
use rusqlite::OpenFlags;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::provider_usage::live::antigravity;
use crate::provider_usage::live::model::{
    Confidence, Freshness, ProviderUsageError, ProviderUsageSnapshot, UsageSource,
};
use crate::provider_usage::live::{LiveUsageSource, SourceOutcome};

use super::antigravity_local::{LocalProbe, LocalUsageTransport};
use super::cooldown::Cooldown;
use super::http;

const SOURCE_ID: &str = super::ANTIGRAVITY_SOURCE_ID;
const MAX_CREDENTIAL_BYTES: u64 = 256 * 1024;
const MAX_STATE_DB_BYTES: u64 = 64 * 1024 * 1024;
const LOAD_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const USERINFO_ENDPOINT: &str = "https://openidconnect.googleapis.com/v1/userinfo";
const QUOTA_ENDPOINT: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
const MODELS_ENDPOINT: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const CLIENT_ID: Option<&str> = option_env!("GOOGLE_ANTIGRAVITY_2_IDE_AGY_OAUTH_CLIENT_ID");
const CLIENT_SECRET: Option<&str> = option_env!("GOOGLE_ANTIGRAVITY_2_IDE_AGY_OAUTH_CLIENT_SECRET");

#[derive(Clone)]
struct Credentials {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<OffsetDateTime>,
}

impl Credentials {
    fn current_at(&self, now: OffsetDateTime) -> bool {
        self.expires_at.is_none_or(|expiry| expiry > now)
    }
}

struct CachedRefresh {
    refresh_token: String,
    credentials: Credentials,
}

type RefreshCache = Mutex<Option<CachedRefresh>>;

pub struct AntigravityDirectFetch {
    agy_path: Option<PathBuf>,
    ide_paths: Vec<PathBuf>,
    transport: Box<dyn AntigravityTransport>,
    local: Box<dyn LocalUsageTransport>,
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    try_keychain: bool,
    refreshed: RefreshCache,
    cooldown: Cooldown,
}

impl AntigravityDirectFetch {
    pub fn new() -> Self {
        Self {
            agy_path: default_agy_path(),
            ide_paths: default_ide_paths(),
            transport: Box::new(LiveTransport),
            local: Box::new(LocalProbe),
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            try_keychain: true,
            refreshed: Mutex::default(),
            cooldown: Cooldown::new(),
        }
    }

    fn credentials(&self, now: OffsetDateTime) -> Option<Credentials> {
        #[cfg(target_os = "macos")]
        if self.try_keychain
            && let Some(credentials) = read_macos_keychain()
                .as_deref()
                .and_then(parse_agy_secret)
                .filter(|credentials| credential_can_run(credentials, now))
        {
            return Some(credentials);
        }
        #[cfg(target_os = "linux")]
        if self.try_keychain
            && let Some(credentials) = read_linux_secret_service()
                .as_deref()
                .and_then(parse_agy_secret)
                .filter(|credentials| credential_can_run(credentials, now))
        {
            return Some(credentials);
        }
        #[cfg(target_os = "windows")]
        if self.try_keychain
            && let Some(credentials) = read_windows_credential_manager()
                .as_deref()
                .and_then(parse_agy_secret)
                .filter(|credentials| credential_can_run(credentials, now))
        {
            return Some(credentials);
        }
        if let Some(credentials) = self
            .agy_path
            .as_deref()
            .and_then(read_bounded)
            .as_deref()
            .and_then(parse_agy_secret)
            .filter(|credentials| credential_can_run(credentials, now))
        {
            return Some(credentials);
        }
        self.ide_paths
            .iter()
            .filter_map(|path| read_ide_credentials(path))
            .find(|credentials| credential_can_run(credentials, now))
    }

    #[cfg(test)]
    fn with_transports(
        agy_path: PathBuf,
        ide_paths: Vec<PathBuf>,
        transport: Box<dyn AntigravityTransport>,
        local: Box<dyn LocalUsageTransport>,
    ) -> Self {
        Self {
            agy_path: Some(agy_path),
            ide_paths,
            transport,
            local,
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            try_keychain: false,
            refreshed: Mutex::default(),
            cooldown: Cooldown::new(),
        }
    }
}

impl Default for AntigravityDirectFetch {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveUsageSource for AntigravityDirectFetch {
    fn id(&self) -> &'static str {
        SOURCE_ID
    }

    fn provider(&self) -> &'static str {
        crate::provider_usage::providers::GOOGLE
    }

    fn requires_online_opt_in(&self) -> bool {
        true
    }

    fn fetch(&self, max_age: std::time::Duration) -> SourceOutcome {
        let now = OffsetDateTime::now_utc();
        self.cooldown.poll(now, max_age, || {
            Ok(fetch_with_refresh_fallback(
                self.transport.as_ref(),
                self.local.as_ref(),
                self.credentials(now).as_ref(),
                &self.refreshed,
                now,
            )?)
        })
    }
}

fn credential_can_run(credentials: &Credentials, now: OffsetDateTime) -> bool {
    credentials.current_at(now) || credentials.refresh_token.is_some()
}

fn default_agy_path() -> Option<PathBuf> {
    let root = antiburn_local::paths::non_empty_env_path("GEMINI_CLI_HOME")
        .or_else(|| antiburn_local::paths::home_dir().map(|home| home.join(".gemini")))?;
    Some(root.join("antigravity-cli/antigravity-oauth-token"))
}

#[cfg(target_os = "macos")]
fn default_ide_paths() -> Vec<PathBuf> {
    let Some(home) = antiburn_local::paths::home_dir() else {
        return Vec::new();
    };
    ["Antigravity IDE", "Antigravity"]
        .map(|name| {
            home.join("Library/Application Support")
                .join(name)
                .join("User/globalStorage/state.vscdb")
        })
        .into()
}

#[cfg(target_os = "linux")]
fn default_ide_paths() -> Vec<PathBuf> {
    let Some(home) = antiburn_local::paths::home_dir() else {
        return Vec::new();
    };
    ["Antigravity IDE", "Antigravity"]
        .map(|name| {
            home.join(".config")
                .join(name)
                .join("User/globalStorage/state.vscdb")
        })
        .into()
}

#[cfg(target_os = "windows")]
fn default_ide_paths() -> Vec<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|root| {
            ["Antigravity IDE", "Antigravity"]
                .map(|name| root.join(name).join("User/globalStorage/state.vscdb"))
                .into()
        })
        .unwrap_or_default()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn default_ide_paths() -> Vec<PathBuf> {
    Vec::new()
}

fn read_bounded(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_CREDENTIAL_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_CREDENTIAL_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_CREDENTIAL_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn parse_agy_secret(input: &str) -> Option<Credentials> {
    let input = input.trim();
    let decoded;
    let input = if let Some(encoded) = input.strip_prefix("go-keyring-base64:") {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .ok()?;
        if bytes.len() as u64 > MAX_CREDENTIAL_BYTES {
            return None;
        }
        decoded = String::from_utf8(bytes).ok()?;
        decoded.trim()
    } else {
        input
    };
    let value: Value = serde_json::from_str(input).ok()?;
    let token = value.get("token").unwrap_or(&value);
    let access_token = token
        .get("access_token")
        .or_else(|| token.get("accessToken"))?
        .as_str()?
        .trim();
    if access_token.is_empty() {
        return None;
    }
    let expires_at = match token.get("expiry").or_else(|| token.get("expiry_date")) {
        Some(value) => Some(parse_expiry(value)?),
        None => None,
    };
    let refresh_token = token
        .get("refresh_token")
        .or_else(|| token.get("refreshToken"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Some(Credentials {
        access_token: access_token.to_owned(),
        refresh_token,
        expires_at,
    })
}

fn parse_expiry(value: &Value) -> Option<OffsetDateTime> {
    if let Some(text) = value.as_str() {
        return OffsetDateTime::parse(text, &Rfc3339).ok();
    }
    let raw = value.as_i64()?;
    let seconds = if raw > 10_000_000_000 {
        raw / 1_000
    } else {
        raw
    };
    OffsetDateTime::from_unix_timestamp(seconds).ok()
}

fn read_ide_credentials(path: &Path) -> Option<Credentials> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_STATE_DB_BYTES {
        return None;
    }
    let connection = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let value: String = connection
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1 AND length(value) <= ?2",
            rusqlite::params![
                "antigravityUnifiedStateSync.oauthToken",
                MAX_CREDENTIAL_BYTES as i64
            ],
            |row| row.get(0),
        )
        .ok()?;
    parse_unified_oauth(&value)
}

fn parse_unified_oauth(value: &str) -> Option<Credentials> {
    let outer = decode_bounded_base64(value)?;
    let wrapper = protobuf_bytes(&outer, 1)?;
    if protobuf_bytes(wrapper, 1)? != b"oauthTokenInfoSentinelKey" {
        return None;
    }
    let payload = protobuf_bytes(wrapper, 2)?;
    let encoded = std::str::from_utf8(protobuf_bytes(payload, 1)?).ok()?;
    let oauth = decode_bounded_base64(encoded)?;
    let access_token = std::str::from_utf8(protobuf_bytes(&oauth, 1)?).ok()?.trim();
    if access_token.is_empty() {
        return None;
    }
    let expires_at = match protobuf_bytes(&oauth, 4) {
        Some(timestamp) => Some(
            protobuf_varint(timestamp, 1)
                .and_then(|seconds| i64::try_from(seconds).ok())
                .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())?,
        ),
        None => None,
    };
    let refresh_token = protobuf_bytes(&oauth, 3)
        .and_then(|value| std::str::from_utf8(value).ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Some(Credentials {
        access_token: access_token.to_owned(),
        refresh_token,
        expires_at,
    })
}

fn decode_bounded_base64(value: &str) -> Option<Vec<u8>> {
    if value.len() as u64 > MAX_CREDENTIAL_BYTES {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .ok()?;
    (decoded.len() as u64 <= MAX_CREDENTIAL_BYTES).then_some(decoded)
}

fn protobuf_bytes(input: &[u8], wanted: u64) -> Option<&[u8]> {
    let mut offset = 0;
    while offset < input.len() {
        let tag = read_varint(input, &mut offset)?;
        let field = tag >> 3;
        match tag & 7 {
            0 => {
                let value_start = offset;
                read_varint(input, &mut offset)?;
                if field == wanted {
                    return Some(&input[value_start..offset]);
                }
            }
            1 => offset = offset.checked_add(8)?,
            2 => {
                let length = usize::try_from(read_varint(input, &mut offset)?).ok()?;
                let end = offset.checked_add(length)?;
                let value = input.get(offset..end)?;
                offset = end;
                if field == wanted {
                    return Some(value);
                }
            }
            5 => offset = offset.checked_add(4)?,
            _ => return None,
        }
        if offset > input.len() {
            return None;
        }
    }
    None
}

fn protobuf_varint(input: &[u8], wanted: u64) -> Option<u64> {
    let bytes = protobuf_bytes(input, wanted)?;
    let mut offset = 0;
    read_varint(bytes, &mut offset)
}

fn read_varint(input: &[u8], offset: &mut usize) -> Option<u64> {
    let mut value = 0_u64;
    for shift in (0..70).step_by(7) {
        let byte = *input.get(*offset)?;
        *offset += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn read_macos_keychain() -> Option<String> {
    read_secret_command(
        "security",
        &[
            "find-generic-password",
            "-s",
            "gemini",
            "-a",
            "antigravity",
            "-w",
        ],
    )
}

#[cfg(target_os = "linux")]
fn read_linux_secret_service() -> Option<String> {
    read_secret_command(
        "secret-tool",
        &["lookup", "service", "gemini", "username", "antigravity"],
    )
}

#[cfg(target_os = "windows")]
fn read_windows_credential_manager() -> Option<String> {
    use std::ptr;
    use windows_sys::Win32::Security::Credentials::{
        CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW,
    };

    let target: Vec<u16> = "gemini:antigravity"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut credential: *mut CREDENTIALW = ptr::null_mut();
    // SAFETY: `target` is null-terminated and `credential` receives an API-owned pointer.
    if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0
        || credential.is_null()
    {
        return None;
    }
    // SAFETY: CredReadW returns a valid CREDENTIALW until CredFree releases it.
    let bytes = unsafe {
        let credential_ref = &*credential;
        let length = usize::try_from(credential_ref.CredentialBlobSize).ok();
        length.and_then(|length| {
            (length as u64 <= MAX_CREDENTIAL_BYTES && !credential_ref.CredentialBlob.is_null())
                .then(|| std::slice::from_raw_parts(credential_ref.CredentialBlob, length).to_vec())
        })
    };
    // SAFETY: CredReadW allocated `credential`, and it is released exactly once here.
    unsafe { CredFree(credential.cast()) };
    String::from_utf8(bytes?).ok()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn read_secret_command(program: &str, args: &[&str]) -> Option<String> {
    use std::io::Read as _;
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    let mut child = antiburn_local::platform::process::headless_std_command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout
            .take(MAX_CREDENTIAL_BYTES + 1)
            .read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let bytes = match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(bytes) => bytes,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let status = child.wait().ok()?;
    if !status.success() || bytes.is_empty() || bytes.len() as u64 > MAX_CREDENTIAL_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

struct HttpReply {
    status: reqwest::StatusCode,
    body: String,
}

trait AntigravityTransport: Send + Sync {
    fn load(&self, access_token: &str) -> Result<HttpReply, ProviderUsageError>;
    fn quota(&self, access_token: &str, project: &str) -> Result<HttpReply, ProviderUsageError>;
    fn models(&self, _access_token: &str, _project: &str) -> Result<HttpReply, ProviderUsageError> {
        Err(ProviderUsageError::Unavailable)
    }
    fn subject(&self, _access_token: &str) -> Result<HttpReply, ProviderUsageError> {
        Err(ProviderUsageError::Unavailable)
    }
    fn refresh(
        &self,
        _refresh_token: &str,
        _now: OffsetDateTime,
    ) -> Result<Credentials, ProviderUsageError> {
        Err(ProviderUsageError::Unavailable)
    }
}

struct LiveTransport;

impl LiveTransport {
    fn post(
        &self,
        endpoint: &str,
        access_token: &str,
        body: &Value,
    ) -> Result<HttpReply, ProviderUsageError> {
        let response = http::client()
            .post(endpoint)
            .bearer_auth(access_token)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, "antigravity")
            .json(body)
            .send()
            .map_err(|_| ProviderUsageError::Unavailable)?;
        let status = response.status();
        let body = http::read_capped_body(response)?;
        Ok(HttpReply { status, body })
    }
}

impl AntigravityTransport for LiveTransport {
    fn load(&self, access_token: &str) -> Result<HttpReply, ProviderUsageError> {
        self.post(
            LOAD_ENDPOINT,
            access_token,
            &json!({
                "metadata": {
                    "ideType": "ANTIGRAVITY",
                    "platform": "PLATFORM_UNSPECIFIED",
                    "pluginType": "GEMINI"
                }
            }),
        )
    }

    fn quota(&self, access_token: &str, project: &str) -> Result<HttpReply, ProviderUsageError> {
        self.post(QUOTA_ENDPOINT, access_token, &json!({ "project": project }))
    }

    fn models(&self, access_token: &str, project: &str) -> Result<HttpReply, ProviderUsageError> {
        self.post(
            MODELS_ENDPOINT,
            access_token,
            &json!({ "project": project }),
        )
    }

    fn subject(&self, access_token: &str) -> Result<HttpReply, ProviderUsageError> {
        let response = http::client()
            .get(USERINFO_ENDPOINT)
            .bearer_auth(access_token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .map_err(|_| ProviderUsageError::Unavailable)?;
        let status = response.status();
        let body = http::read_capped_body(response)?;
        Ok(HttpReply { status, body })
    }

    fn refresh(
        &self,
        refresh_token: &str,
        now: OffsetDateTime,
    ) -> Result<Credentials, ProviderUsageError> {
        let client_id = CLIENT_ID
            .filter(|value| !value.trim().is_empty())
            .ok_or(ProviderUsageError::Authentication)?;
        let client_secret = CLIENT_SECRET
            .filter(|value| !value.trim().is_empty())
            .ok_or(ProviderUsageError::Authentication)?;
        let response = http::client()
            .post(TOKEN_ENDPOINT)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", client_id),
                ("client_secret", client_secret),
            ])
            .send()
            .map_err(|_| ProviderUsageError::Unavailable)?;
        check_status(response.status())?;
        let body = http::read_capped_body(response)?;
        let value: Value = serde_json::from_str(&body).map_err(|_| {
            ProviderUsageError::Schema(
                crate::provider_usage::live::model::SchemaReason::InvalidValue,
            )
        })?;
        let access_token = value
            .get("access_token")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(ProviderUsageError::Schema(
                crate::provider_usage::live::model::SchemaReason::MissingRequiredField,
            ))?;
        let expires_in = value
            .get("expires_in")
            .and_then(Value::as_i64)
            .filter(|seconds| (1..=86_400).contains(seconds))
            .ok_or(ProviderUsageError::Schema(
                crate::provider_usage::live::model::SchemaReason::InvalidValue,
            ))?;
        Ok(Credentials {
            access_token: access_token.to_owned(),
            refresh_token: Some(refresh_token.to_owned()),
            expires_at: Some(now + time::Duration::seconds(expires_in)),
        })
    }
}

fn fetch_cloud(
    transport: &dyn AntigravityTransport,
    credentials: &Credentials,
    now: OffsetDateTime,
) -> Result<ProviderUsageSnapshot, ProviderUsageError> {
    if credentials.expires_at.is_some_and(|expiry| expiry <= now) {
        return Err(ProviderUsageError::Authentication);
    }
    let load = transport.load(&credentials.access_token)?;
    check_status(load.status)?;
    let account = antigravity::parse_load_code_assist(&load.body)?;
    let (summary, quota_error) = match transport.quota(&credentials.access_token, &account.project)
    {
        Ok(quota) => match check_status(quota.status) {
            Ok(()) => (Some(antigravity::parse_quota_summary(&quota.body)?), None),
            Err(error) => (None, Some(error)),
        },
        Err(error) => (None, Some(error)),
    };
    let needs_models = summary
        .as_ref()
        .is_none_or(|summary| summary.windows.len() < 4);
    let windows = if needs_models {
        match transport.models(&credentials.access_token, &account.project) {
            Ok(models) if check_status(models.status).is_ok() => {
                let models = antigravity::parse_available_models(&models.body)?;
                antigravity::merge_windows(
                    summary.map_or_else(Vec::new, |summary| summary.windows),
                    models.windows,
                )
            }
            Ok(models) => match summary {
                Some(summary) => summary.windows,
                None => {
                    let model_error =
                        check_status(models.status).expect_err("status is not successful");
                    return Err(quota_error.map_or(model_error, |quota_error| {
                        preferred_error(quota_error, model_error)
                    }));
                }
            },
            Err(error) => match summary {
                Some(summary) => summary.windows,
                None => {
                    return Err(quota_error
                        .map_or(error, |quota_error| preferred_error(quota_error, error)));
                }
            },
        }
    } else {
        summary.expect("a complete summary exists").windows
    };
    Ok(ProviderUsageSnapshot {
        provider: crate::provider_usage::providers::GOOGLE,
        account: google_subject(transport, &credentials.access_token),
        account_uuid: None,
        account_email: None,
        plan: account.plan,
        plan_tier: account.tier,
        observed_at: now,
        source: UsageSource {
            id: SOURCE_ID,
            label: "Asked Antigravity directly".into(),
            confidence: Confidence::High,
            freshness: Freshness::Fresh,
        },
        windows,
        supplemental: account.credits,
        reset_credits: None,
    })
}

#[cfg(test)]
fn fetch_with_fallback(
    cloud: &dyn AntigravityTransport,
    local: &dyn LocalUsageTransport,
    credentials: Option<&Credentials>,
    now: OffsetDateTime,
) -> Result<Option<ProviderUsageSnapshot>, ProviderUsageError> {
    let cloud_error = match credentials {
        Some(credentials) => match fetch_cloud(cloud, credentials, now) {
            Ok(snapshot) => return Ok(Some(snapshot)),
            Err(error) => Some(error),
        },
        None => None,
    };
    match local.fetch(now) {
        Ok(Some(mut snapshot)) => {
            snapshot.account = credentials
                .and_then(|credentials| google_subject(cloud, &credentials.access_token));
            Ok(Some(snapshot))
        }
        Ok(None) => match cloud_error {
            Some(error) => Err(error),
            None => Ok(None),
        },
        Err(local_error) => Err(match cloud_error {
            Some(cloud_error) => preferred_error(cloud_error, local_error),
            None => local_error,
        }),
    }
}

fn fetch_with_refresh_fallback(
    cloud: &dyn AntigravityTransport,
    local: &dyn LocalUsageTransport,
    credentials: Option<&Credentials>,
    refreshed: &RefreshCache,
    now: OffsetDateTime,
) -> Result<Option<ProviderUsageSnapshot>, ProviderUsageError> {
    if credentials.is_none() {
        cached_refresh(refreshed, None, now);
    }
    let cloud_error = match credentials {
        Some(credentials) => match fetch_cloud_with_refresh(cloud, credentials, refreshed, now) {
            Ok(snapshot) => return Ok(Some(snapshot)),
            Err(error) => Some(error),
        },
        None => None,
    };
    match local.fetch(now) {
        Ok(Some(mut snapshot)) => {
            snapshot.account = credentials
                .and_then(|credentials| google_subject(cloud, &credentials.access_token));
            Ok(Some(snapshot))
        }
        Ok(None) => cloud_error.map_or(Ok(None), Err),
        Err(local_error) => Err(match cloud_error {
            Some(cloud_error) => preferred_error(cloud_error, local_error),
            None => local_error,
        }),
    }
}

fn fetch_cloud_with_refresh(
    transport: &dyn AntigravityTransport,
    credentials: &Credentials,
    refreshed: &RefreshCache,
    now: OffsetDateTime,
) -> Result<ProviderUsageSnapshot, ProviderUsageError> {
    let mut credentials = credentials.clone();
    if let Some(cached) = cached_refresh(refreshed, credentials.refresh_token.as_deref(), now) {
        credentials = cached;
    }

    let mut did_refresh = false;
    if !credentials.current_at(now) {
        credentials = refresh_credentials(transport, &credentials, refreshed, now)?;
        did_refresh = true;
    }
    match fetch_cloud(transport, &credentials, now) {
        Err(ProviderUsageError::Authentication) if !did_refresh => {
            let credentials = refresh_credentials(transport, &credentials, refreshed, now)?;
            fetch_cloud(transport, &credentials, now)
        }
        result => result,
    }
}

fn cached_refresh(
    refreshed: &RefreshCache,
    refresh_token: Option<&str>,
    now: OffsetDateTime,
) -> Option<Credentials> {
    let mut cached = refreshed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let applicable = cached.as_ref().is_some_and(|cached| {
        refresh_token == Some(cached.refresh_token.as_str())
            && cached
                .credentials
                .expires_at
                .is_some_and(|expiry| expiry > now)
    });
    if !applicable {
        *cached = None;
    }
    cached.as_ref().map(|cached| cached.credentials.clone())
}

fn google_subject(transport: &dyn AntigravityTransport, access_token: &str) -> Option<String> {
    let reply = transport.subject(access_token).ok()?;
    check_status(reply.status).ok()?;
    let value: Value = serde_json::from_str(&reply.body).ok()?;
    value
        .get("sub")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|subject| !subject.is_empty() && subject.len() <= 512)
        .map(str::to_owned)
}

fn refresh_credentials(
    transport: &dyn AntigravityTransport,
    credentials: &Credentials,
    refreshed: &RefreshCache,
    now: OffsetDateTime,
) -> Result<Credentials, ProviderUsageError> {
    let refresh_token = credentials
        .refresh_token
        .as_deref()
        .ok_or(ProviderUsageError::Authentication)?;
    let credentials = transport.refresh(refresh_token, now)?;
    let cache_entry = credentials
        .expires_at
        .filter(|expiry| *expiry > now)
        .map(|_| CachedRefresh {
            refresh_token: refresh_token.to_owned(),
            credentials: credentials.clone(),
        });
    *refreshed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = cache_entry;
    Ok(credentials)
}

fn preferred_error(cloud: ProviderUsageError, local: ProviderUsageError) -> ProviderUsageError {
    let rank = |error| match error {
        ProviderUsageError::Authentication => 4,
        ProviderUsageError::RateLimited => 3,
        ProviderUsageError::Schema(_) => 2,
        ProviderUsageError::Unavailable => 1,
    };
    if rank(cloud) >= rank(local) {
        cloud
    } else {
        local
    }
}

fn check_status(status: reqwest::StatusCode) -> Result<(), ProviderUsageError> {
    match http::status_error(status) {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const NOW: i64 = 1_800_000_000;
    const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(600);
    const LOAD: &str = r#"{
      "cloudaicompanionProject":"projects/synthetic",
      "currentTier":{"name":"Google AI Pro","id":"pro-tier"},
      "planInfo":{"monthlyPromptCredits":1000},
      "availablePromptCredits":800
    }"#;
    const QUOTA: &str = r#"{"groups":[
      {"displayName":"Gemini","buckets":[
        {"bucketId":"gemini-5h","remainingFraction":0.8},
        {"bucketId":"gemini-weekly","remainingFraction":0.7}]},
      {"displayName":"Claude + GPT","buckets":[
        {"bucketId":"3p-5h","remainingFraction":0.6},
        {"bucketId":"3p-weekly","remainingFraction":0.5}]}
    ]}"#;
    const PARTIAL_QUOTA: &str = r#"{"groups":[
      {"displayName":"Gemini","buckets":[
        {"bucketId":"gemini-weekly","remainingFraction":0.7}]},
      {"displayName":"Claude + GPT","buckets":[
        {"bucketId":"3p-weekly","remainingFraction":0.5}]}
    ]}"#;
    const MODELS: &str = r#"{"models":{
      "gemini-3-pro-high":{"displayName":"Gemini 3 Pro (High)","quotaInfo":{"remainingFraction":0.8,"resetTime":"2027-01-15T12:00:00Z"}},
      "claude-sonnet":{"displayName":"Claude Sonnet","quotaInfo":{"remainingFraction":0.6}}
    }}"#;

    struct Fake {
        load_status: reqwest::StatusCode,
        quota_status: reqwest::StatusCode,
        calls: Arc<AtomicUsize>,
    }

    struct RefreshFake {
        load_statuses: Mutex<Vec<reqwest::StatusCode>>,
        tokens: Mutex<Vec<String>>,
        refreshes: AtomicUsize,
    }

    struct CompatibilityFake {
        quota: Result<HttpReply, ProviderUsageError>,
        model_projects: Mutex<Vec<String>>,
    }

    struct FakeLocal {
        calls: Arc<AtomicUsize>,
        result: Result<bool, ProviderUsageError>,
    }

    impl LocalUsageTransport for FakeLocal {
        fn fetch(
            &self,
            now: OffsetDateTime,
        ) -> Result<Option<ProviderUsageSnapshot>, ProviderUsageError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.map(|found| found.then(|| local_snapshot(now)))
        }
    }

    fn fake_local(
        result: Result<bool, ProviderUsageError>,
    ) -> (Box<dyn LocalUsageTransport>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Box::new(FakeLocal {
                calls: Arc::clone(&calls),
                result,
            }),
            calls,
        )
    }

    fn local_snapshot(now: OffsetDateTime) -> ProviderUsageSnapshot {
        let status = antigravity::parse_get_user_status(
            r#"{"userStatus":{"email":"person@example.test","planStatus":{"planInfo":{"planName":"Pro"}},"clientModelConfigs":[{"label":"Gemini 3 Pro","quotaInfo":{"remainingFraction":0.5}}]}}"#,
        )
        .unwrap();
        ProviderUsageSnapshot {
            provider: crate::provider_usage::providers::GOOGLE,
            account: status.account,
            account_uuid: None,
            account_email: None,
            plan: status.plan,
            plan_tier: status.tier,
            observed_at: now,
            source: UsageSource {
                id: SOURCE_ID,
                label: "Read from Antigravity IDE".into(),
                confidence: Confidence::Medium,
                freshness: Freshness::Fresh,
            },
            windows: status.windows,
            supplemental: None,
            reset_credits: None,
        }
    }

    impl AntigravityTransport for Fake {
        fn load(&self, _: &str) -> Result<HttpReply, ProviderUsageError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(HttpReply {
                status: self.load_status,
                body: LOAD.into(),
            })
        }

        fn quota(&self, _: &str, project: &str) -> Result<HttpReply, ProviderUsageError> {
            assert_eq!(project, "projects/synthetic");
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(HttpReply {
                status: self.quota_status,
                body: QUOTA.into(),
            })
        }

        fn subject(&self, _: &str) -> Result<HttpReply, ProviderUsageError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(HttpReply {
                status: reqwest::StatusCode::OK,
                body: r#"{"sub":"google-subject"}"#.into(),
            })
        }
    }

    impl AntigravityTransport for RefreshFake {
        fn load(&self, access_token: &str) -> Result<HttpReply, ProviderUsageError> {
            self.tokens.lock().unwrap().push(access_token.to_owned());
            let status = self.load_statuses.lock().unwrap().remove(0);
            Ok(HttpReply {
                status,
                body: LOAD.into(),
            })
        }

        fn quota(&self, _: &str, _: &str) -> Result<HttpReply, ProviderUsageError> {
            Ok(HttpReply {
                status: reqwest::StatusCode::OK,
                body: QUOTA.into(),
            })
        }

        fn refresh(
            &self,
            refresh_token: &str,
            now: OffsetDateTime,
        ) -> Result<Credentials, ProviderUsageError> {
            assert_eq!(refresh_token, "synthetic-refresh");
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            Ok(Credentials {
                access_token: "refreshed-access".into(),
                refresh_token: Some(refresh_token.to_owned()),
                expires_at: Some(now + time::Duration::hours(1)),
            })
        }
    }

    impl AntigravityTransport for CompatibilityFake {
        fn load(&self, _: &str) -> Result<HttpReply, ProviderUsageError> {
            Ok(HttpReply {
                status: reqwest::StatusCode::OK,
                body: LOAD.into(),
            })
        }

        fn quota(&self, _: &str, _: &str) -> Result<HttpReply, ProviderUsageError> {
            self.quota.as_ref().map_or_else(
                |error| Err(*error),
                |reply| {
                    Ok(HttpReply {
                        status: reply.status,
                        body: reply.body.clone(),
                    })
                },
            )
        }

        fn models(&self, _: &str, project: &str) -> Result<HttpReply, ProviderUsageError> {
            self.model_projects.lock().unwrap().push(project.to_owned());
            Ok(HttpReply {
                status: reqwest::StatusCode::OK,
                body: MODELS.into(),
            })
        }
    }

    fn credentials() -> Credentials {
        Credentials {
            access_token: "synthetic-access".into(),
            refresh_token: None,
            expires_at: Some(OffsetDateTime::from_unix_timestamp(NOW + 3_600).unwrap()),
        }
    }

    #[test]
    fn cloud_flow_loads_the_project_before_the_summary() {
        let calls = Arc::new(AtomicUsize::new(0));
        let fake = Fake {
            load_status: reqwest::StatusCode::OK,
            quota_status: reqwest::StatusCode::OK,
            calls: Arc::clone(&calls),
        };
        let snapshot = fetch_cloud(
            &fake,
            &credentials(),
            OffsetDateTime::from_unix_timestamp(NOW).unwrap(),
        )
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(snapshot.provider, crate::provider_usage::providers::GOOGLE);
        assert_eq!(snapshot.source.id, SOURCE_ID);
        assert!(snapshot.source.label.contains("Antigravity"));
        assert_eq!(snapshot.windows.len(), 4);
        assert_eq!(snapshot.plan.as_deref(), Some("Google AI Pro"));
        assert!(snapshot.supplemental.is_some());
        assert_eq!(snapshot.account.as_deref(), Some("google-subject"));
    }

    #[test]
    fn partial_cloud_summary_fills_missing_shared_pools_from_model_windows() {
        let fake = CompatibilityFake {
            quota: Ok(HttpReply {
                status: reqwest::StatusCode::OK,
                body: PARTIAL_QUOTA.into(),
            }),
            model_projects: Mutex::default(),
        };
        let snapshot = fetch_cloud(
            &fake,
            &credentials(),
            OffsetDateTime::from_unix_timestamp(NOW).unwrap(),
        )
        .unwrap();

        assert_eq!(snapshot.windows.len(), 4);
        assert_eq!(
            fake.model_projects.lock().unwrap().as_slice(),
            ["projects/synthetic"]
        );
        assert!(
            snapshot
                .windows
                .iter()
                .any(|window| window.id == "antigravity-gemini-weekly")
        );
        assert_eq!(
            snapshot
                .windows
                .iter()
                .map(|window| window.id.as_str())
                .collect::<std::collections::HashSet<_>>(),
            [
                "antigravity-gemini-5h",
                "antigravity-gemini-weekly",
                "antigravity-claude-gpt-5h",
                "antigravity-claude-gpt-weekly",
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn unavailable_summary_uses_bounded_model_compatibility_response() {
        let fake = CompatibilityFake {
            quota: Err(ProviderUsageError::Unavailable),
            model_projects: Mutex::default(),
        };
        let snapshot = fetch_cloud(
            &fake,
            &credentials(),
            OffsetDateTime::from_unix_timestamp(NOW).unwrap(),
        )
        .unwrap();
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.windows[0].id, "antigravity-gemini-5h");
        assert_eq!(snapshot.windows[1].id, "antigravity-claude-gpt-5h");
    }

    #[test]
    fn expired_credentials_refresh_once_and_reuse_the_memory_cache() {
        let fake = RefreshFake {
            load_statuses: Mutex::new(vec![reqwest::StatusCode::OK, reqwest::StatusCode::OK]),
            tokens: Mutex::default(),
            refreshes: AtomicUsize::new(0),
        };
        let expired = Credentials {
            access_token: "expired-access".into(),
            refresh_token: Some("synthetic-refresh".into()),
            expires_at: Some(OffsetDateTime::from_unix_timestamp(NOW - 1).unwrap()),
        };
        let cache = Mutex::default();
        let now = OffsetDateTime::from_unix_timestamp(NOW).unwrap();

        fetch_cloud_with_refresh(&fake, &expired, &cache, now).unwrap();
        fetch_cloud_with_refresh(&fake, &expired, &cache, now).unwrap();

        assert_eq!(fake.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(
            fake.tokens.lock().unwrap().as_slice(),
            ["refreshed-access", "refreshed-access"]
        );
    }

    #[test]
    fn refresh_cache_drops_expired_and_replaced_credentials() {
        let now = OffsetDateTime::from_unix_timestamp(NOW).unwrap();
        let cache = Mutex::new(Some(CachedRefresh {
            refresh_token: "old-refresh".into(),
            credentials: Credentials {
                access_token: "old-access".into(),
                refresh_token: Some("old-refresh".into()),
                expires_at: Some(now + time::Duration::hours(1)),
            },
        }));

        assert!(cached_refresh(&cache, Some("new-refresh"), now).is_none());
        assert!(cache.lock().unwrap().is_none());

        *cache.lock().unwrap() = Some(CachedRefresh {
            refresh_token: "new-refresh".into(),
            credentials: Credentials {
                access_token: "new-access".into(),
                refresh_token: Some("new-refresh".into()),
                expires_at: Some(now),
            },
        });
        assert!(cached_refresh(&cache, Some("new-refresh"), now).is_none());
        assert!(cache.lock().unwrap().is_none());
    }

    #[test]
    fn missing_current_credentials_clear_the_refresh_cache() {
        let now = OffsetDateTime::from_unix_timestamp(NOW).unwrap();
        let cache = Mutex::new(Some(CachedRefresh {
            refresh_token: "synthetic-refresh".into(),
            credentials: Credentials {
                access_token: "cached-access".into(),
                refresh_token: Some("synthetic-refresh".into()),
                expires_at: Some(now + time::Duration::hours(1)),
            },
        }));

        cached_refresh(&cache, None, now);

        assert!(cache.lock().unwrap().is_none());
    }

    #[test]
    fn authentication_failure_refreshes_and_retries_once() {
        let fake = RefreshFake {
            load_statuses: Mutex::new(vec![
                reqwest::StatusCode::UNAUTHORIZED,
                reqwest::StatusCode::OK,
            ]),
            tokens: Mutex::default(),
            refreshes: AtomicUsize::new(0),
        };
        let credentials = Credentials {
            access_token: "rejected-access".into(),
            refresh_token: Some("synthetic-refresh".into()),
            expires_at: Some(OffsetDateTime::from_unix_timestamp(NOW + 3_600).unwrap()),
        };

        fetch_cloud_with_refresh(
            &fake,
            &credentials,
            &Mutex::default(),
            OffsetDateTime::from_unix_timestamp(NOW).unwrap(),
        )
        .unwrap();

        assert_eq!(fake.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(
            fake.tokens.lock().unwrap().as_slice(),
            ["rejected-access", "refreshed-access"]
        );
    }

    #[test]
    fn fake_http_statuses_map_through_the_source() {
        for (status, expected) in [
            (
                reqwest::StatusCode::UNAUTHORIZED,
                ProviderUsageError::Authentication,
            ),
            (
                reqwest::StatusCode::FORBIDDEN,
                ProviderUsageError::Authentication,
            ),
            (
                reqwest::StatusCode::TOO_MANY_REQUESTS,
                ProviderUsageError::RateLimited,
            ),
            (
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                ProviderUsageError::Unavailable,
            ),
        ] {
            let fake = Fake {
                load_status: status,
                quota_status: reqwest::StatusCode::OK,
                calls: Arc::new(AtomicUsize::new(0)),
            };
            assert_eq!(
                fetch_cloud(
                    &fake,
                    &credentials(),
                    OffsetDateTime::from_unix_timestamp(NOW).unwrap()
                ),
                Err(expected)
            );

            let fake = Fake {
                load_status: reqwest::StatusCode::OK,
                quota_status: status,
                calls: Arc::new(AtomicUsize::new(0)),
            };
            assert_eq!(
                fetch_cloud(
                    &fake,
                    &credentials(),
                    OffsetDateTime::from_unix_timestamp(NOW).unwrap()
                ),
                Err(expected)
            );
        }
    }

    #[test]
    fn expired_credentials_surface_auth_without_a_request() {
        let calls = Arc::new(AtomicUsize::new(0));
        let fake = Fake {
            load_status: reqwest::StatusCode::OK,
            quota_status: reqwest::StatusCode::OK,
            calls: Arc::clone(&calls),
        };
        let expired = Credentials {
            access_token: "expired".into(),
            refresh_token: None,
            expires_at: Some(OffsetDateTime::from_unix_timestamp(NOW - 1).unwrap()),
        };
        assert_eq!(
            fetch_cloud(
                &fake,
                &expired,
                OffsetDateTime::from_unix_timestamp(NOW).unwrap()
            ),
            Err(ProviderUsageError::Authentication)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn agy_plain_and_keyring_wrapped_credentials_parse() {
        let json = r#"{"token":{"access_token":"synthetic","refresh_token":"synthetic-refresh","expiry":"2027-01-15T12:00:00Z"}}"#;
        assert_eq!(parse_agy_secret(json).unwrap().access_token, "synthetic");
        assert_eq!(
            parse_agy_secret(json).unwrap().refresh_token.as_deref(),
            Some("synthetic-refresh")
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(json);
        assert_eq!(
            parse_agy_secret(&format!("go-keyring-base64:{encoded}"))
                .unwrap()
                .access_token,
            "synthetic"
        );
    }

    #[test]
    fn present_malformed_expiry_rejects_the_credential() {
        assert!(
            parse_agy_secret(r#"{"token":{"access_token":"synthetic","expiry":"not-a-time"}}"#)
                .is_none()
        );
    }

    #[test]
    fn oversized_credential_files_are_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credential");
        fs::write(&path, vec![b'x'; (MAX_CREDENTIAL_BYTES + 1) as usize]).unwrap();
        assert!(read_bounded(&path).is_none());
    }

    #[test]
    fn bounded_credentials_require_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_bounded(dir.path()).is_none());
        let path = dir.path().join("credential");
        fs::write(&path, "synthetic").unwrap();
        assert_eq!(read_bounded(&path).as_deref(), Some("synthetic"));
    }

    fn field(number: u8, bytes: &[u8]) -> Vec<u8> {
        let mut value = vec![number << 3 | 2, bytes.len() as u8];
        value.extend_from_slice(bytes);
        value
    }

    fn unified_value(access_token: &str, expiry: i64) -> String {
        let mut timestamp = vec![8];
        let mut seconds = expiry as u64;
        while seconds >= 0x80 {
            timestamp.push((seconds as u8) | 0x80);
            seconds >>= 7;
        }
        timestamp.push(seconds as u8);
        let mut oauth = field(1, access_token.as_bytes());
        oauth.extend(field(4, &timestamp));
        let encoded = base64::engine::general_purpose::STANDARD.encode(oauth);
        let payload = field(1, encoded.as_bytes());
        let mut wrapper = field(1, b"oauthTokenInfoSentinelKey");
        wrapper.extend(field(2, &payload));
        base64::engine::general_purpose::STANDARD.encode(field(1, &wrapper))
    }

    fn write_ide_credentials(path: &Path, access_token: &str, expiry: i64) {
        let connection = rusqlite::Connection::open(path).unwrap();
        connection
            .execute("CREATE TABLE ItemTable (key TEXT, value TEXT)", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                rusqlite::params![
                    "antigravityUnifiedStateSync.oauthToken",
                    unified_value(access_token, expiry)
                ],
            )
            .unwrap();
    }

    #[test]
    fn unified_ide_oauth_contract_yields_access_refresh_and_expiry() {
        let mut timestamp = vec![8];
        let mut seconds = (NOW + 3_600) as u64;
        while seconds >= 0x80 {
            timestamp.push((seconds as u8) | 0x80);
            seconds >>= 7;
        }
        timestamp.push(seconds as u8);
        let mut oauth = field(1, b"ide-access");
        oauth.extend(field(3, b"ignored-refresh"));
        oauth.extend(field(4, &timestamp));
        let encoded = base64::engine::general_purpose::STANDARD.encode(oauth);
        let payload = field(1, encoded.as_bytes());
        let mut wrapper = field(1, b"oauthTokenInfoSentinelKey");
        wrapper.extend(field(2, &payload));
        let outer = field(1, &wrapper);
        let value = base64::engine::general_purpose::STANDARD.encode(outer);

        let parsed = parse_unified_oauth(&value).unwrap();
        assert_eq!(parsed.access_token, "ide-access");
        assert_eq!(parsed.refresh_token.as_deref(), Some("ignored-refresh"));
        assert_eq!(parsed.expires_at.unwrap().unix_timestamp(), NOW + 3_600);
    }

    #[test]
    fn expired_carriers_are_skipped_until_a_current_ide_token_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let agy_path = dir.path().join("agy-token");
        fs::write(
            &agy_path,
            r#"{"token":{"access_token":"expired-file","expiry":"2027-01-15T07:59:59Z"}}"#,
        )
        .unwrap();
        let expired_ide = dir.path().join("expired.vscdb");
        let current_ide = dir.path().join("current.vscdb");
        write_ide_credentials(&expired_ide, "expired-ide", NOW - 1);
        write_ide_credentials(&current_ide, "current-ide", NOW + 3_600);
        let (local, _) = fake_local(Ok(false));
        let source = AntigravityDirectFetch::with_transports(
            agy_path,
            vec![expired_ide, current_ide],
            Box::new(Fake {
                load_status: reqwest::StatusCode::OK,
                quota_status: reqwest::StatusCode::OK,
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            local,
        );
        let selected = source
            .credentials(OffsetDateTime::from_unix_timestamp(NOW).unwrap())
            .unwrap();
        assert_eq!(selected.access_token, "current-ide");
    }

    #[test]
    fn source_cooldown_prevents_a_second_cloud_flow() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credential");
        fs::write(
            &path,
            r#"{"token":{"access_token":"synthetic","expiry":"2099-01-01T00:00:00Z"}}"#,
        )
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let (local, local_calls) = fake_local(Ok(false));
        let source = AntigravityDirectFetch::with_transports(
            path,
            Vec::new(),
            Box::new(Fake {
                load_status: reqwest::StatusCode::OK,
                quota_status: reqwest::StatusCode::OK,
                calls: Arc::clone(&calls),
            }),
            local,
        );
        source.fetch(MAX_AGE);
        source.fetch(MAX_AGE);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(local_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn absent_credentials_skip_cloud_and_use_local() {
        let cloud_calls = Arc::new(AtomicUsize::new(0));
        let (local, local_calls) = fake_local(Ok(true));
        let source = AntigravityDirectFetch::with_transports(
            PathBuf::from("/missing/credential"),
            Vec::new(),
            Box::new(Fake {
                load_status: reqwest::StatusCode::OK,
                quota_status: reqwest::StatusCode::OK,
                calls: Arc::clone(&cloud_calls),
            }),
            local,
        );
        let outcome = source.fetch(MAX_AGE);
        assert_eq!(cloud_calls.load(Ordering::SeqCst), 0);
        assert_eq!(local_calls.load(Ordering::SeqCst), 1);
        assert_eq!(outcome.snapshots[0].source.confidence, Confidence::Medium);
        assert!(outcome.snapshots[0].account.is_none());
    }

    #[test]
    fn cloud_failure_falls_back_locally_without_passing_an_oauth_argument() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credential");
        fs::write(
            &path,
            r#"{"token":{"access_token":"must-stay-cloud-only","expiry":"2099-01-01T00:00:00Z"}}"#,
        )
        .unwrap();
        let cloud_calls = Arc::new(AtomicUsize::new(0));
        let (local, local_calls) = fake_local(Ok(true));
        let source = AntigravityDirectFetch::with_transports(
            path,
            Vec::new(),
            Box::new(Fake {
                load_status: reqwest::StatusCode::UNAUTHORIZED,
                quota_status: reqwest::StatusCode::OK,
                calls: Arc::clone(&cloud_calls),
            }),
            local,
        );
        let outcome = source.fetch(MAX_AGE);
        assert_eq!(cloud_calls.load(Ordering::SeqCst), 2);
        assert_eq!(local_calls.load(Ordering::SeqCst), 1);
        assert!(outcome.error.is_none());
        assert_eq!(
            outcome.snapshots[0].source.label,
            "Read from Antigravity IDE"
        );
        assert_ne!(
            outcome.snapshots[0].account.as_deref(),
            Some("person@example.test")
        );
    }

    #[test]
    fn cloud_success_never_probes_local_and_both_failures_keep_actionable_error() {
        let cloud = Fake {
            load_status: reqwest::StatusCode::OK,
            quota_status: reqwest::StatusCode::OK,
            calls: Arc::new(AtomicUsize::new(0)),
        };
        let (local, local_calls) = fake_local(Err(ProviderUsageError::Unavailable));
        let result = fetch_with_fallback(
            &cloud,
            local.as_ref(),
            Some(&credentials()),
            OffsetDateTime::from_unix_timestamp(NOW).unwrap(),
        )
        .unwrap();
        assert!(result.is_some());
        assert_eq!(local_calls.load(Ordering::SeqCst), 0);

        let cloud = Fake {
            load_status: reqwest::StatusCode::UNAUTHORIZED,
            quota_status: reqwest::StatusCode::OK,
            calls: Arc::new(AtomicUsize::new(0)),
        };
        let (local, _) = fake_local(Err(ProviderUsageError::Unavailable));
        assert_eq!(
            fetch_with_fallback(
                &cloud,
                local.as_ref(),
                Some(&credentials()),
                OffsetDateTime::from_unix_timestamp(NOW).unwrap()
            ),
            Err(ProviderUsageError::Authentication)
        );
    }

    #[test]
    fn source_is_online_gated_and_registered_for_google() {
        let source = AntigravityDirectFetch::new();
        assert!(source.requires_online_opt_in());
        assert_eq!(source.provider(), crate::provider_usage::providers::GOOGLE);
    }
}
