use antiburn_local::analysis::opencode_v2_events::{Gap, OpenCodeV2EventJoiner};
use antiburn_local::analysis::{
    NativeEvidenceFact, NativeMessageObservation, PublicMessagePayload, ResponseCycleStatus,
    ToolResultPosition, UserMessageChange,
};
use serde_json::{Value, json};

#[path = "opencode_public_events/merge.rs"]
mod merge;
#[path = "opencode_public_events/request_history.rs"]
mod request_history;
#[path = "opencode_public_events/review.rs"]
mod review;
#[path = "opencode_public_events/v2_tool_facts.rs"]
mod v2_tool_facts;

fn v2_fixture() -> Vec<Value> {
    include_str!("fixtures/opencode_public_events/v2.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn v2_facts(
    events: &[Value],
    baseline: Option<u64>,
) -> (Vec<NativeEvidenceFact>, OpenCodeV2EventJoiner) {
    let mut joiner = OpenCodeV2EventJoiner::new();
    if let Some(baseline) = baseline {
        joiner.set_live_baseline(baseline);
    }
    let mut facts = Vec::new();
    for event in events {
        facts.extend(
            joiner
                .observe_evidence(event)
                .events
                .into_iter()
                .map(|event| event.fact),
        );
    }
    (facts, joiner)
}

#[test]
fn v2_busy_period_preserves_delivery_full_calls_final_slots_and_result_order() {
    let events = v2_fixture();
    let (facts, joiner) = v2_facts(&events, Some(0));
    assert!(joiner.gaps().is_empty());
    let messages: Vec<_> = facts
        .iter()
        .filter_map(|fact| {
            if let NativeEvidenceFact::Message(message) = fact {
                Some(message)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(messages.len(), 5);
    assert!(matches!(messages[0], NativeMessageObservation::User(_)));
    assert!(
        matches!(messages[1], NativeMessageObservation::Public(public) if matches!(public.payload, PublicMessagePayload::ToolCall { .. }))
    );
    assert!(matches!(
        messages[2],
        NativeMessageObservation::ToolResult(_)
    ));
    let NativeMessageObservation::Public(assistant) = messages[3] else {
        panic!("assistant text");
    };
    let PublicMessagePayload::AssistantText { parts, generator } = &assistant.payload else {
        panic!("assistant text");
    };
    assert_eq!(
        parts.iter().map(|part| part.item_index).collect::<Vec<_>>(),
        [0, 2]
    );
    assert_eq!(generator.fast_mode, Some(true));
    assert_eq!(
        assistant.position,
        ToolResultPosition::OpenCodeV2Event {
            sequence: 14,
            item_index: 0
        }
    );
    let cycles: Vec<_> = facts
        .iter()
        .filter_map(|fact| {
            if let NativeEvidenceFact::ResponseCycle(cycle) = fact {
                Some(cycle)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(cycles.len(), 1);
    assert_eq!(
        cycles[0].messages,
        messages.into_iter().cloned().collect::<Vec<_>>()
    );
    assert_eq!(cycles[0].status, ResponseCycleStatus::Completed);
    assert_eq!(cycles[0].requester.fast_mode, Some(true));
    assert_eq!(cycles[0].context.user_messages.len(), 2);
    assert!(
        !serde_json::to_string(&facts)
            .unwrap()
            .contains("PRIVATE_SYNTHETIC_REASONING")
    );
}

#[test]
fn v2_seeded_start_missing_start_reconnect_overlap_and_shutdown_never_complete_a_cycle() {
    for case in [
        "seed",
        "missing",
        "no-baseline",
        "overlap",
        "shutdown",
        "superseded",
        "missing-input",
        "partial-text",
        "failed-text",
    ] {
        let mut events = v2_fixture();
        match case {
            "missing" => events.retain(|event| event["type"] != "session.execution.started"),
            "overlap" => {
                let mut start = events[1].clone();
                start["id"] = json!("overlap");
                start["durable"]["seq"] = json!(15);
                events.insert(14, start);
            }
            "shutdown" | "superseded" => {
                let end = events.last_mut().unwrap();
                end["type"] = json!("session.execution.interrupted");
                end["data"]["reason"] = json!(case);
            }
            "missing-input" => {
                events.remove(0);
            }
            "partial-text" => {
                events.retain(|event| event["id"] != "text-end-2");
            }
            "failed-text" => {
                let end = events
                    .iter_mut()
                    .find(|event| event["id"] == "step-end-1")
                    .unwrap();
                end["type"] = json!("session.step.failed");
                end["data"]["error"] = json!({"type":"provider","message":"Synthetic failure"});
            }
            _ => {}
        }
        let (facts, _) = v2_facts(
            &events,
            if case == "no-baseline" {
                None
            } else if case == "seed" {
                Some(2)
            } else {
                Some(0)
            },
        );
        assert!(
            !facts
                .iter()
                .any(|fact| matches!(fact, NativeEvidenceFact::ResponseCycle(_))),
            "{case}"
        );
    }
    let mut joiner = OpenCodeV2EventJoiner::new();
    joiner.set_live_baseline(0);
    let events = v2_fixture();
    for event in &events[..14] {
        joiner.observe_evidence(event);
    }
    joiner.mark_gap();
    for event in &events[14..] {
        assert!(
            !joiner
                .observe_evidence(event)
                .events
                .iter()
                .any(|event| matches!(event.fact, NativeEvidenceFact::ResponseCycle(_)))
        );
    }
    assert!(joiner.gaps().contains(&Gap::ContinuityLost));
}

#[test]
fn v2_terminal_status_and_no_text_steps_have_independent_attribution() {
    for (kind, reason, status) in [
        ("session.execution.failed", None, ResponseCycleStatus::Error),
        (
            "session.execution.interrupted",
            Some("user"),
            ResponseCycleStatus::Cancelled,
        ),
        (
            "session.execution.interrupted",
            Some("inactivity"),
            ResponseCycleStatus::Cancelled,
        ),
    ] {
        let mut events = v2_fixture();
        let second = events
            .iter_mut()
            .find(|event| event["id"] == "step-2")
            .unwrap();
        second["data"]["model"]["id"] = json!("unreviewed-model");
        let end = events.last_mut().unwrap();
        end["type"] = json!(kind);
        if let Some(reason) = reason {
            end["data"]["reason"] = json!(reason);
        } else {
            end["data"]["error"] = json!({"type":"provider.failure","message":"Synthetic failure"});
        }
        let (facts, _) = v2_facts(&events, Some(0));
        let cycle = facts
            .iter()
            .find_map(|fact| {
                if let NativeEvidenceFact::ResponseCycle(cycle) = fact {
                    Some(cycle)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(cycle.status, status);
        assert_eq!(cycle.requester.model, None);
        assert_eq!(cycle.requester.fast_mode, None);
    }
}

#[test]
fn v2_selection_is_explicit_and_does_not_count_as_native_activity() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    joiner.set_live_baseline(0);
    for (seq, kind) in [
        (1, "session.model.selected"),
        (2, "session.renamed"),
        (3, "session.viewed"),
        (4, "session.metadata.updated"),
    ] {
        let event = json!({"id":format!("event-{seq}"),"type":kind,"created":1000,"durable":{"aggregateID":"root","seq":seq,"version":1},"data":{"sessionID":"root","model":{"providerID":"openai","id":"gpt-6.1-sol-ultrafast"},"title":"Synthetic title"}});
        let batch = joiner.observe_evidence(&event);
        assert!(
            !batch
                .events
                .iter()
                .any(|event| matches!(event.fact, NativeEvidenceFact::Activity(_)))
        );
        if seq == 1 {
            assert!(
                matches!(&batch.events[0].fact, NativeEvidenceFact::SessionSelection(selection) if selection.selection.fast_mode == Some(true))
            );
        } else {
            assert!(batch.events.is_empty());
        }
    }
}

#[test]
fn v2_mode_uses_only_the_ten_reviewed_catalog_ids() {
    let ids = [
        "gpt-5.5-fast",
        "gpt-5.6-luna-fast",
        "gpt-5.6-sol-fast",
        "gpt-5.6-terra-fast",
        "gpt-6-astra-fast",
        "gpt-6-astra-ultrafast",
        "gpt-6-luna-fast",
        "gpt-6-sol-fast",
        "gpt-6.1-sol-fast",
        "gpt-6.1-sol-ultrafast",
        "invented-fast",
        "gpt-6.1-sol",
        "GPT-6.1-SOL-FAST",
    ];
    for (index, id) in ids.iter().enumerate() {
        let mut joiner = OpenCodeV2EventJoiner::new();
        let event = json!({"id":"selection","type":"session.model.selected","created":1000,"durable":{"aggregateID":"root","seq":1,"version":1},"data":{"sessionID":"root","model":{"providerID":"openai","id":id}}});
        let batch = joiner.observe_evidence(&event);
        let NativeEvidenceFact::SessionSelection(selection) = &batch.events[0].fact else {
            panic!("explicit selection");
        };
        assert_eq!(selection.selection.fast_mode, (index < 10).then_some(true));
    }
}

#[test]
fn v2_retry_resets_text_finality_and_keeps_one_assistant_identity() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    joiner.set_live_baseline(0);
    let events = v2_fixture();
    for event in &events[..14] {
        joiner.observe_evidence(event);
    }
    let mut retry = events[3].clone();
    retry["id"] = json!("retry");
    retry["durable"]["seq"] = json!(15);
    joiner.observe_evidence(&retry);
    let mut start = events[4].clone();
    start["id"] = json!("retry-text");
    start["durable"]["seq"] = json!(16);
    joiner.observe_evidence(&start);
    let mut end = events[13].clone();
    end["id"] = json!("retry-end");
    end["durable"]["seq"] = json!(17);
    let batch = joiner.observe_evidence(&end);
    assert!(batch.events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::Public(public)) if public.record_id == "assistant-1" && !public.eligible)));
}

#[test]
fn v2_queued_and_steering_input_share_one_period_but_cancelled_input_stays_out() {
    let mut events = v2_fixture();
    let mut parked = events[14].clone();
    parked["id"] = json!("parked");
    parked["durable"]["seq"] = json!(21);
    parked["data"]["inboxID"] = json!("parked-input");
    parked["data"]["item"]["payload"]["text"] = json!("PARKED_INPUT");
    let mut cancel = parked.clone();
    cancel["id"] = json!("cancel");
    cancel["durable"]["seq"] = json!(22);
    cancel["type"] = json!("session.inbox.cancelled");
    let mut change = events[14].clone();
    change["id"] = json!("delivery-change");
    change["type"] = json!("session.inbox.delivery.changed");
    change["data"]["delivery"] = json!("steer");
    for event in &mut events[15..] {
        let seq = event["durable"]["seq"].as_u64().unwrap();
        event["durable"]["seq"] = json!(seq + 1);
    }
    change["durable"]["seq"] = json!(17);
    events.insert(15, change);
    let mut end = events.pop().unwrap();
    end["durable"]["seq"] = json!(23);
    events.extend([parked, cancel, end]);
    let (facts, joiner) = v2_facts(&events, Some(0));
    assert!(joiner.gaps().is_empty());
    let cycles: Vec<_> = facts
        .iter()
        .filter_map(|fact| {
            if let NativeEvidenceFact::ResponseCycle(cycle) = fact {
                Some(cycle)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(cycles.len(), 1);
    assert_eq!(cycles[0].context.user_messages.len(), 2);
    assert!(
        !serde_json::to_string(cycles[0])
            .unwrap()
            .contains("PARKED_INPUT")
    );
}

#[test]
fn v2_schema_conflicts_and_text_capacity_withhold_cycles_without_private_payloads() {
    for case in ["version", "duplicate", "text-capacity"] {
        let mut events = v2_fixture();
        match case {
            "version" => events[5]["durable"]["version"] = json!(9),
            "duplicate" => {
                let mut duplicate = events[5].clone();
                duplicate["data"]["text"] = json!("Changed text");
                events.insert(6, duplicate);
            }
            "text-capacity" => events[5]["data"]["text"] = json!("x".repeat(16 * 1024 + 1)),
            _ => unreachable!(),
        }
        let (facts, _) = v2_facts(&events, Some(0));
        assert!(
            !facts
                .iter()
                .any(|fact| matches!(fact, NativeEvidenceFact::ResponseCycle(_))),
            "{case}"
        );
    }
}

#[test]
fn v2_live_evidence_crosses_every_capture_frontier_once() {
    use antiburn_local::analysis::opencode_v2_capture::OpenCodeV2CaptureEpoch;
    use std::collections::HashMap;
    let events = v2_fixture();
    for baseline in 0..=20 {
        for buffered in [0, 5, events.len()] {
            let mut capture = OpenCodeV2CaptureEpoch::new(vec!["root".into()]).unwrap();
            capture.confirm_subscription().unwrap();
            for event in &events[..buffered] {
                capture.observe(event.clone()).unwrap();
            }
            let mut captured = capture
                .finish_hydration(HashMap::from([("root".into(), Some(baseline))]))
                .unwrap();
            for event in &events[buffered..] {
                captured.extend(capture.observe(event.clone()).unwrap());
            }
            let mut joiner = OpenCodeV2EventJoiner::new();
            joiner.set_live_baseline(baseline);
            let mut results = 0;
            let mut cycles = 0;
            for event in captured {
                for evidence in joiner
                    .observe_captured_evidence(&event)
                    .events
                    .into_iter()
                    .filter(|event| event.live_eligible)
                {
                    match evidence.fact {
                        NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(_)) => {
                            results += 1
                        }
                        NativeEvidenceFact::ResponseCycle(_) => cycles += 1,
                        _ => {}
                    }
                }
            }
            assert_eq!(
                results,
                usize::from(baseline < 13),
                "baseline={baseline}, buffered={buffered}"
            );
            assert_eq!(
                cycles,
                usize::from(baseline < 2),
                "baseline={baseline}, buffered={buffered}"
            );
        }
    }
}

fn journal() -> (tempfile::TempDir, std::path::PathBuf, rusqlite::Connection) {
    let directory = tempfile::TempDir::new().unwrap();
    let path = directory.path().join("journal.db");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT); INSERT INTO session VALUES ('root',NULL);
        CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, data TEXT);
        CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, data TEXT);
        CREATE TABLE event_sequence (aggregate_id TEXT PRIMARY KEY, seq INTEGER NOT NULL, owner_id TEXT);
        CREATE TABLE event (id TEXT PRIMARY KEY, aggregate_id TEXT NOT NULL, seq INTEGER NOT NULL, type TEXT NOT NULL, data TEXT NOT NULL);
        CREATE UNIQUE INDEX event_aggregate_seq_idx ON event(aggregate_id,seq);").unwrap();
    append(
        &connection,
        0,
        "session.created.1",
        json!({"sessionID":"root","info":{"id":"root"}}),
    );
    (directory, path, connection)
}

fn append(connection: &rusqlite::Connection, sequence: u64, kind: &str, data: Value) {
    connection
        .execute(
            "INSERT INTO event VALUES (?1,'root',?2,?3,?4)",
            rusqlite::params![
                format!("event-{sequence}"),
                sequence,
                kind,
                data.to_string()
            ],
        )
        .unwrap();
    connection.execute("INSERT INTO event_sequence VALUES ('root',?1,NULL) ON CONFLICT(aggregate_id) DO UPDATE SET seq=excluded.seq", [sequence]).unwrap();
}

fn v1_messages(connection: &rusqlite::Connection) {
    v1_messages_with_tool_status(connection, false);
}

fn v1_messages_with_tool_status(connection: &rusqlite::Connection, completed_tool: bool) {
    append(
        connection,
        1,
        "message.updated.1",
        json!({"sessionID":"root","info":{"id":"user","sessionID":"root","role":"user","time":{"created":1000}}}),
    );
    append(
        connection,
        2,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{"id":"user-part","sessionID":"root","messageID":"user","type":"text","text":"Inspect the synthetic file"}}),
    );
    append(
        connection,
        3,
        "message.updated.1",
        json!({"sessionID":"root","info":{"id":"assistant","sessionID":"root","parentID":"user","role":"assistant","providerID":"openai","modelID":"synthetic-model","path":{"cwd":"/synthetic/project"},"time":{"created":1001}}}),
    );
    append(
        connection,
        4,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{"id":"step","sessionID":"root","messageID":"assistant","type":"step-start"}}),
    );
    append(
        connection,
        5,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{"id":"tool","sessionID":"root","messageID":"assistant","type":"tool","callID":"call","tool":"read","state":{"status":"running","input":{"path":"synthetic.rs"},"time":{"start":1002}}}}),
    );
    if completed_tool {
        append(
            connection,
            6,
            "message.part.updated.1",
            json!({"sessionID":"root","part":{"id":"tool","sessionID":"root","messageID":"assistant","type":"tool","callID":"call","tool":"read",
                "state":{"status":"completed","input":{"path":"synthetic.rs"},"output":"Initial contents","title":"Read synthetic.rs","metadata":{},"time":{"start":1002,"end":1003}}}}),
        );
    }
    let terminal_events = u64::from(completed_tool);
    append(
        connection,
        6 + terminal_events,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{"id":"text","sessionID":"root","messageID":"assistant","type":"text","text":"The synthetic file is ready.","time":{"start":1003,"end":1004}}}),
    );
    append(
        connection,
        7 + terminal_events,
        "message.updated.1",
        json!({"sessionID":"root","info":{"id":"assistant","sessionID":"root","parentID":"user","role":"assistant","providerID":"openai","modelID":"synthetic-model","path":{"cwd":"/synthetic/project"},"time":{"created":1001,"completed":1005}}}),
    );
}

#[test]
fn v1_live_users_calls_and_text_use_separate_boundaries_and_removal_tombstones() {
    use antiburn_local::analysis::read_opencode_live_tool_result_page as read;
    let (_directory, path, connection) = journal();
    let seed = read(&path, "root", None, None, &|| false).unwrap();
    v1_messages(&connection);
    let page = read(&path, "root", Some(&seed.cursor), None, &|| false).unwrap();
    let messages: Vec<_> = page
        .evidence
        .events
        .iter()
        .filter_map(|event| {
            if let NativeEvidenceFact::Message(message) = &event.fact {
                Some(message)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        messages
            .iter()
            .filter(|message| matches!(message, NativeMessageObservation::User(_)))
            .count(),
        1
    );
    assert_eq!(
        messages
            .iter()
            .filter(|message| matches!(message, NativeMessageObservation::Public(_)))
            .count(),
        2
    );
    assert!(messages.iter().any(|message| matches!(message, NativeMessageObservation::Public(public) if matches!(&public.payload, PublicMessagePayload::ToolCall { facts: Some(facts), .. } if facts.requester.model.as_deref() == Some("synthetic-model") && facts.operation_paths.is_none()))));
    append(
        &connection,
        8,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{"id":"user-part","sessionID":"root","messageID":"user","type":"text","text":"generated","synthetic":true}}),
    );
    append(
        &connection,
        9,
        "message.removed.1",
        json!({"sessionID":"root","messageID":"assistant"}),
    );
    let removed = read(&path, "root", Some(&page.cursor), None, &|| false).unwrap();
    assert!(removed.evidence.events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::User(user)) if user.message.change == UserMessageChange::Removal)));
    assert!(removed.evidence.events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::MessageRemoval(removal) if removal.message_id == "assistant")));
    append(
        &connection,
        10,
        "message.updated.1",
        json!({"sessionID":"root","info":{"id":"assistant","sessionID":"root","role":"assistant"}}),
    );
    let later = read(&path, "root", Some(&removed.cursor), None, &|| false).unwrap();
    assert!(!later.evidence.events.iter().any(|event| matches!(
        event.fact,
        NativeEvidenceFact::Message(NativeMessageObservation::Public(_))
    )));
}

#[test]
fn v1_v4_conversion_keeps_progress_results_and_freezes_a_separate_public_frontier() {
    use antiburn_local::analysis::{
        OpenCodeJournalCursor, read_opencode_live_tool_result_page as read,
    };
    let (_directory, path, connection) = journal();
    v1_messages(&connection);
    let seed = read(&path, "root", None, None, &|| false).unwrap();
    let mut legacy = serde_json::to_value(&seed.cursor).unwrap();
    legacy["parser_revision"] = json!(62);
    legacy["live"]["revision"] = json!(4);
    legacy["live"].as_object_mut().unwrap().remove("public");
    legacy["live"]
        .as_object_mut()
        .unwrap()
        .remove("public_baseline");
    let old: OpenCodeJournalCursor = serde_json::from_value(legacy.clone()).unwrap();
    let mut unsupported = legacy.clone();
    unsupported["parser_revision"] = json!(999);
    let unsupported: OpenCodeJournalCursor = serde_json::from_value(unsupported).unwrap();
    assert!(read(&path, "root", Some(&unsupported), None, &|| false).is_err());
    append(
        &connection,
        8,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{"id":"user-part-2","sessionID":"root","messageID":"user","type":"text","text":"More input"}}),
    );
    let converted = read(&path, "root", Some(&old), None, &|| false).unwrap();
    let cursor = serde_json::to_value(&converted.cursor).unwrap();
    assert_eq!(cursor["through_sequence"], 8);
    assert_eq!(cursor["live"]["baseline"], legacy["live"]["baseline"]);
    assert_eq!(cursor["live"]["public_baseline"], 8);
    assert_eq!(cursor["live"]["revision"], 5);
    let mut missing_baseline = cursor.clone();
    missing_baseline["live"]
        .as_object_mut()
        .unwrap()
        .remove("public_baseline");
    let missing_baseline: OpenCodeJournalCursor = serde_json::from_value(missing_baseline).unwrap();
    assert!(read(&path, "root", Some(&missing_baseline), None, &|| false).is_err());
    assert!(
        converted
            .evidence
            .events
            .iter()
            .all(|event| !event.live_eligible)
    );
    let idle = read(&path, "root", Some(&converted.cursor), None, &|| false).unwrap();
    assert!(idle.evidence.events.is_empty());
    append(
        &connection,
        9,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{"id":"tool","sessionID":"root","messageID":"assistant","type":"tool","callID":"call","tool":"read","state":{"status":"completed","input":{"path":"synthetic.rs"},"output":"contents"}}}),
    );
    append(
        &connection,
        10,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{"id":"user-part-3","sessionID":"root","messageID":"user","type":"text","text":"A later correction"}}),
    );
    let later = read(&path, "root", Some(&idle.cursor), None, &|| false).unwrap();
    assert_eq!(later.results.len(), 1);
    assert_eq!(
        later.results[0].admission(),
        antiburn_local::analysis::ToolResultAdmission::Admitted
    );
    assert!(later.evidence.events.iter().any(|event| event.live_eligible
        && matches!(
            event.fact,
            NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(_))
        )));
    assert!(
        later
            .evidence
            .events
            .iter()
            .filter(|event| matches!(
                event.fact,
                NativeEvidenceFact::Message(NativeMessageObservation::User(_))
            ))
            .all(|event| !event.live_eligible)
    );
    let drained = read(&path, "root", Some(&later.cursor), None, &|| false).unwrap();
    assert!(drained.results.is_empty());
}

#[test]
fn v1_cold_completions_and_terminal_only_calls_do_not_become_new_public_boundaries() {
    use antiburn_local::analysis::read_opencode_live_tool_result_page as read;
    let (_directory, path, connection) = journal();
    v1_messages(&connection);
    let seed = read(&path, "root", None, None, &|| false).unwrap();
    assert!(!seed.evidence.events.iter().any(|event| matches!(
        event.fact,
        NativeEvidenceFact::Message(NativeMessageObservation::Public(_))
    )));
    append(
        &connection,
        8,
        "message.updated.1",
        json!({"sessionID":"root","info":{"id":"unknown-assistant","sessionID":"root","role":"assistant","time":{"created":1000,"completed":1001}}}),
    );
    append(
        &connection,
        9,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{"id":"unknown-text","sessionID":"root","messageID":"unknown-assistant","type":"text","text":"Historical correction","time":{"start":1000,"end":1001}}}),
    );
    append(
        &connection,
        10,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{"id":"unknown-tool","sessionID":"root","messageID":"unknown-assistant","type":"tool","tool":"read","callID":"unknown-call","state":{"status":"completed","input":{"path":"synthetic.rs"},"output":"contents"}}}),
    );
    let page = read(&path, "root", Some(&seed.cursor), None, &|| false).unwrap();
    assert!(!page.evidence.events.iter().any(|event| matches!(
        event.fact,
        NativeEvidenceFact::Message(NativeMessageObservation::Public(_))
    )));
    let result = page
        .results
        .iter()
        .find(|result| result.result_id.as_deref() == Some("unknown-tool"))
        .unwrap();
    assert_eq!(
        result.completeness,
        antiburn_local::analysis::ToolResultCompleteness::UnsafeIdentity
    );
}

#[test]
fn v1_public_output_pages_resume_without_a_false_history_gap() {
    use antiburn_local::analysis::read_opencode_live_tool_result_page as read;
    let (_directory, path, connection) = journal();
    let seed = read(&path, "root", None, None, &|| false).unwrap();
    append(
        &connection,
        1,
        "message.updated.1",
        json!({"sessionID":"root","info":{"id":"user","sessionID":"root","role":"user"}}),
    );
    for seq in 2..72 {
        append(
            &connection,
            seq,
            "message.part.updated.1",
            json!({"sessionID":"root","part":{"id":"user-text","messageID":"user","sessionID":"root","type":"text","text":format!("Synthetic request {seq}")}}),
        );
    }
    let mut cursor = seed.cursor;
    let mut sequences = Vec::new();
    loop {
        let page = read(&path, "root", Some(&cursor), None, &|| false).unwrap();
        assert!(page.evidence.events.len() <= 40);
        assert!(page.history_complete);
        for event in &page.evidence.events {
            if let NativeEvidenceFact::Message(NativeMessageObservation::User(user)) = &event.fact {
                sequences.push(user.revision);
            }
        }
        cursor = page.cursor;
        if !page.has_more {
            break;
        }
    }
    assert_eq!(sequences, (3..73).collect::<Vec<_>>());
}

#[test]
fn v1_header_usage_refresh_does_not_reset_source_activity() {
    use antiburn_local::analysis::read_opencode_live_tool_result_page as read;
    let (_directory, path, connection) = journal();
    v1_messages(&connection);
    let seed = read(&path, "root", None, None, &|| false).unwrap();
    append(
        &connection,
        8,
        "message.updated.1",
        json!({"sessionID":"root","info":{"id":"assistant","sessionID":"root","parentID":"user","role":"assistant","providerID":"openai","modelID":"synthetic-model","path":{"cwd":"/synthetic/project"},"time":{"created":1001,"completed":1005},"tokens":{"input":400,"output":20},"cost":0.01}}),
    );
    let page = read(&path, "root", Some(&seed.cursor), None, &|| false).unwrap();
    assert!(page.evidence.events.is_empty());
}
