// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The native window mechanism for the floating usage HUD.
//!
//! The crate creates, positions, reuses, and shows the transparent macOS
//! window. It also tracks the drawn hover region and reports cursor edges to
//! the webview. The desktop shell owns IPC policy and session discovery.

use std::sync::atomic::{AtomicU32, Ordering};

use tauri::AppHandle;
#[cfg(target_os = "macos")]
use tauri::{Emitter, LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder};

/// The floating HUD window label.
pub const OVERLAY_LABEL: &str = "antiburn-overlay";

static HOVER_TOP_LOGICAL: AtomicU32 = AtomicU32::new(0);
static HOVER_BOTTOM_LOGICAL: AtomicU32 = AtomicU32::new(0);

/// Record the drawn panel edges in logical pixels from the window top.
pub fn set_hover_region(top_logical: f64, bottom_logical: f64) {
    HOVER_TOP_LOGICAL.store(clamp_edge(top_logical), Ordering::Relaxed);
    HOVER_BOTTOM_LOGICAL.store(clamp_edge(bottom_logical), Ordering::Relaxed);
}

fn clamp_edge(value: f64) -> u32 {
    value.clamp(0.0, u32::MAX as f64).round() as u32
}

#[cfg(target_os = "macos")]
const OVERLAY_WIDTH: f64 = 176.0;
#[cfg(target_os = "macos")]
const OVERLAY_HEIGHT: f64 = 500.0;
#[cfg(target_os = "macos")]
const OVERLAY_TOP_INSET: f64 = 24.0 + 8.0;

/// Open or re-show the floating HUD.
#[cfg(target_os = "macos")]
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        window.show()?;
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(
        app,
        OVERLAY_LABEL,
        WebviewUrl::App("index.html#/overlay".into()),
    )
    .title("antiburn")
    .inner_size(OVERLAY_WIDTH, OVERLAY_HEIGHT)
    .resizable(false)
    .visible(false)
    .focused(false)
    .focusable(false)
    .shadow(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .accept_first_mouse(true)
    .decorations(false)
    .transparent(true)
    .build()?;

    spawn_hover_watcher(window.clone());

    if let Ok(Some(monitor)) = window.primary_monitor() {
        let scale = monitor.scale_factor();
        let monitor_x = monitor.position().x as f64 / scale;
        let monitor_y = monitor.position().y as f64 / scale;
        let monitor_width = monitor.size().width as f64 / scale;
        let x = monitor_x + (monitor_width - OVERLAY_WIDTH) / 2.0;
        let y = monitor_y + OVERLAY_TOP_INSET;
        window.set_position(LogicalPosition::new(x, y))?;
    }

    window.show()?;
    Ok(())
}

/// Keep the HUD unavailable on platforms whose behavior is not tuned.
#[cfg(not(target_os = "macos"))]
pub fn open(_app: &AppHandle) -> tauri::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn spawn_hover_watcher(window: tauri::WebviewWindow) {
    tauri::async_runtime::spawn(async move {
        let mut inside_last = false;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if window
                .app_handle()
                .get_webview_window(OVERLAY_LABEL)
                .is_none()
            {
                break;
            }
            if !window.is_visible().unwrap_or(false) {
                continue;
            }
            let inside = cursor_inside(&window).unwrap_or(false);
            if inside != inside_last {
                inside_last = inside;
                let _ = window.emit_to(OVERLAY_LABEL, "overlay_hover", inside);
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn cursor_inside(window: &tauri::WebviewWindow) -> Option<bool> {
    let cursor = window.cursor_position().ok()?;
    let position = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    let x = position.x as f64;
    let y = position.y as f64;
    let mut top = y;
    let mut bottom = y + size.height as f64;
    let reported_bottom = HOVER_BOTTOM_LOGICAL.load(Ordering::Relaxed);
    if reported_bottom > 0 {
        let scale = window.scale_factor().unwrap_or(1.0);
        top = y + HOVER_TOP_LOGICAL.load(Ordering::Relaxed) as f64 * scale;
        bottom = bottom.min(y + reported_bottom as f64 * scale);
    }
    Some(cursor.x >= x && cursor.x < x + size.width as f64 && cursor.y >= top && cursor.y < bottom)
}

#[cfg(test)]
mod tests {
    use super::clamp_edge;

    #[test]
    fn hover_edges_round_and_stay_in_the_native_range() {
        assert_eq!(clamp_edge(-2.0), 0);
        assert_eq!(clamp_edge(10.4), 10);
        assert_eq!(clamp_edge(10.6), 11);
        assert_eq!(clamp_edge(f64::MAX), u32::MAX);
    }
}
