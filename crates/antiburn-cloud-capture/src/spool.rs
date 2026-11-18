//! Private durable capture and one-chunk upload reconciliation.
//!
//! The host owns consent and scheduling. Run filesystem, SQLite, compression, and
//! transport work on bounded blocking workers, not UI or async runtime threads.

use crate::contract::{encode_bounded, hex, safe_id};
use crate::directory::Directory;
use crate::discovery::{CandidateSource, ClaudeSource};
use crate::filesystem::same_source;
use crate::protocol::{
    stream_spec, validate_manifest, validate_plan, validate_status, validate_status_progress,
};
use crate::stream::{ChunkStream, NativeMetadata, chunk_name};
use crate::*;
use sha2::{Digest, Sha256};
use std::fs::{File, Metadata};
use std::io::{Read, Seek};
use std::path::Path;

const MAX_DIRECTORY_FILES: usize = 132;

/// Retain file pins across reservation; SQLite payload opens only afterward.
enum PreparedSource<'a> {
    Claude {
        source: &'a ClaudeSource,
        files: Vec<File>,
        pinned: Vec<Metadata>,
    },
    HeaderFile {
        file: File,
        pinned: Metadata,
    },
    #[cfg(unix)]
    OpenCode(&'a crate::opencode::OpenCodeSource),
}

/// Private saved bodies and durable queue records owned by one executor.
///
/// The desktop host owns consent, credentials, request selection, and terminal
/// state. This value owns the exclusive spool lock and all queue file changes.
/// Drop it to stop execution; dropping it never cancels captures or deletes
/// native bytes. Run its blocking methods on a bounded blocking worker, never on
/// UI or async runtime threads. Other platforms return [`Error::UnsupportedPlatform`].
pub struct Spool {
    root: Directory,
    limits: Limits,
    _lock: File,
}

impl Spool {
    /// Create or reopen a private Unix spool and acquire its exclusive lock.
    ///
    /// `root` must be an absolute path with no symlink components and an existing
    /// parent. Directories must belong to the effective user with mode `0700`;
    /// files require mode `0600` and one hard link. Another executor returns
    /// [`Error::Busy`].
    ///
    /// Opening finishes interrupted publication and terminal deletion. It also
    /// recovers a validated accepted status saved with an older pending state,
    /// retains the accepted receipt, and releases its chunks. It does not resume
    /// native reads or contact a transport.
    ///
    /// One corrupt record can block the whole spool. Preserve its private files
    /// for operator recovery; do not delete entries to make opening succeed.
    /// Restore prior limits if lower limits no longer admit saved entries.
    /// An unsupported saved manifest returns [`Error::UnsupportedFormat`] without
    /// rewriting it or releasing its chunks. Use its original build to drain or
    /// explicitly cancel; never change version fields or recompress a retry.
    pub fn open(root: &Path, limits: Limits) -> Result<Self> {
        limits.validate()?;
        let root = Directory::open(root)?;
        let lock = match root.file("queue.lock", true) {
            Ok(file) => file,
            Err(Error::Io(std::io::ErrorKind::AlreadyExists)) => root.file("queue.lock", false)?,
            Err(error) => return Err(error),
        };
        lock.try_lock().map_err(|_| Error::Busy)?;
        let spool = Self {
            root,
            limits,
            _lock: lock,
        };
        spool.recover_terminal_entries()?;
        spool.usage()?;
        Ok(spool)
    }

    /// Read bounded queue snapshots ordered by capture ID, including receipts.
    ///
    /// This does not read chunk bodies or contact the service. The host must
    /// reconcile pending entries and acknowledge accepted ones separately.
    pub fn entries(&self) -> Result<Vec<QueueEntry>> {
        let mut entries = Vec::new();
        for directory in self.directories()? {
            entries.push(self.read_entry(&directory)?);
        }
        entries.sort_by(|left, right| left.identity.capture_id.cmp(&right.identity.capture_id));
        Ok(entries)
    }

    /// The host must obtain consent and bind this source to the destination
    /// before calling. This full capture reads no native content until reserve
    /// succeeds. Separately authorized local project-evidence scans can read
    /// bounded transcript content before capture; they never upload it.
    ///
    /// Use a scoped candidate and a fresh, globally unique capture ID. Repeating
    /// an existing identity returns its saved entry without reading new source
    /// bytes or replacing metadata. Different bindings return
    /// [`Error::IdentityMismatch`].
    ///
    /// Admission errors can occur before publication. After a `Preparing` entry
    /// is published, a reservation, read, mutation, or metadata failure leaves it
    /// interrupted. Inspect [`Self::entries`] after failure. Explicitly cancel
    /// that entry before capturing a later generation with a new ID.
    ///
    /// Successful capture freezes exact native streams and typed metadata on
    /// disk. Upload resume never reopens the source.
    pub fn capture(
        &mut self,
        identity: CaptureIdentity,
        candidate: &Candidate,
        sections: MetadataSections,
        transport: &mut impl Transport,
    ) -> Result<QueueEntry> {
        identity.validate()?;
        if identity.source_id != candidate.source_id {
            return Err(Error::IdentityMismatch);
        }
        let name = identity.capture_id.clone();
        match self.root.open_dir(&name) {
            Ok(directory) => {
                let existing = self.read_directory_entry(&directory, &name)?;
                if existing.identity != identity {
                    return Err(Error::IdentityMismatch);
                }
                // A repeated capture call must not read a newer source generation.
                return Ok(existing);
            }
            Err(Error::Io(std::io::ErrorKind::NotFound)) => {}
            Err(error) => return Err(error),
        }
        self.capture_source(identity, candidate, sections, transport, None)
    }

    /// Retry a proved capacity refusal under the same identity, without removing
    /// the entry or replacing any saved bytes.
    ///
    /// The host must durably record a definitive reservation backpressure result,
    /// clear that retry permission before this call, and recheck current consent,
    /// source identity, source observation, and project admission. Unknown prior
    /// errors do not permit this call. A crash after retry starts requires normal
    /// interrupted-capture recovery, not another inferred capacity retry.
    ///
    /// Only a present `Preparing` entry with no reservation, manifest, plan,
    /// status, completion, or files beyond `record.json` can retry. Missing,
    /// quarantined, reserved, and partial entries fail without changing bytes.
    pub fn retry_reservation(
        &mut self,
        identity: CaptureIdentity,
        candidate: &Candidate,
        sections: MetadataSections,
        transport: &mut impl Transport,
    ) -> Result<QueueEntry> {
        identity.validate()?;
        if identity.source_id != candidate.source_id {
            return Err(Error::IdentityMismatch);
        }
        let directory = self.root.open_dir(&identity.capture_id)?;
        let entry = self.read_directory_entry(&directory, &identity.capture_id)?;
        if entry.identity != identity {
            return Err(Error::IdentityMismatch);
        }
        if entry.state != CaptureState::Preparing
            || entry.reservation_id.is_some()
            || entry.manifest.is_some()
            || entry.upload_plan.is_some()
            || entry.server_status.is_some()
            || entry.completion_submitted
            || directory.names(2)? != ["record.json"]
        {
            return Err(Error::InterruptedCapture);
        }
        self.capture_source(identity, candidate, sections, transport, Some(directory))
    }

    /// Prepare bounded source reads and reserve before saving native bytes.
    /// A supplied directory has already passed the empty-reservation retry checks.
    fn capture_source(
        &mut self,
        identity: CaptureIdentity,
        candidate: &Candidate,
        sections: MetadataSections,
        transport: &mut impl Transport,
        directory: Option<Directory>,
    ) -> Result<QueueEntry> {
        validate_sections(&sections)?;
        if let Some(selection) = &candidate.project_selection {
            selection.verify_directories()?;
        }
        let (prepared, native_bytes, native_streams) = match &candidate.source {
            CandidateSource::Claude(source) => {
                let mut files = vec![source.directory.source_file(&source.filename)?];
                if let Some(companion) = &source.companion {
                    files.push(source.directory.source_file(companion)?);
                }
                let mut native_bytes = 0u64;
                let mut pinned = Vec::new();
                for (index, file) in files.iter().enumerate() {
                    let metadata = file.metadata()?;
                    if !same_source(&metadata, &metadata) {
                        return Err(Error::InvalidSource);
                    }
                    let maximum = if index > 0 {
                        METADATA_BYTES
                    } else {
                        self.limits.source_bytes
                    };
                    if metadata.len() > maximum {
                        return Err(Error::Limit);
                    }
                    native_bytes = native_bytes
                        .checked_add(metadata.len())
                        .ok_or(Error::Limit)?;
                    pinned.push(metadata);
                }
                let count = files.len();
                (
                    PreparedSource::Claude {
                        source,
                        files,
                        pinned,
                    },
                    native_bytes,
                    count,
                )
            }
            CandidateSource::Codex(source) | CandidateSource::Pi(source) => {
                let (file, pinned) = source.pin(&self.limits)?;
                let size = pinned.len();
                (PreparedSource::HeaderFile { file, pinned }, size, 1)
            }
            #[cfg(unix)]
            CandidateSource::OpenCode(source) => (
                PreparedSource::OpenCode(source),
                self.limits.source_bytes,
                1,
            ),
        };
        let maximum_expanded = native_bytes
            .checked_add(METADATA_BYTES)
            .ok_or(Error::Limit)?;
        if maximum_expanded > self.limits.capture_bytes {
            return Err(Error::Limit);
        }
        let (used, count) = self.usage()?;
        let maximum_compressed =
            maximum_expanded + native_bytes / 100 + native_streams as u64 * 1024 + 2048;
        let new_entry = directory.is_none();
        let disk_reservation = maximum_compressed + if new_entry { 2 * RECORD_BYTES } else { 0 };
        if (new_entry && count >= self.limits.queue_entries)
            || disk_reservation > self.limits.spool_bytes.saturating_sub(used)
        {
            return Err(Error::SpoolCapacity);
        }
        let mut entry = QueueEntry {
            identity,
            state: CaptureState::Preparing,
            reservation_id: None,
            manifest: None,
            upload_plan: None,
            server_status: None,
            completion_submitted: false,
        };
        let directory = match directory {
            Some(directory) => directory,
            None => self.create_entry(&entry.identity.capture_id, &entry)?,
        };
        let reservation_id = transport.reserve(&entry.identity, maximum_expanded)?;
        if !safe_id(&reservation_id) {
            return Err(Error::InvalidServerStatus);
        }
        entry.reservation_id = Some(reservation_id);
        self.save(&directory, &entry)?;

        let (mut streams, metadata) = match prepared {
            PreparedSource::Claude {
                source,
                files,
                pinned,
            } => capture_claude(&directory, candidate, source, files, pinned, sections)?,
            PreparedSource::HeaderFile { file, pinned } => {
                capture_header_file(&directory, candidate, file, pinned, sections)?
            }
            #[cfg(unix)]
            PreparedSource::OpenCode(source) => {
                capture_opencode(&directory, candidate, source, &self.limits, sections)?
            }
        };
        validate_section_bindings(&metadata.sections, &streams)?;
        let metadata_fingerprint = metadata.stable_fingerprint()?;
        let metadata = encode_bounded(&metadata, METADATA_BYTES)?;
        let stream_count = streams.len() + 1;
        let (id, role, schema) =
            stream_spec(candidate.source_kind(), stream_count - 1, stream_count);
        let mut stream = ChunkStream::new(&directory, id, METADATA_BYTES + 2048);
        stream.write(&metadata)?;
        streams.push(stream.finish(role, schema)?);
        let mut manifest = CaptureManifest {
            version: CAPTURE_VERSION,
            identity: entry.identity.clone(),
            compression: "gzip-mtime0-os255-level6".into(),
            chunk_expanded_bytes: CHUNK_BYTES,
            metadata_fingerprint,
            source_fingerprint: String::new(),
            streams,
        };
        manifest.source_fingerprint = manifest.calculate_source_fingerprint()?;
        validate_manifest(&manifest, &self.limits)?;
        entry.manifest = Some(manifest);
        entry.state = CaptureState::Queued;
        self.save(&directory, &entry)?;
        Ok(entry)
    }

    /// Drain saved parts synchronously. Schedulers should use resume_step.
    ///
    /// Return after acceptance, completion submission, a polling turn without
    /// an upload, or one bounded part pass plus a completion turn. This helper
    /// is not a fair scheduler or a loop that waits for
    /// integrity acceptance. Errors retain the same retry identity and bodies.
    pub fn resume(
        &mut self,
        identity: &CaptureIdentity,
        transport: &mut impl Transport,
    ) -> Result<QueueEntry> {
        let mut turns = 0usize;
        loop {
            let step = self.resume_step(identity, transport)?;
            turns += 1;
            let maximum_turns = step
                .entry
                .upload_plan
                .as_ref()
                .map_or(1, |plan| plan.chunks.len().saturating_add(1));
            if step.uploaded_chunk.is_none()
                || turns >= maximum_turns
                || matches!(
                    step.entry.state,
                    CaptureState::Accepted | CaptureState::Cancelled
                )
            {
                return Ok(step.entry);
            }
        }
    }

    /// Reconcile and send at most one independently compressed saved chunk.
    ///
    /// A turn negotiates only when no plan is saved, validates the saved plan,
    /// reconciles authoritative status, and hashes at most the selected saved
    /// chunk before sending it. Completion uses a later turn after all required
    /// upload receipts exist. Poll again while the returned state is
    /// `AwaitingIntegrity`. Unchanged status does not rewrite the queue record.
    ///
    /// The caller must recheck current consent and authorization before each
    /// turn. Errors never authorize a new identity, destination, or body.
    /// `Preparing` returns [`Error::InterruptedCapture`]. Acceptance saves state
    /// and verified status together before releasing chunks; its receipt remains
    /// until [`Self::acknowledge_accepted`].
    pub fn resume_step(
        &mut self,
        identity: &CaptureIdentity,
        transport: &mut impl Transport,
    ) -> Result<ResumeStep> {
        identity.validate()?;
        let name = &identity.capture_id;
        let directory = self.root.open_dir(name)?;
        let mut entry = self.read_directory_entry(&directory, name)?;
        if entry.identity != *identity {
            return Err(Error::IdentityMismatch);
        }
        if entry.state == CaptureState::Preparing {
            return Err(Error::InterruptedCapture);
        }
        if matches!(
            entry.state,
            CaptureState::Accepted | CaptureState::Cancelled
        ) {
            if entry.state == CaptureState::Accepted {
                self.release_accepted_bytes(name)?;
            }
            return step_result(entry, None);
        }
        let manifest = entry.manifest.clone().ok_or(Error::CorruptSpool)?;
        let reservation = entry.reservation_id.clone().ok_or(Error::CorruptSpool)?;
        let plan = match entry.upload_plan.clone() {
            Some(plan) => plan,
            None => transport.negotiate(&reservation, &manifest)?,
        };
        validate_plan(&plan, &manifest)?;
        if entry.upload_plan.is_none() {
            entry.upload_plan = Some(plan.clone());
            self.save(&directory, &entry)?;
        }
        let status = transport.status(&reservation, &manifest, &plan)?;
        let confirmed = self.save_status(&directory, &mut entry, status)?;
        if entry.state == CaptureState::Accepted {
            return step_result(entry, None);
        }
        let missing = plan.chunks.iter().find(|chunk| {
            chunk.disposition == ChunkDisposition::Upload && !confirmed.contains(&chunk.key)
        });
        if let Some(planned) = missing {
            let chunk = manifest.chunk(&planned.key).ok_or(Error::CorruptSpool)?;
            let body = self.checked_part(&directory, &planned.key, chunk)?;
            transport.upload_part(
                &reservation,
                &manifest,
                &plan,
                &planned.key,
                chunk,
                &mut body.take(chunk.compressed_bytes),
            )?;
            let status = transport.status(&reservation, &manifest, &plan)?;
            let confirmed = self.save_status(&directory, &mut entry, status)?;
            if !confirmed.contains(&planned.key) {
                return Err(Error::Transport(crate::TransportError::Offline));
            }
            return step_result(entry, Some(planned.key.clone()));
        }
        if !entry.completion_submitted {
            // Save intent before the idempotent completion request.
            entry.state = CaptureState::AwaitingIntegrity;
            self.save(&directory, &entry)?;
            transport.complete(&reservation, &manifest, &plan)?;
            entry.completion_submitted = true;
            self.save(&directory, &entry)?;
            let status = transport.status(&reservation, &manifest, &plan)?;
            self.save_status(&directory, &mut entry, status)?;
        }
        step_result(entry, None)
    }

    /// Commit verified acceptance with its state before releasing saved chunks.
    fn save_status(
        &self,
        directory: &Directory,
        entry: &mut QueueEntry,
        status: ServerStatus,
    ) -> Result<std::collections::BTreeSet<ChunkKey>> {
        let manifest = entry.manifest.as_ref().ok_or(Error::CorruptSpool)?;
        let plan = entry.upload_plan.as_ref().ok_or(Error::CorruptSpool)?;
        let confirmed = validate_status(&status, manifest, plan)?;
        validate_status_progress(entry.server_status.as_ref(), &status)?;
        let state = if status.integrity_accepted {
            CaptureState::Accepted
        } else if entry.state == CaptureState::AwaitingIntegrity
            && plan.chunks.iter().any(|chunk| {
                chunk.disposition == ChunkDisposition::Upload && !confirmed.contains(&chunk.key)
            })
        {
            // Expired storage can lose provisional parts. Refill them from the
            // same saved bytes, then resubmit the exact finalization request.
            entry.completion_submitted = false;
            CaptureState::Queued
        } else {
            entry.state
        };
        if entry.state != state || entry.server_status.as_ref() != Some(&status) {
            entry.state = state;
            entry.server_status = Some(status);
            self.save(directory, entry)?;
        }
        if entry.state == CaptureState::Accepted {
            self.release_accepted_bytes(&entry.identity.capture_id)?;
        }
        Ok(confirmed)
    }

    /// Use only for an explicit capture cancellation, not logout or executor stop.
    ///
    /// This saves cancellation before removing local bytes and the queue entry.
    /// It does not cancel remote uploads or delete remote content. The host must
    /// keep terminal request state and never reuse the capture ID.
    /// Accepted entries return [`Error::AlreadyAccepted`] and retain their receipt.
    pub fn cancel(&mut self, identity: &CaptureIdentity) -> Result<()> {
        identity.validate()?;
        let name = &identity.capture_id;
        let directory = self.root.open_dir(name)?;
        let mut entry = self.read_directory_entry(&directory, name)?;
        if entry.identity != *identity {
            return Err(Error::IdentityMismatch);
        }
        if entry.state == CaptureState::Accepted {
            return Err(Error::AlreadyAccepted);
        }
        entry.state = CaptureState::Cancelled;
        self.save(&directory, &entry)?;
        self.release_entry(name)
    }

    /// Call only after the host durably saves the accepted fingerprint and
    /// terminal request state. Until then, the accepted receipt stays queued.
    ///
    /// The identity must match exactly. Pending entries return
    /// [`Error::NotAccepted`]. An I/O failure may leave deletion in progress;
    /// reopen the spool to finish it and consult the host's saved terminal state.
    pub fn acknowledge_accepted(&mut self, identity: &CaptureIdentity) -> Result<()> {
        identity.validate()?;
        let entry = self.read_entry(&identity.capture_id)?;
        if entry.identity != *identity {
            return Err(Error::IdentityMismatch);
        }
        if entry.state != CaptureState::Accepted {
            return Err(Error::NotAccepted);
        }
        self.release_entry(&identity.capture_id)
    }

    fn create_entry(&self, name: &str, entry: &QueueEntry) -> Result<Directory> {
        let pending_name = format!(".new-{}", entry.identity.capture_id);
        let pending = self.root.create_dir(&pending_name)?;
        self.save(&pending, entry)?;
        self.root.rename(&pending_name, name)?;
        Ok(pending)
    }

    fn save(&self, directory: &Directory, entry: &QueueEntry) -> Result<()> {
        directory.save_record(&encode_bounded(entry, RECORD_BYTES)?)
    }

    fn read_entry(&self, name: &str) -> Result<QueueEntry> {
        self.read_directory_entry(&self.root.open_dir(name)?, name)
    }

    fn read_directory_entry(&self, directory: &Directory, name: &str) -> Result<QueueEntry> {
        let record: serde_json::Value =
            serde_json::from_slice(&directory.read_bounded("record.json", RECORD_BYTES)?)?;
        if let Some(manifest) = record.get("manifest").filter(|value| !value.is_null())
            && manifest.get("version").and_then(serde_json::Value::as_u64)
                != Some(u64::from(CAPTURE_VERSION))
        {
            return Err(Error::UnsupportedFormat);
        }
        let entry: QueueEntry = serde_json::from_value(record)?;
        entry.identity.validate().map_err(|_| Error::CorruptSpool)?;
        if name != entry.identity.capture_id {
            return Err(Error::CorruptSpool);
        }
        if let Some(reservation) = &entry.reservation_id
            && !safe_id(reservation)
        {
            return Err(Error::CorruptSpool);
        }
        if let Some(manifest) = &entry.manifest {
            validate_manifest(manifest, &self.limits)?;
            if manifest.identity != entry.identity {
                return Err(Error::CorruptSpool);
            }
        }
        if matches!(
            entry.state,
            CaptureState::Queued | CaptureState::AwaitingIntegrity
        ) && (entry.manifest.is_none() || entry.reservation_id.is_none())
        {
            return Err(Error::CorruptSpool);
        }
        if entry.manifest.is_none()
            && (entry.upload_plan.is_some()
                || entry.server_status.is_some()
                || entry.completion_submitted)
        {
            return Err(Error::CorruptSpool);
        }
        if entry.completion_submitted
            && !matches!(
                entry.state,
                CaptureState::AwaitingIntegrity | CaptureState::Accepted | CaptureState::Cancelled
            )
        {
            return Err(Error::CorruptSpool);
        }
        entry.progress().map_err(|_| Error::CorruptSpool)?;
        if entry.state == CaptureState::Accepted
            && (entry.manifest.is_none()
                || entry.reservation_id.is_none()
                || entry.upload_plan.is_none()
                || !entry
                    .server_status
                    .as_ref()
                    .is_some_and(|status| status.integrity_accepted))
        {
            return Err(Error::CorruptSpool);
        }
        if entry.state == CaptureState::AwaitingIntegrity {
            let progress = entry.progress().map_err(|_| Error::CorruptSpool)?;
            if entry.server_status.is_none()
                || !progress.negotiated
                || progress.confirmed_upload_bytes != progress.required_transfer_bytes
            {
                return Err(Error::CorruptSpool);
            }
        }
        Ok(entry)
    }

    fn directories(&self) -> Result<Vec<String>> {
        let mut directories = Vec::new();
        for name in self.root.names(self.limits.queue_entries + 1)? {
            if name == "queue.lock" {
                continue;
            }
            if directories.len() >= self.limits.queue_entries {
                return Err(Error::Limit);
            }
            if !(safe_id(&name)
                || name.strip_prefix(".new-").is_some_and(safe_id)
                || name.strip_prefix(".delete-").is_some_and(safe_id))
            {
                return Err(Error::CorruptSpool);
            }
            self.root.open_dir(&name)?;
            directories.push(name);
        }
        Ok(directories)
    }

    fn usage(&self) -> Result<(u64, usize)> {
        let directories = self.directories()?;
        let mut used = 0u64;
        for name in &directories {
            let directory = self.root.open_dir(name)?;
            // Each entry keeps both record slots reserved until host acknowledgment.
            used = used.checked_add(2 * RECORD_BYTES).ok_or(Error::Limit)?;
            for name in directory.names(MAX_DIRECTORY_FILES)? {
                let file = directory.file(&name, false)?;
                let size = file.metadata()?.len();
                if name == "record.json" || name == "record.next" {
                    if size > RECORD_BYTES {
                        return Err(Error::CorruptSpool);
                    }
                    continue;
                }
                used = used.checked_add(size).ok_or(Error::Limit)?;
                if used > self.limits.spool_bytes {
                    return Err(Error::Limit);
                }
            }
            if used > self.limits.spool_bytes {
                return Err(Error::Limit);
            }
        }
        Ok((used, directories.len()))
    }

    fn recover_terminal_entries(&self) -> Result<()> {
        for name in self.directories()? {
            if name.starts_with(".new-") || name.starts_with(".delete-") {
                // These names identify incomplete publication or released bytes.
                self.remove_entry(&name)?;
                continue;
            }
            let mut entry = self.read_entry(&name)?;
            if matches!(
                entry.state,
                CaptureState::Queued | CaptureState::AwaitingIntegrity
            ) && entry
                .server_status
                .as_ref()
                .is_some_and(|status| status.integrity_accepted)
            {
                // Older records can save acceptance before the state transition.
                entry.state = CaptureState::Accepted;
                self.save(&self.root.open_dir(&name)?, &entry)?;
            }
            if entry.state == CaptureState::Accepted {
                self.release_accepted_bytes(&name)?;
            } else if entry.state == CaptureState::Cancelled {
                self.release_entry(&name)?;
            }
        }
        Ok(())
    }

    fn release_entry(&self, name: &str) -> Result<()> {
        let deleting = format!(".delete-{name}");
        self.root.rename(name, &deleting)?;
        self.remove_entry(&deleting)
    }

    fn release_accepted_bytes(&self, name: &str) -> Result<()> {
        let directory = self.root.open_dir(name)?;
        for file_name in directory.names(MAX_DIRECTORY_FILES)? {
            if file_name == "record.json" || file_name == "record.next" {
                continue;
            }
            if !file_name.starts_with("chunk-") || !file_name.ends_with(".gz") {
                return Err(Error::CorruptSpool);
            }
            directory.file(&file_name, false)?;
            directory.remove(&file_name, false)?;
        }
        directory.sync()
    }

    fn remove_entry(&self, name: &str) -> Result<()> {
        let directory = self.root.open_dir(name)?;
        let mut files = Vec::new();
        for file_name in directory.names(MAX_DIRECTORY_FILES)? {
            if file_name != "record.json"
                && file_name != "record.next"
                && !(file_name.starts_with("chunk-") && file_name.ends_with(".gz"))
            {
                return Err(Error::CorruptSpool);
            }
            directory.file(&file_name, false)?;
            files.push(file_name);
        }
        // Retain the terminal record until every byte file is removed.
        files.sort_by_key(|name| name == "record.json");
        for file in files {
            directory.remove(&file, false)?;
        }
        directory.sync()?;
        self.root.remove(name, true)?;
        self.root.sync()?;
        Ok(())
    }

    fn checked_part(&self, directory: &Directory, key: &ChunkKey, chunk: &Chunk) -> Result<File> {
        let mut buffer = [0; 64 * 1024];
        let mut file = directory.file(&chunk_name(key), false)?;
        if file.metadata()?.len() != chunk.compressed_bytes {
            return Err(Error::CorruptSpool);
        }
        let mut hash = Sha256::new();
        let mut remaining = chunk.compressed_bytes;
        while remaining > 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            let read = file.read(&mut buffer[..count])?;
            if read == 0 {
                return Err(Error::CorruptSpool);
            }
            hash.update(&buffer[..read]);
            remaining -= read as u64;
        }
        if hex(&hash.finalize()) != chunk.compressed_sha256 {
            return Err(Error::CorruptSpool);
        }
        file.rewind()?;
        Ok(file)
    }
}

/// Stream one pinned file generation after its reservation is durable.
fn capture_claude(
    directory: &Directory,
    candidate: &Candidate,
    source: &ClaudeSource,
    mut files: Vec<File>,
    pinned: Vec<Metadata>,
    sections: MetadataSections,
) -> Result<(Vec<CapturedStream>, CaptureMetadata)> {
    let mut streams = Vec::new();
    let mut scanner =
        NativeMetadata::new(&source.native_session_id, source.native_agent_id.as_deref());
    let mut project_scanner = candidate.project_selection.as_ref().map(|_| {
        crate::project::ClaudeProjectScan::new(
            &source.native_session_id,
            source.native_agent_id.as_deref(),
        )
    });
    let mut companion_bytes = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    let stream_count = files.len() + 1;
    for (index, file) in files.iter_mut().enumerate() {
        if !same_source(&pinned[index], &file.metadata()?) {
            return Err(Error::SourceChanged);
        }
        let (id, role, schema) = stream_spec(CaptureSourceKind::ClaudeJsonl, index, stream_count);
        let size = pinned[index].len();
        let mut stream = ChunkStream::new(directory, id, size + size / 100 + 1024);
        let mut remaining = size;
        while remaining > 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            let read = file.read(&mut buffer[..count])?;
            if read == 0 {
                return Err(Error::SourceChanged);
            }
            let bytes = &buffer[..read];
            stream.write(bytes)?;
            if index == 0 {
                scanner.feed(bytes);
                if let Some(scan) = &mut project_scanner {
                    scan.feed(bytes)?;
                }
            } else {
                companion_bytes.extend_from_slice(bytes);
            }
            remaining -= read as u64;
        }
        if !same_source(&pinned[index], &file.metadata()?) {
            return Err(Error::SourceChanged);
        }
        streams.push(stream.finish(role, schema)?);
    }
    // Check both files again after streaming either one.
    for (index, file) in files.iter().enumerate() {
        if !same_source(&pinned[index], &file.metadata()?) {
            return Err(Error::SourceChanged);
        }
    }
    scanner.finish();
    if let (Some(scan), Some(selection)) = (project_scanner, &candidate.project_selection) {
        scan.validate_complete(selection)?;
    }
    if scanner.owned_records == 0 && scanner.has_inherited_records {
        return Err(Error::InvalidSource);
    }
    Ok((
        streams,
        claude_metadata(candidate, source, scanner, &companion_bytes, sections),
    ))
}

/// Recheck the first owned header after reservation, then save the pinned file.
fn capture_header_file(
    directory: &Directory,
    candidate: &Candidate,
    mut file: File,
    pinned: Metadata,
    sections: MetadataSections,
) -> Result<(Vec<CapturedStream>, CaptureMetadata)> {
    if !same_source(&pinned, &file.metadata()?) {
        return Err(Error::SourceChanged);
    }
    let id = candidate.native_session_id();
    let (project, metadata) = match &candidate.source {
        CandidateSource::Codex(_) => {
            let header = crate::codex::read_header(&mut file, id)?;
            (
                header.project_evidence(),
                header.metadata(candidate, sections),
            )
        }
        CandidateSource::Pi(_) => (
            crate::pi::project_evidence(&mut file, id)?,
            crate::pi::metadata(id, sections),
        ),
        _ => return Err(Error::InvalidSource),
    };
    if let Some(selection) = &candidate.project_selection
        && !selection.matches(&project)?
    {
        return Err(Error::SourceChanged);
    }
    file.rewind()?;
    let (id, role, schema) = stream_spec(candidate.source_kind(), 0, 2);
    let size = pinned.len();
    let mut stream = ChunkStream::new(directory, id, size + size / 100 + 1024);
    let mut remaining = size;
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let count = remaining.min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..count])?;
        if read == 0 {
            return Err(Error::SourceChanged);
        }
        stream.write(&buffer[..read])?;
        remaining -= read as u64;
    }
    if !same_source(&pinned, &file.metadata()?) {
        return Err(Error::SourceChanged);
    }
    if let Some(selection) = &candidate.project_selection {
        selection.verify_directories()?;
    }
    Ok((vec![stream.finish(role, schema)?], metadata))
}

#[cfg(unix)]
/// Freeze one SQLite snapshot within the source and compressed-spool budgets.
fn capture_opencode(
    directory: &Directory,
    candidate: &Candidate,
    source: &crate::opencode::OpenCodeSource,
    limits: &Limits,
    sections: MetadataSections,
) -> Result<(Vec<CapturedStream>, CaptureMetadata)> {
    let (id, role, schema) = stream_spec(CaptureSourceKind::OpenCodeSqliteV2, 0, 2);
    let maximum_compressed = limits.source_bytes + limits.source_bytes / 100 + 1024;
    let mut stream = ChunkStream::new(directory, id, maximum_compressed);
    let emit = |bytes: &[u8]| {
        if bytes.len() as u64 > limits.source_bytes.saturating_sub(stream.bytes) {
            return Err(Error::Limit);
        }
        stream.write(bytes)
    };
    let exported = match &candidate.project_selection {
        Some(selection) => crate::opencode::export_in_project(source, limits, selection, emit)?,
        None => crate::opencode::export(source, limits, emit)?,
    };
    if let Some(selection) = &candidate.project_selection {
        selection.verify_directories()?;
    }
    let stream = stream.finish(role, schema)?;
    let relationship = if let Some(parent) = exported.fork_session_id {
        if parent == source.native_session_id() {
            Relationship::Unknown
        } else {
            Relationship::Fork {
                parent_source_id: candidate.scoped_key(&source.key_for_session(&parent)?),
                proof: "opencode-session-v2-fork".into(),
            }
        }
    } else if let Some(parent) = exported.parent_session_id {
        if parent == source.native_session_id() {
            Relationship::Unknown
        } else {
            Relationship::Child {
                parent_source_id: candidate.scoped_key(&source.key_for_session(&parent)?),
                proof: "opencode-session-v2-parent".into(),
            }
        }
    } else {
        Relationship::Unknown
    };
    let metadata = CaptureMetadata {
        version: CAPTURE_VERSION,
        agent: "opencode".into(),
        source_format: "OpenCodeSqliteV2".into(),
        native_session_id: source.native_session_id().into(),
        native_agent_id: None,
        source_agent_version: None,
        version_observations: Vec::new(),
        version_scan_complete: false,
        owned_version_complete: false,
        relationship,
        history_completeness: "unknown".into(),
        sections,
    };
    Ok((vec![stream], metadata))
}

/// Attribute only owned runtime versions and directly proved native relationships.
fn claude_metadata(
    candidate: &Candidate,
    source: &ClaudeSource,
    scanner: NativeMetadata,
    companion: &[u8],
    sections: MetadataSections,
) -> CaptureMetadata {
    let mut relationship = source.relationship.clone();
    if scanner.fork_conflict {
        relationship = Relationship::Unknown;
    } else if source.native_agent_id.is_some() {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(companion)
            && value.get("isFork").and_then(|value| value.as_bool()) == Some(true)
        {
            relationship = match value.get("parentAgentId").and_then(|value| value.as_str()) {
                Some(parent)
                    if safe_id(parent)
                        && !parent.starts_with("agent-")
                        && source.native_agent_id.as_deref() != Some(parent) =>
                {
                    Relationship::Fork {
                        parent_source_id: candidate.scoped_key(&format!(
                            "{}/subagents/agent-{parent}",
                            source.native_session_id
                        )),
                        proof: "claude-native-meta-v1".into(),
                    }
                }
                _ => Relationship::Unknown,
            };
        }
    } else if let Some(parent) = scanner.fork_parent {
        relationship = if parent == source.native_session_id {
            Relationship::Unknown
        } else {
            Relationship::Fork {
                parent_source_id: candidate.scoped_key(&parent),
                proof: "claude-forked-from-v1".into(),
            }
        };
    }
    let owned_version_complete =
        scanner.complete && scanner.owned_version_complete && scanner.owned_records > 0;
    let source_agent_version = if owned_version_complete && scanner.owned_versions.len() == 1 {
        scanner.owned_versions.iter().next().cloned()
    } else {
        None
    };
    CaptureMetadata {
        version: CAPTURE_VERSION,
        agent: "claude-code".into(),
        source_format: "ClaudeJsonl".into(),
        native_session_id: source.native_session_id.clone(),
        native_agent_id: source.native_agent_id.clone(),
        source_agent_version,
        version_observations: scanner.observations.into_iter().collect(),
        version_scan_complete: scanner.complete,
        owned_version_complete,
        relationship,
        history_completeness: "unknown".into(),
        sections,
    }
}

fn step_result(entry: QueueEntry, uploaded_chunk: Option<ChunkKey>) -> Result<ResumeStep> {
    let progress = entry.progress()?;
    Ok(ResumeStep {
        entry,
        progress,
        uploaded_chunk,
    })
}
