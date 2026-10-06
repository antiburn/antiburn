use super::{Case, IgnoredInstructionsCheck, JevCheck, source_format};
use antiburn_local::analysis::ignored_instructions::{
    prepare_session_content, select_session_content,
};
use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, FenceScope, MemoryTurnRowStore, RawSource,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SourceKind, TurnRowSink,
    TurnRowStore, TurnSessionKey, query_turn_content_offset_selected, reader_for,
};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) fn validate(case: &Case, expected: &[Value]) {
    let agent = match case.format.as_str() {
        "ClaudeJsonl" => "claude",
        "CodexRolloutJsonl" => "codex",
        "PiV3Jsonl" => "pi",
        "OpenCodeSqliteV2" => "opencode",
        "CursorCliAgentJsonl" => "cursor",
        "AntigravityBrainJsonl" => "antigravity",
        other => panic!("unknown native format {other}"),
    };
    let directory = tempfile::tempdir().expect("native confirmation has temporary directory");
    let path = directory.path().join("confirmation.db");
    let mut records = Vec::new();
    if agent == "codex" {
        records.push(
            json!({"type":"session_meta","payload":{"id":"confirmation","cwd":"/synthetic"}}),
        );
    } else if agent == "pi" {
        records.push(json!({"type":"session","version":3,"id":"confirmation","timestamp":"2026-01-01T00:00:00Z","cwd":"/synthetic"}));
    } else if agent == "cursor" {
        records.push(json!({"sessionId":"confirmation","cursor_source":"agent_transcript"}));
    }
    let database = if agent == "opencode" {
        let connection = rusqlite::Connection::open(&path)
            .expect("native confirmation opens temporary database");
        connection.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY); CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, data TEXT); CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, data TEXT); INSERT INTO session VALUES ('confirmation');").expect("native confirmation creates database schema");
        Some(connection)
    } else {
        None
    };
    for (index, event) in case.fixture["events"]
        .as_array()
        .expect("native confirmation events are an array")
        .iter()
        .enumerate()
    {
        let kind = event[0].as_str().expect("native event kind is text");
        let text = event[1].as_str().expect("native event content is text");
        if matches!(
            kind,
            "user" | "bash_output" | "read_output" | "search_output" | "other_output"
        ) {
            continue;
        }
        let id = format!("native-{index:02}");
        let timestamp = format!("2026-01-01T00:00:{:02}Z", index + 1);
        let request = match kind {
            "assistant" => None,
            "bash" => Some(("bash", json!({"command":text}))),
            "read" => Some(("read", json!({"path":text}))),
            "edit_path" => Some(("edit", json!({"path":text}))),
            "edit_content" => Some(("edit", json!({"path":"src/App.tsx","newText":text}))),
            "search" => Some((
                "grep",
                serde_json::from_str(text).expect("native search input is JSON"),
            )),
            "other" => Some((
                "issue_tool",
                serde_json::from_str(text).expect("native other-tool input is JSON"),
            )),
            // Native companions characterize selected fields. Excluded fields use the shared normalized harness.
            "user" | "bash_output" | "read_output" | "search_output" | "other_output" => continue,
            other => panic!("unsupported native fixture kind {other}"),
        };
        let block = match &request {
            Some((name, arguments)) => match agent {
                "pi" => json!({"type":"toolCall","id":id,"name":name,"arguments":arguments}),
                "cursor" => json!({"type":"tool-use","id":id,"name":name,"input":arguments}),
                _ => json!({"type":"tool_use","id":id,"name":name,"input":arguments}),
            },
            None => json!({"type":"text","text":text}),
        };
        let record = match agent {
            "claude" => {
                json!({"type":"assistant","uuid":id,"timestamp":timestamp,"message":{"role":"assistant","content":[block]}})
            }
            "pi" => {
                json!({"type":"message","id":id,"parentId":null,"timestamp":timestamp,"message":{"role":"assistant","content":[block]}})
            }
            "cursor" => {
                json!({"role":"assistant","timestamp":timestamp,"message":{"content":[block]}})
            }
            "codex" => match request {
                Some((name, arguments)) => {
                    json!({"type":"response_item","timestamp":timestamp,"payload":{"type":"function_call","call_id":id,"name":name,"arguments":arguments}})
                }
                None => {
                    json!({"type":"response_item","timestamp":timestamp,"payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}})
                }
            },
            "antigravity" => match request {
                Some((name, arguments)) => {
                    json!({"type":"PLANNER_RESPONSE","step_index":index+1,"timestamp":timestamp,"tool_calls":[{"name":name,"args":arguments}]})
                }
                None => {
                    json!({"type":"PLANNER_RESPONSE","step_index":index+1,"timestamp":timestamp,"content":text})
                }
            },
            "opencode" => {
                let connection = database
                    .as_ref()
                    .expect("OpenCode native source has a database");
                connection
                    .execute(
                        "INSERT INTO message VALUES (?1,'confirmation',?2)",
                        rusqlite::params![
                            id,
                            json!({"role":"assistant","time":{"created":1000+index}}).to_string()
                        ],
                    )
                    .expect("native confirmation inserts message");
                let part = match request {
                    Some((name, arguments)) => {
                        json!({"type":"tool","tool":name,"callID":id,"state":{"status":"running","input":arguments}})
                    }
                    None => json!({"type":"text","text":text}),
                };
                connection
                    .execute(
                        "INSERT INTO part VALUES (?1,?1,?2)",
                        rusqlite::params![id, part.to_string()],
                    )
                    .expect("native confirmation inserts part");
                continue;
            }
            _ => unreachable!(),
        };
        records.push(record);
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
        session_id: "confirmation".to_owned(),
        source,
        source_format: source_format(&case.format),
        fork_parent_session_id: None,
    };
    let reader = reader_for(agent);
    let store = MemoryTurnRowStore::new(agent, "confirmation");
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new(agent, "confirmation"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: agent.to_owned(),
            session_id: "confirmation".to_owned(),
            kind: SourceKind::from(&input.source),
            capabilities: reader.capabilities(&input),
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            "confirmation".to_owned(),
            None,
        ),
    );
    let outcome = reader
        .visit(&input, &mut sink)
        .expect("native confirmation parses source");
    sink.observe_source_outcome(outcome);
    assert!(!sink.turn_row_write_failed());
    let content = store.with_connection(|connection| {
        query_turn_content_offset_selected(
            connection,
            &TurnSessionKey {
                environment_key: "native",
                agent,
                session_id: "confirmation",
            },
            &FenceScope::single(1),
            None,
            &Default::default(),
            0,
            IgnoredInstructionsCheck.input_selection(),
        )
        .expect("native confirmation selects turn content")
    });
    let selected = select_session_content(
        &prepare_session_content("confirmation", input.source_format, content, Vec::new()),
        IgnoredInstructionsCheck.input_selection(),
    );
    for field in [
        "AssistantMessage",
        "BashCommandInput",
        "FileEditPath",
        "ReadFilePath",
        "SearchFilesQuery",
        "OtherToolInput",
    ] {
        let expected_count = expected.iter().filter(|entry| entry[1] == field).count();
        let observed_count = selected
            .field_availability
            .iter()
            .find(|availability| format!("{:?}", availability.field) == field)
            .map_or(0, |availability| availability.observed_parts as usize);
        assert_eq!(observed_count, expected_count, "{} {field}", case.id);
    }
    let texts = selected
        .actions
        .iter()
        .map(|action| action.text.as_str())
        .collect::<Vec<_>>();
    let expected_texts = expected
        .iter()
        .map(|field| field[2].as_str().expect("native selected content is text"))
        .collect::<Vec<_>>();
    assert_eq!(
        texts, expected_texts,
        "{} native selected projection",
        case.id
    );
}
