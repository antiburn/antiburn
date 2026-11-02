use super::*;
use antiburn_local::analysis::CheckToolCategory;
use antiburn_local::analysis::opencode_v2_capture::{BoundaryKind, CapturedEvent};

#[test]
fn captured_and_raw_public_evidence_match_after_terminal_cleanup() {
    for failed in [false, true] {
        let mut events = v2_fixture();
        events[10]["data"]["name"] = json!("write");
        events[11]["data"]["input"] = json!({"path":"synthetic.rs","content":"new"});
        events[12]["data"]["executed"] = json!(true);
        if failed {
            events[12]["type"] = json!("session.tool.failed");
            events[12]["data"]["error"] =
                json!({"type":"tool.execution","message":"Synthetic failure"});
        }
        let mut raw = OpenCodeV2EventJoiner::new();
        let mut captured = OpenCodeV2EventJoiner::new();
        raw.set_live_baseline(0);
        captured.set_live_baseline(0);
        let mut revised = None;
        for event in events {
            let envelope = CapturedEvent::new(event.clone(), BoundaryKind::Live).unwrap();
            let raw_batch = raw.observe_evidence(&event);
            let batch = captured.observe_captured_evidence(&envelope);
            assert_eq!(raw_batch, batch);
            assert!(raw.gaps().is_empty());
            assert!(captured.gaps().is_empty());
            if event["id"] == "tool-result-1" {
                revised = batch.events.iter().find_map(|event| match &event.fact {
                    NativeEvidenceFact::Message(NativeMessageObservation::Public(call)) => {
                        Some(call.clone())
                    }
                    _ => None,
                });
                assert!(matches!(&revised.as_ref().unwrap().payload,
                    PublicMessagePayload::ToolCall { facts: Some(facts), provider_executed: Some(true), .. }
                    if facts.category == Some(CheckToolCategory::Other)));
            }
            if event["id"] == "step-end-2" {
                assert!(captured.has_pending_work());
            }
            if event["id"] == "execution-end-1" {
                let cycle = batch
                    .events
                    .iter()
                    .find_map(|event| match &event.fact {
                        NativeEvidenceFact::ResponseCycle(cycle) => Some(cycle),
                        _ => None,
                    })
                    .unwrap();
                assert!(
                    cycle
                        .messages
                        .contains(&NativeMessageObservation::Public(revised.clone().unwrap()))
                );
                assert!(!captured.has_pending_work());
            }
            assert!(raw.observe_evidence(&event).events.is_empty());
            assert!(
                captured
                    .observe_captured_evidence(&envelope)
                    .events
                    .is_empty()
            );
        }
    }
}

#[test]
fn completed_public_steps_and_calls_do_not_exhaust_active_limits() {
    let fixture = v2_fixture();
    let mut joiner = OpenCodeV2EventJoiner::new();
    joiner.set_live_baseline(0);
    for index in [0, 2] {
        joiner.observe_evidence(&fixture[index]);
    }
    let mut sequence = 3;
    for index in 0..300 {
        for slot in [3, 10, 11, 12, 13] {
            sequence += 1;
            let mut event = fixture[slot].clone();
            event["id"] = json!(format!("event-{sequence}"));
            event["durable"]["seq"] = json!(sequence);
            event["data"]["assistantMessageID"] = json!(format!("assistant-{index}"));
            if matches!(slot, 10..=12) {
                event["data"]["id"] = json!(format!("call-{index}"));
            }
            let batch = joiner.observe_evidence(&event);
            assert!(joiner.gaps().is_empty(), "step={index} slot={slot}");
            if slot == 11 {
                assert!(batch.events.iter().any(|event| matches!(&event.fact,
                    NativeEvidenceFact::Message(NativeMessageObservation::Public(call)) if call.eligible)));
            }
            if slot == 12 {
                assert_eq!(
                    batch
                        .events
                        .iter()
                        .filter(|event| matches!(
                            event.fact,
                            NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(_))
                        ))
                        .count(),
                    1
                );
            }
        }
        assert!(!joiner.has_pending_work());
    }
}

#[test]
fn oversized_raw_input_cannot_replay_the_previous_evidence_batch() {
    use antiburn_local::analysis::opencode_v2_capture::MAX_CAPTURE_EVENT_BYTES;
    let mut joiner = OpenCodeV2EventJoiner::new();
    joiner.set_live_baseline(0);
    for event in &v2_fixture()[..12] {
        joiner.observe(event);
    }
    let mut oversized = v2_fixture()[12].clone();
    oversized["data"]["content"][0]["text"] = json!("\0".repeat(MAX_CAPTURE_EVENT_BYTES));
    assert!(joiner.observe_evidence(&oversized).events.is_empty());
    assert!(joiner.gaps().contains(&Gap::CapacityExceeded));
}
