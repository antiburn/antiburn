pub(crate) mod probe;

use crate::analytics::{self, event::CliOperation};
pub use antiburn_cli_runtime::{CliInstallState, CliRuntime};
use antiburn_cli_runtime::{Installation, RuntimeDigestCache, file_name};
use std::sync::Mutex;
use tauri::{Emitter, Manager};

fn installation(app: &tauri::AppHandle) -> Result<Installation, String> {
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let home = app.path().home_dir().map_err(|e| e.to_string())?;
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
        home.join(".local").join("bin").join(file_name())
    };
    Ok(Installation {
        root: data.join("cli"),
        terminal,
        payload: app
            .path()
            .resource_dir()
            .map_err(|e| e.to_string())?
            .join("cli-payload"),
        target: env!("ANTIBURN_CLI_TARGET").into(),
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
        let result = tauri::async_runtime::spawn_blocking(move || {
            let i = installation(&worker_app)?;
            let diagnostic = i.reconcile_at_startup().err().map(|e| e.to_string());
            Ok(i.state(diagnostic))
        })
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
        let i = installation(&worker_app)?;
        let diagnostic = i.reconcile().err().map(|e| e.to_string());
        let state = i.state(diagnostic);
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
