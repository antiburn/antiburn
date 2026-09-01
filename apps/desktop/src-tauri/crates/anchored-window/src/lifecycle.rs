use crate::model::{
    AnchorRegion, AnchoredWindowRenderRequest, AnchoredWindowRequest, AnchoredWindowState,
    RevealPolicy,
};

#[derive(Debug, PartialEq)]
pub(crate) enum RequestTransition<T> {
    Retained {
        request: AnchoredWindowRequest<T>,
        reposition: bool,
    },
    Retargeted {
        request: AnchoredWindowRequest<T>,
        reveal_now: bool,
    },
}

#[derive(Debug)]
pub(crate) struct Lifecycle<T, P> {
    pub(crate) generation: u64,
    pub(crate) target: Option<T>,
    pub(crate) initial_presentation: Option<P>,
    pub(crate) anchor_region: AnchorRegion,
    pub(crate) renderer_generation: u64,
    pub(crate) renderer_ready: bool,
    delivery_pending: bool,
    placeholder_reveal_pending: bool,
    pub(crate) visible: bool,
    pub(crate) awaiting_retarget_commit: bool,
    pub(crate) awaiting_presentation: bool,
    pub(crate) awaiting_concealment: bool,
    pub(crate) height: f64,
    measured_height: Option<f64>,
    pub(crate) task_revision: u64,
    pub(crate) task: Option<ScheduledTask>,
}

#[derive(Debug)]
pub(crate) struct ScheduledTask {
    pub(crate) token: u64,
    pub(crate) handle: tauri::async_runtime::JoinHandle<()>,
}

impl<T: Clone + PartialEq, P: Clone> Lifecycle<T, P> {
    pub(crate) fn new(initial_height: f64) -> Self {
        Self {
            generation: 0,
            target: None,
            initial_presentation: None,
            anchor_region: AnchorRegion {
                top: 0.0,
                height: 0.0,
            },
            renderer_generation: 0,
            renderer_ready: false,
            delivery_pending: false,
            placeholder_reveal_pending: false,
            visible: false,
            awaiting_retarget_commit: false,
            awaiting_presentation: false,
            awaiting_concealment: false,
            height: initial_height,
            measured_height: None,
            task_revision: 0,
            task: None,
        }
    }

    fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.generation = 1;
        }
        self.generation
    }

    pub(crate) fn cancel_task(&mut self) {
        self.task_revision = self.task_revision.wrapping_add(1).max(1);
        if let Some(task) = self.task.take() {
            task.handle.abort();
        }
    }

    pub(crate) fn reserve_task(&mut self) -> u64 {
        self.cancel_task();
        self.task_revision
    }

    pub(crate) fn install_task(
        &mut self,
        token: u64,
        handle: tauri::async_runtime::JoinHandle<()>,
    ) -> Result<(), tauri::async_runtime::JoinHandle<()>> {
        if self.task_revision != token || self.task.is_some() {
            return Err(handle);
        }
        self.task = Some(ScheduledTask { token, handle });
        Ok(())
    }

    pub(crate) fn request(
        &mut self,
        target: T,
        anchor_region: AnchorRegion,
        reveal: RevealPolicy,
        initial_height: f64,
        initial_presentation: Option<P>,
    ) -> RequestTransition<T> {
        let target_is_retained = self.target.as_ref() == Some(&target);
        if target_is_retained {
            self.cancel_task();
            self.anchor_region = anchor_region.sanitized();
            return RequestTransition::Retained {
                request: AnchoredWindowRequest {
                    generation: self.generation,
                    target: Some(target),
                    retarget_commit_required: self.awaiting_retarget_commit,
                },
                reposition: !self.awaiting_retarget_commit,
            };
        }
        let stored_target = target.clone();
        self.cancel_task();
        self.retarget(
            target,
            stored_target,
            anchor_region,
            reveal,
            initial_height,
            initial_presentation,
        )
    }

    fn retarget(
        &mut self,
        target: T,
        stored_target: T,
        anchor_region: AnchorRegion,
        reveal: RevealPolicy,
        initial_height: f64,
        initial_presentation: Option<P>,
    ) -> RequestTransition<T> {
        let was_visible = self.visible;
        let generation = self.next_generation();
        self.target = Some(stored_target);
        self.initial_presentation = initial_presentation;
        self.anchor_region = anchor_region.sanitized();
        self.measured_height = None;
        if !was_visible {
            self.height = initial_height;
        }
        if reveal == RevealPolicy::AfterPresentation {
            self.visible = false;
        }
        self.awaiting_presentation = true;
        self.awaiting_concealment = false;
        self.delivery_pending = true;
        self.placeholder_reveal_pending = reveal == RevealPolicy::ImmediatePlaceholder
            && !was_visible
            && self.initial_presentation.is_none();
        let reveal_now = reveal == RevealPolicy::ImmediatePlaceholder
            && self.renderer_ready
            && self.placeholder_reveal_pending;
        self.placeholder_reveal_pending &= !reveal_now;
        self.visible |= reveal_now;
        self.awaiting_retarget_commit =
            reveal == RevealPolicy::ImmediatePlaceholder && was_visible && self.visible;
        RequestTransition::Retargeted {
            request: AnchoredWindowRequest {
                generation,
                target: Some(target),
                retarget_commit_required: self.awaiting_retarget_commit,
            },
            reveal_now,
        }
    }

    pub(crate) fn renderer_ready(
        &mut self,
        renderer_generation: u64,
        reveal: RevealPolicy,
    ) -> Option<bool> {
        if renderer_generation != self.renderer_generation {
            return None;
        }
        self.renderer_ready = true;
        self.delivery_pending = true;
        let reveal_now = reveal == RevealPolicy::ImmediatePlaceholder
            && self.placeholder_reveal_pending
            && self.target.is_some()
            && !self.visible;
        self.placeholder_reveal_pending &= !reveal_now;
        self.visible |= reveal_now;
        Some(reveal_now)
    }

    pub(crate) fn renderer_destroyed(&mut self) {
        self.cancel_task();
        self.renderer_ready = false;
        self.delivery_pending = self.target.is_some();
        self.placeholder_reveal_pending = false;
        self.awaiting_retarget_commit = false;
        self.awaiting_presentation = self.target.is_some();
        self.visible = false;
    }

    pub(crate) fn conceal(&mut self) -> AnchoredWindowRequest<T> {
        self.cancel_task();
        self.transition_to_concealment()
    }

    fn transition_to_concealment(&mut self) -> AnchoredWindowRequest<T> {
        let generation = self.next_generation();
        self.target = None;
        self.initial_presentation = None;
        self.placeholder_reveal_pending = false;
        self.awaiting_retarget_commit = false;
        self.awaiting_presentation = false;
        self.awaiting_concealment = true;
        self.delivery_pending = true;
        AnchoredWindowRequest {
            generation,
            target: None,
            retarget_commit_required: false,
        }
    }

    pub(crate) fn conceal_scheduled(
        &mut self,
        scheduled_generation: u64,
        task_token: u64,
    ) -> Option<AnchoredWindowRequest<T>> {
        if self.generation != scheduled_generation || self.target.is_none() {
            return None;
        }
        if !self
            .task
            .as_ref()
            .is_some_and(|task| task.token == task_token)
        {
            return None;
        }
        self.task = None;
        Some(self.transition_to_concealment())
    }

    pub(crate) fn presented(&mut self, generation: u64) -> bool {
        if generation != self.generation || self.target.is_none() || !self.awaiting_presentation {
            return false;
        }
        self.awaiting_retarget_commit = false;
        self.awaiting_presentation = false;
        self.visible = true;
        true
    }

    pub(crate) fn record_height(&mut self, height: f64) -> bool {
        let Some(previous) = self.measured_height else {
            self.measured_height = Some(height);
            self.height = height;
            return true;
        };
        if (height - previous).abs() <= 1.0 {
            return false;
        }
        self.measured_height = Some(height);
        self.height = height;
        true
    }

    pub(crate) fn concealed(&mut self, generation: u64) -> bool {
        if !self.concealment_is_current(generation) {
            return false;
        }
        self.cancel_task();
        self.awaiting_concealment = false;
        self.visible = false;
        true
    }

    pub(crate) fn concealment_is_current(&self, generation: u64) -> bool {
        generation == self.generation && self.awaiting_concealment
    }

    pub(crate) fn force_hidden(&mut self) {
        self.awaiting_retarget_commit = false;
        self.placeholder_reveal_pending = false;
        self.visible = false;
        if self.target.is_some() {
            self.awaiting_presentation = true;
            self.delivery_pending = true;
        }
    }

    pub(crate) fn retarget_committed(&mut self, generation: u64) -> bool {
        if !self.retarget_commit_is_current(generation) {
            return false;
        }
        self.awaiting_retarget_commit = false;
        true
    }

    pub(crate) fn retarget_commit_is_current(&self, generation: u64) -> bool {
        generation == self.generation && self.target.is_some() && self.awaiting_retarget_commit
    }

    pub(crate) fn can_reposition(&self) -> bool {
        !self.awaiting_retarget_commit
    }

    pub(crate) fn fallback_concealed(&mut self, generation: u64, task_token: u64) -> bool {
        if !self.fallback_task_is_current(generation, task_token) {
            return false;
        }
        self.task = None;
        self.awaiting_concealment = false;
        self.visible = false;
        true
    }

    pub(crate) fn fallback_is_current(&self, generation: u64) -> bool {
        generation == self.generation && self.awaiting_concealment
    }

    pub(crate) fn fallback_task_is_current(&self, generation: u64, task_token: u64) -> bool {
        self.fallback_is_current(generation)
            && self
                .task
                .as_ref()
                .is_some_and(|task| task.token == task_token)
    }

    pub(crate) fn state(&self) -> AnchoredWindowState<T> {
        AnchoredWindowState {
            generation: self.generation,
            target: self.target.clone(),
            renderer_ready: self.renderer_ready,
            visible: self.visible,
            awaiting_retarget_commit: self.awaiting_retarget_commit,
            awaiting_presentation: self.awaiting_presentation,
            awaiting_concealment: self.awaiting_concealment,
        }
    }

    pub(crate) fn pending_render_request(&self) -> Option<AnchoredWindowRenderRequest<T, P>> {
        (self.renderer_ready && self.delivery_pending).then(|| self.render_request())
    }

    pub(crate) fn mark_delivered(&mut self, generation: u64) -> bool {
        if generation != self.generation || !self.delivery_pending {
            return false;
        }
        self.delivery_pending = false;
        true
    }

    pub(crate) fn render_request(&self) -> AnchoredWindowRenderRequest<T, P> {
        AnchoredWindowRenderRequest {
            generation: self.generation,
            target: self.target.clone(),
            retarget_commit_required: self.awaiting_retarget_commit,
            initial_presentation: self.initial_presentation.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
