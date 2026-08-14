// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Linux-only: order the notification window front without taking input focus.
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
use tauri::WebviewWindow;

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
