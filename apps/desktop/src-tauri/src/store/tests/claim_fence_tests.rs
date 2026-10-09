use std::path::Path;

use antiburn_local::analysis::{TurnSessionKey, count_turn_rows, insert_turn_rows};

use super::*;

fn rows_at(store: &Store, key: &SessionKey, fence: i64) -> u64 {
    count_turn_rows(&store.lock(), &turn_session_key(key), fence).unwrap()
}

/// Clear the store while `claim` runs, then discover the session again.
fn clear_and_rediscover(store: &Store, session_id: &str) {
    store.clear_local_session_data().unwrap();
    let mut again = session(session_id, 1_000);
    again.source_fingerprint = Some(format!("sv1:{session_id}"));
    store
        .upsert_sessions(
            std::slice::from_ref(&again),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
}

fn ready(claim: &EvidenceClaim) -> EvidenceCompletion {
    evidence_completion(
        claim,
        PublishedEvidence::Ready,
        crate::store::test_support::evidence_json(&claim.key),
    )
}

#[test]
fn a_pass_from_before_a_clear_cannot_touch_the_new_claim() {
    let store = store();
    let (record, stale) = claimed_projection(&store, "fence-across-clear", 100, 300);
    let key = record.key.clone();
    let stale_writer = FencedTurnRowStore::new(store.clone(), key.clone(), stale.claim_fence);
    stale_writer.write_turn_rows(&[turn_row(0)]).unwrap();

    clear_and_rediscover(&store, "fence-across-clear");
    // The stale pass keeps reading, and its next batch lands on the new row.
    stale_writer.write_turn_rows(&[turn_row(1)]).unwrap();

    let current = store
        .claim_next_evidence(&["claude-code"], 101, 300)
        .unwrap()
        .unwrap();
    assert_eq!(current.source_generation, stale.source_generation);
    assert_ne!(current.claim_fence, stale.claim_fence);
    assert!(!store.renew_evidence_lease(&stale, 102, 300).unwrap());

    FencedTurnRowStore::new(store.clone(), key.clone(), current.claim_fence)
        .write_turn_rows(&[turn_row(0), turn_row(1), turn_row(2)])
        .unwrap();
    assert!(
        store
            .publish_projections(&record, None, &ready(&current), &[], &[])
            .unwrap()
    );
    assert_eq!(rows_at(&store, &key, current.claim_fence), 3);
    assert_eq!(rows_at(&store, &key, stale.claim_fence), 0);

    // The stale pass loses the publish race. Its cleanup keeps the new rows.
    assert!(
        !store
            .publish_projections(&record, None, &ready(&stale), &[], &[])
            .unwrap()
    );
    assert_eq!(rows_at(&store, &key, current.claim_fence), 3);
    assert_eq!(
        store.evidence(&key).unwrap().map(|row| row.status),
        Some(EvidenceStatus::Ready)
    );
}

#[test]
fn rows_from_a_cancelled_pass_before_a_clear_are_not_published() {
    let store = store();
    let (record, stale) = claimed_projection(&store, "cancelled-across-clear", 100, 300);
    let key = record.key.clone();
    let stale_writer = FencedTurnRowStore::new(store.clone(), key.clone(), stale.claim_fence);

    clear_and_rediscover(&store, "cancelled-across-clear");
    stale_writer
        .write_turn_rows(&[turn_row(0), turn_row(1)])
        .unwrap();
    // The worker cancels the stale pass here, without a cleanup.
    assert!(!store.renew_evidence_lease(&stale, 102, 300).unwrap());

    let current = store
        .claim_next_evidence(&["claude-code"], 103, 300)
        .unwrap()
        .unwrap();
    FencedTurnRowStore::new(store.clone(), key.clone(), current.claim_fence)
        .write_turn_rows(&[turn_row(0), turn_row(1), turn_row(2)])
        .unwrap();
    assert!(
        store
            .publish_projections(&record, None, &ready(&current), &[], &[])
            .unwrap()
    );

    let published = store.lock().query_row(
        "SELECT COUNT(*) FROM turn
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![key.environment_key, key.agent, key.session_id],
        |row| row.get::<_, i64>(0),
    );
    assert_eq!(published.unwrap(), 3);
}

#[test]
fn claims_on_different_sessions_never_share_a_fence() {
    let store = store();
    let (_, first) = claimed_projection(&store, "fence-first", 100, 300);
    let (_, second) = claimed_projection(&store, "fence-second", 100, 300);

    assert_ne!(first.claim_fence, second.claim_fence);
    assert!(second.claim_fence > first.claim_fence);
}

#[test]
fn v71_starts_the_fence_counter_above_every_stored_fence() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..70] {
        connection.execute_batch(sql).unwrap();
    }
    connection.pragma_update(None, "user_version", 70).unwrap();
    connection
        .execute(
            "INSERT INTO session (environment_key, agent, session_id, source_kind,
                                  source_label, first_seen_at, last_seen_at,
                                  source_generation, incarnation)
             VALUES ('native', 'claude-code', 'v71', 'file', 'v71', 'now', 'now', 1, 1)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO session_evidence (environment_key, agent, session_id, claim_fence)
             VALUES ('native', 'claude-code', 'v71', 4)",
            [],
        )
        .unwrap();
    // A row an abandoned pass left at a higher fence than the evidence row.
    insert_turn_rows(
        &connection,
        &TurnSessionKey {
            environment_key: "native",
            agent: "claude-code",
            session_id: "v71",
        },
        9,
        &[turn_row(0)],
    )
    .unwrap();

    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-v71-claim-fence-migration").to_path_buf(),
    )
    .unwrap();
    assert_eq!(store.schema_version().unwrap(), 75);

    let claim = store
        .claim_next_evidence(&["claude-code"], 100, 300)
        .unwrap()
        .unwrap();
    assert_eq!(claim.claim_fence, 10);
}
