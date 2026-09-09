use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, CoverageReason, EventSource, EvidenceSource, EvidenceValue, MemoryTurnRowStore,
    NormalizedRecord, PartialReason, RawSource, RecordSink, SessionCollector, SessionEvidence,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SessionSummary,
    SourceCapabilities, SourceKind, TurnRowSink, TurnRowStore, VisitOutcome, reader_for,
};
use antiburn_local::discovery::Explorers;
use antiburn_local::insights::{
    CoverageCounts, DetectorId, EfficiencyReportAccumulator, ReportContext, ReportWindow,
};
use antiburn_local::model::AgentKind;
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
        fork_parent_session_id: None,
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
        capabilities: reader_for("opencode").capabilities(&input.source),
    });
    let store = MemoryTurnRowStore::new(&input.agent, &input.session_id);
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        &input.session_id,
        None,
    );
    let mut sink = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = reader_for("opencode")
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
            source_format: antiburn_local::analysis::SourceFormat::OpenCodeJsonl,
            request_context_tokens: true,
            cache_write_tokens: true,
            timestamps_and_order: true,
            tool_invocations: true,
            skill_inventory: false,
            mcp_inventory: false,
            tool_definitions: false,
            model_identity: true,
            token_classes: true,
            reasoning_effort_tier: false,
            fast_tier: false,
            service_tier: false,
            subagent_relationships: true,
            subagent_models: true,
            compaction_boundaries: true,
            thread_identity: true,
            record_identity: false,
            linear_record_order: true,
            quota_incidents: false,
            harness_version: false,
            repeated_context_accounting: None,
        }
    );
}

#[test]
fn native_repeated_context_reaches_reports_without_capability_overrides() {
    use antiburn_local::analysis::RepeatedContextAccounting;

    for (provider, model, accounting) in [
        (
            "anthropic",
            "claude-sonnet-4",
            RepeatedContextAccounting::CacheWrite,
        ),
        ("openai", "gpt-5", RepeatedContextAccounting::UncachedInput),
    ] {
        for case in [
            "finding",
            "clean",
            "route_switch",
            "unknown_route",
            "compaction",
            "initial_compaction",
            "record_loss",
            "missing_time",
            "conflicting_time",
            "fork",
            "known_fork",
            "tied_time",
            "reversed_export",
            "unwrapped_export",
            "duplicate_export",
        ] {
            let (_directory, path) = create_database();
            let connection = Connection::open(&path).expect("database");
            let start = if case == "fork" { 15 } else { 1 };
            insert_session(&connection, "root", None, None, start);
            let mut export = vec![
                json!({"type":"session_meta","time":{"created":start},"payload":{"id":"root"}}),
            ];
            for index in 0..4 {
                let id = format!("m{index}");
                let timestamp = if case == "tied_time" {
                    10
                } else {
                    10 + index * 10
                };
                let mut message = if index == 0 {
                    json!({"role":"user"})
                } else {
                    let paid = if case == "finding" || case == "tied_time" {
                        1000
                    } else {
                        0
                    };
                    json!({"role":"assistant","parentID":"m0","modelID":model,"providerID":provider,
                        "tokens":{"input":if provider == "openai" { paid } else { 0 },"output":10,"reasoning":3,
                            "cache":{"read":1000,"write":if provider == "anthropic" { paid } else { 0 }}}})
                };
                if case == "route_switch" && index == 2 {
                    message["providerID"] = json!(if provider == "anthropic" {
                        "openai"
                    } else {
                        "anthropic"
                    });
                }
                if case == "unknown_route" && index > 0 {
                    message["providerID"] = json!("custom-proxy");
                }
                if case == "conflicting_time" && index == 2 {
                    message["time"] = json!({"created":99});
                }
                insert_message(&connection, &id, "root", timestamp, &message.to_string());
                export.push(json!({"type":"message","sessionID":"root","messageID":id,"time":{"created":timestamp},"payload":message}));
                if case == "missing_time" && index == 2 {
                    connection
                        .execute(
                            "UPDATE message SET time_created = NULL WHERE id = ?1",
                            [&id],
                        )
                        .expect("missing creation time");
                    export.last_mut().unwrap()["time"]["created"] = json!(null);
                }
                if case == "record_loss" && index == 2 {
                    connection
                        .execute("UPDATE message SET data = '{' WHERE id = ?1", [&id])
                        .expect("malformed message");
                    *export.last_mut().unwrap() =
                        json!({"type":"message","messageID":id,"payload":null});
                }
                if (case == "compaction" && index == 2)
                    || (case == "initial_compaction" && index == 0)
                {
                    let part = json!({"type":"compaction","auto":true});
                    insert_part(
                        &connection,
                        "compact",
                        &id,
                        "root",
                        timestamp,
                        &part.to_string(),
                    );
                    export.push(json!({"type":"part","messageID":id,"payload":part}));
                }
            }
            drop(connection);
            if case == "reversed_export" {
                export.swap(2, 3);
            }
            if case == "unwrapped_export" {
                export.remove(0);
            }
            if case == "duplicate_export" {
                export[4]["messageID"] = json!("m1");
            }
            for source in [
                RawSource::Sqlite(path.clone()),
                RawSource::Jsonl(
                    export
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            ] {
                let export_only_gap = matches!(source, RawSource::Jsonl(_))
                    && matches!(
                        case,
                        "reversed_export" | "unwrapped_export" | "duplicate_export"
                    );
                let input = SessionInput {
                    source,
                    fork_parent_session_id: (case == "known_fork").then(|| "parent".to_owned()),
                    ..sqlite_input(&path, "root")
                };
                let (evidence, store) = evidence_and_rows(&input);
                let cache = observed(&evidence.cache);
                let incomplete = export_only_gap
                    || matches!(
                        case,
                        "route_switch"
                            | "compaction"
                            | "initial_compaction"
                            | "record_loss"
                            | "missing_time"
                            | "conflicting_time"
                            | "fork"
                            | "known_fork"
                    );
                if case == "unknown_route" {
                    assert_eq!(cache.repeated_context, EvidenceValue::Unsupported);
                } else {
                    assert_eq!(
                        matches!(cache.repeated_context, EvidenceValue::Partial { .. }),
                        incomplete,
                        "{provider}: {case}: {:?}",
                        cache.repeated_context
                    );
                    let repeated = observed(&cache.repeated_context);
                    assert_eq!(repeated.accounting, accounting, "{provider}: {case}");
                    if matches!(case, "finding" | "tied_time") {
                        assert_eq!(repeated.pairs_considered, 2);
                        assert_eq!(repeated.repeated_tokens, 2000);
                        assert_eq!(repeated.paid_tokens, 2000);
                    }
                }
                store.with_connection(|connection| {
                    let route: (String, Option<String>, Option<String>, i64) = connection.query_row(
                        "SELECT provider, api, parent_uuid, output_tokens FROM turn WHERE role = 'assistant' ORDER BY turn_index LIMIT 1",
                        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    ).expect("request route");
                    assert_eq!(route.0, if case == "unknown_route" { "custom-proxy" } else { provider });
                    assert_eq!(route.1, None);
                    assert_eq!(route.2, None);
                    assert_eq!(route.3, 13);
                });
                let mut report = EfficiencyReportAccumulator::new();
                report.observe_session(evidence);
                let report = report.finish(ReportContext {
                    environment_key: "native".to_owned(),
                    window: ReportWindow {
                        start_epoch: 0,
                        end_epoch: 100,
                    },
                    computed_at_epoch: 100,
                    parser_revision: antiburn_local::analysis::PARSER_REVISION,
                    analyzer_revision: antiburn_local::analysis::ANALYZER_REVISION,
                    evidence_schema_revision: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                    coverage: CoverageCounts::default(),
                });
                let counts = report.detectors[DetectorId::CacheChurn.index()];
                if matches!(case, "finding" | "tied_time") {
                    assert_eq!(counts.finding, 1, "{provider}: {case}");
                } else if incomplete || case == "unknown_route" {
                    assert_eq!(counts.clean, 0, "{provider}: {case}");
                    assert_eq!(counts.assessed, 0, "{provider}: {case}");
                } else {
                    assert_eq!(counts.clean, 1, "{provider}: {case}");
                }
            }
        }
    }
}

#[test]
fn completed_skill_results_publish_identity_without_inventory() {
    // Official packages/opencode/src/tool/skill.ts defines both pinned output formats.
    for (revision, base) in [
        (
            "ffc000de8e446c63d41a2e352d119d9ff43530d0",
            "file:///PRIVATE_DIR",
        ),
        ("ecbc6ccac85b3e8087b6445e584318419b9e2b34", "/PRIVATE_DIR"),
    ] {
        for case in [
            "complete",
            "pending",
            "running",
            "error",
            "missing_output",
            "missing_name",
            "wrong_name",
            "empty_body",
            "redacted",
            "truncated",
            "metadata_truncated",
            "compacted",
            "unclosed",
            "invocation_only",
            "user",
        ] {
            let output = format!(
                "<skill_content name=\"review\">\n# Skill: review\n\nPRIVATE_BODY\n\nBase directory for this skill: {base}\nRelative paths in this skill (e.g., scripts/, reference/) are relative to this base directory.\nNote: file list is sampled.\n\n<skill_files>\n<file>/PRIVATE_FILE</file>\n</skill_files>\n</skill_content>"
            );
            let mut part = json!({"type":"tool","tool":"skill","callID":"skill-call","state":{
                "status":"completed", "input":{"name":"review"},
                "metadata":{"name":"review","dir":"/PRIVATE_DIR","truncated":false},
                "time":{"start":11,"end":12}, "output":output
            }});
            match case {
                "pending" | "running" | "error" => part["state"]["status"] = json!(case),
                "missing_output" | "invocation_only" => part["state"]["output"] = json!(null),
                "missing_name" => part["state"]["metadata"]["name"] = json!(null),
                "wrong_name" => part["state"]["metadata"]["name"] = json!("other"),
                "empty_body" => part["state"]["output"] = json!(output.replace("PRIVATE_BODY", "")),
                "redacted" | "truncated" => {
                    part["state"]["output"] =
                        json!(output.replace("PRIVATE_BODY", &format!("[{case}]")))
                }
                "metadata_truncated" => part["state"]["metadata"]["truncated"] = json!(true),
                "compacted" => part["state"]["time"]["compacted"] = json!(20),
                "unclosed" => {
                    part["state"]["output"] = json!(output.replace("</skill_content>", ""))
                }
                _ => {}
            }
            let message = json!({"role":if case == "user" { "user" } else { "assistant" }, "modelID":"model-a", "tokens":{"input":1,"output":1}});
            let (_directory, path) = create_database();
            let connection = Connection::open(&path).expect("database");
            insert_session(&connection, "root", None, None, 10);
            insert_message(&connection, "m1", "root", 10, &message.to_string());
            insert_part(&connection, "p1", "m1", "root", 11, &part.to_string());
            drop(connection);
            let export = [
                json!({"type":"message","messageID":"m1","sessionID":"root","time":{"created":10},"payload":message}),
                json!({"type":"part","messageID":"m1","payload":part}),
            ].iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
            for source in [RawSource::Sqlite(path.clone()), RawSource::Jsonl(export)] {
                let input = SessionInput {
                    source,
                    ..sqlite_input(&path, "root")
                };
                let (evidence, _) = evidence_and_rows(&input);
                assert!(!evidence.capabilities.skill_inventory);
                if case == "complete" {
                    let sources = observed(&evidence.context_sources);
                    let expected = match &input.source {
                        RawSource::Sqlite(_) => EvidenceValue::Complete(()),
                        _ => EvidenceValue::Partial {
                            observed: (),
                            reason: CoverageReason::AttributionIncomplete,
                        },
                    };
                    assert_eq!(sources.skill_coverage, expected, "{revision}");
                    assert_eq!(sources.skills.len(), 1);
                    let skill = &sources.skills["review"];
                    assert!(skill.injected && skill.invoked);
                    assert!(skill.description.is_none());
                    assert!(skill.token_count.is_none());
                    assert_eq!(skill.origin, EvidenceValue::Unsupported);
                } else {
                    assert_eq!(
                        evidence.context_sources,
                        EvidenceValue::Unsupported,
                        "{revision}: {case}"
                    );
                }
                let session = reader_for("opencode").normalize(&input).expect("normalize");
                let retained = format!("{}{}", json!(evidence), json!(session));
                for private in ["PRIVATE_BODY", "PRIVATE_DIR", "PRIVATE_FILE"] {
                    assert!(!retained.contains(private), "{revision}: {case}: {private}");
                }
                let mut report = EfficiencyReportAccumulator::new();
                report.observe_session(evidence);
                let report = report.finish(ReportContext {
                    environment_key: "native".to_owned(),
                    window: ReportWindow {
                        start_epoch: 0,
                        end_epoch: 100,
                    },
                    computed_at_epoch: 100,
                    parser_revision: antiburn_local::analysis::PARSER_REVISION,
                    analyzer_revision: antiburn_local::analysis::ANALYZER_REVISION,
                    evidence_schema_revision: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                    coverage: CoverageCounts::default(),
                });
                let counts = report.detectors[DetectorId::UnusedSkills.index()];
                assert_eq!(counts.finding, 0, "{revision}: {case}");
                assert_eq!(counts.assessed, 0, "{revision}: {case}");
                assert_eq!(counts.clean, 0, "{revision}: {case}");
            }
        }
    }
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
    reader_for("opencode")
        .visit(&input, &mut collector)
        .expect("stream database");
    let session = collector.into_session().expect("finished session");

    assert_eq!(session.events.len(), 3);
    assert_eq!(session.events[0].role, antiburn_local::analysis::Role::User);
    // Ancestry separates child work but does not prove a native spawn.
    assert_eq!(session.events[0].thread_id.as_deref(), Some("child"));
    assert_eq!(session.events[0].source, EventSource::Subagent);
    let child_assistant = &session.events[1];
    assert_eq!(child_assistant.model.as_deref(), Some("model-child"));
    assert_eq!(child_assistant.tools.len(), 1);
    assert_eq!(child_assistant.thread_id.as_deref(), Some("child"));
    assert_eq!(child_assistant.source, EventSource::Subagent);
    let assistant = &session.events[2];
    assert_eq!(assistant.thinking_mode.as_deref(), Some("high"));
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

    let (evidence, store) = evidence_and_rows(&input);
    assert!(matches!(
        evidence.subagents,
        EvidenceValue::Partial {
            reason: CoverageReason::AttributionIncomplete,
            ..
        }
    ));
    assert_eq!(observed(&evidence.subagents).spawn_count, 0);
    assert!(
        observed(&evidence.subagents)
            .delegated_models
            .contains("model-child")
    );
    let rows = turn_identities(&store, "root");
    assert_eq!(rows[0].0, "delegated");
    assert_eq!(rows[1].0, "delegated");
    assert_eq!(rows[2].0, "main");

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
    // The task metadata follows OpenCode v1.2.0 packages/opencode/src/tool/task.ts.
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
    insert_part(
        &connection,
        "task-child",
        "r1",
        "root",
        11,
        r#"{"type":"tool","tool":"task","callID":"call-child","state":{"status":"completed","input":{"description":"Inspect code","prompt":"Inspect code","subagent_type":"explore"},"metadata":{"sessionId":"child","model":{"providerID":"test","modelID":"model-b"}},"time":{"start":11,"end":40},"output":"Complete"}}"#,
    );
    insert_part(
        &connection,
        "task-grandchild",
        "c2",
        "child",
        31,
        r#"{"type":"tool","tool":"task","callID":"call-grandchild","state":{"status":"completed","input":{"description":"Inspect tests","prompt":"Inspect tests","subagent_type":"explore"},"metadata":{"sessionId":"grandchild","model":{"providerID":"test","modelID":"model-c"}},"time":{"start":31,"end":40},"output":"Complete"}}"#,
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
    assert!(subagents.delegated_models.contains("model-c"));
    assert_eq!(
        subagents.children[0].parent_model.as_deref(),
        Some("model-a")
    );
    assert_eq!(
        subagents.children[1].parent_model.as_deref(),
        Some("model-b")
    );
    assert!(
        subagents
            .children
            .iter()
            .all(|child| child.provenance
                == antiburn_local::analysis::RelationProvenance::TaskToolUse)
    );

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
    reader_for("opencode")
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
    reader_for("opencode")
        .visit(&fork_input, &mut fork_collector)
        .expect("stream fork");
    let fork_session = fork_collector.into_session().expect("finished fork");
    assert_eq!(fork_session.events.len(), 4);
    assert!(
        fork_session
            .events
            .iter()
            .all(|event| event.thread_id.as_deref() == Some("fork")
                && event.source == EventSource::Parent)
    );
    let (fork_evidence, _) = evidence_and_rows(&fork_input);
    assert_eq!(observed(&fork_evidence.subagents).spawn_count, 0);
}

#[test]
fn native_task_requires_child_identity_and_both_models() {
    for missing in [
        "parent_model",
        "task_model",
        "child_model",
        "child_identity",
        "wrong_identity",
        "wrong_model",
        "wrong_parent",
        "pending",
        "subtask",
        "ancestry",
        "metadata",
    ] {
        let (_directory, path) = create_database();
        let connection = Connection::open(&path).expect("database");
        insert_session(&connection, "root", None, None, 10);
        insert_session(&connection, "child", Some("root"), None, 20);
        let mut parent =
            json!({"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}});
        let mut child =
            json!({"role":"assistant","modelID":"model-b","tokens":{"input":2,"output":3}});
        let mut task = json!({"type":"tool","tool":"task","callID":"call-child","state":{
            "status":"completed","input":{"description":"Inspect code","prompt":"Inspect code","subagent_type":"explore"},
            "metadata":{"sessionId":"child","model":{"providerID":"test","modelID":"model-b"}},
            "time":{"start":11,"end":40},"output":"Complete"
        }});
        match missing {
            "parent_model" => parent["modelID"] = json!(null),
            "task_model" => task["state"]["metadata"]["model"] = json!(null),
            "child_model" => child["modelID"] = json!(null),
            "child_identity" => task["state"]["metadata"]["sessionId"] = json!(null),
            "wrong_identity" => task["state"]["metadata"]["sessionId"] = json!("other-child"),
            "wrong_model" => task["state"]["metadata"]["model"]["modelID"] = json!("other-model"),
            "wrong_parent" => task["state"]["metadata"]["parentSessionId"] = json!("other-parent"),
            "pending" => task["state"]["status"] = json!("pending"),
            "subtask" => {
                parent = json!({"role":"user","model":{"providerID":"test","modelID":"model-a"}});
                task = json!({"type":"subtask","prompt":"Inspect code","description":"Inspect code","agent":"explore","model":{"providerID":"test","modelID":"model-b"}});
            }
            "ancestry" => task = json!({"type":"text","text":"Inspect code"}),
            "metadata" => task["state"]["metadata"] = json!(null),
            _ => unreachable!(),
        }
        insert_message(&connection, "r1", "root", 10, &parent.to_string());
        insert_part(&connection, "task", "r1", "root", 11, &task.to_string());
        insert_message(&connection, "c1", "child", 30, &child.to_string());
        drop(connection);

        let export = [
            json!({"type":"session_meta","payload":{"id":"root"}}),
            json!({"type":"session_member","originSessionID":"child","parentSessionID":"root","payload":{"id":"child"}}),
            json!({"type":"message","sessionID":"root","messageID":"r1","time":{"created":10},"payload":parent}),
            json!({"type":"part","messageID":"r1","payload":task}),
            json!({"type":"message","sessionID":"child","messageID":"c1","time":{"created":30},"payload":child}),
        ].iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        for source in [RawSource::Sqlite(path.clone()), RawSource::Jsonl(export)] {
            let input = SessionInput {
                source,
                ..sqlite_input(&path, "root")
            };
            let (evidence, store) = evidence_and_rows(&input);
            assert!(
                matches!(
                    evidence.subagents,
                    EvidenceValue::Partial {
                        reason: CoverageReason::AttributionIncomplete,
                        ..
                    }
                ),
                "{missing}: {:?}",
                evidence.subagents
            );
            let subagents = observed(&evidence.subagents);
            assert_eq!(subagents.spawn_count, 0, "{missing}");
            assert_eq!(subagents.delegated_turns, 1, "{missing}");
            assert!(subagents.children.is_empty(), "{missing}");
            assert_eq!(
                subagents.delegated_models.is_empty(),
                missing == "child_model",
                "{missing}"
            );
            assert_eq!(
                turn_identities(&store, "root"),
                vec![
                    ("main".to_owned(), "root".to_owned(), None),
                    (
                        "delegated".to_owned(),
                        "child".to_owned(),
                        Some("child".to_owned())
                    ),
                ],
                "{missing}"
            );
            let session = reader_for("opencode").normalize(&input).expect("normalize");
            assert_eq!(session.events.len(), 2, "{missing}");
            assert_eq!(session.events[1].usage.input_tokens, 2, "{missing}");
            assert_eq!(session.events[1].usage.output_tokens, 3, "{missing}");
        }
    }
}

#[test]
fn child_model_changes_and_missing_tasks_do_not_pollute_parent_totals() {
    for case in [
        "native",
        "model_switch",
        "wrong_model",
        "no_metadata",
        "no_task",
    ] {
        let (_directory, path) = create_database();
        let connection = Connection::open(&path).expect("database");
        insert_session(&connection, "root", None, None, 10);
        insert_session(&connection, "child", Some("root"), None, 20);
        let mut export = vec![
            json!({"type":"session_meta","time":{"created":10},"payload":{"id":"root"}}),
            json!({"type":"session_member","originSessionID":"child","parentSessionID":"root","payload":{"id":"child"}}),
        ];
        for (index, session_id) in ["root", "child", "child", "root"].iter().enumerate() {
            let id = format!("m{index}");
            let timestamp = 10 + index as i64 * 10;
            let model = if case == "model_switch" && index == 1 {
                "claude-sonnet-4-6"
            } else {
                "claude-opus-4-6"
            };
            let message = json!({"role":"assistant","modelID":model,"providerID":"anthropic",
                "tokens":{"input":if *session_id == "root" { 1 } else { 200_000 },"output":1,
                    "cache":{"read":0,"write":if *session_id == "root" { 0 } else { 200_000 }}}});
            insert_message(
                &connection,
                &id,
                session_id,
                timestamp,
                &message.to_string(),
            );
            export.push(json!({"type":"message","sessionID":session_id,"messageID":id,"time":{"created":timestamp},"payload":message}));
            if index == 0 && case != "no_task" {
                let mut task = json!({"type":"tool","tool":"task","state":{"status":"completed",
                    "metadata":{"sessionId":"child","model":{"modelID":if matches!(case, "model_switch" | "wrong_model") { "claude-sonnet-4-6" } else { model }}}}});
                if case == "no_metadata" {
                    task["state"]["metadata"] = json!(null);
                }
                insert_part(
                    &connection,
                    "task",
                    &id,
                    session_id,
                    timestamp,
                    &task.to_string(),
                );
                export.push(json!({"type":"part","messageID":id,"payload":task}));
            }
        }
        drop(connection);
        for source in [
            RawSource::Sqlite(path.clone()),
            RawSource::Jsonl(
                export
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ] {
            let input = SessionInput {
                source,
                ..sqlite_input(&path, "root")
            };
            let (evidence, store) = evidence_and_rows(&input);
            let facts = store.query_turn_facts().expect("turn facts");
            let parent_totals: (i64, i64, i64) = store.with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*), SUM(input_tokens), SUM(cache_write_tokens) FROM turn WHERE scope = 'main'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                ).expect("parent totals")
            });
            assert_eq!(parent_totals, (2, 2, 0), "{case}");
            assert_eq!(facts.repeated_context_cache_write_tokens, 0, "{case}");
            assert_eq!(facts.delegated_turns, 2, "{case}");
            assert!(facts.model_transitions.is_empty(), "{case}");
            let subagents = observed(&evidence.subagents);
            let paired = matches!(case, "native" | "model_switch");
            assert_eq!(subagents.spawn_count, u64::from(paired), "{case}");
            assert_eq!(subagents.children.len(), usize::from(paired), "{case}");
            assert_eq!(
                matches!(evidence.subagents, EvidenceValue::Partial { .. }),
                case != "native",
                "{case}"
            );
            let mut report = EfficiencyReportAccumulator::new();
            report.observe_session(evidence);
            let report = report.finish(ReportContext {
                environment_key: "native".to_owned(),
                window: ReportWindow {
                    start_epoch: 0,
                    end_epoch: 100,
                },
                computed_at_epoch: 100,
                parser_revision: antiburn_local::analysis::PARSER_REVISION,
                analyzer_revision: antiburn_local::analysis::ANALYZER_REVISION,
                evidence_schema_revision: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                coverage: CoverageCounts::default(),
            });
            let counts = report.detectors[DetectorId::OverpoweredSubagents.index()];
            assert_eq!(counts.finding, u64::from(case == "native"), "{case}");
            if case != "native" {
                assert_eq!(counts.clean, 0, "{case}");
                assert_eq!(counts.assessed, 0, "{case}");
            }
        }
    }
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
    insert_part(
        &connection,
        "task-child",
        "r1",
        "root",
        11,
        r#"{"type":"tool","tool":"task","callID":"call-child","state":{"status":"completed","input":{"description":"Inspect code","prompt":"Inspect code","subagent_type":"explore"},"metadata":{"sessionId":"child","model":{"providerID":"test","modelID":"model-b"}},"time":{"start":11,"end":40},"output":"Complete"}}"#,
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
    assert_eq!(observed(&evidence.subagents).spawn_count, 0);
    assert!(observed(&evidence.subagents).delegated_models.is_empty());
    let session = reader_for("opencode").normalize(&input).expect("normalize");
    assert!(
        session
            .events
            .iter()
            .all(|event| event.source == EventSource::Parent)
    );
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
        r#"{"type":"part","messageID":"m1","payload":{"type":"tool","tool":"task","callID":"call-child","state":{"status":"completed","input":{"description":"Inspect code","prompt":"Inspect code","subagent_type":"explore"},"metadata":{"sessionId":"child","model":{"providerID":"test","modelID":"model-b"}},"time":{"start":1500,"end":1700},"output":"Complete"}}}"#,
        "\n",
        r#"{"type":"message","rootSessionID":"root","sessionID":"child","sessionRole":"child","parentSessionID":"root","messageID":"m2","time":{"created":1600},"payload":{"role":"assistant","modelID":"model-b","tokens":{"input":2,"output":1}}}"#,
        "\n",
    );
    let input = SessionInput {
        agent: "opencode".to_owned(),
        session_id: "root".to_owned(),
        source: RawSource::Jsonl(jsonl.to_owned()),
        fork_parent_session_id: None,
    };

    let mut collector = SessionCollector::new("opencode", "root");
    reader_for("opencode")
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
    insert_session(&connection, "root", None, None, 10);
    insert_message(&connection, "message", "root", 20, r#"{"role":"user"}"#);
    drop(connection);
    antiburn_local::discovery::track_provider_db_renders(&path);

    let input = sqlite_input(&path, "root");
    let mut collector = SessionCollector::new("opencode", "root");
    reader_for("opencode")
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
    reader_for("opencode")
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

#[tokio::test]
async fn database_claim_is_checked_inside_the_snapshot() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, None, 100);
    insert_message(&connection, "message", "root", 120, r#"{"role":"user"}"#);
    drop(connection);
    let input = sqlite_input(&path, "root");
    let adapter = reader_for("opencode");

    let mut mismatch = SessionCollector::new("opencode", "root");
    let outcome = adapter
        .visit_db_claimed(&input, "sv1:db:0:0", &|| false, &mut mismatch)
        .expect("check mismatch");
    assert!(matches!(outcome, VisitOutcome::SourceChanged(_)));

    let (latest, rows) = Explorers::DISK
        .provider_db_fingerprint(&AgentKind::OpenCode, &path, "root")
        .await
        .expect("database fingerprint");
    let fingerprint = format!("sv1:db:{latest}:{rows}");
    let mut matching = SessionCollector::new("opencode", "root");
    let outcome = adapter
        .visit_db_claimed(&input, &fingerprint, &|| false, &mut matching)
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
        fork_parent_session_id: None,
    };
    let mut sink = CountingSink {
        records: 0,
        finished: false,
    };
    reader_for("opencode")
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
        fork_parent_session_id: None,
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
    let outcome = reader_for("opencode")
        .visit(&input, &mut sink)
        .expect("stream export");
    sink.observe_source_outcome(outcome);
    let evidence = sink.evidence().expect("published evidence");
    let metrics = sink.metrics().expect("published metrics");

    assert_eq!(metrics.tokens_out, 5);
    assert_eq!(metrics.tokens_in, 15);
    assert_eq!(evidence.capabilities, SourceCapabilities::opencode());
    let models = match &evidence.models {
        EvidenceValue::Complete(models)
        | EvidenceValue::Partial {
            observed: models, ..
        } => models,
        EvidenceValue::Unsupported => panic!("expected model evidence"),
    };
    assert_eq!(models.control_observations.len(), 1);
    assert_eq!(
        models.control_observations[0].effort.as_deref(),
        Some("high")
    );
    assert!(!json!(evidence).to_string().contains("PRIVATE_PATH"));
}
