// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The native window mechanism for the floating usage HUD.
//!
//! The crate creates, positions, sizes, reuses, and shows the transparent
//! macOS window. It also reports cursor edges to the webview. The desktop
//! shell owns IPC policy and session discovery.

#[cfg(any(target_os = "macos", test))]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(target_os = "macos")]
use std::sync::{Mutex, MutexGuard};
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
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        window.hide()?;
    }
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
    window.run_on_main_thread(move || {
        if !RESIZE_STATE.wants_visible() {
            return;
        }
        if let Ok(pointer) = native_window.ns_window() {
            // SAFETY: The callback runs on the main thread and the pointer is the live NSWindow.
            unsafe {
                (&*pointer.cast::<NSWindow>()).orderFrontRegardless();
            }
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
}
