//! Shared scheduler and TypeSafe executor for Jev-powered Burn Checks.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use antiburn_local::analysis::jev::{
    JevCheck, JevCheckPlan, JevError, JevExecutionOutcome, JevOrchestrationPermit, JevRequestBatch,
    JevResponse, JevRunProgress, JevSessionContext, MAX_REQUEST_TOKENS,
};
use antiburn_local::checks::DetectorId;
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager};
use tokio::sync::Notify;

use crate::jev::client::TypeSafeClient;
use crate::jev::config::{SystemOneConnection, SystemOneEndpoint, SystemOneProvider};
use crate::session_lifecycle::{SessionEvents, SessionRef};
use crate::store::{
    BurnCheckCandidate, BurnCheckInput, BurnCheckRequestAdmission, BurnCheckReservation,
    CachedAssessmentResponse, SessionKey, Store,
};

const RETRY_ATTEMPTS: usize = 3;
#[cfg(test)]
const IDLE_SECS: i64 = 180;
const POLL_SECS: u64 = 60;
const CANDIDATES_PER_WAKE: usize = 16;
const CANDIDATE_WORKERS: usize = 4;
const DISPATCHES_PER_TURN: u64 = 8;
const REQUEST_DEADLINE: Duration = Duration::from_secs(60);
const GLOBAL_REQUEST_BYTES: usize = 4 * 1024 * 1024;

pub(crate) async fn run_blocking_preparation<T: Send + 'static>(
    prepare: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    static SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    let permit = SLOTS
        .get_or_init(|| tokio::sync::Semaphore::new(CANDIDATE_WORKERS))
        .acquire()
        .await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        prepare()
    })
    .await?
}

fn request_bytes() -> &'static tokio::sync::Semaphore {
    static BYTES: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| tokio::sync::Semaphore::new(GLOBAL_REQUEST_BYTES))
}

fn request_slots() -> &'static tokio::sync::Semaphore {
    static SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| {
        tokio::sync::Semaphore::new(antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS)
    })
}

const GLOBAL_INPUT_TOKENS_PER_SECOND: u64 = 100_000;
// Keep the existing conservative admission estimate; provider limits are not
// inferred from undocumented or changing API rate limits.
const PROVIDER_START_INTERVAL: Duration = Duration::from_millis(25);

struct TokenStart {
    reservation_id: String,
    started: tokio::time::Instant,
    tokens: u64,
}

struct ProviderPacing {
    next_start: tokio::time::Instant,
    starts: std::collections::VecDeque<TokenStart>,
}

impl ProviderPacing {
    fn new(now: tokio::time::Instant) -> Self {
        Self {
            next_start: now,
            starts: std::collections::VecDeque::new(),
        }
    }

    fn admit(
        &mut self,
        reservation_id: &str,
        estimated_tokens: u64,
        now: tokio::time::Instant,
    ) -> bool {
        while self.starts.front().is_some_and(|start| {
            now.saturating_duration_since(start.started) >= Duration::from_secs(1)
        }) {
            self.starts.pop_front();
        }
        let tokens = self.starts.iter().map(|start| start.tokens).sum::<u64>();
        let estimated_tokens = estimated_tokens.clamp(1, MAX_REQUEST_TOKENS);
        if now < self.next_start
            || tokens.saturating_add(estimated_tokens) > GLOBAL_INPUT_TOKENS_PER_SECOND
        {
            return false;
        }
        self.starts.push_back(TokenStart {
            reservation_id: reservation_id.to_owned(),
            started: now,
            tokens: estimated_tokens,
        });
        self.next_start = now + PROVIDER_START_INTERVAL;
        true
    }

    fn settle(&mut self, reservation_id: &str, tokens: u64) {
        if let Some(start) = self
            .starts
            .iter_mut()
            .find(|start| start.reservation_id == reservation_id)
        {
            start.tokens = tokens.min(MAX_REQUEST_TOKENS);
        }
    }
}

struct ProviderPacingSet {
    jev: std::sync::Mutex<ProviderPacing>,
    ollama: std::sync::Mutex<ProviderPacing>,
    cloudflare: std::sync::Mutex<ProviderPacing>,
    custom: std::sync::Mutex<ProviderPacing>,
}

impl ProviderPacingSet {
    fn new(now: tokio::time::Instant) -> Self {
        Self {
            jev: std::sync::Mutex::new(ProviderPacing::new(now)),
            ollama: std::sync::Mutex::new(ProviderPacing::new(now)),
            cloudflare: std::sync::Mutex::new(ProviderPacing::new(now)),
            custom: std::sync::Mutex::new(ProviderPacing::new(now)),
        }
    }
}

fn provider_pacing(provider: SystemOneProvider) -> &'static std::sync::Mutex<ProviderPacing> {
    static PACING: std::sync::OnceLock<ProviderPacingSet> = std::sync::OnceLock::new();
    let pacing = PACING.get_or_init(|| ProviderPacingSet::new(tokio::time::Instant::now()));
    match provider {
        SystemOneProvider::Jev => &pacing.jev,
        SystemOneProvider::Ollama => &pacing.ollama,
        SystemOneProvider::Cloudflare => &pacing.cloudflare,
        SystemOneProvider::Custom => &pacing.custom,
    }
}

pub(crate) type WorkerFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

pub(crate) struct CandidateExecution<'a> {
    pub(crate) app: &'a tauri::AppHandle,
    pub(crate) store: &'a Store,
    pub(crate) candidate: &'a BurnCheckCandidate,
    pub(crate) client: TypeSafeClient,
    pub(crate) handle: &'a WorkerHandle,
    pub(crate) key_generation: u64,
    pub(crate) events: &'a SessionEvents,
}

#[derive(Clone, Copy)]
pub(crate) struct CheckPolicy {
    pub(crate) idle_secs: i64,
    pub(crate) lease_secs: i64,
    pub(crate) retry_delay_secs: i64,
}

/// Supplies feature policy, source preparation, and publication to one scheduler.
/// The adapter uses `run_prepared_check` for typed reduction and durable execution.
pub(crate) trait JevCheckDescriptor: Send + Sync {
    fn id(&self) -> &'static str;

    fn evaluator_revision(&self) -> String;

    fn policy(&self) -> CheckPolicy;

    fn run_candidate<'a>(&'a self, execution: CandidateExecution<'a>) -> WorkerFuture<'a>;
}

pub(crate) fn registered_checks() -> &'static [&'static dyn JevCheckDescriptor] {
    &[
        &crate::ignored_instructions_worker::CHECK,
        &crate::scope_creep_worker::CHECK,
        &crate::over_exploring_worker::CHECK,
        &crate::skill_opportunities_worker::CHECK,
    ]
}

pub(crate) fn registered_check_ids() -> Vec<DetectorId> {
    registered_checks()
        .iter()
        .map(|check| DetectorId::from_key(check.id()).expect("registered checks have detector IDs"))
        .collect()
}

/// Shared credential and wake state for the Jev worker.
pub(crate) struct WorkerHandle {
    wake: Notify,
    request_admission: Mutex<()>,
    check_generations: Mutex<std::collections::BTreeMap<String, u64>>,
    system_one: RwLock<(SystemOneConnection, Option<String>)>,
    key_generation: AtomicU64,
    runtime_enabled: AtomicBool,
    authentication_rejected: AtomicBool,
    discovery: tokio::sync::Mutex<Option<CapabilityDiscovery>>,
    turn_dispatches: Mutex<std::collections::BTreeMap<(String, SessionKey), u64>>,
}

struct CapabilityDiscovery {
    generation: u64,
    expires: tokio::time::Instant,
    capabilities: antiburn_local::analysis::jev::capabilities::ModelCapabilities,
}

impl WorkerHandle {
    pub(crate) fn start_candidate_turn(&self, check_id: &str, key: &SessionKey) {
        self.turn_dispatches
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert((check_id.to_owned(), key.clone()), 0);
    }

    pub(crate) fn end_candidate_turn(&self, check_id: &str, key: &SessionKey) {
        self.turn_dispatches
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&(check_id.to_owned(), key.clone()));
    }

    pub(crate) fn turn_exhausted(&self, check_id: &str, key: &SessionKey) -> bool {
        self.turn_dispatches
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&(check_id.to_owned(), key.clone()))
            .copied()
            .unwrap_or_default()
            >= DISPATCHES_PER_TURN
    }

    fn admit_turn_dispatch(
        &self,
        check_id: &str,
        key: &SessionKey,
        admit: impl FnOnce() -> anyhow::Result<BurnCheckRequestAdmission>,
    ) -> anyhow::Result<BurnCheckRequestAdmission> {
        let mut turns = self
            .turn_dispatches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let count = turns.entry((check_id.to_owned(), key.clone())).or_default();
        if *count >= DISPATCHES_PER_TURN {
            return Ok(BurnCheckRequestAdmission::Deferred);
        }
        let admission = admit()?;
        if admission == BurnCheckRequestAdmission::Admitted {
            *count = count.saturating_add(1);
        }
        Ok(admission)
    }

    pub(crate) async fn resolve_capabilities(
        &self,
        generation: u64,
    ) -> Result<antiburn_local::analysis::jev::capabilities::ModelCapabilities, JevError> {
        let (connection, credential) = self.active_system_one();
        if !self.key_is_current(generation) {
            return Err(JevError::Cancelled);
        }
        let defaults = connection
            .capabilities()
            .map_err(|_| JevError::InvalidRequestSchema)?;
        if connection.provider != SystemOneProvider::Ollama {
            return Ok(defaults);
        }
        let mut cache = self.discovery.lock().await;
        if !self.key_is_current(generation) {
            return Err(JevError::Cancelled);
        }
        if let Some(entry) = cache.as_ref()
            && entry.generation == generation
            && tokio::time::Instant::now() < entry.expires
        {
            return Ok(entry.capabilities.clone());
        }
        let SystemOneEndpoint::BaseUrl(base) = &connection.endpoint else {
            return Err(JevError::InvalidRequestSchema);
        };
        let discovery = async {
            crate::jev_ollama::OllamaClient::new(base, credential)?
                .discover(&connection.model)
                .await
        };
        let (capabilities, ttl) =
            match tokio::time::timeout(Duration::from_secs(10), discovery).await {
                Ok(Ok(model)) => (
                    connection.apply_capability_overrides(model.capabilities),
                    60,
                ),
                result => {
                    ::tracing::warn!(event = "ollama_runtime_discovery_failed", error = ?result);
                    (defaults, 30)
                }
            };
        if !self.key_is_current(generation) {
            return Err(JevError::Cancelled);
        }
        *cache = Some(CapabilityDiscovery {
            generation,
            expires: tokio::time::Instant::now() + Duration::from_secs(ttl),
            capabilities: capabilities.clone(),
        });
        Ok(capabilities)
    }

    /// Install a validated provider connection and its resolved secret.
    pub fn set_system_one_connection(
        &self,
        connection: SystemOneConnection,
        credential: Option<String>,
    ) -> Result<(), crate::jev::config::ConnectionValidationError> {
        self.install_system_one_connection(connection, credential, true)
    }

    pub(crate) fn install_system_one_connection(
        &self,
        connection: SystemOneConnection,
        credential: Option<String>,
        enabled: bool,
    ) -> Result<(), crate::jev::config::ConnectionValidationError> {
        connection.validate()?;
        let _admission = self
            .request_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut active = self
            .system_one
            .write()
            .unwrap_or_else(|error| error.into_inner());
        if active.0 != connection
            || active.1 != credential
            || self.runtime_enabled.load(Ordering::Acquire) != enabled
        {
            *active = (connection, credential);
            self.runtime_enabled.store(enabled, Ordering::Release);
            self.key_generation.fetch_add(1, Ordering::AcqRel);
            self.authentication_rejected.store(false, Ordering::Release);
        }
        drop(active);
        self.wake.notify_one();
        Ok(())
    }

    /// Disable dispatch and clear the secret. Keep the selected connection.
    pub(crate) fn suspend_system_one(&self) {
        let _admission = self
            .request_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut active = self
            .system_one
            .write()
            .unwrap_or_else(|error| error.into_inner());
        active.1 = None;
        self.runtime_enabled.store(false, Ordering::Release);
        self.key_generation.fetch_add(1, Ordering::AcqRel);
        self.authentication_rejected.store(false, Ordering::Release);
        drop(active);
        self.wake.notify_one();
    }

    /// Return the active connection without exposing the resolved credential.
    pub(crate) fn system_one_connection(&self) -> SystemOneConnection {
        self.system_one
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .0
            .clone()
    }

    fn active_system_one(&self) -> (SystemOneConnection, Option<String>) {
        self.system_one
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub(crate) fn execution_client(&self) -> Option<(TypeSafeClient, u64)> {
        let active = self
            .system_one
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let (connection, credential) = &*active;
        if !self.runtime_enabled.load(Ordering::Acquire) || self.authentication_rejected() {
            return None;
        }
        let generation = self.key_generation.load(Ordering::Acquire);
        if matches!(
            connection.provider,
            SystemOneProvider::Jev | SystemOneProvider::Cloudflare
        ) && credential.is_none()
        {
            return None;
        }
        let client =
            TypeSafeClient::new(credential.clone().unwrap_or_else(|| "unused".to_owned())).ok()?;
        connection.validate().ok().map(|()| (client, generation))
    }

    pub(crate) fn with_current_generation<T>(
        &self,
        generation: u64,
        action: impl FnOnce() -> T,
    ) -> Option<T> {
        let _active = self
            .system_one
            .read()
            .unwrap_or_else(|error| error.into_inner());
        (self.runtime_enabled.load(Ordering::Acquire)
            && self.key_generation.load(Ordering::Acquire) == generation)
            .then(action)
    }

    pub(crate) fn key_is_current(&self, generation: u64) -> bool {
        let active = self
            .system_one
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let credential_required = matches!(
            active.0.provider,
            SystemOneProvider::Jev | SystemOneProvider::Cloudflare
        );
        self.key_generation.load(Ordering::Acquire) == generation
            && self.runtime_enabled.load(Ordering::Acquire)
            && (!credential_required || active.1.is_some())
            && !self.authentication_rejected()
    }

    pub(crate) fn check_generation(&self, check_id: &str) -> u64 {
        self.check_generations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(check_id)
            .copied()
            .unwrap_or_default()
    }

    pub(crate) fn check_is_current(&self, check_id: &str, generation: u64) -> bool {
        self.check_generation(check_id) == generation
    }

    pub(crate) fn persist_check_transition<T, E>(
        &self,
        check_id: &str,
        persist: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let _admission = self
            .request_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let result = persist()?;
        let mut generations = self
            .check_generations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let generation = generations.entry(check_id.to_owned()).or_default();
        *generation = generation.saturating_add(1);
        self.wake.notify_one();
        Ok(result)
    }

    pub(crate) fn admit_if_current<T>(
        &self,
        key_generation: u64,
        check_id: &str,
        check_generation: u64,
        admit: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<Option<T>> {
        let _admission = self
            .request_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !self.key_is_current(key_generation)
            || !self.check_is_current(check_id, check_generation)
        {
            return Ok(None);
        }
        admit().map(Some)
    }

    pub(crate) fn is_available(&self) -> bool {
        let active = self
            .system_one
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let credential_required = matches!(
            active.0.provider,
            SystemOneProvider::Jev | SystemOneProvider::Cloudflare
        );
        active.0.validate().is_ok()
            && self.runtime_enabled.load(Ordering::Acquire)
            && (!credential_required || active.1.is_some())
            && !self.authentication_rejected()
    }

    pub(crate) fn authentication_rejected(&self) -> bool {
        self.authentication_rejected.load(Ordering::Acquire)
    }

    pub(crate) fn reject_authentication(
        &self,
        store: &Store,
        generation: u64,
    ) -> anyhow::Result<bool> {
        let _admission = self
            .request_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.with_current_generation(generation, || {
            self.authentication_rejected.store(true, Ordering::Release);
            self.wake.notify_one();
            store.set_internal_value_checked("internal:typesafeAuthRejectedV1", "true")?;
            Ok(true)
        })
        .unwrap_or(Ok(false))
    }
}

struct CandidateTurnGuard<'a> {
    handle: &'a WorkerHandle,
    check_id: &'static str,
    key: SessionKey,
}

impl Drop for CandidateTurnGuard<'_> {
    fn drop(&mut self) {
        self.handle.end_candidate_turn(self.check_id, &self.key);
    }
}

impl Default for WorkerHandle {
    fn default() -> Self {
        Self {
            wake: Notify::new(),
            request_admission: Mutex::new(()),
            check_generations: Mutex::new(std::collections::BTreeMap::new()),
            system_one: RwLock::new((SystemOneConnection::jev_default(), None)),
            key_generation: AtomicU64::new(0),
            runtime_enabled: AtomicBool::new(false),
            authentication_rejected: AtomicBool::new(false),
            discovery: tokio::sync::Mutex::new(None),
            turn_dispatches: Mutex::new(std::collections::BTreeMap::new()),
        }
    }
}

pub(crate) fn settle_authentication_rejection(
    app: &tauri::AppHandle,
    store: &Store,
    handle: &WorkerHandle,
    generation: u64,
    rejected: bool,
) -> anyhow::Result<()> {
    if rejected && handle.reject_authentication(store, generation)? {
        crate::jev::settings::changed(app);
    }
    Ok(())
}

/// Wake every registered check after source or Settings changes.
pub(crate) fn wake(app: &tauri::AppHandle) {
    app.state::<WorkerHandle>().wake.notify_one();
}

/// Run each registered check through the same bounded candidate scheduler.
pub(crate) fn spawn(app: &tauri::AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let checks = registered_checks();
        let mut lifecycle = app.state::<SessionEvents>().subscribe();
        let mut poll = tokio::time::interval(Duration::from_secs(POLL_SECS));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut skill_observation_after: Option<(String, String)> = None;
        let mut scope_observation_after: Option<(String, String)> = None;
        loop {
            let handle = app.state::<WorkerHandle>();
            let store = (*app.state::<Store>()).clone();
            let now = unix_now();
            let retry_at = match store.next_burn_check_retry_at(now) {
                Ok(retry_at) => retry_at,
                Err(error) => {
                    ::tracing::warn!(event = "burn_check_retry_schedule_failed", error = %error);
                    None
                }
            };
            let retry_delay = retry_at.map(|retry_at| {
                Duration::from_secs(
                    u64::try_from(retry_at.saturating_sub(now)).unwrap_or(POLL_SECS),
                )
            });
            tokio::select! {
                () = handle.wake.notified() => {},
                _ = poll.tick() => {},
                () = tokio::time::sleep(retry_delay.unwrap_or(Duration::from_secs(POLL_SECS))), if retry_at.is_some() => {},
                event = lifecycle.recv() => {
                    match event {
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {},
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
            let Some((client, key_generation)) = handle.execution_client() else {
                continue;
            };
            match store.burn_check_dependency_candidates(
                "scope_creep",
                scope_observation_after
                    .as_ref()
                    .map(|(agent, session)| (agent.as_str(), session.as_str())),
                32,
            ) {
                Ok(candidates) => {
                    for candidate in &candidates {
                        match crate::scope_creep_worker::reconcile_scope_dependencies(
                            &store, candidate,
                        ) {
                            Ok(true) => {
                                let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
                            }
                            Ok(false) => {}
                            Err(error) => {
                                ::tracing::warn!(event = "scope_dependency_observation_failed", error = %error)
                            }
                        }
                    }
                    scope_observation_after = if candidates.len() == 32 {
                        candidates.last().map(|candidate| {
                            (
                                candidate.session.key.agent.clone(),
                                candidate.session.key.session_id.clone(),
                            )
                        })
                    } else {
                        None
                    };
                }
                Err(error) => {
                    ::tracing::warn!(event = "scope_dependency_candidates_failed", error = %error)
                }
            }
            match store.skill_observation_candidates(
                skill_observation_after
                    .as_ref()
                    .map(|(agent, session)| (agent.as_str(), session.as_str())),
                32,
            ) {
                Ok(candidates) => {
                    for candidate in &candidates {
                        reconcile_skill_candidate(&app, &store, &handle, key_generation, candidate);
                    }
                    skill_observation_after = if candidates.len() == 32 {
                        candidates.last().map(|candidate| {
                            (
                                candidate.session.key.agent.clone(),
                                candidate.session.key.session_id.clone(),
                            )
                        })
                    } else {
                        None
                    };
                }
                Err(error) => {
                    ::tracing::warn!(event = "skill_input_observation_failed", error = %error)
                }
            }
            let mut cursor = match scheduler_cursor(&store) {
                Ok(cursor) => cursor,
                Err(error) => {
                    ::tracing::warn!(event = "burn_check_scheduler_load_failed", error = %error);
                    continue;
                }
            };
            let mut jobs = tokio::task::JoinSet::new();
            let mut in_flight = std::collections::BTreeSet::new();
            let mut scheduled = 0;
            loop {
                while scheduled < CANDIDATES_PER_WAKE && jobs.len() < CANDIDATE_WORKERS {
                    if !handle.key_is_current(key_generation) {
                        break;
                    }
                    let selected = match select_turn_excluding(
                        &store,
                        checks,
                        &cursor,
                        unix_now(),
                        &in_flight,
                    ) {
                        Ok(selected) => selected,
                        Err(error) => {
                            ::tracing::warn!(
                                event = "burn_check_candidates_failed",
                                error = %error
                            );
                            break;
                        }
                    };
                    let Some((index, candidate)) = selected else {
                        break;
                    };
                    let check = checks[index];
                    let check_generation = handle.check_generation(check.id());
                    if !handle.check_is_current(check.id(), check_generation) {
                        continue;
                    }
                    cursor.turn = match cursor.turn.checked_add(1) {
                        Some(turn) => turn,
                        None => break,
                    };
                    cursor.next_check = (index + 1) % checks.len();
                    if let Err(error) = store.serve_burn_check_candidate(
                        &candidate,
                        check.id(),
                        cursor.turn,
                        &serde_json::to_string(&cursor).expect("scheduler cursor serializes"),
                    ) {
                        ::tracing::warn!(event = "burn_check_scheduler_save_failed", error = %error);
                        break;
                    }
                    handle.start_candidate_turn(check.id(), &candidate.session.key);
                    in_flight.insert((check.id(), candidate.session.key.clone()));
                    let app = app.clone();
                    let store = store.clone();
                    let client = client.clone();
                    let task_candidate = candidate.clone();
                    let candidate_turn = cursor.turn;
                    let check_id = check.id();
                    jobs.spawn(async move {
                    let handle = app.state::<WorkerHandle>();
                    let events = app.state::<SessionEvents>();
                    let _turn = CandidateTurnGuard {
                        handle: &handle,
                        check_id,
                        key: task_candidate.session.key.clone(),
                    };
                    let future = check.run_candidate(CandidateExecution {
                        app: &app,
                        store: &store,
                        candidate: &task_candidate,
                        client,
                        handle: &handle,
                        key_generation,
                        events: &events,
                    });
                    tokio::pin!(future);
                    let mut cancellation = tokio::time::interval(Duration::from_millis(250));
                    let mut observation = tokio::time::interval(Duration::from_secs(5));
                    observation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    let result = loop {
                        tokio::select! {
                            result = &mut future => break result,
                            _ = cancellation.tick() => {
                                if !handle.key_is_current(key_generation)
                                    || !handle.check_is_current(check_id, check_generation) {
                                    break Ok(());
                                }
                            }
                            _ = observation.tick(), if check_id == "skill_opportunities" => {
                                reconcile_skill_candidate(&app, &store, &handle, key_generation, &task_candidate);
                            }
                        }
                    };
                    (candidate, check_id, candidate_turn, check_generation, result)
                });
                    scheduled += 1;
                }
                if jobs.is_empty() {
                    break;
                }
                let Some(Ok((candidate, check_id, candidate_turn, check_generation, result))) =
                    jobs.join_next().await
                else {
                    continue;
                };
                in_flight.remove(&(check_id, candidate.session.key.clone()));
                if candidate.historical {
                    crate::jev::settings::progress_changed(&app);
                }
                if let Err(error) = result {
                    ::tracing::warn!(
                        event = "burn_check_assessment_failed",
                        check_id,
                        agent = %candidate.session.key.agent,
                        error = %error,
                    );
                    let check = checks
                        .iter()
                        .find(|check| check.id() == check_id)
                        .expect("scheduled check is registered");
                    let (category, retry_at) =
                        candidate_error_policy(&error, check.policy(), unix_now());
                    match handle.admit_if_current(
                        key_generation,
                        check_id,
                        check_generation,
                        || {
                            store.settle_burn_check_candidate_error(
                                &candidate,
                                check_id,
                                &check.evaluator_revision(),
                                unix_now(),
                                category,
                                retry_at,
                            )
                        },
                    ) {
                        Ok(Some(true)) => {
                            let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
                        }
                        Ok(Some(false) | None) => {}
                        Err(error) => {
                            ::tracing::error!(event = "burn_check_candidate_settlement_failed", error = %error)
                        }
                    }
                }
                if let Err(error) = store.serve_burn_check_candidate(
                    &candidate,
                    check_id,
                    candidate_turn,
                    &serde_json::to_string(&cursor).expect("scheduler cursor serializes"),
                ) {
                    ::tracing::warn!(event = "burn_check_scheduler_save_failed", error = %error);
                    break;
                }
            }
        }
    })
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SchedulerCursor {
    turn: u64,
    next_check: usize,
}

fn scheduler_cursor(store: &Store) -> anyhow::Result<SchedulerCursor> {
    store
        .burn_check_scheduler_cursor()?
        .map(|json| serde_json::from_str(&json).map_err(Into::into))
        .unwrap_or_else(|| Ok(SchedulerCursor::default()))
}

#[cfg(test)]
fn select_turn(
    store: &Store,
    checks: &[&dyn JevCheckDescriptor],
    cursor: &SchedulerCursor,
    now: i64,
) -> anyhow::Result<Option<(usize, BurnCheckCandidate)>> {
    select_turn_excluding(
        store,
        checks,
        cursor,
        now,
        &std::collections::BTreeSet::new(),
    )
}

fn select_turn_excluding(
    store: &Store,
    checks: &[&dyn JevCheckDescriptor],
    cursor: &SchedulerCursor,
    now: i64,
    in_flight: &std::collections::BTreeSet<(&'static str, SessionKey)>,
) -> anyhow::Result<Option<(usize, BurnCheckCandidate)>> {
    let continuation = cursor.turn % 5 == 4;
    for lane in [continuation, !continuation] {
        for offset in 0..checks.len() {
            let index = (cursor.next_check + offset) % checks.len();
            let check = checks[index];
            let candidates = store.burn_check_candidates_in_lane(
                check.id(),
                &check.evaluator_revision(),
                now,
                check.policy().idle_secs,
                in_flight.len() + 1,
                Some(lane),
            )?;
            if let Some(candidate) = candidates
                .into_iter()
                .find(|candidate| !in_flight.contains(&(check.id(), candidate.session.key.clone())))
            {
                return Ok(Some((index, candidate)));
            }
        }
    }
    Ok(None)
}

fn candidate_error_policy(
    error: &anyhow::Error,
    policy: CheckPolicy,
    now: i64,
) -> (&'static str, Option<i64>) {
    match error.downcast_ref::<JevError>() {
        Some(
            JevError::InvalidCheckContext | JevError::InvalidCheckPlan | JevError::EmptyQuestions,
        ) => ("candidate_error", None),
        Some(error) => (
            error_category(error),
            Some(now.saturating_add(policy.retry_delay_secs)),
        ),
        None => (
            "candidate_retry",
            Some(now.saturating_add(policy.retry_delay_secs)),
        ),
    }
}

fn reconcile_skill_candidate(
    app: &tauri::AppHandle,
    store: &Store,
    handle: &WorkerHandle,
    generation: u64,
    candidate: &crate::store::BurnCheckCandidate,
) {
    match crate::skill_opportunities_worker::reconcile_skill_inputs(
        store, handle, generation, candidate,
    ) {
        Ok(Some(observation)) if observation.invalidated => {
            handle.wake.notify_one();
            let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
        }
        Ok(_) => {}
        Err(error) => ::tracing::warn!(event = "skill_input_reconciliation_failed", error = %error),
    }
}

#[cfg(test)]
fn next_candidate<T: Copy, C>(
    scheduled: &mut [(T, std::collections::VecDeque<C>)],
    next: &mut usize,
) -> Option<(T, C)> {
    for _ in 0..scheduled.len() {
        let index = *next % scheduled.len();
        *next = (index + 1) % scheduled.len();
        let (check, candidates) = &mut scheduled[index];
        if let Some(candidate) = candidates.pop_front() {
            return Some((*check, candidate));
        }
    }
    None
}

pub(crate) struct BatchExecution<'a> {
    pub(crate) app: &'a tauri::AppHandle,
    pub(crate) store: &'a Store,
    pub(crate) input: &'a BurnCheckInput,
    pub(crate) client: TypeSafeClient,
    pub(crate) handle: &'a WorkerHandle,
    pub(crate) key_generation: u64,
    pub(crate) events: &'a SessionEvents,
    pub(crate) policy: CheckPolicy,
    pub(crate) capabilities: &'a antiburn_local::analysis::jev::capabilities::ModelCapabilities,
}

/// Runs a typed plan through shared transport and fenced checkpoint storage.
/// The adapter serializes its cursor; the worker persists answers and progress.
pub(crate) async fn run_prepared_check<C, S>(
    execution: BatchExecution<'_>,
    check: &C,
    context: &JevSessionContext,
    plan: &mut JevCheckPlan<C::Prepared>,
    progress: JevRunProgress,
    orchestration: JevOrchestrationPermit,
    mut checkpoint: S,
) -> Result<JevExecutionOutcome<C::Result>, JevError>
where
    C: JevCheck,
    S: FnMut(&JevRunProgress) -> Result<String, JevError>,
{
    let outcome = antiburn_local::analysis::jev::run_jev_check_prepared_with_readiness(
        check,
        context,
        plan,
        progress,
        orchestration,
        |batch| {
            execute_jev_batch(
                BatchExecution {
                    client: execution.client.clone(),
                    ..execution
                },
                batch,
            )
        },
        |progress| {
            save_checkpoint(
                execution.store,
                execution.input,
                execution.policy,
                progress,
                &mut checkpoint,
                execution.handle,
                execution.key_generation,
            )
        },
        &|batch| {
            let (mut connection, _) = execution.handle.active_system_one();
            connection
                .model_revision
                .clone_from(&execution.capabilities.model_revision);
            let identities = batch_request_identities(&connection, execution.input, batch);
            let readiness = execution
                .store
                .burn_check_target_readiness(&identities, unix_now())
                .map_err(|_| JevError::ProgressStorageFailure)?;
            let [admission] = readiness.as_slice() else {
                return Err(JevError::InvalidCheckPlan);
            };
            match admission {
                BurnCheckRequestAdmission::Admitted => Ok(()),
                BurnCheckRequestAdmission::Unresolved => Err(JevError::RequestOutcomeUnknown),
                BurnCheckRequestAdmission::Exhausted => Err(JevError::ProviderUnavailable),
                BurnCheckRequestAdmission::Deferred | BurnCheckRequestAdmission::Stale => {
                    Err(JevError::Cancelled)
                }
            }
        },
    )
    .await;
    if let Err(error) = &outcome {
        let yielded = matches!(error, JevError::Cancelled)
            && execution
                .handle
                .turn_exhausted(&execution.input.check_id, &execution.input.key);
        let delay = if yielded {
            1
        } else {
            retry_delay(error, 0)
                .map(|delay| i64::try_from(delay.as_secs()).unwrap_or(i64::MAX))
                .unwrap_or(execution.policy.retry_delay_secs)
        };
        let retry_at = execution
            .store
            .burn_check_next_attempt_at(execution.input)
            .map_err(|_| JevError::ProgressStorageFailure)?
            .filter(|retry_at| *retry_at > unix_now())
            .unwrap_or_else(|| unix_now().saturating_add(delay));
        execution
            .store
            .release_failed_burn_check_lease(
                execution.input,
                if yielded {
                    "continuing"
                } else {
                    error_category(error)
                },
                retry_at,
            )
            .map_err(|_| JevError::ProgressStorageFailure)?;
    }
    outcome
}

fn save_checkpoint(
    store: &Store,
    input: &BurnCheckInput,
    policy: CheckPolicy,
    progress: &JevRunProgress,
    checkpoint: &mut impl FnMut(&JevRunProgress) -> Result<String, JevError>,
    handle: &WorkerHandle,
    key_generation: u64,
) -> Result<(), JevError> {
    let started = std::time::Instant::now();
    let serialized = checkpoint(progress)?;
    let serialization_elapsed_ms = started.elapsed().as_millis();
    let store_started = std::time::Instant::now();
    let saved = handle
        .with_current_generation(key_generation, || {
            store.save_burn_check_checkpoint(
                input,
                &serialized,
                Some(progress),
                unix_now(),
                policy.lease_secs,
                policy.idle_secs,
            )
        })
        .ok_or(JevError::Cancelled)?;
    if !saved
        .map_err(|error| {
            ::tracing::warn!(event = "burn_check_progress_save_failed", progress_bytes = serialized.len(), error = %error);
            JevError::ProgressStorageFailure
        })? {
        return Err(JevError::Cancelled);
    }
    ::tracing::debug!(
        event = "burn_check_progress_saved",
        progress_bytes = serialized.len(),
        completed_batches = progress.completed_batch_ids.len(),
        completed_work_items = progress.results.len(),
        serialization_elapsed_ms,
        store_elapsed_ms = store_started.elapsed().as_millis(),
        checkpoint_elapsed_ms = started.elapsed().as_millis(),
    );
    Ok(())
}

struct BatchContext<'a> {
    store: &'a Store,
    input: &'a BurnCheckInput,
    client: TypeSafeClient,
    handle: &'a WorkerHandle,
    key_generation: u64,
    events: &'a SessionEvents,
    idle_secs: i64,
    lease_secs: i64,
    capabilities: &'a antiburn_local::analysis::jev::capabilities::ModelCapabilities,
}

struct DispatchGuard<'a> {
    store: &'a Store,
    notify: &'a (dyn Fn() + Sync),
    reservation_id: String,
    provider: SystemOneProvider,
    settled: bool,
    rejected: bool,
}

impl Drop for DispatchGuard<'_> {
    fn drop(&mut self) {
        if !self.settled {
            let settlement = if self.rejected {
                provider_pacing(self.provider)
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .settle(&self.reservation_id, 0);
                self.store
                    .release_rejected_burn_check_usage(&self.reservation_id)
            } else {
                self.store
                    .settle_burn_check_usage(&self.reservation_id, None, unix_now())
            };
            if let Err(error) = settlement {
                ::tracing::error!(event = "jev_abandoned_request_settlement_failed", error = %error);
            }
            (self.notify)();
        }
    }
}

/// Execute one validated request with shared cache, reservation, retry, and
/// cancellation behavior. The request digest contains no check-specific ID.
pub(crate) async fn execute_jev_batch(
    execution: BatchExecution<'_>,
    batch: std::sync::Arc<JevRequestBatch>,
) -> Result<JevResponse, JevError> {
    let BatchExecution {
        app,
        store,
        input,
        client,
        handle,
        key_generation,
        events,
        policy,
        capabilities,
    } = execution;
    execute_batch(
        BatchContext {
            store,
            input,
            client,
            handle,
            key_generation,
            events,
            idle_secs: policy.idle_secs,
            lease_secs: policy.lease_secs,
            capabilities,
        },
        batch,
        &|| crate::jev::settings::changed(app),
    )
    .await
}

async fn execute_batch(
    execution: BatchContext<'_>,
    batch: std::sync::Arc<JevRequestBatch>,
    notify: &(dyn Fn() + Sync),
) -> Result<JevResponse, JevError> {
    let handle = execution.handle;
    let check_id = execution.input.check_id.clone();
    let check_generation = handle.check_generation(&check_id);
    let future = execute_batch_inner(execution, batch, notify, check_generation);
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => return result,
            () = tokio::time::sleep(Duration::from_millis(250)) => {
                if !handle.check_is_current(&check_id, check_generation) {
                    return Err(JevError::Cancelled);
                }
            }
        }
    }
}

async fn execute_batch_inner(
    execution: BatchContext<'_>,
    batch: std::sync::Arc<JevRequestBatch>,
    notify: &(dyn Fn() + Sync),
    check_generation: u64,
) -> Result<JevResponse, JevError> {
    let BatchContext {
        store,
        input,
        client,
        handle,
        key_generation,
        events,
        idle_secs,
        lease_secs,
        capabilities,
    } = execution;
    if !handle.key_is_current(key_generation)
        || session_is_active(events, &input.key)
        || !store
            .renew_burn_check_assessment(input, unix_now(), lease_secs, idle_secs)
            .map_err(|_| JevError::ProviderUnavailable)?
    {
        return Err(JevError::Cancelled);
    }
    let (mut connection, _) = handle.active_system_one();
    connection
        .model_revision
        .clone_from(&capabilities.model_revision);
    let resident_bytes = antiburn_local::analysis::jev::jev_batch_resident_bytes_with_capabilities(
        &batch,
        capabilities,
    )?;
    let provider = connection.provider;
    let provider_id = provider_id(provider);
    let cache_digest = compatible_request_identity(&connection, input, &batch.digest);
    if let Some(cached) = store
        .cached_assessment_response(provider_id, &cache_digest, unix_now())
        .map_err(|_| JevError::ProviderUnavailable)?
    {
        ::tracing::debug!(
            event = "typesafe_request_completed",
            model = %batch.request.model,
            cache_hit = true,
            request_bytes = batch.serialized_bytes,
            question_count = batch.request.questions.len(),
            work_item_count = batch.work_item_ids.len(),
            input_tokens = cached.input_tokens,
            output_tokens = 0,
            elapsed_ms = 0,
        );
        let response = decode_cached_response(cached, &batch)?;
        let cache_hit_id = request_cache_identity(input, &cache_digest);
        let Some(recorded) = handle.with_current_generation(key_generation, || {
            store.record_burn_check_cache_hit(
                input,
                provider_id,
                &batch.request.model,
                &input.check_id,
                &cache_hit_id,
            )
        }) else {
            return Err(JevError::Cancelled);
        };
        recorded.map_err(|_| JevError::ProviderUnavailable)?;
        notify();
        return Ok(response);
    }
    let request_identities = batch_request_identities(&connection, input, &batch);
    let attempt = store
        .burn_check_dispatch_attempts(&request_identities)
        .map_err(|_| JevError::ProgressStorageFailure)?;
    {
        match store
            .burn_check_dispatch_readiness(&request_identities, unix_now())
            .map_err(|_| JevError::ProgressStorageFailure)?
        {
            BurnCheckRequestAdmission::Admitted => {}
            BurnCheckRequestAdmission::Unresolved => return Err(JevError::RequestOutcomeUnknown),
            BurnCheckRequestAdmission::Exhausted => return Err(JevError::ProviderUnavailable),
            BurnCheckRequestAdmission::Deferred | BurnCheckRequestAdmission::Stale => {
                return Err(JevError::Cancelled);
            }
        }
        let _slot = acquire_budget(
            request_slots().acquire(),
            handle,
            key_generation,
            events,
            &input.key,
        )
        .await?;
        let _bytes = acquire_budget(
            request_bytes().acquire_many(
                u32::try_from(resident_bytes).map_err(|_| JevError::RequestSerialization)?,
            ),
            handle,
            key_generation,
            events,
            &input.key,
        )
        .await?;
        let started = std::time::Instant::now();
        if !handle.key_is_current(key_generation) || session_is_active(events, &input.key) {
            return Err(JevError::Cancelled);
        }
        if !store
            .renew_burn_check_assessment(input, unix_now(), lease_secs, idle_secs)
            .map_err(|_| JevError::ProviderUnavailable)?
        {
            return Err(JevError::Cancelled);
        }
        let reservation = store
            .reserve_burn_check_usage(
                input,
                provider_id,
                &batch.request.model,
                MAX_REQUEST_TOKENS,
                unix_now(),
                idle_secs,
            )
            .map_err(|_| JevError::ProviderUnavailable)?;
        let BurnCheckReservation::Reserved(reservation_id) = reservation else {
            return Err(JevError::Cancelled);
        };
        let mut dispatch = DispatchGuard {
            store,
            notify,
            reservation_id: reservation_id.clone(),
            provider,
            settled: false,
            rejected: true,
        };
        let estimated_tokens = u64::try_from(batch.serialized_bytes.div_ceil(3))
            .unwrap_or(MAX_REQUEST_TOKENS)
            .min(MAX_REQUEST_TOKENS);
        wait_for_provider(
            provider,
            &reservation_id,
            estimated_tokens,
            handle,
            key_generation,
            events,
            &input.key,
        )
        .await?;
        if !handle.key_is_current(key_generation)
            || session_is_active(events, &input.key)
            || !store
                .renew_burn_check_assessment(input, unix_now(), lease_secs, idle_secs)
                .map_err(|_| JevError::ProviderUnavailable)?
        {
            return Err(JevError::Cancelled);
        }
        let Some(admission) = handle
            .admit_if_current(key_generation, &input.check_id, check_generation, || {
                handle.admit_turn_dispatch(&input.check_id, &input.key, || {
                    store.admit_burn_check_requests(
                        input,
                        &request_identities,
                        &reservation_id,
                        unix_now(),
                    )
                })
            })
            .map_err(|_| JevError::ProviderUnavailable)?
        else {
            return Err(JevError::Cancelled);
        };
        match admission {
            BurnCheckRequestAdmission::Admitted => {}
            BurnCheckRequestAdmission::Stale => return Err(JevError::Cancelled),
            BurnCheckRequestAdmission::Unresolved => return Err(JevError::RequestOutcomeUnknown),
            BurnCheckRequestAdmission::Deferred => return Err(JevError::Cancelled),
            BurnCheckRequestAdmission::Exhausted => return Err(JevError::ProviderUnavailable),
        }
        dispatch.rejected = false;
        let (connection, credential) = handle.active_system_one();
        if !handle.key_is_current(key_generation) {
            return Err(JevError::Cancelled);
        }
        let call = async {
            match connection.provider {
                SystemOneProvider::Jev => {
                    client
                        .evaluate_at_with_capabilities(
                            &batch.request,
                            &crate::jev::config::system_one_endpoint(),
                            capabilities,
                        )
                        .await
                }
                SystemOneProvider::Ollama => {
                    let SystemOneEndpoint::BaseUrl(base) = &connection.endpoint else {
                        return Err(JevError::InvalidRequestSchema);
                    };
                    crate::jev_ollama::OllamaClient::new(base, credential.clone())
                        .map_err(|_| JevError::ProviderUnavailable)?
                        .evaluate(&batch.request, capabilities)
                        .await
                        .map_err(|error| match error {
                            crate::jev_ollama::OllamaError::AuthenticationRejected => {
                                JevError::AuthenticationRejected
                            }
                            crate::jev_ollama::OllamaError::RequestOutcomeUnknown => {
                                JevError::RequestOutcomeUnknown
                            }
                            crate::jev_ollama::OllamaError::ResponseTooLarge => {
                                JevError::ResponseTooLarge
                            }
                            crate::jev_ollama::OllamaError::ResponseDecode => {
                                JevError::ResponseDecode
                            }
                            crate::jev_ollama::OllamaError::InvalidRequest
                            | crate::jev_ollama::OllamaError::RequestBodyTooLarge => {
                                JevError::InvalidRequestSchema
                            }
                            _ => JevError::ProviderUnavailable,
                        })
                }
                SystemOneProvider::Cloudflare => {
                    let SystemOneEndpoint::CloudflareAccount(account_id) = &connection.endpoint
                    else {
                        return Err(JevError::InvalidRequestSchema);
                    };
                    let Some(credential) = credential else {
                        return Err(JevError::AuthenticationRejected);
                    };
                    crate::jev_cloudflare::CloudflareClient::new(
                        account_id.clone(),
                        credential,
                        connection.model.clone(),
                    )?
                    .evaluate_with_capabilities(&batch.request, capabilities)
                    .await
                }
                SystemOneProvider::Custom => {
                    let endpoint = connection
                        .inference_endpoint()
                        .map_err(|_| JevError::InvalidRequestSchema)?;
                    crate::jev::client::evaluate_custom(
                        &endpoint,
                        credential.as_deref(),
                        connection.response_mode,
                        &batch.request,
                        capabilities,
                    )
                    .await
                }
            }
        };
        let call = request_with_deadline(call);
        tokio::pin!(call);
        let response = loop {
            tokio::select! {
                result = &mut call => {
                    break result;
                }
                () = tokio::time::sleep(Duration::from_millis(250)) => {
                    if !handle.key_is_current(key_generation) || session_is_active(events, &input.key) {
                        store.settle_burn_check_usage(&reservation_id, None, unix_now())
                            .map_err(|_| JevError::ProviderUnavailable)?;
                        dispatch.settled = true;
                        notify();
                        return Err(JevError::Cancelled);
                    }
                }
            }
        };
        dispatch.rejected = response.as_ref().err().is_some_and(request_was_rejected);
        match response {
            Ok(response) => {
                let Some(recorded) = record_response_if_current(
                    handle,
                    key_generation,
                    store,
                    &reservation_id,
                    provider_id,
                    &cache_digest,
                    &response,
                ) else {
                    settle_stale_response(store, &reservation_id, &response)?;
                    dispatch.settled = true;
                    notify();
                    return Err(JevError::Cancelled);
                };
                provider_pacing(provider)
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .settle(&reservation_id, response.usage.input_tokens);
                ::tracing::debug!(
                    event = "typesafe_request_completed",
                    model = %response.model,
                    cache_hit = false,
                    attempt = attempt + 1,
                    request_bytes = batch.serialized_bytes,
                    question_count = batch.request.questions.len(),
                    work_item_count = batch.work_item_ids.len(),
                    input_tokens = response.usage.input_tokens,
                    output_tokens = response.usage.output_tokens,
                    elapsed_ms = started.elapsed().as_millis(),
                );
                if recorded.is_err() {
                    store
                        .settle_burn_check_usage_with_output(
                            &reservation_id,
                            Some(response.usage.input_tokens),
                            Some(response.usage.output_tokens),
                            unix_now(),
                        )
                        .map_err(|_| JevError::ProviderUnavailable)?;
                    dispatch.settled = true;
                    notify();
                    return Err(JevError::ProviderUnavailable);
                }
                dispatch.settled = true;
                notify();
                Ok(response)
            }
            Err(error) => {
                let retry_at = retry_delay(&error, attempt).map(|delay| {
                    unix_now().saturating_add(i64::try_from(delay.as_secs()).unwrap_or(i64::MAX))
                });
                store
                    .defer_burn_check_dispatch(input, &request_identities, retry_at)
                    .map_err(|_| JevError::ProgressStorageFailure)?;
                if request_was_rejected(&error) {
                    provider_pacing(provider)
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .settle(&reservation_id, 0);
                    store
                        .release_rejected_burn_check_usage(&reservation_id)
                        .map_err(|_| JevError::ProviderUnavailable)?;
                } else {
                    store
                        .settle_burn_check_usage(&reservation_id, None, unix_now())
                        .map_err(|_| JevError::ProviderUnavailable)?;
                }
                dispatch.settled = true;
                ::tracing::debug!(
                    event = "typesafe_request_failed",
                    model = %batch.request.model,
                    attempt = attempt + 1,
                    request_bytes = batch.serialized_bytes,
                    question_count = batch.request.questions.len(),
                    work_item_count = batch.work_item_ids.len(),
                    failure_category = error_category(&error),
                    retry_delay_ms = 0,
                    elapsed_ms = started.elapsed().as_millis(),
                );
                notify();
                Err(error)
            }
        }
    }
}

async fn request_with_deadline(
    call: impl Future<Output = Result<JevResponse, JevError>>,
) -> Result<JevResponse, JevError> {
    tokio::time::timeout(REQUEST_DEADLINE, call)
        .await
        .unwrap_or(Err(JevError::RequestOutcomeUnknown))
}

async fn wait_for_provider(
    provider: SystemOneProvider,
    reservation_id: &str,
    estimated_tokens: u64,
    handle: &WorkerHandle,
    generation: u64,
    events: &SessionEvents,
    key: &SessionKey,
) -> Result<(), JevError> {
    loop {
        if !handle.key_is_current(generation) || session_is_active(events, key) {
            return Err(JevError::Cancelled);
        }
        let admitted = provider_pacing(provider)
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .admit(
                reservation_id,
                estimated_tokens,
                tokio::time::Instant::now(),
            );
        if admitted {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn settle_stale_response(
    store: &Store,
    reservation_id: &str,
    response: &JevResponse,
) -> Result<(), JevError> {
    store
        .settle_burn_check_usage_with_output(
            reservation_id,
            Some(response.usage.input_tokens),
            Some(response.usage.output_tokens),
            unix_now(),
        )
        .map_err(|_| JevError::ProviderUnavailable)
}

fn record_response_if_current(
    handle: &WorkerHandle,
    generation: u64,
    store: &Store,
    reservation_id: &str,
    provider: &str,
    request_digest: &str,
    response: &JevResponse,
) -> Option<Result<(), JevError>> {
    handle.with_current_generation(generation, || {
        let response_json =
            serde_json::to_string(response).map_err(|_| JevError::ResponseDecode)?;
        store
            .record_burn_check_response(
                reservation_id,
                CachedAssessmentResponse {
                    provider: provider.to_owned(),
                    request_digest: request_digest.to_owned(),
                    returned_model: response.model.clone(),
                    response_json,
                    input_tokens: response.usage.input_tokens,
                    output_tokens: response.usage.output_tokens,
                    created_at_epoch: unix_now(),
                },
            )
            .map_err(|_| JevError::ProviderUnavailable)
    })
}

fn request_was_rejected(error: &JevError) -> bool {
    matches!(
        error,
        JevError::AuthenticationRejected
            | JevError::InvalidRequestSchema
            | JevError::RateLimited { .. }
            | JevError::ProviderOverloaded { .. }
            | JevError::ProviderUnavailable
    )
}

async fn acquire_budget<T>(
    acquire: impl Future<Output = Result<T, tokio::sync::AcquireError>>,
    handle: &WorkerHandle,
    generation: u64,
    events: &SessionEvents,
    key: &SessionKey,
) -> Result<T, JevError> {
    tokio::pin!(acquire);
    loop {
        if !handle.key_is_current(generation) || session_is_active(events, key) {
            return Err(JevError::Cancelled);
        }
        tokio::select! {
            permit = &mut acquire => return permit.map_err(|_| JevError::Cancelled),
            () = tokio::time::sleep(Duration::from_millis(250)) => {},
        }
    }
}

fn decode_cached_response(
    cached: CachedAssessmentResponse,
    batch: &JevRequestBatch,
) -> Result<JevResponse, JevError> {
    let mut response: JevResponse =
        serde_json::from_str(&cached.response_json).map_err(|_| JevError::ResponseDecode)?;
    antiburn_local::analysis::jev::validate_jev_response(&response, &batch.request)?;
    if response.model != cached.returned_model {
        return Err(JevError::ResponseModelMismatch);
    }
    response.usage = antiburn_local::analysis::jev::JevUsage {
        input_tokens: 0,
        output_tokens: 0,
    };
    Ok(response)
}

pub(crate) fn retry_delay(error: &JevError, attempt: usize) -> Option<Duration> {
    if attempt >= RETRY_ATTEMPTS - 1 {
        return None;
    }
    let backoff = Duration::from_secs(if attempt == 0 { 5 } else { 30 });
    match error {
        JevError::RateLimited { retry_after } | JevError::ProviderOverloaded { retry_after } => {
            Some(retry_after.unwrap_or_default().max(backoff))
        }
        JevError::ProviderUnavailable => Some(backoff),
        _ => None,
    }
}

fn session_is_active(events: &SessionEvents, key: &SessionKey) -> bool {
    events
        .presence(&[SessionRef::from(key)])
        .present
        .iter()
        .any(|session| !session.quiet)
}

pub(crate) fn error_category(error: &JevError) -> &'static str {
    match error {
        JevError::AuthenticationRejected => "authentication_rejected",
        JevError::InvalidRequestSchema => "invalid_request_schema",
        JevError::RateLimited { .. } => "rate_limited",
        JevError::ProviderOverloaded { .. } => "provider_overloaded",
        JevError::ProviderUnavailable => "provider_unavailable",
        JevError::RequestOutcomeUnknown => "outcome_unknown",
        JevError::RequestSerialization
        | JevError::EmptyQuestions
        | JevError::QuestionLimitExceeded
        | JevError::UnsupportedModel
        | JevError::RequestTooLarge { .. }
        | JevError::RequestTokenLimitExceeded { .. } => "invalid_request",
        JevError::ResponseModelMismatch
        | JevError::ResponseAnswerCountMismatch
        | JevError::ResponseAnswerMissing
        | JevError::ResponseAnswerTypeMismatch
        | JevError::InvalidChoiceDistribution
        | JevError::InvalidNoulProbability
        | JevError::InvalidScoreDistribution
        | JevError::InvalidProbabilitySum
        | JevError::WorkItemHasNoAnswers => "invalid_response",
        JevError::ResponseTooLarge => "response_too_large",
        JevError::ResponseDecode => "response_decode",
        JevError::ResponseUsageExceeded => "response_usage_exceeded",
        JevError::ProgressStorageFailure => "progress_storage_failed",
        JevError::InvalidCheckContext | JevError::InvalidCheckPlan => "invalid_assessment_plan",
        JevError::Cancelled => "cancelled",
    }
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

pub(crate) fn request_cache_identity(input: &BurnCheckInput, request_digest: &str) -> String {
    hash_identity([
        input.key.environment_key.as_str(),
        input.key.agent.as_str(),
        input.key.session_id.as_str(),
        input.check_id.as_str(),
        input.input_revision.as_str(),
        request_digest,
    ])
}

fn provider_id(provider: SystemOneProvider) -> &'static str {
    match provider {
        SystemOneProvider::Jev => "typesafe-systemone",
        SystemOneProvider::Ollama => "ollama-systemone",
        SystemOneProvider::Cloudflare => "cloudflare-workers-ai",
        SystemOneProvider::Custom => "custom-systemone",
    }
}

fn compatible_request_identity(
    connection: &SystemOneConnection,
    input: &BurnCheckInput,
    semantic_request_digest: &str,
) -> String {
    let endpoint = match &connection.endpoint {
        SystemOneEndpoint::ProviderDefault => "typesafe-default".to_owned(),
        SystemOneEndpoint::BaseUrl(value) | SystemOneEndpoint::ExactUrl(value) => {
            hash_identity([value, "", "", "", "", ""])
        }
        SystemOneEndpoint::CloudflareAccount(value) => value.clone(),
    };
    let mode = format!("{:?}", connection.response_mode);
    let revision = connection.revision.to_string();
    let context = format!("{:?}", connection.context_override);
    let credential_binding = match &connection.credential {
        Some(crate::jev::config::CredentialReference::LegacyTypeSafe) => {
            "legacy-typesafe".to_owned()
        }
        Some(crate::jev::config::CredentialReference::Connection(id)) => {
            hash_identity([id, "", "", "", "", ""])
        }
        None => "keyless".to_owned(),
    };
    let connection_identity = hash_identity([
        &format!("{:?}", connection.provider),
        &endpoint,
        &hash_identity([
            &connection.model,
            connection.model_revision.as_deref().unwrap_or(""),
            "",
            "",
            "",
            "",
        ]),
        &mode,
        &format!("{revision}:{credential_binding}"),
        &format!(
            "{}:{}:{}",
            input.check_id, input.evaluator_revision, context
        ),
    ]);
    hash_identity([
        &connection_identity,
        semantic_request_digest,
        "",
        "",
        "",
        "",
    ])
}

pub(crate) fn batch_request_identities(
    connection: &SystemOneConnection,
    input: &BurnCheckInput,
    batch: &JevRequestBatch,
) -> Vec<String> {
    batch
        .work_item_digests
        .values()
        .map(|digest| {
            hash_identity([
                &input.key.environment_key,
                &input.key.agent,
                &input.key.session_id,
                &input.incarnation.to_string(),
                &input.check_id,
                &compatible_request_identity(connection, input, digest),
            ])
        })
        .collect()
}

fn hash_identity(parts: [&str; 6]) -> String {
    let mut hash = Sha256::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            hash.update(b"\0");
        }
        hash.update(part.as_bytes());
    }
    let digest = hash.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    pub(super) fn checkpoint_fixture() -> (Store, BurnCheckInput, WorkerHandle, u64) {
        let store = Store::open_in_memory(std::path::Path::new("synthetic-state")).unwrap();
        checkpoint_fixture_in(store)
    }

    pub(super) fn checkpoint_fixture_in(
        store: Store,
    ) -> (Store, BurnCheckInput, WorkerHandle, u64) {
        let now = unix_now();
        let record = crate::store::SessionRecord {
            key: SessionKey::new("native", "claude-code", "checkpoint"),
            source_kind: "file".into(),
            source_label: "synthetic.jsonl".into(),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: None,
            surface: "cli".into(),
            updated_at_epoch: Some(now - 600),
            activity_cursor: "cursor".into(),
            activity_source: "mtime".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: None,
        };
        store
            .upsert_sessions(
                std::slice::from_ref(&record),
                &crate::agents::evidence_cohort(),
            )
            .unwrap();
        store
            .lock()
            .execute(
                "UPDATE session_evidence SET status = 'ready', analyzed_generation = 0,
             parser_revision = ?1, analyzer_revision = ?2, evidence_schema_revision = ?3,
             evidence_json = '{}', claim_fence = 1, published_fence = 1",
                rusqlite::params![
                    antiburn_local::analysis::PARSER_REVISION,
                    antiburn_local::analysis::ANALYZER_REVISION,
                    antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION
                ],
            )
            .unwrap();
        store.set_internal_value(
            "internal:burnChecksEnabledAtEpochV1",
            &(now - 1000).to_string(),
        );
        let input = BurnCheckInput {
            key: record.key,
            check_id: "ignored_instructions".into(),
            input_revision: "input-v1".into(),
            evaluator_revision: "eval-v1".into(),
            incarnation: 1,
            source_generation: 0,
            source_fingerprint: None,
            published_fence: 1,
            activity_cursor: "cursor".into(),
            boundary_at_epoch: now - 1000,
        };
        assert!(store.queue_burn_check_assessment(&input, now, 180).unwrap());
        assert!(
            store
                .claim_burn_check_assessment(&input, now, 300, 180)
                .unwrap()
        );
        let handle = WorkerHandle::default();
        handle
            .set_system_one_connection(
                SystemOneConnection::jev_default(),
                Some("synthetic-key".into()),
            )
            .unwrap();
        let generation = handle.key_generation.load(Ordering::Acquire);
        (store, input, handle, generation)
    }

    #[test]
    fn a_turn_counts_only_admitted_dispatches_and_yields_at_its_limit() {
        let (store, input, handle, generation) = checkpoint_fixture();
        handle.start_candidate_turn(&input.check_id, &input.key);
        for index in 0..9 {
            let identity = [format!("semantic-target-{index}")];
            let admission = handle
                .admit_if_current(generation, &input.check_id, 0, || {
                    handle.admit_turn_dispatch(&input.check_id, &input.key, || {
                        store.admit_burn_check_requests(
                            &input,
                            &identity,
                            "reservation",
                            unix_now(),
                        )
                    })
                })
                .unwrap()
                .unwrap();
            assert_eq!(
                admission,
                if index < DISPATCHES_PER_TURN as usize {
                    BurnCheckRequestAdmission::Admitted
                } else {
                    BurnCheckRequestAdmission::Deferred
                }
            );
            assert_eq!(
                store.burn_check_dispatch_attempts(&identity).unwrap(),
                usize::from(index < DISPATCHES_PER_TURN as usize)
            );
        }
        assert!(handle.turn_exhausted(&input.check_id, &input.key));
        let mut other_key = input.key.clone();
        other_key.session_id.push_str("-parallel");
        handle.start_candidate_turn(&input.check_id, &other_key);
        assert!(!handle.turn_exhausted(&input.check_id, &other_key));
        handle.end_candidate_turn(&input.check_id, &other_key);
        handle.end_candidate_turn(&input.check_id, &input.key);
        handle.start_candidate_turn(&input.check_id, &input.key);
        let unresolved = handle
            .admit_turn_dispatch(&input.check_id, &input.key, || {
                store.admit_burn_check_requests(
                    &input,
                    &["semantic-target-0".into()],
                    "another",
                    unix_now(),
                )
            })
            .unwrap();
        assert_eq!(unresolved, BurnCheckRequestAdmission::Unresolved);
        assert!(!handle.turn_exhausted(&input.check_id, &input.key));
        handle.end_candidate_turn(&input.check_id, &input.key);
    }

    #[tokio::test(start_paused = true)]
    async fn request_deadline_releases_capacity_and_keeps_unknown_delivery_blocked() {
        let (store, input, _, _) = checkpoint_fixture();
        let BurnCheckReservation::Reserved(reservation_id) = store
            .reserve_burn_check_usage(
                &input,
                "typesafe-systemone",
                "jev-1.13.0",
                MAX_REQUEST_TOKENS,
                unix_now(),
                180,
            )
            .unwrap()
        else {
            panic!("synthetic reservation")
        };
        let identities = ["timed-out-target".to_owned()];
        assert_eq!(
            store
                .admit_burn_check_requests(&input, &identities, &reservation_id, unix_now())
                .unwrap(),
            BurnCheckRequestAdmission::Admitted
        );
        let slots = tokio::sync::Semaphore::new(1);
        let notify = || {};
        let outcome = request_with_deadline(async {
            let _slot = slots.acquire().await.unwrap();
            let _dispatch = DispatchGuard {
                store: &store,
                notify: &notify,
                reservation_id,
                provider: SystemOneProvider::Jev,
                settled: false,
                rejected: false,
            };
            std::future::pending().await
        })
        .await;
        assert_eq!(outcome, Err(JevError::RequestOutcomeUnknown));
        assert_eq!(slots.available_permits(), 1);
        assert!(
            store
                .burn_check_requests_are_unresolved(&identities)
                .unwrap()
        );
        assert_eq!(store.burn_check_dispatch_attempts(&identities).unwrap(), 1);
        assert_eq!(
            store.burn_check_usage_summary().unwrap().unknown_outcomes,
            1
        );
    }

    #[test]
    fn authentication_rejection_cannot_disable_a_replacement_connection() {
        let (store, _, handle, generation) = checkpoint_fixture();
        handle
            .set_system_one_connection(
                SystemOneConnection::jev_default(),
                Some("replacement".into()),
            )
            .unwrap();
        assert!(!handle.reject_authentication(&store, generation).unwrap());
        assert!(handle.is_available());
        assert_eq!(
            store.internal_value("internal:typesafeAuthRejectedV1"),
            None
        );
    }

    #[test]
    fn check_transitions_fence_only_the_changed_check_and_failed_writes_keep_work_current() {
        let (_, _, handle, generation) = checkpoint_fixture();
        let ignored = handle.check_generation("ignored_instructions");
        let scope = handle.check_generation("scope_creep");
        assert!(
            handle
                .persist_check_transition("ignored_instructions", || Err::<(), _>("write"))
                .is_err()
        );
        assert!(handle.check_is_current("ignored_instructions", ignored));
        let during_write = std::cell::Cell::new(None);
        handle
            .persist_check_transition("ignored_instructions", || {
                during_write.set(Some(handle.check_generation("ignored_instructions")));
                Ok::<_, std::convert::Infallible>(())
            })
            .unwrap();
        assert!(!handle.check_is_current("ignored_instructions", during_write.get().unwrap()));
        assert!(handle.check_is_current("scope_creep", scope));
        assert!(
            handle
                .admit_if_current::<()>(generation, "ignored_instructions", ignored, || panic!(
                    "stale check dispatch"
                ))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            handle
                .admit_if_current(generation, "scope_creep", scope, || Ok(7))
                .unwrap(),
            Some(7)
        );
    }

    #[test]
    fn authentication_rejection_preserves_failure_settlement_for_current_generation() {
        let (store, input, handle, generation) = checkpoint_fixture();
        assert!(handle.reject_authentication(&store, generation).unwrap());
        assert!(!handle.key_is_current(generation));
        assert!(
            handle
                .with_current_generation(generation, || {
                    store.fail_burn_check_assessment_with_result(
                        &input,
                        &crate::store::BurnCheckFailure {
                            error_category: "authentication_rejected",
                            result_json: "{}",
                            progress_json: "{}",
                            retry_at_epoch: None,
                        },
                        unix_now(),
                        180,
                    )
                })
                .unwrap()
                .unwrap()
        );
        let saved = store
            .burn_check_assessment(&input.key, &input.check_id)
            .unwrap()
            .unwrap();
        assert_eq!(saved.status, "failed");
        assert_eq!(saved.result_json.as_deref(), Some("{}"));
        assert_eq!(
            store
                .internal_value("internal:typesafeAuthRejectedV1")
                .as_deref(),
            Some("true")
        );
        assert!(handle.execution_client().is_none());
    }

    fn reusable_progress(item: &str) -> JevRunProgress {
        use antiburn_local::analysis::jev::{JevAnswer, JevUsage, JevWorkItemResult, PINNED_MODEL};
        let mut progress = JevRunProgress::default();
        progress.completed_batch_ids.extend([
            "reuse-scope:exact".into(),
            format!("reuse-item:{item}:exact"),
        ]);
        progress.results.insert(
            item.into(),
            JevWorkItemResult {
                request_id: "batch".into(),
                work_item_id: item.into(),
                answers: std::collections::BTreeMap::from([(
                    "q".into(),
                    JevAnswer::Noul { noul: 0.8 },
                )]),
                evidence: Vec::new(),
                model: PINNED_MODEL.into(),
                usage: JevUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            },
        );
        progress
    }

    #[test]
    fn checkpoint_rejects_stale_fences_without_saving_reusable_answers() {
        let (store, input, handle, generation) = checkpoint_fixture();
        let policy = registered_checks()[0].policy();
        for stale in [
            BurnCheckInput {
                published_fence: 2,
                ..input.clone()
            },
            BurnCheckInput {
                evaluator_revision: "old-evaluator".into(),
                ..input.clone()
            },
            BurnCheckInput {
                input_revision: "old-input".into(),
                ..input.clone()
            },
            BurnCheckInput {
                incarnation: 2,
                ..input.clone()
            },
            BurnCheckInput {
                source_generation: 1,
                ..input.clone()
            },
        ] {
            assert!(matches!(
                save_checkpoint(
                    &store,
                    &stale,
                    policy,
                    &reusable_progress("stale"),
                    &mut |_| Ok("{\"stale\":true}".into()),
                    &handle,
                    generation
                ),
                Err(JevError::Cancelled)
            ));
            assert!(store.burn_check_work_answers(&input).unwrap().is_empty());
            assert_eq!(
                store
                    .burn_check_assessment(&input.key, &input.check_id)
                    .unwrap()
                    .unwrap()
                    .progress_json,
                "{}"
            );
        }
    }

    #[test]
    fn checkpoint_serialization_and_sql_failures_leave_answers_and_progress_unchanged() {
        let (store, input, handle, generation) = checkpoint_fixture();
        let policy = registered_checks()[0].policy();
        let progress = reusable_progress("answer");
        assert!(matches!(
            save_checkpoint(
                &store,
                &input,
                policy,
                &progress,
                &mut |_| Err(JevError::InvalidCheckPlan),
                &handle,
                generation
            ),
            Err(JevError::InvalidCheckPlan)
        ));
        assert!(store.burn_check_work_answers(&input).unwrap().is_empty());
        store
            .lock()
            .execute_batch(
                "CREATE TRIGGER reject_work_answer BEFORE INSERT ON burn_check_work_answer
            BEGIN SELECT RAISE(ABORT, 'synthetic storage failure'); END;",
            )
            .unwrap();
        assert!(matches!(
            save_checkpoint(
                &store,
                &input,
                policy,
                &progress,
                &mut |_| Ok("{\"page\":1}".into()),
                &handle,
                generation
            ),
            Err(JevError::ProgressStorageFailure)
        ));
        assert!(store.burn_check_work_answers(&input).unwrap().is_empty());
        assert_eq!(
            store
                .burn_check_assessment(&input.key, &input.check_id)
                .unwrap()
                .unwrap()
                .progress_json,
            "{}"
        );
        store
            .lock()
            .execute_batch("DROP TRIGGER reject_work_answer;")
            .unwrap();
        save_checkpoint(
            &store,
            &input,
            policy,
            &progress,
            &mut |_| Ok("{\"page\":1}".into()),
            &handle,
            generation,
        )
        .unwrap();
        assert_eq!(
            store.burn_check_work_answers(&input).unwrap()[0].2,
            progress.results["answer"]
        );
        assert_eq!(
            store
                .burn_check_assessment(&input.key, &input.check_id)
                .unwrap()
                .unwrap()
                .progress_json,
            "{\"page\":1}"
        );
    }

    #[test]
    fn checkpoint_after_deletion_or_clear_cannot_recreate_answers() {
        for clear in [false, true] {
            let (store, input, handle, generation) = checkpoint_fixture();
            let progress = reusable_progress("deleted");
            assert!(matches!(
                save_checkpoint(
                    &store,
                    &input,
                    registered_checks()[0].policy(),
                    &progress,
                    &mut |_| {
                        if clear {
                            store.clear_local_session_data().unwrap();
                        } else {
                            store.delete_session(&input.key).unwrap();
                        }
                        Ok("{\"page\":1}".into())
                    },
                    &handle,
                    generation
                ),
                Err(JevError::Cancelled)
            ));
            assert!(store.burn_check_work_answers(&input).unwrap().is_empty());
            assert!(
                store
                    .burn_check_assessment(&input.key, &input.check_id)
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn checkpoint_provider_change_during_serialization_cancels_all_writes() {
        let (store, input, handle, generation) = checkpoint_fixture();
        assert!(matches!(
            save_checkpoint(
                &store,
                &input,
                registered_checks()[0].policy(),
                &reusable_progress("old-provider"),
                &mut |_| {
                    handle
                        .set_system_one_connection(
                            SystemOneConnection::jev_default(),
                            Some("replacement-key".into()),
                        )
                        .unwrap();
                    Ok("{\"page\":1}".into())
                },
                &handle,
                generation
            ),
            Err(JevError::Cancelled)
        ));
        assert!(store.burn_check_work_answers(&input).unwrap().is_empty());
        assert_eq!(
            store
                .burn_check_assessment(&input.key, &input.check_id)
                .unwrap()
                .unwrap()
                .progress_json,
            "{}"
        );
    }

    #[test]
    fn checkpoint_holds_provider_generation_guard_until_store_commit() {
        let (store, input, handle, generation) = checkpoint_fixture();
        let store_guard = store.lock();
        std::thread::scope(|scope| {
            let store = &store;
            let input = &input;
            let handle = &handle;
            let (serialized_tx, serialized_rx) = std::sync::mpsc::channel();
            let checkpoint = scope.spawn(move || {
                save_checkpoint(
                    store,
                    input,
                    registered_checks()[0].policy(),
                    &reusable_progress("guarded"),
                    &mut |_| {
                        serialized_tx.send(()).unwrap();
                        Ok("{\"page\":1}".into())
                    },
                    handle,
                    generation,
                )
            });
            serialized_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while handle.system_one.try_write().is_ok() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "checkpoint did not acquire generation guard"
                );
                std::thread::yield_now();
            }
            let (changed_tx, changed_rx) = std::sync::mpsc::channel();
            let provider_change = scope.spawn(move || {
                handle
                    .set_system_one_connection(
                        SystemOneConnection::jev_default(),
                        Some("replacement-key".into()),
                    )
                    .unwrap();
                changed_tx.send(()).unwrap();
            });
            assert!(matches!(
                changed_rx.recv_timeout(Duration::from_millis(50)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ));
            drop(store_guard);
            checkpoint.join().unwrap().unwrap();
            provider_change.join().unwrap();
            changed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        assert_eq!(store.burn_check_work_answers(&input).unwrap().len(), 1);
        assert_eq!(
            store
                .burn_check_assessment(&input.key, &input.check_id)
                .unwrap()
                .unwrap()
                .progress_json,
            "{\"page\":1}"
        );
        assert!(!handle.key_is_current(generation));
    }

    #[test]
    fn production_registry_has_one_descriptor_per_check() {
        let checks = registered_checks();
        let ids = checks
            .iter()
            .map(|check| check.id())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), checks.len());
        assert_eq!(
            ids,
            std::collections::BTreeSet::from([
                "ignored_instructions",
                "skill_opportunities",
                "over_exploring",
                "scope_creep"
            ])
        );
        assert_eq!(
            checks[0].evaluator_revision(),
            crate::ignored_instructions_worker::CHECK.evaluator_revision()
        );
        assert_eq!(checks[0].policy().idle_secs, 180);
    }

    #[tokio::test]
    async fn runtime_discovery_refreshes_digest_limits_and_invalidates_on_credentials() {
        use antiburn_local::analysis::jev::capabilities::CapabilitySource;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let connection = SystemOneConnection {
            provider: SystemOneProvider::Ollama,
            endpoint: SystemOneEndpoint::BaseUrl(format!(
                "http://{}",
                listener.local_addr().unwrap()
            )),
            model: "clef".into(),
            credential: None,
            model_revision: None,
            ..SystemOneConnection::default()
        };
        let server = tokio::spawn(async move {
            for (digest, context) in [
                ("digest-one", 8192),
                ("digest-two", 16384),
                ("digest-three", 32768),
            ] {
                for (route, body) in [
                    ("/api/version", serde_json::json!({"version": "0.35.0"})),
                    (
                        "/api/tags",
                        serde_json::json!({"models": [
                            {"name": "clef:custom", "digest": "custom-digest"},
                            {"name": "clef:latest", "digest": digest}
                        ]}),
                    ),
                    (
                        "/api/show",
                        serde_json::json!({"capabilities": ["decision"], "model_info": {
                            "general.architecture": "clef", "clef.context_length": 65536
                        }}),
                    ),
                    (
                        "/api/ps",
                        serde_json::json!({"models": [
                            {"name": "clef:custom", "digest": "custom-digest", "context_length": 4096},
                            {"name": "clef:latest", "digest": digest, "context_length": context}
                        ]}),
                    ),
                ] {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(String::from_utf8_lossy(&buffer[..count]).contains(route));
                    let body = body.to_string();
                    socket.write_all(format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body,
                    ).as_bytes()).await.unwrap();
                }
            }
        });
        let handle = WorkerHandle::default();
        handle
            .set_system_one_connection(connection.clone(), None)
            .unwrap();
        let generation = handle.key_generation.load(Ordering::Acquire);
        let first = handle.resolve_capabilities(generation).await.unwrap();
        assert_eq!(first.model_revision.as_deref(), Some("digest-one"));
        assert_eq!(first.runtime_context_tokens.value, Some(8192));
        assert_eq!(
            first.runtime_context_tokens.source,
            CapabilitySource::RuntimeMetadata
        );
        assert_eq!(first.request_body_bytes.value, Some(65536));
        assert_eq!(
            handle.resolve_capabilities(generation).await.unwrap(),
            first
        );

        handle.discovery.lock().await.as_mut().unwrap().expires = tokio::time::Instant::now();
        let refreshed = handle.resolve_capabilities(generation).await.unwrap();
        assert_eq!(refreshed.model_revision.as_deref(), Some("digest-two"));
        assert_eq!(refreshed.runtime_context_tokens.value, Some(16384));
        let input = BurnCheckInput {
            key: SessionKey::new("native", "opencode", "synthetic-session"),
            check_id: "ignored_instructions".into(),
            input_revision: "same-input".into(),
            evaluator_revision: "same-evaluator".into(),
            incarnation: 1,
            source_generation: 1,
            source_fingerprint: None,
            published_fence: 1,
            activity_cursor: "same-cursor".into(),
            boundary_at_epoch: 1,
        };
        let first_connection = SystemOneConnection {
            model_revision: first.model_revision.clone(),
            ..connection.clone()
        };
        let refreshed_connection = SystemOneConnection {
            model_revision: refreshed.model_revision.clone(),
            ..connection.clone()
        };
        assert_ne!(
            compatible_request_identity(&first_connection, &input, "same-request"),
            compatible_request_identity(&refreshed_connection, &input, "same-request"),
        );
        handle
            .set_system_one_connection(connection.clone(), Some("replacement-secret".into()))
            .unwrap();
        assert_eq!(
            handle.resolve_capabilities(generation).await,
            Err(JevError::Cancelled)
        );
        let generation = handle.key_generation.load(Ordering::Acquire);
        let changed = handle.resolve_capabilities(generation).await.unwrap();
        assert_eq!(changed.model_revision.as_deref(), Some("digest-three"));
        assert_eq!(changed.runtime_context_tokens.value, Some(32768));
        server.await.unwrap();

        handle.discovery.lock().await.as_mut().unwrap().expires = tokio::time::Instant::now();
        let fallback = handle.resolve_capabilities(generation).await.unwrap();
        assert_eq!(fallback, connection.capabilities().unwrap());
        assert_eq!(
            fallback.runtime_context_tokens.source,
            CapabilitySource::DocumentedDefault
        );
        assert_eq!(
            handle.resolve_capabilities(generation).await.unwrap(),
            fallback
        );
    }

    #[tokio::test]
    async fn provider_change_during_dispatch_cancels_without_cache_or_publication() {
        use antiburn_local::analysis::jev::{JevQuestion, JevRequest};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        for return_response in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let connection = SystemOneConnection {
                provider: SystemOneProvider::Custom,
                endpoint: SystemOneEndpoint::ExactUrl(format!("http://{address}/infer")),
                model: "clef-flash".into(),
                model_revision: None,
                response_mode: crate::jev::config::SystemOneResponseMode::Direct,
                credential: None,
                revision: 1,
                context_override: Some(crate::jev::config::ContextLimitOverride {
                    total_input_tokens: Some(32_768),
                    state_and_longest_question_tokens: Some(16_384),
                    runtime_context_tokens: Some(32_768),
                }),
            };
            let next_connection = SystemOneConnection {
                provider: SystemOneProvider::Ollama,
                endpoint: SystemOneEndpoint::BaseUrl(format!("http://{address}")),
                revision: 2,
                ..connection.clone()
            };
            let handle = WorkerHandle::default();
            handle
                .set_system_one_connection(connection.clone(), None)
                .unwrap();
            let generation = handle.key_generation.load(Ordering::Acquire);
            let store = Store::open_in_memory(std::path::Path::new("synthetic-state")).unwrap();
            let now = unix_now();
            let record = crate::store::SessionRecord {
                key: SessionKey::new("native", "claude-code", "provider-change"),
                source_kind: "file".into(),
                source_label: "synthetic-session.jsonl".into(),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: None,
                surface: "cli".into(),
                updated_at_epoch: Some(now - 600),
                activity_cursor: "cursor".into(),
                activity_source: "mtime".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: None,
            };
            store
                .upsert_sessions(
                    std::slice::from_ref(&record),
                    &crate::agents::evidence_cohort(),
                )
                .unwrap();
            store
                .lock()
                .execute(
                    "UPDATE session_evidence SET status = 'ready', analyzed_generation = 0,
                 parser_revision = ?1, analyzer_revision = ?2, evidence_schema_revision = ?3,
                 evidence_json = '{}', claim_fence = 1, published_fence = 1",
                    rusqlite::params![
                        antiburn_local::analysis::PARSER_REVISION,
                        antiburn_local::analysis::ANALYZER_REVISION,
                        antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION
                    ],
                )
                .unwrap();
            store.set_internal_value(
                "internal:burnChecksEnabledAtEpochV1",
                &(now - 1000).to_string(),
            );
            let input = BurnCheckInput {
                key: record.key,
                check_id: "ignored_instructions".into(),
                input_revision: "input-v1".into(),
                evaluator_revision: "eval-v1".into(),
                incarnation: 1,
                source_generation: 0,
                source_fingerprint: None,
                published_fence: 1,
                activity_cursor: "cursor".into(),
                boundary_at_epoch: now - 1000,
            };
            assert!(
                store
                    .queue_burn_check_assessment(&input, now, IDLE_SECS)
                    .unwrap()
            );
            assert!(
                store
                    .claim_burn_check_assessment(&input, now, 300, IDLE_SECS)
                    .unwrap()
            );
            let policy = registered_checks()[0].policy();
            save_checkpoint(
                &store,
                &input,
                policy,
                &JevRunProgress::default(),
                &mut |_| Ok("{\"page\":1}".into()),
                &handle,
                generation,
            )
            .unwrap();
            assert_eq!(
                store
                    .burn_check_assessment(&input.key, &input.check_id)
                    .unwrap()
                    .unwrap()
                    .progress_json,
                "{\"page\":1}"
            );
            let stale_input = BurnCheckInput {
                published_fence: input.published_fence + 1,
                ..input.clone()
            };
            assert!(matches!(
                save_checkpoint(
                    &store,
                    &stale_input,
                    policy,
                    &JevRunProgress::default(),
                    &mut |_| Ok("{}".into()),
                    &handle,
                    generation,
                ),
                Err(JevError::Cancelled)
            ));
            assert_eq!(
                store
                    .burn_check_assessment(&input.key, &input.check_id)
                    .unwrap()
                    .unwrap()
                    .progress_json,
                "{\"page\":1}"
            );
            let request = JevRequest {
                model: connection.model.clone(),
                state: serde_json::json!({"activity": "synthetic work"}),
                questions: std::collections::BTreeMap::from([(
                    "q".into(),
                    JevQuestion::Noul {
                        instructions: serde_json::json!("Does this activity follow the task?"),
                        criteria: None,
                    },
                )]),
            };
            let batch = std::sync::Arc::new(JevRequestBatch {
                id: "batch".into(),
                serialized_bytes: serde_json::to_vec(&request).unwrap().len(),
                request,
                work_item_ids: vec!["work".into()],
                work_item_digests: std::collections::BTreeMap::from([(
                    "work".into(),
                    "semantic-work".into(),
                )]),
                answer_owners: std::collections::BTreeMap::from([(
                    "q".into(),
                    ("work".into(), "q".into()),
                )]),
                evidence_owners: Default::default(),
                digest: "request-digest".into(),
            });
            let response = serde_json::json!({
                "model": "clef-flash", "answers": {"q": {"type": "noul", "noul": 0.99}},
                "usage": {"input_tokens": 321, "output_tokens": 45}
            })
            .to_string();
            let server = async {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request_bytes = Vec::new();
                let mut buffer = [0; 4096];
                while !request_bytes.ends_with(&serde_json::to_vec(&batch.request).unwrap()) {
                    let size = stream.read(&mut buffer).await.unwrap();
                    assert!(size > 0);
                    request_bytes.extend_from_slice(&buffer[..size]);
                }
                handle
                    .set_system_one_connection(next_connection.clone(), None)
                    .unwrap();
                if return_response {
                    stream.write_all(format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        response.len(), response,
                    ).as_bytes()).await.unwrap();
                } else {
                    assert_eq!(stream.read(&mut buffer).await.unwrap(), 0);
                }
            };
            let notifications = std::sync::atomic::AtomicUsize::new(0);
            let notify = || {
                notifications.fetch_add(1, Ordering::SeqCst);
            };
            let events = SessionEvents::default();
            let run = async {
                let result = execute_batch(
                    BatchContext {
                        store: &store,
                        input: &input,
                        client: TypeSafeClient::new("synthetic-unused".into()).unwrap(),
                        handle: &handle,
                        key_generation: generation,
                        events: &events,
                        idle_secs: IDLE_SECS,
                        lease_secs: 300,
                        capabilities: &connection.capabilities().unwrap(),
                    },
                    batch.clone(),
                    &notify,
                )
                .await;
                if result.is_ok() {
                    store
                        .complete_burn_check_assessment(&input, "{}", unix_now(), IDLE_SECS)
                        .unwrap();
                }
                result
            };
            let (result, ()) =
                tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(run, server) })
                    .await
                    .unwrap();
            assert_eq!(result, Err(JevError::Cancelled));
            assert_eq!(notifications.load(Ordering::SeqCst), 1);
            let usage = store.burn_check_usage_summary().unwrap();
            assert_eq!(usage.confirmed_calls, u64::from(return_response));
            assert_eq!(usage.unknown_outcomes, u64::from(!return_response));
            assert_eq!(usage.input_tokens, if return_response { 321 } else { 0 });
            assert_eq!(usage.output_tokens, if return_response { 45 } else { 0 });
            let reservation: String = store
                .lock()
                .query_row("SELECT id FROM burn_check_usage_reservation", [], |row| {
                    row.get(0)
                })
                .unwrap();
            store
                .settle_burn_check_usage(&reservation, None, unix_now())
                .unwrap();
            assert_eq!(store.burn_check_usage_summary().unwrap(), usage);
            for selected in [&connection, &next_connection] {
                assert!(
                    store
                        .cached_assessment_response(
                            provider_id(selected.provider),
                            &compatible_request_identity(selected, &input, &batch.digest),
                            unix_now(),
                        )
                        .unwrap()
                        .is_none()
                );
            }
            let assessment = store
                .burn_check_assessment(&input.key, &input.check_id)
                .unwrap()
                .unwrap();
            assert_eq!(assessment.status, "running");
            assert_eq!(assessment.request_count, 1);
            assert!(assessment.result_json.is_none());
            assert!(assessment.result_revision.is_none());
            assert_eq!(assessment.progress_json, "{\"page\":1}");

            let next_server = async {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request_bytes = Vec::new();
                let mut buffer = [0; 4096];
                while !request_bytes.ends_with(&serde_json::to_vec(&batch.request).unwrap()) {
                    let size = stream.read(&mut buffer).await.unwrap();
                    assert!(size > 0);
                    request_bytes.extend_from_slice(&buffer[..size]);
                }
                assert!(request_bytes.starts_with(b"POST /v1/systemone "));
                stream.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(), response,
                ).as_bytes()).await.unwrap();
            };
            let next_capabilities = next_connection.capabilities().unwrap();
            let next_run = execute_batch(
                BatchContext {
                    store: &store,
                    input: &input,
                    client: TypeSafeClient::new("synthetic-unused".into()).unwrap(),
                    handle: &handle,
                    key_generation: handle.key_generation.load(Ordering::Acquire),
                    events: &events,
                    idle_secs: IDLE_SECS,
                    lease_secs: 300,
                    capabilities: &next_capabilities,
                },
                batch.clone(),
                &notify,
            );
            let (next_result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(next_run, next_server)
            })
            .await
            .unwrap();
            assert!(
                next_result.is_ok(),
                "the new provider can assess the same work"
            );
            let next_usage = store.burn_check_usage_summary().unwrap();
            assert_eq!(next_usage.confirmed_calls, usage.confirmed_calls + 1);
            assert_eq!(next_usage.unknown_outcomes, usage.unknown_outcomes);
            assert_eq!(next_usage.input_tokens, usage.input_tokens + 321);
            assert!(
                store
                    .cached_assessment_response(
                        provider_id(next_connection.provider),
                        &compatible_request_identity(&next_connection, &input, &batch.digest),
                        unix_now(),
                    )
                    .unwrap()
                    .is_some()
            );
        }
    }

    #[test]
    fn cache_reuse_does_not_treat_evaluated_usage_as_context_capacity() {
        use antiburn_local::analysis::jev::{JevAnswer, JevQuestion, JevRequest, JevUsage};
        let response = JevResponse {
            model: "clef-flash".into(),
            answers: std::collections::BTreeMap::from([(
                "q".into(),
                JevAnswer::Noul { noul: 0.9 },
            )]),
            usage: JevUsage {
                input_tokens: MAX_REQUEST_TOKENS * 4,
                output_tokens: 45,
            },
        };
        let batch = JevRequestBatch {
            id: "batch".into(),
            request: JevRequest {
                model: response.model.clone(),
                state: serde_json::json!({"activity": "synthetic work"}),
                questions: std::collections::BTreeMap::from([(
                    "q".into(),
                    JevQuestion::Noul {
                        instructions: serde_json::json!("Does this activity follow the task?"),
                        criteria: None,
                    },
                )]),
            },
            work_item_ids: vec!["work".into()],
            work_item_digests: Default::default(),
            answer_owners: Default::default(),
            evidence_owners: Default::default(),
            digest: "request-digest".into(),
            serialized_bytes: 0,
        };
        let cached = CachedAssessmentResponse {
            provider: "ollama-systemone".into(),
            request_digest: batch.digest.clone(),
            returned_model: response.model.clone(),
            response_json: serde_json::to_string(&response).unwrap(),
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
            created_at_epoch: unix_now(),
        };
        let reused = decode_cached_response(cached, &batch).unwrap();
        assert_eq!(reused.answers, response.answers);
        assert_eq!(
            reused.usage,
            JevUsage {
                input_tokens: 0,
                output_tokens: 0
            }
        );
    }

    #[test]
    fn registered_checks_share_candidate_turns_without_waiting_for_history_drain() {
        let mut scheduled = [
            (
                "first",
                std::collections::VecDeque::from(["recent", "history-1", "history-2"]),
            ),
            (
                "second",
                std::collections::VecDeque::from(["recent", "history-1"]),
            ),
        ];
        let mut next = 0;
        let order =
            std::iter::from_fn(|| next_candidate(&mut scheduled, &mut next)).collect::<Vec<_>>();
        assert_eq!(
            order,
            [
                ("first", "recent"),
                ("second", "recent"),
                ("first", "history-1"),
                ("second", "history-1"),
                ("first", "history-2")
            ]
        );
        assert!(next_candidate::<&str, &str>(&mut [], &mut next).is_none());
    }

    #[tokio::test]
    async fn four_mock_descriptors_checkpoint_fairly_and_resume_after_cancelled_wake() {
        trait MockDispatch: JevCheckDescriptor {
            fn dispatch<'a>(
                &'a self,
                store: &'a Store,
                input: &'a BurnCheckInput,
                handle: &'a WorkerHandle,
                generation: u64,
                page: u32,
            ) -> WorkerFuture<'a>;
            fn completed(&self) -> u32;
        }
        struct MockDescriptor {
            id: &'static str,
            completed: std::sync::Mutex<u32>,
        }
        impl JevCheckDescriptor for MockDescriptor {
            fn id(&self) -> &'static str {
                self.id
            }
            fn evaluator_revision(&self) -> String {
                "eval-v1".into()
            }
            fn policy(&self) -> CheckPolicy {
                CheckPolicy {
                    idle_secs: 180,
                    lease_secs: 300,
                    retry_delay_secs: 1800,
                }
            }
            fn run_candidate<'a>(&'a self, execution: CandidateExecution<'a>) -> WorkerFuture<'a> {
                Box::pin(async move {
                    let input = BurnCheckInput {
                        key: execution.candidate.session.key.clone(),
                        check_id: self.id.into(),
                        input_revision: "input-v1".into(),
                        evaluator_revision: self.evaluator_revision(),
                        incarnation: execution.candidate.incarnation,
                        source_generation: execution.candidate.source_generation,
                        source_fingerprint: execution.candidate.source_fingerprint.clone(),
                        published_fence: execution.candidate.published_fence,
                        activity_cursor: execution.candidate.session.activity_cursor.clone(),
                        boundary_at_epoch: 0,
                    };
                    self.dispatch(
                        execution.store,
                        &input,
                        execution.handle,
                        execution.key_generation,
                        self.completed() + 1,
                    )
                    .await
                })
            }
        }
        impl MockDispatch for MockDescriptor {
            fn dispatch<'a>(
                &'a self,
                store: &'a Store,
                input: &'a BurnCheckInput,
                handle: &'a WorkerHandle,
                generation: u64,
                page: u32,
            ) -> WorkerFuture<'a> {
                Box::pin(async move {
                    save_checkpoint(
                        store,
                        input,
                        self.policy(),
                        &reusable_progress(&format!("page-{page}")),
                        &mut |_| Ok(format!("{{\"page\":{page}}}")),
                        handle,
                        generation,
                    )?;
                    *self.completed.lock().unwrap() = page;
                    Ok(())
                })
            }
            fn completed(&self) -> u32 {
                *self.completed.lock().unwrap()
            }
        }
        let mocks = [
            "ignored_instructions",
            "skill_opportunities",
            "over_exploring",
            "scope_creep",
        ]
        .map(|id| MockDescriptor {
            id,
            completed: std::sync::Mutex::new(0),
        });
        let (store, base_input, handle, generation) = checkpoint_fixture();
        store
            .lock()
            .execute("DELETE FROM burn_check_assessment", [])
            .unwrap();
        for detector in registered_check_ids() {
            store.set_check_enabled(detector, true).unwrap();
        }
        for check in &mocks {
            let input = BurnCheckInput {
                check_id: check.id().into(),
                ..base_input.clone()
            };
            assert!(
                store
                    .queue_burn_check_assessment(&input, unix_now(), check.policy().idle_secs)
                    .unwrap()
            );
            assert!(
                store
                    .claim_burn_check_assessment(
                        &input,
                        unix_now(),
                        check.policy().lease_secs,
                        check.policy().idle_secs
                    )
                    .unwrap()
            );
        }
        let mut scheduled = mocks
            .iter()
            .map(|check| {
                (
                    check as &dyn MockDispatch,
                    std::collections::VecDeque::from([1, 2]),
                )
            })
            .collect::<Vec<_>>();
        let mut next = 0;
        let mut completed = Vec::new();
        for _ in 0..6 {
            let (check, page) = next_candidate(&mut scheduled, &mut next).unwrap();
            let input = BurnCheckInput {
                check_id: check.id().into(),
                ..base_input.clone()
            };
            check
                .dispatch(&store, &input, &handle, generation, page)
                .await
                .unwrap();
            completed.push((check.id(), page));
        }
        assert_eq!(
            completed,
            [
                ("ignored_instructions", 1),
                ("skill_opportunities", 1),
                ("over_exploring", 1),
                ("scope_creep", 1),
                ("ignored_instructions", 2),
                ("skill_opportunities", 2)
            ]
        );
        handle
            .set_system_one_connection(
                SystemOneConnection::jev_default(),
                Some("next-wake-key".into()),
            )
            .unwrap();
        let (check, page) = next_candidate(&mut scheduled, &mut next).unwrap();
        let input = BurnCheckInput {
            check_id: check.id().into(),
            ..base_input.clone()
        };
        assert!(
            check
                .dispatch(&store, &input, &handle, generation, page)
                .await
                .is_err()
        );
        assert_eq!(check.completed(), 1);
        assert_eq!(store.burn_check_work_answers(&input).unwrap().len(), 1);
        assert_eq!(
            store
                .burn_check_assessment(&input.key, check.id())
                .unwrap()
                .unwrap()
                .progress_json,
            "{\"page\":1}"
        );
        let generation = handle.key_generation.load(Ordering::Acquire);
        let mut resumed = mocks
            .iter()
            .map(|check| {
                (
                    check as &dyn MockDispatch,
                    ((check.completed() + 1)..=2).collect::<std::collections::VecDeque<_>>(),
                )
            })
            .collect::<Vec<_>>();
        let mut next = 0;
        while let Some((check, page)) = next_candidate(&mut resumed, &mut next) {
            let input = BurnCheckInput {
                check_id: check.id().into(),
                ..base_input.clone()
            };
            check
                .dispatch(&store, &input, &handle, generation, page)
                .await
                .unwrap();
            completed.push((check.id(), page));
        }
        assert_eq!(&completed[6..], [("over_exploring", 2), ("scope_creep", 2)]);
        for check in &mocks {
            let input = BurnCheckInput {
                check_id: check.id().into(),
                ..base_input.clone()
            };
            assert_eq!(check.completed(), 2);
            assert_eq!(store.burn_check_work_answers(&input).unwrap().len(), 2);
            assert_eq!(
                store
                    .burn_check_assessment(&input.key, check.id())
                    .unwrap()
                    .unwrap()
                    .progress_json,
                "{\"page\":2}"
            );
        }
        assert!(next_candidate(&mut resumed, &mut next).is_none());
    }

    #[test]
    fn provider_pacing_reserves_tokens_then_uses_measured_usage_and_shared_backoff() {
        let now = tokio::time::Instant::now();
        let mut pacing = ProviderPacing::new(now);
        assert!(pacing.admit("first", 30_000, now));
        assert!(pacing.admit("second", 30_000, now + Duration::from_millis(100)));
        assert!(pacing.admit("third", 30_000, now + Duration::from_millis(200)));
        assert!(!pacing.admit("fourth", 30_000, now + Duration::from_millis(300)));
        pacing.settle("first", 10_000);
        pacing.settle("second", 0);
        pacing.settle("third", 0);
        assert!(pacing.admit("fourth", 20_000, now + Duration::from_millis(300)));
        assert_eq!(
            pacing.starts.iter().map(|start| start.tokens).sum::<u64>(),
            30_000
        );
        pacing.next_start = now + Duration::from_secs(12);
        assert!(!pacing.admit("fifth", 30_000, now + Duration::from_secs(1)));
        assert!(pacing.admit("fifth", 30_000, now + Duration::from_secs(12)));
        assert_eq!(pacing.starts.len(), 1);
        println!(
            "provider pacing estimated request tokens allows bounded concurrency within rolling limit"
        );
    }

    #[test]
    fn provider_pacing_buckets_do_not_share_admission_state() {
        let now = tokio::time::Instant::now();
        let pacing = ProviderPacingSet::new(now);
        let mut jev = pacing.jev.lock().unwrap();
        let mut custom = pacing.custom.lock().unwrap();
        assert!(jev.admit("jev", GLOBAL_INPUT_TOKENS_PER_SECOND, now));
        assert!(!jev.admit("jev-next", 1, now));
        assert!(custom.admit("custom", 1, now));
    }

    #[test]
    fn stale_success_records_measured_usage_once_without_cache() {
        let store = Store::open_in_memory(std::path::Path::new("synthetic-state")).unwrap();
        store.set_internal_value("internal:burnCheckUsageLedgerV1", &serde_json::json!({
            "reservations": [{"id": "stale-reservation", "session_key": "synthetic-session",
                "provider": "typesafe-systemone", "check_id": "synthetic-check", "model": "jev-1.13.0",
                "input_tokens": 65536, "expires_at_epoch": unix_now() + 86400,
                "settled": false, "unknown_recorded": false}], "summary": {}
        }).to_string());
        let response = JevResponse {
            model: "jev-1.13.0".to_owned(),
            answers: Default::default(),
            usage: antiburn_local::analysis::jev::JevUsage {
                input_tokens: 321,
                output_tokens: 45,
            },
        };
        let handle = WorkerHandle::default();
        let old_generation = handle.key_generation.load(Ordering::Acquire);
        let next_connection = SystemOneConnection {
            provider: SystemOneProvider::Custom,
            endpoint: SystemOneEndpoint::ExactUrl("https://proxy.example/infer".into()),
            model: "other-model".into(),
            model_revision: None,
            response_mode: crate::jev::config::SystemOneResponseMode::Direct,
            credential: None,
            revision: 2,
            context_override: Some(crate::jev::config::ContextLimitOverride {
                total_input_tokens: Some(8192),
                ..crate::jev::config::ContextLimitOverride::default()
            }),
        };
        handle
            .set_system_one_connection(next_connection, None)
            .unwrap();
        let outcome = match record_response_if_current(
            &handle,
            old_generation,
            &store,
            "stale-reservation",
            "typesafe-systemone",
            "stale-digest",
            &response,
        ) {
            None => {
                settle_stale_response(&store, "stale-reservation", &response).unwrap();
                Err(JevError::Cancelled)
            }
            Some(Ok(())) => Ok(()),
            Some(Err(error)) => Err(error),
        };
        assert_eq!(outcome, Err(JevError::Cancelled));
        for _ in 0..2 {
            settle_stale_response(&store, "stale-reservation", &response).unwrap();
        }
        let usage = store.burn_check_usage_summary().unwrap();
        assert_eq!(usage.confirmed_calls, 1);
        assert_eq!(usage.input_tokens, 321);
        assert_eq!(usage.output_tokens, 45);
        assert!(
            store
                .cached_assessment_response("typesafe-systemone", "stale-digest", unix_now())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unknown_delivery_never_gets_an_automatic_retry() {
        for attempt in 0..4 {
            assert_eq!(retry_delay(&JevError::RequestOutcomeUnknown, attempt), None);
        }
    }

    #[test]
    fn append_revision_keeps_transport_identity_but_changed_context_reopens_it() {
        let (_, input, _, _) = checkpoint_fixture();
        let connection = SystemOneConnection::jev_default();
        let original = compatible_request_identity(&connection, &input, "context-digest");
        let appended = BurnCheckInput {
            input_revision: "appended-source".into(),
            source_fingerprint: Some("changed-source-fingerprint".into()),
            ..input.clone()
        };
        assert_eq!(
            original,
            compatible_request_identity(&connection, &appended, "context-digest")
        );
        assert_ne!(
            original,
            compatible_request_identity(&connection, &appended, "changed-context-digest")
        );
        let evaluator = BurnCheckInput {
            evaluator_revision: "new-evaluator".into(),
            ..appended
        };
        assert_ne!(
            original,
            compatible_request_identity(&connection, &evaluator, "context-digest")
        );
    }

    #[test]
    fn dropping_dispatched_work_settles_unknown_usage_once_and_preserves_resume_block() {
        let store = Store::open_in_memory(std::path::Path::new("synthetic-state")).unwrap();
        store.set_internal_value("internal:burnCheckUsageLedgerV1", &serde_json::json!({
            "reservations": [{"id": "reservation", "session_key": "synthetic-session",
                "provider": "typesafe-systemone", "check_id": "synthetic-check", "model": "jev-1.13.0",
                "input_tokens": 65536, "expires_at_epoch": unix_now() + 86400,
                "settled": false, "unknown_recorded": false}], "summary": {}
        }).to_string());
        assert!(
            store
                .track_burn_check_request("work-identity", "reservation", unix_now())
                .unwrap()
        );
        let notifications = std::sync::atomic::AtomicUsize::new(0);
        let notify = || {
            notifications.fetch_add(1, Ordering::SeqCst);
        };
        for _ in 0..2 {
            drop(DispatchGuard {
                store: &store,
                notify: &notify,
                reservation_id: "reservation".to_owned(),
                provider: SystemOneProvider::Jev,
                settled: false,
                rejected: false,
            });
        }
        assert_eq!(
            store.burn_check_usage_summary().unwrap().unknown_outcomes,
            1
        );
        assert!(
            store
                .burn_check_requests_are_unresolved(&["work-identity".to_owned()])
                .unwrap()
        );
        assert_eq!(notifications.load(Ordering::SeqCst), 2);
        drop(DispatchGuard {
            store: &store,
            notify: &notify,
            reservation_id: "reservation".to_owned(),
            provider: SystemOneProvider::Jev,
            settled: false,
            rejected: true,
        });
        assert!(
            !store
                .burn_check_requests_are_unresolved(&["work-identity".to_owned()])
                .unwrap()
        );
        assert_eq!(
            store.burn_check_usage_summary().unwrap().unknown_outcomes,
            1
        );
    }

    #[tokio::test]
    async fn global_request_bytes_are_shared_and_released_after_cancellation() {
        let handle = WorkerHandle::default();
        handle
            .set_system_one_connection(
                SystemOneConnection::jev_default(),
                Some("synthetic-key".to_owned()),
            )
            .unwrap();
        let generation = handle.key_generation.load(Ordering::Acquire);
        let events = SessionEvents::default();
        let key = SessionKey::new("native", "claude", "synthetic-session");
        let bytes = request_bytes();
        assert!(std::ptr::eq(bytes, request_bytes()));
        let held = bytes
            .acquire_many(GLOBAL_REQUEST_BYTES as u32)
            .await
            .unwrap();
        assert_eq!(request_bytes().available_permits(), 0);
        let revoke = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            handle.suspend_system_one();
        };
        let (outcome, ()) = tokio::join!(
            acquire_budget(
                request_bytes().acquire_many(1),
                &handle,
                generation,
                &events,
                &key
            ),
            revoke
        );
        assert!(matches!(outcome, Err(JevError::Cancelled)));
        drop(held);
        let restored = request_bytes()
            .acquire_many(GLOBAL_REQUEST_BYTES as u32)
            .await
            .unwrap();
        assert_eq!(request_bytes().available_permits(), 0);
        drop(restored);
        assert!(std::ptr::eq(request_slots(), request_slots()));
        let slots = request_slots()
            .acquire_many(antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS as u32)
            .await
            .unwrap();
        assert_eq!(request_slots().available_permits(), 0);
        drop(slots);
    }

    #[tokio::test]
    async fn byte_admission_wait_stops_on_credential_revocation() {
        let semaphore = tokio::sync::Semaphore::new(512);
        let held = semaphore.acquire_many(512).await.unwrap();
        let handle = WorkerHandle::default();
        handle
            .set_system_one_connection(
                SystemOneConnection::jev_default(),
                Some("synthetic-key".to_owned()),
            )
            .unwrap();
        let generation = handle.key_generation.load(Ordering::Acquire);
        let events = SessionEvents::default();
        let key = SessionKey::new("native", "claude", "synthetic-session");
        let revoke = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            handle.suspend_system_one();
        };
        let (outcome, ()) = tokio::join!(
            acquire_budget(
                semaphore.acquire_many(1),
                &handle,
                generation,
                &events,
                &key
            ),
            revoke
        );
        assert!(matches!(outcome, Err(JevError::Cancelled)));
        drop(held);
        assert_eq!(semaphore.available_permits(), 512);
    }

    #[test]
    fn provider_retry_delay_uses_retry_after_and_bounded_backoff() {
        assert_eq!(
            retry_delay(
                &JevError::RateLimited {
                    retry_after: Some(Duration::from_secs(12)),
                },
                0,
            ),
            Some(Duration::from_secs(12))
        );
        assert_eq!(
            retry_delay(&JevError::ProviderOverloaded { retry_after: None }, 1),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            retry_delay(
                &JevError::RateLimited {
                    retry_after: Some(Duration::from_secs(300)),
                },
                0,
            ),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            retry_delay(&JevError::ProviderUnavailable, 0),
            Some(Duration::from_secs(5))
        );
        assert_eq!(retry_delay(&JevError::ProviderUnavailable, 2), None);
    }

    #[test]
    fn connection_and_credential_changes_advance_generation_and_fence_old_work() {
        let handle = WorkerHandle::default();
        let initial = handle.key_generation.load(Ordering::Acquire);
        let connection = crate::jev::config::SystemOneConnection {
            provider: crate::jev::config::SystemOneProvider::Custom,
            endpoint: crate::jev::config::SystemOneEndpoint::ExactUrl(
                "https://proxy.example/v1/systemone".to_owned(),
            ),
            model: "proxy-model".to_owned(),
            model_revision: None,
            response_mode: crate::jev::config::SystemOneResponseMode::Direct,
            credential: None,
            revision: 2,
            context_override: Some(crate::jev::config::ContextLimitOverride {
                total_input_tokens: Some(8192),
                ..crate::jev::config::ContextLimitOverride::default()
            }),
        };
        handle
            .set_system_one_connection(connection.clone(), Some("secret-one".to_owned()))
            .unwrap();
        let selected = handle.key_generation.load(Ordering::Acquire);
        assert_eq!(selected, initial + 1);
        assert_eq!(handle.system_one_connection(), connection);

        let mut revised = connection;
        revised.revision += 1;
        handle
            .set_system_one_connection(revised, Some("secret-two".to_owned()))
            .unwrap();
        let current = handle.key_generation.load(Ordering::Acquire);
        assert_eq!(current, selected + 1);
        assert!(handle.with_current_generation(selected, || ()).is_none());
        assert!(handle.with_current_generation(current, || ()).is_some());
    }

    #[test]
    fn keyless_provider_connections_are_available_and_current() {
        let handle = WorkerHandle::default();
        let connection = crate::jev::config::SystemOneConnection {
            provider: SystemOneProvider::Ollama,
            endpoint: SystemOneEndpoint::BaseUrl("http://127.0.0.1:11434".to_owned()),
            model: "clef-flash".to_owned(),
            model_revision: None,
            response_mode: crate::jev::config::SystemOneResponseMode::Direct,
            credential: None,
            revision: 2,
            context_override: None,
        };
        handle.set_system_one_connection(connection, None).unwrap();
        let generation = handle.key_generation.load(Ordering::Acquire);

        assert!(handle.is_available());
        assert!(handle.key_is_current(generation));
        assert!(handle.execution_client().is_some());
    }

    #[test]
    fn suspending_keyless_providers_fences_dispatch_and_publication_until_explicit_resume() {
        for provider in [SystemOneProvider::Ollama, SystemOneProvider::Custom] {
            let handle = WorkerHandle::default();
            let connection = SystemOneConnection {
                provider,
                endpoint: match provider {
                    SystemOneProvider::Ollama => {
                        SystemOneEndpoint::BaseUrl("http://127.0.0.1:11434".into())
                    }
                    _ => SystemOneEndpoint::ExactUrl("https://proxy.example/systemone".into()),
                },
                model: "clef-flash".into(),
                credential: None,
                context_override: Some(crate::jev::config::ContextLimitOverride {
                    total_input_tokens: Some(8192),
                    ..crate::jev::config::ContextLimitOverride::default()
                }),
                ..SystemOneConnection::default()
            };
            handle
                .set_system_one_connection(connection.clone(), None)
                .unwrap();
            let (_, generation) = handle.execution_client().unwrap();
            handle.suspend_system_one();
            let paused = handle.key_generation.load(Ordering::Acquire);
            assert!(!handle.is_available());
            assert!(handle.execution_client().is_none());
            assert!(!handle.key_is_current(generation));
            assert!(!handle.key_is_current(paused));
            assert!(
                handle
                    .with_current_generation(generation, || panic!("stale publication"))
                    .is_none()
            );
            assert!(
                handle
                    .with_current_generation(paused, || panic!("paused publication"))
                    .is_none()
            );
            handle
                .install_system_one_connection(connection.clone(), None, false)
                .unwrap();
            assert!(!handle.is_available());
            handle.set_system_one_connection(connection, None).unwrap();
            let (_, resumed) = handle.execution_client().unwrap();
            assert!(handle.is_available());
            assert!(handle.key_is_current(resumed));
            assert!(!handle.key_is_current(generation));
            assert!(!handle.key_is_current(paused));
        }
    }

    #[test]
    fn compatible_request_identity_separates_endpoints_and_never_contains_credentials() {
        let input = crate::store::BurnCheckInput {
            key: SessionKey::new("native", "claude", "synthetic-session"),
            check_id: "ignored_instructions".into(),
            input_revision: "input-v1".into(),
            evaluator_revision: "eval-v1".into(),
            incarnation: 1,
            source_generation: 1,
            source_fingerprint: None,
            published_fence: 1,
            activity_cursor: "cursor-v1".into(),
            boundary_at_epoch: 1,
        };
        let first = SystemOneConnection {
            provider: SystemOneProvider::Custom,
            endpoint: SystemOneEndpoint::ExactUrl(
                "https://proxy.example/infer?tenant=one&token=secret-one".into(),
            ),
            model: "same-model".into(),
            model_revision: None,
            response_mode: crate::jev::config::SystemOneResponseMode::Direct,
            credential: None,
            revision: 1,
            context_override: Some(crate::jev::config::ContextLimitOverride {
                total_input_tokens: Some(8192),
                ..crate::jev::config::ContextLimitOverride::default()
            }),
        };
        let second = SystemOneConnection {
            endpoint: SystemOneEndpoint::ExactUrl(
                "https://other.example/infer?tenant=two&token=secret-two".into(),
            ),
            ..first.clone()
        };
        let first_identity = compatible_request_identity(&first, &input, "same-request");
        let second_identity = compatible_request_identity(&second, &input, "same-request");
        assert_ne!(first_identity, second_identity);
        assert!(!first_identity.contains("proxy.example"));
        assert!(!first_identity.contains("secret-one"));
        assert!(!second_identity.contains("secret-two"));
        assert_eq!(
            first_identity,
            compatible_request_identity(&first, &input, "same-request")
        );
        let credential_profile = SystemOneConnection {
            credential: Some(crate::jev::config::CredentialReference::Connection(
                "profile-one".to_owned(),
            )),
            ..first.clone()
        };
        let credential_identity =
            compatible_request_identity(&credential_profile, &input, "same-request");
        assert_ne!(first_identity, credential_identity);
        assert!(!credential_identity.contains("profile-one"));
        assert_ne!(
            first_identity,
            compatible_request_identity(&first, &input, "changed-request")
        );
        let revised_model = SystemOneConnection {
            model_revision: Some("sha256:model-digest".into()),
            ..first.clone()
        };
        assert_ne!(
            first_identity,
            compatible_request_identity(&revised_model, &input, "same-request")
        );
        let revised_check = BurnCheckInput {
            evaluator_revision: "eval-v2".into(),
            ..input.clone()
        };
        assert_ne!(
            first_identity,
            compatible_request_identity(&first, &revised_check, "same-request")
        );
    }
}
