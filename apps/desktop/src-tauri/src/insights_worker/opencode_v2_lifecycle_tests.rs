use std::sync::atomic::AtomicUsize;

use base64::Engine;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::broadcast,
    task::JoinSet,
};

use super::runtime_tests::Runtime;
use super::*;

const DISCOVERY_KEY: &str = "internal:yamlBurnChecksDiscoveryEnabledV1";
const MASTER_KEY: &str = "internal:burnChecksEnabledAtEpochV1";

#[derive(Default)]
struct ServiceState {
    password: std::sync::Mutex<String>,
    version: std::sync::Mutex<String>,
    open_connections: AtomicUsize,
    info_requests: AtomicUsize,
    event_requests: AtomicUsize,
    feeds: AtomicUsize,
    open_feeds: AtomicUsize,
    closed_feeds: AtomicUsize,
    hold_info: AtomicBool,
    hold_info_after: AtomicUsize,
    hold_event_headers: AtomicBool,
    reject_info: AtomicBool,
    heartbeat: AtomicBool,
}

struct FixtureHost {
    store: Store,
    database: PathBuf,
    registration: PathBuf,
    enabled: AtomicBool,
    stopped: AtomicBool,
    hold_baseline: AtomicBool,
    baseline_calls: AtomicUsize,
    baseline_active: AtomicUsize,
    hold_locate: AtomicBool,
    locate_waiting: AtomicBool,
}

#[async_trait::async_trait]
impl CaptureHost for FixtureHost {
    fn store(&self) -> &Store {
        &self.store
    }
    fn ready(&self) -> bool {
        crate::installed_checks::live_delivery_permitted(
            &self.store,
            self.enabled.load(Ordering::SeqCst),
            self.stopping(),
            true,
        )
    }
    fn stopping(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
    fn registration_path(&self) -> anyhow::Result<PathBuf> {
        Ok(self.registration.clone())
    }
    fn announce(&self, _: &SessionKey) {}
    fn report_changed(&self) {}
    async fn wait_for_checks(&self) {}
    async fn locate_sources(
        &self,
        records: &[crate::store::SessionRecord],
        cancelled: &AtomicBool,
    ) -> anyhow::Result<Vec<(crate::store::SessionRecord, PathBuf)>> {
        while self.hold_locate.load(Ordering::SeqCst) && !cancelled.load(Ordering::SeqCst) {
            self.locate_waiting.store(true, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        self.locate_waiting.store(false, Ordering::SeqCst);
        if cancelled.load(Ordering::SeqCst) {
            return Err(opencode_live::stream::EventStreamError::Cancelled.into());
        }
        locate_v2_sources(records, cancelled).await
    }
    fn read_baselines(
        &self,
        groups: &[DatabaseGroup],
        targets: &[ToolResultObservation],
        cancel: &dyn Fn() -> bool,
    ) -> anyhow::Result<CaptureReconciliation> {
        self.baseline_calls.fetch_add(1, Ordering::SeqCst);
        self.baseline_active.fetch_add(1, Ordering::SeqCst);
        while self.hold_baseline.load(Ordering::SeqCst) && !cancel() {
            std::thread::sleep(Duration::from_millis(2));
        }
        let result = read_baselines(groups, targets, cancel);
        self.baseline_active.fetch_sub(1, Ordering::SeqCst);
        result
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    database: rusqlite::Connection,
    runtime: Runtime,
    host: Arc<FixtureHost>,
    service: Arc<ServiceState>,
    frames: broadcast::Sender<String>,
    server: tokio::task::JoinHandle<()>,
    endpoint: String,
}

fn open_database(path: &std::path::Path, baseline: u64) -> rusqlite::Connection {
    let database = rusqlite::Connection::open(path).unwrap();
    database.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE session_v2 (id TEXT PRIMARY KEY, parent_id TEXT, fork_session_id TEXT,
            directory TEXT NOT NULL DEFAULT '/repo', title TEXT, time_created INTEGER, time_updated INTEGER);
        CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER,
            time_created INTEGER, time_updated INTEGER, data TEXT);
        CREATE UNIQUE INDEX native_sequence ON session_message(session_id, seq);
        CREATE TABLE event_sequence (aggregate_id TEXT PRIMARY KEY NOT NULL, seq INTEGER NOT NULL);
        INSERT INTO session_v2 (id) VALUES ('ses_v2');").unwrap();
    database
        .execute(
            "INSERT INTO event_sequence VALUES ('ses_v2', ?1)",
            [baseline],
        )
        .unwrap();
    database
}

fn admitted_record(
    path: &std::path::Path,
    database: &rusqlite::Connection,
) -> crate::store::SessionRecord {
    admitted_record_for_session(path, database, "ses_v2")
}

fn admitted_record_for_session(
    path: &std::path::Path,
    database: &rusqlite::Connection,
    session: &str,
) -> crate::store::SessionRecord {
    let mut record = super::tests::record(unix_now());
    record.key.session_id = session.into();
    record.source_label = format!("opencode:{session}:open_code_v2_sqlite");
    let file = std::fs::File::open(path).unwrap();
    let identity = antiburn_local::discovery::SourceStat::from_open_std_file(&file)
        .unwrap()
        .identity
        .unwrap();
    let scope =
        antiburn_local::discovery::agents::opencode::db_session_admission_revision_connection_with_identity(
            database,
            &record.key.session_id,
            &identity,
        )
        .unwrap();
    record.activity_cursor = format!(
        "opencode-admission-v2:{}",
        json!([path, record.key.session_id, identity, scope])
    );
    record.source_fingerprint = None;
    record
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("opencode.db");
        let database = open_database(&database_path, 0);
        let runtime = Runtime::single_result(&directory.path().join("app"));
        runtime
            .store
            .upsert_sessions(
                &[admitted_record(&database_path, &database)],
                &crate::agents::evidence_cohort(),
            )
            .unwrap();
        runtime.store.set_internal_value(DISCOVERY_KEY, "true");
        let host = Arc::new(FixtureHost {
            store: (*runtime.store).clone(),
            database: database_path,
            registration: directory.path().join("service.json"),
            enabled: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            hold_baseline: AtomicBool::new(false),
            baseline_calls: AtomicUsize::new(0),
            baseline_active: AtomicUsize::new(0),
            hold_locate: AtomicBool::new(false),
            locate_waiting: AtomicBool::new(false),
        });
        let service = Arc::new(ServiceState::default());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (frames, _) = broadcast::channel(32);
        let server_state = service.clone();
        let server_frames = frames.clone();
        let server = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        tasks.spawn(serve(socket, server_state.clone(), server_frames.subscribe()));
                    }
                    completed = tasks.join_next(), if !tasks.is_empty() => { completed.unwrap().unwrap(); }
                }
            }
        });
        let fixture = Self {
            _directory: directory,
            database,
            runtime,
            host,
            service,
            frames,
            server,
            endpoint,
        };
        fixture.register("initial", "synthetic-secret");
        fixture
    }

    fn register(&self, id: &str, password: &str) {
        self.register_version(id, password, "2.0.32");
    }

    fn register_version(&self, id: &str, password: &str, version: &str) {
        *self.service.password.lock().unwrap() = password.to_owned();
        *self.service.version.lock().unwrap() = version.to_owned();
        std::fs::write(&self.host.registration, json!({
            "id":id, "url":self.endpoint, "pid":std::process::id(), "version":version, "password":password,
        }).to_string()).unwrap();
    }

    fn replace_database(&mut self, baseline: u64) {
        self.database
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
            .unwrap();
        std::fs::rename(
            &self.host.database,
            self._directory.path().join("previous.db"),
        )
        .unwrap();
        self.database = open_database(&self.host.database, baseline);
    }

    fn start(&self, timing: CaptureTiming) -> tokio::task::JoinHandle<()> {
        tokio::spawn(run_capture_driver(self.host.clone(), timing))
    }

    fn lease(&self) -> Option<String> {
        use rusqlite::OptionalExtension;
        self.runtime.store.lock().query_row(
            "SELECT capture_epoch FROM opencode_v2_capture WHERE lease_expires_at_epoch IS NOT NULL", [], |row| row.get(0),
        ).optional().unwrap()
    }

    fn status(&self) -> Option<String> {
        use rusqlite::OptionalExtension;
        self.runtime
            .store
            .lock()
            .query_row(
                "SELECT code FROM opencode_v2_source_status WHERE session_id = 'ses_v2'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
    }

    fn report(&self) -> crate::insights_report::installed_checks::InstalledCheckReportPayload {
        use crate::insights_report::installed_checks::{read_installed_checks, tests::definition};
        let mut definition = definition("/repo");
        definition.check_id = "v2-check".into();
        read_installed_checks(&self.runtime.store, &[definition])
            .unwrap()
            .remove(0)
    }

    fn assert_blocked_report(&self, code: &str) {
        let report = self.report();
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == code),
            "{:?}",
            report.diagnostics
        );
        assert_eq!(report.clean, 0);
        assert_eq!(report.waiting_for_data_count, 0);
        assert_eq!(report.waiting_for_results_count, 0);
        assert!(report.waiting_for_events.is_none());
    }

    fn send(&self, events: Vec<Value>) {
        project_fixture_events(&self.database, &events);
        let mut watermarks = HashMap::new();
        for event in &events {
            watermarks.insert(
                event
                    .pointer("/durable/aggregateID")
                    .unwrap()
                    .as_str()
                    .unwrap(),
                event.pointer("/durable/seq").unwrap().as_u64().unwrap(),
            );
        }
        for (session, sequence) in watermarks {
            assert_eq!(
                self.database
                    .execute(
                        "UPDATE event_sequence SET seq = ?1 WHERE aggregate_id = ?2",
                        rusqlite::params![sequence, session],
                    )
                    .unwrap(),
                1
            );
        }
        let body = events
            .into_iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
        self.frames.send(body).unwrap();
    }

    async fn stop(&self, driver: tokio::task::JoinHandle<()>) {
        self.host.stopped.store(true, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(3), driver)
            .await
            .unwrap()
            .unwrap();
        wait_for(|| {
            self.service.open_feeds.load(Ordering::SeqCst) == 0
                && self.service.open_connections.load(Ordering::SeqCst) == 0
                && self.host.baseline_active.load(Ordering::SeqCst) == 0
        })
        .await;
        assert!(self.lease().is_none());
    }
}

fn project_fixture_events(database: &rusqlite::Connection, events: &[Value]) {
    for event in events {
        let session = event.pointer("/data/sessionID").unwrap().as_str().unwrap();
        let data = &event["data"];
        let kind = event["type"].as_str().unwrap();
        let (id, message_type, message) = match kind {
            "session.inbox.enqueued" => (
                data["inboxID"].as_str().unwrap(),
                "user",
                json!({"text":data.pointer("/item/payload/text").unwrap()}),
            ),
            "session.tool.success" | "session.tool.failed" => {
                let id = data["assistantMessageID"].as_str().unwrap();
                let call = data["id"].as_str().unwrap();
                let started = events
                    .iter()
                    .find(|event| {
                        event["type"] == "session.tool.input.started"
                            && event
                                .pointer("/data/assistantMessageID")
                                .and_then(Value::as_str)
                                == Some(id)
                            && event.pointer("/data/id").and_then(Value::as_str) == Some(call)
                    })
                    .unwrap();
                let called = events
                    .iter()
                    .find(|event| {
                        event["type"] == "session.tool.called"
                            && event
                                .pointer("/data/assistantMessageID")
                                .and_then(Value::as_str)
                                == Some(id)
                            && event.pointer("/data/id").and_then(Value::as_str) == Some(call)
                    })
                    .unwrap();
                let message = json!({"content":[{
                    "type":"tool", "id":call, "name":started["data"]["name"], "executed":data["executed"],
                    "state":{"status":if kind == "session.tool.success" {"completed"} else {"error"},
                        "input":called["data"]["input"], "content":data["content"]},
                }]});
                (id, "assistant", message)
            }
            _ => continue,
        };
        database.execute(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
               VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(seq), -1) + 1 FROM session_message WHERE session_id = ?2), 1000, 1000, ?4)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data",
            rusqlite::params![id, session, message_type, message.to_string()],
        ).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn serve(
    mut socket: TcpStream,
    state: Arc<ServiceState>,
    mut frames: broadcast::Receiver<String>,
) {
    state.open_connections.fetch_add(1, Ordering::SeqCst);
    let _connection_count = ConnectionCount(state.clone());
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        if socket.read_exact(&mut byte).await.is_err() {
            return;
        }
        request.push(byte[0]);
        assert!(request.len() < 8192);
    }
    let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
    let auth = base64::engine::general_purpose::STANDARD
        .encode(format!(
            "opencode:{}",
            state.password.lock().unwrap().as_str()
        ))
        .to_ascii_lowercase();
    let authorized = request.contains(&format!("authorization: basic {auth}"));
    if request.starts_with("get /api/info ") {
        let count = state.info_requests.fetch_add(1, Ordering::SeqCst) + 1;
        let hold_after = state.hold_info_after.load(Ordering::SeqCst);
        if state.hold_info.load(Ordering::SeqCst) || (hold_after > 0 && count >= hold_after) {
            let _ = socket.read(&mut byte).await;
            return;
        }
        let (status, body) = if !authorized || state.reject_info.load(Ordering::SeqCst) {
            ("401 Unauthorized", "{}".to_owned())
        } else {
            (
                "200 OK",
                json!({"pid":std::process::id(), "version":state.version.lock().unwrap().as_str()})
                    .to_string(),
            )
        };
        let _ = socket
            .write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await;
        return;
    }
    assert!(request.starts_with("get /api/event "));
    assert!(authorized);
    state.event_requests.fetch_add(1, Ordering::SeqCst);
    if state.hold_event_headers.load(Ordering::SeqCst) {
        let _ = socket.read(&mut byte).await;
        return;
    }
    if socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\ndata: {\"type\":\"server.connected\"}\n\n").await.is_err() { return; }
    state.feeds.fetch_add(1, Ordering::SeqCst);
    state.open_feeds.fetch_add(1, Ordering::SeqCst);
    loop {
        tokio::select! {
            read = socket.read(&mut byte) => { if !matches!(read, Ok(n) if n > 0) { break; } }
            frame = frames.recv() => {
                let Ok(frame) = frame else { break; };
                if socket.write_all(frame.as_bytes()).await.is_err() { break; }
            }
            () = tokio::time::sleep(Duration::from_millis(20)), if state.heartbeat.load(Ordering::SeqCst) => {
                if socket.write_all(b": heartbeat\n\n").await.is_err() { break; }
            }
        }
    }
    state.open_feeds.fetch_sub(1, Ordering::SeqCst);
    state.closed_feeds.fetch_add(1, Ordering::SeqCst);
}

struct ConnectionCount(Arc<ServiceState>);

impl Drop for ConnectionCount {
    fn drop(&mut self) {
        self.0.open_connections.fetch_sub(1, Ordering::SeqCst);
    }
}

fn timing() -> CaptureTiming {
    CaptureTiming {
        settings_check: Duration::from_millis(5),
        recheck: Duration::from_millis(50),
        retry_base: Duration::from_millis(30),
        retry_max: Duration::from_millis(120),
        connect_timeout: Duration::from_millis(200),
        empty_scope_wait: Duration::from_millis(20),
        stream: opencode_live::stream::StreamTiming {
            idle_timeout: Duration::from_secs(3),
            cancel_poll: Duration::from_millis(5),
        },
    }
}

async fn wait_for(mut condition: impl FnMut() -> bool) {
    wait_for_named(
        &mut condition,
        "the capture transition must finish within its bound",
    )
    .await;
}

async fn wait_for_named(condition: &mut impl FnMut() -> bool, message: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{message}"));
}

fn event(sequence: u64, kind: &str, extra: Value) -> Value {
    let mut data = json!({"sessionID":"ses_v2"});
    data.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    json!({"id":format!("evt_{sequence}"), "type":kind, "created":1000,
        "durable":{"aggregateID":"ses_v2", "seq":sequence, "version":if matches!(kind, "session.tool.success" | "session.tool.failed" | "session.deleted") {2} else {1}}, "data":data})
}

fn result_events(start: u64) -> Vec<Value> {
    let request = format!("request-{start}");
    let step = format!("step-{start}");
    let call = format!("call-{start}");
    vec![
        event(
            start,
            "session.inbox.enqueued",
            json!({"inboxID":request, "item":{"type":"user", "payload":{"text":"Inspect the build"}, "delivery":"queue"}}),
        ),
        event(
            start + 1,
            "session.inbox.delivered",
            json!({"inboxID":request}),
        ),
        event(
            start + 2,
            "session.step.started",
            json!({"assistantMessageID":step, "agent":"build", "model":{"providerID":"synthetic", "id":"no-network"}, "started":1000}),
        ),
        event(
            start + 3,
            "session.tool.input.started",
            json!({"assistantMessageID":step, "id":call, "name":"read"}),
        ),
        event(
            start + 4,
            "session.tool.called",
            json!({"assistantMessageID":step, "id":call, "input":{"path":"source.rs"}, "executed":false}),
        ),
        event(
            start + 5,
            "session.tool.success",
            json!({"assistantMessageID":step, "id":call, "content":[{"type":"text", "text":"source"}], "executed":false}),
        ),
    ]
}

fn result_events_for_session(session: &str, start: u64) -> Vec<Value> {
    result_events(start)
        .into_iter()
        .map(|mut event| {
            event["id"] = json!(format!("{session}-{}", event["id"].as_str().unwrap()));
            event["durable"]["aggregateID"] = json!(session);
            event["data"]["sessionID"] = json!(session);
            for field in ["inboxID", "assistantMessageID", "id"] {
                if let Some(id) = event["data"][field].as_str() {
                    event["data"][field] = json!(format!("{session}-{id}"));
                }
            }
            event
        })
        .collect()
}

#[tokio::test]
async fn admitted_database_path_selects_the_matching_baseline_with_duplicate_session_ids() {
    let fixture = Fixture::new().await;
    let other_path = fixture._directory.path().join("other.db");
    let other = open_database(&other_path, 40);
    for (database, text) in [
        (&fixture.database, "unselected result"),
        (&other, "selected result"),
    ] {
        let data = json!({"time":{"created":1000,"completed":1001}, "content":[{
            "type":"tool", "id":"stored-call", "name":"read", "executed":false,
            "time":{"created":1000,"ran":1000,"completed":1001},
            "state":{"status":"completed", "input":{"path":"source.rs"},
                "content":[{"type":"text", "text":text}]}
        }]})
        .to_string();
        database.execute("INSERT INTO session_message VALUES ('stored-step', 'ses_v2', 'assistant', 0, 1000, 1001, ?1)", [data]).unwrap();
    }
    let selected = admitted_record(&other_path, &other);
    fixture
        .runtime
        .store
        .upsert_sessions(
            std::slice::from_ref(&selected),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let sources = locate_v2_sources(std::slice::from_ref(&selected), &AtomicBool::new(false))
        .await
        .unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].1, other_path);
    assert_eq!(sources[0].0.cwd, Some("/repo".into()));
    let driver = fixture.start(timing());
    wait_for(|| fixture.lease().is_some()).await;
    let baseline: Option<u64> = fixture
        .runtime
        .store
        .lock()
        .query_row(
            "SELECT baseline_sequence FROM opencode_v2_capture",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(baseline, Some(40));
    assert!(
        fixture
            .runtime
            .store
            .native_tool_results_current(&selected.key)
            .unwrap()
            .is_empty()
    );
    let mut events = result_events(41);
    events.last_mut().unwrap()["data"]["content"][0]["text"] = json!("selected live result");
    other
        .execute("UPDATE event_sequence SET seq = 46", [])
        .unwrap();
    fixture
        .frames
        .send(
            events
                .into_iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect(),
        )
        .unwrap();
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&selected.key)
            .unwrap()
            .len()
            == 1
    })
    .await;
    let results = fixture
        .runtime
        .store
        .native_tool_results_current(&selected.key)
        .unwrap();
    assert_eq!(
        results[0].result.content.as_deref(),
        Some("selected live result")
    );
    assert_eq!(fixture.service.feeds.load(Ordering::SeqCst), 1);
    fixture.stop(driver).await;
}

#[tokio::test]
async fn same_path_database_replacement_waits_for_a_new_admitted_generation() {
    for active in [false, true] {
        let mut fixture = Fixture::new().await;
        let key = SessionKey::new("native", "opencode", "ses_v2");
        let old_generation = fixture
            .runtime
            .store
            .source_generation_batch(std::slice::from_ref(&key))
            .unwrap()[0]
            .unwrap();
        let running = active.then(|| fixture.start(timing()));
        if active {
            wait_for(|| fixture.lease().is_some()).await;
        }
        fixture.replace_database(90);
        let driver = running.unwrap_or_else(|| fixture.start(timing()));
        wait_for(|| {
            fixture.status().as_deref() == Some("source-mismatch") && fixture.lease().is_none()
        })
        .await;
        let feeds = fixture.service.feeds.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(fixture.service.feeds.load(Ordering::SeqCst), feeds);
        assert_eq!(feeds, usize::from(active));
        assert!(
            fixture
                .runtime
                .store
                .native_tool_results_current(&key)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            fixture
                .runtime
                .store
                .source_generation_batch(std::slice::from_ref(&key))
                .unwrap()[0],
            Some(old_generation)
        );
        let replacement = admitted_record(&fixture.host.database, &fixture.database);
        fixture
            .runtime
            .store
            .upsert_sessions(&[replacement], &crate::agents::evidence_cohort())
            .unwrap();
        let new_generation = fixture
            .runtime
            .store
            .source_generation_batch(std::slice::from_ref(&key))
            .unwrap()[0]
            .unwrap();
        assert!(new_generation > old_generation);
        wait_for(|| fixture.lease().is_some()).await;
        let binding: (i64, Option<u64>) = fixture
            .runtime
            .store
            .lock()
            .query_row(
                "SELECT source_generation, baseline_sequence FROM opencode_v2_capture",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(binding, (new_generation, Some(90)));
        fixture.send(result_events(91));
        wait_for(|| {
            fixture
                .runtime
                .store
                .native_tool_results_current(&key)
                .unwrap()
                .len()
                == 1
        })
        .await;
        assert!(
            fixture
                .runtime
                .store
                .native_tool_result_publications_after(0, 100)
                .unwrap()
                .iter()
                .filter(|row| row.payload.is_some())
                .all(|row| row.source_generation == new_generation)
        );
        fixture.stop(driver).await;
    }
}

#[tokio::test]
async fn replacement_after_source_resolution_fails_even_with_a_valid_native_process_binding() {
    let mut fixture = Fixture::new().await;
    let record = admitted_record(&fixture.host.database, &fixture.database);
    let cancelled = AtomicBool::new(false);
    let sources = locate_v2_sources(&[record], &cancelled).await.unwrap();
    assert_eq!(sources.len(), 1);
    fixture.replace_database(90);
    let groups = select_bound_databases(
        &fixture.host.registration,
        sources,
        timing().connect_timeout,
        &cancelled,
    )
    .await
    .unwrap();
    assert_eq!(groups.len(), 1);
    let error = match DatabaseIdentity::read_admitted(&groups[0].0, &groups[0].1) {
        Ok(_) => panic!("capture must reject the replacement identity"),
        Err(error) => error,
    };
    assert_eq!(
        capture_failure_status(&error),
        OpenCodeV2SourceStatus::SourceMismatch
    );
    assert_eq!(fixture.service.event_requests.load(Ordering::SeqCst), 0);
    assert!(fixture.lease().is_none());
}

#[tokio::test]
async fn admitted_database_change_during_hydration_rejects_the_old_baseline() {
    let fixture = Fixture::new().await;
    fixture.host.hold_baseline.store(true, Ordering::SeqCst);
    let host: Arc<dyn CaptureHost> = fixture.host.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let epoch = tokio::spawn(async move { run_capture_epoch(&host, timing(), cancelled).await });
    wait_for(|| fixture.host.baseline_active.load(Ordering::SeqCst) > 0).await;
    let replacement_path = fixture._directory.path().join("replacement.db");
    let replacement = open_database(&replacement_path, 100);
    fixture
        .runtime
        .store
        .upsert_sessions(
            &[admitted_record(&replacement_path, &replacement)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    fixture.host.hold_baseline.store(false, Ordering::SeqCst);
    let error = tokio::time::timeout(Duration::from_secs(3), epoch)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        capture_failure_status(&error),
        OpenCodeV2SourceStatus::SourceMismatch
    );
    assert!(fixture.lease().is_none());
    assert_eq!(fixture.runtime.count("native_tool_result_publication"), 0);
    wait_for(|| fixture.service.open_feeds.load(Ordering::SeqCst) == 0).await;
}

#[tokio::test]
async fn malformed_or_missing_admitted_identity_never_uses_a_session_lookup() {
    let fixture = Fixture::new().await;
    let record = admitted_record(&fixture.host.database, &fixture.database);
    let mut fields: Value = serde_json::from_str(
        record
            .activity_cursor
            .strip_prefix("opencode-admission-v2:")
            .unwrap(),
    )
    .unwrap();
    let mut cursors = vec![
        String::new(),
        "v2-test".into(),
        "opencode-admission-v2:{".into(),
    ];
    for (index, value) in [
        (0, json!("relative.db")),
        (1, json!("other-session")),
        (2, Value::Null),
        (2, json!("")),
        (3, json!("")),
    ] {
        let previous = std::mem::replace(&mut fields[index], value);
        cursors.push(format!("opencode-admission-v2:{fields}"));
        fields[index] = previous;
    }
    for cursor in cursors {
        let mut record = record.clone();
        record.activity_cursor = cursor;
        assert!(
            locate_v2_sources(&[record], &AtomicBool::new(false))
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(fixture.service.info_requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn same_database_admission_updates_keep_the_existing_feed() {
    let fixture = Fixture::new().await;
    let driver = fixture.start(timing());
    wait_for(|| fixture.lease().is_some()).await;
    let epoch = fixture.lease().unwrap();
    fixture
        .database
        .execute("UPDATE session_v2 SET time_updated = 1000", [])
        .unwrap();
    fixture
        .runtime
        .store
        .upsert_sessions(
            &[admitted_record(&fixture.host.database, &fixture.database)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(fixture.lease().as_deref(), Some(epoch.as_str()));
    assert_eq!(fixture.service.feeds.load(Ordering::SeqCst), 1);
    fixture.send(result_events(1));
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&SessionKey::new("native", "opencode", "ses_v2"))
            .unwrap()
            .len()
            == 1
    })
    .await;
    fixture.stop(driver).await;
}

#[tokio::test]
async fn new_unbound_candidate_reports_source_mismatch_without_stopping_the_healthy_feed() {
    let fixture = Fixture::new().await;
    let driver = fixture.start(timing());
    wait_for(|| fixture.lease().is_some()).await;
    let epoch = fixture.lease().unwrap();
    let path = fixture._directory.path().join("unbound-added.db");
    let database = open_database(&path, 0);
    database
        .execute("UPDATE session_v2 SET id = 'ses_unbound'", [])
        .unwrap();
    database
        .execute("UPDATE event_sequence SET aggregate_id = 'ses_unbound'", [])
        .unwrap();
    let record = admitted_record_for_session(&path, &database, "ses_unbound");
    drop(database);
    fixture
        .runtime
        .store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    wait_for(|| fixture.runtime.store.lock().query_row(
        "SELECT COUNT(*) FROM opencode_v2_source_status WHERE session_id = 'ses_unbound' AND code = 'source-mismatch'", [], |row| row.get::<_, usize>(0),
    ).unwrap() == 1).await;
    fixture.send(result_events(1));
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&SessionKey::new("native", "opencode", "ses_v2"))
            .unwrap()
            .len()
            == 1
    })
    .await;
    assert_eq!(fixture.lease().as_deref(), Some(epoch.as_str()));
    assert_eq!(fixture.service.feeds.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.host.baseline_calls.load(Ordering::SeqCst), 1);
    fixture.stop(driver).await;
}

#[tokio::test]
async fn new_same_database_candidate_keeps_healthy_feed_and_hydrates_without_replay() {
    let fixture = Fixture::new().await;
    let driver = fixture.start(timing());
    wait_for(|| fixture.lease().is_some()).await;
    let healthy_epoch = fixture.lease().unwrap();
    fixture.runtime.sync();
    fixture.host.hold_baseline.store(true, Ordering::SeqCst);
    fixture
        .database
        .execute_batch(
            "INSERT INTO session_v2 (id) VALUES ('ses_added');
         INSERT INTO event_sequence VALUES ('ses_added', 0);",
        )
        .unwrap();
    fixture
        .runtime
        .store
        .upsert_sessions(
            &[admitted_record_for_session(
                &fixture.host.database,
                &fixture.database,
                "ses_added",
            )],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    wait_for(|| fixture.host.baseline_active.load(Ordering::SeqCst) > 0).await;
    fixture.send(result_events_for_session("ses_added", 1));
    fixture.send(result_events(1));
    let healthy = SessionKey::new("native", "opencode", "ses_v2");
    let added = SessionKey::new("native", "opencode", "ses_added");
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&healthy)
            .unwrap()
            .len()
            == 1
    })
    .await;
    fixture.runtime.sync();
    assert_eq!(
        fixture.runtime.run(100_000).outcome,
        antiburn_checks::engine::RunOutcome::Accepted
    );
    fixture.host.hold_baseline.store(false, Ordering::SeqCst);
    wait_for(|| {
        fixture
            .runtime
            .store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM opencode_v2_capture WHERE lease_expires_at_epoch IS NOT NULL",
                [],
                |row| row.get::<_, usize>(0),
            )
            .unwrap()
            == 2
    })
    .await;
    assert_eq!(fixture.service.feeds.load(Ordering::SeqCst), 1);
    let epoch: String = fixture
        .runtime
        .store
        .lock()
        .query_row(
            "SELECT capture_epoch FROM opencode_v2_capture WHERE session_id = 'ses_v2'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(epoch, healthy_epoch);
    assert!(
        fixture
            .runtime
            .store
            .native_tool_results_current(&added)
            .unwrap()
            .is_empty()
    );
    fixture.runtime.sync();
    fixture.send(result_events_for_session("ses_added", 7));
    fixture.send(result_events(7));
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&healthy)
            .unwrap()
            .len()
            == 2
            && fixture
                .runtime
                .store
                .native_tool_results_current(&added)
                .unwrap()
                .len()
                == 1
    })
    .await;
    fixture.runtime.sync();
    for _ in 0..2 {
        assert_eq!(
            fixture.runtime.run(200_000).outcome,
            antiburn_checks::engine::RunOutcome::Accepted
        );
    }
    fixture.runtime.assert_idle(300_000);
    assert_eq!(fixture.runtime.evaluator_calls(), 3);
    assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 3);
    assert_eq!(fixture.runtime.count("burn_check_assessment_history"), 3);
    fixture.stop(driver).await;
}

#[tokio::test]
async fn shutdown_during_connect_hydration_live_recovery_and_retry_finishes_tasks() {
    for phase in [
        "connect",
        "stream-connect",
        "stream-proof",
        "hydration",
        "live",
        "recovery",
        "live-proof",
        "retry",
    ] {
        let fixture = Fixture::new().await;
        fixture
            .service
            .hold_info
            .store(phase == "connect", Ordering::SeqCst);
        fixture
            .service
            .hold_event_headers
            .store(phase == "stream-connect", Ordering::SeqCst);
        fixture
            .service
            .reject_info
            .store(phase == "retry", Ordering::SeqCst);
        fixture
            .host
            .hold_baseline
            .store(phase == "hydration", Ordering::SeqCst);
        if phase == "stream-proof" {
            fixture.service.hold_info_after.store(3, Ordering::SeqCst);
        }
        let mut config = timing();
        config.connect_timeout = Duration::from_secs(30);
        let driver = fixture.start(config);
        match phase {
            "connect" | "retry" => {
                wait_for(|| fixture.service.info_requests.load(Ordering::SeqCst) > 0).await
            }
            "stream-connect" => {
                wait_for(|| fixture.service.event_requests.load(Ordering::SeqCst) > 0).await
            }
            "stream-proof" => {
                wait_for(|| fixture.service.info_requests.load(Ordering::SeqCst) >= 3).await
            }
            "hydration" => {
                wait_for(|| fixture.host.baseline_active.load(Ordering::SeqCst) > 0).await
            }
            _ => wait_for(|| fixture.lease().is_some()).await,
        }
        if phase == "recovery" {
            fixture.host.hold_baseline.store(true, Ordering::SeqCst);
            let mut unknown = event(
                1,
                "session.step.started",
                json!({"assistantMessageID":"step"}),
            );
            unknown["durable"]["version"] = json!(99);
            fixture.send(vec![unknown]);
            wait_for(|| {
                fixture.host.baseline_calls.load(Ordering::SeqCst) >= 2
                    && fixture.host.baseline_active.load(Ordering::SeqCst) > 0
            })
            .await;
        }
        if phase == "live-proof" {
            let count = fixture.service.info_requests.load(Ordering::SeqCst);
            fixture.service.hold_info.store(true, Ordering::SeqCst);
            wait_for(|| fixture.service.info_requests.load(Ordering::SeqCst) > count).await;
        }
        fixture.stop(driver).await;
        let report = fixture.report();
        assert_eq!(report.clean, 0, "{phase}");
        assert_eq!(report.waiting_for_results_count, 0, "{phase}");
        assert!(report.waiting_for_events.is_none(), "{phase}");
        assert_eq!(
            fixture.runtime.count("burn_check_evaluation_attempt"),
            0,
            "{phase}"
        );
        assert_eq!(fixture.runtime.evaluator_calls(), 0, "{phase}");
        assert!(
            fixture.service.info_requests.load(Ordering::SeqCst) < 12,
            "{phase}"
        );
    }
}

#[tokio::test]
async fn master_and_yaml_discovery_pause_replace_one_feed_but_disabled_agents_does_not_pause() {
    for pause in ["master", "yaml-discovery"] {
        let fixture = Fixture::new().await;
        if pause == "master" {
            fixture.runtime.store.set_internal_value(MASTER_KEY, "1");
            fixture
                .runtime
                .store
                .lock()
                .execute("DELETE FROM setting WHERE key = ?1", [DISCOVERY_KEY])
                .unwrap();
        }
        let driver = fixture.start(timing());
        wait_for(|| fixture.lease().is_some()).await;
        let first = fixture.lease().unwrap();
        let mut settings = fixture.runtime.store.settings().unwrap();
        settings.disabled_agents = crate::store::DisabledAgents::parse("opencode");
        fixture.runtime.store.replace_settings(&settings).unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(fixture.lease().as_deref(), Some(first.as_str()));
        assert_eq!(fixture.service.feeds.load(Ordering::SeqCst), 1);
        if pause == "master" {
            fixture
                .runtime
                .store
                .lock()
                .execute("DELETE FROM setting WHERE key = ?1", [MASTER_KEY])
                .unwrap();
        } else {
            fixture
                .runtime
                .store
                .set_internal_value(DISCOVERY_KEY, "false");
        }
        wait_for(|| {
            fixture.lease().is_none() && fixture.service.open_feeds.load(Ordering::SeqCst) == 0
        })
        .await;
        let attempts = fixture.service.info_requests.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            fixture.service.info_requests.load(Ordering::SeqCst),
            attempts
        );
        if pause == "master" {
            fixture.runtime.store.set_internal_value(MASTER_KEY, "1");
        } else {
            fixture
                .runtime
                .store
                .set_internal_value(DISCOVERY_KEY, "true");
        }
        wait_for(|| fixture.lease().is_some_and(|epoch| epoch != first)).await;
        assert_eq!(fixture.service.feeds.load(Ordering::SeqCst), 2);
        fixture.stop(driver).await;
        assert_eq!(fixture.runtime.evaluator_calls(), 0);
    }
}

#[tokio::test]
async fn session_discovery_pause_keeps_admitted_checks_and_the_existing_feed() {
    let fixture = Fixture::new().await;
    let driver = fixture.start(timing());
    wait_for(|| fixture.lease().is_some()).await;
    fixture.runtime.sync();
    let epoch = fixture.lease().unwrap();
    let mut settings = fixture.runtime.store.settings().unwrap();
    settings.discovery_paused = true;
    fixture.runtime.store.replace_settings(&settings).unwrap();
    fixture.send(result_events(1));
    let key = SessionKey::new("native", "opencode", "ses_v2");
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&key)
            .unwrap()
            .len()
            == 1
    })
    .await;
    fixture.runtime.sync();
    assert_eq!(
        fixture.runtime.run(100_000).outcome,
        antiburn_checks::engine::RunOutcome::Accepted
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(fixture.lease().as_deref(), Some(epoch.as_str()));
    assert_eq!(fixture.service.feeds.load(Ordering::SeqCst), 1);
    settings.discovery_paused = false;
    fixture.runtime.store.replace_settings(&settings).unwrap();
    fixture.send(result_events(7));
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&key)
            .unwrap()
            .len()
            == 2
    })
    .await;
    fixture.runtime.sync();
    assert_eq!(
        fixture.runtime.run(200_000).outcome,
        antiburn_checks::engine::RunOutcome::Accepted
    );
    assert_eq!(fixture.lease().as_deref(), Some(epoch.as_str()));
    assert_eq!(fixture.service.feeds.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.runtime.evaluator_calls(), 2);
    assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 2);
    fixture.stop(driver).await;
}

#[tokio::test]
async fn session_admission_accepts_exact_limit_and_rejects_larger_scopes() {
    for count in [
        CAPTURE_SESSION_LIMIT,
        CAPTURE_SESSION_LIMIT + 1,
        CANDIDATE_SESSION_LIMIT,
    ] {
        let fixture = Fixture::new().await;
        let mut records = vec![admitted_record(&fixture.host.database, &fixture.database)];
        for index in 1..count {
            let session = format!("ses_bound_{index}");
            fixture
                .database
                .execute("INSERT INTO session_v2 (id) VALUES (?1)", [&session])
                .unwrap();
            fixture
                .database
                .execute("INSERT INTO event_sequence VALUES (?1, 0)", [&session])
                .unwrap();
            records.push(admitted_record_for_session(
                &fixture.host.database,
                &fixture.database,
                &session,
            ));
        }
        fixture
            .runtime
            .store
            .upsert_sessions(&records, &crate::agents::evidence_cohort())
            .unwrap();
        let driver = fixture.start(timing());
        if count == CAPTURE_SESSION_LIMIT {
            wait_for(|| fixture.lease().is_some()).await;
            assert_eq!(
                fixture.runtime.count("opencode_v2_capture"),
                CAPTURE_SESSION_LIMIT as u64
            );
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert_eq!(fixture.service.feeds.load(Ordering::SeqCst), 1);
            assert_eq!(fixture.host.baseline_calls.load(Ordering::SeqCst), 1);
            assert_eq!(fixture.status().as_deref(), Some("live"));
        } else {
            wait_for(|| fixture.status().as_deref() == Some("source-limit")).await;
            fixture.assert_blocked_report("opencode-v2-source-limit");
            assert_eq!(fixture.runtime.count("opencode_v2_capture"), 0);
            assert_eq!(fixture.service.event_requests.load(Ordering::SeqCst), 0);
        }
        assert_eq!(fixture.runtime.count("native_tool_result_current"), 0);
        assert_eq!(fixture.runtime.evaluator_calls(), 0);
        fixture.stop(driver).await;
    }
}

#[tokio::test]
async fn database_candidates_accept_eight_with_explicit_unbound_status_and_reject_nine() {
    for count in [CAPTURE_DATABASE_LIMIT, CAPTURE_DATABASE_LIMIT + 1] {
        let fixture = Fixture::new().await;
        let mut records = Vec::new();
        for index in 1..count {
            let path = fixture
                ._directory
                .path()
                .join(format!("unbound-{index}.db"));
            let database = open_database(&path, 0);
            let session = format!("ses_unbound_{index}");
            database
                .execute("UPDATE session_v2 SET id = ?1", [&session])
                .unwrap();
            database
                .execute("UPDATE event_sequence SET aggregate_id = ?1", [&session])
                .unwrap();
            records.push(admitted_record_for_session(&path, &database, &session));
            drop(database);
        }
        fixture
            .runtime
            .store
            .upsert_sessions(&records, &crate::agents::evidence_cohort())
            .unwrap();
        let driver = fixture.start(timing());
        if count == CAPTURE_DATABASE_LIMIT {
            wait_for(|| fixture.lease().is_some()).await;
            assert_eq!(fixture.runtime.count("opencode_v2_capture"), 1);
            let unbound: usize = fixture
                .runtime
                .store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM opencode_v2_source_status WHERE code = 'source-mismatch'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(unbound, CAPTURE_DATABASE_LIMIT - 1);
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert_eq!(fixture.service.feeds.load(Ordering::SeqCst), 1);
            assert_eq!(fixture.host.baseline_calls.load(Ordering::SeqCst), 1);
        } else {
            wait_for(|| fixture.status().as_deref() == Some("source-limit")).await;
            fixture.assert_blocked_report("opencode-v2-source-limit");
            assert_eq!(fixture.runtime.count("opencode_v2_capture"), 0);
            assert_eq!(fixture.service.event_requests.load(Ordering::SeqCst), 0);
            assert_eq!(fixture.service.info_requests.load(Ordering::SeqCst), 0);
        }
        assert_eq!(fixture.runtime.count("native_tool_result_current"), 0);
        assert_eq!(fixture.runtime.evaluator_calls(), 0);
        fixture.stop(driver).await;
    }
}

#[tokio::test]
async fn active_credential_rotation_and_registration_replacement_retire_old_epochs() {
    let fixture = Fixture::new().await;
    let driver = fixture.start(timing());
    wait_for(|| fixture.lease().is_some()).await;
    fixture.runtime.sync();
    fixture.send(result_events(1));
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&SessionKey::new("native", "opencode", "ses_v2"))
            .unwrap()
            .len()
            == 1
    })
    .await;
    fixture.runtime.sync();
    assert_eq!(
        fixture.runtime.run(100_000).outcome,
        antiburn_checks::engine::RunOutcome::Accepted
    );
    for (id, password) in [
        ("initial", "rotated-synthetic-secret"),
        ("replacement", "synthetic-secret"),
    ] {
        let old = fixture.lease().unwrap();
        let closed = fixture.service.closed_feeds.load(Ordering::SeqCst);
        fixture.host.hold_locate.store(true, Ordering::SeqCst);
        wait_for(|| fixture.host.locate_waiting.load(Ordering::SeqCst)).await;
        fixture.register(id, password);
        fixture.host.hold_locate.store(false, Ordering::SeqCst);
        wait_for(|| fixture.status().as_deref() == Some("source-mismatch")).await;
        fixture.assert_blocked_report("opencode-v2-source-mismatch");
        wait_for(|| fixture.lease().is_some_and(|epoch| epoch != old)).await;
        assert!(fixture.service.closed_feeds.load(Ordering::SeqCst) > closed);
        assert_eq!(fixture.service.open_feeds.load(Ordering::SeqCst), 1);
        fixture.runtime.sync();
        fixture.runtime.assert_idle(200_000);
        assert_eq!(fixture.runtime.evaluator_calls(), 1);
        assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 1);
        assert_eq!(fixture.runtime.count("burn_check_assessment_history"), 1);
    }
    assert!(
        fixture
            .runtime
            .store
            .native_tool_result_publications_after(0, 100)
            .unwrap()
            .iter()
            .any(|row| row.gap_reason.as_deref() == Some("worker-stopped"))
    );
    fixture.send(result_events(7));
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&SessionKey::new("native", "opencode", "ses_v2"))
            .unwrap()
            .len()
            == 2
    })
    .await;
    fixture.runtime.sync();
    assert_eq!(
        fixture.runtime.run(200_000).outcome,
        antiburn_checks::engine::RunOutcome::Accepted
    );
    fixture.stop(driver).await;
    assert_eq!(fixture.runtime.evaluator_calls(), 2);
    assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 2);
    assert_eq!(fixture.runtime.count("burn_check_assessment_history"), 2);
}

#[tokio::test]
async fn registration_replacement_blocks_the_next_old_feed_event() {
    let fixture = Fixture::new().await;
    let driver = fixture.start(timing());
    wait_for(|| fixture.lease().is_some()).await;
    let old = fixture.lease().unwrap();
    fixture.register("replacement", "synthetic-secret");
    fixture.send(result_events(1));
    wait_for(|| fixture.status().as_deref() == Some("source-mismatch")).await;
    assert!(fixture.lease().is_none_or(|epoch| epoch != old));
    assert!(
        fixture
            .runtime
            .store
            .native_tool_results_current(&SessionKey::new("native", "opencode", "ses_v2"))
            .unwrap()
            .is_empty()
    );
    fixture.runtime.sync();
    assert_eq!(fixture.runtime.evaluator_calls(), 0);
    fixture.stop(driver).await;
}

#[tokio::test]
async fn selecting_a_different_open_database_blocks_the_next_old_feed_event() {
    let fixture = Fixture::new().await;
    let driver = fixture.start(timing());
    wait_for(|| fixture.lease().is_some()).await;
    let other_path = fixture._directory.path().join("new-selection.db");
    let other = open_database(&other_path, 40);
    fixture
        .runtime
        .store
        .upsert_sessions(
            &[admitted_record(&other_path, &other)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    fixture.send(result_events(1));
    wait_for(|| fixture.status().as_deref() == Some("source-mismatch")).await;
    let key = SessionKey::new("native", "opencode", "ses_v2");
    assert!(
        fixture
            .runtime
            .store
            .native_tool_results_current(&key)
            .unwrap()
            .is_empty()
    );
    assert_eq!(fixture.runtime.evaluator_calls(), 0);
    fixture.stop(driver).await;
}

#[tokio::test]
async fn reconnect_retires_exact_deleted_and_changed_bindings_without_a_historical_attempt() {
    for correction in ["delete", "tool-content", "request-content"] {
        let fixture = Fixture::new().await;
        let driver = fixture.start(timing());
        wait_for(|| fixture.lease().is_some()).await;
        fixture.runtime.sync();
        fixture.send(result_events(1));
        let key = SessionKey::new("native", "opencode", "ses_v2");
        wait_for(|| {
            fixture
                .runtime
                .store
                .native_tool_results_current(&key)
                .unwrap()
                .len()
                == 1
        })
        .await;
        fixture.runtime.sync();
        assert_eq!(
            fixture.runtime.run(100_000).outcome,
            antiburn_checks::engine::RunOutcome::Accepted
        );
        match correction {
            "delete" => {
                fixture
                    .database
                    .execute("DELETE FROM session_message WHERE id = 'step-1'", [])
                    .unwrap();
            }
            "tool-content" => {
                fixture.database.execute(
                "UPDATE session_message SET data = json_set(data, '$.content[0].state.content[0].text', 'changed retained output') WHERE id = 'step-1'", [],
            ).unwrap();
            }
            "request-content" => {
                fixture.database.execute(
                "UPDATE session_message SET data = json_set(data, '$.text', 'Changed task') WHERE id = 'request-1'", [],
            ).unwrap();
            }
            _ => unreachable!(),
        }
        let old = fixture.lease().unwrap();
        fixture.register("replacement", "synthetic-secret");
        wait_for(|| fixture.lease().is_some_and(|epoch| epoch != old)).await;
        let retired = fixture
            .runtime
            .store
            .native_tool_results_current(&key)
            .unwrap();
        assert_eq!(retired.len(), 1, "{correction}");
        assert!(retired[0].removed, "{correction}");
        fixture.runtime.sync();
        fixture.runtime.assert_idle(200_000);
        assert_eq!(fixture.runtime.evaluator_calls(), 1, "{correction}");
        assert_eq!(fixture.runtime.count("burn_check_assessment_history"), 1);
        let valid_statuses: usize = fixture
            .runtime
            .store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_current_status WHERE evidence_valid = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(valid_statuses, 0);
        fixture.send(result_events(7));
        wait_for(|| {
            fixture
                .runtime
                .store
                .native_tool_results_current(&key)
                .unwrap()
                .iter()
                .filter(|row| !row.removed)
                .count()
                == 1
        })
        .await;
        fixture.runtime.sync();
        assert_eq!(
            fixture.runtime.run(300_000).outcome,
            antiburn_checks::engine::RunOutcome::Accepted
        );
        assert_eq!(fixture.runtime.evaluator_calls(), 2);
        assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 2);
        fixture.stop(driver).await;
    }
}

#[tokio::test]
async fn native_moves_deletes_and_recovery_scope_loss_retire_exact_old_bindings() {
    for reason in ["session-moved", "session-deleted", "source-scope-changed"] {
        let fixture = Fixture::new().await;
        let driver = fixture.start(timing());
        wait_for(|| fixture.lease().is_some()).await;
        fixture.runtime.sync();
        fixture.send(result_events(1));
        let key = SessionKey::new("native", "opencode", "ses_v2");
        wait_for(|| {
            fixture
                .runtime
                .store
                .native_tool_results_current(&key)
                .unwrap()
                .len()
                == 1
        })
        .await;
        fixture.runtime.sync();
        assert_eq!(
            fixture.runtime.run(100_000).outcome,
            antiburn_checks::engine::RunOutcome::Accepted
        );
        let event = match reason {
            "session-moved" => {
                fixture
                    .database
                    .execute("UPDATE session_v2 SET directory = '/elsewhere'", [])
                    .unwrap();
                event(
                    7,
                    "session.moved",
                    json!({"location":{"directory":"/elsewhere"}, "projectID":"prj_synthetic"}),
                )
            }
            "session-deleted" => {
                fixture
                    .database
                    .execute("DELETE FROM session_v2", [])
                    .unwrap();
                event(7, "session.deleted", json!({}))
            }
            _ => {
                fixture
                    .database
                    .execute("UPDATE session_v2 SET directory = '/elsewhere'", [])
                    .unwrap();
                let mut unknown = event(
                    7,
                    "session.step.started",
                    json!({"assistantMessageID":"later"}),
                );
                unknown["durable"]["version"] = json!(99);
                unknown
            }
        };
        fixture.send(vec![event]);
        wait_for(|| {
            fixture
                .runtime
                .store
                .native_tool_results_current(&key)
                .unwrap()
                .iter()
                .all(|row| row.removed)
        })
        .await;
        assert!(
            fixture
                .runtime
                .store
                .native_tool_result_publications_after(0, 100)
                .unwrap()
                .iter()
                .any(|publication| publication.gap_reason.as_deref() == Some(reason)),
            "{reason}"
        );
        fixture.runtime.sync();
        fixture.runtime.assert_idle(200_000);
        assert_eq!(fixture.runtime.evaluator_calls(), 1, "{reason}");
        assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 1);
        fixture.stop(driver).await;
    }
}

#[tokio::test]
async fn interrupted_provider_delivery_is_not_resent_after_capture_reconnect() {
    let mut fixture = Fixture::new().await;
    let driver = fixture.start(timing());
    wait_for(|| fixture.lease().is_some()).await;
    fixture.runtime.sync();
    fixture.send(result_events(1));
    let key = SessionKey::new("native", "opencode", "ses_v2");
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&key)
            .unwrap()
            .len()
            == 1
    })
    .await;
    fixture.runtime.sync();
    fixture.runtime.abandon_dispatched(100_000);
    assert_eq!(fixture.runtime.evaluator_calls(), 1);
    assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 1);
    let old = fixture.lease().unwrap();
    fixture.register("replacement", "synthetic-secret");
    wait_for(|| fixture.lease().is_some_and(|epoch| epoch != old)).await;
    for _ in 0..2 {
        fixture.runtime.sync();
        fixture.runtime.assert_idle(200_000);
    }
    assert_eq!(fixture.runtime.evaluator_calls(), 1);
    assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 1);
    fixture.stop(driver).await;
    fixture.runtime = fixture
        .runtime
        .restart(&fixture._directory.path().join("app"));
    fixture.host.stopped.store(false, Ordering::SeqCst);
    let driver = fixture.start(timing());
    wait_for(|| fixture.lease().is_some()).await;
    fixture.runtime.sync();
    fixture.runtime.assert_idle(250_000);
    assert_eq!(fixture.runtime.evaluator_calls(), 1);
    assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 1);
    fixture.send(result_events(7));
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&key)
            .unwrap()
            .len()
            == 2
    })
    .await;
    fixture.runtime.sync();
    assert_eq!(
        fixture.runtime.run(300_000).outcome,
        antiburn_checks::engine::RunOutcome::Accepted
    );
    assert_eq!(fixture.runtime.evaluator_calls(), 2);
    assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 2);
    fixture.stop(driver).await;
}

#[tokio::test]
async fn later_v2_versions_reach_live_capture_and_publish_results() {
    for version in ["2.1.1", "2.99.0", "v2.1.1"] {
        let fixture = Fixture::new().await;
        fixture.register_version("later-v2", "synthetic-secret", version);
        let driver = fixture.start(timing());
        wait_for(|| fixture.lease().is_some()).await;
        fixture.send(result_events(1));
        let key = SessionKey::new("native", "opencode", "ses_v2");
        wait_for(|| {
            fixture
                .runtime
                .store
                .native_tool_results_current(&key)
                .unwrap()
                .len()
                == 1
        })
        .await;
        fixture.stop(driver).await;
        assert_eq!(fixture.runtime.evaluator_calls(), 0);
    }
}

#[tokio::test]
async fn non_v2_services_remain_blocked_before_baseline_admission() {
    for version in ["1.9.0", "3.0.0", "20.1.1"] {
        let fixture = Fixture::new().await;
        fixture.register_version("non-v2", "synthetic-secret", version);
        let driver = fixture.start(timing());
        wait_for(|| fixture.status().as_deref() == Some("version-unsupported")).await;
        fixture.assert_blocked_report("opencode-v2-version-unsupported");
        assert!(fixture.lease().is_none());
        assert_eq!(fixture.host.baseline_calls.load(Ordering::SeqCst), 0);
        fixture.stop(driver).await;
        assert_eq!(fixture.runtime.evaluator_calls(), 0);
    }
}

#[tokio::test]
async fn unsupported_baseline_schema_reaches_the_report_as_a_typed_version_diagnostic() {
    let fixture = Fixture::new().await;
    fixture.register_version("unsupported-schema", "synthetic-secret", "2.1.1");
    fixture
        .database
        .execute("DROP TABLE session_message", [])
        .unwrap();
    let driver = fixture.start(timing());
    wait_for(|| fixture.status().as_deref() == Some("version-unsupported")).await;
    assert!(fixture.host.baseline_calls.load(Ordering::SeqCst) > 0);
    assert!(fixture.service.event_requests.load(Ordering::SeqCst) > 0);
    fixture.assert_blocked_report("opencode-v2-version-unsupported");
    assert!(fixture.lease().is_none());
    fixture.stop(driver).await;
    assert_eq!(fixture.runtime.evaluator_calls(), 0);
    assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 0);
}

#[tokio::test]
async fn aborting_the_capture_driver_cancels_reads_and_releases_the_live_lease() {
    for hydration in [false, true] {
        let fixture = Fixture::new().await;
        fixture
            .host
            .hold_baseline
            .store(hydration, Ordering::SeqCst);
        let driver = fixture.start(timing());
        if hydration {
            wait_for(|| fixture.host.baseline_active.load(Ordering::SeqCst) > 0).await;
        } else {
            wait_for(|| fixture.lease().is_some()).await;
        }
        driver.abort();
        assert!(driver.await.unwrap_err().is_cancelled());
        wait_for(|| {
            fixture.service.open_connections.load(Ordering::SeqCst) == 0
                && fixture.host.baseline_active.load(Ordering::SeqCst) == 0
        })
        .await;
        assert!(fixture.lease().is_none());
        assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 0);
        assert_eq!(fixture.runtime.evaluator_calls(), 0);
    }
}

#[tokio::test]
async fn heartbeat_bytes_keep_idle_feed_healthy_and_dead_stream_reconnects() {
    let fixture = Fixture::new().await;
    fixture.service.heartbeat.store(true, Ordering::SeqCst);
    let mut config = timing();
    config.stream.idle_timeout = Duration::from_millis(120);
    let driver = fixture.start(config);
    wait_for(|| fixture.lease().is_some()).await;
    let epoch = fixture.lease().unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(fixture.lease().as_deref(), Some(epoch.as_str()));
    fixture.service.heartbeat.store(false, Ordering::SeqCst);
    wait_for(|| fixture.lease().is_some_and(|next| next != epoch)).await;
    assert!(fixture.service.closed_feeds.load(Ordering::SeqCst) >= 1);
    fixture.stop(driver).await;
}

#[tokio::test]
async fn idle_default_driver_keeps_one_feed_for_two_seconds_and_stops_all_requests() {
    let fixture = Fixture::new().await;
    let driver = fixture.start(CaptureTiming::default());
    wait_for(|| fixture.lease().is_some()).await;
    let epoch = fixture.lease().unwrap();
    assert_eq!(fixture.service.info_requests.load(Ordering::SeqCst), 4);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(fixture.lease().as_deref(), Some(epoch.as_str()));
    assert_eq!(fixture.service.info_requests.load(Ordering::SeqCst), 4);
    assert_eq!(fixture.service.event_requests.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.service.feeds.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.host.baseline_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.runtime.count("opencode_v2_capture"), 1);
    assert_eq!(fixture.runtime.count("native_tool_result_current"), 0);
    assert_eq!(fixture.runtime.evaluator_calls(), 0);
    fixture.stop(driver).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fixture.service.info_requests.load(Ordering::SeqCst), 4);
    assert_eq!(fixture.service.event_requests.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.service.open_connections.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.host.baseline_active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_service_retries_are_bounded_and_stop_during_backoff() {
    let fixture = Fixture::new().await;
    fixture.service.reject_info.store(true, Ordering::SeqCst);
    let driver = fixture.start(timing());
    wait_for(|| fixture.service.info_requests.load(Ordering::SeqCst) >= 2).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    let attempts = fixture.service.info_requests.load(Ordering::SeqCst);
    assert!((2..=6).contains(&attempts), "{attempts}");
    fixture.stop(driver).await;
    let attempts = fixture.service.info_requests.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        fixture.service.info_requests.load(Ordering::SeqCst),
        attempts
    );
    assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 0);
}

#[tokio::test]
async fn real_feed_pump_overflow_gaps_then_accepts_one_future_result() {
    let fixture = Fixture::new().await;
    let driver = fixture.start(timing());
    wait_for(|| fixture.lease().is_some()).await;
    fixture.runtime.sync();
    let old = fixture.lease().unwrap();
    fixture.host.hold_locate.store(true, Ordering::SeqCst);
    wait_for(|| fixture.host.locate_waiting.load(Ordering::SeqCst)).await;
    fixture.send(
        (1..=200)
            .map(|sequence| event(sequence, "session.updated", json!({"info":{"id":"ses_v2"}})))
            .collect(),
    );
    wait_for(|| fixture.service.closed_feeds.load(Ordering::SeqCst) > 0).await;
    fixture.host.hold_locate.store(false, Ordering::SeqCst);
    wait_for(|| fixture.lease().is_some_and(|epoch| epoch != old)).await;
    fixture.runtime.sync();
    assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 0);
    fixture.send(result_events(201));
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&SessionKey::new("native", "opencode", "ses_v2"))
            .unwrap()
            .len()
            == 1
    })
    .await;
    fixture.runtime.sync();
    assert_eq!(
        fixture.runtime.run(100_000).outcome,
        antiburn_checks::engine::RunOutcome::Accepted
    );
    assert_eq!(fixture.runtime.evaluator_calls(), 1);
    assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 1);
    assert_eq!(fixture.runtime.count("burn_check_assessment_history"), 1);
    fixture.stop(driver).await;
}

#[tokio::test]
async fn invalid_live_frames_and_event_versions_report_typed_failures() {
    for (frame, expected) in [
        ("data: {invalid-json}\n\n".to_owned(), "capture-gap"),
        (
            format!("data: {}\n\n", "x".repeat(256 * 1024 + 1)),
            "source-limit",
        ),
        (
            {
                let mut unknown = event(
                    1,
                    "session.step.started",
                    json!({"assistantMessageID":"step"}),
                );
                unknown["durable"]["version"] = json!(99);
                format!("data: {unknown}\n\n")
            },
            "version-unsupported",
        ),
    ] {
        let fixture = Fixture::new().await;
        let driver = fixture.start(timing());
        wait_for(|| fixture.lease().is_some() || fixture.status().is_some()).await;
        assert!(
            fixture.lease().is_some(),
            "capture lease did not start; source status: {:?}",
            fixture.status()
        );
        fixture.host.hold_baseline.store(true, Ordering::SeqCst);
        fixture.frames.send(frame).unwrap();
        wait_for_named(
            &mut || fixture.status().as_deref() == Some(expected),
            "typed status did not arrive",
        )
        .await;
        fixture.assert_blocked_report(&format!("opencode-v2-{expected}"));
        assert!(fixture.lease().is_none());
        fixture.stop(driver).await;
        assert_eq!(fixture.runtime.evaluator_calls(), 0);
        assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 0);
    }
}

#[tokio::test]
async fn capture_restart_and_expired_wake_lease_do_not_repeat_accepted_work() {
    let fixture = Fixture::new().await;
    fixture.service.heartbeat.store(true, Ordering::SeqCst);
    let driver = fixture.start(timing());
    wait_for(|| fixture.lease().is_some()).await;
    fixture.runtime.sync();
    fixture.send(result_events(1));
    let key = SessionKey::new("native", "opencode", "ses_v2");
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&key)
            .unwrap()
            .len()
            == 1
    })
    .await;
    fixture.runtime.sync();
    assert_eq!(
        fixture.runtime.run(100_000).outcome,
        antiburn_checks::engine::RunOutcome::Accepted
    );
    let first = fixture.lease().unwrap();
    driver.abort();
    assert!(driver.await.unwrap_err().is_cancelled());
    wait_for(|| fixture.service.open_feeds.load(Ordering::SeqCst) == 0).await;
    let driver = fixture.start(timing());
    wait_for(|| fixture.lease().is_some_and(|epoch| epoch != first)).await;
    fixture.runtime.sync();
    fixture.runtime.assert_idle(200_000);
    let restarted = fixture.lease().unwrap();
    // A suspended process can resume after its wall-clock lease expires.
    fixture
        .runtime
        .store
        .lock()
        .execute(
            "UPDATE opencode_v2_capture SET lease_expires_at_epoch = ?1",
            [unix_now() - 1],
        )
        .unwrap();
    wait_for(|| fixture.lease().is_some_and(|epoch| epoch != restarted)).await;
    fixture.runtime.sync();
    fixture.runtime.assert_idle(300_000);
    assert_eq!(fixture.runtime.evaluator_calls(), 1);
    assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 1);
    fixture.send(result_events(7));
    wait_for(|| {
        fixture
            .runtime
            .store
            .native_tool_results_current(&key)
            .unwrap()
            .len()
            == 2
    })
    .await;
    fixture.runtime.sync();
    assert_eq!(
        fixture.runtime.run(400_000).outcome,
        antiburn_checks::engine::RunOutcome::Accepted
    );
    assert_eq!(fixture.runtime.evaluator_calls(), 2);
    assert_eq!(fixture.runtime.count("burn_check_evaluation_attempt"), 2);
    assert_eq!(fixture.runtime.count("burn_check_assessment_history"), 2);
    fixture.stop(driver).await;
}

#[tokio::test]
async fn delete_retention_and_clear_stop_capture_without_recreating_local_payloads() {
    for operation in ["delete", "retention", "clear"] {
        let fixture = Fixture::new().await;
        let driver = fixture.start(timing());
        wait_for(|| fixture.lease().is_some()).await;
        fixture.runtime.sync();
        fixture.send(result_events(1));
        let key = SessionKey::new("native", "opencode", "ses_v2");
        wait_for(|| {
            fixture
                .runtime
                .store
                .native_tool_results_current(&key)
                .unwrap()
                .len()
                == 1
        })
        .await;
        fixture.runtime.sync();
        assert_eq!(
            fixture.runtime.run(100_000).outcome,
            antiburn_checks::engine::RunOutcome::Accepted
        );
        let source_before = std::fs::read(&fixture.host.database).unwrap();
        let wal = fixture.host.database.with_extension("db-wal");
        let wal_before = std::fs::read(&wal).unwrap();
        match operation {
            "delete" => assert!(
                fixture
                    .runtime
                    .store
                    .delete_session(&key)
                    .unwrap()
                    .is_some()
            ),
            "retention" => {
                let mut settings = fixture.runtime.store.settings().unwrap();
                settings.session_data_retention_days =
                    crate::store::model::SESSION_DATA_RETENTION_DAYS_30;
                fixture.runtime.store.replace_settings(&settings).unwrap();
                fixture
                    .runtime
                    .store
                    .lock()
                    .execute("UPDATE session SET updated_at_epoch = 0", [])
                    .unwrap();
                assert_eq!(
                    fixture
                        .runtime
                        .store
                        .apply_session_retention(unix_now())
                        .unwrap()
                        .0,
                    1
                );
            }
            "clear" => assert_eq!(
                fixture.runtime.store.clear_local_session_data().unwrap().0,
                1
            ),
            _ => unreachable!(),
        }
        let _ = fixture.frames.send(
            result_events(7)
                .into_iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect(),
        );
        wait_for(|| {
            fixture.lease().is_none() && fixture.service.open_feeds.load(Ordering::SeqCst) == 0
        })
        .await;
        let requests = fixture.service.info_requests.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            fixture.service.info_requests.load(Ordering::SeqCst),
            requests,
            "{operation}"
        );
        for table in [
            "opencode_v2_capture",
            "opencode_v2_source_status",
            "native_tool_result_current",
            "native_tool_result_publication",
            "burn_check_candidate",
            "burn_check_assessment_history",
        ] {
            assert_eq!(fixture.runtime.count(table), 0, "{operation}: {table}");
        }
        assert_eq!(fixture.runtime.evaluator_calls(), 1, "{operation}");
        fixture.stop(driver).await;
        assert_eq!(
            std::fs::read(&fixture.host.database).unwrap(),
            source_before
        );
        assert_eq!(std::fs::read(&wal).unwrap(), wal_before);
    }
}
