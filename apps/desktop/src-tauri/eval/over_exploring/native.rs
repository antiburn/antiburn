use std::sync::Arc;

use antiburn_local::analysis::jev::JevCheck;
use antiburn_local::analysis::jev_evidence::{
    JevReadStatus, prepare_session_content, select_session_content,
};
use antiburn_local::analysis::*;
use antiburn_local::checks::over_exploring::*;
use rusqlite::Connection;
use serde_json::{Value, json};

fn evidence(
    agent: &str,
    source_format: SourceFormat,
    source: RawSource,
) -> antiburn_local::analysis::jev_evidence::SessionContentEvidence {
    let input = SessionInput {
        agent: agent.into(),
        session_id: "native-oe".into(),
        source,
        fork_parent_session_id: None,
        source_format,
    };
    let reader = reader_for(agent);
    let store = MemoryTurnRowStore::new(agent, "native-oe");
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new(agent, "native-oe"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: agent.into(),
            session_id: "native-oe".into(),
            kind: SourceKind::from(&input.source),
            capabilities: reader.capabilities(&input),
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            "native-oe",
            None,
        ),
    );
    let outcome = reader
        .visit(&input, &mut sink)
        .expect("Native fixture reader visits the synthetic source");
    sink.observe_source_outcome(outcome);
    store.with_connection(|connection| {
        let published = query_turn_content_offset_selected(
            connection,
            &TurnSessionKey {
                environment_key: "native",
                agent,
                session_id: "native-oe",
            },
            &FenceScope::single(1),
            None,
            &Default::default(),
            0,
            OverExploringCheck.input_selection(),
        )
        .expect("Native fixture publishes selected turn content");
        select_session_content(
            &prepare_session_content("native-oe", source_format, published, Vec::new()),
            OverExploringCheck.input_selection(),
        )
    })
}

fn records(agent: &str, output: &str) -> Vec<Value> {
    let args =
        json!({"path":"lib/conversion.py","file_path":"lib/conversion.py","offset":1,"limit":3});
    let mut records = match agent {
        "claude" => vec![
            json!({"type":"assistant","uuid":"request-record","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"read-call","name":"Read","input":args}]}}),
            json!({"type":"user","uuid":"result-record","parentUuid":"request-record","timestamp":"2026-01-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"read-call","content":output}]}}),
        ],
        "codex" => vec![
            json!({"type":"session_meta","payload":{"id":"native-oe","cwd":"/synthetic"}}),
            json!({"type":"response_item","payload":{"type":"function_call","call_id":"read-call","name":"read_file","arguments":args.to_string()}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"read-call","output":output}}),
        ],
        "pi" => vec![
            json!({"type":"session","version":3,"id":"native-oe","timestamp":"2026-01-01T00:00:00Z","cwd":"/synthetic"}),
            json!({"type":"message","id":"request-record","parentId":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"read-call","name":"read","arguments":args}]}}),
            json!({"type":"message","id":"result-record","parentId":"request-record","timestamp":"2026-01-01T00:00:02Z","message":{"role":"toolResult","toolCallId":"read-call","toolName":"read","isError":false,"content":[{"type":"text","text":output}]}}),
        ],
        "cursor" => vec![
            json!({"sessionId":"native-oe","cursor_source":"agent_transcript"}),
            json!({"role":"assistant","id":"request-record","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool-use","id":"read-call","name":"read_file","input":args}]}}),
            json!({"role":"tool","id":"result-record","timestamp":"2026-01-01T00:00:02Z","tool_call_id":"read-call","name":"read_file","content":output}),
        ],
        "antigravity" => vec![
            json!({"type":"PLANNER_RESPONSE","step_index":1,"content":"Inspect the conversion.","tool_calls":[{"name":"read_file","args":args}]}),
            json!({"type":"TOOL_RESPONSE","step_index":2,"tool_name":"read_file","content":output}),
        ],
        _ => unreachable!(),
    };
    for (index, record) in records.iter_mut().enumerate() {
        record["timestamp"] = json!(format!("2026-01-01T00:00:{index:02}Z"));
    }
    records
}

#[test]
fn six_native_source_boundaries_do_not_inherit_opencode_observed_extents() {
    let output = "<path>lib/conversion.py</path>\n<type>file</type>\n<content>\n1: def minutes(seconds):\n2:     return seconds / 60\n3: # Preserve fractional minutes.\n\n(End of file - total 3 lines)\n</content>";
    for (agent, format) in [
        ("opencode", SourceFormat::OpenCodeSqliteV2),
        ("claude", SourceFormat::ClaudeJsonl),
        ("codex", SourceFormat::CodexRolloutJsonl),
        ("pi", SourceFormat::PiV3Jsonl),
        ("cursor", SourceFormat::CursorCliAgentJsonl),
        ("antigravity", SourceFormat::AntigravityBrainJsonl),
    ] {
        let directory = tempfile::tempdir().expect("Native fixture temporary directory is created");
        let source = if agent == "opencode" {
            let path = directory.path().join("opencode.db");
            let connection = Connection::open(&path).expect("Native fixture database opens");
            connection.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY); CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, data TEXT); CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, data TEXT); INSERT INTO session VALUES ('native-oe');").expect("Native fixture schema and session are created");
            connection
                .execute(
                    "INSERT INTO message VALUES ('request-record', 'native-oe', ?1)",
                    [json!({"role":"assistant","time":{"created":1000}}).to_string()],
                )
                .expect("Native fixture request message is inserted");
            connection.execute("INSERT INTO part VALUES ('read-part', 'request-record', ?1)", [json!({"type":"tool","tool":"read","callID":"read-call","state":{"status":"completed","input":{"filePath":"lib/conversion.py","offset":1,"limit":3},"output":output}}).to_string()]).expect("Native fixture read part is inserted");
            RawSource::Sqlite(path)
        } else {
            RawSource::Jsonl(
                records(agent, output)
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n",
            )
        };
        let content = evidence(agent, format, source);
        let reads = content
            .actions
            .iter()
            .filter_map(|action| action.metadata.read_request.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(
            reads.len(),
            1,
            "{agent}: native request must reach projection"
        );
        let results = content
            .actions
            .iter()
            .filter_map(|action| action.metadata.read_result.as_ref())
            .collect::<Vec<_>>();
        if agent == "opencode" {
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].status, JevReadStatus::Success);
            assert_eq!(
                results[0]
                    .returned_extent
                    .as_ref()
                    .expect("OpenCode fixture result retains its observed read extent")
                    .limit,
                Some(3)
            );
        } else if agent == "pi" {
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].status, JevReadStatus::Success);
            assert!(
                results[0].returned_extent.is_none(),
                "Pi status comes from isError; formatted text does not prove read bounds"
            );
        } else {
            assert!(
                results
                    .iter()
                    .all(|result| result.status == JevReadStatus::Unknown
                        && result.returned_extent.is_none()),
                "{agent}: an OpenCode-looking payload cannot establish accepted source support"
            );
        }
    }
}
