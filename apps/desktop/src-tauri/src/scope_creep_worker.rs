//! Scope Creep adapter for the single shared Smart Burn Check worker.
//! OpenCode SQLite v2 proves current retained root content, not original retention.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, Mutex};

use antiburn_local::analysis::SourceFormat;
use antiburn_local::analysis::jev::capabilities::ModelCapabilities;
use antiburn_local::analysis::jev::{
    JevCheck, JevCheckPlan, JevError, JevRunProgress, JevUsage, admit_jev_orchestration,
};
use antiburn_local::checks::sampling::{SamplingJob, SamplingLimits, SamplingProgress, StableId};
#[cfg(test)]
use antiburn_local::checks::scope_creep::ScopeQuestion;
use antiburn_local::checks::scope_creep::{
    DECISION_THRESHOLD, REVISIONS, ScopeAnswer, ScopeCreepCheck, ScopeCreepFinding,
    ScopeCreepPrepared, ScopeCreepResult, ScopeCreepStatus, ScopeDescriptorInventory,
};
use rusqlite::{OptionalExtension, params};
use tauri::Emitter;

use crate::jev::worker::{
    BatchExecution, CandidateExecution, CheckPolicy, JevCheckDescriptor, WorkerFuture,
    error_category, run_prepared_check, unix_now,
};
use crate::smart_check_inputs::{
    DetectorInput, InputLoadError, InputUnavailable, SmartCheckInputSnapshot,
};
use crate::store::{
    BurnCheckAssessment, BurnCheckCandidate, BurnCheckFailure, BurnCheckInput, SessionKey, Store,
};

pub(crate) const CHECK_ID: &str = "scope_creep";
const CURSOR_REVISION: u32 = 4;
const POLICY: CheckPolicy = CheckPolicy {
    idle_secs: 180,
    lease_secs: 300,
    retry_delay_secs: 300,
};

pub(crate) struct ScopeCreepDescriptor;
pub(crate) const CHECK: ScopeCreepDescriptor = ScopeCreepDescriptor;

struct CachedScopeCheck {
    key: String,
    check: Arc<ScopeCreepCheck>,
}

static SOURCE_CHECK: LazyLock<Mutex<Option<CachedScopeCheck>>> = LazyLock::new(|| Mutex::new(None));

impl JevCheckDescriptor for ScopeCreepDescriptor {
    fn id(&self) -> &'static str {
        CHECK_ID
    }
    fn evaluator_revision(&self) -> String {
        format!(
            "scope-creep-adapter-v{CURSOR_REVISION}:{}:{}:{}:{}",
            REVISIONS.projection, REVISIONS.chunking, REVISIONS.questions, REVISIONS.reducer
        )
    }
    fn policy(&self) -> CheckPolicy {
        POLICY
    }
    fn run_candidate<'a>(&'a self, execution: CandidateExecution<'a>) -> WorkerFuture<'a> {
        Box::pin(run_candidate(execution))
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct AssessmentCursor {
    revision: u32,
    input_revision: String,
    provider_generation: u64,
    sampling: Option<SamplingProgress>,
    active_job: Option<SamplingJob>,
    run_progress: JevRunProgress,
    result: Option<ScopeCreepResult>,
    run_finished: bool,
    run_started: bool,
    blocked_fit_key: Option<String>,
    accepted_request_usage: BTreeMap<String, JevUsage>,
    inventory: ScopeDescriptorInventory,
    prepared: Option<ScopeCreepPrepared>,
    context_epoch: Option<StableId>,
}

pub(crate) struct PreparedInput {
    pub(crate) durable: BurnCheckInput,
    pub(crate) check: Arc<ScopeCreepCheck>,
    pub(crate) plan: JevCheckPlan<ScopeCreepPrepared>,
    snapshot_revision: String,
    configuration_fence: String,
    fit_key: String,
    ignored_work: BTreeSet<String>,
    citations: BTreeMap<String, String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Publication {
    pub(crate) input_revision: String,
    snapshot_revision: String,
    configuration_fence: String,
    fit_key: String,
    ignored_work: BTreeSet<String>,
    capabilities: ModelCapabilities,
    prepared: ScopeCreepPrepared,
    citations: BTreeMap<String, String>,
    pub(crate) assessment: ScopeCreepResult,
}

pub(crate) struct SourceFence<'a> {
    pub(crate) key: &'a SessionKey,
    pub(crate) incarnation: u64,
    pub(crate) source_generation: i64,
    pub(crate) source_fingerprint: Option<&'a str>,
    pub(crate) published_fence: i64,
}

impl<'a> From<&'a BurnCheckCandidate> for SourceFence<'a> {
    fn from(candidate: &'a BurnCheckCandidate) -> Self {
        Self {
            key: &candidate.session.key,
            incarnation: candidate.incarnation,
            source_generation: candidate.source_generation,
            source_fingerprint: candidate.source_fingerprint.as_deref(),
            published_fence: candidate.published_fence,
        }
    }
}

pub(crate) fn source_supported(agent: &str, format: SourceFormat) -> bool {
    antiburn_local::analysis::smart_check_source_supported(agent, format)
}

fn identity(domain: &str, value: &impl serde::Serialize) -> Result<String, serde_json::Error> {
    Ok(StableId::new(domain, &[&serde_json::to_vec(value)?]).into())
}

// Read only the active profile. Unrelated saved profiles do not invalidate work.
fn configuration_fence(connection: &rusqlite::Connection) -> anyhow::Result<String> {
    let value = |key: &str| -> rusqlite::Result<Option<String>> {
        connection
            .query_row("SELECT value FROM setting WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
    };
    let active = value("internal:smartChecksActiveConnectionV1")?;
    let profiles = value("internal:smartChecksConnectionsV1")?
        .map(|json| serde_json::from_str::<serde_json::Value>(&json))
        .transpose()?;
    let profile = match (&active, &profiles) {
        (Some(id), Some(profiles)) => Some(
            profiles["profiles"]
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("active check profile is missing"))?,
        ),
        (None, None) => None,
        _ => anyhow::bail!("active check profile is incomplete"),
    };
    identity(
        "scope-configuration-v1",
        &(
            &active,
            profile,
            value("internal:burnChecksEnabledAtEpochV1")?,
            value("internal:smartChecksConnectionChangePendingV1")?,
            value("internal:typesafeCredentialChangePendingV1")?,
            value("internal:smartChecksCredentialRemovalPendingV1")?,
        ),
    )
    .map_err(Into::into)
}

fn ignored_instruction_work_ids(
    connection: &rusqlite::Connection,
    fence: &SourceFence<'_>,
) -> anyhow::Result<BTreeSet<String>> {
    use antiburn_local::analysis::ignored_instructions::{AssessmentResult, evaluator_revision};
    let stored = connection
        .query_row(
            "SELECT result_json, result_revision FROM burn_check_assessment
         WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
           AND check_id = 'ignored_instructions' AND incarnation = ?4
           AND source_generation = ?5 AND source_fingerprint IS ?6 AND published_fence = ?7
           AND evaluator_revision = ?8 AND input_revision = result_revision
           AND status IN ('completed', 'failed')",
            params![
                fence.key.environment_key,
                fence.key.agent,
                fence.key.session_id,
                fence.incarnation,
                fence.source_generation,
                fence.source_fingerprint,
                fence.published_fence,
                evaluator_revision()
            ],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()?;
    let Some((Some(json), Some(revision))) = stored else {
        return Ok(BTreeSet::new());
    };
    let Ok(result) = serde_json::from_str::<AssessmentResult>(&json) else {
        return Ok(BTreeSet::new());
    };
    if result.input_revision != revision {
        return Ok(BTreeSet::new());
    }
    Ok(result
        .findings
        .iter()
        .filter(|finding| {
            finding.decision_record().is_some_and(|decision| {
                decision.source_generation == fence.source_generation
                    && decision.source_fingerprint.as_deref() == fence.source_fingerprint
                    && decision.publication_fence == fence.published_fence
                    && decision.model == result.model_version
            })
        })
        .map(|finding| finding.reference.action_id.clone())
        .collect())
}

fn prepare_descriptor_input(
    candidate: &BurnCheckCandidate,
    snapshot: SmartCheckInputSnapshot,
    capabilities: &ModelCapabilities,
    ignored_work: BTreeSet<String>,
    configuration_fence: String,
) -> Result<PreparedInput, InputLoadError> {
    let snapshot = snapshot.for_candidate(candidate)?;
    if candidate.session.key.environment_key != "native"
        || !source_supported(
            &candidate.session.key.agent,
            snapshot.content().source_format,
        )
    {
        return Err(InputLoadError::Unavailable(
            InputUnavailable::IncompleteEvidence,
        ));
    }
    let cache_key = identity(
        "scope-source-check-v1",
        &(
            snapshot.input_revision(),
            (
                &candidate.session.key.environment_key,
                &candidate.session.key.agent,
                &candidate.session.key.session_id,
            ),
            candidate.incarnation,
            candidate.source_generation,
            candidate.published_fence,
            &candidate.source_fingerprint,
            &ignored_work,
        ),
    )
    .map_err(InputLoadError::Serialization)?;
    let check = {
        let mut cache = SOURCE_CHECK
            .lock()
            .map_err(|_| InputLoadError::Preparation(JevError::InvalidCheckContext))?;
        if let Some(cached) = cache.as_ref().filter(|cached| cached.key == cache_key) {
            Arc::clone(&cached.check)
        } else {
            let check = Arc::new(
                ScopeCreepCheck::new(snapshot.scope_creep_input(ignored_work.clone())?)
                    .map_err(InputLoadError::Preparation)?,
            );
            *cache = Some(CachedScopeCheck {
                key: cache_key,
                check: Arc::clone(&check),
            });
            check
        }
    };
    let scope_digest = check.context().check_context["scope_digest"]
        .as_str()
        .ok_or(InputLoadError::Preparation(JevError::InvalidCheckContext))?
        .to_owned();
    let plan = JevCheckPlan {
        check_id: CHECK_ID.into(),
        input_revision: check.context().input_revision.clone(),
        revisions: REVISIONS,
        work_items: Vec::new(),
        skipped_item_ids: Vec::new(),
        coverage: Default::default(),
        capabilities: capabilities.clone(),
        shared_context: None,
        prepared: ScopeCreepPrepared {
            scope_digest,
            semantic_epoch: check
                .semantic_epoch(capabilities)
                .map_err(InputLoadError::Preparation)?,
            source_generation: candidate.source_generation,
            publication_fence: candidate.published_fence,
            groups: Vec::new(),
            scope_bindings: snapshot.scope().user_context().evidence,
            session_limitation: None,
        },
    };
    let evaluator_revision = CHECK.evaluator_revision();
    let fit_key = identity(
        "scope-fit-v1",
        &(snapshot.input_revision(), capabilities, &evaluator_revision),
    )
    .map_err(InputLoadError::Serialization)?;
    let input_revision = identity(
        "scope-execution-v1",
        &(
            &fit_key,
            &check.context().input_revision,
            &configuration_fence,
        ),
    )
    .map_err(InputLoadError::Serialization)?;
    let mut citations: BTreeMap<_, _> = snapshot
        .content()
        .actions
        .iter()
        .map(|action| {
            (
                action.reference.id.clone(),
                action.text.chars().take(4096).collect(),
            )
        })
        .collect();
    for occurrence in snapshot.scope().occurrences() {
        let text = serde_json::to_string(&snapshot.scope().values()[occurrence.value_index])
            .map_err(InputLoadError::Serialization)?;
        citations.insert(
            occurrence.reference.id.clone(),
            text.chars().take(4096).collect(),
        );
    }
    Ok(PreparedInput {
        durable: BurnCheckInput {
            key: candidate.session.key.clone(),
            check_id: CHECK_ID.into(),
            incarnation: candidate.incarnation,
            source_generation: candidate.source_generation,
            source_fingerprint: candidate.source_fingerprint.clone(),
            activity_cursor: candidate.activity_cursor.clone(),
            published_fence: candidate.published_fence,
            input_revision,
            evaluator_revision,
            boundary_at_epoch: candidate.boundary_at_epoch,
        },
        check,
        plan,
        snapshot_revision: snapshot.input_revision().into(),
        configuration_fence,
        fit_key,
        ignored_work,
        citations,
    })
}

fn load_descriptor_input(
    store: &Store,
    candidate: &BurnCheckCandidate,
    capabilities: &ModelCapabilities,
) -> Result<PreparedInput, InputLoadError> {
    let candidate = store
        .enrolled_burn_check_candidate(candidate, CHECK_ID)
        .map_err(InputLoadError::Storage)?;
    let snapshot = store.load_smart_check_inputs(
        &candidate.session.key,
        candidate.published_fence,
        candidate.source_generation,
        DetectorInput::ScopeCreep,
    )?;
    let (ignored, configuration) = {
        let connection = store.lock();
        (
            ignored_instruction_work_ids(&connection, &SourceFence::from(&candidate))
                .map_err(InputLoadError::Storage)?,
            configuration_fence(&connection).map_err(InputLoadError::Storage)?,
        )
    };
    prepare_descriptor_input(&candidate, snapshot, capabilities, ignored, configuration)
}

#[cfg(test)]
pub(crate) fn prepare(
    candidate: &BurnCheckCandidate,
    snapshot: SmartCheckInputSnapshot,
    capabilities: &ModelCapabilities,
    ignored_work: BTreeSet<String>,
    configuration_fence: String,
) -> Result<PreparedInput, InputLoadError> {
    let mut input = prepare_descriptor_input(
        candidate,
        snapshot,
        capabilities,
        ignored_work,
        configuration_fence,
    )?;
    input.plan = input
        .check
        .prepare_with_capabilities(input.check.context(), capabilities)
        .map_err(InputLoadError::Preparation)?;
    Ok(input)
}

#[cfg(test)]
pub(crate) fn load_input(
    store: &Store,
    candidate: &BurnCheckCandidate,
    capabilities: &ModelCapabilities,
) -> Result<PreparedInput, InputLoadError> {
    let mut input = load_descriptor_input(store, candidate, capabilities)?;
    input.plan = input
        .check
        .prepare_with_capabilities(input.check.context(), capabilities)
        .map_err(InputLoadError::Preparation)?;
    Ok(input)
}

fn new_sampling() -> anyhow::Result<SamplingProgress> {
    SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: 65_536,
        answers_per_candidate: 1,
        judgments_per_run: 4,
    })
    .map_err(|error| anyhow::anyhow!("invalid sampling limits: {error:?}"))
}

fn restore_cursor(
    stored: Option<&BurnCheckAssessment>,
    input: &BurnCheckInput,
    generation: u64,
) -> AssessmentCursor {
    let previous = stored
        .and_then(|saved| serde_json::from_str::<AssessmentCursor>(&saved.progress_json).ok())
        .filter(|cursor| cursor.revision == CURSOR_REVISION);
    match previous {
        Some(mut cursor) if cursor.input_revision == input.input_revision => {
            cursor.provider_generation = generation;
            cursor
        }
        Some(mut cursor) => {
            cursor.input_revision = input.input_revision.clone();
            cursor.provider_generation = generation;
            cursor.active_job = None;
            cursor.run_progress = JevRunProgress::default();
            cursor.run_started = false;
            cursor.run_finished = false;
            cursor.blocked_fit_key = None;
            cursor
        }
        None => AssessmentCursor {
            revision: CURSOR_REVISION,
            input_revision: input.input_revision.clone(),
            provider_generation: generation,
            ..Default::default()
        },
    }
}

fn save_cursor(
    store: &Store,
    input: &BurnCheckInput,
    cursor: &AssessmentCursor,
) -> anyhow::Result<bool> {
    store.save_burn_check_checkpoint(
        input,
        &serde_json::to_string(cursor)?,
        None,
        unix_now(),
        POLICY.lease_secs,
        POLICY.idle_secs,
    )
}

fn fit_is_blocked(stored: Option<&BurnCheckAssessment>, input: &PreparedInput) -> bool {
    stored
        .and_then(|saved| serde_json::from_str::<AssessmentCursor>(&saved.progress_json).ok())
        .is_some_and(|cursor| {
            cursor.revision == CURSOR_REVISION
                && cursor.blocked_fit_key.as_ref() == Some(&input.fit_key)
        })
}

fn save_failure(
    store: &Store,
    input: &PreparedInput,
    cursor: &AssessmentCursor,
    category: &str,
    retry: Option<i64>,
) -> anyhow::Result<bool> {
    let result = cursor
        .result
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("scope result is missing"))?;
    store.fail_burn_check_assessment_with_result(
        &input.durable,
        &BurnCheckFailure {
            error_category: category,
            result_json: &serde_json::to_string(&publication(input, result.clone()))?,
            progress_json: &serde_json::to_string(cursor)?,
            retry_at_epoch: retry,
        },
        unix_now(),
        POLICY.idle_secs,
    )
}

async fn run_candidate(execution: CandidateExecution<'_>) -> anyhow::Result<()> {
    let CandidateExecution {
        app,
        store,
        candidate,
        client,
        handle,
        key_generation,
        events,
    } = execution;
    let capabilities = handle.resolve_capabilities(key_generation).await?;
    let mut input = match load_descriptor_input(store, candidate, &capabilities) {
        Ok(input) => input,
        Err(error) => {
            tracing::debug!(event = "scope_creep_input_unavailable", error = ?error);
            store.record_burn_check_candidate_issue_for_check(
                CHECK_ID,
                candidate,
                candidate.session.key.environment_key != "native"
                    || !matches!(
                        candidate.session.key.agent.as_str(),
                        "opencode" | "codex" | "claude" | "claude-code" | "pi"
                    ),
                unix_now().saturating_add(POLICY.retry_delay_secs),
                unix_now(),
            )?;
            return Ok(());
        }
    };
    let stored = store.burn_check_assessment(&candidate.session.key, CHECK_ID)?;
    if fit_is_blocked(stored.as_ref(), &input) {
        return Ok(());
    }
    let mut cursor = restore_cursor(stored.as_ref(), &input.durable, key_generation);
    if cursor.sampling.is_none() {
        cursor.sampling = Some(new_sampling()?);
    }
    enumerate_scope_turn(&mut input, &mut cursor)?;
    if !store.queue_burn_check_assessment(&input.durable, unix_now(), POLICY.idle_secs)?
        || !store.claim_burn_check_assessment(
            &input.durable,
            unix_now(),
            POLICY.lease_secs,
            POLICY.idle_secs,
        )?
    {
        return Ok(());
    }
    save_scheduling(store, &input, &cursor)?;
    if !save_cursor(store, &input.durable, &cursor)? {
        return Ok(());
    }
    if !cursor.run_started || cursor.run_finished {
        cursor.sampling.as_mut().expect("initialized").begin_run();
        cursor.run_started = true;
        cursor.run_finished = false;
    }
    loop {
        let Some(job) = cursor
            .active_job
            .clone()
            .or_else(|| cursor.sampling.as_mut().expect("initialized").choose_job())
        else {
            cursor.run_finished = true;
            break;
        };
        cursor.active_job = Some(job.clone());
        if !input_is_current(store, candidate, &capabilities, &input)?
            || !save_cursor(store, &input.durable, &cursor)?
        {
            return Ok(());
        }
        let mut plan = input.check.prepare_descriptors(
            &cursor.inventory,
            &capabilities,
            &BTreeSet::from([job.candidate]),
        )?;
        for group in &plan.prepared.groups {
            if let Some(current) = input
                .plan
                .prepared
                .groups
                .iter_mut()
                .find(|current| current.id == group.id)
            {
                *current = group.clone();
            }
        }
        let prepared = cursor
            .prepared
            .get_or_insert_with(|| input.plan.prepared.clone());
        for group in &plan.prepared.groups {
            if let Some(current) = prepared
                .groups
                .iter_mut()
                .find(|current| current.id == group.id)
            {
                *current = group.clone();
            }
        }
        if plan.work_items.is_empty() {
            let gap = input.check.reduce(&plan, &[], false)?;
            merge_result(cursor.result.as_mut().expect("initialized"), gap, &plan);
            update_result_counts(&mut cursor, &input);
        }
        if cursor
            .result
            .as_ref()
            .expect("initialized")
            .decisions
            .iter()
            .any(|decision| {
                StableId::new("scope_work", &[decision.group_id.as_bytes()]) == job.candidate
                    && decision.outcome.is_some()
            })
        {
            record_completion(
                cursor.sampling.as_mut().expect("initialized"),
                &job,
                cursor.result.as_ref().expect("initialized"),
                &plan,
            )?;
            cursor.active_job = None;
            cursor.run_progress = JevRunProgress::default();
            save_cursor(store, &input.durable, &cursor)?;
            save_scheduling(store, &input, &cursor)?;
            continue;
        }
        match dispatch_readiness(
            store,
            handle,
            &input.durable,
            &capabilities,
            &plan,
            &cursor.run_progress,
        )? {
            crate::store::BurnCheckRequestAdmission::Exhausted
            | crate::store::BurnCheckRequestAdmission::Unresolved => {
                cursor
                    .sampling
                    .as_mut()
                    .expect("initialized")
                    .terminate_candidate(&job)
                    .map_err(|error| anyhow::anyhow!("scope termination rejected: {error:?}"))?;
                cursor.active_job = None;
                cursor.run_progress = JevRunProgress::default();
                save_cursor(store, &input.durable, &cursor)?;
                save_scheduling(store, &input, &cursor)?;
                continue;
            }
            crate::store::BurnCheckRequestAdmission::Deferred => {
                save_cursor(store, &input.durable, &cursor)?;
                store.release_failed_burn_check_lease(
                    &input.durable,
                    "continuing",
                    store
                        .burn_check_next_attempt_at(&input.durable)?
                        .unwrap_or(unix_now() + 1),
                )?;
                return Ok(());
            }
            crate::store::BurnCheckRequestAdmission::Stale => return Ok(()),
            crate::store::BurnCheckRequestAdmission::Admitted => {}
        }
        let progress = std::mem::take(&mut cursor.run_progress);
        let orchestration = admit_jev_orchestration().await?;
        let outcome = run_prepared_check(
            BatchExecution {
                app,
                store,
                input: &input.durable,
                client: client.clone(),
                handle,
                key_generation,
                events,
                policy: POLICY,
                capabilities: &capabilities,
            },
            input.check.as_ref(),
            input.check.context(),
            &mut plan,
            progress,
            orchestration,
            |progress| {
                cursor.run_progress = progress.clone();
                serde_json::to_string(&cursor).map_err(|_| JevError::ProgressStorageFailure)
            },
        )
        .await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(JevError::Cancelled) if handle.turn_exhausted(CHECK_ID, &candidate.session.key) => {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        if !handle.key_is_current(key_generation) {
            store.supersede_burn_check_assessment(&input.durable, unix_now())?;
            return Ok(());
        }
        cursor.run_progress = outcome.progress;
        for result in cursor.run_progress.results.values() {
            if let Some(previous) = cursor
                .accepted_request_usage
                .insert(result.request_id.clone(), result.usage)
                && previous != result.usage
            {
                anyhow::bail!("scope request usage changed");
            }
        }
        record_completion(
            cursor.sampling.as_mut().expect("initialized"),
            &job,
            &outcome.result,
            &plan,
        )?;
        merge_result(
            cursor.result.as_mut().expect("initialized"),
            outcome.result,
            &plan,
        );
        update_result_counts(&mut cursor, &input);
        save_cursor(store, &input.durable, &cursor)?;
        save_scheduling(store, &input, &cursor)?;
        if let Some(error) = outcome.failure {
            if matches!(error, JevError::Cancelled)
                && handle.turn_exhausted(CHECK_ID, &candidate.session.key)
            {
                store.release_failed_burn_check_lease(
                    &input.durable,
                    "continuing",
                    unix_now() + 1,
                )?;
                return Ok(());
            }
            if target_failure_is_terminal(&error, || {
                dispatch_readiness(
                    store,
                    handle,
                    &input.durable,
                    &capabilities,
                    &plan,
                    &cursor.run_progress,
                )
            })? && !cursor
                .sampling
                .as_ref()
                .expect("initialized")
                .completed_ids(ScopeCreepCheck::check_identity())
                .contains(&job.candidate)
            {
                cursor
                    .sampling
                    .as_mut()
                    .expect("initialized")
                    .terminate_candidate(&job)
                    .map_err(|error| anyhow::anyhow!("scope termination rejected: {error:?}"))?;
                cursor.active_job = None;
                cursor.run_progress = JevRunProgress::default();
                save_cursor(store, &input.durable, &cursor)?;
                save_scheduling(store, &input, &cursor)?;
                continue;
            }
            let rejected = matches!(error, JevError::AuthenticationRejected);
            let saved = handle
                .with_current_generation(key_generation, || {
                    let published = save_failure(
                        store,
                        &input,
                        &cursor,
                        error_category(&error),
                        store.burn_check_next_attempt_at(&input.durable)?,
                    )?;
                    if published && !matches!(error, JevError::Cancelled) {
                        let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
                        record_assessment(
                            app,
                            candidate.historical,
                            crate::analytics::event::SmartCheckAssessmentOutcome::Failed,
                        );
                    }
                    Ok::<_, anyhow::Error>(published)
                })
                .transpose();
            let rejection = crate::jev::worker::settle_authentication_rejection(
                app,
                store,
                handle,
                key_generation,
                rejected,
            );
            saved?;
            rejection?;
            return Ok(());
        }
        cursor.active_job = None;
        cursor.run_progress = JevRunProgress::default();
        if !save_cursor(store, &input.durable, &cursor)? {
            return Ok(());
        }
    }
    update_result_counts(&mut cursor, &input);
    let published = handle
        .with_current_generation(key_generation, || {
            let outcome = publish_current(store, candidate, &capabilities, &input, &cursor)?;
            if let Some(outcome) = outcome {
                record_assessment(app, candidate.historical, outcome);
            }
            Ok::<_, anyhow::Error>(outcome.is_some())
        })
        .transpose()?
        .unwrap_or(false);
    if published {
        let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
    }
    Ok(())
}

fn update_result_counts(cursor: &mut AssessmentCursor, input: &PreparedInput) {
    let result = cursor.result.as_mut().expect("initialized");
    result.coverage = input.plan.coverage.clone();
    result.coverage.skipped_items = input
        .plan
        .prepared
        .groups
        .iter()
        .filter(|group| group.window_ids.is_empty() && group.limitation.is_some())
        .count();
    result.coverage.limitations.extend(
        input
            .plan
            .prepared
            .groups
            .iter()
            .filter_map(|group| group.limitation.clone()),
    );
    if result
        .decisions
        .iter()
        .any(|decision| decision.status == ScopeCreepStatus::Uncertain)
    {
        result
            .coverage
            .limitations
            .push("uncertain_decision".into());
    }
    result.assessed_candidates = result
        .decisions
        .iter()
        .filter(|decision| {
            decision.outcome.is_some()
                && input
                    .plan
                    .prepared
                    .groups
                    .iter()
                    .any(|group| group.id == decision.group_id)
        })
        .count();
    result.remaining_candidates = input
        .plan
        .prepared
        .groups
        .len()
        .saturating_sub(result.assessed_candidates);
    result.coverage.selected_items = result.assessed_candidates;
    result.coverage.not_selected_items = result.remaining_candidates;
    result.coverage.processing_limit_reached |= !cursor.inventory.complete;
    if !cursor.inventory.complete {
        result
            .coverage
            .limitations
            .push("descriptor_enumeration_incomplete".into());
    }
    if result.remaining_candidates > 0 {
        result
            .coverage
            .limitations
            .push("assessment_incomplete".into());
    }
    result.request_count = cursor.accepted_request_usage.len();
    result.input_tokens = cursor
        .accepted_request_usage
        .values()
        .map(|usage| usage.input_tokens)
        .sum();
    result.output_tokens = cursor
        .accepted_request_usage
        .values()
        .map(|usage| usage.output_tokens)
        .sum();
}

fn save_scheduling(
    store: &Store,
    input: &PreparedInput,
    cursor: &AssessmentCursor,
) -> anyhow::Result<bool> {
    let (eligible, reviewed, runnable) = scope_scheduling_counts(input, cursor);
    store.save_burn_check_scheduling(&input.durable, eligible, reviewed, runnable)
}

fn scope_scheduling_counts(
    input: &PreparedInput,
    cursor: &AssessmentCursor,
) -> (Option<usize>, usize, usize) {
    let sampling = cursor.sampling.as_ref().expect("initialized");
    let coverage = sampling
        .coverage(ScopeCreepCheck::check_identity())
        .expect("synchronized");
    let reviewed = cursor.result.as_ref().map_or(coverage.completed, |result| {
        result
            .decisions
            .iter()
            .filter(|decision| {
                decision.outcome.is_some()
                    && input
                        .plan
                        .prepared
                        .groups
                        .iter()
                        .any(|group| group.id == decision.group_id)
            })
            .count()
    });
    (
        cursor.inventory.complete.then_some(coverage.eligible),
        reviewed,
        sampling
            .runnable_count(ScopeCreepCheck::check_identity())
            .saturating_sub(reviewed.saturating_sub(coverage.completed))
            .max(usize::from(!cursor.inventory.complete)),
    )
}

fn enumerate_scope_turn(
    input: &mut PreparedInput,
    cursor: &mut AssessmentCursor,
) -> anyhow::Result<()> {
    input.check.enumerate_descriptors(&mut cursor.inventory)?;
    let candidates = input
        .check
        .descriptor_candidates(&cursor.inventory, &input.plan.capabilities)?;
    let engine_epoch = input.check.semantic_epoch(&input.plan.capabilities)?;
    let epoch = StableId::new(
        "scope-context-v2",
        &[
            String::from(engine_epoch).as_bytes(),
            input.configuration_fence.as_bytes(),
        ],
    );
    if cursor
        .context_epoch
        .is_some_and(|previous| previous != epoch)
    {
        cursor.result = None;
        cursor.prepared = None;
        cursor.active_job = None;
        cursor.run_progress = JevRunProgress::default();
        cursor.run_started = false;
    }
    cursor.context_epoch = Some(epoch);
    cursor
        .sampling
        .as_mut()
        .expect("initialized")
        .synchronize_ordered(
            ScopeCreepCheck::check_identity(),
            epoch,
            &candidates,
            &ScopeCreepCheck::descriptor_chronology(&cursor.inventory),
        )
        .map_err(|error| anyhow::anyhow!("scope sampling inventory rejected: {error:?}"))?;
    let baseline = input.check.prepare_descriptors(
        &cursor.inventory,
        &input.plan.capabilities,
        &BTreeSet::new(),
    )?;
    let initial = input.check.reduce(&baseline, &[], false)?;
    let result = cursor.result.get_or_insert(initial.clone());
    if result.revisions != REVISIONS || result.model != initial.model {
        *result = initial.clone();
        cursor.prepared = None;
    }
    result.input_revision = initial.input_revision;
    result.scope_digest = initial.scope_digest;
    result.session_limitation = initial.session_limitation;
    input.plan = baseline;
    input.plan.prepared.groups = cursor.inventory.groups.clone();
    for (group, candidate) in input.plan.prepared.groups.iter_mut().zip(&candidates) {
        let previous = cursor.prepared.as_ref().and_then(|prepared| {
            prepared.groups.iter().find(|previous| {
                previous.id == group.id
                    && previous.semantic_digest == group.semantic_digest
                    && previous.context == group.context
                    && previous.work == group.work
            })
        });
        let accepted = result.decisions.iter().any(|decision| {
            decision.group_id == group.id
                && decision.outcome.is_some()
                && decision.reduced_answer_ids == candidate.required_answers
        });
        if accepted {
            if let Some(previous) = previous {
                *group = previous.clone();
            }
            for finding in result
                .findings
                .iter_mut()
                .filter(|finding| finding.group_id == group.id)
            {
                finding.source_generation = input.durable.source_generation;
                finding.publication_fence = input.durable.published_fence;
                finding.scope_digest = input.plan.prepared.scope_digest.clone();
                finding.id = antiburn_local::analysis::ignored_instructions::sha256_hex(
                    &serde_json::to_vec(&(
                        &group.id,
                        &finding.scope_digest,
                        &finding.model,
                        &finding.model_revision,
                        REVISIONS,
                    ))?,
                );
            }
        } else {
            if let Some(previous) = previous {
                *group = previous.clone();
                if result
                    .decisions
                    .iter()
                    .any(|decision| decision.group_id == group.id && decision.outcome.is_none())
                {
                    continue;
                }
            }
            result
                .decisions
                .retain(|decision| decision.group_id != group.id);
            result
                .findings
                .retain(|finding| finding.group_id != group.id);
            result
                .decisions
                .push(antiburn_local::checks::scope_creep::ScopeCreepDecision {
                    group_id: group.id.clone(),
                    status: ScopeCreepStatus::Unassessed,
                    outcome: None,
                    decision_probability: None,
                    limitation: group.limitation.clone(),
                    reduced_answer_ids: Vec::new(),
                });
        }
    }
    if cursor.inventory.complete {
        result.decisions.retain(|decision| {
            input
                .plan
                .prepared
                .groups
                .iter()
                .any(|group| group.id == decision.group_id)
        });
        result.findings.retain(|finding| {
            input
                .plan
                .prepared
                .groups
                .iter()
                .any(|group| group.id == finding.group_id)
        });
    }
    let mut prepared = input.plan.prepared.clone();
    if !cursor.inventory.complete
        && let Some(previous) = &cursor.prepared
    {
        prepared.groups.extend(
            previous
                .groups
                .iter()
                .filter(|previous| {
                    !cursor
                        .inventory
                        .groups
                        .iter()
                        .any(|group| group.id == previous.id)
                })
                .cloned(),
        );
    }
    cursor.prepared = Some(prepared);
    update_result_counts(cursor, input);
    Ok(())
}

#[cfg(test)]
fn synchronize_sampling(
    input: &PreparedInput,
    sampling: &mut SamplingProgress,
) -> anyhow::Result<()> {
    let inventory = ScopeCreepCheck::sampling_candidates(&input.plan);
    let chronology: Vec<_> = input
        .plan
        .prepared
        .groups
        .iter()
        .map(|group| StableId::new("scope_work", &[group.id.as_bytes()]))
        .collect();
    let epoch = StableId::new(
        "scope-context-v1",
        &[
            serde_json::to_string(&(&input.plan.capabilities, REVISIONS))?.as_bytes(),
            input.configuration_fence.as_bytes(),
        ],
    );
    sampling
        .synchronize_ordered(
            ScopeCreepCheck::check_identity(),
            epoch,
            &inventory,
            &chronology,
        )
        .map_err(|error| anyhow::anyhow!("scope sampling inventory rejected: {error:?}"))
}

pub(crate) fn dispatch_readiness<P>(
    store: &Store,
    handle: &crate::jev::worker::WorkerHandle,
    input: &BurnCheckInput,
    capabilities: &ModelCapabilities,
    plan: &JevCheckPlan<P>,
    progress: &JevRunProgress,
) -> anyhow::Result<crate::store::BurnCheckRequestAdmission> {
    use crate::store::BurnCheckRequestAdmission;
    let mut connection = handle.system_one_connection();
    connection
        .model_revision
        .clone_from(&capabilities.model_revision);
    let mut readiness = BurnCheckRequestAdmission::Admitted;
    for item in &plan.work_items {
        if progress.results.contains_key(&item.id) {
            continue;
        }
        let packed = match &plan.shared_context {
            Some(shared) => antiburn_local::analysis::jev::pack_work_items_with_shared_context(
                std::slice::from_ref(item),
                capabilities,
                shared,
            ),
            None => antiburn_local::analysis::jev::pack_work_items_with_capabilities(
                std::slice::from_ref(item),
                capabilities,
            ),
        };
        if packed.batches.is_empty() {
            return Ok(BurnCheckRequestAdmission::Exhausted);
        }
        for batch in &packed.batches {
            let identities =
                crate::jev::worker::batch_request_identities(&connection, input, batch);
            match store.burn_check_dispatch_readiness(&identities, unix_now())? {
                BurnCheckRequestAdmission::Admitted => {}
                BurnCheckRequestAdmission::Deferred => {
                    readiness = BurnCheckRequestAdmission::Deferred
                }
                blocked => return Ok(blocked),
            }
        }
    }
    if plan.work_items.is_empty() {
        return Ok(BurnCheckRequestAdmission::Exhausted);
    }
    Ok(readiness)
}

pub(crate) fn target_failure_is_terminal(
    error: &JevError,
    readiness: impl FnOnce() -> anyhow::Result<crate::store::BurnCheckRequestAdmission>,
) -> anyhow::Result<bool> {
    if matches!(error, JevError::AuthenticationRejected) {
        return Ok(false);
    }
    Ok(matches!(
        readiness()?,
        crate::store::BurnCheckRequestAdmission::Exhausted
            | crate::store::BurnCheckRequestAdmission::Unresolved
    ))
}

#[cfg(test)]
fn reuse_accepted_result(
    stored: Option<&BurnCheckAssessment>,
    input: &PreparedInput,
    cursor: &mut AssessmentCursor,
) -> anyhow::Result<()> {
    let Some(previous) = stored
        .and_then(|saved| saved.result_json.as_deref())
        .and_then(|json| serde_json::from_str::<Publication>(json).ok())
    else {
        return Ok(());
    };
    if previous.configuration_fence != input.configuration_fence
        || previous.capabilities != input.plan.capabilities
    {
        return Ok(());
    }
    let target = cursor.result.as_mut().expect("initialized");
    for group in &input.plan.prepared.groups {
        if !previous.prepared.groups.contains(group) {
            continue;
        }
        let Some(decision) = previous.assessment.decisions.iter().find(|decision| {
            decision.group_id == group.id
                && valid_decision(decision, group, &previous.prepared)
                && decision.outcome.is_some()
        }) else {
            continue;
        };
        if let Some(current) = target
            .decisions
            .iter_mut()
            .find(|current| current.group_id == group.id)
        {
            *current = decision.clone();
            current.reduced_answer_ids = group
                .window_ids
                .iter()
                .map(|window| {
                    ScopeCreepCheck::answer_identity(&input.plan, window, ScopeQuestion::Decision)
                })
                .collect();
        }
        for finding in previous
            .assessment
            .findings
            .iter()
            .filter(|finding| finding.group_id == group.id)
        {
            let mut finding = finding.clone();
            finding.source_generation = input.durable.source_generation;
            finding.publication_fence = input.durable.published_fence;
            finding.scope_digest = input.plan.prepared.scope_digest.clone();
            finding.id = antiburn_local::analysis::ignored_instructions::sha256_hex(
                &serde_json::to_vec(&(
                    &group.id,
                    &finding.scope_digest,
                    &finding.model,
                    &finding.model_revision,
                    REVISIONS,
                ))?,
            );
            target.findings.push(finding);
        }
    }
    Ok(())
}

fn record_assessment(
    app: &tauri::AppHandle,
    historical: bool,
    outcome: crate::analytics::event::SmartCheckAssessmentOutcome,
) {
    crate::analytics::record_smart_check_lifecycle(
        app,
        crate::analytics::event::SmartCheckLifecycle::Assessment {
            check: crate::analytics::event::SmartCheck::ScopeCreep,
            historical,
            outcome,
        },
    );
}

fn record_completion(
    sampling: &mut SamplingProgress,
    job: &SamplingJob,
    result: &ScopeCreepResult,
    _plan: &JevCheckPlan<ScopeCreepPrepared>,
) -> anyhow::Result<()> {
    if sampling.completed_ids(job.check).contains(&job.candidate) {
        return Ok(());
    }
    if let Some(decision) = result.decisions.iter().find(|decision| {
        StableId::new("scope_work", &[decision.group_id.as_bytes()]) == job.candidate
            && decision.outcome.is_some()
    }) {
        for answer in &decision.reduced_answer_ids {
            sampling
                .record_reduced_answer(job, *answer)
                .map_err(|error| anyhow::anyhow!("scope answer rejected: {error:?}"))?;
        }
        sampling
            .complete_candidate(job)
            .map_err(|error| anyhow::anyhow!("scope completion rejected: {error:?}"))?;
    }
    Ok(())
}

fn merge_result(
    target: &mut ScopeCreepResult,
    page: ScopeCreepResult,
    plan: &JevCheckPlan<ScopeCreepPrepared>,
) {
    let groups: BTreeSet<_> = plan.prepared.groups.iter().map(|group| &group.id).collect();
    target
        .findings
        .retain(|finding| !groups.contains(&finding.group_id));
    target
        .decisions
        .retain(|decision| !groups.contains(&decision.group_id));
    target.findings.extend(page.findings);
    target.decisions.extend(page.decisions);
}

fn input_is_current(
    store: &Store,
    candidate: &BurnCheckCandidate,
    capabilities: &ModelCapabilities,
    input: &PreparedInput,
) -> anyhow::Result<bool> {
    if load_descriptor_input(store, candidate, capabilities)
        .as_ref()
        .is_ok_and(|current| current.durable.input_revision == input.durable.input_revision)
    {
        return Ok(true);
    }
    store.supersede_burn_check_assessment(&input.durable, unix_now())?;
    Ok(false)
}

fn publish_current(
    store: &Store,
    candidate: &BurnCheckCandidate,
    capabilities: &ModelCapabilities,
    input: &PreparedInput,
    cursor: &AssessmentCursor,
) -> anyhow::Result<Option<crate::analytics::event::SmartCheckAssessmentOutcome>> {
    if !input_is_current(store, candidate, capabilities, input)?
        || !save_cursor(store, &input.durable, cursor)?
    {
        return Ok(None);
    }
    let publication = publication(input, cursor.result.as_ref().expect("initialized").clone());
    if !valid_publication(&publication) {
        anyhow::bail!("scope publication proof is invalid");
    }
    let clean = publication_has_clean_coverage(&publication.assessment);
    let has_finding = !publication.assessment.findings.is_empty()
        && publication
            .assessment
            .findings
            .iter()
            .all(|finding| publishable_finding(finding, &publication));
    let published = if clean {
        store.complete_burn_check_assessment(
            &input.durable,
            &serde_json::to_string(&publication)?,
            unix_now(),
            POLICY.idle_secs,
        )?
    } else {
        let (_, _, runnable) = scope_scheduling_counts(input, cursor);
        save_failure(
            store,
            input,
            cursor,
            if runnable > 0 || !cursor.inventory.complete {
                "continuing"
            } else {
                "sampling_incomplete"
            },
            (runnable > 0 || !cursor.inventory.complete).then(|| unix_now() + 1),
        )?
    };
    Ok(published.then(|| {
        crate::analytics::event::SmartCheckAssessmentOutcome::from_evidence(has_finding, clean)
    }))
}

pub(crate) fn publication(input: &PreparedInput, assessment: ScopeCreepResult) -> Publication {
    let mut assessment = assessment;
    assessment.decisions.retain(|decision| {
        input
            .plan
            .prepared
            .groups
            .iter()
            .any(|group| group.id == decision.group_id)
    });
    assessment.findings.retain(|finding| {
        input
            .plan
            .prepared
            .groups
            .iter()
            .any(|group| group.id == finding.group_id)
    });
    let ids: BTreeSet<_> = assessment
        .findings
        .iter()
        .flat_map(|finding| {
            finding
                .task_scope
                .iter()
                .map(|item| &item.source_id)
                .chain(finding.work.iter().map(|work| &work.reference.id))
        })
        .collect();
    Publication {
        input_revision: input.durable.input_revision.clone(),
        snapshot_revision: input.snapshot_revision.clone(),
        configuration_fence: input.configuration_fence.clone(),
        capabilities: input.plan.capabilities.clone(),
        fit_key: input.fit_key.clone(),
        ignored_work: input.ignored_work.clone(),
        prepared: input.plan.prepared.clone(),
        citations: input
            .citations
            .iter()
            .filter(|(id, _)| ids.contains(id))
            .map(|(id, text)| (id.clone(), text.clone()))
            .collect(),
        assessment,
    }
}

pub(crate) fn publication_has_clean_coverage(result: &ScopeCreepResult) -> bool {
    result.session_limitation.is_none()
        && result.remaining_candidates == 0
        && result.assessed_candidates > 0
        && result.assessed_candidates == result.decisions.len()
        && result.coverage.not_selected_items == 0
        && result.coverage.skipped_items == 0
        && !result.coverage.processing_limit_reached
        && result.coverage.limitations.is_empty()
        && result.decisions.iter().all(|decision| {
            matches!(
                decision.status,
                ScopeCreepStatus::Finding | ScopeCreepStatus::Clean
            ) && decision.limitation.is_none()
        })
}

fn valid_publication(publication: &Publication) -> bool {
    let result = &publication.assessment;
    let prepared = &publication.prepared;
    let groups: BTreeSet<_> = prepared.groups.iter().map(|group| &group.id).collect();
    let decisions: BTreeSet<_> = result
        .decisions
        .iter()
        .map(|decision| &decision.group_id)
        .collect();
    let finding_groups: BTreeSet<_> = result
        .findings
        .iter()
        .map(|finding| &finding.group_id)
        .collect();
    let decision_findings: BTreeSet<_> = result
        .decisions
        .iter()
        .filter(|decision| decision.status == ScopeCreepStatus::Finding)
        .map(|decision| &decision.group_id)
        .collect();
    !publication.input_revision.is_empty()
        && !publication.snapshot_revision.is_empty()
        && !result.input_revision.is_empty()
        && result.revisions == REVISIONS
        && result.scope_digest == prepared.scope_digest
        && result.model == publication.capabilities.model
        && result.session_limitation == prepared.session_limitation
        && groups.len() == prepared.groups.len()
        && groups == decisions
        && decisions.len() == result.decisions.len()
        && finding_groups.len() == result.findings.len()
        && finding_groups == decision_findings
        && identity(
            "scope-fit-v1",
            &(
                &publication.snapshot_revision,
                &publication.capabilities,
                CHECK.evaluator_revision(),
            ),
        )
        .is_ok_and(|key| key == publication.fit_key)
        && identity(
            "scope-execution-v1",
            &(
                &publication.fit_key,
                &result.input_revision,
                &publication.configuration_fence,
            ),
        )
        .is_ok_and(|key| key == publication.input_revision)
        && result.assessed_candidates
            == result
                .decisions
                .iter()
                .filter(|decision| decision.outcome.is_some())
                .count()
        && result.remaining_candidates
            == prepared
                .groups
                .len()
                .saturating_sub(result.assessed_candidates)
        && result.decisions.iter().all(|decision| {
            let group = prepared
                .groups
                .iter()
                .find(|group| group.id == decision.group_id)
                .expect("validated groups");
            valid_decision(decision, group, prepared)
        })
        && result
            .findings
            .iter()
            .all(|finding| publishable_finding(finding, publication))
}

fn valid_decision(
    decision: &antiburn_local::checks::scope_creep::ScopeCreepDecision,
    group: &antiburn_local::checks::scope_creep::WorkGroup,
    prepared: &ScopeCreepPrepared,
) -> bool {
    let Some(outcome) = decision.outcome else {
        return decision.status == ScopeCreepStatus::Unassessed
            && decision.decision_probability.is_none()
            && decision.reduced_answer_ids.is_empty();
    };
    let Some(probability) = decision.decision_probability else {
        return false;
    };
    if !probability.is_finite()
        || !(0.0..=1.0).contains(&probability)
        || group.window_ids.len() != 1
    {
        return false;
    }
    let expected = match outcome {
        ScopeAnswer::LikelyScopeExpansion if probability >= DECISION_THRESHOLD => {
            ScopeCreepStatus::Finding
        }
        ScopeAnswer::LikelyScopeExpansion | ScopeAnswer::Uncertain => ScopeCreepStatus::Uncertain,
        ScopeAnswer::NoIssue => ScopeCreepStatus::Clean,
        _ => return false,
    };
    let answer_plan = JevCheckPlan {
        check_id: CHECK_ID.into(),
        input_revision: String::new(),
        revisions: REVISIONS,
        work_items: Vec::new(),
        skipped_item_ids: Vec::new(),
        coverage: Default::default(),
        capabilities: ModelCapabilities::jev_default(),
        shared_context: None,
        prepared: ScopeCreepPrepared {
            scope_digest: String::new(),
            semantic_epoch: prepared.semantic_epoch,
            source_generation: prepared.source_generation,
            publication_fence: prepared.publication_fence,
            groups: vec![group.clone()],
            scope_bindings: Vec::new(),
            session_limitation: None,
        },
    };
    decision.status == expected
        && decision.reduced_answer_ids
            == ScopeCreepCheck::sampling_candidates(&answer_plan)[0].required_answers
}

pub(crate) fn publishable_finding(finding: &ScopeCreepFinding, publication: &Publication) -> bool {
    let Some(group) = publication
        .prepared
        .groups
        .iter()
        .find(|group| group.id == finding.group_id)
    else {
        return false;
    };
    let Some(decision) = publication
        .assessment
        .decisions
        .iter()
        .find(|decision| decision.group_id == finding.group_id)
    else {
        return false;
    };
    let expected_id = serde_json::to_vec(&(
        &group.id,
        &publication.prepared.scope_digest,
        &publication.capabilities.model,
        &publication.capabilities.model_revision,
        REVISIONS,
    ))
    .map(|bytes| antiburn_local::analysis::ignored_instructions::sha256_hex(&bytes));
    expected_id.is_ok_and(|id| id == finding.id)
        && finding.work == group.work
        && !finding.work.is_empty()
        && finding.task_scope == group.task_scope
        && finding.observation_kind == group.observation_kind
        && finding.decision_probability.is_finite()
        && finding.decision_probability >= DECISION_THRESHOLD
        && finding.decision_probability <= 1.0
        && decision.decision_probability == Some(finding.decision_probability)
        && finding.scope_digest == publication.prepared.scope_digest
        && finding.revisions == REVISIONS
        && finding.model == publication.capabilities.model
        && finding.model_revision == publication.capabilities.model_revision
        && finding.source_generation == publication.prepared.source_generation
        && finding.publication_fence == publication.prepared.publication_fence
        && decision.status == ScopeCreepStatus::Finding
        && valid_decision(decision, group, &publication.prepared)
        && finding.work.iter().all(|work| {
            work.reference.stable
                && !work.digest.is_empty()
                && publication.citations.contains_key(&work.reference.id)
        })
        && finding
            .task_scope
            .iter()
            .all(|item| publication.citations.contains_key(&item.source_id))
}

/// Report, finding-detail, and prompt actions use this same source/config gate.
pub(crate) fn current_publication(
    connection: &rusqlite::Connection,
    fence: &SourceFence<'_>,
) -> anyhow::Result<Option<Publication>> {
    if fence.key.environment_key != "native" {
        return Ok(None);
    }
    let stored = connection.query_row(
        "SELECT a.status, a.input_revision, a.result_revision, a.result_json, a.last_error_category, e.evidence_json
         FROM burn_check_assessment a JOIN session s
           ON s.environment_key = a.environment_key AND s.agent = a.agent AND s.session_id = a.session_id
         JOIN session_evidence e ON e.environment_key = s.environment_key AND e.agent = s.agent AND e.session_id = s.session_id
         WHERE a.environment_key = ?1 AND a.agent = ?2 AND a.session_id = ?3 AND a.check_id = ?4
           AND s.incarnation = ?5 AND a.incarnation = s.incarnation
           AND s.source_generation = ?6 AND a.source_generation = s.source_generation
           AND s.source_fingerprint IS ?7 AND a.source_fingerprint IS s.source_fingerprint
           AND e.published_fence = ?8 AND a.published_fence = e.published_fence
           AND a.evaluator_revision = ?9 AND e.status = 'ready'
           AND e.analyzed_generation = s.source_generation AND e.processed_fingerprint IS s.source_fingerprint
           AND e.parser_revision = ?10 AND e.analyzer_revision = ?11 AND e.evidence_schema_revision = ?12",
        params![fence.key.environment_key, fence.key.agent, fence.key.session_id, CHECK_ID,
            fence.incarnation, fence.source_generation, fence.source_fingerprint, fence.published_fence,
            CHECK.evaluator_revision(), antiburn_local::analysis::PARSER_REVISION,
            antiburn_local::analysis::ANALYZER_REVISION, antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, Option<String>>(4)?, row.get::<_, Option<String>>(5)?)),
    ).optional()?;
    let Some((status, Some(input), Some(revision), Some(json), category, Some(evidence_json))) =
        stored
    else {
        return Ok(None);
    };
    let Ok(evidence) =
        serde_json::from_str::<antiburn_local::analysis::SessionEvidence>(&evidence_json)
    else {
        return Ok(None);
    };
    if evidence.identity.agent != fence.key.agent
        || evidence.identity.session_id != fence.key.session_id
        || !source_supported(&fence.key.agent, evidence.capabilities.source_format)
    {
        return Ok(None);
    }
    let Ok(publication) = serde_json::from_str::<Publication>(&json) else {
        return Ok(None);
    };
    if input != revision
        || publication.input_revision != revision
        || publication.configuration_fence != configuration_fence(connection)?
        || publication.ignored_work != ignored_instruction_work_ids(connection, fence)?
        || publication.prepared.source_generation != fence.source_generation
        || publication.prepared.publication_fence != fence.published_fence
        || !valid_publication(&publication)
    {
        return Ok(None);
    }
    let complete = status == "completed" && publication_has_clean_coverage(&publication.assessment);
    let partial = status == "failed"
        && (category.as_deref() == Some("sampling_incomplete")
            || !publication.assessment.findings.is_empty());
    Ok((complete || partial).then_some(publication))
}

/// Retrieve saved citation text only after the current publication passes its gate.
pub(crate) fn saved_finding_citations(
    connection: &rusqlite::Connection,
    fence: &SourceFence<'_>,
    finding_id: &str,
) -> anyhow::Result<Option<BTreeMap<String, String>>> {
    let Some(publication) = current_publication(connection, fence)? else {
        return Ok(None);
    };
    let Some(finding) = publication
        .assessment
        .findings
        .iter()
        .find(|finding| finding.id == finding_id)
    else {
        return Ok(None);
    };
    let ids: BTreeSet<_> = finding
        .work
        .iter()
        .map(|work| &work.reference.id)
        .chain(finding.task_scope.iter().map(|item| &item.source_id))
        .collect();
    Ok(Some(
        publication
            .citations
            .iter()
            .filter(|(id, _)| ids.contains(id))
            .map(|(id, text)| (id.clone(), text.clone()))
            .collect(),
    ))
}

#[cfg(test)]
pub(crate) mod tests;
