//! Distinguish explicit opens from background startup without changing preferences.

use std::ffi::OsStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaunchIntent {
    Explicit,
    Background,
}

/// Read launch arguments, including the executable at index zero.
pub(crate) fn from_args<I, S>(arguments: I) -> LaunchIntent
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    if arguments
        .into_iter()
        .skip(1)
        .take_while(|argument| argument.as_ref() != OsStr::new("--"))
        .any(|argument| argument.as_ref() == OsStr::new("--background"))
    {
        LaunchIntent::Background
    } else {
        LaunchIntent::Explicit
    }
}

/// Call during Tauri setup while the macOS launch event remains current.
pub(crate) fn current() -> LaunchIntent {
    if from_args(std::env::args_os()) == LaunchIntent::Background || launched_at_login() {
        LaunchIntent::Background
    } else {
        LaunchIntent::Explicit
    }
}

#[cfg(target_os = "macos")]
fn launched_at_login() -> bool {
    use objc2_foundation::NSAppleEventManager;

    let Some(event) = NSAppleEventManager::sharedAppleEventManager().currentAppleEvent() else {
        return false;
    };
    let property = event
        .paramDescriptorForKeyword(u32::from_be_bytes(*b"prdt"))
        .map(|descriptor| descriptor.enumCodeValue());
    is_login_event(event.eventID(), property)
}

#[cfg(not(target_os = "macos"))]
fn launched_at_login() -> bool {
    false
}

#[cfg(any(target_os = "macos", test))]
fn is_login_event(event_id: u32, property: Option<u32>) -> bool {
    event_id == u32::from_be_bytes(*b"oapp") && property == Some(u32::from_be_bytes(*b"lgit"))
}

#[cfg(test)]
mod tests {
    use super::{LaunchIntent, from_args, is_login_event};

    #[test]
    fn explicit_launch_does_not_depend_on_saved_login_preferences() {
        assert_eq!(from_args(["antiburn"]), LaunchIntent::Explicit);
        assert_eq!(from_args(["antiburn", "--other"]), LaunchIntent::Explicit);
    }

    #[test]
    fn background_flag_is_an_exact_option_after_the_executable() {
        assert_eq!(
            from_args(["antiburn", "--background"]),
            LaunchIntent::Background
        );
        assert_eq!(from_args(["--background"]), LaunchIntent::Explicit);
        assert_eq!(
            from_args(["antiburn", "--background=false"]),
            LaunchIntent::Explicit
        );
        assert_eq!(
            from_args(["antiburn", "--", "--background"]),
            LaunchIntent::Explicit
        );
    }

    #[test]
    fn only_the_login_open_event_selects_background_startup() {
        let open = u32::from_be_bytes(*b"oapp");
        let login = u32::from_be_bytes(*b"lgit");
        assert!(is_login_event(open, Some(login)));
        assert!(!is_login_event(open, None));
        assert!(!is_login_event(open, Some(0)));
        assert!(!is_login_event(u32::from_be_bytes(*b"rapp"), Some(login)));
    }
}
