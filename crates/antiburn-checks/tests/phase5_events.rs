use std::sync::Arc;

use antiburn_checks::check_store::SourceProgress;
use antiburn_checks::compile;
use antiburn_checks::compiler::{ToolCategory, TriggerFamily};
use antiburn_checks::context::{CurrentInventories, ObservedAttribution};
use antiburn_checks::definition_lifecycle::EffectiveActivation;
use antiburn_checks::domain::{
    ActivationId, BranchId, EvidenceDependency, EvidenceVersion, Generation, NativeOrderKey,
    Observation, ObservationKind, ObservationPosition,
};
use antiburn_checks::filters::{AttributionScope, FilterReason};
use antiburn_checks::selection::{SelectionOutcome, SelectionState};
use antiburn_checks::source::*;
use serde_json::{Value, json};

fn definition(events: &str, condition: &str, input: Option<&str>) -> String {
    let decisions = input.map_or(String::new(), |input| format!("decisions:\n  review:\n    input: {{value: {input}}}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {{yes: Useful., no: Not useful.}}\n"));
    format!(
        "name: Public events\non:\n{events}\n{condition}{decisions}actions:\n  - type: status\n    input: {{result: failure}}\n"
    )
}

fn activation(source: &str) -> EffectiveActivation {
    EffectiveActivation {
        id: ActivationId {
            scope: Default::default(),
            definition_id: "events".into(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: antiburn_checks::discovery::DefinitionScope::Global,
        source_paths: vec![],
        project_root: Some("/repo".into()),
        compiled: Arc::new(compile(source).unwrap()),
        bootstrap_observation_position: ObservationPosition(0),
        bootstrap_publication: PublicationCursor(0),
    }
}

fn context() -> PublishedContext {
    PublishedContext {
        requests: vec![],
        session_id: "session".into(),
        agent_name: "synthetic".into(),
        project_root: Some("/repo".into()),
        version: "context-1".into(),
        main_session: true,
        complete_through_event: true,
        inventories: CurrentInventories::default(),
        session_selection: Some(ObservedAttribution {
            model: Some("current/selection".into()),
            fast_mode: Some(false),
        }),
    }
}

struct Reader(Vec<PublishedEvidenceEvent>);
impl PublishedEvidenceReader for Reader {
    fn read_batch(&self, _: PublicationRead) -> Result<PublishedEvidenceBatch, SourceError> {
        Ok(PublishedEvidenceBatch {
            next: self
                .0
                .last()
                .map_or(PublicationCursor(0), |event| event.cursor),
            events: self.0.clone(),
            has_more: false,
        })
    }
}

fn event(position: u64, payload: PublishedObservationPayload) -> PublishedEvidenceEvent {
    let identity = match &payload {
        PublishedObservationPayload::UserMessage(message)
        | PublishedObservationPayload::AssistantMessage(message)
        | PublishedObservationPayload::ToolCall(message) => message.id.clone(),
        PublishedObservationPayload::ResponseCycle(cycle) => cycle.id.clone(),
        _ => format!("host-{position}"),
    };
    PublishedEvidenceEvent {
        source_evidence_version: None,
        cursor: PublicationCursor(position),
        branch: "main".into(),
        context: Some(context()),
        change: PublishedEvidenceChange::ObservationUpsert(PublishedObservation {
            identity,
            native_order: NativeOrderKey(format!("order:{position}")),
            first_eligible_publication: Some(PublicationCursor(position)),
            revision: 1,
            eligible: true,
            payload_complete: true,
            validity: EvidenceValidity::Valid,
            payload,
        }),
    }
}

fn normalized(event: PublishedEvidenceEvent) -> SourceEvent {
    PublishedEvidenceSource::new(Reader(vec![event]), 1)
        .unwrap()
        .read_batch(PublicationCursor(0))
        .unwrap()
        .events
        .remove(0)
}

fn observation(event: PublishedEvidenceEvent) -> Observation {
    let SourceEvent::Observation {
        cursor,
        observation,
        ..
    } = normalized(event)
    else {
        panic!("expected an admitted observation")
    };
    observation.commit(ObservationPosition(cursor.0))
}

fn user(id: &str) -> PublishedObservationPayload {
    PublishedObservationPayload::UserMessage(
        normalize_user_message(id, "synthetic", "Public request", true).unwrap(),
    )
}

fn assistant(id: &str, mode: Option<bool>) -> PublishedObservationPayload {
    PublishedObservationPayload::AssistantMessage(
        normalize_assistant_message(
            id,
            "synthetic",
            "Public answer",
            true,
            &ObservedAttribution {
                model: Some("p/m".into()),
                fast_mode: mode,
            },
        )
        .unwrap(),
    )
}

fn call(id: &str) -> PublishedObservationPayload {
    PublishedObservationPayload::ToolCall(
        normalize_tool_call(
            id,
            "synthetic",
            "call-1",
            "shell",
            Some(json!("echo 'exact'\n")),
            &PublishedToolMetadata {
                attribution: ObservedAttribution {
                    model: Some("p/m".into()),
                    fast_mode: Some(false),
                },
                check_category: Some(ToolCategory::Shell),
                paths: Some(vec!["src/lib.rs".into()]),
                paths_complete: true,
                ..Default::default()
            },
            Some(false),
            true,
        )
        .unwrap(),
    )
}

fn cycle(id: &str) -> PublishedObservationPayload {
    let PublishedObservationPayload::UserMessage(message) = user("cycle-user") else {
        unreachable!()
    };
    PublishedObservationPayload::ResponseCycle(PublishedResponseCycle {
        id: id.into(),
        status: ResponseCycleStatus::Completed,
        messages: vec![message],
        attribution: ObservedAttribution {
            model: Some("p/actual-request-set".into()),
            fast_mode: Some(false),
        },
        native_reason: Some("native-completion".into()),
        native_error_code: None,
        started_at_ms: Some(4),
        ended_at_ms: Some(3),
        dependencies: vec![PublishedSourceEvidence {
            identity: "cycle-user".into(),
            version: EvidenceVersion("native-current-version".into()),
        }],
    })
}

fn result_event(position: u64, status: PublishedResultStatus) -> PublishedEvidenceEvent {
    PublishedEvidenceEvent {
        source_evidence_version: None,
        cursor: PublicationCursor(position),
        branch: "main".into(),
        context: Some(context()),
        change: PublishedEvidenceChange::Upsert(PublishedToolResult {
            identity: format!("result-{position}"),
            native_order: NativeOrderKey(format!("order:{position}")),
            first_terminal_publication: Some(PublicationCursor(position)),
            revision: 1,
            tool_name: Some("shell".into()),
            category: Some("shell".into()),
            metadata: Box::default(),
            arguments: Some(json!({"command": "test"})),
            content: Some("public output".into()),
            outcome: None,
            provider_executed: Some(false),
            status,
            finality: true,
            payload_complete: true,
            validity: EvidenceValidity::Valid,
        }),
    }
}

#[test]
fn compiler_admits_every_event_form_and_keeps_families_typed() {
    for name in [
        "user_message",
        "assistant_message",
        "tool_call",
        "tool_result",
        "turn_end",
        "session_start",
        "session_idle",
    ] {
        for filters in ["", "{}", "{models: [p/m], fast_mode: false}"] {
            let check = compile(&definition(&format!("  {name}: {filters}"), "", None)).unwrap();
            assert!(match check.trigger.family {
                TriggerFamily::Messages { .. } => matches!(
                    name,
                    "user_message" | "assistant_message" | "tool_call" | "tool_result"
                ),
                TriggerFamily::ResponseCycle(_) => name == "turn_end",
                TriggerFamily::Session { .. } => matches!(name, "session_start" | "session_idle"),
            });
        }
    }
    for events in [
        "  user_message:\n  turn_end:",
        "  session_start:\n  tool_call:",
        "  turn_end:\n  session_idle:",
        "  turn_end:\n  window: 2",
        "  session_start:\n  window: 2",
    ] {
        assert!(
            compile(&definition(events, "if: false\n", None)).is_err(),
            "{events}"
        );
    }
    for (name, filters) in [
        ("user_message", "{tools: [shell]}"),
        ("assistant_message", "{paths: [src/**]}"),
        ("tool_call", "{status: [completed]}"),
        ("session_idle", "{reason: [attached]}"),
        ("session_start", "{reason: []}"),
        ("session_start", "{reason: [attached, attached]}"),
        ("session_start", "{reason: [unknown]}"),
        ("turn_end", "{status: [unknown]}"),
        ("turn_end", "{fast_mode: 'false'}"),
    ] {
        assert!(
            compile(&definition(
                &format!("  {name}: {filters}"),
                "if: false\n",
                None
            ))
            .is_err(),
            "{name}: {filters}"
        );
    }
    for events in [
        "  user_message:\n  assistant_message:\n  tool_call:\n  tool_result:\n  window: {size: 4, min: 2}",
        "  session_start: {reason: [attached, resumed, new]}\n  session_idle:",
        "  turn_end: {status: [completed, error, cancelled]}",
    ] {
        assert!(compile(&definition(events, "", None)).is_ok());
        assert!(
            compile(
                &definition(events, "if: false\n", None).replace("type: status", "type: message")
            )
            .is_err()
        );
    }
}

#[test]
fn compiler_checks_each_real_root_in_direct_literal_dynamic_and_whole_forms() {
    for (events, allowed, forbidden) in [
        (
            "  tool_call:",
            vec![
                "message",
                "message['tool']['arguments']",
                "session.model",
                "session['fast_mode']",
            ],
            vec!["turn", "session.start_reason", "session['idle_seconds']"],
        ),
        (
            "  turn_end:",
            vec![
                "turn",
                "turn['messages'][0].content",
                "turn.messages.*.tool.arguments",
                "turn.status",
                "session.model",
            ],
            vec!["message", "window", "session.start_reason"],
        ),
        (
            "  session_start:\n  session_idle:",
            vec![
                "session",
                "session['start_reason']",
                "session.idle_seconds",
                "session.last_activity_at",
                "session.fast_mode",
            ],
            vec!["message", "turn", "window"],
        ),
        (
            "  session_start:",
            vec!["session.start_reason"],
            vec!["session.idle_seconds"],
        ),
        (
            "  session_idle:",
            vec!["session.idle_seconds"],
            vec!["session.start_reason"],
        ),
    ] {
        for expression in allowed {
            assert!(
                compile(&definition(
                    events,
                    "",
                    Some(&format!("\"${{{{ {expression} }}}}\""))
                ))
                .is_ok(),
                "{events}: {expression}"
            );
        }
        for expression in forbidden.into_iter().chain([
            "session[agent.name]",
            "session.misspelled",
            "session['misspelled']",
        ]) {
            let source = definition(
                events,
                "",
                Some(&format!("\"${{{{ false && {expression} }}}}\"")),
            );
            assert!(compile(&source).is_err(), "{events}: {expression}");
        }
    }
    for expression in [
        "turn.misspelled",
        "turn['reasoning']",
        "turn[agent.name]",
        "turn.messages[0].reasoning",
        "turn.messages.*.reasoning",
    ] {
        assert!(
            compile(&definition(
                "  turn_end:",
                "",
                Some(&format!("\"${{{{ {expression} }}}}\""))
            ))
            .is_err(),
            "{expression}"
        );
    }
}

#[test]
fn public_builders_preserve_text_input_and_reject_mismatched_or_open_records() {
    let PublishedObservationPayload::ToolCall(message) = call("call") else {
        unreachable!()
    };
    assert_eq!(message.value["tool"]["arguments"], json!("echo 'exact'\n"));
    let PublishedObservationPayload::UserMessage(user) = user("user") else {
        unreachable!()
    };
    assert!(user.value.get("model").is_none());
    assert!(user.value.get("fast_mode").is_none());
    let mut invalid_messages = Vec::new();
    for (field, value) in [
        ("id", json!("other")),
        ("kind", json!("tool_result")),
        ("role", json!("user")),
        ("reasoning", json!("private")),
        ("source", Value::Null),
        ("occurred_at", json!({"reasoning": "private"})),
        ("native_order", json!({"reasoning": "private"})),
    ] {
        let mut invalid = message.clone();
        invalid.value[field] = value;
        invalid_messages.push(invalid);
    }
    let mut invalid = message.clone();
    invalid.value["tool"]
        .as_object_mut()
        .unwrap()
        .remove("call_id");
    invalid_messages.push(invalid);
    let mut invalid = message.clone();
    invalid.value["tool"]["reasoning"] = json!("private");
    invalid_messages.push(invalid);
    let mut invalid = message.clone();
    invalid.value["tool"]["result"] = json!({"status": "completed"});
    invalid_messages.push(invalid);
    for invalid in invalid_messages {
        assert!(validate_public_message(&invalid, "tool_call").is_err());
        assert!(matches!(
            normalized(event(1, PublishedObservationPayload::ToolCall(invalid))),
            SourceEvent::UnsafeUpdate { .. }
        ));
    }
    let mut invalid_user = user.clone();
    invalid_user.value["fast_mode"] = json!(false);
    assert!(validate_public_message(&invalid_user, "user_message").is_err());
    let oversized =
        normalize_user_message("oversized", "synthetic", &"x".repeat(4 * 1024 * 1024), true);
    assert!(oversized.is_err());
}

#[test]
fn typed_publication_json_round_trips_and_rejects_unknown_envelope_fields() {
    for payload in [
        user("u"),
        assistant("a", Some(false)),
        call("c"),
        cycle("cycle"),
        PublishedObservationPayload::SessionStart {
            reason: SessionStartReason::Attached,
        },
        PublishedObservationPayload::SessionIdle {
            attachment_epoch: "e".into(),
            activity_revision: 1,
            idle_seconds: 120,
            last_activity_at_ms: None,
        },
    ] {
        let PublishedEvidenceChange::ObservationUpsert(published) = event(1, payload).change else {
            unreachable!()
        };
        let value = serde_json::to_value(&published).unwrap();
        assert_eq!(
            serde_json::from_value::<PublishedObservation>(value.clone()).unwrap(),
            published
        );
        let mut unknown = value;
        unknown["reasoning"] = json!("private");
        assert!(serde_json::from_value::<PublishedObservation>(unknown).is_err());
    }
}

#[test]
fn native_current_tokens_are_transient_and_do_not_change_hashes_or_checkpoints() {
    for (event_name, publication) in [
        ("user_message", event(1, user("u"))),
        ("assistant_message", event(1, assistant("a", Some(false)))),
        ("tool_call", event(1, call("c"))),
        ("turn_end", event(1, cycle("cycle"))),
        ("tool_result", result_event(1, PublishedResultStatus::Error)),
    ] {
        let active = activation(&definition(&format!("  {event_name}:"), "", None));
        let mut expected = None;
        for token in [
            None,
            Some(EvidenceVersion("opaque-native-version-a".into())),
            Some(EvidenceVersion("opaque-native-version-b".into())),
        ] {
            let mut published = publication.clone();
            published.source_evidence_version = token.clone();
            let SourceEvent::Observation {
                identity,
                revision,
                source_evidence_version,
                observation,
                ..
            } = normalized(published)
            else {
                panic!("expected admitted evidence")
            };
            assert_eq!(source_evidence_version, token);
            assert_ne!(source_evidence_version.as_ref(), Some(&observation.version));
            let draft_json = serde_json::to_value(&observation).unwrap();
            assert!(draft_json.get("source_evidence_version").is_none());
            let observation = observation.commit(ObservationPosition(1));
            let mut selection = SelectionState::default();
            let SelectionOutcome::Candidate(candidate) =
                selection.apply_published(&active, &identity, revision, &observation)
            else {
                panic!("expected selected evidence")
            };
            let progress = SourceProgress {
                publication_cursor: PublicationCursor(1),
                observation_position: ObservationPosition(1),
                selection,
            };
            let checkpoint = progress.encode().unwrap();
            let candidate_json = serde_json::to_value(&candidate).unwrap();
            assert!(candidate_json.get("source_evidence_version").is_none());
            let actual = (draft_json, candidate_json, checkpoint, progress.head());
            if let Some(expected) = &expected {
                assert_eq!(&actual, expected, "{event_name}");
            } else {
                expected = Some(actual);
            }
        }
    }
}

#[test]
fn cycle_uses_supplied_complete_request_attribution_and_exact_source_bindings() {
    let standalone = observation(event(1, user("cycle-user")));
    let observation = observation(event(2, cycle("cycle")));
    let ObservationKind::TurnEnd { turn } = &observation.kind else {
        panic!("expected cycle")
    };
    let ObservationKind::UserMessage(message) = standalone.kind else {
        unreachable!()
    };
    assert_eq!(turn["messages"][0], message.value);
    assert_eq!(turn["model"], "p/actual-request-set");
    assert_eq!(turn["fast_mode"], false);
    assert!(turn.get("native_reason").is_none());
    assert!(turn["messages"][0].get("event").is_none());
    let active = activation(&definition(
        "  turn_end: {models: [p/actual-request-set], fast_mode: false, status: [completed]}",
        "",
        None,
    ));
    let SelectionOutcome::Candidate(candidate) =
        SelectionState::default().apply_published(&active, "cycle", 1, &observation)
    else {
        panic!("expected cycle candidate")
    };
    assert!(candidate.messages.is_empty());
    assert!(candidate.context.value.get("message").is_none());
    assert!(candidate.context.value.get("window").is_none());
    assert_eq!(
        candidate.dependencies,
        vec![
            EvidenceDependency::Context {
                id: "user-messages-through-event".into(),
                version: EvidenceVersion("context-1".into())
            },
            EvidenceDependency::SourceEvidence {
                id: "cycle".into(),
                version: observation.version.clone()
            },
            EvidenceDependency::SourceEvidence {
                id: "cycle-user".into(),
                version: EvidenceVersion("native-current-version".into())
            }
        ]
    );
    for change in [
        "missing-binding",
        "extra-binding",
        "bad-member",
        "incomplete",
        "duplicate-binding",
    ] {
        let mut event = event(3, cycle("cycle"));
        let PublishedEvidenceChange::ObservationUpsert(published) = &mut event.change else {
            unreachable!()
        };
        let PublishedObservationPayload::ResponseCycle(cycle) = &mut published.payload else {
            unreachable!()
        };
        match change {
            "missing-binding" => cycle.dependencies.clear(),
            "extra-binding" => cycle.dependencies.push(PublishedSourceEvidence {
                identity: "not-a-member".into(),
                version: EvidenceVersion("v".into()),
            }),
            "bad-member" => cycle.messages[0].value["reasoning"] = json!("private"),
            "incomplete" => published.payload_complete = false,
            _ => cycle.dependencies.push(cycle.dependencies[0].clone()),
        }
        assert!(
            matches!(normalized(event), SourceEvent::UnsafeUpdate { .. }),
            "{change}"
        );
    }
}

#[test]
fn mixed_message_window_filters_each_alternative_before_counting() {
    let active = activation(&definition(
        "  user_message:\n  assistant_message: {models: [p/m], fast_mode: false}\n  tool_call: {tools: [shell], paths: ['src/**'], paths-ignore: ['**/generated/**']}\n  tool_result: {status: [error]}\n  window: {size: 3, min: 3}",
        "if: message.id != 'skip'\n",
        None,
    ));
    let mut state = SelectionState::default();
    for (position, payload, expected) in [
        (1, user("u"), "updated"),
        (2, assistant("unknown-mode", None), "rejected"),
        (3, assistant("a", Some(false)), "updated"),
        (4, call("c"), "candidate"),
    ] {
        let observation = observation(event(position, payload));
        let outcome = state.apply_published(
            &active,
            observation_message_id(&observation),
            1,
            &observation,
        );
        match (expected, outcome) {
            ("updated", SelectionOutcome::Updated) => {}
            ("rejected", SelectionOutcome::FilterRejected(diagnostic)) => {
                assert_eq!(diagnostic.scope, AttributionScope::Message);
                assert_eq!(diagnostic.reason, FilterReason::Unavailable);
            }
            ("candidate", SelectionOutcome::Candidate(candidate)) => {
                assert_eq!(
                    candidate
                        .messages
                        .iter()
                        .map(|message| message.id.as_str())
                        .collect::<Vec<_>>(),
                    ["u", "a", "c"]
                );
                assert_eq!(candidate.context.value["message"]["kind"], "tool_call");
                assert_eq!(
                    candidate.context.value["window"]["messages"]
                        .as_array()
                        .unwrap()
                        .len(),
                    3
                );
            }
            (_, outcome) => panic!("unexpected {outcome:?}"),
        }
    }
    assert_eq!(state.branches[&BranchId("main".into())].results.len(), 3);
    let skipped = observation(event(5, user("skip")));
    assert!(matches!(
        state.apply_published(&active, "skip", 1, &skipped),
        SelectionOutcome::ConditionFiltered(_)
    ));
    let filtered_result = observation(result_event(6, PublishedResultStatus::Completed));
    assert_eq!(
        state.apply_published(&active, "result-6", 1, &filtered_result),
        SelectionOutcome::Filtered
    );
    let result = observation(result_event(7, PublishedResultStatus::Error));
    let SelectionOutcome::Candidate(candidate) =
        state.apply_published(&active, "result-7", 1, &result)
    else {
        panic!("a matching result must join the mixed window")
    };
    assert_eq!(
        candidate
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "c", "result-7"]
    );
    let PublishedEvidenceChange::Upsert(result) =
        result_event(7, PublishedResultStatus::Error).change
    else {
        unreachable!()
    };
    assert_eq!(
        normalize_tool_result_message(&result, &context()).unwrap(),
        candidate.messages[2]
    );
}

#[test]
fn every_event_filter_uses_only_its_own_scope_and_missing_mode_never_means_false() {
    for (events, payload, scope) in [
        ("user_message", user("u"), AttributionScope::Message),
        (
            "assistant_message",
            assistant("a", None),
            AttributionScope::Message,
        ),
        ("tool_call", call("c"), AttributionScope::Message),
        ("turn_end", cycle("cycle"), AttributionScope::Turn),
        (
            "session_start",
            PublishedObservationPayload::SessionStart {
                reason: SessionStartReason::Attached,
            },
            AttributionScope::Session,
        ),
        (
            "session_idle",
            PublishedObservationPayload::SessionIdle {
                attachment_epoch: "e".into(),
                activity_revision: 0,
                idle_seconds: 120,
                last_activity_at_ms: None,
            },
            AttributionScope::Session,
        ),
    ] {
        for filters in ["{models: [current/selection]}", "{fast_mode: false}"] {
            let active = activation(&definition(&format!("  {events}: {filters}"), "", None));
            let mut publication = event(1, payload.clone());
            if matches!(scope, AttributionScope::Session) {
                publication.context.as_mut().unwrap().session_selection = None;
            }
            if let PublishedEvidenceChange::ObservationUpsert(published) = &mut publication.change {
                match &mut published.payload {
                    PublishedObservationPayload::ToolCall(message) => {
                        message.value.as_object_mut().unwrap().remove("model");
                        message.value.as_object_mut().unwrap().remove("fast_mode");
                    }
                    PublishedObservationPayload::ResponseCycle(cycle) => {
                        cycle.attribution = ObservedAttribution::default()
                    }
                    _ => {}
                }
            }
            let observation = observation(publication);
            let SelectionOutcome::FilterRejected(diagnostic) =
                SelectionState::default().apply_published(&active, "identity", 1, &observation)
            else {
                panic!("{events} {filters} must not fall back")
            };
            assert_eq!(diagnostic.scope, scope);
            assert!(matches!(
                diagnostic.reason,
                FilterReason::Unavailable | FilterReason::NoMatch
            ));
        }
    }
    for (events, payload) in [
        (
            "  session_start: {reason: [new]}",
            PublishedObservationPayload::SessionStart {
                reason: SessionStartReason::Attached,
            },
        ),
        ("  turn_end: {status: [cancelled, error]}", cycle("cycle")),
        ("  tool_call: {tools: [read]}", call("call")),
        ("  assistant_message:", user("user")),
    ] {
        assert_eq!(
            SelectionState::default().apply_published(
                &activation(&definition(events, "", None)),
                "identity",
                1,
                &observation(event(1, payload))
            ),
            SelectionOutcome::Filtered
        );
    }
    let active = activation(&definition(
        "  tool_call: {paths: ['src/**'], paths-ignore: ['**/generated/**']}",
        "",
        None,
    ));
    let mut payload = call("call");
    let PublishedObservationPayload::ToolCall(message) = &mut payload else {
        unreachable!()
    };
    message.value["tool"]["paths"] = json!(["src/generated/output.rs"]);
    let SelectionOutcome::FilterRejected(diagnostic) = SelectionState::default().apply_published(
        &active,
        "call",
        1,
        &observation(event(1, payload)),
    ) else {
        panic!("a veto path must reject the call")
    };
    assert_eq!(diagnostic.reason, FilterReason::IgnoredPath);
}

fn observation_message_id(observation: &Observation) -> &str {
    match &observation.kind {
        ObservationKind::UserMessage(message)
        | ObservationKind::AssistantMessage(message)
        | ObservationKind::ToolCall(message)
        | ObservationKind::ToolResult { message, .. } => &message.id,
        _ => panic!("expected message"),
    }
}

#[test]
fn session_events_use_current_selection_and_freeze_activity_without_fake_messages() {
    let active = activation(&definition(
        "  session_start: {reason: [attached], models: [current/selection], fast_mode: false}\n  session_idle: {fast_mode: false}",
        "",
        None,
    ));
    let mut state = SelectionState::default();
    for (position, payload) in [
        (
            1,
            PublishedObservationPayload::SessionStart {
                reason: SessionStartReason::Attached,
            },
        ),
        (
            2,
            PublishedObservationPayload::SessionIdle {
                attachment_epoch: "attachment-1".into(),
                activity_revision: 7,
                idle_seconds: 120,
                last_activity_at_ms: Some(99),
            },
        ),
    ] {
        let observation = observation(event(position, payload));
        let SelectionOutcome::Candidate(candidate) =
            state.apply_published(&active, &format!("host-{position}"), 1, &observation)
        else {
            panic!("expected host candidate")
        };
        assert!(candidate.messages.is_empty());
        assert!(candidate.context.value.get("message").is_none());
        assert!(candidate.context.value.get("turn").is_none());
        assert!(candidate.context.value.get("window").is_none());
        assert_eq!(candidate.context.value["session"]["fast_mode"], false);
        if position == 2 {
            assert!(
                candidate
                    .dependencies
                    .contains(&EvidenceDependency::SessionActivity {
                        attachment_epoch: "attachment-1".into(),
                        activity_revision: 7
                    })
            );
            assert!(
                candidate.context.value["session"]
                    .get("start_reason")
                    .is_none()
            );
        } else {
            assert_eq!(
                candidate.context.value["session"]["start_reason"],
                "attached"
            );
            assert!(
                candidate.context.value["session"]
                    .get("idle_seconds")
                    .is_none()
            );
        }
        assert_eq!(
            state.apply_published(&active, &format!("host-{position}"), 1, &observation),
            SelectionOutcome::Updated
        );
    }
    let mut unknown = event(
        3,
        PublishedObservationPayload::SessionStart {
            reason: SessionStartReason::Attached,
        },
    );
    unknown.context.as_mut().unwrap().session_selection = None;
    let SelectionOutcome::FilterRejected(diagnostic) =
        state.apply_published(&active, "host-3", 1, &observation(unknown))
    else {
        panic!("missing selection must not match false")
    };
    assert_eq!(diagnostic.scope, AttributionScope::Session);
    assert_eq!(diagnostic.reason, FilterReason::Unavailable);
}

#[test]
fn corrections_removals_and_checkpoint_restore_do_not_retrigger_or_reset_the_window() {
    let active = activation(&definition(
        "  user_message:\n  assistant_message:\n  tool_call:\n  window: {size: 3, min: 2}",
        "",
        None,
    ));
    let mut state = SelectionState::default();
    for (position, payload) in [(1, user("u")), (2, assistant("a", Some(false)))] {
        let observation = observation(event(position, payload));
        state.apply_published(
            &active,
            observation_message_id(&observation),
            1,
            &observation,
        );
    }
    let checkpoint = SourceProgress {
        publication_cursor: PublicationCursor(2),
        observation_position: ObservationPosition(2),
        selection: state,
    }
    .encode()
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&checkpoint).unwrap()["version"],
        1
    );
    let restored = SourceProgress::decode(&checkpoint).unwrap();
    assert_eq!(restored.encode().unwrap(), checkpoint);
    let mut state = restored.selection;
    let mut correction = event(3, assistant("a", Some(false)));
    let PublishedEvidenceChange::ObservationUpsert(published) = &mut correction.change else {
        unreachable!()
    };
    published.native_order = NativeOrderKey("order:2".into());
    published.first_eligible_publication = Some(PublicationCursor(2));
    published.revision = 1;
    let PublishedObservationPayload::AssistantMessage(message) = &mut published.payload else {
        unreachable!()
    };
    message.value["content"] = json!("Corrected public answer");
    assert_eq!(
        state.apply_published(&active, "a", 1, &observation(correction.clone())),
        SelectionOutcome::Updated
    );
    let next = observation(event(4, call("c")));
    let SelectionOutcome::Candidate(candidate) = state.apply_published(&active, "c", 1, &next)
    else {
        panic!("expected next mixed window")
    };
    assert_eq!(
        candidate.messages[1].value["content"],
        "Corrected public answer"
    );
    let PublishedEvidenceChange::ObservationUpsert(published) = &mut correction.change else {
        unreachable!()
    };
    published.eligible = false;
    published.revision = 2;
    correction.cursor = PublicationCursor(5);
    assert_eq!(
        state.apply_published(&active, "a", 2, &observation(correction)),
        SelectionOutcome::Updated
    );
    assert_eq!(state.branches[&BranchId("main".into())].results.len(), 2);
    assert!(state.remove_result(&BranchId("main".into()), "u", 3));
    let stale = observation(event(6, user("u")));
    assert_eq!(
        state.apply_published(&active, "u", 2, &stale),
        SelectionOutcome::Updated
    );
    assert_eq!(state.branches[&BranchId("main".into())].results.len(), 1);
}

#[test]
fn new_events_without_post_baseline_first_eligibility_cannot_select_history() {
    for (events, payload) in [
        ("  user_message:", user("u")),
        ("  assistant_message:", assistant("a", Some(false))),
        ("  tool_call:", call("c")),
        ("  turn_end:", cycle("cycle")),
        (
            "  session_start:",
            PublishedObservationPayload::SessionStart {
                reason: SessionStartReason::Attached,
            },
        ),
        (
            "  session_idle:",
            PublishedObservationPayload::SessionIdle {
                attachment_epoch: "e".into(),
                activity_revision: 1,
                idle_seconds: 120,
                last_activity_at_ms: None,
            },
        ),
    ] {
        let mut active = activation(&definition(events, "", None));
        active.bootstrap_publication = PublicationCursor(10);
        for first in [None, Some(PublicationCursor(10))] {
            let mut event = event(20, payload.clone());
            let PublishedEvidenceChange::ObservationUpsert(published) = &mut event.change else {
                unreachable!()
            };
            published.first_eligible_publication = first;
            assert_eq!(
                SelectionState::default().apply_published(
                    &active,
                    "identity",
                    1,
                    &observation(event)
                ),
                SelectionOutcome::Updated
            );
        }
        assert!(
            matches!(
                SelectionState::default().apply_published(
                    &active,
                    "identity",
                    1,
                    &observation(event(20, payload))
                ),
                SelectionOutcome::Candidate(_)
            ),
            "{events}"
        );
    }
}

#[test]
fn additive_event_metadata_preserves_old_result_shapes_and_seed_state() {
    let result = observation(result_event(1, PublishedResultStatus::Error));
    let value = serde_json::to_value(&result).unwrap();
    assert!(value.get("first_eligible_publication").is_none());
    let restored: Observation = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(restored, result);
    assert_eq!(serde_json::to_value(restored).unwrap(), value);
    let mut legacy_context = context();
    legacy_context.session_selection = None;
    assert!(
        serde_json::to_value(legacy_context)
            .unwrap()
            .get("session_selection")
            .is_none()
    );
    let active = activation(&definition(
        "  tool_result:\n  user_message:\n  assistant_message:\n  window: {size: 3, min: 1}",
        "",
        None,
    ));
    let mut state = SelectionState::default();
    let SelectionOutcome::Candidate(seed) = state.apply_published(&active, "result-1", 1, &result)
    else {
        panic!("the existing result seed remains reachable")
    };
    let checkpoint = SourceProgress {
        publication_cursor: PublicationCursor(1),
        observation_position: ObservationPosition(1),
        selection: state,
    }
    .encode()
    .unwrap();
    let mut restored = SourceProgress::decode(&checkpoint).unwrap().selection;
    let mut historical = observation(event(2, user("old-user")));
    historical.first_eligible_publication = None;
    assert_eq!(
        restored.apply_published(&active, "old-user", 1, &historical),
        SelectionOutcome::Updated
    );
    assert_eq!(restored.branches[&BranchId("main".into())].results.len(), 1);
    assert_eq!(seed.messages[0].id, "result-1");
    let live = observation(event(3, assistant("live", Some(false))));
    let SelectionOutcome::Candidate(next) = restored.apply_published(&active, "live", 1, &live)
    else {
        panic!("live events must continue from the restored result window")
    };
    assert_eq!(
        next.messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["result-1", "live"]
    );
    let live_value = serde_json::to_value(&live).unwrap();
    assert_eq!(live_value["first_eligible_publication"], 3);
    assert_eq!(
        serde_json::from_value::<Observation>(live_value).unwrap(),
        live
    );
}

#[test]
fn new_publications_reject_unsafe_source_admission_and_invalid_idle_boundaries() {
    for change in [
        "identity",
        "order",
        "owner",
        "validity",
        "source",
        "future-eligibility",
        "payload-source",
    ] {
        let mut event = event(1, user("user"));
        let PublishedEvidenceChange::ObservationUpsert(published) = &mut event.change else {
            unreachable!()
        };
        match change {
            "identity" => published.identity.clear(),
            "order" => published.native_order.0.clear(),
            "owner" => event.context.as_mut().unwrap().main_session = false,
            "validity" => published.validity = EvidenceValidity::UnsafeOrder,
            "source" => event.context.as_mut().unwrap().agent_name.clear(),
            "future-eligibility" => {
                published.first_eligible_publication = Some(PublicationCursor(2))
            }
            _ => {
                let PublishedObservationPayload::UserMessage(message) = &mut published.payload
                else {
                    unreachable!()
                };
                message.value["source"] = json!("different");
            }
        }
        assert!(
            matches!(normalized(event), SourceEvent::UnsafeUpdate { .. }),
            "{change}"
        );
    }
    for (epoch, seconds) in [("", 120), ("epoch", 119)] {
        assert!(matches!(
            normalized(event(
                1,
                PublishedObservationPayload::SessionIdle {
                    attachment_epoch: epoch.into(),
                    activity_revision: 0,
                    idle_seconds: seconds,
                    last_activity_at_ms: None
                }
            )),
            SourceEvent::UnsafeUpdate { .. }
        ));
    }
    let mut withdrawal = event(1, cycle("cycle"));
    let PublishedEvidenceChange::ObservationUpsert(published) = &mut withdrawal.change else {
        unreachable!()
    };
    published.eligible = false;
    published.payload_complete = false;
    assert_eq!(observation(withdrawal).kind, ObservationKind::Other);
}

#[test]
fn every_new_event_keeps_lazy_inventory_capture_before_condition_or_candidate_freeze() {
    for (events, payload) in [
        ("  user_message:", user("u")),
        ("  assistant_message:", assistant("a", Some(false))),
        ("  tool_call:", call("c")),
        ("  turn_end:", cycle("cycle")),
        (
            "  session_start:",
            PublishedObservationPayload::SessionStart {
                reason: SessionStartReason::Attached,
            },
        ),
        (
            "  session_idle:",
            PublishedObservationPayload::SessionIdle {
                attachment_epoch: "e".into(),
                activity_revision: 1,
                idle_seconds: 120,
                last_activity_at_ms: None,
            },
        ),
    ] {
        for condition in [true, false] {
            let active = activation(&definition(
                events,
                if condition {
                    "if: global.skills_complete\n"
                } else {
                    ""
                },
                Some("'${{ global.skills_complete }}'"),
            ));
            let mut captures = 0;
            let observation = observation(event(1, payload.clone()));
            let mut state = SelectionState::default();
            let outcome = state
                .apply_published_with_inventories(&active, "identity", 1, &observation, &mut || {
                    captures += 1;
                    let mut inventories = CurrentInventories::default();
                    inventories.global.skills_complete = true;
                    Ok::<_, ()>(Some(inventories))
                })
                .unwrap();
            let SelectionOutcome::Candidate(candidate) = outcome else {
                panic!("expected event candidate")
            };
            assert_eq!(captures, 1, "{events}");
            assert_eq!(candidate.context.value["global"]["skills_complete"], true);
            assert_eq!(
                observation.context.value["global"]["skills_complete"],
                false
            );
        }
        let blocked = activation(&definition(
            events,
            "if: false\n",
            Some("'${{ global.skills }}'"),
        ));
        let outcome = SelectionState::default()
            .apply_published_with_inventories(
                &blocked,
                "identity",
                1,
                &observation(event(1, payload)),
                &mut || -> Result<Option<CurrentInventories>, ()> {
                    panic!("a rejected event must not capture candidate inventories")
                },
            )
            .unwrap();
        assert!(matches!(outcome, SelectionOutcome::ConditionFiltered(_)));
    }
    let active = activation(&definition(
        "  user_message:\n  window: {size: 3, min: 3}",
        "",
        Some("'${{ global.skills }}'"),
    ));
    let mut captures = 0;
    let outcome = SelectionState::default()
        .apply_published_with_inventories(
            &active,
            "u",
            1,
            &observation(event(1, user("u"))),
            &mut || {
                captures += 1;
                Ok::<_, ()>(None)
            },
        )
        .unwrap();
    assert_eq!(outcome, SelectionOutcome::Updated);
    assert_eq!(captures, 0);
}
