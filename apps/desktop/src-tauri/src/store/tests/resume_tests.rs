//! Store-level pinning tests for `source_resume` (continuous ingest, phase
//! 3b): the snapshot write inside a winning publish, the fence restamp and
//! replace `Store::publish_projections` runs per source, and the startup
//! purge of stale revisions. See "R4. Fence semantics", "R5. Snapshot
//! storage", and "R6. Invalidation" in the phase 3b design rules in
//! `docs/plans/continuous-session-ingest.md`.

use super::*;
use antiburn_local::analysis::{ContentKind, ContentPart, count_turn_content_rows};

fn sample_resume(source_fingerprint: &str) -> StoredResume {
    StoredResume {
        snapshot: vec![1, 2, 3],
        snapshot_revision: 1,
        parser_revision: 1,
        analyzer_revision: 1,
        metrics_schema_revision: 1,
        evidence_schema_revision: 1,
        coverage_schema_revision: 1,
        source_fingerprint: source_fingerprint.to_owned(),
    }
}

/// [`turn_row`] with `source_key` and `thread_id` overridden, so a test can
/// build rows for more than one source under the same session.
fn turn_row_for(source_key: &str, turn_index: u64) -> TurnRow {
    TurnRow {
        source_key: source_key.to_owned(),
        thread_id: source_key.to_owned(),
        ..turn_row(turn_index)
    }
}

fn turn_row_with_content_for(source_key: &str, turn_index: u64, text: &str) -> TurnRow {
    TurnRow {
        content: vec![ContentPart::new(ContentKind::AssistantText, text)],
        ..turn_row_for(source_key, turn_index)
    }
}

fn publish_single_full_source(store: &Store, source_key: &str) -> (SessionKey, i64) {
    let (record, claim) = claimed_projection(store, source_key, 100, 60);
    let key = record.key.clone();
    FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence)
        .write_turn_rows(&[turn_row_with_content_for(source_key, 0, "first content")])
        .unwrap();
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );
    let sources = [SourcePublishOutcome {
        source_key: source_key.into(),
        mode: SourcePublishMode::Full,
        resume: Some(sample_resume("fp-first")),
    }];
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &sources)
            .unwrap()
    );
    (key, claim.claim_fence)
}

fn claim_source_with_next_row(
    store: &Store,
    key: &SessionKey,
    source_key: &str,
) -> (AnalysisRecord, EvidenceClaim, EvidenceCompletion) {
    mark_evidence_pending_in(&store.lock(), key).unwrap();
    let claim = store
        .claim_next_evidence(&["claude-code"], 200, 60)
        .unwrap()
        .expect("reclaimable");
    let record = projection_record(
        key.clone(),
        &format!("sv1:{source_key}-next"),
        claim.source_generation,
    );
    FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence)
        .write_turn_rows(&[turn_row_with_content_for(source_key, 1, "next content")])
        .unwrap();
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );
    (record, claim, completion)
}

#[test]
fn a_winning_publish_writes_the_resume_snapshot_named_for_its_source() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "resume-write", 100, 60);
    let key = record.key.clone();
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );
    let sources = [SourcePublishOutcome {
        source_key: "resume-write".into(),
        mode: SourcePublishMode::Full,
        resume: Some(sample_resume("fp1")),
    }];

    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &sources)
            .unwrap()
    );

    let connection = store.lock();
    assert_eq!(
        query_source_resume(&connection, &turn_session_key(&key), "resume-write").unwrap(),
        Some(sample_resume("fp1"))
    );
}

#[test]
fn a_named_full_source_keeps_its_rows_on_the_first_publish() {
    let store = store();
    let (key, _) = publish_single_full_source(&store, "resume-first-full");

    let published = store.published_turn_rows(&key).unwrap().expect("ready");
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].source_key, "resume-first-full");
}

#[test]
fn a_source_with_no_resume_has_its_stored_snapshot_dropped() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "resume-drop", 100, 60);
    let key = record.key.clone();
    {
        let connection = store.lock();
        insert_source_resume(
            &connection,
            &turn_session_key(&key),
            "resume-drop",
            &sample_resume("fp1"),
        )
        .unwrap();
    }
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );
    let sources = [SourcePublishOutcome {
        source_key: "resume-drop".into(),
        mode: SourcePublishMode::Full,
        resume: None,
    }];

    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &sources)
            .unwrap()
    );

    let connection = store.lock();
    assert_eq!(
        query_source_resume(&connection, &turn_session_key(&key), "resume-drop").unwrap(),
        None,
        "an adapter that returned no AdapterResume must not leave a stale snapshot behind"
    );
}

#[test]
fn a_lost_publish_race_writes_no_resume_snapshot() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "resume-lost-race", 100, 60);
    let key = record.key.clone();
    // Bumping the source generation makes the fenced UPDATE inside
    // `publish_projections` affect zero rows — the same "lost the race"
    // shape the turn-row and coverage-record equivalents of this test use.
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
        )
        .unwrap();
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );
    let sources = [SourcePublishOutcome {
        source_key: "resume-lost-race".into(),
        mode: SourcePublishMode::Full,
        resume: Some(sample_resume("fp1")),
    }];

    assert!(
        !store
            .publish_projections(&record, None, &completion, &[], &sources)
            .unwrap()
    );

    let connection = store.lock();
    assert_eq!(
        query_source_resume(&connection, &turn_session_key(&key), "resume-lost-race").unwrap(),
        None,
        "a losing pass must never write a resume snapshot"
    );
}

#[test]
fn a_resumed_source_re_stamps_only_its_own_appended_rows() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "resume-restamp", 100, 60);
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer
        .write_turn_rows(&[
            turn_row_for("resume-restamp", 0),
            turn_row_for("child-1", 0),
        ])
        .unwrap();
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
    let first_published_fence = store.evidence(&key).unwrap().unwrap().published_fence;

    // Second pass: the session resumes. Only the parent appends one new row
    // under the new claim fence; the child writes nothing new but is still
    // named as Resumed — a still-present, unchanged source a resume-aware
    // adapter names on every visit regardless of new row count.
    mark_evidence_pending_in(&store.lock(), &key).unwrap();
    let next_claim = store
        .claim_next_evidence(&["claude-code"], 200, 60)
        .unwrap()
        .expect("reclaimable");
    let next_record = projection_record(
        key.clone(),
        "sv1:resume-restamp",
        next_claim.source_generation,
    );
    let next_writer = FencedTurnRowStore::new(store.clone(), key.clone(), next_claim.claim_fence);
    next_writer
        .write_turn_rows(&[turn_row_for("resume-restamp", 1)])
        .unwrap();
    let next_completion = evidence_completion(
        &next_claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&next_claim.key),
    );
    let sources = [
        SourcePublishOutcome {
            source_key: "resume-restamp".into(),
            mode: SourcePublishMode::Resumed,
            resume: None,
        },
        SourcePublishOutcome {
            source_key: "child-1".into(),
            mode: SourcePublishMode::Resumed,
            resume: None,
        },
    ];

    assert!(
        store
            .publish_projections(&next_record, None, &next_completion, &[], &sources)
            .unwrap()
    );

    assert_eq!(
        store.evidence(&key).unwrap().unwrap().published_fence,
        first_published_fence,
        "a resumed pass must not move published_fence"
    );
    let published = store.published_turn_rows(&key).unwrap().expect("ready");
    let parent_rows: Vec<_> = published
        .iter()
        .filter(|row| row.source_key == "resume-restamp")
        .collect();
    let child_rows: Vec<_> = published
        .iter()
        .filter(|row| row.source_key == "child-1")
        .collect();
    assert_eq!(
        parent_rows.len(),
        2,
        "the parent's original row and its appended row must both survive"
    );
    assert_eq!(
        child_rows.len(),
        1,
        "the untouched child's row must survive, not be deleted"
    );
}

#[test]
fn a_full_read_source_replaces_only_its_own_published_rows() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "resume-full-replace", 100, 60);
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer
        .write_turn_rows(&[
            turn_row_with_content_for("resume-full-replace", 0, "old parent zero"),
            turn_row_with_content_for("resume-full-replace", 1, "old parent one"),
            turn_row_with_content_for("child-1", 0, "unchanged child"),
            turn_row_with_content_for("empty-full", 0, "removed full source"),
            turn_row_with_content_for("unnamed-full", 0, "old unnamed source"),
        ])
        .unwrap();
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
    let first_published_fence = store.evidence(&key).unwrap().unwrap().published_fence;

    // Second pass: the parent forces a full read (a tail rewrite, say) and
    // rewrites its rows from scratch; the child writes nothing new but is
    // still named as Resumed — a still-present, unchanged source a
    // resume-aware adapter names on every visit regardless of new row
    // count.
    mark_evidence_pending_in(&store.lock(), &key).unwrap();
    let next_claim = store
        .claim_next_evidence(&["claude-code"], 200, 60)
        .unwrap()
        .expect("reclaimable");
    let next_record = projection_record(
        key.clone(),
        "sv1:resume-full-replace",
        next_claim.source_generation,
    );
    let next_writer = FencedTurnRowStore::new(store.clone(), key.clone(), next_claim.claim_fence);
    next_writer
        .write_turn_rows(&[
            turn_row_with_content_for("resume-full-replace", 0, "new parent zero"),
            turn_row_with_content_for("unnamed-full", 1, "new unnamed source"),
        ])
        .unwrap();
    let next_completion = evidence_completion(
        &next_claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&next_claim.key),
    );
    let sources = [
        SourcePublishOutcome {
            source_key: "resume-full-replace".into(),
            mode: SourcePublishMode::Full,
            resume: None,
        },
        SourcePublishOutcome {
            source_key: "child-1".into(),
            mode: SourcePublishMode::Resumed,
            resume: None,
        },
        SourcePublishOutcome {
            source_key: "empty-full".into(),
            mode: SourcePublishMode::Full,
            resume: None,
        },
    ];

    assert!(
        store
            .publish_projections(&next_record, None, &next_completion, &[], &sources)
            .unwrap()
    );

    let published = store.published_turn_rows(&key).unwrap().expect("ready");
    let parent_rows: Vec<_> = published
        .iter()
        .filter(|row| row.source_key == "resume-full-replace")
        .collect();
    let child_rows: Vec<_> = published
        .iter()
        .filter(|row| row.source_key == "child-1")
        .collect();
    let empty_full_rows: Vec<_> = published
        .iter()
        .filter(|row| row.source_key == "empty-full")
        .collect();
    let unnamed_rows: Vec<_> = published
        .iter()
        .filter(|row| row.source_key == "unnamed-full")
        .collect();
    assert_eq!(
        parent_rows.len(),
        1,
        "the old two-row parent set must be replaced outright by the new read"
    );
    assert_eq!(
        child_rows.len(),
        1,
        "the untouched child's row must survive, not be deleted"
    );
    assert!(
        empty_full_rows.is_empty(),
        "a named full source with no new rows must remove its old rows"
    );
    assert_eq!(
        unnamed_rows.len(),
        1,
        "an unnamed source must replace its old rows as a full read"
    );
    assert_eq!(unnamed_rows[0].turn_index, 1);
    let connection = store.lock();
    assert_eq!(
        count_turn_content_rows(
            &connection,
            &turn_session_key(&key),
            first_published_fence.unwrap()
        )
        .unwrap(),
        3
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM turn_content", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        3,
        "the replaced source's old content must not remain orphaned"
    );
}

#[test]
fn a_vanished_source_has_its_rows_and_resume_dropped_on_the_next_publish() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "resume-vanish", 100, 60);
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer
        .write_turn_rows(&[turn_row_for("resume-vanish", 0), turn_row_for("child-1", 0)])
        .unwrap();
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    );
    let sources = [
        SourcePublishOutcome {
            source_key: "resume-vanish".into(),
            mode: SourcePublishMode::Full,
            resume: Some(sample_resume("fp-parent")),
        },
        SourcePublishOutcome {
            source_key: "child-1".into(),
            mode: SourcePublishMode::Full,
            resume: Some(sample_resume("fp-child")),
        },
        SourcePublishOutcome {
            source_key: "empty-child".into(),
            mode: SourcePublishMode::Full,
            resume: Some(sample_resume("fp-empty-child")),
        },
    ];
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &sources)
            .unwrap()
    );

    let other = session("resume-vanish-other", 1_000);
    let other_key = other.key.clone();
    store.upsert_sessions(&[other], &[]).unwrap();
    FencedTurnRowStore::new(store.clone(), other_key.clone(), 777)
        .write_turn_rows(&[turn_row_for("child-1", 0)])
        .unwrap();
    {
        let connection = store.lock();
        insert_source_resume(
            &connection,
            &turn_session_key(&other_key),
            "child-1",
            &sample_resume("fp-other-child"),
        )
        .unwrap();
        insert_source_resume(
            &connection,
            &turn_session_key(&other_key),
            "empty-child",
            &sample_resume("fp-other-empty"),
        )
        .unwrap();
    }

    // Second pass: the child transcript is gone (removed from disk, or
    // unreadable this time), so this pass names and reads only the parent.
    mark_evidence_pending_in(&store.lock(), &key).unwrap();
    let next_claim = store
        .claim_next_evidence(&["claude-code"], 200, 60)
        .unwrap()
        .expect("reclaimable");
    let next_record = projection_record(
        key.clone(),
        "sv1:resume-vanish",
        next_claim.source_generation,
    );
    let next_writer = FencedTurnRowStore::new(store.clone(), key.clone(), next_claim.claim_fence);
    next_writer
        .write_turn_rows(&[turn_row_for("resume-vanish", 1)])
        .unwrap();
    let next_completion = evidence_completion(
        &next_claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&next_claim.key),
    );
    let next_sources = [SourcePublishOutcome {
        source_key: "resume-vanish".into(),
        mode: SourcePublishMode::Resumed,
        resume: None,
    }];

    assert!(
        store
            .publish_projections(&next_record, None, &next_completion, &[], &next_sources)
            .unwrap()
    );

    let published = store.published_turn_rows(&key).unwrap().expect("ready");
    let parent_rows: Vec<_> = published
        .iter()
        .filter(|row| row.source_key == "resume-vanish")
        .collect();
    let child_rows: Vec<_> = published
        .iter()
        .filter(|row| row.source_key == "child-1")
        .collect();
    assert_eq!(
        parent_rows.len(),
        2,
        "the parent's original row and its appended row must both survive"
    );
    assert!(
        child_rows.is_empty(),
        "a source absent from this pass must not keep its rows forever"
    );

    let connection = store.lock();
    assert_eq!(
        query_source_resume(&connection, &turn_session_key(&key), "child-1").unwrap(),
        None,
        "a vanished source's stale resume snapshot must be dropped too"
    );
    assert_eq!(
        query_source_resume(&connection, &turn_session_key(&key), "empty-child").unwrap(),
        None,
        "a vanished source must not need published turn rows for snapshot cleanup"
    );
    assert_eq!(
        query_source_resume(&connection, &turn_session_key(&other_key), "child-1").unwrap(),
        Some(sample_resume("fp-other-child"))
    );
    assert_eq!(
        query_source_resume(&connection, &turn_session_key(&other_key), "empty-child").unwrap(),
        Some(sample_resume("fp-other-empty"))
    );
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&other_key), 777).unwrap(),
        1,
        "publication must keep another session's matching source keys"
    );
}

#[test]
fn a_late_publish_error_rolls_back_set_row_changes_and_resume_updates() {
    let store = store();
    let (key, first_fence) = publish_single_full_source(&store, "resume-rollback");
    let (next_record, next_claim, next_completion) =
        claim_source_with_next_row(&store, &key, "resume-rollback");
    store
        .lock()
        .execute_batch(
            "CREATE TRIGGER fail_resume_insert
             BEFORE INSERT ON source_resume
             WHEN NEW.source_key = 'resume-rollback'
             BEGIN
                 SELECT RAISE(ABORT, 'forced resume write failure');
             END;",
        )
        .unwrap();
    let next_sources = [SourcePublishOutcome {
        source_key: "resume-rollback".into(),
        mode: SourcePublishMode::Full,
        resume: Some(sample_resume("fp-next")),
    }];

    let error = store
        .publish_projections(&next_record, None, &next_completion, &[], &next_sources)
        .expect_err("the trigger must abort the publish");
    assert!(error.to_string().contains("forced resume write failure"));

    let published = store.published_turn_rows(&key).unwrap().expect("ready");
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].turn_index, 0);
    let connection = store.lock();
    assert_eq!(
        query_source_resume(&connection, &turn_session_key(&key), "resume-rollback").unwrap(),
        Some(sample_resume("fp-first"))
    );
    let row_fences = connection
        .prepare(
            "SELECT turn_index, claim_fence FROM turn
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
              ORDER BY turn_index",
        )
        .unwrap()
        .query_map(
            params![key.environment_key, key.agent, key.session_id],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        row_fences,
        vec![(0, first_fence), (1, next_claim.claim_fence)]
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM turn_content", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        2,
        "rollback must restore old content and keep unpublished content"
    );
}

#[test]
fn conflicting_duplicate_source_outcomes_are_rejected_without_publishing() {
    let store = store();
    let (key, _) = publish_single_full_source(&store, "resume-duplicate");
    let (next_record, next_claim, next_completion) =
        claim_source_with_next_row(&store, &key, "resume-duplicate");
    let duplicate_sources = [
        SourcePublishOutcome {
            source_key: "resume-duplicate".into(),
            mode: SourcePublishMode::Resumed,
            resume: Some(sample_resume("fp-next")),
        },
        SourcePublishOutcome {
            source_key: "resume-duplicate".into(),
            mode: SourcePublishMode::Full,
            resume: None,
        },
    ];

    let error = store
        .publish_projections(
            &next_record,
            None,
            &next_completion,
            &[],
            &duplicate_sources,
        )
        .expect_err("duplicate sources must be rejected");
    assert!(
        error
            .to_string()
            .contains("duplicate source publication outcome")
    );

    let published = store.published_turn_rows(&key).unwrap().expect("ready");
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].turn_index, 0);
    let connection = store.lock();
    assert_eq!(
        query_source_resume(&connection, &turn_session_key(&key), "resume-duplicate").unwrap(),
        Some(sample_resume("fp-first"))
    );
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), next_claim.claim_fence).unwrap(),
        1,
        "validation must leave the unpublished claim rows unchanged"
    );
}

#[test]
fn current_resume_revisions_reject_each_prior_batch_revision() {
    let current = crate::analysis::resume_revisions();
    assert_eq!(current.snapshot_revision, 9);
    assert_eq!(current.parser_revision, 38);
    assert_eq!(current.analyzer_revision, 24);
    assert_eq!(current.metrics_schema_revision, 8);
    assert_eq!(current.evidence_schema_revision, 19);
    assert_eq!(current.coverage_schema_revision, 5);
    let mut stored = sample_resume("current");
    stored.snapshot_revision = current.snapshot_revision;
    stored.parser_revision = current.parser_revision;
    stored.analyzer_revision = current.analyzer_revision;
    stored.metrics_schema_revision = current.metrics_schema_revision;
    stored.evidence_schema_revision = current.evidence_schema_revision;
    stored.coverage_schema_revision = current.coverage_schema_revision;
    assert!(current.matches(&stored));
    for field in 0..6 {
        let mut stale = stored.clone();
        match field {
            0 => stale.snapshot_revision = 6,
            1 => stale.parser_revision = 32,
            2 => stale.analyzer_revision = 20,
            3 => stale.metrics_schema_revision = 7,
            4 => stale.evidence_schema_revision = 16,
            5 => stale.coverage_schema_revision = 3,
            _ => unreachable!(),
        }
        assert!(!current.matches(&stale), "accepted stale revision {field}");
    }
}

#[test]
fn purge_stale_source_resume_removes_only_mismatched_revisions() {
    let store = store();
    let (record, _claim) = claimed_projection(&store, "resume-purge", 100, 60);
    let key = record.key.clone();
    {
        let connection = store.lock();
        insert_source_resume(
            &connection,
            &turn_session_key(&key),
            "resume-purge",
            &sample_resume("fp1"),
        )
        .unwrap();
        let mut stale = sample_resume("fp2");
        stale.analyzer_revision = 999;
        insert_source_resume(&connection, &turn_session_key(&key), "child-1", &stale).unwrap();
    }
    let current = ResumeRevisions {
        snapshot_revision: 1,
        parser_revision: 1,
        analyzer_revision: 1,
        metrics_schema_revision: 1,
        evidence_schema_revision: 1,
        coverage_schema_revision: 1,
    };

    let removed = store.purge_stale_source_resume(current).unwrap();
    assert_eq!(removed, 1);

    let connection = store.lock();
    assert!(
        query_source_resume(&connection, &turn_session_key(&key), "resume-purge")
            .unwrap()
            .is_some()
    );
    assert!(
        query_source_resume(&connection, &turn_session_key(&key), "child-1")
            .unwrap()
            .is_none()
    );
}
