//! Deterministic gzip chunks and bounded Claude source evidence.
//!
//! Capture writes exact input before evidence inspection. The scanner never
//! repairs native bytes or turns inherited records into owned runtime evidence.

use crate::contract::{hex, safe_id};
use crate::directory::Directory;
use crate::{
    CHUNK_BYTES, CapturedStream, Chunk, ChunkKey, Error, Result, VersionObservation,
    VersionOwnership,
};
use flate2::{Compression, GzBuilder, write::GzEncoder};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;

/// Name a chunk from a validated stream key, never a source path.
pub(crate) fn chunk_name(key: &ChunkKey) -> String {
    format!("chunk-{}-{:05}.gz", key.stream_id, key.index)
}

struct Output {
    file: File,
    digest: Sha256,
    bytes: u64,
    maximum: u64,
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.maximum.saturating_sub(self.bytes) {
            return Err(std::io::Error::from(std::io::ErrorKind::StorageFull));
        }
        let written = self.file.write(bytes)?;
        self.digest.update(&bytes[..written]);
        self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

struct CurrentChunk {
    encoder: GzEncoder<Output>,
    digest: Sha256,
    bytes: u64,
}

/// Save independent gzip members under one reserved compressed-byte budget.
pub(crate) struct ChunkStream<'a> {
    directory: &'a Directory,
    stream_id: String,
    current: Option<CurrentChunk>,
    chunks: Vec<Chunk>,
    digest: Sha256,
    pub bytes: u64,
    compressed_bytes: u64,
    maximum_compressed: u64,
}

impl<'a> ChunkStream<'a> {
    /// Set up a stream; create a chunk only when nonempty input arrives.
    pub fn new(directory: &'a Directory, stream_id: &str, maximum_compressed: u64) -> Self {
        Self {
            directory,
            stream_id: stream_id.into(),
            current: None,
            chunks: Vec::new(),
            digest: Sha256::new(),
            bytes: 0,
            compressed_bytes: 0,
            maximum_compressed,
        }
    }

    /// Split exact expanded bytes at fixed boundaries; the caller owns its input cap.
    pub fn write(&mut self, mut bytes: &[u8]) -> Result<()> {
        while !bytes.is_empty() {
            if self.current.is_none() {
                let file = self.directory.file(
                    &chunk_name(&ChunkKey {
                        stream_id: self.stream_id.clone(),
                        index: self.chunks.len() as u32,
                    }),
                    true,
                )?;
                let output = Output {
                    file,
                    digest: Sha256::new(),
                    bytes: 0,
                    maximum: self.maximum_compressed - self.compressed_bytes,
                };
                self.current = Some(CurrentChunk {
                    encoder: GzBuilder::new()
                        .mtime(0)
                        .operating_system(255)
                        .write(output, Compression::new(6)),
                    digest: Sha256::new(),
                    bytes: 0,
                });
            }
            let current = self.current.as_mut().ok_or(Error::CorruptSpool)?;
            let count = bytes.len().min((CHUNK_BYTES - current.bytes) as usize);
            let part = &bytes[..count];
            current.encoder.write_all(part)?;
            current.digest.update(part);
            self.digest.update(part);
            current.bytes += count as u64;
            self.bytes += count as u64;
            bytes = &bytes[count..];
            if current.bytes == CHUNK_BYTES {
                self.finish_chunk()?;
            }
        }
        Ok(())
    }

    fn finish_chunk(&mut self) -> Result<()> {
        let Some(current) = self.current.take() else {
            return Ok(());
        };
        let output = current.encoder.finish()?;
        output.file.sync_all()?;
        self.compressed_bytes += output.bytes;
        self.chunks.push(Chunk {
            index: self.chunks.len() as u32,
            expanded_bytes: current.bytes,
            expanded_sha256: hex(&current.digest.finalize()),
            compressed_bytes: output.bytes,
            compressed_sha256: hex(&output.digest.finalize()),
        });
        Ok(())
    }

    /// Sync all members and publish descriptors, including an empty-file digest.
    pub fn finish(mut self, role: &str, schema: &str) -> Result<CapturedStream> {
        self.finish_chunk()?;
        self.directory.sync()?;
        Ok(CapturedStream {
            stream_id: self.stream_id,
            role: role.into(),
            schema: schema.into(),
            expanded_bytes: self.bytes,
            expanded_sha256: hex(&self.digest.finalize()),
            chunks: self.chunks,
        })
    }
}

const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_VERSIONS: usize = 16;

/// Inspect bounded Claude records without changing the saved native stream.
///
/// Unknown object keys remain literal data, not JSON-library type directives.
pub(crate) struct NativeMetadata {
    line: Vec<u8>,
    oversized_line: bool,
    pub complete: bool,
    pub versions: BTreeSet<String>,
    pub observations: BTreeSet<VersionObservation>,
    pub owned_versions: BTreeSet<String>,
    pub owned_version_complete: bool,
    pub owned_records: usize,
    pub has_inherited_records: bool,
    native_session_id: String,
    native_agent_id: Option<String>,
    pub fork_parent: Option<String>,
    pub fork_conflict: bool,
}

impl NativeMetadata {
    /// Bind ownership checks to the native filename identity, not installation state.
    pub fn new(native_session_id: &str, native_agent_id: Option<&str>) -> Self {
        Self {
            line: Vec::new(),
            oversized_line: false,
            complete: true,
            versions: BTreeSet::new(),
            observations: BTreeSet::new(),
            owned_versions: BTreeSet::new(),
            owned_version_complete: true,
            owned_records: 0,
            has_inherited_records: false,
            native_session_id: native_session_id.into(),
            native_agent_id: native_agent_id.map(str::to_owned),
            fork_parent: None,
            fork_conflict: false,
        }
    }

    /// Inspect complete bounded records and mark malformed or oversized scans partial.
    pub fn feed(&mut self, bytes: &[u8]) {
        for byte in bytes {
            if *byte == b'\n' {
                self.finish_line();
            } else if self.line.len() < MAX_LINE_BYTES && !self.oversized_line {
                self.line.push(*byte);
            } else {
                self.oversized_line = true;
                self.complete = false;
            }
        }
    }

    fn finish_line(&mut self) {
        if self.oversized_line || self.line.is_empty() {
            self.line.clear();
            self.oversized_line = false;
            return;
        }
        let record = crate::metadata::decode_metadata_value(&self.line);
        self.line.clear();
        let Ok(record) = record else {
            self.complete = false;
            return;
        };
        let ownership = self.ownership(&record);
        self.has_inherited_records |= ownership == VersionOwnership::Inherited;
        self.inspect_version(&record, ownership.clone());
        if ownership == VersionOwnership::Owned {
            self.inspect_fork(&record);
        }
    }

    fn ownership(&self, record: &serde_json::Value) -> VersionOwnership {
        let session = record.get("sessionId").and_then(|value| value.as_str());
        let agent = match record.get("agentId") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(agent)) => Some(agent.as_str()),
            _ => return VersionOwnership::Unknown,
        };
        let sidechain = match record.get("isSidechain") {
            None => false,
            Some(serde_json::Value::Bool(value)) => *value,
            _ => return VersionOwnership::Unknown,
        };
        let owned_agent = match &self.native_agent_id {
            Some(expected) => agent == Some(expected.as_str()),
            None => agent.is_none() && !sidechain,
        };
        if session == Some(self.native_session_id.as_str()) && owned_agent {
            VersionOwnership::Owned
        } else if agent.is_some() || sidechain || session.is_some() {
            VersionOwnership::Inherited
        } else {
            VersionOwnership::Unknown
        }
    }

    fn inspect_version(&mut self, record: &serde_json::Value, ownership: VersionOwnership) {
        let owned = ownership == VersionOwnership::Owned;
        if owned {
            self.owned_records += 1;
        }
        let Some(value) = record.get("version") else {
            if owned {
                self.owned_version_complete = false;
            }
            return;
        };
        let Some(version) = value.as_str() else {
            self.complete = false;
            if owned {
                self.owned_version_complete = false;
            }
            return;
        };
        if !safe_id(version.replace('.', "-").as_str())
            || version.len() > 128
            || (!self.versions.contains(version) && self.versions.len() >= MAX_VERSIONS)
        {
            self.complete = false;
            if owned {
                self.owned_version_complete = false;
            }
            return;
        }
        self.versions.insert(version.to_owned());
        if owned {
            self.owned_versions.insert(version.to_owned());
        }
        self.observations.insert(VersionObservation {
            version: version.into(),
            ownership,
        });
    }

    fn inspect_fork(&mut self, record: &serde_json::Value) {
        let Some(fork) = record.get("forkedFrom") else {
            return;
        };
        let Some(parent) = fork
            .get("sessionId")
            .and_then(|value| value.as_str())
            .filter(|value| safe_id(value))
        else {
            self.fork_conflict = true;
            return;
        };
        self.fork_conflict |= self.fork_parent.as_deref().is_some_and(|old| old != parent);
        self.fork_parent = Some(parent.into());
    }

    /// Deny complete evidence for an unfinished final record without repairing it.
    pub fn finish(&mut self) {
        if !self.line.is_empty() || self.oversized_line {
            self.complete = false;
        }
        self.line.clear();
    }
}
