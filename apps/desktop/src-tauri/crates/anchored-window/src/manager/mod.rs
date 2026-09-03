mod tasks;
mod window;

use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager, WebviewWindow};

#[cfg(target_os = "macos")]
use crate::geometry::Rect;
use crate::lifecycle::{Lifecycle, RequestTransition};
use crate::model::{
    AnchorRegion, AnchoredWindowConfig, AnchoredWindowLifecycleEvent, AnchoredWindowRenderRequest,
    AnchoredWindowRequest, AnchoredWindowState, RevealPolicy, initial_height,
};
use crate::{REQUEST_EVENT, STATE_EVENT, platform};

struct Inner<T, P> {
    config: AnchoredWindowConfig,
    lifecycle: Mutex<Lifecycle<T, P>>,
    frame_update: Mutex<()>,
    #[cfg(target_os = "macos")]
    native_frame: Arc<Mutex<Option<Rect>>>,
    #[cfg(target_os = "linux")]
    pointer_tracker: Arc<crate::linux::PointerTracker>,
}

/// Creates, reuses, places, reveals, and conceals one anchored companion.
#[derive(Clone)]
pub struct AnchoredWindowManager<T, P = ()> {
    inner: Arc<Inner<T, P>>,
}

impl<T, P> AnchoredWindowManager<T, P>
where
    T: Clone + PartialEq + Send + Sync + Serialize + 'static,
    P: Clone + Send + Sync + Serialize + 'static,
{
    /// Create one manager for a host-owned anchor and companion configuration.
    pub fn new(config: AnchoredWindowConfig) -> Self {
        let initial_height = initial_height(config.height);
        Self {
            inner: Arc::new(Inner {
                config,
                lifecycle: Mutex::new(Lifecycle::new(initial_height)),
                frame_update: Mutex::new(()),
                #[cfg(target_os = "macos")]
                native_frame: Arc::new(Mutex::new(None)),
                #[cfg(target_os = "linux")]
                pointer_tracker: Arc::new(crate::linux::PointerTracker::new()),
            }),
        }
    }

    /// Build the hidden renderer if it does not exist yet.
    pub fn prewarm(&self, app: &tauri::AppHandle) -> tauri::Result<()> {
        let _frame_update = self.lock_frame_update();
        self.ensure_window(app).map(|_| ())
    }

    /// Retarget the resident renderer and apply its configured reveal policy.
    pub fn request(
        &self,
        app: &tauri::AppHandle,
        target: T,
        anchor_region: AnchorRegion,
    ) -> tauri::Result<AnchoredWindowRequest<T>> {
        self.request_with_presentation(app, target, anchor_region, None)
    }

    /// Retarget the resident renderer with optional instigator-owned content.
    pub fn request_with_presentation(
        &self,
        app: &tauri::AppHandle,
        target: T,
        anchor_region: AnchorRegion,
        initial_presentation: Option<P>,
    ) -> tauri::Result<AnchoredWindowRequest<T>> {
        let _frame_update = self.lock_frame_update();
        let window = self.ensure_window(app)?;
        let (transition, render_request) =
            self.prepare_request(target, anchor_region, initial_presentation);
        let delivery_pending = render_request.is_some();
        let request = self.apply_request_transition(app, &window, transition, delivery_pending)?;
        self.deliver_render_request(&window, render_request)?;
        self.emit_state(app)?;
        Ok(request)
    }

    fn prepare_request(
        &self,
        target: T,
        anchor_region: AnchorRegion,
        initial_presentation: Option<P>,
    ) -> (
        RequestTransition<T>,
        Option<AnchoredWindowRenderRequest<T, P>>,
    ) {
        let transition = self.lock_lifecycle().request(
            target,
            anchor_region,
            self.inner.config.reveal,
            initial_height(self.inner.config.height),
            initial_presentation,
        );
        let render_request = self.lock_lifecycle().pending_render_request();
        (transition, render_request)
    }

    fn apply_request_transition(
        &self,
        app: &tauri::AppHandle,
        window: &WebviewWindow,
        transition: RequestTransition<T>,
        delivery_pending: bool,
    ) -> tauri::Result<AnchoredWindowRequest<T>> {
        match transition {
            RequestTransition::Retained {
                request,
                reposition,
            } => {
                if delivery_pending {
                    self.retry_pending_native_transition(app, window)?;
                } else if reposition {
                    self.apply_size_and_position(app, window)?;
                }
                Ok(request)
            }
            RequestTransition::Retargeted {
                request,
                reveal_now,
            } => {
                if self.inner.config.reveal == RevealPolicy::AfterPresentation {
                    platform::hide(window)
                } else if reveal_now {
                    self.reveal_placeholder(app, window)
                } else {
                    Ok(())
                }?;
                Ok(request)
            }
        }
    }

    fn retry_pending_native_transition(
        &self,
        app: &tauri::AppHandle,
        window: &WebviewWindow,
    ) -> tauri::Result<()> {
        if self.inner.config.reveal == RevealPolicy::AfterPresentation {
            return platform::hide(window);
        }
        let should_reveal = {
            let lifecycle = self.lock_lifecycle();
            lifecycle.renderer_ready
                && !lifecycle.visible
                && lifecycle.initial_presentation.is_none()
        };
        if should_reveal {
            self.reveal_placeholder(app, window)?;
        }
        Ok(())
    }

    fn deliver_render_request(
        &self,
        window: &WebviewWindow,
        request: Option<AnchoredWindowRenderRequest<T, P>>,
    ) -> tauri::Result<()> {
        let Some(request) = request else {
            return Ok(());
        };
        window.emit(REQUEST_EVENT, &request)?;
        let delivered = self.lock_lifecycle().mark_delivered(request.generation);
        debug_assert!(delivered, "the frame update serializes renderer delivery");
        Ok(())
    }

    /// Apply the retained native frame after the renderer commits the new target shell.
    pub fn retarget_committed(
        &self,
        app: &tauri::AppHandle,
        generation: u64,
    ) -> tauri::Result<bool> {
        self.retarget_committed_with_height(app, generation, None)
    }

    /// Apply the retained frame after the renderer commits the new target.
    ///
    /// A measured content height also confirms that the initial presentation is visible.
    pub fn retarget_committed_with_height(
        &self,
        app: &tauri::AppHandle,
        generation: u64,
        content_height: Option<f64>,
    ) -> tauri::Result<bool> {
        let _frame_update = self.lock_frame_update();
        let Some(window) = app.get_webview_window(&self.inner.config.label) else {
            return Ok(false);
        };
        let presented = {
            let mut lifecycle = self.lock_lifecycle();
            if !lifecycle.retarget_commit_is_current(generation) {
                return Ok(false);
            }
            if let Some(height) = content_height {
                lifecycle.record_height(self.clamp_height(height));
            }
            content_height.is_some() && lifecycle.awaiting_presentation
        };
        self.apply_size_and_position(app, &window)?;
        let committed = {
            let mut lifecycle = self.lock_lifecycle();
            if presented {
                lifecycle.presented(generation)
            } else {
                lifecycle.retarget_committed(generation)
            }
        };
        debug_assert!(committed, "the frame update serializes the retarget commit");
        self.emit_state(app)?;
        Ok(true)
    }

    /// Ask the renderer to clear its content before the native window hides.
    pub fn conceal(&self, app: &tauri::AppHandle) -> tauri::Result<AnchoredWindowRequest<T>> {
        let _frame_update = self.lock_frame_update();
        let (request, ready) = {
            let mut lifecycle = self.lock_lifecycle();
            let request = lifecycle.conceal();
            (request, lifecycle.renderer_ready)
        };
        let render_request = self.lock_lifecycle().render_request();
        self.emit_state(app)?;
        self.deliver_conceal_request(app, &render_request, ready)?;
        Ok(request)
    }

    /// Start concealment and hide at once because the anchor is leaving the screen.
    pub fn conceal_for_anchor_hide(&self, app: &tauri::AppHandle) -> tauri::Result<()> {
        let _frame_update = self.lock_frame_update();
        let (request, ready) = {
            let mut lifecycle = self.lock_lifecycle();
            let request = lifecycle.conceal();
            (request, lifecycle.renderer_ready)
        };
        let render_request = self.lock_lifecycle().render_request();
        if let Some(window) = app.get_webview_window(&self.inner.config.label) {
            platform::hide(&window)?;
        }
        {
            let mut lifecycle = self.lock_lifecycle();
            if lifecycle.generation == request.generation {
                lifecycle.force_hidden();
            }
        }
        self.emit_state(app)?;
        self.deliver_conceal_request(app, &render_request, ready)
    }

    /// Mark the current renderer load ready and deliver the latest request.
    pub fn renderer_ready(
        &self,
        window: &WebviewWindow,
        renderer_generation: u64,
    ) -> tauri::Result<bool> {
        let _frame_update = self.lock_frame_update();
        let Some(reveal_now) = ({
            let mut lifecycle = self.lock_lifecycle();
            lifecycle.renderer_ready(renderer_generation, self.inner.config.reveal)
        }) else {
            return Ok(false);
        };
        let request = self.lock_lifecycle().pending_render_request();
        if reveal_now {
            self.reveal_placeholder(window.app_handle(), window)?;
        }
        self.deliver_render_request(window, request)?;
        if let Err(error) = self.emit_state(window.app_handle()) {
            tracing::warn!(%error, "failed to emit anchored-window renderer state");
        }
        Ok(true)
    }

    /// Reveal the current request after the renderer has committed its content.
    pub fn presented(
        &self,
        app: &tauri::AppHandle,
        generation: u64,
        content_height: Option<f64>,
    ) -> tauri::Result<bool> {
        let _frame_update = self.lock_frame_update();
        let Some(window) = app.get_webview_window(&self.inner.config.label) else {
            return Ok(false);
        };
        let (presentation_pending, reveal_pending, should_apply_frame) = {
            let mut lifecycle = self.lock_lifecycle();
            if generation != lifecycle.generation || lifecycle.target.is_none() {
                return Ok(false);
            }
            let height_changed = content_height
                .map(|height| lifecycle.record_height(self.clamp_height(height)))
                .unwrap_or(false);
            let presentation_pending = lifecycle.awaiting_presentation;
            (
                presentation_pending,
                presentation_pending && !lifecycle.visible,
                presentation_pending || height_changed,
            )
        };
        if should_apply_frame {
            self.apply_size_and_position(app, &window)?;
        }
        if presentation_pending && reveal_pending {
            if !self.anchor_is_visible(app) {
                self.lock_lifecycle().force_hidden();
                return Ok(false);
            }
            #[cfg(target_os = "linux")]
            self.inner.pointer_tracker.reset_for_show();
            platform::show(&window, self.inner.config.interaction)?;
        }
        let should_reveal = presentation_pending && self.lock_lifecycle().presented(generation);
        if should_reveal && let Err(error) = self.emit_state(app) {
            tracing::warn!(%error, "failed to emit anchored-window presented state");
        }
        Ok(should_reveal)
    }

    /// Hide after the renderer confirms that it cleared the current generation.
    pub fn concealed(&self, app: &tauri::AppHandle, generation: u64) -> bool {
        let _frame_update = self.lock_frame_update();
        if !self.lock_lifecycle().concealment_is_current(generation) {
            return false;
        }
        if let Some(window) = app.get_webview_window(&self.inner.config.label)
            && let Err(error) = platform::hide(&window)
        {
            tracing::warn!(%error, "failed to hide the concealed anchored window");
            return false;
        }
        let concealed = self.lock_lifecycle().concealed(generation);
        if concealed && let Err(error) = self.emit_state(app) {
            tracing::warn!(%error, "failed to emit anchored-window concealed state");
        }
        concealed
    }

    /// Return the typed lifecycle snapshot for host-owned IPC.
    pub fn state(&self) -> AnchoredWindowState<T> {
        self.lock_lifecycle().state()
    }

    /// Keep a visible companion attached when its anchor geometry changes.
    pub fn handle_anchor_event(&self, window: &tauri::Window, event: &tauri::WindowEvent) {
        if window.label() != self.inner.config.anchor_label {
            return;
        }
        match event {
            tauri::WindowEvent::Moved(_)
            | tauri::WindowEvent::Resized(_)
            | tauri::WindowEvent::ScaleFactorChanged { .. } => {
                let _frame_update = self.lock_frame_update();
                if !self.lock_lifecycle().can_reposition() {
                    return;
                }
                if let Some(companion) = window
                    .app_handle()
                    .get_webview_window(&self.inner.config.label)
                    && let Err(error) =
                        self.apply_size_and_position(window.app_handle(), &companion)
                {
                    tracing::warn!(%error, "failed to reposition the anchored window");
                }
            }
            tauri::WindowEvent::Destroyed => {
                if let Err(error) = self.conceal_for_anchor_hide(window.app_handle()) {
                    tracing::warn!(%error, "failed to conceal the anchored window with its anchor");
                }
            }
            _ => {}
        }
    }

    /// Reset renderer readiness after native destruction.
    pub fn handle_companion_destroyed(&self) {
        let _frame_update = self.lock_frame_update();
        self.lock_lifecycle().renderer_destroyed();
        #[cfg(target_os = "linux")]
        self.inner.pointer_tracker.reset_for_install();
    }

    fn lock_lifecycle(&self) -> std::sync::MutexGuard<'_, Lifecycle<T, P>> {
        match self.inner.lifecycle.lock() {
            Ok(lifecycle) => lifecycle,
            Err(poisoned) => {
                tracing::error!("the anchored-window lifecycle lock is poisoned");
                poisoned.into_inner()
            }
        }
    }

    fn lock_frame_update(&self) -> std::sync::MutexGuard<'_, ()> {
        match self.inner.frame_update.lock() {
            Ok(update) => update,
            Err(poisoned) => {
                tracing::error!("the anchored-window frame lock is poisoned");
                poisoned.into_inner()
            }
        }
    }

    fn emit_state(&self, app: &tauri::AppHandle) -> tauri::Result<()> {
        let Some(anchor) = app.get_webview_window(&self.inner.config.anchor_label) else {
            return Ok(());
        };
        anchor.emit(
            STATE_EVENT,
            AnchoredWindowLifecycleEvent {
                companion_label: self.inner.config.label.clone(),
                state: self.state(),
            },
        )
    }
}

#[cfg(test)]
mod tests;
