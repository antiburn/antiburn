use antiburn_local::analysis::ignored_instructions::{
    INPUT_SELECTION, SessionContentEvidence, prepare_session_content, select_session_content,
};
use antiburn_local::analysis::*;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn project(
    format: SourceFormat,
    role: &str,
    request: Option<(&str, Value)>,
    text: &str,
    output_tool: Option<&str>,
) -> SessionContentEvidence {
    let agent = match format {
        SourceFormat::ClaudeJsonl => "claude",
        SourceFormat::CodexRolloutJsonl => "codex",
        SourceFormat::OpenCodeSqliteV2 => "opencode",
        SourceFormat::PiV3Jsonl => "pi",
        SourceFormat::CursorCliAgentJsonl => "cursor",
        SourceFormat::AntigravityBrainJsonl => "antigravity",
        _ => panic!("unsupported source contract"),
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native.db");
    let block = if let Some(name) = output_tool {
        match agent {
            "pi" => json!({"type":"text","text":text}),
            "cursor" => {
                json!({"type":"tool-result","tool_use_id":"native-call","tool_name":name,"content":text})
            }
            _ => json!({"type":"tool_result","tool_use_id":"native-call","content":text}),
        }
    } else {
        match &request {
            Some((name, arguments)) => match agent {
                "pi" => {
                    json!({"type":"toolCall","id":"native-call","name":name,"arguments":arguments})
                }
                "cursor" => {
                    json!({"type":"tool-use","id":"native-call","name":name,"input":arguments})
                }
                _ => json!({"type":"tool_use","id":"native-call","name":name,"input":arguments}),
            },
            None => json!({"type":"text","text":text}),
        }
    };
    let mut records = match agent {
        "claude" => vec![
            json!({"type":if output_tool.is_some() {"user"} else {role},"uuid":"native-record","message":{"role":if output_tool.is_some() {"user"} else {role},"content":[block]}}),
        ],
        "pi" => vec![
            json!({"type":"session","version":3,"id":"inventory","timestamp":"2026-01-01T00:00:00Z","cwd":"/synthetic"}),
            json!({"type":"message","id":"native-record","parentId":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":if output_tool.is_some() {"toolResult"} else {role},"toolCallId":if output_tool.is_some() {Some("native-call")} else {None},"toolName":output_tool,"content":[block]}}),
        ],
        "cursor" => vec![
            json!({"sessionId":"inventory","cursor_source":"agent_transcript"}),
            json!({"role":role,"message":{"content":[block]}}),
        ],
        "codex" => vec![
            json!({"type":"session_meta","payload":{"id":"inventory","cwd":"/synthetic"}}),
            if output_tool.is_some() {
                json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"native-call","output":text}})
            } else {
                match &request {
                    Some((name, arguments)) => {
                        json!({"type":"response_item","payload":{"type":"function_call","call_id":"native-call","name":name,"arguments":arguments}})
                    }
                    None => {
                        json!({"type":"response_item","payload":{"type":"message","role":role,"content":[{"type":"output_text","text":text}]}})
                    }
                }
            },
        ],
        "antigravity" => vec![if output_tool.is_some() {
            json!({"type":"RUN_COMMAND","step_index":1,"content":text})
        } else {
            match &request {
                Some((name, arguments)) => {
                    json!({"type":"PLANNER_RESPONSE","step_index":1,"tool_calls":[{"name":name,"args":arguments}]})
                }
                None if role == "assistant" => {
                    json!({"type":"PLANNER_RESPONSE","step_index":1,"content":text})
                }
                None => {
                    json!({"type":"USER_INPUT","step_index":1,"userInput":{"userResponse":text}})
                }
            }
        }],
        "opencode" => {
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY); CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, data TEXT); CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, data TEXT); INSERT INTO session VALUES ('inventory');").unwrap();
            connection
                .execute(
                    "INSERT INTO message VALUES ('native-record','inventory',?1)",
                    [json!({"role":role,"time":{"created":1000}}).to_string()],
                )
                .unwrap();
            let part = if let Some(name) = output_tool {
                json!({"type":"tool","tool":name,"callID":"native-call","state":{"status":"completed","output":text}})
            } else {
                match &request {
                    Some((name, arguments)) => {
                        json!({"type":"tool","tool":name,"callID":"native-call","state":{"status":"running","input":arguments}})
                    }
                    None => json!({"type":"text","text":text}),
                }
            };
            connection
                .execute(
                    "INSERT INTO part VALUES ('native-part','native-record',?1)",
                    [part.to_string()],
                )
                .unwrap();
            Vec::new()
        }
        _ => unreachable!(),
    };
    for (index, record) in records.iter_mut().enumerate() {
        record["timestamp"] = json!(format!("2026-01-01T00:00:{index:02}Z"));
    }
    assert!(
        records
            .iter()
            .map(|record| record.to_string().len())
            .sum::<usize>()
            <= 65536
    );
    let input = SessionInput {
        agent: agent.into(),
        session_id: "inventory".into(),
        source_format: format,
        fork_parent_session_id: None,
        source: if agent == "opencode" {
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
        },
    };
    let reader = reader_for(agent);
    let store = MemoryTurnRowStore::new(agent, "inventory");
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new(agent, "inventory"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: agent.into(),
            session_id: "inventory".into(),
            kind: SourceKind::from(&input.source),
            capabilities: reader.capabilities(&input),
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            "inventory",
            None,
        ),
    );
    let outcome = reader.visit(&input, &mut sink).unwrap();
    sink.observe_source_outcome(outcome);
    assert!(!sink.turn_row_write_failed());
    let published = store.with_connection(|connection| {
        query_turn_content_offset_selected(
            connection,
            &TurnSessionKey {
                environment_key: "native",
                agent,
                session_id: "inventory",
            },
            &FenceScope::single(1),
            None,
            &Default::default(),
            0,
            INPUT_SELECTION,
        )
        .unwrap()
    });
    select_session_content(
        &prepare_session_content("inventory", format, published, Vec::new()),
        INPUT_SELECTION,
    )
}
