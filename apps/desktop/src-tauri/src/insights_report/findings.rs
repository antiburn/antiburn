use super::*;

/// One fresh session assessment used by the generic remediation verifier.
pub(crate) struct CurrentDetectorAssessment {
    pub assessment: FindingAssessment,
    pub clean_for_verification: bool,
    pub observed_at_ms: i64,
    pub finding_observed_at_ms: Vec<Option<i64>>,
    pub started_at_ms: i64,
    pub workspace_candidate: Option<PathBuf>,
    pub source_format: antiburn_local::analysis::SourceFormat,
    pub session_id: String,
    pub control_observations: Vec<antiburn_local::analysis::ModelControlObservation>,
    pub effective_reasoning_target_hash: Option<String>,
    pub effective_reasoning_scope: Option<String>,
}

pub(crate) struct RemediationAssessments {
    pub assessments: Vec<CurrentDetectorAssessment>,
    pub named_resource_assessments: Vec<antiburn_local::remediation::NamedResourceAssessment>,
    pub truncated: bool,
}

/// Reports whether current accepted evidence exists after an action boundary.
pub(crate) fn has_current_evidence_after(
    data_dir: &Path,
    environment_key: &str,
    agent: &str,
    boundary_ms: i64,
) -> Result<bool> {
    let connection = open_read_only(data_dir, REPORT_BUSY_TIMEOUT)?;
    let sql = format!(
        "SELECT EXISTS (
            SELECT 1
              FROM session s
              JOIN session_evidence e
                ON e.environment_key = s.environment_key
               AND e.agent = s.agent
               AND e.session_id = s.session_id
             WHERE s.environment_key = ?1
               AND s.agent = ?2
               AND s.started_at_epoch > ?3
               AND {CURRENT_EVIDENCE_PREDICATE}
             LIMIT 1
        )"
    );
    connection
        .query_row(
            &sql,
            params![
                environment_key,
                agent,
                boundary_ms.div_euclid(1_000),
                PARSER_REVISION,
                ANALYZER_REVISION,
                EVIDENCE_SCHEMA_REVISION,
            ],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

/// Reads a bounded set of fresh post-boundary detector assessments.
pub(crate) fn remediation_assessments(
    data_dir: &Path,
    environment_key: &str,
    agent: &str,
    detector: DetectorId,
    boundary_ms: i64,
) -> Result<RemediationAssessments> {
    let request = CurrentFindingsRequest {
        environment_key: environment_key.to_owned(),
        window: ReportWindow {
            start_epoch: i64::MIN,
            end_epoch: i64::MAX,
        },
        detector,
    };
    let connection = open_read_only(data_dir, REPORT_BUSY_TIMEOUT)?;
    let transaction = connection.unchecked_transaction()?;
    let catalogs = ReportCatalogs::default();
    let sql = CURRENT_FINDINGS_SQL
        .replace("{current}", CURRENT_EVIDENCE_PREDICATE)
        .replace(
            "  ORDER BY",
            "   AND s.agent = ?8\n   AND s.started_at_epoch > ?9\n  ORDER BY",
        )
        .replace("LIMIT ?8", "LIMIT ?10");
    let mut statement = transaction.prepare(&sql)?;
    let mut rows = statement.query(params![
        request.environment_key,
        request.window.start_epoch,
        request.window.end_epoch,
        PARSER_REVISION,
        ANALYZER_REVISION,
        EVIDENCE_SCHEMA_REVISION,
        METRICS_SCHEMA_REVISION,
        agent,
        boundary_ms.div_euclid(1_000),
        CURRENT_FINDING_SESSION_SCAN_BUDGET + 1,
    ])?;
    let cancel = AtomicBool::new(false);
    let mut result = Vec::new();
    let mut named_resource_assessments = Vec::new();
    let mut sessions_scanned = 0;
    let mut truncated = false;
    while let Some(row) = rows.next()? {
        let session = current_finding_session(row)?;
        let Some(started_at_epoch) = session.started_at_epoch else {
            continue;
        };
        sessions_scanned += 1;
        if sessions_scanned > CURRENT_FINDING_SESSION_SCAN_BUDGET {
            truncated = true;
            break;
        }
        let observed_at_ms = match &session.evidence.time_range {
            antiburn_local::analysis::EvidenceValue::Complete(range) => range.last_ts_ms,
            antiburn_local::analysis::EvidenceValue::Partial {
                observed: range, ..
            } => {
                if matches!(
                    detector,
                    DetectorId::UnusedMcpServers
                        | DetectorId::UnusedBuiltInTools
                        | DetectorId::UnusedSkills
                ) {
                    truncated = true;
                }
                range.last_ts_ms
            }
            antiburn_local::analysis::EvidenceValue::Unsupported => {
                if matches!(
                    detector,
                    DetectorId::UnusedMcpServers
                        | DetectorId::UnusedBuiltInTools
                        | DetectorId::UnusedSkills
                ) {
                    truncated = true;
                }
                continue;
            }
        };
        if observed_at_ms <= boundary_ms {
            continue;
        }
        let assessment = assess_current_detector(
            &transaction,
            &session,
            detector,
            &catalogs,
            &cancel,
            &mut || {},
        )?;
        let clean_for_verification = assessment == FindingAssessment::Clean
            || scoped_resource_clean_for_verification(
                &transaction,
                &session,
                detector,
                &catalogs,
                &cancel,
            )?;
        let finding_observed_at_ms = match &assessment {
            FindingAssessment::Findings(findings) => findings
                .iter()
                .map(|finding| finding_observation_ms(&session.evidence, finding))
                .collect(),
            _ => Vec::new(),
        };
        named_resource_assessments.extend(named_resource_assessments_for_session(
            detector,
            &session,
            observed_at_ms,
        ));
        result.push(CurrentDetectorAssessment {
            assessment,
            clean_for_verification,
            observed_at_ms,
            finding_observed_at_ms,
            started_at_ms: started_at_epoch.saturating_mul(1_000),
            workspace_candidate: session.workspace_candidate,
            source_format: session.evidence.capabilities.source_format,
            session_id: session.session_id,
            control_observations: match session.evidence.models {
                antiburn_local::analysis::EvidenceValue::Complete(models)
                | antiburn_local::analysis::EvidenceValue::Partial {
                    observed: models, ..
                } => models.control_observations,
                antiburn_local::analysis::EvidenceValue::Unsupported => Vec::new(),
            },
            effective_reasoning_target_hash: session.effective_reasoning_target_hash,
            effective_reasoning_scope: session.effective_reasoning_scope,
        });
    }
    Ok(RemediationAssessments {
        assessments: result,
        named_resource_assessments,
        truncated,
    })
}

fn named_resource_assessments_for_session(
    detector: DetectorId,
    session: &CurrentFindingSession,
    observed_at_ms: i64,
) -> Vec<antiburn_local::remediation::NamedResourceAssessment> {
    use antiburn_local::analysis::{CoverageReason, SourceOrigin, ToolClass};
    use antiburn_local::remediation::{NamedResourceEvidence, NamedResourceObservation};

    if !matches!(
        detector,
        DetectorId::UnusedMcpServers | DetectorId::UnusedBuiltInTools | DetectorId::UnusedSkills
    ) {
        return Vec::new();
    }

    let scopes = [
        Some("global".to_owned()),
        session
            .workspace_candidate
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
    ];
    let mut statuses = vec![None, None];
    let mut resources = vec![Vec::new(), Vec::new()];
    let mut merge_status = |status: antiburn_local::remediation::NamedResourceEvidence| {
        for current in &mut statuses {
            let replace = match current {
                None => true,
                Some(NamedResourceEvidence::Partial) => {
                    !matches!(status, NamedResourceEvidence::Partial)
                }
                Some(NamedResourceEvidence::Capped) => false,
                Some(NamedResourceEvidence::Ambiguous) => false,
                Some(NamedResourceEvidence::HistoricalObservedSubset { .. }) => true,
                Some(NamedResourceEvidence::Complete { .. }) => true,
            };
            if replace {
                *current = Some(status.clone());
            }
        }
    };
    let status_for_reason = |reason: CoverageReason| match reason {
        CoverageReason::CapExceeded => NamedResourceEvidence::Capped,
        CoverageReason::AttributionIncomplete => NamedResourceEvidence::Ambiguous,
        _ => NamedResourceEvidence::Partial,
    };
    let context_sources = match &session.evidence.context_sources {
        antiburn_local::analysis::EvidenceValue::Unsupported => {
            merge_status(NamedResourceEvidence::Partial);
            None
        }
        antiburn_local::analysis::EvidenceValue::Partial { observed, reason } => {
            merge_status(status_for_reason(*reason));
            Some(observed)
        }
        antiburn_local::analysis::EvidenceValue::Complete(observed) => Some(observed),
    };

    match detector {
        DetectorId::UnusedBuiltInTools => {
            if let Some(status) = named_resource_status(&session.evidence.tools) {
                merge_status(status);
            }
            let definitions = context_sources.and_then(|sources| match &sources.tool_definitions {
                antiburn_local::analysis::EvidenceValue::Unsupported => {
                    merge_status(NamedResourceEvidence::Partial);
                    None
                }
                antiburn_local::analysis::EvidenceValue::Partial { observed, reason } => {
                    merge_status(status_for_reason(*reason));
                    Some(observed)
                }
                antiburn_local::analysis::EvidenceValue::Complete(observed) => Some(observed),
            });
            if let Some(definitions) = definitions {
                for (name, definition) in definitions {
                    resources[0].push(NamedResourceObservation {
                        resource: name.clone(),
                        used: definition.invoked,
                    });
                }
            }
        }
        DetectorId::UnusedMcpServers | DetectorId::UnusedSkills => {
            if let Some(status) = named_resource_status(&session.evidence.tools) {
                merge_status(status);
            }
            let Some(sources) = context_sources else {
                return named_resource_assessments_from_parts(
                    session,
                    observed_at_ms,
                    scopes,
                    statuses,
                    resources,
                );
            };
            let (kind, coverage) = if detector == DetectorId::UnusedMcpServers {
                ("mcp", &sources.mcp_coverage)
            } else {
                ("skill", &sources.skill_coverage)
            };
            if let Some(status) = named_resource_status(coverage) {
                merge_status(status);
            }
            let values = if kind == "mcp" {
                &sources.mcp_servers
            } else {
                &sources.skills
            };
            for (name, source) in values {
                let scope_index = match &source.origin {
                    antiburn_local::analysis::EvidenceValue::Complete(SourceOrigin::Bundled)
                    | antiburn_local::analysis::EvidenceValue::Complete(SourceOrigin::User) => 0,
                    antiburn_local::analysis::EvidenceValue::Complete(SourceOrigin::Project) => 1,
                    antiburn_local::analysis::EvidenceValue::Partial { reason, .. } => {
                        merge_status(status_for_reason(*reason));
                        continue;
                    }
                    antiburn_local::analysis::EvidenceValue::Unsupported
                    | antiburn_local::analysis::EvidenceValue::Complete(
                        SourceOrigin::Plugin | SourceOrigin::Unknown,
                    ) => {
                        merge_status(NamedResourceEvidence::Ambiguous);
                        continue;
                    }
                };
                if scopes[scope_index].is_none() {
                    continue;
                }
                resources[scope_index].push(NamedResourceObservation {
                    resource: name.clone(),
                    used: source.invoked,
                });
            }
            if let antiburn_local::analysis::EvidenceValue::Complete(tools) =
                &session.evidence.tools
                && tools
                    .by_name
                    .values()
                    .any(|tool| tool.calls > 0 && matches!(tool.class, ToolClass::Unclassified))
            {
                merge_status(NamedResourceEvidence::Ambiguous);
            }
        }
        _ => {}
    }

    named_resource_assessments_from_parts(session, observed_at_ms, scopes, statuses, resources)
}

fn named_resource_assessments_from_parts(
    session: &CurrentFindingSession,
    observed_at_ms: i64,
    scopes: [Option<String>; 2],
    statuses: Vec<Option<antiburn_local::remediation::NamedResourceEvidence>>,
    resources: Vec<Vec<antiburn_local::remediation::NamedResourceObservation>>,
) -> Vec<antiburn_local::remediation::NamedResourceAssessment> {
    use antiburn_local::analysis::SourceFormat;
    use antiburn_local::remediation::{NamedResourceAssessment, NamedResourceEvidence};

    scopes
        .into_iter()
        .enumerate()
        .filter_map(|(index, project_scope)| {
            project_scope.map(|project_scope| NamedResourceAssessment {
                observed_at_ms,
                source_format: session.evidence.capabilities.source_format,
                agent: session.agent.clone(),
                project_scope,
                evidence: statuses[index].clone().unwrap_or_else(|| {
                    NamedResourceEvidence::HistoricalObservedSubset {
                        resources: resources[index].clone(),
                    }
                }),
            })
        })
        .filter(|assessment| {
            assessment.source_format != SourceFormat::Uncharacterized
                || !matches!(
                    assessment.evidence,
                    NamedResourceEvidence::HistoricalObservedSubset { .. }
                )
        })
        .collect()
}

fn named_resource_status<T>(
    value: &antiburn_local::analysis::EvidenceValue<T>,
) -> Option<antiburn_local::remediation::NamedResourceEvidence> {
    use antiburn_local::remediation::NamedResourceEvidence;

    match value {
        antiburn_local::analysis::EvidenceValue::Unsupported => {
            Some(NamedResourceEvidence::Partial)
        }
        antiburn_local::analysis::EvidenceValue::Partial { reason, .. } => Some(match reason {
            antiburn_local::analysis::CoverageReason::CapExceeded => NamedResourceEvidence::Capped,
            antiburn_local::analysis::CoverageReason::AttributionIncomplete => {
                NamedResourceEvidence::Ambiguous
            }
            _ => NamedResourceEvidence::Partial,
        }),
        antiburn_local::analysis::EvidenceValue::Complete(_) => None,
    }
}

fn scoped_resource_clean_for_verification(
    connection: &rusqlite::Connection,
    session: &CurrentFindingSession,
    detector: DetectorId,
    catalogs: &ReportCatalogs,
    cancel: &AtomicBool,
) -> Result<bool> {
    if !matches!(
        detector,
        DetectorId::UnusedBuiltInTools | DetectorId::UnusedMcpServers | DetectorId::UnusedSkills
    ) {
        return Ok(false);
    }
    let mut turn_probe = || {};
    let mut resource_turn_probe = |_| {};
    let mut probes = TokenBurnProbes {
        turn: &mut turn_probe,
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
    Ok(antiburn_local::remediation::scoped_resource_no_finding(
        detector,
        &session.evidence,
        catalogs,
        Some(&token_evidence),
    ))
}

/// A read-only aggregate for one exact old-model watch.
#[derive(Debug)]
pub(crate) struct OldModelRemediationEvidence {
    pub observations: Vec<ModelVerificationObservation>,
    pub replacement_tokens: Option<ModelTokens>,
    pub measured_through_ms: Option<i64>,
    pub recurrence_ms: Option<i64>,
    pub evidence_revision: String,
    pub token_overflow: bool,
}

/// Reads one stable evidence snapshot without loading transcript content.
pub(crate) fn old_model_remediation_evidence(
    data_dir: &Path,
    remediation: &RemediationRecord,
    definition: &WatchDefinition,
    boundary_ms: i64,
    fixed_at_ms: Option<i64>,
) -> Result<OldModelRemediationEvidence> {
    let connection = open_read_only(data_dir, REPORT_BUSY_TIMEOUT)?;
    let transaction = connection.unchecked_transaction()?;
    let mut observations = Vec::new();
    let mut replacement_tokens = ModelTokens::default();
    let mut replacement_evidence = false;
    let mut measured_through_ms: Option<i64> = None;
    let mut recurrence_ms: Option<i64> = None;
    let mut token_overflow = false;
    let mut max_fence = 0_i64;
    let mut turns = transaction.prepare(
        "SELECT t.ts_ms, t.provider, t.api, t.model, t.effort, t.input_tokens,
                t.output_tokens, t.cache_read_tokens, t.cache_write_tokens,
                s.session_id, s.started_at_epoch, s.cwd, e.published_fence,
                e.effective_model_target_hash, e.effective_model_scope,
                e.effective_model, t.cache_write_1h_tokens
           FROM turn t
           JOIN session s USING (environment_key, agent, session_id)
           JOIN session_evidence e USING (environment_key, agent, session_id)
          WHERE t.environment_key = ?1 AND t.agent = ?2 AND t.role = 'assistant'
            AND t.scope = 'main' AND t.ts_ms IS NOT NULL AND t.ts_ms > ?3
            AND e.status = 'ready' AND e.analyzed_generation = s.source_generation
            AND e.published_fence = t.claim_fence AND e.parser_revision = ?4
            AND e.analyzer_revision = ?5 AND e.evidence_schema_revision = ?6
          ORDER BY t.ts_ms, t.rowid",
    )?;
    let mut turn_rows = turns.query(params![
        remediation.environment_key,
        remediation.agent,
        boundary_ms,
        PARSER_REVISION,
        ANALYZER_REVISION,
        EVIDENCE_SCHEMA_REVISION
    ])?;
    while let Some(turn) = turn_rows.next()? {
        let timestamp_ms: i64 = turn.get(0)?;
        let started_at_epoch: Option<i64> = turn.get(10)?;
        let fence: i64 = turn.get(12)?;
        if !started_at_epoch.is_some_and(|started| started.saturating_mul(1_000) > boundary_ms) {
            continue;
        }
        let attributed_target: Option<String> = turn.get(13)?;
        let attributed_scope: Option<String> = turn.get(14)?;
        let attributed_model: Option<String> = turn.get(15)?;
        let provider: Option<String> = turn.get(1)?;
        let api: Option<String> = turn.get(2)?;
        let applies = model_attribution_matches(
            definition,
            &remediation.agent,
            &remediation.scope_kind,
            attributed_target.as_deref(),
            attributed_scope.as_deref(),
            attributed_model.as_deref(),
            (provider.as_deref(), api.as_deref()),
        );
        if !applies {
            continue;
        }
        max_fence = max_fence.max(fence);
        measured_through_ms = Some(timestamp_ms);
        let model: Option<String> = turn.get(3)?;
        let Some(model) = model else { continue };
        let observation = ModelVerificationObservation {
            timestamp_ms,
            scope: remediation.scope_key.clone(),
            provider,
            api,
            model: model.clone(),
        };
        let route_matches =
            observation.provider == definition.provider && observation.api == definition.api;
        if route_matches
            && definition.old_model.as_deref() == Some(model.as_str())
            && fixed_at_ms.is_some_and(|fixed_at_ms| timestamp_ms > fixed_at_ms)
        {
            recurrence_ms.get_or_insert(timestamp_ms);
        } else if route_matches
            && definition.replacement.as_deref() == Some(model.as_str())
            && recurrence_ms.is_none()
        {
            token_overflow |= !add_tokens(&mut replacement_tokens, turn)?;
            replacement_evidence = !token_overflow;
        }
        if route_matches
            && (definition.old_model.as_deref() == Some(model.as_str())
                || definition.replacement.as_deref() == Some(model.as_str()))
        {
            observations.push(observation);
        }
    }
    drop(turn_rows);
    drop(turns);

    Ok(OldModelRemediationEvidence {
        observations,
        replacement_tokens: replacement_evidence.then_some(replacement_tokens),
        measured_through_ms,
        recurrence_ms,
        evidence_revision: format!(
            "evidence-{}-{}-{max_fence}",
            ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION
        ),
        token_overflow,
    })
}

pub(crate) fn model_attribution_matches(
    definition: &WatchDefinition,
    agent: &str,
    scope_kind: &str,
    target_hash: Option<&str>,
    attributed_scope: Option<&str>,
    model: Option<&str>,
    observed_route: (Option<&str>, Option<&str>),
) -> bool {
    let (observed_provider, observed_api) = observed_route;
    let normalized_model = match agent {
        "opencode" | "pi" => {
            let Some((provider, model)) = model.and_then(|value| value.split_once('/')) else {
                return false;
            };
            if Some(provider) != definition.provider.as_deref()
                || Some(provider) != observed_provider
                || definition.api.as_deref() != observed_api
            {
                return false;
            }
            let target = antiburn_local::model_catalog::ModelTarget::new(
                agent,
                provider,
                observed_api.unwrap_or_default(),
                model,
            );
            if !matches!(
                antiburn_local::model_catalog::ReviewedModelCatalog::default().resolve(&target),
                antiburn_local::model_catalog::Support::Supported(_)
            ) {
                return false;
            }
            Some(model)
        }
        _ => model,
    };
    target_hash == definition.physical_target_key.as_deref()
        && attributed_scope == Some(scope_kind)
        && normalized_model.is_some_and(|model| {
            definition.old_model.as_deref() == Some(model)
                || definition.replacement.as_deref() == Some(model)
        })
}

pub(crate) fn add_tokens(
    target: &mut ModelTokens,
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<bool> {
    Ok(checked_add_tokens(
        target,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(16)?,
    ))
}

pub(crate) fn checked_add_tokens(
    target: &mut ModelTokens,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
    cache_creation_1h: u64,
) -> bool {
    let Some(input_tokens) = target.input_tokens.checked_add(input) else {
        return false;
    };
    let Some(output_tokens) = target.output_tokens.checked_add(output) else {
        return false;
    };
    let Some(cache_read_tokens) = target.cache_read_tokens.checked_add(cache_read) else {
        return false;
    };
    let Some(cache_creation_tokens) = target.cache_creation_tokens.checked_add(cache_creation)
    else {
        return false;
    };
    let Some(cache_creation_1h_tokens) = target
        .cache_creation_1h_tokens
        .checked_add(cache_creation_1h)
    else {
        return false;
    };
    *target = ModelTokens {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
        cache_creation_1h_tokens,
    };
    true
}

pub(crate) struct CurrentFindingSession {
    evidence: SessionEvidence,
    environment_key: String,
    agent: String,
    session_id: String,
    source_generation: i64,
    published_fence: i64,
    source_fingerprint: Option<String>,
    processed_fingerprint: Option<String>,
    parser_revision: i64,
    analyzer_revision: i64,
    evidence_schema_revision: i64,
    metrics_schema_revision: i64,
    started_at_epoch: Option<i64>,
    workspace_candidate: Option<PathBuf>,
    initial_context: Option<InitialContextBreakdown>,
    effective_model_target_hash: Option<String>,
    effective_model_scope: Option<String>,
    effective_model: Option<String>,
    effective_reasoning_target_hash: Option<String>,
    effective_reasoning_scope: Option<String>,
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
pub async fn reduce_report(
    data_dir: PathBuf,
    request: ReportRequest,
    cancel: Arc<AtomicBool>,
) -> Result<ReducedReport> {
    let resource_home = antiburn_local::paths::home_dir();
    tokio::task::spawn_blocking(move || {
        reduce_with_state_on_snapshot(
            &data_dir,
            request,
            &mut || {},
            &cancel,
            &mut || {},
            resource_home.as_deref(),
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
    let sql = CURRENT_FINDINGS_SQL.replace("{current}", CURRENT_EVIDENCE_PREDICATE);
    let mut findings = Vec::with_capacity(CURRENT_FINDING_LIMIT + 1);
    let cancel = AtomicBool::new(false);
    let mut sessions_scanned = 0;
    {
        let mut statement = transaction.prepare(&sql)?;
        let mut rows = statement.query(params![
            request.environment_key,
            request.window.start_epoch,
            request.window.end_epoch,
            PARSER_REVISION,
            ANALYZER_REVISION,
            EVIDENCE_SCHEMA_REVISION,
            METRICS_SCHEMA_REVISION,
            CURRENT_FINDING_SESSION_SCAN_BUDGET + 1,
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
) -> Result<Vec<CurrentFinding>> {
    let catalogs = ReportCatalogs::default();
    let sql = CURRENT_FINDING_BY_KEY_SQL.replace("{current}", CURRENT_EVIDENCE_PREDICATE);
    let Some(session) = connection
        .query_row(
            &sql,
            params![
                key.environment_key,
                key.agent,
                key.session_id,
                PARSER_REVISION,
                ANALYZER_REVISION,
                EVIDENCE_SCHEMA_REVISION,
                METRICS_SCHEMA_REVISION,
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
    let sql = CURRENT_FINDING_BY_KEY_SQL.replace("{current}", CURRENT_EVIDENCE_PREDICATE);
    let session = transaction
        .query_row(
            &sql,
            params![
                cached.environment_key,
                cached.agent,
                cached.session_id,
                PARSER_REVISION,
                ANALYZER_REVISION,
                EVIDENCE_SCHEMA_REVISION,
                METRICS_SCHEMA_REVISION,
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
