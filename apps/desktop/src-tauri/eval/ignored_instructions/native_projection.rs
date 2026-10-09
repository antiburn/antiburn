use antiburn_local::analysis::jev::{JevCheck, JevInputField};
use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, FenceScope, MemoryTurnRowStore, RawSource,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SourceFormat, SourceKind,
    TurnRowSink, TurnRowStore, TurnSessionKey, query_turn_content_offset_selected, reader_for,
};
use antiburn_local::checks::ignored_instructions::{
    IgnoredInstructionsCheck, prepare_session_content, select_session_content,
};
use serde_json::{Value, json};
use std::sync::Arc;

fn records(agent: &str) -> Vec<Value> {
    let user = "Inspect the queue.";
    let result = "Finished checks successfully.";
    match agent {
        "claude" => vec![
            json!({"type":"user","uuid":"u0","parentUuid":null,"sessionId":"native-eval","version":"2.1.278","entrypoint":"cli","isSidechain":false,"origin":{"kind":"human"},"promptSource":"typed","turnOrigin":"human","message":{"role":"user","content":[{"type":"text","text":user}]}}),
            json!({"type":"assistant","uuid":"a0","parentUuid":"u0","sessionId":"native-eval","version":"2.1.278","entrypoint":"cli","isSidechain":false,"message":{"role":"assistant","content":[{"type":"tool_use","id":"c0","name":"Bash","input":{"command":"cargo check"}}]}}),
            json!({"type":"user","uuid":"r0","parentUuid":"a0","sessionId":"native-eval","version":"2.1.278","entrypoint":"cli","isSidechain":false,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"c0","content":result,"is_error":false}]},"toolUseResult":{"stdout":result,"stderr":"","interrupted":false,"isImage":false,"noOutputExpected":false}}),
        ],
        "codex" => vec![
            json!({"type":"session_meta","payload":{"id":"native-eval","cwd":"/synthetic"}}),
            json!({"type":"response_item","payload":{"type":"message","id":"u0","role":"user","content":[{"type":"input_text","text":user}]}}),
            json!({"type":"response_item","payload":{"type":"function_call","call_id":"c0","name":"bash","arguments":"{\"command\":\"cargo check\"}"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"c0","output":result}}),
        ],
        "pi" => vec![
            json!({"type":"session","version":3,"id":"native-eval","cwd":"/synthetic","timestamp":"2026-01-01T00:00:00Z"}),
            json!({"type":"message","id":"u0","parentId":null,"message":{"role":"user","content":[{"type":"text","text":user}]}}),
            json!({"type":"message","id":"a0","parentId":"u0","message":{"role":"assistant","content":[{"type":"toolCall","id":"c0","name":"bash","arguments":{"command":"cargo check"}}]}}),
            json!({"type":"message","id":"r0","parentId":"a0","message":{"role":"toolResult","toolCallId":"c0","toolName":"bash","isError":false,"content":[{"type":"text","text":result}]}}),
        ],
        _ => panic!("JSON records require a JSONL agent"),
    }
}

fn opencode(path: &std::path::Path) -> RawSource {
    let database = rusqlite::Connection::open(path).expect("temporary OpenCode database");
    database.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY); CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, data TEXT, time_created INTEGER); CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, data TEXT); INSERT INTO session VALUES ('native-eval');").expect("source schema");
    for (index, (id, role, part)) in [
        (
            "u0",
            "user",
            json!({"type":"text","text":"Inspect the queue."}),
        ),
        (
            "a0",
            "assistant",
            json!({"type":"tool","tool":"bash","callID":"c0","state":{"status":"completed","input":{"command":"cargo check"},"output":"Finished checks successfully."}}),
        ),
    ].into_iter().enumerate() {
        database
            .execute(
                "INSERT INTO message VALUES (?1,'native-eval',?2,?3)",
                rusqlite::params![id, json!({"role":role,"time":{"created":1000+index}}).to_string(),1000+index],
            )
            .expect("native message");
        database
            .execute(
                "INSERT INTO part VALUES (?1,?1,?2)",
                rusqlite::params![id, part.to_string()],
            )
            .expect("native part");
    }
    RawSource::Sqlite(path.to_owned())
}

#[test]
fn four_native_agents_project_human_command_and_exact_output_through_store() {
    for (agent, format) in [
        ("claude", SourceFormat::ClaudeJsonl),
        ("codex", SourceFormat::CodexRolloutJsonl),
        ("pi", SourceFormat::PiV3Jsonl),
        ("opencode", SourceFormat::OpenCodeSqliteV2),
    ] {
        let directory = tempfile::tempdir().expect("temporary native fixture");
        let source = if agent == "opencode" {
            opencode(&directory.path().join("session.db"))
        } else {
            let mut records = records(agent);
            for (index, record) in records.iter_mut().enumerate() {
                record["timestamp"] = json!(format!("2026-01-01T00:00:{index:02}Z"));
            }
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
            session_id: "native-eval".to_owned(),
            source,
            source_format: format,
            fork_parent_session_id: None,
        };
        let reader = reader_for(agent);
        let store = MemoryTurnRowStore::new(agent, "native-eval");
        let mut sink = CompositeSink::with_turn_rows(
            SessionMetricsAccumulator::new(agent, "native-eval"),
            SessionEvidenceAccumulator::new(EvidenceSource {
                agent: agent.to_owned(),
                session_id: "native-eval".to_owned(),
                kind: SourceKind::from(&input.source),
                capabilities: reader.capabilities(&input),
            }),
            TurnRowSink::new(
                Arc::clone(&store) as Arc<dyn TurnRowStore>,
                "native-eval".to_owned(),
                None,
            ),
        );
        let outcome = reader.visit(&input, &mut sink).expect("native parse");
        sink.observe_source_outcome(outcome);
        assert!(!sink.turn_row_write_failed());
        let published = store.with_connection(|connection| {
            query_turn_content_offset_selected(
                connection,
                &TurnSessionKey {
                    environment_key: "native",
                    agent,
                    session_id: "native-eval",
                },
                &FenceScope::single(1),
                None,
                &Default::default(),
                0,
                IgnoredInstructionsCheck.input_selection(),
            )
            .expect("stored selection")
        });
        let selected = select_session_content(
            &prepare_session_content("native-eval", format, published, Vec::new()),
            IgnoredInstructionsCheck.input_selection(),
        );
        for field in [
            JevInputField::UserMessage,
            JevInputField::BashCommandInput,
            JevInputField::BashCommandOutput,
        ] {
            let available = selected
                .field_availability
                .iter()
                .find(|entry| entry.field == field)
                .expect("field availability");
            assert_eq!(available.observed_parts, 1, "{agent} {field:?}");
        }
        assert_eq!(
            selected
                .actions
                .iter()
                .map(|action| action.text.as_str())
                .collect::<Vec<_>>(),
            [
                "Inspect the queue.",
                "cargo check",
                "Finished checks successfully."
            ],
            "{agent}"
        );
        let request = selected
            .actions
            .iter()
            .find(|action| action.kind == "tool_input")
            .expect("request");
        let result = selected
            .actions
            .iter()
            .find(|action| action.kind == "tool_result")
            .expect("result");
        assert_eq!(request.tool_call_id, result.tool_call_id, "{agent}");
    }
}
