//! The debug-only session window.
//!
//! An ordinary resizable window with the activity list down the left and the
//! selected session's detail filling the rest (`src/views/SessionWindowView.tsx`).
//! It exists so the detail can be seen and tuned at any width while the app
//! runs. The tray opens it in debug builds; `lib.rs` compiles the module out
//! of release builds, so nothing here can reach a reader.
//!
//! No readiness dance: the window is built hidden only long enough to centre
//! it, then shown. Closing it destroys the webview, and the next request
//! builds a new one.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::window_placement::center_on_active_monitor;

/// Window label. Also listed in `capabilities/default.json`.
pub const LABEL: &str = "session-window";

/// The resident shell entry, on the fragment `src/lib/route.ts` maps to the view.
const URL: &str = "index.html#/session-window";

const TITLE: &str = "antiburn Session (dev)";

// Wide enough for the list and a detail column that reads as a desktop pane,
// and still inside a 1280×800 display. The minimum keeps the detail column
// wider than the popover, which is the point of the window.
const WIDTH: f64 = 1180.0;
const HEIGHT: f64 = 820.0;
const MIN_WIDTH: f64 = 760.0;
const MIN_HEIGHT: f64 = 600.0;

/// Show the session window. Create it if this is the first request.
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    if let Some(existing) = app.get_webview_window(LABEL) {
        existing.show()?;
        existing.unminimize()?;
        existing.set_focus()?;
        return Ok(());
    }

    // Built hidden and positioned before the first show, so the window never
    // visibly jumps from a default position to the centred one.
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(URL.into()))
        .title(TITLE)
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .resizable(true)
        .maximizable(true)
        .visible(false)
        .build()?;
    center_on_active_monitor(&window, WIDTH, HEIGHT);
    window.show()?;
    window.set_focus()?;
    ::tracing::info!(event = "window_revealed", window = LABEL);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_uses_the_resident_shell_on_the_session_window_route() {
        assert!(URL.starts_with("index.html#/"));
        assert!(URL.ends_with(LABEL));
    }
}
