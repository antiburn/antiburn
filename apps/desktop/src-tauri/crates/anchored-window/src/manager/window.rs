#[cfg(target_os = "linux")]
use std::sync::Arc;

use serde::Serialize;
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
#[cfg(not(target_os = "macos"))]
use tauri::{PhysicalPosition, PhysicalSize};

use crate::geometry::CursorProximity;
#[cfg(not(target_os = "macos"))]
use crate::geometry::{Point, Rect, classify_cursor, place_left_preferred};
use crate::model::{HeightPolicy, PlacementPolicy, normalized_height_policy};
use crate::platform;

use super::AnchoredWindowManager;

impl<T, P> AnchoredWindowManager<T, P>
where
    T: Clone + PartialEq + Send + Sync + Serialize + 'static,
    P: Clone + Send + Sync + Serialize + 'static,
{
    pub(super) fn ensure_window(&self, app: &tauri::AppHandle) -> tauri::Result<WebviewWindow> {
        if let Some(window) = app.get_webview_window(&self.inner.config.label) {
            #[cfg(target_os = "linux")]
            crate::linux::install_pointer_tracking(
                &window,
                Arc::clone(&self.inner.pointer_tracker),
            )?;
            return Ok(window);
        }
        let (renderer_generation, initial_height) = {
            let mut lifecycle = self.lock_lifecycle();
            lifecycle.renderer_generation = lifecycle.renderer_generation.wrapping_add(1).max(1);
            lifecycle.renderer_ready = false;
            (lifecycle.renderer_generation, lifecycle.height)
        };
        let script = format!(
            "Object.defineProperty(globalThis, \"__ANTIBURN_WINDOW_GENERATION__\", {{ value: {renderer_generation}, writable: false, configurable: false }});"
        );
        let builder = WebviewWindowBuilder::new(
            app,
            &self.inner.config.label,
            WebviewUrl::App(self.inner.config.route.clone().into()),
        )
        .initialization_script(script)
        .title(&self.inner.config.title)
        .inner_size(self.inner.config.width, initial_height)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .shadow(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .transparent(platform::is_transparent(self.inner.config.material))
        .focusable(self.inner.config.interaction == crate::model::InteractionPolicy::Interactive);
        let window = platform::configure(builder, self.inner.config.material).build()?;
        #[cfg(target_os = "linux")]
        crate::linux::install_pointer_tracking(&window, Arc::clone(&self.inner.pointer_tracker))?;
        Ok(window)
    }

    pub(super) fn apply_size_and_position(
        &self,
        app: &tauri::AppHandle,
        companion: &WebviewWindow,
    ) -> tauri::Result<()> {
        let Some(anchor) = app.get_webview_window(&self.inner.config.anchor_label) else {
            return Ok(());
        };
        let PlacementPolicy::LeftPreferred { gap, screen_margin } = self.inner.config.placement;
        self.apply_platform_frame(&anchor, companion, gap, screen_margin)
    }

    #[cfg(target_os = "macos")]
    fn apply_platform_frame(
        &self,
        anchor: &WebviewWindow,
        companion: &WebviewWindow,
        gap: f64,
        screen_margin: f64,
    ) -> tauri::Result<()> {
        let (height, anchor_region) = {
            let lifecycle = self.lock_lifecycle();
            let height = match self.inner.config.height {
                HeightPolicy::Content { .. } => Some(lifecycle.height),
                HeightPolicy::MatchAnchor => None,
            };
            (height, lifecycle.anchor_region)
        };
        crate::macos::apply_frame(
            anchor,
            companion,
            crate::macos::FrameRequest {
                width: self.inner.config.width,
                height,
                anchor_region,
                gap,
                screen_margin,
            },
            self.inner.native_frame.clone(),
        )
    }

    #[cfg(not(target_os = "macos"))]
    fn apply_platform_frame(
        &self,
        anchor: &WebviewWindow,
        companion: &WebviewWindow,
        gap: f64,
        screen_margin: f64,
    ) -> tauri::Result<()> {
        let position = anchor.outer_position()?;
        let size = anchor.outer_size()?;
        let Some(monitor) = anchor.current_monitor()?.or(anchor.primary_monitor()?) else {
            return Ok(());
        };
        let scale = monitor.scale_factor();
        let area = monitor.work_area();
        let (height, anchor_region) = {
            let lifecycle = self.lock_lifecycle();
            let height = match self.inner.config.height {
                HeightPolicy::Content { .. } => lifecycle.height,
                HeightPolicy::MatchAnchor => f64::from(size.height) / scale,
            };
            (height, lifecycle.anchor_region)
        };
        companion.set_size(PhysicalSize::new(
            (self.inner.config.width * scale).round().max(1.0) as u32,
            (height * scale).round().max(1.0) as u32,
        ))?;
        let point = place_left_preferred(
            Rect {
                x: f64::from(position.x),
                y: f64::from(position.y)
                    + anchor_region.top_within(f64::from(size.height) / scale) * scale,
                width: f64::from(size.width),
                height: anchor_region.height * scale,
            },
            Rect {
                x: f64::from(area.position.x),
                y: f64::from(area.position.y),
                width: f64::from(area.size.width),
                height: f64::from(area.size.height),
            },
            self.inner.config.width,
            height,
            scale,
            gap,
            screen_margin,
        );
        companion.set_position(PhysicalPosition::new(point.x, point.y))?;
        Ok(())
    }

    pub(super) fn anchor_is_visible(&self, app: &tauri::AppHandle) -> bool {
        let Some(window) = app.get_webview_window(&self.inner.config.anchor_label) else {
            return false;
        };
        match window.is_visible() {
            Ok(visible) => visible,
            Err(error) => {
                tracing::warn!(%error, "failed to read anchored-window anchor visibility");
                false
            }
        }
    }

    pub(super) fn cursor_is_over_companion(&self, app: &tauri::AppHandle) -> Option<bool> {
        self.cursor_location(app, 0.0)
            .map(|proximity| proximity == CursorProximity::Inside)
    }

    pub(super) fn cursor_location(
        &self,
        app: &tauri::AppHandle,
        edge_tolerance: f64,
    ) -> Option<CursorProximity> {
        let Some(window) = app.get_webview_window(&self.inner.config.label) else {
            return Some(CursorProximity::Outside);
        };
        match window.is_visible() {
            Ok(true) => {}
            Ok(false) => return Some(CursorProximity::Outside),
            Err(_) => return None,
        }

        #[cfg(target_os = "macos")]
        {
            crate::macos::cursor_location(&self.inner.native_frame, edge_tolerance)
        }

        #[cfg(not(target_os = "macos"))]
        {
            #[cfg(target_os = "linux")]
            match self.inner.pointer_tracker.source() {
                crate::linux::CursorSource::Pending => return None,
                crate::linux::CursorSource::Local(proximity) => return Some(proximity),
                crate::linux::CursorSource::Global => {}
            }

            let cursor = app.cursor_position().ok()?;
            let position = window.outer_position().ok()?;
            let size = window.outer_size().ok()?;
            let scale = window.scale_factor().ok()?;
            Some(classify_cursor(
                Rect {
                    x: f64::from(position.x),
                    y: f64::from(position.y),
                    width: f64::from(size.width),
                    height: f64::from(size.height),
                },
                Point {
                    x: cursor.x,
                    y: cursor.y,
                },
                edge_tolerance,
                scale,
            ))
        }
    }

    pub(super) fn clamp_height(&self, requested: f64) -> f64 {
        match normalized_height_policy(self.inner.config.height) {
            HeightPolicy::Content { initial, min, max } => {
                if requested.is_finite() {
                    requested.clamp(min, max)
                } else {
                    initial
                }
            }
            HeightPolicy::MatchAnchor => requested,
        }
    }

    pub(super) fn reveal_placeholder(
        &self,
        app: &tauri::AppHandle,
        window: &WebviewWindow,
    ) -> tauri::Result<()> {
        if let Err(error) = self.apply_size_and_position(app, window) {
            self.lock_lifecycle().force_hidden();
            return Err(error);
        }
        if !self.anchor_is_visible(app) {
            self.lock_lifecycle().force_hidden();
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        self.inner.pointer_tracker.reset_for_show();
        if let Err(error) = platform::show(window, self.inner.config.interaction) {
            self.lock_lifecycle().force_hidden();
            return Err(error);
        }
        Ok(())
    }
}
