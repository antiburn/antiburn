use serde_json::json;

use super::*;

const CHECK_IDS: &[&str] = &["ignored_instructions", "future_check"];

#[test]
fn activity_boundaries_are_shared_per_check_and_stale_results_are_rejected() {
    let store = store();
    let mut record = session("assessment-boundary", 10_000);
    record.activity_cursor = "cursor-one".to_owned();
    record.source_fingerprint = Some("fingerprint-one".to_owned());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    publish_ready(&store, &record, 7);

    assert_eq!(
        store
            .capture_burn_check_boundaries(CHECK_IDS, 20_000)
            .unwrap(),
        2
    );
    assert!(
        store
            .burn_check_candidates("ignored_instructions", 30_000, 180, 10)
            .unwrap()
            .is_empty()
    );

    record.updated_at_epoch = Some(29_000);
    record.activity_cursor = "cursor-two".to_owned();
    record.source_fingerprint = Some("fingerprint-two".to_owned());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    publish_ready(&store, &record, 8);

    let candidate = store
        .burn_check_candidates("ignored_instructions", 40_000, 180, 10)
        .unwrap()
        .pop()
        .expect("new activity after enablement is eligible");
    assert_eq!(candidate.boundary_at_epoch, 20_000);
    let input = input(&candidate, "revision-one");
    assert!(
        store
            .queue_burn_check_assessment(&input, 40_000, 180)
            .unwrap()
    );
    assert!(
        !store
            .queue_burn_check_assessment(&input, 40_001, 180)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input, 40_000, 300, 180)
            .unwrap()
    );
    assert!(
        !store
            .claim_burn_check_assessment(&input, 40_001, 300, 180)
            .unwrap()
    );
    assert!(
        store
            .save_burn_check_progress(&input, r#"{"completed":1}"#, 40_002, 300, 180)
            .unwrap()
    );

    record.updated_at_epoch = Some(40_003);
    record.activity_cursor = "cursor-three".to_owned();
    record.source_fingerprint = Some("fingerprint-three".to_owned());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    publish_ready(&store, &record, 9);
    assert!(
        !store
            .complete_burn_check_assessment(&input, r#"{"findings":[]}"#, 40_004, 180)
            .unwrap()
    );
    let assessment = store
        .burn_check_assessment(&record.key, "ignored_instructions")
        .unwrap()
        .unwrap();
    assert_eq!(assessment.status, "superseded");
    assert_eq!(assessment.result_json, None);
}

#[test]
fn request_reservations_survive_unknown_outcomes_and_exact_responses_are_cached() {
    let store = store();
    let mut record = session("assessment-usage", 10_000);
    record.activity_cursor = "cursor-one".to_owned();
    record.source_fingerprint = Some("fingerprint-one".to_owned());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    publish_ready(&store, &record, 8);
    store
        .capture_burn_check_boundaries(CHECK_IDS, 20_000)
        .unwrap();
    record.updated_at_epoch = Some(29_000);
    record.activity_cursor = "cursor-two".to_owned();
    record.source_fingerprint = Some("fingerprint-two".to_owned());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    publish_ready(&store, &record, 9);
    let candidate = store
        .burn_check_candidates("ignored_instructions", 40_000, 180, 10)
        .unwrap()
        .pop()
        .unwrap();
    let input = input(&candidate, "usage-revision");
    assert!(
        store
            .queue_burn_check_assessment(&input, 40_000, 180)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input, 40_000, 300, 180)
            .unwrap()
    );

    let mut reservation_ids = Vec::new();
    for _ in 0..30 {
        match store
            .reserve_burn_check_usage(&input, 8_192, 40_001, 180)
            .unwrap()
        {
            BurnCheckReservation::Reserved(id) => reservation_ids.push(id),
            other => panic!("unexpected reservation result: {other:?}"),
        }
    }
    assert_eq!(
        store
            .reserve_burn_check_usage(&input, 8_192, 40_001, 180)
            .unwrap(),
        BurnCheckReservation::UsageLimitReached
    );
    store
        .settle_burn_check_usage(&reservation_ids[0], Some(0), 40_002)
        .unwrap();
    let BurnCheckReservation::Reserved(reservation_id) = store
        .reserve_burn_check_usage(&input, 8_192, 40_002, 180)
        .unwrap()
    else {
        panic!("released usage should permit the next bounded request");
    };
    store
        .record_burn_check_response(
            &reservation_id,
            CachedAssessmentResponse {
                provider: "synthetic-provider".to_owned(),
                request_digest: "exact-request-digest".to_owned(),
                returned_model: "model-v1".to_owned(),
                response_json: json!({"answers": ["typed"]}).to_string(),
                input_tokens: 12,
                output_tokens: 1,
                created_at_epoch: 40_002,
            },
        )
        .unwrap();

    assert_eq!(
        store
            .cached_assessment_response("synthetic-provider", "exact-request-digest", 40_003)
            .unwrap()
            .unwrap()
            .input_tokens,
        12
    );
    assert!(
        store
            .cached_assessment_response("synthetic-provider", "another-digest", 40_003)
            .unwrap()
            .is_none()
    );
    let ledger: serde_json::Value = serde_json::from_str(
        &store
            .internal_value("internal:burnCheckUsageLedgerV1")
            .unwrap(),
    )
    .unwrap();
    let total_reserved: u64 = ledger["reservations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["input_tokens"].as_u64().unwrap())
        .sum();
    assert_eq!(total_reserved, 29 * 8_192 + 12);
}

#[test]
fn clearing_session_data_removes_assessment_progress_cache_and_usage() {
    let store = store();
    let mut record = session("assessment-clear", 10_000);
    record.activity_cursor = "cursor-one".to_owned();
    record.source_fingerprint = Some("fingerprint-one".to_owned());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    publish_ready(&store, &record, 8);
    store
        .capture_burn_check_boundaries(CHECK_IDS, 20_000)
        .unwrap();
    record.updated_at_epoch = Some(29_000);
    record.activity_cursor = "cursor-two".to_owned();
    record.source_fingerprint = Some("fingerprint-two".to_owned());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    publish_ready(&store, &record, 9);
    let candidate = store
        .burn_check_candidates("ignored_instructions", 40_000, 180, 10)
        .unwrap()
        .pop()
        .unwrap();
    let input = input(&candidate, "clear-revision");
    store
        .queue_burn_check_assessment(&input, 40_000, 180)
        .unwrap();
    store
        .cache_assessment_response(CachedAssessmentResponse {
            provider: "synthetic-provider".to_owned(),
            request_digest: "digest-clear".to_owned(),
            returned_model: "model-v1".to_owned(),
            response_json: "{}".to_owned(),
            input_tokens: 1,
            output_tokens: 0,
            created_at_epoch: 40_000,
        })
        .unwrap();

    let before_clear = time::OffsetDateTime::now_utc().unix_timestamp();
    store.clear_local_session_data().unwrap();

    assert!(
        store
            .burn_check_assessment(&record.key, "ignored_instructions")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.internal_value("internal:burnCheckUsageLedgerV1"),
        None
    );
    assert_eq!(
        store.internal_value("internal:burnCheckResponseCacheV1"),
        None
    );
    assert!(
        store
            .internal_value("internal:burnChecksEnabledAtEpochV1")
            .unwrap()
            .parse::<i64>()
            .unwrap()
            >= before_clear
    );
}

fn publish_ready(store: &Store, record: &SessionRecord, fence: i64) {
    let (generation, fingerprint): (i64, Option<String>) = store
        .lock()
        .query_row(
            "SELECT source_generation, source_fingerprint FROM session
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session_evidence
                SET status = 'ready', analyzed_generation = ?4,
                    processed_fingerprint = ?5, parser_revision = ?6,
                    analyzer_revision = ?7, evidence_schema_revision = ?8,
                    evidence_json = '{}', claim_fence = ?9, published_fence = ?9
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id,
                generation,
                fingerprint,
                antiburn_local::analysis::PARSER_REVISION,
                antiburn_local::analysis::ANALYZER_REVISION,
                antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                fence,
            ],
        )
        .unwrap();
}

fn input(candidate: &BurnCheckCandidate, input_revision: &str) -> BurnCheckInput {
    BurnCheckInput {
        key: candidate.session.key.clone(),
        check_id: "ignored_instructions".to_owned(),
        incarnation: candidate.incarnation,
        source_generation: candidate.source_generation,
        source_fingerprint: candidate.source_fingerprint.clone(),
        activity_cursor: candidate.activity_cursor.clone(),
        published_fence: candidate.published_fence,
        input_revision: input_revision.to_owned(),
        evaluator_revision: "synthetic-evaluator-v1".to_owned(),
        boundary_at_epoch: candidate.boundary_at_epoch,
    }
}
