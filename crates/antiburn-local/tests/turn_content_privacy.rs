//! Turn-content capture must never leak transcript text into any other
//! metrics or summary projection. Content lives in `turn_content` and explicitly
//! selected check inputs. `NormalizedSession`,
//! `SessionEvidence` (diagnostics included), `SessionMetrics`, and every
//! other table in the schema must never carry it. Deleting a session's turn
//! rows must remove it completely.
//!
//! This file carries fixtures for every characterized source that stores
//! content: Claude, Codex, OpenCode, Pi, Cursor, and Antigravity. The generic
//! JSONL fallback does not emit `TurnContent` records.

use std::sync::Arc;

use antiburn_local::analysis::ignored_instructions::{
    INPUT_SELECTION, prepare_session_content, select_session_content,
};
use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, FenceScope, MemoryTurnRowStore, RawSource,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SourceCapabilities,
    SourceFormat, SourceKind, TurnRowSink, TurnRowStore, TurnSessionKey, delete_turn_rows,
    normalize_source, query_turn_content_offset_selected, reader_for,
};
use rusqlite::Connection;
use rusqlite::types::Value as SqlValue;

/// Every `(table, column, text)` value across a connection's tables.
///
/// Reads the table list from `sqlite_master` instead of a fixed list. This
/// sweeps a table this test does not yet know about, and so catches a
/// future regression in a table that does not exist today.
fn all_text_and_blob_values(connection: &Connection) -> Vec<(String, String, String)> {
    let mut found = Vec::new();
    let table_names: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .expect("prepare table list")
        .query_map([], |row| row.get(0))
        .expect("query table list")
        .collect::<Result<_, _>>()
        .expect("collect table names");

    for table in table_names {
        let columns: Vec<String> = connection
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .expect("prepare table info")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query table info")
            .collect::<Result<_, _>>()
            .expect("collect column names");
        let select_list = columns
            .iter()
            .map(|column| format!("\"{column}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = connection
            .prepare(&format!("SELECT {select_list} FROM \"{table}\""))
            .expect("prepare row scan");
        let mut rows = statement.query([]).expect("query rows");
        while let Some(row) = rows.next().expect("advance row") {
            for (index, column) in columns.iter().enumerate() {
                let value: SqlValue = row.get(index).expect("read column value");
                let text = match value {
                    SqlValue::Text(text) => Some(text),
                    SqlValue::Blob(blob) => String::from_utf8(blob).ok(),
                    _ => None,
                };
                if let Some(text) = text {
                    found.push((table.clone(), column.clone(), text));
                }
            }
        }
    }
    found
}

/// Fails the test with the offending table and column if `sentinel` appears
/// anywhere except `turn_content`.
fn assert_confined_to_turn_content(connection: &Connection, sentinel: &str) {
    for (table, column, text) in all_text_and_blob_values(connection) {
        if table == "turn_content" {
            continue;
        }
        assert!(
            !text.contains(sentinel),
            "{sentinel} leaked into {table}.{column}: {text}"
        );
    }
}

/// Fails the test with the offending table and column if `sentinel` appears
/// anywhere at all. Use this after a delete, when `turn_content` must be
/// empty of it too.
fn assert_absent_everywhere(connection: &Connection, sentinel: &str) {
    for (table, column, text) in all_text_and_blob_values(connection) {
        assert!(
            !text.contains(sentinel),
            "{sentinel} survived deletion in {table}.{column}: {text}"
        );
    }
}

/// Runs one fixture through the real metrics, evidence, and turn-row
/// pipeline, exactly as the durable analysis worker does, and returns every
/// serialized projection plus the row store for direct inspection.
struct PrivacyRun {
    normalized_json: String,
    evidence_json: String,
    metrics_json: String,
    store: Arc<MemoryTurnRowStore>,
    source_format: SourceFormat,
}

fn run_pipeline(
    agent: &str,
    session_id: &str,
    source: RawSource,
    capabilities: SourceCapabilities,
) -> PrivacyRun {
    let source_format = match (agent, &source) {
        ("claude", _) => SourceFormat::ClaudeJsonl,
        ("codex", _) => SourceFormat::CodexRolloutJsonl,
        ("opencode", RawSource::Sqlite(_)) => SourceFormat::OpenCodeSqliteV2,
        ("opencode", _) => SourceFormat::OpenCodeJsonl,
        ("pi", _) => SourceFormat::PiV3Jsonl,
        ("cursor", _) => SourceFormat::CursorCliAgentJsonl,
        ("antigravity", _) => SourceFormat::AntigravityBrainJsonl,
        _ => SourceFormat::Uncharacterized,
    };
    let input = SessionInput {
        agent: agent.to_string(),
        session_id: session_id.to_string(),
        source,
        fork_parent_session_id: None,
        source_format,
    };

    // The normalized model never carries message text.
    let normalized_session = normalize_source(&input).expect("fixture must normalize");
    let normalized_json = serde_json::to_string(&normalized_session).expect("serialize session");

    let store = MemoryTurnRowStore::new(agent, session_id);
    let metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities,
    });
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        None,
    );
    let mut composite = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = reader_for(agent)
        .visit(&input, &mut composite)
        .unwrap_or_else(|error| panic!("{agent} adapter must visit its own fixture: {error}"));
    composite.observe_source_outcome(outcome);
    assert!(
        !composite.turn_row_write_failed(),
        "{agent} turn row write must not fail"
    );

    let evidence = composite.evidence().expect("evidence must publish");
    let evidence_json = serde_json::to_string(&evidence).expect("serialize evidence");
    let metrics = composite.metrics().expect("metrics must publish");
    let metrics_json = serde_json::to_string(&metrics).expect("serialize metrics");

    PrivacyRun {
        normalized_json,
        evidence_json,
        metrics_json,
        store,
        source_format: input.source_format,
    }
}

fn turn_key(agent: &'static str, session_id: &'static str) -> TurnSessionKey<'static> {
    TurnSessionKey {
        environment_key: "native",
        agent,
        session_id,
    }
}

/// Runs the full containment and deletion check shared by every vendor.
///
/// Every `sentinel` must land in `turn_content` and nowhere else. It must
/// not appear in the normalized session, evidence, or metrics JSON. It must
/// vanish from the whole database once `delete_turn_rows` runs. This is the
/// same function `Store::delete_session` and `Store::clear_local_session_data`
/// call — see `apps/desktop/src-tauri/src/store/mod.rs`.
fn assert_vendor_privacy(
    agent: &'static str,
    session_id: &'static str,
    source: RawSource,
    capabilities: SourceCapabilities,
    sentinels: &[&str],
) {
    let run = run_pipeline(agent, session_id, source, capabilities);

    for sentinel in sentinels {
        assert!(
            !run.normalized_json.contains(sentinel),
            "{agent} NormalizedSession leaked {sentinel}"
        );
        assert!(
            !run.evidence_json.contains(sentinel),
            "{agent} SessionEvidence (including diagnostics) leaked {sentinel}"
        );
        assert!(
            !run.metrics_json.contains(sentinel),
            "{agent} SessionMetrics leaked {sentinel}"
        );
    }

    run.store.with_connection(|connection| {
        // Every sentinel reached `turn_content`, proving the fixture
        // exercised the content path, and nowhere else in the schema.
        let stored: Vec<String> = connection
            .prepare("SELECT content FROM turn_content")
            .expect("prepare")
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .expect("query")
            .map(|blob| String::from_utf8(blob.expect("row content is valid UTF-8")).unwrap())
            .collect();
        let all_content = stored.join("\n");
        for sentinel in sentinels {
            assert!(
                all_content.contains(sentinel),
                "{agent} turn_content is missing {sentinel}"
            );
            assert_confined_to_turn_content(connection, sentinel);
        }
    });

    if antiburn_local::analysis::ignored_instructions::source_supported(run.source_format) {
        let key = turn_key(agent, session_id);
        let published = run.store.with_connection(|connection| {
            query_turn_content_offset_selected(
                connection,
                &key,
                &FenceScope::single(1),
                None,
                &Default::default(),
                0,
                INPUT_SELECTION,
            )
            .expect("query selected source content")
        });
        let content = prepare_session_content(session_id, run.source_format, published, Vec::new());
        let selected = select_session_content(&content, INPUT_SELECTION);
        assert!(
            !selected.actions.is_empty(),
            "{agent} selected content is non-empty"
        );
        assert!(selected.actions.iter().all(|action| {
            action.kind != "thinking"
                && (action.kind != "tool_result"
                    || (action.authority == "tool" && action.metadata.human_text.is_none()))
        }));
        assert!(selected.field_availability.iter().any(|field| field.field
            == antiburn_local::analysis::JevInputField::UserMessage
            && field.selected));
        assert!(selected.field_availability.iter().any(|field| field.field
            == antiburn_local::analysis::JevInputField::BashCommandOutput
            && field.selected));
        for field in &selected.field_availability {
            assert_eq!(field.selected, INPUT_SELECTION.includes(field.field));
        }
        let selected_json = serde_json::to_string(&selected).unwrap();
        for sentinel in sentinels.iter().filter(|sentinel| {
            sentinel.contains("thinking") || **sentinel == "private reasoning stays local"
        }) {
            assert!(
                !selected_json.contains(sentinel),
                "{agent} selected thinking: {sentinel}"
            );
        }
        assert!(selected.field_availability.iter().all(|field| {
            !field.selected
                || field.capability != antiburn_local::analysis::JevFieldCapability::Unavailable
        }));
    }

    // Deleting the session's turn rows — `delete_turn_rows`, the function
    // both `Store::delete_session` and `Store::clear_local_session_data`
    // call — removes every sentinel from the database. The store runs
    // SQLite in WAL mode in production, so a deleted row's bytes can still
    // sit in the WAL file; this in-memory connection has no WAL file at
    // all, so the assertion below is the right level for this crate — the
    // SQL-visible state, not the bytes on disk.
    run.store.with_connection(|connection| {
        delete_turn_rows(connection, &turn_key(agent, session_id)).expect("delete turn rows");
        for sentinel in sentinels {
            assert_absent_everywhere(connection, sentinel);
        }
    });
}

/// A small Claude transcript carrying one sentinel per captured content
/// kind: a user prompt, an assistant text block, a thinking block, a tool
/// call's input, and a tool result.
fn claude_fixture() -> String {
    let user = serde_json::json!({
        "type": "user",
        "timestamp": "2026-01-01T00:00:00Z",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": format!("{CLAUDE_USER} please investigate")}],
        }
    })
    .to_string();
    let assistant = serde_json::json!({
        "type": "assistant",
        "timestamp": "2026-01-01T00:00:01Z",
        "message": {
            "id": "msg-1",
            "role": "assistant",
            "model": "claude-opus-4-6",
            "usage": {"input_tokens": 10, "output_tokens": 5},
            "content": [
                {"type": "text", "text": format!("{CLAUDE_ASSISTANT} responding")},
                {"type": "thinking", "thinking": format!("{CLAUDE_THINK} pondering")},
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": {"command": format!("echo {CLAUDE_TOOLIN}")},
                },
            ],
        }
    })
    .to_string();
    let tool_result = serde_json::json!({
        "type": "user",
        "timestamp": "2026-01-01T00:00:02Z",
        "message": {
            "role": "user",
            "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": format!("{CLAUDE_RESULT} done")},
            ],
        }
    })
    .to_string();
    // Claude also reads a `skill_listing` attachment — one line per skill,
    // `- name: description` — into the evidence `context_sources.skills`
    // catalog, not into `turn_content`. That catalog is deliberately
    // surfaced metadata (see the `store` module's doc comment: "the current
    // schema stores ... capped skill descriptions"), not raw transcript
    // content, so this fixture's own assertions expect its sentinel in
    // `SessionEvidence`, unlike the other four.
    let skill_listing = serde_json::json!({
        "type": "system",
        "timestamp": "2026-01-01T00:00:03Z",
        "attachment": {
            "type": "skill_listing",
            "content": format!("- verify: {CLAUDE_SKILL} checks synthetic output."),
        }
    })
    .to_string();
    format!("{user}\n{assistant}\n{tool_result}\n{skill_listing}\n")
}

const CLAUDE_USER: &str = "PRIVACY-SENTINEL-claude-user-7f3a";
const CLAUDE_ASSISTANT: &str = "PRIVACY-SENTINEL-claude-assistant-2b6c";
const CLAUDE_THINK: &str = "PRIVACY-SENTINEL-claude-thinking-9c2b";
const CLAUDE_TOOLIN: &str = "PRIVACY-SENTINEL-claude-toolin-3d1e";
const CLAUDE_RESULT: &str = "PRIVACY-SENTINEL-claude-result-88aa";
const CLAUDE_SKILL: &str = "PRIVACY-SENTINEL-claude-skill-5f10";

#[test]
fn claude_turn_content_captures_sentinels_while_every_other_table_and_projection_stays_clean() {
    assert_vendor_privacy(
        "claude",
        "content-privacy-claude",
        RawSource::Jsonl(claude_fixture()),
        SourceCapabilities::claude(),
        &[
            CLAUDE_USER,
            CLAUDE_ASSISTANT,
            CLAUDE_THINK,
            CLAUDE_TOOLIN,
            CLAUDE_RESULT,
        ],
    );
}

/// The `skill_listing` sentinel is a documented exception.
///
/// It must reach `SessionEvidence`'s `context_sources.skills` catalog —
/// capped, surfaced metadata — not `turn_content`. This dedicated run checks
/// that boundary directly. Folding it into the confinement sweep above
/// would flag this sentinel's legitimate appearance in evidence JSON as a
/// leak.
#[test]
fn claude_skill_listing_descriptions_surface_in_evidence_not_turn_content() {
    let run = run_pipeline(
        "claude",
        "content-privacy-claude-skill",
        RawSource::Jsonl(claude_fixture()),
        SourceCapabilities::claude(),
    );
    assert!(
        run.evidence_json.contains(CLAUDE_SKILL),
        "the skill catalog's description is meant to reach SessionEvidence"
    );
    run.store.with_connection(|connection| {
        let stored: Vec<String> = connection
            .prepare("SELECT content FROM turn_content")
            .expect("prepare")
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .expect("query")
            .map(|blob| String::from_utf8(blob.expect("row content is valid UTF-8")).unwrap())
            .collect();
        assert!(
            !stored.join("\n").contains(CLAUDE_SKILL),
            "a skill catalog description is not transcript content and must not reach turn_content"
        );
    });
}

/// A Codex transcript carrying one sentinel per captured content kind.
///
/// The `developer`-role message also stands in for Codex's skill catalog
/// position. Codex writes it as an ordinary instruction message, so it
/// captures as user-side text in `turn_content`, like any other message.
/// Unlike Claude's dedicated `skill_listing` attachment, Codex has no
/// separate evidence-only skill surface.
fn codex_fixture() -> String {
    let lines = [
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:00Z", "type": "session_meta",
            "payload": {"id": "content-privacy-codex", "timestamp": "2026-08-01T09:59:58Z", "cwd": "/home/avery/demo", "cli_version": "0.0.0-test", "source": "cli"}
        }),
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:01Z", "type": "turn_context",
            "payload": {"model": "gpt-test", "effort": "medium"}
        }),
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:02Z", "type": "response_item",
            "payload": {"type": "message", "role": "developer", "content": [
                {"type": "input_text", "text": format!("## Skills\n- verify: {CODEX_SKILL} checks synthetic output.")}
            ]}
        }),
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:03Z", "type": "response_item",
            "payload": {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": format!("{CODEX_USER} please investigate")}
            ]}
        }),
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:04Z", "type": "response_item",
            "payload": {"type": "reasoning", "summary": [{"text": format!("{CODEX_THINK} pondering")}]}
        }),
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:05Z", "type": "response_item",
            "payload": {"type": "message", "role": "assistant", "content": [
                {"type": "output_text", "text": format!("{CODEX_ASSISTANT} responding")}
            ]}
        }),
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:06Z", "type": "response_item",
            "payload": {"type": "function_call", "name": "exec_command", "arguments": format!("{{\"cmd\":\"echo {CODEX_TOOLIN}\"}}"), "call_id": "call-1"}
        }),
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:07Z", "type": "response_item",
            "payload": {"type": "function_call_output", "call_id": "call-1", "output": format!("{CODEX_RESULT} done")}
        }),
    ];
    lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

const CODEX_USER: &str = "PRIVACY-SENTINEL-codex-user-4a11";
const CODEX_ASSISTANT: &str = "PRIVACY-SENTINEL-codex-assistant-6b22";
const CODEX_THINK: &str = "PRIVACY-SENTINEL-codex-thinking-8c33";
const CODEX_TOOLIN: &str = "PRIVACY-SENTINEL-codex-toolin-1d44";
const CODEX_RESULT: &str = "PRIVACY-SENTINEL-codex-result-9e55";
const CODEX_SKILL: &str = "PRIVACY-SENTINEL-codex-skill-2f66";

#[test]
fn codex_turn_content_captures_sentinels_while_every_other_table_and_projection_stays_clean() {
    assert_vendor_privacy(
        "codex",
        "content-privacy-codex",
        RawSource::Jsonl(codex_fixture()),
        SourceCapabilities::codex(),
        &[
            CODEX_USER,
            CODEX_ASSISTANT,
            CODEX_THINK,
            CODEX_TOOLIN,
            CODEX_RESULT,
            CODEX_SKILL,
        ],
    );
}

/// An OpenCode export-stream transcript (the `RawSource::Jsonl` shape the
/// adapter also accepts, alongside its native SQLite export) carrying one
/// sentinel per captured content kind.
fn opencode_fixture() -> String {
    let lines = [
        serde_json::json!({
            "type": "session_meta", "sessionID": "content-privacy-opencode", "sessionRole": "root",
            "time": {"created": 1000}, "payload": {"id": "content-privacy-opencode", "title": "Fixture session"}
        }),
        serde_json::json!({
            "type": "message", "rootSessionID": "content-privacy-opencode", "sessionID": "content-privacy-opencode",
            "sessionRole": "root", "messageID": "m-user", "time": {"created": 1001},
            "payload": {"role": "user"}
        }),
        serde_json::json!({
            "type": "part", "messageID": "m-user", "time": {"created": 1001},
            "payload": {"type": "text", "text": format!("{OPENCODE_USER} please investigate")}
        }),
        serde_json::json!({
            "type": "message", "rootSessionID": "content-privacy-opencode", "sessionID": "content-privacy-opencode",
            "sessionRole": "root", "messageID": "m-assistant", "time": {"created": 1002},
            "payload": {"role": "assistant", "modelID": "model-a"}
        }),
        serde_json::json!({
            "type": "part", "messageID": "m-assistant", "time": {"created": 1002},
            "payload": {"type": "text", "text": format!("{OPENCODE_ASSISTANT} responding")}
        }),
        serde_json::json!({
            "type": "part", "messageID": "m-assistant", "time": {"created": 1002},
            "payload": {"type": "reasoning", "text": format!("{OPENCODE_THINK} pondering")}
        }),
        serde_json::json!({
            "type": "part", "messageID": "m-assistant", "time": {"created": 1002},
            "payload": {"type": "tool", "tool": "bash", "state": {
                "input": {"command": format!("echo {OPENCODE_TOOLIN}")},
                "output": format!("{OPENCODE_RESULT} done"),
            }}
        }),
    ];
    lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

const OPENCODE_USER: &str = "PRIVACY-SENTINEL-opencode-user-3g77";
const OPENCODE_ASSISTANT: &str = "PRIVACY-SENTINEL-opencode-assistant-4h88";
const OPENCODE_THINK: &str = "PRIVACY-SENTINEL-opencode-thinking-5i99";
const OPENCODE_TOOLIN: &str = "PRIVACY-SENTINEL-opencode-toolin-6j00";
const OPENCODE_RESULT: &str = "PRIVACY-SENTINEL-opencode-result-7k11";

#[test]
fn opencode_turn_content_captures_sentinels_while_every_other_table_and_projection_stays_clean() {
    assert_vendor_privacy(
        "opencode",
        "content-privacy-opencode",
        RawSource::Jsonl(opencode_fixture()),
        SourceCapabilities::opencode(),
        &[
            OPENCODE_USER,
            OPENCODE_ASSISTANT,
            OPENCODE_THINK,
            OPENCODE_TOOLIN,
            OPENCODE_RESULT,
        ],
    );
}

#[test]
fn opencode_sqlite_fields_reach_the_fenced_selected_input_path() {
    const TOOL: &str = "SQLITE-OPENCODE-BAASH-SENTINEL";
    const OUTPUT: &str = "SQLITE-OPENCODE-OUTPUT-SENTINEL";
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("opencode.db");
    let connection = Connection::open(&path).expect("OpenCode database");
    connection
        .execute_batch(
            "CREATE TABLE session (
                 id TEXT PRIMARY KEY, parent_id TEXT, title TEXT,
                 time_created INTEGER, time_updated INTEGER
             );
             CREATE TABLE message (
                 id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                 time_updated INTEGER, data TEXT
             );
             CREATE TABLE part (
                 id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
                 time_created INTEGER, time_updated INTEGER, data TEXT
             );",
        )
        .expect("create OpenCode tables");
    connection
        .execute(
            "INSERT INTO session VALUES ('content-privacy-opencode-sqlite', NULL, NULL, 1, 1)",
            [],
        )
        .expect("insert session");
    connection
        .execute(
            "INSERT INTO message VALUES ('m1', 'content-privacy-opencode-sqlite', 2, 2, ?1)",
            [r#"{"role":"assistant","modelID":"model-a"}"#],
        )
        .expect("insert message");
    connection
        .execute(
            "INSERT INTO part VALUES ('p1', 'm1', 'content-privacy-opencode-sqlite', 2, 2, ?1)",
            [serde_json::json!({"type":"tool","tool":"bash","state":{"input":{"command":format!("cargo test {TOOL}"),"description":"PRIVATE-DESCRIPTION"},"output":OUTPUT}}).to_string()],
        )
        .expect("insert tool part");
    drop(connection);

    assert_vendor_privacy(
        "opencode",
        "content-privacy-opencode-sqlite",
        RawSource::Sqlite(path),
        SourceCapabilities::opencode(),
        &[TOOL, OUTPUT],
    );
}

/// A Pi transcript carrying one sentinel per captured content kind.
fn pi_fixture() -> String {
    let lines = [
        serde_json::json!({
            "type": "session", "version": 3, "id": "content-privacy-pi",
            "timestamp": "2026-01-01T00:00:00Z", "cwd": "/synthetic/work"
        }),
        serde_json::json!({
            "type": "message", "id": "row-1", "parentId": null, "timestamp": "2026-01-01T00:00:01Z",
            "message": {"role": "user", "timestamp": 1000, "content": [
                {"type": "text", "text": format!("{PI_USER} please investigate")}
            ]}
        }),
        serde_json::json!({
            "type": "message", "id": "row-2", "parentId": "row-1", "timestamp": "2026-01-01T00:00:02Z",
            "message": {"role": "assistant", "timestamp": 1001, "model": "model-a", "content": [
                {"type": "text", "text": format!("{PI_ASSISTANT} responding")},
                {"type": "thinking", "thinking": format!("{PI_THINK} pondering")},
                {"type": "toolCall", "id": "call-1", "name": "bash", "arguments": {"command": format!("echo {PI_TOOLIN}")}},
            ]}
        }),
        serde_json::json!({
            "type": "message", "id": "row-3", "parentId": "row-2", "timestamp": "2026-01-01T00:00:03Z",
            "message": {"role": "toolResult", "toolCallId": "call-1", "toolName": "bash", "isError": false, "content": [
                {"type": "text", "text": format!("{PI_RESULT} done")}
            ]}
        }),
    ];
    lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

const PI_USER: &str = "PRIVACY-SENTINEL-pi-user-8l22";
const PI_ASSISTANT: &str = "PRIVACY-SENTINEL-pi-assistant-9m33";
const PI_THINK: &str = "PRIVACY-SENTINEL-pi-thinking-0n44";
const PI_TOOLIN: &str = "PRIVACY-SENTINEL-pi-toolin-1o55";
const PI_RESULT: &str = "PRIVACY-SENTINEL-pi-result-2p66";

#[test]
fn pi_turn_content_captures_sentinels_while_every_other_table_and_projection_stays_clean() {
    assert_vendor_privacy(
        "pi",
        "content-privacy-pi",
        RawSource::Jsonl(pi_fixture()),
        SourceCapabilities::pi(),
        &[PI_USER, PI_ASSISTANT, PI_THINK, PI_TOOLIN, PI_RESULT],
    );
}

#[test]
fn cursor_turn_content_is_confined_and_removed_after_session_deletion() {
    const USER: &str = "PRIVACY-SENTINEL-cursor-user-8q22";
    const ASSISTANT: &str = "PRIVACY-SENTINEL-cursor-assistant-9r33";
    const TOOL_INPUT: &str = "PRIVACY-SENTINEL-cursor-tool-input-0s44";
    const TOOL_RESULT: &str = "PRIVACY-SENTINEL-cursor-tool-result-1t55";
    let source = format!(
        "{{\"sessionId\":\"content-privacy-cursor\",\"cursor_source\":\"agent_transcript\"}}\n{{\"role\":\"user\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{USER}\"}}]}}}}\n{{\"role\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool-use\",\"name\":\"Shell\",\"input\":{{\"command\":\"{TOOL_INPUT}\"}}}},{{\"type\":\"text\",\"text\":\"{ASSISTANT}\"}}]}}}}\n{{\"role\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"tool_result\",\"content\":[{{\"type\":\"text\",\"text\":\"{TOOL_RESULT}\"}}]}}]}}}}\n"
    );
    assert_vendor_privacy(
        "cursor",
        "content-privacy-cursor",
        RawSource::Jsonl(source),
        SourceCapabilities::cursor(),
        &[USER, ASSISTANT, TOOL_INPUT, TOOL_RESULT],
    );
}

#[test]
fn antigravity_turn_content_captures_sentinels_while_every_other_table_and_projection_stays_clean()
{
    assert_vendor_privacy(
        "antigravity",
        "content-privacy-antigravity",
        RawSource::Jsonl(
            include_str!(
                "fixtures/antigravity_characterization/ignored_instructions_content.jsonl"
            )
            .to_owned(),
        ),
        SourceCapabilities::antigravity(),
        &[
            "ANTIGRAVITY-USER",
            "The focused tests passed.",
            "private reasoning stays local",
            "cargo test --test focused",
            "test result: ok",
        ],
    );
}

#[test]
fn native_field_sentinels_remain_isolated_in_fenced_queries_and_projection() {
    use antiburn_local::analysis::jev::{
        JevFieldAvailabilityState, JevInputField, JevInputSelection,
    };
    let source = include_str!("fixtures/claude_characterization/selection_isolation.jsonl");
    let run = run_pipeline(
        "claude",
        "selection-isolation",
        RawSource::Jsonl(source.to_owned()),
        SourceCapabilities::claude(),
    );
    let fields = [
        (JevInputField::UserMessage, "ISO_USER"),
        (JevInputField::AssistantMessage, "ISO_ASSISTANT"),
        (JevInputField::BashCommandInput, "ISO_BASH_INPUT"),
        (JevInputField::BashCommandOutput, "ISO_BASH_OUTPUT"),
        (JevInputField::FileEditPath, "ISO_EDIT_PATH"),
        (JevInputField::FileEditContent, "ISO_EDIT_CONTENT"),
        (JevInputField::ReadFilePath, "ISO_READ_PATH"),
        (JevInputField::ReadFileOutput, "ISO_READ_OUTPUT"),
        (JevInputField::SearchFilesQuery, "ISO_SEARCH_QUERY"),
        (JevInputField::SearchFilesOutput, "ISO_SEARCH_OUTPUT"),
        (JevInputField::OtherToolInput, "ISO_OTHER_INPUT"),
        (JevInputField::OtherToolOutput, "ISO_OTHER_OUTPUT"),
    ];
    run.store.with_connection(|connection| {
        for (field, sentinel) in fields {
            let selection = JevInputSelection::from_fields(&[field]);
            let published = query_turn_content_offset_selected(
                connection,
                &turn_key("claude", "selection-isolation"),
                &FenceScope::single(1),
                None,
                &Default::default(),
                0,
                selection,
            )
            .unwrap();
            assert_eq!(published.parts.len(), 1, "{field:?}");
            let selected = select_session_content(
                &prepare_session_content(
                    "selection-isolation",
                    run.source_format,
                    published,
                    Vec::new(),
                ),
                selection,
            );
            let serialized = serde_json::to_string(&selected).unwrap();
            for (_, candidate) in fields {
                assert_eq!(
                    serialized.contains(candidate),
                    candidate == sentinel,
                    "{field:?}: {candidate}"
                );
            }
            assert!(!serialized.contains("ISO_PRIVATE_THINKING"));
            let action = &selected.actions[0];
            if matches!(
                field,
                JevInputField::BashCommandInput
                    | JevInputField::FileEditPath
                    | JevInputField::FileEditContent
                    | JevInputField::ReadFilePath
                    | JevInputField::SearchFilesQuery
            ) {
                assert_eq!(action.metadata.bindings.len(), 1, "{field:?}");
                let binding = &action.metadata.bindings[0];
                assert_eq!(binding.field, field);
                let native: serde_json::Value =
                    serde_json::from_str(source.lines().nth(1).unwrap()).unwrap();
                let text = native.pointer(&binding.pointer).unwrap().as_str().unwrap();
                assert_eq!(text.get(binding.start..binding.end), Some(sentinel));
            } else {
                assert!(action.metadata.bindings.is_empty());
            }
            let availability = selected
                .field_availability
                .iter()
                .find(|value| value.field == field)
                .unwrap();
            assert_eq!(availability.state, JevFieldAvailabilityState::Observed);
            assert_eq!(availability.observed_parts, 1);
            assert_eq!(availability.malformed_parts, 0);
        }
    });
}

#[test]
fn native_empty_text_is_observed_but_non_text_results_remain_unavailable() {
    use antiburn_local::analysis::jev::{
        JevFieldAvailabilityState, JevInputField, JevInputSelection,
    };
    let mut source =
        include_str!("fixtures/claude_characterization/selection_isolation.jsonl").to_owned();
    for sentinel in [
        "ISO_USER",
        "ISO_ASSISTANT",
        "ISO_EDIT_CONTENT",
        "ISO_BASH_OUTPUT",
        "ISO_READ_OUTPUT",
        "ISO_SEARCH_OUTPUT",
    ] {
        source = source.replace(sentinel, "");
    }
    source = source.replace(
        "\"ISO_OTHER_OUTPUT\"",
        r#"[{"type":"image","text":"UNSUPPORTED_IMAGE_TEXT"}]"#,
    );
    let run = run_pipeline(
        "claude",
        "empty-native-fields",
        RawSource::Jsonl(source),
        SourceCapabilities::claude(),
    );
    run.store.with_connection(|connection| {
        for field in [
            JevInputField::UserMessage,
            JevInputField::AssistantMessage,
            JevInputField::FileEditContent,
            JevInputField::BashCommandOutput,
            JevInputField::ReadFileOutput,
            JevInputField::SearchFilesOutput,
            JevInputField::OtherToolOutput,
        ] {
            let selection = JevInputSelection::from_fields(&[field]);
            let published = query_turn_content_offset_selected(
                connection,
                &turn_key("claude", "empty-native-fields"),
                &FenceScope::single(1),
                None,
                &Default::default(),
                0,
                selection,
            )
            .unwrap();
            let selected = select_session_content(
                &prepare_session_content(
                    "empty-native-fields",
                    run.source_format,
                    published,
                    Vec::new(),
                ),
                selection,
            );
            let availability = selected
                .field_availability
                .iter()
                .find(|value| value.field == field)
                .unwrap();
            if field == JevInputField::OtherToolOutput {
                assert_eq!(availability.state, JevFieldAvailabilityState::NotObserved);
                assert!(selected.actions.is_empty());
            } else {
                assert_eq!(
                    availability.state,
                    JevFieldAvailabilityState::Observed,
                    "{field:?}"
                );
                assert_eq!(availability.empty_parts, 1, "{field:?}");
                assert_eq!(availability.malformed_parts, 0, "{field:?}");
            }
            assert!(
                !serde_json::to_string(&selected)
                    .unwrap()
                    .contains("UNSUPPORTED_IMAGE_TEXT")
            );
        }
    });
}

#[test]
fn conflicting_native_call_ids_do_not_supply_selected_result_names() {
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    let source = include_str!("fixtures/claude_characterization/selection_isolation.jsonl")
        .replace(r#""id":"other-1""#, r#""id":"bash-1""#);
    let run = run_pipeline(
        "claude",
        "conflicting-native-ids",
        RawSource::Jsonl(source),
        SourceCapabilities::claude(),
    );
    run.store.with_connection(|connection| {
        let published = query_turn_content_offset_selected(connection,
            &turn_key("claude", "conflicting-native-ids"), &FenceScope::single(1),
            None, &Default::default(), 0,
            JevInputSelection::from_fields(&[JevInputField::BashCommandOutput, JevInputField::OtherToolOutput])).unwrap();
        assert!(published.parts.is_empty());
        let raw: String = connection.query_row(
            "SELECT CAST(content AS TEXT) FROM turn_content WHERE tool_call_id = 'bash-1' AND kind = 'tool_result'",
            [], |row| row.get(0)).unwrap();
        assert_eq!(raw, "ISO_BASH_OUTPUT", "ambiguous output remains local");
    });
}

#[test]
fn pi_non_text_result_blocks_do_not_supply_selected_output() {
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    let mut records: Vec<serde_json::Value> = pi_fixture()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    records.last_mut().unwrap()["message"]["content"][0]["type"] = serde_json::json!("image");
    let source = records
        .iter()
        .map(serde_json::Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let run = run_pipeline(
        "pi",
        "pi-non-text-result",
        RawSource::Jsonl(source),
        SourceCapabilities::pi(),
    );
    run.store.with_connection(|connection| {
        let published = query_turn_content_offset_selected(
            connection,
            &turn_key("pi", "pi-non-text-result"),
            &FenceScope::single(1),
            None,
            &Default::default(),
            0,
            JevInputSelection::from_fields(&[JevInputField::BashCommandOutput]),
        )
        .unwrap();
        assert!(published.parts.is_empty());
    });
}

#[test]
fn equivalent_native_requests_have_the_same_selected_meaning_for_all_six_formats() {
    use antiburn_local::analysis::jev::JevInputField;
    use serde_json::{Value, json};

    let requests = [
        ("bash", json!({"command":"printf equivalent"})),
        (
            "edit",
            json!({"path":"src/equivalent.rs","oldText":"EXCLUDED_OLD","newText":"EXCLUDED_NEW"}),
        ),
        ("read", json!({"path":"src/reference.rs"})),
        (
            "grep",
            json!({"pattern":"equivalent","path":"src","include":"*.rs","matches":"EXCLUDED_MATCH"}),
        ),
        ("notify", json!({"message":"equivalent notification"})),
    ];
    for (agent, encoded) in [
        ("claude", false),
        ("codex", true),
        ("codex", false),
        ("opencode", false),
        ("pi", false),
        ("cursor", false),
        ("antigravity", false),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let mut records: Vec<Value> = Vec::new();
        match agent {
            "claude" => {
                let mut blocks =
                    vec![json!({"type":"text","text":"Equivalent assistant response."})];
                blocks.extend(requests.iter().enumerate().map(|(index, (name, arguments))| json!({"type":"tool_use","id":format!("call-{index}"),"name":name,"input":arguments})));
                records.push(json!({"type":"assistant","uuid":"assistant-record","message":{"role":"assistant","content":blocks}}));
            }
            "codex" => {
                records.push(
                    json!({"type":"session_meta","payload":{"id":"equivalent","cwd":"/synthetic"}}),
                );
                records.push(json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Equivalent assistant response."}]}}));
                records.extend(requests.iter().enumerate().map(|(index, (name, arguments))| json!({"type":"response_item","payload":{"type":"function_call","call_id":format!("call-{index}"),"name":name,"arguments":if encoded { Value::String(arguments.to_string()) } else { arguments.clone() }}})));
            }
            "opencode" => {
                let connection = Connection::open(&path).unwrap();
                connection.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY); CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, data TEXT); CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, data TEXT);").unwrap();
                connection
                    .execute("INSERT INTO session VALUES ('equivalent')", [])
                    .unwrap();
                connection
                    .execute(
                        "INSERT INTO message VALUES ('assistant-record', 'equivalent', ?1)",
                        [json!({"role":"assistant","time":{"created":1000}}).to_string()],
                    )
                    .unwrap();
                connection
                    .execute(
                        "INSERT INTO part VALUES ('text', 'assistant-record', ?1)",
                        [
                            json!({"type":"text","text":"Equivalent assistant response."})
                                .to_string(),
                        ],
                    )
                    .unwrap();
                for (index, (name, arguments)) in requests.iter().enumerate() {
                    connection.execute("INSERT INTO part VALUES (?1, 'assistant-record', ?2)", [format!("part-{index}"), json!({"type":"tool","tool":name,"callID":format!("call-{index}"),"state":{"status":"running","input":arguments}}).to_string()]).unwrap();
                }
            }
            "pi" => {
                records.push(json!({"type":"session","version":3,"id":"equivalent","timestamp":"2026-01-01T00:00:00Z","cwd":"/synthetic"}));
                let mut blocks =
                    vec![json!({"type":"text","text":"Equivalent assistant response."})];
                blocks.extend(requests.iter().enumerate().map(|(index, (name, arguments))| json!({"type":"toolCall","id":format!("call-{index}"),"name":name,"arguments":arguments})));
                records.push(json!({"type":"message","id":"assistant-record","parentId":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","content":blocks}}));
            }
            "cursor" => {
                records.push(json!({"sessionId":"equivalent","cursor_source":"agent_transcript"}));
                let mut blocks =
                    vec![json!({"type":"text","text":"Equivalent assistant response."})];
                blocks.extend(requests.iter().enumerate().map(|(index, (name, arguments))| json!({"type":"tool-use","id":format!("call-{index}"),"name":name,"input":arguments})));
                records.push(json!({"role":"assistant","message":{"content":blocks}}));
            }
            "antigravity" => {
                records.push(json!({"type":"PLANNER_RESPONSE","step_index":1,"content":"Equivalent assistant response.","tool_calls":requests.iter().map(|(name, arguments)| json!({"name":name,"args":arguments})).collect::<Vec<_>>()}));
            }
            _ => unreachable!(),
        }
        for (index, record) in records.iter_mut().enumerate() {
            record["timestamp"] = json!(format!("2026-01-01T00:00:{index:02}Z"));
        }
        let source = if agent == "opencode" {
            RawSource::Sqlite(path)
        } else {
            RawSource::Jsonl(
                records
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n",
            )
        };
        let input = SessionInput {
            agent: agent.to_owned(),
            session_id: "equivalent".to_owned(),
            source: source.clone(),
            fork_parent_session_id: None,
            source_format: SourceFormat::Uncharacterized,
        };
        let capabilities = reader_for(agent).capabilities(&input);
        let run = run_pipeline(agent, "equivalent", source, capabilities);
        run.store.with_connection(|connection| {
            let published = query_turn_content_offset_selected(
                connection,
                &TurnSessionKey {
                    environment_key: "native",
                    agent,
                    session_id: "equivalent",
                },
                &FenceScope::single(1),
                None,
                &Default::default(),
                0,
                INPUT_SELECTION,
            )
            .unwrap();
            let selected = select_session_content(
                &prepare_session_content("equivalent", run.source_format, published, Vec::new()),
                INPUT_SELECTION,
            );
            assert_eq!(selected.actions.len(), 6, "{agent}: {selected:?}");
            for action in selected
                .actions
                .iter()
                .filter(|action| action.kind == "tool_input")
            {
                let unknown_tool = action.tool_name.as_deref() == Some("notify");
                assert_eq!(
                    action.metadata.bindings.is_empty(),
                    encoded || unknown_tool,
                    "{agent}: {action:?}"
                );
                assert!(
                    action
                        .metadata
                        .bindings
                        .iter()
                        .all(|binding| binding.field != JevInputField::FileEditContent)
                );
                let index = requests
                    .iter()
                    .position(|(name, _)| Some(*name) == action.tool_name.as_deref())
                    .unwrap();
                use antiburn_local::analysis::jev_evidence::JevNativeFieldContainer;
                let (prefix, container) = match agent {
                    "claude" => (
                        format!("/message/content/{}/input/", index + 1),
                        JevNativeFieldContainer::Record,
                    ),
                    "pi" => (
                        format!("/message/content/{}/arguments/", index + 1),
                        JevNativeFieldContainer::Record,
                    ),
                    "codex" => (
                        "/payload/arguments/".into(),
                        JevNativeFieldContainer::Record,
                    ),
                    "opencode" => ("/state/input/".into(), JevNativeFieldContainer::Part),
                    "cursor" => ("/input/".into(), JevNativeFieldContainer::ToolBlock),
                    "antigravity" => (
                        format!("/tool_calls/{index}/args/"),
                        JevNativeFieldContainer::Step,
                    ),
                    _ => unreachable!(),
                };
                for binding in &action.metadata.bindings {
                    assert_eq!(binding.container, container);
                    let key = binding.pointer.strip_prefix(&prefix).unwrap();
                    let native = requests[index].1.get(key).unwrap().as_str().unwrap();
                    assert_eq!(native.get(binding.start..binding.end), Some(native));
                }
                assert_eq!(
                    action.metadata.state,
                    match agent {
                        "opencode" =>
                            antiburn_local::analysis::jev_evidence::JevOperationState::Running,
                        "pi" => antiburn_local::analysis::jev_evidence::JevOperationState::Pending,
                        "claude" | "codex" | "cursor" | "antigravity" =>
                            antiburn_local::analysis::jev_evidence::JevOperationState::Unknown,
                        _ => unreachable!(),
                    }
                );
            }
            assert!(
                selected
                    .actions
                    .iter()
                    .all(|action| action.authority == "assistant"),
                "{agent}"
            );
            let expected = [
                (
                    JevInputField::AssistantMessage,
                    "Equivalent assistant response.",
                ),
                (JevInputField::BashCommandInput, "printf equivalent"),
                (
                    JevInputField::FileEditPath,
                    r#"{"paths":["src/equivalent.rs"]}"#,
                ),
                (
                    JevInputField::ReadFilePath,
                    r#"{"paths":["src/reference.rs"]}"#,
                ),
                (
                    JevInputField::SearchFilesQuery,
                    r#"{"include":"*.rs","path":"src","pattern":"equivalent"}"#,
                ),
                (
                    JevInputField::OtherToolInput,
                    r#"{"message":"equivalent notification"}"#,
                ),
            ];
            for (field, expected_text) in expected {
                assert!(
                    selected
                        .actions
                        .iter()
                        .any(|action| action.text == expected_text),
                    "{agent}: {field:?}"
                );
                let availability = selected
                    .field_availability
                    .iter()
                    .find(|availability| availability.field == field)
                    .unwrap();
                assert_eq!(availability.observed_parts, 1, "{agent}: {field:?}");
                assert_eq!(availability.malformed_parts, 0, "{agent}: {field:?}");
            }
            let retained = serde_json::to_string(&selected).unwrap();
            for excluded in ["EXCLUDED_OLD", "EXCLUDED_NEW", "EXCLUDED_MATCH"] {
                assert!(!retained.contains(excluded), "{agent}: {excluded}");
            }
        });
    }
}

#[test]
fn antigravity_recorded_content_truncation_survives_without_marking_tool_arguments() {
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    let run = run_pipeline("antigravity", "truncated-content", RawSource::Jsonl(serde_json::json!({"type":"PLANNER_RESPONSE","step_index":1,"created_at":"2026-01-01T00:00:01Z","model":"model-a","content":"partial response","truncated_fields":["content"],"tool_calls":[{"name":"bash","args":{"command":"printf request"}}]}).to_string()), SourceCapabilities::antigravity());
    run.store.with_connection(|connection| {
        let selection = JevInputSelection::from_fields(&[
            JevInputField::AssistantMessage,
            JevInputField::BashCommandInput,
        ]);
        let published = query_turn_content_offset_selected(
            connection,
            &turn_key("antigravity", "truncated-content"),
            &FenceScope::single(1),
            None,
            &Default::default(),
            0,
            selection,
        )
        .unwrap();
        let selected = select_session_content(
            &prepare_session_content(
                "truncated-content",
                run.source_format,
                published,
                Vec::new(),
            ),
            selection,
        );
        let response = selected
            .actions
            .iter()
            .find(|action| action.kind == "assistant")
            .unwrap();
        assert!(response.truncated);
        let request = selected
            .actions
            .iter()
            .find(|action| action.kind == "tool_input")
            .unwrap();
        assert!(!request.truncated);
        assert_eq!(request.metadata.bindings.len(), 1);
        assert_eq!(request.text, "printf request");
    });
}

#[test]
fn pi_subagent_filter_preserves_original_native_field_indexes() {
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    let source = [
        serde_json::json!({"type":"session","version":3,"id":"indexed-fields","timestamp":"2026-01-01T00:00:00Z","cwd":"/synthetic"}),
        serde_json::json!({"type":"message","id":"assistant","parentId":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"child","name":"subagent","arguments":{"agent":"worker","task":"synthetic task"}},{"type":"toolCall","id":"read","name":"read","arguments":{"path":"src/é.rs"}}]}}),
    ].iter().map(serde_json::Value::to_string).collect::<Vec<_>>().join("\n") + "\n";
    let run = run_pipeline(
        "pi",
        "indexed-fields",
        RawSource::Jsonl(source),
        SourceCapabilities::pi(),
    );
    run.store.with_connection(|connection| {
        let published = query_turn_content_offset_selected(
            connection,
            &turn_key("pi", "indexed-fields"),
            &FenceScope::single(1),
            None,
            &Default::default(),
            0,
            JevInputSelection::from_fields(&[JevInputField::ReadFilePath]),
        )
        .unwrap();
        assert_eq!(published.parts.len(), 1);
        let bindings = &published.parts[0].part.metadata.bindings;
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].pointer, "/message/content/1/arguments/path");
        assert_eq!(bindings[0].end, "src/é.rs".len());
    });
}
