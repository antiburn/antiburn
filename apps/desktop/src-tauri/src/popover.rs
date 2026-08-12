// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The tray-anchored popover window.
//!
//! The popover is created once, hidden, at startup and then shown and hidden
//! for the rest of the process lifetime. Creating it lazily would make the
//! first open visibly slower (a webview has to boot and the bundle has to
//! parse) and would lose any state the views accumulate.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{
    AppHandle, Manager, PhysicalPosition, Rect, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    Window,
};

/// Window label. Also listed in `capabilities/default.json`.
pub const LABEL: &str = "popover";

/// Popover width in logical pixels. Fixed: the views size themselves to it.
const WIDTH: f64 = 380.0;

/// Starting height in logical pixels. Later streams animate this per view.
const HEIGHT: f64 = 480.0;

/// Gap in physical pixels between the menu-bar item and the popover edge.
const ANCHOR_GAP: f64 = 6.0;

/// Minimum physical gap between the popover and the edge of its display.
const SCREEN_MARGIN: f64 = 8.0;

/// How long after an automatic dismissal a tray click is treated as part of
/// that dismissal rather than a fresh open.
///
/// Clicking the menu-bar item while the popover is open fires focus loss
/// *before* the tray click arrives, so a naive toggle would hide the popover
/// and immediately reopen it. This window swallows that second half.
const REOPEN_SUPPRESSION: Duration = Duration::from_millis(250);

/// Shared show/hide bookkeeping, registered as Tauri managed state.
#[derive(Default)]
pub struct PopoverState {
    auto_hidden_at: Mutex<Option<Instant>>,
}

impl PopoverState {
    fn record_auto_hide(&self) {
        if let Ok(mut slot) = self.auto_hidden_at.lock() {
            *slot = Some(Instant::now());
        }
    }

    /// True when an automatic dismissal just happened, meaning the tray click
    /// being handled is the same user gesture that caused it.
    fn suppresses_reopen(&self) -> bool {
        let Ok(mut slot) = self.auto_hidden_at.lock() else {
            return false;
        };
        match *slot {
            Some(at) if at.elapsed() < REOPEN_SUPPRESSION => {
                *slot = None;
                true
            }
            _ => false,
        }
    }
}

/// Creates the popover window, hidden and off the taskbar.
pub fn create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .title("antiburn")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .shadow(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false);

    // Let the first click both focus the popover and act on the control under
    // the cursor; a menu-bar surface that eats the first click feels broken.
    #[cfg(target_os = "macos")]
    let builder = builder.accept_first_mouse(true);

    builder.build()
}

/// Handles a click on the menu-bar item.
///
/// `anchor` is the item's screen rectangle as reported by the tray backend.
pub fn toggle(app: &AppHandle, anchor: Rect) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };

    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        note_hidden(app);
        return;
    }

    if let Some(state) = app.try_state::<PopoverState>()
        && state.suppresses_reopen()
    {
        return;
    }

    if let Err(error) = anchor_to(&window, anchor) {
        // Positioning is best-effort: a popover in the wrong place still beats
        // no popover at all.
        eprintln!("antiburn: could not anchor the popover ({error})");
    }

    let _ = window.show();
    let _ = window.set_focus();
    note_shown(app);
}

/// Hides the popover after it loses focus, remembering when it happened.
pub fn hide_on_focus_loss(window: &Window) {
    if let Some(state) = window.app_handle().try_state::<PopoverState>() {
        state.record_auto_hide();
    }
    let _ = window.hide();
    note_hidden(window.app_handle());
}

/// Tell the scan scheduler the popover is on screen.
///
/// The popover *is* the view of the scanned data, so its visibility is what
/// gates the periodic rescan (see [`crate::scan`]). Reported from here rather
/// than inferred from window events, because a hidden window that was never
/// shown produces no event at all.
fn note_shown(app: &AppHandle) {
    if let Some(controller) = app.try_state::<crate::scan::ScanController>() {
        controller.set_popover_visible(true);
        // Opening the popover is the one moment a reader is guaranteed to be
        // looking, so refresh immediately instead of waiting out a tick.
        controller.request();
    }
}

/// Tell the scan scheduler the popover is gone.
pub fn note_hidden(app: &AppHandle) {
    if let Some(controller) = app.try_state::<crate::scan::ScanController>() {
        controller.set_popover_visible(false);
    }
}

/// Places the popover under (or above) the menu-bar item, clamped to the
/// display the item lives on.
fn anchor_to(window: &WebviewWindow, anchor: Rect) -> tauri::Result<()> {
    // Tray backends report physical coordinates on macOS and Windows; the
    // conversion only matters where they report logical ones.
    let scale = window.scale_factor()?;
    let anchor_position = anchor.position.to_physical::<f64>(scale);
    let anchor_size = anchor.size.to_physical::<f64>(scale);
    let window_size = window.outer_size()?;
    let width = f64::from(window_size.width);
    let height = f64::from(window_size.height);

    let anchor_center_x = anchor_position.x + anchor_size.width / 2.0;
    let mut x = anchor_center_x - width / 2.0;
    let mut y = anchor_position.y + anchor_size.height + ANCHOR_GAP;

    if let Some(monitor) = window
        .app_handle()
        .monitor_from_point(anchor_center_x, anchor_position.y)?
    {
        let origin = monitor.position();
        let size = monitor.size();
        let left = f64::from(origin.x) + SCREEN_MARGIN;
        let top = f64::from(origin.y) + SCREEN_MARGIN;
        let right = f64::from(origin.x) + f64::from(size.width) - width - SCREEN_MARGIN;
        let bottom = f64::from(origin.y) + f64::from(size.height) - height - SCREEN_MARGIN;

        // Where the menu bar sits at the bottom of the screen — Windows, and
        // Linux panels — flip the popover above its anchor instead.
        if y > bottom {
            y = anchor_position.y - height - ANCHOR_GAP;
        }

        x = clamp(x, left, right);
        y = clamp(y, top, bottom);
    }

    window.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32))
}

/// `f64::clamp` panics when `max < min`, which happens on displays narrower
/// than the popover. Prefer the low edge there.
fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if max < min {
        return min;
    }
    value.clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_prefers_the_low_edge_on_undersized_displays() {
        assert_eq!(clamp(500.0, 8.0, -20.0), 8.0);
    }

    #[test]
    fn clamp_bounds_the_value_normally() {
        assert_eq!(clamp(-5.0, 0.0, 100.0), 0.0);
        assert_eq!(clamp(150.0, 0.0, 100.0), 100.0);
        assert_eq!(clamp(42.0, 0.0, 100.0), 42.0);
    }

    #[test]
    fn a_fresh_state_does_not_suppress_reopening() {
        let state = PopoverState::default();
        assert!(!state.suppresses_reopen());
    }

    #[test]
    fn an_automatic_dismissal_suppresses_exactly_one_reopen() {
        let state = PopoverState::default();
        state.record_auto_hide();
        assert!(state.suppresses_reopen());
        assert!(!state.suppresses_reopen());
    }
}
