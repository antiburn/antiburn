use antiburn_local::analysis::file_live::{NativeFileCursor, read_native_file_page};
use antiburn_local::analysis::{
    CheckToolCategory, CompositeSink, EvidenceSource, NativeEvidenceEvent, NativeEvidenceFact,
    NativeMessageObservation, PublicMessagePayload, RawSource, SessionEvidenceAccumulator,
    SessionInput, SessionMetricsAccumulator, SourceCapabilities, SourceFormat, SourceKind,
    reader_for,
};
use serde_json::{Value, json};
use std::io::Write;
use tempfile::TempDir;

#[path = "support/jsonl_fixture.rs"]
mod jsonl_fixture;

const CODEX: &str = include_str!("fixtures/public_events/codex.jsonl");
const PI: &str = include_str!("fixtures/public_events/pi.jsonl");

fn input(agent: &str, source: RawSource) -> SessionInput {
    SessionInput {
        agent: agent.into(),
        session_id: "facts".into(),
        source_format: if agent == "codex" {
            SourceFormat::CodexRolloutJsonl
        } else {
            SourceFormat::PiV3Jsonl
        },
        source,
        fork_parent_session_id: None,
    }
}

fn rows(source: &str) -> Vec<Value> {
    source
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn encode(records: &[Value]) -> String {
    records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let mut record = record.clone();
            if record.get("ordinal").is_some() {
                record["ordinal"] = json!(index);
            }
            format!("{record}\n")
        })
        .collect()
}

fn full_events(agent: &str, source: &str) -> Vec<NativeEvidenceEvent> {
    let mut sink = CompositeSink::new(
        SessionMetricsAccumulator::new(agent, "facts"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: agent.into(),
            session_id: "facts".into(),
            kind: SourceKind::Jsonl,
            capabilities: if agent == "codex" {
                SourceCapabilities::codex()
            } else {
                SourceCapabilities::pi()
            },
        }),
    );
    reader_for(agent)
        .visit(&input(agent, RawSource::Jsonl(source.into())), &mut sink)
        .unwrap();
    sink.native_evidence().to_vec()
}

fn file_events(agent: &str, source: &str) -> Vec<NativeEvidenceEvent> {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("session.jsonl");
    std::fs::write(&path, source).unwrap();
    let page =
        read_native_file_page(&input(agent, RawSource::File(path)), None, &|| false).unwrap();
    assert!(!page.has_more);
    page.evidence.events
}

fn assert_live_splits(agent: &str, source: &str) {
    for line_ending in ["\n", "\r\n"] {
        let source = jsonl_fixture::with_line_ending(source, line_ending);
        let expected: Vec<_> = file_events(agent, &source)
            .into_iter()
            .map(|event| event.fact)
            .collect();
        let lines: Vec<_> = source.split_inclusive('\n').collect();
        for boundary in 1..lines.len() {
            let directory = TempDir::new().unwrap();
            let path = directory.path().join("session.jsonl");
            std::fs::write(&path, lines[..boundary].concat()).unwrap();
            let input = input(agent, RawSource::File(path.clone()));
            let first = read_native_file_page(&input, None, &|| false).unwrap();
            let cursor: NativeFileCursor =
                serde_json::from_slice(&serde_json::to_vec(&first.cursor).unwrap()).unwrap();
            let cursor = NativeFileCursor::decode(&cursor.encode()).unwrap();
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            file.write_all(lines[boundary..].concat().as_bytes())
                .unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), source.as_bytes());
            let second = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
            assert!(
                second
                    .evidence
                    .events
                    .iter()
                    .filter(|event| matches!(
                        &event.fact,
                        NativeEvidenceFact::Message(NativeMessageObservation::Public(_))
                    ))
                    .all(|event| event.live_eligible)
            );
            let actual: Vec<_> = first
                .evidence
                .events
                .into_iter()
                .chain(second.evidence.events)
                .map(|event| event.fact)
                .collect();
            assert_eq!(
                actual, expected,
                "{agent}, boundary {boundary}, line ending: {line_ending:?}"
            );
        }
    }
}

fn assert_no_live_admission(events: &[NativeEvidenceEvent]) {
    for event in events {
        match &event.fact {
            NativeEvidenceFact::AdmissionFailure(_) => {}
            NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(result)) => {
                assert!(!event.live_eligible, "{event:?}");
                let context = result.context.as_ref().unwrap();
                assert!(!context.main_session, "{event:?}");
                assert!(!context.complete_through_event, "{event:?}");
                assert_ne!(
                    result.completeness,
                    antiburn_local::analysis::ToolResultCompleteness::Complete
                );
            }
            _ => panic!("unadmitted session emits public evidence: {event:?}"),
        }
    }
}

fn assert_pi_denial_splits(source: &str, invalid_index: usize, case: &str) {
    for line_ending in ["\n", "\r\n"] {
        let source = jsonl_fixture::with_line_ending(source, line_ending);
        let lines: Vec<_> = source.split_inclusive('\n').collect();
        let invalid_offset = lines[..invalid_index]
            .iter()
            .map(|line| line.len() as u64)
            .sum::<u64>();
        for boundary in 1..lines.len() {
            let directory = TempDir::new().unwrap();
            let path = directory.path().join("session.jsonl");
            std::fs::write(&path, lines[..boundary].concat()).unwrap();
            let input = input("pi", RawSource::File(path.clone()));
            let first = read_native_file_page(&input, None, &|| false).unwrap();
            let cursor: NativeFileCursor =
                serde_json::from_slice(&serde_json::to_vec(&first.cursor).unwrap()).unwrap();
            let cursor = NativeFileCursor::decode(&cursor.encode()).unwrap();
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            file.write_all(lines[boundary..].concat().as_bytes())
                .unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), source.as_bytes());
            let second = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
            assert!(!second.history_complete, "{case}, boundary {boundary}");
            let failure = second
                .admission_failure
                .as_ref()
                .expect("unsafe source retains its admission failure");
            match case {
                "parent" => assert_eq!(
                    failure.reason,
                    antiburn_local::analysis::ToolResultCompleteness::UnsafeOrder
                ),
                "duplicate-id" => assert_eq!(
                    failure.reason,
                    antiburn_local::analysis::ToolResultCompleteness::UnsafeIdentity
                ),
                _ => {}
            }
            let later: Vec<_> = first
                .evidence
                .events
                .into_iter()
                .chain(second.evidence.events)
                .filter(|event| {
                    matches!(event.fact.position(), antiburn_local::analysis::ToolResultPosition::FileBytes { record_start, .. } if record_start >= invalid_offset)
                })
                .collect();
            assert_no_live_admission(&later);
            let cursor: NativeFileCursor =
                serde_json::from_slice(&serde_json::to_vec(&second.cursor).unwrap()).unwrap();
            let cursor = NativeFileCursor::decode(&cursor.encode()).unwrap();
            let mut later_assistant = rows(PI).pop().unwrap();
            later_assistant["id"] = json!("assistant-after-unsafe");
            later_assistant["parentId"] = json!("assistant-2");
            write!(file, "{later_assistant}{line_ending}").unwrap();
            let third = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
            assert_eq!(
                third.admission_failure.as_ref(),
                Some(failure),
                "{case}, boundary {boundary}, line ending: {line_ending:?}"
            );
            assert!(!third.history_complete);
            assert_no_live_admission(&third.evidence.events);
        }
    }
}

fn cycle_count(events: &[NativeEvidenceEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(&event.fact, NativeEvidenceFact::ResponseCycle(_)))
        .count()
}

fn codex_exec_records() -> Vec<Value> {
    let mut records = rows(CODEX);
    let completed = rows(include_str!(
        "fixtures/codex_characterization/paginated_completed.jsonl"
    ));
    let operations = [5, 7, 8, 9].map(|index| {
        let mut record = completed[index].clone();
        if record["type"] == "event_msg" {
            record["payload"]["thread_id"] = json!("facts");
        }
        record
    });
    records.splice(6..8, operations);
    records
}

#[test]
fn codex_replaced_exec_output_closes_only_its_outer_call_without_duplicate_results() {
    for single_operation in [true, false] {
        let mut records = codex_exec_records();
        let expected = if single_operation {
            records[6]["payload"]["input"] =
                json!("text(await tools.exec_command({cmd:\"python3 -m unittest -v\"}));");
            records[9]["payload"]["output"] = json!("Script completed.");
            records.remove(8);
            vec!["test-1"]
        } else {
            vec!["test-1", "read-1"]
        };
        let source = encode(&records);
        for events in [full_events("codex", &source), file_events("codex", &source)] {
            assert_eq!(cycle_count(&events), 1);
            let mut call_ids = Vec::new();
            let mut result_ids = Vec::new();
            for event in &events {
                match &event.fact {
                    NativeEvidenceFact::Message(NativeMessageObservation::Public(value))
                        if matches!(value.payload, PublicMessagePayload::ToolCall { .. }) =>
                    {
                        call_ids.push(value.call_id.as_deref().unwrap());
                    }
                    NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) => {
                        result_ids.push(value.call_id.as_deref().unwrap());
                    }
                    NativeEvidenceFact::ResponseCycle(cycle) => {
                        let ids: Vec<_> = cycle
                            .messages
                            .iter()
                            .filter_map(|message| match message {
                                NativeMessageObservation::ToolResult(result) => {
                                    result.call_id.as_deref()
                                }
                                _ => None,
                            })
                            .collect();
                        assert_eq!(ids, expected);
                    }
                    _ => {}
                }
            }
            assert_eq!(call_ids, ["outer-call-1"]);
            assert_eq!(result_ids, expected);
        }
        assert_live_splits("codex", &source);
    }
}

#[test]
fn codex_replaced_exec_needs_its_exact_complete_wrapper_output_to_close() {
    for case in [
        "missing",
        "wrong-id",
        "wrong-turn",
        "incomplete",
        "pending-other-call",
    ] {
        let mut records = codex_exec_records();
        match case {
            "missing" => {
                records.remove(9);
            }
            "wrong-id" => records[9]["payload"]["call_id"] = json!("another-call"),
            "wrong-turn" => records[9]["payload"]["turn_id"] = json!("another-turn"),
            "incomplete" => records[9]["payload"]["output"] = json!([{"type":"image"}]),
            "pending-other-call" => {
                let mut call = rows(CODEX)[6].clone();
                call["payload"]["call_id"] = json!("pending-call");
                records.insert(7, call);
            }
            _ => unreachable!(),
        }
        let source = encode(&records);
        for events in [full_events("codex", &source), file_events("codex", &source)] {
            assert_eq!(cycle_count(&events), 0, "{case}");
        }
        assert_live_splits("codex", &source);
    }
}

#[test]
fn codex_replaced_exec_outputs_require_valid_timestamps_in_full_and_live_reads() {
    for case in ["missing", "invalid"] {
        let mut records = codex_exec_records();
        if case == "missing" {
            records[9].as_object_mut().unwrap().remove("timestamp");
        } else {
            records[9]["timestamp"] = json!("not-a-timestamp");
        }
        let source = encode(&records);
        for (reader, events) in [
            ("full", full_events("codex", &source)),
            ("live", file_events("codex", &source)),
        ] {
            assert_eq!(cycle_count(&events), 0, "{reader}, {case}");
        }
        assert_live_splits("codex", &source);
    }
}

#[test]
fn codex_replaced_exec_output_call_id_alias_matches_full_and_live_reads() {
    let mut records = codex_exec_records();
    let payload = records[9]["payload"].as_object_mut().unwrap();
    let call = payload.remove("call_id").unwrap();
    payload.insert("callId".into(), call);
    let source = encode(&records);
    for (reader, events) in [
        ("full", full_events("codex", &source)),
        ("live", file_events("codex", &source)),
    ] {
        assert_eq!(cycle_count(&events), 1, "{reader}");
        let results: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.fact {
                NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(result)) => {
                    result.call_id.as_deref()
                }
                _ => None,
            })
            .collect();
        assert_eq!(results, ["test-1", "read-1"], "{reader}");
    }
    assert_live_splits("codex", &source);
}

#[test]
fn codex_replacement_filter_collisions_cannot_close_an_ordinary_call() {
    let mut records = rows(CODEX);
    let mut ordinary = records[6].clone();
    ordinary["payload"]["call_id"] = json!("ordinary-call");
    let mut operations = vec![ordinary];
    for id in [
        "exec-proof-55240",
        "exec-proof-87869",
        "exec-proof-104981",
        "exec-proof-131544",
        "exec-proof-132961",
        "exec-proof-259736",
        "exec-proof-267599",
        "exec-proof-327225",
    ] {
        let mut call = records[6].clone();
        call["payload"]["call_id"] = json!(id);
        call["payload"]["name"] = json!("exec");
        call["payload"]["input"] = json!("text(1);");
        let mut output = records[7].clone();
        output["payload"]["call_id"] = json!(id);
        operations.extend([call, output]);
    }
    let mut output = records[7].clone();
    output["payload"]["call_id"] = json!("ordinary-call");
    operations.push(output);
    records.splice(6..8, operations);
    let source = encode(&records);
    let full = full_events("codex", &source);
    assert_eq!(cycle_count(&full), 1);
    assert!(full.iter().any(|event| matches!(
        &event.fact,
        NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(result))
            if result.call_id.as_deref() == Some("ordinary-call")
    )));
    let live = file_events("codex", &source);
    assert_eq!(cycle_count(&live), 0);
    assert_live_splits("codex", &source);
}

#[test]
fn codex_recognized_inert_metadata_keeps_later_full_and_live_history_complete() {
    let mut records = rows(CODEX);
    records.insert(
        4,
        json!({"timestamp":"2026-10-07T10:00:03Z","ordinal":0,"type":"inter_agent_communication_metadata","payload":{"trigger_turn":true}}),
    );
    let source = encode(&records);
    for events in [full_events("codex", &source), file_events("codex", &source)] {
        assert_eq!(cycle_count(&events), 0);
        let mut messages = 0;
        for event in events {
            let complete = match event.fact {
                NativeEvidenceFact::Message(NativeMessageObservation::User(value)) => {
                    value.context.complete_through_event
                }
                NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) => {
                    value.context.unwrap().complete_through_event
                }
                NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) => {
                    value.context.unwrap().complete_through_event
                }
                _ => continue,
            };
            messages += 1;
            assert!(complete);
        }
        assert_eq!(messages, 4);
    }
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("session.jsonl");
    std::fs::write(&path, &source).unwrap();
    let page =
        read_native_file_page(&input("codex", RawSource::File(path)), None, &|| false).unwrap();
    assert!(page.history_complete);
    assert_live_splits("codex", &source);
}

#[test]
fn codex_recognized_records_with_unhandled_evidence_still_mark_history_partial() {
    for kind in ["inter_agent_communication_metadata", "world_state"] {
        let mut records = rows(CODEX);
        records.insert(
            4,
            json!({"timestamp":"2026-10-07T10:00:03Z","ordinal":0,"type":kind,"payload":{"trigger_turn":true,"usage":{"output_tokens":1}}}),
        );
        let source = encode(&records);
        for (reader, events) in [
            ("full", full_events("codex", &source)),
            ("live", file_events("codex", &source)),
        ] {
            assert_eq!(cycle_count(&events), 0, "{reader}, {kind}");
            assert!(
                events.iter().any(|event| matches!(
                    &event.fact,
                    NativeEvidenceFact::Message(NativeMessageObservation::Public(value))
                        if value.context.as_ref().is_some_and(|context| !context.complete_through_event)
                )),
                "{reader}, {kind}"
            );
        }
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("session.jsonl");
        std::fs::write(&path, &source).unwrap();
        let page =
            read_native_file_page(&input("codex", RawSource::File(path)), None, &|| false).unwrap();
        assert!(!page.history_complete, "{kind}");
        assert_live_splits("codex", &source);
    }
}

fn lifecycle_payloads() -> Vec<Value> {
    vec![
        json!({"type":"web_search_begin","call_id":"search-1"}),
        json!({"type":"web_search_end","call_id":"search-1","query":"synthetic","action":{"type":"search","query":"synthetic","queries":["synthetic"]}}),
        json!({"type":"collab_agent_spawn_begin","call_id":"spawn-1","sender_thread_id":"facts","started_at_ms":100,"prompt":"PRIVATE CHILD PROMPT","model":"child-model","reasoning_effort":"low"}),
        json!({"type":"collab_agent_spawn_end","call_id":"spawn-1","sender_thread_id":"facts","new_thread_id":"child","completed_at_ms":200,"prompt":"PRIVATE CHILD PROMPT","model":"child-model","reasoning_effort":"low","new_agent_nickname":"worker","new_agent_role":"reviewer","status":"running"}),
        json!({"type":"collab_agent_interaction_begin","call_id":"send-1","sender_thread_id":"facts","receiver_thread_id":"child","prompt":"PRIVATE CHILD PROMPT"}),
        json!({"type":"collab_agent_interaction_end","call_id":"send-1","sender_thread_id":"facts","receiver_thread_id":"child","prompt":"PRIVATE CHILD PROMPT","status":{"completed":"PRIVATE CHILD OUTPUT"}}),
        json!({"type":"collab_waiting_begin","call_id":"wait-1","sender_thread_id":"facts","receiver_thread_ids":["child"],"receiver_agents":[{"thread_id":"child","agent_nickname":"worker","agent_role":"reviewer"}]}),
        json!({"type":"collab_waiting_end","call_id":"wait-1","sender_thread_id":"facts","statuses":{"child":{"completed":"PRIVATE CHILD OUTPUT"}},"agent_statuses":[{"thread_id":"child","agent_nickname":"worker","agent_role":"reviewer","status":{"completed":"PRIVATE CHILD OUTPUT"}}]}),
        json!({"type":"collab_close_begin","call_id":"close-1","sender_thread_id":"facts","receiver_thread_id":"child"}),
        json!({"type":"collab_close_end","call_id":"close-1","sender_thread_id":"facts","receiver_thread_id":"child","status":"shutdown"}),
        json!({"type":"collab_resume_begin","call_id":"resume-1","sender_thread_id":"facts","receiver_thread_id":"child","receiver_agent_nickname":"worker","receiver_agent_role":"reviewer"}),
        json!({"type":"collab_resume_end","call_id":"resume-1","sender_thread_id":"facts","receiver_thread_id":"child","status":"running"}),
    ]
}

fn with_lifecycle(payloads: Vec<Value>) -> String {
    let mut records = rows(CODEX);
    records.splice(8..8, payloads.into_iter().map(|payload| json!({"timestamp":"2026-10-07T10:00:08Z","ordinal":0,"type":"event_msg","payload":payload})));
    encode(&records)
}

#[test]
fn codex_persisted_legacy_web_search_end_keeps_standalone_native_evidence() {
    let mut records = rows(include_str!("fixtures/native_tool_facts/codex.jsonl"));
    let payload = lifecycle_payloads().remove(1);
    records.insert(
        4,
        json!({"timestamp":"2026-10-07T10:00:02Z","type":"event_msg","payload":payload}),
    );
    let source = encode(&records);
    for events in [full_events("codex", &source), file_events("codex", &source)] {
        assert_eq!(cycle_count(&events), 0);
        let messages: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.fact {
                NativeEvidenceFact::Message(message) => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(messages.len(), 2);
        assert!(
            matches!(messages[0], NativeMessageObservation::Public(value) if value.eligible && matches!(&value.payload, PublicMessagePayload::ToolCall { tool_name, .. } if tool_name == "spawn_agent"))
        );
        assert!(
            matches!(messages[1], NativeMessageObservation::ToolResult(value) if value.arguments.is_some() && value.context.as_ref().is_some_and(|context| context.main_session))
        );
    }
    assert_live_splits("codex", &source);
}

#[test]
fn codex_unpersisted_or_changed_paginated_lifecycle_shapes_still_block_cycles() {
    let mut cases = lifecycle_payloads();
    for mut payload in lifecycle_payloads() {
        payload["usage"] = json!({"output_tokens":1});
        cases.push(payload);
    }
    cases.extend([
        json!({"call_id":"search-1"}),
        json!({"type":"unreviewed_lifecycle","call_id":"search-1"}),
        json!({"type":"web_search_begin"}),
        json!({"type":"web_search_begin","call_id":"search-1","extra":true}),
        json!({"type":"web_search_end","call_id":"search-1","query":"synthetic","action":{"type":"unreviewed"}}),
        json!({"type":"web_search_end","call_id":"search-1","query":"synthetic","action":{"type":"search"},"results":[{"text":"unbound output"}]}),
        json!({"type":"collab_close_end","call_id":"close-1","sender_thread_id":"foreign","receiver_thread_id":"child","status":"shutdown"}),
        json!({"type":"collab_close_end","call_id":"close-1","sender_thread_id":"facts","receiver_thread_id":"child","status":{"unreviewed":"state"}}),
    ]);
    for payload in cases {
        let source = with_lifecycle(vec![payload.clone()]);
        for events in [full_events("codex", &source), file_events("codex", &source)] {
            assert_eq!(cycle_count(&events), 0, "{payload}");
            assert!(
                events
                    .iter()
                    .any(|event| matches!(&event.fact, NativeEvidenceFact::Message(_)))
            );
        }
    }
}

fn pi_continuation_rows() -> Vec<Value> {
    vec![
        json!({"type":"custom","customType":"synthetic-state","data":{"model":"extension data","toolCallId":"not a call"}}),
        json!({"type":"custom_message","customType":"synthetic-note","content":[{"type":"text","text":"PRIVATE EXTENSION CONTEXT"}],"display":false,"details":{"usage":"extension data"}}),
        json!({"type":"compaction","summary":"PRIVATE COMPACTION SUMMARY","firstKeptEntryId":"user-1","tokensBefore":1200,"usage":{"input":100,"output":10,"cacheRead":0,"cacheWrite":0,"totalTokens":110,"cost":{"input":0.1,"output":0.1,"cacheRead":0,"cacheWrite":0,"total":0.2}},"fromHook":false}),
    ]
}

fn with_pi_continuations(mut entries: Vec<Value>) -> String {
    let mut records = rows(PI);
    let mut parent = "user-1".to_owned();
    for (index, entry) in entries.iter_mut().enumerate() {
        entry["id"] = json!(format!("continuation-{index}"));
        entry["parentId"] = json!(parent);
        entry["timestamp"] = json!("2026-10-07T10:00:02Z");
        parent = format!("continuation-{index}");
    }
    records[3]["parentId"] = json!(parent);
    records.splice(3..3, entries);
    encode(&records)
}

#[test]
fn pi_reviewed_custom_and_compaction_rows_keep_later_public_events_live() {
    let source = with_pi_continuations(pi_continuation_rows());
    for events in [full_events("pi", &source), file_events("pi", &source)] {
        let public: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.fact {
                NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(public.len(), 3);
        assert!(public.iter().all(|message| message.eligible));
        assert!(events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) if value.call_id.as_deref() == Some("call-1") && value.arguments.is_some())));
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains("PRIVATE EXTENSION")
        );
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains("PRIVATE COMPACTION")
        );
    }
    assert_live_splits("pi", &source);
}

#[test]
fn pi_context_changing_continuations_keep_admission_but_mark_later_context_partial() {
    for entry in pi_continuation_rows() {
        let partial = entry["type"] != "custom";
        let source = with_pi_continuations(vec![entry]);
        for events in [full_events("pi", &source), file_events("pi", &source)] {
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(&event.fact, NativeEvidenceFact::AdmissionFailure(_)))
            );
            let mut calls = 0;
            let mut results = 0;
            let mut text = 0;
            for event in events {
                match event.fact {
                    NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) => {
                        assert!(value.eligible);
                        let context = value.context.as_ref().unwrap();
                        assert!(context.main_session);
                        assert_eq!(context.complete_through_event, !partial);
                        match value.payload {
                            PublicMessagePayload::ToolCall { .. } => calls += 1,
                            PublicMessagePayload::AssistantText { .. } => text += 1,
                        }
                    }
                    NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) => {
                        let context = value.context.as_ref().unwrap();
                        assert!(context.main_session);
                        assert_eq!(context.complete_through_event, !partial);
                        assert_eq!(
                            value.completeness,
                            antiburn_local::analysis::ToolResultCompleteness::Complete
                        );
                        results += 1;
                    }
                    _ => {}
                }
            }
            assert_eq!((calls, results, text), (1, 1, 2));
        }
        assert_live_splits("pi", &source);
    }
}

#[test]
fn pi_continuation_requires_reviewed_content_and_the_exact_parent_chain() {
    for case in [
        "parent",
        "missing-parent",
        "missing-id",
        "duplicate-id",
        "custom-tool",
        "custom-message-role",
        "compaction-shape",
        "compaction-usage",
        "branch",
    ] {
        let mut records = rows(&with_pi_continuations(pi_continuation_rows()));
        match case {
            "parent" => records[3]["parentId"] = json!("selection"),
            "missing-parent" => {
                records[3].as_object_mut().unwrap().remove("parentId");
            }
            "missing-id" => {
                records[3].as_object_mut().unwrap().remove("id");
            }
            "duplicate-id" => records[3]["id"] = json!("user-1"),
            "custom-tool" => records[3]["toolCallId"] = json!("unbound"),
            "custom-message-role" => {
                records[4]["message"] =
                    json!({"role":"assistant","content":[{"type":"text","text":"unbound"}]})
            }
            "compaction-shape" => {
                records[5]
                    .as_object_mut()
                    .unwrap()
                    .remove("firstKeptEntryId");
            }
            "compaction-usage" => records[5]["usage"]["model"] = json!("unreviewed"),
            "branch" => records[5]["type"] = json!("branch_summary"),
            _ => unreachable!(),
        }
        let source = encode(&records);
        let events = file_events("pi", &source);
        assert!(
            !events.iter().any(|event| matches!(
                &event.fact,
                NativeEvidenceFact::Message(NativeMessageObservation::Public(_))
            )),
            "{case}"
        );
        let invalid_index = match case {
            "custom-message-role" => 4,
            "compaction-shape" | "compaction-usage" | "branch" => 5,
            _ => 3,
        };
        assert_pi_denial_splits(&source, invalid_index, case);
    }
}

#[test]
fn pi_legacy_metric_migrations_do_not_enable_v3_live_evidence() {
    for (fixture, session_id, input_tokens) in [
        (
            include_str!("fixtures/pi_characterization/v1_migrated.jsonl"),
            "v1-session",
            2,
        ),
        (
            include_str!("fixtures/pi_characterization/v2_migrated.jsonl"),
            "v2-session",
            3,
        ),
    ] {
        let source = fixture.replace(session_id, "facts");
        let normalized = reader_for("pi")
            .normalize(&input("pi", RawSource::Jsonl(source.clone())))
            .unwrap();
        assert!(
            normalized
                .events
                .iter()
                .any(|event| event.usage.input_tokens == input_tokens)
        );
        assert_no_live_admission(&file_events("pi", &source));
        assert_live_splits("pi", &source);
    }
    for parent in [Value::Null, json!("/synthetic/parent.jsonl")] {
        let mut records = rows(&with_pi_continuations(pi_continuation_rows()));
        records[0]["parentSession"] = parent;
        assert_no_live_admission(&file_events("pi", &encode(&records)));
    }
    for version in [json!(1), json!(2), json!(4), json!("3"), Value::Null] {
        let mut records = rows(PI);
        records[0]["version"] = version;
        assert_no_live_admission(&file_events("pi", &encode(&records)));
    }
    let mut records = rows(PI);
    records[0]["id"] = json!("foreign-session");
    assert_no_live_admission(&file_events("pi", &encode(&records)));
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("session.jsonl");
    std::fs::write(&path, PI).unwrap();
    let mut fork = input("pi", RawSource::File(path));
    fork.fork_parent_session_id = Some("parent".into());
    let error = read_native_file_page(&fork, None, &|| false).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<antiburn_local::analysis::file_live::NativeFileReadError>(),
        Some(antiburn_local::analysis::file_live::NativeFileReadError::Unsupported)
    ));
}

#[test]
fn pi_continuations_do_not_replace_exact_direct_parent_result_joins() {
    for mut entry in pi_continuation_rows() {
        let mut records = rows(PI);
        entry["id"] = json!("bridge");
        entry["parentId"] = json!("assistant-1");
        entry["timestamp"] = json!("2026-10-07T10:00:03Z");
        records[4]["parentId"] = json!("bridge");
        records.insert(4, entry);
        let source = encode(&records);
        for events in [full_events("pi", &source), file_events("pi", &source)] {
            let result = events
                .iter()
                .find_map(|event| match &event.fact {
                    NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) => {
                        Some(value)
                    }
                    _ => None,
                })
                .unwrap();
            assert!(result.arguments.is_none());
            assert!(result.facts.is_none());
            assert_eq!(
                result.completeness,
                antiburn_local::analysis::ToolResultCompleteness::UnsafeIdentity
            );
            assert!(events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) if value.record_id == "assistant-2")));
        }
        assert_live_splits("pi", &source);
    }
}

#[test]
fn pi_continuation_does_not_turn_a_missing_result_status_into_success() {
    let mut records = rows(&with_pi_continuations(pi_continuation_rows()));
    records[7]["message"]
        .as_object_mut()
        .unwrap()
        .remove("isError");
    let source = encode(&records);
    for events in [full_events("pi", &source), file_events("pi", &source)] {
        let result = events
            .iter()
            .find_map(|event| match &event.fact {
                NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) => {
                    Some(value)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(
            result.status,
            antiburn_local::analysis::ToolResultStatus::Unknown
        );
        assert!(!result.finality);
        assert_eq!(
            result.completeness,
            antiburn_local::analysis::ToolResultCompleteness::Incomplete
        );
    }
    assert_live_splits("pi", &source);
}

#[test]
fn codex_mcp_check_categories_require_exact_flat_native_names_and_call_joins() {
    let fixture = include_str!("fixtures/native_tool_facts/codex.jsonl");
    for (name, expected) in [
        ("mcp__server__tool", CheckToolCategory::Mcp),
        ("mcp__Calendar_2__read-file", CheckToolCategory::Mcp),
        ("MCP__server__tool", CheckToolCategory::Other),
        ("mcp__server", CheckToolCategory::Other),
        ("mcp____tool", CheckToolCategory::Other),
        ("mcp__server__", CheckToolCategory::Other),
        ("mcp__server__tool__extra", CheckToolCategory::Other),
        ("mcp__server___tool", CheckToolCategory::Other),
        ("mcp__server__tool/suffix", CheckToolCategory::Other),
        ("mcp__server__ tool", CheckToolCategory::Other),
        ("custom_mcp__server__tool", CheckToolCategory::Other),
    ] {
        let mut records = rows(fixture);
        records[3]["payload"]["name"] = json!(name);
        let source = encode(&records);
        for events in [full_events("codex", &source), file_events("codex", &source)] {
            let categories: Vec<_> = events
                .iter()
                .filter_map(|event| match &event.fact {
                    NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) => {
                        match &value.payload {
                            PublicMessagePayload::ToolCall {
                                tool_name, facts, ..
                            } => {
                                assert_eq!(tool_name, name);
                                facts.as_ref().and_then(|facts| facts.category)
                            }
                            _ => None,
                        }
                    }
                    NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) => {
                        value.facts.as_ref().and_then(|facts| facts.category)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(categories, [expected, expected], "{name}");
        }
        assert_live_splits("codex", &source);
        records[5]["payload"]["call_id"] = json!("unmatched");
        let events = file_events("codex", &encode(&records));
        assert!(events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) if value.facts.is_none())));
    }
}

#[test]
fn codex_namespaced_mcp_calls_keep_the_same_exact_identity_in_calls_and_results() {
    let fixture = include_str!("fixtures/native_tool_facts/codex.jsonl");
    for (namespace, name, expected_name, category) in [
        (
            "mcp__echo__",
            "query_with_delay",
            "mcp__echo__query_with_delay",
            CheckToolCategory::Mcp,
        ),
        (
            "mcp__Calendar",
            "read",
            "mcp__Calendar__read",
            CheckToolCategory::Mcp,
        ),
        (
            "mcp__Calendar",
            "shell",
            "mcp__Calendar__shell",
            CheckToolCategory::Mcp,
        ),
        (
            "mcp__ambiguous__server",
            "shell",
            "shell",
            CheckToolCategory::Other,
        ),
        (
            "mcp__echo____",
            "query_with_delay",
            "query_with_delay",
            CheckToolCategory::Other,
        ),
        (
            "mcp__echo__",
            "_query_with_delay",
            "_query_with_delay",
            CheckToolCategory::Other,
        ),
        (
            "other",
            "mcp__Calendar__read",
            "mcp__Calendar__read",
            CheckToolCategory::Other,
        ),
        (
            "mcp",
            "server__tool",
            "server__tool",
            CheckToolCategory::Other,
        ),
    ] {
        let mut records = rows(fixture);
        records[3]["payload"]["name"] = json!(name);
        records[3]["payload"]["namespace"] = json!(namespace);
        let source = encode(&records);
        for events in [full_events("codex", &source), file_events("codex", &source)] {
            for event in events {
                match event.fact {
                    NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) => {
                        if let PublicMessagePayload::ToolCall {
                            tool_name, facts, ..
                        } = value.payload
                        {
                            assert_eq!(tool_name, expected_name);
                            assert_eq!(facts.unwrap().category, Some(category));
                        }
                    }
                    NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) => {
                        assert_eq!(value.tool_name.as_deref(), Some(expected_name));
                        assert_eq!(value.facts.unwrap().category, Some(category));
                    }
                    _ => {}
                }
            }
        }
        assert_live_splits("codex", &source);
    }
}

#[test]
fn codex_numbered_shell_reads_need_a_proven_successful_extent_for_check_category() {
    let fixture = rows(include_str!(
        "fixtures/codex_characterization/paginated_completed.jsonl"
    ));
    for case in [
        "proved",
        "failed",
        "unknown-status",
        "missing-exit",
        "empty",
        "clipped",
        "wrong-number",
        "stdout-mismatch",
        "stderr",
        "unreviewed-command",
        "unreviewed-parsed-type",
        "wrong-parsed-path",
    ] {
        let mut records = vec![fixture[0].clone(), fixture[1].clone(), fixture[8].clone()];
        records[0]["payload"]["id"] = json!("facts");
        records[2]["payload"]["thread_id"] = json!("facts");
        let item = &mut records[2]["payload"]["item"];
        match case {
            "proved" => {}
            "failed" => {
                item["status"] = json!("failed");
                item["exit_code"] = json!(1);
            }
            "unknown-status" => item["status"] = json!("unreviewed"),
            "missing-exit" => {
                item.as_object_mut().unwrap().remove("exit_code");
            }
            "empty" => {
                item["stdout"] = json!("");
                item["aggregated_output"] = json!("");
            }
            "clipped" => {
                item["stdout"] = json!("     2\t[truncated]\n");
                item["aggregated_output"] = item["stdout"].clone();
            }
            "wrong-number" => {
                item["stdout"] = json!("    99\tunproved\n");
                item["aggregated_output"] = item["stdout"].clone();
            }
            "stdout-mismatch" => item["stdout"] = json!("different"),
            "stderr" => item["stderr"] = json!("warning"),
            "unreviewed-command" => item["command"][2] = json!("cat sample.py"),
            "unreviewed-parsed-type" => item["parsed_cmd"][0]["type"] = json!("unknown"),
            "wrong-parsed-path" => item["parsed_cmd"][0]["path"] = json!("different.py"),
            _ => unreachable!(),
        }
        let source = encode(&records);
        let expected = if case == "proved" {
            CheckToolCategory::Read
        } else {
            CheckToolCategory::Shell
        };
        for events in [full_events("codex", &source), file_events("codex", &source)] {
            let result = events
                .iter()
                .find_map(|event| match &event.fact {
                    NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) => {
                        Some(value)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                result.facts.as_ref().unwrap().category,
                Some(expected),
                "{case}"
            );
        }
        assert_live_splits("codex", &source);
    }
    let mut records = rows(include_str!("fixtures/native_tool_facts/codex.jsonl"));
    records[3]["payload"]["name"] = json!("exec_command");
    records[3]["payload"]["arguments"] = json!("{\"cmd\":\"nl -ba sample.py | sed -n '2,4p'\"}");
    let events = file_events("codex", &encode(&records));
    assert!(events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) if value.facts.as_ref().unwrap().category == Some(CheckToolCategory::Shell))));
}
