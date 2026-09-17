use super::*;
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar};
use tokio::sync::broadcast::error::TryRecvError;

mod aggregates;
mod execution;
mod model_worker;
mod reconcile;
mod schedules;
mod scoped_models;

fn key(session_id: &str) -> SessionKey {
    SessionKey::new("native", "claude-code", session_id)
}

fn session_ref(session_id: &str) -> SessionRef {
    SessionRef::from(&key(session_id))
}

/// A `Touched` fact for incarnation `inc` of the session, read at `seen`.
fn touched(session_id: &str, inc: u64, at: i64, seen: u64) -> Observation {
    Observation::Touched {
        session: TouchedSession {
            key: key(session_id),
            incarnation: Incarnation(inc),
            seen: Revision(seen),
        },
        agent: AgentKind::Claude,
        at,
    }
}

/// An anonymous touch of `agent` with generation `generation`.
fn anonymous(agent: AgentKind, at: i64, generation: u64) -> Observation {
    Observation::Anonymous {
        agent,
        at,
        generation: AnonymousGen(generation),
    }
}

/// One pass's cover of `agent` through generation `through`.
fn covered(agent: AgentKind, through: u64) -> Observation {
    Observation::AnonymousCovered {
        covers: vec![AnonymousCover {
            agent,
            through: AnonymousGen(through),
        }],
    }
}

fn cleared(agent: AgentKind, at: i64, cause: AnonymousClearCause) -> SessionEvent {
    SessionEvent::AnonymousCleared { agent, at, cause }
}

fn indexed(session_id: &str, inc: u64, at: i64, is_new: bool) -> IndexedSession {
    IndexedSession {
        key: key(session_id),
        agent: AgentKind::Claude,
        incarnation: Incarnation(inc),
        at,
        is_new,
    }
}

/// An `Indexed` fact for one session at `revision`.
fn index(session_id: &str, inc: u64, at: i64, is_new: bool, revision: u64) -> Observation {
    Observation::Indexed {
        sessions: vec![indexed(session_id, inc, at, is_new)],
        revision: Revision(revision),
    }
}

/// A keyed removal of incarnation `inc`, committed at `revision`.
fn removed(session_id: &str, inc: u64, reason: RemovalReason, revision: u64) -> Observation {
    Observation::Removed {
        scope: RemovalScope::One(key(session_id), Incarnation(inc)),
        reason,
        revision: Revision(revision),
    }
}

fn sync_removed(
    session_id: &str,
    inc: u64,
    reason: RemovalReason,
    revision: u64,
) -> SyncObservation {
    SyncObservation::Removed {
        scope: RemovalScope::One(key(session_id), Incarnation(inc)),
        reason,
        revision: Revision(revision),
    }
}

/// A broad removal committed at `revision`.
fn broad(reason: RemovalReason, revision: u64) -> Observation {
    Observation::Removed {
        scope: RemovalScope::Broad,
        reason,
        revision: Revision(revision),
    }
}

fn presence(session_id: &str, inc: u64, epoch: i64) -> Presence {
    Presence {
        key: key(session_id),
        incarnation: Incarnation(inc),
        epoch,
    }
}

/// A clock tied to tokio's own (pausable) instant, not the wall clock: every
/// `tokio::time::sleep` the actor awaits advances it exactly as far as it
/// advances tokio's clock, so a paused test can drive it deterministically.
/// `offset` lets a test move the actor's clock without waking a timer.
fn instant_clock(
    base_epoch: i64,
    offset: Arc<std::sync::atomic::AtomicI64>,
) -> impl Fn() -> i64 + Send + Sync + 'static {
    let base_instant = Instant::now();
    move || base_epoch + base_instant.elapsed().as_secs() as i64 + offset.load(Ordering::Relaxed)
}

/// What one scripted page read returns.
type PageResult = anyhow::Result<(Vec<Presence>, Revision)>;

/// The scripted store behind a test actor: which rows exist, at which
/// revision, plus a failure budget and a gate that holds a computed page
/// until the test releases it.
#[derive(Default)]
struct Scripted {
    rows: Mutex<BTreeMap<SessionKey, (Incarnation, i64)>>,
    revision: Mutex<Revision>,
    /// The next this many presence pages fail.
    failures: Mutex<usize>,
    requests: Mutex<Vec<Vec<SessionKey>>>,
    active_pages: Mutex<VecDeque<PageResult>>,
    active_requests: Mutex<Vec<(i64, Option<ActiveCursor>, usize)>>,
    active_gated: std::sync::atomic::AtomicBool,
    /// `None`: pages return at once. `Some(n)`: the next `n` computed pages
    /// return, then pages wait for `allow` or `release`.
    gate: (Mutex<Option<usize>>, Condvar),
    /// Presence pages that returned to the actor's blocking task.
    completed: std::sync::atomic::AtomicUsize,
}

impl Scripted {
    fn wait_for_gate(&self) {
        {
            // A held page waits for a permit. The wait is bounded so a
            // failing test cannot hang the runtime's shutdown on this thread.
            let mut gate = self.gate.0.lock().unwrap();
            let opened = std::time::Instant::now();
            loop {
                match *gate {
                    None => break,
                    Some(0) => {
                        if opened.elapsed() > Duration::from_secs(15) {
                            break;
                        }
                        gate = self
                            .gate
                            .1
                            .wait_timeout(gate, Duration::from_millis(50))
                            .unwrap()
                            .0;
                    }
                    Some(permits) => {
                        *gate = Some(permits - 1);
                        break;
                    }
                }
            }
        }
    }

    fn set_rows(&self, rows: Vec<(SessionKey, u64, i64)>, revision: u64) {
        *self.rows.lock().unwrap() = rows
            .into_iter()
            .map(|(key, inc, epoch)| (key, (Incarnation(inc), epoch)))
            .collect();
        *self.revision.lock().unwrap() = Revision(revision);
    }

    fn fail_next(&self, pages: usize) {
        *self.failures.lock().unwrap() = pages;
    }

    fn requests(&self) -> Vec<Vec<SessionKey>> {
        self.requests.lock().unwrap().clone()
    }

    /// Hold every page after it is computed, until `allow` or `release`.
    fn hold(&self) {
        *self.gate.0.lock().unwrap() = Some(0);
    }

    /// Let the next `pages` held or computed pages return, one each.
    fn allow(&self, pages: usize) {
        let mut gate = self.gate.0.lock().unwrap();
        *gate = Some(gate.unwrap_or(0) + pages);
        self.gate.1.notify_all();
    }

    fn release(&self) {
        *self.gate.0.lock().unwrap() = None;
        self.gate.1.notify_all();
    }

    /// Spin, without yielding to the actor, until `pages` presence pages
    /// have returned, then a short margin so the blocking task can finish.
    fn spin_until_completed(&self, pages: usize) {
        for _ in 0..5_000 {
            if self.completed.load(Ordering::Relaxed) >= pages {
                std::thread::sleep(Duration::from_millis(20));
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the page did not complete in time");
    }
}

impl ReconcileSource for Scripted {
    fn presence(&self, keys: &[SessionKey]) -> anyhow::Result<(Vec<Presence>, Revision)> {
        assert!(
            keys.len() <= RECONCILE_PAGE,
            "a page never exceeds RECONCILE_PAGE"
        );
        self.requests.lock().unwrap().push(keys.to_vec());
        let answer = {
            let mut failures = self.failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                Err(anyhow::anyhow!("scripted page failure"))
            } else {
                let rows = self.rows.lock().unwrap();
                let present = keys
                    .iter()
                    .filter_map(|key| {
                        rows.get(key).map(|(incarnation, epoch)| Presence {
                            key: key.clone(),
                            incarnation: *incarnation,
                            epoch: *epoch,
                        })
                    })
                    .collect();
                Ok((present, *self.revision.lock().unwrap()))
            }
        };
        self.wait_for_gate();
        self.completed.fetch_add(1, Ordering::Relaxed);
        answer
    }

    fn active(
        &self,
        since: i64,
        after: Option<&ActiveCursor>,
        limit: usize,
    ) -> anyhow::Result<(Vec<Presence>, Revision)> {
        self.active_requests
            .lock()
            .unwrap()
            .push((since, after.cloned(), limit));
        let answer = self
            .active_pages
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok((Vec::new(), Revision(0))));
        if self.active_gated.load(Ordering::Relaxed) {
            self.wait_for_gate();
            self.completed.fetch_add(1, Ordering::Relaxed);
        }
        answer
    }
}

/// A running test actor: the bus handle, the scripted store, and the clock
/// offset the test can move.
struct Harness {
    events: Arc<SessionEvents>,
    offset: Arc<std::sync::atomic::AtomicI64>,
}

impl Harness {
    fn rounds(&self) -> u64 {
        self.events.rounds.load(Ordering::Relaxed)
    }

    /// Put an observation in the inbox without yielding. Panics when full.
    fn push(&self, observation: Observation) {
        self.events
            .inbox
            .try_send(observation)
            .expect("the inbox has room");
    }

    fn with_registry<T>(&self, read: impl FnOnce(&Registry) -> T) -> T {
        read(&self.events.registry.lock().unwrap())
    }
}

/// Seed the registry with incarnation-zero rows, start the actor on tokio's
/// clock with an empty scripted store, and return the harness.
fn start(base_epoch: i64, seed: Vec<(SessionKey, i64)>) -> Harness {
    start_with(base_epoch, seed, Arc::new(Scripted::default()))
}

fn start_with(base_epoch: i64, seed: Vec<(SessionKey, i64)>, source: Arc<Scripted>) -> Harness {
    let source: Arc<dyn ReconcileSource> = source;
    let events = Arc::new(SessionEvents::default());
    events.seed(
        seed.into_iter()
            .map(|(key, epoch)| Presence {
                key,
                incarnation: Incarnation(0),
                epoch,
            })
            .collect(),
        Revision(0),
        base_epoch,
    );
    let inbox = events.claim_actor().expect("a fresh bus has no actor yet");
    let offset = Arc::new(std::sync::atomic::AtomicI64::new(0));
    let actor_events = events.clone();
    let clock = instant_clock(base_epoch, offset.clone());
    tokio::spawn(async move {
        run(&actor_events, inbox, source, &clock).await;
    });
    Harness { events, offset }
}

/// Let the actor drain its inbox without advancing the clock.
async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

/// Yield until `done` holds. Blocking-pool work completes in real time, so
/// this also sleeps the thread briefly between checks. Panics after a
/// generous real-time bound.
async fn wait_until(mut done: impl FnMut() -> bool) {
    for _ in 0..5_000 {
        if done() {
            return;
        }
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("the condition did not hold in time");
}

/// The next event on the bus, without its sequence.
async fn next(bus: &mut broadcast::Receiver<Sequenced>) -> SessionEvent {
    bus.recv().await.unwrap().event
}

/// Every event the bus holds right now, without sequences.
fn drain(bus: &mut broadcast::Receiver<Sequenced>) -> Vec<SessionEvent> {
    let mut out = Vec::new();
    loop {
        match bus.try_recv() {
            Ok(sequenced) => out.push(sequenced.event),
            Err(TryRecvError::Empty) => return out,
            Err(other) => panic!("unexpected bus state: {other:?}"),
        }
    }
}

/// The live sessions from an effectively unbounded snapshot.
fn live(events: &SessionEvents) -> Vec<LiveSession> {
    events.snapshot(usize::MAX).sessions
}

const BASE: i64 = 1_000_000;

#[tokio::test(start_paused = true)]
async fn touched_publishes_activity_with_the_same_key_and_time() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    events.report_async(touched("busy", 1, BASE + 3, 4)).await;

    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: Some(session_ref("busy")),
            agent: AgentKind::Claude,
            at: BASE + 3,
            resumed: false,
        }
    );
    assert_eq!(
        live(events),
        vec![LiveSession {
            session: session_ref("busy"),
            agent: AgentKind::Claude,
            last_activity_at: BASE + 3,
            quiet: false,
            execution: None,
        }]
    );
}

#[tokio::test(start_paused = true)]
async fn a_duplicate_touch_publishes_nothing() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    for _ in 0..3 {
        events.report_async(touched("busy", 1, BASE + 3, 4)).await;
    }
    settle().await;

    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { .. }
    ));
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // An older epoch for the same session is also nothing new.
    events.report_async(touched("busy", 1, BASE + 1, 4)).await;
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
}

#[tokio::test(start_paused = true)]
async fn a_touch_older_than_the_window_is_rejected() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    events
        .report_async(touched("ancient", 1, BASE - ACTIVE_SESSION_WINDOW_SECS, 4))
        .await;
    settle().await;

    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    assert!(live(events).is_empty());
}

#[tokio::test(start_paused = true)]
async fn touched_without_a_session_publishes_agent_activity_once_per_epoch() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    events
        .report_async(anonymous(AgentKind::Codex, BASE, 1))
        .await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: None,
            agent: AgentKind::Codex,
            at: BASE,
            resumed: false,
        }
    );
    assert!(live(events).is_empty());
    assert_eq!(
        events.snapshot(usize::MAX).anonymous,
        vec![LiveAnonymous {
            agent: AgentKind::Codex,
            last_activity_at: BASE,
        }]
    );

    // The same epoch again, with a newer generation, says nothing new.
    events
        .report_async(anonymous(AgentKind::Codex, BASE, 2))
        .await;
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // A later anonymous write publishes again.
    events
        .report_async(anonymous(AgentKind::Codex, BASE + 4, 3))
        .await;
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { session: None, at, .. } if at == BASE + 4
    ));
    assert_eq!(
        events.snapshot(usize::MAX).anonymous,
        vec![LiveAnonymous {
            agent: AgentKind::Codex,
            last_activity_at: BASE + 4,
        }]
    );
}

#[tokio::test(start_paused = true)]
async fn indexing_a_new_session_does_not_clear_anonymous_activity_but_its_cover_does() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    events
        .report_async(anonymous(AgentKind::Claude, BASE + 2, 1))
        .await;
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { session: None, .. }
    ));

    // Discovery resolves the key: one Started, no duplicate resume, and
    // the anonymous state stays until the pass's own cover arrives.
    events
        .report_async(index("resolved", 1, BASE + 2, true, 3))
        .await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Started {
            session: session_ref("resolved"),
            agent: AgentKind::Claude,
            at: BASE + 2,
        }
    );
    assert_eq!(events.snapshot(usize::MAX).anonymous.len(), 1);
    events
        .report_async(anonymous(AgentKind::Claude, BASE + 2, 2))
        .await;
    settle().await;
    assert_eq!(
        bus.try_recv().unwrap_err(),
        TryRecvError::Empty,
        "the entry survived Started, so an equal epoch is nothing new"
    );

    // The cover that follows the pass's Indexed report clears it.
    events.report_async(covered(AgentKind::Claude, 2)).await;
    assert_eq!(
        next(&mut bus).await,
        cleared(AgentKind::Claude, BASE, AnonymousClearCause::Resolved)
    );
    assert!(events.snapshot(usize::MAX).anonymous.is_empty());

    // Cleared: an equal anonymous epoch publishes again.
    events
        .report_async(anonymous(AgentKind::Claude, BASE + 2, 3))
        .await;
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { session: None, .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn an_older_cover_leaves_a_newer_generation_in_the_same_second() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    // Generation 1 is captured by a pass; generation 2 arrives in the same
    // second while that pass runs.
    events
        .report_async(anonymous(AgentKind::Codex, BASE, 1))
        .await;
    events
        .report_async(anonymous(AgentKind::Codex, BASE, 2))
        .await;
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { session: None, .. }
    ));
    events.report_async(covered(AgentKind::Codex, 1)).await;
    settle().await;
    assert_eq!(
        bus.try_recv().unwrap_err(),
        TryRecvError::Empty,
        "a cover through 1 cannot clear generation 2"
    );
    assert_eq!(events.snapshot(usize::MAX).anonymous.len(), 1);

    // The next pass captures generation 2 and clears it.
    events.report_async(covered(AgentKind::Codex, 2)).await;
    assert_eq!(
        next(&mut bus).await,
        cleared(AgentKind::Codex, BASE, AnonymousClearCause::Resolved)
    );
    assert!(events.snapshot(usize::MAX).anonymous.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_clock_rollback_cannot_let_an_old_cover_clear_a_newer_touch() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    events
        .report_async(anonymous(AgentKind::Codex, BASE + 10, 1))
        .await;
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { session: None, at, .. } if at == BASE + 10
    ));
    // The clock moved back: a newer generation carries an older time. It
    // publishes no activity, but it protects the entry from the older
    // cover and keeps the later deadline.
    events
        .report_async(anonymous(AgentKind::Codex, BASE + 5, 2))
        .await;
    events.report_async(covered(AgentKind::Codex, 1)).await;
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    assert_eq!(
        events.snapshot(usize::MAX).anonymous,
        vec![LiveAnonymous {
            agent: AgentKind::Codex,
            last_activity_at: BASE + 10,
        }]
    );

    // The registry deadline runs from the later time, not the rolled-back one.
    tokio::time::sleep(Duration::from_secs(QUIET_WINDOW_SECS as u64 + 1)).await;
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(
        next(&mut bus).await,
        cleared(
            AgentKind::Codex,
            BASE + 10 + QUIET_WINDOW_SECS + 1,
            AnonymousClearCause::Expired
        )
    );
}

#[tokio::test(start_paused = true)]
async fn a_cover_for_another_agent_or_an_older_generation_clears_nothing() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    events
        .report_async(anonymous(AgentKind::Codex, BASE, 3))
        .await;
    events
        .report_async(anonymous(AgentKind::Claude, BASE, 4))
        .await;
    settle().await;
    drain(&mut bus);

    // A scoped pass over Claude only covers Claude; a stale cover from a
    // pass that started before generation 3 covers nothing.
    events.report_async(covered(AgentKind::Claude, 4)).await;
    events.report_async(covered(AgentKind::Codex, 2)).await;
    settle().await;
    assert_eq!(
        drain(&mut bus),
        vec![cleared(
            AgentKind::Claude,
            BASE,
            AnonymousClearCause::Resolved
        )]
    );
    assert_eq!(
        events.snapshot(usize::MAX).anonymous,
        vec![LiveAnonymous {
            agent: AgentKind::Codex,
            last_activity_at: BASE,
        }]
    );

    // A cover for an agent with no entry is silent.
    events.report_async(covered(AgentKind::Claude, 9)).await;
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
}

#[tokio::test(start_paused = true)]
async fn anonymous_activity_expires_on_the_registry_deadline_and_an_equal_epoch_reannounces() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    events
        .report_async(anonymous(AgentKind::Codex, BASE, 1))
        .await;
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { session: None, .. }
    ));

    // Nothing before the quiet window.
    tokio::time::sleep(Duration::from_secs(QUIET_WINDOW_SECS as u64 - 1)).await;
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // The registry clears it at the window, no reader timer involved.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        next(&mut bus).await,
        cleared(
            AgentKind::Codex,
            BASE + QUIET_WINDOW_SECS + 1,
            AnonymousClearCause::Expired
        )
    );
    assert!(events.snapshot(usize::MAX).anonymous.is_empty());

    // A late cover for the expired generation is silent.
    events.report_async(covered(AgentKind::Codex, 1)).await;
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // After expiry, a touch at the same epoch is new activity again, and a
    // touch already outside the window is not.
    harness
        .offset
        .store(-(QUIET_WINDOW_SECS + 1), Ordering::Relaxed);
    events
        .report_async(anonymous(AgentKind::Codex, BASE, 2))
        .await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: None,
            agent: AgentKind::Codex,
            at: BASE,
            resumed: false,
        }
    );
    harness.offset.store(0, Ordering::Relaxed);
    events.report_async(covered(AgentKind::Codex, 2)).await;
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::AnonymousCleared { .. }
    ));
    events
        .report_async(anonymous(AgentKind::Codex, BASE - 1, 3))
        .await;
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    assert!(events.snapshot(usize::MAX).anonymous.is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_indexed_report_before_its_cover_starts_the_session_then_clears_the_agent() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    events
        .report_async(anonymous(AgentKind::Codex, BASE, 1))
        .await;
    settle().await;
    drain(&mut bus);

    // The pass reports Indexed, then its cover, on one ordered path.
    let codex = SessionKey::new("native", "codex", "found");
    harness.push(Observation::Indexed {
        sessions: vec![IndexedSession {
            key: codex.clone(),
            agent: AgentKind::Codex,
            incarnation: Incarnation(1),
            at: BASE,
            is_new: true,
        }],
        revision: Revision(2),
    });
    harness.push(covered(AgentKind::Codex, 1));
    settle().await;
    let seen = drain(&mut bus);
    assert_eq!(
        seen,
        vec![
            SessionEvent::Started {
                session: SessionRef::from(&codex),
                agent: AgentKind::Codex,
                at: BASE,
            },
            cleared(AgentKind::Codex, BASE, AnonymousClearCause::Resolved),
        ],
        "the session starts before the agent's anonymous state clears"
    );
    let snapshot = events.snapshot(usize::MAX);
    assert_eq!(snapshot.sessions.len(), 1);
    assert!(snapshot.anonymous.is_empty());
}

#[tokio::test(start_paused = true)]
async fn anonymous_deadlines_and_session_deadlines_share_one_index() {
    let harness = start(BASE, vec![(key("seeded"), BASE - 1)]);
    let events = &harness.events;
    let mut bus = events.subscribe();

    events
        .report_async(anonymous(AgentKind::Codex, BASE, 1))
        .await;
    settle().await;
    drain(&mut bus);
    harness.with_registry(|registry| {
        assert_eq!(registry.deadlines.len(), 2);
    });

    // One wake at the session's deadline: the session goes quiet first,
    // then the anonymous entry, whose window has also passed, expires.
    tokio::time::sleep(Duration::from_secs(QUIET_WINDOW_SECS as u64 + 1)).await;
    assert_eq!(
        drain(&mut bus),
        vec![
            SessionEvent::Quiet {
                session: session_ref("seeded"),
                agent: AgentKind::Claude,
                at: BASE + QUIET_WINDOW_SECS,
            },
            cleared(
                AgentKind::Codex,
                BASE + QUIET_WINDOW_SECS,
                AnonymousClearCause::Expired
            ),
        ]
    );
    harness.with_registry(|registry| {
        assert_eq!(registry.deadlines.len(), 1);
        assert!(registry.anonymous.is_empty());
    });
}

#[tokio::test(start_paused = true)]
async fn a_first_index_publishes_started_once() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    let observation = index("fresh", 1, BASE, true, 3);
    events.report_async(observation.clone()).await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Started {
            session: session_ref("fresh"),
            agent: AgentKind::Claude,
            at: BASE,
        }
    );

    // The same rows again, with no newer epoch: nothing to say.
    events.report_async(observation).await;
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // A later epoch for a known session is activity, not another start.
    events
        .report_async(index("fresh", 1, BASE + 5, false, 4))
        .await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: Some(session_ref("fresh")),
            agent: AgentKind::Claude,
            at: BASE + 5,
            resumed: false,
        }
    );
}

#[tokio::test(start_paused = true)]
async fn an_index_older_than_the_window_is_history_not_activity() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    events
        .report_async(index("old", 1, BASE - ACTIVE_SESSION_WINDOW_SECS, true, 3))
        .await;
    settle().await;

    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    assert!(live(events).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_session_goes_idle_at_the_window_and_a_touch_moves_the_deadline() {
    let harness = start(BASE, vec![(key("seeded"), BASE)]);
    let events = &harness.events;
    let mut bus = events.subscribe();

    // 170 s in: quiet since 30 s, still 10 s left, and a touch restarts
    // both windows.
    tokio::time::sleep(Duration::from_secs(170)).await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Quiet {
            session: session_ref("seeded"),
            agent: AgentKind::Claude,
            at: BASE + 31,
        }
    );
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    events
        .report_async(touched("seeded", 0, BASE + 170, 0))
        .await;
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { resumed: true, .. }
    ));

    // The original deadline (180 s plus slack) passes with nothing to say.
    tokio::time::sleep(Duration::from_secs(15)).await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // The moved deadlines: quiet at 170 + 30 + 1 s of slack, then idle at
    // 170 + 180 + 1.
    tokio::time::sleep(Duration::from_secs(170)).await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Quiet {
            session: session_ref("seeded"),
            agent: AgentKind::Claude,
            at: BASE + 201,
        }
    );
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Idle {
            session: session_ref("seeded"),
            agent: AgentKind::Claude,
            at: BASE + 351,
        }
    );
    assert!(live(events).is_empty());
}

#[tokio::test(start_paused = true)]
async fn sessions_expire_in_deadline_order() {
    let harness = start(
        BASE,
        vec![(key("older"), BASE - 100), (key("newer"), BASE - 10)],
    );
    let mut bus = harness.events.subscribe();

    // The newer session, seeded inside the quiet window, goes quiet at
    // t=20 s plus slack. The older one was seeded quiet and says nothing.
    tokio::time::sleep(Duration::from_secs(22)).await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Quiet {
            session: session_ref("newer"),
            agent: AgentKind::Claude,
            at: BASE + 21,
        }
    );

    // The older session crosses its window at t=80 s, plus slack.
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Idle { session, .. } if session == session_ref("older")
    ));
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // The newer one at t=170 s, plus slack.
    tokio::time::sleep(Duration::from_secs(90)).await;
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Idle { session, .. } if session == session_ref("newer")
    ));
}

#[tokio::test(start_paused = true)]
async fn a_session_goes_quiet_at_thirty_seconds_and_resumes_on_a_touch() {
    let harness = start(BASE, vec![(key("busy"), BASE)]);
    let events = &harness.events;
    let mut bus = events.subscribe();

    tokio::time::sleep(Duration::from_secs(31)).await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Quiet {
            session: session_ref("busy"),
            agent: AgentKind::Claude,
            at: BASE + 31,
        }
    );
    // Quiet is not idle: the session is still in the snapshot, marked
    // quiet, so a reader never derives the window from the timestamp.
    assert_eq!(live(events).len(), 1);
    assert!(live(events)[0].quiet);

    // A write after quiet is a resume, and starts a new quiet window.
    events.report_async(touched("busy", 0, BASE + 40, 0)).await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: Some(session_ref("busy")),
            agent: AgentKind::Claude,
            at: BASE + 40,
            resumed: true,
        }
    );
    tokio::time::sleep(Duration::from_secs(40)).await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Quiet {
            session: session_ref("busy"),
            agent: AgentKind::Claude,
            at: BASE + 71,
        }
    );
    assert_eq!(live(events).len(), 1);
    assert!(live(events)[0].quiet);
}

#[tokio::test(start_paused = true)]
async fn a_stale_observation_cannot_resurrect_an_idle_session() {
    let harness = start(BASE, vec![(key("done"), BASE)]);
    let events = &harness.events;
    let mut bus = events.subscribe();

    tokio::time::sleep(Duration::from_secs(181)).await;
    assert!(matches!(next(&mut bus).await, SessionEvent::Quiet { .. }));
    assert!(matches!(next(&mut bus).await, SessionEvent::Idle { .. }));
    assert!(live(events).is_empty());

    // A write at or before the last known write is old news.
    for at in [BASE, BASE - 10] {
        events.report_async(touched("done", 0, at, 0)).await;
    }
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    assert!(live(events).is_empty());

    // A genuinely newer write resumes the session without a new Started.
    events.report_async(touched("done", 0, BASE + 181, 0)).await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: Some(session_ref("done")),
            agent: AgentKind::Claude,
            at: BASE + 181,
            resumed: true,
        }
    );
    assert_eq!(live(events).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_indexed_report_for_a_recently_idle_session_resumes_it() {
    let harness = start(BASE, vec![(key("done"), BASE)]);
    let events = &harness.events;
    let mut bus = events.subscribe();

    tokio::time::sleep(Duration::from_secs(181)).await;
    assert!(matches!(next(&mut bus).await, SessionEvent::Quiet { .. }));
    assert!(matches!(next(&mut bus).await, SessionEvent::Idle { .. }));

    // A source-label reuse can mislabel a known identity as new. The
    // registry remembers the identity and publishes a resume, not Started.
    events
        .report_async(index("done", 0, BASE + 182, true, 5))
        .await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: Some(session_ref("done")),
            agent: AgentKind::Claude,
            at: BASE + 182,
            resumed: true,
        }
    );
}

#[tokio::test(start_paused = true)]
async fn events_carry_an_increasing_sequence_the_snapshot_matches() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();
    assert_eq!(events.snapshot(usize::MAX).seq, 0);

    for (index, name) in ["one", "two", "three"].iter().enumerate() {
        events
            .report_async(touched(name, 1, BASE + index as i64, 2))
            .await;
    }
    settle().await;

    for expected in 1..=3 {
        assert_eq!(bus.recv().await.unwrap().seq, expected);
    }
    assert_eq!(events.snapshot(usize::MAX).seq, 3);
    assert_eq!(events.current_seq(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_returns_the_requested_most_recent_limit() {
    let harness = start(
        BASE,
        vec![
            (key("oldest"), BASE - 20),
            (key("middle"), BASE - 10),
            (key("newest"), BASE - 1),
            // An equal epoch orders by the full identity.
            (key("also-middle"), BASE - 10),
        ],
    );
    let events = &harness.events;
    settle().await;

    let snapshot = events.snapshot(3);
    assert_eq!(
        snapshot
            .sessions
            .iter()
            .map(|session| session.session.session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["newest", "also-middle", "middle"]
    );
    assert_eq!(events.snapshot(0).sessions.len(), 0);
    assert_eq!(events.snapshot(usize::MAX).sessions.len(), 4);
}

#[tokio::test(start_paused = true)]
async fn a_late_subscriber_reads_the_seeded_set_from_the_snapshot_not_a_replay() {
    let harness = start(
        BASE,
        vec![
            (key("first"), BASE - 30),
            (key("second"), BASE - 5),
            (key("stale"), BASE - ACTIVE_SESSION_WINDOW_SECS),
            (SessionKey::new("native", "not-an-agent", "unknown"), BASE),
        ],
    );
    let events = &harness.events;
    settle().await;

    let mut late = events.subscribe();
    assert_eq!(late.try_recv().unwrap_err(), TryRecvError::Empty);
    assert_eq!(
        live(events),
        vec![
            LiveSession {
                session: session_ref("second"),
                agent: AgentKind::Claude,
                last_activity_at: BASE - 5,
                quiet: false,
                execution: None,
            },
            LiveSession {
                session: session_ref("first"),
                agent: AgentKind::Claude,
                last_activity_at: BASE - 30,
                // Seeded past the quiet window: the snapshot says so.
                quiet: true,
                execution: None,
            },
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn row_changed_publishes_an_updated_projection_trigger() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    let facets = UpdateFacets {
        analysis: true,
        ..UpdateFacets::default()
    };
    events.report(SyncObservation::RowChanged {
        session: key("row"),
        facets,
        at: BASE + 1,
    });

    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Updated {
            session: session_ref("row"),
            facets,
            at: BASE + 1,
        }
    );
    // Updated is a projection trigger, not liveness.
    assert!(live(events).is_empty());
}

#[tokio::test(start_paused = true)]
async fn removed_clears_the_live_entry_and_publishes() {
    let harness = start(BASE, vec![(key("gone"), BASE)]);
    let events = &harness.events;
    let mut bus = events.subscribe();

    events.report(sync_removed("gone", 0, RemovalReason::Deleted, 1));
    // The lifecycle scope narrates the decay first: a reader tracking
    // working state must not wait for a `Quiet` or `Idle` that the
    // registry can no longer publish for a dropped entry.
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Idle {
            session: session_ref("gone"),
            agent: AgentKind::Claude,
            at: BASE,
        }
    );
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Removed {
            session: Some(session_ref("gone")),
            reason: RemovalReason::Deleted,
        }
    );
    assert!(live(events).is_empty());

    // No deadline remains: the actor sleeps instead of waking for it.
    tokio::time::sleep(Duration::from_secs(200)).await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    events.report(SyncObservation::IndexChanged {
        reason: IndexChangeReason::Invalidated,
    });
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::IndexChanged {
            reason: IndexChangeReason::Invalidated,
        }
    );
}

#[tokio::test(start_paused = true)]
async fn removing_an_unknown_session_publishes_removed_only() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    events.report(sync_removed("never-seen", 1, RemovalReason::Purged, 7));

    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Removed {
            session: Some(session_ref("never-seen")),
            reason: RemovalReason::Purged,
        }
    );
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
}

#[tokio::test(start_paused = true)]
async fn a_removed_session_cannot_be_resurrected_by_a_stale_touch() {
    let harness = start(BASE, vec![(key("gone"), BASE)]);
    let events = &harness.events;
    let mut bus = events.subscribe();

    events.report(sync_removed("gone", 0, RemovalReason::Rejected, 6));
    assert!(matches!(next(&mut bus).await, SessionEvent::Idle { .. }));
    assert!(matches!(next(&mut bus).await, SessionEvent::Removed { .. }));

    // Any write attributed to the deleted incarnation is old news, whatever
    // its epoch: the row it describes no longer exists.
    for at in [BASE, BASE + 5] {
        events.report_async(touched("gone", 0, at, 5)).await;
    }
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    assert!(live(events).is_empty());

    // A rediscovered row is a new incarnation, and it starts fresh.
    events.report_async(touched("gone", 1, BASE + 5, 8)).await;
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { resumed: false, .. }
    ));
    assert_eq!(live(events).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_async_reporter_waits_for_capacity_and_loses_nothing() {
    let events = Arc::new(SessionEvents::default());
    let inbox = events.claim_actor().expect("a fresh bus has no actor yet");
    let producer_events = events.clone();
    // The actor is not running yet: the producer fills the inbox and waits.
    let producer = tokio::spawn(async move {
        for index in 0..1000 {
            producer_events
                .report_async(touched(&format!("s{index}"), 1, BASE, 2))
                .await;
        }
    });
    settle().await;
    assert!(!producer.is_finished(), "the producer waits for inbox room");
    assert_eq!(
        events.inbox.capacity(),
        0,
        "the inbox is full while nothing drains it"
    );

    let actor_events = events.clone();
    let source: Arc<dyn ReconcileSource> = Arc::new(Scripted::default());
    let clock = instant_clock(BASE, Arc::new(std::sync::atomic::AtomicI64::new(0)));
    tokio::spawn(async move {
        run(&actor_events, inbox, source, &clock).await;
    });
    wait_until(|| producer.is_finished()).await;
    wait_until(|| events.snapshot(usize::MAX).sessions.len() == 1000).await;
    assert_eq!(
        events.current_seq(),
        1000,
        "one Activity per fact, none lost"
    );
}

#[tokio::test(start_paused = true)]
async fn a_full_inbox_spills_sync_facts_without_loss() {
    let events = Arc::new(SessionEvents::default());
    let inbox = events.claim_actor().expect("a fresh bus has no actor yet");
    for index in 0..INBOX_CAPACITY {
        events
            .inbox
            .try_send(touched(&format!("s{index}"), 1, BASE, 2))
            .unwrap();
    }
    // The inbox is full. Three hundred row changes for one key fold into
    // one cell with merged facets and the latest epoch.
    for index in 0..300 {
        events.report(SyncObservation::RowChanged {
            session: key("row"),
            facets: if index % 2 == 0 {
                UpdateFacets {
                    analysis: true,
                    ..UpdateFacets::default()
                }
            } else {
                UpdateFacets {
                    usage: true,
                    ..UpdateFacets::default()
                }
            },
            at: BASE + index,
        });
    }
    {
        let spill = events.spill.lock().unwrap();
        assert_eq!(spill.rows.len(), 1);
        assert_eq!(
            spill.rows[&key("row")],
            (
                UpdateFacets {
                    analysis: true,
                    usage: true,
                    ..UpdateFacets::default()
                },
                BASE + 299
            )
        );
    }

    let mut bus = events.subscribe();
    let actor_events = events.clone();
    let source: Arc<dyn ReconcileSource> = Arc::new(Scripted::default());
    let clock = instant_clock(BASE, Arc::new(std::sync::atomic::AtomicI64::new(0)));
    tokio::spawn(async move {
        run(&actor_events, inbox, source, &clock).await;
    });
    settle().await;
    let updated = drain(&mut bus)
        .into_iter()
        .filter(|event| matches!(event, SessionEvent::Updated { .. }))
        .collect::<Vec<_>>();
    assert_eq!(
        updated,
        vec![SessionEvent::Updated {
            session: session_ref("row"),
            facets: UpdateFacets {
                analysis: true,
                usage: true,
                ..UpdateFacets::default()
            },
            at: BASE + 299,
        }]
    );
    assert!(events.spill.lock().unwrap().is_empty());
}

#[test]
fn a_spilled_keyed_removal_past_the_cap_becomes_a_broad_removal() {
    let mut spill = Spill::default();
    for index in 0..SPILL_KEY_CAP {
        spill.fold(sync_removed(
            &format!("s{index}"),
            1,
            RemovalReason::Deleted,
            index as u64,
        ));
    }
    assert_eq!(spill.removed.len(), SPILL_KEY_CAP);
    assert_eq!(spill.broad, None);

    // The 1025th distinct key does not fit: it degrades to a broad removal
    // at its revision, and the presence walk removes the row.
    spill.fold(sync_removed("overflow", 3, RemovalReason::Purged, 5_000));
    assert_eq!(spill.removed.len(), SPILL_KEY_CAP);
    assert_eq!(spill.broad, Some((RemovalReason::Purged, Revision(5_000))));

    // An older broad report never lowers the requirement.
    spill.fold(sync_removed("overflow-2", 1, RemovalReason::Deleted, 4_000));
    assert_eq!(spill.broad, Some((RemovalReason::Purged, Revision(5_000))));

    // A known key still folds by maximum, whatever the cap.
    spill.fold(sync_removed("s0", 2, RemovalReason::Rejected, 9_000));
    assert_eq!(
        spill.removed[&key("s0")],
        RemovedCell {
            incarnation: Incarnation(2),
            reason: RemovalReason::Rejected,
            revision: Revision(9_000),
        }
    );
}

#[test]
fn a_spilled_row_change_past_the_cap_becomes_one_list_refetch() {
    let mut spill = Spill::default();
    let facets = UpdateFacets {
        title: true,
        ..UpdateFacets::default()
    };
    for index in 0..SPILL_KEY_CAP {
        spill.fold(SyncObservation::RowChanged {
            session: key(&format!("s{index}")),
            facets,
            at: BASE,
        });
    }
    assert!(spill.index_changed.is_empty());
    for index in 0..3 {
        spill.fold(SyncObservation::RowChanged {
            session: key(&format!("overflow{index}")),
            facets,
            at: BASE,
        });
    }
    assert_eq!(spill.rows.len(), SPILL_KEY_CAP);
    assert_eq!(
        spill.index_changed.into_iter().collect::<Vec<_>>(),
        vec![IndexChangeReason::Invalidated],
        "three overflowing rows become one refetch"
    );
}

#[test]
fn sync_observation_converts_field_for_field() {
    let facets = UpdateFacets {
        checks: true,
        ..UpdateFacets::default()
    };
    assert_eq!(
        Observation::from(SyncObservation::RowChanged {
            session: key("a"),
            facets,
            at: 7,
        }),
        Observation::RowChanged {
            session: key("a"),
            facets,
            at: 7,
        }
    );
    assert_eq!(
        Observation::from(sync_removed("b", 4, RemovalReason::Purged, 9)),
        removed("b", 4, RemovalReason::Purged, 9)
    );
    assert_eq!(
        Observation::from(SyncObservation::Removed {
            scope: RemovalScope::Broad,
            reason: RemovalReason::Deleted,
            revision: Revision(11),
        }),
        broad(RemovalReason::Deleted, 11)
    );
    assert_eq!(
        Observation::from(SyncObservation::IndexChanged {
            reason: IndexChangeReason::ScanPass,
        }),
        Observation::IndexChanged {
            reason: IndexChangeReason::ScanPass,
        }
    );
}

#[tokio::test(start_paused = true)]
async fn a_closed_inbox_drops_with_a_log_and_never_blocks() {
    let events = SessionEvents::default();
    // Taking and dropping the receiver closes the inbox, as shutdown does.
    drop(events.claim_actor());
    events.report(sync_removed("late", 1, RemovalReason::Deleted, 1));
    assert!(
        events.spill.lock().unwrap().is_empty(),
        "a closed inbox never spills"
    );
    // The async path returns at once instead of waiting for room.
    tokio::time::timeout(
        Duration::from_secs(1),
        events.report_async(touched("late", 1, BASE, 1)),
    )
    .await
    .expect("a closed inbox does not wait");
}

#[tokio::test(start_paused = true)]
async fn concurrent_sync_reporters_fold_into_one_spill_without_loss() {
    let events = Arc::new(SessionEvents::default());
    let inbox = events.claim_actor().expect("a fresh bus has no actor yet");
    for index in 0..INBOX_CAPACITY {
        events
            .inbox
            .try_send(touched(&format!("s{index}"), 1, BASE, 2))
            .unwrap();
    }
    // Four analysis workers report at once while the inbox is full, as
    // the shell's worker pool does.
    let workers = (0..4)
        .map(|worker| {
            let events = events.clone();
            std::thread::spawn(move || {
                for index in 0..100 {
                    events.report(SyncObservation::RowChanged {
                        session: key(&format!("row{}", index % 10)),
                        facets: UpdateFacets {
                            analysis: worker % 2 == 0,
                            checks: worker % 2 == 1,
                            ..UpdateFacets::default()
                        },
                        at: BASE + worker,
                    });
                    events.report(sync_removed(
                        &format!("gone{}", index % 5),
                        worker as u64,
                        RemovalReason::Deleted,
                        100 + worker as u64,
                    ));
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    {
        let spill = events.spill.lock().unwrap();
        assert_eq!(spill.rows.len(), 10);
        for (facets, at) in spill.rows.values() {
            assert!(
                facets.analysis && facets.checks,
                "both workers' facets merged"
            );
            assert_eq!(*at, BASE + 3, "the latest epoch wins");
        }
        assert_eq!(spill.removed.len(), 5);
        for cell in spill.removed.values() {
            assert_eq!(cell.incarnation, Incarnation(3));
            assert_eq!(cell.revision, Revision(103));
        }
    }

    let mut bus = events.subscribe();
    let actor_events = events.clone();
    let source: Arc<dyn ReconcileSource> = Arc::new(Scripted::default());
    let clock = instant_clock(BASE, Arc::new(std::sync::atomic::AtomicI64::new(0)));
    tokio::spawn(async move {
        run(&actor_events, inbox, source, &clock).await;
    });
    settle().await;
    let events_seen = drain(&mut bus);
    assert_eq!(
        events_seen
            .iter()
            .filter(|event| matches!(event, SessionEvent::Updated { .. }))
            .count(),
        10
    );
    assert_eq!(
        events_seen
            .iter()
            .filter(|event| matches!(event, SessionEvent::Removed { .. }))
            .count(),
        5
    );
}

#[tokio::test(start_paused = true)]
async fn a_round_services_every_ready_source_once() {
    let source = Arc::new(Scripted::default());
    source.set_rows(
        vec![(key("waiting"), 1, BASE - 5), (key("ticking"), 0, BASE)],
        20,
    );
    let harness = start_with(BASE, vec![(key("ticking"), BASE)], source.clone());
    let events = &harness.events;
    let mut bus = events.subscribe();

    // A deferred key makes the actor ask for a page; the gate holds the
    // computed page so the test can line the other sources up behind it.
    source.hold();
    events.report_async(broad(RemovalReason::Purged, 10)).await;
    events
        .report_async(index("waiting", 1, BASE - 5, false, 5))
        .await;
    wait_until(|| source.requests().len() == 1).await;
    assert_eq!(
        drain(&mut bus),
        vec![SessionEvent::Removed {
            session: None,
            reason: RemovalReason::Purged,
        }]
    );
    let rounds_before = harness.rounds();

    // Without yielding: expiry is due (the clock moved), the page is
    // complete (the gate opens), a sync fact is spilled, and the inbox is
    // ready. Nothing runs until the test yields.
    harness.offset.store(31, Ordering::Relaxed);
    for index in 0..INBOX_CAPACITY {
        harness.push(touched(&format!("fill{index}"), 1, BASE, 30));
    }
    events.report(sync_removed("spilled", 1, RemovalReason::Deleted, 25));
    source.release();
    source.spin_until_completed(1);
    assert_eq!(
        harness.rounds(),
        rounds_before,
        "nothing ran without a yield"
    );
    tokio::task::yield_now().await;

    // One round: expiry, then the page, then the spill, then the inbox.
    let after_round = drain(&mut bus);
    assert_eq!(
        after_round[0],
        SessionEvent::Quiet {
            session: session_ref("ticking"),
            agent: AgentKind::Claude,
            at: BASE + 31,
        },
        "expiry comes first"
    );
    assert_eq!(
        after_round[1],
        SessionEvent::Activity {
            session: Some(session_ref("waiting")),
            agent: AgentKind::Claude,
            at: BASE - 5,
            resumed: false,
        },
        "the finished page is applied right after expiry"
    );
    assert_eq!(
        after_round[2],
        SessionEvent::Removed {
            session: Some(session_ref("spilled")),
            reason: RemovalReason::Deleted,
        },
        "the spill follows the page"
    );
    assert!(
        after_round[3..]
            .iter()
            .all(|event| matches!(event, SessionEvent::Activity { .. })),
        "then the inbox batch"
    );
    assert_eq!(
        after_round.len() - 3,
        DRAIN_BATCH,
        "one inbox batch per round"
    );
    assert_eq!(harness.rounds(), rounds_before + 1);
}

#[tokio::test(start_paused = true)]
async fn expiry_drains_a_backlog_in_ceil_n_over_batch_rounds_while_the_inbox_keeps_moving() {
    let events = Arc::new(SessionEvents::default());
    let inbox = events.claim_actor().expect("a fresh bus has no actor yet");
    // A thousand sessions seeded just past the quiet window: every deadline
    // is due at the first round.
    events.seed(
        (0..1000)
            .map(|index| presence(&format!("due{index}"), 0, BASE - QUIET_WINDOW_SECS - 1))
            .collect(),
        Revision(0),
        BASE - 2,
    );
    for index in 0..INBOX_CAPACITY {
        events
            .inbox
            .try_send(touched(&format!("fact{index}"), 1, BASE, 2))
            .unwrap();
    }
    let actor_events = events.clone();
    let source: Arc<dyn ReconcileSource> = Arc::new(Scripted::default());
    let clock = instant_clock(BASE, Arc::new(std::sync::atomic::AtomicI64::new(0)));
    tokio::spawn(async move {
        run(&actor_events, inbox, source, &clock).await;
    });

    let quiet_count = || {
        events
            .snapshot(usize::MAX)
            .sessions
            .iter()
            .filter(|session| session.quiet)
            .count()
    };
    let admitted = || events.snapshot(usize::MAX).sessions.len() - 1000;
    for round in 1..=4 {
        tokio::task::yield_now().await;
        assert_eq!(events.rounds.load(Ordering::Relaxed), round);
        assert_eq!(quiet_count(), (round as usize * EXPIRE_BATCH).min(1000));
        assert_eq!(
            admitted(),
            round as usize * DRAIN_BATCH,
            "facts move every round"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_spilled_removal_is_applied_in_the_next_round_under_a_ready_inbox() {
    let events = Arc::new(SessionEvents::default());
    let inbox = events.claim_actor().expect("a fresh bus has no actor yet");
    events.seed(vec![presence("victim", 0, BASE)], Revision(0), BASE);
    for index in 0..INBOX_CAPACITY {
        events
            .inbox
            .try_send(touched(&format!("fact{index}"), 1, BASE, 2))
            .unwrap();
    }
    // The inbox is full and stays ready for four rounds. The removal spills.
    events.report(sync_removed("victim", 0, RemovalReason::Deleted, 3));
    let mut bus = events.subscribe();
    let actor_events = events.clone();
    let source: Arc<dyn ReconcileSource> = Arc::new(Scripted::default());
    let clock = instant_clock(BASE, Arc::new(std::sync::atomic::AtomicI64::new(0)));
    tokio::spawn(async move {
        run(&actor_events, inbox, source, &clock).await;
    });

    tokio::task::yield_now().await;
    assert_eq!(events.rounds.load(Ordering::Relaxed), 1);
    let first_round = drain(&mut bus);
    assert_eq!(
        &first_round[..2],
        &[
            SessionEvent::Idle {
                session: session_ref("victim"),
                agent: AgentKind::Claude,
                at: BASE,
            },
            SessionEvent::Removed {
                session: Some(session_ref("victim")),
                reason: RemovalReason::Deleted,
            },
        ],
        "the spill is applied before the round's inbox batch"
    );
    assert_eq!(first_round.len(), 2 + DRAIN_BATCH);
}

#[tokio::test(start_paused = true)]
async fn an_iteration_applies_at_most_drain_weight_session_facts_and_carries_the_rest() {
    let events = Arc::new(SessionEvents::default());
    let inbox = events.claim_actor().expect("a fresh bus has no actor yet");
    // Three chunks of 200 sessions: 600 session-level facts in 3 observations.
    for chunk in 0..3 {
        events
            .inbox
            .try_send(Observation::Indexed {
                sessions: (0..200)
                    .map(|index| indexed(&format!("c{chunk}-{index}"), 1, BASE, true))
                    .collect(),
                revision: Revision(2),
            })
            .unwrap();
    }
    let actor_events = events.clone();
    let source: Arc<dyn ReconcileSource> = Arc::new(Scripted::default());
    let clock = instant_clock(BASE, Arc::new(std::sync::atomic::AtomicI64::new(0)));
    tokio::spawn(async move {
        run(&actor_events, inbox, source, &clock).await;
    });

    tokio::task::yield_now().await;
    assert_eq!(events.rounds.load(Ordering::Relaxed), 1);
    assert_eq!(
        events.snapshot(usize::MAX).sessions.len(),
        DRAIN_WEIGHT,
        "the round stops inside the second chunk"
    );
    tokio::task::yield_now().await;
    assert_eq!(events.snapshot(usize::MAX).sessions.len(), 2 * DRAIN_WEIGHT);
    tokio::task::yield_now().await;
    assert_eq!(
        events.snapshot(usize::MAX).sessions.len(),
        600,
        "nothing is lost"
    );
    assert_eq!(events.current_seq(), 600);
}

#[test]
fn the_registry_never_publishes_resync() {
    let source = include_str!("../session_lifecycle.rs");
    assert!(
        !source.contains("SessionEvent::Resync"),
        "only the projection bridge publishes Resync"
    );
}

/// Only the pinned producers report through the non-waiting path, and only
/// the scan task reports through the waiting one. A new sync caller is a
/// review item; a sync establishing fact is a compile error.
#[test]
fn sync_report_callers_are_the_pinned_list() {
    let sync_callers = [
        ("commands.rs", include_str!("../commands.rs"), 5),
        (
            "insights_worker.rs",
            include_str!("../insights_worker.rs"),
            1,
        ),
        ("retention.rs", include_str!("../retention.rs"), 1),
        (
            "runtime_pricing.rs",
            include_str!("../runtime_pricing.rs"),
            1,
        ),
    ];
    for (name, source, expected) in sync_callers {
        let production = source.split("#[cfg(test)]").next().unwrap_or(source);
        assert_eq!(
            production.matches("session_lifecycle::report(").count(),
            expected,
            "{name} sync report call count"
        );
        assert!(
            !production.contains("report_async("),
            "{name} must not use the waiting reporter"
        );
    }
    let async_callers = [
        ("scan/mod.rs", include_str!("../scan/mod.rs")),
        ("scan/scoped.rs", include_str!("../scan/scoped.rs")),
    ];
    for (name, source) in async_callers {
        let production = source.split("#[cfg(test)]").next().unwrap_or(source);
        if name == "scan/mod.rs" {
            assert!(production.contains("session_lifecycle::report_async("));
        } else {
            for reporter in [
                "report_discovery",
                "report_row_changes",
                "report_rejected",
                "report_membership_changed",
            ] {
                assert!(
                    production.contains(&format!("super::{reporter}(")),
                    "{name} uses {reporter}"
                );
                let body = function_body(include_str!("../scan/mod.rs"), reporter);
                assert!(
                    body.contains("session_lifecycle::report_async("),
                    "{reporter} waits for capacity"
                );
            }
        }
        assert!(
            !production.contains("session_lifecycle::report("),
            "{name} must not use the non-waiting reporter"
        );
    }
}

#[test]
fn events_serialize_with_a_kind_tag_and_camel_case_fields() {
    let event = SessionEvent::Started {
        session: session_ref("abc"),
        agent: AgentKind::Claude,
        at: 7,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "started");
    assert_eq!(json["agent"], "claude-code");
    assert_eq!(json["session"]["sessionId"], "abc");
    assert_eq!(json["session"]["environmentKey"], "native");

    let activity = SessionEvent::Activity {
        session: None,
        agent: AgentKind::Codex,
        at: 8,
        resumed: true,
    };
    let json = serde_json::to_value(&activity).unwrap();
    assert_eq!(json["kind"], "activity");
    assert!(json["session"].is_null());
    assert_eq!(json["resumed"], true);

    let quiet = SessionEvent::Quiet {
        session: session_ref("abc"),
        agent: AgentKind::Claude,
        at: 9,
    };
    assert_eq!(serde_json::to_value(&quiet).unwrap()["kind"], "quiet");

    let updated = SessionEvent::Updated {
        session: session_ref("abc"),
        facets: UpdateFacets {
            title: true,
            ..UpdateFacets::default()
        },
        at: 10,
    };
    let json = serde_json::to_value(&updated).unwrap();
    assert_eq!(json["kind"], "updated");
    assert_eq!(json["facets"]["title"], true);
    assert_eq!(json["facets"]["usage"], false);

    let removed = SessionEvent::Removed {
        session: None,
        reason: RemovalReason::Purged,
    };
    let json = serde_json::to_value(&removed).unwrap();
    assert_eq!(json["kind"], "removed");
    assert_eq!(json["reason"], "purged");

    let reconciled = SessionEvent::Removed {
        session: Some(session_ref("abc")),
        reason: RemovalReason::Reconciled,
    };
    assert_eq!(
        serde_json::to_value(&reconciled).unwrap()["reason"],
        "reconciled"
    );

    let cleared = SessionEvent::AnonymousCleared {
        agent: AgentKind::Codex,
        at: 11,
        cause: AnonymousClearCause::Resolved,
    };
    let json = serde_json::to_value(&cleared).unwrap();
    assert_eq!(json["kind"], "anonymous_cleared");
    assert_eq!(json["agent"], "codex");
    assert_eq!(json["at"], 11);
    assert_eq!(json["cause"], "resolved");
    let expired = SessionEvent::AnonymousCleared {
        agent: AgentKind::Codex,
        at: 12,
        cause: AnonymousClearCause::Expired,
    };
    assert_eq!(serde_json::to_value(&expired).unwrap()["cause"], "expired");

    let snapshot = LiveSnapshot {
        seq: 3,
        working: 0,
        total: 0,
        sweep: Vec::new(),
        sessions: Vec::new(),
        anonymous: vec![LiveAnonymous {
            agent: AgentKind::Codex,
            last_activity_at: 13,
        }],
    };
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(json["anonymous"][0]["agent"], "codex");
    assert_eq!(json["anonymous"][0]["lastActivityAt"], 13);
}

#[test]
fn the_lifecycle_envelope_flattens_the_event_beside_the_sequence() {
    let event = SessionEvent::Resync;
    let envelope = LifecycleEnvelope {
        seq: 41,
        event: &event,
        aggregate: None,
    };
    let json = serde_json::to_value(&envelope).unwrap();
    assert_eq!(json["seq"], 41);
    assert_eq!(json["kind"], "resync");

    let event = SessionEvent::Activity {
        session: Some(session_ref("abc")),
        agent: AgentKind::Claude,
        at: 7,
        resumed: false,
    };
    let envelope = LifecycleEnvelope {
        seq: 42,
        event: &event,
        aggregate: None,
    };
    let json = serde_json::to_value(&envelope).unwrap();
    assert_eq!(json["seq"], 42);
    assert_eq!(json["kind"], "activity");
    assert_eq!(json["session"]["sessionId"], "abc");
}

/// The body of one `fn name(` in `source`, up to its closing brace at
/// column zero.
fn function_body(source: &str, name: &str) -> String {
    let source = source.replace("\r\n", "\n");
    let start = source
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("{name} exists"));
    let body = &source[start..];
    let end = body.find("\n}\n").expect("the function closes");
    body[..end].to_owned()
}

#[test]
fn source_contract_body_accepts_both_checkout_line_endings() {
    let source = "fn sample() {\n    report();\n}\nfn other() {}\n";
    let expected = "fn sample() {\n    report();";
    assert_eq!(function_body(source, "sample"), expected);
    assert_eq!(
        function_body(&source.replace('\n', "\r\n"), "sample"),
        expected
    );
}

/// The producer seams cannot run without a Tauri app, which the crate's
/// tests never build. Their fact shapes are pinned at the source instead:
/// each broad-purge producer reports a broad removal with the revision the
/// store returned, then one invalidation, in that order and before any
/// pass is asked for.
#[test]
fn clear_local_index_reports_a_broad_deletion_before_the_pass() {
    let body = function_body(include_str!("../commands.rs"), "clear_local_index");
    let removal = body.find("RemovalScope::Broad").expect("a broad removal");
    assert!(body[..removal].contains("let (removed, revision) = run_blocking("));
    assert!(body[removal..].contains("RemovalReason::Deleted,\n            revision,"));
    let invalidated = body
        .find("IndexChangeReason::Invalidated")
        .expect("one invalidation");
    let pass = body
        .find("ScanTrigger::IndexCleared")
        .expect("the refill pass");
    assert!(
        removal < invalidated && invalidated < pass,
        "removal, invalidation, then the pass"
    );
}

#[test]
fn opt_out_reports_a_broad_purge_then_invalidated() {
    let body = function_body(include_str!("../commands.rs"), "set_repository_enabled");
    let purge = body.find("RemovalScope::Broad").expect("a broad removal");
    assert!(
        body[..purge].contains("if !enabled {"),
        "only disabling purges rows"
    );
    assert!(
        body[..purge].contains("store.revision()"),
        "the revision is read after the purge"
    );
    assert!(body[purge..].contains("RemovalReason::Purged,\n                revision,"));
    let invalidated = body
        .find("IndexChangeReason::Invalidated")
        .expect("one invalidation");
    assert!(purge < invalidated);
}

#[test]
fn retention_reports_a_broad_purge_with_a_revision() {
    let source = include_str!("../retention.rs");
    let note = function_body(source, "note_removed");
    assert!(note.contains("removed: usize, revision: Revision"));
    assert!(note.contains("RemovalScope::Broad"));
    assert!(note.contains("RemovalReason::Purged,\n            revision,"));
    let cleanup = function_body(source, "cleanup");
    assert!(
        cleanup.contains("Ok((removed, revision)) => note_removed(app, removed, revision)"),
        "the retention commit's own revision travels with the purge"
    );
    let settings = function_body(include_str!("../commands.rs"), "set_settings");
    assert!(
        settings.contains("let revision = store.revision();\n        crate::retention::note_removed(&database_app, removed, revision);"),
        "the settings path reads the revision after the commit"
    );
}

#[test]
fn a_rejected_row_reports_the_deleted_incarnation() {
    let scan = include_str!("../scan/mod.rs");
    let rejected = function_body(scan, "report_rejected");
    assert!(
        rejected.contains("membership_reports(described, &[(key.clone(), incarnation, revision)])")
    );
    let membership = function_body(scan, "membership_reports");
    assert!(membership.contains("RemovalScope::One(key.clone(), *incarnation)"));
    assert!(membership.contains("RemovalReason::Rejected"));
    assert!(membership.contains("revision: *revision"));
    for (name, source) in [
        ("scan/mod.rs", scan),
        ("scan/scoped.rs", include_str!("../scan/scoped.rs")),
    ] {
        let production = source.split("#[cfg(test)]").next().unwrap_or(source);
        assert!(
            production.contains("if let Some((incarnation, revision)) = removed {"),
            "{name} reports only a delete that found a row, with its evidence"
        );
        assert!(
            production
                .contains("report_rejected(app, &described, key, incarnation, revision).await"),
            "{name} routes the rejection through the shared reporter"
        );
    }
}

/// Seed through the production boundary and start the actor with a controlled clock.
pub(crate) fn start_seeded(
    events: Arc<SessionEvents>,
    source: Arc<dyn ReconcileSource>,
    epoch: i64,
) -> tokio::task::JoinHandle<()> {
    seed(&events, source.as_ref(), epoch);
    let inbox = events.claim_actor().unwrap();
    let clock = instant_clock(epoch, Arc::new(std::sync::atomic::AtomicI64::new(0)));
    tokio::spawn(async move { run(&events, inbox, source, &clock).await })
}
