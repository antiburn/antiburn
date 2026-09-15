//! One projection worker, and one Tauri bridge, for every session event.
//!
//! The worker subscribes to the canonical lifecycle bus and owns all
//! session event emission to the webviews:
//!
//! - `session:lifecycle` relays lifecycle transitions, canonical anonymous
//!   clears, and resync metadata.
//! - `session:updated` carries one enriched row per coalesced `Updated`.
//! - `session:index-changed` carries membership and invalidation changes.
//!
//! Producers never emit these events; they report observations to the
//! registry. Row loads run on the blocking pool, off the lifecycle actor
//! and off this loop's relay path: the loop keeps receiving from the bus
//! while one load runs. Pending rows coalesce by session and flush on a
//! fixed deadline, so a continued event stream cannot starve the flush,
//! and a bounded cap degrades to one index refresh instead of an
//! unbounded patch queue.
//!
//! A row change that arrives while its row loads wins over the load: the
//! loaded row is not emitted, and its facets merge into the next batch. A
//! keyed removal drops the loading row. A broad removal, an index
//! invalidation, or transport lag invalidates the whole batch in flight,
//! because the refetch those tell readers to make supersedes every patch.
//!
//! On lag the bridge relays one `Resync` at the registry's current
//! sequence and schedules an index refresh. Further lag in the same
//! recovery cycle is coalesced; when it names a sequence above the one the
//! reader was told to resync at, a follow-up `Resync` is emitted when the
//! cycle closes, so coalescing cannot hide a newer gap. The bridge never
//! invents a sequence: every `seq` it emits is the registry's.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant};

use crate::dto::ActivityEntry;
use crate::session_lifecycle::{
    IndexChangeReason, LifecycleEnvelope, RemovalReason, Sequenced, SessionEvent, SessionEvents,
    SessionRef, UpdateFacets,
};
use crate::store::{SessionKey, Store};

/// How long the worker waits after the first pending row before it loads
/// and emits. The deadline is fixed when the batch opens: later events
/// coalesce into it, they do not move it. A deadline that passes while a
/// load runs starts the next batch as soon as that load ends.
const FLUSH_DELAY: Duration = Duration::from_millis(250);

/// How many rows one batch holds, pending or in flight. Pending rows past
/// this bound degrade to one index refresh, which tells readers to refetch
/// the list instead of patching rows one by one. One batch loads at a
/// time, so this also bounds the rows in flight.
const PENDING_ROW_CAP: usize = 512;

/// The `session:updated` payload: the enriched row plus what changed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdatedPayload {
    pub seq: u64,
    pub session: SessionRef,
    pub facets: UpdateFacets,
    pub entry: ActivityEntry,
}

/// The `session:index-changed` payload: why list membership changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexChangedPayload {
    pub seq: u64,
    pub cause: IndexChangeCause,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removal: Option<RemovalReason>,
}

/// What kind of index change a payload names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexChangeCause {
    /// A scan pass changed list membership.
    ScanPass,
    /// A broad invalidation: readers refetch list data.
    Invalidated,
    /// One session left the store.
    Removed,
    /// Events or rows were lost. Readers refetch the list and re-read the
    /// live snapshot.
    Resync,
}

impl From<IndexChangeReason> for IndexChangeCause {
    fn from(reason: IndexChangeReason) -> Self {
        match reason {
            IndexChangeReason::ScanPass => Self::ScanPass,
            IndexChangeReason::Invalidated => Self::Invalidated,
        }
    }
}

/// One pending row: merged facets and the latest epoch and sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingRow {
    facets: UpdateFacets,
    at: i64,
    seq: u64,
}

/// What the loop emits at once, before any row load.
#[derive(Clone, Debug, PartialEq)]
enum Immediate {
    /// Relay this event on `session:lifecycle`.
    Lifecycle(Sequenced),
    /// Emit this on `session:index-changed`.
    IndexChanged(IndexChangedPayload),
}

/// Everything the bridge hands to Tauri.
#[derive(Debug)]
pub(crate) enum Emission {
    /// Relay this event on `session:lifecycle`.
    Lifecycle(Sequenced),
    /// Emit this on `session:index-changed`.
    IndexChanged(IndexChangedPayload),
    /// Emit this enriched row on `session:updated`.
    Updated(Box<SessionUpdatedPayload>),
}

impl From<Immediate> for Emission {
    fn from(immediate: Immediate) -> Self {
        match immediate {
            Immediate::Lifecycle(sequenced) => Self::Lifecycle(sequenced),
            Immediate::IndexChanged(payload) => Self::IndexChanged(payload),
        }
    }
}

/// The coalesced work between flushes.
#[derive(Default)]
struct Pending {
    rows: HashMap<SessionKey, PendingRow>,
    /// Set when rows were lost to the cap, transport lag, or a registry
    /// resync. The next flush emits one index refresh instead of patches.
    index_refresh: bool,
    /// The highest sequence this worker has seen.
    last_seq: u64,
    /// Set when the next batch carries rows a failed load gave back. That
    /// batch gets no second retry: a further failure becomes one refetch.
    retry: bool,
}

impl Pending {
    /// Merge one row change in, bounded by [`PENDING_ROW_CAP`].
    fn note_row(&mut self, key: SessionKey, facets: UpdateFacets, at: i64, seq: u64) {
        if self.index_refresh {
            // A refetch supersedes every patch in this batch.
            return;
        }
        if let Some(row) = self.rows.get_mut(&key) {
            row.facets.merge(facets);
            row.at = row.at.max(at);
            row.seq = row.seq.max(seq);
            return;
        }
        if self.rows.len() >= PENDING_ROW_CAP {
            self.rows.clear();
            self.index_refresh = true;
            return;
        }
        self.rows.insert(key, PendingRow { facets, at, seq });
    }

    fn has_work(&self) -> bool {
        self.index_refresh || !self.rows.is_empty()
    }

    /// Drain the batch. Rows come out in full identity order, so one input
    /// always flushes the same way.
    fn take(&mut self) -> FlushPlan {
        let index_refresh = std::mem::take(&mut self.index_refresh);
        let retried = std::mem::take(&mut self.retry);
        let mut rows = std::mem::take(&mut self.rows)
            .into_iter()
            .collect::<Vec<_>>();
        if index_refresh {
            rows.clear();
        }
        rows.sort_by(|(a, _), (b, _)| a.cmp(b));
        FlushPlan {
            rows,
            index_refresh,
            seq: self.last_seq,
            retried,
        }
    }
}

/// One flush's inputs: the rows to project, or one index refresh.
struct FlushPlan {
    rows: Vec<(SessionKey, PendingRow)>,
    index_refresh: bool,
    seq: u64,
    /// True when a failed load already gave these rows back once.
    retried: bool,
}

/// The batch one blocking load carries, and what the bus said about it
/// while it ran.
#[derive(Debug, PartialEq, Eq)]
struct Batch {
    rows: BTreeMap<SessionKey, PendingRow>,
    seq: u64,
    /// Set by a broad removal, an index invalidation, or lag: the refetch
    /// they cause supersedes every row here, so the load's result is
    /// discarded.
    invalidated: bool,
    /// True when a failed load already gave these rows back once.
    retried: bool,
}

/// One lag recovery cycle: the sequence the reader was told to resync at,
/// and a newer gap seen since, which needs its own resync when the cycle
/// closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Recovery {
    watermark: u64,
    follow_up: Option<u64>,
}

/// The loop's state between bus events: the coalesced batch, the batch one
/// load carries, and the open lag recovery cycle.
#[derive(Default)]
struct Projector {
    pending: Pending,
    in_flight: Option<Batch>,
    recovery: Option<Recovery>,
}

impl Projector {
    /// Fold one bus event in. Returns what to emit immediately.
    fn absorb(&mut self, sequenced: Sequenced) -> Vec<Immediate> {
        self.pending.last_seq = self.pending.last_seq.max(sequenced.seq);
        match &sequenced.event {
            // Anonymous clears relay at once: the registry is the only
            // authority on anonymous state, and no reader keeps a timer.
            SessionEvent::Started { .. }
            | SessionEvent::Activity { .. }
            | SessionEvent::Quiet { .. }
            | SessionEvent::AnonymousCleared { .. } => vec![Immediate::Lifecycle(sequenced)],
            SessionEvent::Resync => {
                // The registry lost observations: rows may be stale too.
                // The event is canonical, so it relays as is and sets the
                // recovery watermark the reader will resync at.
                self.lose(sequenced.seq);
                self.recovery = Some(match self.recovery {
                    Some(open) => Recovery {
                        watermark: open.watermark.max(sequenced.seq),
                        follow_up: open.follow_up.filter(|seq| *seq > sequenced.seq),
                    },
                    None => Recovery {
                        watermark: sequenced.seq,
                        follow_up: None,
                    },
                });
                vec![Immediate::Lifecycle(sequenced)]
            }
            SessionEvent::Idle { session, at, .. } => {
                // The row's active state flips at idle, so the row also
                // projects again.
                let key = session_key(session);
                self.retire(&key);
                self.pending.note_row(
                    key,
                    UpdateFacets {
                        metadata: true,
                        ..Default::default()
                    },
                    *at,
                    sequenced.seq,
                );
                vec![Immediate::Lifecycle(sequenced)]
            }
            SessionEvent::Updated {
                session,
                facets,
                at,
            } => {
                let key = session_key(session);
                self.retire(&key);
                self.pending.note_row(key, *facets, *at, sequenced.seq);
                Vec::new()
            }
            SessionEvent::Removed { session, reason } => {
                match session {
                    Some(session) => {
                        let key = session_key(session);
                        self.pending.rows.remove(&key);
                        if let Some(batch) = self.in_flight.as_mut() {
                            batch.rows.remove(&key);
                        }
                    }
                    None => self.invalidate_in_flight(),
                }
                vec![Immediate::IndexChanged(IndexChangedPayload {
                    seq: sequenced.seq,
                    cause: IndexChangeCause::Removed,
                    session: session.clone(),
                    removal: Some(*reason),
                })]
            }
            SessionEvent::IndexChanged { reason } => {
                if *reason == IndexChangeReason::Invalidated {
                    self.invalidate_in_flight();
                }
                vec![Immediate::IndexChanged(IndexChangedPayload {
                    seq: sequenced.seq,
                    cause: IndexChangeCause::from(*reason),
                    session: None,
                    removal: None,
                })]
            }
        }
    }

    /// Record transport lag at the registry's current sequence. The first
    /// lag of a cycle relays a resync at that sequence and schedules an
    /// index refresh. Later lag in the same cycle is coalesced; when it is
    /// above the watermark, a follow-up is retained for the cycle's end.
    fn absorb_lag(&mut self, watermark: u64) -> Vec<Immediate> {
        self.lose(watermark);
        match self.recovery.as_mut() {
            None => {
                self.recovery = Some(Recovery {
                    watermark,
                    follow_up: None,
                });
                vec![Immediate::Lifecycle(Sequenced {
                    seq: watermark,
                    event: SessionEvent::Resync,
                    // The bridge fabricates no counts: the snapshot the
                    // reader re-reads carries them.
                    aggregate: None,
                })]
            }
            Some(open) => {
                if watermark > open.watermark {
                    open.follow_up =
                        Some(open.follow_up.map_or(watermark, |seq| seq.max(watermark)));
                }
                Vec::new()
            }
        }
    }

    /// Events or rows were lost up to `seq`: the list refetches, and the
    /// load in flight, if any, has nothing to add after that refetch.
    fn lose(&mut self, seq: u64) {
        self.pending.last_seq = self.pending.last_seq.max(seq);
        self.pending.index_refresh = true;
        self.invalidate_in_flight();
    }

    fn invalidate_in_flight(&mut self) {
        if let Some(batch) = self.in_flight.as_mut() {
            batch.invalidated = true;
        }
    }

    /// Move a key's row from the batch in flight back to the pending
    /// batch: a newer change arrived during its load, so only the later
    /// projection is emitted, with the facets of both.
    fn retire(&mut self, key: &SessionKey) {
        if let Some(batch) = self.in_flight.as_mut()
            && let Some(row) = batch.rows.remove(key)
        {
            self.pending
                .note_row(key.clone(), row.facets, row.at, row.seq);
        }
    }

    fn has_work(&self) -> bool {
        self.pending.has_work()
    }

    fn loading(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Close the pending batch. Returns what to emit at once and, when the
    /// batch has rows, the keys to load. An index refresh closes the lag
    /// recovery cycle; a newer gap retained during it opens the next one.
    fn open_batch(&mut self) -> (Vec<Immediate>, Option<Vec<SessionKey>>) {
        debug_assert!(self.in_flight.is_none(), "one batch loads at a time");
        let plan = self.pending.take();
        let mut out = Vec::new();
        if plan.index_refresh {
            out.push(Immediate::IndexChanged(IndexChangedPayload {
                seq: plan.seq,
                cause: IndexChangeCause::Resync,
                session: None,
                removal: None,
            }));
            if let Some(Recovery {
                follow_up: Some(seq),
                ..
            }) = self.recovery.take()
            {
                self.pending.index_refresh = true;
                self.pending.last_seq = self.pending.last_seq.max(seq);
                self.recovery = Some(Recovery {
                    watermark: seq,
                    follow_up: None,
                });
                out.push(Immediate::Lifecycle(Sequenced {
                    seq,
                    event: SessionEvent::Resync,
                    aggregate: None,
                }));
            }
            return (out, None);
        }
        if plan.rows.is_empty() {
            return (out, None);
        }
        let keys = plan.rows.iter().map(|(key, _)| key.clone()).collect();
        self.in_flight = Some(Batch {
            rows: plan.rows.into_iter().collect(),
            seq: plan.seq,
            invalidated: false,
            retried: plan.retried,
        });
        (out, Some(keys))
    }

    /// Apply one finished load to the batch in flight. A loaded row is
    /// emitted unless the batch was invalidated or the row was retired or
    /// removed during the load. A missing row means one index refetch. A
    /// failed load gives its rows back to the pending batch once; a second
    /// failure becomes one index refetch.
    fn complete(&mut self, outcome: LoadResult) -> Vec<Emission> {
        let Some(batch) = self.in_flight.take() else {
            return Vec::new();
        };
        if batch.invalidated {
            return Vec::new();
        }
        match outcome {
            Ok(mut loaded) => {
                let mut out = Vec::new();
                let mut missing = false;
                for (key, row) in batch.rows {
                    let Some(entry) = loaded.remove(&key) else {
                        missing = true;
                        continue;
                    };
                    out.push(Emission::Updated(Box::new(SessionUpdatedPayload {
                        seq: row.seq,
                        session: SessionRef::from(&key),
                        facets: row.facets,
                        entry,
                    })));
                }
                if missing {
                    // A pending row vanished before the load: the list
                    // refetches.
                    out.push(Emission::IndexChanged(IndexChangedPayload {
                        seq: batch.seq,
                        cause: IndexChangeCause::Invalidated,
                        session: None,
                        removal: None,
                    }));
                }
                out
            }
            Err(error) => {
                ::tracing::warn!(
                    event = "session_projection_load_failed",
                    error = %error,
                    rows = batch.rows.len(),
                    retried = batch.retried,
                );
                if batch.retried {
                    return vec![Emission::IndexChanged(IndexChangedPayload {
                        seq: batch.seq,
                        cause: IndexChangeCause::Invalidated,
                        session: None,
                        removal: None,
                    })];
                }
                for (key, row) in batch.rows {
                    self.pending.note_row(key, row.facets, row.at, row.seq);
                }
                self.pending.retry = true;
                Vec::new()
            }
        }
    }
}

fn session_key(session: &SessionRef) -> SessionKey {
    SessionKey::new(
        session.environment_key.clone(),
        session.agent.clone(),
        session.session_id.clone(),
    )
}

/// What one blocking row load returns.
type LoadResult = anyhow::Result<HashMap<SessionKey, ActivityEntry>>;

/// Where the worker loads enriched rows from. Production is the store; a
/// test injects a scripted loader.
pub(crate) trait RowLoader: Send + Sync {
    /// Load the enriched row of every key it finds. A key with no row is
    /// absent from the result; a failed read is an error.
    fn load(&self, keys: &[SessionKey], now: i64) -> LoadResult;
}

impl RowLoader for Store {
    /// Load enriched rows for `keys` in one batch: the repository list once,
    /// the records in bounded chunks, then one projection per record. A
    /// record whose projection fails is left out, so the batch refetches.
    fn load(&self, keys: &[SessionKey], now: i64) -> LoadResult {
        let repositories = self.repositories()?;
        let records = self.session_records_for_session_keys(keys)?;
        let mut entries = HashMap::with_capacity(records.len());
        for record in records {
            let key = record.key.clone();
            match crate::commands::activity_entry(self, &repositories, record, now) {
                Ok(entry) => {
                    entries.insert(key, entry);
                }
                Err(error) => {
                    ::tracing::warn!(event = "session_projection_row_failed", error = %error);
                }
            }
        }
        Ok(entries)
    }
}

/// Where the worker sends what it projects. Production is the Tauri app
/// handle; a test injects a recorder.
pub(crate) trait ProjectionEmitter: Send + Sync {
    fn emit(&self, emission: Emission);
}

impl ProjectionEmitter for AppHandle {
    fn emit(&self, emission: Emission) {
        match emission {
            Emission::Lifecycle(sequenced) => {
                let _ = Emitter::emit(
                    self,
                    crate::commands::SESSION_LIFECYCLE_EVENT,
                    &LifecycleEnvelope {
                        seq: sequenced.seq,
                        event: &sequenced.event,
                        aggregate: sequenced.aggregate,
                    },
                );
            }
            Emission::IndexChanged(payload) => {
                let _ = Emitter::emit(self, crate::commands::SESSION_INDEX_CHANGED_EVENT, &payload);
            }
            Emission::Updated(payload) => {
                let _ = Emitter::emit(self, crate::commands::SESSION_UPDATED_EVENT, &*payload);
            }
        }
    }
}

/// Start the projection worker. The returned handle is aborted with the
/// rest of the schedulers on exit; a load still running then finishes on
/// the pool and its result is discarded.
pub fn spawn(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let bus = app.state::<SessionEvents>().subscribe();
    let loader: Arc<dyn RowLoader> = Arc::new((*app.state::<Store>()).clone());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let emitter: Arc<dyn ProjectionEmitter> = Arc::new(app.clone());
        let events = app.state::<SessionEvents>();
        let watermark = || events.current_seq();
        run(bus, loader, emitter, &watermark, &crate::scan::unix_now).await;
    })
}

/// The loop [`spawn`] runs forever. Split out so a test can drive it with a
/// paused clock, a gated loader, and a recording emitter.
///
/// The bus is received at every wake, including while a load runs. Each
/// wake then applies a finished load and, when the fixed deadline has
/// passed and no load runs, opens the next batch. `watermark` is the
/// registry's current sequence, read only on lag.
pub(crate) async fn run(
    mut bus: broadcast::Receiver<Sequenced>,
    loader: Arc<dyn RowLoader>,
    emitter: Arc<dyn ProjectionEmitter>,
    watermark: &(dyn Fn() -> u64 + Send + Sync),
    now: &(dyn Fn() -> i64 + Send + Sync),
) {
    let mut projector = Projector::default();
    let mut flush_at: Option<Instant> = None;
    let mut load: Option<JoinHandle<LoadResult>> = None;
    loop {
        let mut finished = None;
        tokio::select! {
            biased;
            outcome = join_load(&mut load), if load.is_some() => {
                load = None;
                finished = Some(outcome);
            }
            received = bus.recv() => match received {
                Ok(sequenced) => {
                    ::tracing::debug!(event = "session_lifecycle_event", payload = ?sequenced);
                    emit_all(emitter.as_ref(), projector.absorb(sequenced));
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    ::tracing::warn!(event = "session_projection_lagged", skipped);
                    emit_all(emitter.as_ref(), projector.absorb_lag(watermark()));
                }
                Err(broadcast::error::RecvError::Closed) => {
                    // Shutdown: a load in flight finishes on the pool and
                    // its result is discarded with the pending batch.
                    return;
                }
            },
            () = sleep_until_instant(flush_at), if flush_at.is_some() && load.is_none() => {}
        }
        if finished.is_none()
            && let Some(handle) = load.as_mut()
            && handle.is_finished()
        {
            finished = Some(join_load(&mut load).await);
            load = None;
        }
        if let Some(outcome) = finished {
            for emission in projector.complete(outcome) {
                emitter.emit(emission);
            }
        }
        // A deadline that passed during a load starts the next batch now:
        // it is not moved, and a continued event stream cannot delay it.
        if !projector.loading() && flush_at.is_some_and(|deadline| Instant::now() >= deadline) {
            flush_at = None;
            let (immediates, keys) = projector.open_batch();
            emit_all(emitter.as_ref(), immediates);
            if let Some(keys) = keys {
                let loader = Arc::clone(&loader);
                let at = now();
                load = Some(tokio::task::spawn_blocking(move || loader.load(&keys, at)));
            }
        }
        // The deadline opens once per batch and later events do not move
        // it, so continued load cannot starve the flush.
        if flush_at.is_none() && projector.has_work() {
            flush_at = Some(Instant::now() + FLUSH_DELAY);
        }
    }
}

fn emit_all(emitter: &dyn ProjectionEmitter, immediates: Vec<Immediate>) {
    for immediate in immediates {
        emitter.emit(Emission::from(immediate));
    }
}

/// Wait for the load in flight. A load whose task panicked or was
/// cancelled reads as a failed load.
async fn join_load(load: &mut Option<JoinHandle<LoadResult>>) -> LoadResult {
    match load.as_mut() {
        Some(handle) => match handle.await {
            Ok(outcome) => outcome,
            Err(error) => Err(anyhow::anyhow!("row load task failed: {error}")),
        },
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

#[cfg(test)]
mod tests;
