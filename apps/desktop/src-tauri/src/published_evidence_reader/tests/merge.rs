use super::*;
use antiburn_checks::context::ObservedAttribution;
use antiburn_checks::domain::{ActivationScope, Generation, ObservationKind, RunScope};
use antiburn_local::analysis::{
    NativeEventContext, UserMessage, UserMessageChange, UserMessageCompleteness,
    UserMessageObservation, UserMessageOrigin, UserMessagePart, UserMessagePosition,
    UserMessageScope,
};

const GAP_REASON: &str = "retained-history-partial";

fn selection(model: &str, fast_mode: bool) -> ObservedAttribution {
    ObservedAttribution {
        model: Some(format!("selection/{model}")),
        fast_mode: Some(fast_mode),
    }
}

fn result_at(index: u64, format: SourceFormat) -> ToolResultObservation {
    let mut result = result(true, format);
    result.position = match format {
        SourceFormat::OpenCodeSqliteV1 => ToolResultPosition::OpenCodeEvent {
            sequence: index,
            item_index: 0,
        },
        _ => ToolResultPosition::FileBytes {
            record_start: index * 10,
            record_end: index * 10 + 9,
            item_index: 0,
        },
    };
    result.context.as_mut().unwrap().result_position = result.position;
    result
}

fn insert_result_snapshot(
    connection: &Connection,
    session: &SessionKey,
    identity: &str,
    result: &ToolResultObservation,
    selection: Option<&ObservedAttribution>,
) -> i64 {
    assert_eq!(result.session_id, session.session_id);
    connection.execute(
        "INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id,
            source_format, native_identity, native_position_json, native_revision, payload_json,
            complete, source_generation, claim_fence, live_eligible, session_incarnation, project_cwd,
            session_selection_json)
         VALUES ('update', ?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, 1, 1, 1, 1, 7, '/synthetic', ?8)",
        params![session.environment_key, session.agent, session.session_id,
            serde_json::to_string(&result.source_format).unwrap(), identity,
            serde_json::to_string(&result.position).unwrap(), serde_json::to_string(result).unwrap(),
            selection.map(serde_json::to_string).transpose().unwrap()],
    ).unwrap();
    connection.last_insert_rowid()
}

fn insert_gap(connection: &Connection, kind: PublicationGapKind) -> i64 {
    connection.execute(
        "INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id,
            source_format, complete, source_generation, claim_fence, session_incarnation,
            project_cwd, gap_reason, gap_kind)
         VALUES ('coverage_gap', 'native', 'pi', 'session', ?1, 0, 1, 1, 7, '/synthetic', ?2, ?3)",
        params![serde_json::to_string(&SourceFormat::PiV3Jsonl).unwrap(), GAP_REASON,
            serde_json::to_value(kind).unwrap().as_str().unwrap()],
    ).unwrap();
    connection.last_insert_rowid()
}

fn assert_result_selection(event: &SourceEvent, expected: Option<&ObservedAttribution>) {
    let SourceEvent::Observation { observation, .. } = event else {
        panic!("expected a result observation, got {event:?}")
    };
    assert!(matches!(
        observation.kind,
        ObservationKind::ToolResult { .. }
    ));
    let session = &observation.context.value["session"];
    assert_eq!(
        session["model"].as_str(),
        expected.and_then(|value| value.model.as_deref())
    );
    assert_eq!(
        session["fast_mode"].as_bool(),
        expected.and_then(|value| value.fast_mode)
    );
}

#[test]
fn generic_store_pages_keep_typed_gaps_selection_snapshots_and_exact_cursors() {
    let store = store();
    let key = SessionKey::new("native", "pi", "session");
    let cases = [
        (
            PublicationGapKind::Partial,
            Some(selection("model-a", true)),
        ),
        (PublicationGapKind::Invalidated, None),
        (
            PublicationGapKind::SegmentReset,
            Some(selection("model-b", false)),
        ),
    ];
    let mut sequences = Vec::new();
    {
        let connection = store.lock();
        insert_project_session(&connection);
        for (index, (kind, selected)) in cases.iter().enumerate() {
            sequences.push(insert_result_snapshot(
                &connection,
                &key,
                &format!("result-{index}"),
                &result_at(index as u64 + 1, SourceFormat::PiV3Jsonl),
                selected.as_ref(),
            ));
            sequences.push(insert_gap(&connection, *kind));
        }
        sequences.push(insert_result_snapshot(
            &connection,
            &key,
            "last-unknown",
            &result_at(4, SourceFormat::PiV3Jsonl),
            None,
        ));
    }
    let generic = store.native_evidence_publications_after(0, 20).unwrap();
    let legacy = store.native_tool_result_publications_after(0, 20).unwrap();
    assert_eq!(generic.len(), sequences.len());
    assert_eq!(legacy.len(), sequences.len());
    for (index, (kind, selected)) in cases.iter().enumerate() {
        assert_eq!(generic[index * 2].session_selection, *selected);
        assert_eq!(legacy[index * 2].session_selection, *selected);
        assert_eq!(generic[index * 2 + 1].gap_kind, *kind);
        assert_eq!(legacy[index * 2 + 1].gap_kind, *kind);
        assert_eq!(generic[index * 2 + 1].session_selection, None);
        assert_eq!(legacy[index * 2 + 1].session_selection, None);
    }
    assert_eq!(generic.last().unwrap().session_selection, None);
    assert_eq!(legacy.last().unwrap().session_selection, None);

    let source = PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic"),
        2,
    )
    .unwrap();
    let mut cursor = PublicationCursor(0);
    let mut consumed = Vec::new();
    for (index, (kind, selected)) in cases.iter().enumerate() {
        let page = source.read_batch(cursor).unwrap();
        assert_eq!(page.events.len(), 2);
        assert_result_selection(&page.events[0], selected.as_ref());
        let SourceEvent::Observation {
            cursor: result_cursor,
            ..
        } = &page.events[0]
        else {
            unreachable!()
        };
        let (gap_cursor, branch, reason) = match (kind, &page.events[1]) {
            (
                PublicationGapKind::Partial,
                SourceEvent::Gap {
                    cursor,
                    branch,
                    reason,
                },
            )
            | (
                PublicationGapKind::Invalidated,
                SourceEvent::UnsafeUpdate {
                    cursor,
                    branch,
                    reason,
                },
            )
            | (
                PublicationGapKind::SegmentReset,
                SourceEvent::SegmentReset {
                    cursor,
                    branch,
                    reason,
                },
            ) => (cursor, branch, reason),
            (_, event) => panic!("wrong classification for {kind:?}: {event:?}"),
        };
        assert_eq!(
            *result_cursor,
            PublicationCursor(sequences[index * 2] as u64)
        );
        assert_eq!(
            *gap_cursor,
            PublicationCursor(sequences[index * 2 + 1] as u64)
        );
        assert_eq!(branch.0, key.to_identity_json());
        assert_eq!(reason, GAP_REASON);
        assert_eq!(page.next, *gap_cursor);
        assert!(page.has_more);
        consumed.extend([result_cursor.0, gap_cursor.0]);
        cursor = page.next;
    }
    let last = source.read_batch(cursor).unwrap();
    assert_eq!(last.events.len(), 1);
    assert_result_selection(&last.events[0], None);
    assert_eq!(
        last.next,
        PublicationCursor(*sequences.last().unwrap() as u64)
    );
    assert!(!last.has_more);
    consumed.push(last.next.0);
    assert_eq!(
        consumed,
        sequences
            .iter()
            .map(|sequence| *sequence as u64)
            .collect::<Vec<_>>()
    );
    let empty = source.read_batch(last.next).unwrap();
    assert!(empty.events.is_empty());
    assert_eq!(empty.next, last.next);
    assert!(!empty.has_more);
}

fn retain_current(connection: &Connection, sequence: i64) {
    assert_eq!(connection.execute(
        "INSERT INTO native_tool_result_current(environment_key, agent, session_id, source_format,
            native_identity, native_position_json, native_revision, payload_json, complete, is_removed,
            publication_sequence, source_generation, claim_fence, event_kind)
         SELECT environment_key, agent, session_id, source_format, native_identity, native_position_json,
            native_revision, payload_json, complete, 0, publication_sequence, source_generation,
            claim_fence, event_kind FROM native_tool_result_publication WHERE publication_sequence = ?1",
        [sequence],
    ).unwrap(), 1);
}

fn insert_generic_user(connection: &Connection) -> i64 {
    let position = ToolResultPosition::FileBytes {
        record_start: 20,
        record_end: 29,
        item_index: 0,
    };
    let message = UserMessageObservation {
        message: UserMessage {
            source_format: SourceFormat::PiV3Jsonl,
            session_id: "session".into(),
            message_id: "generic-user".into(),
            position: UserMessagePosition::FileBytes {
                record_start: 20,
                record_end: 29,
            },
            origin: UserMessageOrigin::PiUserContent,
            scope: UserMessageScope::MainSession,
            change: UserMessageChange::Present,
            completeness: UserMessageCompleteness::Complete,
            parts: vec![UserMessagePart {
                item_index: 0,
                native_part_id: None,
                text: "Keep the result window limited to results.".into(),
            }],
            proof_revision: 1,
        },
        revision: 1,
        context: NativeEventContext {
            first_eligible_position: position,
            main_session: true,
            complete_through_event: true,
            user_messages: Vec::new(),
        },
    };
    assert!(message.message.is_bounded());
    let identity = serde_json::to_string(&(
        "user_text",
        &message.message.session_id,
        &message.message.message_id,
    ))
    .unwrap();
    let mut payload = serde_json::to_value(&message).unwrap();
    // A result-like extension must not bypass the stored event kind.
    payload["finality"] = serde_json::Value::Bool(true);
    connection.execute(
        "INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id,
            source_format, native_identity, native_position_json, native_revision, payload_json,
            complete, source_generation, claim_fence, live_eligible, session_incarnation, project_cwd,
            event_kind, first_eligible_publication)
         VALUES ('update', 'native', 'pi', 'session', ?1, ?2, ?3, 1, ?4,
            1, 1, 1, 1, 7, '/synthetic', 'user_message',
            (SELECT seq + 1 FROM sqlite_sequence WHERE name = 'native_tool_result_publication'))",
        params![serde_json::to_string(&SourceFormat::PiV3Jsonl).unwrap(), identity,
            serde_json::to_string(&position).unwrap(), serde_json::to_string(&payload).unwrap()],
    ).unwrap();
    connection.last_insert_rowid()
}

#[test]
fn startup_window_keeps_result_cutoff_and_excludes_generic_and_typed_v2_sources() {
    let store = store();
    let key = SessionKey::new("native", "pi", "session");
    let known = selection("startup", true);
    let (first, second, third) = {
        let connection = store.lock();
        insert_project_session(&connection);
        let first = insert_result_snapshot(
            &connection,
            &key,
            "first",
            &result_at(1, SourceFormat::PiV3Jsonl),
            Some(&known),
        );
        retain_current(&connection, first);
        let generic = insert_generic_user(&connection);
        retain_current(&connection, generic);
        let second = insert_result_snapshot(
            &connection,
            &key,
            "second",
            &result_at(3, SourceFormat::PiV3Jsonl),
            None,
        );
        retain_current(&connection, second);
        let third = insert_result_snapshot(
            &connection,
            &key,
            "third",
            &result_at(4, SourceFormat::PiV3Jsonl),
            None,
        );
        retain_current(&connection, third);
        (first, second, third)
    };
    let reader = StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic");
    let public = PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic"),
        10,
    )
    .unwrap()
    .read_batch(PublicationCursor(0))
    .unwrap();
    assert_eq!(public.events.len(), 4);
    assert!(
        matches!(&public.events[1], SourceEvent::Observation { observation, .. }
        if matches!(observation.kind, ObservationKind::UserMessage(_)))
    );
    for (before, expected) in [
        (None, vec!["first", "second", "third"]),
        (Some(third), vec!["first", "second"]),
        (Some(second), vec!["first"]),
        (Some(first), vec![]),
    ] {
        let events = reader.recent_results_before(&key, before).unwrap();
        let identities = events
            .iter()
            .map(|event| {
                let SourceEvent::Observation { identity, .. } = event else {
                    panic!("startup result expected")
                };
                identity.as_str()
            })
            .collect::<Vec<_>>();
        assert_eq!(identities, expected);
        for (index, event) in events.iter().enumerate() {
            assert_result_selection(event, (index == 0).then_some(&known));
        }
    }

    let v2_key = SessionKey::new("native", "opencode", "ses_v2");
    let retained_legacy = {
        let connection = store.lock();
        insert_opencode_v2_session(&connection);
        connection
            .execute(
                "UPDATE session SET source_label = 'opencode:ses_v2' WHERE agent = 'opencode'",
                [],
            )
            .unwrap();
        insert_opencode_v2_result(&connection, 1);
        let mut legacy = result_at(2, SourceFormat::OpenCodeSqliteV1);
        legacy.session_id = v2_key.session_id.clone();
        let sequence = insert_result_snapshot(
            &connection,
            &v2_key,
            "retained-v1-result",
            &legacy,
            Some(&known),
        );
        retain_current(&connection, sequence);
        sequence
    };
    for before in [None, Some(retained_legacy + 1)] {
        assert!(
            reader
                .recent_results_before(&v2_key, before)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn typed_v2_with_v1_label_waits_only_for_an_exact_capture_and_then_an_attachment() {
    for capture in ["missing", "stale", "exact"] {
        let store = store();
        let key = SessionKey::new("native", "opencode", "ses_v2");
        let activation = ActivationId {
            definition_id: "merge-reader".into(),
            generation: Generation(1),
            scope: ActivationScope {
                environment: "native".into(),
                session_cwd: "/synthetic".into(),
                run: RunScope::Live,
            },
        };
        let scope = crate::check_store::StoreCheckStore::activation_scope_key(&activation.scope);
        {
            let connection = store.lock();
            insert_opencode_v2_session(&connection);
            connection
                .execute("UPDATE session SET source_label = 'opencode:ses_v2'", [])
                .unwrap();
            insert_opencode_v2_result(&connection, 1);
            connection.execute(
                "INSERT INTO burn_check_activation(check_id, activation_scope_key, definition_revision,
                    enabled, generation, bootstrap_sequence, applied_sequence, updated_at_epoch)
                 VALUES (?1, ?2, 'synthetic', 1, 1, 0, 0, 0)",
                params![activation.definition_id, scope],
            ).unwrap();
            if capture != "missing" {
                connection.execute(
                    "INSERT INTO opencode_v2_capture(environment_key, agent, session_id, incarnation,
                        source_generation, source_label, cwd, capture_epoch, claim_fence, history_gap)
                     SELECT environment_key, agent, session_id, incarnation + ?1, source_generation,
                        source_label, cwd, 'synthetic-capture', 1, 0 FROM session",
                    [i64::from(capture == "stale")],
                ).unwrap();
            }
            if capture == "exact" {
                insert_update(
                    &connection,
                    "unrelated-tail",
                    true,
                    Some(7),
                    Some("/other-project"),
                    SourceFormat::PiV3Jsonl,
                );
            }
        }
        let sequence =
            store.native_evidence_publications_after(0, 10).unwrap()[0].publication_sequence;
        let source = PublishedEvidenceSource::new(
            StorePublishedEvidenceReader::for_live_project(&store, "native", "/synthetic")
                .with_activation(&activation),
            10,
        )
        .unwrap();
        let page = source.read_batch(PublicationCursor(0)).unwrap();
        if capture != "exact" {
            assert_eq!(page.events.len(), 1, "{capture}");
            assert!(
                matches!(page.events[0], SourceEvent::Ignored { .. }),
                "{capture}"
            );
            assert_eq!(page.next, PublicationCursor(sequence as u64));
            assert!(!page.has_more);
            let empty = source.read_batch(page.next).unwrap();
            assert!(empty.events.is_empty());
            assert_eq!(empty.next, page.next);
            assert!(!empty.has_more);
            continue;
        }
        for _ in 0..2 {
            let held = source.read_batch(page.next).unwrap();
            assert!(held.events.is_empty());
            assert_eq!(held.next, PublicationCursor(0));
            assert!(!held.has_more);
        }
        assert!(page.events.is_empty());
        assert_eq!(page.next, PublicationCursor(0));
        store.lock().execute(
            "INSERT INTO burn_check_session_attachment(check_id, activation_scope_key, generation,
                branch_key, incarnation) VALUES (?1, ?2, 1, ?3, 7)",
            params![activation.definition_id, scope, key.to_identity_json()],
        ).unwrap();
        let attached = source.read_batch(page.next).unwrap();
        assert_eq!(attached.events.len(), 1);
        assert_result_selection(&attached.events[0], None);
        assert_eq!(attached.next, PublicationCursor(sequence as u64));
        assert!(!attached.has_more);
        let empty = source.read_batch(attached.next).unwrap();
        assert!(empty.events.is_empty());
        assert_eq!(empty.next, PublicationCursor(sequence as u64 + 1));
        assert!(!empty.has_more);
    }
}
