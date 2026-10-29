use super::*;

fn publish_current(
    host: &InstalledCheckHost<ScriptedEvaluator>,
    state: &InstalledChecks,
) -> Vec<InventoryEvent> {
    let mut events = Vec::new();
    publish_inventory(state, &host.store, &host.definitions, |event| {
        events.push(event)
    })
    .unwrap();
    events
}

fn rows(store: &Store, table: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let connection = store.lock();
    let mut statement = connection
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns).map(|column| row.get(column)).collect()
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn pause(fixture: &Fixture, host: &mut InstalledCheckHost<ScriptedEvaluator>) {
    fixture
        .store
        .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "false")
        .unwrap();
    fixture
        .store
        .cancel_unfinished_burn_check_assessments_for_connection_change()
        .unwrap();
    host.pause_disabled().unwrap();
}

#[test]
fn cold_paused_inventory_lists_definitions_without_creating_runtime_work() {
    let fixture = Fixture::new();
    fixture
        .store
        .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "false")
        .unwrap();
    let mut host = fixture.host();
    let before = rows(&fixture.store, "burn_check_activation");
    host.refresh_paused_inventory().unwrap();
    let state = InstalledChecks::default();
    assert_eq!(
        publish_current(&host, &state),
        [InventoryEvent::Availability, InventoryEvent::Report]
    );
    assert_eq!(host.definitions.len(), 1);
    assert_eq!(host.definitions[0].check_id, "failed-retry-loop");
    assert!(matches!(
        host.definitions[0].availability,
        InstalledCheckAvailability::Paused { .. }
    ));
    assert_eq!(host.definitions[0].generation, None);
    assert!(host.projects.is_empty());
    assert!(host.evaluation_jobs(1).is_empty());
    assert!(fixture.evaluator.inputs.lock().unwrap().is_empty());
    assert_eq!(rows(&fixture.store, "burn_check_activation"), before);
    assert!(rows(&fixture.store, "burn_check_candidate").is_empty());
    assert!(host.watch_directories.contains(&fixture.global));
    assert!(antiburn_checks_store::runtime_snapshot_ready(&fixture.store.lock()).unwrap());
    assert!(publish_current(&host, &state).is_empty());
}

#[tokio::test]
async fn watched_removal_updates_both_consumers_and_keeps_history_and_incurred_usage() {
    for paused in [false, true] {
        let fixture = Fixture::new();
        let mut host = fixture.host();
        host.reconcile(&Admission::Ready).unwrap();
        fixture.publish("main", 3);
        host.catch_up().unwrap();
        fixture.evaluator.answer("stuck");
        assert_eq!(
            host.evaluation_jobs(1)
                .pop()
                .unwrap()
                .run()
                .await
                .unwrap()
                .unwrap()
                .outcome,
            RunOutcome::Accepted
        );
        fixture
            .store
            .set_internal_value_checked(
                "internal:burnCheckUsageLedgerV1",
                r#"{"summary":{"unknown_outcomes":3}}"#,
            )
            .unwrap();
        let accepted_history = rows(&fixture.store, "burn_check_assessment_history");
        let attempts = rows(&fixture.store, "burn_check_evaluation_attempt");
        let usage = rows(&fixture.store, "burn_check_decision_usage");
        let ledger = fixture
            .store
            .internal_value("internal:burnCheckUsageLedgerV1");
        fixture
            .store
            .lock()
            .execute(
                "UPDATE burn_check_cooldown SET cooldown_until_epoch = 0",
                [],
            )
            .unwrap();
        fixture.publish("main", 6);
        host.catch_up().unwrap();
        assert!(
            fixture
                .store
                .lock()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM burn_check_candidate WHERE state = 'pending')",
                    [],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
        );
        if paused {
            pause(&fixture, &mut host);
            host.refresh_paused_inventory().unwrap();
        }
        let history = rows(&fixture.store, "burn_check_assessment_history");
        assert!(accepted_history.iter().all(|row| history.contains(row)));
        let state = InstalledChecks::default();
        publish_current(&host, &state);
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut watcher = watch::DirectoryWatcher::new(move || {
            let _ = sender.send(());
        })
        .unwrap();
        watcher.refresh(&host.watch_directories);
        std::fs::remove_file(fixture.global.join("failed-retry-loop.yml")).unwrap();
        receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("the YAML removal wakes discovery");
        if paused {
            host.refresh_paused_inventory().unwrap();
        } else {
            host.reconcile(&Admission::Ready).unwrap();
        }
        assert_eq!(
            publish_current(&host, &state),
            [InventoryEvent::Availability, InventoryEvent::Report]
        );
        let snapshot = state.metadata.read().unwrap().clone();
        assert!(snapshot.is_empty());
        assert!(
            read_installed_checks(&fixture.store, &snapshot)
                .unwrap()
                .is_empty()
        );
        assert!(crate::insights_report::installed_checks::availability_rows(&snapshot).is_empty());
        assert!(
            antiburn_checks_store::read_session_checks(
                &fixture.store.lock(),
                "native",
                "pi",
                "main"
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(
            rows(&fixture.store, "burn_check_assessment_history"),
            history
        );
        assert_eq!(
            rows(&fixture.store, "burn_check_evaluation_attempt"),
            attempts
        );
        assert_eq!(rows(&fixture.store, "burn_check_decision_usage"), usage);
        assert_eq!(
            fixture
                .store
                .internal_value("internal:burnCheckUsageLedgerV1"),
            ledger
        );
        assert_eq!(fixture.store.lock().query_row(
            "SELECT COUNT(*) FROM burn_check_candidate WHERE state IN ('pending', 'claimed', 'dispatching', 'answered')",
            [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(fixture.evaluator.inputs.lock().unwrap().len(), 1);
        assert!(publish_current(&host, &state).is_empty());
    }
}

#[test]
fn paused_inventory_does_not_recover_or_modify_saved_work() {
    let fixture = Fixture::new();
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    fixture.publish("main", 3);
    host.catch_up().unwrap();
    fixture
        .store
        .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "false")
        .unwrap();
    let tables = [
        "session",
        "session_evidence",
        "burn_check_activation",
        "burn_check_candidate",
        "burn_check_observation",
        "burn_check_evaluation_attempt",
        "burn_check_assessment_history",
        "burn_check_current_status",
        "burn_check_decision_usage",
        "setting",
    ];
    let before = tables.map(|table| rows(&fixture.store, table));
    let mut cold_host = fixture.host();
    cold_host.refresh_paused_inventory().unwrap();
    for (table, expected) in tables.into_iter().zip(before) {
        assert_eq!(rows(&fixture.store, table), expected, "{table}");
    }
    assert!(cold_host.projects.is_empty());
    assert!(fixture.evaluator.inputs.lock().unwrap().is_empty());
}

#[test]
fn paused_override_deletion_reveals_global_and_final_deletion_removes_the_row() {
    let fixture = Fixture::new();
    let project_file = fixture
        .project
        .join(".antiburn/checks/failed-retry-loop.yml");
    let project_check = CHECK.replacen("name:", "name: Project", 1);
    std::fs::write(&project_file, &project_check).unwrap();
    let mut host = fixture.host();
    pause(&fixture, &mut host);
    host.refresh_paused_inventory().unwrap();
    assert_eq!(
        host.definitions[0].source.scope,
        InstalledCheckScope::Project
    );
    std::fs::remove_file(&project_file).unwrap();
    host.refresh_paused_inventory().unwrap();
    assert_eq!(host.definitions.len(), 1);
    assert_eq!(
        host.definitions[0].source.scope,
        InstalledCheckScope::Global
    );
    std::fs::remove_file(fixture.global.join("failed-retry-loop.yml")).unwrap();
    host.refresh_paused_inventory().unwrap();
    assert!(host.definitions.is_empty());
}

#[test]
fn invalid_or_unreadable_project_override_does_not_silently_use_global() {
    let fixture = Fixture::new();
    let directory = fixture.project.join(".antiburn/checks");
    let project_file = directory.join("failed-retry-loop.yml");
    std::fs::write(&project_file, CHECK).unwrap();
    let mut host = fixture.host();
    pause(&fixture, &mut host);
    host.refresh_paused_inventory().unwrap();
    std::fs::write(&project_file, "name: [").unwrap();
    host.refresh_paused_inventory().unwrap();
    assert_eq!(
        host.definitions[0].source.scope,
        InstalledCheckScope::Project
    );
    assert!(matches!(
        host.definitions[0].availability,
        InstalledCheckAvailability::Invalid { .. }
    ));
    let saved = fixture.project.join(".antiburn/saved-checks");
    let state = InstalledChecks::default();
    publish_current(&host, &state);
    std::fs::rename(&directory, &saved).unwrap();
    std::fs::write(&directory, "not a directory").unwrap();
    let update = host.refresh_paused_inventory();
    assert!(update.is_err());
    assert!(!finish_inventory_update(
        &state,
        &host,
        update,
        diagnostics::RuntimeStage::Reconcile,
        &mut None,
        |_| {},
    ));
    assert_eq!(
        host.definitions[0].source.scope,
        InstalledCheckScope::Project
    );
    assert!(matches!(
        host.definitions[0].availability,
        InstalledCheckAvailability::Invalid { .. }
    ));
    std::fs::remove_file(&directory).unwrap();
    std::fs::create_dir(&directory).unwrap();
    host.refresh_paused_inventory().unwrap();
    assert_eq!(
        host.definitions[0].source.scope,
        InstalledCheckScope::Global
    );
}

fn exceed_repository_limit(fixture: &Fixture) {
    let repositories = std::iter::once(repository(&fixture.project))
        .chain((0..MAX_DEFINITIONS).map(|index| {
            repository(
                &fixture
                    .directory
                    .path()
                    .join(format!("extra-project-{index}")),
            )
        }))
        .collect::<Vec<_>>();
    fixture.store.replace_repositories(&repositories).unwrap();
}

fn assert_saved_registry_paused(store: &Store, before: &[Vec<rusqlite::types::Value>]) {
    let after = rows(store, "burn_check_runtime_definition");
    assert_eq!(after.len(), before.len());
    for (saved, previous) in after.iter().zip(before) {
        assert_eq!(&saved[..5], &previous[..5]);
        assert_eq!(saved[5], rusqlite::types::Value::Text("paused".into()));
    }
}

#[tokio::test]
async fn cold_failed_inventory_preserves_saved_rows_without_exposing_active_results() {
    for paused in [false, true] {
        for unreadable in [false, true] {
            let fixture = Fixture::new();
            let mut previous = fixture.host();
            previous.reconcile(&Admission::Ready).unwrap();
            fixture.publish("main", 3);
            previous.catch_up().unwrap();
            fixture.evaluator.answer("stuck");
            assert_eq!(
                previous
                    .evaluation_jobs(1)
                    .pop()
                    .unwrap()
                    .run()
                    .await
                    .unwrap()
                    .unwrap()
                    .outcome,
                RunOutcome::Accepted
            );
            assert_eq!(
                antiburn_checks_store::read_session_checks(
                    &fixture.store.lock(),
                    "native",
                    "pi",
                    "main",
                )
                .unwrap()[0]
                    .status,
                Some(antiburn_checks_store::CheckStatus::Failure)
            );
            let saved_registry = rows(&fixture.store, "burn_check_runtime_definition");
            let initialization = rows(&fixture.store, "burn_check_runtime_definition_state");
            let history = rows(&fixture.store, "burn_check_assessment_history");
            let attempts = rows(&fixture.store, "burn_check_evaluation_attempt");
            let usage = rows(&fixture.store, "burn_check_decision_usage");
            drop(previous);
            if paused {
                fixture
                    .store
                    .set_internal_value_checked(
                        "internal:yamlBurnChecksDiscoveryEnabledV1",
                        "false",
                    )
                    .unwrap();
            }
            let saved_directory = fixture.directory.path().join("saved-global-checks");
            if unreadable {
                std::fs::rename(&fixture.global, &saved_directory).unwrap();
                std::fs::write(&fixture.global, "not a directory").unwrap();
            } else {
                exceed_repository_limit(&fixture);
            }
            let mut host = fixture.host();
            assert!(!host.inventory_ready);
            let state = InstalledChecks::default();
            state.delivery_ready.store(true, Ordering::Release);
            let mut previous_failure = None;
            let update = if paused {
                host.refresh_paused_inventory()
            } else {
                host.reconcile(&Admission::Ready)
            };
            assert!(update.is_err());
            assert!(!finish_inventory_update(
                &state,
                &host,
                update,
                diagnostics::RuntimeStage::Reconcile,
                &mut previous_failure,
                |_| {},
            ));
            assert!(!state.delivery_ready.load(Ordering::Acquire));
            assert!(!host.inventory_ready);
            assert!(host.definitions.is_empty());
            assert_saved_registry_paused(&fixture.store, &saved_registry);
            assert_eq!(
                rows(&fixture.store, "burn_check_runtime_definition_state"),
                initialization
            );
            let checks = antiburn_checks_store::read_session_checks(
                &fixture.store.lock(),
                "native",
                "pi",
                "main",
            )
            .unwrap();
            assert_eq!(checks.len(), 1);
            assert_eq!(
                checks[0].eligibility,
                antiburn_checks_store::Eligibility::Paused
            );
            assert_eq!(checks[0].status, None);
            assert_eq!(checks[0].message, None);
            assert_eq!(
                rows(&fixture.store, "burn_check_assessment_history"),
                history
            );
            assert_eq!(
                rows(&fixture.store, "burn_check_evaluation_attempt"),
                attempts
            );
            assert_eq!(rows(&fixture.store, "burn_check_decision_usage"), usage);
            if unreadable {
                std::fs::remove_file(&fixture.global).unwrap();
                std::fs::rename(&saved_directory, &fixture.global).unwrap();
            } else {
                fixture
                    .store
                    .replace_repositories(&[repository(&fixture.project)])
                    .unwrap();
            }
            let update = if paused {
                host.refresh_paused_inventory()
            } else {
                host.reconcile(&Admission::Ready)
            };
            let mut events = Vec::new();
            assert!(finish_inventory_update(
                &state,
                &host,
                update,
                diagnostics::RuntimeStage::Reconcile,
                &mut previous_failure,
                |event| events.push(event),
            ));
            assert_eq!(
                events,
                [InventoryEvent::Availability, InventoryEvent::Report]
            );
            assert!(host.inventory_ready);
            assert_eq!(state.metadata.read().unwrap().len(), 1);
            assert_eq!(fixture.evaluator.inputs.lock().unwrap().len(), 1);
        }
    }
}

#[test]
fn incomplete_cold_inventory_does_not_initialize_a_saved_snapshot() {
    let fixture = Fixture::new();
    fixture.host().reconcile(&Admission::Ready).unwrap();
    fixture
        .store
        .lock()
        .execute(
            "UPDATE burn_check_runtime_definition_state SET initialized = 0",
            [],
        )
        .unwrap();
    let saved = rows(&fixture.store, "burn_check_runtime_definition");
    exceed_repository_limit(&fixture);
    let mut host = fixture.host();
    let update = host.refresh_paused_inventory();
    assert!(update.is_err());
    assert!(!finish_inventory_update(
        &InstalledChecks::default(),
        &host,
        update,
        diagnostics::RuntimeStage::Reconcile,
        &mut None,
        |_| {},
    ));
    assert!(!antiburn_checks_store::runtime_snapshot_ready(&fixture.store.lock()).unwrap());
    assert_saved_registry_paused(&fixture.store, &saved);
}

#[test]
fn successful_cold_empty_discovery_replaces_the_saved_registry() {
    for paused in [false, true] {
        let fixture = Fixture::new();
        fixture.host().reconcile(&Admission::Ready).unwrap();
        assert_eq!(
            rows(&fixture.store, "burn_check_runtime_definition").len(),
            1
        );
        fixture
            .store
            .lock()
            .execute(
                "UPDATE burn_check_runtime_definition_state SET initialized = 0",
                [],
            )
            .unwrap();
        std::fs::remove_file(fixture.global.join("failed-retry-loop.yml")).unwrap();
        let mut host = fixture.host();
        let update = if paused {
            host.refresh_paused_inventory()
        } else {
            host.reconcile(&Admission::Ready)
        };
        assert!(host.inventory_ready);
        assert!(host.definitions.is_empty());
        assert!(finish_inventory_update(
            &InstalledChecks::default(),
            &host,
            update,
            diagnostics::RuntimeStage::Reconcile,
            &mut None,
            |_| {},
        ));
        assert!(rows(&fixture.store, "burn_check_runtime_definition").is_empty());
        assert!(antiburn_checks_store::runtime_snapshot_ready(&fixture.store.lock()).unwrap());
    }
}

#[test]
fn failed_registry_replacement_retries_without_publishing_uncommitted_definitions() {
    for paused in [false, true] {
        let fixture = Fixture::new();
        let mut host = fixture.host();
        host.reconcile(&Admission::Ready).unwrap();
        if paused {
            pause(&fixture, &mut host);
        }
        let state = InstalledChecks::default();
        publish_current(&host, &state);
        let name = host.definitions[0].name.clone();
        let saved = rows(&fixture.store, "burn_check_runtime_definition");
        std::fs::write(
            fixture.global.join("failed-retry-loop.yml"),
            CHECK.replacen("name:", "name: Changed", 1),
        )
        .unwrap();
        fixture.store.lock().execute_batch(
            "CREATE TRIGGER reject_inventory_write BEFORE DELETE ON burn_check_runtime_definition
             BEGIN SELECT RAISE(ABORT, 'synthetic registry failure'); END;",
        ).unwrap();
        let mut previous_failure = None;
        let mut completed_turns = 0;
        for retry in [false, true] {
            state.delivery_ready.store(true, Ordering::Release);
            let update = if paused {
                host.refresh_paused_inventory()
            } else {
                host.reconcile(&Admission::Ready)
            };
            assert_eq!(update.is_ok(), retry);
            let mut events = Vec::new();
            let updated = finish_inventory_update(
                &state,
                &host,
                update,
                diagnostics::RuntimeStage::Reconcile,
                &mut previous_failure,
                |event| events.push(event),
            );
            completed_turns += 1;
            assert_eq!(updated, retry);
            if retry {
                assert!(host.inventory_ready);
                assert_eq!(
                    events,
                    [InventoryEvent::Availability, InventoryEvent::Report]
                );
                assert_ne!(state.metadata.read().unwrap()[0].name, name);
                assert_eq!(
                    state.metadata.read().unwrap()[0].name,
                    host.definitions[0].name
                );
                assert!(
                    state.metadata.read().unwrap()[0]
                        .diagnostics
                        .iter()
                        .all(|diagnostic| diagnostic.code != "storage-failed")
                );
            } else {
                assert!(!host.inventory_ready);
                assert!(!state.delivery_ready.load(Ordering::Acquire));
                assert_eq!(host.definitions[0].name, name);
                assert_eq!(state.metadata.read().unwrap()[0].name, name);
                assert_saved_registry_paused(&fixture.store, &saved);
                fixture
                    .store
                    .lock()
                    .execute_batch("DROP TRIGGER reject_inventory_write")
                    .unwrap();
            }
        }
        assert_eq!(completed_turns, 2);
        assert!(fixture.evaluator.inputs.lock().unwrap().is_empty());
    }
}

#[test]
fn failed_registry_publication_retries_at_reconcile_and_catch_up_boundaries() {
    for stage in [
        diagnostics::RuntimeStage::Reconcile,
        diagnostics::RuntimeStage::SourceCatchUp,
    ] {
        let fixture = Fixture::new();
        let mut host = fixture.host();
        host.reconcile(&Admission::Ready).unwrap();
        let state = InstalledChecks::default();
        publish_current(&host, &state);
        fixture.store.lock().execute_batch(
            "UPDATE burn_check_runtime_definition_state SET initialized = 0;
             CREATE TRIGGER reject_inventory_initialization BEFORE UPDATE ON burn_check_runtime_definition_state
             BEGIN SELECT RAISE(ABORT, 'synthetic registry initialization failure'); END;",
        ).unwrap();
        let mut previous_failure = None;
        state.delivery_ready.store(true, Ordering::Release);
        assert!(!finish_inventory_update(
            &state,
            &host,
            Ok(()),
            stage,
            &mut previous_failure,
            |_| {},
        ));
        assert!(!state.delivery_ready.load(Ordering::Acquire));
        assert!(!antiburn_checks_store::runtime_snapshot_ready(&fixture.store.lock()).unwrap());
        assert!(matches!(
            state.metadata.read().unwrap()[0].availability,
            InstalledCheckAvailability::Paused { .. }
        ));
        fixture
            .store
            .lock()
            .execute_batch("DROP TRIGGER reject_inventory_initialization")
            .unwrap();
        let mut events = Vec::new();
        assert!(finish_inventory_update(
            &state,
            &host,
            Ok(()),
            stage,
            &mut previous_failure,
            |event| events.push(event),
        ));
        assert_eq!(
            events,
            [InventoryEvent::Availability, InventoryEvent::Report]
        );
        assert!(antiburn_checks_store::runtime_snapshot_ready(&fixture.store.lock()).unwrap());
        assert_eq!(
            state.metadata.read().unwrap()[0].availability,
            InstalledCheckAvailability::Available
        );
        assert!(fixture.evaluator.inputs.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn incomplete_inventory_keeps_dispatched_attempts_until_their_normal_settlement() {
    let fixture = Fixture::new();
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    fixture.publish("main", 3);
    host.catch_up().unwrap();
    let state = InstalledChecks::default();
    publish_current(&host, &state);
    fixture.evaluator.answer("stuck");
    fixture.evaluator.wait.store(true, Ordering::Release);
    let task = tokio::spawn(host.evaluation_jobs(1).pop().unwrap().run());
    fixture.evaluator.started.notified().await;
    let candidates = rows(&fixture.store, "burn_check_candidate");
    let attempts = rows(&fixture.store, "burn_check_evaluation_attempt");
    let usage = rows(&fixture.store, "burn_check_decision_usage");
    assert_eq!(attempts.len(), 1);
    let saved = fixture.directory.path().join("saved-global-checks");
    std::fs::rename(&fixture.global, &saved).unwrap();
    std::fs::write(&fixture.global, "not a directory").unwrap();
    let update = host.reconcile(&Admission::Ready);
    assert!(update.is_err());
    assert!(!finish_inventory_update(
        &state,
        &host,
        update,
        diagnostics::RuntimeStage::Reconcile,
        &mut None,
        |_| {},
    ));
    assert_eq!(rows(&fixture.store, "burn_check_candidate"), candidates);
    assert_eq!(
        rows(&fixture.store, "burn_check_evaluation_attempt"),
        attempts
    );
    assert_eq!(rows(&fixture.store, "burn_check_decision_usage"), usage);
    std::fs::remove_file(&fixture.global).unwrap();
    std::fs::rename(&saved, &fixture.global).unwrap();
    fixture.evaluator.release.notify_one();
    assert_eq!(
        task.await.unwrap().unwrap().unwrap().outcome,
        RunOutcome::Accepted
    );
    fixture.evaluator.wait.store(false, Ordering::Release);
    host.reconcile(&Admission::Ready).unwrap();
    assert!(
        host.evaluation_jobs(1)
            .pop()
            .unwrap()
            .run()
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(fixture.evaluator.inputs.lock().unwrap().len(), 1);
}

#[test]
fn one_surviving_project_keeps_only_its_same_id_definition() {
    let fixture = Fixture::new();
    std::fs::remove_file(fixture.global.join("failed-retry-loop.yml")).unwrap();
    let second = fixture.directory.path().join("second-project");
    std::fs::create_dir_all(second.join(".antiburn/checks")).unwrap();
    fixture
        .store
        .replace_repositories(&[repository(&fixture.project), repository(&second)])
        .unwrap();
    fixture.index_in_cwd("native", "second", 0, &second);
    for root in [&fixture.project, &second] {
        std::fs::write(root.join(".antiburn/checks/failed-retry-loop.yml"), CHECK).unwrap();
    }
    let mut host = fixture.host();
    pause(&fixture, &mut host);
    host.refresh_paused_inventory().unwrap();
    assert_eq!(host.definitions.len(), 2);
    std::fs::remove_file(
        fixture
            .project
            .join(".antiburn/checks/failed-retry-loop.yml"),
    )
    .unwrap();
    host.refresh_paused_inventory().unwrap();
    assert_eq!(host.definitions.len(), 1);
    assert!(
        host.definitions[0]
            .source
            .path
            .as_ref()
            .unwrap()
            .starts_with(second.to_str().unwrap())
    );
    assert_eq!(
        read_installed_checks(&fixture.store, &host.definitions)
            .unwrap()
            .len(),
        1
    );
}
