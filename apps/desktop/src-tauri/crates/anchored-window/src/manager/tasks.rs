use std::sync::{Arc, Weak};
use std::time::Duration;

use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::REQUEST_EVENT;
use crate::lifecycle::Lifecycle;
use crate::model::AnchoredWindowRenderRequest;
use crate::platform;

use super::{AnchoredWindowManager, Inner};

impl<T, P> AnchoredWindowManager<T, P>
where
    T: Clone + PartialEq + Send + Sync + Serialize + 'static,
    P: Clone + Send + Sync + Serialize + 'static,
{
    /// Delay concealment so the pointer can cross from the anchor to the companion.
    pub fn conceal_after(&self, app: &tauri::AppHandle, delay: Duration) {
        let Some((generation, task_token)) = self.reserve_conceal_task() else {
            return;
        };
        let manager = Arc::downgrade(&self.inner);
        let app_handle = app.clone();
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let task = tauri::async_runtime::spawn(async move {
            if start_rx.await.is_ok() {
                Self::run_delayed_conceal(manager, app_handle, delay, generation, task_token).await;
            }
        });
        let installed = self.install_task_when(task_token, task, |lifecycle| {
            lifecycle.generation == generation && lifecycle.target.is_some()
        });
        if installed && start_tx.send(()).is_err() {
            tracing::warn!("the anchored-window concealment task stopped before startup");
        }
    }

    fn reserve_conceal_task(&self) -> Option<(u64, u64)> {
        let mut lifecycle = self.lock_lifecycle();
        lifecycle.target.as_ref()?;
        let generation = lifecycle.generation;
        let task_token = lifecycle.reserve_task();
        Some((generation, task_token))
    }

    async fn run_delayed_conceal(
        manager: Weak<Inner<T, P>>,
        app: tauri::AppHandle,
        delay: Duration,
        generation: u64,
        task_token: u64,
    ) {
        tokio::time::sleep(delay).await;
        if !Self::wait_for_cursor_exit(&manager, |owner| owner.cursor_is_over_companion(&app)).await
        {
            return;
        }
        let Some(inner) = manager.upgrade() else {
            return;
        };
        Self { inner }.finish_scheduled_conceal(&app, generation, task_token);
    }

    fn finish_scheduled_conceal(&self, app: &tauri::AppHandle, generation: u64, task_token: u64) {
        let _frame_update = self.lock_frame_update();
        let transition = {
            let mut lifecycle = self.lock_lifecycle();
            lifecycle
                .conceal_scheduled(generation, task_token)
                .map(|request| (request, lifecycle.renderer_ready))
        };
        let Some((_request, ready)) = transition else {
            return;
        };
        let render_request = self.lock_lifecycle().render_request();
        if let Err(error) = self.emit_state(app) {
            tracing::warn!(%error, "failed to emit anchored-window concealment state");
        }
        if let Err(error) = self.deliver_conceal_request(app, &render_request, ready) {
            tracing::warn!(%error, "failed to deliver anchored-window concealment");
        }
    }

    pub(super) async fn wait_for_cursor_exit(
        manager: &Weak<Inner<T, P>>,
        mut cursor_is_inside: impl FnMut(&Self) -> Option<bool>,
    ) -> bool {
        loop {
            let cursor_is_inside = {
                let Some(inner) = manager.upgrade() else {
                    return false;
                };
                cursor_is_inside(&Self { inner })
            };
            if let Some(false) = cursor_is_inside {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    pub(super) fn deliver_conceal_request(
        &self,
        app: &tauri::AppHandle,
        request: &AnchoredWindowRenderRequest<T, P>,
        renderer_ready: bool,
    ) -> tauri::Result<()> {
        let Some(window) = app.get_webview_window(&self.inner.config.label) else {
            if self.lock_lifecycle().concealed(request.generation) {
                self.emit_state(app)?;
            }
            return Ok(());
        };
        if !renderer_ready {
            platform::hide(&window)?;
            if self.lock_lifecycle().concealed(request.generation) {
                self.emit_state(app)?;
            }
            return Ok(());
        }

        self.install_conceal_fallback(app, request.generation);
        window.emit(REQUEST_EVENT, request)?;
        let delivered = self.lock_lifecycle().mark_delivered(request.generation);
        debug_assert!(
            delivered,
            "the frame update serializes concealment delivery"
        );
        Ok(())
    }

    fn force_concealed(&self, app: &tauri::AppHandle, generation: u64, task_token: u64) {
        let _frame_update = self.lock_frame_update();
        if !self
            .lock_lifecycle()
            .fallback_task_is_current(generation, task_token)
        {
            return;
        }
        if let Some(window) = app.get_webview_window(&self.inner.config.label)
            && let Err(error) = platform::hide(&window)
        {
            tracing::warn!(%error, "failed to hide anchored window after fallback");
            return;
        }
        if self
            .lock_lifecycle()
            .fallback_concealed(generation, task_token)
            && let Err(error) = self.emit_state(app)
        {
            tracing::warn!(%error, "failed to emit anchored-window fallback state");
        }
    }

    fn install_conceal_fallback(&self, app: &tauri::AppHandle, generation: u64) {
        let Some(task_token) = self.reserve_fallback_task(generation) else {
            return;
        };
        let manager = Arc::downgrade(&self.inner);
        let app_handle = app.clone();
        let delay = self.inner.config.conceal_fallback;
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let task = tauri::async_runtime::spawn(async move {
            if start_rx.await.is_err() {
                return;
            }
            tokio::time::sleep(delay).await;
            if let Some(inner) = manager.upgrade() {
                Self { inner }.force_concealed(&app_handle, generation, task_token);
            }
        });
        let installed = self.install_task_when(task_token, task, |lifecycle| {
            lifecycle.fallback_is_current(generation)
        });
        if installed && start_tx.send(()).is_err() {
            tracing::warn!("the anchored-window fallback task stopped before startup");
        }
    }

    fn reserve_fallback_task(&self, generation: u64) -> Option<u64> {
        let mut lifecycle = self.lock_lifecycle();
        lifecycle
            .fallback_is_current(generation)
            .then(|| lifecycle.reserve_task())
    }

    fn install_task_when(
        &self,
        task_token: u64,
        task: tauri::async_runtime::JoinHandle<()>,
        is_current: impl FnOnce(&Lifecycle<T, P>) -> bool,
    ) -> bool {
        let result = {
            let mut lifecycle = self.lock_lifecycle();
            if is_current(&lifecycle) {
                lifecycle.install_task(task_token, task)
            } else {
                Err(task)
            }
        };
        match result {
            Ok(()) => true,
            Err(task) => {
                task.abort();
                false
            }
        }
    }
}
