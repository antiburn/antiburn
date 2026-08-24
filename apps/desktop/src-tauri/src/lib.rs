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
//! - [`analysis`] — turning a located transcript into what the views render.
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
//! - [`startup_registration`] — applying the packaged app's launch-at-login preference.
//! - [`storage_health`] — whether the local database still accepts writes.
//! - [`store`] — the app's local SQLite database.
//! - [`tray`] — the menu-bar item and its click and menu handling.
//! - [`tray_title`] — the attributed-string text beside the tray glyph.
//! - [`updates`] — whether, and when, the release feed may be contacted.
//! - [`usage_alerts`] — the usage milestone monitor.
//! - [`window_placement`] — where the app's ordinary windows open.
//!
//! # Local by construction
//!
//! antiburn needs no connection to any service of ours — no antiburn account,
//! server, or backend, ever. Everything runs on this machine, as the reader.
//! The provider limit figures on the Usage
//! surface are read from a file an agent already wrote, and one switch, on
//! by default once first-run setup is complete, runs that agent so it
//! refreshes the file — the agent goes online on its own account, exactly as
//! it would if the reader ran it, and this crate still only reads the file it
//! leaves behind (`docs/deviations.md`, D-20 and D-21). Notifications are antiburn's own
//! window, fed by a local event; nothing about one leaves the machine. The one
//! call this crate makes to a service of ours is the updater plugin —
//! registered in release builds only, so a development run makes no such
//! request at all — and the app never depends on it. The webview side is held
//! to the same boundary by a test (`apps/desktop/tests/no-exfiltration.test.ts`).

mod agents;
mod analysis;
mod analytics;
mod commands;
mod consent;
mod disk_monitor;
mod dto;
mod export;
mod global_click;
mod hud;
mod notifications;
mod nudges;
mod onboarding;
mod popover;
mod provider_usage;
mod repositories;
mod scan;
mod settings;
mod startup_registration;
mod storage_health;
mod store;
mod tray;
mod tray_title;
mod updates;
mod usage_alerts;
mod window_lifecycle;
mod window_placement;
mod window_readiness;

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
/// Panics if the webview runtime, tray item, or local database cannot be
/// created: none of the three has a meaningful degraded mode. Windows are
/// created lazily and report their own failures at the interaction that asked
/// for them.
pub fn run() {
    let log_directory_name = if cfg!(debug_assertions) {
        "antiburn-debug"
    } else {
        "antiburn"
    };
    let trace_guard = antiburn_trace::init(&antiburn_trace::TraceConfig {
        log_directory_name,
        debug_build: cfg!(debug_assertions),
    });
    ::tracing::info!(event = "app_started", version = env!("CARGO_PKG_VERSION"));
    let retention_log_dir = trace_guard.log_dir.clone();
    let mut trace_guard = Some(trace_guard);

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
            commands::get_latest_session_activity,
            commands::refresh_live_usage,
            commands::get_scan_status,
            commands::get_folder_permissions,
            commands::request_folder_access,
            commands::restart_onboarding,
            commands::open_folder_access_settings,
            commands::open_github_repo,
            commands::open_overlay_window,
            commands::show_hud_detail,
            commands::hide_hud_detail,
            commands::conceal_hud_detail,
            commands::get_hud_detail_state,
            commands::set_hud_detail_size,
            commands::get_consent_diagnostics,
            commands::recheck_folder_permissions,
            commands::get_session_analysis,
            commands::get_settings,
            commands::get_storage_health,
            commands::get_subagent_analysis,
            commands::hide_popover,
            commands::finish_onboarding,
            commands::note_interaction,
            commands::list_recent_sessions,
            commands::list_repositories,
            commands::list_scan_roots,
            commands::open_settings_window,
            commands::post_test_notification,
            commands::post_sample_notification,
            commands::quit_app,
            commands::refresh_repositories,
            commands::remove_scan_root,
            commands::reveal_source,
            commands::scan_now,
            commands::set_popover_height,
            commands::set_overlay_hover_region,
            commands::set_repository_enabled,
            commands::set_settings,
            commands::take_settings_pane,
            commands::window_ready,
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
                if settings.onboarding_completed {
                    startup_registration::reconcile(app.handle(), settings.launch_at_login);
                }
            }
            app.manage(scan::ScanController::default());
            app.manage(Schedulers::default());
            app.manage(popover::PopoverState::default());
            app.manage(updates::UpdaterState::default());
            app.manage(notifications::NotificationState::default());
            app.manage(storage_health::StorageHealth::default());
            app.manage(settings::PendingPane::default());
            app.manage(settings::SettingsWindowState::default());
            app.manage(onboarding::OnboardingWindowState::default());
            app.manage(nudges::AnchorOverride::default());

            tray::create(app.handle())?;
            // After the tray, and on the main thread: the monitor reaches the
            // menu-bar item to unlight it. The popover itself is lazy, and its
            // dismissal path already treats a missing window as idle.
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
                    ::tracing::warn!(
                        event = "onboarding_window_open_failed",
                        trigger = "startup",
                        error = %error
                    );
                }
            } else {
                settings::schedule_prewarm(app.handle());
            }

            // Registered before the update scheduler starts, so the first
            // automatic check can see whether there is anything to check with.
            install_updater(app.handle());

            // The consented analytics channel (D-027, deviations D-28). Both
            // calls are inert unless this build injected an endpoint AND the
            // reader has finished onboarding with the switch on — see
            // `analytics::allowed`, which is the single gate both go through.
            analytics::install(app.handle());
            analytics::record(
                app.handle(),
                analytics::event::EventName::AppLaunched,
                analytics::event::Facts::default(),
            );

            // The notification window's manager and the chime player. The
            // webview itself is created only when policy delivers a nudge.
            nudges::init(app.handle())?;
            // The live-usage registry: the sources that can prove a
            // provider's own limit figures, and the milestone ledger they
            // feed. Registered before the schedulers so the first pass sees
            // a populated registry rather than an empty one.
            let live_usage = {
                let store = app.state::<store::Store>();
                usage_alerts::LiveUsage::from_store(&store)
            };
            app.manage(live_usage);
            if let Some(schedulers) = app.try_state::<Schedulers>() {
                schedulers.push(scan::spawn_scheduler(app.handle()));
                schedulers.push(updates::spawn_scheduler(app.handle()));
                schedulers.push(usage_alerts::spawn_scheduler(app.handle()));
                schedulers.push(disk_monitor::spawn_disk_monitor(app.handle().clone()));
            }

            Ok(())
        });

    let app = builder
        .build(tauri::generate_context!())
        .expect("failed to build the antiburn application");
    let mut retention_cleanup = retention_log_dir.map(|log_dir| {
        tauri::async_runtime::spawn_blocking(move || {
            match antiburn_trace::clean_old_logs(&log_dir, antiburn_trace::DEFAULT_LOG_MAX_AGE) {
                Ok(removed) => ::tracing::info!(event = "log_retention_cleaned", removed),
                Err(error) => ::tracing::warn!(event = "log_retention_failed", error = %error),
            }
        })
    });
    app.run(move |app, event| match event {
        RunEvent::ExitRequested { api, code, .. }
            if should_prevent_exit(onboarding::is_pending(app), code) =>
        {
            api.prevent_exit();
        }
        // A deliberate quit: stop the background tasks before the store
        // they write to is dropped.
        RunEvent::Exit => {
            abort_schedulers(app);
            finish_retention_cleanup(&mut retention_cleanup);
            if let Some(mut guard) = trace_guard.take() {
                guard.flush();
            }
        }
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
                ::tracing::warn!(
                    event = "onboarding_window_open_failed",
                    trigger = "dock",
                    error = %error
                );
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

/// Wait for the bounded retention sweep before tracing stops.
fn finish_retention_cleanup(handle: &mut Option<tauri::async_runtime::JoinHandle<()>>) {
    if let Some(handle) = handle.take()
        && let Err(error) = tauri::async_runtime::block_on(handle)
    {
        ::tracing::warn!(event = "log_retention_join_failed", error = %error);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClosePolicy {
    Allow,
    HidePopover,
    HideSettings,
    HidePendingOnboarding,
    HideNudge,
}

fn close_policy(label: &str, onboarding_pending: bool) -> ClosePolicy {
    if label == popover::LABEL {
        ClosePolicy::HidePopover
    } else if label == settings::LABEL {
        ClosePolicy::HideSettings
    } else if label == antiburn_nudge::NUDGE_LABEL {
        ClosePolicy::HideNudge
    } else if label == onboarding::LABEL && onboarding_pending {
        ClosePolicy::HidePendingOnboarding
    } else {
        ClosePolicy::Allow
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
        WindowEvent::CloseRequested { api, .. } => {
            match close_policy(window.label(), onboarding::is_pending(window.app_handle())) {
                ClosePolicy::Allow => {}
                ClosePolicy::HidePopover => {
                    api.prevent_close();
                    // Through `popover::hide` rather than `window.hide()`, so
                    // this path answers to the pin like every dismissal does.
                    popover::hide(window.app_handle());
                }
                ClosePolicy::HideSettings => {
                    if let Err(error) = window.hide() {
                        ::tracing::error!(
                            event = "settings_window_hide_on_close_failed",
                            error = %error
                        );
                    } else {
                        api.prevent_close();
                    }
                }
                ClosePolicy::HidePendingOnboarding => {
                    api.prevent_close();
                    // Preserve first-run progress until it is completed. The
                    // Dock and tray can both reopen this same window.
                    let _ = window.hide();
                }
                ClosePolicy::HideNudge => {
                    api.prevent_close();
                    if let Some(manager) = window
                        .app_handle()
                        .try_state::<antiburn_nudge::NudgeManager>()
                    {
                        manager.dismiss();
                    }
                }
            }
        }
        WindowEvent::Destroyed => match window.label() {
            popover::LABEL => popover::rebuild_after_destroy(window.app_handle()),
            settings::LABEL => settings::rebuild_after_destroy(window.app_handle()),
            onboarding::LABEL => onboarding::rebuild_after_destroy(window.app_handle()),
            _ => {}
        },
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
            ::tracing::warn!(event = "updater_disabled_no_public_key");
            return;
        }
        match app.plugin(tauri_plugin_updater::Builder::new().build()) {
            Ok(()) => {
                if let Some(state) = app.try_state::<updates::UpdaterState>() {
                    state.note_registered();
                }
            }
            Err(error) => ::tracing::warn!(
                event = "updater_registration_failed",
                error = %error
            ),
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
    use super::{ClosePolicy, close_policy, finish_retention_cleanup, should_prevent_exit};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

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

    #[test]
    fn retention_cleanup_finishes_before_shutdown_continues() {
        let completed = Arc::new(AtomicBool::new(false));
        let worker_completed = Arc::clone(&completed);
        let mut handle = Some(tauri::async_runtime::spawn_blocking(move || {
            worker_completed.store(true, Ordering::Release);
        }));

        finish_retention_cleanup(&mut handle);

        assert!(completed.load(Ordering::Acquire));
        assert!(handle.is_none());
    }

    #[test]
    fn only_transient_or_incomplete_windows_intercept_close() {
        assert_eq!(
            close_policy(super::popover::LABEL, false),
            ClosePolicy::HidePopover
        );
        assert_eq!(
            close_policy(super::onboarding::LABEL, true),
            ClosePolicy::HidePendingOnboarding
        );
        assert_eq!(
            close_policy(super::onboarding::LABEL, false),
            ClosePolicy::Allow
        );
        assert_eq!(
            close_policy(super::settings::LABEL, false),
            ClosePolicy::HideSettings
        );
        assert_eq!(
            close_policy(antiburn_nudge::NUDGE_LABEL, false),
            ClosePolicy::HideNudge
        );
    }
}
