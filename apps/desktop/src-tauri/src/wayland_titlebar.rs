//! Makes the title-bar buttons work on native Wayland.
//!
//! On Wayland, tao 0.35 draws its own title bar: a GTK `HeaderBar` inside a
//! `GtkEventBox` that has the `above-child` flag. The flag routes every
//! pointer event to the event box before the buttons can see the event.
//! Close and Minimize then do nothing (tauri-apps/tao#899,
//! tauri-apps/tao#1046). The shell clears the flag after it builds a
//! decorated window. GTK then delivers clicks to the buttons again.
//! Title-bar drag keeps working outside the buttons, as in every GTK
//! application.
//!
//! The repair changes nothing in most Linux sessions. `prefer_x11_backend`
//! (see `main.rs`) rewrites a session-default `GDK_BACKEND=wayland` to
//! `x11`, and the repair returns early on a backend that is not Wayland.
//! The repair matters on a deliberate native Wayland run
//! (`ANTIBURN_GDK_BACKEND=wayland`) and in a session that has no X server.
//!
//! Every decorated window must call [`repair`]. The undecorated windows do
//! not need the call. tao 0.36 removes the event box (tauri-apps/tao#1218).
//! Remove this module when a stable tauri release ships tao >= 0.36.

use tauri::WebviewWindow;

/// Lets the native title-bar buttons of `window` receive clicks.
///
/// Call this on the main thread, directly after the shell builds the
/// window, and before the window shows.
#[cfg(target_os = "linux")]
pub fn repair(window: &WebviewWindow) {
    use gtk::gdk::prelude::DisplayExtManual;
    use gtk::prelude::{Cast, EventBoxExt, GtkWindowExt, WidgetExt};

    let Ok(gtk_window) = window.gtk_window() else {
        return;
    };
    if !gtk_window.display().backend().is_wayland() {
        return;
    }
    let Some(titlebar) = gtk_window.titlebar() else {
        return;
    };
    // A different widget shape means tao changed its title bar, and the
    // repair is stale. See the removal note above.
    let Ok(event_box) = titlebar.downcast::<gtk::EventBox>() else {
        ::tracing::warn!(event = "wayland_titlebar_shape_changed", window = %window.label());
        return;
    };
    event_box.set_above_child(false);
}

/// The other platforms draw native title bars that need no repair.
#[cfg(not(target_os = "linux"))]
pub fn repair(_window: &WebviewWindow) {}
