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
use tauri_nspanel::objc2_app_kit::{
    NSFloatingWindowLevel, NSResponder, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use tauri_nspanel::objc2_foundation::NSThread;
use tauri_nspanel::{ManagerExt, PanelHandle, WebviewPanelManager, WebviewWindowExt};

tauri_nspanel::tauri_panel! {
    panel!(PopoverPanel {
        config: {
            can_become_key_window: true,
            can_become_main_window: false
        }
    })
}

/// Resolve and configure the panel on the main thread before presentation.
fn configured_panel(window: &WebviewWindow) -> tauri::Result<PanelHandle<tauri::Wry>> {
    debug_assert!(NSThread::isMainThread_class());
    if window
        .try_state::<WebviewPanelManager<tauri::Wry>>()
        .is_none()
    {
        return Err(tauri::Error::Anyhow(anyhow::anyhow!(
            "popover panel plugin is unavailable"
        )));
    }
    let panel = window
        .get_webview_panel(super::LABEL)
        .or_else(|_| window.to_panel::<PopoverPanel>())?;
    panel.set_style_mask(NSWindowStyleMask::NonactivatingPanel);
    // Keep the interactive panel below status-level nudges and system menus.
    panel.set_level(NSFloatingWindowLevel as i64);
    panel.set_collection_behavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    Ok(panel)
}

/// Prepare the hidden panel without requesting keyboard focus.
pub(super) fn to_nonactivating_panel(window: &WebviewWindow) {
    let window = window.clone();
    let _ = window.clone().run_on_main_thread(move || {
        if let Err(error) = configured_panel(&window) {
            tracing::warn!(%error, "failed to configure the popover panel");
        }
    });
}

/// Reveal only a configured panel, then report the native presentation result.
pub(super) fn show_without_activation(
    window: &WebviewWindow,
    generation: u64,
    on_shown: impl FnOnce(tauri::Result<()>) + Send + 'static,
) -> tauri::Result<()> {
    let window = window.clone();
    window.clone().run_on_main_thread(move || {
        if !window
            .app_handle()
            .try_state::<super::PopoverState>()
            .is_some_and(|state| state.owns_renderer(generation))
        {
            return;
        }
        on_shown(configured_panel(&window).map(|panel| panel.order_front_regardless()));
    })
}

/// Give the popover key-window status without activating the application.
///
/// Orders the panel front and makes it key, then gives its webview keyboard
/// focus. The direct webview handle avoids the wrapper and its material views.
///
/// Configuration failures leave the window unchanged instead of activating the application.
pub(super) fn focus_without_activation(window: &WebviewWindow) -> tauri::Result<()> {
    let window = window.clone();
    window
        .clone()
        .with_webview(move |webview| {
            // A dismissal can occur while the focus request waits for the main thread.
            if !window.is_visible().unwrap_or(false) {
                return;
            }
            match configured_panel(&window) {
                Ok(panel) => {
                    panel.order_front_regardless();
                    panel.make_key_window();
                    // SAFETY: The handle is the live WKWebView, and this callback runs on the main thread.
                    let responder = unsafe { &*webview.inner().cast::<NSResponder>() };
                    if !panel.make_first_responder(Some(responder)) {
                        tracing::warn!("failed to give the popover webview keyboard focus");
                    }
                }
                Err(error) => {
                    tracing::error!(event = "window_focus_failed", %error, "failed to focus the popover panel");
                }
            }
        })
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
