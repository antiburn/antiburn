use std::{ptr::NonNull, sync::Mutex};

use block2::RcBlock;
use dispatch2::MainThreadBound;
use objc2::{MainThreadMarker, rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{
    NSWindow, NSWindowButton, NSWindowDidBecomeKeyNotification,
    NSWindowDidChangeBackingPropertiesNotification, NSWindowDidExitFullScreenNotification,
    NSWindowDidResignKeyNotification, NSWindowDidResizeNotification, NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint};
use tauri::{WebviewWindow, WindowEvent};

const TOOLBAR_HEIGHT: f64 = 40.0;

struct ChromeObservers(Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>);

impl ChromeObservers {
    fn new(window: &NSWindow) -> Self {
        let center = NSNotificationCenter::defaultCenter();
        let block = RcBlock::new(|notification: NonNull<NSNotification>| {
            if MainThreadMarker::new().is_none() {
                return;
            }
            // SAFETY: NotificationCenter passes a live notification for the duration of this callback.
            let notification = unsafe { notification.as_ref() };
            if let Some(object) = notification.object()
                && let Some(window) = object.downcast_ref::<NSWindow>()
            {
                align_native(window);
            }
        });
        // SAFETY: AppKit provides these notification names as immutable process-lifetime constants.
        let names = unsafe {
            [
                NSWindowDidResizeNotification,
                NSWindowDidChangeBackingPropertiesNotification,
                NSWindowDidBecomeKeyNotification,
                NSWindowDidResignKeyNotification,
                NSWindowDidExitFullScreenNotification,
            ]
        };
        Self(
            names
                .into_iter()
                .map(|name| {
                    // SAFETY: The filter is a live NSWindow. The capture-free callback checks the main thread.
                    unsafe {
                        center.addObserverForName_object_queue_usingBlock(
                            Some(name),
                            Some(window),
                            None,
                            &block,
                        )
                    }
                })
                .collect(),
        )
    }
}

impl Drop for ChromeObservers {
    fn drop(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        for token in &self.0 {
            let observer: &ProtocolObject<dyn NSObjectProtocol> = token;
            // SAFETY: Each token comes from this notification center and remains retained until removal.
            unsafe { center.removeObserver(observer.as_ref()) };
        }
    }
}

pub(super) fn install(window: &WebviewWindow) {
    let handle = window.clone();
    with_native_window(window, move |native, mtm| {
        let observers = Mutex::new(Some(MainThreadBound::new(
            ChromeObservers::new(native),
            mtm,
        )));
        handle.on_window_event(move |event| {
            if matches!(event, WindowEvent::Destroyed) {
                let mut observers = observers.lock().unwrap_or_else(|error| error.into_inner());
                observers.take();
            }
        });
        align_native(native);
    });
}

pub(super) fn align(window: &WebviewWindow) {
    with_native_window(window, |native, _| align_native(native));
}

fn with_native_window(
    window: &WebviewWindow,
    callback: impl FnOnce(&NSWindow, MainThreadMarker) + Send + 'static,
) {
    if let Err(error) = window.with_webview(move |webview| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let pointer = webview.ns_window().cast::<NSWindow>();
        if pointer.is_null() {
            return;
        }
        // SAFETY: Tauri owns this window, and this callback runs on the main thread.
        callback(unsafe { &*pointer }, mtm);
    }) {
        tracing::debug!(event = "main_window_chrome_dispatch_failed", %error);
    }
}

fn align_native(native: &NSWindow) {
    if native.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return;
    }
    let center_y = native.frame().size.height - TOOLBAR_HEIGHT / 2.0;
    for kind in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        let Some(button) = native.standardWindowButton(kind) else {
            continue;
        };
        // SAFETY: AppKit owns the parent, and this callback runs on the main thread.
        let Some(parent) = (unsafe { button.superview() }) else {
            continue;
        };
        let frame = button.frame();
        let window_rect = button.convertRect_toView(button.bounds(), None);
        let center = parent.convertPoint_fromView(
            NSPoint::new(
                window_rect.origin.x + window_rect.size.width / 2.0,
                center_y,
            ),
            None,
        );
        button.setFrameOrigin(NSPoint::new(
            frame.origin.x,
            center.y - frame.size.height / 2.0,
        ));
        if kind == NSWindowButton::CloseButton {
            let aligned = button.convertRect_toView(button.bounds(), None);
            let before =
                native.frame().size.height - window_rect.origin.y - window_rect.size.height / 2.0;
            let after = native.frame().size.height - aligned.origin.y - aligned.size.height / 2.0;
            if (before - after).abs() > 0.5 {
                tracing::debug!(
                    event = "main_window_traffic_lights_aligned",
                    before,
                    after,
                    target = TOOLBAR_HEIGHT / 2.0,
                );
            }
        }
    }
}
