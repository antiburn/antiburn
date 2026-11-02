use super::*;
use antiburn_local::analysis::{
    CheckToolCategory, NativeEvidenceEvent, NativeToolInput, PublicMessageObservation,
    ToolCategory, ToolResultCompleteness, ToolResultStatus,
};

fn tool_fixture(
    name: &str,
    input: Value,
    called_executed: bool,
    terminal_executed: bool,
    failed: bool,
) -> Vec<Value> {
    let mut events = v2_fixture();
    for event in &mut events {
        match event["id"].as_str().unwrap() {
            "tool-start-1" => event["data"]["name"] = json!(name),
            "tool-call-1" => {
                event["data"]["input"] = input.clone();
                event["data"]["executed"] = json!(called_executed);
            }
            "tool-result-1" => {
                event["data"]["executed"] = json!(terminal_executed);
                if failed {
                    event["type"] = json!("session.tool.failed");
                    event["data"]["error"] =
                        json!({"type":"tool.execution","message":"Synthetic provider failure"});
                }
            }
            _ => {}
        }
    }
    events
}

fn observe(events: &[Value], baseline: u64) -> (Vec<NativeEvidenceEvent>, OpenCodeV2EventJoiner) {
    let mut joiner = OpenCodeV2EventJoiner::new();
    joiner.set_live_baseline(baseline);
    let evidence = events
        .iter()
        .flat_map(|event| joiner.observe_evidence(event).events)
        .collect();
    (evidence, joiner)
}

fn calls(evidence: &[NativeEvidenceEvent]) -> Vec<&PublicMessageObservation> {
    evidence
        .iter()
        .filter_map(|event| match &event.fact {
            NativeEvidenceFact::Message(NativeMessageObservation::Public(call))
                if matches!(call.payload, PublicMessagePayload::ToolCall { .. }) =>
            {
                Some(call)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn native_edit_and_write_categories_match_calls_results_and_cycle_members() {
    for (name, input, category) in [
        (
            "edit",
            json!({"path":"synthetic.rs","oldString":"old","newString":"new"}),
            CheckToolCategory::Edit,
        ),
        (
            "write",
            json!({"path":"synthetic.rs","content":"new contents"}),
            CheckToolCategory::Write,
        ),
    ] {
        for executed in [false, true] {
            for failed in [false, true] {
                let events = tool_fixture(name, input.clone(), executed, executed, failed);
                let (evidence, joiner) = observe(&events, 0);
                assert!(joiner.gaps().is_empty());
                let calls = calls(&evidence);
                assert_eq!(calls.len(), 1);
                let call = calls[0];
                let PublicMessagePayload::ToolCall {
                    tool_name,
                    input: call_input,
                    facts,
                    provider_executed,
                } = &call.payload
                else {
                    panic!("public call");
                };
                assert_eq!(tool_name, name);
                assert_eq!(call_input, &Some(NativeToolInput::Json(input.clone())));
                assert_eq!(*provider_executed, Some(executed));
                let facts = facts.as_ref().unwrap();
                assert_eq!(
                    facts.category,
                    Some(if executed {
                        CheckToolCategory::Other
                    } else {
                        category
                    })
                );
                assert_eq!(facts.operation_paths, None);
                assert_eq!(facts.requester.model.as_deref(), Some("gpt-6.1-sol-fast"));
                let result = evidence
                    .iter()
                    .find_map(|event| match &event.fact {
                        NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(
                            result,
                        )) => Some(result),
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(result.facts.as_ref(), Some(facts));
                assert_eq!(result.provider_executed, Some(executed));
                assert_eq!(
                    result.tool_category,
                    Some(if executed {
                        ToolCategory::Other
                    } else {
                        ToolCategory::Edit
                    })
                );
                let cycle = evidence
                    .iter()
                    .find_map(|event| match &event.fact {
                        NativeEvidenceFact::ResponseCycle(cycle) => Some(cycle),
                        _ => None,
                    })
                    .unwrap();
                assert!(
                    cycle
                        .messages
                        .contains(&NativeMessageObservation::Public(call.clone()))
                );
                assert!(
                    cycle
                        .messages
                        .contains(&NativeMessageObservation::ToolResult(result.clone()))
                );
                assert_eq!(cycle.messages.len(), 5);
            }
        }
    }
}

#[test]
fn terminal_execution_upgrades_revise_calls_and_replace_exact_cycle_members() {
    for (name, input, category) in [
        ("shell", json!({"command":"true"}), CheckToolCategory::Shell),
        (
            "read",
            json!({"path":"synthetic.rs"}),
            CheckToolCategory::Read,
        ),
        (
            "edit",
            json!({"path":"synthetic.rs","oldString":"old","newString":"new"}),
            CheckToolCategory::Edit,
        ),
        (
            "write",
            json!({"path":"synthetic.rs","content":"new contents"}),
            CheckToolCategory::Write,
        ),
    ] {
        for failed in [false, true] {
            for baseline in [0, 12] {
                let events = tool_fixture(name, input.clone(), false, true, failed);
                let (evidence, mut joiner) = observe(&events, baseline);
                assert!(joiner.gaps().is_empty());
                let calls = calls(&evidence);
                assert_eq!(calls.len(), 2);
                let (original, corrected) = (calls[0], calls[1]);
                assert!(original.eligible && corrected.eligible);
                assert_eq!(original.source_format, corrected.source_format);
                assert_eq!(original.session_id, corrected.session_id);
                assert_eq!(original.record_id, corrected.record_id);
                assert_eq!(original.part_id, corrected.part_id);
                assert_eq!(original.call_id, corrected.call_id);
                assert_eq!(original.context, corrected.context);
                assert_eq!(original.revision, 12);
                assert_eq!(corrected.revision, 13);
                assert_eq!(
                    corrected.position,
                    ToolResultPosition::OpenCodeV2Event {
                        sequence: 13,
                        item_index: 0,
                    }
                );
                assert_eq!(corrected.completeness, ToolResultCompleteness::Complete);
                let context = corrected.context.as_ref().unwrap();
                assert_eq!(context.first_eligible_position, original.position);
                assert_eq!(context.user_messages.len(), 1);
                let PublicMessagePayload::ToolCall {
                    facts: original_facts,
                    ..
                } = &original.payload
                else {
                    panic!("original call");
                };
                assert_eq!(original_facts.as_ref().unwrap().category, Some(category));
                let PublicMessagePayload::ToolCall {
                    tool_name,
                    input: corrected_input,
                    facts,
                    provider_executed,
                } = &corrected.payload
                else {
                    panic!("corrected call");
                };
                assert_eq!(tool_name, name);
                assert_eq!(corrected_input, &Some(NativeToolInput::Json(input.clone())));
                assert_eq!(*provider_executed, Some(true));
                assert_eq!(
                    facts.as_ref().unwrap().category,
                    Some(CheckToolCategory::Other)
                );
                assert_eq!(
                    facts.as_ref().unwrap().requester,
                    original_facts.as_ref().unwrap().requester
                );
                let results: Vec<_> = evidence
                    .iter()
                    .filter_map(|event| match &event.fact {
                        NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(
                            result,
                        )) => Some(result),
                        _ => None,
                    })
                    .collect();
                assert_eq!(results.len(), 1);
                assert_eq!(&results[0].facts, facts);
                assert_eq!(results[0].provider_executed, Some(true));
                assert_eq!(results[0].outcome, None);
                assert_eq!(
                    results[0].status,
                    if failed {
                        ToolResultStatus::Error
                    } else {
                        ToolResultStatus::Success
                    }
                );
                for event in &evidence {
                    if matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::Public(call))
                        if matches!(call.payload, PublicMessagePayload::ToolCall { .. }))
                    {
                        assert_eq!(event.live_eligible, baseline < 12);
                    }
                }
                let cycles: Vec<_> = evidence
                    .iter()
                    .filter_map(|event| match &event.fact {
                        NativeEvidenceFact::ResponseCycle(cycle) => Some(cycle),
                        _ => None,
                    })
                    .collect();
                assert_eq!(cycles.len(), usize::from(baseline == 0));
                if let Some(cycle) = cycles.first() {
                    let members: Vec<_> = cycle
                        .messages
                        .iter()
                        .filter_map(|message| match message {
                            NativeMessageObservation::Public(call) if call.call_id.is_some() => {
                                Some(call)
                            }
                            _ => None,
                        })
                        .collect();
                    assert_eq!(members, [corrected]);
                    assert!(
                        cycle
                            .messages
                            .contains(&NativeMessageObservation::ToolResult(results[0].clone()))
                    );
                    assert_eq!(cycle.messages.len(), 5);
                }
                assert_eq!(
                    evidence
                        .iter()
                        .filter(|event| matches!(
                            event.fact,
                            NativeEvidenceFact::Message(NativeMessageObservation::User(_))
                        ))
                        .count(),
                    2
                );
                assert!(joiner.observe_evidence(&events[12]).events.is_empty());
                assert!(joiner.gaps().is_empty());
            }
        }
    }
}

#[test]
fn failed_upgrade_without_content_corrects_the_call_but_withholds_the_cycle() {
    let mut events = tool_fixture("shell", json!({"command":"true"}), false, true, true);
    events[12]["data"]
        .as_object_mut()
        .unwrap()
        .remove("content");
    let (evidence, joiner) = observe(&events, 0);
    assert!(joiner.gaps().is_empty());
    let calls = calls(&evidence);
    assert_eq!(calls.len(), 2);
    assert!(matches!(&calls[1].payload, PublicMessagePayload::ToolCall {
        provider_executed: Some(true), facts: Some(facts), ..
    } if facts.category == Some(CheckToolCategory::Other)));
    let results: Vec<_> = evidence
        .iter()
        .filter_map(|event| match &event.fact {
            NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(result)) => {
                Some(result)
            }
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, None);
    assert_eq!(results[0].provider_executed, Some(true));
    assert!(
        !evidence
            .iter()
            .any(|event| matches!(event.fact, NativeEvidenceFact::ResponseCycle(_)))
    );
}

#[test]
fn terminal_execution_downgrades_remain_unsafe_in_public_evidence() {
    for failed in [false, true] {
        let events = tool_fixture("shell", json!({"command":"true"}), true, false, failed);
        let mut joiner = OpenCodeV2EventJoiner::new();
        joiner.set_live_baseline(0);
        let mut evidence = Vec::new();
        for event in &events {
            let batch = joiner.observe_evidence(event);
            if event["id"] == "tool-result-1" {
                assert!(!batch.history_complete);
                assert!(
                    !batch
                        .events
                        .iter()
                        .any(|event| matches!(event.fact, NativeEvidenceFact::Message(_)))
                );
            }
            evidence.extend(batch.events);
        }
        assert!(joiner.gaps().contains(&Gap::ConflictingDuplicate));
        assert_eq!(calls(&evidence).len(), 1);
        assert!(!evidence.iter().any(|event| matches!(
            event.fact,
            NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(_))
                | NativeEvidenceFact::ResponseCycle(_)
        )));
    }
}
