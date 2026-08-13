// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Desktop notifications, and the rules about when antiburn is allowed to use
//! one.
//!
//! A notification is the only thing this application does that a reader cannot
//! choose to look at. That makes it the surface where restraint matters most,
//! so the whole policy is here rather than spread across the callers:
//!
//! - **There are exactly two kinds.** A newer version is available, and a scan
//!   failed. Both are things a reader would act on; neither is a progress
//!   report. Nothing else in the app posts a notification.
//! - **Every kind is gated twice** — once by the master preference and once by
//!   its own ([`allowed`]). Both default on, because a notification surface
//!   that has to be discovered before it says anything is a surface nobody
//!   discovers.
//! - **Nothing repeats.** A scan failure is announced once per run of the app,
//!   not once per tick ([`NotificationState::claim_scan_failure`]), and a
//!   version is announced once, not every six hours
//!   ([`NotificationState::claim_update`]). The six-hourly update schedule
//!   makes this the difference between a useful app and an alarm clock.
//! - **Only the shell posts one.** The webview is granted no notification
//!   permission (`capabilities/default.json`), so "what is worth interrupting
//!   someone for" is decided in one place, in this file.
//!
//! Delivery goes through the [`Notifier`] seam: the decisions above are
//! ordinary functions over settings and state, and the one line that talks to
//! the operating system is behind a trait so the tests never need a
//! notification centre.
//!
//! Nothing here is network-capable. The platform plugin hands a title and a
//! body to the local notification centre — on macOS through the system's own
//! delivery service, on Linux through D-Bus, on Windows through WinRT — and
//! antiburn's notification bodies carry a version string or a scan error,
//! never session content.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager};

use crate::dto::ScanStatus;
use crate::store::{AppSettings, Store};
use crate::updates::UpdateStatus;

/// Something antiburn is willing to interrupt a reader for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An update check found a newer version.
    UpdateAvailable,
    /// A scan pass ended in an error.
    ScanFailure,
}

/// Whether `kind` may be delivered under these preferences.
///
/// The master switch wins: turning notifications off turns *all* of them off,
/// without the two per-kind preferences having to be rewritten (so turning the
/// master back on restores the reader's earlier choices rather than a default).
pub fn allowed(settings: &AppSettings, kind: Kind) -> bool {
    settings.notifications_enabled
        && match kind {
            Kind::UpdateAvailable => settings.notify_update_available,
            Kind::ScanFailure => settings.notify_scan_failure,
        }
}

/// What has already been said this run, so nothing is said twice.
///
/// Registered as Tauri managed state. Deliberately not persisted: "have I
/// already told you about this" is a fact about *this* run of the app, and a
/// reader who restarts antiburn after a failure should be told again if it is
/// still failing.
#[derive(Default)]
pub struct NotificationState {
    scan_failure_reported: AtomicBool,
    update_version_reported: Mutex<Option<String>>,
}

impl NotificationState {
    /// Claim the right to report a scan failure, once per run.
    ///
    /// Returns true exactly once. A scan runs every minute while the popover is
    /// open, and whatever broke the first pass — an unreadable directory, a
    /// full disk — is overwhelmingly likely to break the next sixty as well;
    /// one notification per run says the same thing without becoming the
    /// reason someone quits the app.
    pub fn claim_scan_failure(&self) -> bool {
        !self.scan_failure_reported.swap(true, Ordering::SeqCst)
    }

    /// Claim the right to report `version`, once per version.
    ///
    /// The automatic check repeats every six hours and keeps finding the same
    /// release until it is installed. Keying on the version rather than on a
    /// flag means a *newer* release is still announced.
    pub fn claim_update(&self, version: &str) -> bool {
        let mut reported = self
            .update_version_reported
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if reported.as_deref() == Some(version) {
            return false;
        }
        *reported = Some(version.to_string());
        true
    }
}

/// The one line that talks to the operating system.
///
/// Behind a trait so the policy above is testable without a notification
/// centre — and so a platform that refuses to deliver (an unbundled
/// development run on macOS, a Linux session with no notification daemon) is a
/// logged failure rather than a panic.
pub trait Notifier {
    fn deliver(&self, title: &str, body: &str);
}

/// The platform's own notification centre, through the Tauri plugin.
pub struct PlatformNotifier<'a>(pub &'a AppHandle);

impl Notifier for PlatformNotifier<'_> {
    fn deliver(&self, title: &str, body: &str) {
        use tauri_plugin_notification::NotificationExt as _;

        if let Err(error) = self
            .0
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
        {
            // Not fatal, and not retried: a notification that could not be
            // delivered has already been overtaken by whatever the reader is
            // doing instead.
            eprintln!("antiburn: could not post a notification ({error})");
        }
    }
}

/// The title and body of an update notification.
///
/// Deliberately does not say "ready to install". This build checks the release
/// feed and reports what it found; downloading and installing an update is the
/// release milestone's work and has no UI yet (`docs/deviations.md`). A
/// notification that promised an install button would be describing an app
/// nobody has shipped.
pub fn update_message(version: &str) -> (String, String) {
    (
        "antiburn update available".to_string(),
        format!(
            "Version {version} is on the release feed. \
             This build checks for updates but does not install them yet — \
             Settings → Updates shows what the last check found."
        ),
    )
}

/// The title and body of a scan-failure notification.
///
/// The error is the store's or the engine's own words. It names a path or a
/// failure at worst — the scan never handles transcript text, so there is none
/// to leak into a notification.
pub fn scan_failure_message(error: &str) -> (String, String) {
    (
        "antiburn could not finish a scan".to_string(),
        format!("{error}. Open antiburn to try again; everything already indexed is unaffected."),
    )
}

/// Report a finished scan pass, if it failed and nothing has reported one yet.
pub fn note_scan_outcome(app: &AppHandle, status: &ScanStatus) {
    let Some(error) = status.error.as_deref() else {
        return;
    };
    if !enabled(app, Kind::ScanFailure) {
        return;
    }
    let Some(state) = app.try_state::<NotificationState>() else {
        return;
    };
    if !state.claim_scan_failure() {
        return;
    }
    let (title, body) = scan_failure_message(error);
    PlatformNotifier(app).deliver(&title, &body);
}

/// Report the outcome of an update check, if it found a version worth naming.
///
/// Only the shell's own schedule reaches here (see [`crate::updates`]): a
/// reader who pressed "Check for updates" is already looking at the answer, and
/// notifying them about it would be telling them what they can see.
pub fn note_update_status(app: &AppHandle, status: &UpdateStatus) {
    if status.kind != "available" {
        return;
    }
    let Some(version) = status.version.as_deref() else {
        return;
    };
    if !enabled(app, Kind::UpdateAvailable) {
        return;
    }
    let Some(state) = app.try_state::<NotificationState>() else {
        return;
    };
    if !state.claim_update(version) {
        return;
    }
    let (title, body) = update_message(version);
    PlatformNotifier(app).deliver(&title, &body);
}

/// The reader's preferences, read fresh, defaulting to *silence* when the store
/// cannot be read: an unreadable preference is not permission.
fn enabled(app: &AppHandle, kind: Kind) -> bool {
    app.try_state::<Store>()
        .and_then(|store| store.settings().ok())
        .is_some_and(|settings| allowed(&settings, kind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A notifier that records instead of delivering.
    #[derive(Default)]
    struct Recorder(RefCell<Vec<(String, String)>>);

    impl Notifier for Recorder {
        fn deliver(&self, title: &str, body: &str) {
            self.0
                .borrow_mut()
                .push((title.to_string(), body.to_string()));
        }
    }

    fn settings() -> AppSettings {
        AppSettings::default()
    }

    #[test]
    fn both_kinds_are_on_for_a_fresh_install() {
        let settings = settings();
        assert!(allowed(&settings, Kind::UpdateAvailable));
        assert!(allowed(&settings, Kind::ScanFailure));
    }

    #[test]
    fn the_master_switch_silences_every_kind() {
        let settings = AppSettings {
            notifications_enabled: false,
            // Left on deliberately: turning the master switch back on must
            // restore what the reader chose, not a default.
            notify_update_available: true,
            notify_scan_failure: true,
            ..settings()
        };
        assert!(!allowed(&settings, Kind::UpdateAvailable));
        assert!(!allowed(&settings, Kind::ScanFailure));
    }

    #[test]
    fn a_kind_can_be_silenced_on_its_own() {
        let settings = AppSettings {
            notify_scan_failure: false,
            ..settings()
        };
        assert!(allowed(&settings, Kind::UpdateAvailable));
        assert!(!allowed(&settings, Kind::ScanFailure));
    }

    #[test]
    fn a_scan_failure_is_reported_once_per_run_not_once_per_tick() {
        let state = NotificationState::default();
        assert!(state.claim_scan_failure());
        for _ in 0..60 {
            assert!(
                !state.claim_scan_failure(),
                "a scan runs every minute; the failure is announced once"
            );
        }
    }

    #[test]
    fn one_version_is_announced_once_but_a_newer_one_is_still_announced() {
        let state = NotificationState::default();
        assert!(state.claim_update("0.2.0"));
        // The six-hourly check keeps finding this release until it is installed.
        assert!(!state.claim_update("0.2.0"));
        assert!(state.claim_update("0.3.0"));
        assert!(!state.claim_update("0.3.0"));
    }

    #[test]
    fn the_delivered_copy_names_the_version_and_claims_no_install_this_build_cannot_do() {
        let recorder = Recorder::default();
        let (title, body) = update_message("0.2.0");
        recorder.deliver(&title, &body);

        let delivered = recorder.0.borrow();
        let (title, body) = &delivered[0];
        assert!(title.contains("antiburn"));
        assert!(body.contains("0.2.0"));
        assert!(
            body.contains("Settings"),
            "a notification must say where to look"
        );
        // There is no install flow in this build; the copy must not imply one.
        assert!(
            body.contains("does not install them yet"),
            "an update notification must not promise an install this build cannot perform"
        );
    }

    #[test]
    fn a_scan_failure_notification_carries_the_error_and_reassures() {
        let (title, body) = scan_failure_message("permission denied reading /home/avery/code");
        assert!(title.contains("scan"));
        assert!(body.contains("permission denied"));
        assert!(
            body.contains("already indexed"),
            "a failure must not read as data loss"
        );
    }
}
