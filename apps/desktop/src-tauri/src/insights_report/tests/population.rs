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
    // Six metric checks and one content-check source remain eligible.
    assert_eq!(
        report
            .detectors
            .iter()
            .map(|counts| counts.eligible)
            .sum::<u64>(),
        7
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
