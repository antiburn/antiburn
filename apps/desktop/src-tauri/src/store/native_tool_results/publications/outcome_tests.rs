use super::*;
use crate::published_evidence_reader::StorePublishedEvidenceReader;
use antiburn_checks::domain::ObservationKind;
use antiburn_checks::source::{FileEventSource, PublicationCursor, SourceEvent};
use antiburn_local::analysis::opencode_v2_events::OpenCodeV2EventJoiner;
use antiburn_local::analysis::{NativeToolOutcome, ShellState};
use serde_json::{Value, json};

fn parsed_outcome() -> ToolResultObservation {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let events = [
        (
            "session.inbox.enqueued",
            json!({"inboxID":"msg_user","item":{"type":"user","payload":{"text":"Synthetic request"}}}),
        ),
        ("session.inbox.delivered", json!({"inboxID":"msg_user"})),
        (
            "session.step.started",
            json!({"assistantMessageID":"msg_assistant"}),
        ),
        (
            "session.tool.input.started",
            json!({"assistantMessageID":"msg_assistant","id":"call_shell","name":"shell"}),
        ),
        (
            "session.tool.called",
            json!({"assistantMessageID":"msg_assistant","id":"call_shell","input":{"command":"exit 7"},"executed":false}),
        ),
        (
            "session.tool.success",
            json!({"assistantMessageID":"msg_assistant","id":"call_shell","executed":false,"metadata":{"status":"completed","exit":7},"content":[{"type":"text","text":"Exited with code 7"}]}),
        ),
    ];
    let mut result = None;
    for (index, (kind, mut data)) in events.into_iter().enumerate() {
        data["sessionID"] = json!("session-1");
        let event = json!({"id":format!("evt_outcome_{index}"),"created":100+index,"type":kind,"data":data,
            "durable":{"aggregateID":"session-1","seq":index,"version":if kind=="session.tool.success"{2}else{1}}});
        result = joiner.observe(&event).or(result);
    }
    result.unwrap().into_observation(true, true)
}

#[test]
fn native_outcome_survives_current_outbox_and_published_reader_without_rewriting_status() {
    let store = store();
    let mut native = session();
    native.source_kind = "providerDb".into();
    native.source_label = "opencode:session-1:open_code_v2_sqlite".into();
    let (record, claim) = claim_and_record_for(&store, native);
    let result = parsed_outcome();
    assert_eq!(result.status, ToolResultStatus::Success);
    assert!(publish(
        &store,
        &record,
        &claim,
        &ToolResultBatch {
            source_format: SourceFormat::OpenCodeSqliteV2,
            results: vec![result.clone()],
            complete: false
        }
    ));

    for table in [
        "native_tool_result_current",
        "native_tool_result_publication",
    ] {
        let payload: String = store
            .lock()
            .query_row(
                &format!("SELECT payload_json FROM {table} WHERE payload_json IS NOT NULL"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        let payload: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["status"], "Success");
        assert_eq!(
            payload["outcome"],
            json!({"kind":"shell","state":"completed","exit":7,"signal":null,"timed_out":null,"shell_id":null})
        );
        assert_eq!(payload["provider_executed"], false);
    }
    let expected = Some(NativeToolOutcome::Shell {
        state: Some(ShellState::Completed),
        exit: Some(7),
        signal: None,
        timed_out: None,
        shell_id: None,
    });
    let current = store.native_tool_results_current(&record.key).unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].result.outcome, expected);
    assert_eq!(current[0].result.provider_executed, Some(false));
    let publications = store.native_tool_result_publications_after(0, 10).unwrap();
    let retained = publications
        .iter()
        .find_map(|publication| publication.payload.as_ref())
        .unwrap();
    assert_eq!(retained.outcome, expected);
    assert_eq!(retained.provider_executed, Some(false));

    let source = FileEventSource::new(StorePublishedEvidenceReader::new(&store), 20).unwrap();
    let page = source.read_batch(PublicationCursor(0)).unwrap();
    let observation = page
        .events
        .iter()
        .find_map(|event| match event {
            SourceEvent::Observation { observation, .. } => Some(observation),
            _ => None,
        })
        .unwrap();
    let ObservationKind::ToolResult {
        message, status, ..
    } = &observation.kind
    else {
        panic!("expected a tool result")
    };
    assert_eq!(status, "completed");
    assert_eq!(message.value["tool"]["result"]["status"], "completed");
    assert_eq!(message.value["tool"]["result"]["outcome"]["exit"], 7);
    assert_eq!(message.value["tool"]["provider_executed"], false);
}
