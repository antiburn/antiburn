//! Session-owned OpenCode V2 capture from an explicitly authorized database.
//!
//! Discovery reads schema and session identity only. Export runs after the spool
//! saves its preparing entry and obtains a reservation. It uses one read snapshot
//! for ownership, dependencies, and values, then closes SQLite before returning.
//! Call these blocking operations only from bounded blocking workers, never from
//! UI or async runtime threads.
//! See `docs/cloud/opencode.md` for supported source shapes and limits.

mod references;
mod selection;

use crate::{Error, Limits, ProjectEvidence, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) const MAX_ROWS: usize = 100_000;
pub(super) const MAX_REFERENCES: usize = 16_384;
pub(super) const MAX_SCHEMA_BYTES: usize = 1024 * 1024;
pub(super) const MAX_ID_BYTES: usize = 128;
const EXPORT_TIME: Duration = Duration::from_secs(30);

/// An admitted identity, not a saved source snapshot or continuing authorization.
///
/// Clones share admitted directory handles and a recorded database identity.
/// They never hold a raw database or SHM descriptor: closing one could release
/// another SQLite connection's process-wide POSIX locks. The filename keeps its
/// normal SQLite context so committed WAL values remain visible. Native keys
/// include the provider and database filename; the caller adds its stable account
/// and authorized-root scope. No installed version is historical source evidence.
#[derive(Clone, Debug)]
pub(crate) struct OpenCodeSource {
    database: Arc<AdmittedDatabase>,
    session_id: String,
    native_key: String,
}

impl OpenCodeSource {
    /// Display locator only. Export verifies it against the admitted handles.
    pub(crate) fn source_path(&self) -> &Path {
        &self.database.path
    }

    /// Exact native ID, independent of titles, models, and root grouping.
    pub(crate) fn native_session_id(&self) -> &str {
        &self.session_id
    }

    /// Stable unscoped key; the host must add its account/root scope.
    pub(crate) fn native_key(&self) -> &str {
        &self.native_key
    }

    /// Return another native key in this store, without granting access to it.
    pub(crate) fn key_for_session(&self, id: &str) -> Result<String> {
        native_key(&self.database.filename, id)
    }

    /// Read only this session's bounded recorded CWD under the source binding.
    pub(crate) fn project_evidence(&self, limits: &Limits) -> Result<ProjectEvidence> {
        let deadline = Instant::now() + EXPORT_TIME;
        let connection = self.database.connect(deadline)?;
        let schema = selection::Schema::read(&connection, limits, deadline)?;
        let evidence = schema.project_evidence(&connection, &self.session_id)?;
        self.database.verify(Some(&connection))?;
        connection.execute_batch("ROLLBACK").map_err(sql_error)?;
        Ok(evidence)
    }
}

/// Direct native edges from the same snapshot as exported values.
///
/// Neither edge establishes another session's owner, availability, or permission.
/// A missing lineage column leaves that relationship unknown. The native version
/// cell is retained without claiming a historical runtime version.
pub(crate) struct OpenCodeExport {
    pub parent_session_id: Option<String>,
    pub fork_session_id: Option<String>,
}

/// Discover every bounded native identity in one authorized OpenCode database.
///
/// This function does not search homes, apply recency filters, group children, or
/// read message, event, instruction, job, or media payloads. Unsupported and
/// partially migrated V2 stores fail visibly and never select a V1 fallback.
/// Run this synchronous filesystem and SQLite work on a bounded blocking worker.
pub(crate) fn discover(database: &Path, limits: &Limits) -> Result<Vec<OpenCodeSource>> {
    let (sources, next_cursor) = discover_page(database, None, limits)?;
    if next_cursor.is_some() {
        return Err(Error::Limit);
    }
    Ok(sources)
}

/// Read one binary-ID page from a fresh, source-bound metadata snapshot.
pub(crate) fn discover_page(
    database: &Path,
    after: Option<&str>,
    limits: &Limits,
) -> Result<(Vec<OpenCodeSource>, Option<String>)> {
    limits.validate()?;
    if let Some(after) = after {
        validate_id(after)?;
    }
    let admitted = Arc::new(AdmittedDatabase::open(database)?);
    let deadline = Instant::now() + EXPORT_TIME;
    let connection = admitted.connect(deadline)?;
    let schema = selection::Schema::read(&connection, limits, deadline)?;
    schema.validate_identities(&connection)?;
    let mut statement = connection
        .prepare(
            "SELECT CASE WHEN typeof(id) = 'text' AND octet_length(id) <= ?1
                         THEN id END
               FROM session_v2 WHERE id COLLATE BINARY > ?2
               ORDER BY id COLLATE BINARY LIMIT ?3",
        )
        .map_err(sql_error)?;
    let mut rows = statement
        .query(params![
            MAX_ID_BYTES,
            after.unwrap_or(""),
            limits.discovery_sessions + 1
        ])
        .map_err(sql_error)?;
    let mut sources = Vec::new();
    let mut next_cursor = None;
    while let Some(row) = rows.next().map_err(sql_error)? {
        check_deadline(deadline)?;
        if sources.len() == limits.discovery_sessions {
            next_cursor = sources
                .last()
                .map(|source: &OpenCodeSource| source.session_id.clone());
            break;
        }
        let id: Option<String> = row.get(0).map_err(sql_error)?;
        let id = id.ok_or(Error::InvalidSource)?;
        validate_id(&id)?;
        sources.push(OpenCodeSource {
            native_key: native_key(&admitted.filename, &id)?,
            database: admitted.clone(),
            session_id: id,
        });
    }
    drop(rows);
    drop(statement);
    admitted.verify(Some(&connection))?;
    connection.execute_batch("ROLLBACK").map_err(sql_error)?;
    Ok((sources, next_cursor))
}

/// Reopen one exact native ID without reading other session rows or payload.
pub(crate) fn reopen(database: &Path, session_id: &str, limits: &Limits) -> Result<OpenCodeSource> {
    validate_id(session_id)?;
    let database = Arc::new(AdmittedDatabase::open(database)?);
    let deadline = Instant::now() + EXPORT_TIME;
    let connection = database.connect(deadline)?;
    selection::Schema::read(&connection, limits, deadline)?;
    let present: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM session_v2 WHERE id = ?1 COLLATE BINARY
                AND typeof(id) = 'text' AND octet_length(id) BETWEEN 1 AND ?2)",
            params![session_id, MAX_ID_BYTES],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if !present {
        return Err(Error::SourceChanged);
    }
    database.verify(Some(&connection))?;
    connection.execute_batch("ROLLBACK").map_err(sql_error)?;
    Ok(OpenCodeSource {
        native_key: native_key(&database.filename, session_id)?,
        database,
        session_id: session_id.into(),
    })
}

/// Export all proved session-owned source under one read-only SQLite snapshot.
///
/// The caller must reserve capacity first. `emit` receives bounded native-format
/// fragments and may stop work with a closed error. Any error leaves publication
/// to the spool's interrupted-capture rules; a partial stream is not a capture.
/// No database, WAL, external file, URL, or parent transcript is copied.
/// This function blocks; the host owns its bounded blocking worker.
pub(crate) fn export<F>(source: &OpenCodeSource, limits: &Limits, emit: F) -> Result<OpenCodeExport>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    export_before(source, limits, emit, Instant::now() + EXPORT_TIME)
}

/// Check admitted project evidence in the same snapshot used for payload export.
pub(crate) fn export_in_project<F>(
    source: &OpenCodeSource,
    limits: &Limits,
    selection: &crate::ProjectSelection,
    emit: F,
) -> Result<OpenCodeExport>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    export_snapshot(
        source,
        limits,
        emit,
        Instant::now() + EXPORT_TIME,
        Some(selection),
    )
}

fn export_before<F>(
    source: &OpenCodeSource,
    limits: &Limits,
    emit: F,
    deadline: Instant,
) -> Result<OpenCodeExport>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    export_snapshot(source, limits, emit, deadline, None)
}

fn export_snapshot<F>(
    source: &OpenCodeSource,
    limits: &Limits,
    emit: F,
    deadline: Instant,
    project_selection: Option<&crate::ProjectSelection>,
) -> Result<OpenCodeExport>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    limits.validate()?;
    check_deadline(deadline)?;
    let connection = source.database.connect(deadline)?;
    let schema = selection::Schema::read(&connection, limits, deadline)?;
    if let Some(selection) = project_selection
        && !selection.matches(&schema.project_evidence(&connection, &source.session_id)?)?
    {
        return Err(Error::SourceChanged);
    }
    let relationships = schema.relationships(&connection, &source.session_id)?;
    let tables = selection::select(&connection, &schema, &source.session_id, limits, deadline)?;
    source.database.verify(Some(&connection))?;
    let mut writer = crate::sqlite_native::SqliteNativeWriter::new(
        &connection,
        emit,
        limits.source_bytes,
        deadline,
    )?;
    for table in tables {
        check_deadline(deadline)?;
        writer.begin_table(table.name, table.keys, table.order)?;
        for rowid in table.rows {
            check_deadline(deadline)?;
            writer.write_row(rowid)?;
        }
    }
    writer.finish()?;
    source.database.verify(Some(&connection))?;
    check_deadline(deadline)?;
    connection.execute_batch("ROLLBACK").map_err(sql_error)?;
    Ok(relationships)
}

#[derive(Debug)]
/// A read-only SQLite source bound to one file identity and its parent entries.
///
/// Opening and closing a raw database or SHM handle could release process-wide
/// SQLite locks. Keep only directory handles until the SQLite connection opens.
pub(crate) struct AdmittedDatabase {
    path: PathBuf,
    filename: String,
    identity: FileIdentity,
    ancestors: Vec<(PathBuf, File)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn of_handle(file: &File) -> Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

struct SourceMetadata {
    identity: FileIdentity,
    mode: libc::mode_t,
    links: libc::nlink_t,
}

impl SourceMetadata {
    fn require_regular(&self) -> Result<()> {
        if self.mode & libc::S_IFMT != libc::S_IFREG || self.links != 1 {
            return Err(Error::InvalidSource);
        }
        Ok(())
    }
}

impl AdmittedDatabase {
    #[cfg(unix)]
    /// Pin the database and directory entry identities without opening a raw database handle.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let parent = path.parent().ok_or(Error::InvalidSource)?;
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(Error::InvalidSource)?;
        if filename.len() > MAX_ID_BYTES || !path.is_absolute() {
            return Err(Error::InvalidSource);
        }
        let mut ancestors = Vec::new();
        for ancestor in parent.ancestors() {
            if ancestors.len() == 64 {
                return Err(Error::Limit);
            }
            ancestors.push((
                ancestor.to_owned(),
                crate::filesystem::open_source(ancestor, true)?,
            ));
        }
        let metadata = metadata_at(&ancestors[0].1, OsStr::new(filename))?;
        metadata.require_regular()?;
        let database = Self {
            path: path.to_owned(),
            filename: filename.to_owned(),
            identity: metadata.identity,
            ancestors,
        };
        database.verify(None)?;
        Ok(database)
    }

    /// Open a read-only transaction with the current SQLite sidecars and a deadline.
    pub(crate) fn connect(&self, deadline: Instant) -> Result<Connection> {
        check_deadline(deadline)?;
        self.verify(None)?;
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection = Connection::open_with_flags(&self.path, flags).map_err(sql_error)?;
        self.verify(Some(&connection))?;
        connection
            .busy_timeout(Duration::from_millis(250))
            .map_err(sql_error)?;
        connection.progress_handler(1000, Some(move || Instant::now() >= deadline));
        connection
            .execute_batch(
                "PRAGMA query_only=ON;
                 PRAGMA trusted_schema=OFF;
                 PRAGMA read_uncommitted=OFF;
                 PRAGMA temp_store=MEMORY;
                 PRAGMA cache_size=-2048;
                 BEGIN;",
            )
            .map_err(sql_error)?;
        Ok(connection)
    }

    /// Return a pinned directory for a source association check.
    pub(crate) fn ancestor_handle(&self, path: &Path) -> Result<&File> {
        self.ancestors
            .iter()
            .find(|(ancestor, _)| ancestor == path)
            .map(|(_, handle)| handle)
            .ok_or(Error::InvalidSource)
    }

    /// Require both databases to belong to the same admitted directory.
    pub(crate) fn verify_shared_ancestor(&self, other: &Self, path: &Path) -> Result<()> {
        if FileIdentity::of_handle(self.ancestor_handle(path)?)?
            != FileIdentity::of_handle(other.ancestor_handle(path)?)?
        {
            return Err(Error::SourceChanged);
        }
        Ok(())
    }

    #[cfg(unix)]
    /// Check current directory entries and, when available, SQLite's open file.
    pub(crate) fn verify(&self, connection: Option<&Connection>) -> Result<()> {
        let (root_path, root) = self.ancestors.last().ok_or(Error::InvalidSource)?;
        let current = std::fs::symlink_metadata(root_path).map_err(|_| Error::SourceChanged)?;
        if !current.is_dir()
            || FileIdentity::of_handle(root)?
                != (FileIdentity {
                    device: current.dev(),
                    inode: current.ino(),
                })
        {
            return Err(Error::SourceChanged);
        }
        for pair in self.ancestors.windows(2).rev() {
            let (path, pinned) = &pair[0];
            let current = metadata_at(&pair[1].1, path.file_name().ok_or(Error::InvalidSource)?)
                .map_err(|_| Error::SourceChanged)?;
            if current.mode & libc::S_IFMT != libc::S_IFDIR
                || current.identity != FileIdentity::of_handle(pinned)?
            {
                return Err(Error::SourceChanged);
            }
        }
        let directory = &self.ancestors[0].1;
        let current =
            metadata_at(directory, OsStr::new(&self.filename)).map_err(|_| Error::SourceChanged)?;
        current.require_regular()?;
        if current.identity != self.identity {
            return Err(Error::SourceChanged);
        }
        for suffix in ["-wal", "-shm", "-journal"] {
            match metadata_at(directory, OsStr::new(&format!("{}{suffix}", self.filename))) {
                Ok(metadata) => metadata.require_regular()?,
                Err(Error::Io(std::io::ErrorKind::NotFound)) => {}
                Err(_) => return Err(Error::InvalidSource),
            }
        }
        if let Some(connection) = connection {
            let mut moved = 0_i32;
            // SQLite checks its own open file, including a replacement restored
            // before this path check. The connection has one synchronous owner.
            let result = unsafe {
                rusqlite::ffi::sqlite3_file_control(
                    connection.handle(),
                    c"main".as_ptr(),
                    rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED,
                    (&mut moved as *mut i32).cast(),
                )
            };
            if result != rusqlite::ffi::SQLITE_OK || moved != 0 {
                return Err(Error::SourceChanged);
            }
        }
        Ok(())
    }
}

/// Inspect one directory entry without opening or closing the source file.
///
/// Closing any raw DB or SHM descriptor could cancel SQLite's POSIX locks for
/// this process, including locks owned by another host connection.
fn metadata_at(directory: &File, name: &OsStr) -> Result<SourceMetadata> {
    let name = CString::new(name.as_bytes()).map_err(|_| Error::InvalidSource)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // fstatat reads metadata relative to a retained directory and does not follow
    // the final entry. It creates no source-file descriptor.
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    // A successful fstatat initializes every stat field used below.
    let stat = unsafe { stat.assume_init() };
    Ok(SourceMetadata {
        identity: FileIdentity {
            #[cfg(target_os = "linux")]
            device: stat.st_dev,
            #[cfg(not(target_os = "linux"))]
            device: stat.st_dev as u64,
            inode: stat.st_ino,
        },
        mode: stat.st_mode,
        links: stat.st_nlink,
    })
}

fn native_key(filename: &str, id: &str) -> Result<String> {
    validate_id(id)?;
    Ok(format!(
        "opencode/{}/{}",
        URL_SAFE_NO_PAD.encode(filename),
        URL_SAFE_NO_PAD.encode(id)
    ))
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > MAX_ID_BYTES || id.chars().any(char::is_control) {
        return Err(Error::InvalidSource);
    }
    Ok(())
}

pub(super) fn check_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        return Err(Error::Limit);
    }
    Ok(())
}

/// Never expose SQL text, source paths, native values, or SQLite's error message.
pub(super) fn sql_error(error: rusqlite::Error) -> Error {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            Error::Busy
        }
        Some(rusqlite::ErrorCode::OperationInterrupted | rusqlite::ErrorCode::TooBig) => {
            Error::Limit
        }
        _ => Error::InvalidSource,
    }
}

/// Read a small native discriminator or identity without loading oversized text.
///
/// Table and column identifiers are provider-owned constants, not source input.
pub(super) fn bounded_text(
    connection: &Connection,
    table: &str,
    column: &str,
    rowid: i64,
) -> Result<Option<String>> {
    let query = format!(
        "SELECT typeof(\"{column}\"), CASE WHEN typeof(\"{column}\") = 'text'
          AND octet_length(\"{column}\") <= ?2 THEN \"{column}\" END
         FROM \"{table}\" WHERE rowid = ?1"
    );
    let value = connection
        .query_row(&query, params![rowid, MAX_ID_BYTES], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .optional()
        .map_err(sql_error)?
        .ok_or(Error::SourceChanged)?;
    match value {
        (kind, None) if kind == "null" => Ok(None),
        (kind, Some(value)) if kind == "text" => Ok(Some(value)),
        _ => Err(Error::InvalidSource),
    }
}

#[cfg(all(test, unix))]
#[path = "opencode/tests.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "opencode/discovery_integration_tests.rs"]
mod discovery_tests;
