#[path = "../src/opencode_live.rs"]
mod opencode_live;

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};

use opencode_live::{
    ProbeError, RegistrationError, ServiceRegistration, SseDecoder, SseError, SseEvent,
    decode_registration, default_registration_path, probe_registration, read_registration,
};

fn registration(url: &str) -> ServiceRegistration {
    let json = serde_json::json!({
        "id": "service-id",
        "version": "2.0.32",
        "url": url,
        "pid": 9123,
        "password": "private-token",
    });
    decode_registration(&serde_json::to_vec(&json).unwrap()).unwrap()
}

#[test]
fn config_authority_test_helpers_keep_their_fail_closed_contract() {
    let directory = tempfile::tempdir().unwrap();
    let authority = opencode_live::config::CurrentConfigAuthority::fixture(
        directory.path(),
        serde_json::json!([]),
    );
    assert_eq!(
        authority.location(),
        directory.path().canonicalize().unwrap()
    );
    assert!(authority.entries().unwrap().is_empty());
    assert!(authority.retained_bytes() > 0);
    #[cfg(not(windows))]
    assert!(authority.refresh().is_err());
    assert!(authority.for_location(directory.path()).is_err());
    assert!(authority.revalidation_only().entries().is_ok());
    assert!(opencode_live::config::CurrentConfigAuthority::registered(directory.path()).is_err());
}

fn read_http_request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    loop {
        let count = stream.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&request).to_ascii_lowercase()
}

fn decode_chunk(decoder: &mut SseDecoder, mut bytes: &[u8]) -> Vec<Result<SseEvent, SseError>> {
    let mut events = Vec::new();
    while let Some(event) = decoder.next_event(&mut bytes) {
        let cancelled = event == Err(SseError::Cancelled);
        events.push(event);
        if cancelled {
            break;
        }
    }
    events
}

#[test]
fn accepts_only_local_http_registration_and_redacts_password_debug() {
    let local = registration("http://127.0.0.1:4400");
    let debug = format!("{local:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("private-token"));

    for url in [
        "https://127.0.0.1:4400",
        "http://example.com:4400",
        "http://127.0.0.1",
        "http://user@127.0.0.1:4400",
        "http://127.0.0.1:4400/?target=remote",
    ] {
        let value = serde_json::json!({"url": url, "pid": 10});
        assert_eq!(
            decode_registration(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
            RegistrationError::InvalidEndpoint,
            "{url}"
        );
    }
}

#[test]
fn rejects_malformed_and_invalid_registration_fields() {
    assert_eq!(decode_registration(b"{"), Err(RegistrationError::Malformed));
    assert_eq!(
        decode_registration(br#"{"url":"http://127.0.0.1:3","pid":0}"#),
        Err(RegistrationError::InvalidField)
    );
}

#[test]
fn reads_a_bounded_registration_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("service.json");
    std::fs::write(&path, br#"{"url":"http://127.0.0.1:4","pid":12}"#).unwrap();
    assert_eq!(read_registration(&path).unwrap().pid, 12);
    assert!(
        default_registration_path()
            .unwrap()
            .ends_with("opencode/service.json")
    );
    std::fs::write(&path, vec![b' '; 16 * 1024 + 1]).unwrap();
    assert_eq!(
        read_registration(&path).unwrap_err(),
        RegistrationError::TooLarge
    );
}

#[test]
fn registration_path_ignores_empty_state_overrides() {
    use std::{ffi::OsStr, path::PathBuf};

    use opencode_live::registration_path_with_roots;

    let home = PathBuf::from("synthetic-home");
    let fallback = home.join(".local/state/opencode/service.json");
    for state_home in [None, Some(OsStr::new(""))] {
        assert_eq!(
            registration_path_with_roots(state_home, || Some(home.clone())).unwrap(),
            fallback
        );
        assert_eq!(
            registration_path_with_roots(state_home, || None),
            Err(RegistrationError::Io)
        );
    }
    assert_eq!(
        registration_path_with_roots(Some(OsStr::new("synthetic-state")), || {
            panic!("a nonempty state override must not need the home directory")
        })
        .unwrap(),
        PathBuf::from("synthetic-state/opencode/service.json")
    );
}

#[tokio::test]
async fn local_probe_checks_pid_and_version() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 2048];
        let count = stream.read(&mut request).unwrap();
        let body = b"{\"pid\":9123,\"version\":\"2.0.32\"}";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
        String::from_utf8_lossy(&request[..count]).to_ascii_lowercase()
    });
    let service = registration(&format!("http://{address}"));
    let info = probe_registration(&service, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(info.pid, 9123);
    assert_eq!(info.version, "2.0.32");
    let request = server.join().unwrap();
    assert!(request.contains("authorization: basic "));
    assert!(!request.contains("private-token"));
}

#[tokio::test]
async fn local_probe_rejects_actual_pid_and_version_mismatches() {
    for (pid, version, expected) in [
        (9124, "2.0.32", ProbeError::ProcessMismatch),
        (9123, "2.0.33", ProbeError::VersionMismatch),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let request = read_http_request(&mut socket);
            assert!(request.starts_with("get /api/info "));
            let body = serde_json::json!({"pid":pid,"version":version}).to_string();
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let service = registration(&format!("http://{address}"));
        assert_eq!(
            probe_registration(&service, Duration::from_secs(1))
                .await
                .unwrap_err(),
            expected
        );
        server.join().unwrap();
    }
}

#[tokio::test]
async fn missing_registration_and_unreachable_service_fail_without_starting_a_process() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        read_registration(&directory.path().join("missing.json")),
        Err(RegistrationError::Io)
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    assert_eq!(
        probe_registration(
            &registration(&format!("http://{address}")),
            Duration::from_millis(100)
        )
        .await,
        Err(ProbeError::Request)
    );
}

#[tokio::test]
async fn probe_never_follows_redirects() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 2048];
        let _ = stream.read(&mut request).unwrap();
        stream
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: http://example.com/api/info\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
    });
    let service = registration(&format!("http://{address}"));
    assert_eq!(
        probe_registration(&service, Duration::from_secs(2))
            .await
            .unwrap_err(),
        ProbeError::Redirect
    );
    server.join().unwrap();
}

#[tokio::test]
async fn async_probe_bounds_declared_chunked_and_close_delimited_bodies() {
    for framing in ["declared", "chunked", "close"] {
        for oversized in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let service = registration(&format!("http://{}", listener.local_addr().unwrap()));
            let server = thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                assert!(read_http_request(&mut socket).starts_with("get /api/info "));
                let mut body = String::from(r#"{"pid":9123,"version":"2.0.32"}"#);
                body.push_str(&" ".repeat(16 * 1024 - body.len() + usize::from(oversized)));
                let response = match framing {
                    "declared" => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ),
                    "chunked" => format!(
                        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
                        body.len()
                    ),
                    _ => format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{body}"),
                };
                let _ = socket.write_all(response.as_bytes());
            });
            let result = probe_registration(&service, Duration::from_secs(2)).await;
            if oversized {
                assert_eq!(result, Err(ProbeError::ResponseTooLarge), "{framing}");
            } else {
                assert_eq!(result.unwrap().pid, service.pid, "{framing}");
            }
            server.join().unwrap();
        }
    }
}

#[test]
fn decodes_fragmented_json_crlf_comments_and_heartbeats() {
    let mut decoder = SseDecoder::new(1024);
    assert!(decode_chunk(&mut decoder, b": heartbeat\r").is_empty());
    assert!(decode_chunk(&mut decoder, b"\n\r\n: another\n\n").is_empty());
    assert!(decode_chunk(&mut decoder, b"event: test\r\nid: 7\r\nda").is_empty());
    let events = decode_chunk(&mut decoder, b"ta: {\"ok\":\r\ndata: true}\r\n\r");
    assert_eq!(events.len(), 1);
    let event = events[0].as_ref().unwrap();
    assert_eq!(event.event.as_deref(), Some("test"));
    assert_eq!(event.id.as_deref(), Some("7"));
    assert_eq!(event.data, serde_json::json!({"ok": true}));

    let mut utf8 = SseDecoder::new(128);
    assert!(decode_chunk(&mut utf8, b"data: {\"name\":\"caf\xc3").is_empty());
    let event = decode_chunk(&mut utf8, b"\xa9\"}\n\n");
    assert_eq!(
        event[0].as_ref().unwrap().data,
        serde_json::json!({"name": "café"})
    );
}

#[test]
fn reports_malformed_json_without_retaining_payload_in_error() {
    let mut decoder = SseDecoder::new(128);
    let events = decode_chunk(&mut decoder, b"data: private malformed payload\n\n");
    assert_eq!(events, vec![Err(SseError::MalformedJson)]);
}

#[test]
fn rejects_oversize_frames_and_resumes_after_the_frame_boundary() {
    let mut decoder = SseDecoder::new(12);
    let events = decode_chunk(&mut decoder, b"data: 123456789012345\n\ndata: 1\n\n");
    assert_eq!(events[0], Err(SseError::FrameTooLarge));
    assert_eq!(events[1].as_ref().unwrap().data, serde_json::json!(1));
}

#[test]
fn cancellation_clears_partial_data_and_stops_future_decoding() {
    let mut decoder = SseDecoder::new(128);
    assert!(decode_chunk(&mut decoder, b"data: {\"unfinished\"").is_empty());
    decoder.cancel();
    assert_eq!(
        decode_chunk(&mut decoder, b": 1}\n\n"),
        vec![Err(SseError::Cancelled)]
    );
}

#[test]
fn incrementally_drains_more_than_sixty_four_events_without_losing_bytes() {
    let mut decoder = SseDecoder::new(32);
    let input = (0..140)
        .map(|index| format!("data: {index}\r\n\r\n"))
        .collect::<String>();
    let mut remaining = input.as_bytes();
    for expected in 0..140 {
        let before = remaining.len();
        let event = decoder.next_event(&mut remaining).unwrap().unwrap();
        assert_eq!(event.data, serde_json::json!(expected));
        assert!(remaining.len() < before);
    }
    assert!(decoder.next_event(&mut remaining).is_none());
    assert!(remaining.is_empty());
}

#[test]
fn incremental_filter_reaches_admitted_work_after_one_chunk_of_unrelated_events() {
    let mut decoder = SseDecoder::new(1024);
    let mut chunk = String::new();
    for index in 0..140 {
        let event = if index % 2 == 0 {
            serde_json::json!({
                "id":index.to_string(), "type":"session.updated",
                "durable":{"aggregateID":"ses_other", "seq":index + 1, "version":1},
                "data":{"sessionID":"ses_other"}
            })
        } else {
            serde_json::json!({
                "id":index.to_string(), "type":"session.text.delta", "data":{"sessionID":"ses_test"}
            })
        };
        chunk.push_str(&format!("data: {event}\n\n"));
    }
    chunk.push_str("data: {\"id\":\"admitted\",\"type\":\"session.updated\",\"durable\":{\"aggregateID\":\"ses_test\",\"seq\":1,\"version\":1},\"data\":{\"sessionID\":\"ses_test\"}}\n\n");

    let mut remaining = chunk.as_bytes();
    let mut admitted = Vec::new();
    let mut count = 0;
    while let Some(event) = decoder.next_event(&mut remaining) {
        let event = event.unwrap();
        count += 1;
        if event
            .data
            .pointer("/durable/aggregateID")
            .and_then(serde_json::Value::as_str)
            == Some("ses_test")
        {
            admitted.push(event.data["id"].clone());
        }
    }
    assert_eq!(count, 141);
    assert_eq!(admitted, [serde_json::json!("admitted")]);
    assert!(remaining.is_empty());
}

#[test]
fn incremental_decode_returns_valid_events_before_a_later_malformed_frame() {
    let mut decoder = SseDecoder::new(128);
    let mut remaining = b"data: 1\n\ndata: invalid\n\ndata: 2\n\n".as_slice();
    assert_eq!(
        decoder.next_event(&mut remaining).unwrap().unwrap().data,
        serde_json::json!(1)
    );
    assert_eq!(
        decoder.next_event(&mut remaining),
        Some(Err(SseError::MalformedJson))
    );
    assert_eq!(
        decoder.next_event(&mut remaining).unwrap().unwrap().data,
        serde_json::json!(2)
    );
}

#[cfg(unix)]
#[test]
fn unix_binding_requires_the_registered_pid_to_hold_the_database_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let _connection = rusqlite::Connection::open(&path).unwrap();

    assert!(opencode_live::unix::process_has_open_database(std::process::id(), &path).unwrap());

    let other = directory.path().join("other.db");
    drop(rusqlite::Connection::open(&other).unwrap());
    assert!(!opencode_live::unix::process_has_open_database(std::process::id(), &other).unwrap());
}

#[tokio::test]
async fn database_association_requires_stable_registration_and_file_use() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("opencode.db");
    let _connection = rusqlite::Connection::open(&database).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let pid = std::process::id();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 1024];
        let _ = stream.read(&mut request).unwrap();
        let body = format!("{{\"pid\":{pid},\"version\":\"2.0.32\"}}");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let registration_path = directory.path().join("service.json");
    std::fs::write(
        &registration_path,
        serde_json::json!({
            "id":"service-id",
            "version":"2.0.32",
            "url":format!("http://{address}"),
            "pid":pid,
        })
        .to_string(),
    )
    .unwrap();

    let info = opencode_live::verify_database_association(
        &registration_path,
        &database,
        Duration::from_secs(2),
    )
    .await
    .unwrap();

    server.join().unwrap();
    assert_eq!(info.pid, pid);
}

#[tokio::test]
async fn database_association_rejects_a_file_not_open_by_the_registered_process() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("unbound.db");
    drop(rusqlite::Connection::open(&database).unwrap());
    let registration = directory.path().join("service.json");
    std::fs::write(&registration, serde_json::json!({
        "id":"synthetic-service", "url":"http://127.0.0.1:1", "pid":std::process::id(), "version":"2.0.32"
    }).to_string()).unwrap();
    assert!(matches!(
        opencode_live::verify_database_association(
            &registration,
            &database,
            Duration::from_millis(100)
        )
        .await,
        Err(opencode_live::DatabaseAssociationError::Binding)
    ));
}

async fn start_config_service(
    registration_path: &std::path::Path,
    version: &str,
    config_body: &str,
    request_count: usize,
) -> tokio::task::JoinHandle<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let pid = std::process::id();
    std::fs::write(
        registration_path,
        serde_json::json!({
            "id":"synthetic-config-service", "version":version, "pid":pid,
            "url":endpoint, "password":"synthetic-private-password"
        })
        .to_string(),
    )
    .unwrap();
    let version = version.to_owned();
    let config_body = config_body.to_owned();
    tokio::spawn(async move {
        for _ in 0..request_count {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
                assert!(request.len() < 16 * 1024);
            }
            let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
            assert!(request.contains("authorization: basic "));
            assert!(!request.contains("synthetic-private-password"));
            let body = if request.starts_with("get /api/info ") {
                serde_json::json!({"pid":pid,"version":version}).to_string()
            } else {
                assert!(request.starts_with("get /api/config?location%5bdirectory%5d="));
                config_body.clone()
            };
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    })
}

async fn capture_config_fixture(
    version: &str,
    config_body: &str,
    request_count: usize,
    blocking: bool,
) -> Result<opencode_live::config::CurrentConfigAuthority, opencode_live::config::AuthorityError> {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("opencode.db");
    let _connection = rusqlite::Connection::open(&database).unwrap();
    let registration_path = directory.path().join("service.json");
    let server =
        start_config_service(&registration_path, version, config_body, request_count).await;
    let request = opencode_live::config::ConfigAuthorityRequest {
        registration_path,
        database_path: database,
        location: directory.path().into(),
    };
    let result = if blocking {
        tokio::task::spawn_blocking(move || {
            opencode_live::config::CurrentConfigAuthority::capture(&request)
        })
        .await
        .unwrap()
    } else {
        opencode_live::config::CurrentConfigAuthority::capture_async(&request).await
    };
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
    result
}

#[tokio::test]
async fn async_config_authority_accepts_v2_versions_with_the_supported_response_shape() {
    for version in ["2.0.32", "2.1.1", "2.99.0", "v2.1.1"] {
        let authority = capture_config_fixture(
            version,
            r#"[{"type":"document","info":{"default_agent":"build"}}]"#,
            3,
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            authority.entries().unwrap()[0].info.as_ref().unwrap()["default_agent"],
            "build",
            "{version}"
        );
        assert!(format!("{authority:?}").contains(version));
    }
}

#[tokio::test]
async fn async_config_authority_rejects_non_v2_versions_and_incompatible_responses() {
    use opencode_live::config::AuthorityError;

    for version in ["1.9.0", "3.0.0", "20.1.1", "unknown"] {
        assert_eq!(
            capture_config_fixture(version, "[]", 1, false).await.err(),
            Some(AuthorityError::UnsupportedConsumer),
            "{version}"
        );
    }
    for body in [r#"{"model":"legacy"}"#, r#"[{"type":"unknown"}]"#] {
        assert_eq!(
            capture_config_fixture("2.1.1", body, 2, false).await.err(),
            Some(AuthorityError::InvalidResponse)
        );
    }
}

#[tokio::test]
async fn sync_config_authority_uses_the_shared_runtime_from_a_blocking_worker() {
    assert!(capture_config_fixture("2.1.1", "[]", 3, true).await.is_ok());
}

#[tokio::test]
async fn registered_config_candidates_require_one_verified_database() {
    use opencode_live::config::{AuthorityError, CurrentConfigAuthority};

    for bound_count in 0..=2 {
        let directory = tempfile::tempdir().unwrap();
        let unavailable = directory.path().join("unavailable.db");
        let unbound = directory.path().join("unbound.db");
        drop(rusqlite::Connection::open(&unbound).unwrap());
        let mut candidates = vec![unavailable, unbound];
        let mut connections = Vec::new();
        for index in 0..bound_count {
            let path = directory.path().join(format!("bound-{index}.db"));
            connections.push(rusqlite::Connection::open(&path).unwrap());
            candidates.push(path);
        }
        let registration_path = directory.path().join("service.json");
        let request_count = if bound_count == 1 { 3 } else { bound_count };
        let server = start_config_service(
            &registration_path,
            "2.1.1",
            r#"[{"type":"document","info":{"default_agent":"build"}}]"#,
            request_count,
        )
        .await;
        let result = CurrentConfigAuthority::registered_from_candidates(
            &registration_path,
            &candidates,
            directory.path(),
            Instant::now(),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
        match bound_count {
            0 => assert_eq!(result.err(), Some(AuthorityError::Unavailable)),
            1 => {
                let authority = result.unwrap();
                assert_eq!(
                    authority.location(),
                    directory.path().canonicalize().unwrap()
                );
                assert_eq!(
                    authority.entries().unwrap()[0].info.as_ref().unwrap()["default_agent"],
                    "build"
                );
            }
            _ => assert_eq!(result.err(), Some(AuthorityError::ChangedConsumer)),
        }
        drop(connections);
    }
}

#[tokio::test]
async fn a_verified_consumer_survives_an_invalid_first_location() {
    use opencode_live::config::VerifiedConfigConsumer;

    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("opencode.db");
    let _connection = rusqlite::Connection::open(&database).unwrap();
    let registration_path = directory.path().join("service.json");
    let server = start_config_service(&registration_path, "2.1.1", "[]", 4).await;
    let mut consumer =
        VerifiedConfigConsumer::from_candidates(&registration_path, &[database], Instant::now())
            .await
            .unwrap();
    assert!(
        consumer
            .capture_async(&directory.path().join("deleted"))
            .await
            .is_err()
    );
    assert!(
        consumer
            .capture_async(directory.path())
            .await
            .unwrap()
            .entries()
            .is_ok()
    );
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn unix_binding_ignores_unrelated_file_descriptor_churn() {
    use std::sync::{Arc, atomic::Ordering};

    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("opencode.db");
    let _connection = rusqlite::Connection::open(&database).unwrap();
    let unrelated = directory.path().join("unrelated");
    std::fs::write(&unrelated, "unrelated").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let worker = thread::spawn(move || {
        while !worker_stop.load(Ordering::Relaxed) {
            drop(std::fs::File::open(&unrelated).unwrap());
            thread::yield_now();
        }
    });
    let results = (0..32)
        .map(|_| opencode_live::unix::process_has_open_database(std::process::id(), &database))
        .collect::<Vec<_>>();
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
    assert!(results.into_iter().all(|result| result.unwrap()));
}

#[tokio::test]
async fn async_registered_test_lookup_stays_unavailable() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        opencode_live::config::CurrentConfigAuthority::registered_async(directory.path())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn verified_event_stream_drains_unrelated_frames_from_the_bound_service() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("opencode.db");
    let _connection = rusqlite::Connection::open(&database).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let pid = std::process::id();
    let server = thread::spawn(move || {
        let mut event_request = String::new();
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            if request.starts_with("get /api/info ") {
                let body = format!("{{\"pid\":{pid},\"version\":\"2.0.32\"}}");
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            } else {
                event_request = request;
                let mut body = String::from(
                    "data: {\"id\":\"connected\",\"type\":\"server.connected\",\"data\":{}}\n\n: ",
                );
                body.push_str(&"x".repeat(16 * 1024 + 128));
                body.push_str("\n\n: heartbeat\n\n");
                for index in 0..140 {
                    let event = if index % 2 == 0 {
                        serde_json::json!({
                            "id":format!("unrelated-{index}"), "type":"session.updated",
                            "durable":{"aggregateID":"ses_other", "seq":index + 1, "version":1},
                            "data":{"sessionID":"ses_other"}
                        })
                    } else {
                        serde_json::json!({
                            "id":format!("ephemeral-{index}"), "type":"session.text.delta",
                            "data":{"sessionID":"ses_test", "text":"delta"}
                        })
                    };
                    body.push_str(&format!("data: {event}\n\n"));
                }
                for index in 0..3 {
                    let event = serde_json::json!({
                        "id":format!("admitted-{index}"), "type":"session.updated",
                        "durable":{"aggregateID":"ses_test", "seq":index + 1, "version":1},
                        "data":{"sessionID":"ses_test"}
                    });
                    body.push_str(&format!("data: {event}\n\n"));
                }
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        }
        event_request
    });
    let registration_path = directory.path().join("service.json");
    std::fs::write(
        &registration_path,
        serde_json::json!({
            "id":"service-id",
            "version":"2.0.32",
            "url":format!("http://{address}"),
            "pid":pid,
            "password":"private-token",
        })
        .to_string(),
    )
    .unwrap();

    let mut stream = opencode_live::stream::open_verified_event_stream(
        &registration_path,
        &database,
        Duration::from_secs(2),
        opencode_live::stream::StreamTiming::default(),
    )
    .await
    .unwrap();
    let event = stream.next_event(&AtomicBool::new(false)).await.unwrap();
    assert_eq!(event.data["type"], "server.connected");
    let mut admitted = Vec::new();
    for _ in 0..143 {
        let event = stream.next_event(&AtomicBool::new(false)).await.unwrap();
        if event
            .data
            .pointer("/durable/aggregateID")
            .and_then(serde_json::Value::as_str)
            == Some("ses_test")
        {
            admitted.push(event.data["id"].as_str().unwrap().to_owned());
        }
    }
    assert_eq!(admitted, ["admitted-0", "admitted-1", "admitted-2"]);
    assert!(matches!(
        stream.next_event(&AtomicBool::new(false)).await,
        Err(opencode_live::stream::EventStreamError::StreamEnded)
    ));
    assert_eq!(stream.service_info().pid, pid);
    assert_eq!(stream.registration().pid, pid);
    let event_request = server.join().unwrap();
    assert!(event_request.starts_with("get /api/event "));
    assert!(event_request.contains("authorization: basic "));
    assert!(!event_request.contains("private-token"));
}

#[tokio::test]
async fn verified_event_stream_rejects_redirects_without_following_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("opencode.db");
    let _connection = rusqlite::Connection::open(&database).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let pid = std::process::id();
    let server = thread::spawn(move || {
        let mut event_request = String::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            if request.starts_with("get /api/info ") {
                let body = format!("{{\"pid\":{pid},\"version\":\"2.0.32\"}}");
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            } else {
                event_request = request;
                stream
                    .write_all(
                        b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/redirect\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap();
            }
        }
        event_request
    });
    let registration_path = directory.path().join("service.json");
    std::fs::write(
        &registration_path,
        serde_json::json!({
            "url":format!("http://{address}"),
            "pid":pid,
            "password":"private-token",
        })
        .to_string(),
    )
    .unwrap();

    let result = opencode_live::stream::open_verified_event_stream(
        &registration_path,
        &database,
        Duration::from_secs(2),
        opencode_live::stream::StreamTiming::default(),
    )
    .await;

    assert!(matches!(
        result,
        Err(opencode_live::stream::EventStreamError::Redirect)
    ));
    let event_request = server.join().unwrap();
    assert!(event_request.starts_with("get /api/event "));
    assert!(event_request.contains("authorization: basic "));
}

#[tokio::test]
async fn verified_event_stream_rejects_bad_credentials_before_opening_the_feed() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("opencode.db");
    let _connection = rusqlite::Connection::open(&database).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let request = read_http_request(&mut socket);
        socket
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        request
    });
    let registration_path = directory.path().join("service.json");
    std::fs::write(
        &registration_path,
        serde_json::json!({
            "url":format!("http://{address}"),
            "pid":std::process::id(),
            "version":"2.0.32",
            "password":"private-token",
        })
        .to_string(),
    )
    .unwrap();

    let result = opencode_live::stream::open_verified_event_stream(
        &registration_path,
        &database,
        Duration::from_secs(2),
        opencode_live::stream::StreamTiming::default(),
    )
    .await;
    let error = match result {
        Ok(_) => panic!("bad credentials opened the event stream"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        opencode_live::stream::EventStreamError::Association(
            opencode_live::DatabaseAssociationError::Probe(opencode_live::ProbeError::HttpStatus(
                401
            ))
        )
    ));
    assert!(!format!("{error:?}").contains("private-token"));
    let request = server.join().unwrap();
    assert!(request.starts_with("get /api/info "));
    assert!(request.contains("authorization: basic "));
    assert!(!request.contains("private-token"));
}

#[tokio::test]
async fn verified_event_stream_rejects_registration_replacement_during_setup() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("opencode.db");
    let _connection = rusqlite::Connection::open(&database).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let pid = std::process::id();
    let registration_path = directory.path().join("service.json");
    let server_registration_path = registration_path.clone();
    let server = thread::spawn(move || {
        for _ in 0..3 {
            let (mut socket, _) = listener.accept().unwrap();
            let request = read_http_request(&mut socket);
            if request.starts_with("get /api/info ") {
                let body = format!("{{\"pid\":{pid},\"version\":\"2.0.32\"}}");
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            } else {
                std::fs::write(
                    &server_registration_path,
                    serde_json::json!({
                        "id":"replacement",
                        "url":format!("http://{address}"),
                        "pid":pid,
                        "version":"2.0.32",
                        "password":"private-token",
                    })
                    .to_string(),
                )
                .unwrap();
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap();
            }
        }
    });
    std::fs::write(
        &registration_path,
        serde_json::json!({
            "id":"initial",
            "url":format!("http://{address}"),
            "pid":pid,
            "version":"2.0.32",
            "password":"private-token",
        })
        .to_string(),
    )
    .unwrap();

    let result = opencode_live::stream::open_verified_event_stream(
        &registration_path,
        &database,
        Duration::from_secs(2),
        opencode_live::stream::StreamTiming::default(),
    )
    .await;

    assert!(matches!(
        result,
        Err(opencode_live::stream::EventStreamError::RegistrationChanged)
    ));
    server.join().unwrap();
}

#[tokio::test]
async fn cancelling_event_stream_drops_the_open_response() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("opencode.db");
    let _connection = rusqlite::Connection::open(&database).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let pid = std::process::id();
    let server = thread::spawn(move || {
        for _ in 0..3 {
            let (mut socket, _) = listener.accept().unwrap();
            let request = read_http_request(&mut socket);
            if request.starts_with("get /api/info ") {
                let body = format!("{{\"pid\":{pid},\"version\":\"2.0.32\"}}");
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            } else {
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\n",
                    )
                    .unwrap();
            }
        }
    });
    let registration_path = directory.path().join("service.json");
    std::fs::write(
        &registration_path,
        serde_json::json!({
            "url":format!("http://{address}"),
            "pid":pid,
        })
        .to_string(),
    )
    .unwrap();

    let mut stream = opencode_live::stream::open_verified_event_stream(
        &registration_path,
        &database,
        Duration::from_secs(2),
        opencode_live::stream::StreamTiming::default(),
    )
    .await
    .unwrap();
    let error = stream.next_event(&AtomicBool::new(true)).await.unwrap_err();

    assert!(matches!(
        error,
        opencode_live::stream::EventStreamError::Cancelled
    ));
    server.join().unwrap();
}
