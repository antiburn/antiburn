// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Windows-only: order the notification window front without activating it.
//! Named `win`, not `windows`, so this module doesn't shadow the `windows`
//! crate it depends on at the crate root.
//!
//! `WebviewWindowBuilder::focused(false)` (see `window::build_nudge_window`)
//! only governs the *initial* build — a later plain `.show()` call can still
//! activate the window and pull key focus from the user's frontmost app.
//! `ShowWindow(HWND, SW_SHOWNOACTIVATE)` shows the window without changing
//! which window is active/foreground, closing that gap.
//!
//! `reveal()` (this function's caller) runs on Tauri's IPC command thread, not
//! the main thread, so this is marshaled through `run_on_main_thread` — the
//! same discipline `macos.rs` documents and follows for its window calls.

use tauri::WebviewWindow;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};

pub(crate) fn show(window: &WebviewWindow) {
    let window = window.clone();
    let _ = window.clone().run_on_main_thread(move || {
        let Ok(hwnd) = window.hwnd() else {
            // No live HWND (window already closed, or a handle error) — nothing
            // to show. Falling back to `window.show()` here would risk exactly
            // the focus-steal this function exists to avoid; the only realistic
            // trigger is the app tearing down mid-reveal, where a missed
            // notification is inconsequential.
            return;
        };
        // SAFETY: on the main thread; `hwnd` is the live HWND backing the
        // notification window. `ShowWindow` takes a window handle and a show
        // command and has no other preconditions.
        unsafe {
            // Tauri currently exposes its HWND through windows 0.61, while
            // this crate calls ShowWindow through windows 0.62. Both wrappers
            // contain the same raw Win32 handle, but Rust correctly treats
            // types from the two crate versions as distinct.
            let _ = ShowWindow(HWND(hwnd.0), SW_SHOWNOACTIVATE);
        }
    });
}
