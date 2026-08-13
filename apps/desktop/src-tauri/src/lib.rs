// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The antiburn desktop shell.
//!
//! The shell owns windows, the menu-bar item, local persistence, and the IPC
//! surface; every analysis, discovery, and pricing decision belongs to the
//! [`antiburn_local`] engine. Keeping that split sharp is what lets the engine
//! stay network-free and independently testable.
//!
//! # Modules
//!
//! - [`agents`] — translating between the engine's two names for an agent.
//! - [`analytics`] — turning a located transcript into what the views render.
//! - [`commands`] — the IPC surface exposed to the webview.
//! - [`dto`] — the shapes that cross that boundary.
//! - [`export`] — the derived-only session export document.
//! - [`popover`] — the tray-anchored popover window and its show/hide policy.
//! - [`provider_usage`] — per-provider totals derived from local sessions.
//! - [`repositories`] — which repositories on this machine antiburn watches.
//! - [`scan`] — the background scan and its scheduling policy.
//! - [`settings`] — the standalone settings window.
//! - [`store`] — the app's local SQLite database.
//! - [`tray`] — the menu-bar item and its click and menu handling.
//!
//! # Offline by construction
//!
//! Nothing in this crate opens a socket. The engine is network-free by its own
//! contract, the shell adds no HTTP client, and the only network-capable
//! surface in the whole application is the updater plugin — registered in
//! release builds only, so a development run performs no network requests at
//! all. The webview side is held to the same rule by a test
//! (`apps/desktop/tests/offline.test.ts`).

mod agents;
mod analytics;
mod commands;
mod dto;
mod export;
mod popover;
mod provider_usage;
mod repositories;
mod scan;
mod settings;
mod store;
mod tray;

use std::sync::Mutex;

use tauri::{Manager, RunEvent, WindowEvent};

/// The scan scheduler's handle, kept so the app can abort it on exit rather
/// than leaving a task running against a store that is going away.
#[derive(Default)]
struct Scheduler(Mutex<Option<tauri::async_runtime::JoinHandle<()>>>);

/// Builds and runs the application. Returns only when the app exits.
///
/// # Panics
///
/// Panics if the webview runtime, the popover window, the tray item, or the
/// local database cannot be created: none of the four has a meaningful degraded
/// mode.
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::add_scan_root,
            commands::app_info,
            commands::default_scan_roots,
            commands::delete_session_data,
            commands::engine_catalog_version,
            commands::export_session,
            commands::get_provider_usage,
            commands::get_scan_status,
            commands::get_session_analytics,
            commands::get_settings,
            commands::get_subagent_analytics,
            commands::list_recent_sessions,
            commands::list_repositories,
            commands::list_scan_roots,
            commands::open_settings_window,
            commands::refresh_repositories,
            commands::remove_scan_root,
            commands::reveal_source,
            commands::scan_now,
            commands::set_repository_enabled,
            commands::set_settings,
        ])
        .on_window_event(on_window_event)
        .setup(|app| {
            // A menu-bar app owns no Dock icon and no application menu. The
            // bundle declares LSUIElement, but development runs are unbundled,
            // so the policy is also applied here.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Local state lives under the app's own data directory. The engine
            // never chooses this location; the shell does, and hands it to the
            // engine's state helpers as an explicit argument.
            let data_dir = app.path().app_data_dir()?;
            app.manage(store::Store::open(&data_dir)?);
            app.manage(scan::ScanController::default());
            app.manage(Scheduler::default());
            app.manage(popover::PopoverState::default());

            popover::create(app.handle())?;
            tray::create(app.handle())?;

            let handle = scan::spawn_scheduler(app.handle());
            if let Some(scheduler) = app.try_state::<Scheduler>()
                && let Ok(mut slot) = scheduler.0.lock()
            {
                *slot = Some(handle);
            }

            install_updater(app.handle());

            Ok(())
        });

    builder
        .build(tauri::generate_context!())
        .expect("failed to build the antiburn application")
        .run(|app, event| match event {
            // Closing the settings window must not quit a menu-bar app: the
            // tray item is the app's real lifetime.
            RunEvent::ExitRequested { api, code, .. } if code.is_none() => {
                api.prevent_exit();
            }
            // A deliberate quit: stop the scan before the store it writes to
            // is dropped.
            RunEvent::Exit => abort_scheduler(app),
            _ => {}
        });
}

/// Stop the scan scheduler. Safe to call when it never started.
fn abort_scheduler(app: &tauri::AppHandle) {
    let Some(scheduler) = app.try_state::<Scheduler>() else {
        return;
    };
    let Ok(mut slot) = scheduler.0.lock() else {
        return;
    };
    if let Some(handle) = slot.take() {
        handle.abort();
    }
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
            if window.label() == popover::LABEL {
                popover::note_hidden(window.app_handle());
            }
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
