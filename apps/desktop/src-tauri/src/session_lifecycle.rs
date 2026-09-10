//! Session lifecycle: one actor decides which sessions are live, and one
//! broadcast bus tells every reader.
//!
//! The scan pipeline reports what it sees as an [`Observation`]: a watcher
//! burst touched a session, or a pass indexed some sessions. The actor is the
//! only sender on the bus. It turns observations into [`SessionEvent`]s, keeps
//! the map of live sessions, publishes `Quiet` when a session crosses
//! [`QUIET_WINDOW_SECS`] without a write, and `Idle` when it crosses
//! [`ACTIVE_SESSION_WINDOW_SECS`].
//!
//! A `Touched` observation arrives at burst classification, before any
//! per-session floor or describe. That is what makes the bus faster than the
//! row-refresh path: a transcript write reaches the bus in about the
//! watcher's quiet window.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS;
use antiburn_local::model::AgentKind;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use crate::store::{SessionKey, Store};

/// How many events a subscriber can fall behind before it is told it lagged.
pub const BUS_CAPACITY: usize = 64;

/// How many observations can queue before a producer drops one. A dropped
/// observation costs nothing durable: the next burst reports the session
/// again.
const INBOX_CAPACITY: usize = 256;

/// Slack added past a session's computed deadline, so the actor never wakes a
/// moment early and finds the session still (barely) active.
const EXPIRY_SLACK_SECS: i64 = 1;

/// How long a session goes without a write before the bus calls it quiet.
/// The meters animate from `Activity` to `Quiet`. The session stays active,
/// for the session list, until [`ACTIVE_SESSION_WINDOW_SECS`].
pub const QUIET_WINDOW_SECS: i64 = 30;

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
    /// store has not indexed the session yet.
    Activity {
        session: Option<SessionRef>,
        agent: AgentKind,
        at: i64,
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
    /// `new` names the keys the store had not indexed before.
    Indexed {
        sessions: Vec<(SessionKey, AgentKind, i64)>,
        new: Vec<SessionKey>,
    },
}

/// One live session, as the snapshot command returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSession {
    pub session: SessionRef,
    pub agent: AgentKind,
    pub last_activity_at: i64,
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

type LiveMap = HashMap<SessionKey, LiveEntry>;

/// The bus and the actor's inbox, held in Tauri managed state.
///
/// `subscribe` gives a reader every event from now on. `report` hands the
/// actor an observation. `live_sessions` is the snapshot a late subscriber
/// reads instead of a replay.
pub struct SessionEvents {
    inbox: mpsc::Sender<Observation>,
    /// Taken once by [`spawn`], which moves it into the actor task.
    pending_inbox: Mutex<Option<mpsc::Receiver<Observation>>>,
    bus: broadcast::Sender<SessionEvent>,
    live: Arc<Mutex<LiveMap>>,
}

impl Default for SessionEvents {
    fn default() -> Self {
        let (inbox, receiver) = mpsc::channel(INBOX_CAPACITY);
        let (bus, _) = broadcast::channel(BUS_CAPACITY);
        Self {
            inbox,
            pending_inbox: Mutex::new(Some(receiver)),
            bus,
            live: Arc::new(Mutex::new(LiveMap::new())),
        }
    }
}

impl SessionEvents {
    /// A receiver that sees every event published after this call.
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.bus.subscribe()
    }

    /// Hand the actor one observation. A full inbox drops it with a log line
    /// rather than blocking the scan pipeline.
    pub fn report(&self, observation: Observation) {
        if let Err(error) = self.inbox.try_send(observation) {
            ::tracing::warn!(event = "session_lifecycle_inbox_full", error = %error);
        }
    }

    /// Every session inside the active window, most recent first.
    pub fn live_sessions(&self) -> Vec<LiveSession> {
        let live = self
            .live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut sessions = live
            .iter()
            .map(|(key, entry)| LiveSession {
                session: SessionRef::from(key),
                agent: entry.agent,
                last_activity_at: entry.last_activity_at,
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|a, b| {
            b.last_activity_at
                .cmp(&a.last_activity_at)
                .then_with(|| a.session.session_id.cmp(&b.session.session_id))
        });
        sessions
    }

    /// Fill the live map from the store once, before the actor runs. A row
    /// outside the window is skipped, and so is an agent slug the shell does
    /// not know.
    fn seed(&self, rows: Vec<(SessionKey, i64)>, now: i64) {
        let mut live = self
            .live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (key, last_activity_at) in rows {
            if now - last_activity_at >= ACTIVE_SESSION_WINDOW_SECS {
                continue;
            }
            let Some(agent) = AgentKind::from_slug(&key.agent) else {
                continue;
            };
            live.insert(
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

    fn take_inbox(&self) -> Option<mpsc::Receiver<Observation>> {
        self.pending_inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

/// Report an observation when the shell manages the bus. A test app without
/// managed state reports nothing.
pub fn report(app: &AppHandle, observation: Observation) {
    if let Some(events) = app.try_state::<SessionEvents>() {
        events.report(observation);
    }
}

/// Seed the live map from the store and start the actor. The returned handle
/// is aborted with the rest of the schedulers on exit.
pub fn spawn(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let events = app.state::<SessionEvents>();
        // `now` tracks tokio's own clock rather than the wall clock directly,
        // so a test can drive it deterministically under
        // `tokio::time::pause()`: every `tokio::time::sleep` below advances
        // this clock exactly as far as it advances the real one.
        let base_epoch = crate::scan::unix_now();
        let base_instant = Instant::now();
        let now = move || base_epoch + base_instant.elapsed().as_secs() as i64;
        let seed_rows = app
            .state::<Store>()
            .sessions_active_since(now() - ACTIVE_SESSION_WINDOW_SECS)
            .unwrap_or_default();
        events.seed(seed_rows, now());
        let Some(inbox) = events.take_inbox() else {
            ::tracing::error!(event = "session_lifecycle_spawned_twice");
            return;
        };
        run(inbox, &events, &now).await;
    })
}

/// Carry every bus event to the webviews as `session:lifecycle`.
///
/// An `Idle` event also becomes the `sessions:entry-changed` the popover's
/// list already reads, so a row's active pill clears the moment its window
/// ends. A lagged subscriber logs and carries on: the webviews re-read the
/// live snapshot on the next `scan:finished`.
pub fn spawn_bridge(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    let mut bus = app.state::<SessionEvents>().subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match bus.recv().await {
                Ok(event) => {
                    ::tracing::debug!(event = "session_lifecycle_event", payload = ?event);
                    let _ = app.emit(crate::commands::SESSION_LIFECYCLE_EVENT, &event);
                    if let SessionEvent::Idle { session, at, .. } = &event {
                        let key = SessionKey::new(
                            session.environment_key.clone(),
                            session.agent.clone(),
                            session.session_id.clone(),
                        );
                        let store = app.state::<Store>();
                        if let Some(entry) =
                            crate::insights_worker::completion_entry(&store, &key, *at)
                        {
                            let _ = app.emit(crate::commands::SESSION_ENTRY_CHANGED_EVENT, &entry);
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    ::tracing::warn!(event = "session_lifecycle_bridge_lagged", skipped);
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    })
}

/// The loop [`spawn`] runs forever. Split out so a test can drive it with a
/// captured clock, without a Tauri app.
async fn run(
    mut inbox: mpsc::Receiver<Observation>,
    events: &SessionEvents,
    now: &(dyn Fn() -> i64 + Send + Sync),
) {
    loop {
        let deadline = soonest_deadline(&events.live);
        tokio::select! {
            observation = inbox.recv() => {
                let Some(observation) = observation else {
                    // Every producer is gone. Nothing can arrive again.
                    return;
                };
                observe(events, observation, now());
            }
            () = sleep_until_deadline(deadline, now) => {
                expire(events, now());
            }
        }
    }
}

/// The earliest moment a live session can go quiet or idle, or `None` with
/// no live session.
fn soonest_deadline(live: &Mutex<LiveMap>) -> Option<i64> {
    live.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .map(LiveEntry::deadline)
        .min()
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

/// Apply one observation to the live map and publish what changed.
fn observe(events: &SessionEvents, observation: Observation, now: i64) {
    match observation {
        Observation::Touched { session, agent, at } => {
            if let Some(key) = &session {
                touch(&events.live, key, agent, at);
            }
            publish(
                events,
                SessionEvent::Activity {
                    session: session.as_ref().map(SessionRef::from),
                    agent,
                    at,
                },
            );
        }
        Observation::Indexed { sessions, new } => {
            let new: HashSet<SessionKey> = new.into_iter().collect();
            for (key, agent, at) in sessions {
                // A row older than the window is history, not activity.
                if now - at >= ACTIVE_SESSION_WINDOW_SECS {
                    continue;
                }
                let session = SessionRef::from(&key);
                match touch(&events.live, &key, agent, at) {
                    Touch::Added if new.contains(&key) => {
                        publish(events, SessionEvent::Started { session, agent, at });
                    }
                    Touch::Added | Touch::Advanced => {
                        publish(
                            events,
                            SessionEvent::Activity {
                                session: Some(session),
                                agent,
                                at,
                            },
                        );
                    }
                    Touch::Unchanged => {}
                }
            }
        }
    }
}

/// What one touch did to the live map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Touch {
    /// The session was not live before.
    Added,
    /// The session was live, and `at` moved its last activity later.
    Advanced,
    /// The session was live with an epoch at least as late as `at`.
    Unchanged,
}

/// Record activity for `key`. An older epoch changes nothing.
fn touch(live: &Mutex<LiveMap>, key: &SessionKey, agent: AgentKind, at: i64) -> Touch {
    let mut live = live.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match live.get_mut(key) {
        Some(entry) if entry.last_activity_at >= at => Touch::Unchanged,
        Some(entry) => {
            entry.last_activity_at = at;
            entry.agent = agent;
            entry.quiet_published = false;
            Touch::Advanced
        }
        None => {
            live.insert(
                key.clone(),
                LiveEntry {
                    agent,
                    last_activity_at: at,
                    quiet_published: false,
                },
            );
            Touch::Added
        }
    }
}

/// Publish `Quiet` for every session past the quiet window, and remove every
/// session past the active window with an `Idle`. Oldest activity first. A
/// session that crosses both windows in one wake gets `Idle` only.
fn expire(events: &SessionEvents, now: i64) {
    let (quiet, expired) = {
        let mut live = events
            .live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut expired = live
            .iter()
            .filter(|(_, entry)| now - entry.last_activity_at >= ACTIVE_SESSION_WINDOW_SECS)
            .map(|(key, entry)| (key.clone(), *entry))
            .collect::<Vec<_>>();
        for (key, _) in &expired {
            live.remove(key);
        }
        let mut quiet = Vec::new();
        for (key, entry) in live.iter_mut() {
            if !entry.quiet_published && now - entry.last_activity_at >= QUIET_WINDOW_SECS {
                entry.quiet_published = true;
                quiet.push((key.clone(), *entry));
            }
        }
        let by_activity = |(a_key, a): &(SessionKey, LiveEntry),
                           (b_key, b): &(SessionKey, LiveEntry)| {
            a.last_activity_at
                .cmp(&b.last_activity_at)
                .then_with(|| a_key.session_id.cmp(&b_key.session_id))
        };
        quiet.sort_by(by_activity);
        expired.sort_by(by_activity);
        (quiet, expired)
    };
    for (key, entry) in quiet {
        publish(
            events,
            SessionEvent::Quiet {
                session: SessionRef::from(&key),
                agent: entry.agent,
                at: now,
            },
        );
    }
    for (key, entry) in expired {
        publish(
            events,
            SessionEvent::Idle {
                session: SessionRef::from(&key),
                agent: entry.agent,
                at: now,
            },
        );
    }
}

/// Send one event. A bus with no subscriber returns an error, and that is
/// not a fault: the HUD may be closed.
fn publish(events: &SessionEvents, event: SessionEvent) {
    let _ = events.bus.send(event);
}

#[cfg(test)]
mod tests;
