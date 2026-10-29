use super::*;
use base64::Engine;

// These fixtures match the pinned producer, not a running WSL installation.
const UPDATED_MS: i64 = 1_790_000_000_000;

fn registration() -> Value {
    json!({"id":"service_test","pid":1234,"version":"2.0.32",
        "url":"http://127.0.0.1:4096/","password":"synthetic-password"})
}

fn service() -> WslServiceInfo {
    parse_service(&serde_json::to_vec(&registration()).unwrap()).unwrap()
}

fn health() -> Value {
    json!({"pid":1234,"version":"2.0.32"})
}

fn session(index: usize) -> Value {
    json!({"id":format!("ses_{index}"), "projectID":"synthetic-project", "location":{"directory":"/work/project"},
        "title":"synthetic session", "time":{"created":UPDATED_MS-1_000_000, "updated":UPDATED_MS-index as i64*1000},
        "model":{"id":"synthetic-model","providerID":"synthetic-provider"},
        "tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}}, "cost":0})
}

fn export(index: usize) -> Value {
    json!({"data":{"info":session(index),"messages":[
        {"id":"msg_user","type":"user","text":"synthetic request","time":{"created":UPDATED_MS}},
        {"id":"msg_assistant","type":"assistant","agent":"build",
         "model":{"id":"synthetic-model","providerID":"synthetic-provider"},
         "time":{"created":UPDATED_MS,"completed":UPDATED_MS+1},
         "content":[{"id":"call_read","type":"tool","name":"read",
           "state":{"status":"completed","input":{"path":"file.txt"},
             "content":[{"type":"text","text":"synthetic output"}]},
           "time":{"created":UPDATED_MS,"completed":UPDATED_MS+1}}]}
    ]}})
}

fn cursor(last: usize) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        json!({"order":"desc","anchor":{"id":format!("ses_{last}"),
            "time":UPDATED_MS-last as i64*1000,"direction":"next"}})
        .to_string(),
    )
}

fn page(start: usize, count: usize) -> Value {
    let next = (count > 0).then(|| cursor(start + count - 1));
    let data = (start..start + count).map(session).collect::<Vec<_>>();
    let mut result = json!({"data":data,"cursor":{}});
    if let Some(next) = next {
        result["cursor"]["next"] = json!(next);
    }
    result
}

async fn test_response(
    response: Result<Vec<u8>, CliBridgeError>,
    limit: usize,
    budget: Option<Arc<ExportByteBudget>>,
) -> Result<Vec<u8>, CliBridgeError> {
    let mut framed = response?;
    framed.extend_from_slice(HTTP_STATUS_TRAILER);
    let mut bytes = read_http_output(framed.as_slice(), limit, budget.as_deref()).await?;
    bytes.truncate(bytes.len() - HTTP_STATUS_TRAILER.len());
    Ok(bytes)
}

async fn scripted_discovery(
    responses: Vec<(String, Value)>,
) -> Result<Vec<SessionLog>, CliBridgeError> {
    let mut responses = VecDeque::from(responses);
    let result = discover_from_service(
        &DiscoveryEnvironment::Wsl {
            distribution: "Ubuntu".into(),
            user: "dev".into(),
        },
        &service(),
        UPDATED_MS / 1000 - 10_000,
        Instant::now() + V2_DISCOVERY_TIMEOUT,
        |path, limit, budget| {
            let (expected, value) = responses.pop_front().expect("unexpected API request");
            assert_eq!(path, expected);
            let bytes = serde_json::to_vec(&value).unwrap();
            assert!(bytes.len() <= limit);
            test_response(Ok(bytes), limit, budget)
        },
    )
    .await;
    if result.is_ok() {
        assert!(responses.is_empty());
    }
    result
}

#[test]
fn registration_uses_url_and_rejects_remote_or_malformed_endpoints() {
    assert_eq!(
        service().endpoint.base_url().as_str(),
        "http://127.0.0.1:4096/"
    );
    for url in [
        "http://[::1]:4096",
        "http://[::1]:4096/",
        "http://127.10.20.30:4096/",
    ] {
        let mut value = registration();
        value["url"] = json!(url);
        assert!(parse_service(value.to_string().as_bytes()).is_ok());
    }
    let userinfo_url = [
        "http://",
        "synthetic-user:synthetic-password@",
        "127.0.0.1:4096",
    ]
    .concat();
    for url in [
        "http://localhost:4096",
        "http://192.0.2.1:4096",
        "https://127.0.0.1:4096",
        "http://127.0.0.1:65536",
        "http://127.0.0.1:4096?x=1",
        userinfo_url.as_str(),
        "http://127.0.0.1:4096#fragment",
        "http://[::]:4096",
    ] {
        let mut value = registration();
        value["url"] = json!(url);
        assert!(
            parse_service(value.to_string().as_bytes()).is_err(),
            "{url}"
        );
    }
    assert!(parse_service(br#"{"hostname":"127.0.0.1","port":4096}"#).is_err());
    for (key, value) in [
        ("pid", json!(0)),
        ("version", json!("1.9.0")),
        ("password", json!("synthetic\0password")),
    ] {
        let mut info = registration();
        info[key] = value;
        assert!(parse_service(info.to_string().as_bytes()).is_err());
    }
}

#[test]
fn registration_uses_the_shared_rules_with_only_a_v2_version_gate() {
    for (field, value) in [
        ("url", json!("http://127.10.20.30:4096/")),
        ("url", json!("http://127.0.0.1:4096/base")),
        ("password", json!("synthetic\npassword")),
        ("id", json!("x".repeat(257))),
        ("version", json!("2.".repeat(65))),
        ("pid", json!(u64::from(u32::MAX) + 1)),
        ("password", Value::Null),
    ] {
        let mut value_with_field = registration();
        value_with_field[field] = value;
        let bytes = value_with_field.to_string().into_bytes();
        assert_eq!(
            parse_service(&bytes).is_ok(),
            decode_registration(&bytes).is_ok(),
            "{field}"
        );
    }
    let mut missing_version = registration();
    missing_version.as_object_mut().unwrap().remove("version");
    let bytes = missing_version.to_string().into_bytes();
    assert!(decode_registration(&bytes).is_ok());
    assert!(parse_service(&bytes).is_err());
}

#[test]
fn registration_paths_use_the_state_home_in_the_owning_environment() {
    use crate::platform::environment::AgentContext;

    let dir = tempfile::tempdir().unwrap();
    let native = AgentContext::native(dir.path().join("synthetic-home"));
    let native_state = dir.path().join("synthetic-state");
    assert_eq!(
        service_path_in(&native, None).unwrap(),
        native.home.join(".local/state/opencode/service.json")
    );
    assert_eq!(
        service_path_in(&native, Some(&native_state)).unwrap(),
        native_state.join("opencode/service.json")
    );
    assert_eq!(
        service_path_in(&native, Some(Path::new("relative-state"))).unwrap(),
        service_path_in(&native, None).unwrap()
    );
    let wsl = AgentContext {
        environment: DiscoveryEnvironment::Wsl {
            distribution: "Synthetic".into(),
            user: "synthetic".into(),
        },
        home: PathBuf::from(r"\\wsl.localhost\Synthetic\home\synthetic"),
        platform_home: PathBuf::from("/home/synthetic"),
    };
    assert_eq!(
        service_path_in(&wsl, Some(Path::new("/synthetic-state"))).unwrap(),
        wsl.environment
            .host_path("/synthetic-state/opencode/service.json")
            .unwrap()
    );
}

#[tokio::test]
async fn v2_releases_use_the_same_validated_transfer_contract() {
    for version in ["2.0.32", "2.0.33", "2.1.1", "2.99.0", "v2.1.1"] {
        let mut registration = registration();
        registration["version"] = json!(version);
        let service = parse_service(registration.to_string().as_bytes()).unwrap();
        let mut responses = VecDeque::from([
            ("/api/info".into(), json!({"pid":1234,"version":version})),
            (session_list_path(None), page(0, 1)),
            ("/api/experimental/session/ses_0/export".into(), export(0)),
            ("/api/info".into(), json!({"pid":1234,"version":version})),
        ]);
        let logs = discover_from_service(
            &DiscoveryEnvironment::Wsl {
                distribution: "Ubuntu".into(),
                user: "dev".into(),
            },
            &service,
            UPDATED_MS / 1000 - 10_000,
            Instant::now() + V2_DISCOVERY_TIMEOUT,
            |path, limit, budget| {
                let (expected, value) = responses.pop_front().unwrap();
                assert_eq!(path, expected);
                let bytes = serde_json::to_vec(&value).unwrap();
                assert!(bytes.len() <= limit);
                test_response(Ok(bytes), limit, budget)
            },
        )
        .await
        .unwrap();
        assert_eq!(logs.len(), 1, "{version}");
        assert!(responses.is_empty());
    }
    for version in ["", "1.9.0", "3.0.0", "20.0.0"] {
        let mut registration = registration();
        registration["version"] = json!(version);
        assert!(parse_service(registration.to_string().as_bytes()).is_err());
    }
}

#[tokio::test]
async fn registration_reads_are_bounded_and_keep_errors_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("service.json");
    let error = read_service(&path).await.err().unwrap().to_string();
    assert_eq!(error, "registered WSL service was unavailable");
    tokio::fs::write(&path, vec![b' '; SERVICE_MAX_BYTES + 1])
        .await
        .unwrap();
    assert!(matches!(
        read_service(&path).await,
        Err(CliBridgeError::Oversized)
    ));
    tokio::fs::write(&path, br#"{"url":"synthetic-private-url","password":"synthetic-private-secret","pid":"synthetic-private-pid"}"#).await.unwrap();
    let error = read_service(&path).await.err().unwrap().to_string();
    assert_eq!(error, "registered WSL service metadata was invalid");
    assert!(!error.contains("synthetic-private"));
}

#[tokio::test]
async fn stock_short_page_with_next_cursor_stops_and_exports_v2() {
    let logs = scripted_discovery(vec![
        ("/api/info".into(), health()),
        (session_list_path(None), page(0, 2)),
        ("/api/experimental/session/ses_0/export".into(), export(0)),
        ("/api/experimental/session/ses_1/export".into(), export(1)),
        ("/api/info".into(), health()),
    ])
    .await
    .unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0].updated_at, Some(UPDATED_MS / 1000));
    assert!(
        matches!(&logs[0].source,SessionSource::Inline{label,..} if label=="opencode-v2-export:ses_0")
    );
    assert!(!logs[0].environment.is_native());
    let read = crate::discovery::session_log_read(&logs[0]).await.unwrap();
    assert_eq!(read.metadata.session_id.as_deref(), Some("ses_0"));
    assert_eq!(read.metadata.title.as_deref(), Some("synthetic session"));
    assert!(read.metadata.cwd.unwrap().contains("work"));
}

#[tokio::test]
async fn v2_export_metadata_uses_the_typed_format_instead_of_the_label() {
    use crate::analysis::SourceFormat::OpenCodeV2SessionExport;

    let mut log = exported_session_log(
        &DiscoveryEnvironment::Native,
        "ses_0",
        0,
        export(0).to_string().as_bytes(),
    )
    .unwrap();
    match &mut log.source {
        SessionSource::Inline {
            label,
            source_format,
            ..
        } => {
            assert_eq!(*source_format, Some(OpenCodeV2SessionExport));
            *label = "display-only-label".into();
        }
        _ => panic!("expected an inline export"),
    }
    let read = crate::discovery::session_log_read(&log).await.unwrap();
    assert_eq!(read.metadata.session_id.as_deref(), Some("ses_0"));
    assert_eq!(read.metadata.cwd.as_deref(), Some("/work/project"));
    assert_eq!(read.metadata.title.as_deref(), Some("synthetic session"));

    log.source = SessionSource::Inline {
        label: "opencode-v2-export:ses_0".into(),
        content: "{}".into(),
        source_format: None,
    };
    assert!(crate::discovery::session_log_read(&log).await.is_some());
}

struct ActiveExport(Arc<AtomicUsize>);

impl Drop for ActiveExport {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn concurrent_exports_keep_order_and_the_full_response_limit() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Barrier::new(CLI_EXPORT_CONCURRENCY));
    let records = (0..12)
        .map(|index| (format!("ses_{index}"), index as i64))
        .collect();
    let logs = collect_v2_exports(
        &DiscoveryEnvironment::Native,
        records,
        1024,
        Instant::now() + Duration::from_secs(2),
        &mut |path: String, limit, budget| {
            let active = active.clone();
            let peak = peak.clone();
            let started = started.clone();
            async move {
                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(count, Ordering::SeqCst);
                let _active = ActiveExport(active);
                assert_eq!(limit, EXPORT_MAX_BYTES);
                let index: usize = path
                    .strip_prefix("/api/experimental/session/ses_")
                    .unwrap()
                    .strip_suffix("/export")
                    .unwrap()
                    .parse()
                    .unwrap();
                if index < CLI_EXPORT_CONCURRENCY {
                    started.wait().await;
                }
                tokio::time::sleep(Duration::from_millis(if index == 0 { 30 } else { 2 })).await;
                test_response(Ok(export(index).to_string().into_bytes()), limit, budget).await
            }
        },
    )
    .await
    .unwrap();
    assert_eq!(peak.load(Ordering::SeqCst), CLI_EXPORT_CONCURRENCY);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(
        logs.iter()
            .map(|log| log.updated_at.unwrap())
            .collect::<Vec<_>>(),
        (0..12).collect::<Vec<_>>()
    );
}

fn sized_export(index: usize, size: usize) -> Vec<u8> {
    let mut value = export(index);
    value["data"]["messages"][0]["text"] = json!("");
    let overhead = serde_json::to_vec(&value).unwrap().len();
    value["data"]["messages"][0]["text"] = json!("x".repeat(size - overhead));
    let bytes = serde_json::to_vec(&value).unwrap();
    assert_eq!(bytes.len(), size);
    bytes
}

#[tokio::test]
async fn asymmetric_exports_fit_the_shared_limit_in_both_completion_orders() {
    const METADATA_BYTES: usize = 1024;
    let sizes = [15 * 1024 * 1024, 1024, EXPORT_MAX_BYTES - 4096];
    assert!(METADATA_BYTES + sizes.iter().sum::<usize>() < V2_SESSION_MAX_BYTES);

    for order in [[0, 1, 2], [1, 2, 0]] {
        let mut responses = sizes
            .into_iter()
            .enumerate()
            .map(|(index, size)| Some(sized_export(index, size)))
            .collect::<Vec<_>>();
        let gates = Arc::new(std::array::from_fn::<_, 3, _>(|_| {
            tokio::sync::Semaphore::new(0)
        }));
        gates[order[0]].add_permits(1);
        let completions = Arc::new(Mutex::new(Vec::new()));
        let mut shared_budget = None;
        let logs = collect_v2_exports(
            &DiscoveryEnvironment::Native,
            (0..3)
                .map(|index| (format!("ses_{index}"), index))
                .collect(),
            METADATA_BYTES,
            Instant::now() + Duration::from_secs(10),
            &mut |path: String, limit, budget: Option<Arc<ExportByteBudget>>| {
                assert_eq!(limit, EXPORT_MAX_BYTES);
                shared_budget.get_or_insert_with(|| budget.as_ref().unwrap().clone());
                let index: usize = path
                    .strip_prefix("/api/experimental/session/ses_")
                    .unwrap()
                    .strip_suffix("/export")
                    .unwrap()
                    .parse()
                    .unwrap();
                let bytes = responses[index].take().unwrap();
                let gates = gates.clone();
                let completions = completions.clone();
                async move {
                    gates[index].acquire().await.unwrap().forget();
                    let body = test_response(Ok(bytes), limit, budget).await?;
                    completions.lock().unwrap().push(index);
                    let position = order.iter().position(|item| *item == index).unwrap();
                    if let Some(next) = order.get(position + 1) {
                        gates[*next].add_permits(1);
                    }
                    Ok(body)
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(*completions.lock().unwrap(), order);
        assert_eq!(logs.len(), 3, "{order:?}");
        assert_eq!(
            logs.iter()
                .map(SessionLog::source_label)
                .collect::<Vec<_>>(),
            [
                "opencode-v2-export:ses_0",
                "opencode-v2-export:ses_1",
                "opencode-v2-export:ses_2",
            ]
        );
        assert_eq!(
            shared_budget.unwrap().remaining(),
            V2_SESSION_MAX_BYTES - METADATA_BYTES - sizes.iter().sum::<usize>()
        );
    }
}

#[tokio::test]
async fn shared_streaming_budget_counts_body_bytes_without_status_trailers() {
    let budget = Arc::new(ExportByteBudget::new(1000));
    for size in [1, HTTP_STATUS_TRAILER.len() - 1, 500] {
        assert_eq!(
            test_response(Ok(vec![b'x'; size]), 600, Some(budget.clone()))
                .await
                .unwrap()
                .len(),
            size
        );
    }
    assert_eq!(budget.remaining(), 1000 - HTTP_STATUS_TRAILER.len() - 500);
    let error = test_response(Ok(vec![b'x'; 500]), 600, Some(budget.clone())).await;
    assert!(matches!(error, Err(CliBridgeError::Oversized)));
    assert_eq!(budget.remaining(), 0);
}

#[tokio::test]
async fn streaming_counts_failure_bodies_and_excludes_valid_status_framing() {
    for status in [b"200", b"302", b"401", b"500", b"000"] {
        let mut framed = vec![b'x'; 512];
        framed.extend_from_slice(HTTP_STATUS_TRAILER);
        let status_start = framed.len() - 4;
        framed[status_start..status_start + 3].copy_from_slice(status);
        let budget = ExportByteBudget::new(1024);
        read_http_output(framed.as_slice(), 1024, Some(&budget))
            .await
            .unwrap();
        assert_eq!(budget.remaining(), 512);
    }
    let budget = ExportByteBudget::new(1024);
    read_http_output(&[b'x'; 512][..], 1024, Some(&budget))
        .await
        .unwrap();
    assert_eq!(budget.remaining(), 512);
}

#[tokio::test]
async fn streaming_keeps_the_individual_limit_across_fragmented_status_trailers() {
    let budget = Arc::new(ExportByteBudget::new(4096));
    let result = test_response(Ok(vec![b'x'; 1025]), 1024, Some(budget.clone())).await;
    assert!(matches!(result, Err(CliBridgeError::Oversized)));
    assert_eq!(budget.remaining(), 4096 - 1024);

    let (mut writer, reader) = tokio::io::duplex(64);
    let write = async move {
        writer.write_all(&[b'x'; 8192]).await.unwrap();
        for byte in HTTP_STATUS_TRAILER {
            writer.write_all(&[*byte]).await.unwrap();
            tokio::task::yield_now().await;
        }
        writer.shutdown().await.unwrap();
    };
    let budget = ExportByteBudget::new(16 * 1024);
    let (result, ()) = tokio::join!(read_http_output(reader, 16 * 1024, Some(&budget)), write);
    assert_eq!(result.unwrap().len(), 8192 + HTTP_STATUS_TRAILER.len());
    assert_eq!(budget.remaining(), 16 * 1024 - 8192);
}

#[tokio::test]
async fn export_deadline_preserves_completed_results_and_checks_the_service() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    for final_health_valid in [true, false] {
        let active = Arc::new(AtomicUsize::new(0));
        let mut health_requests = 0;
        let result = discover_from_service(
            &DiscoveryEnvironment::Native,
            &service(),
            0,
            Instant::now() + CLI_TIMEOUT + Duration::from_millis(60),
            |path, limit, budget| {
                let active = active.clone();
                let value = if path == "/api/info" {
                    health_requests += 1;
                    if health_requests == 2 && !final_health_valid {
                        json!({"pid":9999,"version":"2.0.32"})
                    } else {
                        health()
                    }
                } else if path == session_list_path(None) {
                    page(0, 3)
                } else {
                    export(0)
                };
                async move {
                    if path.contains("/session/ses_") {
                        active.fetch_add(1, Ordering::SeqCst);
                        let _active = ActiveExport(active);
                        if path != "/api/experimental/session/ses_0/export" {
                            std::future::pending::<()>().await;
                        }
                    }
                    test_response(Ok(value.to_string().into_bytes()), limit, budget).await
                }
            },
        )
        .await;
        assert_eq!(health_requests, 2);
        if final_health_valid {
            let logs = result.unwrap();
            assert_eq!(logs.len(), 1);
            assert_eq!(logs[0].source_label(), "opencode-v2-export:ses_0");
        } else {
            assert!(result.is_err());
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while active.load(Ordering::SeqCst) > 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn failed_exports_do_not_remove_other_completed_sessions() {
    let mut responses = VecDeque::from([
        Ok(export(0).to_string().into_bytes()),
        Err(CliBridgeError::Timeout),
    ]);
    let logs = collect_v2_exports(
        &DiscoveryEnvironment::Native,
        vec![("ses_0".into(), 0), ("ses_1".into(), 1)],
        0,
        Instant::now() + Duration::from_secs(1),
        &mut |_, limit, budget| test_response(responses.pop_front().unwrap(), limit, budget),
    )
    .await
    .unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].source_label(), "opencode-v2-export:ses_0");

    let mut responses = VecDeque::from([
        Ok(b"invalid export".to_vec()),
        Ok(export(1).to_string().into_bytes()),
        Ok(export(2).to_string().into_bytes()),
    ]);
    let logs = collect_v2_exports(
        &DiscoveryEnvironment::Native,
        (0..3)
            .map(|index| (format!("ses_{index}"), index))
            .collect(),
        0,
        Instant::now() + Duration::from_secs(1),
        &mut |_, limit, budget| test_response(responses.pop_front().unwrap(), limit, budget),
    )
    .await
    .unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0].source_label(), "opencode-v2-export:ses_1");
    assert_eq!(logs[1].source_label(), "opencode-v2-export:ses_2");
}

#[tokio::test]
async fn shared_byte_limit_preserves_completed_exports() {
    let bytes = export(0).to_string().into_bytes();
    let mut requests = 0;
    let logs = collect_v2_exports(
        &DiscoveryEnvironment::Native,
        vec![("ses_0".into(), 0), ("ses_1".into(), 1)],
        V2_SESSION_MAX_BYTES - bytes.len(),
        Instant::now() + Duration::from_secs(1),
        &mut |_, limit, budget| {
            requests += 1;
            assert_eq!(limit, EXPORT_MAX_BYTES);
            test_response(Ok(bytes.clone()), limit, budget)
        },
    )
    .await
    .unwrap();
    assert_eq!(requests, 2);
    assert_eq!(logs.len(), 1);

    let result = collect_v2_exports(
        &DiscoveryEnvironment::Native,
        vec![("ses_0".into(), 0)],
        V2_SESSION_MAX_BYTES - 8,
        Instant::now() + Duration::from_secs(1),
        &mut |_, limit, budget| {
            assert_eq!(limit, EXPORT_MAX_BYTES);
            test_response(Ok(vec![b' '; 9]), limit, budget)
        },
    )
    .await;
    assert!(matches!(result, Err(CliBridgeError::Oversized)));
}

#[test]
fn database_dedupe_keeps_one_canonical_session_id_across_database_paths() {
    fn log(path: &str, format: crate::analysis::SourceFormat) -> SessionLog {
        SessionLog {
            agent_type: AgentKind::OpenCode,
            environment: DiscoveryEnvironment::Native,
            updated_at: Some(0),
            source: SessionSource::ProviderDb {
                agent: AgentKind::OpenCode,
                db_path: PathBuf::from(path),
                session_id: "ses_shared".into(),
                source_format: Some(format),
            },
        }
    }
    use crate::analysis::SourceFormat::{OpenCodeSqliteV1, OpenCodeSqliteV2};
    let v1 = log("/synthetic/opencode.db", OpenCodeSqliteV1);
    let v2 = log("/synthetic/opencode.db", OpenCodeSqliteV2);
    let other_database = log("/synthetic/opencode-beta.db", OpenCodeSqliteV1);
    assert_eq!(v1.dedupe_key(), v2.dedupe_key());
    assert_eq!(v1.dedupe_key(), "native|opencode:ses_shared");
    assert_eq!(v1.dedupe_key(), other_database.dedupe_key());
    let mut logs = vec![v1, v2, other_database];
    crate::discovery::dedupe_environment_sessions(&mut logs);
    assert_eq!(logs.len(), 1);
    assert!(matches!(
        &logs[0].source,
        SessionSource::ProviderDb {
            source_format: Some(OpenCodeSqliteV2),
            ..
        }
    ));
}

#[test]
fn v2_session_ids_use_the_native_validator() {
    for id in [
        "ses_0",
        "ses_with-hyphen",
        "ses_",
        "v1_id",
        "ses_bad/path",
        "ses_bad?query",
        "ses_bad\nline",
    ] {
        assert_eq!(
            session_id(&json!(id)),
            valid_opencode_v2_session_id(id),
            "{id:?}"
        );
    }
}

#[tokio::test]
async fn stock_full_pages_follow_opaque_cursors_until_empty_page() {
    let responses = vec![
        (session_list_path(None), page(0, 100)),
        (session_list_path(Some(&cursor(99))), page(100, 0)),
    ];
    let mut responses = VecDeque::from(responses);
    let records = list_sessions(UPDATED_MS / 1000 - 1000, &mut 0, &mut |path, _| {
        let (expected, response) = responses.pop_front().unwrap();
        assert_eq!(path, expected);
        std::future::ready(Ok(response.to_string().into_bytes()))
    })
    .await
    .unwrap();
    assert_eq!(records.len(), 100);
    assert!(responses.is_empty());
}

#[tokio::test]
async fn more_than_500_stored_sessions_keep_the_newest_100_exports() {
    let stored = (0..550).map(session).collect::<Vec<_>>();
    let mut responses = vec![("/api/info".into(), health())];
    for index in 0..5 {
        let previous = (index > 0).then(|| cursor(index * 100 - 1));
        let mut response = page(index * 100, 100);
        response["data"] = json!(&stored[index * 100..(index + 1) * 100]);
        responses.push((session_list_path(previous.as_deref()), response));
    }
    for index in 0..100 {
        responses.push((
            format!("/api/experimental/session/ses_{index}/export"),
            export(index),
        ));
    }
    responses.push(("/api/info".into(), health()));
    let logs = scripted_discovery(responses).await.unwrap();
    assert_eq!(logs.len(), 100);
    assert_eq!(
        logs.last().unwrap().source_label(),
        "opencode-v2-export:ses_99"
    );
    assert_eq!(
        V2_SESSION_MAX_PAGES * V2_SESSION_PAGE_SIZE,
        V2_SESSION_MAX_RECORDS
    );
}

#[tokio::test]
async fn list_stops_at_cutoff_and_distinguishes_successful_empty() {
    for count in [0, 2] {
        let records = list_sessions(UPDATED_MS / 1000 + 1, &mut 0, &mut |path, _| {
            assert_eq!(path, session_list_path(None));
            std::future::ready(Ok(page(0, count).to_string().into_bytes()))
        })
        .await
        .unwrap();
        assert!(records.is_empty());
    }
}

#[tokio::test]
async fn malformed_lists_and_cursor_cycles_fail_closed() {
    let mut repeated_ids = page(0, 100);
    repeated_ids["data"][1]["id"] = json!("ses_0");
    let mut wrong_time = page(0, 1);
    wrong_time["data"][0]["time"]["updated"] = Value::Null;
    let mut wrong_order = page(0, 100);
    wrong_order["data"].as_array_mut().unwrap().reverse();
    let mut missing_cursor = page(0, 100);
    missing_cursor["cursor"] = json!({});
    for response in [
        repeated_ids,
        wrong_time,
        wrong_order,
        missing_cursor,
        json!({"data":[],"cursor":null}),
        json!({"data":[{"id":"../../private"}],"cursor":{}}),
    ] {
        assert!(matches!(
            list_sessions(0, &mut 0, &mut |_, _| {
                std::future::ready(Ok(response.to_string().into_bytes()))
            })
            .await,
            Err(CliBridgeError::InvalidOutput(_))
        ));
    }
    let mut index = 0;
    let result = list_sessions(0, &mut 0, &mut |_, _| {
        let mut response = page(index * 100, 100);
        response["cursor"]["next"] = json!("same-cursor");
        index += 1;
        std::future::ready(Ok(response.to_string().into_bytes()))
    })
    .await;
    assert!(matches!(result, Err(CliBridgeError::InvalidOutput(_))));
    assert_eq!(index, 2);
    let mut response = page(0, 100);
    response["cursor"]["next"] = json!("x".repeat(CURSOR_MAX_BYTES + 1));
    assert!(matches!(
        list_sessions(0, &mut 0, &mut |_, _| {
            std::future::ready(Ok(response.to_string().into_bytes()))
        })
        .await,
        Err(CliBridgeError::InvalidOutput(_))
    ));
}

#[tokio::test]
async fn list_and_export_byte_and_record_limits_reject_oversized_data() {
    assert!(matches!(
        list_sessions(0, &mut 0, &mut |_, _| {
            std::future::ready(Ok(page(0, 101).to_string().into_bytes()))
        })
        .await,
        Err(CliBridgeError::Oversized)
    ));
    let mut total_bytes = V2_SESSION_MAX_BYTES;
    assert!(matches!(
        list_sessions(0, &mut total_bytes, &mut |_, _| {
            std::future::ready(Ok(page(0, 0).to_string().into_bytes()))
        })
        .await,
        Err(CliBridgeError::Oversized)
    ));
    assert!(matches!(
        parse_v2_session_export(&vec![b' '; EXPORT_MAX_BYTES + 1], "ses_0"),
        Err(CliBridgeError::Oversized)
    ));
    let oversized =
        json!({"data":{"info":{"id":"ses_0"},"messages":vec![Value::Null;EXPORT_MAX_MESSAGES+1]}});
    assert!(matches!(
        parse_v2_session_export(oversized.to_string().as_bytes(), "ses_0"),
        Err(CliBridgeError::Oversized)
    ));
}

#[test]
fn transfer_rejects_mismatched_ids_duplicate_messages_and_truncated_json() {
    let encoded = export(0).to_string();
    let data = parse_v2_session_export(encoded.as_bytes(), "ses_0").unwrap();
    assert_eq!(data["info"]["id"], "ses_0");
    assert!(parse_v2_session_export(encoded.as_bytes(), "ses_other").is_err());
    for cut in [1, encoded.len() / 2, encoded.len() - 1] {
        assert!(parse_v2_session_export(&encoded.as_bytes()[..cut], "ses_0").is_err());
    }
    for replacement in [
        json!({"info":{},"parts":[]}),
        json!({"id":"msg_bad","type":"user"}),
    ] {
        let mut value = export(0);
        value["data"]["messages"][0] = replacement;
        assert!(parse_v2_session_export(value.to_string().as_bytes(), "ses_0").is_err());
    }
    let mut value = export(0);
    value["data"]["messages"][1]["id"] = json!("msg_user");
    assert!(parse_v2_session_export(value.to_string().as_bytes(), "ses_0").is_err());
    let mut value = export(0);
    value["data"]["messages"][0]["sessionID"] = json!("ses_other");
    assert!(parse_v2_session_export(value.to_string().as_bytes(), "ses_0").is_err());
    let mut value = export(0);
    value["data"]["extra"] = json!(true);
    assert!(parse_v2_session_export(value.to_string().as_bytes(), "ses_0").is_err());
}

fn schema_messages() -> Vec<Value> {
    let model = json!({"id":"synthetic-model","providerID":"synthetic-provider"});
    let mut values = vec![
        json!({"type":"agent-switched","agent":"build"}),
        json!({"type":"model-switched","model":model}),
        json!({"type":"location-switched","location":{"directory":"/work/project"}}),
        json!({"type":"user","text":"synthetic request",
            "files":[{"data":"eA==","mime":"text/plain","source":{"type":"inline"}}],
            "agents":[{"name":"build"}],"skills":[{"id":"skill-test","name":"test"}]}),
        json!({"type":"synthetic","text":"synthetic context"}),
        json!({"type":"system","text":"synthetic system text"}),
        json!({"type":"skill","skill":"skill-test","name":"test","text":"synthetic skill"}),
        json!({"type":"shell","shellID":"sh_test","command":"synthetic command","status":"exited",
            "exit":0,"output":{"output":"synthetic output","cursor":16,"size":16,"truncated":false}}),
        export(0)["data"]["messages"][1].clone(),
        json!({"type":"compaction","status":"completed","reason":"auto","summary":"synthetic summary","recent":"synthetic recent"}),
        json!({"type":"idle","outcome":"succeeded"}),
    ];
    for (index, value) in values.iter_mut().enumerate() {
        value["id"] = json!(format!("msg_schema_{index}"));
        value["time"] = json!({"created":UPDATED_MS});
    }
    values
}

#[test]
fn transfer_accepts_all_pinned_public_message_types() {
    let mut value = export(0);
    value["data"]["messages"] = json!(schema_messages());
    assert!(parse_v2_session_export(value.to_string().as_bytes(), "ses_0").is_ok());
    for message in [
        json!({"type":"compaction","status":"running","reason":"manual","summary":"","recent":""}),
        json!({"type":"compaction","status":"failed","reason":"auto","error":{"type":"test","message":"synthetic error"}}),
    ] {
        let mut message = message;
        message["id"] = json!("msg_compaction");
        message["time"] = json!({"created":UPDATED_MS});
        value["data"]["messages"] = json!([message]);
        assert!(parse_v2_session_export(value.to_string().as_bytes(), "ses_0").is_ok());
    }
}

#[test]
fn transfer_rejects_missing_required_session_and_message_fields() {
    for field in ["id", "projectID", "cost", "tokens", "time", "location"] {
        let mut value = export(0);
        value["data"]["info"].as_object_mut().unwrap().remove(field);
        assert!(
            parse_v2_session_export(value.to_string().as_bytes(), "ses_0").is_err(),
            "{field}"
        );
    }
    let mut id_only = export(0);
    id_only["data"]["info"] = json!({"id":"ses_0"});
    assert!(parse_v2_session_export(id_only.to_string().as_bytes(), "ses_0").is_err());
    let messages = schema_messages();
    let fields = [
        vec!["agent"],
        vec!["model"],
        vec!["location"],
        vec!["text"],
        vec!["text"],
        vec!["text"],
        vec!["skill", "name", "text"],
        vec!["shellID", "command", "status"],
        vec!["agent", "model", "content"],
        vec!["status", "reason", "summary", "recent"],
        vec!["outcome"],
    ];
    for (message, fields) in messages.iter().zip(fields) {
        for field in fields.into_iter().chain(["id", "type", "time"]) {
            let mut malformed = message.clone();
            malformed.as_object_mut().unwrap().remove(field);
            let mut value = export(0);
            value["data"]["messages"] = json!([malformed]);
            assert!(
                parse_v2_session_export(value.to_string().as_bytes(), "ses_0").is_err(),
                "{} missing {field}",
                message["type"]
            );
        }
    }
}

#[test]
fn transfer_rejects_unknown_tags_and_wrong_nested_field_types() {
    for (path, replacement) in [
        ("/data/info/projectID", json!(7)),
        ("/data/info/cost", json!("0")),
        ("/data/info/tokens/input", json!("0")),
        ("/data/info/tokens/cache", json!([])),
        ("/data/info/time/created", Value::Null),
        ("/data/info/time/updated", json!("later")),
        ("/data/info/location/directory", json!(false)),
        ("/data/messages/0/type", json!("unknown-future-type")),
        ("/data/messages/0/text", json!({})),
        ("/data/messages/0/time/created", json!("now")),
        ("/data/messages/1/model/providerID", json!(1)),
        ("/data/messages/1/agent", Value::Null),
        ("/data/messages/1/content", json!({})),
        ("/data/messages/1/content/0/type", json!("unknown")),
        ("/data/messages/1/content/0/name", json!(5)),
        ("/data/messages/1/content/0/state/status", json!("unknown")),
        ("/data/messages/1/content/0/state/input", json!([])),
        ("/data/messages/1/content/0/state/content", json!([])),
        ("/data/messages/1/content/0/state/content/0/text", json!(7)),
    ] {
        let mut value = export(0);
        *value.pointer_mut(path).unwrap() = replacement;
        assert!(
            parse_v2_session_export(value.to_string().as_bytes(), "ses_0").is_err(),
            "{path}"
        );
    }
    for mut message in schema_messages() {
        message["time"].as_object_mut().unwrap().remove("created");
        let mut value = export(0);
        value["data"]["messages"] = json!([message]);
        assert!(parse_v2_session_export(value.to_string().as_bytes(), "ses_0").is_err());
    }
}

#[test]
fn transfer_rejects_missing_required_nested_fields_and_invalid_variants() {
    for (parent, field) in [
        ("/data/info/time", "created"),
        ("/data/info/time", "updated"),
        ("/data/info/location", "directory"),
        ("/data/info/tokens", "cache"),
        ("/data/info/tokens/cache", "read"),
        ("/data/info/tokens/cache", "write"),
        ("/data/messages/1/model", "id"),
        ("/data/messages/1/model", "providerID"),
        ("/data/messages/1/content/0", "state"),
        ("/data/messages/1/content/0", "time"),
        ("/data/messages/1/content/0/state", "input"),
        ("/data/messages/1/content/0/state", "status"),
        ("/data/messages/1/content/0/state", "content"),
    ] {
        let mut value = export(0);
        value
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            parse_v2_session_export(value.to_string().as_bytes(), "ses_0").is_err(),
            "{parent}/{field}"
        );
    }
    for (index, field, value) in [
        (0, "agent", json!(false)),
        (1, "model", json!({"id":"missing-provider"})),
        (2, "location", json!({})),
        (
            3,
            "files",
            json!([{"data":"eA==","mime":7,"source":{"type":"inline"}}]),
        ),
        (4, "text", json!(null)),
        (5, "text", json!(false)),
        (6, "name", json!(7)),
        (7, "status", json!("unsupported")),
        (9, "reason", json!("unsupported")),
        (9, "status", json!("unsupported")),
        (10, "outcome", json!("unsupported")),
    ] {
        let mut message = schema_messages().swap_remove(index);
        message[field] = value;
        let mut value = export(0);
        value["data"]["messages"] = json!([message]);
        assert!(
            parse_v2_session_export(value.to_string().as_bytes(), "ses_0").is_err(),
            "message {index} {field}"
        );
    }
}

#[tokio::test]
async fn health_pid_version_and_mid_scan_replacement_fail_closed() {
    for info in [
        json!({"pid":1235,"version":"2.0.32"}),
        json!({"pid":1234,"version":"1.2.0"}),
        json!({"pid":1234}),
        json!({"error":"synthetic-private-error"}),
    ] {
        let error = probe_service(&service(), info.to_string().as_bytes())
            .unwrap_err()
            .to_string();
        assert_eq!(error, "registered WSL service PID or version did not match");
        assert!(!error.contains("synthetic-private"));
    }
    assert!(
        scripted_discovery(vec![
            ("/api/info".into(), health()),
            (session_list_path(None), page(0, 0)),
            ("/api/info".into(), json!({"pid":9999,"version":"2.0.32"})),
        ])
        .await
        .is_err()
    );
}

#[tokio::test]
async fn discovery_rejects_malformed_or_wrong_session_export_without_leaking_content() {
    for body in [
        br#"{"data":"synthetic-private-content"}"#.as_slice(),
        br#"{"data":{"info":{"id":"ses_other"},"messages":[]}}"#,
        br#"{"data":{"info":{"id":"ses_0"},"messages":["synthetic-private-content""#,
    ] {
        let mut index = 0;
        let result = discover_from_service(
            &DiscoveryEnvironment::Native,
            &service(),
            0,
            Instant::now() + V2_DISCOVERY_TIMEOUT,
            |_, limit, budget| {
                index += 1;
                let bytes = match index {
                    1 => health().to_string().into_bytes(),
                    2 => page(0, 1).to_string().into_bytes(),
                    3 => body.to_vec(),
                    _ => panic!("discovery continued after an invalid export"),
                };
                test_response(Ok(bytes), limit, budget)
            },
        )
        .await;
        let error = result.unwrap_err().to_string();
        assert!(error.contains("V2 session export"));
        assert!(!error.contains("synthetic-private"));
    }
    let error = parse_cli_session_rows(
        br#"[{"id":"ses_v1","title":{},"directory":"synthetic-private-path"}]"#,
    )
    .unwrap_err()
    .to_string();
    assert_eq!(error, "OpenCode session metadata was invalid");
}

#[test]
fn v2_curl_command_allows_only_explicit_local_read_routes() {
    for path in [
        "/api/info",
        session_list_path(None).as_str(),
        session_list_path(Some("opaque/cursor")).as_str(),
        "/api/experimental/session/ses_0/export",
    ] {
        let command =
            build_v2_http_command(&DiscoveryEnvironment::Native, "http://127.0.0.1:4096", path)
                .unwrap();
        let args = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(command.as_std().get_program(), "curl");
        assert_eq!(args[0], "--disable");
        assert!(args.windows(2).any(|pair| pair == ["--proxy", ""]));
        assert!(args.windows(2).any(|pair| pair == ["--noproxy", "*"]));
        assert!(args.windows(2).any(|pair| pair == ["--request", "GET"]));
        assert!(args.windows(2).any(|pair| pair == ["--config", "-"]));
        assert!(
            !args.iter().any(
                |arg| ["--pure", "--location", "-L", "api", "--server"].contains(&arg.as_str())
            )
        );
        assert!(!args.iter().any(|arg| arg.contains("synthetic-password")));
    }
    for path in [
        "/api/session",
        "/api/session?limit=1000",
        "/api/session?limit=100&cursor=x&unexpected=1",
        "/api/experimental/session/../../export",
        "/api/experimental/session/ses_0/export?extra=1",
    ] {
        assert!(
            build_v2_http_command(&DiscoveryEnvironment::Native, "http://127.0.0.1:4096", path)
                .is_err()
        );
    }
    assert_eq!(encode_query_component("a b&c"), "a%20b%26c");
}

#[test]
fn characterized_transfer_keeps_stored_tools_without_native_live_results() {
    use crate::analysis::{NormalizedRecord, RecordSink, SessionSummary};
    #[derive(Default)]
    struct Sink {
        native_results: usize,
        gaps: usize,
    }
    impl RecordSink for Sink {
        fn record(&mut self, record: NormalizedRecord) {
            if matches!(record, NormalizedRecord::ToolResult(_)) {
                self.native_results += 1;
            }
        }
        fn tool_result_gap(&mut self) {
            self.gaps += 1;
        }
        fn finish(&mut self, _: SessionSummary) {}
    }
    let input = crate::analysis::SessionInput {
        agent: "opencode".into(),
        session_id: "ses_0".into(),
        source_format: crate::analysis::SourceFormat::OpenCodeV2SessionExport,
        source: crate::analysis::RawSource::Jsonl(
            parse_v2_session_export(export(0).to_string().as_bytes(), "ses_0")
                .unwrap()
                .to_string(),
        ),
        fork_parent_session_id: None,
    };
    let reader = crate::analysis::reader_for_input(&input);
    assert_eq!(
        reader.capabilities(&input).source_format,
        input.source_format
    );
    let normalized = reader.normalize(&input).unwrap();
    assert_eq!(
        normalized
            .events
            .iter()
            .map(|event| event.tools.len())
            .sum::<usize>(),
        1
    );
    let mut sink = Sink::default();
    reader.visit(&input, &mut sink).unwrap();
    assert_eq!(sink.native_results, 0);
    assert_eq!(sink.gaps, 1);
}

#[cfg(unix)]
mod command_harness {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub(super) async fn install(dir: &Path, relative: &str, body: &str) -> String {
        let path = dir.join(relative);
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, format!("#!/bin/sh\n{body}\n"))
            .await
            .unwrap();
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .await
            .unwrap();
        path.to_string_lossy().into_owned()
    }

    fn info(home: &Path) -> WslEnvironmentInfo {
        WslEnvironmentInfo {
            context: crate::platform::environment::AgentContext::native(home.to_owned()),
            distribution: "synthetic".into(),
            user: "synthetic".into(),
        }
    }

    #[tokio::test]
    async fn common_v1_bridge_still_exports_without_service_registration() {
        let dir = tempfile::tempdir().unwrap();
        let metadata = json!([{"id":"ses_wsl_v1_harness","directory":"/work/project",
            "title":"synthetic V1", "time_created":UPDATED_MS,"time_updated":UPDATED_MS}]);
        let transcript = json!({"info":{"id":"ses_wsl_v1_harness"},"messages":[
            {"info":{"id":"msg_v1","role":"user","sessionID":"ses_wsl_v1_harness","time":{"created":UPDATED_MS}},
             "parts":[{"id":"prt_v1","type":"text","text":"synthetic request"}]}
        ]});
        let script = format!(
            r#"
[ "$1" = '--pure' ] || exit 91
case "$2" in
  db) printf '%s' '{}' ;;
  export) [ "$3" = 'ses_wsl_v1_harness' ] || exit 92; printf '%s' '{}' ;;
  *) exit 93 ;;
esac
"#,
            metadata, transcript
        );
        install(dir.path(), ".local/bin/opencode", &script).await;
        let logs =
            super::super::super::discover_recent_in_wsl(&info(dir.path()), UPDATED_MS / 1000, 100)
                .await;
        assert_eq!(logs.len(), 1);
        assert!(!logs[0].source_label().starts_with("opencode-v2-export:"));
        assert!(
            crate::discovery::session_source_content(&logs[0].source)
                .await
                .unwrap()
                .contains("synthetic request")
        );
    }
}

mod http_harness {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Server {
        url: String,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Server {
        fn start(mut respond: impl FnMut(&mut TcpStream, &str) + Send + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = requests.clone();
            let thread = std::thread::spawn(move || {
                while !stopped.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream.set_nonblocking(false).unwrap();
                            stream
                                .set_read_timeout(Some(Duration::from_secs(1)))
                                .unwrap();
                            stream
                                .set_write_timeout(Some(Duration::from_secs(1)))
                                .unwrap();
                            let mut request = Vec::new();
                            let mut chunk = [0; 1024];
                            while request.len() < 16 * 1024 && !request.ends_with(b"\r\n\r\n") {
                                match stream.read(&mut chunk) {
                                    Ok(0) | Err(_) => break,
                                    Ok(count) => request.extend_from_slice(&chunk[..count]),
                                }
                            }
                            let request = String::from_utf8(request).unwrap();
                            captured.lock().unwrap().push(request.clone());
                            respond(&mut stream, &request);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(1))
                        }
                        Err(error) => panic!("test listener failed: {error}"),
                    }
                }
            });
            Self {
                url,
                requests,
                stop,
                thread: Some(thread),
            }
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.thread.take().unwrap().join().unwrap();
        }
    }

    fn curl_available() -> bool {
        let available = std::process::Command::new("curl")
            .args(["--disable", "--version"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !available {
            eprintln!("curl is unavailable; skipping the local HTTP runtime test");
        }
        available
    }

    fn reply(stream: &mut TcpStream, status: u16, extra: &str, body: &[u8]) {
        // Cancellation and bounded reads can close the test connection early.
        let _ = write!(
            stream,
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
            body.len()
        )
        .and_then(|()| stream.write_all(body));
    }

    fn command(server: &Server, path: &str) -> tokio::process::Command {
        build_v2_http_command(&DiscoveryEnvironment::Native, &server.url, path).unwrap()
    }

    async fn register(home: &Path, server: &Server) -> WslEnvironmentInfo {
        let path = home.join(".local/state/opencode/service.json");
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        let mut registration = registration();
        registration["url"] = json!(server.url);
        tokio::fs::write(path, registration.to_string())
            .await
            .unwrap();
        WslEnvironmentInfo {
            context: crate::platform::environment::AgentContext::native(home.to_owned()),
            distribution: "synthetic".into(),
            user: "synthetic".into(),
        }
    }

    #[tokio::test]
    async fn registered_service_discovery_uses_real_curl_without_an_opencode_executable() {
        if !curl_available() {
            return;
        }
        let server = Server::start(|stream, request| {
            let body = match request.split_whitespace().nth(1).unwrap() {
                "/api/info" => health(),
                "/api/session?limit=100&order=desc" => page(0, 1),
                "/api/experimental/session/ses_0/export" => export(0),
                _ => panic!("unexpected request path"),
            };
            reply(stream, 200, "", body.to_string().as_bytes());
        });
        let dir = tempfile::tempdir().unwrap();
        let info = register(dir.path(), &server).await;
        let logs = discover_recent_v2_api(&info, UPDATED_MS / 1000 - 100)
            .await
            .unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].source_label(), "opencode-v2-export:ses_0");
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(requests.iter().all(|request| request.starts_with("GET ")));
        let encoded =
            base64::engine::general_purpose::STANDARD.encode("opencode:synthetic-password");
        assert!(
            requests
                .iter()
                .all(|request| request.contains(&format!("Authorization: Basic {encoded}")))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_v2_discovery_still_probes_v1_with_read_only_cli_flags() {
        assert!(curl_available(), "The local bridge test requires curl");
        let server = Server::start(|stream, request| {
            let body = match request.split_whitespace().nth(1).unwrap() {
                "/api/info" => health(),
                "/api/session?limit=100&order=desc" => page(0, 1),
                "/api/experimental/session/ses_0/export" => export(0),
                _ => panic!("unexpected request path"),
            };
            reply(stream, 200, "", body.to_string().as_bytes());
        });
        let dir = tempfile::tempdir().unwrap();
        let info = register(dir.path(), &server).await;
        let registration_path = dir.path().join(".local/state/opencode/service.json");
        let before = tokio::fs::read(&registration_path).await.unwrap();
        super::command_harness::install(
            dir.path(),
            ".opencode/bin/opencode",
            r#"
directory=${0%/*}
printf '%s\n' "$@" >> "$directory/args"
printf '%s\n' "${OPENCODE_DISABLE_AUTOUPDATE-}" "${OPENCODE_DISABLE_MODELS_FETCH-}" \
  "${OPENCODE_DISABLE_DEFAULT_PLUGINS-}" "${OPENCODE_SKIP_MIGRATIONS-}" >> "$directory/flags"
printf '[]\n'
"#,
        )
        .await;
        let now = UPDATED_MS / 1000;
        let logs = discover_recent_in_wsl(&info, now, 100).await;
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].source_label(), "opencode-v2-export:ses_0");
        let args = tokio::fs::read_to_string(dir.path().join(".opencode/bin/args"))
            .await
            .unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            [
                "--pure",
                "db",
                cli_session_metadata_query(now - 100).as_str(),
                "--format",
                "json",
            ]
        );
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join(".opencode/bin/flags"))
                .await
                .unwrap(),
            "true\ntrue\ntrue\ntrue\n"
        );
        assert_eq!(tokio::fs::read(registration_path).await.unwrap(), before);
        assert_eq!(server.requests.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn redirects_never_contact_the_destination_including_health_reads() {
        if !curl_available() {
            return;
        }
        let destination = TcpListener::bind("127.0.0.1:0").unwrap();
        destination.set_nonblocking(true).unwrap();
        let location = format!(
            "Location: http://{}/private\r\n",
            destination.local_addr().unwrap()
        );
        let server = Server::start(move |stream, _| {
            reply(stream, 302, &location, b"synthetic-private-redirect")
        });
        for path in [
            "/api/info",
            "/api/session?limit=100&order=desc",
            "/api/experimental/session/ses_0/export",
        ] {
            let error = run_v2_http(
                &DiscoveryEnvironment::Native,
                &server.url,
                path,
                Some("synthetic-password"),
                1024,
                None,
            )
            .await
            .unwrap_err()
            .to_string();
            assert_eq!(error, "V2 HTTP read did not return HTTP 200");
            assert!(
                matches!(destination.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock)
            );
        }
        assert_eq!(server.requests.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn curl_ignores_rc_proxy_and_inherited_agent_passwords_and_escapes_credentials() {
        if !curl_available() {
            return;
        }
        let destination = TcpListener::bind("127.0.0.1:0").unwrap();
        destination.set_nonblocking(true).unwrap();
        let server = Server::start(|stream, _| reply(stream, 200, "", b"{}"));
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".curlrc"),
            format!(
                "url = \"http://{}/private\"\nlocation\n",
                destination.local_addr().unwrap()
            ),
        )
        .unwrap();
        for password in [None, Some("synthetic\"\\\t\nsecret")] {
            let mut command = command(&server, "/api/info");
            command
                .env("CURL_HOME", dir.path())
                .env("HOME", dir.path())
                .env(
                    "http_proxy",
                    format!("http://{}", destination.local_addr().unwrap()),
                )
                .env(
                    "ALL_PROXY",
                    format!("http://{}", destination.local_addr().unwrap()),
                )
                .env("OPENCODE_PASSWORD", "synthetic-stale-secret")
                .env("OPENCODE_SERVER_PASSWORD", "synthetic-stale-secret");
            let args = format!("{:?}", command.as_std().get_args().collect::<Vec<_>>());
            assert!(!args.contains("secret"));
            assert_eq!(
                run_command(command, password, 1024, Duration::from_secs(2), None)
                    .await
                    .unwrap(),
                b"{}"
            );
            let requests = server.requests.lock().unwrap();
            let request = requests.last().unwrap();
            if let Some(password) = password {
                let encoded = base64::engine::general_purpose::STANDARD
                    .encode(format!("opencode:{password}"));
                assert!(request.contains(&format!("Authorization: Basic {encoded}")));
                assert!(!args.contains(&encoded));
            } else {
                assert!(!request.to_lowercase().contains("authorization:"));
            }
        }
        assert!(
            matches!(destination.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock)
        );
        assert_eq!(server.requests.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn real_curl_streaming_stops_oversized_bodies_before_the_full_body_arrives() {
        if !curl_available() {
            return;
        }
        const TOTAL: usize = 32 * 1024 * 1024;
        let sent = Arc::new(AtomicUsize::new(0));
        let written = sent.clone();
        let server = Server::start(move |stream, _| {
            if write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {TOTAL}\r\nConnection: close\r\n\r\n"
            )
            .is_err()
            {
                return;
            }
            for _ in 0..TOTAL / 8192 {
                if stream.write_all(&[b'x'; 8192]).is_err() {
                    break;
                }
                written.fetch_add(8192, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let result = run_command(
            command(&server, "/api/info"),
            None,
            1024,
            Duration::from_secs(2),
            None,
        )
        .await;
        assert!(matches!(result, Err(CliBridgeError::Oversized)));
        drop(server);
        assert!(sent.load(Ordering::SeqCst) < TOTAL);
    }

    #[tokio::test]
    async fn real_curl_uses_the_shared_streaming_byte_limit() {
        if !curl_available() {
            return;
        }
        let server = Server::start(|stream, _| reply(stream, 200, "", &[b'x'; 768]));
        let budget = Arc::new(ExportByteBudget::new(1024));
        let first = run_command(
            command(&server, "/api/info"),
            None,
            1024,
            Duration::from_secs(2),
            Some(budget.clone()),
        )
        .await
        .unwrap();
        assert_eq!(first.len(), 768);
        assert_eq!(budget.remaining(), 256);
        let second = run_command(
            command(&server, "/api/info"),
            None,
            1024,
            Duration::from_secs(2),
            Some(budget.clone()),
        )
        .await;
        assert!(matches!(second, Err(CliBridgeError::Oversized)));
        assert_eq!(budget.remaining(), 0);
    }

    #[tokio::test]
    async fn real_curl_and_host_deadlines_stop_delayed_responses() {
        if !curl_available() {
            return;
        }
        for host_timeout in [true, false] {
            let server = Server::start(|stream, _| {
                std::thread::sleep(Duration::from_millis(200));
                reply(stream, 200, "", b"{}");
            });
            let mut command = command(&server, "/api/info");
            let timeout = if host_timeout {
                Duration::from_millis(40)
            } else {
                command.args(["--max-time", "0.04"]);
                Duration::from_secs(2)
            };
            let result = run_command(command, None, 1024, timeout, None).await;
            assert!(matches!(result, Err(CliBridgeError::Timeout)));
        }
    }

    #[tokio::test]
    async fn non_200_and_incomplete_http_responses_keep_failure_bodies_private() {
        if !curl_available() {
            return;
        }
        for status in [201, 204, 206, 301, 307, 401, 500] {
            let server = Server::start(move |stream, _| {
                reply(
                    stream,
                    status,
                    "",
                    b"synthetic-private-output\nantiburn-http-status:200\n",
                )
            });
            let error = run_v2_http(
                &DiscoveryEnvironment::Native,
                &server.url,
                "/api/info",
                None,
                1024,
                None,
            )
            .await
            .unwrap_err()
            .to_string();
            assert!(!error.contains("synthetic-private"));
            assert!(error.contains("HTTP 200") || error.contains("HTTP read failed"));
        }
        let server = Server::start(|stream, _| {
            let _ =
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"truncated\":");
        });
        assert!(
            run_v2_http(
                &DiscoveryEnvironment::Native,
                &server.url,
                "/api/info",
                None,
                1024,
                None,
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn missing_curl_has_a_clear_private_unavailable_error() {
        let dir = tempfile::tempdir().unwrap();
        let command = tokio::process::Command::new(dir.path().join("missing-curl"));
        let error = run_command(
            command,
            Some("synthetic-private-secret"),
            1024,
            Duration::from_secs(1),
            None,
        )
        .await
        .unwrap_err()
        .to_string();
        assert_eq!(
            error,
            "WSL V2 HTTP reads require curl in the selected distribution"
        );
    }

    #[tokio::test]
    async fn registration_replacement_during_real_http_discovery_rejects_the_scan() {
        if !curl_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let registration_path = dir.path().join(".local/state/opencode/service.json");
        let server = Server::start(move |stream, request| {
            let body = if request.starts_with("GET /api/info ") {
                health()
            } else {
                let mut registration: Value =
                    serde_json::from_slice(&std::fs::read(&registration_path).unwrap()).unwrap();
                registration["id"] = json!("replacement");
                std::fs::write(&registration_path, registration.to_string()).unwrap();
                page(0, 0)
            };
            reply(stream, 200, "", body.to_string().as_bytes());
        });
        let info = register(dir.path(), &server).await;
        let error = discover_recent_v2_api(&info, 0)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(error, "registered WSL service changed during discovery");
    }
}
