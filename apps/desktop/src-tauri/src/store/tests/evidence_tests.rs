use super::*;

#[test]
fn the_generation_increments_only_when_the_fingerprint_changes() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "generation");
    let mut record = session("generation", 1_000);
    record.source_fingerprint = Some("sv1:first".to_string());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let first = store
        .session_source_state(&key)
        .unwrap()
        .expect("source state");
    assert_eq!(first.source_generation, 1);

    record.source_fingerprint = Some("sv1:second".to_string());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let second = store
        .session_source_state(&key)
        .unwrap()
        .expect("source state");
    assert_eq!(second.source_generation, 2);
    assert_eq!(second.source_fingerprint.as_deref(), Some("sv1:second"));

    record.source_fingerprint = None;
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    let unreadable = store
        .session_source_state(&key)
        .unwrap()
        .expect("source state");
    assert_eq!(unreadable, second);
}

#[test]
fn a_new_source_generation_marks_session_evidence_pending() {
    let store = store();
    let mut record = seed_current_session_evidence(&store, "new-generation-evidence");
    record.source_fingerprint = Some("sv1:changed".into());

    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    assert_eq!(
        store
            .session_source_state(&record.key)
            .unwrap()
            .unwrap()
            .source_generation,
        2
    );
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
}

#[test]
fn a_changed_child_activity_cursor_requeues_the_same_parent_generation() {
    let store = store();
    let mut record = seed_current_session_evidence(&store, "changed-child-cursor");
    record.activity_cursor = "parent-and-child-v2".to_owned();

    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    assert_eq!(
        store
            .session_source_state(&record.key)
            .unwrap()
            .unwrap()
            .source_generation,
        1
    );
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
}

#[test]
fn marking_session_evidence_pending_keeps_the_last_completed_payload() {
    let store = store();
    let mut record = seed_current_session_evidence(&store, "preserved-evidence");
    let before = store.evidence(&record.key).unwrap().unwrap();
    record.source_fingerprint = Some("sv1:changed".into());

    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();

    let after = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(after.status, EvidenceStatus::Pending);
    assert_eq!(after.evidence_json, before.evidence_json);
    assert_eq!(after.analyzed_generation, before.analyzed_generation);
    assert_eq!(after.processed_fingerprint, before.processed_fingerprint);
    assert_eq!(after.parser_revision, before.parser_revision);
    assert_eq!(after.analyzer_revision, before.analyzer_revision);
    assert_eq!(
        after.evidence_schema_revision,
        before.evidence_schema_revision
    );
    assert_eq!(after.claim_fence, before.claim_fence);
    assert_eq!(after.retry_count, 0);
    assert_eq!(after.next_attempt_at_epoch, None);
    assert_eq!(after.last_error, None);
}

#[test]
fn an_unchanged_fingerprint_leaves_a_ready_session_evidence_row_alone() {
    assert_unchanged_session_evidence("unchanged-ready", "ready", 0, None, None, None, None);
}

#[test]
fn an_unchanged_fingerprint_leaves_a_processing_session_evidence_claim_alone() {
    assert_unchanged_session_evidence(
        "unchanged-processing",
        "processing",
        2,
        Some(100),
        Some(200),
        None,
        None,
    );
}

#[test]
fn an_unchanged_fingerprint_keeps_session_evidence_retry_backoff() {
    assert_unchanged_session_evidence(
        "unchanged-backoff",
        "pending",
        3,
        None,
        None,
        Some(300),
        Some("try later"),
    );
}

#[test]
fn an_unchanged_fingerprint_leaves_a_failed_session_evidence_row_failed() {
    assert_unchanged_session_evidence(
        "unchanged-failed",
        "failed",
        4,
        None,
        None,
        None,
        Some("terminal"),
    );
}

#[test]
fn reconciling_enrolls_a_session_evidence_row_for_an_upgraded_session() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::super::schema::MIGRATIONS[..10] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute(
            "INSERT INTO session (
                 environment_key, agent, session_id, source_kind, source_label,
                 first_seen_at, last_seen_at, source_fingerprint, source_generation)
             VALUES ('native', 'claude-code', 'upgrade-enrollment', 'file',
                     '/home/avery/.claude/projects/demo/upgrade-enrollment.jsonl',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                     'sv1:current', 1)",
            [],
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 10).unwrap();
    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-evidence-enrollment-test").to_path_buf(),
    )
    .unwrap();
    let mut record = session("upgrade-enrollment", 1_000);
    record.source_fingerprint = Some("sv1:current".into());

    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(store.evidence(&record.key).unwrap().is_none());
    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], projection_revisions())
            .unwrap(),
        1
    );

    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
    assert_eq!(
        store
            .session_source_state(&record.key)
            .unwrap()
            .unwrap()
            .source_generation,
        1
    );
    assert_eq!(
        store
            .claim_next_evidence(&["claude-code"], 100, 60)
            .unwrap()
            .unwrap()
            .key,
        record.key
    );
}

/// Seam R3c: a worker pass publishes rows and per-source summaries, and
/// `analysis::analysis_from_rows` rebuilds the session-detail payload from
/// `store` alone. `record.source_label` names a transcript path that does
/// not exist on disk, so a payload here proves the replay path reads no
/// transcript — a live parse of that path would find nothing.
#[tokio::test]
async fn analysis_from_rows_serves_a_published_pass_without_reading_a_transcript() {
    let store = store();
    let mut record = session("rows-replay-parent", 1_000);
    record.source_fingerprint = Some("sv1:rows-replay-parent".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    // `published_evidence_pass` (used elsewhere in this module) writes its
    // rows to a standalone `MemoryTurnRowStore`, disconnected from `store`'s
    // own tables — fine for the evidence-JSON assertions those tests make,
    // but this test needs rows `store.published_turn_rows` can actually
    // read back, so the runner fences its rows into `store` itself, exactly
    // as `insights_worker::run_record_pass` does in production.
    let store_for_runner = store.clone();
    let runner = move |record: &SessionRecord,
                       _signal: crate::analysis::PassSignal,
                       claim_fence: i64| {
        let row_store: Arc<dyn TurnRowStore> = Arc::new(FencedTurnRowStore::new(
            store_for_runner.clone(),
            record.key.clone(),
            claim_fence,
        ));
        let mut pass = crate::analysis::evidence_pass_with_turn_rows(
            &[antiburn_local::analysis::SessionInput {
                agent: "claude".into(),
                session_id: record.key.session_id.clone(),
                source: antiburn_local::analysis::RawSource::Jsonl(
                    r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"start"}}
{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"id":"m1","role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":100,"cache_read_input_tokens":30000,"output_tokens":3},"content":[]}}
{"type":"user","timestamp":"2026-01-01T02:00:00Z","message":{"role":"user","content":"resume"}}
{"type":"assistant","timestamp":"2026-01-01T02:00:01Z","message":{"id":"m2","role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":100,"cache_read_input_tokens":5000,"cache_creation_input_tokens":25000,"output_tokens":3},"content":[]}}
"#
                    .into(),
                ),
                source_format: Default::default(),
                fork_parent_session_id: None,
            }],
            &|| false,
            Some(row_store),
        );
        pass.analysis.fingerprint = record
            .source_fingerprint
            .clone()
            .unwrap_or_else(|| crate::analysis::MISSING_FINGERPRINT.into());
        Box::pin(async move { pass }) as crate::insights_worker::PassFuture
    };
    assert!(
        crate::insights_worker::process_next(
            &store,
            &|| 1_100,
            &runner,
            &|_| {},
            &|_, _| {},
            &|| {}
        )
        .await
        .unwrap()
    );
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Ready
    );

    let replayed = crate::analysis::analysis_from_rows(
        &store,
        &record.key,
        &record.key.session_id,
        AgentKind::Claude,
    )
    .expect("a ready, published pass replays from rows");

    let metrics = replayed.metrics.as_ref().expect("replayed metrics");
    assert_eq!(metrics.cache_rehydration_count, 1);
    assert_eq!(metrics.cache_routing_miss_count, 0);
    assert_eq!(replayed.fingerprint, "sv1:rows-replay-parent");
    assert_eq!(replayed.cost, replayed.top_level_cost);
    assert!(replayed.source_path.is_none());
}

/// The published_fence carve-out this whole change exists for: an
/// actively-written session is requeued on nearly every fingerprint check,
/// which used to make `published_turn_rows` — and so `analysis_from_rows`
/// — return `None` on every drilldown open, however many passes had
/// already published. Requeuing flips `status` back to `pending` without
/// touching `published_fence`, so the last winning pass's rows and analysis
/// stay fully replayable while the fresh pass is only queued.
#[tokio::test]
async fn analysis_from_rows_still_serves_a_published_pass_after_a_requeue() {
    let store = store();
    let mut record = session("rows-replay-requeued", 1_000);
    record.source_fingerprint = Some("sv1:rows-replay-requeued".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    let store_for_runner = store.clone();
    let runner = move |record: &SessionRecord,
                       _signal: crate::analysis::PassSignal,
                       claim_fence: i64| {
        let row_store: Arc<dyn TurnRowStore> = Arc::new(FencedTurnRowStore::new(
            store_for_runner.clone(),
            record.key.clone(),
            claim_fence,
        ));
        let mut pass = crate::analysis::evidence_pass_with_turn_rows(
            &[antiburn_local::analysis::SessionInput {
                agent: "claude".into(),
                session_id: record.key.session_id.clone(),
                source: antiburn_local::analysis::RawSource::Jsonl(
                    r#"{"type":"assistant","timestamp":100,"message":{"id":"m","role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":2,"output_tokens":3},"content":[]}}
"#
                    .into(),
                ),
                source_format: Default::default(),
                fork_parent_session_id: None,
            }],
            &|| false,
            Some(row_store),
        );
        pass.analysis.fingerprint = record
            .source_fingerprint
            .clone()
            .unwrap_or_else(|| crate::analysis::MISSING_FINGERPRINT.into());
        Box::pin(async move { pass }) as crate::insights_worker::PassFuture
    };
    assert!(
        crate::insights_worker::process_next(
            &store,
            &|| 1_100,
            &runner,
            &|_| {},
            &|_, _| {},
            &|| {}
        )
        .await
        .unwrap()
    );

    // The transcript grew: the drilldown's own nudge requeues the session
    // for a fresh pass, exactly as `commands::nudge_if_evidence_stale` does.
    store.requeue_session_evidence(&record.key).unwrap();
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );

    let replayed = crate::analysis::analysis_from_rows(
        &store,
        &record.key,
        &record.key.session_id,
        AgentKind::Claude,
    )
    .expect("a requeued session still replays its last published rows");

    assert!(replayed.metrics.is_some());
    assert_eq!(replayed.fingerprint, "sv1:rows-replay-requeued");
}

/// Before a worker pass has ever published anything, `published_fence` is
/// `NULL`, so `analysis_from_rows` finds no row set and returns `None` — the
/// command switch's own signal that this session's drilldown is still
/// pending. The worker fills the gap on its own next pass; the command no
/// longer re-parses the transcript in-process to answer this call. A claim
/// in flight over an *already-published* session is a different case —
/// see `published_turn_rows_serves_the_last_published_fence_while_a_newer_claim_is_in_flight`
/// in `publish_tests`.
#[test]
fn analysis_from_rows_returns_none_before_anything_was_ever_published() {
    let store = store();
    let (_, claim) = claimed_projection(&store, "rows-not-ready", 100, 60);
    assert_eq!(
        store.evidence(&claim.key).unwrap().unwrap().status,
        EvidenceStatus::Processing
    );

    assert!(
        crate::analysis::analysis_from_rows(
            &store,
            &claim.key,
            &claim.key.session_id,
            AgentKind::Claude,
        )
        .is_none()
    );
}

/// `published_turn_rows`' widening (R5 part 1) reaches `analysis_from_rows`
/// too: a pass published with status `unsupported` is exactly as replayable
/// as one published `ready`, because `Unsupported` is an insights verdict
/// (no detector was eligible), not a parse-quality one — the rows and the
/// `session_analysis` record a winning pass wrote are complete either way.
#[test]
fn analysis_from_rows_serves_a_pass_published_unsupported() {
    let store = store();
    let (mut record, claim) = claimed_projection(&store, "s1", 100, 60);
    // A row whose own `source_key` names the parent session, so
    // `metrics_by_source` groups it under `record.key.session_id` the way
    // `analysis_from_rows` expects for the parent entry.
    record.source_summaries_json = Some("{}".into());
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0)]).unwrap();
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Unsupported,
        crate::store::test_support::evidence_json(&claim.key),
    );
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );
    assert_eq!(
        store.evidence(&key).unwrap().unwrap().status,
        EvidenceStatus::Unsupported
    );

    let replayed =
        crate::analysis::analysis_from_rows(&store, &key, &key.session_id, AgentKind::Claude)
            .expect("an unsupported, published pass still replays from rows");

    assert!(replayed.metrics.is_some());
}

/// The drilldown's rows-replay path requeues a session whose stored
/// fingerprint no longer matches the live transcript
/// (`commands::nudge_if_evidence_stale`). This proves the underlying store
/// mechanism it calls: requeuing a `ready` row moves it back to `pending`
/// for the worker's next pass.
#[test]
fn requeue_session_evidence_marks_a_ready_session_pending() {
    let store = store();
    let record = seed_current_session_evidence(&store, "requeue-on-mismatch");
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Ready
    );

    store.requeue_session_evidence(&record.key).unwrap();

    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
}

#[tokio::test]
async fn reprocessing_a_revision_one_row_leaves_no_placeholder_in_stored_evidence_json() {
    let store = store();
    let record = seed_revision_one_placeholder(&store, "revision-placeholder-success");
    let revisions = ProjectionRevisions {
        evidence_schema_revision: 2,
        ..projection_revisions()
    };
    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], revisions)
            .unwrap(),
        1
    );
    let pending = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(pending.status, EvidenceStatus::Pending);
    assert_eq!(
        pending.evidence_json.as_deref(),
        Some("{\"state\":\"unimplemented\"}")
    );

    let runner = |record: &SessionRecord, _signal: crate::analysis::PassSignal, _: i64| {
        let pass = published_evidence_pass(record);
        Box::pin(async move { pass }) as crate::insights_worker::PassFuture
    };
    assert!(
        crate::insights_worker::process_next(
            &store,
            &|| 1_100,
            &runner,
            &|_| {},
            &|_, _| {},
            &|| {}
        )
        .await
        .unwrap()
    );

    let ready = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(ready.status, EvidenceStatus::Ready);
    assert_eq!(ready.evidence_schema_revision, Some(22));
    assert!(!ready.evidence_json.unwrap().contains("unimplemented"));
}

#[tokio::test]
async fn a_terminal_failure_clears_an_outdated_placeholder_payload() {
    let store = store();
    let record = seed_revision_one_placeholder(&store, "revision-placeholder-failed");
    let revisions = ProjectionRevisions {
        evidence_schema_revision: 2,
        ..projection_revisions()
    };
    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], revisions)
            .unwrap(),
        1
    );
    let runner = |_record: &SessionRecord, _signal: crate::analysis::PassSignal, _: i64| {
        Box::pin(async {
            crate::analysis::EvidencePass {
                analysis: crate::analysis::SessionAnalysis::unavailable(),
                evidence: None,
                outcome: crate::analysis::PassOutcome::SourceMissing,
                source_outcomes: Vec::new(),
            }
        }) as crate::insights_worker::PassFuture
    };
    assert!(
        crate::insights_worker::process_next(
            &store,
            &|| 1_100,
            &runner,
            &|_| {},
            &|_, _| {},
            &|| {}
        )
        .await
        .unwrap()
    );

    let failed = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(failed.status, EvidenceStatus::Failed);
    assert_eq!(failed.evidence_schema_revision, Some(22));
    assert!(failed.evidence_json.is_none());
}

#[test]
fn a_catalog_change_requeues_no_session_evidence() {
    let store = store();
    let record = seed_current_session_evidence(&store, "catalog-no-requeue");
    store
        .save_analysis(
            &AnalysisRecord {
                key: record.key.clone(),
                model_breakdown_json: "{}".into(),
                pricing_breakdown_json: "{}".into(),
                inclusive_models_json: "[]".into(),
                initial_context_json: None,
                source_summaries_json: None,
                provider_hints_json: None,
                source_fingerprint: "sv1:current".into(),
                pricing_generation: 2,
                analyzed_generation: 1,
                parser_revision: 1,
                analyzer_revision: 1,
                metrics_schema_revision: 1,
            },
            None,
        )
        .unwrap();
    let before = store.evidence(&record.key).unwrap().unwrap();

    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], projection_revisions())
            .unwrap(),
        0
    );
    assert_eq!(store.evidence(&record.key).unwrap().unwrap(), before);
}

#[test]
fn reconciling_session_evidence_skips_a_disabled_agent() {
    let store = store();
    let claude = session("enabled-evidence", 1_000);
    let mut codex = session("disabled-evidence", 1_000);
    codex.key.agent = "codex".into();
    store
        .upsert_sessions(
            &[claude.clone(), codex.clone()],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .lock()
        .execute("DELETE FROM session_evidence", [])
        .unwrap();

    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], projection_revisions())
            .unwrap(),
        1
    );
    assert!(store.evidence(&claude.key).unwrap().is_some());
    assert!(store.evidence(&codex.key).unwrap().is_none());
}

#[test]
fn every_agent_kind_is_in_the_evidence_cohort_and_gets_a_row() {
    // The cohort now covers every AgentKind (crate::agents::evidence_cohort
    // derives from AgentKind::ALL), so upserting a session for any slug
    // queues an evidence row. No agent is outside the cohort any more.
    let store = store();
    let cohort = crate::agents::evidence_cohort();
    let sessions: Vec<SessionRecord> = cohort
        .iter()
        .map(|slug| {
            let mut record = session(&format!("cohort-{slug}"), 1_000);
            record.key.agent = (*slug).to_string();
            record
        })
        .collect();

    store.upsert_sessions(&sessions, &cohort).unwrap();

    for record in &sessions {
        assert!(
            store.evidence(&record.key).unwrap().is_some(),
            "{} should get an evidence row",
            record.key.agent
        );
    }
}

#[test]
fn a_terminal_failure_survives_two_startup_reconciles() {
    let store = store();
    let (_, claim) = claimed_projection(&store, "terminal-reconcile", 100, 60);
    assert!(
        store
            .fail_evidence(
                &claim,
                EvidenceFailure::Failed {
                    revisions: projection_revisions(),
                },
                "source-missing",
            )
            .unwrap()
    );

    for _ in 0..2 {
        assert_eq!(
            store
                .reconcile_evidence_revisions(&["claude-code"], projection_revisions())
                .unwrap(),
            0
        );
        assert_eq!(
            store.evidence(&claim.key).unwrap().unwrap().status,
            EvidenceStatus::Failed
        );
        assert!(
            store
                .claim_next_evidence(&["claude-code"], 200, 60)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn a_returned_source_requeues_with_an_unchanged_fingerprint() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "returned-source", 100, 60);
    assert!(
        store
            .fail_evidence(
                &claim,
                EvidenceFailure::Failed {
                    revisions: projection_revisions(),
                },
                "source-missing",
            )
            .unwrap()
    );
    let generation = claim.source_generation;
    let mut returned = session("returned-source", 1_000);
    returned.source_fingerprint = Some(record.source_fingerprint);

    store
        .upsert_sessions(&[returned], &crate::agents::evidence_cohort())
        .unwrap();

    let evidence = store.evidence(&claim.key).unwrap().unwrap();
    assert_eq!(
        store
            .session_source_state(&claim.key)
            .unwrap()
            .unwrap()
            .source_generation,
        generation
    );
    assert_eq!(evidence.status, EvidenceStatus::Pending);
    assert_eq!(evidence.retry_count, 0);
}

#[test]
fn an_abandoned_processing_row_requeues_at_startup() {
    let store = store();
    let (_, claim) = claimed_projection(&store, "abandoned-startup", 100, 60);

    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], projection_revisions())
            .unwrap(),
        1
    );
    assert_eq!(
        store.evidence(&claim.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
}

#[test]
fn the_first_session_evidence_claim_excludes_a_second_claim() {
    let store = store();
    let mut record = session("exclusive-claim", 1_000);
    record.source_fingerprint = Some("sv1:exclusive".into());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();

    let first = store
        .claim_next_evidence(&["claude-code"], 100, 60)
        .unwrap()
        .unwrap();

    assert_eq!(first.claim_fence, 1);
    assert!(
        store
            .claim_next_evidence(&["claude-code"], 100, 60)
            .unwrap()
            .is_none()
    );
}

#[test]
fn reclaiming_an_abandoned_session_evidence_row_raises_the_fence() {
    let store = store();
    let mut record = session("reclaimed-claim", 1_000);
    record.source_fingerprint = Some("sv1:reclaim".into());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    let first = store
        .claim_next_evidence(&["claude-code"], 100, 10)
        .unwrap()
        .unwrap();

    let reclaimed = store
        .claim_next_evidence(&["claude-code"], 110, 10)
        .unwrap()
        .unwrap();

    assert_eq!(reclaimed.key, first.key);
    assert_eq!(reclaimed.source_generation, first.source_generation);
    assert_eq!(reclaimed.claim_fence, first.claim_fence + 1);
}

#[test]
fn a_late_session_evidence_transition_is_rejected() {
    let store = store();
    let mut record = session("late-transition", 1_000);
    record.source_fingerprint = Some("sv1:late".into());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    let first = store
        .claim_next_evidence(&["claude-code"], 100, 10)
        .unwrap()
        .unwrap();
    let current = store
        .claim_next_evidence(&["claude-code"], 110, 10)
        .unwrap()
        .unwrap();
    let before = store.evidence(&current.key).unwrap().unwrap();

    assert!(
        !store
            .fail_evidence(
                &first,
                EvidenceFailure::Failed {
                    revisions: projection_revisions()
                },
                "late"
            )
            .unwrap()
    );
    assert_eq!(store.evidence(&current.key).unwrap().unwrap(), before);
}

#[test]
fn a_stale_generation_rejects_a_session_evidence_lease_renewal() {
    let store = store();
    let mut record = session("stale-renewal", 1_000);
    record.source_fingerprint = Some("sv1:renewal".into());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    let claim = store
        .claim_next_evidence(&["claude-code"], 100, 60)
        .unwrap()
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();
    let before = store.evidence(&record.key).unwrap().unwrap();

    assert!(!store.renew_evidence_lease(&claim, 110, 60).unwrap());
    assert_eq!(store.evidence(&record.key).unwrap().unwrap(), before);
}

#[test]
fn a_stale_generation_rejects_a_session_evidence_failure() {
    let store = store();
    let mut record = session("stale-failure", 1_000);
    record.source_fingerprint = Some("sv1:failure".into());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    let claim = store
        .claim_next_evidence(&["claude-code"], 100, 60)
        .unwrap()
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();
    let before = store.evidence(&record.key).unwrap().unwrap();

    assert!(
        !store
            .fail_evidence(
                &claim,
                EvidenceFailure::Failed {
                    revisions: projection_revisions()
                },
                "stale"
            )
            .unwrap()
    );
    assert_eq!(store.evidence(&record.key).unwrap().unwrap(), before);
}

#[test]
fn a_session_evidence_claim_is_not_eligible_before_its_next_attempt() {
    let store = store();
    let mut record = session("delayed-claim", 1_000);
    record.source_fingerprint = Some("sv1:delayed".into());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET next_attempt_at_epoch = 200
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();

    assert!(
        store
            .claim_next_evidence(&["claude-code"], 199, 60)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .claim_next_evidence(&["claude-code"], 200, 60)
            .unwrap()
            .is_some()
    );
}

#[test]
fn failing_session_evidence_with_retry_returns_it_to_pending_with_backoff() {
    let store = store();
    let mut record = session("retry-failure", 1_000);
    record.source_fingerprint = Some("sv1:retry".into());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    let claim = store
        .claim_next_evidence(&["claude-code"], 100, 60)
        .unwrap()
        .unwrap();

    assert!(
        store
            .fail_evidence(
                &claim,
                EvidenceFailure::Retry {
                    next_attempt_at_epoch: 300,
                    counts_as_attempt: true,
                },
                "try later",
            )
            .unwrap()
    );

    let evidence = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(evidence.status, EvidenceStatus::Pending);
    assert_eq!(evidence.retry_count, 1);
    assert_eq!(evidence.next_attempt_at_epoch, Some(300));
    assert_eq!(evidence.last_error.as_deref(), Some("try later"));
    assert_eq!(evidence.claimed_at_epoch, None);
    assert_eq!(evidence.lease_expires_at_epoch, None);
    assert!(
        store
            .claim_next_evidence(&["claude-code"], 299, 60)
            .unwrap()
            .is_none()
    );
}

#[test]
fn failing_session_evidence_terminally_marks_it_failed() {
    let store = store();
    let mut record = session("terminal-failure", 1_000);
    record.source_fingerprint = Some("sv1:terminal".into());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    let claim = store
        .claim_next_evidence(&["claude-code"], 100, 60)
        .unwrap()
        .unwrap();

    assert!(
        store
            .fail_evidence(
                &claim,
                EvidenceFailure::Failed {
                    revisions: projection_revisions()
                },
                "terminal"
            )
            .unwrap()
    );

    let evidence = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(evidence.status, EvidenceStatus::Failed);
    assert_eq!(evidence.retry_count, 1);
    assert_eq!(evidence.next_attempt_at_epoch, None);
    assert_eq!(evidence.last_error.as_deref(), Some("terminal"));
    assert_ne!(evidence.status, EvidenceStatus::Ready);
    assert_ne!(evidence.status, EvidenceStatus::Unsupported);
}

#[test]
fn publishing_session_evidence_writes_both_projections_and_the_start_time() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "publish-both", 100, 60);
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Unsupported,
        crate::store::test_support::evidence_json(&claim.key),
    );

    assert!(
        store
            .publish_projections(&record, Some(50), &completion, &[], &[])
            .unwrap()
    );

    assert_eq!(store.analysis(&record.key).unwrap(), Some(record.clone()));
    assert_eq!(
        store
            .session_source_state(&record.key)
            .unwrap()
            .unwrap()
            .started_at_epoch,
        Some(50)
    );
    let evidence = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(evidence.status, EvidenceStatus::Unsupported);
    assert_eq!(
        evidence.analyzed_generation,
        Some(record.analyzed_generation)
    );
    assert_eq!(
        evidence.processed_fingerprint.as_deref(),
        Some(record.source_fingerprint.as_str())
    );
    assert_eq!(evidence.parser_revision, Some(record.parser_revision));
    assert_eq!(evidence.analyzer_revision, Some(record.analyzer_revision));
    assert_eq!(evidence.evidence_schema_revision, Some(1));
    assert_eq!(evidence.retry_count, 0);
    assert_eq!(evidence.last_error, None);
    assert_eq!(evidence.claimed_at_epoch, None);
    assert_eq!(evidence.lease_expires_at_epoch, None);
    assert_eq!(evidence.next_attempt_at_epoch, None);
    assert!(evidence.analyzed_at_epoch.is_some());
}

#[test]
fn published_session_evidence_and_analysis_describe_the_same_pass() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "same-pass", 100, 60);
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );

    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );

    let analysis = store.analysis(&record.key).unwrap().unwrap();
    let evidence = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(
        evidence.analyzed_generation,
        Some(analysis.analyzed_generation)
    );
    assert_eq!(
        evidence.processed_fingerprint,
        Some(analysis.source_fingerprint)
    );
    assert_eq!(evidence.parser_revision, Some(analysis.parser_revision));
    assert_eq!(evidence.analyzer_revision, Some(analysis.analyzer_revision));
}

#[test]
fn a_stale_generation_publishes_no_session_evidence_and_no_analysis() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "stale-publish-generation", 100, 60);
    let sentinel = projection_record(record.key.clone(), "sv1:sentinel", 0);
    store.save_analysis(&sentinel, Some(77)).unwrap();
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();
    let source_before = store.session_source_state(&record.key).unwrap().unwrap();
    let analysis_before = store.analysis(&record.key).unwrap().unwrap();
    let evidence_before = store.evidence(&record.key).unwrap().unwrap();
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );

    assert!(
        !store
            .publish_projections(&record, Some(88), &completion, &[], &[])
            .unwrap()
    );

    assert_eq!(
        store.session_source_state(&record.key).unwrap().unwrap(),
        source_before
    );
    assert_eq!(
        store.analysis(&record.key).unwrap().unwrap(),
        analysis_before
    );
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap(),
        evidence_before
    );
}

#[test]
fn a_stale_fence_publishes_no_session_evidence_and_no_analysis() {
    let store = store();
    let (record, first_claim) = claimed_projection(&store, "stale-publish-fence", 100, 10);
    let current_claim = store
        .claim_next_evidence(&["claude-code"], 110, 60)
        .unwrap()
        .unwrap();
    assert!(current_claim.claim_fence > first_claim.claim_fence);
    let sentinel = projection_record(record.key.clone(), "sv1:sentinel", 0);
    store.save_analysis(&sentinel, Some(77)).unwrap();
    let source_before = store.session_source_state(&record.key).unwrap().unwrap();
    let analysis_before = store.analysis(&record.key).unwrap().unwrap();
    let evidence_before = store.evidence(&record.key).unwrap().unwrap();
    let completion = evidence_completion(
        &first_claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&first_claim.key),
    );

    assert!(
        !store
            .publish_projections(&record, Some(88), &completion, &[], &[])
            .unwrap()
    );

    assert_eq!(
        store.session_source_state(&record.key).unwrap().unwrap(),
        source_before
    );
    assert_eq!(
        store.analysis(&record.key).unwrap().unwrap(),
        analysis_before
    );
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap(),
        evidence_before
    );
}

#[test]
fn a_stale_claim_cannot_change_projections_or_relations() {
    let store = store();
    let (record, first_claim) = claimed_projection(&store, "stale-all-projections", 100, 10);
    let current_claim = store
        .claim_next_evidence(&["claude-code"], 110, 60)
        .unwrap()
        .unwrap();
    let current_completion = evidence_completion(
        &current_claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&current_claim.key),
    );
    let current_relations = [RelationRecord {
        kind: RelationKind::Subagent,
        related_id: "new-child".into(),
        label: Some("New child".into()),
    }];
    assert!(
        store
            .publish_projections(&record, None, &current_completion, &current_relations, &[])
            .unwrap()
    );
    let analysis_before = store.analysis(&record.key).unwrap();
    let evidence_before = store.evidence(&record.key).unwrap();
    let relations_before = store.relations(&record.key).unwrap();
    let mut stale_record = record.clone();
    stale_record.model_breakdown_json = "{\"old\":true}".into();
    let stale_completion = evidence_completion(
        &first_claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&first_claim.key),
    );
    let stale_relations = [RelationRecord {
        kind: RelationKind::Subagent,
        related_id: "old-child".into(),
        label: Some("Old child".into()),
    }];

    assert!(
        !store
            .publish_projections(
                &stale_record,
                None,
                &stale_completion,
                &stale_relations,
                &[]
            )
            .unwrap()
    );
    assert_eq!(store.analysis(&record.key).unwrap(), analysis_before);
    assert_eq!(store.evidence(&record.key).unwrap(), evidence_before);
    assert_eq!(store.relations(&record.key).unwrap(), relations_before);
}

#[test]
fn publication_replaces_subagent_relations_in_one_transaction() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "relation-publication", 100, 60);
    store
        .replace_relations(
            &record.key,
            RelationKind::ForkParent,
            &[RelationRecord {
                kind: RelationKind::ForkParent,
                related_id: "fork-parent".into(),
                label: None,
            }],
        )
        .unwrap();
    store
        .replace_relations(
            &record.key,
            RelationKind::Subagent,
            &[RelationRecord {
                kind: RelationKind::Subagent,
                related_id: "old-child".into(),
                label: None,
            }],
        )
        .unwrap();
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );
    let new_relations = [RelationRecord {
        kind: RelationKind::Subagent,
        related_id: "new-child".into(),
        label: Some("New child".into()),
    }];

    assert!(
        store
            .publish_projections(&record, None, &completion, &new_relations, &[])
            .unwrap()
    );
    assert_eq!(
        store.relations(&record.key).unwrap(),
        vec![
            RelationRecord {
                kind: RelationKind::ForkParent,
                related_id: "fork-parent".into(),
                label: None,
            },
            new_relations[0].clone(),
        ]
    );
}

#[test]
fn deleting_a_session_removes_its_session_evidence() {
    let store = store();
    let mut record = session("delete-evidence", 1_000);
    record.source_fingerprint = Some("sv1:delete".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert!(store.evidence(&record.key).unwrap().is_some());

    assert!(store.delete_session(&record.key).unwrap().is_some());

    assert!(store.evidence(&record.key).unwrap().is_none());
}

#[test]
fn clearing_local_session_data_removes_every_session_evidence_row() {
    let store = store();
    let mut first = session("clear-evidence-one", 1_000);
    first.source_fingerprint = Some("sv1:clear-one".into());
    let mut second = session("clear-evidence-two", 1_000);
    second.source_fingerprint = Some("sv1:clear-two".into());
    store
        .upsert_sessions(&[first, second], &crate::agents::evidence_cohort())
        .unwrap();

    assert_eq!(store.clear_local_session_data().unwrap().0, 2);

    let count: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM session_evidence", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn a_session_evidence_payload_round_trips_through_the_store() {
    use antiburn_local::analysis::{
        EvidenceSource, SessionEvidence, SessionEvidenceAccumulator, SourceCapabilities,
        SourceKind, TurnFacts,
    };

    let store = store();
    let (record, claim) = claimed_projection(&store, "payload-round-trip", 100, 60);
    let payload = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude-code".into(),
        session_id: "payload-round-trip".into(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    })
    .evidence(&TurnFacts::default());
    let payload_json = serde_json::to_string(&payload).unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, payload_json);

    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );

    let stored_json = store
        .evidence(&record.key)
        .unwrap()
        .unwrap()
        .evidence_json
        .unwrap();
    let restored: SessionEvidence = serde_json::from_str(&stored_json).unwrap();
    assert_eq!(restored, payload);
}
