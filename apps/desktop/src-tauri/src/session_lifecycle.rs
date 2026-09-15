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
//! Two reporter classes exist. The scan task reports through
//! [`report_async`], which waits for inbox capacity and never loses a fact.
//! Every other producer reports a [`SyncObservation`] through [`report`],
//! which never waits: a full inbox folds the fact into a bounded spill. The
//! sync shape has no establishing variant, so no fact that says "this
//! session exists" can ever be folded or reordered.
//!
//! Every existence fact carries the store [`Revision`] it was read at and
//! the row's [`Incarnation`]. The registry admits a key only when no
//! remembered or forgotten deletion can be newer than the fact; otherwise it
//! defers the key and asks the store for a presence page through an
//! injected [`ReconcileSource`] on the blocking pool. The actor itself never
//! touches the store.
//!
//! Anonymous activity is a watched write under an agent root that maps to
//! no indexed session. Each such touch carries an [`AnonymousGen`] from the
//! scan scheduler's ledger. The registry keeps the highest generation per
//! agent and clears it only when a successful pass covers that generation
//! ([`Observation::AnonymousCovered`]) or when the registry's own
//! [`QUIET_WINDOW_SECS`] deadline passes. `Started` never clears it.
//!
//! Every published event carries a registry sequence number. A snapshot
//! carries the sequence at the time of the clone, so a subscriber can take
//! a snapshot, then apply only the deltas with a higher sequence.
//!
//! The registry keeps exact counts: how many sessions work, how many are
//! live, and how many agents have anonymous activity. Every atomic batch
//! stamps them on its last lifecycle event as an [`Aggregate`], and every
//! snapshot carries them, so a reader never counts a bounded row list. A
//! reader that needs the state of named identities beyond the snapshot's
//! rows asks for a [`LivePresence`] at one sequence.

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

/// How many events a subscriber can fall behind before it is told it lagged.
/// This is not a no-lag claim: the projection worker relays a resync when
/// it lags.
pub const BUS_CAPACITY: usize = 1024;

/// How many observations wait for the actor. The async reporter waits for
/// room past this; the sync reporter folds into the spill.
const INBOX_CAPACITY: usize = 256;

/// How many observations one service round receives from the inbox.
const DRAIN_BATCH: usize = 64;

/// How many session-level facts one service round applies. The rest stay in
/// the carry for the next round.
const DRAIN_WEIGHT: usize = 256;

/// How many sessions one `Indexed` observation carries at most.
pub(crate) const INDEXED_CHUNK: usize = 256;

/// How many due deadlines one service round expires.
const EXPIRE_BATCH: usize = 256;

/// How many keyed cells each spill map holds. Past this, a keyed removal
/// becomes a broad removal and a row change becomes one list refetch.
const SPILL_KEY_CAP: usize = 1024;

/// How many deletions the registry remembers. Eviction raises
/// `forgotten_through`, so an evicted deletion stays a guard.
const DELETION_MEMORY_CAP: usize = 1024;

/// How many keys wait for a presence page. Past this, the actor holds the
/// inbox instead of dropping a fact.
const ADMISSION_PENDING_CAP: usize = 1024;

/// How many keys or rows one presence or seed page carries.
const RECONCILE_PAGE: usize = 256;

/// The first wait after a failed page. Each further failure doubles it.
const RECONCILE_BACKOFF_MIN: Duration = Duration::from_secs(2);

/// The longest wait after a failed page.
const RECONCILE_BACKOFF_MAX: Duration = Duration::from_secs(30);

/// How many recently idled sessions the registry remembers, so a later
/// write narrates a resume. This memory has no correctness role.
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

/// The identity of one session, as the webview receives it and names it
/// in a presence request.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalReason {
    /// The reader or a worker deleted the row.
    Deleted,
    /// Retention purged the row.
    Purged,
    /// The scan gate rejected the source and removed the row.
    Rejected,
    /// The registry learned from the store that the row is gone, or that a
    /// newer incarnation of the key replaced it.
    Reconciled,
}

/// Why the session list as a whole changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexChangeReason {
    /// A scan pass changed list membership.
    ScanPass,
    /// A broad invalidation: readers refetch list data.
    Invalidated,
}

/// Why anonymous activity for an agent cleared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnonymousClearCause {
    /// A successful pass covered every generation the registry held.
    Resolved,
    /// The registry's quiet deadline passed without a covering pass.
    Expired,
}

/// The causal order of one anonymous touch. The scan scheduler issues one
/// per anonymous report from a checked monotonic counter. A cover names the
/// highest generation a pass accounts for; time never decides a cover.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnonymousGen(pub u64);

/// One agent's anonymous generations a successful pass accounts for: every
/// generation at or below `through`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnonymousCover {
    pub agent: AgentKind,
    pub through: AnonymousGen,
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
    /// The agent's anonymous activity cleared: a pass covered it, or the
    /// registry's quiet deadline passed. Only the registry clears it.
    AnonymousCleared {
        agent: AgentKind,
        at: i64,
        cause: AnonymousClearCause,
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
    /// Events were lost on the way to a reader. Only the projection bridge
    /// publishes this, on transport lag; the registry never does. Readers
    /// re-read the snapshot.
    Resync,
}

impl SessionEvent {
    /// True for the events the bridge relays on `session:lifecycle`. The
    /// other events reach readers as row projections or index changes.
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

/// The registry's exact counts after one atomic batch: sessions that work,
/// sessions that are live (working or quiet), and agents with anonymous
/// activity. The last lifecycle event of the batch carries them, so a
/// reader never derives liveness from a bounded row list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Aggregate {
    pub working: usize,
    pub total: usize,
    pub anonymous: usize,
}

/// One bus message: the event, the registry sequence it was published at,
/// and the batch's counts when this is the batch's last lifecycle event.
/// This is the internal shape; the webview receives [`LifecycleEnvelope`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sequenced {
    pub seq: u64,
    pub event: SessionEvent,
    pub aggregate: Option<Aggregate>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<Aggregate>,
}

/// One session a scan pass indexed: the row's incarnation, its activity
/// epoch, and the identity-level `is_new` flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedSession {
    pub key: SessionKey,
    pub agent: AgentKind,
    pub incarnation: Incarnation,
    pub at: i64,
    pub is_new: bool,
}

/// The session a watcher burst resolved: the row's incarnation and the
/// revision the lookup read it at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TouchedSession {
    pub key: SessionKey,
    pub incarnation: Incarnation,
    pub seen: Revision,
}

/// Which rows a removal names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemovalScope {
    /// One row, with the incarnation the delete returned.
    One(SessionKey, Incarnation),
    /// Rows the reporter cannot name. The registry checks every live entry
    /// that predates the revision against the store.
    Broad,
}

/// What a producer saw. Only the actor turns these into events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observation {
    /// A watcher burst touched a path that maps to this indexed session.
    Touched {
        session: TouchedSession,
        agent: AgentKind,
        at: i64,
    },
    /// A watcher burst touched a path under an agent root that maps to no
    /// indexed session. `generation` orders the touch against covers.
    Anonymous {
        agent: AgentKind,
        at: i64,
        generation: AnonymousGen,
    },
    /// A scan pass upserted these sessions. Each row existed at `revision`
    /// with its incarnation and activity epoch.
    Indexed {
        sessions: Vec<IndexedSession>,
        revision: Revision,
    },
    /// A successful scheduler pass accounted for these anonymous
    /// generations. It follows every `Indexed` report of that pass on the
    /// same ordered path.
    AnonymousCovered { covers: Vec<AnonymousCover> },
    /// A worker changed parts of one session's row.
    RowChanged {
        session: SessionKey,
        facets: UpdateFacets,
        at: i64,
    },
    /// Rows left the store by the transaction that ended at `revision`.
    Removed {
        scope: RemovalScope,
        reason: RemovalReason,
        revision: Revision,
    },
    /// List membership changed in a way no single row names.
    IndexChanged { reason: IndexChangeReason },
}

/// The subset a producer that must not wait may report. It has no
/// establishing variant, so the spill never holds evidence that a key
/// exists, and no fold can promote a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncObservation {
    /// A worker changed parts of one session's row.
    RowChanged {
        session: SessionKey,
        facets: UpdateFacets,
        at: i64,
    },
    /// Rows left the store by the transaction that ended at `revision`.
    Removed {
        scope: RemovalScope,
        reason: RemovalReason,
        revision: Revision,
    },
    /// List membership changed in a way no single row names.
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

/// One agent with anonymous activity inside the registry's quiet window,
/// as the snapshot command returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveAnonymous {
    pub agent: AgentKind,
    pub last_activity_at: i64,
}

/// A bounded, versioned view of the live registry. `seq` is the sequence of
/// the last event whose effect the snapshot includes: a subscriber applies
/// only deltas with a higher sequence. `working` and `total` are exact and
/// independent of the row limit: `sessions` holds at most the limit's most
/// recent rows, so a reader compares `sessions.len()` with `total` to know
/// whether the rows are complete. `anonymous` is complete: it is bounded by
/// the number of agent kinds, and a resync replaces it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSnapshot {
    pub seq: u64,
    pub working: usize,
    pub total: usize,
    pub sessions: Vec<LiveSession>,
    pub anonymous: Vec<LiveAnonymous>,
}

/// The registry's answer for named identities at one sequence: each
/// requested identity is live (`present`, with its registry state) or not
/// (`absent`). Both lists are read under one registry lock, so they agree
/// with `seq` and with each other.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LivePresence {
    pub seq: u64,
    pub present: Vec<LiveSession>,
    pub absent: Vec<SessionRef>,
}

/// The store reads the actor asks for. Every call is one blocking store
/// read; the actor runs it on the blocking pool and never holds a lock
/// while it waits.
pub trait ReconcileSource: Send + Sync {
    /// Which of `keys` exist, with incarnation and epoch, and the revision
    /// the rows were read at. At most [`RECONCILE_PAGE`] keys.
    fn presence(&self, keys: &[SessionKey]) -> anyhow::Result<(Vec<Presence>, Revision)>;

    /// One page of rows active since `since`, newest first, after `after`.
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
    /// The incarnation of the row this entry describes.
    incarnation: Incarnation,
    /// The highest revision at which the row was seen to exist.
    exists_at: Revision,
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

/// One agent's anonymous activity: the highest generation received and the
/// latest activity time. The generation decides covers; the time decides
/// the deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AnonymousEntry {
    generation: AnonymousGen,
    at: i64,
}

impl AnonymousEntry {
    /// The moment the entry expires without a cover.
    fn deadline(&self) -> i64 {
        self.at + QUIET_WINDOW_SECS
    }
}

/// What one deadline in the index belongs to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DeadlineKey {
    Session(SessionKey),
    Anonymous(AgentKind),
}

/// One remembered deletion: the highest deleted incarnation of the key and
/// the highest revision at which the key was seen absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Deletion {
    incarnation: Incarnation,
    absent_at: Revision,
}

/// One key that waits for a presence page before it can become live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingAdmission {
    agent: AgentKind,
    incarnation: Incarnation,
    at: i64,
    revision: Revision,
    is_new: bool,
}

/// One fact that says "this incarnation of the key exists at `revision`
/// with activity `at`".
#[derive(Clone, Copy, Debug)]
struct Existence {
    agent: AgentKind,
    incarnation: Incarnation,
    at: i64,
    revision: Revision,
    is_new: bool,
}

/// What one existence fact did to the registry.
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
    /// The fact is older than the window, names a dead incarnation, or is
    /// older than the last write of the same incarnation. It changes nothing.
    Stale,
    /// A deletion the registry no longer remembers exactly may be newer than
    /// the fact. The key waits for a presence page.
    Deferred,
    /// The key would wait, but the pending set is full. The caller keeps the
    /// fact and retries after a page frees room.
    Blocked,
}

/// How far one observation got in a service round.
#[derive(Debug)]
enum Progress {
    /// Applied in full.
    Done,
    /// The round's fact budget ran out. The remainder goes back on the carry.
    Exhausted(Observation),
    /// The pending set is full. The remainder goes back on the carry and the
    /// actor stops applying facts until a page frees room.
    Blocked(Observation),
}

/// Which page class the next free turn goes to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PageClass {
    #[default]
    Admission,
    Walk,
}

impl PageClass {
    fn other(self) -> Self {
        match self {
            Self::Admission => Self::Walk,
            Self::Walk => Self::Admission,
        }
    }
}

/// One presence walk over the live map: every entry that predates
/// `through` is checked against the store, in key order after `cursor`.
#[derive(Clone, Debug)]
struct Run {
    cursor: Option<SessionKey>,
    through: Revision,
    reason: RemovalReason,
}

/// The walk requirements and the page turn.
#[derive(Debug, Default)]
struct ReconcileState {
    /// A broad removal that no walk has covered yet.
    needs: Option<(RemovalReason, Revision)>,
    run: Option<Run>,
    turn: PageClass,
}

/// One presence page the actor asked the store for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PageRequest {
    /// Pending keys that wait for admission.
    Admission { keys: Vec<SessionKey> },
    /// Live keys a broad removal put in doubt.
    Walk {
        keys: Vec<SessionKey>,
        reason: RemovalReason,
    },
}

impl PageRequest {
    fn keys(&self) -> &[SessionKey] {
        match self {
            Self::Admission { keys } | Self::Walk { keys, .. } => keys,
        }
    }
}

/// One finished page: what was asked and what the store said.
struct PageOutcome {
    request: PageRequest,
    result: anyhow::Result<(Vec<Presence>, Revision)>,
}

/// The lifecycle state one lock guards: the live map, its deadline index,
/// the deletion memory and its guards, the pending admissions, the
/// recent-idle memory, keyless activity, the walk state, and the event
/// sequence.
#[derive(Default)]
struct Registry {
    seq: u64,
    /// Key order is the presence walk's cursor order.
    live: BTreeMap<SessionKey, LiveEntry>,
    /// How many live entries have no `Quiet` published. Every insert,
    /// removal, quiet, and resume moves it, so a batch never counts the map.
    working: usize,
    /// One entry per live session and per anonymous agent, keyed by its
    /// current deadline. Touches move entries, so the set never accumulates
    /// stale rows.
    deadlines: BTreeSet<(i64, DeadlineKey)>,
    /// Remembered deletions, bounded by [`DELETION_MEMORY_CAP`].
    deleted: BTreeMap<SessionKey, Deletion>,
    /// Eviction index over `deleted`: the lowest `absent_at` leaves first.
    deleted_by_revision: BTreeSet<(Revision, SessionKey)>,
    /// The highest `absent_at` ever evicted from `deleted`.
    forgotten_through: Revision,
    /// The highest revision of any broad removal applied.
    broad_through: Revision,
    /// Keys that wait for a presence page, bounded by
    /// [`ADMISSION_PENDING_CAP`]. The set never overflows: a fact that finds
    /// it full blocks the carry instead.
    pending: BTreeMap<SessionKey, PendingAdmission>,
    /// Recently idled sessions and when each idled, bounded by
    /// [`RECENT_IDLE_CAP`]. Narration only: a later write of the same
    /// incarnation resumes instead of starting.
    recently_idle: HashMap<SessionKey, i64>,
    /// Anonymous activity per agent. Bounded by the number of agent kinds.
    /// Only a covering generation or the deadline removes an entry.
    anonymous: BTreeMap<AgentKind, AnonymousEntry>,
    reconcile: ReconcileState,
}

impl Registry {
    /// Apply one existence fact. Steps follow the admission rules: the
    /// window, the live entry's incarnation, the deletion memory, the
    /// guards, then admission with any pending evidence for the key.
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
                // An older incarnation's activity is never imported.
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
                    // A higher incarnation exists, so the live one is dead.
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
                    // Newer pending evidence proves this incarnation dead.
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
                // The fact names an incarnation the pending evidence
                // already proves dead.
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

    /// Narrate what an existence fact did. `Started` needs a new identity
    /// that did not resume; everything else that moved is `Activity`.
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
            // `Started` never clears anonymous state: only the pass's own
            // cover says which generations it accounted for.
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
            // The lifecycle scope narrates the decay: without this, a
            // reader tracking working state waits for a `Quiet` or `Idle`
            // the registry can no longer publish.
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

    /// Apply a broad removal: rows the reporter cannot name were deleted at
    /// or before `revision`. Every live entry that predates it is walked.
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

    /// Remember that `incarnation` of `key` is dead and the key was absent
    /// at `absent_at`. Both values only grow. Past the cap, the entry with
    /// the lowest `absent_at` leaves and raises `forgotten_through`.
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

    /// Apply one anonymous touch. The entry keeps the highest generation
    /// and the latest time. A later time publishes `Activity`; a newer
    /// generation alone (a same-second write, or a clock that moved back)
    /// only protects the entry from an older cover. A touch already past
    /// the quiet window would expire at once, so it changes nothing.
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

    /// Apply one pass's covers. An entry clears only when its generation is
    /// at or below the cover; a newer generation survives.
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

    /// Apply one observation. `budget` counts session-level facts; an
    /// `Indexed` chunk stops at zero and returns its remainder.
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

    /// Apply one presence page against the registry's current state for
    /// every requested key. A key may have moved while the page was in
    /// flight; each rule below reads what the key is now.
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
            PageRequest::Walk { reason, .. } => *reason,
        };
        let mut present: HashMap<SessionKey, Presence> =
            rows.into_iter().map(|row| (row.key.clone(), row)).collect();
        for key in request.keys() {
            match present.remove(key) {
                Some(row) => self.apply_present_row(key, &row, revision, now, out),
                None => self.apply_absent_row(key, reason, revision, now, out),
            }
        }
        if let PageRequest::Walk { keys, .. } = request
            && let Some(run) = self.reconcile.run.as_mut()
        {
            run.cursor = keys.last().cloned();
        }
    }

    /// One page row: the key exists at `revision` with this incarnation.
    fn apply_present_row(
        &mut self,
        key: &SessionKey,
        row: &Presence,
        revision: Revision,
        now: i64,
        out: &mut Vec<SessionEvent>,
    ) {
        let (agent, at, is_new) = match self.pending.get(key).copied() {
            Some(pending) => match pending.incarnation.cmp(&row.incarnation) {
                std::cmp::Ordering::Equal => {
                    // The transient watcher time joins the page epoch.
                    self.pending.remove(key);
                    (pending.agent, pending.at.max(row.epoch), pending.is_new)
                }
                std::cmp::Ordering::Less => {
                    // A newer incarnation exists: the pending one is dead
                    // and its transient time is discarded, nothing else.
                    self.pending.remove(key);
                    self.remember(key, pending.incarnation, revision);
                    let Some(agent) = self.agent_for(key) else {
                        return;
                    };
                    (agent, row.epoch, false)
                }
                std::cmp::Ordering::Greater => {
                    // The page is stale relative to newer pending evidence.
                    // The next page revalidates the pending entry.
                    self.remember(key, row.incarnation, pending.revision);
                    return;
                }
            },
            None => {
                let Some(agent) = self.agent_for(key) else {
                    return;
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
    }

    /// One requested key the page did not return: absent at `revision`.
    fn apply_absent_row(
        &mut self,
        key: &SessionKey,
        reason: RemovalReason,
        revision: Revision,
        now: i64,
        out: &mut Vec<SessionEvent>,
    ) {
        if let Some(pending) = self.pending.get(key).copied() {
            // Pending evidence newer than the page keeps the entry: the
            // page cannot say which incarnation it failed to find.
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

    /// The agent of a key the registry knows, or the one its slug names.
    fn agent_for(&self, key: &SessionKey) -> Option<AgentKind> {
        self.live
            .get(key)
            .map(|entry| entry.agent)
            .or_else(|| self.pending.get(key).map(|pending| pending.agent))
            .or_else(|| AgentKind::from_slug(&key.agent))
    }

    /// Prune pending entries the window would reject, then choose the next
    /// page. Admission pages and walk pages alternate while both have work.
    fn next_page_request(&mut self, now: i64) -> Option<PageRequest> {
        self.pending
            .retain(|_, pending| now - pending.at < ACTIVE_SESSION_WINDOW_SECS);
        let preferred = self.reconcile.turn;
        for class in [preferred, preferred.other()] {
            let request = match class {
                PageClass::Admission => self.next_admission_page(),
                PageClass::Walk => self.next_walk_page(),
            };
            if request.is_some() {
                self.reconcile.turn = class.other();
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

    /// The next walk page, starting a run from `needs` when none is open.
    /// A run whose remaining entries are all at or above its revision is
    /// complete; a newer requirement starts another run.
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

    /// True when a page could do useful work now.
    fn wants_page(&self) -> bool {
        !self.pending.is_empty() || self.reconcile.run.is_some() || self.reconcile.needs.is_some()
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

    /// The exact counts now. Constant time: the map is never walked.
    fn aggregate(&self) -> Aggregate {
        Aggregate {
            working: self.working,
            total: self.live.len(),
            anonymous: self.anonymous.len(),
        }
    }

    /// Remember an idled session, evicting the oldest past the cap.
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

    /// Publish `Quiet`, `Idle`, and `AnonymousCleared` for at most
    /// [`EXPIRE_BATCH`] entries whose deadline passed, in deadline order. A
    /// session that crosses both windows in one wake gets `Idle` only.
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
                        // The index and the map move together; a miss is a bug.
                        self.deadlines.remove(&(deadline, deadline_key));
                        continue;
                    };
                    if now - entry.at < QUIET_WINDOW_SECS {
                        // Wake slack can fire before the deadline second.
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
                // The index and the map move together; a miss is a bug.
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
                // Wake slack can fire before the deadline second. Move the
                // entry to its computed deadline and wait again.
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

/// One folded keyed removal: the highest incarnation and revision reported
/// for the key, with the reason of the highest revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RemovedCell {
    incarnation: Incarnation,
    reason: RemovalReason,
    revision: Revision,
}

/// Where a sync report goes when the inbox is full. Every fold is a per-key
/// maximum or union; past a cap, a fact degrades to a broader canonical
/// fact. Nothing is dropped, and nothing here establishes a key.
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
                    // Past the cap, the row patch degrades to one list refetch.
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
                    // Past the cap, the keyed removal degrades to a broad
                    // one: the presence walk removes the row.
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

/// The bus and the actor's state, held in Tauri managed state.
///
/// `subscribe` gives a reader every event from now on. `report_async` and
/// `report` hand the actor an observation. `snapshot` is the versioned view
/// a late subscriber reads instead of a replay.
pub struct SessionEvents {
    inbox: mpsc::Sender<Observation>,
    /// The receiving end, taken once by the actor.
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
    /// A receiver that sees every event published after this call.
    pub fn subscribe(&self) -> broadcast::Receiver<Sequenced> {
        self.bus.subscribe()
    }

    /// Hand the actor one observation, waiting for inbox room. Only the
    /// scan task calls this: its wait is bounded by actor progress, never by
    /// a lock, and it holds no store guard while it waits.
    pub async fn report_async(&self, observation: Observation) {
        if self.inbox.send(observation).await.is_err() {
            ::tracing::debug!(event = "session_lifecycle_inbox_closed");
        }
    }

    /// Hand the actor one observation without waiting. A full inbox folds
    /// the fact into the spill; a closed inbox drops it with a log.
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

    /// The most recent live sessions, versioned and bounded to `limit`, the
    /// exact counts, and every agent with anonymous activity. The clone
    /// happens under the lock; sorting happens outside it.
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
        LiveSnapshot {
            seq,
            working: aggregate.working,
            total: aggregate.total,
            sessions,
            anonymous,
        }
    }

    /// The registry's state for the named identities, all read under one
    /// lock at one sequence. An identity the registry does not hold is
    /// `absent`; a duplicate in `sessions` is answered once. The caller
    /// bounds the request.
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

    /// The sequence of the last published event.
    pub fn current_seq(&self) -> u64 {
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .seq
    }

    /// Fill the live map from one seed page, before the actor runs. Every
    /// row exists at the page's revision. A row outside the window is
    /// skipped, and so is an agent slug the shell does not know.
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
                    // A row already past the quiet window has no `Quiet` to
                    // publish: the snapshot carries its epoch.
                    quiet_published: now - row.epoch >= QUIET_WINDOW_SECS,
                },
            );
        }
    }

    /// The inbox receiver, on the first call only. The caller becomes the
    /// actor.
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

/// One live entry as a snapshot or presence row.
fn live_session(key: &SessionKey, entry: &LiveEntry) -> LiveSession {
    LiveSession {
        session: SessionRef::from(key),
        agent: entry.agent,
        last_activity_at: entry.last_activity_at,
        quiet: entry.quiet_published,
    }
}

/// Report an observation from a context that must not wait. A test app
/// without managed state reports nothing.
pub fn report(app: &AppHandle, observation: SyncObservation) {
    if let Some(events) = app.try_state::<SessionEvents>() {
        events.report(observation);
    }
}

/// Report an observation from the scan task, waiting for inbox room. A
/// test app without managed state reports nothing.
pub async fn report_async(app: &AppHandle, observation: Observation) {
    if let Some(events) = app.try_state::<SessionEvents>() {
        events.report_async(observation).await;
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

/// Walk the store's active window one page at a time and seed each page
/// silently. A page that fails ends the walk with the rows read so far; the
/// launch pass re-reports them.
fn seed(events: &SessionEvents, source: &dyn ReconcileSource, now: i64) {
    let since = now - ACTIVE_SESSION_WINDOW_SECS;
    let mut cursor: Option<ActiveCursor> = None;
    loop {
        let Ok((page, revision)) = source.active(since, cursor.as_ref(), RECONCILE_PAGE) else {
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

/// The actor's own state between service rounds.
struct Actor {
    /// Observations received but not yet applied. The head may be a fact
    /// that is blocked on a full pending set.
    carry: VecDeque<Observation>,
    /// The receive buffer one wake fills before it joins the carry.
    batch: Vec<Observation>,
    carry_blocked: bool,
    in_flight: Option<JoinHandle<PageOutcome>>,
    backoff: Duration,
    /// Set after a failed page: no page is issued before this instant.
    next_allowed: Option<Instant>,
}

/// The loop [`spawn`] runs forever. Split out so a test can drive it with a
/// captured clock and a scripted source, without a Tauri app.
///
/// Each wake runs one service round in a fixed order: expiry, one finished
/// page, the spill, then a bounded batch of facts. Every source has a
/// quota, so no source waits more than one round for another. No arm holds
/// a lock across an await.
async fn run(
    events: &SessionEvents,
    mut inbox: mpsc::Receiver<Observation>,
    source: Arc<dyn ReconcileSource>,
    now: &(dyn Fn() -> i64 + Send + Sync),
) {
    let mut actor = Actor {
        carry: VecDeque::new(),
        batch: Vec::with_capacity(DRAIN_BATCH),
        carry_blocked: false,
        in_flight: None,
        backoff: RECONCILE_BACKOFF_MIN,
        next_allowed: None,
    };
    loop {
        let deadline = soonest_deadline(&events.registry);
        let wants_page = events
            .registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .wants_page();
        let mut finished = None;
        tokio::select! {
            biased;
            () = events.spill_wake.notified() => {}
            outcome = join_page(&mut actor.in_flight), if actor.in_flight.is_some() => {
                actor.in_flight = None;
                finished = Some(outcome);
            }
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

/// Wait for the page in flight. A page whose task panicked or was
/// cancelled reads as a failed page.
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

/// Apply at most [`DRAIN_WEIGHT`] session-level facts from the carry, in
/// order. Returns true when the head is blocked on a full pending set.
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

/// Apply a taken spill: rows, keyed removals, the broad removal, then the
/// index flags. Each cell is one atomic apply.
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

/// Choose and start the next page on the blocking pool, if any work wants
/// one. The registry lock is released before the task starts.
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
        let result = source.presence(request.keys());
        PageOutcome { request, result }
    }))
}

/// Apply one finished page, or back off after a failed one. A failure
/// keeps the cursor and every requirement.
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

/// Sleep until `instant`, or forever when there is none.
async fn sleep_until_instant(instant: Option<Instant>) {
    match instant {
        Some(instant) => tokio::time::sleep_until(instant).await,
        None => std::future::pending().await,
    }
}

/// Publish `Quiet` and `Idle` for at most [`EXPIRE_BATCH`] due sessions.
fn expire(events: &SessionEvents, now: i64) {
    apply(events, |registry| {
        let mut out = Vec::new();
        registry.expire(now, &mut out);
        (out, ())
    });
}

/// Mutate the registry, assign a sequence to every produced event under the
/// same lock, and publish after the lock is released. The batch's exact
/// counts ride on its last lifecycle event, never on an event the bridge
/// turns into a row or index change, so every reader of the lifecycle scope
/// sees the counts the batch ended with.
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
        // A bus with no subscriber returns an error, and that is not a
        // fault: the HUD may be closed.
        let _ = events.bus.send(event);
    }
    value
}

/// Publish events that mutate nothing. Sequencing still goes through the
/// registry lock.
fn publish(events: &SessionEvents, out: Vec<SessionEvent>) {
    apply(events, move |_| (out, ()));
}

#[cfg(test)]
mod tests;
