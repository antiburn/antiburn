// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The standalone settings window.
//!
//! Unlike the popover this is an ordinary, decorated, resizable window: it is
//! a place to read and change configuration, not a transient surface. It is
//! created on first use and then reused, because a menu-bar app can be asked
//! to open settings many times per session.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// Window label. Also listed in `capabilities/default.json`.
pub const LABEL: &str = "settings";

/// Fragment the settings window loads. The frontend serves one bundle and
/// selects its view from this fragment (see `src/lib/route.ts`).
const URL: &str = "index.html#/settings";

const WIDTH: f64 = 620.0;
const HEIGHT: f64 = 460.0;
const MIN_WIDTH: f64 = 480.0;
const MIN_HEIGHT: f64 = 360.0;

/// Shows the settings window, creating it if this is the first request.
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    if let Some(existing) = app.get_webview_window(LABEL) {
        existing.show()?;
        existing.unminimize()?;
        return existing.set_focus();
    }

    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(URL.into()))
        .title("antiburn Settings")
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .resizable(true)
        .center()
        .focused(true)
        .build()?;

    Ok(())
}
