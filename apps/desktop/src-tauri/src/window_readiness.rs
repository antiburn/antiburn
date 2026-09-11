//! Pure lifecycle state for windows that wait for their renderer.

use std::time::{Duration, Instant};

/// A renderer load becomes stale after this duration.
pub const STALE_LOAD_AFTER: Duration = Duration::from_secs(5);

/// Build the script that binds a renderer to its native load generation.
pub fn renderer_generation_script(generation: u64) -> String {
    format!(
        "Object.defineProperty(globalThis, \"__ANTIBURN_WINDOW_GENERATION__\", {{ value: {generation}, writable: false, configurable: false }});"
    )
}

/// The action for a request that must show a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAction {
    /// Build the first renderer for this window.
    StartLoading { generation: u64 },
    /// Keep waiting for the active renderer load.
    AwaitReady,
    /// Reveal the renderer that is already ready.
    Reveal,
    /// Replace the stale renderer once for this load cycle.
    Rebuild { generation: u64 },
}

/// The action for a retained main-window request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedOpenAction {
    StartLoading { generation: u64 },
    AwaitReady,
    Reveal,
    Rebuild { generation: u64 },
    Verify { generation: u64, request_id: u64 },
    AwaitVerification,
    AttendTerminal,
}

/// The action for a request that toggles a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleAction {
    /// Build the first renderer for this window.
    StartLoading { generation: u64 },
    /// Keep waiting and reveal the window when the renderer becomes ready.
    AwaitReady,
    /// Cancel the reveal that was waiting for renderer readiness.
    CancelPendingReveal,
    /// Use the current native visibility to show or hide the ready window.
    UseWindowVisibility,
    /// Replace the stale renderer once for this load cycle.
    Rebuild { generation: u64 },
}

/// The action for a request that warms a hidden window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrewarmAction {
    /// Build the first renderer without a pending reveal.
    StartLoading { generation: u64 },
    /// Keep the renderer or load that already exists.
    KeepExisting,
}

/// The action to take after the renderer reports readiness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadyAction {
    /// The renderer is ready and the pending request must reveal it.
    Reveal { loading_for: Duration },
    /// The renderer is ready but no request needs to reveal it.
    StayHidden { loading_for: Duration },
    /// This readiness report does not change the lifecycle.
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthAckAction {
    ArmReveal,
    RecoverEligible,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalRetry {
    RebuildAfterDestroy { generation: u64 },
    StartLoading { generation: u64 },
}

#[derive(Debug)]
struct Loading {
    generation: u64,
    started_at: Instant,
    reveal_pending: bool,
    rebuild_used: bool,
    build_after_destroy: bool,
}

#[derive(Debug)]
struct Verification {
    request_id: u64,
}

#[derive(Debug)]
struct Ready {
    generation: u64,
    verification: Option<Verification>,
    armed_reveal: Option<u64>,
}

#[derive(Debug)]
struct Terminal {
    generation: u64,
    window_exists: bool,
}

#[derive(Debug, Default)]
enum Phase {
    #[default]
    Idle,
    Loading(Loading),
    Ready(Ready),
    Terminal(Terminal),
}

/// Tracks one window's renderer readiness and reveal intent.
#[derive(Debug, Default)]
pub struct WindowReadiness {
    next_generation: u64,
    next_request_id: u64,
    phase: Phase,
}

impl WindowReadiness {
    /// Request a hidden renderer without changing an existing reveal request.
    pub fn request_prewarm(&mut self, now: Instant) -> PrewarmAction {
        match &self.phase {
            Phase::Idle => {
                let generation = self.replace_loading(now, false, false);
                PrewarmAction::StartLoading { generation }
            }
            Phase::Loading(_) | Phase::Ready(_) | Phase::Terminal(_) => PrewarmAction::KeepExisting,
        }
    }

    /// Request a visible window without duplicating an active renderer load.
    pub fn request_open(&mut self, now: Instant) -> OpenAction {
        match &mut self.phase {
            Phase::Idle => {
                let generation = self.replace_loading(now, true, false);
                OpenAction::StartLoading { generation }
            }
            Phase::Ready(_) => OpenAction::Reveal,
            Phase::Terminal(_) => {
                debug_assert!(false, "terminal windows require informed recovery");
                OpenAction::AwaitReady
            }
            Phase::Loading(load) => {
                load.reveal_pending = true;
                if load_is_stale(load, now) && !load.rebuild_used {
                    let generation = self.replace_loading(now, true, true);
                    OpenAction::Rebuild { generation }
                } else {
                    OpenAction::AwaitReady
                }
            }
        }
    }

    /// Request a retained window and verify a hidden ready renderer.
    pub fn request_open_retained(&mut self, now: Instant, presented: bool) -> RetainedOpenAction {
        match &mut self.phase {
            Phase::Idle => {
                let generation = self.replace_loading(now, true, false);
                RetainedOpenAction::StartLoading { generation }
            }
            Phase::Loading(load) => {
                load.reveal_pending = true;
                if load_is_stale(load, now) && !load.rebuild_used {
                    let generation = self.replace_loading(now, true, true);
                    RetainedOpenAction::Rebuild { generation }
                } else {
                    RetainedOpenAction::AwaitReady
                }
            }
            Phase::Ready(ready) if presented => {
                ready.verification = None;
                ready.armed_reveal = None;
                RetainedOpenAction::Reveal
            }
            Phase::Ready(ready) if ready.verification.is_some() => {
                RetainedOpenAction::AwaitVerification
            }
            Phase::Ready(ready) => {
                let request_id = next_nonzero(&mut self.next_request_id);
                ready.verification = Some(Verification { request_id });
                RetainedOpenAction::Verify {
                    generation: ready.generation,
                    request_id,
                }
            }
            Phase::Terminal(_) => RetainedOpenAction::AttendTerminal,
        }
    }

    /// Request a toggle without showing a renderer that still loads.
    pub fn toggle_open(&mut self, now: Instant) -> ToggleAction {
        match &mut self.phase {
            Phase::Idle => {
                let generation = self.replace_loading(now, true, false);
                ToggleAction::StartLoading { generation }
            }
            Phase::Ready(_) => ToggleAction::UseWindowVisibility,
            Phase::Terminal(_) => ToggleAction::AwaitReady,
            Phase::Loading(load) if load.reveal_pending => {
                load.reveal_pending = false;
                ToggleAction::CancelPendingReveal
            }
            Phase::Loading(load) => {
                load.reveal_pending = true;
                if load_is_stale(load, now) && !load.rebuild_used {
                    let generation = self.replace_loading(now, true, true);
                    ToggleAction::Rebuild { generation }
                } else {
                    ToggleAction::AwaitReady
                }
            }
        }
    }

    /// Cancel a reveal request while the active renderer still loads.
    pub fn cancel_pending_reveal(&mut self) -> bool {
        let Phase::Loading(load) = &mut self.phase else {
            return false;
        };
        std::mem::take(&mut load.reveal_pending)
    }

    /// Replace an expired load and keep its pending reveal request.
    pub fn replace_expired_loading(&mut self, generation: u64, now: Instant) -> Option<u64> {
        let Phase::Loading(load) = &self.phase else {
            return None;
        };
        if load.generation != generation || !load.reveal_pending {
            return None;
        }
        Some(self.replace_loading(now, true, true))
    }

    /// Replace a hung load without using reveal intent as a gate.
    pub fn replace_hung_loading(&mut self, generation: u64, now: Instant) -> Option<u64> {
        let Phase::Loading(load) = &self.phase else {
            return None;
        };
        if load.generation != generation {
            return None;
        }
        let reveal_pending = load.reveal_pending;
        Some(self.replace_loading(now, reveal_pending, true))
    }

    /// Defer the matching renderer build until Tauri removes the old window.
    pub fn defer_build_until_destroyed(&mut self, generation: u64) -> bool {
        let Phase::Loading(load) = &mut self.phase else {
            return false;
        };
        if load.generation != generation {
            return false;
        }
        load.build_after_destroy = true;
        true
    }

    /// Reset after normal destruction or start one deferred replacement.
    pub fn begin_deferred_build(&mut self, now: Instant) -> Option<u64> {
        match &mut self.phase {
            Phase::Loading(load) if load.build_after_destroy => {
                load.build_after_destroy = false;
                load.started_at = now;
                Some(load.generation)
            }
            Phase::Loading(_) | Phase::Ready(_) => {
                self.reset();
                None
            }
            Phase::Terminal(terminal) if terminal.window_exists => {
                terminal.window_exists = false;
                None
            }
            Phase::Terminal(_) | Phase::Idle => None,
        }
    }

    /// Mark the matching renderer ready and return its one reveal action.
    pub fn renderer_ready(&mut self, generation: u64, now: Instant) -> ReadyAction {
        let Phase::Loading(active_load) = &self.phase else {
            return ReadyAction::None;
        };
        if active_load.generation != generation {
            return ReadyAction::None;
        }

        let previous = std::mem::replace(
            &mut self.phase,
            Phase::Ready(Ready {
                generation,
                verification: None,
                armed_reveal: None,
            }),
        );
        let Phase::Loading(load) = previous else {
            unreachable!("the active renderer load was checked before replacement")
        };

        let loading_for = now.saturating_duration_since(load.started_at);
        if load.reveal_pending {
            ReadyAction::Reveal { loading_for }
        } else {
            ReadyAction::StayHidden { loading_for }
        }
    }

    /// Accept one response for the active hidden-window verification.
    pub fn health_ack(
        &mut self,
        request_id: u64,
        generation: u64,
        healthy: bool,
    ) -> HealthAckAction {
        let Phase::Ready(ready) = &mut self.phase else {
            return HealthAckAction::None;
        };
        if ready.generation != generation
            || !ready
                .verification
                .as_ref()
                .is_some_and(|verification| verification.request_id == request_id)
        {
            return HealthAckAction::None;
        }
        ready.verification = None;
        if healthy {
            ready.armed_reveal = Some(request_id);
            HealthAckAction::ArmReveal
        } else {
            HealthAckAction::RecoverEligible
        }
    }

    /// Consume the reveal that a matching healthy response armed.
    pub fn take_armed_reveal(&mut self, generation: u64) -> bool {
        let Phase::Ready(ready) = &mut self.phase else {
            return false;
        };
        ready.generation == generation && ready.armed_reveal.take().is_some()
    }

    /// Expire the exact active verification request.
    pub fn verification_timeout(&mut self, request_id: u64) -> bool {
        let Phase::Ready(ready) = &mut self.phase else {
            return false;
        };
        if !ready
            .verification
            .as_ref()
            .is_some_and(|verification| verification.request_id == request_id)
        {
            return false;
        }
        ready.verification = None;
        true
    }

    /// Cancel verification and any reveal that its response armed.
    pub fn cancel_pending_verification(&mut self) -> bool {
        let Phase::Ready(ready) = &mut self.phase else {
            return false;
        };
        let verification = ready.verification.take().is_some();
        let armed_reveal = ready.armed_reveal.take().is_some();
        verification || armed_reveal
    }

    /// Return the active health-check request.
    pub fn pending_health_check(&self) -> Option<(u64, u64)> {
        let Phase::Ready(ready) = &self.phase else {
            return None;
        };
        ready
            .verification
            .as_ref()
            .map(|verification| (verification.request_id, ready.generation))
    }

    /// Start recovery from the matching ready generation.
    pub fn begin_recovery(&mut self, expected_generation: u64, now: Instant) -> Option<u64> {
        let Phase::Ready(ready) = &self.phase else {
            return None;
        };
        if ready.generation != expected_generation {
            return None;
        }
        Some(self.replace_loading(now, true, true))
    }

    /// Keep ownership after native destruction fails.
    pub fn destroy_failed_retry(&mut self, generation: u64, now: Instant) -> Option<u64> {
        let Phase::Loading(load) = &mut self.phase else {
            return None;
        };
        if load.generation != generation || !load.build_after_destroy {
            return None;
        }
        load.started_at = now;
        Some(generation)
    }

    /// Start a fresh load after a build fails with a free label.
    pub fn build_failed_retry(&mut self, generation: u64, now: Instant) -> Option<u64> {
        let Phase::Loading(load) = &self.phase else {
            return None;
        };
        if load.generation != generation || load.build_after_destroy {
            return None;
        }
        let reveal_pending = load.reveal_pending;
        Some(self.replace_loading(now, reveal_pending, true))
    }

    /// Enter terminal recovery from a ready renderer.
    pub fn enter_terminal(&mut self) -> bool {
        let Phase::Ready(ready) = &self.phase else {
            return false;
        };
        let generation = ready.generation;
        self.phase = Phase::Terminal(Terminal {
            generation,
            window_exists: true,
        });
        true
    }

    /// Enter terminal recovery from a matching loading generation.
    pub fn enter_terminal_from_loading(
        &mut self,
        generation: u64,
        window_exists: bool,
    ) -> Option<bool> {
        let Phase::Loading(load) = &self.phase else {
            return None;
        };
        if load.generation != generation {
            return None;
        }
        let reveal_pending = load.reveal_pending;
        self.phase = Phase::Terminal(Terminal {
            generation,
            window_exists,
        });
        Some(reveal_pending)
    }

    /// Start one informed retry from terminal recovery.
    pub fn retry_from_terminal(&mut self, now: Instant) -> Option<TerminalRetry> {
        let Phase::Terminal(terminal) = &self.phase else {
            return None;
        };
        let previous_generation = terminal.generation;
        let window_exists = terminal.window_exists;
        let generation = self.replace_loading(now, true, true);
        debug_assert_ne!(generation, previous_generation);
        if window_exists {
            Some(TerminalRetry::RebuildAfterDestroy { generation })
        } else {
            Some(TerminalRetry::StartLoading { generation })
        }
    }

    /// Return the active load duration.
    pub fn loading_duration(&self, now: Instant) -> Option<Duration> {
        let Phase::Loading(load) = &self.phase else {
            return None;
        };
        Some(now.saturating_duration_since(load.started_at))
    }

    /// Return the active load generation.
    pub fn loading_generation(&self) -> Option<u64> {
        let Phase::Loading(load) = &self.phase else {
            return None;
        };
        Some(load.generation)
    }

    /// Return the active ready generation.
    pub fn ready_generation(&self) -> Option<u64> {
        let Phase::Ready(ready) = &self.phase else {
            return None;
        };
        Some(ready.generation)
    }

    /// Return the generation that owns the active renderer or load.
    pub fn active_generation(&self) -> Option<u64> {
        match &self.phase {
            Phase::Loading(load) => Some(load.generation),
            Phase::Ready(ready) => Some(ready.generation),
            Phase::Idle | Phase::Terminal(_) => None,
        }
    }

    /// Return whether only informed recovery can start another load.
    pub fn is_terminal(&self) -> bool {
        matches!(self.phase, Phase::Terminal(_))
    }

    /// Check whether a warning still belongs to the active stale load.
    pub fn warning_is_current(&self, generation: u64, now: Instant) -> bool {
        let Phase::Loading(load) = &self.phase else {
            return false;
        };
        load.generation == generation && load_is_stale(load, now)
    }

    /// Clear lifecycle state only after the native label is free.
    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
    }

    fn replace_loading(
        &mut self,
        started_at: Instant,
        reveal_pending: bool,
        rebuild_used: bool,
    ) -> u64 {
        let generation = next_nonzero(&mut self.next_generation);
        self.phase = Phase::Loading(Loading {
            generation,
            started_at,
            reveal_pending,
            rebuild_used,
            build_after_destroy: false,
        });
        generation
    }
}

fn next_nonzero(value: &mut u64) -> u64 {
    *value = value.wrapping_add(1);
    if *value == 0 {
        *value = 1;
    }
    *value
}

fn load_is_stale(load: &Loading, now: Instant) -> bool {
    now.saturating_duration_since(load.started_at) >= STALE_LOAD_AFTER
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn start_loading(readiness: &mut WindowReadiness, started_at: Instant) -> u64 {
        match readiness.request_open(started_at) {
            OpenAction::StartLoading { generation } => generation,
            _ => panic!("an idle lifecycle must start loading"),
        }
    }

    fn ready_hidden(readiness: &mut WindowReadiness, now: Instant) -> u64 {
        let PrewarmAction::StartLoading { generation } = readiness.request_prewarm(now) else {
            panic!("an idle lifecycle must start prewarming")
        };
        assert!(matches!(
            readiness.renderer_ready(generation, now),
            ReadyAction::StayHidden { .. }
        ));
        generation
    }

    #[test]
    fn existing_open_and_prewarm_behavior_remains_available() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = start_loading(&mut readiness, started_at);
        assert_eq!(readiness.request_open(started_at), OpenAction::AwaitReady);
        assert_eq!(
            readiness.renderer_ready(generation, started_at + Duration::from_secs(1)),
            ReadyAction::Reveal {
                loading_for: Duration::from_secs(1)
            }
        );
        assert_eq!(readiness.request_open(started_at), OpenAction::Reveal);
        assert_eq!(
            readiness.request_prewarm(started_at),
            PrewarmAction::KeepExisting
        );
    }

    #[test]
    fn retained_hidden_opens_coalesce_one_verification() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = ready_hidden(&mut readiness, now);
        let RetainedOpenAction::Verify {
            generation: checked_generation,
            request_id,
        } = readiness.request_open_retained(now, false)
        else {
            panic!("a hidden retained renderer must verify")
        };
        assert_eq!(checked_generation, generation);
        assert_eq!(
            readiness.request_open_retained(now, false),
            RetainedOpenAction::AwaitVerification
        );
        assert_eq!(
            readiness.pending_health_check(),
            Some((request_id, generation))
        );
    }

    #[test]
    fn health_ack_and_timeout_are_generation_and_request_scoped() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = ready_hidden(&mut readiness, now);
        let RetainedOpenAction::Verify { request_id, .. } =
            readiness.request_open_retained(now, false)
        else {
            panic!("a hidden retained renderer must verify")
        };
        assert_eq!(
            readiness.health_ack(request_id + 1, generation, true),
            HealthAckAction::None
        );
        assert_eq!(
            readiness.health_ack(request_id, generation + 1, true),
            HealthAckAction::None
        );
        assert_eq!(
            readiness.health_ack(request_id, generation, true),
            HealthAckAction::ArmReveal
        );
        assert!(!readiness.verification_timeout(request_id));
        assert!(readiness.take_armed_reveal(generation));
        assert!(!readiness.take_armed_reveal(generation));
    }

    #[test]
    fn close_cancels_verification_and_an_armed_reveal() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = ready_hidden(&mut readiness, now);
        let RetainedOpenAction::Verify { request_id, .. } =
            readiness.request_open_retained(now, false)
        else {
            panic!("a hidden retained renderer must verify")
        };
        assert_eq!(
            readiness.health_ack(request_id, generation, true),
            HealthAckAction::ArmReveal
        );
        assert!(readiness.cancel_pending_verification());
        assert!(!readiness.take_armed_reveal(generation));
    }

    #[test]
    fn fallback_ack_is_recovery_eligible_without_changing_phase() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = ready_hidden(&mut readiness, now);
        let RetainedOpenAction::Verify { request_id, .. } =
            readiness.request_open_retained(now, false)
        else {
            panic!("a hidden retained renderer must verify")
        };
        assert_eq!(
            readiness.health_ack(request_id, generation, false),
            HealthAckAction::RecoverEligible
        );
        assert_eq!(readiness.ready_generation(), Some(generation));
    }

    #[test]
    fn destroy_failure_keeps_pending_label_ownership() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        start_loading(&mut readiness, now);
        let RetainedOpenAction::Rebuild { generation } =
            readiness.request_open_retained(now + STALE_LOAD_AFTER, false)
        else {
            panic!("a stale load must rebuild")
        };
        assert!(readiness.defer_build_until_destroyed(generation));
        assert_eq!(
            readiness.destroy_failed_retry(generation, now),
            Some(generation)
        );
        assert_eq!(readiness.request_open(now), OpenAction::AwaitReady);
        assert_eq!(
            readiness.request_open_retained(now, false),
            RetainedOpenAction::AwaitReady
        );
    }

    #[test]
    fn build_failure_uses_a_fresh_generation_only_when_the_label_is_free() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = start_loading(&mut readiness, now);
        let replacement = readiness
            .build_failed_retry(generation, now)
            .expect("a direct build failure has a free label");
        assert_ne!(replacement, generation);
        assert!(readiness.defer_build_until_destroyed(replacement));
        assert_eq!(readiness.build_failed_retry(replacement, now), None);
        assert_eq!(readiness.renderer_ready(generation, now), ReadyAction::None);
    }

    #[test]
    fn deferred_build_failure_starts_a_fresh_watched_generation() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        start_loading(&mut readiness, now);
        let OpenAction::Rebuild { generation } = readiness.request_open(now + STALE_LOAD_AFTER)
        else {
            panic!("a stale load must rebuild")
        };
        assert!(readiness.defer_build_until_destroyed(generation));
        assert_eq!(readiness.begin_deferred_build(now), Some(generation));
        let fresh = readiness
            .build_failed_retry(generation, now)
            .expect("a deferred build failure has a free label");
        assert_ne!(fresh, generation);
        assert!(readiness.cancel_pending_reveal());
        assert_eq!(readiness.renderer_ready(generation, now), ReadyAction::None);
    }

    #[test]
    fn hung_informed_retry_remains_replaceable_and_terminalizable() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = start_loading(&mut readiness, now);
        readiness.enter_terminal_from_loading(generation, true);
        let Some(TerminalRetry::RebuildAfterDestroy { generation }) =
            readiness.retry_from_terminal(now)
        else {
            panic!("a live terminal retry must destroy first")
        };
        assert!(readiness.defer_build_until_destroyed(generation));
        assert_eq!(readiness.begin_deferred_build(now), Some(generation));
        let replacement = readiness
            .replace_hung_loading(generation, now)
            .expect("a hung informed retry remains replaceable");
        assert_eq!(
            readiness.enter_terminal_from_loading(replacement, true),
            Some(true)
        );
        assert!(readiness.is_terminal());
    }

    #[test]
    fn hung_replacement_ignores_and_preserves_reveal_intent() {
        let now = Instant::now();
        for reveal_pending in [false, true] {
            let mut readiness = WindowReadiness::default();
            let generation = start_loading(&mut readiness, now);
            if !reveal_pending {
                assert!(readiness.cancel_pending_reveal());
            }
            assert!(readiness.defer_build_until_destroyed(generation));
            let replacement = readiness
                .replace_hung_loading(generation, now)
                .expect("a live hung generation must be replaced");
            assert_ne!(replacement, generation);
            assert_eq!(readiness.cancel_pending_reveal(), reveal_pending);
            assert_eq!(readiness.renderer_ready(generation, now), ReadyAction::None);
        }
    }

    #[test]
    fn close_watchdog_reopen_sequence_stays_live() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = ready_hidden(&mut readiness, now);
        let replacement = readiness
            .begin_recovery(generation, now)
            .expect("the ready generation starts recovery");
        assert!(readiness.defer_build_until_destroyed(replacement));
        assert!(readiness.cancel_pending_reveal());
        let watched = readiness
            .replace_hung_loading(replacement, now)
            .expect("close does not block watchdog replacement");
        assert!(!readiness.cancel_pending_reveal());
        assert_eq!(
            readiness.request_open_retained(now, false),
            RetainedOpenAction::AwaitReady
        );
        assert_eq!(
            readiness.renderer_ready(watched, now),
            ReadyAction::Reveal {
                loading_for: Duration::ZERO
            }
        );
    }

    #[test]
    fn hidden_recovery_can_settle_then_verify_later() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = ready_hidden(&mut readiness, now);
        let replacement = readiness.begin_recovery(generation, now).unwrap();
        readiness.cancel_pending_reveal();
        let replacement = readiness.replace_hung_loading(replacement, now).unwrap();
        assert!(matches!(
            readiness.renderer_ready(replacement, now),
            ReadyAction::StayHidden { .. }
        ));
        assert!(matches!(
            readiness.request_open_retained(now, false),
            RetainedOpenAction::Verify {
                generation: value,
                ..
            } if value == replacement
        ));
    }

    #[test]
    fn terminal_destruction_preserves_the_informed_retry_gate() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = start_loading(&mut readiness, now);
        assert_eq!(
            readiness.enter_terminal_from_loading(generation, true),
            Some(true)
        );
        assert!(readiness.is_terminal());
        assert_eq!(readiness.begin_deferred_build(now), None);
        assert_eq!(
            readiness.request_open_retained(now, false),
            RetainedOpenAction::AttendTerminal
        );
        assert!(matches!(
            readiness.retry_from_terminal(now),
            Some(TerminalRetry::StartLoading { generation: value }) if value > generation
        ));
    }

    #[test]
    fn duplicate_terminal_and_idle_destruction_are_no_ops() {
        let now = Instant::now();
        let mut idle = WindowReadiness::default();
        assert_eq!(idle.begin_deferred_build(now), None);

        let mut terminal = WindowReadiness::default();
        let generation = start_loading(&mut terminal, now);
        terminal.enter_terminal_from_loading(generation, false);
        assert_eq!(terminal.begin_deferred_build(now), None);
        assert!(terminal.is_terminal());
    }

    #[test]
    fn terminal_retry_with_a_live_label_requires_destroy_first() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = start_loading(&mut readiness, now);
        readiness.enter_terminal_from_loading(generation, true);
        let Some(TerminalRetry::RebuildAfterDestroy { generation }) =
            readiness.retry_from_terminal(now)
        else {
            panic!("a live terminal window must destroy before retry")
        };
        assert!(readiness.defer_build_until_destroyed(generation));
        assert_eq!(readiness.request_open(now), OpenAction::AwaitReady);
    }

    #[test]
    fn deferred_build_keeps_existing_non_terminal_behavior() {
        let now = Instant::now();
        let mut loading = WindowReadiness::default();
        let generation = start_loading(&mut loading, now);
        assert!(loading.defer_build_until_destroyed(generation));
        assert_eq!(loading.begin_deferred_build(now), Some(generation));

        let mut ready = WindowReadiness::default();
        let generation = start_loading(&mut ready, now);
        ready.renderer_ready(generation, now);
        assert_eq!(ready.begin_deferred_build(now), None);
        assert!(matches!(
            ready.request_open(now),
            OpenAction::StartLoading { .. }
        ));
    }

    #[test]
    fn expired_replacement_still_requires_a_pending_reveal() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = start_loading(&mut readiness, now);
        readiness.cancel_pending_reveal();
        assert_eq!(readiness.replace_expired_loading(generation, now), None);
    }

    #[test]
    fn stale_warning_and_reset_behavior_remain_scoped() {
        let now = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = start_loading(&mut readiness, now);
        assert!(
            !readiness
                .warning_is_current(generation, now + STALE_LOAD_AFTER - Duration::from_nanos(1))
        );
        assert!(readiness.warning_is_current(generation, now + STALE_LOAD_AFTER));
        readiness.reset();
        assert_eq!(readiness.loading_generation(), None);
        assert!(!readiness.warning_is_current(generation, now + STALE_LOAD_AFTER));
    }
}
