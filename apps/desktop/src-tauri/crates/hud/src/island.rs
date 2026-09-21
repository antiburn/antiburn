//! Island: the HUD lives in the notch of a built-in display.
//!
//! Collapsed, the window is the notch row: the notch plus one wing either
//! side, pure black, so the black and the cutout read as one shape. The
//! pointer resting in the notch or on a wing expands the island downward with
//! the full HUD. It collapses again after a quiet spell. Everything else is
//! the edge dock's behaviour: a wake expands it, a drag tears it off, and a
//! drop over the notch brings it back.
//!
//! The notch is read from `NSScreen` on the main thread and cached in the
//! dock state. Every other function reads the cache, so watchers on the
//! async runtime never touch AppKit.

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

use serde::Serialize;
#[cfg(target_os = "macos")]
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewWindow};

#[cfg(any(target_os = "macos", test))]
use super::dock::Rect;
#[cfg(target_os = "macos")]
use super::dock::{self, state};

/// The drawable strip either side of the notch, in logical pixels.
///
/// The reference notch apps use `(notch height - 12) + 10`: 30 for a 32 px
/// notch. Room for one 16 px element and its padding.
#[cfg(any(target_os = "macos", test))]
pub(crate) const WING: f64 = 30.0;

/// A transparent gutter either side of the wings, in logical pixels.
///
/// The webview draws the top corners curving outward into the screen edge
/// inside this gutter. It is the expanded radius, the larger of the two.
#[cfg(any(target_os = "macos", test))]
pub(crate) const FILLET: f64 = 19.0;

/// How far the notch spreads over the cutout, in logical pixels.
///
/// The cutout's edge is anti-aliased. Two extra pixels a side keep it black.
#[cfg(any(target_os = "macos", test))]
const NOTCH_SPREAD: f64 = 4.0;

/// How far the hotspot reaches past the notch's sides, in logical pixels.
#[cfg(any(target_os = "macos", test))]
const HOTSPOT_SIDE: f64 = 10.0;

/// How far the hotspot reaches below the notch, in logical pixels.
#[cfg(any(target_os = "macos", test))]
const HOTSPOT_BELOW: f64 = 5.0;

/// The notch the development menu pretends a display has, in logical pixels.
#[cfg(target_os = "macos")]
const FAKE_NOTCH: (f64, f64) = (200.0, 32.0);

/// How often a drag checks whether a drop would make the island.
#[cfg(target_os = "macos")]
const DRAG_POLL: Duration = Duration::from_millis(50);

/// The event that carries the island state to the HUD webview.
#[cfg(target_os = "macos")]
const ISLAND_STATE_EVENT: &str = "hud-island:state";

/// Whether the development menu fakes a notch on the primary display.
#[cfg(target_os = "macos")]
static FAKE_NOTCH_ON: AtomicBool = AtomicBool::new(false);

/// What the island is doing, as the webview renders it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IslandPhase {
    /// The HUD is floating or edge-docked.
    Off,
    /// A drag is over the notch: a drop now makes the island.
    Preview,
    /// The notch row alone.
    Collapsed,
    /// The full HUD below the notch row.
    Expanded,
}

/// The island state the webview renders, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandState {
    pub island: IslandPhase,
    /// The width of one wing.
    pub wing: f64,
    /// The transparent gutter outside each wing.
    pub fillet: f64,
    /// The notch width, which the webview leaves empty.
    pub notch: f64,
    /// The notch height: the collapsed row.
    pub height: f64,
}

impl IslandState {
    /// The state with no island, for every platform.
    pub const fn off() -> Self {
        Self {
            island: IslandPhase::Off,
            wing: 0.0,
            fillet: 0.0,
            notch: 0.0,
            height: 0.0,
        }
    }
}

/// A notch in physical desktop pixels, with its display's scale.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Notch {
    pub(crate) rect: Rect,
    pub(crate) scale: f64,
}

#[cfg(any(target_os = "macos", test))]
impl Notch {
    /// The state the webview renders for this notch in `phase`.
    fn state(&self, phase: IslandPhase) -> IslandState {
        IslandState {
            island: phase,
            wing: WING,
            fillet: FILLET,
            notch: self.rect.width / self.scale,
            height: self.rect.height / self.scale,
        }
    }

    /// The window width in logical pixels: the notch, two wings, two gutters.
    fn window_width(&self) -> f64 {
        self.rect.width / self.scale + 2.0 * (WING + FILLET)
    }
}

/* -------------------------------------------------------------------------
 * Pure geometry, in physical desktop pixels
 * ---------------------------------------------------------------------- */

/// The notch of a display, or `None` when the display has none.
///
/// `display` is the display frame. `safe_top` is the top safe-area inset and
/// `aux_left` and `aux_right` are the widths of the areas either side of the
/// camera housing, all in logical pixels. The notch is the gap between the
/// two areas, spread over the cutout's edge. Pure.
#[cfg(any(target_os = "macos", test))]
fn notch_rect(
    display: &Rect,
    scale: f64,
    safe_top: f64,
    aux_left: f64,
    aux_right: f64,
) -> Option<Rect> {
    if safe_top <= 0.0 || scale <= 0.0 {
        return None;
    }
    let gap = display.width / scale - aux_left - aux_right;
    if gap <= 0.0 {
        return None;
    }
    Some(Rect {
        x: display.x + (aux_left - NOTCH_SPREAD / 2.0) * scale,
        y: display.y,
        width: (gap + NOTCH_SPREAD) * scale,
        height: safe_top * scale,
    })
}

/// The collapsed window: the notch row with a wing and a gutter each side. Pure.
#[cfg(any(target_os = "macos", test))]
fn island_rect(notch: &Rect, scale: f64) -> Rect {
    let margin = (WING + FILLET) * scale;
    Rect {
        x: notch.x - margin,
        y: notch.y,
        width: notch.width + 2.0 * margin,
        height: notch.height,
    }
}

/// True when `cursor` is in the notch or just around it. Pure.
///
/// The pointer is hidden in the notch, so the hotspot reaches a little past
/// it: a pointer that skims the cutout still counts.
#[cfg(any(target_os = "macos", test))]
fn in_hotspot(notch: &Rect, scale: f64, cursor: (f64, f64)) -> bool {
    let (x, y) = cursor;
    let side = HOTSPOT_SIDE * scale;
    let below = HOTSPOT_BELOW * scale;
    x >= notch.x - side
        && x < notch.x + notch.width + side
        && y >= notch.y
        && y < notch.y + notch.height + below
}

/// True when a window dropped at `window` lands on the notch. Pure.
///
/// The window's top is past the top of the display and its span overlaps the
/// notch. A drop past the top edge that misses the notch is a top dock.
#[cfg(any(target_os = "macos", test))]
fn notch_dropped_on(notch: &Rect, window: &Rect) -> bool {
    window.y < notch.y && window.x < notch.x + notch.width && window.x + window.width > notch.x
}

/* -------------------------------------------------------------------------
 * The notch, from AppKit
 * ---------------------------------------------------------------------- */

/// Read the notch of the first display that has one. Main thread only.
///
/// Off the main thread this returns `None`, so callers refresh the cache
/// from a main-thread context and read the cache elsewhere.
#[cfg(target_os = "macos")]
fn read_notch() -> Option<Notch> {
    use objc2_app_kit::NSScreen;
    use objc2_foundation::MainThreadMarker;

    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    let primary = screens.firstObject()?;
    let primary_height = primary.frame().size.height;
    let real = screens.iter().find_map(|screen| {
        let insets = screen.safeAreaInsets();
        if insets.top <= 0.0 {
            return None;
        }
        let scale = screen.backingScaleFactor();
        let display = desktop_rect(screen.frame(), primary_height, scale);
        let left = screen.auxiliaryTopLeftArea().size.width;
        let right = screen.auxiliaryTopRightArea().size.width;
        notch_rect(&display, scale, insets.top, left, right).map(|rect| Notch { rect, scale })
    });
    if real.is_some() || !FAKE_NOTCH_ON.load(Ordering::Relaxed) {
        return real;
    }
    let scale = primary.backingScaleFactor();
    let display = desktop_rect(primary.frame(), primary_height, scale);
    let (width, height) = FAKE_NOTCH;
    let side = ((display.width / scale - width) / 2.0).max(0.0);
    notch_rect(&display, scale, height, side, side).map(|rect| Notch { rect, scale })
}

/// A Cocoa screen frame as a desktop rect in physical pixels.
///
/// Cocoa measures from the bottom-left of the primary display. The desktop,
/// as the window and cursor report it, measures from the top-left of the
/// primary display, scaled by the display's own factor.
#[cfg(target_os = "macos")]
fn desktop_rect(frame: objc2_foundation::NSRect, primary_height: f64, scale: f64) -> Rect {
    Rect {
        x: frame.origin.x * scale,
        y: (primary_height - frame.origin.y - frame.size.height) * scale,
        width: frame.size.width * scale,
        height: frame.size.height * scale,
    }
}

/// Read the notch again and cache it. Returns true when a display has one.
///
/// Main thread only: off it, the cache keeps its last value and the result
/// says whether that value is a notch.
#[cfg(target_os = "macos")]
pub fn refresh_notch() -> bool {
    let read = objc2_foundation::MainThreadMarker::new().map(|_| read_notch());
    let mut dock = state();
    if let Some(notch) = read {
        dock.notch = notch;
    }
    dock.notch.is_some()
}

/// Keep the notch absent where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn refresh_notch() -> bool {
    false
}

/// True when the reader wants the notch and the HUD is not there.
///
/// A display change can take the notch away and leave the HUD at the top
/// edge. The shell asks with this before it reads the notch again.
#[cfg(target_os = "macos")]
pub fn island_wanted_off_notch() -> bool {
    let dock = state();
    dock.island_wanted && !dock.island
}

/// Keep the question answerable where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn island_wanted_off_notch() -> bool {
    false
}

/// Put the HUD back on the notch after a notch came back.
///
/// The caller reads the notch again on the main thread first. Returns true
/// when the island came back.
#[cfg(target_os = "macos")]
pub fn reclaim_island(app: &AppHandle) -> bool {
    {
        let dock = state();
        if !dock.island_wanted || dock.island || dock.notch.is_none() {
            return false;
        }
    }
    let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) else {
        return false;
    };
    dock::tear_off(app);
    if island_at(app, &window) {
        tracing::info!(event = "hud_island_reclaimed");
        return true;
    }
    // The island did not take. Hold the wish for the next try.
    state().island_wanted = true;
    false
}

/// Keep the island unavailable where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn reclaim_island(_app: &tauri::AppHandle) -> bool {
    false
}

/// Pretend the primary display has a notch, or stop pretending. Development
/// menu only. Returns the new setting.
///
/// The event tells the settings pane that the notch came or went, because the
/// pane reads the notch one time when it opens.
#[cfg(target_os = "macos")]
pub fn set_fake_notch(app: &AppHandle, on: bool) -> bool {
    FAKE_NOTCH_ON.store(on, Ordering::Relaxed);
    refresh_notch();
    emit(app, island_state());
    on
}

/// Keep the fake notch inert where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn set_fake_notch(_app: &AppHandle, _on: bool) -> bool {
    false
}

/* -------------------------------------------------------------------------
 * State
 * ---------------------------------------------------------------------- */

/// The island state the webview should render now.
#[cfg(target_os = "macos")]
pub fn island_state() -> IslandState {
    let dock = state();
    match dock.notch {
        Some(notch) if dock.island => notch.state(if dock.docked {
            IslandPhase::Collapsed
        } else {
            IslandPhase::Expanded
        }),
        _ => IslandState::off(),
    }
}

/// Keep the island off where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn island_state() -> IslandState {
    IslandState::off()
}

/// The window width in logical pixels while the island is on, or `None`.
#[cfg(target_os = "macos")]
pub(crate) fn frame_width() -> Option<f64> {
    let dock = state();
    dock.notch
        .filter(|_| dock.island)
        .map(|notch| notch.window_width())
}

/// Send the current island state to the HUD webview.
#[cfg(target_os = "macos")]
fn emit_state(app: &AppHandle) {
    emit(app, island_state());
}

#[cfg(target_os = "macos")]
fn emit(app: &AppHandle, state: IslandState) {
    // Every window hears it: the HUD draws the shape, Settings shows the switch.
    if let Err(error) = app.emit(ISLAND_STATE_EVENT, state) {
        tracing::warn!(event = "hud_island_emit_failed", error = %error);
    }
}

/* -------------------------------------------------------------------------
 * Transitions
 * ---------------------------------------------------------------------- */

/// Put the HUD in the notch now. Settings and the development menu.
///
/// Main thread only, because it reads the notch. Returns false when no
/// display has a notch, and the HUD stays as it is.
#[cfg(target_os = "macos")]
pub fn island_overlay(app: &AppHandle) -> bool {
    if !refresh_notch() {
        return false;
    }
    let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) else {
        return false;
    };
    dock::tear_off(app);
    island_at(app, &window)
}

/// Keep the island inert where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn island_overlay(_app: &tauri::AppHandle) -> bool {
    false
}

/// Park the HUD in the cached notch, collapsed. Returns false without one.
///
/// The move is a jump, not a slide: the drop and the restore both want the
/// island in place at once.
#[cfg(target_os = "macos")]
pub(crate) fn island_at(app: &AppHandle, window: &WebviewWindow) -> bool {
    let Some(monitor) = dock::monitor_of(window) else {
        return false;
    };
    let frame = dock::monitor_rect(&monitor);
    let (notch, generation) = {
        let mut dock = state();
        let Some(notch) = dock.notch else {
            return false;
        };
        let rect = island_rect(&notch.rect, notch.scale);
        dock.island = true;
        dock.island_wanted = true;
        dock.docked = true;
        dock.edge = dock::DockEdge::Top;
        dock.home = Some((rect.x, rect.y));
        dock.frame = Some(frame);
        dock.scale = notch.scale;
        dock.generation += 1;
        (notch, dock.generation)
    };
    super::hide_detail(app);
    let rect = island_rect(&notch.rect, notch.scale);
    tracing::info!(
        event = "hud_island",
        x = rect.x,
        y = rect.y,
        width = rect.width
    );
    fit_window(
        window,
        &rect,
        notch.window_width(),
        notch.rect.height / notch.scale,
    );
    emit_state(app);
    spawn_hotspot_watcher(app.clone(), window.clone(), generation);
    true
}

/// Move the window to `rect` and give it the island's logical size.
#[cfg(target_os = "macos")]
fn fit_window(window: &WebviewWindow, rect: &Rect, width: f64, height: f64) {
    let _guard = super::resize_apply_guard();
    let _ = window.set_resizable(true);
    let _ = window.set_size(LogicalSize::new(width, height));
    let _ = window.set_resizable(false);
    let _ = window.set_position(PhysicalPosition::new(rect.x, rect.y));
    super::record_window_height(window);
}

/// Give a window that leaves the island the floating width again.
///
/// The webview measures its content at the window's width, so a window left
/// at the island's width would keep the floating frame stretched. The
/// narrower window centres on the pointer, so a drag out of the island keeps
/// the pointer on the panel.
#[cfg(target_os = "macos")]
pub(crate) fn leave_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) {
        let scale = window.scale_factor().unwrap_or(1.0);
        let height = window
            .outer_size()
            .map_or(super::OVERLAY_SEED_HEIGHT, |size| {
                f64::from(size.height) / scale
            });
        let position = window
            .outer_position()
            .ok()
            .zip(window.cursor_position().ok())
            .map(|(origin, cursor)| {
                let half = (super::OVERLAY_WIDTH * scale / 2.0).round() as i32;
                PhysicalPosition::new((cursor.x.round() as i32) - half, origin.y)
            });
        let _guard = super::resize_apply_guard();
        let _ = window.set_resizable(true);
        let _ = window.set_size(LogicalSize::new(super::OVERLAY_WIDTH, height));
        let _ = window.set_resizable(false);
        if let Some(position) = position {
            let _ = window.set_position(position);
        }
    }
    emit(app, IslandState::off());
}

/// Show the full HUD below the notch for a while.
///
/// `hold` is the least time it stays. `linger` is how long it stays after
/// the pointer leaves the island and the hotspot.
#[cfg(target_os = "macos")]
pub(crate) fn expand(app: &AppHandle, window: &WebviewWindow, hold: Duration, linger: Duration) {
    let generation = {
        let mut dock = state();
        if !dock.island || !dock.docked || dock.notch.is_none() {
            return;
        }
        dock.docked = false;
        dock.generation += 1;
        dock.generation
    };
    tracing::info!(event = "hud_island_expand");
    emit_state(app);
    dock::spawn_auto_park(
        app.clone(),
        window.clone(),
        generation,
        hold,
        linger,
        cursor_in_hotspot,
        move |app, window| {
            tracing::info!(event = "hud_island_collapse", generation);
            collapse(app, window);
        },
    );
}

/// Open a collapsed island now: a mouse-down on it. It lingers as a peek does.
#[cfg(target_os = "macos")]
pub fn expand_island(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) {
        tracing::info!(event = "hud_island_press");
        expand(app, &window, Duration::ZERO, dock::PEEK_LINGER);
    }
}

/// Keep the island inert where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn expand_island(_app: &tauri::AppHandle) {}

/// Fold the island back to the notch row.
#[cfg(target_os = "macos")]
fn collapse(app: &AppHandle, window: &WebviewWindow) {
    let generation = {
        let mut dock = state();
        if !dock.island || dock.docked {
            return;
        }
        dock.docked = true;
        dock.generation += 1;
        dock.generation
    };
    super::hide_detail(app);
    emit_state(app);
    spawn_hotspot_watcher(app.clone(), window.clone(), generation);
}

/// True when the pointer is in the cached notch's hotspot.
#[cfg(target_os = "macos")]
fn cursor_in_hotspot(window: &WebviewWindow) -> bool {
    let notch = state().notch;
    match (notch, window.cursor_position().ok()) {
        (Some(notch), Some(cursor)) => in_hotspot(&notch.rect, notch.scale, (cursor.x, cursor.y)),
        _ => false,
    }
}

/// Watch for the pointer resting in the notch or on a wing while collapsed.
#[cfg(target_os = "macos")]
fn spawn_hotspot_watcher(app: AppHandle, window: WebviewWindow, generation: u64) {
    tauri::async_runtime::spawn(async move {
        let mut since: Option<Instant> = None;
        loop {
            tokio::time::sleep(dock::TAB_POLL).await;
            {
                let dock = state();
                if dock.generation != generation || !dock.docked || !dock.island {
                    return;
                }
            }
            if app.get_webview_window(super::OVERLAY_LABEL).is_none() {
                return;
            }
            let near = cursor_in_hotspot(&window) || super::cursor_inside(&window).unwrap_or(false);
            if !near {
                since = None;
                continue;
            }
            if since.get_or_insert_with(Instant::now).elapsed() < dock::TAB_HOLD {
                continue;
            }
            tracing::info!(event = "hud_island_peek");
            expand(&app, &window, Duration::ZERO, dock::PEEK_LINGER);
            return;
        }
    });
}

/* -------------------------------------------------------------------------
 * The drag
 * ---------------------------------------------------------------------- */

/// Free the HUD for a drag and preview the island while the drop would make
/// one. Main thread only. Returns true when the HUD was docked or islanded.
#[cfg(target_os = "macos")]
pub fn begin_drag(app: &AppHandle) -> bool {
    let was_parked = dock::tear_off(app);
    if !refresh_notch() {
        return was_parked;
    }
    if let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) {
        let generation = state().generation;
        spawn_drag_watcher(app.clone(), window, generation);
    }
    was_parked
}

/// Keep the drag inert where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn begin_drag(app: &tauri::AppHandle) -> bool {
    super::dock::tear_off(app)
}

/// Tell the webview when the dragged window enters or leaves the notch.
///
/// The drop bumps the generation, which ends the watch.
#[cfg(target_os = "macos")]
fn spawn_drag_watcher(app: AppHandle, window: WebviewWindow, generation: u64) {
    tauri::async_runtime::spawn(async move {
        let mut over = false;
        loop {
            tokio::time::sleep(DRAG_POLL).await;
            let notch = {
                let dock = state();
                if dock.generation != generation {
                    return;
                }
                dock.notch
            };
            let Some(notch) = notch else {
                return;
            };
            if app.get_webview_window(super::OVERLAY_LABEL).is_none() {
                return;
            }
            let Some(rect) = dock::window_rect(&window) else {
                continue;
            };
            let now_over = notch_dropped_on(&notch.rect, &rect);
            if now_over != over {
                over = now_over;
                tracing::info!(event = "hud_island_preview", over);
                let phase = if over {
                    IslandPhase::Preview
                } else {
                    IslandPhase::Off
                };
                emit(&app, notch.state(phase));
            }
        }
    });
}

/// True when a drop at `window` lands on the cached notch.
#[cfg(target_os = "macos")]
pub(crate) fn dropped_on_notch(window: &Rect) -> bool {
    state()
        .notch
        .is_some_and(|notch| notch_dropped_on(&notch.rect, window))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 14-inch MacBook Pro display at 2x: 1512 x 982 logical.
    const DISPLAY: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 3024.0,
        height: 1964.0,
    };
    const SCALE: f64 = 2.0;

    fn notch() -> Rect {
        notch_rect(&DISPLAY, SCALE, 32.0, 656.0, 656.0).expect("a notched display")
    }

    #[test]
    fn the_notch_is_the_gap_between_the_auxiliary_areas_plus_the_spread() {
        let notch = notch();
        assert_eq!(notch.x, (656.0 - 2.0) * SCALE);
        assert_eq!(notch.y, 0.0);
        assert_eq!(notch.width, (1512.0 - 656.0 - 656.0 + 4.0) * SCALE);
        assert_eq!(notch.height, 32.0 * SCALE);
    }

    #[test]
    fn a_display_without_a_safe_area_has_no_notch() {
        assert_eq!(notch_rect(&DISPLAY, SCALE, 0.0, 0.0, 0.0), None);
        // Auxiliary areas that meet leave no gap.
        assert_eq!(notch_rect(&DISPLAY, SCALE, 32.0, 756.0, 756.0), None);
    }

    #[test]
    fn a_second_display_keeps_its_desktop_offset() {
        let second = Rect {
            x: 3024.0,
            y: 200.0,
            ..DISPLAY
        };
        let notch = notch_rect(&second, SCALE, 32.0, 656.0, 656.0).expect("a notch");
        assert_eq!(notch.x, 3024.0 + (656.0 - 2.0) * SCALE);
        assert_eq!(notch.y, 200.0);
    }

    #[test]
    fn the_island_adds_a_wing_and_a_gutter_each_side() {
        let notch = notch();
        let island = island_rect(&notch, SCALE);
        assert_eq!(island.x, notch.x - (WING + FILLET) * SCALE);
        assert_eq!(island.y, 0.0);
        assert_eq!(island.width, notch.width + 2.0 * (WING + FILLET) * SCALE);
        assert_eq!(island.height, notch.height);
    }

    #[test]
    fn the_hotspot_reaches_past_the_notch() {
        let notch = notch();
        let inside = (notch.x + 10.0, 5.0);
        assert!(in_hotspot(&notch, SCALE, inside));
        // Ten logical pixels beside it still count.
        assert!(in_hotspot(&notch, SCALE, (notch.x - 19.0, 5.0)));
        assert!(!in_hotspot(&notch, SCALE, (notch.x - 21.0, 5.0)));
        // Five logical pixels below it still count.
        assert!(in_hotspot(
            &notch,
            SCALE,
            (notch.x + 10.0, notch.height + 9.0)
        ));
        assert!(!in_hotspot(
            &notch,
            SCALE,
            (notch.x + 10.0, notch.height + 11.0)
        ));
        // Above the display never counts.
        assert!(!in_hotspot(&notch, SCALE, (notch.x + 10.0, -1.0)));
    }

    #[test]
    fn a_drop_past_the_top_over_the_notch_makes_the_island() {
        let notch = notch();
        let over = Rect {
            x: notch.x + 20.0,
            y: -30.0,
            width: 352.0,
            height: 120.0,
        };
        assert!(notch_dropped_on(&notch, &over));
        // Still on screen: a floating drop.
        assert!(!notch_dropped_on(&notch, &Rect { y: 0.0, ..over }));
        // Past the top but clear of the notch: a top dock.
        let beside = Rect {
            x: notch.x + notch.width + 1.0,
            ..over
        };
        assert!(!notch_dropped_on(&notch, &beside));
        // Overlapping by one pixel counts.
        let edge = Rect {
            x: notch.x - over.width + 1.0,
            ..over
        };
        assert!(notch_dropped_on(&notch, &edge));
    }

    #[test]
    fn the_webview_state_is_in_logical_pixels() {
        let notch = Notch {
            rect: notch(),
            scale: SCALE,
        };
        let state = notch.state(IslandPhase::Collapsed);
        assert_eq!(state.island, IslandPhase::Collapsed);
        assert_eq!(state.wing, WING);
        assert_eq!(state.fillet, FILLET);
        assert_eq!(state.notch, 204.0);
        assert_eq!(state.height, 32.0);
        assert_eq!(notch.window_width(), 204.0 + 2.0 * (WING + FILLET));
        let json = serde_json::to_value(state).expect("serializable");
        assert_eq!(json["island"], "collapsed");
        assert_eq!(json["notch"], 204.0);
    }

    #[test]
    fn the_off_state_serializes_as_off() {
        let json = serde_json::to_value(IslandState::off()).expect("serializable");
        assert_eq!(json["island"], "off");
        assert_eq!(json["wing"], 0.0);
    }
}
