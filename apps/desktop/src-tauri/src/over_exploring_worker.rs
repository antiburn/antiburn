//! Feature-owned adapter for the shared Smart Burn Check worker.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, Mutex};

use antiburn_local::analysis::SourceFormat;
use antiburn_local::analysis::jev::capabilities::ModelCapabilities;
use antiburn_local::analysis::jev::{
    JevCheck, JevCheckPlan, JevError, JevEvidenceReference, JevRunProgress, JevSessionContext,
    admit_jev_orchestration,
};
use antiburn_local::checks::over_exploring::{
    self, Assessment, Decision, MAX_TARGETS_PER_TURN, OverExploringCheck, PreparedAssessment,
    SEMANTIC_PROBABILITY_THRESHOLD, SemanticOutcome, Target,
};
use antiburn_local::checks::sampling::{SamplingJob, SamplingLimits, SamplingProgress, StableId};
use tauri::Emitter;

use crate::jev::worker::{
    BatchExecution, CandidateExecution, CheckPolicy, JevCheckDescriptor, WorkerFuture,
    error_category, run_prepared_check, unix_now,
};
use crate::smart_check_inputs::{
    DetectorInput, InputLoadError, InputUnavailable, SmartCheckInputSnapshot,
};
use crate::store::{
    BurnCheckAssessment, BurnCheckCandidate, BurnCheckFailure, BurnCheckInput, Store,
};

pub(crate) const CHECK_ID: &str = "over_exploring";
static PREPARED_INPUTS: LazyLock<
    Mutex<crate::smart_check_inputs::cache::PreparedInputCache<PreparedInput>>,
> = LazyLock::new(|| Mutex::new(Default::default()));
const CURSOR_REVISION: u32 = 6;
// Accepted input has at most 4096 events and 256 disjoint episodes.
const MAX_SAMPLING_CANDIDATES: usize = 2 * 4096 + 256;
const POLICY: CheckPolicy = CheckPolicy {
    idle_secs: 180,
    lease_secs: 300,
    retry_delay_secs: 300,
};

pub(crate) struct OverExploringDescriptor;
pub(crate) const CHECK: OverExploringDescriptor = OverExploringDescriptor;

impl JevCheckDescriptor for OverExploringDescriptor {
    fn id(&self) -> &'static str {
        CHECK_ID
    }

    fn evaluator_revision(&self) -> String {
        let revisions = OverExploringCheck.revisions();
        format!(
            "over-exploring-adapter-v{CURSOR_REVISION}:{}:{}:{}:{}",
            revisions.projection, revisions.chunking, revisions.questions, revisions.reducer
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
    #[serde(default)]
    context_blocked_ids: BTreeSet<String>,
    revision: u32,
    input_revision: String,
    provider_generation: u64,
    inventory_count: usize,
    inventory_total: usize,
    sampling: Option<SamplingProgress>,
    active_job: Option<SamplingJob>,
    run_progress: JevRunProgress,
    result: Option<Assessment>,
    run_started: bool,
    run_finished: bool,
}

#[derive(Clone)]
pub(crate) struct PreparedInput {
    pub(crate) durable: BurnCheckInput,
    pub(crate) context: JevSessionContext,
    pub(crate) plan: JevCheckPlan<PreparedAssessment>,
    snapshot_revision: String,
    sampling_overflow: usize,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Publication {
    pub(crate) input_revision: String,
    pub(crate) snapshot_revision: String,
    pub(crate) semantic_revision: String,
    pub(crate) model: String,
    pub(crate) work_evidence: BTreeMap<String, Vec<JevEvidenceReference>>,
    pub(crate) work_targets: BTreeMap<String, Target>,
    pub(crate) task_evidence: Vec<JevEvidenceReference>,
    #[serde(default)]
    pub(crate) work_task_evidence: BTreeMap<String, Vec<JevEvidenceReference>>,
    #[serde(flatten)]
    pub(crate) assessment: Assessment,
}

pub(crate) fn source_supported(agent: &str, format: SourceFormat) -> bool {
    antiburn_local::analysis::smart_check_source_supported(agent, format)
}

pub(crate) fn prepare(
    candidate: &BurnCheckCandidate,
    snapshot: SmartCheckInputSnapshot,
    capabilities: &ModelCapabilities,
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
    let context = over_exploring::build_jev_context(&snapshot.over_exploring_input()?)
        .map_err(InputLoadError::Preparation)?;
    let mut plan = OverExploringCheck
        .prepare_with_capabilities(&context, capabilities)
        .map_err(InputLoadError::Preparation)?;
    let sampling_overflow = bound_sampling_inventory(&mut plan);
    let evaluator_revision = CHECK.evaluator_revision();
    let bytes = serde_json::to_vec(&(
        snapshot.input_revision(),
        &context.input_revision,
        &evaluator_revision,
        capabilities,
    ))
    .map_err(InputLoadError::Serialization)?;
    let revision: String = StableId::new("over-exploring-execution-v1", &[&bytes]).into();
    Ok(PreparedInput {
        durable: BurnCheckInput {
            key: candidate.session.key.clone(),
            check_id: CHECK_ID.into(),
            incarnation: candidate.incarnation,
            source_generation: candidate.source_generation,
            source_fingerprint: candidate.source_fingerprint.clone(),
            activity_cursor: candidate.activity_cursor.clone(),
            published_fence: candidate.published_fence,
            input_revision: revision,
            evaluator_revision,
            boundary_at_epoch: candidate.boundary_at_epoch,
        },
        context,
        plan,
        snapshot_revision: snapshot.input_revision().to_owned(),
        sampling_overflow,
    })
}

fn load_input(
    store: &Store,
    candidate: &BurnCheckCandidate,
    capabilities: &ModelCapabilities,
) -> Result<Arc<PreparedInput>, InputLoadError> {
    let candidate = store
        .enrolled_burn_check_candidate(candidate, CHECK_ID)
        .map_err(InputLoadError::Storage)?;
    if !crate::smart_check_inputs::cache::source_is_current(
        store,
        &candidate,
        CHECK_ID,
        CHECK.evaluator_revision(),
    )
    .map_err(InputLoadError::Storage)?
    {
        return Err(InputLoadError::Unavailable(
            InputUnavailable::PublicationChanged,
        ));
    }
    let key = crate::smart_check_inputs::cache::source_key(
        &candidate,
        CHECK_ID,
        &CHECK.evaluator_revision(),
    )?;
    let key = serde_json::to_string(&(key, capabilities)).map_err(InputLoadError::Serialization)?;
    if let Some(input) = PREPARED_INPUTS
        .lock()
        .map_err(|_| InputLoadError::Preparation(JevError::InvalidCheckContext))?
        .get(store, &key)
    {
        return Ok(input);
    }
    let input = Arc::new(prepare(
        &candidate,
        store.load_smart_check_inputs(
            &candidate.session.key,
            candidate.published_fence,
            candidate.source_generation,
            DetectorInput::OverExploring,
        )?,
        capabilities,
    )?);
    let bytes = serde_json::to_vec(&(&input.context, &input.plan.prepared))
        .map_err(InputLoadError::Serialization)?
        .len()
        .saturating_mul(4);
    PREPARED_INPUTS
        .lock()
        .map_err(|_| InputLoadError::Preparation(JevError::InvalidCheckContext))?
        .insert(store, key, bytes, Arc::clone(&input));
    Ok(input)
}

fn bound_sampling_inventory(plan: &mut JevCheckPlan<PreparedAssessment>) -> usize {
    let overflow = plan
        .prepared
        .candidates
        .len()
        .saturating_sub(MAX_SAMPLING_CANDIDATES);
    if overflow > 0 {
        plan.prepared.candidates.truncate(MAX_SAMPLING_CANDIDATES);
        plan.coverage.not_selected_items = plan
            .prepared
            .candidates
            .len()
            .saturating_sub(plan.coverage.selected_items);
    }
    overflow
}

fn new_sampling() -> anyhow::Result<SamplingProgress> {
    SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: MAX_SAMPLING_CANDIDATES,
        answers_per_candidate: 1,
        judgments_per_run: MAX_TARGETS_PER_TURN,
    })
    .map_err(|error| anyhow::anyhow!("invalid sampling limits: {error:?}"))
}

fn restore_cursor(
    stored: Option<&BurnCheckAssessment>,
    input: &BurnCheckInput,
    provider_generation: u64,
) -> AssessmentCursor {
    stored
        .filter(|saved| saved.input_revision.as_deref() == Some(&input.input_revision))
        .and_then(|saved| serde_json::from_str::<AssessmentCursor>(&saved.progress_json).ok())
        .filter(|cursor| {
            cursor.revision == CURSOR_REVISION
                && cursor.input_revision == input.input_revision
                && cursor.provider_generation == provider_generation
        })
        .unwrap_or_else(|| AssessmentCursor {
            revision: CURSOR_REVISION,
            input_revision: input.input_revision.clone(),
            provider_generation,
            ..Default::default()
        })
}

fn unavailable(
    store: &Store,
    candidate: &BurnCheckCandidate,
    category: &str,
) -> anyhow::Result<()> {
    store.record_burn_check_candidate_failure_for_check(
        CHECK_ID,
        candidate,
        category == "unsupported_format",
        category,
        unix_now().saturating_add(POLICY.retry_delay_secs),
        unix_now(),
    )?;
    Ok(())
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
    if candidate.session.key.environment_key != "native"
        || !matches!(
            candidate.session.key.agent.as_str(),
            "opencode" | "codex" | "claude" | "claude-code" | "pi"
        )
    {
        return unavailable(store, candidate, "unsupported_format");
    }
    let capabilities = handle.resolve_capabilities(key_generation).await?;
    let preparation = admit_jev_orchestration().await?;
    let input_store = store.clone();
    let input_candidate = candidate.clone();
    let input_capabilities = capabilities.clone();
    let loaded = tauri::async_runtime::spawn_blocking(move || {
        let _permit = preparation;
        load_input(&input_store, &input_candidate, &input_capabilities)
    })
    .await?;
    let input = match loaded {
        Ok(input) => input,
        Err(error) if error.is_stale() => return Ok(()),
        Err(error) => {
            return unavailable(store, candidate, error.failure_category());
        }
    };
    if input.plan.prepared.candidates.is_empty() {
        if store.record_burn_check_no_candidates(
            &input.durable,
            "{}",
            unix_now(),
            POLICY.idle_secs,
        )? {
            let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
        }
        return Ok(());
    }
    let stored = store.burn_check_assessment(&candidate.session.key, CHECK_ID)?;
    let mut cursor = restore_cursor(stored.as_ref(), &input.durable, key_generation);
    if cursor.sampling.is_none() {
        cursor.sampling = Some(new_sampling()?);
    }
    cursor.inventory_total = input.plan.prepared.candidates.len();
    cursor.inventory_count = cursor
        .inventory_count
        .saturating_add(256)
        .min(cursor.inventory_total);
    synchronize_sampling_prefix(
        &input.plan,
        cursor.inventory_count,
        cursor.sampling.as_mut().expect("initialized"),
    )
    .map_err(|error| anyhow::anyhow!("sampling inventory rejected: {error:?}"))?;
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
    if cursor.result.is_none() {
        cursor.result = Some(OverExploringCheck.reduce(&input.plan, &[], false)?);
        cursor.result.as_mut().expect("initialized").unassessed =
            input.plan.prepared.unassessed.clone();
    }
    if !cursor.run_started || cursor.run_finished {
        cursor.sampling.as_mut().expect("initialized").begin_run();
        cursor.run_started = true;
        cursor.run_finished = false;
    }
    loop {
        let job = match cursor
            .active_job
            .clone()
            .or_else(|| cursor.sampling.as_mut().expect("initialized").choose_job())
        {
            Some(job) => job,
            None => {
                cursor.run_finished = true;
                break;
            }
        };
        cursor.active_job = Some(job.clone());
        if !save_cursor(store, &input.durable, &cursor)? {
            return Ok(());
        }
        let orchestration = admit_jev_orchestration().await?;
        let selected_input = Arc::clone(&input);
        let selected_job = job.clone();
        let (mut plan, checkpoint_plan) = tauri::async_runtime::spawn_blocking(move || {
            let mut selected_plan = selected_input.plan.clone();
            PreparedAssessment::select_jobs(
                &mut selected_plan,
                std::slice::from_ref(&selected_job),
            )?;
            Ok::<_, JevError>((selected_plan.clone(), selected_plan))
        })
        .await??;
        let readiness = crate::scope_creep_worker::dispatch_readiness(
            store,
            handle,
            &input.durable,
            &capabilities,
            &plan,
            &cursor.run_progress,
        )?;
        if readiness == crate::store::BurnCheckRequestAdmission::Stale {
            return Ok(());
        }
        let terminal = matches!(
            readiness,
            crate::store::BurnCheckRequestAdmission::Exhausted
                | crate::store::BurnCheckRequestAdmission::Unresolved
        );
        let deferred = readiness == crate::store::BurnCheckRequestAdmission::Deferred;
        if terminal {
            if !plan.skipped_item_ids.is_empty() {
                cursor
                    .context_blocked_ids
                    .extend(plan.skipped_item_ids.iter().cloned());
            }
            let reviewed = OverExploringCheck.reduce(&plan, &[], false)?;
            merge_result(
                cursor.result.as_mut().expect("initialized"),
                reviewed,
                &plan,
            );
            cursor
                .sampling
                .as_mut()
                .expect("initialized")
                .terminate_candidate(&job)
                .map_err(|error| anyhow::anyhow!("sampling termination rejected: {error:?}"))?;
            cursor.active_job = None;
            cursor.run_progress = JevRunProgress::default();
            if !save_cursor(store, &input.durable, &cursor)? {
                return Ok(());
            }
            continue;
        }
        if deferred {
            save_failure(
                store,
                &input,
                &cursor,
                "continuing",
                store.burn_check_next_attempt_at(&input.durable)?,
            )?;
            return Ok(());
        }
        let progress = std::mem::take(&mut cursor.run_progress);
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
            &OverExploringCheck,
            &input.context,
            &mut plan,
            progress,
            orchestration,
            |progress| {
                cursor.run_progress = progress.clone();
                let reviewed = OverExploringCheck.reduce(
                    &checkpoint_plan,
                    &progress.results.values().cloned().collect::<Vec<_>>(),
                    false,
                )?;
                if checkpoint_plan
                    .work_items
                    .iter()
                    .all(|item| reviewed.completed_work_item_ids.contains(&item.id))
                    && !cursor
                        .sampling
                        .as_ref()
                        .expect("initialized")
                        .completed_ids(check_identity())
                        .contains(&job.candidate)
                {
                    checkpoint_plan
                        .prepared
                        .record_completion(
                            &reviewed,
                            &job,
                            cursor.sampling.as_mut().expect("initialized"),
                        )
                        .map_err(|_| JevError::ProgressStorageFailure)?;
                }
                merge_result(
                    cursor.result.as_mut().expect("initialized"),
                    reviewed,
                    &checkpoint_plan,
                );
                save_scheduling(store, &input.durable, &cursor)
                    .map_err(|_| JevError::ProgressStorageFailure)?;
                serde_json::to_string(&cursor).map_err(|_| JevError::ProgressStorageFailure)
            },
        )
        .await?;
        if !handle.key_is_current(key_generation) {
            store.supersede_burn_check_assessment(&input.durable, unix_now())?;
            return Ok(());
        }
        cursor.run_progress = outcome.progress;
        merge_result(
            cursor.result.as_mut().expect("initialized"),
            outcome.result.clone(),
            &plan,
        );
        if let Some(error) = outcome.failure {
            if matches!(error, JevError::Cancelled)
                && handle.turn_exhausted(CHECK_ID, &candidate.session.key)
            {
                save_failure(store, &input, &cursor, "continuing", None)?;
                return Ok(());
            }
            if matches!(
                error_category(&error),
                "outcome_unknown"
                    | "invalid_response"
                    | "response_too_large"
                    | "response_decode"
                    | "response_usage_exceeded"
                    | "invalid_request"
            ) {
                if matches!(
                    error,
                    JevError::RequestTooLarge { .. } | JevError::RequestTokenLimitExceeded { .. }
                ) {
                    cursor
                        .context_blocked_ids
                        .extend(plan.work_items.iter().map(|item| item.id.clone()));
                }
                let sampling = cursor.sampling.as_mut().expect("initialized");
                if !sampling
                    .completed_ids(check_identity())
                    .contains(&job.candidate)
                {
                    sampling.terminate_candidate(&job).map_err(|error| {
                        anyhow::anyhow!("sampling termination rejected: {error:?}")
                    })?;
                }
                cursor.active_job = None;
                cursor.run_progress = JevRunProgress::default();
                if !save_cursor(store, &input.durable, &cursor)? {
                    return Ok(());
                }
                continue;
            }
            let rejected = matches!(error, JevError::AuthenticationRejected);
            let saved = handle
                .with_current_generation(key_generation, || {
                    let published = save_failure(
                        store,
                        &input,
                        &cursor,
                        if matches!(error, JevError::ProviderUnavailable)
                            && store.burn_check_next_attempt_at(&input.durable)?.is_none()
                        {
                            "continuing"
                        } else {
                            error_category(&error)
                        },
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
        if !plan.work_items.is_empty()
            && !cursor
                .sampling
                .as_ref()
                .expect("initialized")
                .completed_ids(check_identity())
                .contains(&job.candidate)
            && plan
                .work_items
                .iter()
                .all(|item| outcome.result.completed_work_item_ids.contains(&item.id))
        {
            plan.prepared
                .record_completion(
                    &outcome.result,
                    &job,
                    cursor.sampling.as_mut().expect("initialized"),
                )
                .map_err(|error| anyhow::anyhow!("sampling completion rejected: {error:?}"))?;
        }
        cursor.active_job = None;
        cursor.run_progress = JevRunProgress::default();
        if !save_cursor(store, &input.durable, &cursor)? {
            return Ok(());
        }
    }
    let coverage = cursor
        .sampling
        .as_ref()
        .expect("initialized")
        .coverage(check_identity())
        .expect("synchronized");
    let result = cursor.result.as_mut().expect("initialized");
    result.coverage.selected_items = coverage.completed;
    let runnable = cursor
        .sampling
        .as_ref()
        .expect("initialized")
        .runnable_count(check_identity());
    result.coverage.not_selected_items = runnable
        + cursor
            .inventory_total
            .saturating_sub(cursor.inventory_count);
    result.coverage.skipped_items = coverage
        .eligible
        .saturating_sub(coverage.completed + runnable);
    result.coverage.limitations = result
        .unassessed
        .iter()
        .map(|item| format!("{:?}", item.limitation))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
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

fn record_assessment(
    app: &tauri::AppHandle,
    historical: bool,
    outcome: crate::analytics::event::SmartCheckAssessmentOutcome,
) {
    crate::analytics::record_smart_check_lifecycle(
        app,
        crate::analytics::event::SmartCheckLifecycle::Assessment {
            check: crate::analytics::event::SmartCheck::OverExploring,
            historical,
            outcome,
        },
    );
}

fn publish_current(
    store: &Store,
    candidate: &BurnCheckCandidate,
    capabilities: &ModelCapabilities,
    input: &PreparedInput,
    cursor: &AssessmentCursor,
) -> anyhow::Result<Option<crate::analytics::event::SmartCheckAssessmentOutcome>> {
    if !Store::burn_check_input_is_current(&store.lock(), &input.durable)?
        || store
            .enrolled_burn_check_candidate(candidate, CHECK_ID)?
            .boundary_positions
            != candidate.boundary_positions
        || capabilities != &input.plan.capabilities
    {
        store.supersede_burn_check_assessment(&input.durable, unix_now())?;
        return Ok(None);
    }
    if !save_cursor(store, &input.durable, cursor)? {
        return Ok(None);
    }
    let publication = publication(input, cursor.result.as_ref().expect("initialized").clone());
    let clean = publication_has_clean_coverage(&publication.assessment);
    let has_finding = publication
        .assessment
        .findings
        .iter()
        .any(|finding| publishable_finding(finding, &publication));
    let published =
        if clean {
            store.complete_burn_check_assessment(
                &input.durable,
                &serde_json::to_string(&publication)?,
                unix_now(),
                POLICY.idle_secs,
            )?
        } else {
            store.fail_burn_check_assessment_with_result(
                &input.durable,
                &BurnCheckFailure {
                    error_category: if cursor.inventory_count < cursor.inventory_total
                        || cursor.sampling.as_ref().is_some_and(|sampling| {
                            sampling.has_runnable_candidates(check_identity())
                        }) {
                        "continuing"
                    } else {
                        "sampling_incomplete"
                    },
                    result_json: &serde_json::to_string(&publication)?,
                    progress_json: &serde_json::to_string(&cursor)?,
                    retry_at_epoch: None,
                },
                unix_now(),
                POLICY.idle_secs,
            )?
        };
    Ok(published.then(|| {
        crate::analytics::event::SmartCheckAssessmentOutcome::from_evidence(has_finding, clean)
    }))
}

fn check_identity() -> StableId {
    StableId::new("smart-check", &[b"over_exploring"])
}

#[cfg(test)]
fn synchronize_sampling(
    plan: &JevCheckPlan<PreparedAssessment>,
    sampling: &mut SamplingProgress,
) -> Result<(), antiburn_local::checks::sampling::SamplingError> {
    synchronize_sampling_prefix(plan, plan.prepared.candidates.len(), sampling)
}

fn synchronize_sampling_prefix(
    plan: &JevCheckPlan<PreparedAssessment>,
    count: usize,
    sampling: &mut SamplingProgress,
) -> Result<(), antiburn_local::checks::sampling::SamplingError> {
    let candidates = plan
        .prepared
        .candidates
        .iter()
        .take(count)
        .map(|candidate| antiburn_local::checks::sampling::Candidate {
            id: candidate.candidate_id,
            required_answers: candidate.required_answers.clone(),
        })
        .collect::<Vec<_>>();
    let mut chronological = plan
        .prepared
        .candidates
        .iter()
        .take(count)
        .collect::<Vec<_>>();
    chronological.sort_by_key(|candidate| {
        let source_index = candidate
            .work_item_ids
            .iter()
            .filter_map(|id| plan.prepared.targets.get(id))
            .flat_map(|target| target.read_indexes.iter().copied())
            .min()
            .unwrap_or(usize::MAX);
        (source_index, candidate.candidate_id)
    });
    sampling.synchronize_ordered(
        check_identity(),
        plan.prepared.epoch,
        &candidates,
        &chronological
            .iter()
            .map(|candidate| candidate.candidate_id)
            .collect::<Vec<_>>(),
    )
}

fn save_cursor(
    store: &Store,
    input: &BurnCheckInput,
    cursor: &AssessmentCursor,
) -> anyhow::Result<bool> {
    if !save_scheduling(store, input, cursor)? {
        return Ok(false);
    }
    store.save_burn_check_checkpoint(
        input,
        &serde_json::to_string(cursor)?,
        None,
        unix_now(),
        POLICY.lease_secs,
        POLICY.idle_secs,
    )
}

fn save_scheduling(
    store: &Store,
    input: &BurnCheckInput,
    cursor: &AssessmentCursor,
) -> anyhow::Result<bool> {
    if let Some(sampling) = &cursor.sampling {
        let coverage = sampling.coverage(check_identity()).expect("synchronized");
        let missing = cursor.result.as_ref().map_or(0, |result| {
            result
                .unassessed
                .iter()
                .filter(|item| item.work_item_id.is_none())
                .count()
        });
        let unenumerated = cursor
            .inventory_total
            .saturating_sub(cursor.inventory_count);
        if !store.save_burn_check_scheduling(
            input,
            (unenumerated == 0).then_some(coverage.eligible + missing),
            coverage.completed,
            sampling.runnable_count(check_identity()) + unenumerated,
        )? {
            return Ok(false);
        }
    }
    Ok(true)
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
        .clone()
        .ok_or_else(|| anyhow::anyhow!("assessment result is missing"))?;
    store.fail_burn_check_assessment_with_result(
        &input.durable,
        &BurnCheckFailure {
            error_category: category,
            result_json: &serde_json::to_string(&publication(input, result))?,
            progress_json: &serde_json::to_string(cursor)?,
            retry_at_epoch: retry,
        },
        unix_now(),
        POLICY.idle_secs,
    )
}

fn merge_result(
    target: &mut Assessment,
    page: Assessment,
    plan: &JevCheckPlan<PreparedAssessment>,
) {
    let selected: BTreeSet<_> = plan
        .work_items
        .iter()
        .map(|item| item.id.as_str())
        .chain(plan.skipped_item_ids.iter().map(String::as_str))
        .collect();
    let accepted: BTreeSet<_> = page
        .completed_work_item_ids
        .iter()
        .map(String::as_str)
        .collect();
    target
        .findings
        .retain(|finding| !accepted.contains(finding.work_item_id.as_str()));
    target.findings.extend(page.findings);
    target.unassessed.retain(|item| {
        item.work_item_id.as_deref().is_none_or(|id| {
            !accepted.contains(id)
                && (!selected.contains(id)
                    || target
                        .completed_work_item_ids
                        .iter()
                        .any(|saved| saved == id))
        })
    });
    target
        .unassessed
        .extend(page.unassessed.into_iter().filter(|item| {
            item.work_item_id.as_deref().is_some_and(|id| {
                selected.contains(id)
                    && (accepted.contains(id)
                        || !target
                            .completed_work_item_ids
                            .iter()
                            .any(|saved| saved == id))
            })
        }));
    target
        .completed_work_item_ids
        .extend(page.completed_work_item_ids);
    target.completed_work_item_ids.sort();
    target.completed_work_item_ids.dedup();
    target.completed_episode_ids.clear();
    target.clean_episode_ids.clear();
    for episode in plan.prepared.episodes.iter() {
        let ids: Vec<_> = plan
            .prepared
            .targets
            .iter()
            .filter(|(_, item)| item.episode_id == episode.id)
            .map(|(id, _)| id)
            .collect();
        if !ids.is_empty()
            && ids
                .iter()
                .all(|id| target.completed_work_item_ids.contains(id))
        {
            target.completed_episode_ids.push(episode.id);
            if !target
                .findings
                .iter()
                .any(|item| item.episode_id == episode.id)
                && !target
                    .unassessed
                    .iter()
                    .any(|item| item.episode_id == episode.id)
            {
                target.clean_episode_ids.push(episode.id);
            }
        }
    }
}

pub(crate) fn publication(input: &PreparedInput, mut assessment: Assessment) -> Publication {
    if input.sampling_overflow > 0 {
        assessment.coverage.not_selected_items += input.sampling_overflow;
        assessment.coverage.processing_limit_reached = true;
        assessment
            .coverage
            .limitations
            .push("sampling_inventory_limit".into());
    }
    let original_task_evidence = input
        .plan
        .prepared
        .task_contexts
        .values()
        .flat_map(|context| context.evidence.clone())
        .collect::<Vec<_>>();
    let valid_task = |finding: &Decision, reference: &JevEvidenceReference| {
        if original_task_evidence.iter().any(|original| {
            original.source_id == reference.source_id
                && original.content_kind == reference.content_kind
                && original.role == reference.role
        }) {
            return true;
        }
        let Some(index) = reference
            .part_id
            .strip_prefix("task[")
            .and_then(|value| value.strip_suffix(']'))
            .and_then(|value| value.parse::<usize>().ok())
        else {
            return false;
        };
        if reference.part_id != format!("task[{index}]") {
            return false;
        }
        let Some(original) = input
            .plan
            .prepared
            .task_contexts
            .get(&finding.episode_id)
            .and_then(|context| {
                context.evidence.iter().find(|original| {
                    original.source_id == reference.source_id && original.role == reference.role
                })
            })
        else {
            return false;
        };
        original.role == antiburn_local::analysis::jev::JevEvidenceRole::Instruction
            && reference.role == original.role
            && finding.explanation.as_ref().is_some_and(|basis| {
                basis.version == 1
                    && basis.task.evidence.iter().any(|task| {
                        task.source_id == original.source_id && task.role == original.role
                    })
                    && basis.snippets.iter().any(|snippet| {
                        snippet.reference.id == reference.source_id
                            && snippet.reference.stable
                            && !snippet.source_digest.is_empty()
                            && snippet.range.0 < snippet.range.1
                            && snippet.range.1 - snippet.range.0 == snippet.text.len()
                    })
            })
    };
    let mut task_evidence = original_task_evidence.clone();
    task_evidence.extend(
        assessment
            .findings
            .iter()
            .flat_map(|finding| finding.task_evidence.iter())
            .filter(|reference| {
                assessment.findings.iter().any(|finding| {
                    finding.task_evidence.contains(reference) && valid_task(finding, reference)
                })
            })
            .cloned(),
    );
    task_evidence.sort_by(|left, right| left.source_id.cmp(&right.source_id));
    task_evidence.dedup();
    Publication {
        input_revision: input.durable.input_revision.clone(),
        snapshot_revision: input.snapshot_revision.clone(),
        semantic_revision: input.context.input_revision.clone(),
        model: input.plan.capabilities.model.clone(),
        task_evidence,
        work_task_evidence: assessment
            .findings
            .iter()
            .map(|finding| {
                (
                    finding.work_item_id.clone(),
                    finding
                        .task_evidence
                        .iter()
                        .filter(|reference| {
                            input
                                .plan
                                .prepared
                                .task_contexts
                                .get(&finding.episode_id)
                                .is_some_and(|context| {
                                    context.evidence.iter().any(|original| {
                                        original.source_id == reference.source_id
                                            && original.role == reference.role
                                            && (original.content_kind == reference.content_kind
                                                || (reference.part_id.starts_with("task[")
                                                    && original.role == antiburn_local::analysis::jev::JevEvidenceRole::Instruction))
                                    })
                                })
                        })
                        .cloned()
                        .collect(),
                )
            })
            .collect(),
        work_targets: input
            .plan
            .prepared
            .targets
            .iter()
            .filter(|(id, _)| {
                assessment
                    .findings
                    .iter()
                    .any(|finding| &finding.work_item_id == *id)
            })
            .map(|(id, target)| (id.clone(), target.clone()))
            .collect(),
        work_evidence: assessment
            .findings
            .iter()
            .map(|finding| {
                (
                    finding.work_item_id.clone(),
                    finding
                        .task_evidence
                        .iter()
                        .chain(&finding.source_evidence)
                        .cloned()
                        .collect(),
                )
            })
            .collect(),
        assessment,
    }
}

pub(crate) fn publication_has_clean_coverage(result: &Assessment) -> bool {
    let completed: BTreeSet<_> = result.completed_work_item_ids.iter().collect();
    result.coverage.selected_items > 0
        && result.coverage.not_selected_items == 0
        && result.coverage.skipped_items == 0
        && !result.coverage.processing_limit_reached
        && result.coverage.limitations.is_empty()
        && result.unassessed.is_empty()
        && completed.len() == result.coverage.selected_items
        && completed.len() == result.completed_work_item_ids.len()
        && result.findings.iter().all(|finding| {
            result
                .completed_work_item_ids
                .contains(&finding.work_item_id)
        })
}

pub(crate) fn publishable_finding(finding: &Decision, publication: &Publication) -> bool {
    let Some(evidence) = publication.work_evidence.get(&finding.work_item_id) else {
        return false;
    };
    let Some(target) = publication.work_targets.get(&finding.work_item_id) else {
        return false;
    };
    let bindings: BTreeSet<_> = finding.reads.iter().map(|read| &read.request_id).collect();
    finding.semantic_revision == publication.semantic_revision
        && finding.model == publication.model
        && finding.revisions == OverExploringCheck.revisions()
        && finding.outcome == SemanticOutcome::LikelyExcess
        && finding.probability.is_finite()
        && (SEMANTIC_PROBABILITY_THRESHOLD..=1.0).contains(&finding.probability)
        && !finding.task_evidence.is_empty()
        && publication.work_task_evidence.get(&finding.work_item_id) == Some(&finding.task_evidence)
        && finding
            .task_evidence
            .iter()
            .all(|reference| publication.task_evidence.contains(reference))
        && finding.reads == target.bindings
        && finding.reason == target.reason
        && finding.episode_id == target.episode_id
        && finding
            .task_evidence
            .iter()
            .chain(&finding.source_evidence)
            .eq(evidence.iter())
        && !finding.reads.is_empty()
        && bindings.len() == finding.reads.len()
        && finding.reads.iter().all(|read| !read.request_id.is_empty())
}

#[cfg(test)]
pub(crate) mod tests;
