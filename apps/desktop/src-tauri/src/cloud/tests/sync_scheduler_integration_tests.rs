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
const PART_CONFLICT: usize = 10;
const FINALIZE_CONFLICT: usize = 11;
const IDLE_PART: usize = 12;
const IDLE_FINALIZE: usize = 13;
const RENEWED_PART_CONFLICT: usize = 14;
const RENEWED_FINALIZE_CONFLICT: usize = 15;
const IDLE_MANIFEST: usize = 16;
const FINALIZE_REPLAY: usize = 17;
const PART_OFFLINE: usize = 18;
const PART_CAPACITY: usize = 19;
const PART_REJECTED: usize = 20;
const CAPS_CAPACITY: usize = 21;
const REPLACED_PART: usize = 22;
const CODEX_AND_PI: usize = 23;
const STATUS_FINALIZE_REPLAY: usize = 24;
const STATUS_REPLAY_REJECTED: usize = 25;
const STATUS_REPLAY_OFFLINE: usize = 26;
const STATUS_REPLAY_CAPACITY: usize = 27;
const PART_DENIED: usize = 28;
const PART_UNAUTHORIZED: usize = 29;
const CAPS_DENIED: usize = 30;
const RESERVE_DENIED: usize = 31;
const STATUS_REPLAY_DENIED: usize = 32;
const CAPS_SMALL: usize = 33;
const CURSOR_IDE: usize = 34;

#[path = "sync_background_integration_tests.rs"]
mod background_tests;
#[path = "sync_runtime_activity_integration_tests.rs"]
mod runtime_activity_tests;
#[path = "sync_transfer_integration_tests.rs"]
mod transfer_tests;

fn session_progress(status: &Status) -> serde_json::Value {
    let mut progress = serde_json::to_value(status).unwrap()["uploadProgress"].clone();
    let progress_fields = progress.as_object_mut().unwrap();
    progress_fields.remove("transfer");
    progress_fields.remove("folderFound");
    progress
}

struct Service {
    origin: String,
    mode: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    reserves: Arc<AtomicUsize>,
    renewals: Arc<AtomicUsize>,
    conflicts: Arc<AtomicUsize>,
    finalizations: Arc<AtomicUsize>,
    part_keys: Arc<Mutex<Vec<ChunkKey>>>,
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
        let renewals = Arc::new(AtomicUsize::new(0));
        let conflicts = Arc::new(AtomicUsize::new(0));
        let finalizations = Arc::new(AtomicUsize::new(0));
        let part_keys = Arc::new(Mutex::new(Vec::new()));
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
        let worker_renewals = renewals.clone();
        let worker_conflicts = conflicts.clone();
        let worker_finalizations = finalizations.clone();
        let worker_part_keys = part_keys.clone();
        let worker = std::thread::spawn(move || {
            let mut manifest: Option<CaptureManifest> = None;
            let mut receipts = Vec::<PartReceipt>::new();
            let mut pending_key: Option<ChunkKey> = None;
            let mut finalized = false;
            let mut active = false;
            let mut renewed = false;
            let mut conflicted_request = None;
            let mut finalized_request = None;
            let mut replaced = false;
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
                let status_request = line.starts_with("GET ")
                    && manifest.as_ref().is_some_and(|saved| {
                        line == format!(
                            "GET /v1/captures/{}?plan_id=plan&manifest_sha256={} HTTP/1.1",
                            saved.identity.capture_id,
                            saved.sha256().unwrap()
                        )
                    });
                let part_url_request = line.contains("/part-urls ");
                if (part_url_request || line.contains("/finalize ") || line.contains("/manifest "))
                    && let Some(previous) = conflicted_request.take()
                {
                    assert_eq!((line.clone(), body.clone()), previous);
                }
                let (status, output) = if line.contains("/capabilities ") {
                    if mode == CAPS_OFFLINE {
                        continue;
                    }
                    if mode == CAPS_CAPACITY {
                        (429, serde_json::json!({}))
                    } else if mode == CAPS_DENIED {
                        (403, serde_json::json!({}))
                    } else if mode == CAPS_MISSING {
                        (404, serde_json::json!({}))
                    } else {
                        let mut formats = vec!["ClaudeJsonl", "OpenCodeSqliteV2"];
                        if mode == CODEX_AND_PI {
                            formats.extend(["CodexJsonl", "PiJsonl"]);
                        }
                        if mode == CURSOR_IDE {
                            formats.push("CursorIdeComposer");
                        }
                        (
                            200,
                            serde_json::json!({
                                "enabled": true,
                                "max_expanded_bytes": if mode == CAPS_SMALL {
                                    256 * 1024
                                } else {
                                    256 * 1024 * 1024
                                },
                                "chunk_expanded_bytes": 8 * 1024 * 1024,
                                "source_formats": formats,
                            }),
                        )
                    }
                } else if line == "POST /v1/captures HTTP/1.1" {
                    worker_reserves.fetch_add(1, Ordering::AcqRel);
                    let identity: CaptureIdentity =
                        serde_json::from_value(input["identity"].clone()).unwrap();
                    if mode == CODEX_AND_PI {
                        let id = input["native_session_id"].as_str().unwrap();
                        let (agent, format) = if identity.source_id.contains("/codex/") {
                            ("codex", "CodexJsonl")
                        } else {
                            ("pi", "PiJsonl")
                        };
                        assert!(identity.source_id.ends_with(&format!("/{agent}/{id}")));
                        assert_eq!(input["agent"], agent);
                        assert_eq!(input["source_format"], format);
                    }
                    if mode == CURSOR_IDE {
                        assert_eq!(input["agent"], "cursor");
                        assert_eq!(input["source_format"], "CursorIdeComposer");
                        assert_eq!(input["native_session_id"], "synthetic-chunked");
                    }
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
                    if mode == RESERVE_DENIED {
                        (403, serde_json::json!({}))
                    } else if mode == UNKNOWN_CAPACITY {
                        (
                            429,
                            serde_json::json!({
                                "code":"unknown_limit", "retryable":true,
                                "message":"Synthetic limit.", "request_id": "synthetic-request",
                            }),
                        )
                    } else if mode == RESERVE_CAPACITY
                        || (matches!(mode, CAPACITY_ONE | VERIFY_PENDING) && active)
                    {
                        worker_capacity_rejections.fetch_add(1, Ordering::AcqRel);
                        (
                            429,
                            serde_json::json!({
                                "code": "capture_limit", "retryable": true,
                                "message": "Capture capacity is full.", "request_id": "synthetic-request",
                            }),
                        )
                    } else if mode == SUPPRESSED
                        && input["identity"]["source_id"]
                            .as_str()
                            .unwrap()
                            .ends_with("/session")
                    {
                        (
                            409,
                            serde_json::json!({
                                "code": "source_suppressed", "retryable": false,
                                "message": "This session was deleted from Cloud. It will not upload again automatically.",
                                "request_id": "synthetic-request",
                            }),
                        )
                    } else if mode == CONFLICT {
                        (
                            409,
                            serde_json::json!({"code": "capture_conflict", "retryable": false, "message": "Conflict.", "request_id": "synthetic-request"}),
                        )
                    } else {
                        manifest = None;
                        receipts.clear();
                        finalized = false;
                        finalized_request = None;
                        renewed = false;
                        active = true;
                        (200, serde_json::json!({"reservation_id": "reservation"}))
                    }
                } else if line.contains("/manifest ") && mode == IDLE_MANIFEST && !renewed {
                    conflicted_request = Some((line.clone(), body.clone()));
                    (
                        409,
                        serde_json::json!({"code": "capture_conflict", "retryable": false, "message": "Conflict.", "request_id": "synthetic-request"}),
                    )
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
                } else if status_request
                    && (matches!(
                        mode,
                        STATUS_REPLAY_REJECTED
                            | STATUS_REPLAY_OFFLINE
                            | STATUS_REPLAY_CAPACITY
                            | STATUS_REPLAY_DENIED
                    ) || (mode == STATUS_FINALIZE_REPLAY
                        && worker_finalizations.load(Ordering::Acquire) < 2))
                {
                    (
                        409,
                        serde_json::json!({"code": "capture_conflict", "retryable": false, "message": "Conflict.", "request_id": "synthetic-request"}),
                    )
                } else if status_request {
                    let saved = manifest.as_ref().unwrap();
                    if mode == REPLACED_PART && !replaced {
                        receipts.retain(|receipt| {
                            receipt.key.stream_id != "transcript" || receipt.key.index != 1
                        });
                        replaced = true;
                    }
                    let accepted = finalized
                        && mode != VERIFY_PENDING
                        && (mode != FINALIZE_REPLAY
                            || worker_finalizations.load(Ordering::Acquire) >= 2);
                    if accepted {
                        active = false;
                    }
                    (
                        200,
                        serde_json::to_value(ServerStatus {
                            identity: saved.identity.clone(),
                            manifest_sha256: saved.sha256().unwrap(),
                            plan_id: "plan".into(),
                            uploaded_parts: receipts.clone(),
                            integrity_accepted: accepted,
                        })
                        .unwrap(),
                    )
                } else if line.contains("/renew-lease ") {
                    worker_renewals.fetch_add(1, Ordering::AcqRel);
                    assert_eq!(input["reservation_id"], "reservation");
                    if let Some(saved) = &manifest {
                        assert_eq!(input["manifest_sha256"], saved.sha256().unwrap());
                        assert_eq!(input["plan_id"], "plan");
                    } else {
                        assert!(input["manifest_sha256"].is_null());
                        assert!(input["plan_id"].is_null());
                    }
                    if matches!(
                        mode,
                        IDLE_PART
                            | IDLE_FINALIZE
                            | READY
                            | RENEWED_PART_CONFLICT
                            | RENEWED_FINALIZE_CONFLICT
                            | IDLE_MANIFEST
                    ) {
                        renewed = true;
                        (204, serde_json::Value::Null)
                    } else {
                        (
                            409,
                            serde_json::json!({"code": "capture_conflict", "retryable": false, "message": "Conflict.", "request_id": "synthetic-request"}),
                        )
                    }
                } else if part_url_request && mode == PART_OFFLINE {
                    (503, serde_json::json!({}))
                } else if part_url_request && mode == PART_CAPACITY {
                    (429, serde_json::json!({}))
                } else if part_url_request && mode == PART_REJECTED {
                    (422, serde_json::json!({}))
                } else if part_url_request && mode == PART_DENIED {
                    (403, serde_json::json!({}))
                } else if part_url_request && mode == PART_UNAUTHORIZED {
                    (401, serde_json::json!({}))
                } else if (mode == PART_CONFLICT && part_url_request)
                    || (mode == FINALIZE_CONFLICT && line.contains("/finalize "))
                    || (mode == RENEWED_PART_CONFLICT && part_url_request)
                    || (mode == RENEWED_FINALIZE_CONFLICT && line.contains("/finalize "))
                    || (!renewed && mode == IDLE_PART && part_url_request)
                    || (!renewed && mode == IDLE_FINALIZE && line.contains("/finalize "))
                {
                    worker_conflicts.fetch_add(1, Ordering::AcqRel);
                    conflicted_request = Some((line.clone(), body.clone()));
                    (
                        409,
                        serde_json::json!({"code": "capture_conflict", "retryable": false, "message": "Conflict.", "request_id": "synthetic-request"}),
                    )
                } else if part_url_request {
                    let key: ChunkKey = serde_json::from_value(input["key"].clone()).unwrap();
                    worker_part_keys.lock().unwrap().push(key.clone());
                    let chunk = manifest.as_ref().unwrap().chunk(&key).unwrap();
                    pending_key = Some(key);
                    (
                        200,
                        serde_json::json!({
                            "url": format!("{service_origin}/object?partNumber=1&uploadId={}", if replaced { "synthetic-replacement" } else { "synthetic" }),
                            "part_number": 1, "headers": {"content-length": chunk.compressed_bytes.to_string()},
                        }),
                    )
                } else if object {
                    if replaced {
                        assert!(line.contains("&uploadId=synthetic-replacement "));
                    } else {
                        assert!(line.contains("&uploadId=synthetic "));
                    }
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
                    worker_finalizations.fetch_add(1, Ordering::AcqRel);
                    if let Some(saved) = &finalized_request {
                        assert_eq!(&(line.clone(), body.clone()), saved);
                    } else {
                        finalized_request = Some((line.clone(), body.clone()));
                    }
                    let saved = manifest.as_ref().unwrap();
                    assert_eq!(receipts.len(), saved.chunks().count());
                    if mode == STATUS_REPLAY_OFFLINE {
                        (503, serde_json::json!({}))
                    } else if mode == STATUS_REPLAY_CAPACITY {
                        (429, serde_json::json!({}))
                    } else if mode == STATUS_REPLAY_DENIED {
                        (403, serde_json::json!({}))
                    } else if mode == STATUS_REPLAY_REJECTED {
                        (
                            409,
                            serde_json::json!({"code": "capture_conflict", "retryable": false, "message": "Conflict.", "request_id": "synthetic-request"}),
                        )
                    } else {
                        finalized = true;
                        (
                            202,
                            serde_json::json!({
                                "operation_id": "synthetic-operation",
                                "status_url": format!(
                                    "/v1/captures/{}?plan_id=plan&manifest_sha256={}",
                                    saved.identity.capture_id,
                                    saved.sha256().unwrap()
                                ),
                            }),
                        )
                    }
                } else {
                    panic!("unexpected test service route");
                };
                let output = if status == 204 {
                    String::new()
                } else {
                    output.to_string()
                };
                let retry_after = if status == 429 || status == 503 {
                    "Retry-After: 90\r\n"
                } else if status == 403 && mode == PART_DENIED {
                    "Retry-After: 3600\r\n"
                } else {
                    ""
                };
                write!(stream, "HTTP/1.1 {status} Synthetic\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{retry_after}\r\n{output}", output.len()).unwrap();
            }
        });
        Self {
            origin,
            mode,
            requests,
            reserves,
            renewals,
            conflicts,
            finalizations,
            part_keys,
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
    other_team: AtomicBool,
    workspaces_unavailable: AtomicBool,
    workspace_lookups: AtomicUsize,
    origin: String,
    workspace: Workspace,
    membership: AtomicUsize,
    deadline: i64,
    content_generation: Arc<AtomicU64>,
    pause_stage: usize,
    exchanges: AtomicUsize,
    sessions: AtomicUsize,
    revocations: AtomicUsize,
    grant: AtomicUsize,
}

impl Authority {
    fn session(&self) -> Session {
        Session {
            user: auth::User {
                id: "account".into(),
                display_name: "Example".into(),
            },
            grant_id: format!("grant-{}", self.grant.load(Ordering::Acquire)),
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
            grant_id: format!("grant-{}", self.grant.load(Ordering::Acquire)),
            user_id: "account".into(),
            legacy_workspace_claim: false,
            grant_expires_at: time::OffsetDateTime::from_unix_timestamp(self.deadline)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
            scope: "capture:write".into(),
            resource: self.origin.clone(),
            iss: self.origin.clone(),
        })
    }
    fn session(&self, _: &Config, _: &str) -> Result<auth::AccountSession, auth::Failure> {
        let index = self.sessions.fetch_add(1, Ordering::AcqRel);
        if index == 1 && self.pause_stage == 2 {
            self.content_generation.fetch_add(1, Ordering::AcqRel);
        }
        Ok(Authority::session(self).into())
    }
    fn workspaces(
        &self,
        _: &Config,
        _: &str,
    ) -> Result<Vec<auth::EligibleWorkspace>, auth::Failure> {
        self.workspace_lookups.fetch_add(1, Ordering::AcqRel);
        if self.workspaces_unavailable.load(Ordering::Acquire) {
            return Err(auth::Failure::Offline);
        }
        let mut entries = vec![auth::EligibleWorkspace {
            workspace: Authority::session(self).workspace,
            membership_generation: None,
        }];
        if self.workspace.kind == WorkspaceKind::Team {
            entries.push(auth::EligibleWorkspace {
                workspace: self.workspace.clone(),
                membership_generation: Some(format!(
                    "membership-{}",
                    self.membership.load(Ordering::Acquire)
                )),
            });
        }
        if self.other_team.load(Ordering::Acquire) {
            entries.push(auth::EligibleWorkspace {
                workspace: Workspace {
                    id: "team".into(),
                    name: "Team".into(),
                    kind: WorkspaceKind::Team,
                },
                membership_generation: Some(format!(
                    "membership-{}",
                    self.membership.load(Ordering::Acquire)
                )),
            });
        }
        Ok(entries)
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

#[test]
fn overlapping_team_folder_does_not_block_personal_when_membership_read_fails() {
    let mut fixture = Fixture::new(READY, 0);
    let root = &fixture.policy.roots[0];
    let project = fixture.project.join("nested");
    fs::create_dir(&project).unwrap();
    fixture.authority.other_team.store(true, Ordering::Release);
    fixture
        .store
        .save_cloud_sync_policy_for_membership(
            (
                "account",
                "team",
                &fixture.service.origin,
                &fixture.service.origin,
                Some(("membership-0", "Team")),
            ),
            &[SourceSelection {
                source_kind: root.source_kind,
                path: root.path.clone(),
            }],
            &[ProjectSelection {
                source_kind: root.source_kind,
                source_path: root.path.clone(),
                project_path: project.to_str().unwrap().into(),
            }],
            &BTreeMap::new(),
            || true,
        )
        .unwrap();

    fixture
        .authority
        .workspaces_unavailable
        .store(true, Ordering::Release);
    fixture.turn();
    let queued = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(queued.identity.destination_id, "personal");
}

#[test]
fn queued_workspace_upload_keeps_its_original_capture_when_another_folder_is_enabled() {
    let mut fixture = Fixture::new(READY, 0);
    let other_folder = fixture.project.with_file_name("another-project");
    fs::create_dir(&other_folder).unwrap();
    let transcript = fixture.root.join("session.jsonl");
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap(),
        "{}",
        serde_json::json!({
            "sessionId": "session", "cwd": other_folder, "version": "2.1.55", "type": "user",
            "message": {"role": "user", "content": "Synthetic second folder"},
        })
    )
    .unwrap();
    fixture.turn();
    let queued = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(queued.state, CaptureState::Queued);
    let job = fixture
        .store
        .cloud_sync_job(&queued.identity.capture_id)
        .unwrap()
        .unwrap();
    assert!(
        job.recorded_project_paths
            .unwrap()
            .contains(&other_folder.to_str().unwrap().to_owned())
    );
    let requests = fixture.service.requests.load(Ordering::Acquire);
    fs::remove_file(transcript).unwrap();

    let root = &fixture.policy.roots[0];
    fixture.authority.other_team.store(true, Ordering::Release);
    fixture
        .store
        .save_cloud_sync_policy_for_membership(
            (
                "account",
                "team",
                &fixture.service.origin,
                &fixture.service.origin,
                Some(("membership-0", "Team")),
            ),
            &[SourceSelection {
                source_kind: root.source_kind,
                path: root.path.clone(),
            }],
            &[ProjectSelection {
                source_kind: root.source_kind,
                source_path: root.path.clone(),
                project_path: other_folder.to_str().unwrap().into(),
            }],
            &BTreeMap::new(),
            || true,
        )
        .unwrap();
    fixture.restart_scheduler();
    fixture.turn();
    assert!(fixture.service.requests.load(Ordering::Acquire) > requests);
    assert_eq!(
        fixture.host.spool().unwrap().entries().unwrap()[0].identity,
        queued.identity
    );
    fixture.accept_first();
}

#[test]
fn one_session_that_matches_two_workspace_folders_has_two_bound_captures() {
    let mut fixture = Fixture::new(READY, 0);
    let other_folder = fixture.project.with_file_name("another-project");
    fs::create_dir(&other_folder).unwrap();
    let root = &fixture.policy.roots[0];
    fixture.authority.other_team.store(true, Ordering::Release);
    fixture
        .store
        .save_cloud_sync_policy_for_membership(
            (
                "account",
                "team",
                &fixture.service.origin,
                &fixture.service.origin,
                Some(("membership-0", "Team")),
            ),
            &[SourceSelection {
                source_kind: root.source_kind,
                path: root.path.clone(),
            }],
            &[ProjectSelection {
                source_kind: root.source_kind,
                source_path: root.path.clone(),
                project_path: other_folder.to_str().unwrap().into(),
            }],
            &BTreeMap::new(),
            || true,
        )
        .unwrap();
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(fixture.root.join("session.jsonl"))
        .unwrap();
    writeln!(
        file,
        "{}",
        serde_json::json!({
            "sessionId": "session", "cwd": other_folder, "version": "2.1.55", "type": "user",
            "message": {"role": "user", "content": "Synthetic second folder"},
        })
    )
    .unwrap();

    fixture.turn();
    fixture.runner = Runner {
        team: Some(("team".into(), "membership-0".into())),
        ..Runner::default()
    };
    fixture.turn();
    let reservations = fixture.service.reserve_identities.lock().unwrap();
    assert_eq!(reservations.len(), 2);
    assert_eq!(reservations[0].destination_id, "personal");
    assert_eq!(reservations[1].destination_id, "team");
    assert_ne!(reservations[0].capture_id, reservations[1].capture_id);
    assert_ne!(reservations[0].source_id, reservations[1].source_id);
}

#[test]
fn codex_and_pi_scheduler_requires_folder_consent_and_advertised_formats() {
    let mut fixture = Fixture::new(READY, 0);
    let codex_root = fixture.root.join("codex");
    let pi_root = fixture.root.join("pi");
    let codex_day = codex_root.join("2026/11/19");
    let pi_folder = pi_root.join("--synthetic--");
    fs::create_dir_all(&codex_day).unwrap();
    fs::create_dir_all(&pi_folder).unwrap();
    let codex_id = "12345678-1234-1234-1234-123456789abc";
    let pi_id = "abcdef12-1234-1234-1234-123456789abc";
    let outside_id = "eeeeeeee-1234-1234-1234-123456789abc";
    let cwd = &fixture.project;
    fs::write(
        codex_day.join(format!("rollout-2026-11-19T12-00-00-{codex_id}.jsonl")),
        format!(
            "{}\n{{\"type\":\"response_item\"}}\n",
            serde_json::json!({
                "type":"session_meta", "payload":{"id":codex_id,"cwd":cwd,"cli_version":"1.2.3"}
            })
        ),
    )
    .unwrap();
    fs::write(
        pi_folder.join(format!("2026-11-19T12-00-00Z_{pi_id}.jsonl")),
        format!(
            "{}\n{{\"type\":\"synthetic\"}}\n",
            serde_json::json!({
                "type":"session","id":pi_id,"cwd":cwd,"version":3
            })
        ),
    )
    .unwrap();
    fs::write(
        pi_folder.join(format!("2026-11-19T12-00-00Z_{outside_id}.jsonl")),
        format!(
            "{}\n",
            serde_json::json!({
                "type":"session","id":outside_id,"cwd":&fixture.root
            })
        ),
    )
    .unwrap();
    let roots = [
        SourceSelection {
            source_kind: SourceKind::CodexJsonl,
            path: codex_root.to_str().unwrap().into(),
        },
        SourceSelection {
            source_kind: SourceKind::PiJsonl,
            path: pi_root.to_str().unwrap().into(),
        },
    ];
    let projects = roots.each_ref().map(|root| ProjectSelection {
        source_kind: root.source_kind,
        source_path: root.path.clone(),
        project_path: cwd.to_str().unwrap().into(),
    });
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            &roots,
            &projects,
            &BTreeMap::new(),
        )
        .unwrap();

    fixture.schedule_backfill();
    let mut unsupported = Vec::new();
    fixture.turn_published(&mut unsupported);
    assert_eq!(unsupported[0].status, State::UploadUnavailable);
    assert_eq!(
        session_progress(&unsupported[0]),
        serde_json::json!({
            "uploaded": 0, "total": 2, "blocked": 0, "unsupported": 2,
            "scanIncomplete": false
        })
    );
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 0);
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());

    fixture.service.mode.store(CODEX_AND_PI, Ordering::Release);
    fixture.runner.next_reconcile = None;
    fixture.turn();
    let entries = fixture.host.spool().unwrap().entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].manifest.is_some());
    assert!(entries[0].upload_plan.is_none());
    let job = fixture
        .store
        .cloud_sync_job(&entries[0].identity.capture_id)
        .unwrap()
        .unwrap();
    fixture
        .store
        .set_cloud_sync_job_state(&job, JobState::Fenced)
        .unwrap();

    fixture.restart_scheduler();
    fixture.service.mode.store(CAPS_MISSING, Ordering::Release);
    fixture.schedule_backfill();
    let mut unavailable = Vec::new();
    fixture.turn_published(&mut unavailable);
    fixture.turn_published(&mut unavailable);
    assert_eq!(
        session_progress(&unavailable[1]),
        serde_json::json!({
            "uploaded": 0, "total": null, "blocked": 1, "unsupported": 0,
            "scanIncomplete": true
        })
    );

    fixture.service.mode.store(READY, Ordering::Release);
    fixture.runner.retry_deadlines.clear();
    fixture.schedule_backfill();
    let requests = fixture.service.requests.load(Ordering::Acquire);
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    assert_eq!(observed[0].status, State::UploadUnavailable);
    assert_eq!(
        serde_json::to_value(&observed[0]).unwrap()["uploadProgress"]["unsupported"],
        0
    );
    assert_eq!(
        fixture.service.requests.load(Ordering::Acquire),
        requests + 1
    );
    let saved = fixture.host.spool().unwrap().entries().unwrap();
    assert_eq!(saved[0].identity, entries[0].identity);
    assert_eq!(saved[0].manifest, entries[0].manifest);
    assert!(saved[0].upload_plan.is_none());
    fixture.turn_published(&mut observed);
    assert_eq!(
        session_progress(&observed[1]),
        serde_json::json!({
            "uploaded": 0, "total": 2, "blocked": 0, "unsupported": 2,
            "scanIncomplete": false
        })
    );

    fixture.service.mode.store(CODEX_AND_PI, Ordering::Release);
    fixture.runner.retry_deadlines.clear();
    fixture.runner.next_reconcile = None;
    for _ in 0..30 {
        fixture.turn();
    }
    let ids = fixture.service.reserve_identities.lock().unwrap();
    assert_eq!(ids.len(), 2);
    assert!(
        ids.iter()
            .any(|id| id.source_id.ends_with(&format!("/codex/{codex_id}")))
    );
    assert!(
        ids.iter()
            .any(|id| id.source_id.ends_with(&format!("/pi/{pi_id}")))
    );
    drop(ids);
    assert_eq!(
        fixture
            .store
            .cloud_sync_outcome_counts("account", "personal", &fixture.service.origin)
            .unwrap()
            .0,
        2
    );
    let malformed_id = "deadbeef-1234-1234-1234-123456789abc";
    for malformed in [
        codex_day.join(format!("rollout-2026-11-19T12-00-00-{malformed_id}.jsonl")),
        pi_folder.join(format!("2026-11-19T12-00-00Z_{malformed_id}.jsonl")),
    ] {
        fs::write(&malformed, b"{\"type\":\n").unwrap();
        fixture.schedule_backfill();
        let mut observed = Vec::new();
        for _ in 0..10 {
            fixture.runner.next_integrity_poll = None;
            fixture.turn_published(&mut observed);
        }
        let result = observed.last().unwrap();
        assert!(!result.backfill_in_progress);
        assert_eq!(
            session_progress(result),
            serde_json::json!({
                "uploaded": 2, "total": null, "blocked": 0, "unsupported": 0,
                "scanIncomplete": true
            })
        );
        fs::remove_file(malformed).unwrap();
    }
}

impl Fixture {
    fn new(mode: usize, pause_stage: usize) -> Self {
        Self::for_workspace(
            mode,
            pause_stage,
            Workspace {
                id: "personal".into(),
                name: "Personal".into(),
                kind: WorkspaceKind::Personal,
            },
        )
    }

    fn for_workspace(mode: usize, pause_stage: usize, workspace: Workspace) -> Self {
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
            .save_cloud_sync_policy_for_membership(
                (
                    "account",
                    &workspace.id,
                    &service.origin,
                    &service.origin,
                    (workspace.kind == WorkspaceKind::Team)
                        .then_some(("membership-0", workspace.name.as_str())),
                ),
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
                || true,
            )
            .unwrap();
        let content_generation = Arc::new(AtomicU64::new(0));
        let authority = Arc::new(Authority {
            other_team: AtomicBool::new(false),
            workspaces_unavailable: AtomicBool::new(false),
            workspace_lookups: AtomicUsize::new(0),
            origin: service.origin.clone(),
            workspace: workspace.clone(),
            membership: AtomicUsize::new(0),
            deadline: now() + 86_400,
            content_generation: content_generation.clone(),
            pause_stage,
            exchanges: AtomicUsize::new(0),
            sessions: AtomicUsize::new(0),
            revocations: AtomicUsize::new(0),
            grant: AtomicUsize::new(0),
        });
        let vault = Arc::new(crate::cloud::tests::MemoryVault::default());
        let mut engine = Engine::new(
            Config::parse(&service.origin, &service.origin),
            Box::new(vault),
            Box::new(authority.clone()),
            StateStore::new(&base),
        );
        let team = (workspace.kind == WorkspaceKind::Team)
            .then(|| (workspace.id.clone(), "membership-0".into()));
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
            runner: Runner {
                team,
                ..Runner::default()
            },
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
                (&|| true, None),
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
                (&|| true, None),
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

    fn published_status(&mut self) -> Status {
        self.runner.report_status(
            &self.store,
            &mut self.host,
            &self.engine,
            false,
            &|| true,
            |_| {},
        )
    }

    fn accept_first(&mut self) {
        self.accept_count(1);
    }

    fn accept_count(&mut self, expected: u32) {
        for _ in 0..10 {
            self.runner.next_integrity_poll = None;
            self.turn();
            if self
                .store
                .cloud_sync_outcome_counts(
                    "account",
                    &self.policy.destination_id,
                    &self.service.origin,
                )
                .unwrap()
                .0
                == expected
            {
                return;
            }
        }
        panic!("test did not reach {expected} accepted sources");
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

    fn sign_in_again(&mut self) {
        self.engine.sign_out().unwrap();
        self.authority.grant.fetch_add(1, Ordering::AcqRel);
        let (_, _listener, attempt) = self.engine.begin(now(), 0).unwrap();
        self.engine
            .callback(
                &attempt,
                auth::Callback::Code("synthetic-new-sign-in".into()),
                now(),
                || true,
            )
            .unwrap();
    }

    fn schedule_backfill(&mut self) {
        let session = self.engine.session().unwrap();
        self.runner.observe_retry_grant(session);
        self.runner
            .schedule_backfill(&self.policy, &session.grant_id);
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

#[test]
fn cloud_team_scheduler_reuses_current_turn_proof_and_rechecks_after_grant_change() {
    let workspace = Workspace {
        id: "team".into(),
        name: "Example Team".into(),
        kind: WorkspaceKind::Team,
    };
    for changed_grant in [false, true] {
        let mut fixture = Fixture::for_workspace(READY, 0, workspace.clone());
        fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
        let session = fixture.engine.session().unwrap().clone();
        let binding = DestinationBinding {
            account_id: session.user.id.clone(),
            destination_id: session.workspace.id.clone(),
        };
        let before = fixture.authority.workspace_lookups.load(Ordering::Acquire);
        let engine_generation = fixture.engine.generation;
        let entries = fixture
            .engine
            .eligible_workspaces(&binding, &|| true)
            .unwrap();
        let selected_policy = fixture.policy.clone();
        let proof =
            TeamTurnProof::for_policy(&session, engine_generation, &selected_policy, &entries)
                .unwrap();
        assert_eq!(
            fixture.authority.workspace_lookups.load(Ordering::Acquire),
            before + 1
        );
        if changed_grant {
            fixture.sign_in_again();
            fixture.authority.membership.store(1, Ordering::Release);
        }
        let before_turn = fixture.authority.workspace_lookups.load(Ordering::Acquire);
        fixture
            .runner
            .turn(
                &fixture.store,
                &mut fixture.host,
                &mut fixture.engine,
                &|| true,
                (&|| true, Some(proof)),
                &mut fixture.cache,
            )
            .unwrap();
        assert_eq!(
            fixture.authority.workspace_lookups.load(Ordering::Acquire),
            before_turn + usize::from(changed_grant)
        );
        assert_eq!(
            fixture.runner.state,
            if changed_grant {
                State::ReauthRequired
            } else {
                State::Ready
            }
        );
        assert_eq!(fixture.service.requests.load(Ordering::Acquire), 0);
    }
}

#[test]
fn cloud_team_scheduler_blocks_changed_membership_even_after_fresh_account_approval() {
    let workspace = Workspace {
        id: "team".into(),
        name: "Example Team".into(),
        kind: WorkspaceKind::Team,
    };
    let mut complete = Fixture::for_workspace(READY, 0, workspace.clone());
    complete.accept_first();
    assert_eq!(
        complete.service.reserve_identities.lock().unwrap()[0].destination_id,
        "team"
    );
    let mut replaced = Fixture::for_workspace(READY, 0, workspace);
    replaced.turn();
    assert_eq!(replaced.host.spool().unwrap().entries().unwrap().len(), 1);
    let requests = replaced.service.requests.load(Ordering::Acquire);
    replaced.authority.membership.store(1, Ordering::Release);
    replaced.sign_in_again();
    replaced.turn();
    assert_eq!(replaced.runner.state, State::ReauthRequired);
    assert_eq!(replaced.service.requests.load(Ordering::Acquire), requests);
    assert_eq!(replaced.host.spool().unwrap().entries().unwrap().len(), 1);
}

#[test]
fn cloud_team_scheduler_retries_interrupted_upload_after_membership_lookup_recovers() {
    let mut fixture = Fixture::for_workspace(
        PART_REJECTED,
        0,
        Workspace {
            id: "team".into(),
            name: "Example Team".into(),
            kind: WorkspaceKind::Team,
        },
    );
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    fixture.turn();
    let job = || {
        fixture
            .store
            .cloud_sync_job(&saved.identity.capture_id)
            .unwrap()
            .unwrap()
    };
    assert_eq!(job().state, JobState::Interrupted);

    fixture.sign_in_again();
    fixture
        .authority
        .workspaces_unavailable
        .store(true, Ordering::Release);
    let requests = fixture.service.requests.load(Ordering::Acquire);
    fixture.turn();
    assert_eq!(fixture.runner.state, State::ReauthRequired);
    assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);

    fixture
        .authority
        .workspaces_unavailable
        .store(false, Ordering::Release);
    fixture.service.mode.store(READY, Ordering::Release);
    fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
    fixture.turn();
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&saved.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Queued
    );
    assert!(fixture.service.requests.load(Ordering::Acquire) > requests);
    fixture.accept_first();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
}

#[test]
fn cloud_team_scheduler_never_reopens_legacy_folder_policy() {
    let workspace = Workspace {
        id: "team".into(),
        name: "Example Team".into(),
        kind: WorkspaceKind::Team,
    };
    let mut fixture = Fixture::for_workspace(READY, 0, workspace);
    let root = &fixture.policy.roots[0];
    fixture
        .store
        .save_cloud_sync_policy_for_grant(
            (
                "account",
                "team",
                &fixture.service.origin,
                &fixture.service.origin,
                Some(("old-grant", "Example Team")),
            ),
            &[SourceSelection {
                source_kind: root.source_kind,
                path: root.path.clone(),
            }],
            &fixture.policy.projects,
            &BTreeMap::new(),
        )
        .unwrap();
    fixture.sign_in_again();
    fixture.turn();
    assert_eq!(fixture.runner.state, State::Fenced);
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
    assert_eq!(fixture.service.requests.load(Ordering::Acquire), 0);
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
fn cloud_scheduler_replaced_multipart_restarts_only_the_missing_chunk() {
    let mut fixture = Fixture::new(READY, 0);
    let line = format!(
        "{}\n",
        serde_json::json!({
            "sessionId": "session", "cwd": fixture.project, "type": "user",
            "message": {"role":"user", "content":"synthetic content ".repeat(64)},
        })
    );
    fs::write(fixture.root.join("session.jsonl"), line.repeat(8_000)).unwrap();
    fixture.turn();
    fixture.turn();
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(
        fixture.service.part_keys.lock().unwrap().as_slice(),
        &[
            ChunkKey {
                stream_id: "transcript".into(),
                index: 0
            },
            ChunkKey {
                stream_id: "transcript".into(),
                index: 1
            },
        ]
    );
    let directory = fixture
        ._directory
        .path()
        .join("cloud-capture-v1")
        .join(&saved.identity.capture_id);
    let bytes = saved_chunks(&directory);
    fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
    fixture.service.mode.store(REPLACED_PART, Ordering::Release);
    fixture.turn();
    let kept = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(kept.identity, saved.identity);
    assert_eq!(kept.manifest, saved.manifest);
    assert_eq!(kept.upload_plan, saved.upload_plan);
    assert_eq!(saved_chunks(&directory), bytes);
    fixture.accept_first();
    assert_eq!(
        fixture.service.part_keys.lock().unwrap().as_slice(),
        &[
            ChunkKey {
                stream_id: "transcript".into(),
                index: 0
            },
            ChunkKey {
                stream_id: "transcript".into(),
                index: 1
            },
            ChunkKey {
                stream_id: "transcript".into(),
                index: 1
            },
            ChunkKey {
                stream_id: "capture-metadata".into(),
                index: 0
            },
        ]
    );
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
}

#[test]
fn cloud_scheduler_replays_stopped_finalization_after_restart_sign_in_or_explicit_retry() {
    for recovery in 0..3 {
        let mut fixture = Fixture::new(FINALIZE_REPLAY, 0);
        for _ in 0..8 {
            fixture.turn();
            if fixture.host.spool().unwrap().entries().unwrap()[0].completion_submitted {
                break;
            }
        }
        let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        assert_eq!(saved.state, CaptureState::AwaitingIntegrity);
        assert!(saved.completion_submitted);
        assert_eq!(fixture.service.finalizations.load(Ordering::Acquire), 1);
        fixture.turn();
        assert_eq!(fixture.service.finalizations.load(Ordering::Acquire), 1);
        fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
        match recovery {
            0 => fixture.restart_scheduler(),
            1 => fixture.sign_in_again(),
            _ => {
                retry_saved_jobs(
                    &fixture.store,
                    fixture.host.spool().unwrap(),
                    &fixture.policy,
                    &|| true,
                )
                .unwrap();
                fixture.runner.finalization_submissions.clear();
                fixture.runner.retry_deadlines.clear();
            }
        }
        fixture.accept_first();
        assert_eq!(fixture.service.finalizations.load(Ordering::Acquire), 2);
        assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
        assert_eq!(
            fixture
                .store
                .cloud_sync_job(&saved.identity.capture_id)
                .unwrap()
                .unwrap()
                .state,
            JobState::Accepted
        );
    }
}

#[test]
fn cloud_scheduler_replays_bound_finalization_once_after_status_and_renewal_conflict() {
    for mode in [STATUS_FINALIZE_REPLAY, STATUS_REPLAY_REJECTED] {
        let mut fixture = Fixture::new(VERIFY_PENDING, 0);
        for _ in 0..8 {
            fixture.turn();
            if fixture.service.finalizations.load(Ordering::Acquire) == 1 {
                break;
            }
        }
        let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        let id = &saved.identity.capture_id;
        assert_eq!(saved.state, CaptureState::AwaitingIntegrity);
        assert!(saved.completion_submitted);
        fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
        fixture.service.mode.store(mode, Ordering::Release);

        fixture.runner.next_integrity_poll = None;
        fixture.turn();
        assert_eq!(fixture.service.renewals.load(Ordering::Acquire), 1);
        assert_eq!(fixture.service.finalizations.load(Ordering::Acquire), 2);
        assert_eq!(fixture.runner.state, State::Fenced);
        assert_eq!(
            fixture.host.spool().unwrap().entries().unwrap(),
            vec![saved.clone()]
        );
        if mode == STATUS_FINALIZE_REPLAY {
            fixture
                .service
                .mode
                .store(VERIFY_PENDING, Ordering::Release);
            fixture.runner.retry_deadlines.get_mut(id).unwrap().0 = Instant::now();
            fixture.turn();
            let pending = fixture.host.spool().unwrap().entries().unwrap().remove(0);
            assert_eq!(pending.state, CaptureState::AwaitingIntegrity);
            assert_eq!(pending.identity, saved.identity);
            assert_eq!(pending.manifest, saved.manifest);
            assert!(!pending.server_status.unwrap().integrity_accepted);
            let observed = fixture.published_status();
            assert_eq!(observed.automatic_recovery_count, Some(1));
            assert_eq!(observed.queued_count, 1);
            assert_eq!(observed.fenced_count, 0);
            assert!(!observed.retry_available);
            assert_eq!(
                fixture.store.cloud_sync_job(id).unwrap().unwrap().state,
                JobState::Queued
            );
            assert_eq!(fixture.service.finalizations.load(Ordering::Acquire), 2);
            assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);

            fixture.service.mode.store(READY, Ordering::Release);
            fixture.accept_first();
            assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
            assert_eq!(
                fixture.store.cloud_sync_job(id).unwrap().unwrap().state,
                JobState::Accepted
            );
        } else {
            for _ in 0..3 {
                fixture.runner.retry_deadlines.get_mut(id).unwrap().0 = Instant::now();
                fixture.turn();
            }
            assert_eq!(fixture.service.renewals.load(Ordering::Acquire), 3);
            assert_eq!(fixture.runner.state, State::Fenced);
            assert_eq!(
                fixture.host.spool().unwrap().entries().unwrap(),
                vec![saved.clone()]
            );
        }
        assert_eq!(fixture.service.finalizations.load(Ordering::Acquire), 2);
        assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    }
}

#[test]
fn cloud_scheduler_bounds_transient_finalization_replays_and_honors_retry_after() {
    for (mode, kind) in [
        (STATUS_REPLAY_OFFLINE, upload::Failure::Offline),
        (STATUS_REPLAY_CAPACITY, upload::Failure::Backpressure),
    ] {
        let mut fixture = Fixture::new(VERIFY_PENDING, 0);
        for _ in 0..8 {
            fixture.turn();
            if fixture.service.finalizations.load(Ordering::Acquire) == 1 {
                break;
            }
        }
        let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        let id = &saved.identity.capture_id;
        assert!(saved.completion_submitted);
        fixture.runner.next_integrity_poll = None;
        fixture.service.mode.store(mode, Ordering::Release);
        for attempt in 1..=FINALIZATION_PROBE_LIMIT {
            fixture.turn();
            assert_eq!(
                fixture.service.finalizations.load(Ordering::Acquire),
                usize::from(attempt) + 1
            );
            assert!(!fixture.runner.conflict_attempts.contains_key(id));
            let (deadline, failure) = fixture.runner.retry_deadlines[id];
            assert_eq!(failure, kind);
            assert!(deadline.saturating_duration_since(Instant::now()) > Duration::from_secs(89));
            let requests = fixture.service.requests.load(Ordering::Acquire);
            fixture.turn();
            assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);
            fixture.runner.retry_deadlines.get_mut(id).unwrap().0 = Instant::now();
        }
        fixture.turn();
        assert_eq!(fixture.service.finalizations.load(Ordering::Acquire), 4);
        assert_eq!(fixture.runner.conflict_attempts[id], 1);
        assert_eq!(
            fixture.host.spool().unwrap().entries().unwrap(),
            vec![saved]
        );
    }
}

#[test]
fn cloud_scheduler_transient_part_errors_wait_and_keep_capacity_backoff_across_sign_in() {
    for mode in [PART_OFFLINE, PART_CAPACITY] {
        let mut fixture = Fixture::new(mode, 0);
        fixture.turn();
        let queued = fixture.published_status();
        assert_eq!(queued.automatic_recovery_count, Some(0));
        assert_eq!(queued.queued_count, 1);
        let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        fixture.turn();
        let delayed = fixture.published_status();
        assert_eq!(delayed.automatic_recovery_count, Some(1));
        assert_eq!(delayed.queued_count, 1);
        assert!(!delayed.retry_available);
        let deadline = fixture.runner.retry_deadlines[&saved.identity.capture_id];
        assert!(deadline.0.saturating_duration_since(Instant::now()) > Duration::from_secs(85));
        let requests = fixture.service.requests.load(Ordering::Acquire);
        fixture.sign_in_again();
        fixture.turn();
        assert_eq!(
            fixture.runner.retry_deadlines[&saved.identity.capture_id],
            deadline
        );
        assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);
        fixture.service.mode.store(READY, Ordering::Release);
        fixture
            .runner
            .retry_deadlines
            .get_mut(&saved.identity.capture_id)
            .unwrap()
            .0 = Instant::now();
        fixture.runner.next_reservation = None;
        fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
        fixture.accept_first();
        assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    }
}

#[test]
fn cloud_scheduler_capability_backoff_survives_a_same_account_sign_in() {
    let mut fixture = Fixture::new(CAPS_CAPACITY, 0);
    fixture.turn();
    let deadline = fixture.runner.next_reservation.unwrap();
    assert!(deadline.saturating_duration_since(Instant::now()) > Duration::from_secs(85));
    let requests = fixture.service.requests.load(Ordering::Acquire);
    fixture.engine.sign_out().unwrap();
    fixture.turn();
    assert_eq!(fixture.runner.next_reservation, Some(deadline));
    fixture.sign_in_again();
    fixture.turn();
    assert_eq!(fixture.runner.next_reservation, Some(deadline));
    assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);
}

#[test]
fn cloud_device_upload_fence_blocks_older_work_until_off_is_saved() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    store.set_cloud_device_upload_enabled(true).unwrap();
    let scheduler = Scheduler::default();
    let admitted_generation = scheduler.generation.load(Ordering::Acquire);
    let earlier_on = scheduler.device_pause.load(Ordering::Acquire);
    let off = scheduler.fence_device_uploads();

    assert_ne!(
        scheduler.generation.load(Ordering::Acquire),
        admitted_generation
    );
    assert!(!scheduler.device_upload_enabled(&store).unwrap());
    assert!(scheduler.finish_device_change(earlier_on).is_err());
    assert!(!scheduler.device_upload_enabled(&store).unwrap());

    store.set_cloud_device_upload_enabled(false).unwrap();
    scheduler.finish_device_change(off).unwrap();
    assert!(!scheduler.device_upload_enabled(&store).unwrap());
}

#[test]
fn cloud_device_upload_resume_requires_the_current_personal_binding() {
    let fixture = Fixture::for_workspace(
        READY,
        0,
        Workspace {
            id: "team".into(),
            name: "Team".into(),
            kind: WorkspaceKind::Team,
        },
    );
    fixture
        .store
        .set_cloud_device_upload_enabled(false)
        .unwrap();
    let scheduler = Scheduler::default();
    let expected_pause = scheduler.fence_device_uploads();
    let resume = |account: &str, workspace: &str, generation, current| {
        device_upload::resume(
            &scheduler,
            &fixture.store,
            &fixture.engine,
            &DestinationBinding {
                account_id: account.into(),
                destination_id: workspace.into(),
            },
            expected_pause,
            generation,
            || current,
        )
    };
    for (account, workspace, generation, current) in [
        ("other", "personal", 0, true),
        ("account", "team", 0, true),
        ("account", "personal", 1, true),
        ("account", "personal", 0, false),
    ] {
        assert!(resume(account, workspace, generation, current).is_err());
        assert!(!fixture.store.cloud_device_upload_enabled().unwrap());
        assert!(!scheduler.device_admission_open());
    }

    resume("account", "personal", 0, true).unwrap();
    assert!(scheduler.device_upload_enabled(&fixture.store).unwrap());
}

#[test]
fn cloud_device_upload_resume_rolls_back_if_sign_out_races_the_device_cas() {
    let fixture = Fixture::new(READY, 0);
    fixture
        .store
        .set_cloud_device_upload_enabled(false)
        .unwrap();
    let scheduler = Scheduler::default();
    let expected_pause = scheduler.fence_device_uploads();
    let checks = std::cell::Cell::new(0);
    let generation = AtomicU64::new(0);
    let result = device_upload::resume(
        &scheduler,
        &fixture.store,
        &fixture.engine,
        &DestinationBinding {
            account_id: "account".into(),
            destination_id: "personal".into(),
        },
        expected_pause,
        0,
        || {
            checks.set(checks.get() + 1);
            if checks.get() == 3 {
                generation.fetch_add(1, Ordering::AcqRel);
            }
            generation.load(Ordering::Acquire) == 0
        },
    );
    assert_eq!(checks.get(), 3);
    assert!(result.is_err());
    assert!(!fixture.store.cloud_device_upload_enabled().unwrap());
    assert_eq!(
        scheduler.device_pause.load(Ordering::Acquire),
        expected_pause
    );
    assert!(!scheduler.device_admission_open());
}

#[test]
fn cloud_device_upload_resume_orders_new_off_requests_against_the_device_cas() {
    let fixture = Fixture::new(READY, 0);
    let binding = DestinationBinding {
        account_id: "account".into(),
        destination_id: "personal".into(),
    };
    for (fence_at_check, resume_accepted) in [(2, false), (3, true)] {
        fixture
            .store
            .set_cloud_device_upload_enabled(false)
            .unwrap();
        let scheduler = Scheduler::default();
        let expected_pause = scheduler.fence_device_uploads();
        let checks = std::cell::Cell::new(0);
        let off = std::cell::Cell::new(None);
        let result = device_upload::resume(
            &scheduler,
            &fixture.store,
            &fixture.engine,
            &binding,
            expected_pause,
            0,
            || {
                let check = checks.get() + 1;
                checks.set(check);
                if check == fence_at_check {
                    off.set(Some(scheduler.fence_device_uploads()));
                }
                true
            },
        );
        assert_eq!(result.is_ok(), resume_accepted);
        assert_eq!(
            fixture.store.cloud_device_upload_enabled().unwrap(),
            resume_accepted
        );
        assert!(!scheduler.device_admission_open());

        fixture
            .store
            .set_cloud_device_upload_enabled(false)
            .unwrap();
        scheduler.finish_device_change(off.get().unwrap()).unwrap();
        assert!(!scheduler.device_upload_enabled(&fixture.store).unwrap());
    }
}

#[test]
fn cloud_device_upload_pause_preserves_personal_and_team_capture_ids() {
    for (id, kind) in [
        ("personal", WorkspaceKind::Personal),
        ("team", WorkspaceKind::Team),
    ] {
        let mut fixture = Fixture::for_workspace(
            READY,
            0,
            Workspace {
                id: id.into(),
                name: id.into(),
                kind,
            },
        );
        fixture.turn();
        let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        let requests = fixture.service.requests.load(Ordering::Acquire);
        fixture
            .store
            .set_cloud_device_upload_enabled(false)
            .unwrap();
        fixture.turn();
        assert_eq!(fixture.runner.state, State::Paused);
        assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);
        let paused = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        assert_eq!(paused.identity, saved.identity);
        assert_eq!(paused.manifest, saved.manifest);

        fixture.store.set_cloud_device_upload_enabled(true).unwrap();
        fixture.accept_first();
        let reserves = fixture.service.reserve_identities.lock().unwrap();
        assert_eq!(reserves.as_slice(), &[saved.identity]);
    }
}

#[test]
fn cloud_scheduler_route_denial_keeps_account_and_saved_upload() {
    for (id, kind) in [
        ("personal", WorkspaceKind::Personal),
        ("team", WorkspaceKind::Team),
    ] {
        let mut fixture = Fixture::for_workspace(
            PART_DENIED,
            0,
            Workspace {
                id: id.into(),
                name: id.into(),
                kind,
            },
        );
        fixture.turn();
        let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        let start = Instant::now();
        let mut observed = Vec::new();
        fixture.turn_published(&mut observed);
        assert_eq!(observed[0].status, State::RouteDenied);
        assert!(observed[0].retry_available);
        assert_eq!(observed[0].destination_id.as_deref(), Some(id));
        assert!(fixture.engine.status == AuthStatus::SignedIn);
        assert!(fixture.engine.session().is_some());
        assert!(fixture.store.cloud_device_upload_enabled().unwrap());
        let deadline = fixture.runner.route_denied_until.unwrap();
        assert!(deadline >= start + Duration::from_secs(300));
        assert!(deadline <= Instant::now() + Duration::from_secs(300));
        let requests = fixture.service.requests.load(Ordering::Acquire);
        fixture.turn();
        fixture.turn();
        assert_eq!(fixture.runner.state, State::RouteDenied);
        assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);
        assert_eq!(fixture.authority.exchanges.load(Ordering::Acquire), 1);

        fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
        fixture.service.mode.store(READY, Ordering::Release);
        fixture.runner.route_denied_until = Some(Instant::now());
        fixture.accept_first();
        assert_eq!(
            fixture
                .service
                .reserve_identities
                .lock()
                .unwrap()
                .as_slice(),
            &[saved.identity]
        );
    }
}

#[test]
fn cloud_scheduler_does_not_continue_discovery_early_during_route_backoff() {
    let mut fixture = Fixture::new(READY, 0);
    let cursor = DiscoveryCursor {
        after: Some("unread-source".into()),
        ..DiscoveryCursor::default()
    };
    fixture.runner.discovery_cursor = Some(cursor.clone());
    fixture.runner.continue_discovery = true;
    assert!(fixture.runner.discovery_continuation_due());
    fixture.runner.route_denied_until = Some(Instant::now() + Duration::from_secs(60));

    fixture.turn();

    assert_eq!(fixture.runner.state, State::RouteDenied);
    assert_eq!(fixture.runner.discovery_cursor, Some(cursor));
    assert!(!fixture.runner.discovery_continuation_due());
}

#[test]
fn cloud_scheduler_capability_denial_waits_before_retrying() {
    let mut fixture = Fixture::new(CAPS_DENIED, 0);
    let start = Instant::now();
    fixture.turn();
    assert_eq!(fixture.runner.state, State::RouteDenied);
    assert!(fixture.runner.route_denied_until.unwrap() >= start + ROUTE_DENIAL_RETRY_DELAY);
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 0);
    fixture.turn();
    assert_eq!(fixture.service.requests.load(Ordering::Acquire), 1);
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.runner.route_denied_until = Some(Instant::now());
    fixture.accept_first();
}

#[test]
fn cloud_scheduler_personal_denial_leaves_team_upload_eligible() {
    let mut fixture = Fixture::new(CAPS_DENIED, 0);
    fixture.authority.other_team.store(true, Ordering::Release);
    fixture
        .store
        .save_cloud_sync_policy_for_membership(
            (
                "account",
                "team",
                &fixture.service.origin,
                &fixture.service.origin,
                Some(("membership-0", "Team")),
            ),
            &[SourceSelection {
                source_kind: SourceKind::ClaudeJsonl,
                path: fixture.root.to_str().unwrap().into(),
            }],
            &fixture.policy.projects,
            &BTreeMap::new(),
            || true,
        )
        .unwrap();
    fixture.turn();
    assert_eq!(fixture.runner.state, State::RouteDenied);
    let personal = std::mem::replace(
        &mut fixture.runner,
        Runner {
            team: Some(("team".into(), "membership-0".into())),
            ..Runner::default()
        },
    );
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(saved.identity.destination_id, "team");
    fixture.runner = personal;
    let requests = fixture.service.requests.load(Ordering::Acquire);
    fixture.turn();
    assert_eq!(fixture.runner.state, State::RouteDenied);
    assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);
}

#[test]
fn cloud_scheduler_retries_denied_finalization_replay_when_status_returns_to_pending() {
    let mut fixture = Fixture::new(VERIFY_PENDING, 0);
    for _ in 0..8 {
        fixture.turn();
        if fixture.service.finalizations.load(Ordering::Acquire) == 1 {
            break;
        }
    }
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert!(saved.completion_submitted);
    fixture
        .service
        .mode
        .store(STATUS_REPLAY_DENIED, Ordering::Release);
    fixture.runner.next_integrity_poll = None;
    fixture.turn();
    assert_eq!(fixture.runner.state, State::RouteDenied);
    assert_eq!(fixture.service.renewals.load(Ordering::Acquire), 1);
    assert_eq!(fixture.service.finalizations.load(Ordering::Acquire), 2);
    assert_eq!(
        fixture.host.spool().unwrap().entries().unwrap(),
        vec![saved.clone()]
    );
    assert!(fixture.engine.status == AuthStatus::SignedIn);

    fixture
        .service
        .mode
        .store(VERIFY_PENDING, Ordering::Release);
    fixture.turn();
    assert_eq!(fixture.service.finalizations.load(Ordering::Acquire), 2);
    fixture.runner.route_denied_until = Some(Instant::now());
    fixture.turn();
    assert_eq!(fixture.service.finalizations.load(Ordering::Acquire), 3);
    assert_eq!(fixture.service.renewals.load(Ordering::Acquire), 1);
    assert_eq!(
        fixture.host.spool().unwrap().entries().unwrap(),
        vec![saved]
    );

    fixture.runner.next_integrity_poll = None;
    fixture.turn();
    assert_eq!(fixture.service.finalizations.load(Ordering::Acquire), 3);
}

#[test]
fn cloud_scheduler_reservation_denial_does_not_retry_source_capture() {
    let mut fixture = Fixture::new(RESERVE_DENIED, 0);
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(fixture.runner.state, State::RouteDenied);
    assert_eq!(saved.state, CaptureState::Preparing);
    assert!(saved.reservation_id.is_none());
    assert!(saved.manifest.is_none());
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.runner.route_denied_until = Some(Instant::now());
    fixture.turn();
    let entries = fixture.host.spool().unwrap().entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].identity, saved.identity);
    assert!(entries[0].manifest.is_none());
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
}

#[test]
fn cloud_scheduler_unauthorized_capture_invalidates_account_access() {
    let mut fixture = Fixture::new(PART_UNAUTHORIZED, 0);
    fixture.turn();
    fixture.turn();
    assert_eq!(fixture.runner.state, State::ReauthRequired);
    assert!(fixture.engine.status == AuthStatus::ReauthRequired);
    assert!(fixture.engine.session().is_none());
    assert!(fixture.runner.route_denied_until.is_none());
}

#[test]
fn cloud_scheduler_permanent_part_rejection_waits_for_explicit_retry() {
    let mut fixture = Fixture::new(PART_REJECTED, 0);
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    fixture.turn();
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&saved.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Interrupted
    );
    let requests = fixture.service.requests.load(Ordering::Acquire);
    fixture
        .store
        .set_cloud_device_upload_enabled(false)
        .unwrap();
    fixture.turn();
    assert_eq!(fixture.runner.state, State::Paused);
    let paused = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(paused.identity, saved.identity);
    assert_eq!(paused.manifest, saved.manifest);
    fixture.restart_scheduler();
    fixture.store.set_cloud_device_upload_enabled(true).unwrap();
    fixture.turn();
    assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&saved.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Interrupted
    );
    retry_saved_jobs(
        &fixture.store,
        fixture.host.spool().unwrap(),
        &fixture.policy,
        &|| true,
    )
    .unwrap();
    fixture.service.mode.store(READY, Ordering::Release);
    fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
    fixture.accept_first();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
}

#[test]
fn cloud_scheduler_missing_capture_does_not_block_a_healthy_source() {
    let mut fixture = Fixture::new(READY, 0);
    let source_id = format!("{}/session", fixture.policy.roots[0].source_scope_id);
    let missing = fixture
        .store
        .enqueue_cloud_sync(
            &fixture.policy,
            &fixture.policy.roots[0].source_scope_id,
            fixture.project.to_str().unwrap(),
            &source_id,
            "missing-source",
        )
        .unwrap()
        .unwrap();
    write_source(
        &fixture.root,
        &fixture.project,
        "healthy",
        "another approved session",
    );
    fixture.accept_first();
    let observed = status(
        &fixture.store,
        &mut fixture.host,
        &fixture.engine,
        fixture.runner.state,
    )
    .unwrap();
    assert!(observed.storage_known);
    assert_eq!(observed.recovery_required_count, 1);
    assert_eq!(observed.interrupted_count, 1);
    assert_eq!(observed.accepted_count, 1);
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    assert_ne!(
        fixture.service.reserve_identities.lock().unwrap()[0],
        missing.identity
    );
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&missing.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Interrupted
    );
}

#[test]
fn cloud_scheduler_local_capacity_wait_survives_restart_and_reuses_the_same_id() {
    let mut fixture = Fixture::new(READY, 0);
    fixture
        .host
        .configure_limits(Limits {
            source_bytes: 128 * 1024 * 1024,
            capture_bytes: 256 * 1024 * 1024,
            queue_entries: 1,
            ..Limits::default()
        })
        .unwrap();
    let candidate = discover_root(SourceKind::ClaudeJsonl, fixture.root.to_str().unwrap(), 512)
        .unwrap()
        .remove(0)
        .in_source_scope(&fixture.policy.roots[0].source_scope_id)
        .unwrap();
    let occupant = CaptureIdentity {
        capture_id: "synthetic-queue-occupant".into(),
        account_id: "account".into(),
        destination_id: "personal".into(),
        source_id: candidate.source_id().into(),
    };
    fixture
        .host
        .spool()
        .unwrap()
        .capture(
            occupant.clone(),
            &candidate,
            MetadataSections::new(),
            &mut Reservation { mutate: None },
        )
        .unwrap();
    fixture.turn();
    let waiting = fixture
        .store
        .cloud_sync_jobs("account", "personal", &fixture.service.origin)
        .unwrap()
        .remove(0);
    assert_eq!(waiting.state, JobState::WaitingForLocalCapacity);
    assert_eq!(fixture.published_status().automatic_recovery_count, Some(1));
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 0);
    fixture.host.spool().unwrap().cancel(&occupant).unwrap();
    fixture
        .store
        .defer_cloud_sync_local_capture(&waiting, now() - 1)
        .unwrap();
    fixture.restart_scheduler();
    fixture.accept_first();
    assert_eq!(
        fixture
            .service
            .reserve_identities
            .lock()
            .unwrap()
            .as_slice(),
        std::slice::from_ref(&waiting.identity)
    );
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&waiting.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Accepted
    );
}

#[test]
fn cloud_scheduler_removing_an_unrelated_folder_does_not_fence_saved_bytes() {
    let mut fixture = Fixture::new(READY, 0);
    let extra = fixture
        ._directory
        .path()
        .canonicalize()
        .unwrap()
        .join("unrelated");
    fs::create_dir(&extra).unwrap();
    let mut projects = fixture.policy.projects.clone();
    projects.push(ProjectSelection {
        project_path: extra.to_str().unwrap().into(),
        ..projects[0].clone()
    });
    projects.sort();
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            &[SourceSelection {
                source_kind: SourceKind::ClaudeJsonl,
                path: fixture.root.to_str().unwrap().into(),
            }],
            &projects,
            &BTreeMap::new(),
        )
        .unwrap();
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    fixture.policy = fixture
        .store
        .set_cloud_sync_project(
            &fixture.policy,
            "unrelated",
            false,
            &projects
                .into_iter()
                .filter(|project| project.project_path == extra.to_str().unwrap())
                .collect(),
            false,
            || true,
        )
        .unwrap();
    fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
    fixture.accept_first();
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&saved.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Accepted
    );
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
}

#[test]
fn cloud_scheduler_accepted_opencode_observation_matches_captured_bytes_not_discovery() {
    let mut fixture = Fixture::new(READY, 0);
    let database = fixture.select_opencode(1);
    fixture
        .runner
        .refresh_candidates(&fixture.policy, &|| true, &mut fixture.cache);
    fixture.schedule_backfill();
    fixture.runner.next_reservation = Some(Instant::now() + Duration::from_secs(60));
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 0);
    let original = fixture.runner.candidates[0].observation.clone();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.execute("INSERT INTO session_message VALUES ('message','session-0000','user',1,'{\"text\":\"new content\"}')", []).unwrap();
    fixture.runner.next_reservation = None;
    fixture.accept_first();
    fixture.turn_published(&mut observed);
    assert_eq!(
        session_progress(observed.last().unwrap()),
        serde_json::json!({
            "uploaded": 1, "total": 1, "blocked": 0, "unsupported": 0,
            "scanIncomplete": false
        })
    );
    let identity = fixture.service.reserve_identities.lock().unwrap()[0].clone();
    let source = fixture
        .store
        .cloud_sync_source(
            "account",
            "personal",
            &fixture.service.origin,
            &identity.source_id,
        )
        .unwrap()
        .unwrap();
    assert_ne!(source.observation_key.as_deref(), Some(original.as_str()));
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    connection
        .execute("DELETE FROM session_message", [])
        .unwrap();
    fixture.runner.next_reconcile = None;
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 2);
}

#[test]
fn cloud_scheduler_closes_refresh_when_a_previously_uploaded_source_disappears() {
    let mut fixture = Fixture::new(READY, 0);
    let database = fixture.select_opencode(1);
    fixture.accept_first();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.execute("INSERT INTO session_message VALUES ('message','session-0000','user',1,'{\"text\":\"changed before scan\"}')", []).unwrap();
    fixture.schedule_backfill();
    fixture.runner.next_reservation = Some(Instant::now() + Duration::from_secs(60));
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    let before = serde_json::to_value(observed.last().unwrap()).unwrap();
    assert_eq!(before["uploadProgress"]["uploaded"], 0);

    connection.execute("UPDATE session_message SET data = '{\"text\":\"changed after scan\"}' WHERE id = 'message'", []).unwrap();
    fixture.runner.next_reservation = None;
    fixture.turn();
    let identity = fixture.service.reserve_identities.lock().unwrap()[1].clone();
    drop(connection);
    fs::remove_file(database).unwrap();

    for _ in 0..10 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
    }
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Accepted
    );
    assert!(
        !fixture
            .runner
            .backfill_progress
            .as_ref()
            .unwrap()
            .needs_refresh()
    );
    assert_eq!(
        session_progress(observed.last().unwrap()),
        serde_json::json!({
            "uploaded": 0, "total": null, "blocked": 1, "unsupported": 0,
            "scanIncomplete": true
        })
    );
}

#[cfg(unix)]
fn select_cursor_chunked_composer(fixture: &mut Fixture) -> rusqlite::Connection {
    let base = fixture._directory.path().canonicalize().unwrap();
    let user = base.join("Cursor/User");
    let workspace = user.join("workspaceStorage/synthetic-workspace");
    let global = user.join("globalStorage");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&global).unwrap();
    fs::write(
        workspace.join("workspace.json"),
        serde_json::json!({ "folder": format!("file://{}", fixture.project.display()) })
            .to_string(),
    )
    .unwrap();
    rusqlite::Connection::open(workspace.join("state.vscdb"))
        .unwrap()
        .execute_batch("CREATE TABLE ItemTable(key TEXT PRIMARY KEY, value BLOB);")
        .unwrap();
    let db = rusqlite::Connection::open(global.join("state.vscdb")).unwrap();
    db.execute_batch("CREATE TABLE cursorDiskKV(key TEXT PRIMARY KEY, value BLOB);")
        .unwrap();
    db.execute_batch(include_str!(
        "../../../../../../fixtures/cloud/cursor/ide-chunked.sql"
    ))
    .unwrap();
    let source_kind = SourceKind::CursorIdeComposer;
    let source_path = workspace.to_str().unwrap();
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            &[SourceSelection {
                source_kind,
                path: source_path.into(),
            }],
            &[ProjectSelection {
                source_kind,
                source_path: source_path.into(),
                project_path: fixture.project.to_str().unwrap().into(),
            }],
            &BTreeMap::new(),
        )
        .unwrap();
    db
}

#[cfg(unix)]
#[test]
fn cloud_scheduler_cursor_chunked_capture_counts_after_integrity_acceptance() {
    let mut fixture = Fixture::new(CURSOR_IDE, 0);
    select_cursor_chunked_composer(&mut fixture);
    fixture.schedule_backfill();
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    let queued = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(queued.state, CaptureState::Queued);
    let manifest = queued.manifest.unwrap();
    assert_eq!(manifest.streams[0].stream_id, "cursor-ide-rows");
    let status = serde_json::to_value(observed.last().unwrap()).unwrap();
    assert_eq!(status["uploadProgress"]["uploaded"], 0);
    assert!(status["uploadTransfer"].is_null());

    fixture.turn_published(&mut observed);
    let transferring = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    let progress = transferring.progress().unwrap();
    assert!(progress.confirmed_upload_bytes > 0);
    assert_eq!(
        serde_json::to_value(observed.last().unwrap()).unwrap()["uploadTransfer"],
        serde_json::json!({
            "sentBytes": progress.confirmed_upload_bytes,
            "totalBytes": progress.required_transfer_bytes,
        })
    );
    for _ in 0..10 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
    }
    let result = observed.last().unwrap();
    assert!(!result.backfill_in_progress);
    assert_eq!(
        session_progress(result),
        serde_json::json!({
            "uploaded": 1, "total": 1, "blocked": 0, "unsupported": 0,
            "scanIncomplete": false
        })
    );
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
}

#[cfg(unix)]
#[test]
fn cloud_scheduler_accepted_cursor_ide_observation_matches_captured_rows() {
    let mut fixture = Fixture::new(CURSOR_IDE, 0);
    let database = select_cursor_chunked_composer(&mut fixture);
    fixture
        .runner
        .refresh_candidates(&fixture.policy, &|| true, &mut fixture.cache);
    let discovered = fixture.runner.candidates[0].observation.clone();
    database
        .execute(
            "UPDATE cursorDiskKV SET value=?1 WHERE key='bubbleId:synthetic-chunked:z-user'",
            [r#"{"bubbleId":"z-user","type":1,"text":"changed synthetic prompt"}"#],
        )
        .unwrap();
    fixture.turn();
    let queued = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    let stream = &queued.manifest.as_ref().unwrap().streams[0];
    assert_eq!(stream.stream_id, "cursor-ide-rows");
    let captured = format!("cursor-ide-native-v1:{}", stream.expanded_sha256);
    assert_ne!(captured, discovered);

    fixture.accept_first();
    let receipt = fixture
        .store
        .cloud_sync_source(
            "account",
            "personal",
            &fixture.service.origin,
            &queued.identity.source_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(receipt.observation_key.as_deref(), Some(captured.as_str()));

    fixture.runner.next_reconcile = None;
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
}

#[test]
fn cloud_scheduler_bad_opencode_content_does_not_block_healthy_uploads() {
    let mut fixture = Fixture::new(READY, 0);
    let database = fixture.select_opencode(3);
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory=?1",
            [fixture.project.to_str().unwrap()],
        )
        .unwrap();
    connection.execute("INSERT INTO session_message VALUES ('broken','session-0001','user',1,?1)",
        [r#"{"files":[{"uri":"media:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#]).unwrap();
    for _ in 0..20 {
        fixture.turn();
        if fixture
            .store
            .cloud_sync_outcome_counts("account", "personal", &fixture.service.origin)
            .unwrap()
            .0
            == 2
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
        2
    );
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 2);
    fixture.turn();
    assert_eq!(fixture.runner.state, State::Fenced);
}

#[test]
fn cloud_scheduler_folder_opt_out_keeps_saved_bytes_without_an_upload_error() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    let directory = fixture
        ._directory
        .path()
        .join("cloud-capture-v1")
        .join(&saved.identity.capture_id);
    let bytes = saved_files(&directory);
    fixture.policy = fixture
        .store
        .set_cloud_sync_project(
            &fixture.policy,
            "synthetic-group",
            false,
            &fixture.policy.projects.iter().cloned().collect(),
            false,
            || true,
        )
        .unwrap();
    let requests = fixture.service.requests.load(Ordering::Acquire);
    fixture.turn();
    let observed = status(
        &fixture.store,
        &mut fixture.host,
        &fixture.engine,
        fixture.runner.state,
    )
    .unwrap();
    assert_eq!(observed.status, State::Ready);
    assert_eq!(observed.queued_count, 0);
    assert_eq!(observed.fenced_count, 0);
    assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);
    assert_eq!(
        fixture.host.spool().unwrap().entries().unwrap(),
        vec![saved]
    );
    assert_eq!(saved_files(&directory), bytes);
}

#[test]
fn cloud_scheduler_renews_idle_parts_and_finalization_without_recapture() {
    for mode in [IDLE_MANIFEST, IDLE_PART, IDLE_FINALIZE] {
        let mut fixture = Fixture::new(mode, 0);
        fixture.turn();
        let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
        fixture.accept_first();
        assert_eq!(fixture.service.renewals.load(Ordering::Acquire), 1);
        assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
        assert_eq!(
            fixture
                .service
                .reserve_identities
                .lock()
                .unwrap()
                .as_slice(),
            std::slice::from_ref(&saved.identity)
        );
        assert_eq!(
            fixture
                .store
                .cloud_sync_job(&saved.identity.capture_id)
                .unwrap()
                .unwrap()
                .state,
            JobState::Accepted,
        );
    }
}

#[test]
fn cloud_scheduler_rechecks_older_fenced_uploads_automatically_with_bounded_conflicts() {
    let mut fixture = Fixture::new(PART_CONFLICT, 0);
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    fixture.turn();
    assert_eq!(fixture.service.renewals.load(Ordering::Acquire), 1);
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&saved.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Fenced
    );
    fixture.turn();
    let requests = fixture.service.requests.load(Ordering::Acquire);
    fixture.turn();
    assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);
    fixture.service.mode.store(IDLE_PART, Ordering::Release);
    fixture.restart_scheduler();
    fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
    fixture.accept_first();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    assert_eq!(fixture.service.renewals.load(Ordering::Acquire), 2);
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&saved.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Accepted
    );
}

#[test]
fn cloud_scheduler_replays_once_when_a_renewed_request_still_conflicts() {
    for mode in [RENEWED_PART_CONFLICT, RENEWED_FINALIZE_CONFLICT] {
        let mut fixture = Fixture::new(mode, 0);
        for _ in 0..8 {
            fixture.turn();
            if fixture.runner.state == State::Fenced {
                break;
            }
        }
        assert_eq!(fixture.runner.state, State::Fenced);
        assert_eq!(fixture.service.renewals.load(Ordering::Acquire), 1);
        assert_eq!(fixture.service.conflicts.load(Ordering::Acquire), 2);
        let requests = fixture.service.requests.load(Ordering::Acquire);
        fixture.turn();
        assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);
    }
}

#[test]
fn cloud_scheduler_recovers_saved_bytes_when_the_conflict_cooldown_expires() {
    let mut fixture = Fixture::new(PART_CONFLICT, 0);
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    fixture.turn();
    assert_eq!(fixture.runner.state, State::Fenced);
    fixture.service.mode.store(IDLE_PART, Ordering::Release);
    fixture.runner.retry_deadlines.insert(
        saved.identity.capture_id.clone(),
        (Instant::now(), upload::Failure::Conflict),
    );
    fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
    fixture.accept_first();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    assert_eq!(fixture.service.renewals.load(Ordering::Acquire), 2);
}

#[test]
fn cloud_scheduler_stops_probing_persistent_conflicts_until_explicit_retry() {
    let mut fixture = Fixture::new(PART_CONFLICT, 0);
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    let id = &saved.identity.capture_id;
    for attempt in 1..=3 {
        fixture.turn();
        assert_eq!(fixture.runner.state, State::Fenced);
        let observed = fixture.published_status();
        let automatic = u32::from(attempt < 3);
        assert_eq!(observed.automatic_recovery_count, Some(automatic));
        assert_eq!(observed.fenced_count, 1 - automatic);
        assert_eq!(observed.retry_available, attempt == 3);
        fixture.runner.retry_deadlines.get_mut(id).unwrap().0 = Instant::now();
    }
    fixture.turn();
    assert_eq!(fixture.service.renewals.load(Ordering::Acquire), 3);
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    fixture.service.mode.store(IDLE_PART, Ordering::Release);
    let retried = retry_saved_jobs(
        &fixture.store,
        fixture.host.spool().unwrap(),
        &fixture.policy,
        &|| true,
    )
    .unwrap();
    fixture.runner.retry(&retried);
    fixture.accept_first();
    assert_eq!(fixture.service.renewals.load(Ordering::Acquire), 4);
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
}

#[test]
fn cloud_scheduler_new_sign_in_retries_saved_uploads_without_waiting_or_recapture() {
    let mut fixture = Fixture::new(PART_CONFLICT, 0);
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    fixture.turn();
    assert_eq!(fixture.runner.state, State::Fenced);
    assert!(fixture.runner.retry_deadlines[&saved.identity.capture_id].0 > Instant::now());
    let original_grant = fixture.engine.session().unwrap().grant_id.clone();
    fixture.engine.sign_out().unwrap();
    fixture.authority.grant.fetch_add(1, Ordering::AcqRel);
    let (_, _listener, attempt) = fixture.engine.begin(now(), 0).unwrap();
    fixture
        .engine
        .callback(
            &attempt,
            auth::Callback::Code("synthetic-new-sign-in".into()),
            now(),
            || true,
        )
        .unwrap();
    assert_ne!(fixture.engine.session().unwrap().grant_id, original_grant);
    fixture.service.mode.store(IDLE_PART, Ordering::Release);
    fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
    fixture.accept_first();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    assert_eq!(
        fixture
            .service
            .reserve_identities
            .lock()
            .unwrap()
            .as_slice(),
        std::slice::from_ref(&saved.identity),
    );
}

#[test]
fn cloud_scheduler_observes_first_grant_while_policy_is_held() {
    let mut fixture = Fixture::new(PART_REJECTED, 0);
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    fixture.turn();
    let id = &saved.identity.capture_id;
    assert_eq!(
        fixture.store.cloud_sync_job(id).unwrap().unwrap().state,
        JobState::Interrupted
    );
    fixture.restart_scheduler();
    let held_policy = fixture
        .store
        .set_cloud_sync_paused(&fixture.policy, true)
        .unwrap();
    let requests = fixture.service.requests.load(Ordering::Acquire);
    fixture.turn();
    assert_eq!(fixture.runner.state, State::Paused);
    fixture.sign_in_again();
    fixture.turn();
    assert_eq!(fixture.runner.state, State::Paused);
    assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);

    fixture
        .store
        .set_cloud_sync_paused(&held_policy, false)
        .unwrap();
    fixture.service.mode.store(READY, Ordering::Release);
    fs::remove_file(fixture.root.join("session.jsonl")).unwrap();
    fixture.turn();
    assert_eq!(
        fixture.store.cloud_sync_job(id).unwrap().unwrap().state,
        JobState::Queued
    );
    assert!(fixture.service.requests.load(Ordering::Acquire) > requests);
    fixture.accept_first();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
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
    let candidate = discover_policy(&fixture.policy, &|| true)
        .unwrap()
        .remove(0)
        .candidate;
    let evidence = candidate
        .project_evidence_with_limits(&metadata_limits())
        .unwrap();
    let legacy_observation = legacy_file_observation(&candidate, &evidence).unwrap();
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
    // Simulate a capacity wait saved by a build that did not pin its observation.
    fixture
        .store
        .lock()
        .execute(
            "UPDATE cloud_sync_job
         SET record_json=json_set(record_json, '$.observation_key', ?1)
         WHERE capture_id=?2",
            [&legacy_observation, &entry.identity.capture_id],
        )
        .unwrap();
    fixture.runner = Runner::default();
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.allow_capacity_retry();
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(saved.identity, entry.identity);
    assert_eq!(saved.state, CaptureState::Queued);
    assert!(
        fixture
            .store
            .cloud_sync_job(&entry.identity.capture_id)
            .unwrap()
            .unwrap()
            .observation_key
            .starts_with(antiburn_cloud_capture::FILE_GENERATION_PREFIX)
    );
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
            &[ProjectSelection {
                project_path: extra.to_str().unwrap().into(),
                ..selected
            }],
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
fn cloud_scheduler_keeps_conflicted_bytes_and_recovers_without_explicit_retry() {
    for mode in [PART_CONFLICT, FINALIZE_CONFLICT] {
        let mut fixture = Fixture::new(mode, 0);
        for _ in 0..8 {
            fixture.turn();
            if fixture.runner.state == State::Fenced {
                break;
            }
        }
        let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
        let job = fixture
            .store
            .cloud_sync_job(&entry.identity.capture_id)
            .unwrap()
            .unwrap();
        assert!(job.allowed(&fixture.policy));
        assert_eq!(job.state, JobState::Fenced);
        assert_eq!(
            entry.state,
            if mode == PART_CONFLICT {
                CaptureState::Queued
            } else {
                CaptureState::AwaitingIntegrity
            }
        );
        assert!(!entry.completion_submitted);
        let directory = fixture
            ._directory
            .path()
            .join("cloud-capture-v1")
            .join(&entry.identity.capture_id);
        let bytes = saved_files(&directory);

        fixture.restart_scheduler();
        fixture.turn();
        let requests = fixture.service.requests.load(Ordering::Acquire);
        fixture.turn();
        assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);
        assert_eq!(
            status(
                &fixture.store,
                &mut fixture.host,
                &fixture.engine,
                fixture.runner.state,
            )
            .unwrap()
            .fenced_count,
            1
        );
        assert_eq!(saved_files(&directory), bytes);

        fixture.service.mode.store(READY, Ordering::Release);
        fixture.restart_scheduler();
        for _ in 0..8 {
            fixture.turn();
            if fixture.host.spool().unwrap().entries().unwrap().is_empty() {
                break;
            }
        }
        assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
        assert_eq!(
            fixture
                .store
                .cloud_sync_job(&entry.identity.capture_id)
                .unwrap()
                .unwrap()
                .state,
            JobState::Accepted
        );
        assert!(!directory.exists());
    }
}

#[test]
fn cloud_scheduler_allows_backfill_between_integrity_polls_even_when_the_deadline_passes() {
    let mut fixture = Fixture::new(VERIFY_PENDING, 0);
    for _ in 0..8 {
        fixture.turn();
        if fixture.service.finalizations.load(Ordering::Acquire) == 1 {
            break;
        }
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
    write_source(&fixture.root, &fixture.project, "another", "synthetic");
    fixture.schedule_backfill();
    assert!(fixture.runner.next_integrity_poll.unwrap() > Instant::now());
    fixture.runner.next_integrity_poll = Some(Instant::now() - Duration::from_secs(1));
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    assert!(fixture.runner.backfill_discovery_due);
    // A slow turn for another workspace can take longer than the poll delay.
    fixture.runner.next_integrity_poll = Some(Instant::now() - Duration::from_secs(1));
    fixture.turn();
    assert!(!fixture.runner.backfill_discovery_due);
    assert_eq!(saved_chunks(&directory), before);
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 2);
    assert_eq!(
        fixture.service.capacity_rejections.load(Ordering::Acquire),
        1
    );
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.runner.next_integrity_poll = None;
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
fn cloud_scheduler_upgrades_issuerless_queued_capture_without_touching_its_spool() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.turn();
    let old = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(old.state, CaptureState::Queued);
    let old_directory = fixture
        ._directory
        .path()
        .join("cloud-capture-v1")
        .join(&old.identity.capture_id);
    let old_chunks = saved_chunks(&old_directory);
    assert!(!old_chunks.is_empty());
    let requests = fixture.service.requests.load(Ordering::Acquire);
    fixture
        .store
        .lock()
        .execute_batch(
            "UPDATE cloud_sync_policy SET record_json=json_remove(record_json, '$.issuerOrigin');
         UPDATE cloud_sync_job SET record_json=json_remove(record_json, '$.issuer_origin');",
        )
        .unwrap();
    let legacy = fixture
        .store
        .cloud_sync_job(&old.identity.capture_id)
        .unwrap()
        .unwrap();
    assert!(legacy.issuer_origin.is_none());
    assert!(!legacy.state.terminal());
    fixture.restart_scheduler();
    fixture.turn();
    assert_eq!(fixture.runner.state, State::ConsentRequired);
    assert_eq!(fixture.service.requests.load(Ordering::Acquire), requests);

    let root = SourceSelection {
        source_kind: SourceKind::ClaudeJsonl,
        path: fixture.root.to_str().unwrap().into(),
    };
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            &[root],
            &fixture.policy.projects,
            &fixture.policy.project_choices,
        )
        .unwrap();
    fixture.restart_scheduler();
    fixture.turn();
    let identities = fixture.service.reserve_identities.lock().unwrap();
    assert_eq!(identities.len(), 2);
    assert_eq!(identities[0], old.identity);
    assert_ne!(identities[1].capture_id, old.identity.capture_id);
    assert_eq!(identities[1].source_id, old.identity.source_id);
    let entries = fixture.host.spool().unwrap().entries().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.contains(&old));
    assert_eq!(saved_chunks(&old_directory), old_chunks);
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&old.identity.capture_id)
            .unwrap(),
        Some(legacy)
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
        fixture.runner.next_reservation = None;
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
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
    let identity = fixture.service.reserve_identities.lock().unwrap()[0].clone();
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&identity.capture_id)
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
    assert_eq!(fixture.host.spool().unwrap().entries().unwrap().len(), 1);
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
fn cloud_scheduler_backfill_stops_when_upload_capabilities_are_unavailable() {
    let mut fixture = Fixture::new(CAPS_MISSING, 0);
    fixture.schedule_backfill();
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    assert_eq!(observed[0].status, State::UploadUnavailable);
    assert!(!observed[0].backfill_in_progress);
    assert_eq!(
        session_progress(&observed[0]),
        serde_json::json!({
            "uploaded": 0, "total": null, "blocked": 0, "unsupported": 0,
            "scanIncomplete": true
        })
    );
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 0);
}

#[test]
fn cloud_scheduler_backfill_keeps_total_unknown_with_incomplete_project_evidence() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.accept_first();
    let outside = fixture.root.clone();
    write_source(
        &fixture.root,
        &outside,
        "partial",
        "outside the saved folder",
    );
    fs::OpenOptions::new()
        .append(true)
        .open(fixture.root.join("partial.jsonl"))
        .unwrap()
        .write_all(b"{\"type\":\n")
        .unwrap();
    fixture.schedule_backfill();

    let mut observed = Vec::new();
    for _ in 0..10 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
    }
    let result = observed.last().unwrap();
    assert!(!result.backfill_in_progress);
    assert_eq!(
        session_progress(result),
        serde_json::json!({
            "uploaded": 1, "total": null, "blocked": 0, "unsupported": 0,
            "scanIncomplete": true
        })
    );
}

#[test]
fn cloud_scheduler_backfill_reports_blocked_sources_and_drains_healthy_work() {
    use crate::store::burn_checks::trace_capture::LogCapture;

    let mut fixture = Fixture::new(CAPS_SMALL, 0);
    write_source(
        &fixture.root,
        &fixture.project,
        "a-oversized",
        &"x".repeat(256 * 1024),
    );
    write_source(
        &fixture.root,
        &fixture.project,
        "b-missing-spool",
        "PRIVATE_SYNTHETIC_SESSION_BODY",
    );
    let scope = &fixture.policy.roots[0].source_scope_id;
    let missing = fixture
        .store
        .enqueue_cloud_sync(
            &fixture.policy,
            scope,
            fixture.project.to_str().unwrap(),
            &format!("{scope}/b-missing-spool"),
            "PRIVATE_SYNTHETIC_SOURCE_OBSERVATION",
        )
        .unwrap()
        .unwrap();
    fixture.schedule_backfill();
    let mut observed = Vec::new();
    let trace = LogCapture::default();
    tracing::subscriber::with_default(trace.clone(), || {
        for _ in 0..12 {
            fixture.runner.next_integrity_poll = None;
            fixture.turn_published(&mut observed);
        }
    });
    let result = observed.last().unwrap();
    assert!(result.storage_known);
    assert!(!result.backfill_in_progress);
    assert_eq!(result.recovery_required_count, 1);
    assert_eq!(
        session_progress(result),
        serde_json::json!({
            "uploaded": 1, "total": 3, "blocked": 2, "unsupported": 0,
            "scanIncomplete": false
        })
    );
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    let saved = fixture
        .store
        .cloud_sync_job(&missing.identity.capture_id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.identity, missing.identity);
    assert_eq!(saved.state, JobState::Interrupted);
    assert_eq!(trace.events("cloud_backfill_saved_job_decision").len(), 1);
    assert_eq!(trace.events("cloud_backfill_receipt_outcome").len(), 1);
    let events: Vec<_> = trace
        .events("cloud_backfill_scan_summary")
        .into_iter()
        .filter(|event| event.fields["outcome"] == "\"finished\"")
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].level, tracing::Level::INFO);
    assert_eq!(events[0].fields["discovered"], "3");
    assert_eq!(events[0].fields["blocked"], "2");
    assert!(events[0].fields["decisions"].contains("saved_job_blocked"));
    let output = trace.text();
    for private in [
        "PRIVATE_SYNTHETIC_SESSION_BODY",
        "PRIVATE_SYNTHETIC_SOURCE_OBSERVATION",
        "b-missing-spool",
        fixture.root.to_str().unwrap(),
        &missing.identity.capture_id,
    ] {
        assert!(!output.contains(private));
    }
}

#[test]
fn cloud_scheduler_backfill_uses_healthy_paths_in_the_same_source_root() {
    let mut fixture = Fixture::new(READY, 0);
    let unavailable = fixture.project.join("unavailable");
    fs::create_dir(&unavailable).unwrap();
    let stale = ProjectSelection {
        project_path: unavailable.to_str().unwrap().into(),
        ..fixture.policy.projects[0].clone()
    };
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            &[SourceSelection {
                source_kind: SourceKind::ClaudeJsonl,
                path: fixture.root.to_str().unwrap().into(),
            }],
            &[fixture.policy.projects[0].clone(), stale],
            &BTreeMap::new(),
        )
        .unwrap();
    fs::remove_dir(unavailable).unwrap();
    fixture.schedule_backfill();
    let mut observed = Vec::new();
    for _ in 0..10 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
    }
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    assert_eq!(
        session_progress(observed.last().unwrap()),
        serde_json::json!({
            "uploaded": 1, "total": null, "blocked": 0, "unsupported": 0,
            "scanIncomplete": true
        })
    );
}

#[test]
fn cloud_scheduler_backfill_removes_a_source_that_moves_outside_consent() {
    let mut fixture = Fixture::new(READY, 0);
    fixture
        .runner
        .refresh_candidates(&fixture.policy, &|| true, &mut fixture.cache);
    fixture.schedule_backfill();
    fixture.runner.next_reservation = Some(Instant::now() + Duration::from_secs(60));
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    assert_eq!(fixture.runner.candidates.len(), 1);

    write_source(
        &fixture.root,
        &fixture.root,
        "session",
        "synthetic source moved outside the approved folder",
    );
    fixture.runner.next_reservation = None;
    fixture.turn_published(&mut observed);
    let result = observed.last().unwrap();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 0);
    assert!(!result.backfill_in_progress);
    assert_eq!(
        session_progress(result),
        serde_json::json!({
            "uploaded": 0, "total": 0, "blocked": 0, "unsupported": 0,
            "scanIncomplete": false
        })
    );
}

#[test]
fn cloud_scheduler_claude_append_before_pin_cannot_stamp_an_older_observation() {
    let mut fixture = Fixture::new(READY, 0);
    fixture
        .runner
        .refresh_candidates(&fixture.policy, &|| true, &mut fixture.cache);
    fixture.schedule_backfill();
    fixture.runner.next_reservation = Some(Instant::now() + Duration::from_secs(60));
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    let selection = &fixture.runner.candidates[0];
    let source_id = selection.candidate.source_id().to_owned();
    let old = selection.observation.clone();
    let path = fixture.root.join("session.jsonl");
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "{}",
        serde_json::json!({
            "sessionId": "session", "cwd": fixture.project, "type": "user",
            "message": {"role": "user", "content": "new synthetic message"}
        })
    )
    .unwrap();
    let new = antiburn_cloud_capture::file_generation_key(&[fs::metadata(path).unwrap()]).unwrap();
    assert_ne!(old, new);

    fixture.runner.next_reservation = None;
    fixture.turn_published(&mut observed);
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 0);
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
    assert_eq!(observed.last().unwrap().status, State::Syncing);
    assert_eq!(observed.last().unwrap().interrupted_count, 0);
    assert_eq!(
        session_progress(observed.last().unwrap()),
        serde_json::json!({
            "uploaded": 0, "total": 1, "blocked": 0, "unsupported": 0,
            "scanIncomplete": false
        })
    );

    for _ in 0..10 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
    }
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    let receipt = fixture
        .store
        .cloud_sync_source("account", "personal", &fixture.service.origin, &source_id)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.observation_key.as_deref(), Some(new.as_str()));
    assert_eq!(
        serde_json::to_value(observed.last().unwrap()).unwrap()["uploadProgress"]["uploaded"],
        1
    );
    fixture.runner.next_reconcile = None;
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
}

#[test]
fn cloud_scheduler_backfill_tracks_the_requested_grant_and_retry_keeps_progress() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.accept_first();
    fixture.schedule_backfill();
    let old_status = status(
        &fixture.store,
        &mut fixture.host,
        &fixture.engine,
        State::Ready,
    )
    .unwrap();
    fixture.sign_in_again();
    let grant_id = fixture.engine.session().unwrap().grant_id.clone();
    let status_before_restart = fixture
        .runner
        .backfill_status(old_status.clone(), Some(&grant_id));
    assert!(status_before_restart.upload_progress.is_none());
    assert!(!status_before_restart.backfill_in_progress);

    fixture.schedule_backfill();
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    let progress = observed[0].upload_progress.clone();
    assert!(progress.is_some());
    assert_eq!(
        fixture
            .runner
            .retry_status(&BTreeSet::new(), &grant_id, |_, _| Ok(old_status))
            .unwrap()
            .upload_progress,
        progress
    );
}

#[test]
fn cloud_scheduler_backfill_does_not_reoffer_a_saved_job_accepted_during_the_scan() {
    let mut fixture = Fixture::new(VERIFY_PENDING, 0);
    for _ in 0..8 {
        fixture.turn();
        if fixture.service.finalizations.load(Ordering::Acquire) == 1 {
            break;
        }
    }
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(saved.state, CaptureState::AwaitingIntegrity);
    assert!(saved.completion_submitted);
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.restart_scheduler();
    fixture.schedule_backfill();
    let mut observed = Vec::new();
    for _ in 0..12 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
    }
    assert_eq!(
        fixture
            .service
            .reserve_identities
            .lock()
            .unwrap()
            .as_slice(),
        &[saved.identity]
    );
    assert_eq!(
        session_progress(observed.last().unwrap()),
        serde_json::json!({
            "uploaded": 1, "total": 1, "blocked": 0, "unsupported": 0,
            "scanIncomplete": false
        })
    );
}

#[test]
fn cloud_scheduler_reconciles_more_than_100_bindings_across_two_roots() {
    let mut fixture = Fixture::new(READY, 0);
    let base = fixture._directory.path().canonicalize().unwrap();
    let other_root = base.join("other-sources");
    fs::create_dir(&other_root).unwrap();
    let mut paths = vec![fixture.project.clone()];
    for index in 1..78 {
        let path = base.join(format!("worktree-{index:02}"));
        fs::create_dir(&path).unwrap();
        paths.push(path);
    }
    write_source(&other_root, &paths[77], "other", "second source root");
    write_source(&fixture.root, &paths[77], "outside", "wrong source root");
    let roots = [&fixture.root, &other_root].map(|root| SourceSelection {
        source_kind: SourceKind::ClaudeJsonl,
        path: root.to_str().unwrap().into(),
    });
    let mut projects = Vec::new();
    for root in &roots {
        for path in &paths {
            if root == &roots[0] && path == &paths[77] {
                continue;
            }
            projects.push(ProjectSelection {
                source_kind: root.source_kind,
                source_path: root.path.clone(),
                project_path: path.to_str().unwrap().into(),
            });
        }
    }
    projects.sort();
    assert_eq!(projects.len(), 155);
    validate_project_directories(projects.iter()).unwrap();
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            &roots,
            &projects,
            &BTreeMap::new(),
        )
        .unwrap();
    fixture.restart_scheduler();
    assert_eq!(
        fixture
            .store
            .cloud_sync_policy("account", "personal", &fixture.service.origin)
            .unwrap(),
        Some(fixture.policy.clone())
    );
    fixture.accept_count(2);
    let identities = fixture.service.reserve_identities.lock().unwrap();
    assert_eq!(identities.len(), 2);
    assert!(identities[0].source_id.ends_with("/session"));
    assert!(identities[1].source_id.ends_with("/other"));
}

#[test]
fn cloud_scheduler_backfill_only_offers_new_and_changed_sources() {
    let mut fixture = Fixture::new(SUPPRESSED, 0);
    fixture.turn();
    write_source(
        &fixture.root,
        &fixture.project,
        "second",
        "already uploaded",
    );
    fixture.runner.next_reconcile = None;
    fixture.accept_first();
    write_source(&fixture.root, &fixture.project, "updated", "first version");
    fixture.runner.next_reconcile = None;
    fixture.accept_count(2);
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 3);
    assert_eq!(
        fixture
            .store
            .cloud_sync_outcome_counts("account", "personal", &fixture.service.origin)
            .unwrap(),
        (2, 1)
    );

    write_source(
        &fixture.root,
        &fixture.project,
        "updated",
        "second version with new content",
    );
    write_source(
        &fixture.root,
        &fixture.project,
        "session",
        "deleted source changed",
    );
    let older = fixture.root.join("third.jsonl");
    fs::write(
        &older,
        format!(
            "{}\n",
            serde_json::json!({
                "sessionId": "third", "cwd": fixture.project, "version": "2.1.55", "type": "user",
                "timestamp": "2000-01-01T00:00:00.000Z",
                "message": {"role": "user", "content": "older retained synthetic session"},
            }),
        ),
    )
    .unwrap();
    fs::File::open(&older)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(946_684_800)),
        )
        .unwrap();
    let other = fixture.project.parent().unwrap().join("other-project");
    write_source(&fixture.root, &other, "outside", "not approved");
    fixture
        .store
        .set_cloud_device_upload_enabled(false)
        .unwrap();
    fixture.runner.discovery_cursor = Some(DiscoveryCursor {
        root: 0,
        after: Some("zzzz".into()),
        ..DiscoveryCursor::default()
    });
    fixture.runner.next_reconcile = Some(Instant::now() + Duration::from_secs(60));
    fixture.schedule_backfill();
    assert!(fixture.runner.discovery_cursor.is_none());
    assert!(fixture.runner.next_reconcile.is_none());
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 3);
    fixture.store.set_cloud_device_upload_enabled(true).unwrap();
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    assert!(observed[0].backfill_in_progress);
    assert_eq!(
        session_progress(&observed[0]),
        serde_json::json!({
            "uploaded": 1, "total": null, "found": 3, "blocked": 0,
            "unsupported": 0, "scanIncomplete": false
        })
    );
    fixture.schedule_backfill();
    for _ in 0..10 {
        fixture.turn();
    }
    fixture.runner.next_reconcile = None;
    observed.clear();
    fixture.turn_published(&mut observed);
    assert!(!observed[0].backfill_in_progress);
    assert_eq!(
        session_progress(&observed[0]),
        serde_json::json!({
            "uploaded": 3, "total": 3, "blocked": 0, "unsupported": 0,
            "scanIncomplete": false
        })
    );
    let mut changed_policy = observed.remove(0);
    changed_policy.policy_revision = Some("other-revision".into());
    assert!(
        fixture
            .runner
            .backfill_status(
                changed_policy,
                Some(&fixture.engine.session().unwrap().grant_id),
            )
            .upload_progress
            .is_none()
    );
    let reservations = fixture.service.reserve_identities.lock().unwrap();
    assert_eq!(reservations.len(), 5);
    assert!(reservations[3].source_id.ends_with("/third"));
    assert_eq!(reservations[4].source_id, reservations[2].source_id);
    assert_ne!(reservations[4].capture_id, reservations[2].capture_id);
    assert_eq!(
        fixture
            .store
            .cloud_sync_outcome_counts("account", "personal", &fixture.service.origin)
            .unwrap(),
        (3, 1)
    );
}

#[test]
fn cloud_scheduler_backfill_accounts_for_96_sources_across_pages() {
    let mut fixture = Fixture::new(READY, 0);
    let database = fixture.select_opencode(603);
    let connection = rusqlite::Connection::open(database).unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory=?1 WHERE id='session-0602'",
            [fixture
                .project
                .parent()
                .unwrap()
                .join("unselected-project")
                .to_str()
                .unwrap()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory=?1 WHERE id BETWEEN 'session-0507' AND 'session-0512'",
            [fixture.project.to_str().unwrap()],
        )
        .unwrap();
    let accepted = |fixture: &Fixture| {
        fixture
            .store
            .cloud_sync_outcome_counts("account", "personal", &fixture.service.origin)
            .unwrap()
            .0
    };
    for _ in 0..96 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn();
        if accepted(&fixture) == 6 {
            break;
        }
    }
    assert_eq!(accepted(&fixture), 6);

    connection
        .execute(
            "UPDATE session_v2 SET directory=?1 WHERE id >= 'session-0507'",
            [fixture.project.to_str().unwrap()],
        )
        .unwrap();
    drop(connection);
    fixture.restart_scheduler();
    fixture.schedule_backfill();
    let mut observed = Vec::new();
    for _ in 0..900 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
        if accepted(&fixture) == 95 {
            break;
        }
    }
    assert_eq!(accepted(&fixture), 95);
    assert!(observed[0].backfill_in_progress);
    assert!(serde_json::to_value(&observed[0]).unwrap()["uploadProgress"]["total"].is_null());

    fixture
        .service
        .mode
        .store(VERIFY_PENDING, Ordering::Release);
    for _ in 0..10 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
    }
    let result = observed.last().unwrap();
    assert!(!result.backfill_in_progress);
    assert_eq!(
        session_progress(result),
        serde_json::json!({
            "uploaded": 95, "total": 96, "blocked": 0, "unsupported": 0,
            "scanIncomplete": false
        })
    );
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.runner.next_integrity_poll = None;
    fixture.turn_published(&mut observed);
    assert_eq!(accepted(&fixture), 96);
    assert_eq!(
        serde_json::to_value(observed.last().unwrap()).unwrap()["uploadProgress"]["uploaded"],
        96
    );
    let reservations = fixture.service.reserve_identities.lock().unwrap();
    assert_eq!(reservations.len(), 96);
    assert_eq!(
        reservations
            .iter()
            .map(|reservation| &reservation.source_id)
            .collect::<BTreeSet<_>>()
            .len(),
        96
    );
}

#[test]
fn cloud_scheduler_finishes_empty_recovery_only_after_saved_user_intent() {
    let mut fixture = Fixture::new(RESERVE_OFFLINE, 0);
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    let job = fixture
        .store
        .cloud_sync_job(&saved.identity.capture_id)
        .unwrap()
        .unwrap();
    fixture.restart_scheduler();
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    assert_eq!(fixture.host.spool().unwrap().entries().unwrap().len(), 1);
    let manual = fixture.published_status();
    assert_eq!(manual.automatic_recovery_count, Some(0));
    assert_eq!(manual.interrupted_count, 1);
    assert!(manual.retry_available);
    fixture
        .store
        .set_cloud_sync_job_state(&job, JobState::Recovering)
        .unwrap();
    let automatic = fixture.published_status();
    assert_eq!(automatic.automatic_recovery_count, Some(1));
    assert_eq!(automatic.interrupted_count, 0);
    assert!(!automatic.retry_available);
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&saved.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Abandoned
    );
}

#[test]
fn cloud_scheduler_automatic_recovery_requires_a_valid_local_recovery_entry() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    let job = fixture
        .store
        .cloud_sync_job(&saved.identity.capture_id)
        .unwrap()
        .unwrap();
    fixture
        .store
        .set_cloud_sync_job_state(&job, JobState::Recovering)
        .unwrap();
    fixture.turn();
    let observed = fixture.published_status();
    assert_eq!(observed.automatic_recovery_count, Some(0));
    assert_eq!(observed.interrupted_count, 1);
    assert_eq!(
        fixture.host.spool().unwrap().entries().unwrap(),
        vec![saved]
    );
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

fn scheduler_git(directory: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .env_clear()
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}");
}

fn init_scheduler_checkout(directory: &Path) {
    scheduler_git(directory, &["init", "--initial-branch=main"]);
    scheduler_git(
        directory,
        &[
            "-c",
            "user.name=Synthetic",
            "-c",
            "user.email=synthetic@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "synthetic",
        ],
    );
}

fn saved_capture_metadata(directory: &Path) -> serde_json::Value {
    saved_chunks(directory)
        .keys()
        .find_map(|name| {
            let expanded = std::process::Command::new("gzip")
                .arg("-dc")
                .arg(directory.join(name))
                .output()
                .unwrap();
            assert!(expanded.status.success());
            serde_json::from_slice::<serde_json::Value>(&expanded.stdout)
                .ok()
                .filter(|body| body["sections"].is_object())
        })
        .expect("saved capture metadata")
}

#[test]
fn cloud_scheduler_git_change_keeps_saved_capture_and_does_not_recapture() {
    let mut fixture = Fixture::new(READY, 0);
    let project = fixture.project.clone();
    init_scheduler_checkout(&project);
    fixture.turn();
    let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(entry.state, CaptureState::Queued);
    let directory = fixture
        ._directory
        .path()
        .join("cloud-capture-v1")
        .join(&entry.identity.capture_id);
    let chunks = saved_chunks(&directory);
    assert_eq!(chunks.len(), 2);
    assert_eq!(
        saved_capture_metadata(&directory)["sections"]["antiburn.git-checkout"]["data"]["branch"],
        "main"
    );

    scheduler_git(&project, &["branch", "-m", "different-branch"]);
    fixture.restart_scheduler();
    fixture.turn();
    assert_eq!(saved_chunks(&directory), chunks);
    fixture.accept_first();

    fixture.restart_scheduler();
    fixture.turn();
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
}

#[test]
fn cloud_scheduler_git_parent_outside_selected_folder_is_not_uploaded() {
    let mut fixture = Fixture::new(READY, 0);
    init_scheduler_checkout(fixture.project.parent().unwrap());
    fixture.turn();
    let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    assert_eq!(entry.state, CaptureState::Queued);
    let directory = fixture
        ._directory
        .path()
        .join("cloud-capture-v1")
        .join(&entry.identity.capture_id);
    assert!(
        saved_capture_metadata(&directory)["sections"]
            .get("antiburn.git-checkout")
            .is_none()
    );
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
    fixture.schedule_backfill();
    assert!(
        retry_saved_jobs(
            &fixture.store,
            fixture.host.spool().unwrap(),
            &fixture.policy,
            &|| true
        )
        .is_ok()
    );
    assert!(
        fixture
            .runner
            .turn(
                &fixture.store,
                &mut fixture.host,
                &mut fixture.engine,
                &|| true,
                (&|| true, None),
                &mut fixture.cache
            )
            .is_ok()
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
        .is_ok()
    );
    assert!(
        fixture
            .runner
            .turn(
                &fixture.store,
                &mut fixture.host,
                &mut fixture.engine,
                &|| true,
                (&|| true, None),
                &mut fixture.cache
            )
            .is_ok()
    );
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&job.identity.capture_id)
            .unwrap()
            .unwrap(),
        Job {
            state: JobState::Interrupted,
            ..job
        }
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
        .is_ok()
    );
    assert!(
        fixture
            .runner
            .turn(
                &fixture.store,
                &mut fixture.host,
                &mut fixture.engine,
                &|| true,
                (&|| true, None),
                &mut fixture.cache
            )
            .is_ok()
    );
    assert_eq!(
        fixture
            .store
            .cloud_sync_job(&saved.identity.capture_id)
            .unwrap()
            .unwrap(),
        Job {
            state: JobState::Interrupted,
            ..job
        }
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
