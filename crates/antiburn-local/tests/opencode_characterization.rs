use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, CoverageReason, EventSource, EvidenceSource, EvidenceValue, MemoryTurnRowStore,
    NormalizedRecord, PartialReason, RawSource, RecordSink, SessionCollector, SessionEvidence,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SessionSummary,
    SourceCapabilities, SourceKind, TurnRowSink, TurnRowStore, VisitOutcome, adapter_for,
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

fn insert_session(
    connection: &Connection,
    id: &str,
    parent: Option<&str>,
    title: Option<&str>,
    timestamp: i64,
) {
    connection
        .execute(
            "INSERT INTO session (id, parent_id, title, time_created, time_updated)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, parent, title, timestamp],
        )
        .expect("session");
}

/// Streams `session_id` through the OpenCode adapter and the real evidence
/// and turn-row pipeline, the same path production uses. Returns the
/// published evidence plus the row store, so a test can also read rows back
/// directly with [`MemoryTurnRowStore::with_connection`].
fn evidence_and_rows(input: &SessionInput) -> (SessionEvidence, Arc<MemoryTurnRowStore>) {
    let metrics = SessionMetricsAccumulator::new(&input.agent, &input.session_id);
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities: SourceCapabilities::opencode(),
    });
    let store = MemoryTurnRowStore::new(&input.agent, &input.session_id);
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        &input.session_id,
        None,
    );
    let mut sink = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = adapter_for("opencode")
        .visit(input, &mut sink)
        .expect("stream opencode source");
    sink.observe_source_outcome(outcome);
    let evidence = sink.evidence().expect("published evidence");
    (evidence, store)
}

/// Reads back `(scope, thread_id, child_id)` for every row of one session,
/// in turn order, straight off [`MemoryTurnRowStore`]'s in-memory database.
fn turn_identities(
    store: &MemoryTurnRowStore,
    session_id: &str,
) -> Vec<(String, String, Option<String>)> {
    store.with_connection(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT scope, thread_id, child_id FROM turn
                  WHERE environment_key = 'native' AND agent = 'opencode'
                    AND session_id = ?1 AND claim_fence = 1
                  ORDER BY turn_index",
            )
            .expect("prepare");
        statement
            .query_map(params![session_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    })
}

fn observed<T: Clone>(value: &EvidenceValue<T>) -> T {
    match value {
        EvidenceValue::Complete(observed) => observed.clone(),
        EvidenceValue::Partial { observed, .. } => observed.clone(),
        EvidenceValue::Unsupported => panic!("evidence unexpectedly unsupported"),
    }
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
            subagent_relationships: true,
            subagent_models: true,
            compaction_boundaries: true,
            thread_identity: true,
            quota_incidents: false,
            harness_version: false,
        }
    );
}

#[test]
fn native_sqlite_streams_root_and_descendant_messages_in_order() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, None, 10);
    insert_session(&connection, "child", Some("root"), None, 20);
    insert_message(
        &connection,
        "later",
        "root",
        40,
        r#"{"role":"assistant","modelID":"model-a","variant":"high","tokens":{"input":100,"output":20,"reasoning":5,"cache":{"read":30,"write":40}}}"#,
    );
    insert_message(&connection, "earlier", "child", 30, r#"{"role":"user"}"#);
    insert_message(
        &connection,
        "child-assistant",
        "child",
        35,
        r#"{"role":"assistant","modelID":"model-child","tokens":{"input":3,"output":2}}"#,
    );
    insert_part(
        &connection,
        "child-tool",
        "child-assistant",
        "child",
        36,
        r#"{"type":"tool","tool":"read","state":{"input":{}}}"#,
    );
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

    assert_eq!(session.events.len(), 3);
    assert_eq!(session.events[0].role, antiburn_local::analysis::Role::User);
    // The child's messages carry the child's own session as their thread and
    // stream as a descendant, even interleaved before the root's message.
    assert_eq!(session.events[0].thread_id.as_deref(), Some("child"));
    assert_eq!(session.events[0].source, EventSource::Subagent);
    let child_assistant = &session.events[1];
    assert_eq!(child_assistant.model.as_deref(), Some("model-child"));
    assert_eq!(child_assistant.tools.len(), 1);
    assert_eq!(child_assistant.thread_id.as_deref(), Some("child"));
    assert_eq!(child_assistant.source, EventSource::Subagent);
    let assistant = &session.events[2];
    assert_eq!(assistant.usage.input_tokens, 100);
    assert_eq!(assistant.usage.output_tokens, 25);
    assert_eq!(assistant.usage.cache_read_tokens, 30);
    assert_eq!(assistant.usage.cache_creation_tokens, 40);
    assert_eq!(assistant.model.as_deref(), Some("model-a"));
    assert_eq!(assistant.thinking_mode.as_deref(), Some("high"));
    assert!(assistant.has_thinking);
    assert!(assistant.is_compaction_boundary);
    assert_eq!(assistant.tools.len(), 2);
    // The root's own message carries the root session as its thread and
    // stays main-scope.
    assert_eq!(assistant.thread_id.as_deref(), Some("root"));
    assert_eq!(assistant.source, EventSource::Parent);

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

#[test]
fn subagent_children_stream_as_delegated_threads() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, None, 10);
    insert_session(&connection, "child", Some("root"), None, 20);
    insert_session(&connection, "grandchild", Some("child"), None, 22);
    insert_message(
        &connection,
        "r1",
        "root",
        10,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}"#,
    );
    insert_message(&connection, "c1", "child", 20, r#"{"role":"user"}"#);
    insert_message(
        &connection,
        "c2",
        "child",
        30,
        r#"{"role":"assistant","modelID":"model-b","tokens":{"input":2,"output":2}}"#,
    );
    insert_message(
        &connection,
        "g1",
        "grandchild",
        32,
        r#"{"role":"assistant","modelID":"model-c","tokens":{"input":1,"output":1}}"#,
    );
    insert_message(
        &connection,
        "r2",
        "root",
        60,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}"#,
    );
    drop(connection);

    let input = sqlite_input(&path, "root");
    let (evidence, store) = evidence_and_rows(&input);

    let rows = turn_identities(&store, "root");
    assert_eq!(
        rows,
        vec![
            ("main".to_owned(), "root".to_owned(), None),
            (
                "delegated".to_owned(),
                "child".to_owned(),
                Some("child".to_owned())
            ),
            (
                "delegated".to_owned(),
                "child".to_owned(),
                Some("child".to_owned())
            ),
            (
                "delegated".to_owned(),
                "grandchild".to_owned(),
                Some("grandchild".to_owned())
            ),
            ("main".to_owned(), "root".to_owned(), None),
        ]
    );

    assert!(matches!(evidence.subagents, EvidenceValue::Complete(_)));
    assert!(
        matches!(evidence.cache, EvidenceValue::Complete(_)),
        "every row carries its message id, so the thread-identity claim holds: {:?}",
        evidence.cache
    );
    let subagents = observed(&evidence.subagents);
    assert_eq!(subagents.spawn_count, 2);
    assert!(subagents.delegated_models.contains("model-b"));

    let facts = store.query_turn_facts().expect("turn facts");
    assert!(
        facts.model_transitions.is_empty(),
        "the child's model switch must not read as a root transition"
    );
    assert_eq!(
        facts.longest_idle_gap_ms, 50_000,
        "the child's activity between the root's two messages must not split the root's own gap"
    );
}

#[test]
fn a_fork_is_its_own_root() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(
        &connection,
        "parent",
        None,
        Some("Investigate the outage"),
        10,
    );
    insert_session(
        &connection,
        "fork",
        None,
        Some("Investigate the outage (fork #1)"),
        50,
    );
    insert_message(&connection, "p1", "parent", 10, r#"{"role":"user"}"#);
    insert_message(
        &connection,
        "p2",
        "parent",
        20,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}"#,
    );
    insert_message(&connection, "p3", "parent", 30, r#"{"role":"user"}"#);
    // Copied prefix: the fork's first three messages carry new ids but keep
    // the parent's own `time_created`, older than the fork session itself.
    insert_message(&connection, "f1", "fork", 10, r#"{"role":"user"}"#);
    insert_message(
        &connection,
        "f2",
        "fork",
        20,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}"#,
    );
    insert_message(&connection, "f3", "fork", 30, r#"{"role":"user"}"#);
    insert_message(&connection, "f4", "fork", 60, r#"{"role":"user"}"#);
    drop(connection);

    let parent_input = sqlite_input(&path, "parent");
    let mut parent_collector = SessionCollector::new("opencode", "parent");
    adapter_for("opencode")
        .visit(&parent_input, &mut parent_collector)
        .expect("stream parent");
    let parent_session = parent_collector.into_session().expect("finished parent");
    assert_eq!(parent_session.events.len(), 3);
    assert!(
        parent_session
            .events
            .iter()
            .all(|event| event.thread_id.as_deref() == Some("parent"))
    );
    let (parent_evidence, _) = evidence_and_rows(&parent_input);
    assert_eq!(observed(&parent_evidence.subagents).spawn_count, 0);

    let fork_input = sqlite_input(&path, "fork");
    let mut fork_collector = SessionCollector::new("opencode", "fork");
    adapter_for("opencode")
        .visit(&fork_input, &mut fork_collector)
        .expect("stream fork");
    let fork_session = fork_collector.into_session().expect("finished fork");
    assert_eq!(fork_session.events.len(), 4);
    assert!(
        fork_session
            .events
            .iter()
            .all(|event| event.thread_id.as_deref() == Some("fork"))
    );
    let (fork_evidence, _) = evidence_and_rows(&fork_input);
    assert_eq!(observed(&fork_evidence.subagents).spawn_count, 0);
}

#[test]
fn a_fork_title_without_a_copied_prefix_is_an_ordinary_root() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(
        &connection,
        "lonely",
        None,
        Some("Refactor payments (fork #2)"),
        10,
    );
    insert_message(&connection, "m1", "lonely", 10, r#"{"role":"user"}"#);
    insert_message(
        &connection,
        "m2",
        "lonely",
        20,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}"#,
    );
    drop(connection);

    let input = sqlite_input(&path, "lonely");
    let (evidence, _) = evidence_and_rows(&input);

    assert_eq!(observed(&evidence.subagents).spawn_count, 0);
    assert!(matches!(evidence.subagents, EvidenceValue::Complete(_)));
}

#[test]
fn a_parent_id_child_with_a_fork_title_degrades_attribution() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, Some("Ship the release"), 10);
    insert_session(
        &connection,
        "child",
        Some("root"),
        Some("Ship the release (fork #1)"),
        20,
    );
    insert_message(
        &connection,
        "r1",
        "root",
        10,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}"#,
    );
    insert_message(&connection, "c1", "child", 20, r#"{"role":"user"}"#);
    insert_message(
        &connection,
        "c2",
        "child",
        30,
        r#"{"role":"assistant","modelID":"model-b","tokens":{"input":1,"output":1}}"#,
    );
    drop(connection);

    let input = sqlite_input(&path, "root");
    let (evidence, _) = evidence_and_rows(&input);

    match &evidence.subagents {
        EvidenceValue::Partial { reason, .. } => {
            assert_eq!(*reason, CoverageReason::AttributionIncomplete);
        }
        other => panic!("expected partial subagents evidence, got {other:?}"),
    }
}

#[test]
fn export_stream_marks_a_child_message_as_delegated_with_one_spawn() {
    let jsonl = concat!(
        r#"{"type":"session_meta","sessionID":"root","sessionRole":"root","time":{"created":1000},"payload":{"id":"root","title":"Root session"}}"#,
        "\n",
        r#"{"type":"session_member","rootSessionID":"root","originSessionID":"child","sessionRole":"child","parentSessionID":"root","time":{"created":1500},"payload":{"id":"child","title":"Child session"}}"#,
        "\n",
        r#"{"type":"message","rootSessionID":"root","sessionID":"root","sessionRole":"root","messageID":"m1","time":{"created":1000},"payload":{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}}"#,
        "\n",
        r#"{"type":"message","rootSessionID":"root","sessionID":"child","sessionRole":"child","parentSessionID":"root","messageID":"m2","time":{"created":1600},"payload":{"role":"user"}}"#,
        "\n",
    );
    let input = SessionInput {
        agent: "opencode".to_owned(),
        session_id: "root".to_owned(),
        source: RawSource::Jsonl(jsonl.to_owned()),
    };

    let mut collector = SessionCollector::new("opencode", "root");
    adapter_for("opencode")
        .visit(&input, &mut collector)
        .expect("stream export");
    let session = collector.into_session().expect("finished session");

    assert_eq!(session.events.len(), 2);
    assert_eq!(session.events[0].thread_id.as_deref(), Some("root"));
    assert_eq!(session.events[0].source, EventSource::Parent);
    assert_eq!(session.events[1].thread_id.as_deref(), Some("child"));
    assert_eq!(session.events[1].source, EventSource::Subagent);

    let (evidence, _) = evidence_and_rows(&input);
    assert_eq!(observed(&evidence.subagents).spawn_count, 1);
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
    insert_session(&connection, "root", None, None, 10);
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
    insert_session(&connection, "root", None, None, 100);
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
    let store = MemoryTurnRowStore::new("opencode", "evidence");
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        "evidence",
        None,
    );
    let mut sink = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = adapter_for("opencode")
        .visit(&input, &mut sink)
        .expect("stream export");
    sink.observe_source_outcome(outcome);
    let evidence = sink.evidence().expect("published evidence");
    let metrics = sink.metrics().expect("published metrics");

    assert_eq!(metrics.tokens_out, 5);
    assert_eq!(metrics.tokens_in, 15);
    assert_eq!(evidence.capabilities, SourceCapabilities::opencode());
    assert!(!json!(evidence).to_string().contains("PRIVATE_PATH"));
}
