// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! antiburn overlay: a small transparent always-on-top window showing the
//! usage bars, parked under the menu bar. Prototype-simple by design (approved
//! in the plan review): a normal window you can hover, drag, and
//! close — no click-through, no cursor-event games. The webview selects
//! `<OverlayWindow/>` from the `#/overlay` fragment (`src/lib/route.ts`).
//!
//! Create-or-show: the ✕ in the overlay hides the window (via the window API
//! from JS), and the pop-out button next to the popover usage bars calls
//! [`open`] to surface it again.
//!
//! v1 ships on macOS only (`docs/deviations.md`, D-28): every behaviour in
//! `docs/hud-states.md` was tuned against a top menu bar, and the platform
//! fallback — an opaque decorated slab — is worse than nothing. The gate
//! lives in this module and nowhere else, so a later platform enable is one
//! function here plus un-hiding the toggle.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tauri::AppHandle;
#[cfg(target_os = "macos")]
use tauri::{Emitter, LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder};

pub const OVERLAY_LABEL: &str = "antiburn-overlay";

/// Top and bottom edges of the overlay's *drawn* panel, in logical px from
/// the window top, reported by the webview (`set_overlay_hover_region`)
/// whenever the panel moves or resizes. The frame is far taller than the
/// panel and transparent everywhere else, so without this the hover watcher
/// would trigger over empty space. Bottom 0 = not yet reported; fall back to
/// the full frame.
static HOVER_TOP_LOGICAL: AtomicU32 = AtomicU32::new(0);
static HOVER_BOTTOM_LOGICAL: AtomicU32 = AtomicU32::new(0);

pub fn set_hover_region(top_logical: f64, bottom_logical: f64) {
    let clamp = |value: f64| value.clamp(0.0, u32::MAX as f64).round() as u32;
    HOVER_TOP_LOGICAL.store(clamp(top_logical), Ordering::Relaxed);
    HOVER_BOTTOM_LOGICAL.store(clamp(bottom_logical), Ordering::Relaxed);
}

/// Fixed frame, with room for the panel to expand in either direction. The
/// webview draws the panel at the frame top most of the time; parked too low
/// to open downward it insets the panel and moves the window up to match, so
/// the empty space above becomes room to expand into. The height must fit the
/// expanded panel at its tallest (several providers × limit windows) plus
/// that inset; if the panel outgrows the frame, the OS clips it flat.
#[cfg(target_os = "macos")]
const OVERLAY_WIDTH: f64 = 176.0;
#[cfg(target_os = "macos")]
const OVERLAY_HEIGHT: f64 = 500.0;

/// Standard macOS menu bar height plus a little air, so the overlay hangs
/// under the menu bar rather than colliding with it.
#[cfg(target_os = "macos")]
const OVERLAY_TOP_INSET: f64 = 24.0 + 8.0;

/// Open (or re-show) the overlay, centered horizontally under the menu bar of
/// the primary display. Dragging afterwards is the user's business — a re-open
/// of a still-live window keeps wherever they dropped it.
#[cfg(target_os = "macos")]
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        window.show()?;
        return Ok(());
    }

    // The fragment, not the default URL: this app's frontend selects its view
    // from the URL hash (`src/lib/route.ts`), unlike the source app, which
    // branched on the window label.
    let builder = WebviewWindowBuilder::new(
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
    .transparent(true);

    let window = builder.build()?;
    spawn_hover_watcher(window.clone());

    if let Ok(Some(monitor)) = window.primary_monitor() {
        let scale = monitor.scale_factor();
        let mx = monitor.position().x as f64 / scale;
        let my = monitor.position().y as f64 / scale;
        let mw = monitor.size().width as f64 / scale;
        let x = mx + (mw - OVERLAY_WIDTH) / 2.0;
        let y = my + OVERLAY_TOP_INSET;
        window.set_position(LogicalPosition::new(x, y))?;
    }

    window.show()?;
    Ok(())
}

/// The D-28 gate: on every other platform the overlay quietly does not open.
/// The settings toggle and the pop-out button are hidden there too, so this
/// path is only reachable by calling the command by hand.
#[cfg(not(target_os = "macos"))]
pub fn open(_app: &AppHandle) -> tauri::Result<()> {
    Ok(())
}

/// Poll the global cursor against the overlay's frame and tell the webview
/// when the pointer enters or leaves. The webview's own mouseenter only fires
/// while the app is active — macOS doesn't deliver hover to background apps —
/// but the overlay must expand on hover even when another app has focus, so
/// hover detection lives out here instead. One watcher per built window; it
/// exits when the window is destroyed and stays quiet while hidden.
#[cfg(target_os = "macos")]
fn spawn_hover_watcher(window: tauri::WebviewWindow) {
    tauri::async_runtime::spawn(async move {
        let mut inside_last = false;
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
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
    let (x, y) = (position.x as f64, position.y as f64);
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

/// Epoch seconds of the most recent transcript write across all locally
/// discovered sessions, or `None` when nothing wrote within the engine's
/// active-session window. The overlay polls this every 5s while visible to
/// drive the live-session LED blink from real file mtimes.
///
/// The source app served this from a discovery cache with a 60s TTL, kept
/// warm by a background tick. This shell has no equivalent — its scan
/// scheduler is deliberately paused while the popover is hidden, which is
/// exactly when the overlay is the only surface watching — so the overlay
/// asks the engine directly and keeps its own 60s memo here. That preserves
/// both the cost ceiling (at most one discovery walk per minute, however
/// often the webview polls) and the staleness contract the blink window was
/// tuned against (the webview's 90s liveness window clears a ≤60s-stale
/// reading with margin).
pub async fn latest_session_activity() -> Option<i64> {
    use std::sync::Mutex;
    use std::time::Instant;

    static MEMO: Mutex<Option<(Instant, Option<i64>)>> = Mutex::new(None);
    const MEMO_TTL: Duration = Duration::from_secs(60);

    if let Ok(memo) = MEMO.lock()
        && let Some((at, value)) = *memo
        && at.elapsed() < MEMO_TTL
    {
        return value;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let logs = antiburn_local::discovery::Explorers::DISK
        .discover_recent_sessions(now, antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS)
        .await;
    let latest = logs.iter().filter_map(|log| log.updated_at).max();

    if let Ok(mut memo) = MEMO.lock() {
        *memo = Some((Instant::now(), latest));
    }
    latest
}
