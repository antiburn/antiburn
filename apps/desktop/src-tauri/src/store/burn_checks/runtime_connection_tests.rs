use super::*;

fn protected_state(store: &Store) -> Vec<String> {
    let connection = store.lock();
    [
        "burn_check_activation",
        "burn_check_observation",
        "burn_check_session_attachment",
        "burn_check_current_status",
        "burn_check_acceptance_watermark",
        "burn_check_cooldown",
    ]
    .into_iter()
    .map(|table| {
        let mut statement = connection
            .prepare(&format!("SELECT * FROM {table}"))
            .unwrap();
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                Ok((0..columns)
                    .map(|index| format!("{:?}", row.get_ref(index).unwrap()))
                    .collect::<Vec<_>>())
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        format!("{table}: {rows:?}")
    })
    .collect()
}

fn configured_live_fixture() -> (Arc<Store>, Arc<StoreCheckStore>, EffectiveActivation) {
    use crate::jev::config::{
        ContextLimitOverride, SystemOneConnection, SystemOneEndpoint, SystemOneProvider,
        SystemOneResponseMode,
    };
    let (store, adapter, active) = fixture();
    assert_eq!(active.id.scope.run, RunScope::Live);
    let connection = SystemOneConnection {
        provider: SystemOneProvider::Custom,
        endpoint: SystemOneEndpoint::ExactUrl("http://127.0.0.1:1/systemone".into()),
        model: "synthetic-model".into(),
        model_revision: None,
        response_mode: SystemOneResponseMode::Direct,
        credential: None,
        revision: 1,
        context_override: Some(ContextLimitOverride {
            total_input_tokens: Some(16_384),
            ..Default::default()
        }),
    };
    connection.validate().unwrap();
    store
        .set_internal_value_checked(
            "internal:smartChecksConnectionsV1",
            &json!({
                "activeId": "custom", "profiles": {"custom": connection}
            })
            .to_string(),
        )
        .unwrap();
    store
        .set_internal_value_checked("internal:smartChecksActiveConnectionV1", "custom")
        .unwrap();
    store
        .set_internal_value_checked("internal:smartChecksProviderMigrationV1", "1")
        .unwrap();
    store
        .lock()
        .execute(
            "DELETE FROM setting WHERE key = 'internal:yamlBurnChecksDiscoveryEnabledV1'",
            [],
        )
        .unwrap();
    (store, adapter, active)
}

#[test]
fn live_assessment_accepts_absent_yaml_preference_without_rewriting_it() {
    let (store, adapter, active) = configured_live_fixture();
    assert!(crate::jev::settings::yaml_discovery_enabled(&store));
    let connection = store.internal_value("internal:smartChecksConnectionsV1");
    store
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_yaml_preference_insert BEFORE INSERT ON setting
        WHEN NEW.key = 'internal:yamlBurnChecksDiscoveryEnabledV1'
        BEGIN SELECT RAISE(ABORT, 'the absent YAML preference must not be rewritten'); END;
        CREATE TRIGGER reject_yaml_preference_update BEFORE UPDATE ON setting
        WHEN NEW.key = 'internal:yamlBurnChecksDiscoveryEnabledV1'
        BEGIN SELECT RAISE(ABORT, 'the absent YAML preference must not be rewritten'); END;",
        )
        .unwrap();
    let evaluator = Arc::new(RecordingEvaluator(Mutex::new(Vec::new())));
    let engine = CheckEngine::new(adapter, evaluator.clone());
    engine
        .apply_source_page(&active, &event(&store, 1, "one", "branch", 1))
        .unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&active, 100_000).unwrap() else {
        panic!("the configured live assessment must start with an absent YAML preference");
    };
    let settled = poll_settlement(task);
    assert_eq!(settled.outcome, RunOutcome::Accepted);
    assert_eq!((settled.attempts, settled.new_intents), (1, 1));
    assert_eq!(*evaluator.0.lock().unwrap(), [1]);
    assert!(
        store
            .internal_value("internal:yamlBurnChecksDiscoveryEnabledV1")
            .is_none()
    );
    assert_eq!(
        store.internal_value("internal:smartChecksConnectionsV1"),
        connection
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_current_status",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn live_acceptance_rechecks_exact_yaml_preference_inside_the_transaction() {
    for blocked in ["false", "malformed", "null", "\"true\"", "TRUE", " true"] {
        let (store, adapter, active) = configured_live_fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
        assert!(crate::jev::settings::yaml_discovery_enabled(&store));
        let hook_store = store.clone();
        store.set_burn_check_test_hook(Some(Arc::new(move |stage| {
            if stage == super::super::super::test_hooks::Stage::BeforeAcceptance {
                hook_store
                    .set_internal_value_checked(
                        "internal:yamlBurnChecksDiscoveryEnabledV1",
                        blocked,
                    )
                    .unwrap();
            }
        })));
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Deferred,
            "{blocked}"
        );
        store.set_burn_check_test_hook(None);
        assert_eq!(
            store
                .internal_value("internal:yamlBurnChecksDiscoveryEnabledV1")
                .as_deref(),
            Some(blocked)
        );
        for table in [
            "burn_check_assessment_history",
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
                "{blocked}: {table}"
            );
        }
        store
            .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "true")
            .unwrap();
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted
        );
    }
}

#[test]
fn live_absent_yaml_preference_preserves_authentication_and_transition_gates() {
    for (key, blocked, restored) in [
        (
            "internal:typesafeCredentialChangePendingV1",
            "true",
            "false",
        ),
        (
            "internal:smartChecksConnectionChangePendingV1",
            "synthetic-change",
            "",
        ),
        (
            "internal:smartChecksCredentialRemovalPendingV1",
            "synthetic-removal",
            "",
        ),
        ("internal:typesafeAuthRejectedV1", "true", "false"),
    ] {
        let (store, adapter, active) = configured_live_fixture();
        let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
        let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
        store.set_internal_value_checked(key, blocked).unwrap();
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Deferred,
            "{key}"
        );
        store.set_internal_value_checked(key, restored).unwrap();
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted,
            "{key}"
        );
        assert!(
            store
                .internal_value("internal:yamlBurnChecksDiscoveryEnabledV1")
                .is_none()
        );
    }
}

#[test]
fn live_yaml_gate_propagates_sqlite_read_errors_without_acceptance() {
    let (store, adapter, active) = configured_live_fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
    store
        .lock()
        .execute_batch("ALTER TABLE setting RENAME COLUMN value TO unavailable_value")
        .unwrap();
    let error = adapter.accept(&assessment).unwrap_err();
    assert!(
        error.message.contains("no such column:") && error.message.contains("value"),
        "{}",
        error.message
    );
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
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_assessment_history",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn connection_changes_cancel_all_unfinished_states_and_preserve_source_and_status() {
    for state in ["pending", "claimed", "dispatching", "answered"] {
        let directory = tempfile::tempdir().unwrap();
        let (store, adapter, active) =
            fixture_with_store(Arc::new(Store::open(directory.path()).unwrap()));
        let accepted = apply(
            &adapter,
            &active,
            &event(&store, 1, "accepted", "branch-a", 1),
        );
        let assessment = answer(&adapter, &active, claim(&adapter, &accepted), 0.99);
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted
        );
        let pending = apply(
            &adapter,
            &active,
            &event(&store, 2, "pending", "branch-a", 1),
        );
        let late = match state {
            "pending" => None,
            "claimed" => {
                claim(&adapter, &pending);
                None
            }
            "dispatching" => {
                let claimed = claim(&adapter, &pending);
                adapter
                    .dispatch_review(&claimed, 100_000, Duration::ZERO, 1)
                    .unwrap();
                Some(AssessmentAcceptance {
                    claimed,
                    ..assessment.clone()
                })
            }
            "answered" => Some(answer(&adapter, &active, claim(&adapter, &pending), 0.99)),
            _ => unreachable!(),
        };
        let protected = protected_state(&store);
        let source = adapter.load_source_progress(&active).unwrap();
        store
            .cancel_unfinished_burn_check_assessments_for_connection_change()
            .unwrap();
        if let Some(late) = &late {
            if state == "dispatching" {
                adapter
                    .record_decision_answer(
                        &late.claimed,
                        &review_attempt(1),
                        &late.evaluated.decisions()[0],
                    )
                    .unwrap();
            }
            assert_eq!(adapter.accept(late).unwrap(), AcceptanceOutcome::Stale);
        }
        // A second transition represents B -> A. It cannot revive cancelled A work.
        store
            .cancel_unfinished_burn_check_assessments_for_connection_change()
            .unwrap();
        assert_eq!(protected_state(&store), protected);
        assert_eq!(adapter.load_source_progress(&active).unwrap(), source);
        assert!(matches!(
            adapter.claim_due(&pending, 100_000, true).unwrap(),
            CandidateClaim::Stale
        ));
        drop(adapter);
        drop(store);

        let store = Arc::new(Store::open(directory.path()).unwrap());
        let adapter =
            StoreCheckStore::new(store.clone(), "restarted".into(), active.id.scope.clone())
                .unwrap();
        adapter.register_activation(&active).unwrap();
        assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
        assert_eq!(protected_state(&store), protected);
        let state: String = store
            .lock()
            .query_row(
                "SELECT state FROM burn_check_candidate WHERE candidate_id = ?1",
                [hex(&pending.fingerprint)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "cancelled");
    }
}

#[test]
fn connection_change_cancels_deferred_acceptance_without_publishing_late_status() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "first", "branch-a", 1));
    let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
    let _later_page = event(&store, 2, "later", "branch-a", 1);
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Deferred
    );
    store
        .cancel_unfinished_burn_check_assessments_for_connection_change()
        .unwrap();
    assert_eq!(
        adapter.accept(&assessment).unwrap(),
        AcceptanceOutcome::Stale
    );
    let statuses: u64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM burn_check_current_status",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(statuses, 0);
}

#[test]
fn connection_change_rolls_back_when_cancellation_cannot_be_saved() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "first", "branch-a", 1));
    store.lock().execute_batch("CREATE TRIGGER reject_cancel BEFORE UPDATE ON burn_check_candidate WHEN NEW.state = 'cancelled' BEGIN SELECT RAISE(ABORT, 'injected cancellation failure'); END").unwrap();
    assert!(
        store
            .cancel_unfinished_burn_check_assessments_for_connection_change()
            .is_err()
    );
    let history: u64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM burn_check_assessment_history",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(history, 0);
    assert!(matches!(
        adapter.claim_due(&candidate, 100_000, true).unwrap(),
        CandidateClaim::Claimed(_)
    ));
}

#[test]
fn terminal_candidate_lookup_is_exact_scoped_bounded_and_propagates_read_errors() {
    let (store, adapter, active) = fixture();
    let mut requested = Vec::new();
    let mut expected = Vec::new();
    let states = [
        "pending",
        "claimed",
        "dispatching",
        "answered",
        "accepted",
        "coalesced",
        "not_assessed",
        "execution_failed",
        "cancelled",
        "stale",
    ];
    for index in 0..states.len() {
        let candidate = apply(
            &adapter,
            &active,
            &event(
                &store,
                index as u64 + 1,
                &format!("item-{index}"),
                &format!("branch-{index}"),
                1,
            ),
        );
        let id = CandidateId::from(&candidate);
        if index >= 4 {
            expected.push(id.clone());
        }
        requested.push(id);
    }
    for (id, state) in requested.iter().zip(states) {
        store
            .lock()
            .execute(
                "UPDATE burn_check_candidate SET state = ?1 WHERE candidate_id = ?2",
                params![state, hex(&id.0)],
            )
            .unwrap();
    }
    store.lock().execute("UPDATE burn_check_candidate SET activation_scope_key = 'other-scope' WHERE candidate_id = ?1", [hex(&expected.pop().unwrap().0)]).unwrap();
    requested.push(CandidateId([255; 32]));
    let terminal = adapter.load_terminal_candidate_ids(&requested).unwrap();
    assert_eq!(
        terminal
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        expected.into_iter().collect()
    );
    assert!(
        adapter
            .load_terminal_candidate_ids(&vec![requested[0].clone(); 129])
            .is_err()
    );
    store
        .lock()
        .execute_batch("ALTER TABLE burn_check_candidate RENAME TO unavailable_candidate")
        .unwrap();
    assert!(adapter.load_terminal_candidate_ids(&requested).is_err());
}
