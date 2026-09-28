//! Explicit, progressive search across all published retained session content.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::Manager as _;

use super::{
    CACHE_BYTE_LIMIT, EXCERPT_CHARS, EvidenceCoverage, EvidenceKind, MAX_SAFE_JS_INTEGER,
    SessionEvidenceHit, StoredEvidenceReference, evidence_reference, session_entry,
};
use crate::store::{
    DeepContentChunk, DeepContentCursor, DeepSessionIdentity, DeepSessionManifest, Store,
};

const BATCH_BYTE_LIMIT: usize = 4 * 1024 * 1024;
const SESSION_QUANTUM_BYTE_LIMIT: usize = 1024 * 1024;
const BATCH_SCHEDULING_TARGET: Duration = Duration::from_millis(100);
const RESULT_LIMIT: usize = 100;
const OVERLAP_CHARS: usize = 256;
const JSON_DECODE_BYTE_LIMIT: usize = 1024 * 1024;
const READER_BUSY_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeepSearchStatus {
    Searching,
    Stopped,
    Finished,
    FinishedUnavailable,
    Partial,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InvalidatedSession {
    environment_key: String,
    agent: String,
    session_id: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeepSearchCoverage {
    eligible_sessions: usize,
    inspected_sessions: usize,
    inspected_parts: usize,
    inspected_bytes: usize,
    unavailable_sessions: usize,
    changed_sessions: usize,
    ingestion_truncated_parts: usize,
    scope_exhausted: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeepSearchResponse {
    scope: Option<crate::session_search_scope::SessionSearchScope>,
    scan_id: u64,
    query_revision: u64,
    available: bool,
    status: DeepSearchStatus,
    continuation_available: bool,
    results: Vec<SessionEvidenceHit>,
    invalidated_sessions: Vec<InvalidatedSession>,
    total_matching_sessions: usize,
    coverage: DeepSearchCoverage,
}

#[derive(Default)]
pub(crate) struct DeepSearchController {
    state: Mutex<DeepControllerState>,
}

#[derive(Default)]
struct DeepControllerState {
    active: Option<ActiveScan>,
    pending: Option<PendingStart>,
    retired: Vec<ActiveScan>,
}

#[derive(Clone)]
struct PendingStart {
    scan_id: u64,
    query_revision: u64,
    cancel: Arc<AtomicBool>,
    discard: Arc<AtomicBool>,
}

#[derive(Clone)]
struct ActiveScan {
    scan_id: u64,
    query_revision: u64,
    cancel: Arc<AtomicBool>,
    discard: Arc<AtomicBool>,
    in_flight: Arc<AtomicBool>,
    scan: Arc<Mutex<DeepScan>>,
}

struct DeepScan {
    scope: Option<crate::session_search_scope::SessionSearchScope>,
    scan_id: u64,
    query_revision: u64,
    query: String,
    terms: Vec<String>,
    phrase: Option<String>,
    status: DeepSearchStatus,
    reader: Store,
    sessions: Vec<SessionProgress>,
    next_session: usize,
    results: HashMap<DeepSessionIdentity, SessionEvidenceHit>,
    matched_sessions: HashSet<DeepSessionIdentity>,
    total_matching_sessions: usize,
    invalidated: Vec<DeepSessionIdentity>,
    coverage: DeepSearchCoverage,
    retained_result_bytes: usize,
}

struct SessionProgress {
    manifest: DeepSessionManifest,
    cursor: DeepContentCursor,
    inspected: bool,
    invalidated: bool,
    row_state: Option<RowState>,
}

struct RowState {
    turn_rowid: i64,
    part_index: i64,
    chars_seen: usize,
    tail: String,
    prefix: Vec<u8>,
    utf8_carry: Vec<u8>,
    json_buffer: Vec<u8>,
    json_too_large: bool,
}

#[derive(Clone)]
struct TextCandidate {
    text: String,
    json_path: Option<String>,
}

impl DeepSearchController {
    fn lock(&self) -> std::sync::MutexGuard<'_, DeepControllerState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn reserve_start(&self, scan_id: u64, query_revision: u64) -> PendingStart {
        let pending = PendingStart {
            scan_id,
            query_revision,
            cancel: Arc::new(AtomicBool::new(false)),
            discard: Arc::new(AtomicBool::new(false)),
        };
        let mut state = self.lock();
        if let Some(previous) = state.active.take() {
            previous.cancel.store(true, Ordering::Release);
            previous.discard.store(true, Ordering::Release);
            if previous.in_flight.load(Ordering::Acquire) {
                state.retired.push(previous);
            }
        }
        if let Some(previous) = state.pending.replace(pending.clone()) {
            previous.cancel.store(true, Ordering::Release);
            previous.discard.store(true, Ordering::Release);
        }
        pending
    }

    fn install_start(&self, pending: &PendingStart, active: ActiveScan) -> bool {
        let mut state = self.lock();
        let current = state.pending.as_ref().is_some_and(|current| {
            current.scan_id == pending.scan_id
                && current.query_revision == pending.query_revision
                && Arc::ptr_eq(&current.cancel, &pending.cancel)
        });
        if !current || pending.discard.load(Ordering::Acquire) {
            return false;
        }
        state.pending = None;
        state.active = Some(active);
        true
    }

    fn matching(&self, scan_id: u64, query_revision: u64) -> Option<ActiveScan> {
        self.lock()
            .active
            .as_ref()
            .filter(|active| active.scan_id == scan_id && active.query_revision == query_revision)
            .cloned()
    }

    fn cancel(
        &self,
        scan_id: u64,
        query_revision: u64,
        discard: bool,
    ) -> (bool, Option<ActiveScan>) {
        let state = self.lock();
        if let Some(active) = state
            .active
            .as_ref()
            .filter(|active| active.scan_id == scan_id && active.query_revision == query_revision)
        {
            active.cancel.store(true, Ordering::Release);
            active.discard.fetch_or(discard, Ordering::AcqRel);
            return (true, Some(active.clone()));
        }
        if let Some(pending) = state.pending.as_ref().filter(|pending| {
            pending.scan_id == scan_id && pending.query_revision == query_revision
        }) {
            pending.cancel.store(true, Ordering::Release);
            pending.discard.fetch_or(discard, Ordering::AcqRel);
            return (true, None);
        }
        (false, None)
    }

    fn discard_if_requested(&self, active: &ActiveScan) {
        if !active.discard.load(Ordering::Acquire) {
            return;
        }
        let mut state = self.lock();
        if state.active.as_ref().is_some_and(|value| {
            value.scan_id == active.scan_id && value.query_revision == active.query_revision
        }) {
            state.active = None;
        }
        state.retired.retain(|value| {
            value.in_flight.load(Ordering::Acquire)
                && !(value.scan_id == active.scan_id
                    && value.query_revision == active.query_revision)
        });
    }

    fn clear_pending(&self, pending: &PendingStart) {
        let mut state = self.lock();
        if state.pending.as_ref().is_some_and(|current| {
            current.scan_id == pending.scan_id
                && current.query_revision == pending.query_revision
                && Arc::ptr_eq(&current.cancel, &pending.cancel)
        }) {
            state.pending = None;
        }
    }

    fn retained_scan_bytes_except(&self, excluded: &ActiveScan) -> usize {
        let mut state = self.lock();
        state
            .retired
            .retain(|active| active.in_flight.load(Ordering::Acquire));
        state
            .active
            .iter()
            .chain(state.retired.iter())
            .filter(|active| !Arc::ptr_eq(&active.scan, &excluded.scan))
            .map(|active| {
                active
                    .scan
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .retained_result_bytes
            })
            .sum()
    }
}

#[tauri::command]
pub(crate) async fn start_deep_session_search(
    app: tauri::AppHandle,
    query: String,
    scan_id: u64,
    query_revision: u64,
    scope: Option<crate::session_search_scope::SessionSearchScope>,
) -> Result<DeepSearchResponse, String> {
    if let Some(scope) = &scope {
        scope.validate().map_err(|e| e.to_string())?;
    }
    validate_request(&query, scan_id, query_revision)?;
    let controller = app.state::<DeepSearchController>();
    let pending = controller.reserve_start(scan_id, query_revision);
    let startup = async {
        let writer = app.state::<Store>().inner().clone();
        let reader =
            tauri::async_runtime::spawn_blocking(move || writer.open_reader(READER_BUSY_TIMEOUT))
                .await
                .map_err(|_| "unavailable".to_owned())?
                .map_err(|_| "unavailable".to_owned())?;
        let preferred_reader = reader.clone();
        let preferred_query = query.clone();
        let preferred_scope = scope.clone();
        let preferred = tauri::async_runtime::spawn_blocking(move || {
            preferred_reader
                .readonly_local_session_candidates(&preferred_query, preferred_scope.as_ref())
                .map(|(values, _)| {
                    values
                        .into_iter()
                        .map(|value| DeepSessionIdentity {
                            environment_key: value.environment_key,
                            agent: value.agent,
                            session_id: value.session_id,
                        })
                        .collect::<Vec<_>>()
                })
        })
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
        let manifest_reader = reader.clone();
        let manifest_scope = scope.clone();
        let manifest_cancel = Arc::clone(&pending.cancel);
        let manifests = tauri::async_runtime::spawn_blocking(move || {
            manifest_reader.deep_session_manifest(
                &preferred,
                manifest_scope.as_ref(),
                manifest_cancel,
            )
        })
        .await
        .map_err(|_| "unavailable".to_owned())?
        .map_err(|error| match error.to_string().as_str() {
            "resource_limit" => "resource_limit".to_owned(),
            _ => "search_failed".to_owned(),
        })?;
        Ok::<_, String>((reader, manifests))
    }
    .await;
    let (reader, manifests) = match startup {
        Ok(value) => value,
        Err(error) => {
            controller.clear_pending(&pending);
            return Err(error);
        }
    };
    let mut scan = DeepScan::new(scan_id, query_revision, query, reader, manifests);
    scan.scope = scope;
    let active = ActiveScan {
        scan_id,
        query_revision,
        cancel: Arc::clone(&pending.cancel),
        discard: Arc::clone(&pending.discard),
        in_flight: Arc::new(AtomicBool::new(false)),
        scan: Arc::new(Mutex::new(scan)),
    };
    if pending.cancel.load(Ordering::Acquire) {
        active
            .scan
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .status = DeepSearchStatus::Stopped;
    }
    if !controller.install_start(&pending, active.clone()) {
        return Err("stale_scan".to_owned());
    }
    enforce_memory_limit(&app, &active);
    if pending.cancel.load(Ordering::Acquire) {
        return Ok(response_for(&app, &active));
    }
    if active
        .scan
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .status
        == DeepSearchStatus::Partial
    {
        return Ok(response_for(&app, &active));
    }
    run_retrieval_batch(&app, active).await
}

#[tauri::command]
pub(crate) async fn continue_deep_session_search(
    app: tauri::AppHandle,
    scan_id: u64,
    query_revision: u64,
) -> Result<DeepSearchResponse, String> {
    validate_identity(scan_id, query_revision)?;
    let controller = app.state::<DeepSearchController>();
    let Some(active) = controller.matching(scan_id, query_revision) else {
        return Err("stale_scan".to_owned());
    };
    let status = active
        .scan
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .status;
    match status {
        DeepSearchStatus::Searching | DeepSearchStatus::Stopped => {
            active.cancel.store(false, Ordering::Release);
            run_retrieval_batch(&app, active).await
        }
        _ => Ok(response_for(&app, &active)),
    }
}

#[tauri::command]
pub(crate) fn cancel_deep_session_search(
    app: tauri::AppHandle,
    scan_id: u64,
    query_revision: u64,
    discard: bool,
) -> Result<(), String> {
    validate_identity(scan_id, query_revision)?;
    let controller = app.state::<DeepSearchController>();
    let (matched, active) = controller.cancel(scan_id, query_revision, discard);
    if !matched {
        return Ok(());
    }
    if let Some(active) = active
        && !active.in_flight.load(Ordering::Acquire)
    {
        if !discard {
            active
                .scan
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .status = DeepSearchStatus::Stopped;
        }
        controller.discard_if_requested(&active);
    }
    Ok(())
}

async fn run_retrieval_batch(
    app: &tauri::AppHandle,
    active: ActiveScan,
) -> Result<DeepSearchResponse, String> {
    if active.in_flight.swap(true, Ordering::AcqRel) {
        return Err("scan_busy".to_owned());
    }
    let scan = Arc::clone(&active.scan);
    let cancel = Arc::clone(&active.cancel);
    let batch = tauri::async_runtime::spawn_blocking(move || {
        scan.lock()
            .unwrap_or_else(|error| error.into_inner())
            .run_batch(cancel)
    })
    .await;
    active.in_flight.store(false, Ordering::Release);
    match batch {
        Ok(Ok(())) => {}
        Ok(Err(_)) | Err(_) => {
            active
                .scan
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .status = batch_failure_status(active.cancel.load(Ordering::Acquire));
        }
    }
    enforce_memory_limit(app, &active);
    app.state::<DeepSearchController>()
        .discard_if_requested(&active);
    Ok(response_for(app, &active))
}

impl DeepScan {
    fn new(
        scan_id: u64,
        query_revision: u64,
        query: String,
        reader: Store,
        manifests: Vec<DeepSessionManifest>,
    ) -> Self {
        let phrase = quoted_phrase(&query);
        let terms = if let Some(phrase) = &phrase {
            vec![phrase.to_lowercase()]
        } else {
            query_terms(&query)
        };
        let eligible_sessions = manifests.len();
        let mut scan = Self {
            scope: None,
            scan_id,
            query_revision,
            query,
            terms,
            phrase,
            status: DeepSearchStatus::Searching,
            reader,
            sessions: manifests
                .into_iter()
                .map(|manifest| SessionProgress {
                    manifest,
                    cursor: DeepContentCursor::default(),
                    inspected: false,
                    invalidated: false,
                    row_state: None,
                })
                .collect(),
            next_session: 0,
            results: HashMap::new(),
            matched_sessions: HashSet::new(),
            total_matching_sessions: 0,
            invalidated: Vec::new(),
            coverage: DeepSearchCoverage {
                eligible_sessions,
                ..DeepSearchCoverage::default()
            },
            retained_result_bytes: 0,
        };
        scan.retained_result_bytes = scan.estimated_bytes();
        scan
    }

    fn run_batch(&mut self, cancel: Arc<AtomicBool>) -> anyhow::Result<()> {
        self.status = DeepSearchStatus::Searching;
        let batch_started = Instant::now();
        let mut batch_bytes = 0_usize;
        let mut visits_without_work = 0_usize;
        while batch_bytes < BATCH_BYTE_LIMIT
            && batch_started.elapsed() < BATCH_SCHEDULING_TARGET
            && !cancel.load(Ordering::Acquire)
            && !self.sessions.is_empty()
        {
            if self.sessions.iter().all(|session| session.cursor.complete) {
                break;
            }
            if self.next_session >= self.sessions.len() {
                self.next_session = 0;
            }
            let index = self.next_session;
            self.next_session += 1;
            if self.sessions[index].cursor.complete {
                visits_without_work += 1;
                if visits_without_work >= self.sessions.len() {
                    break;
                }
                continue;
            }
            visits_without_work = 0;
            let remaining = BATCH_BYTE_LIMIT - batch_bytes;
            let quantum_limit = SESSION_QUANTUM_BYTE_LIMIT.min(remaining);
            let scheduling_limit = BATCH_SCHEDULING_TARGET.saturating_sub(batch_started.elapsed());
            let quantum = {
                let session = &mut self.sessions[index];
                self.reader.read_deep_content_quantum(
                    &session.manifest,
                    &mut session.cursor,
                    quantum_limit,
                    scheduling_limit,
                    Arc::clone(&cancel),
                )?
            };
            if !self.sessions[index].inspected {
                self.sessions[index].inspected = true;
                self.coverage.inspected_sessions += 1;
            }
            batch_bytes += quantum.inspected_bytes;
            self.coverage.inspected_bytes += quantum.inspected_bytes;
            self.coverage.inspected_parts += quantum.inspected_parts;
            self.coverage.ingestion_truncated_parts += quantum.ingestion_truncated_parts;
            if quantum.unavailable {
                self.invalidate_session(index, quantum.changed);
                continue;
            }
            for chunk in quantum.chunks {
                self.inspect_chunk(index, chunk);
            }
            if quantum.complete {
                self.sessions[index].cursor.complete = true;
            }
        }
        if cancel.load(Ordering::Acquire) {
            self.status = DeepSearchStatus::Stopped;
        } else if self.sessions.iter().all(|session| session.cursor.complete) {
            self.revalidate_matches()?;
            self.coverage.scope_exhausted = true;
            self.status = if self.coverage.unavailable_sessions > 0 {
                DeepSearchStatus::FinishedUnavailable
            } else {
                DeepSearchStatus::Finished
            };
        }
        self.update_result_ranks();
        self.retained_result_bytes = self.estimated_bytes();
        Ok(())
    }

    fn inspect_chunk(&mut self, session_index: usize, chunk: DeepContentChunk) {
        let session = &mut self.sessions[session_index];
        let reset_row = session.row_state.as_ref().is_none_or(|state| {
            state.turn_rowid != chunk.turn_rowid || state.part_index != chunk.part_index
        });
        if reset_row {
            session.row_state = Some(RowState {
                turn_rowid: chunk.turn_rowid,
                part_index: chunk.part_index,
                chars_seen: 0,
                tail: String::new(),
                prefix: Vec::new(),
                utf8_carry: Vec::new(),
                json_buffer: Vec::new(),
                json_too_large: false,
            });
        }
        let row_state = session.row_state.as_mut().expect("row state exists");
        if row_state.prefix.len() < 64 {
            let needed = 64 - row_state.prefix.len();
            row_state.prefix.extend(chunk.content.iter().take(needed));
        }
        if matches!(chunk.kind.as_str(), "tool_input" | "tool_result") && !row_state.json_too_large
        {
            if row_state
                .json_buffer
                .len()
                .saturating_add(chunk.content.len())
                <= JSON_DECODE_BYTE_LIMIT
            {
                row_state.json_buffer.extend_from_slice(&chunk.content);
            } else {
                row_state.json_buffer.clear();
                row_state.json_too_large = true;
            }
        }
        let row_complete =
            chunk.byte_offset.saturating_add(chunk.content.len()) >= chunk.original_bytes;
        let decoded = decode_chunk(row_state, &chunk.content, row_complete);
        let tail_chars = row_state.tail.chars().count();
        let combined = format!("{}{}", row_state.tail, decoded);
        let combined_start = row_state.chars_seen.saturating_sub(tail_chars);
        let mut candidates = content_candidates(&chunk.kind, &combined, false);
        if row_complete && !row_state.json_too_large {
            let json_text = String::from_utf8_lossy(&row_state.json_buffer);
            candidates.extend(decoded_json_candidates(&json_text));
        }
        let mut best = None;
        for candidate in candidates {
            if let Some((start, end, score)) = find_match(
                &candidate.text,
                &self.query,
                &self.terms,
                self.phrase.as_deref(),
            ) {
                if candidate.json_path.is_none() && end <= tail_chars {
                    continue;
                }
                let absolute_start = if candidate.json_path.is_some() {
                    start
                } else {
                    combined_start.saturating_add(start)
                };
                let absolute_end = if candidate.json_path.is_some() {
                    end
                } else {
                    combined_start.saturating_add(end)
                };
                let (excerpt, excerpt_truncated) = excerpt_around(&candidate.text, start, end);
                let kind = evidence_kind(&chunk.kind, &candidate.text);
                let adjusted =
                    score + kind_bonus(kind) + u64::from(candidate.json_path.is_some()) * 30;
                let value = (
                    adjusted,
                    excerpt,
                    excerpt_truncated,
                    kind,
                    absolute_start,
                    absolute_end,
                    candidate.json_path,
                );
                if best
                    .as_ref()
                    .is_none_or(|current: &(u64, _, _, _, _, _, _)| value.0 > current.0)
                {
                    best = Some(value);
                }
            }
        }
        row_state.chars_seen += decoded.chars().count();
        row_state.tail = decoded
            .chars()
            .rev()
            .take(OVERLAP_CHARS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if row_complete {
            row_state.json_buffer = Vec::new();
        }
        let Some((score, excerpt, excerpt_truncated, kind, start, end, json_path)) = best else {
            return;
        };
        let stored = StoredEvidenceReference {
            environment_key: session.manifest.identity.environment_key.clone(),
            agent: session.manifest.identity.agent.clone(),
            session_id: session.manifest.identity.session_id.clone(),
            source_generation: session.manifest.source_generation,
            published_fence: session.manifest.published_fence,
            source_key: chunk.source_key,
            thread_id: chunk.thread_id,
            scope: chunk.scope,
            turn_rowid: chunk.turn_rowid,
            turn_index: chunk.turn_index,
            part_index: chunk.part_index,
        };
        let mut reference = evidence_reference(stored, &row_state.prefix);
        reference.match_start = Some(start);
        reference.match_end = Some(end);
        reference.json_path = json_path;
        let hit = SessionEvidenceHit {
            session: session_entry(session.manifest.session.clone()),
            reference,
            excerpt,
            kind,
            score,
            retrieval_rank: 0,
            coverage: EvidenceCoverage {
                state: if session.cursor.complete {
                    "complete"
                } else {
                    "progressive"
                },
                inspected_bytes: self.coverage.inspected_bytes,
                byte_limit: BATCH_BYTE_LIMIT,
            },
            truncated: chunk.stored_truncated || excerpt_truncated,
        };
        let identity = session.manifest.identity.clone();
        let is_new = self.matched_sessions.insert(identity.clone());
        let replace = self
            .results
            .get(&identity)
            .is_none_or(|current| hit.score > current.score);
        if replace {
            self.results.insert(identity, hit);
        }
        if is_new {
            self.total_matching_sessions = self.matched_sessions.len();
        }
    }

    fn invalidate_session(&mut self, index: usize, changed: bool) {
        let session = &mut self.sessions[index];
        session.cursor.complete = true;
        if session.invalidated {
            return;
        }
        session.invalidated = true;
        let identity = session.manifest.identity.clone();
        self.coverage.unavailable_sessions += 1;
        self.coverage.changed_sessions += usize::from(changed);
        self.invalidated.push(identity.clone());
        self.results.remove(&identity);
        self.matched_sessions.remove(&identity);
        self.total_matching_sessions = self.matched_sessions.len();
    }

    fn revalidate_matches(&mut self) -> anyhow::Result<()> {
        let identities = self.results.keys().cloned().collect::<Vec<_>>();
        for identity in identities {
            let Some(index) = self
                .sessions
                .iter()
                .position(|session| session.manifest.identity == identity)
            else {
                continue;
            };
            let (matches, changed) = self
                .reader
                .deep_session_version_matches(&self.sessions[index].manifest)?;
            if !matches {
                self.invalidate_session(index, changed);
            }
        }
        Ok(())
    }

    fn sorted_results(&self) -> Vec<SessionEvidenceHit> {
        let mut results = self.results.values().cloned().collect::<Vec<_>>();
        results.sort_by(compare_hits);
        results.truncate(RESULT_LIMIT);
        for (index, hit) in results.iter_mut().enumerate() {
            hit.retrieval_rank = index + 1;
        }
        results
    }

    fn update_result_ranks(&mut self) {
        let ranked = self.sorted_results();
        let retained = ranked
            .iter()
            .map(|hit| hit.reference.key.clone())
            .collect::<HashSet<_>>();
        self.results
            .retain(|_, hit| retained.contains(&hit.reference.key));
        let ranks = ranked
            .iter()
            .map(|hit| (hit.reference.key.clone(), hit.retrieval_rank))
            .collect::<HashMap<_, _>>();
        for hit in self.results.values_mut() {
            hit.retrieval_rank = ranks.get(&hit.reference.key).copied().unwrap_or(0);
        }
    }

    fn estimated_bytes(&self) -> usize {
        let session_bytes = self
            .sessions
            .iter()
            .map(|session| {
                identity_bytes(&session.manifest.identity)
                    + session.manifest.session.environment_key.capacity()
                    + session.manifest.session.agent.capacity()
                    + session.manifest.session.session_id.capacity()
                    + session
                        .manifest
                        .session
                        .wsl_distro
                        .as_ref()
                        .map_or(0, String::capacity)
                    + session
                        .manifest
                        .session
                        .title
                        .as_ref()
                        .map_or(0, String::capacity)
                    + session.manifest.session.repository.capacity()
                    + session.manifest.session.cwd_label.capacity()
                    + session.manifest.session.models.capacity() * std::mem::size_of::<String>()
                    + session
                        .manifest
                        .session
                        .models
                        .iter()
                        .map(String::capacity)
                        .sum::<usize>()
                    + session.cursor.thread_id.capacity()
                    + session.row_state.as_ref().map_or(0, |state| {
                        state.tail.capacity()
                            + state.prefix.capacity()
                            + state.utf8_carry.capacity()
                            + state.json_buffer.capacity()
                    })
            })
            .sum::<usize>();
        let result_bytes = self
            .results
            .iter()
            .map(|(identity, hit)| identity_bytes(identity) + hit_bytes(hit))
            .sum::<usize>();
        let matched_bytes = self
            .matched_sessions
            .iter()
            .map(identity_bytes)
            .sum::<usize>();
        let invalidated_bytes = self.invalidated.iter().map(identity_bytes).sum::<usize>();
        std::mem::size_of::<Self>()
            + self.sessions.capacity() * std::mem::size_of::<SessionProgress>()
            + self.results.capacity()
                * (std::mem::size_of::<DeepSessionIdentity>()
                    + std::mem::size_of::<SessionEvidenceHit>()
                    + 32)
            + self.matched_sessions.capacity() * (std::mem::size_of::<DeepSessionIdentity>() + 32)
            + self.invalidated.capacity() * std::mem::size_of::<DeepSessionIdentity>()
            + self.query.capacity()
            + self.terms.capacity() * std::mem::size_of::<String>()
            + self.terms.iter().map(String::capacity).sum::<usize>()
            + self.phrase.as_ref().map_or(0, String::capacity)
            + session_bytes
            + result_bytes
            + matched_bytes
            + invalidated_bytes
    }

    fn release_for_resource_limit(&mut self, byte_limit: usize) {
        self.status = DeepSearchStatus::Partial;
        self.sessions.clear();
        self.sessions.shrink_to_fit();
        self.matched_sessions.clear();
        self.matched_sessions.shrink_to_fit();
        self.invalidated.truncate(RESULT_LIMIT);
        self.invalidated.shrink_to_fit();
        self.terms.shrink_to_fit();
        self.query.shrink_to_fit();
        self.results.shrink_to_fit();
        self.retained_result_bytes = self.estimated_bytes();
        while self.retained_result_bytes > byte_limit && !self.results.is_empty() {
            let remove = self
                .results
                .iter()
                .max_by(|(_, left), (_, right)| compare_hits(left, right))
                .map(|(identity, _)| identity.clone());
            if let Some(identity) = remove {
                self.results.remove(&identity);
                self.results.shrink_to_fit();
                self.retained_result_bytes = self.estimated_bytes();
            }
        }
    }
}

fn compare_hits(left: &SessionEvidenceHit, right: &SessionEvidenceHit) -> std::cmp::Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| right.session.timestamp.cmp(&left.session.timestamp))
        .then_with(|| left.session.session_id.cmp(&right.session.session_id))
        .then_with(|| {
            left.session
                .environment_key
                .cmp(&right.session.environment_key)
        })
        .then_with(|| left.session.agent.cmp(&right.session.agent))
}

fn identity_bytes(identity: &DeepSessionIdentity) -> usize {
    identity.environment_key.capacity() + identity.agent.capacity() + identity.session_id.capacity()
}

fn hit_bytes(hit: &SessionEvidenceHit) -> usize {
    let session = &hit.session;
    let reference = &hit.reference;
    hit.excerpt.capacity()
        + session.environment_key.capacity()
        + session.agent.capacity()
        + session.session_id.capacity()
        + session.wsl_distro.as_ref().map_or(0, String::capacity)
        + session.title.as_ref().map_or(0, String::capacity)
        + session.repository.capacity()
        + session.cwd_label.capacity()
        + session.models.capacity() * std::mem::size_of::<String>()
        + session.models.iter().map(String::capacity).sum::<usize>()
        + session.timestamp.capacity()
        + reference.key.capacity()
        + reference.environment_key.capacity()
        + reference.agent.capacity()
        + reference.session_id.capacity()
        + reference.source_key.capacity()
        + reference.thread_id.capacity()
        + reference.scope.capacity()
        + reference.json_path.as_ref().map_or(0, String::capacity)
}

fn decode_chunk(row: &mut RowState, content: &[u8], row_complete: bool) -> String {
    let mut bytes = std::mem::take(&mut row.utf8_carry);
    bytes.extend_from_slice(content);
    match std::str::from_utf8(&bytes) {
        Ok(text) => text.to_owned(),
        Err(error) if error.error_len().is_none() && !row_complete => {
            let valid = error.valid_up_to();
            row.utf8_carry.extend_from_slice(&bytes[valid..]);
            String::from_utf8_lossy(&bytes[..valid]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(&bytes).into_owned(),
    }
}

fn validate_request(query: &str, scan_id: u64, query_revision: u64) -> Result<(), String> {
    if query.trim().is_empty() || query.chars().count() > 200 {
        return Err("invalid_request".to_owned());
    }
    validate_identity(scan_id, query_revision)
}

fn validate_identity(scan_id: u64, query_revision: u64) -> Result<(), String> {
    if scan_id > MAX_SAFE_JS_INTEGER || query_revision > MAX_SAFE_JS_INTEGER {
        return Err("invalid_request".to_owned());
    }
    Ok(())
}

fn quoted_phrase(query: &str) -> Option<String> {
    let trimmed = query.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        let value = trimmed[1..trimmed.len() - 1].trim();
        (!value.is_empty()).then(|| value.to_owned())
    } else {
        None
    }
}

fn query_terms(query: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "a", "an", "and", "about", "did", "find", "for", "from", "i", "in", "me", "my", "of", "on",
        "please", "session", "show", "that", "the", "to", "where", "with",
    ];
    let mut seen = HashSet::new();
    query
        .split(|character: char| {
            !character.is_alphanumeric() && !matches!(character, '_' | '-' | '/' | '.')
        })
        .map(str::trim)
        .map(str::to_lowercase)
        .filter(|term| {
            term.len() >= 2 && !STOP.contains(&term.as_str()) && seen.insert(term.clone())
        })
        .take(16)
        .collect()
}

fn find_match(
    text: &str,
    query: &str,
    terms: &[String],
    phrase: Option<&str>,
) -> Option<(usize, usize, u64)> {
    let (lower, offsets) = super::lowercase_with_offsets(text);
    if let Some(phrase) = phrase {
        let lower_phrase = phrase.to_lowercase();
        let byte_start = lower.find(&lower_phrase)?;
        let byte_end = byte_start + lower_phrase.len();
        let start = text[..offsets[byte_start]].chars().count();
        let end = text[..offsets[byte_end]].chars().count();
        return Some((start, end, 2_000));
    }
    let normalized_query = query.trim().to_lowercase();
    let phrase_position = (!normalized_query.is_empty())
        .then(|| lower.find(&normalized_query))
        .flatten();
    let mut positions = terms
        .iter()
        .filter_map(|term| lower.find(term).map(|position| (position, term.len())))
        .collect::<Vec<_>>();
    if positions.is_empty() {
        return None;
    }
    positions.sort_unstable();
    let matched = positions.len();
    let (byte_start, byte_len) = phrase_position
        .map(|position| (position, normalized_query.len()))
        .unwrap_or(positions[0]);
    let byte_end = byte_start + byte_len;
    let start = text[..offsets[byte_start]].chars().count();
    let end = text[..offsets[byte_end]].chars().count();
    let all_terms = matched == terms.len().max(1);
    let score = u64::from(phrase_position.is_some()) * 1_000
        + u64::from(all_terms) * 500
        + (matched as u64) * 200 / terms.len().max(1) as u64
        + matched as u64;
    Some((start, end, score))
}

fn excerpt_around(text: &str, start: usize, end: usize) -> (String, bool) {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= EXCERPT_CHARS {
        return (text.trim().to_owned(), false);
    }
    let match_center = start.saturating_add(end).saturating_div(2);
    let excerpt_start = match_center
        .saturating_sub(EXCERPT_CHARS / 2)
        .min(chars.len() - EXCERPT_CHARS);
    let excerpt_end = excerpt_start + EXCERPT_CHARS;
    let mut excerpt = chars[excerpt_start..excerpt_end].iter().collect::<String>();
    if excerpt_start > 0 {
        excerpt.insert(0, '…');
    }
    if excerpt_end < chars.len() {
        excerpt.push('…');
    }
    (excerpt.trim().to_owned(), true)
}

fn content_candidates(kind: &str, text: &str, complete_row: bool) -> Vec<TextCandidate> {
    let mut candidates = vec![TextCandidate {
        text: text.to_owned(),
        json_path: None,
    }];
    if complete_row
        && matches!(kind, "tool_input" | "tool_result")
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(text)
    {
        collect_json_strings(&value, "", &mut candidates);
    }
    candidates
}

fn decoded_json_candidates(text: &str) -> Vec<TextCandidate> {
    let mut candidates = Vec::new();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        collect_json_strings(&value, "", &mut candidates);
    }
    candidates
}

fn collect_json_strings(value: &serde_json::Value, path: &str, output: &mut Vec<TextCandidate>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                let key = key.replace('~', "~0").replace('/', "~1");
                collect_json_strings(value, &format!("{path}/{key}"), output);
            }
        }
        serde_json::Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_json_strings(value, &format!("{path}/{index}"), output);
            }
        }
        serde_json::Value::String(text) => output.push(TextCandidate {
            text: text.clone(),
            json_path: Some(path.to_owned()),
        }),
        _ => {}
    }
}

fn evidence_kind(kind: &str, text: &str) -> EvidenceKind {
    match kind {
        "user" => EvidenceKind::User,
        "assistant" => EvidenceKind::Assistant,
        "thinking" => EvidenceKind::Thinking,
        "tool_input" => EvidenceKind::ToolInput,
        "tool_result"
            if ["error", "failed", "failure", "exception", "denied"]
                .iter()
                .any(|marker| text.to_lowercase().contains(marker)) =>
        {
            EvidenceKind::ToolError
        }
        _ => EvidenceKind::ToolResult,
    }
}

fn kind_bonus(kind: EvidenceKind) -> u64 {
    match kind {
        EvidenceKind::User => 20,
        EvidenceKind::Assistant => 10,
        EvidenceKind::Thinking => 5,
        EvidenceKind::ToolInput => 25,
        EvidenceKind::ToolError => 80,
        EvidenceKind::ToolResult => 15,
    }
}

fn batch_failure_status(cancelled: bool) -> DeepSearchStatus {
    if cancelled {
        DeepSearchStatus::Stopped
    } else {
        DeepSearchStatus::Partial
    }
}

fn enforce_memory_limit(app: &tauri::AppHandle, active: &ActiveScan) {
    let controller = app.state::<DeepSearchController>();
    let other_cache_bytes = controller.retained_scan_bytes_except(active);
    let mut scan = active
        .scan
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    enforce_scan_memory(&mut scan, other_cache_bytes);
}

fn enforce_scan_memory(scan: &mut DeepScan, other_cache_bytes: usize) {
    let scan_limit = CACHE_BYTE_LIMIT.saturating_sub(other_cache_bytes);
    scan.retained_result_bytes = scan.estimated_bytes();
    if scan.retained_result_bytes > scan_limit {
        scan.release_for_resource_limit(scan_limit);
    }
}

fn response_for(_app: &tauri::AppHandle, active: &ActiveScan) -> DeepSearchResponse {
    let scan = active
        .scan
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    DeepSearchResponse {
        scope: scan.scope.clone(),
        scan_id: scan.scan_id,
        query_revision: scan.query_revision,
        available: true,
        status: scan.status,
        continuation_available: matches!(
            scan.status,
            DeepSearchStatus::Searching | DeepSearchStatus::Stopped
        ),
        results: scan.sorted_results(),
        invalidated_sessions: scan
            .invalidated
            .iter()
            .map(|identity| InvalidatedSession {
                environment_key: identity.environment_key.clone(),
                agent: identity.agent.clone(),
                session_id: identity.session_id.clone(),
            })
            .collect(),
        total_matching_sessions: scan.total_matching_sessions,
        coverage: scan.coverage.clone(),
    }
}

#[cfg(test)]
mod tests;
