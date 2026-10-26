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

    let mut command = default_lookup_command(temp.path(), &data_root);
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

    let mut command = default_lookup_command(temp.path(), &data_root);
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

    let mut repeated_command = default_lookup_command(temp.path(), &data_root);
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
fn checks_command_reports_a_new_priced_session_outlier_once() {
    let temp = tempdir().unwrap();
    let data_root = platform_data_root(temp.path());
    let database_dir = data_root.join("ai.antiburn.desktop");
    std::fs::create_dir_all(&database_dir).unwrap();
    let database_path = database_dir.join("antiburn.sqlite3");
    create_fixture_database(&database_path);
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute(
            "UPDATE session SET updated_at_epoch = 1_000_000
              WHERE session_id = 'argument-session'",
            [],
        )
        .unwrap();
    insert_price_analysis(&connection, "native", "pi", "argument-session", 4_000_000);
    for index in 0_i64..7 {
        let session_id = format!("neighbor-{index:02}");
        connection
            .execute(
                "INSERT INTO session (
                    environment_key, agent, session_id, updated_at_epoch
                 ) VALUES ('native', 'pi', ?1, ?2)",
                rusqlite::params![session_id, 1_000_001 + index],
            )
            .unwrap();
        insert_price_analysis(&connection, "native", "pi", &session_id, 1_000_000);
    }
    drop(connection);
    std::fs::write(
        database_dir.join("model-pricing.json"),
        json!({
            "schema": 2,
            "source": "models.dev",
            "version": "2026-10-16",
            "fetched_at": "2026-10-16T00:00:00Z",
            "etag": null,
            "models": {
                "test-model": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.0,
                    "cache_read_cost_per_token": 0.0,
                    "cache_write_cost_per_token": 0.0
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let mut command = default_lookup_command(temp.path(), &data_root);
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
    assert_eq!(response["checks"].as_array().unwrap().len(), 1);
    assert_eq!(response["checks"][0]["source"], "session_price_outlier");
    assert_eq!(response["checks"][0]["check_id"], "outlier_priced_session");
    assert_eq!(response["checks"][0]["status"], "finding");
    assert_eq!(response["checks"][0]["result"]["estimated_cost_usd"], 4.0);
    assert_eq!(response["checks"][0]["result"]["threshold_usd"], 3.0);

    let mut repeated_command = default_lookup_command(temp.path(), &data_root);
    let repeated = repeated_command
        .args(["checks", "argument-session"])
        .output()
        .unwrap();
    assert!(repeated.status.success());
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
            "busy_timeout_ms": 12000,
            "state_file": temp.path().join("cli-state.json")
        })
        .to_string(),
    )
    .unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_antiburn-cli"));
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

fn default_lookup_command(home: &std::path::Path, data_root: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_antiburn-cli"));
    configure_data_directory(&mut command, home, data_root);
    #[cfg(target_os = "windows")]
    {
        // Windows Known Folders do not follow the APPDATA environment override.
        let config_path = home.join("fixture-config.json");
        std::fs::write(
            &config_path,
            json!({
                "data_root": data_root,
                "state_file": home.join("cli-state.json")
            })
            .to_string(),
        )
        .unwrap();
        command.arg("--config").arg(config_path);
    }
    command
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

fn insert_price_analysis(
    connection: &Connection,
    environment: &str,
    agent: &str,
    session_id: &str,
    input_tokens: u64,
) {
    let breakdown = json!({
        "test-model": {
            "input_tokens": input_tokens,
            "output_tokens": 0,
            "cache_read_tokens": 0,
            "cache_creation_tokens": 0,
            "cache_creation_1h_tokens": 0
        }
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO session_analysis (
                environment_key, agent, session_id, model_breakdown_json,
                pricing_breakdown_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![environment, agent, session_id, breakdown, breakdown],
        )
        .unwrap();
}

fn create_fixture_database(path: &std::path::Path) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (
                environment_key TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_id TEXT NOT NULL,
                updated_at_epoch INTEGER,
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
             CREATE TABLE session_analysis (
                environment_key TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_id TEXT NOT NULL,
                model_breakdown_json TEXT NOT NULL DEFAULT '{}',
                pricing_breakdown_json TEXT NOT NULL DEFAULT '{}',
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

fn delivery_fixture(
    root: &std::path::Path,
) -> (Connection, std::path::PathBuf, std::path::PathBuf) {
    let data_root = platform_data_root(root);
    let database_dir = data_root.join("ai.antiburn.desktop");
    std::fs::create_dir_all(&database_dir).unwrap();
    let database_path = database_dir.join("antiburn.sqlite3");
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute_batch(include_str!(
            "../../../crates/antiburn-checks-store/tests/fixtures/session.sql"
        ))
        .unwrap();
    connection
        .execute(
            "UPDATE native_file_delivery SET parser_revision = ?1",
            [antiburn_local::analysis::PARSER_REVISION],
        )
        .unwrap();
    let scope = r#"{"environment":"native","session_cwd":"/repo","run":"Live"}"#;
    connection
        .execute(
            "INSERT INTO burn_check_activation VALUES ('retry-loop',?1,2,1,'revision')",
            [scope],
        )
        .unwrap();
    connection.execute("INSERT INTO burn_check_runtime_definition VALUES ('retry-loop',?1,2,'revision','Retry loop','available')", [scope]).unwrap();
    connection.execute("INSERT INTO burn_check_current_status VALUES ('retry-loop',?1,2,'[\"native\",\"claude-code\",\"s1\"]','failure','a1',1)", [scope]).unwrap();
    connection.execute("INSERT INTO burn_check_assessment_history VALUES ('a1',2,'assessed',1,?1,123)", [r#"{"outcomes":[{"outcome":{"Applied":{"result":"Failure","message":"Try another approach."}}}]}"#]).unwrap();
    let config_path = root.join("delivery-config.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec(
            &json!({"data_root":data_root,"state_file":root.join("cli-state.json")}),
        )
        .unwrap(),
    )
    .unwrap();
    (connection, database_path, config_path)
}

fn delivery_call(
    root: &std::path::Path,
    config: &std::path::Path,
    consumer: &str,
    receipt: Option<&str>,
) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_antiburn-cli"));
    configure_data_directory(&mut command, root, &platform_data_root(root));
    command.arg("--config").arg(config).args([
        "checks",
        "s1",
        "--delivery-v1",
        "--consumer",
        consumer,
    ]);
    if let Some(receipt) = receipt {
        command.args(["--ack", receipt]);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cursor_desktop_delivery_reports_ready_and_excludes_other_sources() {
    let temp = tempdir().unwrap();
    let (connection, _, config) = delivery_fixture(temp.path());
    connection.execute("UPDATE session SET agent = 'cursor', source_kind = 'inline', source_label = 'cursor-desktop:workspace:s1'", []).unwrap();
    connection
        .execute(
            "INSERT INTO session_evidence VALUES ('native','cursor','s1','ready',1,?1,?2,?3)",
            rusqlite::params![
                antiburn_local::analysis::PARSER_REVISION,
                antiburn_local::analysis::ANALYZER_REVISION,
                antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION
            ],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE burn_check_current_status SET branch_key = '[\"native\",\"cursor\",\"s1\"]'",
            [],
        )
        .unwrap();
    let response = delivery_call(temp.path(), &config, "cursor:context", None);
    assert_eq!(response["state"], "ready");
    assert_eq!(response["snapshot"][0]["status"], "failure");
    assert_eq!(response["updates"][0]["kind"], "finding");
    connection
        .execute("UPDATE session SET source_label = 'cursor-cli:s1'", [])
        .unwrap();
    let response = delivery_call(temp.path(), &config, "cursor:status", None);
    assert_eq!(response["state"], "unsupported");
    assert_eq!(response["snapshot"], json!([]));
}

#[test]
fn delivery_command_acknowledges_independent_consumers_without_database_writes() {
    let temp = tempdir().unwrap();
    let (connection, database, config) = delivery_fixture(temp.path());
    let before = std::fs::read(&database).unwrap();
    let status = delivery_call(temp.path(), &config, "claude-code:status", None);
    assert_eq!(status["state"], "ready");
    assert_eq!(status["updates"], json!([]));
    assert!(status["receipt"].is_null());
    assert!(!temp.path().join("cli-state.json.delivery-v1").exists());
    let first = delivery_call(temp.path(), &config, "claude-code:context", None);
    assert_eq!(first["protocol_version"], 1);
    assert_eq!(first["session_scope"], "main");
    assert_eq!(first["freshness_evaluated"], false);
    assert_eq!(first["updates"][0]["kind"], "finding");
    assert_eq!(first["updates"][0]["message"], "Try another approach.");
    assert!(first.get("evidence").is_none());
    let repeated = delivery_call(temp.path(), &config, "claude-code:context", None);
    assert_eq!(first["receipt"], repeated["receipt"]);
    let ack = delivery_call(
        temp.path(),
        &config,
        "claude-code:context",
        first["receipt"].as_str(),
    );
    assert_eq!(ack["acknowledged"], true);
    assert_eq!(
        delivery_call(temp.path(), &config, "claude-code:context", None)["updates"],
        json!([])
    );
    assert_eq!(
        delivery_call(temp.path(), &config, "claude-code:ui", None)["updates"][0]["kind"],
        "finding"
    );
    assert_eq!(std::fs::read(&database).unwrap(), before);
    connection
        .execute(
            "UPDATE burn_check_current_status SET status = 'success'",
            [],
        )
        .unwrap();
    connection.execute("UPDATE burn_check_assessment_history SET actions_json = ?1", [r#"{"outcomes":[{"outcome":{"Applied":{"result":"Success","message":"Assessment cleared."}}}]}"#]).unwrap();
    let clear = delivery_call(temp.path(), &config, "claude-code:context", None);
    assert_eq!(clear["updates"][0]["kind"], "cleared");
    delivery_call(
        temp.path(),
        &config,
        "claude-code:context",
        clear["receipt"].as_str(),
    );
    assert_eq!(
        delivery_call(temp.path(), &config, "claude-code:context", None)["updates"],
        json!([])
    );
}

#[test]
fn delivery_rejects_stale_definitions_and_reports_incompatible_schema() {
    let temp = tempdir().unwrap();
    let (connection, _, config) = delivery_fixture(temp.path());
    connection
        .execute(
            "UPDATE burn_check_runtime_definition_state SET initialized = 0",
            [],
        )
        .unwrap();
    let initial = delivery_call(temp.path(), &config, "claude-code:context", None);
    assert_eq!(initial["state"], "waiting");
    assert!(initial["receipt"].is_null());
    connection
        .execute(
            "UPDATE burn_check_runtime_definition_state SET initialized = 1",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE burn_check_runtime_definition SET definition_revision = 'obsolete'",
            [],
        )
        .unwrap();
    let obsolete = delivery_call(temp.path(), &config, "claude-code:context", None);
    assert_eq!(obsolete["updates"], json!([]));
    assert_eq!(obsolete["snapshot"], json!([]));
    connection
        .execute("DROP TABLE burn_check_runtime_definition", [])
        .unwrap();
    let incompatible = delivery_call(temp.path(), &config, "claude-code:context", None);
    assert_eq!(incompatible["state"], "incompatible");
    assert!(incompatible["receipt"].is_null());
}

#[test]
fn ordinary_yaml_reads_do_not_consume_native_delivery() {
    let temp = tempdir().unwrap();
    let (connection, _, config) = delivery_fixture(temp.path());
    connection
        .execute_batch(
            "ALTER TABLE session_evidence ADD COLUMN processed_fingerprint TEXT;
        ALTER TABLE session_evidence ADD COLUMN evidence_json TEXT;
        ALTER TABLE session_evidence ADD COLUMN diagnostics_json TEXT;
        ALTER TABLE session_evidence ADD COLUMN published_fence INTEGER;
        CREATE TABLE burn_check_assessment AS SELECT
            NULL AS environment_key, NULL AS agent, NULL AS session_id, NULL AS check_id,
            NULL AS status, NULL AS input_revision, NULL AS evaluator_revision,
            NULL AS result_revision, NULL AS incarnation, NULL AS boundary_generation,
            NULL AS boundary_activity_cursor, NULL AS boundary_at_epoch, NULL AS source_generation,
            NULL AS source_fingerprint, NULL AS published_fence, NULL AS request_count,
            NULL AS created_at_epoch, NULL AS updated_at_epoch, NULL AS last_error_category,
            NULL AS progress_json, NULL AS result_json WHERE 0;",
        )
        .unwrap();
    let ordinary = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_antiburn-cli"));
        configure_data_directory(&mut command, temp.path(), &platform_data_root(temp.path()));
        let output = command
            .arg("--config")
            .arg(&config)
            .args(["checks", "s1"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let first = ordinary();
    assert_eq!(first["checks"][0]["source"], "predefined_yaml");
    assert_eq!(
        first["checks"][0]["result"]["message"],
        "Try another approach."
    );
    assert_eq!(ordinary()["checks"], json!([]));
    let delivery = delivery_call(temp.path(), &config, "claude-code:context", None);
    assert_eq!(delivery["updates"][0]["kind"], "finding");
}

#[test]
fn saved_ready_findings_remain_deliverable_during_reassessment() {
    let temp = tempdir().unwrap();
    let (connection, _, config) = delivery_fixture(temp.path());
    connection.execute("INSERT INTO burn_check_candidate SELECT check_id, activation_scope_key, generation, '[\"native\",\"claude-code\",\"s1\"]', 'pending' FROM burn_check_activation", []).unwrap();
    let first = delivery_call(temp.path(), &config, "claude-code:context", None);
    assert_eq!(first["state"], "ready");
    assert_eq!(first["snapshot"][0]["eligibility"], "ready");
    assert_eq!(first["snapshot"][0]["checking"], true);
    assert_eq!(first["freshness_evaluated"], false);
    assert_eq!(first["updates"][0]["kind"], "finding");
    delivery_call(
        temp.path(),
        &config,
        "claude-code:context",
        first["receipt"].as_str(),
    );
    connection
        .execute(
            "UPDATE burn_check_assessment_history SET accepted_at_epoch = 456",
            [],
        )
        .unwrap();
    let repeated = delivery_call(temp.path(), &config, "claude-code:context", None);
    assert_eq!(repeated["snapshot"][0]["assessed_at_epoch"], 456);
    assert_eq!(repeated["updates"], json!([]));
}

#[test]
fn source_invalidation_withdraws_advice_and_recovery_retriggers_it() {
    let temp = tempdir().unwrap();
    let (connection, _, config) = delivery_fixture(temp.path());
    let first = delivery_call(temp.path(), &config, "claude-code:context", None);
    delivery_call(
        temp.path(),
        &config,
        "claude-code:context",
        first["receipt"].as_str(),
    );
    connection
        .execute("UPDATE native_file_delivery SET blocked = 1", [])
        .unwrap();
    let unavailable = delivery_call(temp.path(), &config, "claude-code:context", None);
    assert!(unavailable["snapshot"][0]["message"].is_null());
    assert!(unavailable["snapshot"][0]["status"].is_null());
    assert_eq!(unavailable["updates"][0]["kind"], "invalidated");
    delivery_call(
        temp.path(),
        &config,
        "claude-code:context",
        unavailable["receipt"].as_str(),
    );
    connection
        .execute("UPDATE native_file_delivery SET blocked = 0", [])
        .unwrap();
    let recovered = delivery_call(temp.path(), &config, "claude-code:context", None);
    assert_eq!(recovered["updates"][0]["kind"], "finding");
    assert_eq!(recovered["updates"][0]["message"], "Try another approach.");
}

#[test]
fn delivery_at_capacity_reclaims_legacy_state_without_database_writes() {
    let temp = tempdir().unwrap();
    let (connection, database, config) = delivery_fixture(temp.path());
    drop(connection);
    let before = std::fs::read(&database).unwrap();
    let consumers: serde_json::Map<String, Value> = (0..1024)
        .map(|index| {
            (
                format!("legacy-{index}"),
                json!({
                    "acknowledged": {}, "exposed": {}, "pending": null, "last_receipt": null
                }),
            )
        })
        .collect();
    std::fs::write(
        temp.path().join("cli-state.json.delivery-v1"),
        serde_json::to_vec(&json!({"version": 1, "consumers": consumers})).unwrap(),
    )
    .unwrap();
    let first = delivery_call(temp.path(), &config, "claude-code:context", None);
    assert_eq!(first["updates"].as_array().unwrap().len(), 1);
    let receipt = first["receipt"].as_str().unwrap();
    delivery_call(temp.path(), &config, "claude-code:context", Some(receipt));
    let repeated = delivery_call(temp.path(), &config, "claude-code:context", None);
    assert!(repeated["updates"].as_array().unwrap().is_empty());
    assert_eq!(std::fs::read(database).unwrap(), before);
}

#[test]
fn capabilities_is_database_free_and_has_no_profile_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_antiburn-cli"))
        .args(["--config", "missing-config.json", "capabilities", "--json"])
        .env("HOME", temp.path())
        .env("USERPROFILE", temp.path())
        .env("XDG_DATA_HOME", temp.path())
        .env("XDG_STATE_HOME", temp.path())
        .env("APPDATA", temp.path())
        .env("LOCALAPPDATA", temp.path())
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        value["delivery_protocols"],
        serde_json::json!(["delivery-v1"])
    );
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}
