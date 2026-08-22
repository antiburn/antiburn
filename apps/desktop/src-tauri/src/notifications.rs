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
//! - **Every kind is enumerated here.** A newer version, a failed scan, low
//!   disk space, unusually fast spend, a crossed usage milestone, where the app
//!   went when the first run finished, and the settings pane's own test. Each is
//!   something a reader would act on (or, for the test, explicitly asked for);
//!   none is a progress report. Nothing else in the app posts a notification.
//! - **Every kind is gated twice** — once by the master preference and once by
//!   its own ([`allowed`]). All default on, because a notification surface
//!   that has to be discovered before it says anything is a surface nobody
//!   discovers. Two kinds bypass the master switch, both because the reader
//!   just pressed the button that causes them: [`Kind::Test`] ("Show test") and
//!   [`Kind::MenuBarHome`] ("Start using antiburn").
//! - **Nothing repeats.** A scan failure is announced once per run of the app,
//!   not once per tick ([`NotificationState::claim_scan_failure`]), and a
//!   version is announced once, not every six hours
//!   ([`NotificationState::claim_update`]). The six-hourly update schedule
//!   makes this the difference between a useful app and an alarm clock.
//! - **Only the shell posts one.** The webview is granted no notification
//!   permission (`capabilities/default.json`), so "what is worth interrupting
//!   someone for" is decided in one place, in this file.
//!
//! Delivery goes to antiburn's own notification window (the `antiburn-nudge`
//! crate, via [`crate::nudges`]): the decisions above are ordinary functions
//! over settings and state, and presentation — placement, auto-dismiss, the
//! chime — is applied at the seam, never decided here.
//!
//! Every notification here is generated on this machine; none of it comes
//! from a service of ours. A nudge is an event emitted to a local webview,
//! and antiburn's notification bodies carry a version string, a scan error,
//! or a locally-estimated figure — never session content.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use antiburn_nudge::{Nudge, NudgeKind, NudgeTone};
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
    /// Free disk space dropped below the reader's threshold.
    DiskSpaceLow,
    /// Sustained spend unusually fast for this machine.
    UsageAnomaly,
    /// A live usage window crossed a milestone the reader asked about.
    UsageMilestone,
    /// A session re-sent a context the prompt cache had already stored, so
    /// the same content is billed a second time at the cache-write rate.
    CacheRehydration,
    /// The first run finished and its window went away. Says where the app is
    /// now, once, in the reader's whole time with it.
    MenuBarHome,
    /// The settings pane's "Show test" button.
    Test,
}

/// Whether `kind` may be delivered under these preferences.
///
/// The master switch wins: turning notifications off turns *all* of them off,
/// without the per-kind preferences having to be rewritten (so turning the
/// master back on restores the reader's earlier choices rather than a
/// default).
///
/// Two kinds ignore it, and for the same reason: they are the direct
/// consequence of a button the reader pressed a second earlier, not something
/// antiburn decided to say. [`Kind::Test`] exists so a reader can see what a
/// notification looks like *before* deciding to allow them. [`Kind::MenuBarHome`]
/// fires once in the app's whole life, as the first-run window closes, and it
/// is the only thing that says where the application just went — suppressing it
/// would leave a reader who turned notifications off mid-onboarding with no
/// window, no Dock icon, and no explanation.
pub fn allowed(settings: &AppSettings, kind: Kind) -> bool {
    if kind == Kind::Test || kind == Kind::MenuBarHome {
        return true;
    }
    settings.notifications_enabled
        && match kind {
            Kind::UpdateAvailable => settings.notify_update_available,
            Kind::ScanFailure => settings.notify_scan_failure,
            Kind::DiskSpaceLow => settings.notify_disk_space_low,
            Kind::UsageAnomaly => settings.notify_usage_anomalies,
            Kind::CacheRehydration => settings.notify_cache_rehydration,
            // Milestones have no single switch: the two per-window rows are
            // the preference, and an empty selection is "off".
            Kind::UsageMilestone => {
                settings.milestones_5h.any() || settings.milestones_weekly.any()
            }
            Kind::MenuBarHome | Kind::Test => true,
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

/// Build and hand off one approved nudge.
///
/// The id is unique per delivery — the view de-duplicates re-emitted events
/// by id, so reusing one would make a genuinely new nudge look like an echo.
/// Every nudge carries a dismiss CTA; `extra_action` is the one optional
/// deeper destination (routed in [`crate::nudges`]).
fn deliver(
    app: &AppHandle,
    kind: Kind,
    title: String,
    body: String,
    extra_action: Option<(&str, &str)>,
) {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let (nudge_kind, tone) = match kind {
        Kind::UpdateAvailable => (NudgeKind::UpdateAvailable, NudgeTone::Info),
        Kind::ScanFailure => (NudgeKind::ScanFailure, NudgeTone::Warning),
        Kind::DiskSpaceLow => (NudgeKind::DiskSpaceLow, NudgeTone::Warning),
        Kind::UsageAnomaly => (NudgeKind::UsageAnomaly, NudgeTone::Warning),
        Kind::UsageMilestone => (NudgeKind::UsageMilestone, NudgeTone::Info),
        Kind::CacheRehydration => (NudgeKind::CacheRehydration, NudgeTone::Warning),
        Kind::MenuBarHome => (NudgeKind::MenuBarLocation, NudgeTone::Success),
        Kind::Test => (NudgeKind::Test, NudgeTone::Info),
    };
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut nudge =
        Nudge::new(format!("antiburn-{sequence}"), nudge_kind, tone, title).reason(body);
    if let Some((id, label)) = extra_action {
        nudge = nudge.action(id, label, true);
    }
    nudge = nudge.action("dismiss", "Dismiss", extra_action.is_none());
    crate::nudges::deliver(app, nudge);
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
             Settings → About shows what the last check found."
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

/// The title and body of a low-disk notification.
pub fn disk_space_low_message(free_gb: u64, threshold_gb: u32) -> (String, String) {
    (
        format!("{free_gb} GB of disk space left"),
        format!(
            "Free space dropped below your {threshold_gb} GB mark. \
             Settings → Notifications has the threshold."
        ),
    )
}

/// The title and body of a spend-anomaly notification.
///
/// Estimates, and the copy says so: the figures come from the local pricing
/// catalog over this machine's own transcripts, never from a provider.
pub fn usage_anomaly_message(hour_usd: f64, week_usd: f64) -> (String, String) {
    (
        "Spending unusually fast".to_string(),
        format!(
            "An estimated ${hour_usd:.2} in the last hour, against roughly \
             ${week_usd:.2} across the whole past week. Estimated locally \
             from your sessions — nothing was fetched."
        ),
    )
}

/// The title and body of a cache-rehydration notification.
///
/// The body carries a count and nothing else. A session title comes from
/// transcript text, so naming the session would put session content in a
/// notification.
pub fn cache_rehydration_message(count: u64) -> (String, String) {
    let detail = if count <= 1 {
        "A session re-sent its whole context to the prompt cache.".to_string()
    } else {
        format!("A session re-sent its whole context to the prompt cache {count} times.")
    };
    (
        "Ouch \u{2014} cache rehydration".to_string(),
        format!(
            "{detail} That content was already cached, so it is charged again \
             at the cache-write rate."
        ),
    )
}

/// The title and body of a usage-milestone notification.
pub fn usage_milestone_message(
    content: &crate::provider_usage::live::MilestoneContent,
) -> (String, String) {
    let highest = content
        .crossings
        .iter()
        .map(|crossing| crossing.threshold)
        .max()
        .unwrap_or(0);
    let windows = content
        .crossings
        .iter()
        .map(|crossing| crossing.window_label.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    (
        format!("{}% of your {} used", highest, windows),
        format!(
            "{} reports the crossing; the number is the provider's own. \
             Settings → Notifications chooses which milestones speak.",
            content.provider
        ),
    )
}

/// What the reader's platform calls the strip the app now lives in.
///
/// A single word, branched once, because the whole notification is about
/// telling somebody where to look and "menu bar" is not where a Windows reader
/// should be looking. Linux panels vary enough that "system tray" is the
/// closest true thing to say.
#[cfg(target_os = "macos")]
const HOME_NOUN: &str = "menu bar";
#[cfg(not(target_os = "macos"))]
const HOME_NOUN: &str = "system tray";

/// The title and body of the "this is where antiburn lives now" notification.
///
/// One sentence each: the title carries the place, the body carries what
/// clicking there does. The notification renders antiburn's own icon beside
/// them, which is the cue that matches the glyph the reader is about to hunt
/// for, so the copy does not describe it.
///
/// Deliberately no direction — no "above", no "up there". This notice is
/// normally anchored right under the menu-bar item, but the anchor is
/// macOS-only and needs a tray rectangle the backend will not always report; on
/// the fallback path it appears at the platform's notification corner instead,
/// and any wording that pointed somewhere would be wrong exactly there. A test
/// below pins that.
pub fn menu_bar_home_message() -> (String, String) {
    (
        format!("antiburn is in your {HOME_NOUN}"),
        "Click it any time to see what your coding agents have been doing.".to_string(),
    )
}

/// The title and body of the settings pane's test notification.
pub fn test_message() -> (String, String) {
    (
        "antiburn notifications are working".to_string(),
        "This is a sample notification from your settings.".to_string(),
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
    deliver(app, Kind::ScanFailure, title, body, None);
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
    deliver(
        app,
        Kind::UpdateAvailable,
        title,
        body,
        Some(("view", "View")),
    );
}

/// Report low disk space. Returns whether the episode was consumed: `true`
/// when delivered, `false` when suppressed by preference so the caller's
/// trigger retries on its next tick (see [`crate::disk_monitor`]).
pub fn note_disk_space_low(app: &AppHandle, free_gb: u64, threshold_gb: u32) -> bool {
    if !enabled(app, Kind::DiskSpaceLow) {
        return false;
    }
    let (title, body) = disk_space_low_message(free_gb, threshold_gb);
    deliver(app, Kind::DiskSpaceLow, title, body, None);
    true
}

/// Report a spend anomaly. Once-per-episode is the caller's job
/// ([`crate::usage_alerts`]); this only applies the preference gate.
pub fn note_usage_anomaly(app: &AppHandle, hour_usd: f64, week_usd: f64) -> bool {
    if !enabled(app, Kind::UsageAnomaly) {
        return false;
    }
    let (title, body) = usage_anomaly_message(hour_usd, week_usd);
    deliver(app, Kind::UsageAnomaly, title, body, None);
    true
}

/// Report a cache rehydration. Deciding that one is *new* is the caller's job
/// ([`crate::rehydration_alert`]); this only applies the preference gate.
pub fn note_cache_rehydration(app: &AppHandle, count: u64) -> bool {
    if !enabled(app, Kind::CacheRehydration) {
        return false;
    }
    let (title, body) = cache_rehydration_message(count);
    deliver(app, Kind::CacheRehydration, title, body, None);
    true
}

/// Report crossed milestones. Selection and dedup happen in the engine
/// ([`crate::provider_usage::live`]); this only applies the preference gate.
pub fn note_usage_milestone(
    app: &AppHandle,
    content: &crate::provider_usage::live::MilestoneContent,
) -> bool {
    if !enabled(app, Kind::UsageMilestone) {
        return false;
    }
    let (title, body) = usage_milestone_message(content);
    deliver(app, Kind::UsageMilestone, title, body, None);
    true
}

/// Say where antiburn went, once, as the first-run window closes.
///
/// Ungated (see [`allowed`]) and forced to hang off the menu-bar item whatever
/// the reader's placement preference says: a notification that answers "where
/// is it" by appearing in the opposite corner of the screen from the answer
/// would be worse than none. Called only from [`crate::onboarding::finish`],
/// which the one onboarding-completed transition in
/// [`crate::commands::set_settings`] reaches — so "once" is a property of that
/// transition, not a flag anything has to remember.
pub fn note_menu_bar_home(app: &AppHandle) {
    let (title, body) = menu_bar_home_message();
    crate::nudges::anchor_next_to_the_tray(app);
    deliver(
        app,
        Kind::MenuBarHome,
        title,
        body,
        Some(("show", "Show me")),
    );
}

/// Post the settings pane's test notification. Bypasses the master switch —
/// the reader pressed the button — but is otherwise the same delivery path as
/// every real kind, so what they see is what they will get.
pub fn note_test(app: &AppHandle) {
    let (title, body) = test_message();
    deliver(app, Kind::Test, title, body, None);
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
    fn only_the_two_reader_pressed_kinds_bypass_the_master_switch() {
        let settings = AppSettings {
            notifications_enabled: false,
            ..settings()
        };
        // Both are the direct consequence of a button pressed a second
        // earlier: "Show test", and "Start using antiburn".
        assert!(allowed(&settings, Kind::Test));
        assert!(allowed(&settings, Kind::MenuBarHome));
        for kind in [
            Kind::UpdateAvailable,
            Kind::ScanFailure,
            Kind::DiskSpaceLow,
            Kind::UsageAnomaly,
            Kind::UsageMilestone,
        ] {
            assert!(!allowed(&settings, kind));
        }
    }

    #[test]
    fn the_menu_bar_copy_names_the_place_once_and_says_what_clicking_does() {
        let (title, body) = menu_bar_home_message();
        assert!(title.contains("antiburn"));
        assert!(title.contains(HOME_NOUN), "title was {title}");
        assert!(
            body.contains("Click it"),
            "a reader who has never noticed the glyph needs to be told what it does"
        );
        // Said once. The title carries the place; a body that repeats it reads
        // as two sentences about the same fact.
        assert!(!body.contains(HOME_NOUN), "body was {body}");
        // And no direction, because the anchored placement is not guaranteed —
        // on the corner fallback "above" or "up there" would be a lie.
        for direction in ["above", "up there", "top of", "pointing"] {
            assert!(!body.contains(direction), "body claims a direction: {body}");
        }
        // The platform's own word, not macOS's everywhere.
        #[cfg(target_os = "macos")]
        assert_eq!(HOME_NOUN, "menu bar");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(HOME_NOUN, "system tray");
    }

    #[test]
    fn cache_rehydration_honors_its_own_switch_and_the_master() {
        let settings = AppSettings {
            notify_cache_rehydration: false,
            ..settings()
        };
        assert!(!allowed(&settings, Kind::CacheRehydration));
        assert!(
            allowed(&settings, Kind::UsageAnomaly),
            "one kind off must leave the others alone"
        );

        let master_off = AppSettings {
            notifications_enabled: false,
            ..AppSettings::default()
        };
        assert!(!allowed(&master_off, Kind::CacheRehydration));
        assert!(allowed(&AppSettings::default(), Kind::CacheRehydration));
    }

    #[test]
    fn the_rehydration_message_counts_without_naming_a_session() {
        let (title, body) = cache_rehydration_message(1);
        assert!(title.starts_with("Ouch"), "title was {title}");
        assert!(title.contains("cache rehydration"), "title was {title}");
        // One event reads as one event, not "1 times".
        assert!(!body.contains('1'), "body was {body}");

        let (_, body) = cache_rehydration_message(4);
        assert!(body.contains('4'), "body was {body}");
        assert!(body.contains("times"), "body was {body}");

        // The reader is told why it costs, since that is the whole point.
        for message in [
            cache_rehydration_message(1).1,
            cache_rehydration_message(9).1,
        ] {
            assert!(message.contains("charged again"), "body was {message}");
        }
    }

    #[test]
    fn an_empty_milestone_selection_is_how_milestones_are_off() {
        let none = crate::store::Milestones {
            at50: false,
            at75: false,
            at90: false,
        };
        let settings = AppSettings {
            milestones_5h: none,
            milestones_weekly: none,
            ..settings()
        };
        assert!(!allowed(&settings, Kind::UsageMilestone));

        let settings = AppSettings {
            milestones_weekly: none,
            ..AppSettings::default()
        };
        // One row still selected is still a preference to hear about it.
        assert!(allowed(&settings, Kind::UsageMilestone));
    }

    #[test]
    fn disk_and_anomaly_kinds_honor_their_own_switches() {
        let settings = AppSettings {
            notify_disk_space_low: false,
            notify_usage_anomalies: false,
            ..settings()
        };
        assert!(!allowed(&settings, Kind::DiskSpaceLow));
        assert!(!allowed(&settings, Kind::UsageAnomaly));
        assert!(allowed(&settings, Kind::ScanFailure));
    }

    #[test]
    fn the_anomaly_copy_says_the_figures_are_local_estimates() {
        let (_, body) = usage_anomaly_message(12.5, 40.0);
        assert!(body.contains("$12.50"));
        assert!(
            body.contains("Estimated locally") && body.contains("nothing was fetched"),
            "an estimate must not read as a provider's own number"
        );
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
        let (title, body) = update_message("0.2.0");
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
