// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Dismissing the popover on clicks that happen outside antiburn entirely.
//!
//! **Why focus loss is not enough.** The popover is put away when it stops
//! being the key window, which covers most of what "look somewhere else" means
//! — another app's window, antiburn's own settings window. It does not cover
//! clicking the **Finder desktop**: no window becomes key, so no window
//! resigns key, so Tauri reports no focus change and the popover just sits
//! there. That was always slightly wrong; with the menu-bar item now staying
//! lit for as long as the popover is open (see [`crate::tray::set_highlight`])
//! it would also strand the icon on, which is worse than the surface being
//! open — a lit menu bar with nothing under it.
//!
//! So this module watches for two things the window layer cannot see:
//!
//! - a **global mouse-down**, which is any press that landed outside every
//!   window this process owns; and
//! - the app **resigning active**, which catches a switch away that arrives
//!   without a click at all (⌘-Tab, Mission Control).
//!
//! Both land on [`crate::popover::hide_on_outside_click`], which is a no-op
//! unless the popover is actually on screen.
//!
//! **The menu-bar item is not one of these clicks.** The status item belongs to
//! this process, so a press on it is delivered locally and a *global* monitor
//! never sees it — which is the whole reason the monitor can be this simple.
//! Clicking the item while the popover is open still closes it the way it
//! always did: focus loss fires, and the reopen-suppression window in
//! [`crate::popover`] keeps the tray's own mouse-up from reading as a fresh
//! open.
//!
//! macOS-only; a no-op elsewhere.

/// Install the global mouse-down monitor and the resign-active observer for
/// the app's lifetime.
///
/// Call once from `setup`, on the main thread, after the tray and the popover
/// exist.
#[cfg(target_os = "macos")]
pub fn install(app: &tauri::AppHandle) {
    use block2::RcBlock;
    use objc2_app_kit::{NSApplicationDidResignActiveNotification, NSEvent, NSEventMask};
    use objc2_foundation::{NSNotification, NSNotificationCenter};

    let click_app = app.clone();
    let click_handler = RcBlock::new(move |_event: core::ptr::NonNull<NSEvent>| {
        crate::popover::hide_on_outside_click(&click_app);
    });

    // Right-click as well as left: a secondary press on some other app's
    // window opens its context menu, which is just as much "looking away" as a
    // plain click.
    let mask = NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown;
    let token = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &click_handler);

    // The token exists only to *remove* the monitor later, and this monitor is
    // meant to outlive everything. Leaking it says so plainly; the alternative
    // is parking a non-`Send` AppKit handle in shared state for no reason,
    // because dropping the token would not stop the monitor either way.
    std::mem::forget(token);

    let resign_app = app.clone();
    let resign_handler = RcBlock::new(move |_notification: core::ptr::NonNull<NSNotification>| {
        crate::popover::hide_on_outside_click(&resign_app);
    });
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: the notification name and the block's signature match
    // Foundation's API. `None` for the queue means the block runs on the
    // posting thread, which for this notification is AppKit's main thread. The
    // block captures only an `AppHandle`, which is `Send + Sync`.
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidResignActiveNotification),
            None,
            None,
            &resign_handler,
        )
    };
    // Kept for the app's lifetime, for the same reason as the monitor token
    // above. The notification center holds its own copy of the block.
    std::mem::forget(observer);
}

#[cfg(not(target_os = "macos"))]
pub fn install(_app: &tauri::AppHandle) {}
