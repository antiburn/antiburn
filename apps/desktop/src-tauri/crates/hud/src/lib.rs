// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The native window mechanism for the floating usage HUD.
//!
//! The crate creates, positions, sizes, reuses, and shows the transparent
//! macOS window. It also reports cursor edges to the webview and owns the hover
//! detail window. The desktop shell owns IPC policy and session discovery.

use std::sync::Mutex;
#[cfg(target_os = "macos")]
use std::sync::MutexGuard;
#[cfg(any(target_os = "macos", test))]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
use objc2_app_kit::NSWindow;
use tauri::AppHandle;
#[cfg(target_os = "macos")]
use tauri::{
    Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

/// The floating HUD window label.
pub const OVERLAY_LABEL: &str = "antiburn-overlay";

/// The hover detail window label. Also listed in `capabilities/default.json`.
pub const DETAIL_LABEL: &str = "antiburn-hud-detail";

const OVERLAY_SEED_HEIGHT: f64 = 1.0;
const OVERLAY_MAX_HEIGHT: f64 = 500.0;

#[cfg(target_os = "macos")]
const OVERLAY_WIDTH: f64 = 176.0;
#[cfg(target_os = "macos")]
const OVERLAY_TOP_INSET: f64 = 24.0 + 8.0;
#[cfg(target_os = "macos")]
const RESIZE_DURATION: Duration = Duration::from_millis(140);
#[cfg(target_os = "macos")]
const RESIZE_STEPS: u32 = 12;
#[cfg(target_os = "macos")]
const OVERLAY_VISIBILITY_EVENT: &str = "overlay_visibility_changed";

#[cfg(any(target_os = "macos", test))]
struct ResizeState {
    height_bits: AtomicU64,
    generation: AtomicU64,
    measured: AtomicBool,
    wanted_visible: AtomicBool,
}

#[cfg(any(target_os = "macos", test))]
impl ResizeState {
    const fn new(height: f64) -> Self {
        Self {
            height_bits: AtomicU64::new(height.to_bits()),
            generation: AtomicU64::new(0),
            measured: AtomicBool::new(false),
            wanted_visible: AtomicBool::new(true),
        }
    }

    fn reset(&self, height: f64) {
        self.height_bits.store(height.to_bits(), Ordering::SeqCst);
        self.measured.store(false, Ordering::SeqCst);
        self.wanted_visible.store(true, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn height(&self) -> f64 {
        f64::from_bits(self.height_bits.load(Ordering::SeqCst))
    }

    fn set_height(&self, height: f64) {
        self.height_bits.store(height.to_bits(), Ordering::SeqCst);
    }

    fn begin_resize(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn resize_is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }

    fn is_measured(&self) -> bool {
        self.measured.load(Ordering::SeqCst)
    }

    fn mark_measured(&self) {
        self.measured.store(true, Ordering::SeqCst);
    }

    fn request_open(&self) {
        self.wanted_visible.store(true, Ordering::SeqCst);
    }

    fn request_hide(&self) {
        self.wanted_visible.store(false, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn wants_visible(&self) -> bool {
        self.wanted_visible.load(Ordering::SeqCst)
    }
}

#[cfg(target_os = "macos")]
static RESIZE_STATE: ResizeState = ResizeState::new(OVERLAY_SEED_HEIGHT);
#[cfg(target_os = "macos")]
static RESIZE_APPLY_LOCK: Mutex<()> = Mutex::new(());

#[cfg(target_os = "macos")]
fn resize_apply_guard() -> MutexGuard<'static, ()> {
    RESIZE_APPLY_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(any(target_os = "macos", test))]
fn clamp_height(value: f64) -> f64 {
    if value.is_nan() {
        return OVERLAY_SEED_HEIGHT;
    }
    value.clamp(OVERLAY_SEED_HEIGHT, OVERLAY_MAX_HEIGHT)
}

#[cfg(any(target_os = "macos", test))]
fn anchored_y(current_y: f64, target_height: f64, bottom_edge: Option<f64>) -> f64 {
    bottom_edge.map_or(current_y, |edge| edge - target_height)
}

#[cfg(any(target_os = "macos", test))]
fn ease_out(progress: f64) -> f64 {
    let remaining = 1.0 - progress.clamp(0.0, 1.0);
    1.0 - remaining * remaining * remaining
}

#[cfg(any(target_os = "macos", test))]
fn contains_point(x: f64, y: f64, width: f64, height: f64, point_x: f64, point_y: f64) -> bool {
    point_x >= x && point_x < x + width && point_y >= y && point_y < y + height
}

/// Open or re-show the floating HUD.
#[cfg(target_os = "macos")]
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let measured = {
            let _guard = resize_apply_guard();
            RESIZE_STATE.request_open();
            RESIZE_STATE.is_measured()
        };
        if measured {
            show_without_activation(&window)?;
        }
        return Ok(());
    }

    {
        let _guard = resize_apply_guard();
        RESIZE_STATE.reset(OVERLAY_SEED_HEIGHT);
    }
    let window = WebviewWindowBuilder::new(
        app,
        OVERLAY_LABEL,
        WebviewUrl::App("index.html#/overlay".into()),
    )
    .title("antiburn")
    .inner_size(OVERLAY_WIDTH, OVERLAY_SEED_HEIGHT)
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

    // The renderer reveals the window after it reports the first content height.
    Ok(())
}

/// Keep the HUD unavailable on platforms whose behavior is not tuned.
#[cfg(not(target_os = "macos"))]
pub fn open(_app: &AppHandle) -> tauri::Result<()> {
    Ok(())
}

/// Hide the HUD and cancel a pending first reveal or animated resize.
#[cfg(target_os = "macos")]
pub fn hide(app: &AppHandle) -> tauri::Result<()> {
    let _guard = resize_apply_guard();
    RESIZE_STATE.request_hide();
    hide_detail(app);
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        window.hide()?;
    }
    let _ = app.emit(OVERLAY_VISIBILITY_EVENT, false);
    Ok(())
}

/// Keep HUD hiding unavailable on unsupported platforms.
#[cfg(not(target_os = "macos"))]
pub fn hide(_app: &AppHandle) -> tauri::Result<()> {
    Ok(())
}

/// Match the native frame to the rendered panel height.
///
/// A bottom anchor grows the frame upward. A top anchor grows it downward.
/// The first measurement reveals the hidden window when it is still requested.
#[cfg(target_os = "macos")]
pub fn resize(
    app: &AppHandle,
    requested_height: f64,
    anchor_bottom: bool,
    animate: bool,
) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(OVERLAY_LABEL) else {
        return Ok(());
    };

    let target = clamp_height(requested_height);
    let visible = window.is_visible().unwrap_or(false);
    let (from, generation, bottom_edge) = {
        let _guard = resize_apply_guard();
        let from = RESIZE_STATE.height();
        let generation = RESIZE_STATE.begin_resize();
        let first_measurement = !RESIZE_STATE.is_measured();
        let bottom_edge = bottom_anchor_edge(&window, from, anchor_bottom)?;

        if !animate || !visible || (from - target).abs() < 1.0 {
            if (from - target).abs() >= 1.0 {
                apply_height(&window, target, bottom_edge)?;
            }
            if first_measurement {
                RESIZE_STATE.mark_measured();
                if RESIZE_STATE.wants_visible() {
                    show_without_activation(&window)?;
                }
            }
            return Ok(());
        }

        (from, generation, bottom_edge)
    };

    tauri::async_runtime::spawn(async move {
        let step = RESIZE_DURATION / RESIZE_STEPS;
        for frame in 1..=RESIZE_STEPS {
            tokio::time::sleep(step).await;
            let _guard = resize_apply_guard();
            if !RESIZE_STATE.resize_is_current(generation) {
                return;
            }
            let progress = f64::from(frame) / f64::from(RESIZE_STEPS);
            let height = from + (target - from) * ease_out(progress);
            if let Err(error) = apply_height(&window, height, bottom_edge) {
                tracing::warn!(
                    error = %error,
                    target_height = target,
                    anchor_bottom,
                    "HUD resize frame failed; applying the final frame"
                );
                match apply_height(&window, target, bottom_edge) {
                    Ok(()) => {}
                    Err(recovery_error) => {
                        record_window_height(&window);
                        tracing::error!(
                            error = %recovery_error,
                            target_height = target,
                            anchor_bottom,
                            "HUD resize recovery failed"
                        );
                    }
                }
                return;
            }
        }
    });
    Ok(())
}

#[cfg(target_os = "macos")]
fn show_without_activation(window: &WebviewWindow) -> tauri::Result<()> {
    let native_window = window.clone();
    let app = window.app_handle().clone();
    window.run_on_main_thread(move || {
        if !RESIZE_STATE.wants_visible() {
            return;
        }
        if let Ok(pointer) = native_window.ns_window() {
            // SAFETY: The callback runs on the main thread and the pointer is the live NSWindow.
            unsafe {
                (&*pointer.cast::<NSWindow>()).orderFrontRegardless();
            }
            let _ = app.emit(OVERLAY_VISIBILITY_EVENT, true);
        }
    })
}

#[cfg(target_os = "macos")]
fn bottom_anchor_edge(
    window: &WebviewWindow,
    current_height: f64,
    anchor_bottom: bool,
) -> tauri::Result<Option<f64>> {
    if !anchor_bottom {
        return Ok(None);
    }
    let scale = window.scale_factor()?;
    let position = window.outer_position()?;
    Ok(Some(position.y as f64 / scale + current_height))
}

#[cfg(target_os = "macos")]
fn record_window_height(window: &WebviewWindow) {
    let Ok(scale) = window.scale_factor() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    RESIZE_STATE.set_height(size.height as f64 / scale);
}

/// Keep dynamic sizing unavailable on unsupported platforms.
#[cfg(not(target_os = "macos"))]
pub fn resize(
    _app: &AppHandle,
    _requested_height: f64,
    _anchor_bottom: bool,
    _animate: bool,
) -> tauri::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn apply_height(
    window: &WebviewWindow,
    target_height: f64,
    bottom_edge: Option<f64>,
) -> tauri::Result<()> {
    let position = match bottom_edge {
        Some(bottom_edge) => {
            let scale = window.scale_factor()?;
            let current = window.outer_position()?;
            Some(LogicalPosition::new(
                current.x as f64 / scale,
                anchored_y(current.y as f64 / scale, target_height, Some(bottom_edge)),
            ))
        }
        None => None,
    };

    window.set_resizable(true)?;
    let size_result = window.set_size(LogicalSize::new(OVERLAY_WIDTH, target_height));
    if size_result.is_ok() {
        RESIZE_STATE.set_height(target_height);
    } else {
        record_window_height(window);
    }
    let position_result = match position {
        Some(position) if size_result.is_ok() => window.set_position(position),
        _ => Ok(()),
    };
    let restore_result = window.set_resizable(false);
    reposition_detail_after_hud_frame(window);
    size_result?;
    position_result?;
    restore_result
}

#[cfg(target_os = "macos")]
fn spawn_hover_watcher(window: WebviewWindow) {
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
fn cursor_inside(window: &WebviewWindow) -> Option<bool> {
    let cursor = window.cursor_position().ok()?;
    let position = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    let x = position.x as f64;
    let y = position.y as f64;
    Some(contains_point(
        x,
        y,
        size.width as f64,
        size.height as f64,
        cursor.x,
        cursor.y,
    ))
}

/* -------------------------------------------------------------------------
 * Hover detail window
 * ---------------------------------------------------------------------- */

/// Event that carries the newest detail payload to the detail webview.
#[cfg(target_os = "macos")]
const DETAIL_STATE_EVENT: &str = "hud-detail:state";

/// Event that asks the detail webview to clear its card before a hide.
#[cfg(target_os = "macos")]
const DETAIL_CONCEAL_EVENT: &str = "hud-detail:conceal";

/// Longest wait for the webview's conceal report before a forced hide.
#[cfg(target_os = "macos")]
const DETAIL_CONCEAL_FALLBACK_MS: u64 = 80;

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
    if position_detail_window(&detail, &hud, height).is_none() {
        return;
    }
    if DETAIL_SHOULD_SHOW.load(Ordering::Relaxed) {
        let _ = detail.show();
    }
}

#[cfg(not(target_os = "macos"))]
pub fn apply_detail_size(_app: &AppHandle, _height: f64) {}

/// Cancel any pending show and start the hide of the detail window.
///
/// A hidden webview keeps its last frame, and macOS flashes that frame on the
/// next show. So the webview first clears its card while it can still paint,
/// then reports through [`conceal_detail`], and only then does the window
/// hide. A fallback hides the window anyway after a short wait.
#[cfg(target_os = "macos")]
pub fn hide_detail(app: &AppHandle) {
    DETAIL_SHOULD_SHOW.store(false, Ordering::Relaxed);
    let Some(window) = app.get_webview_window(DETAIL_LABEL) else {
        return;
    };
    if app.emit_to(DETAIL_LABEL, DETAIL_CONCEAL_EVENT, ()).is_err() {
        let _ = window.hide();
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(DETAIL_CONCEAL_FALLBACK_MS)).await;
        if !DETAIL_SHOULD_SHOW.load(Ordering::Relaxed) {
            let _ = window.hide();
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn hide_detail(_app: &AppHandle) {}

/// Hide the detail window after the webview cleared its card.
///
/// A new show request can land between the clear and this report. The
/// [`DETAIL_SHOULD_SHOW`] check keeps that fresh show on screen.
#[cfg(target_os = "macos")]
pub fn conceal_detail(app: &AppHandle) {
    if DETAIL_SHOULD_SHOW.load(Ordering::Relaxed) {
        return;
    }
    if let Some(window) = app.get_webview_window(DETAIL_LABEL) {
        let _ = window.hide();
    }
}

#[cfg(not(target_os = "macos"))]
pub fn conceal_detail(_app: &AppHandle) {}

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

/// Read the content-sized HUD frame in logical screen coordinates.
#[cfg(target_os = "macos")]
fn panel_anchor(hud: &tauri::WebviewWindow) -> Option<PanelAnchor> {
    let position = hud.outer_position().ok()?;
    let size = hud.outer_size().ok()?;
    let scale = hud.scale_factor().unwrap_or(1.0);
    let x = position.x as f64 / scale;
    let window_top = position.y as f64 / scale;
    Some(PanelAnchor {
        x,
        top: window_top,
        bottom: window_top + size.height as f64 / scale,
    })
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

/// Keep a visible detail window joined to each HUD animation frame.
#[cfg(target_os = "macos")]
fn reposition_detail_after_hud_frame(hud: &WebviewWindow) {
    if !DETAIL_SHOULD_SHOW.load(Ordering::Relaxed) {
        return;
    }
    let Some(detail) = hud.app_handle().get_webview_window(DETAIL_LABEL) else {
        return;
    };
    let Ok(size) = detail.outer_size() else {
        return;
    };
    let scale = detail.scale_factor().unwrap_or(1.0);
    let _ = position_detail_window(&detail, hud, size.height as f64 / scale);
}

#[cfg(target_os = "macos")]
fn position_detail_window(detail: &WebviewWindow, hud: &WebviewWindow, height: f64) -> Option<()> {
    let anchor = panel_anchor(hud)?;
    let frame = monitor_frame(hud);
    let (x, y) = compute_detail_position(&anchor, frame.as_ref(), DETAIL_WIDTH, height);
    detail.set_position(LogicalPosition::new(x, y)).ok()
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
    fn heights_stay_inside_the_supported_frame() {
        assert_eq!(clamp_height(f64::NAN), OVERLAY_SEED_HEIGHT);
        assert_eq!(clamp_height(-2.0), OVERLAY_SEED_HEIGHT);
        assert_eq!(clamp_height(120.0), 120.0);
        assert_eq!(clamp_height(900.0), OVERLAY_MAX_HEIGHT);
    }

    #[test]
    fn top_anchor_keeps_the_window_origin() {
        assert_eq!(anchored_y(40.0, 120.0, None), 40.0);
    }

    #[test]
    fn bottom_anchor_keeps_the_window_bottom() {
        assert_eq!(anchored_y(40.0, 120.0, Some(70.0)), -50.0);
        assert_eq!(anchored_y(-50.0, 30.0, Some(70.0)), 40.0);
    }

    #[test]
    fn a_new_resize_invalidates_the_previous_generation() {
        let state = ResizeState::new(30.0);
        let first = state.begin_resize();
        assert!(state.resize_is_current(first));
        let second = state.begin_resize();
        assert!(state.resize_is_current(second));
        assert!(!state.resize_is_current(first));
    }

    #[test]
    fn only_a_measured_window_is_ready_to_reopen() {
        let state = ResizeState::new(OVERLAY_SEED_HEIGHT);
        assert!(!state.is_measured());
        state.mark_measured();
        assert!(state.is_measured());
        state.reset(OVERLAY_SEED_HEIGHT);
        assert!(!state.is_measured());
    }

    #[test]
    fn hiding_cancels_a_pending_first_reveal() {
        let state = ResizeState::new(OVERLAY_SEED_HEIGHT);
        state.request_hide();
        state.mark_measured();
        assert!(!state.wants_visible());
        state.request_open();
        assert!(state.wants_visible());
    }

    #[test]
    fn hover_hit_testing_uses_half_open_frame_edges() {
        assert!(contains_point(100.0, 40.0, 176.0, 28.0, 100.0, 40.0));
        assert!(contains_point(100.0, 40.0, 176.0, 28.0, 275.9, 67.9));
        assert!(!contains_point(100.0, 40.0, 176.0, 28.0, 276.0, 50.0));
        assert!(!contains_point(100.0, 40.0, 176.0, 28.0, 120.0, 68.0));
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
    fn a_growing_hud_moves_the_detail_with_its_bottom_edge() {
        let before = PanelAnchor {
            x: 600.0,
            top: 40.0,
            bottom: 68.0,
        };
        let after = PanelAnchor {
            x: 600.0,
            top: 40.0,
            bottom: 84.0,
        };
        let (_, before_y) = compute_detail_position(&before, Some(&frame()), DETAIL_WIDTH, 200.0);
        let (_, after_y) = compute_detail_position(&after, Some(&frame()), DETAIL_WIDTH, 200.0);
        assert_eq!(after_y - before_y, 16.0);
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
