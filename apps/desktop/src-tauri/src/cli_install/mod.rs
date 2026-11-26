pub(crate) mod probe;

use crate::analytics::{self, event::CliOperation};
pub use antiburn_cli_runtime::{CliInstallState, CliRuntime};
use antiburn_cli_runtime::{Installation, RuntimeDigestCache, file_name};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Emitter, Manager};

fn installation(app: &tauri::AppHandle) -> Result<Installation, String> {
    let data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot locate the app data directory for the CLI: {e}"))?;
    let terminal = if cfg!(debug_assertions) || !cfg!(feature = "distribution") {
        data.join("cli").join("terminal").join(file_name())
    } else if cfg!(windows) {
        app.path()
            .local_data_dir()
            .map_err(|e| e.to_string())?
            .join("antiburn")
            .join("bin")
            .join(file_name())
    } else {
        app.path()
            .home_dir()
            .map_err(|e| format!("Cannot locate the home directory for CLI cleanup: {e}"))?
            .join(".local")
            .join("bin")
            .join(file_name())
    };
    Ok(Installation {
        root: data.join("cli"),
        terminal,
        payload: None,
        target: env!("ANTIBURN_CLI_TARGET").into(),
    })
}

/// Only startup and Repair need a payload. Read and Test use the installed runtime.
fn payload_directory(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if cfg!(debug_assertions) && tauri::is_dev() {
        // Tauri copies development resources beside the executable. Its resource
        // lookup can miss custom Cargo output directories on macOS.
        let executable = tauri::utils::platform::current_exe()
            .map_err(|e| format!("Cannot locate the development CLI payload: {e}"))?;
        let payload = executable
            .parent()
            .ok_or("Cannot locate the development app directory")?
            .join("cli-payload");
        if !payload.is_dir() {
            return Err("Development CLI payload is missing. Launch with `pnpm --filter @antiburn/desktop dev` to prepare it.".into());
        }
        return Ok(payload);
    }
    app.path()
        .resource_dir()
        .map(|directory| directory.join("cli-payload"))
        .map_err(|e| {
            format!("Cannot locate bundled CLI resources: {e}. Quit and reopen Desktop. If the problem continues, reinstall it.")
        })
}

/// Resolve only the durable runtime whose bytes match its installation receipt.
pub fn resolve_cli(app: &tauri::AppHandle) -> Result<CliRuntime, String> {
    installation(app)?.resolve().map_err(|e| e.to_string())
}

/// Inspect cached receipt-bound bytes while retaining typed lock-contention errors.
pub fn inspect_cli(app: &tauri::AppHandle) -> anyhow::Result<CliRuntime> {
    let cache = app.state::<Mutex<RuntimeDigestCache>>();
    let mut cache = cache
        .lock()
        .map_err(|_| anyhow::anyhow!("CLI cache is unavailable"))?;
    installation(app)
        .map_err(anyhow::Error::msg)?
        .resolve_cached(&mut cache)
}

/// Keep this lock alive while probing or binding the returned runtime.
/// This prevents another managed installer from replacing the tested executable.
pub(crate) fn runtime_for_test(
    app: &tauri::AppHandle,
) -> Result<(std::fs::File, CliRuntime), String> {
    installation(app)?
        .runtime_for_test()
        .map_err(|e| e.to_string())
}

/// Run on the CLI worker. Release skill writes hold the same lock as staging.
fn reconcile(app: &tauri::AppHandle, startup: bool) -> Result<CliInstallState, String> {
    let mut installation = installation(app)?;
    installation.payload = match payload_directory(app) {
        Ok(directory) => Some(directory),
        Err(error) => return Ok(installation.state(Some(error))),
    };
    #[cfg(target_os = "macos")]
    {
        use crate::local_tools::release_skill;
        if release_skill::enabled(app) {
            return Ok(match installation.reconcile_for_binding(startup) {
                Ok((_lock, runtime)) => {
                    release_skill::install(app, &runtime);
                    CliInstallState {
                        runtime: Some(runtime),
                        diagnostic: None,
                    }
                }
                Err(error) => installation.state(Some(format!("{error:#}"))),
            });
        }
    }
    let result = if startup {
        installation.reconcile_at_startup()
    } else {
        installation.reconcile()
    };
    Ok(installation.state(result.err().map(|error| format!("{error:#}"))))
}

pub fn start(app: &tauri::AppHandle) {
    app.manage(Mutex::new(RuntimeDigestCache::default()));
    if !automatic_installation(
        cfg!(feature = "distribution"),
        cfg!(debug_assertions),
        &app.config().identifier,
    ) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let worker_app = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || reconcile(&worker_app, true))
            .await
            .map_err(|e| e.to_string())
            .and_then(|result| result);
        let result = complete(result, healthy, |succeeded| {
            analytics::record_cli_operation(&app, CliOperation::Startup, succeeded)
        });
        match result {
            Ok(state) => {
                if let Some(error) = &state.diagnostic {
                    tracing::warn!(event = "cli_installation_unavailable", error = %error);
                }
                let _ = app.emit_to("settings", "cli-install:state", state);
                crate::local_tools::request_setup(&app);
            }
            Err(error) => {
                tracing::warn!(event = "cli_installation_unavailable", error = %error);
                crate::local_tools::request_setup(&app);
            }
        }
    });
}

fn automatic_installation(distribution: bool, debug: bool, identifier: &str) -> bool {
    (distribution && !debug) || (debug && identifier == crate::local_tools::DEBUG_IDENTIFIER)
}

fn healthy(state: &CliInstallState) -> bool {
    state.runtime.is_some() && state.diagnostic.is_none()
}

fn complete<T>(
    result: Result<T, String>,
    successful: impl FnOnce(&T) -> bool,
    record: impl FnOnce(bool),
) -> Result<T, String> {
    record(result.as_ref().is_ok_and(successful));
    result
}

#[tauri::command]
pub async fn get_cli_install_state(app: tauri::AppHandle) -> Result<CliInstallState, String> {
    tauri::async_runtime::spawn_blocking(move || Ok(installation(&app)?.state(None)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn test_cli_installation(app: tauri::AppHandle) -> Result<CliRuntime, String> {
    let worker_app = app.clone();
    let result = async {
        let (_lock, runtime) =
            tauri::async_runtime::spawn_blocking(move || runtime_for_test(&worker_app))
                .await
                .map_err(|e| e.to_string())??;
        probe::test(&runtime).await.map_err(|e| e.to_string())?;
        Ok(runtime)
    }
    .await;
    complete(
        result,
        |_| true,
        |succeeded| analytics::record_cli_operation(&app, CliOperation::Test, succeeded),
    )
}

#[tauri::command]
pub async fn repair_cli_installation(app: tauri::AppHandle) -> Result<CliInstallState, String> {
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = reconcile(&worker_app, false)?;
        let _ = worker_app.emit_to("settings", "cli-install:state", &state);
        crate::local_tools::request_setup(&worker_app);
        Ok(state)
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|result| result);
    complete(result, healthy, |succeeded| {
        analytics::record_cli_operation(&app, CliOperation::Repair, succeeded)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_setup_keeps_debug_and_release_profiles_separate() {
        assert!(automatic_installation(
            false,
            true,
            crate::local_tools::DEBUG_IDENTIFIER
        ));
        assert!(automatic_installation(true, false, "ai.antiburn.desktop"));
        assert!(!automatic_installation(false, true, "ai.antiburn.desktop"));
        assert!(!automatic_installation(
            false,
            false,
            crate::local_tools::DEBUG_IDENTIFIER
        ));
    }

    #[test]
    fn completion_records_one_outcome_and_preserves_the_result() {
        for result in [Ok(true), Ok(false), Err("worker failed".to_owned())] {
            let expected = result.clone();
            let mut outcomes = Vec::new();
            let output = complete(result, |value| *value, |outcome| outcomes.push(outcome));
            assert_eq!(output, expected);
            assert_eq!(outcomes, vec![expected.is_ok_and(|value| value)]);
        }
    }
}
