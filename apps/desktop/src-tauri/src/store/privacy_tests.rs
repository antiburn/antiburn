//! Full-database privacy sweep, at the layer that owns the schema.
//!
//! `crates/antiburn-local/tests/turn_content_privacy.rs` proves the same
//! rule at the engine layer. That crate defines only a small `session` +
//! `turn` + `turn_content` schema. This module proves the rule again
//! against the real, fully migrated [`Store`] schema: every table, including
//! `session_evidence`, `session_analysis`, `session_relation`, and any table
//! a future migration adds. It also calls the real [`Store::delete_session`]
//! and [`Store::clear_local_session_data`] methods, not a lower-level
//! stand-in for them. See "Privacy with content stored" in
//! `docs/plans/session-evidence-harness-parity.md`.

use std::path::Path;
use std::sync::Arc;

use antiburn_local::analysis::{RawSource, SessionInput, TurnRowStore};
use antiburn_local::model::AgentKind;
use rusqlite::Connection;
use rusqlite::types::Value as SqlValue;

use super::*;

fn store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-privacy-test-state")).expect("in-memory store")
}

/// Every `(table, column, text)` value across a connection's tables.
///
/// Reads the table list from `sqlite_master` instead of a fixed list. This
/// sweeps a table this test does not know about, including one a future
/// migration adds.
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
/// anywhere at all.
fn assert_absent_everywhere(connection: &Connection, sentinel: &str) {
    for (table, column, text) in all_text_and_blob_values(connection) {
        assert!(
            !text.contains(sentinel),
            "{sentinel} survived deletion in {table}.{column}: {text}"
        );
    }
}

/// A session row whose title never carries `sentinel`.
///
/// The `session` table's doc comment in `schema.rs` describes a real
/// exception to "content lives only in `turn_content`": `title` can hold a
/// 200-character excerpt of the first user message, when `title_source` is
/// `"firstMessage"`. This helper sets a fixed title directly instead, so it
/// never derives one from the fixture. Every fixture below still keeps its
/// sentinel out of the first user turn, as a defensive practice in case a
/// future change starts deriving titles here.
fn privacy_session(kind: AgentKind, session_id: &str) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", kind.slug(), session_id),
        source_kind: "file".into(),
        source_label: format!(
            "/home/avery/.{}/projects/demo/{session_id}.jsonl",
            kind.slug()
        ),
        wsl_distro: None,
        title: Some("Fixture session (no sentinel here)".into()),
        title_source: Some("explicit".into()),
        cwd: Some("/home/avery/code/widgets".into()),
        surface: "cli".into(),
        updated_at_epoch: Some(1_000),
        activity_cursor: String::new(),
        activity_source: "mtime".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: None,
    }
}

/// Ingests `source` for `kind`/`session_id` through the real analysis
/// pipeline and publishes the result into `store`.
///
/// Runs `crate::analysis::evidence_pass_with_turn_rows`, the same path
/// `insights_worker` runs, then calls [`Store::publish_projections`], the
/// same call the durable worker makes once a pass succeeds (see
/// `apply_outcome` in `insights_worker.rs`). This populates the session's
/// `session_analysis` and `session_evidence` rows for real, not just its
/// `turn`/`turn_content` rows, so the sweep below also checks the evidence
/// and metrics JSON columns.
fn ingest_and_publish(store: &Store, kind: AgentKind, session_id: &str, source: RawSource) {
    let mut record = privacy_session(kind, session_id);
    let fingerprint = format!("sv1:{session_id}");
    record.source_fingerprint = Some(fingerprint.clone());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .expect("upsert session");
    let claim = store
        .claim_next_evidence(&[kind.slug()], 1_000, 60)
        .expect("claim evidence")
        .unwrap_or_else(|| panic!("a claim must be available for {kind:?}"));

    let turn_store: Arc<dyn TurnRowStore> = Arc::new(FencedTurnRowStore::new(
        store.clone(),
        record.key.clone(),
        claim.claim_fence,
    ));
    let input = SessionInput {
        agent: crate::agents::vendor_label(kind).to_owned(),
        session_id: session_id.to_owned(),
        source,
    };
    let mut pass = crate::analysis::evidence_pass_with_turn_rows(
        std::slice::from_ref(&input),
        &|| false,
        Some(turn_store),
    );
    assert_eq!(
        pass.outcome,
        crate::analysis::PassOutcome::Published,
        "the {kind:?} fixture must publish cleanly"
    );
    // `evidence_pass_with_turn_rows` streams straight from the fixture. It
    // has no scan step to stamp a fingerprint; the real worker's outer
    // caller does that (see `run_record_pass_with`). Stamp it here, so this
    // pass's cache record matches the session row's fingerprint. This
    // mirrors `published_evidence_pass` in `tests.rs`.
    pass.analysis.fingerprint = fingerprint;

    let mut analysis_record = pass
        .analysis
        .record(&record.key)
        .expect("a published pass yields a cache record");
    analysis_record.analyzed_generation = claim.source_generation;

    let evidence = pass.evidence.expect("a published pass carries evidence");
    let completion = EvidenceCompletion {
        claim_fence: claim.claim_fence,
        // The real worker derives Ready vs. Unsupported from detector
        // eligibility (`published_status` in `insights_worker.rs`). Every
        // fixture here is a supported vendor's content-bearing session, so
        // it is always Ready. This test checks where content lands, not
        // detector eligibility.
        status: PublishedEvidence::Ready,
        evidence_schema_revision: evidence.schema_revision,
        evidence_json: serde_json::to_string(&evidence).expect("serialize evidence"),
        diagnostics_json: Some(
            serde_json::to_string(&evidence.diagnostics).expect("serialize diagnostics"),
        ),
    };
    let published = store
        .publish_projections(
            &analysis_record,
            pass.analysis.started_at_epoch,
            &completion,
            &[],
        )
        .expect("publish projections");
    assert!(
        published,
        "publish_projections must win its fence for {kind:?}"
    );
}

const CLAUDE_SENTINEL: &str = "PRIVACY-SENTINEL-claude-desktop-e11f9a";

fn claude_fixture() -> String {
    let user = serde_json::json!({
        "type": "user",
        "timestamp": "2026-01-01T00:00:00Z",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": format!("{CLAUDE_SENTINEL} please investigate")}],
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
                {"type": "text", "text": format!("{CLAUDE_SENTINEL} responding")},
                {"type": "thinking", "thinking": format!("{CLAUDE_SENTINEL} pondering")},
                {"type": "tool_use", "name": "Bash", "input": {"command": format!("echo {CLAUDE_SENTINEL}")}},
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
                {"type": "tool_result", "tool_use_id": "t1", "content": format!("{CLAUDE_SENTINEL} done")},
            ],
        }
    })
    .to_string();
    format!("{user}\n{assistant}\n{tool_result}\n")
}

const CODEX_SENTINEL: &str = "PRIVACY-SENTINEL-codex-desktop-7c58d2";

fn codex_fixture() -> String {
    let lines = [
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:00Z", "type": "session_meta",
            "payload": {"id": "privacy-desktop-codex", "timestamp": "2026-08-01T09:59:58Z", "cwd": "/home/avery/demo", "cli_version": "0.0.0-test", "source": "cli"}
        }),
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:01Z", "type": "turn_context",
            "payload": {"model": "gpt-test", "effort": "medium"}
        }),
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:02Z", "type": "response_item",
            "payload": {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": format!("{CODEX_SENTINEL} please investigate")}
            ]}
        }),
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:03Z", "type": "response_item",
            "payload": {"type": "reasoning", "summary": [{"text": format!("{CODEX_SENTINEL} pondering")}]}
        }),
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:04Z", "type": "response_item",
            "payload": {"type": "message", "role": "assistant", "content": [
                {"type": "output_text", "text": format!("{CODEX_SENTINEL} responding")}
            ]}
        }),
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:05Z", "type": "response_item",
            "payload": {"type": "function_call", "name": "exec_command", "arguments": format!("{{\"cmd\":\"echo {CODEX_SENTINEL}\"}}"), "call_id": "call-1"}
        }),
        serde_json::json!({
            "timestamp": "2026-08-01T10:00:06Z", "type": "response_item",
            "payload": {"type": "function_call_output", "call_id": "call-1", "output": format!("{CODEX_SENTINEL} done")}
        }),
    ];
    lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[test]
fn claude_session_ingest_confines_its_sentinel_to_turn_content_across_the_whole_schema() {
    let store = store();
    ingest_and_publish(
        &store,
        AgentKind::Claude,
        "privacy-desktop-claude",
        RawSource::Jsonl(claude_fixture()),
    );

    let connection = store.lock();
    let stored: Vec<String> = connection
        .prepare("SELECT content FROM turn_content")
        .expect("prepare")
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .expect("query")
        .map(|blob| String::from_utf8(blob.expect("row content is valid UTF-8")).unwrap())
        .collect();
    assert!(
        stored.join("\n").contains(CLAUDE_SENTINEL),
        "turn_content is missing the Claude sentinel"
    );
    assert_confined_to_turn_content(&connection, CLAUDE_SENTINEL);
}

#[test]
fn codex_session_ingest_confines_its_sentinel_to_turn_content_across_the_whole_schema() {
    let store = store();
    ingest_and_publish(
        &store,
        AgentKind::Codex,
        "privacy-desktop-codex",
        RawSource::Jsonl(codex_fixture()),
    );

    let connection = store.lock();
    let stored: Vec<String> = connection
        .prepare("SELECT content FROM turn_content")
        .expect("prepare")
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .expect("query")
        .map(|blob| String::from_utf8(blob.expect("row content is valid UTF-8")).unwrap())
        .collect();
    assert!(
        stored.join("\n").contains(CODEX_SENTINEL),
        "turn_content is missing the Codex sentinel"
    );
    assert_confined_to_turn_content(&connection, CODEX_SENTINEL);
}

#[test]
fn delete_session_removes_the_sentinel_from_every_table() {
    let store = store();
    ingest_and_publish(
        &store,
        AgentKind::Claude,
        "privacy-desktop-delete",
        RawSource::Jsonl(claude_fixture()),
    );
    let key = SessionKey::new("native", AgentKind::Claude.slug(), "privacy-desktop-delete");

    assert!(store.delete_session(&key).expect("delete session"));

    let connection = store.lock();
    assert_absent_everywhere(&connection, CLAUDE_SENTINEL);
}

#[test]
fn clear_local_session_data_removes_the_sentinel_from_every_table() {
    let store = store();
    ingest_and_publish(
        &store,
        AgentKind::Claude,
        "privacy-desktop-clear-claude",
        RawSource::Jsonl(claude_fixture()),
    );
    ingest_and_publish(
        &store,
        AgentKind::Codex,
        "privacy-desktop-clear-codex",
        RawSource::Jsonl(codex_fixture()),
    );

    assert_eq!(store.clear_local_session_data().expect("clear"), 2);

    let connection = store.lock();
    assert_absent_everywhere(&connection, CLAUDE_SENTINEL);
    assert_absent_everywhere(&connection, CODEX_SENTINEL);
}
