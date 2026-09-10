//! macOS: subclass the popover window into a non-activating `NSPanel` via
//! `tauri-nspanel`.
//!
//! A plain `set_focus` activates the application. macOS then deactivates the
//! frontmost application, which dims its title bar and traffic lights. Native
//! menu-bar extras do not do this. The `NonactivatingPanel` style mask splits
//! the two states: the panel becomes the key window and receives keyboard
//! input, while the previous application stays active.
//!
//! Window operations must run on the main thread. Each public function here
//! marshals its work through Tauri's main-thread callbacks, except
//! [`prepare_for_destroy`], which its callers already run there.

use tauri::{Manager, WebviewWindow};
use tauri_nspanel::objc2_app_kit::{NSResponder, NSWindowCollectionBehavior, NSWindowStyleMask};
use tauri_nspanel::objc2_foundation::NSThread;
use tauri_nspanel::{ManagerExt, WebviewPanelManager, WebviewWindowExt};

tauri_nspanel::tauri_panel! {
    panel!(PopoverPanel {
        config: {
            can_become_key_window: true,
            can_become_main_window: false
        }
    })
}

/// Convert the popover window into a non-activating panel. Requires the
/// nspanel plugin (registered via `antiburn_nudge::register`); a no-op if it
/// isn't present. Safe to call from any thread — the work is marshaled onto
/// the main thread.
///
/// The window keeps the level that `always_on_top` set. It joins every Space,
/// including a full-screen Space owned by another application.
pub(super) fn to_nonactivating_panel(window: &WebviewWindow) {
    if window
        .try_state::<WebviewPanelManager<tauri::Wry>>()
        .is_none()
    {
        return;
    }
    let window = window.clone();
    let _ = window.clone().run_on_main_thread(move || {
        let Ok(panel) = window.to_panel::<PopoverPanel>() else {
            return;
        };
        // Borderless + never activate the application on key.
        panel.set_style_mask(NSWindowStyleMask::NonactivatingPanel);
        panel.set_collection_behavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
    });
}

/// Give the popover key-window status without activating the application.
///
/// Orders the panel front and makes it key, then gives its webview keyboard
/// focus. The direct webview handle avoids the wrapper and its material views.
///
/// Returns `false` when the nspanel plugin is not registered or the closure
/// cannot be marshaled; the caller falls back to a plain `set_focus`. Inside
/// the closure an unconverted window (a cold-start race with
/// [`to_nonactivating_panel`]) gets the same fallback.
pub(super) fn focus_without_activation(window: &WebviewWindow) -> bool {
    if window
        .try_state::<WebviewPanelManager<tauri::Wry>>()
        .is_none()
    {
        return false;
    }
    let window = window.clone();
    window
        .clone()
        .with_webview(
            move |webview| match window.get_webview_panel(super::LABEL) {
                Ok(panel) => {
                    panel.order_front_regardless();
                    panel.make_key_window();
                    // SAFETY: The handle is the live WKWebView, and this callback runs on the main thread.
                    let responder = unsafe { &*webview.inner().cast::<NSResponder>() };
                    if !panel.make_first_responder(Some(responder)) {
                        tracing::warn!("failed to give the popover webview keyboard focus");
                    }
                }
                Err(_) => {
                    let _ = window.set_focus();
                }
            },
        )
        .is_ok()
}

/// Convert the popover panel back to its original window class and remove the
/// retained panel handle before Tauri destroys the webview. The pinned
/// nspanel revision owns the class restoration in `Panel::to_window`. Skipping
/// this makes AppKit terminate the process while it unregisters WebKit's
/// window-visibility observer. Returns `false` if a registered panel cannot
/// be converted safely.
pub(super) fn prepare_for_destroy(window: &WebviewWindow) -> bool {
    debug_assert!(
        NSThread::isMainThread_class(),
        "popover::panel::prepare_for_destroy called off the main thread — AppKit calls are undefined behavior there"
    );
    match window.get_webview_panel(super::LABEL) {
        Ok(panel) => panel.to_window().is_some(),
        Err(_) => true,
    }
}
