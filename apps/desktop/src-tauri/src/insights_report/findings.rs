use super::*;

pub(crate) struct CurrentFindingSession {
    pub(super) evidence: SessionEvidence,
    pub(super) environment_key: String,
    pub(super) agent: String,
    pub(super) session_id: String,
    pub(super) source_generation: i64,
    pub(super) incarnation: u64,
    pub(super) published_fence: i64,
    pub(super) source_fingerprint: Option<String>,
    processed_fingerprint: Option<String>,
    parser_revision: i64,
    analyzer_revision: i64,
    evidence_schema_revision: i64,
    metrics_schema_revision: i64,
    pub(super) started_at_epoch: Option<i64>,
    pub(super) workspace_candidate: Option<PathBuf>,
    pub(super) initial_context: Option<InitialContextBreakdown>,
    effective_model_target_hash: Option<String>,
    effective_model_scope: Option<String>,
    effective_model: Option<String>,
    pub(super) effective_reasoning_target_hash: Option<String>,
    pub(super) effective_reasoning_scope: Option<String>,
    effective_reasoning: Option<String>,
}

/// Marks a reduction that stopped because its caller cancelled it.
///
/// The reduction reads one snapshot and writes nothing, so a cancelled
/// run leaves the durable evidence state untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportCancelled;

impl std::fmt::Display for ReportCancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the insights report reduction was cancelled")
    }
}

impl std::error::Error for ReportCancelled {}

/// Tells whether an error marks a cancelled reduction.
pub fn is_cancelled(error: &anyhow::Error) -> bool {
    error.is::<ReportCancelled>()
}

pub(crate) fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        return Err(anyhow::Error::new(ReportCancelled));
    }
    Ok(())
}

/// Reduces one report without blocking the async runtime.
///
/// The cancel flag is a cooperative probe: `spawn_blocking` tasks cannot
/// be aborted, so the reduction checks the flag between phases and per
/// cohort row, and returns [`ReportCancelled`] when it is set.
#[cfg(test)]
pub async fn reduce_report(
    data_dir: PathBuf,
    request: ReportRequest,
    cancel: Arc<AtomicBool>,
) -> Result<ReducedReport> {
    reduce_report_with_selection(data_dir, request, cancel, DetectorSelection::all()).await
}

pub async fn reduce_report_with_selection(
    data_dir: PathBuf,
    request: ReportRequest,
    cancel: Arc<AtomicBool>,
    enabled_detectors: DetectorSelection,
) -> Result<ReducedReport> {
    let resource_home = antiburn_local::paths::home_dir();
    tokio::task::spawn_blocking(move || {
        reduce_with_selection_on_snapshot(
            &data_dir,
            request,
            &mut || {},
            &cancel,
            &mut || {},
            resource_home.as_deref(),
            enabled_detectors,
        )
    })
    .await
    .context("report reduction task failed")?
}

/// Reduces one report on the caller's blocking thread.
pub fn reduce_report_blocking(data_dir: &Path, request: ReportRequest) -> Result<ReducedReport> {
    let resource_home = antiburn_local::paths::home_dir();
    reduce_with_state_on_snapshot(
        data_dir,
        request,
        &mut || {},
        &AtomicBool::new(false),
        &mut || {},
        resource_home.as_deref(),
    )
}

#[cfg(test)]
pub(crate) fn reduce_report_blocking_with_home(
    data_dir: &Path,
    request: ReportRequest,
    home: &Path,
) -> Result<ReducedReport> {
    reduce_with_state_on_snapshot(
        data_dir,
        request,
        &mut || {},
        &AtomicBool::new(false),
        &mut || {},
        Some(home),
    )
}

/// Lists one detector's current findings from one read transaction.
pub fn list_current_findings(
    data_dir: &Path,
    request: CurrentFindingsRequest,
) -> Result<CurrentFindingsPage> {
    list_current_findings_on_snapshot(data_dir, request, &mut || {}, &mut || {})
}

pub(crate) fn list_current_findings_on_snapshot(
    data_dir: &Path,
    request: CurrentFindingsRequest,
    after_session_read: &mut dyn FnMut(),
    turn_probe: &mut dyn FnMut(),
) -> Result<CurrentFindingsPage> {
    let connection = open_read_only(data_dir, REPORT_BUSY_TIMEOUT)?;
    let transaction = connection.unchecked_transaction()?;
    let catalogs = ReportCatalogs::default();
    let sql = CURRENT_FINDINGS_SQL;
    let mut findings = Vec::with_capacity(CURRENT_FINDING_LIMIT + 1);
    let cancel = AtomicBool::new(false);
    let mut sessions_scanned = 0;
    {
        let mut statement = transaction.prepare(sql)?;
        let mut rows = statement.query(named_params![
            ":environment_key": request.environment_key,
            ":window_start": request.window.start_epoch,
            ":window_end": request.window.end_epoch,
            ":parser_revision": PARSER_REVISION,
            ":analyzer_revision": ANALYZER_REVISION,
            ":evidence_schema_revision": EVIDENCE_SCHEMA_REVISION,
            ":metrics_schema_revision": METRICS_SCHEMA_REVISION,
            ":limit": CURRENT_FINDING_SESSION_SCAN_BUDGET + 1,
        ])?;
        while let Some(row) = rows.next()? {
            let session = current_finding_session(row)?;
            if session.started_at_epoch.is_none() {
                continue;
            }
            sessions_scanned += 1;
            after_session_read();
            let assessment = assess_current_detector(
                &transaction,
                &session,
                request.detector,
                &catalogs,
                &cancel,
                turn_probe,
            )?;
            let FindingAssessment::Findings(session_findings) = &assessment else {
                continue;
            };
            for finding in session_findings {
                if let Some(finding) = current_finding(&session, finding.clone(), catalogs.revision)
                {
                    findings.push(finding);
                }
                if findings.len() > CURRENT_FINDING_LIMIT {
                    break;
                }
            }
            if findings.len() > CURRENT_FINDING_LIMIT {
                break;
            }
        }
    }

    let finding_overflow = findings.len() > CURRENT_FINDING_LIMIT;
    findings.truncate(CURRENT_FINDING_LIMIT);
    Ok(CurrentFindingsPage {
        findings,
        truncated: finding_overflow || sessions_scanned > CURRENT_FINDING_SESSION_SCAN_BUDGET,
    })
}

/// Recomputes one cached finding and accepts only its exact cause and freshness.
pub fn revalidate_current_finding(data_dir: &Path, cached: &CurrentFinding) -> Result<bool> {
    revalidate_current_finding_on_snapshot(data_dir, cached, &mut || {})
}

/// Derives bounded findings from the publication that is winning this transaction.
pub(crate) fn publication_findings_in(
    connection: &rusqlite::Connection,
    key: &crate::store::SessionKey,
    enabled_detectors: &DetectorSelection,
) -> Result<Vec<CurrentFinding>> {
    let catalogs = ReportCatalogs::default();
    let sql = CURRENT_FINDING_BY_KEY_SQL;
    let Some(session) = connection
        .query_row(
            sql,
            named_params![
                ":environment_key": key.environment_key,
                ":agent": key.agent,
                ":session_id": key.session_id,
                ":parser_revision": PARSER_REVISION,
                ":analyzer_revision": ANALYZER_REVISION,
                ":evidence_schema_revision": EVIDENCE_SCHEMA_REVISION,
                ":metrics_schema_revision": METRICS_SCHEMA_REVISION,
            ],
            current_finding_session,
        )
        .optional()?
    else {
        return Ok(Vec::new());
    };
    let cancel = AtomicBool::new(false);
    let mut findings_by_detector = Vec::with_capacity(DetectorId::COUNT);
    for detector in DetectorId::ALL {
        if !enabled_detectors.contains(detector) {
            continue;
        }
        let assessment = assess_current_detector(
            connection,
            &session,
            detector,
            &catalogs,
            &cancel,
            &mut || {},
        )?;
        let findings = match assessment {
            FindingAssessment::Findings(values) => values
                .into_iter()
                .take(100)
                .filter_map(|finding| current_finding(&session, finding, catalogs.revision))
                .collect(),
            _ => Vec::new(),
        };
        findings_by_detector.push(findings);
    }
    Ok(fair_bounded_selection(findings_by_detector, 100))
}

pub(crate) fn fair_bounded_selection<T>(buckets: Vec<Vec<T>>, limit: usize) -> Vec<T> {
    let mut buckets = buckets.into_iter().map(Vec::into_iter).collect::<Vec<_>>();
    let mut selected = Vec::with_capacity(limit);
    while selected.len() < limit {
        let mut added = false;
        for bucket in &mut buckets {
            if let Some(value) = bucket.next() {
                selected.push(value);
                added = true;
                if selected.len() == limit {
                    break;
                }
            }
        }
        if !added {
            break;
        }
    }
    selected
}

pub(crate) fn revalidate_current_finding_on_snapshot(
    data_dir: &Path,
    cached: &CurrentFinding,
    turn_probe: &mut dyn FnMut(),
) -> Result<bool> {
    let connection = open_read_only(data_dir, REPORT_BUSY_TIMEOUT)?;
    let transaction = connection.unchecked_transaction()?;
    let catalogs = ReportCatalogs::default();
    if cached.catalog_revision != catalogs.revision {
        return Ok(false);
    }
    let sql = CURRENT_FINDING_BY_KEY_SQL;
    let session = transaction
        .query_row(
            sql,
            named_params![
                ":environment_key": cached.environment_key,
                ":agent": cached.agent,
                ":session_id": cached.session_id,
                ":parser_revision": PARSER_REVISION,
                ":analyzer_revision": ANALYZER_REVISION,
                ":evidence_schema_revision": EVIDENCE_SCHEMA_REVISION,
                ":metrics_schema_revision": METRICS_SCHEMA_REVISION,
            ],
            current_finding_session,
        )
        .optional()?;
    let Some(session) = session.filter(|session| freshness_matches(cached, session)) else {
        return Ok(false);
    };
    let cancel = AtomicBool::new(false);
    let assessment = assess_current_detector(
        &transaction,
        &session,
        cached.finding.detector,
        &catalogs,
        &cancel,
        turn_probe,
    )?;
    let FindingAssessment::Findings(findings) = &assessment else {
        return Ok(false);
    };
    Ok(findings.iter().any(|finding| {
        finding.detector == cached.finding.detector && finding.cause() == cached.finding.cause()
    }))
}

pub(crate) fn assess_current_detector(
    connection: &rusqlite::Connection,
    session: &CurrentFindingSession,
    detector: DetectorId,
    catalogs: &ReportCatalogs,
    cancel: &AtomicBool,
    turn_probe: &mut dyn FnMut(),
) -> Result<FindingAssessment> {
    if !matches!(
        detector,
        DetectorId::UnusedBuiltInTools | DetectorId::UnusedMcpServers | DetectorId::UnusedSkills
    ) {
        return Ok(antiburn_local::remediation::assess_detector(
            detector,
            &session.evidence,
            catalogs,
        ));
    }
    let mut resource_turn_probe = |_| {};
    let mut probes = TokenBurnProbes {
        turn: turn_probe,
        resource_turn: &mut resource_turn_probe,
    };
    let token_evidence = token_burn_evidence(
        connection,
        TokenBurnSessionKey {
            environment_key: &session.environment_key,
            agent: &session.agent,
            session_id: &session.session_id,
            published_fence: session.published_fence,
            cwd: session
                .workspace_candidate
                .as_deref()
                .and_then(Path::to_str),
        },
        session.initial_context.as_ref(),
        &session.evidence,
        &TokenBurnReportContext {
            depth_cap: u128::from(catalogs.depth_cap_tokens),
            catalogs,
        },
        cancel,
        &mut probes,
    )?;
    Ok(
        antiburn_local::remediation::assess_detector_with_source_evidence(
            detector,
            &session.evidence,
            catalogs,
            Some(&token_evidence),
        ),
    )
}

pub(crate) fn current_finding_session(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CurrentFindingSession> {
    let evidence_json: String = row.get(0)?;
    let initial_context_json: Option<String> = row.get(14)?;
    let evidence = serde_json::from_str(&evidence_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let initial_context = initial_context_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                14,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    Ok(CurrentFindingSession {
        evidence,
        environment_key: row.get(1)?,
        agent: row.get(2)?,
        session_id: row.get(3)?,
        source_generation: row.get(4)?,
        published_fence: row.get(5)?,
        source_fingerprint: row.get(6)?,
        processed_fingerprint: row.get(7)?,
        parser_revision: row.get(8)?,
        analyzer_revision: row.get(9)?,
        evidence_schema_revision: row.get(10)?,
        metrics_schema_revision: row.get(11)?,
        started_at_epoch: row.get(12)?,
        workspace_candidate: row.get::<_, Option<String>>(13)?.map(PathBuf::from),
        initial_context,
        effective_model_target_hash: row.get(15)?,
        effective_model_scope: row.get(16)?,
        effective_model: row.get(17)?,
        effective_reasoning_target_hash: row.get(18)?,
        effective_reasoning_scope: row.get(19)?,
        effective_reasoning: row.get(20)?,
        incarnation: row.get(21)?,
    })
}

pub(crate) fn current_finding(
    session: &CurrentFindingSession,
    finding: Finding,
    catalog_revision: i64,
) -> Option<CurrentFinding> {
    let started_at_epoch = session.started_at_epoch?;
    let observed_at_ms = finding_observation_ms(&session.evidence, &finding).unwrap_or_else(|| {
        match &session.evidence.time_range {
            antiburn_local::analysis::EvidenceValue::Complete(range) => range.last_ts_ms,
            _ => started_at_epoch.saturating_mul(1_000),
        }
    });
    Some(CurrentFinding {
        finding,
        environment_key: session.environment_key.clone(),
        agent: session.agent.clone(),
        session_id: session.session_id.clone(),
        source_generation: session.source_generation,
        incarnation: session.incarnation,
        published_fence: session.published_fence,
        source_fingerprint: session.source_fingerprint.clone(),
        processed_fingerprint: session.processed_fingerprint.clone(),
        parser_revision: session.parser_revision,
        analyzer_revision: session.analyzer_revision,
        evidence_schema_revision: session.evidence_schema_revision,
        metrics_schema_revision: session.metrics_schema_revision,
        catalog_revision,
        started_at_epoch,
        observed_at_ms,
        workspace_candidate: session.workspace_candidate.clone(),
        effective_model_target_hash: session.effective_model_target_hash.clone(),
        effective_model_scope: session.effective_model_scope.clone(),
        effective_model: session.effective_model.clone(),
        effective_reasoning_target_hash: session.effective_reasoning_target_hash.clone(),
        effective_reasoning_scope: session.effective_reasoning_scope.clone(),
        effective_reasoning: session.effective_reasoning.clone(),
    })
}

pub(crate) fn finding_observation_ms(evidence: &SessionEvidence, finding: &Finding) -> Option<i64> {
    use antiburn_local::analysis::EvidenceValue;
    use antiburn_local::remediation::FindingCause;

    let models = match &evidence.models {
        EvidenceValue::Complete(models)
        | EvidenceValue::Partial {
            observed: models, ..
        } => Some(models),
        EvidenceValue::Unsupported => None,
    };
    match finding.cause() {
        FindingCause::SessionsOverDepth { requests, .. } => requests
            .iter()
            .filter_map(|request| request.timestamp_ms)
            .max(),
        FindingCause::ModelOverthinking {
            provider,
            api,
            model,
            reasoning,
            ..
        } => models?
            .control_observations
            .iter()
            .filter(|observation| {
                observation.provider == *provider
                    && observation.api == *api
                    && observation.model == *model
                    && observation.effort.as_deref() == Some(reasoning)
            })
            .map(|observation| observation.last_ts_ms)
            .max(),
        FindingCause::OldModelUsage { model, .. } | FindingCause::CacheChurn { model, .. } => {
            models?.by_model.get(model).map(|tokens| tokens.last_ts_ms)
        }
        FindingCause::OveruseOfFastMode {
            provider,
            api,
            model,
            ..
        } => models?
            .control_observations
            .iter()
            .filter(|observation| {
                observation.provider == *provider
                    && observation.api == *api
                    && observation.model == *model
                    && observation.speed.as_deref()
                        == Some(antiburn_local::analysis::FAST_SPEED_KEY)
                    && observation.turns.delegated > 0
            })
            .map(|observation| observation.last_ts_ms)
            .max(),
        FindingCause::OverpoweredSubagents { .. }
        | FindingCause::UnusedMcpServer { .. }
        | FindingCause::UnusedBuiltInTool { .. }
        | FindingCause::UnusedSkill { .. } => None,
    }
    .filter(|timestamp| *timestamp > 0)
}

pub(crate) fn freshness_matches(cached: &CurrentFinding, session: &CurrentFindingSession) -> bool {
    cached.environment_key == session.environment_key
        && cached.agent == session.agent
        && cached.session_id == session.session_id
        && cached.source_generation == session.source_generation
        && cached.incarnation == session.incarnation
        && cached.published_fence == session.published_fence
        && cached.source_fingerprint == session.source_fingerprint
        && cached.processed_fingerprint == session.processed_fingerprint
        && cached.parser_revision == session.parser_revision
        && cached.analyzer_revision == session.analyzer_revision
        && cached.evidence_schema_revision == session.evidence_schema_revision
        && cached.metrics_schema_revision == session.metrics_schema_revision
        && Some(cached.started_at_epoch) == session.started_at_epoch
        && cached.workspace_candidate == session.workspace_candidate
}

#[cfg(test)]
mod tests {
    use super::super::verification::named_resource_assessments_for_session;
    use std::collections::BTreeMap;

    use antiburn_local::analysis::{
        ContextSourceEvidence, EvidenceSource, EvidenceValue, LoadedSource,
        SessionEvidenceAccumulator, SessionTimeRange, SourceCapabilities, SourceKind, SourceOrigin,
        ToolDefinition, TurnFacts,
    };
    use antiburn_local::remediation::{
        NamedResourceEvidence, NamedResourceVerificationTarget, VerificationOutcome,
        VerificationStage, VerificationUnknownReason, verify_named_resource_watch,
    };

    use super::*;

    fn session_with_resource(
        detector: DetectorId,
        resource: &str,
        evidence_state: Option<NamedResourceEvidence>,
    ) -> CurrentFindingSession {
        let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude-code".into(),
            session_id: "later".into(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence(&TurnFacts::default());
        evidence.time_range = EvidenceValue::Complete(SessionTimeRange {
            first_ts_ms: 101,
            last_ts_ms: 101,
            timestamped_turns: 1,
        });
        if let EvidenceValue::Complete(sources) = &mut evidence.context_sources {
            sources.mcp_coverage = EvidenceValue::Complete(());
            sources.skill_coverage = EvidenceValue::Complete(());
            sources.tool_definitions = EvidenceValue::Complete(BTreeMap::from([(
                resource.to_owned(),
                ToolDefinition {
                    tokens: 100,
                    invoked: false,
                    deferred: false,
                },
            )]));
            let loaded = LoadedSource {
                description: None,
                configured: true,
                available: true,
                injected: true,
                invoked: false,
                token_count: None,
                origin: EvidenceValue::Complete(SourceOrigin::User),
            };
            sources
                .mcp_servers
                .insert(resource.to_owned(), loaded.clone());
            sources.skills.insert(resource.to_owned(), loaded);
        }
        if let Some(state) = evidence_state {
            match state {
                NamedResourceEvidence::Partial => {
                    evidence.context_sources = EvidenceValue::Partial {
                        observed: match evidence.context_sources {
                            EvidenceValue::Complete(value) => value,
                            _ => unreachable!(),
                        },
                        reason: antiburn_local::analysis::CoverageReason::IncompleteTail,
                    };
                }
                NamedResourceEvidence::Capped => {
                    if detector == DetectorId::UnusedBuiltInTools {
                        let EvidenceValue::Complete(ContextSourceEvidence {
                            skills,
                            mcp_servers,
                            skill_coverage,
                            mcp_coverage,
                            ..
                        }) = evidence.context_sources
                        else {
                            unreachable!()
                        };
                        evidence.context_sources = EvidenceValue::Complete(ContextSourceEvidence {
                            skills,
                            mcp_servers,
                            skill_coverage,
                            mcp_coverage,
                            tool_definitions: EvidenceValue::Partial {
                                observed: BTreeMap::from([(
                                    resource.to_owned(),
                                    ToolDefinition {
                                        tokens: 100,
                                        invoked: false,
                                        deferred: false,
                                    },
                                )]),
                                reason: antiburn_local::analysis::CoverageReason::CapExceeded,
                            },
                        });
                    } else {
                        evidence.context_sources = match evidence.context_sources {
                            EvidenceValue::Complete(value) => EvidenceValue::Partial {
                                observed: value,
                                reason: antiburn_local::analysis::CoverageReason::CapExceeded,
                            },
                            _ => unreachable!(),
                        };
                    }
                }
                NamedResourceEvidence::Ambiguous => {
                    evidence.context_sources = EvidenceValue::Partial {
                        observed: match evidence.context_sources {
                            EvidenceValue::Complete(value) => value,
                            _ => unreachable!(),
                        },
                        reason: antiburn_local::analysis::CoverageReason::AttributionIncomplete,
                    };
                }
                NamedResourceEvidence::HistoricalObservedSubset { .. } => {}
                NamedResourceEvidence::Complete { .. } => {}
            }
        }
        if !matches!(detector, DetectorId::UnusedBuiltInTools) {
            evidence.context_sources = match evidence.context_sources {
                EvidenceValue::Complete(mut sources) => {
                    sources.tool_definitions = EvidenceValue::Unsupported;
                    EvidenceValue::Complete(sources)
                }
                partial => partial,
            };
        }
        CurrentFindingSession {
            evidence,
            environment_key: "native".into(),
            agent: "claude-code".into(),
            session_id: "later".into(),
            source_generation: 1,
            incarnation: 1,
            published_fence: 1,
            source_fingerprint: None,
            processed_fingerprint: None,
            parser_revision: PARSER_REVISION,
            analyzer_revision: ANALYZER_REVISION,
            evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
            metrics_schema_revision: METRICS_SCHEMA_REVISION,
            started_at_epoch: Some(1),
            workspace_candidate: None,
            initial_context: None,
            effective_model_target_hash: None,
            effective_model_scope: None,
            effective_model: None,
            effective_reasoning_target_hash: None,
            effective_reasoning_scope: None,
            effective_reasoning: None,
        }
    }

    fn target(detector: DetectorId, resource: &str) -> NamedResourceVerificationTarget {
        NamedResourceVerificationTarget {
            detector,
            source_format: antiburn_local::analysis::SourceFormat::ClaudeJsonl,
            agent: "claude-code".into(),
            project_scope: "global".into(),
            resource: resource.into(),
        }
    }

    #[test]
    fn named_desktop_session_evidence_cannot_prove_current_resource_absence() {
        for detector in [
            DetectorId::UnusedMcpServers,
            DetectorId::UnusedBuiltInTools,
            DetectorId::UnusedSkills,
        ] {
            let session = session_with_resource(detector, "other-resource", None);
            let assessments = named_resource_assessments_for_session(detector, &session, 101);
            let result = verify_named_resource_watch(
                &target(detector, "target-resource"),
                VerificationStage::Watching,
                100,
                &assessments,
            );
            assert_eq!(
                result.outcome,
                VerificationOutcome::Unknown(
                    VerificationUnknownReason::MissingPostBoundaryEvidence
                )
            );
        }
    }

    #[test]
    fn named_desktop_session_evidence_cannot_verify_presence_or_recurrence() {
        for detector in [
            DetectorId::UnusedMcpServers,
            DetectorId::UnusedBuiltInTools,
            DetectorId::UnusedSkills,
        ] {
            let session = session_with_resource(detector, "target-resource", None);
            let assessments = named_resource_assessments_for_session(detector, &session, 101);
            let watching = verify_named_resource_watch(
                &target(detector, "target-resource"),
                VerificationStage::Watching,
                100,
                &assessments,
            );
            assert!(matches!(watching.outcome, VerificationOutcome::Unknown(_)));
            let fixed = verify_named_resource_watch(
                &target(detector, "target-resource"),
                VerificationStage::Fixed,
                100,
                &assessments,
            );
            assert!(matches!(fixed.outcome, VerificationOutcome::Unknown(_)));
            let different = verify_named_resource_watch(
                &target(detector, "different-resource"),
                VerificationStage::Fixed,
                100,
                &assessments,
            );
            assert!(matches!(different.outcome, VerificationOutcome::Unknown(_)));
        }
    }

    #[test]
    fn named_desktop_evidence_rejects_wrong_identity_and_incomplete_coverage() {
        let session = session_with_resource(DetectorId::UnusedMcpServers, "target-resource", None);
        let assessment =
            named_resource_assessments_for_session(DetectorId::UnusedMcpServers, &session, 101);
        for (source_format, agent, project_scope) in [
            (
                antiburn_local::analysis::SourceFormat::CodexRolloutJsonl,
                "claude-code",
                "global",
            ),
            (
                antiburn_local::analysis::SourceFormat::ClaudeJsonl,
                "codex",
                "global",
            ),
            (
                antiburn_local::analysis::SourceFormat::ClaudeJsonl,
                "claude-code",
                "other-project",
            ),
        ] {
            let mut wrong = assessment[0].clone();
            wrong.source_format = source_format;
            wrong.agent = agent.into();
            wrong.project_scope = project_scope.into();
            let result = verify_named_resource_watch(
                &target(DetectorId::UnusedMcpServers, "target-resource"),
                VerificationStage::Watching,
                100,
                &[wrong],
            );
            assert_eq!(
                result.outcome,
                VerificationOutcome::Unknown(
                    VerificationUnknownReason::MissingPostBoundaryEvidence
                )
            );
        }
        for state in [
            NamedResourceEvidence::Partial,
            NamedResourceEvidence::Capped,
            NamedResourceEvidence::Ambiguous,
        ] {
            let session =
                session_with_resource(DetectorId::UnusedMcpServers, "target-resource", Some(state));
            let assessments =
                named_resource_assessments_for_session(DetectorId::UnusedMcpServers, &session, 101);
            let result = verify_named_resource_watch(
                &target(DetectorId::UnusedMcpServers, "target-resource"),
                VerificationStage::Watching,
                100,
                &assessments,
            );
            assert!(matches!(result.outcome, VerificationOutcome::Unknown(_)));
        }
    }
}
