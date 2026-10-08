use super::*;
use crate::dto::ChecksReviewCoveragePayload;
use crate::jev::worker::JevCheckDescriptor;
use serde::Deserialize;

pub(crate) struct CheckReportProgress {
    pub checking: bool,
    pub partial_context: bool,
    pub coverage: Option<ChecksReviewCoveragePayload>,
}

pub(crate) fn check_report_progress(
    data_dir: &Path,
    request: &ReportRequest,
    check_id: &str,
) -> Result<CheckReportProgress> {
    let home = antiburn_local::paths::home_dir();
    check_report_progress_with_home(data_dir, request, check_id, home.as_deref())
}

fn check_report_progress_with_home(
    data_dir: &Path,
    request: &ReportRequest,
    check_id: &str,
    home: Option<&Path>,
) -> Result<CheckReportProgress> {
    let evaluator_revision = match check_id {
        "scope_creep" => crate::scope_creep_worker::CHECK.evaluator_revision(),
        "over_exploring" => crate::over_exploring_worker::CHECK.evaluator_revision(),
        "skill_opportunities" => crate::skill_opportunities_worker::CHECK.evaluator_revision(),
        "ignored_instructions" => {
            antiburn_local::analysis::ignored_instructions::evaluator_revision()
        }
        _ => anyhow::bail!("unknown persisted check: {check_id}"),
    };
    let connection = open_read_only(data_dir, REPORT_BUSY_TIMEOUT)?;
    let current_evidence = crate::store::revision_sql::current_evidence("e", "s");
    let sql = format!(
        "SELECT a.status, a.last_error_category, a.input_revision, a.result_revision,
                a.result_json, c.coverage_json,
                EXISTS (SELECT 1 FROM turn t LEFT JOIN turn_content p ON p.turn_rowid = t.rowid
                  WHERE t.environment_key = s.environment_key AND t.agent = s.agent
                    AND t.session_id = s.session_id AND t.claim_fence = e.published_fence
                    AND t.scope = 'main' AND (t.is_compaction_boundary != 0
                      OR (p.kind != 'thinking' AND p.truncated != 0)
                      OR (t.role = 'user' AND p.turn_rowid IS NULL))),
                (SELECT COUNT(*) FROM turn t WHERE t.environment_key = s.environment_key
                  AND t.agent = s.agent AND t.session_id = s.session_id
                  AND t.claim_fence = e.published_fence AND t.scope = 'main'),
                (SELECT role FROM turn t WHERE t.environment_key = s.environment_key
                  AND t.agent = s.agent AND t.session_id = s.session_id
                  AND t.claim_fence = e.published_fence AND t.scope = 'main'
                   ORDER BY turn_index LIMIT 1),
                 s.agent, s.cwd, e.evidence_json, s.session_id
          FROM session s
         JOIN session_evidence e ON e.environment_key = s.environment_key AND e.agent = s.agent
           AND e.session_id = s.session_id
         LEFT JOIN session_coverage c ON c.environment_key = s.environment_key AND c.agent = s.agent
            AND c.session_id = s.session_id AND c.claim_fence = e.published_fence
          LEFT JOIN burn_check_assessment a ON a.environment_key = s.environment_key AND a.agent = s.agent
            AND a.session_id = s.session_id AND a.check_id = :check_id
            AND a.incarnation = s.incarnation AND a.source_generation = s.source_generation
            AND a.source_fingerprint IS s.source_fingerprint AND a.published_fence = e.published_fence
            AND a.evaluator_revision = :evaluator_revision
          WHERE s.environment_key = :environment_key
           AND COALESCE(s.updated_at_epoch, s.started_at_epoch) >= :window_start
           AND COALESCE(s.updated_at_epoch, s.started_at_epoch) < :window_end
            AND e.status = 'ready' AND e.processed_fingerprint IS s.source_fingerprint AND {current_evidence}"
    );
    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query(named_params![
        ":environment_key": request.environment_key,
        ":check_id": check_id,
        ":window_start": request.window.start_epoch,
        ":window_end": request.window.end_epoch,
        ":evaluator_revision": evaluator_revision,
        ":parser_revision": PARSER_REVISION,
        ":analyzer_revision": ANALYZER_REVISION,
        ":evidence_schema_revision": EVIDENCE_SCHEMA_REVISION,
    ])?;
    let mut progress = CheckReportProgress {
        checking: false,
        partial_context: false,
        coverage: None,
    };
    let mut missing_denominator = false;
    while let Some(row) = rows.next()? {
        let agent: String = row.get(9)?;
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
        progress.checking |= matches!(status.as_deref(), Some("queued" | "running"));
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
        let continuing =
            status.as_deref() == Some("failed") && error.as_deref() == Some("continuing");
        let input: Option<String> = row.get(2)?;
        let revision: Option<String> = row.get(3)?;
        let json: Option<String> = row.get(4)?;
        if !matches!(status.as_deref(), Some("completed" | "failed"))
            || input.is_none()
            || input != revision
        {
            missing_denominator = true;
            continue;
        }
        let (Some(json), Some(input)) = (json, input) else {
            missing_denominator = true;
            continue;
        };
        let inventory_revision = if check_id == "skill_opportunities" {
            home.and_then(|home| {
                skill_opportunities::current_skill_snapshot(&agent, cwd.map(PathBuf::from), home)
            })
            .map(|snapshot| snapshot.revision())
        } else {
            None
        };
        let Some((mut coverage, partial)) =
            published_coverage(check_id, &input, &json, inventory_revision.as_deref())
        else {
            missing_denominator = true;
            continue;
        };
        if status.as_deref() == Some("failed")
            && check_id != "over_exploring"
            && !matches!(error.as_deref(), Some("sampling_incomplete" | "continuing"))
        {
            missing_denominator = true;
            continue;
        }
        coverage.continuing = continuing;
        if partial_source {
            coverage.total = None;
        }
        progress.partial_context |= partial;
        if let Some(total) = &mut progress.coverage {
            total.reviewed += coverage.reviewed;
            total.total = total
                .total
                .zip(coverage.total)
                .map(|(left, right)| left + right);
            total.uncertain += coverage.uncertain;
            total.pending += coverage.pending;
            total.continuing |= coverage.continuing;
        } else {
            progress.coverage = Some(coverage);
        }
    }
    if missing_denominator && let Some(coverage) = &mut progress.coverage {
        coverage.total = None;
    }
    Ok(progress)
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

pub(super) fn published_coverage(
    check_id: &str,
    input_revision: &str,
    json: &str,
    inventory_revision: Option<&str>,
) -> Option<(ChecksReviewCoveragePayload, bool)> {
    if check_id == "ignored_instructions" {
        let result: antiburn_local::analysis::ignored_instructions::AssessmentResult =
            serde_json::from_str(json).ok()?;
        if result.input_revision != input_revision {
            return None;
        }
        let partial = result.coverage.limitations.iter().any(|limit| {
            !matches!(
                limit.as_str(),
                "sampled_candidate_selection" | "sampled_content_selection"
            )
        });
        return Some((
            ChecksReviewCoveragePayload {
                reviewed: result
                    .coverage
                    .selected_comparisons
                    .saturating_sub(result.unassessed_comparisons.len())
                    as u64,
                total: (!partial).then_some(result.coverage.candidate_pairs as u64),
                uncertain: result.unassessed_comparisons.len() as u64,
                pending: result.coverage.unselected_pairs as u64,
                continuing: false,
            },
            partial,
        ));
    }
    let (coverage, uncertain, scope_partial) = if check_id == "skill_opportunities" {
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
        let uncertain = saved
            .result
            .decisions
            .iter()
            .filter(|decision| {
                matches!(
                    decision.outcome,
                    SkillOpportunityOutcome::Unassessed | SkillOpportunityOutcome::Uncertain
                )
            })
            .count();
        (saved.result.coverage, uncertain, false)
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
            .filter(|decision| {
                matches!(
                    decision.status,
                    ScopeCreepStatus::Unassessed | ScopeCreepStatus::Uncertain
                )
            })
            .count();
        (
            saved.assessment.coverage,
            uncertain,
            saved.assessment.session_limitation.is_some(),
        )
    } else {
        let saved: crate::over_exploring_worker::Publication = serde_json::from_str(json).ok()?;
        if saved.input_revision != input_revision {
            return None;
        }
        let uncertain = saved
            .assessment
            .unassessed
            .iter()
            .filter_map(|item| item.work_item_id.as_deref())
            .collect::<BTreeSet<_>>()
            .len();
        let coverage = saved.assessment.coverage;
        let partial = !coverage.limitations.is_empty();
        return Some((
            ChecksReviewCoveragePayload {
                reviewed: saved
                    .assessment
                    .completed_work_item_ids
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len() as u64,
                total: (!partial)
                    .then_some((coverage.selected_items + coverage.not_selected_items) as u64),
                uncertain: uncertain as u64,
                pending: coverage.not_selected_items as u64,
                continuing: false,
            },
            partial,
        ));
    };
    let partial = scope_partial || !coverage.limitations.is_empty();
    Some((
        ChecksReviewCoveragePayload {
            reviewed: coverage.selected_items as u64,
            total: (!partial).then_some(
                (coverage.selected_items + coverage.skipped_items + coverage.not_selected_items)
                    as u64,
            ),
            uncertain: uncertain as u64,
            pending: coverage.not_selected_items as u64,
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
        result.coverage.selected_items = 3;
        result.coverage.not_selected_items = 0;
        result.coverage.skipped_items = 0;
        result.coverage.limitations.clear();
        let json = serde_json::to_string(&publication(&input, result)).unwrap();
        let (coverage, _) =
            published_coverage("over_exploring", &input.durable.input_revision, &json, None)
                .unwrap();
        assert_eq!(coverage.reviewed, 0);
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
        assert_eq!(read().coverage.unwrap().reviewed, 2);
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
        assert_eq!(coverage.total, None);
        assert_eq!(coverage.pending, 2);
        assert!(coverage.uncertain > 0);
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
        assert_eq!(uncertain.uncertain, coverage.uncertain);
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
        assert_eq!(coverage.reviewed, 4);
        assert_eq!(coverage.total, None);
        assert_eq!(coverage.pending, 3);
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
        assert_eq!(coverage.total, None);
        assert_eq!(coverage.pending, 3);
        assert!(coverage.uncertain > 0);
        store.lock().execute("UPDATE burn_check_assessment SET status = 'failed', last_error_category = 'continuing'", []).unwrap();
        assert!(!read().checking);
        assert!(read().coverage.unwrap().continuing);
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
