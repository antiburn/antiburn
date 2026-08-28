use super::*;
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
    let mut exports = tokio::task::JoinSet::new();
    exports.spawn(async {
        Ok((
            "ses_valid".to_string(),
            10,
            r#"{"info":{"id":"ses_valid"},"messages":[]}"#.to_string(),
            "test-valid".to_string(),
        ))
    });
    exports.spawn(async {
        Ok((
            "ses_invalid".to_string(),
            10,
            "not-json".to_string(),
            "test-invalid".to_string(),
        ))
    });

    let collected = collect_cli_exports(exports).await.unwrap();
    assert_eq!(collected.len(), 1);
    assert!(collected.contains_key("ses_valid"));
    assert!(!collected.contains_key("ses_invalid"));
}

#[tokio::test]
async fn cli_export_collection_falls_back_when_every_session_fails() {
    let mut exports = tokio::task::JoinSet::new();
    exports.spawn(async {
        Ok((
            "ses_invalid".to_string(),
            10,
            "not-json".to_string(),
            "test-invalid-only".to_string(),
        ))
    });

    assert!(matches!(
        collect_cli_exports(exports).await,
        Err(CliBridgeError::InvalidOutput(_))
    ));
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
        } => {
            assert_eq!(agent, &AgentKind::OpenCode);
            assert_eq!(source_db_path, &db_path);
            assert_eq!(session_id, "ses_db");
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
        db_session_fingerprint_blocking(&db_path, "ses_db"),
        Some((1_772_602_559_000, 3)),
    );

    let located = run_with_data_dir(root, || async {
        locate_db_session_source("ses_db".to_string()).await
    })
    .await;
    match located {
        DirectSessionSource::Found(SessionSource::ProviderDb {
            db_path: located_db,
            session_id,
            ..
        }) => {
            assert_eq!(located_db, db_path);
            assert_eq!(session_id, "ses_db");
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
async fn test_opencode_discover_cwds_reads_recent_sqlite_directories() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    let db_path = root.join("opencode.db");
    let conn = Connection::open(&db_path).unwrap();

    conn.execute_batch(
        "
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                directory TEXT NOT NULL,
                title TEXT,
                time_created INTEGER,
                time_updated INTEGER
            );
            ",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session (id, directory, title, time_created, time_updated)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            "ses_recent",
            "/Users/avery/dev/ai-barometer",
            "Recent",
            10_000_000i64,
            10_000_000i64
        ],
    )
    .unwrap();

    let cwds = discover_cwds_in(&[root.to_path_buf()], 10_000, 100).await;

    assert_eq!(cwds, vec!["/Users/avery/dev/ai-barometer".to_string()]);
}

#[tokio::test]
async fn test_opencode_discover_cwds_uses_session_json_without_message_parts() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    let session_file = root
        .join("storage")
        .join("session")
        .join("global")
        .join("ses_recent.json");
    write_json(
        &session_file,
        r#"{"id":"ses_recent","directory":"/Users/avery/dev/demo-cli"}"#,
    )
    .await;
    set_file_mtime(&session_file, 9_950);

    let old_part = root
        .join("storage")
        .join("part")
        .join("global")
        .join("part_recent.json");
    write_json(
        &old_part,
        r#"{"id":"part_recent","sessionID":"ses_recent","type":"text"}"#,
    )
    .await;
    set_file_mtime(&old_part, 9_950);

    let cwds = discover_cwds_in(&[root.to_path_buf()], 10_000, 100).await;

    assert_eq!(cwds, vec!["/Users/avery/dev/demo-cli".to_string()]);
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

// Diverged for Desktop: session_title reads the `session.title` column
// out of `opencode.db`. OpenCode stores session metadata exclusively in
// SQLite — there are no per-session JSON files for titles.
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

/// Look up a single id's `(title, surface)` in the batched result. The
/// batched method returns *all* rows; per-id lookup is what every test
/// here cared about historically.
async fn fetch_row(root: &Path, id: &str) -> Option<SessionTitleAndSurface> {
    run_with_data_dir(root, || async {
        OpenCodeExplorer
            .session_titles_and_surfaces()
            .await
            .into_iter()
            .find(|row| row.session_id == id)
    })
    .await
}

#[tokio::test]
#[serial]
async fn test_session_titles_returns_title_from_sqlite() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    seed_session_title_db(
        &root.join("opencode.db"),
        &[("ses_titled", "Implement title interface")],
    );

    let row = fetch_row(root, "ses_titled").await.expect("row present");
    assert_eq!(row.title.as_deref(), Some("Implement title interface"));
    assert_eq!(row.surface, "cli");
}

#[tokio::test]
#[serial]
async fn test_session_titles_trims_whitespace() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    seed_session_title_db(
        &root.join("opencode.db"),
        &[("ses_padded", "  Padded title  ")],
    );

    let row = fetch_row(root, "ses_padded").await.expect("row present");
    assert_eq!(row.title.as_deref(), Some("Padded title"));
}

#[tokio::test]
#[serial]
async fn test_session_titles_returns_none_for_whitespace_only_title() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    seed_session_title_db(&root.join("opencode.db"), &[("ses_blank", "   ")]);

    let row = fetch_row(root, "ses_blank").await.expect("row present");
    // Whitespace-only titles trim to None so the frontend's "no title"
    // rendering applies; the row itself still appears in the batch.
    assert!(row.title.is_none());
}

#[tokio::test]
#[serial]
async fn test_session_titles_skips_unknown_ids() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    seed_session_title_db(&root.join("opencode.db"), &[("ses_other", "Other")]);

    assert!(fetch_row(root, "ses_unknown").await.is_none());
}

#[tokio::test]
#[serial]
async fn test_session_titles_returns_empty_when_db_absent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();

    let rows = run_with_data_dir(root, || async {
        OpenCodeExplorer.session_titles_and_surfaces().await
    })
    .await;

    assert!(rows.is_empty());
}
