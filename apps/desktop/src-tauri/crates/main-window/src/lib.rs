//! The native mechanism for the ordinary antiburn window.
//!
//! The desktop shell owns renderer readiness, persistence, and launch policy.

use serde::{Deserialize, Serialize};
use tauri::webview::PageLoadPayload;
use tauri::{AppHandle, PhysicalRect, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// The stable label used by native events and capabilities.
pub const LABEL: &str = "main";
/// The dedicated frontend entry.
pub const URL: &str = "main.html";
/// The first inner width in logical pixels.
pub const DEFAULT_WIDTH: f64 = 900.0;
/// The first inner height in logical pixels.
pub const DEFAULT_HEIGHT: f64 = 600.0;
/// The minimum inner width in logical pixels.
pub const MIN_WIDTH: f64 = 800.0;
/// The minimum inner height in logical pixels.
pub const MIN_HEIGHT: f64 = 560.0;

const INITIAL_WORK_AREA_FRACTION: f64 = 0.85;

/// The last normal bounds and maximized state, in physical pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
    #[serde(default = "unit_scale")]
    pub scale_factor: f64,
}

/// A new hidden renderer and the normal bounds applied before maximization.
pub struct BuiltWindow {
    pub window: WebviewWindow,
    pub placement: Option<Placement>,
}

const fn unit_scale() -> f64 {
    1.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Frame {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl From<&PhysicalRect<i32, u32>> for Frame {
    fn from(rect: &PhysicalRect<i32, u32>) -> Self {
        Self {
            x: rect.position.x,
            y: rect.position.y,
            width: rect.size.width,
            height: rect.size.height,
        }
    }
}

/// Build a hidden decorated window and apply a valid saved placement.
pub fn build<F>(
    app: &AppHandle,
    initialization_script: String,
    placement: Option<&Placement>,
    on_page_load: F,
) -> tauri::Result<BuiltWindow>
where
    F: Fn(WebviewWindow, PageLoadPayload<'_>) + Send + Sync + 'static,
{
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(URL.into()))
        .initialization_script(initialization_script)
        .title("antiburn")
        .inner_size(DEFAULT_WIDTH, DEFAULT_HEIGHT)
        .resizable(true)
        .maximizable(true)
        .decorations(true)
        .skip_taskbar(false)
        .visible(false)
        .on_page_load(on_page_load);

    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }

    let window = builder.build()?;
    // Geometry failure must not leak a hidden label that blocks every retry.
    // Keep the usable default window and let the shell persist its later move.
    let applied = apply_placement(&window, placement).ok();
    Ok(BuiltWindow {
        window,
        placement: applied,
    })
}

/// Show and focus an existing renderer.
pub fn reveal(window: &WebviewWindow) -> tauri::Result<()> {
    window.show()?;
    window.unminimize()?;
    window.set_focus()
}

/// Hide the window without destroying its renderer.
pub fn conceal(window: &WebviewWindow) -> tauri::Result<()> {
    window.hide()
}

/// Capture normal bounds without replacing them with maximized bounds.
pub fn capture(window: &WebviewWindow, previous: Option<&Placement>) -> Option<Placement> {
    let maximized = window.is_maximized().ok()?;
    if maximized {
        return previous.cloned().map(|mut placement| {
            placement.maximized = true;
            placement
        });
    }
    let position = window.outer_position().ok()?;
    let size = window.inner_size().ok()?;
    let scale_factor = window.scale_factor().ok()?;
    Some(Placement {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        maximized: false,
        scale_factor,
    })
}

fn apply_placement(
    window: &WebviewWindow,
    placement: Option<&Placement>,
) -> tauri::Result<Placement> {
    let monitors = window.available_monitors()?;
    let frames: Vec<Frame> = monitors
        .iter()
        .map(|monitor| Frame::from(monitor.work_area()))
        .collect();
    let primary = window
        .primary_monitor()?
        .map(|monitor| Frame::from(monitor.work_area()));
    let active = window.cursor_position().ok().and_then(|cursor| {
        #[cfg(target_os = "macos")]
        {
            let primary_scale = window.primary_monitor().ok()??.scale_factor();
            let monitor = window
                .monitor_from_point(cursor.x / primary_scale, cursor.y / primary_scale)
                .ok()??;
            let frame = Frame::from(monitor.work_area());
            frames.iter().position(|item| *item == frame)
        }
        #[cfg(not(target_os = "macos"))]
        monitors.iter().position(|monitor| {
            let area = monitor.work_area();
            cursor.x >= f64::from(area.position.x)
                && cursor.x < f64::from(area.position.x) + f64::from(area.size.width)
                && cursor.y >= f64::from(area.position.y)
                && cursor.y < f64::from(area.position.y) + f64::from(area.size.height)
        })
    });
    #[cfg(target_os = "macos")]
    let saved_monitor = placement.and_then(|saved| {
        monitor_for_saved_scaled(
            saved,
            &frames,
            &monitors
                .iter()
                .map(tauri::Monitor::scale_factor)
                .collect::<Vec<_>>(),
        )
    });
    #[cfg(not(target_os = "macos"))]
    let saved_monitor = placement.and_then(|saved| monitor_for_saved(saved, &frames));
    let target_index = saved_monitor
        .or(active)
        .or_else(|| primary.and_then(|frame| frames.iter().position(|item| *item == frame)))
        .unwrap_or(0);
    let scale = monitors
        .get(target_index)
        .map_or(1.0, tauri::Monitor::scale_factor);
    let outer = window.outer_size()?;
    let inner = window.inner_size()?;
    let current_scale = window.scale_factor()?;
    let chrome_width =
        (f64::from(outer.width.saturating_sub(inner.width)) / current_scale * scale).round() as u32;
    let chrome_height = (f64::from(outer.height.saturating_sub(inner.height)) / current_scale
        * scale)
        .round() as u32;

    let fallback = frames.get(target_index).copied().or(primary);
    #[cfg(target_os = "macos")]
    let normalized = placement
        .filter(|_| saved_monitor.is_some())
        .map(|saved| placement_at_scale(saved, scale));
    #[cfg(target_os = "macos")]
    let placement = normalized.as_ref();
    let target_frames: Vec<Frame> = fallback.into_iter().collect();
    let target = validated_placement(
        placement,
        &target_frames,
        fallback,
        scale,
        chrome_width,
        chrome_height,
    );
    #[cfg(target_os = "macos")]
    {
        // AppKit uses logical dimensions before an asynchronous display move completes.
        let size = tauri::LogicalSize::new(
            f64::from(target.width) / target.scale_factor,
            f64::from(target.height) / target.scale_factor,
        );
        window.set_min_size(Some(tauri::LogicalSize::new(
            MIN_WIDTH.min(size.width),
            MIN_HEIGHT.min(size.height),
        )))?;
        window.set_size(size)?;
        window.set_position(tauri::LogicalPosition::new(
            f64::from(target.x) / target.scale_factor,
            f64::from(target.y) / target.scale_factor,
        ))?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let min_width = ((MIN_WIDTH * scale).round() as u32).min(target.width.max(1));
        let min_height = ((MIN_HEIGHT * scale).round() as u32).min(target.height.max(1));
        window.set_min_size(Some(tauri::PhysicalSize::new(min_width, min_height)))?;
        window.set_size(tauri::PhysicalSize::new(target.width, target.height))?;
        window.set_position(tauri::PhysicalPosition::new(target.x, target.y))?;
    }
    if target.maximized {
        window.maximize()?;
    }
    Ok(target)
}

fn validated_placement(
    saved: Option<&Placement>,
    monitors: &[Frame],
    primary: Option<Frame>,
    scale: f64,
    chrome_width: u32,
    chrome_height: u32,
) -> Placement {
    let fallback = primary
        .or_else(|| monitors.first().copied())
        .unwrap_or(Frame {
            x: 0,
            y: 0,
            width: DEFAULT_WIDTH as u32,
            height: DEFAULT_HEIGHT as u32,
        });
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let min_width = (MIN_WIDTH * scale).round() as u32;
    let min_height = (MIN_HEIGHT * scale).round() as u32;

    let Some(saved) = saved else {
        return centered_default(fallback, scale, chrome_width, chrome_height);
    };
    let Some(frame) = monitor_for_saved(saved, monitors).map(|index| monitors[index]) else {
        return centered_default(fallback, scale, chrome_width, chrome_height);
    };

    let saved_scale = if saved.scale_factor.is_finite() && saved.scale_factor > 0.0 {
        saved.scale_factor
    } else {
        1.0
    };
    let max_width = frame.width.saturating_sub(chrome_width).max(1);
    let max_height = frame.height.saturating_sub(chrome_height).max(1);
    let restored_width = (f64::from(saved.width) / saved_scale * scale).round() as u32;
    let restored_height = (f64::from(saved.height) / saved_scale * scale).round() as u32;
    let width = restored_width.clamp(min_width.min(max_width), max_width);
    let height = restored_height.clamp(min_height.min(max_height), max_height);
    let outer_width = width.saturating_add(chrome_width).min(frame.width);
    let outer_height = height.saturating_add(chrome_height).min(frame.height);
    let max_x = i64::from(frame.x) + i64::from(frame.width.saturating_sub(outer_width));
    let max_y = i64::from(frame.y) + i64::from(frame.height.saturating_sub(outer_height));
    Placement {
        x: i64::from(saved.x).clamp(i64::from(frame.x), max_x) as i32,
        y: i64::from(saved.y).clamp(i64::from(frame.y), max_y) as i32,
        width,
        height,
        maximized: saved.maximized,
        scale_factor: scale,
    }
}

fn centered_default(frame: Frame, scale: f64, chrome_width: u32, chrome_height: u32) -> Placement {
    let max_width = ((f64::from(frame.width) * INITIAL_WORK_AREA_FRACTION).floor() as u32)
        .saturating_sub(chrome_width)
        .max(1);
    let max_height = ((f64::from(frame.height) * INITIAL_WORK_AREA_FRACTION).floor() as u32)
        .saturating_sub(chrome_height)
        .max(1);
    let width = ((DEFAULT_WIDTH * scale).round() as u32).min(max_width);
    let height = ((DEFAULT_HEIGHT * scale).round() as u32).min(max_height);
    let outer_width = width.saturating_add(chrome_width).min(frame.width);
    let outer_height = height.saturating_add(chrome_height).min(frame.height);
    Placement {
        x: frame.x + ((frame.width - outer_width) / 2) as i32,
        y: frame.y + ((frame.height - outer_height) / 2) as i32,
        width,
        height,
        maximized: false,
        scale_factor: scale,
    }
}

fn monitor_for_saved(saved: &Placement, monitors: &[Frame]) -> Option<usize> {
    monitors
        .iter()
        .enumerate()
        .map(|(index, frame)| (index, overlap(saved, *frame)))
        .filter(|(_, area)| *area > 0)
        .max_by_key(|(_, area)| *area)
        .map(|(index, _)| index)
}

#[cfg(any(target_os = "macos", test))]
fn placement_at_scale(saved: &Placement, scale: f64) -> Placement {
    let previous_scale = if saved.scale_factor.is_finite() && saved.scale_factor > 0.0 {
        saved.scale_factor
    } else {
        1.0
    };
    let ratio = scale / previous_scale;
    Placement {
        x: (f64::from(saved.x) * ratio).round() as i32,
        y: (f64::from(saved.y) * ratio).round() as i32,
        width: (f64::from(saved.width) * ratio).round() as u32,
        height: (f64::from(saved.height) * ratio).round() as u32,
        maximized: saved.maximized,
        scale_factor: scale,
    }
}

#[cfg(any(target_os = "macos", test))]
fn monitor_for_saved_scaled(
    saved: &Placement,
    monitors: &[Frame],
    scales: &[f64],
) -> Option<usize> {
    monitors
        .iter()
        .zip(scales)
        .enumerate()
        .map(|(index, (frame, scale))| {
            let area = overlap(&placement_at_scale(saved, *scale), *frame) as f64 / scale.powi(2);
            (index, area)
        })
        .filter(|(_, area)| *area > 0.0)
        .max_by(|(_, left), (_, right)| left.total_cmp(right))
        .map(|(index, _)| index)
}

fn overlap(saved: &Placement, frame: Frame) -> u64 {
    let left = i64::from(saved.x).max(i64::from(frame.x));
    let top = i64::from(saved.y).max(i64::from(frame.y));
    let right = (i64::from(saved.x) + i64::from(saved.width))
        .min(i64::from(frame.x) + i64::from(frame.width));
    let bottom = (i64::from(saved.y) + i64::from(saved.height))
        .min(i64::from(frame.y) + i64::from(frame.height));
    u64::try_from((right - left).max(0)).unwrap_or_default()
        * u64::try_from((bottom - top).max(0)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY: Frame = Frame {
        x: 0,
        y: 24,
        width: 1_440,
        height: 876,
    };

    #[test]
    fn default_geometry_centers_inside_the_work_area() {
        assert_eq!(
            validated_placement(None, &[PRIMARY], Some(PRIMARY), 1.0, 0, 0),
            Placement {
                x: 270,
                y: 162,
                width: 900,
                height: 600,
                maximized: false,
                scale_factor: 1.0,
            }
        );
    }

    #[test]
    fn restored_geometry_is_clamped_to_its_visible_work_area() {
        let saved = Placement {
            x: -200,
            y: -100,
            width: 2_000,
            height: 200,
            maximized: true,
            scale_factor: 1.0,
        };
        assert_eq!(
            validated_placement(Some(&saved), &[PRIMARY], Some(PRIMARY), 1.0, 0, 0),
            Placement {
                x: 0,
                y: 24,
                width: 1_440,
                height: 560,
                maximized: true,
                scale_factor: 1.0,
            }
        );
    }

    #[test]
    fn saved_small_window_expands_to_the_desktop_minimum() {
        let saved = Placement {
            x: 100,
            y: 100,
            width: 560,
            height: 420,
            maximized: false,
            scale_factor: 1.0,
        };
        let restored = validated_placement(Some(&saved), &[PRIMARY], Some(PRIMARY), 1.0, 0, 0);
        assert_eq!((restored.width, restored.height), (800, 560));
    }

    #[test]
    fn disconnected_monitor_geometry_uses_the_primary_default() {
        let saved = Placement {
            x: 8_000,
            y: 5_000,
            width: 900,
            height: 600,
            maximized: false,
            scale_factor: 1.0,
        };
        assert_eq!(
            validated_placement(Some(&saved), &[PRIMARY], Some(PRIMARY), 1.0, 0, 0),
            validated_placement(None, &[PRIMARY], Some(PRIMARY), 1.0, 0, 0)
        );
    }

    #[test]
    fn overlap_selects_the_monitor_with_most_of_the_window() {
        let secondary = Frame {
            x: 1_440,
            y: 0,
            width: 1_920,
            height: 1_080,
        };
        let saved = Placement {
            x: 1_300,
            y: 100,
            width: 1_000,
            height: 700,
            maximized: false,
            scale_factor: 1.0,
        };
        assert_eq!(monitor_for_saved(&saved, &[PRIMARY, secondary]), Some(1));
    }

    #[test]
    fn logical_size_survives_a_scale_factor_change() {
        let saved = Placement {
            x: 10,
            y: 40,
            width: 1_100,
            height: 720,
            maximized: false,
            scale_factor: 1.0,
        };
        let retina = Frame {
            x: 0,
            y: 0,
            width: 3_000,
            height: 2_000,
        };
        let restored = validated_placement(Some(&saved), &[retina], Some(retina), 2.0, 0, 0);
        assert_eq!((restored.width, restored.height), (2_200, 1_440));
    }

    #[test]
    fn tiny_work_area_takes_precedence_over_the_normal_minimum() {
        let tiny = Frame {
            x: 20,
            y: 30,
            width: 640,
            height: 440,
        };
        let placement = validated_placement(None, &[tiny], Some(tiny), 1.0, 16, 40);
        assert_eq!((placement.x, placement.y), (68, 63));
        assert_eq!((placement.width, placement.height), (528, 334));
    }

    #[test]
    fn retina_default_keeps_its_logical_dimensions() {
        let retina = Frame {
            x: 0,
            y: 66,
            width: 3_024,
            height: 1_920,
        };
        let placement = validated_placement(None, &[retina], Some(retina), 2.0, 0, 0);
        assert_eq!((placement.width, placement.height), (1_800, 1_200));
        assert!(!placement.maximized);
    }

    #[test]
    fn saved_user_size_is_not_limited_by_the_initial_cap() {
        let saved = Placement {
            x: 0,
            y: 24,
            width: 1_400,
            height: 850,
            maximized: false,
            scale_factor: 1.0,
        };
        assert_eq!(
            validated_placement(Some(&saved), &[PRIMARY], Some(PRIMARY), 1.0, 0, 0),
            saved
        );
    }

    #[test]
    fn mixed_scale_monitor_selection_uses_logical_overlap() {
        let primary = Frame {
            x: 0,
            y: 0,
            width: 2_880,
            height: 1_800,
        };
        let external = Frame {
            x: 1_440,
            y: 0,
            width: 1_920,
            height: 1_080,
        };
        let saved = Placement {
            x: 1_500,
            y: 40,
            width: 900,
            height: 600,
            maximized: false,
            scale_factor: 1.0,
        };
        assert_eq!(
            monitor_for_saved_scaled(&saved, &[primary, external], &[2.0, 1.0]),
            Some(1)
        );
        let scaled_external = Frame {
            x: 2_880,
            y: 0,
            width: 3_840,
            height: 2_160,
        };
        assert_eq!(
            monitor_for_saved_scaled(&saved, &[primary, scaled_external], &[2.0, 2.0]),
            Some(1)
        );
        let normalized = placement_at_scale(&saved, 2.0);
        assert_eq!(
            (
                normalized.x,
                normalized.y,
                normalized.width,
                normalized.height
            ),
            (3_000, 80, 1_800, 1_200)
        );
        assert_eq!(
            validated_placement(
                Some(&normalized),
                &[scaled_external],
                Some(scaled_external),
                2.0,
                0,
                0
            ),
            normalized
        );
    }
}
