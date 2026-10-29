use antiburn_local::analysis::file_live::{NativeFileCursor, read_native_file_page};
use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, NativeEvidenceEvent, NativeEvidenceFact,
    NativeMessageObservation, NativeToolInput, PublicMessagePayload, RawSource,
    ResponseCycleStatus, SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator,
    SourceCapabilities, SourceFormat, SourceKind, ToolResultPosition, reader_for,
};
use serde_json::{Value, json};
use std::io::Write;
use tempfile::TempDir;

#[path = "support/jsonl_fixture.rs"]
mod jsonl_fixture;

#[path = "support/public_event_fixtures.rs"]
mod public_event_fixtures;

const CODEX: &str = include_str!("fixtures/public_events/codex.jsonl");
const PI: &str = include_str!("fixtures/public_events/pi.jsonl");
const CLAUDE: &str = include_str!("fixtures/native_tool_facts/claude.jsonl");

fn input(agent: &str, format: SourceFormat, source: RawSource) -> SessionInput {
    SessionInput {
        agent: agent.to_owned(),
        session_id: "facts".to_owned(),
        source_format: format,
        source,
        fork_parent_session_id: None,
    }
}

fn file_events(agent: &str, format: SourceFormat, source: &str) -> Vec<NativeEvidenceEvent> {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("session.jsonl");
    std::fs::write(&path, source).unwrap();
    let input = input(agent, format, RawSource::File(path));
    let mut cursor = None;
    let mut events = Vec::new();
    loop {
        let page = read_native_file_page(&input, cursor.as_ref(), &|| false).unwrap();
        assert!(page.evidence.events.len() <= 256);
        assert!(serde_json::to_vec(&page.evidence.events).unwrap().len() <= 12 * 1024 * 1024);
        assert_eq!(
            page.results.len(),
            page.evidence
                .events
                .iter()
                .filter(|event| matches!(
                    &event.fact,
                    NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(_))
                ))
                .count()
        );
        events.extend(page.evidence.events);
        cursor = Some(NativeFileCursor::decode(&page.cursor.encode()).unwrap());
        if !page.has_more {
            return events;
        }
    }
}

fn edited(mut edit: impl FnMut(&mut Vec<Value>)) -> String {
    let mut records: Vec<Value> = CODEX
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    edit(&mut records);
    records
        .iter_mut()
        .enumerate()
        .map(|(index, record)| {
            record["ordinal"] = json!(index);
            format!("{record}\n")
        })
        .collect()
}

fn with_appended_codex_terminal(
    source: &str,
    line_ending: &str,
    edit: impl FnOnce(&mut Value),
) -> String {
    assert!(source.ends_with(line_ending));
    let (index, terminal) = source.lines().enumerate().last().unwrap();
    let mut terminal: Value = serde_json::from_str(terminal).unwrap();
    terminal["ordinal"] = json!(index + 1);
    edit(&mut terminal);
    format!("{source}{terminal}{line_ending}")
}

fn cycles(
    events: &[NativeEvidenceEvent],
) -> Vec<&antiburn_local::analysis::ResponseCycleObservation> {
    events
        .iter()
        .filter_map(|event| match &event.fact {
            NativeEvidenceFact::ResponseCycle(value) => Some(value),
            _ => None,
        })
        .collect()
}

fn full_events(agent: &str, format: SourceFormat, source: &str) -> Vec<NativeEvidenceEvent> {
    let capabilities = match agent {
        "codex" => SourceCapabilities::codex(),
        "pi" => SourceCapabilities::pi(),
        _ => panic!("unsupported test agent"),
    };
    let mut sink = CompositeSink::new(
        SessionMetricsAccumulator::new(agent, "facts"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: agent.into(),
            session_id: "facts".into(),
            kind: SourceKind::Jsonl,
            capabilities,
        }),
    );
    reader_for(agent)
        .visit(
            &input(agent, format, RawSource::Jsonl(source.to_owned())),
            &mut sink,
        )
        .unwrap();
    sink.native_evidence().to_vec()
}

#[test]
fn incomplete_codex_result_content_never_completes_a_cycle() {
    for output in [
        json!([{"type":"text","text":"retained"},{"type":"text"}]),
        json!([{"type":"text","text":"retained"},{"type":"image","data":"private"}]),
    ] {
        let source = public_event_fixtures::codex_incomplete_output(output);
        for events in [
            file_events("codex", SourceFormat::CodexRolloutJsonl, &source),
            full_events("codex", SourceFormat::CodexRolloutJsonl, &source),
        ] {
            assert!(cycles(&events).is_empty());
            let result = events
                .iter()
                .find_map(|event| match &event.fact {
                    NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) => {
                        Some(value)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(result.content.as_deref(), Some("retained"));
            assert_eq!(
                result.completeness,
                antiburn_local::analysis::ToolResultCompleteness::Incomplete
            );
        }
    }
}

#[test]
fn incomplete_earlier_file_history_does_not_hide_a_complete_codex_interval() {
    let (header, interval) = CODEX.split_once('\n').unwrap();
    let source = format!("{header}\n{}{interval}", "\n".repeat(1024 * 1024));
    let events = file_events("codex", SourceFormat::CodexRolloutJsonl, &source);
    let cycles = cycles(&events);
    assert_eq!(cycles.len(), 1);
    assert!(!cycles[0].context.complete_through_event);
    let result = cycles[0]
        .messages
        .iter()
        .find_map(|message| match message {
            NativeMessageObservation::ToolResult(value) => Some(value),
            _ => None,
        })
        .unwrap();
    assert!(!result.context.as_ref().unwrap().complete_through_event);
    assert_eq!(
        result.completeness,
        antiburn_local::analysis::ToolResultCompleteness::Complete
    );
}

#[test]
fn pi_public_evidence_requires_its_own_record_and_block_content_to_be_complete() {
    for owner in ["record", "message", "call", "text", "private"] {
        for (field, marker) in [
            ("truncated", json!(true)),
            ("incomplete", json!(true)),
            ("complete", json!(false)),
        ] {
            let source = public_event_fixtures::pi_completion_marker(owner, field, marker);
            for events in [
                file_events("pi", SourceFormat::PiV3Jsonl, &source),
                full_events("pi", SourceFormat::PiV3Jsonl, &source),
            ] {
                let public: Vec<_> =
                    events
                        .iter()
                        .filter_map(|event| match &event.fact {
                            NativeEvidenceFact::Message(NativeMessageObservation::Public(
                                value,
                            )) if value.record_id == "assistant-1" => Some(value),
                            _ => None,
                        })
                        .collect();
                let calls = public
                    .iter()
                    .filter(|value| matches!(value.payload, PublicMessagePayload::ToolCall { .. }))
                    .count();
                let text = public
                    .iter()
                    .filter(|value| {
                        matches!(value.payload, PublicMessagePayload::AssistantText { .. })
                    })
                    .count();
                assert_eq!(
                    calls,
                    usize::from(matches!(owner, "text" | "private")),
                    "{owner}.{field}"
                );
                assert_eq!(
                    text,
                    usize::from(matches!(owner, "call" | "private")),
                    "{owner}.{field}"
                );
                assert!(public.iter().all(|value| value.eligible
                    && value.completeness
                        == antiburn_local::analysis::ToolResultCompleteness::Complete));
                assert!(events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) if value.record_id == "assistant-2")));
            }
        }
    }
}

#[test]
fn codex_replaced_work_keeps_the_same_explicit_call_in_full_and_live_delivery() {
    for name in ["exec", "exec_command", "apply_patch"] {
        let source = public_event_fixtures::codex_replaced_call(name);
        let full = full_events("codex", SourceFormat::CodexRolloutJsonl, &source);
        let live = file_events("codex", SourceFormat::CodexRolloutJsonl, &source);
        let call = |events: &[NativeEvidenceEvent]| {
            events
                .iter()
                .filter_map(|event| match &event.fact {
                    NativeEvidenceFact::Message(NativeMessageObservation::Public(value))
                        if matches!(&value.payload, PublicMessagePayload::ToolCall { .. }) =>
                    {
                        Some(value.clone())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let full_calls = call(&full);
        let live_calls = call(&live);
        assert_eq!(full_calls.len(), 1, "{name}");
        assert_eq!(live_calls.len(), 1, "{name}");
        assert_eq!(full_calls[0].record_id, live_calls[0].record_id);
        assert_eq!(full_calls[0].call_id, live_calls[0].call_id);
        assert_eq!(full_calls[0].payload, live_calls[0].payload);
        assert!(full_calls[0].eligible && live_calls[0].eligible);
        let PublicMessagePayload::ToolCall { input, .. } = &full_calls[0].payload else {
            unreachable!()
        };
        assert_eq!(
            matches!(input, Some(NativeToolInput::Text(_))),
            name == "exec"
        );
        for events in [&full, &live] {
            let results: Vec<_> = events
                .iter()
                .filter_map(|event| match &event.fact {
                    NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) => {
                        value.result_id.as_deref()
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(results, ["edit-1", "test-1", "read-1"], "{name}");
        }
    }
}

#[test]
fn codex_public_cycle_members_equal_finalized_standalone_values() {
    let events = file_events("codex", SourceFormat::CodexRolloutJsonl, CODEX);
    let cycles = cycles(&events);
    assert_eq!(cycles.len(), 1);
    let cycle = cycles[0];
    assert_eq!(cycle.status, ResponseCycleStatus::Completed);
    assert_eq!(
        cycle.requester.model_key().as_deref(),
        Some("openai/gpt-test")
    );
    assert_eq!(cycle.requester.fast_mode, Some(true));
    let messages: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.fact {
            NativeEvidenceFact::Message(value) => Some(value.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(cycle.messages, messages);
    assert_eq!(messages.len(), 4);
    assert!(!serde_json::to_string(&events).unwrap().contains("PRIVATE"));
    assert!(matches!(
        cycle.start_position,
        ToolResultPosition::FileBytes { .. }
    ));
    for message in &messages {
        let position = match message {
            NativeMessageObservation::User(value) => value.context.first_eligible_position,
            NativeMessageObservation::Public(value) => value.position,
            NativeMessageObservation::ToolResult(value) => value.position,
        };
        assert!(matches!(position, ToolResultPosition::FileBytes { .. }));
    }
    let call = messages
        .iter()
        .find_map(|message| match message {
            NativeMessageObservation::Public(value) => match &value.payload {
                PublicMessagePayload::ToolCall { input, .. } => input.as_ref(),
                _ => None,
            },
            _ => None,
        })
        .unwrap();
    assert!(
        matches!(call, NativeToolInput::Text(text) if text == "{\"raw\":\"keep this as text\"}")
    );
}

#[test]
fn pi_splits_calls_and_text_without_losing_native_ordinals() {
    let events = file_events("pi", SourceFormat::PiV3Jsonl, PI);
    let public: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.fact {
            NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(public.len(), 3);
    assert!(matches!(
        public[0].position,
        ToolResultPosition::FileBytes { item_index: 2, .. }
    ));
    assert!(matches!(
        public[1].position,
        ToolResultPosition::FileBytes { item_index: 4, .. }
    ));
    let PublicMessagePayload::AssistantText { parts, generator } = &public[1].payload else {
        panic!("expected text")
    };
    assert_eq!(
        parts.iter().map(|part| part.item_index).collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(generator.fast_mode, Some(true));
    assert!(cycles(&events).is_empty());
    assert!(!serde_json::to_string(&events).unwrap().contains("PRIVATE"));
}

#[test]
fn all_file_agents_deliver_explicit_calls_but_claude_has_no_invented_final_text() {
    for (agent, format, source) in [
        ("claude", SourceFormat::ClaudeJsonl, CLAUDE),
        ("codex", SourceFormat::CodexRolloutJsonl, CODEX),
        ("pi", SourceFormat::PiV3Jsonl, PI),
    ] {
        let events = file_events(agent, format, source);
        assert!(events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) if matches!(&value.payload, PublicMessagePayload::ToolCall { .. }))));
        assert!(
            events
                .iter()
                .any(|event| matches!(&event.fact, NativeEvidenceFact::Activity(_)))
        );
        if agent == "claude" {
            assert!(!events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) if matches!(&value.payload, PublicMessagePayload::AssistantText { .. }))));
        }
    }
}

#[test]
fn codex_cycles_require_complete_correlated_inputs_and_public_intervals() {
    for case in [
        "missing-start",
        "missing-input",
        "unbound-input",
        "wrong-input-turn",
        "wrong-terminal",
        "idless-terminal",
        "overlap",
        "gap",
        "event-gap",
        "compaction",
        "missing-projection",
        "changed-projection",
        "unresolved-call",
        "unknown-abort",
    ] {
        let source = edited(|records| match case {
            "missing-start" => {
                records.remove(1);
            }
            "missing-input" => {
                records.remove(5);
                records.remove(4);
            }
            "unbound-input" => records[4]["metadata"]["retained_source"]["complete"] = json!(false),
            "wrong-input-turn" => {
                records[4]["payload"]["internal_chat_message_metadata_passthrough"]["turn_id"] =
                    json!("other-turn")
            }
            "wrong-terminal" => {
                records.last_mut().unwrap()["payload"]["turn_id"] = json!("other-turn")
            }
            "idless-terminal" => {
                records.last_mut().unwrap()["payload"]
                    .as_object_mut()
                    .unwrap()
                    .remove("turn_id");
            }
            "overlap" => {
                records.insert(4, records[1].clone());
            }
            "gap" => {
                records[8] =
                    json!({"type":"response_item","payload":{"type":"unreviewed-content"}});
            }
            "event-gap" => {
                records[8] = json!({"type":"event_msg","payload":{"type":"unreviewed-content","text":"synthetic"}});
            }
            "compaction" => records[8]["type"] = json!("compacted"),
            "missing-projection" => {
                records.remove(9);
            }
            "changed-projection" => {
                records[10]["payload"]["content"][0]["text"] = json!("different")
            }
            "unresolved-call" => {
                records.remove(7);
            }
            "unknown-abort" => {
                records.last_mut().unwrap()["payload"] =
                    json!({"type":"turn_aborted","turn_id":"turn-1","reason":"unknown"})
            }
            _ => unreachable!(),
        });
        let events = file_events("codex", SourceFormat::CodexRolloutJsonl, &source);
        assert!(cycles(&events).is_empty(), "{case}");
        assert!(
            events
                .iter()
                .any(|event| matches!(&event.fact, NativeEvidenceFact::Message(_))),
            "{case}"
        );
    }
}

#[test]
fn codex_terminal_aliases_errors_and_each_typed_abort_keep_their_meaning() {
    for reason in ["interrupted", "replaced", "review_ended", "budget_limited"] {
        let source = edited(|records| {
            records.last_mut().unwrap()["payload"] =
                json!({"type":"turn_aborted","turn_id":"turn-1","reason":reason})
        });
        let events = file_events("codex", SourceFormat::CodexRolloutJsonl, &source);
        assert_eq!(cycles(&events)[0].status, ResponseCycleStatus::Cancelled);
        assert_eq!(cycles(&events)[0].native_reason.as_deref(), Some(reason));
    }
    let source = edited(|records| {
        records[1]["payload"]["type"] = json!("turn_started");
        records.last_mut().unwrap()["payload"] = json!({"type":"turn_complete","turn_id":"turn-1","error":{"codex_error_info":"server_overloaded","message":"synthetic"}});
    });
    let events = file_events("codex", SourceFormat::CodexRolloutJsonl, &source);
    assert_eq!(cycles(&events)[0].status, ResponseCycleStatus::Error);
    assert_eq!(
        cycles(&events)[0].native_error_code.as_deref(),
        Some("server_overloaded")
    );
}

#[test]
fn settings_only_records_update_selection_without_activity() {
    for (agent, format, source) in [
        (
            "pi",
            SourceFormat::PiV3Jsonl,
            PI.lines()
                .take(2)
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
        ),
        (
            "codex",
            SourceFormat::CodexRolloutJsonl,
            edited(|records| {
                records.remove(1);
                records.truncate(2);
            }),
        ),
    ] {
        let events = file_events(agent, format, &source);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0].fact, NativeEvidenceFact::SessionSelection(value) if value.selection.fast_mode == Some(true))
        );
    }
}

#[test]
fn every_live_split_and_cursor_encoding_keeps_the_same_frozen_members() {
    for (agent, format, source) in [
        ("codex", SourceFormat::CodexRolloutJsonl, CODEX),
        ("pi", SourceFormat::PiV3Jsonl, PI),
        ("claude", SourceFormat::ClaudeJsonl, CLAUDE),
    ] {
        for line_ending in ["\n", "\r\n"] {
            let source = jsonl_fixture::with_line_ending(source, line_ending);
            let expected: Vec<_> = file_events(agent, format, &source)
                .into_iter()
                .map(|event| event.fact)
                .collect();
            let lines: Vec<_> = source.split_inclusive('\n').collect();
            for boundary in 1..lines.len() {
                let directory = TempDir::new().unwrap();
                let path = directory.path().join("session.jsonl");
                std::fs::write(&path, lines[..boundary].concat()).unwrap();
                let input = input(agent, format, RawSource::File(path.clone()));
                let first = read_native_file_page(&input, None, &|| false).unwrap();
                let json = serde_json::to_vec(&first.cursor).unwrap();
                let cursor: NativeFileCursor = serde_json::from_slice(&json).unwrap();
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
}

#[test]
fn full_parse_uses_jsonl_positions_and_the_same_cycle_admission() {
    let mut sink = CompositeSink::new(
        SessionMetricsAccumulator::new("codex", "facts"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "codex".into(),
            session_id: "facts".into(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::codex(),
        }),
    );
    reader_for("codex")
        .visit(
            &input(
                "codex",
                SourceFormat::CodexRolloutJsonl,
                RawSource::Jsonl(CODEX.to_owned()),
            ),
            &mut sink,
        )
        .unwrap();
    let cycles = cycles(sink.native_evidence());
    assert_eq!(cycles.len(), 1);
    assert!(matches!(
        cycles[0].position,
        ToolResultPosition::Jsonl { .. }
    ));
    let standalone: Vec<_> = sink
        .native_evidence()
        .iter()
        .filter_map(|event| match &event.fact {
            NativeEvidenceFact::Message(value) => Some(value.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(cycles[0].messages, standalone);
}

#[test]
fn exact_steering_keeps_as_of_prefixes_and_no_text_requests_affect_uniformity() {
    let source = edited(|records| {
        let mut steering = records[4].clone();
        steering["payload"]["id"] = json!("steering-1");
        steering["payload"]["content"][0]["text"] = json!("Also check naming.");
        steering["metadata"]["retained_source"]["id"]["message_id"] = json!("steering-1");
        steering["metadata"]["retained_source"]["revision"] = json!("retained-2");
        records.insert(8, steering);
        let mut context = records[3].clone();
        context["payload"]["model"] = json!("different-model");
        records.insert(9, context);
        records.insert(10, json!({"timestamp":"2026-10-07T10:00:08Z","type":"response_item","payload":{"type":"reasoning","summary":[],"encrypted_content":"private"}}));
    });
    let events = file_events("codex", SourceFormat::CodexRolloutJsonl, &source);
    let cycle = cycles(&events)[0];
    assert_eq!(
        cycle
            .messages
            .iter()
            .filter(|message| matches!(message, NativeMessageObservation::User(_)))
            .count(),
        2
    );
    assert_eq!(cycle.requester.model, None);
    assert_eq!(cycle.requester.fast_mode, None);
    let standalone: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.fact {
            NativeEvidenceFact::Message(value) => Some(value.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(cycle.messages, standalone);
    let result = cycle
        .messages
        .iter()
        .find_map(|message| match message {
            NativeMessageObservation::ToolResult(value) => Some(value),
            _ => None,
        })
        .unwrap();
    assert_eq!(result.context.as_ref().unwrap().user_messages.len(), 1);
    let assistant = cycle
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            NativeMessageObservation::Public(value) => Some(value),
            _ => None,
        })
        .unwrap();
    assert_eq!(assistant.context.as_ref().unwrap().user_messages.len(), 2);
}

#[test]
fn pi_error_aborted_length_and_missing_stops_do_not_complete_public_text() {
    for stop in [
        Value::Null,
        json!("error"),
        json!("aborted"),
        json!("length"),
        json!("unknown"),
    ] {
        let source: String = PI
            .lines()
            .map(|line| {
                let mut value: Value = serde_json::from_str(line).unwrap();
                if value["message"]["role"] == "assistant" {
                    value["message"]["stopReason"] = stop.clone();
                }
                format!("{value}\n")
            })
            .collect();
        let events = file_events("pi", SourceFormat::PiV3Jsonl, &source);
        assert!(!events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) if matches!(&value.payload, PublicMessagePayload::AssistantText { .. }))));
        assert!(events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) if matches!(&value.payload, PublicMessagePayload::ToolCall { .. }))));
    }
}

#[test]
fn oversized_cycle_is_unavailable_while_standalone_messages_still_flow() {
    let source = edited(|records| {
        for index in 0..14 {
            let mut user = records[4].clone();
            let id = format!("steering-{index}");
            user["payload"]["id"] = json!(id);
            user["metadata"]["retained_source"]["id"]["message_id"] = json!(id);
            user["payload"]["content"][0]["text"] = json!("x".repeat(60_000));
            records.insert(8 + index, user);
        }
    });
    let events = file_events("codex", SourceFormat::CodexRolloutJsonl, &source);
    assert!(cycles(&events).is_empty());
    assert!(
        events
            .iter()
            .filter(|event| matches!(&event.fact, NativeEvidenceFact::Message(_)))
            .count()
            > 4
    );
}

#[test]
fn conflicting_terminal_withdraws_the_same_cycle_instead_of_creating_another() {
    for line_ending in ["\n", "\r\n"] {
        let baseline = jsonl_fixture::with_line_ending(CODEX, line_ending);
        let source = with_appended_codex_terminal(&baseline, line_ending, |terminal| {
            terminal["payload"]["error"] =
                json!({"codex_error_info":"server_overloaded","message":"synthetic"});
        });
        let expected = file_events("codex", SourceFormat::CodexRolloutJsonl, &baseline);
        let events = file_events("codex", SourceFormat::CodexRolloutJsonl, &source);
        assert_eq!(
            &events[..expected.len()],
            expected.as_slice(),
            "line ending: {line_ending:?}"
        );
        let cycles = cycles(&events);
        assert_eq!(cycles.len(), 2);
        assert_eq!(cycles[0].key, cycles[1].key);
        assert_eq!(
            cycles[1].change,
            antiburn_local::analysis::ToolResultChange::Removal
        );
        assert!(cycles[1].messages.is_empty());
        let activity_count = |events: &[NativeEvidenceEvent]| {
            events
                .iter()
                .filter(|event| matches!(&event.fact, NativeEvidenceFact::Activity(_)))
                .count()
        };
        assert_eq!(activity_count(&events), activity_count(&expected) + 1);
    }
}

#[test]
fn terminal_alias_replay_does_not_withdraw_or_repeat_a_cycle() {
    for line_ending in ["\n", "\r\n"] {
        let baseline = jsonl_fixture::with_line_ending(CODEX, line_ending);
        let source = with_appended_codex_terminal(&baseline, line_ending, |terminal| {
            terminal["payload"]["type"] = json!("turn_complete");
        });
        let events = file_events("codex", SourceFormat::CodexRolloutJsonl, &source);
        assert_eq!(cycles(&events).len(), 1);
        assert_eq!(
            events,
            file_events("codex", SourceFormat::CodexRolloutJsonl, &baseline),
            "line ending: {line_ending:?}"
        );
        assert_eq!(
            full_events("codex", SourceFormat::CodexRolloutJsonl, &source),
            full_events("codex", SourceFormat::CodexRolloutJsonl, &baseline),
            "line ending: {line_ending:?}"
        );
    }
}

#[test]
fn user_only_abort_and_private_only_generation_have_complete_public_intervals() {
    for private_generation in [false, true] {
        let source = edited(|records| {
            let mut terminal = records.last().unwrap().clone();
            terminal["payload"] =
                json!({"type":"turn_aborted","turn_id":"turn-1","reason":"interrupted"});
            let reasoning = records[8].clone();
            records.truncate(6);
            if private_generation {
                records.push(reasoning);
            }
            records.push(terminal);
        });
        let events = file_events("codex", SourceFormat::CodexRolloutJsonl, &source);
        let cycles = cycles(&events);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].messages.len(), 1);
        assert_eq!(cycles[0].status, ResponseCycleStatus::Cancelled);
        assert_eq!(cycles[0].requester.model.is_some(), private_generation);
        assert!(!serde_json::to_string(&events).unwrap().contains("PRIVATE"));
    }
}

#[test]
fn claude_duplicate_call_blocks_are_unsafe_and_native_holes_stay_in_order() {
    for duplicate in [false, true] {
        let source: String = CLAUDE
            .lines()
            .map(|line| {
                let mut record: Value = serde_json::from_str(line).unwrap();
                if record["type"] == "assistant" {
                    let call = record["message"]["content"][0].clone();
                    let mut second = call.clone();
                    if !duplicate {
                        second["id"] = json!("other-call");
                    }
                    record["message"]["content"] = json!([
                        {"type":"thinking","thinking":"PRIVATE"},
                        {"type":"text","text":"This does not prove final public text."},
                        call,
                        {"type":"thinking","thinking":"PRIVATE"},
                        second,
                    ]);
                }
                format!("{record}\n")
            })
            .collect();
        let events = file_events("claude", SourceFormat::ClaudeJsonl, &source);
        let calls: Vec<_> = events
            .iter()
            .filter_map(|event| match &event.fact {
                NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) => Some(value),
                _ => None,
            })
            .collect();
        if duplicate {
            assert!(calls.iter().all(|call| !call.eligible
                && call.completeness
                    == antiburn_local::analysis::ToolResultCompleteness::UnsafeIdentity));
        } else {
            assert_eq!(calls.len(), 2);
            assert!(matches!(
                calls[0].position,
                ToolResultPosition::FileBytes { item_index: 2, .. }
            ));
            assert!(matches!(
                calls[1].position,
                ToolResultPosition::FileBytes { item_index: 4, .. }
            ));
        }
        assert!(!serde_json::to_string(&events).unwrap().contains("PRIVATE"));
    }
}

#[test]
fn a_partial_assistant_projection_keeps_cycle_state_without_publishing_fragments() {
    for line_ending in ["\n", "\r\n"] {
        let source = jsonl_fixture::with_line_ending(CODEX, line_ending);
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("session.jsonl");
        let lines: Vec<_> = source.split_inclusive('\n').collect();
        std::fs::write(&path, lines[..10].concat()).unwrap();
        let input = input(
            "codex",
            SourceFormat::CodexRolloutJsonl,
            RawSource::File(path.clone()),
        );
        let first = read_native_file_page(&input, None, &|| false).unwrap();
        let middle = lines[10].len() / 2;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        write!(file, "{}", &lines[10][..middle]).unwrap();
        let pending = read_native_file_page(&input, Some(&first.cursor), &|| false).unwrap();
        assert!(pending.pending_tail);
        assert!(pending.evidence.events.is_empty());
        let cursor = NativeFileCursor::decode(&pending.cursor.encode()).unwrap();
        let cursor: NativeFileCursor =
            serde_json::from_slice(&serde_json::to_vec(&cursor).unwrap()).unwrap();
        write!(file, "{}{}", &lines[10][middle..], lines[11]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), source.as_bytes());
        let final_page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
        assert_eq!(cycles(&final_page.evidence.events).len(), 1);
        let actual: Vec<_> = first
            .evidence
            .events
            .into_iter()
            .chain(final_page.evidence.events)
            .map(|event| event.fact)
            .collect();
        let expected: Vec<_> = file_events("codex", SourceFormat::CodexRolloutJsonl, &source)
            .into_iter()
            .map(|event| event.fact)
            .collect();
        assert_eq!(actual, expected, "line ending: {line_ending:?}");
    }
}
