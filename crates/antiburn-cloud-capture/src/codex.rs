//! Admission and capture metadata for one Codex rollout.

use crate::contract::safe_id;
use crate::discovery::CandidateSource;
use crate::header_file::{HeaderFileSource, parse_object, read_first_line};
use crate::{
    CAPTURE_VERSION, Candidate, CaptureMetadata, Error, Limits, MetadataSections, ProjectEvidence,
    Relationship, Result, VersionObservation, VersionOwnership,
};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::fs::File;
use std::path::Path;

/// Re-admit one authorized Codex rollout by its exact path and expected session ID.
///
/// A bounded first-line read must prove the expected `session_meta.payload.id`.
/// The path must name an absolute `rollout-*.jsonl` file without symlink
/// components. This does not scan a tree, use other files, or grant project
/// permission. Scope and admit the result before capture. Run on a bounded
/// blocking worker. Non-Unix systems return [`Error::UnsupportedPlatform`].
pub fn reopen_codex(rollout: &Path, native_session_id: &str, limits: &Limits) -> Result<Candidate> {
    limits.validate()?;
    let source = HeaderFileSource::new(rollout, native_session_id, |name| {
        name.starts_with("rollout-") && name.ends_with(".jsonl")
    })?;
    source.project_evidence(limits, project_evidence)?;
    Ok(Candidate {
        source_id: native_key(native_session_id),
        source: CandidateSource::Codex(source),
        source_scope: None,
        project_paths: Vec::new(),
        project_selection: None,
    })
}

fn native_key(id: &str) -> String {
    format!("codex/{id}")
}

/// Read only the owned rollout header for project evidence.
pub(crate) fn project_evidence(file: &mut File, id: &str) -> Result<ProjectEvidence> {
    Ok(read_header(file, id)?.project_evidence())
}

/// Prove authority from the first physical line only. Never scan for a match.
pub(crate) fn read_header(file: &mut File, id: &str) -> Result<CodexHeader> {
    let line = read_first_line(file)?;
    let record: HeaderRecord<'_> = parse_object(&line).ok_or(Error::InvalidSource)?;
    if record.kind != "session_meta" || record.payload.id != id {
        return Err(Error::InvalidSource);
    }
    Ok(record.payload.into())
}

#[derive(Deserialize)]
struct HeaderRecord<'a> {
    #[serde(rename = "type")]
    kind: String,
    #[serde(borrow, deserialize_with = "deserialize_object")]
    payload: HeaderPayload<'a>,
}

fn deserialize_object<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let raw = <&RawValue>::deserialize(deserializer)?;
    parse_object(raw.get()).ok_or_else(|| serde::de::Error::custom("invalid native object"))
}

#[derive(Deserialize)]
struct HeaderPayload<'a> {
    id: String,
    #[serde(borrow)]
    cwd: Option<&'a RawValue>,
    #[serde(borrow)]
    cli_version: Option<&'a RawValue>,
    #[serde(borrow)]
    forked_from_id: Option<&'a RawValue>,
    #[serde(borrow)]
    source: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct SpawnSource {
    #[serde(deserialize_with = "deserialize_object")]
    subagent: SpawnAgent,
}

#[derive(Deserialize)]
struct SpawnAgent {
    #[serde(deserialize_with = "deserialize_object")]
    thread_spawn: SpawnThread,
}

#[derive(Deserialize)]
struct SpawnThread {
    parent_thread_id: String,
}

/// Evidence from the first owned header, not from replayed records.
pub(crate) struct CodexHeader {
    cwd: Option<String>,
    cli_version: Option<String>,
    fork_parent: Option<String>,
    child_parent: Option<String>,
}

impl From<HeaderPayload<'_>> for CodexHeader {
    fn from(payload: HeaderPayload<'_>) -> Self {
        fn string(value: Option<&RawValue>) -> Option<String> {
            serde_json::from_str(value?.get()).ok()
        }
        Self {
            cwd: string(payload.cwd),
            cli_version: string(payload.cli_version)
                .filter(|version| safe_id(&version.replace('.', "-"))),
            fork_parent: string(payload.forked_from_id).filter(|id| safe_id(id)),
            child_parent: payload
                .source
                .and_then(|source| parse_object::<SpawnSource>(source.get()))
                .map(|source| source.subagent.thread_spawn.parent_thread_id)
                .filter(|id| safe_id(id)),
        }
    }
}

impl CodexHeader {
    pub(crate) fn project_evidence(&self) -> ProjectEvidence {
        match &self.cwd {
            Some(cwd) => ProjectEvidence::recorded(cwd),
            None => ProjectEvidence::unknown(),
        }
    }

    /// Report only the producer observation and direct edges proved by this header.
    pub(crate) fn metadata(
        self,
        candidate: &Candidate,
        sections: MetadataSections,
    ) -> CaptureMetadata {
        let id = candidate.native_session_id();
        let relationship = match (self.fork_parent, self.child_parent) {
            (Some(parent), None) if parent != id => Relationship::Fork {
                parent_source_id: candidate.scoped_key(&native_key(&parent)),
                proof: "codex-session-meta-fork-v1".into(),
            },
            (None, Some(parent)) if parent != id => Relationship::Child {
                parent_source_id: candidate.scoped_key(&native_key(&parent)),
                proof: "codex-session-meta-thread-spawn-v1".into(),
            },
            _ => Relationship::Unknown,
        };
        CaptureMetadata {
            version: CAPTURE_VERSION,
            agent: "codex".into(),
            source_format: "CodexJsonl".into(),
            native_session_id: id.into(),
            native_agent_id: None,
            source_agent_version: None,
            version_observations: self
                .cli_version
                .into_iter()
                .map(|version| VersionObservation {
                    version,
                    ownership: VersionOwnership::Owned,
                })
                .collect(),
            version_scan_complete: false,
            owned_version_complete: false,
            relationship,
            history_completeness: "unknown".into(),
            sections,
        }
    }
}
