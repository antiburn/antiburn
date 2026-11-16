use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;

pub const CAPTURE_VERSION: &str = "antiburn-native-capture-v1";
pub const FIXTURE_VERSION: &str = "claude-native-capture-fixtures-v1";
pub const CHUNK_BYTES: u64 = 8 * 1024 * 1024;
pub const METADATA_BYTES: u64 = 64 * 1024;
pub const RECORD_BYTES: u64 = 512 * 1024;
pub const MAX_QUEUE_ENTRIES: usize = 512;

#[derive(Clone, Debug)]
pub struct Limits {
    pub discovery_entries: usize,
    pub discovery_sessions: usize,
    pub source_bytes: u64,
    pub capture_bytes: u64,
    pub spool_bytes: u64,
    pub queue_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            discovery_entries: 4096,
            discovery_sessions: 512,
            source_bytes: 512 * 1024 * 1024,
            capture_bytes: 1024 * 1024 * 1024,
            spool_bytes: 2 * 1024 * 1024 * 1024,
            queue_entries: MAX_QUEUE_ENTRIES,
        }
    }
}

impl Limits {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.discovery_entries == 0
            || self.discovery_entries > 16384
            || self.discovery_sessions == 0
            || self.discovery_sessions > MAX_QUEUE_ENTRIES
            || self.source_bytes == 0
            || self.source_bytes > 1024 * 1024 * 1024
            || self.capture_bytes == 0
            || self.capture_bytes > 1024 * 1024 * 1024
            || self.spool_bytes == 0
            || self.spool_bytes > 16 * 1024 * 1024 * 1024
            || self.queue_entries == 0
            || self.queue_entries > MAX_QUEUE_ENTRIES
        {
            return Err(Error::Limit);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureIdentity {
    pub capture_id: String,
    pub account_id: String,
    /// A fixed, verified service identifier. This is not a URL.
    pub destination_id: String,
    /// A device-local source key, independent of inferred repository identity.
    pub source_id: String,
}

impl CaptureIdentity {
    pub(crate) fn validate(&self) -> Result<()> {
        if !safe_id(&self.capture_id)
            || !safe_id(&self.account_id)
            || !safe_id(&self.destination_id)
            || self.source_id.is_empty()
            || self.source_id.len() > 512
            || self.source_id.chars().any(char::is_control)
        {
            return Err(Error::InvalidIdentity);
        }
        Ok(())
    }
}

pub(crate) fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Relationship {
    Unknown,
    Child {
        parent_source_id: String,
        proof: String,
    },
    Fork {
        parent_source_id: String,
        proof: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureMetadata {
    pub schema_version: u32,
    pub capture_version: String,
    pub identity: CaptureIdentity,
    pub agent: String,
    pub source_format: String,
    pub native_session_id: String,
    pub native_agent_id: Option<String>,
    pub source_agent_version: Option<String>,
    pub recorded_agent_versions: Vec<String>,
    pub version_scan_complete: bool,
    pub relationship: Relationship,
    /// Native bytes are retained as found; this is not a whole-history claim.
    pub history_completeness: String,
    pub extensions: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedFile {
    pub name: String,
    pub role: String,
    pub offset: u64,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub index: u32,
    pub expanded_offset: u64,
    pub expanded_bytes: u64,
    pub expanded_sha256: String,
    pub compressed_bytes: u64,
    pub compressed_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureManifest {
    pub capture_version: String,
    pub identity: CaptureIdentity,
    pub compression: String,
    pub chunk_expanded_bytes: u64,
    pub expanded_bytes: u64,
    pub expanded_sha256: String,
    pub files: Vec<CapturedFile>,
    pub chunks: Vec<Chunk>,
}

impl CaptureManifest {
    pub fn sha256(&self) -> Result<String> {
        Ok(digest(&encode_bounded(self, RECORD_BYTES)?))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    Preparing,
    Queued,
    AwaitingIntegrity,
    Accepted,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub identity: CaptureIdentity,
    pub state: CaptureState,
    pub reservation_id: Option<String>,
    pub manifest: Option<CaptureManifest>,
}

/// A server-confirmed part receipt, scoped by `ServerStatus` to one manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartReceipt {
    pub index: u32,
    pub compressed_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerStatus {
    pub identity: CaptureIdentity,
    pub manifest_sha256: String,
    pub parts: Vec<PartReceipt>,
    /// True only after the service verifies and accepts the complete capture.
    pub integrity_accepted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    Offline,
    Unauthorized,
    Conflict,
    Rejected,
}

/// Implement this boundary in the native host. It must bind authenticated
/// requests to the exact identity and use capture_id as the idempotency key.
/// No method may follow service-supplied upload URLs without host validation.
pub trait Transport {
    fn reserve(
        &mut self,
        identity: &CaptureIdentity,
        maximum_expanded_bytes: u64,
    ) -> std::result::Result<String, TransportError>;

    fn negotiate(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
    ) -> std::result::Result<ServerStatus, TransportError>;

    fn status(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
    ) -> std::result::Result<ServerStatus, TransportError>;

    /// Consume at most chunk.compressed_bytes bytes. A successful call does not
    /// replace the next server status query.
    fn upload_part(
        &mut self,
        reservation_id: &str,
        identity: &CaptureIdentity,
        chunk: &Chunk,
        body: &mut dyn Read,
    ) -> std::result::Result<(), TransportError>;

    /// Submission, including HTTP 202, is not integrity acceptance.
    fn complete(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
    ) -> std::result::Result<(), TransportError>;
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::ErrorKind),
    InvalidIdentity,
    InvalidSource,
    SourceChanged,
    Limit,
    CorruptSpool,
    Busy,
    InterruptedCapture,
    IdentityMismatch,
    InvalidServerStatus,
    UnsupportedPlatform,
    Transport(TransportError),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "native capture failed: {self:?}")
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.kind())
    }
}

impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::CorruptSpool
    }
}

impl From<TransportError> for Error {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(bytes))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn encode_bounded(value: &impl Serialize, maximum: u64) -> Result<Vec<u8>> {
    use std::io::Write;
    struct Output {
        bytes: Vec<u8>,
        maximum: u64,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() as u64 > self.maximum.saturating_sub(self.bytes.len() as u64) {
                return Err(std::io::Error::from(std::io::ErrorKind::StorageFull));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Output {
        bytes: Vec::new(),
        maximum,
    };
    serde_json::to_writer(&mut output, value).map_err(|_| Error::Limit)?;
    Ok(output.bytes)
}
