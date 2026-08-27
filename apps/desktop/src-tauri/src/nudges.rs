//! Glue between notification policy and the notification window.
//!
//! [`crate::notifications`] decides *whether* something is worth saying;
//! the `antiburn-nudge` crate owns the window mechanics; this module is the
//! seam between them: it applies the presentation preferences (placement,
//! auto-dismiss, the chime) and routes a clicked CTA back into the app.
//! Nothing here decides policy — every gating question stays in
//! `notifications.rs`, which is what keeps "what may interrupt a reader"
//! reviewable in one file.

use std::sync::atomic::{AtomicBool, Ordering};

use antiburn_nudge::{
    Nudge, NudgeActionEvent, NudgeActionTarget, NudgeKind, NudgeManager, NudgePlacement,
};
use tauri::{AppHandle, Manager};

use crate::store::{NudgePlacement as PlacementPref, Store};

/// One notification's licence to ignore the placement preference.
///
/// The crate evaluates a single placement closure at reveal time, so there is
/// no per-nudge placement argument to pass. Rather than widen that seam for one
/// caller, this is a one-shot flag the closure consults: set it, deliver, and
/// the very next reveal hangs off the menu-bar item.
///
/// **Taken, not read.** A nudge that is replaced before it reveals — or one
/// whose window never comes up at all — must not leave the flag set for
/// whatever fires next, which could be a disk warning hours later.
#[derive(Default)]
pub struct AnchorOverride(AtomicBool);

impl AnchorOverride {
    fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Read and clear.
    fn take(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }
}

/// Make the next nudge hang off the menu-bar item, whatever the reader's
/// placement preference says. See [`crate::notifications::note_menu_bar_home`]
/// for the one thing that needs this and why.
pub fn anchor_next_to_the_tray(app: &AppHandle) {
    if let Some(override_) = app.try_state::<AnchorOverride>() {
        override_.set();
    }
}

/// Build the manager and the chime player, once, at setup.
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let action_app = app.clone();
    let placement_app = app.clone();
    let manager = NudgeManager::with_placement(
        app,
        move |event| on_action(&action_app, event),
        move || placement(&placement_app),
    )?;
    app.manage(manager);
    app.manage(antiburn_sound::SoundPlayer::new());
    Ok(())
}

/// Deliver a nudge the policy layer already approved.
///
/// Applies the reader's auto-dismiss preference to every nudge (the builder's
/// own timeout is a fallback for an unreadable store), plays the chime when
/// the sound preference and the kind's own mapping both say so, then hands
/// the window the payload. Sound goes first: the audio thread returns
/// immediately, while showing marshals to the main thread.
pub fn deliver(app: &AppHandle, mut nudge: Nudge) {
    let settings = app
        .try_state::<Store>()
        .and_then(|store| store.settings().ok());

    if let Some(settings) = &settings {
        nudge.timeout_ms = Some(settings.nudge_auto_dismiss_secs.saturating_mul(1000));
    }

    let sound_allowed = settings
        .as_ref()
        .is_none_or(|settings| settings.notification_sound);
    if sound_allowed
        && let Some(kind) = sound_for(nudge.kind)
        && let Some(player) = app.try_state::<antiburn_sound::SoundPlayer>()
    {
        player.play(kind, nudge.actor.as_deref());
    }

    if let Some(manager) = app.try_state::<NudgeManager>() {
        manager.show(nudge);
    }
}

/// Which kinds carry the chime. The test plays it so the toggle is auditable.
/// Updates, scans, disk, and milestones stay quiet.
fn sound_for(kind: NudgeKind) -> Option<antiburn_sound::SoundKind> {
    match kind {
        NudgeKind::Test => Some(antiburn_sound::SoundKind::Notification),
        _ => None,
    }
}

/// Where the window appears, read fresh at reveal time so a placement change
/// applies to the very next nudge.
///
/// The override is consumed here whether or not it changes the answer: on a
/// platform where the anchor is unavailable, or with a tray rect the backend
/// will not report, the notification falls back to the native corner and the
/// flag is still spent. Leaving it set to "try again next time" would place an
/// unrelated notification under the menu bar much later.
fn placement(app: &AppHandle) -> NudgePlacement {
    let forced = app
        .try_state::<AnchorOverride>()
        .is_some_and(|override_| override_.take());
    let pref = app
        .try_state::<Store>()
        .and_then(|store| store.settings().ok())
        .map(|settings| settings.nudge_placement)
        .unwrap_or_default();
    if (forced || pref == PlacementPref::MenuBar)
        && let Some(rect) = app
            .tray_by_id("antiburn")
            .and_then(|tray| tray.rect().ok().flatten())
    {
        return NudgePlacement::MenuBarAnchor { rect };
    }
    NudgePlacement::NativeCorner
}

/// A clicked CTA. Dismissal is the crate's own affair; anything else lands on
/// the settings pane that can act on the nudge's subject — except the one CTA
/// whose subject is the menu-bar item itself.
fn on_action(app: &AppHandle, event: NudgeActionEvent) {
    if event.action_id == "dismiss" {
        return;
    }
    if event.action_id == crate::notifications::NOTIFICATION_SETTINGS_ACTION_ID {
        let _ = crate::settings::open(app, Some("notifications".to_string()));
        return;
    }
    // "Show me" opens the popover under the glyph the notification is already
    // pointing at, so the reader's first sight of it is the thing the icon
    // does rather than a settings pane about it.
    if event.kind == NudgeKind::MenuBarLocation {
        match app
            .tray_by_id("antiburn")
            .and_then(|tray| tray.rect().ok().flatten())
        {
            Some(rect) => crate::popover::toggle(app, rect),
            None => open_without_a_tray_rect(app),
        }
        return;
    }
    if event.kind == NudgeKind::UpdateAvailable {
        let expected_version = match classify_update_action(&event) {
            UpdateAction::OpenAbout => {
                if event.action_id == "install" {
                    tracing::warn!("the update install action has no valid version target");
                }
                None
            }
            UpdateAction::Install(expected_version) => Some(expected_version.to_string()),
        };
        let _ = crate::settings::open(app, Some("about".to_string()));
        if let Some(expected_version) = expected_version {
            let install_app = app.clone();
            tauri::async_runtime::spawn(async move {
                crate::updates::install(&install_app, &expected_version).await;
            });
        }
        return;
    }
    let pane = match event.kind {
        NudgeKind::ScanFailure => Some("sources"),
        NudgeKind::DiskSpaceLow | NudgeKind::UsageMilestone => Some("notifications"),
        // Test, and anything the crate's non-exhaustive enum grows later: a
        // CTA with no better home still lands somewhere real.
        _ => None,
    };
    let _ = crate::settings::open(app, pane.map(str::to_string));
}

#[derive(Debug, PartialEq, Eq)]
enum UpdateAction<'a> {
    OpenAbout,
    Install(&'a str),
}

fn classify_update_action(event: &NudgeActionEvent) -> UpdateAction<'_> {
    if event.kind != NudgeKind::UpdateAvailable || event.action_id != "install" {
        return UpdateAction::OpenAbout;
    }
    let Some(NudgeActionTarget::Update { expected_version }) = event.target.as_ref() else {
        return UpdateAction::OpenAbout;
    };
    if expected_version.trim().is_empty() {
        return UpdateAction::OpenAbout;
    }
    UpdateAction::Install(expected_version)
}

/// The same CTA where the tray backend reports no rectangle.
///
/// On Linux that is every time: the AppIndicator backend has no item geometry
/// to give, so the button would do nothing at all. The tray menu's own open
/// path already knows how to make an anchor, so send the reader through it.
#[cfg(target_os = "linux")]
fn open_without_a_tray_rect(app: &AppHandle) {
    crate::popover::open_from_tray_menu(app);
}

/// macOS and Windows do report a rectangle, so a missing one means the tray
/// item is not there yet. Doing nothing is what this CTA already did.
#[cfg(not(target_os = "linux"))]
fn open_without_a_tray_rect(_app: &AppHandle) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_anchor_override_is_spent_by_the_first_reveal_that_reads_it() {
        let override_ = AnchorOverride::default();
        assert!(!override_.take(), "nothing has asked for the anchor");

        override_.set();
        assert!(override_.take());
        // The next notification is somebody else's — a disk warning, an
        // update — and must land wherever the reader's preference says.
        assert!(!override_.take());
    }

    #[test]
    fn only_the_exact_update_install_action_is_classified_for_install() {
        let exact = NudgeActionEvent {
            kind: NudgeKind::UpdateAvailable,
            action_id: "install".to_string(),
            target: Some(NudgeActionTarget::Update {
                expected_version: "2.4.0-beta.1+build.7".to_string(),
            }),
        };
        assert_eq!(
            classify_update_action(&exact),
            UpdateAction::Install("2.4.0-beta.1+build.7")
        );

        for malformed in [
            NudgeActionEvent {
                action_id: "open_app".to_string(),
                ..exact.clone()
            },
            NudgeActionEvent {
                target: None,
                ..exact.clone()
            },
            NudgeActionEvent {
                target: Some(NudgeActionTarget::ProviderUsage {
                    provider: "openai".to_string(),
                    account_key: None,
                }),
                ..exact.clone()
            },
            NudgeActionEvent {
                target: Some(NudgeActionTarget::Update {
                    expected_version: "  ".to_string(),
                }),
                ..exact.clone()
            },
            NudgeActionEvent {
                kind: NudgeKind::ScanFailure,
                ..exact.clone()
            },
        ] {
            assert_eq!(classify_update_action(&malformed), UpdateAction::OpenAbout);
        }
    }
}
