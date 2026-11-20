use std::{fs, path::PathBuf, process::Command};

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
                     PRIMARY KEY (environment_key, agent, session_id)
                 ) STRICT;
                 INSERT INTO session VALUES
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
        let output = self.command().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        serde_json::from_slice(&output.stdout).unwrap()
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
            "sessions": [
                {
                    "identity": {"environment_key": "native", "agent": "codex", "session_id": "example-shared"},
                    "title": null,
                    "updated_at_epoch": 30
                },
                {
                    "identity": {"environment_key": "native", "agent": "pi", "session_id": "example-shared"},
                    "title": "Example migration",
                    "updated_at_epoch": 20
                }
            ],
            "truncated": true
        })
    );
    let response = fixture.search(&["example", "--limit", "6"]);
    assert_eq!(response["truncated"], false);
    let sessions = response["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 5);
    assert_eq!(sessions[2]["identity"]["environment_key"], "wsl:example");
    assert_eq!(sessions[3]["updated_at_epoch"], 5);
    assert_eq!(sessions[4]["updated_at_epoch"], Value::Null);
    assert_eq!(sessions[4]["title"], Value::Null);
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
        response["sessions"][0]["identity"]["session_id"],
        "plain-id"
    );
    assert_eq!(response["sessions"][0]["title"], "Literal 100%_done");
    for query in ["hidden prompt", "%' OR 1 = 1 --", "missing"] {
        assert_eq!(
            fixture.search(&[query]),
            json!({"source": "local", "sessions": [], "truncated": false})
        );
    }
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
        response["sessions"][0]["title"].as_str(),
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
