// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Glue between notification policy and the notification window.
//!
//! [`crate::notifications`] decides *whether* something is worth saying;
//! the `antiburn-nudge` crate owns the window mechanics; this module is the
//! seam between them: it applies the presentation preferences (placement,
//! auto-dismiss, the chime) and routes a clicked CTA back into the app.
//! Nothing here decides policy — every gating question stays in
//! `notifications.rs`, which is what keeps "what may interrupt a reader"
//! reviewable in one file.

use antiburn_nudge::{Nudge, NudgeActionEvent, NudgeKind, NudgeManager, NudgePlacement};
use tauri::{AppHandle, Manager};

use crate::store::{NudgePlacement as PlacementPref, Store};

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

/// Which kinds carry the chime. Deliberately short — a sound that plays for
/// everything is a sound people silence: the test plays it (so the toggle is
/// auditable), and the anomaly plays it (the one kind meant to catch someone
/// mid-flow). Updates, scans, disk, and milestones stay quiet.
fn sound_for(kind: NudgeKind) -> Option<antiburn_sound::SoundKind> {
    match kind {
        NudgeKind::Test | NudgeKind::UsageAnomaly => Some(antiburn_sound::SoundKind::Notification),
        _ => None,
    }
}

/// Where the window appears, read fresh at reveal time so a placement change
/// applies to the very next nudge.
fn placement(app: &AppHandle) -> NudgePlacement {
    let pref = app
        .try_state::<Store>()
        .and_then(|store| store.settings().ok())
        .map(|settings| settings.nudge_placement)
        .unwrap_or_default();
    if pref == PlacementPref::MenuBar
        && let Some(rect) = app
            .tray_by_id("antiburn")
            .and_then(|tray| tray.rect().ok().flatten())
    {
        return NudgePlacement::MenuBarAnchor { rect };
    }
    NudgePlacement::NativeCorner
}

/// A clicked CTA. Dismissal is the crate's own affair; anything else lands on
/// the settings pane that can act on the nudge's subject.
fn on_action(app: &AppHandle, event: NudgeActionEvent) {
    if event.action_id == "dismiss" {
        return;
    }
    let pane = match event.kind {
        // Software update lives inside About (with the build it updates).
        NudgeKind::UpdateAvailable => Some("about"),
        NudgeKind::ScanFailure => Some("general"),
        NudgeKind::DiskSpaceLow | NudgeKind::UsageAnomaly | NudgeKind::UsageMilestone => {
            Some("notifications")
        }
        // Test, and anything the crate's non-exhaustive enum grows later: a
        // CTA with no better home still lands somewhere real.
        _ => None,
    };
    let _ = crate::settings::open(app, pane.map(str::to_string));
}
