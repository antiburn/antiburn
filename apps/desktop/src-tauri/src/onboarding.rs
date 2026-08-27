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
//! The first-run flow uses a window instead of a bounded-height popover surface.
//!
//! Chrome follows [`crate::settings`] rather than inventing a second pattern:
//! fixed size, non-resizable, and on macOS an overlay title bar with the
//! floating title hidden, so the frontend paints its own
//! `data-tauri-drag-region` strip (`src/views/onboarding/OnboardingFlow.tsx`).

use std::sync::Mutex;
use std::time::Instant;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::window_lifecycle::{self, ManagedWindowReadiness};
use crate::window_placement::center_on_active_monitor;
use crate::window_readiness::{OpenAction, WindowReadiness, renderer_generation_script};

/// Window label. Also listed in `capabilities/default.json`.
pub const LABEL: &str = "onboarding";

/// Dedicated frontend entry for the onboarding window.
const URL: &str = "onboarding.html";

/// Fixed geometry. 480 tall leaves ~379pt of body under the 44pt header and
/// over the 57pt footer, which is more than the tallest step needs; 680 wide is
/// what stops the permission notice wrapping and the paths truncating.
/// Non-resizable, like Settings — every step is designed for this rectangle.
const WIDTH: f64 = 680.0;
const HEIGHT: f64 = 480.0;

/// Give the final settings command time to return before its caller's webview
/// is destroyed. The window is hidden immediately, so this delay is invisible.
const FINISH_TEARDOWN_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

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

/// Renderer lifecycle for the onboarding window.
#[derive(Default)]
pub struct OnboardingWindowState(Mutex<WindowReadiness>);

impl ManagedWindowReadiness for OnboardingWindowState {
    fn readiness(&self) -> std::sync::MutexGuard<'_, WindowReadiness> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Start setup again at Welcome and give it the first-run app presence.
pub fn restart(app: &AppHandle) -> tauri::Result<()> {
    apply_activation_policy(app, true);
    let Some(existing) = app.get_webview_window(LABEL) else {
        return open(app);
    };

    let generation = {
        let state = app.state::<OnboardingWindowState>();
        let mut readiness = state.readiness();
        readiness.reset();
        let OpenAction::StartLoading { generation } = readiness.request_open(Instant::now()) else {
            unreachable!("an idle lifecycle starts loading")
        };
        let deferred = readiness.defer_build_until_destroyed(generation);
        debug_assert!(deferred, "the replacement generation must remain active");
        generation
    };
    if let Err(error) = existing.destroy() {
        window_lifecycle::cancel_load::<OnboardingWindowState>(app, generation);
        return Err(error);
    }
    Ok(())
}

/// Shows the onboarding window, creating it if this is the first request.
///
/// Called twice over a first run's life: once from setup, and again if the
/// reader closes the window before finishing and then clicks the menu-bar item
/// ([`crate::popover::toggle`]). The second path is why this reuses an existing
/// window rather than assuming it is the only caller — a close hides rather
/// than destroys (see `crate::on_window_event`), so the flow comes back with
/// the steps the reader already walked still behind it.
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    let state = app.state::<OnboardingWindowState>();
    let Some(existing) = app.get_webview_window(LABEL) else {
        let mut readiness = state.readiness();
        let action = readiness.request_open(Instant::now());
        let generation = match action {
            OpenAction::StartLoading { generation } | OpenAction::Rebuild { generation } => {
                generation
            }
            OpenAction::AwaitReady => return Ok(()),
            OpenAction::Reveal => {
                readiness.reset();
                match readiness.request_open(Instant::now()) {
                    OpenAction::StartLoading { generation } => generation,
                    _ => unreachable!("an idle lifecycle starts loading"),
                }
            }
        };
        drop(readiness);
        return build(app, generation);
    };

    let action = {
        let mut readiness = state.readiness();
        readiness.request_open(Instant::now())
    };
    match action {
        OpenAction::Reveal => show(&existing),
        OpenAction::AwaitReady => Ok(()),
        OpenAction::StartLoading { generation } | OpenAction::Rebuild { generation } => {
            if !state.readiness().defer_build_until_destroyed(generation) {
                return Ok(());
            }
            if let Err(error) = existing.destroy() {
                window_lifecycle::cancel_load::<OnboardingWindowState>(app, generation);
                return Err(error);
            }
            Ok(())
        }
    }
}

/// Build a deferred replacement after Tauri removes the old window label.
pub fn rebuild_after_destroy(app: &AppHandle) {
    let generation =
        window_lifecycle::begin_deferred_build::<OnboardingWindowState>(app, Instant::now());
    let Some(generation) = generation else {
        return;
    };
    if let Err(error) = build(app, generation) {
        ::tracing::error!(event = "window_rebuild_failed", window = LABEL, error = %error);
    }
}

fn build(app: &AppHandle, generation: u64) -> tauri::Result<()> {
    ::tracing::info!(
        event = "window_renderer_load_started",
        window = LABEL,
        generation
    );
    window_lifecycle::arm_stale_warning::<OnboardingWindowState>(app, generation, LABEL);

    // Built hidden and positioned before the first show, so the window never
    // visibly jumps from a default position to the right one. Deliberately no
    // `.center()`: the builder's centering computes against the primary
    // monitor before the window has a screen.
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(URL.into()))
        .initialization_script(renderer_generation_script(generation))
        .title("Set up antiburn")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .maximizable(false)
        .visible(false)
        .on_page_load(|window, payload| {
            window_lifecycle::trace_page_load::<OnboardingWindowState>(window, payload, LABEL);
        });

    #[cfg(target_os = "macos")]
    {
        // Overlay keeps decorations while making the title bar transparent;
        // `hidden_title` drops the floating title text. `.title(...)` above
        // stays so Mission Control and accessibility still name the window.
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }

    let window = match builder.build() {
        Ok(window) => window,
        Err(error) => {
            window_lifecycle::cancel_load::<OnboardingWindowState>(app, generation);
            return Err(error);
        }
    };
    center_on_active_monitor(&window, WIDTH, HEIGHT);
    Ok(())
}

/// Reveal onboarding after React commits its shell.
pub fn renderer_ready(window: &tauri::WebviewWindow, generation: u64) {
    let app = window.app_handle();
    if window_lifecycle::renderer_ready::<OnboardingWindowState>(
        app,
        LABEL,
        generation,
        Instant::now(),
    ) && let Err(error) = show(window)
    {
        ::tracing::error!(event = "window_reveal_failed", window = LABEL, error = %error);
    }
}

fn show(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    center_on_active_monitor(window, WIDTH, HEIGHT);
    window.show()?;
    window.unminimize()?;
    // This window opens without a click behind it. Focus activates the regular
    // macOS application and keeps the flow reachable from the Dock.
    window.set_focus()?;
    ::tracing::info!(event = "window_revealed", window = LABEL);
    Ok(())
}

/// Put the window away and point the reader at where the app now lives.
///
/// Called when the current setup run changes from pending to complete. The
/// order is deliberate: the window goes first, so
/// the notification arrives into the gap it leaves rather than on top of it —
/// "where did that window go" is the question being answered, and it is asked
/// after the window is gone.
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
    crate::settings::schedule_prewarm(app);

    // `finish` runs inside the onboarding webview's final `set_settings` IPC.
    // Destroying that webview synchronously can cut off the command response;
    // hide now, then retire it once the response and location nudge are away.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FINISH_TEARDOWN_DELAY).await;
        let check_app = app.clone();
        let _ = app.run_on_main_thread(move || {
            if is_pending(&check_app) {
                return;
            }
            if let Some(window) = check_app.get_webview_window(LABEL) {
                let _ = window.destroy();
            }
        });
    });
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
            ::tracing::warn!(event = "activation_policy_set_failed", error = %error);
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

    /// The shell must open the entry that mounts the first-run view.
    #[test]
    fn the_url_uses_the_onboarding_entry() {
        assert_eq!(URL, "onboarding.html");
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
