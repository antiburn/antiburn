// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The notification window mechanism: build it on demand, position it at the
//! OS-native corner, show it **without stealing focus**, and retire it after
//! dismissal. This module knows
//! nothing about *what* a nudge means — that is the app's policy.
//!
//! The notification loads the app's own frontend bundle (same SPA as the
//! popover) at the `#/nudge` fragment; the frontend selects the notification
//! view from that hash (`src/lib/route.ts`). The contract is the label
//! [`crate::NUDGE_LABEL`], the fragment, and the `nudge:show` event.
//!
//! Types use the default `Wry` runtime — this is an in-repo crate for the
//! antiburn app, not a general-purpose plugin, so there's no need to be generic
//! over the runtime.

use tauri::{
    AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use crate::NudgePlacement;
use crate::geometry::{self, Rect};

/// Initial notification footprint in logical pixels. The webview can grow itself
/// to fit content via the Tauri window API after rendering; this is the seed
/// size. 344pt wide matches a native macOS notification banner.
const NUDGE_WIDTH: f64 = 344.0;
const NUDGE_HEIGHT: f64 = 168.0;

/// Inset from the screen edges.
const NUDGE_MARGIN: f64 = 12.0;

/// Standard macOS menu bar height, in points.
#[cfg(target_os = "macos")]
const MACOS_MENU_BAR_HEIGHT: f64 = 24.0;

/// Breathing room between the bottom of the menu bar and the top of the
/// notification — more than the corner margin used on the other edges, since
/// flush-under-the-menu-bar read as too tight.
#[cfg(target_os = "macos")]
const NUDGE_MENU_BAR_GAP: f64 = NUDGE_MARGIN * 2.0;

/// The same gap, halved, for the menu-bar anchor: the notification reads as
/// hanging off the tray item it points at, so it sits tighter under the menu bar
/// than the free-floating corner placement does.
#[cfg(target_os = "macos")]
const NUDGE_ANCHOR_MENU_BAR_GAP: f64 = NUDGE_MENU_BAR_GAP / 2.0;

/// Extra top inset on macOS so the notification clears the menu bar plus the gap.
#[cfg(target_os = "macos")]
const NUDGE_TOP_INSET: f64 = MACOS_MENU_BAR_HEIGHT + NUDGE_MENU_BAR_GAP;

/// Return the notification window, building it (hidden) on first call.
pub(crate) fn get_or_create_nudge_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window(crate::NUDGE_LABEL) {
        return Ok(window);
    }
    match build_nudge_window(app, crate::NUDGE_LABEL) {
        Ok(window) => Ok(window),
        // Concurrent policy events can both observe `None` above and race into
        // `build`; return whichever build won instead of dropping a nudge.
        Err(err) => app.get_webview_window(crate::NUDGE_LABEL).ok_or(err),
    }
}

fn build_nudge_window(app: &AppHandle, label: &str) -> tauri::Result<WebviewWindow> {
    // The fragment, not the default URL: this app's frontend selects its view
    // from the URL hash (`src/lib/route.ts`), unlike the source app, which
    // branched on the window label. Without it the hidden window renders the
    // popover view, which never reports a content height — and a nudge whose
    // height never arrives is never revealed.
    let mut builder =
        WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html#/nudge".into()))
            .title("antiburn")
            .inner_size(NUDGE_WIDTH, NUDGE_HEIGHT)
            .resizable(false)
            .visible(false)
            .focused(false) // never take key focus
            .shadow(true)
            .skip_taskbar(true)
            .always_on_top(true);

    #[cfg(target_os = "macos")]
    {
        // Undecorated with a real window shadow, and first-click-through so a CTA
        // responds to the click that would otherwise only have activated the
        // window. The card paints its surface and its 16pt corner itself, so
        // this window needs no vibrancy material. The popover takes the other
        // route and lets a material draw its corner; see `popover.rs`.
        builder = builder
            .decorations(false)
            .shadow(true)
            .accept_first_mouse(true);
    }

    #[cfg(target_os = "linux")]
    {
        // The resizable flag is for the toolkit, not for the reader. GTK pins
        // the geometry hints of a non-resizable window to its natural size, and
        // then it refuses a later resize. The notification must match the height
        // that the webview measures, so the flag stays on and overrides the
        // shared `.resizable(false)` above. The window has no decorations, so it
        // shows no resize handle and the reader cannot drag it.
        builder = builder
            .decorations(false)
            .transparent(false)
            .resizable(true);
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        builder = builder.decorations(false).transparent(false);
    }

    let window = builder.build()?;

    // macOS: subclass into a non-activating floating NSPanel so the notification
    // never steals key focus. No-op (and untouched window) on other platforms.
    #[cfg(target_os = "macos")]
    crate::macos::to_nonactivating_panel(&window);

    Ok(window)
}

/// Position + size the notification at the requested placement. Shared by
/// [`reveal`] (first show) and the non-macOS [`resize`] path; it does not show
/// or focus the window.
fn place(window: &WebviewWindow, content_height: f64, placement: NudgePlacement) {
    match placement {
        #[cfg(target_os = "macos")]
        NudgePlacement::MenuBarAnchor { rect } => {
            if place_at_menu_bar_anchor(window, content_height, rect) {
                return;
            }
            place_native_corner(window, content_height);
        }
        _ => place_native_corner(window, content_height),
    }
}

/// Position + size the notification at the OS-native corner of the **active**
/// display. It does not show or focus the window.
fn place_native_corner(window: &WebviewWindow, content_height: f64) {
    let Some(display) = active_display(window) else {
        return;
    };

    let w = NUDGE_WIDTH;
    let h = content_height.max(1.0);

    // macOS / Linux: top-right under the menu bar. Windows: bottom-right above
    // the taskbar, matching each platform's native notification corner.
    #[cfg(target_os = "windows")]
    let (x, y) = geometry::bottom_right_origin(display, w, h, NUDGE_MARGIN);
    #[cfg(target_os = "macos")]
    let (x, y) = geometry::top_right_origin(display, w, NUDGE_MARGIN, NUDGE_TOP_INSET);
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let (x, y) = geometry::top_right_origin(display, w, NUDGE_MARGIN, NUDGE_MARGIN);

    set_frame(window, w, h, x, y);
}

#[cfg(target_os = "macos")]
fn place_at_menu_bar_anchor(
    window: &WebviewWindow,
    content_height: f64,
    tray_rect: tauri::Rect,
) -> bool {
    let tray_phys: PhysicalPosition<f64> = tray_rect.position.to_physical(1.0);
    let tray_size_phys: tauri::PhysicalSize<f64> = tray_rect.size.to_physical(1.0);
    let Some(monitor) = monitor_containing_physical_point(window, tray_phys) else {
        return false;
    };
    let scale = monitor.scale_factor();
    let tray_x = tray_phys.x / scale;
    let tray_y = tray_phys.y / scale;
    let tray_w = tray_size_phys.width / scale;
    let tray_h = tray_size_phys.height / scale;

    let w = NUDGE_WIDTH;
    let h = content_height.max(1.0);
    let x = tray_x + (tray_w / 2.0) - (w / 2.0);
    // The tray rect spans the full menu bar, so its bottom edge is the menu bar's
    // bottom edge — the same baseline `place_native_corner` insets from.
    let y = tray_y + tray_h + NUDGE_ANCHOR_MENU_BAR_GAP;

    set_frame(window, w, h, x, y);
    true
}

fn set_frame(window: &WebviewWindow, w: f64, h: f64, x: f64, y: f64) {
    // Linux applies its frame in `linux.rs`: the GTK toolkit needs the main
    // thread, and it needs different resize handling.
    #[cfg(target_os = "linux")]
    crate::linux::apply_frame(window, w, h, x, y);

    #[cfg(not(target_os = "linux"))]
    {
        use tauri::{LogicalPosition, LogicalSize};

        // The window is built non-resizable (the OS clamps `set_size`); flip it
        // resizable around the resize. It has no decorations, so no handle appears.
        let _ = window.set_resizable(true);
        let _ = window.set_size(LogicalSize::new(w, h));
        let _ = window.set_resizable(false);
        let _ = window.set_position(LogicalPosition::new(x, y));
    }

    // macOS: hand the same rect to the cursor-based hover watcher. `x`/`y` are
    // already in the global logical point space it hit-tests against, so it
    // never has to ask AppKit (i.e. the main thread) where the window is.
    #[cfg(target_os = "macos")]
    crate::macos::record_frame(Rect { x, y, w, h });
}

/// The logical bounds of the display to place the notification on: the one
/// showing the frontmost application's window. See
/// [`crate::macos::active_display`] for why neither `NSScreen.main` nor the
/// cursor alone can answer this for a background app.
#[cfg(target_os = "macos")]
fn active_display(_window: &WebviewWindow) -> Option<Rect> {
    crate::macos::active_display()
}

/// Elsewhere, the monitor under the cursor.
///
/// The mixed-DPI hazard described in [`crate::geometry`] does **not** apply on
/// Windows: tao reports true Win32 physical monitor rects (`rcMonitor`) and
/// `GetCursorPos` in that same space, so the hit-test below is sound. It *does*
/// apply on Linux, where tao builds each monitor's rect by scaling its logical
/// geometry by that monitor's own factor — the very construction that makes the
/// rects overlap on macOS — so a fractional-scaling multi-monitor Linux session
/// can still resolve the wrong monitor here.
#[cfg(not(target_os = "macos"))]
fn active_display(window: &WebviewWindow) -> Option<Rect> {
    let monitor = monitor_under_cursor(window)
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten())?;

    // The *work area*, not the full monitor: it excludes the Windows taskbar and
    // Linux panels/docks, so the notification is inset from the usable region
    // rather than tucked underneath them. (macOS clears the menu bar via
    // `NUDGE_TOP_INSET` instead.) The cursor hit-test above still uses the full
    // monitor rect, since the cursor can legitimately sit over a taskbar.
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    Some(Rect {
        x: area.position.x as f64 / scale,
        y: area.position.y as f64 / scale,
        w: area.size.width as f64 / scale,
        h: area.size.height as f64 / scale,
    })
}

/// Size the notification to `content_height`, place it at the requested
/// location, then reveal it — sized and positioned *before* it's shown, so it
/// never resizes on screen. Deliberately does **not** `set_focus()` — a
/// notification must not activate the app; on macOS the panel is ordered front
/// without becoming key.
pub(crate) fn reveal(window: &WebviewWindow, content_height: f64, placement: NudgePlacement) {
    place(window, content_height, placement);

    // Platform-specific no-activate show: `.focused(false)` on the builder (see
    // `build_nudge_window`) only governs the *initial* build, not a later
    // `show()` — each platform needs its own glue to keep revealing the
    // notification from activating the window or pulling key focus from the
    // user's frontmost app.
    #[cfg(target_os = "macos")]
    {
        crate::macos::show(window);
        // Hover has to be sampled from the cursor rather than left to the
        // webview's own mouse events, which stop arriving as soon as another
        // antiburn window is key. See `macos::start_hover_watch`.
        crate::macos::start_hover_watch(window);
    }
    #[cfg(windows)]
    crate::win::show(window);
    #[cfg(target_os = "linux")]
    crate::linux::show(window);
    // No non-activating glue exists for any other target (the app doesn't ship
    // for one); fall back to a plain show rather than never revealing the
    // notification at all.
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    let _ = window.show();
}

/// Resize the **already-visible** notification to `content_height` (e.g. when it
/// expands on hover). On macOS the native window frame animates with the
/// system's default resize ease, anchored at its top edge so it grows/shrinks
/// downward like a real notification. Elsewhere it snaps to the new size at the
/// platform's corner.
pub(crate) fn resize(window: &WebviewWindow, content_height: f64) {
    #[cfg(target_os = "macos")]
    crate::macos::animate_resize(window, content_height);
    #[cfg(not(target_os = "macos"))]
    place(window, content_height, NudgePlacement::NativeCorner);
}

/// Hide the notification. On macOS this orders the panel out; elsewhere a plain
/// hide.
pub(crate) fn hide(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        crate::macos::stop_hover_watch();
        crate::macos::hide(window);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window.hide();
}

/// Retire the hidden notification webview. The next delivery recreates it.
pub(crate) fn destroy(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        crate::macos::stop_hover_watch();
        // `tauri-nspanel` mutates the native window class in place. Restore it
        // and release the plugin's retained handle before Tauri removes the
        // WKWebView, or AppKit terminates the process while unregistering
        // WebKit's window-visibility observer.
        if !crate::macos::prepare_for_destroy(window) {
            return;
        }
    }
    let _ = window.destroy();
}

/// The monitor currently containing the cursor, if it can be determined. Cursor
/// and monitor bounds are both in physical pixels, so they compare directly.
#[cfg(not(target_os = "macos"))]
fn monitor_under_cursor(window: &WebviewWindow) -> Option<tauri::Monitor> {
    let cursor = window.cursor_position().ok()?;
    monitor_containing_physical_point(window, PhysicalPosition::new(cursor.x, cursor.y))
}

fn monitor_containing_physical_point(
    window: &WebviewWindow,
    point: PhysicalPosition<f64>,
) -> Option<tauri::Monitor> {
    window.available_monitors().ok()?.into_iter().find(|m| {
        let pos = m.position();
        let size = m.size();
        point.x >= pos.x as f64
            && point.x < pos.x as f64 + size.width as f64
            && point.y >= pos.y as f64
            && point.y < pos.y as f64 + size.height as f64
    })
}
