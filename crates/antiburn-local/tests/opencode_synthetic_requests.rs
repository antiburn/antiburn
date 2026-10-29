use antiburn_local::analysis::{
    NormalizedRecord, RawSource, RecordSink, SessionInput, SessionSummary, SourceFormat,
    UserMessage, read_opencode_live_tool_result_page, read_opencode_tool_result_page, reader_for,
};
use rusqlite::{Connection, params};
use serde_json::json;

#[derive(Default)]
struct Requests {
    messages: Vec<UserMessage>,
    gap: bool,
}

impl RecordSink for Requests {
    fn record(&mut self, _: NormalizedRecord) {}

    fn user_message(&mut self, message: UserMessage) {
        self.messages.push(message);
    }

    fn tool_result_gap(&mut self) {
        self.gap = true;
    }

    fn finish(&mut self, _: SessionSummary) {}
}

fn database(path: &std::path::Path, real: bool, replacement: bool) {
    let connection = Connection::open(path).unwrap();
    connection.execute_batch(
        r#"CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, time_created INTEGER, time_updated INTEGER);
         CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
         CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
         CREATE TABLE event_sequence (aggregate_id TEXT PRIMARY KEY, seq INTEGER NOT NULL, owner_id TEXT);
         CREATE TABLE event (id TEXT PRIMARY KEY, aggregate_id TEXT NOT NULL, seq INTEGER NOT NULL, type TEXT NOT NULL, data TEXT NOT NULL);
         CREATE UNIQUE INDEX event_aggregate_seq_idx ON event(aggregate_id, seq);
         INSERT INTO session VALUES ('root', NULL, 1000, 1000);
         INSERT INTO message VALUES ('user', 'root', 1000, 1000, '{"role":"user"}');
         INSERT INTO message VALUES ('assistant', 'root', 2000, 2000, '{"role":"assistant","parentID":"user"}');"#,
    ).unwrap();
    let mut events = vec![
        ("session.created.1", json!({"info":{"id":"root"}})),
        (
            "message.updated.1",
            json!({"info":{"id":"user","role":"user"}}),
        ),
    ];
    let mut parts = Vec::new();
    if real {
        let part = json!({"id":"real","sessionID":"root","messageID":"user","type":"text","text":"Review the API. Do not publish.","synthetic":false});
        events.push(("message.part.updated.1", json!({"part":part})));
        if !replacement {
            parts.push(part);
        }
    }
    let generated = json!({"id":if replacement {"real"} else {"generated"},"sessionID":"root","messageID":"user","type":"text","text":"Implement the approved plan.","synthetic":true});
    events.push(("message.part.updated.1", json!({"part":generated})));
    parts.push(generated);
    events.push((
        "message.updated.1",
        json!({"info":{"id":"assistant","role":"assistant","parentID":"user"}}),
    ));
    events.push(("message.part.updated.1", json!({"part":{"id":"tool","sessionID":"root","messageID":"assistant","type":"tool","callID":"call","tool":"bash","state":{"status":"error","input":{"command":"test-api"},"error":"Test failed."}}})));
    for part in parts {
        connection
            .execute(
                "INSERT INTO part VALUES (?1, 'user', 'root', 1000, 1000, ?2)",
                params![part["id"].as_str().unwrap(), part.to_string()],
            )
            .unwrap();
    }
    for (sequence, (kind, mut data)) in events.into_iter().enumerate() {
        data["sessionID"] = json!("root");
        connection
            .execute(
                "INSERT INTO event VALUES (?1, 'root', ?2, ?3, ?4)",
                params![
                    format!("event-{sequence}"),
                    sequence,
                    kind,
                    data.to_string()
                ],
            )
            .unwrap();
        connection.execute(
            "INSERT INTO event_sequence VALUES ('root', ?1, 'owner') ON CONFLICT(aggregate_id) DO UPDATE SET seq = excluded.seq",
            [sequence],
        ).unwrap();
    }
}

#[test]
fn snapshot_and_journal_exclude_synthetic_text_and_preserve_real_parts() {
    for (real, replacement) in [(true, false), (false, false), (true, true)] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        database(&path, real, replacement);
        let input = SessionInput {
            agent: "opencode".to_owned(),
            session_id: "root".to_owned(),
            source: RawSource::Sqlite(path.clone()),
            source_format: SourceFormat::OpenCodeSqliteV1,
            fork_parent_session_id: None,
        };
        let mut snapshot = Requests::default();
        reader_for("opencode").visit(&input, &mut snapshot).unwrap();
        let page = read_opencode_tool_result_page(&path, "root", None, &|| false).unwrap();
        let context = page.results.last().unwrap().context.as_ref().unwrap();
        let live =
            read_opencode_live_tool_result_page(&path, "root", None, None, &|| false).unwrap();
        let live_context = live.results.last().unwrap().context.as_ref().unwrap();
        assert!(context.main_session);
        assert!(live_context.main_session);
        let retained = real && !replacement;
        assert_eq!(snapshot.messages.len(), usize::from(retained));
        assert_eq!(context.user_messages.len(), usize::from(retained));
        assert_eq!(live_context.user_messages.len(), usize::from(retained));
        assert_eq!(context.complete_through_event, retained);
        assert!(!live_context.complete_through_event);
        assert_eq!(snapshot.gap, !retained);
        for messages in [
            &snapshot.messages,
            &context.user_messages,
            &live_context.user_messages,
        ] {
            for message in messages {
                assert_eq!(message.parts.len(), 1);
                assert_eq!(message.parts[0].text, "Review the API. Do not publish.");
                assert!(message.is_bounded());
            }
        }
        let resumed =
            read_opencode_tool_result_page(&path, "root", Some(&page.cursor), &|| false).unwrap();
        assert!(resumed.results.is_empty());
    }
}
