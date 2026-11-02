use super::*;
use crate::store::native_tool_results::{publications::PublicationPolicy, request_origin};
use antiburn_local::analysis::file_live::NativeFileReadError;

const CANARY: &str = "LEGACY_FROZEN_REQUEST_MUST_NOT_DISPATCH";

fn legacy_fixture() -> FileFixture {
    let fixture = FileFixture::with_source(SourceFormat::ClaudeJsonl, CHECK, false, false, false);
    let RawSource::File(path) = &fixture.input.source else {
        unreachable!()
    };
    let header = std::fs::read_to_string(path)
        .unwrap()
        .replace("Baseline request.", CANARY);
    std::fs::write(path, header).unwrap();
    let (call, result) = fixture.result_records(0, true);
    fixture.append_bytes(format!("{call}{result}").as_bytes());
    let mut page = read_native_file_page(&fixture.input, None, &|| false).unwrap();
    for event in &mut page.evidence.events {
        event.live_eligible = true;
    }
    {
        let mut connection = fixture.store.lock();
        let transaction = connection.transaction().unwrap();
        assert!(
            evidence::reconcile_evidence_in(
                &transaction,
                &fixture.key,
                1,
                1,
                &page.evidence,
                PublicationPolicy {
                    suppress_coverage: true,
                    ..Default::default()
                },
                NOW * 1000
            )
            .unwrap()
        );
        transaction.commit().unwrap();
    }
    fixture
        .store
        .claim_next_native_file(NOW, 60)
        .unwrap()
        .unwrap();
    fixture
        .store
        .lock()
        .execute(
            "UPDATE native_file_delivery SET parser_revision = 64, cursor_json = ?1,
        observed_generation = (SELECT source_generation FROM session WHERE agent = 'claude-code'),
        pending = 0, claim_fence = NULL, lease_expires_at_epoch = NULL, next_attempt_at_epoch = ?2",
            params![serde_json::to_string(&page.cursor).unwrap(), NOW + 1],
        )
        .unwrap();
    {
        let mut connection = fixture.store.lock();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute(
                "INSERT INTO burn_check_session_attachment
            (check_id, activation_scope_key, generation, branch_key, incarnation)
            SELECT ?1, ?2, ?3, ?4, incarnation FROM session WHERE agent = 'claude-code'",
                params![
                    fixture.active.id.definition_id,
                    fixture.adapter.activation_scope_key,
                    fixture.active.id.generation.0,
                    fixture.key.to_identity_json()
                ],
            )
            .unwrap();
        let incarnation = transaction
            .query_row(
                "SELECT incarnation FROM session WHERE agent = 'claude-code'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        crate::store::native_tool_results::observers::attach_in(
            &transaction,
            &fixture.active.id,
            &fixture.key,
            incarnation,
            &serde_json::to_string(&page.cursor).unwrap(),
            NOW * 1000,
        )
        .unwrap();
        transaction.commit().unwrap();
    }
    let source = PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_project(&fixture.store, "native", "/repo")
            .with_activation(&fixture.active.id),
        40,
    )
    .unwrap();
    let page = source.read_batch(PublicationCursor(0)).unwrap();
    fixture
        .engine
        .apply_source_page(&fixture.active, &page)
        .unwrap();
    assert!(
        serde_json::to_string(&fixture.pending())
            .unwrap()
            .contains(CANARY)
    );
    fixture
}

fn historical_claim(fixture: &FileFixture, candidate: FrozenCandidate) -> ClaimedCandidate {
    fixture
        .store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET state = 'claimed',
        claim_owner = 'owner-1', claim_fence = 1 WHERE candidate_id = ?1",
            [hex(&candidate.fingerprint)],
        )
        .unwrap();
    let claimed = ClaimedCandidate {
        id: CandidateId::from(&candidate),
        candidate,
        prior_status: None,
        fence: 1,
    };
    fixture
        .adapter
        .record_preparation(
            &claimed,
            &PreparationRecord::Ready(prepared_review(
                "fixture-v1",
                json!({"legacy":CANARY}),
                json!({"exact":"saved"}),
                Default::default(),
                EvaluationBudget::default(),
            )),
        )
        .unwrap();
    claimed
}

fn historical_dispatch(fixture: &FileFixture, claimed: &ClaimedCandidate) {
    fixture
        .store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET state = 'dispatching' WHERE candidate_id = ?1",
            [hex(&claimed.id.0)],
        )
        .unwrap();
    fixture.store.lock().execute("INSERT INTO burn_check_evaluation_attempt(attempt_id, candidate_id,
        decision_name, attempt_number, state, updated_at_epoch) VALUES (?1, ?2, 'review', 1, 'dispatch_intent', 100)",
        params![attempt_id(&claimed.id.0, &review_attempt(1)), hex(&claimed.id.0)]).unwrap();
}

fn historical_answer(fixture: &FileFixture, claimed: ClaimedCandidate) -> AssessmentAcceptance {
    historical_dispatch(fixture, &claimed);
    let result = review_result(&fixture.active, 1, 0.99);
    let evaluated = EvaluatedAssessment::new(
        &fixture.active.compiled.decisions,
        &fixture.active.compiled.question_requirements(),
        vec![result.clone()],
    )
    .unwrap();
    let actions = plan_status_actions(
        &fixture.active.compiled.actions,
        &claimed.candidate.context.value,
        &evaluated,
        None,
    )
    .unwrap();
    fixture.store.lock().execute("UPDATE burn_check_evaluation_attempt SET state = 'settled', outcome = 'answer', response_json = ?1
        WHERE candidate_id = ?2", params![serde_json::to_string(&result).unwrap(), hex(&claimed.id.0)]).unwrap();
    fixture.store.lock().execute("UPDATE burn_check_candidate SET state = 'answered', answer_json = ?1 WHERE candidate_id = ?2",
        params![serde_json::to_string(&evaluated).unwrap(), hex(&claimed.id.0)]).unwrap();
    AssessmentAcceptance {
        claimed,
        evaluated,
        actions,
    }
}

fn immutable_input(fixture: &FileFixture, id: &CandidateId) -> String {
    fixture
        .store
        .lock()
        .query_row(
            "SELECT json_array(frozen_input_json, input_fingerprint,
        preparation_json, dependencies_json) FROM burn_check_candidate WHERE candidate_id = ?1",
            [hex(&id.0)],
            |row| row.get(0),
        )
        .unwrap()
}

fn save_parser_65_receipt(fixture: &FileFixture) {
    save_parser_receipt(fixture, 65);
}

fn save_parser_receipt(fixture: &FileFixture, parser_revision: i64) {
    assert_eq!(
        fixture
            .store
            .lock()
            .execute(
                "UPDATE native_file_delivery SET parser_revision = ?4
               WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                 AND json_extract(cursor_json, '$.revision') = 5",
                params![
                    fixture.key.environment_key,
                    fixture.key.agent,
                    fixture.key.session_id,
                    parser_revision
                ],
            )
            .unwrap(),
        1
    );
}

#[test]
fn parser_65_receipt_keeps_saved_pending_work_current_before_the_first_parser_67_page() {
    prior_parser_receipt_keeps_saved_pending_work_current(65);
}

#[test]
fn parser_66_receipt_keeps_saved_pending_work_current_before_the_first_parser_67_page() {
    prior_parser_receipt_keeps_saved_pending_work_current(66);
}

fn prior_parser_receipt_keeps_saved_pending_work_current(parser_revision: i64) {
    let mut fixture =
        FileFixture::with_source(SourceFormat::ClaudeJsonl, CHECK, false, false, true);
    let (call, result) = fixture.result_records(0, true);
    fixture.append_bytes(format!("{call}{result}").as_bytes());
    fixture.publish(NOW + 1);
    fixture.sync();
    let candidate = fixture.pending();
    let id = CandidateId::from(&candidate);
    let input = immutable_input(&fixture, &id);
    let progress = fixture
        .adapter
        .load_source_progress(&fixture.active)
        .unwrap();
    let publications = fixture
        .store
        .native_evidence_publications_after(0, 100)
        .unwrap();
    save_parser_receipt(&fixture, parser_revision);
    fixture.reopen();

    let claimed = claim(&fixture.adapter, &candidate);
    assert_eq!(claimed.candidate, candidate);
    assert_eq!(immutable_input(&fixture, &id), input);
    assert_eq!(
        fixture
            .adapter
            .load_source_progress(&fixture.active)
            .unwrap(),
        progress
    );
    assert_eq!(
        fixture
            .store
            .native_evidence_publications_after(0, 100)
            .unwrap(),
        publications
    );
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row(
                "SELECT parser_revision FROM native_file_delivery",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        parser_revision
    );
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        fixture
            .adapter
            .dispatch_review(&claimed, 1_100_000, Duration::ZERO, 1)
            .unwrap(),
        ClaimOutcome::Current
    );
}

#[test]
fn parser_65_receipt_accepts_a_saved_paid_answer_before_the_first_parser_67_page() {
    prior_parser_receipt_accepts_a_saved_paid_answer(65);
}

#[test]
fn parser_66_receipt_accepts_a_saved_paid_answer_before_the_first_parser_67_page() {
    prior_parser_receipt_accepts_a_saved_paid_answer(66);
}

fn prior_parser_receipt_accepts_a_saved_paid_answer(parser_revision: i64) {
    let mut fixture =
        FileFixture::with_source(SourceFormat::ClaudeJsonl, CHECK, false, false, true);
    let (call, result) = fixture.result_records(0, true);
    fixture.append_bytes(format!("{call}{result}").as_bytes());
    fixture.publish(NOW + 1);
    fixture.sync();
    let assessment = answer(
        &fixture.adapter,
        &fixture.active,
        claim(&fixture.adapter, &fixture.pending()),
        0.99,
    );
    let id = hex(&assessment.claimed.id.0);
    fixture
        .store
        .record_burn_check_decision_usage(
            &id,
            &review_attempt(1),
            &fixture.active.id.definition_id,
            ("custom-systemone", "synthetic-model"),
            Some(antiburn_local::analysis::jev::JevUsage {
                input_tokens: 7,
                output_tokens: 3,
            }),
            NOW + 1,
        )
        .unwrap();
    save_parser_receipt(&fixture, parser_revision);
    let input = immutable_input(&fixture, &assessment.claimed.id);
    let usage = fixture.store.burn_check_usage_summary().unwrap();
    assert_eq!(
        (
            usage.confirmed_calls,
            usage.input_tokens,
            usage.output_tokens
        ),
        (1, 7, 3)
    );
    let progress = fixture
        .adapter
        .load_source_progress(&fixture.active)
        .unwrap();
    let publications = fixture
        .store
        .native_evidence_publications_after(0, 100)
        .unwrap();
    let attempts = |store: &Store| {
        store
            .lock()
            .query_row(
                "SELECT json_group_array(json_array(attempt_id, state, outcome, response_json))
           FROM burn_check_evaluation_attempt",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    };
    let saved_attempts = attempts(&fixture.store);
    let receipt: String = fixture.store.lock().query_row(
        "SELECT json_array(parser_revision, cursor_json, observed_generation) FROM native_file_delivery",
        [],
        |row| row.get(0),
    ).unwrap();
    fixture.reopen();

    let mut recovery = fixture.adapter.load_recovery(None, 10).unwrap();
    assert_eq!(recovery.len(), 1);
    let RecoveryWork::Answered { claimed, evaluated } = recovery.remove(0).work else {
        panic!("the paid answer must recover without another dispatch");
    };
    assert_eq!(evaluated, assessment.evaluated);
    let recovered = AssessmentAcceptance {
        claimed,
        evaluated,
        actions: assessment.actions,
    };
    assert_eq!(
        fixture.adapter.accept(&recovered).unwrap(),
        AcceptanceOutcome::Accepted
    );
    assert_eq!(immutable_input(&fixture, &recovered.claimed.id), input);
    assert_eq!(fixture.store.burn_check_usage_summary().unwrap(), usage);
    assert_eq!(attempts(&fixture.store), saved_attempts);
    assert_eq!(
        fixture
            .adapter
            .load_source_progress(&fixture.active)
            .unwrap(),
        progress
    );
    assert_eq!(
        fixture
            .store
            .native_evidence_publications_after(0, 100)
            .unwrap(),
        publications
    );
    assert_eq!(fixture.store.lock().query_row(
        "SELECT json_array(parser_revision, cursor_json, observed_generation) FROM native_file_delivery",
        [],
        |row| row.get::<_, String>(0),
    ).unwrap(), receipt);
    let status: String = fixture
        .store
        .lock()
        .query_row(
            "SELECT json_array(status, assessment_id, position, evidence_valid, updated_at_epoch)
           FROM burn_check_current_status",
            [],
            |row| row.get(0),
        )
        .unwrap();
    fixture.reopen();
    assert_eq!(fixture.engine.recover_pending().unwrap().candidates, 0);
    assert!(fixture.adapter.load_recovery(None, 10).unwrap().is_empty());
    assert_eq!(
        fixture.adapter.accept(&recovered).unwrap(),
        AcceptanceOutcome::AlreadyAccepted
    );
    assert_eq!(fixture.store.burn_check_usage_summary().unwrap(), usage);
    assert_eq!(attempts(&fixture.store), saved_attempts);
    assert_eq!(fixture.store.lock().query_row(
        "SELECT json_array(status, assessment_id, position, evidence_valid, updated_at_epoch)
           FROM burn_check_current_status",
        [],
        |row| row.get::<_, String>(0),
    ).unwrap(), status);
}

#[test]
fn parser_65_boundary_blocks_legacy_claim_dispatch_and_final_acceptance() {
    let fixture = legacy_fixture();
    let candidate = fixture.pending();
    assert_eq!(
        fixture
            .adapter
            .claim_due(&candidate, 100_000, true)
            .unwrap(),
        CandidateClaim::Stale
    );

    let fixture = legacy_fixture();
    let claimed = historical_claim(&fixture, fixture.pending());
    let input = immutable_input(&fixture, &claimed.id);
    assert_eq!(
        fixture
            .adapter
            .dispatch_review(&claimed, 100_000, Duration::ZERO, 1)
            .unwrap(),
        ClaimOutcome::Stale
    );
    assert_eq!(immutable_input(&fixture, &claimed.id), input);
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );

    let fixture = legacy_fixture();
    let assessment = historical_answer(&fixture, historical_claim(&fixture, fixture.pending()));
    let input = immutable_input(&fixture, &assessment.claimed.id);
    fixture.publish(NOW + 1);
    assert_eq!(
        fixture.adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Stale
    );
    assert_eq!(immutable_input(&fixture, &assessment.claimed.id), input);
    fixture.sync();
    assert_eq!(
        fixture.adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Stale
    );
}

#[test]
fn parser_65_boundary_keeps_old_frozen_work_stale_after_the_cursor_is_safe_and_the_app_restarts() {
    let mut fixture = legacy_fixture();
    let candidate = fixture.pending();
    let id = CandidateId::from(&candidate);
    let input = immutable_input(&fixture, &id);
    fixture.publish(NOW + 1);
    fixture.reopen();
    fixture.sync();
    assert!(
        !request_origin::new_candidate_request_origin_current_in(
            &fixture.store.lock(),
            &hex(&id.0)
        )
        .unwrap()
    );
    assert_eq!(
        fixture
            .adapter
            .claim_due(&candidate, 1_100_000, true)
            .unwrap(),
        CandidateClaim::Stale
    );
    assert_eq!(immutable_input(&fixture, &id), input);
}

#[test]
fn parser_65_current_work_checks_exact_receipt_binding_at_claim_dispatch_answer_and_acceptance() {
    for mutation in [
        "UPDATE session SET source_label = source_label || ':changed'; UPDATE native_file_delivery SET source_label = source_label || ':changed'",
        "UPDATE session SET cwd = '/other'; UPDATE native_file_delivery SET cwd = '/other'",
        "UPDATE session SET incarnation = incarnation + 1; UPDATE native_file_delivery SET incarnation = incarnation + 1",
    ] {
        for stage in ["claim", "dispatch", "answer", "accept"] {
            let fixture =
                FileFixture::with_source(SourceFormat::ClaudeJsonl, CHECK, false, false, true);
            let (call, result) = fixture.result_records(0, true);
            fixture.append_bytes(format!("{call}{result}").as_bytes());
            fixture.publish(NOW + 1);
            fixture.sync();
            let candidate = fixture.pending();
            assert!(
                request_origin::new_candidate_request_origin_current_in(
                    &fixture.store.lock(),
                    &hex(&candidate.fingerprint)
                )
                .unwrap()
            );
            if stage == "claim" {
                fixture.store.lock().execute_batch(mutation).unwrap();
                assert_eq!(
                    fixture
                        .adapter
                        .claim_due(&candidate, 1_100_000, true)
                        .unwrap(),
                    CandidateClaim::Stale,
                    "{stage}: {mutation}"
                );
                continue;
            }
            let claimed = claim(&fixture.adapter, &candidate);
            fixture
                .adapter
                .record_preparation(
                    &claimed,
                    &PreparationRecord::Ready(prepared_review(
                        "fixture-v1",
                        json!({}),
                        json!({"exact":"saved"}),
                        Default::default(),
                        EvaluationBudget::default(),
                    )),
                )
                .unwrap();
            let input = immutable_input(&fixture, &claimed.id);
            if stage == "dispatch" {
                fixture.store.lock().execute_batch(mutation).unwrap();
                assert_eq!(
                    fixture
                        .adapter
                        .dispatch_review(&claimed, 1_100_000, Duration::ZERO, 1)
                        .unwrap(),
                    ClaimOutcome::Stale,
                    "{stage}: {mutation}"
                );
                assert_eq!(immutable_input(&fixture, &claimed.id), input);
                continue;
            }
            if stage == "answer" {
                assert_eq!(
                    fixture
                        .adapter
                        .dispatch_review(&claimed, 1_100_000, Duration::ZERO, 1)
                        .unwrap(),
                    ClaimOutcome::Current
                );
                fixture.store.lock().execute_batch(mutation).unwrap();
                assert_eq!(
                    fixture
                        .adapter
                        .record_decision_answer(
                            &claimed,
                            &review_attempt(1),
                            &review_result(&fixture.active, 1, 0.99)
                        )
                        .unwrap(),
                    ClaimOutcome::Stale,
                    "{stage}: {mutation}"
                );
                assert_eq!(immutable_input(&fixture, &claimed.id), input);
                assert!(fixture.store.lock().query_row("SELECT response_json IS NOT NULL AND outcome = 'answer' FROM burn_check_evaluation_attempt", [], |row| row.get::<_, bool>(0)).unwrap());
                continue;
            }
            let assessment = answer(&fixture.adapter, &fixture.active, claimed, 0.99);
            fixture.store.lock().execute_batch(mutation).unwrap();
            assert_eq!(
                fixture.adapter.accept(&assessment).unwrap(),
                AcceptanceOutcome::Stale,
                "{stage}: {mutation}"
            );
            assert_eq!(immutable_input(&fixture, &assessment.claimed.id), input);
        }
    }
}

#[test]
fn parser_65_paid_result_accepts_a_late_answer_after_a_real_native_read_outage() {
    for failure in [
        NativeFileReadError::ReadFailed,
        NativeFileReadError::Unavailable,
    ] {
        let mut fixture =
            FileFixture::with_source(SourceFormat::ClaudeJsonl, CHECK, false, false, true);
        let (call, result) = fixture.result_records(0, true);
        fixture.append_bytes(format!("{call}{result}").as_bytes());
        fixture.publish(NOW + 1);
        fixture.sync();
        let claimed = claim(&fixture.adapter, &fixture.pending());
        assert_eq!(
            fixture
                .adapter
                .dispatch_review(
                    &claimed,
                    u64::try_from(NOW + 1).unwrap() * 1000,
                    Duration::from_secs(60),
                    1
                )
                .unwrap(),
            ClaimOutcome::Current
        );
        save_parser_65_receipt(&fixture);
        let input = immutable_input(&fixture, &claimed.id);
        let publications = fixture
            .store
            .native_evidence_publications_after(0, 100)
            .unwrap();
        let cursor: String = fixture
            .store
            .lock()
            .query_row("SELECT cursor_json FROM native_file_delivery", [], |row| {
                row.get(0)
            })
            .unwrap();
        let file_claim = fixture
            .store
            .claim_next_native_file(NOW + 2, 60)
            .unwrap()
            .unwrap();
        assert!(
            fixture
                .store
                .fail_native_file(&file_claim, failure, NOW + 2, NOW + 3)
                .unwrap()
        );
        let id = hex(&claimed.id.0);
        assert!(
            request_origin::new_candidate_request_origin_current_in(&fixture.store.lock(), &id)
                .unwrap()
        );
        assert!(
            crate::store::native_tool_results::observers::typed_dependencies_current_in(
                &fixture.store.lock(),
                &id
            )
            .unwrap()
        );
        assert_eq!(
            fixture
                .store
                .native_evidence_publications_after(0, 100)
                .unwrap(),
            publications
        );
        assert_eq!(
            fixture
                .store
                .lock()
                .query_row("SELECT cursor_json FROM native_file_delivery", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            cursor
        );
        let result = review_result(&fixture.active, 1, 0.99);
        assert_eq!(
            fixture
                .adapter
                .record_decision_answer(&claimed, &review_attempt(1), &result)
                .unwrap(),
            ClaimOutcome::Current
        );
        fixture
            .store
            .record_burn_check_decision_usage(
                &id,
                &review_attempt(1),
                &fixture.active.id.definition_id,
                ("custom-systemone", "synthetic-model"),
                Some(antiburn_local::analysis::jev::JevUsage {
                    input_tokens: 7,
                    output_tokens: 3,
                }),
                NOW + 2,
            )
            .unwrap();
        let usage = fixture.store.burn_check_usage_summary().unwrap();
        assert_eq!(
            (
                usage.confirmed_calls,
                usage.input_tokens,
                usage.output_tokens
            ),
            (1, 7, 3)
        );
        let evaluated = EvaluatedAssessment::new(
            &fixture.active.compiled.decisions,
            &fixture.active.compiled.question_requirements(),
            vec![result],
        )
        .unwrap();
        let actions = plan_status_actions(
            &fixture.active.compiled.actions,
            &claimed.candidate.context.value,
            &evaluated,
            claimed.prior_status,
        )
        .unwrap();
        let assessment = AssessmentAcceptance {
            claimed,
            evaluated,
            actions,
        };
        assert_eq!(
            fixture.adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted
        );
        assert_eq!(immutable_input(&fixture, &assessment.claimed.id), input);
        assert_eq!(fixture.store.burn_check_usage_summary().unwrap(), usage);
        fixture.reopen();
        assert_eq!(fixture.store.burn_check_usage_summary().unwrap(), usage);
        assert_eq!(
            fixture.adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::AlreadyAccepted
        );
        assert!(
            fixture
                .store
                .lock()
                .query_row(
                    "SELECT COUNT(*) = 1 AND MIN(outcome) = 'answer'
            FROM burn_check_evaluation_attempt",
                    [],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
        );
    }
}

#[test]
fn parser_65_boundary_is_consumed_before_same_page_new_work_can_dispatch() {
    let fixture = legacy_fixture();
    let legacy = fixture.pending();
    let (call, result) = fixture.result_records(1, true);
    fixture.append_bytes(format!("{call}{result}").as_bytes());
    fixture.publish(NOW + 1);
    let boundary: i64 = fixture
        .store
        .lock()
        .query_row(
            "SELECT publication_sequence FROM native_tool_result_publication WHERE gap_reason = ?1",
            [request_origin::RESET_REASON],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        !request_origin::new_candidate_request_origin_current_in(
            &fixture.store.lock(),
            &hex(&legacy.fingerprint)
        )
        .unwrap()
    );
    let progress = fixture
        .adapter
        .load_source_progress(&fixture.active)
        .unwrap();
    let page = fixture
        .source()
        .read_batch(progress.publication_cursor)
        .unwrap();
    assert!(page.events.iter().any(|event| matches!(event, SourceEvent::SegmentReset { cursor, .. } if cursor.0 == boundary as u64)));
    fixture
        .engine
        .apply_source_page(&fixture.active, &page)
        .unwrap();
    let candidate = fixture.pending();
    assert_ne!(candidate.fingerprint, legacy.fingerprint);
    assert!(!serde_json::to_string(&candidate).unwrap().contains(CANARY));
    assert!(
        request_origin::new_candidate_request_origin_current_in(
            &fixture.store.lock(),
            &hex(&candidate.fingerprint)
        )
        .unwrap()
    );
    let claimed = claim(&fixture.adapter, &candidate);
    assert_eq!(
        fixture
            .adapter
            .dispatch_review(&claimed, 101_000, Duration::ZERO, 1)
            .unwrap(),
        ClaimOutcome::Current
    );
}

#[test]
fn parser_65_boundary_keeps_late_answers_unknown_delivery_and_usage_without_resend() {
    for unknown in [false, true] {
        let mut fixture = legacy_fixture();
        let claimed = historical_claim(&fixture, fixture.pending());
        historical_dispatch(&fixture, &claimed);
        let input = immutable_input(&fixture, &claimed.id);
        fixture.publish(NOW + 1);
        let outcome = if unknown {
            fixture
                .adapter
                .record_attempt_failure(
                    &claimed,
                    &review_attempt(1),
                    AttemptFailure::Unknown,
                    &EvaluationDiagnostic {
                        code: antiburn_checks::engine::EvaluationFailureCode::DeliveryUnknown,
                        detail: "Synthetic unknown delivery.".into(),
                    },
                )
                .unwrap()
        } else {
            fixture
                .adapter
                .record_decision_answer(
                    &claimed,
                    &review_attempt(1),
                    &review_result(&fixture.active, 1, 0.99),
                )
                .unwrap()
        };
        assert_eq!(outcome, ClaimOutcome::Stale);
        fixture
            .store
            .record_burn_check_decision_usage(
                &hex(&claimed.id.0),
                &review_attempt(1),
                &fixture.active.id.definition_id,
                ("custom-systemone", "synthetic-model"),
                if unknown {
                    None
                } else {
                    Some(antiburn_local::analysis::jev::JevUsage {
                        input_tokens: 7,
                        output_tokens: 3,
                    })
                },
                NOW,
            )
            .unwrap();
        let usage = fixture.store.burn_check_usage_summary().unwrap();
        let attempt: String = fixture
            .store
            .lock()
            .query_row(
                "SELECT json_array(attempt_id, state, outcome, response_json)
            FROM burn_check_evaluation_attempt",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(immutable_input(&fixture, &claimed.id), input);
        fixture.reopen();
        fixture.sync();
        assert!(fixture.adapter.load_recovery(None, 10).unwrap().is_empty());
        assert_eq!(fixture.store.burn_check_usage_summary().unwrap(), usage);
        assert_eq!(
            fixture
                .store
                .lock()
                .query_row(
                    "SELECT json_array(attempt_id, state, outcome, response_json)
            FROM burn_check_evaluation_attempt",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            attempt
        );
        assert_eq!(immutable_input(&fixture, &claimed.id), input);
    }
}

#[test]
fn parser_65_boundary_preserves_already_accepted_inputs_status_history_and_cooldown() {
    let fixture = legacy_fixture();
    let assessment = historical_answer(&fixture, historical_claim(&fixture, fixture.pending()));
    let id = hex(&assessment.claimed.id.0);
    fixture
        .store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET state = 'accepted' WHERE candidate_id = ?1",
            [&id],
        )
        .unwrap();
    fixture.store.lock().execute("INSERT INTO burn_check_assessment_history
        SELECT ?2, candidate_id, check_id, activation_scope_key, generation, branch_key, trigger_position,
            'assessed', answer_json, ?3, 1, 100 FROM burn_check_candidate WHERE candidate_id = ?1",
        params![id, format!("assessment:{id}"), serde_json::to_string(&assessment.actions).unwrap()]).unwrap();
    fixture
        .store
        .lock()
        .execute(
            "INSERT INTO burn_check_current_status
        SELECT check_id, activation_scope_key, branch_key, generation, 'failure', ?2,
            trigger_position, 1, 100 FROM burn_check_candidate WHERE candidate_id = ?1",
            params![id, format!("assessment:{id}")],
        )
        .unwrap();
    fixture.store.lock().execute("INSERT INTO burn_check_cooldown
        SELECT check_id, activation_scope_key, generation, branch_key, 200 FROM burn_check_candidate WHERE candidate_id = ?1", [&id]).unwrap();
    let snapshot = || {
        let connection = fixture.store.lock();
        [
            "burn_check_candidate",
            "burn_check_assessment_history",
            "burn_check_current_status",
            "burn_check_cooldown",
            "burn_check_evaluation_attempt",
        ]
        .into_iter()
        .map(|table| {
            let columns = connection
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
                .into_iter()
                .map(|name| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(",");
            connection
                .query_row(
                    &format!("SELECT json_group_array(json_array({columns})) FROM {table}"),
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap()
        })
        .collect::<Vec<_>>()
    };
    let before = snapshot();
    assert_eq!(
        fixture.adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::AlreadyAccepted
    );
    fixture.publish(NOW + 1);
    fixture.sync();
    assert_eq!(
        fixture.adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::AlreadyAccepted
    );
    assert_eq!(snapshot(), before);
}

#[test]
fn parser_65_startup_rejects_old_results_even_when_the_global_cursor_is_after_reset() {
    let fixture = legacy_fixture();
    let source = PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_project(&fixture.store, "native", "/repo")
            .with_activation(&fixture.active.id),
        40,
    )
    .unwrap();
    let old = source.read_batch(PublicationCursor(0)).unwrap().events.into_iter().find(|event| {
        matches!(event, SourceEvent::Observation { observation, .. } if matches!(observation.kind, ObservationKind::ToolResult { .. }))
    }).unwrap();
    fixture.publish(NOW + 1);
    let progress = fixture
        .adapter
        .load_source_progress(&fixture.active)
        .unwrap();
    let reset = fixture
        .source()
        .read_batch(progress.publication_cursor)
        .unwrap();
    fixture
        .engine
        .apply_source_page(&fixture.active, &reset)
        .unwrap();
    let next: u64 = fixture
        .store
        .lock()
        .query_row(
            "SELECT MAX(publication_sequence) + 1 FROM native_tool_result_publication",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut unrelated = event(&fixture.store, next, "ignored", "other-branch", 1);
    unrelated.events = vec![SourceEvent::Ignored {
        cursor: PublicationCursor(next),
    }];
    fixture
        .engine
        .apply_source_page(&fixture.active, &unrelated)
        .unwrap();
    fixture
        .store
        .lock()
        .execute("DELETE FROM burn_check_session_attachment", [])
        .unwrap();
    fixture
        .store
        .lock()
        .execute(
            "DELETE FROM burn_check_observation WHERE event_kind = 'tool_result'",
            [],
        )
        .unwrap();
    let sessions = fixture
        .adapter
        .startup_sessions(&fixture.active, NOW - 180)
        .unwrap();
    assert_eq!(sessions.len(), 1);
    let reader = StorePublishedEvidenceReader::for_project(&fixture.store, "native", "/repo")
        .with_activation(&fixture.active.id);
    assert!(
        reader
            .recent_results_before(&fixture.key, None)
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .adapter
            .initialize_recent(&fixture.active, &sessions[0], &[old])
            .unwrap()
            .is_none()
    );
    assert_eq!(fixture.store.lock().query_row("SELECT COUNT(*) FROM burn_check_candidate WHERE state = 'pending' AND session_key_json = ?1",
        [fixture.key.to_identity_json()], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn parser_65_boundary_survives_retention_and_restart_without_another_reset() {
    let mut fixture =
        FileFixture::with_source(SourceFormat::ClaudeJsonl, CHECK, false, false, true);
    let (call, result) = fixture.result_records(0, true);
    fixture.append_bytes(format!("{call}{result}").as_bytes());
    fixture.publish(NOW + 1);
    fixture.sync();
    let candidate = fixture.pending();
    save_parser_65_receipt(&fixture);
    let progress = fixture
        .adapter
        .load_source_progress(&fixture.active)
        .unwrap();
    let marker: i64 = fixture
        .store
        .lock()
        .query_row(
            "SELECT publication_sequence FROM native_tool_result_publication WHERE gap_reason = ?1",
            [request_origin::RESET_REASON],
            |row| row.get(0),
        )
        .unwrap();
    let old_publication = {
        let connection = fixture.store.lock();
        connection
            .execute(
                "INSERT INTO session(environment_key, agent, session_id, source_kind,
            source_label, cwd, updated_at_epoch, first_seen_at, last_seen_at)
            VALUES ('native', 'claude-code', 'expired', 'file', '/synthetic/expired.jsonl',
                '/expired', ?1, 'now', 'now')",
                [NOW - 172_800],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO native_file_delivery(environment_key, agent, session_id,
            incarnation, source_label, cwd, parser_revision, cursor_json, active_until_epoch)
            SELECT environment_key, agent, session_id, incarnation, source_label, cwd, 64, '{}', 0
              FROM session WHERE session_id = 'expired'",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO native_tool_result_publication(outcome, environment_key,
            agent, session_id, source_format, native_identity, native_position_json, native_revision,
            payload_json, complete, source_generation, claim_fence, project_cwd, session_incarnation)
            SELECT 'update', environment_key, agent, session_id, '\"claude_jsonl\"', 'expired-result',
                '{}', 1, '{\"private\":\"EXPIRED_PAYLOAD_CANARY\"}', 1, 1, 1, cwd, incarnation
              FROM session WHERE session_id = 'expired'", []).unwrap();
        connection.last_insert_rowid()
    };
    {
        let mut connection = fixture.store.lock();
        let transaction = connection.transaction().unwrap();
        assert_eq!(
            crate::store::apply_session_retention_in(&transaction, 1, NOW + 200).unwrap(),
            1
        );
        transaction.commit().unwrap();
        assert!(!connection.query_row("SELECT EXISTS(SELECT 1 FROM native_tool_result_publication WHERE publication_sequence = ?1)",
            [old_publication], |row| row.get::<_, bool>(0)).unwrap());
        assert!(!connection.query_row("SELECT EXISTS(SELECT 1 FROM native_file_delivery WHERE session_id = 'expired')",
            [], |row| row.get::<_, bool>(0)).unwrap());
        assert!(
            connection
                .execute(
                    "DELETE FROM native_tool_result_publication WHERE publication_sequence = ?1",
                    [marker]
                )
                .is_err()
        );
        assert!(
            connection
                .query_row(
                    "SELECT payload_json IS NULL AND dependency_binding_json IS NULL
            FROM native_tool_result_publication WHERE publication_sequence = ?1",
                    [marker],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
        );
    }
    assert!(
        request_origin::new_candidate_request_origin_current_in(
            &fixture.store.lock(),
            &hex(&candidate.fingerprint)
        )
        .unwrap()
    );
    fixture.reopen();
    assert!(
        request_origin::new_candidate_request_origin_current_in(
            &fixture.store.lock(),
            &hex(&candidate.fingerprint),
        )
        .unwrap()
    );
    assert_eq!(
        fixture
            .adapter
            .load_source_progress(&fixture.active)
            .unwrap(),
        progress
    );
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row(
                "SELECT parser_revision FROM native_file_delivery",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        65
    );
    let page = fixture.publish(NOW + 201);
    assert_eq!(page.bytes_read, 0);
    assert!(page.evidence.events.is_empty() && page.results.is_empty());
    fixture.sync();
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM native_tool_result_publication WHERE gap_reason = ?1",
                [request_origin::RESET_REASON],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    let claimed = claim(&fixture.adapter, &candidate);
    assert_eq!(
        fixture
            .adapter
            .dispatch_review(&claimed, 1_300_000, Duration::ZERO, 1)
            .unwrap(),
        ClaimOutcome::Current
    );
}
