//! Frozen capture descriptors, request bindings, progress, and closed failures.
//!
//! The host owns authorization and transport I/O. These values describe saved
//! work; changing a returned snapshot does not change queue state or consent.

use crate::MetadataSections;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Read;

/// Supported manifest and metadata version, not a native agent runtime version.
pub const CAPTURE_VERSION: u32 = 1;
/// Version of the public synthetic input and wire-reference package.
pub const FIXTURE_VERSION: &str = "claude-native-capture-fixtures-v1";
/// Fixed expanded size of every non-final chunk in each stream.
pub const CHUNK_BYTES: u64 = 8 * 1024 * 1024;
/// Independent expanded limit for companions and generated metadata.
pub const METADATA_BYTES: u64 = 64 * 1024;
/// Maximum serialized queue record size; admission reserves two record slots.
pub const RECORD_BYTES: u64 = 512 * 1024;
/// Maximum queue entries, including unacknowledged accepted receipts.
pub const MAX_QUEUE_ENTRIES: usize = 512;

fn deserialize_version<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version != CAPTURE_VERSION {
        return Err(serde::de::Error::custom("unsupported capture version"));
    }
    Ok(version)
}

/// Admission and execution bounds.
///
/// Discovery bounds apply to one root, not all roots selected by the host.
/// `source_bytes` and `capture_bytes` limit expanded input. `spool_bytes` bounds
/// compressed files plus two reserved record slots per entry. Keep limits large
/// enough for existing entries until they are released; lowering them can prevent
/// the spool from opening. The host must also bound source roots, selections, and
/// workers. Invalid bounds return [`Error::Limit`] when opening or discovering.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Maximum directory-tree or SQLite schema entries inspected per source.
    pub discovery_entries: usize,
    /// Maximum returned session candidates; overflow is an error, not truncation.
    pub discovery_sessions: usize,
    /// Expanded bytes per transcript or SQLite stream, including framing.
    pub source_bytes: u64,
    /// Maximum expanded bytes across native and generated metadata streams.
    pub capture_bytes: u64,
    /// Accounted disk bytes, including two reserved record slots per entry.
    pub spool_bytes: u64,
    /// Maximum entries, including accepted receipts awaiting acknowledgment.
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

/// Frozen request bindings. These values cannot change during a saved retry.
///
/// The host generates a globally unique capture ID and never reuses a terminal
/// ID, even after local acknowledgment or cancellation removes its queue entry.
/// IDs other than `source_id` contain at most 128 ASCII letters, digits, hyphens,
/// or underscores. Obtain `source_id` from a scoped [`crate::Candidate`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureIdentity {
    /// Globally unique request ID that the host never reuses after termination.
    pub capture_id: String,
    /// Verified account binding, not a display name or credential.
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

/// A directly proved source edge, not a claim of shared ownership or access.
///
/// A parent may be absent from discovery and need not have its own capture.
/// Unknown or conflicting evidence must not establish a fork.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Relationship {
    /// No accepted direct edge, including absent, conflicting, or self evidence.
    Unknown,
    /// A directly proved child edge that does not grant parent access.
    Child {
        /// Scoped parent key; the parent need not have its own capture.
        parent_source_id: String,
        /// Native evidence rule recorded by capture.
        proof: String,
    },
    /// A directly proved fork edge, separate from inherited version observations.
    Fork {
        /// Scoped source key of the directly proved fork parent.
        parent_source_id: String,
        /// Native evidence rule recorded by capture.
        proof: String,
    },
}

/// Stable metadata saved as its own stream, separate from exact native bytes.
///
/// Runtime version and relationship fields describe source evidence, not the
/// currently installed agent. Optional sections retain their observation times
/// on the wire. They must remain frozen with the native bytes during retries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureMetadata {
    /// Only integer [`CAPTURE_VERSION`] is supported during decoding.
    #[serde(deserialize_with = "deserialize_version")]
    pub version: u32,
    /// Native provider name, not the currently installed executable.
    pub agent: String,
    /// Native layout name, such as `ClaudeJsonl` or `OpenCodeSqliteV2`.
    pub source_format: String,
    /// Session identity from the native filename or selected SQLite row.
    pub native_session_id: String,
    /// Claude child filename ID; OpenCode personas leave this absent.
    pub native_agent_id: Option<String>,
    /// Historical owned runtime version, absent when evidence cannot prove one.
    pub source_agent_version: Option<String>,
    /// Bounded Claude version/ownership pairs; OpenCode leaves this empty.
    pub version_observations: Vec<VersionObservation>,
    /// Whether the Claude scan avoided partial records and invalid versions.
    pub version_scan_complete: bool,
    /// Complete owned-version evidence; agreement is checked separately.
    pub owned_version_complete: bool,
    /// Direct native relationship evidence, not permission or parent availability.
    pub relationship: Relationship,
    /// Native bytes are retained as found; this is not a whole-history claim.
    pub history_completeness: String,
    /// Namespaced, evidence-bound producer claims frozen with this capture.
    pub sections: MetadataSections,
}

impl CaptureMetadata {
    /// Hash sorted-key metadata after removing only `ObservedAt` envelope times.
    ///
    /// Evidence, unknown fields, and data fields named `unix_millis` remain in the
    /// projection. This computes a comparison key; it does not validate producer
    /// claims or authorize collection. Oversized serialization returns [`Error::Limit`].
    pub fn stable_fingerprint(&self) -> Result<String> {
        let mut value: serde_json::Value =
            crate::metadata::decode_metadata_value(&encode_bounded(self, METADATA_BYTES)?)?;
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

/// One ordered stream with its own offsets, digest, and independent gzip chunks.
///
/// An empty native stream has no chunks and the digest of the empty byte string.
/// Stream IDs and schemas describe framing; they are never extraction paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedStream {
    /// Stable stream slot, never an extraction path.
    pub stream_id: String,
    /// Framing role checked against the complete provider layout.
    pub role: String,
    /// Schema identifier for the native or generated stream framing.
    pub schema: String,
    /// Full expanded stream length in bytes.
    pub expanded_bytes: u64,
    /// Lowercase SHA-256 of the full expanded stream.
    pub expanded_sha256: String,
    /// Consecutive stream-local chunks; empty only for an empty native file.
    pub chunks: Vec<Chunk>,
}

/// Ownership of a Claude versioned record relative to its filename identity.
///
/// `Inherited` means another logical owner, not proof of a fork relationship.
/// `Unknown` cannot establish the captured source's historical runtime version.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionOwnership {
    /// Record ownership matches the captured Claude filename identity.
    Owned,
    /// Record names another logical owner; this does not prove a fork.
    Inherited,
    /// Source fields cannot prove record ownership.
    Unknown,
}

/// A bounded observed version and its ownership, not whole-history evidence.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VersionObservation {
    /// Recorded source string, not an installed or inferred version.
    pub version: String,
    /// Logical owner classification relative to the captured source.
    pub ownership: VersionOwnership,
}

/// One gzip member. Indices are local to its stream.
///
/// Every non-final chunk expands to [`CHUNK_BYTES`]. Resume uses the saved
/// compressed body and digest; an upgrade must not recompress a pending chunk.
/// After layout validation, its expanded offset is `index * CHUNK_BYTES`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    /// Zero-based index within this stream, not a multipart part number.
    pub index: u32,
    /// Expanded member length; non-final members must equal [`CHUNK_BYTES`].
    pub expanded_bytes: u64,
    /// Lowercase SHA-256 of this member's exact expanded bytes.
    pub expanded_sha256: String,
    /// Saved gzip body length in bytes, used for transfer and reuse totals.
    pub compressed_bytes: u64,
    /// Lowercase SHA-256 of the exact saved gzip body.
    pub compressed_sha256: String,
}

/// Frozen stream descriptors and request identity submitted for one capture.
///
/// The manifest digest binds actual metadata bytes, including observation times.
/// The source fingerprint instead binds native descriptors and stable metadata.
/// A consumer must verify expanded metadata and each wire digest separately.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureManifest {
    /// Only integer [`CAPTURE_VERSION`] is supported during decoding.
    #[serde(deserialize_with = "deserialize_version")]
    pub version: u32,
    /// Frozen request, account, destination, and local source binding.
    pub identity: CaptureIdentity,
    /// Deterministic gzip implementation identifier, not a negotiated codec.
    pub compression: String,
    /// Fixed expanded member size used to derive stream-local offsets.
    pub chunk_expanded_bytes: u64,
    /// Stable metadata digest that excludes only observation-envelope timestamps.
    pub metadata_fingerprint: String,
    /// Native descriptors plus stable metadata, excluding request identity.
    pub source_fingerprint: String,
    /// Complete ordered provider layout, including generated metadata last.
    pub streams: Vec<CapturedStream>,
}

impl CaptureManifest {
    /// Hash compact JSON in declared field order without validating its contents.
    pub fn sha256(&self) -> Result<String> {
        Ok(digest(&encode_bounded(self, RECORD_BYTES)?))
    }

    /// This content key excludes capture, account, and destination identities.
    ///
    /// It includes ordered native stream descriptors and `metadata_fingerprint`,
    /// not compression details or the physical metadata stream descriptor.
    /// Computing this hash does not verify the descriptors against saved bytes.
    pub fn calculate_source_fingerprint(&self) -> Result<String> {
        #[derive(Serialize)]
        struct Source<'a> {
            version: u32,
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
            version: self.version,
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

    /// Visit chunks in stream order, then in each stream's saved chunk order.
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

    /// Look up a stream-local chunk in a valid, ordered manifest.
    pub fn chunk(&self, key: &ChunkKey) -> Option<&Chunk> {
        self.streams
            .iter()
            .find(|stream| stream.stream_id == key.stream_id)?
            .chunks
            .get(key.index as usize)
    }
}

/// A host-owned unchanged-source key saved only after integrity acceptance.
///
/// Keep this with the terminal request state before acknowledging the queue
/// receipt. A match does not prove current remote availability, current consent,
/// or permission to discard another pending capture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedSourceFingerprint {
    /// Verified account that accepted this source.
    pub account_id: String,
    /// Fixed accepted destination, not a current default.
    pub destination_id: String,
    /// Device-local scoped source key bound by the accepted request.
    pub source_id: String,
    /// Content comparison key from the integrity-accepted manifest.
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

/// Durable queue states. Only acceptance or explicit cancellation releases bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    /// Desktop capture did not finish. Cancel explicitly before using a new ID.
    Preparing,
    /// Saved source is complete. Reconcile and upload from the spool only.
    Queued,
    /// Completion intent is saved. Submission may need its same idempotent retry.
    AwaitingIntegrity,
    /// Integrity is accepted. Keep the receipt until the host acknowledges it.
    Accepted,
    /// Explicit cancellation is saved. Startup can finish local deletion.
    Cancelled,
}

/// A queue snapshot, not an editable storage interface.
///
/// [`crate::Spool`] owns all record writes. Editing a returned value does not
/// update the queue. The manifest, plan, server status, and completion intent
/// survive process exit; the host retains consent and terminal request state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    /// Immutable bindings shared by reservation, manifest, plan, and status.
    pub identity: CaptureIdentity,
    /// Last durable lifecycle state, not inferred from transfer totals.
    pub state: CaptureState,
    /// Fixed server reservation, absent before reservation succeeds.
    pub reservation_id: Option<String>,
    /// Frozen descriptors, present only after all stream bytes are saved.
    pub manifest: Option<CaptureManifest>,
    /// First validated negotiation, kept unchanged through all later turns.
    pub upload_plan: Option<UploadPlan>,
    /// Last validated status. Provisional parts can expire before acceptance.
    pub server_status: Option<ServerStatus>,
    /// Completion returned success; false can still accompany saved intent.
    pub completion_submitted: bool,
}

impl QueueEntry {
    /// Return the bound comparison key only for an integrity-accepted snapshot.
    ///
    /// Persist it with the terminal request before removing the retained receipt
    /// through [`crate::Spool::acknowledge_accepted`].
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

    /// Calculate negotiated totals and server-confirmed uploads, not acceptance.
    pub fn progress(&self) -> Result<TransferProgress> {
        crate::protocol::entry_progress(self)
    }
}

/// A stream-local chunk address, not a filesystem path or multipart part number.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChunkKey {
    /// Validated manifest stream slot.
    pub stream_id: String,
    /// Zero-based chunk index local to that stream.
    pub index: u32,
}

/// The frozen transfer decision. Reused chunks still remain local until acceptance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkDisposition {
    /// Transfer the saved body and obtain service confirmation of stored transfer.
    Upload,
    /// Use the service's protected base object but keep local retry bytes.
    Reuse,
}

/// A plan decision bound to the manifest chunk's exact compressed digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedChunk {
    /// Exact stream-local chunk selected by this decision.
    pub key: ChunkKey,
    /// Registered expected saved-body digest; must equal the manifest chunk digest.
    pub compressed_sha256: String,
    /// Frozen transfer or reuse choice, not a confirmation.
    pub disposition: ChunkDisposition,
}

/// An immutable server decision for every manifest chunk, in manifest order.
///
/// Required and reused byte sums count compressed bodies. A reuse decision is
/// not an upload receipt and does not permit early removal of local bytes.
/// Chunk digests bind expected bodies and later receipts to this plan. The plan
/// does not prove storage or byte integrity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadPlan {
    /// Exact request bindings from the manifest.
    pub identity: CaptureIdentity,
    /// Digest of compact manifest JSON in declared field order.
    pub manifest_sha256: String,
    /// Fixed service plan identifier, not an upload URL.
    pub plan_id: String,
    /// One decision per manifest chunk in the same order.
    pub chunks: Vec<PlannedChunk>,
    /// Sum of compressed bytes for `Upload` decisions only.
    pub required_transfer_bytes: u64,
    /// Sum of compressed bytes for `Reuse` decisions only.
    pub reused_bytes: u64,
}

/// Service confirmation of a stored part for an uploaded chunk's bound request.
///
/// The enclosing [`ServerStatus`] supplies the identity, manifest, and plan
/// binding. This receipt confirms transfer, not complete byte integrity, and
/// never permits spool release by itself. Reuse decisions have no upload receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartReceipt {
    /// Uploaded chunk, never a reuse-only decision.
    pub key: ChunkKey,
    /// Registered expected saved-body digest that identifies the acknowledged chunk.
    ///
    /// This must equal the manifest and frozen plan digest. It is a request
    /// binding, not a claim that storage returned or independently verified SHA-256.
    /// Never substitute an ETag or invent a verified checksum when none exists.
    pub compressed_sha256: String,
}

/// Authoritative status bound to the exact identity, manifest, and frozen plan.
///
/// Provisional receipts can disappear after storage expiry. Retry those chunks
/// from saved bytes under the same plan. Acceptance cannot regress and requires
/// receipts for every `Upload` decision; `Reuse` decisions must not appear here.
/// Receipt digest matches establish expected-request bindings, not full byte
/// integrity. Only `integrity_accepted` confirms complete capture verification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerStatus {
    /// Exact account, destination, source, and capture binding.
    pub identity: CaptureIdentity,
    /// Exact manifest digest accepted by negotiation.
    pub manifest_sha256: String,
    /// Identifier of the unchanged saved plan.
    pub plan_id: String,
    /// Stored-transfer confirmations; duplicates and reuse receipts are invalid.
    pub uploaded_parts: Vec<PartReceipt>,
    /// True only after the service reads and verifies the complete stored capture.
    ///
    /// Verification covers compressed and expanded chunk lengths and digests,
    /// stream lengths and digests, and manifest and metadata bindings, including
    /// reused chunks. Stored-part receipts or optional storage checksum metadata
    /// cannot replace these checks. Only this acceptance permits automatic release
    /// of local retry bytes.
    pub integrity_accepted: bool,
}

/// Compressed-byte progress. Sent bytes alone never advance confirmed uploads.
///
/// Before negotiation, only `total_compressed_bytes` is known. These counters do
/// not prove that the service has accepted or published the captured source.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferProgress {
    /// Whether the required/reused split is known from a validated plan.
    pub negotiated: bool,
    /// All saved compressed bytes, including reused bodies.
    pub total_compressed_bytes: u64,
    /// Compressed bytes the frozen plan requires transferring.
    pub required_transfer_bytes: u64,
    /// Compressed bytes the frozen plan reuses without upload.
    pub reused_bytes: u64,
    /// Required upload bytes backed by validated service receipts.
    pub confirmed_upload_bytes: u64,
}

/// One scheduler turn and its saved queue snapshot.
///
/// `uploaded_chunk` identifies the one body sent this turn, if any. `None` can
/// mean completion submission or polling, not that the capture is accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeStep {
    /// Durable snapshot after this turn, including any retained accepted receipt.
    pub entry: QueueEntry,
    /// Counters derived from this snapshot, not a separate acceptance signal.
    pub progress: TransferProgress,
    /// The one body sent and confirmed this turn, absent for polling/completion.
    pub uploaded_chunk: Option<ChunkKey>,
}

/// Closed transport failures safe to expose without raw service diagnostics.
///
/// An error does not prove that the remote operation had no effect. Reconcile
/// saved captures under the same identity after restoring current authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// The service asks the caller to retry the same operation later. A reserve
    /// implementation must prove that capacity refusal created no reservation.
    /// A generic HTTP 429 or unknown delivery does not establish this proof.
    Backpressure,
    /// Network, timeout, or body I/O failed; remote success can remain unknown.
    Offline,
    /// Current authentication or consent does not permit the fixed request.
    Unauthorized,
    /// The service fenced the saved request, reservation, or plan.
    Conflict,
    /// The service rejected the fixed request without exposing raw diagnostics.
    Rejected,
}

/// Connect [`crate::Spool`] to host-owned service I/O without adding a network
/// client to capture or giving renderer code control of upload requests.
///
/// The host supplies authentication, current consent checks, endpoint and
/// redirect validation, bounded response decoding, timeouts, and scheduling.
/// Bind every call to its exact account, destination, source, capture,
/// reservation, manifest, plan, and chunk arguments where present. Preserve
/// service upload identifiers across retries; do not derive them from paths.
///
/// Calls are synchronous and ordered within one spool executor. The host must
/// run them off the UI thread and bound concurrent work across other spools.
/// Return closed [`TransportError`] values for service, body-read, or network
/// failures. Never include content, credentials, URLs, or raw errors in
/// diagnostics. Errors can follow remote success, so retry and status must be
/// safe under the same frozen inputs.
pub trait Transport {
    /// Reserve the fixed identity before full desktop capture reads source content.
    ///
    /// A separate, authorized local project-evidence scan can read bounded native
    /// transcript content before reservation. That scan never spools or uploads
    /// content and does not grant ongoing full-content upload consent.
    ///
    /// `maximum_expanded_bytes` includes conservative generated-metadata space.
    /// Use the capture ID for reservation idempotency. Return a reservation ID
    /// with 1 through 128 ASCII letters, digits, hyphens, or underscores.
    /// Failure leaves a `Preparing` entry. Only a proved [`TransportError::Backpressure`]
    /// refusal can use [`crate::Spool::retry_reservation`] under the same capture ID.
    /// The host must recheck current authorization and unchanged source observation,
    /// then durably clear its saved capacity proof before reservation I/O.
    /// The spool requires a present empty entry with no reservation or partial files.
    /// Missing or quarantined entries cannot retry. Unknown delivery and other
    /// failures cannot automatically read a later source generation.
    fn reserve(
        &mut self,
        identity: &CaptureIdentity,
        maximum_expanded_bytes: u64,
    ) -> std::result::Result<String, TransportError>;

    /// Return one ordered decision per manifest chunk and exact compressed sums.
    ///
    /// Repeated calls, including after completion submission, must return the
    /// same plan ID, decisions, hashes, and totals. The spool saves and validates
    /// this plan before the first upload. Refreshing credentials must not change
    /// it or move the capture to a new destination.
    fn negotiate(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
    ) -> std::result::Result<UploadPlan, TransportError>;

    /// Read service-confirmed stored transfers and acceptance for the frozen request.
    ///
    /// Do not infer confirmation from bytes sent or a successful upload response.
    /// Receipts cover only current `Upload` parts and can disappear before
    /// acceptance when incomplete storage expires. Their digests
    /// bind acknowledgments to registered expected chunks; they do not establish
    /// full byte integrity or require storage-provided SHA-256 metadata. Report
    /// integrity acceptance only after reading and verifying all stored bytes,
    /// compressed and expanded chunks, streams, and metadata, including reused
    /// chunks. Never treat an ETag as SHA-256 or report an unverified checksum as
    /// verified.
    fn status(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
        plan: &UploadPlan,
    ) -> std::result::Result<ServerStatus, TransportError>;

    /// Send this exact saved gzip body for the supplied stream-local chunk key.
    ///
    /// Send `chunk.compressed_bytes` bytes without recompression, repair, or
    /// concatenation. Treat a short read or send failure as an error. Bind the
    /// upload location to this reservation, manifest, plan, and key, and validate
    /// service-supplied URLs and redirects before use. A chunk index is not a
    /// multipart part number.
    ///
    /// `Ok(())` does not confirm upload. The spool queries status afterward, and
    /// a lost response must remain safe to reconcile without changing the body.
    fn upload_part(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
        plan: &UploadPlan,
        key: &ChunkKey,
        chunk: &Chunk,
        body: &mut dyn Read,
    ) -> std::result::Result<(), TransportError>;

    /// Submit completion only after all required uploads have confirmed receipts.
    ///
    /// The spool saves intent first. A failed or lost response may repeat this
    /// request with the same capture ID, manifest, and plan; make it idempotent.
    /// After `Ok(())`, the spool polls status instead of submitting again.
    /// Submission, including HTTP 202, is not integrity acceptance.
    fn complete(
        &mut self,
        reservation_id: &str,
        manifest: &CaptureManifest,
        plan: &UploadPlan,
    ) -> std::result::Result<(), TransportError>;
}

/// Capture failures without native content, paths, or raw provider diagnostics.
///
/// Errors do not cancel entries or authorize recapture. Inspect the saved state:
/// `Preparing` requires explicit cancellation unless a proved capacity refusal
/// meets [`crate::Spool::retry_reservation`]'s empty-entry and host proof rules.
/// A queued retry keeps its identity, metadata, plan, and saved bodies. Corruption
/// can block the whole spool and needs operator recovery rather than deletion.
#[derive(Debug)]
pub enum Error {
    /// Filesystem error kind without the original path or error message.
    Io(std::io::ErrorKind),
    /// A request ID or source binding has invalid syntax or size.
    InvalidIdentity,
    /// The source or filesystem admission cannot establish a safe valid input.
    InvalidSource,
    /// The source database uses unsupported or unsafe schema or ownership rules.
    UnsupportedSource,
    /// A metadata envelope, evidence digest, or source position is invalid.
    InvalidMetadata,
    /// A pinned file generation or admitted database binding changed.
    SourceChanged,
    /// A configured byte, count, size, or execution bound was reached.
    Limit,
    /// Queue or disk capacity refused admission before publication or reservation.
    /// A host may retry a new entry under the same ID after checking current
    /// consent and source evidence. This does not authorize partial-capture retry.
    SpoolCapacity,
    /// A saved record, private file, layout, or selected chunk fails validation.
    CorruptSpool,
    /// Another spool executor or a SQLite lock prevents current work.
    Busy,
    /// A preparing capture cannot resume native reads under the same ID.
    InterruptedCapture,
    /// A caller supplied different frozen request bindings.
    IdentityMismatch,
    /// Negotiation or status violates bindings, totals, or receipt rules.
    InvalidServerStatus,
    /// Negotiation differs from the first saved plan.
    PlanChanged,
    /// Acknowledgment requires an integrity-accepted receipt.
    NotAccepted,
    /// Accepted work cannot be cancelled; the host must acknowledge its receipt.
    AlreadyAccepted,
    /// Desktop capture and private spooling require a supported Unix host.
    UnsupportedPlatform,
    /// The saved manifest needs its original build; no migration is attempted.
    UnsupportedFormat,
    /// Closed transport failure; saved retry bytes remain local.
    Transport(TransportError),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if matches!(self, Self::UnsupportedFormat) {
            return formatter.write_str(
                "desktop capture failed: unsupported capture format; use the original build to drain or cancel, or preserve a private copy for recovery",
            );
        }
        write!(formatter, "desktop capture failed: {self:?}")
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

/// A capture result that never carries native content or raw service errors.
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex(&Sha256::digest(bytes))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Serialize compact JSON in declared field order without exceeding its byte cap.
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
