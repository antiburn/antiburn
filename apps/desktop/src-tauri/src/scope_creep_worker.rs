//! Scope Creep adapter for the single shared Smart Burn Check worker.
//! OpenCode SQLite v2 proves current retained root content, not original retention.

use std::collections::{BTreeMap, BTreeSet};

use antiburn_local::analysis::SourceFormat;
use antiburn_local::analysis::jev::capabilities::ModelCapabilities;
use antiburn_local::analysis::jev::{
    JevCheck, JevCheckPlan, JevError, JevRunProgress, JevUsage, admit_jev_orchestration,
};
use antiburn_local::analysis::session_scope::SessionScopeError;
use antiburn_local::checks::sampling::{SamplingJob, SamplingLimits, SamplingProgress, StableId};
use antiburn_local::checks::scope_creep::{
    REVISIONS, ScopeCreepCheck, ScopeCreepFinding, ScopeCreepPrepared, ScopeCreepResult,
    ScopeCreepStatus, ScopeQuestion,
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
const CURSOR_REVISION: u32 = 1;
const POLICY: CheckPolicy = CheckPolicy {
    idle_secs: 180,
    lease_secs: 300,
    retry_delay_secs: 300,
};

pub(crate) struct ScopeCreepDescriptor;
pub(crate) const CHECK: ScopeCreepDescriptor = ScopeCreepDescriptor;

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
}

pub(crate) struct PreparedInput {
    pub(crate) durable: BurnCheckInput,
    pub(crate) check: ScopeCreepCheck,
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

pub(crate) fn prepare(
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
    let check = ScopeCreepCheck::new(snapshot.scope_creep_input(ignored_work.clone())?)
        .map_err(InputLoadError::Preparation)?;
    let plan = check
        .prepare_with_capabilities(check.context(), capabilities)
        .map_err(InputLoadError::Preparation)?;
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
        .map(|action| (action.reference.id.clone(), action.text.clone()))
        .collect();
    for occurrence in snapshot.scope().occurrences() {
        let text = serde_json::to_string(&snapshot.scope().values()[occurrence.value_index])
            .map_err(InputLoadError::Serialization)?;
        citations.insert(occurrence.reference.id.clone(), text);
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

pub(crate) fn load_input(
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
    prepare(&candidate, snapshot, capabilities, ignored, configuration)
}

fn new_sampling() -> anyhow::Result<SamplingProgress> {
    SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: 65_536,
        answers_per_candidate: 65_536 * ScopeQuestion::ALL.len(),
        judgments_per_run: 8,
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
        Some(cursor)
            if cursor.input_revision == input.input_revision
                && cursor.provider_generation == generation =>
        {
            cursor
        }
        previous => AssessmentCursor {
            revision: CURSOR_REVISION,
            input_revision: input.input_revision.clone(),
            provider_generation: generation,
            sampling: previous.and_then(|cursor| cursor.sampling),
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
    let input = match load_input(store, candidate, &capabilities) {
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
    cursor
        .sampling
        .as_mut()
        .expect("initialized")
        .synchronize(
            ScopeCreepCheck::check_identity(),
            StableId::new(
                "scope-shell-epoch",
                &[
                    input.durable.input_revision.as_bytes(),
                    &key_generation.to_be_bytes(),
                ],
            ),
            &ScopeCreepCheck::sampling_candidates(&input.plan),
        )
        .map_err(|error| anyhow::anyhow!("scope sampling inventory rejected: {error:?}"))?;
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
        cursor.result = Some(input.check.reduce(&input.plan, &[], false)?);
    }
    if input.plan.prepared.session_limitation == Some(SessionScopeError::ScopeTooLarge) {
        cursor.blocked_fit_key = Some(input.fit_key.clone());
        handle
            .with_current_generation(key_generation, || {
                if save_failure(store, &input, &cursor, "scope_context_too_large", None)? {
                    let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
                    record_assessment(
                        app,
                        candidate.historical,
                        crate::analytics::event::SmartCheckAssessmentOutcome::Abstained,
                    );
                }
                Ok::<_, anyhow::Error>(())
            })
            .transpose()?;
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
        let mut plan =
            ScopeCreepCheck::select_candidates(&input.plan, &BTreeSet::from([job.candidate]));
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
            &input.check,
            input.check.context(),
            &mut plan,
            progress,
            orchestration,
            |progress| {
                cursor.run_progress = progress.clone();
                serde_json::to_string(&cursor).map_err(|_| JevError::ProgressStorageFailure)
            },
        )
        .await?;
        if !handle.key_is_current(key_generation) {
            store.supersede_burn_check_assessment(&input.durable, unix_now())?;
            return Ok(());
        }
        cursor.run_progress = outcome.progress;
        if let Some(error) = outcome.failure {
            let rejected = matches!(error, JevError::AuthenticationRejected);
            let saved = handle
                .with_current_generation(key_generation, || {
                    let published = save_failure(
                        store,
                        &input,
                        &cursor,
                        error_category(&error),
                        Some(unix_now().saturating_add(POLICY.retry_delay_secs)),
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
        for result in cursor.run_progress.results.values() {
            if let Some(previous) = cursor
                .accepted_request_usage
                .insert(result.request_id.clone(), result.usage)
                && previous != result.usage
            {
                anyhow::bail!("scope request usage changed");
            }
        }
        if outcome.complete {
            record_completion(
                cursor.sampling.as_mut().expect("initialized"),
                &job,
                &outcome.result,
            )?;
        }
        merge_result(
            cursor.result.as_mut().expect("initialized"),
            outcome.result,
            &plan,
        );
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
        .coverage(ScopeCreepCheck::check_identity())
        .expect("synchronized");
    let result = cursor.result.as_mut().expect("initialized");
    result.assessed_candidates = coverage.completed;
    result.remaining_candidates = input
        .plan
        .prepared
        .groups
        .len()
        .saturating_sub(coverage.completed);
    result.coverage.selected_items = coverage.completed;
    result.coverage.not_selected_items = coverage.remaining;
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
) -> anyhow::Result<()> {
    if let Some(decision) = result.decisions.iter().find(|decision| {
        StableId::new("scope_work", &[decision.group_id.as_bytes()]) == job.candidate
            && decision.status != ScopeCreepStatus::Unassessed
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
    if load_input(store, candidate, capabilities)
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
        save_failure(store, input, cursor, "sampling_incomplete", None)?
    };
    Ok(published.then(|| {
        crate::analytics::event::SmartCheckAssessmentOutcome::from_evidence(has_finding, clean)
    }))
}

pub(crate) fn publication(input: &PreparedInput, assessment: ScopeCreepResult) -> Publication {
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
            decision.status != ScopeCreepStatus::Unassessed && decision.limitation.is_none()
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
                .filter(|decision| decision.status != ScopeCreepStatus::Unassessed)
                .count()
        && result.remaining_candidates
            == prepared
                .groups
                .len()
                .saturating_sub(result.assessed_candidates)
        && result.decisions.iter().all(|decision| {
            if decision.status == ScopeCreepStatus::Unassessed {
                return true;
            }
            let group = prepared
                .groups
                .iter()
                .find(|group| group.id == decision.group_id)
                .expect("validated groups");
            group.limitation.is_none()
                && !group.window_ids.is_empty()
                && decision.limitation.is_none()
                && decision.judgments.len() == group.window_ids.len()
                && [
                    ScopeQuestion::Authority,
                    ScopeQuestion::Sufficiency,
                    ScopeQuestion::Coherence,
                ]
                .iter()
                .all(|question| decision.proof_questions.contains(question))
                && group.window_ids.iter().all(|window| {
                    decision.proof_questions.iter().all(|question| {
                        decision
                            .accepted_questions
                            .get(window)
                            .is_some_and(|accepted| accepted.contains(question))
                            && decision
                                .judgments
                                .get(window)
                                .and_then(|judgments| judgments.get(question))
                                .is_some_and(|answer| {
                                    if decision.status == ScopeCreepStatus::Finding
                                        || matches!(
                                            question,
                                            ScopeQuestion::Authority
                                                | ScopeQuestion::Sufficiency
                                                | ScopeQuestion::Coherence
                                        )
                                    {
                                        answer.key() == question.positive()
                                    } else {
                                        matches!(
                                            (question, answer.key()),
                                            (ScopeQuestion::Performed, "not_performed")
                                                | (ScopeQuestion::Approval, "authorized")
                                                | (ScopeQuestion::Necessity, "necessary")
                                                | (ScopeQuestion::OptionalWork, "not_optional")
                                                | (ScopeQuestion::Materiality, "minor")
                                                | (ScopeQuestion::LaterAcceptance, "accepted")
                                        )
                                    }
                                })
                    })
                })
                && decision.proof_questions.len() >= 4
        })
        && result
            .findings
            .iter()
            .all(|finding| publishable_finding(finding, publication))
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
        && group.limitation.is_none()
        && finding.task_scope == publication.prepared.scope_bindings
        && !finding.task_scope.is_empty()
        && finding.scope_digest == publication.prepared.scope_digest
        && finding.revisions == REVISIONS
        && finding.model == publication.capabilities.model
        && finding.model_revision == publication.capabilities.model_revision
        && finding.source_generation == publication.prepared.source_generation
        && finding.publication_fence == publication.prepared.publication_fence
        && decision.status == ScopeCreepStatus::Finding
        && decision.limitation.is_none()
        && decision.proof_questions == ScopeQuestion::ALL
        && !group.window_ids.is_empty()
        && decision.judgments.len() == group.window_ids.len()
        && group.window_ids.iter().all(|window| {
            ScopeQuestion::ALL.into_iter().all(|question| {
                decision
                    .accepted_questions
                    .get(window)
                    .is_some_and(|accepted| accepted.contains(&question))
                    && decision
                        .judgments
                        .get(window)
                        .and_then(|judgments| judgments.get(&question))
                        .is_some_and(|answer| answer.key() == question.positive())
            })
        })
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
        && category.as_deref() == Some("sampling_incomplete")
        && !publication.assessment.findings.is_empty();
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
