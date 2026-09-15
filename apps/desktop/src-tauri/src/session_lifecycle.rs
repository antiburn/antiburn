//! Session lifecycle: one actor decides which sessions are live, and one
//! broadcast bus tells every reader.
//!
//! Producers report typed facts as an [`Observation`]: a watcher burst
//! touched a session, a pass indexed some sessions, a worker changed a row,
//! or the store removed one. The actor is the only sender on the bus. It
//! turns observations into [`SessionEvent`]s, keeps the registry of live
//! sessions, publishes `Quiet` when a session crosses [`QUIET_WINDOW_SECS`]
//! without a write, and `Idle` when it crosses
//! [`ACTIVE_SESSION_WINDOW_SECS`].
//!
//! Every published event carries a registry sequence number. A snapshot
//! carries the sequence at the time of the clone, so a subscriber can take
//! a snapshot, then apply only the deltas with a higher sequence. `Resync`
//! tells a subscriber that events were lost and the snapshot must be read
//! again.
//!
//! A `Touched` observation arrives at burst classification, before any
//! per-session floor or describe. That is what makes the bus faster than the
//! row-refresh path: a transcript write reaches the bus in about the
//! watcher's quiet window.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS;
use antiburn_local::model::AgentKind;
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokio::sync::{Notify, broadcast};
use tokio::time::Instant;

use crate::store::{SessionKey, Store};

/// How many events a subscriber can fall behind before it is told it lagged.
pub const BUS_CAPACITY: usize = 64;

/// How many observations can queue before `report` coalesces into an
/// already queued observation. An observation that cannot coalesce is
/// dropped, and the actor publishes `Resync` so readers reconcile from the
/// snapshot.
const INBOX_CAPACITY: usize = 256;

/// Ceiling on how many sessions one coalesced `Indexed` observation can
/// carry. Past this, the report is dropped and `Resync` reconciles.
const COALESCED_INDEX_CAP: usize = 1024;

/// How many recently idled sessions the registry remembers, so a later
/// write is a resume and a stale observation cannot resurrect one.
const RECENT_IDLE_CAP: usize = 256;

/// Slack added past a session's computed deadline, so the actor never wakes a
/// moment early and finds the session still (barely) active.
const EXPIRY_SLACK_SECS: i64 = 1;

/// How long a session goes without a write before the bus calls it quiet.
/// The meters animate from `Activity` to `Quiet`. The session stays active,
/// for the session list, until [`ACTIVE_SESSION_WINDOW_SECS`].
pub const QUIET_WINDOW_SECS: i64 = 30;

/// How many sessions a snapshot returns when the caller names no limit.
pub const DEFAULT_SNAPSHOT_LIMIT: usize = 128;

/// The identity of one session, as the webview receives it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRef {
    pub environment_key: String,
    pub agent: String,
    pub session_id: String,
}

impl From<&SessionKey> for SessionRef {
    fn from(key: &SessionKey) -> Self {
        Self {
            environment_key: key.environment_key.clone(),
            agent: key.agent.clone(),
            session_id: key.session_id.clone(),
        }
    }
}

/// Which parts of a session row changed. Producers set facets; the
/// projection layer decides what to reload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFacets {
    pub metadata: bool,
    pub title: bool,
    pub analysis: bool,
    pub usage: bool,
    pub checks: bool,
    pub limits: bool,
}

impl UpdateFacets {
    /// Fold another report's facets into this one.
    pub fn merge(&mut self, other: UpdateFacets) {
        self.metadata |= other.metadata;
        self.title |= other.title;
        self.analysis |= other.analysis;
        self.usage |= other.usage;
        self.checks |= other.checks;
        self.limits |= other.limits;
    }
}

/// Why a session left the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalReason {
    /// The reader or a worker deleted the row.
    Deleted,
    /// Retention purged the row.
    Purged,
    /// The scan gate rejected the source and removed the row.
    Rejected,
}

/// Why the session list as a whole changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexChangeReason {
    /// A scan pass changed list membership.
    ScanPass,
    /// A broad invalidation: readers refetch list data.
    Invalidated,
}

/// One transition on the bus. Every subscriber sees every event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    /// The store indexed this session for the first time.
    Started {
        session: SessionRef,
        agent: AgentKind,
        at: i64,
    },
    /// A write to this session's source was observed.
    /// `session` is `None` when the path is under an agent root but the
    /// store has not indexed the session yet. `resumed` is true when the
    /// write follows `Quiet` or `Idle`.
    Activity {
        session: Option<SessionRef>,
        agent: AgentKind,
        at: i64,
        resumed: bool,
    },
    /// The session crossed [`QUIET_WINDOW_SECS`] without a write. It is
    /// still active. A later write publishes `Activity` again.
    Quiet {
        session: SessionRef,
        agent: AgentKind,
        at: i64,
    },
    /// The session crossed [`ACTIVE_SESSION_WINDOW_SECS`] without a write.
    Idle {
        session: SessionRef,
        agent: AgentKind,
        at: i64,
    },
    /// A session row changed. This is a projection trigger, not a row
    /// payload: the projection layer loads the enriched row.
    Updated {
        session: SessionRef,
        facets: UpdateFacets,
        at: i64,
    },
    /// A session left the store. `session` is `None` for a broad purge.
    Removed {
        session: Option<SessionRef>,
        reason: RemovalReason,
    },
    /// List membership changed in a way no single row names.
    IndexChanged { reason: IndexChangeReason },
    /// Events were lost. Readers re-read the snapshot; the envelope's
    /// sequence tells them where the stream resumes.
    Resync,
}

/// One bus message: the event and the registry sequence it was published
/// at. This is the internal shape; the webview receives
/// [`LifecycleEnvelope`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sequenced {
    pub seq: u64,
    pub event: SessionEvent,
}

/// The `session:lifecycle` payload, emitted by the projection worker.
/// Tauri is transport: this shape can change independently of the internal
/// bus type.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleEnvelope<'a> {
    pub seq: u64,
    #[serde(flatten)]
    pub event: &'a SessionEvent,
}

/// One session a scan pass indexed, with the identity-level `is_new` flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedSession {
    pub key: SessionKey,
    pub agent: AgentKind,
    pub at: i64,
    pub is_new: bool,
}

/// What a producer saw. Only the actor turns these into events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observation {
    /// A watcher burst touched a path that maps to this session or agent.
    Touched {
        session: Option<SessionKey>,
        agent: AgentKind,
        at: i64,
    },
    /// A scan pass upserted these sessions with these activity epochs.
    Indexed { sessions: Vec<IndexedSession> },
    /// A worker changed parts of one session's row.
    RowChanged {
        session: SessionKey,
        facets: UpdateFacets,
        at: i64,
    },
    /// A session left the store, or many did when `session` is `None`.
    Removed {
        session: Option<SessionKey>,
        reason: RemovalReason,
    },
    /// List membership changed in a way no single row names.
    IndexChanged { reason: IndexChangeReason },
}

/// One live session, as the snapshot command returns it. `quiet` mirrors
/// the registry's own state, so a reader never derives lifecycle windows
/// from timestamps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSession {
    pub session: SessionRef,
    pub agent: AgentKind,
    pub last_activity_at: i64,
    pub quiet: bool,
}

/// A bounded, versioned view of the live registry. `seq` is the sequence of
/// the last event whose effect the snapshot includes: a subscriber applies
/// only deltas with a higher sequence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSnapshot {
    pub seq: u64,
    pub sessions: Vec<LiveSession>,
}

#[derive(Clone, Copy, Debug)]
struct LiveEntry {
    agent: AgentKind,
    last_activity_at: i64,
    /// `Quiet` was published for `last_activity_at`. A newer write clears it.
    quiet_published: bool,
}

impl LiveEntry {
    /// The next moment this entry has something to publish.
    fn deadline(&self) -> i64 {
        if self.quiet_published {
            self.last_activity_at + ACTIVE_SESSION_WINDOW_SECS
        } else {
            self.last_activity_at + QUIET_WINDOW_SECS
        }
    }
}

/// One remembered idle session: what its last write was, and when it idled.
#[derive(Clone, Copy, Debug)]
struct RecentIdle {
    last_activity_at: i64,
    idled_at: i64,
}

/// What one touch did to the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Touch {
    /// The session was not live before. `resumed` is true when it idled
    /// recently.
    Added { resumed: bool },
    /// The session was live, and `at` moved its last activity later.
    /// `resumed` is true when the session was quiet.
    Advanced { resumed: bool },
    /// The session was live with an epoch at least as late as `at`.
    Unchanged,
    /// The observation is older than the window, or older than the last
    /// write of a session that already idled. It changes nothing.
    Stale,
}

/// The lifecycle state one lock guards: the live map, its deadline index,
/// the recent-idle memory, keyless activity, and the event sequence.
#[derive(Default)]
struct Registry {
    seq: u64,
    live: HashMap<SessionKey, LiveEntry>,
    /// One entry per live session, keyed by its current deadline. Touches
    /// move entries, so the set never accumulates stale rows.
    deadlines: BTreeSet<(i64, SessionKey)>,
    /// Recently idled sessions, bounded by [`RECENT_IDLE_CAP`].
    recently_idle: HashMap<SessionKey, RecentIdle>,
    /// Last published keyless activity per agent. Bounded by the number of
    /// agent kinds.
    keyless: HashMap<AgentKind, i64>,
}

impl Registry {
    /// Record activity for `key`. An older epoch changes nothing.
    fn touch(&mut self, key: &SessionKey, agent: AgentKind, at: i64, now: i64) -> Touch {
        if now - at >= ACTIVE_SESSION_WINDOW_SECS {
            return Touch::Stale;
        }
        match self.live.get_mut(key) {
            Some(entry) if entry.last_activity_at >= at => Touch::Unchanged,
            Some(entry) => {
                let resumed = entry.quiet_published;
                self.deadlines.remove(&(entry.deadline(), key.clone()));
                entry.last_activity_at = at;
                entry.agent = agent;
                entry.quiet_published = false;
                self.deadlines.insert((entry.deadline(), key.clone()));
                Touch::Advanced { resumed }
            }
            None => {
                if let Some(idle) = self.recently_idle.get(key) {
                    // A write at or before the last known write is an old
                    // observation. It must not resurrect the session.
                    if idle.last_activity_at >= at {
                        return Touch::Stale;
                    }
                }
                let resumed = self.recently_idle.remove(key).is_some();
                self.insert_live(
                    key.clone(),
                    LiveEntry {
                        agent,
                        last_activity_at: at,
                        quiet_published: false,
                    },
                );
                Touch::Added { resumed }
            }
        }
    }

    fn insert_live(&mut self, key: SessionKey, entry: LiveEntry) {
        self.deadlines.insert((entry.deadline(), key.clone()));
        self.live.insert(key, entry);
    }

    fn remove_live(&mut self, key: &SessionKey) -> Option<LiveEntry> {
        let entry = self.live.remove(key)?;
        self.deadlines.remove(&(entry.deadline(), key.clone()));
        Some(entry)
    }

    /// Remember an idled session, evicting the oldest past the cap.
    fn remember_idle(&mut self, key: SessionKey, last_activity_at: i64, idled_at: i64) {
        if self.recently_idle.len() >= RECENT_IDLE_CAP {
            let oldest = self
                .recently_idle
                .iter()
                .min_by_key(|(key, idle)| (idle.idled_at, (*key).clone()))
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.recently_idle.remove(&oldest);
            }
        }
        self.recently_idle.insert(
            key,
            RecentIdle {
                last_activity_at,
                idled_at,
            },
        );
    }
}

/// The bounded inbox `report` fills and the actor drains. A full queue
/// coalesces same-identity observations; one that cannot coalesce is
/// dropped and `overflowed` asks the actor to publish `Resync`.
#[derive(Default)]
struct Inbox {
    queue: VecDeque<Observation>,
    overflowed: bool,
}

/// The bus and the actor's state, held in Tauri managed state.
///
/// `subscribe` gives a reader every event from now on. `report` hands the
/// actor an observation. `snapshot` is the versioned view a late subscriber
/// reads instead of a replay.
pub struct SessionEvents {
    inbox: Mutex<Inbox>,
    inbox_wake: Notify,
    bus: broadcast::Sender<Sequenced>,
    registry: Mutex<Registry>,
    /// Set once by [`spawn`], so a second actor cannot start.
    actor_claimed: AtomicBool,
}

impl Default for SessionEvents {
    fn default() -> Self {
        let (bus, _) = broadcast::channel(BUS_CAPACITY);
        Self {
            inbox: Mutex::new(Inbox::default()),
            inbox_wake: Notify::new(),
            bus,
            registry: Mutex::new(Registry::default()),
            actor_claimed: AtomicBool::new(false),
        }
    }
}

impl SessionEvents {
    /// A receiver that sees every event published after this call.
    pub fn subscribe(&self) -> broadcast::Receiver<Sequenced> {
        self.bus.subscribe()
    }

    /// Hand the actor one observation. A full queue coalesces by identity;
    /// an observation that cannot coalesce is dropped and the actor
    /// publishes `Resync` so readers reconcile from the snapshot.
    pub fn report(&self, observation: Observation) {
        {
            let mut inbox = self
                .inbox
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if inbox.queue.len() < INBOX_CAPACITY {
                inbox.queue.push_back(observation);
            } else if !coalesce(&mut inbox.queue, observation) {
                inbox.overflowed = true;
                ::tracing::warn!(event = "session_lifecycle_inbox_overflow");
            }
        }
        self.inbox_wake.notify_one();
    }

    /// The most recent live sessions, versioned and bounded to `limit`.
    /// The clone happens under the lock; sorting happens outside it.
    pub fn snapshot(&self, limit: usize) -> LiveSnapshot {
        let (seq, mut sessions) = {
            let registry = self
                .registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let sessions = registry
                .live
                .iter()
                .map(|(key, entry)| LiveSession {
                    session: SessionRef::from(key),
                    agent: entry.agent,
                    last_activity_at: entry.last_activity_at,
                    quiet: entry.quiet_published,
                })
                .collect::<Vec<_>>();
            (registry.seq, sessions)
        };
        // Most recent first, then the full identity, so equal epochs still
        // order the same way on every call.
        sessions.sort_by(|a, b| {
            b.last_activity_at
                .cmp(&a.last_activity_at)
                .then_with(|| a.session.environment_key.cmp(&b.session.environment_key))
                .then_with(|| a.session.agent.cmp(&b.session.agent))
                .then_with(|| a.session.session_id.cmp(&b.session.session_id))
        });
        sessions.truncate(limit);
        LiveSnapshot { seq, sessions }
    }

    /// The sequence of the last published event.
    pub fn current_seq(&self) -> u64 {
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .seq
    }

    /// Fill the live map from the store once, before the actor runs. A row
    /// outside the window is skipped, and so is an agent slug the shell does
    /// not know.
    fn seed(&self, rows: Vec<(SessionKey, i64)>, now: i64) {
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (key, last_activity_at) in rows {
            if now - last_activity_at >= ACTIVE_SESSION_WINDOW_SECS {
                continue;
            }
            let Some(agent) = AgentKind::from_slug(&key.agent) else {
                continue;
            };
            registry.insert_live(
                key,
                LiveEntry {
                    agent,
                    last_activity_at,
                    // A row already past the quiet window has no `Quiet` to
                    // publish: the snapshot carries its epoch.
                    quiet_published: now - last_activity_at >= QUIET_WINDOW_SECS,
                },
            );
        }
    }

    /// True on the first call only. The caller becomes the actor.
    fn claim_actor(&self) -> bool {
        !self.actor_claimed.swap(true, Ordering::AcqRel)
    }
}

/// Merge `observation` into an already queued observation with the same
/// identity. False means nothing merged safely and the report is lost.
///
/// A merge must not move a report across a queued observation that can
/// change the same session's state the other way. Folding a fresh touch
/// into a slot before a queued `Removed` would replay the removal last and
/// kill a session the newest report said is alive; the mirror image would
/// revive one. Such a merge is refused, the report is dropped, and the
/// actor's `Resync` reconciles.
fn coalesce(queue: &mut VecDeque<Observation>, observation: Observation) -> bool {
    match observation {
        Observation::Touched { session, agent, at } => {
            let Some(index) = queue.iter().rposition(|queued| {
                matches!(
                    queued,
                    Observation::Touched {
                        session: queued_session,
                        agent: queued_agent,
                        ..
                    } if *queued_session == session && *queued_agent == agent
                )
            }) else {
                return false;
            };
            if let Some(key) = &session
                && removed_after(queue, index, key)
            {
                return false;
            }
            if let Some(Observation::Touched { at: queued_at, .. }) = queue.get_mut(index) {
                *queued_at = (*queued_at).max(at);
                return true;
            }
            false
        }
        Observation::Indexed { sessions } => {
            let Some(index) = queue
                .iter()
                .rposition(|queued| matches!(queued, Observation::Indexed { .. }))
            else {
                return false;
            };
            if sessions
                .iter()
                .any(|session| removed_after(queue, index, &session.key))
            {
                return false;
            }
            let Some(Observation::Indexed {
                sessions: queued_sessions,
            }) = queue.get_mut(index)
            else {
                return false;
            };
            if queued_sessions.len() + sessions.len() > COALESCED_INDEX_CAP {
                return false;
            }
            for session in sessions {
                match queued_sessions
                    .iter_mut()
                    .find(|queued| queued.key == session.key)
                {
                    Some(queued) => {
                        queued.at = queued.at.max(session.at);
                        queued.is_new |= session.is_new;
                    }
                    None => queued_sessions.push(session),
                }
            }
            true
        }
        Observation::RowChanged {
            session,
            facets,
            at,
        } => {
            let Some(index) = queue.iter().rposition(|queued| {
                matches!(
                    queued,
                    Observation::RowChanged {
                        session: queued_session,
                        ..
                    } if *queued_session == session
                )
            }) else {
                return false;
            };
            if removed_after(queue, index, &session) {
                return false;
            }
            if let Some(Observation::RowChanged {
                facets: queued_facets,
                at: queued_at,
                ..
            }) = queue.get_mut(index)
            {
                queued_facets.merge(facets);
                *queued_at = (*queued_at).max(at);
                return true;
            }
            false
        }
        Observation::Removed { .. } => {
            let Some(index) = queue.iter().rposition(|queued| *queued == observation) else {
                return false;
            };
            // The duplicate stands in for this report only while nothing
            // after it re-establishes the session.
            if let Observation::Removed {
                session: Some(key), ..
            } = &observation
                && establishes_after(queue, index, key)
            {
                return false;
            }
            true
        }
        Observation::IndexChanged { .. } => queue.iter().any(|queued| *queued == observation),
    }
}

/// True when a queued `Removed` after `index` names `key`.
fn removed_after(queue: &VecDeque<Observation>, index: usize, key: &SessionKey) -> bool {
    queue.iter().skip(index + 1).any(|queued| {
        matches!(
            queued,
            Observation::Removed {
                session: Some(removed),
                ..
            } if removed == key
        )
    })
}

/// True when a queued observation after `index` reports `key` alive again.
fn establishes_after(queue: &VecDeque<Observation>, index: usize, key: &SessionKey) -> bool {
    queue.iter().skip(index + 1).any(|queued| match queued {
        Observation::Touched {
            session: Some(touched),
            ..
        } => touched == key,
        Observation::Indexed { sessions } => sessions.iter().any(|session| &session.key == key),
        Observation::RowChanged { session, .. } => session == key,
        Observation::Touched { session: None, .. }
        | Observation::Removed { .. }
        | Observation::IndexChanged { .. } => false,
    })
}

/// Report an observation when the shell manages the bus. A test app without
/// managed state reports nothing.
pub fn report(app: &AppHandle, observation: Observation) {
    if let Some(events) = app.try_state::<SessionEvents>() {
        events.report(observation);
    }
}

/// Seed the live map from the store, then start the actor. Seeding happens
/// synchronously in this call, so every producer started after it sees a
/// populated registry. The returned handle is aborted with the rest of the
/// schedulers on exit.
pub fn spawn(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let events = app.state::<SessionEvents>();
    // `now` tracks tokio's own clock rather than the wall clock directly,
    // so a test can drive it deterministically under
    // `tokio::time::pause()`: every `tokio::time::sleep` below advances
    // this clock exactly as far as it advances the real one.
    let base_epoch = crate::scan::unix_now();
    let base_instant = Instant::now();
    let claimed = events.claim_actor();
    if claimed {
        let seed_rows = app
            .state::<Store>()
            .sessions_active_since(base_epoch - ACTIVE_SESSION_WINDOW_SECS)
            .unwrap_or_default();
        events.seed(seed_rows, base_epoch);
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if !claimed {
            ::tracing::error!(event = "session_lifecycle_spawned_twice");
            return;
        }
        let events = app.state::<SessionEvents>();
        let now = move || base_epoch + base_instant.elapsed().as_secs() as i64;
        run(&events, &now).await;
    })
}

/// The loop [`spawn`] runs forever. Split out so a test can drive it with a
/// captured clock, without a Tauri app.
async fn run(events: &SessionEvents, now: &(dyn Fn() -> i64 + Send + Sync)) {
    loop {
        let deadline = soonest_deadline(&events.registry);
        tokio::select! {
            () = events.inbox_wake.notified() => {
                drain(events, now);
            }
            () = sleep_until_deadline(deadline, now) => {
                expire(events, now());
            }
        }
    }
}

/// Apply every queued observation, then publish `Resync` when the queue
/// overflowed and a report was lost.
fn drain(events: &SessionEvents, now: &(dyn Fn() -> i64 + Send + Sync)) {
    loop {
        let (observation, overflowed) = {
            let mut inbox = events
                .inbox
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match inbox.queue.pop_front() {
                Some(observation) => (Some(observation), false),
                None => (None, std::mem::take(&mut inbox.overflowed)),
            }
        };
        match observation {
            Some(observation) => observe(events, observation, now()),
            None => {
                if overflowed {
                    publish(events, vec![SessionEvent::Resync]);
                }
                return;
            }
        }
    }
}

/// The earliest moment a live session can go quiet or idle, or `None` with
/// no live session.
fn soonest_deadline(registry: &Mutex<Registry>) -> Option<i64> {
    registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .deadlines
        .first()
        .map(|(deadline, _)| *deadline)
}

/// Sleep until `deadline` plus slack, or forever when nothing is live. A
/// `tokio::select!` arm with nothing pending must never fire.
async fn sleep_until_deadline(deadline: Option<i64>, now: &(dyn Fn() -> i64 + Send + Sync)) {
    match deadline {
        Some(deadline) => {
            let wait_secs = (deadline + EXPIRY_SLACK_SECS - now()).max(0);
            tokio::time::sleep(Duration::from_secs(wait_secs as u64)).await;
        }
        None => std::future::pending().await,
    }
}

/// Apply one observation to the registry and publish what changed.
fn observe(events: &SessionEvents, observation: Observation, now: i64) {
    match observation {
        Observation::Touched {
            session: Some(key),
            agent,
            at,
        } => {
            apply(events, |registry| {
                match registry.touch(&key, agent, at, now) {
                    Touch::Added { resumed } | Touch::Advanced { resumed } => {
                        vec![SessionEvent::Activity {
                            session: Some(SessionRef::from(&key)),
                            agent,
                            at,
                            resumed,
                        }]
                    }
                    // A duplicate or an old observation says nothing new.
                    Touch::Unchanged | Touch::Stale => Vec::new(),
                }
            });
        }
        Observation::Touched {
            session: None,
            agent,
            at,
        } => {
            apply(events, |registry| {
                if now - at >= ACTIVE_SESSION_WINDOW_SECS {
                    return Vec::new();
                }
                match registry.keyless.get(&agent) {
                    Some(published) if *published >= at => Vec::new(),
                    _ => {
                        registry.keyless.insert(agent, at);
                        vec![SessionEvent::Activity {
                            session: None,
                            agent,
                            at,
                            resumed: false,
                        }]
                    }
                }
            });
        }
        Observation::Indexed { sessions } => {
            apply(events, |registry| {
                let mut out = Vec::new();
                for indexed in sessions {
                    let session = SessionRef::from(&indexed.key);
                    match registry.touch(&indexed.key, indexed.agent, indexed.at, now) {
                        Touch::Added { resumed: false } if indexed.is_new => {
                            // Discovery resolved this identity: any keyless
                            // activity for the agent is accounted for.
                            registry.keyless.remove(&indexed.agent);
                            out.push(SessionEvent::Started {
                                session,
                                agent: indexed.agent,
                                at: indexed.at,
                            });
                        }
                        Touch::Added { resumed } | Touch::Advanced { resumed } => {
                            out.push(SessionEvent::Activity {
                                session: Some(session),
                                agent: indexed.agent,
                                at: indexed.at,
                                resumed,
                            });
                        }
                        Touch::Unchanged | Touch::Stale => {}
                    }
                }
                out
            });
        }
        Observation::RowChanged {
            session,
            facets,
            at,
        } => {
            publish(
                events,
                vec![SessionEvent::Updated {
                    session: SessionRef::from(&session),
                    facets,
                    at,
                }],
            );
        }
        Observation::Removed { session, reason } => {
            apply(events, |registry| {
                let session_ref = session.as_ref().map(SessionRef::from);
                let mut out = Vec::new();
                if let Some(key) = &session
                    && let Some(entry) = registry.remove_live(key)
                {
                    // The lifecycle scope narrates the decay: without this,
                    // a reader tracking working state waits for a `Quiet`
                    // or `Idle` the registry can no longer publish. The
                    // identity is remembered so a stale touch cannot
                    // resurrect the deleted session.
                    registry.remember_idle(key.clone(), entry.last_activity_at, now);
                    out.push(SessionEvent::Idle {
                        session: SessionRef::from(key),
                        agent: entry.agent,
                        at: now,
                    });
                }
                out.push(SessionEvent::Removed {
                    session: session_ref,
                    reason,
                });
                out
            });
        }
        Observation::IndexChanged { reason } => {
            publish(events, vec![SessionEvent::IndexChanged { reason }]);
        }
    }
}

/// Publish `Quiet` and `Idle` for every session whose deadline passed, in
/// deadline order. A session that crosses both windows in one wake gets
/// `Idle` only.
fn expire(events: &SessionEvents, now: i64) {
    apply(events, |registry| {
        let mut out = Vec::new();
        while let Some((deadline, key)) = registry.deadlines.first().cloned() {
            if deadline > now {
                break;
            }
            let Some(entry) = registry.live.get(&key).copied() else {
                // The index and the map move together; a miss is a bug.
                registry.deadlines.remove(&(deadline, key));
                continue;
            };
            if now - entry.last_activity_at >= ACTIVE_SESSION_WINDOW_SECS {
                registry.remove_live(&key);
                registry.remember_idle(key.clone(), entry.last_activity_at, now);
                out.push(SessionEvent::Idle {
                    session: SessionRef::from(&key),
                    agent: entry.agent,
                    at: now,
                });
            } else if !entry.quiet_published && now - entry.last_activity_at >= QUIET_WINDOW_SECS {
                registry.deadlines.remove(&(deadline, key.clone()));
                let entry = registry
                    .live
                    .get_mut(&key)
                    .expect("the entry was read above");
                entry.quiet_published = true;
                registry.deadlines.insert((entry.deadline(), key.clone()));
                out.push(SessionEvent::Quiet {
                    session: SessionRef::from(&key),
                    agent: entry.agent,
                    at: now,
                });
            } else {
                // Wake slack can fire before the deadline second. Move the
                // entry to its computed deadline and wait again.
                registry.deadlines.remove(&(deadline, key.clone()));
                registry.deadlines.insert((entry.deadline(), key));
                break;
            }
        }
        out
    });
}

/// Mutate the registry, assign a sequence to every produced event under the
/// same lock, and publish after the lock is released.
fn apply(events: &SessionEvents, mutate: impl FnOnce(&mut Registry) -> Vec<SessionEvent>) {
    let sequenced = {
        let mut registry = events
            .registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        mutate(&mut registry)
            .into_iter()
            .map(|event| {
                registry.seq += 1;
                Sequenced {
                    seq: registry.seq,
                    event,
                }
            })
            .collect::<Vec<_>>()
    };
    for event in sequenced {
        // A bus with no subscriber returns an error, and that is not a
        // fault: the HUD may be closed.
        let _ = events.bus.send(event);
    }
}

/// Publish events that mutate nothing. Sequencing still goes through the
/// registry lock.
fn publish(events: &SessionEvents, out: Vec<SessionEvent>) {
    apply(events, move |_| out);
}

#[cfg(test)]
mod tests;
