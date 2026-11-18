use super::*;
use crate::cloud::{
    AuthStatus,
    auth::{
        Config, Session, TokenRequest, Tokens, Transport as AuthTransport, Workspace, WorkspaceKind,
    },
    state::StateStore,
};
use antiburn_cloud_capture::{
    CaptureIdentity, CaptureManifest, Chunk, ChunkDisposition, ChunkKey, PartReceipt, PlannedChunk,
    ServerStatus, Transport, TransportError, UploadPlan,
};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize},
    },
    thread::JoinHandle,
};

const READY: usize = 0;
const CAPS_MISSING: usize = 1;
const CAPS_OFFLINE: usize = 2;
const SUPPRESSED: usize = 3;
const CONFLICT: usize = 4;
const RESERVE_OFFLINE: usize = 5;
const CAPACITY_ONE: usize = 6;
const RESERVE_CAPACITY: usize = 7;
const VERIFY_PENDING: usize = 8;
const UNKNOWN_CAPACITY: usize = 9;

struct Service {
    origin: String,
    mode: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    reserves: Arc<AtomicUsize>,
    capacity_rejections: Arc<AtomicUsize>,
    reserve_identities: Arc<Mutex<Vec<CaptureIdentity>>>,
    reserve_database: Arc<Mutex<Option<std::path::PathBuf>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Service {
    fn new(mode: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mode = Arc::new(AtomicUsize::new(mode));
        let requests = Arc::new(AtomicUsize::new(0));
        let reserves = Arc::new(AtomicUsize::new(0));
        let capacity_rejections = Arc::new(AtomicUsize::new(0));
        let reserve_identities = Arc::new(Mutex::new(Vec::new()));
        let reserve_database = Arc::new(Mutex::new(None::<std::path::PathBuf>));
        let stop = Arc::new(AtomicBool::new(false));
        let (worker_mode, worker_requests, worker_reserves, worker_stop) = (
            mode.clone(),
            requests.clone(),
            reserves.clone(),
            stop.clone(),
        );
        let service_origin = origin.clone();
        let worker_capacity_rejections = capacity_rejections.clone();
        let worker_reserve_identities = reserve_identities.clone();
        let worker_reserve_database = reserve_database.clone();
        let worker = std::thread::spawn(move || {
            let mut manifest: Option<CaptureManifest> = None;
            let mut receipts = Vec::<PartReceipt>::new();
            let mut pending_key: Option<ChunkKey> = None;
            let mut finalized = false;
            let mut active = false;
            while !worker_stop.load(Ordering::Acquire) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("test service accept failed: {:?}", error.kind()),
                };
                let (line, headers, body) = read_request(&mut stream);
                worker_requests.fetch_add(1, Ordering::AcqRel);
                let mode = worker_mode.load(Ordering::Acquire);
                let object = line.starts_with("PUT /object?");
                assert_eq!(headers.contains_key("authorization"), !object);
                assert!(!headers.contains_key("cookie"));
                assert!(!headers.contains_key("origin"));
                let input: serde_json::Value = if body.is_empty() || object {
                    serde_json::Value::Null
                } else {
                    serde_json::from_slice(&body).unwrap()
                };
                let (status, output) = if line.contains("/capabilities ") {
                    if mode == CAPS_OFFLINE {
                        continue;
                    }
                    if mode == CAPS_MISSING {
                        (404, serde_json::json!({}))
                    } else {
                        (
                            200,
                            serde_json::json!({
                                "enabled": true, "max_expanded_bytes": 256 * 1024 * 1024,
                                "chunk_expanded_bytes": 8 * 1024 * 1024,
                                "source_formats": ["ClaudeJsonl", "OpenCodeSqliteV2"],
                            }),
                        )
                    }
                } else if line.contains("/reserve ") {
                    worker_reserves.fetch_add(1, Ordering::AcqRel);
                    let identity: CaptureIdentity =
                        serde_json::from_value(input["identity"].clone()).unwrap();
                    if let Some(path) = worker_reserve_database.lock().unwrap().as_ref() {
                        let connection = rusqlite::Connection::open_with_flags(
                            path,
                            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
                        )
                        .unwrap();
                        let (state, retry_at): (String, Option<i64>) = connection
                            .query_row(
                                "SELECT json_extract(record_json,'$.state'),
                                    json_extract(record_json,'$.reservation_retry_at')
                                 FROM cloud_sync_job WHERE capture_id=?1",
                                [&identity.capture_id],
                                |row| Ok((row.get(0)?, row.get(1)?)),
                            )
                            .unwrap();
                        assert_eq!(state, "pending");
                        assert!(retry_at.is_none());
                    }
                    worker_reserve_identities.lock().unwrap().push(identity);
                    if mode == RESERVE_OFFLINE {
                        continue;
                    }
                    if mode == UNKNOWN_CAPACITY {
                        (
                            429,
                            serde_json::json!({"error": {
                                "code":"unknown_limit", "retryable":true,
                                "message":"Synthetic limit.",
                            }}),
                        )
                    } else if mode == RESERVE_CAPACITY || (mode == CAPACITY_ONE && active) {
                        worker_capacity_rejections.fetch_add(1, Ordering::AcqRel);
                        (
                            429,
                            serde_json::json!({"error": {
                                "code": "capture_limit", "retryable": true,
                                "message": "Capture capacity is full.",
                            }}),
                        )
                    } else if mode == SUPPRESSED
                        && input["identity"]["source_id"]
                            .as_str()
                            .unwrap()
                            .ends_with("/session")
                    {
                        (
                            409,
                            serde_json::json!({"error": {
                                "code": "source_suppressed", "retryable": false,
                                "message": "Cloud deletion prevents automatic reimport.",
                            }}),
                        )
                    } else if mode == CONFLICT {
                        (
                            409,
                            serde_json::json!({"error": {"code": "capture_conflict", "retryable": false, "message": "Conflict."}}),
                        )
                    } else {
                        manifest = None;
                        receipts.clear();
                        finalized = false;
                        active = true;
                        (200, serde_json::json!({"reservation_id": "reservation"}))
                    }
                } else if line.contains("/manifest ") {
                    let incoming: CaptureManifest =
                        serde_json::from_value(input["manifest"].clone()).unwrap();
                    if let Some(saved) = &manifest {
                        assert_eq!(saved, &incoming);
                    }
                    let plan = UploadPlan {
                        identity: incoming.identity.clone(),
                        manifest_sha256: incoming.sha256().unwrap(),
                        plan_id: "plan".into(),
                        chunks: incoming
                            .chunks()
                            .map(|(key, chunk)| PlannedChunk {
                                key,
                                compressed_sha256: chunk.compressed_sha256.clone(),
                                disposition: ChunkDisposition::Upload,
                            })
                            .collect(),
                        required_transfer_bytes: incoming
                            .chunks()
                            .map(|(_, chunk)| chunk.compressed_bytes)
                            .sum(),
                        reused_bytes: 0,
                    };
                    manifest = Some(incoming);
                    (200, serde_json::to_value(plan).unwrap())
                } else if line.starts_with("GET ") && line.contains("/status?") {
                    let saved = manifest.as_ref().unwrap();
                    if finalized && mode != VERIFY_PENDING {
                        active = false;
                    }
                    (
                        200,
                        serde_json::to_value(ServerStatus {
                            identity: saved.identity.clone(),
                            manifest_sha256: saved.sha256().unwrap(),
                            plan_id: "plan".into(),
                            uploaded_parts: receipts.clone(),
                            integrity_accepted: finalized && mode != VERIFY_PENDING,
                        })
                        .unwrap(),
                    )
                } else if line.contains("/parts ") {
                    let key: ChunkKey = serde_json::from_value(input["key"].clone()).unwrap();
                    let chunk = manifest.as_ref().unwrap().chunk(&key).unwrap();
                    pending_key = Some(key);
                    (
                        200,
                        serde_json::json!({
                            "url": format!("{service_origin}/object?partNumber=1&uploadId=synthetic"),
                            "part_number": 1, "headers": {"content-length": chunk.compressed_bytes.to_string()},
                        }),
                    )
                } else if object {
                    use sha2::{Digest, Sha256};
                    let key = pending_key.take().unwrap();
                    let chunk = manifest.as_ref().unwrap().chunk(&key).unwrap();
                    let digest = Sha256::digest(&body)
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>();
                    assert_eq!(body.len() as u64, chunk.compressed_bytes);
                    assert_eq!(digest, chunk.compressed_sha256);
                    receipts.push(PartReceipt {
                        key,
                        compressed_sha256: digest,
                    });
                    (200, serde_json::Value::Null)
                } else if line.contains("/finalize ") {
                    assert_eq!(receipts.len(), manifest.as_ref().unwrap().chunks().count());
                    finalized = true;
                    (202, serde_json::Value::Null)
                } else {
                    panic!("unexpected test service route");
                };
                let output = output.to_string();
                write!(stream, "HTTP/1.1 {status} Synthetic\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{output}", output.len()).unwrap();
            }
        });
        Self {
            origin,
            mode,
            requests,
            reserves,
            capacity_rejections,
            reserve_identities,
            reserve_database,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> (String, BTreeMap<String, String>, Vec<u8>) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        assert!(bytes.len() < 32_768);
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
    }
    let text = String::from_utf8(bytes).unwrap();
    let mut lines = text.split("\r\n");
    let line = lines.next().unwrap().into();
    let headers: BTreeMap<_, _> = lines
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (name, value) = line.split_once(':').unwrap();
            (name.to_ascii_lowercase(), value.trim().to_owned())
        })
        .collect();
    let length = headers
        .get("content-length")
        .map_or(0, |value| value.parse().unwrap());
    assert!(length < 65_536);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).unwrap();
    (line, headers, body)
}

struct Authority {
    origin: String,
    deadline: i64,
    content_generation: Arc<AtomicU64>,
    pause_stage: usize,
    exchanges: AtomicUsize,
    sessions: AtomicUsize,
    revocations: AtomicUsize,
}

impl Authority {
    fn session(&self) -> Session {
        Session {
            user: auth::User {
                id: "account".into(),
                display_name: "Example".into(),
            },
            grant_id: "grant".into(),
            device_name: "Example desktop".into(),
            workspace: Workspace {
                id: "personal".into(),
                name: "Personal".into(),
                kind: WorkspaceKind::Personal,
            },
            resource: self.origin.clone(),
            scope: "capture:write".into(),
            iss: self.origin.clone(),
            expires_at: self.deadline,
            access_expires_at: now() + 900,
        }
    }
}

impl AuthTransport for Arc<Authority> {
    fn exchange(&self, _: &Config, _: &TokenRequest<'_>) -> Result<Tokens, auth::Failure> {
        let index = self.exchanges.fetch_add(1, Ordering::AcqRel);
        if index == 1 && self.pause_stage == 1 {
            self.content_generation.fetch_add(1, Ordering::AcqRel);
        }
        Ok(Tokens {
            access_token: format!("synthetic-access-{index}"),
            refresh_token: format!("synthetic-refresh-{index}"),
            expires_in: if index == 0 && self.pause_stage != 0 {
                30
            } else {
                900
            },
            token_type: "Bearer".into(),
            grant_id: "grant".into(),
            user_id: "account".into(),
            workspace_id: "personal".into(),
            grant_expires_at: time::OffsetDateTime::from_unix_timestamp(self.deadline)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
            scope: "capture:write".into(),
            resource: self.origin.clone(),
            iss: self.origin.clone(),
        })
    }
    fn session(&self, _: &Config, _: &str) -> Result<Session, auth::Failure> {
        let index = self.sessions.fetch_add(1, Ordering::AcqRel);
        if index == 1 && self.pause_stage == 2 {
            self.content_generation.fetch_add(1, Ordering::AcqRel);
        }
        Ok(Authority::session(self))
    }
    fn revoke(&self, _: &Config, _: &str) -> Result<(), auth::Failure> {
        self.revocations.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    store: Store,
    policy: Policy,
    engine: Engine,
    host: capture::Host,
    runner: Runner,
    cache: super::super::projects::Cache,
    service: Service,
    content_generation: Arc<AtomicU64>,
    authority: Arc<Authority>,
    root: std::path::PathBuf,
    project: std::path::PathBuf,
}

impl Fixture {
    fn new(mode: usize, pause_stage: usize) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let root = base.join("desktop-sources");
        let project = base.join("project");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&project).unwrap();
        write_source(&root, &project, "session", "first");
        let service = Service::new(mode);
        let store = Store::open(&base).unwrap();
        store.initialize_cloud_setup().unwrap();
        let policy = store
            .save_cloud_sync_policy(
                "account",
                "personal",
                &service.origin,
                &[SourceSelection {
                    source_kind: SourceKind::ClaudeJsonl,
                    path: root.to_str().unwrap().into(),
                }],
                &[ProjectSelection {
                    source_kind: SourceKind::ClaudeJsonl,
                    source_path: root.to_str().unwrap().into(),
                    project_path: project.to_str().unwrap().into(),
                }],
                &BTreeMap::new(),
            )
            .unwrap();
        let content_generation = Arc::new(AtomicU64::new(0));
        let authority = Arc::new(Authority {
            origin: service.origin.clone(),
            deadline: now() + 86_400,
            content_generation: content_generation.clone(),
            pause_stage,
            exchanges: AtomicUsize::new(0),
            sessions: AtomicUsize::new(0),
            revocations: AtomicUsize::new(0),
        });
        let vault = Arc::new(crate::cloud::tests::MemoryVault::default());
        let mut engine = Engine::new(
            Config::parse(&service.origin, &service.origin),
            Box::new(vault),
            Box::new(authority.clone()),
            StateStore::new(&base),
        );
        let (_, _listener, attempt) = engine.begin(now(), 0).unwrap();
        engine
            .callback(
                &attempt,
                auth::Callback::Code("synthetic".into()),
                now(),
                || true,
            )
            .unwrap();
        Self {
            _directory: directory,
            store,
            policy,
            engine,
            host: capture::Host::new(&base),
            runner: Runner::default(),
            cache: super::super::projects::Cache::default(),
            service,
            content_generation,
            authority,
            root,
            project,
        }
    }

    fn turn(&mut self) {
        let content_generation = self.content_generation.load(Ordering::Acquire);
        let current = || self.content_generation.load(Ordering::Acquire) == content_generation;
        self.runner
            .turn(
                &self.store,
                &mut self.host,
                &mut self.engine,
                &current,
                &|| true,
                &mut self.cache,
            )
            .unwrap();
    }

    fn turn_published(&mut self, observed: &mut Vec<Status>) {
        let content_generation = self.content_generation.load(Ordering::Acquire);
        let current = || self.content_generation.load(Ordering::Acquire) == content_generation;
        let failed = self
            .runner
            .turn(
                &self.store,
                &mut self.host,
                &mut self.engine,
                &current,
                &|| true,
                &mut self.cache,
            )
            .is_err();
        if current() {
            self.runner.report_status(
                &self.store,
                &mut self.host,
                &self.engine,
                failed,
                &|| true,
                |value| observed.push(value),
            );
        }
    }

    fn accept_first(&mut self) {
        for _ in 0..10 {
            self.turn();
            if self
                .store
                .cloud_sync_outcome_counts("account", "personal", &self.service.origin)
                .unwrap()
                .0
                == 1
            {
                return;
            }
        }
        panic!("test capture did not become accepted");
    }

    /// Advance only synthetic capacity deadlines without sleeping or changing IDs.
    fn allow_capacity_retry(&mut self) {
        for job in self
            .store
            .cloud_sync_jobs("account", "personal", &self.service.origin)
            .unwrap()
        {
            if job.state == JobState::WaitingForCapacity {
                self.store
                    .defer_cloud_sync_reservation(&job, now() - 1)
                    .unwrap();
            }
        }
        self.runner.next_reservation = None;
    }

    fn restart_scheduler(&mut self) {
        let base = self._directory.path().canonicalize().unwrap();
        self.runner = Runner::default();
        self.cache = super::super::projects::Cache::default();
        self.host = capture::Host::new(&base);
        self.store = Store::open(&base).unwrap();
    }

    /// Select only the final synthetic OpenCode ID; earlier IDs stay unapproved.
    fn select_opencode(&mut self, sessions: usize) -> std::path::PathBuf {
        let base = self._directory.path().canonicalize().unwrap();
        let database = base.join("opencode.db");
        let outside = base.join("unselected-project");
        fs::create_dir(&outside).unwrap();
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session_v2(id TEXT PRIMARY KEY, directory TEXT);
                 CREATE TABLE session_message(
                    id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
                    seq INTEGER NOT NULL, data TEXT NOT NULL, UNIQUE(session_id,seq));",
            )
            .unwrap();
        for index in 0..sessions {
            connection
                .execute(
                    "INSERT INTO session_v2 VALUES (?1, ?2)",
                    rusqlite::params![
                        format!("session-{index:04}"),
                        if index + 1 == sessions {
                            &self.project
                        } else {
                            &outside
                        }
                        .to_str()
                        .unwrap(),
                    ],
                )
                .unwrap();
        }
        drop(connection);
        self.policy = self
            .store
            .save_cloud_sync_policy(
                "account",
                "personal",
                &self.service.origin,
                &[SourceSelection {
                    source_kind: SourceKind::OpenCodeSqliteV2,
                    path: database.to_str().unwrap().into(),
                }],
                &[ProjectSelection {
                    source_kind: SourceKind::OpenCodeSqliteV2,
                    source_path: database.to_str().unwrap().into(),
                    project_path: self.project.to_str().unwrap().into(),
                }],
                &BTreeMap::new(),
            )
            .unwrap();
        database
    }
}

fn write_source(root: &Path, project: &Path, id: &str, message: &str) {
    fs::write(
        root.join(format!("{id}.jsonl")),
        format!(
            "{}\n",
            serde_json::json!({
                "sessionId": id, "cwd": project, "version": "2.1.55", "type": "user",
                "message": {"role": "user", "content": message},
            })
        ),
    )
    .unwrap();
}

#[test]
fn cloud_scheduler_pause_during_refresh_preserves_rotated_credentials_and_stops_capture() {
    for stage in [1, 2] {
        let mut fixture = Fixture::new(READY, stage);
        fixture.turn();
        assert_eq!(fixture.content_generation.load(Ordering::Acquire), 1);
        assert!(fixture.engine.status == AuthStatus::SignedIn);
        assert!(fixture.engine.session().is_some());
        assert_eq!(
            fixture.engine.vault.read().unwrap().unwrap().refresh_token,
            "synthetic-refresh-1"
        );
        assert_eq!(fixture.authority.revocations.load(Ordering::Acquire), 0);
        assert_eq!(fixture.service.requests.load(Ordering::Acquire), 0);
        fixture
            .store
            .set_cloud_sync_paused(&fixture.policy, true)
            .unwrap();
        fixture.turn();
        assert_eq!(fixture.runner.state, State::Paused);
        assert_eq!(fixture.service.requests.load(Ordering::Acquire), 0);
    }
}

#[test]
fn cloud_scheduler_cache_expiry_does_not_recapture_unchanged_sources_or_replace_pending_bytes() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.accept_first();
    let first_reservations = fixture.service.reserves.load(Ordering::Acquire);
    fixture.cache.expire_evidence();
    fixture.runner.next_reconcile = None;
    fixture.turn();
    assert_eq!(
        fixture.service.reserves.load(Ordering::Acquire),
        first_reservations
    );
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row("SELECT count(*) FROM cloud_sync_job", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
    write_source(
        &fixture.root,
        &fixture.project,
        "session",
        "a changed source",
    );
    fixture.runner.next_reconcile = None;
    fixture.turn();
    assert_eq!(
        fixture.service.reserves.load(Ordering::Acquire),
        first_reservations + 1
    );
    let pending = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    write_source(
        &fixture.root,
        &fixture.project,
        "session",
        "another source generation",
    );
    fixture.cache.expire_evidence();
    fixture.runner.next_reconcile = None;
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(saved.identity, pending.identity);
    assert_eq!(saved.manifest, pending.manifest);
    assert_eq!(
        fixture.service.reserves.load(Ordering::Acquire),
        first_reservations + 1
    );
}

#[test]
fn cloud_scheduler_drains_each_capture_before_admitting_a_backfill_larger_than_service_capacity() {
    let mut fixture = Fixture::new(CAPACITY_ONE, 0);
    for index in 0..17 {
        write_source(
            &fixture.root,
            &fixture.project,
            &format!("backfill-{index:02}"),
            "synthetic backfill",
        );
    }
    for _ in 0..160 {
        fixture.turn();
        if fixture
            .store
            .cloud_sync_outcome_counts("account", "personal", &fixture.service.origin)
            .unwrap()
            .0
            == 18
        {
            break;
        }
    }
    assert_eq!(
        fixture
            .store
            .cloud_sync_outcome_counts("account", "personal", &fixture.service.origin)
            .unwrap()
            .0,
        18
    );
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 18);
    assert_eq!(
        fixture.service.capacity_rejections.load(Ordering::Acquire),
        0
    );
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
}

#[test]
fn cloud_scheduler_reservation_capacity_is_backpressure_not_a_permanent_fence() {
    let mut fixture = Fixture::new(RESERVE_CAPACITY, 0);
    *fixture.service.reserve_database.lock().unwrap() =
        Some(fixture._directory.path().join("antiburn-debug.sqlite3"));
    fixture.turn();
    assert_eq!(
        fixture.service.capacity_rejections.load(Ordering::Acquire),
        1
    );
    let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert!(entry.reservation_id.is_none());
    assert!(entry.manifest.is_none());
    assert_eq!(fixture.runner.state, State::Syncing);
    let job = fixture
        .store
        .cloud_sync_job(&entry.identity.capture_id)
        .unwrap()
        .unwrap();
    assert_eq!(job.state, JobState::WaitingForCapacity);
    assert!(fixture.runner.next_reservation.unwrap() > Instant::now());
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    fixture.runner = Runner::default();
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.allow_capacity_retry();
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(saved.identity, entry.identity);
    assert_eq!(saved.state, CaptureState::Queued);
    assert_eq!(
        *fixture.service.reserve_identities.lock().unwrap(),
        vec![entry.identity.clone(), entry.identity.clone()]
    );
    fixture.accept_first();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 2);
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
}

#[test]
fn cloud_scheduler_does_not_infer_capacity_retry_from_unknown_old_errors() {
    for mode in [RESERVE_OFFLINE, UNKNOWN_CAPACITY] {
        let mut fixture = Fixture::new(mode, 0);
        fixture.turn();
        let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        let before = fs::read(
            fixture
                ._directory
                .path()
                .join("cloud-capture-v1")
                .join(&entry.identity.capture_id)
                .join("record.json"),
        )
        .unwrap();
        assert_eq!(
            fixture
                .store
                .cloud_sync_job(&entry.identity.capture_id)
                .unwrap()
                .unwrap()
                .state,
            JobState::Interrupted
        );
        fixture.service.mode.store(READY, Ordering::Release);
        fixture.runner.next_reconcile = None;
        fixture.turn();
        assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
        assert_eq!(
            fs::read(
                fixture
                    ._directory
                    .path()
                    .join("cloud-capture-v1")
                    .join(&entry.identity.capture_id)
                    .join("record.json"),
            )
            .unwrap(),
            before
        );
    }
}

#[test]
fn cloud_scheduler_capacity_retry_after_restart_rechecks_the_source_observation_and_current_project()
 {
    for change_cwd in [false, true] {
        let mut fixture = Fixture::new(RESERVE_CAPACITY, 0);
        fixture.turn();
        let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        let directory = fixture
            ._directory
            .path()
            .join("cloud-capture-v1")
            .join(&entry.identity.capture_id);
        let before = saved_files(&directory);
        let outside = fixture
            ._directory
            .path()
            .canonicalize()
            .unwrap()
            .join("outside");
        fs::create_dir(&outside).unwrap();
        write_source(
            &fixture.root,
            if change_cwd {
                &outside
            } else {
                &fixture.project
            },
            "session",
            "changed source",
        );
        fixture.service.mode.store(READY, Ordering::Release);
        fixture.allow_capacity_retry();
        fixture.restart_scheduler();
        fixture.turn();
        assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
        let kept = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        assert_eq!(kept, entry);
        assert_eq!(saved_files(&directory), before);
        assert_eq!(
            fixture
                .store
                .cloud_sync_job(&entry.identity.capture_id)
                .unwrap()
                .unwrap()
                .state,
            JobState::Interrupted
        );
    }
}

#[test]
fn cloud_scheduler_capacity_retry_after_restart_interrupts_a_missing_native_id_without_changing_bytes()
 {
    for opencode in [false, true] {
        let mut fixture = Fixture::new(RESERVE_CAPACITY, 0);
        let database = opencode.then(|| fixture.select_opencode(1));
        fixture.turn();
        let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        let directory = fixture
            ._directory
            .path()
            .join("cloud-capture-v1")
            .join(&entry.identity.capture_id);
        let before = saved_files(&directory);
        if let Some(database) = database {
            rusqlite::Connection::open(database)
                .unwrap()
                .execute("DELETE FROM session_v2 WHERE id='session-0000'", [])
                .unwrap();
        } else {
            fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
        }
        fixture.service.mode.store(READY, Ordering::Release);
        fixture.allow_capacity_retry();
        fixture.restart_scheduler();
        fixture.turn();
        assert_eq!(fixture.runner.state, State::Interrupted);
        let job = fixture
            .store
            .cloud_sync_job(&entry.identity.capture_id)
            .unwrap()
            .unwrap();
        assert_eq!(job.identity, entry.identity);
        assert_eq!(job.state, JobState::Interrupted);
        assert!(job.reservation_retry_at.is_none());
        assert_eq!(
            fixture.host.spool().unwrap().entries().unwrap(),
            vec![entry]
        );
        assert_eq!(saved_files(&directory), before);
        assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    }
}

#[test]
fn cloud_scheduler_capacity_retry_after_restart_does_not_treat_a_later_page_id_as_missing() {
    let mut fixture = Fixture::new(RESERVE_CAPACITY, 0);
    fixture.select_opencode(1025);
    for _ in 0..3 {
        fixture.turn();
    }
    let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.allow_capacity_retry();
    fixture.restart_scheduler();
    fixture.turn();
    let kept = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(kept.identity, entry.identity);
    assert_eq!(kept.state, CaptureState::Queued);
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 2);
}

#[test]
fn cloud_scheduler_capacity_retry_cannot_reuse_a_narrowed_policy() {
    let mut fixture = Fixture::new(RESERVE_CAPACITY, 0);
    let extra = fixture
        ._directory
        .path()
        .canonicalize()
        .unwrap()
        .join("extra");
    fs::create_dir(&extra).unwrap();
    let root = SourceSelection {
        source_kind: SourceKind::ClaudeJsonl,
        path: fixture.root.to_str().unwrap().into(),
    };
    let selected = ProjectSelection {
        source_kind: SourceKind::ClaudeJsonl,
        source_path: root.path.clone(),
        project_path: fixture.project.to_str().unwrap().into(),
    };
    let mut projects = vec![
        selected.clone(),
        ProjectSelection {
            project_path: extra.to_str().unwrap().into(),
            ..selected.clone()
        },
    ];
    projects.sort();
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            std::slice::from_ref(&root),
            &projects,
            &BTreeMap::new(),
        )
        .unwrap();
    let approved = fixture.policy.clone();
    fixture.turn();
    let before = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            &[root],
            &[selected],
            &BTreeMap::new(),
        )
        .unwrap();
    fixture.allow_capacity_retry();
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    assert_eq!(
        fixture.host.spool().unwrap().entries().unwrap().remove(0),
        before
    );
    let job = fixture
        .store
        .cloud_sync_job(&before.identity.capture_id)
        .unwrap()
        .unwrap();
    assert_eq!(job.state, JobState::Fenced);
    assert!(job.reservation_retry_at.is_none());
    let roots = approved
        .roots
        .iter()
        .map(|root| SourceSelection {
            source_kind: root.source_kind,
            path: root.path.clone(),
        })
        .collect::<Vec<_>>();
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            &roots,
            &approved.projects,
            &BTreeMap::new(),
        )
        .unwrap();
    retry_saved_jobs(
        &fixture.store,
        fixture.host.spool().unwrap(),
        &fixture.policy,
        &|| true,
    )
    .unwrap();
    fixture.runner.next_reconcile = None;
    fixture.turn();
    let retried = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_ne!(retried.identity.capture_id, before.identity.capture_id);
    assert_eq!(retried.identity.source_id, before.identity.source_id);
}

#[test]
fn cloud_scheduler_retains_bytes_and_defers_new_capture_until_integrity_acceptance() {
    let mut fixture = Fixture::new(VERIFY_PENDING, 0);
    write_source(&fixture.root, &fixture.project, "another", "synthetic");
    for _ in 0..8 {
        fixture.turn();
    }
    let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(entry.state, CaptureState::AwaitingIntegrity);
    assert!(entry.completion_submitted);
    assert!(!entry.server_status.as_ref().unwrap().integrity_accepted);
    let directory = fixture
        ._directory
        .path()
        .join("cloud-capture-v1")
        .join(&entry.identity.capture_id);
    let before = saved_chunks(&directory);
    assert!(!before.is_empty());
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    fixture.turn();
    assert_eq!(saved_chunks(&directory), before);
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.turn();
    assert!(!directory.exists());
    assert_eq!(
        fixture
            .store
            .cloud_sync_outcome_counts("account", "personal", &fixture.service.origin)
            .unwrap()
            .0,
        1
    );
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 2);
}

#[test]
fn cloud_scheduler_captures_a_selected_opencode_session_after_two_unselected_pages_without_duplicates()
 {
    let mut fixture = Fixture::new(READY, 0);
    let database = fixture
        ._directory
        .path()
        .canonicalize()
        .unwrap()
        .join("opencode.db");
    let outside = fixture
        ._directory
        .path()
        .canonicalize()
        .unwrap()
        .join("outside");
    fs::create_dir(&outside).unwrap();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session_v2(id TEXT PRIMARY KEY, directory TEXT);
         CREATE TABLE session_message(
            id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
            seq INTEGER NOT NULL, data TEXT NOT NULL, UNIQUE(session_id,seq));",
        )
        .unwrap();
    connection
        .execute(
            "WITH RECURSIVE sessions(n) AS (
            VALUES(0) UNION ALL SELECT n+1 FROM sessions WHERE n < 1024
         )
         INSERT INTO session_v2 SELECT printf('session-%04d',n), ?1 FROM sessions",
            [outside.to_str().unwrap()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory=?1 WHERE id='session-1024'",
            [fixture.project.to_str().unwrap()],
        )
        .unwrap();
    drop(connection);
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            &[SourceSelection {
                source_kind: SourceKind::OpenCodeSqliteV2,
                path: database.to_str().unwrap().into(),
            }],
            &[ProjectSelection {
                source_kind: SourceKind::OpenCodeSqliteV2,
                source_path: database.to_str().unwrap().into(),
                project_path: fixture.project.to_str().unwrap().into(),
            }],
            &BTreeMap::new(),
        )
        .unwrap();
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 0);
    assert_eq!(
        fixture
            .runner
            .discovery_cursor
            .as_ref()
            .unwrap()
            .after
            .as_deref(),
        Some("session-0511")
    );
    fixture.policy = fixture
        .store
        .set_cloud_sync_paused(&fixture.policy, true)
        .unwrap();
    fixture.turn();
    assert!(fixture.runner.discovery_cursor.is_none());
    assert!(fixture.runner.next_reconcile.is_none());
    fixture.policy = fixture
        .store
        .set_cloud_sync_paused(&fixture.policy, false)
        .unwrap();
    fixture.turn();
    assert_eq!(
        fixture
            .runner
            .discovery_cursor
            .as_ref()
            .unwrap()
            .after
            .as_deref(),
        Some("session-0511")
    );
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 0);
    assert_eq!(
        fixture
            .runner
            .discovery_cursor
            .as_ref()
            .unwrap()
            .after
            .as_deref(),
        Some("session-1023")
    );
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    assert!(fixture.runner.discovery_cursor.is_none());
    fixture.accept_first();
    fixture.cache.expire_evidence();
    fixture.runner.next_reconcile = None;
    for _ in 0..4 {
        fixture.turn();
    }
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row("SELECT count(*) FROM cloud_sync_job", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
}

#[test]
fn cloud_scheduler_capability_failures_stay_visible_until_a_successful_retry() {
    for (mode, expected) in [
        (CAPS_MISSING, State::UploadUnavailable),
        (CAPS_OFFLINE, State::Offline),
    ] {
        let mut fixture = Fixture::new(mode, 0);
        for _ in 0..3 {
            fixture.turn();
            assert_eq!(fixture.runner.state, expected);
            assert_eq!(fixture.runner.candidates.len(), 1);
            assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 0);
        }
        fixture.service.mode.store(READY, Ordering::Release);
        fixture.turn();
        assert_eq!(fixture.runner.state, State::Syncing);
        assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    }
}

#[test]
fn cloud_scheduler_verified_suppression_is_terminal_and_other_sources_continue() {
    let mut fixture = Fixture::new(SUPPRESSED, 0);
    fixture.turn();
    assert_eq!(
        fixture
            .store
            .cloud_sync_outcome_counts("account", "personal", &fixture.service.origin)
            .unwrap(),
        (0, 1)
    );
    let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(entry.state, CaptureState::Preparing);
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&entry.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Suppressed
    );
    let roots = vec![SourceSelection {
        source_kind: SourceKind::ClaudeJsonl,
        path: fixture.root.to_str().unwrap().into(),
    }];
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            &roots,
            &fixture.policy.projects,
            &fixture.policy.project_choices,
        )
        .unwrap();
    write_source(
        &fixture.root,
        &fixture.project,
        "session",
        "changed but still suppressed",
    );
    write_source(
        &fixture.root,
        &fixture.project,
        "second",
        "another allowed source",
    );
    fixture.runner = Runner::default();
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 2);
    assert_eq!(
        fixture
            .store
            .cloud_sync_outcome_counts("account", "personal", &fixture.service.origin)
            .unwrap(),
        (0, 1)
    );
    assert_eq!(fixture.host.spool().unwrap().entries().unwrap().len(), 2);
}

#[test]
fn cloud_scheduler_generic_conflict_and_offline_reserve_do_not_suppress_sources() {
    for mode in [CONFLICT, RESERVE_OFFLINE] {
        let mut fixture = Fixture::new(mode, 0);
        fixture.turn();
        assert_eq!(
            fixture
                .store
                .cloud_sync_outcome_counts("account", "personal", &fixture.service.origin)
                .unwrap(),
            (0, 0)
        );
        assert_eq!(
            fixture.host.spool().unwrap().entries().unwrap()[0].state,
            CaptureState::Preparing
        );
        assert_eq!(
            fixture
                .store
                .cloud_sync_jobs("account", "personal", &fixture.service.origin)
                .unwrap()[0]
                .state,
            JobState::Interrupted
        );
    }
}

#[test]
fn cloud_scheduler_explicit_retry_replaces_empty_preparing_with_new_id_and_keeps_accepted_state() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.accept_first();
    let source_id = format!("{}/session", fixture.policy.roots[0].source_scope_id);
    let accepted = fixture
        .store
        .cloud_sync_source("account", "personal", &fixture.service.origin, &source_id)
        .unwrap()
        .unwrap();
    write_source(
        &fixture.root,
        &fixture.project,
        "session",
        "changed before an offline reserve",
    );
    fixture
        .service
        .mode
        .store(RESERVE_OFFLINE, Ordering::Release);
    fixture.runner.next_reconcile = None;
    fixture.turn();
    let old = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(old.state, CaptureState::Preparing);
    assert!(old.reservation_id.is_none());
    retry_saved_jobs(
        &fixture.store,
        fixture.host.spool().unwrap(),
        &fixture.policy,
        &|| true,
    )
    .unwrap();
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&old.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Abandoned
    );
    assert_eq!(
        fixture
            .store
            .cloud_sync_source("account", "personal", &fixture.service.origin, &source_id)
            .unwrap()
            .unwrap(),
        accepted
    );
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.runner.next_reconcile = None;
    fixture.turn();
    let replacement = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_ne!(replacement.identity.capture_id, old.identity.capture_id);
    assert_eq!(replacement.identity.source_id, old.identity.source_id);
    assert_eq!(replacement.state, CaptureState::Queued);
    assert_eq!(
        fixture
            .store
            .cloud_sync_source("account", "personal", &fixture.service.origin, &source_id)
            .unwrap()
            .unwrap(),
        accepted
    );
}

#[test]
fn cloud_scheduler_restart_finishes_an_authorized_empty_recovery_without_reusing_the_id() {
    let mut fixture = Fixture::new(RESERVE_OFFLINE, 0);
    fixture.turn();
    let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert!(entry.reservation_id.is_none());
    let job = fixture
        .store
        .cloud_sync_job(&entry.identity.capture_id)
        .unwrap()
        .unwrap();
    fixture
        .store
        .set_cloud_sync_job_state(&job, JobState::Recovering)
        .unwrap();
    fixture
        .host
        .spool()
        .unwrap()
        .cancel(&entry.identity)
        .unwrap();
    let base = fixture._directory.path().canonicalize().unwrap();
    fixture.host = capture::Host::new(&base);
    fixture.store = Store::open(&base).unwrap();
    retry_saved_jobs(
        &fixture.store,
        fixture.host.spool().unwrap(),
        &fixture.policy,
        &|| true,
    )
    .unwrap();
    retry_saved_jobs(
        &fixture.store,
        fixture.host.spool().unwrap(),
        &fixture.policy,
        &|| true,
    )
    .unwrap();
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&entry.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Abandoned
    );
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.runner = Runner::default();
    fixture.turn();
    let replacement = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_ne!(replacement.identity.capture_id, entry.identity.capture_id);
}

struct Reservation {
    mutate: Option<std::path::PathBuf>,
}

impl Transport for Reservation {
    fn reserve(&mut self, _: &CaptureIdentity, _: u64) -> Result<String, TransportError> {
        if let Some(path) = self.mutate.take() {
            fs::OpenOptions::new()
                .append(true)
                .open(path)
                .unwrap()
                .write_all(b"\n")
                .unwrap();
        }
        Ok("reservation".into())
    }
    fn negotiate(&mut self, _: &str, _: &CaptureManifest) -> Result<UploadPlan, TransportError> {
        unreachable!()
    }
    fn status(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
    ) -> Result<ServerStatus, TransportError> {
        unreachable!()
    }
    fn upload_part(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
        _: &ChunkKey,
        _: &Chunk,
        _: &mut dyn Read,
    ) -> Result<(), TransportError> {
        unreachable!()
    }
    fn complete(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
    ) -> Result<(), TransportError> {
        unreachable!()
    }
}

fn saved_files(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap())
        .map(|entry| {
            (
                entry.file_name().to_str().unwrap().to_owned(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

fn saved_chunks(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".gz"))
        .map(|entry| {
            (
                entry.file_name().to_str().unwrap().to_owned(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

#[test]
fn cloud_scheduler_retry_preserves_partial_capture_chunks_and_complete_uploads() {
    let mut fixture = Fixture::new(READY, 0);
    let line = format!(
        "{}\n",
        serde_json::json!({
            "sessionId": "other-session", "cwd": fixture.project, "version": "2.1.55", "type": "user",
            "message": {"role":"user","content":"synthetic repeated content"},
        })
    );
    fs::write(fixture.root.join("session.jsonl"), line.repeat(60_000)).unwrap();
    let candidate = discover_root(SourceKind::ClaudeJsonl, fixture.root.to_str().unwrap(), 512)
        .unwrap()
        .remove(0)
        .in_source_scope(&fixture.policy.roots[0].source_scope_id)
        .unwrap();
    let job = fixture
        .store
        .enqueue_cloud_sync(
            &fixture.policy,
            &fixture.policy.roots[0].source_scope_id,
            fixture.project.to_str().unwrap(),
            candidate.source_id(),
            "partial-source",
        )
        .unwrap()
        .unwrap();
    let result = fixture.host.spool().unwrap().capture(
        job.identity.clone(),
        &candidate,
        MetadataSections::new(),
        &mut Reservation { mutate: None },
    );
    assert!(result.is_err());
    fixture
        .store
        .set_cloud_sync_job_state(&job, JobState::Interrupted)
        .unwrap();
    let directory = fixture
        ._directory
        .path()
        .join("cloud-capture-v1")
        .join(&job.identity.capture_id);
    let chunks = saved_chunks(&directory);
    assert!(!chunks.is_empty());
    write_source(
        &fixture.root,
        &fixture.project,
        "second",
        "saved upload body",
    );
    let candidate = discover_root(SourceKind::ClaudeJsonl, fixture.root.to_str().unwrap(), 512)
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.native_session_id() == "second")
        .unwrap()
        .in_source_scope(&fixture.policy.roots[0].source_scope_id)
        .unwrap();
    let queued_job = fixture
        .store
        .enqueue_cloud_sync(
            &fixture.policy,
            &fixture.policy.roots[0].source_scope_id,
            fixture.project.to_str().unwrap(),
            candidate.source_id(),
            "queued-source",
        )
        .unwrap()
        .unwrap();
    let queued = fixture
        .host
        .spool()
        .unwrap()
        .capture(
            queued_job.identity.clone(),
            &candidate,
            MetadataSections::new(),
            &mut Reservation { mutate: None },
        )
        .unwrap();
    fixture
        .store
        .set_cloud_sync_job_state(&queued_job, JobState::Queued)
        .unwrap();
    retry_saved_jobs(
        &fixture.store,
        fixture.host.spool().unwrap(),
        &fixture.policy,
        &|| true,
    )
    .unwrap();
    assert_eq!(chunks, saved_chunks(&directory));
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&job.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Interrupted
    );
    assert!(
        fixture
            .host
            .spool()
            .unwrap()
            .entries()
            .unwrap()
            .contains(&queued)
    );
}

#[test]
fn cloud_scheduler_quarantined_partial_capture_keeps_its_identity_and_blocks_recapture() {
    let mut fixture = Fixture::new(READY, 0);
    let line = format!(
        "{}\n",
        serde_json::json!({
            "sessionId": "other-session", "cwd": fixture.project, "version": "2.1.55", "type": "user",
            "message": {"role":"user","content":"synthetic quarantined content"},
        })
    );
    fs::write(fixture.root.join("session.jsonl"), line.repeat(60_000)).unwrap();
    let candidate = discover_root(SourceKind::ClaudeJsonl, fixture.root.to_str().unwrap(), 512)
        .unwrap()
        .remove(0)
        .in_source_scope(&fixture.policy.roots[0].source_scope_id)
        .unwrap();
    let job = fixture
        .store
        .enqueue_cloud_sync(
            &fixture.policy,
            &fixture.policy.roots[0].source_scope_id,
            fixture.project.to_str().unwrap(),
            candidate.source_id(),
            "partial-source",
        )
        .unwrap()
        .unwrap();
    assert!(
        fixture
            .host
            .spool()
            .unwrap()
            .capture(
                job.identity.clone(),
                &candidate,
                MetadataSections::new(),
                &mut Reservation { mutate: None }
            )
            .is_err()
    );
    fixture
        .store
        .set_cloud_sync_job_state(&job, JobState::Interrupted)
        .unwrap();
    let saved = fixture
        .store
        .cloud_sync_job(&job.identity.capture_id)
        .unwrap()
        .unwrap();
    let base = fixture._directory.path().canonicalize().unwrap();
    let original = base.join("cloud-capture-v1").join(&job.identity.capture_id);
    let quarantine = base.join("quarantined-capture");
    let chunks = saved_chunks(&original);
    assert!(!chunks.is_empty());
    fs::rename(&original, &quarantine).unwrap();
    write_source(
        &fixture.root,
        &fixture.project,
        "session",
        "new valid provider content",
    );
    fixture.host = capture::Host::new(&base);
    fixture.store = Store::open(&base).unwrap();
    fixture.runner = Runner::default();
    assert!(
        retry_saved_jobs(
            &fixture.store,
            fixture.host.spool().unwrap(),
            &fixture.policy,
            &|| true
        )
        .is_err()
    );
    assert!(
        fixture
            .runner
            .turn(
                &fixture.store,
                &mut fixture.host,
                &mut fixture.engine,
                &|| true,
                &|| true,
                &mut fixture.cache
            )
            .is_err()
    );
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&job.identity.capture_id)
            .unwrap()
            .unwrap(),
        saved
    );
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 0);
    assert_eq!(saved_chunks(&quarantine), chunks);
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
}

#[test]
fn cloud_scheduler_missing_pending_state_does_not_authorize_capture_or_recovery() {
    let mut fixture = Fixture::new(READY, 0);
    let source_id = format!("{}/session", fixture.policy.roots[0].source_scope_id);
    let job = fixture
        .store
        .enqueue_cloud_sync(
            &fixture.policy,
            &fixture.policy.roots[0].source_scope_id,
            fixture.project.to_str().unwrap(),
            &source_id,
            "pending-source",
        )
        .unwrap()
        .unwrap();
    assert!(
        retry_saved_jobs(
            &fixture.store,
            fixture.host.spool().unwrap(),
            &fixture.policy,
            &|| true
        )
        .is_err()
    );
    assert!(
        fixture
            .runner
            .turn(
                &fixture.store,
                &mut fixture.host,
                &mut fixture.engine,
                &|| true,
                &|| true,
                &mut fixture.cache
            )
            .is_err()
    );
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&job.identity.capture_id)
            .unwrap()
            .unwrap(),
        job
    );
    assert_eq!(fixture.service.requests.load(Ordering::Acquire), 0);
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
}

#[test]
fn cloud_scheduler_quarantined_queued_capture_is_not_abandoned_or_replaced() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    let job = fixture
        .store
        .cloud_sync_job(&saved.identity.capture_id)
        .unwrap()
        .unwrap();
    let base = fixture._directory.path().canonicalize().unwrap();
    let original = base
        .join("cloud-capture-v1")
        .join(&saved.identity.capture_id);
    let quarantine = base.join("quarantined-upload");
    let bytes = saved_chunks(&original);
    fs::rename(&original, &quarantine).unwrap();
    write_source(
        &fixture.root,
        &fixture.project,
        "session",
        "newer source while queued bytes are quarantined",
    );
    fixture.host = capture::Host::new(&base);
    fixture.store = Store::open(&base).unwrap();
    fixture.runner = Runner::default();
    let reservations = fixture.service.reserves.load(Ordering::Acquire);
    assert!(
        retry_saved_jobs(
            &fixture.store,
            fixture.host.spool().unwrap(),
            &fixture.policy,
            &|| true
        )
        .is_err()
    );
    assert!(
        fixture
            .runner
            .turn(
                &fixture.store,
                &mut fixture.host,
                &mut fixture.engine,
                &|| true,
                &|| true,
                &mut fixture.cache
            )
            .is_err()
    );
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&saved.identity.capture_id)
            .unwrap()
            .unwrap(),
        job
    );
    assert_eq!(
        fixture.service.reserves.load(Ordering::Acquire),
        reservations
    );
    assert_eq!(saved_chunks(&quarantine), bytes);
}

#[test]
fn cloud_scheduler_storage_failure_publishes_unknown_status_without_rereading_the_spool() {
    let mut fixture = Fixture::new(READY, 0);
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    let healthy = observed.last().unwrap().clone();
    assert_eq!(healthy.status, State::Syncing);
    assert!(healthy.storage_known);
    assert_eq!(healthy.queued_count, 1);
    let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    let directory = fixture
        ._directory
        .path()
        .join("cloud-capture-v1")
        .join(&entry.identity.capture_id);
    let chunks = saved_chunks(&directory);
    let record = directory.join("record.json");
    let damaged = b"synthetic unreadable capture record";
    fs::write(&record, damaged).unwrap();
    fixture.turn_published(&mut observed);
    assert_eq!(observed.len(), 2);
    let failure = observed.last().unwrap();
    assert_eq!(failure.status, State::StorageUnavailable);
    assert!(!failure.storage_known);
    assert_eq!(failure.queued_count, healthy.queued_count);
    assert_eq!(failure.accepted_count, healthy.accepted_count);
    assert_eq!(
        serde_json::to_value(&failure.progress).unwrap(),
        serde_json::to_value(&healthy.progress).unwrap()
    );
    assert_eq!(
        serde_json::to_value(failure).unwrap()["storageKnown"],
        false
    );
    assert_eq!(fs::read(&record).unwrap(), damaged);
    assert_eq!(saved_chunks(&directory), chunks);
}

#[test]
fn cloud_scheduler_first_storage_failure_does_not_claim_known_zero_counters() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.turn();
    let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    let record = fixture
        ._directory
        .path()
        .join("cloud-capture-v1")
        .join(&entry.identity.capture_id)
        .join("record.json");
    fs::write(&record, b"synthetic corrupt record").unwrap();
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].status, State::StorageUnavailable);
    assert!(!observed[0].storage_known);
    assert_eq!(observed[0].account_id.as_deref(), Some("account"));
    assert_eq!(observed[0].destination_id.as_deref(), Some("personal"));
}

#[test]
fn cloud_project_choices_restore_in_status_after_reopen_and_old_missing_maps_stay_empty() {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let mut fixture = Fixture::new(READY, 0);
    let group = |path: &str| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(
            serde_json::to_vec(&("native", path)).unwrap(),
        ))
    };
    let choices = BTreeMap::from([
        (group(fixture.project.to_str().unwrap()), true),
        (group("/Example/excluded"), false),
        (group("/Example/unrelated"), false),
    ]);
    let roots = vec![SourceSelection {
        source_kind: SourceKind::ClaudeJsonl,
        path: fixture.root.to_str().unwrap().into(),
    }];
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            &roots,
            &fixture.policy.projects,
            &choices,
        )
        .unwrap();
    fixture.store = Store::open(fixture._directory.path()).unwrap();
    let restored = status(
        &fixture.store,
        &mut fixture.host,
        &fixture.engine,
        State::Ready,
    )
    .unwrap();
    assert_eq!(restored.project_choices, choices);
    assert_eq!(
        serde_json::to_value(&restored).unwrap()["projectChoices"][group("/Example/excluded")],
        false
    );
    assert_eq!(fixture.service.requests.load(Ordering::Acquire), 0);
    let mut legacy = serde_json::to_value(&fixture.policy).unwrap();
    legacy.as_object_mut().unwrap().remove("projectChoices");
    fixture
        .store
        .lock()
        .execute(
            "UPDATE cloud_sync_policy SET record_json=?1",
            [legacy.to_string()],
        )
        .unwrap();
    let legacy_status = status(
        &fixture.store,
        &mut fixture.host,
        &fixture.engine,
        State::Ready,
    )
    .unwrap();
    assert!(legacy_status.project_choices.is_empty());
    assert_eq!(
        serde_json::to_value(legacy_status).unwrap()["projectChoices"],
        serde_json::json!({})
    );
}
