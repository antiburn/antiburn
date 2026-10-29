use std::collections::BTreeMap;
use std::time::Instant;

const SHARED_SLOTS: usize = 15;
const TOTAL_SLOTS: usize = 16;

#[derive(Default)]
struct RecentFirstTurns {
    recent_dispatches: u8,
}

impl RecentFirstTurns {
    fn is_fair_turn(&self) -> bool {
        self.recent_dispatches == 3
    }

    fn dispatched(&mut self) {
        self.recent_dispatches = (self.recent_dispatches + 1) % 4;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WorkOrigin {
    Pass,
    Live,
    Recovery,
}

pub(super) struct QueueInput<T> {
    /// Use a collision-free encoding of environment, agent, and physical binding.
    /// Include the source session ID when several sessions share one database.
    pub key: String,
    pub payload: T,
    pub activity: Option<i64>,
    pub observed_at_epoch: i64,
    pub observed_at: Instant,
    pub origin: WorkOrigin,
    pub live: bool,
}

pub(super) struct QueuedWork<T> {
    pub key: String,
    pub payload: T,
    pub activity: Option<i64>,
    pub priority: Option<i64>,
    pub generation: u64,
    pub ticket: u64,
    pub enqueued_at: Instant,
    pub dirtied_at: Instant,
    pub origin: WorkOrigin,
    pub live: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DispatchLane {
    Shared,
    ReservedLive,
}

pub(super) struct Dispatch<T> {
    pub work: QueuedWork<T>,
    pub lane: DispatchLane,
}

// The scan owner must commit prepared results through the guarded Store API.
// The queue does not write to Store.
struct RunningWork {
    generation: u64,
    lane: DispatchLane,
    origin: WorkOrigin,
}

pub(super) struct ScanQueue<T> {
    pending: BTreeMap<String, QueuedWork<T>>,
    running: BTreeMap<String, RunningWork>,
    turns: RecentFirstTurns,
    next_generation: u64,
    next_ticket: u64,
    shared_running: usize,
}

impl<T> Default for ScanQueue<T> {
    fn default() -> Self {
        Self {
            pending: BTreeMap::new(),
            running: BTreeMap::new(),
            turns: RecentFirstTurns::default(),
            next_generation: 0,
            next_ticket: 0,
            shared_running: 0,
        }
    }
}

impl<T> ScanQueue<T> {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Each dirty hint creates a generation, including equal or older activity.
    pub(super) fn enqueue(&mut self, input: QueueInput<T>) -> u64 {
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .expect("scan dirty generation overflows");
        let generation = self.next_generation;
        let priority = input.activity.map(|at| at.min(input.observed_at_epoch));
        if let Some(pending) = self.pending.get_mut(&input.key) {
            pending.payload = input.payload;
            pending.activity = input.activity;
            pending.priority = priority;
            pending.generation = generation;
            pending.dirtied_at = input.observed_at;
            pending.live |= input.live;
            if input.origin == WorkOrigin::Recovery
                || (input.origin == WorkOrigin::Pass && pending.origin != WorkOrigin::Recovery)
            {
                pending.origin = input.origin;
            }
        } else {
            self.next_ticket = self
                .next_ticket
                .checked_add(1)
                .expect("scan service ticket overflows");
            self.pending.insert(
                input.key.clone(),
                QueuedWork {
                    key: input.key,
                    payload: input.payload,
                    activity: input.activity,
                    priority,
                    generation,
                    ticket: self.next_ticket,
                    enqueued_at: input.observed_at,
                    dirtied_at: input.observed_at,
                    origin: input.origin,
                    live: input.live,
                },
            );
        }
        generation
    }

    pub(super) fn dispatch(&mut self) -> Option<Dispatch<T>> {
        if self.running.len() >= TOTAL_SLOTS {
            return None;
        }
        let lane = if self.shared_running < SHARED_SLOTS {
            DispatchLane::Shared
        } else {
            DispatchLane::ReservedLive
        };
        let fair = lane == DispatchLane::Shared && self.turns.is_fair_turn();
        let key = self
            .pending
            .iter()
            .filter(|(key, work)| {
                !self.running.contains_key(*key) && (lane == DispatchLane::Shared || work.live)
            })
            .min_by(|(left_key, left), (right_key, right)| {
                if fair {
                    left.ticket
                        .cmp(&right.ticket)
                        .then_with(|| left_key.cmp(right_key))
                } else {
                    right
                        .priority
                        .cmp(&left.priority)
                        .then_with(|| left_key.cmp(right_key))
                }
            })
            .map(|(key, _)| key.clone())?;
        let work = self.pending.remove(&key).expect("selected work is pending");
        self.running.insert(
            key,
            RunningWork {
                generation: work.generation,
                lane,
                origin: work.origin,
            },
        );
        if lane == DispatchLane::Shared {
            self.shared_running += 1;
            self.turns.dispatched();
        }
        Some(Dispatch { work, lane })
    }

    /// Keep the key and generation with the task ID, outside the task result.
    pub(super) fn release(&mut self, key: &str, generation: u64) -> bool {
        let Some(running) = self.running.get(key) else {
            return false;
        };
        if running.generation != generation {
            return false;
        }
        let lane = running.lane;
        self.running.remove(key);
        if lane == DispatchLane::Shared {
            self.shared_running -= 1;
        }
        true
    }

    pub(super) fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.running.is_empty()
    }

    #[cfg(test)]
    pub(super) fn running_len(&self) -> usize {
        self.running.len()
    }

    pub(super) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub(super) fn pending_live_len(&self) -> usize {
        self.pending
            .values()
            .filter(|work| work.origin == WorkOrigin::Live)
            .count()
    }

    pub(super) fn contains_pending(&self, key: &str) -> bool {
        self.pending.contains_key(key)
    }

    pub(super) fn has_pass_work(&self) -> bool {
        self.pending
            .values()
            .any(|work| work.origin != WorkOrigin::Live)
            || self
                .running
                .values()
                .any(|work| work.origin != WorkOrigin::Live)
    }
}

#[cfg(test)]
mod tests {
    use std::panic::catch_unwind;
    use std::time::Duration;

    use super::*;

    fn input(key: &str, activity: Option<i64>, at: Instant) -> QueueInput<usize> {
        QueueInput {
            key: key.to_owned(),
            payload: 0,
            activity,
            observed_at_epoch: 10_000,
            observed_at: at,
            origin: WorkOrigin::Pass,
            live: false,
        }
    }

    fn release(queue: &mut ScanQueue<usize>, dispatch: &Dispatch<usize>) {
        assert!(queue.release(&dispatch.work.key, dispatch.work.generation));
    }

    #[test]
    fn recovery_keeps_its_cutoff_exemption_when_hints_coalesce() {
        let at = Instant::now();
        let mut queue = ScanQueue::new();
        let mut live = input("source", Some(1), at);
        live.origin = WorkOrigin::Live;
        live.live = true;
        queue.enqueue(live);
        let mut recovery = input("source", Some(1), at);
        recovery.origin = WorkOrigin::Recovery;
        queue.enqueue(recovery);
        queue.enqueue(input("source", Some(1), at));
        assert!(queue.has_pass_work());
        assert_eq!(queue.dispatch().unwrap().work.origin, WorkOrigin::Recovery);
    }

    #[test]
    fn older_tickets_progress_during_continuous_new_and_hot_arrivals() {
        let at = Instant::now();
        let mut queue = ScanQueue::new();
        queue.enqueue(input("native/claude/old-a", Some(1), at));
        queue.enqueue(input("wsl/codex/old-b", None, at));
        queue.enqueue(input("native/pi/old-c", Some(-1), at));
        let mut served = Vec::new();
        for turn in 1..=12 {
            let key = format!("native/opencode/new-{turn:02}");
            queue.enqueue(input(&key, Some(100 + turn), at));
            queue.enqueue(input("native/claude/hot", Some(200 + turn), at));
            let dispatch = queue.dispatch().expect("eligible work exists");
            assert_eq!(dispatch.lane, DispatchLane::Shared);
            if turn % 4 == 0 {
                served.push((turn, dispatch.work.key.clone()));
            } else {
                assert_eq!(dispatch.work.key, "native/claude/hot");
            }
            release(&mut queue, &dispatch);
        }
        assert_eq!(
            served,
            vec![
                (4, "native/claude/old-a".to_owned()),
                (8, "wsl/codex/old-b".to_owned()),
                (12, "native/pi/old-c".to_owned()),
            ]
        );
    }

    #[test]
    fn background_cannot_take_the_live_only_slot() {
        let at = Instant::now();
        let mut queue = ScanQueue::new();
        for index in 0..16 {
            queue.enqueue(input(&format!("native/pi/history-{index:02}"), Some(1), at));
        }
        for _ in 0..15 {
            assert_eq!(queue.dispatch().unwrap().lane, DispatchLane::Shared);
        }
        assert_eq!(queue.running_len(), 15);
        assert!(queue.dispatch().is_none());

        let mut live = input("native/codex/active", Some(2), at);
        live.live = true;
        queue.enqueue(live);
        let dispatch = queue.dispatch().expect("the live slot is free");
        assert_eq!(dispatch.lane, DispatchLane::ReservedLive);
        assert_eq!(dispatch.work.origin, WorkOrigin::Pass);
        assert_eq!(queue.running_len(), 16);
        assert!(queue.dispatch().is_none());
        release(&mut queue, &dispatch);
        assert!(queue.dispatch().is_none());
    }

    #[test]
    fn reserved_dispatch_and_empty_polls_preserve_the_owed_fair_turn() {
        let at = Instant::now();
        let mut queue = ScanQueue::new();
        let mut running = Vec::new();
        for index in 0..15 {
            queue.enqueue(input(
                &format!("native/pi/running-{index:02}"),
                Some(10),
                at,
            ));
            running.push(queue.dispatch().unwrap());
        }
        assert!(queue.turns.is_fair_turn());
        assert!(queue.dispatch().is_none());
        queue.enqueue(input("native/pi/old", Some(1), at));
        assert!(queue.dispatch().is_none());
        for index in 0..8 {
            let mut live = input(&format!("native/claude/live-{index:02}"), Some(100), at);
            live.origin = WorkOrigin::Live;
            live.live = true;
            queue.enqueue(live);
            let dispatch = queue.dispatch().unwrap();
            assert_eq!(dispatch.lane, DispatchLane::ReservedLive);
            assert!(queue.turns.is_fair_turn());
            release(&mut queue, &dispatch);
        }
        queue.enqueue(input("native/codex/new", Some(200), at));
        release(&mut queue, &running[0]);
        let fair = queue.dispatch().unwrap();
        assert_eq!(fair.work.key, "native/pi/old");
        assert_eq!(fair.lane, DispatchLane::Shared);
        assert!(!queue.turns.is_fair_turn());
    }

    #[test]
    fn dirty_hints_keep_one_follow_up_with_its_ticket_and_latest_snapshot() {
        let at = Instant::now();
        let key = "native/opencode/db-a/session-a";
        let mut queue = ScanQueue::new();
        let first_generation = queue.enqueue(input(key, Some(100), at));
        let first = queue.dispatch().unwrap();
        let follow_up_at = at + Duration::from_secs(1);
        let tied_generation = queue.enqueue(input(key, Some(100), follow_up_at));
        let ticket = queue.pending[key].ticket;
        let mut backward = input(key, Some(99), at + Duration::from_secs(2));
        backward.payload = 42;
        backward.origin = WorkOrigin::Live;
        backward.live = true;
        let backward_generation = queue.enqueue(backward);
        assert!(first_generation < tied_generation);
        assert!(tied_generation < backward_generation);
        assert_eq!(queue.pending.len(), 1);
        assert!(queue.dispatch().is_none());

        release(&mut queue, &first);
        let follow_up = queue.dispatch().unwrap();
        assert_eq!(follow_up.work.payload, 42);
        assert_eq!(follow_up.work.activity, Some(99));
        assert_eq!(follow_up.work.generation, backward_generation);
        assert_eq!(follow_up.work.ticket, ticket);
        assert!(follow_up.work.ticket > first.work.ticket);
        assert_eq!(follow_up.work.enqueued_at, follow_up_at);
        assert_eq!(follow_up.work.dirtied_at, at + Duration::from_secs(2));
        assert_eq!(follow_up.work.origin, WorkOrigin::Pass);
        assert!(follow_up.work.live);
        assert!(!queue.release(key, first_generation));
        assert_eq!(queue.running_len(), 1);
        release(&mut queue, &follow_up);
        assert!(queue.is_empty());
    }

    #[test]
    fn full_identity_breaks_ties_and_keeps_distinct_bindings_separate() {
        let at = Instant::now();
        let keys = [
            "wsl/claude/file-a",
            "native/opencode/db-b/session-a",
            "native/opencode/db-a/session-a",
            "native/claude/file-a",
        ];
        let mut queue = ScanQueue::new();
        for key in keys {
            queue.enqueue(input(key, Some(100), at));
        }
        let mut selected = Vec::new();
        for _ in 0..4 {
            let dispatch = queue.dispatch().unwrap();
            selected.push(dispatch.work.key.clone());
            release(&mut queue, &dispatch);
        }
        assert_eq!(
            selected,
            vec![
                "native/claude/file-a",
                "native/opencode/db-a/session-a",
                "native/opencode/db-b/session-a",
                "wsl/claude/file-a",
            ]
        );
    }

    #[test]
    fn future_activity_is_clamped_for_priority_only() {
        let at = Instant::now();
        let mut queue = ScanQueue::new();
        let mut future = input("native/pi/future", Some(i64::MAX), at);
        future.observed_at_epoch = 100;
        queue.enqueue(future);
        queue.enqueue(input("native/codex/current", Some(101), at));
        let current = queue.dispatch().unwrap();
        assert_eq!(current.work.key, "native/codex/current");
        let future = queue.dispatch().unwrap();
        assert_eq!(future.work.activity, Some(i64::MAX));
        assert_eq!(future.work.priority, Some(100));
    }

    #[test]
    fn owner_releases_a_panicked_task_without_a_result() {
        let at = Instant::now();
        let key = "native/claude/file-a";
        let mut queue = ScanQueue::new();
        queue.enqueue(input(key, Some(1), at));
        let dispatch = queue.dispatch().unwrap();
        let mut task_owners = BTreeMap::new();
        task_owners.insert(7, (dispatch.work.key.clone(), dispatch.work.generation));
        queue.enqueue(input(key, Some(1), at));
        let joined = catch_unwind(|| {
            let _payload = dispatch.work.payload;
            panic!("preparation fails before it returns an identity");
        });
        assert!(joined.is_err());
        let (key, generation) = task_owners.remove(&7).unwrap();
        assert!(queue.release(&key, generation));
        let follow_up = queue.dispatch().expect("the follow-up is not stranded");
        assert_eq!(follow_up.work.key, key);
        release(&mut queue, &follow_up);
        assert!(queue.is_empty());
        assert!(queue.dispatch().is_none());
    }
}
