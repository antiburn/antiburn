use crate::MetadataSections;
use serde::{Deserialize, Serialize};
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
    pub agent: String,
    pub source_format: String,
    pub native_session_id: String,
    pub native_agent_id: Option<String>,
    pub source_agent_version: Option<String>,
    pub recorded_agent_versions: Vec<String>,
    pub version_observations: Vec<VersionObservation>,
    pub version_scan_complete: bool,
    pub owned_version_complete: bool,
    pub relationship: Relationship,
    /// Native bytes are retained as found; this is not a whole-history claim.
    pub history_completeness: String,
    pub sections: MetadataSections,
}

impl CaptureMetadata {
    /// Ignore only the envelope's observation time, not evidence or data fields.
    pub fn stable_fingerprint(&self) -> Result<String> {
        let mut value: serde_json::Value =
            serde_json::from_slice(&encode_bounded(self, METADATA_BYTES)?)?;
        let sections = value["sections"]
            .as_object_mut()
            .ok_or(Error::InvalidMetadata)?;
        for section in sections.values_mut() {
            if section["observation"]["kind"] == "observed_at" {
                section["observation"]
                    .as_object_mut()
                    .ok_or(Error::InvalidMetadata)?
                    .remove("unix_millis");
            }
        }
        value.sort_all_objects();
        Ok(digest(&encode_bounded(&value, METADATA_BYTES)?))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedStream {
    pub stream_id: String,
    pub role: String,
    pub schema: String,
    pub expanded_bytes: u64,
    pub expanded_sha256: String,
    pub chunks: Vec<Chunk>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionOwnership {
    Owned,
    Inherited,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VersionObservation {
    pub version: String,
    pub ownership: VersionOwnership,
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
    pub metadata_fingerprint: String,
    pub source_fingerprint: String,
    pub streams: Vec<CapturedStream>,
}

impl CaptureManifest {
    pub fn sha256(&self) -> Result<String> {
        Ok(digest(&encode_bounded(self, RECORD_BYTES)?))
    }

    /// This content key excludes capture, account, and destination identities.
    pub fn calculate_source_fingerprint(&self) -> Result<String> {
        #[derive(Serialize)]
        struct Source<'a> {
            capture_version: &'a str,
            metadata_fingerprint: &'a str,
            streams: Vec<Stream<'a>>,
        }
        #[derive(Serialize)]
        struct Stream<'a> {
            stream_id: &'a str,
            role: &'a str,
            schema: &'a str,
            expanded_bytes: u64,
            expanded_sha256: &'a str,
        }
        let source = Source {
            capture_version: &self.capture_version,
            metadata_fingerprint: &self.metadata_fingerprint,
            streams: self
                .streams
                .iter()
                .filter(|stream| stream.role != "capture_metadata")
                .map(|stream| Stream {
                    stream_id: &stream.stream_id,
                    role: &stream.role,
                    schema: &stream.schema,
                    expanded_bytes: stream.expanded_bytes,
                    expanded_sha256: &stream.expanded_sha256,
                })
                .collect(),
        };
        Ok(digest(&encode_bounded(&source, RECORD_BYTES)?))
    }

    pub fn chunks(&self) -> impl Iterator<Item = (ChunkKey, &Chunk)> {
        self.streams.iter().flat_map(|stream| {
            stream.chunks.iter().map(move |chunk| {
                (
                    ChunkKey {
                        stream_id: stream.stream_id.clone(),
                        index: chunk.index,
                    },
                    chunk,
                )
            })
        })
    }

    pub fn chunk(&self, key: &ChunkKey) -> Option<&Chunk> {
        self.streams
            .iter()
            .find(|stream| stream.stream_id == key.stream_id)?
            .chunks
            .get(key.index as usize)
    }
}

/// The host may persist this only after integrity acceptance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedSourceFingerprint {
    pub account_id: String,
    pub destination_id: String,
    pub source_id: String,
    pub source_fingerprint: String,
}

impl AcceptedSourceFingerprint {
    /// Comparison never cancels, replaces, or releases an existing queue entry.
    pub fn matches_source(&self, manifest: &CaptureManifest) -> bool {
        self.account_id == manifest.identity.account_id
            && self.destination_id == manifest.identity.destination_id
            && self.source_id == manifest.identity.source_id
            && self.source_fingerprint == manifest.source_fingerprint
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
    pub upload_plan: Option<UploadPlan>,
    pub server_status: Option<ServerStatus>,
    pub completion_submitted: bool,
}

impl QueueEntry {
    pub fn accepted_source_fingerprint(&self) -> Option<AcceptedSourceFingerprint> {
        if self.state != CaptureState::Accepted || !self.server_status.as_ref()?.integrity_accepted
        {
            return None;
        }
        let manifest = self.manifest.as_ref()?;
        Some(AcceptedSourceFingerprint {
            account_id: self.identity.account_id.clone(),
            destination_id: self.identity.destination_id.clone(),
            source_id: self.identity.source_id.clone(),
            source_fingerprint: manifest.source_fingerprint.clone(),
        })
    }

    pub fn progress(&self) -> Result<TransferProgress> {
        crate::protocol::entry_progress(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChunkKey {
    pub stream_id: String,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkDisposition {
    Upload,
    Reuse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedChunk {
    pub key: ChunkKey,
    pub compressed_sha256: String,
    pub disposition: ChunkDisposition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadPlan {
    pub identity: CaptureIdentity,
    pub manifest_sha256: String,
    pub plan_id: String,
    pub chunks: Vec<PlannedChunk>,
    pub required_transfer_bytes: u64,
    pub reused_bytes: u64,
}

/// This receipt confirms an uploaded chunk, not a reuse decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartReceipt {
    pub key: ChunkKey,
    pub compressed_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerStatus {
    pub identity: CaptureIdentity,
    pub manifest_sha256: String,
    pub plan_id: String,
    pub uploaded_parts: Vec<PartReceipt>,
    /// True only after the service verifies and accepts the complete capture.
    pub integrity_accepted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferProgress {
    pub negotiated: bool,
    pub total_compressed_bytes: u64,
    pub required_transfer_bytes: u64,
    pub reused_bytes: u64,
    pub confirmed_upload_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeStep {
    pub entry: QueueEntry,
    pub progress: TransferProgress,
    pub uploaded_chunk: Option<ChunkKey>,
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
    ) -> std::result::Result<UploadPlan, TransportError>;

    fn status(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
        plan: &UploadPlan,
    ) -> std::result::Result<ServerStatus, TransportError>;

    /// Consume at most chunk.compressed_bytes bytes. A successful call does not
    /// replace the next server status query.
    fn upload_part(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
        plan: &UploadPlan,
        key: &ChunkKey,
        chunk: &Chunk,
        body: &mut dyn Read,
    ) -> std::result::Result<(), TransportError>;

    /// Submission, including HTTP 202, is not integrity acceptance.
    fn complete(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
        plan: &UploadPlan,
    ) -> std::result::Result<(), TransportError>;
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::ErrorKind),
    InvalidIdentity,
    InvalidSource,
    InvalidMetadata,
    SourceChanged,
    Limit,
    CorruptSpool,
    Busy,
    InterruptedCapture,
    IdentityMismatch,
    InvalidServerStatus,
    PlanChanged,
    NotAccepted,
    AlreadyAccepted,
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
