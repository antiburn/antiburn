// antiburn has no console UI; a Windows release build must not flash a
// terminal behind the popover.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Chooses the value [`prefer_x11_backend`] writes into `GDK_BACKEND`.
///
/// The function is pure, so tests cover the decision table without the
/// process environment. A value that is empty or only whitespace counts
/// as an unset one.
///
/// The rules apply in this order:
///
/// - `requested` is `ANTIBURN_GDK_BACKEND`. The function copies it into
///   `GDK_BACKEND` verbatim, and it always wins. It is the way to ask
///   for a native Wayland run on purpose.
/// - `current` is a pre-set `GDK_BACKEND`. It stands, unless
///   [`restates_the_wayland_default`] matches it. Session tooling
///   injects those values without the reader's intent, and they are the
///   values that break window placement.
/// - `display` is `DISPLAY`. Without it there is no X server and no
///   XWayland socket, and a forced x11 backend would stop GTK from
///   starting. The function then changes nothing.
#[cfg(any(target_os = "linux", test))]
fn backend_override<'a>(
    requested: Option<&'a str>,
    current: Option<&str>,
    display: Option<&str>,
) -> Option<&'a str> {
    let requested = requested.filter(|value| !value.trim().is_empty());
    let current = current.filter(|value| !value.trim().is_empty());
    let has_display = display.is_some_and(|value| !value.trim().is_empty());

    if let Some(value) = requested {
        return Some(value);
    }
    if current.is_some_and(|value| !restates_the_wayland_default(value)) {
        return None;
    }
    if !has_display {
        return None;
    }
    Some("x11")
}

/// Reports whether `value` only restates what a Wayland session does on
/// its own.
///
/// `GDK_BACKEND` holds a comma-separated backend list, and GDK tries the
/// entries in order. Session tooling injects the values this function
/// matches — plain "wayland", and wayland-first lists such as
/// "wayland,x11" or "wayland,x11,*" — as a session-wide default, not as
/// a choice about antiburn. Each matched list also names "x11" or "*"
/// itself, so a switch to x11 stays inside the set the value permits.
///
/// A list that puts another backend first, or that excludes x11, is a
/// deliberate choice. It stands.
#[cfg(any(target_os = "linux", test))]
fn restates_the_wayland_default(value: &str) -> bool {
    let mut entries = value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty());
    let Some(first) = entries.next() else {
        return false;
    };
    // "*" permits every backend, so it always permits x11.
    if first == "*" {
        return true;
    }
    if first != "wayland" {
        return false;
    }
    let mut rest = entries.peekable();
    if rest.peek().is_none() {
        return true;
    }
    rest.any(|entry| entry == "x11" || entry == "*")
}

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
///
/// [`backend_override`] holds the decision rules: which pre-set values
/// stand, and how `ANTIBURN_GDK_BACKEND` selects a backend explicitly.
#[cfg(target_os = "linux")]
fn prefer_x11_backend() {
    // `var` reports an error for a value that is not UTF-8, so such a
    // value counts as an unset one.
    let requested = std::env::var("ANTIBURN_GDK_BACKEND").ok();
    let current = std::env::var("GDK_BACKEND").ok();
    let display = std::env::var("DISPLAY").ok();
    let Some(backend) =
        backend_override(requested.as_deref(), current.as_deref(), display.as_deref())
    else {
        return;
    };
    // SAFETY: This call runs at the start of `main`. No thread has
    // started yet, and GTK has not read the environment. No other
    // thread can read or write the environment at this point.
    unsafe {
        std::env::set_var("GDK_BACKEND", backend);
    }
}

#[cfg(not(target_os = "linux"))]
fn prefer_x11_backend() {}

fn main() {
    prefer_x11_backend();
    antiburn_desktop::run();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_backend_becomes_x11_when_a_display_exists() {
        assert_eq!(backend_override(None, None, Some(":0")), Some("x11"));
    }

    #[test]
    fn no_display_leaves_the_backend_alone() {
        assert_eq!(backend_override(None, None, None), None);
    }

    /// The `DISPLAY= app` idiom hides X from an application. An empty
    /// `DISPLAY` must not send GTK to a display it cannot open.
    #[test]
    fn an_empty_display_counts_as_no_display() {
        assert_eq!(backend_override(None, Some("wayland"), Some("")), None);
    }

    #[test]
    fn a_restated_wayland_default_is_overridden() {
        assert_eq!(
            backend_override(None, Some("wayland"), Some(":0")),
            Some("x11")
        );
    }

    /// A wayland-first list that also names "x11" or "*" permits the x11
    /// backend, so the switch stays inside the set the value allows.
    #[test]
    fn a_wayland_first_list_that_permits_x11_is_overridden() {
        assert_eq!(
            backend_override(None, Some("wayland,x11"), Some(":0")),
            Some("x11")
        );
        assert_eq!(
            backend_override(None, Some("wayland,x11,*"), Some(":0")),
            Some("x11")
        );
        assert_eq!(
            backend_override(None, Some(" wayland , x11 "), Some(":0")),
            Some("x11")
        );
        assert_eq!(backend_override(None, Some("*"), Some(":0")), Some("x11"));
    }

    /// A list that puts another backend first, or that excludes x11, is
    /// a deliberate choice. The function does not touch it.
    #[test]
    fn a_deliberate_backend_choice_stands() {
        assert_eq!(backend_override(None, Some("x11"), Some(":0")), None);
        assert_eq!(
            backend_override(None, Some("x11,wayland"), Some(":0")),
            None
        );
        assert_eq!(
            backend_override(None, Some("wayland,broadway"), Some(":0")),
            None
        );
        assert_eq!(backend_override(None, Some("broadway"), Some(":0")), None);
    }

    /// The dedicated variable is the deliberate override. The function
    /// copies it into `GDK_BACKEND` verbatim, with or without a display.
    #[test]
    fn the_antiburn_variable_always_wins() {
        assert_eq!(
            backend_override(Some("wayland"), None, Some(":0")),
            Some("wayland")
        );
        assert_eq!(
            backend_override(Some("wayland"), Some("x11"), None),
            Some("wayland")
        );
    }

    #[test]
    fn empty_values_count_as_unset() {
        assert_eq!(
            backend_override(Some(" "), Some(""), Some(":0")),
            Some("x11")
        );
    }
}
