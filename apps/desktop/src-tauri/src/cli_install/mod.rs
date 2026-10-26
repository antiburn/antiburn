mod lifecycle;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub use lifecycle::{CliInstallState, CliRuntime};
use lifecycle::{Installation, file_name};
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

pub fn start(app: &tauri::AppHandle) {
    // Source and debug builds never export a command into the user's shell.
    if !cfg!(feature = "distribution") || cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Ok(i) = installation(&app) {
            let diagnostic = i.reconcile(false).err().map(|e| e.to_string());
            if let Some(error) = &diagnostic {
                tracing::warn!(event = "cli_installation_unavailable", error = %error);
            }
            let _ = app.emit_to("settings", "cli-install:state", i.state(diagnostic));
        }
    });
}

#[tauri::command]
pub async fn get_cli_install_state(app: tauri::AppHandle) -> Result<CliInstallState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let i = installation(&app)?;
        let diagnostic = resolve_cli(&app).err();
        Ok(i.state(diagnostic))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn repair_cli_installation(app: tauri::AppHandle) -> Result<CliInstallState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let i = installation(&app)?;
        let diagnostic = i.reconcile(true).err().map(|e| e.to_string());
        Ok(i.state(diagnostic))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn remove_cli_installation(app: tauri::AppHandle) -> Result<CliInstallState, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let i = installation(&app)?;
        let diagnostic = i.remove_terminal().err().map(|e| e.to_string());
        Ok(i.state(diagnostic))
    })
    .await
    .map_err(|e| e.to_string())?
}
