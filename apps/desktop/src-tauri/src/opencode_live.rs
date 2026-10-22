//! Bounded local OpenCode service discovery and SSE decoding.
//!
//! This module does not start a service or admit events for live checks. The
//! coordinator must prove that the service reads the selected database first.

#[path = "opencode_live/config.rs"]
pub(crate) mod config;
#[path = "opencode_live/stream.rs"]
pub mod stream;
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[path = "opencode_live/unix.rs"]
pub mod unix;
#[cfg(target_os = "windows")]
#[path = "opencode_live/windows_binding.rs"]
pub mod windows_binding;

use std::{
    fmt, fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use reqwest::{Client, redirect::Policy};
use serde_json::Value;
use url::Url;

const MAX_REGISTRATION_BYTES: u64 = 16 * 1024;
const MAX_INFO_BYTES: usize = 16 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct ServiceRegistration {
    pub id: Option<String>,
    pub version: Option<String>,
    pub endpoint: LocalEndpoint,
    pub pid: u32,
}

impl fmt::Debug for ServiceRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceRegistration")
            .field("id", &self.id)
            .field("version", &self.version)
            .field("endpoint", &self.endpoint)
            .field("pid", &self.pid)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct LocalEndpoint {
    base_url: Url,
    password: Option<Secret>,
}

impl fmt::Debug for LocalEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalEndpoint")
            .field("base_url", &self.base_url)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Secret(String);

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl LocalEndpoint {
    fn parse(url: &str, password: Option<String>) -> Result<Self, RegistrationError> {
        let parsed = Url::parse(url).map_err(|_| RegistrationError::InvalidEndpoint)?;
        let host_is_loopback = match parsed.host() {
            Some(url::Host::Domain(_)) => false,
            Some(url::Host::Ipv4(host)) => host.is_loopback(),
            Some(url::Host::Ipv6(host)) => host.is_loopback(),
            None => false,
        };
        if parsed.scheme() != "http"
            || !host_is_loopback
            || parsed.port().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(RegistrationError::InvalidEndpoint);
        }
        let password = password.filter(|value| !value.is_empty()).map(Secret);
        Ok(Self {
            base_url: parsed,
            password,
        })
    }

    fn info_url(&self) -> Url {
        self.base_url.join("/api/info").expect("validated base URL")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationError {
    Io,
    TooLarge,
    Malformed,
    InvalidField,
    InvalidEndpoint,
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Io => "service registration could not be read",
            Self::TooLarge => "service registration exceeds the size limit",
            Self::Malformed => "service registration is malformed",
            Self::InvalidField => "service registration has invalid fields",
            Self::InvalidEndpoint => "service endpoint is not a valid local endpoint",
        })
    }
}

impl std::error::Error for RegistrationError {}

pub fn default_registration_path() -> Result<PathBuf, RegistrationError> {
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home).join("opencode/service.json"));
    }
    let home = antiburn_local::paths::home_dir().ok_or(RegistrationError::Io)?;
    Ok(home.join(".local/state/opencode/service.json"))
}

pub fn read_registration(path: &Path) -> Result<ServiceRegistration, RegistrationError> {
    let file = fs::File::open(path).map_err(|_| RegistrationError::Io)?;
    let mut bytes = Vec::new();
    file.take(MAX_REGISTRATION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| RegistrationError::Io)?;
    if bytes.len() as u64 > MAX_REGISTRATION_BYTES {
        return Err(RegistrationError::TooLarge);
    }
    decode_registration(&bytes)
}

pub fn decode_registration(bytes: &[u8]) -> Result<ServiceRegistration, RegistrationError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| RegistrationError::Malformed)?;
    let object = value.as_object().ok_or(RegistrationError::Malformed)?;
    let url = object
        .get("url")
        .and_then(Value::as_str)
        .ok_or(RegistrationError::InvalidField)?;
    let pid = object
        .get("pid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0 && *pid <= u32::MAX as u64)
        .ok_or(RegistrationError::InvalidField)? as u32;
    let id = optional_string(object.get("id"), 256)?;
    let version = optional_string(object.get("version"), 128)?;
    let password = optional_string(object.get("password"), 4096)?;
    let endpoint = LocalEndpoint::parse(url, password)?;
    Ok(ServiceRegistration {
        id,
        version,
        endpoint,
        pid,
    })
}

fn optional_string(
    value: Option<&Value>,
    max_len: usize,
) -> Result<Option<String>, RegistrationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let text = value.as_str().ok_or(RegistrationError::InvalidField)?;
    if text.len() > max_len || text.contains('\0') {
        return Err(RegistrationError::InvalidField);
    }
    Ok(Some(text.to_owned()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    Request,
    Redirect,
    HttpStatus(u16),
    ResponseTooLarge,
    MalformedInfo,
    ProcessMismatch,
    VersionMismatch,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Request => "local OpenCode service probe failed",
            Self::Redirect => "local OpenCode service probe returned a redirect",
            Self::HttpStatus(_) => "local OpenCode service probe returned an error status",
            Self::ResponseTooLarge => "local OpenCode service info exceeds the size limit",
            Self::MalformedInfo => "local OpenCode service info is malformed",
            Self::ProcessMismatch => "local OpenCode service process does not match registration",
            Self::VersionMismatch => "local OpenCode service version does not match registration",
        })
    }
}

impl std::error::Error for ProbeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInfo {
    pub pid: u32,
    pub version: String,
}

pub async fn probe_registration(
    registration: &ServiceRegistration,
    timeout: Duration,
) -> Result<ServiceInfo, ProbeError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = Client::builder()
        .timeout(timeout)
        .redirect(Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| ProbeError::Request)?;
    let mut request = client.get(registration.endpoint.info_url());
    if let Some(password) = &registration.endpoint.password {
        request = request.basic_auth("opencode", Some(&password.0));
    }
    let mut response = request.send().await.map_err(|_| ProbeError::Request)?;
    if response.status().is_redirection() {
        return Err(ProbeError::Redirect);
    }
    if !response.status().is_success() {
        return Err(ProbeError::HttpStatus(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_INFO_BYTES as u64)
    {
        return Err(ProbeError::ResponseTooLarge);
    }
    let mut bytes = Vec::with_capacity(MAX_INFO_BYTES.min(1024));
    while let Some(chunk) = response.chunk().await.map_err(|_| ProbeError::Request)? {
        if bytes.len().saturating_add(chunk.len()) > MAX_INFO_BYTES {
            return Err(ProbeError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    decode_service_info(registration, &bytes)
}

fn decode_service_info(
    registration: &ServiceRegistration,
    bytes: &[u8],
) -> Result<ServiceInfo, ProbeError> {
    let info: Value = serde_json::from_slice(bytes).map_err(|_| ProbeError::MalformedInfo)?;
    let pid = info
        .get("pid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0 && *pid <= u32::MAX as u64)
        .ok_or(ProbeError::MalformedInfo)? as u32;
    let version = info
        .get("version")
        .and_then(Value::as_str)
        .filter(|version| !version.is_empty() && version.len() <= 128)
        .ok_or(ProbeError::MalformedInfo)?
        .to_owned();
    if pid != registration.pid {
        return Err(ProbeError::ProcessMismatch);
    }
    if registration
        .version
        .as_ref()
        .is_some_and(|expected| expected != &version)
    {
        return Err(ProbeError::VersionMismatch);
    }
    Ok(ServiceInfo { pid, version })
}

pub(crate) fn is_v2_version(version: &str) -> bool {
    let version = version.strip_prefix('v').unwrap_or(version);
    version.split('.').next() == Some("2")
}

#[derive(Debug)]
pub enum DatabaseAssociationError {
    Registration(RegistrationError),
    Probe(ProbeError),
    Binding,
    RegistrationChanged,
}

impl fmt::Display for DatabaseAssociationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registration(error) => {
                write!(
                    formatter,
                    "OpenCode service registration is unavailable: {error}"
                )
            }
            Self::Probe(error) => {
                write!(formatter, "OpenCode service identity probe failed: {error}")
            }
            Self::Binding => {
                formatter.write_str("OpenCode service is not proven to use this database")
            }
            Self::RegistrationChanged => {
                formatter.write_str("OpenCode service registration changed during verification")
            }
        }
    }
}

impl std::error::Error for DatabaseAssociationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Registration(error) => Some(error),
            Self::Probe(error) => Some(error),
            Self::Binding | Self::RegistrationChanged => None,
        }
    }
}

/// Verify that the same registered native process uses `database_path`.
///
/// This proof fails closed when platform process inspection is unavailable.
/// It does not support a Windows process that points into a WSL namespace.
pub async fn verify_database_association(
    registration_path: &Path,
    database_path: &Path,
    timeout: Duration,
) -> Result<ServiceInfo, DatabaseAssociationError> {
    tokio::time::timeout(timeout, async {
        let before =
            read_registration(registration_path).map_err(DatabaseAssociationError::Registration)?;
        verify_open_file(before.pid, database_path.to_path_buf()).await?;
        let info = probe_registration(&before, timeout)
            .await
            .map_err(DatabaseAssociationError::Probe)?;
        let after =
            read_registration(registration_path).map_err(DatabaseAssociationError::Registration)?;
        if before != after {
            return Err(DatabaseAssociationError::RegistrationChanged);
        }
        verify_open_file(after.pid, database_path.to_path_buf()).await?;
        Ok(info)
    })
    .await
    .map_err(|_| DatabaseAssociationError::Probe(ProbeError::Request))?
}

async fn verify_open_file(pid: u32, path: PathBuf) -> Result<(), DatabaseAssociationError> {
    tokio::task::spawn_blocking(move || prove_open_file(pid, &path))
        .await
        .map_err(|_| DatabaseAssociationError::Binding)?
}

#[cfg(target_os = "windows")]
fn prove_open_file(pid: u32, database_path: &Path) -> Result<(), DatabaseAssociationError> {
    windows_binding::prove_database_binding(pid, database_path)
        .map(|_| ())
        .map_err(|_| DatabaseAssociationError::Binding)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn prove_open_file(pid: u32, database_path: &Path) -> Result<(), DatabaseAssociationError> {
    unix::process_has_open_database(pid, database_path)
        .map_err(|_| DatabaseAssociationError::Binding)?
        .then_some(())
        .ok_or(DatabaseAssociationError::Binding)
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn prove_open_file(_pid: u32, _database_path: &Path) -> Result<(), DatabaseAssociationError> {
    Err(DatabaseAssociationError::Binding)
}

#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub id: Option<String>,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseError {
    FrameTooLarge,
    MalformedJson,
    Cancelled,
}

#[derive(Debug)]
pub struct SseDecoder {
    max_frame_bytes: usize,
    line: Vec<u8>,
    data: Vec<u8>,
    event: Option<String>,
    id: Option<String>,
    frame_bytes: usize,
    discarding: bool,
    discard_line_nonempty: bool,
    cancelled: bool,
    skip_lf: bool,
}

impl SseDecoder {
    pub fn new(max_frame_bytes: usize) -> Self {
        Self {
            max_frame_bytes: max_frame_bytes.max(1),
            line: Vec::new(),
            data: Vec::new(),
            event: None,
            id: None,
            frame_bytes: 0,
            discarding: false,
            discard_line_nonempty: false,
            cancelled: false,
            skip_lf: false,
        }
    }

    /// Decode at most one event and leave the remaining bytes for the caller.
    /// Callers can limit work by passing a bounded slice.
    pub fn next_event(&mut self, bytes: &mut &[u8]) -> Option<Result<SseEvent, SseError>> {
        if self.cancelled {
            return Some(Err(SseError::Cancelled));
        }
        while let Some((&byte, remaining)) = bytes.split_first() {
            *bytes = remaining;
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\n' || byte == b'\r' {
                self.skip_lf = byte == b'\r';
                if let Some(event) = self.finish_line() {
                    return Some(event);
                }
            } else if !self.discarding {
                self.frame_bytes = self.frame_bytes.saturating_add(1);
                if self.frame_bytes > self.max_frame_bytes {
                    self.discarding = true;
                    self.line.clear();
                    self.data.clear();
                    self.event = None;
                    self.id = None;
                    self.discard_line_nonempty = true;
                    return Some(Err(SseError::FrameTooLarge));
                } else {
                    self.line.push(byte);
                }
            } else {
                self.discard_line_nonempty = true;
            }
        }
        None
    }

    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.line.clear();
        self.reset_frame();
    }

    fn finish_line(&mut self) -> Option<Result<SseEvent, SseError>> {
        if self.discarding {
            if !self.discard_line_nonempty {
                self.discarding = false;
                self.reset_frame();
            } else {
                self.discard_line_nonempty = false;
            }
            return None;
        }
        if self.line.is_empty() {
            let event = if !self.data.is_empty() {
                if self.data.last() == Some(&b'\n') {
                    self.data.pop();
                }
                Some(match serde_json::from_slice(&self.data) {
                    Ok(data) => Ok(SseEvent {
                        event: self.event.take(),
                        id: self.id.take(),
                        data,
                    }),
                    Err(_) => Err(SseError::MalformedJson),
                })
            } else {
                None
            };
            self.reset_frame();
            return event;
        } else if self.line[0] != b':' {
            let separator = self.line.iter().position(|byte| *byte == b':');
            let (field, value) = match separator {
                Some(index) => (
                    &self.line[..index],
                    self.line[index + 1..]
                        .strip_prefix(b" ")
                        .unwrap_or(&self.line[index + 1..]),
                ),
                None => (&self.line[..], &[][..]),
            };
            match field {
                b"data" => {
                    self.data.extend_from_slice(value);
                    self.data.push(b'\n');
                }
                b"event" => self.event = String::from_utf8(value.to_vec()).ok(),
                b"id" if !value.contains(&0) => self.id = String::from_utf8(value.to_vec()).ok(),
                _ => {}
            }
            self.line.clear();
        } else {
            self.line.clear();
        }
        None
    }

    fn reset_frame(&mut self) {
        self.line.clear();
        self.data.clear();
        self.event = None;
        self.id = None;
        self.frame_bytes = 0;
    }
}
