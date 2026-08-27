//! Linux-only glue for the notification window: apply its frame, and order it
//! front without taking input focus.
//!
//! `set_focus_on_map(false)` on the underlying GTK window suppresses the input
//! focus grab GTK would otherwise perform when the window is mapped (shown),
//! so revealing the notification doesn't pull key focus from the user's
//! frontmost app. Must be set before the window is shown — GTK reads the flag
//! at map time, so setting it after `.show()` would be too late for that show.
//!
//! GTK widget/window calls are only safe from the GTK main-loop thread, but
//! `reveal()` (this function's caller) runs on Tauri's IPC command thread —
//! not the main thread — so every GTK touch here is marshaled through
//! `run_on_main_thread`, the same discipline `macos.rs` documents and follows
//! for its AppKit calls.

use gtk::prelude::*;
use tauri::{LogicalPosition, LogicalSize, WebviewWindow};

/// Size and position the notification window.
///
/// The window keeps its resizable flag for the whole of its life (see
/// `window::build_nudge_window`). The other platforms turn that flag off again
/// directly after the resize, but on GTK that clamps the window: GTK reads the
/// geometry hints from the natural size of the window, and it can apply them
/// before it allocates the new size. The window then keeps the size it had, and
/// the page background paints the strip that the notification card does not
/// cover.
///
/// The work runs on the GTK main-loop thread for the reason the module doc
/// gives. It also makes the calls take effect inline, so the size and the
/// position are correct before the show that comes after them.
pub(crate) fn apply_frame(window: &WebviewWindow, w: f64, h: f64, x: f64, y: f64) {
    let window = window.clone();
    let _ = window.clone().run_on_main_thread(move || {
        let _ = window.set_size(LogicalSize::new(w, h));
        let _ = window.set_position(LogicalPosition::new(x, y));
    });
}

pub(crate) fn show(window: &WebviewWindow) {
    let window = window.clone();
    let _ = window.clone().run_on_main_thread(move || {
        // If the GTK window handle can't be resolved, set_focus_on_map(false)
        // can't be applied — showing anyway would fall back to a plain, focus-
        // stealing show, exactly what this function exists to avoid. Mirrors
        // win.rs's same-shaped guard on `hwnd()`.
        let Ok(gtk_window) = window.gtk_window() else {
            return;
        };
        gtk_window.set_focus_on_map(false);
        let _ = window.show();
    });
}
