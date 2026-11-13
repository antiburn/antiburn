//! Own the native onboarding window and its renderer lifecycle.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::watch;

use crate::store::{OnboardingState, Store};
use crate::window_lifecycle::{self, ManagedWindowReadiness};
use crate::window_placement::{center_on_active_monitor, resize_on_current_monitor};
use crate::window_readiness::{OpenAction, WindowReadiness, renderer_generation_script};

pub(crate) const LABEL: &str = "onboarding";
const URL: &str = "onboarding.html";
const DEMO_CSP: &str = include_str!("../../src/onboarding/demo/csp.txt");
const VISIBILITY_EVENT: &str = "onboarding:visibility";
const WIDTH: f64 = 860.0;
const HEIGHT: f64 = 620.0;
const PRESENTATION_TIMEOUT: Duration = Duration::from_secs(10);
const PRESENTATION_ERROR: &str = "The Sessions window could not open.";

pub(crate) struct OnboardingWindowState {
    completed: AtomicBool,
    promotion_later_session: AtomicBool,
    dismissal_recorded: AtomicBool,
    readiness: Mutex<WindowReadiness>,
    main_presentation_changed: watch::Sender<()>,
}

impl OnboardingWindowState {
    pub(crate) fn load(store: &Store) -> Self {
        Self {
            completed: AtomicBool::new(
                store.onboarding_state().unwrap_or_default() == OnboardingState::Completed,
            ),
            promotion_later_session: AtomicBool::new(
                store.onboarding_state().unwrap_or_default() == OnboardingState::Completed,
            ),
            dismissal_recorded: AtomicBool::new(false),
            readiness: Mutex::default(),
            main_presentation_changed: watch::channel(()).0,
        }
    }

    #[cfg(debug_assertions)]
    fn reset(&self, store: &Store) -> anyhow::Result<()> {
        store.reset_onboarding()?;
        self.promotion_later_session.store(false, Ordering::Release);
        self.completed.store(false, Ordering::Release);
        self.dismissal_recorded.store(false, Ordering::Release);
        self.readiness().reset();
        Ok(())
    }

    pub(crate) fn promotion_later_session(&self) -> bool {
        self.promotion_later_session.load(Ordering::Acquire)
    }

    pub(crate) fn completed(&self) -> bool {
        self.completed.load(Ordering::Acquire)
    }

    pub(crate) fn mark_completed(&self) {
        self.completed.store(true, Ordering::Release);
    }
}

impl ManagedWindowReadiness for OnboardingWindowState {
    fn readiness(&self) -> MutexGuard<'_, WindowReadiness> {
        self.readiness
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Restart the native onboarding flow from Welcome in development builds.
#[cfg(debug_assertions)]
pub(crate) fn reset(app: &AppHandle) -> anyhow::Result<()> {
    app.state::<OnboardingWindowState>()
        .reset(&app.state::<Store>())?;
    app.emit("onboarding:changed", ())?;
    open(app)?;
    Ok(())
}

/// Open a fresh renderer or focus the current window.
pub(crate) fn open(app: &AppHandle) -> tauri::Result<()> {
    let state = app.state::<OnboardingWindowState>();
    let existing = app.get_webview_window(LABEL);
    let action = {
        let mut readiness = state.readiness();
        if existing.is_none() {
            readiness.reset();
        }
        readiness.request_open(Instant::now())
    };
    match (existing, action) {
        (Some(window), OpenAction::Reveal) => show(&window),
        (
            Some(window),
            OpenAction::StartLoading { generation } | OpenAction::Rebuild { generation },
        ) => {
            state.readiness().defer_build_until_destroyed(generation);
            if let Err(error) = window.destroy() {
                window_lifecycle::cancel_load::<OnboardingWindowState>(app, generation);
                return Err(error);
            }
            Ok(())
        }
        (None, OpenAction::StartLoading { generation }) => build(app, generation),
        _ => Ok(()),
    }
}

pub(crate) fn rebuild_after_destroy(app: &AppHandle) {
    if app.get_webview_window(LABEL).is_some() {
        return;
    }
    if let Some(generation) =
        window_lifecycle::begin_deferred_build::<OnboardingWindowState>(app, Instant::now())
        && !app.state::<OnboardingWindowState>().completed()
        && let Err(error) = build(app, generation)
    {
        ::tracing::warn!(event = "onboarding_window_rebuild_failed", error = %error);
    }
}

fn build(app: &AppHandle, generation: u64) -> tauri::Result<()> {
    app.state::<OnboardingWindowState>()
        .dismissal_recorded
        .store(false, Ordering::Release);
    let scale = crate::interface_scale::current(app);
    let (width, height) = preferred_dimensions(scale);
    let builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(URL.into()))
        .initialization_script(crate::interface_scale::append_initialization_script(
            renderer_generation_script(generation),
            scale,
        ))
        .title("Welcome to antiburn")
        .inner_size(width, height)
        .resizable(false)
        .maximizable(false)
        .zoom_hotkeys_enabled(false)
        .visible(false)
        .on_web_resource_request(|request, response| {
            if request.uri().path() == "/onboarding-demo/index.html" {
                response.headers_mut().insert(
                    tauri::http::header::CONTENT_SECURITY_POLICY,
                    tauri::http::HeaderValue::from_static(DEMO_CSP.trim()),
                );
            }
        })
        .on_page_load(move |window, payload| {
            #[cfg(feature = "memory-probe")]
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                crate::memory_probe::report_web_content(&window, generation);
            }
            window_lifecycle::trace_page_load::<OnboardingWindowState>(window, payload, LABEL);
        });
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    let window = match builder.build() {
        Ok(window) => window,
        Err(error) => {
            window_lifecycle::cancel_load::<OnboardingWindowState>(app, generation);
            return Err(error);
        }
    };
    if let Err(error) = crate::interface_scale::apply_window(&window, scale) {
        ::tracing::warn!(event = "interface_scale_apply_failed", window = LABEL, error = %error);
    }
    center_on_active_monitor(&window, width, height);
    window_lifecycle::arm_stale_warning::<OnboardingWindowState>(app, generation, LABEL);
    Ok(())
}

fn preferred_dimensions(scale: crate::interface_scale::InterfaceScale) -> (f64, f64) {
    (WIDTH * scale.factor(), HEIGHT * scale.factor())
}

pub(crate) fn reconcile_interface_scale(
    app: &AppHandle,
    scale: crate::interface_scale::InterfaceScale,
) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window(LABEL) else {
        return Ok(());
    };
    let (width, height) = preferred_dimensions(scale);
    resize_on_current_monitor(&window, width, height)
}

pub(crate) fn renderer_ready(window: &WebviewWindow, generation: u64) {
    let ready_window = window.clone();
    if let Err(error) = window.run_on_main_thread(move || {
        if window_lifecycle::renderer_ready::<OnboardingWindowState>(
            ready_window.app_handle(),
            LABEL,
            generation,
            Instant::now(),
        ) && let Err(error) = show(&ready_window)
        {
            ::tracing::warn!(event = "onboarding_window_reveal_failed", error = %error);
        }
    }) {
        ::tracing::warn!(event = "onboarding_window_reveal_failed", error = %error);
    }
}

fn show(window: &WebviewWindow) -> tauri::Result<()> {
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    emit_visibility_changed(window);
    Ok(())
}

pub(crate) fn emit_visibility_changed(window: &WebviewWindow) {
    emit_visibility(window, crate::main_window::is_visible(window));
}

pub(crate) fn on_close_requested(window: &WebviewWindow) {
    emit_visibility(window, false);
    let app = window.app_handle().clone();
    let state = app.state::<OnboardingWindowState>();
    if state.completed() || state.dismissal_recorded.swap(true, Ordering::AcqRel) {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        if let Ok(state) = app.state::<Store>().onboarding_state() {
            crate::analytics::record_onboarding_dismissed(&app, state);
        }
    });
}

fn emit_visibility(window: &WebviewWindow, visible: bool) {
    if let Err(error) = window.emit_to(LABEL, VISIBILITY_EVENT, visible) {
        ::tracing::debug!(event = "onboarding_window_visibility_emit_failed", error = %error);
    }
}

#[tauri::command]
pub(crate) fn get_onboarding_window_visible(window: WebviewWindow) -> bool {
    window.label() == LABEL && crate::main_window::is_visible(&window)
}

pub(crate) fn notify_main_presentation_changed(app: &AppHandle) {
    if let Some(state) = app.try_state::<OnboardingWindowState>() {
        state.main_presentation_changed.send_replace(());
    }
}

/// Keep onboarding open until Sessions is visible.
pub(crate) async fn finish(app: &AppHandle) -> Result<(), String> {
    use crate::main_window::{self, on_main_value};

    let mut changes = app
        .state::<OnboardingWindowState>()
        .main_presentation_changed
        .subscribe();
    let revision = on_main_value(app, main_window::open_sessions_after_onboarding).await??;

    let deadline = Instant::now() + PRESENTATION_TIMEOUT;
    let wait = async {
        loop {
            if on_main_value(app, move |app| {
                close_after_main_presentation(app, deadline, revision)
            })
            .await??
            {
                return Ok(());
            }
            changes
                .changed()
                .await
                .map_err(|_| PRESENTATION_ERROR.to_owned())?;
        }
    };
    tokio::time::timeout(PRESENTATION_TIMEOUT, wait)
        .await
        .map_err(|_| PRESENTATION_ERROR.to_owned())?
}

fn close_after_main_presentation(
    app: &AppHandle,
    deadline: Instant,
    revision: u64,
) -> Result<bool, String> {
    if Instant::now() >= deadline || !crate::main_window::has_presented_target(app, revision) {
        return Ok(false);
    }
    if let Some(window) = app.get_webview_window(LABEL) {
        emit_visibility(&window, false);
        window.destroy().map_err(|error| error.to_string())?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(debug_assertions)]
    fn debug_reset_clears_completion_and_replaces_the_renderer() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        store
            .set_onboarding_state(OnboardingState::Completed)
            .unwrap();
        let window = OnboardingWindowState::load(&store);
        window.readiness().request_open(Instant::now());
        window.dismissal_recorded.store(true, Ordering::Release);
        window.reset(&store).unwrap();
        assert!(!window.completed());
        assert!(!window.promotion_later_session());
        assert!(!window.dismissal_recorded.load(Ordering::Acquire));
        assert_eq!(store.onboarding_state().unwrap(), OnboardingState::Welcome);
        assert!(matches!(
            window.readiness().request_open(Instant::now()),
            OpenAction::StartLoading { .. }
        ));
    }

    #[test]
    fn onboarding_launch_selection_tracks_durable_completion() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        store.set_onboarding_state(OnboardingState::Hud).unwrap();
        let window = OnboardingWindowState::load(&store);
        assert!(!window.completed());
        assert!(!window.promotion_later_session());

        store
            .set_onboarding_state(OnboardingState::Completed)
            .unwrap();
        window.mark_completed();
        assert!(window.completed());
        assert!(!window.promotion_later_session());
        assert!(OnboardingWindowState::load(&store).promotion_later_session());
        assert!(OnboardingWindowState::load(&store).completed());
    }

    #[test]
    fn onboarding_capability_is_scoped_to_its_window() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/onboarding.json")).unwrap();
        assert_eq!(capability["windows"], serde_json::json!([LABEL]));
        let permissions = capability["permissions"].as_array().unwrap();
        for required in [
            "allow-get-onboarding-state",
            "allow-set-onboarding-state",
            "allow-get-onboarding-window-visible",
            "allow-get-settings",
            "allow-set-interface-scale",
            "allow-note-interaction",
            "allow-window-ready",
        ] {
            assert!(permissions.contains(&serde_json::json!(required)));
        }
        for excluded in ["default", "allow-set-settings", "allow-scan-now"] {
            assert!(!permissions.contains(&serde_json::json!(excluded)));
        }
        for config in [
            include_str!("../tauri.conf.json"),
            include_str!("../tauri.debug.conf.json"),
        ] {
            let config: serde_json::Value = serde_json::from_str(config).unwrap();
            assert!(
                config["app"]["security"]["capabilities"]
                    .as_array()
                    .unwrap()
                    .contains(&capability["identifier"])
            );
        }
    }
}

#[tauri::command]
pub(crate) fn cloud_preview_window(window: WebviewWindow, action: &str) -> Result<(), String> {
    if !cfg!(debug_assertions) || window.label() != "cloud-dev" {
        return Err("Cloud preview is unavailable.".to_owned());
    }
    let result = match action {
        "ready" => window.show().and_then(|()| window.set_focus()),
        "close" => window.destroy(),
        "drag" => window.start_dragging(),
        _ => return Err("Unknown preview action.".to_owned()),
    };
    result.map_err(|error| error.to_string())
}
