use antiburn_local::analysis::{
    ContentAuthority, ContentKind, ContentPart, NormalizedRecord, PartialReason, RawSource,
    RecordSink, SessionInput, SessionSummary, SourceFormat, UserMessage, UserMessageChange,
    UserMessageCompleteness, UserMessageOrigin, UserMessagePosition, UserMessageScope, reader_for,
};
use rusqlite::{Connection, params};
use serde_json::Value;
use tempfile::TempDir;

#[derive(Default)]
struct RequestSink {
    requests: Vec<UserMessage>,
    result_gap: bool,
    unusable: Vec<PartialReason>,
    parts: Vec<ContentPart>,
}

impl RecordSink for RequestSink {
    fn record(&mut self, record: NormalizedRecord) {
        match record {
            NormalizedRecord::Unusable(reason) => self.unusable.push(reason),
            NormalizedRecord::TurnContent(content) => self.parts.extend(content.parts),
            _ => {}
        }
    }

    fn user_message(&mut self, request: UserMessage) {
        self.requests.push(request);
    }

    fn finish(&mut self, _summary: SessionSummary) {}

    fn tool_result_gap(&mut self) {
        self.result_gap = true;
    }
}

fn jsonl_input(
    agent: &str,
    source_format: SourceFormat,
    session_id: &str,
    source: &str,
) -> SessionInput {
    SessionInput {
        agent: agent.to_owned(),
        session_id: session_id.to_owned(),
        source: RawSource::Jsonl(source.to_owned()),
        source_format,
        fork_parent_session_id: None,
    }
}

#[test]
fn claude_emits_user_slot_content_from_the_retained_root() {
    let input = jsonl_input(
        "claude",
        SourceFormat::ClaudeJsonl,
        "scope-test",
        include_str!("fixtures/claude_characterization/retained_native_results.jsonl"),
    );
    let mut sink = RequestSink::default();
    reader_for("claude").visit(&input, &mut sink).unwrap();

    assert_eq!(sink.requests.len(), 2);
    assert_eq!(
        sink.requests[0].message_id,
        "00000000-0000-0000-0000-000000000001"
    );
    assert_eq!(
        sink.requests[0].parts[0].text,
        "Read three lines and run the API tests. Keep billing unchanged."
    );
    assert_eq!(
        sink.requests[1].message_id,
        "00000000-0000-0000-0000-00000000000a"
    );
    assert_eq!(
        sink.requests[0].position,
        UserMessagePosition::Jsonl { record_index: 0 }
    );
    assert_eq!(
        sink.requests[1].position,
        UserMessagePosition::Jsonl { record_index: 9 }
    );
    assert!(sink.requests.iter().all(|request| {
        request.source_format == SourceFormat::ClaudeJsonl
            && request.session_id == "scope-test"
            && request.completeness == UserMessageCompleteness::Complete
    }));
    assert_eq!(
        sink.requests[1].parts[0].text,
        "Stop. Do not make further edits."
    );
    assert_eq!(
        sink.requests[0].origin,
        UserMessageOrigin::ClaudeUserContent
    );
    assert!(sink.requests.iter().all(|request| {
        request.scope == UserMessageScope::MainSession
            && request.change == UserMessageChange::Present
            && request.completeness == UserMessageCompleteness::Complete
            && request.is_bounded()
    }));
    assert!(sink.requests.iter().all(|request| {
        !request.parts.iter().any(|part| {
            part.text.contains("Unknown skill")
                || part.text.contains("change billing too")
                || part.text.contains("Base directory for this skill")
        })
    }));
}

#[test]
fn codex_emits_only_complete_retained_user_text_in_the_root_rollout() {
    let input = jsonl_input(
        "codex",
        SourceFormat::CodexRolloutJsonl,
        "synthetic-root",
        include_str!("fixtures/codex_characterization/paginated_completed.jsonl"),
    );
    let mut sink = RequestSink::default();
    reader_for("codex").visit(&input, &mut sink).unwrap();

    assert_eq!(sink.requests.len(), 1);
    let request = &sink.requests[0];
    assert_eq!(request.message_id, "human-1");
    assert_eq!(request.parts.len(), 1);
    assert_eq!(request.parts[0].text, "Review the sample. Do not publish.");
    assert_eq!(request.source_format, SourceFormat::CodexRolloutJsonl);
    assert_eq!(request.session_id, "synthetic-root");
    assert_eq!(
        request.position,
        UserMessagePosition::Jsonl { record_index: 2 }
    );
    assert_eq!(request.origin, UserMessageOrigin::CodexRetainedUserText);
    assert_eq!(request.scope, UserMessageScope::MainSession);
    assert!(request.is_bounded());
    assert!(!request.parts[0].text.contains("Check the sample."));
}

#[test]
fn claude_admits_api_and_unmarked_user_content_without_changing_legacy_human_proofs() {
    let source = include_str!("fixtures/claude_characterization/retained_native_results.jsonl")
        .lines()
        .map(|line| {
            let mut record: Value = serde_json::from_str(line).unwrap();
            if record["uuid"] == "00000000-0000-0000-0000-000000000001" {
                let object = record.as_object_mut().unwrap();
                object.remove("origin");
                object.remove("promptSource");
                object.remove("turnOrigin");
            }
            if record["uuid"] == "00000000-0000-0000-0000-00000000000a" {
                record["origin"] = serde_json::json!({"kind": "api"});
                record["promptSource"] = serde_json::json!("extension");
                record["turnOrigin"] = serde_json::json!("extension");
            }
            record.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let input = jsonl_input("claude", SourceFormat::ClaudeJsonl, "scope-test", &source);
    let mut sink = RequestSink::default();
    reader_for("claude").visit(&input, &mut sink).unwrap();
    assert_eq!(sink.requests.len(), 2);
    assert_eq!(
        sink.requests[0].position,
        UserMessagePosition::Jsonl { record_index: 0 }
    );
    assert_eq!(
        sink.requests[1].parts[0].text,
        "Stop. Do not make further edits."
    );
    assert_eq!(
        sink.requests[1].message_id,
        "00000000-0000-0000-0000-00000000000a"
    );
    assert!(
        sink.requests
            .iter()
            .flat_map(|request| &request.parts)
            .all(|part| {
                !part.text.contains("change billing too")
                    && !part.text.contains("Base directory for this skill")
            })
    );
    let user_parts: Vec<_> = sink
        .parts
        .iter()
        .filter(|part| part.kind == ContentKind::UserText)
        .collect();
    assert!(!user_parts.is_empty());
    assert!(user_parts.iter().all(|part| {
        part.authority == ContentAuthority::Unknown && part.metadata.user_text_history.is_none()
    }));
}

#[test]
fn opencode_sqlite_excludes_synthetic_user_slot_text() {
    let (_directory, input) = opencode_input();
    let mut sink = RequestSink::default();
    reader_for("opencode").visit(&input, &mut sink).unwrap();

    assert_eq!(sink.requests.len(), 1);
    let request = &sink.requests[0];
    assert_eq!(request.message_id, "human-1");
    assert_eq!(
        request.parts[0].text,
        "Review the API. Keep billing unchanged."
    );
    assert_eq!(request.source_format, SourceFormat::OpenCodeSqliteV1);
    assert_eq!(request.session_id, "root");
    assert_eq!(
        request.position,
        UserMessagePosition::OpenCodeSqlite {
            created_at_ms: 1_000_000
        }
    );
    assert_eq!(request.parts[0].native_part_id.as_deref(), None);
    assert_eq!(request.origin, UserMessageOrigin::OpenCodeUserText);
    assert_eq!(request.scope, UserMessageScope::MainSession);
    assert!(request.is_bounded());
    assert!(sink.unusable.is_empty());
    assert!(sink.result_gap);
}

#[test]
fn opencode_updated_user_rows_mark_publication_incomplete_without_legacy_attribution_loss() {
    let (_directory, input) = opencode_input();
    let RawSource::Sqlite(path) = &input.source else {
        unreachable!();
    };
    let connection = Connection::open(path).expect("database");
    connection
        .execute(
            "UPDATE message SET time_updated = time_created + 1 WHERE id = 'human-1'",
            [],
        )
        .expect("update message row");
    drop(connection);

    let mut sink = RequestSink::default();
    reader_for("opencode").visit(&input, &mut sink).unwrap();

    assert!(sink.requests.is_empty());
    assert!(sink.result_gap);
    assert!(
        !sink
            .unusable
            .contains(&PartialReason::AttributionIncomplete)
    );
}

#[test]
fn pi_emits_user_slot_content_without_claiming_sender_origin() {
    let input = jsonl_input(
        "pi",
        SourceFormat::PiV3Jsonl,
        "pi-core-linear-v3",
        include_str!("fixtures/pi_characterization/core_linear_v3.jsonl"),
    );
    let mut sink = RequestSink::default();
    reader_for("pi").visit(&input, &mut sink).unwrap();

    assert!(!sink.requests.is_empty());
    assert!(sink.requests.iter().all(|message| {
        message.origin == UserMessageOrigin::PiUserContent && message.is_bounded()
    }));
}

#[test]
fn user_message_facts_reject_oversized_or_mismatched_contracts() {
    let input = jsonl_input(
        "claude",
        SourceFormat::ClaudeJsonl,
        "scope-test",
        include_str!("fixtures/claude_characterization/retained_native_results.jsonl"),
    );
    let mut sink = RequestSink::default();
    reader_for("claude").visit(&input, &mut sink).unwrap();
    let mut request = sink.requests.into_iter().next().unwrap();
    request.parts[0].text = "x".repeat(antiburn_local::analysis::MAX_USER_MESSAGE_BYTES + 1);
    assert!(!request.is_bounded());

    request.parts[0].text = "request".to_owned();
    request.source_format = SourceFormat::PiV3Jsonl;
    assert!(!request.is_bounded());
}

#[test]
fn user_message_type_can_represent_removals_and_conflicts() {
    let input = jsonl_input(
        "claude",
        SourceFormat::ClaudeJsonl,
        "scope-test",
        include_str!("fixtures/claude_characterization/retained_native_results.jsonl"),
    );
    let mut sink = RequestSink::default();
    reader_for("claude").visit(&input, &mut sink).unwrap();
    let mut request = sink.requests.into_iter().next().unwrap();

    request.change = UserMessageChange::Removal;
    request.parts.clear();
    assert!(request.is_bounded());

    request.change = UserMessageChange::Conflict;
    request.completeness = UserMessageCompleteness::Incomplete;
    assert!(request.is_bounded());
}

fn opencode_input() -> (TempDir, SessionInput) {
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
             );
             INSERT INTO session VALUES ('root', NULL, NULL, 1000, 1000);",
        )
        .expect("schema");

    for line in include_str!("fixtures/opencode_characterization/user_messages.jsonl").lines() {
        let record: Value = serde_json::from_str(line).expect("fixture record");
        match record["type"].as_str().expect("record type") {
            "session_meta" => {}
            "message" => {
                let message_id = record["messageID"].as_str().expect("message ID");
                let created = record["time"]["created"].as_i64().expect("creation time");
                connection
                    .execute(
                        "INSERT INTO message VALUES (?1, 'root', ?2, ?2, ?3)",
                        params![message_id, created, record["payload"].to_string()],
                    )
                    .expect("message row");
            }
            "part" => {
                let part_id = record["partID"].as_str().expect("part ID");
                let message_id = record["messageID"].as_str().expect("message ID");
                let created = record["time"]["created"].as_i64().expect("creation time");
                connection
                    .execute(
                        "INSERT INTO part VALUES (?1, ?2, 'root', ?3, ?3, ?4)",
                        params![part_id, message_id, created, record["payload"].to_string()],
                    )
                    .expect("part row");
            }
            other => panic!("unexpected fixture type {other}"),
        }
    }
    drop(connection);

    let input = SessionInput {
        agent: "opencode".to_owned(),
        session_id: "root".to_owned(),
        source: RawSource::Sqlite(path),
        source_format: SourceFormat::OpenCodeSqliteV1,
        fork_parent_session_id: None,
    };
    (directory, input)
}
