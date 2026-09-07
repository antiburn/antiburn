//! Scheduling policy for the analytics delivery queue.

use std::time::Duration;

/// The maximum number of network requests in one delivery pass.
pub(super) const REQUEST_BUDGET: u32 = 50;

/// Queue depth that starts a delivery pass without the settling delay.
const PRESSURE_DEPTH: u32 = REQUEST_BUDGET;

/// The maximum wait for the first event in a new backlog.
const SETTLE_DELAY: Duration = Duration::from_secs(60);

/// The pause between delivery passes while a backlog remains.
const DRAIN_COOLDOWN: Duration = Duration::from_secs(60);

/// The maximum wait after delivery fails or consent cannot be read.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitKind {
    Settle,
    DrainCooldown,
    RetryBackoff,
    ConsentRecheck,
}

/// The result of one bounded delivery pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FlushOutcome {
    Empty,
    Suspended,
    Delivered { remaining: u32 },
    Failed { remaining: u32 },
}

/// The queue scheduler state for one application run.
#[derive(Debug, Clone, Copy)]
pub(super) struct DeliverySchedule {
    deadline: Option<Duration>,
    wait_kind: Option<WaitKind>,
    consecutive_failures: u32,
}

impl DeliverySchedule {
    /// Create a schedule from the queue depth found at startup.
    pub(super) fn new(now: Duration, queue_depth: u32) -> Self {
        let mut schedule = Self {
            deadline: None,
            wait_kind: None,
            consecutive_failures: 0,
        };
        schedule.queued(now, queue_depth);
        schedule
    }

    /// Update the schedule after the queue depth changes.
    pub(super) fn queued(&mut self, now: Duration, queue_depth: u32) {
        if queue_depth == 0 {
            self.consecutive_failures = 0;
            self.park();
            return;
        }
        match self.wait_kind {
            Some(WaitKind::DrainCooldown | WaitKind::RetryBackoff | WaitKind::ConsentRecheck) => {}
            Some(WaitKind::Settle) if queue_depth >= PRESSURE_DEPTH => {
                self.deadline = Some(now);
            }
            Some(WaitKind::Settle) => {}
            None => {
                self.wait_kind = Some(WaitKind::Settle);
                self.deadline = Some(if queue_depth >= PRESSURE_DEPTH {
                    now
                } else {
                    now + SETTLE_DELAY
                });
            }
        }
    }

    /// Return the remaining wait, or `None` when the scheduler is parked.
    pub(super) fn next_delay(&self, now: Duration) -> Option<Duration> {
        self.deadline.map(|deadline| deadline.saturating_sub(now))
    }

    /// Update the schedule after one delivery pass finishes.
    pub(super) fn flush_completed(&mut self, now: Duration, outcome: FlushOutcome) {
        match outcome {
            FlushOutcome::Empty => {
                self.consecutive_failures = 0;
                self.park();
            }
            FlushOutcome::Suspended => {
                self.wait_kind = Some(WaitKind::ConsentRecheck);
                self.deadline = Some(now + MAX_RETRY_DELAY);
            }
            FlushOutcome::Delivered { remaining } => {
                self.consecutive_failures = 0;
                if remaining == 0 {
                    self.park();
                } else {
                    self.wait_kind = Some(WaitKind::DrainCooldown);
                    self.deadline = Some(now + DRAIN_COOLDOWN);
                }
            }
            FlushOutcome::Failed { remaining } => {
                if remaining == 0 {
                    self.consecutive_failures = 0;
                    self.park();
                    return;
                }
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                self.wait_kind = Some(WaitKind::RetryBackoff);
                self.deadline = Some(now + retry_delay(self.consecutive_failures));
            }
        }
    }

    fn park(&mut self) {
        self.deadline = None;
        self.wait_kind = None;
    }
}

fn retry_delay(consecutive_failures: u32) -> Duration {
    let exponent = consecutive_failures.saturating_sub(1).min(4);
    SETTLE_DELAY
        .checked_mul(1_u32 << exponent)
        .unwrap_or(MAX_RETRY_DELAY)
        .min(MAX_RETRY_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VISIT_EVENTS: u32 = 10;

    struct Workload {
        schedule: DeliverySchedule,
        store: crate::store::Store,
        _directory: tempfile::TempDir,
        now: Duration,
        captured: u32,
        delivered: u32,
        given_up: u32,
        failed_attempts: u32,
        largest_delivery: u32,
    }

    impl Workload {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            Self {
                schedule: DeliverySchedule::new(Duration::ZERO, 0),
                store: crate::store::Store::open_in_memory(directory.path()).unwrap(),
                _directory: directory,
                now: Duration::ZERO,
                captured: 0,
                delivered: 0,
                given_up: 0,
                failed_attempts: 0,
                largest_delivery: 0,
            }
        }

        fn queue_visit(&mut self) {
            for event in 0..VISIT_EVENTS {
                let payload = format!("{}:{event}", self.captured / VISIT_EVENTS);
                self.store
                    .queue_analytics_event("surface_viewed", &payload)
                    .unwrap();
                self.captured += 1;
                let depth = self.queue_depth();
                self.schedule.queued(self.now, depth);
            }
        }

        fn queue_depth(&self) -> u32 {
            self.store.analytics_event_count().unwrap()
        }

        fn overflow_drops(&self) -> u32 {
            self.captured - self.delivered - self.given_up - self.queue_depth()
        }

        fn advance_to(&mut self, target: Duration, online: bool) {
            while let Some(delay) = self.schedule.next_delay(self.now) {
                let deadline = self.now + delay;
                if deadline > target {
                    break;
                }
                self.now = deadline;
                if online {
                    let pending = self.store.pending_analytics_events(REQUEST_BUDGET).unwrap();
                    let delivered = pending.len() as u32;
                    self.largest_delivery = self.largest_delivery.max(delivered);
                    self.delivered += delivered;
                    let ids = pending.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
                    self.store.drop_analytics_events(&ids).unwrap();
                    let remaining = self.queue_depth();
                    self.schedule.flush_completed(
                        self.now,
                        if delivered == 0 {
                            FlushOutcome::Empty
                        } else {
                            FlushOutcome::Delivered { remaining }
                        },
                    );
                } else {
                    self.failed_attempts += 1;
                    let pending = self.store.pending_analytics_events(1).unwrap();
                    let ids = pending.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
                    self.given_up += self.store.fail_analytics_events(&ids, 5).unwrap() as u32;
                    let remaining = self.queue_depth();
                    self.schedule
                        .flush_completed(self.now, FlushOutcome::Failed { remaining });
                }
            }
            self.now = target;
        }
    }

    fn minutes(value: u64) -> Duration {
        Duration::from_secs(value * 60)
    }

    #[test]
    fn the_first_arrival_has_a_fixed_deadline_and_pressure_brings_it_forward() {
        let mut schedule = DeliverySchedule::new(Duration::ZERO, 0);
        schedule.queued(minutes(5), 1);
        assert_eq!(schedule.next_delay(minutes(5)), Some(minutes(1)));

        schedule.queued(minutes(5) + Duration::from_secs(30), 2);
        assert_eq!(schedule.next_delay(minutes(5)), Some(minutes(1)));

        schedule.queued(minutes(5) + Duration::from_secs(40), PRESSURE_DEPTH);
        assert_eq!(
            schedule.next_delay(minutes(5) + Duration::from_secs(40)),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn a_delivery_budget_cannot_be_bypassed_by_new_arrivals() {
        let mut schedule = DeliverySchedule::new(Duration::ZERO, PRESSURE_DEPTH);
        assert_eq!(schedule.next_delay(Duration::ZERO), Some(Duration::ZERO));

        schedule.flush_completed(Duration::ZERO, FlushOutcome::Delivered { remaining: 450 });
        schedule.queued(Duration::from_secs(1), 451);

        assert_eq!(
            schedule.next_delay(Duration::from_secs(1)),
            Some(Duration::from_secs(59))
        );
    }

    #[test]
    fn failures_back_off_without_restart_from_arrivals() {
        let mut schedule = DeliverySchedule::new(Duration::ZERO, PRESSURE_DEPTH);
        let expected = [1, 2, 4, 8, 15, 15];

        for expected_minutes in expected {
            schedule.flush_completed(Duration::ZERO, FlushOutcome::Failed { remaining: 500 });
            schedule.queued(Duration::from_secs(30), 500);
            assert_eq!(
                schedule.next_delay(Duration::ZERO),
                Some(minutes(expected_minutes))
            );
        }
    }

    #[test]
    fn suspended_delivery_rechecks_without_waking_for_new_rows() {
        let mut schedule = DeliverySchedule::new(Duration::ZERO, PRESSURE_DEPTH);
        schedule.flush_completed(Duration::ZERO, FlushOutcome::Suspended);
        schedule.queued(minutes(5), 100);

        assert_eq!(schedule.next_delay(minutes(5)), Some(minutes(10)));
    }

    #[test]
    fn an_empty_or_cleared_queue_parks_the_scheduler() {
        let mut schedule = DeliverySchedule::new(Duration::ZERO, 1);
        schedule.queued(Duration::from_secs(10), 0);
        assert_eq!(schedule.next_delay(Duration::from_secs(10)), None);

        schedule.queued(minutes(1), 1);
        schedule.flush_completed(minutes(2), FlushOutcome::Empty);
        assert_eq!(schedule.next_delay(minutes(2)), None);
    }

    /// This replay models activity, two previews, and four provider states per visit.
    #[test]
    fn phase_one_load_recovers_a_normal_offline_backlog_within_thirty_minutes() {
        let mut workload = Workload::new();

        // Six visits per hour stay well below the steady delivery budget.
        for visit in 0..12 {
            let at = minutes(visit * 10);
            workload.advance_to(at, true);
            workload.queue_visit();
        }
        workload.advance_to(minutes(120), true);
        assert_eq!(workload.queue_depth(), 0);
        assert_eq!(workload.overflow_drops(), 0);

        // Nine offline hours fill the bound while retry backoff limits requests.
        for visit in 12..66 {
            let at = minutes(visit * 10);
            workload.advance_to(at, false);
            workload.queue_visit();
        }
        let recovery_started = minutes(660);
        workload.advance_to(recovery_started, false);
        assert_eq!(workload.queue_depth(), 500);
        assert!(workload.failed_attempts < 50);
        assert!(workload.given_up > 0);
        let overflow_before_recovery = workload.overflow_drops();
        assert!(overflow_before_recovery > 0);

        // Normal visits continue while the backlog drains after connectivity returns.
        for at in [minutes(670), minutes(680)] {
            workload.advance_to(at, true);
            workload.queue_visit();
        }
        workload.advance_to(minutes(690), true);

        assert_eq!(workload.queue_depth(), 0);
        assert_eq!(workload.overflow_drops(), overflow_before_recovery);
        assert_eq!(workload.largest_delivery, REQUEST_BUDGET);
    }
}
