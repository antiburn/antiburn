use super::*;
use crate::store::burn_checks::trace_capture::LogCapture;

fn activation(definition_id: &str) -> antiburn_checks::definition_lifecycle::EffectiveActivation {
    use antiburn_checks::discovery::DefinitionScope;
    use antiburn_checks::domain::{ActivationScope, Generation, ObservationPosition, RunScope};
    use antiburn_checks::source::PublicationCursor;
    antiburn_checks::definition_lifecycle::EffectiveActivation {
        id: ActivationId {
            scope: ActivationScope {
                environment: "native".into(),
                session_cwd: "/PRIVATE_CONTEXT_CWD".into(),
                run: RunScope::Live,
            },
            definition_id: definition_id.into(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: DefinitionScope::Global,
        source_paths: vec![],
        project_root: None,
        compiled: std::sync::Arc::new(antiburn_checks::compiler::compile("name: Trace\non:\n  tool_result:\nactions:\n  - type: status\n    input: {result: failure}\n").unwrap()),
        bootstrap_observation_position: ObservationPosition(0),
        bootstrap_publication: PublicationCursor(0),
    }
}

struct TraceRuntime {
    store: std::sync::Arc<crate::store::Store>,
    worker: crate::jev::worker::WorkerHandle,
    installed: super::super::InstalledChecks,
}

impl super::super::SelectedJobRuntime for TraceRuntime {
    fn store(&self) -> &crate::store::Store {
        &self.store
    }
    fn worker(&self) -> &crate::jev::worker::WorkerHandle {
        &self.worker
    }
    fn installed(&self) -> &super::super::InstalledChecks {
        &self.installed
    }
}

impl TraceRuntime {
    fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            store: std::sync::Arc::new(
                crate::store::Store::open_in_memory(Path::new("/PRIVATE_TRACE_STORE")).unwrap(),
            ),
            worker: Default::default(),
            installed: Default::default(),
        })
    }
}

#[test]
fn definition_trace_preserves_error_severity_and_logs_safe_transitions_once() {
    let path = PathBuf::from("/PRIVATE_DIAGNOSTIC_PATH/PRIVATE_CHECK_NAME.yml");
    let diagnostic = Diagnostic {
        code: DiagnosticCode::DiscoveryReadFailed,
        message: "PRIVATE_ERROR_BODY credential=PRIVATE_KEY".into(),
        config_path: Some("PRIVATE_CONFIG_LOCATION".into()),
        span: Some(antiburn_checks::diagnostics::SourceSpan::point(0, 3, 5)),
        source_path: Some(path),
    };
    let mut batch = DiagnosticBatch::new();
    collect(&mut batch, &[diagnostic]);
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        let mut log = DiagnosticLog::default();
        log.publish(batch.clone());
        for _ in 0..100 {
            log.publish(batch.clone());
        }
        log.publish(DiagnosticBatch::new());
        log.publish(batch);
    });
    let events = captured.events("installed_check_invalid");
    assert_eq!(events.len(), 2);
    for event in events {
        assert_eq!(event.level, ::tracing::Level::ERROR);
        assert_eq!(event.fields["code"], "\"discovery-read-failed\"");
        assert_eq!(event.fields["line"], "3");
        assert_eq!(event.fields["column"], "5");
        assert_eq!(event.fields.len(), 4);
    }
    assert!(!captured.text().contains("PRIVATE_"));
}

#[test]
fn runtime_failures_keep_distinct_safe_stages_categories_and_sqlite_codes() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    let sqlite = connection
        .execute_batch(
            "CREATE TABLE PRIVATE_TABLE (id INTEGER UNIQUE);
        INSERT INTO PRIVATE_TABLE VALUES (1); INSERT INTO PRIVATE_TABLE VALUES (1);",
        )
        .unwrap_err();
    let sqlite = anyhow::Error::new(sqlite).context("PRIVATE_SQLITE_CONTEXT /PRIVATE_PATH");
    let invariant = anyhow::anyhow!("PRIVATE_INVARIANT_BODY credential=PRIVATE_KEY");
    let failures = [
        RuntimeFailure::classify(RuntimeStage::Reconcile, &sqlite),
        RuntimeFailure::classify(RuntimeStage::SourceCatchUp, &invariant),
    ];
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        let mut previous = None;
        for failure in &failures {
            for _ in 0..100 {
                failure.trace_if_changed(&mut previous, "installed_check_update_failed");
            }
        }
    });
    let events = captured.events("installed_check_update_failed");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].fields["stage"], "\"reconcile\"");
    assert_eq!(events[0].fields["reason"], "\"sqlite\"");
    assert_eq!(events[0].fields["sqlite_code"], "2067");
    assert_eq!(events[1].fields["stage"], "\"source_catch_up\"");
    assert_eq!(events[1].fields["reason"], "\"runtime_invariant\"");
    assert!(!events[1].fields.contains_key("sqlite_code"));
    assert!(
        events
            .iter()
            .all(|event| event.level == tracing::Level::ERROR)
    );
    assert!(!captured.text().contains("PRIVATE_"));
    for failure in failures {
        let diagnostic = failure.diagnostic();
        assert!(diagnostic.details.iter().any(
            |detail| detail.label == "Runtime stage" && detail.value == failure.stage.as_str()
        ));
        assert!(
            diagnostic
                .details
                .iter()
                .any(|detail| detail.label == "Failure category"
                    && detail.value == failure.cause.category)
        );
        assert!(
            !serde_json::to_string(&diagnostic)
                .unwrap()
                .contains("PRIVATE_")
        );
    }
}

#[test]
fn current_context_trace_keeps_sqlite_cause_and_resets_after_a_successful_capture() {
    use antiburn_checks::domain::{
        BranchId, EvidenceVersion, FrozenContext, NativeOrderKey, Observation, ObservationKind,
        ObservationPosition,
    };
    use antiburn_checks::source::PublicationCursor;

    let store = crate::store::Store::open_in_memory(Path::new("/PRIVATE_TRACE_STORE")).unwrap();
    store
        .lock()
        .execute_batch(
            "ALTER TABLE native_tool_result_publication RENAME TO PRIVATE_PUBLICATION_TABLE",
        )
        .unwrap();
    let activation = activation("private-current-context-check");
    let observation = Observation {
        position: ObservationPosition(1),
        branch: BranchId("PRIVATE_BRANCH".into()),
        native_order: NativeOrderKey("PRIVATE_ORDER".into()),
        first_terminal_publication: None,
        first_eligible_publication: None,
        version: EvidenceVersion("PRIVATE_VERSION".into()),
        kind: ObservationKind::Other,
        context: FrozenContext {
            value: serde_json::json!({"credential": "PRIVATE_KEY"}),
            dependencies: vec![],
        },
        complete: true,
    };
    let captured = LogCapture::default();
    let mut capture = super::super::current_context::CurrentContextCapture::new(Some(
        PathBuf::from("/PRIVATE_HOME"),
    ));
    tracing::subscriber::with_default(captured.clone(), || {
        for _ in 0..100 {
            assert!(
                capture
                    .capture(&store, &activation, PublicationCursor(1), &observation)
                    .is_err()
            );
        }
        assert_eq!(
            captured.events("burn_check_runtime_operation_failed").len(),
            1
        );
        super::super::current_context::CurrentContextCapture::default()
            .capture(&store, &activation, PublicationCursor(1), &observation)
            .unwrap();
        assert!(
            capture
                .capture(&store, &activation, PublicationCursor(1), &observation)
                .is_err()
        );
    });
    let failures = captured.events("burn_check_runtime_operation_failed");
    assert_eq!(failures.len(), 2);
    for failure in failures {
        assert_eq!(failure.level, tracing::Level::ERROR);
        assert_eq!(failure.fields["stage"], "\"current_context\"");
        assert_eq!(failure.fields["reason"], "\"sqlite\"");
        assert_eq!(failure.fields["sqlite_code"], "1");
        assert_eq!(
            failure.fields["activation_ref"],
            format!("{:?}", trace_refs::activation_ref(&activation.id))
        );
    }
    assert_eq!(capture.calls, 0);
    assert!(!captured.text().contains("PRIVATE_"));
    assert!(!captured.text().contains("private-current-context-check"));
}

#[tokio::test]
async fn outer_job_trace_deduplicates_real_work_probe_failures_per_activation_and_success() {
    use super::super::{
        CheckEngine, EvaluationJob, InstalledDecisionEvaluator, SelectedJobRuntime, StoreCheckStore,
    };
    use std::sync::Arc;
    use tracing::instrument::WithSubscriber;

    let runtime = TraceRuntime::new();
    let selected: Arc<dyn SelectedJobRuntime> = runtime.clone();
    let evaluator = Arc::new(InstalledDecisionEvaluator::factory(selected.clone()));
    let first = activation("PRIVATE_FIRST_JOB");
    let second = activation("PRIVATE_SECOND_JOB");
    let adapter = Arc::new(
        StoreCheckStore::new(
            runtime.store.clone(),
            "PRIVATE_OWNER".into(),
            first.id.scope.clone(),
        )
        .unwrap(),
    );
    let engine = Arc::new(CheckEngine::new(adapter.clone(), evaluator));
    let job =
        |activation: &antiburn_checks::definition_lifecycle::EffectiveActivation| EvaluationJob {
            engine: engine.clone(),
            store: adapter.clone(),
            activation: activation.clone(),
        };
    let fail_probe = || {
        runtime
            .store
            .lock()
            .execute_batch(
                "ALTER TABLE burn_check_session_attachment RENAME TO PRIVATE_ATTACHMENT_TABLE",
            )
            .unwrap()
    };
    fail_probe();
    let captured = LogCapture::default();
    async {
        for _ in 0..100 {
            for activation in [&first, &second] {
                assert!(
                    job(activation)
                        .run_selected(selected.clone())
                        .await
                        .is_err()
                );
            }
        }
        assert_eq!(
            captured.events("installed_check_settlement_failed").len(),
            2
        );
        runtime
            .store
            .lock()
            .execute_batch(
                "ALTER TABLE PRIVATE_ATTACHMENT_TABLE RENAME TO burn_check_session_attachment",
            )
            .unwrap();
        assert!(
            job(&first)
                .run_selected(selected.clone())
                .await
                .unwrap()
                .is_none()
        );
        fail_probe();
        for _ in 0..100 {
            for activation in [&first, &second] {
                assert!(
                    job(activation)
                        .run_selected(selected.clone())
                        .await
                        .is_err()
                );
            }
        }
    }
    .with_subscriber(captured.clone())
    .await;
    let failures = captured.events("installed_check_settlement_failed");
    assert_eq!(failures.len(), 3);
    for failure in &failures {
        assert_eq!(failure.level, tracing::Level::ERROR);
        assert_eq!(failure.fields["stage"], "\"work_probe\"");
        assert_eq!(failure.fields["reason"], "\"sqlite\"");
        assert_eq!(failure.fields["sqlite_code"], "1");
    }
    assert_eq!(
        failures[0].fields["activation_ref"],
        failures[2].fields["activation_ref"]
    );
    assert_ne!(
        failures[0].fields["activation_ref"],
        failures[1].fields["activation_ref"]
    );
    assert!(runtime.worker.execution_snapshot().is_none());
    assert!(!captured.text().contains("PRIVATE_"));
}

#[tokio::test(start_paused = true)]
async fn idle_query_retries_keep_safe_causes_deduplicate_logs_and_reset_after_recovery() {
    use super::super::{IdleQuery, InstalledCheckHost, InstalledDecisionEvaluator};
    use std::time::Duration;
    use tracing::instrument::WithSubscriber;

    let runtime = TraceRuntime::new();
    let evaluator = std::sync::Arc::new(InstalledDecisionEvaluator::factory(runtime.clone()));
    let host = InstalledCheckHost::new(runtime.store.clone(), evaluator, None).unwrap();
    let fail_queries = || {
        runtime
            .store
            .lock()
            .execute_batch(
                "ALTER TABLE burn_check_session_attachment RENAME TO PRIVATE_ATTACHMENT_TABLE",
            )
            .unwrap();
    };
    fail_queries();
    let mut deadline = IdleQuery::default();
    let mut catch_up = IdleQuery::default();
    let captured = LogCapture::default();
    async {
        for _ in 0..100 {
            assert!(
                deadline
                    .run(RuntimeStage::IdleDeadline, || host.next_idle_deadline())
                    .is_none()
            );
            assert!(
                catch_up
                    .run(RuntimeStage::IdleCatchUp, || host.request_idle_catch_up(1))
                    .is_none()
            );
            tokio::time::advance(Duration::from_millis(999)).await;
            for query in [&mut deadline, &mut catch_up] {
                assert!(
                    query
                        .run::<()>(RuntimeStage::Worker, || panic!(
                            "idle queries must wait for the next poll"
                        ),)
                        .is_none()
                );
            }
            tokio::time::advance(Duration::from_millis(1)).await;
        }
        assert_eq!(captured.events("installed_check_update_failed").len(), 2);
        for query in [&deadline, &catch_up] {
            let diagnostic = query.failure.unwrap().diagnostic();
            assert!(
                diagnostic.details.iter().any(|detail| {
                    detail.label == "Failure category" && detail.value == "sqlite"
                })
            );
            assert!(
                !serde_json::to_string(&diagnostic)
                    .unwrap()
                    .contains("PRIVATE_")
            );
        }
        runtime
            .store
            .lock()
            .execute_batch(
                "ALTER TABLE PRIVATE_ATTACHMENT_TABLE RENAME TO burn_check_session_attachment",
            )
            .unwrap();
        assert!(
            deadline
                .run(RuntimeStage::IdleDeadline, || host.next_idle_deadline())
                .is_some()
        );
        assert!(
            catch_up
                .run(RuntimeStage::IdleCatchUp, || host.request_idle_catch_up(1))
                .is_some()
        );
        assert!(deadline.failure.is_none());
        assert!(catch_up.failure.is_none());
        fail_queries();
        assert!(
            deadline
                .run(RuntimeStage::IdleDeadline, || host.next_idle_deadline())
                .is_none()
        );
        assert!(
            catch_up
                .run(RuntimeStage::IdleCatchUp, || host.request_idle_catch_up(1))
                .is_none()
        );
    }
    .with_subscriber(captured.clone())
    .await;
    let failures = captured.events("installed_check_update_failed");
    assert_eq!(failures.len(), 4);
    for (failure, stage) in failures
        .iter()
        .zip(["idle_deadline", "idle_catch_up"].into_iter().cycle())
    {
        assert_eq!(failure.level, tracing::Level::ERROR);
        assert_eq!(failure.fields["stage"], format!("{stage:?}"));
        assert_eq!(failure.fields["reason"], "\"sqlite\"");
        assert_eq!(failure.fields["sqlite_code"], "1");
    }
    assert!(captured.events("installed_checks_stopped").is_empty());
    assert!(!captured.text().contains("PRIVATE_"));
}

#[test]
fn worker_trace_keeps_actual_idle_query_sqlite_causes_before_the_stop_boundary() {
    use super::super::{InstalledCheckHost, InstalledDecisionEvaluator};
    let runtime = TraceRuntime::new();
    let evaluator = std::sync::Arc::new(InstalledDecisionEvaluator::factory(runtime.clone()));
    let host = InstalledCheckHost::new(runtime.store.clone(), evaluator, None).unwrap();
    runtime
        .store
        .lock()
        .execute_batch(
            "ALTER TABLE burn_check_session_attachment RENAME TO PRIVATE_ATTACHMENT_TABLE",
        )
        .unwrap();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        trace_worker_result(host.next_idle_deadline().map(|_| ()));
        trace_worker_result(host.request_idle_catch_up(1).map(|_| ()));
        trace_worker_result(Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "PRIVATE_IO_BODY /PRIVATE_PATH",
        )
        .into()));
        runtime
            .store
            .lock()
            .execute_batch(
                "ALTER TABLE PRIVATE_ATTACHMENT_TABLE RENAME TO burn_check_session_attachment",
            )
            .unwrap();
        trace_worker_result(host.next_idle_deadline().map(|_| ()));
        trace_worker_result(host.request_idle_catch_up(1).map(|_| ()));
    });
    let failures = captured.events("installed_checks_stopped");
    assert_eq!(failures.len(), 3);
    for (failure, stage) in failures
        .iter()
        .zip(["idle_deadline", "idle_catch_up", "worker"])
    {
        assert_eq!(failure.level, tracing::Level::ERROR);
        assert_eq!(failure.fields["stage"], format!("{stage:?}"));
    }
    for failure in &failures[..2] {
        assert_eq!(failure.fields["reason"], "\"sqlite\"");
        assert_eq!(failure.fields["sqlite_code"], "1");
    }
    assert_eq!(failures[2].fields["reason"], "\"io\"");
    assert_eq!(failures[2].fields["io_kind"], "Some(PermissionDenied)");
    assert!(!captured.text().contains("PRIVATE_"));
}
