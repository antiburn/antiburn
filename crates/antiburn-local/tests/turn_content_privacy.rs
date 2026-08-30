//! Turn-content capture must never leak transcript text into any other
//! projection. Content lives only in `turn_content`: `NormalizedSession`,
//! `SessionEvidence` (diagnostics included), and `SessionMetrics` must never
//! carry it, and deleting a session's turn rows must remove it completely.
//! See "Privacy with content stored" in
//! `docs/plans/session-evidence-harness-parity.md`.

use std::sync::{Arc, Mutex};

use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, RawSource, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SourceCapabilities, SourceKind, TURN_MIGRATIONS, TurnFacts, TurnRow,
    TurnRowError, TurnRowSink, TurnRowStore, TurnSessionKey, adapter_for, count_turn_content_rows,
    delete_turn_rows, insert_turn_rows, normalize_source,
};
use rusqlite::{Connection, params};

const SENTINEL_USER: &str = "SENTINEL-USER-7f3a";
const SENTINEL_THINK: &str = "SENTINEL-THINK-9c2b";
const SENTINEL_TOOLIN: &str = "SENTINEL-TOOLIN-3d1e";
const SENTINEL_RESULT: &str = "SENTINEL-RESULT-88aa";

const SENTINELS: [&str; 4] = [
    SENTINEL_USER,
    SENTINEL_THINK,
    SENTINEL_TOOLIN,
    SENTINEL_RESULT,
];

const ENVIRONMENT_KEY: &str = "native";
const AGENT: &str = "claude";
const SESSION_ID: &str = "content-privacy-session";

fn key() -> TurnSessionKey<'static> {
    TurnSessionKey {
        environment_key: ENVIRONMENT_KEY,
        agent: AGENT,
        session_id: SESSION_ID,
    }
}

/// A small Claude transcript carrying one sentinel per captured content
/// kind: a user prompt, an assistant thinking block, a tool call's input,
/// and a tool result.
fn fixture() -> String {
    let user = serde_json::json!({
        "type": "user",
        "timestamp": "2026-01-01T00:00:00Z",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": format!("{SENTINEL_USER} please investigate")}],
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
                {"type": "thinking", "thinking": format!("{SENTINEL_THINK} pondering")},
                {
                    "type": "tool_use",
                    "name": "Bash",
                    "input": {"command": format!("echo {SENTINEL_TOOLIN}")},
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
                {"type": "tool_result", "tool_use_id": "t1", "content": format!("{SENTINEL_RESULT} done")},
            ],
        }
    })
    .to_string();
    format!("{user}\n{assistant}\n{tool_result}\n")
}

fn test_connection() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory connection");
    conn.execute_batch(
        "CREATE TABLE session (
            environment_key TEXT NOT NULL,
            agent TEXT NOT NULL,
            session_id TEXT NOT NULL,
            PRIMARY KEY (environment_key, agent, session_id)
        ) STRICT;",
    )
    .expect("create session table");
    for migration in TURN_MIGRATIONS {
        conn.execute_batch(migration)
            .expect("apply turn schema migration");
    }
    conn.execute(
        "INSERT INTO session (environment_key, agent, session_id) VALUES (?1, ?2, ?3)",
        params![ENVIRONMENT_KEY, AGENT, SESSION_ID],
    )
    .expect("insert session");
    conn
}

struct TestWriter(Mutex<Connection>);

impl TurnRowStore for TestWriter {
    fn write_turn_rows(&self, rows: &[TurnRow]) -> Result<(), TurnRowError> {
        let conn = self.0.lock().expect("lock");
        insert_turn_rows(&conn, &key(), 1, rows).map_err(TurnRowError::from)
    }

    // Never read: this test inspects `turn_content` through its own
    // connection handle instead of through the store trait.
    fn query_turn_facts(&self) -> Result<TurnFacts, TurnRowError> {
        Err(TurnRowError("not readable".to_owned()))
    }
}

#[test]
fn turn_content_captures_sentinels_while_every_other_projection_stays_clean() {
    let input = SessionInput {
        agent: AGENT.to_string(),
        session_id: SESSION_ID.to_string(),
        source: RawSource::Jsonl(fixture()),
    };

    // The normalized model never carries message text.
    let normalized_session = normalize_source(&input).expect("fixture must normalize");
    let normalized_json = serde_json::to_string(&normalized_session).expect("serialize session");
    for sentinel in SENTINELS {
        assert!(
            !normalized_json.contains(sentinel),
            "NormalizedSession leaked {sentinel}"
        );
    }

    // Run the full pipeline: metrics, evidence, and turn rows in one pass,
    // exactly as the durable analysis worker does.
    let writer = Arc::new(TestWriter(Mutex::new(test_connection())));
    let metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities: SourceCapabilities::claude(),
    });
    let turn_rows = TurnRowSink::new(
        Arc::clone(&writer) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        None,
    );
    let mut composite = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = adapter_for("claude")
        .visit(&input, &mut composite)
        .expect("claude source must be visited");
    composite.observe_source_outcome(outcome);
    assert!(!composite.turn_row_write_failed());

    let evidence = composite.evidence().expect("evidence must publish");
    let evidence_json = serde_json::to_string(&evidence).expect("serialize evidence");
    let metrics = composite.metrics().expect("metrics must publish");
    let metrics_json = serde_json::to_string(&metrics).expect("serialize metrics");
    for sentinel in SENTINELS {
        assert!(
            !evidence_json.contains(sentinel),
            "SessionEvidence (including diagnostics) leaked {sentinel}"
        );
        assert!(
            !metrics_json.contains(sentinel),
            "SessionMetrics leaked {sentinel}"
        );
    }

    // Every sentinel reached `turn_content`.
    {
        let conn = writer.0.lock().expect("lock");
        let stored: Vec<String> = conn
            .prepare("SELECT content FROM turn_content")
            .expect("prepare")
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .expect("query")
            .map(|blob| String::from_utf8(blob.expect("row content is valid UTF-8")).unwrap())
            .collect();
        let all_content = stored.join("\n");
        for sentinel in SENTINELS {
            assert!(
                all_content.contains(sentinel),
                "turn_content is missing {sentinel}"
            );
        }
        assert!(count_turn_content_rows(&conn, &key(), 1).expect("count") >= 4);
    }

    // Deleting the session's turn rows removes every sentinel from the
    // database — the mechanism `Store::delete_session` and
    // `Store::clear_local_session_data` both rely on.
    {
        let conn = writer.0.lock().expect("lock");
        delete_turn_rows(&conn, &key()).expect("delete turn rows");
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM turn_content", [], |row| row.get(0))
            .expect("count remaining turn_content rows");
        assert_eq!(remaining, 0);
    }
}
