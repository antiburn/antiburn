//! Shared file admission for transcripts with a first-line identity.

use crate::directory::Directory;
use crate::filesystem::same_source;
use crate::{Error, Limits, ProjectEvidence, Result};
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const HEADER_BYTES: u64 = 64 * 1024;

/// Retain the admitted directory so a replaced ancestor cannot redirect reads.
#[derive(Clone, Debug)]
pub(crate) struct HeaderFileSource {
    pub(crate) path: PathBuf,
    pub(crate) native_session_id: String,
    directory: Arc<Directory>,
    filename: String,
}

impl HeaderFileSource {
    /// Admit an exact regular file locator with a provider-specific filename rule.
    pub(crate) fn new(
        path: &Path,
        native_session_id: &str,
        matches_filename: impl FnOnce(&str) -> bool,
    ) -> Result<Self> {
        if !crate::contract::safe_id(native_session_id) {
            return Err(Error::InvalidIdentity);
        }
        let normalized: PathBuf = path.components().collect();
        if !path.is_absolute() || normalized.as_os_str() != path.as_os_str() {
            return Err(Error::InvalidSource);
        }
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| matches_filename(name))
            .ok_or(Error::InvalidSource)?;
        let directory = Directory::source(path.parent().ok_or(Error::InvalidSource)?)?;
        Ok(Self {
            directory: Arc::new(directory),
            path: path.to_owned(),
            filename: filename.to_owned(),
            native_session_id: native_session_id.to_owned(),
        })
    }

    /// Open a regular single-link file and pin its bounded generation.
    pub(crate) fn pin(&self, limits: &Limits) -> Result<(File, Metadata)> {
        let file = self.directory.source_file(&self.filename)?;
        let pinned = file.metadata()?;
        if !same_source(&pinned, &pinned) {
            return Err(Error::InvalidSource);
        }
        if pinned.len() > limits.source_bytes {
            return Err(Error::Limit);
        }
        Ok((file, pinned))
    }

    /// Read project evidence and reject a generation change during the lookup.
    pub(crate) fn project_evidence(
        &self,
        limits: &Limits,
        read: impl FnOnce(&mut File, &str) -> Result<ProjectEvidence>,
    ) -> Result<ProjectEvidence> {
        let (mut file, pinned) = self.pin(limits)?;
        let evidence = read(&mut file, &self.native_session_id);
        if !same_source(&pinned, &file.metadata()?) {
            return Err(Error::SourceChanged);
        }
        evidence
    }
}

/// Read only the first physical line for evidence, with a 64 KiB byte limit.
pub(crate) fn read_first_line(file: &mut File) -> Result<String> {
    let mut line = Vec::new();
    BufReader::new(file)
        .take(HEADER_BYTES + 1)
        .read_until(b'\n', &mut line)?;
    if line.len() as u64 > HEADER_BYTES {
        return Err(Error::Limit);
    }
    if line.last() != Some(&b'\n') {
        return Err(Error::InvalidSource);
    }
    String::from_utf8(line).map_err(|_| Error::InvalidSource)
}

/// Parse a JSON object. Serde also accepts positional arrays for structs.
pub(crate) fn parse_object<'a, T: serde::Deserialize<'a>>(json: &'a str) -> Option<T> {
    if !json.trim_start().starts_with('{') {
        return None;
    }
    serde_json::from_str(json).ok()
}
