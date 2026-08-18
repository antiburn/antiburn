// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The antiburn desktop shell.
//!
//! The shell owns windows, the menu-bar item, local persistence, and the IPC
//! surface; every analysis, discovery, and pricing decision belongs to the
//! [`antiburn_local`] engine. Keeping that split sharp is what lets the engine
//! stay independent of any service of ours, and independently testable.
//!
//! # Modules
//!
//! - [`agents`] — translating between the engine's two names for an agent.
//! - [`analytics`] — turning a located transcript into what the views render.
//! - [`commands`] — the IPC surface exposed to the webview.
//! - [`disk_monitor`] — free-space polling, the tray readout, the low edge.
//! - [`dto`] — the shapes that cross that boundary.
//! - [`export`] — the derived-only session export document.
//! - [`global_click`] — dismissing the popover on clicks outside the app.
//! - [`notifications`] — the policy on what may interrupt a reader.
//! - [`nudges`] — presentation glue between that policy and the window.
//! - [`onboarding`] — the standalone first-run window.
//! - [`popover`] — the tray-anchored popover window and its show/hide policy.
//! - [`provider_usage`] — per-provider totals derived from local sessions.
//! - [`repositories`] — which repositories on this machine antiburn watches.
//! - [`scan`] — the background scan and its scheduling policy.
//! - [`settings`] — the standalone settings window.
//! - [`storage_health`] — whether the local database still accepts writes.
//! - [`store`] — the app's local SQLite database.
//! - [`tray`] — the menu-bar item and its click and menu handling.
//! - [`tray_title`] — the attributed-string text beside the tray glyph.
//! - [`updates`] — whether, and when, the release feed may be contacted.
//! - [`usage_alerts`] — the spend-anomaly and milestone monitor.
//! - [`window_placement`] — where the app's ordinary windows open.
//!
//! # Local by construction
//!
//! antiburn needs no connection to any service of ours — no antiburn account,
//! server, or backend, ever. Everything runs on this machine, as the reader.
//! The provider limit figures on the Usage
//! surface are read from a file an agent already wrote, and one opt-in
//! setting, default off, runs that agent so it refreshes the file — the agent
//! goes online on its own account, exactly as it would if the reader ran it,
//! and this crate still only reads the file it leaves behind
//! (`docs/deviations.md`, D-20 and D-21). Notifications are antiburn's own
//! window, fed by a local event; nothing about one leaves the machine. The one
//! call this crate makes to a service of ours is the updater plugin —
//! registered in release builds only, so a development run makes no such
//! request at all — and the app never depends on it. The webview side is held
//! to the same boundary by a test (`apps/desktop/tests/no-exfiltration.test.ts`).

mod agents;
mod analytics;
mod commands;
mod consent;
mod disk_monitor;
mod dto;
mod export;
mod global_click;
mod notifications;
mod nudges;
mod onboarding;
mod popover;
mod provider_usage;
mod repositories;
mod scan;
mod settings;
mod storage_health;
mod store;
mod tray;
mod tray_title;
mod updates;
mod usage_alerts;
mod window_placement;

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
    // `register` installs the non-activating-panel support the notification
    // window needs on macOS; a no-op elsewhere.
    let builder = antiburn_nudge::register(tauri::Builder::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::add_scan_root,
            commands::app_info,
            commands::begin_popover_hold,
            commands::cancel_scan,
            commands::clear_local_index,
            commands::default_scan_roots,
            commands::delete_session_data,
            commands::engine_catalog_version,
            commands::end_popover_hold,
            commands::export_session,
            commands::get_provider_usage,
            commands::get_live_usage,
            commands::get_scan_status,
            commands::get_folder_permissions,
            commands::request_folder_access,
            commands::open_folder_access_settings,
            commands::get_consent_diagnostics,
            commands::recheck_folder_permissions,
            commands::get_session_analytics,
            commands::get_settings,
            commands::get_storage_health,
            commands::get_subagent_analytics,
            commands::hide_popover,
            commands::list_recent_sessions,
            commands::list_repositories,
            commands::list_scan_roots,
            commands::open_settings_window,
            commands::post_test_notification,
            commands::quit_app,
            commands::refresh_repositories,
            commands::remove_scan_root,
            commands::reveal_source,
            commands::scan_now,
            commands::set_popover_height,
            commands::set_repository_enabled,
            commands::set_settings,
            commands::take_settings_pane,
            antiburn_nudge::commands::nudge_action,
            antiburn_nudge::commands::nudge_dismiss,
            antiburn_nudge::commands::nudge_ready,
            antiburn_nudge::commands::nudge_resize,
            antiburn_nudge::commands::nudge_reveal,
            antiburn_nudge::commands::nudge_set_hovered,
        ])
        .on_window_event(on_window_event)
        .setup(|app| {
            // A menu-bar app owns no Dock icon and no application menu. The
            // bundle declares LSUIElement, but development runs are unbundled,
            // so the policy is also applied here.
            //
            // Unconditional on purpose, and applied before the store is even
            // open: a completed install must never flash a Dock icon while its
            // settings are being read. The first run overrides it a few lines
            // below — see `onboarding::policy_for` for why it has to.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Local state lives under the app's own data directory. The engine
            // never chooses this location; the shell does, and hands it to the
            // engine's state helpers as an explicit argument.
            let data_dir = app.path().app_data_dir()?;
            app.manage(store::Store::open(&data_dir)?);

            // Apply the persisted theme before any window shows, so the first
            // paint is already in the reader's chosen appearance. "system" and
            // anything unrecognized mean: follow the OS.
            if let Ok(settings) = app.state::<store::Store>().settings() {
                app.set_theme(match settings.theme.as_str() {
                    "light" => Some(tauri::Theme::Light),
                    "dark" => Some(tauri::Theme::Dark),
                    _ => None,
                });
            }
            app.manage(scan::ScanController::default());
            app.manage(Schedulers::default());
            app.manage(popover::PopoverState::default());
            app.manage(updates::UpdaterState::default());
            app.manage(notifications::NotificationState::default());
            app.manage(storage_health::StorageHealth::default());
            app.manage(settings::PendingPane::default());
            app.manage(nudges::AnchorOverride::default());

            popover::create(app.handle())?;
            tray::create(app.handle())?;
            // After both, and on the main thread: the monitor dismisses the
            // popover and reaches the menu-bar item to unlight it, so neither
            // may be missing by the time a click can arrive.
            global_click::install(app.handle());

            // The first run gets a window rather than silence. Everything above
            // is in place by now, so the flow's first paint can already read
            // settings and ask the engine for its default roots.
            //
            // Best-effort on purpose, unlike the four `?`s above: a window that
            // will not build is not a reason to refuse to start, and the
            // menu-bar item still reaches the flow (see `popover::toggle`).
            if onboarding::is_pending(app.handle()) {
                // Before the window, not after: it should be born into an
                // application that already has a Dock presence rather than
                // acquiring one underneath it. The accessory policy applied
                // above stands for every completed install, so nothing flashes
                // a Dock icon while the store is being read.
                onboarding::apply_activation_policy(app.handle(), true);
                if let Err(error) = onboarding::open(app.handle()) {
                    eprintln!("antiburn: could not open the first-run window ({error})");
                }
            }

            // Registered before the update scheduler starts, so the first
            // automatic check can see whether there is anything to check with.
            install_updater(app.handle());

            // The notification window's manager and the chime player. Prewarm
            // is deferred a little so the app has a valid display context —
            // the same reasoning as the popover's own deferred build.
            nudges::init(app.handle())?;
            // The live-usage registry: the sources that can prove a
            // provider's own limit figures, and the milestone ledger they
            // feed. Registered before the schedulers so the first pass sees
            // a populated registry rather than an empty one.
            app.manage(usage_alerts::LiveUsage::new());
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    if let Some(manager) = handle.try_state::<antiburn_nudge::NudgeManager>() {
                        manager.prewarm();
                    }
                });
            }

            if let Some(schedulers) = app.try_state::<Schedulers>() {
                schedulers.push(scan::spawn_scheduler(app.handle()));
                schedulers.push(updates::spawn_scheduler(app.handle()));
                schedulers.push(usage_alerts::spawn_scheduler(app.handle()));
                schedulers.push(disk_monitor::spawn_disk_monitor(app.handle().clone()));
            }

            Ok(())
        });

    builder
        .build(tauri::generate_context!())
        .expect("failed to build the antiburn application")
        .run(|app, event| match event {
            RunEvent::ExitRequested { api, code, .. }
                if should_prevent_exit(onboarding::is_pending(app), code) =>
            {
                api.prevent_exit();
            }
            // A deliberate quit: stop the background tasks before the store
            // they write to is dropped.
            RunEvent::Exit => abort_schedulers(app),
            // Clicking the Dock icon. Only reachable while the first run is
            // pending, because that is the only time antiburn has a Dock icon
            // (see `onboarding::policy_for`) — and it is exactly then that
            // somebody who closed the window early has no other way back to it.
            // A visible affordance that did nothing would be a worse failure
            // than the one the Dock icon is here to fix.
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => {
                if onboarding::is_pending(app)
                    && let Err(error) = onboarding::open(app)
                {
                    eprintln!("antiburn: could not reopen the first-run window ({error})");
                }
            }
            _ => {}
        });
}

/// Whether an exit request should be swallowed.
///
/// A menu-bar app outlives its windows: closing settings, or the popover, must
/// not quit antiburn, and those arrive here with no exit code. Only the shell's
/// own `exit(0)` — the tray menu, the settings sidebar — carries one, which is
/// what distinguishes a deliberate quit from a window close.
///
/// The exception is the first run. While it is pending antiburn is an ordinary
/// Dock application (again, `onboarding::policy_for`), so it has an application
/// menu and a Dock context menu, both offering Quit, and both arriving here
/// with `code: None`. Swallowing those would give the reader a Quit item that
/// silently does nothing at the one moment they have no other way to get rid of
/// the app. During the first run it quits like the ordinary application it is
/// pretending to be.
fn should_prevent_exit(onboarding_pending: bool, code: Option<i32>) -> bool {
    code.is_none() && !onboarding_pending
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
            if window.label() == popover::LABEL {
                // Through `popover::hide` rather than `window.hide()`, so this
                // path answers to the pin like every other dismissal does.
                popover::hide(window.app_handle());
            } else {
                let _ = window.hide();
            }
        }
        _ => {}
    }
}

/// Registers the GitHub Releases updater.
///
/// Development builds never install it, so `pnpm dev` makes no request to a
/// service of ours at all. A release build without a configured signing public key
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
        // The plugin is the only surface that talks to a service of ours;
        // a development run must not carry it at all.
        let _ = app;
    }
}

#[cfg(test)]
mod tests {
    use super::should_prevent_exit;

    #[test]
    fn a_window_close_never_quits_the_finished_menu_bar_app() {
        // Settings and the popover both close with no exit code, and the tray
        // item is the app's real lifetime.
        assert!(should_prevent_exit(false, None));
        // The shell's own `exit(0)` is the deliberate quit and always lands.
        assert!(!should_prevent_exit(false, Some(0)));
        assert!(!should_prevent_exit(true, Some(0)));
    }

    #[test]
    fn cmd_q_works_while_the_first_run_owns_a_dock_icon() {
        // The case that matters. A Regular app's application menu and Dock
        // context menu both offer Quit and both arrive with no code; swallowing
        // them would ship a Quit item that does nothing.
        assert!(!should_prevent_exit(true, None));
    }
}
