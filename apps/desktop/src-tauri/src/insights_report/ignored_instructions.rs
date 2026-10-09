use super::findings::CurrentFindingSession;
use super::*;

pub(super) fn has_published_sampled_instruction_assessment_in(
    connection: &rusqlite::Connection,
    request: &ReportRequest,
) -> Result<bool> {
    let current_evidence = crate::store::revision_sql::current_evidence("e", "s");
    let sql = format!(
        "SELECT e.evidence_json, s.agent, s.session_id, s.incarnation,
                s.source_generation, s.source_fingerprint, e.published_fence
           FROM session s
           JOIN session_evidence e
             ON e.environment_key = s.environment_key AND e.agent = s.agent
            AND e.session_id = s.session_id
           JOIN burn_check_assessment a
             ON a.environment_key = s.environment_key AND a.agent = s.agent
            AND a.session_id = s.session_id
            AND a.check_id = 'ignored_instructions' AND a.status = 'completed'
            AND a.incarnation = s.incarnation
            AND a.source_generation = s.source_generation
            AND a.source_fingerprint IS s.source_fingerprint
            AND a.published_fence = e.published_fence
            AND a.input_revision = a.result_revision
           WHERE s.environment_key = :environment_key
             AND COALESCE(s.updated_at_epoch, s.started_at_epoch) >= :window_start
             AND COALESCE(s.updated_at_epoch, s.started_at_epoch) < :window_end
             AND e.status = 'ready' AND {current_evidence}"
    );
    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query(named_params![
        ":environment_key": request.environment_key,
        ":window_start": request.window.start_epoch,
        ":window_end": request.window.end_epoch,
        ":parser_revision": PARSER_REVISION,
        ":analyzer_revision": ANALYZER_REVISION,
        ":evidence_schema_revision": EVIDENCE_SCHEMA_REVISION,
    ])?;
    while let Some(row) = rows.next()? {
        let evidence: SessionEvidence = serde_json::from_str(&row.get::<_, String>(0)?)
            .context("stored session evidence is invalid")?;
        let agent: String = row.get(1)?;
        let session_id: String = row.get(2)?;
        let source_fingerprint: Option<String> = row.get(5)?;
        let identity = IgnoredInstructionSessionIdentity {
            environment_key: &request.environment_key,
            agent: &agent,
            session_id: &session_id,
            incarnation: row.get(3)?,
            source_generation: row.get(4)?,
            source_fingerprint: source_fingerprint.as_deref(),
            published_fence: row.get(6)?,
        };
        if ignored_instruction_result_for(connection, &evidence, identity)?
            .as_ref()
            .is_some_and(sampled_coverage)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Read the same current assessment used by the report for a bounded session list.
pub(crate) fn ignored_instruction_session_statuses(
    data_dir: &Path,
    keys: &[crate::store::SessionKey],
) -> Result<Vec<crate::dto::IgnoredInstructionSessionStatus>> {
    let connection = open_read_only(data_dir, REPORT_BUSY_TIMEOUT)?;
    ignored_instruction_session_statuses_in(&connection, keys)
}

fn ignored_instruction_session_statuses_in(
    connection: &rusqlite::Connection,
    keys: &[crate::store::SessionKey],
) -> Result<Vec<crate::dto::IgnoredInstructionSessionStatus>> {
    let current_evidence = crate::store::revision_sql::current_evidence("e", "s");
    let sql = format!("SELECT e.evidence_json, s.incarnation, s.source_generation,
                s.source_fingerprint, e.published_fence, e.status,
                 (e.status = 'ready' AND e.processed_fingerprint IS s.source_fingerprint
                  AND {current_evidence}),
                 EXISTS (
                     SELECT 1 FROM turn_content AS content
                     JOIN turn AS content_turn ON content_turn.rowid = content.turn_rowid
                     WHERE content_turn.environment_key = s.environment_key
                       AND content_turn.agent = s.agent
                       AND content_turn.session_id = s.session_id
                       AND content.kind <> 'thinking' AND length(content.content) > 0
                  ),
                   a.status, a.last_error_category, a.incarnation,
                   a.source_generation, a.source_fingerprint, a.published_fence,
                   a.evaluator_revision
            FROM session s LEFT JOIN session_evidence e
              ON e.environment_key = s.environment_key AND e.agent = s.agent
             AND e.session_id = s.session_id
            LEFT JOIN burn_check_assessment a
              ON a.environment_key = s.environment_key AND a.agent = s.agent
             AND a.session_id = s.session_id AND a.check_id = 'ignored_instructions'
            WHERE s.environment_key = :environment_key AND s.agent = :agent AND s.session_id = :session_id
               AND EXISTS (SELECT 1 FROM setting WHERE key = 'internal:burnChecksEnabledAtEpochV1')");
    let mut statement = connection.prepare(&sql)?;
    let current_evaluator_revision =
        antiburn_local::analysis::ignored_instructions::evaluator_revision();
    keys.iter()
        .map(|key| {
            let row = statement
                .query_row(
                    named_params![
                        ":environment_key": key.environment_key,
                        ":agent": key.agent,
                        ":session_id": key.session_id,
                        ":parser_revision": PARSER_REVISION,
                        ":analyzer_revision": ANALYZER_REVISION,
                        ":evidence_schema_revision": EVIDENCE_SCHEMA_REVISION
                    ],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, u64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, Option<bool>>(6)?,
                            row.get::<_, bool>(7)?,
                            row.get::<_, Option<String>>(8)?,
                            row.get::<_, Option<String>>(9)?,
                            row.get::<_, Option<u64>>(10)?,
                            row.get::<_, Option<i64>>(11)?,
                            row.get::<_, Option<String>>(12)?,
                            row.get::<_, Option<i64>>(13)?,
                            row.get::<_, Option<String>>(14)?,
                        ))
                    },
                )
                .optional()?;
            let Some((
                json,
                incarnation,
                generation,
                fingerprint,
                fence,
                status,
                current,
                has_content,
                assessment_status,
                assessment_error_category,
                assessment_incarnation,
                assessment_generation,
                assessment_fingerprint,
                assessment_fence,
                assessment_evaluator_revision,
            )) = row
            else {
                return Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::Checking,
                    Some("Waiting for current session evidence."),
                ));
            };
            if !has_content {
                return Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::NotAssessed,
                    Some("This session has no saved content to check."),
                ));
            }
            if status.as_deref() == Some("unsupported") {
                return Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::CouldntCheck,
                    Some("This session source format is not supported."),
                ));
            }
            if status.as_deref() == Some("failed") {
                return Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::CouldntCheck,
                    Some("Could not read complete session evidence."),
                ));
            }
            let Some(json) = json else {
                return Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::Checking,
                    Some("Waiting for current session evidence."),
                ));
            };
            if current != Some(true) || status.as_deref() != Some("ready") {
                return Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::Checking,
                    Some("Waiting for current session evidence."),
                ));
            }
            let Some(fence) = fence else {
                return Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::Checking,
                    Some("Waiting for published session evidence."),
                ));
            };
            let Ok(evidence) = serde_json::from_str::<SessionEvidence>(&json) else {
                return Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::CouldntCheck,
                    Some("Could not read complete session evidence."),
                ));
            };
            let current_assessment = assessment_incarnation == Some(incarnation)
                && assessment_generation == Some(generation)
                && assessment_fingerprint == fingerprint
                && assessment_fence == Some(fence)
                && assessment_evaluator_revision.as_deref()
                    == Some(current_evaluator_revision.as_str());
            if current_assessment && assessment_status.as_deref() == Some("failed") {
                if assessment_error_category.as_deref() == Some("no_candidates") {
                    return Ok(ignored_session_status(
                        crate::dto::SessionHygieneStatus::NoCandidates,
                        Some("no_candidates"),
                    ));
                }
                if assessment_error_category.as_deref() == Some("continuing") {
                    return Ok(ignored_session_status(
                        crate::dto::SessionHygieneStatus::Checking,
                        Some("An instruction assessment is continuing."),
                    ));
                }
                return Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::CouldntCheck,
                    Some(ignored_assessment_failure_reason(
                        assessment_error_category.as_deref(),
                    )),
                ));
            }
            let result = ignored_instruction_result_for(
                connection,
                &evidence,
                IgnoredInstructionSessionIdentity {
                    environment_key: &key.environment_key,
                    agent: &key.agent,
                    session_id: &key.session_id,
                    incarnation,
                    source_generation: generation,
                    source_fingerprint: fingerprint.as_deref(),
                    published_fence: fence,
                },
            )?;
            let Some(result) = result else {
                return Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::Checking,
                    Some("Waiting for a current instruction assessment."),
                ));
            };
            if assessment_status.as_deref() != Some("completed") {
                return Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::Checking,
                    Some("Waiting for a current instruction assessment."),
                ));
            }
            let Some(session_findings) =
                ignored_instruction_findings_for_evidence(&evidence, &result)
            else {
                return Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::CouldntCheck,
                    Some("The current instruction assessment is unavailable."),
                ));
            };
            if !session_findings.is_empty() {
                Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::Finding,
                    sampled_coverage(&result)
                        .then_some("Reviewed a selected sample of instruction and action pairs."),
                ))
            } else if ignored_result_has_scoped_no_issues(&result, &session_findings) {
                Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::Clean,
                    sampled_coverage(&result).then_some(
                        "No issues found in the selected sample of instruction and action pairs.",
                    ),
                ))
            } else {
                Ok(ignored_session_status(
                    crate::dto::SessionHygieneStatus::CouldntCheck,
                    Some(ignored_assessment_reason(&result)),
                ))
            }
        })
        .collect()
}

fn ignored_assessment_failure_reason(category: Option<&str>) -> &'static str {
    match category {
        Some("authentication_rejected") => "The provider rejected the API key.",
        Some("rate_limited") => "The provider rate limit interrupted the assessment.",
        Some("usage_limit") => "The configured usage limit interrupted the assessment.",
        Some("provider_overloaded" | "provider_unavailable") => {
            "The provider was unavailable for the assessment."
        }
        Some("outcome_unknown") => "The provider did not confirm the request outcome.",
        Some("unsupported_format") => "This session source format is not supported.",
        Some("evidence_unavailable") => "Current session evidence is unavailable.",
        Some("cancelled") => "The instruction assessment was cancelled.",
        Some(
            "invalid_request_schema"
            | "invalid_request"
            | "invalid_response"
            | "response_too_large"
            | "response_decode"
            | "response_usage_exceeded"
            | "invalid_assessment_plan"
            | "progress_storage_failed",
        ) => "The instruction assessment could not be completed.",
        _ => "The instruction assessment failed.",
    }
}

fn ignored_session_status(
    status: crate::dto::SessionHygieneStatus,
    reason: Option<&'static str>,
) -> crate::dto::IgnoredInstructionSessionStatus {
    crate::dto::IgnoredInstructionSessionStatus { status, reason }
}

fn ignored_assessment_reason(
    result: &antiburn_local::analysis::ignored_instructions::AssessmentResult,
) -> &'static str {
    if !sampled_coverage(result)
        && (result.coverage.processing_limit_reached || result.coverage.unselected_pairs > 0)
    {
        "The assessment limit prevented checking every eligible rule and action."
    } else if result
        .coverage
        .limitations
        .contains(&"current_file_not_historical_proof".to_owned())
    {
        "Current instruction files do not prove which rules applied during this session."
    } else if result
        .coverage
        .limitations
        .iter()
        .any(|limit| limit == "historical_instruction_snapshot_unavailable")
    {
        "No recorded instruction snapshot proves which rules applied during this session."
    } else if result
        .coverage
        .limitations
        .contains(&"source_evidence_is_partial".to_owned())
    {
        "The session evidence is incomplete."
    } else if !result.pending_rules.is_empty() {
        "The session has no observed completion boundary for an eventual instruction."
    } else if !result.unassessed_comparisons.is_empty() {
        "Some instruction comparisons did not reach a result."
    } else if result.coverage.selected_comparisons == 0 {
        "No eligible instruction and action comparison was available."
    } else {
        "The available assessment evidence is incomplete."
    }
}

pub(crate) fn current_ignored_instruction_result(
    connection: &rusqlite::Connection,
    session: &CurrentFindingSession,
) -> Result<Option<antiburn_local::analysis::ignored_instructions::AssessmentResult>> {
    ignored_instruction_result_for(
        connection,
        &session.evidence,
        IgnoredInstructionSessionIdentity {
            environment_key: &session.environment_key,
            agent: &session.agent,
            session_id: &session.session_id,
            incarnation: session.incarnation,
            source_generation: session.source_generation,
            source_fingerprint: session.source_fingerprint.as_deref(),
            published_fence: session.published_fence,
        },
    )
}

pub(crate) struct IgnoredInstructionSessionIdentity<'a> {
    pub environment_key: &'a str,
    pub agent: &'a str,
    pub session_id: &'a str,
    pub incarnation: u64,
    pub source_generation: i64,
    pub source_fingerprint: Option<&'a str>,
    pub published_fence: i64,
}

pub(crate) fn ignored_instruction_result_for(
    connection: &rusqlite::Connection,
    evidence: &SessionEvidence,
    identity: IgnoredInstructionSessionIdentity<'_>,
) -> Result<Option<antiburn_local::analysis::ignored_instructions::AssessmentResult>> {
    use antiburn_local::analysis::ignored_instructions::{CHECK_ID, source_supported};

    if !source_supported(evidence.capabilities.source_format) {
        return Ok(None);
    }
    let stored = connection
        .query_row(
            "SELECT result_revision, result_json, input_revision, status
               FROM burn_check_assessment
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND check_id = ?4 AND incarnation = ?5 AND source_generation = ?6
                 AND source_fingerprint IS ?7 AND published_fence = ?8
                 AND status IN ('completed', 'failed', 'queued', 'running', 'superseded')
                 AND result_revision IS NOT NULL",
            params![
                identity.environment_key,
                identity.agent,
                identity.session_id,
                CHECK_ID,
                identity.incarnation,
                identity.source_generation,
                identity.source_fingerprint,
                identity.published_fence,
            ],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((Some(revision), Some(result_json), input_revision, status)) = stored else {
        return Ok(None);
    };
    let Ok(mut result) = serde_json::from_str::<
        antiburn_local::analysis::ignored_instructions::AssessmentResult,
    >(&result_json) else {
        return Ok(None);
    };
    if result.input_revision != revision
        || result.model_version != antiburn_local::analysis::ignored_instructions::ASSESSMENT_MODEL
        || ignored_instruction_findings_for_evidence(evidence, &result).is_none()
    {
        return Ok(None);
    }
    if input_revision.as_deref() != Some(revision.as_str())
        || (status != "completed" && status != "failed")
        || (status == "failed" && result.findings.is_empty())
    {
        return Ok(None);
    }
    for finding in &mut result.findings {
        if !finding.decision_record().is_some_and(|decision| {
            decision.source_generation == identity.source_generation
                && decision.source_fingerprint.as_deref() == identity.source_fingerprint
                && decision.publication_fence == identity.published_fence
                && decision.model == result.model_version
        }) {
            finding.decision = None;
        }
    }
    Ok(Some(result))
}

pub(crate) fn ignored_instruction_findings(
    session: &CurrentFindingSession,
    result: &antiburn_local::analysis::ignored_instructions::AssessmentResult,
) -> Option<Vec<Finding>> {
    ignored_instruction_findings_for_evidence(&session.evidence, result)
}

pub(crate) fn ignored_instruction_findings_for_evidence(
    evidence: &SessionEvidence,
    result: &antiburn_local::analysis::ignored_instructions::AssessmentResult,
) -> Option<Vec<Finding>> {
    let mut seen = BTreeSet::new();
    result
        .findings
        .iter()
        .map(|finding| {
            if !seen.insert(finding.id.as_str()) {
                return None;
            }
            Finding::ignored_instruction(evidence, &result.input_revision, finding)
        })
        .collect()
}

#[derive(Default)]
pub(super) struct IgnoredInstructionReportCounts {
    pub eligible: u64,
    pub assessed: u64,
    pub clean: u64,
    pub clean_agents: BTreeSet<String>,
    pub finding_agents: BTreeSet<String>,
    pub unavailable: u64,
    pub not_applicable: u64,
    pub finding_sessions: u64,
    pub examples: Vec<SessionExample>,
}

pub(super) fn apply_ignored_instruction_counts(
    report: &mut EfficiencyReport,
    ignored: IgnoredInstructionReportCounts,
) {
    apply_persisted_check_counts(report, DetectorId::IgnoredInstructions, ignored);
}

pub(super) fn apply_persisted_check_counts(
    report: &mut EfficiencyReport,
    detector: DetectorId,
    ignored: IgnoredInstructionReportCounts,
) {
    let index = detector.index();
    let counts = &mut report.detectors[index];
    counts.eligible = ignored.eligible;
    counts.assessed = ignored.assessed;
    counts.finding = ignored.finding_sessions;
    counts.clean = ignored.clean;
    counts.unavailable = ignored.unavailable;
    counts.not_applicable = ignored.not_applicable;
    report.finding_agents[index] = ignored.finding_agents;
    report.clean_agents[index] = ignored.clean_agents;
    report.detector_statuses[index] = if ignored.finding_sessions > 0 {
        DetectorStatus::Findings(DetectorFindings {
            finding_sessions: ignored.finding_sessions,
            examples: ignored.examples,
        })
    } else if ignored.eligible > 0 && ignored.unavailable == 0 && ignored.clean == ignored.eligible
    {
        DetectorStatus::Clean
    } else {
        DetectorStatus::NotAssessed(if ignored.eligible == 0 {
            NotAssessedReason::CapabilityMissing
        } else {
            NotAssessedReason::IncompleteEvidence
        })
    };
    report.detector_estimated_token_burn_basis_points[index] = None;
}

pub(crate) fn ignored_result_has_scoped_no_issues(
    result: &antiburn_local::analysis::ignored_instructions::AssessmentResult,
    findings: &[Finding],
) -> bool {
    let sampled = sampled_coverage(result);
    findings.is_empty()
        && (sampled || result.coverage.unselected_pairs == 0)
        && result.coverage.skipped_rules.is_empty()
        && result.coverage.skipped_actions.is_empty()
        && (sampled || !result.coverage.processing_limit_reached)
        && result.coverage.limitations.iter().all(|limit| {
            sampled
                && matches!(
                    limit.as_str(),
                    "sampled_candidate_selection"
                        | "sampled_content_selection"
                        | "assessment_candidate_limit"
                )
        })
        && result.pending_rules.is_empty()
        && result.unassessed_comparisons.is_empty()
        && (result.coverage.selected_comparisons > 0
            || (result.coverage.eligible_rules == 0 && result.coverage.candidate_pairs == 0))
}

fn sampled_coverage(
    result: &antiburn_local::analysis::ignored_instructions::AssessmentResult,
) -> bool {
    result.coverage.sampled_pass && result.coverage.selector_revision > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use antiburn_local::analysis::ignored_instructions::{
        ASSESSMENT_MODEL, AssessmentCoverage, AssessmentResult,
    };
    use antiburn_local::analysis::{
        EvidenceSource, SessionEvidenceAccumulator, SourceCapabilities, SourceKind, TurnFacts,
    };

    #[test]
    fn persisted_advisory_findings_win_over_clean_sessions_without_savings() {
        let mut report = EfficiencyReportAccumulator::new().finish(ReportContext {
            environment_key: "native".into(),
            window: ReportWindow {
                start_epoch: 0,
                end_epoch: 100,
            },
            computed_at_epoch: 100,
            parser_revision: PARSER_REVISION,
            analyzer_revision: ANALYZER_REVISION,
            evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
            coverage: Default::default(),
        });
        apply_persisted_check_counts(
            &mut report,
            DetectorId::SkillOpportunities,
            IgnoredInstructionReportCounts {
                eligible: 2,
                assessed: 2,
                clean: 1,
                finding_sessions: 1,
                examples: vec![SessionExample {
                    agent: "opencode".into(),
                    session_id: "finding-session".into(),
                }],
                ..Default::default()
            },
        );
        let index = DetectorId::SkillOpportunities.index();
        assert_eq!(report.detectors[index].assessed, 2);
        assert_eq!(report.detectors[index].finding, 1);
        assert_eq!(report.detectors[index].clean, 1);
        assert!(
            matches!(&report.detector_statuses[index], DetectorStatus::Findings(values) if values.examples[0].session_id == "finding-session")
        );
        assert_eq!(
            report.detector_estimated_token_burn_basis_points[index],
            None
        );
    }

    fn result() -> AssessmentResult {
        AssessmentResult {
            input_revision: "synthetic".into(),
            model_version: ASSESSMENT_MODEL.into(),
            findings: Vec::new(),
            pending_rules: Vec::new(),
            unassessed_comparisons: Vec::new(),
            coverage: AssessmentCoverage {
                eligible_rules: 2,
                candidate_pairs: 12,
                selected_comparisons: 4,
                unselected_pairs: 8,
                skipped_rules: Vec::new(),
                skipped_actions: Vec::new(),
                processing_limit_reached: false,
                sampled_pass: true,
                selector_revision: 1,
                limitations: vec!["sampled_candidate_selection".into()],
                reassessed_comparison_ids: Vec::new(),
                reassessed_rule_ids: Vec::new(),
                reassessed_finding_ids: Vec::new(),
                instruction_sources: Vec::new(),
            },
            request_count: 1,
            input_tokens: 10,
            output_tokens: 2,
        }
    }

    fn session_status_connection() -> rusqlite::Connection {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (
                    environment_key TEXT, agent TEXT, session_id TEXT,
                    incarnation INTEGER, source_generation INTEGER, source_fingerprint TEXT);
                 CREATE TABLE session_evidence (
                    environment_key TEXT, agent TEXT, session_id TEXT, evidence_json TEXT,
                    published_fence INTEGER, status TEXT, analyzed_generation INTEGER,
                    processed_fingerprint TEXT, parser_revision INTEGER,
                    analyzer_revision INTEGER, evidence_schema_revision INTEGER);
                 CREATE TABLE turn (rowid INTEGER PRIMARY KEY, environment_key TEXT,
                    agent TEXT, session_id TEXT);
                 CREATE TABLE turn_content (turn_rowid INTEGER, kind TEXT, content TEXT);
                 CREATE TABLE setting (key TEXT);
                 CREATE TABLE burn_check_assessment (
                    environment_key TEXT, agent TEXT, session_id TEXT, check_id TEXT,
                    status TEXT, last_error_category TEXT, incarnation INTEGER,
                    source_generation INTEGER, source_fingerprint TEXT,
                    published_fence INTEGER, evaluator_revision TEXT,
                    input_revision TEXT, result_revision TEXT, result_json TEXT);
                 INSERT INTO session VALUES ('native', 'claude-code', 'failed', 1, 3, 'fp');
                 INSERT INTO turn VALUES (1, 'native', 'claude-code', 'failed');
                 INSERT INTO turn_content VALUES (1, 'assistant_text', 'Saved assistant text.');
                 INSERT INTO setting VALUES ('internal:burnChecksEnabledAtEpochV1');",
            )
            .unwrap();
        let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude-code".into(),
            session_id: "failed".into(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence(&TurnFacts::default());
        connection
            .execute(
                "INSERT INTO session_evidence VALUES
                 ('native', 'claude-code', 'failed', ?1, 4, 'ready', 3, 'fp', ?2, ?3, ?4)",
                params![
                    serde_json::to_string(&evidence).unwrap(),
                    PARSER_REVISION,
                    ANALYZER_REVISION,
                    EVIDENCE_SCHEMA_REVISION
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO burn_check_assessment VALUES
                 ('native', 'claude-code', 'failed', 'ignored_instructions',
                  'failed', 'authentication_rejected', 1, 3, 'fp', 4, ?1,
                  'input', NULL, NULL)",
                [antiburn_local::analysis::ignored_instructions::evaluator_revision()],
            )
            .unwrap();
        connection
    }

    fn session_status(
        connection: &rusqlite::Connection,
    ) -> crate::dto::IgnoredInstructionSessionStatus {
        ignored_instruction_session_statuses_in(
            connection,
            &[crate::store::SessionKey::new(
                "native",
                "claude-code",
                "failed",
            )],
        )
        .unwrap()[0]
    }

    #[test]
    fn sampled_no_finding_is_ordinary_clean_after_the_bounded_pass() {
        let result = result();
        assert!(ignored_result_has_scoped_no_issues(&result, &[]));
        let restored: AssessmentResult =
            serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
        assert!(ignored_result_has_scoped_no_issues(&restored, &[]));
    }

    #[test]
    fn sampling_does_not_hide_evidence_or_provider_failures() {
        let mut result = result();
        result
            .coverage
            .limitations
            .push("source_evidence_is_partial".into());
        assert!(!ignored_result_has_scoped_no_issues(&result, &[]));
        result.coverage.limitations.pop();
        result.unassessed_comparisons.push("comparison".into());
        assert!(!ignored_result_has_scoped_no_issues(&result, &[]));
        result.unassessed_comparisons.clear();
        result.coverage.selector_revision = 0;
        assert!(!ignored_result_has_scoped_no_issues(&result, &[]));
    }

    #[test]
    fn current_failed_assessments_show_bounded_reasons_and_continuations_stay_checking() {
        let connection = session_status_connection();
        let failed = session_status(&connection);
        assert_eq!(
            failed.status,
            crate::dto::SessionHygieneStatus::CouldntCheck
        );
        assert_eq!(failed.reason, Some("The provider rejected the API key."));

        connection
            .execute(
                "UPDATE burn_check_assessment SET last_error_category = 'continuing',
                    input_revision = 'input', result_revision = 'input', result_json = ?1",
                [serde_json::to_string(&result()).unwrap()],
            )
            .unwrap();
        let continuing = session_status(&connection);
        assert_eq!(
            continuing.status,
            crate::dto::SessionHygieneStatus::Checking
        );
        assert_eq!(
            continuing.reason,
            Some("An instruction assessment is continuing.")
        );

        connection
            .execute(
                "UPDATE burn_check_assessment SET last_error_category = 'provider_unavailable'",
                [],
            )
            .unwrap();
        let provider_failed = session_status(&connection);
        assert_eq!(
            provider_failed.status,
            crate::dto::SessionHygieneStatus::CouldntCheck
        );
        assert_eq!(
            provider_failed.reason,
            Some("The provider was unavailable for the assessment.")
        );

        connection
            .execute("UPDATE burn_check_assessment SET source_generation = 2", [])
            .unwrap();
        let stale = session_status(&connection);
        assert_eq!(stale.status, crate::dto::SessionHygieneStatus::Checking);
        assert_eq!(
            stale.reason,
            Some("Waiting for a current instruction assessment.")
        );

        connection
            .execute(
                "UPDATE burn_check_assessment SET source_generation = 3,
                    last_error_category = 'unrecognized_failure'",
                [],
            )
            .unwrap();
        let unknown_failure = session_status(&connection);
        assert_eq!(
            unknown_failure.status,
            crate::dto::SessionHygieneStatus::CouldntCheck
        );
        assert_eq!(
            unknown_failure.reason,
            Some("The instruction assessment failed.")
        );

        for stale_field in [
            "source_fingerprint = 'old-fingerprint'",
            "published_fence = 99",
            "evaluator_revision = 'old-revision'",
        ] {
            connection
                .execute(
                    &format!("UPDATE burn_check_assessment SET {stale_field}"),
                    [],
                )
                .unwrap();
            assert_eq!(
                session_status(&connection).status,
                crate::dto::SessionHygieneStatus::Checking,
                "stale assessment field: {stale_field}"
            );
            connection
                .execute(
                    "UPDATE burn_check_assessment SET source_fingerprint = 'fp',
                        published_fence = 4, evaluator_revision = ?1",
                    [antiburn_local::analysis::ignored_instructions::evaluator_revision()],
                )
                .unwrap();
        }

        connection
            .execute("UPDATE burn_check_assessment SET status = 'queued'", [])
            .unwrap();
        assert_eq!(
            session_status(&connection).status,
            crate::dto::SessionHygieneStatus::Checking
        );
        connection
            .execute_batch(
                "UPDATE burn_check_assessment SET status = 'idle';
                 UPDATE session_evidence SET status = 'failed', evidence_json = NULL;",
            )
            .unwrap();
        assert_eq!(
            session_status(&connection).status,
            crate::dto::SessionHygieneStatus::CouldntCheck
        );
    }

    #[test]
    fn category_sampling_requires_a_current_completed_assessment_in_the_window() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (
                    environment_key TEXT, agent TEXT, session_id TEXT,
                    incarnation INTEGER, source_generation INTEGER,
                    source_fingerprint TEXT, started_at_epoch INTEGER,
                    updated_at_epoch INTEGER);
                 CREATE TABLE session_evidence (
                    environment_key TEXT, agent TEXT, session_id TEXT,
                    evidence_json TEXT, published_fence INTEGER, status TEXT,
                    analyzed_generation INTEGER, parser_revision INTEGER,
                    analyzer_revision INTEGER, evidence_schema_revision INTEGER);
                 CREATE TABLE burn_check_assessment (
                    environment_key TEXT, agent TEXT, session_id TEXT, check_id TEXT,
                    incarnation INTEGER, source_generation INTEGER, source_fingerprint TEXT,
                    published_fence INTEGER, status TEXT, input_revision TEXT,
                    result_revision TEXT, result_json TEXT);
                 INSERT INTO session VALUES
                    ('native', 'claude-code', 'sampled', 1, 1, 'fingerprint', 120, NULL);",
            )
            .unwrap();
        let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude-code".into(),
            session_id: "sampled".into(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence(&TurnFacts::default());
        connection
            .execute(
                "INSERT INTO session_evidence VALUES
                 ('native', 'claude-code', 'sampled', ?1, 7, 'ready', 1, ?2, ?3, ?4)",
                params![
                    serde_json::to_string(&evidence).unwrap(),
                    PARSER_REVISION,
                    ANALYZER_REVISION,
                    EVIDENCE_SCHEMA_REVISION
                ],
            )
            .unwrap();
        let sampled = result();
        connection
            .execute(
                "INSERT INTO burn_check_assessment VALUES
                 ('native', 'claude-code', 'sampled', 'ignored_instructions',
                  1, 1, 'fingerprint', 7, 'completed', 'synthetic', 'synthetic', ?1)",
                [serde_json::to_string(&sampled).unwrap()],
            )
            .unwrap();
        let request = ReportRequest {
            environment_key: "native".into(),
            window: ReportWindow {
                start_epoch: 100,
                end_epoch: 200,
            },
            computed_at_epoch: 200,
        };
        let has_sampled =
            || has_published_sampled_instruction_assessment_in(&connection, &request).unwrap();
        assert!(has_sampled());

        for status in ["queued", "running", "failed", "superseded"] {
            connection
                .execute("UPDATE burn_check_assessment SET status = ?1", [status])
                .unwrap();
            assert!(!has_sampled(), "{status} cannot publish sampling");
        }
        connection
            .execute("UPDATE burn_check_assessment SET status = 'completed'", [])
            .unwrap();
        for column in ["incarnation", "source_generation", "published_fence"] {
            connection
                .execute(
                    &format!("UPDATE burn_check_assessment SET {column} = 99"),
                    [],
                )
                .unwrap();
            assert!(!has_sampled(), "stale {column} cannot publish sampling");
            connection
                .execute(
                    &format!("UPDATE burn_check_assessment SET {column} = ?1"),
                    [if column == "published_fence" { 7 } else { 1 }],
                )
                .unwrap();
        }
        connection
            .execute(
                "UPDATE burn_check_assessment SET input_revision = 'new'",
                [],
            )
            .unwrap();
        assert!(!has_sampled());
        connection
            .execute(
                "UPDATE burn_check_assessment SET input_revision = 'synthetic'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE burn_check_assessment SET result_revision = 'old'",
                [],
            )
            .unwrap();
        assert!(!has_sampled());
        connection
            .execute(
                "UPDATE burn_check_assessment SET result_revision = 'synthetic'",
                [],
            )
            .unwrap();
        connection
            .execute("UPDATE session_evidence SET status = 'pending'", [])
            .unwrap();
        assert!(!has_sampled());
        connection
            .execute("UPDATE session_evidence SET status = 'ready'", [])
            .unwrap();
        connection
            .execute("UPDATE session SET started_at_epoch = 200", [])
            .unwrap();
        assert!(!has_sampled());
        connection
            .execute("UPDATE session SET started_at_epoch = 120", [])
            .unwrap();
        let mut not_sampled = sampled;
        not_sampled.coverage.sampled_pass = false;
        connection
            .execute(
                "UPDATE burn_check_assessment SET result_json = ?1",
                [serde_json::to_string(&not_sampled).unwrap()],
            )
            .unwrap();
        assert!(!has_sampled());
    }
}
