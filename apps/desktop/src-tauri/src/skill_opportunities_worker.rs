//! Feature-owned preparation and publication checks for skill opportunities.

use std::sync::{Arc, LazyLock, Mutex};

use antiburn_local::analysis::SourceFormat;
#[cfg(test)]
use antiburn_local::analysis::jev::capabilities::ModelCapabilities;
use antiburn_local::analysis::jev::{
    JevCheck, JevCheckPlan, JevError, JevExecutionOutcome, JevOrchestrationPermit, JevRunProgress,
    admit_jev_orchestration,
};
use antiburn_local::checks::sampling::{SamplingJob, SamplingLimits, SamplingProgress};
use antiburn_local::checks::skill_opportunities::{
    SKILL_OPPORTUNITIES_CHECK_ID, SKILL_OPPORTUNITIES_REVISIONS, SkillDescriptorInventory,
    SkillOpportunitiesCheck, SkillOpportunitiesResult, SkillOpportunityFinding,
};
use antiburn_local::model::AgentKind;
use tauri::Emitter;

use crate::jev::worker::{
    BatchExecution, CandidateExecution, CheckPolicy, JevCheckDescriptor, WorkerFuture,
    WorkerHandle, error_category, run_prepared_check, unix_now,
};
use crate::smart_check_inputs::{InputLoadError, InventoryRevisionObserver, SkillInputs};
use crate::store::{
    BurnCheckCandidate, BurnCheckFailure, BurnCheckInput, BurnCheckSampledPair, Store,
};

const POLICY: CheckPolicy = CheckPolicy {
    idle_secs: 180,
    lease_secs: 300,
    retry_delay_secs: 300,
};
const MAX_SAMPLE_JUDGMENTS: usize = 4;
const MAX_SAMPLE_CANDIDATES: usize = 4096;
const MAX_SAMPLE_ANSWERS: usize = 1;
const MAX_SAMPLE_CHECKS: usize = 1;
const CURSOR_REVISION: u32 = 8;

static INPUT_OBSERVATIONS: LazyLock<Mutex<InventoryRevisionObserver>> =
    LazyLock::new(|| Mutex::new(InventoryRevisionObserver::default()));

static PREPARED_INPUTS: LazyLock<
    Mutex<crate::smart_check_inputs::cache::PreparedInputCache<PreparedSkillOpportunityInput>>,
> = LazyLock::new(|| Mutex::new(Default::default()));

struct CachedInputRevision {
    _store: Store,
    store_identity: usize,
    source_key: String,
    config: crate::agent_config::ConfigContext,
    input_revision: String,
    inventory_revision: String,
}

static INPUT_REVISIONS: LazyLock<Mutex<std::collections::VecDeque<CachedInputRevision>>> =
    LazyLock::new(|| Mutex::new(std::collections::VecDeque::new()));

fn cache_input_revision(revision: CachedInputRevision) -> anyhow::Result<()> {
    let mut cache = INPUT_REVISIONS
        .lock()
        .map_err(|_| anyhow::anyhow!("skill revision cache lock failed"))?;
    cache.retain(|cached| {
        cached.store_identity != revision.store_identity
            || cached.source_key != revision.source_key
            || cached.config != revision.config
    });
    if cache.len() == InventoryRevisionObserver::MAX_INPUTS {
        cache.pop_front();
    }
    cache.push_back(revision);
    Ok(())
}

fn revision_source_key(candidate: &BurnCheckCandidate) -> Result<String, serde_json::Error> {
    serde_json::to_string(&(
        (
            &candidate.session.key.environment_key,
            &candidate.session.key.agent,
            &candidate.session.key.session_id,
        ),
        candidate.incarnation,
        candidate.source_generation,
        &candidate.source_fingerprint,
        &candidate.activity_cursor,
        candidate.published_fence,
        &candidate.boundary_positions,
        candidate.boundary_at_epoch,
        candidate.historical,
        &candidate.session.cwd,
        SKILL_OPPORTUNITIES_CHECK_ID,
        CHECK.evaluator_revision(),
        (
            antiburn_local::analysis::PARSER_REVISION,
            antiburn_local::analysis::ANALYZER_REVISION,
            antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
        ),
    ))
}

pub(crate) struct SkillOpportunitiesDescriptor;
pub(crate) const CHECK: SkillOpportunitiesDescriptor = SkillOpportunitiesDescriptor;

impl JevCheckDescriptor for SkillOpportunitiesDescriptor {
    fn id(&self) -> &'static str {
        SKILL_OPPORTUNITIES_CHECK_ID
    }

    fn evaluator_revision(&self) -> String {
        let revisions = SKILL_OPPORTUNITIES_REVISIONS;
        format!(
            "skill-opportunities-adapter-v{CURSOR_REVISION}:p{}:c{}:q{}:r{}",
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

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct SkillCursor {
    reviewed_before_page: usize,
    uncertain_before_page: usize,
    skipped_before_page: usize,
    context_blocked_before_page: usize,
    context_blocked_ids: std::collections::BTreeSet<String>,
    revision: u32,
    engine_revisions: antiburn_local::analysis::jev::JevCheckRevisions,
    input_revision: String,
    provider_generation: u64,
    connection_context: String,
    sampling: Option<SamplingProgress>,
    active_job: Option<SamplingJob>,
    batch_jobs: Vec<SamplingJob>,
    active_plan: Option<
        JevCheckPlan<antiburn_local::checks::skill_opportunities::PreparedSkillOpportunities>,
    >,
    run_progress: JevRunProgress,
    result: Option<SkillOpportunitiesResult>,
    inventory: SkillDescriptorInventory,
}

impl Default for SkillCursor {
    fn default() -> Self {
        Self {
            reviewed_before_page: 0,
            uncertain_before_page: 0,
            skipped_before_page: 0,
            context_blocked_before_page: 0,
            context_blocked_ids: Default::default(),
            revision: CURSOR_REVISION,
            engine_revisions: SKILL_OPPORTUNITIES_REVISIONS,
            input_revision: String::new(),
            provider_generation: 0,
            connection_context: String::new(),
            sampling: None,
            active_job: None,
            batch_jobs: Vec::new(),
            active_plan: None,
            run_progress: JevRunProgress::default(),
            result: None,
            inventory: SkillDescriptorInventory::default(),
        }
    }
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
    let enrolled = store.enrolled_burn_check_candidate(candidate, SKILL_OPPORTUNITIES_CHECK_ID)?;
    let candidate = &enrolled;
    let Some(home) = antiburn_local::paths::home_dir() else {
        unavailable(store, candidate, "home_unavailable", handle, key_generation)?;
        return Ok(());
    };
    let Some(agent) = AgentKind::from_slug(&candidate.session.key.agent) else {
        unavailable(
            store,
            candidate,
            "unsupported_format",
            handle,
            key_generation,
        )?;
        return Ok(());
    };
    if !matches!(
        agent,
        AgentKind::OpenCode | AgentKind::Codex | AgentKind::Claude | AgentKind::Pi
    ) || candidate.session.key.environment_key != "native"
    {
        unavailable(
            store,
            candidate,
            "unsupported_format",
            handle,
            key_generation,
        )?;
        return Ok(());
    }
    let cwd = candidate
        .session
        .cwd
        .as_deref()
        .map(std::path::Path::new)
        .map(std::path::Path::canonicalize)
        .transpose();
    let cwd = match cwd {
        Ok(cwd) => cwd,
        Err(_) => {
            unavailable(
                store,
                candidate,
                "workspace_unavailable",
                handle,
                key_generation,
            )?;
            return Ok(());
        }
    };
    let config = crate::agent_config::ConfigContext::native(agent, home, cwd);
    let preparation = admit_jev_orchestration().await?;
    let input_store = store.clone();
    let input_candidate = candidate.clone();
    let input_config = config.clone();
    let loaded = tauri::async_runtime::spawn_blocking(move || {
        let _permit = preparation;
        let inventory =
            crate::smart_check_inputs::inventory_cache::discover_inventory(&input_config)
                .map_err(InputLoadError::Inventory)?;
        load_input(&input_store, &input_candidate, &input_config, inventory)
    })
    .await?;
    let input = match loaded {
        Ok(input) => input,
        Err(error) if error.is_stale() => return Ok(()),
        Err(error) => {
            let category = error.failure_category();
            unavailable(store, candidate, category, handle, key_generation)?;
            return Ok(());
        }
    };
    let Some(observation) = observe_skill_input(
        store,
        handle,
        key_generation,
        candidate,
        Some(input.durable.input_revision.clone()),
    )?
    else {
        return Ok(());
    };
    if observation.invalidated {
        let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
    }
    if observation.input_revision.as_deref() != Some(&input.durable.input_revision) {
        return Ok(());
    }
    let write_fence = SkillWriteFence {
        store,
        handle,
        provider_generation: key_generation,
        input_generation: observation.generation,
        input: &input,
        candidate,
        config: &config,
    };
    let stored =
        store.burn_check_assessment(&candidate.session.key, SKILL_OPPORTUNITIES_CHECK_ID)?;
    let mut cursor = restore_cursor(
        stored.as_ref(),
        &input.durable.input_revision,
        key_generation,
    );
    let capabilities = handle.resolve_capabilities(key_generation).await?;
    let connection_context =
        serde_json::to_string(&(handle.system_one_connection(), &capabilities))?;
    if !cursor.connection_context.is_empty() && cursor.connection_context != connection_context {
        cursor = SkillCursor {
            input_revision: input.durable.input_revision.clone(),
            provider_generation: key_generation,
            ..Default::default()
        };
    }
    cursor.connection_context = connection_context;
    if cursor.input_revision != input.durable.input_revision {
        cursor = SkillCursor {
            revision: CURSOR_REVISION,
            input_revision: input.durable.input_revision.clone(),
            provider_generation: key_generation,
            sampling: Some(new_sampling_progress()?),
            ..SkillCursor::default()
        };
    }
    cursor.sampling.get_or_insert(new_sampling_progress()?);
    enumerate_skill_turn(&input, &mut cursor)?;
    if cursor.inventory.complete
        && !cursor
            .sampling
            .as_ref()
            .and_then(|sampling| sampling.coverage(input.check.sampling_identity()))
            .is_some_and(|coverage| {
                coverage.eligible > 0
                    || cursor.reviewed_before_page + cursor.skipped_before_page > 0
            })
    {
        write_fence.commit(|| record_no_candidates(store, &input, &cursor))?;
        let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
        return Ok(());
    }
    if !write_fence
        .commit(|| {
            if !store.queue_burn_check_assessment(&input.durable, unix_now(), POLICY.idle_secs)? {
                return Ok(false);
            }
            store.claim_burn_check_assessment(
                &input.durable,
                unix_now(),
                POLICY.lease_secs,
                POLICY.idle_secs,
            )
        })?
        .unwrap_or(false)
    {
        return Ok(());
    }
    save_scheduling(store, &input, &cursor)?;
    if cursor.active_job.is_none() {
        cursor
            .sampling
            .as_mut()
            .expect("sampling was initialized")
            .begin_run();
    }
    let mut sampled_pairs = Vec::new();
    loop {
        let job = match cursor.active_job.clone() {
            Some(job) => job,
            None => match cursor
                .sampling
                .as_mut()
                .expect("sampling was initialized")
                .choose_job()
            {
                Some(job) => {
                    cursor.active_job = Some(job.clone());
                    job
                }
                None => break,
            },
        };
        let mut jobs = vec![job];
        jobs.extend(cursor.batch_jobs.clone());
        if cursor.run_progress == JevRunProgress::default() {
            while jobs.len() < 4 {
                let Some(job) = cursor
                    .sampling
                    .as_mut()
                    .expect("sampling was initialized")
                    .choose_job()
                else {
                    break;
                };
                jobs.push(job);
            }
            cursor.batch_jobs = jobs.iter().skip(1).cloned().collect();
        }
        let orchestration = admit_jev_orchestration().await?;
        let mut plan = match cursor.active_plan.clone() {
            Some(plan) => plan,
            None => {
                let check = Arc::clone(&input.check);
                let inventory = cursor.inventory.clone();
                let capabilities = capabilities.clone();
                let jobs = jobs.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    check.prepare_inventory_sampled(
                        &inventory,
                        &check.session_context(),
                        &capabilities,
                        &jobs,
                    )
                })
                .await??
            }
        };
        let mut blocked = Vec::new();
        let mut deferred = false;
        for job in &jobs {
            if let Some(result) = &cursor.result
                && plan.prepared.comparisons.iter().any(|comparison| {
                    antiburn_local::checks::sampling::StableId::new(
                        "skill-opportunities",
                        &[comparison.id.as_bytes()],
                    ) == job.candidate
                        && result.decisions.iter().any(|decision| {
                            decision.comparison == *comparison
                                && decision.judgments.is_some()
                                && decision.model.as_deref() == Some(capabilities.model.as_str())
                        })
                })
            {
                input
                    .check
                    .record_sampling_result(
                        cursor.sampling.as_mut().expect("sampling was initialized"),
                        job,
                        result,
                    )
                    .map_err(|error| anyhow::anyhow!("sampling reuse rejected: {error:?}"))?;
                blocked.push(job.candidate);
                continue;
            }
            let mut target = plan.clone();
            target.work_items.retain(|item| {
                antiburn_local::checks::sampling::StableId::new(
                    "skill-opportunities",
                    &[item.id.as_bytes()],
                ) == job.candidate
            });
            match crate::scope_creep_worker::dispatch_readiness(
                store,
                handle,
                &input.durable,
                &capabilities,
                &target,
                &cursor.run_progress,
            )? {
                crate::store::BurnCheckRequestAdmission::Exhausted
                | crate::store::BurnCheckRequestAdmission::Unresolved => {
                    if target.work_items.is_empty()
                        || !antiburn_local::analysis::jev::pack_work_items_with_capabilities(
                            &target.work_items,
                            &capabilities,
                        )
                        .skipped_item_ids
                        .is_empty()
                    {
                        cursor
                            .context_blocked_ids
                            .insert(serde_json::to_string(&job.candidate)?);
                    }
                    cursor
                        .sampling
                        .as_mut()
                        .expect("sampling was initialized")
                        .terminate_candidate(job)
                        .map_err(|error| {
                            anyhow::anyhow!("skill termination rejected: {error:?}")
                        })?;
                    blocked.push(job.candidate);
                }
                crate::store::BurnCheckRequestAdmission::Deferred => deferred = true,
                crate::store::BurnCheckRequestAdmission::Stale => return Ok(()),
                crate::store::BurnCheckRequestAdmission::Admitted => {}
            }
        }
        jobs.retain(|job| !blocked.contains(&job.candidate));
        cursor.active_job = jobs.first().cloned();
        cursor.batch_jobs = jobs.iter().skip(1).cloned().collect();
        input.check.retain_plan_jobs(&mut plan, &jobs)?;
        cursor
            .run_progress
            .results
            .retain(|id, _| plan.work_items.iter().any(|item| &item.id == id));
        cursor.active_plan = Some(plan.clone());
        write_fence.commit(|| {
            save_scheduling(store, &input, &cursor)?;
            store.save_burn_check_checkpoint(
                &input.durable,
                &serde_json::to_string(&cursor)?,
                Some(&cursor.run_progress),
                unix_now(),
                POLICY.lease_secs,
                POLICY.idle_secs,
            )
        })?;
        if deferred {
            write_fence.commit(|| {
                store.release_failed_burn_check_lease(
                    &input.durable,
                    "continuing",
                    store
                        .burn_check_next_attempt_at(&input.durable)?
                        .unwrap_or(unix_now() + 1),
                )
            })?;
            return Ok(());
        }
        if jobs.is_empty() {
            cursor.run_progress = JevRunProgress::default();
            cursor.active_plan = None;
            continue;
        }
        let outcome = run_prepared(
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
            &input,
            &mut plan,
            std::mem::take(&mut cursor.run_progress),
            orchestration,
            |progress| {
                cursor.run_progress = progress.clone();
                serde_json::to_string(&cursor).map_err(|_| JevError::ProgressStorageFailure)
            },
        )
        .await;
        let mut outcome = match outcome {
            Ok(outcome) => outcome,
            Err(JevError::Cancelled)
                if handle.turn_exhausted("skill_opportunities", &candidate.session.key) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        if !handle.key_is_current(key_generation) {
            return Ok(());
        }
        if let Some(error) = outcome.failure.take() {
            for job in &jobs {
                let reviewed = sampled_pairs_for_job(
                    &plan,
                    &outcome.result,
                    job,
                    candidate.incarnation,
                    &input.durable.input_revision,
                );
                if reviewed.iter().any(|pair| pair.pair.assessed) {
                    input
                        .check
                        .record_sampling_result(
                            cursor.sampling.as_mut().expect("sampling was initialized"),
                            job,
                            &outcome.result,
                        )
                        .map_err(|error| anyhow::anyhow!("sampling outcome rejected: {error:?}"))?;
                }
            }
            merge_skill_result(
                cursor.result.get_or_insert_with(|| outcome.result.clone()),
                outcome.result,
            );
            cursor.run_progress = outcome.progress;
            write_fence.commit(|| {
                save_scheduling(store, &input, &cursor)?;
                store.save_burn_check_checkpoint(
                    &input.durable,
                    &serde_json::to_string(&cursor)?,
                    Some(&cursor.run_progress),
                    unix_now(),
                    POLICY.lease_secs,
                    POLICY.idle_secs,
                )
            })?;
            if matches!(error, JevError::Cancelled)
                && handle.turn_exhausted("skill_opportunities", &candidate.session.key)
            {
                write_fence.commit(|| {
                    store.release_failed_burn_check_lease(
                        &input.durable,
                        "continuing",
                        unix_now() + 1,
                    )
                })?;
                return Ok(());
            }
            let mut terminal = Vec::new();
            for job in &jobs {
                if cursor
                    .sampling
                    .as_ref()
                    .expect("sampling was initialized")
                    .completed_ids(input.check.sampling_identity())
                    .contains(&job.candidate)
                {
                    continue;
                }
                let mut target = plan.clone();
                target.work_items.retain(|item| {
                    antiburn_local::checks::sampling::StableId::new(
                        "skill-opportunities",
                        &[item.id.as_bytes()],
                    ) == job.candidate
                });
                if crate::scope_creep_worker::target_failure_is_terminal(&error, || {
                    crate::scope_creep_worker::dispatch_readiness(
                        store,
                        handle,
                        &input.durable,
                        &capabilities,
                        &target,
                        &cursor.run_progress,
                    )
                })? {
                    if matches!(
                        error,
                        JevError::RequestTooLarge { .. }
                            | JevError::RequestTokenLimitExceeded { .. }
                    ) {
                        cursor
                            .context_blocked_ids
                            .insert(serde_json::to_string(&job.candidate)?);
                    }
                    cursor
                        .sampling
                        .as_mut()
                        .expect("sampling was initialized")
                        .terminate_candidate(job)
                        .map_err(|error| {
                            anyhow::anyhow!("skill termination rejected: {error:?}")
                        })?;
                    terminal.push(job.candidate);
                }
            }
            if !terminal.is_empty() {
                jobs.retain(|job| !terminal.contains(&job.candidate));
                cursor.active_job = jobs.first().cloned();
                cursor.batch_jobs = jobs.iter().skip(1).cloned().collect();
                input.check.retain_plan_jobs(&mut plan, &jobs)?;
                cursor
                    .run_progress
                    .results
                    .retain(|id, _| plan.work_items.iter().any(|item| &item.id == id));
                cursor.active_plan = (!jobs.is_empty()).then_some(plan);
                if jobs.is_empty() {
                    cursor.run_progress = JevRunProgress::default();
                }
                write_fence.commit(|| {
                    save_scheduling(store, &input, &cursor)?;
                    store.save_burn_check_checkpoint(
                        &input.durable,
                        &serde_json::to_string(&cursor)?,
                        Some(&cursor.run_progress),
                        unix_now(),
                        POLICY.lease_secs,
                        POLICY.idle_secs,
                    )
                })?;
                continue;
            }
            let result_json = publication_json(
                &input,
                cursor.result.as_ref().expect("result was initialized"),
            )?;
            let progress_json = serde_json::to_string(&cursor)?;
            let saved = write_fence.publish(|| {
                let published = store.fail_burn_check_assessment_with_result(
                    &input.durable,
                    &BurnCheckFailure {
                        error_category: error_category(&error),
                        result_json: &result_json,
                        progress_json: &progress_json,
                        retry_at_epoch: store.burn_check_next_attempt_at(&input.durable)?,
                    },
                    unix_now(),
                    POLICY.idle_secs,
                )?;
                if published {
                    store.save_burn_check_sampled_pairs(
                        &input.durable,
                        &accepted_pairs(
                            &input,
                            cursor.result.as_ref().expect("result was initialized"),
                        ),
                    )?;
                }
                if published && !matches!(error, JevError::Cancelled) {
                    let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
                    record_assessment(
                        app,
                        candidate.historical,
                        crate::analytics::event::SmartCheckAssessmentOutcome::Failed,
                    );
                }
                Ok(published)
            });
            let saved = saved.await;
            let rejection = crate::jev::worker::settle_authentication_rejection(
                app,
                store,
                handle,
                key_generation,
                matches!(error, JevError::AuthenticationRejected),
            );
            saved?;
            rejection?;
            return Ok(());
        }
        for job in &jobs {
            if outcome.result.decisions.iter().any(|decision| {
                antiburn_local::checks::sampling::StableId::new(
                    "skill-opportunities",
                    &[decision.comparison.id.as_bytes()],
                ) == job.candidate
                    && decision.judgments.is_some()
            }) {
                input
                    .check
                    .record_sampling_result(
                        cursor.sampling.as_mut().expect("sampling was initialized"),
                        job,
                        &outcome.result,
                    )
                    .map_err(|error| anyhow::anyhow!("sampling outcome rejected: {error:?}"))?;
            } else {
                cursor
                    .sampling
                    .as_mut()
                    .expect("sampling was initialized")
                    .terminate_candidate(job)
                    .map_err(|error| anyhow::anyhow!("skill termination rejected: {error:?}"))?;
            }
            sampled_pairs.extend(sampled_pairs_for_job(
                &plan,
                &outcome.result,
                job,
                candidate.incarnation,
                &input.durable.input_revision,
            ));
        }
        merge_skill_result(
            cursor.result.get_or_insert_with(|| outcome.result.clone()),
            outcome.result,
        );
        cursor.active_job = None;
        cursor.batch_jobs.clear();
        cursor.active_plan = None;
        cursor.run_progress = JevRunProgress::default();
        let progress_json = serde_json::to_string(&cursor)?;
        if !write_fence
            .commit(|| {
                save_scheduling(store, &input, &cursor)?;
                store.save_burn_check_checkpoint(
                    &input.durable,
                    &progress_json,
                    None,
                    unix_now(),
                    POLICY.lease_secs,
                    POLICY.idle_secs,
                )
            })?
            .unwrap_or(false)
        {
            return Ok(());
        }
        if !handle.key_is_current(key_generation) {
            return Ok(());
        }
    }
    let complete = cursor.inventory.complete
        && cursor
            .sampling
            .as_ref()
            .and_then(|sampling| sampling.coverage(input.check.sampling_identity()))
            .is_some_and(|_| {
                cursor
                    .sampling
                    .as_ref()
                    .expect("sampling initialized")
                    .runnable_count(input.check.sampling_identity())
                    == 0
            });
    let mut result = cursor
        .result
        .clone()
        .unwrap_or_else(|| SkillOpportunitiesResult {
            findings: Vec::new(),
            decisions: Vec::new(),
            coverage: antiburn_local::analysis::jev::JevCoverage::default(),
            complete: false,
        });
    if cursor.reviewed_before_page + cursor.skipped_before_page == 0 {
        retain_skill_inventory(&mut result, &cursor.inventory);
    }
    result
        .coverage
        .limitations
        .retain(|limit| limit != "descriptor_enumeration_incomplete");
    result.coverage.processing_limit_reached =
        !cursor.inventory.complete || !result.coverage.limitations.is_empty();
    if !cursor.inventory.complete {
        result
            .coverage
            .limitations
            .push("descriptor_enumeration_incomplete".into());
    }
    result.complete = complete && result.coverage.limitations.is_empty() && result.decisions.iter().all(|decision| matches!(decision.outcome, antiburn_local::checks::skill_opportunities::SkillOpportunityOutcome::Advisory | antiburn_local::checks::skill_opportunities::SkillOpportunityOutcome::NoOpportunity));
    if let Some(coverage) = cursor
        .sampling
        .as_ref()
        .and_then(|sampling| sampling.coverage(input.check.sampling_identity()))
    {
        result.coverage.selected_items = cursor.reviewed_before_page + coverage.completed;
        let runnable = cursor
            .sampling
            .as_ref()
            .expect("sampling initialized")
            .runnable_count(input.check.sampling_identity());
        result.coverage.not_selected_items = runnable
            + input
                .check
                .descriptor_count()
                .saturating_sub(cursor.inventory.next_comparison);
        result.coverage.skipped_items = cursor.skipped_before_page
            + coverage
                .eligible
                .saturating_sub(coverage.completed + runnable);
    }
    // No completion claim is made for an empty or interrupted sampled run.
    result.complete &= result.coverage.skipped_items == 0
        && result.coverage.not_selected_items == 0
        && cursor.uncertain_before_page == 0;
    if sampled_pairs.is_empty() && !complete && cursor.active_job.is_some() {
        let progress_json = serde_json::to_string(&cursor)?;
        write_fence.commit(|| {
            store.save_burn_check_checkpoint(
                &input.durable,
                &progress_json,
                Some(&cursor.run_progress),
                unix_now(),
                POLICY.lease_secs,
                POLICY.idle_secs,
            )
        })?;
        return Ok(());
    }
    result.findings.retain(publishable_finding);
    let result_json = publication_json(&input, &result)?;
    let progress_json = serde_json::to_string(&cursor)?;
    let pairs = accepted_pairs(&input, &result);
    let published = write_fence
        .publish(|| {
            let published = if complete {
                store.complete_burn_check_assessment(
                    &input.durable,
                    &result_json,
                    unix_now(),
                    POLICY.idle_secs,
                )?
            } else {
                store.fail_burn_check_assessment_with_result(
                    &input.durable,
                    &BurnCheckFailure {
                        error_category: if cursor
                            .sampling
                            .as_ref()
                            .expect("sampling was initialized")
                            .runnable_count(input.check.sampling_identity())
                            > 0
                            || !cursor.inventory.complete
                        {
                            "continuing"
                        } else {
                            "sampling_incomplete"
                        },
                        result_json: &result_json,
                        progress_json: &progress_json,
                        retry_at_epoch: (cursor
                            .sampling
                            .as_ref()
                            .expect("sampling was initialized")
                            .runnable_count(input.check.sampling_identity())
                            > 0
                            || !cursor.inventory.complete)
                            .then(|| unix_now() + 1),
                    },
                    unix_now(),
                    POLICY.idle_secs,
                )?
            };
            if published {
                store.save_burn_check_sampled_pairs(&input.durable, &pairs)?;
                let assessed = publication_has_assessed_coverage(&result);
                record_assessment(
                    app,
                    candidate.historical,
                    crate::analytics::event::SmartCheckAssessmentOutcome::from_evidence(
                        assessed && !result.findings.is_empty(),
                        assessed && result.complete,
                    ),
                );
            }
            Ok(published)
        })
        .await?
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
            check: crate::analytics::event::SmartCheck::SkillOpportunities,
            historical,
            outcome,
        },
    );
}

pub(crate) fn publication_has_assessed_coverage(result: &SkillOpportunitiesResult) -> bool {
    use antiburn_local::checks::skill_opportunities::SkillOpportunityOutcome;
    result.coverage.selected_items > 0
        && if result.complete {
            result.coverage.selected_items >= result.decisions.len()
                && result.coverage.not_selected_items == 0
                && result.coverage.skipped_items == 0
                && result.coverage.limitations.is_empty()
                && result.decisions.iter().all(|decision| {
                    decision.judgments.is_some()
                        && matches!(
                            decision.outcome,
                            SkillOpportunityOutcome::Advisory
                                | SkillOpportunityOutcome::NoOpportunity
                        )
                })
        } else {
            true
        }
        && result
            .decisions
            .iter()
            .any(|decision| decision.judgments.is_some())
        && result.decisions.iter().all(|decision| {
            decision.outcome != SkillOpportunityOutcome::Advisory
                || result
                    .findings
                    .iter()
                    .any(|finding| finding.comparison == decision.comparison)
        })
        && result.findings.iter().all(|finding| {
            publishable_finding(finding)
                && result.decisions.iter().any(|decision| {
                    decision.outcome == SkillOpportunityOutcome::Advisory
                        && decision.comparison == finding.comparison
                })
        })
}

pub(crate) struct SkillInputObservation {
    pub(crate) input_revision: Option<String>,
    pub(crate) generation: u64,
    pub(crate) invalidated: bool,
}

/// The shared worker calls this for enrolled sessions, including completed sessions.
/// Use its existing poll and in-flight cadence. Do not add a feature task.
pub(crate) fn reconcile_skill_inputs(
    store: &Store,
    handle: &WorkerHandle,
    provider_generation: u64,
    candidate: &BurnCheckCandidate,
) -> anyhow::Result<Option<SkillInputObservation>> {
    let enrolled = store.enrolled_burn_check_candidate(candidate, SKILL_OPPORTUNITIES_CHECK_ID)?;
    let current = match candidate_config(&enrolled) {
        Some(config) => current_input_revisions(store, &enrolled, &config)?,
        None => None,
    };
    observe_skill_input(
        store,
        handle,
        provider_generation,
        &enrolled,
        current.map(|(input, _)| input),
    )
}

fn observe_skill_input(
    store: &Store,
    handle: &WorkerHandle,
    provider_generation: u64,
    candidate: &BurnCheckCandidate,
    input_revision: Option<String>,
) -> anyhow::Result<Option<SkillInputObservation>> {
    handle
        .with_current_generation(provider_generation, || {
            let mut observations = INPUT_OBSERVATIONS
                .lock()
                .map_err(|_| anyhow::anyhow!("skill input observation lock failed"))?;
            let Some(stored_revision) = current_assessment_revision(store, candidate)? else {
                return Ok(None);
            };
            if !source_is_current(store, candidate)? {
                return Ok(None);
            }
            let invalidated = if stored_revision.is_some() && stored_revision != input_revision {
                store.invalidate_burn_check_inputs(
                    &candidate.session.key,
                    SKILL_OPPORTUNITIES_CHECK_ID,
                    stored_revision.as_deref(),
                    unix_now(),
                )?
            } else {
                false
            };
            let generation = observations
                .observe_input(&candidate.session.key, input_revision.clone())
                .map_err(|error| anyhow::anyhow!("skill input observation failed: {error:?}"))?;
            Ok(Some(SkillInputObservation {
                input_revision,
                generation,
                invalidated,
            }))
        })
        .unwrap_or(Ok(None))
}

/// Read source fences and the prior revision together before any reload or invalidation.
fn current_assessment_revision(
    store: &Store,
    candidate: &BurnCheckCandidate,
) -> anyhow::Result<Option<Option<String>>> {
    use rusqlite::OptionalExtension;
    Ok(store
        .lock()
        .query_row(
            "SELECT a.input_revision FROM session s
         JOIN session_evidence e ON e.environment_key = s.environment_key
           AND e.agent = s.agent AND e.session_id = s.session_id
         LEFT JOIN burn_check_assessment a ON a.environment_key = s.environment_key
           AND a.agent = s.agent AND a.session_id = s.session_id AND a.check_id = ?4
         WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
           AND s.incarnation = ?5 AND s.source_generation = ?6
           AND s.source_fingerprint IS ?7 AND s.activity_cursor = ?8
           AND e.published_fence = ?9",
            rusqlite::params![
                candidate.session.key.environment_key,
                candidate.session.key.agent,
                candidate.session.key.session_id,
                SKILL_OPPORTUNITIES_CHECK_ID,
                candidate.incarnation,
                candidate.source_generation,
                candidate.source_fingerprint,
                candidate.activity_cursor,
                candidate.published_fence
            ],
            |row| row.get(0),
        )
        .optional()?)
}

fn candidate_config(candidate: &BurnCheckCandidate) -> Option<crate::agent_config::ConfigContext> {
    if candidate.session.key.environment_key != "native"
        || !matches!(
            candidate.session.key.agent.as_str(),
            "opencode" | "codex" | "claude-code" | "pi"
        )
    {
        return None;
    }
    let home = antiburn_local::paths::home_dir()?;
    let cwd = candidate
        .session
        .cwd
        .as_deref()
        .map(std::path::Path::new)
        .map(std::path::Path::canonicalize)
        .transpose()
        .ok()?;
    Some(crate::agent_config::ConfigContext::native(
        AgentKind::from_slug(&candidate.session.key.agent)?,
        home,
        cwd,
    ))
}

struct InputWritePermit<'a> {
    key: &'a crate::store::SessionKey,
    revision: &'a str,
    generation: u64,
}

fn with_skill_input_generation<T>(
    handle: &WorkerHandle,
    provider_generation: u64,
    observations: &Mutex<InventoryRevisionObserver>,
    permit: InputWritePermit<'_>,
    reload: impl FnOnce() -> anyhow::Result<Option<String>>,
    commit: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<Option<T>> {
    handle
        .with_current_generation(provider_generation, || {
            let mut observations = observations
                .lock()
                .map_err(|_| anyhow::anyhow!("skill input observation lock failed"))?;
            if !observations.input_is_current(permit.key, permit.revision, permit.generation) {
                return Ok(None);
            }
            // The locks fence app observations. External filesystem writes remain outside these locks.
            let current = reload()?;
            observations
                .observe_input(permit.key, current)
                .map_err(|error| anyhow::anyhow!("skill input observation failed: {error:?}"))?;
            if !observations.input_is_current(permit.key, permit.revision, permit.generation) {
                return Ok(None);
            }
            commit().map(Some)
        })
        .unwrap_or(Ok(None))
}

struct SkillWriteFence<'a> {
    store: &'a Store,
    handle: &'a WorkerHandle,
    provider_generation: u64,
    input_generation: u64,
    input: &'a PreparedSkillOpportunityInput,
    candidate: &'a BurnCheckCandidate,
    config: &'a crate::agent_config::ConfigContext,
}

impl SkillWriteFence<'_> {
    fn commit<T>(&self, commit: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<Option<T>> {
        self.commit_with_inventory(Some(&self.input.inventory_revision), commit)
    }

    async fn publish<T>(
        &self,
        commit: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<Option<T>> {
        if !self.handle.key_is_current(self.provider_generation) {
            return Ok(None);
        }
        let preparation = admit_jev_orchestration().await?;
        let config = self.config.clone();
        let inventory = tauri::async_runtime::spawn_blocking(move || {
            let _permit = preparation;
            crate::smart_check_inputs::inventory_cache::discover_inventory(&config)
        })
        .await?;
        let revision = match inventory {
            Ok(inventory) => Some(inventory.revision()),
            Err(error) => {
                ::tracing::debug!(event = "skill_inventory_publication_unavailable", error = ?error);
                None
            }
        };
        self.commit_with_inventory(revision.as_deref(), commit)
    }

    fn commit_with_inventory<T>(
        &self,
        inventory_revision: Option<&str>,
        commit: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<Option<T>> {
        with_skill_input_generation(
            self.handle,
            self.provider_generation,
            &INPUT_OBSERVATIONS,
            InputWritePermit {
                key: &self.input.durable.key,
                revision: &self.input.durable.input_revision,
                generation: self.input_generation,
            },
            || {
                let current = match inventory_revision {
                    Some(revision) => {
                        cached_input_revisions(self.store, self.candidate, self.config, revision)?
                    }
                    None => None,
                };
                if !current_revisions_match(
                    current.as_ref(),
                    &self.input.durable.input_revision,
                    &self.input.inventory_revision,
                ) {
                    self.store.invalidate_burn_check_inputs(
                        &self.input.durable.key,
                        SKILL_OPPORTUNITIES_CHECK_ID,
                        Some(&self.input.durable.input_revision),
                        unix_now(),
                    )?;
                }
                Ok(current.map(|(input, _)| input))
            },
            commit,
        )
    }
}

fn restore_cursor(
    stored: Option<&crate::store::BurnCheckAssessment>,
    input_revision: &str,
    provider_generation: u64,
) -> SkillCursor {
    let mut cursor = stored
        .and_then(|assessment| serde_json::from_str::<SkillCursor>(&assessment.progress_json).ok())
        .filter(|cursor| {
            cursor.revision == CURSOR_REVISION
                && cursor.engine_revisions == SKILL_OPPORTUNITIES_REVISIONS
        })
        .unwrap_or_default();
    cursor.provider_generation = provider_generation;
    if cursor.input_revision != input_revision {
        cursor.context_blocked_ids.clear();
        cursor.reviewed_before_page = 0;
        cursor.uncertain_before_page = 0;
        cursor.skipped_before_page = 0;
        cursor.context_blocked_before_page = 0;
        cursor.input_revision = input_revision.to_owned();
        cursor.active_job = None;
        cursor.batch_jobs.clear();
        cursor.active_plan = None;
        cursor.run_progress = JevRunProgress::default();
    }
    cursor
}

fn save_scheduling(
    store: &Store,
    input: &PreparedSkillOpportunityInput,
    cursor: &SkillCursor,
) -> anyhow::Result<bool> {
    let sampling = cursor.sampling.as_ref().expect("sampling was initialized");
    let coverage = sampling
        .coverage(input.check.sampling_identity())
        .expect("synchronized");
    store.save_burn_check_scheduling(
        &input.durable,
        Some(input.check.descriptor_count()),
        cursor.reviewed_before_page + coverage.completed,
        sampling.runnable_count(input.check.sampling_identity())
            + input
                .check
                .descriptor_count()
                .saturating_sub(cursor.inventory.next_comparison),
    )
}

fn enumerate_skill_turn(
    input: &PreparedSkillOpportunityInput,
    cursor: &mut SkillCursor,
) -> anyhow::Result<()> {
    if !cursor.inventory.complete
        && cursor.inventory.descriptors.len() == MAX_SAMPLE_CANDIDATES
        && cursor.active_job.is_none()
        && cursor.batch_jobs.is_empty()
        && cursor.sampling.as_ref().is_some_and(|sampling| {
            sampling.coverage(input.check.sampling_identity()).is_some()
                && sampling.runnable_count(input.check.sampling_identity()) == 0
        })
    {
        let coverage = cursor
            .sampling
            .as_ref()
            .expect("sampling initialized")
            .coverage(input.check.sampling_identity())
            .expect("inventory synchronized");
        cursor.reviewed_before_page += coverage.completed;
        cursor.skipped_before_page += coverage.eligible.saturating_sub(coverage.completed);
        cursor.context_blocked_before_page += cursor.context_blocked_ids.len();
        cursor.context_blocked_ids.clear();
        input.check.advance_descriptor_page(&mut cursor.inventory)?;
        cursor.sampling = Some(new_sampling_progress()?);
        if let Some(result) = &mut cursor.result {
            cursor.uncertain_before_page += result.decisions.iter().filter(|decision| decision.outcome == antiburn_local::checks::skill_opportunities::SkillOpportunityOutcome::Uncertain && decision.judgments.is_some()).count();
            result.decisions.retain(|decision| {
                result
                    .findings
                    .iter()
                    .any(|finding| finding.comparison == decision.comparison)
            });
        }
    }
    input.check.enumerate_descriptors(&mut cursor.inventory)?;
    cursor
        .sampling
        .as_mut()
        .expect("sampling was initialized")
        .synchronize_ordered(
            input.check.sampling_identity(),
            input.check.sampling_epoch(),
            &input.check.descriptor_candidates(&cursor.inventory)?,
            &input.check.descriptor_chronology(&cursor.inventory)?,
        )
        .map_err(|error| anyhow::anyhow!("sampling inventory rejected: {error:?}"))?;
    if cursor.inventory.complete
        && cursor.reviewed_before_page + cursor.skipped_before_page == 0
        && let Some(result) = &mut cursor.result
    {
        retain_skill_inventory(result, &cursor.inventory);
    }
    Ok(())
}

fn retain_skill_inventory(
    result: &mut SkillOpportunitiesResult,
    inventory: &SkillDescriptorInventory,
) {
    let ids: std::collections::BTreeSet<_> = inventory
        .descriptors
        .iter()
        .map(|descriptor| descriptor.0.as_str())
        .collect();
    result
        .decisions
        .retain(|decision| ids.contains(decision.comparison.id.as_str()));
    result
        .findings
        .retain(|finding| ids.contains(finding.comparison.id.as_str()));
}

fn current_revisions_match(
    current: Option<&(String, String)>,
    input_revision: &str,
    inventory_revision: &str,
) -> bool {
    current.is_some_and(|(current_input, current_inventory)| {
        current_input == input_revision && current_inventory == inventory_revision
    })
}

fn publication_json(
    input: &PreparedSkillOpportunityInput,
    result: &SkillOpportunitiesResult,
) -> Result<String, serde_json::Error> {
    #[derive(serde::Serialize)]
    struct Publication<'a> {
        input_revision: &'a str,
        inventory_revision: &'a str,
        use_revision: &'a str,
        #[serde(flatten)]
        result: &'a SkillOpportunitiesResult,
    }
    serde_json::to_string(&Publication {
        input_revision: &input.durable.input_revision,
        inventory_revision: &input.inventory_revision,
        use_revision: &input.use_revision,
        result,
    })
}

fn new_sampling_progress() -> anyhow::Result<SamplingProgress> {
    SamplingProgress::new(SamplingLimits {
        checks: MAX_SAMPLE_CHECKS,
        candidates_per_check: MAX_SAMPLE_CANDIDATES,
        answers_per_candidate: MAX_SAMPLE_ANSWERS,
        judgments_per_run: MAX_SAMPLE_JUDGMENTS,
    })
    .map_err(|error| anyhow::anyhow!("sampling limits are invalid: {error:?}"))
}

fn record_no_candidates(
    store: &Store,
    input: &PreparedSkillOpportunityInput,
    cursor: &SkillCursor,
) -> anyhow::Result<bool> {
    store.record_burn_check_no_candidates(
        &input.durable,
        &serde_json::to_string(cursor)?,
        unix_now(),
        POLICY.idle_secs,
    )
}

fn unavailable(
    store: &Store,
    candidate: &BurnCheckCandidate,
    category: &str,
    handle: &WorkerHandle,
    provider_generation: u64,
) -> anyhow::Result<()> {
    if let Some(result) = handle.with_current_generation(provider_generation, || {
        store.record_burn_check_candidate_failure_for_check(
            SKILL_OPPORTUNITIES_CHECK_ID,
            candidate,
            unavailable_is_terminal(category),
            category,
            unix_now().saturating_add(POLICY.retry_delay_secs),
            unix_now(),
        )
    }) {
        result?;
    }
    Ok(())
}

fn unavailable_is_terminal(category: &str) -> bool {
    matches!(category, "unsupported_format" | "skill_use_invalid")
}

struct SampledSkillPair {
    pair: BurnCheckSampledPair,
}

fn accepted_pairs(
    input: &PreparedSkillOpportunityInput,
    result: &SkillOpportunitiesResult,
) -> Vec<BurnCheckSampledPair> {
    result
        .decisions
        .iter()
        .filter(|decision| decision.judgments.is_some())
        .map(|decision| BurnCheckSampledPair {
            comparison_id: antiburn_local::checks::sampling::StableId::new(
                "skill-opportunities",
                &[decision.comparison.id.as_bytes()],
            )
            .into(),
            dependency_digest: input.durable.input_revision.clone(),
            incarnation: input.durable.incarnation,
            action_id: decision.comparison.id.clone(),
            action_digest: input.durable.input_revision.clone(),
            instruction_digest: String::new(),
            selector_revision: CURSOR_REVISION,
            round: 0,
            assessed: true,
        })
        .collect()
}

fn sampled_pairs_for_job(
    plan: &JevCheckPlan<<SkillOpportunitiesCheck as JevCheck>::Prepared>,
    result: &SkillOpportunitiesResult,
    job: &SamplingJob,
    incarnation: u64,
    revision: &str,
) -> Vec<SampledSkillPair> {
    let candidate = serde_json::to_value(job.candidate)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    plan.prepared.comparisons.iter().filter(|comparison| antiburn_local::checks::sampling::StableId::new("skill-opportunities", &[comparison.id.as_bytes()]) == job.candidate).map(|comparison| {
        let decision = result.decisions.iter().find(|decision| decision.comparison.id == comparison.id);
        let assessed = decision.is_some_and(|decision| decision.outcome != antiburn_local::checks::skill_opportunities::SkillOpportunityOutcome::Unassessed);
        SampledSkillPair {
            pair: BurnCheckSampledPair {
                comparison_id: candidate.clone(), dependency_digest: revision.to_owned(), incarnation,
                action_id: comparison.id.clone(), action_digest: revision.to_owned(), instruction_digest: String::new(),
                selector_revision: CURSOR_REVISION, round: 0, assessed,
            },
        }
    }).collect()
}

fn merge_skill_result(target: &mut SkillOpportunitiesResult, page: SkillOpportunitiesResult) {
    let judged: std::collections::BTreeSet<_> = page
        .decisions
        .iter()
        .filter(|decision| decision.judgments.is_some())
        .map(|decision| decision.comparison.id.clone())
        .collect();
    target
        .findings
        .retain(|finding| !judged.contains(&finding.comparison.id));
    for decision in page.decisions {
        if let Some(existing) = target
            .decisions
            .iter_mut()
            .find(|existing| existing.comparison.id == decision.comparison.id)
        {
            if decision.judgments.is_some() {
                *existing = decision;
            }
        } else {
            target.decisions.push(decision);
        }
    }
    for finding in page.findings {
        if !judged.contains(&finding.comparison.id) {
            continue;
        }
        if !target
            .findings
            .iter()
            .any(|existing| existing.comparison.id == finding.comparison.id)
        {
            target.findings.push(finding);
        }
    }
    target.coverage = page.coverage;
}

fn current_input_revisions(
    store: &Store,
    candidate: &BurnCheckCandidate,
    config: &crate::agent_config::ConfigContext,
) -> anyhow::Result<Option<(String, String)>> {
    if !source_is_current(store, candidate)? {
        return Ok(None);
    }
    let inventory = match crate::smart_check_inputs::inventory_cache::sweep_inventory(
        config,
        &candidate.session.key,
    ) {
        Ok(inventory) => inventory,
        Err(InputLoadError::Inventory(_)) => return Ok(None),
        Err(error) => {
            return Err(anyhow::anyhow!(
                "skill inventory could not be read: {error:?}"
            ));
        }
    };
    current_input_revisions_with_inventory(store, candidate, config, inventory)
}

fn source_is_current(store: &Store, candidate: &BurnCheckCandidate) -> anyhow::Result<bool> {
    crate::smart_check_inputs::cache::source_is_current(
        store,
        candidate,
        SKILL_OPPORTUNITIES_CHECK_ID,
        CHECK.evaluator_revision(),
    )
}

fn config_matches_candidate(
    candidate: &BurnCheckCandidate,
    config: &crate::agent_config::ConfigContext,
) -> bool {
    let canonical =
        |path: Option<&std::path::Path>| path.map(std::path::Path::canonicalize).transpose().ok();
    config.native_environment
        && candidate.session.key.environment_key == "native"
        && candidate.session.wsl_distro.is_none()
        && config.agent.slug() == candidate.session.key.agent
        && canonical(candidate.session.cwd.as_deref().map(std::path::Path::new))
            .is_some_and(|cwd| Some(cwd) == canonical(config.workspace_cwd.as_deref()))
}

fn cached_input_revisions(
    store: &Store,
    candidate: &BurnCheckCandidate,
    config: &crate::agent_config::ConfigContext,
    inventory_revision: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let enrolled = store.enrolled_burn_check_candidate(candidate, SKILL_OPPORTUNITIES_CHECK_ID)?;
    let candidate = &enrolled;
    if !source_is_current(store, candidate)? || !config_matches_candidate(candidate, config) {
        return Ok(None);
    }
    if current_assessment_revision(store, candidate)?.is_none() {
        return Ok(None);
    }
    let source_key = revision_source_key(candidate)?;
    let store_identity = crate::smart_check_inputs::cache::store_identity(store);
    if let Some(cached) = INPUT_REVISIONS
        .lock()
        .map_err(|_| anyhow::anyhow!("skill revision cache lock failed"))?
        .iter()
        .find(|cached| {
            cached.store_identity == store_identity
                && cached.source_key == source_key
                && cached.config == *config
                && cached.inventory_revision == inventory_revision
        })
    {
        return Ok(Some((
            cached.input_revision.clone(),
            cached.inventory_revision.clone(),
        )));
    }
    Ok(None)
}

fn current_input_revisions_with_inventory(
    store: &Store,
    candidate: &BurnCheckCandidate,
    config: &crate::agent_config::ConfigContext,
    inventory: Arc<antiburn_local::checks::skill_opportunities::SkillOpportunitySnapshot>,
) -> anyhow::Result<Option<(String, String)>> {
    if let Some(revisions) =
        cached_input_revisions(store, candidate, config, &inventory.revision())?
    {
        return Ok(Some(revisions));
    }
    let input = match load_input(store, candidate, config, inventory) {
        Ok(input) => input,
        Err(
            InputLoadError::Unavailable(_)
            | InputLoadError::Inventory(_)
            | InputLoadError::SkillUse(_)
            | InputLoadError::Preparation(_)
            | InputLoadError::Scope(crate::session_scope::ScopeLoadError::Scope(_)),
        ) => return Ok(None),
        Err(error) => {
            return Err(anyhow::anyhow!(
                "current skill inputs could not be read: {error:?}"
            ));
        }
    };
    Ok(Some((
        input.durable.input_revision.clone(),
        input.inventory_revision.clone(),
    )))
}

fn load_input(
    store: &Store,
    candidate: &BurnCheckCandidate,
    config: &crate::agent_config::ConfigContext,
    inventory: Arc<antiburn_local::checks::skill_opportunities::SkillOpportunitySnapshot>,
) -> Result<Arc<PreparedSkillOpportunityInput>, InputLoadError> {
    let candidate = store
        .enrolled_burn_check_candidate(candidate, SKILL_OPPORTUNITIES_CHECK_ID)
        .map_err(InputLoadError::Storage)?;
    if !source_is_current(store, &candidate).map_err(InputLoadError::Storage)? {
        return Err(InputLoadError::Unavailable(
            crate::smart_check_inputs::InputUnavailable::PublicationChanged,
        ));
    }
    if !config_matches_candidate(&candidate, config) {
        return Err(InputLoadError::Unavailable(
            crate::smart_check_inputs::InputUnavailable::InventoryContextMismatch,
        ));
    }
    let source_key = revision_source_key(&candidate).map_err(InputLoadError::Serialization)?;
    let key = serde_json::to_string(&(&source_key, format!("{config:?}"), inventory.revision()))
        .map_err(InputLoadError::Serialization)?;
    if let Some(input) = PREPARED_INPUTS
        .lock()
        .map_err(|_| InputLoadError::Preparation(JevError::InvalidCheckContext))?
        .get(store, &key)
    {
        return Ok(input);
    }
    let snapshot = store.load_smart_check_inputs(
        &candidate.session.key,
        candidate.published_fence,
        candidate.source_generation,
        crate::smart_check_inputs::DetectorInput::SkillOpportunities,
    )?;
    let bytes = serde_json::to_vec(&(snapshot.scope(), snapshot.content(), inventory.skills()))
        .map_err(InputLoadError::Serialization)?
        .len()
        .saturating_mul(4);
    let inputs = store.load_smart_check_skill_inputs_with_inventory(snapshot, config, inventory)?;
    let input = Arc::new(prepare(&candidate, inputs, CHECK.evaluator_revision())?);
    cache_input_revision(CachedInputRevision {
        _store: store.clone(),
        store_identity: crate::smart_check_inputs::cache::store_identity(store),
        source_key,
        config: config.clone(),
        input_revision: input.durable.input_revision.clone(),
        inventory_revision: input.inventory_revision.clone(),
    })
    .map_err(InputLoadError::Storage)?;
    PREPARED_INPUTS
        .lock()
        .map_err(|_| InputLoadError::Preparation(JevError::InvalidCheckContext))?
        .insert(store, key, bytes, Arc::clone(&input));
    Ok(input)
}

/// Prepared production input. The revision binds activity, scope, skill use, and inventory.
pub(crate) struct PreparedSkillOpportunityInput {
    pub(crate) check: Arc<SkillOpportunitiesCheck>,
    pub(crate) durable: BurnCheckInput,
    pub(crate) inventory_revision: String,
    pub(crate) use_revision: String,
}

/// Source admission uses the accepted native agent and format pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceLimit {
    Supported,
    ProducerContractNotAdmitted,
}

pub(crate) fn source_limit(agent: &str, format: SourceFormat) -> SourceLimit {
    if antiburn_local::analysis::smart_check_source_supported(agent, format) {
        SourceLimit::Supported
    } else {
        SourceLimit::ProducerContractNotAdmitted
    }
}

/// Bind loader output to the candidate's durable publication fence.
pub(crate) fn prepare(
    candidate: &BurnCheckCandidate,
    inputs: SkillInputs,
    evaluator_revision: String,
) -> Result<PreparedSkillOpportunityInput, InputLoadError> {
    let inputs = inputs.for_candidate(candidate)?;
    let snapshot = inputs.input();
    if candidate.session.key.environment_key != "native"
        || source_limit(
            &candidate.session.key.agent,
            snapshot.content().source_format,
        ) != SourceLimit::Supported
    {
        return Err(InputLoadError::Unavailable(
            crate::smart_check_inputs::InputUnavailable::IncompleteEvidence,
        ));
    }

    let revision = inputs.input_revision().to_owned();
    let check = Arc::new(inputs.check()?);
    let durable = BurnCheckInput {
        key: candidate.session.key.clone(),
        check_id: SKILL_OPPORTUNITIES_CHECK_ID.to_owned(),
        incarnation: candidate.incarnation,
        source_generation: candidate.source_generation,
        source_fingerprint: candidate.source_fingerprint.clone(),
        activity_cursor: candidate.activity_cursor.clone(),
        published_fence: candidate.published_fence,
        input_revision: revision,
        evaluator_revision,
        boundary_at_epoch: candidate.boundary_at_epoch,
    };
    Ok(PreparedSkillOpportunityInput {
        check,
        durable,
        inventory_revision: inputs.inventory().revision().to_owned(),
        use_revision: inputs.usage().revision().to_owned(),
    })
}

/// Synchronize the semantic inventory before choosing bounded fair jobs.
/// Persist `SamplingProgress` with the feature cursor; do not call `begin_run`
/// when restoring an interrupted assessment.
#[cfg(test)]
pub(crate) fn synchronize_sampling(
    input: &PreparedSkillOpportunityInput,
    progress: &mut SamplingProgress,
) -> Result<(), antiburn_local::checks::sampling::SamplingError> {
    input.check.synchronize_sampling(progress)
}

/// Build a plan only for jobs chosen by the shared durable sampling ledger.
#[cfg(test)]
pub(crate) fn prepare_sampled(
    input: &PreparedSkillOpportunityInput,
    capabilities: &ModelCapabilities,
    jobs: &[SamplingJob],
) -> Result<
    JevCheckPlan<<SkillOpportunitiesCheck as antiburn_local::analysis::jev::JevCheck>::Prepared>,
    JevError,
> {
    input
        .check
        .prepare_sampled(&input.check.session_context(), capabilities, jobs)
}

/// Run through the shared client, batching, checkpoint, and lease machinery.
pub(crate) async fn run_prepared<S>(
    execution: BatchExecution<'_>,
    input: &PreparedSkillOpportunityInput,
    plan: &mut JevCheckPlan<
        <SkillOpportunitiesCheck as antiburn_local::analysis::jev::JevCheck>::Prepared,
    >,
    progress: JevRunProgress,
    orchestration: JevOrchestrationPermit,
    checkpoint: S,
) -> Result<JevExecutionOutcome<SkillOpportunitiesResult>, JevError>
where
    S: FnMut(&JevRunProgress) -> Result<String, JevError>,
{
    run_prepared_check(
        execution,
        input.check.as_ref(),
        &input.check.session_context(),
        plan,
        progress,
        orchestration,
        checkpoint,
    )
    .await
}

/// A model result cannot publish a finding without the engine's typed citation set.
pub(crate) fn publishable_finding(finding: &SkillOpportunityFinding) -> bool {
    !finding.comparison.work.is_empty()
        && !finding.evidence.is_empty()
        && !finding.comparison.use_revision.is_empty()
        && finding.revisions == SKILL_OPPORTUNITIES_REVISIONS
}

#[cfg(test)]
mod tests;
