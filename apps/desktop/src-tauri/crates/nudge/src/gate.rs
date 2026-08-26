// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Best-effort OS interruption gate for automated nudges.
//!
//! Each platform probe maps its failures to [`NotificationGateState::Unknown`].
//! Delivery fails open in that state, with bounded diagnostics, so a broken
//! desktop integration cannot silence notifications forever.
//!
//! The gate keeps no resident state between probes. Each query runs on demand,
//! immediately before one automated nudge shows, and retains nothing after it
//! returns. Do not replace this with an observer or a cached listener: the app
//! idles in the menu bar, and its resting footprint must not grow.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

const UNKNOWN_LOG_LIMIT: u8 = 3;

/// The operating system's current willingness to accept an automated nudge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationGateState {
    Allowed,
    Suppressed,
    Unknown,
}

/// Queries the current platform immediately before automated nudge delivery.
pub struct NotificationGate {
    authorization_initialized: AtomicBool,
    unknown_logs: AtomicU8,
}

impl Default for NotificationGate {
    fn default() -> Self {
        Self {
            authorization_initialized: AtomicBool::new(false),
            unknown_logs: AtomicU8::new(0),
        }
    }
}

impl NotificationGate {
    /// Query the platform's current notification-interruption state.
    pub fn state(&self) -> NotificationGateState {
        platform_state()
    }

    /// Whether an automated nudge may be delivered now.
    ///
    /// The unknown state permits delivery on purpose. Only the first few
    /// unknown results write a log line, so an absent desktop service cannot
    /// flood the log.
    pub fn delivery_allowed(&self) -> bool {
        self.delivery_allowed_for(self.state())
    }

    fn delivery_allowed_for(&self, state: NotificationGateState) -> bool {
        match state {
            NotificationGateState::Allowed => true,
            NotificationGateState::Suppressed => false,
            NotificationGateState::Unknown => {
                if self
                    .unknown_logs
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                        (count < UNKNOWN_LOG_LIMIT).then_some(count + 1)
                    })
                    .is_ok()
                {
                    tracing::debug!(
                        event = "nudge_notification_gate_unknown",
                        "OS notification state unavailable; allowing automated nudge"
                    );
                }
                true
            }
        }
    }

    /// Start the platform authorization needed to query interruption state.
    ///
    /// macOS can show its Focus-status authorization prompt here. The request
    /// runs at most once per process, and only when the app tells the gate
    /// that the notification preference is enabled. Other platforms need no
    /// authorization, so this is a no-op there.
    pub fn initialize_authorization(&self) {
        let first_request = self
            .authorization_initialized
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok();

        #[cfg(target_os = "macos")]
        if first_request {
            macos::initialize_authorization();
        }

        #[cfg(not(target_os = "macos"))]
        let _ = first_request;
    }
}

#[cfg(target_os = "macos")]
fn platform_state() -> NotificationGateState {
    macos::state()
}

#[cfg(windows)]
fn platform_state() -> NotificationGateState {
    windows::state()
}

#[cfg(target_os = "linux")]
fn platform_state() -> NotificationGateState {
    linux::state()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn platform_state() -> NotificationGateState {
    NotificationGateState::Unknown
}

#[cfg(any(test, target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacAuthorization {
    Authorized,
    NotDetermined,
    Restricted,
    Denied,
    Unknown,
}

#[cfg(any(test, target_os = "macos"))]
fn macos_state(authorization: MacAuthorization, is_focused: Option<bool>) -> NotificationGateState {
    match (authorization, is_focused) {
        (MacAuthorization::Authorized, Some(true)) => NotificationGateState::Suppressed,
        (MacAuthorization::Authorized, Some(false)) => NotificationGateState::Allowed,
        _ => NotificationGateState::Unknown,
    }
}

#[cfg(any(test, target_os = "macos"))]
fn macos_app_bundle_executable(path: &std::path::Path) -> bool {
    let Some(mac_os_dir) = path.parent() else {
        return false;
    };
    let Some(contents_dir) = mac_os_dir.parent() else {
        return false;
    };
    let Some(app_bundle) = contents_dir.parent() else {
        return false;
    };
    mac_os_dir.file_name() == Some(std::ffi::OsStr::new("MacOS"))
        && contents_dir.file_name() == Some(std::ffi::OsStr::new("Contents"))
        && app_bundle.extension() == Some(std::ffi::OsStr::new("app"))
}

#[cfg(target_os = "macos")]
mod macos {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::NSError;
    use objc2_intents::{
        INFocusStatusAuthorizationStatus as AuthorizationStatus, INFocusStatusCenter,
    };
    use objc2_user_notifications::{UNAuthorizationOptions, UNUserNotificationCenter};

    use super::{
        MacAuthorization, NotificationGateState, macos_app_bundle_executable, macos_state,
    };

    fn authorization(raw: AuthorizationStatus) -> MacAuthorization {
        if raw == AuthorizationStatus::Authorized {
            MacAuthorization::Authorized
        } else if raw == AuthorizationStatus::NotDetermined {
            MacAuthorization::NotDetermined
        } else if raw == AuthorizationStatus::Restricted {
            MacAuthorization::Restricted
        } else if raw == AuthorizationStatus::Denied {
            MacAuthorization::Denied
        } else {
            MacAuthorization::Unknown
        }
    }

    pub(super) fn state() -> NotificationGateState {
        // SAFETY: Intents.framework is available on the app's macOS 13 minimum.
        // The generated bindings keep Objective-C ownership, and every returned
        // object stays retained for the duration of this query.
        unsafe {
            let center = INFocusStatusCenter::defaultCenter();
            let authorization = authorization(center.authorizationStatus());
            let focused = (authorization == MacAuthorization::Authorized)
                .then(|| center.focusStatus().isFocused())
                .flatten()
                .map(|value| value.boolValue());
            macos_state(authorization, focused)
        }
    }

    fn request_focus_authorization() {
        // SAFETY: The completion block has an Objective-C-compatible signature,
        // and Intents.framework copies it for the asynchronous request.
        unsafe {
            let center = INFocusStatusCenter::defaultCenter();
            if center.authorizationStatus() != AuthorizationStatus::NotDetermined {
                return;
            }
            let completion = RcBlock::new(move |status: AuthorizationStatus| {
                tracing::debug!(
                    event = "nudge_focus_authorization_completed",
                    status = status.0
                );
            });
            center.requestAuthorizationWithCompletionHandler(Some(&completion));
        }
    }

    pub(super) fn initialize_authorization() {
        // UserNotifications raises an Objective-C exception (which Rust cannot
        // catch) when the unbundled binary from `tauri dev` calls
        // `currentNotificationCenter`. Such a binary also lacks the signed
        // communication entitlement, so skip the query and fail open.
        let Ok(executable) = std::env::current_exe() else {
            tracing::debug!(event = "nudge_authorization_executable_unknown");
            return;
        };
        if !macos_app_bundle_executable(&executable) {
            tracing::debug!(
                event = "nudge_authorization_skipped_unbundled",
                executable = %executable.display()
            );
            return;
        }

        // The Focus status has a value only after both User Notifications and
        // Focus-status authorization. Ask for the modern notification
        // authorization first, then request Focus authorization from its
        // completion, so macOS never has to show two permission sheets at the
        // same time.
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let completion = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            tracing::debug!(
                event = "nudge_notification_authorization_completed",
                granted = granted.as_bool(),
                error = !error.is_null()
            );
            request_focus_authorization();
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert,
            &completion,
        );
    }
}

#[cfg(any(test, windows))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsProbe<T> {
    Value(T),
    Missing,
    Failed,
}

#[cfg(any(test, windows))]
fn windows_state(
    shell_state: WindowsProbe<i32>,
    global_toasts: WindowsProbe<u32>,
) -> NotificationGateState {
    if matches!(global_toasts, WindowsProbe::Value(0)) {
        return NotificationGateState::Suppressed;
    }
    match shell_state {
        // QUNS_ACCEPTS_NOTIFICATIONS
        WindowsProbe::Value(5) => NotificationGateState::Allowed,
        WindowsProbe::Value(_) => NotificationGateState::Suppressed,
        WindowsProbe::Missing | WindowsProbe::Failed => NotificationGateState::Unknown,
    }
}

#[cfg(windows)]
mod windows {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::Win32::UI::Shell::SHQueryUserNotificationState;
    use windows::core::w;

    use super::{NotificationGateState, WindowsProbe, windows_state};

    fn global_toasts() -> WindowsProbe<u32> {
        let mut value = 0_u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: Both strings are static UTF-16 values. `value` and `size`
        // point to writable storage of the sizes passed to RegGetValueW.
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings"),
                w!("NOC_GLOBAL_SETTING_TOASTS_ENABLED"),
                RRF_RT_REG_DWORD,
                None,
                Some((&mut value as *mut u32).cast()),
                Some(&mut size),
            )
        };
        if result == ERROR_SUCCESS {
            WindowsProbe::Value(value)
        } else if result == ERROR_FILE_NOT_FOUND || result == ERROR_PATH_NOT_FOUND {
            WindowsProbe::Missing
        } else {
            WindowsProbe::Failed
        }
    }

    pub(super) fn state() -> NotificationGateState {
        // SAFETY: SHQueryUserNotificationState takes no caller-owned pointers;
        // the generated wrapper turns HRESULT failure into Result::Err.
        let shell = unsafe { SHQueryUserNotificationState() }
            .map(|state| WindowsProbe::Value(state.0))
            .unwrap_or(WindowsProbe::Failed);
        windows_state(shell, global_toasts())
    }
}

#[cfg(any(test, target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinuxProbe<T> {
    Value(T),
    Missing,
    Malformed,
    TimedOut,
    Failed,
}

#[cfg(any(test, target_os = "linux"))]
fn linux_state(
    gnome_banners: LinuxProbe<bool>,
    plasma_inhibited: LinuxProbe<bool>,
) -> NotificationGateState {
    if matches!(gnome_banners, LinuxProbe::Value(false))
        || matches!(plasma_inhibited, LinuxProbe::Value(true))
    {
        NotificationGateState::Suppressed
    } else if matches!(gnome_banners, LinuxProbe::Value(true))
        || matches!(plasma_inhibited, LinuxProbe::Value(false))
    {
        NotificationGateState::Allowed
    } else {
        NotificationGateState::Unknown
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use gio::prelude::SettingsExt;
    use zbus::blocking::{Proxy, connection};
    use zbus::proxy::MethodFlags;
    use zbus::zvariant::OwnedValue;

    use super::{LinuxProbe, NotificationGateState, linux_state};

    const PLASMA_TIMEOUT: Duration = Duration::from_millis(150);
    static PLASMA_PROBE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

    fn gnome_banners() -> LinuxProbe<bool> {
        let Some(source) = gio::SettingsSchemaSource::default() else {
            return LinuxProbe::Missing;
        };
        let Some(schema) = source.lookup("org.gnome.desktop.notifications", true) else {
            return LinuxProbe::Missing;
        };
        if !schema.has_key("show-banners") {
            return LinuxProbe::Missing;
        }
        let settings = gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, None);
        LinuxProbe::Value(settings.boolean("show-banners"))
    }

    fn classify_error(error: &zbus::Error) -> LinuxProbe<bool> {
        let message = error.to_string().to_ascii_lowercase();
        if message.contains("timed out") || message.contains("timeout") {
            LinuxProbe::TimedOut
        } else if message.contains("namehasnoowner")
            || message.contains("serviceunknown")
            || message.contains("no such")
        {
            LinuxProbe::Missing
        } else {
            LinuxProbe::Failed
        }
    }

    fn plasma_inhibited_blocking() -> LinuxProbe<bool> {
        let connection = match connection::Builder::session()
            .map(|builder| builder.method_timeout(PLASMA_TIMEOUT))
            .and_then(connection::Builder::build)
        {
            Ok(connection) => connection,
            Err(error) => return classify_error(&error),
        };
        let proxy = match Proxy::new(
            &connection,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.DBus.Properties",
        ) {
            Ok(proxy) => proxy,
            Err(error) => return classify_error(&error),
        };
        let reply: Option<OwnedValue> = match proxy.call_with_flags(
            "Get",
            MethodFlags::NoAutoStart.into(),
            &("org.freedesktop.Notifications", "Inhibited"),
        ) {
            Ok(reply) => reply,
            Err(error) => return classify_error(&error),
        };
        let Some(value) = reply else {
            return LinuxProbe::Malformed;
        };
        bool::try_from(value)
            .map(LinuxProbe::Value)
            .unwrap_or(LinuxProbe::Malformed)
    }

    fn plasma_inhibited() -> LinuxProbe<bool> {
        // `Proxy::call_with_flags` bypasses the connection-level method
        // timeout in zbus, so a worker bounds the whole operation, including
        // the bus connection. If a broken bus never returns, keep at most one
        // worker and fail later probes open, so blocked threads do not
        // accumulate.
        if PLASMA_PROBE_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return LinuxProbe::TimedOut;
        }

        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("antiburn-plasma-notification-probe".to_string())
            .spawn(move || {
                let result = plasma_inhibited_blocking();
                PLASMA_PROBE_IN_FLIGHT.store(false, Ordering::Release);
                let _ = sender.send(result);
            });
        if worker.is_err() {
            PLASMA_PROBE_IN_FLIGHT.store(false, Ordering::Release);
            return LinuxProbe::Failed;
        }

        receiver
            .recv_timeout(PLASMA_TIMEOUT)
            .unwrap_or(LinuxProbe::TimedOut)
    }

    pub(super) fn state() -> NotificationGateState {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_ascii_uppercase();
        if desktop.contains("GNOME") || desktop.contains("UNITY") {
            linux_state(gnome_banners(), LinuxProbe::Missing)
        } else if desktop.contains("KDE") || desktop.contains("PLASMA") {
            linux_state(LinuxProbe::Missing, plasma_inhibited())
        } else {
            NotificationGateState::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_active_inactive_and_unauthorized_states() {
        assert_eq!(
            macos_state(MacAuthorization::Authorized, Some(true)),
            NotificationGateState::Suppressed
        );
        assert_eq!(
            macos_state(MacAuthorization::Authorized, Some(false)),
            NotificationGateState::Allowed
        );
        for authorization in [
            MacAuthorization::NotDetermined,
            MacAuthorization::Restricted,
            MacAuthorization::Denied,
            MacAuthorization::Unknown,
        ] {
            assert_eq!(
                macos_state(authorization, None),
                NotificationGateState::Unknown
            );
        }
        assert_eq!(
            macos_state(MacAuthorization::Authorized, None),
            NotificationGateState::Unknown
        );
    }

    #[test]
    fn macos_authorization_requires_an_app_bundle_executable() {
        assert!(macos_app_bundle_executable(std::path::Path::new(
            "/Applications/antiburn.app/Contents/MacOS/antiburn"
        )));
        assert!(!macos_app_bundle_executable(std::path::Path::new(
            "/work/apps/desktop/src-tauri/target/debug/antiburn"
        )));
        assert!(!macos_app_bundle_executable(std::path::Path::new(
            "/Applications/antiburn.app/antiburn"
        )));
    }

    #[test]
    fn every_windows_notification_state_is_mapped() {
        for raw in [1, 2, 3, 4, 6, 7] {
            assert_eq!(
                windows_state(WindowsProbe::Value(raw), WindowsProbe::Missing),
                NotificationGateState::Suppressed,
                "QUNS state {raw} must suppress"
            );
        }
        assert_eq!(
            windows_state(WindowsProbe::Value(5), WindowsProbe::Missing),
            NotificationGateState::Allowed
        );
        assert_eq!(
            windows_state(WindowsProbe::Failed, WindowsProbe::Missing),
            NotificationGateState::Unknown
        );
    }

    #[test]
    fn windows_global_toast_setting_is_fail_open_except_explicit_zero() {
        assert_eq!(
            windows_state(WindowsProbe::Value(5), WindowsProbe::Value(0)),
            NotificationGateState::Suppressed
        );
        assert_eq!(
            windows_state(WindowsProbe::Value(5), WindowsProbe::Value(1)),
            NotificationGateState::Allowed
        );
        assert_eq!(
            windows_state(WindowsProbe::Value(5), WindowsProbe::Missing),
            NotificationGateState::Allowed
        );
        assert_eq!(
            windows_state(WindowsProbe::Failed, WindowsProbe::Failed),
            NotificationGateState::Unknown
        );
    }

    #[test]
    fn linux_gnome_and_plasma_states_are_mapped() {
        assert_eq!(
            linux_state(LinuxProbe::Value(false), LinuxProbe::Missing),
            NotificationGateState::Suppressed
        );
        assert_eq!(
            linux_state(LinuxProbe::Value(true), LinuxProbe::Missing),
            NotificationGateState::Allowed
        );
        assert_eq!(
            linux_state(LinuxProbe::Missing, LinuxProbe::Value(true)),
            NotificationGateState::Suppressed
        );
        assert_eq!(
            linux_state(LinuxProbe::Missing, LinuxProbe::Value(false)),
            NotificationGateState::Allowed
        );
    }

    #[test]
    fn linux_missing_malformed_timeout_and_failure_states_fail_open() {
        for unavailable in [
            LinuxProbe::Missing,
            LinuxProbe::Malformed,
            LinuxProbe::TimedOut,
            LinuxProbe::Failed,
        ] {
            assert_eq!(
                linux_state(unavailable, unavailable),
                NotificationGateState::Unknown
            );
        }
    }

    #[test]
    fn unknown_state_allows_delivery() {
        let gate = NotificationGate::default();
        for _ in 0..(UNKNOWN_LOG_LIMIT + 2) {
            assert!(gate.delivery_allowed_for(NotificationGateState::Unknown));
        }
        assert_eq!(gate.unknown_logs.load(Ordering::Relaxed), UNKNOWN_LOG_LIMIT);
    }
}
