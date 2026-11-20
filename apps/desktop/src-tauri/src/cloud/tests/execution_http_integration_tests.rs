use super::*;
use antiburn_cloud_capture::{
    CaptureIdentity, CaptureManifest, Chunk, ChunkKey, Transport as _, UploadPlan,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    sync::Arc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

struct Request {
    line: String,
    headers: std::collections::BTreeMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut std::net::TcpStream) -> Request {
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
    let headers = lines
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (name, value) = line.split_once(':').unwrap();
            (name.to_ascii_lowercase(), value.trim().to_owned())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let length = headers
        .get("content-length")
        .map_or(0, |value| value.parse().unwrap());
    assert!(length < 32_768);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).unwrap();
    Request {
        line,
        headers,
        body,
    }
}

fn server(status: u16, body: String, extra: &str) -> (String, JoinHandle<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let extra = extra.to_owned();
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("synthetic accept failed: {:?}", error.kind()),
            }
        };
        let request = read_request(&mut stream);
        write!(stream, "HTTP/1.1 {status} Synthetic\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{extra}\r\n{body}", body.len()).unwrap();
        request
    });
    (origin, worker)
}

fn connected(directory: &Path, origin: &str) -> Engine {
    let config = Config::parse(origin, origin).unwrap();
    let mut session = super::super::tests::session();
    session.resource = origin.into();
    session.iss = origin.into();
    let mut engine = Engine::new(
        Some(config),
        Box::new(Arc::new(super::super::tests::MemoryVault::default())),
        Box::new(auth::HttpTransport),
        StateStore::new(directory),
    )
    .with_clock(|| 100);
    let record = engine
        .state
        .lock()
        .unwrap()
        .replace(Phase::Active, Cleanup::None)
        .unwrap();
    engine.access = Some(Access {
        token: "synthetic-api-access".into(),
        expires_at: 900,
        epoch: record.epoch,
        session,
    });
    engine.status = AuthStatus::SignedIn;
    engine
}

#[test]
fn cloud_capture_only_account_bearer_reserves_only_the_exact_team_membership() {
    assert_team_reservation("capture:write");
}

#[test]
fn cloud_capture_and_session_account_bearer_reserves_only_the_exact_team_membership() {
    assert_team_reservation("capture:write sessions:list sessions:read");
}

fn assert_team_reservation(scope: &'static str) {
    use super::super::upload::{HttpTransport, Source};
    use crate::cloud::auth::Vault;
    use antiburn_cloud_capture::TransportError;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let service_origin = origin.clone();
    let team_id = "12345678-1234-1234-1234-123456789abc";
    let personal_id = "00000000-0000-0000-0000-000000000001";
    let generation = "opaque-membership";
    let worker = std::thread::spawn(move || {
        let workspaces = serde_json::json!({"workspaces":[
            {"workspace_id":personal_id,"name":"Personal","type":"personal","membership_generation":null},
            {"workspace_id":team_id,"name":"Example Team","type":"team","membership_generation":generation}
        ]});
        let responses = [
            (
                "/v1/devices/token",
                serde_json::json!({
                    "access_token":"account-bearer", "refresh_token":"account-refresh", "expires_in": 200,
                    "token_type":"Bearer", "grant_id":"account-grant", "user_id":"synthetic-user",
                    "grant_expires_at":"1970-01-01T02:46:40Z",
                    "scope":scope, "resource":service_origin, "iss":service_origin,
                }),
            ),
            (
                "/v1/devices/session",
                serde_json::json!({
                    "user_id":"synthetic-user", "display_name":"Example",
                    "grant_id":"account-grant", "device_name":"Example",
                    "scope":scope, "resource":service_origin, "iss":service_origin,
                    "grant_expires_at":"1970-01-01T02:46:40Z", "access_expires_at":"1970-01-01T00:05:00Z",
                }),
            ),
            ("/v1/captures/destinations", workspaces.clone()),
            ("/v1/captures/destinations", workspaces.clone()),
            ("/v1/captures/destinations", workspaces),
            (
                "/v1/captures/reserve",
                serde_json::json!({"reservation_id":"team-reservation"}),
            ),
        ];
        listener.set_nonblocking(true).unwrap();
        for (index, (path, body)) in responses.into_iter().enumerate() {
            let deadline = Instant::now() + Duration::from_secs(4);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(3))
                    }
                    Err(error) => panic!("Mock team request was not sent: {}", error.kind()),
                }
            };
            let request = read_request(&mut stream);
            assert_eq!(
                request.line,
                format!(
                    "{} {path} HTTP/1.1",
                    if matches!(index, 0 | 5) {
                        "POST"
                    } else {
                        "GET"
                    }
                )
            );
            assert!(!request.headers.contains_key("cookie"));
            assert!(!request.headers.contains_key("origin"));
            assert_eq!(
                request.headers.get("authorization").map(String::as_str),
                (index != 0).then_some("Bearer account-bearer")
            );
            if index == 5 {
                let body = serde_json::from_slice::<serde_json::Value>(&request.body).unwrap();
                assert_eq!(body["identity"]["destination_id"], team_id);
                assert_eq!(body["membership_generation"], generation);
            }
            let body = body.to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let config = Config::parse(&origin, &origin).unwrap();
    let vault = Arc::new(super::super::tests::MemoryVault::default());
    let mut engine = Engine::new(
        Some(config),
        Box::new(vault.clone()),
        Box::new(auth::HttpTransport),
        StateStore::new(directory.path()),
    )
    .with_clock(|| 100);
    let (url, _listener, attempt) = engine.begin(100, 1).unwrap();
    assert!(!url.contains("workspace_id"));
    engine
        .callback(
            &attempt,
            auth::Callback::Code("account-code".into()),
            101,
            || true,
        )
        .unwrap();
    assert_eq!(engine.session().unwrap().workspace.id, personal_id);
    assert_eq!(
        vault.read().unwrap().unwrap().refresh_token,
        "account-refresh"
    );
    let target = CaptureIdentity {
        destination_id: team_id.into(),
        ..identity()
    };
    for (generation, expected) in [
        ("wrong-membership", Err(TransportError::Unauthorized)),
        (generation, Ok("team-reservation".into())),
    ] {
        let mut transport = HttpTransport::new(
            &mut engine,
            &|| true,
            &|| true,
            &origin,
            Some(Source {
                agent: "codex",
                source_format: "CodexJsonl",
                native_session_id: "synthetic".into(),
            }),
        )
        .with_membership_generation(Some(generation));
        assert_eq!(transport.reserve(&target, 1024), expected);
    }
    assert!(engine.session().is_some());
    worker.join().unwrap();
}

fn identity() -> CaptureIdentity {
    CaptureIdentity {
        capture_id: "capture-synthetic".into(),
        account_id: "synthetic-user".into(),
        destination_id: "synthetic-personal".into(),
        source_id: "scope/session".into(),
    }
}

#[test]
fn cloud_execution_http_reserve_uses_exact_device_route_and_api_bearer() {
    use super::super::upload::{HttpTransport, Source};
    let (origin, server) = server(200, r#"{"reservation_id":"reservation"}"#.into(), "");
    let directory = tempfile::tempdir().unwrap();
    let mut engine = connected(directory.path(), &origin);
    let mut transport = HttpTransport::new(
        &mut engine,
        &|| true,
        &|| true,
        &origin,
        Some(Source {
            agent: "claude-code",
            source_format: "ClaudeJsonl",
            native_session_id: "session".into(),
        }),
    );
    assert_eq!(
        transport.reserve(&identity(), 123_456).unwrap(),
        "reservation"
    );
    let request = server.join().unwrap();
    assert_eq!(request.line, "POST /v1/captures/reserve HTTP/1.1");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer synthetic-api-access")
    );
    assert!(!request.headers.contains_key("cookie"));
    assert!(!request.headers.contains_key("origin"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
        serde_json::json!({
            "identity": identity(), "maximum_expanded_bytes": 123_456,
            "agent": "claude-code", "source_format": "ClaudeJsonl", "native_session_id": "session",
            "membership_generation": null,
        })
    );
}

#[test]
fn cloud_execution_http_reserve_capacity_requires_the_flat_typed_error() {
    use super::super::upload::{HttpTransport, Source};
    use antiburn_cloud_capture::TransportError;
    let valid = serde_json::json!({
        "code": "capture_limit", "message": "Any safe explanation.",
        "retryable": true, "requestId": "synthetic-request",
    });
    for (body, expected) in [
        (valid.clone(), TransportError::Backpressure),
        (serde_json::json!({"error": valid}), TransportError::Offline),
        (
            serde_json::json!({"message":"capture_limit", "retryable":true}),
            TransportError::Offline,
        ),
        (
            serde_json::json!({"code":"capture_limit","message":null,"retryable":true,"requestId":"synthetic"}),
            TransportError::Offline,
        ),
        (
            serde_json::json!({"code":"capture_limit","message":"capacity","retryable":false,"requestId":"synthetic"}),
            TransportError::Offline,
        ),
    ] {
        let (origin, worker) = server(429, body.to_string(), "Retry-After: 73\r\n");
        let directory = tempfile::tempdir().unwrap();
        let mut engine = connected(directory.path(), &origin);
        let mut transport = HttpTransport::new(
            &mut engine,
            &|| true,
            &|| true,
            &origin,
            Some(Source {
                agent: "claude-code",
                source_format: "ClaudeJsonl",
                native_session_id: "session".into(),
            }),
        );
        assert_eq!(transport.reserve(&identity(), 100), Err(expected));
        assert_eq!(transport.retry_after, Some(Duration::from_secs(73)));
        worker.join().unwrap();
    }
}

#[test]
fn cloud_execution_http_retry_after_accepts_dates_and_bounds_untrusted_waits() {
    use super::super::upload::Route;
    let future = (time::OffsetDateTime::now_utc() + time::Duration::seconds(120))
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap();
    for (value, range) in [
        ("0".to_owned(), Some(1..=1)),
        ("999999".to_owned(), Some(300..=300)),
        (future, Some(110..=120)),
        ("Sun, 06 Nov 1994 08:49:37 GMT".to_owned(), Some(1..=1)),
        ("not a delay".to_owned(), None),
        ("-20".to_owned(), None),
    ] {
        let (origin, worker) = server(503, "{}".into(), &format!("Retry-After: {value}\r\n"));
        let directory = tempfile::tempdir().unwrap();
        let mut engine = connected(directory.path(), &origin);
        let response = engine
            .capture_request(
                &identity(),
                (&origin, None),
                Route::Capabilities,
                &(),
                &|| true,
                &|| true,
            )
            .unwrap();
        match range {
            Some(range) => assert!(range.contains(&response.retry_after.unwrap().as_secs())),
            None => assert!(response.retry_after.is_none()),
        }
        worker.join().unwrap();
    }
}

#[test]
fn cloud_execution_http_status_queries_frozen_plan_without_json_body() {
    use super::super::upload::Route;
    let (origin, server) = server(200, "{}".into(), "");
    let directory = tempfile::tempdir().unwrap();
    let mut engine = connected(directory.path(), &origin);
    engine
        .capture_request(
            &identity(),
            (&origin, None),
            Route::Status,
            &serde_json::json!({
                "plan_id": "plan-one", "manifest_sha256": "a".repeat(64),
            }),
            &|| true,
            &|| true,
        )
        .unwrap();
    let request = server.join().unwrap();
    assert_eq!(
        request.line,
        format!(
            "GET /v1/captures/capture-synthetic/status?plan_id=plan-one&manifest_sha256={} HTTP/1.1",
            "a".repeat(64)
        )
    );
    assert!(request.body.is_empty());
}

#[test]
fn cloud_execution_http_capabilities_bound_capture_and_reject_missing_or_changed_contracts() {
    use super::super::upload;
    use crate::store::cloud_capture::SourceKind;
    let valid = serde_json::json!({
        "enabled": true, "max_expanded_bytes": 256 * 1024 * 1024,
        "chunk_expanded_bytes": 8 * 1024 * 1024,
        "source_formats": ["ClaudeJsonl", "OpenCodeSqliteV2"],
    });
    let (origin, worker) = server(200, valid.to_string(), "");
    let directory = tempfile::tempdir().unwrap();
    let mut engine = connected(directory.path(), &origin);
    let caps = upload::capabilities(
        &mut engine,
        "synthetic-user",
        ("synthetic-personal", None),
        &|| true,
        &|| true,
    )
    .unwrap();
    assert_eq!(caps.limits.source_bytes, 128 * 1024 * 1024);
    assert_eq!(caps.limits.capture_bytes, 256 * 1024 * 1024);
    assert!(!caps.supports(SourceKind::CodexJsonl));
    assert!(!caps.supports(SourceKind::PiJsonl));
    let request = worker.join().unwrap();
    assert_eq!(request.line, "GET /v1/captures/capabilities HTTP/1.1");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer synthetic-api-access")
    );
    assert!(request.body.is_empty());
    let mut extended = valid.clone();
    extended["source_formats"] = serde_json::json!(["CodexJsonl", "future-format", "PiJsonl"]);
    let caps = serde_json::from_value::<upload::Capabilities>(extended)
        .unwrap()
        .validate()
        .unwrap();
    assert_eq!(caps.limits.capture_bytes, 256 * 1024 * 1024);
    assert!(caps.supports(SourceKind::CodexJsonl));
    assert!(caps.supports(SourceKind::PiJsonl));
    assert!(!caps.supports(SourceKind::ClaudeJsonl));

    for body in [
        {
            let mut value = valid.clone();
            value["enabled"] = false.into();
            value
        },
        {
            let mut value = valid.clone();
            value["chunk_expanded_bytes"] = 1.into();
            value
        },
        {
            let mut value = valid.clone();
            value["source_formats"] = serde_json::json!(["unknown"]);
            value
        },
        {
            let mut value = valid.clone();
            value["source_formats"] =
                serde_json::json!(["ClaudeJsonl", "OpenCodeSqliteV2", "future", "future"]);
            value
        },
        serde_json::json!({}),
    ] {
        let value = serde_json::from_value::<upload::Capabilities>(body);
        assert!(value.map(|value| value.validate().is_err()).unwrap_or(true));
    }
    let (origin, worker) = server(404, "{}".into(), "");
    let directory = tempfile::tempdir().unwrap();
    let mut engine = connected(directory.path(), &origin);
    assert!(
        upload::capabilities(
            &mut engine,
            "synthetic-user",
            ("synthetic-personal", None),
            &|| true,
            &|| { true }
        )
        .is_err()
    );
    worker.join().unwrap();
}

#[test]
fn cloud_execution_http_rechecks_account_and_generation_before_sending_bearer() {
    use super::super::upload::Route;
    let directory = tempfile::tempdir().unwrap();
    let mut engine = connected(directory.path(), "http://127.0.0.1:1");
    assert!(matches!(
        engine.capture_request(
            &identity(),
            ("http://127.0.0.1:1", None),
            Route::Reserve,
            &(),
            &|| true,
            &|| false
        ),
        Err(antiburn_cloud_capture::TransportError::Unauthorized)
    ));
    let mut changed = identity();
    changed.account_id = "another-account".into();
    assert!(matches!(
        engine.capture_request(
            &changed,
            ("http://127.0.0.1:1", None),
            Route::Reserve,
            &(),
            &|| true,
            &|| true
        ),
        Err(antiburn_cloud_capture::TransportError::Unauthorized)
    ));
}

#[test]
fn cloud_execution_http_changed_origin_rejects_saved_work_before_auth_or_upload() {
    use super::super::upload::{HttpTransport, Route, Source};
    let directory = tempfile::tempdir().unwrap();
    let original = "http://127.0.0.1:1";
    let mut engine = connected(directory.path(), original);
    engine.config = Config::parse("http://127.0.0.1:2", "http://127.0.0.1:2");
    assert!(engine.session().is_none());
    assert!(matches!(
        engine.capture_request(
            &identity(),
            (original, None),
            Route::Reserve,
            &(),
            &|| true,
            &|| true
        ),
        Err(antiburn_cloud_capture::TransportError::Unauthorized)
    ));
    assert!(engine.state.lock().unwrap().read().unwrap().phase == Phase::Active);
    let mut transport = HttpTransport::new(
        &mut engine,
        &|| true,
        &|| true,
        original,
        Some(Source {
            agent: "claude-code",
            source_format: "ClaudeJsonl",
            native_session_id: "session".into(),
        }),
    );
    assert_eq!(
        transport.reserve(&identity(), 100),
        Err(antiburn_cloud_capture::TransportError::Unauthorized)
    );
    assert!(engine.state.lock().unwrap().read().unwrap().phase == Phase::Active);
}

#[test]
fn cloud_execution_http_only_exact_reserve_error_establishes_source_suppression() {
    use super::super::upload::{Failure, HttpTransport, Source};
    let valid = serde_json::json!({
        "code": "source_suppressed", "retryable": false,
        "message": "This session was deleted from Cloud. It will not upload again automatically.",
        "requestId": "synthetic-request",
    });
    for (body, expected) in [
        (valid.clone(), Failure::SourceSuppressed),
        (
            {
                let mut value = valid.clone();
                value["retryable"] = true.into();
                value
            },
            Failure::Conflict,
        ),
        (
            {
                let mut value = valid.clone();
                value["code"] = "capture_conflict".into();
                value
            },
            Failure::Conflict,
        ),
        (
            {
                let mut value = valid.clone();
                value["message"] = "A different safe explanation.".into();
                value
            },
            Failure::SourceSuppressed,
        ),
        (
            serde_json::json!({"code":"source_suppressed","retryable":false}),
            Failure::Conflict,
        ),
    ] {
        let (origin, worker) = server(409, body.to_string(), "");
        let directory = tempfile::tempdir().unwrap();
        let mut engine = connected(directory.path(), &origin);
        let mut transport = HttpTransport::new(
            &mut engine,
            &|| true,
            &|| true,
            &origin,
            Some(Source {
                agent: "claude-code",
                source_format: "ClaudeJsonl",
                native_session_id: "session".into(),
            }),
        );
        assert!(transport.reserve(&identity(), 100).is_err());
        assert_eq!(transport.failure, Some(expected));
        worker.join().unwrap();
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let body = valid.to_string();
        let mut requests = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            requests.push(read_request(&mut stream));
            write!(stream, "HTTP/1.1 409 Conflict\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        requests
    });
    let directory = tempfile::tempdir().unwrap();
    let mut engine = connected(directory.path(), &origin);
    let mut transport = HttpTransport::new(&mut engine, &|| true, &|| true, &origin, None);
    let manifest = CaptureManifest {
        version: 1,
        identity: identity(),
        compression: "gzip-mtime0-os255-level6".into(),
        chunk_expanded_bytes: 8 * 1024 * 1024,
        metadata_fingerprint: "a".repeat(64),
        source_fingerprint: "b".repeat(64),
        streams: Vec::new(),
    };
    let plan = UploadPlan {
        identity: identity(),
        manifest_sha256: "a".repeat(64),
        plan_id: "synthetic-plan".into(),
        chunks: Vec::new(),
        required_transfer_bytes: 0,
        reused_bytes: 0,
    };
    assert!(transport.status("reservation", &manifest, &plan).is_err());
    assert_eq!(transport.failure, Some(Failure::Conflict));
    let requests = worker.join().unwrap();
    assert!(
        requests[0]
            .line
            .starts_with("GET /v1/captures/capture-synthetic/status?")
    );
    assert_eq!(
        requests[1].line,
        "POST /v1/captures/capture-synthetic/renew HTTP/1.1"
    );
}

#[test]
fn cloud_execution_http_missing_endpoint_and_redirect_do_not_claim_reservation() {
    use super::super::upload::{Failure, HttpTransport, Source};
    for (status, expected) in [(404, Failure::Unavailable), (302, Failure::Rejected)] {
        let (origin, server) = server(status, "{}".into(), "Location: http://127.0.0.1:1/leak\r\n");
        let directory = tempfile::tempdir().unwrap();
        let mut engine = connected(directory.path(), &origin);
        let mut transport = HttpTransport::new(
            &mut engine,
            &|| true,
            &|| true,
            &origin,
            Some(Source {
                agent: "opencode",
                source_format: "OpenCodeSqliteV2",
                native_session_id: "session".into(),
            }),
        );
        assert!(transport.reserve(&identity(), 100).is_err());
        assert_eq!(transport.failure, Some(expected));
        assert_eq!(
            server.join().unwrap().line,
            "POST /v1/captures/reserve HTTP/1.1"
        );
    }
}

#[test]
fn cloud_execution_http_saved_part_has_signed_checksum_length_and_no_api_bearer() {
    use super::super::upload::HttpTransport;
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let bytes = b"synthetic saved gzip body";
    let digest = Sha256::digest(bytes);
    let checksum = base64::engine::general_purpose::STANDARD.encode(digest);
    let hash = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let (object_origin, object_server) = server(200, String::new(), "");
    let (origin, api_server) = server(200, serde_json::json!({
        "url": format!("{object_origin}/object?partNumber=1&uploadId=synthetic"),
        "part_number": 1,
        "headers": {"content-length": bytes.len().to_string(), "x-amz-checksum-sha256": checksum},
    }).to_string(), "");
    let directory = tempfile::tempdir().unwrap();
    let mut engine = connected(directory.path(), &origin);
    let mut transport = HttpTransport::new(&mut engine, &|| true, &|| true, &origin, None);
    let manifest = CaptureManifest {
        version: 1,
        identity: identity(),
        compression: "gzip-mtime0-os255-level6".into(),
        chunk_expanded_bytes: 8 * 1024 * 1024,
        metadata_fingerprint: "a".repeat(64),
        source_fingerprint: "b".repeat(64),
        streams: Vec::new(),
    };
    let plan = UploadPlan {
        identity: identity(),
        manifest_sha256: manifest.sha256().unwrap(),
        plan_id: "plan".into(),
        chunks: Vec::new(),
        required_transfer_bytes: bytes.len() as u64,
        reused_bytes: 0,
    };
    let chunk = Chunk {
        index: 0,
        expanded_bytes: 1,
        expanded_sha256: "a".repeat(64),
        compressed_bytes: bytes.len() as u64,
        compressed_sha256: hash,
    };
    let key = ChunkKey {
        stream_id: "transcript".into(),
        index: 0,
    };
    transport
        .upload_part(
            "reservation",
            &manifest,
            &plan,
            &key,
            &chunk,
            &mut bytes.as_slice(),
        )
        .unwrap();
    let api = api_server.join().unwrap();
    assert_eq!(
        api.line,
        "POST /v1/captures/capture-synthetic/parts HTTP/1.1"
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&api.body).unwrap(),
        serde_json::json!({
            "reservation_id": "reservation", "manifest_sha256": plan.manifest_sha256, "plan_id": "plan", "key": key,
        })
    );
    let object = object_server.join().unwrap();
    assert_eq!(
        object.line,
        "PUT /object?partNumber=1&uploadId=synthetic HTTP/1.1"
    );
    assert_eq!(object.body, bytes);
    assert!(!object.headers.contains_key("authorization"));
    assert!(!object.headers.contains_key("cookie"));
    assert!(!object.headers.contains_key("origin"));
    assert_eq!(object.headers.get("x-amz-checksum-sha256"), Some(&checksum));
}

/// Serve public capture types over loopback without using any provider account.
fn capture_service() -> (String, JoinHandle<usize>) {
    use antiburn_cloud_capture::{ChunkDisposition, PartReceipt, PlannedChunk, ServerStatus};
    use base64::Engine as _;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let service_origin = origin.clone();
    let worker = std::thread::spawn(move || {
        let mut manifest: Option<CaptureManifest> = None;
        let mut plan: Option<UploadPlan> = None;
        let mut receipts = Vec::<PartReceipt>::new();
        let mut pending_part: Option<ChunkKey> = None;
        let mut finalized = false;
        for count in 0..64 {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("synthetic service accept failed: {:?}", error.kind()),
                }
            };
            let request = read_request(&mut stream);
            let object = request.line.starts_with("PUT /object?");
            let (_, path) = request.line.split_once(' ').unwrap();
            assert!(object || path.starts_with("/v1/captures/"));
            assert_eq!(request.headers.contains_key("authorization"), !object);
            assert!(!request.headers.contains_key("cookie"));
            assert!(!request.headers.contains_key("origin"));
            let input: serde_json::Value = if object || request.body.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::from_slice(&request.body).unwrap()
            };
            let (status, body) = if request.line == "POST /v1/captures/reserve HTTP/1.1" {
                assert_eq!(input["agent"], "claude-code");
                (200, serde_json::json!({"reservation_id":"reservation"}))
            } else if request.line.contains("/manifest ") {
                let incoming: CaptureManifest =
                    serde_json::from_value(input["manifest"].clone()).unwrap();
                if let Some(saved) = &manifest {
                    assert_eq!(saved, &incoming);
                }
                let value = UploadPlan {
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
                plan = Some(value.clone());
                (200, serde_json::to_value(value).unwrap())
            } else if request.line.starts_with("GET ") {
                let saved = manifest.as_ref().unwrap();
                let value = ServerStatus {
                    identity: saved.identity.clone(),
                    manifest_sha256: saved.sha256().unwrap(),
                    plan_id: "plan".into(),
                    uploaded_parts: receipts.clone(),
                    integrity_accepted: finalized,
                };
                (200, serde_json::to_value(value).unwrap())
            } else if request.line.contains("/parts ") {
                let key: ChunkKey = serde_json::from_value(input["key"].clone()).unwrap();
                let chunk = manifest.as_ref().unwrap().chunk(&key).unwrap();
                let digest = (0..64)
                    .step_by(2)
                    .map(|index| {
                        u8::from_str_radix(&chunk.compressed_sha256[index..index + 2], 16).unwrap()
                    })
                    .collect::<Vec<_>>();
                pending_part = Some(key);
                (
                    200,
                    serde_json::json!({
                        "url": format!("{service_origin}/object?partNumber=1&uploadId=synthetic"),
                        "part_number": 1, "headers": {
                            "content-length": chunk.compressed_bytes.to_string(),
                            "x-amz-checksum-sha256": base64::engine::general_purpose::STANDARD.encode(digest),
                        },
                    }),
                )
            } else if object {
                use sha2::{Digest, Sha256};
                let key = pending_part.take().unwrap();
                let chunk = manifest.as_ref().unwrap().chunk(&key).unwrap();
                assert_eq!(request.body.len() as u64, chunk.compressed_bytes);
                let digest = Sha256::digest(&request.body)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                assert_eq!(digest, chunk.compressed_sha256);
                receipts.push(PartReceipt {
                    key,
                    compressed_sha256: digest,
                });
                (200, serde_json::Value::Null)
            } else if request.line.contains("/finalize ") {
                assert_eq!(receipts.len(), plan.as_ref().unwrap().chunks.len());
                finalized = true;
                (202, serde_json::Value::Null)
            } else {
                panic!("unexpected synthetic service route");
            };
            let accepted = body["integrity_accepted"] == true;
            let body = body.to_string();
            write!(stream, "HTTP/1.1 {status} Synthetic\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            if accepted {
                return count + 1;
            }
        }
        panic!("synthetic service request bound exceeded");
    });
    (origin, worker)
}

#[test]
fn cloud_execution_http_spool_resume_and_accepted_transaction_survive_restart() {
    use super::super::upload::{HttpTransport, Source};
    use crate::store::{
        Store,
        cloud_capture::{SourceKind, SourceSelection, sync::ProjectSelection},
    };
    use antiburn_cloud_capture::{CaptureState, Limits, MetadataSections, Spool, discover_claude};
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let root = base.join("desktop-sources");
    let project = base.join("project");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&project).unwrap();
    std::fs::write(
        root.join("session.jsonl"),
        format!(
            "{}\n",
            serde_json::json!({
                "sessionId": "session", "cwd": project, "type": "user", "version": "2.1.55",
                "message": {"role":"user","content":"synthetic desktop content"},
            })
        ),
    )
    .unwrap();
    let store = Store::open(&base).unwrap();
    store.initialize_cloud_setup().unwrap();
    let (origin, service) = capture_service();
    let policy = store
        .save_cloud_sync_policy(
            "synthetic-user",
            "synthetic-personal",
            &origin,
            &[SourceSelection {
                source_kind: SourceKind::ClaudeJsonl,
                path: root.to_str().unwrap().into(),
            }],
            &[ProjectSelection {
                source_kind: SourceKind::ClaudeJsonl,
                source_path: root.to_str().unwrap().into(),
                project_path: project.to_str().unwrap().into(),
            }],
            &std::collections::BTreeMap::new(),
        )
        .unwrap();
    let candidate = discover_claude(&root, &Limits::default())
        .unwrap()
        .remove(0);
    let candidate = antiburn_cloud_capture::ProjectSelection::new(vec![project.clone()])
        .unwrap()
        .admit(&candidate)
        .unwrap()
        .unwrap()
        .in_source_scope(&policy.roots[0].source_scope_id)
        .unwrap();
    let job = store
        .enqueue_cloud_sync(
            &policy,
            &policy.roots[0].source_scope_id,
            project.to_str().unwrap(),
            candidate.source_id(),
            "observation",
        )
        .unwrap()
        .unwrap();
    let mut engine = connected(&base, &origin);
    let mut spool = Spool::open(&base.join("spool"), Limits::default()).unwrap();
    let mut transport = HttpTransport::new(
        &mut engine,
        &|| true,
        &|| true,
        &origin,
        Some(Source::from_candidate(&candidate)),
    );
    let entry = spool
        .capture(
            job.identity.clone(),
            &candidate,
            MetadataSections::new(),
            &mut transport,
        )
        .unwrap();
    assert_eq!(entry.state, CaptureState::Queued);
    drop(spool);
    let mut spool = Spool::open(&base.join("spool"), Limits::default()).unwrap();
    let accepted = (0..8)
        .find_map(|_| {
            let step = spool.resume_step(&job.identity, &mut transport).unwrap();
            (step.entry.state == CaptureState::Accepted).then_some(step.entry)
        })
        .expect("bounded capture must become accepted");
    assert_eq!(spool.entries().unwrap().len(), 1);
    store.accept_cloud_sync(&accepted).unwrap();
    spool.acknowledge_accepted(&job.identity).unwrap();
    assert!(spool.entries().unwrap().is_empty());
    assert!(service.join().unwrap() > 5);
    drop(store);
    let store = Store::open(&base).unwrap();
    store.initialize_cloud_setup().unwrap();
    assert_eq!(
        store
            .cloud_sync_source(
                &job.identity.account_id,
                &job.identity.destination_id,
                &origin,
                &job.identity.source_id
            )
            .unwrap()
            .unwrap()
            .fingerprint,
        accepted.accepted_source_fingerprint()
    );
}
