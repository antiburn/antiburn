//! Fit utility windows to a monitor's work area before reveal or during resize.

use tauri::WebviewWindow;

/// Center `window` on the monitor the cursor is on.
///
/// `width` and `height` are the window's **logical** size, passed in rather
/// than read back: a window built hidden has not necessarily been laid out yet,
/// and a window being re-shown reports a physical size scaled for whichever
/// display it was last on.
///
/// The cursor is the active-monitor signal because every path into these
/// windows follows a click — the tray menu, the popover's affordances, ⌘, —
/// so the pointer is on the display the reader is working on. The one path that
/// does not is the onboarding window at launch, where the cursor is still the
/// best guess available and the fallbacks below catch the rest.
///
/// Falls back through the window's current monitor to the primary one, and does
/// nothing if even that cannot be resolved: a window at its old position is
/// better than one at (0,0).
pub fn center_on_active_monitor(window: &WebviewWindow, width: f64, height: f64) {
    let monitor = window
        .cursor_position()
        .ok()
        .and_then(|cursor| {
            window.available_monitors().ok()?.into_iter().find(|m| {
                let pos = m.position();
                let size = m.size();
                cursor.x >= pos.x as f64
                    && cursor.x < pos.x as f64 + size.width as f64
                    && cursor.y >= pos.y as f64
                    && cursor.y < pos.y as f64 + size.height as f64
            })
        })
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };

    let scale = monitor.scale_factor();
    let old_scale = window.scale_factor().unwrap_or(scale);
    let chrome = match (window.outer_size(), window.inner_size()) {
        (Ok(outer), Ok(inner)) if old_scale.is_finite() && old_scale > 0.0 => (
            f64::from(outer.width.saturating_sub(inner.width)) / old_scale,
            f64::from(outer.height.saturating_sub(inner.height)) / old_scale,
        ),
        _ => (0.0, 0.0),
    };
    let area = monitor.work_area();
    let Some((size, position)) = fit_geometry(
        (width, height),
        chrome,
        scale,
        (
            f64::from(area.position.x),
            f64::from(area.position.y),
            f64::from(area.size.width),
            f64::from(area.size.height),
        ),
    ) else {
        return;
    };
    let logical_size = tauri::LogicalSize::new(size.width / scale, size.height / scale);
    if let Err(error) = window
        .set_position(position)
        .and_then(|()| window.set_size(logical_size))
    {
        tracing::error!(event = "window_placement_failed", window = window.label(), %error);
    }
}

/// Resize existing content around its center, within its current monitor's work area.
pub fn resize_on_current_monitor(
    window: &WebviewWindow,
    width: f64,
    height: f64,
) -> tauri::Result<()> {
    let monitor = match window.current_monitor()? {
        Some(monitor) => Some(monitor),
        None => window.primary_monitor()?,
    };
    let Some(monitor) = monitor else {
        return Ok(());
    };
    let dpi = monitor.scale_factor();
    let old_dpi = window.scale_factor()?;
    if !old_dpi.is_finite() || old_dpi <= 0.0 {
        return Ok(());
    }
    let outer = window.outer_size()?;
    let inner = window.inner_size()?;
    let origin = window.outer_position()?;
    let chrome = (
        f64::from(outer.width.saturating_sub(inner.width)) / old_dpi,
        f64::from(outer.height.saturating_sub(inner.height)) / old_dpi,
    );
    let area = monitor.work_area();
    let Some((size, position)) = fit_resized_geometry(
        (width, height),
        chrome,
        dpi,
        (
            f64::from(area.position.x),
            f64::from(area.position.y),
            f64::from(area.size.width),
            f64::from(area.size.height),
        ),
        (
            f64::from(origin.x),
            f64::from(origin.y),
            f64::from(outer.width),
            f64::from(outer.height),
        ),
    ) else {
        return Ok(());
    };
    window.set_size(tauri::LogicalSize::new(size.width / dpi, size.height / dpi))?;
    window.set_position(position)
}

/// Convert logical content and chrome sizes once, then fit the physical work area.
fn fit_geometry(
    preferred: (f64, f64),
    chrome: (f64, f64),
    dpi: f64,
    area: (f64, f64, f64, f64),
) -> Option<(tauri::PhysicalSize<f64>, tauri::PhysicalPosition<f64>)> {
    if !dpi.is_finite() || dpi <= 0.0 {
        return None;
    }
    let (left, top, available_width, available_height) = area;
    let chrome_width = chrome.0 * dpi;
    let chrome_height = chrome.1 * dpi;
    if available_width <= chrome_width || available_height <= chrome_height {
        return None;
    }
    let width = (preferred.0 * dpi).clamp(1.0, available_width - chrome_width);
    let height = (preferred.1 * dpi).clamp(1.0, available_height - chrome_height);
    Some((
        tauri::PhysicalSize::new(width, height),
        tauri::PhysicalPosition::new(
            left + (available_width - width - chrome_width) / 2.0,
            top + (available_height - height - chrome_height) / 2.0,
        ),
    ))
}

fn fit_resized_geometry(
    preferred: (f64, f64),
    chrome: (f64, f64),
    dpi: f64,
    area: (f64, f64, f64, f64),
    frame: (f64, f64, f64, f64),
) -> Option<(tauri::PhysicalSize<f64>, tauri::PhysicalPosition<f64>)> {
    let (size, _) = fit_geometry(preferred, chrome, dpi, area)?;
    let outer_width = size.width + chrome.0 * dpi;
    let outer_height = size.height + chrome.1 * dpi;
    let x = frame.0 + (frame.2 - outer_width) / 2.0;
    let y = frame.1 + (frame.3 - outer_height) / 2.0;
    Some((
        size,
        tauri::PhysicalPosition::new(
            x.clamp(area.0, (area.0 + area.2 - outer_width).max(area.0)),
            y.clamp(area.1, (area.1 + area.3 - outer_height).max(area.1)),
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::{fit_geometry, fit_resized_geometry};

    #[test]
    fn live_resize_preserves_the_window_center_instead_of_recentering_the_display() {
        let (size, position) = fit_resized_geometry(
            (850.0, 600.0),
            (0.0, 0.0),
            1.0,
            (0.0, 30.0, 2560.0, 1410.0),
            (100.0, 100.0, 680.0, 480.0),
        )
        .unwrap();
        assert_eq!((size.width, size.height), (850.0, 600.0));
        assert_eq!((position.x, position.y), (15.0, 40.0));
    }

    #[test]
    fn live_resize_clamps_content_and_chrome_on_a_negative_origin_retina_display() {
        let (size, position) = fit_resized_geometry(
            (1360.0, 960.0),
            (8.0, 32.0),
            2.0,
            (-2560.0, 66.0, 2560.0, 1500.0),
            (-1500.0, 200.0, 1376.0, 1024.0),
        )
        .unwrap();
        assert_eq!((size.width, size.height), (2544.0, 1436.0));
        assert_eq!((position.x, position.y), (-2560.0, 66.0));
    }

    #[test]
    fn live_resize_shrinks_to_ninety_percent_without_losing_its_center() {
        let (size, position) = fit_resized_geometry(
            (612.0, 432.0),
            (0.0, 0.0),
            2.0,
            (0.0, 66.0, 3024.0, 1898.0),
            (152.0, 66.0, 2720.0, 1898.0),
        )
        .unwrap();
        assert_eq!((size.width, size.height), (1224.0, 864.0));
        assert_eq!((position.x, position.y), (900.0, 583.0));
    }

    #[test]
    fn preserves_preferred_size_and_converts_dpi_once() {
        let (size, position) = fit_geometry(
            (850.0, 600.0),
            (0.0, 0.0),
            2.0,
            (-2560.0, 50.0, 2560.0, 1500.0),
        )
        .unwrap();
        assert_eq!((size.width, size.height), (1700.0, 1200.0));
        assert_eq!((position.x, position.y), (-2130.0, 200.0));
    }

    #[test]
    fn fits_enlarged_content_and_native_chrome_inside_the_work_area() {
        let (size, position) = fit_geometry(
            (1920.0, 1360.0),
            (8.0, 32.0),
            1.5,
            (100.0, 45.0, 1200.0, 800.0),
        )
        .unwrap();
        assert_eq!((size.width, size.height), (1188.0, 752.0));
        assert_eq!((position.x, position.y), (100.0, 45.0));
    }

    #[test]
    fn rejects_invalid_monitor_geometry() {
        assert!(fit_geometry((680.0, 480.0), (0.0, 0.0), 0.0, (0.0, 0.0, 100.0, 100.0)).is_none());
        assert!(fit_geometry((680.0, 480.0), (0.0, 32.0), 2.0, (0.0, 0.0, 100.0, 40.0)).is_none());
    }
}
