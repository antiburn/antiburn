use serde::Serialize;
use tauri::{Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
#[cfg(not(target_os = "macos"))]
use tauri::{PhysicalPosition, PhysicalSize};

#[cfg(not(target_os = "macos"))]
use crate::geometry::{Rect, place_left_preferred};
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
        platform::configure(builder, self.inner.config.material).build()
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
        let Some(window) = app.get_webview_window(&self.inner.config.label) else {
            return Some(false);
        };
        match window.is_visible() {
            Ok(true) => {}
            Ok(false) => return Some(false),
            Err(error) => {
                tracing::warn!(%error, "failed to read anchored-window visibility");
                return None;
            }
        }

        #[cfg(target_os = "macos")]
        {
            Some(crate::macos::cursor_inside(&self.inner.native_frame))
        }

        #[cfg(not(target_os = "macos"))]
        {
            let cursor = app.cursor_position().map_err(|error| {
                tracing::warn!(%error, "failed to read the anchored-window cursor position");
            });
            let position = window.outer_position().map_err(|error| {
                tracing::warn!(%error, "failed to read the anchored-window position");
            });
            let size = window.outer_size().map_err(|error| {
                tracing::warn!(%error, "failed to read the anchored-window size");
            });
            let (Ok(cursor), Ok(position), Ok(size)) = (cursor, position, size) else {
                return None;
            };
            let left = f64::from(position.x);
            let top = f64::from(position.y);
            Some(
                cursor.x >= left
                    && cursor.x < left + f64::from(size.width)
                    && cursor.y >= top
                    && cursor.y < top + f64::from(size.height),
            )
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
        if let Err(error) = platform::show(window, self.inner.config.interaction) {
            self.lock_lifecycle().force_hidden();
            return Err(error);
        }
        Ok(())
    }
}
