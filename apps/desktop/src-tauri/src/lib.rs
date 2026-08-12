// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The antiburn desktop shell.
//!
//! The shell owns windows, the menu-bar item, and the IPC surface; every
//! analysis, discovery, and pricing decision belongs to the
//! [`antiburn_local`] engine. Keeping that split sharp is what lets the engine
//! stay network-free and independently testable.
//!
//! # Modules
//!
//! - [`commands`] — the IPC surface exposed to the webview.
//! - [`popover`] — the tray-anchored popover window and its show/hide policy.
//! - [`settings`] — the standalone settings window.
//! - [`tray`] — the menu-bar item and its click and menu handling.

mod commands;
mod popover;
mod settings;
mod tray;

use tauri::{Manager, RunEvent, WindowEvent};

/// Builds and runs the application. Returns only when the app exits.
///
/// # Panics
///
/// Panics if the webview runtime, the popover window, or the tray item cannot
/// be created: none of the three has a meaningful degraded mode.
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init());

    builder = builder
        .invoke_handler(tauri::generate_handler![
            commands::engine_catalog_version,
            commands::open_settings_window,
        ])
        .on_window_event(on_window_event)
        .setup(|app| {
            // A menu-bar app owns no Dock icon and no application menu. The
            // bundle declares LSUIElement, but development runs are unbundled,
            // so the policy is also applied here.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            app.manage(popover::PopoverState::default());
            popover::create(app.handle())?;
            tray::create(app.handle())?;

            install_updater(app.handle());

            Ok(())
        });

    builder
        .build(tauri::generate_context!())
        .expect("failed to build the antiburn application")
        .run(|_app, event| {
            // Closing the settings window must not quit a menu-bar app: the
            // tray item is the app's real lifetime.
            if let RunEvent::ExitRequested { api, code, .. } = event
                && code.is_none()
            {
                api.prevent_exit();
            }
        });
}

/// Window policy shared by every window the shell creates.
fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    match event {
        // A popover is dismissed by looking away from it. Anything else that
        // takes focus — another app, the settings window — closes it.
        WindowEvent::Focused(false) if window.label() == popover::LABEL => {
            popover::hide_on_focus_loss(window);
        }
        // Neither window is ever destroyed by the user: the popover hides and
        // the settings window is reused on the next open.
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = window.hide();
        }
        _ => {}
    }
}

/// Registers the GitHub Releases updater.
///
/// Development builds never install it, so `pnpm dev` performs no network
/// requests at all. Registration failure (most likely an unconfigured signing
/// public key) is reported and then ignored: an app that cannot check for
/// updates must still start.
#[allow(unused_variables)]
fn install_updater(app: &tauri::AppHandle) {
    #[cfg(not(debug_assertions))]
    if let Err(error) = app.plugin(tauri_plugin_updater::Builder::new().build()) {
        eprintln!("antiburn: update checks are disabled ({error})");
    }
}
