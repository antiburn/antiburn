use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, NormalizedRecord, PartialReason, RawSource, RecordSink,
    SessionCollector, SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator,
    SessionSummary, SourceCapabilities, SourceKind, VisitOutcome, adapter_for,
};
use rusqlite::{Connection, params};
use serde_json::json;
use tempfile::TempDir;

fn create_database() -> (TempDir, std::path::PathBuf) {
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("opencode.db");
    let connection = Connection::open(&path).expect("database");
    connection
        .execute_batch(
            "CREATE TABLE session (
                 id TEXT PRIMARY KEY, parent_id TEXT, time_created INTEGER, time_updated INTEGER
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
        .expect("schema");
    drop(connection);
    (directory, path)
}

fn sqlite_input(path: &std::path::Path, session_id: &str) -> SessionInput {
    SessionInput {
        agent: "opencode".to_owned(),
        session_id: session_id.to_owned(),
        source: RawSource::Sqlite(path.to_owned()),
    }
}

fn insert_session(connection: &Connection, id: &str, parent: Option<&str>, timestamp: i64) {
    connection
        .execute(
            "INSERT INTO session VALUES (?1, ?2, ?3, ?3)",
            params![id, parent, timestamp],
        )
        .expect("session");
}

fn insert_message(connection: &Connection, id: &str, session_id: &str, timestamp: i64, data: &str) {
    connection
        .execute(
            "INSERT INTO message VALUES (?1, ?2, ?3, ?3, ?4)",
            params![id, session_id, timestamp, data],
        )
        .expect("message");
}

fn insert_part(
    connection: &Connection,
    id: &str,
    message_id: &str,
    session_id: &str,
    timestamp: i64,
    data: &str,
) {
    connection
        .execute(
            "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            params![id, message_id, session_id, timestamp, data],
        )
        .expect("part");
}

#[test]
fn opencode_capabilities_match_the_observed_contract() {
    assert_eq!(
        SourceCapabilities::opencode(),
        SourceCapabilities {
            request_context_tokens: true,
            cache_write_tokens: true,
            timestamps_and_order: true,
            tool_invocations: true,
            skill_mcp_attribution: false,
            tool_definitions: false,
            model_identity: true,
            token_classes: true,
            reasoning_effort_tier: true,
            fast_tier: false,
            service_tier: false,
            subagent_relationships: false,
            subagent_models: false,
            compaction_boundaries: true,
            thread_identity: false,
            quota_incidents: false,
            harness_version: false,
        }
    );
}

#[test]
fn native_sqlite_streams_root_and_descendant_messages_in_order() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, 10);
    insert_session(&connection, "child", Some("root"), 20);
    insert_message(
        &connection,
        "later",
        "root",
        40,
        r#"{"role":"assistant","modelID":"model-a","variant":"high","tokens":{"input":100,"output":20,"reasoning":5,"cache":{"read":30,"write":40}}}"#,
    );
    insert_message(&connection, "earlier", "child", 30, r#"{"role":"user"}"#);
    insert_part(
        &connection,
        "reasoning",
        "later",
        "root",
        41,
        r#"{"type":"reasoning","text":"PRIVATE_REASONING"}"#,
    );
    insert_part(
        &connection,
        "tool",
        "later",
        "root",
        42,
        r#"{"type":"tool","tool":"bash","state":{"input":{"command":"cargo test PRIVATE_PATH"},"output":"PRIVATE_OUTPUT"}}"#,
    );
    insert_part(
        &connection,
        "patch",
        "later",
        "root",
        43,
        r#"{"type":"patch","files":["PRIVATE_PATH"],"diff":"PRIVATE_DIFF"}"#,
    );
    insert_part(
        &connection,
        "compaction",
        "later",
        "root",
        44,
        r#"{"type":"compaction","auto":true,"snapshot":"PRIVATE_SNAPSHOT"}"#,
    );
    drop(connection);

    let input = sqlite_input(&path, "root");
    let mut collector = SessionCollector::new("opencode", "root");
    adapter_for("opencode")
        .visit(&input, &mut collector)
        .expect("stream database");
    let session = collector.into_session().expect("finished session");

    assert_eq!(session.events.len(), 2);
    assert_eq!(session.events[0].role, antiburn_local::analysis::Role::User);
    let assistant = &session.events[1];
    assert_eq!(assistant.usage.input_tokens, 100);
    assert_eq!(assistant.usage.output_tokens, 25);
    assert_eq!(assistant.usage.cache_read_tokens, 30);
    assert_eq!(assistant.usage.cache_creation_tokens, 40);
    assert_eq!(assistant.model.as_deref(), Some("model-a"));
    assert_eq!(assistant.thinking_mode.as_deref(), Some("high"));
    assert!(assistant.has_thinking);
    assert!(assistant.is_compaction_boundary);
    assert_eq!(assistant.tools.len(), 2);

    let retained = serde_json::to_string(&session).expect("serialize normalized session");
    for private in [
        "PRIVATE_REASONING",
        "PRIVATE_PATH",
        "PRIVATE_OUTPUT",
        "PRIVATE_DIFF",
        "PRIVATE_SNAPSHOT",
    ] {
        assert!(!retained.contains(private));
    }
}

#[cfg(feature = "test-instrumentation")]
#[test]
fn native_sqlite_does_not_call_discovery_rendering() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, 10);
    insert_message(&connection, "message", "root", 20, r#"{"role":"user"}"#);
    drop(connection);
    antiburn_local::discovery::track_provider_db_renders(&path);

    let input = sqlite_input(&path, "root");
    let mut collector = SessionCollector::new("opencode", "root");
    adapter_for("opencode")
        .visit(&input, &mut collector)
        .expect("stream database");
    collector.into_session().expect("finished session");

    assert_eq!(
        antiburn_local::discovery::take_tracked_provider_db_renders(&path),
        0
    );
}

#[test]
fn malformed_unknown_and_oversized_rows_report_partial_without_payload() {
    const PRIVATE: &str = "PRIVATE_OVERSIZED_CONTENT";
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, 10);
    insert_message(&connection, "malformed", "root", 20, "{not-json");
    insert_message(
        &connection,
        "valid",
        "root",
        30,
        r#"{"role":"assistant","tokens":{"input":1}}"#,
    );
    insert_part(
        &connection,
        "unknown",
        "valid",
        "root",
        31,
        &format!(r#"{{"type":"future-part","text":"{PRIVATE}"}}"#),
    );
    let first_part = format!(
        r#"{{"type":"text","text":"{}"}}"#,
        "a".repeat(4 * 1024 * 1024 + 100)
    );
    let second_part = format!(
        r#"{{"type":"text","text":"{PRIVATE}{}"}}"#,
        "b".repeat(4 * 1024 * 1024 + 100)
    );
    insert_part(&connection, "large-a", "valid", "root", 32, &first_part);
    insert_part(&connection, "large-b", "valid", "root", 33, &second_part);
    let oversized = format!(
        r#"{{"role":"assistant","text":"{}"}}"#,
        "x".repeat(8 * 1024 * 1024)
    );
    insert_message(&connection, "oversized", "root", 40, &oversized);
    drop(connection);

    let input = sqlite_input(&path, "root");
    let mut collector = SessionCollector::new("opencode", "root");
    adapter_for("opencode")
        .visit(&input, &mut collector)
        .expect("stream database");
    let reasons = collector.partial_reasons().clone();
    let session = collector.into_session().expect("finished session");

    assert!(reasons.contains(&PartialReason::MalformedRecord));
    assert!(reasons.contains(&PartialReason::UnrecognizedRecordType));
    assert!(reasons.contains(&PartialReason::Oversized));
    assert_eq!(session.events.len(), 1);
    assert!(!format!("{reasons:?}").contains(PRIVATE));
    assert!(!serde_json::to_string(&session).unwrap().contains(PRIVATE));
}

#[test]
fn database_claim_is_checked_inside_the_snapshot() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, 100);
    insert_message(&connection, "message", "root", 120, r#"{"role":"user"}"#);
    drop(connection);
    let input = sqlite_input(&path, "root");
    let adapter = adapter_for("opencode");

    let mut mismatch = SessionCollector::new("opencode", "root");
    let outcome = adapter
        .visit_db_claimed(&input, "sv1:db:0:0", &|| false, &mut mismatch)
        .expect("check mismatch");
    assert!(matches!(outcome, VisitOutcome::SourceChanged(_)));

    let mut matching = SessionCollector::new("opencode", "root");
    let outcome = adapter
        .visit_db_claimed(&input, "sv1:db:120:2", &|| false, &mut matching)
        .expect("check matching claim");
    assert_eq!(outcome, VisitOutcome::AcceptedFull);
    assert_eq!(matching.into_session().expect("finished").events.len(), 1);
}

#[test]
fn exported_messages_stream_without_session_wide_collection() {
    struct CountingSink {
        records: u64,
        finished: bool,
    }

    impl RecordSink for CountingSink {
        fn record(&mut self, record: NormalizedRecord) {
            if matches!(record, NormalizedRecord::MetricsEvent(_)) {
                self.records += 1;
            }
        }

        fn finish(&mut self, _summary: SessionSummary) {
            self.finished = true;
        }
    }

    let mut jsonl = String::new();
    for index in 0..10_000 {
        jsonl.push_str(&format!(
            "{{\"type\":\"message\",\"messageID\":\"m{index}\",\"time\":{{\"created\":{index}}},\"payload\":{{\"role\":\"user\"}}}}\n"
        ));
    }
    let input = SessionInput {
        agent: "opencode".to_owned(),
        session_id: "many".to_owned(),
        source: RawSource::Jsonl(jsonl),
    };
    let mut sink = CountingSink {
        records: 0,
        finished: false,
    };
    adapter_for("opencode")
        .visit(&input, &mut sink)
        .expect("stream synthetic export");

    assert_eq!(sink.records, 10_000);
    assert!(sink.finished);
}

#[test]
fn metrics_and_evidence_publish_from_the_stream() {
    let input = SessionInput {
        agent: "opencode".to_owned(),
        session_id: "evidence".to_owned(),
        source: RawSource::Jsonl(
            concat!(
                r#"{"type":"message","messageID":"m1","time":{"created":1000},"payload":{"role":"assistant","modelID":"model-a","variant":"high","tokens":{"input":10,"output":2,"reasoning":3,"cache":{"read":4,"write":5}}}}"#,
                "\n",
                r#"{"type":"part","messageID":"m1","payload":{"type":"tool","tool":"read","state":{"input":{"filePath":"PRIVATE_PATH"}}}}"#,
                "\n"
            )
            .to_owned(),
        ),
    };
    let metrics = SessionMetricsAccumulator::new("opencode", "evidence");
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "opencode".to_owned(),
        session_id: "evidence".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::opencode(),
    });
    let mut sink = CompositeSink::new(metrics, evidence);
    let outcome = adapter_for("opencode")
        .visit(&input, &mut sink)
        .expect("stream export");
    sink.observe_source_outcome(outcome);
    let (metrics, evidence) = sink.into_parts().expect("published evidence");
    let evidence = evidence.evidence();

    assert_eq!(metrics.metrics().tokens_out, 5);
    assert_eq!(metrics.metrics().tokens_in, 15);
    assert_eq!(evidence.capabilities, SourceCapabilities::opencode());
    assert!(!json!(evidence).to_string().contains("PRIVATE_PATH"));
}
