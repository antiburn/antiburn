// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Tauri commands the notification webview invokes. Kept in their own module (not
//! at the crate root) so the `#[tauri::command]` helper macros don't collide with
//! their own re-exports under edition-2024 macro hygiene.
//!
//! The app registers each of these in its `generate_handler!` under the
//! `antiburn_nudge::commands::` path (`nudge_action`, `nudge_dismiss`,
//! `nudge_reveal`, `nudge_resize`, `nudge_ready`, `nudge_set_hovered`).

use tauri::State;

use crate::NudgeManager;
use crate::model::{NudgeActionEvent, NudgeActionTarget, NudgeKind};

/// Invoked by the notification webview when a CTA is clicked.
#[tauri::command]
pub fn nudge_action(
    manager: State<'_, NudgeManager>,
    kind: NudgeKind,
    action_id: String,
    target: Option<NudgeActionTarget>,
) {
    manager.handle_action(NudgeActionEvent {
        kind,
        action_id,
        target,
    });
}

/// Invoked by the notification webview to dismiss without acting (close button /
/// timeout).
#[tauri::command]
pub fn nudge_dismiss(manager: State<'_, NudgeManager>) {
    manager.dismiss();
}

/// Invoked by the notification webview once it has measured its content
/// `height`, so the crate can size + show the window at its final size instead
/// of resizing on screen.
#[tauri::command]
pub fn nudge_reveal(manager: State<'_, NudgeManager>, height: f64) {
    manager.reveal(height);
}

/// Invoked by the notification webview when it remeasures after
/// expanding/collapsing on hover, so the crate can animate the already-visible
/// window to the new height.
#[tauri::command]
pub fn nudge_resize(manager: State<'_, NudgeManager>, height: f64) {
    manager.resize(height);
}

/// Invoked by the notification webview once its `nudge:show` listener is
/// attached, so a nudge emitted before the webview was ready (e.g. right after
/// creation) is re-delivered instead of silently lost.
#[tauri::command]
pub fn nudge_ready(manager: State<'_, NudgeManager>) {
    manager.on_nudge_ready();
}

/// Invoked by the notification webview's `onMouseEnter`/`onMouseLeave` hover
/// handlers, so the mechanism can acquire key-window status only once the
/// user's cursor has genuinely entered the notification (see
/// `NudgeManager::set_hovered`'s doc for why that timing matters).
#[tauri::command]
pub fn nudge_set_hovered(manager: State<'_, NudgeManager>, hovered: bool) {
    manager.set_hovered(hovered);
}
