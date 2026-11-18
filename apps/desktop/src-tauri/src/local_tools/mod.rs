//! Debug CLI and skill setup, separate from vendor callback registration.
//!
//! See `docs/cli-installation.md` for profile binding and ownership rules.

mod installation;
mod state;

use std::sync::Mutex;

use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::cli_install;

pub(crate) const DEBUG_IDENTIFIER: &str = "ai.antiburn.desktop.debug";

/// Setup state for the debug CLI and skill, not vendor delivery or Cloud sync.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Pending,
    Ready,
    Unavailable,
    Failed,
}

/// Closed local diagnostics. Paths, process output, and raw errors stay private.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    DevelopmentToolsDisabled,
    UnsupportedPlatform,
    CliUnavailable,
    CliTestFailed,
    ProfileUnavailable,
    ProfileChanged,
    OwnedFileChanged,
    OwnershipConflict,
    SetupUnavailable,
}

/// A passed CLI test and installed skill do not prove host discovery or callbacks.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Monotonic within this app process. Ignore responses below the latest revision.
    pub revision: u64,
    /// `ready` requires both the tested CLI digest and owned generated files.
    pub phase: Phase,
    /// The app resolved a receipt-validated internal runtime.
    pub cli_installed: bool,
    /// The resolved runtime passed its bounded, empty-PATH capability test.
    pub cli_tested: bool,
    /// The generated skill and bound config match their ownership receipts.
    pub skill_installed: bool,
    /// A closed setup failure class, not a raw error or filesystem path.
    pub reason: Option<Reason>,
}

impl Status {
    fn initial(reason: Option<Reason>) -> Self {
        Self {
            revision: 0,
            phase: if reason.is_some() {
                Phase::Unavailable
            } else {
                Phase::Pending
            },
            cli_installed: false,
            cli_tested: false,
            skill_installed: false,
            reason,
        }
    }

    fn failed(reason: Reason, cli_installed: bool, cli_tested: bool) -> Self {
        Self {
            revision: 0,
            phase: Phase::Failed,
            cli_installed,
            cli_tested,
            skill_installed: false,
            reason: Some(reason),
        }
    }
}

#[derive(Clone)]
struct Snapshot {
    status: Status,
    tested_sha256: Option<String>,
}

struct Controller {
    state: Mutex<state::State>,
}

fn disabled(app: &tauri::AppHandle) -> Option<Reason> {
    if !cfg!(debug_assertions) || app.config().identifier != DEBUG_IDENTIFIER {
        Some(Reason::DevelopmentToolsDisabled)
    } else if !cfg!(target_os = "macos") {
        Some(Reason::UnsupportedPlatform)
    } else {
        None
    }
}

/// Register state before CLI startup can request debug tool setup.
pub(crate) fn start(app: &tauri::AppHandle) {
    app.manage(Controller {
        state: Mutex::new(state::State::new(Status::initial(disabled(app)))),
    });
}

struct Running<'a> {
    app: &'a tauri::AppHandle,
    controller: &'a Controller,
    worker: u64,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        let status = self
            .controller
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.abort_setup(self.worker));
        if let Some(status) = status {
            let _ = self.app.emit("local-tools:changed", status);
        }
    }
}

/// Reconcile after CLI startup or explicit Repair. One bounded worker owns writes.
pub(crate) fn request_setup(app: &tauri::AppHandle) {
    if disabled(app).is_some() {
        return;
    }
    let controller = app.state::<Controller>();
    let (status, worker) = match controller.state.lock() {
        Ok(mut state) => state.begin_setup(),
        Err(_) => {
            tracing::warn!(event = "local_tools_state_unavailable");
            return;
        }
    };
    let _ = app.emit("local-tools:changed", status);
    let Some(worker) = worker else { return };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let controller = app.state::<Controller>();
        let _running = Running {
            app: &app,
            controller: &controller,
            worker,
        };
        loop {
            let snapshot = setup(&app).await;
            let (status, rerun) = match controller.state.lock() {
                Ok(mut state) => state.finish_setup(worker, snapshot),
                Err(_) => {
                    tracing::warn!(event = "local_tools_state_unavailable");
                    break;
                }
            };
            tracing::debug!(
                event = "local_tools_setup",
                revision = status.revision,
                phase = ?status.phase,
                reason = ?status.reason,
                cli_installed = status.cli_installed,
                cli_tested = status.cli_tested,
                skill_installed = status.skill_installed,
            );
            let _ = app.emit("local-tools:changed", &status);
            if !rerun {
                break;
            }
        }
    });
}

async fn setup(app: &tauri::AppHandle) -> Snapshot {
    let worker = app.clone();
    let prepared = tauri::async_runtime::spawn_blocking(move || {
        cli_install::runtime_for_test(&worker).map_err(|_| Reason::CliUnavailable)
    })
    .await
    .unwrap_or(Err(Reason::SetupUnavailable));
    let (_runtime_lock, runtime) = match prepared {
        Ok(value) => value,
        Err(reason) => {
            return Snapshot {
                status: Status::failed(reason, false, false),
                tested_sha256: None,
            };
        }
    };
    if cli_install::probe::test(&runtime).await.is_err() {
        return Snapshot {
            status: Status::failed(Reason::CliTestFailed, true, false),
            tested_sha256: None,
        };
    }
    let digest = runtime.sha256.clone();
    let worker = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        installation::Installation::for_app(&worker)?.reconcile(&runtime)
    })
    .await
    .unwrap_or(Err(Reason::SetupUnavailable));
    Snapshot {
        status: match result {
            Ok(()) => Status {
                revision: 0,
                phase: Phase::Ready,
                cli_installed: true,
                cli_tested: true,
                skill_installed: true,
                reason: None,
            },
            Err(reason) => Status::failed(reason, true, true),
        },
        tested_sha256: Some(digest),
    }
}

/// Return busy errors without turning temporary contention into stored failure.
fn inspect_ready(
    snapshot: &Snapshot,
    inspected: anyhow::Result<cli_install::CliRuntime>,
    inspect_files: impl FnOnce(&cli_install::CliRuntime) -> Result<(), Reason>,
) -> Result<Result<(), Reason>, String> {
    if inspected.as_ref().is_err_and(|error| {
        matches!(
            error.downcast_ref::<std::fs::TryLockError>(),
            Some(std::fs::TryLockError::WouldBlock)
        )
    }) {
        return Err(
            "Local CLI inspection is busy. Retry after the other operation finishes.".into(),
        );
    }
    Ok((|| {
        let runtime = inspected.map_err(|_| Reason::CliUnavailable)?;
        if Some(runtime.sha256.as_str()) != snapshot.tested_sha256.as_deref() {
            return Err(Reason::CliTestFailed);
        }
        inspect_files(&runtime)
    })())
}

/// Inspect setup without installing files or changing vendor configuration.
/// A ready snapshot remains valid only for its tested CLI digest and owned files.
#[tauri::command]
pub async fn get_local_tools_status(app: tauri::AppHandle) -> Result<Status, String> {
    let snapshot = app
        .state::<Controller>()
        .state
        .lock()
        .map_err(|_| "Local tool status is unavailable.")?
        .snapshot();
    if snapshot.status.phase != Phase::Ready {
        return Ok(snapshot.status);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let checked = inspect_ready(&snapshot, cli_install::inspect_cli(&app), |runtime| {
            installation::Installation::for_app(&app)?.inspect(runtime)
        })?;
        let (status, changed) = app
            .state::<Controller>()
            .state
            .lock()
            .map_err(|_| "Local tool status is unavailable.".to_owned())?
            .apply_inspection(snapshot.status.revision, checked);
        if changed {
            let _ = app.emit("local-tools:changed", &status);
        }
        Ok(status)
    })
    .await
    .map_err(|_| "Local tool status is unavailable.".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_wire_contract_contains_only_closed_setup_metadata() {
        let status = Status {
            revision: 0,
            phase: Phase::Ready,
            cli_installed: true,
            cli_tested: true,
            skill_installed: true,
            reason: None,
        };
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            serde_json::json!({
                "revision": 0, "phase": "ready", "cliInstalled": true, "cliTested": true,
                "skillInstalled": true, "reason": null,
            }),
        );
        let disabled = Status::initial(Some(Reason::DevelopmentToolsDisabled));
        assert_eq!(disabled.phase, Phase::Unavailable);
        assert!(!disabled.cli_installed && !disabled.cli_tested && !disabled.skill_installed);
    }
}
