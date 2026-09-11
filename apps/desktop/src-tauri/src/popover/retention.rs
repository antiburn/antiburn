//! The popover renderer retention policy.
//!
//! This module owns onboarding prewarm deadlines and eviction callback
//! identity. The popover facade owns all window operations and timer
//! scheduling.

use std::time::{Duration, Instant};

/// How long onboarding keeps a hidden renderer after it becomes ready.
pub(super) const PREWARM_READY_EVICTION_DELAY: Duration = Duration::from_secs(60);

/// How long the shell waits before it retries a failed destruction.
pub(super) const EVICTION_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EvictionMode {
    PrewarmReady,
    PrewarmLoading,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EvictionToken(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EvictionSchedule {
    token: EvictionToken,
    renderer_generation: u64,
    delay: Duration,
    mode: EvictionMode,
}

impl EvictionSchedule {
    pub(super) fn token(self) -> EvictionToken {
        self.token
    }

    pub(super) fn renderer_generation(self) -> u64 {
        self.renderer_generation
    }

    pub(super) fn delay(self) -> Duration {
        self.delay
    }

    pub(super) fn mode(self) -> EvictionMode {
        self.mode
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DueEviction {
    renderer_generation: u64,
    mode: EvictionMode,
}

impl DueEviction {
    pub(super) fn renderer_generation(self) -> u64 {
        self.renderer_generation
    }

    pub(super) fn mode(self) -> EvictionMode {
        self.mode
    }
}

#[derive(Debug, Default)]
struct PrewarmLease {
    generation: Option<u64>,
    loading_deadline: Option<Instant>,
    ready_deadline: Option<Instant>,
}

impl PrewarmLease {
    fn contains(&self, generation: u64) -> bool {
        self.generation == Some(generation)
    }

    fn mark_ready(&mut self, generation: u64, now: Instant) -> bool {
        if !self.contains(generation)
            || self
                .loading_deadline
                .is_some_and(|deadline| deadline <= now)
        {
            return false;
        }
        self.ready_deadline = Some(now + PREWARM_READY_EVICTION_DELAY);
        true
    }

    fn schedule(&self, generation: u64, now: Instant) -> Option<(Duration, EvictionMode)> {
        if !self.contains(generation) {
            return None;
        }
        if let Some(deadline) = self.ready_deadline {
            return Some((
                deadline.saturating_duration_since(now),
                EvictionMode::PrewarmReady,
            ));
        }
        self.loading_deadline.map(|deadline| {
            (
                deadline.saturating_duration_since(now),
                EvictionMode::PrewarmLoading,
            )
        })
    }

    fn expired(&self, now: Instant) -> Option<DueEviction> {
        let renderer_generation = self.generation?;
        if let Some(deadline) = self.ready_deadline {
            return (deadline <= now).then_some(DueEviction {
                renderer_generation,
                mode: EvictionMode::PrewarmReady,
            });
        }
        self.loading_deadline
            .filter(|deadline| *deadline <= now)
            .map(|_| DueEviction {
                renderer_generation,
                mode: EvictionMode::PrewarmLoading,
            })
    }

    fn take(&mut self) -> Option<u64> {
        let generation = self.generation;
        self.clear();
        generation
    }

    fn clear(&mut self) {
        self.generation = None;
        self.loading_deadline = None;
        self.ready_deadline = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ArmedEviction {
    token: EvictionToken,
    renderer_generation: u64,
    mode: EvictionMode,
}

#[derive(Debug, Default)]
pub(super) struct Retention {
    prewarm: PrewarmLease,
    next_token: u64,
    armed: Option<ArmedEviction>,
    task: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl Retention {
    pub(super) fn is_prewarm(&self, generation: u64) -> bool {
        self.prewarm.contains(generation)
    }

    pub(super) fn mark_prewarm_ready(&mut self, generation: u64, now: Instant) -> bool {
        self.prewarm.mark_ready(generation, now)
    }

    pub(super) fn expired_prewarm(&self, now: Instant) -> Option<DueEviction> {
        self.prewarm.expired(now)
    }

    pub(super) fn consume_prewarm_on_reveal(&mut self, generation: u64) -> bool {
        if !self.prewarm.contains(generation) {
            return false;
        }
        self.prewarm.clear();
        true
    }

    pub(super) fn take_prewarm(&mut self) -> Option<u64> {
        self.prewarm.take()
    }

    pub(super) fn clear_prewarm_generation(&mut self, generation: u64) -> bool {
        if !self.prewarm.contains(generation) {
            return false;
        }
        self.prewarm.clear();
        true
    }

    pub(super) fn arm_hidden(
        &mut self,
        renderer_generation: u64,
        now: Instant,
    ) -> Option<EvictionSchedule> {
        let Some((delay, mode)) = self.prewarm.schedule(renderer_generation, now) else {
            self.cancel_eviction();
            return None;
        };
        Some(self.arm(renderer_generation, delay, mode))
    }

    pub(super) fn arm_retry(
        &mut self,
        renderer_generation: u64,
        mode: EvictionMode,
    ) -> EvictionSchedule {
        self.arm(renderer_generation, EVICTION_RETRY_DELAY, mode)
    }

    pub(super) fn attach_task(
        &mut self,
        token: EvictionToken,
        task: tauri::async_runtime::JoinHandle<()>,
    ) {
        if self.armed.is_some_and(|armed| armed.token == token) {
            self.task = Some(task);
        } else {
            task.abort();
        }
    }

    pub(super) fn cancel_eviction(&mut self) {
        self.next_token = self.next_token.wrapping_add(1).max(1);
        self.armed = None;
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    pub(super) fn take_due(
        &mut self,
        token: EvictionToken,
        current_renderer_generation: u64,
        visible: bool,
    ) -> Option<DueEviction> {
        let armed = self.armed?;
        let due = armed.token == token
            && armed.renderer_generation == current_renderer_generation
            && !visible;
        if !due {
            return None;
        }
        self.armed = None;
        self.task = None;
        Some(DueEviction {
            renderer_generation: armed.renderer_generation,
            mode: armed.mode,
        })
    }

    fn arm(
        &mut self,
        renderer_generation: u64,
        delay: Duration,
        mode: EvictionMode,
    ) -> EvictionSchedule {
        self.cancel_eviction();
        let token = EvictionToken(self.next_token);
        self.armed = Some(ArmedEviction {
            token,
            renderer_generation,
            mode,
        });
        EvictionSchedule {
            token,
            renderer_generation,
            delay,
            mode,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_dismissal_keeps_the_renderer_for_the_process_lifetime() {
        let now = Instant::now();
        let mut retention = Retention::default();

        assert_eq!(retention.arm_hidden(4, now), None);
    }
}
