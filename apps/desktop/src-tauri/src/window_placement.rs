//! Where the app's ordinary windows go when they open.
//!
//! The popover has its own placement (it hangs off the menu-bar item, see
//! [`crate::popover`]) and the notification window has the nudge crate's. What
//! is left is the two fixed-size, non-resizable windows a reader opens
//! deliberately — [`crate::settings`] and [`crate::onboarding`] — and they want
//! the same answer to the same question, so it lives here once rather than
//! twice.

use tauri::WebviewWindow;

/// Center `window` on the monitor the cursor is on.
///
/// `width` and `height` are the window's **logical** size, passed in rather
/// than read back: a window built hidden has not necessarily been laid out yet,
/// and a window being re-shown reports a physical size scaled for whichever
/// display it was last on.
///
/// The cursor is the active-monitor signal because every path into these
/// windows follows a click — the tray menu, the popover's affordances, ⌘, —
/// so the pointer is on the display the reader is working on. The one path that
/// does not is the onboarding window at launch, where the cursor is still the
/// best guess available and the fallbacks below catch the rest.
///
/// Falls back through the window's current monitor to the primary one, and does
/// nothing if even that cannot be resolved: a window at its old position is
/// better than one at (0,0).
pub fn center_on_active_monitor(window: &WebviewWindow, width: f64, height: f64) {
    let monitor = window
        .cursor_position()
        .ok()
        .and_then(|cursor| {
            window.available_monitors().ok()?.into_iter().find(|m| {
                let pos = m.position();
                let size = m.size();
                cursor.x >= pos.x as f64
                    && cursor.x < pos.x as f64 + size.width as f64
                    && cursor.y >= pos.y as f64
                    && cursor.y < pos.y as f64 + size.height as f64
            })
        })
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };

    // Physical pixels throughout: the monitor's frame is physical, and the
    // window's fixed logical size scales by that monitor's own factor.
    let scale = monitor.scale_factor();
    let width = width * scale;
    let height = height * scale;
    let pos = monitor.position();
    let size = monitor.size();
    let x = pos.x as f64 + (size.width as f64 - width) / 2.0;
    let y = pos.y as f64 + (size.height as f64 - height) / 2.0;
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}
