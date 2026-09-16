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
use rusqlite::{OpenFlags, OptionalExtension as _};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::provider_usage::live::antigravity;
use crate::provider_usage::live::model::{
    Confidence, Detection, Freshness, LoginCarrier, Presence, ProviderUsageError,
    ProviderUsageSnapshot, SourceErrorDetail, UsageSource,
};
use crate::provider_usage::live::{LiveUsageSource, SourceOutcome};

use super::antigravity_local::{LocalProbe, LocalUsageTransport};
use super::cooldown::{Cooldown, FetchFailure};
use super::http;
#[cfg(target_os = "macos")]
#[cfg(target_os = "macos")]
use super::presence::KeychainMetadata;
use super::presence::{self, PresenceProbe, SystemPresenceProbe};

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

    fn detect(&self, _online: bool) -> Presence {
        detect_presence(
            &SystemPresenceProbe {
                #[cfg(target_os = "macos")]
                try_keychain: self.try_keychain,
            },
            &SqliteIdeState,
            self.agy_path.as_deref(),
            &self.ide_paths,
        )
    }

    fn fetch(&self, max_age: std::time::Duration) -> SourceOutcome {
        let now = OffsetDateTime::now_utc();
        self.cooldown.poll(now, max_age, || {
            fetch_with_refresh_fallback(
                self.transport.as_ref(),
                self.local.as_ref(),
                self.credentials(now).as_ref(),
                &self.refreshed,
                now,
            )
        })
    }
}

/// The keyring entry `agy` keeps its login in.
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "gemini";
#[cfg(target_os = "macos")]
const KEYCHAIN_ACCOUNT: &str = "antigravity";

/// The CLI's own executable name on `PATH`. Only the macOS branch checks it.
#[cfg(target_os = "macos")]
const BINARY: &str = "agy";

/// The one vendor-specific presence check: whether the IDE's state database
/// holds the unified OAuth key. Read-only, key column only, never the value.
/// Injectable beside [`PresenceProbe`] so a test can record the call.
trait IdeStateProbe {
    fn ide_has_oauth_key(&self, path: &Path) -> Result<bool, ()>;
}

struct SqliteIdeState;

impl IdeStateProbe for SqliteIdeState {
    fn ide_has_oauth_key(&self, path: &Path) -> Result<bool, ()> {
        ide_has_oauth_key(path)
    }
}

fn ide_has_oauth_key(path: &Path) -> Result<bool, ()> {
    if fs::metadata(path).map_err(|_| ())?.len() > MAX_STATE_DB_BYTES {
        return Err(());
    }
    let connection = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| ())?;
    connection
        .busy_timeout(std::time::Duration::from_millis(100))
        .map_err(|_| ())?;
    connection
        .query_row(
            "SELECT 1 FROM ItemTable WHERE key = 'antigravityUnifiedStateSync.oauthToken' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .optional()
        .map(|row| row.is_some())
        .map_err(|_| ())
}

/// Presence rules, in order: the `agy` token file is a login; an IDE state
/// database with the OAuth key is a login; the keyring entry is a login
/// (macOS only); a tool directory or the binary is an install without a
/// login; nothing is no install. Off macOS the keyring and the running
/// language server are not checked, so a negative stays `Unknown`.
fn detect_presence(
    probe: &impl PresenceProbe,
    ide_state: &impl IdeStateProbe,
    agy_path: Option<&Path>,
    ide_paths: &[PathBuf],
) -> Presence {
    if let Some(path) = agy_path {
        match presence::path_exists(probe, path) {
            Ok(true) => return Presence::via(Detection::SignedIn, LoginCarrier::AgyToken),
            Ok(false) => {}
            Err(_) => return Presence::UNKNOWN,
        }
    }
    for path in ide_paths {
        match presence::path_exists(probe, path) {
            Ok(true) => match ide_state.ide_has_oauth_key(path) {
                Ok(true) => {
                    return Presence::via(Detection::SignedIn, LoginCarrier::AntigravityIde);
                }
                Ok(false) => {}
                Err(()) => return Presence::UNKNOWN,
            },
            Ok(false) => {}
            Err(_) => return Presence::UNKNOWN,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Presence::UNKNOWN
    }
    #[cfg(target_os = "macos")]
    {
        match probe.keychain_metadata(KEYCHAIN_SERVICE, Some(KEYCHAIN_ACCOUNT)) {
            KeychainMetadata::Found(_) => {
                return Presence::via(Detection::SignedIn, LoginCarrier::AntigravityKeyring);
            }
            KeychainMetadata::Absent => {}
            KeychainMetadata::Unreadable => return Presence::UNKNOWN,
        }
        let Some(agy_dir) = agy_path.and_then(Path::parent) else {
            return Presence::UNKNOWN;
        };
        // IDE paths end with User/globalStorage/state.vscdb. Check the application support directory above them.
        let directories = std::iter::once(agy_dir)
            .chain(ide_paths.iter().filter_map(|path| path.ancestors().nth(3)));
        for directory in directories {
            match presence::path_exists(probe, directory) {
                Ok(true) => return Presence::new(Detection::InstalledNotSignedIn),
                Ok(false) => {}
                Err(_) => return Presence::UNKNOWN,
            }
        }
        if probe.binary_present(BINARY) {
            Presence::new(Detection::InstalledNotSignedIn)
        } else {
            Presence::new(Detection::NotInstalled)
        }
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
    ) -> Result<Credentials, RefreshError> {
        Err(ProviderUsageError::Unavailable.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RefreshError {
    error: ProviderUsageError,
    detail: Option<SourceErrorDetail>,
}

impl From<ProviderUsageError> for RefreshError {
    fn from(error: ProviderUsageError) -> Self {
        Self {
            error,
            detail: None,
        }
    }
}

impl From<RefreshError> for FetchFailure {
    fn from(failure: RefreshError) -> Self {
        Self {
            error: failure.error,
            detail: failure.detail,
            last_known: None,
        }
    }
}

fn refresh_client_credentials<'a>(
    client_id: Option<&'a str>,
    client_secret: Option<&'a str>,
) -> Result<(&'a str, &'a str), RefreshError> {
    let unsupported = RefreshError {
        error: ProviderUsageError::Authentication,
        detail: Some(SourceErrorDetail::RefreshUnsupported),
    };
    let client_id = client_id
        .filter(|value| !value.trim().is_empty())
        .ok_or(unsupported)?;
    let client_secret = client_secret
        .filter(|value| !value.trim().is_empty())
        .ok_or(unsupported)?;
    Ok((client_id, client_secret))
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
    ) -> Result<Credentials, RefreshError> {
        refresh_access_token(refresh_token, now, CLIENT_ID, CLIENT_SECRET)
    }
}

fn refresh_access_token(
    refresh_token: &str,
    now: OffsetDateTime,
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Result<Credentials, RefreshError> {
    let (client_id, client_secret) = refresh_client_credentials(client_id, client_secret)?;
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
        ProviderUsageError::Schema(crate::provider_usage::live::model::SchemaReason::InvalidValue)
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
) -> Result<Option<ProviderUsageSnapshot>, FetchFailure> {
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
        Ok(None) => cloud_error.map_or(Ok(None), |error| Err(error.into())),
        Err(local_error) => Err(match cloud_error {
            Some(cloud_error)
                if preferred_error(cloud_error.error, local_error) == cloud_error.error =>
            {
                cloud_error.into()
            }
            _ => local_error.into(),
        }),
    }
}

fn fetch_cloud_with_refresh(
    transport: &dyn AntigravityTransport,
    credentials: &Credentials,
    refreshed: &RefreshCache,
    now: OffsetDateTime,
) -> Result<ProviderUsageSnapshot, RefreshError> {
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
            fetch_cloud(transport, &credentials, now).map_err(Into::into)
        }
        result => result.map_err(Into::into),
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
) -> Result<Credentials, RefreshError> {
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

    use super::super::presence::RecordingPresence;
    use std::io;

    /// A probe whose unlisted paths answer from the fixture on disk.
    fn probe() -> RecordingPresence {
        RecordingPresence {
            fallthrough: true,
            ..Default::default()
        }
    }

    /// Records the IDE key check on the same call log as the probe.
    struct RecordingIdeState<'a>(&'a RecordingPresence);

    impl IdeStateProbe for RecordingIdeState<'_> {
        fn ide_has_oauth_key(&self, path: &Path) -> Result<bool, ()> {
            self.0
                .record(format!("ide_has_oauth_key:{}", path.display()));
            ide_has_oauth_key(path)
        }
    }

    fn detected(probe: &RecordingPresence, agy: &Path, ide: &[PathBuf]) -> Detection {
        detect_presence(probe, &RecordingIdeState(probe), Some(agy), ide).detection
    }

    fn presence_paths(root: &Path) -> (PathBuf, [PathBuf; 2]) {
        (
            root.join(".gemini/antigravity-cli/antigravity-oauth-token"),
            ["Antigravity IDE", "Antigravity"]
                .map(|name| root.join(name).join("User/globalStorage/state.vscdb")),
        )
    }

    fn write_presence_database(path: &Path, key: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = rusqlite::Connection::open(path).unwrap();
        connection
            .execute("CREATE TABLE ItemTable (key TEXT PRIMARY KEY)", [])
            .unwrap();
        connection
            .execute("INSERT INTO ItemTable (key) VALUES (?1)", [key])
            .unwrap();
    }

    #[test]
    fn detection_without_carriers_uses_only_metadata_calls() {
        let dir = tempfile::tempdir().unwrap();
        let (agy, ide) = presence_paths(dir.path());
        let probe = probe();
        let expected = if cfg!(target_os = "macos") {
            Detection::NotInstalled
        } else {
            Detection::Unknown
        };
        assert_eq!(detected(&probe, &agy, &ide), expected);
        let expected_calls = vec![
            format!("path_exists:{}", agy.display()),
            format!("path_exists:{}", ide[0].display()),
            format!("path_exists:{}", ide[1].display()),
            #[cfg(target_os = "macos")]
            "keychain_metadata".into(),
            #[cfg(target_os = "macos")]
            format!("path_exists:{}", agy.parent().unwrap().display()),
            #[cfg(target_os = "macos")]
            format!(
                "path_exists:{}",
                dir.path().join("Antigravity IDE").display()
            ),
            #[cfg(target_os = "macos")]
            format!("path_exists:{}", dir.path().join("Antigravity").display()),
            #[cfg(target_os = "macos")]
            "binary_present".into(),
        ];
        assert_eq!(*probe.calls.borrow(), expected_calls);
    }

    #[test]
    fn detection_of_empty_tool_directories_is_platform_limited() {
        for name in [".gemini/antigravity-cli", "Antigravity IDE", "Antigravity"] {
            let dir = tempfile::tempdir().unwrap();
            let (agy, ide) = presence_paths(dir.path());
            fs::create_dir_all(dir.path().join(name)).unwrap();
            let expected = if cfg!(target_os = "macos") {
                Detection::InstalledNotSignedIn
            } else {
                Detection::Unknown
            };
            assert_eq!(detected(&probe(), &agy, &ide), expected);
        }
    }

    #[test]
    fn detection_of_a_token_file_never_parses_it_or_probes_other_carriers() {
        let dir = tempfile::tempdir().unwrap();
        let (agy, ide) = presence_paths(dir.path());
        fs::create_dir_all(agy.parent().unwrap()).unwrap();
        fs::write(&agy, "not valid JSON").unwrap();
        let probe = probe();
        assert_eq!(
            detect_presence(&probe, &RecordingIdeState(&probe), Some(&agy), &ide),
            Presence::via(Detection::SignedIn, LoginCarrier::AgyToken)
        );
        assert_eq!(
            *probe.calls.borrow(),
            [format!("path_exists:{}", agy.display())]
        );
    }

    #[test]
    fn detection_queries_the_oauth_key_without_a_value_column() {
        for index in 0..2 {
            let dir = tempfile::tempdir().unwrap();
            let (agy, ide) = presence_paths(dir.path());
            write_presence_database(&ide[index], "antigravityUnifiedStateSync.oauthToken");
            let before = fs::read(&ide[index]).unwrap();
            let probe = probe();
            assert_eq!(
                detect_presence(&probe, &RecordingIdeState(&probe), Some(&agy), &ide),
                Presence::via(Detection::SignedIn, LoginCarrier::AntigravityIde)
            );
            let mut expected = vec![format!("path_exists:{}", agy.display())];
            expected.extend(
                ide[..=index]
                    .iter()
                    .map(|path| format!("path_exists:{}", path.display())),
            );
            expected.push(format!("ide_has_oauth_key:{}", ide[index].display()));
            assert_eq!(*probe.calls.borrow(), expected);
            assert_eq!(fs::read(&ide[index]).unwrap(), before);
        }
    }

    #[test]
    fn detection_does_not_treat_an_unrelated_database_key_as_a_login() {
        let dir = tempfile::tempdir().unwrap();
        let (agy, ide) = presence_paths(dir.path());
        write_presence_database(&ide[0], "unrelated.setting");
        let expected = if cfg!(target_os = "macos") {
            Detection::InstalledNotSignedIn
        } else {
            Detection::Unknown
        };
        assert_eq!(detected(&probe(), &agy, &ide), expected);
    }

    #[test]
    fn detection_keeps_database_open_and_query_errors_unknown() {
        for invalid_database in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let (agy, ide) = presence_paths(dir.path());
            fs::create_dir_all(ide[0].parent().unwrap()).unwrap();
            if invalid_database {
                fs::write(&ide[0], "not a SQLite database").unwrap();
            } else {
                rusqlite::Connection::open(&ide[0]).unwrap();
            }
            let probe = probe();
            assert_eq!(detected(&probe, &agy, &ide), Detection::Unknown);
            assert_eq!(
                probe.calls.borrow().last().unwrap(),
                &format!("ide_has_oauth_key:{}", ide[0].display())
            );
        }
    }

    #[test]
    fn the_presence_query_does_not_create_a_missing_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.vscdb");
        assert_eq!(ide_has_oauth_key(&path), Err(()));
        assert!(!path.exists());
    }

    #[test]
    fn detection_rejects_an_oversized_database() {
        let dir = tempfile::tempdir().unwrap();
        let (agy, ide) = presence_paths(dir.path());
        write_presence_database(&ide[0], "antigravityUnifiedStateSync.oauthToken");
        fs::OpenOptions::new()
            .write(true)
            .open(&ide[0])
            .unwrap()
            .set_len(MAX_STATE_DB_BYTES + 1)
            .unwrap();
        assert_eq!(detected(&probe(), &agy, &ide), Detection::Unknown);
    }

    #[test]
    fn detection_keeps_path_metadata_errors_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let (agy, ide) = presence_paths(dir.path());
        let paths = [
            agy.as_path(),
            ide[0].as_path(),
            ide[1].as_path(),
            #[cfg(target_os = "macos")]
            agy.parent().unwrap(),
            #[cfg(target_os = "macos")]
            ide[0].ancestors().nth(3).unwrap(),
            #[cfg(target_os = "macos")]
            ide[1].ancestors().nth(3).unwrap(),
        ];
        for path in paths {
            for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::Other] {
                let mut probe = probe();
                probe.paths.insert(path.into(), Err(kind));
                assert_eq!(detected(&probe, &agy, &ide), Detection::Unknown);
                assert_eq!(
                    probe.calls.borrow().last().unwrap(),
                    &format!("path_exists:{}", path.display())
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detection_uses_keychain_attributes_without_a_secret_read() {
        let dir = tempfile::tempdir().unwrap();
        let (agy, ide) = presence_paths(dir.path());
        for (metadata, expected) in [
            (
                KeychainMetadata::Found(b"synthetic attributes".to_vec()),
                Presence::via(Detection::SignedIn, LoginCarrier::AntigravityKeyring),
            ),
            (KeychainMetadata::Unreadable, Presence::UNKNOWN),
        ] {
            let probe = RecordingPresence {
                fallthrough: true,
                keychain: Some(metadata),
                ..Default::default()
            };
            assert_eq!(
                detect_presence(&probe, &RecordingIdeState(&probe), Some(&agy), &ide),
                expected
            );
            assert_eq!(probe.calls.borrow().last().unwrap(), "keychain_metadata");
            assert_eq!(probe.calls.borrow().len(), 4);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detection_accepts_an_agy_binary_without_a_login_carrier() {
        let dir = tempfile::tempdir().unwrap();
        let (agy, ide) = presence_paths(dir.path());
        let probe = RecordingPresence {
            fallthrough: true,
            binary: true,
            ..Default::default()
        };
        assert_eq!(
            detected(&probe, &agy, &ide),
            Detection::InstalledNotSignedIn
        );
    }

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
        ) -> Result<Credentials, RefreshError> {
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

    struct UnstampedTransport;

    impl AntigravityTransport for UnstampedTransport {
        fn load(&self, _: &str) -> Result<HttpReply, ProviderUsageError> {
            panic!("expired credentials must refresh before cloud retrieval")
        }

        fn quota(&self, _: &str, _: &str) -> Result<HttpReply, ProviderUsageError> {
            panic!("expired credentials must refresh before cloud retrieval")
        }

        fn refresh(&self, token: &str, now: OffsetDateTime) -> Result<Credentials, RefreshError> {
            refresh_access_token(token, now, None, Some("synthetic-secret"))
        }
    }

    #[test]
    fn missing_or_blank_refresh_configuration_reports_unsupported_without_network_access() {
        let now = OffsetDateTime::from_unix_timestamp(NOW).unwrap();
        for (client_id, client_secret) in [
            (None, Some("synthetic-secret")),
            (Some("synthetic-client"), None),
            (Some("  "), Some("synthetic-secret")),
            (Some("synthetic-client"), Some("\t")),
            (None, None),
        ] {
            let failure = refresh_access_token("synthetic-refresh", now, client_id, client_secret)
                .err()
                .unwrap();
            assert_eq!(failure.error, ProviderUsageError::Authentication);
            assert_eq!(failure.detail, Some(SourceErrorDetail::RefreshUnsupported));
        }
        assert_eq!(
            refresh_client_credentials(Some("synthetic-client"), Some("synthetic-secret")),
            Ok(("synthetic-client", "synthetic-secret"))
        );
    }

    #[test]
    fn unsupported_refresh_detail_survives_failed_fallback_and_clears_on_local_success() {
        let now = OffsetDateTime::from_unix_timestamp(NOW).unwrap();
        let expired = Credentials {
            access_token: "expired-access".into(),
            refresh_token: Some("synthetic-refresh".into()),
            expires_at: Some(now - time::Duration::seconds(1)),
        };
        for local_result in [
            Ok(false),
            Err(ProviderUsageError::Unavailable),
            Err(ProviderUsageError::Authentication),
        ] {
            let cooldown = Cooldown::new();
            let cache = Mutex::default();
            let local = FakeLocal {
                calls: Arc::default(),
                result: local_result,
            };
            let outcome = cooldown.poll(now, MAX_AGE, || {
                fetch_with_refresh_fallback(
                    &UnstampedTransport,
                    &local,
                    Some(&expired),
                    &cache,
                    now,
                )
            });
            assert_eq!(outcome.error, Some(ProviderUsageError::Authentication));
            assert_eq!(outcome.detail, Some(SourceErrorDetail::RefreshUnsupported));
            let cached = cooldown.poll(now, MAX_AGE, || panic!("cooldown must skip the refresh"));
            assert_eq!(cached.detail, outcome.detail);

            cooldown.open_for_test();
            let local = FakeLocal {
                calls: Arc::default(),
                result: Ok(true),
            };
            let recovered = cooldown.poll(now, MAX_AGE, || {
                fetch_with_refresh_fallback(
                    &UnstampedTransport,
                    &local,
                    Some(&expired),
                    &cache,
                    now,
                )
            });
            assert_eq!(recovered.error, None);
            assert_eq!(recovered.detail, None);
            assert_eq!(recovered.snapshots.len(), 1);
        }
    }

    #[test]
    fn ordinary_refresh_and_local_failures_have_no_detail() {
        let now = OffsetDateTime::from_unix_timestamp(NOW).unwrap();
        let credentials = Credentials {
            refresh_token: None,
            ..credentials()
        };
        let failure =
            refresh_credentials(&UnstampedTransport, &credentials, &Mutex::default(), now)
                .err()
                .unwrap();
        assert_eq!(failure.error, ProviderUsageError::Authentication);
        assert_eq!(failure.detail, None);
        let local = FakeLocal {
            calls: Arc::default(),
            result: Err(ProviderUsageError::Unavailable),
        };
        let failure =
            fetch_with_refresh_fallback(&UnstampedTransport, &local, None, &Mutex::default(), now)
                .unwrap_err();
        assert_eq!(failure.error, ProviderUsageError::Unavailable);
        assert_eq!(failure.detail, None);
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
