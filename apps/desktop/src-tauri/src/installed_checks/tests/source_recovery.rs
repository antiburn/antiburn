use super::super::execution::CapabilityFuture;
use super::*;
use crate::jev::config::{
    ContextLimitOverride, SystemOneConnection, SystemOneEndpoint, SystemOneResponseMode,
};
use crate::jev::worker::{ExecutionSnapshot, WorkerHandle};
use crate::store::native_tool_results::source_delivery::opencode_v2::{
    OpenCodeV2AdmittedSource, OpenCodeV2CaptureClaim, OpenCodeV2CaptureStart,
};
use antiburn_local::analysis::opencode_v2_events::OpenCodeV2EventJoiner;
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SOURCE: &str = r#"name: Source recovery
on:
  tool_result:
  cooldown: 0s
decisions:
  review:
    input: {result: '${{ message.tool.result }}'}
    output:
      verdict:
        type: choice
        question: Is the failure resolved?
        criteria: {clear: Resolved., concern: Unresolved.}
actions:
  - type: status
    input: {result: failure}
"#;

struct Runtime {
    store: Arc<Store>,
    worker: WorkerHandle,
    installed: InstalledChecks,
    discovery_failed: AtomicBool,
    discoveries: AtomicUsize,
}

impl Runtime {
    fn new(fixture: &Fixture, endpoint: String) -> Arc<Self> {
        let worker = WorkerHandle::default();
        worker
            .set_system_one_connection(
                SystemOneConnection {
                    provider: SystemOneProvider::Custom,
                    endpoint: SystemOneEndpoint::ExactUrl(endpoint),
                    model: "synthetic-v2-model".into(),
                    model_revision: None,
                    response_mode: SystemOneResponseMode::Direct,
                    credential: None,
                    revision: 1,
                    context_override: Some(ContextLimitOverride {
                        total_input_tokens: Some(32_768),
                        state_and_longest_question_tokens: Some(16_384),
                        runtime_context_tokens: None,
                    }),
                },
                None,
            )
            .unwrap();
        let runtime = Arc::new(Self {
            store: Arc::clone(&fixture.store),
            worker,
            installed: InstalledChecks::default(),
            discovery_failed: AtomicBool::new(false),
            discoveries: AtomicUsize::new(0),
        });
        runtime
            .installed
            .credentials_restored
            .store(true, Ordering::Release);
        runtime
    }

    fn delivery_ready(&self) -> bool {
        live_delivery_permitted(
            &self.store,
            self.installed.delivery_ready.load(Ordering::Acquire),
            self.installed.stopping.load(Ordering::Acquire),
            admission_for_worker(&self.worker) == Admission::Ready,
        )
    }
}

impl SelectedJobRuntime for Runtime {
    fn store(&self) -> &Store {
        &self.store
    }

    fn worker(&self) -> &WorkerHandle {
        &self.worker
    }

    fn installed(&self) -> &InstalledChecks {
        &self.installed
    }

    fn resolve<'a>(&'a self, snapshot: &'a ExecutionSnapshot) -> CapabilityFuture<'a> {
        Box::pin(async move {
            self.discoveries.fetch_add(1, Ordering::AcqRel);
            ensure!(
                !self.discovery_failed.load(Ordering::Acquire),
                "Synthetic capability outage"
            );
            let mut capabilities = ModelCapabilities::jev_default();
            capabilities.model.clone_from(&snapshot.connection.model);
            Ok(snapshot.connection.apply_capability_overrides(capabilities))
        })
    }
}

fn host(
    fixture: &Fixture,
    runtime: &Arc<Runtime>,
) -> InstalledCheckHost<InstalledDecisionEvaluator> {
    let selected: Arc<dyn SelectedJobRuntime> = runtime.clone();
    InstalledCheckHost::new(
        Arc::clone(&fixture.store),
        Arc::new(InstalledDecisionEvaluator::factory(selected)),
        Some(fixture.global.clone()),
    )
    .unwrap()
}

fn reconcile(host: &mut InstalledCheckHost<InstalledDecisionEvaluator>, runtime: &Runtime) {
    let generation = runtime
        .worker
        .execution_snapshot()
        .map(|saved| saved.generation);
    (host.capabilities, host.capability_error) =
        execution::coordinator_capabilities(&runtime.installed, generation);
    let admission = admission_for_worker(&runtime.worker);
    let update = host.reconcile(&admission);
    assert!(finish_inventory_update(
        &runtime.installed,
        host,
        update,
        diagnostics::RuntimeStage::Reconcile,
        &mut None,
        |_| {},
    ));
    update_delivery_readiness(
        &runtime.installed,
        host,
        admission == Admission::Ready && admission_for_worker(&runtime.worker) == Admission::Ready,
    );
}

fn drain(host: &mut InstalledCheckHost<InstalledDecisionEvaluator>) {
    for _ in 0..8 {
        if !host.catch_up().unwrap().1 {
            return;
        }
    }
    panic!("the synthetic V2 source must fit the catch-up bound");
}

fn scalar<T: rusqlite::types::FromSql>(store: &Store, sql: &str) -> T {
    store.lock().query_row(sql, [], |row| row.get(0)).unwrap()
}

fn candidate_state(store: &Store, candidate: &str) -> String {
    store
        .lock()
        .query_row(
            "SELECT state FROM burn_check_candidate WHERE candidate_id = ?1",
            [candidate],
            |row| row.get(0),
        )
        .unwrap()
}

struct Capture {
    claim: OpenCodeV2CaptureClaim,
    joiner: OpenCodeV2EventJoiner,
    sequence: u64,
}

impl Capture {
    fn index(fixture: &Fixture) -> OpenCodeV2CaptureStart {
        let pi_key = SessionKey::new("native", "pi", "main");
        let mut record = fixture.store.session(&pi_key).unwrap().unwrap();
        fixture.store.delete_session(&pi_key).unwrap();
        record.key = SessionKey::new("native", "opencode", "v2");
        record.source_kind = "providerDb".into();
        record.source_label = "opencode:v2:open_code_v2_sqlite".into();
        let admitted_source = OpenCodeV2AdmittedSource {
            database_path: fixture.directory.path().join("opencode.db"),
            file_identity: "synthetic-database-identity".into(),
        };
        record.activity_cursor = format!(
            "opencode-admission-v2:{}",
            json!([
                admitted_source.database_path,
                "v2",
                admitted_source.file_identity,
                "synthetic-scope"
            ])
        );
        fixture
            .store
            .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
            .unwrap();
        let (presence, _) = fixture
            .store
            .session_presence_for_keys(std::slice::from_ref(&record.key))
            .unwrap();
        let source_generation = fixture
            .store
            .source_generation_batch(std::slice::from_ref(&record.key))
            .unwrap()[0]
            .unwrap();
        OpenCodeV2CaptureStart {
            key: record.key,
            incarnation: presence[0].incarnation,
            source_generation,
            source_label: record.source_label,
            cwd: record.cwd.unwrap(),
            admitted_source,
            baseline_sequence: Some(0),
        }
    }

    fn start(runtime: &Runtime, start: OpenCodeV2CaptureStart) -> Self {
        assert!(runtime.delivery_ready());
        Self {
            claim: runtime
                .store
                .start_opencode_v2_capture_epoch(
                    &[start],
                    "source-recovery",
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                    600,
                )
                .unwrap()
                .unwrap()
                .remove(0),
            joiner: OpenCodeV2EventJoiner::new(),
            sequence: 0,
        }
    }

    fn settings_check(&self, runtime: &Runtime) {
        if !runtime.delivery_ready() {
            assert_eq!(
                runtime
                    .store
                    .mark_opencode_v2_capture_gap(
                        std::slice::from_ref(&self.claim),
                        "checks-disabled",
                        time::OffsetDateTime::now_utc().unix_timestamp(),
                    )
                    .unwrap(),
                1
            );
        }
    }

    fn emit(&mut self, runtime: &Runtime, kind: &str, extra: Value) {
        assert!(runtime.delivery_ready());
        self.sequence += 1;
        let mut data = json!({"sessionID":"v2"});
        data.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        if matches!(kind, "session.tool.called" | "session.tool.failed") {
            data["executed"] = json!(false);
        }
        let event = json!({"id":format!("v2-event-{}", self.sequence), "type":kind, "created":1000,
            "durable":{"aggregateID":"v2", "seq":self.sequence, "version":if kind == "session.tool.failed" {2} else {1}}, "data":data});
        let result = self
            .joiner
            .observe(&event)
            .map(|joined| joined.into_observation(false, true));
        assert!(
            runtime
                .store
                .advance_opencode_v2_capture(
                    &self.claim,
                    self.sequence,
                    result.as_ref(),
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                    600,
                )
                .unwrap()
        );
    }

    fn result(&mut self, runtime: &Runtime, call: usize) {
        self.emit(
            runtime,
            "session.tool.input.started",
            json!({"assistantMessageID":"step", "id":format!("call-{call}"), "name":"shell"}),
        );
        self.emit(runtime, "session.tool.called", json!({"assistantMessageID":"step", "id":format!("call-{call}"), "input":{"command":"synthetic-check"}}));
        self.emit(runtime, "session.tool.failed", json!({"assistantMessageID":"step", "id":format!("call-{call}"), "content":[{"type":"text", "text":"Synthetic failure"}]}));
    }
}

async fn model_server() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/systemone", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            let (body_start, body_size) = loop {
                let read = socket.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&buffer[..read]);
                assert!(bytes.len() <= 256 * 1024);
                if let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    assert!(headers.starts_with("POST /systemone "));
                    let size = headers.lines().find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                    }).unwrap();
                    break (end + 4, size);
                }
            };
            assert!(body_size <= 256 * 1024);
            while bytes.len() < body_start + body_size {
                let read = socket.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&buffer[..read]);
            }
            let request: Value = serde_json::from_slice(&bytes[body_start..body_start + body_size]).unwrap();
            assert_eq!(request["model"], "synthetic-v2-model");
            let body = json!({"model":"synthetic-v2-model", "answers":{"verdict":{"type":"choice", "choice":"concern", "probabilities":{"clear":0.01,"concern":0.99},"confidence":0.99}}, "usage":{"input_tokens":12,"output_tokens":3}}).to_string();
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        }).await.expect("the recovered assessment must reach the synthetic model server");
    });
    (endpoint, server)
}

#[tokio::test]
async fn v2_capture_survives_capability_outage_and_recovers_on_the_same_connection() {
    let fixture = Fixture::new();
    std::fs::write(fixture.global.join("failed-retry-loop.yml"), SOURCE).unwrap();
    let start = Capture::index(&fixture);
    let (endpoint, server) = model_server().await;
    let runtime = Runtime::new(&fixture, endpoint);
    let generation = runtime.worker.execution_snapshot().unwrap().generation;
    let mut host = host(&fixture, &runtime);
    reconcile(&mut host, &runtime);
    assert_eq!(host.definitions.len(), 1);
    assert!(
        !host.projects.values().next().unwrap().active[0]
            .compiled
            .is_local()
    );
    let activation = host.projects.values().next().unwrap().active[0].id.clone();
    let mut capture = Capture::start(&runtime, start);
    capture.emit(&runtime, "session.inbox.enqueued", json!({"inboxID":"request", "item":{"type":"user", "delivery":"queue", "payload":{"text":"Inspect the synthetic failure"}}}));
    capture.emit(
        &runtime,
        "session.inbox.delivered",
        json!({"inboxID":"request"}),
    );
    capture.emit(
        &runtime,
        "session.step.started",
        json!({"assistantMessageID":"step"}),
    );
    drain(&mut host);
    capture.result(&runtime, 0);
    drain(&mut host);
    let gap_count = || {
        scalar::<i64>(
            &fixture.store,
            "SELECT COUNT(*) FROM native_tool_result_publication WHERE gap_kind = 'segment_reset'",
        )
    };
    let before_gaps = gap_count();
    let candidate: String = scalar(
        &fixture.store,
        "SELECT candidate_id FROM burn_check_candidate WHERE state = 'pending'",
    );
    fixture
        .store
        .record_burn_check_decision_usage(
            &"f".repeat(64),
            &DecisionAttempt::new("review", 1).unwrap(),
            "previous-check",
            ("custom-systemone", "previous-model"),
            None,
            1,
        )
        .unwrap();

    runtime.discovery_failed.store(true, Ordering::Release);
    assert!(
        host.evaluation_jobs(1)
            .pop()
            .unwrap()
            .run_selected(runtime.clone())
            .await
            .unwrap()
            .is_none()
    );
    reconcile(&mut host, &runtime);
    assert!(matches!(
        host.definitions[0].availability,
        InstalledCheckAvailability::Unsupported { .. }
    ));
    assert_eq!(
        scalar::<String>(
            &fixture.store,
            "SELECT availability FROM burn_check_runtime_definition"
        ),
        "unsupported"
    );
    assert!(
        runtime.installed.metadata.read().unwrap()[0]
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "provider-unavailable")
    );
    capture.settings_check(&runtime);
    drain(&mut host);
    assert_eq!(candidate_state(&fixture.store, &candidate), "pending");
    assert_eq!(gap_count(), before_gaps);
    assert!(runtime.delivery_ready());
    capture.result(&runtime, 1);
    drain(&mut host);
    assert!(
        host.evaluation_jobs(1)
            .pop()
            .unwrap()
            .run_selected(runtime.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(runtime.discoveries.load(Ordering::Acquire), 1);
    assert_eq!(
        scalar::<i64>(
            &fixture.store,
            "SELECT COUNT(*) FROM burn_check_evaluation_attempt"
        ),
        0
    );

    runtime.discovery_failed.store(false, Ordering::Release);
    runtime
        .installed
        .capabilities
        .write()
        .unwrap()
        .as_mut()
        .unwrap()
        .retry_at = Instant::now();
    reconcile(&mut host, &runtime);
    assert!(matches!(
        host.definitions[0].availability,
        InstalledCheckAvailability::Unsupported { .. }
    ));
    assert!(runtime.delivery_ready());
    let settled = host
        .evaluation_jobs(1)
        .pop()
        .unwrap()
        .run_selected(runtime.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.outcome, RunOutcome::Accepted);
    server.await.unwrap();
    reconcile(&mut host, &runtime);
    assert_eq!(
        runtime.worker.execution_snapshot().unwrap().generation,
        generation
    );
    assert_eq!(runtime.discoveries.load(Ordering::Acquire), 2);
    assert_eq!(
        host.projects.values().next().unwrap().active[0].id,
        activation
    );
    assert_eq!(
        host.definitions[0].availability,
        InstalledCheckAvailability::Available
    );
    assert!(runtime.delivery_ready());
    let usage = fixture.store.burn_check_usage_summary().unwrap();
    assert_eq!((usage.confirmed_calls, usage.unknown_outcomes), (1, 1));

    runtime.installed.stopping.store(true, Ordering::Release);
    assert!(!runtime.delivery_ready());
    capture.settings_check(&runtime);
    assert_eq!(
        scalar::<String>(
            &fixture.store,
            "SELECT gap_kind FROM native_tool_result_publication WHERE gap_reason = 'checks-disabled'"
        ),
        "segment_reset"
    );
    drain(&mut host);
    assert_eq!(candidate_state(&fixture.store, &candidate), "accepted");
    assert_eq!(
        scalar::<i64>(
            &fixture.store,
            "SELECT COUNT(*) FROM burn_check_candidate WHERE state = 'pending'"
        ),
        0
    );
    assert_eq!(fixture.store.burn_check_usage_summary().unwrap(), usage);
}

#[tokio::test]
async fn source_delivery_keeps_base_and_inventory_gates_closed() {
    for gate in [
        "master",
        "credentials",
        "shutdown",
        "provider",
        "authentication",
        "empty",
        "invalid",
        "unsupported_definition",
        "paused",
        "inventory",
    ] {
        let fixture = Fixture::new();
        std::fs::write(fixture.global.join("failed-retry-loop.yml"), SOURCE).unwrap();
        let runtime = Runtime::new(&fixture, "http://127.0.0.1:1/systemone".into());
        let mut host = host(&fixture, &runtime);
        reconcile(&mut host, &runtime);
        let active = host.projects.values().next().unwrap().active[0].clone();
        match gate {
            "master" => fixture
                .store
                .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "false")
                .unwrap(),
            "credentials" => runtime
                .installed
                .credentials_restored
                .store(false, Ordering::Release),
            "shutdown" => runtime.installed.stopping.store(true, Ordering::Release),
            "provider" => runtime.worker.suspend_system_one(),
            "authentication" => {
                assert!(
                    runtime
                        .worker
                        .reject_authentication(
                            &fixture.store,
                            runtime.worker.execution_snapshot().unwrap().generation
                        )
                        .unwrap()
                );
            }
            "empty" => {
                std::fs::remove_file(fixture.global.join("failed-retry-loop.yml")).unwrap();
                host.reconcile(&Admission::Ready).unwrap();
            }
            "invalid" => {
                std::fs::write(fixture.global.join("failed-retry-loop.yml"), "name: [").unwrap();
                host.reconcile(&Admission::Ready).unwrap();
            }
            "unsupported_definition" => {
                std::fs::write(
                    fixture.global.join("failed-retry-loop.yml"),
                    "name: Unsupported\non: {tool_result: null}\nactions:\n  - type: message\n    input: {text: Unsupported action}\n",
                )
                .unwrap();
                host.reconcile(&Admission::Ready).unwrap();
                assert!(matches!(
                    host.definitions[0].availability,
                    InstalledCheckAvailability::Unsupported { .. }
                ));
            }
            "paused" => host
                .reconcile(&Admission::Paused("Synthetic repository pause".into()))
                .unwrap(),
            "inventory" => {
                std::fs::write(
                    fixture.global.join("failed-retry-loop.yml"),
                    SOURCE.replace("name: Source recovery", "name: Inventory retry"),
                )
                .unwrap();
                fixture.store.lock().execute_batch("CREATE TRIGGER reject_inventory BEFORE UPDATE ON burn_check_runtime_definition_state BEGIN SELECT RAISE(ABORT, 'synthetic inventory failure'); END;").unwrap();
                let update = host.reconcile(&Admission::Ready);
                assert!(update.is_err());
                assert!(matches!(
                    update
                        .as_ref()
                        .unwrap_err()
                        .downcast_ref::<rusqlite::Error>(),
                    Some(rusqlite::Error::SqliteFailure(_, Some(message)))
                        if message == "synthetic inventory failure"
                ));
                assert!(!host.inventory_ready);
                assert!(
                    host.projects
                        .values()
                        .any(|project| !project.active.is_empty())
                );
                assert_eq!(
                    scalar::<String>(
                        &fixture.store,
                        "SELECT name FROM burn_check_runtime_definition"
                    ),
                    "Source recovery"
                );
                assert!(!finish_inventory_update(
                    &runtime.installed,
                    &host,
                    update,
                    diagnostics::RuntimeStage::Reconcile,
                    &mut None,
                    |_| {}
                ));
                assert!(!runtime.delivery_ready());
                assert_eq!(
                    scalar::<String>(
                        &fixture.store,
                        "SELECT availability FROM burn_check_runtime_definition"
                    ),
                    "paused"
                );
            }
            _ => unreachable!(),
        }
        update_delivery_readiness(
            &runtime.installed,
            &host,
            admission_for_worker(&runtime.worker) == Admission::Ready,
        );
        assert!(!runtime.delivery_ready(), "{gate}");
        if matches!(
            gate,
            "master" | "credentials" | "shutdown" | "provider" | "authentication"
        ) {
            assert!(
                InstalledDecisionEvaluator::for_assessment(runtime.clone(), &active)
                    .await
                    .unwrap()
                    .is_none(),
                "{gate}"
            );
        }
        assert_eq!(runtime.discoveries.load(Ordering::Acquire), 0, "{gate}");
        assert_eq!(
            scalar::<i64>(&fixture.store, "SELECT COUNT(*) FROM opencode_v2_capture"),
            0,
            "{gate}"
        );
        if gate == "inventory" {
            fixture
                .store
                .lock()
                .execute_batch("DROP TRIGGER reject_inventory")
                .unwrap();
            reconcile(&mut host, &runtime);
            assert!(runtime.delivery_ready());
            assert_eq!(host.definitions[0].name, "Inventory retry");
            assert_eq!(
                scalar::<String>(
                    &fixture.store,
                    "SELECT name FROM burn_check_runtime_definition"
                ),
                "Inventory retry"
            );
            assert_eq!(
                scalar::<String>(
                    &fixture.store,
                    "SELECT availability FROM burn_check_runtime_definition"
                ),
                "available"
            );
            assert_eq!(runtime.discoveries.load(Ordering::Acquire), 0);
        }
    }
}
