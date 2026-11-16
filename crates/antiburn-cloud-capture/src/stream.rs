use crate::contract::{hex, safe_id};
use crate::directory::Directory;
use crate::{CHUNK_BYTES, Chunk, Error, Result};
use flate2::{Compression, GzBuilder, write::GzEncoder};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;

pub(crate) fn chunk_name(index: u32) -> String {
    format!("chunk-{index:05}.gz")
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

pub(crate) struct ChunkStream<'a> {
    directory: &'a Directory,
    current: Option<CurrentChunk>,
    chunks: Vec<Chunk>,
    digest: Sha256,
    pub bytes: u64,
    compressed_bytes: u64,
    maximum_compressed: u64,
}

impl<'a> ChunkStream<'a> {
    pub fn new(directory: &'a Directory, maximum_compressed: u64) -> Self {
        Self {
            directory,
            current: None,
            chunks: Vec::new(),
            digest: Sha256::new(),
            bytes: 0,
            compressed_bytes: 0,
            maximum_compressed,
        }
    }

    pub fn write(&mut self, mut bytes: &[u8]) -> Result<()> {
        while !bytes.is_empty() {
            if self.current.is_none() {
                let file = self
                    .directory
                    .file(&chunk_name(self.chunks.len() as u32), true)?;
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
            expanded_offset: self.bytes - current.bytes,
            expanded_bytes: current.bytes,
            expanded_sha256: hex(&current.digest.finalize()),
            compressed_bytes: output.bytes,
            compressed_sha256: hex(&output.digest.finalize()),
        });
        Ok(())
    }

    pub fn finish(mut self) -> Result<(Vec<Chunk>, String, u64)> {
        self.finish_chunk()?;
        self.directory.sync()?;
        Ok((self.chunks, hex(&self.digest.finalize()), self.bytes))
    }
}

const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_VERSIONS: usize = 16;

pub(crate) struct NativeMetadata {
    line: Vec<u8>,
    oversized_line: bool,
    pub complete: bool,
    pub versions: BTreeSet<String>,
    pub fork_parent: Option<String>,
    pub fork_conflict: bool,
}

impl NativeMetadata {
    pub fn new() -> Self {
        Self {
            line: Vec::new(),
            oversized_line: false,
            complete: true,
            versions: BTreeSet::new(),
            fork_parent: None,
            fork_conflict: false,
        }
    }

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
        let record = serde_json::from_slice::<serde_json::Value>(&self.line);
        self.line.clear();
        let Ok(record) = record else {
            self.complete = false;
            return;
        };
        self.inspect_version(&record);
        self.inspect_fork(&record);
    }

    fn inspect_version(&mut self, record: &serde_json::Value) {
        let Some(value) = record.get("version") else {
            return;
        };
        let Some(version) = value.as_str() else {
            self.complete = false;
            return;
        };
        if !safe_id(version.replace('.', "-").as_str())
            || version.len() > 128
            || (!self.versions.contains(version) && self.versions.len() >= MAX_VERSIONS)
        {
            self.complete = false;
            return;
        }
        self.versions.insert(version.to_owned());
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

    pub fn finish(&mut self) {
        if !self.line.is_empty() || self.oversized_line {
            self.complete = false;
        }
        self.line.clear();
    }
}
