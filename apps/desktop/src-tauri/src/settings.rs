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
use std::time::{Duration, Instant};

use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::window_placement::center_on_active_monitor;
use crate::window_readiness::{
    OpenAction, PrewarmAction, ReadyAction, STALE_LOAD_AFTER, WindowReadiness,
    renderer_generation_script,
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

impl SettingsWindowState {
    fn lock(&self) -> std::sync::MutexGuard<'_, WindowReadiness> {
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

/// Delay that staggers Settings after the main popover prewarm.
const SETTINGS_PREWARM_DELAY: Duration = Duration::from_millis(750);

/// Schedule one hidden Settings load after launch or onboarding.
pub fn schedule_prewarm(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SETTINGS_PREWARM_DELAY).await;
        let prewarm_app = app.clone();
        if let Err(error) = app.run_on_main_thread(move || prewarm(&prewarm_app)) {
            ::tracing::warn!(event = "settings_prewarm_schedule_failed", error = %error);
        }
    });
}

fn prewarm(app: &AppHandle) {
    if crate::onboarding::is_pending(app) {
        return;
    }
    let state = app.state::<SettingsWindowState>();
    let action = {
        let mut readiness = state.lock();
        readiness.request_prewarm(Instant::now())
    };
    let PrewarmAction::StartLoading { generation } = action else {
        return;
    };
    if let Err(error) = build(app, generation) {
        cancel_load(app, generation);
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
        let mut readiness = state.lock();
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
        let mut readiness = state.lock();
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
            if !state.lock().defer_build_until_destroyed(generation) {
                return Ok(());
            }
            if let Err(error) = existing.destroy() {
                cancel_load(app, generation);
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
    let generation = app
        .state::<SettingsWindowState>()
        .lock()
        .begin_deferred_build(Instant::now());
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
    arm_stale_warning(app, generation);

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
        .on_page_load(trace_page_load);

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
            cancel_load(app, generation);
            return Err(error);
        }
    };
    center_on_active_monitor(&window, WIDTH, HEIGHT);
    Ok(())
}

/// Reveal Settings after React commits its shell.
pub fn renderer_ready(window: &tauri::WebviewWindow, generation: u64) {
    let app = window.app_handle();
    let action = app
        .state::<SettingsWindowState>()
        .lock()
        .renderer_ready(generation, Instant::now());
    match action {
        ReadyAction::Reveal { loading_for } => {
            ::tracing::info!(
                event = "window_renderer_ready",
                window = LABEL,
                loading_ms = loading_for.as_millis() as u64,
                reveal = true
            );
            if let Err(error) = show(window) {
                ::tracing::error!(event = "window_reveal_failed", window = LABEL, error = %error);
            }
        }
        ReadyAction::StayHidden { loading_for } => {
            ::tracing::info!(
                event = "window_renderer_ready",
                window = LABEL,
                loading_ms = loading_for.as_millis() as u64,
                reveal = false
            );
        }
        ReadyAction::None => {}
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

fn trace_page_load(window: tauri::WebviewWindow, payload: tauri::webview::PageLoadPayload<'_>) {
    let phase = match payload.event() {
        PageLoadEvent::Started => "started",
        PageLoadEvent::Finished => "finished",
    };
    let loading_ms = window
        .app_handle()
        .state::<SettingsWindowState>()
        .lock()
        .loading_duration(Instant::now())
        .map(|duration| duration.as_millis() as u64);
    ::tracing::debug!(
        event = "window_page_load",
        window = LABEL,
        phase,
        loading_ms
    );
}

fn arm_stale_warning(app: &AppHandle, generation: u64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STALE_LOAD_AFTER).await;
        if app
            .state::<SettingsWindowState>()
            .lock()
            .warning_is_current(generation, Instant::now())
        {
            ::tracing::warn!(
                event = "window_renderer_ready_timeout",
                window = LABEL,
                generation,
                timeout_ms = STALE_LOAD_AFTER.as_millis() as u64
            );
        }
    });
}

fn cancel_load(app: &AppHandle, generation: u64) {
    let state = app.state::<SettingsWindowState>();
    let mut readiness = state.lock();
    if readiness.loading_generation() == Some(generation) {
        readiness.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_uses_the_settings_entry() {
        assert_eq!(URL, "settings.html");
    }

    #[test]
    fn prewarm_staggers_settings_after_the_main_window() {
        assert_eq!(
            SETTINGS_PREWARM_DELAY,
            std::time::Duration::from_millis(750)
        );
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
