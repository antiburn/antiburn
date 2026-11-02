use super::*;

struct MemoryReader(PublishedEvidenceBatch);

impl PublishedEvidenceReader for MemoryReader {
    fn read_batch(&self, _request: PublicationRead) -> Result<PublishedEvidenceBatch, SourceError> {
        Ok(self.0.clone())
    }
}

fn event(cursor: u64, change: PublishedEvidenceChange) -> PublishedEvidenceEvent {
    PublishedEvidenceEvent {
        source_evidence_version: None,
        cursor: PublicationCursor(cursor),
        branch: "env/agent/session/main".to_owned(),
        context: Some(PublishedContext {
            session_selection: None,
            requests: vec![],
            session_id: "session".into(),
            agent_name: "opencode".into(),
            project_root: None,
            version: "context-v1".into(),
            main_session: true,
            complete_through_event: false,
            inventories: CurrentInventories::default(),
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
        metadata: Box::default(),
        arguments: Some(serde_json::json!({"command":"true"})),
        content: Some("done".to_owned()),
        outcome: None,
        provider_executed: None,
        status: PublishedResultStatus::Completed,
        finality: true,
        payload_complete: true,
        validity: EvidenceValidity::Valid,
    }
}

#[test]
fn explicit_unsafe_publication_stays_distinct_from_partial_history_without_an_observation() {
    let mut unsafe_event = event(
        2,
        PublishedEvidenceChange::UnsafeUpdate {
            reason: "unsafe-identity".into(),
        },
    );
    unsafe_event.context = None;
    let source = PublishedEvidenceSource::new(
        MemoryReader(PublishedEvidenceBatch {
            events: vec![
                event(
                    1,
                    PublishedEvidenceChange::PartialGap {
                        reason: "history-incomplete".into(),
                    },
                ),
                unsafe_event,
            ],
            next: PublicationCursor(2),
            has_more: false,
        }),
        2,
    )
    .unwrap();
    let batch = source.read_batch(PublicationCursor(0)).unwrap();
    assert_eq!(
        batch.events,
        vec![
            SourceEvent::Gap {
                cursor: PublicationCursor(1),
                branch: BranchId("env/agent/session/main".into()),
                reason: "history-incomplete".into()
            },
            SourceEvent::UnsafeUpdate {
                cursor: PublicationCursor(2),
                branch: BranchId("env/agent/session/main".into()),
                reason: "unsafe-identity".into()
            },
        ]
    );
    assert_eq!(batch.next, PublicationCursor(2));
    assert!(!batch.has_more);
}

#[test]
fn mode_evidence_qualifier_has_equivalent_direct_and_literal_bracket_access() {
    for expression in [
        "message.quality.fast_mode_evidence",
        "message['quality']['fast_mode_evidence']",
    ] {
        let source = format!(
            "name: Mode evidence\non: {{tool_result: {{fast_mode: true}}}}\nactions:\n  - type: status\n    input:\n      result: success\n      message: ${{{{ {expression} }}}}\n"
        );
        assert!(crate::compile(&source).is_ok(), "{expression}");
    }
}

#[test]
fn mode_evidence_qualifier_is_frozen_with_known_mode_only() {
    for mode in [Some(true), Some(false), None] {
        let mut result = result();
        result.metadata.attribution.fast_mode = mode;
        result.metadata.fast_mode_evidence = Some("requested-tier-preference".into());
        let source = PublishedEvidenceSource::new(
            MemoryReader(PublishedEvidenceBatch {
                events: vec![event(1, PublishedEvidenceChange::Upsert(result))],
                next: PublicationCursor(1),
                has_more: false,
            }),
            1,
        )
        .unwrap();
        let batch = source.read_batch(PublicationCursor(0)).unwrap();
        let SourceEvent::Observation { observation, .. } = &batch.events[0] else {
            panic!("expected an observation");
        };
        let ObservationKind::ToolResult { message, .. } = &observation.kind else {
            panic!("expected a tool result");
        };
        if let Some(mode) = mode {
            assert_eq!(message.value["fast_mode"], mode);
            assert_eq!(
                message.value["quality"]["fast_mode_evidence"],
                "requested-tier-preference"
            );
        } else {
            assert!(message.value.get("fast_mode").is_none());
            assert!(message.value["quality"].get("fast_mode_evidence").is_none());
        }
        let frozen = serde_json::to_value(observation).unwrap();
        let restored: ObservationDraft = serde_json::from_value(frozen.clone()).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), frozen);
    }
}

#[test]
fn keeps_publication_and_native_order_separate_until_engine_commit() {
    let reader = MemoryReader(PublishedEvidenceBatch {
        events: vec![event(8, PublishedEvidenceChange::Upsert(result()))],
        next: PublicationCursor(8),
        has_more: false,
    });
    let source = PublishedEvidenceSource::new(reader, 16).unwrap();
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
    let batch = PublishedEvidenceSource::new(reader, 4)
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
    assert!(matches!(batch.events[2], SourceEvent::UnsafeUpdate { .. }));
    assert!(matches!(
        batch.events[3],
        SourceEvent::CoverageRestored { .. }
    ));
}

#[test]
fn program_outcome_and_provider_execution_do_not_change_tool_status() {
    for outcome in [
        serde_json::json!({"kind":"shell","exit":7}),
        serde_json::json!({"kind":"code_mode","error":true}),
        serde_json::json!({"kind":"subagent","state":"running"}),
    ] {
        let mut result = result();
        result.outcome = Some(outcome.clone());
        result.provider_executed = Some(false);
        let normalized = normalize(event(1, PublishedEvidenceChange::Upsert(result))).unwrap();
        let SourceEvent::Observation { observation, .. } = normalized else {
            panic!("expected a result");
        };
        let ObservationKind::ToolResult {
            message, status, ..
        } = observation.kind
        else {
            panic!("expected a tool result");
        };
        assert_eq!(status, "completed");
        assert_eq!(message.value["tool"]["result"]["outcome"], outcome);
        assert_eq!(message.value["tool"]["provider_executed"], false);
    }
    let value = serde_json::to_value(result()).unwrap();
    assert!(value.get("outcome").is_none());
    assert!(value.get("provider_executed").is_none());
}

#[test]
fn continuity_reset_stays_distinct_from_unsafe_publication_gaps() {
    let normalized = normalize(event(
        12,
        PublishedEvidenceChange::SegmentReset {
            reason: "capture-lost".into(),
        },
    ))
    .unwrap();
    assert_eq!(
        normalized,
        SourceEvent::SegmentReset {
            cursor: PublicationCursor(12),
            branch: BranchId("env/agent/session/main".into()),
            reason: "capture-lost".into(),
        }
    );
    assert_eq!(
        normalize(event(
            13,
            PublishedEvidenceChange::Gap {
                reason: "journal-main-session-unavailable".into()
            }
        ))
        .unwrap(),
        SourceEvent::UnsafeUpdate {
            cursor: PublicationCursor(13),
            branch: BranchId("env/agent/session/main".into()),
            reason: "journal-main-session-unavailable".into(),
        }
    );
    assert_eq!(
        normalize(event(
            14,
            PublishedEvidenceChange::PartialGap {
                reason: "request-history-incomplete".into()
            }
        ))
        .unwrap(),
        SourceEvent::Gap {
            cursor: PublicationCursor(14),
            branch: BranchId("env/agent/session/main".into()),
            reason: "request-history-incomplete".into(),
        }
    );
}

#[test]
fn partial_context_is_preserved_with_explicit_quality_fields() {
    let mut event = event(1, PublishedEvidenceChange::Upsert(result()));
    event.context = Some(PublishedContext {
        session_selection: None,
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
        inventories: CurrentInventories::default(),
    });
    let reader = MemoryReader(PublishedEvidenceBatch {
        events: vec![event],
        next: PublicationCursor(1),
        has_more: false,
    });
    let batch = PublishedEvidenceSource::new(reader, 1)
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
fn empty_pages_can_advance_past_filtered_publications_but_cannot_rewind() {
    for next in [3, 8] {
        let source = PublishedEvidenceSource::new(
            MemoryReader(PublishedEvidenceBatch {
                events: vec![],
                next: PublicationCursor(next),
                has_more: false,
            }),
            1,
        )
        .unwrap();
        let batch = source.read_batch(PublicationCursor(3)).unwrap();
        assert!(batch.events.is_empty());
        assert_eq!(batch.next, PublicationCursor(next));
        assert!(!batch.has_more);
        assert!(matches!(
            source.read_batch(PublicationCursor(next + 1)),
            Err(SourceError::InvalidNextCursor { .. })
        ));
    }
}

#[test]
fn rejects_cursor_gaps_and_bad_page_progress() {
    for next in [4, 6] {
        let reader = MemoryReader(PublishedEvidenceBatch {
            events: vec![event(
                5,
                PublishedEvidenceChange::Gap {
                    reason: "gap".into(),
                },
            )],
            next: PublicationCursor(next),
            has_more: false,
        });
        let source = PublishedEvidenceSource::new(reader, 1).unwrap();
        assert!(matches!(
            source.read_batch(PublicationCursor(3)),
            Err(SourceError::InvalidNextCursor { .. })
        ));
    }
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
fn evidence_validity_is_separate_from_missing_payload_fields() {
    let mut partial = result();
    partial.payload_complete = false;
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
    for unavailable in [
        "unknown",
        "unsafe_identity",
        "unsafe_order",
        "unsafe_ownership",
        "context",
        "scope",
        "branch",
        "session",
        "agent",
        "identity",
        "order",
    ] {
        let mut source = event(2, PublishedEvidenceChange::Upsert(partial.clone()));
        match unavailable {
            "context" => source.context = None,
            "scope" => source.context.as_mut().unwrap().main_session = false,
            "branch" => source.branch.clear(),
            "session" => source.context.as_mut().unwrap().session_id.clear(),
            "agent" => source.context.as_mut().unwrap().agent_name.clear(),
            field => {
                let PublishedEvidenceChange::Upsert(result) = &mut source.change else {
                    unreachable!()
                };
                match field {
                    "unknown" => result.validity = EvidenceValidity::Unknown,
                    "unsafe_identity" => result.validity = EvidenceValidity::UnsafeIdentity,
                    "unsafe_order" => result.validity = EvidenceValidity::UnsafeOrder,
                    "unsafe_ownership" => result.validity = EvidenceValidity::UnsafeOwnership,
                    "identity" => result.identity.clear(),
                    "order" => result.native_order.0.clear(),
                    _ => unreachable!(),
                }
            }
        }
        let branch = BranchId(source.branch.clone());
        let SourceEvent::UnsafeUpdate {
            cursor,
            branch: normalized_branch,
            reason,
        } = normalize(source).unwrap()
        else {
            panic!("{unavailable} must revoke unsafe evidence")
        };
        assert_eq!(cursor, PublicationCursor(2), "{unavailable}");
        assert_eq!(normalized_branch, branch, "{unavailable}");
        assert!(!reason.is_empty(), "{unavailable}");
    }
}

#[test]
fn evidence_validity_keeps_the_existing_serialized_boolean() {
    let valid = serde_json::to_value(result()).unwrap();
    assert_eq!(valid["complete"], true);
    assert_eq!(valid["selection_safe"], true);
    assert!(valid.get("payload_complete").is_none());
    assert!(valid.get("validity").is_none());

    let mut unknown = result();
    unknown.validity = EvidenceValidity::Unknown;
    let unknown = serde_json::to_value(unknown).unwrap();
    assert_eq!(unknown["complete"], true);
    assert_eq!(unknown["selection_safe"], false);
    assert!(unknown.get("validity").is_none());
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
        session_selection: None,
        requests: vec![("request-1".into(), serde_json::json!("API request"))],
        session_id: "session-1".into(),
        agent_name: "opencode".into(),
        project_root: Some("/repo".into()),
        version: "context-v1".into(),
        main_session: true,
        complete_through_event: true,
        inventories: CurrentInventories::default(),
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
