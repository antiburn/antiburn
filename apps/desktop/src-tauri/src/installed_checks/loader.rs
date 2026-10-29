use std::collections::HashSet;
use std::io::{self, Read};
use std::path::Path;

use antiburn_checks::diagnostics::{Diagnostic, DiagnosticCode};
use antiburn_checks::discovery::CheckFile;
use antiburn_local::paths::protected;

use super::{MAX_DEFINITIONS, MAX_FILE_BYTES};
use crate::store::Store;

pub(super) struct DirectoryChecks {
    pub files: Vec<CheckFile>,
    pub error: Option<Diagnostic>,
}

pub(super) fn load_directory(
    store: &Store,
    directory: &Path,
    granted: &HashSet<String>,
) -> DirectoryChecks {
    if protected::cwd_resolution_blocked(directory, granted) {
        return unavailable(directory, "Folder access is not granted.");
    }
    match read_check_files(directory) {
        Ok(files) => DirectoryChecks { files, error: None },
        Err(error) => {
            if error.kind() == io::ErrorKind::PermissionDenied
                && let Some(name) = protected::protected_dir_name(directory)
                && store.revoke_dir_grant(&name).is_err()
            {
                ::tracing::error!(
                    event = "installed_check_grant_revoke_failed",
                    reason = "storage_write_failed"
                );
            }
            unavailable(
                directory,
                "The checks folder cannot be read. Check folder access and permissions.",
            )
        }
    }
}

pub(super) fn unavailable(directory: &Path, message: &str) -> DirectoryChecks {
    DirectoryChecks {
        files: Vec::new(),
        error: Some(read_failure(directory, message)),
    }
}

pub(super) fn disabled(directory: &Path) -> DirectoryChecks {
    DirectoryChecks {
        files: Vec::new(),
        error: Some(
            Diagnostic::new(DiagnosticCode::DiscoveryPaused, "Repository is disabled.")
                .with_source_path(directory),
        ),
    }
}

pub(super) fn has_check_extension(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("yml" | "yaml")
    )
}

fn read_check_files(directory: &Path) -> io::Result<Vec<CheckFile>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !has_check_extension(&path) {
            continue;
        }
        let content = match entry.file_type() {
            Ok(kind) if !kind.is_file() => continue,
            Ok(_) => read_content(&path)
                .map_err(|_| read_failure(&path, "The check file cannot be read. Use UTF-8 text within 256 KiB and check file permissions.")),
            Err(_) => Err(read_failure(&path, "The check file cannot be inspected. Check file permissions.")),
        };
        if files.len() == MAX_DEFINITIONS {
            return Err(io::Error::other("More than 128 check files"));
        }
        files.push(CheckFile {
            path,
            content,
            enabled: true,
        });
    }
    Ok(files)
}

fn read_content(path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(io::Error::other("Check file exceeds 256 KiB"));
    }
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Check file is not valid UTF-8"))
}

fn read_failure(path: &Path, message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticCode::DiscoveryReadFailed, message).with_source_path(path)
}
