//! Turn-content capture must never leak transcript text into any other
//! projection. Content lives only in `turn_content`. `NormalizedSession`,
//! `SessionEvidence` (diagnostics included), `SessionMetrics`, and every
//! other table in the schema must never carry it. Deleting a session's turn
//! rows must remove it completely. See "Privacy with content stored" in
//! `docs/plans/session-evidence-harness-parity.md`.
//!
//! This file carries one fixture per vendor that stores content: Claude,
//! Codex, OpenCode, and Pi. `cursor`, `antigravity`, and the generic JSONL
//! fallback emit no `TurnContent` records at all. They never call
//! `extract_content_parts` or push a `ContentPart`. So they have no
//! content-privacy surface to test.

use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, MemoryTurnRowStore, RawSource, SessionEvidenceAccumulator,
    SessionInput, SessionMetricsAccumulator, SourceCapabilities, SourceKind, TurnRowSink,
    TurnRowStore, TurnSessionKey, adapter_for, delete_turn_rows, normalize_source,
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
}

fn run_pipeline(
    agent: &str,
    session_id: &str,
    source: RawSource,
    capabilities: SourceCapabilities,
) -> PrivacyRun {
    let input = SessionInput {
        agent: agent.to_string(),
        session_id: session_id.to_string(),
        source,
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
    let outcome = adapter_for(agent)
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
