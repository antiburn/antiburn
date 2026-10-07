//! Feature-owned preparation and publication checks for skill opportunities.

use std::sync::{LazyLock, Mutex};

use antiburn_local::analysis::SourceFormat;
use antiburn_local::analysis::jev::capabilities::ModelCapabilities;
use antiburn_local::analysis::jev::{
    JevCheck, JevCheckPlan, JevError, JevExecutionOutcome, JevOrchestrationPermit, JevRunProgress,
    admit_jev_orchestration,
};
use antiburn_local::checks::sampling::{SamplingJob, SamplingLimits, SamplingProgress};
use antiburn_local::checks::skill_opportunities::{
    SKILL_OPPORTUNITIES_CHECK_ID, SKILL_OPPORTUNITIES_REVISIONS, SkillAbsenceEvidence,
    SkillOpportunitiesCheck, SkillOpportunitiesResult, SkillOpportunityFinding, SkillUseLifecycle,
    SkillUseLimit, SkillUseStatus,
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
const MAX_SAMPLE_JUDGMENTS: usize = 8;
const MAX_SAMPLE_CANDIDATES: usize = 4096;
const MAX_SAMPLE_ANSWERS: usize = 8;
const MAX_SAMPLE_CHECKS: usize = 1;
const CURSOR_REVISION: u32 = 4;

static INPUT_OBSERVATIONS: LazyLock<Mutex<InventoryRevisionObserver>> =
    LazyLock::new(|| Mutex::new(InventoryRevisionObserver::default()));

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
    revision: u32,
    engine_revisions: antiburn_local::analysis::jev::JevCheckRevisions,
    input_revision: String,
    provider_generation: u64,
    sampling: Option<SamplingProgress>,
    active_job: Option<SamplingJob>,
    run_progress: JevRunProgress,
    result: Option<SkillOpportunitiesResult>,
}

impl Default for SkillCursor {
    fn default() -> Self {
        Self {
            revision: CURSOR_REVISION,
            engine_revisions: SKILL_OPPORTUNITIES_REVISIONS,
            input_revision: String::new(),
            provider_generation: 0,
            sampling: None,
            active_job: None,
            run_progress: JevRunProgress::default(),
            result: None,
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
        unavailable(store, candidate, false, handle, key_generation)?;
        return Ok(());
    };
    let Some(agent) = AgentKind::from_slug(&candidate.session.key.agent) else {
        unavailable(store, candidate, true, handle, key_generation)?;
        return Ok(());
    };
    if !matches!(
        agent,
        AgentKind::OpenCode | AgentKind::Codex | AgentKind::Claude | AgentKind::Pi
    ) || candidate.session.key.environment_key != "native"
    {
        unavailable(store, candidate, true, handle, key_generation)?;
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
        Ok(Some(cwd)) => cwd,
        Ok(None) => {
            unavailable(store, candidate, false, handle, key_generation)?;
            return Ok(());
        }
        Err(_) => {
            unavailable(store, candidate, false, handle, key_generation)?;
            return Ok(());
        }
    };
    let config = crate::agent_config::ConfigContext::native(agent, home, Some(cwd));
    let snapshot = match store.load_smart_check_inputs(
        &candidate.session.key,
        candidate.published_fence,
        candidate.source_generation,
        crate::smart_check_inputs::DetectorInput::SkillOpportunities,
    ) {
        Ok(snapshot) => snapshot,
        Err(_) => {
            unavailable(store, candidate, false, handle, key_generation)?;
            return Ok(());
        }
    };
    if source_limit(
        &candidate.session.key.agent,
        snapshot.content().source_format,
    ) != SourceLimit::Supported
    {
        unavailable(store, candidate, true, handle, key_generation)?;
        return Ok(());
    }
    let skills = match store.load_smart_check_skill_inputs(snapshot, &config) {
        Ok(skills) => skills,
        Err(_) => {
            unavailable(store, candidate, false, handle, key_generation)?;
            return Ok(());
        }
    };
    let input = match prepare(candidate, skills, CHECK.evaluator_revision()) {
        Ok(input) => input,
        Err(InputLoadError::Unavailable(_)) => {
            unavailable(store, candidate, false, handle, key_generation)?;
            return Ok(());
        }
        Err(error) => return Err(anyhow::anyhow!("skill input preparation failed: {error:?}")),
    };
    let Some(observation) = reconcile_skill_inputs(store, handle, key_generation, candidate)?
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
    synchronize_sampling(
        &input,
        cursor.sampling.as_mut().expect("sampling was initialized"),
    )
    .map_err(|error| anyhow::anyhow!("sampling inventory rejected: {error:?}"))?;
    if !cursor
        .sampling
        .as_ref()
        .and_then(|sampling| sampling.coverage(input.check.sampling_identity()))
        .is_some_and(|coverage| coverage.eligible > 0)
    {
        unavailable(store, candidate, false, handle, key_generation)?;
        return Ok(());
    }
    let capabilities = handle.resolve_capabilities(key_generation).await?;
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
        let orchestration = admit_jev_orchestration().await?;
        let mut plan = prepare_sampled(&input, &capabilities, std::slice::from_ref(&job))?;
        let mut outcome = run_prepared(
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
        .await?;
        if !handle.key_is_current(key_generation) {
            return Ok(());
        }
        if let Some(error) = outcome.failure.take() {
            cursor.run_progress = outcome.progress;
            let result_json = serde_json::to_string(&outcome.result)?;
            let progress_json = serde_json::to_string(&cursor)?;
            let saved = write_fence.commit(|| {
                let published = store.fail_burn_check_assessment_with_result(
                    &input.durable,
                    &BurnCheckFailure {
                        error_category: error_category(&error),
                        result_json: &result_json,
                        progress_json: &progress_json,
                        retry_at_epoch: Some(unix_now().saturating_add(POLICY.retry_delay_secs)),
                    },
                    unix_now(),
                    POLICY.idle_secs,
                )?;
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
        input
            .check
            .record_sampling_result(
                cursor.sampling.as_mut().expect("sampling was initialized"),
                &job,
                &outcome.result,
            )
            .map_err(|error| anyhow::anyhow!("sampling outcome rejected: {error:?}"))?;
        sampled_pairs.extend(sampled_pairs_for_job(
            &plan,
            &outcome.result,
            &job,
            candidate.incarnation,
            &input.durable.input_revision,
        ));
        merge_skill_result(
            cursor.result.get_or_insert_with(|| outcome.result.clone()),
            outcome.result,
        );
        cursor.active_job = None;
        cursor.run_progress = JevRunProgress::default();
        let progress_json = serde_json::to_string(&cursor)?;
        if !write_fence
            .commit(|| {
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
    let complete = cursor
        .sampling
        .as_ref()
        .and_then(|sampling| sampling.coverage(input.check.sampling_identity()))
        .is_some_and(|coverage| coverage.remaining == 0);
    let mut result = cursor
        .result
        .clone()
        .unwrap_or_else(|| SkillOpportunitiesResult {
            findings: Vec::new(),
            decisions: Vec::new(),
            coverage: antiburn_local::analysis::jev::JevCoverage::default(),
            complete: false,
        });
    result.complete = complete;
    if let Some(coverage) = cursor
        .sampling
        .as_ref()
        .and_then(|sampling| sampling.coverage(input.check.sampling_identity()))
    {
        result.coverage.selected_items = coverage.completed;
        result.coverage.not_selected_items = coverage.remaining;
        result.coverage.skipped_items = coverage
            .eligible
            .saturating_sub(coverage.completed + coverage.remaining);
    }
    // No completion claim is made for an empty or interrupted sampled run.
    if sampled_pairs.is_empty() && !complete {
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
    result.findings = cursor.result.as_ref().map_or_else(Vec::new, |saved| {
        saved
            .findings
            .iter()
            .filter(|finding| publishable_finding(finding))
            .cloned()
            .collect()
    });
    let result_json = publication_json(&input, &result)?;
    let progress_json = serde_json::to_string(&cursor)?;
    let pairs = sampled_pairs
        .into_iter()
        .map(|pair| pair.pair)
        .collect::<Vec<_>>();
    let published = write_fence
        .commit(|| {
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
                        error_category: "sampling_incomplete",
                        result_json: &result_json,
                        progress_json: &progress_json,
                        retry_at_epoch: None,
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
        })?
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
    !result.coverage.processing_limit_reached
        && result.coverage.selected_items > 0
        && result.coverage.selected_items >= result.decisions.len()
        && if result.complete {
            result.coverage.not_selected_items == 0
                && result.coverage.skipped_items == 0
                && result.coverage.limitations.is_empty()
        } else {
            result.coverage.not_selected_items > 0 && !result.findings.is_empty()
        }
        && result.decisions.iter().all(|decision| {
            decision.judgments.is_some()
                && decision.outcome != SkillOpportunityOutcome::Unassessed
                && (decision.outcome != SkillOpportunityOutcome::Advisory
                    || result
                        .findings
                        .iter()
                        .any(|finding| finding.comparison == decision.comparison))
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
    handle
        .with_current_generation(provider_generation, || {
            let mut observations = INPUT_OBSERVATIONS
                .lock()
                .map_err(|_| anyhow::anyhow!("skill input observation lock failed"))?;
            let Some(stored_revision) = current_assessment_revision(store, candidate)? else {
                return Ok(None);
            };
            let config = candidate_config(candidate);
            let current = match config.as_ref() {
                Some(config) => current_input_revisions(store, candidate, config)?,
                None => None,
            };
            let input_revision = current.map(|(input, _)| input);
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
    let cwd = std::path::Path::new(candidate.session.cwd.as_deref()?)
        .canonicalize()
        .ok()?;
    Some(crate::agent_config::ConfigContext::native(
        AgentKind::from_slug(&candidate.session.key.agent)?,
        home,
        Some(cwd),
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
                let current = current_input_revisions(self.store, self.candidate, self.config)?;
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
    stored
        .filter(|assessment| assessment.input_revision.as_deref() == Some(input_revision))
        .and_then(|assessment| serde_json::from_str::<SkillCursor>(&assessment.progress_json).ok())
        .filter(|cursor| {
            cursor.revision == CURSOR_REVISION
                && cursor.engine_revisions == SKILL_OPPORTUNITIES_REVISIONS
                && cursor.input_revision == input_revision
                && cursor.provider_generation == provider_generation
        })
        .unwrap_or_default()
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

fn unavailable(
    store: &Store,
    candidate: &BurnCheckCandidate,
    unsupported: bool,
    handle: &WorkerHandle,
    provider_generation: u64,
) -> anyhow::Result<()> {
    if let Some(result) = handle.with_current_generation(provider_generation, || {
        store.record_burn_check_candidate_issue_for_check(
            SKILL_OPPORTUNITIES_CHECK_ID,
            candidate,
            unsupported,
            unix_now().saturating_add(POLICY.retry_delay_secs),
            unix_now(),
        )
    }) {
        result?;
    }
    Ok(())
}

struct SampledSkillPair {
    pair: BurnCheckSampledPair,
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
    plan.prepared.comparisons.iter().map(|comparison| {
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
    for decision in page.decisions {
        if !target
            .decisions
            .iter()
            .any(|existing| existing.comparison.id == decision.comparison.id)
        {
            target.decisions.push(decision);
        }
    }
    for finding in page.findings {
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
    let enrolled = store.enrolled_burn_check_candidate(candidate, SKILL_OPPORTUNITIES_CHECK_ID)?;
    let candidate = &enrolled;
    let load = || -> Result<SkillInputs, InputLoadError> {
        let snapshot = store.load_smart_check_inputs(
            &candidate.session.key,
            candidate.published_fence,
            candidate.source_generation,
            crate::smart_check_inputs::DetectorInput::SkillOpportunities,
        )?;
        if source_limit(
            &candidate.session.key.agent,
            snapshot.content().source_format,
        ) != SourceLimit::Supported
        {
            return Err(InputLoadError::Unavailable(
                crate::smart_check_inputs::InputUnavailable::IncompleteEvidence,
            ));
        }
        store
            .load_smart_check_skill_inputs(snapshot, config)?
            .for_candidate(candidate)
    };
    let skills = match load() {
        Ok(skills) => skills,
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
        skills.input_revision().to_owned(),
        skills.inventory().revision().to_owned(),
    )))
}

/// Prepared production input. The revision binds activity, scope, skill use, and inventory.
pub(crate) struct PreparedSkillOpportunityInput {
    pub(crate) check: SkillOpportunitiesCheck,
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
        || !skill_use_is_typed_and_complete(&inputs)
    {
        return Err(InputLoadError::Unavailable(
            crate::smart_check_inputs::InputUnavailable::IncompleteEvidence,
        ));
    }

    let revision = inputs.input_revision().to_owned();
    let check = inputs.check()?;
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

fn skill_use_is_typed_and_complete(inputs: &SkillInputs) -> bool {
    let coverage = inputs.usage().coverage();
    inputs.usage().publication_fence() == Some(inputs.input().scope().publication_fence())
        && inputs.usage().events().iter().all(|event| {
            event.source_format == inputs.input().content().source_format
                && !matches!(
                    event.skill,
                    antiburn_local::checks::skill_opportunities::RecordedSkillIdentity::Unknown
                )
                && event.producer
                    != antiburn_local::checks::skill_opportunities::SkillUseProducer::Unknown
                && event.reference.stable
                && inputs.input().content().actions.iter().any(|action| {
                    action.reference == event.reference && action.tool_call_id == event.tool_call_id
                })
                && event.request_reference.as_ref().is_none_or(|reference| {
                    reference.stable
                        && reference.source_key_digest == event.reference.source_key_digest
                        && reference.thread_digest == event.reference.thread_digest
                        && inputs.input().content().actions.iter().any(|action| {
                            action.reference == *reference
                                && action.tool_call_id == event.tool_call_id
                        })
                })
        })
        && typed_lifecycle_is_complete(
            coverage.status,
            coverage
                .limitations
                .contains(&SkillUseLimit::NativeMetadataUnavailable),
            &inputs
                .usage()
                .events()
                .iter()
                .map(|event| event.lifecycle)
                .collect::<Vec<_>>(),
        )
}

fn typed_lifecycle_is_complete(
    status: SkillUseStatus,
    metadata_unavailable: bool,
    lifecycles: &[SkillUseLifecycle],
) -> bool {
    status == SkillUseStatus::Complete
        && !metadata_unavailable
        && lifecycles.iter().all(|lifecycle| {
            matches!(
                lifecycle,
                SkillUseLifecycle::Succeeded
                    | SkillUseLifecycle::Failed
                    | SkillUseLifecycle::Requested
                    | SkillUseLifecycle::DocumentSelected
            )
        })
}

/// Synchronize the semantic inventory before choosing bounded fair jobs.
/// Persist `SamplingProgress` with the feature cursor; do not call `begin_run`
/// when restoring an interrupted assessment.
pub(crate) fn synchronize_sampling(
    input: &PreparedSkillOpportunityInput,
    progress: &mut SamplingProgress,
) -> Result<(), antiburn_local::checks::sampling::SamplingError> {
    input.check.synchronize_sampling(progress)
}

/// Build a plan only for jobs chosen by the shared durable sampling ledger.
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
        &input.check,
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
        && finding.comparison.absence_assessable
        && !finding.comparison.use_revision.is_empty()
        && finding.comparison.use_eligibility.absence
            == SkillAbsenceEvidence::SelectedWindowNoMatchingUse
        && finding.comparison.work_context_assessable
}

#[cfg(test)]
mod tests;
