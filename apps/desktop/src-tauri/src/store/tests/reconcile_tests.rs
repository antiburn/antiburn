use super::*;

#[test]
fn reconciling_backfills_existing_pi_sessions_with_current_revisions() {
    let store = store();
    let mut pi = session("pi-upgrade-enrollment", 1_000);
    pi.key.agent = "pi".to_owned();
    pi.source_label = "/synthetic/pi-upgrade-enrollment.jsonl".to_owned();
    pi.source_fingerprint = Some("sv1:synthetic".to_owned());
    store
        .upsert_sessions(std::slice::from_ref(&pi), &[])
        .unwrap();
    assert!(store.evidence(&pi.key).unwrap().is_none());

    assert_eq!(
        store
            .reconcile_evidence_revisions(
                &crate::agents::evidence_cohort(),
                crate::analysis::projection_revisions(),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        store.evidence(&pi.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
    assert_eq!(
        crate::analysis::projection_revisions(),
        ProjectionRevisions {
            parser_revision: 17,
            analyzer_revision: 16,
            metrics_schema_revision: 3,
            evidence_schema_revision: 12,
        }
    );
}

#[test]
fn a_revision_change_requeues_session_evidence_without_touching_the_generation() {
    let store = store();
    let record = seed_current_session_evidence(&store, "revision-requeue");
    // Set nonzero retry state first. Then the test can show that
    // reconcile resets the state, not that it was already zero.
    store
        .lock()
        .execute(
            "UPDATE session_evidence
                SET retry_count = 3, last_error = 'stale attempt',
                    next_attempt_at_epoch = 500
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();
    let before = store.session_source_state(&record.key).unwrap().unwrap();
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

    let evidence = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(evidence.status, EvidenceStatus::Pending);
    assert_eq!(evidence.evidence_json.as_deref(), Some("{\"groups\":[]}"));
    assert_eq!(evidence.retry_count, 0);
    assert_eq!(evidence.last_error, None);
    assert_eq!(evidence.next_attempt_at_epoch, None);
    assert_eq!(
        store.session_source_state(&record.key).unwrap().unwrap(),
        before
    );
}

// The tests below pin `reconcile_evidence_revisions`'s individual requeue
// decisions. Each test isolates one arm of its SQL. A future edit that
// changes requeue behavior then fails one named test.

#[test]
fn a_parser_revision_mismatch_alone_requeues_and_resets_retry_state() {
    let store = store();
    let record = seed_current_session_evidence(&store, "parser-revision-requeue");
    // Set nonzero retry state first. Then the test can show that
    // reconcile resets the state, not that it was already zero.
    store
        .lock()
        .execute(
            "UPDATE session_evidence
                SET retry_count = 3, last_error = 'stale attempt',
                    next_attempt_at_epoch = 500
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();
    let before = store.session_source_state(&record.key).unwrap().unwrap();
    let revisions = ProjectionRevisions {
        parser_revision: 2,
        ..projection_revisions()
    };

    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], revisions)
            .unwrap(),
        1
    );

    let evidence = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(evidence.status, EvidenceStatus::Pending);
    assert_eq!(evidence.retry_count, 0);
    assert_eq!(evidence.last_error, None);
    assert_eq!(evidence.next_attempt_at_epoch, None);
    assert_eq!(
        store.session_source_state(&record.key).unwrap().unwrap(),
        before
    );
}

#[test]
fn an_analyzer_revision_mismatch_alone_requeues_session_evidence() {
    let store = store();
    let record = seed_current_session_evidence(&store, "analyzer-revision-requeue");
    let before = store.session_source_state(&record.key).unwrap().unwrap();
    let revisions = ProjectionRevisions {
        analyzer_revision: 2,
        ..projection_revisions()
    };

    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], revisions)
            .unwrap(),
        1
    );

    let evidence = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(evidence.status, EvidenceStatus::Pending);
    assert_eq!(evidence.evidence_json.as_deref(), Some("{\"groups\":[]}"));
    assert_eq!(
        store.session_source_state(&record.key).unwrap().unwrap(),
        before
    );
}

#[test]
fn a_stale_metrics_schema_revision_in_session_analysis_requeues_a_current_row() {
    let store = store();
    let record = seed_current_session_evidence(&store, "metrics-stale-requeue");
    // The evidence row's own columns, and `analyzed_generation`, are current.
    // Only the joined `session_analysis` row's `metrics_schema_revision` is
    // stale, so this isolates the missing-analysis requeue arm.
    store
        .lock()
        .execute(
            "UPDATE session_analysis SET metrics_schema_revision = 0
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();

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
}

#[test]
fn a_missing_session_analysis_row_requeues_an_otherwise_current_evidence_row() {
    let store = store();
    let record = seed_ready_evidence_row(&store, "metrics-missing-requeue");

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
}

#[test]
fn an_analyzed_generation_behind_the_session_requeues_even_when_revisions_match() {
    let store = store();
    let record = seed_current_session_evidence(&store, "generation-behind-requeue");
    // Increase the session's generation directly. This bypasses
    // `upsert_sessions` and tests only the generation check in
    // `reconcile_evidence_revisions`.
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = 2
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();

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
}

#[test]
fn a_failed_row_with_a_revision_mismatch_requeues_despite_its_status() {
    let store = store();
    let record = seed_current_session_evidence(&store, "failed-revision-requeue");
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'failed', last_error = 'earlier failure'
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();
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

    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
}

#[test]
fn a_failed_row_current_except_for_a_missing_analysis_row_does_not_requeue() {
    let store = store();
    let record = seed_ready_evidence_row(&store, "failed-missing-analysis-guard");
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'failed', last_error = 'earlier failure'
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
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
fn an_unsupported_row_with_a_revision_mismatch_requeues_despite_its_status() {
    let store = store();
    let record = seed_current_session_evidence(&store, "unsupported-revision-requeue");
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'unsupported'
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();
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

    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
}

#[test]
fn an_unsupported_row_current_except_for_a_missing_analysis_row_does_not_requeue() {
    let store = store();
    let record = seed_ready_evidence_row(&store, "unsupported-missing-analysis-guard");
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'unsupported'
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
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
fn a_stale_row_for_an_agent_outside_the_list_is_neither_enrolled_nor_requeued() {
    let store = store();
    let claude = seed_current_session_evidence(&store, "agent-filter-claude");
    let mut codex = session("agent-filter-codex", 1_000);
    codex.key.agent = "codex".into();
    codex.source_fingerprint = Some("sv1:current".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&codex),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .save_analysis(
            &projection_record(codex.key.clone(), "sv1:current", 1),
            None,
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session_evidence
                SET status = 'ready', analyzed_generation = 1,
                    processed_fingerprint = 'sv1:current', parser_revision = 1,
                    analyzer_revision = 1, evidence_schema_revision = 1,
                    evidence_json = '{\"groups\":[]}',
                    retry_count = 0, claim_fence = 4, analyzed_at_epoch = 900
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                codex.key.environment_key,
                codex.key.agent,
                codex.key.session_id
            ],
        )
        .unwrap();
    let codex_before = store.evidence(&codex.key).unwrap().unwrap();
    let mut codex_unseen = session("agent-filter-codex-unseen", 1_000);
    codex_unseen.key.agent = "codex".into();
    store
        .upsert_sessions(std::slice::from_ref(&codex_unseen), &[])
        .unwrap();
    // The mismatch spans two revisions, not one. An agent-filter
    // bug would still requeue this row.
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

    assert_eq!(
        store.evidence(&claude.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
    assert_eq!(store.evidence(&codex.key).unwrap().unwrap(), codex_before);
    assert!(store.evidence(&codex_unseen.key).unwrap().is_none());
}

#[test]
fn reconcile_counts_enrollment_and_requeue_together_in_one_call() {
    let store = store();
    let requeue_target = seed_current_session_evidence(&store, "combined-requeue");
    let mut new_session = session("combined-enroll", 1_000);
    new_session.source_fingerprint = Some("sv1:new".into());
    store
        .upsert_sessions(std::slice::from_ref(&new_session), &[])
        .unwrap();
    assert!(store.evidence(&new_session.key).unwrap().is_none());
    let revisions = ProjectionRevisions {
        parser_revision: 2,
        ..projection_revisions()
    };

    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], revisions)
            .unwrap(),
        2
    );

    assert!(store.evidence(&new_session.key).unwrap().is_some());
    assert_eq!(
        store.evidence(&requeue_target.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
}
