//! Edge dock: the HUD parks at one edge of its display with a tab showing.
//!
//! A drag that drops the HUD against a display edge docks it there. Docked,
//! the window sits outside the display except for a tab, and its renderer
//! keeps running. The pointer resting on the tab peeks the HUD in; it parks
//! again after a quiet spell. A drag on a docked or peeked HUD tears it off,
//! and it stays free until the next drop at an edge. The shell can also wake
//! a docked HUD for a while.

use std::sync::Mutex;
#[cfg(any(target_os = "macos", test))]
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use tauri::{AppHandle, Manager, Monitor, PhysicalPosition, WebviewWindow};

/// How much of the docked HUD frame stays on screen, in logical pixels.
#[cfg(any(target_os = "macos", test))]
const TAB: f64 = 6.0;

/// The transparent gap between the window's side edges and the HUD frame.
///
/// The webview draws the frame with an 8 px margin on the left and right, so
/// a side tab adds this gap to keep `TAB` of the frame visible.
#[cfg(any(target_os = "macos", test))]
const SIDE_INSET: f64 = 8.0;

/// How long one slide takes.
#[cfg(target_os = "macos")]
const SLIDE_DURATION: Duration = Duration::from_millis(200);

/// Frames per slide.
#[cfg(target_os = "macos")]
const SLIDE_STEPS: u32 = 12;

/// How often the docked HUD reads the cursor.
#[cfg(target_os = "macos")]
pub(crate) const TAB_POLL: Duration = Duration::from_millis(100);

/// How long the cursor must rest on the tab before the HUD peeks in.
#[cfg(target_os = "macos")]
pub(crate) const TAB_HOLD: Duration = Duration::from_millis(150);

/// How often a peeked HUD checks whether it should park again.
#[cfg(target_os = "macos")]
const AUTO_DOCK_POLL: Duration = Duration::from_millis(200);

/// How long a peeked HUD stays after the pointer leaves it and the edge.
#[cfg(target_os = "macos")]
pub(crate) const PEEK_LINGER: Duration = Duration::from_millis(1_500);

/// How long a woken HUD stays after the pointer leaves it.
#[cfg(target_os = "macos")]
const WAKE_LINGER: Duration = Duration::from_secs(3);

/// How long a woken HUD stays, at least.
#[cfg(target_os = "macos")]
const WAKE_HOLD: Duration = Duration::from_millis(4_800);

/// The display edge the HUD docks against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DockEdge {
    Left,
    #[default]
    Right,
    Top,
    Bottom,
}

/// Whether the HUD is docked, and at which edge. The shell stores this so a
/// docked HUD comes back docked.
///
/// `island` means the HUD sits in the notch. The edge is then `Top`, which
/// is where it docks on a display without a notch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockSettings {
    pub docked: bool,
    pub edge: DockEdge,
    #[serde(default)]
    pub island: bool,
}

/// A rectangle in physical desktop pixels.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

pub(crate) struct DockState {
    pub(crate) edge: DockEdge,
    pub(crate) docked: bool,
    /// The HUD sits in the notch. `docked` then means collapsed.
    pub(crate) island: bool,
    /// The reader wants the notch. It stays true while a display change takes
    /// the notch away, so the island comes back when a notch returns.
    pub(crate) island_wanted: bool,
    /// Where the HUD sits when peeked in, in physical desktop pixels.
    pub(crate) home: Option<(f64, f64)>,
    /// The display the HUD docked against.
    #[cfg(target_os = "macos")]
    pub(crate) frame: Option<Rect>,
    /// The display's scale, so the tab is in logical pixels.
    #[cfg(target_os = "macos")]
    pub(crate) scale: f64,
    /// The notch of the built-in display, when one exists.
    #[cfg(target_os = "macos")]
    pub(crate) notch: Option<super::island::Notch>,
    /// A new value cancels every task from an earlier transition.
    pub(crate) generation: u64,
}

static DOCK: Mutex<DockState> = Mutex::new(DockState {
    edge: DockEdge::Right,
    docked: false,
    island: false,
    island_wanted: false,
    home: None,
    #[cfg(target_os = "macos")]
    frame: None,
    #[cfg(target_os = "macos")]
    scale: 1.0,
    #[cfg(target_os = "macos")]
    notch: None,
    generation: 0,
});

/// Lock the dock state.
///
/// Never call into the window while the guard is held. A window getter waits
/// for the main thread, and the main thread takes this lock in sync commands,
/// so a getter under the lock deadlocks the app.
pub(crate) fn state() -> std::sync::MutexGuard<'static, DockState> {
    DOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether the HUD is docked now, and at which edge.
///
/// A peeked HUD still counts as docked: it parks again on its own.
pub fn dock_settings() -> DockSettings {
    let dock = state();
    DockSettings {
        docked: dock.docked || dock.home.is_some(),
        edge: dock.edge,
        island: dock.island || dock.island_wanted,
    }
}

/// Dock again at launch or reopen, when the reader left the HUD docked.
///
/// An island comes back in the notch. Without a notch it docks at the top.
#[cfg(target_os = "macos")]
pub fn restore_dock(app: &AppHandle, settings: DockSettings) {
    if !settings.docked {
        return;
    }
    let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) else {
        return;
    };
    if settings.island && super::island::refresh_notch() && super::island::island_at(app, &window) {
        return;
    }
    dock_at(app, &window, settings.edge);
}

/// Keep the dock inert where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn restore_dock(_app: &tauri::AppHandle, _settings: DockSettings) {}

/// Dock the HUD when a drag dropped it against a display edge.
///
/// Returns the dock state after the drop, for the shell to store.
#[cfg(target_os = "macos")]
pub fn settle_after_drag(app: &AppHandle) -> DockSettings {
    let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) else {
        return dock_settings();
    };
    super::island::refresh_notch();
    if let Some(window_rect) = window_rect(&window)
        && let Some(monitor) = monitor_of(&window)
    {
        let frame = monitor_rect(&monitor);
        if super::island::dropped_on_notch(&window_rect) {
            super::island::island_at(app, &window);
        } else if let Some(edge) =
            edge_dropped_on(&frame, &window_rect, SIDE_INSET * monitor.scale_factor())
        {
            let others: Vec<Rect> = window
                .available_monitors()
                .map(|all| {
                    all.iter()
                        .map(monitor_rect)
                        .filter(|rect| !same_rect(rect, &frame))
                        .collect()
                })
                .unwrap_or_default();
            if edge_is_shared(edge, &frame, &others) {
                // The edge meets another display, so a dock there would hide
                // the HUD on the neighbour. Bring the drop back on screen.
                let (x, y) = clamp_inside(&frame, &window_rect);
                tracing::info!(event = "hud_edge_bounce", edge = ?edge);
                let _guard = super::resize_apply_guard();
                let _ = window.set_position(PhysicalPosition::new(x, y));
            } else {
                dock_at(app, &window, edge);
            }
        }
    }
    // A free drop ends the drag's notch preview.
    {
        let mut dock = state();
        if !dock.docked {
            dock.generation += 1;
        }
    }
    dock_settings()
}

/// Keep the drop inert where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn settle_after_drag(_app: &tauri::AppHandle) -> DockSettings {
    dock_settings()
}

/// Free the HUD: a drag started on a docked, peeked, or islanded HUD.
///
/// Returns true when the HUD was parked. An island window gets the floating
/// width back.
pub fn tear_off(app: &tauri::AppHandle) -> bool {
    let (was_docked, was_island) = {
        let mut dock = state();
        let was_docked = dock.docked || dock.home.is_some();
        #[cfg(target_os = "macos")]
        if was_docked {
            tracing::info!(event = "hud_tear_off", edge = ?dock.edge, island = dock.island);
        }
        let was_island = dock.island;
        dock.docked = false;
        dock.island = false;
        dock.island_wanted = false;
        dock.home = None;
        dock.generation += 1;
        (was_docked, was_island)
    };
    #[cfg(target_os = "macos")]
    if was_island {
        super::island::leave_window(app);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, was_island);
    was_docked
}

/// Dock the HUD at `edge` now. Development menu only.
#[cfg(target_os = "macos")]
pub fn dock_overlay(app: &AppHandle, edge: DockEdge) {
    let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) else {
        return;
    };
    // A HUD parked at another edge jumps home first, so the new dock
    // measures an on-screen frame.
    let parked_home = {
        let dock = state();
        if dock.docked { dock.home } else { None }
    };
    if let Some((x, y)) = parked_home {
        let _guard = super::resize_apply_guard();
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
    tear_off(app);
    dock_at(app, &window, edge);
}

/// Keep the dock inert where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn dock_overlay(_app: &tauri::AppHandle, _edge: DockEdge) {}

/// Bring a docked HUD in for a while. `reason` is for the log only.
#[cfg(target_os = "macos")]
pub fn wake_overlay(app: &AppHandle, reason: &str) {
    let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) else {
        return;
    };
    let (docked, island) = {
        let dock = state();
        (dock.docked, dock.island)
    };
    if !docked {
        return;
    }
    tracing::info!(event = "hud_wake", reason, island);
    if island {
        super::island::expand(app, &window, WAKE_HOLD, WAKE_LINGER);
    } else {
        undock(app, &window, WAKE_HOLD, WAKE_LINGER);
    }
}

/// Keep waking inert where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn wake_overlay(_app: &tauri::AppHandle, _reason: &str) {}

/// Forget the dock position. The window is about to hide.
#[cfg(target_os = "macos")]
pub(crate) fn reset() {
    let mut dock = state();
    dock.docked = false;
    dock.island = false;
    dock.home = None;
    dock.generation += 1;
}

/// Dock again after a placement moved the window on screen.
///
/// The placement is the new home, and the dock edge is the same edge of the
/// display the placement chose. An island goes back to the notch when one is
/// still there, and to the top edge when it is gone.
#[cfg(target_os = "macos")]
pub(crate) fn redock_after_placement(app: &AppHandle, window: &WebviewWindow) {
    let (edge, wants_island, has_notch) = {
        let dock = state();
        if !dock.docked && dock.home.is_none() {
            return;
        }
        (
            dock.edge,
            dock.island || dock.island_wanted,
            dock.notch.is_some(),
        )
    };
    if redock_choice(wants_island, has_notch) == Redock::Island
        && super::island::island_at(app, window)
    {
        return;
    }
    tear_off(app);
    if wants_island {
        // The notch went with the display. The wish stays, so the island
        // returns when a display with a notch does.
        state().island_wanted = true;
    }
    dock_at(app, window, edge);
}

/// What a placement change makes of the HUD. Pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Redock {
    /// Back to the notch.
    Island,
    /// No notch to go to: park at the remembered edge, which an island left
    /// as the top, and keep the wish.
    EdgeKeepingWish,
    /// An ordinary dock.
    Edge,
}

/// Decide what a placement change does with a stored island. Pure.
fn redock_choice(wants_island: bool, has_notch: bool) -> Redock {
    match (wants_island, has_notch) {
        (true, true) => Redock::Island,
        (true, false) => Redock::EdgeKeepingWish,
        (false, _) => Redock::Edge,
    }
}

/// Keep a docked window at its tab after its height changed.
///
/// The caller holds the resize guard, so this writes the position directly.
/// An island stays at its home: it only grows down from the notch.
#[cfg(target_os = "macos")]
pub(crate) fn keep_docked_after_resize(window: &WebviewWindow) {
    let island_home = {
        let dock = state();
        if dock.island { Some(dock.home) } else { None }
    };
    if let Some(home) = island_home {
        if let Some((x, y)) = home {
            let _ = window.set_position(PhysicalPosition::new(x, y));
        }
        return;
    }
    let (edge, frame, scale) = {
        let dock = state();
        let Some(frame) = dock.frame.filter(|_| dock.docked) else {
            return;
        };
        (dock.edge, frame, dock.scale)
    };
    let Some(window_rect) = window_rect(window) else {
        return;
    };
    let target = docked_position(edge, &frame, &window_rect, tab_depth(edge, scale));
    // The height sets the flush position at the bottom edge. A HUD docked at
    // launch, before the renderer reports its height, has a stale home.
    {
        let mut dock = state();
        if dock.docked && dock.home.is_some() {
            dock.home = Some(flush_position(edge, &frame, &window_rect));
        }
    }
    let _ = window.set_position(PhysicalPosition::new(target.0, target.1));
}

/// Park the HUD at `edge` of the display it is on.
///
/// Home becomes the position flush inside that edge, so a peek brings the
/// whole HUD on screen even after a drop that went past the edge.
#[cfg(target_os = "macos")]
fn dock_at(app: &AppHandle, window: &WebviewWindow, edge: DockEdge) {
    if state().docked {
        return;
    }
    let Some(current) = window_rect(window) else {
        return;
    };
    let Some(monitor) = monitor_of(window) else {
        return;
    };
    let frame = monitor_rect(&monitor);
    let scale = monitor.scale_factor();
    let (start, target, generation) = {
        let mut dock = state();
        if dock.docked {
            return;
        }
        let home = dock
            .home
            .unwrap_or_else(|| flush_position(edge, &frame, &current));
        dock.edge = edge;
        dock.home = Some(home);
        dock.frame = Some(frame);
        dock.scale = scale;
        dock.docked = true;
        dock.generation += 1;
        let target = docked_position(edge, &frame, &current, tab_depth(edge, scale));
        ((current.x, current.y), target, dock.generation)
    };
    super::hide_detail(app);
    tracing::info!(event = "hud_dock", edge = ?edge, x = target.0, y = target.1);
    let watcher = (app.clone(), window.clone());
    slide(window.clone(), start, target, generation, move || {
        let parked = window_rect(&watcher.1).map(|rect| (rect.x, rect.y));
        tracing::info!(event = "hud_dock_parked", ?parked, generation);
        spawn_tab_watcher(watcher.0, watcher.1, generation);
    });
}

/// Slide a docked HUD in, then park it again after a quiet spell.
///
/// `hold` is the least time the HUD stays. `linger` is how long it stays
/// after the pointer leaves it.
#[cfg(target_os = "macos")]
fn undock(app: &AppHandle, window: &WebviewWindow, hold: Duration, linger: Duration) {
    if !state().docked {
        return;
    }
    let Some(window_rect) = window_rect(window) else {
        return;
    };
    let (start, home, generation) = {
        let mut dock = state();
        if !dock.docked {
            return;
        }
        let Some(home) = dock.home else {
            dock.docked = false;
            return;
        };
        dock.docked = false;
        dock.generation += 1;
        ((window_rect.x, window_rect.y), home, dock.generation)
    };
    let after = (app.clone(), window.clone());
    slide(window.clone(), start, home, generation, move || {
        spawn_auto_dock(after.0, after.1, generation, hold, linger);
    });
}

/// Move the window from `from` to `to` over the slide duration.
///
/// A window that is not on screen jumps, so the reader never sees a slide at
/// launch. `done` runs after the last frame, only when no later transition
/// cancelled this one.
#[cfg(target_os = "macos")]
fn slide(
    window: WebviewWindow,
    from: (f64, f64),
    to: (f64, f64),
    generation: u64,
    done: impl FnOnce() + Send + 'static,
) {
    if !window.is_visible().unwrap_or(false) {
        {
            let _guard = super::resize_apply_guard();
            let _ = window.set_position(PhysicalPosition::new(to.0, to.1));
        }
        done();
        return;
    }
    tauri::async_runtime::spawn(async move {
        let step = SLIDE_DURATION / SLIDE_STEPS;
        for frame in 1..=SLIDE_STEPS {
            tokio::time::sleep(step).await;
            let _guard = super::resize_apply_guard();
            if state().generation != generation {
                return;
            }
            let progress = super::ease_out(f64::from(frame) / f64::from(SLIDE_STEPS));
            let x = from.0 + (to.0 - from.0) * progress;
            let y = from.1 + (to.1 - from.1) * progress;
            if let Err(error) = window.set_position(PhysicalPosition::new(x, y)) {
                tracing::warn!(error = %error, "HUD slide frame failed; applying the final frame");
                let _ = window.set_position(PhysicalPosition::new(to.0, to.1));
                break;
            }
        }
        done();
    });
}

/// Watch for the cursor resting on the tab while docked.
#[cfg(target_os = "macos")]
fn spawn_tab_watcher(app: AppHandle, window: WebviewWindow, generation: u64) {
    tauri::async_runtime::spawn(async move {
        let mut on_tab_since: Option<Instant> = None;
        loop {
            tokio::time::sleep(TAB_POLL).await;
            {
                let dock = state();
                if dock.generation != generation || !dock.docked {
                    return;
                }
            }
            let (edge, docked_frame, scale) = {
                let dock = state();
                (dock.edge, dock.frame, dock.scale)
            };
            let on_strip = match (docked_frame, window.cursor_position().ok()) {
                (Some(strip), Some(cursor)) => {
                    on_tab_strip(edge, &strip, tab_depth(edge, scale), (cursor.x, cursor.y))
                }
                _ => false,
            };
            if !on_strip {
                on_tab_since = None;
                continue;
            }
            if on_tab_since.is_none() {
                tracing::info!(event = "hud_tab_hover", generation);
            }
            let since = *on_tab_since.get_or_insert_with(Instant::now);
            if since.elapsed() < TAB_HOLD {
                continue;
            }
            tracing::info!(event = "hud_peek");
            undock(&app, &window, Duration::ZERO, PEEK_LINGER);
            return;
        }
    });
}

/// Park the peeked HUD once the hold passed and the pointer left it.
///
/// A pointer held on the tab strip counts as on the HUD. The strip is what
/// woke the HUD, so resting there keeps it open.
#[cfg(target_os = "macos")]
fn spawn_auto_dock(
    app: AppHandle,
    window: WebviewWindow,
    generation: u64,
    hold: Duration,
    linger: Duration,
) {
    let on_strip = |window: &WebviewWindow| {
        let (edge, docked_frame, scale) = {
            let dock = state();
            (dock.edge, dock.frame, dock.scale)
        };
        match (docked_frame, window.cursor_position().ok()) {
            (Some(strip), Some(cursor)) => {
                on_tab_strip(edge, &strip, tab_depth(edge, scale), (cursor.x, cursor.y))
            }
            _ => false,
        }
    };
    spawn_auto_park(
        app,
        window,
        generation,
        hold,
        linger,
        on_strip,
        move |app, window| {
            let edge = state().edge;
            tracing::info!(event = "hud_auto_dock", generation);
            dock_at(app, window, edge);
        },
    );
}

/// Park an open HUD once the hold passed and the pointer left it.
///
/// `near` says whether the pointer counts as on the HUD when it is outside
/// the window. `park` runs once, when the HUD should park. The watch ends
/// when a later transition changes the generation.
#[cfg(target_os = "macos")]
pub(crate) fn spawn_auto_park(
    app: AppHandle,
    window: WebviewWindow,
    generation: u64,
    hold: Duration,
    linger: Duration,
    near: impl Fn(&WebviewWindow) -> bool + Send + 'static,
    park: impl FnOnce(&AppHandle, &WebviewWindow) + Send + 'static,
) {
    tauri::async_runtime::spawn(async move {
        let start = Instant::now();
        let mut last_inside = start;
        loop {
            tokio::time::sleep(AUTO_DOCK_POLL).await;
            {
                let dock = state();
                if dock.generation != generation || dock.docked || dock.home.is_none() {
                    return;
                }
            }
            if app.get_webview_window(super::OVERLAY_LABEL).is_none() {
                return;
            }
            let now = Instant::now();
            if near(&window) || super::cursor_inside(&window).unwrap_or(false) {
                last_inside = now;
                continue;
            }
            if should_dock(now, start, hold, linger, last_inside) {
                park(&app, &window);
                return;
            }
        }
    });
}

/// The window frame in physical desktop pixels.
#[cfg(target_os = "macos")]
pub(crate) fn window_rect(window: &WebviewWindow) -> Option<Rect> {
    let position = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    Some(Rect {
        x: f64::from(position.x),
        y: f64::from(position.y),
        width: f64::from(size.width),
        height: f64::from(size.height),
    })
}

/// The display the window is on, or the primary one for a window off screen.
#[cfg(target_os = "macos")]
pub(crate) fn monitor_of(window: &WebviewWindow) -> Option<Monitor> {
    window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
}

#[cfg(target_os = "macos")]
pub(crate) fn monitor_rect(monitor: &Monitor) -> Rect {
    Rect {
        x: f64::from(monitor.position().x),
        y: f64::from(monitor.position().y),
        width: f64::from(monitor.size().width),
        height: f64::from(monitor.size().height),
    }
}

/// How far the window reaches into the screen at `edge`, in physical pixels.
///
/// A side tab adds the window's transparent side gap, so the frame shows.
#[cfg(any(target_os = "macos", test))]
fn tab_depth(edge: DockEdge, scale: f64) -> f64 {
    match edge {
        DockEdge::Left | DockEdge::Right => (TAB + SIDE_INSET) * scale,
        DockEdge::Top | DockEdge::Bottom => TAB * scale,
    }
}

/// Where the window sits outside `frame` at `edge` with `tab` showing. Pure.
#[cfg(any(target_os = "macos", test))]
fn docked_position(edge: DockEdge, frame: &Rect, window: &Rect, tab: f64) -> (f64, f64) {
    match edge {
        DockEdge::Left => (frame.x - window.width + tab, window.y),
        DockEdge::Right => (frame.x + frame.width - tab, window.y),
        DockEdge::Top => (window.x, frame.y - window.height + tab),
        DockEdge::Bottom => (window.x, frame.y + frame.height - tab),
    }
}

/// Where the window sits fully inside `frame`, flush against `edge`. Pure.
#[cfg(any(target_os = "macos", test))]
fn flush_position(edge: DockEdge, frame: &Rect, window: &Rect) -> (f64, f64) {
    match edge {
        DockEdge::Left => (frame.x, window.y),
        DockEdge::Right => (frame.x + frame.width - window.width, window.y),
        DockEdge::Top => (window.x, frame.y),
        DockEdge::Bottom => (window.x, frame.y + frame.height - window.height),
    }
}

/// True when `cursor` rests in the strip `tab` deep along `edge`, anywhere
/// on that edge of `frame`. The strip, not the window, is the tab. Pure.
#[cfg(any(target_os = "macos", test))]
fn on_tab_strip(edge: DockEdge, frame: &Rect, tab: f64, cursor: (f64, f64)) -> bool {
    let (x, y) = cursor;
    let inside_x = x >= frame.x && x < frame.x + frame.width;
    let inside_y = y >= frame.y && y < frame.y + frame.height;
    match edge {
        DockEdge::Left => inside_y && x >= frame.x && x < frame.x + tab,
        DockEdge::Right => {
            inside_y && x >= frame.x + frame.width - tab && x < frame.x + frame.width
        }
        DockEdge::Top => inside_x && y >= frame.y && y < frame.y + tab,
        DockEdge::Bottom => {
            inside_x && y >= frame.y + frame.height - tab && y < frame.y + frame.height
        }
    }
}

#[cfg(target_os = "macos")]
fn same_rect(left: &Rect, right: &Rect) -> bool {
    left.x == right.x
        && left.y == right.y
        && left.width == right.width
        && left.height == right.height
}

/// True when another display sits against `edge` of `frame`. Pure.
///
/// The displays share the edge when one's far side meets the other's near
/// side and their spans overlap along that edge.
#[cfg(any(target_os = "macos", test))]
fn edge_is_shared(edge: DockEdge, frame: &Rect, others: &[Rect]) -> bool {
    let spans_x = |other: &Rect| other.x < frame.x + frame.width && other.x + other.width > frame.x;
    let spans_y =
        |other: &Rect| other.y < frame.y + frame.height && other.y + other.height > frame.y;
    others.iter().any(|other| match edge {
        DockEdge::Left => (other.x + other.width - frame.x).abs() < 1.0 && spans_y(other),
        DockEdge::Right => (other.x - (frame.x + frame.width)).abs() < 1.0 && spans_y(other),
        DockEdge::Top => (other.y + other.height - frame.y).abs() < 1.0 && spans_x(other),
        DockEdge::Bottom => (other.y - (frame.y + frame.height)).abs() < 1.0 && spans_x(other),
    })
}

/// The nearest position that keeps the whole window inside `frame`. Pure.
#[cfg(any(target_os = "macos", test))]
fn clamp_inside(frame: &Rect, window: &Rect) -> (f64, f64) {
    let max_x = (frame.x + frame.width - window.width).max(frame.x);
    let max_y = (frame.y + frame.height - window.height).max(frame.y);
    (
        window.x.clamp(frame.x, max_x),
        window.y.clamp(frame.y, max_y),
    )
}

/// The edge a dropped window's frame went past, deepest first. Pure.
///
/// A window that stops short of the edge stays free, so a HUD can sit near an
/// edge without docking. `side_inset` is the window's transparent side gap:
/// the frame is past a side edge only when the window is past it by more.
#[cfg(any(target_os = "macos", test))]
fn edge_dropped_on(frame: &Rect, window: &Rect, side_inset: f64) -> Option<DockEdge> {
    let gaps = [
        (DockEdge::Left, window.x - frame.x),
        (
            DockEdge::Right,
            frame.x + frame.width - (window.x + window.width),
        ),
        (DockEdge::Top, window.y - frame.y),
        (
            DockEdge::Bottom,
            frame.y + frame.height - (window.y + window.height),
        ),
    ];
    gaps.into_iter()
        .filter(|(edge, gap)| {
            let limit = match edge {
                DockEdge::Left | DockEdge::Right => -side_inset,
                DockEdge::Top | DockEdge::Bottom => 0.0,
            };
            *gap < limit
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(edge, _)| edge)
}

/// Whether the peeked HUD should park: the hold passed, and the pointer has
/// been off it for the linger. Pure.
#[cfg(any(target_os = "macos", test))]
fn should_dock(
    now: Instant,
    start: Instant,
    hold: Duration,
    linger: Duration,
    last_inside: Instant,
) -> bool {
    now.duration_since(start) >= hold && now.duration_since(last_inside) >= linger
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Rect = Rect {
        x: 100.0,
        y: 50.0,
        width: 1000.0,
        height: 600.0,
    };
    const WINDOW: Rect = Rect {
        x: 500.0,
        y: 80.0,
        width: 176.0,
        height: 60.0,
    };

    #[test]
    fn side_tabs_add_the_transparent_gap() {
        assert_eq!(tab_depth(DockEdge::Left, 2.0), 28.0);
        assert_eq!(tab_depth(DockEdge::Right, 1.0), 14.0);
        assert_eq!(tab_depth(DockEdge::Top, 2.0), 12.0);
        assert_eq!(tab_depth(DockEdge::Bottom, 1.0), 6.0);
    }

    #[test]
    fn docked_positions_leave_only_the_tab_on_screen() {
        assert_eq!(
            docked_position(DockEdge::Left, &FRAME, &WINDOW, TAB),
            (-70.0, 80.0)
        );
        assert_eq!(
            docked_position(DockEdge::Right, &FRAME, &WINDOW, TAB),
            (1094.0, 80.0)
        );
        assert_eq!(
            docked_position(DockEdge::Top, &FRAME, &WINDOW, TAB),
            (500.0, -4.0)
        );
        assert_eq!(
            docked_position(DockEdge::Bottom, &FRAME, &WINDOW, TAB),
            (500.0, 644.0)
        );
    }

    #[test]
    fn flush_positions_sit_inside_the_edge() {
        assert_eq!(
            flush_position(DockEdge::Left, &FRAME, &WINDOW),
            (100.0, 80.0)
        );
        assert_eq!(
            flush_position(DockEdge::Right, &FRAME, &WINDOW),
            (924.0, 80.0)
        );
        assert_eq!(
            flush_position(DockEdge::Top, &FRAME, &WINDOW),
            (500.0, 50.0)
        );
        assert_eq!(
            flush_position(DockEdge::Bottom, &FRAME, &WINDOW),
            (500.0, 590.0)
        );
    }

    #[test]
    fn only_a_drop_past_an_edge_docks_there() {
        let mid = WINDOW;
        assert_eq!(edge_dropped_on(&FRAME, &mid, SIDE_INSET), None);
        // Near the edge, still on screen: free.
        let near_right = Rect { x: 920.0, ..WINDOW };
        assert_eq!(edge_dropped_on(&FRAME, &near_right, SIDE_INSET), None);
        // Only the transparent side gap is off screen: free.
        let gap_off = Rect { x: 928.0, ..WINDOW };
        assert_eq!(edge_dropped_on(&FRAME, &gap_off, SIDE_INSET), None);
        let past_right = Rect { x: 940.0, ..WINDOW };
        assert_eq!(
            edge_dropped_on(&FRAME, &past_right, SIDE_INSET),
            Some(DockEdge::Right)
        );
        let flush_top = Rect { y: 50.0, ..WINDOW };
        assert_eq!(edge_dropped_on(&FRAME, &flush_top, SIDE_INSET), None);
        let past_top = Rect { y: 40.0, ..WINDOW };
        assert_eq!(
            edge_dropped_on(&FRAME, &past_top, SIDE_INSET),
            Some(DockEdge::Top)
        );
        // A corner picks the edge the window is further past.
        let corner = Rect {
            x: 80.0,
            y: 45.0,
            ..WINDOW
        };
        assert_eq!(
            edge_dropped_on(&FRAME, &corner, SIDE_INSET),
            Some(DockEdge::Left)
        );
    }

    #[test]
    fn a_placement_change_puts_a_stored_island_back_on_the_notch() {
        assert_eq!(redock_choice(true, true), Redock::Island);
    }

    #[test]
    fn a_placement_change_without_a_notch_keeps_the_island_wish() {
        // The display carrying the notch left. The HUD parks at the edge it
        // remembers, and the wish waits for a notch to come back.
        assert_eq!(redock_choice(true, false), Redock::EdgeKeepingWish);
    }

    #[test]
    fn a_placement_change_leaves_an_ordinary_dock_alone() {
        assert_eq!(redock_choice(false, true), Redock::Edge);
        assert_eq!(redock_choice(false, false), Redock::Edge);
    }

    #[test]
    fn a_shared_display_edge_bounces_instead_of_docking() {
        let right_neighbour = Rect {
            x: 1100.0,
            y: 200.0,
            width: 800.0,
            height: 500.0,
        };
        let far_away = Rect {
            x: 1100.0,
            y: 700.0,
            width: 800.0,
            height: 500.0,
        };
        assert!(edge_is_shared(DockEdge::Right, &FRAME, &[right_neighbour]));
        assert!(!edge_is_shared(DockEdge::Right, &FRAME, &[far_away]));
        assert!(!edge_is_shared(DockEdge::Left, &FRAME, &[right_neighbour]));
        let above = Rect {
            x: 300.0,
            y: -550.0,
            width: 800.0,
            height: 600.0,
        };
        assert!(edge_is_shared(DockEdge::Top, &FRAME, &[above]));
        assert!(!edge_is_shared(DockEdge::Bottom, &FRAME, &[above]));
        let past_right = Rect {
            x: 1050.0,
            ..WINDOW
        };
        assert_eq!(clamp_inside(&FRAME, &past_right), (924.0, 80.0));
        assert_eq!(clamp_inside(&FRAME, &WINDOW), (500.0, 80.0));
    }

    #[test]
    fn the_tab_strip_runs_the_whole_edge() {
        assert!(on_tab_strip(DockEdge::Right, &FRAME, 16.0, (1090.0, 600.0)));
        assert!(on_tab_strip(DockEdge::Right, &FRAME, 16.0, (1099.0, 51.0)));
        assert!(!on_tab_strip(
            DockEdge::Right,
            &FRAME,
            16.0,
            (1080.0, 300.0)
        ));
        assert!(!on_tab_strip(
            DockEdge::Right,
            &FRAME,
            16.0,
            (1090.0, 700.0)
        ));
        assert!(on_tab_strip(DockEdge::Left, &FRAME, 16.0, (100.0, 300.0)));
        assert!(on_tab_strip(DockEdge::Top, &FRAME, 8.0, (500.0, 57.0)));
        assert!(!on_tab_strip(DockEdge::Top, &FRAME, 8.0, (500.0, 58.0)));
        assert!(on_tab_strip(DockEdge::Bottom, &FRAME, 8.0, (500.0, 649.0)));
    }

    #[test]
    fn auto_dock_waits_for_the_hold_and_the_linger() {
        let start = Instant::now();
        let hold = Duration::from_secs(5);
        let linger = Duration::from_secs(3);
        assert!(!should_dock(
            start + Duration::from_secs(4),
            start,
            hold,
            linger,
            start
        ));
        assert!(should_dock(
            start + Duration::from_secs(5),
            start,
            hold,
            linger,
            start
        ));
        let hovered = start + Duration::from_secs(4);
        assert!(!should_dock(
            start + Duration::from_secs(6),
            start,
            hold,
            linger,
            hovered
        ));
        assert!(should_dock(
            start + Duration::from_secs(7),
            start,
            hold,
            linger,
            hovered
        ));
        assert!(should_dock(
            start + linger,
            start,
            Duration::ZERO,
            linger,
            start
        ));
    }

    #[test]
    fn dock_settings_round_trip_in_camel_case() {
        let settings: DockSettings =
            serde_json::from_str("{\"docked\":true,\"edge\":\"left\"}").unwrap();
        assert_eq!(
            settings,
            DockSettings {
                docked: true,
                edge: DockEdge::Left,
                island: false
            }
        );
        assert_eq!(
            serde_json::to_string(&settings).unwrap(),
            "{\"docked\":true,\"edge\":\"left\",\"island\":false}"
        );
    }

    #[test]
    fn stored_settings_without_the_island_field_read_as_no_island() {
        let settings: DockSettings =
            serde_json::from_str("{\"docked\":true,\"edge\":\"top\",\"island\":true}").unwrap();
        assert!(settings.island);
        assert_eq!(settings.edge, DockEdge::Top);
    }
}
