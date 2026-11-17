//! Explicit source discovery and provider-bound candidate identities.
//!
//! Callers authorize the source first and schedule blocking discovery on bounded
//! workers. Discovery never searches homes or reads session payload.

use crate::contract::safe_id;
use crate::directory::{Directory, EntryKind};
use crate::{Error, Limits, Relationship, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The native source contract used by a candidate, not an installed agent version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureSourceKind {
    /// An exact Claude JSONL file with its optional owned child companion.
    ClaudeJsonl,
    /// Session-owned OpenCode V2 SQLite cells in the typed native stream.
    OpenCodeSqliteV2,
}

#[derive(Clone, Debug)]
/// Keep each provider's admission state without mixing file and database inputs.
pub(crate) enum CandidateSource {
    Claude(ClaudeSource),
    #[cfg(unix)]
    OpenCode(crate::opencode::OpenCodeSource),
}

#[derive(Clone, Debug)]
/// A Claude layout binding with retained directory handles and direct child proof.
pub(crate) struct ClaudeSource {
    pub(crate) path: PathBuf,
    pub(crate) directory: Arc<Directory>,
    pub(crate) filename: String,
    pub(crate) companion: Option<String>,
    pub(crate) native_session_id: String,
    pub(crate) native_agent_id: Option<String>,
    pub(crate) relationship: Relationship,
}

/// A provider-bound source identity, not an open session body.
///
/// Claude discovery reads directory metadata and retains admitted handles.
/// OpenCode discovery reads bounded session IDs, not session payload. Capture
/// uses the provider binding; callers must not read payload through a display
/// path. Rediscover when the host needs a new source listing.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub(crate) source: CandidateSource,
    pub(crate) source_id: String,
    pub(crate) source_scope: Option<String>,
}

impl Candidate {
    /// Return the device-local key to use in [`crate::CaptureIdentity`].
    ///
    /// Hosts must first call [`Self::in_source_scope`] to separate accounts and
    /// authorized source roots that contain the same native filename IDs.
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Identify the native provider contract without opening its content.
    pub fn source_kind(&self) -> CaptureSourceKind {
        match &self.source {
            CandidateSource::Claude(_) => CaptureSourceKind::ClaudeJsonl,
            #[cfg(unix)]
            CandidateSource::OpenCode(_) => CaptureSourceKind::OpenCodeSqliteV2,
        }
    }

    /// Display path for the transcript or source database, not an upload body.
    ///
    /// A database path can appear on many independent session candidates.
    pub fn source_path(&self) -> &Path {
        match &self.source {
            CandidateSource::Claude(source) => &source.path,
            #[cfg(unix)]
            CandidateSource::OpenCode(source) => source.source_path(),
        }
    }

    /// Return the provider's native session ID without inferring a repository.
    pub fn native_session_id(&self) -> &str {
        match &self.source {
            CandidateSource::Claude(source) => &source.native_session_id,
            #[cfg(unix)]
            CandidateSource::OpenCode(source) => source.native_session_id(),
        }
    }

    /// Return a Claude child filename ID; OpenCode personas are not source agents.
    pub fn native_agent_id(&self) -> Option<&str> {
        match &self.source {
            CandidateSource::Claude(source) => source.native_agent_id.as_deref(),
            #[cfg(unix)]
            CandidateSource::OpenCode(_) => None,
        }
    }

    /// Bind native keys to one stable account and device-local source root.
    /// Keep this source_scope_id when a directory access grant is replaced.
    ///
    /// The host owns and persists the scope ID. It is not a grant or repository
    /// ID. A candidate can be scoped only once. Invalid or repeated scoping
    /// returns [`Error::InvalidIdentity`] without changing the original candidate.
    pub fn in_source_scope(&self, scope: &str) -> Result<Self> {
        if !safe_id(scope) || self.source_scope.is_some() {
            return Err(Error::InvalidIdentity);
        }
        let mut candidate = self.clone();
        candidate.source_scope = Some(scope.into());
        candidate.source_id = candidate.scoped_key(&candidate.source_id);
        match &mut candidate.source {
            CandidateSource::Claude(source) => {
                if let Relationship::Child {
                    parent_source_id, ..
                } = &mut source.relationship
                {
                    *parent_source_id = format!("{scope}/{parent_source_id}");
                }
            }
            #[cfg(unix)]
            CandidateSource::OpenCode(_) => {}
        }
        Ok(candidate)
    }

    pub(crate) fn scoped_key(&self, native_key: &str) -> String {
        match &self.source_scope {
            Some(scope) => format!("{scope}/{native_key}"),
            None => native_key.into(),
        }
    }
}

/// Inspect one explicitly authorized Claude `projects/<project>` directory.
/// Each transcript, including a child or fork, is a separate candidate.
///
/// `project` must be an absolute path with no symlink components. The host must
/// authorize this root first; discovery never locates home or agent directories.
/// Only admitted transcripts and exact same-stem child companions enter capture.
/// Non-UTF-8 filenames are outside that layout but still count toward the bound.
///
/// Bounds apply across root and child entries. Exceeding either bound returns
/// [`Error::Limit`] rather than a partial result. Filesystem failures can also
/// fail discovery. Non-Unix systems return [`Error::UnsupportedPlatform`].
pub fn discover_claude(project: &Path, limits: &Limits) -> Result<Vec<Candidate>> {
    limits.validate()?;
    discover_directory(project, Arc::new(Directory::source(project)?), limits)
}

/// Discover bounded OpenCode V2 session identities in one authorized database.
///
/// The host must authorize this exact source before discovery. Discovery does
/// not read session payload or parent content. Each root, child, and fork gets
/// its own candidate. Scope candidates before assigning capture identities.
/// Unsupported or unsafe source schemas return [`Error::UnsupportedSource`].
/// Non-Unix systems return [`Error::UnsupportedPlatform`].
pub fn discover_opencode(database: &Path, limits: &Limits) -> Result<Vec<Candidate>> {
    limits.validate()?;
    #[cfg(unix)]
    {
        Ok(crate::opencode::discover(database, limits)?
            .into_iter()
            .map(|source| Candidate {
                source_id: source.native_key().into(),
                source: CandidateSource::OpenCode(source),
                source_scope: None,
            })
            .collect())
    }
    #[cfg(not(unix))]
    {
        let _ = database;
        Err(Error::UnsupportedPlatform)
    }
}

fn discover_directory(
    project: &Path,
    root: Arc<Directory>,
    limits: &Limits,
) -> Result<Vec<Candidate>> {
    let names = root.source_names(limits.discovery_entries)?;
    let mut visited = names.len();
    let mut candidates = Vec::new();
    for name in names {
        let Ok(name) = name.into_string() else {
            continue;
        };
        let kind = root.kind(&name)?;
        if kind == EntryKind::File {
            let Some(id) = transcript_id(Path::new(&name)) else {
                continue;
            };
            if id != "audit" {
                push_candidate(
                    &mut candidates,
                    Candidate {
                        source: CandidateSource::Claude(ClaudeSource {
                            path: project.join(&name),
                            directory: root.clone(),
                            filename: name,
                            companion: None,
                            native_session_id: id.clone(),
                            native_agent_id: None,
                            relationship: Relationship::Unknown,
                        }),
                        source_id: id,
                        source_scope: None,
                    },
                    limits,
                )?;
            }
            continue;
        }
        if kind != EntryKind::Directory || !safe_id(&name) {
            continue;
        }
        let id = name.as_str();
        let session_directory = root.source_dir(id)?;
        let children = match session_directory.source_dir("subagents") {
            Ok(directory) => Arc::new(directory),
            Err(Error::Io(std::io::ErrorKind::NotFound)) => continue,
            Err(error) => return Err(error),
        };
        let child_names = children.source_names(limits.discovery_entries - visited)?;
        visited += child_names.len();
        for child_name in child_names {
            let Ok(child_name) = child_name.into_string() else {
                continue;
            };
            if children.kind(&child_name)? != EntryKind::File {
                continue;
            }
            let Some(agent_id) = transcript_id(Path::new(&child_name)) else {
                continue;
            };
            if !agent_id.starts_with("agent-") || agent_id.len() == "agent-".len() {
                continue;
            }
            let companion = format!("{agent_id}.meta.json");
            let companion = match children.kind(&companion) {
                Ok(EntryKind::File) => Some(companion),
                Ok(_) => return Err(Error::InvalidSource),
                Err(Error::Io(std::io::ErrorKind::NotFound)) => None,
                Err(error) => return Err(error),
            };
            push_candidate(
                &mut candidates,
                Candidate {
                    source: CandidateSource::Claude(ClaudeSource {
                        path: project.join(id).join("subagents").join(&child_name),
                        directory: children.clone(),
                        filename: child_name,
                        companion,
                        native_session_id: id.into(),
                        native_agent_id: Some(agent_id["agent-".len()..].into()),
                        relationship: Relationship::Child {
                            parent_source_id: id.into(),
                            proof: "claude-subagents-layout-v1".into(),
                        },
                    }),
                    source_id: format!("{id}/subagents/{agent_id}"),
                    source_scope: None,
                },
                limits,
            )?;
        }
    }
    candidates.sort_by(|left, right| left.source_id.cmp(&right.source_id));
    Ok(candidates)
}

fn transcript_id(path: &Path) -> Option<String> {
    if path.extension()?.to_str()? != "jsonl" {
        return None;
    }
    let id = path.file_stem()?.to_str()?;
    safe_id(id).then(|| id.to_owned())
}

fn push_candidate(
    candidates: &mut Vec<Candidate>,
    candidate: Candidate,
    limits: &Limits,
) -> Result<()> {
    if candidates.len() >= limits.discovery_sessions {
        return Err(Error::Limit);
    }
    candidates.push(candidate);
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
    fn discovery_enumerates_the_admitted_handle_after_ancestor_replacement() {
        let temporary = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let project = temporary.path().join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("original.jsonl"), b"{}\n").unwrap();
        let admitted = Arc::new(Directory::source(&project).unwrap());
        fs::rename(&project, temporary.path().join("moved")).unwrap();
        let decoy = temporary.path().join("decoy");
        fs::create_dir(&decoy).unwrap();
        fs::write(decoy.join("outside.jsonl"), b"{}\n").unwrap();
        symlink(&decoy, &project).unwrap();
        let candidates = discover_directory(&project, admitted, &Limits::default()).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].source_id(), "original");
    }
}
