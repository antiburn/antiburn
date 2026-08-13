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
//! - [`notifications`] — the two things worth interrupting a reader for.
//! - [`popover`] — the tray-anchored popover window and its show/hide policy.
//! - [`provider_usage`] — per-provider totals derived from local sessions.
//! - [`repositories`] — which repositories on this machine antiburn watches.
//! - [`scan`] — the background scan and its scheduling policy.
//! - [`settings`] — the standalone settings window.
//! - [`storage_health`] — whether the local database still accepts writes.
//! - [`store`] — the app's local SQLite database.
//! - [`tray`] — the menu-bar item and its click and menu handling.
//! - [`updates`] — whether, and when, the release feed may be contacted.
//!
//! # Offline by construction
//!
//! Nothing in this crate opens a socket. The engine is network-free by its own
//! contract, the shell adds no HTTP client, and the only network-capable
//! surface in the whole application is the updater plugin — registered in
//! release builds only, so a development run performs no network requests at
//! all. The notification plugin talks to the platform's local notification
//! centre and nothing else. The webview side is held to the same rule by a test
//! (`apps/desktop/tests/offline.test.ts`).

mod agents;
mod analytics;
mod commands;
mod dto;
mod export;
mod notifications;
mod popover;
mod provider_usage;
mod repositories;
mod scan;
mod settings;
mod storage_health;
mod store;
mod tray;
mod updates;

use std::sync::Mutex;

use tauri::{Manager, RunEvent, WindowEvent};

/// Handles of the app's background tasks, kept so it can abort them on exit
/// rather than leaving them running against a store that is going away.
#[derive(Default)]
struct Schedulers(Mutex<Vec<tauri::async_runtime::JoinHandle<()>>>);

impl Schedulers {
    fn push(&self, handle: tauri::async_runtime::JoinHandle<()>) {
        if let Ok(mut handles) = self.0.lock() {
            handles.push(handle);
        }
    }
}

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
        // Local notifications, driven from Rust only: the webview is granted no
        // notification permission, so `notifications` is the single place that
        // decides what is worth interrupting a reader for.
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::add_scan_root,
            commands::app_info,
            commands::cancel_scan,
            commands::clear_local_index,
            commands::default_scan_roots,
            commands::delete_session_data,
            commands::engine_catalog_version,
            commands::export_session,
            commands::get_provider_usage,
            commands::get_scan_status,
            commands::get_session_analytics,
            commands::get_settings,
            commands::get_storage_health,
            commands::get_subagent_analytics,
            commands::hide_popover,
            commands::list_recent_sessions,
            commands::list_repositories,
            commands::list_scan_roots,
            commands::open_settings_window,
            commands::quit_app,
            commands::refresh_repositories,
            commands::remove_scan_root,
            commands::reveal_source,
            commands::scan_now,
            commands::set_popover_height,
            commands::set_repository_enabled,
            commands::set_settings,
            commands::take_settings_pane,
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
            app.manage(Schedulers::default());
            app.manage(popover::PopoverState::default());
            app.manage(updates::UpdaterState::default());
            app.manage(notifications::NotificationState::default());
            app.manage(storage_health::StorageHealth::default());
            app.manage(settings::PendingPane::default());

            popover::create(app.handle())?;
            tray::create(app.handle())?;

            // Registered before the update scheduler starts, so the first
            // automatic check can see whether there is anything to check with.
            install_updater(app.handle());

            if let Some(schedulers) = app.try_state::<Schedulers>() {
                schedulers.push(scan::spawn_scheduler(app.handle()));
                schedulers.push(updates::spawn_scheduler(app.handle()));
            }

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
            // A deliberate quit: stop the background tasks before the store
            // they write to is dropped.
            RunEvent::Exit => abort_schedulers(app),
            _ => {}
        });
}

/// Stop every background task. Safe to call when none ever started.
fn abort_schedulers(app: &tauri::AppHandle) {
    let Some(schedulers) = app.try_state::<Schedulers>() else {
        return;
    };
    let Ok(mut handles) = schedulers.0.lock() else {
        return;
    };
    for handle in handles.drain(..) {
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
/// requests at all. A release build without a configured signing public key
/// does not install it either: an updater that cannot verify what it downloads
/// is worse than none. Either way the app starts, and
/// [`updates::supported`] reports the truth — it is set from *here*, on the one
/// path where registration actually succeeded, so nothing downstream can claim
/// an update capability this build does not have.
fn install_updater(app: &tauri::AppHandle) {
    #[cfg(not(debug_assertions))]
    {
        if !updates::signing_key_configured(app) {
            eprintln!("antiburn: update checks are disabled (no updater public key is configured)");
            return;
        }
        match app.plugin(tauri_plugin_updater::Builder::new().build()) {
            Ok(()) => {
                if let Some(state) = app.try_state::<updates::UpdaterState>() {
                    state.note_registered();
                }
            }
            Err(error) => eprintln!("antiburn: update checks are disabled ({error})"),
        }
    }
    #[cfg(debug_assertions)]
    {
        // The plugin is the only network-capable surface in the application;
        // a development run must not carry it at all.
        let _ = app;
    }
}
