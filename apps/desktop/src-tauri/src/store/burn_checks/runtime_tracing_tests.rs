use super::*;
use crate::store::burn_checks::trace_capture::LogCapture;
use std::collections::BTreeSet;

const SECRET: &str = "PRIVATE_TRACE_CANARY";

#[test]
fn rejected_preparation_traces_keep_typed_size_facts_and_preserve_absent_detail() {
    use antiburn_checks::fit::{
        NoFitReason, PreparationBudget, PreparationBudgetKind, PreparationFailureCode,
        PreparationFailureContext,
    };
    for reason in [
        None,
        Some(NoFitReason::NoRules),
        Some(NoFitReason::NoMatchingTargets),
        Some(NoFitReason::TargetsAtMinimum),
        Some(NoFitReason::ReductionsExhausted),
    ] {
        for kind in [
            PreparationBudgetKind::RequestBytes,
            PreparationBudgetKind::EstimatedTokens,
            PreparationBudgetKind::ResidentBytes,
        ] {
            let (store, adapter, active) = fixture();
            let candidate = apply(&adapter, &active, &event(&store, 1, SECRET, SECRET, 1));
            let claimed = claim(&adapter, &candidate);
            let failure = PreparationFailure {
                code: PreparationFailureCode::NoFit,
                detail: SECRET.into(),
                report: None,
                context: Box::new(PreparationFailureContext {
                    decision: Some(SECRET.into()),
                    field: Some(SECRET.into()),
                    input_path: Some(SECRET.into()),
                    no_fit_reason: reason,
                    budget: Some(PreparationBudget {
                        kind,
                        observed: 32641,
                        maximum: 28672,
                    }),
                }),
            };
            let captured = LogCapture::default();
            tracing::subscriber::with_default(captured.clone(), || {
                for _ in 0..100 {
                    adapter
                        .record_preparation(&claimed, &PreparationRecord::Rejected(failure.clone()))
                        .unwrap();
                }
                adapter
                    .record_preparation_rejection(&claimed, &failure)
                    .unwrap();
            });
            let preparations = captured.events("burn_check_preparation_committed");
            assert_eq!(preparations.len(), 1);
            let settled = captured.events("burn_check_candidate_settled");
            assert_eq!(settled.len(), 1);
            for event in [preparations.first().unwrap(), settled.first().unwrap()] {
                assert_eq!(event.fields["no_fit_reason"], format!("{reason:?}"));
                assert_eq!(event.fields["budget_kind"], format!("{:?}", Some(kind)));
                assert_eq!(event.fields["budget_observed"], "32641");
                assert_eq!(event.fields["budget_maximum"], "28672");
            }
            let allowed = [
                "event",
                "activation_ref",
                "candidate_ref",
                "generation",
                "claim_fence",
                "status",
                "preparation_failure_code",
                "no_fit_reason",
                "budget_kind",
                "budget_observed",
                "budget_maximum",
                "elapsed_ms",
            ]
            .into_iter()
            .collect::<BTreeSet<_>>();
            assert_eq!(
                preparations[0]
                    .fields
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
                allowed
            );
            assert!(!captured.text().contains(SECRET));
            assert_eq!(
                store
                    .lock()
                    .query_row(
                        "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                        [],
                        |row| row.get::<_, usize>(0)
                    )
                    .unwrap(),
                0
            );
        }
    }
    for code in [
        PreparationFailureCode::NoFit,
        PreparationFailureCode::PreparationResourceLimit,
        PreparationFailureCode::ProviderUnavailable,
    ] {
        let (store, adapter, active) = fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, SECRET, SECRET, 1));
        let claimed = claim(&adapter, &candidate);
        let failure = PreparationFailure {
            code,
            detail: SECRET.into(),
            report: None,
            context: Default::default(),
        };
        let captured = LogCapture::default();
        tracing::subscriber::with_default(captured.clone(), || {
            adapter
                .record_preparation_rejection(&claimed, &failure)
                .unwrap();
        });
        let event = captured.events("burn_check_candidate_settled").remove(0);
        assert_eq!(
            event.fields["preparation_reason"],
            format!("{:?}", Some(code))
        );
        assert_eq!(event.fields["no_fit_reason"], "None");
        assert_eq!(event.fields["budget_kind"], "None");
        assert!(!event.fields.contains_key("budget_observed"));
        assert!(!event.fields.contains_key("budget_maximum"));
        assert!(!captured.text().contains(SECRET));
    }
}

#[test]
fn preparation_write_failure_does_not_log_a_committed_preparation_or_settlement() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, SECRET, SECRET, 1));
    let claimed = claim(&adapter, &candidate);
    let failure = PreparationFailure {
        code: antiburn_checks::fit::PreparationFailureCode::NoFit,
        detail: SECRET.into(),
        report: None,
        context: Default::default(),
    };
    store.lock().execute_batch("CREATE TRIGGER reject_preparation BEFORE UPDATE OF preparation_json ON burn_check_candidate
        BEGIN SELECT RAISE(ABORT, 'PRIVATE_PREPARATION_ERROR'); END;").unwrap();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for _ in 0..100 {
            assert!(
                adapter
                    .record_preparation(&claimed, &PreparationRecord::Rejected(failure.clone()))
                    .is_err()
            );
        }
        assert!(
            adapter
                .record_preparation_rejection(&claimed, &failure)
                .is_err()
        );
    });
    assert!(
        captured
            .events("burn_check_preparation_committed")
            .is_empty()
    );
    assert!(captured.events("burn_check_candidate_settled").is_empty());
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT state, preparation_json FROM burn_check_candidate",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            )
            .unwrap(),
        ("claimed".into(), None)
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_assessment_history",
                [],
                |row| row.get::<_, usize>(0)
            )
            .unwrap(),
        0
    );
    assert!(!captured.text().contains("PRIVATE_"));
}

#[test]
fn source_page_transition_counts_actual_revocations_and_logs_only_after_commit() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, SECRET, SECRET, 1));
    let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Accepted
    );
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    let correction = event(&store, 2, SECRET, SECRET, 2);
    store.lock().execute_batch("CREATE TRIGGER reject_page_revocation BEFORE UPDATE OF evidence_valid ON burn_check_assessment_history
        WHEN OLD.evidence_valid = 1 AND NEW.evidence_valid = 0 BEGIN SELECT RAISE(ABORT, 'PRIVATE_PAGE_REVOCATION_ERROR'); END;").unwrap();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for _ in 0..100 {
            assert!(engine.apply_source_page(&active, &correction).is_err());
        }
        assert!(
            captured
                .events("burn_check_evidence_transition_committed")
                .is_empty()
        );
        assert_eq!(
            adapter
                .load_source_progress(&active)
                .unwrap()
                .publication_cursor,
            PublicationCursor(1)
        );
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT evidence_valid FROM burn_check_assessment_history",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        store
            .lock()
            .execute_batch("DROP TRIGGER reject_page_revocation")
            .unwrap();
        engine.apply_source_page(&active, &correction).unwrap();
        let empty = SourceBatch {
            events: vec![],
            next: PublicationCursor(2),
            has_more: false,
        };
        for _ in 0..100 {
            engine.apply_source_page(&active, &empty).unwrap();
        }
    });
    let transitions = captured.events("burn_check_evidence_transition_committed");
    assert_eq!(transitions.len(), 1);
    assert_eq!(
        transitions[0].fields["publication_origin"],
        "\"runtime_source_page\""
    );
    assert_eq!(transitions[0].fields["publication_after_cursor"], "1");
    assert_eq!(transitions[0].fields["publication_through_cursor"], "2");
    assert_eq!(transitions[0].fields["accepted_evidence_revoked"], "1");
    assert_eq!(transitions[0].fields["current_status_revoked"], "1");
    assert_eq!(transitions[0].fields["unfinished_stale"], "0");
    let allowed = [
        "event",
        "publication_origin",
        "reason",
        "activation_ref",
        "generation",
        "publication_after_cursor",
        "publication_through_cursor",
        "accepted_evidence_revoked",
        "current_status_revoked",
        "unfinished_stale",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(
        transitions[0]
            .fields
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        allowed
    );
    assert!(!captured.text().contains("PRIVATE_"));
}

#[test]
fn source_page_revocation_stays_in_its_activation_scope_with_equal_candidate_fences() {
    let (store, first_adapter, first) = fixture();
    let mut second = first.clone();
    second.id.scope.session_cwd = "/PRIVATE_SECOND_REPO".into();
    second.project_root = Some("/PRIVATE_SECOND_REPO".into());
    let second_adapter = Arc::new(
        StoreCheckStore::new(
            store.clone(),
            "PRIVATE_SECOND_OWNER".into(),
            second.id.scope.clone(),
        )
        .unwrap(),
    );
    second_adapter
        .activate(
            &second.id.definition_id,
            &LifecycleTransition::Activated(second.clone()),
            Some(&second),
        )
        .unwrap();
    let first_candidate = apply(
        &first_adapter,
        &first,
        &event(&store, 1, "PRIVATE_FIRST_ID", "PRIVATE_FIRST_SESSION", 1),
    );
    let second_page = event(&store, 2, "PRIVATE_SECOND_ID", "PRIVATE_SECOND_SESSION", 1);
    store.lock().execute("UPDATE session SET cwd = '/PRIVATE_SECOND_REPO' WHERE session_id = 'PRIVATE_SECOND_SESSION'", []).unwrap();
    let second_candidate = apply(&second_adapter, &second, &second_page);
    let first_claim = claim(&first_adapter, &first_candidate);
    let second_claim = claim(&second_adapter, &second_candidate);
    assert_eq!(first_claim.fence, second_claim.fence);
    for (adapter, active, claimed) in [
        (&first_adapter, &first, first_claim),
        (&second_adapter, &second, second_claim),
    ] {
        let assessment = answer(adapter, active, claimed, 0.99);
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted
        );
    }
    let correction = event(&store, 3, "PRIVATE_FIRST_ID", "PRIVATE_FIRST_SESSION", 2);
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        CheckEngine::new(first_adapter.clone(), Arc::new(NoEvaluator))
            .apply_source_page(&first, &correction)
            .unwrap();
    });
    let events = captured.events("burn_check_evidence_transition_committed");
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].fields["activation_ref"],
        format!(
            "{:?}",
            crate::store::burn_checks::trace_refs::activation_ref(&first.id)
        )
    );
    assert_eq!(events[0].fields["accepted_evidence_revoked"], "1");
    let valid: i64 = store.lock().query_row("SELECT evidence_valid FROM burn_check_assessment_history WHERE activation_scope_key = ?1", [&second_adapter.activation_scope_key], |row| row.get(0)).unwrap();
    assert_eq!(valid, 1);
    assert!(!captured.text().contains("PRIVATE_"));
}

#[test]
fn tracing_records_committed_stages_without_content_or_repeated_preparation_and_recovery() {
    let (store, adapter, active) = fixture();
    let captured = LogCapture::default();
    let mut candidate_id = String::new();
    tracing::subscriber::with_default(captured.clone(), || {
        let mut page = event(&store, 1, SECRET, SECRET, 1);
        let SourceEvent::Observation { observation, .. } = &mut page.events[0] else {
            unreachable!()
        };
        observation.context.value =
            json!({"session": {"requests": [SECRET]}, "credential": SECRET});
        let candidate = apply(&adapter, &active, &page);
        candidate_id = hex(&candidate.fingerprint);
        let claimed = claim(&adapter, &candidate);
        let prepared = PreparationRecord::Ready(prepared_review(
            SECRET,
            json!({"prompt": SECRET}),
            json!({"body": SECRET}),
            Default::default(),
            EvaluationBudget::default(),
        ));
        for _ in 0..20 {
            adapter.record_preparation(&claimed, &prepared).unwrap();
            adapter.load_recovery(None, 10).unwrap();
            assert!(matches!(
                adapter.claim_due(&candidate, 100_000, true).unwrap(),
                CandidateClaim::Stale
            ));
        }
        adapter
            .record_dispatch_intent(&claimed, 100_000, Duration::ZERO, &review_attempt(1))
            .unwrap();
        adapter
            .record_attempt_failure(
                &claimed,
                &review_attempt(1),
                AttemptFailure::Unknown,
                &EvaluationDiagnostic {
                    code: antiburn_checks::engine::EvaluationFailureCode::DeliveryUnknown,
                    detail: SECRET.into(),
                },
            )
            .unwrap();
        adapter
            .record_evaluation_failure(
                &claimed,
                &EvaluationDiagnostic {
                    code: antiburn_checks::engine::EvaluationFailureCode::DeliveryUnknown,
                    detail: SECRET.into(),
                },
            )
            .unwrap();
    });
    for event in [
        "burn_check_source_page_committed",
        "burn_check_candidate_claimed",
        "burn_check_preparation_committed",
        "burn_check_dispatch_intent_committed",
        "burn_check_attempt_failed",
        "burn_check_candidate_settled",
    ] {
        assert_eq!(captured.events(event).len(), 1, "{event}");
    }
    assert_eq!(captured.events("burn_check_recovery_page").len(), 1);
    let failure = &captured.events("burn_check_attempt_failed")[0];
    assert_eq!(failure.level, tracing::Level::WARN);
    assert_eq!(failure.fields["status"], "\"unknown\"");
    assert_eq!(failure.fields["retry_allowed"], "false");
    for name in ["generation", "claim_fence", "attempt_number", "elapsed_ms"] {
        assert!(failure.fields.contains_key(name), "{name}");
    }
    let logs = captured.text();
    for private in [
        SECRET,
        &candidate_id,
        "/repo",
        "owner-1",
        "credential",
        "prompt",
        "body",
    ] {
        assert!(!logs.contains(private), "{private}: {logs}");
    }
}

#[test]
fn tracing_reports_unknown_restart_once_without_resend_or_empty_poll_events() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, SECRET, SECRET, 1));
    let claimed = claim(&adapter, &candidate);
    adapter
        .dispatch_review(&claimed, 100_000, Duration::ZERO, 1)
        .unwrap();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        let restarted =
            StoreCheckStore::new(store.clone(), SECRET.into(), active.id.scope.clone()).unwrap();
        restarted.register_activation(&active).unwrap();
        for _ in 0..100 {
            assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
        }
        let _ =
            StoreCheckStore::new(store.clone(), SECRET.into(), active.id.scope.clone()).unwrap();
    });
    assert_eq!(captured.events("burn_check_delivery_unknown").len(), 1);
    assert_eq!(
        captured.events("burn_check_delivery_unknown")[0].fields["automatic_resend"],
        "false"
    );
    assert!(captured.events("burn_check_recovery_page").is_empty());
    assert!(!captured.text().contains(SECRET));
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_evaluation_attempt WHERE outcome = 'unknown'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn tracing_bounds_deferred_acceptance_and_records_the_commit_without_answer_content() {
    let (store, adapter, active) = fixture();
    let allowed = Arc::new(AtomicBool::new(false));
    let gate = allowed.clone();
    let adapter = Arc::new(
        Arc::try_unwrap(adapter)
            .ok()
            .unwrap()
            .with_publication_gate(Arc::new(move |_| gate.load(Ordering::Acquire))),
    );
    let candidate = apply(&adapter, &active, &event(&store, 1, SECRET, SECRET, 1));
    let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.9);
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for _ in 0..100 {
            assert_eq!(
                adapter.accept(&assessment).unwrap(),
                AcceptanceOutcome::Deferred
            );
        }
        allowed.store(true, Ordering::Release);
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted
        );
        for _ in 0..100 {
            assert_eq!(
                adapter.accept(&assessment).unwrap(),
                AcceptanceOutcome::AlreadyAccepted
            );
        }
    });
    let events = captured.events("burn_check_acceptance");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].fields["status"], "\"deferred\"");
    assert_eq!(events[0].fields["reason"], "\"publication_gate\"");
    assert_eq!(events[1].fields["status"], "\"accepted\"");
    assert!(!captured.text().contains(SECRET));
}

#[test]
fn tracing_recovery_rejection_omits_invalid_saved_data_and_does_not_repeat() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, SECRET, SECRET, 1));
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET frozen_input_json = ?1 WHERE candidate_id = ?2",
            params![
                json!({"private_canary": SECRET}).to_string(),
                hex(&candidate.fingerprint)
            ],
        )
        .unwrap();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for _ in 0..100 {
            assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
        }
    });
    let events = captured.events("burn_check_recovery_rejected");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].level, tracing::Level::WARN);
    assert_eq!(
        events[0].fields["reason"],
        "\"saved_work_invalid_or_incompatible\""
    );
    assert!(!captured.text().contains(SECRET));
    assert_eq!(
        store
            .lock()
            .query_row("SELECT state FROM burn_check_candidate", [], |row| row
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "not_assessed"
    );
}

#[test]
fn tracing_deferred_acceptance_keeps_a_fixed_memory_bound() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, SECRET, SECRET, 1));
    let mut claimed = claim(&adapter, &candidate);
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for index in 0..256_u16 {
            claimed.id.0[..2].copy_from_slice(&index.to_le_bytes());
            adapter.trace_acceptance(&claimed, "deferred", "publication_gate", Duration::ZERO);
        }
    });
    assert_eq!(
        adapter.deferred_traces.lock().unwrap().len(),
        antiburn_checks::engine::MAX_PENDING_CANDIDATES
    );
    assert_eq!(
        captured.events("burn_check_acceptance").len(),
        antiburn_checks::engine::MAX_PENDING_CANDIDATES
    );
}

#[test]
fn tracing_repeated_engine_recovery_sweeps_do_not_repeat_nonempty_or_empty_pages() {
    let (store, adapter, active) = fixture();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    engine
        .apply_source_page(&active, &event(&store, 1, SECRET, SECRET, 1))
        .unwrap();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for _ in 0..100 {
            engine.recover_pending().unwrap();
        }
        assert_eq!(captured.events("burn_check_recovery_page").len(), 1);
        engine
            .apply_source_page(&active, &event(&store, 2, "PRIVATE_NEXT_RECORD", SECRET, 1))
            .unwrap();
        for _ in 0..100 {
            engine.recover_pending().unwrap();
        }
    });
    let events = captured.events("burn_check_recovery_page");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].fields["recovered"], "1");
    assert_eq!(events[1].fields["recovered"], "2");
    assert!(!captured.text().contains("PRIVATE_"));
    assert!(
        adapter.recovery_traces.lock().unwrap().len()
            <= antiburn_checks::engine::MAX_PENDING_CANDIDATES
    );
}

fn decision_result(active: &EffectiveActivation, attempt: &DecisionAttempt) -> DecisionResult {
    let declaration = active
        .compiled
        .decisions
        .iter()
        .find(|decision| decision.name == attempt.decision)
        .unwrap();
    let question = &declaration.outputs[0];
    DecisionResult::new(
        declaration,
        &active.compiled.question_requirements(),
        vec![
            NamedAnswer::new(
                &question.name,
                QuestionResult::Choice(
                    ChoiceResult::new(
                        &question.question,
                        "yes",
                        Some(BTreeMap::from([("yes".into(), 0.99), ("no".into(), 0.01)])),
                        None,
                    )
                    .unwrap(),
                ),
            )
            .unwrap(),
        ],
        attempt.number,
    )
    .unwrap()
}

#[test]
fn tracing_correlates_interleaved_checks_and_decisions_without_private_names_or_inputs() {
    use crate::store::burn_checks::trace_refs;
    let source = CHECK.replace("actions:\n", "  private_decision:\n    input: {activity: available}\n    output:\n      verdict:\n        type: choice\n        question: PRIVATE_QUESTION\n        criteria: {yes: Yes., no: No.}\nactions:\n");
    let store = Arc::new(Store::open_in_memory(Path::new("/PRIVATE_TRACE_STORE")).unwrap());
    let (store, adapter, template) = fixture_with_definition(store, &source);
    let mut first = template.clone();
    first.id.definition_id = "private-first-check".into();
    let mut second = template;
    second.id.definition_id = "private-second-check".into();
    for active in [&first, &second] {
        adapter
            .activate(
                &active.id.definition_id,
                &LifecycleTransition::Activated(active.clone()),
                Some(active),
            )
            .unwrap();
    }
    let page = event(&store, 1, SECRET, SECRET, 1);
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
        for active in [&first, &second] {
            engine.apply_source_page(active, &page).unwrap();
        }
        let records = adapter.load_recovery(None, 10).unwrap();
        let claimed = [&first, &second].map(|active| {
            let candidate = records
                .iter()
                .find_map(|record| match &record.work {
                    RecoveryWork::Pending { candidate, .. }
                        if candidate.activation == active.id =>
                    {
                        Some(candidate)
                    }
                    _ => None,
                })
                .unwrap();
            let claimed = claim(&adapter, candidate);
            assert_eq!(claimed.fence, 1);
            let prepared = prepare_inputs(
                active
                    .compiled
                    .decisions
                    .iter()
                    .map(|decision| DecisionInput {
                        decision: decision.name.clone(),
                        input: json!({"credential": SECRET}),
                    })
                    .collect(),
                EvaluationBudget::default(),
                SECRET,
            );
            adapter
                .record_preparation(&claimed, &PreparationRecord::Ready(prepared))
                .unwrap();
            claimed
        });
        let mut results = [Vec::new(), Vec::new()];
        for decision in ["review", "private_decision"] {
            for (index, active) in [&first, &second].into_iter().enumerate() {
                let attempt = DecisionAttempt::new(decision, 1).unwrap();
                adapter
                    .record_dispatch_intent(&claimed[index], 100_000, Duration::ZERO, &attempt)
                    .unwrap();
                let result = decision_result(active, &attempt);
                trace_refs::provider_attempt_completed(
                    &claimed[index].candidate,
                    &attempt,
                    &Ok(result.clone()),
                    Duration::ZERO,
                );
                adapter
                    .record_decision_answer(&claimed[index], &attempt, &result)
                    .unwrap();
                results[index].push(result);
            }
        }
        for ((active, claimed), results) in [&first, &second].into_iter().zip(claimed).zip(results)
        {
            let evaluated = EvaluatedAssessment::new(
                &active.compiled.decisions,
                &active.compiled.question_requirements(),
                results,
            )
            .unwrap();
            let actions = plan_status_actions(
                &active.compiled.actions,
                &claimed.candidate.context.value,
                &evaluated,
                claimed.prior_status,
            )
            .unwrap();
            assert_eq!(
                adapter
                    .accept(&AssessmentAcceptance {
                        claimed,
                        evaluated,
                        actions
                    })
                    .unwrap(),
                AcceptanceOutcome::Accepted
            );
        }
    });
    let source = captured.events("burn_check_source_page_committed");
    let dispatch = captured.events("burn_check_dispatch_intent_committed");
    let provider = captured.events("burn_check_provider_attempt_completed");
    let answers = captured.events("burn_check_answer_committed");
    let accepted = captured.events("burn_check_acceptance");
    assert_eq!(source.len(), 2);
    assert_ne!(
        source[0].fields["activation_ref"],
        source[1].fields["activation_ref"]
    );
    assert_eq!(
        (
            dispatch.len(),
            provider.len(),
            answers.len(),
            accepted.len()
        ),
        (4, 4, 4, 2)
    );
    let decisions = dispatch
        .iter()
        .map(|event| &event.fields["decision_ref"])
        .collect::<BTreeSet<_>>();
    assert_eq!(decisions.len(), 4);
    for ((dispatch, provider), answer) in dispatch.iter().zip(&provider).zip(&answers) {
        for field in ["activation_ref", "candidate_ref", "decision_ref"] {
            assert_eq!(dispatch.fields[field], provider.fields[field]);
            assert_eq!(dispatch.fields[field], answer.fields[field]);
        }
        assert_eq!(dispatch.fields["generation"], "1");
        assert_eq!(dispatch.fields["claim_fence"], "1");
        assert!(accepted.iter().any(|event| event.fields["candidate_ref"]
            == dispatch.fields["candidate_ref"]
            && event.fields["activation_ref"] == dispatch.fields["activation_ref"]));
        assert!(
            source
                .iter()
                .any(|event| event.fields["activation_ref"] == dispatch.fields["activation_ref"])
        );
    }
    for private in [
        SECRET,
        "private-first-check",
        "private-second-check",
        "private_decision",
        "PRIVATE_QUESTION",
        "/repo",
        "credential",
    ] {
        assert!(!captured.text().contains(private), "{private}");
    }
}

#[test]
fn tracing_report_and_runtime_share_only_the_logical_check_reference() {
    use crate::insights_report::installed_checks::{
        InstalledCheckAvailability, InstalledCheckDefinition, InstalledCheckScope,
        InstalledCheckSource, read_installed_checks,
    };
    use crate::store::burn_checks::trace_refs;
    let (store, _adapter, mut active) = fixture();
    active.id.definition_id = "private-report-check".into();
    let definition = InstalledCheckDefinition {
        effective_id: "PRIVATE_EFFECTIVE_SCOPE".into(),
        check_id: active.id.definition_id.clone(),
        activation_scope_key: StoreCheckStore::activation_scope_key(&active.id.scope),
        generation: Some(1),
        definition_revision: hex(&active.revision),
        name: SECRET.into(),
        aliases: Vec::new(),
        description: None,
        is_local: active.compiled.is_local(),
        decision_name: None,
        trigger: None,
        source: InstalledCheckSource {
            scope: InstalledCheckScope::Global,
            path: Some("/PRIVATE_CHECK_PATH".into()),
        },
        availability: InstalledCheckAvailability::Available,
        diagnostic: None,
        diagnostics: Vec::new(),
        definition_sources: Vec::new(),
    };
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        read_installed_checks(&store, &[definition]).unwrap();
    });
    let events = captured.events("installed_check_report_state");
    assert_eq!(events.len(), 1);
    let check_ref = trace_refs::check_ref(&active.id.definition_id);
    assert_eq!(events[0].fields["check_ref"], format!("{check_ref:?}"));
    assert_ne!(
        events[0].fields["report_ref"],
        events[0].fields["check_ref"]
    );
    assert_ne!(trace_refs::activation_ref(&active.id), check_ref);
    assert!(!captured.text().contains("PRIVATE_"));
    assert!(!captured.text().contains("private-report-check"));
}

#[test]
fn tracing_engine_settlement_keeps_sqlite_cause_before_the_adapter_erases_its_type() {
    use crate::store::burn_checks::trace_refs;
    let (store, adapter, active) = fixture();
    let evaluator = Arc::new(RecordingEvaluator(Mutex::new(Vec::new())));
    let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
    engine
        .apply_source_page(&active, &event(&store, 1, SECRET, SECRET, 1))
        .unwrap();
    let RecoveryWork::Pending { candidate, .. } =
        adapter.load_recovery(None, 10).unwrap().remove(0).work
    else {
        panic!("pending candidate required")
    };
    store.lock().execute_batch("CREATE TRIGGER private_settlement_failure BEFORE INSERT ON burn_check_assessment_history
        BEGIN SELECT RAISE(ABORT, 'PRIVATE_SQLITE_BODY /PRIVATE_PATH'); END;").unwrap();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
            panic!("candidate must start")
        };
        let error = poll_task_result(task).unwrap_err();
        assert_eq!(error.attempts, 1);
        assert_eq!(error.new_intents, 1);
        assert!(error.source.message.contains("PRIVATE_SQLITE_BODY"));
        assert_eq!(
            store
                .lock()
                .query_row("SELECT state FROM burn_check_candidate", [], |row| row
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
            "answered"
        );
        for _ in 0..100 {
            assert!(engine.recover_pending().is_err());
        }
        assert_eq!(
            captured.events("burn_check_store_operation_failed").len(),
            1
        );
        store
            .lock()
            .execute_batch("DROP TRIGGER private_settlement_failure")
            .unwrap();
        engine.recover_pending().unwrap();
    });
    let events = captured.events("burn_check_store_operation_failed");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].level, tracing::Level::ERROR);
    assert_eq!(events[0].fields["stage"], "\"acceptance\"");
    assert_eq!(events[0].fields["reason"], "\"sqlite\"");
    assert_eq!(events[0].fields["sqlite_code"], "1811");
    assert_eq!(
        events[0].fields["activation_ref"],
        format!("{:?}", trace_refs::activation_ref(&active.id))
    );
    assert_eq!(
        events[0].fields["candidate_ref"],
        format!("{:?}", trace_refs::candidate_ref(&candidate))
    );
    assert!(!captured.text().contains("PRIVATE_"));
    assert_eq!(*evaluator.0.lock().unwrap(), vec![1]);
    assert_eq!(
        store
            .lock()
            .query_row("SELECT state FROM burn_check_candidate", [], |row| row
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "accepted"
    );
    assert!(adapter.store_failure_traces.is_empty());
}

#[test]
fn tracing_claim_update_keeps_sqlite_cause_and_logs_each_outage_once() {
    use crate::store::burn_checks::trace_refs;
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, SECRET, SECRET, 1));
    let reject_claim =
        "CREATE TRIGGER private_claim_failure BEFORE UPDATE OF state ON burn_check_candidate
        WHEN OLD.state = 'pending' AND NEW.state = 'claimed'
        BEGIN SELECT RAISE(ABORT, 'PRIVATE_CLAIM_ERROR /PRIVATE_PATH'); END;";
    store.lock().execute_batch(reject_claim).unwrap();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for _ in 0..100 {
            assert!(adapter.claim_due(&candidate, 100_000, true).is_err());
        }
        assert_eq!(
            captured.events("burn_check_store_operation_failed").len(),
            1
        );
        assert_eq!(
            store
                .lock()
                .query_row("SELECT state FROM burn_check_candidate", [], |row| row
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
            "pending"
        );
        store
            .lock()
            .execute_batch("DROP TRIGGER private_claim_failure")
            .unwrap();
        let claimed = claim(&adapter, &candidate);
        adapter.release_pre_dispatch_claim(&claimed).unwrap();
        store.lock().execute_batch(reject_claim).unwrap();
        for _ in 0..100 {
            assert!(adapter.claim_due(&candidate, 100_000, true).is_err());
        }
    });
    let failures = captured.events("burn_check_store_operation_failed");
    assert_eq!(failures.len(), 2);
    for failure in failures {
        assert_eq!(failure.level, tracing::Level::ERROR);
        assert_eq!(failure.fields["stage"], "\"claim\"");
        assert_eq!(failure.fields["reason"], "\"sqlite\"");
        assert_eq!(failure.fields["sqlite_code"], "1811");
        assert_eq!(
            failure.fields["candidate_ref"],
            format!("{:?}", trace_refs::candidate_ref(&candidate))
        );
        assert!(!failure.fields.contains_key("claim_fence"));
    }
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    assert!(!captured.text().contains("PRIVATE_"));
    assert!(!captured.text().contains(SECRET));
}

#[test]
fn tracing_source_page_transaction_keeps_sqlite_cause_and_does_not_repeat_failed_polls() {
    use crate::store::burn_checks::trace_refs;
    let (store, adapter, active) = fixture();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    let first = event(&store, 1, SECRET, SECRET, 1);
    let before = adapter.load_source_progress(&active).unwrap();
    let reject_page = "CREATE TRIGGER private_page_failure BEFORE INSERT ON burn_check_observation
        BEGIN SELECT RAISE(ABORT, 'PRIVATE_PAGE_ERROR /PRIVATE_PATH'); END;";
    store.lock().execute_batch(reject_page).unwrap();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for _ in 0..100 {
            assert!(engine.apply_source_page(&active, &first).is_err());
        }
        assert_eq!(adapter.load_source_progress(&active).unwrap(), before);
        assert_eq!(
            store
                .lock()
                .query_row("SELECT COUNT(*) FROM burn_check_observation", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .lock()
                .query_row("SELECT COUNT(*) FROM burn_check_candidate", [], |row| row
                    .get::<_, u64>(
                    0
                ))
                .unwrap(),
            0
        );
        assert_eq!(
            captured.events("burn_check_store_operation_failed").len(),
            1
        );
        store
            .lock()
            .execute_batch("DROP TRIGGER private_page_failure")
            .unwrap();
        engine.apply_source_page(&active, &first).unwrap();
        store.lock().execute_batch(reject_page).unwrap();
        let second = event(&store, 2, "PRIVATE_NEXT_EVENT", SECRET, 1);
        for _ in 0..100 {
            assert!(engine.apply_source_page(&active, &second).is_err());
        }
    });
    let failures = captured.events("burn_check_store_operation_failed");
    assert_eq!(failures.len(), 2);
    for failure in failures {
        assert_eq!(failure.level, tracing::Level::ERROR);
        assert_eq!(failure.fields["stage"], "\"source_page\"");
        assert_eq!(failure.fields["reason"], "\"sqlite\"");
        assert_eq!(failure.fields["sqlite_code"], "1811");
        assert_eq!(
            failure.fields["activation_ref"],
            format!("{:?}", trace_refs::activation_ref(&active.id))
        );
        assert!(!failure.fields.contains_key("candidate_ref"));
    }
    assert_eq!(
        adapter
            .load_source_progress(&active)
            .unwrap()
            .publication_cursor,
        PublicationCursor(1)
    );
    assert_eq!(captured.events("burn_check_source_page_committed").len(), 1);
    assert!(!captured.text().contains("PRIVATE_"));
    assert!(!captured.text().contains(SECRET));
}

#[test]
fn tracing_publication_reader_keeps_sqlite_code_across_fresh_poll_readers() {
    use crate::published_evidence_reader::StorePublishedEvidenceReader;
    use crate::store::burn_checks::trace_refs;
    use antiburn_checks::source::{PublicationRead, PublishedEvidenceReader};
    let (store, _adapter, mut active) = fixture();
    active.id.definition_id = "private-reader-check".into();
    store
        .lock()
        .execute_batch(
            "ALTER TABLE native_tool_result_publication RENAME TO PRIVATE_PUBLICATION_TABLE",
        )
        .unwrap();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for _ in 0..100 {
            let reader = StorePublishedEvidenceReader::for_project(&store, "native", "/repo")
                .with_activation(&active.id);
            assert!(
                reader
                    .read_batch(PublicationRead {
                        after: PublicationCursor(0),
                        limit: 10
                    })
                    .is_err()
            );
        }
        assert_eq!(
            captured.events("burn_check_publication_read_failed").len(),
            1
        );
        store
            .lock()
            .execute_batch(
                "ALTER TABLE PRIVATE_PUBLICATION_TABLE RENAME TO native_tool_result_publication",
            )
            .unwrap();
        let reader = StorePublishedEvidenceReader::for_project(&store, "native", "/repo")
            .with_activation(&active.id);
        assert!(
            reader
                .read_batch(PublicationRead {
                    after: PublicationCursor(0),
                    limit: 10
                })
                .unwrap()
                .events
                .is_empty()
        );
        store
            .lock()
            .execute_batch(
                "ALTER TABLE native_tool_result_publication RENAME TO PRIVATE_PUBLICATION_TABLE",
            )
            .unwrap();
        assert!(
            reader
                .read_batch(PublicationRead {
                    after: PublicationCursor(0),
                    limit: 10
                })
                .is_err()
        );
    });
    let failures = captured.events("burn_check_publication_read_failed");
    assert_eq!(failures.len(), 2);
    for failure in failures {
        assert_eq!(failure.fields["stage"], "\"publication_read\"");
        assert_eq!(failure.fields["reason"], "\"sqlite\"");
        assert_eq!(failure.fields["sqlite_code"], "1");
        assert_eq!(
            failure.fields["activation_ref"],
            format!("{:?}", trace_refs::activation_ref(&active.id))
        );
    }
    assert!(!captured.text().contains("PRIVATE_"));
    assert!(!captured.text().contains("/repo"));
}

#[test]
fn tracing_publication_decode_failures_reset_only_after_each_activation_reads_a_successful_batch() {
    use crate::published_evidence_reader::StorePublishedEvidenceReader;
    use crate::store::burn_checks::trace_refs;
    use antiburn_checks::source::{PublicationRead, PublishedEvidenceReader};

    let (store, _adapter, mut first) = fixture();
    first.id.definition_id = "PRIVATE_FIRST_DECODE_CHECK".into();
    let mut second = first.clone();
    second.id.definition_id = "PRIVATE_SECOND_DECODE_CHECK".into();
    let captured = LogCapture::default();
    let read = |activation: &ActivationId, after| {
        StorePublishedEvidenceReader::for_project(&store, "native", "/repo")
            .with_activation(activation)
            .read_batch(PublicationRead {
                after: PublicationCursor(after),
                limit: 1,
            })
    };
    tracing::subscriber::with_default(captured.clone(), || {
        for _ in 0..100 {
            assert!(read(&first.id, u64::MAX).is_err());
            assert!(read(&second.id, u64::MAX).is_err());
        }
        assert_eq!(
            captured.events("burn_check_publication_read_failed").len(),
            2
        );
        assert!(read(&first.id, 0).unwrap().events.is_empty());
        for _ in 0..100 {
            assert!(read(&second.id, u64::MAX).is_err());
        }
        assert_eq!(
            captured.events("burn_check_publication_read_failed").len(),
            2
        );
        assert!(read(&first.id, u64::MAX).is_err());
        assert!(read(&second.id, 0).unwrap().events.is_empty());
        assert!(read(&second.id, u64::MAX).is_err());
    });
    let failures = captured.events("burn_check_publication_read_failed");
    assert_eq!(failures.len(), 4);
    for (failure, activation) in failures
        .iter()
        .zip([&first.id, &second.id, &first.id, &second.id])
    {
        assert_eq!(failure.level, tracing::Level::ERROR);
        assert_eq!(failure.fields["stage"], "\"publication_decode\"");
        assert_eq!(failure.fields["reason"], "\"runtime_invariant\"");
        assert_eq!(
            failure.fields["activation_ref"],
            format!("{:?}", trace_refs::activation_ref(activation))
        );
        assert_eq!(
            failure.fields["source_ref"],
            failure.fields["activation_ref"]
        );
    }
    assert!(!captured.text().contains("PRIVATE_"));
    assert!(!captured.text().contains("/repo"));
}
