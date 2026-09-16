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

/// How much of the docked HUD stays on screen, in logical pixels.
#[cfg(any(target_os = "macos", test))]
const TAB: f64 = 8.0;

/// How near a display edge a dropped HUD docks, in logical pixels.
#[cfg(any(target_os = "macos", test))]
const SNAP: f64 = 16.0;

/// How long one slide takes.
#[cfg(target_os = "macos")]
const SLIDE_DURATION: Duration = Duration::from_millis(200);

/// Frames per slide.
#[cfg(target_os = "macos")]
const SLIDE_STEPS: u32 = 12;

/// How often the docked HUD reads the cursor.
#[cfg(target_os = "macos")]
const TAB_POLL: Duration = Duration::from_millis(100);

/// How long the cursor must rest on the tab before the HUD peeks in.
#[cfg(target_os = "macos")]
const TAB_HOLD: Duration = Duration::from_millis(150);

/// How often a peeked HUD checks whether it should park again.
#[cfg(target_os = "macos")]
const AUTO_DOCK_POLL: Duration = Duration::from_millis(200);

/// How long the HUD stays after the pointer leaves it.
#[cfg(any(target_os = "macos", test))]
const LINGER: Duration = Duration::from_secs(3);

/// How long a woken HUD stays, at least.
#[cfg(target_os = "macos")]
const WAKE_HOLD: Duration = Duration::from_secs(5);

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockSettings {
    pub docked: bool,
    pub edge: DockEdge,
}

/// A rectangle in physical desktop pixels.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

struct DockState {
    edge: DockEdge,
    docked: bool,
    /// Where the HUD sits when peeked in, in physical desktop pixels.
    home: Option<(f64, f64)>,
    /// The display the HUD docked against.
    #[cfg(target_os = "macos")]
    frame: Option<Rect>,
    /// The display's scale, so the tab is in logical pixels.
    #[cfg(target_os = "macos")]
    scale: f64,
    /// A new value cancels every task from an earlier transition.
    generation: u64,
}

static DOCK: Mutex<DockState> = Mutex::new(DockState {
    edge: DockEdge::Right,
    docked: false,
    home: None,
    #[cfg(target_os = "macos")]
    frame: None,
    #[cfg(target_os = "macos")]
    scale: 1.0,
    generation: 0,
});

fn state() -> std::sync::MutexGuard<'static, DockState> {
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
    }
}

/// Dock again at launch or reopen, when the reader left the HUD docked.
#[cfg(target_os = "macos")]
pub fn restore_dock(app: &AppHandle, settings: DockSettings) {
    if !settings.docked {
        return;
    }
    let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) else {
        return;
    };
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
    if let Some(window_rect) = window_rect(&window)
        && let Some(monitor) = monitor_of(&window)
        && let Some(edge) = edge_dropped_on(
            &monitor_rect(&monitor),
            &window_rect,
            SNAP * monitor.scale_factor(),
        )
    {
        dock_at(app, &window, edge);
    }
    dock_settings()
}

/// Keep the drop inert where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn settle_after_drag(_app: &tauri::AppHandle) -> DockSettings {
    dock_settings()
}

/// Free the HUD: a drag started on a docked or peeked HUD.
pub fn tear_off() {
    let mut dock = state();
    if dock.docked || dock.home.is_some() {
        tracing::info!(event = "hud_tear_off", edge = ?dock.edge);
    }
    dock.docked = false;
    dock.home = None;
    dock.generation += 1;
}

/// Bring a docked HUD in for a while. `reason` is for the log only.
#[cfg(target_os = "macos")]
pub fn wake_overlay(app: &AppHandle, reason: &str) {
    let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) else {
        return;
    };
    let docked = state().docked;
    if !docked {
        return;
    }
    tracing::info!(event = "hud_wake", reason);
    undock(app, &window, WAKE_HOLD);
}

/// Keep waking inert where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn wake_overlay(_app: &tauri::AppHandle, _reason: &str) {}

/// Forget the dock position. The window is about to hide.
pub(crate) fn reset() {
    let mut dock = state();
    dock.docked = false;
    dock.home = None;
    dock.generation += 1;
}

/// Dock again after a placement moved the window on screen.
///
/// The placement is the new home, and the dock edge is the same edge of the
/// display the placement chose.
#[cfg(target_os = "macos")]
pub(crate) fn redock_after_placement(app: &AppHandle, window: &WebviewWindow) {
    let edge = {
        let dock = state();
        if !dock.docked && dock.home.is_none() {
            return;
        }
        dock.edge
    };
    tear_off();
    dock_at(app, window, edge);
}

/// Keep a docked window at its tab after its height changed.
///
/// The caller holds the resize guard, so this writes the position directly.
#[cfg(target_os = "macos")]
pub(crate) fn keep_docked_after_resize(window: &WebviewWindow) {
    let target = {
        let dock = state();
        if !dock.docked {
            return;
        }
        let Some(frame) = dock.frame else {
            return;
        };
        let Some(window_rect) = window_rect(window) else {
            return;
        };
        docked_position(dock.edge, &frame, &window_rect, TAB * dock.scale)
    };
    let _ = window.set_position(PhysicalPosition::new(target.0, target.1));
}

/// Park the HUD at `edge` of the display it is on.
///
/// Home becomes the position flush inside that edge, so a peek brings the
/// whole HUD on screen even after a drop that went past the edge.
#[cfg(target_os = "macos")]
fn dock_at(app: &AppHandle, window: &WebviewWindow, edge: DockEdge) {
    let (start, target, generation) = {
        let mut dock = state();
        if dock.docked {
            return;
        }
        let Some(window_rect) = window_rect(window) else {
            return;
        };
        let Some(monitor) = monitor_of(window) else {
            return;
        };
        let frame = monitor_rect(&monitor);
        let scale = monitor.scale_factor();
        let home = dock
            .home
            .unwrap_or_else(|| flush_position(edge, &frame, &window_rect));
        dock.edge = edge;
        dock.home = Some(home);
        dock.frame = Some(frame);
        dock.scale = scale;
        dock.docked = true;
        dock.generation += 1;
        let target = docked_position(edge, &frame, &window_rect, TAB * scale);
        ((window_rect.x, window_rect.y), target, dock.generation)
    };
    super::hide_detail(app);
    tracing::info!(event = "hud_dock", edge = ?edge);
    let watcher = (app.clone(), window.clone());
    slide(window.clone(), start, target, generation, move || {
        spawn_tab_watcher(watcher.0, watcher.1, generation);
    });
}

/// Slide a docked HUD in, then park it again after a quiet spell.
///
/// `hold` is the least time the HUD stays.
#[cfg(target_os = "macos")]
fn undock(app: &AppHandle, window: &WebviewWindow, hold: Duration) {
    let (start, home, generation) = {
        let mut dock = state();
        if !dock.docked {
            return;
        }
        let Some(home) = dock.home else {
            dock.docked = false;
            return;
        };
        let Some(window_rect) = window_rect(window) else {
            return;
        };
        dock.docked = false;
        dock.generation += 1;
        ((window_rect.x, window_rect.y), home, dock.generation)
    };
    let after = (app.clone(), window.clone());
    slide(window.clone(), start, home, generation, move || {
        spawn_auto_dock(after.0, after.1, generation, hold);
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
            if !super::cursor_inside(&window).unwrap_or(false) {
                on_tab_since = None;
                continue;
            }
            let since = *on_tab_since.get_or_insert_with(Instant::now);
            if since.elapsed() < TAB_HOLD {
                continue;
            }
            tracing::info!(event = "hud_peek");
            undock(&app, &window, Duration::ZERO);
            return;
        }
    });
}

/// Park the peeked HUD once the hold passed and the pointer left it.
#[cfg(target_os = "macos")]
fn spawn_auto_dock(app: AppHandle, window: WebviewWindow, generation: u64, hold: Duration) {
    tauri::async_runtime::spawn(async move {
        let start = Instant::now();
        let mut last_inside = start;
        loop {
            tokio::time::sleep(AUTO_DOCK_POLL).await;
            let edge = {
                let dock = state();
                if dock.generation != generation || dock.docked || dock.home.is_none() {
                    return;
                }
                dock.edge
            };
            if app.get_webview_window(super::OVERLAY_LABEL).is_none() {
                return;
            }
            let now = Instant::now();
            if super::cursor_inside(&window).unwrap_or(false) {
                last_inside = now;
                continue;
            }
            if should_dock(now, start, hold, last_inside) {
                dock_at(&app, &window, edge);
                return;
            }
        }
    });
}

/// The window frame in physical desktop pixels.
#[cfg(target_os = "macos")]
fn window_rect(window: &WebviewWindow) -> Option<Rect> {
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
fn monitor_of(window: &WebviewWindow) -> Option<Monitor> {
    window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
}

#[cfg(target_os = "macos")]
fn monitor_rect(monitor: &Monitor) -> Rect {
    Rect {
        x: f64::from(monitor.position().x),
        y: f64::from(monitor.position().y),
        width: f64::from(monitor.size().width),
        height: f64::from(monitor.size().height),
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

/// The edge a dropped window touches, within `snap`, nearest first. Pure.
#[cfg(any(target_os = "macos", test))]
fn edge_dropped_on(frame: &Rect, window: &Rect, snap: f64) -> Option<DockEdge> {
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
        .filter(|(_, gap)| *gap <= snap)
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(edge, _)| edge)
}

/// Whether the peeked HUD should park: the hold passed, and the pointer has
/// been off it for the linger. Pure.
#[cfg(any(target_os = "macos", test))]
fn should_dock(now: Instant, start: Instant, hold: Duration, last_inside: Instant) -> bool {
    now.duration_since(start) >= hold && now.duration_since(last_inside) >= LINGER
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
    fn docked_positions_leave_only_the_tab_on_screen() {
        assert_eq!(
            docked_position(DockEdge::Left, &FRAME, &WINDOW, TAB),
            (-68.0, 80.0)
        );
        assert_eq!(
            docked_position(DockEdge::Right, &FRAME, &WINDOW, TAB),
            (1092.0, 80.0)
        );
        assert_eq!(
            docked_position(DockEdge::Top, &FRAME, &WINDOW, TAB),
            (500.0, -2.0)
        );
        assert_eq!(
            docked_position(DockEdge::Bottom, &FRAME, &WINDOW, TAB),
            (500.0, 642.0)
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
    fn a_drop_near_an_edge_docks_there() {
        let mid = WINDOW;
        assert_eq!(edge_dropped_on(&FRAME, &mid, SNAP), None);
        let near_right = Rect { x: 920.0, ..WINDOW };
        assert_eq!(
            edge_dropped_on(&FRAME, &near_right, SNAP),
            Some(DockEdge::Right)
        );
        let past_right = Rect {
            x: 1050.0,
            ..WINDOW
        };
        assert_eq!(
            edge_dropped_on(&FRAME, &past_right, SNAP),
            Some(DockEdge::Right)
        );
        let near_top = Rect { y: 60.0, ..WINDOW };
        assert_eq!(
            edge_dropped_on(&FRAME, &near_top, SNAP),
            Some(DockEdge::Top)
        );
        // A corner picks the nearer edge.
        let corner = Rect {
            x: 104.0,
            y: 58.0,
            ..WINDOW
        };
        assert_eq!(edge_dropped_on(&FRAME, &corner, SNAP), Some(DockEdge::Left));
    }

    #[test]
    fn auto_dock_waits_for_the_hold_and_the_linger() {
        let start = Instant::now();
        let hold = Duration::from_secs(5);
        assert!(!should_dock(
            start + Duration::from_secs(4),
            start,
            hold,
            start
        ));
        assert!(should_dock(
            start + Duration::from_secs(5),
            start,
            hold,
            start
        ));
        let hovered = start + Duration::from_secs(4);
        assert!(!should_dock(
            start + Duration::from_secs(6),
            start,
            hold,
            hovered
        ));
        assert!(should_dock(
            start + Duration::from_secs(7),
            start,
            hold,
            hovered
        ));
        assert!(should_dock(start + LINGER, start, Duration::ZERO, start));
    }

    #[test]
    fn dock_settings_round_trip_in_camel_case() {
        let settings: DockSettings =
            serde_json::from_str("{\"docked\":true,\"edge\":\"left\"}").unwrap();
        assert_eq!(
            settings,
            DockSettings {
                docked: true,
                edge: DockEdge::Left
            }
        );
        assert_eq!(
            serde_json::to_string(&settings).unwrap(),
            "{\"docked\":true,\"edge\":\"left\"}"
        );
    }
}
