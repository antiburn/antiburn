//! `antiburn-nudge` — a self-contained, cross-platform floating **notification**
//! for the antiburn desktop app.
//!
//! This crate is the **mechanism**: it owns the notification window, positions it
//! at the OS-native corner, shows it without stealing focus, pushes a [`Nudge`]
//! payload to the notification webview, and reports which CTA was clicked back to
//! the app via a caller-supplied callback. It knows nothing about *why* a nudge
//! fires (a failed scan, a usage milestone, …), about user preferences, or about
//! the app's navigation — that is the app's **policy**.
//!
//! The seam between crate and app is intentionally tiny:
//! - [`NudgeManager::new`] — created once at app setup with an `on_action`
//!   callback (the only app-specific behavior injected in).
//! - [`NudgeManager::show`] / [`NudgeManager::dismiss`].
//! - the [`commands::nudge_action`] / [`commands::nudge_dismiss`] commands the
//!   notification webview calls.
//!
//! The crate↔frontend contract is two strings: the window label [`NUDGE_LABEL`]
//! and the [`NUDGE_SHOW_EVENT`] event carrying a serialized [`Nudge`].
//!
//! Types use the default `Wry` runtime — this is an in-repo crate for the
//! antiburn app, so there's no need to be generic over the runtime.

pub mod commands;
mod gate;
mod geometry;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
pub mod model;
#[cfg(windows)]
mod win;
mod window;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

pub use gate::{NotificationGate, NotificationGateState};
pub use model::{Nudge, NudgeAction, NudgeActionEvent, NudgeActionTarget, NudgeKind, NudgeTone};

/// Register the platform support this crate needs while building the Tauri app.
///
/// On macOS this installs the `tauri-nspanel` plugin so the notification window
/// can be subclassed into a non-activating floating `NSPanel` (it never steals
/// key focus). A no-op on other platforms. Call once, before `.run()`.
pub fn register<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    #[cfg(target_os = "macos")]
    {
        builder.plugin(tauri_nspanel::init())
    }
    #[cfg(not(target_os = "macos"))]
    {
        builder
    }
}

/// The notification window's label. The frontend renders the notification view
/// when its window has this label.
pub const NUDGE_LABEL: &str = "nudge";

/// Event emitted to the notification webview carrying a [`Nudge`] payload.
pub const NUDGE_SHOW_EVENT: &str = "nudge:show";

/// Event emitted to the notification webview carrying a `bool`: whether the
/// cursor is currently over the notification.
///
/// macOS only, and deliberately not a replacement for the webview's own
/// `mouseenter`/`mouseleave` — a backstop for the states in which those never
/// fire because AppKit is routing mouse-moved events to some other antiburn
/// window that holds key (see `macos::start_hover_watch`). The frontend treats
/// whichever signal arrives first as the hover edge and ignores the other.
pub const NUDGE_HOVER_EVENT: &str = "nudge:hover";

type ActionCallback = Arc<dyn Fn(NudgeActionEvent) + Send + Sync + 'static>;
type PlacementProvider = Arc<dyn Fn() -> NudgePlacement + Send + Sync + 'static>;

/// Leave enough time for a dismissing IPC command to return before destroying
/// the webview that sent it. A replacement nudge cancels the task.
const TEARDOWN_DELAY: Duration = Duration::from_secs(1);

#[derive(Default)]
struct TeardownGeneration(AtomicU64);

impl TeardownGeneration {
    fn advance(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn is_current(&self, generation: u64) -> bool {
        self.0.load(Ordering::SeqCst) == generation
    }

    fn current(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Where the notification should appear when it is revealed.
#[derive(Debug, Clone)]
pub enum NudgePlacement {
    /// Platform-native notification corner.
    NativeCorner,
    /// Anchor under the antiburn menu-bar item. Only used on macOS; other
    /// platforms fall back to [`Self::NativeCorner`].
    MenuBarAnchor { rect: tauri::Rect },
}

/// Owns the notification window and the app's on-action callback. Store one in
/// Tauri managed state at setup.
pub struct NudgeManager {
    app: AppHandle,
    on_action: ActionCallback,
    placement: PlacementProvider,
    /// The nudge currently meant to be on screen, if any. Retained so it can be
    /// re-delivered when the notification webview signals it is ready (its
    /// `listen()` may not have been attached when [`Self::show`] first emitted).
    /// Cleared on [`Self::dismiss`].
    pending: Mutex<Option<Nudge>>,
    /// Serializes window creation/reuse with the final teardown check and
    /// destruction. The generation remains the cheap stale-task filter; this
    /// gate closes the check-then-destroy race with a replacement `show`.
    lifecycle: Mutex<()>,
    teardown_generation: TeardownGeneration,
}

impl NudgeManager {
    /// Capture the app handle and the on-action callback. The (hidden)
    /// notification window itself is created lazily on first [`Self::show`].
    ///
    /// `on_action` is invoked with `{ kind, action_id }` when the user clicks a
    /// CTA; the notification is then dismissed automatically. The app implements
    /// focus/navigation there.
    pub fn new(
        app: &AppHandle,
        on_action: impl Fn(NudgeActionEvent) + Send + Sync + 'static,
    ) -> tauri::Result<Self> {
        Self::with_placement(app, on_action, || NudgePlacement::NativeCorner)
    }

    /// Capture the app handle, on-action callback, and placement provider. The
    /// provider is evaluated at reveal time so it can reflect the latest user
    /// setting and current tray rect.
    pub fn with_placement(
        app: &AppHandle,
        on_action: impl Fn(NudgeActionEvent) + Send + Sync + 'static,
        placement: impl Fn() -> NudgePlacement + Send + Sync + 'static,
    ) -> tauri::Result<Self> {
        Ok(Self {
            app: app.clone(),
            on_action: Arc::new(on_action),
            placement: Arc::new(placement),
            pending: Mutex::new(None),
            lifecycle: Mutex::new(()),
            teardown_generation: TeardownGeneration::default(),
        })
    }

    /// Present `nudge`: position the notification at the OS-native corner,
    /// reveal it without taking focus, and push the payload to the webview.
    /// Policy-free — the caller has already decided this should be shown.
    pub fn show(&self, nudge: Nudge) {
        let Ok(_lifecycle) = self.lifecycle.lock() else {
            return;
        };
        // Invalidate a dismiss task before it can retire the window this show
        // is about to reuse (or the new one it is about to create).
        self.teardown_generation.advance();
        let Ok(window) = window::get_or_create_nudge_window(&self.app) else {
            return;
        };
        // Hide first so the whole size/position/reveal cycle is invisible — even
        // when a new nudge replaces one that's still on screen. This is what keeps
        // the notification from visibly resizing (wobbling) on show. The
        // notification is *revealed* via [`Self::reveal`] only after the frontend
        // sizes it.
        window::hide(&window);
        // Retain the payload so it can be re-delivered if the notification
        // webview's listener wasn't attached yet (see [`Self::on_nudge_ready`]).
        if let Ok(mut pending) = self.pending.lock() {
            *pending = Some(nudge.clone());
        }
        let _ = self.app.emit_to(NUDGE_LABEL, NUDGE_SHOW_EVENT, &nudge);
    }

    /// Re-emit the pending nudge, if one is still meant to be on screen. Called
    /// by the `nudge_ready` command once the notification webview has attached
    /// its listener, so a nudge emitted before the webview was ready still
    /// appears. A no-op once the notification has been dismissed (pending is
    /// cleared then).
    pub fn on_nudge_ready(&self) {
        let pending = self
            .pending
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().cloned());
        if let Some(nudge) = pending {
            let _ = self.app.emit_to(NUDGE_LABEL, NUDGE_SHOW_EVENT, &nudge);
        }
    }

    /// Size the (hidden) notification to `height`, place it on the active
    /// monitor, and order it front without activating — sized and positioned
    /// before it's shown, so it never resizes on screen. Called by the
    /// `nudge_reveal` command once the frontend has measured its content.
    pub fn reveal(&self, height: f64) {
        let generation = self.teardown_generation.current();
        let _lifecycle = match self.lifecycle.try_lock() {
            Ok(lifecycle) => lifecycle,
            Err(TryLockError::WouldBlock) => {
                let app = self.app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let Some(manager) = app.try_state::<NudgeManager>() else {
                        return;
                    };
                    let Ok(_lifecycle) = manager.lifecycle.lock() else {
                        return;
                    };
                    if !manager.teardown_generation.is_current(generation) {
                        return;
                    }
                    manager.reveal_locked(height);
                });
                return;
            }
            Err(TryLockError::Poisoned(_)) => return,
        };
        self.reveal_locked(height);
    }

    fn reveal_locked(&self, height: f64) {
        if !self.is_showing() {
            return;
        }
        if let Some(window) = self.app.get_webview_window(NUDGE_LABEL) {
            window::reveal(&window, height, (self.placement)());
        }
    }

    /// Resize the already-visible notification to `height` (e.g. when it expands
    /// or collapses on hover). On macOS the frame animates with the system ease;
    /// elsewhere it snaps. Called by the `nudge_resize` command after the
    /// frontend remeasures its content.
    pub fn resize(&self, height: f64) {
        if let Some(window) = self.app.get_webview_window(NUDGE_LABEL) {
            window::resize(&window, height);
        }
    }

    /// Acquire or release key-window status in step with the frontend's own
    /// hover state (macOS only; a no-op elsewhere, where the notification
    /// never takes key at all — see [`window::show`]). An unprompted `show`
    /// deliberately never takes key (see `macos::show`'s doc for why), so
    /// continuous `mouseMoved`-driven hover CSS only works once the user's
    /// cursor has genuinely entered the notification — at which point taking
    /// key carries none of the keystroke-theft risk a forced acquire on
    /// appearance would. Called by the `nudge_set_hovered` command from
    /// `NudgeView.tsx`'s `onMouseEnter`/`onMouseLeave`.
    pub fn set_hovered(&self, hovered: bool) {
        #[cfg(target_os = "macos")]
        {
            if let Some(window) = self.app.get_webview_window(NUDGE_LABEL) {
                macos::set_hovered(&window, hovered);
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = hovered;
        }
    }

    /// Hide the notification without invoking any action.
    pub fn dismiss(&self) {
        let generation = self.teardown_generation.current();
        let _lifecycle = match self.lifecycle.try_lock() {
            Ok(lifecycle) => lifecycle,
            Err(TryLockError::WouldBlock) => {
                let app = self.app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let Some(manager) = app.try_state::<NudgeManager>() else {
                        return;
                    };
                    let Ok(_lifecycle) = manager.lifecycle.lock() else {
                        return;
                    };
                    if !manager.teardown_generation.is_current(generation) {
                        return;
                    }
                    manager.dismiss_locked();
                });
                return;
            }
            Err(TryLockError::Poisoned(_)) => return,
        };
        self.dismiss_locked();
    }

    fn dismiss_locked(&self) {
        // Clear the retained payload so a later `nudge_ready` (e.g. a dev
        // webview reload) doesn't resurrect a nudge the user already dismissed.
        if let Ok(mut pending) = self.pending.lock() {
            *pending = None;
        }
        if let Some(window) = self.app.get_webview_window(NUDGE_LABEL) {
            window::hide(&window);
        }
        self.schedule_teardown();
    }

    fn schedule_teardown(&self) {
        let generation = self.teardown_generation.advance();
        Self::schedule_teardown_attempt(self.app.clone(), generation, TEARDOWN_DELAY);
    }

    fn schedule_teardown_attempt(app: AppHandle, generation: u64, delay: Duration) {
        tauri::async_runtime::spawn_blocking(move || {
            std::thread::sleep(delay);
            let check_app = app.clone();
            let _ = app.run_on_main_thread(move || {
                let Some(manager) = check_app.try_state::<NudgeManager>() else {
                    return;
                };
                let _lifecycle = match manager.lifecycle.try_lock() {
                    Ok(lifecycle) => lifecycle,
                    Err(TryLockError::WouldBlock) => {
                        Self::schedule_teardown_attempt(
                            check_app.clone(),
                            generation,
                            Duration::from_millis(25),
                        );
                        return;
                    }
                    Err(TryLockError::Poisoned(_)) => return,
                };
                if !manager.teardown_generation.is_current(generation) || manager.is_showing() {
                    return;
                }
                if let Some(window) = check_app.get_webview_window(NUDGE_LABEL) {
                    window::destroy(&window);
                }
            });
        });
    }

    /// Whether a nudge is currently on screen or in flight to the webview — a
    /// `show` has fired and no `dismiss` (via CTA, timeout, or the app) has
    /// cleared it yet. Tracks the retained `pending` payload, so it flips to
    /// `false` the moment the notification is dismissed. Callers that must not
    /// stack on or replace a visible notification (e.g. a tray-hover trigger)
    /// gate on this.
    pub fn is_showing(&self) -> bool {
        self.pending.lock().map(|p| p.is_some()).unwrap_or(false)
    }

    pub(crate) fn handle_action(&self, event: NudgeActionEvent) {
        (self.on_action)(event);
        self.dismiss();
    }

    /// Whether the notification currently holds key-window status (macOS
    /// only; always `false` elsewhere, where the notification never takes
    /// key at all — see [`window::show`]). A live query, not an event or a
    /// flag, so it can't go stale or race: unlike `WindowEvent::Focused`,
    /// which tao delivers through a queued event pipeline well after the
    /// AppKit call that triggered it returns, and unlike mere *visibility*,
    /// which stays true for as long as the notification sits on screen —
    /// long after any key handoff it caused has been superseded by an
    /// unrelated, genuine focus change. The app uses this to tell "another
    /// antiburn window lost key focus because the notification just took it"
    /// apart from a real focus loss.
    pub fn is_nudge_key(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.app
                .get_webview_window(NUDGE_LABEL)
                .is_some_and(|w| macos::is_key(&w))
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TeardownGeneration;

    #[test]
    fn a_replacement_nudge_invalidates_the_pending_teardown() {
        let generation = TeardownGeneration::default();
        let dismissed = generation.advance();
        assert!(generation.is_current(dismissed));

        let shown = generation.advance();
        assert!(generation.is_current(shown));
        assert!(!generation.is_current(dismissed));
    }
}
