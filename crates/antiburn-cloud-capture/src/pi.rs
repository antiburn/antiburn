//! Admission and capture metadata for one Pi JSONL session.

use crate::discovery::CandidateSource;
use crate::header_file::{HeaderFileSource, parse_object, read_first_line};
use crate::{
    CAPTURE_VERSION, Candidate, CaptureMetadata, Error, Limits, MetadataSections, ProjectEvidence,
    Relationship, Result,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::fs::File;
use std::path::Path;

/// Re-admit one authorized Pi session by its exact path and expected session ID.
///
/// The absolute filename must end in `_<id>.jsonl`, with the ID equal to the
/// full part after the last underscore. The first physical line must be a
/// bounded `type: "session"` object with the same `id`. This does not search
/// for sessions, distinguish compatible producers, or grant project permission.
/// Scope and admit the result before capture. Run on a bounded blocking worker.
/// Non-Unix systems return [`Error::UnsupportedPlatform`].
pub fn reopen_pi(path: &Path, native_session_id: &str, limits: &Limits) -> Result<Candidate> {
    limits.validate()?;
    let source = HeaderFileSource::new(path, native_session_id, |filename| {
        filename
            .strip_suffix(".jsonl")
            .and_then(|stem| stem.rsplit_once('_'))
            .is_some_and(|(prefix, id)| !prefix.is_empty() && id == native_session_id)
    })?;
    source.project_evidence(limits, project_evidence)?;
    Ok(Candidate {
        source_id: format!("pi/{native_session_id}"),
        source: CandidateSource::Pi(source),
        source_scope: None,
        project_paths: Vec::new(),
        project_selection: None,
    })
}

#[derive(Deserialize)]
struct SessionHeader<'a> {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    #[serde(borrow)]
    cwd: Option<&'a RawValue>,
}

/// Prove the ID and read CWD from the same header. Later lines have no authority.
pub(crate) fn project_evidence(file: &mut File, id: &str) -> Result<ProjectEvidence> {
    let line = read_first_line(file)?;
    let header: SessionHeader<'_> = parse_object(&line).ok_or(Error::InvalidSource)?;
    if header.kind != "session" || header.id != id {
        return Err(Error::InvalidSource);
    }
    let cwd = header
        .cwd
        .and_then(|value| serde_json::from_str::<String>(value.get()).ok());
    Ok(match cwd.as_deref() {
        Some(path) => ProjectEvidence::recorded(path),
        None => ProjectEvidence::unknown(),
    })
}

/// Pi branch and tool IDs identify records, not separate captured workers.
pub(crate) fn metadata(id: &str, sections: MetadataSections) -> CaptureMetadata {
    CaptureMetadata {
        version: CAPTURE_VERSION,
        agent: "pi".into(),
        source_format: "PiJsonl".into(),
        native_session_id: id.into(),
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
