use super::*;

const LOCAL: &str = "name: Local status\non:\n  tool_result:\nactions:\n  - type: status\n    input: {result: failure}\n";

fn local_harness(a: &Server, b: &Server) -> Harness {
    let mut harness = Harness::new(a, b);
    std::fs::write(harness.fixture.global.join("failed-retry-loop.yml"), LOCAL).unwrap();
    harness.host.reconcile(&Admission::Ready).unwrap();
    publish_metadata(&harness.runtime.installed, &harness.host.definitions);
    harness
        .runtime
        .prohibit_model_work
        .store(true, Ordering::Release);
    harness
}

fn count(store: &Store, table: &str) -> u64 {
    store
        .lock()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn assert_provider_free(harness: &Harness, a: &Server, b: &Server) {
    assert_eq!(harness.runtime.discoveries.load(Ordering::Acquire), 0);
    assert_eq!((a.count(), b.count()), (0, 0));
    for table in ["burn_check_evaluation_attempt", "burn_check_decision_usage"] {
        assert_eq!(count(&harness.fixture.store, table), 0, "{table}");
    }
    assert_eq!(
        harness
            .fixture
            .store
            .burn_check_usage_summary()
            .unwrap()
            .confirmed_calls,
        0
    );
    assert_eq!(
        harness
            .fixture
            .store
            .burn_check_usage_summary()
            .unwrap()
            .unknown_outcomes,
        0
    );
}

fn routes(a: &Server) -> Vec<(SystemOneConnection, Option<String>)> {
    let jev = SystemOneConnection::jev_default();
    let ollama = SystemOneConnection {
        provider: SystemOneProvider::Ollama,
        endpoint: SystemOneEndpoint::BaseUrl(format!("http://{}", a.address)),
        model: "synthetic-model".into(),
        credential: None,
        ..jev.clone()
    };
    let cloudflare = cloudflare_connection();
    let direct = a.connection("local-only-model");
    let envelope = SystemOneConnection {
        response_mode: SystemOneResponseMode::CloudflareEnvelope,
        ..direct.clone()
    };
    vec![
        (jev, Some("synthetic-secret".into())),
        (ollama.clone(), None),
        (cloudflare, Some("synthetic-secret".into())),
        (direct.clone(), None),
        (envelope, None),
        (ollama, Some("synthetic-optional-secret".into())),
        (direct, Some("synthetic-optional-secret".into())),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_five_routes_run_local_with_failed_or_unsupported_cached_model_state() {
    for cache in ["empty", "failed", "unsupported"] {
        let (a, b) = (Server::new(), Server::new());
        for (connection, credential) in routes(&a) {
            let mut harness = local_harness(&a, &b);
            harness
                .runtime
                .worker
                .set_system_one_connection(connection, credential)
                .unwrap();
            let generation = harness
                .runtime
                .worker
                .execution_snapshot()
                .unwrap()
                .generation;
            if cache != "empty" {
                let result = if cache == "failed" {
                    Err("Synthetic cached discovery failure".into())
                } else {
                    let mut capabilities = ModelCapabilities::jev_default();
                    capabilities.output.choice = None;
                    Ok(capabilities)
                };
                *harness.runtime.installed.capabilities.write().unwrap() =
                    Some(execution::CapabilityState {
                        generation,
                        result,
                        retry_at: Instant::now() + DISCOVERY_INTERVAL,
                    });
            }
            let (capabilities, capability_error) =
                execution::coordinator_capabilities(&harness.runtime.installed, Some(generation));
            harness.host.capabilities = capabilities;
            harness.host.capability_error = capability_error;
            harness.host.reconcile(&Admission::Ready).unwrap();
            publish_metadata(&harness.runtime.installed, &harness.host.definitions);
            assert_eq!(
                harness.host.definitions[0].availability,
                InstalledCheckAvailability::Available
            );
            harness.publish(1);
            let project = harness.host.projects.values().next().unwrap();
            let store = Arc::clone(&project.store);
            let activation = project.active[0].clone();
            let progress = store.load_source_progress(&activation).unwrap();
            let settlement = harness.job().await.unwrap().unwrap().unwrap();
            assert_eq!(settlement.outcome, RunOutcome::Accepted, "{cache}");
            assert_eq!(settlement.attempts, 0);
            assert_eq!(store.load_source_progress(&activation).unwrap(), progress);
            let answer: String = harness
                .fixture
                .store
                .lock()
                .query_row(
                    "SELECT answer_json FROM burn_check_candidate WHERE state = 'accepted'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&answer).unwrap(),
                json!({"version": 1, "decisions": [], "attempts": 0})
            );
            assert_provider_free(&harness, &a, &b);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_failed_discovery_cache_does_not_block_a_later_local_definition() {
    let (a, b) = (Server::new(), Server::new());
    let mut harness = Harness::new(&a, &b);
    harness
        .runtime
        .fail_resolution
        .store(true, Ordering::Release);
    harness.publish(1);
    assert!(harness.job().await.unwrap().unwrap().is_none());
    assert_eq!(harness.runtime.discoveries.load(Ordering::Acquire), 1);
    std::fs::write(harness.fixture.global.join("failed-retry-loop.yml"), LOCAL).unwrap();
    let generation = harness
        .runtime
        .worker
        .execution_snapshot()
        .unwrap()
        .generation;
    let (capabilities, capability_error) =
        execution::coordinator_capabilities(&harness.runtime.installed, Some(generation));
    assert!(capabilities.is_none());
    assert!(capability_error.is_some());
    harness.host.capabilities = capabilities;
    harness.host.capability_error = capability_error;
    harness.host.reconcile(&Admission::Ready).unwrap();
    publish_metadata(&harness.runtime.installed, &harness.host.definitions);
    harness
        .runtime
        .prohibit_model_work
        .store(true, Ordering::Release);
    harness.publish(2);
    assert_eq!(
        harness.job().await.unwrap().unwrap().unwrap().outcome,
        RunOutcome::Accepted
    );
    assert_eq!(harness.runtime.discoveries.load(Ordering::Acquire), 1);
    assert_eq!((a.count(), b.count()), (0, 0));
    assert_eq!(
        count(&harness.fixture.store, "burn_check_evaluation_attempt"),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_no_write_preserves_accepted_model_data_source_progress_and_incurred_usage() {
    let (a, b) = (Server::new(), Server::new());
    let mut harness = Harness::new(&a, &b);
    harness.baseline(&a).await;
    let prior_status = harness.status();
    let prior_usage = harness.fixture.store.burn_check_usage_summary().unwrap();
    let prior_history: String = harness
        .fixture
        .store
        .lock()
        .query_row(
            "SELECT answer_json FROM burn_check_candidate WHERE state = 'accepted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    std::fs::write(
        harness.fixture.global.join("local-status.yml"),
        LOCAL.replace("  - type: status\n", "  - type: status\n    if: false\n"),
    )
    .unwrap();
    harness.host.reconcile(&Admission::Ready).unwrap();
    publish_metadata(&harness.runtime.installed, &harness.host.definitions);
    harness
        .runtime
        .prohibit_model_work
        .store(true, Ordering::Release);
    harness.publish(2);
    let job = harness
        .host
        .evaluation_jobs(2)
        .into_iter()
        .find(|job| job.activation.compiled.is_local())
        .unwrap();
    let store = Arc::clone(&job.store);
    let activation = job.activation.clone();
    let progress = store.load_source_progress(&activation).unwrap();
    let runtime: Arc<dyn SelectedJobRuntime> = harness.runtime.clone();
    let settlement = job.run_selected(runtime).await.unwrap().unwrap();
    assert_eq!(settlement.outcome, RunOutcome::Accepted);
    assert_eq!(settlement.attempts, 0);
    assert_eq!(store.load_source_progress(&activation).unwrap(), progress);
    assert_eq!(harness.status(), prior_status);
    assert_eq!(
        harness.fixture.store.burn_check_usage_summary().unwrap(),
        prior_usage
    );
    let retained: String = harness.fixture.store.lock().query_row(
        "SELECT answer_json FROM burn_check_candidate WHERE check_id = 'failed-retry-loop' AND state = 'accepted'", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(retained, prior_history);
    assert_eq!(
        count(&harness.fixture.store, "burn_check_assessment_history"),
        2
    );
    assert_eq!(
        count(&harness.fixture.store, "burn_check_evaluation_attempt"),
        1
    );
    assert_eq!(
        count(&harness.fixture.store, "burn_check_decision_usage"),
        1
    );
    assert_eq!(harness.runtime.discoveries.load(Ordering::Acquire), 1);
    assert_eq!((a.count(), b.count()), (1, 0));
}

#[test]
fn local_admission_preserves_every_nonready_base_gate() {
    let (a, b) = (Server::new(), Server::new());
    let mut harness = local_harness(&a, &b);
    harness.host.capability_error = Some("Synthetic discovery failure".into());
    for admission in [
        Admission::Paused("Repository is disabled.".into()),
        Admission::Unsupported("Base runtime is unsupported.".into()),
        Admission::ProviderNotConfigured,
        Admission::AuthenticationRejected,
    ] {
        harness.host.reconcile(&admission).unwrap();
        assert_ne!(
            harness.host.definitions[0].availability,
            InstalledCheckAvailability::Available
        );
    }
}

#[tokio::test]
async fn discovered_checks_without_a_provider_show_setup_errors_and_do_no_work() {
    for (indexed, repository_known) in [(true, true), (false, true), (false, false)] {
        let (a, b) = (Server::new(), Server::new());
        let fixture = Fixture::new();
        std::fs::write(fixture.global.join("failed-retry-loop.yml"), SOURCE).unwrap();
        std::fs::write(fixture.global.join("local-status.yml"), LOCAL).unwrap();
        let repository_checks = fixture.project.join(".antiburn/checks");
        std::fs::write(repository_checks.join("repo-model.yml"), SOURCE).unwrap();
        std::fs::write(repository_checks.join("repo-local.yml"), LOCAL).unwrap();
        if !indexed {
            fixture
                .store
                .lock()
                .execute("DELETE FROM session", [])
                .unwrap();
        }
        if !repository_known {
            fixture.store.replace_repositories(&[]).unwrap();
        }
        let mut harness = Harness::with_worker(fixture, Arc::new(WorkerHandle::default()));
        assert_eq!(
            admission_for_worker(&harness.runtime.worker),
            Admission::ProviderNotConfigured
        );
        assert!(harness.runtime.worker.execution_snapshot().is_none());
        let expected_count = if repository_known { 4 } else { 2 };
        assert_eq!(harness.host.definitions.len(), expected_count);
        assert_eq!(
            harness
                .host
                .definitions
                .iter()
                .filter(|definition| definition.source.scope == InstalledCheckScope::Global)
                .count(),
            2
        );
        assert_eq!(
            harness
                .host
                .definitions
                .iter()
                .filter(|definition| definition.source.scope == InstalledCheckScope::Project)
                .count(),
            expected_count - 2
        );
        if indexed {
            harness.publish(1);
            assert_eq!(
                harness
                    .host
                    .projects
                    .values()
                    .map(|project| project.active.len())
                    .sum::<usize>(),
                expected_count
            );
            let now = i64::try_from(harness.runtime.clock.load(Ordering::Acquire) / 1000).unwrap();
            for project in harness.host.projects.values() {
                for activation in &project.active {
                    assert!(project.store.has_due_candidate(activation, now).unwrap());
                    let runtime: Arc<dyn SelectedJobRuntime> = harness.runtime.clone();
                    let job = EvaluationJob {
                        engine: project.engine.clone(),
                        store: project.store.clone(),
                        activation: activation.clone(),
                    };
                    assert!(job.run_selected(runtime).await.unwrap().is_none());
                }
            }
        }
        assert!(harness.host.evaluation_jobs(1).is_empty());
        let metadata = harness.runtime.installed.metadata.read().unwrap().clone();
        assert_eq!(
            crate::insights_report::installed_checks::availability_rows(&metadata).len(),
            expected_count
        );
        let report = read_installed_checks(&harness.fixture.store, &metadata).unwrap();
        assert_eq!(report.len(), expected_count);
        for detail in report {
            assert_eq!(detail.diagnostics.len(), 1);
            let diagnostic = &detail.diagnostics[0];
            assert_eq!(diagnostic.code, "provider-not-configured");
            assert_eq!(
                diagnostic.severity,
                crate::insights_report::installed_checks::InstalledCheckDiagnosticSeverity::Error
            );
            assert_eq!(diagnostic.title, "Set up a decision model");
            assert_eq!(
                diagnostic.message,
                "Set up a decision model in Settings → Checks before this check can run."
            );
            assert_eq!(detail.checking_count, 0);
            assert!(!detail.has_started);
        }
        let preparations: u64 = harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_candidate WHERE preparation_json IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preparations, 0);
        assert_eq!(
            count(&harness.fixture.store, "burn_check_assessment_history"),
            0
        );
        assert_eq!(
            count(&harness.fixture.store, "burn_check_current_status"),
            0
        );
        assert_provider_free(&harness, &a, &b);
    }
}

#[test]
fn empty_check_directories_without_a_provider_do_not_invent_a_setup_error_or_card() {
    for indexed in [false, true] {
        let (a, b) = (Server::new(), Server::new());
        let fixture = Fixture::new();
        std::fs::remove_file(fixture.global.join("failed-retry-loop.yml")).unwrap();
        if !indexed {
            fixture
                .store
                .lock()
                .execute("DELETE FROM session", [])
                .unwrap();
        }
        let mut harness = Harness::with_worker(fixture, Arc::new(WorkerHandle::default()));
        assert_eq!(
            admission_for_worker(&harness.runtime.worker),
            Admission::ProviderNotConfigured
        );
        assert!(harness.host.definitions.is_empty());
        let metadata = harness.runtime.installed.metadata.read().unwrap().clone();
        assert!(crate::insights_report::installed_checks::availability_rows(&metadata).is_empty());
        assert!(
            read_installed_checks(&harness.fixture.store, &metadata)
                .unwrap()
                .is_empty()
        );
        assert_eq!(harness.host.catch_up().unwrap(), (false, false));
        assert!(harness.host.evaluation_jobs(1).is_empty());
        assert_provider_free(&harness, &a, &b);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_repository_removes_local_jobs_without_starting_work() {
    let (a, b) = (Server::new(), Server::new());
    let mut harness = local_harness(&a, &b);
    harness.publish(1);
    assert!(
        harness
            .fixture
            .store
            .set_repository_enabled(harness.fixture.project.to_str().unwrap(), false,)
            .unwrap()
    );
    let repositories = harness.fixture.store.repositories().unwrap();
    assert_eq!(repositories.len(), 1);
    assert!(!repositories[0].enabled);
    harness.host.reconcile(&Admission::Ready).unwrap();
    publish_metadata(&harness.runtime.installed, &harness.host.definitions);
    assert!(harness.host.evaluation_jobs(1).is_empty());
    assert!(matches!(
        &harness.host.definitions[0].availability,
        InstalledCheckAvailability::Paused { reason: Some(reason) } if reason == "Repository is disabled."
    ));
    assert!(
        harness
            .host
            .projects
            .values()
            .all(|project| project.active.is_empty())
    );
    assert_eq!(
        count(&harness.fixture.store, "burn_check_current_status"),
        0
    );
    let started: u64 = harness
        .fixture
        .store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM burn_check_candidate WHERE preparation_json IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(started, 0);
    assert_provider_free(&harness, &a, &b);
}

fn block_runtime(harness: &Harness, gate: &str) {
    match gate {
        "master" => harness
            .fixture
            .store
            .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "false")
            .unwrap(),
        "restoring" => harness
            .runtime
            .installed
            .credentials_restored
            .store(false, Ordering::Release),
        "shutdown" => harness
            .runtime
            .installed
            .stopping
            .store(true, Ordering::Release),
        "runtime" => harness.runtime.worker.suspend_system_one(),
        "invalid-configuration" | "missing-configuration" => {
            if gate == "invalid-configuration" {
                harness
                    .fixture
                    .store
                    .set_internal_value_checked("internal:smartChecksConnectionsV1", "{}")
                    .unwrap();
            } else {
                harness
                    .fixture
                    .store
                    .remove_internal_value("internal:smartChecksConnectionsV1");
            }
            assert!(
                headless::restore_keyless_profile(&harness.fixture.store, &harness.runtime.worker)
                    .is_err()
            );
            assert!(!harness.runtime.worker.is_available());
        }
        "authentication" => {
            let generation = harness
                .runtime
                .worker
                .execution_snapshot()
                .unwrap()
                .generation;
            assert!(
                harness
                    .runtime
                    .worker
                    .reject_authentication(&harness.fixture.store, generation)
                    .unwrap()
            );
        }
        "missing-jev-credential" => harness
            .runtime
            .worker
            .set_system_one_connection(SystemOneConnection::jev_default(), None)
            .unwrap(),
        "missing-cloudflare-credential" => {
            harness
                .runtime
                .worker
                .set_system_one_connection(cloudflare_connection(), None)
                .unwrap();
        }
        marker @ ("internal:smartChecksConnectionChangePendingV1"
        | "internal:typesafeCredentialChangePendingV1"
        | "internal:smartChecksCredentialRemovalPendingV1") => harness
            .fixture
            .store
            .set_internal_value_checked(marker, "true")
            .unwrap(),
        _ => panic!("Unknown runtime gate: {gate}"),
    }
}

fn cloudflare_connection() -> SystemOneConnection {
    SystemOneConnection {
        provider: SystemOneProvider::Cloudflare,
        endpoint: SystemOneEndpoint::CloudflareAccount("synthetic-account".into()),
        model: "clef".into(),
        response_mode: SystemOneResponseMode::CloudflareEnvelope,
        ..SystemOneConnection::jev_default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_selection_and_factory_recovery_keep_base_runtime_gates() {
    for gate in [
        "master",
        "restoring",
        "shutdown",
        "runtime",
        "authentication",
        "missing-configuration",
        "invalid-configuration",
        "missing-jev-credential",
        "missing-cloudflare-credential",
    ] {
        let (a, b) = (Server::new(), Server::new());
        let mut harness = local_harness(&a, &b);
        harness.publish(1);
        let before = harness.protected();
        block_runtime(&harness, gate);
        let activation = harness.host.projects.values().next().unwrap().active[0].clone();
        assert!(
            !harness
                .host
                .evaluator
                .local_execution_is_current(&activation),
            "{gate}"
        );
        assert!(harness.job().await.unwrap().unwrap().is_none(), "{gate}");
        assert_eq!(harness.protected(), before, "{gate}");
        assert_eq!(
            count(&harness.fixture.store, "burn_check_assessment_history"),
            0
        );
        assert_provider_free(&harness, &a, &b);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_runtime_and_durable_gates_preserve_local_answers_for_recovery() {
    for gate_name in [
        "master",
        "restoring",
        "shutdown",
        "runtime",
        "authentication",
        "missing-configuration",
        "invalid-configuration",
        "missing-jev-credential",
        "missing-cloudflare-credential",
        "internal:smartChecksConnectionChangePendingV1",
        "internal:typesafeCredentialChangePendingV1",
        "internal:smartChecksCredentialRemovalPendingV1",
    ] {
        let (a, b) = (Server::new(), Server::new());
        let mut harness = local_harness(&a, &b);
        harness.publish(1);
        let gate = Arc::new(Gate::default());
        let _release = ReleaseGate(Arc::clone(&gate));
        let callback = Arc::clone(&gate);
        harness
            .fixture
            .store
            .set_burn_check_test_hook(Some(Arc::new(move |stage| {
                if stage == StoreStage::BeforeAcceptance {
                    callback.block();
                }
            })));
        let task = harness.job();
        gate.wait().await;
        block_runtime(&harness, gate_name);
        gate.open();
        let settlement = task.await.unwrap().unwrap().unwrap();
        assert_eq!(settlement.outcome, RunOutcome::Deferred, "{gate_name}");
        assert_eq!(settlement.attempts, 0);
        harness.fixture.store.set_burn_check_test_hook(None);
        let saved: (String, String) = harness.fixture.store.lock().query_row("SELECT preparation_json, answer_json FROM burn_check_candidate WHERE state = 'answered'", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        let protected = harness.protected();
        harness.host.catch_up().unwrap();
        assert_eq!(harness.protected(), protected, "{gate_name}");
        assert_eq!(
            count(&harness.fixture.store, "burn_check_assessment_history"),
            0
        );
        harness
            .fixture
            .store
            .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "true")
            .unwrap();
        harness
            .fixture
            .store
            .remove_internal_value("internal:typesafeAuthRejectedV1");
        harness.fixture.store.remove_internal_value(gate_name);
        harness
            .runtime
            .installed
            .credentials_restored
            .store(true, Ordering::Release);
        harness
            .runtime
            .installed
            .stopping
            .store(false, Ordering::Release);
        harness.runtime.worker.suspend_system_one();
        harness
            .runtime
            .worker
            .set_system_one_connection(a.connection("model-a"), None)
            .unwrap();
        harness.host.catch_up().unwrap();
        assert_eq!(
            count(&harness.fixture.store, "burn_check_assessment_history"),
            1,
            "{gate_name}"
        );
        let accepted: (String, String) = harness.fixture.store.lock().query_row("SELECT preparation_json, answer_json FROM burn_check_candidate WHERE state = 'accepted'", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert_eq!(accepted, saved);
        assert_provider_free(&harness, &a, &b);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_answer_recovers_on_first_reconcile_before_metadata_publication() {
    let (a, b) = (Server::new(), Server::new());
    let mut harness = local_harness(&a, &b);
    harness.publish(1);
    let gate = Arc::new(Gate::default());
    let _release = ReleaseGate(Arc::clone(&gate));
    let callback = Arc::clone(&gate);
    harness
        .fixture
        .store
        .set_burn_check_test_hook(Some(Arc::new(move |stage| {
            if stage == StoreStage::BeforeAcceptance {
                callback.block();
            }
        })));
    let task = harness.job();
    gate.wait().await;
    harness
        .runtime
        .installed
        .stopping
        .store(true, Ordering::Release);
    gate.open();
    assert_eq!(
        task.await.unwrap().unwrap().unwrap().outcome,
        RunOutcome::Deferred
    );
    harness.fixture.store.set_burn_check_test_hook(None);
    let mut harness = harness.reopen(true);
    harness
        .runtime
        .prohibit_model_work
        .store(true, Ordering::Release);
    assert_eq!(
        count(&harness.fixture.store, "burn_check_assessment_history"),
        1
    );
    assert!(harness.job().await.unwrap().unwrap().is_none());
    harness.host.catch_up().unwrap();
    assert_eq!(
        count(&harness.fixture.store, "burn_check_assessment_history"),
        1
    );
    assert_provider_free(&harness, &a, &b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn factory_gate_blocks_recovery_at_the_final_acceptance_boundary() {
    let (a, b) = (Server::new(), Server::new());
    let mut harness = local_harness(&a, &b);
    harness.publish(1);
    let gate = Arc::new(Gate::default());
    let _release = ReleaseGate(Arc::clone(&gate));
    let callback = Arc::clone(&gate);
    harness
        .fixture
        .store
        .set_burn_check_test_hook(Some(Arc::new(move |stage| {
            if stage == StoreStage::AfterAnswer {
                callback.block();
            }
        })));
    let task = harness.job();
    gate.wait().await;
    harness
        .runtime
        .installed
        .stopping
        .store(true, Ordering::Release);
    gate.open();
    assert_eq!(
        task.await.unwrap().unwrap().unwrap().outcome,
        RunOutcome::Deferred
    );
    harness
        .runtime
        .installed
        .stopping
        .store(false, Ordering::Release);
    harness.runtime.installed.metadata.write().unwrap().clear();
    let gate = Arc::new(Gate::default());
    let _release = ReleaseGate(Arc::clone(&gate));
    let callback = Arc::clone(&gate);
    harness
        .fixture
        .store
        .set_burn_check_test_hook(Some(Arc::new(move |stage| {
            if stage == StoreStage::BeforeAcceptance {
                callback.block();
            }
        })));
    let mut host = harness.host;
    let recovery = tokio::task::spawn_blocking(move || {
        host.reconcile(&Admission::Ready).unwrap();
        host
    });
    gate.wait().await;
    harness
        .runtime
        .installed
        .stopping
        .store(true, Ordering::Release);
    gate.open();
    harness.host = recovery.await.unwrap();
    harness.fixture.store.set_burn_check_test_hook(None);
    assert_eq!(
        count(&harness.fixture.store, "burn_check_assessment_history"),
        0
    );
    assert_eq!(
        harness
            .fixture
            .store
            .lock()
            .query_row::<String, _, _>("SELECT state FROM burn_check_candidate", [], |row| row
                .get(0))
            .unwrap(),
        "answered"
    );
    harness
        .runtime
        .installed
        .stopping
        .store(false, Ordering::Release);
    harness.host.reconcile(&Admission::Ready).unwrap();
    assert_eq!(
        count(&harness.fixture.store, "burn_check_assessment_history"),
        1
    );
    assert_provider_free(&harness, &a, &b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_selected_generation_and_activation_do_not_use_a_factory_fallback() {
    let (a, b) = (Server::new(), Server::new());
    let mut harness = local_harness(&a, &b);
    harness.publish(1);
    let activation = harness.host.projects.values().next().unwrap().active[0].clone();
    let runtime: Arc<dyn SelectedJobRuntime> = harness.runtime.clone();
    let selected = InstalledDecisionEvaluator::for_assessment(runtime, &activation)
        .await
        .unwrap()
        .unwrap();
    assert!(selected.local_execution_is_current(&activation));
    harness.select("b");
    harness.select("a");
    assert!(!selected.local_execution_is_current(&activation));
    assert!(
        harness
            .host
            .evaluator
            .local_execution_is_current(&activation)
    );
    harness.publish(2);
    let project = harness.host.projects.values().next().unwrap();
    let branches = project.store.initialized_branches(&activation).unwrap();
    let start = project
        .engine
        .start_one_due_for_branches_with_evaluator(
            &activation,
            harness.runtime.clock.load(Ordering::Acquire),
            &branches,
            selected,
        )
        .unwrap();
    assert!(matches!(start, StartOutcome::NotDue));
    assert!(
        project
            .store
            .has_due_candidate(
                &activation,
                i64::try_from(harness.runtime.clock.load(Ordering::Acquire) / 1000).unwrap()
            )
            .unwrap()
    );
    let runtime: Arc<dyn SelectedJobRuntime> = harness.runtime.clone();
    let selected = InstalledDecisionEvaluator::for_assessment(runtime, &activation)
        .await
        .unwrap()
        .unwrap();
    harness.runtime.installed.metadata.write().unwrap().clear();
    assert!(selected.local_execution_is_current(&activation));
    std::fs::remove_file(harness.fixture.global.join("failed-retry-loop.yml")).unwrap();
    harness.host.reconcile(&Admission::Ready).unwrap();
    assert!(!selected.local_execution_is_current(&activation));
    assert!(
        harness
            .host
            .evaluator
            .local_execution_is_current(&activation)
    );
    assert_provider_free(&harness, &a, &b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_to_b_to_a_cancels_local_answers_before_acceptance_and_during_recovery() {
    for (stage, deferred) in [
        (StoreStage::BeforeAnswer, false),
        (StoreStage::AfterAnswer, false),
        (StoreStage::BeforeAcceptance, false),
        (StoreStage::BeforeAcceptance, true),
    ] {
        let (a, b) = (Server::new(), Server::new());
        let mut harness = local_harness(&a, &b);
        harness.publish(1);
        let gate = Arc::new(Gate::default());
        let _release = ReleaseGate(Arc::clone(&gate));
        let callback = Arc::clone(&gate);
        harness
            .fixture
            .store
            .set_burn_check_test_hook(Some(Arc::new(move |actual| {
                if actual == stage {
                    callback.block();
                }
            })));
        let task = harness.job();
        gate.wait().await;
        if deferred {
            harness
                .runtime
                .installed
                .stopping
                .store(true, Ordering::Release);
            gate.open();
            assert_eq!(
                task.await.unwrap().unwrap().unwrap().outcome,
                RunOutcome::Deferred
            );
            harness
                .runtime
                .installed
                .stopping
                .store(false, Ordering::Release);
        } else {
            harness.select("b");
            harness.select("a");
            gate.open();
            let settlement = task.await.unwrap().unwrap();
            match stage {
                StoreStage::BeforeAnswer => assert!(settlement.is_none()),
                StoreStage::AfterAnswer => {
                    assert_eq!(settlement.unwrap().outcome, RunOutcome::Deferred)
                }
                StoreStage::BeforeAcceptance => {
                    assert_eq!(settlement.unwrap().outcome, RunOutcome::Stale)
                }
                _ => panic!("Unexpected local switch stage: {stage:?}"),
            }
        }
        harness.fixture.store.set_burn_check_test_hook(None);
        if deferred {
            harness.select("b");
            harness.select("a");
        }
        harness.host.catch_up().unwrap();
        assert_eq!(
            count(&harness.fixture.store, "burn_check_current_status"),
            0
        );
        let cancelled: u64 = harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_candidate WHERE state = 'cancelled'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cancelled, 1);
        harness.publish(2);
        let settlement = harness.job().await.unwrap().unwrap().unwrap();
        assert_eq!(settlement.outcome, RunOutcome::Accepted);
        assert_eq!(settlement.attempts, 0);
        assert_provider_free(&harness, &a, &b);
    }
}
