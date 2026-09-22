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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

use serde::Serialize;
#[cfg(target_os = "macos")]
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewWindow};

#[cfg(any(target_os = "macos", test))]
use super::dock::Rect;
#[cfg(target_os = "macos")]
use super::dock::{self, state};

/// The drawable strip at 100%, in native logical points.
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

#[cfg(target_os = "macos")]
static GEOMETRY_REVISION: AtomicU64 = AtomicU64::new(0);

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
    /// The applied WebView scale, independent of the display backing factor.
    pub scale: f64,
    /// Reject measurements and events from an older native geometry.
    pub revision: u64,
    /// Expanded ink width, in native logical points.
    pub body_width: f64,
    /// Usable height below the fixed header, in native logical points.
    pub body_max_height: f64,
    /// Header offset from the native window's left edge, in logical points.
    pub header_offset: f64,
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
            scale: 1.0,
            revision: 0,
            body_width: 0.0,
            body_max_height: 0.0,
            header_offset: 0.0,
        }
    }
}

/// A notch in physical desktop pixels, with its display's scale.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Notch {
    pub(crate) rect: Rect,
    pub(crate) scale: f64,
    pub(crate) available: Rect,
}

#[cfg(any(target_os = "macos", test))]
struct IslandLayout {
    x: f64,
    width: f64,
    header_height: f64,
    header_offset: f64,
    body_max_height: f64,
}

#[cfg(any(target_os = "macos", test))]
impl Notch {
    /// The state the webview renders for this notch in `phase`.
    fn state(&self, phase: IslandPhase, scale: f64, revision: u64) -> IslandState {
        let layout = self.layout(phase, scale);
        IslandState {
            island: phase,
            wing: self.wing_width(scale),
            fillet: FILLET,
            notch: self.rect.width / self.scale,
            height: self.rect.height / self.scale,
            scale,
            revision,
            body_width: layout.width - 2.0 * FILLET,
            body_max_height: layout.body_max_height,
            header_offset: layout.header_offset,
        }
    }

    fn layout(&self, phase: IslandPhase, scale: f64) -> IslandLayout {
        let wing = self.wing_width(scale);
        let header_width = self.window_width(scale);
        let desired = if phase == IslandPhase::Expanded {
            ((self.rect.width / self.scale + 2.0 * WING) * scale + 2.0 * FILLET).max(header_width)
        } else {
            header_width
        };
        let width = desired.min(self.available.width / self.scale).max(1.0);
        let center = self.rect.x + self.rect.width / 2.0;
        let x = (center - width * self.scale / 2.0).clamp(
            self.available.x,
            (self.available.x + self.available.width - width * self.scale).max(self.available.x),
        );
        IslandLayout {
            x,
            width,
            header_height: self.rect.height / self.scale,
            header_offset: (self.rect.x - (wing + FILLET) * self.scale - x) / self.scale,
            body_max_height: ((self.available.y + self.available.height
                - self.rect.y
                - self.rect.height)
                / self.scale)
                .max(0.0),
        }
    }

    /// The window width in logical pixels: the notch, two wings, two gutters.
    fn window_width(&self, scale: f64) -> f64 {
        self.rect.width / self.scale + 2.0 * (self.wing_width(scale) + FILLET)
    }

    /// Grow both wings equally without shifting the camera gap or clipping a side.
    fn wing_width(&self, scale: f64) -> f64 {
        let left = (self.rect.x - self.available.x) / self.scale - FILLET;
        let right = (self.available.x + self.available.width - self.rect.x - self.rect.width)
            / self.scale
            - FILLET;
        (WING * scale).min(left.min(right).max(0.0))
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
fn island_rect(notch: &Rect, scale: f64, wing: f64) -> Rect {
    let margin = (wing + FILLET) * scale;
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
        let available = desktop_rect(screen.visibleFrame(), primary_height, scale);
        notch_rect(&display, scale, insets.top, left, right).map(|rect| Notch {
            rect,
            scale,
            available,
        })
    });
    if real.is_some() || !FAKE_NOTCH_ON.load(Ordering::Relaxed) {
        return real;
    }
    let scale = primary.backingScaleFactor();
    let display = desktop_rect(primary.frame(), primary_height, scale);
    let (width, height) = FAKE_NOTCH;
    let side = ((display.width / scale - width) / 2.0).max(0.0);
    let available = desktop_rect(primary.visibleFrame(), primary_height, scale);
    notch_rect(&display, scale, height, side, side).map(|rect| Notch {
        rect,
        scale,
        available,
    })
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
pub fn set_fake_notch(_app: &tauri::AppHandle, _on: bool) -> bool {
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
        Some(notch) if dock.island => notch.state(
            if dock.docked {
                IslandPhase::Collapsed
            } else {
                IslandPhase::Expanded
            },
            super::interface_scale(),
            revision(),
        ),
        _ => IslandState {
            scale: super::interface_scale(),
            revision: revision(),
            ..IslandState::off()
        },
    }
}

/// Keep the island off where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn island_state() -> IslandState {
    IslandState::off()
}

#[cfg(target_os = "macos")]
pub(crate) fn revision() -> u64 {
    GEOMETRY_REVISION.load(Ordering::Acquire)
}

#[cfg(target_os = "macos")]
pub(crate) fn geometry_changed(app: &AppHandle) {
    emit_state(app);
}

/// Keep native geometry anchored to the cutout, not the floating placement.
#[cfg(target_os = "macos")]
pub(crate) fn fit_height(window: &WebviewWindow, height: f64) -> Option<(f64, f64)> {
    let dock = state();
    let notch = dock.notch.filter(|_| dock.island)?;
    let collapsed = dock.docked;
    drop(dock);
    let layout = notch.layout(
        if collapsed {
            IslandPhase::Collapsed
        } else {
            IslandPhase::Expanded
        },
        super::interface_scale(),
    );
    let height = if collapsed {
        layout.header_height
    } else {
        height.clamp(
            layout.header_height,
            layout.header_height + layout.body_max_height,
        )
    };
    let _ = window.set_position(PhysicalPosition::new(layout.x, notch.rect.y));
    Some((layout.width, height))
}

/// Send the current island state to the HUD webview.
#[cfg(target_os = "macos")]
fn emit_state(app: &AppHandle) {
    emit(app, island_state());
}

#[cfg(target_os = "macos")]
fn emit(app: &AppHandle, mut state: IslandState) {
    state.scale = super::interface_scale();
    state.revision = GEOMETRY_REVISION.fetch_add(1, Ordering::AcqRel) + 1;
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
/// Worker only, after the shell refreshes the notch on the main thread. Returns false when no
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
    let _ = super::resize_apply_guard();
    let scale = super::interface_scale();
    let (notch, generation) = {
        let mut dock = state();
        let Some(notch) = dock.notch else {
            return false;
        };
        let rect = island_rect(&notch.rect, notch.scale, notch.wing_width(scale));
        dock.island = true;
        dock.island_wanted = true;
        dock.docked = true;
        dock.park_timing = None;
        dock.edge = dock::DockEdge::Top;
        dock.home = Some((rect.x, rect.y));
        dock.frame = Some(notch.available);
        dock.scale = notch.scale;
        dock.generation += 1;
        (notch, dock.generation)
    };
    super::hide_detail(app);
    let rect = island_rect(&notch.rect, notch.scale, notch.wing_width(scale));
    tracing::info!(
        event = "hud_island",
        x = rect.x,
        y = rect.y,
        width = rect.width
    );
    fit_window(
        window,
        &rect,
        notch.window_width(scale),
        notch.rect.height / notch.scale,
    );
    emit_state(app);
    spawn_hotspot_watcher(app.clone(), window.clone(), generation);
    true
}

/// Move the window to `rect` and give it the island's logical size.
#[cfg(target_os = "macos")]
fn fit_window(window: &WebviewWindow, rect: &Rect, width: f64, height: f64) {
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
    let _guard = super::resize_apply_guard();
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
                let half =
                    (super::OVERLAY_WIDTH * super::interface_scale() * scale / 2.0).round() as i32;
                PhysicalPosition::new((cursor.x.round() as i32) - half, origin.y)
            });
        let _ = window.set_resizable(true);
        let _ = window.set_size(LogicalSize::new(
            super::OVERLAY_WIDTH * super::interface_scale(),
            height,
        ));
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
    let _guard = super::resize_apply_guard();
    let generation = {
        let mut dock = state();
        if !dock.island || !dock.docked || dock.notch.is_none() {
            return;
        }
        dock.docked = false;
        dock.park_timing = Some(dock::ParkTiming::new(Instant::now(), hold, linger));
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
            collapse(app, window, generation);
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
fn collapse(app: &AppHandle, window: &WebviewWindow, expected_generation: u64) {
    let _guard = super::resize_apply_guard();
    let generation = {
        let mut dock = state();
        if !dock.island || dock.docked || dock.generation != expected_generation {
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

/// Restore the watcher after scale reconciliation cancels old motion.
#[cfg(target_os = "macos")]
pub(crate) fn resume_after_scale(app: &AppHandle, window: &WebviewWindow, generation: u64) {
    if state().docked {
        spawn_hotspot_watcher(app.clone(), window.clone(), generation);
    } else {
        dock::spawn_auto_park(
            app.clone(),
            window.clone(),
            generation,
            Duration::ZERO,
            dock::PEEK_LINGER,
            cursor_in_hotspot,
            move |app, window| collapse(app, window, generation),
        );
    }
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
/// one. Worker only, after a main-thread notch snapshot. Returns true when the HUD was docked or islanded.
#[cfg(target_os = "macos")]
pub fn begin_drag(app: &AppHandle, revision: u64) -> bool {
    if !super::drag_is_current(revision) {
        return false;
    }
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
pub fn begin_drag(app: &tauri::AppHandle, revision: u64) -> bool {
    if !super::drag_is_current(revision) {
        return false;
    }
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
                emit(
                    &app,
                    notch.state(phase, super::interface_scale(), revision()),
                );
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
        let island = island_rect(&notch, SCALE, WING);
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
            available: DISPLAY,
        };
        let state = notch.state(IslandPhase::Collapsed, 1.0, 0);
        assert_eq!(state.island, IslandPhase::Collapsed);
        assert_eq!(state.wing, WING);
        assert_eq!(state.fillet, FILLET);
        assert_eq!(state.notch, 204.0);
        assert_eq!(state.height, 32.0);
        assert_eq!(notch.window_width(1.0), 204.0 + 2.0 * (WING + FILLET));
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

    #[test]
    fn scale_preserves_the_notch_and_caps_the_expanded_body() {
        for backing in [1.0, 2.0] {
            let notch = Notch {
                rect: Rect {
                    x: -600.0 * backing,
                    y: 50.0 * backing,
                    width: 204.0 * backing,
                    height: 32.0 * backing,
                },
                scale: backing,
                available: Rect {
                    x: -800.0 * backing,
                    y: 82.0 * backing,
                    width: 650.0 * backing,
                    height: 500.0 * backing,
                },
            };
            for scale in [0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0] {
                let collapsed = notch.layout(IslandPhase::Collapsed, scale);
                let expanded = notch.layout(IslandPhase::Expanded, scale);
                let initial = island_rect(&notch.rect, backing, notch.wing_width(scale));
                assert_eq!(initial.x, collapsed.x);
                assert_eq!(initial.width / backing, collapsed.width);
                assert_eq!(collapsed.width, 204.0 + 2.0 * (WING * scale + FILLET));
                let rendered = notch.state(IslandPhase::Expanded, scale, 1);
                assert_eq!(rendered.wing, WING * scale);
                assert_eq!(rendered.notch, 204.0);
                assert_eq!(collapsed.header_height, 32.0);
                assert_eq!(expanded.header_height, 32.0);
                assert!(expanded.width >= collapsed.width);
                assert!(expanded.width <= 650.0);
                assert_eq!(expanded.body_max_height, 500.0);
                let header_left = expanded.x / backing + expanded.header_offset;
                assert_eq!(header_left, notch.rect.x / backing - WING * scale - FILLET);
                assert!(expanded.x >= notch.available.x);
                assert!(
                    expanded.x + expanded.width * backing
                        <= notch.available.x + notch.available.width
                );
            }
        }
    }

    #[test]
    fn expanded_body_is_clamped_to_a_small_display_without_scaling_the_header() {
        let notch = Notch {
            rect: Rect {
                x: 180.0,
                y: 0.0,
                width: 204.0,
                height: 32.0,
            },
            scale: 1.0,
            available: Rect {
                x: 0.0,
                y: 32.0,
                width: 600.0,
                height: 190.0,
            },
        };
        let layout = notch.layout(IslandPhase::Expanded, 2.0);
        assert_eq!(layout.width, 566.0);
        assert_eq!(layout.body_max_height, 190.0);
        assert_eq!(layout.header_height, 32.0);
    }

    #[test]
    fn scaled_wings_fit_both_sides_without_moving_the_camera_gap() {
        let notch = Notch {
            rect: Rect {
                x: -146.0,
                y: -20.0,
                width: 204.0,
                height: 24.0,
            },
            scale: 1.0,
            available: Rect {
                x: -200.0,
                y: 4.0,
                width: 320.0,
                height: 400.0,
            },
        };
        for phase in [IslandPhase::Collapsed, IslandPhase::Expanded] {
            let layout = notch.layout(phase, 2.0);
            let state = notch.state(phase, 2.0, 1);
            assert_eq!(state.wing, 35.0);
            assert_eq!(
                layout.x + state.header_offset + FILLET + state.wing,
                notch.rect.x
            );
            assert_eq!(state.height, 24.0);
            assert!(layout.x >= notch.available.x);
            assert!(layout.x + layout.width <= notch.available.x + notch.available.width);
        }
    }
}
