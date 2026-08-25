// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The standalone settings window.
//!
//! Unlike the popover this is an ordinary window with real decorations: a
//! place to read and change configuration, not a transient surface. It is
//! fixed at 960×680 and prewarmed after launch. Closing it hides the resident
//! webview, so the next request can show the same ready renderer.
//!
//! On macOS the title bar is an overlay: decorations (traffic lights, system
//! shadow, real close semantics) are kept, the bar itself is transparent, and
//! the floating title text is hidden — the frontend paints a
//! `data-tauri-drag-region` strip across the top as the drag handle (see
//! `src/views/SettingsView.tsx`). Windows and Linux keep the stock title bar.

use std::sync::Mutex;
use std::time::Instant;

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::window_lifecycle::{self, ManagedWindowReadiness};
use crate::window_placement::center_on_active_monitor;
use crate::window_readiness::{
    OpenAction, PrewarmAction, WindowReadiness, renderer_generation_script,
};

/// Window label. Also listed in `capabilities/default.json`.
pub const LABEL: &str = "settings";

/// Event the shell emits to move an *already open* settings window to a pane.
pub const EVENT_PANE: &str = "settings:pane";

/// The pane a caller asked the window to open on, until the window takes it.
///
/// Two paths need this and only one of them can use an event. A window being
/// created has no webview listening yet, so the frontend asks for the pending
/// pane as it mounts ([`crate::commands::take_settings_pane`]); a window that
/// already exists is never re-mounted, so it is told through [`EVENT_PANE`].
/// The value is *taken* rather than read, so a request can never be applied
/// twice or apply late.
#[derive(Default)]
pub struct PendingPane(Mutex<Option<String>>);

impl PendingPane {
    fn set(&self, pane: Option<String>) {
        *self.lock() = pane;
    }

    /// Read and clear the pending pane.
    pub fn take(&self) -> Option<String> {
        self.lock().take()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Renderer lifecycle for the Settings window.
#[derive(Default)]
pub struct SettingsWindowState(Mutex<WindowReadiness>);

impl ManagedWindowReadiness for SettingsWindowState {
    fn readiness(&self) -> std::sync::MutexGuard<'_, WindowReadiness> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Dedicated frontend entry for the settings window.
const URL: &str = "settings.html";

// Fixed geometry: a 220px sidebar leaves a ≥600px content column, and the
// whole window still fits a 1280×800 display. Non-resizable — every pane is
// designed for exactly this rectangle.
const WIDTH: f64 = 960.0;
const HEIGHT: f64 = 680.0;

/// Schedule one hidden Settings load after launch or onboarding.
pub fn schedule_prewarm(app: &AppHandle) {
    let app = app.clone();
    let prewarm_app = app.clone();
    if let Err(error) = app.run_on_main_thread(move || prewarm(&prewarm_app)) {
        ::tracing::warn!(event = "settings_prewarm_schedule_failed", error = %error);
    }
}

fn prewarm(app: &AppHandle) {
    if crate::onboarding::is_pending(app) {
        return;
    }
    let state = app.state::<SettingsWindowState>();
    let action = {
        let mut readiness = state.readiness();
        readiness.request_prewarm(Instant::now())
    };
    let PrewarmAction::StartLoading { generation } = action else {
        return;
    };
    if let Err(error) = build(app, generation) {
        ::tracing::warn!(event = "settings_prewarm_failed", error = %error);
    }
}

/// Shows the settings window, creating it if this is the first request.
///
/// `pane` is the section the caller wants shown — the popover's attention
/// banners use it to land a reader on the pane that can fix what they were told
/// about, instead of on whichever pane they last left open.
pub fn open(app: &AppHandle, pane: Option<String>) -> tauri::Result<()> {
    let state = app.state::<SettingsWindowState>();
    app.state::<PendingPane>().set(pane.clone());
    let Some(existing) = app.get_webview_window(LABEL) else {
        let mut readiness = state.readiness();
        let action = readiness.request_open(Instant::now());
        let generation = match action {
            OpenAction::StartLoading { generation } | OpenAction::Rebuild { generation } => {
                generation
            }
            OpenAction::AwaitReady => return Ok(()),
            OpenAction::Reveal => {
                readiness.reset();
                match readiness.request_open(Instant::now()) {
                    OpenAction::StartLoading { generation } => generation,
                    _ => unreachable!("an idle lifecycle starts loading"),
                }
            }
        };
        drop(readiness);
        return build(app, generation);
    };

    let action = {
        let mut readiness = state.readiness();
        readiness.request_open(Instant::now())
    };
    if pane_event_reaches_renderer(action)
        && let Some(pane) = pane.as_ref()
    {
        app.emit_to(LABEL, EVENT_PANE, pane)?;
    }
    match action {
        OpenAction::Reveal => {
            show(&existing)?;
            Ok(())
        }
        OpenAction::AwaitReady => Ok(()),
        OpenAction::StartLoading { generation } | OpenAction::Rebuild { generation } => {
            if !state.readiness().defer_build_until_destroyed(generation) {
                return Ok(());
            }
            if let Err(error) = existing.destroy() {
                window_lifecycle::cancel_load::<SettingsWindowState>(app, generation);
                return Err(error);
            }
            Ok(())
        }
    }
}

fn pane_event_reaches_renderer(action: OpenAction) -> bool {
    matches!(action, OpenAction::Reveal | OpenAction::AwaitReady)
}

/// Build a deferred replacement after Tauri removes the old window label.
pub fn rebuild_after_destroy(app: &AppHandle) {
    let generation =
        window_lifecycle::begin_deferred_build::<SettingsWindowState>(app, Instant::now());
    let Some(generation) = generation else {
        return;
    };
    if let Err(error) = build(app, generation) {
        ::tracing::error!(event = "window_rebuild_failed", window = LABEL, error = %error);
    }
}

fn build(app: &AppHandle, generation: u64) -> tauri::Result<()> {
    ::tracing::info!(
        event = "window_renderer_load_started",
        window = LABEL,
        generation
    );
    window_lifecycle::arm_stale_warning::<SettingsWindowState>(app, generation, LABEL);

    // Built hidden and positioned before the first show, so the window never
    // visibly jumps from a default position to the right one. Deliberately no
    // `.center()`: the builder's centering computes against the primary
    // monitor before the window has a screen, which is exactly the "opens on
    // the wrong display" this function exists to avoid.
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(URL.into()))
        .initialization_script(renderer_generation_script(generation))
        .title("antiburn Settings")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .maximizable(false)
        .visible(false)
        .on_page_load(|window, payload| {
            window_lifecycle::trace_page_load::<SettingsWindowState>(window, payload, LABEL);
        });

    #[cfg(target_os = "macos")]
    {
        // Overlay keeps decorations while making the title bar transparent;
        // `hidden_title` drops the floating title text. `.title(...)` above
        // stays so Mission Control and accessibility still name the window.
        // The webview covers the bar's area, so the frontend supplies the drag
        // handle (`data-tauri-drag-region` in SettingsView) — the ACL already
        // grants `core:window:allow-start-dragging`.
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }

    let window = match builder.build() {
        Ok(window) => window,
        Err(error) => {
            window_lifecycle::cancel_load::<SettingsWindowState>(app, generation);
            return Err(error);
        }
    };
    center_on_active_monitor(&window, WIDTH, HEIGHT);
    Ok(())
}

/// Reveal Settings after React commits its shell.
pub fn renderer_ready(window: &tauri::WebviewWindow, generation: u64) {
    let app = window.app_handle();
    if window_lifecycle::renderer_ready::<SettingsWindowState>(
        app,
        LABEL,
        generation,
        Instant::now(),
    ) && let Err(error) = show(window)
    {
        ::tracing::error!(event = "window_reveal_failed", window = LABEL, error = %error);
    }
}

fn show(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    center_on_active_monitor(window, WIDTH, HEIGHT);
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    ::tracing::info!(event = "window_revealed", window = LABEL);
    Ok(())
}

/// Hide Settings after it starts work in another window.
pub fn hide(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(LABEL) {
        window.hide()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_uses_the_settings_entry() {
        assert_eq!(URL, "settings.html");
    }

    #[test]
    fn pane_requests_reach_ready_and_prewarming_renderers() {
        assert!(pane_event_reaches_renderer(OpenAction::Reveal));
        assert!(pane_event_reaches_renderer(OpenAction::AwaitReady));
        assert!(!pane_event_reaches_renderer(OpenAction::StartLoading {
            generation: 1
        }));
        assert!(!pane_event_reaches_renderer(OpenAction::Rebuild {
            generation: 2
        }));
    }

    #[test]
    fn a_requested_pane_is_delivered_exactly_once() {
        let pending = PendingPane::default();
        assert_eq!(pending.take(), None, "nothing was requested");

        pending.set(Some("sources".to_string()));
        assert_eq!(pending.take().as_deref(), Some("sources"));
        // Taken, not read: a window opened later must not jump to a pane
        // somebody asked for an hour ago.
        assert_eq!(pending.take(), None);
    }

    #[test]
    fn opening_without_a_pane_clears_an_earlier_request() {
        let pending = PendingPane::default();
        pending.set(Some("sources".to_string()));
        // The gear affordance asks for no pane in particular, and that must not
        // inherit the last banner's destination.
        pending.set(None);
        assert_eq!(pending.take(), None);
    }
}
