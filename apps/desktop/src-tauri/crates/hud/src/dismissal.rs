//! Shared pointer departure policy for docked and notch HUDs.

use std::time::{Duration, Instant};

pub(crate) const SAMPLE_INTERVAL: Duration = Duration::from_millis(50);
const GRACE: Duration = Duration::from_millis(400);
const PROXIMITY: f64 = 48.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Zone {
    Inside,
    Near,
    Far,
}

impl Zone {
    /// Compare desktop points with rectangles in native logical points.
    pub(crate) fn at(cursor: (f64, f64), rects: &[(f64, f64, f64, f64)]) -> Self {
        let mut distance = f64::INFINITY;
        for &(x, y, width, height) in rects {
            if cursor.0 >= x && cursor.0 < x + width && cursor.1 >= y && cursor.1 < y + height {
                return Self::Inside;
            }
            let dx = (x - cursor.0).max(cursor.0 - x - width).max(0.0);
            let dy = (y - cursor.1).max(cursor.1 - y - height).max(0.0);
            distance = distance.min(dx.hypot(dy));
        }
        if distance <= PROXIMITY {
            Self::Near
        } else {
            Self::Far
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Dismissal {
    start: Instant,
    hold: Duration,
    linger: Duration,
    interacted: bool,
    exited: Option<Instant>,
    unavailable: Option<Instant>,
}

impl Dismissal {
    pub(crate) fn new(now: Instant, hold: Duration, linger: Duration) -> Self {
        Self {
            start: now,
            hold,
            linger,
            interacted: hold.is_zero(),
            exited: None,
            unavailable: None,
        }
    }

    /// Return true when the current reveal can park.
    pub(crate) fn sample(&mut self, now: Instant, zone: Option<Zone>) -> bool {
        let zone = match zone {
            Some(zone) => {
                self.unavailable = None;
                zone
            }
            None => {
                let unavailable = *self.unavailable.get_or_insert(now);
                if now.saturating_duration_since(unavailable) < GRACE {
                    return false;
                }
                Zone::Far
            }
        };
        if zone == Zone::Inside {
            self.interacted = true;
            self.exited = None;
            return false;
        }
        if !self.interacted {
            let elapsed = now.saturating_duration_since(self.start);
            return elapsed >= self.hold && elapsed >= self.linger;
        }
        match zone {
            Zone::Inside => false,
            Zone::Far => true,
            Zone::Near => now.saturating_duration_since(*self.exited.get_or_insert(now)) >= GRACE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peek(now: Instant) -> Dismissal {
        Dismissal::new(now, Duration::ZERO, Duration::from_millis(1500))
    }

    #[test]
    fn far_departures_close_on_the_first_sample() {
        let now = Instant::now();
        let mut timing = peek(now);
        assert!(!timing.sample(now, Some(Zone::Inside)));
        assert!(timing.sample(now + SAMPLE_INTERVAL, Some(Zone::Far)));
    }

    #[test]
    fn nearby_movement_does_not_restart_the_exit_timer() {
        let now = Instant::now();
        let mut timing = peek(now);
        assert!(!timing.sample(now, Some(Zone::Near)));
        assert!(!timing.sample(now + Duration::from_millis(399), Some(Zone::Near)));
        assert!(timing.sample(now + GRACE, Some(Zone::Near)));
    }

    #[test]
    fn returning_inside_cancels_the_pending_exit() {
        let now = Instant::now();
        let mut timing = peek(now);
        assert!(!timing.sample(now, Some(Zone::Near)));
        assert!(!timing.sample(now + GRACE, Some(Zone::Inside)));
        assert!(!timing.sample(now + GRACE, Some(Zone::Near)));
        assert!(timing.sample(now + GRACE * 2, Some(Zone::Near)));
    }

    #[test]
    fn unattended_wakes_keep_their_original_hold() {
        let now = Instant::now();
        let mut timing = Dismissal::new(now, Duration::from_millis(4800), Duration::from_secs(3));
        assert!(!timing.sample(now, Some(Zone::Far)));
        assert!(!timing.sample(now + Duration::from_millis(4799), Some(Zone::Far)));
        assert!(timing.sample(now + Duration::from_millis(4800), Some(Zone::Far)));
    }

    #[test]
    fn entering_a_wake_releases_its_minimum_hold() {
        let now = Instant::now();
        let mut timing = Dismissal::new(now, Duration::from_millis(4800), Duration::from_secs(3));
        assert!(!timing.sample(now, Some(Zone::Inside)));
        assert!(timing.sample(now + SAMPLE_INTERVAL, Some(Zone::Far)));
    }

    #[test]
    fn nearby_wakes_do_not_count_as_interaction() {
        let now = Instant::now();
        let mut timing = Dismissal::new(now, Duration::from_millis(4800), Duration::from_secs(3));
        assert!(!timing.sample(now + GRACE, Some(Zone::Near)));
        assert!(!timing.sample(now + GRACE * 2, Some(Zone::Far)));
    }

    #[test]
    fn unavailable_samples_have_a_bounded_recovery_period() {
        let now = Instant::now();
        let mut timing = peek(now);
        assert!(!timing.sample(now, None));
        assert!(!timing.sample(now + Duration::from_millis(399), None));
        assert!(timing.sample(now + GRACE, None));
    }

    #[test]
    fn recovered_samples_clear_the_failure_deadline() {
        let now = Instant::now();
        let mut timing = peek(now);
        assert!(!timing.sample(now, None));
        assert!(!timing.sample(now + GRACE, Some(Zone::Inside)));
        assert!(!timing.sample(now + GRACE, None));
        assert!(timing.sample(now + GRACE * 2, None));
    }

    #[test]
    fn failures_do_not_shorten_an_unattended_wake() {
        let now = Instant::now();
        let mut timing = Dismissal::new(now, Duration::from_millis(4800), Duration::from_secs(3));
        assert!(!timing.sample(now, None));
        assert!(!timing.sample(now + GRACE, None));
        assert!(timing.sample(now + Duration::from_millis(4800), None));
    }

    #[test]
    fn geometry_uses_half_open_bounds_and_euclidean_distance() {
        let rect = [(-300.0, -100.0, 176.0, 80.0)];
        assert_eq!(Zone::at((-300.0, -100.0), &rect), Zone::Inside);
        assert_eq!(Zone::at((-124.0, -20.0), &rect), Zone::Near);
        assert_eq!(Zone::at((-76.0, -50.0), &rect), Zone::Near);
        assert_eq!(Zone::at((-75.9, -50.0), &rect), Zone::Far);
        assert_eq!(Zone::at((-90.0, 14.0), &rect), Zone::Far);
    }

    #[test]
    fn all_anchors_and_backing_scales_keep_the_same_grace_distance() {
        for scale in [1.0, 1.5, 2.0] {
            for (x, y) in [(-1920.0, -1080.0), (0.0, 0.0), (1744.0, 0.0), (0.0, 1000.0)] {
                let physical = (x * scale, y * scale, 176.0 * scale, 80.0 * scale);
                let logical = (
                    physical.0 / scale,
                    physical.1 / scale,
                    physical.2 / scale,
                    physical.3 / scale,
                );
                for (dx, dy) in [(-48.0, 40.0), (224.0, 40.0), (88.0, -48.0), (88.0, 128.0)] {
                    let cursor = ((x + dx) * scale, (y + dy) * scale);
                    assert_eq!(
                        Zone::at((cursor.0 / scale, cursor.1 / scale), &[logical]),
                        Zone::Near
                    );
                }
                assert_eq!(Zone::at((x + 225.0, y + 40.0), &[logical]), Zone::Far);
            }
        }
    }

    #[test]
    fn any_local_region_can_hold_the_hud_open() {
        let rects = [(0.0, 0.0, 200.0, 32.0), (40.0, 32.0, 120.0, 100.0)];
        assert_eq!(Zone::at((100.0, 16.0), &rects), Zone::Inside);
        assert_eq!(Zone::at((100.0, 100.0), &rects), Zone::Inside);
        assert_eq!(Zone::at((500.0, 0.0), &rects), Zone::Far);
    }
}
