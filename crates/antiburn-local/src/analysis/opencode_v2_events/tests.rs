use super::*;
use serde_json::json;
use std::collections::HashSet;

fn event(sequence: u64, kind: &str, data: Value) -> Value {
    let mut data = data.as_object().unwrap().clone();
    data.insert("sessionID".into(), json!("session"));
    json!({
        "id": format!("event-{sequence}"), "type": kind,
        "durable": {"aggregateID": "session", "seq": sequence,
            "version": event_schema_version(kind).unwrap()},
        "data": data,
    })
}

fn observe(
    joiner: &mut OpenCodeV2EventJoiner,
    sequence: &mut u64,
    kind: &str,
    data: Value,
) -> Option<JoinedToolResult> {
    *sequence += 1;
    joiner.observe(&event(*sequence, kind, data))
}

fn request(joiner: &mut OpenCodeV2EventJoiner, sequence: &mut u64, id: &str, text: &str) {
    observe(
        joiner,
        sequence,
        "session.inbox.enqueued",
        json!({
            "inboxID": id, "item": {"type": "user", "payload": {"text": text}}
        }),
    );
    observe(
        joiner,
        sequence,
        "session.inbox.delivered",
        json!({"inboxID": id}),
    );
}

fn start_call(joiner: &mut OpenCodeV2EventJoiner, sequence: &mut u64, message: &str, call: &str) {
    observe(
        joiner,
        sequence,
        "session.tool.input.started",
        json!({
            "assistantMessageID": message, "id": call, "name": "read"
        }),
    );
    observe(
        joiner,
        sequence,
        "session.tool.called",
        json!({
            "assistantMessageID": message, "id": call, "input": {"path": "sample.rs"}, "executed": false
        }),
    );
}

fn finish_call(
    joiner: &mut OpenCodeV2EventJoiner,
    sequence: &mut u64,
    message: &str,
    call: &str,
) -> JoinedToolResult {
    observe(joiner, sequence, "session.tool.success", json!({
        "assistantMessageID": message, "id": call, "content": [{"type": "text", "text": "sample"}]
    })).unwrap()
}

fn execution_terminals() -> [(&'static str, Value); 3] {
    [
        ("session.execution.succeeded", json!({})),
        (
            "session.execution.failed",
            json!({"error": {"type": "aborted", "message": "Execution failed"}}),
        ),
        ("session.execution.interrupted", json!({"reason": "user"})),
    ]
}

fn assert_accounting(joiner: &OpenCodeV2EventJoiner) {
    assert_eq!(
        joiner.terminal_call_order.len(),
        joiner
            .calls
            .values()
            .filter(|call| call.terminal.is_some())
            .count()
    );
    for call in joiner.calls.values() {
        assert_eq!(
            call.input_bytes,
            call.input.as_ref().map_or(0, json_payload_bytes)
        );
    }
    let mut expected = joiner.session_id.as_ref().map_or(0, String::len)
        + joiner
            .seen_event_ids
            .keys()
            .map(|id| id.len() + 32)
            .sum::<usize>()
        + joiner
            .seen_event_id_order
            .iter()
            .map(String::len)
            .sum::<usize>()
        + joiner
            .pending_inbox
            .iter()
            .map(|(id, pending)| id.len() + pending.payload_bytes())
            .sum::<usize>()
        + joiner
            .steps
            .iter()
            .map(|(id, step)| id.len() + step.context.requester.retained_bytes())
            .sum::<usize>()
        + joiner
            .calls
            .iter()
            .map(|((message, call), value)| message.len() + call.len() + value.payload_bytes())
            .sum::<usize>()
        + joiner
            .terminal_call_order
            .iter()
            .map(|(message, call)| message.len() + call.len())
            .sum::<usize>()
        + joiner.public.retained_bytes();
    let mut histories = HashSet::new();
    for root in std::iter::once(&joiner.users)
        .chain(joiner.steps.values().map(|step| &step.context.users))
        .chain(joiner.calls.values().map(|call| &call.context.users))
    {
        let mut history = root.as_ref();
        while let Some(node) = history {
            if !histories.insert(Arc::as_ptr(node)) {
                break;
            }
            expected += node.request.payload_bytes();
            history = node.previous.as_ref();
        }
    }
    assert_eq!(joiner.retained_payload_bytes(), expected);
    assert_eq!(
        joiner.seen_event_ids.len(),
        joiner.seen_event_id_order.len()
    );
}

#[test]
fn pending_work_tracks_open_steps_without_retaining_terminal_only_sessions() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let mut sequence = 0;
    assert!(!joiner.has_pending_work());
    request(&mut joiner, &mut sequence, "user", "request");
    assert!(!joiner.has_pending_work());
    observe(
        &mut joiner,
        &mut sequence,
        "session.step.started",
        json!({"assistantMessageID": "step"}),
    );
    assert!(joiner.has_pending_work());
    start_call(&mut joiner, &mut sequence, "step", "call");
    assert!(joiner.has_pending_work());
    finish_call(&mut joiner, &mut sequence, "step", "call");
    assert!(joiner.has_pending_work());
    observe(
        &mut joiner,
        &mut sequence,
        "session.step.ended",
        json!({"assistantMessageID": "step"}),
    );
    assert!(!joiner.has_pending_work());
    assert_eq!(joiner.calls.len(), 1);
    assert_eq!(joiner.terminal_call_order.len(), 1);
    assert_accounting(&joiner);
}

#[test]
fn ended_steps_remain_pending_until_the_last_call_finishes() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let mut sequence = 0;
    request(&mut joiner, &mut sequence, "user", "request");
    observe(
        &mut joiner,
        &mut sequence,
        "session.step.started",
        json!({"assistantMessageID": "step"}),
    );
    start_call(&mut joiner, &mut sequence, "step", "first");
    start_call(&mut joiner, &mut sequence, "step", "last");
    observe(
        &mut joiner,
        &mut sequence,
        "session.step.ended",
        json!({"assistantMessageID": "step"}),
    );
    assert!(joiner.has_pending_work());
    finish_call(&mut joiner, &mut sequence, "step", "first");
    assert!(joiner.has_pending_work());
    finish_call(&mut joiner, &mut sequence, "step", "last");
    assert!(!joiner.has_pending_work());
    assert_accounting(&joiner);
}

#[test]
fn execution_terminals_settle_orphan_retry_steps_and_allow_later_safe_work() {
    for (kind, data) in execution_terminals() {
        let mut joiner = OpenCodeV2EventJoiner::new();
        let mut sequence = 0;
        request(&mut joiner, &mut sequence, "user", "request");
        observe(
            &mut joiner,
            &mut sequence,
            "session.execution.started",
            json!({}),
        );
        observe(
            &mut joiner,
            &mut sequence,
            "session.step.started",
            json!({"assistantMessageID": "retry-step"}),
        );
        observe(
            &mut joiner,
            &mut sequence,
            "session.retry.scheduled",
            json!({
                "assistantMessageID": "retry-step", "attempt": 2, "at": 1000,
                "error": {"type": "aborted", "message": "Request failed"}
            }),
        );
        assert!(joiner.has_pending_work());
        assert!(observe(&mut joiner, &mut sequence, kind, data).is_none());
        assert!(!joiner.has_pending_work(), "{kind}");
        assert!(joiner.steps.is_empty());
        assert!(joiner.calls.is_empty());
        assert!(joiner.gaps().is_empty());
        assert_accounting(&joiner);

        observe(
            &mut joiner,
            &mut sequence,
            "session.execution.started",
            json!({}),
        );
        observe(
            &mut joiner,
            &mut sequence,
            "session.step.started",
            json!({"assistantMessageID": "next-step"}),
        );
        start_call(&mut joiner, &mut sequence, "next-step", "next-call");
        let result = finish_call(&mut joiner, &mut sequence, "next-step", "next-call");
        assert_eq!(result.request_message_id.as_deref(), Some("user"));
        assert_eq!(result.request_text.as_deref(), Some("request"));
        assert!(result.gaps.is_empty());
        assert_eq!(
            result.into_observation(true, true).completeness,
            ToolResultCompleteness::Complete
        );
        observe(
            &mut joiner,
            &mut sequence,
            "session.step.ended",
            json!({"assistantMessageID": "next-step"}),
        );
        assert!(!joiner.has_pending_work());
        assert_accounting(&joiner);
    }
}

#[test]
fn execution_terminals_drop_pending_calls_but_keep_completed_fingerprints_and_inbox() {
    for (kind, data) in execution_terminals() {
        let mut joiner = OpenCodeV2EventJoiner::new();
        let mut sequence = 0;
        request(&mut joiner, &mut sequence, "user", "request");
        observe(
            &mut joiner,
            &mut sequence,
            "session.step.started",
            json!({"assistantMessageID": "step"}),
        );
        start_call(&mut joiner, &mut sequence, "step", "completed");
        let accepted = finish_call(&mut joiner, &mut sequence, "step", "completed");
        start_call(&mut joiner, &mut sequence, "step", "pending");
        observe(
            &mut joiner,
            &mut sequence,
            "session.tool.input.started",
            json!({"assistantMessageID": "step", "id": "partial", "name": "read"}),
        );
        observe(
            &mut joiner,
            &mut sequence,
            "session.step.ended",
            json!({"assistantMessageID": "step"}),
        );
        observe(
            &mut joiner,
            &mut sequence,
            "session.inbox.enqueued",
            json!({
                "inboxID": "queued", "item": {"type": "user", "payload": {"text": "next request"}}
            }),
        );
        assert_accounting(&joiner);
        assert!(observe(&mut joiner, &mut sequence, kind, data.clone()).is_none());
        assert!(!joiner.has_pending_work(), "{kind}");
        assert_eq!(joiner.calls.len(), 1);
        assert_eq!(joiner.terminal_call_order.len(), 1);
        assert!(joiner.pending_inbox.contains_key("queued"));
        assert_accounting(&joiner);

        let terminal = json!({
            "assistantMessageID": "step", "id": "completed",
            "content": [{"type": "text", "text": "sample"}]
        });
        assert!(
            observe(
                &mut joiner,
                &mut sequence,
                "session.tool.success",
                terminal.clone()
            )
            .is_none()
        );
        assert!(joiner.gaps().is_empty());
        observe(
            &mut joiner,
            &mut sequence,
            "session.inbox.delivered",
            json!({"inboxID": "queued"}),
        );
        observe(
            &mut joiner,
            &mut sequence,
            "session.step.started",
            json!({"assistantMessageID": "next-step"}),
        );
        start_call(&mut joiner, &mut sequence, "next-step", "next-call");
        let next = finish_call(&mut joiner, &mut sequence, "next-step", "next-call");
        assert_eq!(next.request_message_id.as_deref(), Some("queued"));
        assert_eq!(next.request_text.as_deref(), Some("next request"));
        assert!(next.gaps.is_empty());
        assert_eq!(
            next.into_observation(true, true).completeness,
            ToolResultCompleteness::Complete
        );
        assert!(observe(&mut joiner, &mut sequence, kind, data).is_none());
        assert!(!joiner.has_pending_work());
        for call in ["pending", "partial"] {
            assert!(
                observe(
                    &mut joiner,
                    &mut sequence,
                    "session.tool.success",
                    json!({
                        "assistantMessageID": "step", "id": call,
                        "content": [{"type": "text", "text": "late result"}]
                    })
                )
                .is_none()
            );
        }
        assert!(joiner.gaps().contains(&Gap::MissingJoin));
        let mut correction = terminal;
        correction["content"][0]["text"] = json!("changed");
        assert!(
            observe(
                &mut joiner,
                &mut sequence,
                "session.tool.success",
                correction
            )
            .is_none()
        );
        assert!(joiner.gaps().contains(&Gap::ConflictingDuplicate));
        assert_eq!(accepted.content.as_deref(), Some("sample"));
        assert!(accepted.gaps.is_empty());
        assert!(!joiner.has_pending_work());
        assert_accounting(&joiner);
    }
}

#[test]
fn execution_terminal_replays_cannot_settle_later_pending_work() {
    for (kind, data) in execution_terminals() {
        let mut joiner = OpenCodeV2EventJoiner::new();
        let mut sequence = 0;
        request(&mut joiner, &mut sequence, "user", "request");
        sequence += 1;
        let terminal = event(sequence, kind, data);
        assert!(joiner.observe(&terminal).is_none());
        observe(
            &mut joiner,
            &mut sequence,
            "session.step.started",
            json!({"assistantMessageID": "next-step"}),
        );
        start_call(&mut joiner, &mut sequence, "next-step", "next-call");
        assert!(joiner.observe(&terminal).is_none());
        assert!(joiner.has_pending_work());
        assert!(joiner.gaps().is_empty());

        let mut conflict = terminal.clone();
        conflict["created"] = json!(1000);
        assert!(joiner.observe(&conflict).is_none());
        assert!(joiner.has_pending_work());
        assert!(joiner.gaps().contains(&Gap::ConflictingDuplicate));
        for _ in 0..MAX_RETAINED_CALLS * 8 {
            observe(
                &mut joiner,
                &mut sequence,
                "session.viewed",
                json!({"idle": 0}),
            );
        }
        assert!(
            !joiner
                .seen_event_ids
                .contains_key(terminal["id"].as_str().unwrap())
        );
        let watermark = joiner.last_sequence;
        assert!(joiner.observe(&terminal).is_none());
        assert_eq!(joiner.last_sequence, watermark);
        assert!(joiner.has_pending_work());
        let result = finish_call(&mut joiner, &mut sequence, "next-step", "next-call");
        assert!(result.gaps.contains(&Gap::ConflictingDuplicate));
        assert!(result.gaps.contains(&Gap::SequenceDiscontinuity));
        assert!(joiner.seen_event_ids.len() <= MAX_RETAINED_CALLS * 8);
        assert_accounting(&joiner);
    }
}

#[test]
fn execution_terminals_require_safe_envelopes_before_settlement() {
    for (kind, data) in execution_terminals() {
        for invalid in ["version", "sequence", "aggregate", "session"] {
            let mut joiner = OpenCodeV2EventJoiner::new();
            let mut sequence = 0;
            request(&mut joiner, &mut sequence, "user", "request");
            observe(
                &mut joiner,
                &mut sequence,
                "session.step.started",
                json!({"assistantMessageID": "step"}),
            );
            start_call(&mut joiner, &mut sequence, "step", "call");
            sequence += 1;
            let mut terminal = event(sequence, kind, data.clone());
            let expected_gap = match invalid {
                "version" => {
                    terminal["durable"]["version"] = json!(2);
                    Gap::UnsupportedVersion
                }
                "sequence" => {
                    terminal["durable"]["seq"] = Value::Null;
                    Gap::MissingSequence
                }
                "aggregate" => {
                    terminal["durable"]["aggregateID"] = json!("other");
                    Gap::UnknownShape
                }
                "session" => {
                    terminal["data"]["sessionID"] = json!("other");
                    Gap::UnknownShape
                }
                _ => unreachable!(),
            };
            assert!(joiner.observe(&terminal).is_none());
            assert!(joiner.has_pending_work(), "{kind}: {invalid}");
            assert!(joiner.gaps().contains(&expected_gap));
            let result = finish_call(&mut joiner, &mut sequence, "step", "call");
            assert!(result.gaps.contains(&expected_gap));
            assert_accounting(&joiner);
        }
    }
}

#[test]
fn execution_terminals_preserve_gaps_and_release_old_pending_history() {
    for (kind, data) in execution_terminals() {
        let mut joiner = OpenCodeV2EventJoiner::new();
        let mut sequence = 0;
        request(&mut joiner, &mut sequence, "old", "old request");
        observe(
            &mut joiner,
            &mut sequence,
            "session.step.started",
            json!({"assistantMessageID": "old-step"}),
        );
        start_call(&mut joiner, &mut sequence, "old-step", "pending");
        observe(
            &mut joiner,
            &mut sequence,
            "session.inbox.delivered",
            json!({"inboxID": "missed"}),
        );
        request(&mut joiner, &mut sequence, "new", "new request");
        joiner.mark_gap();
        let old_bytes = "old".len() + "old request".len();
        let new_bytes = "new".len() + "new request".len();
        assert_eq!(
            joiner.history_bytes.load(Ordering::Relaxed),
            old_bytes + new_bytes
        );
        assert!(observe(&mut joiner, &mut sequence, kind, data).is_none());
        assert_eq!(joiner.history_bytes.load(Ordering::Relaxed), new_bytes);
        assert!(!joiner.has_pending_work());
        assert!(joiner.gaps().contains(&Gap::ContinuityLost));
        assert!(joiner.gaps().contains(&Gap::MissingJoin));
        assert_accounting(&joiner);
        observe(
            &mut joiner,
            &mut sequence,
            "session.step.started",
            json!({"assistantMessageID": "new-step"}),
        );
        start_call(&mut joiner, &mut sequence, "new-step", "new-call");
        let result = finish_call(&mut joiner, &mut sequence, "new-step", "new-call");
        assert_eq!(result.request_message_id.as_deref(), Some("new"));
        assert!(result.gaps.contains(&Gap::ContinuityLost));
        assert_eq!(
            result.into_observation(true, true).completeness,
            ToolResultCompleteness::Incomplete
        );
        assert_accounting(&joiner);
    }
}

#[test]
fn calls_without_a_step_keep_work_pending_and_capacity_reset_releases_it() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let mut sequence = 0;
    start_call(&mut joiner, &mut sequence, "missing-step", "first");
    assert!(joiner.has_pending_work());
    finish_call(&mut joiner, &mut sequence, "missing-step", "first");
    assert!(!joiner.has_pending_work());
    start_call(&mut joiner, &mut sequence, "missing-step", "pending");
    assert!(joiner.has_pending_work());
    for index in 0..40 {
        observe(
            &mut joiner,
            &mut sequence,
            "session.inbox.enqueued",
            json!({"inboxID":format!("user-{index}"),
                "item":{"type":"user","payload":{"text":"x".repeat(MAX_STRING_BYTES)}}}),
        );
        if joiner.gaps().contains(&Gap::CapacityExceeded) {
            break;
        }
        assert!(joiner.has_pending_work());
    }
    assert!(joiner.gaps().contains(&Gap::CapacityExceeded));
    assert!(!joiner.has_pending_work());
    assert!(joiner.calls.len() <= MAX_RETAINED_CALLS);
    assert!(joiner.retained_payload_bytes() <= MAX_RETAINED_PAYLOAD_BYTES);
    assert_accounting(&joiner);
}

#[test]
fn steps_and_pending_calls_share_each_user_payload_once() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let mut sequence = 0;
    for index in 0..7 {
        request(
            &mut joiner,
            &mut sequence,
            &format!("user-{index}"),
            &"x".repeat(256),
        );
    }
    let history_bytes = joiner.history_bytes.load(Ordering::Relaxed);
    for index in 0..16 {
        let message = format!("assistant-{index}");
        observe(
            &mut joiner,
            &mut sequence,
            "session.step.started",
            json!({"assistantMessageID": message}),
        );
        start_call(&mut joiner, &mut sequence, &message, "call");
        assert_accounting(&joiner);
        assert_eq!(joiner.history_bytes.load(Ordering::Relaxed), history_bytes);
        assert!(Arc::ptr_eq(
            joiner.steps[&message].context.users.as_ref().unwrap(),
            joiner.users.as_ref().unwrap()
        ));
        assert!(joiner.gaps().is_empty());
    }
    for index in 0..16 {
        let message = format!("assistant-{index}");
        // A step can end before its last pending tool finishes.
        observe(
            &mut joiner,
            &mut sequence,
            "session.step.ended",
            json!({"assistantMessageID": message}),
        );
        assert!(joiner.steps.contains_key(&message));
        let result = finish_call(&mut joiner, &mut sequence, &message, "call");
        assert_eq!(result.user_messages.len(), 7);
        assert!(!joiner.steps.contains_key(&message));
        assert_accounting(&joiner);
    }
    assert!(joiner.gaps().is_empty());
}

#[test]
fn public_steps_and_calls_share_nine_full_request_prefixes() {
    use crate::analysis::{NativeEvidenceFact, NativeMessageObservation, PublicMessagePayload};

    let mut joiner = OpenCodeV2EventJoiner::new();
    let mut sequence = 0;
    for index in 0..9 {
        request(
            &mut joiner,
            &mut sequence,
            &format!("user-{index}"),
            &"x".repeat(MAX_STRING_BYTES),
        );
    }
    joiner.set_live_baseline(sequence);
    observe(
        &mut joiner,
        &mut sequence,
        "session.step.started",
        json!({"assistantMessageID": "step"}),
    );
    start_call(&mut joiner, &mut sequence, "step", "call");
    assert!(joiner.gaps().is_empty(), "{:?}", joiner.gaps());
    let first_call_position = ToolResultPosition::OpenCodeV2Event {
        sequence,
        item_index: 0,
    };
    let call = joiner.evidence.iter().find_map(|event| match &event.fact {
        NativeEvidenceFact::Message(NativeMessageObservation::Public(message))
            if matches!(message.payload, PublicMessagePayload::ToolCall { .. }) =>
        {
            assert!(event.live_eligible);
            Some(message)
        }
        _ => None,
    });
    let context = call.unwrap().context.as_ref().unwrap();
    assert_eq!(context.first_eligible_position, first_call_position);
    assert_eq!(context.user_messages.len(), 9);
    assert!(context.complete_through_event);
    assert!(
        context
            .user_messages
            .iter()
            .all(|message| message.parts[0].text.len() == MAX_STRING_BYTES)
    );
    assert_accounting(&joiner);

    request(&mut joiner, &mut sequence, "later-user", "later request");
    observe(
        &mut joiner,
        &mut sequence,
        "session.tool.called",
        json!({
            "assistantMessageID": "step", "id": "call",
            "input": {"path": "changed.rs"}, "executed": false
        }),
    );
    let correction = joiner.evidence.iter().find_map(|event| match &event.fact {
        NativeEvidenceFact::Message(NativeMessageObservation::Public(message)) => Some(message),
        _ => None,
    });
    let context = correction.unwrap().context.as_ref().unwrap();
    assert_eq!(context.first_eligible_position, first_call_position);
    assert_eq!(context.user_messages.len(), 9);

    let result = finish_call(&mut joiner, &mut sequence, "step", "call");
    assert_eq!(result.user_messages.len(), 9);
    assert!(result.gaps.is_empty());
    assert_eq!(
        result.into_observation(true, true).completeness,
        ToolResultCompleteness::Complete
    );
    for (kind, data) in [
        (
            "session.text.started",
            json!({"assistantMessageID": "step", "ordinal": 0}),
        ),
        (
            "session.text.ended",
            json!({"assistantMessageID": "step", "ordinal": 0, "text": "Done"}),
        ),
        (
            "session.step.ended",
            json!({"assistantMessageID": "step", "finish": "stop"}),
        ),
    ] {
        observe(&mut joiner, &mut sequence, kind, data);
    }
    let assistant = joiner.evidence.iter().find_map(|event| match &event.fact {
        NativeEvidenceFact::Message(NativeMessageObservation::Public(message)) => Some(message),
        _ => None,
    });
    assert_eq!(
        assistant
            .unwrap()
            .context
            .as_ref()
            .unwrap()
            .user_messages
            .len(),
        9
    );
    assert!(joiner.gaps().is_empty());
    assert!(joiner.retained_payload_bytes() <= MAX_RETAINED_PAYLOAD_BYTES);
    assert_accounting(&joiner);
}

#[test]
fn public_state_overflow_keeps_a_gap_on_later_evidence() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let mut sequence = 0;
    request(&mut joiner, &mut sequence, "user", "request");
    joiner.set_live_baseline(sequence);
    observe(
        &mut joiner,
        &mut sequence,
        "session.step.started",
        json!({"assistantMessageID": "step"}),
    );
    for ordinal in 0..17 {
        observe(
            &mut joiner,
            &mut sequence,
            "session.text.started",
            json!({"assistantMessageID": "step", "ordinal": ordinal}),
        );
        observe(
            &mut joiner,
            &mut sequence,
            "session.text.ended",
            json!({
                "assistantMessageID": "step", "ordinal": ordinal,
                "text": "x".repeat(MAX_STRING_BYTES)
            }),
        );
        if joiner.gaps().contains(&Gap::CapacityExceeded) {
            break;
        }
    }
    assert!(joiner.gaps().contains(&Gap::CapacityExceeded));
    assert!(joiner.retained_payload_bytes() <= MAX_RETAINED_PAYLOAD_BYTES);
    assert_accounting(&joiner);

    start_call(&mut joiner, &mut sequence, "step", "call");
    let result = finish_call(&mut joiner, &mut sequence, "step", "call");
    assert!(result.gaps.contains(&Gap::CapacityExceeded));
    assert!(joiner.evidence.iter().all(|event| !event.live_eligible));
    assert_eq!(
        result.into_observation(true, true).completeness,
        ToolResultCompleteness::Incomplete
    );
    assert_accounting(&joiner);
}

#[test]
fn public_output_overflow_still_omits_oversized_request_evidence() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let mut sequence = 0;
    for index in 0..14 {
        request(
            &mut joiner,
            &mut sequence,
            &format!("user-{index}"),
            &"x".repeat(MAX_STRING_BYTES),
        );
    }
    assert!(joiner.gaps().is_empty());
    request(
        &mut joiner,
        &mut sequence,
        "user-14",
        &"x".repeat(MAX_STRING_BYTES),
    );
    assert!(joiner.evidence.is_empty());
    assert!(joiner.gaps().contains(&Gap::CapacityExceeded));
    assert!(joiner.retained_payload_bytes() <= MAX_RETAINED_PAYLOAD_BYTES);
    assert_accounting(&joiner);

    sequence += 1;
    let batch = joiner.observe_evidence(&event(
        sequence,
        "session.step.started",
        json!({"assistantMessageID": "step"}),
    ));
    assert!(!batch.history_complete);
    assert!(batch.events.iter().all(|event| !event.live_eligible));
}

#[test]
fn old_history_counts_until_its_last_step_and_call_release_it() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let mut sequence = 0;
    request(
        &mut joiner,
        &mut sequence,
        "old",
        &"x".repeat(MAX_STRING_BYTES),
    );
    observe(
        &mut joiner,
        &mut sequence,
        "session.step.started",
        json!({"assistantMessageID": "old-step"}),
    );
    start_call(&mut joiner, &mut sequence, "old-step", "call");
    observe(
        &mut joiner,
        &mut sequence,
        "session.inbox.delivered",
        json!({"inboxID": "missed"}),
    );
    request(&mut joiner, &mut sequence, "new", "new request");
    assert_eq!(
        joiner.history_bytes.load(Ordering::Relaxed),
        3 + MAX_STRING_BYTES + 3 + "new request".len()
    );
    assert_accounting(&joiner);
    let result = finish_call(&mut joiner, &mut sequence, "old-step", "call");
    assert_eq!(result.request_message_id.as_deref(), Some("old"));
    observe(
        &mut joiner,
        &mut sequence,
        "session.step.ended",
        json!({"assistantMessageID": "old-step"}),
    );
    assert_eq!(
        joiner.history_bytes.load(Ordering::Relaxed),
        3 + "new request".len()
    );
    assert_accounting(&joiner);
}

#[test]
fn terminal_eviction_and_input_replacement_keep_exact_running_counts() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let mut sequence = 0;
    request(&mut joiner, &mut sequence, "user", "request");
    observe(
        &mut joiner,
        &mut sequence,
        "session.step.started",
        json!({"assistantMessageID": "step"}),
    );
    for index in 0..MAX_RETAINED_CALLS + 20 {
        let call = format!("call-{index}");
        start_call(&mut joiner, &mut sequence, "step", &call);
        observe(
            &mut joiner,
            &mut sequence,
            "session.tool.input.ended",
            json!({
                "assistantMessageID": "step", "id": call, "text": "{\"path\":\"short\"}"
            }),
        );
        assert_accounting(&joiner);
        finish_call(&mut joiner, &mut sequence, "step", &call);
        assert_accounting(&joiner);
    }
    assert_eq!(joiner.calls.len(), MAX_RETAINED_CALLS);
    assert_eq!(joiner.terminal_call_order.len(), MAX_RETAINED_CALLS);
    for _ in 0..MAX_RETAINED_CALLS * 8 {
        observe(
            &mut joiner,
            &mut sequence,
            "session.text.delta",
            json!({"delta": "x"}),
        );
    }
    assert_accounting(&joiner);
    assert!(joiner.gaps().is_empty());
}

#[test]
fn inbox_replacement_cancellation_and_capacity_reset_release_all_payloads() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let mut sequence = 0;
    for text in ["first request", "short"] {
        observe(
            &mut joiner,
            &mut sequence,
            "session.inbox.enqueued",
            json!({
                "inboxID": "replaced", "item": {"type": "user", "payload": {"text": text}}
            }),
        );
        assert_accounting(&joiner);
    }
    observe(
        &mut joiner,
        &mut sequence,
        "session.inbox.cancelled",
        json!({"inboxID": "replaced"}),
    );
    assert_accounting(&joiner);
    for index in 0..40 {
        let id = format!("user-{index}");
        observe(
            &mut joiner,
            &mut sequence,
            "session.inbox.enqueued",
            json!({"inboxID": id, "item": {"type": "user", "payload": {"text": "x".repeat(MAX_STRING_BYTES)}}}),
        );
        if joiner.gaps().contains(&Gap::CapacityExceeded) {
            break;
        }
    }
    assert!(joiner.gaps().contains(&Gap::CapacityExceeded));
    assert_eq!(joiner.history_bytes.load(Ordering::Relaxed), 0);
    assert_eq!(joiner.retained_payload_bytes(), "session".len());
    assert_accounting(&joiner);
}

#[test]
fn evicted_replay_cannot_change_pending_input_or_lower_the_watermark() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let mut sequence = 0;
    request(&mut joiner, &mut sequence, "user", "request");
    observe(
        &mut joiner,
        &mut sequence,
        "session.step.started",
        json!({"assistantMessageID": "step"}),
    );
    start_call(&mut joiner, &mut sequence, "step", "call");
    let replay = event(
        sequence,
        "session.tool.called",
        json!({
            "assistantMessageID": "step", "id": "call", "input": {"path": "replay.rs"}
        }),
    );
    for _ in 0..MAX_RETAINED_CALLS * 8 {
        observe(
            &mut joiner,
            &mut sequence,
            "session.text.delta",
            json!({"delta": "x"}),
        );
    }
    let high_watermark = joiner.last_sequence;
    assert!(joiner.observe(&replay).is_none());
    assert_eq!(joiner.last_sequence, high_watermark);
    let result = finish_call(&mut joiner, &mut sequence, "step", "call");
    assert_eq!(result.input, Some(json!({"path": "sample.rs"})));
    assert!(result.gaps.contains(&Gap::SequenceDiscontinuity));
    assert_accounting(&joiner);
}
