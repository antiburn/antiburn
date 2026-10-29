use super::*;
use crate::store::Incarnation;
use crate::store::native_tool_results::observers;
use crate::store::native_tool_results::source_delivery::opencode_v2::{
    OpenCodeV2AdmittedSource, OpenCodeV2CaptureClaim, OpenCodeV2CaptureStart,
};
use antiburn_checks::context::ObservedAttribution;
use antiburn_checks::domain::{ObservationDraft, ObservationKind};
use antiburn_checks::source::{PublicationCursor, PublishedEvidenceSource, SourceEvent};

const FORMATS: [SourceFormat; 5] = [
    SourceFormat::ClaudeJsonl,
    SourceFormat::CodexRolloutJsonl,
    SourceFormat::PiV3Jsonl,
    SourceFormat::OpenCodeSqliteV1,
    SourceFormat::OpenCodeSqliteV2,
];

fn position(format: SourceFormat, index: u64) -> ToolResultPosition {
    match format {
        SourceFormat::OpenCodeSqliteV1 => ToolResultPosition::OpenCodeEvent {
            sequence: index,
            item_index: 0,
        },
        SourceFormat::OpenCodeSqliteV2 => ToolResultPosition::OpenCodeV2Event {
            sequence: index,
            item_index: 0,
        },
        SourceFormat::ClaudeJsonl | SourceFormat::CodexRolloutJsonl | SourceFormat::PiV3Jsonl => {
            ToolResultPosition::FileBytes {
                record_start: index * 100,
                record_end: index * 100 + 99,
                item_index: 0,
            }
        }
        _ => panic!("unsupported selection fixture format"),
    }
}

fn attribution(provider: &str, model: &str, fast_mode: bool) -> NativeRequestAttribution {
    NativeRequestAttribution {
        provider: Some(provider.into()),
        model: Some(model.into()),
        fast_mode: Some(fast_mode),
    }
}

fn selected(model: &str, fast_mode: bool) -> ObservedAttribution {
    ObservedAttribution {
        model: Some(format!("selection/{model}")),
        fast_mode: Some(fast_mode),
    }
}

fn source_fixture(format: SourceFormat) -> (Store, SessionKey) {
    let agent = match format {
        SourceFormat::ClaudeJsonl => "claude-code",
        SourceFormat::CodexRolloutJsonl => "codex",
        SourceFormat::PiV3Jsonl => "pi",
        SourceFormat::OpenCodeSqliteV1 | SourceFormat::OpenCodeSqliteV2 => "opencode",
        _ => panic!("unsupported selection fixture format"),
    };
    let store = Store::open_in_memory(std::path::Path::new("/synthetic/selection")).unwrap();
    let key = SessionKey::new("native", agent, "session");
    store
        .upsert_sessions(
            &[SessionRecord {
                key: key.clone(),
                source_kind: if agent == "opencode" {
                    "providerDb"
                } else {
                    "file"
                }
                .into(),
                source_label: match format {
                    SourceFormat::OpenCodeSqliteV1 => "opencode:session".into(),
                    SourceFormat::OpenCodeSqliteV2 => "opencode:session:open_code_v2_sqlite".into(),
                    _ => "/synthetic/session.jsonl".into(),
                },
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: Some("/synthetic/repo".into()),
                surface: "cli".into(),
                updated_at_epoch: Some(1000),
                activity_cursor: String::new(),
                activity_source: "native".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some("synthetic".into()),
            }],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    seed_delivery(&store, &key, format);
    (store, key)
}

fn seed_delivery(store: &Store, key: &SessionKey, format: SourceFormat) {
    let sql = match format {
        SourceFormat::OpenCodeSqliteV1 => "INSERT INTO opencode_journal_delivery
            (environment_key, agent, session_id, incarnation, source_label, cwd, parser_revision, main_session)
            SELECT environment_key, agent, session_id, incarnation, source_label, cwd, ?4, 1 FROM session
            WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 ON CONFLICT DO NOTHING",
        SourceFormat::OpenCodeSqliteV2 => "INSERT INTO opencode_v2_capture
            (environment_key, agent, session_id, incarnation, source_generation, source_label, cwd,
             capture_epoch, claim_fence, history_gap)
            SELECT environment_key, agent, session_id, incarnation, source_generation, source_label, cwd,
                'synthetic-capture', 1, 0 FROM session
            WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 ON CONFLICT DO NOTHING",
        SourceFormat::ClaudeJsonl | SourceFormat::CodexRolloutJsonl | SourceFormat::PiV3Jsonl => {
            "INSERT INTO native_file_delivery
            (environment_key, agent, session_id, incarnation, source_label, cwd, parser_revision, active_until_epoch)
            SELECT environment_key, agent, session_id, incarnation, source_label, cwd, ?4, 10000 FROM session
            WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 ON CONFLICT DO NOTHING"
        }
        _ => panic!("unsupported selection fixture format"),
    };
    let connection = store.lock();
    if format == SourceFormat::OpenCodeSqliteV2 {
        connection
            .execute(sql, params![key.environment_key, key.agent, key.session_id])
            .unwrap();
    } else {
        connection
            .execute(
                sql,
                params![
                    key.environment_key,
                    key.agent,
                    key.session_id,
                    PARSER_REVISION
                ],
            )
            .unwrap();
    }
}

fn select(
    store: &Store,
    key: &SessionKey,
    format: SourceFormat,
    index: u64,
    model: &str,
    fast_mode: bool,
) {
    let mut connection = store.lock();
    let tx = connection.transaction().unwrap();
    observers::selection_in(
        &tx,
        key,
        &NativeSessionSelectionObservation {
            source_format: format,
            session_id: key.session_id.clone(),
            position: position(format, index),
            selection: attribution("selection", model, fast_mode),
        },
    )
    .unwrap();
    tx.commit().unwrap();
}

fn frozen_at(
    store: &Store,
    key: &SessionKey,
    format: SourceFormat,
    index: u64,
) -> Option<ObservedAttribution> {
    observers::frozen_selection_json_in(
        &store.lock(),
        key,
        &serde_json::to_string(&format).unwrap(),
        "unpublished-identity",
        position(format, index),
    )
    .unwrap()
    .map(|json| serde_json::from_str(&json).unwrap())
}

fn messages(
    format: SourceFormat,
    key: &SessionKey,
    prefix: &str,
    first: u64,
) -> Vec<NativeMessageObservation> {
    let NativeEvidenceFact::Message(NativeMessageObservation::User(mut request)) =
        user(1, "Inspect the current directory.").fact
    else {
        unreachable!()
    };
    request.message.source_format = format;
    request.message.session_id = key.session_id.clone();
    request.message.message_id = format!("{prefix}-user");
    request.message.position = match position(format, first) {
        ToolResultPosition::FileBytes {
            record_start,
            record_end,
            ..
        } => UserMessagePosition::FileBytes {
            record_start,
            record_end,
        },
        ToolResultPosition::OpenCodeEvent { sequence, .. } => {
            UserMessagePosition::OpenCodeEvent { sequence }
        }
        ToolResultPosition::OpenCodeV2Event { sequence, .. } => {
            UserMessagePosition::OpenCodeV2Event { sequence }
        }
        ToolResultPosition::Jsonl { .. } => unreachable!(),
    };
    request.message.origin = match format {
        SourceFormat::ClaudeJsonl => UserMessageOrigin::ClaudeUserContent,
        SourceFormat::CodexRolloutJsonl => UserMessageOrigin::CodexRetainedUserText,
        SourceFormat::PiV3Jsonl => UserMessageOrigin::PiUserContent,
        SourceFormat::OpenCodeSqliteV1 | SourceFormat::OpenCodeSqliteV2 => {
            UserMessageOrigin::OpenCodeUserText
        }
        _ => unreachable!(),
    };
    assert!(request.message.is_bounded());
    let context = |index| NativeEventContext {
        first_eligible_position: position(format, index),
        main_session: true,
        complete_through_event: true,
        user_messages: vec![request.message.clone()],
    };
    request.context = context(first);
    let tool_name = match format {
        SourceFormat::ClaudeJsonl => "Bash",
        SourceFormat::CodexRolloutJsonl => "exec_command",
        _ => "bash",
    };
    let input = if format == SourceFormat::CodexRolloutJsonl {
        serde_json::json!({"cmd": "pwd"})
    } else {
        serde_json::json!({"command": "pwd"})
    };
    let facts = NativeToolFacts {
        requester: attribution("requester", "model", false),
        category: Some(CheckToolCategory::Shell),
        operation_paths: None,
    };
    let assistant = PublicMessageObservation {
        source_format: format,
        session_id: key.session_id.clone(),
        record_id: format!("{prefix}-assistant"),
        part_id: Some(format!("{prefix}-text")),
        call_id: None,
        position: position(format, first + 1),
        revision: 1,
        change: ToolResultChange::Update,
        eligible: true,
        completeness: ToolResultCompleteness::Complete,
        context: Some(context(first + 1)),
        payload: PublicMessagePayload::AssistantText {
            parts: vec![PublicTextPart {
                item_index: 0,
                native_part_id: None,
                text: "I will inspect the directory.".into(),
            }],
            generator: attribution("generator", "model", false),
        },
    };
    let call = PublicMessageObservation {
        record_id: format!("{prefix}-call-owner"),
        part_id: Some(format!("{prefix}-call-part")),
        call_id: Some(format!("{prefix}-call")),
        position: position(format, first + 2),
        context: Some(context(first + 2)),
        payload: PublicMessagePayload::ToolCall {
            tool_name: tool_name.into(),
            input: Some(NativeToolInput::Json(input.clone())),
            facts: Some(facts.clone()),
            provider_executed: Some(false),
        },
        ..assistant.clone()
    };
    let result = ToolResultObservation {
        source_format: format,
        session_id: key.session_id.clone(),
        record_id: Some(format!("{prefix}-result-owner")),
        call_id: Some(format!("{prefix}-call")),
        result_id: Some(format!("{prefix}-result")),
        position: position(format, first + 3),
        revision: 1,
        change: ToolResultChange::Update,
        tool_name: Some(tool_name.into()),
        tool_category: None,
        arguments: Some(input),
        content: Some("/synthetic/repo".into()),
        status: ToolResultStatus::Success,
        finality: true,
        completeness: ToolResultCompleteness::Complete,
        context: Some(ToolResultContext {
            result_position: position(format, first + 3),
            main_session: true,
            complete_through_event: true,
            user_messages: vec![request.message.clone()],
        }),
        outcome: None,
        provider_executed: Some(false),
        facts: Some(facts),
    };
    vec![
        NativeMessageObservation::User(request),
        NativeMessageObservation::Public(assistant),
        NativeMessageObservation::Public(call),
        NativeMessageObservation::ToolResult(result),
    ]
}

fn publish_messages(
    store: &Store,
    key: &SessionKey,
    format: SourceFormat,
    messages: Vec<NativeMessageObservation>,
    cycle: bool,
) {
    let mut events = messages
        .iter()
        .cloned()
        .map(|message| NativeEvidenceEvent {
            live_eligible: true,
            fact: NativeEvidenceFact::Message(message),
        })
        .collect::<Vec<_>>();
    if cycle {
        let NativeMessageObservation::User(request) = &messages[0] else {
            unreachable!()
        };
        let end = position(format, 6);
        events.push(NativeEvidenceEvent {
            live_eligible: true,
            fact: NativeEvidenceFact::ResponseCycle(ResponseCycleObservation {
                source_format: format,
                session_id: key.session_id.clone(),
                key: match format {
                    SourceFormat::CodexRolloutJsonl => NativeCycleKey::CodexTurn {
                        turn_id: "turn-one".into(),
                    },
                    SourceFormat::OpenCodeSqliteV2 => NativeCycleKey::OpenCodeV2Execution {
                        start_event_id: "execution-one".into(),
                        start_sequence: 2,
                    },
                    _ => panic!("unsupported cycle fixture format"),
                },
                start_position: position(format, 2),
                position: end,
                revision: 1,
                change: ToolResultChange::Update,
                status: ResponseCycleStatus::Completed,
                native_reason: None,
                native_error_code: None,
                started_at_ms: Some(2000),
                ended_at_ms: Some(3000),
                requester: attribution("requester", "cycle-model", false),
                context: NativeEventContext {
                    first_eligible_position: end,
                    main_session: true,
                    complete_through_event: true,
                    user_messages: vec![request.message.clone()],
                },
                messages: messages.clone(),
            }),
        });
    }
    let mut connection = store.lock();
    let tx = connection.transaction().unwrap();
    assert!(
        reconcile_evidence_in(
            &tx,
            key,
            1,
            1,
            &NativeEvidenceBatch {
                source_format: format,
                events,
                history_complete: true
            },
            PublicationPolicy {
                suppress_coverage: true,
                ..Default::default()
            },
            2000
        )
        .unwrap()
    );
    tx.commit().unwrap();
}

fn observation(event: &SourceEvent) -> &ObservationDraft {
    let SourceEvent::Observation { observation, .. } = event else {
        panic!("expected admitted observation, got {event:?}")
    };
    observation
}

fn assert_session(observation: &ObservationDraft, model: Option<&str>, fast_mode: Option<bool>) {
    assert_eq!(
        observation.context.value["session"]["model"].as_str(),
        model
    );
    assert_eq!(
        observation.context.value["session"]["fast_mode"].as_bool(),
        fast_mode
    );
    assert_eq!(observation.context.value["session"]["id"], "session");
}

#[test]
fn native_formats_freeze_selection_before_delayed_consumption_and_keep_request_attribution() {
    for format in FORMATS {
        let (store, key) = source_fixture(format);
        select(&store, &key, format, 1, "model-a", true);
        assert_eq!(frozen_at(&store, &key, format, 0), None, "{format:?}");
        assert_eq!(
            frozen_at(&store, &key, format, 1),
            Some(selected("model-a", true)),
            "{format:?}"
        );
        let cycle = matches!(
            format,
            SourceFormat::CodexRolloutJsonl | SourceFormat::OpenCodeSqliteV2
        );
        publish_messages(
            &store,
            &key,
            format,
            messages(format, &key, "original", 2),
            cycle,
        );
        select(&store, &key, format, 10, "model-b", false);

        let raw = store.native_evidence_publications_after(0, 20).unwrap();
        assert_eq!(raw.len(), 4 + usize::from(cycle), "{format:?}");
        assert!(
            raw.iter()
                .all(|event| event.session_selection == Some(selected("model-a", true))),
            "{format:?}"
        );
        let results = store.native_tool_result_publications_after(0, 20).unwrap();
        assert_eq!(results.len(), 1, "{format:?}");
        assert_eq!(
            results[0].session_selection,
            Some(selected("model-a", true))
        );

        let source =
            PublishedEvidenceSource::new(StorePublishedEvidenceReader::new(&store), 20).unwrap();
        let page = source.read_batch(PublicationCursor(0)).unwrap();
        assert_eq!(page.events.len(), raw.len());
        for event in &page.events {
            let observation = observation(event);
            assert!(observation.complete, "{format:?}: {observation:?}");
            assert_session(observation, Some("selection/model-a"), Some(true));
            assert_eq!(
                observation.context.value["session"]["requests"][0]["text"],
                "Inspect the current directory."
            );
            match &observation.kind {
                ObservationKind::UserMessage(message) => {
                    assert!(message.value.get("model").is_none())
                }
                ObservationKind::AssistantMessage(message) => {
                    assert_eq!(message.value["model"], "generator/model");
                    assert_eq!(message.value["fast_mode"], false);
                }
                ObservationKind::ToolCall(message)
                | ObservationKind::ToolResult { message, .. } => {
                    assert_eq!(message.value["model"], "requester/model");
                    assert_eq!(message.value["fast_mode"], false);
                }
                ObservationKind::TurnEnd { turn } => {
                    assert_eq!(turn["model"], "requester/cycle-model");
                    assert_eq!(turn["fast_mode"], false);
                    assert_eq!(turn["messages"][1]["model"], "generator/model");
                    assert_eq!(turn["messages"][3]["model"], "requester/model");
                }
                kind => panic!("unexpected selection fixture kind: {kind:?}"),
            }
        }
        let retained = messages(format, &key, "retained", 2).remove(0);
        publish_messages(&store, &key, format, vec![retained], false);
        let retained = source.read_batch(page.next).unwrap();
        assert_eq!(retained.events.len(), 1);
        assert_session(observation(&retained.events[0]), None, None);
        assert_eq!(frozen_at(&store, &key, format, 9), None);
        assert_eq!(
            frozen_at(&store, &key, format, 10),
            Some(selected("model-b", false))
        );
    }
}

fn correct(messages: &mut [NativeMessageObservation], format: SourceFormat, first: u64) {
    for (offset, message) in messages.iter_mut().enumerate() {
        let current = position(format, first + offset as u64);
        match message {
            NativeMessageObservation::User(message) => {
                message.revision = 2;
                message.message.parts[0].text = "Inspect the directory again.".into();
                let ToolResultPosition::FileBytes {
                    record_start,
                    record_end,
                    ..
                } = current
                else {
                    unreachable!()
                };
                message.message.position = UserMessagePosition::FileBytes {
                    record_start,
                    record_end,
                };
            }
            NativeMessageObservation::Public(message) => {
                message.revision = 2;
                message.position = current;
            }
            NativeMessageObservation::ToolResult(message) => {
                message.revision = 2;
                message.position = current;
                message.content = Some("Corrected directory output.".into());
            }
        }
    }
}

#[test]
fn disk_reopen_and_startup_results_keep_known_and_unknown_selection_on_correction() {
    let directory = tempfile::tempdir().unwrap();
    let (store, key, _) = fixture_with_store(Store::open(directory.path()).unwrap());
    let format = SourceFormat::ClaudeJsonl;
    seed_delivery(&store, &key, format);
    let boundary = {
        let mut connection = store.lock();
        let transaction = connection.transaction().unwrap();
        assert!(
            crate::store::native_tool_results::request_origin::ensure_boundary_in(
                &transaction,
                &key,
                1,
                1,
            )
            .unwrap()
        );
        let boundary = crate::store::native_tool_results::request_origin::boundary_sql("s");
        let sequence = transaction
            .query_row(
                &format!(
                    "SELECT {boundary} FROM session s
                     WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3"
                ),
                params![key.environment_key, key.agent, key.session_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        transaction.commit().unwrap();
        sequence
    };
    let mut early = messages(format, &key, "early", 2);
    publish_messages(&store, &key, format, early.clone(), false);
    select(&store, &key, format, 10, "model-a", true);
    let mut later = messages(format, &key, "later", 11);
    publish_messages(&store, &key, format, later.clone(), false);
    select(&store, &key, format, 20, "model-b", false);
    correct(&mut early, format, 21);
    correct(&mut later, format, 31);
    publish_messages(&store, &key, format, early, false);
    publish_messages(&store, &key, format, later, false);
    drop(store);

    let store = Store::open(directory.path()).unwrap();
    let raw = store
        .native_evidence_publications_after(boundary, 30)
        .unwrap();
    assert_eq!(raw.len(), 16);
    for event in raw.iter().filter(|event| event.native_identity.is_some()) {
        let expected = event
            .native_identity
            .as_ref()
            .unwrap()
            .contains("later-")
            .then(|| selected("model-a", true));
        assert_eq!(event.session_selection, expected);
    }
    let legacy = store
        .native_tool_result_publications_after(boundary, 20)
        .unwrap();
    assert_eq!(legacy.len(), 4);
    for result in &legacy {
        let expected = result
            .native_identity
            .as_ref()
            .unwrap()
            .contains("later-")
            .then(|| selected("model-a", true));
        assert_eq!(result.session_selection, expected);
    }

    let page = PublishedEvidenceSource::new(StorePublishedEvidenceReader::new(&store), 30)
        .unwrap()
        .read_batch(PublicationCursor(boundary as u64))
        .unwrap();
    assert_eq!(page.events.len(), 16);
    for event in &page.events {
        let SourceEvent::Observation { identity, .. } = event else {
            panic!("expected native observation")
        };
        let known = identity.contains("later-");
        assert_session(
            observation(event),
            known.then_some("selection/model-a"),
            known.then_some(true),
        );
    }

    let reader = StorePublishedEvidenceReader::new(&store);
    let startup = reader.recent_results_before(&key, None).unwrap();
    assert_eq!(startup.len(), 2);
    assert_session(observation(&startup[0]), None, None);
    assert_session(
        observation(&startup[1]),
        Some("selection/model-a"),
        Some(true),
    );
    for event in &startup {
        let ObservationKind::ToolResult { message, .. } = &observation(event).kind else {
            panic!("expected startup result")
        };
        assert_eq!(message.value["model"], "requester/model");
        assert_eq!(message.value["fast_mode"], false);
    }
    let before_later = reader
        .recent_results_before(&key, Some(legacy.last().unwrap().publication_sequence))
        .unwrap();
    assert_eq!(before_later.len(), 1);
    assert_session(observation(&before_later[0]), None, None);
}

#[test]
fn frozen_selection_rejects_changed_source_bindings_for_every_native_format() {
    for format in FORMATS {
        for mutation in [
            "UPDATE session SET source_label = source_label || ':replaced'",
            "UPDATE session SET cwd = '/synthetic/other'",
            "UPDATE session SET cwd = NULL",
            "UPDATE session SET incarnation = incarnation + 1",
        ] {
            let (store, key) = source_fixture(format);
            select(&store, &key, format, 1, "model-a", true);
            assert_eq!(
                frozen_at(&store, &key, format, 2),
                Some(selected("model-a", true))
            );
            store.lock().execute(mutation, []).unwrap();
            assert_eq!(
                frozen_at(&store, &key, format, 2),
                None,
                "{format:?}: {mutation}"
            );
        }
    }
}

fn v2_capture_fixture() -> (Store, SessionKey, ActivationId, OpenCodeV2CaptureStart) {
    let (store, key) = source_fixture(SourceFormat::OpenCodeSqliteV2);
    let activation = ActivationId {
        definition_id: "one".into(),
        generation: Generation(1),
        scope: ActivationScope {
            environment: "native".into(),
            session_cwd: "/synthetic/repo".into(),
            run: RunScope::Live,
        },
    };
    attach(&store, &key, &activation, 1000);
    let admitted_source = OpenCodeV2AdmittedSource {
        database_path: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("synthetic/opencode.db"),
        file_identity: "synthetic-database-identity".into(),
    };
    assert!(admitted_source.database_path.is_absolute());
    let cursor = format!(
        "opencode-admission-v2:{}",
        serde_json::to_string(&(
            &admitted_source.database_path,
            &key.session_id,
            &admitted_source.file_identity,
            "synthetic-scope"
        ))
        .unwrap()
    );
    store
        .lock()
        .execute("UPDATE session SET activity_cursor = ?1", [cursor])
        .unwrap();
    let (incarnation, source_generation, source_label, cwd): (u64, i64, String, String) = store
        .lock()
        .query_row(
            "SELECT incarnation, source_generation, source_label, cwd FROM session",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let start = OpenCodeV2CaptureStart {
        key: key.clone(),
        incarnation: Incarnation(incarnation),
        source_generation,
        source_label,
        cwd,
        admitted_source,
        baseline_sequence: Some(0),
    };
    (store, key, activation, start)
}

#[test]
fn v2_capture_gap_and_cold_attach_require_a_new_explicit_selection() {
    for recovered_epoch in ["before-gap", "after-gap"] {
        let format = SourceFormat::OpenCodeSqliteV2;
        let (store, key, _, mut start) = v2_capture_fixture();
        let claim = store
            .start_opencode_v2_capture_epoch(std::slice::from_ref(&start), "before-gap", 100, 60)
            .unwrap()
            .unwrap()
            .remove(0);
        select(&store, &key, format, 1, "model-a", true);
        publish_messages(
            &store,
            &key,
            format,
            messages(format, &key, "before-gap", 2),
            false,
        );
        assert_eq!(
            store
                .mark_opencode_v2_capture_gap(std::slice::from_ref(&claim), "capture-lost", 101)
                .unwrap(),
            1
        );
        start.baseline_sequence = Some(10);
        store
            .start_opencode_v2_capture_epoch(std::slice::from_ref(&start), recovered_epoch, 102, 60)
            .unwrap()
            .unwrap();
        assert_eq!(frozen_at(&store, &key, format, 11), None);
        let after_gap = store
            .native_evidence_publications_after(0, 20)
            .unwrap()
            .last()
            .unwrap()
            .publication_sequence;
        publish_messages(
            &store,
            &key,
            format,
            messages(format, &key, "gap-unknown", 11),
            false,
        );
        select(&store, &key, format, 20, "model-b", false);
        publish_messages(
            &store,
            &key,
            format,
            messages(format, &key, "gap-selected", 21),
            false,
        );

        let raw = store.native_evidence_publications_after(0, 30).unwrap();
        let before = raw
            .iter()
            .filter(|event| {
                event
                    .native_identity
                    .as_ref()
                    .is_some_and(|identity| identity.contains("before-gap-"))
            })
            .collect::<Vec<_>>();
        assert_eq!(before.len(), 4);
        assert!(
            before
                .iter()
                .all(|event| event.session_selection == Some(selected("model-a", true)))
        );
        let page = PublishedEvidenceSource::new(StorePublishedEvidenceReader::new(&store), 20)
            .unwrap()
            .read_batch(PublicationCursor(after_gap as u64))
            .unwrap();
        assert_eq!(page.events.len(), 8);
        for event in &page.events {
            let SourceEvent::Observation { identity, .. } = event else {
                panic!("expected post-gap native observation")
            };
            let known = identity.contains("gap-selected-");
            assert_session(
                observation(event),
                known.then_some("selection/model-b"),
                known.then_some(false),
            );
        }
    }
}

fn advance_v2_fact(
    store: &Store,
    claim: &OpenCodeV2CaptureClaim,
    sequence: u64,
    fact: NativeEvidenceFact,
    now: i64,
) {
    assert!(
        store
            .advance_opencode_v2_evidence(
                claim,
                sequence,
                &NativeEvidenceBatch {
                    source_format: SourceFormat::OpenCodeSqliteV2,
                    events: vec![NativeEvidenceEvent {
                        fact,
                        live_eligible: true,
                    }],
                    history_complete: false,
                },
                now,
                600,
            )
            .unwrap()
    );
}

fn validate_v2_frontier(store: &Store, claim: &OpenCodeV2CaptureClaim, sequence: u64, now: i64) {
    assert!(
        store
            .validate_opencode_v2_idle_frontier(
                std::slice::from_ref(claim),
                &std::collections::HashMap::from([(claim.key.session_id.clone(), Some(sequence))]),
                now,
            )
            .unwrap()
    );
}

fn host_idles(store: &Store, activation: &ActivationId) -> Vec<HostSessionObservation> {
    store
        .native_evidence_publications_for_project_after(
            "native",
            "/synthetic/repo",
            Some(activation),
            0,
            100,
        )
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.payload {
            Some(EvidencePublicationPayload::Host(host))
                if matches!(host.payload, HostSessionPayload::Idle { .. }) =>
            {
                Some(host)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn v2_idle_selection_does_not_cross_capture_epochs() {
    for restore_stale_gapped_selection in [false, true] {
        let (store, key, activation, mut start) = v2_capture_fixture();
        let selection = |sequence, model, fast_mode| {
            NativeEvidenceFact::SessionSelection(NativeSessionSelectionObservation {
                source_format: SourceFormat::OpenCodeSqliteV2,
                session_id: key.session_id.clone(),
                position: position(SourceFormat::OpenCodeSqliteV2, sequence),
                selection: attribution("selection", model, fast_mode),
            })
        };
        let epoch_a = store
            .start_opencode_v2_capture_epoch(std::slice::from_ref(&start), "epoch-a", 100, 600)
            .unwrap()
            .unwrap()
            .remove(0);
        advance_v2_fact(&store, &epoch_a, 1, selection(1, "model-a", true), 101);
        validate_v2_frontier(&store, &epoch_a, 1, 102);
        assert_eq!(store.next_session_idle_deadline().unwrap(), Some(222_000));
        validate_v2_frontier(&store, &epoch_a, 1, 222);
        let first = host_idles(&store, &activation);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].attribution, selected("model-a", true));
        assert_eq!(
            first[0].context.session_selection,
            Some(selected("model-a", true))
        );

        assert_eq!(
            store
                .mark_opencode_v2_capture_gap(std::slice::from_ref(&epoch_a), "capture-lost", 223)
                .unwrap(),
            1
        );
        let state: (bool, Option<String>) = store
            .lock()
            .query_row(
                "SELECT continuity_eligible, session_selection_json FROM burn_check_session_attachment",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, (false, None));
        if restore_stale_gapped_selection {
            // Restore stale attachment state that an older build can leave after a gap.
            assert_eq!(store.lock().execute(
                "UPDATE burn_check_session_attachment SET session_selection_json = ?1 WHERE continuity_eligible = 0",
                [serde_json::to_string(&selected("model-a", true)).unwrap()],
            ).unwrap(), 1);
        }
        start.baseline_sequence = Some(10);
        let epoch_b = store
            .start_opencode_v2_capture_epoch(std::slice::from_ref(&start), "epoch-b", 224, 600)
            .unwrap()
            .unwrap()
            .remove(0);
        let mutable_selection: Option<String> = store
            .lock()
            .query_row(
                "SELECT session_selection_json FROM burn_check_session_attachment",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mutable_selection, None);
        validate_v2_frontier(&store, &epoch_b, 10, 225);
        assert_eq!(store.next_session_idle_deadline().unwrap(), Some(345_000));
        validate_v2_frontier(&store, &epoch_b, 10, 345);
        let second = host_idles(&store, &activation);
        assert_eq!(second.len(), 2);
        assert_eq!(second[0], first[0]);
        assert_eq!(second[1].attribution, ObservedAttribution::default());
        assert_eq!(second[1].context.session_selection, None);

        advance_v2_fact(&store, &epoch_b, 11, selection(11, "model-b", false), 346);
        validate_v2_frontier(&store, &epoch_b, 11, 347);
        assert_eq!(store.next_session_idle_deadline().unwrap(), None);
        assert_eq!(host_idles(&store, &activation), second);
        advance_v2_fact(
            &store,
            &epoch_b,
            12,
            NativeEvidenceFact::Activity(NativeActivityObservation {
                source_format: SourceFormat::OpenCodeSqliteV2,
                session_id: key.session_id.clone(),
                position: position(SourceFormat::OpenCodeSqliteV2, 12),
                token: "epoch-b-work".into(),
                native_at_ms: Some(348_000),
            }),
            348,
        );
        validate_v2_frontier(&store, &epoch_b, 12, 348);
        assert_eq!(store.next_session_idle_deadline().unwrap(), Some(468_000));
        validate_v2_frontier(&store, &epoch_b, 12, 468);
        let final_idles = host_idles(&store, &activation);
        assert_eq!(final_idles.len(), 3);
        assert_eq!(&final_idles[..2], second.as_slice());
        assert_eq!(final_idles[2].attribution, selected("model-b", false));
        assert_eq!(
            final_idles[2].context.session_selection,
            Some(selected("model-b", false))
        );
        assert!(matches!(
            final_idles[2].payload,
            HostSessionPayload::Idle {
                last_activity_at_ms: Some(348_000),
                ..
            }
        ));

        let page = PublishedEvidenceSource::new(
            StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic/repo")
                .with_activation(&activation),
            100,
        )
        .unwrap()
        .read_batch(PublicationCursor(0))
        .unwrap();
        let normalized_idles = page
            .events
            .iter()
            .filter_map(|event| match event {
                SourceEvent::Observation { observation, .. }
                    if matches!(observation.kind, ObservationKind::SessionIdle { .. }) =>
                {
                    Some(observation)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(normalized_idles.len(), 3);
        assert_session(normalized_idles[0], Some("selection/model-a"), Some(true));
        assert_session(normalized_idles[1], None, None);
        assert_session(normalized_idles[2], Some("selection/model-b"), Some(false));
    }
}

#[test]
fn session_start_and_idle_keep_their_recorded_selection() {
    let (store, key, first) = fixture();
    let format = SourceFormat::ClaudeJsonl;
    seed_delivery(&store, &key, format);
    select(&store, &key, format, 1, "model-a", true);
    let mut second = first.clone();
    second.definition_id = "second".into();
    attach(&store, &key, &second, 2000);
    assert_eq!(store.next_session_idle_deadline().unwrap(), Some(121_000));
    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        observers::frontier_in(&tx, &key, "validated-receipt", true, 122_000).unwrap();
        tx.commit().unwrap();
    }
    select(&store, &key, format, 10, "model-b", false);
    assert_eq!(store.next_session_idle_deadline().unwrap(), None);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT SUM(activity_revision) FROM burn_check_session_attachment",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    for (activation, start_known) in [(&first, false), (&second, true)] {
        let page = PublishedEvidenceSource::new(
            StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic/repo")
                .with_activation(activation),
            20,
        )
        .unwrap()
        .read_batch(PublicationCursor(0))
        .unwrap();
        assert_eq!(page.events.len(), 2);
        assert!(matches!(
            observation(&page.events[0]).kind,
            ObservationKind::SessionStart { .. }
        ));
        assert_session(
            observation(&page.events[0]),
            start_known.then_some("selection/model-a"),
            start_known.then_some(true),
        );
        assert!(matches!(
            observation(&page.events[1]).kind,
            ObservationKind::SessionIdle { .. }
        ));
        assert_session(
            observation(&page.events[1]),
            Some("selection/model-a"),
            Some(true),
        );
    }
}
