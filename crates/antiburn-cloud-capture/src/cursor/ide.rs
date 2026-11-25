//! Cursor IDE workspace membership and exact global composer rows.

use super::headers::{HeaderLog, MAX_BUBBLES};
use super::sqlite::{
    PROOF_BYTES, WorkspaceProof, composer_index, exact_row, indexed_workspace, proof_bytes, rowid,
    workspace_index_proof,
};
use super::{SOURCE_TIME, safe_id};
use crate::directory::Directory;
use crate::filesystem::same_source;
use crate::opencode::{AdmittedDatabase, check_deadline, sql_error};
use crate::sqlite_native::SqliteNativeWriter;
use crate::{Error, Limits, ProjectEvidence, ProjectSelection, Result};
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

const INDEX_KEY: &str = "composer.composerData";

#[derive(Debug)]
struct Workspace {
    global_path: PathBuf,
    id: String,
    directory: Directory,
    database: AdmittedDatabase,
    global: AdmittedDatabase,
}

/// Keep the paired database and workspace directory bindings across capture.
#[derive(Clone, Debug)]
pub(super) struct IdeSource {
    pub(super) path: PathBuf,
    pub(super) id: String,
    workspace: Arc<Workspace>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComposerIndex {
    all_composers: Option<Vec<ComposerId>>,
    has_migrated_composer_data: Option<bool>,
    selected_composer_ids: Option<Vec<String>>,
    last_focused_composer_ids: Option<Vec<String>>,
}

#[derive(Default)]
struct Membership {
    local: BTreeSet<String>,
    focused: BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComposerId {
    composer_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComposerIdentity {
    composer_id: Option<String>,
    workspace_id: Option<String>,
    workspace_identifier: Option<WorkspaceIdentifier>,
    has_inline_headers_only: bool,
    has_chunked_headers: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Composer<'a> {
    full_conversation_headers_only: Vec<BubbleId>,
    header_log: Option<HeaderLog>,
    #[serde(borrow)]
    conversation: Option<&'a RawValue>,
    #[serde(borrow)]
    conversation_map: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct WorkspaceIdentifier {
    id: String,
    uri: WorkspaceUri,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceUri {
    scheme: String,
    fs_path: String,
    path: String,
    external: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BubbleId {
    bubble_id: String,
}

#[derive(Deserialize)]
struct WorkspaceFolder {
    folder: String,
    workspace: Option<serde::de::IgnoredAny>,
}

impl Workspace {
    fn open(path: &Path) -> Result<Self> {
        let id = path.file_name().and_then(|name| name.to_str());
        let id = id.filter(|id| safe_id(id)).ok_or(Error::InvalidSource)?;
        let storage = path.parent().ok_or(Error::InvalidSource)?;
        if storage
            .file_name()
            .is_none_or(|name| name != "workspaceStorage")
        {
            return Err(Error::InvalidSource);
        }
        let user = storage.parent().ok_or(Error::InvalidSource)?;
        if user.file_name().is_none_or(|name| name != "User") {
            return Err(Error::InvalidSource);
        }
        let directory = Directory::source(path)?;
        let database = AdmittedDatabase::open(&path.join("state.vscdb"))?;
        let global_path = user.join("globalStorage/state.vscdb");
        let global = AdmittedDatabase::open(&global_path)?;
        directory.verify_identity(database.ancestor_handle(path)?)?;
        database.verify_shared_ancestor(&global, user)?;
        Ok(Self {
            global_path,
            id: id.into(),
            directory,
            database,
            global,
        })
    }

    /// A migrated focus list is not workspace ownership on its own.
    fn membership(&self, db: &Connection, limits: &Limits) -> Result<Membership> {
        let locator = rowid(db, "ItemTable", "key", "value")?;
        let row = match exact_row(db, "ItemTable", locator, INDEX_KEY) {
            Ok(row) => row,
            Err(Error::SourceChanged) => return Ok(Membership::default()),
            Err(error) => return Err(error),
        };
        let bytes = proof_bytes(db, "ItemTable", locator, row, limits.source_bytes)?;
        let index: ComposerIndex =
            serde_json::from_slice(&bytes).map_err(|_| Error::UnsupportedSource)?;
        let mut result = Membership::default();
        for id in index
            .selected_composer_ids
            .into_iter()
            .chain(index.last_focused_composer_ids)
            .flatten()
        {
            if !safe_id(&id) {
                return Err(Error::UnsupportedSource);
            }
            result.focused.insert(id);
        }
        if index.has_migrated_composer_data != Some(true) {
            for entry in index.all_composers.into_iter().flatten() {
                if !safe_id(&entry.composer_id) || !result.local.insert(entry.composer_id) {
                    return Err(Error::UnsupportedSource);
                }
            }
        }
        if result.local.union(&result.focused).count() > limits.discovery_entries {
            return Err(Error::Limit);
        }
        Ok(result)
    }

    /// Read only IDs whose bounded global metadata names this workspace.
    fn recorded_ids(
        &self,
        db: &Connection,
        after: Option<&str>,
        limits: &Limits,
    ) -> Result<(Vec<String>, Option<String>, bool)> {
        let locator = rowid(db, "cursorDiskKV", "key", "value")?;
        let mut query = db
            .prepare(&format!(
                "SELECT key, {locator} FROM main.cursorDiskKV
                 WHERE typeof(key) = 'text' AND key COLLATE BINARY > ?1
                   AND key COLLATE BINARY < 'composerData;' AND octet_length(key) <= 141
                   AND instr(key, char(0)) = 0
                   AND substr(key, 14) NOT GLOB '*[^A-Za-z0-9_-]*'
                 ORDER BY key COLLATE BINARY LIMIT ?2",
            ))
            .map_err(sql_error)?;
        let start = format!("composerData:{}", after.unwrap_or(""));
        let mut rows = query
            .query(params![start, limits.discovery_sessions + 1])
            .map_err(sql_error)?;
        let mut page = Vec::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            let key: String = row.get(0).map_err(sql_error)?;
            let id = key
                .strip_prefix("composerData:")
                .filter(|id| safe_id(id))
                .ok_or(Error::UnsupportedSource)?;
            page.push((id.to_owned(), row.get::<_, i64>(1).map_err(sql_error)?));
        }
        let next = if page.len() > limits.discovery_sessions {
            page.pop();
            page.last().map(|(id, _)| id.clone())
        } else {
            None
        };
        let mut probe = db
            .prepare(&format!(
                "SELECT coalesce(CASE WHEN typeof(value) IN ('text','blob')
                            AND octet_length(value) <= ?2 AND json_valid(CAST(value AS TEXT))
                     THEN json_extract(CAST(value AS TEXT), '$.workspaceIdentifier.id')
                          = ?3 COLLATE BINARY END, 0)
                 FROM main.cursorDiskKV WHERE {locator} = ?1"
            ))
            .map_err(sql_error)?;
        let mut ids = Vec::new();
        let mut partial = false;
        let has_index = composer_index(db)?;
        for (id, row) in page {
            let matches: bool = probe
                .query_row(
                    params![row, PROOF_BYTES.min(limits.source_bytes), self.id],
                    |row| row.get(0),
                )
                .map_err(sql_error)?;
            if matches {
                ids.push(id);
            } else if has_index {
                match workspace_index_proof(db, &id)? {
                    WorkspaceProof::Unambiguous(workspace)
                        if workspace.as_deref() == Some(&self.id) =>
                    {
                        ids.push(id);
                    }
                    WorkspaceProof::Unambiguous(_) => {}
                    WorkspaceProof::Ambiguous => partial = true,
                }
            }
        }
        Ok((ids, next, partial))
    }

    /// Read the current workspace file by its retained directory handle.
    fn folder(&self, limits: &Limits) -> Result<ProjectEvidence> {
        let file = self.directory.source_file("workspace.json")?;
        let before = file.metadata()?;
        if !same_source(&before, &before) {
            return Err(Error::InvalidSource);
        }
        let maximum = limits.source_bytes.min(PROOF_BYTES);
        let mut bytes = Vec::new();
        let mut limited = file.take(maximum + 1);
        limited.read_to_end(&mut bytes)?;
        if bytes.len() as u64 > maximum {
            return Err(Error::Limit);
        }
        if !same_source(&before, &limited.into_inner().metadata()?) {
            return Err(Error::SourceChanged);
        }
        let folder: WorkspaceFolder =
            serde_json::from_slice(&bytes).map_err(|_| Error::UnsupportedSource)?;
        if folder.workspace.is_some() {
            return Err(Error::UnsupportedSource);
        }
        let path = folder
            .folder
            .strip_prefix("file://")
            .filter(|path| path.starts_with('/') && !path.starts_with("//"))
            .filter(|path| !path.contains(['%', '?', '#', '\\']))
            .ok_or(Error::UnsupportedSource)?;
        Ok(ProjectEvidence::recorded(path))
    }
}

pub(super) fn discover(
    workspace: &Path,
    after: Option<&str>,
    limits: &Limits,
) -> Result<(Vec<IdeSource>, Option<String>, bool)> {
    let deadline = Instant::now() + SOURCE_TIME;
    let workspace = Arc::new(Workspace::open(workspace)?);
    let db = workspace.database.connect(deadline)?;
    let members = workspace.membership(&db, limits)?;
    workspace.database.verify(Some(&db))?;
    let global = workspace.global.connect(deadline)?;
    let mut ids = members.local;
    ids.extend(members.focused);
    let (global_ids, global_next, partial) = workspace.recorded_ids(&global, after, limits)?;
    ids.extend(global_ids);
    workspace.global.verify(Some(&global))?;
    let mut ids: Vec<_> = ids
        .into_iter()
        .filter(|id| after.is_none_or(|after| id.as_str() > after))
        .filter(|id| global_next.as_ref().is_none_or(|fence| id <= fence))
        .take(limits.discovery_sessions + 1)
        .collect();
    let next = if ids.len() > limits.discovery_sessions {
        ids.pop();
        ids.last().cloned()
    } else {
        global_next
    };
    Ok((
        ids.into_iter()
            .map(|id| IdeSource {
                path: workspace.global_path.clone(),
                id,
                workspace: workspace.clone(),
            })
            .collect(),
        next,
        partial,
    ))
}

pub(super) fn reopen(path: &Path, id: &str) -> Result<IdeSource> {
    let workspace = Arc::new(Workspace::open(path)?);
    let source = IdeSource {
        path: workspace.global_path.clone(),
        id: id.into(),
        workspace,
    };
    let deadline = Instant::now() + SOURCE_TIME;
    let db = source.workspace.global.connect(deadline)?;
    let locator = rowid(&db, "cursorDiskKV", "key", "value")?;
    exact_row(&db, "cursorDiskKV", locator, &format!("composerData:{id}"))?;
    source.workspace.global.verify(Some(&db))?;
    Ok(source)
}

impl IdeSource {
    /// Resolve the exact global composer key, without reading its value.
    fn composer_row(&self, global: &Connection) -> Result<(&'static str, i64)> {
        let locator = rowid(global, "cursorDiskKV", "key", "value")?;
        let key = format!("composerData:{}", self.id);
        let row = exact_row(global, "cursorDiskKV", locator, &key)?;
        Ok((locator, row))
    }

    /// Read only bounded identity fields and layout flags before content approval.
    fn composer_identity(
        global: &Connection,
        locator: &str,
        row: i64,
        limits: &Limits,
    ) -> Result<ComposerIdentity> {
        let evidence: Option<String> = global
            .query_row(
                &format!(
                    "WITH candidate(raw) AS MATERIALIZED (
                        SELECT CASE WHEN typeof(value) IN ('text','blob')
                            AND octet_length(value) <= ?2 THEN CAST(value AS TEXT) END
                        FROM main.cursorDiskKV WHERE {locator} = ?1
                    ), valid(raw) AS MATERIALIZED (
                        SELECT CASE WHEN json_valid(raw) THEN raw END FROM candidate
                    ), proof(value) AS MATERIALIZED (
                        SELECT CASE WHEN json_type(raw) = 'object' AND NOT EXISTS(
                            SELECT 1 FROM json_each(valid.raw)
                            WHERE key IN ('composerId','workspaceId','workspaceIdentifier',
                                          '_v','headerLog','fullConversationHeadersOnly')
                            GROUP BY key HAVING count(*) > 1
                        ) THEN json_object(
                            'composerId', raw -> '$.composerId',
                            'workspaceId', raw -> '$.workspaceId',
                            'workspaceIdentifier', raw -> '$.workspaceIdentifier',
                            'hasInlineHeadersOnly',
                            json(CASE WHEN json_type(raw, '$.fullConversationHeadersOnly') = 'array'
                                       AND json_array_length(raw, '$.fullConversationHeadersOnly') > 0
                                       AND coalesce(json_type(raw, '$.headerLog'), 'null') = 'null'
                                      THEN 'true' ELSE 'false' END),
                            'hasChunkedHeaders',
                            json(CASE WHEN json_type(raw, '$._v') = 'integer'
                                       AND json_extract(raw, '$._v') = 18
                                       AND json_type(raw, '$.fullConversationHeadersOnly') = 'array'
                                       AND json_array_length(raw, '$.fullConversationHeadersOnly') = 0
                                       AND json_type(raw, '$.headerLog') = 'object'
                                      THEN 'true' ELSE 'false' END)
                        ) END FROM valid
                    ) SELECT CASE WHEN octet_length(value) <= 8192 THEN value END FROM proof"
                ),
                params![row, PROOF_BYTES.min(limits.source_bytes)],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        serde_json::from_str(evidence.as_deref().ok_or(Error::UnsupportedSource)?)
            .map_err(|_| Error::UnsupportedSource)
    }

    /// Return whether the composer proves workspace ownership without a focus guess.
    fn verify_composer(
        &self,
        global: &Connection,
        composer: &ComposerIdentity,
        members: &Membership,
        folder: &ProjectEvidence,
    ) -> Result<bool> {
        if composer
            .composer_id
            .as_ref()
            .is_some_and(|id| id != &self.id)
            || composer
                .workspace_id
                .as_ref()
                .is_some_and(|id| id != &self.workspace.id)
        {
            return Err(Error::SourceChanged);
        }
        if let Some(workspace) = &composer.workspace_identifier {
            let path = folder
                .project_path()
                .and_then(Path::to_str)
                .ok_or(Error::SourceChanged)?;
            if workspace.id != self.workspace.id
                || workspace.uri.scheme != "file"
                || workspace.uri.fs_path != path
                || workspace.uri.path != path
                || workspace.uri.external.strip_prefix("file://") != Some(path)
            {
                return Err(Error::SourceChanged);
            }
            if !composer.has_chunked_headers {
                return Ok(true);
            }
        }
        if composer.has_chunked_headers {
            return Ok(composer.composer_id.as_deref() == Some(&self.id)
                && folder.project_path().is_some()
                && composer_index(global)?
                && indexed_workspace(global, &self.id)?.as_deref() == Some(&self.workspace.id));
        }
        Ok(members.local.contains(&self.id)
            || (members.focused.contains(&self.id)
                && composer.workspace_id.as_ref() == Some(&self.workspace.id)))
    }

    pub(super) fn project_evidence(
        &self,
        limits: &Limits,
        deadline: Instant,
    ) -> Result<ProjectEvidence> {
        let workspace = self.workspace.database.connect(deadline)?;
        let members = self.workspace.membership(&workspace, limits)?;
        let evidence = self.workspace.folder(limits)?;
        let global = self.workspace.global.connect(deadline)?;
        let (locator, row) = self.composer_row(&global)?;
        let identity = Self::composer_identity(&global, locator, row, limits)?;
        let owns_folder = self.verify_composer(&global, &identity, &members, &evidence)?;
        if owns_folder && !identity.has_inline_headers_only && !identity.has_chunked_headers {
            return Err(Error::UnsupportedSource);
        }
        self.workspace.global.verify(Some(&global))?;
        self.workspace.database.verify(Some(&workspace))?;
        Ok(if owns_folder {
            evidence
        } else {
            ProjectEvidence::unknown()
        })
    }

    pub(super) fn export(
        &self,
        limits: &Limits,
        selection: &ProjectSelection,
        deadline: Instant,
        emit: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let workspace = self.workspace.database.connect(deadline)?;
        let members = self.workspace.membership(&workspace, limits)?;
        let evidence = self.workspace.folder(limits)?;
        if !selection.matches(&evidence)? {
            return Err(Error::SourceChanged);
        }
        let global = self.workspace.global.connect(deadline)?;
        let (locator, row) = self.composer_row(&global)?;
        let identity = Self::composer_identity(&global, locator, row, limits)?;
        if !self.verify_composer(&global, &identity, &members, &evidence)? {
            return Err(Error::SourceChanged);
        }
        if !identity.has_inline_headers_only && !identity.has_chunked_headers {
            return Err(Error::UnsupportedSource);
        }
        let bytes = proof_bytes(&global, "cursorDiskKV", locator, row, limits.source_bytes)?;
        let composer: Composer<'_> =
            serde_json::from_slice(&bytes).map_err(|_| Error::UnsupportedSource)?;
        if !empty_container(composer.conversation, true)
            || !empty_container(composer.conversation_map, false)
        {
            return Err(Error::UnsupportedSource);
        }
        if composer.full_conversation_headers_only.len() > MAX_BUBBLES {
            return Err(Error::Limit);
        }
        let mut rows = vec![row];
        if let Some(log) = composer.header_log {
            log.select_rows(&global, locator, &self.id, limits, deadline, &mut rows)?;
        } else {
            let mut seen = BTreeSet::new();
            for bubble in composer.full_conversation_headers_only {
                check_deadline(deadline)?;
                if !safe_id(&bubble.bubble_id) || !seen.insert(bubble.bubble_id.clone()) {
                    return Err(Error::UnsupportedSource);
                }
                let key = format!("bubbleId:{}:{}", self.id, bubble.bubble_id);
                rows.push(exact_row(&global, "cursorDiskKV", locator, &key)?);
            }
        }
        let mut writer = SqliteNativeWriter::new(&global, emit, limits.source_bytes, deadline)?;
        writer.begin_table("cursorDiskKV", &["key"], &[])?;
        for row in rows {
            writer.write_row(row)?;
        }
        writer.finish()?;
        check_deadline(deadline)?;
        self.workspace.global.verify(Some(&global))?;
        self.workspace.database.verify(Some(&workspace))?;
        drop(workspace);
        let current = self.workspace.database.connect(deadline)?;
        let current_members = self.workspace.membership(&current, limits)?;
        if self.workspace.folder(limits)? != evidence
            || !self.verify_composer(&global, &identity, &current_members, &evidence)?
        {
            return Err(Error::SourceChanged);
        }
        if identity.has_chunked_headers {
            let latest = self.workspace.global.connect(deadline)?;
            if !composer_index(&latest)?
                || indexed_workspace(&latest, &self.id)?.as_deref() != Some(&self.workspace.id)
            {
                return Err(Error::SourceChanged);
            }
            self.workspace.global.verify(Some(&latest))?;
        }
        self.workspace.database.verify(Some(&current))?;
        check_deadline(deadline)?;
        Ok(())
    }
}

fn empty_container(value: Option<&RawValue>, array: bool) -> bool {
    let (open, close) = if array { ('[', ']') } else { ('{', '}') };
    value.is_none_or(|value| {
        let value = value.get();
        value == "null"
            || value
                .strip_prefix(open)
                .and_then(|value| value.strip_suffix(close))
                .is_some_and(|value| value.trim().is_empty())
    })
}

#[cfg(test)]
#[path = "tests/ide_integration_tests.rs"]
mod tests;
