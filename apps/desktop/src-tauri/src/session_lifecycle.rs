//! One actor owns the live session registry and publishes transitions on a broadcast
//! bus.
//!
//! Producers report compact typed facts through [`Observation`]. The scan task uses
//! [`report_async`] and waits for inbox capacity. Other producers use [`report`] with
//! [`SyncObservation`]. A full inbox sends sync facts to the bounded spill. Sync
//! reporting can briefly wait for the spill mutex, but never for inbox capacity. Sync
//! facts cannot establish presence.
//!
//! Each existence fact carries the writer's [`Revision`] and the row's [`Incarnation`].
//! The registry defers uncertain admissions until an injected [`ReconcileSource`]
//! supplies presence evidence. Reads run on the blocking pool, outside the actor's
//! registry lock. Activity never crosses incarnations.
//!
//! Anonymous touches carry an [`AnonymousGen`] from the scan scheduler. Only a covering
//! generation or the registry's quiet deadline clears anonymous activity. `Started`
//! does not clear it.
//!
//! The registry publishes `Quiet` after [`QUIET_WINDOW_SECS`] without activity. It
//! publishes `Idle` after [`ACTIVE_SESSION_WINDOW_SECS`]. Snapshots include the
//! canonical sequence and exact counts. The last lifecycle event of each atomic batch
//! carries its [`Aggregate`]. Readers use [`LivePresence`] for identities that a
//! bounded snapshot omits.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::ops::Bound;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS;
use antiburn_local::model::AgentKind;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tokio::sync::{Notify, broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::store::{ActiveCursor, Incarnation, Presence, Revision, SessionKey, Store};

/// The broadcast ring holds this many events. A lagging projection worker requests
/// snapshot recovery.
pub const BUS_CAPACITY: usize = 1024;

/// The inbox holds this many observations. Async reporters wait for capacity. Sync
/// reporters use the spill.
const INBOX_CAPACITY: usize = 256;

/// Each service round receives at most this many observations.
const DRAIN_BATCH: usize = 64;

/// Each service round applies at most this many session facts. The carry retains the
/// remainder.
const DRAIN_WEIGHT: usize = 256;

/// Each `Indexed` observation holds at most this many sessions.
pub(crate) const INDEXED_CHUNK: usize = 256;

/// Each service round expires at most this many due deadlines.
const EXPIRE_BATCH: usize = 256;

/// Each spill map holds at most this many keyed cells. Overflow converts removals to
/// broad removals and row changes to list refreshes.
const SPILL_KEY_CAP: usize = 1024;

/// The registry remembers at most this many deletions. Eviction raises
/// `forgotten_through` to retain the admission guard.
const DELETION_MEMORY_CAP: usize = 1024;

/// At most this many keys wait for presence evidence. A full pending set blocks inbox
/// consumption without dropping facts.
const ADMISSION_PENDING_CAP: usize = 1024;

/// Each presence or seed page holds at most this many keys or rows.
const RECONCILE_PAGE: usize = 256;

/// The first failed page starts this retry delay. Each further failure doubles the
/// delay.
const RECONCILE_BACKOFF_MIN: Duration = Duration::from_secs(2);

/// Failed pages cannot increase the retry delay beyond this duration.
const RECONCILE_BACKOFF_MAX: Duration = Duration::from_secs(30);

/// The registry remembers at most this many recently idle keys for resume events.
/// Admission safety does not depend on this memory.
const RECENT_IDLE_CAP: usize = 256;

/// This delay prevents a wake just before the computed deadline.
const EXPIRY_SLACK_SECS: i64 = 1;

/// This duration without activity makes a session quiet. It remains live until
/// [`ACTIVE_SESSION_WINDOW_SECS`].
pub const QUIET_WINDOW_SECS: i64 = 30;

/// A snapshot returns at most this many rows when the caller supplies no limit.
pub const DEFAULT_SNAPSHOT_LIMIT: usize = 128;

/// This identity connects a webview session to a named presence request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

impl From<&SessionRef> for SessionKey {
    fn from(session: &SessionRef) -> Self {
        SessionKey::new(
            session.environment_key.clone(),
            session.agent.clone(),
            session.session_id.clone(),
        )
    }
}

/// These facets name the changed parts of a session row. The projection worker decides
/// what to load.
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

/// This reason identifies why a session leaves the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalReason {
    /// The reader or a worker deletes the row.
    Deleted,
    /// Retention removes the row.
    Purged,
    /// The scan gate rejects the source and removes the row.
    Rejected,
    /// Store evidence shows an absent row or a newer incarnation.
    Reconciled,
}

/// This reason identifies a change to the session list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexChangeReason {
    /// A scan pass changes list membership.
    ScanPass,
    /// Readers must reload the list after a broad invalidation.
    Invalidated,
}

/// This cause identifies why anonymous activity ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnonymousClearCause {
    /// A successful pass covers every generation the registry holds.
    Resolved,
    /// The registry reaches the quiet deadline without a covering pass.
    Expired,
}

/// The scheduler assigns each anonymous touch a checked increasing generation. Covers
/// compare generations, not timestamps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnonymousGen(pub u64);

/// A successful pass covers this agent through the named generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnonymousCover {
    pub agent: AgentKind,
    pub through: AnonymousGen,
}

/// This event describes a session transition or a projection request. Broadcast lag
/// requires snapshot recovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    /// The store indexes this session for the first time.
    Started {
        session: SessionRef,
        agent: AgentKind,
        at: i64,
    },
    /// A watcher observes a write to the session source. `session` is `None` for an
    /// unresolved agent path. `resumed` identifies activity after `Quiet` or `Idle`.
    Activity {
        session: Option<SessionRef>,
        agent: AgentKind,
        at: i64,
        resumed: bool,
    },
    /// The session reaches [`QUIET_WINDOW_SECS`] without activity. It remains live
    /// until the idle deadline.
    Quiet {
        session: SessionRef,
        agent: AgentKind,
        at: i64,
    },
    /// The session reaches [`ACTIVE_SESSION_WINDOW_SECS`] without activity.
    Idle {
        session: SessionRef,
        agent: AgentKind,
        at: i64,
    },
    /// A covering pass or the quiet deadline ends anonymous activity. Only the registry
    /// clears this state.
    AnonymousCleared {
        agent: AgentKind,
        at: i64,
        cause: AnonymousClearCause,
    },
    /// A row change requests projection work. The event carries no enriched row.
    Updated {
        session: SessionRef,
        facets: UpdateFacets,
        at: i64,
    },
    /// A session leaves the store. `session` is `None` for a broad removal.
    Removed {
        session: Option<SessionRef>,
        reason: RemovalReason,
    },
    /// List membership changes beyond one named row.
    IndexChanged { reason: IndexChangeReason },
    /// The projection bridge requests a snapshot after transport lag. The registry
    /// never publishes this marker.
    Resync,
}

impl SessionEvent {
    /// Return whether the bridge relays this event on `session:lifecycle`.
    pub fn is_lifecycle(&self) -> bool {
        matches!(
            self,
            Self::Started { .. }
                | Self::Activity { .. }
                | Self::Quiet { .. }
                | Self::Idle { .. }
                | Self::AnonymousCleared { .. }
                | Self::Resync
        )
    }
}

/// These exact counts describe the registry after an atomic batch. Its last lifecycle
/// event carries the counts independently of snapshot row limits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Aggregate {
    pub working: usize,
    pub total: usize,
    pub anonymous: usize,
}

/// This internal bus message carries the canonical event sequence. The last lifecycle
/// event of a batch also carries exact counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sequenced {
    pub seq: u64,
    pub event: SessionEvent,
    pub aggregate: Option<Aggregate>,
}

/// The projection bridge sends this payload on `session:lifecycle`. The wire shape can
/// change independently of the internal bus.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleEnvelope<'a> {
    pub seq: u64,
    #[serde(flatten)]
    pub event: &'a SessionEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<Aggregate>,
}

/// A scan pass reports this session with its incarnation and activity epoch. `is_new`
/// refers to the full identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedSession {
    pub key: SessionKey,
    pub agent: AgentKind,
    pub incarnation: Incarnation,
    pub at: i64,
    pub is_new: bool,
}

/// A watcher lookup supplies this session identity and incarnation at the writer
/// revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TouchedSession {
    pub key: SessionKey,
    pub incarnation: Incarnation,
    pub seen: Revision,
}

/// This scope identifies the rows a removal affects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemovalScope {
    /// The delete returns this row identity and incarnation.
    One(SessionKey, Incarnation),
    /// The reporter cannot name the removed rows. The registry checks live entries that
    /// predate the removal revision.
    Broad,
}

/// Producers report these compact facts. Only the actor converts them to canonical
/// events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observation {
    /// A watcher burst touches a path for this indexed session.
    Touched {
        session: TouchedSession,
        agent: AgentKind,
        at: i64,
    },
    /// A watcher burst touches an unresolved agent path. `generation` orders the touch
    /// against covers.
    Anonymous {
        agent: AgentKind,
        at: i64,
        generation: AnonymousGen,
    },
    /// A scan pass writes these rows at `revision`. Each row carries its incarnation
    /// and activity epoch.
    Indexed {
        sessions: Vec<IndexedSession>,
        revision: Revision,
    },
    /// A successful scheduler pass covers these anonymous generations. The cover
    /// follows every `Indexed` report from that pass.
    AnonymousCovered { covers: Vec<AnonymousCover> },
    /// A worker changes parts of one session row.
    RowChanged {
        session: SessionKey,
        facets: UpdateFacets,
        at: i64,
    },
    /// The transaction removes rows at `revision`.
    Removed {
        scope: RemovalScope,
        reason: RemovalReason,
        revision: Revision,
    },
    /// List membership changes beyond one named row.
    IndexChanged { reason: IndexChangeReason },
}

/// Sync producers report this subset without waiting for inbox capacity. It cannot
/// establish presence. The bounded spill can therefore combine its facts safely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncObservation {
    /// A worker changes parts of one session row.
    RowChanged {
        session: SessionKey,
        facets: UpdateFacets,
        at: i64,
    },
    /// The transaction removes rows at `revision`.
    Removed {
        scope: RemovalScope,
        reason: RemovalReason,
        revision: Revision,
    },
    /// List membership changes beyond one named row.
    IndexChanged { reason: IndexChangeReason },
}

impl From<SyncObservation> for Observation {
    fn from(observation: SyncObservation) -> Self {
        match observation {
            SyncObservation::RowChanged {
                session,
                facets,
                at,
            } => Observation::RowChanged {
                session,
                facets,
                at,
            },
            SyncObservation::Removed {
                scope,
                reason,
                revision,
            } => Observation::Removed {
                scope,
                reason,
                revision,
            },
            SyncObservation::IndexChanged { reason } => Observation::IndexChanged { reason },
        }
    }
}

/// The snapshot returns this live session state. Readers use `quiet` instead of
/// computing lifecycle windows from timestamps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSession {
    pub session: SessionRef,
    pub agent: AgentKind,
    pub last_activity_at: i64,
    pub quiet: bool,
}

/// The snapshot returns this agent while its anonymous activity remains inside the
/// quiet window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveAnonymous {
    pub agent: AgentKind,
    pub last_activity_at: i64,
}

/// This snapshot contains the registry state at `seq`. Readers apply only later deltas.
/// `working` and `total` remain exact regardless of the row limit. `sessions.len() <
/// total` identifies omitted live rows. `anonymous` contains every anonymous agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSnapshot {
    pub seq: u64,
    pub working: usize,
    pub total: usize,
    pub sessions: Vec<LiveSession>,
    pub anonymous: Vec<LiveAnonymous>,
}

/// The registry answers every requested identity at one sequence. One lock protects the
/// present and absent lists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LivePresence {
    pub seq: u64,
    pub present: Vec<LiveSession>,
    pub absent: Vec<SessionRef>,
}

/// This boundary supplies Store evidence through blocking reads. The actor holds no
/// registry lock while it waits.
pub trait ReconcileSource: Send + Sync {
    /// Read presence evidence for at most [`RECONCILE_PAGE`] keys at one writer
    /// revision.
    fn presence(&self, keys: &[SessionKey]) -> anyhow::Result<(Vec<Presence>, Revision)>;

    /// Read the next page of active rows in descending cursor order.
    fn active(
        &self,
        since: i64,
        after: Option<&ActiveCursor>,
        limit: usize,
    ) -> anyhow::Result<(Vec<Presence>, Revision)>;
}

impl ReconcileSource for Store {
    fn presence(&self, keys: &[SessionKey]) -> anyhow::Result<(Vec<Presence>, Revision)> {
        self.session_presence_for_keys(keys)
    }

    fn active(
        &self,
        since: i64,
        after: Option<&ActiveCursor>,
        limit: usize,
    ) -> anyhow::Result<(Vec<Presence>, Revision)> {
        self.sessions_active_since_page(since, after, limit)
    }
}

#[derive(Clone, Copy, Debug)]
struct LiveEntry {
    agent: AgentKind,

    incarnation: Incarnation,
    /// The writer confirms the row exists at this revision.
    exists_at: Revision,
    last_activity_at: i64,
    /// A newer write clears this quiet state.
    quiet_published: bool,
}

impl LiveEntry {
    fn deadline(&self) -> i64 {
        if self.quiet_published {
            self.last_activity_at + ACTIVE_SESSION_WINDOW_SECS
        } else {
            self.last_activity_at + QUIET_WINDOW_SECS
        }
    }
}

/// The generation decides which covers apply. The activity time decides the anonymous
/// deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AnonymousEntry {
    generation: AnonymousGen,
    at: i64,
}

impl AnonymousEntry {
    fn deadline(&self) -> i64 {
        self.at + QUIET_WINDOW_SECS
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DeadlineKey {
    Session(SessionKey),
    Anonymous(AgentKind),
}

/// Deletion memory keeps the highest deleted incarnation and absence revision for each
/// key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Deletion {
    incarnation: Incarnation,
    absent_at: Revision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingAdmission {
    agent: AgentKind,
    incarnation: Incarnation,
    at: i64,
    revision: Revision,
    is_new: bool,
}

/// This fact confirms one incarnation exists at `revision` with activity at `at`.
#[derive(Clone, Copy, Debug)]
struct Existence {
    agent: AgentKind,
    incarnation: Incarnation,
    at: i64,
    revision: Revision,
    is_new: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Touch {
    /// `resumed` identifies a recently idle key.
    Added {
        resumed: bool,
    },
    /// `resumed` identifies a previously quiet session.
    Advanced {
        resumed: bool,
    },

    Unchanged,
    /// The fact falls outside the activity window or names a dead incarnation.
    Stale,
    /// A deletion guard makes this evidence uncertain. A presence page decides
    /// admission.
    Deferred,
    /// A full pending set blocks admission. The caller keeps the fact until capacity
    /// changes.
    Blocked,
}

#[derive(Debug)]
enum Progress {
    Done,
    /// The carry retains the remainder when the fact budget reaches zero.
    Exhausted(Observation),
    /// The carry retains the remainder while the pending set is full.
    Blocked(Observation),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PageClass {
    #[default]
    Admission,
    Walk,
    Recovery,
}

impl PageClass {
    fn successor(self) -> Self {
        match self {
            Self::Admission => Self::Walk,
            Self::Walk => Self::Recovery,
            Self::Recovery => Self::Admission,
        }
    }
}

/// The presence walk checks entries older than `through` in key order after `cursor`.
#[derive(Clone, Debug)]
struct Run {
    cursor: Option<SessionKey>,
    through: Revision,
    reason: RemovalReason,
}

#[derive(Debug, Default)]
struct ReconcileState {
    needs: Option<(RemovalReason, Revision)>,
    run: Option<Run>,
    turn: PageClass,
    recovery: Option<SeedRecovery>,
}

#[derive(Debug)]
struct SeedRecovery {
    since: i64,
    cursor: Option<ActiveCursor>,
    page: Option<RecoveryPage>,
}

#[derive(Debug)]
struct RecoveryPage {
    rows: VecDeque<Presence>,
    revision: Revision,
    cursor: Option<ActiveCursor>,
    exhausted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PageRequest {
    Active {
        since: i64,
        after: Option<ActiveCursor>,
    },
    Admission {
        keys: Vec<SessionKey>,
    },

    Walk {
        keys: Vec<SessionKey>,
        reason: RemovalReason,
    },
}

impl PageRequest {
    fn keys(&self) -> &[SessionKey] {
        match self {
            Self::Admission { keys } | Self::Walk { keys, .. } => keys,
            Self::Active { .. } => &[],
        }
    }
}

struct PageOutcome {
    request: PageRequest,
    result: anyhow::Result<(Vec<Presence>, Revision)>,
}

/// One lock protects the canonical lifecycle state and its event sequence.
#[derive(Default)]
struct Registry {
    seq: u64,
    /// The presence walk uses the live map key order.
    live: BTreeMap<SessionKey, LiveEntry>,
    /// Each lifecycle change maintains this count without walking the live map.
    working: usize,
    /// The index holds one current deadline per live session or anonymous agent.
    /// Touches replace deadlines instead of retaining stale entries.
    deadlines: BTreeSet<(i64, DeadlineKey)>,
    /// The deletion memory cannot exceed [`DELETION_MEMORY_CAP`].
    deleted: BTreeMap<SessionKey, Deletion>,
    /// The lowest `absent_at` leaves the deletion memory first.
    deleted_by_revision: BTreeSet<(Revision, SessionKey)>,
    /// Eviction preserves the highest removed absence revision as an admission guard.
    forgotten_through: Revision,
    /// Broad removals preserve their highest revision as an admission guard.
    broad_through: Revision,
    /// A full pending set blocks the carry instead of dropping facts.
    pending: BTreeMap<SessionKey, PendingAdmission>,
    /// Recently idle keys affect resume events only. They do not decide admission
    /// safety.
    recently_idle: HashMap<SessionKey, i64>,
    /// Only a covering generation or a deadline removes anonymous state.
    anonymous: BTreeMap<AgentKind, AnonymousEntry>,
    reconcile: ReconcileState,
}

impl Registry {
    /// Apply the window, incarnation, deletion, and revision guards before admitting a
    /// key.
    fn establish(
        &mut self,
        key: &SessionKey,
        fact: Existence,
        now: i64,
        out: &mut Vec<SessionEvent>,
    ) -> Touch {
        if now - fact.at >= ACTIVE_SESSION_WINDOW_SECS {
            return Touch::Stale;
        }
        if let Some(entry) = self.live.get(key).copied() {
            match fact.incarnation.cmp(&entry.incarnation) {
                // The registry rejects activity from an older incarnation.
                std::cmp::Ordering::Less => return Touch::Stale,
                std::cmp::Ordering::Equal => {
                    let entry = self.live.get_mut(key).expect("the entry was read above");
                    entry.exists_at = entry.exists_at.max(fact.revision);
                    if fact.at <= entry.last_activity_at {
                        return Touch::Unchanged;
                    }
                    let resumed = entry.quiet_published;
                    self.deadlines
                        .remove(&(entry.deadline(), DeadlineKey::Session(key.clone())));
                    entry.last_activity_at = fact.at;
                    entry.quiet_published = false;
                    self.deadlines
                        .insert((entry.deadline(), DeadlineKey::Session(key.clone())));
                    if resumed {
                        self.working += 1;
                    }
                    return Touch::Advanced { resumed };
                }
                std::cmp::Ordering::Greater => {
                    // A higher incarnation proves the live incarnation no longer
                    // exists.
                    self.remove_live(key);
                    self.recently_idle.remove(key);
                    out.push(SessionEvent::Idle {
                        session: SessionRef::from(key),
                        agent: entry.agent,
                        at: now,
                    });
                    out.push(SessionEvent::Removed {
                        session: Some(SessionRef::from(key)),
                        reason: RemovalReason::Reconciled,
                    });
                    self.remember(key, entry.incarnation, fact.revision);
                }
            }
        }
        if self
            .deleted
            .get(key)
            .is_some_and(|deletion| deletion.incarnation >= fact.incarnation)
        {
            return Touch::Stale;
        }
        if fact.revision < self.forgotten_through || fact.revision < self.broad_through {
            return self.defer(key, fact);
        }
        let mut fact = fact;
        if let Some(pending) = self.pending.get(key).copied() {
            match pending.incarnation.cmp(&fact.incarnation) {
                std::cmp::Ordering::Greater => {
                    // Newer pending evidence proves this incarnation no longer exists.
                    self.remember(key, fact.incarnation, pending.revision);
                    return Touch::Stale;
                }
                std::cmp::Ordering::Equal => {
                    fact.at = fact.at.max(pending.at);
                    fact.revision = fact.revision.max(pending.revision);
                    fact.is_new |= pending.is_new;
                    self.pending.remove(key);
                }
                std::cmp::Ordering::Less => {
                    self.pending.remove(key);
                    self.remember(key, pending.incarnation, fact.revision);
                }
            }
        }
        let resumed = self.recently_idle.remove(key).is_some();
        self.insert_live(
            key.clone(),
            LiveEntry {
                agent: fact.agent,
                incarnation: fact.incarnation,
                exists_at: fact.revision,
                last_activity_at: fact.at,
                quiet_published: false,
            },
        );
        Touch::Added { resumed }
    }

    /// Hold a fact whose revision is below a guard until a page decides.
    fn defer(&mut self, key: &SessionKey, fact: Existence) -> Touch {
        if let Some(pending) = self.pending.get_mut(key) {
            match pending.incarnation.cmp(&fact.incarnation) {
                std::cmp::Ordering::Equal => {
                    pending.at = pending.at.max(fact.at);
                    pending.revision = pending.revision.max(fact.revision);
                    pending.is_new |= fact.is_new;
                }
                std::cmp::Ordering::Less => *pending = PendingAdmission::from(fact),
                // The pending evidence proves this incarnation no longer exists.
                std::cmp::Ordering::Greater => {}
            }
            return Touch::Deferred;
        }
        if self.pending.len() >= ADMISSION_PENDING_CAP {
            return Touch::Blocked;
        }
        self.pending
            .insert(key.clone(), PendingAdmission::from(fact));
        Touch::Deferred
    }

    /// A new identity produces `Started` only when it does not resume. Other activity
    /// changes produce `Activity`.
    fn narrate(
        &mut self,
        key: &SessionKey,
        agent: AgentKind,
        at: i64,
        is_new: bool,
        touch: Touch,
        out: &mut Vec<SessionEvent>,
    ) {
        match touch {
            // Only the pass cover identifies which anonymous generations it accounts
            // for.
            Touch::Added { resumed: false } if is_new => {
                out.push(SessionEvent::Started {
                    session: SessionRef::from(key),
                    agent,
                    at,
                });
            }
            Touch::Added { resumed } | Touch::Advanced { resumed } => {
                out.push(SessionEvent::Activity {
                    session: Some(SessionRef::from(key)),
                    agent,
                    at,
                    resumed,
                });
            }
            Touch::Unchanged | Touch::Stale | Touch::Deferred | Touch::Blocked => {}
        }
    }

    /// Apply a keyed removal: `(key, incarnation)` is absent at `revision`.
    fn remove(
        &mut self,
        key: &SessionKey,
        incarnation: Incarnation,
        reason: RemovalReason,
        revision: Revision,
        now: i64,
        out: &mut Vec<SessionEvent>,
    ) {
        self.remember(key, incarnation, revision);
        if let Some(entry) = self.live.get(key).copied() {
            if entry.incarnation > incarnation {
                return;
            }
            self.remove_live(key);
            self.recently_idle.remove(key);
            // The lifecycle reader needs `Idle` before removal eliminates the entry and
            // its deadline.
            out.push(SessionEvent::Idle {
                session: SessionRef::from(key),
                agent: entry.agent,
                at: now,
            });
            out.push(SessionEvent::Removed {
                session: Some(SessionRef::from(key)),
                reason,
            });
            return;
        }
        if let Some(pending) = self.pending.get(key).copied() {
            if pending.incarnation > incarnation {
                return;
            }
            self.pending.remove(key);
        }
        out.push(SessionEvent::Removed {
            session: Some(SessionRef::from(key)),
            reason,
        });
    }

    /// Apply a broad removal at `revision`. The presence walk checks older live
    /// entries.
    fn broad(&mut self, reason: RemovalReason, revision: Revision, out: &mut Vec<SessionEvent>) {
        self.broad_through = self.broad_through.max(revision);
        match self.reconcile.needs {
            Some((_, through)) if through >= revision => {}
            _ => self.reconcile.needs = Some((reason, revision)),
        }
        out.push(SessionEvent::Removed {
            session: None,
            reason,
        });
    }

    /// Remember the highest deleted incarnation and absence revision. Eviction raises
    /// `forgotten_through` to retain the guard.
    fn remember(&mut self, key: &SessionKey, incarnation: Incarnation, absent_at: Revision) {
        match self.deleted.get_mut(key) {
            Some(deletion) => {
                deletion.incarnation = deletion.incarnation.max(incarnation);
                if absent_at > deletion.absent_at {
                    self.deleted_by_revision
                        .remove(&(deletion.absent_at, key.clone()));
                    deletion.absent_at = absent_at;
                    self.deleted_by_revision.insert((absent_at, key.clone()));
                }
            }
            None => {
                self.deleted.insert(
                    key.clone(),
                    Deletion {
                        incarnation,
                        absent_at,
                    },
                );
                self.deleted_by_revision.insert((absent_at, key.clone()));
                if self.deleted.len() > DELETION_MEMORY_CAP
                    && let Some((evicted_at, evicted)) = self.deleted_by_revision.pop_first()
                {
                    self.deleted.remove(&evicted);
                    self.forgotten_through = self.forgotten_through.max(evicted_at);
                }
            }
        }
    }

    /// Apply an anonymous touch. A newer timestamp publishes `Activity`. A newer
    /// generation alone protects the entry from older covers. Out-of-window touches
    /// change nothing.
    fn anonymous(
        &mut self,
        agent: AgentKind,
        at: i64,
        generation: AnonymousGen,
        now: i64,
        out: &mut Vec<SessionEvent>,
    ) {
        if now - at >= QUIET_WINDOW_SECS {
            return;
        }
        match self.anonymous.get_mut(&agent) {
            Some(entry) => {
                if generation <= entry.generation {
                    return;
                }
                entry.generation = generation;
                if at <= entry.at {
                    return;
                }
                self.deadlines
                    .remove(&(entry.deadline(), DeadlineKey::Anonymous(agent)));
                entry.at = at;
                self.deadlines
                    .insert((entry.deadline(), DeadlineKey::Anonymous(agent)));
            }
            None => {
                let entry = AnonymousEntry { generation, at };
                self.deadlines
                    .insert((entry.deadline(), DeadlineKey::Anonymous(agent)));
                self.anonymous.insert(agent, entry);
            }
        }
        out.push(SessionEvent::Activity {
            session: None,
            agent,
            at,
            resumed: false,
        });
    }

    /// Apply covers only to generations at or below their limits.
    fn cover(&mut self, covers: &[AnonymousCover], now: i64, out: &mut Vec<SessionEvent>) {
        for cover in covers {
            let Some(entry) = self.anonymous.get(&cover.agent).copied() else {
                continue;
            };
            if entry.generation > cover.through {
                continue;
            }
            self.remove_anonymous(cover.agent, &entry);
            out.push(SessionEvent::AnonymousCleared {
                agent: cover.agent,
                at: now,
                cause: AnonymousClearCause::Resolved,
            });
        }
    }

    fn remove_anonymous(&mut self, agent: AgentKind, entry: &AnonymousEntry) {
        self.anonymous.remove(&agent);
        self.deadlines
            .remove(&(entry.deadline(), DeadlineKey::Anonymous(agent)));
    }

    /// Apply observations within the session fact budget. Return any unapplied
    /// `Indexed` remainder.
    fn observe(
        &mut self,
        observation: Observation,
        now: i64,
        budget: &mut usize,
        out: &mut Vec<SessionEvent>,
    ) -> Progress {
        match observation {
            Observation::Touched {
                session: touched,
                agent,
                at,
            } => {
                let fact = Existence {
                    agent,
                    incarnation: touched.incarnation,
                    at,
                    revision: touched.seen,
                    is_new: false,
                };
                let touch = self.establish(&touched.key, fact, now, out);
                if touch == Touch::Blocked {
                    return Progress::Blocked(Observation::Touched {
                        session: touched,
                        agent,
                        at,
                    });
                }
                *budget = budget.saturating_sub(1);
                self.narrate(&touched.key, agent, at, false, touch, out);
                Progress::Done
            }
            Observation::Anonymous {
                agent,
                at,
                generation,
            } => {
                *budget = budget.saturating_sub(1);
                self.anonymous(agent, at, generation, now, out);
                Progress::Done
            }
            Observation::AnonymousCovered { covers } => {
                *budget = budget.saturating_sub(1);
                self.cover(&covers, now, out);
                Progress::Done
            }
            Observation::Indexed { sessions, revision } => {
                for (index, indexed) in sessions.iter().enumerate() {
                    if *budget == 0 {
                        return Progress::Exhausted(Observation::Indexed {
                            sessions: sessions[index..].to_vec(),
                            revision,
                        });
                    }
                    let fact = Existence {
                        agent: indexed.agent,
                        incarnation: indexed.incarnation,
                        at: indexed.at,
                        revision,
                        is_new: indexed.is_new,
                    };
                    let touch = self.establish(&indexed.key, fact, now, out);
                    if touch == Touch::Blocked {
                        return Progress::Blocked(Observation::Indexed {
                            sessions: sessions[index..].to_vec(),
                            revision,
                        });
                    }
                    *budget -= 1;
                    self.narrate(
                        &indexed.key,
                        indexed.agent,
                        indexed.at,
                        indexed.is_new,
                        touch,
                        out,
                    );
                }
                Progress::Done
            }
            Observation::RowChanged {
                session,
                facets,
                at,
            } => {
                *budget = budget.saturating_sub(1);
                out.push(SessionEvent::Updated {
                    session: SessionRef::from(&session),
                    facets,
                    at,
                });
                Progress::Done
            }
            Observation::Removed {
                scope: RemovalScope::One(key, incarnation),
                reason,
                revision,
            } => {
                *budget = budget.saturating_sub(1);
                self.remove(&key, incarnation, reason, revision, now, out);
                Progress::Done
            }
            Observation::Removed {
                scope: RemovalScope::Broad,
                reason,
                revision,
            } => {
                *budget = budget.saturating_sub(1);
                self.broad(reason, revision, out);
                Progress::Done
            }
            Observation::IndexChanged { reason } => {
                *budget = budget.saturating_sub(1);
                out.push(SessionEvent::IndexChanged { reason });
                Progress::Done
            }
        }
    }

    /// Apply the page against current registry state. Newer facts can arrive while the
    /// read runs.
    fn apply_page(
        &mut self,
        request: &PageRequest,
        rows: Vec<Presence>,
        revision: Revision,
        now: i64,
        out: &mut Vec<SessionEvent>,
    ) {
        let reason = match request {
            PageRequest::Admission { .. } => RemovalReason::Reconciled,
            PageRequest::Active { .. } => {
                let page = RecoveryPage {
                    cursor: rows.last().map(ActiveCursor::after),
                    exhausted: rows.len() < RECONCILE_PAGE,
                    rows: rows.into(),
                    revision,
                };
                if let Some(recovery) = self.reconcile.recovery.as_mut() {
                    recovery.page = Some(page);
                }
                self.apply_recovery(now, out);
                return;
            }
            PageRequest::Walk { reason, .. } => *reason,
        };
        let mut present: HashMap<SessionKey, Presence> =
            rows.into_iter().map(|row| (row.key.clone(), row)).collect();
        for key in request.keys() {
            match present.remove(key) {
                Some(row) => {
                    self.apply_present_row(key, &row, revision, now, out);
                }
                None => self.apply_absent_row(key, reason, revision, now, out),
            }
        }
        if let PageRequest::Walk { keys, .. } = request
            && let Some(run) = self.reconcile.run.as_mut()
        {
            run.cursor = keys.last().cloned();
        }
    }

    /// The page confirms this incarnation exists at `revision`.
    fn apply_present_row(
        &mut self,
        key: &SessionKey,
        row: &Presence,
        revision: Revision,
        now: i64,
        out: &mut Vec<SessionEvent>,
    ) -> Touch {
        let (agent, at, is_new) = match self.pending.get(key).copied() {
            Some(pending) => match pending.incarnation.cmp(&row.incarnation) {
                std::cmp::Ordering::Equal => {
                    // Only the same incarnation retains the transient watcher time.
                    self.pending.remove(key);
                    (pending.agent, pending.at.max(row.epoch), pending.is_new)
                }
                std::cmp::Ordering::Less => {
                    // The new incarnation must not inherit the old incarnation’s
                    // activity time.
                    self.pending.remove(key);
                    self.remember(key, pending.incarnation, revision);
                    let Some(agent) = self.agent_for(key) else {
                        return Touch::Stale;
                    };
                    (agent, row.epoch, false)
                }
                std::cmp::Ordering::Greater => {
                    // The page is stale relative to newer pending evidence.
                    // The next page revalidates the pending entry.
                    self.remember(key, row.incarnation, pending.revision);
                    return Touch::Stale;
                }
            },
            None => {
                let Some(agent) = self.agent_for(key) else {
                    return Touch::Stale;
                };
                (agent, row.epoch, false)
            }
        };
        let fact = Existence {
            agent,
            incarnation: row.incarnation,
            at,
            revision,
            is_new,
        };
        let touch = self.establish(key, fact, now, out);
        self.narrate(key, agent, at, is_new, touch, out);
        touch
    }

    /// The page confirms the requested key is absent at `revision`.
    fn apply_absent_row(
        &mut self,
        key: &SessionKey,
        reason: RemovalReason,
        revision: Revision,
        now: i64,
        out: &mut Vec<SessionEvent>,
    ) {
        if let Some(pending) = self.pending.get(key).copied() {
            // A stale absence cannot identify which newer incarnation no longer exists.
            if pending.revision <= revision {
                self.remember(key, pending.incarnation, revision);
                self.pending.remove(key);
            }
            return;
        }
        if let Some(entry) = self.live.get(key).copied()
            && entry.exists_at <= revision
        {
            self.remove(key, entry.incarnation, reason, revision, now, out);
        }
    }

    fn agent_for(&self, key: &SessionKey) -> Option<AgentKind> {
        self.live
            .get(key)
            .map(|entry| entry.agent)
            .or_else(|| self.pending.get(key).map(|pending| pending.agent))
            .or_else(|| AgentKind::from_slug(&key.agent))
    }

    /// Keep the unaccepted suffix until admission capacity becomes available.
    fn apply_recovery(&mut self, now: i64, out: &mut Vec<SessionEvent>) {
        let Some(mut recovery) = self.reconcile.recovery.take() else {
            return;
        };
        if let Some(page) = recovery.page.as_mut() {
            while let Some(row) = page.rows.front() {
                if self.apply_present_row(&row.key, row, page.revision, now, out) == Touch::Blocked
                {
                    break;
                }
                page.rows.pop_front();
            }
            if page.rows.is_empty() {
                if page.exhausted {
                    return;
                }
                recovery.cursor = page.cursor.take();
                recovery.page = None;
            }
        }
        self.reconcile.recovery = Some(recovery);
    }

    fn recovery_ready(&self) -> bool {
        self.pending.len() < ADMISSION_PENDING_CAP
            && self
                .reconcile
                .recovery
                .as_ref()
                .is_some_and(|recovery| recovery.page.is_some())
    }

    /// Prune expired pending entries before selecting a page. Admission and walk pages
    /// share turns with startup recovery when each class has work.
    fn next_page_request(&mut self, now: i64) -> Option<PageRequest> {
        self.pending
            .retain(|_, pending| now - pending.at < ACTIVE_SESSION_WINDOW_SECS);
        // Apply the retained suffix before issuing another read when capacity is available.
        if self.recovery_ready() {
            return None;
        }
        let preferred = self.reconcile.turn;
        for class in [
            preferred,
            preferred.successor(),
            preferred.successor().successor(),
        ] {
            let request = match class {
                PageClass::Admission => self.next_admission_page(),
                PageClass::Walk => self.next_walk_page(),
                PageClass::Recovery => self.reconcile.recovery.as_ref().and_then(|recovery| {
                    recovery.page.is_none().then(|| PageRequest::Active {
                        since: recovery.since,
                        after: recovery.cursor.clone(),
                    })
                }),
            };
            if request.is_some() {
                self.reconcile.turn = class.successor();
                return request;
            }
        }
        None
    }

    fn next_admission_page(&self) -> Option<PageRequest> {
        if self.pending.is_empty() {
            return None;
        }
        Some(PageRequest::Admission {
            keys: self.pending.keys().take(RECONCILE_PAGE).cloned().collect(),
        })
    }

    /// Start a walk when necessary. A newer removal requirement starts another walk
    /// after this run completes.
    fn next_walk_page(&mut self) -> Option<PageRequest> {
        loop {
            if self.reconcile.run.is_none() {
                let (reason, through) = self.reconcile.needs.take()?;
                self.reconcile.run = Some(Run {
                    cursor: None,
                    through,
                    reason,
                });
            }
            let run = self.reconcile.run.clone().expect("a run was opened above");
            let lower = match &run.cursor {
                Some(cursor) => Bound::Excluded(cursor.clone()),
                None => Bound::Unbounded,
            };
            let keys = self
                .live
                .range((lower, Bound::Unbounded))
                .filter(|(_, entry)| entry.exists_at < run.through)
                .take(RECONCILE_PAGE)
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            if !keys.is_empty() {
                return Some(PageRequest::Walk {
                    keys,
                    reason: run.reason,
                });
            }
            self.reconcile.run = None;
            match self.reconcile.needs {
                Some((_, through)) if through > run.through => continue,
                _ => {
                    self.reconcile.needs = None;
                    return None;
                }
            }
        }
    }

    fn wants_page(&self) -> bool {
        !self.pending.is_empty()
            || self.reconcile.run.is_some()
            || self.reconcile.needs.is_some()
            || self
                .reconcile
                .recovery
                .as_ref()
                .is_some_and(|recovery| recovery.page.is_none())
    }

    fn insert_live(&mut self, key: SessionKey, entry: LiveEntry) {
        self.deadlines
            .insert((entry.deadline(), DeadlineKey::Session(key.clone())));
        if let Some(previous) = self.live.insert(key, entry)
            && !previous.quiet_published
        {
            self.working -= 1;
        }
        if !entry.quiet_published {
            self.working += 1;
        }
    }

    fn remove_live(&mut self, key: &SessionKey) -> Option<LiveEntry> {
        let entry = self.live.remove(key)?;
        self.deadlines
            .remove(&(entry.deadline(), DeadlineKey::Session(key.clone())));
        if !entry.quiet_published {
            self.working -= 1;
        }
        Some(entry)
    }

    /// Exact counts require no live-map walk.
    fn aggregate(&self) -> Aggregate {
        Aggregate {
            working: self.working,
            total: self.live.len(),
            anonymous: self.anonymous.len(),
        }
    }

    fn remember_idle(&mut self, key: SessionKey, idled_at: i64) {
        if self.recently_idle.len() >= RECENT_IDLE_CAP {
            let oldest = self
                .recently_idle
                .iter()
                .min_by_key(|(key, idled_at)| (**idled_at, (*key).clone()))
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.recently_idle.remove(&oldest);
            }
        }
        self.recently_idle.insert(key, idled_at);
    }

    /// Expire at most [`EXPIRE_BATCH`] deadlines in order. A session that crosses both
    /// windows in one wake gets only `Idle`.
    fn expire(&mut self, now: i64, out: &mut Vec<SessionEvent>) {
        let mut expired = 0;
        while expired < EXPIRE_BATCH
            && let Some((deadline, deadline_key)) = self.deadlines.first().cloned()
        {
            if deadline > now {
                break;
            }
            let key = match deadline_key {
                DeadlineKey::Session(key) => key,
                DeadlineKey::Anonymous(agent) => {
                    let Some(entry) = self.anonymous.get(&agent).copied() else {
                        // The index and the map must contain the same entry.
                        self.deadlines.remove(&(deadline, deadline_key));
                        continue;
                    };
                    if now - entry.at < QUIET_WINDOW_SECS {
                        // The supplied clock can precede the indexed deadline.
                        self.deadlines.remove(&(deadline, deadline_key));
                        self.deadlines
                            .insert((entry.deadline(), DeadlineKey::Anonymous(agent)));
                        break;
                    }
                    self.remove_anonymous(agent, &entry);
                    out.push(SessionEvent::AnonymousCleared {
                        agent,
                        at: now,
                        cause: AnonymousClearCause::Expired,
                    });
                    expired += 1;
                    continue;
                }
            };
            let Some(entry) = self.live.get(&key).copied() else {
                // The index and the map must contain the same entry.
                self.deadlines
                    .remove(&(deadline, DeadlineKey::Session(key)));
                continue;
            };
            if now - entry.last_activity_at >= ACTIVE_SESSION_WINDOW_SECS {
                self.remove_live(&key);
                self.remember_idle(key.clone(), now);
                out.push(SessionEvent::Idle {
                    session: SessionRef::from(&key),
                    agent: entry.agent,
                    at: now,
                });
                expired += 1;
            } else if !entry.quiet_published && now - entry.last_activity_at >= QUIET_WINDOW_SECS {
                self.deadlines
                    .remove(&(deadline, DeadlineKey::Session(key.clone())));
                let entry = self.live.get_mut(&key).expect("the entry was read above");
                entry.quiet_published = true;
                self.working -= 1;
                self.deadlines
                    .insert((entry.deadline(), DeadlineKey::Session(key.clone())));
                out.push(SessionEvent::Quiet {
                    session: SessionRef::from(&key),
                    agent: entry.agent,
                    at: now,
                });
                expired += 1;
            } else {
                // The supplied clock can precede the deadline. Retain the entry until
                // its computed deadline.
                self.deadlines
                    .remove(&(deadline, DeadlineKey::Session(key.clone())));
                self.deadlines
                    .insert((entry.deadline(), DeadlineKey::Session(key)));
                break;
            }
        }
    }
}

impl From<Existence> for PendingAdmission {
    fn from(fact: Existence) -> Self {
        Self {
            agent: fact.agent,
            incarnation: fact.incarnation,
            at: fact.at,
            revision: fact.revision,
            is_new: fact.is_new,
        }
    }
}

/// Spill folding retains the highest incarnation and revision. The highest revision
/// supplies the removal reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemovedCell {
    incarnation: Incarnation,
    reason: RemovalReason,
    revision: Revision,
}

/// The spill combines sync facts by maximum values or unions. Overflow converts facts
/// to broader removals or invalidations. No spill fact establishes presence.
#[derive(Debug, Default)]
struct Spill {
    removed: BTreeMap<SessionKey, RemovedCell>,
    rows: BTreeMap<SessionKey, (UpdateFacets, i64)>,
    index_changed: BTreeSet<IndexChangeReason>,
    broad: Option<(RemovalReason, Revision)>,
}

impl Spill {
    fn fold(&mut self, observation: SyncObservation) {
        match observation {
            SyncObservation::RowChanged {
                session,
                facets,
                at,
            } => {
                if let Some((cell_facets, cell_at)) = self.rows.get_mut(&session) {
                    cell_facets.merge(facets);
                    *cell_at = (*cell_at).max(at);
                } else if self.rows.len() < SPILL_KEY_CAP {
                    self.rows.insert(session, (facets, at));
                } else {
                    // Overflow converts the row patch to a list refresh.
                    self.index_changed.insert(IndexChangeReason::Invalidated);
                }
            }
            SyncObservation::Removed {
                scope: RemovalScope::One(key, incarnation),
                reason,
                revision,
            } => {
                if let Some(cell) = self.removed.get_mut(&key) {
                    cell.incarnation = cell.incarnation.max(incarnation);
                    if revision > cell.revision {
                        cell.revision = revision;
                        cell.reason = reason;
                    }
                } else if self.removed.len() < SPILL_KEY_CAP {
                    self.removed.insert(
                        key,
                        RemovedCell {
                            incarnation,
                            reason,
                            revision,
                        },
                    );
                } else {
                    // Overflow converts the keyed removal to a broad removal. The
                    // presence walk checks the row.
                    self.fold_broad(reason, revision);
                }
            }
            SyncObservation::Removed {
                scope: RemovalScope::Broad,
                reason,
                revision,
            } => self.fold_broad(reason, revision),
            SyncObservation::IndexChanged { reason } => {
                self.index_changed.insert(reason);
            }
        }
    }

    fn fold_broad(&mut self, reason: RemovalReason, revision: Revision) {
        match self.broad {
            Some((_, through)) if through >= revision => {}
            _ => self.broad = Some((reason, revision)),
        }
    }

    fn is_empty(&self) -> bool {
        self.removed.is_empty()
            && self.rows.is_empty()
            && self.index_changed.is_empty()
            && self.broad.is_none()
    }
}

/// Tauri manages the bus and actor state together. Subscribers recover through
/// versioned snapshots rather than event replay.
pub struct SessionEvents {
    inbox: mpsc::Sender<Observation>,
    /// Only one actor can take this receiver.
    pending_inbox: Mutex<Option<mpsc::Receiver<Observation>>>,
    spill: Mutex<Spill>,
    spill_wake: Notify,
    bus: broadcast::Sender<Sequenced>,
    registry: Mutex<Registry>,
    #[cfg(test)]
    rounds: std::sync::atomic::AtomicU64,
}

impl Default for SessionEvents {
    fn default() -> Self {
        let (bus, _) = broadcast::channel(BUS_CAPACITY);
        let (inbox, receiver) = mpsc::channel(INBOX_CAPACITY);
        Self {
            inbox,
            pending_inbox: Mutex::new(Some(receiver)),
            spill: Mutex::new(Spill::default()),
            spill_wake: Notify::new(),
            bus,
            registry: Mutex::new(Registry::default()),
            #[cfg(test)]
            rounds: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl SessionEvents {
    /// Subscribe to future events. Broadcast lag requires snapshot recovery.
    pub fn subscribe(&self) -> broadcast::Receiver<Sequenced> {
        self.bus.subscribe()
    }

    /// Send one observation, waiting for inbox capacity. The scan caller must hold no
    /// Store guard while it waits.
    pub async fn report_async(&self, observation: Observation) {
        if self.inbox.send(observation).await.is_err() {
            ::tracing::debug!(event = "session_lifecycle_inbox_closed");
        }
    }

    /// Send one sync observation without waiting for inbox capacity. Spill access can
    /// briefly wait for its mutex. A closed inbox drops the report with a log.
    pub fn report(&self, observation: SyncObservation) {
        match self.inbox.try_reserve() {
            Ok(permit) => permit.send(Observation::from(observation)),
            Err(mpsc::error::TrySendError::Full(())) => {
                self.spill
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .fold(observation);
                self.spill_wake.notify_one();
            }
            Err(mpsc::error::TrySendError::Closed(())) => {
                ::tracing::debug!(event = "session_lifecycle_inbox_closed");
            }
        }
    }

    /// Return bounded recent rows with exact counts and complete anonymous state. Clone
    /// under the registry lock. Sort after releasing the lock.
    pub fn snapshot(&self, limit: usize) -> LiveSnapshot {
        let (seq, aggregate, mut sessions, anonymous) = {
            let registry = self
                .registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let sessions = registry
                .live
                .iter()
                .map(|(key, entry)| live_session(key, entry))
                .collect::<Vec<_>>();
            let anonymous = registry
                .anonymous
                .iter()
                .map(|(agent, entry)| LiveAnonymous {
                    agent: *agent,
                    last_activity_at: entry.at,
                })
                .collect::<Vec<_>>();
            (registry.seq, registry.aggregate(), sessions, anonymous)
        };
        // Full identity order keeps equal activity epochs deterministic.
        sessions.sort_by(|a, b| {
            b.last_activity_at
                .cmp(&a.last_activity_at)
                .then_with(|| a.session.environment_key.cmp(&b.session.environment_key))
                .then_with(|| a.session.agent.cmp(&b.session.agent))
                .then_with(|| a.session.session_id.cmp(&b.session.session_id))
        });
        sessions.truncate(limit);
        LiveSnapshot {
            seq,
            working: aggregate.working,
            total: aggregate.total,
            sessions,
            anonymous,
        }
    }

    /// Read named presence under one registry lock. The caller bounds the request.
    /// Duplicate identities receive one answer.
    pub fn presence(&self, sessions: &[SessionRef]) -> LivePresence {
        let registry = self
            .registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut present = Vec::new();
        let mut absent = Vec::new();
        let mut seen = HashSet::with_capacity(sessions.len());
        for session in sessions {
            let key = SessionKey::from(session);
            if !seen.insert(key.clone()) {
                continue;
            }
            match registry.live.get(&key) {
                Some(entry) => present.push(live_session(&key, entry)),
                None => absent.push(session.clone()),
            }
        }
        LivePresence {
            seq: registry.seq,
            present,
            absent,
        }
    }

    /// Read the last canonical event sequence.
    pub fn current_seq(&self) -> u64 {
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .seq
    }

    /// Seed one page before the actor runs. Rows retain the page revision. Unknown
    /// agents and out-of-window rows do not enter the registry.
    fn seed(&self, rows: Vec<Presence>, revision: Revision, now: i64) {
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for row in rows {
            if now - row.epoch >= ACTIVE_SESSION_WINDOW_SECS {
                continue;
            }
            let Some(agent) = AgentKind::from_slug(&row.key.agent) else {
                continue;
            };
            registry.insert_live(
                row.key,
                LiveEntry {
                    agent,
                    incarnation: row.incarnation,
                    exists_at: revision,
                    last_activity_at: row.epoch,
                    // The snapshot supplies quiet state without publishing a seed
                    // event.
                    quiet_published: now - row.epoch >= QUIET_WINDOW_SECS,
                },
            );
        }
    }

    /// Claim the inbox once for the actor.
    fn claim_actor(&self) -> Option<mpsc::Receiver<Observation>> {
        self.pending_inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    /// Take the whole spill, or `None` when it is empty.
    fn take_spill(&self) -> Option<Spill> {
        let mut spill = self
            .spill
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if spill.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut *spill))
    }
}

fn live_session(key: &SessionKey, entry: &LiveEntry) -> LiveSession {
    LiveSession {
        session: SessionRef::from(key),
        agent: entry.agent,
        last_activity_at: entry.last_activity_at,
        quiet: entry.quiet_published,
    }
}

/// Report a sync fact without waiting for inbox capacity. Test apps without managed
/// state report nothing.
pub fn report(app: &AppHandle, observation: SyncObservation) {
    if let Some(events) = app.try_state::<SessionEvents>() {
        events.report(observation);
    }
}

/// Report a scan fact, waiting for inbox capacity. Test apps without managed state
/// report nothing.
pub async fn report_async(app: &AppHandle, observation: Observation) {
    if let Some(events) = app.try_state::<SessionEvents>() {
        events.report_async(observation).await;
    }
}

/// Seed the registry synchronously before starting the actor. Shutdown aborts the
/// returned task with the other schedulers.
pub fn spawn(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let events = app.state::<SessionEvents>();
    // Tokio elapsed time lets paused-clock tests control lifecycle deadlines.
    let base_epoch = crate::scan::unix_now();
    let base_instant = Instant::now();
    let source: Arc<dyn ReconcileSource> = Arc::new((*app.state::<Store>()).clone());
    let inbox = events.claim_actor();
    if inbox.is_some() {
        seed(&events, source.as_ref(), base_epoch);
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(inbox) = inbox else {
            ::tracing::error!(event = "session_lifecycle_spawned_twice");
            return;
        };
        let events = app.state::<SessionEvents>();
        let now = move || base_epoch + base_instant.elapsed().as_secs() as i64;
        run(&events, inbox, source, &now).await;
    })
}

/// Seed silently before startup. Retain a failed cursor for guarded recovery after startup.
fn seed(events: &SessionEvents, source: &dyn ReconcileSource, now: i64) {
    let since = now - ACTIVE_SESSION_WINDOW_SECS;
    let mut cursor: Option<ActiveCursor> = None;
    loop {
        let Ok((page, revision)) = source.active(since, cursor.as_ref(), RECONCILE_PAGE) else {
            events
                .registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .reconcile
                .recovery = Some(SeedRecovery {
                since,
                cursor,
                page: None,
            });
            return;
        };
        let last_page = page.len() < RECONCILE_PAGE;
        cursor = page.last().map(ActiveCursor::after);
        events.seed(page, revision, now);
        if last_page {
            return;
        }
    }
}

struct Actor {
    /// The carry retains a blocked head fact while admission waits for capacity.
    carry: VecDeque<Observation>,

    batch: Vec<Observation>,
    carry_blocked: bool,
    in_flight: Option<JoinHandle<PageOutcome>>,
    backoff: Duration,
    /// Failed pages cannot retry before this instant.
    next_allowed: Option<Instant>,
}

/// Each round services expiry, one completed page, the spill, and a bounded batch of
/// facts. Per-source quotas prevent starvation. No await holds a registry lock.
async fn run(
    events: &SessionEvents,
    mut inbox: mpsc::Receiver<Observation>,
    source: Arc<dyn ReconcileSource>,
    now: &(dyn Fn() -> i64 + Send + Sync),
) {
    let recovering = events
        .registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .reconcile
        .recovery
        .is_some();
    let mut actor = Actor {
        carry: VecDeque::new(),
        batch: Vec::with_capacity(DRAIN_BATCH),
        carry_blocked: false,
        in_flight: None,
        backoff: if recovering {
            RECONCILE_BACKOFF_MIN * 2
        } else {
            RECONCILE_BACKOFF_MIN
        },
        next_allowed: recovering.then(|| Instant::now() + RECONCILE_BACKOFF_MIN),
    };
    loop {
        let deadline = soonest_deadline(&events.registry);
        let (wants_page, recovery_ready) = {
            let registry = events
                .registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (registry.wants_page(), registry.recovery_ready())
        };
        let mut finished = None;
        tokio::select! {
            biased;
            () = events.spill_wake.notified() => {}
            outcome = join_page(&mut actor.in_flight), if actor.in_flight.is_some() => {
                actor.in_flight = None;
                finished = Some(outcome);
            }
            () = std::future::ready(()), if recovery_ready => {}
            () = std::future::ready(()), if !actor.carry.is_empty() && !actor.carry_blocked => {}
            () = sleep_until_deadline(deadline, now) => {}
            () = sleep_until_instant(actor.next_allowed),
                if actor.in_flight.is_none() && actor.next_allowed.is_some() && wants_page => {}
            received = inbox.recv_many(&mut actor.batch, DRAIN_BATCH), if actor.carry.is_empty() => {
                if received == 0 {
                    return;
                }
                actor.carry.extend(actor.batch.drain(..));
            }
        }
        #[cfg(test)]
        events
            .rounds
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let now_epoch = now();
        expire(events, now_epoch);
        if finished.is_none()
            && let Some(handle) = actor.in_flight.as_mut()
            && handle.is_finished()
        {
            finished = Some(join_page(&mut actor.in_flight).await);
            actor.in_flight = None;
        }
        if let Some(outcome) = finished {
            apply_page(events, &mut actor, outcome, now_epoch);
        } else if recovery_ready {
            apply(events, |registry| {
                let mut out = Vec::new();
                registry.apply_recovery(now_epoch, &mut out);
                (out, ())
            });
        }
        if let Some(spill) = events.take_spill() {
            apply_spill(events, spill, now_epoch);
        }
        if actor.carry.is_empty() {
            receive_ready(&mut inbox, &mut actor.carry);
        }
        actor.carry_blocked = apply_facts(events, &mut actor.carry, now_epoch);
        if actor.in_flight.is_none()
            && actor
                .next_allowed
                .is_none_or(|allowed| Instant::now() >= allowed)
        {
            actor.in_flight = issue_page(events, &source, now_epoch);
            // Page selection can prune pending entries and free capacity without issuing a page.
            actor.carry_blocked &= events
                .registry
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pending
                .len()
                == ADMISSION_PENDING_CAP;
        }
        tokio::task::yield_now().await;
    }
}

/// Receive whatever the inbox holds now, up to [`DRAIN_BATCH`], without
/// waiting.
fn receive_ready(inbox: &mut mpsc::Receiver<Observation>, carry: &mut VecDeque<Observation>) {
    while carry.len() < DRAIN_BATCH {
        match inbox.try_recv() {
            Ok(observation) => carry.push_back(observation),
            Err(_) => return,
        }
    }
}

/// Wait for the pending page. A panic or cancellation returns a read error.
async fn join_page(in_flight: &mut Option<JoinHandle<PageOutcome>>) -> PageOutcome {
    match in_flight.as_mut() {
        Some(handle) => match handle.await {
            Ok(outcome) => outcome,
            Err(error) => PageOutcome {
                request: PageRequest::Admission { keys: Vec::new() },
                result: Err(anyhow::anyhow!("presence page task failed: {error}")),
            },
        },
        None => std::future::pending().await,
    }
}

/// Apply at most [`DRAIN_WEIGHT`] facts in order. Return true when a full pending set
/// blocks the head.
fn apply_facts(events: &SessionEvents, carry: &mut VecDeque<Observation>, now: i64) -> bool {
    let mut budget = DRAIN_WEIGHT;
    while budget > 0
        && let Some(observation) = carry.pop_front()
    {
        let progress = apply(events, |registry| {
            let mut out = Vec::new();
            let progress = registry.observe(observation, now, &mut budget, &mut out);
            (out, progress)
        });
        match progress {
            Progress::Done => {}
            Progress::Exhausted(remainder) => {
                carry.push_front(remainder);
                return false;
            }
            Progress::Blocked(remainder) => {
                carry.push_front(remainder);
                return true;
            }
        }
    }
    false
}

/// Apply each spill cell atomically in row, keyed-removal, broad-removal, then index
/// order.
fn apply_spill(events: &SessionEvents, spill: Spill, now: i64) {
    for (session, (facets, at)) in spill.rows {
        publish(
            events,
            vec![SessionEvent::Updated {
                session: SessionRef::from(&session),
                facets,
                at,
            }],
        );
    }
    for (key, cell) in spill.removed {
        apply(events, |registry| {
            let mut out = Vec::new();
            registry.remove(
                &key,
                cell.incarnation,
                cell.reason,
                cell.revision,
                now,
                &mut out,
            );
            (out, ())
        });
    }
    if let Some((reason, revision)) = spill.broad {
        apply(events, |registry| {
            let mut out = Vec::new();
            registry.broad(reason, revision, &mut out);
            (out, ())
        });
    }
    for reason in spill.index_changed {
        publish(events, vec![SessionEvent::IndexChanged { reason }]);
    }
}

/// Choose the next page under the registry lock. Release the lock before starting its
/// blocking read.
fn issue_page(
    events: &SessionEvents,
    source: &Arc<dyn ReconcileSource>,
    now: i64,
) -> Option<JoinHandle<PageOutcome>> {
    let request = events
        .registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .next_page_request(now)?;
    let source = Arc::clone(source);
    Some(tokio::task::spawn_blocking(move || {
        let result = match &request {
            PageRequest::Active { since, after } => {
                source.active(*since, after.as_ref(), RECONCILE_PAGE)
            }
            _ => source.presence(request.keys()),
        };
        PageOutcome { request, result }
    }))
}

/// Apply a completed page or delay its retry. A read failure preserves the cursor and
/// requirements.
fn apply_page(events: &SessionEvents, actor: &mut Actor, outcome: PageOutcome, now: i64) {
    match outcome.result {
        Ok((rows, revision)) => {
            actor.backoff = RECONCILE_BACKOFF_MIN;
            actor.next_allowed = None;
            apply(events, |registry| {
                let mut out = Vec::new();
                registry.apply_page(&outcome.request, rows, revision, now, &mut out);
                (out, ())
            });
        }
        Err(error) => {
            ::tracing::warn!(
                event = "session_lifecycle_page_failed",
                error = %error,
                retry_in_secs = actor.backoff.as_secs(),
            );
            actor.next_allowed = Some(Instant::now() + actor.backoff);
            actor.backoff = (actor.backoff * 2).min(RECONCILE_BACKOFF_MAX);
        }
    }
}

/// Read the earliest session or anonymous deadline.
fn soonest_deadline(registry: &Mutex<Registry>) -> Option<i64> {
    registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .deadlines
        .first()
        .map(|(deadline, _)| *deadline)
}

/// Wait until the deadline plus slack. No deadline means this select arm remains
/// pending.
async fn sleep_until_deadline(deadline: Option<i64>, now: &(dyn Fn() -> i64 + Send + Sync)) {
    match deadline {
        Some(deadline) => {
            let wait_secs = (deadline + EXPIRY_SLACK_SECS - now()).max(0);
            tokio::time::sleep(Duration::from_secs(wait_secs as u64)).await;
        }
        None => std::future::pending().await,
    }
}

/// Sleep until `instant`, or forever when there is none.
async fn sleep_until_instant(instant: Option<Instant>) {
    match instant {
        Some(instant) => tokio::time::sleep_until(instant).await,
        None => std::future::pending().await,
    }
}

/// Expire at most [`EXPIRE_BATCH`] session or anonymous deadlines.
fn expire(events: &SessionEvents, now: i64) {
    apply(events, |registry| {
        let mut out = Vec::new();
        registry.expire(now, &mut out);
        (out, ())
    });
}

/// Update state and assign canonical sequences under one lock. Attach exact counts to
/// the last lifecycle event. Publish after releasing the lock.
fn apply<T>(
    events: &SessionEvents,
    mutate: impl FnOnce(&mut Registry) -> (Vec<SessionEvent>, T),
) -> T {
    let (sequenced, value) = {
        let mut registry = events
            .registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (out, value) = mutate(&mut registry);
        let last_lifecycle = out.iter().rposition(SessionEvent::is_lifecycle);
        let aggregate = registry.aggregate();
        let sequenced = out
            .into_iter()
            .enumerate()
            .map(|(index, event)| {
                registry.seq += 1;
                Sequenced {
                    seq: registry.seq,
                    event,
                    aggregate: (last_lifecycle == Some(index)).then_some(aggregate),
                }
            })
            .collect::<Vec<_>>();
        (sequenced, value)
    };
    for event in sequenced {
        // No subscribers is valid when all readers are closed.
        let _ = events.bus.send(event);
    }
    value
}

/// Publish events without state changes. The registry lock still assigns their
/// sequences.
fn publish(events: &SessionEvents, out: Vec<SessionEvent>) {
    apply(events, move |_| (out, ()));
}

#[cfg(test)]
pub(crate) mod tests;
