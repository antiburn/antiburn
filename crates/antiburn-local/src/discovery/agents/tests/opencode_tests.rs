use super::*;

fn db_session_admission_revision_connection(
    connection: &rusqlite::Connection,
    session_id: &str,
) -> Option<String> {
    let identity = match connection.path()? {
        "" => "memory".to_owned(),
        path => {
            let file = std::fs::File::open(path).ok()?;
            crate::discovery::SourceStat::from_open_std_file(&file)?.identity?
        }
    };
    crate::discovery::agents::opencode::db_session_admission_revision_connection_with_identity(
        connection, session_id, &identity,
    )
}

fn db_session_fingerprint_connection(
    connection: &rusqlite::Connection,
    session_id: &str,
) -> Option<(u64, u64)> {
    let identity = match connection.path()? {
        "" => "memory".to_owned(),
        path => {
            let file = std::fs::File::open(path).ok()?;
            crate::discovery::SourceStat::from_open_std_file(&file)?.identity?
        }
    };
    crate::discovery::agents::opencode::db_session_fingerprint_cancellable_with_identity(
        connection,
        session_id,
        &identity,
        &|| false,
    )
}
use crate::discovery::{FORK_OBSERVATION_KEY, ForkObservation};

/// Read back the [`ForkObservation`] the adapter embedded in a rendered
/// session's synthetic metadata header.
fn embedded_fork_observation(content: &str) -> Option<ForkObservation> {
    let value: Value = serde_json::from_str(content.lines().next()?).ok()?;
    serde_json::from_value(value.get(FORK_OBSERVATION_KEY)?.clone()).ok()
}
use crate::discovery::set_file_mtime;
use rusqlite::Connection;
use serial_test::serial;

#[path = "opencode_native_tests.rs"]
mod native;

fn recent_db_session_logs(root: &Path, cutoff: i64) -> Vec<SessionLog> {
    match query_recent_db_session_sources(root, cutoff) {
        DbSourceRead::Ready(logs) => logs,
        DbSourceRead::NotPresent => panic!("expected a database source"),
        DbSourceRead::UnsupportedSchema => panic!("expected a supported database schema"),
        DbSourceRead::Failed(stage) => panic!("database discovery failed at {stage}"),
    }
}

async fn write_json(path: &Path, value: &str) {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.unwrap();
    }
    tokio::fs::write(path, value).await.unwrap();
}

fn cli_record(kind: &str, id: &str, message_id: Option<&str>, data_len: usize) -> CliRecordRow {
    CliRecordRow {
        kind: kind.to_string(),
        id: id.to_string(),
        message_id: message_id.map(str::to_string),
        session_id: "ses_root".to_string(),
        time_created: Some(10),
        time_updated: Some(20),
        data_len,
    }
}

#[test]
fn cli_record_validation_rejects_wrong_session_kind_and_aggregate_size() {
    let mut row = cli_record("message", "msg_1", None, 10);
    assert!(validate_cli_record_rows(&[row], "ses_root").is_ok());

    row = cli_record("unknown", "msg_1", None, 10);
    assert!(matches!(
        validate_cli_record_rows(&[row], "ses_root"),
        Err(CliBridgeError::InvalidOutput(_))
    ));

    row = cli_record("message", "msg_1", None, CLI_EXPORT_MAX_BYTES + 1);
    assert!(matches!(
        validate_cli_record_rows(&[row], "ses_root"),
        Err(CliBridgeError::Oversized)
    ));
}

#[test]
fn provider_usage_query_reads_only_recent_assistant_messages() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE message (id TEXT PRIMARY KEY, time_created INTEGER, data TEXT);",
        )
        .unwrap();
    for (id, created, role) in [
        ("recent-assistant", 2_000_000_i64, "assistant"),
        ("newest-user", 2_100_000_i64, "user"),
        ("second-assistant", 2_050_000_i64, "assistant"),
        ("old-assistant", 500_000_i64, "assistant"),
    ] {
        connection
            .execute(
                "INSERT INTO message (id, time_created, data) VALUES (?1, ?2, ?3)",
                params![
                    id,
                    created,
                    serde_json::json!({
                        "id": id,
                        "role": role,
                        "sessionID": "session",
                        "providerID": "openai",
                        "modelID": "gpt",
                        "cost": 0.1,
                        "tokens": {"input": 1, "output": 1},
                        "time": {"created": created}
                    })
                    .to_string()
                ],
            )
            .unwrap();
    }

    let records = query_recent_assistant_messages_from_db(&path, 1_000, 2);

    assert_eq!(records.len(), 2);
    assert!(records[0].contains("second-assistant"));
    assert!(records[1].contains("recent-assistant"));
}

#[test]
fn reconstructed_records_replace_untrusted_identity_fields() {
    let record = normalize_cli_record(
        cli_record("part", "part_1", Some("msg_1"), 2),
        r#"{"id":"spoof","sessionID":"other","messageID":"other","text":"ok"}"#,
    )
    .unwrap();
    let ReconstructedRecord::Part { message_id, value } = record else {
        panic!("expected reconstructed part");
    };

    assert_eq!(message_id, "msg_1");
    assert_eq!(value["id"], "part_1");
    assert_eq!(value["sessionID"], "ses_root");
    assert_eq!(value["messageID"], "msg_1");
}

#[test]
fn cli_chunk_parsing_reassembles_exact_utf8_and_rejects_mutation() {
    assert_eq!(
        parse_cli_data_chunk(br#"[{"chunk":"6869"}]"#).unwrap(),
        b"hi"
    );
    assert_eq!(finish_cli_record_data(b"hi".to_vec(), 2).unwrap(), "hi");
    assert!(matches!(
        finish_cli_record_data(b"hi".to_vec(), 3),
        Err(CliBridgeError::InvalidOutput(_))
    ));
    assert!(matches!(
        parse_cli_data_chunk(br#"[{"chunk":"abc"}]"#),
        Err(CliBridgeError::InvalidOutput(_))
    ));
}

#[test]
fn recent_child_selects_its_whole_cli_cluster() {
    let environment = DiscoveryEnvironment::Wsl {
        distribution: "Ubuntu".to_string(),
        user: "dev".to_string(),
    };
    let sessions = session_records_from_cli_rows(
        &environment,
        vec![
            CliSessionRow {
                id: "ses_root".into(),
                parent_id: None,
                directory: Some("/repo".into()),
                title: None,
                time_created: Some(1),
                time_updated: Some(1),
            },
            CliSessionRow {
                id: "ses_child".into(),
                parent_id: Some("ses_root".into()),
                directory: Some("/repo".into()),
                title: None,
                time_created: Some(100),
                time_updated: Some(100),
            },
        ],
    );

    let clusters = recent_cli_clusters(&sessions, 50);
    assert_eq!(clusters.len(), 1);
    assert_eq!(clusters[0].root_session_id, "ses_root");
    assert_eq!(clusters[0].session_ids.len(), 2);
}

#[tokio::test]
async fn cli_export_collection_keeps_valid_sessions_when_one_fails() {
    let exports = vec![
        std::future::ready(Ok((
            "ses_valid".to_string(),
            10,
            r#"{"info":{"id":"ses_valid"},"messages":[]}"#.to_string(),
            "test-valid".to_string(),
        ))),
        std::future::ready(Ok((
            "ses_invalid".to_string(),
            10,
            "not-json".to_string(),
            "test-invalid".to_string(),
        ))),
    ];

    let collected = collect_cli_exports(exports).await.unwrap();
    assert_eq!(collected.len(), 1);
    assert!(collected.contains_key("ses_valid"));
    assert!(!collected.contains_key("ses_invalid"));
}

#[tokio::test]
async fn cli_export_collection_falls_back_when_every_session_fails() {
    let exports = vec![std::future::ready(Ok((
        "ses_invalid".to_string(),
        10,
        "not-json".to_string(),
        "test-invalid-only".to_string(),
    )))];

    assert!(matches!(
        collect_cli_exports(exports).await,
        Err(CliBridgeError::InvalidOutput(_))
    ));
}

#[tokio::test]
async fn cli_export_collection_bounds_tasks_and_refills_before_slow_exports_finish() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let started = Arc::new(AtomicUsize::new(0));
    let changed = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let jobs = (0..12).map(|index| {
        started.fetch_add(1, Ordering::SeqCst);
        let changed = changed.clone();
        let release = release.clone();
        async move {
            changed.notify_one();
            release.acquire().await.unwrap().forget();
            let id = format!("ses_bounded_{index}");
            Ok((
                id.clone(),
                1,
                format!(r#"{{"info":{{"id":"{id}"}},"messages":[]}}"#),
                format!("bounded-test:{id}"),
            ))
        }
    });
    let collect = collect_cli_exports(jobs);
    tokio::pin!(collect);
    let check = async {
        while started.load(Ordering::SeqCst) < CLI_EXPORT_CONCURRENCY {
            changed.notified().await;
        }
        assert_eq!(started.load(Ordering::SeqCst), CLI_EXPORT_CONCURRENCY);
        release.add_permits(1);
        while started.load(Ordering::SeqCst) < CLI_EXPORT_CONCURRENCY + 1 {
            changed.notified().await;
        }
        assert_eq!(started.load(Ordering::SeqCst), CLI_EXPORT_CONCURRENCY + 1);
        release.add_permits(12);
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(collect, check)
    })
    .await
    .unwrap();
    assert_eq!(result.unwrap().len(), 12);
}

#[test]
fn cli_export_converts_messages_and_parts_to_cluster_records() {
    let exports = HashMap::from([
            (
                "ses_root".to_string(),
                r#"{"info":{"id":"ses_root"},"messages":[{"info":{"id":"msg_root","sessionID":"ses_root","role":"user","time":{"created":1700000000000}},"parts":[{"id":"part_root","type":"text","text":"hello"}]}]}"#.to_string(),
            ),
            (
                "ses_child".to_string(),
                r#"{"info":{"id":"ses_child"},"messages":[{"info":{"id":"msg_child","sessionID":"ses_child","role":"assistant","time":{"created":1700000001000}},"parts":[{"id":"part_child","type":"text","text":"done"}]}]}"#.to_string(),
            ),
        ]);
    let (messages, parts) = records_from_cli_exports(&exports).unwrap();
    assert_eq!(messages["ses_root"][0].id, "msg_root");
    assert_eq!(
        parts["ses_child"][0].message_id.as_deref(),
        Some("msg_child")
    );
    assert_eq!(parts["ses_child"][0].raw["text"], "done");
}

#[test]
fn cli_metadata_rejects_unvalidated_ids() {
    let error = parse_cli_session_rows(
        br#"[{"id":"../../escape","parent_id":null,"time_updated":1700000000000}]"#,
    )
    .unwrap_err();
    assert!(matches!(error, CliBridgeError::InvalidOutput(_)));
}

#[tokio::test]
async fn fragmented_fallback_never_reads_an_adjacent_sqlite_store() {
    let root = tempfile::TempDir::new().unwrap();
    let db = root.path().join("opencode.db");
    Connection::open(&db).unwrap();
    let session = root.path().join("storage/session/project/ses_file.json");
    write_json(
            &session,
            r#"{"id":"ses_file","directory":"/repo","time":{"created":1700000000000,"updated":1700000000000}}"#,
        )
        .await;
    set_file_mtime(&session, 1_700_000_000);

    let logs =
        discover_recent_in_impl(&[root.path().to_path_buf()], 1_700_000_100, 1_000, false).await;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].source_label(), "opencode:ses_file");
    assert!(matches!(logs[0].source, SessionSource::Inline { .. }));
}

fn session_record(
    directory: Option<&str>,
    parent_session_id: Option<&str>,
    created_at: Option<i64>,
    updated_at: Option<i64>,
) -> SessionRecord {
    SessionRecord {
        directory: directory.map(str::to_string),
        title: Some("session".to_string()),
        parent_session_id: parent_session_id.map(str::to_string),
        source_file: "session.json".to_string(),
        created_at,
        updated_at,
        file_mtime: None,
        raw: json!({
            "directory": directory,
            "parentID": parent_session_id,
            "time": {
                "created": created_at,
                "updated": updated_at,
            }
        }),
    }
}

fn message_record(session_id: &str, id: &str, created_at: i64) -> MessageRecord {
    MessageRecord {
        id: id.to_string(),
        session_id: session_id.to_string(),
        role: Some("assistant".to_string()),
        source_file: format!("{id}.json"),
        created_at: Some(created_at),
        file_mtime: None,
        raw: json!({
            "id": id,
            "sessionID": session_id,
            "time": {
                "created": created_at,
            }
        }),
    }
}

fn inline_content(log: &SessionLog) -> &str {
    match &log.source {
        SessionSource::Inline { content, .. } => content,
        SessionSource::File(_) => panic!("expected inline session"),
        SessionSource::ProviderDb { .. } => panic!("expected inline session"),
    }
}

#[tokio::test]
async fn test_opencode_normalizes_session_messages_and_parts() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();

    write_json(
            &root
                .join("storage")
                .join("session")
                .join("global")
                .join("ses_1.json"),
            r#"{"id":"ses_1","directory":"/repo","time":{"created":1772602456612,"updated":1772602457665}}"#,
        )
        .await;

    write_json(
        &root
            .join("storage")
            .join("message")
            .join("ses_1")
            .join("msg_1.json"),
        r#"{"id":"msg_1","sessionID":"ses_1","role":"user","time":{"created":1772602458000}}"#,
    )
    .await;

    write_json(
            &root
                .join("storage")
                .join("part")
                .join("msg_1")
                .join("prt_1.json"),
            r#"{"id":"prt_1","sessionID":"ses_1","messageID":"msg_1","type":"text","time":{"created":1772602459000}}"#,
        )
        .await;

    let logs = discover_recent_in(&[root.to_path_buf()], 1_772_602_700, 9_999_999).await;
    assert_eq!(logs.len(), 1);

    match &logs[0].source {
        SessionSource::Inline { label, content } => {
            assert_eq!(label, "opencode:ses_1");
            assert!(content.contains("\"type\":\"session_meta\""));
            assert!(content.contains("\"type\":\"message\""));
            assert!(content.contains("\"type\":\"part\""));
            assert!(content.contains("\"sessionID\":\"ses_1\""));
        }
        SessionSource::File(_) => panic!("expected inline session"),
        SessionSource::ProviderDb { .. } => panic!("expected inline session"),
    }
}

#[tokio::test]
async fn test_opencode_filters_by_cutoff_using_latest_fragment_time() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();

    write_json(
        &root
            .join("storage")
            .join("session")
            .join("global")
            .join("ses_old.json"),
        r#"{"id":"ses_old","time":{"updated":1000}}"#,
    )
    .await;

    let logs = discover_recent_in(&[root.to_path_buf()], 10_000, 100).await;
    assert!(logs.is_empty());
}

#[tokio::test]
async fn test_opencode_partial_data_still_emits_session() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();

    write_json(
            &root
                .join("storage")
                .join("message")
                .join("ses_partial")
                .join("msg_1.json"),
            r#"{"id":"msg_1","sessionID":"ses_partial","role":"user","time":{"created":1772602458000}}"#,
        )
        .await;

    let logs = discover_recent_in(&[root.to_path_buf()], 1_772_602_700, 9_999_999).await;
    assert_eq!(logs.len(), 1);
    match &logs[0].source {
        SessionSource::Inline { label, content } => {
            assert_eq!(label, "opencode:ses_partial");
            assert!(content.contains("\"type\":\"session_meta\""));
            assert!(content.contains("\"type\":\"message\""));
        }
        SessionSource::File(_) => panic!("expected inline session"),
        SessionSource::ProviderDb { .. } => panic!("expected inline session"),
    }
}

#[tokio::test]
async fn test_opencode_untimestamped_session_uses_file_mtime_for_cutoff() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();

    let session_file = root
        .join("storage")
        .join("session")
        .join("global")
        .join("ses_untimed.json");
    write_json(&session_file, r#"{"id":"ses_untimed","directory":"/repo"}"#).await;
    set_file_mtime(&session_file, 100);

    let logs = discover_recent_in(&[root.to_path_buf()], 10_000, 100).await;
    assert!(logs.is_empty());

    set_file_mtime(&session_file, 9_950);
    let logs = discover_recent_in(&[root.to_path_buf()], 10_000, 100).await;
    assert_eq!(logs.len(), 1);
}

#[tokio::test]
async fn test_opencode_loads_older_file_backed_root_for_recent_child() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();

    let root_file = root
        .join("storage")
        .join("session")
        .join("global")
        .join("ses_root.json");
    write_json(&root_file, r#"{"id":"ses_root","directory":"/repo"}"#).await;
    set_file_mtime(&root_file, 100);

    let child_file = root
        .join("storage")
        .join("session")
        .join("global")
        .join("ses_child.json");
    write_json(
        &child_file,
        r#"{"id":"ses_child","parentID":"ses_root","directory":"/repo"}"#,
    )
    .await;
    set_file_mtime(&child_file, 9_950);

    let logs = discover_recent_in(&[root.to_path_buf()], 10_000, 100).await;
    assert_eq!(logs.len(), 1);

    match &logs[0].source {
        SessionSource::Inline { label, content } => {
            assert_eq!(label, "opencode:ses_root");
            assert!(content.contains("\"originSessionID\":\"ses_child\""));
            assert!(content.contains("\"parentSessionID\":\"ses_root\""));
        }
        SessionSource::File(_) => panic!("expected inline session"),
        SessionSource::ProviderDb { .. } => panic!("expected inline session"),
    }
}

#[tokio::test]
async fn test_opencode_prefers_newer_mtime_when_updated_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();

    let older = root
        .join("storage")
        .join("session")
        .join("global")
        .join("ses_dupe_old.json");
    write_json(&older, r#"{"id":"ses_dupe","directory":"/old"}"#).await;
    set_file_mtime(&older, 1_000);

    let newer = root
        .join("storage")
        .join("session")
        .join("workspace")
        .join("ses_dupe_new.json");
    write_json(&newer, r#"{"id":"ses_dupe","directory":"/new"}"#).await;
    set_file_mtime(&newer, 2_000);

    let logs = discover_recent_in(&[root.to_path_buf()], 3_000, 5_000).await;
    assert_eq!(logs.len(), 1);

    match &logs[0].source {
        SessionSource::Inline { content, .. } => assert!(content.contains("\"cwd\":\"/new\"")),
        SessionSource::File(_) => panic!("expected inline session"),
        SessionSource::ProviderDb { .. } => panic!("expected inline session"),
    }
}

#[tokio::test]
#[serial]
async fn test_opencode_discovers_recent_sqlite_sessions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    let db_path = root.join("opencode.db");
    let conn = Connection::open(&db_path).unwrap();

    conn.execute_batch(
        "
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                directory TEXT NOT NULL,
                title TEXT NOT NULL,
                version TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            ",
    )
    .unwrap();

    conn.execute(
            "INSERT INTO session (id, project_id, directory, title, version, time_created, time_updated, data)
             VALUES (?1, 'global', '/repo', 'SQLite session', '1.0.0', ?2, ?3, ?4)",
            rusqlite::params![
                "ses_db",
                1_772_602_456_000_i64,
                1_772_602_557_000_i64,
                r#"{"slug":"sqlite-session"}"#,
            ],
        )
        .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            "msg_db",
            "ses_db",
            1_772_602_558_000_i64,
            1_772_602_558_000_i64,
            r#"{"role":"user","time":{"created":1772602558000}}"#,
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            "prt_db",
            "msg_db",
            "ses_db",
            1_772_602_559_000_i64,
            1_772_602_559_000_i64,
            r#"{"type":"text","text":"hello"}"#,
        ],
    )
    .unwrap();

    let logs = discover_recent_in(&[root.to_path_buf()], 1_772_602_700, 9_999_999).await;
    assert_eq!(logs.len(), 1);

    match &logs[0].source {
        SessionSource::ProviderDb {
            agent,
            db_path: source_db_path,
            session_id,
            source_format,
            ..
        } => {
            assert_eq!(agent, &AgentKind::OpenCode);
            assert_eq!(source_db_path, &db_path);
            assert_eq!(session_id, "ses_db");
            assert_eq!(
                source_format,
                &Some(crate::analysis::SourceFormat::OpenCodeSqliteV1)
            );
        }
        SessionSource::Inline { .. } | SessionSource::File(_) => {
            panic!("expected provider db session")
        }
    }
    assert_eq!(logs[0].source_label(), "opencode:ses_db");

    let content = render_db_session(db_path.clone(), "ses_db".to_string())
        .await
        .expect("render db session");
    assert!(content.contains("\"source\":\"opencode\""));
    assert!(content.contains("\"sessionID\":\"ses_db\""));
    assert!(content.contains("\"messageID\":\"msg_db\""));
    assert!(content.contains("\"partID\":\"prt_db\""));
    assert!(content.contains("\"cwd\":\"/repo\""));

    assert_eq!(
        db_session_fingerprint_blocking(&db_path, "ses_db").map(|(_, rows)| rows),
        Some(3),
    );

    let located = run_with_data_dir(root, || async {
        locate_db_session_source("ses_db".to_string()).await
    })
    .await;
    match located {
        DirectSessionSource::Found(SessionSource::ProviderDb {
            db_path: located_db,
            session_id,
            source_format,
            ..
        }) => {
            assert_eq!(located_db, db_path);
            assert_eq!(session_id, "ses_db");
            assert_eq!(
                source_format,
                Some(crate::analysis::SourceFormat::OpenCodeSqliteV1)
            );
        }
        other => panic!("expected provider db source, got {other:?}"),
    }

    let missing = run_with_data_dir(root, || async {
        locate_db_session_source("ses_missing".to_string()).await
    })
    .await;
    assert!(matches!(missing, DirectSessionSource::Missing));
}

#[tokio::test]
async fn v2_provider_db_does_not_enter_v1_metadata_or_content_readers() {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("opencode.db");
    let connection = Connection::open(&db_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (id TEXT, directory TEXT, title TEXT);
             INSERT INTO session VALUES ('ses_v2', '/repo', 'V1 row must not be read');",
        )
        .unwrap();
    drop(connection);
    let log = crate::discovery::SessionLog {
        agent_type: AgentKind::OpenCode,
        source: SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path,
            session_id: "ses_v2".to_owned(),
            source_format: Some(crate::analysis::SourceFormat::OpenCodeSqliteV2),
        },
        updated_at: Some(1),
        environment: Default::default(),
    };

    assert!(crate::discovery::session_log_read(&log).await.is_none());
}

#[test]
fn pinned_v2_schema_discovers_only_admitted_session_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("opencode.db");
    let connection = Connection::open(&db_path).unwrap();
    connection
        .execute_batch(
            r#"CREATE TABLE session_v2 (
                id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL,
                title TEXT, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL
             );
              CREATE TABLE session_message (
                 id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
                 seq INTEGER NOT NULL, time_created INTEGER NOT NULL,
                 time_updated INTEGER NOT NULL, data TEXT NOT NULL
              );
              CREATE UNIQUE INDEX session_message_session_seq_idx
                  ON session_message(session_id, seq);
              INSERT INTO session_v2 VALUES
                ('ses_root', NULL, '/repo', 'Pinned V2', 1700000000000, 1700000000000),
                ('invalid id', NULL, '/repo', 'Not admitted', 1700000000000, 1700000000000);
             INSERT INTO session_message VALUES
                ('msg_root', 'ses_root', 'user', 0, 1700000001000, 1700000001000,
                 '{"text":"private transcript body"}');"#,
        )
        .unwrap();
    drop(connection);

    let logs = recent_db_session_logs(directory.path(), 1_699_999_999);
    assert_eq!(logs.len(), 1);
    assert!(matches!(
        &logs[0].source,
        SessionSource::ProviderDb {
            session_id,
            source_format: Some(crate::analysis::SourceFormat::OpenCodeSqliteV2),
            ..
        } if session_id == "ses_root"
    ));
    assert_eq!(logs[0].updated_at, Some(1_700_000_001));

    let metadata = db_session_metadata_blocking(
        &db_path,
        "ses_root",
        crate::analysis::SourceFormat::OpenCodeSqliteV2,
    )
    .unwrap();
    assert_eq!(metadata.title.as_deref(), Some("Pinned V2"));
    assert_eq!(metadata.cwd.as_deref(), Some("/repo"));
    assert!(
        db_session_metadata_blocking(
            &db_path,
            "invalid id",
            crate::analysis::SourceFormat::OpenCodeSqliteV2,
        )
        .is_none()
    );
}

#[test]
fn incomplete_v2_marker_does_not_fall_back_to_v1() {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("opencode.db");
    let connection = Connection::open(&db_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session_v2 (id TEXT PRIMARY KEY);
             CREATE TABLE session (id TEXT, directory TEXT, title TEXT, time_created INTEGER, time_updated INTEGER);
             INSERT INTO session VALUES ('ses_legacy', '/repo', 'Must not use V1', 1700000000000, 1700000000000);",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        query_recent_db_session_sources(directory.path(), 0),
        DbSourceRead::UnsupportedSchema
    ));
    assert!(db_session_source_format(&db_path, "ses_legacy").is_none());
}

#[test]
fn v2_hierarchy_errors_stay_distinct_from_a_successful_empty_result() {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("opencode.db");
    let connection = Connection::open(&db_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session_v2 (
                 id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, title TEXT,
                 time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE TABLE session_message (
                  id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER,
                  time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE UNIQUE INDEX session_message_session_seq_idx
                 ON session_message(session_id, seq);
             INSERT INTO session_v2 VALUES
                 ('ses_cycle', 'ses_cycle', '/repo', 'Cycle', 1700000000000, 1700000000000, '{}');",
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        query_recent_db_session_sources(directory.path(), 0),
        DbSourceRead::Failed("hierarchy")
    ));
}

#[test]
fn db_activity_uses_latest_valid_timestamp_in_each_table() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE message (session_id TEXT, time_created INTEGER, time_updated INTEGER);
             CREATE TABLE part (session_id TEXT, time_created INTEGER, time_updated INTEGER);",
        )
        .unwrap();
    let cases = [
        (
            Some(1_700_000_000_000_i64),
            Some(1_700_000_100_000_i64),
            Some(1_700_000_100),
        ),
        (
            Some(1_700_000_100_000),
            Some(1_700_000_000_000),
            Some(1_700_000_100),
        ),
        (
            Some(1_700_000_000),
            Some(1_700_000_100_000),
            Some(1_700_000_100),
        ),
        (
            Some(1_700_000_100_000),
            Some(1_700_000_000),
            Some(1_700_000_100),
        ),
        (None, Some(1_700_000_100), Some(1_700_000_100)),
        (Some(1_700_000_100), None, Some(1_700_000_100)),
        (Some(0), Some(1_700_000_100), Some(1_700_000_100)),
        (Some(1_700_000_100), Some(-1), Some(1_700_000_100)),
        (None, None, None),
        (Some(-1), Some(0), None),
    ];
    let mut ids = HashSet::new();
    for table in ["message", "part"] {
        for (index, (created, updated, _)) in cases.iter().enumerate() {
            let id = format!("{table}_{index}");
            ids.insert(id.clone());
            connection
                .execute(
                    &format!("INSERT INTO {table} VALUES (?1, ?2, ?3)"),
                    params![id, created, updated],
                )
                .unwrap();
        }
        connection
            .execute(
                &format!("INSERT INTO {table} VALUES ('unselected', 1900000000000, 1900000000000)"),
                [],
            )
            .unwrap();
    }
    let activity = fetch_db_activity_by_session(&connection, &ids);
    assert!(!activity.contains_key("unselected"));
    for table in ["message", "part"] {
        for (index, (_, _, expected)) in cases.iter().enumerate() {
            assert_eq!(
                activity.get(&format!("{table}_{index}")).copied(),
                *expected
            );
        }
    }
    connection
        .execute_batch(
            "INSERT INTO message VALUES ('invalid', 'not a timestamp', 1700000100000);
             INSERT INTO part VALUES ('invalid', NULL, 'not a timestamp');",
        )
        .unwrap();
    assert_eq!(
        fetch_db_activity_by_session(&connection, &HashSet::from(["invalid".to_string()])),
        HashMap::from([("invalid".to_string(), 1_700_000_100)])
    );
}

#[test]
fn db_discovery_tracks_updates_new_parts_and_child_activity() {
    let temp = tempfile::TempDir::new().unwrap();
    let connection = Connection::open(temp.path().join("opencode.db")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (
                 id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, title TEXT,
                 time_created INTEGER, time_updated INTEGER);
             CREATE TABLE message (
                 id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                 time_updated INTEGER, data TEXT);
             CREATE TABLE part (
                 id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
                 time_created INTEGER, time_updated INTEGER, data TEXT);
             INSERT INTO session VALUES
                 ('ses_root', NULL, '/repo', 'Title', 1700000000000, 1700000000000),
                 ('ses_child', 'ses_root', '/repo', 'Child', 1700000000000, 1700000000000);
             INSERT INTO message VALUES
                 ('msg_root', 'ses_root', 1700000000000, 1700000000000, '{}');
             INSERT INTO part VALUES
                 ('part_root', 'msg_root', 'ses_root', 1700000000000, 1700000000000, '{}');",
        )
        .unwrap();
    let cutoff = 1_700_000_050;
    assert!(recent_db_session_logs(temp.path(), cutoff).is_empty());
    for (sql, expected) in [
        (
            "UPDATE message SET time_updated = 1700000100000 WHERE id = 'msg_root'",
            1_700_000_100,
        ),
        (
            "UPDATE part SET time_updated = 1700000200000 WHERE id = 'part_root'",
            1_700_000_200,
        ),
        (
            "INSERT INTO part VALUES ('part_new', 'msg_root', 'ses_root', 1700000300000, 1700000300000, '{}')",
            1_700_000_300,
        ),
        (
            "INSERT INTO message VALUES ('msg_child', 'ses_child', 1700000000000, 1700000400000, '{}')",
            1_700_000_400,
        ),
        (
            "INSERT INTO part VALUES ('part_child', 'msg_child', 'ses_child', 1700000000000, 1700000500000, '{}')",
            1_700_000_500,
        ),
        (
            "UPDATE session SET time_updated = 1700000600000 WHERE id = 'ses_root'",
            1_700_000_600,
        ),
    ] {
        connection.execute(sql, []).unwrap();
        let logs = recent_db_session_logs(temp.path(), expected);
        assert_eq!(logs.len(), 1, "{sql}");
        assert_eq!(logs[0].source_label(), "opencode:ses_root", "{sql}");
        assert_eq!(logs[0].updated_at, Some(expected), "{sql}");
        assert!(recent_db_session_logs(temp.path(), expected + 1).is_empty());
    }
    connection
        .execute(
            "UPDATE session SET title = 'Renamed' WHERE id = 'ses_root'",
            [],
        )
        .unwrap();
    let logs = recent_db_session_logs(temp.path(), cutoff);
    assert_eq!(logs[0].updated_at, Some(1_700_000_600));
}

#[test]
fn db_session_fingerprint_detects_content_mutation_without_time_or_count_changes() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (
                 id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, title TEXT,
                 time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE TABLE message (
                 id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                 time_updated INTEGER, data TEXT);
             CREATE TABLE part (
                 id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
                 time_created INTEGER, time_updated INTEGER, data TEXT);
             INSERT INTO session VALUES
                 ('ses_root', NULL, '/repo', 'Title', 10, 20, '{}');
             INSERT INTO message VALUES
                 ('msg_1', 'ses_root', 30, 40, '{\"role\":\"assistant\"}');
             INSERT INTO part VALUES
                 ('part_1', 'msg_1', 'ses_root', 50, 60, '{\"text\":\"first\"}');",
        )
        .unwrap();
    let before = db_session_fingerprint_connection(&connection, "ses_root").unwrap();

    connection
        .execute(
            "UPDATE part SET data = '{\"text\":\"other\"}' WHERE id = 'part_1'",
            [],
        )
        .unwrap();
    let after = db_session_fingerprint_connection(&connection, "ses_root").unwrap();

    assert_eq!(before.1, after.1);
    assert_ne!(before.0, after.0);
    connection
        .execute_batch(
            "CREATE TABLE event_sequence (aggregate_id TEXT, seq INTEGER, owner_id TEXT);
             CREATE TABLE event (aggregate_id TEXT, seq INTEGER, id TEXT, type TEXT, data TEXT);
             INSERT INTO event_sequence VALUES ('ses_root', 1, 'owner');
             INSERT INTO event VALUES ('ses_root', 1, 'evt_1', 'part.updated.1', '{}');",
        )
        .unwrap();
    let mut before = db_session_fingerprint_connection(&connection, "ses_root").unwrap();
    for sql in [
        "UPDATE session SET title = 'Other' WHERE id = 'ses_root'",
        "UPDATE message SET data = '{\"role\":\"user\"}' WHERE id = 'msg_1'",
        "UPDATE event SET data = '{\"changed\":true}' WHERE id = 'evt_1'",
        "UPDATE event_sequence SET owner_id = 'other' WHERE aggregate_id = 'ses_root'",
    ] {
        connection.execute(sql, []).unwrap();
        let after = db_session_fingerprint_connection(&connection, "ses_root").unwrap();
        assert_eq!(before.1, after.1, "{sql}");
        assert_ne!(before.0, after.0, "{sql}");
        before = after;
    }
}

#[test]
fn v2_db_fingerprint_detects_older_message_edits_and_fork_changes() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            r#"CREATE TABLE session_v2 (
                 id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, title TEXT,
                 time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE TABLE session_message (
                 id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER,
                 time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE UNIQUE INDEX session_message_session_seq_idx
                 ON session_message(session_id, seq);
             INSERT INTO session_v2 VALUES
                 ('ses_root', NULL, '/repo', 'Root', 10, 20, '{}'),
                 ('ses_child', 'ses_root', '/repo', 'Child', 30, 40, '{}');
             INSERT INTO session_message VALUES
                 ('msg_old', 'ses_root', 'user', 1, 10, 20, '{"text":"first"}'),
                 ('msg_child', 'ses_child', 'assistant', 1, 30, 40, '{"text":"child"}');"#,
        )
        .unwrap();
    let before = db_session_fingerprint_connection(&connection, "ses_root").unwrap();
    assert_eq!(before.1, 4);

    connection
        .execute(
            "UPDATE session_message SET data = '{\"text\":\"edited\"}' WHERE id = 'msg_old'",
            [],
        )
        .unwrap();
    let after_edit = db_session_fingerprint_connection(&connection, "ses_root").unwrap();
    assert_eq!(after_edit.1, before.1);
    assert_ne!(after_edit.0, before.0);

    connection
        .execute(
            "UPDATE session_v2 SET parent_id = NULL WHERE id = 'ses_child'",
            [],
        )
        .unwrap();
    let after_fork = db_session_fingerprint_connection(&connection, "ses_root").unwrap();
    assert_eq!(after_fork.1, 2);
    assert_ne!(after_fork.0, after_edit.0);
}

#[test]
fn db_session_fingerprint_detects_appended_records() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (
                 id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, title TEXT,
                 time_created INTEGER, time_updated INTEGER, data TEXT);
             CREATE TABLE message (
                 id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                 time_updated INTEGER, data TEXT);
             CREATE TABLE part (
                 id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
                 time_created INTEGER, time_updated INTEGER, data TEXT);
             INSERT INTO session VALUES
                 ('ses_root', NULL, '/repo', 'Title', 10, 20, '{}');",
        )
        .unwrap();

    let before = db_session_fingerprint_connection(&connection, "ses_root").unwrap();
    connection
        .execute(
            "INSERT INTO message VALUES
                 ('msg_1', 'ses_root', 30, 40, '{\"role\":\"assistant\"}')",
            [],
        )
        .unwrap();
    let after = db_session_fingerprint_connection(&connection, "ses_root").unwrap();

    assert_eq!(before.1 + 1, after.1);
    assert_ne!(before.0, after.0);
}

#[tokio::test]
async fn test_opencode_discovers_sqlite_sessions_without_session_data_column() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    let db_path = root.join("opencode.db");
    let conn = Connection::open(&db_path).unwrap();

    conn.execute_batch(
        "
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                directory TEXT NOT NULL,
                title TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            ",
    )
    .unwrap();

    conn.execute(
        "INSERT INTO session (id, project_id, directory, title, time_created, time_updated)
             VALUES (?1, 'workspace', '/Users/avery/dev/demo-cli', 'DB-only session', ?2, ?3)",
        rusqlite::params![
            "ses_db_nodata",
            1_774_323_440_000_i64,
            1_774_324_993_000_i64
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            "msg_db_nodata",
            "ses_db_nodata",
            1_774_324_993_000_i64,
            1_774_324_993_000_i64,
            r#"{"role":"user"}"#,
        ],
    )
    .unwrap();

    let logs = discover_recent_in(&[root.to_path_buf()], 1_774_324_994, 10_000).await;
    assert_eq!(logs.len(), 1);

    match &logs[0].source {
        SessionSource::ProviderDb { session_id, .. } => {
            assert_eq!(session_id, "ses_db_nodata");
        }
        SessionSource::Inline { .. } | SessionSource::File(_) => {
            panic!("expected provider db session")
        }
    }
    let content = render_db_session(db_path.clone(), "ses_db_nodata".to_string())
        .await
        .expect("render db session");
    assert!(content.contains("\"sessionID\":\"ses_db_nodata\""));
    assert!(content.contains("\"cwd\":\"/Users/avery/dev/demo-cli\""));
    assert!(content.contains("\"directory\":\"/Users/avery/dev/demo-cli\""));
}

#[tokio::test]
async fn test_opencode_prefers_sqlite_over_storage_walk_when_available() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();

    write_json(
            &root
                .join("storage")
                .join("session")
                .join("global")
                .join("ses_mixed.json"),
            r#"{"id":"ses_mixed","directory":"/repo","time":{"created":1772602456000,"updated":1772602457000}}"#,
        )
        .await;
    write_json(
            &root
                .join("storage")
                .join("message")
                .join("ses_mixed")
                .join("msg_same.json"),
            r#"{"id":"msg_same","sessionID":"ses_mixed","role":"user","time":{"created":1772602458000}}"#,
        )
        .await;
    write_json(
            &root
                .join("storage")
                .join("part")
                .join("msg_same")
                .join("prt_same.json"),
            r#"{"id":"prt_same","sessionID":"ses_mixed","messageID":"msg_same","type":"text","text":"hello from file","time":{"created":1772602459000}}"#,
        )
        .await;

    let db_path = root.join("opencode.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                directory TEXT NOT NULL,
                title TEXT NOT NULL,
                version TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            ",
    )
    .unwrap();
    conn.execute(
            "INSERT INTO session (id, project_id, directory, title, version, time_created, time_updated, data)
             VALUES (?1, 'global', '/repo', 'Mixed session', '1.0.0', ?2, ?3, ?4)",
            rusqlite::params![
                "ses_mixed",
                1_772_602_456_000_i64,
                1_772_602_557_000_i64,
                r#"{"slug":"mixed-session"}"#,
            ],
        )
        .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            "msg_same",
            "ses_mixed",
            1_772_602_558_000_i64,
            1_772_602_558_000_i64,
            r#"{"role":"user","time":{"created":1772602558000}}"#,
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            "prt_same",
            "msg_same",
            "ses_mixed",
            1_772_602_559_000_i64,
            1_772_602_559_000_i64,
            r#"{"type":"text","text":"hello from db"}"#,
        ],
    )
    .unwrap();

    let logs = discover_recent_in(&[root.to_path_buf()], 1_772_602_700, 9_999_999).await;
    assert_eq!(logs.len(), 1);

    match &logs[0].source {
        SessionSource::ProviderDb { session_id, .. } => assert_eq!(session_id, "ses_mixed"),
        SessionSource::Inline { .. } | SessionSource::File(_) => {
            panic!("expected provider db session")
        }
    }
    let content = render_db_session(db_path.clone(), "ses_mixed".to_string())
        .await
        .expect("render db session");
    assert!(content.contains("hello from db"));
    assert!(!content.contains("hello from file"));
}

#[tokio::test]
async fn production_db_fixture_detects_null_parent_id_fork_and_rejects_title_only_match() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/opencode-cli-production-db.json")).unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("opencode.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (
            id TEXT PRIMARY KEY, project_id TEXT NOT NULL, parent_id TEXT,
            directory TEXT NOT NULL, title TEXT NOT NULL, version TEXT NOT NULL,
            time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
         );
         CREATE TABLE message (
            id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL, data TEXT NOT NULL
         );
         CREATE TABLE part (
            id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL,
            time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL
         );",
    )
    .unwrap();
    for row in fixture["tables"]["session"].as_array().unwrap() {
        conn.execute(
            "INSERT INTO session VALUES (?1, 'fixture-project', ?2, ?3, ?4, '1.1.25', ?5, ?6, '{}')",
            params![
                row["id"].as_str().unwrap(),
                row["parent_id"].as_str(),
                row["directory"].as_str().unwrap(),
                row["title"].as_str().unwrap(),
                row["time_created"].as_i64().unwrap(),
                row["time_updated"].as_i64().unwrap(),
            ],
        )
        .unwrap();
    }
    for row in fixture["tables"]["message"].as_array().unwrap() {
        conn.execute(
            "INSERT INTO message VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                row["id"].as_str().unwrap(),
                row["session_id"].as_str().unwrap(),
                row["time_created"].as_i64().unwrap(),
                row["time_updated"].as_i64().unwrap(),
                row["data"].to_string(),
            ],
        )
        .unwrap();
    }
    for row in fixture["tables"]["part"].as_array().unwrap() {
        conn.execute(
            "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                row["id"].as_str().unwrap(),
                row["message_id"].as_str().unwrap(),
                row["session_id"].as_str().unwrap(),
                row["time_created"].as_i64().unwrap(),
                row["time_updated"].as_i64().unwrap(),
                row["data"].to_string(),
            ],
        )
        .unwrap();
    }
    drop(conn);

    let logs = discover_recent_in(&[tmp.path().to_path_buf()], 1_785_480_000, 86_400).await;
    assert_eq!(logs.len(), 3, "parent, child, and negative stay distinct");

    assert_eq!(
        db_fork_parent(db_path.clone(), "ses_child".to_string()).await,
        Some("ses_parent".to_string())
    );
    assert_eq!(
        db_fork_parent(db_path.clone(), "ses_similar".to_string()).await,
        None,
        "a generated fork title without an exact copied prefix must fail closed"
    );
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
        params![
            "prt_child_malformed",
            "msg_child_assistant_1",
            "ses_child",
            1_785_479_000_000_i64,
            "{malformed",
        ],
    )
    .unwrap();
    drop(conn);
    assert_eq!(
        db_fork_parent(db_path.clone(), "ses_child".to_string()).await,
        None,
        "malformed comparison input must fail closed"
    );

    let child = render_db_session(db_path.clone(), "ses_child".to_string())
        .await
        .expect("child transcript");
    assert_eq!(
        embedded_fork_observation(&child)
            .expect("fork observation")
            .parent_agent_session_id,
        "ses_parent"
    );

    let similar = render_db_session(db_path, "ses_similar".to_string())
        .await
        .expect("similar transcript");
    assert!(
        embedded_fork_observation(&similar).is_none(),
        "a generated fork title without an exact copied prefix must fail closed"
    );
}

#[tokio::test]
async fn db_fork_parent_rejects_candidate_overflow_as_ambiguous() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("opencode.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (
             id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, title TEXT,
             time_created INTEGER
         );
         INSERT INTO session VALUES (
             'ses_child', NULL, '/repo', 'Parent statement (fork #1)', 200
         );",
    )
    .unwrap();
    for index in 0..=MAX_DB_FORK_CANDIDATES {
        conn.execute(
            "INSERT INTO session VALUES (?1, NULL, '/repo', 'Parent statement', 100)",
            params![format!("ses_parent_{index}")],
        )
        .unwrap();
    }
    drop(conn);

    assert_eq!(db_fork_parent(db_path, "ses_child".to_string()).await, None);
}

#[test]
fn test_opencode_renders_one_log_for_root_and_child_cluster() {
    let mut sessions = HashMap::new();
    sessions.insert(
        "ses_root".to_string(),
        session_record(Some("/repo"), None, Some(100), Some(150)),
    );
    sessions.insert(
        "ses_child".to_string(),
        session_record(Some("/repo"), Some("ses_root"), Some(120), Some(180)),
    );

    let mut messages_by_session = BTreeMap::new();
    messages_by_session.insert(
        "ses_child".to_string(),
        vec![message_record("ses_child", "msg_child", 170)],
    );

    let logs = render_session_logs(0, sessions, messages_by_session, BTreeMap::new());
    assert_eq!(logs.len(), 1);
    match &logs[0].source {
        SessionSource::Inline { label, content } => {
            assert_eq!(label, "opencode:ses_root");
            assert!(content.contains("\"clusterSessionCount\":2"));
            assert!(content.contains("\"childSessionIDs\":[\"ses_child\"]"));
            assert!(content.contains("\"type\":\"session_member\""));
            assert!(content.contains("\"originSessionID\":\"ses_child\""));
            assert!(content.contains("\"parentSessionID\":\"ses_root\""));
            let session_meta_idx = content.find("\"type\":\"session_meta\"").unwrap();
            let session_member_idx = content.find("\"type\":\"session_member\"").unwrap();
            let child_message_idx = content.find("\"messageID\":\"msg_child\"").unwrap();
            assert!(session_meta_idx < session_member_idx);
            assert!(session_member_idx < child_message_idx);
        }
        SessionSource::File(_) => panic!("expected inline session"),
        SessionSource::ProviderDb { .. } => panic!("expected inline session"),
    }
}

#[test]
fn test_opencode_renders_multiple_children_under_one_root() {
    let mut sessions = HashMap::new();
    sessions.insert(
        "ses_root".to_string(),
        session_record(Some("/repo"), None, Some(100), Some(150)),
    );
    sessions.insert(
        "ses_child_a".to_string(),
        session_record(Some("/repo"), Some("ses_root"), Some(120), Some(170)),
    );
    sessions.insert(
        "ses_child_b".to_string(),
        session_record(Some("/repo"), Some("ses_root"), Some(125), Some(190)),
    );

    let logs = render_session_logs(0, sessions, BTreeMap::new(), BTreeMap::new());
    assert_eq!(logs.len(), 1);
    let content = inline_content(&logs[0]);
    assert!(content.contains("\"childSessionIDs\":[\"ses_child_a\",\"ses_child_b\"]"));
}

#[test]
fn test_opencode_leaves_unrelated_sessions_separate() {
    let mut sessions = HashMap::new();
    sessions.insert(
        "ses_one".to_string(),
        session_record(Some("/repo-one"), None, Some(100), Some(150)),
    );
    sessions.insert(
        "ses_two".to_string(),
        session_record(Some("/repo-two"), None, Some(200), Some(250)),
    );

    let logs = render_session_logs(0, sessions, BTreeMap::new(), BTreeMap::new());
    assert_eq!(logs.len(), 2);
}

#[test]
fn test_opencode_cluster_recency_tracks_newer_child_activity() {
    let mut sessions = HashMap::new();
    sessions.insert(
        "ses_root".to_string(),
        session_record(Some("/repo"), None, Some(100), Some(110)),
    );
    sessions.insert(
        "ses_child".to_string(),
        session_record(Some("/repo"), Some("ses_root"), Some(120), Some(130)),
    );

    let mut messages_by_session = BTreeMap::new();
    messages_by_session.insert(
        "ses_child".to_string(),
        vec![message_record("ses_child", "msg_child", 400)],
    );

    let logs = render_session_logs(0, sessions, messages_by_session, BTreeMap::new());
    assert_eq!(logs[0].updated_at, Some(400));
}

#[test]
fn test_opencode_root_cwd_falls_back_to_child_directory() {
    let mut sessions = HashMap::new();
    sessions.insert(
        "ses_root".to_string(),
        session_record(None, None, Some(100), Some(150)),
    );
    sessions.insert(
        "ses_child".to_string(),
        session_record(
            Some("/repo-from-child"),
            Some("ses_root"),
            Some(120),
            Some(180),
        ),
    );

    let logs = render_session_logs(0, sessions, BTreeMap::new(), BTreeMap::new());
    let content = inline_content(&logs[0]);
    let metadata = crate::discovery::scanner::parse_session_metadata_str(content);
    assert_eq!(metadata.session_id, Some("ses_root".to_string()));
    assert_eq!(metadata.cwd, Some("/repo-from-child".to_string()));
}

#[tokio::test]
async fn test_opencode_discovers_child_activity_under_root_upload_identity() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    let db_path = root.join("opencode.db");
    let conn = Connection::open(&db_path).unwrap();

    conn.execute_batch(
        "
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                parent_id TEXT,
                directory TEXT NOT NULL,
                title TEXT NOT NULL,
                version TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            ",
    )
    .unwrap();

    conn.execute(
            "INSERT INTO session (id, project_id, parent_id, directory, title, version, time_created, time_updated, data)
             VALUES (?1, 'workspace', NULL, '/repo', 'root', '1.0.0', ?2, ?3, ?4)",
            rusqlite::params![
                "ses_root",
                1_700_000_000_000_i64,
                1_700_000_001_000_i64,
                r#"{"slug":"root"}"#,
            ],
        )
        .unwrap();
    conn.execute(
            "INSERT INTO session (id, project_id, parent_id, directory, title, version, time_created, time_updated, data)
             VALUES (?1, 'workspace', ?2, '/repo', 'child', '1.0.0', ?3, ?4, ?5)",
            rusqlite::params![
                "ses_child",
                "ses_root",
                1_700_000_100_000_i64,
                1_700_000_200_000_i64,
                r#"{"slug":"child"}"#,
            ],
        )
        .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            "msg_child",
            "ses_child",
            1_700_000_300_000_i64,
            1_700_000_300_000_i64,
            r#"{"role":"assistant","time":{"created":1700000300000}}"#,
        ],
    )
    .unwrap();

    let logs = discover_recent_in(&[root.to_path_buf()], 1_700_000_400, 500).await;
    assert_eq!(logs.len(), 1);
    match &logs[0].source {
        SessionSource::ProviderDb { session_id, .. } => assert_eq!(session_id, "ses_root"),
        SessionSource::Inline { .. } | SessionSource::File(_) => {
            panic!("expected provider db session")
        }
    }
    let content = render_db_session(db_path.clone(), "ses_root".to_string())
        .await
        .expect("render db session");
    assert!(content.contains("\"originSessionID\":\"ses_child\""));
    assert!(content.contains("\"parentSessionID\":\"ses_root\""));
    assert_eq!(logs[0].updated_at, Some(1_700_000_300));
}

#[test]
#[serial]
fn test_data_roots_include_xdg_location() {
    let home = Path::new("/Users/tester");

    let previous = std::env::var_os("XDG_DATA_HOME");
    if previous.is_some() {
        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
    }

    let roots = data_roots_for_home(home);

    if let Some(value) = previous {
        unsafe {
            std::env::set_var("XDG_DATA_HOME", value);
        }
    }

    assert!(roots.contains(&home.join(".local").join("share").join("opencode")));
    if cfg!(target_os = "macos") {
        assert!(
            roots.contains(
                &home
                    .join("Library")
                    .join("Application Support")
                    .join("opencode")
            )
        );
    }
}

#[test]
#[serial]
fn test_data_roots_honor_xdg_data_home() {
    let previous = std::env::var_os("XDG_DATA_HOME");
    unsafe {
        std::env::set_var("XDG_DATA_HOME", "/tmp/opencode-xdg");
    }

    let roots = data_roots_for_home(Path::new("/Users/tester"));

    match previous {
        Some(value) => unsafe {
            std::env::set_var("XDG_DATA_HOME", value);
        },
        None => unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        },
    }

    assert!(roots.contains(&PathBuf::from("/tmp/opencode-xdg").join("opencode")));
}

#[test]
fn data_roots_for_platform_covers_windows_and_linux_paths() {
    let windows_home = Path::new("C:\\Users\\tester");
    let windows_roots = data_roots_for_platform(
        windows_home,
        DesktopPlatform::Windows,
        Some(Path::new("D:\\Roaming")),
        None,
    );
    assert!(windows_roots.contains(&windows_home.join(".local").join("share").join("opencode")));
    assert!(windows_roots.contains(&PathBuf::from("D:\\Roaming").join("opencode")));

    let fallback_roots =
        data_roots_for_platform(windows_home, DesktopPlatform::Windows, None, None);
    assert!(
        fallback_roots.contains(
            &windows_home
                .join("AppData")
                .join("Roaming")
                .join("opencode")
        )
    );

    let linux_home = Path::new("/home/tester");
    let linux_roots = data_roots_for_platform(
        linux_home,
        DesktopPlatform::Linux,
        None,
        Some(Path::new("/tmp/xdg-data")),
    );
    assert_eq!(
        linux_roots,
        vec![PathBuf::from("/tmp/xdg-data").join("opencode")]
    );
}

#[test]
fn configured_opencode_db_paths_follow_the_pinned_resolution_rules() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    assert_eq!(
        database_path_for_root_config(root, None),
        Some(root.join("opencode.db"))
    );
    assert_eq!(
        database_path_for_root_config(root, Some(Path::new("custom/opencode.db"))),
        Some(root.join("custom/opencode.db"))
    );
    assert_eq!(
        database_path_for_root_config(root, Some(&directory.path().join("data/opencode.db"))),
        Some(directory.path().join("data/opencode.db"))
    );
    assert_eq!(
        database_path_for_root_config(root, Some(Path::new(":memory:"))),
        None
    );
    assert_eq!(
        database_path_for_root_config(root, Some(Path::new(""))),
        Some(root.to_path_buf())
    );
    assert_eq!(
        database_path_for_root_config(root, Some(Path::new("missing/../custom.db"))),
        Some(root.join("custom.db"))
    );
}

fn pinned_mixed_db(path: &Path) -> Connection {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(include_str!("fixtures/opencode_pinned_mixed.sql"))
        .unwrap();
    connection.execute_batch(
        "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
         VALUES ('ses_root', 'global', 'root', '/old', 'V1 copy', '1.0', 1700000000000, 1700000009000),
                ('ses_v1_only', 'global', 'legacy', '/legacy', 'Not migrated', '1.0', 1700000000000, 1700000000000),
                ('ses_after_migration', 'global', 'new', '/legacy/new', 'New V1', '1.0', 1700000008000, 1700000008000);
         INSERT INTO session_v2 (id, project_id, parent_id, fork_session_id, slug, directory, title, version, time_created, time_updated, time_archived)
         VALUES ('ses_root', 'global', NULL, NULL, 'root', '/repo/Exact CWD/..', 'V2 root', '2.0.32', 1700000000000, 1700000000000, NULL),
                ('ses_child', 'global', 'ses_root', NULL, 'child', '/child', 'Child', '2.0.32', 1700000001000, 1700000001000, NULL),
                ('ses_fork', 'global', NULL, 'ses_root', 'fork', '/fork', 'Fork', '2.0.32', 1700000002000, 1700000002000, NULL),
                ('ses_archived', 'global', NULL, NULL, 'archived', '/archive', 'Archived', '2.0.32', 1700000003000, 1700000003000, 1700000004000);
         INSERT INTO session_message VALUES
             ('msg_old', 'ses_root', 'user', 0, 1700000000000, 1700000000000, 'not JSON'),
             ('msg_child', 'ses_child', 'assistant', 0, 1700000005000, 1700000005000, 'not JSON');
         INSERT INTO message VALUES ('msg_legacy', 'ses_root', 1700000009000, 1700000009000, 'not JSON');
         INSERT INTO part VALUES ('prt_legacy', 'msg_legacy', 'ses_root', 1700000009000, 1700000009000, 'not JSON');"
    ).unwrap();
    connection
}

#[test]
fn pinned_mixed_schema_keeps_v1_only_sessions_without_reading_transcripts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    assert_eq!(
        opencode_sqlite_schema(&connection).unwrap(),
        OpenCodeSqliteSchema::Mixed
    );
    let logs = recent_db_session_logs(directory.path(), 1_699_999_999);
    let selected: BTreeMap<_, _> = logs
        .iter()
        .map(|log| match &log.source {
            SessionSource::ProviderDb {
                session_id,
                source_format,
                ..
            } => (session_id.as_str(), source_format.unwrap()),
            _ => panic!("expected metadata-only database source"),
        })
        .collect();
    use crate::analysis::SourceFormat::{OpenCodeSqliteV1, OpenCodeSqliteV2};
    assert_eq!(
        selected,
        BTreeMap::from([
            ("ses_root", OpenCodeSqliteV2),
            ("ses_fork", OpenCodeSqliteV2),
            ("ses_archived", OpenCodeSqliteV2),
            ("ses_v1_only", OpenCodeSqliteV1),
            ("ses_after_migration", OpenCodeSqliteV1),
        ])
    );
    let root = logs
        .iter()
        .find(|log| log.source_label().contains("ses_root"))
        .unwrap();
    assert_eq!(root.updated_at, Some(1_700_000_005));
    assert_eq!(
        db_session_source_format(&path, "ses_root"),
        Some(OpenCodeSqliteV2)
    );
    assert_eq!(
        db_session_source_format(&path, "ses_after_migration"),
        Some(OpenCodeSqliteV1)
    );
    assert!(db_session_metadata_blocking(&path, "ses_root", OpenCodeSqliteV1).is_none());
    assert!(db_session_metadata_blocking(&path, "ses_after_migration", OpenCodeSqliteV1).is_some());
    assert_eq!(
        query_session_title_by_id(&path, "ses_root"),
        Some((Some("V2 root".into()), true))
    );
    let metadata = db_session_metadata_blocking(&path, "ses_root", OpenCodeSqliteV2).unwrap();
    assert_eq!(metadata.cwd.as_deref(), Some("/repo/Exact CWD/.."));
    connection
        .execute(
            "UPDATE session_v2 SET directory = '/moved/Exact CWD' WHERE id = 'ses_root'",
            [],
        )
        .unwrap();
    assert_eq!(
        db_session_metadata_blocking(&path, "ses_root", OpenCodeSqliteV2)
            .unwrap()
            .cwd
            .as_deref(),
        Some("/moved/Exact CWD")
    );
    let root_fingerprint = db_session_fingerprint_blocking(&path, "ses_root").unwrap();
    connection.execute(
        "INSERT INTO session (id, project_id, parent_id, slug, directory, title, version, time_created, time_updated)
         VALUES ('ses_new_v1_child', 'global', 'ses_root', 'child', '/legacy/child', 'New V1 child', '1.0', 1700000008000, 1700000008000)", []
    ).unwrap();
    let logs = recent_db_session_logs(directory.path(), 0);
    assert!(logs.iter().any(|log| matches!(&log.source, SessionSource::ProviderDb { session_id, source_format: Some(OpenCodeSqliteV1), .. } if session_id == "ses_new_v1_child")));
    assert_eq!(
        db_session_fingerprint_blocking(&path, "ses_root").unwrap(),
        root_fingerprint
    );
}

#[test]
fn schema_characterization_rejects_unproven_and_malformed_families() {
    for (sql, expected) in [
        (
            "CREATE TABLE unrelated(id TEXT)",
            OpenCodeSqliteSchema::Unsupported,
        ),
        (
            "CREATE TABLE session(other TEXT)",
            OpenCodeSqliteSchema::Malformed,
        ),
        (
            "CREATE TABLE session(id TEXT PRIMARY KEY, directory TEXT, title TEXT, time_created INTEGER, time_updated INTEGER)",
            OpenCodeSqliteSchema::V1,
        ),
        (
            "CREATE TABLE session(id TEXT PRIMARY KEY, directory TEXT, title TEXT, time_created INTEGER, time_updated INTEGER); CREATE TABLE session_message(id TEXT PRIMARY KEY)",
            OpenCodeSqliteSchema::Malformed,
        ),
    ] {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(sql).unwrap();
        assert_eq!(
            opencode_sqlite_schema(&connection).unwrap(),
            expected,
            "{sql}"
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    std::fs::write(path, b"not a SQLite database").unwrap();
    assert!(matches!(
        query_recent_db_session_sources(directory.path(), 0),
        DbSourceRead::Failed("schema")
    ));
}

#[test]
fn native_channel_paths_include_only_sanitized_names_with_bounds() {
    let directory = tempfile::tempdir().unwrap();
    for name in [
        "opencode-local.db",
        "opencode-detached.db",
        "opencode-feature-check.db",
        "opencode.db-wal",
        "opencode-bad channel.db",
        "unrelated.db",
    ] {
        std::fs::write(directory.path().join(name), []).unwrap();
    }
    let paths = database_paths_for_root_config(directory.path(), None).unwrap();
    assert_eq!(
        paths,
        [
            "opencode.db",
            "opencode-detached.db",
            "opencode-feature-check.db",
            "opencode-local.db"
        ]
        .map(|name| directory.path().join(name))
    );
    assert_eq!(
        database_paths_for_root_config(directory.path(), Some(Path::new("custom.db"))).unwrap(),
        vec![directory.path().join("custom.db")]
    );
    assert!(
        database_paths_for_root_config(directory.path(), Some(Path::new(":memory:")))
            .unwrap()
            .is_empty()
    );
    for index in 0..MAX_CHANNEL_DATABASES {
        std::fs::write(directory.path().join(format!("opencode-{index}.db")), []).unwrap();
    }
    assert_eq!(
        database_paths_for_root_config(directory.path(), None),
        Err("channel_limit")
    );
    let crowded = tempfile::tempdir().unwrap();
    for index in 0..=MAX_DB_DIRECTORY_ENTRIES {
        std::fs::write(crowded.path().join(format!("other-{index}")), []).unwrap();
    }
    assert_eq!(
        database_paths_for_root_config(crowded.path(), None),
        Err("directory_limit")
    );
}

#[tokio::test]
async fn native_missing_empty_busy_and_invalid_stores_keep_distinct_outcomes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    assert!(matches!(
        query_recent_db_session_sources(directory.path(), 0),
        DbSourceRead::NotPresent
    ));
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!("fixtures/opencode_pinned_mixed.sql"))
        .unwrap();
    assert!(
        matches!(query_recent_db_session_sources(directory.path(), 0), DbSourceRead::Ready(logs) if logs.is_empty())
    );
    write_json(
        &directory
            .path()
            .join("storage/session/global/ses_stale.json"),
        r#"{"id":"ses_stale","time":{"updated":1700000000000}}"#,
    )
    .await;
    connection.execute_batch("BEGIN EXCLUSIVE").unwrap();
    // Lock waits can exhaust the query deadline before schema discovery fails.
    assert!(matches!(
        query_recent_db_session_sources(directory.path(), 0),
        DbSourceRead::Failed("schema" | "query_limit")
    ));
    assert!(
        discover_recent_in(&[directory.path().to_path_buf()], 1_700_000_010, 60)
            .await
            .is_empty()
    );
    connection.execute_batch("ROLLBACK").unwrap();
    drop(connection);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(matches!(
        query_recent_db_session_sources(directory.path(), 0),
        DbSourceRead::Failed("open")
    ));
}

#[test]
fn pinned_v2_only_schema_remains_supported_without_retained_v1_tables() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    connection
        .execute_batch("DROP TABLE part; DROP TABLE message; DROP TABLE session")
        .unwrap();
    assert_eq!(
        opencode_sqlite_schema(&connection).unwrap(),
        OpenCodeSqliteSchema::V2
    );
    let logs = recent_db_session_logs(directory.path(), 0);
    assert_eq!(logs.len(), 3);
    assert!(logs.iter().all(|log| matches!(
        &log.source,
        SessionSource::ProviderDb {
            source_format: Some(crate::analysis::SourceFormat::OpenCodeSqliteV2),
            ..
        }
    )));
}

#[tokio::test]
#[serial]
async fn channel_duplicates_and_explicit_db_override_select_one_native_source() {
    let directory = tempfile::tempdir().unwrap();
    seed_session_title_db(
        &directory.path().join("opencode.db"),
        &[("ses_root", "V1 copy")],
    );
    let preferred_path = directory.path().join("opencode-local.db");
    drop(pinned_mixed_db(&preferred_path));
    drop(pinned_mixed_db(&directory.path().join("opencode-z.db")));
    let previous = std::env::var_os("OPENCODE_DB");
    unsafe {
        std::env::remove_var("OPENCODE_DB");
    }
    for _ in 0..2 {
        let logs = recent_db_session_logs(directory.path(), 0);
        let roots: Vec<_> = logs
            .iter()
            .filter(|log| log.source_label().contains("ses_root"))
            .collect();
        assert_eq!(roots.len(), 1);
        assert!(
            matches!(&roots[0].source, SessionSource::ProviderDb { db_path, source_format: Some(crate::analysis::SourceFormat::OpenCodeSqliteV2), .. } if db_path == &preferred_path)
        );
        run_with_data_dir(directory.path(), || async {
            assert!(matches!(locate_db_session_source("ses_root".into()).await, DirectSessionSource::Found(SessionSource::ProviderDb { db_path, .. }) if db_path == preferred_path));
            assert_eq!(OpenCodeExplorer.indexed_session_title("ses_root").await.unwrap().text, "V2 root");
        }).await;
    }
    let older_root = tempfile::tempdir().unwrap();
    seed_session_title_db(
        &older_root.path().join("opencode.db"),
        &[("ses_root", "V1 copy in another root")],
    );
    let roots = vec![
        older_root.path().to_path_buf(),
        directory.path().to_path_buf(),
    ];
    let logs = discover_recent_in(&roots, 1_700_000_010, i64::MAX).await;
    assert_eq!(
        logs.iter()
            .filter(|log| log.source_label().contains("ses_root"))
            .count(),
        1
    );
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let discover = discover_recent_batches_in(
        &roots,
        AgentKind::OpenCode,
        1_700_000_010,
        i64::MAX,
        1,
        sender,
    );
    let collect = async {
        let mut logs = Vec::new();
        while let Some(batch) = receiver.recv().await {
            logs.extend(batch.candidates);
        }
        logs
    };
    let (_, logs) = tokio::join!(discover, collect);
    assert_eq!(
        logs.iter()
            .filter(|log| log.source_label().contains("ses_root"))
            .count(),
        1
    );
    Connection::open(&preferred_path)
        .unwrap()
        .execute(
            "UPDATE session_v2 SET title = NULL WHERE id = 'ses_root'",
            [],
        )
        .unwrap();
    run_with_data_dir(directory.path(), || async {
        assert!(
            OpenCodeExplorer
                .indexed_session_title("ses_root")
                .await
                .is_none()
        );
        assert!(
            !OpenCodeExplorer
                .indexed_session_titles(&["ses_root".into()])
                .await
                .contains_key("ses_root")
        );
    })
    .await;
    unsafe {
        std::env::set_var("OPENCODE_DB", "opencode-z.db");
    }
    let logs = recent_db_session_logs(directory.path(), 0);
    assert!(logs.iter().all(|log| matches!(&log.source, SessionSource::ProviderDb { db_path, .. } if db_path == &directory.path().join("opencode-z.db"))));
    let external = tempfile::tempdir().unwrap();
    let custom_path = external.path().join("configured.sqlite");
    drop(pinned_mixed_db(&custom_path));
    unsafe {
        std::env::set_var("OPENCODE_DB", &custom_path);
    }
    run_with_data_dir(directory.path(), || async {
        assert_eq!(data_roots(), vec![directory.path().to_path_buf()]);
        let logs = OpenCodeExplorer.discover_recent(1_700_000_010, 60).await;
        assert_eq!(logs.len(), 5);
        assert!(logs.iter().all(|log| matches!(&log.source, SessionSource::ProviderDb { db_path, .. } if db_path == &custom_path)));
        let watches = OpenCodeExplorer.watch_roots(&home_dir().unwrap());
        assert!(watches.iter().any(|watch| watch.path == external.path()));
        assert!(watches.iter().all(|watch| !watch.recursive));
    }).await;
    match previous {
        Some(value) => unsafe {
            std::env::set_var("OPENCODE_DB", value);
        },
        None => unsafe {
            std::env::remove_var("OPENCODE_DB");
        },
    }
}

#[test]
fn primary_native_root_stays_first_with_empty_or_custom_xdg() {
    let home = Path::new("/home/tester");
    for platform in [
        DesktopPlatform::Macos,
        DesktopPlatform::Linux,
        DesktopPlatform::Windows,
    ] {
        assert_eq!(
            data_roots_for_platform(home, platform, None, Some(Path::new("")))[0],
            home.join(".local/share/opencode")
        );
        assert_eq!(
            data_roots_for_platform(home, platform, None, Some(Path::new("/z/custom")))[0],
            PathBuf::from("/z/custom/opencode")
        );
    }
}

#[test]
fn v2_wal_fingerprint_covers_older_edits_reverts_deletions_and_archive_state() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    connection
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    for seq in 1..=10 {
        connection.execute("INSERT INTO session_message VALUES (?1, 'ses_root', 'user', ?2, 1700000000000, 1700000000000, '{}')", params![format!("msg_{seq}"), seq]).unwrap();
    }
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    let main_file = std::fs::read(&path).unwrap();
    let before = db_session_fingerprint_blocking(&path, "ses_root").unwrap();
    let revision = db_session_admission_revision_connection(&connection, "ses_root").unwrap();
    assert!(revision.starts_with("scope-v2:"));
    connection
        .execute(
            "UPDATE session_message SET data = 'changed older body' WHERE id = 'msg_old'",
            [],
        )
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), main_file);
    assert_eq!(
        db_session_admission_revision_connection(&connection, "ses_root").unwrap(),
        revision
    );
    let edited = db_session_fingerprint_blocking(&path, "ses_root").unwrap();
    assert_eq!(edited.1, before.1);
    assert_ne!(edited.0, before.0);
    connection
        .execute(
            "UPDATE session_message SET data = 'not JSON' WHERE id = 'msg_old'",
            [],
        )
        .unwrap();
    assert_eq!(
        db_session_fingerprint_blocking(&path, "ses_root").unwrap(),
        before
    );
    let mut previous = before;
    for sql in [
        "UPDATE session_v2 SET revert = '{\"messageID\":\"msg_old\"}' WHERE id = 'ses_root'",
        "UPDATE session_v2 SET time_archived = 1700000010000 WHERE id = 'ses_root'",
        "UPDATE session_v2 SET fork_boundary = '{\"messageID\":\"msg_old\"}' WHERE id = 'ses_child'",
        "UPDATE session_message SET data = 'changed child body' WHERE id = 'msg_child'",
        "DELETE FROM session_message WHERE id = 'msg_old'",
        "DELETE FROM session_v2 WHERE id = 'ses_child'",
    ] {
        connection.execute(sql, []).unwrap();
        let current = db_session_fingerprint_blocking(&path, "ses_root").unwrap();
        assert_ne!(current.0, previous.0, "{sql}");
        previous = current;
    }
    connection
        .execute("DELETE FROM session_v2 WHERE id = 'ses_root'", [])
        .unwrap();
    // The retained V1 row has its own source and fingerprint after V2 deletion.
    assert_eq!(
        db_session_source_format(&path, "ses_root"),
        Some(crate::analysis::SourceFormat::OpenCodeSqliteV1)
    );
    connection
        .execute("DELETE FROM session WHERE id = 'ses_root'", [])
        .unwrap();
    assert!(db_session_fingerprint_blocking(&path, "ses_root").is_none());
}

#[test]
fn v2_database_replacement_changes_fingerprint_even_with_identical_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    let before = db_session_fingerprint_blocking(&path, "ses_root").unwrap();
    let revision = db_session_admission_revision_connection(&connection, "ses_root").unwrap();
    drop(connection);
    let backup = directory.path().join("previous.db");
    std::fs::rename(&path, &backup).unwrap();
    std::fs::copy(backup, &path).unwrap();
    let after = db_session_fingerprint_blocking(&path, "ses_root").unwrap();
    assert_eq!(before.1, after.1);
    assert_ne!(before.0, after.0);
    let connection = Connection::open(&path).unwrap();
    assert_ne!(
        db_session_admission_revision_connection(&connection, "ses_root").unwrap(),
        revision
    );
}

#[test]
fn v2_cluster_limits_reject_partial_admission_and_fingerprints() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = pinned_mixed_db(&path);
    for index in 0..MAX_V2_CLUSTER_SESSIONS {
        connection.execute("INSERT INTO session_v2 (id, project_id, parent_id, slug, directory, version, time_created, time_updated) VALUES (?1, 'global', 'ses_root', 'child', '/repo', '2.0.32', 1, 1)", [format!("ses_extra_{index}")]).unwrap();
    }
    assert!(db_session_admission_revision_connection(&connection, "ses_root").is_none());
    assert!(db_session_fingerprint_connection(&connection, "ses_root").is_none());
}

// Diverged for Desktop: indexed_session_title reads the `session.title`
// column out of `opencode.db`. OpenCode stores session metadata exclusively
// in SQLite — there are no per-session JSON files for titles.
fn seed_session_title_db(db_path: &Path, rows: &[(&str, &str)]) {
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                directory TEXT NOT NULL,
                title TEXT NOT NULL,
                version TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
    )
    .unwrap();
    for (id, title) in rows {
        conn.execute(
                "INSERT INTO session (id, project_id, directory, title, version, time_created, time_updated, data)
                 VALUES (?1, 'global', '/repo', ?2, '1.0.0', 1, 2, '{}')",
                rusqlite::params![id, title],
            )
            .unwrap();
    }
}

#[tokio::test]
async fn opencode_batches_preserve_database_sources_and_cross_root_legacy_clusters() {
    let database = tempfile::TempDir::new().unwrap();
    let legacy_parent = tempfile::TempDir::new().unwrap();
    let legacy_child = tempfile::TempDir::new().unwrap();
    let db_path = database.path().join("opencode.db");
    seed_session_title_db(&db_path, &[("ses_db_batch", "Database session")]);
    Connection::open(&db_path)
        .unwrap()
        .execute(
            "UPDATE session SET time_created = 1700000000000, time_updated = 1700000000000",
            [],
        )
        .unwrap();
    write_json(
        &legacy_parent
            .path()
            .join("storage/session/global/ses_parent.json"),
        r#"{"id":"ses_parent","time":{"updated":1700000000000}}"#,
    )
    .await;
    write_json(
        &legacy_child
            .path()
            .join("storage/session/global/ses_child.json"),
        r#"{"id":"ses_child","parentID":"ses_parent","time":{"updated":1700000001000}}"#,
    )
    .await;
    let roots = vec![
        database.path().to_path_buf(),
        legacy_parent.path().to_path_buf(),
        legacy_child.path().to_path_buf(),
    ];
    let expected = discover_recent_in(&roots, 1_700_000_010, 60).await;
    assert_eq!(expected.len(), 2);
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let discover =
        discover_recent_batches_in(&roots, AgentKind::OpenCode, 1_700_000_010, 60, 1, sender);
    let collect = async {
        let mut batches = Vec::new();
        while let Some(batch) = receiver.recv().await {
            batches.push(batch);
        }
        batches
    };
    let (_, batches) = tokio::join!(discover, collect);
    assert!(matches!(
        batches[0].candidates[0].source,
        SessionSource::ProviderDb { .. }
    ));
    assert!(!batches[0].complete);
    assert!(batches.last().unwrap().complete);
    assert!(batches.iter().all(|batch| batch.candidates.len() <= 1));
    let actual: Vec<_> = batches
        .into_iter()
        .flat_map(|batch| batch.candidates)
        .collect();
    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
}

async fn run_with_data_dir<F, Fut, T>(root: &Path, body: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let previous = std::env::var_os("OPENCODE_DATA_DIR");
    unsafe {
        std::env::set_var("OPENCODE_DATA_DIR", root);
    }
    let result = body().await;
    match previous {
        Some(value) => unsafe {
            std::env::set_var("OPENCODE_DATA_DIR", value);
        },
        None => unsafe {
            std::env::remove_var("OPENCODE_DATA_DIR");
        },
    }
    result
}

/// Look up a single id's title through the indexed point-query path
/// (`AgentExplorer::indexed_session_title`), the production route now that
/// the batched title+surface scan is gone.
async fn fetch_title(root: &Path, id: &str) -> Option<ResolvedTitle> {
    run_with_data_dir(root, || async {
        OpenCodeExplorer.indexed_session_title(id).await
    })
    .await
}

#[tokio::test]
#[serial]
async fn test_indexed_session_title_returns_title_from_sqlite() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    seed_session_title_db(
        &root.join("opencode.db"),
        &[("ses_titled", "Implement title interface")],
    );

    let resolved = fetch_title(root, "ses_titled")
        .await
        .expect("title present");
    assert_eq!(resolved.text, "Implement title interface");
    assert_eq!(resolved.source, TitleSource::Explicit);
}

#[tokio::test]
#[serial]
async fn test_indexed_session_title_trims_whitespace() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    seed_session_title_db(
        &root.join("opencode.db"),
        &[("ses_padded", "  Padded title  ")],
    );

    let resolved = fetch_title(root, "ses_padded")
        .await
        .expect("title present");
    assert_eq!(resolved.text, "Padded title");
}

#[tokio::test]
#[serial]
async fn test_indexed_session_title_returns_none_for_whitespace_only_title() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    seed_session_title_db(&root.join("opencode.db"), &[("ses_blank", "   ")]);

    // Whitespace-only titles trim to None so the frontend's "no title"
    // rendering applies.
    assert!(fetch_title(root, "ses_blank").await.is_none());
}

#[tokio::test]
#[serial]
async fn test_indexed_session_title_skips_unknown_ids() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    seed_session_title_db(&root.join("opencode.db"), &[("ses_other", "Other")]);

    assert!(fetch_title(root, "ses_unknown").await.is_none());
}

#[tokio::test]
#[serial]
async fn test_indexed_session_title_returns_none_when_db_absent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();

    assert!(fetch_title(root, "ses_missing").await.is_none());
}
