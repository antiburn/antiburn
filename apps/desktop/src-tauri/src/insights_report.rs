use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use antiburn_local::analysis::{
    ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION, SessionEvidence,
};
use antiburn_local::insights::{
    CoverageBucket, CoverageCounts, EfficiencyReport, EfficiencyReportAccumulator, ReportContext,
    ReportWindow,
};
use anyhow::{Context, Result, ensure};
use rusqlite::params;

use crate::store::open_read_only;

const REPORT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const CURRENT_EVIDENCE_PREDICATE: &str = "
    e.status = 'ready'
    AND NOT (e.analyzed_generation IS NOT s.source_generation)
    AND NOT (e.parser_revision IS NOT ?4)
    AND NOT (e.analyzer_revision IS NOT ?5)
    AND NOT (e.evidence_schema_revision IS NOT ?6)";

const DENOMINATOR_SQL: &str = "
SELECT bucket, COUNT(*), SUM(awaiting_provider_support)
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
                THEN 1 ELSE 0 END AS awaiting_provider_support
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
SELECT e.evidence_json
  FROM session s
  JOIN session_evidence e
    ON e.environment_key = s.environment_key
   AND e.agent = s.agent
   AND e.session_id = s.session_id
 WHERE s.environment_key = ?1
   AND s.started_at_epoch >= ?2
   AND s.started_at_epoch < ?3
   AND {current}
 ORDER BY s.started_at_epoch, s.session_id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportRequest {
    pub environment_key: String,
    pub window: ReportWindow,
    pub computed_at_epoch: i64,
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
) -> Result<EfficiencyReport> {
    tokio::task::spawn_blocking(move || reduce_on_snapshot(&data_dir, request, &mut || {}, &cancel))
        .await
        .context("report reduction task failed")?
}

fn reduce_on_snapshot(
    data_dir: &Path,
    request: ReportRequest,
    after_denominator: &mut dyn FnMut(),
    cancel: &AtomicBool,
) -> Result<EfficiencyReport> {
    ensure_not_cancelled(cancel)?;
    let connection = open_read_only(data_dir, REPORT_BUSY_TIMEOUT)?;
    let transaction = connection.unchecked_transaction()?;
    let mut coverage = CoverageCounts::default();
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
        }
    }
    ensure!(
        coverage.is_consistent(),
        "report coverage does not partition"
    );

    after_denominator();
    ensure_not_cancelled(cancel)?;

    let mut accumulator = EfficiencyReportAccumulator::new();
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
        ])?;
        while let Some(row) = rows.next()? {
            ensure_not_cancelled(cancel)?;
            let evidence_json: String = row.get(0)?;
            let evidence: SessionEvidence = serde_json::from_str(&evidence_json)
                .context("stored session evidence is invalid")?;
            accumulator.observe_session(evidence);
        }
    }

    let report = accumulator.finish(ReportContext {
        environment_key: request.environment_key,
        window: request.window,
        computed_at_epoch: request.computed_at_epoch,
        parser_revision: PARSER_REVISION,
        analyzer_revision: ANALYZER_REVISION,
        evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
        coverage,
    });
    ensure!(
        report.context.coverage.actively_growing <= report.context.coverage.ready,
        "actively growing coverage exceeds ready coverage"
    );
    drop(transaction);
    drop(connection);
    Ok(report)
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
        EVIDENCE_SCHEMA_REVISION, EvidenceSource, METRICS_SCHEMA_REVISION,
        SessionEvidenceAccumulator, SourceCapabilities, SourceKind, TurnFacts,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::store::{
        AnalysisRecord, EvidenceCompletion, EvidenceFailure, ProjectionRevisions,
        PublishedEvidence, SessionKey, SessionRecord, Store,
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
        let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude-code".to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence(&TurnFacts::default());
        let analysis = AnalysisRecord {
            key: session.key,
            model_breakdown_json: "{}".to_owned(),
            inclusive_models_json: "[]".to_owned(),
            initial_context_json: None,
            source_summaries_json: None,
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
            diagnostics_json: None,
        };
        assert!(
            store
                .publish_projections(&analysis, Some(started_at_epoch), &completion, &[])
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

            assert_eq!(report.context.coverage.discovered, 1);
            assert_eq!(report.assessed_sessions, 1);
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

            // Only the "ready" session is in the cohort, so every capability gap
            // example must name it. The Claude capability set blocks only
            // Unused Built-In Tools, so the example list cannot be empty.
            let all_examples: Vec<_> = report
                .capability_gap_examples
                .values()
                .flat_map(|v| v.iter())
                .collect();
            assert!(!all_examples.is_empty());
            for example in all_examples {
                assert_eq!(example.session_id, "ready");
            }

            // The cohort session carries no assistant turns, so the
            // zero-work denominator exclusion (CH-011b) keeps it out of
            // the Unused MCP Servers and Unused Skills denominators:
            // Eight capability-eligible detectors minus the two absence
            // detectors an idle session cannot support.
            assert_eq!(
                report
                    .detectors
                    .iter()
                    .map(|counts| counts.eligible)
                    .sum::<u64>(),
                6
            );
            assert_eq!(
                report
                    .detectors
                    .iter()
                    .map(|counts| counts.assessed)
                    .sum::<u64>(),
                6
            );
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
                        inclusive_models_json: "[]".to_owned(),
                        initial_context_json: None,
                        source_summaries_json: None,
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
