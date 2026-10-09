//! Archive, restore and index-line removal for one memory directory.

use std::path::Path;
#[cfg(not(windows))]
use std::{
    fs,
    io::Write,
    path::{Component, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(not(windows))]
use antiburn_local::memories::inventory::parse_index_line;
#[cfg(not(windows))]
use serde::{Deserialize, Serialize};

use crate::agent_config::ConfigUnavailableReason;
#[cfg(unix)]
use crate::agent_config::filesystem::file_ownership;
#[cfg(not(windows))]
use crate::agent_config::filesystem::{
    CheckedFile, canonical_root, create_temporary, map_write_error, path_entry_exists, read_checked,
};
use crate::dto::MemoryEditOutcome;

#[cfg(not(windows))]
const INDEX_FILE: &str = "MEMORY.md";
#[cfg(not(windows))]
const BACKUP_FILE: &str = "MEMORY.md.antiburn-bak";
#[cfg(not(windows))]
const BACKUP_MARKER: &str = ".antiburn-bak";

#[cfg(windows)]
fn unsupported() -> Result<MemoryEditOutcome, String> {
    Ok(MemoryEditOutcome::Unavailable {
        reason: ConfigUnavailableReason::AutomaticApplyUnsupported.to_string(),
    })
}

#[cfg(windows)]
pub(crate) fn archive_memory(
    _home: &Path,
    _archive_root: &Path,
    _slug: &str,
    _file_name: &str,
    _expected_size_bytes: u64,
    _expected_modified_ms: Option<i64>,
) -> Result<MemoryEditOutcome, String> {
    unsupported()
}

#[cfg(windows)]
pub(crate) fn restore_memory(
    _home: &Path,
    _archive_root: &Path,
    _slug: &str,
    _archive_id: &str,
) -> Result<MemoryEditOutcome, String> {
    unsupported()
}

#[cfg(windows)]
pub(crate) fn remove_index_line(
    _home: &Path,
    _slug: &str,
    _line_number: usize,
    _expected_target: &str,
) -> Result<MemoryEditOutcome, String> {
    unsupported()
}

/// The data that lets Undo put a memory back.
#[cfg(not(windows))]
#[derive(Debug, Serialize, Deserialize)]
struct ArchiveSidecar {
    slug: String,
    file_name: String,
    original_path: String,
    index_line: Option<String>,
    index_line_number: Option<usize>,
    archived_at_ms: i64,
    restored_at_ms: Option<i64>,
}

/// Why an operation stopped. `Outcome` is a normal result. `Failed` is an
/// error after a change started.
#[cfg(not(windows))]
#[derive(Debug)]
enum Stop {
    Outcome(MemoryEditOutcome),
    Failed(String),
}

#[cfg(not(windows))]
impl From<ConfigUnavailableReason> for Stop {
    fn from(reason: ConfigUnavailableReason) -> Self {
        Self::Outcome(MemoryEditOutcome::Unavailable {
            reason: reason.to_string(),
        })
    }
}

#[cfg(not(windows))]
fn finish(result: Result<MemoryEditOutcome, Stop>) -> Result<MemoryEditOutcome, String> {
    match result {
        Ok(outcome) | Err(Stop::Outcome(outcome)) => Ok(outcome),
        Err(Stop::Failed(message)) => Err(message),
    }
}

#[cfg(not(windows))]
fn io_stop(error: std::io::Error) -> Stop {
    map_write_error(error).into()
}

#[cfg(not(windows))]
fn failed(context: &str, error: impl std::fmt::Display) -> Stop {
    Stop::Failed(format!("{context}: {error}"))
}

#[cfg(not(windows))]
fn now_ms() -> Result<i64, Stop> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ConfigUnavailableReason::WriteFailed)?;
    i64::try_from(elapsed.as_millis()).map_err(|_| ConfigUnavailableReason::WriteFailed.into())
}

/// A value that is exactly one normal path component.
#[cfg(not(windows))]
fn single_component(value: &str) -> Result<(), Stop> {
    let mut components = Path::new(value).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None) if name == value => Ok(()),
        _ => Err(ConfigUnavailableReason::UnsafePath.into()),
    }
}

#[cfg(not(windows))]
fn validate_file_name(file_name: &str) -> Result<(), Stop> {
    single_component(file_name)?;
    if !file_name.ends_with(".md") || file_name == INDEX_FILE || file_name.contains(BACKUP_MARKER) {
        return Err(ConfigUnavailableReason::UnsafePath.into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn memory_root(home: &Path, slug: &str) -> Result<PathBuf, Stop> {
    single_component(slug)?;
    Ok(canonical_root(
        &home
            .join(".claude")
            .join("projects")
            .join(slug)
            .join("memory"),
    )?)
}

/// Reads a file under the trusted root. A missing file gives `None`.
#[cfg(not(windows))]
fn read_optional(path: &Path, root: &Path) -> Result<Option<CheckedFile>, Stop> {
    match read_checked(path, root) {
        Ok(file) => Ok(Some(file)),
        Err(ConfigUnavailableReason::MissingConfig) => Ok(None),
        Err(reason) => Err(reason.into()),
    }
}

#[cfg(not(windows))]
fn modified_ms(metadata: &fs::Metadata) -> Option<i64> {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
}

#[cfg(not(windows))]
fn sync_dir(path: &Path) -> std::io::Result<()> {
    fs::File::open(path).and_then(|directory| directory.sync_all())
}

#[cfg(not(windows))]
fn temporary_path(path: &Path) -> Result<PathBuf, Stop> {
    let parent = path.parent().ok_or(ConfigUnavailableReason::UnsafePath)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(ConfigUnavailableReason::UnsafePath)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ConfigUnavailableReason::WriteFailed)?
        .as_nanos();
    Ok(parent.join(format!(".{name}.antiburn-{nonce}.tmp")))
}

/// Writes `bytes` to a temporary file, then renames it onto `path`. With
/// `create_only`, the rename does not happen when `path` exists.
#[cfg(not(windows))]
fn write_atomic(
    path: &Path,
    permissions: &fs::Permissions,
    bytes: &[u8],
    create_only: bool,
) -> Result<(), Stop> {
    let temporary = temporary_path(path)?;
    let staged = (|| {
        let mut output = create_temporary(&temporary, permissions).map_err(io_stop)?;
        output
            .write_all(bytes)
            .and_then(|()| output.sync_all())
            .map_err(io_stop)?;
        drop(output);
        if create_only && path_entry_exists(path)? {
            return Err(Stop::Outcome(MemoryEditOutcome::AlreadyExists));
        }
        fs::rename(&temporary, path).map_err(io_stop)
    })();
    if staged.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    staged?;
    let parent = path.parent().ok_or(ConfigUnavailableReason::UnsafePath)?;
    sync_dir(parent).map_err(|error| failed("Could not sync the memory folder", error))
}

/// Replaces an existing file in the way `agent_config` does. The file must
/// still have the identity and bytes of `original` at rename time.
#[cfg(not(windows))]
fn replace_checked(
    path: &Path,
    root: &Path,
    original: &CheckedFile,
    bytes: &[u8],
) -> Result<(), Stop> {
    let temporary = temporary_path(path)?;
    let staged = (|| {
        let mut output = create_temporary(&temporary, &original.permissions).map_err(io_stop)?;
        #[cfg(unix)]
        if file_ownership(
            &output
                .metadata()
                .map_err(|_| ConfigUnavailableReason::WriteFailed)?,
        ) != original.ownership
        {
            return Err(ConfigUnavailableReason::UnsupportedOwner.into());
        }
        output
            .write_all(bytes)
            .and_then(|()| output.sync_all())
            .map_err(io_stop)?;
        drop(output);
        let replacement = crate::agent_config::filesystem::file_identity(
            &fs::symlink_metadata(&temporary).map_err(|_| ConfigUnavailableReason::WriteFailed)?,
        );
        let current = read_checked(path, root)?;
        if current.identity != original.identity || current.bytes != original.bytes {
            return Err(Stop::Outcome(MemoryEditOutcome::ChangedOnDisk));
        }
        fs::rename(&temporary, path).map_err(io_stop)?;
        Ok(replacement)
    })();
    if staged.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    let replacement = staged?;
    let parent = path.parent().ok_or(ConfigUnavailableReason::UnsafePath)?;
    sync_dir(parent).map_err(|error| failed("Could not sync the memory folder", error))?;
    let readback = read_checked(path, root)
        .map_err(|reason| failed("Could not read the file back", reason))?;
    if readback.identity != replacement || readback.bytes != bytes {
        return Err(Stop::Failed(
            "The file changed after it was written".to_owned(),
        ));
    }
    Ok(())
}

/// Refuses a backup path that is a symlink or otherwise unsafe.
#[cfg(not(windows))]
fn check_backup_target(root: &Path) -> Result<(), Stop> {
    let backup = root.join(BACKUP_FILE);
    if path_entry_exists(&backup)? {
        read_checked(&backup, root)?;
    }
    Ok(())
}

/// Writes the previous index bytes to the backup file. A new backup replaces
/// an older one.
#[cfg(not(windows))]
fn write_backup(root: &Path, index: &CheckedFile) -> Result<(), Stop> {
    check_backup_target(root)?;
    write_atomic(
        &root.join(BACKUP_FILE),
        &index.permissions,
        &index.bytes,
        false,
    )
}

/// Backs up the index, then replaces it. The backup may exist when the
/// replace fails with `ChangedOnDisk`. That is safe: it holds bytes that were
/// valid when the index was read.
#[cfg(not(windows))]
fn commit_index(root: &Path, index: &IndexFile, bytes: &[u8]) -> Result<(), Stop> {
    write_backup(root, &index.checked)?;
    replace_checked(&root.join(INDEX_FILE), root, &index.checked, bytes)
}

#[cfg(not(windows))]
struct IndexFile {
    checked: CheckedFile,
    text: String,
}

#[cfg(not(windows))]
fn read_index(root: &Path) -> Result<Option<IndexFile>, Stop> {
    let Some(checked) = read_optional(&root.join(INDEX_FILE), root)? else {
        return Ok(None);
    };
    let text = String::from_utf8(checked.bytes.clone())
        .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    Ok(Some(IndexFile { checked, text }))
}

/// The link target without a leading `./`.
#[cfg(not(windows))]
fn bare_target(target: &str) -> &str {
    target.strip_prefix("./").unwrap_or(target)
}

/// The target of an index line, or `None` when the line is not an entry.
#[cfg(not(windows))]
fn line_target(segment: &str) -> Option<String> {
    let line = segment.trim_end_matches(['\r', '\n']);
    parse_index_line(line, 1).map(|entry| bare_target(&entry.target).to_owned())
}

/// Lines with their terminators. The count matches the line numbers that the
/// inventory reports.
#[cfg(not(windows))]
fn segments(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// Joins `kept` lines. When the original had no final newline and the last
/// original line is gone, the new last line gets no newline either.
#[cfg(not(windows))]
fn join_removed(kept: &[&str], original: &str, last_removed: bool) -> String {
    let mut text = kept.concat();
    if last_removed && !original.ends_with('\n') {
        let trimmed = text.trim_end_matches(['\r', '\n']).len();
        text.truncate(trimmed);
    }
    text
}

/// What removing every entry for `file_name` does to the index.
#[cfg(not(windows))]
struct Removal {
    text: String,
    first_line: String,
    first_number: usize,
}

#[cfg(not(windows))]
fn remove_target_lines(text: &str, file_name: &str) -> Option<Removal> {
    let all = segments(text);
    let mut first: Option<(String, usize)> = None;
    let mut kept = Vec::new();
    for (index, segment) in all.iter().enumerate() {
        if line_target(segment).as_deref() == Some(file_name) {
            first.get_or_insert_with(|| {
                (segment.trim_end_matches(['\r', '\n']).to_owned(), index + 1)
            });
        } else {
            kept.push(*segment);
        }
    }
    let (first_line, first_number) = first?;
    let last_removed =
        line_target(all.last().copied().unwrap_or_default()).as_deref() == Some(file_name);
    Some(Removal {
        text: join_removed(&kept, text, last_removed),
        first_line,
        first_number,
    })
}

#[cfg(not(windows))]
fn insert_line(text: &str, line: &str, line_number: usize) -> String {
    let mut all: Vec<String> = segments(text).into_iter().map(str::to_owned).collect();
    let ending = if all.iter().any(|segment| segment.ends_with("\r\n")) {
        "\r\n"
    } else {
        "\n"
    };
    let position = line_number.saturating_sub(1).min(all.len());
    if position == all.len()
        && let Some(last) = all.last_mut()
        && !last.ends_with('\n')
    {
        last.push_str(ending);
    }
    all.insert(position, format!("{line}{ending}"));
    all.concat()
}

#[cfg(not(windows))]
pub(crate) fn archive_memory(
    home: &Path,
    archive_root: &Path,
    slug: &str,
    file_name: &str,
    expected_size_bytes: u64,
    expected_modified_ms: Option<i64>,
) -> Result<MemoryEditOutcome, String> {
    finish(archive(
        home,
        archive_root,
        slug,
        file_name,
        expected_size_bytes,
        expected_modified_ms,
    ))
}

#[cfg(not(windows))]
fn archive(
    home: &Path,
    archive_root: &Path,
    slug: &str,
    file_name: &str,
    expected_size_bytes: u64,
    expected_modified_ms: Option<i64>,
) -> Result<MemoryEditOutcome, Stop> {
    validate_file_name(file_name)?;
    let root = memory_root(home, slug)?;
    let path = root.join(file_name);
    let Some(file) = read_optional(&path, &root)? else {
        return Ok(MemoryEditOutcome::Missing);
    };
    let metadata = fs::metadata(&path).map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if metadata.len() != expected_size_bytes
        || file.bytes.len() as u64 != expected_size_bytes
        || modified_ms(&metadata) != expected_modified_ms
    {
        return Ok(MemoryEditOutcome::ChangedOnDisk);
    }

    // Check every index precondition before the first write.
    let index = read_index(&root)?;
    let removal = index
        .as_ref()
        .and_then(|index| remove_target_lines(&index.text, file_name));
    if removal.is_some() {
        check_backup_target(&root)?;
    }

    let archived_at_ms = now_ms()?;
    let archive_id = format!("{archived_at_ms}-{file_name}");
    let archive_dir = archive_root.join(slug);
    crate::remote_sessions::private_dir(&archive_dir).map_err(Stop::Failed)?;
    let data_path = archive_dir.join(&archive_id);
    let sidecar_path = archive_dir.join(format!("{archive_id}.json"));
    let sidecar = ArchiveSidecar {
        slug: slug.to_owned(),
        file_name: file_name.to_owned(),
        original_path: path.display().to_string(),
        index_line: removal.as_ref().map(|removal| removal.first_line.clone()),
        index_line_number: removal.as_ref().map(|removal| removal.first_number),
        archived_at_ms,
        restored_at_ms: None,
    };
    write_archive(&archive_dir, &data_path, &sidecar_path, &file, &sidecar)?;

    let moved = (|| -> Result<(), Stop> {
        // The file must still be the archived copy before the index changes.
        let current = read_checked(&path, &root)?;
        if current.identity != file.identity || current.bytes != file.bytes {
            return Err(Stop::Outcome(MemoryEditOutcome::ChangedOnDisk));
        }
        if let (Some(index), Some(removal)) = (&index, &removal) {
            commit_index(&root, index, removal.text.as_bytes())?;
        }
        Ok(())
    })();
    if let Err(stop) = moved {
        // A stop here is a normal outcome and the memory file still exists.
        // A copy in the archive has no use, so remove it.
        if matches!(stop, Stop::Outcome(_)) {
            let _ = fs::remove_file(&data_path);
            let _ = fs::remove_file(&sidecar_path);
        }
        return Err(stop);
    }

    if let Err(error) = fs::remove_file(&path) {
        // The index line is gone but the file is still there. Put the line
        // back so the file does not become an orphan, but only while the
        // index still holds the text this call committed. Then drop the
        // archive copy that nothing refers to.
        if let (Some(index), Some(removal)) = (&index, &removal) {
            let index_path = root.join(INDEX_FILE);
            if let Ok(current) = read_checked(&index_path, &root)
                && current.bytes == removal.text.as_bytes()
            {
                let _ = replace_checked(&index_path, &root, &current, index.text.as_bytes());
            }
        }
        let _ = fs::remove_file(&data_path);
        let _ = fs::remove_file(&sidecar_path);
        return Err(failed("Could not remove the memory file", error));
    }
    sync_dir(&root).map_err(|error| failed("Could not sync the memory folder", error))?;
    Ok(MemoryEditOutcome::Archived {
        archive_id,
        index_line_removed: removal.is_some(),
    })
}

/// Writes the archived bytes and the sidecar. The data file comes first, so a
/// sidecar never points at missing bytes.
#[cfg(not(windows))]
fn write_archive(
    archive_dir: &Path,
    data_path: &Path,
    sidecar_path: &Path,
    file: &CheckedFile,
    sidecar: &ArchiveSidecar,
) -> Result<(), Stop> {
    let written = (|| {
        let mut data = create_temporary(data_path, &file.permissions).map_err(io_stop)?;
        data.write_all(&file.bytes)
            .and_then(|()| data.sync_all())
            .map_err(io_stop)?;
        let text = serde_json::to_vec_pretty(sidecar)
            .map_err(|error| failed("Could not encode the archive record", error))?;
        let mut record = create_temporary(sidecar_path, &file.permissions).map_err(io_stop)?;
        record
            .write_all(&text)
            .and_then(|()| record.sync_all())
            .map_err(io_stop)?;
        sync_dir(archive_dir).map_err(io_stop)
    })();
    if written.is_err() {
        let _ = fs::remove_file(data_path);
        let _ = fs::remove_file(sidecar_path);
    }
    written
}

#[cfg(not(windows))]
pub(crate) fn restore_memory(
    home: &Path,
    archive_root: &Path,
    slug: &str,
    archive_id: &str,
) -> Result<MemoryEditOutcome, String> {
    finish(restore(home, archive_root, slug, archive_id))
}

#[cfg(not(windows))]
fn restore(
    home: &Path,
    archive_root: &Path,
    slug: &str,
    archive_id: &str,
) -> Result<MemoryEditOutcome, Stop> {
    single_component(slug)?;
    single_component(archive_id)?;
    let archive_dir = archive_root.join(slug);
    match fs::symlink_metadata(&archive_dir) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MemoryEditOutcome::Missing);
        }
        Err(_) => return Err(ConfigUnavailableReason::UnsafePath.into()),
    }
    let archive_dir_root = canonical_root(&archive_dir)?;
    let sidecar_path = archive_dir_root.join(format!("{archive_id}.json"));
    let Some(record) = read_optional(&sidecar_path, &archive_dir_root)? else {
        return Ok(MemoryEditOutcome::Missing);
    };
    let mut sidecar: ArchiveSidecar = serde_json::from_slice(&record.bytes)
        .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    if sidecar.slug != slug {
        return Err(ConfigUnavailableReason::UnsafePath.into());
    }
    validate_file_name(&sidecar.file_name)?;
    let Some(archived) = read_optional(&archive_dir_root.join(archive_id), &archive_dir_root)?
    else {
        return Ok(MemoryEditOutcome::Missing);
    };

    let root = memory_root(home, slug)?;
    let target = root.join(&sidecar.file_name);
    if path_entry_exists(&target)? {
        return Ok(MemoryEditOutcome::AlreadyExists);
    }

    // Check the index before the first write.
    let index = read_index(&root)?;
    let restored_text = match (&sidecar.index_line, &index) {
        (Some(line), Some(index)) => {
            let listed = segments(&index.text)
                .iter()
                .any(|segment| line_target(segment).as_deref() == Some(sidecar.file_name.as_str()));
            (!listed).then(|| {
                insert_line(
                    &index.text,
                    line,
                    sidecar.index_line_number.unwrap_or(usize::MAX),
                )
            })
        }
        (Some(line), None) => Some(format!("{line}\n")),
        (None, _) => None,
    };
    if restored_text.is_some() && index.is_some() {
        check_backup_target(&root)?;
    }

    write_atomic(&target, &archived.permissions, &archived.bytes, true)?;

    // The memory file is back. An index problem now does not undo that, so
    // report the index line as not restored instead of a failure.
    let mut index_line_restored = false;
    if let Some(text) = restored_text {
        let committed = match &index {
            Some(index) => commit_index(&root, index, text.as_bytes()),
            None => write_atomic(
                &root.join(INDEX_FILE),
                &archived.permissions,
                text.as_bytes(),
                true,
            ),
        };
        index_line_restored = committed.is_ok();
    }

    sidecar.restored_at_ms = Some(now_ms()?);
    let text = serde_json::to_vec_pretty(&sidecar)
        .map_err(|error| failed("Could not encode the archive record", error))?;
    write_atomic(&sidecar_path, &record.permissions, &text, false)
        .map_err(|_| Stop::Failed("Could not update the archive record".to_owned()))?;
    Ok(MemoryEditOutcome::Restored {
        index_line_restored,
    })
}

#[cfg(not(windows))]
pub(crate) fn remove_index_line(
    home: &Path,
    slug: &str,
    line_number: usize,
    expected_target: &str,
) -> Result<MemoryEditOutcome, String> {
    finish(remove_line(home, slug, line_number, expected_target))
}

#[cfg(not(windows))]
fn remove_line(
    home: &Path,
    slug: &str,
    line_number: usize,
    expected_target: &str,
) -> Result<MemoryEditOutcome, Stop> {
    let root = memory_root(home, slug)?;
    let Some(index) = read_index(&root)? else {
        return Ok(MemoryEditOutcome::Missing);
    };
    let expected = bare_target(expected_target);
    let all = segments(&index.text);
    let matches_line = line_number
        .checked_sub(1)
        .and_then(|position| all.get(position))
        .is_some_and(|segment| line_target(segment).as_deref() == Some(expected));
    if !matches_line {
        let listed = all
            .iter()
            .any(|segment| line_target(segment).as_deref() == Some(expected));
        return Ok(if listed {
            MemoryEditOutcome::ChangedOnDisk
        } else {
            MemoryEditOutcome::Missing
        });
    }
    let position = line_number - 1;
    let kept: Vec<&str> = all
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != position)
        .map(|(_, segment)| *segment)
        .collect();
    let text = join_removed(&kept, &index.text, position + 1 == all.len());
    commit_index(&root, &index, text.as_bytes())?;
    Ok(MemoryEditOutcome::IndexLineRemoved)
}
