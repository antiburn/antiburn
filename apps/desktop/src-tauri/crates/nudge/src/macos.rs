// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! macOS: subclass the notification window into a **non-activating floating
//! NSPanel** via `tauri-nspanel`, so showing it (and clicking its CTAs) never
//! steals key focus from the user's frontmost app or activates antiburn.
//!
//! Every *window* operation here must run on the main thread, so each is dispatched through
//! [`tauri::WebviewWindow::run_on_main_thread`]. The display-resolution helpers
//! at the bottom are the exception: Core Graphics' window/display queries,
//! `+[NSEvent mouseLocation]`, and `NSWorkspace.frontmostApplication` are all
//! thread-safe, and [`crate::NudgeManager::reveal`] calls them from the IPC
//! command thread.
//!
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use core_foundation::base::TCFType;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::display::CGDisplay;
use core_graphics::geometry::CGRect;
use core_graphics::window::{
    create_description_from_array, create_window_list, kCGNullWindowID, kCGWindowBounds,
    kCGWindowLayer, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
    kCGWindowOwnerPID,
};
use tauri::{Emitter, Manager, WebviewWindow};
use tauri_nspanel::objc2_app_kit::{NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace};
use tauri_nspanel::objc2_foundation::NSThread;
use tauri_nspanel::{ManagerExt, WebviewPanelManager, WebviewWindowExt};

use crate::geometry::{self, Point, Rect};

tauri_nspanel::tauri_panel! {
    panel!(NudgePanel {
        config: {
            can_become_key_window: true,
            can_become_main_window: false
        }
    })
}

/// `NSStatusWindowLevel` — float above normal and floating windows (still below
/// menus / pop-ups), so the notification reliably sits on top.
const STATUS_WINDOW_LEVEL: i64 = 25;

/// Convert the notification window into a non-activating floating panel and
/// apply the notification behaviors. Requires the nspanel plugin (registered via
/// [`crate::register`]); a no-op if it isn't present. Safe to call from any
/// thread — the work is marshaled onto the main thread.
pub(crate) fn to_nonactivating_panel(window: &WebviewWindow) {
    if window
        .try_state::<WebviewPanelManager<tauri::Wry>>()
        .is_none()
    {
        return;
    }
    let window = window.clone();
    let _ = window.clone().run_on_main_thread(move || {
        let Ok(panel) = window.to_panel::<NudgePanel>() else {
            return;
        };
        // Borderless + never activate the app on key.
        panel.set_style_mask(NSWindowStyleMask::NonactivatingPanel);
        // Float above other windows; show on every Space and above fullscreen apps.
        panel.set_level(STATUS_WINDOW_LEVEL);
        panel.set_collection_behavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
    });
}

/// Convert the notification panel back to its original window class and remove
/// the retained panel handle before Tauri destroys the webview. The pinned
/// nspanel revision owns the class restoration in `Panel::to_window`.
/// Returns `false` if a registered panel cannot be converted safely.
pub(crate) fn prepare_for_destroy(window: &WebviewWindow) -> bool {
    match window.get_webview_panel(crate::NUDGE_LABEL) {
        Ok(panel) => panel.to_window().is_some(),
        Err(_) => true,
    }
}

/// Show the notification by ordering its panel front, **without** taking key
/// window status. An unprompted nudge must never take key: even though the
/// `NonactivatingPanel` style mask (see [`to_nonactivating_panel`]) keeps a
/// `makeKeyWindow` call from activating antiburn or moving the frontmost *app*,
/// AppKit still lets a nonactivating panel hold real key-window status and
/// receive genuine keyboard input while some other app stays frontmost — so a
/// forced `makeKeyWindow` on every show could swallow keystrokes the user is
/// mid-typing into their editor or terminal. `tauri-nspanel` v2.0's
/// `RawNSPanel::show()` called `makeKeyWindow` unconditionally, so this calls
/// the lower-level `order_front_regardless()` instead — visually front, key
/// status untouched.
/// Key is acquired later, lazily, only once the user's mouse has genuinely
/// entered the notification (see [`set_hovered`]), by which point their
/// attention has already shifted there.
pub(crate) fn show(window: &WebviewWindow) {
    let window = window.clone();
    let _ = window.clone().run_on_main_thread(move || {
        match window.get_webview_panel(crate::NUDGE_LABEL) {
            Ok(panel) => panel.order_front_regardless(),
            Err(_) => {
                // The panel subclass isn't attached yet (cold-start race, since
                // `to_nonactivating_panel` converts asynchronously). Order the
                // underlying NSWindow front *without* activating antiburn — a plain
                // `window.show()` here would make the window key and steal focus
                // from the user's frontmost app, the very thing this crate avoids.
                if let Ok(ptr) = window.ns_window() {
                    // SAFETY: on the main thread; `ns_window` is the live NSWindow
                    // backing the notification. `orderFrontRegardless` takes no
                    // arguments and returns void.
                    unsafe {
                        (&*ptr.cast::<NSWindow>()).orderFrontRegardless();
                    }
                }
            }
        }
    });
}

/// Acquire or release key-window status for the notification, driven by the
/// frontend's own hover state (`onMouseEnter`/`onMouseLeave` in `NudgeView.tsx`).
/// Continuous `mouseMoved` delivery for the hover-expand CSS requires the window
/// to be key — AppKit only forwards `mouseMoved` to the key window — but by the
/// time the user's cursor has actually entered the notification, their attention
/// (and any in-progress typing) has already shifted there, so acquiring key at
/// that point carries none of the risk a forced acquire on an unprompted [`show`]
/// would. Resigns key on mouse-leave so the notification goes back to being a
/// purely passive, non-key panel the moment the user looks away, ready for the
/// next unprompted show. A no-op if the panel isn't registered yet (mirrors
/// [`show`]'s cold-start fallback — a nudge shown via the raw
/// `orderFrontRegardless` fallback is never hovered before its own next
/// `show` re-resolves the panel).
///
/// On acquire, also makes the content view first responder — mirroring what
/// `RawNSPanel::show()` used to do unconditionally. `makeKeyWindow` alone
/// doesn't hand keyboard events to the embedded WKWebView; without an
/// explicit first responder, the window itself absorbs them, and CTA/close
/// buttons stop responding to Tab/Enter/Space while hovered even though mouse
/// clicks (hit-testing, independent of first-responder state) keep working.
pub(crate) fn set_hovered(window: &WebviewWindow, hovered: bool) {
    let window = window.clone();
    let _ = window.clone().run_on_main_thread(move || {
        if let Ok(panel) = window.get_webview_panel(crate::NUDGE_LABEL) {
            if hovered {
                let content_view = panel.content_view();
                panel.make_first_responder(Some(&content_view));
                panel.make_key_window();
            } else if is_key(&window) {
                // Only resign key we actually hold. `resignKeyWindow` is
                // documented as "never invoke this method directly" — it tells a
                // window it *has* lost key rather than handing key anywhere — so
                // firing it at a panel that was never key would reset its first
                // responder and post a spurious `DidResignKey` for no reason.
                // Reachable now that hover can be reported by the cursor watcher
                // below without the panel ever having taken key.
                panel.resign_key_window();
            }
        }
    });
}

/// How often [`start_hover_watch`] samples the cursor. 60 ms is fast enough that
/// hover reads as immediate (the notification's expand animation is longer than
/// one tick) and cheap enough to be irrelevant: one thread-safe AppKit point
/// read plus a rect comparison, and only while a notification is on screen.
const HOVER_POLL_INTERVAL: Duration = Duration::from_millis(60);

/// The frame the notification was last placed at, in Core Graphics' global point
/// space (top-left origin) — the same space [`cursor_point`] reports, so the two
/// hit-test directly.
///
/// Recorded by the placement code rather than read back from the `NSWindow`,
/// because reading the live frame means an AppKit call, which means the main
/// thread, which is exactly what the watcher thread must not block on 16 times a
/// second.
static NOTIFICATION_FRAME: Mutex<Option<Rect>> = Mutex::new(None);

/// Bumped by every [`start_hover_watch`] / [`stop_hover_watch`]. A watcher thread
/// exits as soon as it sees a generation other than its own, so at most one is
/// ever live and a rapid hide→reveal can't leave two racing threads behind.
static HOVER_WATCH_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Record where the notification was just placed, for the hover watcher's
/// hit-test. Called from `window::set_frame`.
pub(crate) fn record_frame(frame: Rect) {
    if let Ok(mut cached) = NOTIFICATION_FRAME.lock() {
        *cached = Some(frame);
    }
}

/// Record a hover-expand/collapse resize. [`animate_resize`] holds the top edge
/// fixed, so only the height changes.
fn record_height(height: f64) {
    if let Ok(mut cached) = NOTIFICATION_FRAME.lock()
        && let Some(frame) = cached.as_mut()
    {
        frame.h = height.max(1.0);
    }
}

fn notification_frame() -> Option<Rect> {
    NOTIFICATION_FRAME.lock().ok().and_then(|frame| *frame)
}

/// Watch the cursor while the notification is on screen and report crossings of
/// its frame to the notification webview as [`crate::NUDGE_HOVER_EVENT`].
///
/// **Why this exists.** The webview's own `mouseenter`/`mouseleave` are not
/// reliable here: AppKit routes mouse-*moved* events to the key window, and the
/// notification deliberately never takes key on show (see [`show`]). While
/// antiburn has no ordinary window that is the app's key window, the panel still
/// sees the cursor and the DOM handlers fire — but the moment another antiburn
/// window is key (the Settings window; equally the popover, if a nudge arrives
/// while it is open) those events go there instead and the notification stops
/// seeing hover entirely: no expand, no CTAs, and no pause of the auto-dismiss
/// timer. Sampling the cursor position sidesteps event routing altogether, so
/// hover works in every activation state.
///
/// Deliberately does **not** take key window status: this signal exists so hover
/// works *without* it. `+[NSEvent mouseLocation]` is thread-safe (see this
/// module's header), so the sampling never touches the main thread; only the
/// emit does, and that goes through Tauri's own event plumbing.
pub(crate) fn start_hover_watch(window: &WebviewWindow) {
    let generation = HOVER_WATCH_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let app = window.app_handle().clone();
    std::thread::spawn(move || {
        // `None` until the first sample, so the watcher always publishes its
        // opening state. That resync matters: a notification hidden while the
        // cursor rested on it leaves the webview believing it is still hovered
        // (expanded, auto-dismiss paused), and the next nudge may well appear
        // nowhere near the cursor.
        let mut hovered: Option<bool> = None;
        while HOVER_WATCH_GENERATION.load(Ordering::SeqCst) == generation {
            let inside = notification_frame().is_some_and(|frame| frame.contains(cursor_point()));
            if hovered != Some(inside) {
                hovered = Some(inside);
                let _ = app.emit_to(crate::NUDGE_LABEL, crate::NUDGE_HOVER_EVENT, inside);
            }
            std::thread::sleep(HOVER_POLL_INTERVAL);
        }
    });
}

/// Stop the cursor watcher. Called when the notification is hidden; the thread
/// notices within one [`HOVER_POLL_INTERVAL`] and exits.
pub(crate) fn stop_hover_watch() {
    HOVER_WATCH_GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Whether the notification's underlying `NSWindow` currently holds
/// key-window status. Used to tell "the notification just took key focus
/// from the popover" (via [`set_hovered`]'s `makeKeyWindow` call) apart from a
/// real focus loss, when the app decides whether to hide the popover on blur.
///
/// A live query, not a flag set around the `makeKeyWindow` call: tao delivers
/// `WindowEvent::Focused` through a queued event pipeline
/// (`AppState::queue_event` in `tao`'s macOS window delegate), not
/// synchronously with the AppKit call that triggers it. A flag toggled true
/// then immediately false around that call would already be back to `false`
/// by the time the app's queued blur handler runs. Reading `isKeyWindow` at
/// the point the handler actually executes has no such staleness — by then
/// the key-window handoff this call started has fully settled one way or the
/// other.
///
/// Deliberately **not** dispatched through `run_on_main_thread` like the
/// other calls in this file: every real caller (a `WindowEvent::Focused`
/// handler) already runs on the main thread — AppKit only ever delivers
/// window-delegate callbacks there — and unlike `show`/`hide`/
/// `animate_resize`, this function must return its result synchronously to
/// that same caller. `run_on_main_thread` is fire-and-forget (it can't hand a
/// value back) and queues the closure to run on a *later* turn of the main
/// run loop; calling it from the main thread while synchronously waiting on
/// the result would deadlock the very thread that needs to process the
/// queue. The `debug_assert!` below converts a future misuse (calling this
/// off the main thread, which is undefined behavior for any AppKit call, not
/// just this one) into a clear panic in debug builds instead of silent UB.
pub(crate) fn is_key(window: &WebviewWindow) -> bool {
    let Ok(ptr) = window.ns_window() else {
        return false;
    };
    unsafe {
        debug_assert!(
            NSThread::isMainThread_class(),
            "antiburn_nudge::macos::is_key called off the main thread — AppKit calls are undefined behavior there"
        );
        // SAFETY: on the main thread (asserted above); `ns_window` is the live
        // NSWindow backing the notification. `isKeyWindow` takes no arguments
        // and returns BOOL.
        (&*ptr.cast::<NSWindow>()).isKeyWindow()
    }
}

/// Animate the notification window's frame to `height` using AppKit's
/// `animator` proxy, which applies the system's default window-resize timing
/// (ease-in-ease-out) — no explicit curve needed. The top edge (`maxY`) is held
/// fixed so the notification grows/shrinks downward, matching the top-anchored
/// corner used on reveal. All AppKit calls run on the main thread.
pub(crate) fn animate_resize(window: &WebviewWindow, height: f64) {
    // Keep the hover watcher's hit-test in step with the expanded/collapsed
    // frame, or the cursor would read as "outside" the moment it sat in the
    // region hover itself revealed.
    record_height(height);
    let window = window.clone();
    let _ = window.clone().run_on_main_thread(move || {
        let Ok(ptr) = window.ns_window() else {
            return;
        };
        let new_h = height.max(1.0);
        // SAFETY: on the main thread; `ns_window` is the live NSWindow backing the
        // notification. Reading `frame` and messaging the `animator` proxy are
        // standard AppKit calls with fixed, well-typed signatures.
        unsafe {
            let ns_window = &*ptr.cast::<NSWindow>();
            let frame = ns_window.frame();
            // AppKit is y-up: `origin.y + height` is the top edge — keep it fixed.
            let max_y = frame.origin.y + frame.size.height;
            let new_frame = NSRect::new(
                NSPoint::new(frame.origin.x, max_y - new_h),
                NSSize::new(frame.size.width, new_h),
            );
            let animator: Retained<NSWindow> = msg_send![ns_window, animator];
            animator.setFrame_display(new_frame, true);
        }
    });
}

/// Hide the notification panel (order out).
pub(crate) fn hide(window: &WebviewWindow) {
    let window = window.clone();
    let _ = window.clone().run_on_main_thread(move || {
        match window.get_webview_panel(crate::NUDGE_LABEL) {
            Ok(panel) => panel.hide(),
            Err(_) => {
                let _ = window.hide();
            }
        }
    });
}

/// The display the user is working on, in Core Graphics' global display space
/// (points, top-left origin at the main display) — which is also exactly what
/// Tauri's `LogicalPosition` means, so the result can be handed to
/// `set_position` untouched.
///
/// Resolved by [`geometry::resolve_display`]: the frontmost application's front
/// window, else the cursor, else the main display — and, failing all three, the
/// first display we know of, since `reveal` shows the notification whether or not
/// placement succeeded. Everything below is a thin shim onto an OS call; the
/// decision itself lives in `geometry`, where it is unit-tested.
///
/// Deliberately *not* `NSScreen.main` (the obvious API): it reports the calling
/// process's key-window screen, and this app's windows are non-activating panels
/// that never take key, so it collapses to the menu-bar display for this app.
/// Deliberately *not* the cursor alone: it's common to type on one monitor while
/// the mouse rests on another.
pub(crate) fn active_display() -> Option<Rect> {
    let (displays, main_index) = cg_displays();
    let resolved = geometry::resolve_display(
        &displays,
        frontmost_app_window_center(),
        Some(cursor_point()),
        main_index,
    )
    .and_then(|(i, source)| displays.get(i).map(|rect| (*rect, source)));

    if let Some((rect, source)) = resolved {
        tracing::debug!(target: "nudge", ?source, ?rect, "resolved the active display");
        return Some(rect);
    }

    // Fall back to *any* known display rather than giving up. `reveal` shows the
    // notification whether or not `place` succeeded, so returning `None` here
    // would reveal it unsized and unpositioned — at the previous nudge's frame.
    // Resolution can come back empty if Core Graphics fails or the main display
    // is missing from the active list (a transient during a display hotplug or a
    // sleep/wake), which is exactly when a poll-driven nudge may fire.
    match displays.first().copied() {
        Some(rect) => {
            tracing::warn!(
                target: "nudge",
                display_count = displays.len(),
                ?rect,
                "could not resolve the active display; falling back to the first one",
            );
            Some(rect)
        }
        None => {
            tracing::warn!(target: "nudge", "no active displays; leaving the notification unplaced");
            None
        }
    }
}

/// Every active display as a logical rect, plus the index of the main display.
fn cg_displays() -> (Vec<Rect>, Option<usize>) {
    let Ok(ids) = CGDisplay::active_displays() else {
        return (Vec::new(), None);
    };
    let main_id = CGDisplay::main().id;
    let main_index = ids.iter().position(|&id| id == main_id);
    let displays = ids
        .into_iter()
        .map(|id| {
            let b = CGDisplay::new(id).bounds();
            Rect {
                x: b.origin.x,
                y: b.origin.y,
                w: b.size.width,
                h: b.size.height,
            }
        })
        .collect();
    (displays, main_index)
}

/// Center of the front window belonging to the application macOS reports as
/// frontmost. `None` when antiburn itself is frontmost (a tray click or a
/// dev-sim), which tells us nothing about where the user is working, or when
/// that application has no eligible window on screen.
fn frontmost_app_window_center() -> Option<Point> {
    let pid = frontmost_app_pid()?;
    if pid == std::process::id() as i64 {
        return None;
    }
    front_window_center_for_pid(pid)
}

/// PID of `NSWorkspace.frontmostApplication`.
fn frontmost_app_pid() -> Option<i64> {
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    let pid = app.processIdentifier();
    (pid > 0).then_some(pid as i64)
}

/// The cursor, in global screen space (top-left origin, points). Always
/// available — a cursor that lands on no display is handled by the caller's
/// next tier, not by an absent value here.
///
/// Read straight from AppKit rather than through `WebviewWindow::cursor_position`,
/// which scales the point by the *primary* monitor's factor while tao's monitor
/// rects are scaled by each monitor's own factor — on a mixed-DPI setup those are
/// different coordinate spaces and the hit-test silently fails.
fn cursor_point() -> Point {
    let main_height = CGDisplay::main().bounds().size.height;
    // `+[NSEvent mouseLocation]` is a thread-safe class method returning an
    // `NSPoint` in AppKit's bottom-left-origin global screen space.
    let location = NSEvent::mouseLocation();
    Point {
        x: location.x,
        y: main_height - location.y,
    }
}

/// The center point (global screen space, top-left origin, points) of `pid`'s
/// front on-screen window at the normal window layer.
/// `CGWindowListCopyWindowInfo` orders windows front-to-back globally, so the
/// first entry owned by `pid` at layer 0 is that application's front window.
///
/// Scoping to `pid` matters: the first layer-0 window in the *global* order is
/// not necessarily the focused one — a background app can order a normal-layer
/// window above it — and taking that window would place the notification on the
/// wrong display.
fn front_window_center_for_pid(pid: i64) -> Option<Point> {
    let ids = create_window_list(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    )?;
    let infos = create_description_from_array(ids)?;

    let layer_key = unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) };
    let pid_key = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerPID) };
    let bounds_key = unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) };

    for info in infos.iter() {
        let number = |key: &CFString| {
            info.find(key)
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_i64())
        };

        // Layer 0 is the normal window layer: it excludes the menu bar, the Dock,
        // status items, and this app's own always-on-top panels.
        if number(&layer_key) != Some(0) || number(&pid_key) != Some(pid) {
            continue;
        }

        let Some(rect) = info
            .find(&bounds_key)
            .and_then(|v| v.downcast::<CFDictionary>())
            .and_then(|d| CGRect::from_dict_representation(&d))
        else {
            continue;
        };
        // Skip degenerate helper windows that report layer 0 but aren't real
        // content (menu extras, invisible accessory windows, ...).
        if rect.size.width < 40.0 || rect.size.height < 40.0 {
            continue;
        }

        return Some(Point {
            x: rect.origin.x + rect.size.width / 2.0,
            y: rect.origin.y + rect.size.height / 2.0,
        });
    }

    None
}
