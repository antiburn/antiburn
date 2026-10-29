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

use reqwest::{Client, ClientBuilder, RequestBuilder, redirect::Policy};
use serde_json::Value;

use antiburn_local::opencode_registration::MAX_REGISTRATION_BYTES;
pub use antiburn_local::opencode_registration::{
    LocalEndpoint, RegistrationError, ServiceRegistration, decode_registration,
};

const MAX_INFO_BYTES: usize = 16 * 1024;

pub fn default_registration_path() -> Result<PathBuf, RegistrationError> {
    registration_path_with_roots(
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        antiburn_local::paths::home_dir,
    )
}

pub(crate) fn registration_path_with_roots(
    state_home: Option<&std::ffi::OsStr>,
    home_dir: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf, RegistrationError> {
    if let Some(state_home) = state_home.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(state_home).join("opencode/service.json"));
    }
    let home = home_dir().ok_or(RegistrationError::Io)?;
    Ok(home.join(".local/state/opencode/service.json"))
}

pub fn read_registration(path: &Path) -> Result<ServiceRegistration, RegistrationError> {
    let file = fs::File::open(path).map_err(|_| RegistrationError::Io)?;
    let mut bytes = Vec::new();
    file.take(MAX_REGISTRATION_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| RegistrationError::Io)?;
    if bytes.len() > MAX_REGISTRATION_BYTES {
        return Err(RegistrationError::TooLarge);
    }
    decode_registration(&bytes)
}

pub(super) fn local_client() -> ClientBuilder {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder().redirect(Policy::none()).no_proxy()
}

pub(super) fn authorized_get(
    endpoint: &LocalEndpoint,
    client: &Client,
    path: &str,
) -> Result<RequestBuilder, ProbeError> {
    let url = endpoint
        .base_url()
        .join(path)
        .map_err(|_| ProbeError::Request)?;
    if !antiburn_local::opencode_registration::is_loopback_http_url(&url)
        || url.origin() != endpoint.base_url().origin()
    {
        return Err(ProbeError::Request);
    }
    let mut request = client.get(url);
    if let Some(password) = endpoint.password() {
        request = request.basic_auth("opencode", Some(password));
    }
    Ok(request)
}

pub(super) async fn read_bounded(
    mut response: reqwest::Response,
    max: usize,
) -> Result<Vec<u8>, ProbeError> {
    if response
        .content_length()
        .is_some_and(|length| length > max as u64)
    {
        return Err(ProbeError::ResponseTooLarge);
    }
    let mut bytes = Vec::with_capacity(max.min(1024));
    while let Some(chunk) = response.chunk().await.map_err(|_| ProbeError::Request)? {
        if bytes.len().saturating_add(chunk.len()) > max {
            return Err(ProbeError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod local_http_tests {
    use super::*;

    #[test]
    fn credentials_stay_on_the_registered_origin() {
        let endpoint =
            LocalEndpoint::parse("http://127.0.0.1:3456", Some("private-password".into())).unwrap();
        let client = local_client().build().unwrap();
        for path in [
            "http://127.0.0.1:7890/api/info",
            "http://example.invalid:3456/api/info",
            "//example.invalid:3456/api/info",
        ] {
            assert!(authorized_get(&endpoint, &client, path).is_err());
        }
        let request = authorized_get(&endpoint, &client, "/api/info")
            .unwrap()
            .build()
            .unwrap();
        assert!(
            request
                .headers()
                .contains_key(reqwest::header::AUTHORIZATION)
        );
        assert!(!request.url().as_str().contains("private-password"));
    }
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
    let client = local_client()
        .timeout(timeout)
        .build()
        .map_err(|_| ProbeError::Request)?;
    let response = authorized_get(&registration.endpoint, &client, "/api/info")?
        .send()
        .await
        .map_err(|_| ProbeError::Request)?;
    if response.status().is_redirection() {
        return Err(ProbeError::Redirect);
    }
    if !response.status().is_success() {
        return Err(ProbeError::HttpStatus(response.status().as_u16()));
    }
    let bytes = read_bounded(response, MAX_INFO_BYTES).await?;
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
        let before = read_registration_async(registration_path).await?;
        verify_open_file(before.pid, database_path.to_path_buf()).await?;
        let info = probe_registration(&before, timeout)
            .await
            .map_err(DatabaseAssociationError::Probe)?;
        let after = read_registration_async(registration_path).await?;
        if before != after {
            return Err(DatabaseAssociationError::RegistrationChanged);
        }
        verify_open_file(after.pid, database_path.to_path_buf()).await?;
        Ok(info)
    })
    .await
    .map_err(|_| DatabaseAssociationError::Probe(ProbeError::Request))?
}

async fn read_registration_async(
    path: &Path,
) -> Result<ServiceRegistration, DatabaseAssociationError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || read_registration(&path))
        .await
        .map_err(|_| DatabaseAssociationError::Registration(RegistrationError::Io))?
        .map_err(DatabaseAssociationError::Registration)
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
