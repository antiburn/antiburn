//! One projection worker owns all session event emission to webviews.
//!
//! `session:lifecycle` relays registry transitions and recovery markers.
//! `session:updated` carries enriched rows. `session:index-changed` carries membership
//! changes and invalidations. Producers report compact observations to the registry
//! instead of emitting these scopes.
//!
//! Row loads run on the blocking pool while the loop continues receiving bus events.
//! Pending rows share a fixed flush deadline. Overflow requests one index refresh
//! instead of retaining more patches.
//!
//! Newer changes suppress older loaded rows and preserve their facets for the next
//! batch. Keyed removals suppress their rows. Broad removals, invalidations, and lag
//! invalidate the batch.
//!
//! Transport lag requests recovery at the registry's current sequence. Recovery retains
//! a follow-up request when later lag exceeds that sequence. The bridge never allocates
//! canonical sequences.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant};

use crate::dto::ActivityEntry;
use crate::session_lifecycle::{
    IndexChangeReason, LifecycleEnvelope, ModelRequest, ModelResult, RemovalReason, Sequenced,
    SessionEvent, SessionEvents, SessionRef, UpdateFacets,
};
use crate::store::{SessionKey, Store};

/// The first pending row starts this fixed delay. Later events do not move the
/// deadline. Only one load runs at a time.
const FLUSH_DELAY: Duration = Duration::from_millis(250);

/// Pending and loading batches each hold at most this many keys. Overflow requests one
/// index refresh.
const PENDING_ROW_CAP: usize = 512;

/// This quota bounds bus receipt before a completed load emits rows.
const RECEIVE_BATCH: usize = 1024;

/// The bridge sends this enriched row and its changed facets on `session:updated`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdatedPayload {
    pub seq: u64,
    pub session: SessionRef,
    pub facets: UpdateFacets,
    pub entry: ActivityEntry,
}

/// The bridge sends this membership change or invalidation on `session:index-changed`.
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

/// This cause identifies the kind of index change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexChangeCause {
    /// A scan pass changes list membership.
    ScanPass,
    /// A broad invalidation requires a list refresh.
    Invalidated,
    /// A session leaves the store.
    Removed,
    /// Recovery requires a list refresh. Transport lag also requests a live snapshot.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingRow {
    facets: UpdateFacets,
    at: i64,
    seq: u64,
}

#[derive(Clone, Debug, PartialEq)]
enum Immediate {
    /// Relay this event on `session:lifecycle`.
    Lifecycle(Sequenced),
    /// Emit this on `session:index-changed`.
    IndexChanged(IndexChangedPayload),
}

/// The bridge sends these outputs through Tauri.
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

#[derive(Default)]
struct Pending {
    rows: HashMap<SessionKey, PendingRow>,
    /// The next flush replaces patches with one index refresh after overflow or
    /// recovery.
    index_refresh: bool,

    last_seq: u64,
    /// A failed load returns rows for one retry. Another failure requests a list
    /// refresh.
    retry: bool,
}

impl Pending {
    /// Merge a row change without exceeding [`PENDING_ROW_CAP`].
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

    /// Drain rows in full identity order for deterministic output.
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

struct FlushPlan {
    rows: Vec<(SessionKey, PendingRow)>,
    index_refresh: bool,
    seq: u64,
    /// The batch permits only one retry.
    retried: bool,
}

/// The loading batch tracks changes that arrive during its blocking read.
#[derive(Debug, PartialEq, Eq)]
struct Batch {
    rows: BTreeMap<SessionKey, PendingRow>,
    seq: u64,
    /// A broad invalidation suppresses every row in the loading batch.
    invalidated: bool,
    /// The batch permits only one retry.
    retried: bool,
}

/// Recovery retains a newer gap for a follow-up request after the current cycle closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Recovery {
    watermark: u64,
    follow_up: Option<u64>,
}

#[derive(Default)]
struct Projector {
    pending: Pending,
    in_flight: Option<Batch>,
    recovery: Option<Recovery>,
}

impl Projector {
    /// Combine a bus event with pending work and return immediate outputs.
    fn absorb(&mut self, sequenced: Sequenced) -> Vec<Immediate> {
        self.pending.last_seq = self.pending.last_seq.max(sequenced.seq);
        match &sequenced.event {
            // The registry alone decides when anonymous activity ends.
            SessionEvent::Started { .. }
            | SessionEvent::Activity { .. }
            | SessionEvent::Quiet { .. }
            | SessionEvent::AnonymousCleared { .. }
            | SessionEvent::SweepChanged => vec![Immediate::Lifecycle(sequenced)],
            SessionEvent::Resync => {
                // This defensive path accepts synthetic recovery markers. The registry
                // never publishes `Resync`.
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
                // Idle also requires a new projection of the row’s activity flag.
                let key = session_key(session);
                self.retire(&key);
                self.note_row(
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
                self.note_row(key, *facets, *at, sequenced.seq);
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
                    None => self.invalidate_rows(),
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
                    self.invalidate_rows();
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

    /// Record lag at the registry sequence. The first gap requests recovery. Later gaps
    /// above its watermark retain a follow-up request.
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
                    // The snapshot supplies exact counts. The bridge does not invent
                    // them.
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

    /// Request an index refresh and suppress older loaded rows.
    fn lose(&mut self, seq: u64) {
        self.pending.last_seq = self.pending.last_seq.max(seq);
        self.pending.index_refresh = true;
        self.invalidate_rows();
    }

    fn note_row(&mut self, key: SessionKey, facets: UpdateFacets, at: i64, seq: u64) {
        self.pending.note_row(key, facets, at, seq);
        if self.pending.index_refresh {
            self.invalidate_in_flight();
        }
    }

    fn invalidate_rows(&mut self) {
        self.pending.rows.clear();
        self.pending.retry = false;
        self.invalidate_in_flight();
    }

    fn invalidate_in_flight(&mut self) {
        if let Some(batch) = self.in_flight.as_mut() {
            batch.invalidated = true;
        }
    }

    /// Retire the loading row after a newer change. Keep its facets in the pending
    /// batch.
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

    /// Open the pending batch. An index refresh closes the recovery cycle. A retained
    /// newer gap starts another cycle.
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

    /// Emit only rows that remain valid after a load. Missing rows request an index
    /// refresh. Failed loads permit one bounded retry.
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
                    // The missing row requires a list refresh.
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

type LoadResult = anyhow::Result<HashMap<SessionKey, ActivityEntry>>;

/// This boundary loads enriched rows outside the actor. Tests inject a controlled
/// loader.
pub(crate) trait RowLoader: Send + Sync {
    /// Load each available row. Missing keys remain absent from the result.
    fn load(&self, keys: &[SessionKey], now: i64) -> LoadResult;
}

impl RowLoader for Store {
    /// Load repositories once and records in bounded chunks. Failed row projections
    /// remain absent and cause a list refresh.
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

/// This boundary reads compact published models outside the actor.
pub(crate) trait ModelLoader: Send + Sync {
    fn load_models(&self, keys: &[SessionKey]) -> ModelResult;
}

impl ModelLoader for Store {
    fn load_models(&self, keys: &[SessionKey]) -> ModelResult {
        self.published_models_for_keys(keys)
    }
}

/// This boundary owns session event emission. Production uses Tauri; tests use a
/// recorder.
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

/// Start the projection worker. Shutdown aborts its task and discards any remaining
/// load result.
pub fn spawn(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let bus = app.state::<SessionEvents>().subscribe();
    let loader: Arc<dyn RowLoader> = Arc::new((*app.state::<Store>()).clone());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let emitter: Arc<dyn ProjectionEmitter> = Arc::new(app.clone());
        let events = app.state::<SessionEvents>();
        let watermark = || events.current_seq();
        let models: Arc<dyn ModelLoader> = Arc::new((*app.state::<Store>()).clone());
        run_with_models(
            bus,
            loader,
            emitter,
            &watermark,
            &crate::scan::unix_now,
            Some((&events, models)),
        )
        .await;
    })
}

/// Keep receiving bus events during blocking loads. Process queued changes before
/// emitting completed rows. Fixed deadlines start batches without waiting for producer
/// silence.
#[cfg(test)]
pub(crate) async fn run(
    bus: broadcast::Receiver<Sequenced>,
    loader: Arc<dyn RowLoader>,
    emitter: Arc<dyn ProjectionEmitter>,
    watermark: &(dyn Fn() -> u64 + Send + Sync),
    now: &(dyn Fn() -> i64 + Send + Sync),
) {
    run_with_models(bus, loader, emitter, watermark, now, None).await;
}

pub(crate) async fn run_with_models(
    mut bus: broadcast::Receiver<Sequenced>,
    loader: Arc<dyn RowLoader>,
    emitter: Arc<dyn ProjectionEmitter>,
    watermark: &(dyn Fn() -> u64 + Send + Sync),
    now: &(dyn Fn() -> i64 + Send + Sync),
    models: Option<(&SessionEvents, Arc<dyn ModelLoader>)>,
) {
    let mut projector = Projector::default();
    let mut flush_at: Option<Instant> = None;
    let mut load: Option<JoinHandle<LoadResult>> = None;
    let mut model_load: Option<JoinHandle<ModelResult>> = None;
    let mut model_requests: Vec<ModelRequest> = Vec::new();
    let mut model_ack: Option<
        std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>>,
    > = None;
    let mut model_at = models.as_ref().map(|_| Instant::now());
    loop {
        if model_load.is_none()
            && model_ack.is_none()
            && let Some((events, loader)) = &models
        {
            let (requests, next) = events.model_page();
            model_at = next;
            if !requests.is_empty() {
                let keys: Vec<_> = requests.iter().map(|request| request.key.clone()).collect();
                model_requests = requests;
                let loader = Arc::clone(loader);
                model_load = Some(tokio::task::spawn_blocking(move || {
                    loader.load_models(&keys)
                }));
            }
        }
        let mut finished = None;
        tokio::select! {
            biased;
            outcome = async { model_load.as_mut().expect("model load exists").await }, if model_load.is_some() => {
                model_load = None;
                let result = outcome.unwrap_or_else(|error| Err(anyhow::anyhow!("model read task failed: {error}")));
                let events = models.as_ref().expect("model loader exists").0;
                model_ack = Some(Box::pin(events.submit_models(std::mem::take(&mut model_requests), result)));
            }
            () = async { model_ack.as_mut().expect("model ack exists").await }, if model_ack.is_some() => {
                model_ack = None;
            }
            () = async { models.as_ref().expect("model loader exists").0.models_changed().await },
                if models.is_some() && model_load.is_none() && model_ack.is_none() => {}
            () = sleep_until_instant(model_at), if model_at.is_some() && model_load.is_none() && model_ack.is_none() => {}
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
                    // Shutdown discards the loading result and pending batch.
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
            // Receive queued changes before a completed load can emit an obsolete row.
            for _ in 0..RECEIVE_BATCH {
                match bus.try_recv() {
                    Ok(sequenced) => emit_all(emitter.as_ref(), projector.absorb(sequenced)),
                    Err(broadcast::error::TryRecvError::Lagged(_)) => {
                        emit_all(emitter.as_ref(), projector.absorb_lag(watermark()));
                    }
                    Err(broadcast::error::TryRecvError::Empty) => break,
                    Err(broadcast::error::TryRecvError::Closed) => return,
                }
            }
            if !bus.is_empty() {
                // A larger backlog requires recovery instead of a possibly obsolete
                // patch.
                emit_all(emitter.as_ref(), projector.absorb_lag(watermark()));
            }
            for emission in projector.complete(outcome) {
                emitter.emit(emission);
            }
        }
        // A due deadline starts the next batch immediately after the current load ends.
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
        // Later events cannot move the first-row deadline.
        if flush_at.is_none() && projector.has_work() {
            flush_at = Some(Instant::now() + FLUSH_DELAY);
        }
        if !projector.has_work() {
            flush_at = None;
        }
        tokio::task::yield_now().await;
    }
}

fn emit_all(emitter: &dyn ProjectionEmitter, immediates: Vec<Immediate>) {
    for immediate in immediates {
        emitter.emit(Emission::from(immediate));
    }
}

/// Wait for the pending load. A panic or cancellation returns a load error.
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
