//! Feature-owned adapter for the shared Smart Burn Check worker.

use std::collections::{BTreeMap, BTreeSet};

use antiburn_local::analysis::SourceFormat;
use antiburn_local::analysis::jev::capabilities::ModelCapabilities;
use antiburn_local::analysis::jev::{
    JevCheck, JevCheckPlan, JevError, JevEvidenceReference, JevRunProgress, JevSessionContext,
    admit_jev_orchestration,
};
use antiburn_local::checks::over_exploring::{
    self, Assessment, Decision, OverExploringCheck, PreparedAssessment, Reason, SemanticOutcome,
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
const CURSOR_REVISION: u32 = 2;
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
    revision: u32,
    input_revision: String,
    provider_generation: u64,
    sampling: Option<SamplingProgress>,
    active_job: Option<SamplingJob>,
    run_progress: JevRunProgress,
    result: Option<Assessment>,
    run_started: bool,
    run_finished: bool,
}

pub(crate) struct PreparedInput {
    pub(crate) durable: BurnCheckInput,
    pub(crate) context: JevSessionContext,
    pub(crate) plan: JevCheckPlan<PreparedAssessment>,
    snapshot_revision: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Publication {
    pub(crate) input_revision: String,
    pub(crate) snapshot_revision: String,
    pub(crate) semantic_revision: String,
    pub(crate) model: String,
    pub(crate) work_evidence: BTreeMap<String, Vec<JevEvidenceReference>>,
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
    let plan = OverExploringCheck
        .prepare_with_capabilities(&context, capabilities)
        .map_err(InputLoadError::Preparation)?;
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
    })
}

fn load_input(
    store: &Store,
    candidate: &BurnCheckCandidate,
    capabilities: &ModelCapabilities,
) -> Result<PreparedInput, InputLoadError> {
    let candidate = store
        .enrolled_burn_check_candidate(candidate, CHECK_ID)
        .map_err(InputLoadError::Storage)?;
    prepare(
        &candidate,
        store.load_smart_check_inputs(
            &candidate.session.key,
            candidate.published_fence,
            candidate.source_generation,
            DetectorInput::OverExploring,
        )?,
        capabilities,
    )
}

fn new_sampling() -> anyhow::Result<SamplingProgress> {
    SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: 256,
        answers_per_candidate: 4096 * 3 * 7,
        judgments_per_run: 8,
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
    unsupported: bool,
) -> anyhow::Result<()> {
    store.record_burn_check_candidate_issue_for_check(
        CHECK_ID,
        candidate,
        unsupported,
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
        return unavailable(store, candidate, true);
    }
    let capabilities = handle.resolve_capabilities(key_generation).await?;
    let input = match load_input(store, candidate, &capabilities) {
        Ok(input) => input,
        Err(error) => {
            ::tracing::debug!(event = "over_exploring_input_unavailable", error = ?error);
            return unavailable(store, candidate, false);
        }
    };
    if input.plan.prepared.candidates.is_empty() {
        return unavailable(store, candidate, false);
    }
    let stored = store.burn_check_assessment(&candidate.session.key, CHECK_ID)?;
    let mut cursor = restore_cursor(stored.as_ref(), &input.durable, key_generation);
    if cursor.sampling.is_none() {
        cursor.sampling = Some(new_sampling()?);
    }
    over_exploring::synchronize_sampling(
        &input.plan,
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
        let mut plan = input.plan.clone();
        PreparedAssessment::select_jobs(&mut plan, std::slice::from_ref(&job))?;
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
            &OverExploringCheck,
            &input.context,
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
        if outcome.complete
            && outcome
                .result
                .completed_episode_ids
                .contains(&job.candidate)
        {
            plan.prepared
                .record_completion(
                    &outcome.result,
                    &job,
                    cursor.sampling.as_mut().expect("initialized"),
                )
                .map_err(|error| anyhow::anyhow!("sampling completion rejected: {error:?}"))?;
        }
        merge_result(
            cursor.result.as_mut().expect("initialized"),
            outcome.result,
            job.candidate,
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
        .coverage(check_identity())
        .expect("synchronized");
    let result = cursor.result.as_mut().expect("initialized");
    result.coverage.selected_items = coverage.completed;
    result.coverage.not_selected_items = coverage.remaining;
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
    if !load_input(store, candidate, capabilities)
        .as_ref()
        .is_ok_and(|current| current.durable.input_revision == input.durable.input_revision)
    {
        store.supersede_burn_check_assessment(&input.durable, unix_now())?;
        return Ok(None);
    }
    if !save_cursor(store, &input.durable, cursor)? {
        return Ok(None);
    }
    let publication = publication(input, cursor.result.as_ref().expect("initialized").clone());
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
        store.fail_burn_check_assessment_with_result(
            &input.durable,
            &BurnCheckFailure {
                error_category: "sampling_incomplete",
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

fn merge_result(target: &mut Assessment, page: Assessment, episode: StableId) {
    target
        .findings
        .retain(|finding| finding.episode_id != episode);
    target.findings.extend(page.findings);
    target.unassessed.retain(|item| item.episode_id != episode);
    target.unassessed.extend(
        page.unassessed
            .into_iter()
            .filter(|item| item.episode_id == episode),
    );
    for (saved, incoming) in [
        (&mut target.clean_episode_ids, page.clean_episode_ids),
        (
            &mut target.completed_episode_ids,
            page.completed_episode_ids,
        ),
    ] {
        saved.retain(|id| *id != episode);
        saved.extend(incoming);
    }
    target
        .completed_work_item_ids
        .extend(page.completed_work_item_ids);
    target.completed_work_item_ids.sort();
    target.completed_work_item_ids.dedup();
}

pub(crate) fn publication(input: &PreparedInput, assessment: Assessment) -> Publication {
    let shared = input
        .plan
        .shared_context
        .as_ref()
        .expect("prepared task context");
    Publication {
        input_revision: input.durable.input_revision.clone(),
        snapshot_revision: input.snapshot_revision.clone(),
        semantic_revision: input.context.input_revision.clone(),
        model: input.plan.capabilities.model.clone(),
        work_evidence: input
            .plan
            .work_items
            .iter()
            .filter(|item| {
                assessment
                    .findings
                    .iter()
                    .any(|finding| finding.work_item_id == item.id)
            })
            .map(|item| {
                (
                    item.id.clone(),
                    shared
                        .evidence
                        .iter()
                        .chain(&item.window.evidence)
                        .cloned()
                        .collect(),
                )
            })
            .collect(),
        assessment,
    }
}

pub(crate) fn publication_has_clean_coverage(result: &Assessment) -> bool {
    let completed: BTreeSet<_> = result.completed_episode_ids.iter().copied().collect();
    let clean: BTreeSet<_> = result.clean_episode_ids.iter().copied().collect();
    let finding_episodes: BTreeSet<_> = result
        .findings
        .iter()
        .map(|finding| finding.episode_id)
        .collect();
    result.coverage.selected_items > 0
        && result.coverage.not_selected_items == 0
        && result.coverage.skipped_items == 0
        && !result.coverage.processing_limit_reached
        && result.coverage.limitations.is_empty()
        && result.unassessed.is_empty()
        && result.completed_episode_ids.len() == result.coverage.selected_items
        && completed.len() == result.completed_episode_ids.len()
        && clean.len() == result.clean_episode_ids.len()
        && clean.is_disjoint(&finding_episodes)
        && completed == clean.union(&finding_episodes).copied().collect()
        && !result.completed_work_item_ids.is_empty()
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
    let bindings: BTreeSet<_> = finding
        .reads
        .iter()
        .map(|read| (&read.request_id, &read.result_id))
        .collect();
    let judgments = finding.judgments;
    let supported = SemanticOutcome::Supported;
    finding.semantic_revision == publication.semantic_revision
        && finding.model == publication.model
        && finding.revisions == OverExploringCheck.revisions()
        && !finding.task_evidence.is_empty()
        && finding
            .task_evidence
            .iter()
            .all(|item| evidence.contains(item))
        && !finding.reads.is_empty()
        && bindings.len() == finding.reads.len()
        && finding.reads.iter().all(|read| {
            !read.output_digest.is_empty()
                && read.request_id != read.result_id
                && evidence
                    .iter()
                    .any(|item| item.source_id == read.request_id)
                && evidence.iter().any(|item| item.source_id == read.result_id)
        })
        && judgments.sufficiency == supported
        && judgments.useful_information == supported
        && judgments.later_use == supported
        && judgments.substantial == supported
        && match finding.reason {
            Reason::UnrelatedFiles => judgments.relevance == supported,
            Reason::ExcessiveFileBreadth => judgments.justified_breadth == supported,
            Reason::ExcessiveWithinFileReading => {
                judgments.relevance == SemanticOutcome::Justified
                    && judgments.justified_extent == supported
            }
        }
}

#[cfg(test)]
pub(crate) mod tests;
