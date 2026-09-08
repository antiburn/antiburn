use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use antiburn_local::analysis::{
    ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, InitialContextBreakdown, METRICS_SCHEMA_REVISION,
    PARSER_REVISION, SessionEvidence, SourceOrigin,
};
use antiburn_local::insights::{
    CoverageBucket, CoverageCounts, DetectorId, EfficiencyReport, EfficiencyReportAccumulator,
    ReportCatalogs, ReportContext, ReportWindow, SessionTokenBurnEvidence, TokenBurnSourceEvidence,
    TokenBurnTurnAccumulator, TokenBurnTurnEvidence,
};
use antiburn_local::remediation::{Finding, FindingAssessment};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};

use crate::store::open_read_only;

const REPORT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const CURRENT_EVIDENCE_PREDICATE: &str = "
    e.status = 'ready'
    AND NOT (e.analyzed_generation IS NOT s.source_generation)
    AND NOT (e.parser_revision IS NOT ?4)
    AND NOT (e.analyzer_revision IS NOT ?5)
    AND NOT (e.evidence_schema_revision IS NOT ?6)";

const DENOMINATOR_SQL: &str = "
SELECT bucket, COUNT(*), SUM(awaiting_provider_support), SUM(evidence_pending)
  FROM (
    SELECT CASE
             WHEN s.started_at_epoch IS NULL THEN 'unknown_start'
             WHEN e.status IS NULL OR e.status = 'pending' THEN 'pending'
             WHEN e.status = 'processing' THEN 'processing'
             WHEN e.status = 'failed' THEN 'failed'
             WHEN e.status = 'unsupported' THEN 'unsupported'
             WHEN NOT ({current}) THEN 'stale'
             ELSE 'ready'
           END AS bucket,
           CASE WHEN s.started_at_epoch IS NOT NULL AND e.status IS NULL
                 THEN 1 ELSE 0 END AS awaiting_provider_support,
           CASE WHEN e.status IS NULL OR e.status = 'pending' OR e.status = 'processing'
                THEN 1 ELSE 0 END AS evidence_pending
      FROM session s
      LEFT JOIN session_evidence e
        ON e.environment_key = s.environment_key
       AND e.agent = s.agent
       AND e.session_id = s.session_id
     WHERE s.environment_key = ?1
       AND ((s.started_at_epoch >= ?2 AND s.started_at_epoch < ?3)
         OR (s.started_at_epoch IS NULL
             AND s.updated_at_epoch >= ?2 AND s.updated_at_epoch < ?3))
  )
 GROUP BY bucket
 ORDER BY bucket";

const COHORT_SQL: &str = "
SELECT e.evidence_json, s.agent, s.session_id, e.published_fence, a.initial_context_json, s.cwd
  FROM session s
  JOIN session_evidence e
    ON e.environment_key = s.environment_key
   AND e.agent = s.agent
   AND e.session_id = s.session_id
  LEFT JOIN session_analysis a
    ON a.environment_key = s.environment_key
   AND a.agent = s.agent
   AND a.session_id = s.session_id
   AND NOT (a.analyzed_generation IS NOT s.source_generation)
   AND NOT (a.parser_revision IS NOT ?4)
   AND NOT (a.analyzer_revision IS NOT ?5)
   AND NOT (a.metrics_schema_revision IS NOT ?7)
 WHERE s.environment_key = ?1
   AND s.started_at_epoch >= ?2
   AND s.started_at_epoch < ?3
   AND {current}
   ORDER BY s.started_at_epoch DESC, s.session_id DESC";

const TOKEN_BURN_TURNS_SQL: &str = "
SELECT scope, model, effort, speed, ts_ms, input_tokens, output_tokens,
       cache_read_tokens, cache_write_tokens
  FROM turn
 WHERE environment_key = ?1
   AND agent = ?2
   AND session_id = ?3
   AND claim_fence = ?4
   AND role = 'assistant'";

const CURRENT_FINDINGS_SQL: &str = "
SELECT e.evidence_json, s.environment_key, s.agent, s.session_id,
       s.source_generation, e.published_fence, s.source_fingerprint,
       e.processed_fingerprint, e.parser_revision, e.analyzer_revision,
       e.evidence_schema_revision, a.metrics_schema_revision,
       s.started_at_epoch, s.cwd, a.initial_context_json
  FROM session s
  JOIN session_evidence e
    ON e.environment_key = s.environment_key
   AND e.agent = s.agent
   AND e.session_id = s.session_id
  JOIN session_analysis a
    ON a.environment_key = s.environment_key
   AND a.agent = s.agent
   AND a.session_id = s.session_id
   AND NOT (a.analyzed_generation IS NOT s.source_generation)
   AND NOT (a.parser_revision IS NOT ?4)
   AND NOT (a.analyzer_revision IS NOT ?5)
   AND NOT (a.metrics_schema_revision IS NOT ?7)
 WHERE s.environment_key = ?1
   AND s.started_at_epoch >= ?2
   AND s.started_at_epoch < ?3
   AND {current}
   AND (
       ?8 IS NULL
       OR s.started_at_epoch < ?8
       OR (s.started_at_epoch = ?8 AND s.agent < ?9)
       OR (s.started_at_epoch = ?8 AND s.agent = ?9 AND s.session_id <= ?10)
   )
 ORDER BY s.started_at_epoch DESC, s.agent DESC, s.session_id DESC";

const CURRENT_FINDING_BY_KEY_SQL: &str = "
SELECT e.evidence_json, s.environment_key, s.agent, s.session_id,
       s.source_generation, e.published_fence, s.source_fingerprint,
       e.processed_fingerprint, e.parser_revision, e.analyzer_revision,
       e.evidence_schema_revision, a.metrics_schema_revision,
       s.started_at_epoch, s.cwd, a.initial_context_json
  FROM session s
  JOIN session_evidence e
    ON e.environment_key = s.environment_key
   AND e.agent = s.agent
   AND e.session_id = s.session_id
  JOIN session_analysis a
    ON a.environment_key = s.environment_key
   AND a.agent = s.agent
   AND a.session_id = s.session_id
   AND NOT (a.analyzed_generation IS NOT s.source_generation)
   AND NOT (a.parser_revision IS NOT ?4)
   AND NOT (a.analyzer_revision IS NOT ?5)
   AND NOT (a.metrics_schema_revision IS NOT ?7)
 WHERE s.environment_key = ?1
   AND s.agent = ?2
   AND s.session_id = ?3
   AND {current}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportRequest {
    pub environment_key: String,
    pub window: ReportWindow,
    pub computed_at_epoch: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReducedReport {
    pub report: EfficiencyReport,
    pub evidence_settled: bool,
}

/// Selects one detector's current findings in a bounded report window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentFindingsRequest {
    pub environment_key: String,
    pub window: ReportWindow,
    pub detector: DetectorId,
    pub cursor: Option<CurrentFindingsCursor>,
    pub limit: usize,
}

/// An opaque keyset position in the current finding order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentFindingsCursor {
    started_at_epoch: i64,
    agent: String,
    session_id: String,
    cause_index: usize,
}

/// One trusted finding and the exact projection version that produced it.
#[derive(Clone, PartialEq, Eq)]
pub struct CurrentFinding {
    pub finding: Finding,
    pub environment_key: String,
    pub agent: String,
    pub session_id: String,
    pub source_generation: i64,
    pub published_fence: i64,
    pub source_fingerprint: Option<String>,
    pub processed_fingerprint: Option<String>,
    pub parser_revision: i64,
    pub analyzer_revision: i64,
    pub evidence_schema_revision: i64,
    pub metrics_schema_revision: i64,
    pub catalog_revision: i64,
    pub started_at_epoch: i64,
    workspace_candidate: Option<PathBuf>,
    cause_index: usize,
}

impl CurrentFinding {
    /// Returns the private path candidate for trusted configuration inspection.
    pub(crate) fn workspace_candidate(&self) -> Option<&Path> {
        self.workspace_candidate.as_deref()
    }
}

/// One bounded page of current findings in newest-session order.
#[derive(Clone, PartialEq, Eq)]
pub struct CurrentFindingsPage {
    pub findings: Vec<CurrentFinding>,
    pub next_cursor: Option<CurrentFindingsCursor>,
}

struct CurrentFindingSession {
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
    started_at_epoch: i64,
    workspace_candidate: Option<PathBuf>,
    initial_context: Option<InitialContextBreakdown>,
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

fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<()> {
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
    tokio::task::spawn_blocking(move || {
        reduce_with_state_on_snapshot(&data_dir, request, &mut || {}, &cancel, &mut || {})
    })
    .await
    .context("report reduction task failed")?
}

/// Lists one detector's current findings from one read transaction.
pub fn list_current_findings(
    data_dir: &Path,
    request: CurrentFindingsRequest,
) -> Result<CurrentFindingsPage> {
    list_current_findings_on_snapshot(data_dir, request, &mut || {})
}

fn list_current_findings_on_snapshot(
    data_dir: &Path,
    request: CurrentFindingsRequest,
    after_session_read: &mut dyn FnMut(),
) -> Result<CurrentFindingsPage> {
    let connection = open_read_only(data_dir, REPORT_BUSY_TIMEOUT)?;
    let transaction = connection.unchecked_transaction()?;
    let catalogs = ReportCatalogs::default();
    let report_context = TokenBurnReportContext {
        depth_cap: u128::from(catalogs.depth_cap_tokens),
        catalogs: &catalogs,
    };
    let sql = CURRENT_FINDINGS_SQL.replace("{current}", CURRENT_EVIDENCE_PREDICATE);
    let cursor_started_at = request
        .cursor
        .as_ref()
        .map(|cursor| cursor.started_at_epoch);
    let cursor_agent = request.cursor.as_ref().map(|cursor| cursor.agent.as_str());
    let cursor_session_id = request
        .cursor
        .as_ref()
        .map(|cursor| cursor.session_id.as_str());
    let limit = request.limit.clamp(1, 100);
    let mut findings = Vec::with_capacity(limit + 1);
    let cancel = AtomicBool::new(false);
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
            cursor_started_at,
            cursor_agent,
            cursor_session_id,
        ])?;
        while let Some(row) = rows.next()? {
            let session = current_finding_session(row)?;
            after_session_read();
            let token_evidence = token_burn_evidence(
                &transaction,
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
                &report_context,
                &cancel,
                &mut || {},
            )?;
            let assessment = antiburn_local::remediation::assess_session_with_source_evidence(
                &session.evidence,
                &catalogs,
                Some(&token_evidence),
            );
            let FindingAssessment::Findings(session_findings) =
                &assessment.detectors[request.detector.index()]
            else {
                continue;
            };
            for (cause_index, finding) in session_findings.iter().enumerate() {
                if request.cursor.as_ref().is_some_and(|cursor| {
                    cursor.started_at_epoch == session.started_at_epoch
                        && cursor.agent == session.agent
                        && cursor.session_id == session.session_id
                        && cause_index <= cursor.cause_index
                }) {
                    continue;
                }
                findings.push(current_finding(
                    &session,
                    finding.clone(),
                    catalogs.revision,
                    cause_index,
                ));
                if findings.len() > limit {
                    break;
                }
            }
            if findings.len() > limit {
                break;
            }
        }
    }

    let has_more = findings.len() > limit;
    findings.truncate(limit);
    let next_cursor = has_more.then(|| {
        let last = findings.last().expect("a full finding page is not empty");
        CurrentFindingsCursor {
            started_at_epoch: last.started_at_epoch,
            agent: last.agent.clone(),
            session_id: last.session_id.clone(),
            cause_index: last.cause_index,
        }
    });
    Ok(CurrentFindingsPage {
        findings,
        next_cursor,
    })
}

/// Recomputes one cached finding and accepts only its exact cause and freshness.
pub fn revalidate_current_finding(data_dir: &Path, cached: &CurrentFinding) -> Result<bool> {
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
    let report_context = TokenBurnReportContext {
        depth_cap: u128::from(catalogs.depth_cap_tokens),
        catalogs: &catalogs,
    };
    let cancel = AtomicBool::new(false);
    let token_evidence = token_burn_evidence(
        &transaction,
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
        &report_context,
        &cancel,
        &mut || {},
    )?;
    let assessment = antiburn_local::remediation::assess_session_with_source_evidence(
        &session.evidence,
        &catalogs,
        Some(&token_evidence),
    );
    let FindingAssessment::Findings(findings) =
        &assessment.detectors[cached.finding.detector.index()]
    else {
        return Ok(false);
    };
    Ok(findings.iter().any(|finding| {
        finding.detector == cached.finding.detector && finding.cause() == cached.finding.cause()
    }))
}

fn current_finding_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<CurrentFindingSession> {
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
    })
}

fn current_finding(
    session: &CurrentFindingSession,
    finding: Finding,
    catalog_revision: i64,
    cause_index: usize,
) -> CurrentFinding {
    CurrentFinding {
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
        started_at_epoch: session.started_at_epoch,
        workspace_candidate: session.workspace_candidate.clone(),
        cause_index,
    }
}

fn freshness_matches(cached: &CurrentFinding, session: &CurrentFindingSession) -> bool {
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
        && cached.started_at_epoch == session.started_at_epoch
        && cached.workspace_candidate == session.workspace_candidate
}

#[cfg(test)]
fn reduce_on_snapshot(
    data_dir: &Path,
    request: ReportRequest,
    after_denominator: &mut dyn FnMut(),
    cancel: &AtomicBool,
) -> Result<EfficiencyReport> {
    Ok(
        reduce_with_state_on_snapshot(data_dir, request, after_denominator, cancel, &mut || {})?
            .report,
    )
}

fn reduce_with_state_on_snapshot(
    data_dir: &Path,
    request: ReportRequest,
    after_denominator: &mut dyn FnMut(),
    cancel: &AtomicBool,
    turn_probe: &mut dyn FnMut(),
) -> Result<ReducedReport> {
    ensure_not_cancelled(cancel)?;
    let connection = open_read_only(data_dir, REPORT_BUSY_TIMEOUT)?;
    let transaction = connection.unchecked_transaction()?;
    let mut coverage = CoverageCounts::default();
    let mut pending_evidence = 0_u64;
    let denominator_sql = DENOMINATOR_SQL.replace("{current}", CURRENT_EVIDENCE_PREDICATE);
    {
        let mut statement = transaction.prepare(&denominator_sql)?;
        let mut rows = statement.query(params![
            request.environment_key,
            request.window.start_epoch,
            request.window.end_epoch,
            PARSER_REVISION,
            ANALYZER_REVISION,
            EVIDENCE_SCHEMA_REVISION,
        ])?;
        while let Some(row) = rows.next()? {
            let bucket = coverage_bucket(row.get::<_, String>(0)?.as_str())?;
            let count = u64::try_from(row.get::<_, i64>(1)?)?;
            let awaiting_provider_support = u64::try_from(row.get::<_, i64>(2)?)?;
            coverage.observe(bucket, count);
            coverage.awaiting_provider_support += awaiting_provider_support;
            pending_evidence += u64::try_from(row.get::<_, i64>(3)?)?;
        }
    }
    ensure!(
        coverage.is_consistent(),
        "report coverage does not partition"
    );

    after_denominator();
    ensure_not_cancelled(cancel)?;

    let mut accumulator = EfficiencyReportAccumulator::new();
    let depth_cap = u128::from(accumulator.catalogs().depth_cap_tokens);
    let cohort_sql = COHORT_SQL.replace("{current}", CURRENT_EVIDENCE_PREDICATE);
    {
        let mut statement = transaction.prepare(&cohort_sql)?;
        let mut rows = statement.query(params![
            request.environment_key,
            request.window.start_epoch,
            request.window.end_epoch,
            PARSER_REVISION,
            ANALYZER_REVISION,
            EVIDENCE_SCHEMA_REVISION,
            METRICS_SCHEMA_REVISION,
        ])?;
        while let Some(row) = rows.next()? {
            ensure_not_cancelled(cancel)?;
            let evidence_json: String = row.get(0)?;
            let evidence: SessionEvidence = serde_json::from_str(&evidence_json)
                .context("stored session evidence is invalid")?;
            let agent: String = row.get(1)?;
            let session_id: String = row.get(2)?;
            let published_fence: i64 = row.get(3)?;
            let initial_context_json: Option<String> = row.get(4)?;
            let cwd: Option<String> = row.get(5)?;
            let initial_context = initial_context_json
                .as_deref()
                .map(serde_json::from_str::<InitialContextBreakdown>)
                .transpose()
                .context("stored initial context is invalid")?;
            let token_burn_context = TokenBurnReportContext {
                catalogs: accumulator.catalogs(),
                depth_cap,
            };
            let token_evidence = token_burn_evidence(
                &transaction,
                TokenBurnSessionKey {
                    environment_key: &request.environment_key,
                    agent: &agent,
                    session_id: &session_id,
                    published_fence,
                    cwd: cwd.as_deref(),
                },
                initial_context.as_ref(),
                &evidence,
                &token_burn_context,
                cancel,
                turn_probe,
            )?;
            accumulator.observe_session_with_token_burn(evidence, token_evidence);
        }
    }

    ensure_not_cancelled(cancel)?;
    let report = accumulator.finish(ReportContext {
        environment_key: request.environment_key,
        window: request.window,
        computed_at_epoch: request.computed_at_epoch,
        parser_revision: PARSER_REVISION,
        analyzer_revision: ANALYZER_REVISION,
        evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
        coverage,
    });
    turn_probe();
    ensure_not_cancelled(cancel)?;
    ensure!(
        report.context.coverage.actively_growing <= report.context.coverage.ready,
        "actively growing coverage exceeds ready coverage"
    );
    drop(transaction);
    drop(connection);
    Ok(ReducedReport {
        report,
        evidence_settled: pending_evidence == 0,
    })
}

struct TokenBurnSessionKey<'a> {
    environment_key: &'a str,
    agent: &'a str,
    session_id: &'a str,
    published_fence: i64,
    cwd: Option<&'a str>,
}

struct SourceTokenCounter {
    evidence: TokenBurnSourceEvidence,
    definition_tokens: u128,
}

struct TokenBurnReportContext<'a> {
    catalogs: &'a ReportCatalogs,
    depth_cap: u128,
}

fn token_burn_evidence(
    connection: &rusqlite::Connection,
    key: TokenBurnSessionKey<'_>,
    initial_context: Option<&InitialContextBreakdown>,
    evidence: &SessionEvidence,
    report_context: &TokenBurnReportContext<'_>,
    cancel: &AtomicBool,
    turn_probe: &mut dyn FnMut(),
) -> Result<SessionTokenBurnEvidence> {
    // The partial index limits row discovery to this session's assistant turns.
    // This build omits rusqlite hooks, so probes run per row and before finalization.
    let mut statement = connection.prepare_cached(TOKEN_BURN_TURNS_SQL)?;
    let mut rows = statement.query(params![
        key.environment_key,
        key.agent,
        key.session_id,
        key.published_fence
    ])?;
    let mut source_groups = initial_context.map(|initial_context| {
        [
            key.cwd.and_then(|cwd| {
                source_token_counters(
                    initial_context,
                    "mcp_instructions",
                    &format!("{}:cwd:{cwd}", key.agent),
                    None,
                )
            }),
            source_token_counters(initial_context, "builtin_tool", key.agent, None)
                .filter(|sources| !sources.is_empty()),
            source_token_counters(initial_context, "skill_instructions", key.agent, key.cwd),
        ]
    });
    let mut turn_accumulator = TokenBurnTurnAccumulator::new(report_context.catalogs);
    let mut has_unattributed_assistant_turn = false;
    let mut raw_total_tokens = 0_u128;
    let mut overdepth_avoidable_tokens = 0_u128;
    while let Some(row) = rows.next()? {
        turn_probe();
        ensure_not_cancelled(cancel)?;
        let scope: String = row.get(0)?;
        let model: Option<String> = row.get(1)?;
        let effort: Option<String> = row.get(2)?;
        let speed: Option<String> = row.get(3)?;
        let ts_ms: Option<i64> = row.get(4)?;
        let input_tokens = u64::try_from(row.get::<_, i64>(5)?)?;
        let output_tokens = u64::try_from(row.get::<_, i64>(6)?)?;
        let cache_read_tokens = u64::try_from(row.get::<_, i64>(7)?)?;
        let cache_write_tokens = u64::try_from(row.get::<_, i64>(8)?)?;
        let Some(model) = model.filter(|model| !model.trim().is_empty()) else {
            has_unattributed_assistant_turn = true;
            continue;
        };
        let turn = TokenBurnTurnEvidence {
            scope: scope.clone(),
            model,
            effort,
            speed,
            ts_ms,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        };
        let input = u128::from(input_tokens);
        let output = u128::from(output_tokens);
        let cache_read = u128::from(cache_read_tokens);
        let cache_write = u128::from(cache_write_tokens);
        let context = input
            .checked_add(cache_read)
            .and_then(|value| value.checked_add(cache_write))
            .context("turn context token total overflowed")?;
        let turn_total = context
            .checked_add(output)
            .context("turn token total overflowed")?;
        raw_total_tokens = raw_total_tokens
            .checked_add(turn_total)
            .context("session token total overflowed")?;
        if context > report_context.depth_cap {
            overdepth_avoidable_tokens = overdepth_avoidable_tokens
                .checked_add(
                    avoidable_overdepth_tokens(
                        input,
                        cache_read,
                        cache_write,
                        report_context.depth_cap,
                    )
                    .context("overdepth token calculation overflowed")?,
                )
                .context("overdepth token total overflowed")?;
        }
        if scope == "main"
            && let Some(groups) = &mut source_groups
        {
            observe_main_context(groups, context)?;
        }
        turn_accumulator.observe(turn);
    }

    let mut result = SessionTokenBurnEvidence::from_session(evidence);
    if raw_total_tokens > 0 {
        result.total_tokens = Some(raw_total_tokens);
    }
    turn_probe();
    ensure_not_cancelled(cancel)?;
    turn_accumulator.finish_into(&mut result);
    result.overdepth_avoidable_tokens = Some(overdepth_avoidable_tokens);
    if has_unattributed_assistant_turn {
        result.repeated_context_avoidable_tokens = None;
    }
    if let Some([mcp, built_in, skills]) = source_groups {
        result.mcp_sources = finish_source_counters(mcp);
        result.built_in_tool_sources = finish_source_counters(built_in);
        result.skill_sources = finish_source_counters(skills);
    }
    Ok(result)
}

fn avoidable_overdepth_tokens(
    input: u128,
    cache_read: u128,
    cache_write: u128,
    depth_cap: u128,
) -> Option<u128> {
    let context = input.checked_add(cache_read)?.checked_add(cache_write)?;
    let excess = context.saturating_sub(depth_cap);
    cache_read
        .checked_add(cache_write)
        .map(|cache| cache.min(excess))
}

fn source_token_counters(
    initial_context: &InitialContextBreakdown,
    source_kind: &str,
    agent: &str,
    skill_cwd: Option<&str>,
) -> Option<Vec<SourceTokenCounter>> {
    let matching = initial_context
        .sources
        .iter()
        .filter(|source| source.source == source_kind)
        .collect::<Vec<_>>();
    if matching.iter().any(|source| {
        source.source_name.as_deref().is_none_or(|name| {
            name == "Other skills" || name == "Other MCP servers" || name == "Other built-in tools"
        }) || source.deferred
    }) {
        return None;
    }
    matching
        .into_iter()
        .filter(|source| source.token_count > 0)
        .map(|source| {
            let name = source.source_name.as_deref()?.trim().to_lowercase();
            if name.is_empty() {
                return None;
            }
            let scope = if source_kind == "skill_instructions"
                && matches!(source.origin, SourceOrigin::Project | SourceOrigin::Unknown)
            {
                format!("{agent}:cwd:{}", skill_cwd?)
            } else {
                format!("{agent}:{}", source_origin_key(source.origin))
            };
            Some(SourceTokenCounter {
                evidence: TokenBurnSourceEvidence {
                    scope,
                    name,
                    replicated_tokens: 0,
                    invoked: source.use_count > 0,
                },
                definition_tokens: u128::from(source.token_count),
            })
        })
        .collect()
}

fn observe_main_context(
    groups: &mut [Option<Vec<SourceTokenCounter>>; 3],
    context_tokens: u128,
) -> Result<()> {
    for sources in groups.iter_mut().flatten() {
        for source in sources {
            if context_tokens >= source.definition_tokens {
                source.evidence.replicated_tokens = source
                    .evidence
                    .replicated_tokens
                    .checked_add(source.definition_tokens)
                    .context("source replicated token total overflowed")?;
            }
        }
    }
    Ok(())
}

fn finish_source_counters(
    counters: Option<Vec<SourceTokenCounter>>,
) -> Option<Vec<TokenBurnSourceEvidence>> {
    counters.map(|counters| {
        counters
            .into_iter()
            .map(|counter| counter.evidence)
            .collect()
    })
}

#[cfg(test)]
fn source_token_evidence(
    initial_context: &InitialContextBreakdown,
    source_kind: &str,
    agent: &str,
    skill_cwd: Option<&str>,
    main_context_capacities: &[u128],
) -> Option<Vec<TokenBurnSourceEvidence>> {
    let mut groups = [
        source_token_counters(initial_context, source_kind, agent, skill_cwd),
        None,
        None,
    ];
    for capacity in main_context_capacities {
        observe_main_context(&mut groups, *capacity).ok()?;
    }
    finish_source_counters(groups[0].take())
}

fn source_origin_key(origin: SourceOrigin) -> &'static str {
    match origin {
        SourceOrigin::Bundled => "bundled",
        SourceOrigin::Plugin => "plugin",
        SourceOrigin::User => "user",
        SourceOrigin::Project => "project",
        SourceOrigin::Unknown => "unknown",
    }
}

fn coverage_bucket(value: &str) -> Result<CoverageBucket> {
    match value {
        "unknown_start" => Ok(CoverageBucket::UnknownStart),
        "pending" => Ok(CoverageBucket::Pending),
        "processing" => Ok(CoverageBucket::Processing),
        "failed" => Ok(CoverageBucket::Failed),
        "unsupported" => Ok(CoverageBucket::Unsupported),
        "stale" => Ok(CoverageBucket::Stale),
        "ready" => Ok(CoverageBucket::Ready),
        _ => anyhow::bail!("unknown report coverage bucket: {value}"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;

    use antiburn_local::analysis::{
        EVIDENCE_SCHEMA_REVISION, EvidenceSource, EvidenceValue, LoadedSource,
        METRICS_SCHEMA_REVISION, SessionEvidenceAccumulator, SourceCapabilities, SourceKind,
        TurnFacts, TurnRow, TurnRowStore, TurnScope,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::store::{
        AnalysisRecord, EvidenceCompletion, EvidenceFailure, FencedTurnRowStore,
        ProjectionRevisions, PublishedEvidence, SessionKey, SessionRecord, Store,
    };

    fn request() -> ReportRequest {
        ReportRequest {
            environment_key: "native".to_owned(),
            window: ReportWindow {
                start_epoch: 100,
                end_epoch: 200,
            },
            computed_at_epoch: 200,
        }
    }

    #[test]
    fn source_estimates_repeat_definition_tokens_across_compatible_turns() {
        let initial_context: InitialContextBreakdown = serde_json::from_str(
            r#"{"sources":[{"source":"skill_instructions","sourceName":"Review","tokenCount":200,"useCount":0,"origin":"user"}]}"#,
        )
        .unwrap();

        let sources = source_token_evidence(
            &initial_context,
            "skill_instructions",
            "claude",
            None,
            &[199, 200, 800],
        )
        .unwrap();

        assert_eq!(
            sources,
            vec![TokenBurnSourceEvidence {
                scope: "claude:user".to_owned(),
                name: "review".to_owned(),
                replicated_tokens: 400,
                invoked: false,
            }]
        );
    }

    #[test]
    fn overdepth_estimates_only_avoidable_cache_buckets_above_the_cap() {
        assert_eq!(
            avoidable_overdepth_tokens(10_000, 450_000, 20_000, 400_000),
            Some(80_000)
        );
        assert_eq!(
            avoidable_overdepth_tokens(10_000, 300_000, 20_000, 400_000),
            Some(0)
        );
    }

    #[test]
    fn source_estimates_reject_ambiguous_or_deferred_rows() {
        for row in [
            r#"{"source":"skill_instructions","sourceName":"Other skills","tokenCount":200,"useCount":0,"origin":"unknown"}"#,
            r#"{"source":"skill_instructions","sourceName":"Review","tokenCount":200,"useCount":0,"origin":"unknown","deferred":true}"#,
            r#"{"source":"skill_instructions","tokenCount":200,"useCount":0,"origin":"unknown"}"#,
        ] {
            let initial_context: InitialContextBreakdown =
                serde_json::from_str(&format!(r#"{{"sources":[{row}]}}"#)).unwrap();
            assert!(
                source_token_evidence(
                    &initial_context,
                    "skill_instructions",
                    "claude",
                    None,
                    &[500]
                )
                .is_none()
            );
        }
    }

    #[test]
    fn project_skill_origins_use_the_working_directory_scope() {
        let initial_context: InitialContextBreakdown = serde_json::from_str(
            r#"{"sources":[{"source":"skill_instructions","sourceName":"Review","tokenCount":200,"useCount":0,"origin":"project"}]}"#,
        )
        .unwrap();

        let sources = source_token_evidence(
            &initial_context,
            "skill_instructions",
            "claude",
            Some("/projects/one"),
            &[500],
        )
        .unwrap();

        assert_eq!(sources[0].scope, "claude:cwd:/projects/one");
        assert!(
            source_token_evidence(
                &initial_context,
                "skill_instructions",
                "claude",
                None,
                &[500]
            )
            .is_none()
        );
    }

    #[test]
    fn unknown_skill_origins_use_the_working_directory_scope() {
        let initial_context: InitialContextBreakdown = serde_json::from_str(
            r#"{"sources":[{"source":"skill_instructions","sourceName":"Review","tokenCount":200,"useCount":0,"origin":"unknown"}]}"#,
        )
        .unwrap();

        let sources = source_token_evidence(
            &initial_context,
            "skill_instructions",
            "claude",
            Some("/projects/one"),
            &[500],
        )
        .unwrap();

        assert_eq!(sources[0].scope, "claude:cwd:/projects/one");
        assert!(
            source_token_evidence(
                &initial_context,
                "skill_instructions",
                "claude",
                None,
                &[500]
            )
            .is_none()
        );
    }

    fn session(session_id: &str, updated_at_epoch: i64, fingerprint: &str) -> SessionRecord {
        SessionRecord {
            key: SessionKey::new("native", "claude-code", session_id),
            source_kind: "file".to_owned(),
            source_label: format!("/home/avery/.claude/{session_id}.jsonl"),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: None,
            surface: "cli".to_owned(),
            updated_at_epoch: Some(updated_at_epoch),
            activity_cursor: String::new(),
            activity_source: "event".to_owned(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: Some(fingerprint.to_owned()),
        }
    }

    fn publish_evidence(
        store: &Store,
        session_id: &str,
        started_at_epoch: i64,
        status: PublishedEvidence,
    ) {
        publish_evidence_with_turns(store, session_id, started_at_epoch, status, 0);
    }

    fn publish_evidence_with_turns(
        store: &Store,
        session_id: &str,
        started_at_epoch: i64,
        status: PublishedEvidence,
        turn_count: usize,
    ) {
        publish_evidence_with_mutator(
            store,
            session_id,
            started_at_epoch,
            status,
            turn_count,
            |_| {},
        );
    }

    fn publish_evidence_with_mutator(
        store: &Store,
        session_id: &str,
        started_at_epoch: i64,
        status: PublishedEvidence,
        turn_count: usize,
        mutate: impl FnOnce(&mut SessionEvidence),
    ) {
        let fingerprint = format!("sv1:{session_id}");
        let session = session(session_id, started_at_epoch, &fingerprint);
        store
            .upsert_sessions(std::slice::from_ref(&session), &["claude-code"])
            .unwrap();
        let claim = store
            .claim_next_evidence(&["claude-code"], 10, 60)
            .unwrap()
            .unwrap();
        assert_eq!(claim.key, session.key);
        if turn_count > 0 {
            let writer =
                FencedTurnRowStore::new(store.clone(), session.key.clone(), claim.claim_fence);
            let turns = (0..turn_count)
                .map(|turn_index| TurnRow {
                    source_key: "synthetic".to_owned(),
                    thread_id: "synthetic".to_owned(),
                    turn_index: turn_index as u64,
                    scope: TurnScope::Main,
                    child_id: None,
                    role: "assistant",
                    ts_ms: Some(1_000 + turn_index as i64),
                    model: Some("claude-sonnet-5".to_owned()),
                    provider: None,
                    api: None,
                    effort: Some("high".to_owned()),
                    speed: None,
                    input_tokens: 10,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    output_tokens: 5,
                    is_compaction_boundary: false,
                    message_id: None,
                    uuid: None,
                    parent_uuid: None,
                    compaction_trigger: None,
                    compaction_pre_tokens: None,
                    compaction_post_tokens: None,
                    has_thinking: false,
                    last_tool: None,
                    subagent_launches: 0,
                    content: Vec::new(),
                })
                .collect::<Vec<_>>();
            writer.write_turn_rows(&turns).unwrap();
        }
        let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude-code".to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence(&TurnFacts::default());
        mutate(&mut evidence);
        let analysis = AnalysisRecord {
            key: session.key,
            model_breakdown_json: "{}".to_owned(),
            pricing_breakdown_json: "{}".to_owned(),
            inclusive_models_json: "[]".to_owned(),
            initial_context_json: None,
            source_summaries_json: None,
            provider_hints_json: None,
            source_fingerprint: fingerprint,
            pricing_generation: 1,
            analyzed_generation: claim.source_generation,
            parser_revision: PARSER_REVISION,
            analyzer_revision: ANALYZER_REVISION,
            metrics_schema_revision: METRICS_SCHEMA_REVISION,
        };
        let completion = EvidenceCompletion {
            claim_fence: claim.claim_fence,
            status,
            evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
            evidence_json: serde_json::to_string(&evidence).unwrap(),
        };
        assert!(
            store
                .publish_projections(&analysis, Some(started_at_epoch), &completion, &[], &[])
                .unwrap()
        );
    }

    fn publish_ready(store: &Store, session_id: &str, started_at_epoch: i64) {
        publish_evidence(
            store,
            session_id,
            started_at_epoch,
            PublishedEvidence::Ready,
        );
    }

    fn publish_mcp_findings(
        store: &Store,
        session_id: &str,
        started_at_epoch: i64,
        servers: &[&str],
    ) {
        publish_evidence_with_mutator(
            store,
            session_id,
            started_at_epoch,
            PublishedEvidence::Ready,
            1,
            |evidence| {
                let EvidenceValue::Complete(eligibility) = &mut evidence.eligibility else {
                    panic!("the Claude fixture must have complete eligibility");
                };
                eligibility.assistant_turns = 1;
                let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
                    panic!("the Claude fixture must have complete context sources");
                };
                sources.mcp_coverage = EvidenceValue::Complete(());
                for server in servers {
                    sources.mcp_servers.insert(
                        (*server).to_owned(),
                        LoadedSource {
                            description: None,
                            configured: true,
                            available: true,
                            injected: true,
                            invoked: false,
                            token_count: None,
                            origin: EvidenceValue::Unsupported,
                        },
                    );
                }
            },
        );
    }

    fn finding_request(
        limit: usize,
        cursor: Option<CurrentFindingsCursor>,
    ) -> CurrentFindingsRequest {
        CurrentFindingsRequest {
            environment_key: "native".to_owned(),
            window: request().window,
            detector: DetectorId::UnusedMcpServers,
            cursor,
            limit,
        }
    }

    #[test]
    fn current_finding_reads_evidence_analysis_and_turns_from_one_snapshot() {
        let data_dir = TempDir::new().unwrap();
        let store = Store::open(data_dir.path()).unwrap();
        publish_mcp_findings(&store, "snapshot", 120, &["server-a"]);
        let writer = Store::open(data_dir.path()).unwrap();
        let mut changed = false;

        let page = list_current_findings_on_snapshot(
            data_dir.path(),
            finding_request(10, None),
            &mut || {
                if !changed {
                    change_source(&writer, "snapshot", &[]);
                    publish_mcp_findings(&writer, "snapshot", 120, &["server-b"]);
                    changed = true;
                }
            },
        )
        .unwrap();

        assert_eq!(page.findings.len(), 1);
        assert_eq!(page.findings[0].source_generation, 1);
        assert_eq!(
            page.findings[0].source_fingerprint.as_deref(),
            Some("sv1:snapshot")
        );
        assert_eq!(
            page.findings[0].processed_fingerprint.as_deref(),
            Some("sv1:snapshot")
        );
        assert_eq!(page.findings[0].parser_revision, PARSER_REVISION);
        assert_eq!(page.findings[0].analyzer_revision, ANALYZER_REVISION);
        assert_eq!(
            page.findings[0].evidence_schema_revision,
            EVIDENCE_SCHEMA_REVISION
        );
        assert_eq!(
            page.findings[0].metrics_schema_revision,
            METRICS_SCHEMA_REVISION
        );
        assert_eq!(
            page.findings[0].catalog_revision,
            ReportCatalogs::default().revision
        );
        assert_eq!(page.findings[0].started_at_epoch, 120);
        assert!(page.findings[0].workspace_candidate().is_none());
        let next = list_current_findings(data_dir.path(), finding_request(10, None)).unwrap();
        let antiburn_local::remediation::FindingCause::UnusedMcpServer { server } =
            next.findings[0].finding.cause()
        else {
            panic!("the MCP detector must return an MCP cause");
        };
        assert_eq!(server, "server-b");
        assert!(next.findings[0].source_generation > page.findings[0].source_generation);
    }

    #[test]
    fn current_finding_cursor_pages_multiple_causes_in_one_session() {
        let data_dir = TempDir::new().unwrap();
        let store = Store::open(data_dir.path()).unwrap();
        publish_mcp_findings(
            &store,
            "many-causes",
            120,
            &["server-a", "server-b", "server-c"],
        );
        let mut cursor = None;
        let mut servers = Vec::new();

        loop {
            let page = list_current_findings(data_dir.path(), finding_request(0, cursor)).unwrap();
            let Some(finding) = page.findings.first() else {
                break;
            };
            let antiburn_local::remediation::FindingCause::UnusedMcpServer { server } =
                finding.finding.cause()
            else {
                panic!("the MCP detector must return an MCP cause");
            };
            servers.push(server.clone());
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }

        assert_eq!(servers, ["server-a", "server-b", "server-c"]);
    }

    #[test]
    fn current_findings_reject_changed_generations_and_pending_evidence() {
        let data_dir = TempDir::new().unwrap();
        let store = Store::open(data_dir.path()).unwrap();
        publish_mcp_findings(&store, "current", 120, &["current-server"]);
        publish_mcp_findings(&store, "changed", 121, &["changed-server"]);
        change_source(&store, "changed", &[]);
        publish_mcp_findings(&store, "pending", 122, &["pending-server"]);
        change_source(&store, "pending", &["claude-code"]);

        let page = list_current_findings(data_dir.path(), finding_request(1000, None)).unwrap();

        assert_eq!(page.findings.len(), 1);
        assert_eq!(page.findings[0].session_id, "current");
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn revalidation_requires_the_exact_detector_cause() {
        let data_dir = TempDir::new().unwrap();
        let store = Store::open(data_dir.path()).unwrap();
        publish_mcp_findings(&store, "target", 120, &["target-server"]);
        publish_mcp_findings(&store, "other", 121, &["other-server"]);
        let page = list_current_findings(data_dir.path(), finding_request(10, None)).unwrap();
        let target = page
            .findings
            .iter()
            .find(|finding| finding.session_id == "target")
            .unwrap();
        let other = page
            .findings
            .iter()
            .find(|finding| finding.session_id == "other")
            .unwrap();

        assert!(revalidate_current_finding(data_dir.path(), target).unwrap());
        let mut wrong_cause = target.clone();
        wrong_cause.finding = other.finding.clone();
        assert!(!revalidate_current_finding(data_dir.path(), &wrong_cause).unwrap());
    }

    #[test]
    fn cohort_query_uses_the_insights_window_index() {
        let data_dir = TempDir::new().unwrap();
        let _store = Store::open(data_dir.path()).unwrap();
        let connection = open_read_only(data_dir.path(), REPORT_BUSY_TIMEOUT).unwrap();
        let sql = format!(
            "EXPLAIN QUERY PLAN {}",
            COHORT_SQL.replace("{current}", CURRENT_EVIDENCE_PREDICATE)
        );
        let mut statement = connection.prepare(&sql).unwrap();
        let details = statement
            .query_map(
                params![
                    "native",
                    100,
                    200,
                    PARSER_REVISION,
                    ANALYZER_REVISION,
                    EVIDENCE_SCHEMA_REVISION,
                    METRICS_SCHEMA_REVISION,
                ],
                |row| row.get::<_, String>(3),
            )
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();

        assert!(
            details
                .iter()
                .any(|detail| detail.contains("session_insights_window")),
            "query plan did not use the Insights window index: {details:?}"
        );
        assert!(
            details
                .iter()
                .all(|detail| !detail.contains("USE TEMP B-TREE FOR ORDER BY")),
            "query plan used a temporary sort: {details:?}"
        );
    }

    #[test]
    fn token_turn_query_uses_the_session_index_without_a_temporary_sort() {
        let data_dir = TempDir::new().unwrap();
        let _store = Store::open(data_dir.path()).unwrap();
        let connection = open_read_only(data_dir.path(), REPORT_BUSY_TIMEOUT).unwrap();
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {TOKEN_BURN_TURNS_SQL}"))
            .unwrap();
        let details = statement
            .query_map(params!["native", "claude-code", "session", 1], |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();

        assert!(
            details.iter().any(
                |detail| detail.contains("USING INDEX turn_assistant_session")
                    && detail.contains("environment_key=?")
                    && detail.contains("agent=?")
                    && detail.contains("session_id=?")
                    && detail.contains("claim_fence=?")
            ),
            "query plan did not constrain the assistant session index: {details:?}"
        );
        assert!(
            details
                .iter()
                .all(|detail| !detail.contains("USE TEMP B-TREE FOR ORDER BY")),
            "query plan used a temporary sort: {details:?}"
        );
    }

    fn change_source(store: &Store, session_id: &str, evidence_agents: &[&str]) {
        let changed = session(session_id, 150, &format!("sv2:{session_id}"));
        store
            .upsert_sessions(std::slice::from_ref(&changed), evidence_agents)
            .unwrap();
    }

    mod concurrency {
        use super::*;

        #[test]
        fn report_pins_one_snapshot_without_blocking_the_writer() {
            let data_dir = TempDir::new().unwrap();
            let store = Store::open(data_dir.path()).unwrap();
            publish_ready(&store, "before", 120);
            let writer = Store::open(data_dir.path()).unwrap();
            let (release_tx, release_rx) = mpsc::channel();
            let (committed_tx, committed_rx) = mpsc::channel();
            let writer_thread = thread::spawn(move || {
                release_rx.recv().unwrap();
                publish_ready(&writer, "during", 130);
                committed_tx.send(()).unwrap();
            });
            let mut release_tx = Some(release_tx);

            let first = reduce_on_snapshot(
                data_dir.path(),
                request(),
                &mut || {
                    release_tx.take().unwrap().send(()).unwrap();
                    committed_rx
                        .recv_timeout(REPORT_BUSY_TIMEOUT)
                        .expect("the writer must commit while the reader holds its snapshot");
                },
                &AtomicBool::new(false),
            )
            .unwrap();
            writer_thread.join().unwrap();

            assert_eq!(first.context.coverage.discovered, 1);
            assert_eq!(first.assessed_sessions, 1);
            let second = reduce_on_snapshot(
                data_dir.path(),
                request(),
                &mut || {},
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(second.context.coverage.discovered, 2);
            assert_eq!(second.assessed_sessions, 2);
        }

        #[tokio::test]
        async fn async_entry_point_reduces_inside_a_blocking_task() {
            let data_dir = TempDir::new().unwrap();
            let store = Store::open(data_dir.path()).unwrap();
            publish_ready(&store, "ready", 120);

            let report = reduce_report(
                data_dir.path().to_path_buf(),
                request(),
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();

            assert_eq!(report.report.context.coverage.discovered, 1);
            assert_eq!(report.report.assessed_sessions, 1);
            assert!(report.evidence_settled);
        }

        #[tokio::test]
        async fn unknown_start_pending_evidence_keeps_the_snapshot_unsettled() {
            let data_dir = TempDir::new().unwrap();
            let store = Store::open(data_dir.path()).unwrap();
            store
                .upsert_sessions(&[session("pending", 120, "sv1:pending")], &["claude-code"])
                .unwrap();

            let report = reduce_report(
                data_dir.path().to_path_buf(),
                request(),
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();

            assert_eq!(report.report.context.coverage.unknown_start, 1);
            assert!(!report.evidence_settled);
        }
    }

    mod cancellation {
        use super::*;

        #[test]
        fn a_cancel_between_phases_stops_the_reduction_and_keeps_evidence_intact() {
            let data_dir = TempDir::new().unwrap();
            let store = Store::open(data_dir.path()).unwrap();
            publish_ready(&store, "ready", 120);
            let key = SessionKey::new("native", "claude-code", "ready");
            let before = store.evidence(&key).unwrap().unwrap();

            let cancel = AtomicBool::new(false);
            let error = reduce_on_snapshot(
                data_dir.path(),
                request(),
                &mut || cancel.store(true, Ordering::SeqCst),
                &cancel,
            )
            .unwrap_err();
            assert!(is_cancelled(&error));

            // The durable evidence state is untouched: the store still
            // opens and the row reads back unchanged.
            let after = store.evidence(&key).unwrap().unwrap();
            assert_eq!(after.status, before.status);
            assert_eq!(after.evidence_json, before.evidence_json);
            assert_eq!(after.claim_fence, before.claim_fence);

            // A fresh reduction succeeds after the cancelled one.
            let report = reduce_on_snapshot(
                data_dir.path(),
                request(),
                &mut || {},
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(report.assessed_sessions, 1);
        }

        #[test]
        fn an_already_cancelled_request_stops_before_it_opens_a_snapshot() {
            let data_dir = TempDir::new().unwrap();
            let store = Store::open(data_dir.path()).unwrap();
            publish_ready(&store, "ready", 120);

            // The flag is set before the call, so the first probe stops
            // the reduction.
            let cancel = AtomicBool::new(true);
            let error =
                reduce_on_snapshot(data_dir.path(), request(), &mut || {}, &cancel).unwrap_err();
            assert!(is_cancelled(&error));
        }

        #[test]
        fn cancellation_during_turn_iteration_stops_without_publishing_a_report() {
            let data_dir = TempDir::new().unwrap();
            let store = Store::open(data_dir.path()).unwrap();
            publish_evidence_with_turns(&store, "large", 120, PublishedEvidence::Ready, 100);
            let key = SessionKey::new("native", "claude-code", "large");
            let before = store.evidence(&key).unwrap().unwrap();
            let cancel = AtomicBool::new(false);
            let mut turns_scanned = 0;

            let error = reduce_with_state_on_snapshot(
                data_dir.path(),
                request(),
                &mut || {},
                &cancel,
                &mut || {
                    turns_scanned += 1;
                    if turns_scanned == 10 {
                        cancel.store(true, Ordering::SeqCst);
                    }
                },
            )
            .unwrap_err();

            assert!(is_cancelled(&error));
            assert_eq!(turns_scanned, 10);
            let after = store.evidence(&key).unwrap().unwrap();
            assert_eq!(after.evidence_json, before.evidence_json);
            assert_eq!(after.published_fence, before.published_fence);
        }

        #[test]
        fn cancellation_before_turn_finalization_stops_without_publishing_a_report() {
            let data_dir = TempDir::new().unwrap();
            let store = Store::open(data_dir.path()).unwrap();
            publish_evidence_with_turns(&store, "large", 120, PublishedEvidence::Ready, 100);
            let cancel = AtomicBool::new(false);
            let mut probes = 0;

            let error = reduce_with_state_on_snapshot(
                data_dir.path(),
                request(),
                &mut || {},
                &cancel,
                &mut || {
                    probes += 1;
                    if probes == 101 {
                        cancel.store(true, Ordering::SeqCst);
                    }
                },
            )
            .unwrap_err();

            assert!(is_cancelled(&error));
            assert_eq!(probes, 101);
        }

        #[test]
        fn cancellation_during_finalization_stops_without_publishing_a_report() {
            let data_dir = TempDir::new().unwrap();
            let store = Store::open(data_dir.path()).unwrap();
            publish_evidence_with_turns(&store, "large", 120, PublishedEvidence::Ready, 100);
            let cancel = AtomicBool::new(false);
            let mut probes = 0;

            let error = reduce_with_state_on_snapshot(
                data_dir.path(),
                request(),
                &mut || {},
                &cancel,
                &mut || {
                    probes += 1;
                    if probes == 102 {
                        cancel.store(true, Ordering::SeqCst);
                    }
                },
            )
            .unwrap_err();

            assert!(is_cancelled(&error));
            assert_eq!(probes, 102);
        }
    }

    mod population {
        use super::*;

        // The evidence cohort now covers every AgentKind
        // (crate::agents::evidence_cohort), so a real scan never leaves a
        // session with no session_evidence row: `awaiting_provider_support`
        // trends to zero once the widened-cohort migration backfills every
        // existing session. The tests below still exercise DENOMINATOR_SQL's
        // partitioning directly, by passing a literal `evidence_agents` list
        // (`&[]` or `&["claude-code"]`) to `upsert_sessions`/`change_source`
        // rather than the real `evidence_cohort()`, so they stay a synthetic,
        // SQL-level pin of the bucket rather than a claim that production
        // still produces that row shape.

        #[test]
        fn denominator_partitions_non_cohort_rows_by_reason() {
            let data_dir = TempDir::new().unwrap();
            let store = Store::open(data_dir.path()).unwrap();

            publish_ready(&store, "processing", 120);
            change_source(&store, "processing", &["claude-code"]);
            let processing_claim = store
                .claim_next_evidence(&["claude-code"], 20, 600)
                .unwrap()
                .unwrap();
            assert_eq!(processing_claim.key.session_id, "processing");

            publish_ready(&store, "failed", 121);
            change_source(&store, "failed", &["claude-code"]);
            let failed_claim = store
                .claim_next_evidence(&["claude-code"], 20, 600)
                .unwrap()
                .unwrap();
            assert_eq!(failed_claim.key.session_id, "failed");
            assert!(
                store
                    .fail_evidence(
                        &failed_claim,
                        EvidenceFailure::Failed {
                            revisions: ProjectionRevisions {
                                parser_revision: PARSER_REVISION,
                                analyzer_revision: ANALYZER_REVISION,
                                metrics_schema_revision: METRICS_SCHEMA_REVISION,
                                evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
                            },
                        },
                        "synthetic terminal failure",
                    )
                    .unwrap()
            );

            publish_evidence(&store, "unsupported", 123, PublishedEvidence::Unsupported);

            publish_ready(&store, "stale", 124);
            change_source(&store, "stale", &[]);

            let unknown_active = session("unknown-active", 150, "sv1:unknown-active");
            let unknown_inactive = session("unknown-inactive", 99, "sv1:unknown-inactive");
            store
                .upsert_sessions(&[unknown_active, unknown_inactive], &[])
                .unwrap();

            publish_ready(&store, "ready", 125);

            publish_ready(&store, "pending", 122);
            change_source(&store, "pending", &["claude-code"]);

            let report = reduce_on_snapshot(
                data_dir.path(),
                request(),
                &mut || {},
                &AtomicBool::new(false),
            )
            .unwrap();
            let coverage = &report.context.coverage;

            assert_eq!(coverage.discovered, 7);
            assert_eq!(coverage.ready, 1);
            assert_eq!(coverage.pending, 1);
            assert_eq!(coverage.processing, 1);
            assert_eq!(coverage.failed, 1);
            assert_eq!(coverage.unsupported, 1);
            assert_eq!(coverage.stale, 1);
            assert_eq!(coverage.unknown_start, 1);
            assert_eq!(report.assessed_sessions, 1);
            assert!(coverage.is_consistent());

            // The ready session has no assistant work. Unused-source checks
            // exclude it instead of reporting a capability gap.
            let all_examples: Vec<_> = report
                .capability_gap_examples
                .values()
                .flat_map(|v| v.iter())
                .collect();
            assert!(all_examples.is_empty());

            // The cohort session carries no assistant turns, so the
            // zero-work denominator exclusion (CH-011b) keeps it out of
            // all three unused-source denominators:
            // Six capability-eligible detectors remain.
            assert_eq!(
                report
                    .detectors
                    .iter()
                    .map(|counts| counts.eligible)
                    .sum::<u64>(),
                6
            );
            // Missing effort and speed signals are unavailable outcomes,
            // not assessed results.
            assert_eq!(
                report
                    .detectors
                    .iter()
                    .map(|counts| counts.assessed)
                    .sum::<u64>(),
                4
            );
            assert!(report.detectors.iter().all(|counts| {
                counts.finding + counts.clean + counts.unavailable + counts.not_applicable == 1
            }));
        }

        #[test]
        fn stale_generation_evidence_never_joins_the_cohort() {
            // `denominator_partitions_non_cohort_rows_by_reason` above pins
            // the coverage bucket this row lands in. This test pins the
            // narrower claim I5 asks for: `COHORT_SQL` itself excludes it,
            // so the report's badge computation never runs on it.
            let data_dir = TempDir::new().unwrap();
            let store = Store::open(data_dir.path()).unwrap();

            publish_ready(&store, "current", 120);
            publish_ready(&store, "stale", 121);
            // The source grows a new generation and no requeue has run yet:
            // this row is still 'ready', with current revisions, but was
            // analyzed against the generation the source has since moved
            // past.
            change_source(&store, "stale", &[]);

            let report = reduce_on_snapshot(
                data_dir.path(),
                request(),
                &mut || {},
                &AtomicBool::new(false),
            )
            .unwrap();

            assert_eq!(report.context.coverage.discovered, 2);
            assert_eq!(report.context.coverage.ready, 1);
            assert_eq!(report.context.coverage.stale, 1);
            assert_eq!(
                report.assessed_sessions, 1,
                "evidence analyzed against a superseded source generation must not join the cohort"
            );
        }

        // Pi names the fixture agent, not a Pi-specific behavior:
        // `reconcile_evidence_revisions(&crate::agents::evidence_cohort(), ..)`
        // now enrolls every agent's late-joining session the same way, since
        // the cohort covers all of them. This still exercises the real
        // `evidence_cohort()` (unlike the other `population` tests above),
        // so it pins that the widened cohort keeps moving a session with no
        // evidence row out of `awaiting_provider_support`.
        #[test]
        fn pi_backfill_moves_awaiting_support_into_the_pending_queue() {
            let data_dir = TempDir::new().unwrap();
            let store = Store::open(data_dir.path()).unwrap();
            let mut pi = session("pi-backfill", 120, "sv1:pi-backfill");
            pi.key.agent = "pi".to_owned();
            pi.source_label = "/synthetic/pi-backfill.jsonl".to_owned();
            store
                .upsert_sessions(std::slice::from_ref(&pi), &[])
                .unwrap();
            store
                .save_analysis(
                    &AnalysisRecord {
                        key: pi.key.clone(),
                        model_breakdown_json: "{}".to_owned(),
                        pricing_breakdown_json: "{}".to_owned(),
                        inclusive_models_json: "[]".to_owned(),
                        initial_context_json: None,
                        source_summaries_json: None,
                        provider_hints_json: None,
                        source_fingerprint: "sv1:pi-backfill".to_owned(),
                        pricing_generation: 1,
                        analyzed_generation: 1,
                        parser_revision: PARSER_REVISION,
                        analyzer_revision: ANALYZER_REVISION,
                        metrics_schema_revision: METRICS_SCHEMA_REVISION,
                    },
                    Some(120),
                )
                .unwrap();

            let before = reduce_on_snapshot(
                data_dir.path(),
                request(),
                &mut || {},
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(before.context.coverage.pending, 1);
            assert_eq!(before.context.coverage.awaiting_provider_support, 1);

            assert_eq!(
                store
                    .reconcile_evidence_revisions(
                        &crate::agents::evidence_cohort(),
                        crate::analysis::projection_revisions(),
                    )
                    .unwrap(),
                1
            );
            let after = reduce_on_snapshot(
                data_dir.path(),
                request(),
                &mut || {},
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(after.context.coverage.pending, 1);
            assert_eq!(after.context.coverage.awaiting_provider_support, 0);
        }

        #[test]
        fn unknown_start_rows_split_on_in_window_activity() {
            // Each case seeds one row alone, so a reversed activity predicate
            // cannot pass by counting the other row.
            let active_dir = TempDir::new().unwrap();
            let active_store = Store::open(active_dir.path()).unwrap();
            let active = session("unknown-active", 150, "sv1:unknown-active");
            active_store.upsert_sessions(&[active], &[]).unwrap();

            let report = reduce_on_snapshot(
                active_dir.path(),
                request(),
                &mut || {},
                &AtomicBool::new(false),
            )
            .unwrap();
            let coverage = &report.context.coverage;
            assert_eq!(coverage.discovered, 1);
            assert_eq!(coverage.unknown_start, 1);
            assert_eq!(report.assessed_sessions, 0);
            assert!(coverage.is_consistent());
            assert_eq!(
                report
                    .detectors
                    .iter()
                    .map(|counts| counts.eligible + counts.assessed)
                    .sum::<u64>(),
                0
            );

            let inactive_dir = TempDir::new().unwrap();
            let inactive_store = Store::open(inactive_dir.path()).unwrap();
            let inactive = session("unknown-inactive", 99, "sv1:unknown-inactive");
            inactive_store.upsert_sessions(&[inactive], &[]).unwrap();

            let report = reduce_on_snapshot(
                inactive_dir.path(),
                request(),
                &mut || {},
                &AtomicBool::new(false),
            )
            .unwrap();
            let coverage = &report.context.coverage;
            assert_eq!(coverage.discovered, 0);
            assert_eq!(coverage.unknown_start, 0);
            assert_eq!(report.assessed_sessions, 0);
            assert!(coverage.is_consistent());
            assert_eq!(
                report
                    .detectors
                    .iter()
                    .map(|counts| counts.eligible + counts.assessed)
                    .sum::<u64>(),
                0
            );
        }

        #[test]
        fn report_excludes_sessions_from_another_environment() {
            let data_dir = TempDir::new().unwrap();
            let store = Store::open(data_dir.path()).unwrap();
            let mut other = session("other-environment", 150, "sv1:other-environment");
            other.key.environment_key = "wsl:ubuntu".to_owned();
            store.upsert_sessions(&[other], &[]).unwrap();

            let report = reduce_on_snapshot(
                data_dir.path(),
                request(),
                &mut || {},
                &AtomicBool::new(false),
            )
            .unwrap();

            assert_eq!(report.context.coverage.discovered, 0);
            assert_eq!(report.assessed_sessions, 0);
            assert!(
                report
                    .detectors
                    .iter()
                    .all(|counts| { counts.eligible == 0 && counts.assessed == 0 })
            );
        }
    }

    #[test]
    fn report_keeps_gap_maps_and_examples_bounded() {
        let data_dir = TempDir::new().unwrap();
        let store = Store::open(data_dir.path()).unwrap();
        for index in 0..10 {
            publish_ready(&store, &format!("ready-{index}"), 120 + index);
        }

        let report = reduce_on_snapshot(
            data_dir.path(),
            request(),
            &mut || {},
            &AtomicBool::new(false),
        )
        .unwrap();

        assert!(report.capability_gaps.len() <= 9);
        assert!(report.capability_gap_examples.len() <= 9);
        assert!(
            report
                .capability_gap_examples
                .values()
                .map(Vec::len)
                .sum::<usize>()
                <= 9 * antiburn_local::insights::MAX_EXAMPLES_PER_DETECTOR
        );
    }
}
