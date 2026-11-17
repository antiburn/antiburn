use crate::contract::safe_id;
use crate::directory::{Directory, EntryKind};
use crate::{Error, Limits, Relationship, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A path-only discovery result. Discovery never reads native content.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub(crate) transcript: PathBuf,
    pub(crate) directory: Arc<Directory>,
    pub(crate) filename: String,
    pub(crate) companion: Option<String>,
    pub(crate) native_session_id: String,
    pub(crate) native_agent_id: Option<String>,
    pub(crate) source_id: String,
    pub(crate) source_scope: Option<String>,
    pub(crate) relationship: Relationship,
}

impl Candidate {
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// Display path only. Capture uses the admitted directory handle.
    pub fn transcript_path(&self) -> &Path {
        &self.transcript
    }

    /// Bind native keys to one stable account and device-local source root.
    /// Keep this source_scope_id when a directory access grant is replaced.
    pub fn in_source_scope(&self, scope: &str) -> Result<Self> {
        if !safe_id(scope) || self.source_scope.is_some() {
            return Err(Error::InvalidIdentity);
        }
        let mut candidate = self.clone();
        candidate.source_scope = Some(scope.into());
        candidate.source_id = candidate.scoped_key(&candidate.source_id);
        if let Relationship::Child {
            parent_source_id,
            proof,
        } = &self.relationship
        {
            candidate.relationship = Relationship::Child {
                parent_source_id: candidate.scoped_key(parent_source_id),
                proof: proof.clone(),
            };
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

/// Inspect one explicitly authorized Claude projects/<project> directory.
/// Each transcript, including a child or fork, is a separate candidate.
pub fn discover_claude(project: &Path, limits: &Limits) -> Result<Vec<Candidate>> {
    limits.validate()?;
    discover_directory(project, Arc::new(Directory::source(project)?), limits)
}

fn discover_directory(
    project: &Path,
    root: Arc<Directory>,
    limits: &Limits,
) -> Result<Vec<Candidate>> {
    let names = root.names(limits.discovery_entries)?;
    let mut visited = names.len();
    let mut candidates = Vec::new();
    for name in names {
        let kind = root.kind(&name)?;
        if kind == EntryKind::File {
            let Some(id) = transcript_id(Path::new(&name)) else {
                continue;
            };
            if id != "audit" {
                push_candidate(
                    &mut candidates,
                    Candidate {
                        transcript: project.join(&name),
                        directory: root.clone(),
                        filename: name,
                        companion: None,
                        native_session_id: id.clone(),
                        native_agent_id: None,
                        source_id: id,
                        source_scope: None,
                        relationship: Relationship::Unknown,
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
        let child_names = children.names(limits.discovery_entries - visited)?;
        visited += child_names.len();
        for child_name in child_names {
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
                    transcript: project.join(id).join("subagents").join(&child_name),
                    directory: children.clone(),
                    filename: child_name,
                    companion,
                    native_session_id: id.into(),
                    native_agent_id: Some(agent_id["agent-".len()..].into()),
                    source_id: format!("{id}/subagents/{agent_id}"),
                    source_scope: None,
                    relationship: Relationship::Child {
                        parent_source_id: id.into(),
                        proof: "claude-subagents-layout-v1".into(),
                    },
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
