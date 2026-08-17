// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The standalone first-run window.
//!
//! Onboarding used to be a surface of the popover, sized at 380×520. Two things
//! were wrong with that and both are why this module exists.
//!
//! The popover is created hidden and shown only by a click on the menu-bar
//! item, so a fresh install booted into the menu bar and waited — silently —
//! for the reader to find a 16pt template glyph. The flow whose job is to
//! establish trust sat behind the discovery problem it should have been
//! solving. This window opens itself at launch instead (see
//! [`crate::run`]'s setup).
//!
//! And 380pt is the wrong room for the work. The Repositories step stacks a
//! folder-permission notice — whose three buttons wrap to two rows at that
//! width — above a list of ~60pt repository rows, which left two or three of
//! them visible at the one moment the reader is deciding what antiburn may
//! read. 680×480 gives the same step about 130pt of list under a notice that no
//! longer wraps, and every other step more room than it had.
//!
//! Recorded as a deviation (`docs/deviations.md`, D-25): the ratified row is a
//! per-view bounded-height popover surface, and this is a window.
//!
//! Chrome follows [`crate::settings`] rather than inventing a second pattern:
//! fixed size, non-resizable, and on macOS an overlay title bar with the
//! floating title hidden, so the frontend paints its own
//! `data-tauri-drag-region` strip (`src/views/onboarding/OnboardingFlow.tsx`).

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::window_placement::center_on_active_monitor;

/// Window label. Also listed in `capabilities/default.json`.
pub const LABEL: &str = "onboarding";

/// Fragment the onboarding window loads. The frontend serves one bundle and
/// selects its view from this fragment (see `src/lib/route.ts`).
const URL: &str = "index.html#/onboarding";

/// Fixed geometry. 480 tall leaves ~379pt of body under the 44pt header and
/// over the 57pt footer, which is more than the tallest step needs; 680 wide is
/// what stops the permission notice wrapping and the paths truncating.
/// Non-resizable, like Settings — every step is designed for this rectangle.
const WIDTH: f64 = 680.0;
const HEIGHT: f64 = 480.0;

// The reasons for those two numbers, as build errors rather than tests —
// following `popover.rs`, and for the same reason: a geometry constant edited
// without its rationale should fail to compile, not fail a suite somebody can
// skip.
//
// Wider than the popover it replaced, or the step that motivated the move is
// no better off than it was. Small enough to sit on a 1280×800 display with
// room around it, or the window is worse placed than the popover was.
const _: () = assert!(WIDTH > 380.0);
const _: () = assert!(WIDTH < 1280.0 && HEIGHT < 800.0);
// Enough body under the 44pt header and over the 57pt footer for the
// Repositories step: ~150pt of chrome, ~110pt of permission notice, and at
// least one ~60pt repository row after them. Anything less ships a list with
// nothing visible in it.
const _: () = assert!(HEIGHT - 44.0 - 57.0 > 150.0 + 110.0 + 60.0);

/// Shows the onboarding window, creating it if this is the first request.
///
/// Called twice over a first run's life: once from setup, and again if the
/// reader closes the window before finishing and then clicks the menu-bar item
/// ([`crate::popover::toggle`]). The second path is why this reuses an existing
/// window rather than assuming it is the only caller — a close hides rather
/// than destroys (see `crate::on_window_event`), so the flow comes back with
/// the steps the reader already walked still behind it.
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    if let Some(existing) = app.get_webview_window(LABEL) {
        // Re-centered on every open for the same reason Settings is: the window
        // hides rather than being destroyed, so without this a reader who
        // works across monitors gets it back on whichever display they last
        // dismissed it on.
        center_on_active_monitor(&existing, WIDTH, HEIGHT);
        existing.show()?;
        existing.unminimize()?;
        existing.set_focus()?;
        return Ok(());
    }

    // Built hidden and positioned before the first show, so the window never
    // visibly jumps from a default position to the right one. Deliberately no
    // `.center()`: the builder's centering computes against the primary
    // monitor before the window has a screen.
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(URL.into()))
        .title("Set up antiburn")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .maximizable(false)
        .visible(false);

    #[cfg(target_os = "macos")]
    {
        // Overlay keeps decorations while making the title bar transparent;
        // `hidden_title` drops the floating title text. `.title(...)` above
        // stays so Mission Control and accessibility still name the window.
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }

    let window = builder.build()?;
    center_on_active_monitor(&window, WIDTH, HEIGHT);
    window.show()?;
    // This is the one window that opens without a click behind it, so nothing
    // else is going to bring it forward. On macOS `set_focus` activates the
    // application as well as ordering the window front — and by here the app is
    // already `Regular` (see [`apply_activation_policy`], applied before this
    // is called), so activating it means a Dock icon and a ⌘-Tab entry, not
    // just a window that happens to be on top.
    window.set_focus()?;

    Ok(())
}

/// Put the window away and point the reader at where the app now lives.
///
/// Called from [`crate::commands::set_settings`] on the one transition that
/// means the flow is over. The order is deliberate: the window goes first, so
/// the notification arrives into the gap it leaves rather than on top of it —
/// "where did that window go" is the question being answered, and it is only
/// asked once the window is gone.
pub fn finish(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
    }
    // The Dock icon goes at the same moment the notification says where the
    // app went, which is the whole choreography: one presence is exchanged for
    // the other in front of the reader rather than behind their back. The nudge
    // is non-activating, so it survives the policy change instead of being
    // ordered out by it — hence this line before that one, not after.
    apply_activation_policy(app, false);
    crate::notifications::note_menu_bar_home(app);
}

/// Which kind of application antiburn is while the first run is pending.
///
/// The accessory policy — no Dock icon, no ⌘-Tab entry, no application menu —
/// is right for a finished menu-bar app and a trap for this one. A reader on
/// step two who clicks another application has no route back: not the Dock, not
/// ⌘-Tab, not Mission Control, only the menu-bar glyph they have not been told
/// about yet, because being told about it is what finishing this window *does*.
///
/// So antiburn is an ordinary application for exactly as long as it owes
/// somebody the flow, and an accessory afterwards.
///
/// Pure, so the rule is testable without AppKit.
#[cfg(target_os = "macos")]
pub fn policy_for(pending: bool) -> tauri::ActivationPolicy {
    if pending {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    }
}

/// Apply [`policy_for`].
///
/// Keyed on whether onboarding is *pending*, deliberately, and never on whether
/// the window is visible: closing the window early hides rather than destroys
/// it, and a visibility-keyed rule would take the Dock icon away at exactly the
/// moment the reader most needs it to get back.
///
/// A no-op off macOS, where the window carries no `skip_taskbar` and is
/// therefore already in the taskbar and already alt-tabbable.
pub fn apply_activation_policy(app: &AppHandle, pending: bool) {
    #[cfg(target_os = "macos")]
    {
        if let Err(error) = app.set_activation_policy(policy_for(pending)) {
            // Best-effort: the wrong Dock presence is a worse first run, not a
            // broken one, and there is nothing useful to do about it here.
            eprintln!("antiburn: could not set the activation policy ({error})");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, pending);
}

/// Whether the first-run flow still owes the reader something.
///
/// An unreadable store answers `false`: the flow's whole job is the *first*
/// run, and re-running it because a read failed would be worse than skipping
/// it. Every caller has a working fallback for that answer — the popover opens
/// normally, and setup simply shows no window.
pub fn is_pending(app: &AppHandle) -> bool {
    app.try_state::<crate::store::Store>()
        .and_then(|store| store.settings().ok())
        .is_some_and(|settings| !settings.onboarding_completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frontend has to be opened with the fragment its router recognizes,
    /// and the two live in different languages — a rename on either side is
    /// silent until a window renders the wrong view.
    #[test]
    fn the_url_carries_the_fragment_the_frontend_routes_on() {
        assert!(
            URL.ends_with("#/onboarding"),
            "`src/lib/route.ts` maps this fragment to the first-run view"
        );
        assert_eq!(
            LABEL, "onboarding",
            "also listed in capabilities/default.json"
        );
    }

    /// Small, but it is the whole rule: antiburn is an ordinary application
    /// for as long as it owes somebody the first run, and an accessory after.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_app_has_a_dock_icon_for_exactly_as_long_as_the_flow_is_owed() {
        assert!(matches!(policy_for(true), tauri::ActivationPolicy::Regular));
        assert!(matches!(
            policy_for(false),
            tauri::ActivationPolicy::Accessory
        ));
    }
}
