use std::process::Command;

use antiburn_local::{
    analysis::{
        ContextEvidence, EvidenceSource, EvidenceValue, SessionEvidenceAccumulator,
        SourceCapabilities, SourceKind, TurnFacts,
    },
    insights::ReportCatalogs,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::tempdir;

#[test]
fn checks_command_reads_the_selected_database() {
    let temp = tempdir().unwrap();
    let data_root = platform_data_root(temp.path());
    let database_dir = data_root.join("ai.antiburn.desktop");
    std::fs::create_dir_all(&database_dir).unwrap();
    let database_path = database_dir.join("antiburn.sqlite3");
    create_fixture_database(&database_path);

    let mut command = Command::new(env!("CARGO_BIN_EXE_antiburn"));
    configure_data_directory(&mut command, temp.path(), &data_root);
    let output = command
        .args(["checks", "argument-session"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["session_id"], "argument-session");
    assert_eq!(response["identity"]["agent"], "pi");
    assert!(response.get("evidence").is_none());
    assert_eq!(response["checks"], json!([]));
}

#[test]
fn checks_command_returns_only_new_problematic_findings() {
    let temp = tempdir().unwrap();
    let data_root = platform_data_root(temp.path());
    let database_dir = data_root.join("ai.antiburn.desktop");
    std::fs::create_dir_all(&database_dir).unwrap();
    let database_path = database_dir.join("antiburn.sqlite3");
    create_fixture_database(&database_path);

    let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".to_owned(),
        session_id: "argument-session".to_owned(),
        kind: SourceKind::File,
        capabilities: SourceCapabilities::claude(),
    })
    .evidence(&TurnFacts::default());
    evidence.context = EvidenceValue::Complete(ContextEvidence {
        max_request_context_tokens: ReportCatalogs::default().depth_cap_tokens + 1,
        top_depth_examples: Vec::new(),
    });
    let evidence_json = serde_json::to_string(&evidence).unwrap();
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute(
            "INSERT INTO session_evidence (
                environment_key, agent, session_id, status, evidence_json
             ) VALUES ('native', 'pi', 'argument-session', 'ready', ?1)",
            [evidence_json],
        )
        .unwrap();
    drop(connection);

    let mut command = Command::new(env!("CARGO_BIN_EXE_antiburn"));
    configure_data_directory(&mut command, temp.path(), &data_root);
    let output = command
        .args(["checks", "argument-session"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["freshness_evaluated"], false);
    assert!(response.get("evidence").is_none());
    assert_eq!(response["checks"].as_array().unwrap().len(), 1);
    assert_eq!(response["checks"][0]["source"], "session_evidence_badge");
    assert_eq!(response["checks"][0]["check_id"], "sessions_over_depth");
    assert_eq!(response["checks"][0]["status"], "finding");

    let mut repeated_command = Command::new(env!("CARGO_BIN_EXE_antiburn"));
    configure_data_directory(&mut repeated_command, temp.path(), &data_root);
    let repeated = repeated_command
        .args(["checks", "argument-session"])
        .output()
        .unwrap();
    assert!(
        repeated.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let repeated_response: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert!(repeated_response["checks"].as_array().unwrap().is_empty());
}

#[test]
fn config_file_can_override_the_release_database_location() {
    let temp = tempdir().unwrap();
    let data_root = temp.path().join("custom-data");
    let database_dir = data_root.join("antiburn-test");
    std::fs::create_dir_all(&database_dir).unwrap();
    create_fixture_database(&database_dir.join("custom.sqlite3"));
    let config_path = temp.path().join("cli.json");
    std::fs::write(
        &config_path,
        json!({
            "data_root": data_root,
            "app_identifier": "antiburn-test",
            "database_filename": "custom.sqlite3",
            "busy_timeout_ms": 12000
        })
        .to_string(),
    )
    .unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_antiburn"));
    configure_data_directory(&mut command, temp.path(), &data_root);
    let output = command
        .arg("--config")
        .arg(config_path)
        .args(["checks", "argument-session"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["identity"]["agent"], "pi");
}

#[cfg(target_os = "macos")]
fn platform_data_root(home: &std::path::Path) -> std::path::PathBuf {
    home.join("Library/Application Support")
}

#[cfg(target_os = "macos")]
fn configure_data_directory(command: &mut Command, home: &std::path::Path, _: &std::path::Path) {
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"));
}

#[cfg(target_os = "linux")]
fn platform_data_root(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".local/share")
}

#[cfg(target_os = "linux")]
fn configure_data_directory(
    command: &mut Command,
    home: &std::path::Path,
    data_root: &std::path::Path,
) {
    command
        .env("HOME", home)
        .env("XDG_DATA_HOME", data_root)
        .env("XDG_CONFIG_HOME", home.join(".config"));
}

#[cfg(target_os = "windows")]
fn platform_data_root(home: &std::path::Path) -> std::path::PathBuf {
    home.join("AppData/Roaming")
}

#[cfg(target_os = "windows")]
fn configure_data_directory(
    command: &mut Command,
    _: &std::path::Path,
    data_root: &std::path::Path,
) {
    command.env("APPDATA", data_root);
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
compile_error!("the CLI integration test needs a supported desktop data directory");

fn create_fixture_database(path: &std::path::Path) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (
                environment_key TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_id TEXT NOT NULL,
                PRIMARY KEY (environment_key, agent, session_id)
             );
             CREATE TABLE session_evidence (
                environment_key TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_id TEXT NOT NULL,
                status TEXT NOT NULL,
                analyzed_generation INTEGER,
                processed_fingerprint TEXT,
                parser_revision INTEGER,
                analyzer_revision INTEGER,
                evidence_schema_revision INTEGER,
                evidence_json TEXT,
                diagnostics_json TEXT,
                published_fence INTEGER,
                PRIMARY KEY (environment_key, agent, session_id)
             );
             CREATE TABLE burn_check_assessment (
                environment_key TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_id TEXT NOT NULL,
                check_id TEXT NOT NULL,
                incarnation INTEGER NOT NULL,
                boundary_generation INTEGER,
                boundary_activity_cursor TEXT,
                boundary_at_epoch INTEGER,
                input_revision TEXT,
                evaluator_revision TEXT,
                source_generation INTEGER,
                source_fingerprint TEXT,
                published_fence INTEGER,
                status TEXT NOT NULL,
                progress_json TEXT NOT NULL,
                result_json TEXT,
                result_revision TEXT,
                request_count INTEGER NOT NULL,
                created_at_epoch INTEGER NOT NULL,
                updated_at_epoch INTEGER NOT NULL,
                last_error_category TEXT,
                PRIMARY KEY (environment_key, agent, session_id, check_id)
             );
             INSERT INTO session (environment_key, agent, session_id)
             VALUES ('native', 'pi', 'argument-session');",
        )
        .unwrap();
}
