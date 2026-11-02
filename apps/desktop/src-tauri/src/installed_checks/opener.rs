use std::collections::HashSet;
use std::path::{Path, PathBuf};

use antiburn_local::paths::protected;
use tauri::Manager;
use tauri_plugin_opener::OpenerExt;

use crate::analytics::event::BurnCheckDefinitionOpenOutcome;
use crate::insights_report::installed_checks::InstalledCheckDefinition;
use crate::store::Store;

#[derive(Debug)]
struct DefinitionOpenError {
    outcome: BurnCheckDefinitionOpenOutcome,
    message: &'static str,
}

impl DefinitionOpenError {
    fn unavailable(message: &'static str) -> Self {
        Self {
            outcome: BurnCheckDefinitionOpenOutcome::Unavailable,
            message,
        }
    }

    fn inaccessible(error: std::io::Error, message: &'static str) -> Self {
        Self {
            outcome: if error.kind() == std::io::ErrorKind::NotFound {
                BurnCheckDefinitionOpenOutcome::Missing
            } else {
                BurnCheckDefinitionOpenOutcome::Unavailable
            },
            message,
        }
    }
}

fn definition_path(
    definitions: &[InstalledCheckDefinition],
    reference: &str,
    granted: &HashSet<String>,
) -> Result<PathBuf, DefinitionOpenError> {
    let source = definitions
        .iter()
        .flat_map(|definition| &definition.definition_sources)
        .find(|source| source.reference == reference)
        .ok_or(DefinitionOpenError {
            outcome: BurnCheckDefinitionOpenOutcome::Missing,
            message: "This check file is no longer available. Refresh Checks and try again.",
        })?;
    let path = Path::new(&source.path);
    if super::diagnostics::source(path).reference != source.reference {
        return Err(DefinitionOpenError::unavailable(
            "The check file path cannot be opened on this system.",
        ));
    }
    if !path.is_absolute()
        || !matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("yml" | "yaml")
        )
    {
        return Err(DefinitionOpenError::unavailable(
            "This check does not have a local YAML file to open.",
        ));
    }
    if protected::cwd_resolution_blocked(path, granted) {
        return Err(DefinitionOpenError::unavailable(
            "Allow access to this folder in Settings → Sources, then try again.",
        ));
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| DefinitionOpenError::inaccessible(error, "The check file cannot be accessed. Check that it still exists and that you can read it."))?;
    if !metadata.is_file() {
        return Err(DefinitionOpenError::unavailable(
            "The check file was replaced. Refresh Checks before opening it.",
        ));
    }
    let resolved = std::fs::canonicalize(path).map_err(|error| {
        DefinitionOpenError::inaccessible(
            error,
            "The check file cannot be accessed. Check its folder permissions and try again.",
        )
    })?;
    if protected::cwd_resolution_blocked(&resolved, granted) {
        return Err(DefinitionOpenError::unavailable(
            "Allow access to this folder in Settings → Sources, then try again.",
        ));
    }
    Ok(crate::commands::presentable(resolved))
}

fn open_definition_file(
    definitions: &[InstalledCheckDefinition],
    reference: &str,
    granted: &HashSet<String>,
    validate_local: impl FnOnce(&Path) -> Result<(), ()>,
    open: impl FnOnce(String) -> Result<(), ()>,
) -> Result<(), DefinitionOpenError> {
    let path = definition_path(definitions, reference, granted)?;
    validate_local(&path).map_err(|()| {
        DefinitionOpenError::unavailable("This check file is not available as a local definition.")
    })?;
    let path = path.into_os_string().into_string().map_err(|_| {
        DefinitionOpenError::unavailable("The check file path cannot be opened on this system.")
    })?;
    open(path).map_err(|()| DefinitionOpenError {
        outcome: BurnCheckDefinitionOpenOutcome::OpenFailed,
        message: "The check file could not be opened. Choose a default application for YAML files and try again.",
    })
}

fn complete_definition_open(
    result: Result<(), DefinitionOpenError>,
    record: impl FnOnce(BurnCheckDefinitionOpenOutcome),
) -> Result<(), String> {
    record(match &result {
        Ok(()) => BurnCheckDefinitionOpenOutcome::Succeeded,
        Err(error) => error.outcome,
    });
    result.map_err(|error| error.message.to_owned())
}

#[tauri::command]
pub(crate) async fn open_installed_check_definition(
    app: tauri::AppHandle,
    source_reference: String,
) -> Result<(), String> {
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let definitions = super::metadata(&worker_app);
        let granted = worker_app.state::<Store>().granted_dirs().map_err(|_| {
            DefinitionOpenError::unavailable(
                "Folder access settings are unavailable. Try again after refreshing Checks.",
            )
        })?;
        open_definition_file(
            &definitions,
            &source_reference,
            &granted,
            |path| crate::commands::reject_remote_cache_path(&worker_app, path).map_err(|_| ()),
            |path| {
                worker_app
                    .opener()
                    .open_path(path, None::<&str>)
                    .map_err(|_| ())
            },
        )
    })
    .await
    .unwrap_or(Err(DefinitionOpenError {
        outcome: BurnCheckDefinitionOpenOutcome::OpenFailed,
        message: "The check file could not be opened. Try again.",
    }));
    complete_definition_open(result, |outcome| {
        crate::analytics::record_burn_check_definition_opened(&app, outcome);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installed_checks::diagnostics;

    #[test]
    fn each_explicit_definition_open_reports_one_typed_completion_and_never_false_success() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private-check-name.yml");
        std::fs::write(&path, "name: private YAML value").unwrap();
        let mut definition =
            crate::insights_report::installed_checks::tests::definition("/private/project");
        let source = diagnostics::source(&path);
        definition.definition_sources = vec![source.clone()];
        for (stage, expected) in [
            ("success", BurnCheckDefinitionOpenOutcome::Succeeded),
            ("opener", BurnCheckDefinitionOpenOutcome::OpenFailed),
            ("cache", BurnCheckDefinitionOpenOutcome::Unavailable),
            ("reference", BurnCheckDefinitionOpenOutcome::Missing),
            ("missing", BurnCheckDefinitionOpenOutcome::Missing),
        ] {
            if stage == "missing" {
                std::fs::remove_file(&path).unwrap();
            }
            let mut opens = 0;
            let result = open_definition_file(
                std::slice::from_ref(&definition),
                if stage == "reference" {
                    "unissued-reference"
                } else {
                    &source.reference
                },
                &HashSet::new(),
                |_| if stage == "cache" { Err(()) } else { Ok(()) },
                |opened| {
                    opens += 1;
                    assert_eq!(
                        Path::new(&opened),
                        crate::commands::presentable(std::fs::canonicalize(&path).unwrap())
                    );
                    if stage == "opener" { Err(()) } else { Ok(()) }
                },
            );
            let mut outcomes = Vec::new();
            let result = complete_definition_open(result, |outcome| outcomes.push(outcome));
            assert_eq!(outcomes, [expected], "{stage}");
            assert_eq!(result.is_ok(), stage == "success");
            assert_eq!(opens, usize::from(matches!(stage, "success" | "opener")));
            if let Err(message) = result {
                assert!(!message.contains("private"));
                assert!(!message.contains(&source.reference));
            }
        }
    }

    #[test]
    fn access_and_worker_failures_each_report_one_completion() {
        for error in [
            DefinitionOpenError::unavailable("Folder access settings are unavailable."),
            DefinitionOpenError {
                outcome: BurnCheckDefinitionOpenOutcome::OpenFailed,
                message: "The check file could not be opened. Try again.",
            },
        ] {
            let expected = error.outcome;
            let mut outcomes = Vec::new();
            assert!(
                complete_definition_open(Err(error), |outcome| outcomes.push(outcome)).is_err()
            );
            assert_eq!(outcomes, [expected]);
        }
    }

    #[test]
    fn opens_only_issued_existing_regular_yaml_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Bad_Name.yml");
        std::fs::write(&path, "name: [").unwrap();
        let mut definition =
            crate::insights_report::installed_checks::tests::definition("/workspace/demo");
        let source = diagnostics::source(&path);
        definition.definition_sources = vec![source.clone()];
        assert_eq!(
            definition_path(
                std::slice::from_ref(&definition),
                &source.reference,
                &HashSet::new()
            )
            .unwrap(),
            crate::commands::presentable(std::fs::canonicalize(&path).unwrap())
        );
        assert!(
            definition_path(
                std::slice::from_ref(&definition),
                &path.to_string_lossy(),
                &HashSet::new()
            )
            .is_err()
        );
        assert!(definition_path(&[], &source.reference, &HashSet::new()).is_err());
        std::fs::remove_file(&path).unwrap();
        let error = definition_path(
            std::slice::from_ref(&definition),
            &source.reference,
            &HashSet::new(),
        )
        .unwrap_err();
        assert!(error.message.contains("still exists"));
        assert!(!error.message.contains("os error"));
        std::fs::create_dir(&path).unwrap();
        assert!(
            definition_path(&[definition], &source.reference, &HashSet::new())
                .unwrap_err()
                .message
                .contains("replaced")
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_file_replaced_with_a_symbolic_link() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("check.yml");
        let target = directory.path().join("other.yml");
        std::fs::write(&path, "name: original").unwrap();
        std::fs::write(&target, "name: other").unwrap();
        let source = diagnostics::source(&path);
        let mut definition =
            crate::insights_report::installed_checks::tests::definition("/workspace/demo");
        definition.definition_sources = vec![source.clone()];
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(
            definition_path(&[definition], &source.reference, &HashSet::new())
                .unwrap_err()
                .message
                .contains("replaced")
        );
    }

    #[test]
    fn an_issued_reference_opens_the_latest_regular_file_after_an_atomic_edit_and_reload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("check.yaml");
        std::fs::write(&path, "name: [").unwrap();
        let mut definition =
            crate::insights_report::installed_checks::tests::definition("/workspace/demo");
        let reference = diagnostics::source(&path);
        definition.definition_sources = vec![reference.clone()];
        let state = super::super::InstalledChecks::default();
        super::super::publish_metadata(&state, std::slice::from_ref(&definition));
        let replacement = directory.path().join("check.new");
        std::fs::write(&replacement, "name: repaired").unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        definition.name = "Repaired".into();
        definition.definition_sources = vec![diagnostics::source(&path)];
        assert!(super::super::publish_metadata(&state, &[definition]));
        let snapshot = state.metadata.read().unwrap().clone();
        let opened = definition_path(&snapshot, &reference.reference, &HashSet::new()).unwrap();
        assert_eq!(std::fs::read_to_string(opened).unwrap(), "name: repaired");
        assert!(state.metadata.try_write().is_ok());
    }
}
