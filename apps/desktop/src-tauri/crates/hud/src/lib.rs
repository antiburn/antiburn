// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The native window mechanism for the floating usage HUD.
//!
//! The crate creates, positions, reuses, and shows the transparent macOS
//! window. It also tracks the drawn hover region and reports cursor edges to
//! the webview, and it owns the hover detail window — a tooltip-style second
//! window that shows the expanded usage stats next to the HUD. The desktop
//! shell owns IPC policy and session discovery.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(target_os = "macos")]
use std::sync::atomic::AtomicBool;

use tauri::AppHandle;
#[cfg(target_os = "macos")]
use tauri::{Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder};

/// The floating HUD window label.
pub const OVERLAY_LABEL: &str = "antiburn-overlay";

/// The hover detail window label. Also listed in `capabilities/default.json`.
pub const DETAIL_LABEL: &str = "antiburn-hud-detail";

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

/* -------------------------------------------------------------------------
 * Hover detail window
 * ---------------------------------------------------------------------- */

/// Event that carries the newest detail payload to the detail webview.
#[cfg(target_os = "macos")]
const DETAIL_STATE_EVENT: &str = "hud-detail:state";

/// Detail window width in logical pixels. Matches the HUD frame width.
#[cfg(any(target_os = "macos", test))]
const DETAIL_WIDTH: f64 = 176.0;

/// Gap between the panel and the detail window frame. Zero: the webview
/// carries a transparent pad for its shadow, and that pad is the visible gap.
#[cfg(any(target_os = "macos", test))]
const DETAIL_GAP: f64 = 0.0;

/// Minimum logical gap between the detail window and the display edge.
#[cfg(any(target_os = "macos", test))]
const DETAIL_SCREEN_MARGIN: f64 = 8.0;

/// Shortest the detail window ever gets, and the height it is created at.
#[cfg(any(target_os = "macos", test))]
const DETAIL_MIN_HEIGHT: f64 = 40.0;

/// Tallest the detail window may ever get. Bounds a webview measurement bug.
#[cfg(any(target_os = "macos", test))]
const DETAIL_MAX_HEIGHT: f64 = 600.0;

/// True between a show request and the matching hide request.
///
/// The webview reports its measured height after each show request, and the
/// window only reaches the screen in that report. This flag keeps a late
/// report from showing a window whose hover already ended.
#[cfg(target_os = "macos")]
static DETAIL_SHOULD_SHOW: AtomicBool = AtomicBool::new(false);

/// The newest detail payload, kept for a detail webview that mounts late.
///
/// The first show request creates the window, so the webview subscribes after
/// the show-time event has already fired. The mount fetch reads this instead.
static DETAIL_STATE: Mutex<Option<serde_json::Value>> = Mutex::new(None);

/// Return the newest detail payload, or `Null` before the first show.
pub fn detail_state() -> serde_json::Value {
    DETAIL_STATE
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or(serde_json::Value::Null)
}

/// Store the newest detail payload and request the detail window.
///
/// The window is created hidden on the first request and stays warm after a
/// hide. It reaches the screen when the webview reports its measured height
/// through [`apply_detail_size`].
#[cfg(target_os = "macos")]
pub fn show_detail(app: &AppHandle, state: serde_json::Value) {
    if let Ok(mut slot) = DETAIL_STATE.lock() {
        *slot = Some(state.clone());
    }
    DETAIL_SHOULD_SHOW.store(true, Ordering::Relaxed);
    if app.get_webview_window(DETAIL_LABEL).is_none() && build_detail(app).is_err() {
        return;
    }
    let _ = app.emit_to(DETAIL_LABEL, DETAIL_STATE_EVENT, state);
}

/// Keep the detail window unavailable where the HUD itself is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn show_detail(_app: &AppHandle, _state: serde_json::Value) {}

/// Size the detail window, place it against the drawn panel, and show it.
///
/// `height` is the webview's measured content height in logical pixels. The
/// window appears at its final size, so it never resizes on screen.
#[cfg(target_os = "macos")]
pub fn apply_detail_size(app: &AppHandle, height: f64) {
    let height = clamp_detail_height(height);
    let Some(detail) = app.get_webview_window(DETAIL_LABEL) else {
        return;
    };
    let Some(hud) = app.get_webview_window(OVERLAY_LABEL) else {
        return;
    };
    if detail
        .set_size(LogicalSize::new(DETAIL_WIDTH, height))
        .is_err()
    {
        return;
    }
    let Some(anchor) = panel_anchor(&hud) else {
        return;
    };
    let frame = monitor_frame(&hud);
    let (x, y) = compute_detail_position(&anchor, frame.as_ref(), DETAIL_WIDTH, height);
    let _ = detail.set_position(LogicalPosition::new(x, y));
    if DETAIL_SHOULD_SHOW.load(Ordering::Relaxed) {
        let _ = detail.show();
    }
}

#[cfg(not(target_os = "macos"))]
pub fn apply_detail_size(_app: &AppHandle, _height: f64) {}

/// Hide the detail window and cancel any pending show.
#[cfg(target_os = "macos")]
pub fn hide_detail(app: &AppHandle) {
    DETAIL_SHOULD_SHOW.store(false, Ordering::Relaxed);
    if let Some(window) = app.get_webview_window(DETAIL_LABEL) {
        let _ = window.hide();
    }
}

#[cfg(not(target_os = "macos"))]
pub fn hide_detail(_app: &AppHandle) {}

/// Create the detail window hidden. Display only: the cursor never interacts
/// with it, so every click passes through to whatever sits below.
#[cfg(target_os = "macos")]
fn build_detail(app: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    let window = WebviewWindowBuilder::new(
        app,
        DETAIL_LABEL,
        WebviewUrl::App("index.html#/hud-detail".into()),
    )
    .title("antiburn")
    .inner_size(DETAIL_WIDTH, DETAIL_MIN_HEIGHT)
    .resizable(false)
    .visible(false)
    .focused(false)
    .shadow(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .decorations(false)
    .transparent(true)
    .build()?;
    let _ = window.set_ignore_cursor_events(true);
    Ok(window)
}

/// The drawn HUD panel in logical screen coordinates.
#[cfg(any(target_os = "macos", test))]
struct PanelAnchor {
    x: f64,
    top: f64,
    bottom: f64,
}

/// Read the drawn panel rect from the HUD window and its reported edges.
///
/// The hover-region edges are logical offsets from the window top. Before the
/// first report the whole window stands in for the panel.
#[cfg(target_os = "macos")]
fn panel_anchor(hud: &tauri::WebviewWindow) -> Option<PanelAnchor> {
    let position = hud.outer_position().ok()?;
    let size = hud.outer_size().ok()?;
    let scale = hud.scale_factor().unwrap_or(1.0);
    let x = position.x as f64 / scale;
    let window_top = position.y as f64 / scale;
    let window_bottom = window_top + size.height as f64 / scale;
    let mut top = window_top;
    let mut bottom = window_bottom;
    let reported_bottom = HOVER_BOTTOM_LOGICAL.load(Ordering::Relaxed);
    if reported_bottom > 0 {
        top = window_top + HOVER_TOP_LOGICAL.load(Ordering::Relaxed) as f64;
        bottom = window_bottom.min(window_top + reported_bottom as f64);
    }
    Some(PanelAnchor { x, top, bottom })
}

/// The display frame in logical coordinates.
#[cfg(any(target_os = "macos", test))]
struct DetailFrame {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

/// The frame of the display the HUD window is on.
#[cfg(target_os = "macos")]
fn monitor_frame(hud: &tauri::WebviewWindow) -> Option<DetailFrame> {
    let monitor = hud.current_monitor().ok().flatten()?;
    let scale = monitor.scale_factor();
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let left = monitor.position().x as f64 / scale;
    let top = monitor.position().y as f64 / scale;
    Some(DetailFrame {
        left,
        top,
        right: left + monitor.size().width as f64 / scale,
        bottom: top + monitor.size().height as f64 / scale,
    })
}

/// Where the detail window belongs, in logical coordinates. Pure, so the flip
/// and clamp behavior is testable without a window.
///
/// The window prefers the space below the panel, left-aligned with the HUD
/// frame. It flips above the panel when it cannot fit below, and the display
/// edges clamp it either way.
#[cfg(any(target_os = "macos", test))]
fn compute_detail_position(
    anchor: &PanelAnchor,
    frame: Option<&DetailFrame>,
    width: f64,
    height: f64,
) -> (f64, f64) {
    let mut x = anchor.x;
    let mut y = anchor.bottom + DETAIL_GAP;
    if let Some(frame) = frame {
        if y + height > frame.bottom - DETAIL_SCREEN_MARGIN {
            y = anchor.top - DETAIL_GAP - height;
        }
        x = clamp_detail(
            x,
            frame.left + DETAIL_SCREEN_MARGIN,
            frame.right - width - DETAIL_SCREEN_MARGIN,
        );
        y = clamp_detail(
            y,
            frame.top + DETAIL_SCREEN_MARGIN,
            frame.bottom - height - DETAIL_SCREEN_MARGIN,
        );
    }
    (x, y)
}

/// A requested detail height, held inside the bounds the window can be.
#[cfg(any(target_os = "macos", test))]
fn clamp_detail_height(height: f64) -> f64 {
    if height.is_nan() {
        return DETAIL_MIN_HEIGHT;
    }
    height.clamp(DETAIL_MIN_HEIGHT, DETAIL_MAX_HEIGHT)
}

/// `f64::clamp` panics when `max < min`, which happens on displays narrower
/// than the detail window. Prefer the low edge there.
#[cfg(any(target_os = "macos", test))]
fn clamp_detail(value: f64, min: f64, max: f64) -> f64 {
    if max < min {
        return min;
    }
    value.clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_edges_round_and_stay_in_the_native_range() {
        assert_eq!(clamp_edge(-2.0), 0);
        assert_eq!(clamp_edge(10.4), 10);
        assert_eq!(clamp_edge(10.6), 11);
        assert_eq!(clamp_edge(f64::MAX), u32::MAX);
    }

    fn frame() -> DetailFrame {
        DetailFrame {
            left: 0.0,
            top: 0.0,
            right: 1512.0,
            bottom: 982.0,
        }
    }

    #[test]
    fn the_detail_window_sits_below_the_panel_when_it_fits() {
        let anchor = PanelAnchor {
            x: 600.0,
            top: 40.0,
            bottom: 90.0,
        };
        let (x, y) = compute_detail_position(&anchor, Some(&frame()), DETAIL_WIDTH, 200.0);
        assert_eq!(x, 600.0);
        assert_eq!(y, 90.0 + DETAIL_GAP);
    }

    #[test]
    fn a_low_panel_flips_the_detail_window_above_itself() {
        let anchor = PanelAnchor {
            x: 600.0,
            top: 900.0,
            bottom: 950.0,
        };
        let (_, y) = compute_detail_position(&anchor, Some(&frame()), DETAIL_WIDTH, 200.0);
        assert_eq!(y, 900.0 - DETAIL_GAP - 200.0);
    }

    #[test]
    fn a_corner_panel_is_clamped_inside_the_display_margin() {
        let anchor = PanelAnchor {
            x: 1500.0,
            top: 40.0,
            bottom: 90.0,
        };
        let (x, _) = compute_detail_position(&anchor, Some(&frame()), DETAIL_WIDTH, 200.0);
        assert_eq!(x, 1512.0 - DETAIL_WIDTH - DETAIL_SCREEN_MARGIN);
    }

    #[test]
    fn no_frame_leaves_the_below_position_unclamped() {
        let anchor = PanelAnchor {
            x: 10.0,
            top: 40.0,
            bottom: 90.0,
        };
        let (x, y) = compute_detail_position(&anchor, None, DETAIL_WIDTH, 200.0);
        assert_eq!((x, y), (10.0, 90.0 + DETAIL_GAP));
    }

    #[test]
    fn a_webview_can_only_ask_for_a_height_the_window_can_be() {
        assert_eq!(clamp_detail_height(2_000.0), DETAIL_MAX_HEIGHT);
        assert_eq!(clamp_detail_height(1.0), DETAIL_MIN_HEIGHT);
        assert_eq!(clamp_detail_height(f64::NAN), DETAIL_MIN_HEIGHT);
        assert_eq!(clamp_detail_height(200.0), 200.0);
    }

    #[test]
    fn before_the_first_show_the_detail_state_is_null() {
        assert_eq!(detail_state(), serde_json::Value::Null);
    }
}
