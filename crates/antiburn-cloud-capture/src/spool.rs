use crate::contract::{digest, encode_bounded, hex, safe_id};
use crate::directory::Directory;
use crate::filesystem::same_source;
use crate::stream::{ChunkStream, NativeMetadata, chunk_name};
use crate::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
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
        extensions: BTreeMap<String, serde_json::Value>,
        transport: &mut impl Transport,
    ) -> Result<QueueEntry> {
        identity.validate()?;
        if identity.source_id != candidate.source_id {
            return Err(Error::IdentityMismatch);
        }
        encode_bounded(&extensions, METADATA_BYTES / 2)?;
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
        let mut sources = vec![(
            "transcript.jsonl",
            "native_transcript",
            candidate.directory.source_file(&candidate.filename)?,
        )];
        if let Some(companion) = &candidate.companion {
            sources.push((
                "native.meta.json",
                "native_companion",
                candidate.directory.source_file(companion)?,
            ));
        }
        let mut native_bytes = 0u64;
        let mut pinned = Vec::new();
        for (name, _, source) in &sources {
            let metadata = source.metadata()?;
            if !same_source(&metadata, &metadata) {
                return Err(Error::InvalidSource);
            }
            let maximum = if *name == "native.meta.json" {
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
        let maximum_compressed = maximum_expanded + maximum_expanded / 100 + 1024;
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
        };
        let directory = self.create_entry(&name, &entry)?;
        let reservation_id = transport.reserve(&entry.identity, maximum_expanded)?;
        if !safe_id(&reservation_id) {
            return Err(Error::InvalidServerStatus);
        }
        entry.reservation_id = Some(reservation_id);
        self.save(&directory, &entry)?;

        let mut stream = ChunkStream::new(&directory, maximum_compressed);
        let mut files = Vec::new();
        let mut scanner = NativeMetadata::new();
        let mut companion_bytes = Vec::new();
        let mut buffer = [0u8; 64 * 1024];
        for (index, (name, role, source)) in sources.iter_mut().enumerate() {
            if !same_source(&pinned[index], &source.metadata()?) {
                return Err(Error::SourceChanged);
            }
            let offset = stream.bytes;
            let mut hash = Sha256::new();
            let mut remaining = pinned[index].len();
            while remaining > 0 {
                let count = remaining.min(buffer.len() as u64) as usize;
                let read = source.read(&mut buffer[..count])?;
                if read == 0 {
                    return Err(Error::SourceChanged);
                }
                let bytes = &buffer[..read];
                stream.write(bytes)?;
                hash.update(bytes);
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
            files.push(CapturedFile {
                name: (*name).into(),
                role: (*role).into(),
                offset,
                bytes: pinned[index].len(),
                sha256: hex(&hash.finalize()),
            });
        }
        // A companion can change while the transcript is read, or vice versa.
        for (index, (_, _, source)) in sources.iter().enumerate() {
            if !same_source(&pinned[index], &source.metadata()?) {
                return Err(Error::SourceChanged);
            }
        }
        scanner.finish();
        let metadata = capture_metadata(
            &entry.identity,
            candidate,
            scanner,
            &companion_bytes,
            extensions,
        );
        let metadata = encode_bounded(&metadata, METADATA_BYTES)?;
        files.push(CapturedFile {
            name: "capture-metadata.json".into(),
            role: "capture_metadata".into(),
            offset: stream.bytes,
            bytes: metadata.len() as u64,
            sha256: digest(&metadata),
        });
        stream.write(&metadata)?;
        let (chunks, expanded_sha256, expanded_bytes) = stream.finish()?;
        entry.manifest = Some(CaptureManifest {
            capture_version: CAPTURE_VERSION.into(),
            identity: entry.identity.clone(),
            compression: "gzip-mtime0-os255-level6".into(),
            chunk_expanded_bytes: CHUNK_BYTES,
            expanded_bytes,
            expanded_sha256,
            files,
            chunks,
        });
        entry.state = CaptureState::Queued;
        self.save(&directory, &entry)?;
        Ok(entry)
    }

    /// Reconcile the server's saved state before sending any saved part again.
    pub fn resume(
        &mut self,
        identity: &CaptureIdentity,
        transport: &mut impl Transport,
    ) -> Result<QueueEntry> {
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
            return Ok(entry);
        }
        let manifest = entry.manifest.as_ref().ok_or(Error::CorruptSpool)?;
        let reservation = entry.reservation_id.as_deref().ok_or(Error::CorruptSpool)?;
        self.validate_saved_parts(&directory, manifest)?;
        let status = transport.negotiate(reservation, manifest)?;
        let mut confirmed = validate_status(&status, manifest)?;
        if !status.integrity_accepted {
            for chunk in &manifest.chunks {
                if confirmed.contains(&chunk.index) {
                    continue;
                }
                let body = directory.file(&chunk_name(chunk.index), false)?;
                transport.upload_part(
                    reservation,
                    identity,
                    chunk,
                    &mut body.take(chunk.compressed_bytes),
                )?;
                let status = transport.status(reservation, manifest)?;
                confirmed = validate_status(&status, manifest)?;
                if !confirmed.contains(&chunk.index) {
                    return Err(Error::InvalidServerStatus);
                }
            }
            if confirmed.len() != manifest.chunks.len() {
                return Err(Error::InvalidServerStatus);
            }
            // This state is durable before submission. A lost response resumes
            // through negotiation and server status, not source recapture.
            entry.state = CaptureState::AwaitingIntegrity;
            self.save(&directory, &entry)?;
            transport.complete(reservation, manifest)?;
            let status = transport.status(reservation, manifest)?;
            validate_status(&status, manifest)?;
            if !status.integrity_accepted {
                return Ok(entry);
            }
        }
        entry.state = CaptureState::Accepted;
        self.save(&directory, &entry)?;
        self.release_entry(name)?;
        Ok(entry)
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
        entry.state = CaptureState::Cancelled;
        self.save(&directory, &entry)?;
        self.release_entry(name)
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
            for name in directory.names(MAX_DIRECTORY_FILES)? {
                let file = directory.file(&name, false)?;
                used = used
                    .checked_add(file.metadata()?.len())
                    .ok_or(Error::Limit)?;
                if used > self.limits.spool_bytes {
                    return Err(Error::Limit);
                }
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
            if matches!(
                entry.state,
                CaptureState::Accepted | CaptureState::Cancelled
            ) {
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

    fn validate_saved_parts(
        &self,
        directory: &Directory,
        manifest: &CaptureManifest,
    ) -> Result<()> {
        let mut buffer = [0; 64 * 1024];
        for chunk in &manifest.chunks {
            let mut file = directory.file(&chunk_name(chunk.index), false)?;
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
        }
        Ok(())
    }
}

fn capture_metadata(
    identity: &CaptureIdentity,
    candidate: &Candidate,
    scanner: NativeMetadata,
    companion: &[u8],
    extensions: BTreeMap<String, serde_json::Value>,
) -> CaptureMetadata {
    let mut relationship = candidate.relationship.clone();
    if candidate.native_agent_id.is_some() {
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
    } else if scanner.fork_conflict {
        relationship = Relationship::Unknown;
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
    let source_agent_version = if scanner.complete && scanner.versions.len() == 1 {
        scanner.versions.iter().next().cloned()
    } else {
        None
    };
    CaptureMetadata {
        schema_version: 1,
        capture_version: CAPTURE_VERSION.into(),
        identity: identity.clone(),
        agent: "claude-code".into(),
        source_format: "ClaudeJsonl".into(),
        native_session_id: candidate.native_session_id.clone(),
        native_agent_id: candidate.native_agent_id.clone(),
        source_agent_version,
        recorded_agent_versions: scanner.versions.into_iter().collect(),
        version_scan_complete: scanner.complete,
        relationship,
        history_completeness: "unknown".into(),
        extensions,
    }
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_manifest(manifest: &CaptureManifest, limits: &Limits) -> Result<()> {
    if manifest.capture_version != CAPTURE_VERSION
        || manifest.compression != "gzip-mtime0-os255-level6"
        || manifest.chunk_expanded_bytes != CHUNK_BYTES
        || manifest.expanded_bytes == 0
        || manifest.expanded_bytes > limits.capture_bytes
        || !valid_hash(&manifest.expanded_sha256)
        || manifest.chunks.is_empty()
        || manifest.chunks.len() > 129
        || !(2..=3).contains(&manifest.files.len())
    {
        return Err(Error::CorruptSpool);
    }
    let mut offset = 0;
    for (index, chunk) in manifest.chunks.iter().enumerate() {
        if chunk.index as usize != index
            || chunk.expanded_offset != offset
            || chunk.expanded_bytes == 0
            || chunk.expanded_bytes > CHUNK_BYTES
            || (index + 1 != manifest.chunks.len() && chunk.expanded_bytes != CHUNK_BYTES)
            || chunk.compressed_bytes == 0
            || chunk.compressed_bytes > CHUNK_BYTES + CHUNK_BYTES / 100 + 1024
            || !valid_hash(&chunk.expanded_sha256)
            || !valid_hash(&chunk.compressed_sha256)
        {
            return Err(Error::CorruptSpool);
        }
        offset += chunk.expanded_bytes;
    }
    if offset != manifest.expanded_bytes {
        return Err(Error::CorruptSpool);
    }
    offset = 0;
    for (index, file) in manifest.files.iter().enumerate() {
        let (name, role) = if index == 0 {
            ("transcript.jsonl", "native_transcript")
        } else if index + 1 == manifest.files.len() {
            ("capture-metadata.json", "capture_metadata")
        } else {
            ("native.meta.json", "native_companion")
        };
        if file.offset != offset
            || (index == 0 && file.bytes > limits.source_bytes)
            || (index > 0 && file.bytes > METADATA_BYTES)
            || file.name != name
            || file.role != role
            || !valid_hash(&file.sha256)
        {
            return Err(Error::CorruptSpool);
        }
        offset = offset.checked_add(file.bytes).ok_or(Error::CorruptSpool)?;
    }
    if offset != manifest.expanded_bytes {
        return Err(Error::CorruptSpool);
    }
    Ok(())
}

fn validate_status(status: &ServerStatus, manifest: &CaptureManifest) -> Result<BTreeSet<u32>> {
    if status.identity != manifest.identity
        || status.manifest_sha256 != manifest.sha256()?
        || status.parts.len() > manifest.chunks.len()
    {
        return Err(Error::InvalidServerStatus);
    }
    let mut confirmed = BTreeSet::new();
    for part in &status.parts {
        let chunk = manifest
            .chunks
            .get(part.index as usize)
            .ok_or(Error::InvalidServerStatus)?;
        if chunk.compressed_sha256 != part.compressed_sha256 || !confirmed.insert(part.index) {
            return Err(Error::InvalidServerStatus);
        }
    }
    if status.integrity_accepted && confirmed.len() != manifest.chunks.len() {
        return Err(Error::InvalidServerStatus);
    }
    Ok(confirmed)
}
