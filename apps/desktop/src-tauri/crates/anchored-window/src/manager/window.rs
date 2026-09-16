#[cfg(target_os = "linux")]
use std::sync::Arc;

#[cfg(target_os = "macos")]
use crate::AnchorRegion;
use crate::companion::CompanionWindow;
use serde::Serialize;
use tauri::{Manager, WebviewWindow};
#[cfg(not(target_os = "macos"))]
use tauri::{PhysicalPosition, PhysicalSize};
#[cfg(not(target_os = "macos"))]
use tauri::{WebviewUrl, WebviewWindowBuilder};

use crate::geometry::CursorProximity;
#[cfg(not(target_os = "macos"))]
use crate::geometry::{Point, Rect, classify_cursor, fit_companion_frame};
use crate::platform;

use super::AnchoredWindowManager;

impl<T, P> AnchoredWindowManager<T, P>
where
    T: Clone + PartialEq + Send + Sync + Serialize + 'static,
    P: Clone + Send + Sync + Serialize + 'static,
{
    pub(super) fn ensure_window(&self, app: &tauri::AppHandle) -> tauri::Result<CompanionWindow> {
        if let Some(window) = self.companion(app) {
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
        #[cfg(target_os = "macos")]
        let window = {
            let manager = std::sync::Arc::downgrade(&self.inner);
            let failure_app = app.clone();
            let handler = self.inner.native_handler.ok_or_else(|| {
                tauri::Error::Io(std::io::Error::other("native companion handler is missing"))
            })?;
            let window = crate::macos::NativeWindow::create(
                app,
                &self.inner.config,
                renderer_generation,
                initial_height,
                self.interface_scale(),
                handler,
                move || {
                    if let Some(inner) = manager.upgrade() {
                        Self { inner }.native_renderer_failed(&failure_app, renderer_generation);
                    }
                },
            )?;
            *self
                .inner
                .native_window
                .lock()
                .expect("native companion slot mutex must not be poisoned") = Some(window.clone());
            window
        };
        #[cfg(not(target_os = "macos"))]
        let window = {
            let interface_scale = self.interface_scale();
            let script = format!(
                "Object.defineProperty(globalThis, \"__ANTIBURN_WINDOW_GENERATION__\", {{ value: {renderer_generation}, writable: false, configurable: false }});globalThis.__ANTIBURN_INTERFACE_SCALE_PERCENT__={};document.addEventListener('DOMContentLoaded',()=>document.documentElement?.style.setProperty('--interface-scale','{interface_scale}'),{{once:true}});",
                (interface_scale * 100.0).round() as u16,
            );
            let builder = WebviewWindowBuilder::new(
                app,
                &self.inner.config.label,
                WebviewUrl::App(self.inner.config.route.clone().into()),
            )
            .initialization_script(script)
            .title(&self.inner.config.title)
            .inner_size(
                self.inner.config.width * interface_scale,
                initial_height * interface_scale,
            )
            .resizable(false)
            .zoom_hotkeys_enabled(false)
            .maximizable(false)
            .minimizable(false)
            .decorations(false)
            .shadow(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .visible(false)
            .focused(false)
            .focusable(false);
            let window = platform::configure(builder).build()?;
            window.set_zoom(interface_scale)?;
            #[cfg(target_os = "linux")]
            crate::linux::install_pointer_tracking(
                &window,
                Arc::clone(&self.inner.pointer_tracker),
            )?;
            window
        };
        Ok(window)
    }

    pub(super) fn apply_size_and_position(
        &self,
        app: &tauri::AppHandle,
        companion: &CompanionWindow,
    ) -> tauri::Result<()> {
        let Some(anchor) = app.get_webview_window(&self.inner.config.anchor_label) else {
            return Ok(());
        };
        self.apply_platform_frame(
            &anchor,
            companion,
            self.inner.config.gap,
            self.inner.config.screen_margin,
        )
    }

    #[cfg(target_os = "macos")]
    fn apply_platform_frame(
        &self,
        anchor: &WebviewWindow,
        companion: &CompanionWindow,
        gap: f64,
        screen_margin: f64,
    ) -> tauri::Result<()> {
        let (height, anchor_region) = {
            let lifecycle = self.lock_lifecycle();
            let height = lifecycle.height;
            (height, lifecycle.anchor_region)
        };
        let interface_scale = self.interface_scale();
        crate::macos::apply_frame(
            anchor,
            companion,
            crate::macos::FrameRequest {
                width: self.inner.config.width * interface_scale,
                height: height * interface_scale,
                anchor_region: AnchorRegion {
                    top: anchor_region.top * interface_scale,
                    height: anchor_region.height * interface_scale,
                },
                gap: gap * interface_scale,
                screen_margin: screen_margin * interface_scale,
            },
            self.inner.native_frame.clone(),
        )
    }

    #[cfg(not(target_os = "macos"))]
    fn apply_platform_frame(
        &self,
        anchor: &WebviewWindow,
        companion: &CompanionWindow,
        gap: f64,
        screen_margin: f64,
    ) -> tauri::Result<()> {
        let position = anchor.outer_position()?;
        let size = anchor.outer_size()?;
        let Some(monitor) = anchor.current_monitor()?.or(anchor.primary_monitor()?) else {
            return Ok(());
        };
        let scale = monitor.scale_factor();
        let interface_scale = self.interface_scale();
        let area = monitor.work_area();
        let (height, anchor_region) = {
            let lifecycle = self.lock_lifecycle();
            let height = lifecycle.height;
            (height, lifecycle.anchor_region)
        };
        let frame = fit_companion_frame(
            Rect {
                x: f64::from(position.x),
                y: f64::from(position.y)
                    + anchor_region.top_within(f64::from(size.height) / scale / interface_scale)
                        * interface_scale
                        * scale,
                width: f64::from(size.width),
                height: anchor_region.height * interface_scale * scale,
            },
            Rect {
                x: f64::from(area.position.x),
                y: f64::from(area.position.y),
                width: f64::from(area.size.width),
                height: f64::from(area.size.height),
            },
            (
                self.inner.config.width * interface_scale * scale,
                height * interface_scale * scale,
            ),
            gap * interface_scale * scale,
            screen_margin * interface_scale * scale,
        );
        companion.set_size(PhysicalSize::new(frame.width as u32, frame.height as u32))?;
        companion.set_position(PhysicalPosition::new(frame.x, frame.y))?;
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
        let Some(window) = self.companion(app) else {
            return Some(CursorProximity::Outside);
        };
        match window.is_visible() {
            Ok(true) => {}
            Ok(false) => return Some(CursorProximity::Outside),
            Err(_) => return None,
        }

        #[cfg(target_os = "macos")]
        {
            crate::macos::cursor_location(
                &self.inner.native_frame,
                edge_tolerance * self.interface_scale(),
            )
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
                edge_tolerance * self.interface_scale(),
                scale,
            ))
        }
    }

    pub(super) fn clamp_height(&self, requested: f64) -> f64 {
        let (initial, min, max) = self.normalized_heights();
        if requested.is_finite() {
            requested.clamp(min, max)
        } else {
            initial
        }
    }

    pub(super) fn reveal_placeholder(
        &self,
        app: &tauri::AppHandle,
        window: &CompanionWindow,
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
        if let Err(error) = platform::show(window) {
            self.lock_lifecycle().force_hidden();
            return Err(error);
        }
        Ok(())
    }
}
