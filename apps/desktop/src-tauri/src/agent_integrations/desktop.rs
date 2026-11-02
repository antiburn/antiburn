use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{Emitter, Listener, Manager};

use super::lifecycle::{CliConnection, Installation, status};
use super::process::NativeRunner;
use super::{IntegrationPhase, IntegrationStatus, Vendor};

pub enum CliValidation {
    Fresh,
    Cached,
}

type Resolver =
    dyn Fn(&tauri::AppHandle, CliValidation) -> Result<CliConnection, String> + Send + Sync;

struct Controller {
    statuses: Mutex<Vec<IntegrationStatus>>,
    resolver: Arc<Resolver>,
    running: AtomicBool,
    pending: AtomicBool,
}

struct RunningGuard<'a>(&'a AtomicBool);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn unavailable(reason: &str) -> Vec<IntegrationStatus> {
    Vendor::ALL
        .into_iter()
        .map(|vendor| status(vendor, IntegrationPhase::Unavailable, reason))
        .collect()
}

fn enabled() -> bool {
    cfg!(feature = "distribution") && !cfg!(debug_assertions)
}

/// Start after the managed CLI owner begins its startup reconciliation.
pub fn start(
    app: &tauri::AppHandle,
    resolver: impl Fn(&tauri::AppHandle, CliValidation) -> Result<CliConnection, String>
    + Send
    + Sync
    + 'static,
) {
    app.manage(Controller {
        statuses: Mutex::new(unavailable(if enabled() {
            "startup_pending"
        } else {
            "development_build"
        })),
        resolver: Arc::new(resolver),
        running: AtomicBool::new(false),
        pending: AtomicBool::new(false),
    });
    if !enabled() {
        return;
    }
    let changed = app.clone();
    app.listen_any("cli-install:state", move |_| request_reconcile(&changed));
    request_reconcile(app);
}

fn request_reconcile(app: &tauri::AppHandle) {
    let controller = app.state::<Controller>();
    controller.pending.store(true, Ordering::Release);
    if controller.running.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let controller = app.state::<Controller>();
        loop {
            let running = RunningGuard(&controller.running);
            controller.pending.store(false, Ordering::Release);
            let started = Instant::now();
            let cli = loop {
                if let Ok(cli) = (controller.resolver)(&app, CliValidation::Fresh) {
                    break Some(cli);
                }
                if started.elapsed() >= Duration::from_secs(5) {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(250));
            };
            let statuses = reconcile(&app, cli.as_ref(), None)
                .unwrap_or_else(|_| unavailable("installation_unavailable"));
            if let Ok(mut snapshot) = controller.statuses.lock() {
                *snapshot = statuses.clone();
            }
            let _ = app.emit_to("settings", "agent-integrations:changed", &statuses);
            drop(running);
            if !controller.pending.load(Ordering::Acquire)
                || controller.running.swap(true, Ordering::AcqRel)
            {
                break;
            }
        }
    });
}

fn environment() -> BTreeMap<String, String> {
    capture_environment(|key| std::env::var(key).ok())
}

pub(super) fn capture_environment(
    mut read: impl FnMut(&str) -> Option<String>,
) -> BTreeMap<String, String> {
    [
        "PATH",
        "VOLTA_HOME",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "PI_CODING_AGENT_DIR",
        "PI_AGENT_DIR",
        "OPENCODE_CONFIG_DIR",
        "OPENCODE_CONFIG",
        "OPENCODE_CONFIG_CONTENT",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ]
    .into_iter()
    .filter_map(|key| read(key).map(|value| (key.into(), value)))
    .collect()
}

enum Action {
    Inspect,
    Repair(Vendor),
    Remove(Vendor),
}

fn reconcile(
    app: &tauri::AppHandle,
    cli: Option<&CliConnection>,
    action: Option<Action>,
) -> Result<Vec<IntegrationStatus>, String> {
    let state = app
        .path()
        .app_data_dir()
        .map_err(|_| "application_data_unavailable")?
        .join("agent-integrations");
    let home = app
        .path()
        .home_dir()
        .map_err(|_| "home_unavailable")?
        .canonicalize()
        .map_err(|_| "home_unavailable")?;
    let resources = app
        .path()
        .resource_dir()
        .map_err(|_| "resources_unavailable")?
        .join("agent-integrations/payload");
    let env = environment();
    let installation = Installation {
        state: &state,
        resources: &resources,
        home: &home,
        env: &env,
        runner: &NativeRunner,
    };
    match action {
        Some(Action::Inspect) => return Ok(installation.inspect(cli)),
        Some(Action::Repair(vendor)) => installation.repair(vendor).map_err(str::to_owned)?,
        Some(Action::Remove(vendor)) => installation.remove(vendor).map_err(str::to_owned)?,
        None => (),
    }
    Ok(installation.reconcile(cli))
}

#[tauri::command]
pub async fn get_agent_integrations(
    app: tauri::AppHandle,
) -> Result<Vec<IntegrationStatus>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let controller = app
            .try_state::<Controller>()
            .ok_or("Integration status is unavailable.")?;
        if !enabled() || controller.running.swap(true, Ordering::AcqRel) {
            return controller
                .statuses
                .lock()
                .map(|snapshot| snapshot.clone())
                .map_err(|_| "Integration status is unavailable.".to_owned());
        }
        let running = RunningGuard(&controller.running);
        let cli = (controller.resolver)(&app, CliValidation::Cached).ok();
        let result = reconcile(&app, cli.as_ref(), Some(Action::Inspect));
        if let Ok(statuses) = &result
            && let Ok(mut snapshot) = controller.statuses.lock()
        {
            *snapshot = statuses.clone();
        }
        drop(running);
        if controller.pending.load(Ordering::Acquire) {
            request_reconcile(&app);
        }
        result.map_err(|_| "Could not read integration status.".to_owned())
    })
    .await
    .map_err(|_| "Integration status stopped unexpectedly.".to_owned())?
}

async fn change(app: tauri::AppHandle, action: Action) -> Result<Vec<IntegrationStatus>, String> {
    if !enabled() {
        return Err("Automatic integration setup requires a packaged build.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let controller = app
            .try_state::<Controller>()
            .ok_or("Integration setup is unavailable.")?;
        if controller.running.swap(true, Ordering::AcqRel) {
            return Err("Integration setup is already running.".into());
        }
        let running = RunningGuard(&controller.running);
        let cli = (controller.resolver)(&app, CliValidation::Fresh).ok();
        let result = reconcile(&app, cli.as_ref(), Some(action));
        if let Ok(statuses) = &result {
            if let Ok(mut snapshot) = controller.statuses.lock() {
                *snapshot = statuses.clone();
            }
            let _ = app.emit_to("settings", "agent-integrations:changed", statuses);
        }
        drop(running);
        if controller.pending.load(Ordering::Acquire) {
            request_reconcile(&app);
        }
        result
            .map_err(|_| "Could not change this integration. Its settings may need review.".into())
    })
    .await
    .map_err(|_| "Integration setup stopped unexpectedly.".to_owned())?
}

#[tauri::command]
pub async fn repair_agent_integration(
    app: tauri::AppHandle,
    vendor: Vendor,
) -> Result<Vec<IntegrationStatus>, String> {
    change(app, Action::Repair(vendor)).await
}

#[tauri::command]
pub async fn remove_agent_integration(
    app: tauri::AppHandle,
    vendor: Vendor,
) -> Result<Vec<IntegrationStatus>, String> {
    change(app, Action::Remove(vendor)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_capture_keeps_a_custom_volta_root_for_metadata_lookup() {
        let root = std::env::temp_dir().join("synthetic-volta");
        let root = root.to_string_lossy().into_owned();
        let env = capture_environment(|key| (key == "VOLTA_HOME").then(|| root.clone()));
        assert_eq!(env, BTreeMap::from([("VOLTA_HOME".into(), root)]));
    }

    #[test]
    fn running_flag_is_released_when_a_task_panics() {
        let running = AtomicBool::new(true);
        let result = std::panic::catch_unwind(|| {
            let _running = RunningGuard(&running);
            panic!("fixture task panic");
        });
        assert!(result.is_err());
        assert!(!running.swap(true, Ordering::AcqRel));
        drop(RunningGuard(&running));
        assert!(!running.load(Ordering::Acquire));
    }
}
