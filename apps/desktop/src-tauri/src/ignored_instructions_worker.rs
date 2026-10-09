//! Implements the Ignored Instructions adapter for the shared worker.

mod progress;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use antiburn_local::analysis::jev::{
    JevCheck, JevError, JevExecutionOutcome, JevRunProgress, JevSessionContext, JevWorkItem,
    admit_jev_orchestration,
};
use antiburn_local::analysis::{
    SelectedContentCursor, SelectedContentQueryError, SelectedContentRequest, SessionEvidence,
    SourceFormat, ignored_instructions,
};
use antiburn_local::platform::git;
use tauri::Emitter;

#[cfg(test)]
use crate::jev::client::TypeSafeClient;
#[cfg(test)]
use crate::jev::worker::WorkerHandle;
use crate::jev::worker::{
    BatchExecution, CandidateExecution, CheckPolicy, JevCheckDescriptor, error_category,
    run_prepared_check, unix_now, wake,
};
use crate::store::{
    BurnCheckAssessment, BurnCheckCandidate, BurnCheckFailure, BurnCheckInput,
    BurnCheckSampleOrigin, BurnCheckSampledPair, SELECTED_CONTENT_PROGRESS_REVISION,
    SelectedContentProgress, Store,
};
use antiburn_local::checks::sampling::{
    Candidate, SamplingJob, SamplingLimits, SamplingProgress, StableId,
};
use progress::{CompactCarriedComparisons, CompactProgress};

const CHECK_ID: &str = "ignored_instructions";
const IDLE_SECS: i64 = 180;
const LEASE_SECS: i64 = 300;
const RETRY_DELAY_SECS: i64 = 30 * 60;
const PAIRS_PER_TURN: usize = 8;
const SAMPLING_REVISION: u32 = 1;
const POLICY: CheckPolicy = CheckPolicy {
    idle_secs: IDLE_SECS,
    lease_secs: LEASE_SECS,
    retry_delay_secs: RETRY_DELAY_SECS,
};

struct SamplingPass<'a> {
    pairs: &'a [BurnCheckSampledPair],
    round: u32,
    backlog: bool,
    origin: &'a BurnCheckSampleOrigin,
    capabilities: &'a antiburn_local::analysis::jev::capabilities::ModelCapabilities,
    #[cfg(test)]
    legacy_fixture: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiscoveryCacheIdentity {
    session: crate::store::SessionKey,
    incarnation: u64,
    source_generation: i64,
    published_fence: i64,
    source_fingerprint: Option<String>,
    format: SourceFormat,
    cwd: Option<PathBuf>,
    home: PathBuf,
}

#[derive(Default)]
struct InstructionDiscoveryCache {
    entry: Option<(
        DiscoveryCacheIdentity,
        ignored_instructions::InstructionDiscovery,
    )>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct AssessmentCursor {
    source_fingerprint: Option<String>,
    sampling_revision: u32,
    sampling: Option<SamplingProgress>,
    active_jobs: Vec<SamplingJob>,
    required_answers: BTreeMap<String, StableId>,
    inventory_counts: BTreeMap<String, usize>,
    reviewed_ids: std::collections::BTreeSet<String>,
    terminal_ids: std::collections::BTreeSet<String>,
    context_blocked_ids: std::collections::BTreeSet<String>,
    input_revision: Option<String>,
    round: u32,
    backlog: bool,
    #[cfg(test)]
    content_offset: usize,
    comparison_after: Option<String>,
    progress: JevRunProgress,
    result: Option<ignored_instructions::AssessmentResult>,
    #[serde(default)]
    prior_findings: Vec<ignored_instructions::AssessmentFinding>,
    #[serde(default)]
    validated_prior_findings: std::collections::BTreeSet<String>,
    #[serde(default)]
    carried_comparisons: Vec<ignored_instructions::CandidateComparison>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StoredProgress {
    Compact(CompactProgress),
    Expanded(JevRunProgress),
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StoredCarriedComparisons {
    Compact(CompactCarriedComparisons),
    Expanded(Vec<ignored_instructions::CandidateComparison>),
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct StoredAssessmentCursor {
    source_fingerprint: Option<String>,
    sampling_revision: u32,
    sampling: Option<SamplingProgress>,
    active_jobs: Vec<SamplingJob>,
    required_answers: BTreeMap<String, StableId>,
    inventory_counts: BTreeMap<String, usize>,
    reviewed_ids: std::collections::BTreeSet<String>,
    terminal_ids: std::collections::BTreeSet<String>,
    context_blocked_ids: std::collections::BTreeSet<String>,
    input_revision: Option<String>,
    round: u32,
    backlog: bool,
    #[cfg(test)]
    content_offset: usize,
    comparison_after: Option<String>,
    progress: Option<StoredProgress>,
    result: Option<ignored_instructions::AssessmentResult>,
    prior_findings: Vec<ignored_instructions::AssessmentFinding>,
    validated_prior_findings: std::collections::BTreeSet<String>,
    carried_comparisons: Option<StoredCarriedComparisons>,
    selected_content: Option<SelectedContentProgress>,
}

fn parse_cursor(raw: &str) -> anyhow::Result<(AssessmentCursor, Option<CompactProgress>)> {
    let (cursor, compact, _) = parse_checkpoint(raw)?;
    Ok((cursor, compact))
}

fn parse_checkpoint(
    raw: &str,
) -> anyhow::Result<(
    AssessmentCursor,
    Option<CompactProgress>,
    Option<SelectedContentProgress>,
)> {
    let stored: StoredAssessmentCursor = serde_json::from_str(raw)?;
    let (progress, compact) = match stored.progress {
        Some(StoredProgress::Compact(compact)) => {
            anyhow::ensure!(
                compact.revision_matches(),
                "Ignored Instructions progress revision is stale"
            );
            (JevRunProgress::default(), Some(compact))
        }
        Some(StoredProgress::Expanded(progress)) => (progress, None),
        None => (JevRunProgress::default(), None),
    };
    let carried_comparisons = match stored.carried_comparisons {
        Some(StoredCarriedComparisons::Compact(compact)) => compact.restore()?,
        Some(StoredCarriedComparisons::Expanded(comparisons)) => comparisons,
        None => Vec::new(),
    };
    Ok((
        AssessmentCursor {
            source_fingerprint: stored.source_fingerprint,
            sampling_revision: stored.sampling_revision,
            sampling: stored.sampling,
            active_jobs: stored.active_jobs,
            required_answers: stored.required_answers,
            inventory_counts: stored.inventory_counts,
            reviewed_ids: stored.reviewed_ids,
            terminal_ids: stored.terminal_ids,
            context_blocked_ids: stored.context_blocked_ids,
            input_revision: stored.input_revision,
            round: stored.round,
            backlog: stored.backlog,
            #[cfg(test)]
            content_offset: stored.content_offset,
            comparison_after: stored.comparison_after,
            progress,
            result: stored.result,
            prior_findings: stored.prior_findings,
            validated_prior_findings: stored.validated_prior_findings,
            carried_comparisons,
        },
        compact,
        stored.selected_content,
    ))
}

#[cfg(test)]
fn serialize_cursor(
    cursor: &AssessmentCursor,
    work_items: &[JevWorkItem],
    context: &JevSessionContext,
) -> anyhow::Result<String> {
    serialize_cursor_progress(
        cursor,
        &cursor.progress,
        work_items,
        context,
        &mut BTreeMap::new(),
    )
}

#[cfg(test)]
fn serialize_cursor_progress(
    cursor: &AssessmentCursor,
    progress: &JevRunProgress,
    work_items: &[JevWorkItem],
    context: &JevSessionContext,
    followups: &mut BTreeMap<String, Option<JevWorkItem>>,
) -> anyhow::Result<String> {
    serialize_selected_cursor_progress(cursor, progress, work_items, context, followups, None)
}

fn serialize_selected_cursor_progress(
    cursor: &AssessmentCursor,
    progress: &JevRunProgress,
    work_items: &[JevWorkItem],
    context: &JevSessionContext,
    followups: &mut BTreeMap<String, Option<JevWorkItem>>,
    selected_content: Option<&SelectedContentProgress>,
) -> anyhow::Result<String> {
    #[derive(serde::Serialize)]
    struct Checkpoint<'a> {
        source_fingerprint: &'a Option<String>,
        sampling_revision: u32,
        sampling: &'a Option<SamplingProgress>,
        active_jobs: &'a [SamplingJob],
        required_answers: &'a BTreeMap<String, StableId>,
        inventory_counts: &'a BTreeMap<String, usize>,
        reviewed_ids: &'a std::collections::BTreeSet<String>,
        terminal_ids: &'a std::collections::BTreeSet<String>,
        context_blocked_ids: &'a std::collections::BTreeSet<String>,
        input_revision: &'a Option<String>,
        round: u32,
        backlog: bool,
        #[cfg(test)]
        content_offset: usize,
        comparison_after: &'a Option<String>,
        progress: CompactProgress,
        result: Option<&'a ignored_instructions::AssessmentResult>,
        prior_findings: &'a [ignored_instructions::AssessmentFinding],
        validated_prior_findings: &'a std::collections::BTreeSet<String>,
        carried_comparisons: CompactCarriedComparisons,
        #[serde(skip_serializing_if = "Option::is_none")]
        selected_content: Option<&'a SelectedContentProgress>,
    }
    Ok(serde_json::to_string(&Checkpoint {
        source_fingerprint: &cursor.source_fingerprint,
        sampling_revision: cursor.sampling_revision,
        sampling: &cursor.sampling,
        active_jobs: &cursor.active_jobs,
        required_answers: &cursor.required_answers,
        inventory_counts: &cursor.inventory_counts,
        reviewed_ids: &cursor.reviewed_ids,
        terminal_ids: &cursor.terminal_ids,
        context_blocked_ids: &cursor.context_blocked_ids,
        input_revision: &cursor.input_revision,
        round: cursor.round,
        backlog: cursor.backlog,
        #[cfg(test)]
        content_offset: cursor.content_offset,
        comparison_after: &cursor.comparison_after,
        progress: CompactProgress::from_progress_cached(
            &ignored_instructions::IgnoredInstructionsCheck,
            context,
            progress,
            work_items,
            followups,
        )?,
        result: selected_content.and(cursor.result.as_ref()),
        prior_findings: &cursor.prior_findings,
        validated_prior_findings: &cursor.validated_prior_findings,
        carried_comparisons: CompactCarriedComparisons::from_comparisons(
            &cursor.carried_comparisons,
        ),
        selected_content,
    })?)
}

fn restore_selected_assessment(
    assessment: Option<&BurnCheckAssessment>,
    input_revision: &str,
    backlog: bool,
) -> anyhow::Result<(
    AssessmentCursor,
    Option<CompactProgress>,
    SelectedContentProgress,
)> {
    let (mut cursor, mut compact, selected) = assessment
        .map(|assessment| parse_checkpoint(&assessment.progress_json))
        .transpose()?
        .unwrap_or_default();
    let round = cursor.round;
    let mut selected = selected.unwrap_or_default();
    if cursor.input_revision.as_deref() != Some(input_revision)
        || selected.revision != SELECTED_CONTENT_PROGRESS_REVISION
    {
        selected = SelectedContentProgress {
            revision: SELECTED_CONTENT_PROGRESS_REVISION,
            cursor: None,
        };
        compact = None;
        cursor = AssessmentCursor {
            input_revision: Some(input_revision.to_owned()),
            round: round.saturating_add(u32::from(assessment.is_some())),
            backlog,
            prior_findings: assessment.map(carried_findings).unwrap_or_default(),
            ..Default::default()
        };
    }
    Ok((cursor, compact, selected))
}

type AssessmentPosition = (bool, Option<SelectedContentCursor>, Option<String>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssessmentPageDecision {
    Continue,
    Complete,
    Incomplete,
}

fn assessment_page_decision(
    has_more_work: bool,
    old: &AssessmentPosition,
    proposed: &AssessmentPosition,
) -> AssessmentPageDecision {
    if !has_more_work {
        AssessmentPageDecision::Complete
    } else if old != proposed {
        AssessmentPageDecision::Continue
    } else {
        AssessmentPageDecision::Incomplete
    }
}

fn advance_selected_assessment_page(
    cursor: &mut AssessmentCursor,
    selected: &mut SelectedContentProgress,
    more_comparisons: bool,
    next_comparison: Option<String>,
    next_content: Option<SelectedContentCursor>,
) {
    if more_comparisons {
        cursor.comparison_after = next_comparison;
    } else if next_content.is_some() {
        selected.cursor = next_content;
        cursor.comparison_after = None;
    }
    cursor.progress = JevRunProgress::default();
}

/// Check-specific policy adapter registered with the shared Jev worker.
pub(crate) struct IgnoredInstructionsDescriptor;

pub(crate) const CHECK: IgnoredInstructionsDescriptor = IgnoredInstructionsDescriptor;

impl JevCheckDescriptor for IgnoredInstructionsDescriptor {
    fn id(&self) -> &'static str {
        CHECK_ID
    }

    fn evaluator_revision(&self) -> String {
        ignored_instructions::evaluator_revision()
    }

    fn policy(&self) -> CheckPolicy {
        POLICY
    }

    fn run_candidate<'a>(
        &'a self,
        execution: CandidateExecution<'a>,
    ) -> crate::jev::worker::WorkerFuture<'a> {
        Box::pin(run_candidate(execution))
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
    let check_generation = handle.check_generation(CHECK_ID);
    if !handle.key_is_current(key_generation)
        || !handle.check_is_current(CHECK_ID, check_generation)
        || !store.check_enabled(antiburn_local::checks::DetectorId::IgnoredInstructions)?
    {
        return Ok(());
    }
    let candidate_started = Instant::now();
    let input_preparation_started = Instant::now();
    let mut capabilities = handle.resolve_capabilities(key_generation).await?;
    if let Some(limit) = capabilities.questions_per_request.value.as_mut() {
        *limit = (*limit).min(4);
    }
    let mut discovery_cache = InstructionDiscoveryCache::default();
    let stored_before_queue = store.burn_check_assessment(&candidate.session.key, CHECK_ID)?;
    let sampling_store = store.clone();
    let sampling_key = candidate.session.key.clone();
    let sampling_incarnation = candidate.incarnation;
    let (last_round, existing_pairs) = tauri::async_runtime::spawn_blocking(move || {
        sampling_store.instruction_sampled_pairs(&sampling_key, sampling_incarnation)
    })
    .await??;
    let origin = store.observe_burn_check_sample_origin(candidate, CHECK_ID)?;
    let round = sampling_round(
        last_round,
        stored_before_queue.as_ref(),
        &candidate.source_fingerprint,
    );
    let saved_position = stored_before_queue
        .as_ref()
        .and_then(|assessment| {
            parse_checkpoint(&assessment.progress_json)
                .ok()
                .map(|checkpoint| (assessment, checkpoint))
        })
        .and_then(|(assessment, (cursor, _, selected))| {
            (cursor.input_revision.as_deref() == assessment.input_revision.as_deref()
                && selected.as_ref().is_some_and(|progress| {
                    progress.revision == SELECTED_CONTENT_PROGRESS_REVISION
                }))
            .then_some((
                selected.and_then(|progress| progress.cursor),
                cursor.comparison_after,
            ))
        })
        .filter(|(content, comparison)| content.is_some() || comparison.is_some());
    let resumed_backlog = stored_before_queue
        .as_ref()
        .and_then(|assessment| parse_cursor(&assessment.progress_json).ok())
        .is_some_and(|(cursor, _)| cursor.backlog);
    let pass = |backlog| SamplingPass {
        pairs: &existing_pairs,
        round,
        backlog,
        origin: &origin,
        capabilities: &capabilities,
        #[cfg(test)]
        legacy_fixture: false,
    };
    let (saved_content, saved_comparison) = saved_position.clone().unwrap_or_default();
    let preparation = prepare_input(
        store,
        candidate,
        saved_content.as_ref(),
        saved_comparison,
        &mut discovery_cache,
        pass(resumed_backlog),
    )
    .await;
    let stale_cursor =
        saved_position.is_some() && matches!(preparation.as_ref(), Ok(PrepareInputOutcome::Stale));
    let preparation = if stale_cursor {
        prepare_input(
            store,
            candidate,
            None,
            None,
            &mut discovery_cache,
            pass(resumed_backlog),
        )
        .await
    } else {
        preparation
    }?;
    let mut input = match preparation {
        PrepareInputOutcome::Ready(input) => *input,
        PrepareInputOutcome::Unsupported => {
            ::tracing::debug!(
                event = "ignored_instruction_candidate_skipped",
                agent = %candidate.session.key.agent,
                historical = candidate.historical,
                reason = "unsupported_source",
            );
            store.record_burn_check_candidate_issue_for_check(
                CHECK_ID,
                candidate,
                true,
                0,
                unix_now(),
            )?;
            if candidate.historical {
                crate::jev::settings::progress_changed(app);
            }
            return Ok(());
        }
        PrepareInputOutcome::Stale => return Ok(()),
        PrepareInputOutcome::Unavailable(reason) => {
            ::tracing::debug!(
                event = "ignored_instruction_candidate_skipped",
                agent = %candidate.session.key.agent,
                historical = candidate.historical,
                reason = reason.category(),
            );
            store.record_burn_check_candidate_failure_for_check(
                CHECK_ID,
                candidate,
                false,
                reason.category(),
                unix_now().saturating_add(RETRY_DELAY_SECS),
                unix_now(),
            )?;
            if candidate.historical {
                crate::jev::settings::progress_changed(app);
            }
            return Ok(());
        }
    };
    let saved_revision = stored_before_queue
        .as_ref()
        .and_then(|assessment| assessment.input_revision.as_deref());
    let resumed_revision_changed = (saved_position.is_some() || resumed_backlog)
        && saved_revision != Some(input.input_revision.as_str());
    if resumed_revision_changed {
        input = match prepare_input(
            store,
            candidate,
            None,
            None,
            &mut discovery_cache,
            pass(resumed_backlog),
        )
        .await?
        {
            PrepareInputOutcome::Ready(input) => *input,
            _ => return Ok(()),
        };
    }
    let (cursor, compact_progress, selected_progress) = restore_selected_assessment(
        stored_before_queue.as_ref(),
        &input.input_revision,
        resumed_backlog,
    )?;
    let (mut cursor, compact_progress, mut selected_progress) = if stale_cursor {
        (
            AssessmentCursor {
                input_revision: Some(input.input_revision.clone()),
                round: cursor.round.saturating_add(1),
                backlog: resumed_backlog,
                prior_findings: stored_before_queue
                    .as_ref()
                    .map(carried_findings)
                    .unwrap_or_default(),
                ..Default::default()
            },
            None,
            SelectedContentProgress {
                revision: SELECTED_CONTENT_PROGRESS_REVISION,
                cursor: None,
            },
        )
    } else {
        (cursor, compact_progress, selected_progress)
    };
    cursor.round = cursor.round.max(round);
    cursor
        .source_fingerprint
        .clone_from(&candidate.source_fingerprint);
    if let Some(reviewed) = input.context.check_context.get("reviewed_comparison_ids") {
        cursor.reviewed_ids.extend(
            serde_json::from_value::<std::collections::BTreeSet<String>>(reviewed.clone())?,
        );
    }
    ignored_instructions::extend_jev_context_with_history_and_capabilities(
        &mut input.context,
        &mut cursor.carried_comparisons,
        &input.page_actions,
        input.prior_history_complete,
        &capabilities,
    )?;
    let incarnation = input.incarnation;
    select_comparison_turn(&mut input.context, &mut cursor, incarnation, &capabilities)?;
    let orchestration = admit_jev_orchestration().await?;
    let mut prepared_plan = ignored_instructions::IgnoredInstructionsCheck
        .prepare_with_capabilities(&input.context, &capabilities)?;
    let plan = &prepared_plan.prepared;
    cursor.round = round;
    let work_items = &prepared_plan.work_items;
    if !input.more_content
        && plan.coverage.candidate_pairs == 0
        && cursor.result.is_none()
        && cursor.reviewed_ids.is_empty()
        && cursor.prior_findings.is_empty()
    {
        if store.record_burn_check_no_candidates(&input, "{}", unix_now(), IDLE_SECS)? {
            let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
        }
        return Ok(());
    }
    if let Some(compact) = compact_progress
        && !compact.is_empty()
    {
        cursor.progress = compact.restore(
            &ignored_instructions::IgnoredInstructionsCheck,
            &input.context,
            work_items,
        )?;
    }
    if cursor.progress.results.is_empty() {
        let cached = store.burn_check_work_answers(&input)?;
        let active = work_items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let mut by_scope = BTreeMap::<
            String,
            Vec<(String, antiburn_local::analysis::jev::JevWorkItemResult)>,
        >::new();
        for (scope, marker, result) in cached {
            if marker.starts_with(&format!("reuse-item:{}:", result.work_item_id)) {
                by_scope.entry(scope).or_default().push((marker, result));
            }
        }
        if let Some((scope, answers)) = by_scope.into_iter().max_by_key(|(_, answers)| {
            (
                answers
                    .iter()
                    .filter(|(_, answer)| active.contains(answer.work_item_id.as_str()))
                    .count(),
                answers.len(),
            )
        }) {
            cursor.progress.completed_batch_ids.insert(scope);
            for (marker, answer) in answers {
                cursor.progress.completed_batch_ids.insert(marker);
                cursor
                    .progress
                    .results
                    .insert(answer.work_item_id.clone(), answer);
            }
        }
    }
    if !store.queue_burn_check_assessment(&input, unix_now(), IDLE_SECS)? {
        return Ok(());
    }
    if !store.claim_burn_check_assessment(&input, unix_now(), LEASE_SECS, IDLE_SECS)? {
        ::tracing::debug!(event = "burn_check_claim_rejected", check_id = CHECK_ID);
        return Ok(());
    }
    save_scheduling(
        store,
        &input,
        &cursor,
        &plan.coverage,
        input.more_content,
        false,
    )?;
    let mut connection = handle.system_one_connection();
    connection
        .model_revision
        .clone_from(&capabilities.model_revision);
    let mut blocked_items = std::collections::BTreeSet::new();
    let mut deferred = false;
    for item in work_items {
        if cursor.progress.results.contains_key(&item.id) {
            continue;
        }
        let packed = antiburn_local::analysis::jev::pack_work_items_with_capabilities(
            std::slice::from_ref(item),
            &capabilities,
        );
        let mut terminal = !packed.skipped_item_ids.is_empty();
        for batch in packed.batches {
            let identities =
                crate::jev::worker::batch_request_identities(&connection, &input, &batch);
            match store.burn_check_dispatch_readiness(&identities, unix_now())? {
                crate::store::BurnCheckRequestAdmission::Admitted => {}
                crate::store::BurnCheckRequestAdmission::Exhausted
                | crate::store::BurnCheckRequestAdmission::Unresolved => terminal = true,
                crate::store::BurnCheckRequestAdmission::Deferred => deferred = true,
                crate::store::BurnCheckRequestAdmission::Stale => return Ok(()),
            }
        }
        if terminal {
            blocked_items.insert(item.id.clone());
            for comparison in &plan.comparisons {
                if item
                    .questions
                    .contains_key(&format!("target-{}::decision", comparison.id))
                    && cursor.terminal_ids.insert(comparison.id.clone())
                    && let Some(job) = cursor
                        .active_jobs
                        .iter()
                        .find(|job| job.candidate == comparison_identity(&comparison.id))
                {
                    if !packed.skipped_item_ids.is_empty() {
                        cursor.context_blocked_ids.insert(comparison.id.clone());
                    }
                    cursor
                        .sampling
                        .as_mut()
                        .expect("initialized")
                        .terminate_candidate(job)
                        .map_err(|error| {
                            anyhow::anyhow!("sampling termination rejected: {error:?}")
                        })?;
                }
            }
        }
    }
    save_scheduling(
        store,
        &input,
        &cursor,
        &plan.coverage,
        input.more_content,
        false,
    )?;
    if deferred {
        let checkpoint = serialize_selected_cursor_progress(
            &cursor,
            &cursor.progress,
            work_items,
            &input.context,
            &mut BTreeMap::new(),
            Some(&selected_progress),
        )?;
        store.save_burn_check_checkpoint(
            &input,
            &checkpoint,
            None,
            unix_now(),
            LEASE_SECS,
            IDLE_SECS,
        )?;
        store.release_failed_burn_check_lease(
            &input,
            "continuing",
            store
                .burn_check_next_attempt_at(&input)?
                .unwrap_or(unix_now().saturating_add(1)),
        )?;
        return Ok(());
    }
    let all_work_items = work_items.clone();
    prepared_plan
        .work_items
        .retain(|item| !blocked_items.contains(&item.id));
    let plan = &prepared_plan.prepared;
    if candidate.historical {
        crate::jev::settings::progress_changed(app);
    }
    let mut global_rules = 0usize;
    let mut global_pairs = 0usize;
    let mut global_selected = 0usize;
    let mut project_rules = 0usize;
    let mut project_pairs = 0usize;
    let mut project_selected = 0usize;
    for source in &plan.coverage.instruction_sources {
        let totals = if source.source.starts_with("home:") {
            (&mut global_rules, &mut global_pairs, &mut global_selected)
        } else if source.source.starts_with("project:") {
            (
                &mut project_rules,
                &mut project_pairs,
                &mut project_selected,
            )
        } else {
            continue;
        };
        *totals.0 = totals.0.saturating_add(source.eligible_rules);
        *totals.1 = totals.1.saturating_add(source.candidate_pairs);
        *totals.2 = totals.2.saturating_add(source.selected_comparisons);
    }
    ::tracing::debug!(
        event = "burn_check_assessment_page_planned",
        check_id = CHECK_ID,
        agent = %candidate.session.key.agent,
        historical = candidate.historical,
        comparison_count = plan.comparisons.len(),
        input_preparation_elapsed_ms = input_preparation_started.elapsed().as_millis(),
        instruction_source_count = plan.coverage.instruction_sources.len(),
        global_rules,
        global_pairs,
        global_selected,
        project_rules,
        project_pairs,
        project_selected,
    );
    let more_content = input.more_content;
    let next_content_cursor = input.next_content_cursor.clone();
    let checkpoint_work_items = all_work_items.clone();
    let checkpoint_plan = prepared_plan.clone();
    let context = input.context.clone();
    let check = ignored_instructions::IgnoredInstructionsCheck;
    let capabilities = &capabilities;
    let mut checkpoint_followups = BTreeMap::new();
    let execution_started = Instant::now();
    let mut outcome = run_prepared_check(
        BatchExecution {
            app,
            store,
            input: &input,
            client,
            handle,
            key_generation,
            events,
            policy: POLICY,
            capabilities,
        },
        &check,
        &context,
        &mut prepared_plan,
        cursor.progress.clone(),
        orchestration,
        |progress| {
            let reviewed = check.reduce(
                &checkpoint_plan,
                &progress.results.values().cloned().collect::<Vec<_>>(),
                false,
            )?;
            record_reviewed_pairs(&mut cursor, &reviewed)
                .map_err(|_| JevError::ProgressStorageFailure)?;
            save_scheduling(
                store,
                &input,
                &cursor,
                &checkpoint_plan.prepared.coverage,
                more_content,
                false,
            )
            .map_err(|_| JevError::ProgressStorageFailure)?;
            serialize_selected_cursor_progress(
                &cursor,
                progress,
                &checkpoint_work_items,
                &context,
                &mut checkpoint_followups,
                Some(&selected_progress),
            )
            .map_err(|_| JevError::InvalidCheckPlan)
        },
    )
    .await?;
    let plan = &prepared_plan.prepared;
    ::tracing::debug!(
        event = "ignored_instruction_assessment_execution_timing",
        check_id = CHECK_ID,
        agent = %candidate.session.key.agent,
        historical = candidate.historical,
        comparison_count = plan.comparisons.len(),
        request_count = outcome.progress.request_count,
        elapsed_ms = execution_started.elapsed().as_millis(),
    );
    if !handle.key_is_current(key_generation)
        || !handle.check_is_current(CHECK_ID, check_generation)
        || !store.check_enabled(antiburn_local::checks::DetectorId::IgnoredInstructions)?
    {
        store.supersede_burn_check_assessment(&input, unix_now())?;
        return Ok(());
    }
    let rejected = matches!(outcome.failure, Some(JevError::AuthenticationRejected));
    let yielded = matches!(outcome.failure, Some(JevError::Cancelled))
        && handle.turn_exhausted(CHECK_ID, &candidate.session.key);
    let permanent = outcome.failure.as_ref().is_some_and(|error| {
        matches!(
            error_category(error),
            "outcome_unknown"
                | "invalid_response"
                | "response_too_large"
                | "response_decode"
                | "response_usage_exceeded"
                | "invalid_request"
        )
    });
    if permanent {
        for item in all_work_items
            .iter()
            .filter(|item| outcome.progress.failed_item_ids.contains(&item.id))
        {
            let packed = antiburn_local::analysis::jev::pack_work_items_with_capabilities(
                std::slice::from_ref(item),
                capabilities,
            );
            let mut blocked = !packed.skipped_item_ids.is_empty()
                || matches!(
                    outcome.failure,
                    Some(
                        JevError::RequestTooLarge { .. }
                            | JevError::RequestTokenLimitExceeded { .. }
                    )
                );
            for batch in packed.batches {
                let identities =
                    crate::jev::worker::batch_request_identities(&connection, &input, &batch);
                blocked |= matches!(
                    store.burn_check_dispatch_readiness(&identities, unix_now())?,
                    crate::store::BurnCheckRequestAdmission::Unresolved
                        | crate::store::BurnCheckRequestAdmission::Exhausted
                );
            }
            if !blocked {
                continue;
            }
            for comparison in &plan.comparisons {
                if item
                    .questions
                    .contains_key(&format!("target-{}::decision", comparison.id))
                    && !cursor.reviewed_ids.contains(&comparison.id)
                    && cursor.terminal_ids.insert(comparison.id.clone())
                    && let Some(job) = cursor
                        .active_jobs
                        .iter()
                        .find(|job| job.candidate == comparison_identity(&comparison.id))
                {
                    if matches!(
                        outcome.failure,
                        Some(
                            JevError::RequestTooLarge { .. }
                                | JevError::RequestTokenLimitExceeded { .. }
                        )
                    ) {
                        cursor.context_blocked_ids.insert(comparison.id.clone());
                    }
                    cursor
                        .sampling
                        .as_mut()
                        .expect("initialized")
                        .terminate_candidate(job)
                        .map_err(|error| {
                            anyhow::anyhow!("sampling termination rejected: {error:?}")
                        })?;
                }
            }
        }
    }
    let page_result = outcome.result;
    record_reviewed_pairs(&mut cursor, &page_result)?;
    save_scheduling(store, &input, &cursor, &plan.coverage, more_content, false)?;
    let new_content_page = selected_progress.cursor.is_some() && cursor.comparison_after.is_none();
    if outcome.failure.is_none() && more_content {
        let mut carried = BTreeMap::new();
        let processed = page_result
            .coverage
            .reassessed_comparison_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        for comparison in plan.comparisons.iter().filter(|comparison| {
            !comparison.prior_history_complete && !processed.contains(&comparison.id)
        }) {
            if carried.len() == ignored_instructions::MAX_ASSESSMENT_CANDIDATES
                && !carried.contains_key(&comparison.id)
            {
                break;
            }
            carried.insert(comparison.id.clone(), comparison.clone());
        }
        cursor.carried_comparisons = carried.into_values().collect();
    } else if outcome.failure.is_none() {
        cursor.carried_comparisons.clear();
    }
    reconcile_prior_findings(
        &mut cursor.prior_findings,
        &mut cursor.validated_prior_findings,
        plan,
        &page_result,
        more_content,
    );
    let sampled_pairs = sampled_pairs_for_page(
        plan,
        &page_result,
        cursor.round,
        input.incarnation,
        capabilities,
    )?;
    let mut result = page_result;
    if input.future_only {
        result
            .coverage
            .limitations
            .retain(|limit| limit != "current_file_not_historical_proof");
    }
    if let Some(previous) = cursor.result.clone() {
        merge_assessment_page(&mut result, previous, new_content_page);
    }
    result.input_revision = input.input_revision.clone();
    let page_remaining = cursor
        .sampling
        .as_ref()
        .expect("initialized")
        .runnable_count(ignored_check_identity())
        > 0;
    let more_comparisons = plan.next_comparison_cursor.is_some();
    let start_backlog = outcome.failure.is_none()
        && !more_comparisons
        && !more_content
        && (!cursor.backlog
            || (!plan.comparisons.is_empty()
                && cursor.inventory_counts.values().sum::<usize>()
                    > cursor.reviewed_ids.len() + cursor.terminal_ids.len()))
        && !existing_pairs.is_empty()
        && !page_remaining;
    let old_position: AssessmentPosition = (
        cursor.backlog,
        selected_progress.cursor.clone(),
        cursor.comparison_after.clone(),
    );
    let proposed_position = if start_backlog {
        (true, None, None)
    } else if more_comparisons {
        (
            cursor.backlog,
            selected_progress.cursor.clone(),
            plan.next_comparison_cursor.clone(),
        )
    } else if continue_content_pass(more_content, result.coverage.selected_comparisons) {
        (cursor.backlog, next_content_cursor.clone(), None)
    } else {
        old_position.clone()
    };
    let has_more_work = more_comparisons
        || start_backlog
        || continue_content_pass(more_content, result.coverage.selected_comparisons);
    let decision = if outcome.failure.is_none() && (page_remaining || start_backlog) {
        AssessmentPageDecision::Continue
    } else {
        assessment_page_decision(
            outcome.failure.is_none() && has_more_work,
            &old_position,
            &proposed_position,
        )
    };
    let advancing = decision == AssessmentPageDecision::Continue;
    let stalled = decision == AssessmentPageDecision::Incomplete;
    if stalled {
        outcome.complete = false;
        result.coverage.sampled_pass = true;
        result
            .coverage
            .limitations
            .push("processing_incomplete".to_owned());
    }
    if outcome.failure.is_none() && more_content && !advancing {
        result.coverage.sampled_pass = true;
        result
            .coverage
            .limitations
            .push("sampled_content_selection".to_owned());
    }
    if advancing && !page_remaining {
        for finding in &mut result.findings {
            finding.limitations.retain(|limit| {
                !matches!(
                    limit.as_str(),
                    "assessment_candidate_limit"
                        | "content_part_limit"
                        | "content_byte_limit"
                        | "content_page_context_limit"
                )
            });
        }
    }
    cursor.input_revision = Some(input.input_revision.clone());
    cursor.result = Some(result.clone());
    if advancing && !page_remaining {
        if start_backlog {
            cursor.backlog = true;
            selected_progress.cursor = None;
            cursor.comparison_after = None;
            cursor.progress = JevRunProgress::default();
        } else {
            advance_selected_assessment_page(
                &mut cursor,
                &mut selected_progress,
                more_comparisons,
                plan.next_comparison_cursor.clone(),
                next_content_cursor,
            );
        }
    } else {
        cursor.progress = outcome.progress;
    }
    if outcome.failure.is_none() || permanent {
        cursor.active_jobs.clear();
        cursor.progress = JevRunProgress::default();
    }
    if outcome.failure.is_none() && !has_more_work && !page_remaining {
        save_scheduling(store, &input, &cursor, &result.coverage, more_content, true)?;
    }
    merge_prior_findings(&mut result, &cursor.prior_findings);
    result.coverage.limitations.sort();
    result.coverage.limitations.dedup();
    ::tracing::debug!(
        event = "ignored_instruction_assessment_page_result",
        check_id = CHECK_ID,
        agent = %candidate.session.key.agent,
        historical = candidate.historical,
        failure_category = outcome.failure.as_ref().map(error_category).unwrap_or("none"),
        advancing,
        complete = outcome.complete,
        request_count = result.request_count,
        finding_count = result.findings.len(),
        pending_rule_count = result.pending_rules.len(),
        unassessed_comparison_count = result.unassessed_comparisons.len(),
        selected_comparisons = plan.coverage.selected_comparisons,
        unselected_pairs = plan.coverage.unselected_pairs,
        skipped_rule_count = plan.coverage.skipped_rules.len(),
        skipped_action_count = plan.coverage.skipped_actions.len(),
        limitation_count = result.coverage.limitations.len(),
        processing_limit_reached = plan.coverage.processing_limit_reached,
        candidate_elapsed_ms = candidate_started.elapsed().as_millis(),
    );
    let serialization_started = Instant::now();
    let serialized = serde_json::to_string(&result)?;
    let serialization_elapsed_ms = serialization_started.elapsed().as_millis();
    let progress_json = serialize_selected_cursor_progress(
        &cursor,
        &cursor.progress,
        if advancing { &[] } else { &all_work_items },
        &context,
        &mut checkpoint_followups,
        Some(&selected_progress),
    )?;
    store.save_instruction_sampled_pairs(&input, &sampled_pairs)?;
    ::tracing::debug!(
        event = "burn_check_assessment_state_sizes",
        result_bytes = serialized.len(),
        progress_bytes = progress_json.len(),
        carried_comparisons = cursor.carried_comparisons.len(),
        advancing,
    );
    if !handle.key_is_current(key_generation)
        || !handle.check_is_current(CHECK_ID, check_generation)
        || !store.check_enabled(antiburn_local::checks::DetectorId::IgnoredInstructions)?
    {
        store.supersede_burn_check_assessment(&input, unix_now())?;
        return Ok(());
    }
    let exhausted_retry = matches!(outcome.failure, Some(JevError::ProviderUnavailable))
        && store.burn_check_next_attempt_at(&input)?.is_none();
    let published = if advancing || yielded || permanent || exhausted_retry {
        let Some(published) = handle.with_current_generation(key_generation, || {
            store.fail_burn_check_assessment_with_result(
                &input,
                &BurnCheckFailure {
                    error_category: "continuing",
                    result_json: &serialized,
                    progress_json: &progress_json,
                    retry_at_epoch: store.burn_check_next_attempt_at(&input)?,
                },
                unix_now(),
                IDLE_SECS,
            )
        }) else {
            return Ok(());
        };
        published?
    } else {
        let saved = handle
            .with_current_generation(key_generation, || {
                save_outcome(
                    app,
                    store,
                    SaveOutcome {
                        evidence: &input.evidence,
                        input: &input,
                        outcome: JevExecutionOutcome {
                            result,
                            progress: cursor.progress,
                            complete: outcome.complete,
                            failure: outcome.failure,
                        },
                        serialized: &serialized,
                        serialization_elapsed_ms,
                        progress_json: &progress_json,
                        historical: candidate.historical,
                        sampled_pairs: &sampled_pairs,
                    },
                )
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
    };
    if published {
        ::tracing::debug!(
            event = "checks_report_changed_emitted",
            source = "jev_worker",
            phase = "assessment_progress"
        );
        let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
        wake(app);
        store.save_instruction_sampled_pairs(&input, &sampled_pairs)?;
    }
    Ok(())
}

fn continue_content_pass(more_content: bool, selected_comparisons: usize) -> bool {
    let _ = selected_comparisons;
    more_content
}

fn ignored_check_identity() -> StableId {
    StableId::new("smart-check", &[b"ignored_instructions"])
}

fn record_reviewed_pairs(
    cursor: &mut AssessmentCursor,
    result: &ignored_instructions::AssessmentResult,
) -> anyhow::Result<()> {
    for comparison in &result.coverage.reassessed_comparison_ids {
        if !cursor.reviewed_ids.contains(comparison)
            && let Some(job) = cursor
                .active_jobs
                .iter()
                .find(|job| job.candidate == comparison_identity(comparison))
        {
            let sampling = cursor.sampling.as_mut().expect("initialized");
            sampling
                .record_reduced_answer(job, cursor.required_answers[comparison])
                .and_then(|()| sampling.complete_candidate(job))
                .map_err(|error| anyhow::anyhow!("sampling completion rejected: {error:?}"))?;
            cursor.reviewed_ids.insert(comparison.clone());
        }
    }
    Ok(())
}

fn comparison_identity(id: &str) -> StableId {
    StableId::new("ignored-instructions-pair", &[id.as_bytes()])
}

fn select_comparison_turn(
    context: &mut JevSessionContext,
    cursor: &mut AssessmentCursor,
    incarnation: u64,
    capabilities: &antiburn_local::analysis::jev::capabilities::ModelCapabilities,
) -> anyhow::Result<()> {
    let mut plan: ignored_instructions::AssessmentPlan =
        serde_json::from_value(context.check_context["assessment_plan"].clone())?;
    let inventory_id =
        ignored_instructions::sha256_hex(&serde_json::to_vec(&plan.current_action_digests)?);
    cursor
        .inventory_counts
        .insert(inventory_id, plan.coverage.candidate_pairs);
    let actions: Vec<ignored_instructions::ContentAction> =
        serde_json::from_value(context.check_context["episode_actions"].clone())?;
    if cursor.sampling_revision != SAMPLING_REVISION {
        cursor.sampling = None;
        cursor.active_jobs.clear();
        cursor.sampling_revision = SAMPLING_REVISION;
    }
    if cursor.sampling.is_none() {
        cursor.sampling = Some(
            SamplingProgress::new(SamplingLimits {
                checks: 1,
                candidates_per_check: ignored_instructions::MAX_ASSESSMENT_CANDIDATES * 2,
                answers_per_candidate: 1,
                judgments_per_run: PAIRS_PER_TURN,
            })
            .map_err(|error| anyhow::anyhow!("invalid sampling limits: {error:?}"))?,
        );
    }
    let candidates = plan
        .comparisons
        .iter()
        .map(|comparison| {
            let digest = comparison_dependency(&plan, comparison, incarnation, capabilities)?;
            let earlier = actions
                .iter()
                .filter(|action| {
                    action.reference.thread_digest == comparison.source_thread_digest
                        && action.turn_scope == comparison.source_turn_scope
                        && action.reference.turn_index < comparison.source_turn_index
                })
                .map(ignored_instructions::content_action_digest)
                .collect::<Vec<_>>();
            let answer = StableId::new(
                "ignored-instructions-answer",
                &[digest.as_bytes(), &serde_json::to_vec(&earlier)?],
            );
            if cursor
                .required_answers
                .get(&comparison.id)
                .is_some_and(|saved| *saved != answer)
            {
                cursor.reviewed_ids.remove(&comparison.id);
                cursor.terminal_ids.remove(&comparison.id);
            }
            cursor
                .required_answers
                .insert(comparison.id.clone(), answer);
            Ok(Candidate {
                id: comparison_identity(&comparison.id),
                required_answers: vec![answer],
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut chronology = plan.comparisons.iter().collect::<Vec<_>>();
    chronology.sort_by_key(|comparison| {
        (
            comparison.source_turn_index,
            comparison.action.source_order,
            comparison_identity(&comparison.id),
        )
    });
    let chronology = chronology
        .iter()
        .map(|comparison| comparison_identity(&comparison.id))
        .collect::<Vec<_>>();
    let sampling = cursor.sampling.as_mut().expect("initialized");
    sampling
        .synchronize_ordered(
            ignored_check_identity(),
            ignored_check_identity(),
            &candidates,
            &chronology,
        )
        .map_err(|error| anyhow::anyhow!("sampling inventory rejected: {error:?}"))?;
    if cursor.active_jobs.is_empty() {
        sampling.begin_run();
        while let Some(job) = sampling.choose_job() {
            cursor.active_jobs.push(job);
        }
    }
    let selected = cursor
        .active_jobs
        .iter()
        .map(|job| job.candidate)
        .collect::<std::collections::BTreeSet<_>>();
    plan.comparisons
        .retain(|comparison| selected.contains(&comparison_identity(&comparison.id)));
    plan.coverage.selected_comparisons = plan.comparisons.len();
    plan.coverage.unselected_pairs = plan
        .coverage
        .candidate_pairs
        .saturating_sub(plan.comparisons.len());
    for source in &mut plan.coverage.instruction_sources {
        source.selected_comparisons = plan
            .comparisons
            .iter()
            .filter(|comparison| comparison.reference.source == source.source)
            .count();
    }
    plan.coverage.sampled_pass |= plan.coverage.unselected_pairs > 0;
    context.check_context["assessment_plan"] = serde_json::to_value(plan)?;
    Ok(())
}

fn save_scheduling(
    store: &Store,
    input: &BurnCheckInput,
    cursor: &AssessmentCursor,
    coverage: &ignored_instructions::AssessmentCoverage,
    more_content: bool,
    exhausted: bool,
) -> anyhow::Result<bool> {
    let total = cursor
        .inventory_counts
        .values()
        .sum::<usize>()
        .max(coverage.candidate_pairs);
    let reviewed = cursor.reviewed_ids.len();
    let runnable = if exhausted {
        0
    } else {
        total
            .saturating_sub(reviewed)
            .saturating_sub(cursor.terminal_ids.len())
    };
    let enumerated = !more_content
        && inventory_enumerated(coverage)
        && cursor
            .result
            .as_ref()
            .is_none_or(|result| inventory_enumerated(&result.coverage));
    store.save_burn_check_scheduling(input, enumerated.then_some(total), reviewed, runnable)
}

fn inventory_enumerated(coverage: &ignored_instructions::AssessmentCoverage) -> bool {
    coverage.skipped_rules.is_empty()
        && coverage.skipped_actions.is_empty()
        && !coverage.processing_limit_reached
        && !coverage
            .limitations
            .iter()
            .any(|limit| limit == "instruction_scan_incomplete")
}

fn sampled_pairs_for_page(
    plan: &ignored_instructions::AssessmentPlan,
    result: &ignored_instructions::AssessmentResult,
    round: u32,
    incarnation: u64,
    capabilities: &antiburn_local::analysis::jev::capabilities::ModelCapabilities,
) -> anyhow::Result<Vec<BurnCheckSampledPair>> {
    let processed = result
        .coverage
        .reassessed_comparison_ids
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    plan.comparisons
        .iter()
        .map(|comparison| {
            let dependency_digest =
                comparison_dependency(plan, comparison, incarnation, capabilities)?;
            let coordinate = ignored_instructions::SavedComparison::from(comparison)
                .coordinate
                .expect("comparison has source coordinates");
            let dependency_digest = serde_json::to_string(&SavedComparisonDependency {
                digest: dependency_digest,
                coordinate,
            })?;
            Ok(BurnCheckSampledPair {
                comparison_id: comparison.id.clone(),
                dependency_digest,
                incarnation,
                action_id: comparison.reference.action_id.clone(),
                action_digest: comparison.reference.action_digest.clone(),
                instruction_digest: comparison.reference.instruction_digest.clone(),
                selector_revision: plan.coverage.selector_revision,
                round,
                assessed: processed.contains(&comparison.id),
            })
        })
        .collect()
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedComparisonDependency {
    digest: String,
    coordinate: ignored_instructions::ComparisonCoordinate,
}

struct SamplingDependencyLookup<'a> {
    store: &'a Store,
    input: &'a BurnCheckInput,
    round: u32,
    backlog: bool,
}

fn comparison_dependency(
    plan: &ignored_instructions::AssessmentPlan,
    comparison: &ignored_instructions::CandidateComparison,
    incarnation: u64,
    capabilities: &antiburn_local::analysis::jev::capabilities::ModelCapabilities,
) -> anyhow::Result<String> {
    Ok(ignored_instructions::sha256_hex(&serde_json::to_vec(&(
        incarnation,
        capabilities,
        plan.model_version.as_str(),
        plan.projection_revision,
        plan.chunking_revision,
        plan.question_revision,
        plan.reducer_revision,
        comparison,
    ))?))
}

fn verify_sampling_ledger(
    input: &mut ignored_instructions::PreparedAssessmentInput,
    incarnation: u64,
    pairs: &[BurnCheckSampledPair],
    ledger: &mut ignored_instructions::SamplingLedger,
    capabilities: &antiburn_local::analysis::jev::capabilities::ModelCapabilities,
    lookup: Option<SamplingDependencyLookup<'_>>,
) -> anyhow::Result<Vec<BurnCheckSampledPair>> {
    if ledger.comparison_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut saved = Vec::new();
    for pair in pairs
        .iter()
        .filter(|pair| ledger.comparison_ids.contains(&pair.comparison_id))
    {
        let dependency =
            serde_json::from_str::<SavedComparisonDependency>(&pair.dependency_digest).ok();
        saved.push(ignored_instructions::SavedComparison {
            id: pair.comparison_id.clone(),
            action_id: pair.action_id.clone(),
            instruction_digest: pair.instruction_digest.clone(),
            coordinate: dependency.map(|dependency| dependency.coordinate),
        });
    }
    let plan = input.dependency_comparisons(&saved, capabilities);
    let mut dependencies = Vec::with_capacity(plan.comparisons.len());
    for comparison in &plan.comparisons {
        let digest = comparison_dependency(&plan, comparison, incarnation, capabilities)?;
        let coordinate = ignored_instructions::SavedComparison::from(comparison)
            .coordinate
            .expect("comparison has source coordinates");
        let encoded = serde_json::to_string(&SavedComparisonDependency {
            digest: digest.clone(),
            coordinate,
        })?;
        dependencies.push((comparison.id.clone(), digest, encoded));
    }
    let matched = if let Some(lookup) = lookup {
        lookup
            .store
            .matching_instruction_sampled_pairs(
                &lookup.input.key,
                incarnation,
                lookup.round,
                lookup.backlog,
                &dependencies,
            )
            .and_then(|matched| {
                lookup.store.prune_verified_instruction_pairs(
                    lookup.input,
                    &matched,
                    &dependencies,
                )?;
                Ok(matched)
            })?
    } else {
        pairs
            .iter()
            .filter(|pair| {
                dependencies.iter().any(|(id, digest, encoded)| {
                    id == &pair.comparison_id
                        && (&pair.dependency_digest == digest || &pair.dependency_digest == encoded)
                })
            })
            .cloned()
            .collect()
    };
    let verified = matched
        .iter()
        .map(|pair| pair.comparison_id.clone())
        .collect();
    ledger.comparison_ids = verified;
    ledger.comparisons = plan
        .comparisons
        .iter()
        .filter(|comparison| ledger.comparison_ids.contains(&comparison.id))
        .map(|comparison| {
            (
                comparison.id.clone(),
                ignored_instructions::SavedComparison::from(comparison),
            )
        })
        .collect();
    Ok(matched)
}

fn sampling_ledger(
    content: &ignored_instructions::SessionContentEvidence,
    pairs: &[BurnCheckSampledPair],
    round: u32,
    incarnation: u64,
    backlog: bool,
) -> ignored_instructions::SamplingLedger {
    let actions = content
        .actions
        .iter()
        .map(|action| {
            (
                action.reference.id.as_str(),
                ignored_instructions::content_action_digest(action),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let instructions = content
        .instructions
        .iter()
        .map(|instruction| instruction.digest.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut ledger = ignored_instructions::SamplingLedger::default();
    for pair in pairs.iter().filter(|pair| {
        pair.incarnation == incarnation
            && (pair.assessed && pair.round < round || backlog && pair.round == round)
    }) {
        if actions
            .get(pair.action_id.as_str())
            .is_none_or(|digest| digest == &pair.action_digest)
        {
            ledger.known_action_ids.insert(pair.action_id.clone());
        }
        if instructions.contains(pair.instruction_digest.as_str())
            && actions.contains_key(pair.action_id.as_str())
        {
            ledger.comparison_ids.insert(pair.comparison_id.clone());
        }
    }
    ledger
}

fn sampling_round(
    last_round: u32,
    stored: Option<&BurnCheckAssessment>,
    source_fingerprint: &Option<String>,
) -> u32 {
    let cursor = stored.and_then(|assessment| {
        parse_cursor(&assessment.progress_json)
            .ok()
            .map(|(cursor, _)| cursor)
    });
    let base = last_round.max(cursor.as_ref().map_or(0, |cursor| cursor.round));
    let advance = stored.is_some_and(|assessment| assessment.status == "completed")
        || cursor.as_ref().is_some_and(|cursor| {
            cursor.source_fingerprint.as_ref() != source_fingerprint.as_ref()
        });
    base.saturating_add(u32::from(advance))
}

#[cfg(test)]
fn merge_assessment(
    result: &mut ignored_instructions::AssessmentResult,
    previous: ignored_instructions::AssessmentResult,
) {
    merge_assessment_page(result, previous, false);
}

fn merge_assessment_page(
    result: &mut ignored_instructions::AssessmentResult,
    previous: ignored_instructions::AssessmentResult,
    new_content_page: bool,
) {
    let reassessed_comparison_ids = result
        .coverage
        .reassessed_comparison_ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let reassessed_rule_ids = result
        .coverage
        .reassessed_rule_ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut findings: BTreeMap<_, _> = previous
        .findings
        .into_iter()
        .map(|finding| (finding.id.clone(), finding))
        .collect();
    for finding in result.findings.drain(..) {
        findings.insert(finding.id.clone(), finding);
    }
    result.findings = findings.into_values().collect();
    result.pending_rules.extend(
        previous
            .pending_rules
            .into_iter()
            .filter(|pending| !reassessed_rule_ids.contains(&pending.rule_id)),
    );
    result
        .pending_rules
        .sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    result.pending_rules.dedup_by(|a, b| a.rule_id == b.rule_id);
    result.unassessed_comparisons.extend(
        previous
            .unassessed_comparisons
            .into_iter()
            .filter(|comparison| !reassessed_comparison_ids.contains(comparison)),
    );
    result.unassessed_comparisons.sort();
    result.unassessed_comparisons.dedup();
    result.coverage.eligible_rules = result
        .coverage
        .eligible_rules
        .max(previous.coverage.eligible_rules);
    result.coverage.candidate_pairs = if new_content_page {
        result
            .coverage
            .candidate_pairs
            .saturating_add(previous.coverage.candidate_pairs)
    } else {
        result
            .coverage
            .candidate_pairs
            .max(previous.coverage.candidate_pairs)
    };
    result.coverage.selected_comparisons = result
        .coverage
        .selected_comparisons
        .saturating_add(previous.coverage.selected_comparisons);
    result.coverage.sampled_pass |= previous.coverage.sampled_pass;
    result.coverage.selector_revision = result
        .coverage
        .selector_revision
        .max(previous.coverage.selector_revision);
    result
        .coverage
        .reassessed_comparison_ids
        .extend(previous.coverage.reassessed_comparison_ids);
    result.coverage.reassessed_comparison_ids.sort();
    result.coverage.reassessed_comparison_ids.dedup();
    result
        .coverage
        .reassessed_rule_ids
        .extend(previous.coverage.reassessed_rule_ids);
    result.coverage.reassessed_rule_ids.sort();
    result.coverage.reassessed_rule_ids.dedup();
    for prior in &previous.coverage.instruction_sources {
        if let Some(current) = result
            .coverage
            .instruction_sources
            .iter_mut()
            .find(|current| current.source == prior.source)
        {
            current.eligible_rules = current.eligible_rules.max(prior.eligible_rules);
            current.candidate_pairs = if new_content_page {
                current
                    .candidate_pairs
                    .saturating_add(prior.candidate_pairs)
            } else {
                current.candidate_pairs.max(prior.candidate_pairs)
            };
            current.selected_comparisons = current
                .selected_comparisons
                .saturating_add(prior.selected_comparisons);
        } else {
            result.coverage.instruction_sources.push(prior.clone());
        }
    }
    result
        .coverage
        .instruction_sources
        .sort_by(|left, right| left.source.cmp(&right.source));
    result.coverage.unselected_pairs = result
        .coverage
        .candidate_pairs
        .saturating_sub(result.coverage.selected_comparisons);
    result
        .coverage
        .skipped_rules
        .extend(previous.coverage.skipped_rules);
    result.coverage.skipped_rules.sort();
    result.coverage.skipped_rules.dedup();
    result
        .coverage
        .skipped_actions
        .extend(previous.coverage.skipped_actions);
    result.coverage.skipped_actions.sort();
    result.coverage.skipped_actions.dedup();
    result
        .coverage
        .limitations
        .extend(previous.coverage.limitations.into_iter().filter(|limit| {
            !matches!(
                limit.as_str(),
                "assessment_candidate_limit"
                    | "content_part_limit"
                    | "content_byte_limit"
                    | "content_page_context_limit"
            )
        }));
    result.coverage.limitations.sort();
    result.coverage.limitations.dedup();
    result.request_count = result.request_count.saturating_add(previous.request_count);
    result.input_tokens = result.input_tokens.saturating_add(previous.input_tokens);
    result.output_tokens = result.output_tokens.saturating_add(previous.output_tokens);
}

#[cfg(test)]
fn advance_assessment_page(
    cursor: &mut AssessmentCursor,
    more_comparisons: bool,
    next_comparison_cursor: Option<String>,
    more_content: bool,
    next_content_offset: usize,
) {
    if more_comparisons {
        cursor.comparison_after = next_comparison_cursor;
    } else if more_content {
        cursor.content_offset = next_content_offset;
        cursor.comparison_after = None;
    }
    cursor.progress = JevRunProgress::default();
}

#[cfg(test)]
fn cache_hit_identity(input: &BurnCheckInput, request_digest: &str) -> String {
    ignored_instructions::sha256_hex(
        format!(
            "{}\0{}\0{}\0{}\0{}\0{}",
            input.key.environment_key,
            input.key.agent,
            input.key.session_id,
            input.check_id,
            input.input_revision,
            request_digest
        )
        .as_bytes(),
    )
}

fn carried_findings(
    assessment: &BurnCheckAssessment,
) -> Vec<ignored_instructions::AssessmentFinding> {
    let mut findings = BTreeMap::new();
    if let Ok((cursor, _)) = parse_cursor(&assessment.progress_json) {
        for finding in cursor.prior_findings {
            findings.insert(finding.id.clone(), finding);
        }
        if let Some(result) = cursor.result
            && result.input_revision == assessment.input_revision.as_deref().unwrap_or_default()
        {
            for finding in result.findings {
                findings.insert(finding.id.clone(), finding);
            }
        }
    }
    if let (Some(revision), Some(result_json)) = (
        assessment.result_revision.as_deref(),
        assessment.result_json.as_deref(),
    ) && let Ok(result) =
        serde_json::from_str::<ignored_instructions::AssessmentResult>(result_json)
        && result.input_revision == revision
    {
        for finding in result.findings {
            findings.insert(finding.id.clone(), finding);
        }
    }
    findings.into_values().collect()
}

fn reconcile_prior_findings(
    findings: &mut Vec<ignored_instructions::AssessmentFinding>,
    validated_findings: &mut std::collections::BTreeSet<String>,
    plan: &ignored_instructions::AssessmentPlan,
    page_result: &ignored_instructions::AssessmentResult,
    _more_content: bool,
) {
    let unassessed: std::collections::BTreeSet<_> = page_result
        .unassessed_comparisons
        .iter()
        .map(String::as_str)
        .collect();
    findings.retain(|finding| {
        let reference = &finding.reference;
        let current_rule = plan.current_rule_ids.contains(&(
            reference.instruction_id.clone(),
            reference.instruction_digest.clone(),
            reference.rule_id.clone(),
        ));
        if !current_rule {
            return true;
        }
        let Some(action_digest) = plan.current_action_digests.get(&reference.action_id) else {
            return true;
        };
        if !reference.action_digest.is_empty() && reference.action_digest != *action_digest {
            return false;
        }
        validated_findings.insert(finding.id.clone());
        let selected = plan.comparisons.iter().find(|comparison| {
            comparison.reference.instruction_id == reference.instruction_id
                && comparison.reference.rule_id == reference.rule_id
                && comparison.reference.action_id == reference.action_id
        });
        selected.is_none_or(|comparison| unassessed.contains(comparison.id.as_str()))
    });
    validated_findings.retain(|id| findings.iter().any(|finding| &finding.id == id));
}

fn merge_prior_findings(
    result: &mut ignored_instructions::AssessmentResult,
    previous: &[ignored_instructions::AssessmentFinding],
) {
    let mut findings: BTreeMap<_, _> = previous
        .iter()
        .cloned()
        .map(|finding| (finding.id.clone(), finding))
        .collect();
    for finding in result.findings.drain(..) {
        findings.insert(finding.id.clone(), finding);
    }
    result.findings = findings.into_values().collect();
}

#[derive(Clone)]
struct PreparedInput {
    evidence: SessionEvidence,
    context: antiburn_local::analysis::jev::JevSessionContext,
    input: BurnCheckInput,
    #[cfg(test)]
    content: ignored_instructions::SessionContentEvidence,
    #[cfg(test)]
    content_offset: usize,
    #[cfg(test)]
    boundary_positions: std::collections::BTreeMap<String, u64>,
    #[cfg(test)]
    comparison_after: Option<String>,
    #[cfg(test)]
    preparation_timings: PreparationTimings,
    prior_history_complete: bool,
    future_only: bool,
    page_actions: Vec<ignored_instructions::ContentAction>,
    more_content: bool,
    next_content_cursor: Option<SelectedContentCursor>,
    #[cfg(test)]
    next_content_offset: usize,
}

#[cfg(test)]
#[derive(Clone, Copy, Default)]
struct PreparationTimings {
    evidence_read_us: u128,
    content_query_us: u128,
    instruction_discovery_us: u128,
    normalization_us: u128,
    projection_us: u128,
    context_build_us: u128,
}

enum PrepareInputOutcome {
    Ready(Box<PreparedInput>),
    Unsupported,
    Stale,
    Unavailable(PreparationUnavailable),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreparationUnavailable {
    HomeMissing,
    EvidenceMissing,
    EvidenceMalformed,
    EvidenceNotPublished,
}

impl PreparationUnavailable {
    fn category(self) -> &'static str {
        match self {
            Self::HomeMissing => "home_unavailable",
            Self::EvidenceMissing => "input_evidence_missing",
            Self::EvidenceMalformed => "input_evidence_malformed",
            Self::EvidenceNotPublished => "input_evidence_not_published",
        }
    }
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
    content_cursor: Option<&SelectedContentCursor>,
    comparison_after: Option<String>,
    discovery_cache: &mut InstructionDiscoveryCache,
    pass: SamplingPass<'_>,
) -> anyhow::Result<PrepareInputOutcome> {
    let Some(home) = home_directory() else {
        return Ok(PrepareInputOutcome::Unavailable(
            PreparationUnavailable::HomeMissing,
        ));
    };
    prepare_selected_input_with_home(
        store,
        candidate,
        content_cursor,
        comparison_after,
        &home,
        discovery_cache,
        pass,
    )
    .await
}

async fn prepare_selected_input_with_home(
    store: &Store,
    candidate: &BurnCheckCandidate,
    content_cursor: Option<&SelectedContentCursor>,
    comparison_after: Option<String>,
    home: &Path,
    discovery_cache: &mut InstructionDiscoveryCache,
    pass: SamplingPass<'_>,
) -> anyhow::Result<PrepareInputOutcome> {
    #[cfg(test)]
    let mut preparation_timings = PreparationTimings::default();
    #[cfg(test)]
    let stage_started = Instant::now();
    let Some(evidence_row) = store.evidence(&candidate.session.key)? else {
        return Ok(PrepareInputOutcome::Unavailable(
            PreparationUnavailable::EvidenceMissing,
        ));
    };
    let source_matches: bool = store.lock().query_row(
        "SELECT EXISTS(SELECT 1 FROM session WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND incarnation = ?4 AND source_generation = ?5 AND source_fingerprint IS ?6 AND activity_cursor = ?7)",
        rusqlite::params![candidate.session.key.environment_key, candidate.session.key.agent, candidate.session.key.session_id, candidate.incarnation, candidate.source_generation, candidate.source_fingerprint, candidate.activity_cursor],
        |row| row.get(0),
    )?;
    if !source_matches {
        return Ok(PrepareInputOutcome::Stale);
    }
    if evidence_row
        .analyzed_generation
        .is_some_and(|generation| generation != candidate.source_generation)
        || evidence_row
            .published_fence
            .is_some_and(|fence| fence != candidate.published_fence)
    {
        return Ok(PrepareInputOutcome::Stale);
    }
    if evidence_row.status.as_str() != "ready" {
        return Ok(PrepareInputOutcome::Unavailable(
            PreparationUnavailable::EvidenceNotPublished,
        ));
    }
    let Some(evidence_json) = evidence_row.evidence_json else {
        return Ok(PrepareInputOutcome::Unavailable(
            PreparationUnavailable::EvidenceMissing,
        ));
    };
    let evidence: SessionEvidence = match serde_json::from_str(&evidence_json) {
        Ok(evidence) => evidence,
        Err(_) => {
            return Ok(PrepareInputOutcome::Unavailable(
                PreparationUnavailable::EvidenceMalformed,
            ));
        }
    };
    let format = evidence.capabilities.source_format;
    if !ignored_instructions::source_supported(format) {
        return Ok(PrepareInputOutcome::Unsupported);
    }
    #[cfg(test)]
    {
        preparation_timings.evidence_read_us = stage_started.elapsed().as_micros();
    }
    #[cfg(test)]
    let stage_started = Instant::now();
    let identity = DiscoveryCacheIdentity {
        session: candidate.session.key.clone(),
        incarnation: candidate.incarnation,
        source_generation: candidate.source_generation,
        published_fence: candidate.published_fence,
        source_fingerprint: candidate.source_fingerprint.clone(),
        format,
        cwd: candidate.session.cwd.as_deref().map(PathBuf::from),
        home: home.to_path_buf(),
    };
    let mut discovery = match &discovery_cache.entry {
        Some((cached_identity, discovery)) if *cached_identity == identity => discovery.clone(),
        _ => {
            discovery_cache.entry = None;
            let discovery = discover_instructions_at(&candidate.session, format, home).await;
            discovery_cache.entry = Some((identity, discovery.clone()));
            discovery
        }
    };
    let revision_instruction_digests = discovery
        .snapshots
        .iter()
        .map(|item| item.digest.clone())
        .collect::<Vec<_>>();
    let mut boundary_positions = if pass.backlog {
        pass.origin.boundary_positions.clone()
    } else {
        candidate.boundary_positions.clone()
    };
    let mut observed_at_ms = None;
    #[cfg(test)]
    let legacy_fixture = pass.legacy_fixture;
    #[cfg(not(test))]
    let legacy_fixture = false;
    if pass.backlog && !pass.origin.historical {
        discovery.snapshots.clear();
    } else if !candidate.historical
        && !pass.origin.historical
        && !legacy_fixture
        && !discovery.snapshots.is_empty()
    {
        let digest = ignored_instructions::sha256_hex(&serde_json::to_vec(
            &discovery
                .snapshots
                .iter()
                .map(|snapshot| (&snapshot.source, &snapshot.digest, snapshot.scope))
                .collect::<Vec<_>>(),
        )?);
        let (observed, at) = store.observe_burn_check_instruction_epoch(
            &candidate.session.key,
            candidate.incarnation,
            candidate.source_generation,
            &digest,
            unix_now().saturating_mul(1000),
        )?;
        observed_at_ms = Some(at);
        for (source, index) in observed {
            let position = boundary_positions.entry(source).or_default();
            *position = (*position).max(index);
        }
    } else if !candidate.historical && !pass.origin.historical && !legacy_fixture {
        discovery.snapshots.clear();
        discovery
            .limitations
            .push("instruction_observation_unavailable".to_owned());
    }
    #[cfg(test)]
    {
        preparation_timings.instruction_discovery_us = stage_started.elapsed().as_micros();
    }
    #[cfg(test)]
    let stage_started = Instant::now();
    let historical = if pass.backlog {
        pass.origin.historical
    } else {
        candidate.historical
    };
    let boundary_ms = (if pass.backlog {
        pass.origin.boundary_at_epoch
    } else {
        candidate.boundary_at_epoch
    })
    .saturating_mul(1000);
    let after_ms = if historical {
        Some(boundary_ms)
    } else {
        Some(boundary_ms.max(observed_at_ms.unwrap_or(boundary_ms)))
    };
    let page = match store.published_turn_content_keyset_selected(
        &candidate.session.key,
        SelectedContentRequest {
            source_generation: candidate.source_generation,
            after_ms,
            source_positions: &boundary_positions,
            cursor: content_cursor,
            selection: ignored_instructions::IgnoredInstructionsCheck.input_selection(),
        },
    ) {
        Ok(page) => page,
        Err(SelectedContentQueryError::StaleCursor) => return Ok(PrepareInputOutcome::Stale),
        Err(error) => return Err(error.into()),
    };
    let Some(page) = page else {
        return Ok(PrepareInputOutcome::Unavailable(
            PreparationUnavailable::EvidenceNotPublished,
        ));
    };
    let mut published = page.content;
    if published.source_generation != Some(candidate.source_generation)
        || published.publication_fence != candidate.published_fence
    {
        return Ok(PrepareInputOutcome::Stale);
    }
    let more_content = published.coverage.more_parts;
    let next_content_cursor = page.next_cursor;
    #[cfg(test)]
    let next_content_offset = published.next_offset;
    #[cfg(test)]
    {
        preparation_timings.content_query_us = stage_started.elapsed().as_micros();
    }
    if observed_at_ms.is_some() {
        published.parts.retain(|part| {
            future_instruction_turn(
                &boundary_positions,
                &part.source_key,
                part.turn_index,
                part.context_only,
            )
        });
    }
    #[cfg(test)]
    let stage_started = Instant::now();
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
    if more_content {
        content
            .limitations
            .retain(|limit| !matches!(limit.as_str(), "content_part_limit" | "content_byte_limit"));
    }
    if !discovery.scan_complete {
        content
            .limitations
            .push("instruction_scan_incomplete".to_owned());
    }
    content.limitations.sort();
    content.limitations.dedup();
    content.complete &= content.limitations.is_empty();
    #[cfg(test)]
    {
        preparation_timings.normalization_us = stage_started.elapsed().as_micros();
    }
    #[cfg(test)]
    let stage_started = Instant::now();
    content = ignored_instructions::select_session_content(
        &content,
        ignored_instructions::IgnoredInstructionsCheck.input_selection(),
    );
    #[cfg(test)]
    {
        preparation_timings.projection_us = stage_started.elapsed().as_micros();
    }
    let prior_history_complete = !more_content
        && !content.limitations.iter().any(|limitation| {
            matches!(
                limitation.as_str(),
                "branch_context_limit"
                    | "content_part_limit"
                    | "content_byte_limit"
                    | "oversized_content_part"
                    | "truncated_source_content"
            )
        });
    let digest_material = format!(
        "{}\0{}",
        content.selected_input_digest,
        content.limitations.join("\0")
    );
    content.selected_input_digest = ignored_instructions::sha256_hex(digest_material.as_bytes());
    let assessment_input = ignored_instructions::AssessmentInput {
        content,
        prior_history_complete,
        activity_after_ms: after_ms,
        boundary_positions: boundary_positions.clone(),
        source_generation: candidate.source_generation,
        source_fingerprint: candidate.source_fingerprint.clone(),
        incarnation: candidate.incarnation,
        comparison_after,
    };
    let page_actions = assessment_input.content.actions.clone();
    let prior_history_complete = assessment_input.prior_history_complete;
    #[cfg(test)]
    let stage_started = Instant::now();
    let blocking_input = assessment_input.clone();
    let blocking_pairs = pass.pairs.to_vec();
    let blocking_capabilities = pass.capabilities.clone();
    let sampling_round = pass.round;
    let sampling_backlog = pass.backlog;
    let blocking_store = store.clone();
    let blocking_fence = crate::smart_check_inputs::cache::source_fence(
        candidate,
        CHECK_ID,
        CHECK.evaluator_revision(),
    );
    let context = tauri::async_runtime::spawn_blocking(move || {
        let mut ledger = sampling_ledger(
            &blocking_input.content,
            &blocking_pairs,
            sampling_round,
            blocking_input.incarnation,
            sampling_backlog,
        );
        let incarnation = blocking_input.incarnation;
        let mut prepared =
            ignored_instructions::PreparedAssessmentInput::from_selected_input(blocking_input);
        let matched_pairs = verify_sampling_ledger(
            &mut prepared,
            incarnation,
            &blocking_pairs,
            &mut ledger,
            &blocking_capabilities,
            Some(SamplingDependencyLookup {
                store: &blocking_store,
                input: &blocking_fence,
                round: sampling_round,
                backlog: sampling_backlog,
            }),
        )?;
        let mut context = prepared.build_context(&ledger, &blocking_capabilities)?;
        let reviewed = matched_pairs
            .iter()
            .filter(|pair| pair.assessed && ledger.comparison_ids.contains(&pair.comparison_id))
            .map(|pair| pair.comparison_id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        context.check_context["reviewed_comparison_ids"] = serde_json::to_value(reviewed)?;
        anyhow::Ok(context)
    })
    .await??;
    #[cfg(test)]
    {
        preparation_timings.context_build_us = stage_started.elapsed().as_micros();
    }
    let revision = ignored_instructions::sha256_hex(
        format!(
            "{}:{}:{}:{}:{}:{}:{:?}:{}:{:?}:{}:{}:{}:{}",
            candidate.session.key.environment_key,
            candidate.session.key.agent,
            candidate.session.key.session_id,
            candidate.incarnation,
            candidate.source_generation,
            candidate.published_fence,
            candidate.source_fingerprint,
            candidate.boundary_at_epoch,
            revision_instruction_digests,
            ignored_instructions::ASSESSMENT_PROJECTION_REVISION,
            ignored_instructions::ASSESSMENT_CHUNKING_REVISION,
            ignored_instructions::ASSESSMENT_QUESTION_REVISION,
            ignored_instructions::ASSESSMENT_REDUCER_REVISION
        )
        .as_bytes(),
    );
    let revision = ignored_instructions::sha256_hex(&serde_json::to_vec(&(
        revision,
        SELECTED_CONTENT_PROGRESS_REVISION,
        antiburn_local::analysis::PARSER_REVISION,
        antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
        ignored_instructions::IgnoredInstructionsCheck.input_selection(),
        &candidate.boundary_positions,
        ignored_instructions::ASSESSMENT_MODEL,
        ignored_instructions::evaluator_revision(),
        pass.capabilities,
    ))?);
    let evaluator_revision = ignored_instructions::evaluator_revision();
    Ok(PrepareInputOutcome::Ready(Box::new(PreparedInput {
        evidence,
        input: BurnCheckInput {
            key: candidate.session.key.clone(),
            check_id: CHECK_ID.to_owned(),
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
        #[cfg(test)]
        content: assessment_input.content,
        #[cfg(test)]
        content_offset: 0,
        #[cfg(test)]
        boundary_positions: assessment_input.boundary_positions.clone(),
        #[cfg(test)]
        comparison_after: assessment_input.comparison_after.clone(),
        #[cfg(test)]
        preparation_timings,
        prior_history_complete,
        future_only: observed_at_ms.is_some(),
        page_actions,
        more_content,
        next_content_cursor,
        #[cfg(test)]
        next_content_offset,
    })))
}

fn future_instruction_turn(
    positions: &BTreeMap<String, u64>,
    source: &str,
    index: u64,
    context_only: bool,
) -> bool {
    context_only || positions.get(source).is_some_and(|before| index > *before)
}

#[cfg(test)]
async fn prepare_input_with_home(
    store: &Store,
    candidate: &BurnCheckCandidate,
    content_offset: usize,
    comparison_after: Option<String>,
    home: &Path,
    discovery_cache: &mut InstructionDiscoveryCache,
) -> anyhow::Result<PrepareInputOutcome> {
    let mut cursor = None;
    let mut consumed = 0;
    loop {
        let outcome = prepare_selected_input_with_home(
            store,
            candidate,
            cursor.as_ref(),
            comparison_after.clone(),
            home,
            discovery_cache,
            SamplingPass {
                pairs: &[],
                round: 0,
                backlog: false,
                origin: &BurnCheckSampleOrigin {
                    incarnation: candidate.incarnation,
                    boundary_positions: candidate.boundary_positions.clone(),
                    boundary_at_epoch: candidate.boundary_at_epoch,
                    historical: candidate.historical,
                },
                legacy_fixture: true,
                capabilities:
                    &antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default(),
            },
        )
        .await?;
        let PrepareInputOutcome::Ready(mut input) = outcome else {
            return Ok(outcome);
        };
        input.content_offset = consumed;
        input.next_content_offset += consumed;
        if consumed == content_offset {
            return Ok(PrepareInputOutcome::Ready(input));
        }
        anyhow::ensure!(
            input.next_content_offset <= content_offset,
            "test offset is not a page boundary"
        );
        anyhow::ensure!(
            input.next_content_cursor.is_some(),
            "test offset exceeds selected evidence"
        );
        consumed = input.next_content_offset;
        cursor = input.next_content_cursor;
    }
}

async fn discover_instructions_at(
    session: &crate::store::SessionRecord,
    format: SourceFormat,
    home: &Path,
) -> ignored_instructions::InstructionDiscovery {
    let Some(cwd) = session.cwd.as_deref().map(Path::new) else {
        return ignored_instructions::InstructionDiscovery {
            limitations: vec!["session_working_directory_unavailable".to_owned()],
            ..Default::default()
        };
    };
    let root = match git::repo_root_at(cwd).await {
        Ok(root) => root,
        Err(_) => cwd.to_path_buf(),
    };
    ignored_instructions::discover_current_instructions(
        crate::agents::kind_from_slug(&session.key.agent)
            .map(crate::agents::vendor_label)
            .unwrap_or(&session.key.agent),
        format,
        &root,
        cwd,
        home,
    )
    .await
}

struct SaveOutcome<'a> {
    evidence: &'a SessionEvidence,
    input: &'a BurnCheckInput,
    outcome: JevExecutionOutcome<ignored_instructions::AssessmentResult>,
    serialized: &'a str,
    serialization_elapsed_ms: u128,
    progress_json: &'a str,
    historical: bool,
    sampled_pairs: &'a [BurnCheckSampledPair],
}

fn save_outcome(
    app: &tauri::AppHandle,
    store: &Store,
    save: SaveOutcome<'_>,
) -> anyhow::Result<()> {
    let SaveOutcome {
        evidence,
        input,
        outcome,
        serialized,
        serialization_elapsed_ms,
        progress_json,
        historical,
        sampled_pairs,
    } = save;
    let failed = outcome.failure.is_some();
    let canceled = matches!(outcome.failure, Some(JevError::Cancelled));
    let assessment_outcome = if failed {
        crate::analytics::event::SmartCheckAssessmentOutcome::Failed
    } else {
        let findings = crate::insights_report::ignored_instruction_findings_for_evidence(
            evidence,
            &outcome.result,
        );
        crate::analytics::event::SmartCheckAssessmentOutcome::from_evidence(
            findings
                .as_ref()
                .is_some_and(|findings| !findings.is_empty()),
            findings.as_ref().is_some_and(|findings| {
                crate::insights_report::ignored_result_has_scoped_no_issues(
                    &outcome.result,
                    findings,
                )
            }),
        )
    };
    ::tracing::debug!(
        event = "ignored_instruction_assessment_finished",
        check_id = CHECK_ID,
        agent = %input.key.agent,
        historical,
        outcome = if failed { "failed" } else { "completed" },
        failure_category = outcome.failure.as_ref().map(error_category).unwrap_or("none"),
        complete = outcome.complete,
        request_count = outcome.result.request_count,
        finding_count = outcome.result.findings.len(),
        pending_rule_count = outcome.result.pending_rules.len(),
        unassessed_comparison_count = outcome.result.unassessed_comparisons.len(),
        limitation_count = outcome.result.coverage.limitations.len(),
    );
    #[cfg(not(feature = "analytics"))]
    let _ = (historical, failed);
    let store_started = Instant::now();
    let published = if let Some(error) = outcome.failure {
        let retry_at = store.burn_check_next_attempt_at(input)?;
        store.fail_burn_check_assessment_with_result(
            input,
            &BurnCheckFailure {
                error_category: error_category(&error),
                result_json: serialized,
                progress_json,
                retry_at_epoch: retry_at,
            },
            unix_now(),
            IDLE_SECS,
        )?
    } else {
        store.complete_burn_check_assessment(input, serialized, unix_now(), IDLE_SECS)?
    };
    let store_elapsed_ms = store_started.elapsed().as_millis();
    let event_started = Instant::now();
    if published {
        if !failed {
            store.save_instruction_sampled_pairs(input, sampled_pairs)?;
        }
        ::tracing::debug!(
            event = "checks_report_changed_emitted",
            source = "jev_worker",
            phase = "assessment_terminal",
            failed
        );
        let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
        if !canceled {
            crate::analytics::record_smart_check_lifecycle(
                app,
                crate::analytics::event::SmartCheckLifecycle::Assessment {
                    check: crate::analytics::event::SmartCheck::IgnoredInstructions,
                    outcome: assessment_outcome,
                    historical,
                },
            );
        }
    }
    ::tracing::debug!(
        event = "ignored_instruction_assessment_publication_timing",
        result_bytes = serialized.len(),
        serialization_elapsed_ms,
        store_elapsed_ms,
        event_elapsed_ms = event_started.elapsed().as_millis(),
        published,
    );
    Ok(())
}

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod evals;

#[cfg(test)]
mod settings_tests {
    use super::*;
    use crate::store::SessionKey;
    use antiburn_local::analysis::ignored_instructions::{
        AssessmentCoverage, AssessmentFinding, AssessmentPlan, AssessmentResult,
        CandidateComparison, CounterEvidence, FindingCertainty, InstructionProvenance,
        InstructionScope, RuleActionRef,
    };

    fn selected_cursor(turn_index: i64) -> SelectedContentCursor {
        serde_json::from_value(serde_json::json!({
            "revision": 1,
            "input_identity": "selected-publication",
            "position": {"source_key": "source", "turn_index": turn_index,
                "turn_rowid": turn_index + 1, "part_index": 0}
        }))
        .unwrap()
    }

    #[test]
    fn skipped_inventories_keep_scheduling_totals_unknown_and_exhaustion_stops_turns() {
        for skipped in ["rule", "action", "prior_rule", "prior_action"] {
            let (store, input) = scheduling_fixture();
            let mut coverage = result(&[]).coverage;
            coverage.candidate_pairs = 4;
            let mut cursor = AssessmentCursor {
                inventory_counts: BTreeMap::from([("page".into(), 4)]),
                reviewed_ids: std::collections::BTreeSet::from(["reviewed".into()]),
                ..Default::default()
            };
            let skipped_coverage = if skipped.starts_with("prior_") {
                cursor.result = Some(result(&[]));
                &mut cursor.result.as_mut().unwrap().coverage
            } else {
                &mut coverage
            };
            if skipped.ends_with("rule") {
                skipped_coverage
                    .skipped_rules
                    .push("unavailable-rule".into());
            } else {
                skipped_coverage
                    .skipped_actions
                    .push("unavailable-action".into());
            }
            assert!(save_scheduling(&store, &input, &cursor, &coverage, false, false).unwrap());
            assert_eq!(scheduling_counts(&store, &input), (None, 1, 3), "{skipped}");
            store
                .fail_burn_check_assessment_with_result(
                    &input,
                    &BurnCheckFailure {
                        error_category: "continuing",
                        result_json: "{}",
                        progress_json: "{}",
                        retry_at_epoch: None,
                    },
                    1_000,
                    IDLE_SECS,
                )
                .unwrap();
            assert!(scheduling_candidates(&store, &input));
            assert!(save_scheduling(&store, &input, &cursor, &coverage, false, true).unwrap());
            assert_eq!(scheduling_counts(&store, &input), (None, 1, 0), "{skipped}");
            assert!(!scheduling_candidates(&store, &input), "{skipped}");
        }
    }

    #[test]
    fn fully_enumerated_inventory_persists_known_scheduling_counts() {
        let (store, input) = scheduling_fixture();
        let mut coverage = result(&[]).coverage;
        coverage.candidate_pairs = 4;
        let cursor = AssessmentCursor {
            inventory_counts: BTreeMap::from([("page".into(), 4)]),
            reviewed_ids: std::collections::BTreeSet::from(["reviewed".into()]),
            terminal_ids: std::collections::BTreeSet::from(["terminal".into()]),
            ..Default::default()
        };
        assert!(save_scheduling(&store, &input, &cursor, &coverage, true, false).unwrap());
        assert_eq!(scheduling_counts(&store, &input), (None, 1, 2));
        assert!(save_scheduling(&store, &input, &cursor, &coverage, false, false).unwrap());
        assert_eq!(scheduling_counts(&store, &input), (Some(4), 1, 2));
        assert!(save_scheduling(&store, &input, &cursor, &coverage, false, true).unwrap());
        assert_eq!(scheduling_counts(&store, &input), (Some(4), 1, 0));
    }

    fn scheduling_fixture() -> (Store, BurnCheckInput) {
        let (store, candidate) = crate::over_exploring_worker::tests::fixture(None, "user");
        store
            .set_check_enabled(
                antiburn_local::checks::DetectorId::IgnoredInstructions,
                true,
            )
            .unwrap();
        store.capture_burn_check_boundaries(&[CHECK_ID], 0).unwrap();
        let candidate = store
            .enrolled_burn_check_candidate(&candidate, CHECK_ID)
            .unwrap();
        let input = BurnCheckInput {
            key: candidate.session.key,
            check_id: CHECK_ID.into(),
            incarnation: candidate.incarnation,
            source_generation: candidate.source_generation,
            source_fingerprint: candidate.source_fingerprint,
            activity_cursor: candidate.activity_cursor,
            published_fence: candidate.published_fence,
            input_revision: "scheduling-counts".into(),
            evaluator_revision: CHECK.evaluator_revision(),
            boundary_at_epoch: candidate.boundary_at_epoch,
        };
        assert!(
            store
                .queue_burn_check_assessment(&input, 1_000, IDLE_SECS)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&input, 1_000, LEASE_SECS, IDLE_SECS)
                .unwrap()
        );
        (store, input)
    }

    fn scheduling_counts(store: &Store, input: &BurnCheckInput) -> (Option<usize>, usize, usize) {
        store.lock().query_row(
            "SELECT eligible_targets, reviewed_targets, runnable_targets FROM burn_check_assessment
                WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND check_id = ?4",
            rusqlite::params![input.key.environment_key, input.key.agent, input.key.session_id, input.check_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap()
    }

    fn scheduling_candidates(store: &Store, input: &BurnCheckInput) -> bool {
        store
            .burn_check_candidates_for_revision(
                CHECK_ID,
                &input.evaluator_revision,
                1_000,
                IDLE_SECS,
                16,
            )
            .unwrap()
            .iter()
            .any(|candidate| candidate.session.key == input.key)
    }

    #[test]
    fn source_positions_exclude_old_timestamped_actions_after_an_edit() {
        let positions = BTreeMap::from([("source".to_owned(), 2)]);
        assert!(!future_instruction_turn(&positions, "source", 1, false));
        assert!(!future_instruction_turn(&positions, "source", 2, false));
        assert!(future_instruction_turn(&positions, "source", 3, false));
        assert!(!future_instruction_turn(&positions, "new-source", 0, false));
        assert!(future_instruction_turn(&positions, "source", 1, true));
    }

    #[test]
    fn keyset_checkpoint_preserves_page_results_citations_and_comparison_resume() {
        let context = JevSessionContext {
            input_revision: "revision".into(),
            session_identity: "session".into(),
            check_context: serde_json::Value::Null,
            limitations: Vec::new(),
            evidence_store: Default::default(),
            reference_snapshots: Vec::new(),
        };
        let mut cursor = AssessmentCursor {
            input_revision: Some("revision".into()),
            result: Some(result(&["prior-page-action"])),
            ..Default::default()
        };
        let mut selected = SelectedContentProgress {
            revision: SELECTED_CONTENT_PROGRESS_REVISION,
            cursor: Some(selected_cursor(255)),
        };
        advance_selected_assessment_page(
            &mut cursor,
            &mut selected,
            true,
            Some("comparison-2".into()),
            Some(selected_cursor(511)),
        );
        assert_eq!(selected.cursor, Some(selected_cursor(255)));
        let raw = serialize_selected_cursor_progress(
            &cursor,
            &cursor.progress,
            &[],
            &context,
            &mut BTreeMap::new(),
            Some(&selected),
        )
        .unwrap();
        let assessment = BurnCheckAssessment {
            key: SessionKey::new("native", "claude-code", "session"),
            check_id: CHECK_ID.into(),
            input_revision: Some("revision".into()),
            status: "failed".into(),
            progress_json: raw,
            result_json: None,
            result_revision: None,
            request_count: 0,
        };
        let (mut restored, compact, mut selected) =
            restore_selected_assessment(Some(&assessment), "revision", false).unwrap();
        assert!(compact.is_some());
        assert_eq!(restored.comparison_after.as_deref(), Some("comparison-2"));
        assert_eq!(restored.result, cursor.result);
        assert_eq!(selected.cursor, Some(selected_cursor(255)));
        advance_selected_assessment_page(
            &mut restored,
            &mut selected,
            false,
            None,
            Some(selected_cursor(511)),
        );
        assert_eq!(selected.cursor, Some(selected_cursor(511)));
        assert!(restored.comparison_after.is_none());
        let mut next_result = result(&["next-page-action"]);
        merge_assessment_page(&mut next_result, restored.result.unwrap(), false);
        assert_eq!(next_result.findings.len(), 2);
        assert!(
            next_result
                .findings
                .iter()
                .any(|finding| finding.reference.action_id == "prior-page-action")
        );
    }

    #[test]
    fn backlog_transition_survives_a_checkpoint_without_reusing_page_answers() {
        let context = JevSessionContext {
            input_revision: "revision".into(),
            session_identity: "session".into(),
            check_context: serde_json::Value::Null,
            limitations: Vec::new(),
            evidence_store: Default::default(),
            reference_snapshots: Vec::new(),
        };
        let mut cursor = AssessmentCursor {
            input_revision: Some("revision".into()),
            round: 2,
            result: Some(result(&["prior"])),
            ..Default::default()
        };
        let mut selected = SelectedContentProgress {
            revision: SELECTED_CONTENT_PROGRESS_REVISION,
            cursor: Some(selected_cursor(255)),
        };
        cursor.backlog = true;
        selected.cursor = None;
        advance_selected_assessment_page(&mut cursor, &mut selected, false, None, None);
        let saved = serialize_selected_cursor_progress(
            &cursor,
            &cursor.progress,
            &[],
            &context,
            &mut BTreeMap::new(),
            Some(&selected),
        )
        .unwrap();
        let (restored, compact, position) = parse_checkpoint(&saved).unwrap();
        assert!(restored.backlog);
        assert_eq!(restored.round, 2);
        assert_eq!(restored.result.unwrap().findings.len(), 1);
        assert!(compact.unwrap().is_empty());
        assert!(position.unwrap().cursor.is_none());
    }

    #[test]
    fn legacy_and_changed_keyset_progress_reset_work_but_keep_prior_findings() {
        let previous = result(&["prior-action"]);
        for selected_content in [
            serde_json::Value::Null,
            serde_json::json!({"revision": SELECTED_CONTENT_PROGRESS_REVISION + 1, "cursor": null}),
        ] {
            let assessment = BurnCheckAssessment {
                key: SessionKey::new("native", "claude-code", "session"),
                check_id: CHECK_ID.into(),
                input_revision: Some("revision".into()),
                status: "failed".into(),
                progress_json:
                    serde_json::json!({"input_revision": "revision", "content_offset": 256,
                    "comparison_after": "old-comparison", "selected_content": selected_content,
                    "result": previous})
                    .to_string(),
                result_json: Some(serde_json::to_string(&previous).unwrap()),
                result_revision: Some("revision".into()),
                request_count: 1,
            };
            let mut assessment = assessment;
            if selected_content.is_null() {
                let mut value: serde_json::Value =
                    serde_json::from_str(&assessment.progress_json).unwrap();
                value.as_object_mut().unwrap().remove("selected_content");
                assessment.progress_json = value.to_string();
            }
            let (cursor, compact, selected) =
                restore_selected_assessment(Some(&assessment), "revision", false).unwrap();
            assert!(compact.is_none());
            assert!(selected.cursor.is_none());
            assert_eq!(selected.revision, SELECTED_CONTENT_PROGRESS_REVISION);
            assert!(cursor.comparison_after.is_none());
            assert_eq!(cursor.progress, JevRunProgress::default());
            assert!(cursor.result.is_none());
            assert_eq!(cursor.prior_findings, previous.findings);
        }
    }

    #[test]
    fn instruction_discovery_cache_identity_is_fenced_to_assessment_inputs() {
        let identity = DiscoveryCacheIdentity {
            session: SessionKey::new("native", "claude-code", "session"),
            incarnation: 3,
            source_generation: 7,
            published_fence: 11,
            source_fingerprint: Some("source-a".to_owned()),
            format: SourceFormat::ClaudeJsonl,
            cwd: Some(PathBuf::from("/project")),
            home: PathBuf::from("/home/user"),
        };
        assert_eq!(identity, identity.clone());
        for changed in [
            DiscoveryCacheIdentity {
                published_fence: 12,
                ..identity.clone()
            },
            DiscoveryCacheIdentity {
                source_generation: 8,
                ..identity.clone()
            },
            DiscoveryCacheIdentity {
                incarnation: 4,
                ..identity.clone()
            },
            DiscoveryCacheIdentity {
                source_fingerprint: Some("source-b".to_owned()),
                ..identity.clone()
            },
            DiscoveryCacheIdentity {
                home: PathBuf::from("/other-home"),
                ..identity.clone()
            },
        ] {
            assert_ne!(identity, changed);
        }
    }

    #[test]
    fn sanitized_failure_categories_keep_transport_and_answer_errors_distinct() {
        assert_eq!(
            error_category(&JevError::RequestTokenLimitExceeded {
                tokens: 4097,
                maximum: 4096,
            }),
            "invalid_request"
        );
        assert_eq!(
            error_category(&JevError::RateLimited { retry_after: None }),
            "rate_limited"
        );
        assert_eq!(error_category(&JevError::ResponseDecode), "response_decode");
        assert_eq!(
            error_category(&JevError::InvalidProbabilitySum),
            "invalid_response"
        );
        assert_eq!(
            error_category(&JevError::InvalidCheckPlan),
            "invalid_assessment_plan"
        );
    }

    fn result(ids: &[&str]) -> AssessmentResult {
        AssessmentResult {
            input_revision: "revision".into(),
            model_version: ignored_instructions::ASSESSMENT_MODEL.into(),
            findings: ids
                .iter()
                .map(|id| AssessmentFinding {
                    decision: None,
                    id: (*id).into(),
                    reference: RuleActionRef {
                        instruction_id: "instruction".into(),
                        instruction_digest: "digest".into(),
                        rule_id: "rule".into(),
                        rule_heading: "Rule".into(),
                        start_line: 1,
                        end_line: 1,
                        source: "project:AGENTS.md".into(),
                        provenance: InstructionProvenance::RecordedInjection,
                        scope: InstructionScope::Project,
                        action_id: (*id).into(),
                        action_digest: format!("digest-{id}"),
                        action_timestamp_ms: Some(1),
                        action_stable: true,
                    },
                    instruction_excerpt: "Test instruction.".into(),
                    instruction_excerpt_truncated: false,
                    action_excerpt: "Test action.".into(),
                    action_excerpt_truncated: false,
                    nearby_context_ids: Vec::new(),
                    counterevidence_ids: Vec::new(),
                    certainty: FindingCertainty::Possible,
                    composite_probability: 0.8,
                    limitations: Vec::new(),
                })
                .collect(),
            pending_rules: Vec::new(),
            unassessed_comparisons: Vec::new(),
            coverage: AssessmentCoverage {
                eligible_rules: 1,
                candidate_pairs: ids.len(),
                selected_comparisons: ids.len(),
                unselected_pairs: 0,
                skipped_rules: Vec::new(),
                skipped_actions: Vec::new(),
                processing_limit_reached: false,
                sampled_pass: false,
                selector_revision: 1,
                limitations: Vec::new(),
                reassessed_comparison_ids: Vec::new(),
                reassessed_rule_ids: Vec::new(),
                reassessed_finding_ids: Vec::new(),
                instruction_sources: Vec::new(),
            },
            request_count: ids.len() as u32,
            input_tokens: 10,
            output_tokens: 2,
        }
    }

    fn plan(reference: &RuleActionRef) -> AssessmentPlan {
        AssessmentPlan {
            input_revision: "new-revision".into(),
            observable_obligations: BTreeMap::new(),
            read_request_orders: BTreeMap::new(),
            earlier_read_only_actions: BTreeMap::new(),
            session_identity_digest: "session".into(),
            source_generation: 2,
            source_fingerprint: Some("fingerprint".into()),
            publication_fence: 2,
            activity_after_ms: None,
            model_version: ignored_instructions::ASSESSMENT_MODEL.into(),
            projection_revision: ignored_instructions::ASSESSMENT_PROJECTION_REVISION,
            chunking_revision: ignored_instructions::ASSESSMENT_CHUNKING_REVISION,
            question_revision: ignored_instructions::ASSESSMENT_QUESTION_REVISION,
            reducer_revision: ignored_instructions::ASSESSMENT_REDUCER_REVISION,
            complete_input: true,
            comparisons: vec![CandidateComparison {
                source_binding: None,
                prerequisite_episode: None,
                id: "comparison".into(),
                reference: reference.clone(),
                source_thread_digest: "thread".into(),
                source_turn_index: 1,
                source_turn_scope: "main".into(),
                rule_text: "Rule".into(),
                instruction_context: Vec::new(),
                rule_text_start: 0,
                rule_text_end: "Rule".len(),
                action: CounterEvidence {
                    action_id: reference.action_id.clone(),
                    source_order: 1,
                    role: "assistant".into(),
                    kind: "assistant_text".into(),
                    timestamp_ms: Some(1),
                    tool_name: None,
                    text: "Action".into(),
                    truncated: false,
                },
                action_text_start: 0,
                action_text_end: "Action".len(),
                context: Vec::new(),
                context_truncated: false,
                counterevidence: Vec::new(),
                earlier_history_truncated: false,
                prior_history_complete: true,
            }],
            current_action_digests: BTreeMap::from([(
                reference.action_id.clone(),
                reference.action_digest.clone(),
            )]),
            current_rule_ids: std::collections::BTreeSet::from([(
                reference.instruction_id.clone(),
                reference.instruction_digest.clone(),
                reference.rule_id.clone(),
            )]),
            next_comparison_cursor: None,
            coverage: AssessmentCoverage {
                eligible_rules: 1,
                candidate_pairs: 1,
                selected_comparisons: 1,
                unselected_pairs: 0,
                skipped_rules: Vec::new(),
                skipped_actions: Vec::new(),
                processing_limit_reached: false,
                sampled_pass: false,
                selector_revision: 1,
                limitations: Vec::new(),
                reassessed_comparison_ids: Vec::new(),
                reassessed_rule_ids: Vec::new(),
                reassessed_finding_ids: Vec::new(),
                instruction_sources: Vec::new(),
            },
        }
    }

    #[test]
    fn previous_results_survive_while_a_new_revision_is_queued() {
        let previous = result(&["action"]);
        let assessment = BurnCheckAssessment {
            key: SessionKey::new("native", "claude-code", "session"),
            check_id: CHECK_ID.to_owned(),
            input_revision: Some("new-revision".to_owned()),
            status: "queued".to_owned(),
            progress_json: "{}".to_owned(),
            result_json: Some(serde_json::to_string(&previous).unwrap()),
            result_revision: Some("revision".to_owned()),
            request_count: 0,
        };

        let carried = carried_findings(&assessment);

        assert_eq!(carried.len(), 1);
        assert_eq!(carried[0].id, "action");
    }

    #[test]
    fn prior_page_result_is_stored_once_and_restored_separately_from_progress() {
        let mut previous = result(&[]);
        previous.unassessed_comparisons = (0..16_000)
            .map(|index| ignored_instructions::sha256_hex(format!("comparison-{index}").as_bytes()))
            .collect();
        let result_json = serde_json::to_string(&previous).unwrap();
        assert!(result_json.len() < 1024 * 1024);
        let cursor = AssessmentCursor {
            input_revision: Some(previous.input_revision.clone()),
            result: Some(previous.clone()),
            ..AssessmentCursor::default()
        };
        let context = JevSessionContext {
            input_revision: "revision".to_owned(),
            session_identity: "test".to_owned(),
            check_context: serde_json::Value::Null,
            limitations: Vec::new(),
            evidence_store: antiburn_local::analysis::jev::JevEvidenceStore::default(),
            reference_snapshots: Vec::new(),
        };
        let saved = serialize_cursor(&cursor, &[], &context).unwrap();
        assert!(saved.len() < 512 * 1024);
        let (mut restored, checkpoint) = parse_cursor(&saved).unwrap();
        let checkpoint = checkpoint.expect("serialization writes compact progress");
        assert_eq!(
            checkpoint
                .restore(
                    &ignored_instructions::IgnoredInstructionsCheck,
                    &context,
                    &[],
                )
                .unwrap(),
            cursor.progress
        );
        assert!(restored.result.is_none());
        restored.result = Some(serde_json::from_str(&result_json).unwrap());
        assert_eq!(
            restored.result.unwrap().unassessed_comparisons,
            previous.unassessed_comparisons
        );
    }

    #[test]
    fn carried_comparisons_share_repeated_rules_and_actions_across_pages() {
        let reference = result(&["action"]).findings[0].reference.clone();
        let template = plan(&reference).comparisons.remove(0);
        let comparisons = (0..256)
            .map(|index| {
                let mut comparison = template.clone();
                let rule = index % 57;
                let action = index / 57;
                comparison.id =
                    ignored_instructions::sha256_hex(format!("{rule}:{action}").as_bytes());
                comparison.reference.rule_id = format!("rule-{rule}");
                comparison.reference.action_id = format!("action-{action}");
                comparison.rule_text =
                    format!("Rule {rule}: {}", "Do the required step. ".repeat(24));
                comparison.action.action_id = format!("action-{action}");
                comparison.action.text =
                    format!("Action {action}: {}", "An observed action. ".repeat(70));
                comparison.context = (0..3)
                    .map(|offset| CounterEvidence {
                        action_id: format!("nearby-{action}-{offset}"),
                        text: "Nearby event. ".repeat(28),
                        ..comparison.action.clone()
                    })
                    .collect();
                ignored_instructions::extend_comparison_with_history(&comparison, &[], false)
            })
            .collect::<Vec<_>>();
        let cursor = AssessmentCursor {
            carried_comparisons: comparisons.clone(),
            ..AssessmentCursor::default()
        };
        assert!(serde_json::to_string(&cursor).unwrap().len() > 512 * 1024);
        let saved = serialize_cursor(
            &cursor,
            &[],
            &JevSessionContext {
                input_revision: "revision".to_owned(),
                session_identity: "test".to_owned(),
                check_context: serde_json::Value::Null,
                limitations: Vec::new(),
                evidence_store: antiburn_local::analysis::jev::JevEvidenceStore::default(),
                reference_snapshots: Vec::new(),
            },
        )
        .unwrap();
        assert!(
            saved.len() < 512 * 1024,
            "carried page is {} bytes",
            saved.len()
        );
        let (restored, _) = parse_cursor(&saved).unwrap();
        for (before, after) in comparisons.iter().zip(restored.carried_comparisons.iter()) {
            let rebuilt = ignored_instructions::extend_comparison_with_history(after, &[], false);
            assert_eq!(&rebuilt, before);
        }
    }

    #[test]
    fn carried_findings_do_not_inflate_the_current_revision_usage() {
        let mut current = result(&["current"]);
        let request_count = current.request_count;
        let input_tokens = current.input_tokens;
        let previous = result(&["prior"]);

        merge_prior_findings(&mut current, &previous.findings);

        assert_eq!(
            current
                .findings
                .iter()
                .map(|finding| finding.id.as_str())
                .collect::<Vec<_>>(),
            ["current", "prior"]
        );
        assert_eq!(current.request_count, request_count);
        assert_eq!(current.input_tokens, input_tokens);
    }

    #[test]
    fn prior_findings_stay_visible_until_the_current_comparison_is_assessed() {
        let mut prior = result(&["action"]).findings;
        let finding_id = prior[0].id.clone();
        let mut plan = plan(&prior[0].reference);
        let mut page_result = result(&[]);
        page_result.unassessed_comparisons.push("comparison".into());
        let mut validated = std::collections::BTreeSet::new();

        reconcile_prior_findings(&mut prior, &mut validated, &plan, &page_result, false);
        assert_eq!(prior.len(), 1);
        assert!(validated.contains(&finding_id));

        plan.current_action_digests.clear();
        plan.comparisons.clear();
        reconcile_prior_findings(&mut prior, &mut validated, &plan, &result(&[]), false);
        assert_eq!(prior.len(), 1, "validated findings survive later pages");

        plan.current_action_digests
            .insert("action".into(), prior[0].reference.action_digest.clone());
        plan.comparisons.push(CandidateComparison {
            source_binding: None,
            prerequisite_episode: None,
            id: "comparison".into(),
            reference: prior[0].reference.clone(),
            source_thread_digest: "thread".into(),
            source_turn_index: 1,
            source_turn_scope: "main".into(),
            rule_text: "Rule".into(),
            instruction_context: Vec::new(),
            rule_text_start: 0,
            rule_text_end: "Rule".len(),
            action: CounterEvidence {
                action_id: "action".into(),
                source_order: 1,
                role: "assistant".into(),
                kind: "assistant_text".into(),
                timestamp_ms: Some(1),
                tool_name: None,
                text: "Action".into(),
                truncated: false,
            },
            action_text_start: 0,
            action_text_end: "Action".len(),
            context: Vec::new(),
            context_truncated: false,
            counterevidence: Vec::new(),
            earlier_history_truncated: false,
            prior_history_complete: true,
        });
        reconcile_prior_findings(&mut prior, &mut validated, &plan, &result(&[]), false);
        assert!(
            prior.is_empty(),
            "a clean current judgment removes the old finding"
        );
    }

    #[test]
    fn prior_findings_are_dropped_when_the_cited_action_changes() {
        let mut prior = result(&["action"]).findings;
        let mut plan = plan(&prior[0].reference);
        plan.current_action_digests
            .insert("action".into(), "new-action-content".into());

        reconcile_prior_findings(
            &mut prior,
            &mut std::collections::BTreeSet::new(),
            &plan,
            &result(&[]),
            false,
        );

        assert!(prior.is_empty());
    }

    #[test]
    fn older_page_reassessment_clears_stale_unassessed_and_pending_status() {
        let mut previous = result(&[]);
        previous
            .unassessed_comparisons
            .push("comparison".to_owned());
        previous
            .pending_rules
            .push(ignored_instructions::PendingRule {
                instruction_id: "instruction".to_owned(),
                instruction_digest: "digest".to_owned(),
                rule_id: "rule".to_owned(),
                heading: "Workflow".to_owned(),
                reason: "completion_boundary_not_observed".to_owned(),
            });
        let mut current = result(&[]);
        current.coverage.reassessed_comparison_ids = vec!["comparison".to_owned()];
        current.coverage.reassessed_rule_ids = vec!["rule".to_owned()];

        merge_assessment_page(&mut current, previous, false);

        assert!(current.unassessed_comparisons.is_empty());
        assert!(current.pending_rules.is_empty());
    }

    #[test]
    fn later_range_does_not_erase_a_finding_in_an_earlier_range() {
        let previous = result(&["reassessed", "unrelated"]);
        let mut current = result(&[]);
        current.coverage.reassessed_finding_ids = vec!["reassessed".to_owned()];

        merge_assessment_page(&mut current, previous, false);

        assert_eq!(
            current
                .findings
                .iter()
                .map(|finding| finding.id.as_str())
                .collect::<Vec<_>>(),
            ["reassessed", "unrelated"]
        );
    }

    #[test]
    fn content_pages_add_candidate_pairs_but_comparison_pages_do_not() {
        let mut previous = result(&["early"]);
        previous.coverage.candidate_pairs = 4;
        previous.coverage.selected_comparisons = 1;
        previous.coverage.reassessed_comparison_ids = vec!["early-pair".into()];
        let mut next = result(&["later"]);
        next.coverage.candidate_pairs = 3;
        next.coverage.selected_comparisons = 1;
        next.coverage.reassessed_comparison_ids = vec!["later-pair".into()];
        merge_assessment_page(&mut next, previous.clone(), true);
        assert_eq!(next.coverage.candidate_pairs, 7);
        assert_eq!(next.coverage.selected_comparisons, 2);
        assert_eq!(next.coverage.unselected_pairs, 5);

        let mut comparison_page = result(&["later"]);
        comparison_page.coverage.candidate_pairs = 4;
        comparison_page.coverage.reassessed_comparison_ids = vec!["later-pair".into()];
        merge_assessment_page(&mut comparison_page, previous, false);
        assert_eq!(comparison_page.coverage.candidate_pairs, 4);
        assert_eq!(comparison_page.coverage.selected_comparisons, 2);
    }

    #[test]
    fn content_enumeration_continues_after_each_turn_budget() {
        assert!(continue_content_pass(true, 255));
        assert!(continue_content_pass(true, 256));
        assert!(continue_content_pass(true, 1024));
        assert!(!continue_content_pass(false, 0));
    }

    #[test]
    fn comparison_turn_preserves_unselected_inventory_and_active_jobs() {
        let reference = result(&["action"]).findings.remove(0).reference;
        let mut assessment = plan(&reference);
        let template = assessment.comparisons[0].clone();
        assessment.comparisons = (0..256)
            .map(|index| {
                let mut comparison = template.clone();
                comparison.id = format!("pair-{index}");
                comparison
            })
            .collect();
        assessment.coverage.candidate_pairs = 400;
        assessment.coverage.selected_comparisons = 256;
        let mut context = JevSessionContext {
            input_revision: assessment.input_revision.clone(),
            session_identity: "session".into(),
            check_context: serde_json::json!({"assessment_plan": assessment, "episode_actions": []}),
            limitations: Vec::new(),
            evidence_store: Default::default(),
            reference_snapshots: Vec::new(),
        };
        let inventory = context.clone();
        let mut cursor = AssessmentCursor::default();
        let capabilities =
            antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default();
        select_comparison_turn(&mut context, &mut cursor, 1, &capabilities).unwrap();
        let selected: AssessmentPlan =
            serde_json::from_value(context.check_context["assessment_plan"].clone()).unwrap();
        assert_eq!(selected.comparisons.len(), PAIRS_PER_TURN);
        assert_eq!(selected.coverage.selected_comparisons, PAIRS_PER_TURN);
        assert_eq!(selected.coverage.unselected_pairs, 400 - PAIRS_PER_TURN);
        assert!(selected.coverage.sampled_pass);
        assert_eq!(
            cursor
                .sampling
                .as_ref()
                .unwrap()
                .coverage(ignored_check_identity())
                .unwrap()
                .eligible,
            256
        );
        let jobs = serde_json::to_string(&cursor.active_jobs).unwrap();
        let mut restored: AssessmentCursor =
            serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
        let mut resumed = inventory.clone();
        select_comparison_turn(&mut resumed, &mut restored, 1, &capabilities).unwrap();
        assert_eq!(serde_json::to_string(&restored.active_jobs).unwrap(), jobs);
        assert_eq!(resumed.check_context, context.check_context);
        let jobs = restored.active_jobs.clone();
        let sampling = restored.sampling.as_mut().unwrap();
        sampling.terminate_candidate(&jobs[0]).unwrap();
        for job in &jobs[1..] {
            let comparison = selected
                .comparisons
                .iter()
                .find(|comparison| comparison_identity(&comparison.id) == job.candidate)
                .unwrap();
            sampling
                .record_reduced_answer(job, restored.required_answers[&comparison.id])
                .unwrap();
            sampling.complete_candidate(job).unwrap();
        }
        assert_eq!(
            sampling
                .coverage(ignored_check_identity())
                .unwrap()
                .completed,
            PAIRS_PER_TURN - 1
        );
        assert_eq!(
            sampling.runnable_count(ignored_check_identity()),
            256 - PAIRS_PER_TURN
        );
        restored.active_jobs.clear();
        let mut next = inventory;
        select_comparison_turn(&mut next, &mut restored, 1, &capabilities).unwrap();
        assert_eq!(restored.active_jobs.len(), PAIRS_PER_TURN);
        assert!(
            restored
                .active_jobs
                .iter()
                .all(|job| jobs.iter().all(|old| old.candidate != job.candidate))
        );
    }

    #[test]
    fn sampled_pair_dependency_changes_when_earlier_evidence_changes() {
        let reference = result(&["action"]).findings.remove(0).reference;
        let assessment = plan(&reference);
        let original = &assessment.comparisons[0];
        let capabilities =
            antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default();
        let digest = comparison_dependency(&assessment, original, 1, &capabilities).unwrap();
        let mut changed = original.clone();
        changed.context.push(CounterEvidence {
            action_id: "earlier".into(),
            text: "Earlier request".into(),
            ..changed.action.clone()
        });
        assert_ne!(
            comparison_dependency(&assessment, &changed, 1, &capabilities).unwrap(),
            digest
        );
        assert_ne!(
            comparison_dependency(&assessment, original, 2, &capabilities).unwrap(),
            digest
        );
        let revised = antiburn_local::analysis::jev::capabilities::ModelCapabilities {
            model_revision: Some("replacement-digest".into()),
            ..capabilities
        };
        assert_ne!(
            comparison_dependency(&assessment, original, 1, &revised).unwrap(),
            digest
        );
    }

    #[test]
    fn sampling_ledger_only_reuses_matching_previous_round_dependencies() {
        let previous = result(&["action"]);
        let reference = &previous.findings[0].reference;
        let pair = BurnCheckSampledPair {
            comparison_id: "pair".into(),
            dependency_digest: "work".into(),
            incarnation: 1,
            action_id: reference.action_id.clone(),
            action_digest: reference.action_digest.clone(),
            instruction_digest: reference.instruction_digest.clone(),
            selector_revision: 2,
            round: 0,
            assessed: true,
        };
        let mut content = ignored_instructions::SessionContentEvidence {
            session_identity_digest: "session".into(),
            source_format: SourceFormat::ClaudeJsonl,
            publication_fence: 1,
            selected_input_digest: "selection".into(),
            actions: Vec::new(),
            instructions: Vec::new(),
            complete: true,
            limitations: Vec::new(),
            excluded_thinking_parts: 0,
            field_availability: Vec::new(),
        };
        let action = ignored_instructions::ContentAction {
            reference: ignored_instructions::ContentEventReference {
                id: reference.action_id.clone(),
                source_key_digest: "source".into(),
                thread_digest: "thread".into(),
                turn_index: 1,
                native_record_id: None,
                part_index: 0,
                stable: true,
            },
            timestamp_ms: Some(1),
            turn_role: "assistant".into(),
            turn_scope: "main".into(),
            authority: "assistant".into(),
            kind: "assistant_text".into(),
            text: "Action".into(),
            tool_name: None,
            tool_call_id: None,
            normalized_fields: None,
            metadata: Default::default(),
            truncated: false,
            context_only: false,
        };
        content.actions.push(action);
        content.instructions.push(
            ignored_instructions::snapshot_from_text(
                "project:AGENTS.md",
                "Do this.".into(),
                InstructionProvenance::RecordedInjection,
                InstructionScope::Project,
            )
            .unwrap(),
        );
        let pair = BurnCheckSampledPair {
            action_digest: ignored_instructions::content_action_digest(&content.actions[0]),
            instruction_digest: content.instructions[0].digest.clone(),
            ..pair
        };
        assert!(
            sampling_ledger(&content, std::slice::from_ref(&pair), 0, 1, false)
                .comparison_ids
                .is_empty()
        );
        assert!(
            sampling_ledger(&content, std::slice::from_ref(&pair), 1, 1, false)
                .comparison_ids
                .contains("pair")
        );
        let pending = BurnCheckSampledPair {
            assessed: false,
            ..pair.clone()
        };
        assert!(
            sampling_ledger(&content, std::slice::from_ref(&pending), 0, 1, true)
                .comparison_ids
                .contains("pair")
        );
        assert!(
            sampling_ledger(&content, std::slice::from_ref(&pending), 1, 1, false)
                .comparison_ids
                .is_empty()
        );
        assert!(
            sampling_ledger(&content, std::slice::from_ref(&pair), 1, 2, false)
                .comparison_ids
                .is_empty()
        );
        let input = ignored_instructions::AssessmentInput {
            content: content.clone(),
            prior_history_complete: true,
            activity_after_ms: None,
            boundary_positions: BTreeMap::new(),
            source_generation: 1,
            source_fingerprint: None,
            incarnation: 1,
            comparison_after: None,
        };
        let capabilities =
            antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default();
        let context = ignored_instructions::build_jev_context_with_capabilities(
            &input,
            &ignored_instructions::SamplingLedger::default(),
            &capabilities,
        )
        .unwrap();
        let plan = ignored_instructions::IgnoredInstructionsCheck
            .prepare_with_capabilities(&context, &capabilities)
            .unwrap()
            .prepared;
        let comparison = &plan.comparisons[0];
        let saved = BurnCheckSampledPair {
            comparison_id: comparison.id.clone(),
            dependency_digest: comparison_dependency(&plan, comparison, 1, &capabilities).unwrap(),
            ..pair.clone()
        };
        let mut prepared = ignored_instructions::PreparedAssessmentInput::new(&input);
        for (incarnation, digest, should_reuse) in [
            (1, saved.dependency_digest.clone(), true),
            (2, saved.dependency_digest.clone(), false),
            (1, "stale-dependency".to_owned(), false),
        ] {
            let saved = BurnCheckSampledPair {
                dependency_digest: digest,
                ..saved.clone()
            };
            let mut ledger = sampling_ledger(&content, std::slice::from_ref(&saved), 1, 1, false);
            verify_sampling_ledger(
                &mut prepared,
                incarnation,
                &[saved],
                &mut ledger,
                &capabilities,
                None,
            )
            .unwrap();
            assert_eq!(ledger.comparison_ids.contains(&comparison.id), should_reuse);
        }
        let (store, durable) = scheduling_fixture();
        assert!(
            store
                .complete_burn_check_assessment(&durable, "{}", 1_001, IDLE_SECS)
                .unwrap()
        );
        let stale_variant = BurnCheckSampledPair {
            dependency_digest: "different-context".into(),
            action_digest: "changed-action".into(),
            round: 1,
            ..saved.clone()
        };
        assert!(
            store
                .save_burn_check_sampled_pairs(&durable, &[saved.clone(), stale_variant.clone()])
                .unwrap()
        );
        let mut compact_ledger =
            sampling_ledger(&content, std::slice::from_ref(&stale_variant), 2, 1, false);
        let matched = verify_sampling_ledger(
            &mut prepared,
            1,
            &[stale_variant],
            &mut compact_ledger,
            &capabilities,
            Some(SamplingDependencyLookup {
                store: &store,
                input: &durable,
                round: 2,
                backlog: false,
            }),
        )
        .unwrap();
        assert_eq!(matched, vec![saved]);
        assert!(compact_ledger.comparison_ids.contains(&comparison.id));
        assert!(compact_ledger.comparisons.contains_key(&comparison.id));
        content.actions[0].text = "changed".into();
        assert!(
            sampling_ledger(&content, &[pair], 1, 1, false)
                .known_action_ids
                .is_empty()
        );
    }

    #[test]
    fn sampling_round_does_not_move_back_after_variant_cleanup() {
        let mut stored = BurnCheckAssessment {
            key: crate::store::SessionKey::new("native", "claude-code", "sampling-round"),
            check_id: CHECK_ID.into(),
            input_revision: Some("revision".into()),
            status: "failed".into(),
            progress_json: serde_json::json!({"round": 9, "source_fingerprint": "source"})
                .to_string(),
            result_json: None,
            result_revision: None,
            request_count: 0,
        };
        let source = Some("source".to_owned());
        assert_eq!(sampling_round(0, Some(&stored), &source), 9);
        assert_eq!(sampling_round(12, Some(&stored), &source), 12);
        stored.status = "completed".into();
        assert_eq!(sampling_round(0, Some(&stored), &source), 10);
        stored.status = "failed".into();
        assert_eq!(
            sampling_round(0, Some(&stored), &Some("changed".into())),
            10
        );
    }

    #[test]
    fn valid_pending_and_uncertain_pairs_are_terminal_even_with_partial_evidence() {
        let reference = result(&["finding"]).findings.remove(0).reference;
        let plan = plan(&reference);
        let capabilities =
            antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default();
        for outcome in ["pending", "uncertain", "missing"] {
            let mut result = result(&[]);
            result
                .coverage
                .limitations
                .push("source_evidence_is_partial".into());
            if outcome != "missing" {
                result
                    .coverage
                    .reassessed_comparison_ids
                    .push("comparison".into());
            }
            if outcome == "uncertain" {
                result.unassessed_comparisons.push("comparison".into());
            }
            if outcome == "pending" {
                result
                    .pending_rules
                    .push(ignored_instructions::PendingRule {
                        instruction_id: reference.instruction_id.clone(),
                        instruction_digest: reference.instruction_digest.clone(),
                        rule_id: reference.rule_id.clone(),
                        heading: reference.rule_heading.clone(),
                        reason: "completion_not_observed".into(),
                    });
            }
            let pairs = sampled_pairs_for_page(&plan, &result, 0, 1, &capabilities).unwrap();
            assert_eq!(pairs.len(), 1);
            assert_eq!(pairs[0].assessed, outcome != "missing", "{outcome}");
        }
    }

    #[test]
    fn durable_ranges_retain_distinct_findings_and_deduplicate_overlap() {
        let mut later = result(&["overlap", "later"]);
        merge_assessment_page(&mut later, result(&["early", "overlap"]), false);
        assert_eq!(
            later
                .findings
                .iter()
                .map(|finding| finding.id.as_str())
                .collect::<Vec<_>>(),
            ["early", "later", "overlap"]
        );
        assert_eq!(later.request_count, 4);
        let cursor = AssessmentCursor {
            input_revision: Some("revision".to_owned()),
            content_offset: 1,
            comparison_after: Some("cursor".to_owned()),
            progress: JevRunProgress::default(),
            round: 0,
            backlog: false,
            result: Some(later),
            prior_findings: Vec::new(),
            validated_prior_findings: Default::default(),
            carried_comparisons: Vec::new(),
            ..Default::default()
        };
        let restored: AssessmentCursor =
            serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
        assert_eq!(restored.content_offset, 1);
        assert_eq!(restored.comparison_after.as_deref(), Some("cursor"));
        assert_eq!(restored.result.unwrap().findings.len(), 3);
    }

    #[test]
    fn replacing_or_removing_a_key_invalidates_the_old_worker_generation() {
        let handle = WorkerHandle::default();
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        handle
            .set_system_one_connection(
                crate::jev::config::SystemOneConnection::jev_default(),
                Some("synthetic-first".into()),
            )
            .unwrap();
        let first = handle.execution_client().unwrap().1;
        handle
            .set_system_one_connection(
                crate::jev::config::SystemOneConnection::jev_default(),
                Some("synthetic-second".into()),
            )
            .unwrap();
        assert!(!handle.key_is_current(first));
        let second = handle.execution_client().unwrap().1;
        handle.reject_authentication(&store, second).unwrap();
        assert!(!handle.key_is_current(second));
        assert!(handle.execution_client().is_none());
        handle.suspend_system_one();
        assert!(!handle.is_available());
        assert!(!handle.authentication_rejected());
    }
}
