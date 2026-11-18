//! Bounded recorded-CWD evidence and explicit project folder permissions.
//!
//! Provider storage paths, tool paths, and lineage never grant project access.
//! Run these synchronous source and directory probes on bounded blocking workers.

use crate::discovery::{CandidateSource, ClaudeSource};
use crate::{Candidate, Error, Limits, Result};
use serde::Deserialize;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// Per-record bound for local Claude project evidence.
///
/// A full metadata scan streams up to `Limits::source_bytes`, not just this many
/// total bytes. The 8 MiB bound permits large recorded tool results while keeping
/// memory bounded. Reads still use 64 KiB buffers. Larger records deny complete
/// evidence but never change source.
pub const PROJECT_RECORD_BYTES: usize = 8 * 1024 * 1024;
const PROJECT_READ_BYTES: usize = 64 * 1024;
const MAX_PATH_BYTES: usize = 4096;
const MAX_SELECTED_FOLDERS: usize = 100;
const MAX_PROJECT_PATHS: usize = 256;
const PROJECT_SCAN_TIME: Duration = Duration::from_secs(30);

/// Sorted, distinct project/CWD paths recorded by this source's own records.
///
/// Changing CWDs are valid. Claude scans bounded native records without retaining
/// the full transcript. OpenCode reads the exact `session_v2.directory` cell.
/// Incomplete evidence cannot admit capture. This value grants no source access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectEvidence {
    paths: Vec<PathBuf>,
    complete: bool,
    source_digest: Option<String>,
}

impl ProjectEvidence {
    /// Return every proved CWD in stable path order, never provider storage paths.
    ///
    /// Inspect [`Self::scan_complete`] before using the set for capture admission.
    /// The set can still help local discovery when other records are unreadable.
    pub fn project_paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Return the first proved CWD only when the metadata scan is complete.
    ///
    /// This single-path helper does not describe every project of a session.
    /// Use [`Self::project_paths`] for project discovery and grouping.
    pub fn project_path(&self) -> Option<&Path> {
        self.complete
            .then(|| self.paths.first().map(PathBuf::as_path))
            .flatten()
    }

    /// Report whether all inspected native records permit a complete CWD check.
    ///
    /// A complete scan can still have no owned recorded CWD. Inherited records
    /// cannot grant permission. Malformed or ambiguous records deny completeness.
    /// This does not prove complete historical CWD history: OpenCode supplies only
    /// its characterized current `session_v2.directory` metadata.
    pub fn scan_complete(&self) -> bool {
        self.complete
    }

    /// Return the full streamed Claude transcript digest, when available.
    ///
    /// This local cache value is not a capture fingerprint, authorization, or a
    /// freshness check. OpenCode returns `None`: this lookup never reads or hashes
    /// the whole database. The host owns cache keys, invalidation, and eviction.
    pub fn source_digest(&self) -> Option<&str> {
        self.source_digest.as_deref()
    }

    /// Build evidence from the directory recorded by the Unix OpenCode reader.
    #[cfg(unix)]
    pub(crate) fn recorded(path: &str) -> Self {
        Self {
            paths: if valid_project_path(path) {
                vec![PathBuf::from(path)]
            } else {
                Vec::new()
            },
            complete: true,
            source_digest: None,
        }
    }

    /// Keep a completed OpenCode lookup without an owned directory unapproved.
    #[cfg(unix)]
    pub(crate) fn unknown() -> Self {
        Self {
            paths: Vec::new(),
            complete: true,
            source_digest: None,
        }
    }
}

/// An explicit allowlist of project/CWD folders, including their descendants.
///
/// The host must bind this selection to the current account and content consent.
/// Source storage grants remain separate. Paths must be existing absolute
/// directories without aliases or symlink components. Matching uses components,
/// so selecting `/work/app` never includes `/work/app-other`.
///
/// This value performs no Git lookup. A separate worktree outside a selected
/// folder needs its own selection, even when it belongs to the same repository.
#[derive(Clone, Debug)]
pub struct ProjectSelection {
    folders: Vec<PathBuf>,
}

impl ProjectSelection {
    /// Validate at most 100 explicit folders without reading their contents.
    ///
    /// Empty selections permit nothing. Invalid paths and inaccessible or
    /// symlinked directories fail closed. Run on a bounded blocking worker.
    pub fn new(folders: Vec<PathBuf>) -> Result<Self> {
        if folders.len() > MAX_SELECTED_FOLDERS {
            return Err(Error::Limit);
        }
        for folder in &folders {
            verify_project_directory(folder)?;
        }
        Ok(Self { folders })
    }

    /// Match any owned recorded CWD against selected folder trees.
    ///
    /// Unknown or deleted CWDs do not match. Inaccessible or aliased paths return
    /// an error, not permission. Directory checks read no file content.
    pub fn matches(&self, evidence: &ProjectEvidence) -> Result<bool> {
        if !evidence.complete {
            return Ok(false);
        }
        for path in &evidence.paths {
            if self.matches_path(path)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Scan and admit one independent candidate under this folder permission.
    ///
    /// A nonmatching or unknown candidate returns `None`. The returned candidate
    /// keeps its native identity and freezes this approved folder policy. Capture
    /// rechecks the policy after reservation against current full-source evidence.
    /// CWD changes remain valid when any owned CWD matches an approved tree.
    /// Local metadata access is separate from ongoing full-content upload consent.
    pub fn admit(&self, candidate: &Candidate) -> Result<Option<Candidate>> {
        self.admit_with_limits(candidate, &Limits::default())
    }

    /// Apply admission with explicit metadata scan byte and schema bounds.
    pub fn admit_with_limits(
        &self,
        candidate: &Candidate,
        limits: &Limits,
    ) -> Result<Option<Candidate>> {
        if self.folders.is_empty() {
            return Ok(None);
        }
        let evidence = candidate.project_evidence_with_limits(limits)?;
        if !self.matches(&evidence)? {
            return Ok(None);
        }
        let mut candidate = candidate.clone();
        candidate.project_paths = Vec::new();
        for path in evidence.paths {
            if self.matches_path(&path)? {
                candidate.project_paths.push(path);
            }
        }
        candidate.project_selection = Some(self.clone());
        Ok(Some(candidate))
    }

    fn matches_path(&self, path: &Path) -> Result<bool> {
        for folder in &self.folders {
            if path.starts_with(folder) {
                verify_project_directory(folder)?;
                match verify_project_directory(path) {
                    Err(Error::Io(std::io::ErrorKind::NotFound)) => continue,
                    result => result?,
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Recheck the frozen approved roots without reading directory contents.
    pub(crate) fn verify_directories(&self) -> Result<()> {
        for folder in &self.folders {
            verify_project_directory(folder)?;
        }
        Ok(())
    }
}

impl Candidate {
    /// Return the first admitted CWD from [`ProjectSelection::admit`].
    ///
    /// Use [`Self::admitted_project_paths`] for every matched CWD. Neither helper
    /// identifies a repository root or grants continuing upload or source access.
    pub fn admitted_project_path(&self) -> Option<&Path> {
        self.project_paths.first().map(PathBuf::as_path)
    }

    /// Return all initially admitted CWDs in stable path order.
    ///
    /// Capture rechecks the approved root policy, not equality with this snapshot.
    /// The host owns project/worktree grouping and cache refresh.
    pub fn admitted_project_paths(&self) -> &[PathBuf] {
        &self.project_paths
    }

    /// Stream bounded local project/CWD evidence with default source limits.
    ///
    /// The host must authorize local source access first. Claude can read all
    /// transcript bytes up to `Limits::source_bytes` (512 MiB by default) through
    /// its admitted handle, retaining only bounded records and CWD paths. These
    /// reads can include session content. Only source-owned top-level `cwd` proves
    /// a path. Encoded directory names, tool paths, and lineage grant nothing.
    /// OpenCode reads only bounded schema and this session's `directory` cell.
    ///
    /// This synchronous scan does not reserve, spool, upload, or grant ongoing
    /// upload consent. Run it on a bounded blocking worker. The host owns bounded
    /// metadata caching; the library owns no global cache and performs no Git read.
    pub fn project_evidence(&self) -> Result<ProjectEvidence> {
        self.project_evidence_with_limits(&Limits::default())
    }

    /// Stream source-owned CWDs with explicit byte/schema limits and a 30s deadline.
    ///
    /// At most 256 distinct paths of 4,096 UTF-8 bytes are retained. An exceeded
    /// byte, path-count, or time bound returns `Limit`, not a complete-looking set.
    /// Malformed, oversized, unfinished, or unowned ambiguous records make
    /// `scan_complete()` false. Capture cannot admit an incomplete result.
    pub fn project_evidence_with_limits(&self, limits: &Limits) -> Result<ProjectEvidence> {
        limits.validate()?;
        match &self.source {
            CandidateSource::Claude(source) => scan_claude(source, limits),
            #[cfg(unix)]
            CandidateSource::OpenCode(source) => source.project_evidence(limits),
        }
    }
}

/// Reject ambiguous lexical paths before any filesystem probe.
fn valid_project_path(value: &str) -> bool {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > MAX_PATH_BYTES
        || value.chars().any(char::is_control)
        || !path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return false;
    }
    let normalized: PathBuf = path.components().collect();
    normalized.as_os_str() == path.as_os_str()
}

/// Check path structure and each directory component without reading contents.
pub(crate) fn verify_project_directory(path: &Path) -> Result<()> {
    if !path.to_str().is_some_and(valid_project_path) {
        return Err(Error::InvalidSource);
    }
    #[cfg(unix)]
    {
        crate::filesystem::open_source(path, true)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err(Error::UnsupportedPlatform)
    }
}

/// Stream local CWD evidence without retaining the full source or uploading it.
fn scan_claude(source: &ClaudeSource, limits: &Limits) -> Result<ProjectEvidence> {
    let mut file = source.directory.source_file(&source.filename)?;
    let before = file.metadata()?;
    if !crate::filesystem::same_source(&before, &before) {
        return Err(Error::InvalidSource);
    }
    if before.len() > limits.source_bytes {
        return Err(Error::Limit);
    }
    let deadline = Instant::now() + PROJECT_SCAN_TIME;
    let mut scan =
        ClaudeProjectScan::new(&source.native_session_id, source.native_agent_id.as_deref());
    let mut digest = Sha256::new();
    let mut buffer = [0; PROJECT_READ_BYTES];
    let mut remaining = before.len();
    while remaining > 0 {
        if Instant::now() >= deadline {
            return Err(Error::Limit);
        }
        let count = remaining.min(buffer.len() as u64) as usize;
        let count = file.read(&mut buffer[..count])?;
        if count == 0 {
            return Err(Error::SourceChanged);
        }
        digest.update(&buffer[..count]);
        scan.feed(&buffer[..count])?;
        remaining -= count as u64;
    }
    if Instant::now() >= deadline {
        return Err(Error::Limit);
    }
    if !crate::filesystem::same_source(&before, &file.metadata()?) {
        return Err(Error::SourceChanged);
    }
    let mut evidence = scan.evidence();
    evidence.source_digest = Some(crate::contract::hex(&digest.finalize()));
    Ok(evidence)
}

/// Inspect only owned top-level CWD fields while discarding message content.
///
/// Capture can feed the full stream after reservation. A malformed, oversized,
/// or unfinished record denies a complete project check without changing bytes.
pub(crate) struct ClaudeProjectScan {
    session: String,
    agent: Option<String>,
    line: Vec<u8>,
    invalid: bool,
    oversized: bool,
    paths: BTreeSet<PathBuf>,
}

impl ClaudeProjectScan {
    pub(crate) fn new(session: &str, agent: Option<&str>) -> Self {
        Self {
            session: session.into(),
            agent: agent.map(str::to_owned),
            line: Vec::new(),
            invalid: false,
            oversized: false,
            paths: BTreeSet::new(),
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Result<()> {
        for byte in bytes {
            if *byte == b'\n' {
                self.finish_line()?;
            } else if self.line.len() < PROJECT_RECORD_BYTES && !self.oversized {
                self.line.push(*byte);
            } else {
                self.oversized = true;
                self.invalid = true;
            }
        }
        Ok(())
    }

    fn finish_line(&mut self) -> Result<()> {
        if !self.oversized && !self.line.is_empty() {
            match serde_json::from_slice::<CwdRecord>(&self.line) {
                Ok(record) => self.inspect(record)?,
                Err(_) => self.invalid = true,
            }
        }
        self.line.clear();
        self.oversized = false;
        Ok(())
    }

    fn inspect(&mut self, record: CwdRecord) -> Result<()> {
        let owned = record.session.as_deref() == Some(self.session.as_str())
            && record.agent == self.agent
            && (self.agent.is_some() || !record.sidechain);
        if !owned {
            if record.session.is_none() && record.cwd.is_some() {
                self.invalid = true;
            }
            return Ok(());
        }
        if let Some(cwd) = record.cwd {
            if !valid_project_path(&cwd) {
                self.invalid = true;
            } else {
                self.paths.insert(PathBuf::from(cwd));
                if self.paths.len() > MAX_PROJECT_PATHS {
                    return Err(Error::Limit);
                }
            }
        }
        Ok(())
    }

    fn evidence(&self) -> ProjectEvidence {
        ProjectEvidence {
            paths: self.paths.iter().cloned().collect(),
            complete: !self.invalid && !self.oversized && self.line.is_empty(),
            source_digest: None,
        }
    }

    pub(crate) fn validate_complete(&self, selection: &ProjectSelection) -> Result<()> {
        if !selection.matches(&self.evidence())? {
            return Err(Error::SourceChanged);
        }
        Ok(())
    }
}

#[derive(Default)]
struct CwdRecord {
    session: Option<String>,
    agent: Option<String>,
    sidechain: bool,
    cwd: Option<String>,
}

impl<'de> Deserialize<'de> for CwdRecord {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct RecordVisitor;
        impl<'de> Visitor<'de> for RecordVisitor {
            type Value = CwdRecord;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a native record object")
            }

            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<CwdRecord, M::Error> {
                let mut record = CwdRecord::default();
                let mut fields = [false; 4];
                while let Some(key) = map.next_key::<String>()? {
                    let index = match key.as_str() {
                        "sessionId" => Some(0),
                        "agentId" => Some(1),
                        "isSidechain" => Some(2),
                        "cwd" => Some(3),
                        _ => None,
                    };
                    if let Some(index) = index
                        && std::mem::replace(&mut fields[index], true)
                    {
                        return Err(serde::de::Error::custom("duplicate native evidence"));
                    }
                    match key.as_str() {
                        "sessionId" => record.session = map.next_value()?,
                        "agentId" => record.agent = map.next_value()?,
                        "isSidechain" => record.sidechain = map.next_value()?,
                        "cwd" => record.cwd = map.next_value()?,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(record)
            }
        }
        deserializer.deserialize_map(RecordVisitor)
    }
}
