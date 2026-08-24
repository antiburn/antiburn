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
//!   disk space, a crossed usage milestone, where the app went when the first
//!   run finished, and the settings pane's own test. Each is
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
//! or a provider usage percentage — never session content.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use antiburn_nudge::{Nudge, NudgeKind, NudgeTone};
use tauri::{AppHandle, Manager};

use crate::dto::ScanStatus;
use crate::store::{AppSettings, Store};
use crate::updates::UpdateStatus;

pub(crate) const NOTIFICATION_SETTINGS_ACTION_ID: &str = "notification_settings";

/// Something antiburn is willing to interrupt a reader for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An update check found a newer version.
    UpdateAvailable,
    /// A scan pass ended in an error.
    ScanFailure,
    /// Free disk space dropped below the reader's threshold.
    DiskSpaceLow,
    /// A live usage window crossed a milestone the reader asked about.
    UsageMilestone,
    /// The first run finished and its window went away. Says where the app is
    /// now, once, in the reader's whole time with it.
    MenuBarHome,
    /// The settings pane's "Show test" button.
    Test,
}

impl Kind {
    /// The wire id the settings pane's debug row sends for each kind.
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "updateAvailable" => Self::UpdateAvailable,
            "scanFailure" => Self::ScanFailure,
            "diskSpaceLow" => Self::DiskSpaceLow,
            "usageMilestone" => Self::UsageMilestone,
            "menuBarHome" => Self::MenuBarHome,
            "test" => Self::Test,
            _ => return None,
        })
    }
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

/// The three layers of copy every notification displays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationCopy {
    pub title: String,
    pub subtitle: String,
    pub description: String,
}

impl NotificationCopy {
    fn new(
        title: impl Into<String>,
        subtitle: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            subtitle: subtitle.into(),
            description: description.into(),
        }
    }
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
/// Every nudge links to notification settings and carries a dismiss CTA.
/// `extra_action` is the optional destination for the notification subject.
fn deliver(
    app: &AppHandle,
    kind: Kind,
    copy: NotificationCopy,
    extra_action: Option<(&str, &str)>,
    tone_override: Option<NudgeTone>,
) {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let (nudge_kind, default_tone) = match kind {
        Kind::UpdateAvailable => (NudgeKind::UpdateAvailable, NudgeTone::Info),
        Kind::ScanFailure => (NudgeKind::ScanFailure, NudgeTone::Warning),
        Kind::DiskSpaceLow => (NudgeKind::DiskSpaceLow, NudgeTone::Warning),
        Kind::UsageMilestone => (NudgeKind::UsageMilestone, NudgeTone::Info),
        Kind::MenuBarHome => (NudgeKind::MenuBarLocation, NudgeTone::Success),
        Kind::Test => (NudgeKind::Test, NudgeTone::Info),
    };
    let tone = tone_override.unwrap_or(default_tone);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut nudge = Nudge::new(
        format!("antiburn-{sequence}"),
        nudge_kind,
        tone,
        copy.title,
        copy.subtitle,
        copy.description,
    )
    .action(NOTIFICATION_SETTINGS_ACTION_ID, "Settings", false);
    if let Some((id, label)) = extra_action {
        nudge = nudge.action(id, label, true);
    }
    nudge = nudge.action("dismiss", "Dismiss", extra_action.is_none());
    crate::nudges::deliver(app, nudge);
}

/// Deliberately does not say "ready to install". This build checks the release
/// feed and reports what it found; downloading and installing an update is the
/// release milestone's work and has no UI yet (`docs/deviations.md`). A
/// notification that promised an install button would be describing an app
/// nobody has shipped.
pub fn update_message(version: &str) -> NotificationCopy {
    NotificationCopy::new(
        "antiburn update released",
        format!("New version {version} is available."),
        "This build checks the release feed but does not install updates yet.",
    )
}

/// The error is the store's or the engine's own words. It names a path or a
/// failure at worst — the scan never handles transcript text, so there is none
/// to leak into a notification.
pub fn scan_failure_message(error: &str) -> NotificationCopy {
    NotificationCopy::new(
        "Scan failed",
        format!("{error}."),
        "Check which folders are being scanned, and whether antiburn has access to them. Everything already indexed is unaffected.",
    )
}

pub fn disk_space_low_message(free_gb: u64, threshold_gb: u32) -> NotificationCopy {
    NotificationCopy::new(
        format!("{free_gb}GB of disk space left"),
        format!("Free space dropped below your {threshold_gb}GB warning threshold."),
        "Agents working in multiple worktrees can use up a lot of space, so antiburn monitors that.",
    )
}

pub fn usage_milestone_message(
    content: &crate::provider_usage::live::MilestoneContent,
) -> NotificationCopy {
    let crossing = &content.crossing;
    let used = crossing.used_percent.round().clamp(0.0, 100.0) as u8;
    let elapsed = crossing.elapsed_percent.round().clamp(0.0, 100.0) as u8;
    let title = if used > elapsed {
        format!(
            "Burn warning: {} {}",
            crate::provider_usage::providers::display_name(&content.provider),
            crossing.window_label
        )
    } else {
        format!(
            "Burn milestone: {} {}",
            crate::provider_usage::providers::display_name(&content.provider),
            crossing.window_label
        )
    };
    let description = if used > elapsed {
        format!(
            "You might hit your limits if you don't slow down. Your burn is faster than a straight line estimate - currently {}% ahead.",
            used - elapsed
        )
    } else {
        format!(
            "All looks fine, you're burning slower than a straight line estimate - currently {}% into safety.",
            elapsed - used
        )
    };
    NotificationCopy::new(
        title,
        format!("{used}% used in {elapsed}% of the usage window."),
        description,
    )
}

fn usage_milestone_tone(content: &crate::provider_usage::live::MilestoneContent) -> NudgeTone {
    let crossing = &content.crossing;
    if crossing.used_percent.round() > crossing.elapsed_percent.round() {
        NudgeTone::Warning
    } else {
        NudgeTone::Info
    }
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

/// Deliberately no direction — no "above", no "up there". This notice is
/// normally anchored right under the menu-bar item, but the anchor is
/// macOS-only and needs a tray rectangle the backend will not always report; on
/// the fallback path it appears at the platform's notification corner instead,
/// and any wording that pointed somewhere would be wrong exactly there. A test
/// below pins that.
pub fn menu_bar_home_message() -> NotificationCopy {
    NotificationCopy::new(
        format!("antiburn is in your {HOME_NOUN}"),
        "Click it to see your limits and details of your coding sessions.",
        "antiburn runs in the background after onboarding closes.",
    )
}

pub fn test_message() -> NotificationCopy {
    NotificationCopy::new(
        "antiburn notifications are working",
        "This sample uses your current position and timing settings.",
        "The notification sound also follows your current choice.",
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
    deliver(
        app,
        Kind::ScanFailure,
        scan_failure_message(error),
        Some(("review_sources", "Review sources")),
        None,
    );
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
    deliver(
        app,
        Kind::UpdateAvailable,
        update_message(version),
        Some(("view", "View")),
        None,
    );
}

/// Report low disk space. Returns whether the episode was consumed: `true`
/// when delivered, `false` when suppressed by preference so the caller's
/// trigger retries on its next tick (see [`crate::disk_monitor`]).
pub fn note_disk_space_low(app: &AppHandle, free_gb: u64, threshold_gb: u32) -> bool {
    if !enabled(app, Kind::DiskSpaceLow) {
        return false;
    }
    deliver(
        app,
        Kind::DiskSpaceLow,
        disk_space_low_message(free_gb, threshold_gb),
        None,
        None,
    );
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
    deliver(
        app,
        Kind::UsageMilestone,
        usage_milestone_message(content),
        None,
        Some(usage_milestone_tone(content)),
    );
    true
}

/// Say where antiburn went as the current setup window closes.
///
/// Ungated (see [`allowed`]) and forced to hang off the menu-bar item whatever
/// the reader's placement preference says: a notification that answers "where
/// is it" by appearing in the opposite corner of the screen from the answer
/// would be worse than none. Called only from [`crate::onboarding::finish`], so
/// it appears after each setup run, including an explicit restart.
pub fn note_menu_bar_home(app: &AppHandle) {
    crate::nudges::anchor_next_to_the_tray(app);
    deliver(
        app,
        Kind::MenuBarHome,
        menu_bar_home_message(),
        Some(("show", "Show me")),
        None,
    );
}

/// Post the settings pane's test notification. Bypasses the master switch —
/// the reader pressed the button — but is otherwise the same delivery path as
/// every real kind, so what they see is what they will get.
pub fn note_test(app: &AppHandle) {
    deliver(app, Kind::Test, test_message(), None, None);
}

/// Post a sample of `kind` with representative figures, for copy work.
///
/// Debug builds only, from the settings pane's debug row. This skips every
/// gate — the preferences, the once-per-run claims, the milestone ledger — on
/// purpose: the reader wants to see the card, not earn it. The figures are
/// fixed so the same wording shows on every press.
pub fn note_sample(app: &AppHandle, kind: Kind) {
    use crate::provider_usage::live::milestones::{MilestoneContent, MilestoneCrossing};

    let (copy, extra_action, tone) = match kind {
        Kind::UpdateAvailable => (update_message("0.2.0"), Some(("view", "View")), None),
        Kind::ScanFailure => (
            scan_failure_message("Could not read ~/.claude/projects"),
            Some(("review_sources", "Review sources")),
            None,
        ),
        Kind::DiskSpaceLow => (disk_space_low_message(18, 25), None, None),
        Kind::UsageMilestone => {
            let content = MilestoneContent {
                provider: "anthropic".to_string(),
                crossing: MilestoneCrossing {
                    window_label: "weekly limit".to_string(),
                    threshold: 40,
                    used_percent: 42.0,
                    elapsed_percent: 20.0,
                    resets_at_epoch: 0,
                },
            };
            let tone = usage_milestone_tone(&content);
            (usage_milestone_message(&content), None, Some(tone))
        }
        Kind::MenuBarHome => {
            crate::nudges::anchor_next_to_the_tray(app);
            (menu_bar_home_message(), Some(("show", "Show me")), None)
        }
        Kind::Test => (test_message(), None, None),
    };
    deliver(app, kind, copy, extra_action, tone);
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
    fn every_kind_has_a_wire_id_and_unknown_ids_are_rejected() {
        for (id, kind) in [
            ("updateAvailable", Kind::UpdateAvailable),
            ("scanFailure", Kind::ScanFailure),
            ("diskSpaceLow", Kind::DiskSpaceLow),
            ("usageMilestone", Kind::UsageMilestone),
            ("menuBarHome", Kind::MenuBarHome),
            ("test", Kind::Test),
        ] {
            assert_eq!(Kind::from_id(id), Some(kind));
        }
        assert_eq!(Kind::from_id("anything-else"), None);
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
            Kind::UsageMilestone,
        ] {
            assert!(!allowed(&settings, kind));
        }
    }

    #[test]
    fn an_empty_milestone_selection_is_how_milestones_are_off() {
        let settings = AppSettings {
            milestones_5h: crate::store::Milestones::none(),
            milestones_weekly: crate::store::Milestones::none(),
            ..settings()
        };
        assert!(!allowed(&settings, Kind::UsageMilestone));

        let settings = AppSettings {
            milestones_weekly: crate::store::Milestones::none(),
            ..AppSettings::default()
        };
        // One row still selected is still a preference to hear about it.
        assert!(allowed(&settings, Kind::UsageMilestone));
    }

    #[test]
    fn a_milestone_warns_only_when_quota_is_ahead_of_elapsed_time() {
        use crate::provider_usage::live::milestones::{MilestoneContent, MilestoneCrossing};

        let mut content = MilestoneContent {
            provider: "anthropic".to_string(),
            crossing: MilestoneCrossing {
                window_label: "5-hour limit".to_string(),
                threshold: 40,
                used_percent: 40.0,
                elapsed_percent: 20.0,
                resets_at_epoch: 0,
            },
        };
        assert_eq!(usage_milestone_tone(&content), NudgeTone::Warning);

        content.crossing.used_percent = 20.0;
        content.crossing.elapsed_percent = 40.0;
        assert_eq!(usage_milestone_tone(&content), NudgeTone::Info);
    }

    #[test]
    fn the_disk_kind_honors_its_own_switch() {
        let settings = AppSettings {
            notify_disk_space_low: false,
            ..settings()
        };
        assert!(!allowed(&settings, Kind::DiskSpaceLow));
        assert!(allowed(&settings, Kind::ScanFailure));
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
}
