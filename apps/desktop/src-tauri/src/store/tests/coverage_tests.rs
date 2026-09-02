use antiburn_local::analysis::{
    EvidenceSource, SessionCoverageRecord, SessionEvidenceAccumulator, SourceCapabilities,
    SourceKind, TurnRowStore,
};

use super::*;

fn coverage_record(session_id: &str) -> SessionCoverageRecord {
    SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude-code".into(),
        session_id: session_id.into(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    })
    .coverage_record()
}

#[test]
fn a_fenced_coverage_writer_writes_a_record_the_store_can_read() {
    let store = store();
    let mut record = session("coverage-writer", 1_000);
    record.source_fingerprint = Some("sv1:coverage-writer".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let key = record.key.clone();

    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), 7);
    let written = coverage_record("coverage-writer");
    writer.write_coverage_record(&written).unwrap();

    let connection = store.lock();
    assert_eq!(
        query_coverage_record(&connection, &turn_session_key(&key), 7).unwrap(),
        Some(written)
    );
    assert_eq!(
        query_coverage_record(&connection, &turn_session_key(&key), 8).unwrap(),
        None
    );
}

#[test]
fn publishing_evidence_keeps_only_the_current_fence_coverage_record() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "publish-current-fence-coverage", 100, 60);
    let key = record.key.clone();

    // A record left over from an earlier, superseded pass under a stale fence.
    {
        let connection = store.lock();
        insert_coverage_record(
            &connection,
            &turn_session_key(&key),
            claim.claim_fence - 1,
            &coverage_record("publish-current-fence-coverage"),
        )
        .unwrap();
    }
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    let current = coverage_record("publish-current-fence-coverage");
    writer.write_coverage_record(&current).unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());

    assert!(
        store
            .publish_projections(&record, None, &completion, &[])
            .unwrap()
    );

    let connection = store.lock();
    assert_eq!(
        query_coverage_record(&connection, &turn_session_key(&key), claim.claim_fence - 1).unwrap(),
        None,
        "the superseded fence's coverage record must be gone"
    );
    assert_eq!(
        query_coverage_record(&connection, &turn_session_key(&key), claim.claim_fence).unwrap(),
        Some(current)
    );
    drop(connection);
    assert_eq!(
        store.published_coverage_record(&key).unwrap(),
        Some(coverage_record("publish-current-fence-coverage")),
        "the published record must survive and be readable through Store::published_coverage_record"
    );
}

#[test]
fn a_lost_publish_race_deletes_only_its_own_fences_coverage_record() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "lost-race-coverage", 100, 60);
    let key = record.key.clone();
    // A record from a still-current, earlier pass. A lost race must not
    // touch a record it does not own.
    {
        let connection = store.lock();
        insert_coverage_record(
            &connection,
            &turn_session_key(&key),
            claim.claim_fence - 1,
            &coverage_record("lost-race-coverage"),
        )
        .unwrap();
    }
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer
        .write_coverage_record(&coverage_record("lost-race-coverage"))
        .unwrap();
    // Bumping the source generation makes the fenced UPDATE inside
    // `publish_projections` affect zero rows — the same "lost the race"
    // shape the turn-row equivalent of this test exercises.
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
        )
        .unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());

    assert!(
        !store
            .publish_projections(&record, None, &completion, &[])
            .unwrap()
    );

    let connection = store.lock();
    assert_eq!(
        query_coverage_record(&connection, &turn_session_key(&key), claim.claim_fence).unwrap(),
        None,
        "the losing pass's own coverage record must be gone"
    );
    assert_eq!(
        query_coverage_record(&connection, &turn_session_key(&key), claim.claim_fence - 1).unwrap(),
        Some(coverage_record("lost-race-coverage")),
        "a record the lost race does not own must be untouched"
    );
}
