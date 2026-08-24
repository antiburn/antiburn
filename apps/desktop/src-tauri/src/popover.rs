// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The tray-anchored popover window.
//!
//! The popover is created lazily on the first tray click. After dismissal it
//! stays warm for the process lifetime, so each later open is immediate and
//! keeps the state that its views already loaded.
//!
//! # Geometry
//!
//! The width is fixed at [`WIDTH`]; the height is **per view**, bounded by
//! [`MIN_HEIGHT`] and [`MAX_HEIGHT`], and animated between the two ends of a
//! change so a surface swap reads as one surface growing rather than two
//! windows replacing each other. The webview asks for a height when its view
//! changes ([`crate::commands::set_popover_height`]) and passes `animate: false`
//! when the reader has asked the system for reduced motion — the preference
//! lives in the webview, so the decision is made there and honored here.
//!
//! Every height change re-runs the anchoring maths against the menu-bar item's
//! last known rectangle, because a taller popover on a bottom-anchored panel
//! (Windows, most Linux panels) has to move as well as grow.
//!
//! On Linux the tray backend reports no item rectangle. The tray menu's "Open
//! antiburn" item makes an anchor from the panel instead: the work area shows
//! where the panel is, and the popover opens against the end of it that holds
//! the tray. That placement needs the X11 backend `main.rs` selects.
//!
//! # Dismissal
//!
//! Three things put the popover away everywhere: looking at something else
//! ([`hide_on_focus_loss`]), Escape ([`hide`], reached from the webview), and a
//! second click on the menu-bar item ([`toggle`]). On macOS a fourth joins
//! them — a click anywhere outside the application (`hide_on_outside_click`,
//! driven by [`crate::global_click`]), which is the one case that reports no
//! focus change at all, because clicking the Finder desktop makes no window
//! key. All of them answer to [`set_pinned`]: while the popover is pinned none
//! fire, and the tray menu's Unpin item is what gives them back.
//!
//! Whichever way it goes, it leaves through [`note_hidden`], which is also
//! where the menu-bar item is unlit; [`note_shown`] is the other half.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use tauri::window::{Effect, EffectState, EffectsBuilder};
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Rect, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, Window,
};

/// Window label. Also listed in `capabilities/default.json`.
pub const LABEL: &str = "popover";

/// Emitted the moment the popover reaches the screen, following the same
/// `module:event` naming [`crate::scan`]'s events use. The webview listens
/// for this to refresh live usage — see [`note_shown`] for why that refresh
/// does not simply ride the scan events this same function also kicks.
pub const EVENT_SHOWN: &str = "popover:shown";

/// Popover width in logical pixels. Fixed: the views size themselves to it.
const WIDTH: f64 = 380.0;

/// Corner radius of the popover window, in logical pixels.
///
/// This is `rounded.popover` from `apps/desktop/design.md`. The window corner
/// and the card corners inside it must agree, so change both together.
/// `scripts/check-design-drift.mjs` reads this constant and fails if the two
/// numbers differ.
#[cfg(target_os = "macos")]
const CORNER_RADIUS: f64 = 10.0;

/// Tallest the popover may ever get, in logical pixels.
///
/// Above the app-shell contract's 700, and recorded as such in the deviations
/// register (D-22). The Usage surface asks for it: a provider card now carries
/// the provider's own limits above the local estimates, and two vendors no
/// longer fit in a window sized before that block existed.
pub const MAX_HEIGHT: f64 = 780.0;

/// The height the window is created at, and the one the activity surface uses
/// — so the resting state of the app is unchanged by the ceiling moving.
///
/// Kept at the contract's 700 deliberately. Only the surface that needed more
/// room takes more room; growing the activity list is a separate decision and
/// has not been made.
pub const DEFAULT_HEIGHT: f64 = 700.0;

// D-22, as a build error rather than a test: the ceiling is a ceiling, and a
// resting height above it would mean the window opens already clamped.
const _: () = assert!(MAX_HEIGHT >= DEFAULT_HEIGHT);
const _: () = assert!(DEFAULT_HEIGHT >= MIN_HEIGHT);

/// Shortest the popover ever gets. Below this a view has no room for its own
/// chrome, and a window that small next to the menu bar reads as a glitch.
pub const MIN_HEIGHT: f64 = 320.0;

/// How long a height change takes.
const RESIZE_DURATION: Duration = Duration::from_millis(140);

/// Frames one height change is drawn in. Twelve over 140ms is ~86 fps of
/// requests, which every platform coalesces down to its own refresh rate.
const RESIZE_STEPS: u32 = 12;

/// Gap in logical pixels between the menu-bar item and the popover edge.
///
/// The macOS popover touches the menu bar. Windows and Linux keep a small gap
/// from their taskbar or panel.
#[cfg(target_os = "macos")]
const ANCHOR_GAP: f64 = 0.0;
#[cfg(not(target_os = "macos"))]
const ANCHOR_GAP: f64 = 6.0;

/// Minimum logical gap between the popover and the edge of its display.
const SCREEN_MARGIN: f64 = 8.0;

/// How long after an automatic dismissal a tray click is treated as part of
/// that dismissal rather than a fresh open.
///
/// Clicking the menu-bar item while the popover is open fires focus loss
/// *before* the tray click arrives, so a naive toggle would hide the popover
/// and immediately reopen it. This window swallows that second half.
const REOPEN_SUPPRESSION: Duration = Duration::from_millis(250);

/// The menu-bar item's rectangle, in physical pixels on the display it lives
/// on. Kept as plain numbers rather than a [`Rect`] so a height change can
/// re-anchor without the tray handing the rectangle over a second time.
#[derive(Debug, Clone, Copy, PartialEq)]
struct AnchorRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// A rectangle on a display, in physical pixels. Used for both a monitor and
/// the usable region inside it.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq)]
struct ScreenRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Makes an anchor for a popover that has no menu-bar rectangle to hang off.
///
/// The tray backend on Linux reports no item rectangle, so the popover needs
/// another point to open against. That point is the outer corner of the panel
/// that holds the tray, which the work area describes: the space a panel takes
/// is exactly the space the work area gives up.
///
/// The panel with the larger inset is the one the tray sits in. Its far edge
/// gives the anchor, because every mainstream desktop keeps the tray at the end
/// of the panel: the right end of a horizontal bar.
///
/// The cursor is deliberately **not** used, although it names the very item the
/// reader just chose. The tray menu belongs to the desktop shell, not to this
/// application, so on a Wayland session the pointer is over a surface the X
/// server cannot see, and the reading is a stale position somewhere else
/// entirely. A wrong point is worse than no point, and the panel geometry
/// answers the same question without asking the pointer.
///
/// The rectangle has no width and no height. [`compute_position`] then centers
/// the popover on the point and drops it [`ANCHOR_GAP`] below. A bottom panel
/// flips it above and the display edges clamp it, through the same code every
/// platform already uses.
///
/// The placement needs the X11 backend `main.rs` selects. A session with no X
/// server leaves the position to the compositor.
#[cfg(any(target_os = "linux", test))]
fn synthesized_anchor(
    work_area: Option<ScreenRect>,
    monitor: Option<ScreenRect>,
) -> Option<AnchorRect> {
    let area = work_area?;
    let monitor = monitor?;

    let top_inset = area.y - monitor.y;
    let bottom_inset = (monitor.y + monitor.height) - (area.y + area.height);

    // The tray hangs at the right end of the panel, so the anchor takes the
    // work area's right edge either way. Only the vertical edge changes.
    let x = area.x + area.width;
    let y = if bottom_inset > top_inset {
        // A bottom panel. The anchor is its top edge, so the flip in
        // `compute_position` lifts the popover clear of it.
        area.y + area.height
    } else {
        // A top panel, or no panel at all. The anchor is its bottom edge.
        area.y
    };

    Some(AnchorRect {
        x,
        y,
        width: 0.0,
        height: 0.0,
    })
}

/// The anchor the Linux popover opens against.
///
/// Every reading stays in **physical** pixels, which is what
/// [`compute_position`] expects: it divides by the containing monitor's own
/// scale, so logical values here would be scaled a second time.
///
/// The **primary** monitor answers first, and that is the point of the order.
/// A desktop puts its panel on the primary display, so the tray is there, and
/// the popover belongs beside the tray rather than beside whatever display the
/// window was last on.
#[cfg(target_os = "linux")]
fn linux_anchor(window: &WebviewWindow) -> Option<AnchorRect> {
    let monitor = window
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| window.current_monitor().ok().flatten())?;

    let area = monitor.work_area();
    let work = ScreenRect {
        x: f64::from(area.position.x),
        y: f64::from(area.position.y),
        width: f64::from(area.size.width),
        height: f64::from(area.size.height),
    };
    let bounds = ScreenRect {
        x: f64::from(monitor.position().x),
        y: f64::from(monitor.position().y),
        width: f64::from(monitor.size().width),
        height: f64::from(monitor.size().height),
    };
    synthesized_anchor(Some(work), Some(bounds))
}

/// Opens the popover from the tray menu's Open item.
///
/// This is the only route into the popover on Linux. The AppIndicator backend
/// reports no click events, so [`toggle`] never runs there.
///
/// The item shows the popover; it never toggles it. A toggle is unreachable
/// through a menu: where opening the menu takes focus, the popover is already
/// hidden by the time the reader chooses the item, and an item that reads "Open
/// antiburn" but hides the window reads as broken. Escape, focus loss, and the
/// pin stay the ways to put it away.
#[cfg(target_os = "linux")]
pub fn open_from_tray_menu(app: &AppHandle) {
    // The same gate [`toggle`] applies: before the first run is finished the
    // popover has nothing to show, so send the reader to the flow they are owed.
    if crate::onboarding::is_pending(app) {
        if let Err(error) = crate::onboarding::open(app) {
            ::tracing::warn!(
                event = "onboarding_window_open_failed",
                trigger = "tray",
                error = %error
            );
        }
        return;
    }

    let Some(state) = app.try_state::<PopoverState>() else {
        return;
    };

    // Opening the AppIndicator menu takes focus from an unpinned popover, which
    // dismisses it and arms the reopen suppression. A menu item is an explicit
    // command, not the second half of that gesture, so clear the latch rather
    // than read it. `set_pinned` clears it for the same reason. If this read the
    // latch, the item would do nothing right after the menu opens.
    state.clear_auto_hide();

    let window = match get_or_create(app) {
        Ok(window) => window,
        Err(error) => {
            ::tracing::error!(event = "popover_create_failed", error = %error);
            return;
        }
    };

    // Record the anchor as well as use it: `apply_height` places the window
    // against the recorded one on every view height change.
    //
    // With no anchor, skip placement. The window manager decides, which is what
    // it does today; invented coordinates would be worse.
    if let Some(anchor) = linux_anchor(&window) {
        state.record_anchor(anchor);
        if let Err(error) = place(&window, anchor, WIDTH, state.height()) {
            // Positioning is best-effort here as everywhere else.
            ::tracing::warn!(event = "popover_anchor_failed", error = %error);
        }
    }

    if window.is_visible().unwrap_or(false) {
        // Already on screen, and now re-placed. The scan gate is open, so
        // `note_shown` has nothing left to do.
        let _ = window.set_focus();
        return;
    }

    let _ = window.show();
    let _ = window.set_focus();
    note_shown(app);
}

/// Shared show/hide bookkeeping, registered as Tauri managed state.
pub struct PopoverState {
    auto_hidden_at: Mutex<Option<Instant>>,
    anchor: Mutex<Option<AnchorRect>>,
    /// The height the popover is currently sized to, in logical pixels.
    height: Mutex<f64>,
    /// Bumped by every height request, so an animation still in flight can see
    /// that a newer one superseded it and stop rather than fight it.
    resize_generation: AtomicU64,
    /// While positive, losing focus does not hide the popover. Held around
    /// native dialogs (the folder picker) the popover itself opens: the dialog
    /// takes focus by design, and hiding would tear down the surface the
    /// reader is in the middle of using.
    focus_hold: AtomicU64,
    /// Latched by the tray's Pin item. A focus hold is a moment; this is a
    /// decision, and it holds until the reader takes it back — nothing puts
    /// the popover away while it is set except an explicit unpin or a quit.
    ///
    /// Deliberately not persisted: a pin means "keep this on screen while I
    /// work", and a relaunch is the end of that work.
    pinned: AtomicBool,
}

impl Default for PopoverState {
    fn default() -> Self {
        PopoverState {
            auto_hidden_at: Mutex::new(None),
            anchor: Mutex::new(None),
            height: Mutex::new(DEFAULT_HEIGHT),
            resize_generation: AtomicU64::new(0),
            focus_hold: AtomicU64::new(0),
            pinned: AtomicBool::new(false),
        }
    }
}

impl PopoverState {
    fn record_auto_hide(&self) {
        if let Ok(mut slot) = self.auto_hidden_at.lock() {
            *slot = Some(Instant::now());
        }
    }

    /// Forget any automatic dismissal, so the next open is treated as a fresh
    /// one. Used when pinning: the tray right-click that opened the menu is
    /// what hid the popover, and the pin must not be swallowed as the second
    /// half of that gesture.
    fn clear_auto_hide(&self) {
        if let Ok(mut slot) = self.auto_hidden_at.lock() {
            *slot = None;
        }
    }

    /// True when an automatic dismissal just happened, meaning the tray click
    /// being handled is the same user gesture that caused it.
    fn suppresses_reopen(&self) -> bool {
        let Ok(mut slot) = self.auto_hidden_at.lock() else {
            return false;
        };
        match *slot {
            Some(at) if at.elapsed() < REOPEN_SUPPRESSION => {
                *slot = None;
                true
            }
            _ => false,
        }
    }

    fn begin_focus_hold(&self) {
        self.focus_hold.fetch_add(1, Ordering::SeqCst);
    }

    /// Ends one hold. Saturating: an unmatched release (a frontend bug) must
    /// not underflow into a near-infinite hold.
    fn end_focus_hold(&self) {
        let _ = self
            .focus_hold
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |held| {
                held.checked_sub(1)
            });
    }

    fn holds_focus(&self) -> bool {
        self.focus_hold.load(Ordering::SeqCst) > 0
    }

    fn is_pinned(&self) -> bool {
        self.pinned.load(Ordering::SeqCst)
    }

    fn set_pinned(&self, pinned: bool) {
        self.pinned.store(pinned, Ordering::SeqCst);
    }

    fn record_anchor(&self, anchor: AnchorRect) {
        if let Ok(mut slot) = self.anchor.lock() {
            *slot = Some(anchor);
        }
    }

    fn anchor(&self) -> Option<AnchorRect> {
        self.anchor.lock().ok().and_then(|slot| *slot)
    }

    fn height(&self) -> f64 {
        self.height
            .lock()
            .map(|height| *height)
            .unwrap_or(DEFAULT_HEIGHT)
    }

    fn set_height(&self, height: f64) {
        if let Ok(mut slot) = self.height.lock() {
            *slot = height;
        }
    }

    /// Claim the right to drive the window's height, invalidating any animation
    /// already running.
    fn begin_resize(&self) -> u64 {
        self.resize_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn resize_is_current(&self, generation: u64) -> bool {
        self.resize_generation.load(Ordering::SeqCst) == generation
    }
}

/// A requested height, held inside the bounds the window can actually be.
pub fn clamp_height(height: f64) -> f64 {
    if height.is_nan() {
        return DEFAULT_HEIGHT;
    }
    height.clamp(MIN_HEIGHT, MAX_HEIGHT)
}

/// Ease-out cubic. A height change should arrive quickly and settle, which is
/// what makes a growing surface read as one surface rather than a jump.
fn ease_out(progress: f64) -> f64 {
    let remaining = 1.0 - progress.clamp(0.0, 1.0);
    1.0 - remaining * remaining * remaining
}

/// Return the popover, creating it hidden and off the taskbar on first use.
fn get_or_create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window(LABEL) {
        return Ok(window);
    }

    let height = app
        .try_state::<PopoverState>()
        .map(|state| state.height())
        .unwrap_or(DEFAULT_HEIGHT);
    let builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .title("antiburn")
        .inner_size(WIDTH, height)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .shadow(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false);

    // Let the first click both focus the popover and act on the control under
    // the cursor; a menu-bar surface that eats the first click feels broken.
    //
    // The popover material also gives the window its rounded corner. Without it
    // the window is an opaque square, which is wrong for a menu-bar surface, and
    // the translucent palette in the stylesheets has nothing to sit on. The
    // stylesheets already paint html, body, and #root transparent, so the
    // material is what the reader sees behind the content.
    #[cfg(target_os = "macos")]
    let builder = builder.accept_first_mouse(true).transparent(true).effects(
        EffectsBuilder::new()
            .effect(Effect::Popover)
            .state(EffectState::Active)
            .radius(CORNER_RADIUS)
            .build(),
    );

    match builder.build() {
        Ok(window) => Ok(window),
        // A tray click and a pin request can race into this function. Return
        // whichever build won instead of dropping the user's request.
        Err(error) => app.get_webview_window(LABEL).ok_or(error),
    }
}

/// Handles a click on the menu-bar item.
///
/// `anchor` is the item's screen rectangle as reported by the tray backend.
pub fn toggle(app: &AppHandle, anchor: Rect) {
    // Before the first run is finished the popover has nothing to show — the
    // activity list is empty by construction, because the scan scheduler is
    // gated on the same flag (see [`crate::scan`]). Send the click to the flow
    // that is actually owed the reader, which also gets the window back for
    // anyone who closed it partway through.
    if crate::onboarding::is_pending(app) {
        if let Err(error) = crate::onboarding::open(app) {
            ::tracing::warn!(
                event = "onboarding_window_open_failed",
                trigger = "tray",
                error = %error
            );
        }
        return;
    }

    if let Some(window) = app.get_webview_window(LABEL)
        && window.is_visible().unwrap_or(false)
    {
        // A pinned popover has no hide half to its toggle. Raise it instead of
        // doing nothing: a menu-bar item that swallows a click reads as broken,
        // and re-anchoring picks up a menu bar that has since moved display.
        if is_pinned(app) {
            if let Err(error) = anchor_to(&window, anchor) {
                ::tracing::warn!(event = "popover_anchor_failed", error = %error);
            }
            let _ = window.set_focus();
            return;
        }
        let _ = window.hide();
        note_hidden(app);
        return;
    }

    if let Some(state) = app.try_state::<PopoverState>()
        && state.suppresses_reopen()
    {
        return;
    }

    let window = match get_or_create(app) {
        Ok(window) => window,
        Err(error) => {
            ::tracing::error!(event = "popover_create_failed", error = %error);
            return;
        }
    };

    if let Err(error) = anchor_to(&window, anchor) {
        // Positioning is best-effort: a popover in the wrong place still beats
        // no popover at all.
        ::tracing::warn!(event = "popover_anchor_failed", error = %error);
    }

    let _ = window.show();
    let _ = window.set_focus();
    note_shown(app);
}

/// Dismisses the popover from the webview — the Escape key, and anything else
/// the views treat as "put this away".
///
/// Deliberately a shell command rather than `getCurrentWindow().hide()`: the
/// scan scheduler is gated on the popover being on screen, and a window hidden
/// behind the shell's back would leave that gate stuck open.
///
/// No-op while pinned. The webview still asks — the decision is the shell's,
/// so every dismissal answers to the same gate.
pub fn hide(app: &AppHandle) {
    if is_pinned(app) {
        return;
    }
    hide_window(app);
}

/// Hides the popover when setup must own the application surface.
///
/// This operation keeps the pin choice. The pin applies again after setup.
pub fn hide_for_onboarding(app: &AppHandle) {
    hide_window(app);
}

fn hide_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
    }
    note_hidden(app);
}

/// Resize the popover to the height the current view asked for.
///
/// Clamped to [`MIN_HEIGHT`]..=[`MAX_HEIGHT`], animated unless the caller says
/// otherwise, and re-anchored on the way so a popover hanging off a
/// bottom-of-screen panel grows upward instead of off the display.
pub fn set_height(app: &AppHandle, requested: f64, animate: bool) {
    let Some(state) = app.try_state::<PopoverState>() else {
        return;
    };
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };

    let target = clamp_height(requested);
    let from = state.height();
    // Sub-pixel requests are the view re-reporting the height it already has.
    if (from - target).abs() < 1.0 {
        return;
    }

    let generation = state.begin_resize();
    if !animate {
        state.set_height(target);
        apply_height(&window, target);
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let step = RESIZE_DURATION / RESIZE_STEPS;
        for frame in 1..=RESIZE_STEPS {
            tokio::time::sleep(step).await;
            let Some(state) = app.try_state::<PopoverState>() else {
                return;
            };
            // A newer request owns the window now.
            if !state.resize_is_current(generation) {
                return;
            }
            let progress = f64::from(frame) / f64::from(RESIZE_STEPS);
            let height = from + (target - from) * ease_out(progress);
            state.set_height(height);
            let Some(window) = app.get_webview_window(LABEL) else {
                return;
            };
            apply_height(&window, height);
        }
    });
}

/// Size the window and put it back where its anchor says it belongs.
fn apply_height(window: &WebviewWindow, height: f64) {
    if window.set_size(LogicalSize::new(WIDTH, height)).is_err() {
        return;
    }
    let Some(state) = window.app_handle().try_state::<PopoverState>() else {
        return;
    };
    let Some(anchor) = state.anchor() else {
        // Never opened, so there is nothing to anchor to yet; the next open
        // places it.
        return;
    };
    let _ = place(window, anchor, WIDTH, height);
}

/// Hides the popover after it loses focus, remembering when it happened.
///
/// No-op while a focus hold is active: a native dialog the popover opened is
/// about to take (or has taken) focus, and the popover must survive it. Also a
/// no-op while pinned — looking away is exactly what a pin is for.
///
/// Both cases return *before* recording the dismissal: nothing was dismissed,
/// so the next tray click is a fresh open and must not be suppressed.
pub fn hide_on_focus_loss(window: &Window) {
    dismiss(window.app_handle());
}

/// Hides the popover after a click somewhere else on the desktop.
///
/// Focus loss is not enough on its own. Clicking the Finder desktop makes no
/// window key, so Tauri reports no focus change and the popover would stay on
/// screen — with, now, the menu-bar item stranded lit. The global click
/// monitor in [`crate::global_click`] closes that gap and lands here.
///
/// Answers to the pin like the other three dismissals, through [`dismiss`].
///
/// Only acts on a popover that is actually on screen. Nothing here should
/// touch the reopen suppression on a popover that is already away: the tray
/// click that just closed it must still be able to reopen it on the press
/// after next.
///
/// macOS-only, because its caller is: the monitor that drives it exists
/// nowhere else, and an uncalled dismissal on the other platforms is dead code
/// rather than a dormant feature.
#[cfg(target_os = "macos")]
pub fn hide_on_outside_click(app: &AppHandle) {
    let visible = app
        .get_webview_window(LABEL)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    if !visible {
        return;
    }
    dismiss(app);
}

/// Puts the popover away because the reader looked elsewhere, and records when
/// — so the tray click that may have caused it is not read as a fresh open.
///
/// No-op while a focus hold is active: a native dialog the popover opened is
/// about to take (or has taken) focus, and the popover must survive it. Also a
/// no-op while pinned — looking away is exactly what a pin is for.
///
/// Both cases return *before* recording the dismissal: nothing was dismissed,
/// so the next tray click is a fresh open and must not be suppressed.
fn dismiss(app: &AppHandle) {
    if let Some(state) = app.try_state::<PopoverState>() {
        if state.holds_focus() || state.is_pinned() {
            return;
        }
        state.record_auto_hide();
    }
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.hide();
    }
    note_hidden(app);
}

/// Begin a focus hold. Paired with [`end_focus_hold`] around a native dialog.
pub fn begin_focus_hold(app: &AppHandle) {
    if let Some(state) = app.try_state::<PopoverState>() {
        state.begin_focus_hold();
    }
}

/// End a focus hold and hand focus back to the popover.
///
/// The dialog that motivated the hold had focus while it was up; when it
/// closes, macOS gives focus back to whichever window it likes. Refocusing the
/// still-visible popover keeps Escape and the keyboard working where the
/// reader left off.
pub fn end_focus_hold(app: &AppHandle) {
    if let Some(state) = app.try_state::<PopoverState>() {
        state.end_focus_hold();
    }
    if let Some(window) = app.get_webview_window(LABEL)
        && window.is_visible().unwrap_or(false)
    {
        let _ = window.set_focus();
    }
}

/// Whether the popover is currently pinned open.
pub fn is_pinned(app: &AppHandle) -> bool {
    app.try_state::<PopoverState>()
        .is_some_and(|state| state.is_pinned())
}

/// Sets the pin, and either way hands focus back to the popover.
///
/// Pinning has to re-show the window, not just set a flag: the tray right-click
/// that opened the menu took focus, so by the time this runs the popover has
/// already hidden itself.
///
/// Unpinning does not hide anything — it restores ordinary dismissal rather
/// than performing one — but it *must* refocus, for the same reason
/// [`end_focus_hold`] does. The window sat there unfocused for as long as the
/// menu was up, so without this there is no focus left to lose and the next
/// click on another application would dismiss nothing.
///
/// Pinning is refused while the first run is unfinished. Re-showing is the
/// point of a pin, and the popover must stay hidden during this period.
/// Unpinning remains available so the tray action always does what it says.
pub fn set_pinned(app: &AppHandle, pinned: bool) {
    if pinned && crate::onboarding::is_pending(app) {
        return;
    }
    let Some(state) = app.try_state::<PopoverState>() else {
        return;
    };
    if !pinned {
        state.set_pinned(false);
        if let Some(window) = app.get_webview_window(LABEL)
            && window.is_visible().unwrap_or(false)
        {
            let _ = window.set_focus();
        }
        return;
    }

    let window = match get_or_create(app) {
        Ok(window) => window,
        Err(error) => {
            ::tracing::error!(event = "popover_create_failed", error = %error);
            return;
        }
    };
    state.set_pinned(true);

    // Only a pin re-opens, and only from hidden: re-anchoring a window already
    // on screen would move it for no reason the reader asked for.
    if pinned && !window.is_visible().unwrap_or(false) {
        // The dismissal this pin is undoing armed the reopen suppression;
        // leaving it armed would let the next tray click be swallowed.
        state.clear_auto_hide();

        // The menu-bar item's live rectangle is the truth, and the only source
        // that works on a cold start where the popover has never been opened.
        // The remembered anchor covers a tray backend that reports none.
        let placed = match app
            .tray_by_id(crate::tray::TRAY_ID)
            .and_then(|tray| tray.rect().ok().flatten())
        {
            Some(rect) => anchor_to(&window, rect),
            None => match state.anchor() {
                Some(anchor) => place(&window, anchor, WIDTH, state.height()),
                None => fallback_place(&window, &state),
            },
        };
        if let Err(error) = placed {
            // Best-effort, as everywhere else: a popover in the wrong place
            // still beats a pin that appears to do nothing.
            ::tracing::warn!(event = "popover_anchor_failed", error = %error);
        }

        let _ = window.show();
        note_shown(app);
    }

    // Guarded exactly as `end_focus_hold` is: focusing a hidden window would
    // order it front, and an unpin is not a request to open anything.
    if window.is_visible().unwrap_or(false) {
        let _ = window.set_focus();
    }
}

/// Everything that has to happen when the popover reaches the screen, whichever
/// way it got there.
///
/// The popover *is* the view of the scanned data, so its visibility is what
/// gates the periodic rescan (see [`crate::scan`]). Reported from here rather
/// than inferred from window events, because a hidden window that was never
/// shown produces no event at all.
///
/// The menu-bar highlight is lit here for the same reason [`note_hidden`]
/// clears it: both open paths already run through this one — the toggle, and a
/// pin re-showing a window the tray menu had dismissed — so pairing the
/// highlight with visibility is structural rather than something each caller
/// has to remember.
fn note_shown(app: &AppHandle) {
    if let Some(controller) = app.try_state::<crate::scan::ScanController>() {
        controller.set_popover_visible(true);
        // Opening the popover is the one moment a reader is guaranteed to be
        // looking, so refresh immediately instead of waiting out a tick.
        controller.request();
    }
    // A separate signal from the scan kick just above, on purpose, rather
    // than something the webview infers from `scan:finished`. Two things
    // about the scan kick make it the wrong vehicle for a usage refresh:
    // `request()` itself always wakes the scheduler loop, but the pass that
    // wake-up would run is silently skipped whenever discovery is paused or
    // onboarding has not finished (`scheduled_scanning_allowed` in
    // `crate::scan`), which would leave usage silently stuck too even
    // though nothing about reading a provider's own limits depends on
    // whether local disk discovery is allowed to run; and even when a scan
    // does run, it is a full disk walk, so gating the usage refresh on its
    // completion would make "reopen the popover" mean "wait out a scan" for
    // a figure that has nothing to do with what is on disk. Emitting this
    // unconditionally, the instant the popover is on screen, keeps the two
    // refreshes independent the way the data they show already is.
    let _ = app.emit(EVENT_SHOWN, ());
    crate::tray::set_highlight(app, true);
}

/// The close-side counterpart: everything that has to happen when the popover
/// leaves the screen, whichever way it left.
///
/// Every close path already runs through here — the toggle, the webview's
/// Escape, dismissal on focus loss or an outside click, and the shell's
/// suppressed window close — which is why the menu-bar highlight is cleared
/// here rather than at each of them.
///
/// Unconditional: clearing a highlight that is already off costs nothing, and
/// a state that somehow drifted out of step is corrected rather than kept.
pub fn note_hidden(app: &AppHandle) {
    if let Some(controller) = app.try_state::<crate::scan::ScanController>() {
        controller.set_popover_visible(false);
    }
    crate::tray::set_highlight(app, false);
}

/// Records the menu-bar item's rectangle and places the popover against it.
fn anchor_to(window: &WebviewWindow, anchor: Rect) -> tauri::Result<()> {
    // Tray backends hand over *physical* coordinates on macOS and Windows, so
    // the `to_physical(1.0)` is a no-op cast, not a conversion. Notably the
    // popover window's own scale factor must NOT be used here: on a
    // multi-display desktop the popover may last have been shown on a
    // different-DPI screen than the one the menu-bar item lives on, and
    // converting with the wrong scale lands the window on the wrong display.
    let position = anchor.position.to_physical::<f64>(1.0);
    let size = anchor.size.to_physical::<f64>(1.0);
    let anchor = AnchorRect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };
    let state = window.app_handle().state::<PopoverState>();
    state.record_anchor(anchor);

    // The window's own logical geometry: the width is a constant and the
    // height is the shell's bookkeeping, so no physical size read (whose scale
    // depends on where the window last was) is involved.
    place(window, anchor, WIDTH, state.height())
}

/// The last placement a pinned show can try: the tray backend reports no
/// rectangle, and no earlier open left an anchor behind.
///
/// Linux only, because it is the only platform that reaches here. A pin on a
/// cold start would otherwise leave the window wherever the window manager put
/// it, which is the middle of the screen.
///
/// Records the anchor as well as using it, so the next height change places the
/// window against the same point.
#[cfg(target_os = "linux")]
fn fallback_place(window: &WebviewWindow, state: &PopoverState) -> tauri::Result<()> {
    let Some(anchor) = linux_anchor(window) else {
        return Ok(());
    };
    state.record_anchor(anchor);
    place(window, anchor, WIDTH, state.height())
}

#[cfg(not(target_os = "linux"))]
fn fallback_place(_window: &WebviewWindow, _state: &PopoverState) -> tauri::Result<()> {
    Ok(())
}

/// The display's usable frame in logical coordinates, plus the scale that maps
/// the anchor's physical rectangle into the same space.
struct MonitorFrame {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    scale: f64,
}

/// The monitor whose physical rectangle contains the menu-bar item's origin.
///
/// Chosen by containment over the whole monitor list rather than
/// `monitor_from_point`, so the answer is the display the *anchor* is on —
/// the only display whose scale correctly reverses the tray rect's
/// physical-to-logical conversion on a mixed-DPI desktop.
fn monitor_frame_for(window: &WebviewWindow, anchor: AnchorRect) -> Option<MonitorFrame> {
    let monitors = window.available_monitors().ok()?;
    let monitor = monitors
        .into_iter()
        .find(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            anchor.x >= f64::from(position.x)
                && anchor.x < f64::from(position.x) + f64::from(size.width)
                && anchor.y >= f64::from(position.y)
                && anchor.y < f64::from(position.y) + f64::from(size.height)
        })
        .or_else(|| window.current_monitor().ok().flatten())?;

    let scale = monitor.scale_factor();
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let position = monitor.position();
    let size = monitor.size();
    let left = f64::from(position.x) / scale;
    let top = f64::from(position.y) / scale;
    Some(MonitorFrame {
        left,
        top,
        right: left + f64::from(size.width) / scale,
        bottom: top + f64::from(size.height) / scale,
        scale,
    })
}

/// Where the popover belongs, in logical coordinates. Pure, so the flip and
/// clamp behavior is testable without a window.
fn compute_position(
    anchor: AnchorRect,
    frame: Option<&MonitorFrame>,
    width: f64,
    height: f64,
) -> (f64, f64) {
    let scale = frame.map(|frame| frame.scale).unwrap_or(1.0);
    let ax = anchor.x / scale;
    let ay = anchor.y / scale;
    let aw = anchor.width / scale;
    let ah = anchor.height / scale;

    let mut x = ax + aw / 2.0 - width / 2.0;
    let mut y = ay + ah + ANCHOR_GAP;

    if let Some(frame) = frame {
        let left = frame.left + SCREEN_MARGIN;
        // The macOS tray rectangle can end one logical pixel above the monitor frame.
        // Keep its anchor position so the clamp does not restore a gap.
        #[cfg(target_os = "macos")]
        let top = frame.top.min(y);
        #[cfg(not(target_os = "macos"))]
        let top = frame.top + SCREEN_MARGIN;
        let right = frame.right - width - SCREEN_MARGIN;
        let bottom = frame.bottom - height - SCREEN_MARGIN;

        // Where the menu bar sits at the bottom of the screen — Windows, and
        // Linux panels — flip the popover above its anchor instead.
        if y > bottom {
            y = ay - height - ANCHOR_GAP;
        }

        x = clamp(x, left, right);
        y = clamp(y, top, bottom);
    }

    (x, y)
}

/// Places the popover under (or above) the menu-bar item, clamped to the
/// display the item lives on.
///
/// `width` and `height` are the window's own size in **logical** pixels. They
/// are passed in rather than read from the window because a resize in flight
/// has not necessarily been reported back yet, and a hidden window's physical
/// size is still scaled for whichever display it was last shown on.
fn place(window: &WebviewWindow, anchor: AnchorRect, width: f64, height: f64) -> tauri::Result<()> {
    let frame = monitor_frame_for(window, anchor);
    let (x, y) = compute_position(anchor, frame.as_ref(), width, height);
    window.set_position(LogicalPosition::new(x, y))
}

/// `f64::clamp` panics when `max < min`, which happens on displays narrower
/// than the popover. Prefer the low edge there.
fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if max < min {
        return min;
    }
    value.clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_prefers_the_low_edge_on_undersized_displays() {
        assert_eq!(clamp(500.0, 8.0, -20.0), 8.0);
    }

    /// The macOS popover starts at the menu bar's bottom edge.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_macos_popover_touches_the_menu_bar() {
        let frame = MonitorFrame {
            left: 0.0,
            top: -497.0,
            right: 1728.0,
            bottom: 620.0,
            scale: 2.0,
        };
        let anchor = AnchorRect {
            x: 3000.0,
            y: -1044.0,
            width: 60.0,
            height: 48.0,
        };
        let (_, y) = compute_position(anchor, Some(&frame), WIDTH, DEFAULT_HEIGHT);
        let menu_bar_bottom = (anchor.y + anchor.height) / frame.scale;

        assert!((y - menu_bar_bottom).abs() < 0.5, "y was {y}");
    }

    /// Windows and Linux keep the existing gap from their taskbar or panel.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn other_platforms_keep_the_existing_anchor_gap() {
        assert_eq!(ANCHOR_GAP, 6.0);
    }

    /// A 2x display: the anchor arrives in physical pixels, the window is
    /// placed in logical ones. The popover must center under the item on the
    /// *anchor's* display, whatever scale the window last rendered at.
    #[test]
    fn a_high_dpi_anchor_is_centered_in_its_displays_logical_space() {
        let frame = MonitorFrame {
            left: 0.0,
            top: 0.0,
            right: 1728.0, // 3456 physical / 2.0
            bottom: 1117.0,
            scale: 2.0,
        };
        // Menu-bar item at physical x=3000, 60 wide, bottom edge y=48.
        let anchor = AnchorRect {
            x: 3000.0,
            y: 0.0,
            width: 60.0,
            height: 48.0,
        };
        let (x, y) = compute_position(anchor, Some(&frame), WIDTH, DEFAULT_HEIGHT);
        // Logical center of the item is 1515; half the popover left of that.
        assert!((x - (1515.0 - WIDTH / 2.0)).abs() < 0.5, "x was {x}");
        assert!((y - (24.0 + ANCHOR_GAP)).abs() < 0.5, "y was {y}");
    }

    /// A secondary 1x display sitting right of a 2x primary: its physical
    /// origin is offset, and the anchor must resolve against ITS frame — the
    /// frame derived by dividing that monitor's physical rect by its own
    /// scale, exactly what `monitor_frame_for` produces.
    #[test]
    fn a_second_display_with_different_scale_places_in_its_own_frame() {
        // Physical origin x=3456 (right of the 2x primary), scale 1.0.
        let frame = MonitorFrame {
            left: 3456.0,
            top: 0.0,
            right: 3456.0 + 1920.0,
            bottom: 1080.0,
            scale: 1.0,
        };
        let anchor = AnchorRect {
            x: 4000.0,
            y: 0.0,
            width: 40.0,
            height: 24.0,
        };
        let (x, y) = compute_position(anchor, Some(&frame), WIDTH, DEFAULT_HEIGHT);
        assert!((x - (4020.0 - WIDTH / 2.0)).abs() < 0.5, "x was {x}");
        assert!((y - (24.0 + ANCHOR_GAP)).abs() < 0.5, "y was {y}");
    }

    /// Bottom taskbar (Windows/Linux): a popover that would overflow the
    /// bottom edge flips above its anchor.
    #[test]
    fn a_bottom_anchored_panel_flips_the_popover_above_the_item() {
        let frame = MonitorFrame {
            left: 0.0,
            top: 0.0,
            right: 1920.0,
            bottom: 1080.0,
            scale: 1.0,
        };
        let anchor = AnchorRect {
            x: 1700.0,
            y: 1040.0,
            width: 40.0,
            height: 40.0,
        };
        let (_, y) = compute_position(anchor, Some(&frame), WIDTH, MAX_HEIGHT);
        assert!(
            (y - (1040.0 - MAX_HEIGHT - ANCHOR_GAP)).abs() < 0.5,
            "y was {y}"
        );
    }

    /// The clamp keeps the popover on the display when the item sits in a
    /// corner.
    #[test]
    fn a_corner_anchor_is_clamped_inside_the_display_margin() {
        let frame = MonitorFrame {
            left: 0.0,
            top: 0.0,
            right: 1920.0,
            bottom: 1080.0,
            scale: 1.0,
        };
        let anchor = AnchorRect {
            x: 1910.0,
            y: 0.0,
            width: 10.0,
            height: 24.0,
        };
        let (x, _) = compute_position(anchor, Some(&frame), WIDTH, MAX_HEIGHT);
        assert!(
            (x - (1920.0 - WIDTH - SCREEN_MARGIN)).abs() < 0.5,
            "x was {x}"
        );
    }

    #[test]
    fn clamp_bounds_the_value_normally() {
        assert_eq!(clamp(-5.0, 0.0, 100.0), 0.0);
        assert_eq!(clamp(150.0, 0.0, 100.0), 100.0);
        assert_eq!(clamp(42.0, 0.0, 100.0), 42.0);
    }

    #[test]
    fn a_fresh_state_does_not_suppress_reopening() {
        let state = PopoverState::default();
        assert!(!state.suppresses_reopen());
    }

    #[test]
    fn an_automatic_dismissal_suppresses_exactly_one_reopen() {
        let state = PopoverState::default();
        state.record_auto_hide();
        assert!(state.suppresses_reopen());
        assert!(!state.suppresses_reopen());
    }

    #[test]
    fn a_fresh_popover_is_not_pinned() {
        let state = PopoverState::default();
        assert!(!state.is_pinned());
    }

    #[test]
    fn the_pin_is_a_latch_the_reader_can_take_back() {
        let state = PopoverState::default();
        state.set_pinned(true);
        assert!(state.is_pinned());
        state.set_pinned(false);
        assert!(!state.is_pinned());
    }

    /// The tray right-click that opens the menu hides the popover, arming the
    /// reopen suppression. Pinning clears it, or the next tray click would be
    /// swallowed as the second half of a dismissal that no longer stands.
    #[test]
    fn clearing_a_dismissal_leaves_no_suppression_behind() {
        let state = PopoverState::default();
        state.record_auto_hide();
        state.clear_auto_hide();
        assert!(!state.suppresses_reopen());
    }

    #[test]
    fn a_view_can_only_ask_for_a_height_the_window_can_actually_be() {
        assert_eq!(clamp_height(MAX_HEIGHT), MAX_HEIGHT);
        assert_eq!(clamp_height(MIN_HEIGHT), MIN_HEIGHT);
        // The ceiling is a ceiling, not a suggestion.
        assert_eq!(clamp_height(2_000.0), MAX_HEIGHT);
        assert_eq!(clamp_height(10.0), MIN_HEIGHT);
        assert_eq!(clamp_height(-1.0), MIN_HEIGHT);
        // A height that is not a number falls back to the resting size rather
        // than the ceiling: an unreadable request is not a request to grow.
        assert_eq!(clamp_height(f64::NAN), DEFAULT_HEIGHT);
    }

    #[test]
    fn a_fresh_popover_rests_at_the_default_height() {
        let state = PopoverState::default();
        assert_eq!(state.height(), DEFAULT_HEIGHT);
        assert!(state.anchor().is_none());
    }

    #[test]
    fn a_newer_height_request_invalidates_the_animation_already_running() {
        let state = PopoverState::default();
        let first = state.begin_resize();
        assert!(state.resize_is_current(first));

        let second = state.begin_resize();
        assert!(state.resize_is_current(second));
        assert!(
            !state.resize_is_current(first),
            "the superseded animation must stop rather than fight the new one"
        );
    }

    #[test]
    fn the_height_curve_starts_and_ends_where_it_should() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        // Ease-out: more than half the distance is covered by the halfway point.
        assert!(ease_out(0.5) > 0.5);
        assert!(ease_out(0.25) < ease_out(0.75));
    }

    /// The GNOME shape, measured from a running session: a 32px top bar and a
    /// 67px dock on the left. The tray is at the right end of the top bar, so
    /// the anchor is the work area's top-right corner.
    #[test]
    fn a_top_panel_anchors_at_the_right_end_of_that_panel() {
        let work = ScreenRect {
            x: 67.0,
            y: 32.0,
            width: 2493.0,
            height: 1338.0,
        };
        let monitor = ScreenRect {
            x: 0.0,
            y: 0.0,
            width: 2560.0,
            height: 1370.0,
        };
        assert_eq!(
            synthesized_anchor(Some(work), Some(monitor)),
            Some(AnchorRect {
                x: 2560.0,
                y: 32.0,
                width: 0.0,
                height: 0.0,
            })
        );
    }

    /// The KDE and Windows-like shape: one panel along the bottom. The anchor
    /// is that panel's *top* edge, so the popover has something to sit above.
    #[test]
    fn a_bottom_panel_anchors_at_the_top_edge_of_that_panel() {
        let work = ScreenRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1036.0,
        };
        let monitor = ScreenRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        assert_eq!(
            synthesized_anchor(Some(work), Some(monitor)),
            Some(AnchorRect {
                x: 1920.0,
                y: 1036.0,
                width: 0.0,
                height: 0.0,
            })
        );
    }

    /// A display whose work area fills it has no panel to read. The top edge is
    /// the right answer, because a tray that is not in a reserved strip is in a
    /// bar the desktop draws over its own windows.
    #[test]
    fn a_display_with_no_reserved_panel_anchors_at_its_top_right() {
        let full = ScreenRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        assert_eq!(
            synthesized_anchor(Some(full), Some(full)),
            Some(AnchorRect {
                x: 1920.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            })
        );
    }

    /// Nothing to place against is not an excuse to invent coordinates: the
    /// window manager keeps the decision.
    #[test]
    fn no_monitor_at_all_leaves_the_placement_to_the_window_manager() {
        assert_eq!(synthesized_anchor(None, None), None);
    }

    /// The whole Linux path, end to end, on the session this was measured from:
    /// the popover opens under the top bar and tucks against the right edge,
    /// which is where the tray item it belongs to sits.
    #[test]
    fn the_gnome_anchor_places_the_popover_beside_the_tray() {
        let work = ScreenRect {
            x: 67.0,
            y: 32.0,
            width: 2493.0,
            height: 1338.0,
        };
        let monitor = ScreenRect {
            x: 0.0,
            y: 0.0,
            width: 2560.0,
            height: 1370.0,
        };
        let frame = MonitorFrame {
            left: 0.0,
            top: 0.0,
            right: 2560.0,
            bottom: 1370.0,
            scale: 1.0,
        };
        let anchor = synthesized_anchor(Some(work), Some(monitor)).expect("the monitor answered");
        let (x, y) = compute_position(anchor, Some(&frame), WIDTH, DEFAULT_HEIGHT);

        // Clamped to the display's right margin rather than centered on the
        // corner, which is what keeps it under the tray instead of half off
        // the screen.
        assert!(
            (x - (2560.0 - WIDTH - SCREEN_MARGIN)).abs() < 0.5,
            "x was {x}"
        );
        assert!((y - (32.0 + ANCHOR_GAP)).abs() < 0.5, "y was {y}");
    }

    /// The contract the Linux glue depends on: the anchor goes in physical, the
    /// window position comes out logical. A 2x display halves both numbers.
    #[test]
    fn a_high_dpi_anchor_is_placed_in_its_displays_logical_space() {
        let frame = MonitorFrame {
            left: 0.0,
            top: 0.0,
            right: 1280.0, // 2560 physical / 2.0
            bottom: 720.0, // 1440 physical / 2.0
            scale: 2.0,
        };
        let anchor = AnchorRect {
            x: 2000.0,
            y: 60.0,
            width: 0.0,
            height: 0.0,
        };
        let (x, y) = compute_position(anchor, Some(&frame), WIDTH, MIN_HEIGHT);
        // Logical anchor is (1000, 30).
        assert!((x - (1000.0 - WIDTH / 2.0)).abs() < 0.5, "x was {x}");
        assert!((y - (30.0 + ANCHOR_GAP)).abs() < 0.5, "y was {y}");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_bottom_anchored_panel_flips_the_popover_above_its_item() {
        // The maths `place` runs, reproduced against a 1080px-tall display with
        // its panel at the bottom. A 700px popover cannot fit below a taskbar
        // item at y=1040, so it must be placed above it.
        let anchor = AnchorRect {
            x: 900.0,
            y: 1040.0,
            width: 32.0,
            height: 40.0,
        };
        let height = 700.0;
        let bottom = 1080.0 - height - SCREEN_MARGIN;
        let below = anchor.y + anchor.height + ANCHOR_GAP;
        assert!(below > bottom, "there is no room below the item");
        assert_eq!(anchor.y - height - ANCHOR_GAP, 334.0);
    }
}
