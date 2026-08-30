//! Content-free latency milestones for menu-bar popover opens.

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default)]
struct RendererMilestones {
    generation: u64,
    renderer_ready_at: Option<Instant>,
    content_ready_at: Option<Instant>,
}

#[derive(Debug)]
struct OpenTiming {
    generation: u64,
    requested_at: Instant,
    prewarmed: bool,
    renderer_state: &'static str,
    renderer_ready_at: Option<Instant>,
    revealed_at: Option<Instant>,
    content_ready_at: Option<Instant>,
    content_reported: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RevealTiming {
    request_to_reveal: Duration,
    ready_to_reveal: Duration,
    renderer_ready_before_request: bool,
    content_before_reveal: bool,
}

impl OpenTiming {
    fn new(
        generation: u64,
        requested_at: Instant,
        prewarmed: bool,
        renderer_state: &'static str,
        milestones: RendererMilestones,
    ) -> Self {
        let milestones = (milestones.generation == generation).then_some(milestones);
        Self {
            generation,
            requested_at,
            prewarmed,
            renderer_state,
            renderer_ready_at: milestones.and_then(|value| value.renderer_ready_at),
            revealed_at: None,
            content_ready_at: milestones.and_then(|value| value.content_ready_at),
            content_reported: false,
        }
    }

    fn mark_renderer_ready(&mut self, generation: u64, now: Instant) -> Option<Duration> {
        if self.generation != generation || self.renderer_ready_at.is_some() {
            return None;
        }
        self.renderer_ready_at = Some(now);
        Some(now.saturating_duration_since(self.requested_at))
    }

    fn mark_revealed(&mut self, generation: u64, now: Instant) -> Option<RevealTiming> {
        if self.generation != generation || self.revealed_at.is_some() {
            return None;
        }
        let renderer_ready_before_request = self
            .renderer_ready_at
            .is_some_and(|ready_at| ready_at <= self.requested_at);
        let ready_boundary = self.renderer_ready_at.unwrap_or(self.requested_at);
        self.revealed_at = Some(now);
        let content_before_reveal = self
            .content_ready_at
            .is_some_and(|ready_at| ready_at <= now);
        Some(RevealTiming {
            request_to_reveal: now.saturating_duration_since(self.requested_at),
            ready_to_reveal: now.saturating_duration_since(ready_boundary),
            renderer_ready_before_request,
            content_before_reveal,
        })
    }

    fn mark_content_ready(
        &mut self,
        generation: u64,
        now: Instant,
    ) -> Option<(Duration, Duration)> {
        if self.generation != generation || self.content_reported {
            return None;
        }
        self.content_ready_at = Some(now);
        let revealed_at = self.revealed_at?;
        self.content_reported = true;
        Some((
            now.saturating_duration_since(self.requested_at),
            now.saturating_duration_since(revealed_at),
        ))
    }

    fn take_content_ready_at_reveal(&mut self) -> bool {
        if self.content_reported || self.content_ready_at.is_none() {
            return false;
        }
        self.content_reported = true;
        true
    }
}

/// Owns renderer milestones and the active menu-bar open measurement.
#[derive(Debug, Default)]
pub(super) struct PopoverTiming {
    milestones: Mutex<RendererMilestones>,
    open: Mutex<Option<OpenTiming>>,
}

impl PopoverTiming {
    pub(super) fn reset_renderer(&self, generation: u64) {
        if let Ok(mut milestones) = self.milestones.lock() {
            *milestones = RendererMilestones {
                generation,
                ..RendererMilestones::default()
            };
        }
    }

    pub(super) fn begin_open(
        &self,
        generation: u64,
        requested_at: Instant,
        prewarmed: bool,
        renderer_state: &'static str,
    ) {
        let milestones = self
            .milestones
            .lock()
            .map(|milestones| *milestones)
            .unwrap_or_default();
        if let Ok(mut open) = self.open.lock() {
            *open = Some(OpenTiming::new(
                generation,
                requested_at,
                prewarmed,
                renderer_state,
                milestones,
            ));
        }
        ::tracing::info!(
            event = "popover_open_timing",
            phase = "requested",
            generation,
            prewarmed,
            renderer_state,
            elapsed_ms = 0_u64
        );
    }

    pub(super) fn cancel_open(&self) {
        if let Ok(mut open) = self.open.lock() {
            *open = None;
        }
    }

    pub(super) fn replace_open_generation(&self, from: u64, to: u64) -> bool {
        let Ok(mut open) = self.open.lock() else {
            return false;
        };
        let Some(current) = open.as_ref().filter(|current| current.generation == from) else {
            return false;
        };
        let requested_at = current.requested_at;
        *open = Some(OpenTiming::new(
            to,
            requested_at,
            false,
            "expired_replacement",
            RendererMilestones::default(),
        ));
        ::tracing::info!(
            event = "popover_open_timing",
            phase = "generation_replaced",
            generation = to,
            previous_generation = from,
            prewarmed = false,
            renderer_state = "expired_replacement",
            elapsed_ms = Instant::now()
                .saturating_duration_since(requested_at)
                .as_millis() as u64
        );
        true
    }

    pub(super) fn build_started(&self, generation: u64, now: Instant) {
        let Ok(open) = self.open.lock() else {
            return;
        };
        let Some(open) = open.as_ref().filter(|open| open.generation == generation) else {
            return;
        };
        ::tracing::info!(
            event = "popover_open_timing",
            phase = "build_started",
            generation,
            prewarmed = open.prewarmed,
            renderer_state = open.renderer_state,
            elapsed_ms = now.saturating_duration_since(open.requested_at).as_millis() as u64
        );
    }

    pub(super) fn renderer_ready(&self, generation: u64, now: Instant) {
        if let Ok(mut milestones) = self.milestones.lock()
            && milestones.generation == generation
        {
            milestones.renderer_ready_at = Some(now);
        }
        let Ok(mut open) = self.open.lock() else {
            return;
        };
        let Some(open) = open.as_mut() else {
            return;
        };
        let prewarmed = open.prewarmed;
        let renderer_state = open.renderer_state;
        let Some(elapsed) = open.mark_renderer_ready(generation, now) else {
            return;
        };
        ::tracing::info!(
            event = "popover_open_timing",
            phase = "renderer_ready",
            generation,
            prewarmed,
            renderer_state,
            elapsed_ms = elapsed.as_millis() as u64
        );
    }

    pub(super) fn revealed(&self, generation: u64, now: Instant) {
        let Ok(mut open) = self.open.lock() else {
            return;
        };
        let Some(open) = open.as_mut() else {
            return;
        };
        let prewarmed = open.prewarmed;
        let renderer_state = open.renderer_state;
        let Some(timing) = open.mark_revealed(generation, now) else {
            return;
        };
        ::tracing::info!(
            event = "popover_open_timing",
            phase = "revealed",
            generation,
            prewarmed,
            renderer_state,
            elapsed_ms = timing.request_to_reveal.as_millis() as u64,
            phase_ms = timing.ready_to_reveal.as_millis() as u64,
            renderer_ready_before_request = timing.renderer_ready_before_request
        );
        if timing.content_before_reveal && open.take_content_ready_at_reveal() {
            ::tracing::info!(
                event = "popover_open_timing",
                phase = "content_ready",
                generation,
                prewarmed,
                renderer_state,
                elapsed_ms = timing.request_to_reveal.as_millis() as u64,
                phase_ms = 0_u64,
                content_before_reveal = true
            );
        }
    }

    pub(super) fn content_ready(&self, generation: u64, now: Instant) {
        if let Ok(mut milestones) = self.milestones.lock()
            && milestones.generation == generation
        {
            milestones.content_ready_at = Some(now);
        }
        let Ok(mut open) = self.open.lock() else {
            return;
        };
        let Some(open) = open.as_mut() else {
            return;
        };
        let prewarmed = open.prewarmed;
        let renderer_state = open.renderer_state;
        let Some((elapsed, reveal_to_content)) = open.mark_content_ready(generation, now) else {
            return;
        };
        ::tracing::info!(
            event = "popover_open_timing",
            phase = "content_ready",
            generation,
            prewarmed,
            renderer_state,
            elapsed_ms = elapsed.as_millis() as u64,
            phase_ms = reveal_to_content.as_millis() as u64,
            content_before_reveal = false
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cold_open_reports_each_boundary_once_for_its_generation() {
        let requested_at = Instant::now();
        let mut timing = OpenTiming::new(
            7,
            requested_at,
            false,
            "build",
            RendererMilestones::default(),
        );

        assert_eq!(
            timing.mark_renderer_ready(7, requested_at + Duration::from_millis(900)),
            Some(Duration::from_millis(900))
        );
        assert_eq!(
            timing.mark_renderer_ready(7, requested_at + Duration::from_secs(1)),
            None
        );
        assert_eq!(
            timing.mark_revealed(7, requested_at + Duration::from_millis(920)),
            Some(RevealTiming {
                request_to_reveal: Duration::from_millis(920),
                ready_to_reveal: Duration::from_millis(20),
                renderer_ready_before_request: false,
                content_before_reveal: false,
            })
        );
        assert_eq!(
            timing.mark_content_ready(7, requested_at + Duration::from_millis(980)),
            Some((Duration::from_millis(980), Duration::from_millis(60)))
        );
        assert_eq!(
            timing.mark_content_ready(7, requested_at + Duration::from_secs(2)),
            None
        );
    }

    #[test]
    fn a_prewarmed_generation_keeps_its_actual_hidden_milestones() {
        let build_started_at = Instant::now();
        let requested_at = build_started_at + Duration::from_millis(900);
        let milestones = RendererMilestones {
            generation: 7,
            renderer_ready_at: Some(build_started_at + Duration::from_millis(700)),
            content_ready_at: Some(build_started_at + Duration::from_millis(800)),
        };
        let mut timing = OpenTiming::new(7, requested_at, true, "ready", milestones);

        assert_eq!(
            timing.mark_revealed(7, requested_at + Duration::from_millis(20)),
            Some(RevealTiming {
                request_to_reveal: Duration::from_millis(20),
                ready_to_reveal: Duration::from_millis(220),
                renderer_ready_before_request: true,
                content_before_reveal: true,
            })
        );
        assert!(timing.take_content_ready_at_reveal());
        assert!(!timing.take_content_ready_at_reveal());
    }

    #[test]
    fn a_stale_generation_cannot_complete_the_active_open_timing() {
        let requested_at = Instant::now();
        let mut timing = OpenTiming::new(
            8,
            requested_at,
            false,
            "build",
            RendererMilestones::default(),
        );

        assert_eq!(
            timing.mark_renderer_ready(7, requested_at + Duration::from_millis(50)),
            None
        );
        assert_eq!(
            timing.mark_content_ready(7, requested_at + Duration::from_millis(60)),
            None
        );
        assert_eq!(
            timing.mark_revealed(7, requested_at + Duration::from_millis(70)),
            None
        );
    }

    #[test]
    fn an_expired_replacement_keeps_the_original_click_boundary() {
        let requested_at = Instant::now();
        let timing = PopoverTiming::default();
        timing.begin_open(7, requested_at, true, "loading");

        assert!(timing.replace_open_generation(7, 8));
        let open = timing.open.lock().expect("the timing lock stays available");
        let open = open.as_ref().expect("the click remains active");
        assert_eq!(open.generation, 8);
        assert_eq!(open.requested_at, requested_at);
        assert!(!open.prewarmed);
        assert_eq!(open.renderer_state, "expired_replacement");
    }
}
