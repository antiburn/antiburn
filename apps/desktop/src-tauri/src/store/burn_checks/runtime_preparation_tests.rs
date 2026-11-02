use super::*;
use antiburn_checks::engine::{EvaluationFailureCode, RunSettlement};
use antiburn_checks::fit::{FitReport, PreparationFailureCode};

pub(super) struct PreparationEvaluator {
    prepare_count: std::sync::atomic::AtomicUsize,
    recorded: Mutex<Vec<PreparationRecord>>,
    sent: Mutex<Vec<PreparedAssessment>>,
    reject: bool,
    incompatible: bool,
}

impl PreparationEvaluator {
    pub(super) fn new(reject: bool, incompatible: bool) -> Self {
        Self {
            prepare_count: std::sync::atomic::AtomicUsize::new(0),
            recorded: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            reject,
            incompatible,
        }
    }
}

impl DecisionEvaluator for PreparationEvaluator {
    fn preparation_recorded(&self, preparation: &PreparationRecord) {
        self.recorded.lock().unwrap().push(preparation.clone());
    }

    fn prepare(
        &self,
        _: &EffectiveActivation,
        inputs: Vec<DecisionInput>,
        budget: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        self.prepare_count.fetch_add(1, Ordering::SeqCst);
        if self.reject {
            Err(PreparationFailure { code: PreparationFailureCode::NoFit, detail: "The complete request exceeds the local model budget after permitted reductions.".into(), report: Some(FitReport::default()), context: Default::default() })
        } else {
            Ok(prepare_inputs(inputs, budget, "fixture-v1"))
        }
    }

    fn validate_prepared(
        &self,
        active: &EffectiveActivation,
        prepared: &PreparedAssessment,
        _: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        if self.incompatible {
            Err(PreparationFailure {
                code: PreparationFailureCode::IncompatibleProfile,
                detail: "The saved request uses an incompatible preparation profile.".into(),
                report: None,
                context: Default::default(),
            })
        } else {
            validate_fixture_preparation(active, prepared, "fixture-v1")
        }
    }

    fn evaluate_prepared<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        prepared: &'a PreparedAssessment,
        _: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        self.sent.lock().unwrap().push(prepared.clone());
        Box::pin(async {
            Err(EvaluationFailure {
                code: EvaluationFailureCode::AuthenticationRejected,
                kind: AttemptFailure::Permanent,
                detail: "Synthetic authentication failure.".into(),
                attempts: 1,
            })
        })
    }
}

fn run(
    engine: &CheckEngine<StoreCheckStore, PreparationEvaluator>,
    active: &EffectiveActivation,
) -> RunSettlement {
    let StartOutcome::Started(task) = engine.start_one_due(active, 100_000).unwrap() else {
        panic!("candidate must start");
    };
    poll_settlement(task)
}

#[test]
fn model_rejection_records_exact_failure_before_terminal_settlement() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    let mut failure = PreparationFailure {
        code: PreparationFailureCode::NoFit,
        detail: "The complete review request does not fit.".into(),
        report: Some(FitReport {
            omissions: vec![antiburn_checks::fit::FitOmission {
                path: "activity".into(),
                strategy: antiburn_checks::fit::FitStrategy::Truncate,
                original_count: 128,
                retained_count: 64,
            }],
        }),
        context: Default::default(),
    };
    failure.context.decision = Some("review".into());
    let record = PreparationRecord::Rejected(failure.clone());
    for _ in 0..2 {
        assert_eq!(
            adapter.record_preparation(&claimed, &record).unwrap(),
            ClaimOutcome::Current
        );
        assert_eq!(
            adapter.load_preparation(&claimed).unwrap(),
            Some(record.clone())
        );
    }
    let mut changed = failure.clone();
    changed.detail = "A different failure must not replace the saved rejection.".into();
    assert!(
        adapter
            .record_preparation(&claimed, &PreparationRecord::Rejected(changed))
            .is_err()
    );
    assert!(
        adapter
            .record_preparation(
                &claimed,
                &PreparationRecord::Local(LocalPreparation::new(1001))
            )
            .is_err()
    );
    assert!(
        adapter
            .record_dispatch_intent(&claimed, 1001, Duration::from_secs(60), &review_attempt(1))
            .is_err()
    );
    assert_eq!(
        store
            .lock()
            .query_row("SELECT state FROM burn_check_candidate", [], |row| row
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "claimed"
    );
    for table in [
        "burn_check_evaluation_attempt",
        "burn_check_cooldown",
        "burn_check_current_status",
        "burn_check_acceptance_watermark",
        "burn_check_assessment_history",
    ] {
        assert_eq!(
            store
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
    assert_eq!(
        adapter
            .record_preparation_rejection(&claimed, &failure)
            .unwrap(),
        ClaimOutcome::Current
    );
    assert_eq!(adapter.load_preparation(&claimed).unwrap(), Some(record));
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT outcome FROM burn_check_assessment_history",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "not_assessed"
    );
}

#[test]
fn no_fit_has_zero_attempts_no_cooldown_no_status_and_does_not_block_later_candidates() {
    let (store, adapter, active) = fixture();
    let evaluator = Arc::new(PreparationEvaluator::new(true, false));
    let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    engine
        .apply_source_page(&active, &event(&store, 2, "two", "branch", 1))
        .unwrap();
    for _ in 0..2 {
        let settled = run(&engine, &active);
        assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
        assert_eq!(settled.attempts, 0);
    }
    assert!(matches!(
        engine.start_one_due(&active, 100_000).unwrap(),
        StartOutcome::NotDue
    ));
    assert!(evaluator.sent.lock().unwrap().is_empty());
    for table in [
        "burn_check_evaluation_attempt",
        "burn_check_cooldown",
        "burn_check_current_status",
        "burn_check_acceptance_watermark",
    ] {
        assert_eq!(
            store
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
    let records = store
        .lock()
        .prepare("SELECT preparation_json FROM burn_check_candidate ORDER BY trigger_position")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(evaluator.recorded.lock().unwrap().len(), 2);
    for saved in records {
        let PreparationRecord::Rejected(failure) = serde_json::from_str(&saved).unwrap() else {
            panic!("local failure must be typed");
        };
        assert_eq!(failure.code, PreparationFailureCode::NoFit);
    }
    let restarted = StoreCheckStore::new(store, "owner-2".into(), active.id.scope.clone()).unwrap();
    restarted.register_activation(&active).unwrap();
    assert!(restarted.load_recovery(None, 10).unwrap().is_empty());
}

#[test]
fn jev_budget_rejection_occurs_before_the_engine_writes_dispatch_intent() {
    let source = CHECK.replace(
        "question: Is this useful?",
        &format!("question: {}", "q".repeat(50_000)),
    );
    let store =
        Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-preparation-test")).unwrap());
    let (store, adapter, active) = fixture_with_definition(store, &source);
    let connection = crate::jev::config::SystemOneConnection::default();
    let mut capabilities = connection.capabilities().unwrap();
    capabilities.total_input_tokens.value = Some(8192);
    let evaluator =
        crate::jev::decision_evaluator::TypeSafeDecisionEvaluator::from_selected_snapshot(
            connection,
            Some("synthetic-key".into()),
            capabilities,
        )
        .unwrap()
        .with_usage(Arc::new(|_, _, _| {
            panic!("local no-fit must not reach provider usage settlement")
        }))
        .with_diagnostics(Arc::new(|_| {
            panic!("local no-fit must not reach provider dispatch")
        }));
    let engine = CheckEngine::new(adapter, Arc::new(evaluator));
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
        panic!("local rejection must settle");
    };
    let settled = poll_settlement(task);
    assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
    assert_eq!(settled.attempts, 0);
    for table in [
        "burn_check_evaluation_attempt",
        "burn_check_cooldown",
        "burn_check_current_status",
        "burn_check_decision_usage",
    ] {
        assert_eq!(
            store
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
    let saved: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let PreparationRecord::Rejected(failure) = serde_json::from_str(&saved).unwrap() else {
        panic!("Jev preflight rejection must be typed");
    };
    assert_eq!(failure.code, PreparationFailureCode::NoFit);
}

#[tokio::test]
async fn native_alias_unknown_delivery_stays_terminal_after_store_reopen() {
    use crate::jev::config::SystemOneConnection;
    use crate::jev::decision_evaluator::TypeSafeDecisionEvaluator;
    use std::io::{Read, Write};

    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(directory.path()).unwrap());
    let (store, adapter, active) = fixture_with_definition(store, CHECK);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let count = socket.read(&mut chunk).unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .map(|value| value.trim().parse().unwrap())
                    })
                    .unwrap();
                if bytes.len() >= end + 4 + length {
                    assert!(headers.contains("typesafe-danger-zone-long-context: true"));
                    break;
                }
            }
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n")
            .unwrap();
        drop(socket);
        listener.set_nonblocking(true).unwrap();
        listener
    });
    let usage = Arc::new(Mutex::new(Vec::new()));
    let recorded = usage.clone();
    let connection = SystemOneConnection::default();
    let caps = connection.capabilities().unwrap();
    let evaluator = Arc::new(
        TypeSafeDecisionEvaluator::from_selected_snapshot(
            connection,
            Some("synthetic-key".into()),
            caps,
        )
        .unwrap()
        .with_native_test_endpoint(endpoint)
        .with_usage(Arc::new(move |_, _, observation| {
            recorded.lock().unwrap().push(observation);
            Ok(())
        })),
    );
    let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
        panic!("Native candidate must start");
    };
    let result = task.await.unwrap();
    assert_eq!(result.attempts, 1);
    assert!(
        matches!(result.outcome, RunOutcome::Failed(_)),
        "{result:?}"
    );
    assert_eq!(*usage.lock().unwrap(), [None]);
    let preparation: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(preparation.contains(antiburn_local::analysis::jev::NATIVE_MODEL_ALIAS));
    drop(engine);
    drop(adapter);
    drop(store);
    let reopened = Arc::new(Store::open(directory.path()).unwrap());
    let adapter = Arc::new(
        StoreCheckStore::new(reopened.clone(), "owner-2".into(), active.id.scope.clone()).unwrap(),
    );
    adapter.register_activation(&active).unwrap();
    assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
    let engine = CheckEngine::new(adapter, evaluator);
    assert!(matches!(
        engine.start_one_due(&active, 200_000).unwrap(),
        StartOutcome::NotDue
    ));
    let saved: String = reopened
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(saved, preparation);
    let attempts: i64 = reopened
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(attempts, 1);
    let (candidate_state, attempt_state, outcome, diagnostic): (String, String, String, String) =
        reopened
            .lock()
            .query_row(
                "SELECT c.state, a.state, a.outcome, c.diagnostic_json
               FROM burn_check_candidate c
               JOIN burn_check_evaluation_attempt a ON a.candidate_id = c.candidate_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
    assert_eq!(candidate_state, "execution_failed");
    assert_eq!(attempt_state, "settled");
    assert_eq!(outcome, "unknown");
    let diagnostic: antiburn_checks::engine::EvaluationDiagnostic =
        serde_json::from_str(&diagnostic).unwrap();
    assert_eq!(diagnostic.code, EvaluationFailureCode::DeliveryUnknown);
    let listener = server.join().unwrap();
    assert!(
        matches!(listener.accept(),Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[test]
fn cloudflare_request_rules_fail_before_dispatch_intent_cooldown_or_usage() {
    use crate::jev::config::{
        SystemOneConnection, SystemOneEndpoint, SystemOneProvider, SystemOneResponseMode,
    };
    use crate::jev::decision_evaluator::TypeSafeDecisionEvaluator;

    let question_id = "a".repeat(101);
    let source = CHECK.replace("verdict", &question_id);
    for (provider, endpoint, expected) in [
        (
            SystemOneProvider::Cloudflare,
            SystemOneEndpoint::CloudflareAccount("synthetic-account".into()),
            PreparationFailureCode::InvalidRequest,
        ),
        (
            SystemOneProvider::Custom,
            SystemOneEndpoint::ExactUrl(
                crate::jev_cloudflare::account_endpoint("synthetic-account", "clef").unwrap(),
            ),
            PreparationFailureCode::InvalidRequest,
        ),
        (
            SystemOneProvider::Custom,
            SystemOneEndpoint::ExactUrl("https://synthetic.example/systemone".into()),
            PreparationFailureCode::ProviderUnavailable,
        ),
    ] {
        let connection = SystemOneConnection {
            provider,
            endpoint,
            model: "clef".into(),
            model_revision: None,
            response_mode: SystemOneResponseMode::CloudflareEnvelope,
            credential: None,
            revision: 1,
            context_override: None,
        };
        let capabilities = connection.capabilities().unwrap();
        let evaluator = TypeSafeDecisionEvaluator::from_selected_snapshot(
            connection,
            Some("synthetic-key".into()),
            capabilities,
        )
        .unwrap()
        .with_usage(Arc::new(|_, _, _| {
            panic!("local preflight rejection must not reach usage settlement")
        }))
        .with_diagnostics(Arc::new(|_| {
            panic!("local preflight rejection must not reach provider dispatch")
        }));
        let store =
            Arc::new(Store::open_in_memory(Path::new("/synthetic/cloudflare-preflight")).unwrap());
        let (store, adapter, active) = fixture_with_definition(store, &source);
        let engine = CheckEngine::new(adapter, Arc::new(evaluator));
        engine
            .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
            .unwrap();
        let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
            panic!("preflight rejection must settle locally");
        };
        let settled = poll_settlement(task);
        assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
        assert_eq!(settled.attempts, 0);
        for table in [
            "burn_check_evaluation_attempt",
            "burn_check_cooldown",
            "burn_check_current_status",
            "burn_check_decision_usage",
        ] {
            assert_eq!(
                store
                    .lock()
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                0,
                "{provider:?}: {table}"
            );
        }
        let saved: String = store
            .lock()
            .query_row(
                "SELECT preparation_json FROM burn_check_candidate",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let PreparationRecord::Rejected(failure) = serde_json::from_str(&saved).unwrap() else {
            panic!("preflight rejection must retain a typed preparation failure");
        };
        assert_eq!(failure.code, expected);
        if expected == PreparationFailureCode::InvalidRequest {
            let field = format!("decisions.review.output.{question_id}");
            assert_eq!(failure.context.field.as_deref(), Some(field.as_str()));
            assert_eq!(failure.context.decision.as_deref(), Some("review"));
        } else {
            assert_eq!(failure.context, Default::default());
            assert_eq!(failure.report, None);
        }
    }
}

#[test]
fn fitting_resource_limit_is_persisted_before_any_dispatch_or_cooldown() {
    let source = CHECK.replace(
        "input: {activity: \"${{ message.tool.result.status }}\"}",
        "input: {activity: \"${{ message.tool.result.content }}\"}\n    fit:\n      activity: truncate",
    );
    let store =
        Arc::new(Store::open_in_memory(Path::new("/tmp/antiburn-resource-limit-test")).unwrap());
    let (store, adapter, active) = fixture_with_definition(store, &source);
    let evaluator = crate::jev::decision_evaluator::TypeSafeDecisionEvaluator::new(
        crate::jev::client::TypeSafeClient::new("synthetic-key".into()).unwrap(),
    )
    .with_usage(Arc::new(|_, _, _| {
        panic!("resource rejection must not reach a provider attempt")
    }));
    let engine = CheckEngine::new(adapter, Arc::new(evaluator));
    let mut page = event(&store, 1, "one", "branch", 1);
    let SourceEvent::Observation { observation, .. } = &mut page.events[0] else {
        panic!("fixture must publish a tool result");
    };
    let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
        panic!("fixture must contain a tool result");
    };
    message.value["tool"]["result"]["content"] = json!("x".repeat(1024 * 1024));
    engine.apply_source_page(&active, &page).unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
        panic!("resource rejection must settle locally");
    };
    let settled = poll_settlement(task);
    assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
    assert_eq!(settled.attempts, 0);
    for table in [
        "burn_check_evaluation_attempt",
        "burn_check_cooldown",
        "burn_check_current_status",
        "burn_check_acceptance_watermark",
    ] {
        assert_eq!(
            store
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
    let saved: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let PreparationRecord::Rejected(failure) = serde_json::from_str(&saved).unwrap() else {
        panic!("resource rejection must be typed");
    };
    assert_eq!(
        failure.code,
        PreparationFailureCode::PreparationResourceLimit
    );
    assert_eq!(failure.context.field.as_deref(), Some("decisions.review"));
    let budget = failure.context.budget.as_ref().unwrap();
    assert_eq!(
        budget.kind,
        antiburn_checks::fit::PreparationBudgetKind::WorkBytes
    );
    assert_eq!(budget.maximum, 64 * 1024 * 1024);
    assert!(budget.observed > budget.maximum);
}

#[test]
fn safe_retry_after_restart_reuses_exact_preparation_and_persists_typed_failure() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    let mut expected_budget = EvaluationBudget::default();
    assert!(matches!(
        antiburn_checks::preparation::prepare_decision_input_with_budget(
            &active.compiled.decisions[0],
            &candidate,
            &mut expected_budget
        ),
        antiburn_checks::preparation::PreparationOutcome::Ready(_)
    ));
    let prepared = prepared_review(
        "fixture-v1",
        json!({"original": [3,4]}),
        json!({"exact": "saved"}),
        FitReport::default(),
        expected_budget.clone(),
    );
    adapter
        .record_preparation(&claimed, &PreparationRecord::Ready(prepared.clone()))
        .unwrap();
    let mut changed = prepared.clone();
    changed.decisions[0].input = json!({"changed": true});
    assert!(
        adapter
            .record_preparation(&claimed, &PreparationRecord::Ready(changed))
            .is_err()
    );
    adapter
        .dispatch_review(&claimed, 1_000, Duration::ZERO, 1)
        .unwrap();
    adapter
        .fail_review(
            &claimed,
            1,
            AttemptFailure::SafeTransient,
            &transient_diagnostic(),
        )
        .unwrap();
    let restarted = Arc::new(
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap(),
    );
    restarted.register_activation(&active).unwrap();
    let evaluator = Arc::new(PreparationEvaluator::new(false, false));
    let engine = CheckEngine::new(restarted, evaluator.clone());
    assert_eq!(engine.recover_pending().unwrap().candidates, 1);
    let settled = run(&engine, &active);
    assert_eq!(settled.attempts, 2);
    assert!(matches!(settled.outcome, RunOutcome::Failed(_)));
    assert_eq!(evaluator.prepare_count.load(Ordering::SeqCst), 0);
    assert!(evaluator.recorded.lock().unwrap().is_empty());
    let sent = evaluator.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].profile, prepared.profile);
    assert_eq!(sent[0].decisions, prepared.decisions);
    assert!(expected_budget.steps() > 0);
    assert_eq!(sent[0].evaluation_budget, expected_budget);
    let saved: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<PreparationRecord>(&saved).unwrap(),
        PreparationRecord::Ready(prepared)
    );
    let saved: String = store
        .lock()
        .query_row(
            "SELECT diagnostic_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<EvaluationDiagnostic>(&saved)
            .unwrap()
            .code,
        EvaluationFailureCode::AuthenticationRejected
    );
}

#[test]
fn recovery_rejects_incompatible_or_missing_preparation_without_a_new_attempt() {
    for saved in [false, true] {
        let (store, adapter, active) = fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let claimed = claim(&adapter, &candidate);
        if saved {
            let prepared = prepared_review(
                "old-profile",
                json!({}),
                json!({}),
                FitReport::default(),
                EvaluationBudget::default(),
            );
            adapter
                .record_preparation(&claimed, &PreparationRecord::Ready(prepared))
                .unwrap();
        }
        adapter
            .dispatch_review(&claimed, 1_000, Duration::ZERO, 1)
            .unwrap();
        adapter
            .fail_review(
                &claimed,
                1,
                AttemptFailure::SafeTransient,
                &transient_diagnostic(),
            )
            .unwrap();
        if !saved {
            store.lock().execute("UPDATE burn_check_candidate SET preparation_json = NULL WHERE candidate_id = ?1",
                [hex(&candidate.fingerprint)]).unwrap();
        }
        let restarted = Arc::new(
            StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap(),
        );
        restarted.register_activation(&active).unwrap();
        let evaluator = Arc::new(PreparationEvaluator::new(false, true));
        let engine = CheckEngine::new(restarted, evaluator.clone());
        engine.recover_pending().unwrap();
        if saved {
            let settled = run(&engine, &active);
            assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
            assert_eq!(settled.attempts, 1);
        } else {
            assert!(matches!(
                engine.start_one_due(&active, 100_000).unwrap(),
                StartOutcome::NotDue
            ));
        }
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert_eq!(evaluator.prepare_count.load(Ordering::SeqCst), 0);
        assert!(evaluator.recorded.lock().unwrap().is_empty());
        assert!(evaluator.sent.lock().unwrap().is_empty());
        let preparation: Option<String> = store
            .lock()
            .query_row(
                "SELECT preparation_json FROM burn_check_candidate",
                [],
                |row| row.get(0),
            )
            .unwrap();
        if saved {
            let PreparationRecord::Rejected(failure) =
                serde_json::from_str(&preparation.unwrap()).unwrap()
            else {
                panic!("recovery rejection must be typed");
            };
            assert_eq!(failure.code, PreparationFailureCode::IncompatibleProfile);
        } else {
            assert!(preparation.is_none());
        }
    }
}

#[test]
fn preparation_observation_follows_commit_and_never_repeats_after_later_storage_failure() {
    for stage in ["save", "rejection", "intent"] {
        let (store, adapter, active) = fixture();
        let evaluator = Arc::new(PreparationEvaluator::new(stage == "rejection", false));
        let mut engine = CheckEngine::new(adapter, evaluator.clone());
        engine
            .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
            .unwrap();
        store.lock().execute_batch(match stage {
            "save" => "CREATE TRIGGER reject_preflight BEFORE UPDATE OF preparation_json ON burn_check_candidate WHEN NEW.preparation_json IS NOT NULL BEGIN SELECT RAISE(ABORT, 'injected preparation failure'); END",
            "rejection" => "CREATE TRIGGER reject_preflight BEFORE INSERT ON burn_check_assessment_history BEGIN SELECT RAISE(ABORT, 'injected rejection failure'); END",
            "intent" => "CREATE TRIGGER reject_preflight BEFORE INSERT ON burn_check_evaluation_attempt BEGIN SELECT RAISE(ABORT, 'injected intent failure'); END",
            _ => unreachable!(),
        }).unwrap();
        assert_start_error(&engine, &active);
        assert_eq!(
            evaluator.recorded.lock().unwrap().len(),
            usize::from(stage != "save"),
            "{stage}"
        );
        assert!(evaluator.sent.lock().unwrap().is_empty());
        let saved: Option<String> = store
            .lock()
            .query_row(
                "SELECT preparation_json FROM burn_check_candidate",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(saved.is_some(), stage != "save");
        store
            .lock()
            .execute_batch("DROP TRIGGER reject_preflight")
            .unwrap();
        if stage == "intent" {
            let restarted = Arc::new(
                StoreCheckStore::new(
                    store.clone(),
                    "storage-restart".into(),
                    active.id.scope.clone(),
                )
                .unwrap(),
            );
            restarted.register_activation(&active).unwrap();
            engine = CheckEngine::new(restarted, evaluator.clone());
            engine.recover_pending().unwrap();
        }
        run(&engine, &active);
        let records = evaluator.recorded.lock().unwrap();
        assert_eq!(records.len(), 1, "{stage}");
        let saved: String = store
            .lock()
            .query_row(
                "SELECT preparation_json FROM burn_check_candidate",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            records[0],
            serde_json::from_str::<PreparationRecord>(&saved).unwrap()
        );
    }
}

#[test]
fn unavailable_input_is_observed_without_calling_the_preparer_or_provider() {
    let store = Arc::new(
        Store::open_in_memory(Path::new("/tmp/antiburn-preparation-observer-test")).unwrap(),
    );
    let source = CHECK.replace("message.tool.result.status", "message.tool.result.content");
    let (store, adapter, active) = fixture_with_definition(store, &source);
    let evaluator = Arc::new(PreparationEvaluator::new(false, false));
    let engine = CheckEngine::new(adapter, evaluator.clone());
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    assert!(matches!(
        run(&engine, &active).outcome,
        RunOutcome::NotAssessed(_)
    ));
    assert_eq!(evaluator.prepare_count.load(Ordering::SeqCst), 0);
    assert!(evaluator.sent.lock().unwrap().is_empty());
    let records = evaluator.recorded.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert!(
        matches!(&records[0], PreparationRecord::Rejected(error) if error.code == PreparationFailureCode::InputUnavailable)
    );
}

#[test]
fn a_stale_preparation_claim_emits_no_observation() {
    struct RevokingEvaluator {
        store: Arc<Store>,
        observations: std::sync::atomic::AtomicUsize,
    }
    impl DecisionEvaluator for RevokingEvaluator {
        fn prepare(
            &self,
            _: &EffectiveActivation,
            inputs: Vec<DecisionInput>,
            budget: EvaluationBudget,
        ) -> Result<PreparedAssessment, PreparationFailure> {
            self.store
                .lock()
                .execute(
                    "UPDATE burn_check_candidate SET state = 'stale' WHERE state = 'claimed'",
                    [],
                )
                .unwrap();
            Ok(prepare_inputs(inputs, budget, "fixture-v1"))
        }

        fn preparation_recorded(&self, _: &PreparationRecord) {
            self.observations.fetch_add(1, Ordering::SeqCst);
        }

        fn validate_prepared(
            &self,
            _: &EffectiveActivation,
            _: &PreparedAssessment,
            _: PreparationValidationPurpose,
        ) -> Result<(), PreparationFailure> {
            panic!("a stale preparation must not validate")
        }
        fn evaluate_prepared<'a>(
            &'a self,
            _: &'a EffectiveActivation,
            _: &'a FrozenCandidate,
            _: &'a PreparedAssessment,
            _: &'a DecisionAttempt,
        ) -> DecisionFuture<'a> {
            panic!("A stale preparation must not dispatch")
        }
    }
    let (store, adapter, active) = fixture();
    let evaluator = Arc::new(RevokingEvaluator {
        store: store.clone(),
        observations: std::sync::atomic::AtomicUsize::new(0),
    });
    let engine = CheckEngine::new(adapter, evaluator.clone());
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    assert!(matches!(
        engine.start_one_due(&active, 100_000).unwrap(),
        StartOutcome::Stale
    ));
    assert_eq!(evaluator.observations.load(Ordering::SeqCst), 0);
}

#[test]
fn recovery_fences_changed_compiled_policy_even_with_the_same_activation_revision() {
    let (store, adapter, mut active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let claimed = claim(&adapter, &candidate);
    let evaluator = crate::jev::decision_evaluator::TypeSafeDecisionEvaluator::new(
        crate::jev::client::TypeSafeClient::new("synthetic-key".into()).unwrap(),
    )
    .with_usage(Arc::new(|_, _, _| {
        panic!("changed policy must not reach a provider attempt")
    }));
    let prepared = evaluator
        .prepare(
            &active,
            vec![DecisionInput {
                decision: "review".into(),
                input: json!({"activity": "error"}),
            }],
            EvaluationBudget::default(),
        )
        .unwrap();
    adapter
        .record_preparation(&claimed, &PreparationRecord::Ready(prepared))
        .unwrap();
    adapter
        .dispatch_review(&claimed, 1_000, Duration::ZERO, 1)
        .unwrap();
    adapter
        .fail_review(
            &claimed,
            1,
            AttemptFailure::SafeTransient,
            &transient_diagnostic(),
        )
        .unwrap();
    let antiburn_checks::decision::Question::Choice { question, .. } =
        &mut Arc::make_mut(&mut active.compiled).decisions[0].outputs[0].question
    else {
        unreachable!()
    };
    question.push_str(" Changed policy.");
    let restarted = Arc::new(
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap(),
    );
    restarted.register_activation(&active).unwrap();
    let engine = CheckEngine::new(restarted, Arc::new(evaluator));
    engine.recover_pending().unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
        panic!("changed preparation must settle locally");
    };
    let settled = poll_settlement(task);
    assert!(matches!(settled.outcome, RunOutcome::NotAssessed(_)));
    assert_eq!(settled.attempts, 1);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    let saved: String = store
        .lock()
        .query_row(
            "SELECT preparation_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let PreparationRecord::Rejected(failure) = serde_json::from_str(&saved).unwrap() else {
        panic!("policy mismatch must be typed");
    };
    assert_eq!(failure.code, PreparationFailureCode::IncompatibleProfile);
    assert_eq!(failure.context.decision, None);
    assert!(failure.detail.contains("incompatible preparation profile"));
    assert_eq!(settled.new_intents, 0);
}

#[test]
fn migration_adds_nullable_preparation_to_existing_candidates() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for migration in &crate::store::schema::MIGRATIONS[..76] {
        connection.execute_batch(migration).unwrap();
    }
    connection.pragma_update(None, "user_version", 76).unwrap();
    connection
        .execute_batch(crate::store::schema::MIGRATIONS[76])
        .unwrap();
    let columns = connection
        .prepare("PRAGMA table_info(burn_check_candidate)")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, bool>(3)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(columns.contains(&("preparation_json".into(), false)));
    assert!(columns.contains(&("diagnostic_json".into(), false)));
}
