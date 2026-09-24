//! Runs enabled remote Burn Check assessments outside report reads.

use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use antiburn_local::analysis::jev::{
    JevError, JevExecutionOutcome, JevRequestBatch, JevResponse, JevRunProgress, MAX_REQUEST_BYTES,
    run_jev_check,
};
use antiburn_local::analysis::{SessionEvidence, SourceFormat, ignored_instructions};
use antiburn_local::platform::git;
use tauri::Manager;
use tokio::sync::Notify;

use crate::jev_client::TypeSafeClient;
use crate::session_lifecycle::{SessionEvents, SessionRef};
use crate::store::{
    BurnCheckAssessment, BurnCheckCandidate, BurnCheckFailure, BurnCheckInput,
    BurnCheckReservation, CachedAssessmentResponse, SessionKey, Store,
};

const CHECK_ID: &str = "ignored_instructions";
const PROVIDER_ID: &str = "typesafe-systemone";
const IDLE_SECS: i64 = 180;
const LEASE_SECS: i64 = 300;
const RETRY_DELAY_SECS: i64 = 30 * 60;
const POLL_SECS: u64 = 60;
const CANDIDATES_PER_WAKE: usize = 16;
const RETRY_ATTEMPTS: usize = 3;

/// Shared settings adapter for the generic assessment worker.
#[derive(Default)]
pub(crate) struct WorkerHandle {
    wake: Notify,
    api_key: RwLock<Option<String>>,
    key_generation: AtomicU64,
}

impl WorkerHandle {
    /// Supply a key loaded from the native credential store by the Settings layer.
    pub fn set_api_key(&self, api_key: Option<String>) {
        let mut current = self
            .api_key
            .write()
            .unwrap_or_else(|error| error.into_inner());
        *current = api_key;
        self.key_generation.fetch_add(1, Ordering::AcqRel);
        drop(current);
        self.wake.notify_one();
    }

    /// Capture source cursors before the Settings layer enables paid checks.
    fn client(&self) -> Option<(TypeSafeClient, u64)> {
        let current = self
            .api_key
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let key = current.clone()?;
        let generation = self.key_generation.load(Ordering::Acquire);
        TypeSafeClient::new(key)
            .ok()
            .map(|client| (client, generation))
    }

    fn key_is_current(&self, generation: u64) -> bool {
        let current = self
            .api_key
            .read()
            .unwrap_or_else(|error| error.into_inner());
        self.key_generation.load(Ordering::Acquire) == generation && current.is_some()
    }
}

pub(crate) fn wake(app: &tauri::AppHandle) {
    app.state::<WorkerHandle>().wake.notify_one();
}

pub(crate) fn spawn(app: &tauri::AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
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
            let candidates = match store.burn_check_candidates(
                CHECK_ID,
                unix_now(),
                IDLE_SECS,
                CANDIDATES_PER_WAKE,
            ) {
                Ok(candidates) => candidates,
                Err(error) => {
                    ::tracing::warn!(event = "burn_check_candidates_failed", error = %error);
                    continue;
                }
            };
            for candidate in candidates {
                let result = run_candidate(
                    &store,
                    &candidate,
                    client.clone(),
                    &handle,
                    key_generation,
                    &events,
                )
                .await;
                if let Err(error) = result {
                    ::tracing::warn!(
                        event = "burn_check_assessment_failed",
                        check_id = CHECK_ID,
                        agent = %candidate.session.key.agent,
                        error = %error,
                    );
                }
            }
        }
    })
}

async fn run_candidate(
    store: &Store,
    candidate: &BurnCheckCandidate,
    client: TypeSafeClient,
    handle: &WorkerHandle,
    key_generation: u64,
    events: &SessionEvents,
) -> anyhow::Result<()> {
    let Some(input) = prepare_input(store, candidate).await? else {
        return Ok(());
    };
    if !store.queue_burn_check_assessment(&input, unix_now(), IDLE_SECS)? {
        return Ok(());
    }
    if !store.claim_burn_check_assessment(&input, unix_now(), LEASE_SECS, IDLE_SECS)? {
        store.supersede_burn_check_assessment(&input, unix_now())?;
        return Ok(());
    }
    let stored: Option<BurnCheckAssessment> = store.burn_check_assessment(&input.key, CHECK_ID)?;
    let mut progress = stored
        .and_then(|assessment| {
            serde_json::from_str::<JevRunProgress>(&assessment.progress_json).ok()
        })
        .unwrap_or_default();
    if progress.input_revision != input.input_revision {
        progress = JevRunProgress {
            input_revision: input.input_revision.clone(),
            ..JevRunProgress::default()
        };
    }
    let context = ignored_instructions::build_jev_context(&input.assessment_input)?;
    let check = ignored_instructions::IgnoredInstructionsCheck;
    let execute_store = store.clone();
    let execute_input = input.clone();
    let execute_client = client.clone();
    let execute_handle = handle;
    let execute_events = events;
    let execute = move |batch: JevRequestBatch| {
        let store = execute_store.clone();
        let input = execute_input.clone();
        let client = execute_client.clone();
        async move {
            execute_batch(
                &store,
                &input,
                batch,
                client,
                execute_handle,
                key_generation,
                execute_events,
            )
            .await
        }
    };
    let save_store = store.clone();
    let save_input = input.clone();
    let outcome = run_jev_check(&check, &context, progress, execute, move |progress| {
        let serialized =
            serde_json::to_string(progress).map_err(|_| JevError::RequestSerialization)?;
        if !save_store
            .save_burn_check_progress(&save_input, &serialized, unix_now(), LEASE_SECS, IDLE_SECS)
            .map_err(|_| JevError::ProviderUnavailable)?
        {
            return Err(JevError::Cancelled);
        }
        Ok(())
    })
    .await?;
    save_outcome(store, &input, outcome).await
}

#[derive(Clone)]
struct PreparedInput {
    assessment_input: ignored_instructions::AssessmentInput,
    input: BurnCheckInput,
}

impl std::ops::Deref for PreparedInput {
    type Target = BurnCheckInput;

    fn deref(&self) -> &Self::Target {
        &self.input
    }
}

async fn prepare_input(
    store: &Store,
    candidate: &BurnCheckCandidate,
) -> anyhow::Result<Option<PreparedInput>> {
    let Some(evidence_row) = store.evidence(&candidate.session.key)? else {
        return Ok(None);
    };
    let Some(evidence_json) = evidence_row.evidence_json else {
        return Ok(None);
    };
    let evidence: SessionEvidence = match serde_json::from_str(&evidence_json) {
        Ok(evidence) => evidence,
        Err(_) => return Ok(None),
    };
    let format = evidence.capabilities.source_format;
    let Some(published) = store.published_turn_content(&candidate.session.key)? else {
        return Ok(None);
    };
    if published.source_generation != Some(candidate.source_generation)
        || published.publication_fence != candidate.published_fence
    {
        return Ok(None);
    }
    let discovery = discover_instructions(&candidate.session, format).await;
    let mut content = ignored_instructions::prepare_session_content(
        &format!(
            "{}\0{}\0{}",
            candidate.session.key.environment_key,
            candidate.session.key.agent,
            candidate.session.key.session_id
        ),
        format,
        published,
        discovery.snapshots,
    );
    content.limitations.extend(discovery.limitations);
    if !discovery.scan_complete {
        content
            .limitations
            .push("instruction_scan_incomplete".to_owned());
    }
    content.limitations.sort();
    content.limitations.dedup();
    content.complete &= content.limitations.is_empty();
    let digest_material = format!(
        "{}\0{}",
        content.selected_input_digest,
        content.limitations.join("\0")
    );
    content.selected_input_digest = ignored_instructions::sha256_hex(digest_material.as_bytes());
    let assessment_input = ignored_instructions::AssessmentInput {
        content,
        activity_after_ms: Some(candidate.boundary_at_epoch.saturating_mul(1000)),
        source_generation: candidate.source_generation,
        source_fingerprint: candidate.source_fingerprint.clone(),
        incarnation: candidate.incarnation,
    };
    let context = ignored_instructions::build_jev_context(&assessment_input)?;
    let evaluator_revision = format!(
        "{}:{}:{}:{}",
        ignored_instructions::ASSESSMENT_MODEL,
        ignored_instructions::ASSESSMENT_PREPARATION_REVISION,
        ignored_instructions::ASSESSMENT_QUESTION_REVISION,
        ignored_instructions::ASSESSMENT_REDUCER_REVISION
    );
    Ok(Some(PreparedInput {
        input: BurnCheckInput {
            key: candidate.session.key.clone(),
            check_id: CHECK_ID.to_owned(),
            incarnation: candidate.incarnation,
            source_generation: candidate.source_generation,
            source_fingerprint: candidate.source_fingerprint.clone(),
            activity_cursor: candidate.activity_cursor.clone(),
            published_fence: candidate.published_fence,
            input_revision: context.input_revision,
            evaluator_revision,
            boundary_at_epoch: candidate.boundary_at_epoch,
        },
        assessment_input,
    }))
}

async fn discover_instructions(
    session: &crate::store::SessionRecord,
    format: SourceFormat,
) -> ignored_instructions::InstructionDiscovery {
    let Some(cwd) = session.cwd.as_deref().map(Path::new) else {
        return ignored_instructions::InstructionDiscovery {
            limitations: vec!["session_working_directory_unavailable".to_owned()],
            ..Default::default()
        };
    };
    let root = match git::repo_root_at(cwd).await {
        Ok(root) => git::canonical_main_repo_root(&root).await,
        Err(_) => cwd.to_path_buf(),
    };
    let home = home_directory().unwrap_or_else(|| PathBuf::from("."));
    ignored_instructions::discover_current_instructions(
        crate::agents::kind_from_slug(&session.key.agent)
            .map(crate::agents::vendor_label)
            .unwrap_or(&session.key.agent),
        format,
        &root,
        cwd,
        &home,
    )
    .await
}

async fn execute_batch(
    store: &Store,
    input: &BurnCheckInput,
    batch: JevRequestBatch,
    client: TypeSafeClient,
    handle: &WorkerHandle,
    key_generation: u64,
    events: &SessionEvents,
) -> Result<JevResponse, JevError> {
    if let Some(cached) = store
        .cached_assessment_response(PROVIDER_ID, &batch.digest, unix_now())
        .map_err(|_| JevError::ProviderUnavailable)?
    {
        return decode_cached_response(cached, &batch);
    }
    for attempt in 0..RETRY_ATTEMPTS {
        if !handle.key_is_current(key_generation) || session_is_active(events, &input.key) {
            return Err(JevError::Cancelled);
        }
        if !store
            .renew_burn_check_assessment(input, unix_now(), LEASE_SECS, IDLE_SECS)
            .map_err(|_| JevError::ProviderUnavailable)?
        {
            return Err(JevError::Cancelled);
        }
        let reservation = store
            .reserve_burn_check_usage(input, MAX_REQUEST_BYTES as u64, unix_now(), IDLE_SECS)
            .map_err(|_| JevError::ProviderUnavailable)?;
        let BurnCheckReservation::Reserved(reservation_id) = reservation else {
            return Err(match reservation {
                BurnCheckReservation::UsageLimitReached => JevError::UsageLimitReached,
                BurnCheckReservation::RequestLimitReached => JevError::RequestLimitReached,
                BurnCheckReservation::Stale => JevError::Cancelled,
                BurnCheckReservation::Reserved(_) => unreachable!(),
            });
        };
        let request = batch.request.clone();
        let client = client.clone();
        let mut call = tauri::async_runtime::spawn_blocking(move || client.evaluate(&request));
        let result = loop {
            tokio::select! {
                result = &mut call => {
                    break result.map_err(|_| JevError::RequestOutcomeUnknown)?;
                }
                () = tokio::time::sleep(Duration::from_millis(250)) => {
                    if !handle.key_is_current(key_generation) || session_is_active(events, &input.key) {
                        drop(call);
                        store.settle_burn_check_usage(&reservation_id, None, unix_now())
                            .map_err(|_| JevError::ProviderUnavailable)?;
                        return Err(JevError::Cancelled);
                    }
                }
            }
        };
        match result {
            Ok(response) => {
                store
                    .settle_burn_check_usage(
                        &reservation_id,
                        Some(response.usage.input_tokens),
                        unix_now(),
                    )
                    .map_err(|_| JevError::ProviderUnavailable)?;
                let response_json =
                    serde_json::to_string(&response).map_err(|_| JevError::ResponseDecode)?;
                store
                    .record_burn_check_response(
                        &reservation_id,
                        CachedAssessmentResponse {
                            provider: PROVIDER_ID.to_owned(),
                            request_digest: batch.digest,
                            returned_model: response.model.clone(),
                            response_json,
                            input_tokens: response.usage.input_tokens,
                            output_tokens: response.usage.output_tokens,
                            created_at_epoch: unix_now(),
                        },
                    )
                    .map_err(|_| JevError::ProviderUnavailable)?;
                if response.usage.input_tokens > MAX_REQUEST_BYTES as u64 {
                    return Err(JevError::ResponseUsageExceeded);
                }
                return Ok(response);
            }
            Err(error) if attempt + 1 < RETRY_ATTEMPTS && retry_delay(&error).is_some() => {
                let delay = retry_delay(&error).unwrap_or(Duration::from_secs(1));
                tokio::time::sleep(delay.min(Duration::from_secs(10))).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(JevError::ProviderUnavailable)
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
    if cached.input_tokens > MAX_REQUEST_BYTES as u64 {
        return Err(JevError::ResponseUsageExceeded);
    }
    response.usage = antiburn_local::analysis::jev::JevUsage {
        input_tokens: 0,
        output_tokens: 0,
    };
    Ok(response)
}

fn retry_delay(error: &JevError) -> Option<Duration> {
    match error {
        JevError::RateLimited { retry_after } | JevError::ProviderOverloaded { retry_after } => {
            Some(retry_after.unwrap_or(Duration::from_secs(1)))
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

async fn save_outcome(
    store: &Store,
    input: &BurnCheckInput,
    outcome: JevExecutionOutcome<ignored_instructions::AssessmentResult>,
) -> anyhow::Result<()> {
    let serialized = serde_json::to_string(&outcome.result)?;
    let progress_json = serde_json::to_string(&outcome.progress)?;
    if let Some(error) = outcome.failure {
        let retry_at = match error {
            JevError::RequestOutcomeUnknown => Some(unix_now().saturating_add(24 * 60 * 60)),
            JevError::RateLimited { .. } | JevError::ProviderOverloaded { .. } => {
                Some(unix_now().saturating_add(RETRY_DELAY_SECS))
            }
            JevError::ProviderUnavailable => Some(unix_now().saturating_add(RETRY_DELAY_SECS)),
            JevError::AuthenticationRejected | JevError::InvalidRequestSchema => {
                Some(unix_now().saturating_add(24 * 60 * 60))
            }
            JevError::UsageLimitReached => Some(unix_now().saturating_add(24 * 60 * 60)),
            _ => None,
        };
        store.fail_burn_check_assessment_with_result(
            input,
            &BurnCheckFailure {
                error_category: error_category(&error),
                result_json: &serialized,
                progress_json: &progress_json,
                retry_at_epoch: retry_at,
            },
            unix_now(),
            IDLE_SECS,
        )?;
    } else {
        store.complete_burn_check_assessment(input, &serialized, unix_now(), IDLE_SECS)?;
    }
    Ok(())
}

fn error_category(error: &JevError) -> &'static str {
    match error {
        JevError::AuthenticationRejected => "authentication_rejected",
        JevError::InvalidRequestSchema => "invalid_request_schema",
        JevError::RateLimited { .. } => "rate_limited",
        JevError::ProviderOverloaded { .. } => "provider_overloaded",
        JevError::ProviderUnavailable => "provider_unavailable",
        JevError::RequestOutcomeUnknown => "outcome_unknown",
        JevError::UsageLimitReached => "usage_limit",
        JevError::RequestLimitReached => "request_limit",
        JevError::Cancelled => "cancelled",
        _ => "assessment_failed",
    }
}

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
