use super::super::execution::{CapabilityFuture, SelectedJobRuntime, Stage};
use super::*;
use crate::jev::config::{
    ContextLimitOverride, SystemOneConnection, SystemOneEndpoint, SystemOneResponseMode,
};
use crate::jev::settings::headless;
use crate::jev::worker::{ExecutionSnapshot, WorkerHandle};
use crate::store::burn_checks::test_hooks::Stage as StoreStage;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Condvar;
use std::sync::atomic::AtomicUsize;

const SOURCE: &str = "name: Selected connection\non:\n  tool_result:\n  cooldown: 0s\ndecisions:\n  review:\n    input: {activity: '${{ message.tool.result }}'}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {clear: Clear., concern: Concern.}\nactions:\n  - type: status\n    if: decisions.review.outputs.verdict == 'concern'\n    input: {result: failure}\n  - type: status\n    if: decisions.review.outputs.verdict == 'clear'\n    input: {result: success}\n";

mod local;
mod ollama;
mod recovery;

#[derive(Default)]
struct Gate {
    entered: AtomicBool,
    reached: Notify,
    released: Mutex<bool>,
    release: Condvar,
}

struct ReleaseGate(Arc<Gate>);
impl Drop for ReleaseGate {
    fn drop(&mut self) {
        self.0.open();
    }
}

impl Gate {
    fn signal(&self) {
        self.entered.store(true, Ordering::Release);
        self.reached.notify_one();
    }
    fn block(&self) {
        self.signal();
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.release.wait(released).unwrap();
        }
    }
    async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let reached = self.reached.notified();
                if self.entered.load(Ordering::Acquire) {
                    break;
                }
                reached.await;
            }
        })
        .await
        .expect("the selected job must reach the barrier");
    }
    fn open(&self) {
        *self.released.lock().unwrap() = true;
        self.release.notify_all();
    }
}

struct Reply {
    choice: &'static str,
    status: u16,
    gate: Option<Arc<Gate>>,
}

#[derive(Clone)]
struct TagsReply {
    wire: String,
    gate: Option<Arc<Gate>>,
}

impl TagsReply {
    fn body(status: u16, body: String) -> Self {
        Self {
            wire: format!(
                "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
            gate: None,
        }
    }
}

struct Server {
    address: std::net::SocketAddr,
    replies: Arc<Mutex<VecDeque<Reply>>>,
    sends: Arc<AtomicUsize>,
    tags_reply: Arc<Mutex<TagsReply>>,
    tags_reads: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let replies = Arc::new(Mutex::new(VecDeque::<Reply>::new()));
        let sends = Arc::new(AtomicUsize::new(0));
        let tags_reply = Arc::new(Mutex::new(TagsReply::body(
            200,
            json!({"models": [
                {"name": "synthetic-local-model:latest", "digest": "synthetic-digest"}
            ]})
            .to_string(),
        )));
        let tags_reads = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let (queue, count, stop) = (
            Arc::clone(&replies),
            Arc::clone(&sends),
            Arc::clone(&stopped),
        );
        let (metadata, metadata_count) = (Arc::clone(&tags_reply), Arc::clone(&tags_reads));
        let thread = std::thread::spawn(move || {
            for socket in listener.incoming() {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let mut socket = socket.unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                let (header_end, length) = loop {
                    let length = socket.read(&mut buffer).unwrap();
                    if length == 0 {
                        break (0, 0);
                    }
                    bytes.extend_from_slice(&buffer[..length]);
                    if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                    assert!(bytes.len() < 512 * 1024);
                };
                if header_end == 0 {
                    continue;
                }
                while bytes.len() < header_end + length {
                    let read = socket.read(&mut buffer).unwrap();
                    assert!(read > 0);
                    bytes.extend_from_slice(&buffer[..read]);
                }
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                if headers.starts_with("GET /api/tags HTTP/1.1\r\n") {
                    assert_eq!(length, 0);
                    metadata_count.fetch_add(1, Ordering::AcqRel);
                    let reply = metadata.lock().unwrap().clone();
                    if let Some(gate) = reply.gate {
                        gate.block();
                    }
                    let _ = socket.write_all(reply.wire.as_bytes());
                    continue;
                }
                count.fetch_add(1, Ordering::AcqRel);
                let reply = queue
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("every request needs an explicit reply");
                let body: Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                if let Some(gate) = reply.gate {
                    gate.block();
                }
                let response = if reply.status == 200 {
                    json!({"model":body["model"], "answers":{"verdict":{"type":"choice", "choice":reply.choice,
                        "probabilities":{"clear":if reply.choice == "clear" {0.99} else {0.01}, "concern":if reply.choice == "concern" {0.99} else {0.01}}, "confidence":0.99}},
                        "usage":{"input_tokens":12,"output_tokens":3}}).to_string()
                } else {
                    "{}".into()
                };
                let status = if reply.status == 200 {
                    "200 OK"
                } else {
                    "401 Unauthorized"
                };
                let _ = write!(
                    socket,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                );
            }
        });
        Self {
            address,
            replies,
            sends,
            tags_reply,
            tags_reads,
            stopped,
            thread: Some(thread),
        }
    }
    fn connection(&self, model: &str) -> SystemOneConnection {
        SystemOneConnection {
            provider: SystemOneProvider::Custom,
            endpoint: SystemOneEndpoint::ExactUrl(format!("http://{}/systemone", self.address)),
            model: model.into(),
            model_revision: None,
            response_mode: SystemOneResponseMode::Direct,
            credential: None,
            revision: 1,
            context_override: Some(ContextLimitOverride {
                total_input_tokens: Some(32_768),
                state_and_longest_question_tokens: Some(16_384),
                runtime_context_tokens: None,
            }),
        }
    }
    fn reply(&self, choice: &'static str, gate: Option<Arc<Gate>>) {
        self.replies.lock().unwrap().push_back(Reply {
            choice,
            status: 200,
            gate,
        });
    }
    fn count(&self) -> usize {
        self.sends.load(Ordering::Acquire)
    }
    fn tags_count(&self) -> usize {
        self.tags_reads.load(Ordering::Acquire)
    }
    fn set_tags_reply(&self, reply: TagsReply) {
        *self.tags_reply.lock().unwrap() = reply;
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

struct Runtime {
    store: Arc<Store>,
    worker: Arc<WorkerHandle>,
    installed: Arc<InstalledChecks>,
    barrier: Mutex<Option<(Stage, Arc<Gate>)>>,
    discoveries: AtomicUsize,
    shared_discovery: AtomicBool,
    fail_resolution: AtomicBool,
    unsupported_resolution: AtomicBool,
    prohibit_model_work: AtomicBool,
    clock: AtomicU64,
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
    fn now_unix_ms(&self) -> Result<u64> {
        Ok(self.clock.load(Ordering::Acquire))
    }
    fn resolve<'a>(&'a self, snapshot: &'a ExecutionSnapshot) -> CapabilityFuture<'a> {
        assert!(
            !self.prohibit_model_work.load(Ordering::Acquire),
            "local work must not resolve a model"
        );
        Box::pin(async move {
            self.discoveries.fetch_add(1, Ordering::AcqRel);
            ensure!(
                !self.fail_resolution.load(Ordering::Acquire),
                "Synthetic discovery failure"
            );
            if self.shared_discovery.load(Ordering::Acquire) {
                return resolve_capabilities(snapshot).await;
            }
            // The loopback fixture implements this explicit synthetic System One contract.
            assert!(matches!(
                snapshot.connection.provider,
                SystemOneProvider::Custom | SystemOneProvider::Ollama
            ));
            let mut capabilities = ModelCapabilities::jev_default();
            capabilities.model.clone_from(&snapshot.connection.model);
            if snapshot.connection.provider == SystemOneProvider::Ollama {
                capabilities.model_revision = Some("synthetic-digest".into());
            }
            if self.unsupported_resolution.load(Ordering::Acquire) {
                capabilities.output.choice = None;
            }
            Ok(snapshot.connection.apply_capability_overrides(capabilities))
        })
    }
    fn stage(&self, stage: Stage) {
        assert!(
            !self.prohibit_model_work.load(Ordering::Acquire),
            "local work must not construct, prepare, or use a model"
        );
        let barrier = self
            .barrier
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(expected, _)| *expected == stage)
            .map(|(_, gate)| Arc::clone(gate));
        if let Some(gate) = barrier {
            gate.block();
        }
    }
    fn preparation_recorded(&self, _: &antiburn_checks::fit::PreparationRecord) {
        assert!(
            !self.prohibit_model_work.load(Ordering::Acquire),
            "local work must not emit model preparation analytics"
        );
    }
}

struct Harness {
    fixture: Fixture,
    runtime: Arc<Runtime>,
    host: InstalledCheckHost<InstalledDecisionEvaluator>,
}

impl Harness {
    fn new(a: &Server, b: &Server) -> Self {
        let mut fixture = Fixture::new();
        fixture.store = Arc::new(Store::open(fixture.directory.path()).unwrap());
        fixture
            .store
            .replace_repositories(&[repository(&fixture.project)])
            .unwrap();
        fixture.index("native", "main", 0);
        fixture.publish("main", 0);
        Self::with_profiles(fixture, a, b)
    }

    fn in_memory(a: &Server, b: &Server) -> Self {
        Self::with_profiles(Fixture::new(), a, b)
    }

    fn with_profiles(fixture: Fixture, a: &Server, b: &Server) -> Self {
        std::fs::write(fixture.global.join("failed-retry-loop.yml"), SOURCE).unwrap();
        headless::setup_keyless_profiles(
            &fixture.store,
            BTreeMap::from([
                ("a".into(), a.connection("model-a")),
                ("b".into(), b.connection("model-b")),
            ]),
            "a",
            true,
        )
        .unwrap();
        let worker = Arc::new(WorkerHandle::default());
        headless::restore_keyless_profile(&fixture.store, &worker).unwrap();
        Self::with_worker(fixture, worker)
    }

    fn with_worker(fixture: Fixture, worker: Arc<WorkerHandle>) -> Self {
        let prohibit_model_work = !worker.is_available();
        let runtime = Arc::new(Runtime {
            store: Arc::clone(&fixture.store),
            worker,
            installed: Arc::new(InstalledChecks::default()),
            barrier: Mutex::new(None),
            discoveries: AtomicUsize::new(0),
            shared_discovery: AtomicBool::new(false),
            fail_resolution: AtomicBool::new(false),
            unsupported_resolution: AtomicBool::new(false),
            prohibit_model_work: AtomicBool::new(prohibit_model_work),
            clock: AtomicU64::new(
                u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
                    .unwrap(),
            ),
        });
        let selected: Arc<dyn SelectedJobRuntime> = runtime.clone();
        runtime
            .installed
            .credentials_restored
            .store(true, Ordering::Release);
        let mut host = InstalledCheckHost::new(
            Arc::clone(&fixture.store),
            Arc::new(InstalledDecisionEvaluator::factory(selected)),
            Some(fixture.global.clone()),
        )
        .unwrap();
        host.reconcile(&admission_for_worker(&runtime.worker))
            .unwrap();
        publish_metadata(&runtime.installed, &host.definitions);
        Self {
            fixture,
            runtime,
            host,
        }
    }
    fn publish(&mut self, count: usize) {
        self.runtime.clock.fetch_add(2000, Ordering::AcqRel);
        self.fixture.publish("main", count);
        for _ in 0..16 {
            if !self.host.catch_up().unwrap().1 {
                return;
            }
        }
        panic!("the synthetic source must fit the bounded catch-up budget");
    }
    fn job(&mut self) -> tokio::task::JoinHandle<Result<Option<RunSettlement>, StoreError>> {
        let job = self.host.evaluation_jobs(1).pop().unwrap();
        let runtime: Arc<dyn SelectedJobRuntime> = self.runtime.clone();
        tokio::spawn(job.run_selected(runtime))
    }
    fn select(&self, id: &str) {
        assert!(
            headless::switch_keyless_profile(&self.fixture.store, &self.runtime.worker, id)
                .unwrap()
        );
    }
    fn status(&self) -> String {
        self.fixture
            .store
            .lock()
            .query_row(
                "SELECT assessment_id FROM burn_check_current_status",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }
    fn protected(&self) -> Vec<String> {
        protected(&self.fixture.store)
    }
    fn reopen(self, ready: bool) -> Self {
        let Self {
            mut fixture,
            runtime,
            host,
        } = self;
        let clock = runtime.clock.load(Ordering::Acquire);
        let prohibit_model_work = runtime.prohibit_model_work.load(Ordering::Acquire);
        drop(host);
        drop(runtime);
        let directory = fixture.directory.path().to_path_buf();
        drop(fixture.store);
        fixture.store = Arc::new(Store::open(&directory).unwrap());
        let worker = Arc::new(WorkerHandle::default());
        let restored = headless::restore_keyless_profile(&fixture.store, &worker);
        if ready {
            restored.unwrap();
        } else {
            assert!(restored.is_err() || !worker.is_available());
        }
        let runtime = Arc::new(Runtime {
            store: Arc::clone(&fixture.store),
            worker,
            installed: Arc::new(InstalledChecks::default()),
            barrier: Mutex::new(None),
            discoveries: AtomicUsize::new(0),
            shared_discovery: AtomicBool::new(false),
            fail_resolution: AtomicBool::new(false),
            unsupported_resolution: AtomicBool::new(false),
            prohibit_model_work: AtomicBool::new(prohibit_model_work),
            clock: AtomicU64::new(clock),
        });
        let selected: Arc<dyn SelectedJobRuntime> = runtime.clone();
        runtime
            .installed
            .credentials_restored
            .store(true, Ordering::Release);
        let mut host = InstalledCheckHost::new(
            Arc::clone(&fixture.store),
            Arc::new(InstalledDecisionEvaluator::factory(selected)),
            Some(fixture.global.clone()),
        )
        .unwrap();
        assert!(runtime.installed.metadata.read().unwrap().is_empty());
        host.reconcile(&if ready {
            Admission::Ready
        } else {
            Admission::ProviderNotConfigured
        })
        .unwrap();
        publish_metadata(&runtime.installed, &host.definitions);
        Self {
            fixture,
            runtime,
            host,
        }
    }
    async fn baseline(&mut self, a: &Server) {
        a.reply("concern", None);
        self.publish(1);
        assert_eq!(
            self.job().await.unwrap().unwrap().unwrap().outcome,
            RunOutcome::Accepted
        );
    }
}

fn protected(store: &Store) -> Vec<String> {
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
            .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
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

#[derive(Clone, Copy, Debug)]
enum SwitchStage {
    Preparation,
    Admission,
    Sending,
    Receipt,
    Persistence(StoreStage),
    Deferred,
}

struct AdmissionBlockers {
    servers: Vec<Server>,
    gates: Vec<Arc<Gate>>,
    tasks: Vec<
        tokio::task::JoinHandle<
            Result<antiburn_checks::results::DecisionResult, EvaluationFailure>,
        >,
    >,
}

impl AdmissionBlockers {
    async fn start(harness: &Harness) -> Self {
        let project = harness.host.projects.values().next().unwrap();
        let active = project.active[0].clone();
        let candidate = project
            .store
            .load_recovery(None, 128)
            .unwrap()
            .into_iter()
            .find_map(|record| match record.work {
                RecoveryWork::Pending { candidate, .. } => Some(candidate),
                _ => None,
            })
            .unwrap();
        let mut blockers = Self {
            servers: Vec::new(),
            gates: Vec::new(),
            tasks: Vec::new(),
        };
        // Each received loopback request holds one real shared inference permit.
        for index in 0..antiburn_local::analysis::jev::MAX_PARALLEL_REQUESTS {
            let server = Server::new();
            let gate = Arc::new(Gate::default());
            server.reply("concern", Some(Arc::clone(&gate)));
            let connection = server.connection("admission-blocker");
            let mut capabilities = ModelCapabilities::jev_default();
            capabilities.model.clone_from(&connection.model);
            let evaluator =
                TypeSafeDecisionEvaluator::from_selected_snapshot(connection, None, capabilities)
                    .unwrap();
            let prepared = evaluator
                .prepare(
                    &active,
                    vec![DecisionInput {
                        decision: "review".into(),
                        input: json!({"activity":"synthetic admission occupant"}),
                    }],
                    EvaluationBudget::default(),
                )
                .unwrap();
            let active = active.clone();
            let mut candidate = candidate.clone();
            candidate.fingerprint = [128 + index as u8; 32];
            blockers.tasks.push(tokio::spawn(async move {
                evaluator
                    .evaluate_prepared(
                        &active,
                        &candidate,
                        &prepared,
                        &DecisionAttempt::new("review", 1).unwrap(),
                    )
                    .await
            }));
            blockers.servers.push(server);
            blockers.gates.push(Arc::clone(&gate));
            gate.wait().await;
        }
        blockers
    }
    async fn finish(mut self) {
        for gate in &self.gates {
            gate.open();
        }
        for task in self.tasks.drain(..) {
            task.await.unwrap().unwrap();
        }
    }
}

impl Drop for AdmissionBlockers {
    fn drop(&mut self) {
        for gate in &self.gates {
            gate.open();
        }
    }
}

fn pending_candidate(harness: &Harness) -> String {
    harness
        .fixture
        .store
        .lock()
        .query_row(
            "SELECT candidate_id FROM burn_check_candidate WHERE state = 'pending'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn queued_job(
    harness: &mut Harness,
    candidate: &str,
    gate: Arc<Gate>,
) -> tokio::task::JoinHandle<Result<Option<RunSettlement>, StoreError>> {
    let job = harness.host.evaluation_jobs(1).pop().unwrap();
    let runtime: Arc<dyn SelectedJobRuntime> = harness.runtime.clone();
    let fingerprint = std::array::from_fn(|index| {
        u8::from_str_radix(&candidate[index * 2..index * 2 + 2], 16).unwrap()
    });
    tokio::spawn(crate::jev::dispatch::test_hooks::with_request_slot_pending(
        antiburn_checks::decision::attempt_id(
            &fingerprint,
            &DecisionAttempt::new("review", 1).unwrap(),
        ),
        move || gate.signal(),
        job.run_selected(runtime),
    ))
}

fn publish_native_change(harness: &Harness, event: antiburn_local::analysis::NativeEvidenceFact) {
    use crate::store::native_tool_results::{evidence, publications::PublicationPolicy};
    use antiburn_local::analysis::{NativeEvidenceBatch, NativeEvidenceEvent};

    let mut connection = harness.fixture.store.lock();
    let tx = connection.transaction().unwrap();
    let generation: i64 = tx.query_row(
        "SELECT source_generation FROM session WHERE environment_key = 'native' AND agent = 'pi' AND session_id = 'main'",
        [], |row| row.get(0),
    ).unwrap();
    assert!(
        evidence::reconcile_evidence_in(
            &tx,
            &SessionKey::new("native", "pi", "main"),
            generation,
            0,
            &NativeEvidenceBatch {
                source_format: SourceFormat::PiV3Jsonl,
                events: vec![NativeEvidenceEvent {
                    live_eligible: true,
                    fact: event
                }],
                history_complete: true,
            },
            PublicationPolicy {
                suppress_coverage: true,
                ..Default::default()
            },
            i64::try_from(harness.runtime.clock.load(Ordering::Acquire)).unwrap(),
        )
        .unwrap()
    );
    tx.commit().unwrap();
}

fn assert_stale_attempt(harness: &Harness, candidate: &str, expected: &str) {
    let connection = harness.fixture.store.lock();
    let (state, outcome, valid, attempts, attempt_outcome, statuses): (String, String, bool, u64, String, u64) = connection.query_row(
        "SELECT c.state, h.outcome, h.evidence_valid,
            (SELECT COUNT(*) FROM burn_check_evaluation_attempt WHERE candidate_id = c.candidate_id),
            a.outcome, (SELECT COUNT(*) FROM burn_check_current_status)
         FROM burn_check_candidate c JOIN burn_check_assessment_history h USING(candidate_id)
         JOIN burn_check_evaluation_attempt a USING(candidate_id) WHERE c.candidate_id = ?1",
        [candidate], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
    ).unwrap();
    assert_eq!(
        (state.as_str(), outcome.as_str(), valid),
        ("stale", "stale", false)
    );
    assert_eq!(
        (attempts, attempt_outcome.as_str(), statuses),
        (1, expected, 0)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_idle_activity_cancels_queued_requests_and_keeps_sent_delivery_unknown() {
    use crate::store::native_tool_results::observers;
    use antiburn_local::analysis::{
        NativeActivityObservation, NativeEvidenceFact, ToolResultPosition,
    };

    for already_sent in [false, true] {
        let (a, b) = (Server::new(), Server::new());
        let mut harness = Harness::in_memory(&a, &b);
        let source = SOURCE
            .replace("  tool_result:", "  session_idle:")
            .replace("message.tool.result", "session.idle_seconds");
        std::fs::write(harness.fixture.global.join("failed-retry-loop.yml"), source).unwrap();
        harness.host.reconcile(&Admission::Ready).unwrap();
        publish_metadata(&harness.runtime.installed, &harness.host.definitions);
        harness.host.catch_up().unwrap();
        let deadline = harness
            .fixture
            .store
            .next_session_idle_deadline()
            .unwrap()
            .unwrap();
        {
            let mut connection = harness.fixture.store.lock();
            let tx = connection.transaction().unwrap();
            observers::frontier_in(
                &tx,
                &SessionKey::new("native", "pi", "main"),
                "{}",
                true,
                deadline,
            )
            .unwrap();
            tx.commit().unwrap();
        }
        harness
            .runtime
            .clock
            .store(u64::try_from(deadline).unwrap(), Ordering::Release);
        harness.host.catch_up().unwrap();
        let candidate = pending_candidate(&harness);
        let generation = harness
            .runtime
            .worker
            .execution_snapshot()
            .unwrap()
            .generation;
        let gate = Arc::new(Gate::default());
        let _release = ReleaseGate(Arc::clone(&gate));
        let blockers = if already_sent {
            None
        } else {
            Some(AdmissionBlockers::start(&harness).await)
        };
        a.reply("clear", already_sent.then(|| Arc::clone(&gate)));
        let task = if already_sent {
            harness.job()
        } else {
            queued_job(&mut harness, &candidate, Arc::clone(&gate))
        };
        gate.wait().await;
        let (frozen, prepared, revision): (String, String, u64) = harness.fixture.store.lock().query_row(
            "SELECT frozen_input_json, preparation_json,
                (SELECT activity_revision FROM burn_check_session_attachment WHERE observer_epoch IS NOT NULL)
             FROM burn_check_candidate WHERE candidate_id = ?1 AND state = 'dispatching'",
            [&candidate], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert!(!task.is_finished());
        harness.runtime.clock.fetch_add(1000, Ordering::AcqRel);
        publish_native_change(
            &harness,
            NativeEvidenceFact::Activity(NativeActivityObservation {
                source_format: SourceFormat::PiV3Jsonl,
                session_id: "main".into(),
                position: ToolResultPosition::Jsonl {
                    record_index: 10,
                    item_index: 0,
                },
                token: "new-native-work".into(),
                native_at_ms: Some(deadline + 1000),
            }),
        );
        let changed: (String, u64) = harness.fixture.store.lock().query_row(
            "SELECT state, (SELECT activity_revision FROM burn_check_session_attachment WHERE observer_epoch IS NOT NULL)
             FROM burn_check_candidate WHERE candidate_id = ?1",
            [&candidate], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(changed, ("stale".into(), revision + 1));
        assert!(harness.runtime.worker.key_is_current(generation));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect(
                    "source cancellation must finish while permits or the response remain blocked"
                )
                .unwrap()
                .unwrap()
                .unwrap()
                .outcome,
            RunOutcome::Stale
        );
        gate.open();
        if let Some(blockers) = blockers {
            blockers.finish().await;
        }
        assert_stale_attempt(
            &harness,
            &candidate,
            if already_sent { "unknown" } else { "permanent" },
        );
        let retained: (String, String) = harness.fixture.store.lock().query_row(
            "SELECT frozen_input_json, preparation_json FROM burn_check_candidate WHERE candidate_id = ?1",
            [&candidate], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(retained, (frozen, prepared));
        let usage = harness.fixture.store.burn_check_usage_summary().unwrap();
        assert_eq!(
            (usage.confirmed_calls, usage.unknown_outcomes),
            (0, u64::from(already_sent))
        );
        harness.host.catch_up().unwrap();
        assert!(harness.job().await.unwrap().unwrap().is_none());
        assert_stale_attempt(
            &harness,
            &candidate,
            if already_sent { "unknown" } else { "permanent" },
        );
        assert_eq!((a.count(), b.count()), (usize::from(already_sent), 0));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_source_correction_cancels_a_queued_request_without_switching_connections() {
    for change in [
        "revision",
        "payload",
        "removal",
        "snapshot",
        "snapshot_removal",
    ] {
        let (a, b) = (Server::new(), Server::new());
        let mut harness = Harness::in_memory(&a, &b);
        harness.publish(1);
        let candidate = pending_candidate(&harness);
        let blockers = AdmissionBlockers::start(&harness).await;
        let gate = Arc::new(Gate::default());
        a.reply("clear", None);
        let task = queued_job(&mut harness, &candidate, Arc::clone(&gate));
        gate.wait().await;
        let generation = harness
            .runtime
            .worker
            .execution_snapshot()
            .unwrap()
            .generation;
        let before: (String, String, i64) = harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT frozen_input_json, preparation_json,
                (SELECT applied_sequence FROM burn_check_activation)
             FROM burn_check_candidate WHERE candidate_id = ?1",
                [&candidate],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let untyped: i64 = harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_candidate_dependency
             WHERE candidate_id = ?1 AND dependency_binding_json IS NULL
               AND event_kind = 'tool_result' AND native_revision IS NOT NULL",
                [&candidate],
                |row| row.get(0),
            )
            .unwrap();
        assert!(untyped > 0);
        publish_tool_result_correction(&harness, change);
        assert_eq!(
            harness
                .fixture
                .store
                .lock()
                .query_row(
                    "SELECT state FROM burn_check_candidate WHERE candidate_id = ?1",
                    [&candidate],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "stale",
            "{change}"
        );
        assert!(harness.runtime.worker.key_is_current(generation));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect("native correction must cancel while inference permits remain held")
                .unwrap()
                .unwrap()
                .unwrap()
                .outcome,
            RunOutcome::Stale,
            "{change}"
        );
        assert_stale_attempt(&harness, &candidate, "permanent");
        let retained: (String, String, i64) = harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT frozen_input_json, preparation_json,
                (SELECT applied_sequence FROM burn_check_activation)
             FROM burn_check_candidate WHERE candidate_id = ?1",
                [&candidate],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(retained, before);
        blockers.finish().await;
        harness.host.catch_up().unwrap();
        assert!(harness.job().await.unwrap().unwrap().is_none());
        assert_stale_attempt(&harness, &candidate, "permanent");
        assert_eq!((a.count(), b.count()), (0, 0));
        let usage = harness.fixture.store.burn_check_usage_summary().unwrap();
        assert_eq!((usage.confirmed_calls, usage.unknown_outcomes), (0, 0));
    }
}

fn publish_tool_result_correction(harness: &Harness, change: &str) {
    use antiburn_local::analysis::{
        NativeEvidenceFact, NativeMessageObservation, ToolResultChange, ToolResultObservation,
    };

    let payload: String = harness.fixture.store.lock().query_row(
        "SELECT payload_json FROM native_tool_result_current WHERE event_kind = 'tool_result' AND is_removed = 0",
        [], |row| row.get(0),
    ).unwrap();
    let mut corrected: ToolResultObservation = serde_json::from_str(&payload).unwrap();
    if change != "payload" {
        corrected.revision += 1;
    }
    corrected.content = Some("Corrected synthetic result: parser tests passed.".into());
    if change == "removal" {
        corrected.change = ToolResultChange::Removal;
    }
    if matches!(change, "snapshot" | "snapshot_removal") {
        use crate::store::native_tool_results::publications::{
            PublicationPolicy, reconcile_with_policy,
        };

        let mut connection = harness.fixture.store.lock();
        let tx = connection.transaction().unwrap();
        let generation: i64 = tx.query_row(
            "SELECT source_generation FROM session WHERE environment_key = 'native' AND agent = 'pi' AND session_id = 'main'",
            [], |row| row.get(0),
        ).unwrap();
        reconcile_with_policy(
            &tx,
            &SessionKey::new("native", "pi", "main"),
            generation,
            0,
            &ToolResultBatch {
                source_format: SourceFormat::PiV3Jsonl,
                results: if change == "snapshot_removal" {
                    Vec::new()
                } else {
                    vec![corrected]
                },
                complete: true,
            },
            PublicationPolicy {
                suppress_coverage: true,
                ..Default::default()
            },
        )
        .unwrap();
        tx.commit().unwrap();
    } else {
        publish_native_change(
            harness,
            NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(corrected)),
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_source_correction_keeps_sent_delivery_unknown_without_resend() {
    let (a, b) = (Server::new(), Server::new());
    let mut harness = Harness::in_memory(&a, &b);
    harness.publish(1);
    let candidate = pending_candidate(&harness);
    let gate = Arc::new(Gate::default());
    let _release = ReleaseGate(Arc::clone(&gate));
    a.reply("clear", Some(Arc::clone(&gate)));
    let task = harness.job();
    gate.wait().await;
    let generation = harness
        .runtime
        .worker
        .execution_snapshot()
        .unwrap()
        .generation;
    publish_tool_result_correction(&harness, "payload");
    assert_eq!(
        harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT state FROM burn_check_candidate WHERE candidate_id = ?1",
                [&candidate],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "stale"
    );
    assert!(harness.runtime.worker.key_is_current(generation));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("native correction must cancel before the response arrives")
            .unwrap()
            .unwrap()
            .unwrap()
            .outcome,
        RunOutcome::Stale
    );
    gate.open();
    assert_stale_attempt(&harness, &candidate, "unknown");
    harness.host.catch_up().unwrap();
    assert!(harness.job().await.unwrap().unwrap().is_none());
    assert_stale_attempt(&harness, &candidate, "unknown");
    assert_eq!((a.count(), b.count()), (1, 0));
    let usage = harness.fixture.store.burn_check_usage_summary().unwrap();
    assert_eq!((usage.confirmed_calls, usage.unknown_outcomes), (0, 1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn selected_job_switches_at_all_six_stages_without_reviving_old_work() {
    for stage in [
        SwitchStage::Preparation,
        SwitchStage::Admission,
        SwitchStage::Sending,
        SwitchStage::Receipt,
        SwitchStage::Persistence(StoreStage::BeforeAnswer),
        SwitchStage::Persistence(StoreStage::AfterAnswer),
        SwitchStage::Persistence(StoreStage::BeforeAcceptance),
        SwitchStage::Deferred,
    ] {
        let (a, b) = (Server::new(), Server::new());
        let mut harness = Harness::new(&a, &b);
        harness.baseline(&a).await;
        let prior = harness.status();
        let gate = Arc::new(Gate::default());
        let _release = ReleaseGate(Arc::clone(&gate));
        harness.publish(2);
        let candidate: String = harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT candidate_id FROM burn_check_candidate WHERE state = 'pending' ORDER BY trigger_position DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let runtime_stage = match stage {
            SwitchStage::Preparation => Some(Stage::Preparation),
            SwitchStage::Receipt => Some(Stage::ResponseReceipt),
            _ => None,
        };
        if let Some(runtime_stage) = runtime_stage {
            *harness.runtime.barrier.lock().unwrap() = Some((runtime_stage, Arc::clone(&gate)));
        }
        if let SwitchStage::Persistence(expected) = stage {
            let gate = Arc::clone(&gate);
            harness
                .fixture
                .store
                .set_burn_check_test_hook(Some(Arc::new(move |stage| {
                    if stage == expected {
                        gate.block();
                    }
                })));
        }
        let admission = if matches!(stage, SwitchStage::Admission) {
            Some(AdmissionBlockers::start(&harness).await)
        } else {
            None
        };
        if !matches!(stage, SwitchStage::Preparation | SwitchStage::Admission) {
            a.reply(
                "clear",
                matches!(stage, SwitchStage::Sending | SwitchStage::Deferred)
                    .then(|| Arc::clone(&gate)),
            );
        }
        let admission_pending = Arc::new(AtomicUsize::new(0));
        let task = if matches!(stage, SwitchStage::Admission) {
            let job = harness.host.evaluation_jobs(1).pop().unwrap();
            let runtime: Arc<dyn SelectedJobRuntime> = harness.runtime.clone();
            let gate = Arc::clone(&gate);
            let pending = Arc::clone(&admission_pending);
            tokio::spawn(crate::jev::dispatch::test_hooks::with_request_slot_pending(
                antiburn_checks::decision::attempt_id(
                    &std::array::from_fn(|index| {
                        u8::from_str_radix(&candidate[index * 2..index * 2 + 2], 16).unwrap()
                    }),
                    &DecisionAttempt::new("review", 1).unwrap(),
                ),
                move || {
                    pending.fetch_add(1, Ordering::AcqRel);
                    gate.signal();
                },
                job.run_selected(runtime),
            ))
        } else {
            harness.job()
        };
        gate.wait().await;
        if let Some(admission) = &admission {
            assert_eq!(admission_pending.load(Ordering::Acquire), 1);
            assert!(!task.is_finished());
            assert!(admission.servers.iter().all(|server| server.count() == 1));
            assert!(
                admission
                    .gates
                    .iter()
                    .all(|gate| !*gate.released.lock().unwrap())
            );
        }
        let protected = harness.protected();
        if matches!(stage, SwitchStage::Deferred) {
            harness.fixture.publish("main", 3);
            gate.open();
            assert_eq!(
                task.await.unwrap().unwrap().unwrap().outcome,
                RunOutcome::Deferred
            );
            assert_eq!(harness.status(), prior);
            let protected = harness.protected();
            harness.select("b");
            assert_eq!(harness.protected(), protected);
            harness.host.catch_up().unwrap();
        } else {
            harness.select("b");
            if !matches!(stage, SwitchStage::Sending) {
                gate.open();
            }
            let settlement = task.await.unwrap().unwrap();
            gate.open();
            if matches!(stage, SwitchStage::Preparation) {
                assert!(settlement.is_none());
            } else {
                assert_eq!(settlement.unwrap().outcome, RunOutcome::Stale, "{stage:?}");
            }
            if let Some(admission) = admission {
                admission.finish().await;
                assert_eq!(admission_pending.load(Ordering::Acquire), 1);
            }
            assert_eq!(harness.protected(), protected, "{stage:?}");
        }
        harness.fixture.store.set_burn_check_test_hook(None);
        *harness.runtime.barrier.lock().unwrap() = None;
        assert_eq!(harness.status(), prior, "{stage:?}");
        assert!(harness.runtime.worker.is_available());
        assert!(!harness.runtime.worker.authentication_rejected());
        let old_sends = if matches!(stage, SwitchStage::Preparation | SwitchStage::Admission) {
            1
        } else {
            2
        };
        assert_eq!(a.count(), old_sends, "{stage:?}");
        let (state, outcome, evidence_valid): (String, String, bool) = harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT c.state, h.outcome, h.evidence_valid FROM burn_check_candidate c
             JOIN burn_check_assessment_history h USING(candidate_id) WHERE c.candidate_id = ?1",
                [&candidate],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (state.as_str(), outcome.as_str(), evidence_valid),
            ("cancelled", "cancelled", false),
            "{stage:?}"
        );
        let attempts = |store: &Store| {
            store.lock().prepare("SELECT attempt_number, state, outcome FROM burn_check_evaluation_attempt WHERE candidate_id = ?1 ORDER BY attempt_number").unwrap()
                .query_map([&candidate], |row| Ok((row.get::<_, u8>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))
                .unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };
        let expected_attempts = match stage {
            SwitchStage::Preparation => Vec::new(),
            SwitchStage::Admission => vec![(1, "settled".into(), "permanent".into())],
            SwitchStage::Sending => vec![(1, "settled".into(), "unknown".into())],
            _ => vec![(1, "settled".into(), "answer".into())],
        };
        assert_eq!(
            attempts(&harness.fixture.store),
            expected_attempts,
            "{stage:?}"
        );
        // Current B activity must run before returning to A.
        b.reply("concern", None);
        harness.publish(4);
        assert_eq!(
            harness.job().await.unwrap().unwrap().unwrap().outcome,
            RunOutcome::Accepted,
            "{stage:?}"
        );
        assert_eq!(b.count(), 1);
        let retained = harness.status();
        harness.select("a");
        let expected = harness.protected();
        let summary = harness.fixture.store.burn_check_usage_summary().unwrap();
        let known = if matches!(
            stage,
            SwitchStage::Preparation | SwitchStage::Admission | SwitchStage::Sending
        ) {
            2
        } else {
            3
        };
        assert_eq!(summary.confirmed_calls, known, "{stage:?}");
        assert_eq!(
            summary.unknown_outcomes,
            u64::from(matches!(stage, SwitchStage::Sending)),
            "{stage:?}"
        );
        assert_eq!(summary.input_tokens, 12 * known);
        assert_eq!(summary.output_tokens, 3 * known);
        assert_eq!(summary.estimated_usd, None);
        let ledger: Value = serde_json::from_str(
            &harness
                .fixture
                .store
                .internal_value("internal:burnCheckUsageLedgerV1")
                .unwrap(),
        )
        .unwrap();
        let aggregates = ledger["summary"]["aggregates"].as_array().unwrap();
        assert!(
            aggregates
                .iter()
                .all(|aggregate| aggregate["provider"] == "custom-systemone")
        );
        assert_eq!(aggregates.len(), 2);
        for (model, count) in [("model-a", known - 1), ("model-b", 1)] {
            let aggregate = aggregates
                .iter()
                .find(|aggregate| aggregate["model"] == model)
                .unwrap();
            assert_eq!(aggregate["confirmed_calls"], count, "{stage:?}: {model}");
        }
        let mut reopened = harness.reopen(true);
        assert_eq!(
            reopened
                .runtime
                .worker
                .execution_snapshot()
                .unwrap()
                .connection
                .model,
            "model-a"
        );
        reopened.host.catch_up().unwrap();
        assert_eq!(reopened.status(), retained);
        assert_eq!(reopened.protected(), expected);
        assert!(reopened.job().await.unwrap().unwrap().is_none());
        assert_eq!(
            attempts(&reopened.fixture.store),
            expected_attempts,
            "{stage:?}"
        );
        assert_eq!(
            reopened
                .fixture
                .store
                .burn_check_usage_summary()
                .unwrap()
                .confirmed_calls,
            summary.confirmed_calls
        );
        assert_eq!(reopened.runtime.discoveries.load(Ordering::Acquire), 0);
        assert_eq!(a.count(), old_sends);
        assert_eq!(b.count(), 1);
        a.reply("concern", None);
        reopened.publish(5);
        assert_eq!(
            reopened.job().await.unwrap().unwrap().unwrap().outcome,
            RunOutcome::Accepted
        );
        assert_eq!(a.count(), old_sends + 1);
        assert_eq!(
            attempts(&reopened.fixture.store),
            expected_attempts,
            "{stage:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switching_a_b_a_after_the_first_saved_block_never_sends_the_second() {
    let (a, b) = (Server::new(), Server::new());
    let mut harness = Harness::new(&a, &b);
    let source = SOURCE.replace("actions:", "  second:\n    input: {activity: '${{ message.tool.result }}'}\n    output:\n      verdict:\n        type: choice\n        question: Is the second assessment clear?\n        criteria: {clear: Clear., concern: Concern.}\nactions:");
    std::fs::write(harness.fixture.global.join("failed-retry-loop.yml"), source).unwrap();
    harness.host.reconcile(&Admission::Ready).unwrap();
    publish_metadata(&harness.runtime.installed, &harness.host.definitions);
    let gate = Arc::new(Gate::default());
    let _release = ReleaseGate(gate.clone());
    let callback = gate.clone();
    harness
        .fixture
        .store
        .set_burn_check_test_hook(Some(Arc::new(move |stage| {
            if stage == StoreStage::AfterAnswer {
                callback.block();
            }
        })));
    a.reply("clear", None);
    harness.publish(1);
    let task = harness.job();
    gate.wait().await;
    let saved: (String, String) = harness
        .fixture
        .store
        .lock()
        .query_row(
            "SELECT decision_name, outcome FROM burn_check_evaluation_attempt",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(saved, ("review".into(), "answer".into()));
    harness.select("b");
    harness.select("a");
    gate.open();
    assert_eq!(
        task.await.unwrap().unwrap().unwrap().outcome,
        RunOutcome::Stale
    );
    harness.fixture.store.set_burn_check_test_hook(None);
    assert_eq!((a.count(), b.count()), (1, 0));
    let connection = harness.fixture.store.lock();
    let second: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM burn_check_evaluation_attempt WHERE decision_name = 'second'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let accepted: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM burn_check_assessment_history WHERE outcome = 'assessed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!((second, accepted), (0, 0));
}

#[tokio::test]
async fn publication_validation_uses_the_factory_without_discovery_clients_or_cached_capabilities()
{
    let (a, b) = (Server::new(), Server::new());
    let harness = Harness::new(&a, &b);
    let active = harness.host.projects.values().next().unwrap().active[0].clone();
    let runtime: Arc<dyn SelectedJobRuntime> = harness.runtime.clone();
    let selected = InstalledDecisionEvaluator::for_assessment(runtime.clone(), &active)
        .await
        .unwrap()
        .unwrap();
    let prepared = selected
        .prepare(
            &active,
            vec![DecisionInput {
                decision: "review".into(),
                input: json!({"activity":"synthetic"}),
            }],
            EvaluationBudget::default(),
        )
        .unwrap();
    let discoveries = harness.runtime.discoveries.load(Ordering::Acquire);
    harness
        .runtime
        .prohibit_model_work
        .store(true, Ordering::Release);
    harness
        .runtime
        .fail_resolution
        .store(true, Ordering::Release);
    *harness.runtime.installed.capabilities.write().unwrap() =
        Some(super::super::execution::CapabilityState {
            generation: harness
                .runtime
                .worker
                .execution_snapshot()
                .unwrap()
                .generation,
            result: Err("synthetic cached discovery failure".into()),
            retry_at: Instant::now() + DISCOVERY_INTERVAL,
        });
    let factory = InstalledDecisionEvaluator::factory(runtime);
    factory
        .validate_prepared(
            &active,
            &prepared,
            PreparationValidationPurpose::Publication,
        )
        .unwrap();
    assert!(
        factory
            .validate_prepared(&active, &prepared, PreparationValidationPurpose::Dispatch)
            .is_err()
    );
    let mut invalid = prepared.clone();
    invalid.profile = "unsupported profile".into();
    assert!(
        factory
            .validate_prepared(&active, &invalid, PreparationValidationPurpose::Publication)
            .is_err()
    );
    assert_eq!(
        harness.runtime.discoveries.load(Ordering::Acquire),
        discoveries
    );
    assert_eq!((a.count(), b.count()), (0, 0));
    harness
        .runtime
        .installed
        .stopping
        .store(true, Ordering::Release);
    assert!(
        factory
            .validate_prepared(
                &active,
                &prepared,
                PreparationValidationPurpose::Publication
            )
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_and_failed_transitions_block_real_answer_recovery_after_reopen() {
    for marker in [
        "internal:smartChecksConnectionChangePendingV1",
        "internal:typesafeCredentialChangePendingV1",
        "internal:smartChecksCredentialRemovalPendingV1",
        "master-pause-failure",
        "switch-cancellation-failure",
    ] {
        let (a, b) = (Server::new(), Server::new());
        let mut harness = Harness::new(&a, &b);
        harness.baseline(&a).await;
        let prior = harness.status();
        let gate = Arc::new(Gate::default());
        let _release = ReleaseGate(Arc::clone(&gate));
        harness.publish(2);
        a.reply("clear", Some(Arc::clone(&gate)));
        let task = harness.job();
        gate.wait().await;
        harness.fixture.publish("main", 3);
        gate.open();
        assert_eq!(
            task.await.unwrap().unwrap().unwrap().outcome,
            RunOutcome::Deferred
        );
        harness
            .runtime
            .prohibit_model_work
            .store(true, Ordering::Release);
        let (candidate, saved_answer, saved_attempt): (String, String, String) = harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT c.candidate_id, c.answer_json, a.attempt_id
                 FROM burn_check_candidate c JOIN burn_check_evaluation_attempt a USING(candidate_id)
                 WHERE c.state = 'answered' AND a.decision_name = 'review' AND a.attempt_number = 1
                   AND a.state = 'settled' AND a.outcome = 'answer'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let usage = harness.fixture.store.burn_check_usage_summary().unwrap();
        let watermark: u64 = harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT position FROM burn_check_acceptance_watermark",
                [],
                |row| row.get(0),
            )
            .unwrap();
        if marker.ends_with("failure") {
            harness.fixture.store.lock().execute_batch("CREATE TRIGGER reject_cancel BEFORE UPDATE ON burn_check_candidate WHEN NEW.state = 'cancelled' BEGIN SELECT RAISE(ABORT, 'injected cancellation failure'); END").unwrap();
            let result = if marker == "master-pause-failure" {
                headless::pause_model_checks(&harness.fixture.store, &harness.runtime.worker)
                    .map(|()| false)
            } else {
                headless::switch_keyless_profile(
                    &harness.fixture.store,
                    &harness.runtime.worker,
                    "b",
                )
            };
            assert!(result.is_err());
        } else {
            harness
                .fixture
                .store
                .set_internal_value_checked(
                    marker,
                    if marker.contains("typesafeCredential") {
                        "true"
                    } else {
                        "b"
                    },
                )
                .unwrap();
            harness.runtime.worker.suspend_system_one();
        }
        harness.host.catch_up().unwrap();
        assert_eq!(harness.status(), prior, "{marker}");
        if marker == "master-pause-failure" {
            let protected = harness.protected();
            harness.host.pause_disabled().unwrap();
            assert_eq!(harness.protected(), protected);
        }
        let expected = harness.protected();
        let mut harness = harness.reopen(false);
        harness.host.catch_up().unwrap();
        assert_eq!(harness.status(), prior, "{marker}");
        assert_eq!(harness.protected(), expected, "{marker}");
        let (state, outcome, evidence_valid, retained_answer, retained_attempt): (String, String, bool, String, String) = harness
            .fixture
            .store
            .lock()
            .query_row(
                "SELECT c.state, h.outcome, h.evidence_valid, c.answer_json, a.attempt_id
                 FROM burn_check_candidate c JOIN burn_check_assessment_history h USING(candidate_id)
                 JOIN burn_check_evaluation_attempt a USING(candidate_id)
                 WHERE c.candidate_id = ?1 AND h.answer_json = c.answer_json
                   AND a.decision_name = 'review' AND a.attempt_number = 1
                   AND a.state = 'settled' AND a.outcome = 'answer'",
                [&candidate],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            (state.as_str(), outcome.as_str(), evidence_valid),
            ("not_assessed", "not_assessed", false),
            "{marker}"
        );
        assert_eq!(
            (retained_answer, retained_attempt),
            (saved_answer, saved_attempt),
            "{marker}"
        );
        assert_eq!(
            harness.fixture.store.burn_check_usage_summary().unwrap(),
            usage,
            "{marker}"
        );
        let (attempts, retained_watermark): (u64, u64) = harness.fixture.store.lock().query_row(
            "SELECT (SELECT COUNT(*) FROM burn_check_evaluation_attempt), position FROM burn_check_acceptance_watermark",
            [], |row| Ok((row.get(0)?, row.get(1)?))
        ).unwrap();
        assert_eq!((attempts, retained_watermark), (2, watermark), "{marker}");
        assert!(!harness.runtime.worker.is_available());
        assert_eq!(a.count(), 2);
        assert_eq!(b.count(), 0);
        assert_eq!(harness.runtime.discoveries.load(Ordering::Acquire), 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_authentication_from_a_cannot_reject_selected_b() {
    let (a, b) = (Server::new(), Server::new());
    let mut harness = Harness::new(&a, &b);
    harness.baseline(&a).await;
    let gate = Arc::new(Gate::default());
    let _release = ReleaseGate(Arc::clone(&gate));
    *harness.runtime.barrier.lock().unwrap() = Some((Stage::Authentication, Arc::clone(&gate)));
    a.replies.lock().unwrap().push_back(Reply {
        choice: "clear",
        status: 401,
        gate: None,
    });
    harness.publish(2);
    let task = harness.job();
    gate.wait().await;
    harness.select("b");
    gate.open();
    assert_eq!(
        task.await.unwrap().unwrap().unwrap().outcome,
        RunOutcome::Stale
    );
    assert!(!harness.runtime.worker.authentication_rejected());
    assert!(harness.runtime.worker.is_available());
    *harness.runtime.barrier.lock().unwrap() = None;
    b.reply("concern", None);
    harness.publish(3);
    assert_eq!(
        harness.job().await.unwrap().unwrap().unwrap().outcome,
        RunOutcome::Accepted
    );
    assert_eq!((a.count(), b.count()), (2, 1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn execution_discovery_requires_due_work_and_never_blocks_master_pause() {
    let (a, b) = (Server::new(), Server::new());
    let mut harness = Harness::new(&a, &b);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let gate = Arc::new(Gate::default());
    let _release = ReleaseGate(Arc::clone(&gate));
    let count = Arc::new(AtomicUsize::new(0));
    let address = listener.local_addr().unwrap();
    let entered = Arc::clone(&gate);
    let calls = Arc::clone(&count);
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut bytes = [0; 4096];
        let read = socket.read(&mut bytes).unwrap();
        assert!(String::from_utf8_lossy(&bytes[..read]).starts_with("GET /api/version "));
        calls.fetch_add(1, Ordering::AcqRel);
        entered.block();
        let body = r#"{"version":"0.35.0"}"#;
        let _ = write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    });
    let mut connection = a.connection("model-a");
    connection.provider = SystemOneProvider::Ollama;
    connection.endpoint = SystemOneEndpoint::BaseUrl(format!("http://{address}"));
    harness
        .runtime
        .worker
        .set_system_one_connection(connection, None)
        .unwrap();
    harness
        .runtime
        .shared_discovery
        .store(true, Ordering::Release);
    for _ in 0..3 {
        let generation = harness
            .runtime
            .worker
            .execution_snapshot()
            .map(|snapshot| snapshot.generation);
        let (capabilities, capability_error) =
            execution::coordinator_capabilities(&harness.runtime.installed, generation);
        harness.host.capabilities = capabilities;
        harness.host.capability_error = capability_error;
        harness.host.reconcile(&Admission::Ready).unwrap();
        assert!(harness.job().await.unwrap().unwrap().is_none());
    }
    assert_eq!(harness.runtime.discoveries.load(Ordering::Acquire), 0);
    assert_eq!(count.load(Ordering::Acquire), 0);
    harness.publish(1);
    let job = harness.job();
    gate.wait().await;
    assert_eq!(count.load(Ordering::Acquire), 1);
    tokio::time::timeout(Duration::from_secs(1), async {
        headless::pause_model_checks(&harness.fixture.store, &harness.runtime.worker).unwrap();
        harness.host.pause_disabled().unwrap();
        harness.runtime.installed.complete_disable(1);
    })
    .await
    .expect("pausing must not wait for blocked provider discovery");
    assert!(!job.is_finished());
    assert!(!*gate.released.lock().unwrap());
    assert_eq!(
        harness
            .runtime
            .installed
            .disabled_turn
            .load(Ordering::Acquire),
        1
    );
    gate.open();
    assert!(job.await.unwrap().unwrap().is_none());
    server.join().unwrap();
    assert_eq!(count.load(Ordering::Acquire), 1);
    assert_eq!((a.count(), b.count()), (0, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_candidate_before_cancellation_is_cancelled_and_one_after_it_can_run() {
    let (a, b) = (Server::new(), Server::new());
    let mut harness = Harness::new(&a, &b);
    harness.baseline(&a).await;
    let gate = Arc::new(Gate::default());
    let _release = ReleaseGate(Arc::clone(&gate));
    let callback = Arc::clone(&gate);
    harness
        .fixture
        .store
        .set_burn_check_test_hook(Some(Arc::new(move |stage| {
            if stage == StoreStage::BeforeCancellation {
                callback.block();
            }
        })));
    let store = Arc::clone(&harness.fixture.store);
    let worker = Arc::clone(&harness.runtime.worker);
    let switch =
        tokio::task::spawn_blocking(move || headless::switch_keyless_profile(&store, &worker, "b"));
    gate.wait().await;
    assert!(!harness.runtime.worker.is_available());
    harness.publish(2);
    gate.open();
    assert!(switch.await.unwrap().unwrap());
    harness.fixture.store.set_burn_check_test_hook(None);
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
    b.reply("concern", None);
    harness.publish(3);
    assert_eq!(
        harness.job().await.unwrap().unwrap().unwrap().outcome,
        RunOutcome::Accepted
    );
    assert_eq!((a.count(), b.count()), (1, 1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_cancelled_queue_accepts_fresh_selected_work_without_restart() {
    let (a, b) = (Server::new(), Server::new());
    let mut harness = Harness::new(&a, &b);
    harness.baseline(&a).await;
    harness.publish(129);
    let pending: u64 = harness
        .fixture
        .store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM burn_check_candidate WHERE state = 'pending'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 128);
    harness.select("b");
    let source = harness.protected();
    harness
        .host
        .projects
        .values()
        .next()
        .unwrap()
        .engine
        .recover_pending()
        .unwrap();
    assert_eq!(harness.protected(), source);
    b.reply("concern", None);
    harness.publish(130);
    assert_eq!(
        harness.job().await.unwrap().unwrap().unwrap().outcome,
        RunOutcome::Accepted
    );
    let (cancelled, overflowed): (u64, u64) = harness.fixture.store.lock().query_row("SELECT SUM(state = 'cancelled'), SUM(state = 'not_assessed') FROM burn_check_candidate", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!((cancelled, overflowed), (128, 0));
    assert_eq!((a.count(), b.count()), (1, 1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_or_unsupported_job_discovery_preserves_definition_and_saved_results() {
    for unsupported in [false, true] {
        let (a, b) = (Server::new(), Server::new());
        let mut harness = Harness::new(&a, &b);
        harness.baseline(&a).await;
        let prior = harness.status();
        harness
            .runtime
            .fail_resolution
            .store(!unsupported, Ordering::Release);
        harness
            .runtime
            .unsupported_resolution
            .store(unsupported, Ordering::Release);
        harness.publish(2);
        let protected = harness.protected();
        assert!(harness.job().await.unwrap().unwrap().is_none());
        let generation = harness
            .runtime
            .worker
            .execution_snapshot()
            .map(|snapshot| snapshot.generation);
        let (capabilities, capability_error) =
            execution::coordinator_capabilities(&harness.runtime.installed, generation);
        harness.host.capabilities = capabilities;
        harness.host.capability_error = capability_error;
        harness.host.reconcile(&Admission::Ready).unwrap();
        publish_metadata(&harness.runtime.installed, &harness.host.definitions);
        assert_eq!(harness.protected(), protected);
        assert_eq!(harness.status(), prior);
        assert!(matches!(
            harness.host.definitions[0].availability,
            InstalledCheckAvailability::Unsupported { .. }
        ));
        let calls = harness.runtime.discoveries.load(Ordering::Acquire);
        assert!(harness.job().await.unwrap().unwrap().is_none());
        assert_eq!(harness.runtime.discoveries.load(Ordering::Acquire), calls);
        assert_eq!((a.count(), b.count()), (1, 0));
        harness.select("b");
        harness
            .runtime
            .fail_resolution
            .store(false, Ordering::Release);
        harness
            .runtime
            .unsupported_resolution
            .store(false, Ordering::Release);
        harness.host.capabilities = None;
        harness.host.capability_error = None;
        harness.host.reconcile(&Admission::Ready).unwrap();
        publish_metadata(&harness.runtime.installed, &harness.host.definitions);
        assert_eq!(harness.protected(), protected);
        b.reply("concern", None);
        harness.publish(3);
        assert_eq!(
            harness.job().await.unwrap().unwrap().unwrap().outcome,
            RunOutcome::Accepted
        );
        assert_eq!((a.count(), b.count()), (1, 1));
    }
}
