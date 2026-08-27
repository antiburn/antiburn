// antiburn has no console UI; a Windows release build must not flash a
// terminal behind the popover.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Sets the GDK backend to x11 so tao can position windows.
///
/// The popover window hangs off the tray item. The notification window
/// sits in a screen corner. Both need the application to set the
/// position of its own top-level windows.
///
/// On a Wayland session, this does not work. A Wayland compositor
/// ignores the position of a top-level window. tao's `set_position` call
/// becomes a `gtk_window_move` call, and the compositor drops it. tao
/// also hardcodes the cursor position to (0, 0) on Wayland, so the code
/// that finds the current monitor fails too. Running under XWayland
/// fixes both problems, so this function selects the x11 backend when
/// it can.
#[cfg(target_os = "linux")]
fn prefer_x11_backend() {
    // The reader's own override always wins. Do not replace it.
    if std::env::var_os("GDK_BACKEND").is_some() {
        return;
    }
    // DISPLAY is absent when no X server and no XWayland socket exist.
    // Forcing x11 here would stop GTK from starting. A native Wayland
    // session without XWayland must still start normally.
    if std::env::var_os("DISPLAY").is_none() {
        return;
    }
    // SAFETY: This call runs at the start of `main`. No thread has
    // started yet, and GTK has not read the environment. No other
    // thread can read or write the environment at this point.
    unsafe {
        std::env::set_var("GDK_BACKEND", "x11");
    }
}

#[cfg(not(target_os = "linux"))]
fn prefer_x11_backend() {}

fn main() {
    prefer_x11_backend();
    antiburn_desktop::run();
}
