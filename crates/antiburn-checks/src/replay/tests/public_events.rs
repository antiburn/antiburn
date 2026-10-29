use super::*;
use crate::domain::{EvidenceDependency, SessionStartReason};
use crate::source::{normalize_assistant_message, normalize_tool_call, normalize_user_message};

fn public_event(position: u64, kind: ObservationKind) -> SourceEvent {
    let mut source = event(position);
    let SourceEvent::Observation {
        identity,
        observation,
        ..
    } = &mut source
    else {
        unreachable!()
    };
    *identity = match &kind {
        ObservationKind::UserMessage(message)
        | ObservationKind::AssistantMessage(message)
        | ObservationKind::ToolCall(message) => message.id.clone(),
        _ => format!("public-{position}"),
    };
    observation.kind = kind;
    observation.first_eligible_publication = Some(PublicationCursor(position));
    observation.context.dependencies = vec![EvidenceDependency::SourceEvidence {
        id: identity.clone(),
        version: observation.version.clone(),
    }];
    match &observation.kind {
        ObservationKind::SessionStart { reason } => {
            observation.context.value["session"]["start_reason"] = json!(reason)
        }
        ObservationKind::SessionIdle {
            attachment_epoch,
            activity_revision,
            idle_seconds,
            ..
        } => {
            observation.context.value["session"]["idle_seconds"] = json!(idle_seconds);
            observation
                .context
                .dependencies
                .push(EvidenceDependency::SessionActivity {
                    attachment_epoch: attachment_epoch.clone(),
                    activity_revision: *activity_revision,
                });
        }
        ObservationKind::TurnEnd { turn } => observation.context.value["turn"] = turn.clone(),
        _ => {}
    }
    source
}

#[test]
fn isolated_replay_accepts_mixed_public_messages_and_keeps_the_frozen_window() {
    let definition = "name: Mixed messages\non:\n  user_message:\n  assistant_message:\n  tool_call:\n  tool_result:\n  window: {size: 4, min: 4}\n  cooldown: 0s\nactions:\n  - type: status\n    input: {result: success, message: '${{ toJSON(window.messages) }}'}\n";
    let user = normalize_user_message("user", "synthetic", "Public request", true).unwrap();
    let assistant = normalize_assistant_message(
        "assistant",
        "synthetic",
        "Public answer",
        true,
        &Default::default(),
    )
    .unwrap();
    let call = normalize_tool_call(
        "call",
        "synthetic",
        "call-1",
        "shell",
        Some(json!("exact text")),
        &Default::default(),
        None,
        true,
    )
    .unwrap();
    let mut source = Events(
        vec![
            public_event(1, ObservationKind::UserMessage(user)),
            public_event(2, ObservationKind::AssistantMessage(assistant)),
            public_event(3, ObservationKind::ToolCall(call)),
            event(4),
        ]
        .into_iter(),
    );
    let store = Arc::new(MemoryStore::default());
    let evaluator = Arc::new(ScriptedEvaluator {
        inputs: Mutex::new(Vec::new()),
    });
    let report = block_on(replay(
        ReplayRunId("mixed-public".into()),
        activation_for(definition),
        store.clone(),
        evaluator.clone(),
        &mut source,
        ReplayLimits {
            events: 8,
            evaluations: 8,
            attempts: 8,
        },
        || false,
    ))
    .unwrap();
    assert_eq!(report.end, ReplayEnd::Complete);
    assert_eq!(report.evaluations, 1);
    assert_eq!(report.attempts, 0);
    assert!(evaluator.inputs.lock().unwrap().is_empty());
    let accepted = store.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 1);
    assert_eq!(
        accepted[0]
            .claimed
            .candidate
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["user", "assistant", "call", "result-4"]
    );
    assert!(matches!(
        accepted[0].claimed.candidate.activation.scope.run,
        crate::domain::RunScope::Replay(_)
    ));
}

#[test]
fn isolated_replay_accepts_single_cycle_start_and_idle_events_without_result_messages() {
    let user = normalize_user_message("user", "synthetic", "Public input", true).unwrap();
    for (event_name, kind) in [
        (
            "turn_end",
            ObservationKind::TurnEnd {
                turn: json!({"id": "cycle", "source": "synthetic", "status": "completed", "messages": [user.value]}),
            },
        ),
        (
            "session_start",
            ObservationKind::SessionStart {
                reason: SessionStartReason::Attached,
            },
        ),
        (
            "session_idle",
            ObservationKind::SessionIdle {
                attachment_epoch: "attachment".into(),
                activity_revision: 4,
                idle_seconds: 120,
                last_activity_at_ms: None,
            },
        ),
    ] {
        let definition = format!(
            "name: Host boundary\non:\n  {event_name}:\n  cooldown: 0s\nactions:\n  - type: status\n    input: {{result: success}}\n"
        );
        let first = public_event(1, kind);
        let mut correction = first.clone();
        let SourceEvent::Observation {
            cursor,
            revision,
            observation,
            ..
        } = &mut correction
        else {
            unreachable!()
        };
        *cursor = PublicationCursor(2);
        *revision = 2;
        observation.version = EvidenceVersion("correction".into());
        let mut source = Events(vec![first, correction].into_iter());
        let store = Arc::new(MemoryStore::default());
        let evaluator = Arc::new(ScriptedEvaluator {
            inputs: Mutex::new(Vec::new()),
        });
        let report = block_on(replay(
            ReplayRunId(event_name.into()),
            activation_for(&definition),
            store.clone(),
            evaluator.clone(),
            &mut source,
            ReplayLimits {
                events: 4,
                evaluations: 4,
                attempts: 4,
            },
            || false,
        ))
        .unwrap();
        assert_eq!(report.end, ReplayEnd::Complete);
        assert_eq!(report.evaluations, 1, "{event_name}");
        assert_eq!(report.attempts, 0);
        let accepted = store.accepted.lock().unwrap();
        assert_eq!(accepted.len(), 1);
        let candidate = &accepted[0].claimed.candidate;
        assert!(candidate.messages.is_empty());
        assert!(candidate.context.value.get("message").is_none());
        assert!(candidate.context.value.get("window").is_none());
        assert!(!candidate.dependencies.is_empty());
        assert!(evaluator.inputs.lock().unwrap().is_empty());
    }
}
