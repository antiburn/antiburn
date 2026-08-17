// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The standalone settings window.
//!
//! Unlike the popover this is an ordinary window with real decorations: a
//! place to read and change configuration, not a transient surface. It is
//! fixed at 960×680, created on first use, and then reused, because a
//! menu-bar app can be asked to open settings many times per session.
//!
//! On macOS the title bar is an overlay: decorations (traffic lights, system
//! shadow, real close semantics) are kept, the bar itself is transparent, and
//! the floating title text is hidden — the frontend paints a
//! `data-tauri-drag-region` strip across the top as the drag handle (see
//! `src/views/SettingsView.tsx`). Windows and Linux keep the stock title bar.

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::window_placement::center_on_active_monitor;

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

/// Fragment the settings window loads. The frontend serves one bundle and
/// selects its view from this fragment (see `src/lib/route.ts`).
const URL: &str = "index.html#/settings";

// Fixed geometry: a 220px sidebar leaves a ≥600px content column, and the
// whole window still fits a 1280×800 display. Non-resizable — every pane is
// designed for exactly this rectangle.
const WIDTH: f64 = 960.0;
const HEIGHT: f64 = 680.0;

/// Shows the settings window, creating it if this is the first request.
///
/// `pane` is the section the caller wants shown — the popover's attention
/// banners use it to land a reader on the pane that can fix what they were told
/// about, instead of on whichever pane they last left open.
pub fn open(app: &AppHandle, pane: Option<String>) -> tauri::Result<()> {
    if let Some(existing) = app.get_webview_window(LABEL) {
        // Re-centered on every open, not just the first: this window hides on
        // close rather than being destroyed, so without this a reader who
        // moved desks (or just works across monitors) would have Settings
        // reappear on whichever display it was last dismissed on. The private
        // app gets the same behavior for free by destroying the window and
        // letting the OS place the rebuild.
        center_on_active_monitor(&existing, WIDTH, HEIGHT);
        existing.show()?;
        existing.unminimize()?;
        existing.set_focus()?;
        // Already mounted, so it will not ask for a pending pane again — and
        // deliberately nothing is left pending, or a later reload of this
        // webview would jump to a pane somebody asked for long ago.
        if let Some(pane) = pane {
            let _ = app.emit_to(LABEL, EVENT_PANE, pane);
        }
        return Ok(());
    }

    if let Some(pending) = app.try_state::<PendingPane>() {
        pending.set(pane);
    }

    // Built hidden and positioned before the first show, so the window never
    // visibly jumps from a default position to the right one. Deliberately no
    // `.center()`: the builder's centering computes against the primary
    // monitor before the window has a screen, which is exactly the "opens on
    // the wrong display" this function exists to avoid.
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(URL.into()))
        .title("antiburn Settings")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .maximizable(false)
        .visible(false);

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

    let window = builder.build()?;
    center_on_active_monitor(&window, WIDTH, HEIGHT);
    window.show()?;
    window.set_focus()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
