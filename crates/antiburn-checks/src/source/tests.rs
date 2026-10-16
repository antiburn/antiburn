use super::*;

struct MemoryReader(PublishedEvidenceBatch);

impl PublishedEvidenceReader for MemoryReader {
    fn read_batch(&self, _request: PublicationRead) -> Result<PublishedEvidenceBatch, SourceError> {
        Ok(self.0.clone())
    }
}

fn event(cursor: u64, change: PublishedEvidenceChange) -> PublishedEvidenceEvent {
    PublishedEvidenceEvent {
        cursor: PublicationCursor(cursor),
        branch: "env/agent/session/main".to_owned(),
        context: Some(PublishedContext {
            requests: vec![],
            session_id: "session".into(),
            agent_name: "opencode".into(),
            project_root: None,
            version: "context-v1".into(),
            main_session: true,
            complete_through_event: false,
        }),
        change,
    }
}

fn result() -> PublishedToolResult {
    PublishedToolResult {
        identity: "call/result".to_owned(),
        native_order: NativeOrderKey("jsonl:4:0".to_owned()),
        first_terminal_publication: None,
        revision: 2,
        tool_name: Some("shell".to_owned()),
        category: Some("shell".to_owned()),
        arguments: Some(serde_json::json!({"command":"true"})),
        content: Some("done".to_owned()),
        status: PublishedResultStatus::Completed,
        finality: true,
        complete: true,
        selection_safe: true,
    }
}

#[test]
fn keeps_publication_and_native_order_separate_until_engine_commit() {
    let reader = MemoryReader(PublishedEvidenceBatch {
        events: vec![event(8, PublishedEvidenceChange::Upsert(result()))],
        next: PublicationCursor(8),
        has_more: false,
    });
    let source = FileEventSource::new(reader, 16).unwrap();
    let batch = source.read_batch(PublicationCursor(7)).unwrap();
    assert_eq!(batch.next, PublicationCursor(8));
    let SourceEvent::Observation { observation, .. } = &batch.events[0] else {
        panic!("expected an observation")
    };
    assert_eq!(
        observation.native_order,
        NativeOrderKey("jsonl:4:0".to_owned())
    );
    assert!(observation.complete);
    assert_eq!(
        observation.context.value["session"]["requests_complete"],
        false
    );
    assert_eq!(observation.branch.0, "env/agent/session/main");
}

#[test]
fn keeps_corrections_removals_and_coverage_gaps_explicit() {
    let reader = MemoryReader(PublishedEvidenceBatch {
        events: vec![
            event(1, PublishedEvidenceChange::Upsert(result())),
            event(
                2,
                PublishedEvidenceChange::Removal {
                    identity: "call/result".to_owned(),
                    revision: 3,
                },
            ),
            event(
                3,
                PublishedEvidenceChange::Gap {
                    reason: "source membership incomplete".to_owned(),
                },
            ),
            event(4, PublishedEvidenceChange::CoverageRestored),
        ],
        next: PublicationCursor(4),
        has_more: false,
    });
    let batch = FileEventSource::new(reader, 4)
        .unwrap()
        .read_batch(PublicationCursor(0))
        .unwrap();
    assert!(matches!(
        batch.events[0],
        SourceEvent::Observation { revision: 2, .. }
    ));
    assert!(matches!(
        batch.events[1],
        SourceEvent::Removal { revision: 3, .. }
    ));
    assert!(matches!(batch.events[2], SourceEvent::Gap { .. }));
    assert!(matches!(
        batch.events[3],
        SourceEvent::CoverageRestored { .. }
    ));
}

#[test]
fn partial_context_is_preserved_with_explicit_quality_fields() {
    let mut event = event(1, PublishedEvidenceChange::Upsert(result()));
    event.context = Some(PublishedContext {
        requests: vec![(
            "request-1".to_owned(),
            serde_json::json!({"text": "Retry the check.", "kind": "user_message", "source": "opencode"}),
        )],
        session_id: "session-1".to_owned(),
        agent_name: "opencode".to_owned(),
        project_root: Some("/repo".to_owned()),
        version: "context-v1".to_owned(),
        main_session: true,
        complete_through_event: false,
    });
    let reader = MemoryReader(PublishedEvidenceBatch {
        events: vec![event],
        next: PublicationCursor(1),
        has_more: false,
    });
    let batch = FileEventSource::new(reader, 1)
        .unwrap()
        .read_batch(PublicationCursor(0))
        .unwrap();
    let SourceEvent::Observation { observation, .. } = &batch.events[0] else {
        panic!("expected an observation")
    };
    assert!(observation.complete);
    assert_eq!(observation.context.value["session"]["id"], "session-1");
    assert_eq!(
        observation.context.value["session"]["requests"][0]["text"],
        "Retry the check."
    );
    assert_eq!(
        observation.context.value["session"]["requests_complete"],
        false
    );
    assert_eq!(
        observation.context.value["session"]["source_coverage"],
        "partial"
    );
    assert_eq!(observation.context.value["agent"]["name"], "opencode");
    assert_eq!(observation.context.value["repo"]["root"], "/repo");
}

#[test]
fn rejects_cursor_gaps_and_bad_page_progress() {
    let reader = MemoryReader(PublishedEvidenceBatch {
        events: vec![event(
            5,
            PublishedEvidenceChange::Gap {
                reason: "gap".into(),
            },
        )],
        next: PublicationCursor(4),
        has_more: false,
    });
    let source = FileEventSource::new(reader, 1).unwrap();
    assert!(matches!(
        source.read_batch(PublicationCursor(3)),
        Err(SourceError::InvalidNextCursor { .. })
    ));
}

#[test]
fn non_final_updates_do_not_become_tool_result_triggers() {
    let mut result = result();
    result.finality = false;
    let SourceEvent::Observation { observation, .. } =
        normalize(event(1, PublishedEvidenceChange::Upsert(result))).unwrap()
    else {
        panic!("expected an observation");
    };
    assert_eq!(observation.kind, ObservationKind::Other);
    assert!(!observation.complete);
}

#[test]
fn normalization_exposes_canonical_authored_message_identity() {
    let SourceEvent::Observation {
        identity,
        observation,
        ..
    } = normalize(event(1, PublishedEvidenceChange::Upsert(result()))).unwrap()
    else {
        panic!("expected an observation")
    };
    let ObservationKind::ToolResult { message, .. } = &observation.kind else {
        panic!("expected a result")
    };
    assert_eq!(message.id, identity);
    assert_eq!(message.value["id"], identity);
    assert_eq!(message.value["source"], "opencode");
    assert_eq!(message.value["kind"], "tool_result");
    assert_eq!(message.value["role"], "tool");
}

#[test]
fn unsafe_selection_is_separate_from_missing_payload_fields() {
    let mut partial = result();
    partial.complete = false;
    partial.category = None;
    partial.arguments = None;
    partial.content = None;
    let SourceEvent::Observation { observation, .. } =
        normalize(event(1, PublishedEvidenceChange::Upsert(partial.clone()))).unwrap()
    else {
        panic!("partial fields must remain usable")
    };
    let ObservationKind::ToolResult {
        message, category, ..
    } = &observation.kind
    else {
        panic!("expected a result")
    };
    assert!(category.is_empty());
    assert!(message.value["tool"].get("category").is_none());
    assert_eq!(message.value["quality"]["category_available"], false);
    assert!(!observation.complete);
    for unavailable in ["admission", "context", "scope", "identity", "order"] {
        let mut source = event(2, PublishedEvidenceChange::Upsert(partial.clone()));
        match unavailable {
            "context" => source.context = None,
            "scope" => source.context.as_mut().unwrap().main_session = false,
            field => {
                let PublishedEvidenceChange::Upsert(result) = &mut source.change else {
                    unreachable!()
                };
                match field {
                    "admission" => result.selection_safe = false,
                    "identity" => result.identity.clear(),
                    "order" => result.native_order.0.clear(),
                    _ => unreachable!(),
                }
            }
        }
        assert!(
            matches!(normalize(source).unwrap(), SourceEvent::Gap { .. }),
            "{unavailable}"
        );
    }
}

#[test]
fn evidence_versions_cover_same_revision_payload_and_context_corrections() {
    let original = event(1, PublishedEvidenceChange::Upsert(result()));
    let version = |event| {
        let SourceEvent::Observation { observation, .. } = normalize(event).unwrap() else {
            panic!("expected an observation");
        };
        observation.version
    };
    let mut replay = original.clone();
    replay.cursor = PublicationCursor(2);
    assert_eq!(version(original.clone()), version(replay));
    let mut corrected = original.clone();
    let PublishedEvidenceChange::Upsert(result) = &mut corrected.change else {
        unreachable!()
    };
    result.content = Some("corrected output".into());
    assert_ne!(version(original.clone()), version(corrected));
    let mut context_changed = original.clone();
    context_changed.context = Some(PublishedContext {
        requests: vec![("request-1".into(), serde_json::json!("API request"))],
        session_id: "session-1".into(),
        agent_name: "opencode".into(),
        project_root: Some("/repo".into()),
        version: "context-v1".into(),
        main_session: true,
        complete_through_event: true,
    });
    assert_ne!(version(original.clone()), version(context_changed.clone()));
    let before = version(context_changed.clone());
    context_changed.context.as_mut().unwrap().requests[0].1 =
        serde_json::json!("corrected request");
    assert_ne!(before, version(context_changed));
}

#[test]
fn first_terminal_publication_survives_normalization_and_observation_serialization() {
    let mut result = result();
    result.first_terminal_publication = Some(PublicationCursor(8));
    let SourceEvent::Observation { observation, .. } =
        normalize(event(20, PublishedEvidenceChange::Upsert(result))).unwrap()
    else {
        panic!("expected a terminal correction")
    };
    assert_eq!(
        observation.first_terminal_publication,
        Some(PublicationCursor(8))
    );
    let committed = observation.commit(crate::domain::ObservationPosition(3));
    let saved = serde_json::to_string(&committed).unwrap();
    let restored: Observation = serde_json::from_str(&saved).unwrap();
    assert_eq!(restored, committed);
    assert_eq!(
        restored.first_terminal_publication,
        Some(PublicationCursor(8))
    );
}

#[test]
fn observations_without_first_terminal_metadata_keep_the_legacy_serialized_shape() {
    let SourceEvent::Observation { observation, .. } =
        normalize(event(20, PublishedEvidenceChange::Upsert(result()))).unwrap()
    else {
        panic!("expected an observation")
    };
    let value = serde_json::to_value(&observation).unwrap();
    assert!(value.get("first_terminal_publication").is_none());
    let restored: ObservationDraft = serde_json::from_value(value).unwrap();
    assert_eq!(restored.first_terminal_publication, None);
    let committed = restored.commit(crate::domain::ObservationPosition(1));
    let value = serde_json::to_value(&committed).unwrap();
    assert!(value.get("first_terminal_publication").is_none());
    assert_eq!(
        serde_json::from_value::<Observation>(value).unwrap(),
        committed
    );
}
