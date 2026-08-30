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

#[derive(Debug)]
struct Loading {
    generation: u64,
    started_at: Instant,
    reveal_pending: bool,
    rebuild_used: bool,
    build_after_destroy: bool,
}

#[derive(Debug, Default)]
enum Phase {
    #[default]
    Idle,
    Loading(Loading),
    Ready,
}

/// Tracks one window's renderer readiness and reveal intent.
#[derive(Debug, Default)]
pub struct WindowReadiness {
    next_generation: u64,
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
            Phase::Loading(_) | Phase::Ready => PrewarmAction::KeepExisting,
        }
    }

    /// Request a visible window without duplicating an active renderer load.
    pub fn request_open(&mut self, now: Instant) -> OpenAction {
        match &mut self.phase {
            Phase::Idle => {
                let generation = self.replace_loading(now, true, false);
                OpenAction::StartLoading { generation }
            }
            Phase::Ready => OpenAction::Reveal,
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

    /// Request a toggle without showing a renderer that still loads.
    pub fn toggle_open(&mut self, now: Instant) -> ToggleAction {
        match &mut self.phase {
            Phase::Idle => {
                let generation = self.replace_loading(now, true, false);
                ToggleAction::StartLoading { generation }
            }
            Phase::Ready => ToggleAction::UseWindowVisibility,
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
        if let Phase::Loading(load) = &mut self.phase
            && std::mem::take(&mut load.build_after_destroy)
        {
            load.started_at = now;
            return Some(load.generation);
        }
        self.reset();
        None
    }

    /// Mark the matching renderer ready and return its one reveal action.
    pub fn renderer_ready(&mut self, generation: u64, now: Instant) -> ReadyAction {
        let Phase::Loading(active_load) = &self.phase else {
            return ReadyAction::None;
        };
        if active_load.generation != generation {
            return ReadyAction::None;
        }

        let previous = std::mem::replace(&mut self.phase, Phase::Ready);
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

    /// Check whether a warning still belongs to the active stale load.
    pub fn warning_is_current(&self, generation: u64, now: Instant) -> bool {
        let Phase::Loading(load) = &self.phase else {
            return false;
        };
        load.generation == generation && load_is_stale(load, now)
    }

    /// Clear all active loading and readiness state.
    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
    }

    fn replace_loading(
        &mut self,
        started_at: Instant,
        reveal_pending: bool,
        rebuild_used: bool,
    ) -> u64 {
        self.next_generation = self.next_generation.wrapping_add(1);
        if self.next_generation == 0 {
            self.next_generation = 1;
        }
        let generation = self.next_generation;
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

    #[test]
    fn a_prewarmed_renderer_stays_hidden_when_it_becomes_ready() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();

        let PrewarmAction::StartLoading { generation } = readiness.request_prewarm(started_at)
        else {
            panic!("an idle lifecycle must start the prewarm load")
        };

        assert_eq!(
            readiness.renderer_ready(generation, started_at + Duration::from_secs(1)),
            ReadyAction::StayHidden {
                loading_for: Duration::from_secs(1)
            }
        );
        assert_eq!(
            readiness.request_prewarm(started_at + Duration::from_secs(2)),
            PrewarmAction::KeepExisting
        );
    }

    #[test]
    fn a_click_during_prewarm_reveals_the_same_renderer() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();

        let PrewarmAction::StartLoading { generation } = readiness.request_prewarm(started_at)
        else {
            panic!("an idle lifecycle must start the prewarm load")
        };

        assert_eq!(
            readiness.toggle_open(started_at + Duration::from_millis(250)),
            ToggleAction::AwaitReady
        );
        assert_eq!(
            readiness.renderer_ready(generation, started_at + Duration::from_secs(1)),
            ReadyAction::Reveal {
                loading_for: Duration::from_secs(1)
            }
        );
    }

    #[test]
    fn an_expired_prewarm_replacement_keeps_the_pending_click() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();
        let PrewarmAction::StartLoading {
            generation: _generation,
        } = readiness.request_prewarm(started_at)
        else {
            panic!("an idle lifecycle must start the prewarm load")
        };

        let pending_generation = match readiness.request_open(started_at + Duration::from_secs(64))
        {
            OpenAction::Rebuild { generation } => generation,
            _ => panic!("the stale prewarm starts its one replacement"),
        };
        let replaced_at = started_at + Duration::from_secs(65);
        let replacement = readiness
            .replace_expired_loading(pending_generation, replaced_at)
            .expect("the pending click starts a replacement");
        assert!(readiness.defer_build_until_destroyed(replacement));
        assert_eq!(
            readiness.begin_deferred_build(replaced_at),
            Some(replacement)
        );
        assert!(matches!(
            readiness.renderer_ready(replacement, replaced_at + Duration::from_secs(1)),
            ReadyAction::Reveal { .. }
        ));
    }

    #[test]
    fn prewarm_does_not_cancel_an_existing_reveal_request() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = start_loading(&mut readiness, started_at);

        assert_eq!(
            readiness.request_prewarm(started_at + Duration::from_millis(250)),
            PrewarmAction::KeepExisting
        );
        assert_eq!(
            readiness.renderer_ready(generation, started_at + Duration::from_secs(1)),
            ReadyAction::Reveal {
                loading_for: Duration::from_secs(1)
            }
        );
    }

    #[test]
    fn a_pending_load_reveals_once_and_reports_its_duration() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();

        let generation = start_loading(&mut readiness, started_at);

        assert_eq!(generation, 1);
        assert_eq!(
            readiness.loading_duration(started_at + Duration::from_secs(2)),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            readiness.renderer_ready(generation, started_at + Duration::from_secs(2)),
            ReadyAction::Reveal {
                loading_for: Duration::from_secs(2)
            }
        );
        assert_eq!(
            readiness.renderer_ready(generation, started_at + Duration::from_secs(3)),
            ReadyAction::None,
            "renderer readiness is idempotent"
        );
        assert_eq!(readiness.loading_duration(started_at), None);
    }

    #[test]
    fn repeated_open_requests_coalesce_while_the_renderer_loads() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();

        assert_eq!(
            readiness.request_open(started_at),
            OpenAction::StartLoading { generation: 1 }
        );
        assert_eq!(
            readiness.request_open(started_at + Duration::from_secs(1)),
            OpenAction::AwaitReady
        );
        assert_eq!(
            readiness.request_open(started_at + Duration::from_secs(2)),
            OpenAction::AwaitReady
        );
        assert_eq!(
            readiness.renderer_ready(1, started_at + Duration::from_secs(3)),
            ReadyAction::Reveal {
                loading_for: Duration::from_secs(3)
            }
        );
        assert_eq!(
            readiness.request_open(started_at + Duration::from_secs(4)),
            OpenAction::Reveal
        );
    }

    #[test]
    fn a_second_toggle_cancels_a_pending_reveal() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();

        assert_eq!(
            readiness.toggle_open(started_at),
            ToggleAction::StartLoading { generation: 1 }
        );
        assert_eq!(
            readiness.toggle_open(started_at + Duration::from_secs(1)),
            ToggleAction::CancelPendingReveal
        );
        assert_eq!(
            readiness.renderer_ready(1, started_at + Duration::from_secs(2)),
            ReadyAction::StayHidden {
                loading_for: Duration::from_secs(2)
            }
        );
    }

    #[test]
    fn a_toggle_can_restore_a_cancelled_pending_reveal() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();
        start_loading(&mut readiness, started_at);

        assert_eq!(
            readiness.toggle_open(started_at + Duration::from_secs(1)),
            ToggleAction::CancelPendingReveal
        );
        assert_eq!(
            readiness.toggle_open(started_at + Duration::from_secs(2)),
            ToggleAction::AwaitReady
        );
        assert_eq!(
            readiness.renderer_ready(1, started_at + Duration::from_secs(3)),
            ReadyAction::Reveal {
                loading_for: Duration::from_secs(3)
            }
        );
    }

    #[test]
    fn the_stale_boundary_is_five_seconds() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = start_loading(&mut readiness, started_at);

        assert!(!readiness.warning_is_current(
            generation,
            started_at + STALE_LOAD_AFTER - Duration::from_nanos(1)
        ));
        assert!(readiness.warning_is_current(generation, started_at + STALE_LOAD_AFTER));
    }

    #[test]
    fn only_a_later_open_can_rebuild_one_stale_load_cycle() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();
        start_loading(&mut readiness, started_at);

        let rebuilt_at = started_at + STALE_LOAD_AFTER;
        assert_eq!(
            readiness.request_open(rebuilt_at),
            OpenAction::Rebuild { generation: 2 }
        );
        assert_eq!(
            readiness.request_open(rebuilt_at + STALE_LOAD_AFTER),
            OpenAction::AwaitReady,
            "one open cycle can rebuild only once"
        );
        assert_eq!(
            readiness.renderer_ready(2, rebuilt_at + STALE_LOAD_AFTER),
            ReadyAction::Reveal {
                loading_for: STALE_LOAD_AFTER
            }
        );
    }

    #[test]
    fn a_stale_toggle_rebuilds_only_when_it_requests_a_reveal() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();
        start_loading(&mut readiness, started_at);
        assert_eq!(
            readiness.toggle_open(started_at),
            ToggleAction::CancelPendingReveal
        );

        assert_eq!(
            readiness.toggle_open(started_at + STALE_LOAD_AFTER),
            ToggleAction::Rebuild { generation: 2 }
        );
        assert_eq!(
            readiness.toggle_open(started_at + STALE_LOAD_AFTER + Duration::from_secs(1)),
            ToggleAction::CancelPendingReveal
        );
    }

    #[test]
    fn an_old_warning_task_cannot_report_a_replacement_load() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();
        let first_generation = start_loading(&mut readiness, started_at);
        let rebuilt_at = started_at + STALE_LOAD_AFTER;

        assert_eq!(
            readiness.request_open(rebuilt_at),
            OpenAction::Rebuild { generation: 2 }
        );
        assert!(!readiness.warning_is_current(first_generation, rebuilt_at + STALE_LOAD_AFTER));
        assert!(readiness.warning_is_current(2, rebuilt_at + STALE_LOAD_AFTER));
    }

    #[test]
    fn an_old_renderer_cannot_complete_a_replacement_load() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();
        let first_generation = start_loading(&mut readiness, started_at);
        let rebuilt_at = started_at + STALE_LOAD_AFTER;

        assert_eq!(
            readiness.request_open(rebuilt_at),
            OpenAction::Rebuild { generation: 2 }
        );
        assert_eq!(
            readiness.renderer_ready(first_generation, rebuilt_at),
            ReadyAction::None
        );
        assert_eq!(readiness.loading_generation(), Some(2));
        assert_eq!(
            readiness.renderer_ready(2, rebuilt_at + Duration::from_secs(1)),
            ReadyAction::Reveal {
                loading_for: Duration::from_secs(1)
            }
        );
    }

    #[test]
    fn a_replacement_build_waits_for_the_destroyed_event() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();
        start_loading(&mut readiness, started_at);
        let rebuilt_at = started_at + STALE_LOAD_AFTER;

        assert_eq!(
            readiness.request_open(rebuilt_at),
            OpenAction::Rebuild { generation: 2 }
        );
        assert!(readiness.defer_build_until_destroyed(2));
        assert_eq!(readiness.begin_deferred_build(rebuilt_at), Some(2));
        assert_eq!(readiness.loading_generation(), Some(2));
    }

    #[test]
    fn destroying_a_ready_renderer_starts_the_next_open_from_idle() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = start_loading(&mut readiness, started_at);
        assert_eq!(
            readiness.renderer_ready(generation, started_at + Duration::from_secs(1)),
            ReadyAction::Reveal {
                loading_for: Duration::from_secs(1)
            }
        );

        assert_eq!(
            readiness.begin_deferred_build(started_at + Duration::from_secs(2)),
            None
        );
        assert_eq!(
            readiness.request_open(started_at + Duration::from_secs(3)),
            OpenAction::StartLoading { generation: 2 }
        );
    }

    #[test]
    fn an_explicit_cancel_keeps_a_loading_renderer_hidden() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();
        let generation = start_loading(&mut readiness, started_at);

        assert!(readiness.cancel_pending_reveal());
        assert!(!readiness.cancel_pending_reveal());
        assert_eq!(
            readiness.renderer_ready(generation, started_at + Duration::from_secs(1)),
            ReadyAction::StayHidden {
                loading_for: Duration::from_secs(1)
            }
        );
    }

    #[test]
    fn reset_cancels_loading_and_starts_a_new_cycle() {
        let started_at = Instant::now();
        let mut readiness = WindowReadiness::default();
        let cancelled_generation = start_loading(&mut readiness, started_at);

        readiness.reset();

        assert_eq!(readiness.loading_generation(), None);
        assert_eq!(readiness.loading_duration(started_at), None);
        assert!(!readiness.warning_is_current(cancelled_generation, started_at + STALE_LOAD_AFTER));
        assert_eq!(
            readiness.request_open(started_at + STALE_LOAD_AFTER),
            OpenAction::StartLoading { generation: 2 }
        );
    }
}
