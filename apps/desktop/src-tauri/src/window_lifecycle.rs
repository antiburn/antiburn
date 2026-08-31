//! Tauri adapters for the shared window renderer lifecycle.

use std::sync::MutexGuard;
use std::time::Instant;

use tauri::webview::{PageLoadEvent, PageLoadPayload};
use tauri::{AppHandle, Manager, WebviewWindow};

use crate::window_readiness::{ReadyAction, STALE_LOAD_AFTER, WindowReadiness};

/// Gives each managed window state access to its renderer lifecycle.
pub(crate) trait ManagedWindowReadiness {
    fn readiness(&self) -> MutexGuard<'_, WindowReadiness>;
}

/// Reset the lifecycle or take its deferred generation after the window ends.
pub(crate) fn begin_deferred_build<S>(app: &AppHandle, now: Instant) -> Option<u64>
where
    S: ManagedWindowReadiness + Send + Sync + 'static,
{
    app.state::<S>().readiness().begin_deferred_build(now)
}

/// Record renderer readiness and report whether the window must reveal.
pub(crate) fn renderer_ready<S>(
    app: &AppHandle,
    label: &'static str,
    generation: u64,
    now: Instant,
) -> bool
where
    S: ManagedWindowReadiness + Send + Sync + 'static,
{
    let action = app.state::<S>().readiness().renderer_ready(generation, now);
    match action {
        ReadyAction::Reveal { loading_for } => {
            ::tracing::info!(
                event = "window_renderer_ready",
                window = label,
                loading_ms = loading_for.as_millis() as u64,
                reveal = true
            );
            true
        }
        ReadyAction::StayHidden { loading_for } => {
            ::tracing::info!(
                event = "window_renderer_ready",
                window = label,
                loading_ms = loading_for.as_millis() as u64,
                reveal = false
            );
            false
        }
        ReadyAction::None => {
            // A report that does not match the active load never reveals the
            // window. Record it, or a hidden-forever window leaves no trace.
            ::tracing::debug!(
                event = "window_renderer_ready_ignored",
                window = label,
                generation
            );
            false
        }
    }
}

/// Record one renderer page-load phase.
pub(crate) fn trace_page_load<S>(
    window: WebviewWindow,
    payload: PageLoadPayload<'_>,
    label: &'static str,
) where
    S: ManagedWindowReadiness + Send + Sync + 'static,
{
    let phase = match payload.event() {
        PageLoadEvent::Started => "started",
        PageLoadEvent::Finished => "finished",
    };
    let loading_ms = window
        .app_handle()
        .state::<S>()
        .readiness()
        .loading_duration(Instant::now())
        .map(|duration| duration.as_millis() as u64);
    ::tracing::debug!(
        event = "window_page_load",
        window = label,
        phase,
        loading_ms
    );
}

/// Warn if the same renderer generation remains stale.
pub(crate) fn arm_stale_warning<S>(app: &AppHandle, generation: u64, label: &'static str)
where
    S: ManagedWindowReadiness + Send + Sync + 'static,
{
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STALE_LOAD_AFTER).await;
        if app
            .state::<S>()
            .readiness()
            .warning_is_current(generation, Instant::now())
        {
            ::tracing::warn!(
                event = "window_renderer_ready_timeout",
                window = label,
                generation,
                timeout_ms = STALE_LOAD_AFTER.as_millis() as u64
            );
        }
    });
}

/// Reset the lifecycle when the matching renderer build fails.
pub(crate) fn cancel_load<S>(app: &AppHandle, generation: u64)
where
    S: ManagedWindowReadiness + Send + Sync + 'static,
{
    let state = app.state::<S>();
    let mut readiness = state.readiness();
    if readiness.loading_generation() == Some(generation) {
        readiness.reset();
    }
}
