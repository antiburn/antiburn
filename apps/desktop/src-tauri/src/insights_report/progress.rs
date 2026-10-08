use super::*;
use crate::dto::ChecksReviewCoveragePayload;
use crate::jev::worker::JevCheckDescriptor;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CheckReportProgress {
    pub checking: bool,
    pub checking_count: u64,
    pub partial_context: bool,
    pub coverage: Option<ChecksReviewCoveragePayload>,
}

struct JoinedReviewCounts {
    eligible: Option<usize>,
    reviewed: usize,
    runnable: usize,
}

#[cfg(test)]
fn all_check_report_progress_with_home(
    data_dir: &Path,
    request: &ReportRequest,
    home: Option<&Path>,
) -> Result<BTreeMap<String, CheckReportProgress>> {
    let connection = open_read_only(data_dir, REPORT_BUSY_TIMEOUT)?;
    let transaction = connection.unchecked_transaction()?;
    check_report_progress_in(&transaction, request, home)
}

pub(super) fn check_report_progress_in(
    connection: &rusqlite::Connection,
    request: &ReportRequest,
    home: Option<&Path>,
) -> Result<BTreeMap<String, CheckReportProgress>> {
    let current_evidence = crate::store::revision_sql::current_evidence("e", "s");
    let sql = format!(
        "WITH checks(check_id, evaluator_revision) AS (VALUES
            ('ignored_instructions', :instructions_revision),
            ('scope_creep', :scope_revision),
            ('over_exploring', :exploring_revision),
            ('skill_opportunities', :skills_revision))
         SELECT a.status, a.last_error_category, a.input_revision, a.result_revision,
                a.result_json, c.coverage_json,
                 CASE WHEN checks.check_id != 'ignored_instructions' THEN EXISTS (SELECT 1 FROM turn t LEFT JOIN turn_content p ON p.turn_rowid = t.rowid
                  WHERE t.environment_key = s.environment_key AND t.agent = s.agent
                    AND t.session_id = s.session_id AND t.claim_fence = e.published_fence
                    AND t.scope = 'main' AND (t.is_compaction_boundary != 0
                      OR (p.kind != 'thinking' AND p.truncated != 0)
                       OR (t.role = 'user' AND p.turn_rowid IS NULL))) ELSE 0 END,
                 CASE WHEN checks.check_id != 'ignored_instructions' THEN (SELECT COUNT(*) FROM turn t WHERE t.environment_key = s.environment_key
                  AND t.agent = s.agent AND t.session_id = s.session_id
                   AND t.claim_fence = e.published_fence AND t.scope = 'main') ELSE 0 END,
                 CASE WHEN checks.check_id != 'ignored_instructions' THEN (SELECT role FROM turn t WHERE t.environment_key = s.environment_key
                  AND t.agent = s.agent AND t.session_id = s.session_id
                  AND t.claim_fence = e.published_fence AND t.scope = 'main'
                    ORDER BY turn_index LIMIT 1) END,
                  s.agent, s.cwd, e.evidence_json, s.session_id,
                  e.status = 'ready' AND e.processed_fingerprint IS s.source_fingerprint
                    AND {current_evidence},
                   CASE WHEN checks.check_id = 'ignored_instructions' AND json_valid(a.progress_json)
                    THEN json_object('input_revision', json_extract(a.progress_json, '$.progress.input_revision'),
                      'version', json_extract(a.progress_json, '$.progress.version'),
                       'answers', json_extract(a.progress_json, '$.progress.answers')) END,
                   checks.check_id,
                   a.input_revision IS NOT NULL AND a.scheduling_revision = a.input_revision
                     AND a.status IN ('queued', 'running', 'completed', 'failed'),
                   a.eligible_targets, a.reviewed_targets, a.runnable_targets
           FROM session s CROSS JOIN checks
         LEFT JOIN session_evidence e ON e.environment_key = s.environment_key AND e.agent = s.agent
           AND e.session_id = s.session_id
         LEFT JOIN session_coverage c ON c.environment_key = s.environment_key AND c.agent = s.agent
            AND c.session_id = s.session_id AND c.claim_fence = e.published_fence
          LEFT JOIN burn_check_assessment a ON a.environment_key = s.environment_key AND a.agent = s.agent
             AND a.session_id = s.session_id AND a.check_id = checks.check_id
            AND a.incarnation = s.incarnation AND a.source_generation = s.source_generation
            AND a.source_fingerprint IS s.source_fingerprint AND a.published_fence = e.published_fence
             AND a.evaluator_revision = checks.evaluator_revision
          WHERE s.environment_key = :environment_key
           AND COALESCE(s.updated_at_epoch, s.started_at_epoch) >= :window_start
            AND COALESCE(s.updated_at_epoch, s.started_at_epoch) < :window_end"
    );
    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query(named_params![
        ":environment_key": request.environment_key,
        ":window_start": request.window.start_epoch,
        ":window_end": request.window.end_epoch,
        ":instructions_revision": antiburn_local::analysis::ignored_instructions::evaluator_revision(),
        ":scope_revision": crate::scope_creep_worker::CHECK.evaluator_revision(),
        ":exploring_revision": crate::over_exploring_worker::CHECK.evaluator_revision(),
        ":skills_revision": crate::skill_opportunities_worker::CHECK.evaluator_revision(),
        ":parser_revision": PARSER_REVISION,
        ":analyzer_revision": ANALYZER_REVISION,
        ":evidence_schema_revision": EVIDENCE_SCHEMA_REVISION,
    ])?;
    let mut progress_by_check = BTreeMap::new();
    let mut missing_denominators = BTreeSet::new();
    let mut inventory_revisions = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let check_id: String = row.get(15)?;
        let check_id = check_id.as_str();
        let progress =
            progress_by_check
                .entry(check_id.to_owned())
                .or_insert(CheckReportProgress {
                    checking: false,
                    checking_count: 0,
                    partial_context: false,
                    coverage: None,
                });
        let agent: String = row.get(9)?;
        let current: Option<bool> = row.get(13)?;
        if current != Some(true) {
            let missing = matches!(agent.as_str(), "claude-code" | "codex" | "opencode" | "pi")
                && (check_id == "ignored_instructions" || request.environment_key == "native");
            if missing {
                missing_denominators.insert(check_id.to_owned());
            }
            continue;
        }
        let cwd: Option<String> = row.get(10)?;
        let evidence: SessionEvidence = serde_json::from_str(&row.get::<_, String>(11)?)
            .context("stored session evidence is invalid")?;
        let session_id: String = row.get(12)?;
        if evidence.identity.agent != agent
            || evidence.identity.session_id != session_id
            || if check_id == "ignored_instructions" {
                !antiburn_local::analysis::ignored_instructions::source_supported(
                    evidence.capabilities.source_format,
                )
            } else {
                request.environment_key != "native"
                    || !antiburn_local::analysis::smart_check_source_supported(
                        &agent,
                        evidence.capabilities.source_format,
                    )
            }
        {
            continue;
        }
        let status: Option<String> = row.get(0)?;
        let error: Option<String> = row.get(1)?;
        let checking = matches!(status.as_deref(), Some("queued" | "running"))
            || (status.as_deref() == Some("failed")
                && matches!(error.as_deref(), Some("sampling_incomplete" | "continuing"))
                && row.get::<_, Option<bool>>(16)? == Some(true)
                && row.get::<_, usize>(19)? > 0);
        progress.checking |= checking;
        progress.checking_count += u64::from(checking);
        let source_coverage: Option<String> = row.get(5)?;
        let retained_loss: bool = row.get(6)?;
        let retained_records: u64 = row.get(7)?;
        let first_role: Option<String> = row.get(8)?;
        let partial_source = check_id != "ignored_instructions"
            && (retained_loss
                || match source_coverage.as_deref() {
                    Some(json) => source_is_partial(json, retained_records, first_role.as_deref())?,
                    None => true,
                });
        progress.partial_context |= partial_source;
        let input: Option<String> = row.get(2)?;
        let revision: Option<String> = row.get(3)?;
        let json: Option<String> = row.get(4)?;
        if check_id == "ignored_instructions"
            && input.is_some()
            && input == revision
            && let Some(json) = &json
            && let Ok(saved) = serde_json::from_str::<IgnoredInstructionCoverageNotice>(json)
            && Some(saved.input_revision.as_str()) == input.as_deref()
        {
            progress.partial_context |= ignored_instruction_partial_context(&saved.coverage);
        }
        let publication_eligible = matches!(status.as_deref(), Some("completed" | "failed"))
            && input.is_some()
            && input == revision
            && (status.as_deref() != Some("failed")
                || check_id == "over_exploring"
                || matches!(error.as_deref(), Some("sampling_incomplete" | "continuing")));
        let publication = if publication_eligible {
            input
                .as_deref()
                .zip(json.as_deref())
                .and_then(|(input, json)| {
                    let inventory_revision = if check_id == "skill_opportunities" {
                        let saved: serde_json::Value = serde_json::from_str(json).ok()?;
                        let saved_revision = saved.get("inventory_revision")?.as_str()?;
                        published_coverage(check_id, input, json, Some(saved_revision))?;
                        inventory_revisions
                            .entry((agent.clone(), cwd.clone()))
                            .or_insert_with(|| {
                                home.and_then(|home| {
                                    skill_opportunities::current_skill_snapshot(
                                        &agent,
                                        cwd.clone().map(PathBuf::from),
                                        home,
                                    )
                                })
                                .map(|snapshot| snapshot.revision())
                            })
                            .clone()
                    } else {
                        None
                    };
                    published_coverage(check_id, input, json, inventory_revision.as_deref())
                })
        } else {
            None
        };
        if let Some((_, partial)) = &publication {
            progress.partial_context |= partial;
        }
        let counts = if row.get::<_, Option<bool>>(16)? == Some(true) {
            let counts = JoinedReviewCounts {
                eligible: row.get(17)?,
                reviewed: row.get(18)?,
                runnable: row.get(19)?,
            };
            if counts.eligible.is_some_and(|total| {
                counts.reviewed > total || counts.runnable > total.saturating_sub(counts.reviewed)
            }) {
                anyhow::bail!("Burn Check scheduling counts are invalid");
            }
            Some(counts)
        } else {
            None
        };
        let mut coverage = match counts {
            Some(counts) => {
                let outcomes = publication
                    .as_ref()
                    .map(|(coverage, _)| coverage)
                    .filter(|coverage| coverage.reviewed == counts.reviewed as u64);
                ChecksReviewCoveragePayload {
                    reviewed: counts.reviewed as u64,
                    total: counts.eligible.map(|total| total as u64),
                    uncertain: if counts.reviewed == 0 {
                        Some(0)
                    } else {
                        outcomes.map(|coverage| coverage.uncertain)
                    },
                    pending: counts
                        .eligible
                        .map(|total| (total - counts.reviewed) as u64),
                    pending_completion: if counts.reviewed == 0 {
                        Some(0)
                    } else {
                        outcomes.and_then(|coverage| coverage.pending_completion)
                    },
                    continuing: checking && counts.runnable > 0,
                }
            }
            None => match publication {
                Some((coverage, _)) => coverage.into(),
                None => {
                    missing_denominators.insert(check_id.to_owned());
                    continue;
                }
            },
        };
        if check_id == "ignored_instructions" {
            let compact: Option<String> = row.get(14)?;
            if let Some((uncertain, pending_completion)) = compact
                .as_deref()
                .zip(input.as_deref())
                .and_then(|(json, input)| {
                    ignored_instruction_outcomes(json, input, coverage.reviewed)
                })
            {
                coverage.uncertain = Some(uncertain);
                coverage.pending_completion = Some(pending_completion);
            }
        }
        if let Some(total) = &mut progress.coverage {
            total.reviewed += coverage.reviewed;
            total.total = total
                .total
                .zip(coverage.total)
                .map(|(left, right)| left + right);
            total.uncertain = total
                .uncertain
                .zip(coverage.uncertain)
                .map(|(left, right)| left + right);
            total.pending = total
                .pending
                .zip(coverage.pending)
                .map(|(left, right)| left + right);
            total.pending_completion = total
                .pending_completion
                .zip(coverage.pending_completion)
                .map(|(left, right)| left + right);
            total.continuing |= coverage.continuing;
        } else {
            progress.coverage = Some(coverage);
        }
    }
    for check_id in missing_denominators {
        if let Some(coverage) = progress_by_check
            .get_mut(&check_id)
            .and_then(|progress| progress.coverage.as_mut())
        {
            coverage.total = None;
            coverage.pending = None;
        }
    }
    Ok(progress_by_check)
}

#[cfg(test)]
fn check_report_progress_with_home(
    data_dir: &Path,
    request: &ReportRequest,
    check_id: &str,
    home: Option<&Path>,
) -> Result<CheckReportProgress> {
    Ok(
        all_check_report_progress_with_home(data_dir, request, home)?
            .remove(check_id)
            .unwrap_or(CheckReportProgress {
                checking: false,
                checking_count: 0,
                partial_context: false,
                coverage: None,
            }),
    )
}

#[derive(Deserialize)]
struct ScopePublication {
    input_revision: String,
    assessment: antiburn_local::checks::scope_creep::ScopeCreepResult,
}

#[derive(Deserialize)]
struct SkillPublication {
    #[serde(flatten)]
    result: antiburn_local::checks::skill_opportunities::SkillOpportunitiesResult,
}

#[derive(Deserialize)]
struct IgnoredInstructionCoverageNotice {
    input_revision: String,
    coverage: antiburn_local::analysis::ignored_instructions::AssessmentCoverage,
}

fn ignored_instruction_partial_context(
    coverage: &antiburn_local::analysis::ignored_instructions::AssessmentCoverage,
) -> bool {
    !coverage.skipped_rules.is_empty()
        || !coverage.skipped_actions.is_empty()
        || coverage.limitations.iter().any(|limit| {
            !matches!(
                limit.as_str(),
                "sampled_candidate_selection"
                    | "sampled_content_selection"
                    | "semantic_decision_uncertain"
                    | "some_comparisons_unassessed"
                    | "assessment_processing_incomplete"
            )
        })
}

#[derive(Deserialize)]
struct IgnoredInstructionCompactOutcomes {
    version: u8,
    input_revision: String,
    answers: Vec<Option<Vec<antiburn_local::analysis::jev::JevAnswer>>>,
}

fn ignored_instruction_outcomes(
    json: &str,
    input_revision: &str,
    reviewed: u64,
) -> Option<(u64, u64)> {
    use antiburn_local::analysis::jev::{JevAnswer, highest_probability_choice};
    let saved: IgnoredInstructionCompactOutcomes = serde_json::from_str(json).ok()?;
    if saved.version != 3 || saved.input_revision != input_revision {
        return None;
    }
    let mut terminal = 0;
    let mut uncertain = 0;
    let mut pending_completion = 0;
    for answers in saved.answers.into_iter().flatten() {
        let [
            JevAnswer::Choice {
                choice,
                probabilities,
                ..
            },
        ] = answers.as_slice()
        else {
            return None;
        };
        if highest_probability_choice(choice, probabilities) != Some(choice.as_str()) {
            return None;
        }
        match choice.as_str() {
            "uncertain" => uncertain += 1,
            "pending_completion" => pending_completion += 1,
            "no_issue" => {}
            // Low-probability conflicts do not have a definitive publication outcome.
            "conflict"
                if probabilities
                    .get(choice)
                    .is_some_and(|value| *value >= 0.75) => {}
            _ => return None,
        }
        terminal += 1;
    }
    (terminal == reviewed).then_some((uncertain, pending_completion))
}

fn source_is_partial(json: &str, retained_records: u64, first_role: Option<&str>) -> Result<bool> {
    use antiburn_local::analysis::{SessionCoverageRecord, SourceAcceptance, SourceFormat};
    let source = serde_json::from_str::<SessionCoverageRecord>(json)
        .context("stored session coverage is invalid")?;
    Ok(source.source_acceptance != SourceAcceptance::AcceptedFull
        || retained_records != source.diagnostics.records_observed
        || (matches!(
            source.capabilities.source_format,
            SourceFormat::ClaudeJsonl | SourceFormat::OpenCodeSqliteV2
        ) && first_role != Some("user"))
        || !source.summary_observed
        || source.record_loss_reason.is_some()
        || source.child_loss_reason.is_some()
        || source.session_cap_exceeded
        || source.thread_parent_unresolved
        || source.subagent_linkage_incomplete
        || source.subagents_cap_exceeded
        || source.diagnostics.records_unusable != 0
        || !source.diagnostics.truncated_strings.is_empty()
        || !source.diagnostics.capped_collections.is_empty())
}

pub(super) struct PublishedReviewCoverage {
    pub reviewed: u64,
    pub total: Option<u64>,
    pub uncertain: u64,
    pub pending: u64,
    pub pending_completion: Option<u64>,
    pub continuing: bool,
}

impl From<PublishedReviewCoverage> for ChecksReviewCoveragePayload {
    fn from(coverage: PublishedReviewCoverage) -> Self {
        Self {
            reviewed: coverage.reviewed,
            total: coverage.total,
            uncertain: Some(coverage.uncertain),
            pending: Some(coverage.pending),
            pending_completion: coverage.pending_completion,
            continuing: coverage.continuing,
        }
    }
}

pub(super) fn published_coverage(
    check_id: &str,
    input_revision: &str,
    json: &str,
    inventory_revision: Option<&str>,
) -> Option<(PublishedReviewCoverage, bool)> {
    if check_id == "ignored_instructions" {
        let result: antiburn_local::analysis::ignored_instructions::AssessmentResult =
            serde_json::from_str(json).ok()?;
        if result.input_revision != input_revision {
            return None;
        }
        // The publication does not separate missing answers from uncertain answers.
        if !result.unassessed_comparisons.is_empty() {
            return None;
        }
        let partial = ignored_instruction_partial_context(&result.coverage);
        return Some((
            PublishedReviewCoverage {
                reviewed: result.coverage.selected_comparisons as u64,
                total: (!result.coverage.processing_limit_reached
                    && result.coverage.skipped_rules.is_empty()
                    && result.coverage.skipped_actions.is_empty())
                .then_some(result.coverage.candidate_pairs as u64),
                uncertain: 0,
                pending: result.coverage.unselected_pairs as u64,
                pending_completion: result.pending_rules.is_empty().then_some(0),
                continuing: false,
            },
            partial,
        ));
    }
    let (coverage, reviewed, uncertain, pending, scope_partial) = if check_id
        == "skill_opportunities"
    {
        use antiburn_local::checks::skill_opportunities::SkillOpportunityOutcome;
        let saved: SkillPublication = serde_json::from_str(json).ok()?;
        if !skill_opportunities::publication_revisions_match(
            json,
            input_revision,
            inventory_revision?,
            &saved.result,
        ) {
            return None;
        }
        let reviewed = saved
            .result
            .decisions
            .iter()
            .filter(|decision| {
                matches!(
                    decision.outcome,
                    SkillOpportunityOutcome::Advisory
                        | SkillOpportunityOutcome::NoOpportunity
                        | SkillOpportunityOutcome::Uncertain
                )
            })
            .count();
        let pending = (saved.result.coverage.selected_items
            + saved.result.coverage.not_selected_items
            + saved.result.coverage.skipped_items)
            .saturating_sub(reviewed);
        let uncertain = saved
            .result
            .decisions
            .iter()
            .filter(|decision| decision.outcome == SkillOpportunityOutcome::Uncertain)
            .count();
        (saved.result.coverage, reviewed, uncertain, pending, false)
    } else if check_id == "scope_creep" {
        use antiburn_local::checks::scope_creep::ScopeCreepStatus;
        let saved: ScopePublication = serde_json::from_str(json).ok()?;
        if saved.input_revision != input_revision {
            return None;
        }
        let uncertain = saved
            .assessment
            .decisions
            .iter()
            .filter(|decision| matches!(decision.status, ScopeCreepStatus::Uncertain))
            .count();
        let reviewed = saved
            .assessment
            .decisions
            .iter()
            .filter(|decision| {
                matches!(
                    decision.status,
                    ScopeCreepStatus::Finding
                        | ScopeCreepStatus::Clean
                        | ScopeCreepStatus::Uncertain
                )
            })
            .count();
        let pending = (saved.assessment.coverage.selected_items
            + saved.assessment.coverage.not_selected_items
            + saved.assessment.coverage.skipped_items)
            .saturating_sub(reviewed);
        (
            saved.assessment.coverage,
            reviewed,
            uncertain,
            pending,
            saved.assessment.session_limitation.is_some(),
        )
    } else {
        use antiburn_local::checks::over_exploring::Abstention;
        let saved: crate::over_exploring_worker::Publication = serde_json::from_str(json).ok()?;
        if saved.input_revision != input_revision {
            return None;
        }
        let uncertain = saved
            .assessment
            .unassessed
            .iter()
            .filter(|item| item.limitation == Abstention::UncertainDecision)
            .filter_map(|item| item.work_item_id.as_deref())
            .collect::<BTreeSet<_>>()
            .len();
        let coverage = saved.assessment.coverage;
        let partial = coverage.limitations.iter().any(|limit| {
            !matches!(
                limit.as_str(),
                "SampledEvidence" | "PartialAssessment" | "UncertainDecision"
            )
        });
        let reviewed = saved
            .assessment
            .completed_work_item_ids
            .iter()
            .collect::<BTreeSet<_>>()
            .len();
        return Some((
            PublishedReviewCoverage {
                reviewed: reviewed as u64,
                total: (!coverage
                    .limitations
                    .iter()
                    .any(|limit| limit == "sampling_inventory_limit"))
                .then_some((coverage.selected_items + coverage.not_selected_items) as u64),
                uncertain: uncertain as u64,
                pending: (coverage.selected_items + coverage.not_selected_items)
                    .saturating_sub(reviewed) as u64,
                pending_completion: Some(0),
                continuing: false,
            },
            partial,
        ));
    };
    let partial = scope_partial
        || coverage.limitations.iter().any(|limit| {
            !matches!(
                limit.as_str(),
                "uncertain_decision" | "assessment_incomplete"
            )
        });
    Some((
        PublishedReviewCoverage {
            reviewed: reviewed as u64,
            total: (!coverage
                .limitations
                .iter()
                .any(|limit| limit == "sampling_inventory_limit"))
            .then_some(
                (coverage.selected_items + coverage.skipped_items + coverage.not_selected_items)
                    as u64,
            ),
            uncertain: uncertain as u64,
            pending: pending as u64,
            pending_completion: Some(0),
            continuing: false,
        },
        partial,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::over_exploring_worker::{prepare, publication, tests::fixture};
    use crate::smart_check_inputs::DetectorInput;
    use antiburn_local::analysis::jev::{JevCheck, capabilities::ModelCapabilities};
    use antiburn_local::checks::over_exploring::OverExploringCheck;

    fn check_report_progress(
        data_dir: &Path,
        request: &ReportRequest,
        check_id: &str,
    ) -> Result<CheckReportProgress> {
        let home = antiburn_local::paths::home_dir();
        check_report_progress_with_home(data_dir, request, check_id, home.as_deref())
    }

    #[test]
    fn report_and_all_check_progress_keep_the_snapshot_during_publication() {
        use crate::over_exploring_worker::tests::reduced;
        use antiburn_local::checks::over_exploring::Reason;

        let directory = tempfile::tempdir().unwrap();
        let (store, candidate) = fixture(Some(directory.path()), "user");
        let input = prepare(
            &candidate,
            store
                .load_smart_check_inputs(
                    &candidate.session.key,
                    candidate.published_fence,
                    candidate.source_generation,
                    DetectorInput::OverExploring,
                )
                .unwrap(),
            &ModelCapabilities::jev_default(),
        )
        .unwrap();
        let json = serde_json::to_string(&publication(
            &input,
            reduced(&input, Reason::UnrelatedFiles),
        ))
        .unwrap();
        for check in crate::jev::worker::registered_checks() {
            store
                .set_check_enabled(DetectorId::from_key(check.id()).unwrap(), true)
                .unwrap();
            store
                .capture_burn_check_boundaries(&[check.id()], 0)
                .unwrap();
            let mut durable = input.durable.clone();
            durable.check_id = check.id().into();
            durable.evaluator_revision = check.evaluator_revision();
            assert!(
                store
                    .queue_burn_check_assessment(&durable, 1000, 180)
                    .unwrap()
            );
            assert!(
                store
                    .save_burn_check_scheduling(&durable, Some(4), 0, 4)
                    .unwrap()
            );
        }
        let request = ReportRequest {
            environment_key: "native".into(),
            window: ReportWindow {
                start_epoch: 0,
                end_epoch: i64::MAX,
            },
            computed_at_epoch: 1001,
        };
        let snapshot = crate::insights_report::reduce_with_state_on_snapshot(
            directory.path(),
            request.clone(),
            &mut || {
                assert!(
                    store
                        .claim_burn_check_assessment(&input.durable, 1000, 300, 180)
                        .unwrap()
                );
                assert!(
                    store
                        .complete_burn_check_assessment(&input.durable, &json, 1001, 180)
                        .unwrap()
                );
                store.lock().execute(
                    "UPDATE burn_check_assessment SET status = 'completed', reviewed_targets = 2,
                     runnable_targets = 0",
                    [],
                ).unwrap();
            },
            &AtomicBool::new(false),
            &mut || {},
            None,
        )
        .unwrap();
        assert_eq!(snapshot.check_progress.len(), 4);
        for progress in snapshot.check_progress.values() {
            assert_eq!(progress.checking_count, 1);
            let coverage = progress.coverage.as_ref().unwrap();
            assert_eq!(coverage.reviewed, 0);
            assert_eq!(coverage.total, Some(4));
            assert!(coverage.continuing);
        }
        let current = reduce_report_blocking(directory.path(), request).unwrap();
        for progress in current.check_progress.values() {
            assert_eq!(progress.checking_count, 0);
            let coverage = progress.coverage.as_ref().unwrap();
            assert_eq!(coverage.reviewed, 2);
            assert!(!coverage.continuing);
        }
        let finding_sessions = |report: &ReducedReport| {
            report.report.detectors[DetectorId::OverExploring.index()].finding
        };
        assert_eq!(finding_sessions(&snapshot), 0);
        assert_eq!(finding_sessions(&current), 1);
    }

    #[test]
    fn current_scheduling_counts_keep_known_targets_separate_from_partial_context() {
        for check_id in [
            "ignored_instructions",
            "scope_creep",
            "over_exploring",
            "skill_opportunities",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let (store, candidate) = fixture(Some(directory.path()), "user");
            let input = prepare(
                &candidate,
                store
                    .load_smart_check_inputs(
                        &candidate.session.key,
                        candidate.published_fence,
                        candidate.source_generation,
                        DetectorInput::OverExploring,
                    )
                    .unwrap(),
                &ModelCapabilities::jev_default(),
            )
            .unwrap();
            let mut durable = input.durable;
            durable.check_id = check_id.into();
            durable.evaluator_revision = match check_id {
                "scope_creep" => crate::scope_creep_worker::CHECK.evaluator_revision(),
                "over_exploring" => crate::over_exploring_worker::CHECK.evaluator_revision(),
                "skill_opportunities" => {
                    crate::skill_opportunities_worker::CHECK.evaluator_revision()
                }
                _ => antiburn_local::analysis::ignored_instructions::evaluator_revision(),
            };
            store
                .set_check_enabled(DetectorId::from_key(check_id).unwrap(), true)
                .unwrap();
            store.capture_burn_check_boundaries(&[check_id], 0).unwrap();
            assert!(
                store
                    .queue_burn_check_assessment(&durable, 1000, 180)
                    .unwrap()
            );
            assert!(
                store
                    .save_burn_check_scheduling(&durable, Some(4), 2, 2)
                    .unwrap()
            );
            store
                .lock()
                .execute(
                    "UPDATE turn SET is_compaction_boundary = 1 WHERE turn_index = 0",
                    [],
                )
                .unwrap();
            let request = ReportRequest {
                environment_key: "native".into(),
                window: ReportWindow {
                    start_epoch: 0,
                    end_epoch: i64::MAX,
                },
                computed_at_epoch: 1001,
            };
            let read = || check_report_progress(directory.path(), &request, check_id).unwrap();
            let progress = read();
            assert!(progress.checking, "{check_id}");
            if check_id != "ignored_instructions" {
                assert!(progress.partial_context, "{check_id}");
            }
            let coverage = progress.coverage.unwrap();
            assert_eq!(coverage.reviewed, 2, "{check_id}");
            assert_eq!(coverage.total, Some(4), "{check_id}");
            assert_eq!(coverage.pending, Some(2), "{check_id}");
            assert_eq!(coverage.uncertain, None, "{check_id}");
            assert!(coverage.continuing, "{check_id}");
            if check_id == "ignored_instructions" {
                let answers = serde_json::json!({"progress": {
                    "version": 3, "input_revision": durable.input_revision,
                    "answers": [
                        [{"type": "choice", "choice": "uncertain", "probabilities": {"uncertain": 1.0}, "confidence": 1.0}],
                        [{"type": "choice", "choice": "pending_completion", "probabilities": {"pending_completion": 1.0}, "confidence": 1.0}],
                    ],
                }});
                store
                    .lock()
                    .execute(
                        "UPDATE burn_check_assessment SET progress_json = ?1 WHERE check_id = ?2",
                        params![answers.to_string(), check_id],
                    )
                    .unwrap();
                let coverage = read().coverage.unwrap();
                assert_eq!(coverage.uncertain, Some(1));
                assert_eq!(coverage.pending_completion, Some(1));
                assert_eq!(coverage.pending, Some(2));
            }
            store
                .lock()
                .execute(
                    "UPDATE burn_check_assessment SET status = 'completed' WHERE check_id = ?1",
                    [check_id],
                )
                .unwrap();
            let terminal = read();
            assert!(!terminal.checking);
            assert!(!terminal.coverage.unwrap().continuing);
            assert!(
                store
                    .save_burn_check_scheduling(&durable, None, 2, 0)
                    .unwrap()
            );
            let unknown = read().coverage.unwrap();
            assert_eq!(unknown.total, None);
            assert_eq!(unknown.pending, None);
        }
    }

    #[test]
    fn compact_outcomes_do_not_claim_counts_for_other_pages_or_revisions() {
        let saved = serde_json::json!({
            "version": 3, "input_revision": "input",
            "answers": [[{"type": "choice", "choice": "uncertain", "probabilities": {"uncertain": 1.0}, "confidence": 1.0}]],
        }).to_string();
        assert_eq!(
            ignored_instruction_outcomes(&saved, "input", 1),
            Some((1, 0))
        );
        assert_eq!(ignored_instruction_outcomes(&saved, "input", 2), None);
        assert_eq!(ignored_instruction_outcomes(&saved, "old", 1), None);
    }

    #[test]
    fn snapshot_counts_sessions_for_all_checks_and_excludes_blocked_continuations() {
        use crate::scope_creep_worker::tests::native_sources;

        let directory = tempfile::tempdir().unwrap();
        let (store, first) = fixture(Some(directory.path()), "user");
        store
            .set_check_enabled(DetectorId::ScopeCreep, true)
            .unwrap();
        store
            .capture_burn_check_boundaries(&["scope_creep"], 0)
            .unwrap();
        let (agent, session, format, records) = native_sources::read_sources().remove(0);
        let second =
            native_sources::publish(&store, &agent, &session, format, &records, directory.path());
        let checks = [
            (
                "ignored_instructions",
                antiburn_local::analysis::ignored_instructions::evaluator_revision(),
            ),
            (
                "scope_creep",
                crate::scope_creep_worker::CHECK.evaluator_revision(),
            ),
            (
                "over_exploring",
                crate::over_exploring_worker::CHECK.evaluator_revision(),
            ),
            (
                "skill_opportunities",
                crate::skill_opportunities_worker::CHECK.evaluator_revision(),
            ),
        ];
        for (check_id, _) in &checks {
            store
                .set_check_enabled(DetectorId::from_key(check_id).unwrap(), true)
                .unwrap();
            store.capture_burn_check_boundaries(&[check_id], 0).unwrap();
        }
        for candidate in [first, second] {
            let input = prepare(
                &candidate,
                store
                    .load_smart_check_inputs(
                        &candidate.session.key,
                        candidate.published_fence,
                        candidate.source_generation,
                        DetectorInput::OverExploring,
                    )
                    .unwrap(),
                &ModelCapabilities::jev_default(),
            )
            .unwrap();
            for (check_id, evaluator_revision) in &checks {
                let mut durable = input.durable.clone();
                durable.check_id = (*check_id).into();
                durable.evaluator_revision = evaluator_revision.clone();
                assert!(
                    store
                        .queue_burn_check_assessment(&durable, 1000, 180)
                        .unwrap()
                );
                assert!(
                    store
                        .save_burn_check_scheduling(&durable, Some(4), 1, 3)
                        .unwrap()
                );
            }
        }
        let request = ReportRequest {
            environment_key: "native".into(),
            window: ReportWindow {
                start_epoch: 0,
                end_epoch: i64::MAX,
            },
            computed_at_epoch: 1001,
        };
        let read =
            || all_check_report_progress_with_home(directory.path(), &request, None).unwrap();
        for (status, error, runnable, expected) in [
            ("queued", None, 3, 2),
            ("running", None, 3, 2),
            ("failed", Some("continuing"), 3, 2),
            ("failed", Some("sampling_incomplete"), 3, 2),
            ("failed", Some("continuing"), 0, 0),
            ("failed", Some("provider_unavailable"), 3, 0),
            ("failed", Some("delivery_unknown"), 3, 0),
            ("completed", None, 3, 0),
        ] {
            store
                .lock()
                .execute(
                    "UPDATE burn_check_assessment SET status = ?1, last_error_category = ?2,
                 runnable_targets = ?3, result_revision = NULL",
                    params![status, error, runnable],
                )
                .unwrap();
            let snapshot = read();
            assert_eq!(snapshot.len(), 4);
            for (check_id, _) in &checks {
                let progress = &snapshot[*check_id];
                assert_eq!(
                    progress.checking_count, expected,
                    "{check_id}: {status} {error:?}"
                );
                assert_eq!(progress.checking, expected > 0);
                let coverage = progress.coverage.as_ref().unwrap();
                assert_eq!(coverage.reviewed, 2);
                assert_eq!(coverage.total, Some(8));
                assert_eq!(coverage.continuing, expected > 0);
            }
        }
        store.lock().execute(
            "UPDATE burn_check_assessment SET status = 'failed', last_error_category = 'continuing',
             runnable_targets = 3, scheduling_revision = 'old'", [],
        ).unwrap();
        assert!(
            read()
                .values()
                .all(|progress| progress.checking_count == 0 && progress.coverage.is_none())
        );
    }

    #[test]
    fn incomplete_session_denominators_keep_the_aggregate_total_unknown() {
        use crate::over_exploring_worker::tests::reduced;
        use crate::scope_creep_worker::tests::native_sources;
        use antiburn_local::checks::over_exploring::Reason;

        let directory = tempfile::tempdir().unwrap();
        let (store, candidate) = fixture(Some(directory.path()), "user");
        let input = prepare(
            &candidate,
            store
                .load_smart_check_inputs(
                    &candidate.session.key,
                    candidate.published_fence,
                    candidate.source_generation,
                    DetectorInput::OverExploring,
                )
                .unwrap(),
            &ModelCapabilities::jev_default(),
        )
        .unwrap();
        let result = reduced(&input, Reason::UnrelatedFiles);
        let json = serde_json::to_string(&publication(&input, result)).unwrap();
        assert!(
            store
                .queue_burn_check_assessment(&input.durable, 1000, 180)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&input.durable, 1000, 300, 180)
                .unwrap()
        );
        assert!(
            store
                .complete_burn_check_assessment(&input.durable, &json, 1001, 180)
                .unwrap()
        );
        let request = ReportRequest {
            environment_key: "native".into(),
            window: ReportWindow {
                start_epoch: 0,
                end_epoch: i64::MAX,
            },
            computed_at_epoch: 1001,
        };
        let read = || check_report_progress(directory.path(), &request, "over_exploring").unwrap();
        let baseline = read().coverage.unwrap();
        assert!(baseline.total.is_some());

        let (agent, session, format, records) = native_sources::read_sources().remove(0);
        store
            .set_check_enabled(DetectorId::ScopeCreep, true)
            .unwrap();
        store
            .capture_burn_check_boundaries(&["scope_creep"], 0)
            .unwrap();
        let second =
            native_sources::publish(&store, &agent, &session, format, &records, directory.path());
        let second_input = prepare(
            &second,
            store
                .load_smart_check_inputs(
                    &second.session.key,
                    second.published_fence,
                    second.source_generation,
                    DetectorInput::OverExploring,
                )
                .unwrap(),
            &ModelCapabilities::jev_default(),
        )
        .unwrap();
        let assert_unknown = |checking| {
            let progress = read();
            let coverage = progress.coverage.unwrap();
            assert_eq!(coverage.reviewed, baseline.reviewed);
            assert_eq!(coverage.total, None);
            assert_eq!(progress.checking, checking);
        };
        assert_unknown(false);
        assert!(
            store
                .queue_burn_check_assessment(&second_input.durable, 1000, 180)
                .unwrap()
        );
        assert_unknown(true);
        assert!(
            store
                .claim_burn_check_assessment(&second_input.durable, 1000, 300, 180)
                .unwrap()
        );
        assert_unknown(true);
        for (status, error, json) in [
            ("failed", Some("provider_unavailable"), None),
            ("completed", None, None),
            ("completed", None, Some("{}")),
        ] {
            store
                .lock()
                .execute(
                    "UPDATE burn_check_assessment SET status = ?1, last_error_category = ?2,
                result_revision = input_revision, result_json = ?3 WHERE session_id = ?4",
                    params![status, error, json, second.session.key.session_id],
                )
                .unwrap();
            assert_unknown(false);
        }
        store
            .lock()
            .execute(
                "UPDATE burn_check_assessment SET status = 'queued', evaluator_revision = 'old'
            WHERE session_id = ?1",
                [&second.session.key.session_id],
            )
            .unwrap();
        assert_unknown(false);
        store
            .lock()
            .execute(
                "UPDATE session_evidence SET status = 'pending' WHERE session_id = ?1",
                [&second.session.key.session_id],
            )
            .unwrap();
        assert_unknown(false);
    }

    #[test]
    fn ignored_instruction_counts_require_distinct_terminal_evidence() {
        use antiburn_local::analysis::ignored_instructions::{
            AssessmentCoverage, AssessmentResult,
        };
        let mut result = AssessmentResult {
            input_revision: "input".into(),
            model_version: "test".into(),
            findings: Vec::new(),
            pending_rules: Vec::new(),
            unassessed_comparisons: Vec::new(),
            coverage: AssessmentCoverage {
                eligible_rules: 1,
                candidate_pairs: 4,
                selected_comparisons: 2,
                unselected_pairs: 2,
                skipped_rules: Vec::new(),
                skipped_actions: Vec::new(),
                processing_limit_reached: false,
                sampled_pass: true,
                selector_revision: 0,
                limitations: vec!["sampled_candidate_selection".into()],
                reassessed_comparison_ids: Vec::new(),
                reassessed_rule_ids: Vec::new(),
                reassessed_finding_ids: Vec::new(),
                instruction_sources: Vec::new(),
            },
            request_count: 1,
            input_tokens: 0,
            output_tokens: 0,
        };
        let read = |result: &AssessmentResult| {
            published_coverage(
                "ignored_instructions",
                "input",
                &serde_json::to_string(result).unwrap(),
                None,
            )
        };
        let (coverage, partial) = read(&result).unwrap();
        assert!(!partial);
        assert_eq!(coverage.reviewed, 2);
        assert_eq!(coverage.total, Some(4));
        assert_eq!(coverage.pending, 2);
        result.coverage.processing_limit_reached = true;
        assert_eq!(read(&result).unwrap().0.total, None);
        result.coverage.processing_limit_reached = false;
        result
            .unassessed_comparisons
            .push("ambiguous-answer".into());
        assert!(read(&result).is_none());
    }

    #[test]
    fn over_exploring_uncertainty_counts_targets_in_the_same_episode() {
        let (store, candidate) = fixture(None, "user");
        let input = prepare(
            &candidate,
            store
                .load_smart_check_inputs(
                    &candidate.session.key,
                    candidate.published_fence,
                    candidate.source_generation,
                    DetectorInput::OverExploring,
                )
                .unwrap(),
            &ModelCapabilities::jev_default(),
        )
        .unwrap();
        let mut result = OverExploringCheck.reduce(&input.plan, &[], false).unwrap();
        assert!(!result.unassessed.is_empty());
        let first = result.unassessed[0].clone();
        result.unassessed = vec![first.clone(), first.clone(), first];
        for (index, item) in result.unassessed.iter_mut().enumerate() {
            item.work_item_id = Some(format!("reason-{index}"));
        }
        for item in &mut result.unassessed {
            item.limitation = antiburn_local::checks::over_exploring::Abstention::UncertainDecision;
        }
        result.completed_work_item_ids = result
            .unassessed
            .iter()
            .filter_map(|item| item.work_item_id.clone())
            .collect();
        result.coverage.selected_items = 3;
        result.coverage.not_selected_items = 0;
        result.coverage.skipped_items = 0;
        result.coverage.limitations.clear();
        let json = serde_json::to_string(&publication(&input, result)).unwrap();
        let (coverage, _) =
            published_coverage("over_exploring", &input.durable.input_revision, &json, None)
                .unwrap();
        assert_eq!(coverage.reviewed, 3);
        assert_eq!(coverage.total, Some(3));
        assert_eq!(coverage.uncertain, 3);
    }

    #[test]
    fn skill_review_counts_reject_changed_or_unavailable_current_inventory() {
        let directory = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let (store, candidate) = fixture(Some(directory.path()), "user");
        let input = prepare(
            &candidate,
            store
                .load_smart_check_inputs(
                    &candidate.session.key,
                    candidate.published_fence,
                    candidate.source_generation,
                    DetectorInput::OverExploring,
                )
                .unwrap(),
            &ModelCapabilities::jev_default(),
        )
        .unwrap();
        let skill_directory = home.path().join(".config/opencode/skills/review");
        std::fs::create_dir_all(&skill_directory).unwrap();
        let skill_path = skill_directory.join("SKILL.md");
        std::fs::write(
            &skill_path,
            "---\nname: review\ndescription: Review parser boundaries.\n---\n",
        )
        .unwrap();
        let snapshot =
            skill_opportunities::current_skill_snapshot("opencode", None, home.path()).unwrap();
        let result = antiburn_local::checks::skill_opportunities::SkillOpportunitiesResult {
            findings: Vec::new(),
            decisions: Vec::new(),
            complete: true,
            coverage: antiburn_local::analysis::jev::JevCoverage {
                selected_items: 2,
                ..Default::default()
            },
        };
        let mut json = serde_json::to_value(result).unwrap();
        json["input_revision"] = serde_json::json!(input.durable.input_revision);
        json["inventory_revision"] = serde_json::json!(snapshot.revision());
        json["use_revision"] = serde_json::json!("bound-use");
        let mut durable = input.durable.clone();
        durable.check_id = "skill_opportunities".into();
        durable.evaluator_revision = crate::skill_opportunities_worker::CHECK.evaluator_revision();
        store
            .set_check_enabled(DetectorId::SkillOpportunities, true)
            .unwrap();
        store
            .capture_burn_check_boundaries(&["skill_opportunities"], 0)
            .unwrap();
        assert!(
            store
                .queue_burn_check_assessment(&durable, 1000, 180)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&durable, 1000, 300, 180)
                .unwrap()
        );
        assert!(
            store
                .complete_burn_check_assessment(&durable, &json.to_string(), 1001, 180)
                .unwrap()
        );
        let request = ReportRequest {
            environment_key: "native".into(),
            window: ReportWindow {
                start_epoch: 0,
                end_epoch: i64::MAX,
            },
            computed_at_epoch: 1001,
        };
        let read = || {
            check_report_progress_with_home(
                directory.path(),
                &request,
                "skill_opportunities",
                Some(home.path()),
            )
            .unwrap()
        };
        assert_eq!(read().coverage.unwrap().reviewed, 0);
        assert!(
            check_report_progress_with_home(
                directory.path(),
                &request,
                "skill_opportunities",
                None
            )
            .unwrap()
            .coverage
            .is_none()
        );
        json["use_revision"] = serde_json::json!("");
        store.lock().execute("UPDATE burn_check_assessment SET result_json = ?1 WHERE check_id = 'skill_opportunities'", [json.to_string()]).unwrap();
        assert!(read().coverage.is_none());
        json["use_revision"] = serde_json::json!("bound-use");
        store.lock().execute("UPDATE burn_check_assessment SET result_json = ?1 WHERE check_id = 'skill_opportunities'", [json.to_string()]).unwrap();
        assert!(read().coverage.is_some());
        std::fs::write(
            &skill_path,
            "---\nname: review\ndescription: Review a different procedure.\n---\n",
        )
        .unwrap();
        assert!(read().coverage.is_none());
        std::fs::remove_file(&skill_path).unwrap();
        assert!(read().coverage.is_none());
    }

    #[test]
    fn scope_and_skill_publication_shapes_keep_partial_review_notices() {
        let fixture = crate::scope_creep_worker::tests::NativeFixture::new(1);
        let candidate = fixture.publish();
        let input = crate::scope_creep_worker::load_input(
            &fixture.store,
            &candidate,
            &ModelCapabilities::jev_default(),
        )
        .unwrap();
        let mut result = input.check.reduce(&input.plan, &[], false).unwrap();
        result.coverage.selected_items = 1;
        result.coverage.not_selected_items = 2;
        result
            .coverage
            .limitations
            .push("partial_user_context".into());
        let json =
            serde_json::to_string(&crate::scope_creep_worker::publication(&input, result)).unwrap();
        let (coverage, partial) =
            published_coverage("scope_creep", &input.durable.input_revision, &json, None).unwrap();
        assert!(partial);
        assert_eq!(coverage.total, Some(3));
        assert_eq!(coverage.pending, 3);
        assert_eq!(coverage.uncertain, 0);
        assert!(published_coverage("scope_creep", "old-input", &json, None).is_none());
        let mut outcomes: serde_json::Value = serde_json::from_str(&json).unwrap();
        outcomes["assessment"]["decisions"][0]["status"] = serde_json::json!("uncertain");
        let (uncertain, _) = published_coverage(
            "scope_creep",
            &input.durable.input_revision,
            &outcomes.to_string(),
            None,
        )
        .unwrap();
        assert_eq!(uncertain.uncertain, coverage.uncertain + 1);
        assert_eq!(uncertain.reviewed, coverage.reviewed + 1);
        outcomes["assessment"]["decisions"][0]["status"] = serde_json::json!("clean");
        let (clean, _) = published_coverage(
            "scope_creep",
            &input.durable.input_revision,
            &outcomes.to_string(),
            None,
        )
        .unwrap();
        assert_eq!(clean.uncertain + 1, uncertain.uncertain);

        let result = antiburn_local::checks::skill_opportunities::SkillOpportunitiesResult {
            findings: Vec::new(),
            decisions: Vec::new(),
            complete: false,
            coverage: antiburn_local::analysis::jev::JevCoverage {
                selected_items: 4,
                not_selected_items: 3,
                limitations: vec!["partial_work_context".into()],
                ..Default::default()
            },
        };
        let mut json = serde_json::to_value(result).unwrap();
        json["input_revision"] = serde_json::json!("skill-input");
        json["inventory_revision"] = serde_json::json!("inventory");
        json["use_revision"] = serde_json::json!("use");
        let json = json.to_string();
        let (coverage, partial) = published_coverage(
            "skill_opportunities",
            "skill-input",
            &json,
            Some("inventory"),
        )
        .unwrap();
        assert!(partial);
        assert_eq!(coverage.reviewed, 0);
        assert_eq!(coverage.total, Some(7));
        assert_eq!(coverage.pending, 7);
        assert!(
            published_coverage("skill_opportunities", "old-input", &json, Some("inventory"))
                .is_none()
        );
        assert!(
            published_coverage(
                "skill_opportunities",
                "skill-input",
                &json,
                Some("changed-inventory")
            )
            .is_none()
        );
        assert!(published_coverage("skill_opportunities", "skill-input", &json, None).is_none());
        let mut invalid_use: serde_json::Value = serde_json::from_str(&json).unwrap();
        invalid_use["use_revision"] = serde_json::json!("");
        assert!(
            published_coverage(
                "skill_opportunities",
                "skill-input",
                &invalid_use.to_string(),
                Some("inventory")
            )
            .is_none()
        );
    }

    #[test]
    fn progress_tracks_current_window_work_without_inventing_clean_results() {
        let directory = tempfile::tempdir().unwrap();
        let (store, candidate) = fixture(Some(directory.path()), "user");
        let snapshot = store
            .load_smart_check_inputs(
                &candidate.session.key,
                candidate.published_fence,
                candidate.source_generation,
                DetectorInput::OverExploring,
            )
            .unwrap();
        let input = prepare(&candidate, snapshot, &ModelCapabilities::jev_default()).unwrap();
        let request = ReportRequest {
            environment_key: "native".into(),
            window: ReportWindow {
                start_epoch: 0,
                end_epoch: i64::MAX,
            },
            computed_at_epoch: 1000,
        };
        let read = || check_report_progress(directory.path(), &request, "over_exploring").unwrap();
        assert!(!read().checking);
        assert!(
            store
                .queue_burn_check_assessment(&input.durable, 1000, 180)
                .unwrap()
        );
        assert!(read().checking);
        assert!(read().coverage.is_none());
        store
            .lock()
            .execute(
                "UPDATE turn SET is_compaction_boundary = 1 WHERE turn_index = 0",
                [],
            )
            .unwrap();
        assert!(read().partial_context);
        assert!(read().coverage.is_none());
        store
            .lock()
            .execute(
                "UPDATE turn SET is_compaction_boundary = 0 WHERE turn_index = 0",
                [],
            )
            .unwrap();
        assert!(
            !check_report_progress(directory.path(), &request, "scope_creep")
                .unwrap()
                .checking
        );
        assert!(
            store
                .claim_burn_check_assessment(&input.durable, 1000, 300, 180)
                .unwrap()
        );
        assert!(read().checking);
        let mut result = OverExploringCheck.reduce(&input.plan, &[], false).unwrap();
        result.coverage.selected_items = 2;
        result.coverage.not_selected_items = 3;
        result
            .coverage
            .limitations
            .push("selected_content_truncated".into());
        let json = serde_json::to_string(&publication(&input, result)).unwrap();
        assert!(
            store
                .complete_burn_check_assessment(&input.durable, &json, 1001, 180)
                .unwrap()
        );
        let progress = read();
        assert!(!progress.checking);
        assert!(progress.partial_context);
        let coverage = progress.coverage.unwrap();
        assert_eq!(coverage.reviewed, 0);
        assert_eq!(coverage.total, Some(5));
        assert_eq!(coverage.pending, Some(5));
        assert_eq!(coverage.uncertain, Some(0));
        store.lock().execute("UPDATE burn_check_assessment SET status = 'failed', last_error_category = 'continuing'", []).unwrap();
        store
            .save_burn_check_scheduling(&input.durable, Some(5), 0, 5)
            .unwrap();
        assert!(read().checking);
        assert_eq!(read().checking_count, 1);
        assert!(read().coverage.unwrap().continuing);
        store
            .lock()
            .execute(
                "UPDATE burn_check_assessment SET scheduling_revision = NULL",
                [],
            )
            .unwrap();
        for mutation in [
            "evaluator_revision = 'old'",
            "source_generation = 999",
            "published_fence = 999",
            "incarnation = 999",
            "source_fingerprint = 'old'",
            "result_revision = 'old'",
        ] {
            store
                .lock()
                .execute(&format!("UPDATE burn_check_assessment SET {mutation}"), [])
                .unwrap();
            assert!(read().coverage.is_none(), "{mutation}");
            assert!(!read().checking, "{mutation}");
            store
                .lock()
                .execute(
                    "UPDATE burn_check_assessment SET evaluator_revision = ?1,
                source_generation = ?2, published_fence = ?3, incarnation = ?4,
                source_fingerprint = ?5, result_revision = input_revision",
                    params![
                        crate::over_exploring_worker::CHECK.evaluator_revision(),
                        candidate.source_generation,
                        candidate.published_fence,
                        candidate.incarnation,
                        candidate.source_fingerprint
                    ],
                )
                .unwrap();
        }
        let outside = ReportRequest {
            window: ReportWindow {
                start_epoch: -2,
                end_epoch: -1,
            },
            ..request
        };
        assert!(
            check_report_progress(directory.path(), &outside, "over_exploring")
                .unwrap()
                .coverage
                .is_none()
        );
    }
}
