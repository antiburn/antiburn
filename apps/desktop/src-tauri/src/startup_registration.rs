// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Reconcile the stored launch-at-login preference with the operating system.
//!
//! Development builds never mutate login items. Packaged builds use
//! `SMAppService.mainApp` on macOS 13+, the per-user Run key on Windows, and a
//! Desktop Entry on Linux. Failures are deliberately best-effort: the
//! preference remains stored, the app remains usable, and a later launch or
//! toggle retries it.

use tauri::Runtime;

#[cfg(target_os = "linux")]
use tauri::Manager;

use crate::store::AppSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegistrationState {
    Unregistered,
    Enabled,
    RequiresApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegistrationAction {
    None,
    Enable,
    Disable,
    AwaitApproval,
}

fn action_for(desired: bool, state: RegistrationState) -> RegistrationAction {
    match (desired, state) {
        (true, RegistrationState::Enabled) | (false, RegistrationState::Unregistered) => {
            RegistrationAction::None
        }
        (true, RegistrationState::Unregistered) => RegistrationAction::Enable,
        (true, RegistrationState::RequiresApproval) => RegistrationAction::AwaitApproval,
        (false, RegistrationState::Enabled | RegistrationState::RequiresApproval) => {
            RegistrationAction::Disable
        }
    }
}

/// Whether a settings write should touch the platform registration.
///
/// A first-run toggle only records intent. The platform is changed once the
/// flow finishes, or when an already-onboarded reader changes the preference.
pub(crate) fn should_reconcile_after_save(previous: &AppSettings, saved: &AppSettings) -> bool {
    let finished_onboarding = !previous.onboarding_completed && saved.onboarding_completed;
    let preference_changed = previous.launch_at_login != saved.launch_at_login;
    saved.onboarding_completed && (finished_onboarding || preference_changed)
}

/// Apply the desired registration without making a settings write fail.
pub fn reconcile<R: Runtime>(app: &tauri::AppHandle<R>, desired: bool) {
    if !registration_is_active(cfg!(debug_assertions), cfg!(feature = "distribution")) {
        let _ = (app, desired);
        return;
    }
    reconcile_platform(app, desired);
}

fn registration_is_active(debug_assertions: bool, distribution: bool) -> bool {
    !debug_assertions && distribution
}

#[cfg(target_os = "macos")]
fn reconcile_platform<R: Runtime>(app: &tauri::AppHandle<R>, desired: bool) {
    // AppService is !Send/!Sync and ServiceManagement requires the application
    // thread. Settings writes can arrive on a command worker, so construct and
    // use the service inside the main-thread closure.
    if let Err(error) = app.run_on_main_thread(move || reconcile_macos(desired)) {
        eprintln!("antiburn: could not schedule launch-at-login reconciliation ({error})");
    }
}

#[cfg(target_os = "macos")]
fn reconcile_macos(desired: bool) {
    use smappservice_rs::{AppService, ServiceManagementError, ServiceStatus, ServiceType};

    let service = AppService::new(ServiceType::MainApp);
    let state = match service.status() {
        ServiceStatus::Enabled => RegistrationState::Enabled,
        ServiceStatus::RequiresApproval => RegistrationState::RequiresApproval,
        ServiceStatus::NotRegistered | ServiceStatus::NotFound => RegistrationState::Unregistered,
    };

    match action_for(desired, state) {
        RegistrationAction::None => {}
        RegistrationAction::AwaitApproval => {
            eprintln!(
                "antiburn: launch at login needs approval in System Settings > General > Login Items"
            );
        }
        RegistrationAction::Enable => match service.register() {
            Ok(()) | Err(ServiceManagementError::AlreadyRegistered) => {}
            Err(error) => {
                eprintln!("antiburn: could not enable launch at login ({error})");
            }
        },
        RegistrationAction::Disable => match service.unregister() {
            Ok(()) | Err(ServiceManagementError::JobNotFound) => {}
            Err(error) => {
                eprintln!("antiburn: could not disable launch at login ({error})");
            }
        },
    }
}

#[cfg(target_os = "windows")]
fn reconcile_platform<R: Runtime>(app: &tauri::AppHandle<R>, desired: bool) {
    if let Err(error) = reconcile_windows(app, desired) {
        eprintln!("antiburn: could not update launch-at-login registration ({error})");
    }
}

#[cfg(target_os = "windows")]
fn reconcile_windows<R: Runtime>(app: &tauri::AppHandle<R>, desired: bool) -> anyhow::Result<()> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};

    const RUN_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run";
    const STARTUP_APPROVED_KEY: &str =
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";
    const ENABLED: [u8; 12] = [0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let run = hkcu.open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_SET_VALUE)?;
    if desired {
        // Always write the current path: an installed update or a moved
        // portable build must not leave a present-but-stale registration.
        let command = windows_run_command(&std::env::current_exe()?.to_string_lossy());
        run.set_value("antiburn", &command)?;

        // If Task Manager has an override row, make an in-app opt-in effective
        // there too. Absence is the normal enabled state and needs no row.
        if let Ok(approved) = hkcu.open_subkey_with_flags(STARTUP_APPROVED_KEY, KEY_SET_VALUE) {
            approved.set_raw_value(
                "antiburn",
                &winreg::RegValue {
                    vtype: winreg::enums::RegType::REG_BINARY,
                    bytes: ENABLED.to_vec(),
                },
            )?;
        }
    } else if let Err(error) = run.delete_value("antiburn")
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error.into());
    }
    let _ = app;
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
fn windows_run_command(executable: &str) -> String {
    // Windows filenames cannot contain a quote, so surrounding the complete
    // executable path is sufficient when no startup arguments are passed.
    format!("\"{executable}\"")
}

#[cfg(target_os = "linux")]
fn reconcile_platform<R: Runtime>(app: &tauri::AppHandle<R>, desired: bool) {
    if let Err(error) = reconcile_linux(app, desired) {
        eprintln!("antiburn: could not update launch-at-login registration ({error})");
    }
}

#[cfg(target_os = "linux")]
fn reconcile_linux<R: Runtime>(app: &tauri::AppHandle<R>, desired: bool) -> anyhow::Result<()> {
    let file = app.path().config_dir()?.join("autostart/antiburn.desktop");
    if !desired {
        if let Err(error) = std::fs::remove_file(&file)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error.into());
        }
        return Ok(());
    }

    let executable = app
        .env()
        .appimage
        .map(std::path::PathBuf::from)
        .map(Ok)
        .unwrap_or_else(std::env::current_exe)?;
    let executable = executable
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("application path is not valid UTF-8"))?;
    let contents = linux_desktop_entry(executable);

    if std::fs::read_to_string(&file)
        .as_deref()
        .is_ok_and(|current| current == contents.as_str())
    {
        return Ok(());
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(file, contents)?;
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
fn linux_desktop_entry(executable: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName=antiburn\nComment=Start antiburn at login\nExec={}\nStartupNotify=false\nTerminal=false\n",
        desktop_exec_quote(executable)
    )
}

#[cfg(any(target_os = "linux", test))]
fn desktop_exec_quote(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        if matches!(character, '"' | '`' | '$' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('"');
    escaped
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn reconcile_platform<R: Runtime>(_app: &tauri::AppHandle<R>, _desired: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_actions_cover_each_platform_state() {
        assert_eq!(
            action_for(true, RegistrationState::Unregistered),
            RegistrationAction::Enable
        );
        assert_eq!(
            action_for(true, RegistrationState::Enabled),
            RegistrationAction::None
        );
        assert_eq!(
            action_for(true, RegistrationState::RequiresApproval),
            RegistrationAction::AwaitApproval
        );
        assert_eq!(
            action_for(false, RegistrationState::Unregistered),
            RegistrationAction::None
        );
        assert_eq!(
            action_for(false, RegistrationState::Enabled),
            RegistrationAction::Disable
        );
        assert_eq!(
            action_for(false, RegistrationState::RequiresApproval),
            RegistrationAction::Disable
        );
    }

    #[test]
    fn only_distribution_release_builds_touch_login_items() {
        assert!(!registration_is_active(true, false));
        assert!(!registration_is_active(true, true));
        assert!(!registration_is_active(false, false));
        assert!(registration_is_active(false, true));
    }

    #[test]
    fn platform_commands_quote_paths_with_spaces_and_reserved_characters() {
        assert_eq!(
            windows_run_command(r"C:\Program Files\antiburn\antiburn.exe"),
            r#""C:\Program Files\antiburn\antiburn.exe""#
        );
        assert!(
            linux_desktop_entry("/opt/anti burn/$stable`/antiburn")
                .contains(r#"Exec="/opt/anti burn/\$stable\`/antiburn""#)
        );
    }

    #[test]
    fn onboarding_defers_registration_until_the_flow_finishes() {
        let previous = AppSettings {
            launch_at_login: false,
            ..AppSettings::default()
        };
        let toggled = AppSettings {
            launch_at_login: true,
            ..previous.clone()
        };
        assert!(!should_reconcile_after_save(&previous, &toggled));

        let finished = AppSettings {
            onboarding_completed: true,
            ..toggled.clone()
        };
        assert!(should_reconcile_after_save(&toggled, &finished));
    }

    #[test]
    fn a_completed_install_reconciles_only_when_the_preference_changes() {
        let previous = AppSettings {
            onboarding_completed: true,
            ..AppSettings::default()
        };
        let unrelated = AppSettings {
            activity_window_days: 14,
            ..previous.clone()
        };
        assert!(!should_reconcile_after_save(&previous, &unrelated));

        let disabled = AppSettings {
            launch_at_login: false,
            ..previous.clone()
        };
        assert!(should_reconcile_after_save(&previous, &disabled));
    }
}
