//! Capture independently authorized Cursor composer rows and CLI transcripts.

mod cli;
mod headers;
mod ide;
mod sqlite;

use crate::contract::safe_id;
use crate::discovery::CandidateSource;
use crate::{
    CAPTURE_VERSION, Candidate, CaptureMetadata, CaptureSourceKind, Error, Limits,
    MetadataSections, ProjectEvidence, ProjectSelection, Relationship, Result,
};
use std::path::Path;
use std::time::{Duration, Instant};

pub(super) const SOURCE_TIME: Duration = Duration::from_secs(30);
const MAX_CLI_PAGE: usize = 16;

/// The separate source bindings for the IDE and CLI native stores.
#[derive(Clone, Debug)]
pub(crate) struct CursorSource(CursorProvider);

/// Hold a Cursor CLI file generation without reading its payload.
pub(crate) struct CursorPin(cli::PinnedTranscript);

impl CursorPin {
    /// Return the pinned byte count for reservation.
    pub(crate) fn len(&self) -> u64 {
        self.0.len()
    }

    /// Return the metadata for comparing the scheduler's expected generation.
    pub(crate) fn metadata(&self) -> &std::fs::Metadata {
        self.0.metadata()
    }
}

#[derive(Clone, Debug)]
enum CursorProvider {
    Ide(ide::IdeSource),
    Cli(cli::CliSource),
}

impl CursorSource {
    /// Identify the native store layout without reading its payload.
    pub(crate) fn kind(&self) -> CaptureSourceKind {
        match &self.0 {
            CursorProvider::Ide(_) => CaptureSourceKind::CursorIdeComposer,
            CursorProvider::Cli(_) => CaptureSourceKind::CursorCliAgentJsonl,
        }
    }

    /// Return the display path for the primary database or CLI transcript.
    pub(crate) fn path(&self) -> &Path {
        match &self.0 {
            CursorProvider::Ide(source) => &source.path,
            CursorProvider::Cli(source) => &source.path,
        }
    }

    /// Return the exact native session ID.
    pub(crate) fn id(&self) -> &str {
        match &self.0 {
            CursorProvider::Ide(source) => &source.id,
            CursorProvider::Cli(source) => &source.id,
        }
    }

    /// Resolve bounded native project evidence without collecting session payload.
    pub(crate) fn project_evidence(&self, limits: &Limits) -> Result<ProjectEvidence> {
        let deadline = Instant::now() + SOURCE_TIME;
        match &self.0 {
            CursorProvider::Ide(source) => source.project_evidence(limits, deadline),
            CursorProvider::Cli(source) => source.project_evidence(limits, deadline),
        }
    }

    /// Pin files without reading payload. SQLite opens its snapshot after reservation.
    pub(crate) fn pin(&self, limits: &Limits) -> Result<Option<CursorPin>> {
        match &self.0 {
            CursorProvider::Ide(_) => Ok(None),
            CursorProvider::Cli(source) => source.pin(limits).map(CursorPin).map(Some),
        }
    }

    /// Recheck the current folder selection before exporting owned rows.
    pub(crate) fn export(
        &self,
        limits: &Limits,
        selection: &ProjectSelection,
        pin: Option<CursorPin>,
        emit: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let deadline = Instant::now() + SOURCE_TIME;
        match (&self.0, pin) {
            (CursorProvider::Ide(source), None) => source.export(limits, selection, deadline, emit),
            (CursorProvider::Cli(source), Some(CursorPin(pin))) => {
                source.export(limits, selection, deadline, pin, emit)
            }
            _ => Err(Error::InvalidSource),
        }
    }

    /// Describe source identity without claiming history or an installed runtime.
    pub(crate) fn metadata(&self, sections: MetadataSections) -> CaptureMetadata {
        CaptureMetadata {
            version: CAPTURE_VERSION,
            agent: "cursor".into(),
            source_format: match &self.0 {
                CursorProvider::Ide(_) => "CursorIdeComposer",
                CursorProvider::Cli(_) => "CursorCliAgentJsonl",
            }
            .into(),
            native_session_id: self.id().into(),
            native_agent_id: None,
            source_agent_version: None,
            version_observations: Vec::new(),
            version_scan_complete: false,
            owned_version_complete: false,
            relationship: Relationship::Unknown,
            history_completeness: "unknown".into(),
            sections,
        }
    }

    fn candidate(self) -> Candidate {
        let surface = match &self.0 {
            CursorProvider::Ide(_) => "ide",
            CursorProvider::Cli(_) => "cli",
        };
        Candidate {
            source_id: format!("cursor/{surface}/{}", self.id()),
            source: CandidateSource::Cursor(self),
            source_scope: None,
            project_paths: Vec::new(),
            project_selection: None,
        }
    }
}

/// One bounded page of exact IDE composer IDs from one authorized workspace.
#[derive(Debug)]
pub struct CursorIdeDiscoveryPage {
    /// Candidates in native ID order. Scope them before capture.
    pub candidates: Vec<Candidate>,
    /// Last visited native ID if another page exists. Bind it to the same root.
    pub next_cursor: Option<String>,
    /// At least one potential composer had ambiguous index proof.
    pub partial: bool,
}

/// One bounded page of CLI transcript candidates from one authorized project root.
#[derive(Debug)]
pub struct CursorCliDiscoveryPage {
    /// Candidates in native ID order. Scope them before capture.
    pub candidates: Vec<Candidate>,
    /// Last visited native ID if another page exists. Bind it to the same root.
    pub next_cursor: Option<String>,
    /// Native IDs visited, including entries without sidecar proof.
    pub visited: usize,
    /// At least one potential CLI source had ambiguous or unreadable proof.
    pub partial: bool,
}

/// List IDE composer identities from one authorized `workspaceStorage/<id>`.
///
/// The grant must also cover the paired `globalStorage/state.vscdb` under the
/// same Cursor `User` directory. Global SQL returns bounded identity data only.
/// Run on a bounded blocking worker. Restart a sweep to find earlier inserts.
pub fn discover_cursor_ide_page(
    workspace: &Path,
    after: Option<&str>,
    limits: &Limits,
) -> Result<CursorIdeDiscoveryPage> {
    limits.validate()?;
    if after.is_some_and(|id| !safe_id(id)) {
        return Err(Error::InvalidIdentity);
    }
    let (sources, next_cursor, partial) = ide::discover(workspace, after, limits)?;
    Ok(CursorIdeDiscoveryPage {
        candidates: sources
            .into_iter()
            .map(|source| CursorSource(CursorProvider::Ide(source)).candidate())
            .collect(),
        next_cursor,
        partial,
    })
}

/// Reopen an explicitly authorized IDE workspace and composer pair.
///
/// Scope and admit under the current folder policy before capture. Run on a
/// bounded blocking worker. The workspace grant must cover the paired global DB.
pub fn reopen_cursor_ide(workspace: &Path, id: &str, limits: &Limits) -> Result<Candidate> {
    limits.validate()?;
    if !safe_id(id) {
        return Err(Error::InvalidIdentity);
    }
    Ok(CursorSource(CursorProvider::Ide(ide::reopen(workspace, id)?)).candidate())
}

/// Reopen an exact CLI transcript below the authorized `agent-transcripts` root.
///
/// Authorize the paired chat metadata root under the same `.cursor` first.
/// Scope and admit under the current folder policy before capture. Run on a
/// bounded blocking worker.
pub fn reopen_cursor_cli(root: &Path, id: &str, limits: &Limits) -> Result<Candidate> {
    limits.validate()?;
    if !safe_id(id) {
        return Err(Error::InvalidIdentity);
    }
    let root = cli::CliRoot::open(root)?;
    let source = root
        .reopen(id, limits, Instant::now() + SOURCE_TIME)?
        .ok_or(Error::UnsupportedSource)?;
    Ok(CursorSource(CursorProvider::Cli(source)).candidate())
}

/// List exact CLI transcripts in one authorized `agent-transcripts` root.
///
/// It visits at most 16 session entries per page to bound retained directory
/// handles. It reads only same-ID metadata, never transcript content. Transcripts
/// without that sidecar or with no conversation are not candidates. Ambiguous
/// sources mark the page partial without blocking a healthy sibling.
/// Run on a bounded blocking worker. Scope and admit candidates independently.
pub fn discover_cursor_cli_page(
    root: &Path,
    after: Option<&str>,
    limits: &Limits,
) -> Result<CursorCliDiscoveryPage> {
    limits.validate()?;
    if after.is_some_and(|id| !safe_id(id)) {
        return Err(Error::InvalidIdentity);
    }
    let root = cli::CliRoot::open(root)?;
    let mut names = root.ids(limits)?;
    names.retain(|name| after.is_none_or(|after| name.as_str() > after));
    let page_size = limits.discovery_sessions.min(MAX_CLI_PAGE);
    let next_cursor = if names.len() > page_size {
        names.truncate(page_size);
        names.last().cloned()
    } else {
        None
    };
    let mut candidates = Vec::new();
    let mut partial = false;
    let deadline = Instant::now() + SOURCE_TIME;
    let visited = names.len();
    for id in names {
        crate::opencode::check_deadline(deadline)?;
        match root.reopen(&id, limits, deadline) {
            Ok(Some(source)) => {
                candidates.push(CursorSource(CursorProvider::Cli(source)).candidate());
            }
            Ok(None) | Err(Error::Io(std::io::ErrorKind::NotFound)) => {}
            Err(Error::Limit) => return Err(Error::Limit),
            Err(_) => partial = true,
        }
    }
    Ok(CursorCliDiscoveryPage {
        candidates,
        next_cursor,
        visited,
        partial,
    })
}
