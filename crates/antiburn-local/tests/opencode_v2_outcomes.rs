use std::path::PathBuf;

use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
use antiburn_local::analysis::jev_evidence::{
    JevOperationState, JevReadResultKind, SessionContentEvidence, prepare_session_content,
};
use antiburn_local::analysis::{
    ContentKind, FenceScope, MemoryTurnRowStore, NativeToolOutcome, RawSource, SessionInput,
    ShellState, SourceFormat, TurnRowSink, TurnSessionKey, WorkState,
    query_turn_content_offset_selected, reader_for_input,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

struct Fixture {
    _directory: tempfile::TempDir,
    path: PathBuf,
    input: SessionInput,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic-opencode-v2.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE session_v2 (
                 id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL, title TEXT,
                 time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL
             );
             CREATE TABLE session_message (
                 id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
                 seq INTEGER NOT NULL, time_created INTEGER NOT NULL,
                 time_updated INTEGER NOT NULL, data TEXT NOT NULL
             );
             CREATE UNIQUE INDEX session_message_session_seq_idx ON session_message(session_id,seq);
             CREATE TABLE event_sequence (aggregate_id TEXT PRIMARY KEY NOT NULL, seq INTEGER NOT NULL);
             INSERT INTO session_v2 VALUES ('ses_outcomes',NULL,'/synthetic',NULL,100,100);
             INSERT INTO event_sequence VALUES ('ses_outcomes',100);",
            )
            .unwrap();
        let input = SessionInput {
            agent: "opencode".into(),
            session_id: "ses_outcomes".into(),
            source: RawSource::Sqlite(path.clone()),
            source_format: SourceFormat::OpenCodeSqliteV2,
            fork_parent_session_id: None,
        };
        Self {
            _directory: directory,
            path,
            input,
        }
    }

    fn insert(&self, id: &str, kind: &str, seq: i64, data: Value) {
        Connection::open(&self.path)
            .unwrap()
            .execute(
                "INSERT INTO session_message VALUES (?1,'ses_outcomes',?2,?3,100,100,?4)",
                params![id, kind, seq, data.to_string()],
            )
            .unwrap();
    }

    fn tools(&self, tools: Value) {
        self.insert(
            "msg_tools",
            "assistant",
            1,
            json!({
                "time":{"created":100,"completed":101},"agent":"build",
                "model":{"providerID":"synthetic-provider","id":"synthetic-model"},
                "content":tools,
            }),
        );
    }

    fn prepare(&self, fields: &[JevInputField]) -> SessionContentEvidence {
        let store = MemoryTurnRowStore::new("opencode", "ses_outcomes");
        let mut sink = TurnRowSink::new(store.clone(), "native", None);
        reader_for_input(&self.input)
            .visit(&self.input, &mut sink)
            .unwrap();
        sink.flush();
        assert!(!sink.has_error());
        store.with_connection(|connection| {
            let retained_facts: u64 = connection.query_row(
                "SELECT COUNT(*) FROM turn_content WHERE kind='tool_result' AND
                     (json_type(normalized_fields_json, '$.metadata.outcome') = 'object' OR
                      json_type(normalized_fields_json, '$.metadata.provider_executed') IN ('true','false'))",
                [], |row| row.get(0),
            ).unwrap();
            assert!(retained_facts > 0, "result facts must reach durable content JSON");
            let page = query_turn_content_offset_selected(
                connection,
                &TurnSessionKey {
                    environment_key: "native",
                    agent: "opencode",
                    session_id: "ses_outcomes",
                },
                &FenceScope::single(1),
                None,
                &Default::default(),
                0,
                JevInputSelection::from_fields(fields),
            )
            .unwrap();
            prepare_session_content("ses_outcomes", SourceFormat::OpenCodeSqliteV2, page, vec![])
        })
    }
}

fn tool(name: &str, id: &str, input: Value, metadata: Value, output: &str) -> Value {
    json!({
        "type":"tool","id":id,"name":name,"executed":false,
        "time":{"created":100,"ran":100,"completed":101},
        "state":{"status":"completed","input":input,"metadata":metadata,
            "content":[{"type":"text","text":output}]},
    })
}

fn shell(
    state: Option<ShellState>,
    exit: Option<i64>,
    signal: Option<&str>,
    timed_out: Option<bool>,
    id: Option<&str>,
) -> NativeToolOutcome {
    NativeToolOutcome::Shell {
        state,
        exit,
        signal: signal.map(str::to_owned),
        timed_out,
        shell_id: id.map(str::to_owned),
    }
}

#[test]
fn nonzero_shell_exit_survives_durable_rows_and_preparation_without_changing_tool_state() {
    let fixture = Fixture::new();
    fixture.tools(json!([tool(
        "shell",
        "call_exit",
        json!({"command":"exit 7"}),
        json!({"status":"completed","exit":7,"shellID":"sh_exit"}),
        "Exited with code 7"
    )]));
    let content = fixture.prepare(&[JevInputField::BashCommandOutput]);
    assert_eq!(content.actions.len(), 1);
    let action = &content.actions[0];
    assert_eq!(action.kind, ContentKind::ToolResult.as_str());
    assert_eq!(action.metadata.state, JevOperationState::Completed);
    assert_eq!(
        action.metadata.outcome,
        Some(shell(
            Some(ShellState::Completed),
            Some(7),
            None,
            None,
            Some("sh_exit")
        ))
    );
    assert_eq!(action.metadata.provider_executed, Some(false));
    assert_eq!(
        action.metadata.tool_output_field,
        Some(JevInputField::BashCommandOutput)
    );
    assert_eq!(action.text, "Exited with code 7");

    let inputs = fixture.prepare(&[JevInputField::BashCommandInput]);
    assert_eq!(inputs.actions.len(), 1);
    assert!(inputs.actions[0].metadata.outcome.is_none());
    assert!(inputs.actions[0].metadata.provider_executed.is_none());
    let excluded = action.metadata.selected(JevInputSelection::from_fields(&[
        JevInputField::OtherToolOutput,
    ]));
    assert!(excluded.outcome.is_none());
    assert!(excluded.provider_executed.is_none());
    assert!(excluded.tool_output_field.is_none());
}

#[test]
fn empty_shell_output_keeps_program_outcome_without_fabricating_text() {
    let fixture = Fixture::new();
    fixture.tools(json!([tool(
        "shell",
        "call_empty",
        json!({"command":"true"}),
        json!({"status":"completed","exit":0,"shellID":"sh_empty"}),
        ""
    )]));
    let content = fixture.prepare(&[JevInputField::BashCommandOutput]);
    let action = content
        .actions
        .iter()
        .find(|action| action.kind == ContentKind::ToolResult.as_str())
        .unwrap();
    assert!(action.text.is_empty());
    assert_eq!(action.metadata.state, JevOperationState::Completed);
    assert_eq!(
        action.metadata.outcome,
        Some(shell(
            Some(ShellState::Completed),
            Some(0),
            None,
            None,
            Some("sh_empty")
        ))
    );
}

#[test]
fn shell_signal_timeout_and_background_state_remain_separate_from_tool_completion() {
    let fixture = Fixture::new();
    fixture.tools(json!([
        tool(
            "shell",
            "signal",
            json!({"command":"synthetic"}),
            json!({"status":"completed","signal":"SIGTERM"}),
            "Killed by SIGTERM"
        ),
        tool(
            "shell",
            "timeout",
            json!({"command":"synthetic"}),
            json!({"status":"completed","timeout":true,"exit":143}),
            "Timed out before completion"
        ),
        tool(
            "shell",
            "background",
            json!({"command":"synthetic","background":true}),
            json!({"status":"running","shellID":"sh_background"}),
            "Command moved to the background"
        ),
    ]));
    let content = fixture.prepare(&[JevInputField::BashCommandOutput]);
    assert_eq!(content.actions.len(), 3);
    let expected = [
        shell(
            Some(ShellState::Completed),
            None,
            Some("SIGTERM"),
            None,
            None,
        ),
        shell(
            Some(ShellState::Completed),
            Some(143),
            None,
            Some(true),
            None,
        ),
        shell(
            Some(ShellState::Running),
            None,
            None,
            None,
            Some("sh_background"),
        ),
    ];
    for (action, expected) in content.actions.iter().zip(expected) {
        assert_eq!(action.metadata.state, JevOperationState::Completed);
        assert_eq!(action.metadata.outcome, Some(expected));
    }
}

#[test]
fn codemode_error_is_one_outer_output_and_nested_rows_remain_activity() {
    let fixture = Fixture::new();
    fixture.tools(json!([tool(
        "execute",
        "outer",
        json!({"code":"synthetic"}),
        json!({
            "error":true,"toolCalls":[
                {"tool":"shell","status":"completed","input":{"command":"exit 7"}},
                {"tool":"read","status":"error","input":{"path":"missing.txt"}},
            ]
        }),
        "synthetic program failed"
    )]));
    let content = fixture.prepare(&[
        JevInputField::OtherToolOutput,
        JevInputField::BashCommandOutput,
        JevInputField::ReadFileOutput,
    ]);
    assert_eq!(content.actions.len(), 1);
    assert_eq!(content.actions[0].tool_name.as_deref(), Some("execute"));
    assert_eq!(
        content.actions[0].metadata.state,
        JevOperationState::Completed
    );
    assert_eq!(
        content.actions[0].metadata.outcome,
        Some(NativeToolOutcome::CodeMode { error: Some(true) })
    );
    let only_shell = fixture.prepare(&[JevInputField::BashCommandOutput]);
    assert!(only_shell.actions.is_empty());
}

#[test]
fn subagent_launch_outcome_keeps_exact_session_and_work_state() {
    let fixture = Fixture::new();
    fixture.tools(json!([
        tool("subagent","running",json!({"agent":"review","description":"Review test","prompt":"Synthetic","background":true}),json!({"sessionID":"ses_child_running","status":"running"}),"The subagent is working in the background"),
        tool("subagent","completed",json!({"agent":"review","description":"Review test","prompt":"Synthetic"}),json!({"sessionID":"ses_child_completed","status":"completed"}),"synthetic child output"),
    ]));
    let content = fixture.prepare(&[JevInputField::OtherToolOutput]);
    assert_eq!(content.actions.len(), 2);
    for (action, id, state) in [
        (&content.actions[0], "ses_child_running", WorkState::Running),
        (
            &content.actions[1],
            "ses_child_completed",
            WorkState::Completed,
        ),
    ] {
        assert_eq!(action.metadata.state, JevOperationState::Completed);
        assert_eq!(
            action.metadata.outcome,
            Some(NativeToolOutcome::Subagent {
                session_id: id.into(),
                state
            })
        );
    }
}

#[test]
fn failed_subagent_projection_keeps_error_status_without_claiming_running_work() {
    let fixture = Fixture::new();
    let messages = [
        "Subagent failed (sessionID: ses_child): synthetic failure",
        "Subagent cancelled (sessionID: ses_child)",
    ];
    let tools: Vec<_> = messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let mut failed = tool(
                "subagent",
                &format!("failed-{index}"),
                json!({"agent":"review","description":"Review test","prompt":"Synthetic"}),
                json!({"sessionID":"ses_child","status":"running"}),
                "",
            );
            failed["state"]["status"] = json!("error");
            failed["state"]["error"] = json!({"type":"tool.execution","message":message});
            failed["state"].as_object_mut().unwrap().remove("content");
            failed
        })
        .collect();
    fixture.tools(json!(tools));
    let content = fixture.prepare(&[JevInputField::OtherToolOutput]);
    assert_eq!(content.actions.len(), 2);
    for (action, message) in content.actions.iter().zip(messages) {
        assert_eq!(action.text, message);
        assert_eq!(action.metadata.state, JevOperationState::Error);
        assert_eq!(action.metadata.provider_executed, Some(false));
        assert_eq!(action.metadata.outcome, None);
    }
}

#[test]
fn provider_execution_preserves_the_flag_without_using_local_outcome_contracts() {
    let fixture = Fixture::new();
    let mut provider = tool(
        "shell",
        "provider",
        json!({"providerSpecific":"synthetic"}),
        json!({"status":"completed","exit":7,"timeout":true,"shellID":"sh_untrusted"}),
        "provider output",
    );
    provider["executed"] = json!(true);
    fixture.tools(json!([provider]));
    let content = fixture.prepare(&[JevInputField::BashCommandOutput]);
    assert_eq!(content.actions.len(), 1);
    assert_eq!(content.actions[0].metadata.provider_executed, Some(true));
    assert_eq!(content.actions[0].metadata.outcome, None);
    let excluded = content.actions[0]
        .metadata
        .selected(JevInputSelection::from_fields(&[
            JevInputField::UserMessage,
        ]));
    assert!(excluded.provider_executed.is_none());
}

#[test]
fn truncation_and_missing_companions_do_not_remove_recorded_outcomes_or_read_files() {
    let fixture = Fixture::new();
    let companion = fixture._directory.path().join("private-companion.txt");
    std::fs::write(&companion, "COMPANION_BODY_MUST_NOT_BE_READ").unwrap();
    fixture.tools(json!([
        tool("shell","clipped",json!({"command":"synthetic"}),json!({"status":"completed","exit":7,"truncated":true,"file":companion}),"retained prefix"),
        tool("shell","missing",json!({"command":"synthetic"}),json!({"status":"completed","shellID":"sh_missing","file":fixture._directory.path().join("absent.out")}),"Shell command output is no longer available."),
    ]));
    let content = fixture.prepare(&[JevInputField::BashCommandOutput]);
    assert_eq!(content.actions.len(), 2);
    assert!(content.actions[0].truncated);
    assert!(
        content
            .actions
            .iter()
            .all(|action| action.metadata.outcome.is_some())
    );
    assert!(
        content
            .actions
            .iter()
            .all(|action| !action.text.contains("COMPANION_BODY"))
    );
    assert_eq!(
        content.actions[1].text,
        "Shell command output is no longer available."
    );
}

#[test]
fn direct_web_tools_and_pdf_projection_keep_only_recorded_text() {
    let fixture = Fixture::new();
    let mut pdf = tool(
        "read",
        "pdf",
        json!({"path":"synthetic.pdf"}),
        json!({"truncated":false}),
        "PDF read successfully",
    );
    pdf["state"]["content"].as_array_mut().unwrap().push(json!({"type":"file","mime":"application/pdf","uri":"data:application/pdf;base64,U1lOVEhFVElD","name":"synthetic.pdf"}));
    fixture.tools(json!([
        tool(
            "webfetch",
            "fetch",
            json!({"url":"https://example.invalid/synthetic","format":"markdown"}),
            json!({"contentType":"text/markdown"}),
            "# Synthetic fetched page"
        ),
        tool(
            "websearch",
            "search",
            json!({"query":"synthetic documentation"}),
            json!({"provider":"synthetic-search"}),
            "## [Synthetic result](https://example.invalid/result)\n\nSynthetic result content"
        ),
        pdf,
    ]));
    let content = fixture.prepare(&[
        JevInputField::OtherToolOutput,
        JevInputField::ReadFileOutput,
    ]);
    assert_eq!(content.actions.len(), 3, "{:#?}", content.actions);
    assert_eq!(content.actions[0].text, "# Synthetic fetched page");
    assert!(content.actions[1].text.starts_with("## [Synthetic result]"));
    assert_eq!(content.actions[2].text, "PDF read successfully");
    assert!(
        content.actions[2]
            .metadata
            .read_result
            .as_ref()
            .is_none_or(|result| result.kind == JevReadResultKind::Unknown)
    );
    let encoded = serde_json::to_string(&content).unwrap();
    assert!(!encoded.contains("U1lOVEhFVElD"));
    assert!(
        content
            .actions
            .iter()
            .all(|action| action.metadata.outcome.is_none())
    );
}

#[test]
fn shell_message_native_state_is_retained_and_opaque_checkpoints_add_no_result() {
    let fixture = Fixture::new();
    fixture.insert("msg_shell","shell",1,json!({"time":{"created":100,"completed":101},"shellID":"sh_native","command":"synthetic","status":"timeout","exit":143,"output":{"output":"native shell output","truncated":false}}));
    fixture.insert("msg_checkpoint","compaction",2,json!({"time":{"created":102},"status":"completed","reason":"manual","summary":"synthetic summary","recent":"synthetic recent","providerContext":{"messages":[{"role":"user","content":"OPAQUE_PROVIDER_BODY"}]}}));
    let content = fixture.prepare(&[
        JevInputField::BashCommandOutput,
        JevInputField::OtherToolOutput,
    ]);
    assert_eq!(content.actions.len(), 1);
    assert_eq!(
        content.actions[0].metadata.outcome,
        Some(shell(
            Some(ShellState::Timeout),
            Some(143),
            None,
            Some(true),
            Some("sh_native")
        ))
    );
    assert!(
        !serde_json::to_string(&content)
            .unwrap()
            .contains("OPAQUE_PROVIDER_BODY")
    );
}
