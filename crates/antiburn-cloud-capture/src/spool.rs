use crate::contract::{encode_bounded, hex, safe_id};
use crate::directory::Directory;
use crate::filesystem::same_source;
use crate::protocol::{
    stream_spec, validate_manifest, validate_plan, validate_status, validate_status_progress,
};
use crate::stream::{ChunkStream, NativeMetadata, chunk_name};
use crate::*;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

const MAX_DIRECTORY_FILES: usize = 132;

/// A single executor owns the spool lock. Drop this value to stop execution;
/// dropping it never cancels captures or deletes native bytes.
pub struct Spool {
    root: Directory,
    limits: Limits,
    _lock: File,
}

impl Spool {
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

    pub fn entries(&self) -> Result<Vec<QueueEntry>> {
        let mut entries = Vec::new();
        for directory in self.directories()? {
            entries.push(self.read_entry(&directory)?);
        }
        entries.sort_by(|left, right| left.identity.capture_id.cmp(&right.identity.capture_id));
        Ok(entries)
    }

    /// The host must obtain consent and bind this source to the destination
    /// before calling. No native content is read before reserve succeeds.
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
        validate_sections(&sections)?;
        let mut sources = vec![candidate.directory.source_file(&candidate.filename)?];
        if let Some(companion) = &candidate.companion {
            sources.push(candidate.directory.source_file(companion)?);
        }
        let mut native_bytes = 0u64;
        let mut pinned = Vec::new();
        for (index, source) in sources.iter().enumerate() {
            let metadata = source.metadata()?;
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
        let maximum_expanded = native_bytes
            .checked_add(METADATA_BYTES)
            .ok_or(Error::Limit)?;
        if maximum_expanded > self.limits.capture_bytes {
            return Err(Error::Limit);
        }
        let (used, count) = self.usage()?;
        let maximum_compressed =
            maximum_expanded + native_bytes / 100 + sources.len() as u64 * 1024 + 2048;
        let disk_reservation = maximum_compressed + 2 * RECORD_BYTES;
        if count >= self.limits.queue_entries
            || disk_reservation > self.limits.spool_bytes.saturating_sub(used)
        {
            return Err(Error::Limit);
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
        let directory = self.create_entry(&name, &entry)?;
        let reservation_id = transport.reserve(&entry.identity, maximum_expanded)?;
        if !safe_id(&reservation_id) {
            return Err(Error::InvalidServerStatus);
        }
        entry.reservation_id = Some(reservation_id);
        self.save(&directory, &entry)?;

        let mut streams = Vec::new();
        let mut scanner = NativeMetadata::new(
            &candidate.native_session_id,
            candidate.native_agent_id.as_deref(),
        );
        let mut companion_bytes = Vec::new();
        let mut buffer = [0u8; 64 * 1024];
        let stream_count = sources.len() + 1;
        for (index, source) in sources.iter_mut().enumerate() {
            if !same_source(&pinned[index], &source.metadata()?) {
                return Err(Error::SourceChanged);
            }
            let (id, role, schema) = stream_spec(index, stream_count);
            let size = pinned[index].len();
            let mut stream = ChunkStream::new(&directory, id, size + size / 100 + 1024);
            let mut remaining = pinned[index].len();
            while remaining > 0 {
                let count = remaining.min(buffer.len() as u64) as usize;
                let read = source.read(&mut buffer[..count])?;
                if read == 0 {
                    return Err(Error::SourceChanged);
                }
                let bytes = &buffer[..read];
                stream.write(bytes)?;
                if index == 0 {
                    scanner.feed(bytes);
                } else {
                    companion_bytes.extend_from_slice(bytes);
                }
                remaining -= read as u64;
            }
            if !same_source(&pinned[index], &source.metadata()?) {
                return Err(Error::SourceChanged);
            }
            streams.push(stream.finish(role, schema)?);
        }
        // A companion can change while the transcript is read, or vice versa.
        for (index, source) in sources.iter().enumerate() {
            if !same_source(&pinned[index], &source.metadata()?) {
                return Err(Error::SourceChanged);
            }
        }
        scanner.finish();
        if scanner.owned_records == 0 && scanner.has_inherited_records {
            return Err(Error::InvalidSource);
        }
        let metadata = capture_metadata(candidate, scanner, &companion_bytes, sections);
        validate_section_bindings(&metadata.sections, &streams)?;
        let metadata_fingerprint = metadata.stable_fingerprint()?;
        let metadata = encode_bounded(&metadata, METADATA_BYTES)?;
        let (id, role, schema) = stream_spec(stream_count - 1, stream_count);
        let mut stream = ChunkStream::new(&directory, id, METADATA_BYTES + 2048);
        stream.write(&metadata)?;
        streams.push(stream.finish(role, schema)?);
        let mut manifest = CaptureManifest {
            capture_version: CAPTURE_VERSION.into(),
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
    pub fn resume(
        &mut self,
        identity: &CaptureIdentity,
        transport: &mut impl Transport,
    ) -> Result<QueueEntry> {
        loop {
            let step = self.resume_step(identity, transport)?;
            if step.uploaded_chunk.is_none()
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
        let plan = transport.negotiate(&reservation, &manifest)?;
        validate_plan(&plan, &manifest)?;
        if let Some(saved) = &entry.upload_plan {
            if saved != &plan {
                return Err(Error::PlanChanged);
            }
        } else {
            entry.upload_plan = Some(plan.clone());
            self.save(&directory, &entry)?;
        }
        let status = transport.status(&reservation, &manifest, &plan)?;
        let confirmed = self.save_status(&directory, &mut entry, status)?;
        if entry
            .server_status
            .as_ref()
            .is_some_and(|status| status.integrity_accepted)
        {
            return self.accept_entry(&directory, entry, None);
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
                return Err(Error::InvalidServerStatus);
            }
            if entry
                .server_status
                .as_ref()
                .is_some_and(|status| status.integrity_accepted)
            {
                return self.accept_entry(&directory, entry, Some(planned.key.clone()));
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
        if entry
            .server_status
            .as_ref()
            .is_some_and(|status| status.integrity_accepted)
        {
            return self.accept_entry(&directory, entry, None);
        }
        step_result(entry, None)
    }

    fn accept_entry(
        &self,
        directory: &Directory,
        mut entry: QueueEntry,
        uploaded: Option<ChunkKey>,
    ) -> Result<ResumeStep> {
        entry.state = CaptureState::Accepted;
        self.save(directory, &entry)?;
        self.release_accepted_bytes(&entry.identity.capture_id)?;
        step_result(entry, uploaded)
    }

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
        entry.server_status = Some(status);
        self.save(directory, entry)?;
        Ok(confirmed)
    }

    /// Use only for an explicit capture cancellation, not logout or executor stop.
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
        let entry: QueueEntry =
            serde_json::from_slice(&directory.read_bounded("record.json", RECORD_BYTES)?)?;
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
            let entry = self.read_entry(&name)?;
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

fn capture_metadata(
    candidate: &Candidate,
    scanner: NativeMetadata,
    companion: &[u8],
    sections: MetadataSections,
) -> CaptureMetadata {
    let mut relationship = candidate.relationship.clone();
    if scanner.fork_conflict {
        relationship = Relationship::Unknown;
    } else if candidate.native_agent_id.is_some() {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(companion)
            && value.get("isFork").and_then(|value| value.as_bool()) == Some(true)
        {
            relationship = match value.get("parentAgentId").and_then(|value| value.as_str()) {
                Some(parent)
                    if safe_id(parent)
                        && !parent.starts_with("agent-")
                        && candidate.native_agent_id.as_deref() != Some(parent) =>
                {
                    Relationship::Fork {
                        parent_source_id: candidate.scoped_key(&format!(
                            "{}/subagents/agent-{parent}",
                            candidate.native_session_id
                        )),
                        proof: "claude-native-meta-v1".into(),
                    }
                }
                _ => Relationship::Unknown,
            };
        }
    } else if let Some(parent) = scanner.fork_parent {
        relationship = if parent == candidate.native_session_id {
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
        schema_version: 1,
        capture_version: CAPTURE_VERSION.into(),
        agent: "claude-code".into(),
        source_format: "ClaudeJsonl".into(),
        native_session_id: candidate.native_session_id.clone(),
        native_agent_id: candidate.native_agent_id.clone(),
        source_agent_version,
        recorded_agent_versions: scanner.versions.into_iter().collect(),
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
