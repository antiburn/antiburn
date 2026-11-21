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
    mut admitted: impl FnMut(&Path) -> bool,
) -> DirectoryChecks {
    if !admitted(directory) {
        return unavailable(directory, "Folder access is not granted.");
    }
    match read_check_files(directory, admitted) {
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

fn read_check_files(
    directory: &Path,
    mut admitted: impl FnMut(&Path) -> bool,
) -> io::Result<Vec<CheckFile>> {
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
        let content = if !admitted(&path) {
            Err(read_failure(
                &path,
                "The check file cannot be inspected. Check file permissions.",
            ))
        } else {
            match entry.file_type() {
                Ok(kind) if !kind.is_file() => continue,
                Ok(_) => read_content(&path)
                    .map_err(|_| read_failure(&path, "The check file cannot be read. Use UTF-8 text within 256 KiB and check file permissions.")),
                Err(_) => Err(read_failure(&path, "The check file cannot be inspected. Check file permissions.")),
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CHECK: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-checks/tests/fixtures/retry-review.yml"
    ));

    #[test]
    fn denied_directory_is_not_enumerated_and_allowed_definitions_still_load() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(temp.path()).unwrap();
        let directory = temp.path().join("checks");
        std::fs::create_dir(&directory).unwrap();
        let definition = directory.join("review.yml");
        std::fs::write(&definition, "name: Review").unwrap();

        let mut probed = Vec::new();
        let denied = load_directory(&store, &directory, |path| {
            assert_eq!(path, directory);
            false
        });
        assert!(denied.files.is_empty());
        assert!(denied.error.is_some());

        let allowed = load_directory(&store, &directory, |path| {
            probed.push(path.to_owned());
            true
        });
        assert_eq!(probed, [directory, definition]);
        assert_eq!(allowed.files.len(), 1);
        assert!(allowed.error.is_none());
    }

    #[test]
    fn a_denied_yaml_entry_stays_as_an_override_while_siblings_load() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(temp.path()).unwrap();
        let project_root = temp.path().join("workspace");
        let directory = project_root.join(".antiburn/checks");
        std::fs::create_dir_all(&directory).unwrap();
        let global_directory = temp.path().join("home/.antiburn/checks");
        std::fs::create_dir_all(&global_directory).unwrap();
        let definition = directory.join("review.yml");
        let sibling = directory.join("sibling.yml");
        std::fs::write(&definition, "name: Review").unwrap();
        std::fs::write(&sibling, VALID_CHECK).unwrap();
        std::fs::write(global_directory.join("review.yml"), VALID_CHECK).unwrap();

        let checks = load_directory(&store, &directory, |path| path != definition);

        assert!(checks.error.is_none());
        assert_eq!(checks.files.len(), 2);
        let denied = checks
            .files
            .iter()
            .find(|file| file.path == definition)
            .unwrap();
        assert_eq!(
            denied.content.as_ref().unwrap_err().code,
            DiagnosticCode::DiscoveryReadFailed
        );
        let allowed = checks
            .files
            .iter()
            .find(|file| file.path == sibling)
            .unwrap();
        assert_eq!(allowed.content.as_deref().unwrap(), VALID_CHECK);
        let ids = std::collections::BTreeSet::from(["review".to_owned()]);
        let overrides = super::super::directory_files(&directory, &checks, &ids);
        assert_eq!(overrides.len(), 2);
        assert!(
            overrides
                .iter()
                .any(|file| file.path == definition && file.content.is_err())
        );
        assert!(
            overrides
                .iter()
                .any(|file| file.path == sibling && file.content.is_ok())
        );

        let global = load_directory(&store, &global_directory, |_| true);
        let mut files = global.files;
        files.extend(overrides);
        let result = antiburn_checks::discovery::discover_definitions(
            &antiburn_checks::discovery::DiscoveryConfig {
                global_checks_directory: Some(global_directory),
                project_root: Some(project_root),
            },
            &files,
        );
        assert_eq!(result.definitions.len(), 2);
        let selected = result
            .definitions
            .iter()
            .find(|definition| definition.id == "review")
            .unwrap();
        assert_eq!(
            selected.scope,
            antiburn_checks::discovery::DefinitionScope::Project
        );
        assert!(matches!(
            selected.state,
            antiburn_checks::discovery::DefinitionState::Invalid(_)
        ));
        let sibling = result
            .definitions
            .iter()
            .find(|definition| definition.id == "sibling")
            .unwrap();
        assert!(matches!(
            sibling.state,
            antiburn_checks::discovery::DefinitionState::Ready(_)
        ));
    }
}
