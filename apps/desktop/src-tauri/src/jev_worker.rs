//! Shared scheduler and TypeSafe executor for Jev-powered Burn Checks.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use antiburn_local::analysis::jev::{JevError, JevRequestBatch, JevResponse, MAX_REQUEST_TOKENS};
use antiburn_local::checks::DetectorId;
use sha2::{Digest, Sha256};
use tauri::Manager;
use tokio::sync::Notify;

use crate::jev_client::TypeSafeClient;
use crate::session_lifecycle::{SessionEvents, SessionRef};
use crate::store::{
    BurnCheckCandidate, BurnCheckHistoryCheck, BurnCheckInput, BurnCheckRequestAdmission,
    BurnCheckReservation, CachedAssessmentResponse, SessionKey, Store,
};

const PROVIDER_ID: &str = "typesafe-systemone";
const IDLE_SECS: i64 = 180;
const RETRY_ATTEMPTS: usize = 3;
const POLL_SECS: u64 = 60;
const CANDIDATES_PER_WAKE: usize = 16;
const GLOBAL_REQUEST_BYTES: usize = 4 * 1024 * 1024;

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
        self.next_start = now + Duration::from_millis(25);
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

fn provider_pacing() -> &'static std::sync::Mutex<ProviderPacing> {
    static PACING: std::sync::OnceLock<std::sync::Mutex<ProviderPacing>> =
        std::sync::OnceLock::new();
    PACING.get_or_init(|| std::sync::Mutex::new(ProviderPacing::new(tokio::time::Instant::now())))
}

pub(crate) type WorkerFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

pub(crate) struct CandidateExecution<'a> {
    pub(crate) app: &'a tauri::AppHandle,
    pub(crate) store: &'a Store,
    pub(crate) candidate: &'a BurnCheckCandidate,
    pub(crate) client: TypeSafeClient,
    pub(crate) handle: &'a WorkerHandle,
    pub(crate) key_generation: u64,
    pub(crate) check_generation: u64,
    pub(crate) events: &'a SessionEvents,
}

/// Supplies check policy to the shared scheduler and transport.
pub(crate) trait JevCheckWorker: Send + Sync {
    fn detector(&self) -> DetectorId;

    fn id(&self) -> &'static str {
        self.detector().key()
    }

    fn evaluator_revision(&self) -> String;

    fn supports_history(&self) -> bool {
        false
    }

    fn run_candidate<'a>(&'a self, execution: CandidateExecution<'a>) -> WorkerFuture<'a>;
}

pub(crate) fn registered_checks() -> [&'static dyn JevCheckWorker; 1] {
    [&crate::ignored_instructions_worker::CHECK]
}

pub(crate) fn registered_check_ids() -> Vec<DetectorId> {
    registered_checks()
        .into_iter()
        .map(JevCheckWorker::detector)
        .collect()
}

pub(crate) fn registered_history_checks() -> Vec<BurnCheckHistoryCheck> {
    registered_checks()
        .into_iter()
        .filter(|check| check.supports_history())
        .map(|check| BurnCheckHistoryCheck {
            check_id: check.id().to_owned(),
            evaluator_revision: check.evaluator_revision(),
        })
        .collect()
}

/// Shared credential and wake state for Jev workers.
#[derive(Default)]
pub(crate) struct WorkerHandle {
    wake: Notify,
    request_admission: Mutex<()>,
    api_key: RwLock<Option<String>>,
    key_generation: AtomicU64,
    authentication_rejected: AtomicBool,
    check_generations: Mutex<std::collections::BTreeMap<String, u64>>,
}

impl WorkerHandle {
    /// Supply a key loaded from the native credential store.
    pub(crate) fn set_api_key(&self, api_key: Option<String>) {
        let _admission = self
            .request_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut current = self
            .api_key
            .write()
            .unwrap_or_else(|error| error.into_inner());
        *current = api_key;
        self.authentication_rejected.store(false, Ordering::Release);
        self.key_generation.fetch_add(1, Ordering::AcqRel);
        drop(current);
        self.wake.notify_one();
    }

    pub(crate) fn client(&self) -> Option<(TypeSafeClient, u64)> {
        let current = self
            .api_key
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let key = current.clone()?;
        if self.authentication_rejected() {
            return None;
        }
        let generation = self.key_generation.load(Ordering::Acquire);
        TypeSafeClient::new(key)
            .ok()
            .map(|client| (client, generation))
    }

    pub(crate) fn key_is_current(&self, generation: u64) -> bool {
        let current = self
            .api_key
            .read()
            .unwrap_or_else(|error| error.into_inner());
        self.key_generation.load(Ordering::Acquire) == generation
            && current.is_some()
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

    pub(crate) fn advance_check_generation(&self, check_id: &str) -> u64 {
        let _admission = self
            .request_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut generations = self
            .check_generations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let generation = generations.entry(check_id.to_owned()).or_default();
        *generation = generation.saturating_add(1);
        self.wake.notify_one();
        *generation
    }

    pub(crate) fn check_is_current(&self, check_id: &str, generation: u64) -> bool {
        self.check_generation(check_id) == generation
    }

    fn admit_if_current<T>(
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
        self.api_key
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }

    pub(crate) fn authentication_rejected(&self) -> bool {
        self.authentication_rejected.load(Ordering::Acquire)
    }

    pub(crate) fn reject_authentication(&self) {
        let _admission = self
            .request_admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.authentication_rejected.store(true, Ordering::Release);
        self.wake.notify_one();
    }
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
        loop {
            let handle = app.state::<WorkerHandle>();
            tokio::select! {
                () = handle.wake.notified() => {},
                _ = poll.tick() => {},
                event = lifecycle.recv() => {
                    match event {
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {},
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
            let Some((client, key_generation)) = handle.client() else {
                continue;
            };
            let store = (*app.state::<Store>()).clone();
            let events = app.state::<SessionEvents>();
            let mut scheduled = Vec::new();
            for check in checks {
                if !handle.key_is_current(key_generation) {
                    break;
                }
                let candidates = match store.burn_check_candidates_for_revision(
                    check.id(),
                    &check.evaluator_revision(),
                    unix_now(),
                    IDLE_SECS,
                    CANDIDATES_PER_WAKE,
                ) {
                    Ok(candidates) => candidates,
                    Err(error) => {
                        ::tracing::warn!(
                            event = "burn_check_candidates_failed",
                            check_id = check.id(),
                            error = %error
                        );
                        continue;
                    }
                };
                scheduled.push((
                    check,
                    handle.check_generation(check.id()),
                    std::collections::VecDeque::from(candidates),
                ));
            }
            let mut next_check = 0;
            while let Some(((check, check_generation), candidate)) =
                next_candidate(&mut scheduled, &mut next_check)
            {
                if !handle.key_is_current(key_generation) {
                    break;
                }
                if !handle.check_is_current(check.id(), check_generation) {
                    continue;
                }
                let result = check
                    .run_candidate(CandidateExecution {
                        app: &app,
                        store: &store,
                        candidate: &candidate,
                        client: client.clone(),
                        handle: &handle,
                        key_generation,
                        check_generation,
                        events: &events,
                    })
                    .await;
                if candidate.historical {
                    crate::jev_settings::progress_changed(&app);
                }
                if let Err(error) = result {
                    ::tracing::warn!(
                        event = "burn_check_assessment_failed",
                        check_id = check.id(),
                        agent = %candidate.session.key.agent,
                        error = %error,
                    );
                }
            }
        }
    })
}

fn next_candidate<T: Copy, C>(
    scheduled: &mut [(T, u64, std::collections::VecDeque<C>)],
    next: &mut usize,
) -> Option<((T, u64), C)> {
    for _ in 0..scheduled.len() {
        let index = *next % scheduled.len();
        *next = (index + 1) % scheduled.len();
        let (check, generation, candidates) = &mut scheduled[index];
        if let Some(candidate) = candidates.pop_front() {
            return Some(((*check, *generation), candidate));
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
    pub(crate) check_generation: u64,
    pub(crate) events: &'a SessionEvents,
    pub(crate) idle_secs: i64,
    pub(crate) lease_secs: i64,
}

#[derive(Clone, Copy)]
struct ExecutionFence<'a> {
    handle: &'a WorkerHandle,
    key_generation: u64,
    check_id: &'a str,
    check_generation: u64,
}

impl ExecutionFence<'_> {
    fn is_current(self) -> bool {
        self.handle.key_is_current(self.key_generation)
            && self
                .handle
                .check_is_current(self.check_id, self.check_generation)
    }
}

struct DispatchGuard<'a> {
    store: &'a Store,
    notify: &'a (dyn Fn() + Sync),
    reservation_id: String,
    settled: bool,
    rejected: bool,
}

impl Drop for DispatchGuard<'_> {
    fn drop(&mut self) {
        if !self.settled {
            let settlement = if self.rejected {
                provider_pacing()
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
        check_generation,
        events,
        idle_secs,
        lease_secs,
    } = execution;
    let fence = ExecutionFence {
        handle,
        key_generation,
        check_id: &input.check_id,
        check_generation,
    };
    let resident_bytes = antiburn_local::analysis::jev::jev_batch_resident_bytes(&batch)?;
    if !fence.is_current()
        || session_is_active(events, &input.key)
        || !store
            .renew_burn_check_assessment(input, unix_now(), lease_secs, idle_secs)
            .map_err(|_| JevError::ProviderUnavailable)?
    {
        return Err(JevError::Cancelled);
    }
    if let Some(cached) = store
        .cached_assessment_response(PROVIDER_ID, &batch.digest, unix_now())
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
        let cache_hit_id = request_cache_identity(input, &batch.digest);
        store
            .record_burn_check_cache_hit(
                input,
                PROVIDER_ID,
                &batch.request.model,
                &input.check_id,
                &cache_hit_id,
            )
            .map_err(|_| JevError::ProviderUnavailable)?;
        crate::jev_settings::changed(app);
        return Ok(response);
    }
    for attempt in 0..RETRY_ATTEMPTS {
        let request_identities = batch
            .work_item_digests
            .values()
            .map(|digest| {
                hash_identity([
                    input.key.environment_key.as_str(),
                    input.key.agent.as_str(),
                    input.key.session_id.as_str(),
                    &input.incarnation.to_string(),
                    input.check_id.as_str(),
                    digest.as_str(),
                ])
            })
            .collect::<Vec<_>>();
        if store
            .burn_check_requests_are_unresolved(&request_identities)
            .map_err(|_| JevError::ProviderUnavailable)?
        {
            return Err(JevError::RequestOutcomeUnknown);
        }
        let _slot = acquire_budget(request_slots().acquire(), fence, events, &input.key).await?;
        let _bytes = acquire_budget(
            request_bytes().acquire_many(
                u32::try_from(resident_bytes).map_err(|_| JevError::RequestSerialization)?,
            ),
            fence,
            events,
            &input.key,
        )
        .await?;
        let started = std::time::Instant::now();
        if !fence.is_current() || session_is_active(events, &input.key) {
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
                PROVIDER_ID,
                &batch.request.model,
                MAX_REQUEST_TOKENS,
                unix_now(),
                idle_secs,
            )
            .map_err(|_| JevError::ProviderUnavailable)?;
        let BurnCheckReservation::Reserved(reservation_id) = reservation else {
            return Err(JevError::Cancelled);
        };
        let notify = || crate::jev_settings::changed(app);
        let mut dispatch = DispatchGuard {
            store,
            notify: &notify,
            reservation_id: reservation_id.clone(),
            settled: false,
            rejected: true,
        };
        let estimated_tokens = u64::try_from(batch.serialized_bytes.div_ceil(3))
            .unwrap_or(MAX_REQUEST_TOKENS)
            .min(MAX_REQUEST_TOKENS);
        wait_for_provider(&reservation_id, estimated_tokens, fence, events, &input.key).await?;
        if !fence.is_current()
            || session_is_active(events, &input.key)
            || !store
                .renew_burn_check_assessment(input, unix_now(), lease_secs, idle_secs)
                .map_err(|_| JevError::ProviderUnavailable)?
        {
            return Err(JevError::Cancelled);
        }
        let Some(admission) = handle
            .admit_if_current(key_generation, &input.check_id, check_generation, || {
                store.admit_burn_check_requests(
                    input,
                    &request_identities,
                    &reservation_id,
                    unix_now(),
                )
            })
            .map_err(|_| JevError::ProviderUnavailable)?
        else {
            return Err(JevError::Cancelled);
        };
        match admission {
            BurnCheckRequestAdmission::Admitted => {}
            BurnCheckRequestAdmission::Stale => return Err(JevError::Cancelled),
            BurnCheckRequestAdmission::Unresolved => {
                return Err(JevError::RequestOutcomeUnknown);
            }
        }
        dispatch.rejected = false;
        let call = client.evaluate_async(&batch.request);
        tokio::pin!(call);
        let response = loop {
            tokio::select! {
                result = &mut call => {
                    break result;
                }
                () = tokio::time::sleep(Duration::from_millis(250)) => {
                    if !fence.is_current() || session_is_active(events, &input.key) {
                        store.settle_burn_check_usage(&reservation_id, None, unix_now())
                            .map_err(|_| JevError::ProviderUnavailable)?;
                        dispatch.settled = true;
                        crate::jev_settings::changed(app);
                        return Err(JevError::Cancelled);
                    }
                }
            }
        };
        dispatch.rejected = response.as_ref().err().is_some_and(request_was_rejected);
        if let Err(error) = &response
            && let Some(delay) = retry_delay(error, attempt)
        {
            let mut pacing = provider_pacing()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            pacing.next_start = pacing.next_start.max(tokio::time::Instant::now() + delay);
        }
        match response {
            Ok(response) => {
                provider_pacing()
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
                let response_json =
                    serde_json::to_string(&response).map_err(|_| JevError::ResponseDecode)?;
                if store
                    .record_burn_check_response(
                        &reservation_id,
                        CachedAssessmentResponse {
                            provider: PROVIDER_ID.to_owned(),
                            request_digest: batch.digest.clone(),
                            returned_model: response.model.clone(),
                            response_json,
                            input_tokens: response.usage.input_tokens,
                            output_tokens: response.usage.output_tokens,
                            created_at_epoch: unix_now(),
                        },
                    )
                    .is_err()
                {
                    store
                        .settle_burn_check_usage(
                            &reservation_id,
                            Some(response.usage.input_tokens),
                            unix_now(),
                        )
                        .map_err(|_| JevError::ProviderUnavailable)?;
                    dispatch.settled = true;
                    crate::jev_settings::changed(app);
                    return Err(JevError::ProviderUnavailable);
                }
                dispatch.settled = true;
                crate::jev_settings::changed(app);
                if response.usage.input_tokens > MAX_REQUEST_TOKENS {
                    return Err(JevError::ResponseUsageExceeded);
                }
                return Ok(response);
            }
            Err(error)
                if attempt + 1 < RETRY_ATTEMPTS && retry_delay(&error, attempt).is_some() =>
            {
                drop(_bytes);
                drop(_slot);
                ::tracing::debug!(
                    event = "typesafe_request_failed",
                    model = %batch.request.model,
                    attempt = attempt + 1,
                    request_bytes = batch.serialized_bytes,
                    question_count = batch.request.questions.len(),
                    work_item_count = batch.work_item_ids.len(),
                    failure_category = error_category(&error),
                    error_detail = ?error,
                    retry_delay_ms = retry_delay(&error, attempt).unwrap_or_default().as_millis(),
                    elapsed_ms = started.elapsed().as_millis(),
                );
                if request_was_rejected(&error) {
                    store
                        .release_rejected_burn_check_usage(&reservation_id)
                        .map_err(|_| JevError::ProviderUnavailable)?;
                    provider_pacing()
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .settle(&reservation_id, 0);
                } else {
                    store
                        .settle_burn_check_usage(&reservation_id, None, unix_now())
                        .map_err(|_| JevError::ProviderUnavailable)?;
                    if matches!(error, JevError::RequestOutcomeUnknown) {
                        store
                            .clear_burn_check_request_outcomes(&request_identities)
                            .map_err(|_| JevError::ProviderUnavailable)?;
                    }
                }
                dispatch.settled = true;
                crate::jev_settings::changed(app);
                if !wait_for_retry(
                    retry_delay(&error, attempt).unwrap_or_default(),
                    fence,
                    events,
                    &input.key,
                )
                .await
                {
                    return Err(JevError::Cancelled);
                }
            }
            Err(error) => {
                if request_was_rejected(&error) {
                    provider_pacing()
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
                    error_detail = ?error,
                    retry_delay_ms = 0,
                    elapsed_ms = started.elapsed().as_millis(),
                );
                crate::jev_settings::changed(app);
                return Err(error);
            }
        }
    }
    Err(JevError::ProviderUnavailable)
}

async fn wait_for_retry(
    delay: Duration,
    fence: ExecutionFence<'_>,
    events: &SessionEvents,
    key: &SessionKey,
) -> bool {
    let deadline = tokio::time::Instant::now() + delay;
    loop {
        if !fence.is_current() || session_is_active(events, key) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return true;
        }
        tokio::time::sleep(remaining.min(Duration::from_millis(250))).await;
    }
}

async fn wait_for_provider(
    reservation_id: &str,
    estimated_tokens: u64,
    fence: ExecutionFence<'_>,
    events: &SessionEvents,
    key: &SessionKey,
) -> Result<(), JevError> {
    loop {
        if !fence.is_current() || session_is_active(events, key) {
            return Err(JevError::Cancelled);
        }
        let admitted = provider_pacing()
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
    fence: ExecutionFence<'_>,
    events: &SessionEvents,
    key: &SessionKey,
) -> Result<T, JevError> {
    tokio::pin!(acquire);
    loop {
        if !fence.is_current() || session_is_active(events, key) {
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
    if cached.input_tokens > MAX_REQUEST_TOKENS {
        return Err(JevError::ResponseUsageExceeded);
    }
    response.usage = antiburn_local::analysis::jev::JevUsage {
        input_tokens: 0,
        output_tokens: 0,
    };
    Ok(response)
}

pub(crate) fn retry_delay(error: &JevError, attempt: usize) -> Option<Duration> {
    match error {
        JevError::RateLimited { retry_after } | JevError::ProviderOverloaded { retry_after } => {
            match retry_after {
                Some(delay) if *delay <= Duration::from_secs(240) => Some(*delay),
                Some(_) => None,
                None => Some(Duration::from_secs(1_u64 << attempt.min(3))),
            }
        }
        JevError::RequestOutcomeUnknown if attempt < RETRY_ATTEMPTS - 1 => {
            Some(Duration::from_secs(1_u64 << attempt.min(3)))
        }
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
        | JevError::RequestTooLarge { .. } => "invalid_request",
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
mod tests {
    use super::*;

    #[test]
    fn registered_checks_share_candidate_turns_without_waiting_for_history_drain() {
        let mut scheduled = [
            (
                "first",
                1,
                std::collections::VecDeque::from(["recent", "history-1", "history-2"]),
            ),
            (
                "second",
                2,
                std::collections::VecDeque::from(["recent", "history-1"]),
            ),
        ];
        let mut next = 0;
        let order =
            std::iter::from_fn(|| next_candidate(&mut scheduled, &mut next)).collect::<Vec<_>>();
        assert_eq!(
            order,
            [
                (("first", 1), "recent"),
                (("second", 2), "recent"),
                (("first", 1), "history-1"),
                (("second", 2), "history-1"),
                (("first", 1), "history-2")
            ]
        );
        assert!(next_candidate::<&str, &str>(&mut [], &mut next).is_none());
    }

    #[test]
    fn advancing_one_check_generation_does_not_cancel_another() {
        let handle = WorkerHandle::default();
        let first = handle.check_generation("first");
        let second = handle.check_generation("second");

        handle.advance_check_generation("first");

        assert!(!handle.check_is_current("first", first));
        assert!(handle.check_is_current("second", second));
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
    fn an_unknown_request_gets_two_bounded_retries() {
        assert_eq!(
            retry_delay(&JevError::RequestOutcomeUnknown, 0),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            retry_delay(&JevError::RequestOutcomeUnknown, 1),
            Some(Duration::from_secs(2))
        );
        assert_eq!(retry_delay(&JevError::RequestOutcomeUnknown, 2), None);
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
        handle.set_api_key(Some("synthetic-key".to_owned()));
        let generation = handle.key_generation.load(Ordering::Acquire);
        let check_generation = handle.check_generation("ignored_instructions");
        let fence = ExecutionFence {
            handle: &handle,
            key_generation: generation,
            check_id: "ignored_instructions",
            check_generation,
        };
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
            handle.set_api_key(None);
        };
        let (outcome, ()) = tokio::join!(
            acquire_budget(request_bytes().acquire_many(1), fence, &events, &key),
            revoke
        );
        assert!(matches!(outcome, Err(JevError::Cancelled)));
        drop(held);
        assert_eq!(request_bytes().available_permits(), GLOBAL_REQUEST_BYTES);
        assert!(std::ptr::eq(request_slots(), request_slots()));
        assert_eq!(
            request_slots().available_permits(),
            antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS
        );
    }

    #[tokio::test]
    async fn byte_admission_wait_stops_on_credential_revocation() {
        let semaphore = tokio::sync::Semaphore::new(512);
        let held = semaphore.acquire_many(512).await.unwrap();
        let handle = WorkerHandle::default();
        handle.set_api_key(Some("synthetic-key".to_owned()));
        let generation = handle.key_generation.load(Ordering::Acquire);
        let check_generation = handle.check_generation("ignored_instructions");
        let fence = ExecutionFence {
            handle: &handle,
            key_generation: generation,
            check_id: "ignored_instructions",
            check_generation,
        };
        let events = SessionEvents::default();
        let key = SessionKey::new("native", "claude", "synthetic-session");
        let revoke = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            handle.set_api_key(None);
        };
        let (outcome, ()) = tokio::join!(
            acquire_budget(semaphore.acquire_many(1), fence, &events, &key),
            revoke
        );
        assert!(matches!(outcome, Err(JevError::Cancelled)));
        drop(held);
        assert_eq!(semaphore.available_permits(), 512);
    }

    #[tokio::test]
    async fn retry_wait_stops_when_credentials_change() {
        let handle = WorkerHandle::default();
        handle.set_api_key(Some("synthetic-key".to_owned()));
        let generation = handle.key_generation.load(Ordering::Acquire);
        let check_generation = handle.check_generation("ignored_instructions");
        let fence = ExecutionFence {
            handle: &handle,
            key_generation: generation,
            check_id: "ignored_instructions",
            check_generation,
        };
        handle.set_api_key(None);
        let events = SessionEvents::default();

        assert!(
            !wait_for_retry(
                Duration::from_secs(30),
                fence,
                &events,
                &SessionKey::new("native", "claude", "synthetic-session"),
            )
            .await
        );
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
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            retry_delay(
                &JevError::RateLimited {
                    retry_after: Some(Duration::from_secs(300)),
                },
                0,
            ),
            None
        );
    }
}
