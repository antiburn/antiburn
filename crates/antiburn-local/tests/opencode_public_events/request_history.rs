use super::*;
use antiburn_local::analysis::{ResponseCycleObservation, UserMessageCompleteness};

fn execution(index: u64, sequence: &mut u64, text: &str) -> Vec<Value> {
    let fixture = v2_fixture();
    fixture[..14]
        .iter()
        .chain(fixture.last())
        .cloned()
        .map(|mut event| {
            *sequence += 1;
            event["id"] = json!(format!("{index}-{}", event["id"].as_str().unwrap()));
            event["durable"]["seq"] = json!(*sequence);
            for field in ["inboxID", "assistantMessageID", "id"] {
                if let Some(id) = event["data"][field].as_str() {
                    event["data"][field] = json!(format!("{index}-{id}"));
                }
            }
            if event["type"] == "session.inbox.enqueued" {
                event["data"]["item"]["payload"]["text"] = json!(text);
            }
            event
        })
        .collect()
}

fn cycles(facts: &[NativeEvidenceFact]) -> Vec<&ResponseCycleObservation> {
    facts
        .iter()
        .filter_map(|fact| match fact {
            NativeEvidenceFact::ResponseCycle(cycle) => Some(cycle),
            _ => None,
        })
        .collect()
}

fn request_texts(cycle: &ResponseCycleObservation) -> Vec<&str> {
    cycle
        .context
        .user_messages
        .iter()
        .map(|message| message.parts[0].text.as_str())
        .collect()
}

#[test]
fn v2_second_execution_keeps_session_requests_separate_from_turn_members() {
    let mut sequence = 0;
    let mut events = execution(1, &mut sequence, "Do not change generated files.");
    events.extend(execution(2, &mut sequence, "Read the synthetic file."));
    let (facts, joiner) = v2_facts(&events, Some(0));
    assert!(joiner.gaps().is_empty());
    let cycles = cycles(&facts);
    assert_eq!(cycles.len(), 2);
    assert_eq!(request_texts(cycles[0]), ["Do not change generated files."]);
    assert_eq!(
        request_texts(cycles[1]),
        ["Do not change generated files.", "Read the synthetic file."]
    );
    assert!(
        cycles
            .iter()
            .all(|cycle| cycle.context.complete_through_event)
    );
    assert_eq!(
        cycles[1].context.first_eligible_position,
        cycles[1].position
    );
    assert_eq!(cycles[1].messages.len(), 4);
    for message in &cycles[1].messages {
        match message {
            NativeMessageObservation::User(user) => {
                assert_eq!(user.message.message_id, "2-input-1");
            }
            NativeMessageObservation::Public(public) => {
                assert_eq!(public.record_id, "2-assistant-1");
                if let Some(call) = &public.call_id {
                    assert_eq!(call, "2-call-1");
                }
            }
            NativeMessageObservation::ToolResult(result) => {
                assert_eq!(result.record_id.as_deref(), Some("2-assistant-1"));
                assert_eq!(result.call_id.as_deref(), Some("2-call-1"));
            }
        }
    }
}

#[test]
fn v2_partial_history_stays_partial_after_a_new_complete_request() {
    let mut sequence = 0;
    let mut events = execution(1, &mut sequence, "DISCARDED_EARLIER_REQUEST");
    let mut partial = execution(9, &mut sequence, "DISCARDED_PARTIAL_REQUEST");
    partial[0]["data"]["item"]["payload"]["files"] = json!([{"path":"synthetic.png"}]);
    events.extend([partial[0].clone(), partial[2].clone()]);
    events.extend(execution(2, &mut sequence, "Read the synthetic file."));
    events.extend(execution(3, &mut sequence, "Check the result."));
    let (facts, joiner) = v2_facts(&events, Some(0));
    assert!(joiner.gaps().is_empty());
    let cycles = cycles(&facts);
    assert_eq!(cycles.len(), 3);
    assert!(cycles[0].context.complete_through_event);
    for cycle in &cycles[1..] {
        assert!(!cycle.context.complete_through_event);
        assert!(
            cycle
                .context
                .user_messages
                .iter()
                .all(|message| { message.completeness == UserMessageCompleteness::Complete })
        );
        let encoded = serde_json::to_string(&cycle.context).unwrap();
        assert!(!encoded.contains("DISCARDED_EARLIER_REQUEST"));
        assert!(!encoded.contains("DISCARDED_PARTIAL_REQUEST"));
    }
    assert_eq!(request_texts(cycles[1]), ["Read the synthetic file."]);
    assert_eq!(
        request_texts(cycles[2]),
        ["Read the synthetic file.", "Check the result."]
    );
}

#[test]
fn v2_missing_history_is_not_complete_after_later_valid_executions() {
    let mut sequence = 0;
    let mut missing = execution(9, &mut sequence, "Missing request.");
    let mut events = vec![missing.remove(2)];
    events.extend(execution(1, &mut sequence, "First known request."));
    events.extend(execution(2, &mut sequence, "Second known request."));
    let (facts, joiner) = v2_facts(&events, Some(0));
    assert_eq!(joiner.gaps(), [Gap::MissingJoin]);
    let cycles = cycles(&facts);
    assert_eq!(cycles.len(), 2);
    assert!(
        cycles
            .iter()
            .all(|cycle| !cycle.context.complete_through_event)
    );
    assert_eq!(
        request_texts(cycles[1]),
        ["First known request.", "Second known request."]
    );
}

#[test]
fn v2_bounded_request_prefix_does_not_make_an_omitted_identity_complete() {
    let mut sequence = 0;
    let mut events = execution(1, &mut sequence, "UNAVAILABLE_REQUEST_CANARY");
    let oversized_id = "x".repeat(antiburn_local::analysis::EVIDENCE_STRING_CAP + 1);
    events[0]["data"]["inboxID"] = json!(oversized_id);
    events[2]["data"]["inboxID"] = events[0]["data"]["inboxID"].clone();
    events.extend(execution(2, &mut sequence, "Read the synthetic file."));
    let (facts, joiner) = v2_facts(&events, Some(0));
    assert!(joiner.gaps().is_empty());
    let cycles = cycles(&facts);
    assert_eq!(cycles.len(), 1);
    assert!(!cycles[0].context.complete_through_event);
    assert_eq!(request_texts(cycles[0]), ["Read the synthetic file."]);
    assert!(
        !serde_json::to_string(&cycles[0].context)
            .unwrap()
            .contains("UNAVAILABLE_REQUEST_CANARY")
    );
}

#[test]
fn v2_request_history_eviction_does_not_restore_old_text_or_complete_a_cycle() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    joiner.set_live_baseline(0);
    let mut sequence = 0;
    for index in 1..=40 {
        let label = if index == 1 {
            "EVICTED_REQUEST_CANARY".to_owned()
        } else {
            format!("Request {index}.")
        };
        let text = format!(
            "{label}{}",
            "x".repeat(
                antiburn_local::analysis::opencode_v2_events::MAX_STRING_BYTES - label.len()
            )
        );
        let events = execution(index, &mut sequence, &text);
        joiner.observe_evidence(&events[0]);
        joiner.observe_evidence(&events[2]);
    }
    assert!(joiner.gaps().contains(&Gap::CapacityExceeded));
    let mut facts = Vec::new();
    for event in execution(41, &mut sequence, "Read the synthetic file.") {
        facts.extend(
            joiner
                .observe_evidence(&event)
                .events
                .into_iter()
                .map(|event| event.fact),
        );
    }
    assert!(cycles(&facts).is_empty());
    assert!(
        !serde_json::to_string(&facts)
            .unwrap()
            .contains("EVICTED_REQUEST_CANARY")
    );
}

#[test]
fn v2_removed_or_reverted_history_does_not_restore_a_cycle() {
    for kind in [
        "session.message.content.updated",
        "session.revert.committed",
        "session.deleted",
    ] {
        let mut sequence = 0;
        let mut events = execution(1, &mut sequence, "REMOVED_REQUEST_CANARY");
        sequence += 1;
        events.push(json!({
            "id":"history-removed", "type":kind,
            "durable":{"aggregateID":"root", "seq":sequence, "version":if kind == "session.deleted" { 2 } else { 1 }},
            "data":{"sessionID":"root"}
        }));
        events.extend(execution(2, &mut sequence, "Read the synthetic file."));
        let (facts, joiner) = v2_facts(&events, Some(0));
        assert!(joiner.gaps().contains(&Gap::ContinuityLost), "{kind}");
        assert_eq!(cycles(&facts).len(), 1, "{kind}");
    }
}
