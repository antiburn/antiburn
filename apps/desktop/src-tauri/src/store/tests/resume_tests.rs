//! Store-level pinning tests for `source_resume` (continuous ingest, phase
//! 3b): the snapshot write inside a winning publish, the fence restamp and
//! replace `Store::publish_projections` runs per source, and the startup
//! purge of stale revisions. See "R4. Fence semantics", "R5. Snapshot
//! storage", and "R6. Invalidation" in the phase 3b build spec
//! (`docs/plans/continuous-session-ingest.md`).

use super::*;

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

#[test]
fn a_winning_publish_writes_the_resume_snapshot_named_for_its_source() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "resume-write", 100, 60);
    let key = record.key.clone();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
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
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
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
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
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
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );
    let first_published_fence = store.evidence(&key).unwrap().unwrap().published_fence;

    // Second pass: the session resumes. Only the parent appends one new row
    // under the new claim fence; the child is untouched — no new rows, no
    // entry in `sources`.
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
    let next_completion = evidence_completion(&next_claim, PublishedEvidence::Ready, "{}".into());
    let sources = [SourcePublishOutcome {
        source_key: "resume-restamp".into(),
        mode: SourcePublishMode::Resumed,
        resume: None,
    }];

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
            turn_row_for("resume-full-replace", 0),
            turn_row_for("resume-full-replace", 1),
            turn_row_for("child-1", 0),
        ])
        .unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );

    // Second pass: the parent forces a full read (a tail rewrite, say) and
    // rewrites its rows from scratch; the child is untouched.
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
        .write_turn_rows(&[turn_row_for("resume-full-replace", 0)])
        .unwrap();
    let next_completion = evidence_completion(&next_claim, PublishedEvidence::Ready, "{}".into());
    let sources = [SourcePublishOutcome {
        source_key: "resume-full-replace".into(),
        mode: SourcePublishMode::Full,
        resume: None,
    }];

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
