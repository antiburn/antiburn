use super::*;
use crate::store::native_tool_results::request_origin;
use antiburn_checks::domain::{ActivationScope, Generation, RunScope};

fn insert_claude_result(store: &Store, identity: &str, incarnation: i64, text: &str) -> i64 {
    let mut result = result(true, SourceFormat::ClaudeJsonl);
    result.record_id = Some(identity.into());
    result.call_id = Some(identity.into());
    result.content = Some(text.into());
    let connection = store.lock();
    connection.execute("INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id,
        source_format, native_identity, native_position_json, native_revision, payload_json, complete,
        source_generation, claim_fence, live_eligible, session_incarnation, project_cwd)
        VALUES ('update', 'native', 'claude-code', 'session', '\"claude_jsonl\"', ?1, ?2, 1, ?3, 1, 1, 1, 1, ?4, '/synthetic')",
        params![identity, serde_json::to_string(&result.position).unwrap(), serde_json::to_string(&result).unwrap(), incarnation]).unwrap();
    let sequence = connection.last_insert_rowid();
    connection.execute("INSERT INTO native_tool_result_current(environment_key, agent, session_id, source_format,
        native_identity, native_position_json, native_revision, payload_json, complete, is_removed,
        publication_sequence, source_generation, claim_fence)
        SELECT environment_key, agent, session_id, source_format, native_identity, native_position_json,
            native_revision, payload_json, 1, 0, publication_sequence, source_generation, claim_fence
          FROM native_tool_result_publication WHERE publication_sequence = ?1", [sequence]).unwrap();
    sequence
}

#[test]
fn parser_65_reader_ignores_legacy_evidence_before_attachment_wait_and_keeps_reset_readable() {
    let store = store();
    store
        .lock()
        .execute(
            "INSERT INTO session(environment_key, agent, session_id, source_kind, source_label,
        cwd, first_seen_at, last_seen_at) VALUES ('native', 'claude-code', 'session', 'file',
        '/synthetic/session.jsonl', '/synthetic', 'now', 'now')",
            [],
        )
        .unwrap();
    let incarnation: i64 = store
        .lock()
        .query_row("SELECT incarnation FROM session", [], |row| row.get(0))
        .unwrap();
    let activation = ActivationId {
        definition_id: "origin".into(),
        generation: Generation(1),
        scope: ActivationScope {
            environment: "native".into(),
            session_cwd: "/synthetic".into(),
            run: RunScope::Live,
        },
    };
    let scope = crate::check_store::StoreCheckStore::activation_scope_key(&activation.scope);
    store
        .lock()
        .execute(
            "INSERT INTO burn_check_activation(check_id, activation_scope_key, definition_revision,
        enabled, generation, bootstrap_sequence, applied_sequence, updated_at_epoch)
        VALUES ('origin', ?1, 'synthetic', 1, 1, 0, 0, 100)",
            [&scope],
        )
        .unwrap();
    for index in 0..45 {
        insert_claude_result(
            &store,
            &format!("legacy-{index}"),
            incarnation,
            "LEGACY_REQUEST_CANARY",
        );
    }
    let public = antiburn_local::analysis::PublicMessageObservation {
        source_format: SourceFormat::ClaudeJsonl,
        session_id: "session".into(),
        record_id: "legacy-call".into(),
        part_id: None,
        call_id: Some("legacy-call".into()),
        position: ToolResultPosition::FileBytes {
            record_start: 10,
            record_end: 20,
            item_index: 0,
        },
        revision: 1,
        change: ToolResultChange::Update,
        eligible: true,
        completeness: ToolResultCompleteness::Complete,
        context: Some(antiburn_local::analysis::NativeEventContext {
            first_eligible_position: ToolResultPosition::FileBytes {
                record_start: 10,
                record_end: 20,
                item_index: 0,
            },
            main_session: true,
            complete_through_event: true,
            user_messages: Vec::new(),
        }),
        payload: antiburn_local::analysis::PublicMessagePayload::ToolCall {
            tool_name: "bash".into(),
            input: Some(antiburn_local::analysis::NativeToolInput::Text(
                "LEGACY_REQUEST_CANARY".into(),
            )),
            facts: None,
            provider_executed: None,
        },
    };
    store.lock().execute("INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id,
        source_format, native_identity, native_position_json, native_revision, payload_json, complete,
        source_generation, claim_fence, live_eligible, session_incarnation, project_cwd, event_kind)
        VALUES ('update', 'native', 'claude-code', 'session', '\"claude_jsonl\"', 'legacy-call', ?1, 1, ?2,
            1, 1, 1, 1, ?3, '/synthetic', 'tool_call')",
        params![serde_json::to_string(&public.position).unwrap(), serde_json::to_string(&public).unwrap(), incarnation]).unwrap();
    store.lock().execute("INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id,
        source_format, complete, source_generation, claim_fence, session_incarnation, project_cwd, gap_reason)
        VALUES ('coverage_gap', 'native', 'claude-code', 'session', '\"claude_jsonl\"', 0, 1, 1,
            ?1, '/synthetic', 'native-unsafe-order')", [incarnation]).unwrap();
    let reader = StorePublishedEvidenceReader::for_live_project(&store, "native", "/synthetic")
        .with_activation(&activation);
    let before = PublishedEvidenceReader::read_batch(
        &reader,
        PublicationRead {
            after: PublicationCursor(0),
            limit: 100,
        },
    )
    .unwrap();
    assert_eq!(before.events.len(), 47);
    assert!(
        before
            .events
            .iter()
            .all(|event| matches!(event.change, PublishedEvidenceChange::Ignored))
    );
    assert!(!before.has_more);
    let boundary = {
        let mut connection = store.lock();
        let transaction = connection.transaction().unwrap();
        assert!(
            request_origin::ensure_boundary_in(
                &transaction,
                &SessionKey::new("native", "claude-code", "session"),
                1,
                2
            )
            .unwrap()
        );
        let sequence: i64 = transaction
            .query_row(
                "SELECT MAX(publication_sequence) FROM native_tool_result_publication",
                [],
                |row| row.get(0),
            )
            .unwrap();
        transaction.commit().unwrap();
        sequence
    };
    let batch = PublishedEvidenceReader::read_batch(
        &reader,
        PublicationRead {
            after: before.next,
            limit: 100,
        },
    )
    .unwrap();
    assert_eq!(batch.events.len(), 1);
    assert!(matches!(
        batch.events[0].change,
        PublishedEvidenceChange::SegmentReset { .. }
    ));
    assert_eq!(batch.next.0, boundary as u64);
    store
        .lock()
        .execute(
            "INSERT INTO burn_check_session_attachment(check_id, activation_scope_key,
        generation, branch_key, incarnation) VALUES ('origin', ?1, 1, ?2, ?3)",
            params![
                scope,
                SessionKey::new("native", "claude-code", "session").to_identity_json(),
                incarnation
            ],
        )
        .unwrap();
    let new = insert_claude_result(&store, "new-result", incarnation, "New safe result.");
    let events = PublishedEvidenceSource::new(reader, 100)
        .unwrap()
        .read_batch(PublicationCursor(boundary as u64))
        .unwrap();
    assert!(events.events.iter().any(
        |event| matches!(event, SourceEvent::Observation { cursor, .. } if cursor.0 == new as u64)
    ));
    assert!(!format!("{events:?}").contains("LEGACY_REQUEST_CANARY"));
    let startup = StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic")
        .with_activation(&activation)
        .recent_results_before(&SessionKey::new("native", "claude-code", "session"), None)
        .unwrap();
    assert_eq!(startup.len(), 1);
    assert!(
        matches!(startup[0], SourceEvent::Observation { cursor, .. } if cursor.0 == new as u64)
    );
    assert_eq!(
        StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic")
            .recent_results_before(&SessionKey::new("native", "claude-code", "session"), None)
            .unwrap()
            .len(),
        40
    );
    let replay = ActivationId {
        scope: ActivationScope {
            run: RunScope::Replay("synthetic".into()),
            ..activation.scope.clone()
        },
        ..activation.clone()
    };
    assert_eq!(
        StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic")
            .with_activation(&replay)
            .recent_results_before(&SessionKey::new("native", "claude-code", "session"), None)
            .unwrap()
            .len(),
        40
    );
}
