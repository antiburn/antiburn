use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    config: PathBuf,
    database: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let database_dir = root.path().join("ai.antiburn.desktop");
        fs::create_dir(&database_dir).unwrap();
        let database = database_dir.join("antiburn.sqlite3");
        let config = root.path().join("cli.json");
        fs::write(
            &config,
            json!({
                "data_root": root.path(),
                "search_limit": 2,
                "state_file": root.path().join("state.json"),
                "log_file": root.path().join("calls.log")
            })
            .to_string(),
        )
        .unwrap();
        Self {
            root,
            config,
            database,
        }
    }

    fn create_database(&self) {
        let connection = Connection::open(&self.database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (
                     environment_key TEXT NOT NULL,
                     agent TEXT NOT NULL,
                     session_id TEXT NOT NULL,
                      title TEXT,
                      title_source TEXT,
                      updated_at_epoch INTEGER,
                      source_kind TEXT NOT NULL DEFAULT 'file',
                      source_label TEXT NOT NULL DEFAULT '',
                      source_format TEXT,
                     PRIMARY KEY (environment_key, agent, session_id)
                 ) STRICT;
                 INSERT INTO session (environment_key, agent, session_id, title, title_source, updated_at_epoch) VALUES
                     ('wsl:example', 'pi', 'example-shared', 'Example run', 'explicit', 20),
                     ('native', 'pi', 'example-shared', 'Example migration', 'userRename', 20),
                     ('native', 'codex', 'example-shared', NULL, NULL, 30),
                     ('native', 'pi', 'example-prompt', 'Hidden prompt', 'firstMessage', NULL),
                     ('native', 'pi', 'plain-id', 'Literal 100%_done', 'aiGenerated', 40),
                     ('native', 'pi', 'example-older', NULL, NULL, 5);",
            )
            .unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_antiburn"));
        command.arg("--config").arg(&self.config).arg("search");
        command
    }

    fn search(&self, args: &[&str]) -> Value {
        let output = self.command().arg("--json").args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn get(&self, session_id: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_antiburn"))
            .arg("--config")
            .arg(&self.config)
            .args(["get", session_id])
            .args(args)
            .output()
            .unwrap()
    }
}

#[test]
fn searches_local_metadata_with_a_bound_and_full_session_identity() {
    let fixture = Fixture::new();
    fixture.create_database();
    let before = fs::read(&fixture.database).unwrap();

    assert_eq!(
        fixture.search(&["  EXAMPLE-shared  "]),
        json!({
            "source": "local",
            "sources": {"local": {"truncated": true}},
            "sessions": [
                {
                    "local": {
                        "identity": {"environment_key": "native", "agent": "codex", "session_id": "example-shared"},
                        "title": null,
                        "updated_at_epoch": 30
                    }
                },
                {
                    "local": {
                        "identity": {"environment_key": "native", "agent": "pi", "session_id": "example-shared"},
                        "title": "Example migration",
                        "updated_at_epoch": 20
                    }
                }
            ],
            "truncated": true
        })
    );
    let response = fixture.search(&["example", "--limit", "6"]);
    assert_eq!(response["truncated"], false);
    let sessions = response["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 5);
    assert_eq!(
        sessions[2]["local"]["identity"]["environment_key"],
        "wsl:example"
    );
    assert_eq!(sessions[3]["local"]["updated_at_epoch"], 5);
    assert_eq!(sessions[4]["local"]["updated_at_epoch"], Value::Null);
    assert_eq!(sessions[4]["local"]["title"], Value::Null);
    assert_eq!(fs::read(&fixture.database).unwrap(), before);
    assert!(!fixture.root.path().join("state.json").exists());
    assert!(!fixture.root.path().join("calls.log").exists());
}

#[test]
fn matches_literal_titles_but_not_first_message_excerpts() {
    let fixture = Fixture::new();
    fixture.create_database();
    let response = fixture.search(&["100%_DONE"]);
    assert_eq!(
        response["sessions"][0]["local"]["identity"]["session_id"],
        "plain-id"
    );
    assert_eq!(
        response["sessions"][0]["local"]["title"],
        "Literal 100%_done"
    );
    for query in ["hidden prompt", "%' OR 1 = 1 --", "missing"] {
        assert_eq!(
            fixture.search(&[query]),
            json!({"source": "local", "sources": {"local": {"truncated": false}}, "sessions": [], "truncated": false})
        );
    }
}

#[test]
fn local_get_uses_the_exact_indexed_source_and_escapes_captured_text() {
    let fixture = Fixture::new();
    fixture.create_database();
    let path = fixture.root.path().join("example-shared.jsonl");
    let contents = concat!(
        r#"{"type":"user","sessionId":"example-shared","uuid":"first","parentUuid":null,"message":{"role":"user","content":"Don't print \"this\" or \\ \u001b[2J\u009b\u202e"}}"#,
        "\n",
        r#"{"type":"assistant","sessionId":"example-shared","uuid":"second","parentUuid":"first","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Synthetic private reasoning"},{"type":"text","text":"Synthetic reply"}]}}"#,
        "\n",
        r#"{"type":"assistant","message":{"content":"Uncommitted tail"#,
    );
    fs::write(&path, contents).unwrap();
    let db = Connection::open(&fixture.database).unwrap();
    db.execute(
        "UPDATE session SET agent='claude-code', source_label=?1, source_format='\"claude_jsonl\"'
           WHERE session_id='example-shared' AND environment_key='native' AND agent='pi'",
        [path.to_str().unwrap()],
    )
    .unwrap();
    drop(db);
    let get = |args: &[&str]| fixture.get("example-shared", args);
    let ambiguous = get(&["--json"]);
    assert!(!ambiguous.status.success());
    assert!(ambiguous.stdout.is_empty());
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("matches multiple local"));
    let selected = get(&[
        "--source",
        "local",
        "--agent",
        "claude-code",
        "--environment",
        "native",
        "--json",
    ]);
    assert!(selected.status.success(), "{:?}", selected.stderr);
    let json = String::from_utf8_lossy(&selected.stdout);
    for omitted in ['\x1b', '\u{009b}', '\u{202e}'] {
        assert!(!json.contains(omitted));
    }
    let result: Value = serde_json::from_slice(&selected.stdout).unwrap();
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["source"], "local");
    assert_eq!(result["session_id"], "example-shared");
    assert!(result.get("workspace_id").is_none());
    assert!(result.get("revision").is_none());
    assert_eq!(result["environment_key"], "native");
    assert_eq!(result["document"]["header"]["title"], "Example migration");
    assert_eq!(result["document"]["header"]["agent"]["name"], "claude-code");
    assert_eq!(result["document"]["coverage"]["source"], "partial");
    assert_eq!(result["document"]["coverage"]["projection"], "partial");
    assert_eq!(result["document"]["messages"].as_array().unwrap().len(), 2);
    assert_eq!(result["document"]["messages"][0]["role"], "user");
    assert_eq!(
        result["document"]["messages"][0]["parts"][0]["text"],
        "Don't print \"this\" or \\ \x1b[2J\u{009b}\u{202e}"
    );
    assert_eq!(
        result["document"]["messages"][1]["parts"][1]["text"],
        "Synthetic reply"
    );
    assert!(!json.contains("Uncommitted tail"));
    assert_eq!(
        result["document"]["messages"][1]["parts"][0]["type"],
        "thinking"
    );
    let text = get(&["--agent", "claude-code", "--environment", "native"]);
    assert!(text.status.success(), "{:?}", text.stderr);
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.starts_with("Example migration\n"));
    assert!(text.contains("Session example-shared (local)"));
    assert!(text.contains("Agent: claude-code\n"));
    assert!(text.contains("Environment: native\n"));
    assert!(text.contains("Source coverage: partial\n"));
    assert!(text.contains(r#"User: "Don't print \"this\" or \\ \u{1b}[2J\u{9b}\u{202e}""#));
    assert!(text.contains("Assistant: \"Synthetic reply\""));
    assert!(!text.contains('\x1b'));

    let downstream = get(&["--agent", "pi", "--environment", "wsl:example"]);
    assert_eq!(downstream.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&downstream.stderr)
            .contains("cannot read this local session environment")
    );
    assert!(downstream.stdout.is_empty());
    let unsupported = get(&["--agent", "codex", "--environment", "native"]);
    assert_eq!(unsupported.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&unsupported.stderr).contains("format is not supported"));
    assert!(unsupported.stdout.is_empty());

    fs::remove_file(&path).unwrap();
    let missing = get(&["--agent", "claude-code", "--environment", "native"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("missing or unreadable"));
    assert!(missing.stdout.is_empty());
}

#[test]
fn local_get_requires_an_exact_unambiguous_identity() {
    let fixture = Fixture::new();
    fixture.create_database();
    let assert_error = |id: &str, args: &[&str], expected: &str| {
        let output = fixture.get(id, args);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!("antiburn: {expected}\n")
        );
    };
    for (id, selectors) in [
        ("example-shar", &[][..]),
        ("EXAMPLE-shared", &[]),
        ("Example migration", &[]),
        ("example-shared' OR 1 = 1 --", &[]),
        ("example-shared", &["--agent", "PI"]),
        ("example-shared", &["--environment", "NATIVE"]),
        (
            "example-shared",
            &["--agent", "codex", "--environment", "wsl:example"],
        ),
    ] {
        assert_error(
            id,
            selectors,
            &format!("session ID {id:?} was not found in the local Antiburn database"),
        );
    }
    for selectors in [&[][..], &["--agent", "pi"], &["--environment", "native"]] {
        assert_error(
            "example-shared",
            selectors,
            "session ID \"example-shared\" matches multiple local Antiburn sessions; use --agent and --environment to select one",
        );
    }
}

#[test]
fn local_get_does_not_label_unattributed_text_as_user_text() {
    for (agent, format, contents) in [
        (
            "claude-code",
            "claude_jsonl",
            concat!(
                r#"{"type":"user","sessionId":"plain-id","uuid":"first","parentUuid":null,"message":{"role":"user","content":"Synthetic request."}}"#,
                "\n",
                r#"{"type":"user","sessionId":"plain-id","uuid":"second","parentUuid":"first","isMeta":true,"message":{"role":"user","content":"Synthetic context."}}"#,
                "\n",
            ),
        ),
        (
            "codex",
            "codex_rollout_jsonl",
            concat!(
                r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"plain-id"}}"#,
                "\n",
                r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Synthetic request."}]}}"#,
                "\n",
                r#"{"timestamp":"2026-01-01T00:00:02Z","type":"response_item","metadata":{"inherited_user_message":true},"payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Synthetic context."}]}}"#,
                "\n",
            ),
        ),
    ] {
        let fixture = Fixture::new();
        fixture.create_database();
        let path = fixture.root.path().join("plain-id.jsonl");
        fs::write(&path, contents).unwrap();
        Connection::open(&fixture.database)
            .unwrap()
            .execute(
                "UPDATE session SET agent=?1, source_label=?2, source_format=?3 WHERE session_id='plain-id'",
                [agent, path.to_str().unwrap(), &json!(format).to_string()],
            )
            .unwrap();
        let output = fixture.get("plain-id", &["--json"]);
        assert!(
            output.status.success(),
            "{agent}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        let messages = document["document"]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2, "{agent}");
        for (message, role, text) in [
            (&messages[0], "user", "Synthetic request."),
            (&messages[1], "unknown", "Synthetic context."),
        ] {
            assert_eq!(message["role"], role, "{agent}: {text}");
            assert_eq!(message["parts"][0]["text"], text, "{agent}: {role}");
        }
        let output = fixture.get("plain-id", &[]);
        assert!(output.status.success(), "{agent}: {:?}", output.stderr);
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("User: \"Synthetic request.\""));
        assert!(text.contains("Unknown: \"Synthetic context.\""));
        assert!(!text.contains("User: \"Synthetic context.\""));
    }
}

#[test]
fn local_get_uses_each_supported_jsonl_reader() {
    for (agent, format, source) in [
        (
            "codex",
            "codex_rollout_jsonl",
            concat!(
                r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"plain-id"}}"#,
                "\n",
                r#"{"timestamp":"2026-01-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Synthetic question."}]}}"#,
                "\n",
            ),
        ),
        (
            "pi",
            "pi_v3_jsonl",
            concat!(
                r#"{"type":"session","version":3,"id":"plain-id","timestamp":"2026-01-01T00:00:00Z"}"#,
                "\n",
                r#"{"type":"message","id":"turn","parentId":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":"Synthetic question."}}"#,
                "\n",
            ),
        ),
        (
            "cursor",
            "cursor_cli_agent_jsonl",
            concat!(
                r#"{"role":"user","message":{"content":[{"type":"text","text":"Synthetic question."}]}}"#,
                "\n",
            ),
        ),
    ] {
        let fixture = Fixture::new();
        fixture.create_database();
        let path = fixture.root.path().join("plain-id.jsonl");
        fs::write(&path, source).unwrap();
        Connection::open(&fixture.database)
            .unwrap()
            .execute(
                "UPDATE session SET agent=?1, source_label=?2, source_format=?3 WHERE session_id='plain-id'",
                [agent, path.to_str().unwrap(), &json!(format).to_string()],
            )
            .unwrap();
        let output = fixture.get("plain-id", &["--source", "local", "--json"]);
        assert!(
            output.status.success(),
            "{agent}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["document"]["header"]["agent"]["name"], agent);
        assert_eq!(
            document["document"]["messages"][0]["parts"][0]["text"],
            "Synthetic question."
        );
    }
}

#[test]
fn local_get_reports_jsonl_size_and_coverage_limits() {
    let fixture = Fixture::new();
    fixture.create_database();
    let path = fixture.root.path().join("plain-id.jsonl");
    Connection::open(&fixture.database)
        .unwrap()
        .execute(
            "UPDATE session SET agent='claude-code', source_label=?1, source_format='\"claude_jsonl\"'
               WHERE session_id='plain-id'",
            [path.to_str().unwrap()],
        )
        .unwrap();
    for (size, expected) in [
        (16 * 1024 * 1024 + 1, "exceeds the size limit"),
        (0, "no supported message content"),
    ] {
        fs::File::create(&path).unwrap().set_len(size).unwrap();
        let output = fixture.get("plain-id", &["--json"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
    }

    fs::write(&path, r#"{"type":"user","message":"incomplete"#).unwrap();
    let output = fixture.get("plain-id", &["--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("incomplete record; try again"));

    let complete = br#"{"type":"user","sessionId":"plain-id","uuid":"first","parentUuid":null,"message":{"role":"user","content":"Complete final record."}}"#;
    fs::write(&path, complete).unwrap();
    let output = fixture.get("plain-id", &["--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["document"]["coverage"]["source"], "unknown");
    assert_eq!(
        response["document"]["messages"][0]["parts"][0]["text"],
        "Complete final record."
    );

    fs::write(
        &path,
        [complete.as_slice(), b"\n{\"message\":\"\xff\"}"].concat(),
    )
    .unwrap();
    let output = fixture.get("plain-id", &["--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["document"]["coverage"]["source"], "partial");
    assert_eq!(
        response["document"]["messages"].as_array().unwrap().len(),
        1
    );

    let oversized = format!(
        "{{\"type\":\"user\",\"message\":{{\"content\":\"{}\"}}}}\n",
        "x".repeat(8 * 1024 * 1024)
    );
    fs::write(&path, oversized).unwrap();
    let output = fixture.get("plain-id", &["--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["document"]["coverage"]["source"], "partial");
    assert_eq!(response["document"]["messages"], json!([]));
}

#[test]
fn local_get_limits_index_rows_and_hides_non_display_titles() {
    let fixture = Fixture::new();
    fixture.create_database();
    let path = fixture.root.path().join("plain-id.jsonl");
    fs::write(
        &path,
        concat!(
            r#"{"type":"user","sessionId":"plain-id","uuid":"first","parentUuid":null,"message":{"role":"user","content":"Synthetic request."}}"#,
            "\n",
        ),
    )
    .unwrap();
    let db = Connection::open(&fixture.database).unwrap();
    let title = "x".repeat(80 * 1024);
    db.execute(
        "UPDATE session SET agent='claude-code', source_label=?1, source_format='\"claude_jsonl\"',
                title=?2, title_source='userRename' WHERE session_id='plain-id'",
        [path.to_str().unwrap(), &title],
    )
    .unwrap();
    let output = fixture.get("plain-id", &["--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("raise local_search_sqlite_length_limit_bytes if it is below the maximum")
    );

    let mut config: Value = serde_json::from_slice(&fs::read(&fixture.config).unwrap()).unwrap();
    config["local_search_sqlite_length_limit_bytes"] = json!(128 * 1024);
    fs::write(&fixture.config, config.to_string()).unwrap();
    let read_title = || {
        let output = fixture.get("plain-id", &["--json"]);
        assert!(output.status.success(), "{:?}", output.stderr);
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        response["document"]["header"].get("title").cloned()
    };
    assert_eq!(read_title(), Some(json!(title)));
    for source in ["firstMessage", "unknown"] {
        db.execute(
            "UPDATE session SET title_source=?1 WHERE session_id='plain-id'",
            [source],
        )
        .unwrap();
        assert_eq!(read_title(), None, "{source}");
    }
}

#[test]
fn local_get_reports_a_locked_index_as_retryable() {
    let fixture = Fixture::new();
    fixture.create_database();
    let mut config: Value = serde_json::from_slice(&fs::read(&fixture.config).unwrap()).unwrap();
    config["busy_timeout_ms"] = json!(0);
    fs::write(&fixture.config, config.to_string()).unwrap();
    let writer = Connection::open(&fixture.database).unwrap();
    writer.execute_batch("BEGIN EXCLUSIVE").unwrap();

    let output = fixture.get("plain-id", &["--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("index is busy; try again"));
}

fn opencode_v2_fixture(path: &std::path::Path) -> Connection {
    let database = Connection::open(path).unwrap();
    database.execute_batch(
        r#"CREATE TABLE session_v2 (
               id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL DEFAULT '/synthetic',
               title TEXT, time_created INTEGER NOT NULL DEFAULT 0,
               time_updated INTEGER NOT NULL DEFAULT 0
           );
           CREATE TABLE session_message (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
               seq INTEGER NOT NULL, time_created INTEGER NOT NULL DEFAULT 0,
               time_updated INTEGER NOT NULL DEFAULT 0, data TEXT NOT NULL
           );
           CREATE UNIQUE INDEX session_message_session_seq_idx ON session_message(session_id, seq);
           CREATE TABLE event_sequence (aggregate_id TEXT PRIMARY KEY NOT NULL, seq INTEGER NOT NULL);
           INSERT INTO session_v2 (id, parent_id) VALUES ('plain-id', NULL), ('child', 'plain-id');
           INSERT INTO session_message (id, session_id, type, seq, data) VALUES
               ('request', 'plain-id', 'user', 0, '{"time":{"created":1},"text":"Synthetic request.","skills":[{"id":"guide","name":"guide","text":"Synthetic context."}]}'),
               ('reply', 'plain-id', 'assistant', 1,
                '{"time":{"created":2},"model":{"id":"synthetic-model","providerID":"synthetic"},"content":[{"type":"text","text":"Synthetic reply."}]}'),
               ('child-reply', 'child', 'assistant', 0,
                '{"time":{"created":3},"model":{"id":"synthetic-model","providerID":"synthetic"},"content":[{"type":"text","text":"Synthetic child content."}]}');"#,
    ).unwrap();
    database
}

#[test]
fn local_get_reads_only_the_indexed_opencode_v2_session() {
    use antiburn_local::discovery::agents::opencode::db_session_fingerprint_snapshot;

    let fixture = Fixture::new();
    fixture.create_database();
    let path = fixture.root.path().join("opencode-v2.db");
    let source = opencode_v2_fixture(&path);
    let long_part = "x".repeat(300 * 1024);
    let reply = json!({
        "time": {"created": 2},
        "model": {"id": "synthetic-model", "providerID": "synthetic"},
        "content": [
            {"type": "text", "text": "Synthetic reply."},
            {"type": "text", "text": long_part}
        ]
    });
    source
        .execute(
            "UPDATE session_message SET data=?1 WHERE id='reply'",
            [reply.to_string()],
        )
        .unwrap();
    let (_, (hash, rows)) = db_session_fingerprint_snapshot(&path, "plain-id", &|| false).unwrap();
    let indexed = Connection::open(&fixture.database).unwrap();
    indexed
        .execute_batch(
            "ALTER TABLE session ADD COLUMN fingerprint_source_path TEXT;
         ALTER TABLE session ADD COLUMN source_fingerprint TEXT;",
        )
        .unwrap();
    indexed.execute(
        r#"UPDATE session SET agent='opencode', source_kind='providerDb',
           source_label='opencode:plain-id:open_code_v2_sqlite', source_format='"open_code_v2_sqlite"',
           fingerprint_source_path=?1, source_fingerprint=?2 WHERE session_id='plain-id'"#,
        [path.to_str().unwrap(), &format!("sv1:db:{hash}:{rows}")],
    ).unwrap();
    let get = || fixture.get("plain-id", &["--source", "local", "--json"]);
    let before = fs::read(&path).unwrap();
    let output = get();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["source"], "local");
    assert_eq!(envelope["session_id"], "plain-id");
    assert_eq!(envelope["document"]["header"]["agent"]["name"], "opencode");
    assert_eq!(envelope["document"]["coverage"]["source"], "partial");
    let messages = envelope["document"]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["parts"][0]["text"], "Synthetic request.");
    assert_eq!(messages[1]["role"], "unknown");
    assert_eq!(messages[1]["parts"][0]["text"], "Synthetic context.");
    assert_eq!(messages[2]["parts"][0]["text"], "Synthetic reply.");
    let clipped = &messages[2]["parts"][1];
    assert_eq!(clipped["content_state"], "partial");
    assert_eq!(
        clipped["text"].as_str().unwrap().len() + "Synthetic reply.".len(),
        256 * 1024
    );
    assert_eq!(fs::read(&path).unwrap(), before);

    source
        .execute(
            "UPDATE session_message SET data=?1 WHERE id='reply'",
            [r#"{"content":[{"type":"text","text":"Changed reply."}]}"#],
        )
        .unwrap();
    let stale = get();
    assert_eq!(stale.status.code(), Some(1));
    assert!(stale.stdout.is_empty());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("indexed OpenCode V2 source changed"));

    indexed
        .execute(
            "UPDATE session SET source_fingerprint=NULL WHERE session_id='plain-id'",
            [],
        )
        .unwrap();
    let unverified = get();
    assert_eq!(unverified.status.code(), Some(1));
    assert!(unverified.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&unverified.stderr)
            .contains("indexed OpenCode V2 database is not ready")
    );
}

#[test]
fn default_search_output_is_a_text_summary_without_terminal_controls() {
    let fixture = Fixture::new();
    fixture.create_database();
    Connection::open(&fixture.database)
        .unwrap()
        .execute(
            "UPDATE session SET title=?1 WHERE session_id='plain-id'",
            ["Example\x1b[2J\nnext"],
        )
        .unwrap();
    let output = fixture.command().arg("plain-id").output().unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("1 session (local)\n"));
    assert!(text.contains("Example [2J next"));
    assert!(!text.contains('\x1b'));
}

#[test]
fn local_search_applies_the_configured_sqlite_length_limit() {
    let fixture = Fixture::new();
    fixture.create_database();
    let title = "x".repeat(80 * 1024);
    let connection = Connection::open(&fixture.database).unwrap();
    connection
        .execute(
            "UPDATE session SET title = ?1 WHERE session_id = 'plain-id'",
            [&title],
        )
        .unwrap();
    drop(connection);

    let output = fixture.command().arg("plain-id").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not search Antiburn sessions"));

    let mut config: Value = serde_json::from_slice(&fs::read(&fixture.config).unwrap()).unwrap();
    config["local_search_sqlite_length_limit_bytes"] = json!(128 * 1024);
    fs::write(&fixture.config, config.to_string()).unwrap();
    let response = fixture.search(&["plain-id"]);
    assert_eq!(
        response["sessions"][0]["local"]["title"].as_str(),
        Some(title.as_str())
    );
}

#[test]
fn fails_on_invalid_input_or_a_missing_database() {
    let fixture = Fixture::new();
    let long_query = "x".repeat(513);
    for args in [
        vec![""],
        vec!["x\n"],
        vec![long_query.as_str()],
        vec!["example", "--limit", "0"],
        vec!["example", "--limit", "101"],
    ] {
        let output = fixture.command().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
    let output = fixture.command().arg("example").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("database was not found"));
    assert!(!fixture.database.exists());
}

#[cfg(not(feature = "cloud"))]
#[test]
fn the_standard_build_does_not_offer_cloud_commands() {
    let fixture = Fixture::new();
    let cloud = fixture
        .command()
        .args(["example", "--source", "cloud"])
        .output()
        .unwrap();
    assert_eq!(cloud.status.code(), Some(2));
    assert!(cloud.stdout.is_empty());
    let get = Command::new(env!("CARGO_BIN_EXE_antiburn"))
        .args(["get", "--help"])
        .output()
        .unwrap();
    assert!(get.status.success());
    let help = String::from_utf8(get.stdout).unwrap();
    assert!(help.contains("--json"));
    assert!(!help.contains("--after"));
}
