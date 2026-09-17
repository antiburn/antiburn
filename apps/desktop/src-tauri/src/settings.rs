//! The standalone settings window.
//!
//! Unlike the popover this is an ordinary window with real decorations: a
//! place to read and change configuration, not a transient surface. It is
//! resizable from a preferred 960×680 size and created on demand. Closing it destroys its webview, so
//! the next request starts a new renderer from persisted settings.
//!
//! On macOS the title bar is an overlay: decorations (traffic lights, system
//! shadow, real close semantics) are kept, the bar itself is transparent, and
//! the floating title text is hidden — the frontend paints a
//! `data-tauri-drag-region` strip across the top as the drag handle (see
//! `src/views/SettingsView.tsx`). Windows and Linux keep the stock title bar.

use std::sync::Mutex;
use std::time::Instant;

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::window_lifecycle::{self, ManagedWindowReadiness};
use crate::window_placement::{center_on_active_monitor, resize_on_current_monitor};
use crate::window_readiness::{OpenAction, WindowReadiness, renderer_generation_script};

/// Window label. Also listed in `capabilities/default.json`.
pub const LABEL: &str = "settings";

/// Event the shell emits to move an *already open* settings window to a pane.
pub const EVENT_PANE: &str = "settings:pane";

/// Event the shell emits after Settings reaches the screen.
pub const EVENT_SHOWN: &str = "settings:shown";

/// The pane a caller asked the window to open on, until the window takes it.
///
/// Two paths need this and only one of them can use an event. A window being
/// created has no webview listening yet, so the frontend asks for the pending
/// pane as it mounts ([`crate::commands::take_settings_pane`]); a window that
/// already exists is never re-mounted, so it is told through [`EVENT_PANE`].
/// The value is *taken* rather than read, so a request can never be applied
/// twice or apply late.
#[derive(Default)]
pub struct PendingPane(Mutex<Option<String>>);

impl PendingPane {
    fn set(&self, pane: Option<String>) {
        *self.lock() = pane;
    }

    /// Read and clear the pending pane.
    pub fn take(&self) -> Option<String> {
        self.lock().take()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Renderer lifecycle for the Settings window.
#[derive(Default)]
pub struct SettingsWindowState {
    readiness: Mutex<WindowReadiness>,
    minimum_size: Mutex<Option<tauri::LogicalSize<f64>>>,
}

impl ManagedWindowReadiness for SettingsWindowState {
    fn readiness(&self) -> std::sync::MutexGuard<'_, WindowReadiness> {
        self.readiness
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Dedicated frontend entry for the settings window.
const URL: &str = "settings.html";

// The preferred size leaves room for the sidebar and the content column.
const WIDTH: f64 = 960.0;
const HEIGHT: f64 = 680.0;

// One CSS pixel protects the 720px navigation breakpoint from native rounding.
const MIN_WIDTH: f64 = 721.0;
const MIN_HEIGHT: f64 = 480.0;

fn minimum_dimensions(factor: f64, available: (f64, f64)) -> tauri::LogicalSize<f64> {
    tauri::LogicalSize::new(
        (MIN_WIDTH * factor).ceil().min(available.0),
        (MIN_HEIGHT * factor).ceil().min(available.1),
    )
}

/// Keep Settings resizable above its scaled minimum, within the current work area.
pub fn reconcile_interface_scale(
    app: &AppHandle,
    scale: crate::interface_scale::InterfaceScale,
) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(LABEL) else {
        return Ok(());
    };
    let Some(monitor) = window.current_monitor()?.or(window.primary_monitor()?) else {
        return Ok(());
    };
    let dpi = window.scale_factor()?;
    let monitor_dpi = monitor.scale_factor();
    if !dpi.is_finite() || dpi <= 0.0 || !monitor_dpi.is_finite() || monitor_dpi <= 0.0 {
        return Ok(());
    }
    let inner = window.inner_size()?;
    let outer = window.outer_size()?;
    let area = monitor.work_area();
    let available = (
        f64::from(area.size.width) / monitor_dpi
            - f64::from(outer.width.saturating_sub(inner.width)) / dpi,
        f64::from(area.size.height) / monitor_dpi
            - f64::from(outer.height.saturating_sub(inner.height)) / dpi,
    );
    if available.0 < 1.0 || available.1 < 1.0 {
        return Ok(());
    }
    let minimum = minimum_dimensions(scale.factor(), available);
    let state = app.state::<SettingsWindowState>();
    let changed = {
        let mut cached = state.minimum_size.lock().unwrap_or_else(|e| e.into_inner());
        let changed = *cached != Some(minimum);
        *cached = Some(minimum);
        changed
    };
    if changed && let Err(error) = window.set_min_size(Some(minimum)) {
        *state.minimum_size.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return Err(error);
    }
    let current = (f64::from(inner.width) / dpi, f64::from(inner.height) / dpi);
    let target = (
        current.0.clamp(minimum.width, available.0),
        current.1.clamp(minimum.height, available.1),
    );
    if target != current {
        resize_on_current_monitor(&window, target.0, target.1)?;
    }
    Ok(())
}

/// Shows the settings window, creating it if this is the first request.
///
/// `pane` is the section the caller wants shown — the popover's attention
/// banners use it to land a reader on the pane that can fix what they were told
/// about, instead of on whichever pane they last left open.
pub fn open(app: &AppHandle, pane: Option<String>) -> tauri::Result<()> {
    let state = app.state::<SettingsWindowState>();
    app.state::<PendingPane>().set(pane.clone());
    let Some(existing) = app.get_webview_window(LABEL) else {
        let mut readiness = state.readiness();
        let action = readiness.request_open(Instant::now());
        let generation = match action {
            OpenAction::StartLoading { generation } | OpenAction::Rebuild { generation } => {
                generation
            }
            OpenAction::AwaitReady => return Ok(()),
            OpenAction::Reveal => {
                readiness.reset();
                match readiness.request_open(Instant::now()) {
                    OpenAction::StartLoading { generation } => generation,
                    _ => unreachable!("an idle lifecycle starts loading"),
                }
            }
        };
        drop(readiness);
        return build(app, generation);
    };

    let action = {
        let mut readiness = state.readiness();
        readiness.request_open(Instant::now())
    };
    if pane_event_reaches_renderer(action)
        && let Some(pane) = pane.as_ref()
    {
        app.emit_to(LABEL, EVENT_PANE, pane)?;
    }
    match action {
        OpenAction::Reveal => {
            show(&existing)?;
            Ok(())
        }
        OpenAction::AwaitReady => Ok(()),
        OpenAction::StartLoading { generation } | OpenAction::Rebuild { generation } => {
            if !state.readiness().defer_build_until_destroyed(generation) {
                return Ok(());
            }
            if let Err(error) = existing.destroy() {
                window_lifecycle::cancel_load::<SettingsWindowState>(app, generation);
                return Err(error);
            }
            Ok(())
        }
    }
}

fn pane_event_reaches_renderer(action: OpenAction) -> bool {
    matches!(action, OpenAction::Reveal | OpenAction::AwaitReady)
}

/// Build a deferred replacement after Tauri removes the old window label.
pub fn rebuild_after_destroy(app: &AppHandle) {
    let generation =
        window_lifecycle::begin_deferred_build::<SettingsWindowState>(app, Instant::now());
    let Some(generation) = generation else {
        return;
    };
    if let Err(error) = build(app, generation) {
        ::tracing::error!(event = "window_rebuild_failed", window = LABEL, error = %error);
    }
}

/// Builds the hidden settings window and starts its renderer load.
fn build(app: &AppHandle, generation: u64) -> tauri::Result<()> {
    *app.state::<SettingsWindowState>()
        .minimum_size
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    ::tracing::info!(
        event = "window_renderer_load_started",
        window = LABEL,
        generation
    );
    window_lifecycle::arm_stale_warning::<SettingsWindowState>(app, generation, LABEL);

    // Built hidden and positioned before the first show, so the window never
    // visibly jumps from a default position to the right one. Deliberately no
    // `.center()`: the builder's centering computes against the primary
    // monitor before the window has a screen, which is exactly the "opens on
    // the wrong display" this function exists to avoid.
    let interface_scale = crate::interface_scale::current(app);
    let width = WIDTH * interface_scale.factor();
    let height = HEIGHT * interface_scale.factor();
    let builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(URL.into()))
        .initialization_script(crate::interface_scale::append_initialization_script(
            renderer_generation_script(generation),
            interface_scale,
        ))
        .title("antiburn Settings")
        .inner_size(width, height)
        .resizable(true)
        .maximizable(false)
        .zoom_hotkeys_enabled(false)
        .visible(false)
        .on_page_load(|window, payload| {
            window_lifecycle::trace_page_load::<SettingsWindowState>(window, payload, LABEL);
        });

    #[cfg(target_os = "macos")]
    let builder = {
        // Overlay keeps decorations while making the title bar transparent;
        // `hidden_title` drops the floating title text. `.title(...)` above
        // stays so Mission Control and accessibility still name the window.
        // The webview covers the bar's area, so the frontend supplies the drag
        // handle (`data-tauri-drag-region` in SettingsView) — the ACL already
        // grants `core:window:allow-start-dragging`.
        builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
    };

    let window = match builder.build() {
        Ok(window) => window,
        Err(error) => {
            window_lifecycle::cancel_load::<SettingsWindowState>(app, generation);
            return Err(error);
        }
    };
    crate::wayland_titlebar::repair(&window);
    crate::interface_scale::apply_window(&window, interface_scale)?;
    center_on_active_monitor(&window, width, height);
    reconcile_interface_scale(app, interface_scale)?;
    Ok(())
}

/// Reveal Settings after React commits its shell.
pub fn renderer_ready(window: &tauri::WebviewWindow, generation: u64) {
    let app = window.app_handle();
    if let Err(error) =
        crate::interface_scale::apply_window(window, crate::interface_scale::current(app))
    {
        ::tracing::error!(event = "interface_scale_apply_failed", window = LABEL, error = %error);
        return;
    }
    if window_lifecycle::renderer_ready::<SettingsWindowState>(
        app,
        LABEL,
        generation,
        Instant::now(),
    ) && let Err(error) = show(window)
    {
        ::tracing::error!(event = "window_reveal_failed", window = LABEL, error = %error);
    }
}

fn show(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    let was_exposed =
        window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false);
    let app = window.app_handle();
    let scale = crate::interface_scale::current(app);
    window.set_min_size(None::<tauri::LogicalSize<f64>>)?;
    *app.state::<SettingsWindowState>()
        .minimum_size
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    center_on_active_monitor(window, WIDTH * scale.factor(), HEIGHT * scale.factor());
    reconcile_interface_scale(app, scale)?;
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    ::tracing::info!(event = "window_revealed", window = LABEL);
    if !was_exposed {
        let analytics_app = window.app_handle().clone();
        tauri::async_runtime::spawn_blocking(move || {
            crate::analytics::record_interaction(
                &analytics_app,
                crate::analytics::event::Interaction::SurfaceViewed {
                    surface: crate::analytics::event::Surface::Settings,
                    origin: crate::analytics::event::Origin::User,
                },
            );
        });
        let _ = window.emit(EVENT_SHOWN, ());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_size_keeps_the_sidebar_at_every_interface_scale() {
        for percent in crate::interface_scale::presets() {
            let scale = crate::interface_scale::InterfaceScale::new(*percent).unwrap();
            let minimum = minimum_dimensions(scale.factor(), (3000.0, 2000.0));
            assert!(minimum.width / scale.factor() > 720.0);
            assert!(minimum.height >= MIN_HEIGHT * scale.factor());
        }
        assert!(
            include_str!("../../src/views/SettingsView.tsx").contains("useViewportWidth() < 720")
        );
    }

    #[test]
    fn minimum_size_never_exceeds_the_available_content_area() {
        let minimum = minimum_dimensions(2.0, (1272.0, 688.0));
        assert_eq!(minimum, tauri::LogicalSize::new(1272.0, 688.0));
    }

    #[test]
    fn minimum_size_decreases_when_the_interface_scale_decreases() {
        assert_eq!(
            minimum_dimensions(1.0, (3000.0, 2000.0)),
            tauri::LogicalSize::new(721.0, 480.0)
        );
        assert_eq!(
            minimum_dimensions(2.0, (3000.0, 2000.0)),
            tauri::LogicalSize::new(1442.0, 960.0)
        );
    }

    #[test]
    fn the_url_uses_the_settings_entry() {
        assert_eq!(URL, "settings.html");
    }

    #[test]
    fn pane_requests_reach_ready_and_loading_renderers() {
        assert!(pane_event_reaches_renderer(OpenAction::Reveal));
        assert!(pane_event_reaches_renderer(OpenAction::AwaitReady));
        assert!(!pane_event_reaches_renderer(OpenAction::StartLoading {
            generation: 1
        }));
        assert!(!pane_event_reaches_renderer(OpenAction::Rebuild {
            generation: 2
        }));
    }

    #[test]
    fn a_requested_pane_is_delivered_exactly_once() {
        let pending = PendingPane::default();
        assert_eq!(pending.take(), None, "nothing was requested");

        pending.set(Some("sources".to_string()));
        assert_eq!(pending.take().as_deref(), Some("sources"));
        // Taken, not read: a window opened later must not jump to a pane
        // somebody asked for an hour ago.
        assert_eq!(pending.take(), None);
    }

    #[test]
    fn opening_without_a_pane_clears_an_earlier_request() {
        let pending = PendingPane::default();
        pending.set(Some("sources".to_string()));
        // The gear affordance asks for no pane in particular, and that must not
        // inherit the last banner's destination.
        pending.set(None);
        assert_eq!(pending.take(), None);
    }
}
