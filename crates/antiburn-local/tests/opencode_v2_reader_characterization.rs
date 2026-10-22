use antiburn_local::analysis as opencode_v2_snapshot;
use std::path::PathBuf;

use antiburn_local::analysis::{
    ContentKind, NormalizedEvent, NormalizedRecord, RawSource, RecordSink, SessionInput,
    SessionReader, SessionSummary, SourceFormat, ToolResultObservation, TurnContent, VisitOutcome,
    reader_for_input,
};
use rusqlite::{Connection, params};
use tempfile::tempdir;

static V2_READER: std::sync::LazyLock<&'static dyn SessionReader> =
    std::sync::LazyLock::new(|| {
        reader_for_input(&SessionInput {
            agent: "opencode".to_owned(),
            session_id: "ses_fixture".to_owned(),
            source: RawSource::Sqlite(PathBuf::new()),
            source_format: SourceFormat::OpenCodeSqliteV2,
            fork_parent_session_id: None,
        })
    });
use V2_READER as OpenCodeV2SessionReader;

#[derive(Default)]
struct CaptureSink {
    events: Vec<NormalizedEvent>,
    contents: Vec<TurnContent>,
    tool_results: Vec<ToolResultObservation>,
    summary: Option<SessionSummary>,
    user_messages: usize,
    tool_result_gaps: usize,
    unusable: Vec<antiburn_local::analysis::PartialReason>,
    observations: Vec<antiburn_local::analysis::EvidenceObservation>,
    source_summaries: Vec<(String, SessionSummary)>,
}

impl RecordSink for CaptureSink {
    fn source_summary(&mut self, session_id: &str, summary: SessionSummary) {
        self.source_summaries.push((session_id.to_owned(), summary));
    }

    fn record(&mut self, record: NormalizedRecord) {
        match record {
            NormalizedRecord::MetricsEvent(event) => self.events.push(*event),
            NormalizedRecord::TurnContent(content) => self.contents.push(*content),
            NormalizedRecord::ToolResult(result) => self.tool_results.push(*result),
            NormalizedRecord::Unusable(reason) => self.unusable.push(reason),
            NormalizedRecord::Observation(observation) => self.observations.push(*observation),
        }
    }

    fn user_message(&mut self, _: antiburn_local::analysis::UserMessage) {
        self.user_messages += 1;
    }

    fn tool_result_gap(&mut self) {
        self.tool_result_gaps += 1;
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.summary = Some(summary);
    }
}

fn fixture() -> (tempfile::TempDir, PathBuf, SessionInput) {
    let directory = tempdir().unwrap();
    let path = directory.path().join("opencode-v2.db");
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE session_v2 (
                  id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL DEFAULT '/repo',
                  title TEXT, time_created INTEGER NOT NULL DEFAULT 0,
                  time_updated INTEGER NOT NULL DEFAULT 0
              );
              CREATE TABLE session_message (
                  id TEXT PRIMARY KEY,
                  session_id TEXT NOT NULL,
                  type TEXT NOT NULL,
                  seq INTEGER NOT NULL,
                  time_created INTEGER NOT NULL DEFAULT 0,
                  time_updated INTEGER NOT NULL DEFAULT 0,
                  data TEXT NOT NULL
              );
              CREATE UNIQUE INDEX session_message_session_seq_idx
                  ON session_message(session_id, seq);
              CREATE TABLE event_sequence (
                  aggregate_id TEXT PRIMARY KEY NOT NULL, seq INTEGER NOT NULL
              );
              INSERT INTO session_v2 (id) VALUES ('ses_v2');",
        )
        .unwrap();
    let input = SessionInput {
        agent: "opencode".to_owned(),
        session_id: "ses_v2".to_owned(),
        source: RawSource::Sqlite(path.clone()),
        source_format: SourceFormat::OpenCodeSqliteV2,
        fork_parent_session_id: None,
    };
    (directory, path, input)
}

fn insert(path: &std::path::Path, id: &str, message_type: &str, seq: i64, data: &str) {
    Connection::open(path)
        .unwrap()
        .execute(
            "INSERT INTO session_message (id, session_id, type, seq, data)
             VALUES (?1, 'ses_v2', ?2, ?3, ?4)",
            params![id, message_type, seq, data],
        )
        .unwrap();
}

fn assistant_message() -> &'static str {
    r#"{
        "time":{"created":1735689600123},
        "agent":"build",
        "model":{"id":"model-v2","providerID":"provider-v2"},
        "tokens":{"input":101,"output":17,"reasoning":5,"cache":{"read":31,"write":13}},
        "content":[
            {"type":"text","text":"public answer"},
            {"type":"reasoning","text":"private chain of thought"},
            {"type":"tool","id":"call-1","name":"read","time":{"created":1735689600123},"state":{
                "status":"completed",
                "input":{"path":"src/main.rs"},
                "content":[{"type":"text","text":"public tool output"}]
            }}
        ]
    }"#
}

#[test]
fn maps_proven_assistant_usage_identity_timestamps_and_public_content() {
    let (_directory, path, input) = fixture();
    insert(&path, "msg_assistant", "assistant", 0, assistant_message());
    let mut sink = CaptureSink::default();

    let outcome = OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();

    assert_eq!(outcome, VisitOutcome::Unvalidated);
    assert_eq!(sink.events.len(), 1);
    let event = &sink.events[0];
    assert_eq!(event.ts_ms, Some(1_735_689_600_123));
    assert_eq!(event.uuid.as_deref(), Some("msg_assistant"));
    assert_eq!(event.thread_id.as_deref(), Some("ses_v2"));
    assert_eq!(event.model.as_deref(), Some("model-v2"));
    assert_eq!(event.provider.as_deref(), Some("provider-v2"));
    assert_eq!(event.usage.input_tokens, 101);
    assert_eq!(event.usage.output_tokens, 22);
    assert_eq!(event.usage.cache_read_tokens, 31);
    assert_eq!(event.usage.cache_creation_tokens, 13);
    assert_eq!(event.tools.len(), 1);
    assert_eq!(event.tools[0].name, "read");

    let captured = &sink.contents[0].parts;
    assert_eq!(captured.len(), 3);
    assert_eq!(captured[0].kind, ContentKind::AssistantText);
    assert_eq!(captured[0].text, "public answer");
    assert_eq!(captured[1].kind, ContentKind::ToolInput);
    assert_eq!(captured[1].tool_call_id.as_deref(), Some("call-1"));
    assert_eq!(captured[2].kind, ContentKind::ToolResult);
    assert_eq!(captured[2].text, "public tool output");
    assert!(
        captured
            .iter()
            .all(|part| !part.text.contains("private chain of thought"))
    );
    assert!(sink.tool_results.is_empty());
    assert_eq!(sink.user_messages, 0);
    assert_eq!(sink.tool_result_gaps, 1);
    assert_eq!(
        sink.summary.as_ref().unwrap().model.as_deref(),
        Some("model-v2")
    );
    assert!(sink.summary.as_ref().unwrap().cache_write_tokens_available);
}

#[test]
fn normalize_matches_metrics_emitted_by_visit() {
    let (_directory, path, input) = fixture();
    insert(&path, "msg_assistant", "assistant", 0, assistant_message());
    let mut sink = CaptureSink::default();

    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    let normalized = OpenCodeV2SessionReader.normalize(&input).unwrap();

    assert_eq!(normalized.events, sink.events);
    assert_eq!(normalized.session_id, "ses_v2");
    assert_eq!(normalized.agent, "opencode");
}

#[test]
fn unknown_rows_report_partial_evidence() {
    let (_directory, path, input) = fixture();
    insert(&path, "msg_before", "user", 0, r#"{"time":{"created":1}}"#);
    insert(&path, "msg_unknown", "future-type", 1, "{}");
    let mut sink = CaptureSink::default();

    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();

    assert_eq!(sink.events.len(), 1);
    assert!(!sink.unusable.is_empty());
    assert!(
        sink.summary
            .as_ref()
            .unwrap()
            .coverage_gaps
            .contains(&antiburn_local::analysis::PartialReason::MalformedRecord)
    );
    assert_eq!(sink.tool_result_gaps, 1);
}

#[test]
fn malformed_json_reports_partial_evidence() {
    let (_directory, path, input) = fixture();
    insert(&path, "msg_bad", "assistant", 0, "{");
    let mut sink = CaptureSink::default();

    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();

    assert!(sink.events.is_empty());
    assert!(
        sink.summary
            .as_ref()
            .unwrap()
            .coverage_gaps
            .contains(&antiburn_local::analysis::PartialReason::MalformedRecord)
    );
}

#[test]
fn user_text_is_saved_as_content_without_becoming_a_request_fact() {
    let (_directory, path, input) = fixture();
    insert(
        &path,
        "msg_user",
        "user",
        0,
        r#"{"time":{"created":1},"text":"private user text"}"#,
    );
    let mut sink = CaptureSink::default();

    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();

    assert_eq!(sink.events.len(), 1);
    assert_eq!(sink.events[0].role, antiburn_local::analysis::Role::User);
    assert_eq!(sink.contents.len(), 1);
    assert_eq!(sink.contents[0].parts[0].kind, ContentKind::UserText);
    assert_eq!(sink.contents[0].parts[0].text, "private user text");
    assert_eq!(sink.user_messages, 0);
}

#[test]
fn v2_reader_source_exposes_the_live_sequence_baseline() {
    let (_directory, path, _input) = fixture();
    Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO event_sequence (aggregate_id, seq) VALUES ('ses_v2', 17)",
            [],
        )
        .unwrap();

    let baseline = opencode_v2_snapshot::read_capture_baseline(
        &path,
        SourceFormat::OpenCodeSqliteV2,
        &["ses_v2".to_owned()],
        &|| false,
    )
    .unwrap();

    assert_eq!(baseline.get("ses_v2"), Some(&Some(17)));
}

#[test]
fn user_attachments_mark_saved_text_partial_without_reading_attachment_paths() {
    let (_directory, path, input) = fixture();
    insert(
        &path,
        "msg_user",
        "user",
        0,
        r#"{"time":{"created":1},"text":"review this image","files":[{"path":"/private/image.png"}]}"#,
    );
    let mut sink = CaptureSink::default();

    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();

    assert_eq!(sink.contents[0].parts[0].text, "review this image");
    assert!(
        sink.unusable
            .contains(&antiburn_local::analysis::PartialReason::UnrecognizedRecordType)
    );
    assert_eq!(sink.user_messages, 0);
}

#[test]
fn oversized_rows_mark_the_session_partial_and_do_not_emit_facts() {
    let (_directory, path, input) = fixture();
    let oversized = format!(
        "{{\"time\":{{\"created\":1}},\"text\":\"{}\"}}",
        "x".repeat(8 * 1024 * 1024)
    );
    insert(&path, "msg_large", "assistant", 0, &oversized);
    insert(&path, "msg_after", "user", 1, r#"{"time":{"created":2}}"#);
    let mut sink = CaptureSink::default();

    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();

    assert_eq!(sink.events.len(), 1);
    assert_eq!(sink.events[0].role, antiburn_local::analysis::Role::User);
    assert!(
        sink.unusable
            .contains(&antiburn_local::analysis::PartialReason::Oversized)
    );
    assert!(
        sink.summary
            .as_ref()
            .unwrap()
            .coverage_gaps
            .contains(&antiburn_local::analysis::PartialReason::Oversized)
    );
}

#[test]
fn transfer_format_uses_the_v2_reader_and_checks_session_identity() {
    let (_directory, _path, mut input) = fixture();
    input.source_format = SourceFormat::OpenCodeV2SessionExport;
    input.source = RawSource::Jsonl(
        serde_json::json!({
            "info": {"id":"ses_v2","location":{"directory":"/repo"}},
            "messages": [{
                "id":"msg_export","type":"assistant",
                    "time":{"created":1735689600123_i64},"agent":"build",
                "model":{"id":"model-v2","providerID":"provider-v2"},
                "tokens":{"input":9,"output":3,"reasoning":2,"cache":{"read":0,"write":0}},
                "content":[]
            }]
        })
        .to_string(),
    );

    let reader = reader_for_input(&input);
    assert_eq!(
        reader.capabilities(&input).source_format,
        SourceFormat::OpenCodeV2SessionExport
    );
    let normalized = reader.normalize(&input).unwrap();
    assert_eq!(normalized.events.len(), 1);
    assert_eq!(normalized.events[0].usage.output_tokens, 5);

    input.session_id = "ses_other".to_owned();
    assert!(reader.normalize(&input).unwrap().events.is_empty());
}

#[test]
fn text_tool_content_is_kept_and_non_text_content_marks_partial() {
    let (_directory, path, input) = fixture();
    let mut value: serde_json::Value = serde_json::from_str(assistant_message()).unwrap();
    value["content"][2]["state"]["content"] = serde_json::json!([
        {"type":"text", "text":"safe output"},
        {"type":"file", "uri":"file:///private/output.png", "mime":"image/png"},
        {"type":"image", "data":"private-image-data"}
    ]);
    insert(
        &path,
        "msg_assistant",
        "assistant",
        0,
        &serde_json::to_string(&value).unwrap(),
    );
    let mut sink = CaptureSink::default();

    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();

    let parts = &sink.contents[0].parts;
    assert_eq!(parts.last().unwrap().text, "safe output");
    assert!(
        !parts
            .iter()
            .any(|part| part.text.contains("private-image-data"))
    );
    assert!(
        sink.unusable
            .contains(&antiburn_local::analysis::PartialReason::UnrecognizedRecordType)
    );
    assert!(
        sink.summary
            .as_ref()
            .unwrap()
            .coverage_gaps
            .contains(&antiburn_local::analysis::PartialReason::UnrecognizedRecordType)
    );
}

#[test]
fn content_capture_caps_part_count_and_aggregate_bytes() {
    let (_directory, path, input) = fixture();
    let mut too_many: serde_json::Value = serde_json::from_str(assistant_message()).unwrap();
    too_many["content"] = serde_json::Value::Array(
        (0..1025)
            .map(|_| serde_json::json!({"type":"text", "text":"x"}))
            .collect(),
    );
    insert(
        &path,
        "msg_many",
        "assistant",
        0,
        &serde_json::to_string(&too_many).unwrap(),
    );
    let mut sink = CaptureSink::default();

    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();

    assert_eq!(sink.contents[0].parts.len(), 1024);
    assert!(
        sink.unusable
            .contains(&antiburn_local::analysis::PartialReason::Oversized)
    );

    let (_directory, path, input) = fixture();
    let mut too_large: serde_json::Value = serde_json::from_str(assistant_message()).unwrap();
    too_large["content"] = serde_json::json!([
        {"type":"text", "text":"a".repeat(200_000)},
        {"type":"text", "text":"b".repeat(200_000)}
    ]);
    insert(
        &path,
        "msg_large_content",
        "assistant",
        0,
        &serde_json::to_string(&too_large).unwrap(),
    );
    let mut sink = CaptureSink::default();

    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();

    let parts = &sink.contents[0].parts;
    assert_eq!(
        parts.iter().map(|part| part.text.len()).sum::<usize>(),
        256 * 1024
    );
    assert!(parts[1].truncated);
    assert!(
        sink.unusable
            .contains(&antiburn_local::analysis::PartialReason::Oversized)
    );
}

#[test]
fn rejects_a_source_format_or_container_mismatch() {
    let (_directory, _path, mut input) = fixture();
    input.source_format = SourceFormat::OpenCodeSqliteV1;
    assert!(OpenCodeV2SessionReader.normalize(&input).is_err());

    input.source_format = SourceFormat::OpenCodeSqliteV2;
    input.source = RawSource::Jsonl(String::new());
    assert!(OpenCodeV2SessionReader.normalize(&input).is_err());
}

fn native_assistant(content: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "time":{"created":100,"completed":200}, "agent":"build",
        "model":{"id":"main-model","providerID":"test-provider","variant":"high"},
        "tokens":{"input":10,"output":3,"reasoning":2,"cache":{"read":7,"write":5}},
        "cost":0.01, "content":content
    })
}

fn native_tool(
    name: &str,
    id: &str,
    input: serde_json::Value,
    metadata: serde_json::Value,
    body: &str,
) -> serde_json::Value {
    serde_json::json!({"type":"tool","id":id,"name":name,
        "time":{"created":100,"ran":101,"completed":150},
        "state":{"status":"completed","input":input,"metadata":metadata,
            "content":[{"type":"text","text":body}]}})
}

fn insert_value(path: &std::path::Path, id: &str, kind: &str, seq: i64, value: serde_json::Value) {
    insert(path, id, kind, seq, &value.to_string());
}

fn add_accounting_columns(path: &std::path::Path) {
    Connection::open(path)
        .unwrap()
        .execute_batch(
            "ALTER TABLE session_v2 ADD COLUMN fork_session_id TEXT;
         ALTER TABLE session_v2 ADD COLUMN fork_boundary TEXT;
         ALTER TABLE session_v2 ADD COLUMN cost REAL NOT NULL DEFAULT 0;
         ALTER TABLE session_v2 ADD COLUMN tokens_input INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE session_v2 ADD COLUMN tokens_output INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE session_v2 ADD COLUMN tokens_reasoning INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE session_v2 ADD COLUMN tokens_cache_read INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE session_v2 ADD COLUMN tokens_cache_write INTEGER NOT NULL DEFAULT 0;",
        )
        .unwrap();
}

#[test]
fn direct_v2_tools_use_native_arguments_categories_and_lifecycle() {
    use antiburn_local::analysis::{
        ToolCategory, jev::JevInputField, jev_evidence::JevOperationState,
    };
    let (_directory, path, input) = fixture();
    let tools = serde_json::json!([
        native_tool(
            "read",
            "read-1",
            serde_json::json!({"path":"src/main.rs","offset":2,"limit":2}),
            serde_json::json!({"truncated":false}),
            "Read file src/main.rs, lines 2-3\n2: first\n3: second"
        ),
        native_tool(
            "patch",
            "patch-1",
            serde_json::json!({"patchText":"*** Begin Patch\n*** Update File: src/main.rs\n@@\n-a\n+b\n*** End Patch"}),
            serde_json::json!({}),
            "Success"
        ),
        native_tool(
            "shell",
            "shell-1",
            serde_json::json!({"command":"cargo nextest run","workdir":"/repo","timeout":2000}),
            serde_json::json!({"exit":7,"status":"completed","truncated":false}),
            "failed test\n\nExited with code 7"
        ),
        native_tool(
            "skill",
            "skill-1",
            serde_json::json!({"id":"review"}),
            serde_json::json!({"name":"review","directory":"/skills/review"}),
            "Skill: review\nUse the recorded guide."
        )
    ]);
    insert_value(&path, "msg_tools", "assistant", 0, native_assistant(tools));
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert_eq!(
        sink.events[0]
            .tools
            .iter()
            .map(|tool| tool.category)
            .collect::<Vec<_>>(),
        vec![
            ToolCategory::Read,
            ToolCategory::Edit,
            ToolCategory::Test,
            ToolCategory::Other
        ]
    );
    assert_eq!(sink.events[0].tools[3].detail.as_deref(), Some("review"));
    assert!(sink.events[0].thinking_mode.is_none());
    assert_eq!(
        sink.summary.as_ref().unwrap().provider_hints[0].provider,
        "test-provider"
    );
    let parts = &sink.contents[0].parts;
    let read = parts
        .iter()
        .find(|part| {
            part.kind == ContentKind::ToolInput && part.tool_name.as_deref() == Some("read")
        })
        .unwrap();
    assert!(
        read.normalized_fields.as_ref().unwrap().values[&JevInputField::ReadFilePath]
            .contains("src/main.rs")
    );
    assert_eq!(
        read.metadata.read_request.as_ref().unwrap().extent.offset,
        Some(2)
    );
    assert!(
        read.metadata
            .read_request
            .as_ref()
            .unwrap()
            .extent_contract
            .as_ref()
            .unwrap()
            .contains("843a6c514e606b436a7fe713422ace0a93ad3d61")
    );
    let read_result = parts
        .iter()
        .find_map(|part| part.metadata.read_result.as_ref())
        .unwrap();
    assert_eq!(
        read_result.returned_extent.as_ref().unwrap().end_inclusive,
        Some(3)
    );
    let shell_result = parts
        .iter()
        .find(|part| {
            part.kind == ContentKind::ToolResult && part.tool_name.as_deref() == Some("shell")
        })
        .unwrap();
    assert_eq!(shell_result.metadata.state, JevOperationState::Completed);
    assert!(shell_result.text.contains("Exited with code 7"));
    assert!(parts.iter().any(|part| {
        part.metadata
            .recorded_skill_result
            .as_ref()
            .and_then(|skill| skill.native_skill.as_ref())
            .is_some_and(|skill| skill.display_name.as_deref() == Some("review"))
    }));
    assert!(sink.tool_results.is_empty());
}

#[test]
fn code_mode_keeps_outer_output_and_only_observed_nested_activity() {
    use antiburn_local::analysis::ToolCategory;
    let (_directory, path, input) = fixture();
    let execute = native_tool(
        "execute",
        "outer",
        serde_json::json!({"code":"return tools.unobserved();"}),
        serde_json::json!({"error":true,"toolCalls":[
            {"tool":"mcp.demo.query","status":"completed","input":{"query":"known activity"}},
            {"tool":"fetch","status":"error","input":{"url":"https://example.invalid"}}
        ]}),
        "Outer script error",
    );
    insert_value(
        &path,
        "msg_code",
        "assistant",
        0,
        native_assistant(serde_json::json!([execute])),
    );
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert_eq!(
        sink.events[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["mcp.demo.query", "fetch"]
    );
    assert_eq!(sink.events[0].wrapper_tool.as_deref(), Some("execute"));
    let results = sink.contents[0]
        .parts
        .iter()
        .filter(|part| part.kind == ContentKind::ToolResult)
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_call_id.as_deref(), Some("outer"));
    assert_eq!(results[0].text, "Outer script error");
    assert!(sink.tool_results.is_empty());
    assert!(
        !sink.events[0]
            .tools
            .iter()
            .any(|tool| tool.name == "unobserved")
    );

    let (_directory, path, input) = fixture();
    insert_value(
        &path,
        "msg_pure",
        "assistant",
        0,
        native_assistant(serde_json::json!([native_tool(
            "execute",
            "pure",
            serde_json::json!({"code":"return 42"}),
            serde_json::json!({"toolCalls":[]}),
            "42"
        )])),
    );
    let normalized = OpenCodeV2SessionReader.normalize(&input).unwrap();
    assert_eq!(normalized.events[0].tools[0].category, ToolCategory::Other);
}

#[test]
fn truncated_and_background_results_do_not_prove_complete_program_output() {
    use antiburn_local::analysis::PartialReason;
    let (_directory, path, input) = fixture();
    insert_value(
        &path,
        "msg_background",
        "assistant",
        0,
        native_assistant(serde_json::json!([
            native_tool(
                "shell",
                "shell-background",
                serde_json::json!({"command":"long-job","background":true}),
                serde_json::json!({"shellID":"sh_job","status":"running","truncated":false}),
                "Command moved to the background (shell ID: sh_job)."
            ),
            native_tool(
                "read",
                "read-truncated",
                serde_json::json!({"path":"src/main.rs"}),
                serde_json::json!({"truncated":true}),
                "Read file src/main.rs, lines 1-1\n1: preview"
            )
        ])),
    );
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert!(sink.unusable.contains(&PartialReason::IncompleteTail));
    let result = sink.contents[0]
        .parts
        .iter()
        .find(|part| {
            part.kind == ContentKind::ToolResult && part.tool_name.as_deref() == Some("read")
        })
        .unwrap();
    assert!(result.truncated);
    assert!(
        result
            .metadata
            .read_result
            .as_ref()
            .unwrap()
            .returned_extent
            .is_none()
    );
    assert!(sink.tool_results.is_empty());
}

#[test]
fn question_form_answers_keep_unknown_origin_and_the_v2_pin() {
    let (_directory, path, input) = fixture();
    insert_value(
        &path,
        "msg_question",
        "assistant",
        0,
        native_assistant(serde_json::json!([native_tool(
            "question",
            "q1",
            serde_json::json!({"questions":[{"question":"Which scope?","header":"Scope","options":[{"label":"Small","description":"Only the parser"}],"multiple":false}]}),
            serde_json::json!({"answers":[["Small"]]}),
            "User has answered your questions: \"Which scope?\"=\"Small\". You can now continue with the user's answers in mind."
        )])),
    );
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    let answer = sink.contents[0]
        .parts
        .iter()
        .flat_map(|part| &part.metadata.user_answers)
        .next()
        .unwrap();
    assert_eq!(answer.selections[0].value.as_deref(), Some("Small"));
    assert_eq!(answer.source.source_format, SourceFormat::OpenCodeSqliteV2);
    assert!(
        answer
            .source
            .producer_revision
            .contains("843a6c514e606b436a7fe713422ace0a93ad3d61")
    );
    assert!(!answer.is_authoritative_user_response());
    assert!(answer.source.acceptance_order.is_none());
}

#[test]
fn native_read_returned_ranges_require_the_requested_filename() {
    for (requested, returned, matches) in [
        ("src/lib.rs", "src/lib.rs", true),
        ("src/lib.rs", "src/other.rs", false),
        (
            "docs/reader's guide.md",
            "docs/reader\u{2019}s guide.md",
            false,
        ),
    ] {
        let (_directory, path, input) = fixture();
        insert_value(
            &path,
            "msg_read",
            "assistant",
            0,
            native_assistant(serde_json::json!([native_tool(
                "read",
                "read",
                serde_json::json!({"path":requested,"offset":2,"limit":2}),
                serde_json::json!({"truncated":false}),
                &format!("Read file {returned}, lines 2-3\n2: first\n3: second")
            )])),
        );
        let mut sink = CaptureSink::default();
        OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
        let result = sink.contents[0]
            .parts
            .iter()
            .find_map(|part| part.metadata.read_result.as_ref())
            .unwrap();
        assert_eq!(result.returned_extent.is_some(), matches, "{requested:?}");
        assert_eq!(
            result.kind,
            antiburn_local::analysis::jev_evidence::JevReadResultKind::File
        );
        assert!(!result.truncated);
        assert!(sink.unusable.is_empty());
    }
}

#[test]
fn compaction_usage_is_counted_once_and_aggregate_remainder_is_not_a_fake_turn() {
    use antiburn_local::analysis::{PartialReason, Role};
    let (_directory, path, input) = fixture();
    add_accounting_columns(&path);
    insert_value(
        &path,
        "msg_assistant",
        "assistant",
        0,
        native_assistant(serde_json::json!([])),
    );
    insert_value(
        &path,
        "msg_compaction",
        "compaction",
        1,
        serde_json::json!({
            "time":{"created":300},"status":"completed","reason":"auto","model":{"id":"compact-model","providerID":"compact-provider"},
            "summary":"private compacted context","recent":"opaque retained checkpoint","providerContext":{"opaque":true},
            "tokens":{"input":20,"output":4,"reasoning":1,"cache":{"read":3,"write":2}},"cost":0.02
        }),
    );
    Connection::open(&path).unwrap().execute_batch(
        "UPDATE session_v2 SET cost=.03,tokens_input=30,tokens_output=7,tokens_reasoning=3,tokens_cache_read=10,tokens_cache_write=7;",
    ).unwrap();
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert_eq!(sink.events.len(), 2);
    assert_eq!(
        sink.events
            .iter()
            .filter(|event| event.role == Role::Assistant)
            .count(),
        1
    );
    assert!(sink.events[1].is_compaction_boundary);
    assert_eq!(sink.events[1].usage.input_tokens, 20);
    assert_eq!(sink.events[1].usage.output_tokens, 5);
    assert_eq!(sink.events[1].model.as_deref(), Some("compact-model"));
    assert!(
        !sink
            .unusable
            .contains(&PartialReason::AttributionIncomplete)
    );
    assert!(sink.contents.is_empty());

    Connection::open(&path)
        .unwrap()
        .execute_batch("UPDATE session_v2 SET cost=.04,tokens_input=40;")
        .unwrap();
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert_eq!(sink.events.len(), 2);
    assert!(
        sink.unusable
            .contains(&PartialReason::AttributionIncomplete)
    );
}

#[test]
fn fork_copied_history_is_not_new_usage_and_missing_boundaries_fail_closed() {
    use antiburn_local::analysis::EvidenceObservation;
    let (_directory, path, input) = fixture();
    add_accounting_columns(&path);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute("INSERT INTO session_v2 (id) VALUES ('ses_origin')", [])
        .unwrap();
    connection.execute("INSERT INTO session_message (id,session_id,type,seq,data) VALUES ('msg_origin','ses_origin','assistant',10,?1)", [native_assistant(serde_json::json!([])).to_string()]).unwrap();
    connection.execute("UPDATE session_v2 SET fork_session_id='ses_origin',fork_boundary=?1,cost=.01,tokens_input=10,tokens_output=3,tokens_reasoning=2,tokens_cache_read=7,tokens_cache_write=5 WHERE id='ses_v2'",
        [serde_json::json!({"type":"through","messageID":"msg_origin"}).to_string()]).unwrap();
    insert_value(
        &path,
        "msg_forkevent_10",
        "assistant",
        10,
        native_assistant(serde_json::json!([])),
    );
    insert_value(
        &path,
        "msg_new",
        "assistant",
        11,
        native_assistant(serde_json::json!([])),
    );
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert_eq!(sink.events.len(), 1);
    assert_eq!(sink.events[0].uuid.as_deref(), Some("msg_new"));
    assert_eq!(sink.events[0].subagent_launch_count(), 0);
    assert!(
        sink.observations
            .contains(&EvidenceObservation::InheritedRecord)
    );
    connection
        .execute("DELETE FROM session_message WHERE id='msg_origin'", [])
        .unwrap();
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert!(sink.events.is_empty());
    assert!(
        sink.unusable
            .contains(&antiburn_local::analysis::PartialReason::AttributionIncomplete)
    );
}

#[test]
fn descendants_are_inclusive_and_delegation_uses_actual_child_history_not_current_selection() {
    use antiburn_local::analysis::{EventSource, EvidenceObservation};
    let (_directory, path, input) = fixture();
    Connection::open(&path)
        .unwrap()
        .execute_batch("INSERT INTO session_v2 (id,parent_id) VALUES ('ses_child','ses_v2');")
        .unwrap();
    let subagent = native_tool(
        "subagent",
        "delegate-1",
        serde_json::json!({"agent":"review","description":"Inspect parser","prompt":"Review it"}),
        serde_json::json!({"sessionID":"ses_child","status":"completed"}),
        "<subagent sessionID=\"ses_child\" state=\"completed\">done</subagent>",
    );
    let explicit_model_subagent = native_tool(
        "subagent",
        "delegate-2",
        serde_json::json!({"agent":"review","description":"Inspect parser","prompt":"Review it","model":"test-provider/requested-model"}),
        serde_json::json!({"sessionID":"ses_child","status":"completed"}),
        "<subagent sessionID=\"ses_child\" state=\"completed\">done</subagent>",
    );
    insert_value(
        &path,
        "msg_parent",
        "assistant",
        0,
        native_assistant(serde_json::json!([subagent, explicit_model_subagent])),
    );
    let mut child = native_assistant(serde_json::json!([]));
    child["model"]["id"] = serde_json::json!("actual-child-model");
    child["time"]["created"] = serde_json::json!(120);
    Connection::open(&path).unwrap().execute(
        "INSERT INTO session_message (id,session_id,type,seq,data) VALUES ('msg_child','ses_child','assistant',0,?1)", [child.to_string()],
    ).unwrap();
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert_eq!(sink.events.len(), 2);
    assert_eq!(sink.events[1].source, EventSource::Subagent);
    assert_eq!(sink.events[1].thread_id.as_deref(), Some("ses_child"));
    assert!(sink.observations.iter().any(|observation| matches!(observation,
        EvidenceObservation::SubagentSpawn { child_model: Some(model), parent_call_id: Some(call), config_binding: Some(_), .. } if model == "actual-child-model" && call == "delegate-1")));
    assert!(sink.observations.iter().any(|observation| matches!(observation,
        EvidenceObservation::SubagentSpawn { child_model: Some(model), parent_call_id: Some(call), config_binding: None, .. } if model == "actual-child-model" && call == "delegate-2")));
    child["model"]["id"] = serde_json::json!("later-child-model");
    Connection::open(&path).unwrap().execute(
        "INSERT INTO session_message (id,session_id,type,seq,data) VALUES ('msg_child_later','ses_child','assistant',1,?1)", [child.to_string()],
    ).unwrap();
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert!(sink.observations.iter().any(|observation| matches!(
        observation,
        EvidenceObservation::SubagentSpawn {
            child_model: None,
            ..
        }
    )));
}

#[test]
fn proven_subagent_launches_exclude_continuations_provider_calls_and_unbound_children() {
    use antiburn_local::analysis::{
        FenceScope, MemoryTurnRowStore, SessionMetricsAccumulator, TurnRowSink, TurnSessionKey,
        metrics_from_rows, query_turn_rows, turn_row_from_event,
    };

    let (_directory, path, input) = fixture();
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "INSERT INTO session_v2 (id,parent_id) VALUES
                 ('ses_child','ses_v2'),('ses_background','ses_v2'),('ses_foreign',NULL);",
        )
        .unwrap();
    let create = native_tool(
        "subagent",
        "create",
        serde_json::json!({"agent":"review","description":"Inspect parser","prompt":"Review it"}),
        serde_json::json!({"sessionID":"ses_child","status":"completed"}),
        "done",
    );
    let mut continued = create.clone();
    continued["id"] = serde_json::json!("continued");
    continued["state"]["input"]["sessionID"] = serde_json::json!("ses_child");
    let mut background = create.clone();
    background["id"] = serde_json::json!("background");
    background["state"]["input"]["background"] = serde_json::json!(true);
    background["state"]["input"]["sessionID"] = serde_json::json!("");
    background["state"]["metadata"] =
        serde_json::json!({"sessionID":"ses_background","status":"running"});
    let mut running = create.clone();
    running["id"] = serde_json::json!("running");
    running["state"]["status"] = serde_json::json!("running");
    running["state"]["metadata"]["status"] = serde_json::json!("running");
    running["state"].as_object_mut().unwrap().remove("content");
    let mut provider = create.clone();
    provider["id"] = serde_json::json!("provider");
    provider["executed"] = serde_json::json!(true);
    let mut foreign = create.clone();
    foreign["id"] = serde_json::json!("foreign");
    foreign["state"]["metadata"]["sessionID"] = serde_json::json!("ses_foreign");
    let mut invalid = create.clone();
    invalid["id"] = serde_json::json!("invalid");
    invalid["state"]["input"]["sessionID"] = serde_json::Value::Null;
    let mut repeated_child = create.clone();
    repeated_child["id"] = serde_json::json!("repeated-child");
    insert_value(
        &path,
        "msg_launches",
        "assistant",
        0,
        native_assistant(serde_json::json!([
            create, continued, background, running, provider, foreign, invalid
        ])),
    );
    insert_value(
        &path,
        "msg_repeated_child",
        "assistant",
        1,
        native_assistant(serde_json::json!([repeated_child])),
    );
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert_eq!(sink.events.len(), 2);
    assert_eq!(sink.events[0].subagent_launch_count(), 2);
    assert_eq!(sink.events[1].subagent_launch_count(), 0);
    assert!(
        sink.events[0]
            .tools
            .iter()
            .all(|tool| tool.name == "subagent")
    );
    let row = turn_row_from_event(&sink.events[0], "ses_v2", 0);
    assert_eq!(row.subagent_launches, 2);
    let store = MemoryTurnRowStore::new("opencode", "ses_v2");
    let mut row_sink = TurnRowSink::new(store.clone(), "ses_v2", None);
    let mut metrics = SessionMetricsAccumulator::new("opencode", "ses_v2");
    for event in sink.events {
        row_sink.record(NormalizedRecord::MetricsEvent(Box::new(event.clone())));
        metrics.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }
    let summary = sink.summary.unwrap();
    row_sink.finish(summary.clone());
    metrics.finish(summary.clone());
    assert_eq!(
        metrics
            .metrics()
            .buckets
            .iter()
            .map(|bucket| bucket.subagent_launches)
            .sum::<u32>(),
        2
    );
    let rows = store.with_connection(|connection| {
        query_turn_rows(
            connection,
            &TurnSessionKey {
                environment_key: "native",
                agent: "opencode",
                session_id: "ses_v2",
            },
            &FenceScope::single(1),
        )
        .unwrap()
    });
    assert_eq!(
        rows.iter()
            .map(|row| row.subagent_launches)
            .collect::<Vec<_>>(),
        [2, 0]
    );
    let replay = metrics_from_rows("opencode", "ses_v2", &rows, |_| summary.clone()).unwrap();
    assert_eq!(
        replay
            .buckets
            .iter()
            .map(|bucket| bucket.subagent_launches)
            .collect::<Vec<_>>(),
        metrics
            .metrics()
            .buckets
            .iter()
            .map(|bucket| bucket.subagent_launches)
            .collect::<Vec<_>>()
    );
}

#[test]
fn proven_launch_count_keeps_legacy_names_and_serialized_snapshots_unchanged() {
    use antiburn_local::analysis::{Role, ToolCall};

    let mut event = NormalizedEvent::new(Role::Assistant);
    event.tools = vec![ToolCall::new("Task"), ToolCall::new("subagent")];
    assert_eq!(event.subagent_launch_count(), 1);
    let serialized = postcard::to_allocvec(&event).unwrap();
    event.proven_subagent_launches = Some(0);
    assert_eq!(event.subagent_launch_count(), 0);
    assert_eq!(postcard::to_allocvec(&event).unwrap(), serialized);
}

#[test]
fn source_boundaries_keep_per_session_summaries_and_one_wal_snapshot() {
    struct MutatingSink {
        captured: CaptureSink,
        writer: Connection,
    }
    impl RecordSink for MutatingSink {
        fn record(&mut self, record: NormalizedRecord) {
            self.captured.record(record);
        }

        fn source_summary(&mut self, session_id: &str, summary: SessionSummary) {
            if session_id == "ses_v2" {
                self.writer
                    .execute(
                        "UPDATE session_message SET data = json_set(data, '$.tokens.input', 999)
                         WHERE session_id = 'ses_child'",
                        [],
                    )
                    .unwrap();
            }
            self.captured.source_summary(session_id, summary);
        }

        fn finish(&mut self, summary: SessionSummary) {
            self.captured.finish(summary);
        }
    }

    let (_directory, path, input) = fixture();
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             INSERT INTO session_v2 (id,parent_id) VALUES ('ses_child','ses_v2');",
        )
        .unwrap();
    insert_value(
        &path,
        "msg_parent",
        "assistant",
        0,
        native_assistant(serde_json::json!([])),
    );
    let mut child = native_assistant(serde_json::json!([]));
    child["model"]["id"] = serde_json::json!("child-model");
    writer
        .execute(
            "INSERT INTO session_message (id,session_id,type,seq,data)
             VALUES ('msg_child','ses_child','assistant',0,?1)",
            [child.to_string()],
        )
        .unwrap();
    let mut sink = MutatingSink {
        captured: CaptureSink::default(),
        writer,
    };
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    let captured = sink.captured;
    assert_eq!(captured.events.len(), 2);
    assert_eq!(captured.events[1].usage.input_tokens, 10);
    assert_eq!(captured.source_summaries.len(), 2);
    assert_eq!(captured.source_summaries[0].0, "ses_v2");
    assert_eq!(captured.source_summaries[1].0, "ses_child");
    assert_eq!(
        captured.source_summaries[1].1.model.as_deref(),
        Some("child-model")
    );
    assert_eq!(
        captured.summary.unwrap().model.as_deref(),
        Some("main-model")
    );
}

#[test]
fn malformed_child_model_or_time_keeps_parent_and_valid_child_suffix_partial() {
    use antiburn_local::analysis::{EventSource, EvidenceObservation, PartialReason};

    let malformed_fields = [
        ("/model/id", serde_json::json!(7)),
        ("/model/id", serde_json::json!(false)),
        ("/model/id", serde_json::json!({"id":"test-model"})),
        ("/model/id", serde_json::json!(["test-model"])),
        ("/model/id", serde_json::Value::Null),
        ("/time/created", serde_json::json!("bad")),
        ("/time/created", serde_json::json!(true)),
        ("/time/created", serde_json::json!(120.5)),
        ("/time/created", serde_json::json!(u64::MAX)),
        ("/time/created", serde_json::json!({"created":120})),
        ("/time/created", serde_json::json!([120])),
        ("/time/created", serde_json::Value::Null),
    ];
    for (pointer, invalid) in malformed_fields {
        for valid_prefix in [false, true] {
            let (_directory, path, input) = fixture();
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "INSERT INTO session_v2 (id,parent_id) VALUES ('ses_child','ses_v2');",
                )
                .unwrap();
            let subagent = native_tool(
                "subagent",
                "delegate-1",
                serde_json::json!({"agent":"review","description":"Inspect parser","prompt":"Review it"}),
                serde_json::json!({"sessionID":"ses_child","status":"completed"}),
                "<subagent sessionID=\"ses_child\" state=\"completed\">done</subagent>",
            );
            insert_value(
                &path,
                "msg_parent",
                "assistant",
                0,
                native_assistant(serde_json::json!([subagent])),
            );
            let mut child = native_assistant(serde_json::json!([]));
            child["time"]["created"] = serde_json::json!(120);
            if valid_prefix {
                connection
                    .execute(
                        "INSERT INTO session_message (id,session_id,type,seq,data)
                         VALUES ('msg_child_prefix','ses_child','assistant',0,?1)",
                        [child.to_string()],
                    )
                    .unwrap();
            }
            let mut malformed = child.clone();
            *malformed.pointer_mut(pointer).unwrap() = invalid.clone();
            connection
                .execute(
                    "INSERT INTO session_message (id,session_id,type,seq,data)
                     VALUES ('msg_child_malformed','ses_child','assistant',1,?1)",
                    [malformed.to_string()],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO session_message (id,session_id,type,seq,data)
                     VALUES ('msg_child_suffix','ses_child','assistant',2,?1)",
                    [child.to_string()],
                )
                .unwrap();

            let mut sink = CaptureSink::default();
            OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
            let expected_ids = if valid_prefix {
                vec!["msg_parent", "msg_child_prefix", "msg_child_suffix"]
            } else {
                vec!["msg_parent", "msg_child_suffix"]
            };
            assert_eq!(
                sink.events
                    .iter()
                    .map(|event| event.uuid.as_deref().unwrap())
                    .collect::<Vec<_>>(),
                expected_ids,
                "{pointer}: {invalid}"
            );
            assert_eq!(sink.events[0].source, EventSource::Parent);
            let suffix = sink.events.last().unwrap();
            assert_eq!(suffix.source, EventSource::Subagent);
            assert_eq!(suffix.thread_id.as_deref(), Some("ses_child"));
            assert_eq!(suffix.usage.input_tokens, 10);
            let spawns = sink
                .observations
                .iter()
                .filter_map(|observation| match observation {
                    EvidenceObservation::SubagentSpawn { child_model, .. } => Some(child_model),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(spawns, vec![&None], "{pointer}: {invalid}");
            assert!(sink.unusable.contains(&PartialReason::MalformedRecord));
            assert!(
                sink.summary
                    .as_ref()
                    .unwrap()
                    .coverage_gaps
                    .contains(&PartialReason::MalformedRecord)
            );
        }
    }
}

#[test]
fn all_pinned_non_assistant_rows_keep_source_authority_and_skill_use() {
    use antiburn_local::analysis::{ContentAuthority, EvidenceObservation};
    let (_directory, path, input) = fixture();
    let rows = [
        (
            "user",
            serde_json::json!({"text":"Review this","skills":[{"id":"review","name":"review","text":"Loaded skill guide"}]}),
        ),
        (
            "synthetic",
            serde_json::json!({"text":"Generated background completion"}),
        ),
        (
            "system",
            serde_json::json!({"text":"Frozen configuration update"}),
        ),
        (
            "skill",
            serde_json::json!({"skill":"review","name":"review","text":"Loaded skill document"}),
        ),
        ("agent-switched", serde_json::json!({"agent":"review"})),
        (
            "model-switched",
            serde_json::json!({"model":{"id":"new-selection","providerID":"test-provider","variant":"fast"}}),
        ),
        (
            "location-switched",
            serde_json::json!({"location":{"directory":"/repo/nested"}}),
        ),
        ("idle", serde_json::json!({"outcome":"succeeded"})),
        (
            "shell",
            serde_json::json!({"shellID":"sh_user","command":"echo done","status":"exited","exit":0,"output":{"output":"done","cursor":4,"size":4,"truncated":false}}),
        ),
    ];
    for (seq, (kind, mut value)) in rows.into_iter().enumerate() {
        value["time"] = serde_json::json!({"created":100});
        insert_value(&path, &format!("msg_{seq}"), kind, seq as i64, value);
    }
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert!(sink.unusable.is_empty());
    assert_eq!(sink.contents[0].parts[0].authority, ContentAuthority::User);
    assert_eq!(
        sink.contents[0].parts[1].authority,
        ContentAuthority::Unknown
    );
    assert!(
        sink.contents
            .iter()
            .flat_map(|content| &content.parts)
            .filter(|part| part.text.starts_with("Generated") || part.text.starts_with("Frozen"))
            .all(|part| part.authority == ContentAuthority::System)
    );
    assert_eq!(sink.observations.iter().filter(|observation| matches!(observation, EvidenceObservation::SkillInjection { name, .. } if name == "review")).count(), 2);
    assert_eq!(sink.user_messages, 0);
    assert!(sink.tool_results.is_empty());
}

#[test]
fn skill_tool_usage_uses_validated_input_id_not_display_name() {
    use antiburn_local::analysis::EvidenceObservation;

    let (_directory, path, input) = fixture();
    insert_value(
        &path,
        "msg_skill_tool",
        "assistant",
        0,
        native_assistant(serde_json::json!([native_tool(
            "skill",
            "skill-call",
            serde_json::json!({"id":"review-core"}),
            serde_json::json!({"name":"Code review"}),
            "Loaded skill guide"
        )])),
    );

    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();

    assert!(sink.observations.iter().any(|observation| matches!(
        observation,
        EvidenceObservation::SkillInjection { name, invoked: true }
            if name == "review-core"
    )));
    assert!(!sink.observations.iter().any(|observation| matches!(
        observation,
        EvidenceObservation::SkillInjection { name, .. }
            if name == "Code review"
    )));
    assert_eq!(
        sink.events[0].tools[0].detail.as_deref(),
        Some("review-core")
    );
    let proof = sink
        .contents
        .iter()
        .flat_map(|content| &content.parts)
        .find_map(|part| {
            part.metadata
                .recorded_skill_result
                .as_ref()?
                .native_skill
                .as_ref()
                .filter(|proof| proof.display_name.is_some())
        })
        .unwrap();
    assert_eq!(proof.id, "review-core");
    assert_eq!(proof.display_name.as_deref(), Some("Code review"));
}

#[test]
fn same_time_projection_order_is_not_tool_completion_order() {
    let (_directory, path, input) = fixture();
    let mut first = native_assistant(serde_json::json!([
        native_tool(
            "shell",
            "late",
            serde_json::json!({"command":"echo late"}),
            serde_json::json!({}),
            "late"
        ),
        native_tool(
            "shell",
            "early",
            serde_json::json!({"command":"echo early"}),
            serde_json::json!({}),
            "early"
        )
    ]));
    first["content"][0]["time"]["completed"] = serde_json::json!(400);
    first["content"][1]["time"]["completed"] = serde_json::json!(200);
    insert_value(&path, "msg_z", "assistant", 1, first);
    insert_value(
        &path,
        "msg_a",
        "assistant",
        2,
        native_assistant(serde_json::json!([])),
    );
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert_eq!(
        sink.events
            .iter()
            .map(|event| event.uuid.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["msg_z", "msg_a"]
    );
    assert!(sink.tool_results.is_empty());
    assert_eq!(sink.user_messages, 0);
    assert_eq!(sink.tool_result_gaps, 1);
}

#[test]
fn claimed_snapshot_rejects_stale_mutable_rows_before_emitting_records() {
    use antiburn_local::discovery::agents::opencode::db_session_fingerprint_snapshot;
    let (_directory, path, input) = fixture();
    insert_value(
        &path,
        "msg_first",
        "assistant",
        0,
        native_assistant(serde_json::json!([])),
    );
    let (_, (hash, rows)) = db_session_fingerprint_snapshot(&path, "ses_v2", &|| false).unwrap();
    let claim = format!("sv1:db:{hash}:{rows}");
    let mut sink = CaptureSink::default();
    assert_eq!(
        OpenCodeV2SessionReader
            .visit_db_claimed(&input, &claim, &|| false, &mut sink)
            .unwrap(),
        VisitOutcome::AcceptedFull
    );
    Connection::open(&path).unwrap().execute("UPDATE session_message SET data = json_set(data,'$.tokens.input',30) WHERE id='msg_first'", []).unwrap();
    let mut sink = CaptureSink::default();
    assert!(matches!(
        OpenCodeV2SessionReader
            .visit_db_claimed(&input, &claim, &|| false, &mut sink)
            .unwrap(),
        VisitOutcome::SourceChanged(_)
    ));
    assert!(sink.events.is_empty());
    assert!(sink.summary.is_none());
}

#[test]
fn claimed_fingerprint_and_transcript_share_one_wal_read_transaction() {
    use antiburn_local::discovery::agents::opencode::db_session_fingerprint_snapshot;
    let (_directory, path, input) = fixture();
    let writer = Connection::open(&path).unwrap();
    writer.pragma_update(None, "journal_mode", "WAL").unwrap();
    insert_value(
        &path,
        "msg_first",
        "assistant",
        0,
        native_assistant(serde_json::json!([])),
    );
    insert_value(
        &path,
        "msg_second",
        "assistant",
        1,
        native_assistant(serde_json::json!([])),
    );
    let (_, (hash, rows)) = db_session_fingerprint_snapshot(&path, "ses_v2", &|| false).unwrap();
    let claim = format!("sv1:db:{hash}:{rows}");
    struct UpdatingSink {
        capture: CaptureSink,
        writer: Connection,
        changed: bool,
    }
    impl RecordSink for UpdatingSink {
        fn record(&mut self, record: NormalizedRecord) {
            if !self.changed && matches!(&record, NormalizedRecord::MetricsEvent(_)) {
                self.writer.execute("UPDATE session_message SET data=json_set(data,'$.tokens.input',99) WHERE id='msg_second'", []).unwrap();
                self.changed = true;
            }
            self.capture.record(record);
        }
        fn finish(&mut self, summary: SessionSummary) {
            self.capture.finish(summary);
        }
        fn tool_result_gap(&mut self) {
            self.capture.tool_result_gap();
        }
    }
    let mut sink = UpdatingSink {
        capture: CaptureSink::default(),
        writer,
        changed: false,
    };
    assert_eq!(
        OpenCodeV2SessionReader
            .visit_db_claimed(&input, &claim, &|| false, &mut sink)
            .unwrap(),
        VisitOutcome::AcceptedFull
    );
    assert!(sink.changed);
    assert_eq!(sink.capture.events[1].usage.input_tokens, 10);
    let mut next = CaptureSink::default();
    assert!(matches!(
        OpenCodeV2SessionReader
            .visit_db_claimed(&input, &claim, &|| false, &mut next)
            .unwrap(),
        VisitOutcome::SourceChanged(_)
    ));
    assert!(next.events.is_empty());
}

#[test]
fn malformed_rows_and_missing_content_keep_usable_usage_and_the_valid_suffix() {
    let (_directory, path, input) = fixture();
    insert(&path, "msg_bad_json", "assistant", 0, "{");
    let mut without_content = native_assistant(serde_json::json!([]));
    without_content.as_object_mut().unwrap().remove("content");
    insert_value(&path, "msg_usage_only", "assistant", 1, without_content);
    insert_value(
        &path,
        "msg_after",
        "assistant",
        2,
        native_assistant(serde_json::json!([])),
    );
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert_eq!(sink.events.len(), 2);
    assert_eq!(
        sink.events
            .iter()
            .map(|event| event.usage.input_tokens)
            .sum::<u64>(),
        20
    );
    assert!(
        sink.unusable
            .contains(&antiburn_local::analysis::PartialReason::MalformedRecord)
    );
}

#[test]
fn native_read_directories_and_media_do_not_borrow_text_file_ranges() {
    use antiburn_local::analysis::jev_evidence::{JevReadResultKind, JevReadUnit};
    let (_directory, path, input) = fixture();
    let directory = native_tool(
        "read",
        "dir",
        serde_json::json!({"path":"src","offset":2,"limit":2}),
        serde_json::json!({"truncated":false}),
        "Read directory src, entries 2-3\nmain.rs\nlib.rs",
    );
    let mut image = native_tool(
        "read",
        "image",
        serde_json::json!({"path":"image.png"}),
        serde_json::json!({"truncated":false}),
        "Image read successfully",
    );
    image["state"]["content"].as_array_mut().unwrap().push(serde_json::json!({"type":"file","uri":"data:image/png;base64,aW1hZ2U=","mime":"image/png","name":"image.png"}));
    let mut pdf = native_tool(
        "read",
        "pdf",
        serde_json::json!({"path":"report.pdf"}),
        serde_json::json!({"truncated":false}),
        "PDF read successfully",
    );
    pdf["state"]["content"].as_array_mut().unwrap().push(serde_json::json!({"type":"file","uri":"data:application/pdf;base64,cGRm","mime":"application/pdf","name":"report.pdf"}));
    insert_value(
        &path,
        "msg_reads",
        "assistant",
        0,
        native_assistant(serde_json::json!([directory, image, pdf])),
    );
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    let parts = &sink.contents[0].parts;
    let request = parts
        .iter()
        .find(|part| {
            part.kind == ContentKind::ToolInput && part.tool_call_id.as_deref() == Some("dir")
        })
        .unwrap()
        .metadata
        .read_request
        .as_ref()
        .unwrap();
    assert_eq!(request.extent.unit, JevReadUnit::Unknown);
    let results = parts
        .iter()
        .filter_map(|part| part.metadata.read_result.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(results[0].kind, JevReadResultKind::Directory);
    assert!(
        results
            .iter()
            .all(|result| result.returned_extent.is_none())
    );
    assert!(
        sink.unusable
            .contains(&antiburn_local::analysis::PartialReason::UnrecognizedRecordType)
    );
    assert!(!parts.iter().any(|part| part.text.contains("aW1hZ2U=")));
    assert!(!parts.iter().any(|part| part.text.contains("cGRm")));
}

#[test]
fn native_tool_errors_keep_error_text_and_provider_execution_stays_untyped() {
    use antiburn_local::analysis::jev_evidence::JevOperationState;
    let (_directory, path, input) = fixture();
    let mut failed = native_tool(
        "read",
        "failed",
        serde_json::json!({"path":"missing.rs"}),
        serde_json::json!({}),
        "unused",
    );
    failed["state"] = serde_json::json!({"status":"error","input":{"path":"missing.rs"},"error":{"type":"Tool.Error","message":"Unable to read missing.rs"}});
    let mut provider = native_tool(
        "read",
        "provider",
        serde_json::json!({"path":"remote.rs"}),
        serde_json::json!({}),
        "provider result",
    );
    provider["executed"] = serde_json::json!(true);
    insert_value(
        &path,
        "msg_errors",
        "assistant",
        0,
        native_assistant(serde_json::json!([failed, provider])),
    );
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    let parts = &sink.contents[0].parts;
    let failed = parts
        .iter()
        .find(|part| {
            part.kind == ContentKind::ToolResult && part.tool_call_id.as_deref() == Some("failed")
        })
        .unwrap();
    assert_eq!(failed.metadata.state, JevOperationState::Error);
    assert_eq!(failed.text, "Unable to read missing.rs");
    let provider = parts
        .iter()
        .filter(|part| part.tool_call_id.as_deref() == Some("provider"))
        .collect::<Vec<_>>();
    assert!(provider.iter().all(|part| part.metadata.read_result.is_none() && part.metadata.read_request.is_none()));
    assert!(provider[0].normalized_fields.is_none());
}

#[test]
fn v1_argument_aliases_do_not_become_v2_read_or_edit_fields() {
    use antiburn_local::analysis::jev::JevInputField;
    let (_directory, path, input) = fixture();
    insert_value(
        &path,
        "msg_old_arguments",
        "assistant",
        0,
        native_assistant(serde_json::json!([
            native_tool(
                "read",
                "legacy-read",
                serde_json::json!({"filePath":"legacy.rs"}),
                serde_json::json!({}),
                "Read file legacy.rs, lines 1-1\n1: not a validated V2 call"
            ),
            native_tool(
                "write",
                "legacy-write",
                serde_json::json!({"filePath":"legacy.rs","content":"text"}),
                serde_json::json!({}),
                "Created file successfully"
            )
        ])),
    );
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert!(
        sink.unusable
            .contains(&antiburn_local::analysis::PartialReason::MalformedRecord)
    );
    let parts = &sink.contents[0].parts;
    assert!(parts.iter().all(|part| part.metadata.read_request.is_none() && part.metadata.read_result.is_none()));
    for fields in parts
        .iter()
        .filter_map(|part| part.normalized_fields.as_ref())
    {
        assert!(!fields.values.contains_key(&JevInputField::ReadFilePath));
        assert!(!fields.values.contains_key(&JevInputField::FileEditPath));
    }
}

#[test]
fn truncated_companion_paths_are_not_opened_or_treated_as_complete_output() {
    let (directory, path, input) = fixture();
    let companion = directory.path().join("private-output.txt");
    std::fs::write(&companion, "synthetic companion bytes must stay unread").unwrap();
    for (seq, output_path) in [companion, directory.path().join("../missing-output")]
        .into_iter()
        .enumerate()
    {
        insert_value(
            &path,
            &format!("msg_truncated_{seq}"),
            "assistant",
            seq as i64,
            native_assistant(serde_json::json!([native_tool(
                "execute",
                &format!("outer-{seq}"),
                serde_json::json!({"code":"return 'preview'"}),
                serde_json::json!({"truncated":true,"outputPath":output_path,"toolCalls":[]}),
                "preview"
            )])),
        );
    }
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert!(
        sink.unusable
            .contains(&antiburn_local::analysis::PartialReason::IncompleteTail)
    );
    let results = sink
        .contents
        .iter()
        .flat_map(|content| &content.parts)
        .filter(|part| part.kind == ContentKind::ToolResult)
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .all(|part| part.text == "preview" && part.truncated)
    );
    assert!(sink.tool_results.is_empty());
}

#[test]
fn edit_write_and_search_keep_only_native_v2_argument_fields() {
    use antiburn_local::analysis::{ToolCategory, jev::JevInputField};
    let (_directory, path, input) = fixture();
    insert_value(
        &path,
        "msg_files",
        "assistant",
        0,
        native_assistant(serde_json::json!([
            native_tool(
                "write",
                "write",
                serde_json::json!({"path":"new.rs","content":"new content"}),
                serde_json::json!({}),
                "Created file successfully: new.rs"
            ),
            native_tool(
                "edit",
                "edit",
                serde_json::json!({"path":"old.rs","oldString":"before","newString":"after","replaceAll":false}),
                serde_json::json!({}),
                "Updated old.rs"
            ),
            native_tool(
                "grep",
                "grep",
                serde_json::json!({"pattern":"symbol","path":"src","include":"*.rs","literal":true,"caseSensitive":false,"limit":10}),
                serde_json::json!({"truncated":false}),
                "No matches found"
            ),
            native_tool(
                "glob",
                "glob",
                serde_json::json!({"pattern":"**/*.rs","path":"src","hidden":false,"limit":10}),
                serde_json::json!({"truncated":false}),
                "src/main.rs"
            )
        ])),
    );
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert_eq!(
        sink.events[0]
            .tools
            .iter()
            .map(|tool| tool.category)
            .collect::<Vec<_>>(),
        [
            ToolCategory::Edit,
            ToolCategory::Edit,
            ToolCategory::Search,
            ToolCategory::Search
        ]
    );
    let requests = sink.contents[0]
        .parts
        .iter()
        .filter(|part| part.kind == ContentKind::ToolInput)
        .collect::<Vec<_>>();
    assert!(
        requests[0].normalized_fields.as_ref().unwrap().values[&JevInputField::FileEditPath]
            .contains("new.rs")
    );
    assert!(
        requests[1].normalized_fields.as_ref().unwrap().values[&JevInputField::FileEditContent]
            .contains("after")
    );
    assert!(
        requests[2].normalized_fields.as_ref().unwrap().values[&JevInputField::SearchFilesQuery]
            .contains("symbol")
    );
    assert!(sink.unusable.is_empty());
}

#[test]
fn empty_native_output_stays_empty_rather_than_becoming_missing_output() {
    let (_directory, path, input) = fixture();
    insert_value(
        &path,
        "msg_empty",
        "assistant",
        0,
        native_assistant(serde_json::json!([native_tool(
            "execute",
            "empty",
            serde_json::json!({"code":"return undefined"}),
            serde_json::json!({"toolCalls":[]}),
            ""
        )])),
    );
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    let result = sink.contents[0]
        .parts
        .iter()
        .find(|part| part.kind == ContentKind::ToolResult)
        .unwrap();
    assert_eq!(result.text, "");
    assert!(!result.truncated);
    assert!(sink.unusable.is_empty());
}

#[test]
fn continued_or_background_children_do_not_inherit_an_old_child_model() {
    use antiburn_local::analysis::EvidenceObservation;
    for background in [false, true] {
        let (_directory, path, input) = fixture();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO session_v2 (id,parent_id) VALUES ('ses_child','ses_v2')",
                [],
            )
            .unwrap();
        let mut child = native_assistant(serde_json::json!([]));
        child["time"]["created"] = serde_json::json!(120);
        connection.execute("INSERT INTO session_message (id,session_id,type,seq,data) VALUES ('msg_child','ses_child','assistant',0,?1)", [child.to_string()]).unwrap();
        let mut arguments = serde_json::json!({"agent":"review","description":"Review parser","prompt":"Inspect it"});
        if !background {
            arguments["sessionID"] = serde_json::json!("ses_child");
        }
        let status = if background { "running" } else { "completed" };
        insert_value(
            &path,
            "msg_parent",
            "assistant",
            0,
            native_assistant(serde_json::json!([native_tool(
                "subagent",
                "delegate",
                arguments,
                serde_json::json!({"sessionID":"ses_child","status":status}),
                "Child state is recorded"
            )])),
        );
        let mut sink = CaptureSink::default();
        OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
        assert!(sink.observations.iter().any(|observation| matches!(
            observation,
            EvidenceObservation::SubagentSpawn {
                child_model: None,
                ..
            }
        )));
    }
}

#[test]
fn injected_skills_keep_native_ids_duplicate_names_and_unavailable_bodies() {
    use antiburn_local::analysis::{
        ContentAuthority,
        jev_evidence::{JevNativeSkillBody, JevNativeSkillOrigin, JevOperationState},
    };
    let (_directory, path, input) = fixture();
    insert_value(
        &path,
        "msg_user",
        "user",
        0,
        serde_json::json!({
            "time":{"created":1},"text":"accepted user slot",
            "skills":[
                {"id":"native-one","name":"Shared display name","text":"First guide"},
                {"id":"native-two","name":"Shared display name","text":"Second guide"},
                {"id":"native-one","name":"Changed display name"}
            ]
        }),
    );
    insert_value(
        &path,
        "msg_skill",
        "skill",
        1,
        serde_json::json!({
            "time":{"created":2},"skill":"native-three","name":"Third display name","text":"Third guide"
        }),
    );
    let mut sink = CaptureSink::default();
    OpenCodeV2SessionReader.visit(&input, &mut sink).unwrap();
    assert_eq!(sink.contents[0].parts[0].authority, ContentAuthority::User);
    let injected = sink
        .contents
        .iter()
        .flat_map(|content| &content.parts)
        .filter(|part| part.metadata.recorded_skill_result.is_some())
        .collect::<Vec<_>>();
    assert_eq!(injected.len(), 4);
    let ids = injected
        .iter()
        .map(|part| {
            part.metadata
                .recorded_skill_result
                .as_ref()
                .unwrap()
                .native_skill
                .as_ref()
                .unwrap()
                .id
                .as_str()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        ["native-one", "native-two", "native-one", "native-three"]
    );
    for part in &injected {
        assert_eq!(part.kind, ContentKind::UserText);
        assert_eq!(part.authority, ContentAuthority::Unknown);
        assert_eq!(part.metadata.state, JevOperationState::Unknown);
        assert!(part.tool_name.is_none() && part.tool_call_id.is_none());
        assert!(part.metadata.human_text.is_none());
        assert!(
            part.metadata
                .recorded_skill_result
                .as_ref()
                .unwrap()
                .location
                .is_none()
        );
    }
    let missing = injected[2].metadata.recorded_skill_result.as_ref().unwrap();
    assert!(injected[2].text.is_empty());
    assert!(!missing.complete);
    assert_eq!(
        missing.native_skill.as_ref().unwrap().body,
        JevNativeSkillBody::Unavailable
    );
    assert_eq!(
        missing.native_skill.as_ref().unwrap().origin,
        JevNativeSkillOrigin::PromptAttachment { index: 2 }
    );
    assert_eq!(missing.ranges.len(), 2);
    assert_eq!(missing.ranges[0].pointer, "/skills/2/id");
    assert_eq!(injected[3].metadata.bindings[0].pointer, "/skill");
    assert!(sink.tool_results.is_empty());
}
