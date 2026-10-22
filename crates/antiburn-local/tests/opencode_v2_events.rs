use antiburn_local::analysis::opencode_v2_events::{
    Gap, MAX_RETAINED_CALLS, OpenCodeV2EventJoiner, TerminalStatus,
};
use antiburn_local::analysis::{
    NativeToolOutcome, ShellState, ToolCategory, ToolResultObservation, ToolResultStatus, WorkState,
};
use serde_json::{Value, json};

fn outcome_call(name: &str, executed: Option<bool>) -> OpenCodeV2EventJoiner {
    let mut joiner = OpenCodeV2EventJoiner::new();
    delivered_request(&mut joiner, 1, "outcome", "synthetic request");
    joiner.observe(&step_started(3, "assistant-1"));
    joiner.observe(&started(4, "call-outcome", name));
    let input = match name {
        "execute" => json!({"code":"throw new Error('synthetic');"}),
        "subagent" => {
            json!({"agent":"review","description":"Review synthetic input","prompt":"Synthetic request"})
        }
        _ => json!({"command":"exit 7"}),
    };
    let mut called = event(
        5,
        "called-outcome",
        "session.tool.called",
        data(json!({
            "assistantMessageID":"assistant-1", "id":"call-outcome", "input":input
        })),
        5.0,
    );
    if let Some(executed) = executed {
        called["data"]["executed"] = json!(executed);
    }
    joiner.observe(&called);
    joiner
}

fn outcome_terminal(metadata: Value, executed: Option<bool>) -> Value {
    let mut value = terminal(
        6,
        "terminal-outcome",
        "call-outcome",
        "session.tool.success",
        "synthetic output",
        6.0,
    );
    value["data"]["metadata"] = metadata;
    if let Some(executed) = executed {
        value["data"]["executed"] = json!(executed);
    } else {
        value["data"].as_object_mut().unwrap().remove("executed");
    }
    value
}

#[test]
fn completed_tools_keep_shell_program_failures_and_background_launch_outcomes() {
    let cases = [
        (
            "shell",
            json!({"status":"completed","exit":7}),
            NativeToolOutcome::Shell {
                state: Some(ShellState::Completed),
                exit: Some(7),
                signal: None,
                timed_out: None,
                shell_id: None,
            },
        ),
        (
            "shell",
            json!({"status":"completed","signal":"SIGTERM","timeout":true}),
            NativeToolOutcome::Shell {
                state: Some(ShellState::Completed),
                exit: None,
                signal: Some("SIGTERM".into()),
                timed_out: Some(true),
                shell_id: None,
            },
        ),
        (
            "shell",
            json!({"status":"running","shellID":"sh_background"}),
            NativeToolOutcome::Shell {
                state: Some(ShellState::Running),
                exit: None,
                signal: None,
                timed_out: None,
                shell_id: Some("sh_background".into()),
            },
        ),
        (
            "execute",
            json!({"error":true,"toolCalls":[{"tool":"shell","input":{"command":"exit 7"},"status":"error"}]}),
            NativeToolOutcome::CodeMode { error: Some(true) },
        ),
        (
            "subagent",
            json!({"sessionID":"ses_background","status":"running"}),
            NativeToolOutcome::Subagent {
                session_id: "ses_background".into(),
                state: WorkState::Running,
            },
        ),
        (
            "subagent",
            json!({"sessionID":"ses_completed","status":"completed"}),
            NativeToolOutcome::Subagent {
                session_id: "ses_completed".into(),
                state: WorkState::Completed,
            },
        ),
    ];
    for (name, metadata, expected) in cases {
        let mut joiner = outcome_call(name, Some(false));
        let result = joiner
            .observe(&outcome_terminal(metadata, Some(false)))
            .unwrap();
        assert_eq!(result.status, TerminalStatus::Success);
        assert_eq!(result.outcome, Some(expected.clone()));
        let observation = result.into_observation(true, true);
        assert_eq!(observation.status, ToolResultStatus::Success);
        assert!(observation.finality);
        assert_eq!(observation.outcome, Some(expected));
        assert_eq!(observation.provider_executed, Some(false));
        if name == "execute" {
            assert_eq!(observation.tool_category, Some(ToolCategory::Other));
        }
        let encoded = serde_json::to_vec(&observation).unwrap();
        assert_eq!(
            serde_json::from_slice::<ToolResultObservation>(&encoded).unwrap(),
            observation
        );
    }
}

#[test]
fn provider_execution_is_typed_without_borrowing_local_tool_outcomes() {
    let mut joiner = outcome_call("shell", Some(true));
    let observation = joiner
        .observe(&outcome_terminal(
            json!({"exit":7,"status":"completed","timeout":true}),
            Some(true),
        ))
        .unwrap()
        .into_observation(true, true);
    assert_eq!(observation.provider_executed, Some(true));
    assert_eq!(observation.outcome, None);
    assert_eq!(observation.tool_category, Some(ToolCategory::Other));
    assert_eq!(observation.status, ToolResultStatus::Success);

    let mut unknown = outcome_call("shell", None);
    let observation = unknown
        .observe(&outcome_terminal(json!({}), None))
        .unwrap()
        .into_observation(true, true);
    assert_eq!(observation.provider_executed, None);
    assert_eq!(observation.outcome, None);
    let mut legacy = serde_json::to_value(&observation).unwrap();
    legacy.as_object_mut().unwrap().remove("outcome");
    legacy.as_object_mut().unwrap().remove("provider_executed");
    assert_eq!(
        serde_json::from_value::<ToolResultObservation>(legacy).unwrap(),
        observation
    );
}

#[test]
fn provider_execution_can_upgrade_from_called_false_to_terminal_true() {
    for failed in [false, true] {
        let mut joiner = outcome_call("shell", Some(false));
        let mut terminal = outcome_terminal(
            json!({"exit":7,"status":"completed","timeout":true}),
            Some(true),
        );
        if failed {
            terminal["type"] = json!("session.tool.failed");
            terminal["data"]["error"] =
                json!({"type":"tool.execution","message":"synthetic provider failure"});
        }
        let result = joiner.observe(&terminal).unwrap();
        assert!(result.gaps.is_empty());
        let observation = result.into_observation(true, true);
        assert_eq!(observation.provider_executed, Some(true));
        assert_eq!(observation.tool_category, Some(ToolCategory::Other));
        assert_eq!(observation.outcome, None);
        assert_eq!(
            observation.status,
            if failed {
                ToolResultStatus::Error
            } else {
                ToolResultStatus::Success
            }
        );
    }
}

#[test]
fn provider_execution_downgrades_remain_unsafe() {
    for failed in [false, true] {
        let mut joiner = outcome_call("shell", Some(true));
        let mut terminal = outcome_terminal(json!({"exit":7}), Some(false));
        if failed {
            terminal["type"] = json!("session.tool.failed");
            terminal["data"]["error"] =
                json!({"type":"tool.execution","message":"synthetic provider failure"});
        }
        assert!(joiner.observe(&terminal).is_none());
        assert!(joiner.gaps().contains(&Gap::ConflictingDuplicate));
    }
}

#[test]
fn failed_subagent_terminal_does_not_promote_running_progress_to_current_work_state() {
    for message in [
        "Subagent failed (sessionID: ses_child): synthetic failure",
        "Subagent cancelled (sessionID: ses_child)",
    ] {
        let mut joiner = outcome_call("subagent", Some(false));
        let mut terminal = outcome_terminal(
            json!({"sessionID":"ses_child","status":"running"}),
            Some(false),
        );
        terminal["type"] = json!("session.tool.failed");
        terminal["data"]["error"] = json!({"type":"tool.execution","message":message});
        terminal["data"].as_object_mut().unwrap().remove("content");
        let result = joiner.observe(&terminal).unwrap();
        assert_eq!(result.status, TerminalStatus::Failed);
        assert_eq!(result.outcome, None);
        let observation = result.into_observation(true, true);
        assert_eq!(observation.status, ToolResultStatus::Error);
        assert_eq!(observation.provider_executed, Some(false));
        assert_eq!(observation.outcome, None);
    }
}

#[test]
fn outcome_changes_conflict_even_when_terminal_text_and_status_match() {
    let mut joiner = outcome_call("shell", Some(false));
    let first = outcome_terminal(json!({"exit":0,"status":"completed"}), Some(false));
    assert!(joiner.observe(&first).is_some());
    let mut changed = first;
    changed["id"] = json!("changed-outcome");
    changed["durable"]["seq"] = json!(7);
    changed["data"]["metadata"]["exit"] = json!(7);
    assert!(joiner.observe(&changed).is_none());
    assert!(joiner.gaps().contains(&Gap::ConflictingDuplicate));
}

#[test]
fn clipped_and_nontext_results_keep_outcomes_but_do_not_claim_complete_text() {
    for nontext in [false, true] {
        let mut joiner = outcome_call("shell", Some(false));
        let mut value = outcome_terminal(
            json!({"status":"completed","exit":7,"truncated":!nontext}),
            Some(false),
        );
        if nontext {
            value["data"]["content"] = json!([{"type":"file","mime":"application/pdf","uri":"data:application/pdf;base64,U1lOVEhFVElD","text":"not a text part"}]);
        }
        let result = joiner.observe(&value).unwrap();
        assert!(result.gaps.contains(&Gap::OptionalContentOmitted));
        assert!(result.outcome.is_some());
        if nontext {
            assert!(result.content.is_none());
        }
        let observation = result.into_observation(true, true);
        assert_eq!(observation.status, ToolResultStatus::Success);
        assert_eq!(
            observation.completeness,
            antiburn_local::analysis::ToolResultCompleteness::Incomplete
        );
    }
}

fn event(seq: u64, id: &str, event_type: &str, data: Value, created: f64) -> Value {
    let durable_version = if matches!(
        event_type,
        "session.deleted"
            | "session.forked"
            | "session.tool.success"
            | "session.tool.failed"
            | "session.instructions.updated"
    ) {
        2
    } else {
        1
    };
    json!({
        "id": id,
        "type": event_type,
        "created": created,
        "durable": {"aggregateID": "sess-1", "seq": seq, "version": durable_version},
        "data": data,
    })
}

fn data(extra: Value) -> Value {
    let mut value = json!({"sessionID": "sess-1"});
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    value
}

fn started(seq: u64, call: &str, name: &str) -> Value {
    event(
        seq,
        &format!("start-{call}"),
        "session.tool.input.started",
        data(json!({
            "assistantMessageID": "assistant-1", "id": call, "name": name,
        })),
        10.0,
    )
}

fn step_started(seq: u64, assistant_message_id: &str) -> Value {
    event(
        seq,
        &format!("step-{assistant_message_id}"),
        "session.step.started",
        data(json!({ "assistantMessageID": assistant_message_id })),
        9.0,
    )
}

fn delivered_request(joiner: &mut OpenCodeV2EventJoiner, seq: u64, id: &str, text: &str) {
    joiner.observe(&event(
        seq,
        &format!("enqueued-{id}"),
        "session.inbox.enqueued",
        data(json!({
            "inboxID":"user-message",
            "item":{"type":"user","payload":{"text":text,"files":[],"agents":[],"skills":[]},"delivery":"queue"}
        })),
        seq as f64,
    ));
    joiner.observe(&event(
        seq + 1,
        &format!("delivered-{id}"),
        "session.inbox.delivered",
        data(json!({"inboxID":"user-message"})),
        (seq + 1) as f64,
    ));
}

fn terminal(seq: u64, id: &str, call: &str, status: &str, content: &str, created: f64) -> Value {
    event(
        seq,
        id,
        status,
        data(json!({
            "assistantMessageID": "assistant-1", "id": call, "executed": true,
            "content": [{"type": "text", "text": content}],
        })),
        created,
    )
}

#[test]
fn increasing_sequence_jumps_are_safe_even_when_timestamps_run_backwards() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    delivered_request(&mut joiner, 1, "a", "request");
    assert!(joiner.observe(&step_started(4, "assistant-1")).is_none());
    assert!(joiner.observe(&started(5, "call-a", "read")).is_none());
    assert!(joiner.observe(&started(6, "call-b", "write")).is_none());

    let later_timestamp_first = terminal(9, "term-a", "call-a", "session.tool.success", "A", 30.0);
    let earlier_timestamp_second =
        terminal(10, "term-b", "call-b", "session.tool.success", "B", 20.0);
    let first = joiner.observe(&later_timestamp_first).unwrap();
    let second = joiner.observe(&earlier_timestamp_second).unwrap();
    assert_eq!(first.sequence, 9);
    assert_eq!(second.sequence, 10);
    assert_eq!(first.created, Some(30.0));
    assert_eq!(second.created, Some(20.0));
    assert!(first.gaps.is_empty());
    assert!(second.gaps.is_empty());
}

#[test]
fn equal_or_regressed_sequence_marks_later_results_unsafe() {
    for (second_sequence, expected_sequence) in [(5, 5), (4, 4)] {
        let mut joiner = OpenCodeV2EventJoiner::new();
        delivered_request(&mut joiner, 1, "a", "request");
        joiner.observe(&step_started(4, "assistant-1"));
        joiner.observe(&started(6, "call-a", "read"));
        joiner.observe(&started(second_sequence + 1, "call-b", "write"));
        let result = joiner
            .observe(&terminal(
                expected_sequence + 2,
                "terminal-b",
                "call-b",
                "session.tool.success",
                "done",
                6.0,
            ))
            .unwrap();
        assert!(result.gaps.contains(&Gap::SequenceDiscontinuity));
        assert!(joiner.gaps().contains(&Gap::SequenceDiscontinuity));
    }
}

#[test]
fn caller_can_mark_transport_gap_for_future_results() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    joiner.mark_gap();
    delivered_request(&mut joiner, 1, "a", "request");
    joiner.observe(&step_started(4, "assistant-1"));
    joiner.observe(&started(5, "call-a", "read"));
    let result = joiner
        .observe(&terminal(
            9,
            "terminal-a",
            "call-a",
            "session.tool.success",
            "done",
            6.0,
        ))
        .unwrap();
    assert!(result.gaps.contains(&Gap::ContinuityLost));
    assert!(joiner.gaps().contains(&Gap::ContinuityLost));
}

#[test]
fn missing_call_join_is_explicit_and_not_emitted() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    assert!(
        joiner
            .observe(&terminal(
                1,
                "term",
                "unknown",
                "session.tool.success",
                "x",
                1.0
            ))
            .is_none()
    );
    assert!(joiner.gaps().contains(&Gap::MissingJoin));
}

#[test]
fn exact_duplicate_is_ignored_and_terminal_correction_is_unsafe() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    delivered_request(&mut joiner, 1, "a", "request");
    joiner.observe(&step_started(4, "assistant-1"));
    joiner.observe(&started(5, "call-a", "read"));
    let first = terminal(6, "term-a", "call-a", "session.tool.success", "old", 2.0);
    assert!(joiner.observe(&first).is_some());
    assert!(joiner.observe(&first).is_none());
    let correction = terminal(
        7,
        "term-b",
        "call-a",
        "session.tool.success",
        "corrected",
        3.0,
    );
    assert!(joiner.observe(&correction).is_none());
    assert!(joiner.gaps().contains(&Gap::ConflictingDuplicate));
}

#[test]
fn only_delivered_user_message_provides_request_context() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    // The native V2 enqueue payload supplies the user text. Delivery admits it.
    delivered_request(&mut joiner, 1, "a", "delivered request");
    joiner.observe(&step_started(4, "assistant-1"));
    joiner.observe(&started(5, "call-a", "read"));
    joiner.observe(&event(6, "called", "session.tool.called", data(json!({
        "assistantMessageID": "assistant-1", "id": "call-a", "input": {"path": "a"}, "executed": true
    })), 4.0));
    let result = joiner
        .observe(&terminal(
            7,
            "term",
            "call-a",
            "session.tool.success",
            "ok",
            5.0,
        ))
        .unwrap();
    assert_eq!(result.request_message_id.as_deref(), Some("user-message"));
    assert_eq!(result.request_text.as_deref(), Some("delivered request"));

    // Text passed as tool input is not promoted to a user request when no
    // delivered user message precedes the call.
    let mut queued_only = OpenCodeV2EventJoiner::new();
    queued_only.observe(&event(
        1,
        "queued-inbox",
        "session.inbox.enqueued",
        data(json!({
            "inboxID":"queued-id",
            "item":{"type":"user","payload":{"text":"queued text"},"delivery":"queue"}
        })),
        1.0,
    ));
    queued_only.observe(&event(
        2,
        "cancel-queued",
        "session.inbox.cancelled",
        data(json!({"inboxID":"queued-id"})),
        2.0,
    ));
    queued_only.observe(&step_started(3, "assistant-1"));
    queued_only.observe(&started(4, "call-q", "execute"));
    queued_only.observe(&event(
        5,
        "called-q",
        "session.tool.called",
        data(json!({
            "assistantMessageID": "assistant-1", "id": "call-q",
            "input": {"prompt": "queued text"}, "executed": true
        })),
        2.0,
    ));
    let queued_result = queued_only
        .observe(&terminal(
            6,
            "term-q",
            "call-q",
            "session.tool.success",
            "done",
            3.0,
        ))
        .unwrap();
    assert_eq!(queued_result.request_message_id, None);
    assert_eq!(queued_result.request_text, None);
    assert!(queued_result.gaps.contains(&Gap::MissingJoin));
}

#[test]
fn a_steer_delivered_during_a_step_does_not_rebind_its_tool_context() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    for (seq, id, message_id, text) in [
        (1, "user-1", "user-1", "first request"),
        (5, "user-2", "user-2", "steer during the step"),
    ] {
        joiner.observe(&event(
            seq,
            &format!("enqueued-{id}"),
            "session.inbox.enqueued",
            data(json!({
                "inboxID": message_id,
                "item":{"type":"user","payload":{"text":text,"files":[],"agents":[],"skills":[]},"delivery":"steer"}
            })),
            seq as f64,
        ));
        joiner.observe(&event(
            seq + 1,
            &format!("delivered-{id}"),
            "session.inbox.delivered",
            data(json!({"inboxID": message_id})),
            (seq + 1) as f64,
        ));
        if seq == 1 {
            joiner.observe(&step_started(4, "assistant-1"));
        }
    }
    joiner.observe(&started(8, "call-a", "read"));
    joiner.observe(&event(
        9,
        "called-a",
        "session.tool.called",
        data(json!({
            "assistantMessageID":"assistant-1", "id":"call-a", "input":{"path":"a"}
        })),
        7.0,
    ));
    let result = joiner
        .observe(&terminal(
            10,
            "terminal-a",
            "call-a",
            "session.tool.success",
            "done",
            8.0,
        ))
        .unwrap();

    assert_eq!(result.request_message_id.as_deref(), Some("user-1"));
    assert_eq!(result.request_text.as_deref(), Some("first request"));
    assert_eq!(result.user_messages.len(), 1);
    assert_eq!(result.user_messages[0].message_id, "user-1");

    joiner.observe(&step_started(11, "assistant-2"));
    joiner.observe(&event(
        12,
        "start-call-b",
        "session.tool.input.started",
        data(json!({
            "assistantMessageID":"assistant-2", "id":"call-b", "name":"read"
        })),
        12.0,
    ));
    joiner.observe(&event(
        13,
        "called-b",
        "session.tool.called",
        data(json!({
            "assistantMessageID":"assistant-2", "id":"call-b", "input":{"path":"b"}
        })),
        13.0,
    ));
    let later = joiner
        .observe(&event(
            14,
            "terminal-b",
            "session.tool.success",
            data(json!({
                "assistantMessageID":"assistant-2", "id":"call-b", "executed":true,
                "content":[{"type":"text","text":"done"}]
            })),
            14.0,
        ))
        .unwrap();
    assert_eq!(later.request_message_id.as_deref(), Some("user-2"));
    assert_eq!(later.request_text.as_deref(), Some("steer during the step"));
    assert_eq!(later.user_messages.len(), 2);
    assert_eq!(later.user_messages[1].message_id, "user-2");
}

#[test]
fn execute_outer_result_is_not_split_into_nested_tool_results() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    delivered_request(&mut joiner, 1, "execute", "run this");
    joiner.observe(&step_started(4, "assistant-1"));
    joiner.observe(&started(5, "execute-call", "execute"));
    joiner.observe(&event(6, "called", "session.tool.called", data(json!({
        "assistantMessageID": "assistant-1", "id": "execute-call", "input": {"code": "tools.search(...)"}, "executed": true
    })), 2.0));
    let result = joiner
        .observe(&terminal(
            7,
            "execute-terminal",
            "execute-call",
            "session.tool.success",
            "nested result text",
            3.0,
        ))
        .unwrap();
    assert_eq!(result.tool_name, "execute");
    assert_eq!(result.status, TerminalStatus::Success);
    assert_eq!(result.content.as_deref(), Some("nested result text"));
    assert_eq!(result.call_id, "execute-call");
}

#[test]
fn joined_v2_result_maps_native_identity_order_and_delivered_request() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    delivered_request(&mut joiner, 1, "read", "inspect the file");
    joiner.observe(&step_started(4, "assistant-1"));
    joiner.observe(&started(5, "call-read", "Read"));
    joiner.observe(&event(
        6,
        "called-read",
        "session.tool.called",
        data(json!({
            "assistantMessageID":"assistant-1", "id":"call-read",
            "input":{"path":"src/main.rs"}, "executed":true
        })),
        6.0,
    ));
    let result = joiner
        .observe(&terminal(
            7,
            "terminal-read",
            "call-read",
            "session.tool.success",
            "file contents",
            7.0,
        ))
        .unwrap();

    let observation = result.into_observation(true, true);

    assert_eq!(
        observation.source_format,
        antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2
    );
    assert_eq!(
        observation.position,
        antiburn_local::analysis::ToolResultPosition::OpenCodeV2Event {
            sequence: 7,
            item_index: 0
        }
    );
    assert_eq!(observation.record_id.as_deref(), Some("assistant-1"));
    assert_eq!(observation.call_id.as_deref(), Some("call-read"));
    assert_eq!(observation.tool_name.as_deref(), Some("Read"));
    assert_eq!(observation.arguments, Some(json!({"path":"src/main.rs"})));
    assert_eq!(observation.content.as_deref(), Some("file contents"));
    assert_eq!(
        observation.status,
        antiburn_local::analysis::ToolResultStatus::Success
    );
    assert_eq!(
        observation.completeness,
        antiburn_local::analysis::ToolResultCompleteness::Complete
    );
    let context = observation.context.unwrap();
    assert!(context.complete_through_event);
    assert_eq!(context.user_messages.len(), 1);
    assert_eq!(
        context.user_messages[0].source_format,
        antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2
    );
    assert_eq!(
        context.user_messages[0].position,
        antiburn_local::analysis::UserMessagePosition::OpenCodeV2Event { sequence: 2 }
    );
    assert_eq!(context.user_messages[0].parts[0].text, "inspect the file");
}

#[test]
fn v2_result_context_stays_partial_when_attachment_content_is_not_captured() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    joiner.observe(&event(
        1,
        "enqueued",
        "session.inbox.enqueued",
        data(json!({
            "inboxID":"user-message",
            "item":{"type":"user","payload":{"text":"review this","files":[{"path":"/private/image.png"}],"agents":[],"skills":[]},"delivery":"queue"}
        })),
        1.0,
    ));
    joiner.observe(&event(
        2,
        "delivered",
        "session.inbox.delivered",
        data(json!({"inboxID":"user-message"})),
        2.0,
    ));
    joiner.observe(&step_started(3, "assistant-1"));
    joiner.observe(&started(4, "call", "Read"));
    joiner.observe(&event(
        5,
        "called",
        "session.tool.called",
        data(json!({
            "assistantMessageID":"assistant-1", "id":"call", "input":{"path":"a"}
        })),
        5.0,
    ));
    let result = joiner
        .observe(&terminal(
            6,
            "terminal",
            "call",
            "session.tool.success",
            "output",
            6.0,
        ))
        .unwrap();

    let observation = result.into_observation(true, true);
    let context = observation.context.as_ref().unwrap();

    assert_eq!(
        observation.admission(),
        antiburn_local::analysis::ToolResultAdmission::Admitted
    );
    assert!(!context.complete_through_event);
    assert_eq!(context.user_messages.len(), 1);
    assert_eq!(
        context.user_messages[0].completeness,
        antiburn_local::analysis::UserMessageCompleteness::Incomplete
    );
}

#[test]
fn conflicting_duplicate_event_ids_and_unknown_event_types_make_context_partial() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    delivered_request(&mut joiner, 1, "read", "original request");
    let conflicting_duplicate = event(
        1,
        "enqueued-read",
        "session.inbox.enqueued",
        data(json!({
            "inboxID":"user-message",
            "item":{"type":"user","payload":{"text":"changed request","files":[],"agents":[],"skills":[]},"delivery":"queue"}
        })),
        1.0,
    );
    joiner.observe(&conflicting_duplicate);
    joiner.observe(&event(
        3,
        "unknown",
        "session.future.request.changed",
        data(json!({})),
        3.0,
    ));
    joiner.observe(&step_started(4, "assistant-1"));
    joiner.observe(&started(5, "call", "Read"));
    joiner.observe(&event(
        6,
        "called",
        "session.tool.called",
        data(json!({
            "assistantMessageID":"assistant-1", "id":"call", "input":{"path":"a"}
        })),
        6.0,
    ));
    let result = joiner
        .observe(&terminal(
            7,
            "terminal",
            "call",
            "session.tool.success",
            "output",
            7.0,
        ))
        .unwrap();

    assert!(result.gaps.contains(&Gap::ConflictingDuplicate));
    assert!(result.gaps.contains(&Gap::UnknownShape));
    let observation = result.into_observation(true, true);
    assert_eq!(
        observation.completeness,
        antiburn_local::analysis::ToolResultCompleteness::Incomplete
    );
    assert!(!observation.context.unwrap().complete_through_event);
}

#[test]
fn live_event_identity_cache_evicts_old_ids_without_capacity_gaps() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    for sequence in 1..=MAX_RETAINED_CALLS * 8 + 1 {
        joiner.observe(&event(
            sequence as u64,
            &format!("event-{sequence}"),
            "session.updated",
            data(json!({})),
            sequence as f64,
        ));
    }

    assert!(!joiner.gaps().contains(&Gap::CapacityExceeded));
    joiner.observe(&event(
        1,
        "event-1",
        "session.updated",
        data(json!({})),
        1.0,
    ));
    assert!(joiner.gaps().contains(&Gap::SequenceDiscontinuity));
    assert!(!joiner.gaps().contains(&Gap::CapacityExceeded));
}

#[test]
fn unknown_durable_schema_version_marks_the_session_unknown() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let mut event = event(
        1,
        "tool-success",
        "session.tool.success",
        data(json!({"assistantMessageID":"assistant-1", "id":"call-1"})),
        1.0,
    );
    event["durable"]["version"] = json!(3);

    assert!(joiner.observe(&event).is_none());
    assert!(joiner.gaps().contains(&Gap::UnsupportedVersion));
}

#[test]
fn oversized_delivered_message_identity_cannot_produce_complete_context() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let message_id = "m".repeat(257);
    joiner.observe(&event(
        1,
        "enqueued-long-id",
        "session.inbox.enqueued",
        data(json!({
            "inboxID":message_id,
            "item":{"type":"user","payload":{"text":"request","files":[],"agents":[],"skills":[]},"delivery":"queue"}
        })),
        1.0,
    ));
    joiner.observe(&event(
        2,
        "delivered-long-id",
        "session.inbox.delivered",
        data(json!({"inboxID":message_id})),
        2.0,
    ));
    joiner.observe(&step_started(3, "assistant-1"));
    joiner.observe(&started(4, "call", "Read"));
    joiner.observe(&event(
        5,
        "called",
        "session.tool.called",
        data(json!({
            "assistantMessageID":"assistant-1", "id":"call", "input":{"path":"a"}
        })),
        5.0,
    ));
    let result = joiner
        .observe(&terminal(
            6,
            "terminal",
            "call",
            "session.tool.success",
            "output",
            6.0,
        ))
        .unwrap();

    let observation = result.into_observation(true, true);

    assert_eq!(
        observation.completeness,
        antiburn_local::analysis::ToolResultCompleteness::Incomplete
    );
    assert!(!observation.context.unwrap().complete_through_event);
}

#[test]
fn later_complete_request_starts_a_safe_context_segment_after_missing_delivery() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    joiner.observe(&event(
        1,
        "delivery-without-enqueue",
        "session.inbox.delivered",
        data(json!({"inboxID":"missed"})),
        1.0,
    ));
    delivered_request(&mut joiner, 3, "later", "new request");
    joiner.observe(&step_started(6, "assistant-1"));
    joiner.observe(&started(7, "call", "Read"));
    joiner.observe(&event(
        8,
        "called",
        "session.tool.called",
        data(json!({
            "assistantMessageID":"assistant-1", "id":"call", "input":{"path":"a"}
        })),
        8.0,
    ));
    let result = joiner
        .observe(&terminal(
            9,
            "terminal",
            "call",
            "session.tool.success",
            "output",
            9.0,
        ))
        .unwrap();

    let observation = result.into_observation(true, true);

    assert_eq!(
        observation.completeness,
        antiburn_local::analysis::ToolResultCompleteness::Complete
    );
    let context = observation.context.unwrap();
    assert!(context.complete_through_event);
    assert_eq!(context.user_messages.len(), 1);
    assert_eq!(context.user_messages[0].message_id, "user-message");
}

#[test]
fn unresolved_delivery_gap_makes_the_current_step_context_incomplete() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    delivered_request(&mut joiner, 1, "known", "known request");
    joiner.observe(&event(
        3,
        "delivery-without-enqueue",
        "session.inbox.delivered",
        data(json!({"inboxID":"missed"})),
        3.0,
    ));
    joiner.observe(&step_started(4, "assistant-1"));
    joiner.observe(&started(5, "call", "Read"));
    joiner.observe(&event(
        6,
        "called",
        "session.tool.called",
        data(json!({
            "assistantMessageID":"assistant-1", "id":"call", "input":{"path":"a"}
        })),
        6.0,
    ));
    let result = joiner
        .observe(&terminal(
            7,
            "terminal",
            "call",
            "session.tool.success",
            "output",
            7.0,
        ))
        .unwrap();

    let observation = result.into_observation(true, true);
    assert_eq!(
        observation.completeness,
        antiburn_local::analysis::ToolResultCompleteness::Incomplete
    );
    assert!(!observation.context.unwrap().complete_through_event);
}

#[test]
fn optional_timestamp_and_non_text_output_do_not_invent_order_or_fail_the_feed() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    delivered_request(&mut joiner, 1, "read", "inspect the image");
    joiner.observe(&step_started(4, "assistant-1"));
    joiner.observe(&started(5, "call", "Read"));
    joiner.observe(&event(
        6,
        "called",
        "session.tool.called",
        data(json!({
            "assistantMessageID":"assistant-1", "id":"call", "input":{"path":"image.png"}
        })),
        6.0,
    ));
    let mut terminal = terminal(7, "terminal", "call", "session.tool.success", "", 7.0);
    terminal["created"] = Value::Null;
    terminal["data"]["content"] = json!([
        {"type":"image", "data":"opaque-image"}
    ]);
    let result = joiner.observe(&terminal).unwrap();

    assert_eq!(result.sequence, 7);
    assert_eq!(result.created, None);
    assert_eq!(result.content, None);
    assert_eq!(result.gaps, [Gap::OptionalContentOmitted]);
    assert!(joiner.gaps().is_empty());
}

#[test]
fn instruction_updates_use_the_pinned_version_two_contract() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    joiner.observe(&event(
        1,
        "instructions",
        "session.instructions.updated",
        data(json!({"delta":{}})),
        1.0,
    ));
    assert!(joiner.gaps().is_empty());
    let mut old = event(
        2,
        "old",
        "session.instructions.updated",
        data(json!({"delta":{}})),
        2.0,
    );
    old["durable"]["version"] = json!(1);
    joiner.observe(&old);
    assert_eq!(joiner.gaps(), [Gap::UnsupportedVersion]);
}

#[test]
fn raw_input_is_optional_until_structured_called_arguments_arrive() {
    for raw in ["", "not JSON", "{broken"] {
        let mut joiner = OpenCodeV2EventJoiner::new();
        delivered_request(&mut joiner, 1, "request", "inspect the source");
        joiner.observe(&step_started(3, "assistant-1"));
        joiner.observe(&started(4, "call", "read"));
        joiner.observe(&event(
            5,
            "raw",
            "session.tool.input.ended",
            data(json!({
                "assistantMessageID":"assistant-1", "id":"call", "text":raw
            })),
            5.0,
        ));
        joiner.observe(&event(6, "called", "session.tool.called", data(json!({
            "assistantMessageID":"assistant-1", "id":"call", "input":{"path":"source.rs"}, "executed":true
        })), 6.0));
        let result = joiner
            .observe(&terminal(
                7,
                "terminal",
                "call",
                "session.tool.success",
                "source",
                7.0,
            ))
            .unwrap();
        assert_eq!(result.input, Some(json!({"path":"source.rs"})));
        assert!(joiner.gaps().is_empty(), "{raw:?}");
    }
}

#[test]
fn omitted_optional_attachment_arrays_keep_earlier_delivered_requests() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    for (seq, inbox, text) in [
        (1, "first", "Keep the public API"),
        (3, "second", "Read the source"),
    ] {
        joiner.observe(&event(
            seq,
            &format!("queued-{inbox}"),
            "session.inbox.enqueued",
            data(json!({
                "inboxID":inbox, "item":{"type":"user", "payload":{"text":text}, "delivery":"queue"}
            })),
            seq as f64,
        ));
        joiner.observe(&event(
            seq + 1,
            &format!("delivered-{inbox}"),
            "session.inbox.delivered",
            data(json!({"inboxID":inbox})),
            (seq + 1) as f64,
        ));
    }
    joiner.observe(&step_started(5, "assistant-1"));
    joiner.observe(&started(6, "call", "read"));
    joiner.observe(&event(
        7,
        "called",
        "session.tool.called",
        data(json!({
            "assistantMessageID":"assistant-1", "id":"call", "input":{"path":"source.rs"}
        })),
        7.0,
    ));
    let result = joiner
        .observe(&terminal(
            8,
            "terminal",
            "call",
            "session.tool.success",
            "source",
            8.0,
        ))
        .unwrap();
    assert_eq!(
        result
            .user_messages
            .iter()
            .map(|message| message.text.as_deref())
            .collect::<Vec<_>>(),
        [Some("Keep the public API"), Some("Read the source")]
    );
    assert!(result.user_messages.iter().all(|message| message.complete));
}

#[test]
fn retained_context_payloads_reach_a_byte_limit_before_count_limits() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    for index in 0..12 {
        let seq = index * 3 + 1;
        let inbox = format!("inbox-{index}");
        joiner.observe(&event(seq, &format!("queued-{index}"), "session.inbox.enqueued", data(json!({
            "inboxID":inbox, "item":{"type":"user", "payload":{"text":"x".repeat(16 * 1024)}, "delivery":"queue"}
        })), seq as f64));
        joiner.observe(&event(
            seq + 1,
            &format!("delivered-{index}"),
            "session.inbox.delivered",
            data(json!({"inboxID":inbox})),
            (seq + 1) as f64,
        ));
        joiner.observe(&step_started(seq + 2, &format!("assistant-{index}")));
        if joiner.gaps().contains(&Gap::CapacityExceeded) {
            return;
        }
    }
    panic!("retained request copies exceeded the byte budget without a gap");
}

#[test]
fn pending_non_user_inbox_keys_count_toward_the_retained_byte_limit() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    for index in 0..32 {
        let inbox = format!("{index:05}{}", "x".repeat(16 * 1024 - 5));
        joiner.observe(&event(
            index + 1,
            &format!("enqueued-{index}"),
            "session.inbox.enqueued",
            data(json!({"inboxID":inbox, "item":{"type":"synthetic"}})),
            index as f64,
        ));
        if index < 31 {
            assert!(joiner.gaps().is_empty());
        }
    }
    assert_eq!(joiner.gaps(), [Gap::CapacityExceeded]);
}

#[test]
fn invalid_envelopes_still_enforce_the_retained_event_id_byte_limit() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    for index in 0..32 {
        let id = format!("{index:05}{}", "x".repeat(16 * 1024 - 5));
        let mut event = event(
            index + 1,
            &id,
            "session.updated",
            data(json!({})),
            index as f64,
        );
        event["durable"].as_object_mut().unwrap().remove("seq");
        joiner.observe(&event);
        if index < 31 {
            assert_eq!(joiner.gaps(), [Gap::MissingSequence]);
        }
    }
    assert_eq!(joiner.gaps(), [Gap::MissingSequence, Gap::CapacityExceeded]);
}

#[test]
fn retained_session_identity_counts_toward_the_byte_limit() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    let session = "s".repeat(16 * 1024);
    for index in 0..31 {
        let id = format!("{index:05}{}", "x".repeat(16 * 1024 - 5));
        let mut event = event(
            index + 1,
            &id,
            "session.updated",
            json!({
                "sessionID":session, "info":{"id":session}
            }),
            index as f64,
        );
        event["durable"]["aggregateID"] = json!(session);
        joiner.observe(&event);
        if index < 30 {
            assert!(joiner.gaps().is_empty());
        }
    }
    assert_eq!(joiner.gaps(), [Gap::CapacityExceeded]);
}

#[test]
fn delivered_request_step_call_and_terminal_cross_every_hydration_boundary_once() {
    use antiburn_local::analysis::opencode_v2_capture::{BoundaryKind, OpenCodeV2CaptureEpoch};
    use std::collections::HashMap;

    let events = [
        event(
            1,
            "enqueued",
            "session.inbox.enqueued",
            data(json!({"inboxID":"user",
            "item":{"type":"user","payload":{"text":"request"}}})),
            1.0,
        ),
        event(
            2,
            "delivered",
            "session.inbox.delivered",
            data(json!({"inboxID":"user"})),
            2.0,
        ),
        step_started(3, "assistant-1"),
        started(4, "call", "shell"),
        event(
            5,
            "called",
            "session.tool.called",
            data(json!({"assistantMessageID":"assistant-1",
            "id":"call","input":{"command":"true"},"executed":false})),
            5.0,
        ),
        event(
            6,
            "terminal",
            "session.tool.success",
            data(json!({"assistantMessageID":"assistant-1",
            "id":"call","executed":false,"content":[{"type":"text","text":"ok"}],
            "metadata":{"exit":7}})),
            6.0,
        ),
    ];
    for boundary in 0..=6 {
        for buffered_count in 0..=events.len() {
            let mut capture = OpenCodeV2CaptureEpoch::new(vec!["sess-1".into()]).unwrap();
            capture.confirm_subscription().unwrap();
            for event in &events[..buffered_count] {
                capture.observe(event.clone()).unwrap();
            }
            let mut captured = capture
                .finish_hydration(HashMap::from([("sess-1".into(), Some(boundary))]))
                .unwrap();
            for event in &events[buffered_count..] {
                captured.extend(capture.observe(event.clone()).unwrap());
            }
            let mut joiner = OpenCodeV2EventJoiner::new();
            let mut live = Vec::new();
            for captured in captured {
                let result = joiner.observe(&captured.event);
                if captured.kind == BoundaryKind::Live {
                    live.extend(result);
                }
            }
            assert_eq!(
                live.len(),
                usize::from(boundary < 6),
                "boundary={boundary}, buffered={buffered_count}"
            );
            if let Some(result) = live.pop() {
                assert_eq!(result.sequence, 6);
                assert_eq!(result.request_text.as_deref(), Some("request"));
                let observation = result.into_observation(false, true);
                assert_eq!(
                    observation.status,
                    antiburn_local::analysis::ToolResultStatus::Success
                );
                assert!(matches!(
                    observation.outcome,
                    Some(antiburn_local::analysis::NativeToolOutcome::Shell { exit: Some(7), .. })
                ));
            }
            assert!(capture.observe(events[5].clone()).unwrap().is_none());
            assert!(capture.gapped_sessions().next().is_none());
        }
    }
}

#[test]
fn repeated_terminal_with_new_event_id_keeps_one_result_without_a_conflict() {
    let mut joiner = OpenCodeV2EventJoiner::new();
    delivered_request(&mut joiner, 1, "a", "request");
    joiner.observe(&step_started(3, "assistant-1"));
    joiner.observe(&started(4, "call", "read"));
    let first = joiner
        .observe(&terminal(
            5,
            "first",
            "call",
            "session.tool.success",
            "ok",
            5.0,
        ))
        .unwrap();
    assert!(
        joiner
            .observe(&terminal(
                6,
                "duplicate",
                "call",
                "session.tool.success",
                "ok",
                6.0
            ))
            .is_none()
    );
    assert_eq!(first.sequence, 5);
    assert_eq!(first.event_id, "first");
    assert!(!joiner.gaps().contains(&Gap::ConflictingDuplicate));
}
