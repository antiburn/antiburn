use antiburn_local::analysis::{
    EvidenceSource, SessionEvidenceAccumulator, SourceCapabilities, SourceKind,
    count_turn_content_rows,
};

use super::*;

fn stage(store: &Store, claim: &EvidenceClaim) -> FencedTurnRowStore {
    let writer = FencedTurnRowStore::new(store.clone(), claim.key.clone(), claim.claim_fence);
    writer
        .write_turn_rows(&[user_turn_with_content(0, "synthetic content")])
        .unwrap();
    writer
        .write_coverage_record(&coverage(&claim.key.session_id))
        .unwrap();
    writer
}

fn coverage(session_id: &str) -> SessionCoverageRecord {
    SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude-code".into(),
        session_id: session_id.into(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    })
    .coverage_record()
}

fn row_counts(store: &Store, claim: &EvidenceClaim) -> (u64, u64, bool) {
    let connection = store.lock();
    let key = turn_session_key(&claim.key);
    (
        count_turn_rows(&connection, &key, claim.claim_fence).unwrap(),
        count_turn_content_rows(&connection, &key, claim.claim_fence).unwrap(),
        query_coverage_record(&connection, &key, &FenceScope::single(claim.claim_fence))
            .unwrap()
            .is_some(),
    )
}

fn retry() -> EvidenceFailure {
    EvidenceFailure::Retry {
        next_attempt_at_epoch: 200,
        counts_as_attempt: true,
    }
}

fn ready(claim: &EvidenceClaim) -> EvidenceCompletion {
    evidence_completion(
        claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    )
}

fn assert_writer_rejected(writer: &FencedTurnRowStore) {
    assert!(writer.write_turn_rows(&[turn_row(1)]).is_err());
    assert!(
        writer
            .write_coverage_record(&coverage("synthetic"))
            .is_err()
    );
    assert!(writer.write_resume("s1", resume_snapshot("stale")).is_err());
    assert!(writer.drop_resume("s1").is_err());
    assert!(writer.delete_rows_for_source("s1").is_err());
}

#[test]
fn failed_parse_attempts_remove_turn_content_and_coverage_without_a_publication() {
    let store = store();
    let (_, mut claim) = claimed_projection(&store, "failed-parse-cleanup", 100, 60);
    for attempt in 0..4 {
        let writer = stage(&store, &claim);
        assert_eq!(row_counts(&store, &claim), (1, 1, true));
        let failure = if attempt == 3 {
            EvidenceFailure::Failed {
                revisions: revisions(),
            }
        } else {
            retry()
        };
        assert!(
            store
                .fail_evidence(&claim, failure, "synthetic parser failure")
                .unwrap()
        );
        assert_eq!(row_counts(&store, &claim), (0, 0, false));
        assert_writer_rejected(&writer);
        assert_eq!(
            store.evidence(&claim.key).unwrap().unwrap().published_fence,
            None
        );
        if attempt < 3 {
            claim = store
                .claim_next_evidence(&["claude-code"], 200, 60)
                .unwrap()
                .unwrap();
        }
    }
    assert_eq!(
        store.evidence(&claim.key).unwrap().unwrap().status,
        EvidenceStatus::Failed
    );
}

#[test]
fn stale_failure_removes_only_its_claim_and_preserves_published_and_newer_rows() {
    let store = store();
    let (record, published) = claimed_projection(&store, "stale-failure-cleanup", 100, 60);
    let published_writer = stage(&store, &published);
    assert!(
        store
            .publish_projections(&record, None, &ready(&published), &[], &[])
            .unwrap()
    );
    store.requeue_session_evidence(&record.key).unwrap();
    let stale = store
        .claim_next_evidence(&["claude-code"], 200, 60)
        .unwrap()
        .unwrap();
    let stale_writer = stage(&store, &stale);
    let current = store
        .claim_next_evidence(&["claude-code"], 261, 60)
        .unwrap()
        .unwrap();
    let current_writer = stage(&store, &current);
    current_writer
        .write_resume("s1", resume_snapshot("current"))
        .unwrap();

    assert!(!store.fail_evidence(&stale, retry(), "superseded").unwrap());

    assert_eq!(row_counts(&store, &stale), (0, 0, false));
    assert_eq!(row_counts(&store, &published), (1, 1, true));
    assert_eq!(row_counts(&store, &current), (1, 1, true));
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().claim_fence,
        current.claim_fence
    );
    assert_writer_rejected(&stale_writer);
    assert_writer_rejected(&published_writer);
    assert_eq!(
        current_writer.read_resume("s1").unwrap(),
        Some(resume_snapshot("current"))
    );
    assert!(store.fail_evidence(&current, retry(), "cancelled").unwrap());
    assert_eq!(row_counts(&store, &current), (0, 0, false));
    assert_eq!(row_counts(&store, &published), (1, 1, true));
    assert_writer_rejected(&current_writer);
}

#[test]
fn cleanup_after_a_committed_publication_preserves_its_rows() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "committed-cleanup", 100, 60);
    let writer = stage(&store, &claim);
    assert!(
        store
            .publish_projections(&record, None, &ready(&claim), &[], &[])
            .unwrap()
    );
    assert!(
        !store
            .publish_projections(&record, None, &ready(&claim), &[], &[])
            .unwrap()
    );
    assert_eq!(row_counts(&store, &claim), (1, 1, true));

    assert!(
        !store
            .fail_evidence(&claim, retry(), "failure after commit")
            .unwrap()
    );
    assert_eq!(row_counts(&store, &claim), (1, 1, true));
    assert_writer_rejected(&writer);
    assert_eq!(row_counts(&store, &claim), (1, 1, true));
}

#[test]
fn cleanup_after_a_resumed_publication_preserves_the_old_target_fence() {
    let store = store();
    let (record, first) = claimed_projection(&store, "resumed-cleanup", 100, 60);
    stage(&store, &first);
    let sources = [SourcePublishOutcome {
        source_key: "s1".into(),
        mode: SourcePublishMode::Full,
        resume: Some(resume_snapshot("first")),
    }];
    assert!(
        store
            .publish_projections(&record, None, &ready(&first), &[], &sources)
            .unwrap()
    );
    store.requeue_session_evidence(&record.key).unwrap();
    let next = store
        .claim_next_evidence(&["claude-code"], 200, 60)
        .unwrap()
        .unwrap();
    let writer = FencedTurnRowStore::new(store.clone(), next.key.clone(), next.claim_fence);
    writer
        .write_turn_rows(&[user_turn_with_content(1, "synthetic append")])
        .unwrap();
    writer
        .write_coverage_record(&coverage(&next.key.session_id))
        .unwrap();
    let sources = [SourcePublishOutcome {
        source_key: "s1".into(),
        mode: SourcePublishMode::Resumed,
        resume: Some(resume_snapshot("next")),
    }];
    assert!(
        store
            .publish_projections(&record, None, &ready(&next), &[], &sources)
            .unwrap()
    );

    assert!(
        !store
            .fail_evidence(&next, retry(), "failure after resumed commit")
            .unwrap()
    );
    assert_eq!(
        store
            .evidence(&record.key)
            .unwrap()
            .unwrap()
            .published_fence,
        Some(first.claim_fence)
    );
    assert_eq!(row_counts(&store, &first), (2, 2, true));
    assert_eq!(row_counts(&store, &next), (0, 0, false));
    assert_writer_rejected(&writer);
    assert_eq!(
        writer.read_resume("s1").unwrap(),
        Some(resume_snapshot("next"))
    );
}

#[test]
fn completed_pass_cleanup_removes_only_its_exact_unpublished_fence() {
    let store = store();
    let (_, stale) = claimed_projection(&store, "completed-cleanup", 100, 60);
    let stale_writer = stage(&store, &stale);
    let current = store
        .claim_next_evidence(&["claude-code"], 161, 60)
        .unwrap()
        .unwrap();
    stage(&store, &current);

    assert!(
        !store
            .fail_evidence(&stale, retry(), "stale pass stopped")
            .unwrap()
    );
    assert!(
        !store
            .fail_evidence(&stale, retry(), "stale pass stopped")
            .unwrap()
    );
    assert_eq!(row_counts(&store, &stale), (0, 0, false));
    assert_eq!(row_counts(&store, &current), (1, 1, true));
    assert_writer_rejected(&stale_writer);
}

#[test]
fn deletion_and_rediscovery_do_not_let_an_old_writer_restore_rows() {
    let store = store();
    let (_, claim) = claimed_projection(&store, "deleted-cleanup", 100, 60);
    let writer = stage(&store, &claim);
    assert!(store.delete_session(&claim.key).unwrap().is_some());
    assert_writer_rejected(&writer);
    assert_eq!(row_counts(&store, &claim), (0, 0, false));
    let (_, current) = claimed_projection(&store, "deleted-cleanup", 200, 60);
    stage(&store, &current);

    assert!(
        !store
            .fail_evidence(&claim, retry(), "deleted pass")
            .unwrap()
    );
    assert_writer_rejected(&writer);
    assert_eq!(row_counts(&store, &claim), (0, 0, false));
    assert_eq!(row_counts(&store, &current), (1, 1, true));
}

#[test]
fn requeue_rejects_late_writes_before_a_new_claim_exists() {
    let store = store();
    let (_, claim) = claimed_projection(&store, "requeue-cleanup", 100, 60);
    let writer = stage(&store, &claim);
    store.requeue_session_evidence(&claim.key).unwrap();

    assert!(
        !store
            .fail_evidence(&claim, retry(), "source changed")
            .unwrap()
    );
    assert_writer_rejected(&writer);
    assert_eq!(row_counts(&store, &claim), (0, 0, false));
}
