//! The popover renderer retention policy.
//!
//! This module owns onboarding prewarm deadlines and eviction callback
//! identity. The popover facade owns all window operations and timer
//! scheduling.

use std::time::{Duration, Instant};

/// How long onboarding keeps a hidden renderer after it becomes ready.
pub(super) const PREWARM_READY_EVICTION_DELAY: Duration = Duration::from_secs(60);

/// How long onboarding keeps a renderer that does not become ready.
pub(super) const PREWARM_LOADING_EVICTION_DELAY: Duration = Duration::from_secs(65);

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
    fn begin(&mut self, generation: u64, now: Instant) {
        self.generation = Some(generation);
        self.loading_deadline = Some(now + PREWARM_LOADING_EVICTION_DELAY);
        self.ready_deadline = None;
    }

    fn contains(&self, generation: u64) -> bool {
        self.generation == Some(generation)
    }

    fn transfer(&mut self, from: u64, to: u64) -> bool {
        if !self.contains(from) {
            return false;
        }
        self.generation = Some(to);
        true
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
    pub(super) fn begin_prewarm(&mut self, generation: u64, now: Instant) {
        self.prewarm.begin(generation, now);
    }

    pub(super) fn is_prewarm(&self, generation: u64) -> bool {
        self.prewarm.contains(generation)
    }

    pub(super) fn prewarm_generation(&self) -> Option<u64> {
        self.prewarm.generation
    }

    pub(super) fn transfer_prewarm(&mut self, from: u64, to: u64) -> bool {
        self.prewarm.transfer(from, to)
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

    #[test]
    fn onboarding_prewarm_changes_from_a_loading_to_a_ready_lease() {
        let started_at = Instant::now();
        let mut retention = Retention::default();
        retention.begin_prewarm(4, started_at);

        let loading = retention
            .arm_hidden(4, started_at)
            .expect("a loading prewarm must get a fail-safe schedule");
        assert_eq!(loading.delay(), Duration::from_secs(65));
        assert_eq!(loading.mode(), EvictionMode::PrewarmLoading);

        let ready_at = started_at + Duration::from_secs(2);
        assert!(retention.mark_prewarm_ready(4, ready_at));
        let ready = retention
            .arm_hidden(4, ready_at)
            .expect("a ready prewarm must keep its renderer");
        assert_eq!(ready.delay(), Duration::from_secs(60));
        assert_eq!(ready.mode(), EvictionMode::PrewarmReady);
    }

    #[test]
    fn a_generation_transfer_preserves_the_original_loading_deadline() {
        let started_at = Instant::now();
        let mut retention = Retention::default();
        retention.begin_prewarm(4, started_at);

        assert!(retention.transfer_prewarm(4, 5));
        assert!(!retention.is_prewarm(4));
        assert!(retention.is_prewarm(5));

        let schedule = retention
            .arm_hidden(5, started_at + Duration::from_secs(10))
            .expect("the replacement must inherit the lease");
        assert_eq!(schedule.delay(), Duration::from_secs(55));
        assert_eq!(schedule.mode(), EvictionMode::PrewarmLoading);
    }

    #[test]
    fn a_ready_generation_transfer_preserves_the_absolute_deadline() {
        let started_at = Instant::now();
        let ready_at = started_at + Duration::from_secs(2);
        let mut retention = Retention::default();
        retention.begin_prewarm(4, started_at);
        assert!(retention.mark_prewarm_ready(4, ready_at));

        assert!(retention.transfer_prewarm(4, 5));
        let first = retention
            .arm_hidden(5, ready_at + Duration::from_secs(10))
            .expect("the transferred ready lease remains active");
        assert_eq!(first.delay(), Duration::from_secs(50));

        let rearmed = retention
            .arm_hidden(5, ready_at + Duration::from_secs(20))
            .expect("rearming keeps the original ready deadline");
        assert_eq!(rearmed.delay(), Duration::from_secs(40));
        assert_eq!(rearmed.mode(), EvictionMode::PrewarmReady);
    }

    #[test]
    fn the_first_reveal_consumes_the_prewarm_lease_once() {
        let mut retention = Retention::default();
        let now = Instant::now();
        retention.begin_prewarm(4, now);

        assert!(retention.consume_prewarm_on_reveal(4));
        assert!(!retention.consume_prewarm_on_reveal(4));
        assert_eq!(retention.prewarm_generation(), None);
        assert_eq!(retention.arm_hidden(4, now), None);
    }

    #[test]
    fn an_expired_loading_lease_cannot_become_a_ready_lease() {
        let started_at = Instant::now();
        let mut retention = Retention::default();
        retention.begin_prewarm(4, started_at);
        let expired_at = started_at + PREWARM_LOADING_EVICTION_DELAY;

        assert_eq!(
            retention.expired_prewarm(expired_at),
            Some(DueEviction {
                renderer_generation: 4,
                mode: EvictionMode::PrewarmLoading,
            })
        );
        assert!(!retention.mark_prewarm_ready(4, expired_at));
        assert!(retention.is_prewarm(4));
    }

    #[test]
    fn a_replacement_schedule_invalidates_the_old_eviction_token() {
        let now = Instant::now();
        let mut retention = Retention::default();
        retention.begin_prewarm(4, now);
        let first = retention
            .arm_hidden(4, now)
            .expect("the first prewarm schedule must arm eviction");
        let replacement = retention
            .arm_hidden(4, now)
            .expect("the repeated prewarm schedule must replace eviction");

        assert_eq!(retention.take_due(first.token(), 4, false), None);
        assert_eq!(
            retention.take_due(replacement.token(), 4, false),
            Some(DueEviction {
                renderer_generation: 4,
                mode: EvictionMode::PrewarmLoading,
            })
        );
    }

    #[test]
    fn visibility_and_renderer_replacement_protect_a_prewarm_renderer() {
        let now = Instant::now();
        let mut visible = Retention::default();
        visible.begin_prewarm(4, now);
        let visible_schedule = visible
            .arm_hidden(4, now)
            .expect("the prewarm must arm eviction");
        assert_eq!(visible.take_due(visible_schedule.token(), 4, true), None);

        let mut replaced = Retention::default();
        replaced.begin_prewarm(4, now);
        let replaced_schedule = replaced
            .arm_hidden(4, now)
            .expect("the prewarm must arm eviction");
        assert_eq!(replaced.take_due(replaced_schedule.token(), 5, false), None);
    }

    #[test]
    fn a_failed_destruction_retries_without_releasing_the_prewarm_lease() {
        let now = Instant::now();
        let mut retention = Retention::default();
        retention.begin_prewarm(4, now);
        let first = retention
            .arm_hidden(4, now)
            .expect("the loading prewarm must arm eviction");

        let due = retention
            .take_due(first.token(), 4, false)
            .expect("the deadline callback must own the matching renderer");
        let retry = retention.arm_retry(due.renderer_generation(), due.mode());

        assert_eq!(retry.delay(), Duration::from_secs(1));
        assert_eq!(retry.mode(), EvictionMode::PrewarmLoading);
        assert!(retention.is_prewarm(4));
        assert_eq!(
            retention.take_due(retry.token(), 4, false),
            Some(DueEviction {
                renderer_generation: 4,
                mode: EvictionMode::PrewarmLoading,
            })
        );
    }

    #[test]
    fn onboarding_restart_takes_the_active_lease_and_invalidates_eviction() {
        let now = Instant::now();
        let mut retention = Retention::default();
        retention.begin_prewarm(4, now);
        let schedule = retention
            .arm_hidden(4, now)
            .expect("the loading prewarm must arm eviction");

        assert_eq!(retention.take_prewarm(), Some(4));
        retention.cancel_eviction();
        assert_eq!(retention.take_due(schedule.token(), 4, false), None);
    }
}
