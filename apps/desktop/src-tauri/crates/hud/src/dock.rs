//! Edge dock: the HUD slides off one edge of its display and comes back.
//!
//! Docked, the window sits fully outside the display and keeps its renderer
//! alive. It returns when the cursor rests on that edge, when the shell asks
//! it to wake, or when the reader turns the dock off. Once back, it docks
//! again after a quiet spell with no pointer on it.

use std::sync::Mutex;
#[cfg(any(target_os = "macos", test))]
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use tauri::{AppHandle, Emitter, Manager, Monitor, PhysicalPosition, WebviewWindow};

/// Event that carries the dock settings to every webview.
#[cfg(target_os = "macos")]
const DOCK_CHANGED_EVENT: &str = "overlay_dock_changed";

/// Event that says the cursor rested on the dock edge.
#[cfg(target_os = "macos")]
const EDGE_HIT_EVENT: &str = "overlay_edge_hit";

/// How long one slide takes.
#[cfg(target_os = "macos")]
const SLIDE_DURATION: Duration = Duration::from_millis(200);

/// Frames per slide.
#[cfg(target_os = "macos")]
const SLIDE_STEPS: u32 = 12;

/// How often the docked HUD reads the cursor.
#[cfg(target_os = "macos")]
const EDGE_POLL: Duration = Duration::from_millis(100);

/// How long the cursor must rest on the edge before the HUD comes back.
#[cfg(target_os = "macos")]
const EDGE_HOLD: Duration = Duration::from_millis(150);

/// How close to the edge counts as on it, in logical pixels.
#[cfg(any(target_os = "macos", test))]
const EDGE_TOLERANCE: f64 = 2.0;

/// How often the shown HUD checks whether it should dock again.
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

/// The dock settings the reader chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DockSettings {
    pub enabled: bool,
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
    settings: DockSettings,
    docked: bool,
    /// Where the HUD sat before it docked, in physical desktop pixels.
    home: Option<(f64, f64)>,
    /// The display the HUD docked against.
    #[cfg(target_os = "macos")]
    frame: Option<Rect>,
    /// The display's scale, so the edge tolerance is in logical pixels.
    #[cfg(target_os = "macos")]
    scale: f64,
    /// A new value cancels every task from an earlier transition.
    generation: u64,
}

static DOCK: Mutex<DockState> = Mutex::new(DockState {
    settings: DockSettings {
        enabled: false,
        edge: DockEdge::Right,
    },
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

/// The dock settings the shell last applied.
pub fn dock_settings() -> DockSettings {
    state().settings
}

/// Return whether the HUD is off screen at its dock edge.
pub fn is_docked() -> bool {
    state().docked
}

/// Apply the reader's dock settings.
///
/// Turning the dock on arms the quiet timer: the shown HUD docks after the
/// wake hold with no pointer on it. Turning it off brings a docked HUD home.
/// A new edge moves a docked HUD to that edge at once.
#[cfg(target_os = "macos")]
pub fn configure_dock(app: &AppHandle, settings: DockSettings) {
    let (was, generation) = {
        let mut dock = state();
        let was = dock.settings;
        dock.settings = settings;
        dock.generation += 1;
        (was, dock.generation)
    };
    let _ = app.emit(DOCK_CHANGED_EVENT, settings);
    let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) else {
        return;
    };
    if !settings.enabled {
        undock(app, &window, None);
        return;
    }
    if state().docked {
        if was.edge != settings.edge {
            snap_to_edge(&window);
        }
        spawn_edge_watcher(app.clone(), window, generation);
        return;
    }
    spawn_auto_dock(app.clone(), window, generation, WAKE_HOLD);
}

/// Keep dock settings inert where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn configure_dock(_app: &tauri::AppHandle, settings: DockSettings) {
    state().settings = settings;
}

/// Slide the HUD off its dock edge now.
#[cfg(target_os = "macos")]
pub fn dock_overlay(app: &AppHandle) {
    let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) else {
        return;
    };
    let (start, target, edge, generation) = {
        let mut dock = state();
        if !dock.settings.enabled || dock.docked {
            return;
        }
        let Some(window_rect) = window_rect(&window) else {
            return;
        };
        let Some(monitor) = monitor_of(&window) else {
            return;
        };
        let frame = monitor_rect(&monitor);
        // A slide back that is still in flight keeps the true home.
        let home = dock.home.unwrap_or((window_rect.x, window_rect.y));
        dock.home = Some(home);
        dock.frame = Some(frame);
        dock.scale = monitor.scale_factor();
        dock.docked = true;
        dock.generation += 1;
        let edge = dock.settings.edge;
        let target = docked_position(edge, &frame, &window_rect);
        (
            (window_rect.x, window_rect.y),
            target,
            edge,
            dock.generation,
        )
    };
    super::hide_detail(app);
    tracing::info!(event = "hud_dock", edge = ?edge);
    let watcher = (app.clone(), window.clone());
    slide(window, start, target, generation, move || {
        spawn_edge_watcher(watcher.0, watcher.1, generation);
    });
}

/// Keep docking inert where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn dock_overlay(_app: &tauri::AppHandle) {}

/// Bring a docked HUD back for a while. `reason` is for the log only.
#[cfg(target_os = "macos")]
pub fn wake_overlay(app: &AppHandle, reason: &str) {
    let Some(window) = app.get_webview_window(super::OVERLAY_LABEL) else {
        return;
    };
    let (enabled, docked) = {
        let dock = state();
        (dock.settings.enabled, dock.docked)
    };
    if !enabled {
        return;
    }
    tracing::info!(event = "hud_wake", reason, docked);
    if docked {
        undock(app, &window, Some(WAKE_HOLD));
        return;
    }
    // Already shown: a wake extends the stay from now.
    let generation = {
        let mut dock = state();
        dock.generation += 1;
        dock.generation
    };
    spawn_auto_dock(app.clone(), window, generation, WAKE_HOLD);
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
pub(crate) fn redock_after_placement(window: &WebviewWindow) {
    if !state().docked {
        return;
    }
    let Some(window_rect) = window_rect(window) else {
        return;
    };
    let Some(monitor) = monitor_of(window) else {
        return;
    };
    {
        let mut dock = state();
        dock.home = Some((window_rect.x, window_rect.y));
        dock.frame = Some(monitor_rect(&monitor));
        dock.scale = monitor.scale_factor();
    }
    snap_to_edge(window);
}

/// Keep a docked window off screen after its height changed.
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
        docked_position(dock.settings.edge, &frame, &window_rect)
    };
    let _ = window.set_position(PhysicalPosition::new(target.0, target.1));
}

/// Slide a docked HUD home, then dock it again after a quiet spell.
///
/// `hold` is the least time the HUD stays. `None` means the reader turned
/// the dock off, so it stays for good.
#[cfg(target_os = "macos")]
fn undock(app: &AppHandle, window: &WebviewWindow, hold: Option<Duration>) {
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
    let after = hold.map(|hold| (app.clone(), window.clone(), hold));
    slide(window.clone(), start, home, generation, move || {
        {
            let mut dock = state();
            if dock.generation == generation {
                dock.home = None;
            }
        }
        if let Some((app, window, hold)) = after {
            spawn_auto_dock(app, window, generation, hold);
        }
    });
}

/// Put a docked window at its edge without animation.
#[cfg(target_os = "macos")]
fn snap_to_edge(window: &WebviewWindow) {
    let _guard = super::resize_apply_guard();
    keep_docked_after_resize(window);
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

/// Watch for the cursor resting on the dock edge while docked.
#[cfg(target_os = "macos")]
fn spawn_edge_watcher(app: AppHandle, window: WebviewWindow, generation: u64) {
    tauri::async_runtime::spawn(async move {
        let mut on_edge_since: Option<Instant> = None;
        loop {
            tokio::time::sleep(EDGE_POLL).await;
            let (edge, frame, scale) = {
                let dock = state();
                if dock.generation != generation || !dock.docked {
                    return;
                }
                let Some(frame) = dock.frame else {
                    return;
                };
                (dock.settings.edge, frame, dock.scale)
            };
            let Ok(cursor) = window.cursor_position() else {
                on_edge_since = None;
                continue;
            };
            if !edge_hit(edge, &frame, (cursor.x, cursor.y), EDGE_TOLERANCE * scale) {
                on_edge_since = None;
                continue;
            }
            let since = *on_edge_since.get_or_insert_with(Instant::now);
            if since.elapsed() < EDGE_HOLD {
                continue;
            }
            let _ = app.emit(EDGE_HIT_EVENT, ());
            tracing::info!(event = "hud_edge_hit", edge = ?edge);
            undock(&app, &window, Some(Duration::ZERO));
            return;
        }
    });
}

/// Dock the shown HUD once the hold passed and the pointer left it.
#[cfg(target_os = "macos")]
fn spawn_auto_dock(app: AppHandle, window: WebviewWindow, generation: u64, hold: Duration) {
    tauri::async_runtime::spawn(async move {
        let start = Instant::now();
        let mut last_inside = start;
        loop {
            tokio::time::sleep(AUTO_DOCK_POLL).await;
            {
                let dock = state();
                if dock.generation != generation || !dock.settings.enabled || dock.docked {
                    return;
                }
            }
            if app.get_webview_window(super::OVERLAY_LABEL).is_none() {
                return;
            }
            let now = Instant::now();
            if super::cursor_inside(&window).unwrap_or(false) {
                last_inside = now;
                continue;
            }
            if should_dock(now, start, hold, last_inside) {
                dock_overlay(&app);
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

/// Where the window sits fully outside `frame` at `edge`. Pure.
#[cfg(any(target_os = "macos", test))]
fn docked_position(edge: DockEdge, frame: &Rect, window: &Rect) -> (f64, f64) {
    match edge {
        DockEdge::Left => (frame.x - window.width, window.y),
        DockEdge::Right => (frame.x + frame.width, window.y),
        DockEdge::Top => (window.x, frame.y - window.height),
        DockEdge::Bottom => (window.x, frame.y + frame.height),
    }
}

/// Whether `cursor` rests within `tolerance` of `edge`, inside the frame's
/// other extent. Pure.
#[cfg(any(target_os = "macos", test))]
fn edge_hit(edge: DockEdge, frame: &Rect, cursor: (f64, f64), tolerance: f64) -> bool {
    let (x, y) = cursor;
    let right = frame.x + frame.width;
    let bottom = frame.y + frame.height;
    let along_x = x >= frame.x && x < right;
    let along_y = y >= frame.y && y < bottom;
    match edge {
        DockEdge::Left => along_y && x >= frame.x && x <= frame.x + tolerance,
        DockEdge::Right => along_y && x >= right - 1.0 - tolerance && x <= right,
        DockEdge::Top => along_x && y >= frame.y && y <= frame.y + tolerance,
        DockEdge::Bottom => along_x && y >= bottom - 1.0 - tolerance && y <= bottom,
    }
}

/// Whether the shown HUD should dock: the hold passed, and the pointer has
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
    fn docked_positions_leave_the_display_entirely() {
        assert_eq!(
            docked_position(DockEdge::Left, &FRAME, &WINDOW),
            (-76.0, 80.0)
        );
        assert_eq!(
            docked_position(DockEdge::Right, &FRAME, &WINDOW),
            (1100.0, 80.0)
        );
        assert_eq!(
            docked_position(DockEdge::Top, &FRAME, &WINDOW),
            (500.0, -10.0)
        );
        assert_eq!(
            docked_position(DockEdge::Bottom, &FRAME, &WINDOW),
            (500.0, 650.0)
        );
    }

    #[test]
    fn edge_hit_needs_the_cursor_on_the_chosen_edge() {
        assert!(edge_hit(
            DockEdge::Right,
            &FRAME,
            (1099.0, 300.0),
            EDGE_TOLERANCE
        ));
        assert!(edge_hit(
            DockEdge::Right,
            &FRAME,
            (1097.0, 300.0),
            EDGE_TOLERANCE
        ));
        assert!(!edge_hit(
            DockEdge::Right,
            &FRAME,
            (1096.0, 300.0),
            EDGE_TOLERANCE
        ));
        assert!(!edge_hit(
            DockEdge::Right,
            &FRAME,
            (1099.0, 700.0),
            EDGE_TOLERANCE
        ));
        assert!(edge_hit(
            DockEdge::Left,
            &FRAME,
            (101.0, 300.0),
            EDGE_TOLERANCE
        ));
        assert!(!edge_hit(
            DockEdge::Left,
            &FRAME,
            (1099.0, 300.0),
            EDGE_TOLERANCE
        ));
        assert!(edge_hit(
            DockEdge::Top,
            &FRAME,
            (600.0, 51.0),
            EDGE_TOLERANCE
        ));
        assert!(edge_hit(
            DockEdge::Bottom,
            &FRAME,
            (600.0, 648.0),
            EDGE_TOLERANCE
        ));
        assert!(!edge_hit(
            DockEdge::Bottom,
            &FRAME,
            (600.0, 51.0),
            EDGE_TOLERANCE
        ));
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
    fn edges_serialize_in_lowercase() {
        assert_eq!(serde_json::to_string(&DockEdge::Top).unwrap(), "\"top\"");
        let settings: DockSettings =
            serde_json::from_str("{\"enabled\":true,\"edge\":\"left\"}").unwrap();
        assert_eq!(
            settings,
            DockSettings {
                enabled: true,
                edge: DockEdge::Left
            }
        );
    }
}
