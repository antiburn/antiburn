//! Exact CLI transcripts admitted by same-session chat metadata.

use super::safe_id;
use crate::directory::{Directory, EntryKind};
use crate::filesystem::same_source;
use crate::opencode::check_deadline;
use crate::{Error, Limits, METADATA_BYTES, ProjectEvidence, ProjectSelection, Result};
use md5::{Digest, Md5};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// Bind the transcript and its folder proof to the same authorized Cursor root.
#[derive(Debug)]
pub(super) struct CliRoot {
    path: PathBuf,
    project_key: String,
    projects: Directory,
    transcripts: Directory,
    chats: Directory,
}

/// Keep exact source handles; the resolved metadata workspace cannot change.
#[derive(Clone, Debug)]
pub(super) struct CliSource {
    pub(super) path: PathBuf,
    pub(super) id: String,
    root: Arc<CliRoot>,
    directory: Arc<Directory>,
    metadata_workspace: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatMetadata {
    schema_version: u32,
    has_conversation: bool,
    cwd: String,
}

#[derive(Deserialize)]
struct FirstRecord<'a> {
    role: String,
    #[serde(borrow)]
    message: FirstMessage<'a>,
}

#[derive(Deserialize)]
struct FirstMessage<'a> {
    #[serde(borrow)]
    content: &'a RawValue,
}

/// Hold one transcript generation across reservation without reading its bytes.
pub(super) struct PinnedTranscript {
    file: File,
    before: Metadata,
}

impl PinnedTranscript {
    pub(super) fn len(&self) -> u64 {
        self.before.len()
    }

    /// Return the file generation pinned across reservation.
    pub(super) fn metadata(&self) -> &Metadata {
        &self.before
    }
}

impl CliRoot {
    /// Open the transcript root and chat metadata root relative to one `.cursor`.
    pub(super) fn open(path: &Path) -> Result<Arc<Self>> {
        let project = path.parent().ok_or(Error::InvalidSource)?;
        let projects = project.parent().ok_or(Error::InvalidSource)?;
        let cursor = projects.parent().ok_or(Error::InvalidSource)?;
        if path
            .file_name()
            .is_none_or(|name| name != "agent-transcripts")
            || projects.file_name().is_none_or(|name| name != "projects")
            || cursor.file_name().is_none_or(|name| name != ".cursor")
        {
            return Err(Error::InvalidSource);
        }
        let project = project
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(Error::InvalidSource)?;
        let cursor = Directory::source(cursor)?;
        let projects = cursor.source_dir("projects")?;
        let transcripts = projects
            .source_dir(project)?
            .source_dir("agent-transcripts")?;
        let chats = cursor.source_dir("chats")?;
        Ok(Arc::new(Self {
            path: path.to_owned(),
            project_key: project.into(),
            projects,
            transcripts,
            chats,
        }))
    }

    /// Enumerate bounded exact session directories without opening transcripts.
    pub(super) fn ids(&self, limits: &Limits) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for name in self.transcripts.source_names(limits.discovery_entries)? {
            let Ok(id) = name.into_string() else { continue };
            if !safe_id(&id) {
                continue;
            }
            match self.transcripts.kind(&id) {
                Ok(EntryKind::Directory) => ids.push(id),
                Ok(_) | Err(Error::Io(ErrorKind::NotFound)) => {}
                Err(error) => return Err(error),
            }
        }
        ids.sort_unstable();
        Ok(ids)
    }

    /// Bind one transcript and look up the unique same-ID chat sidecar.
    /// Return `None` when a supported sidecar declares no conversation.
    pub(super) fn reopen(
        self: &Arc<Self>,
        id: &str,
        limits: &Limits,
        deadline: Instant,
    ) -> Result<Option<CliSource>> {
        let directory = self.transcripts.source_dir(id)?;
        if directory.kind(&format!("{id}.jsonl"))? != EntryKind::File {
            return Err(Error::InvalidSource);
        }
        let Some((metadata_workspace, _)) = self.metadata(id, limits, deadline)? else {
            return Ok(None);
        };
        Ok(Some(CliSource {
            path: self.path.join(id).join(format!("{id}.jsonl")),
            id: id.into(),
            root: self.clone(),
            directory: Arc::new(directory),
            metadata_workspace,
        }))
    }

    /// Only an exact metadata filename in one matching session path can prove CWD.
    /// A supported sidecar without a conversation is not an inspection failure.
    fn metadata(
        &self,
        id: &str,
        limits: &Limits,
        deadline: Instant,
    ) -> Result<Option<(String, ProjectEvidence)>> {
        let mut matched = None;
        for name in self.chats.source_names(limits.discovery_entries)? {
            check_deadline(deadline)?;
            let Ok(name) = name.into_string() else {
                continue;
            };
            match self.chats.kind(&name) {
                Ok(EntryKind::Directory) => {}
                Ok(_) | Err(Error::Io(ErrorKind::NotFound)) => continue,
                Err(error) => return Err(error),
            }
            let session = match self
                .chats
                .source_dir(&name)
                .and_then(|workspace| workspace.source_dir(id))
            {
                Ok(session) => session,
                Err(Error::Io(ErrorKind::NotFound)) => continue,
                Err(error) => return Err(error),
            };
            let kind = match session.kind("meta.json") {
                Ok(kind) => kind,
                Err(Error::Io(ErrorKind::NotFound)) => continue,
                Err(error) => return Err(error),
            };
            if kind != EntryKind::File || matched.is_some() {
                return Err(Error::UnsupportedSource);
            }
            matched = Some((name, session));
        }
        let (name, directory) = matched.ok_or(Error::Io(ErrorKind::NotFound))?;
        let file = directory.source_file("meta.json")?;
        let before = file.metadata()?;
        if !same_source(&before, &before) {
            return Err(Error::InvalidSource);
        }
        let maximum = METADATA_BYTES.min(limits.source_bytes);
        let mut bytes = Vec::new();
        let mut reader = file.take(maximum + 1);
        reader.read_to_end(&mut bytes)?;
        if bytes.len() as u64 > maximum {
            // Only this sidecar is unsupported. Directory and time limits still fail the page.
            return Err(Error::UnsupportedSource);
        }
        if !same_source(&before, &reader.into_inner().metadata()?) {
            return Err(Error::SourceChanged);
        }
        let metadata: ChatMetadata =
            serde_json::from_slice(&bytes).map_err(|_| Error::UnsupportedSource)?;
        if metadata.schema_version != 1 {
            return Err(Error::UnsupportedSource);
        }
        if !metadata.has_conversation {
            return Ok(None);
        }
        let evidence = ProjectEvidence::recorded(&metadata.cwd);
        let cwd = evidence
            .project_path()
            .and_then(Path::to_str)
            .ok_or(Error::UnsupportedSource)?;
        let project_key = cwd
            .strip_prefix('/')
            .filter(|path| !path.is_empty())
            .ok_or(Error::UnsupportedSource)?
            .replace('/', "-");
        if self.project_key != project_key || name != crate::contract::hex(&Md5::digest(cwd)) {
            return Err(Error::SourceChanged);
        }
        self.verify_unique_transcript(id, limits, deadline)?;
        Ok(Some((name, evidence)))
    }

    /// A copied transcript with the same ID in another project cannot inherit its CWD.
    fn verify_unique_transcript(&self, id: &str, limits: &Limits, deadline: Instant) -> Result<()> {
        let file = format!("{id}.jsonl");
        for project in self.projects.source_names(limits.discovery_entries)? {
            check_deadline(deadline)?;
            let Ok(project) = project.into_string() else {
                continue;
            };
            if project == self.project_key {
                continue;
            }
            match self.projects.kind(&project) {
                Ok(EntryKind::Directory) => {}
                Ok(_) | Err(Error::Io(ErrorKind::NotFound)) => continue,
                Err(error) => return Err(error),
            }
            let kind = self
                .projects
                .source_dir(&project)
                .and_then(|directory| directory.source_dir("agent-transcripts"))
                .and_then(|directory| directory.source_dir(id))
                .and_then(|directory| directory.kind(&file));
            match kind {
                Err(Error::Io(ErrorKind::NotFound)) => {}
                Ok(_) => return Err(Error::UnsupportedSource),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

impl CliSource {
    /// Check the current exact sidecar without opening the transcript.
    pub(super) fn project_evidence(
        &self,
        limits: &Limits,
        deadline: Instant,
    ) -> Result<ProjectEvidence> {
        let (workspace, evidence) = self
            .root
            .metadata(&self.id, limits, deadline)?
            .ok_or(Error::UnsupportedSource)?;
        if workspace != self.metadata_workspace {
            return Err(Error::SourceChanged);
        }
        Ok(evidence)
    }

    /// Open and bound the exact file before reservation. This reads no payload.
    pub(super) fn pin(&self, limits: &Limits) -> Result<PinnedTranscript> {
        let filename = format!("{}.jsonl", self.id);
        let file = self.directory.source_file(&filename)?;
        let before = file.metadata()?;
        if !same_source(&before, &before) || before.len() == 0 {
            return Err(Error::InvalidSource);
        }
        if before.len() > limits.source_bytes {
            return Err(Error::Limit);
        }
        Ok(PinnedTranscript { file, before })
    }

    /// Recheck sidecar proof and stream exact bytes from one pinned transcript.
    pub(super) fn export(
        &self,
        limits: &Limits,
        selection: &ProjectSelection,
        deadline: Instant,
        pinned: PinnedTranscript,
        mut emit: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let evidence = self.project_evidence(limits, deadline)?;
        if !selection.matches(&evidence)? {
            return Err(Error::SourceChanged);
        }
        let PinnedTranscript { mut file, before } = pinned;
        let filename = format!("{}.jsonl", self.id);
        if !same_source(&before, &file.metadata()?)
            || !same_source(&before, &self.directory.source_file(&filename)?.metadata()?)
        {
            return Err(Error::SourceChanged);
        }
        export_records(
            (&mut file).take(before.len()),
            before.len(),
            deadline,
            &mut emit,
        )?;
        if !same_source(&before, &file.metadata()?)
            || !same_source(&before, &self.directory.source_file(&filename)?.metadata()?)
            || self.project_evidence(limits, deadline)? != evidence
        {
            return Err(Error::SourceChanged);
        }
        check_deadline(deadline)
    }
}

/// Check the observed first record and every JSONL boundary without changing bytes.
fn export_records(
    file: impl Read,
    mut remaining: u64,
    deadline: Instant,
    mut emit: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let record_limit = crate::PROJECT_RECORD_BYTES as u64;
    let mut reader = BufReader::new(file);
    let mut record = Vec::new();
    let mut first = true;
    while remaining > 0 {
        check_deadline(deadline)?;
        record.clear();
        let bytes = (&mut reader)
            .take(record_limit + 1)
            .read_until(b'\n', &mut record)? as u64;
        if bytes == 0 || record.last() != Some(&b'\n') {
            return Err(if bytes > record_limit {
                Error::Limit
            } else {
                Error::InvalidSource
            });
        }
        if bytes > record_limit {
            return Err(Error::Limit);
        }
        let raw: &RawValue = serde_json::from_slice(&record).map_err(|_| Error::InvalidSource)?;
        if !raw.get().starts_with('{') {
            return Err(Error::InvalidSource);
        }
        if first {
            let start: FirstRecord<'_> =
                serde_json::from_str(raw.get()).map_err(|_| Error::InvalidSource)?;
            if start.role != "user" || !start.message.content.get().starts_with('[') {
                return Err(Error::InvalidSource);
            }
            first = false;
        }
        emit(&record)?;
        remaining -= bytes;
    }
    Ok(())
}
