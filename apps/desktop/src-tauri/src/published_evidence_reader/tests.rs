use std::path::Path;

use antiburn_local::analysis::{
    SourceFormat, ToolResultChange, ToolResultCompleteness, ToolResultContext,
    ToolResultObservation, ToolResultPosition, ToolResultStatus,
};
use rusqlite::{Connection, StatementStatus, params};

use super::*;

mod merge;
mod request_origin;

fn store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-publication-reader-tests")).unwrap()
}

fn result(finality: bool, format: SourceFormat) -> ToolResultObservation {
    let position = ToolResultPosition::FileBytes {
        record_start: 10,
        record_end: 20,
        item_index: 0,
    };
    ToolResultObservation {
        facts: None,
        outcome: None,
        provider_executed: None,
        source_format: format,
        session_id: "session".into(),
        record_id: Some("record".into()),
        call_id: Some("call".into()),
        result_id: Some("result".into()),
        position,
        revision: 1,
        change: ToolResultChange::Update,
        tool_name: Some("bash".into()),
        tool_category: None,
        arguments: None,
        content: Some("Synthetic output".into()),
        status: ToolResultStatus::Success,
        finality,
        completeness: ToolResultCompleteness::Complete,
        context: Some(ToolResultContext {
            result_position: position,
            main_session: true,
            complete_through_event: true,
            user_messages: Vec::new(),
        }),
    }
}

fn insert_update(
    connection: &Connection,
    identity: &str,
    finality: bool,
    incarnation: Option<i64>,
    cwd: Option<&str>,
    format: SourceFormat,
) -> i64 {
    let result = result(finality, format);
    insert_observation(connection, identity, &result, incarnation, cwd)
}

fn insert_observation(
    connection: &Connection,
    identity: &str,
    result: &ToolResultObservation,
    incarnation: Option<i64>,
    cwd: Option<&str>,
) -> i64 {
    connection.execute(
        "INSERT INTO native_tool_result_publication (outcome, environment_key, agent, session_id,
             source_format, native_identity, native_position_json, native_revision, payload_json,
             complete, source_generation, claim_fence, live_eligible, session_incarnation, project_cwd)
         VALUES ('update', 'native', 'pi', 'session', ?1, ?2, ?3, 1, ?4, 1, 1, 1, 1, ?5, ?6)",
        params![serde_json::to_string(&result.source_format).unwrap(), identity,
            serde_json::to_string(&result.position).unwrap(), serde_json::to_string(&result).unwrap(),
            incarnation, cwd],
    ).unwrap();
    connection.last_insert_rowid()
}

#[test]
fn mode_evidence_qualifiers_use_the_frozen_native_source_format() {
    use antiburn_local::analysis::{NativeRequestAttribution, NativeToolFacts};

    for (format, qualifier) in [
        (SourceFormat::ClaudeJsonl, "reported-speed"),
        (SourceFormat::CodexRolloutJsonl, "requested-tier-preference"),
        (SourceFormat::OpenCodeSqliteV2, "selected-catalog-model"),
        (SourceFormat::PiV3Jsonl, "model-id-suffix-hint"),
    ] {
        let store = store();
        let mut result = result(true, format);
        result.facts = Some(NativeToolFacts {
            requester: NativeRequestAttribution {
                fast_mode: Some(true),
                ..Default::default()
            },
            ..Default::default()
        });
        let sequence =
            insert_observation(&store.lock(), "mode", &result, Some(7), Some("/synthetic"));
        result.facts.as_mut().unwrap().requester.fast_mode = None;
        let converted = publication_event(
            &StorePublishedEvidenceReader::new(&store),
            publication(&store, sequence),
            Some("/synthetic"),
            None,
        )
        .unwrap();
        let source = PublishedEvidenceSource::new(OnePublication(converted), 1).unwrap();
        let batch = source.read_batch(PublicationCursor(0)).unwrap();
        let SourceEvent::Observation { observation, .. } = &batch.events[0] else {
            panic!("expected an observation for {format:?}");
        };
        let antiburn_checks::domain::ObservationKind::ToolResult { message, .. } =
            &observation.kind
        else {
            panic!("expected a tool result for {format:?}");
        };
        assert_eq!(message.value["fast_mode"], true);
        assert_eq!(message.value["quality"]["fast_mode_evidence"], qualifier);
        assert!(message.value.get("model").is_none());
    }
    let missing = NativeToolFacts::default();
    let metadata = published_tool_metadata(Some(&missing), SourceFormat::PiV3Jsonl, None);
    assert!(metadata.attribution.fast_mode.is_none());
    assert!(metadata.fast_mode_evidence.is_none());
}

#[test]
fn native_result_version_keeps_raw_payload_bytes_and_legacy_observation_hash() {
    use crate::store::native_tool_results::evidence::{EvidenceEventKind, NativeCurrentBinding};
    let store = store();
    insert_project_session(&store.lock());
    let result = result(true, SourceFormat::PiV3Jsonl);
    let raw = serde_json::to_string_pretty(&result).unwrap();
    store.lock().execute("INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id,
        source_format, native_identity, native_position_json, native_revision, payload_json, complete,
        source_generation, claim_fence, live_eligible, session_incarnation, project_cwd)
        VALUES ('update', 'native', 'pi', 'session', ?1, 'raw-result', ?2, 1, ?3, 1, 1, 1, 1, 7, '/synthetic')",
        params![serde_json::to_string(&result.source_format).unwrap(), serde_json::to_string(&result.position).unwrap(), raw]).unwrap();
    let sequence = store.lock().last_insert_rowid();
    let binding = NativeCurrentBinding {
        identity: "raw-result".into(),
        event_kind: EvidenceEventKind::ToolResult,
        source_format: serde_json::to_string(&result.source_format).unwrap(),
        revision: 1,
        publication_sequence: sequence,
        payload_sha256: Sha256::digest(raw.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    };
    let reader = StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic");
    let legacy = reader
        .convert_publication(publication(&store, sequence))
        .unwrap();
    assert!(legacy.source_evidence_version.is_none());
    let generic = reader
        .read_batch(PublicationRead {
            after: PublicationCursor(0),
            limit: 1,
        })
        .unwrap()
        .events
        .pop()
        .unwrap();
    assert_eq!(generic.source_evidence_version, Some(binding.version()));
    let legacy = PublishedEvidenceSource::new(OnePublication(legacy), 1)
        .unwrap()
        .read_batch(PublicationCursor(0))
        .unwrap();
    let generic = PublishedEvidenceSource::new(OnePublication(generic), 1)
        .unwrap()
        .read_batch(PublicationCursor(0))
        .unwrap();
    let SourceEvent::Observation {
        observation: legacy,
        ..
    } = &legacy.events[0]
    else {
        panic!("legacy result expected")
    };
    let SourceEvent::Observation {
        observation: generic,
        source_evidence_version,
        ..
    } = &generic.events[0]
    else {
        panic!("generic result expected")
    };
    assert_eq!(generic, legacy);
    assert_ne!(&generic.version, source_evidence_version.as_ref().unwrap());
    assert!(
        !serde_json::to_string(&generic)
            .unwrap()
            .contains("source_evidence_version")
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT payload_json FROM native_tool_result_publication",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        raw
    );
}

#[test]
fn requester_and_path_facts_round_trip_through_immutable_publications() {
    use antiburn_local::analysis::{
        CheckToolCategory, NativeOperationPaths, NativeRequestAttribution, NativeToolFacts,
    };

    let store = store();
    let mut result = result(true, SourceFormat::PiV3Jsonl);
    result.tool_category = Some(antiburn_local::analysis::ToolCategory::Edit);
    result.facts = Some(NativeToolFacts {
        requester: NativeRequestAttribution {
            provider: Some("recorded-provider".into()),
            model: Some("namespace/recorded-model".into()),
            fast_mode: Some(false),
        },
        category: Some(CheckToolCategory::Write),
        operation_paths: Some(NativeOperationPaths {
            cwd: "/synthetic/nested".into(),
            paths: vec!["file.rs".into(), "/outside/file.rs".into()],
        }),
    });
    insert_project_session(&store.lock());
    let sequence = insert_observation(
        &store.lock(),
        "factual",
        &result,
        Some(7),
        Some("/synthetic"),
    );
    result.facts.as_mut().unwrap().requester.model = Some("changed-model".into());
    result.facts.as_mut().unwrap().requester.fast_mode = Some(true);

    let reader = StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic")
        .with_repository_root(Some("/synthetic"));
    let batch = PublishedEvidenceSource::new(reader, 10)
        .unwrap()
        .read_batch(PublicationCursor(0))
        .unwrap();
    let SourceEvent::Observation { observation, .. } = &batch.events[0] else {
        panic!("the admitted result must normalize");
    };
    let antiburn_checks::domain::ObservationKind::ToolResult { message, .. } = &observation.kind
    else {
        panic!("the result must keep its message identity");
    };
    let message = &message.value;
    assert_eq!(
        message["model"],
        "recorded-provider/namespace/recorded-model"
    );
    assert_eq!(message["fast_mode"], false);
    assert_eq!(message["tool"]["category"], "write");
    assert_eq!(
        message["tool"]["paths"],
        serde_json::json!(["nested/file.rs"])
    );
    assert_eq!(message["quality"]["paths_complete"], true);
    let saved = publication(&store, sequence)
        .payload
        .unwrap()
        .facts
        .unwrap();
    assert_eq!(saved.requester.fast_mode, Some(false));
    assert_eq!(saved.category, Some(CheckToolCategory::Write));
}

#[test]
fn unavailable_new_facts_do_not_inherit_metric_classes_or_argument_text() {
    let store = store();
    let mut result = result(true, SourceFormat::OpenCodeSqliteV2);
    result.facts = Some(Default::default());
    result.provider_executed = Some(true);
    result.tool_category = Some(antiburn_local::analysis::ToolCategory::Bash);
    result.arguments = Some(
        serde_json::json!({"model":"current/model", "fast_mode":false, "file_path":"/synthetic/guessed.rs"}),
    );
    let sequence = insert_observation(
        &store.lock(),
        "unavailable-facts",
        &result,
        Some(7),
        Some("/synthetic"),
    );
    let converted = publication_event(
        &StorePublishedEvidenceReader::new(&store),
        publication(&store, sequence),
        Some("/synthetic"),
        None,
    )
    .unwrap();
    let source = PublishedEvidenceSource::new(OnePublication(converted), 1).unwrap();
    let batch = source.read_batch(PublicationCursor(0)).unwrap();
    let SourceEvent::Observation { observation, .. } = &batch.events[0] else {
        panic!("available partial content must remain usable");
    };
    let antiburn_checks::domain::ObservationKind::ToolResult { message, .. } = &observation.kind
    else {
        panic!("the result must keep its message identity");
    };
    let message = &message.value;
    assert!(message.get("model").is_none());
    assert!(message.get("fast_mode").is_none());
    assert!(message["tool"].get("category").is_none());
    assert!(message["tool"].get("paths").is_none());
    assert_eq!(message["quality"]["paths_available"], false);
}

fn insert_project_session(connection: &Connection) {
    connection.execute(
        "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
             cwd, first_seen_at, last_seen_at, incarnation)
         VALUES ('native', 'pi', 'session', 'synthetic', 'synthetic', '/synthetic', 'now', 'now', 7)",
        [],
    ).unwrap();
}

fn insert_opencode_v2_session(connection: &Connection) {
    connection
        .execute(
            "INSERT INTO session (environment_key, agent, session_id, source_kind,
                 source_label, cwd, first_seen_at, last_seen_at, incarnation, source_format)
             VALUES ('native', 'opencode', 'ses_v2', 'providerDb',
                 'opencode:ses_v2:open_code_v2_sqlite', '/synthetic', 'now', 'now', 7, ?1)",
            [serde_json::to_string(&SourceFormat::OpenCodeSqliteV2).unwrap()],
        )
        .unwrap();
}

fn insert_opencode_v2_result(connection: &Connection, sequence: u64) {
    let mut result = result(true, SourceFormat::OpenCodeSqliteV2);
    result.session_id = "ses_v2".into();
    result.record_id = Some(format!("message-{sequence}"));
    result.call_id = Some(format!("call-{sequence}"));
    result.result_id = Some(format!("result-{sequence}"));
    let position = ToolResultPosition::OpenCodeV2Event {
        sequence,
        item_index: 0,
    };
    result.position = position;
    result.context.as_mut().unwrap().result_position = position;
    let source_format = serde_json::to_string(&SourceFormat::OpenCodeSqliteV2).unwrap();
    let identity = format!("v2-result-{sequence}");
    let position_json = serde_json::to_string(&position).unwrap();
    let payload = serde_json::to_string(&result).unwrap();
    connection
        .execute(
            "INSERT INTO native_tool_result_publication (
                 outcome, environment_key, agent, session_id, source_format,
                 native_identity, native_position_json, native_revision, payload_json,
                 complete, source_generation, claim_fence, live_eligible,
                 session_incarnation, project_cwd)
             VALUES ('update', 'native', 'opencode', 'ses_v2', ?1, ?2, ?3, 1, ?4,
                 1, 1, 1, 1, 7, '/synthetic')",
            params![source_format, identity, position_json, payload],
        )
        .unwrap();
    let publication_sequence = connection.last_insert_rowid();
    connection
        .execute(
            "INSERT INTO native_tool_result_current (
                 environment_key, agent, session_id, source_format, native_identity,
                 native_position_json, native_revision, payload_json, complete,
                 is_removed, publication_sequence, source_generation, claim_fence)
             VALUES ('native', 'opencode', 'ses_v2', ?1, ?2, ?3, 1, ?4,
                 1, 0, ?5, 1, 1)",
            params![
                serde_json::to_string(&SourceFormat::OpenCodeSqliteV2).unwrap(),
                identity,
                position_json,
                payload,
                publication_sequence,
            ],
        )
        .unwrap();
}

fn publication(store: &Store, sequence: i64) -> ToolResultPublication {
    let publication = store
        .native_tool_result_publications_after(sequence - 1, 1)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(publication.publication_sequence, sequence);
    publication
}

#[test]
fn nonterminal_and_control_events_skip_terminal_history_but_updates_require_admission_state() {
    let store = store();
    let sequence = insert_update(
        &store.lock(),
        "target",
        false,
        Some(7),
        Some("/synthetic"),
        SourceFormat::PiV3Jsonl,
    );
    let mut event = publication(&store, sequence);
    store
        .lock()
        .execute_batch("DROP TABLE native_tool_result_publication")
        .unwrap();
    let reader = StorePublishedEvidenceReader::new(&store);
    for outcome in [
        ToolResultPublicationOutcome::Update,
        ToolResultPublicationOutcome::Removal,
        ToolResultPublicationOutcome::CoverageGap,
        ToolResultPublicationOutcome::CoverageRestored,
    ] {
        event.outcome = outcome;
        if let Some(result) = &mut event.payload {
            result.finality = outcome != ToolResultPublicationOutcome::Update;
        }
        assert_eq!(reader.first_terminal_publication(&event).unwrap(), None);
        if outcome == ToolResultPublicationOutcome::Update {
            assert!(
                matches!(reader.convert_publication(event.clone()), Err(SourceError::Reader(message)) if message.contains("no such table"))
            );
            continue;
        }
        let converted = reader.convert_publication(event.clone()).unwrap();
        assert_eq!(
            converted.cursor,
            PublicationCursor(u64::try_from(sequence).unwrap())
        );
        assert_eq!(converted.branch, event.session.to_identity_json());
    }
}

#[test]
fn startup_window_excludes_v2_stored_order() {
    let store = store();
    {
        let connection = store.lock();
        insert_opencode_v2_session(&connection);
        for sequence in 1..=45 {
            insert_opencode_v2_result(&connection, sequence);
        }
    }

    let events = StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic")
        .recent_results_before(&SessionKey::new("native", "opencode", "ses_v2"), None)
        .unwrap();

    assert!(events.is_empty());
}

#[test]
fn typed_gap_kind_controls_invalidation_without_source_or_reason_inference() {
    let store = store();
    let sequence = insert_update(
        &store.lock(),
        "gap",
        false,
        Some(7),
        Some("/synthetic"),
        SourceFormat::PiV3Jsonl,
    );
    let mut event = publication(&store, sequence);
    event.outcome = ToolResultPublicationOutcome::CoverageGap;
    event.gap_reason = Some("future-history-reason".into());
    event.payload = None;
    for (kind, expected) in [
        (PublicationGapKind::Invalidated, "unsafe"),
        (PublicationGapKind::Partial, "partial"),
        (PublicationGapKind::SegmentReset, "reset"),
    ] {
        event.gap_kind = kind;
        let converted = StorePublishedEvidenceReader::new(&store)
            .convert_publication(event.clone())
            .unwrap();
        let source = PublishedEvidenceSource::new(OnePublication(converted), 1).unwrap();
        let batch = source.read_batch(PublicationCursor(0)).unwrap();
        let actual = match &batch.events[0] {
            SourceEvent::UnsafeUpdate { .. } => "unsafe",
            SourceEvent::Gap { .. } => "partial",
            SourceEvent::SegmentReset { .. } => "reset",
            other => panic!("unexpected gap event: {other:?}"),
        };
        assert_eq!(actual, expected);
    }
}

#[test]
fn ignored_terminal_updates_skip_context_but_require_readable_admission_state() {
    let store = store();
    let sequence = insert_update(
        &store.lock(),
        "target",
        true,
        Some(7),
        Some("/synthetic"),
        SourceFormat::PiV3Jsonl,
    );
    let mut event = publication(&store, sequence);
    event.live_eligible = Some(false);
    assert!(event.payload.as_ref().unwrap().context.is_some());
    let reader = StorePublishedEvidenceReader::for_live_project(&store, "native", "/synthetic");
    let converted = reader.convert_publication(event.clone()).unwrap();
    store.lock().execute_batch(
        "ALTER TABLE native_tool_result_publication RENAME COLUMN source_format TO unavailable_format",
    ).unwrap();
    assert!(reader.first_terminal_publication(&event).is_err());
    assert!(
        matches!(reader.convert_publication(event.clone()), Err(SourceError::Reader(message)) if message.contains("source_format"))
    );
    assert_eq!(converted.change, PublishedEvidenceChange::Ignored);
    assert_eq!(converted.context, None);
    assert_eq!(
        converted.cursor,
        PublicationCursor(u64::try_from(sequence).unwrap())
    );
    assert_eq!(converted.branch, event.session.to_identity_json());
}

#[test]
fn first_terminal_lookup_seeks_exact_identity_and_binding_without_scanning_session_history() {
    let store = store();
    let (first, correction) = {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        for index in 0..4_096 {
            insert_update(
                &tx,
                &format!("unrelated-{index}"),
                true,
                Some(7),
                Some("/synthetic"),
                SourceFormat::PiV3Jsonl,
            );
        }
        for (incarnation, cwd, format) in [
            (None, None, SourceFormat::PiV3Jsonl),
            (Some(6), Some("/synthetic"), SourceFormat::PiV3Jsonl),
            (None, Some("/synthetic"), SourceFormat::PiV3Jsonl),
            (Some(7), None, SourceFormat::PiV3Jsonl),
            (Some(7), Some("/other-project"), SourceFormat::PiV3Jsonl),
            (Some(7), Some("/synthetic"), SourceFormat::ClaudeJsonl),
        ] {
            insert_update(&tx, "target", true, incarnation, cwd, format);
        }
        insert_update(
            &tx,
            "target",
            false,
            Some(7),
            Some("/synthetic"),
            SourceFormat::PiV3Jsonl,
        );
        let first = insert_update(
            &tx,
            "target",
            true,
            Some(7),
            Some("/synthetic"),
            SourceFormat::PiV3Jsonl,
        );
        let correction = insert_update(
            &tx,
            "target",
            true,
            Some(7),
            Some("/synthetic"),
            SourceFormat::PiV3Jsonl,
        );
        tx.commit().unwrap();
        (first, correction)
    };
    let event = publication(&store, correction);
    let reader = StorePublishedEvidenceReader::new(&store);
    assert_eq!(
        reader.first_terminal_publication(&event).unwrap(),
        Some(PublicationCursor(u64::try_from(first).unwrap()))
    );
    let connection = store.lock();
    let plan = connection
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {FIRST_TERMINAL_PUBLICATION_SQL}"
        ))
        .unwrap()
        .query_map([correction], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        plan.iter().any(|line| line
            .contains("SEARCH p USING INDEX native_tool_result_publication_identity")
            && line.contains("native_identity=?")
            && line.contains("session_incarnation=?")
            && line.contains("project_cwd=?")
            && line.contains("publication_sequence<?")),
        "{plan:?}"
    );
    assert!(
        !plan
            .iter()
            .any(|line| line.starts_with("SCAN p") || line.contains("USE TEMP B-TREE")),
        "{plan:?}"
    );
    let mut statement = connection.prepare(FIRST_TERMINAL_PUBLICATION_SQL).unwrap();
    assert_eq!(
        statement
            .query_row([correction], |row| row.get::<_, i64>(0))
            .unwrap(),
        first
    );
    assert_eq!(statement.get_status(StatementStatus::FullscanStep), 0);
    assert!(
        statement.get_status(StatementStatus::VmStep) < 200,
        "lookup steps: {}",
        statement.get_status(StatementStatus::VmStep)
    );
    let columns = connection
        .prepare("PRAGMA index_info('native_tool_result_publication_identity')")
        .unwrap()
        .query_map([], |row| row.get::<_, Option<String>>(2))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(columns.iter().all(Option::is_some));
    assert!(
        !columns
            .iter()
            .any(|column| column.as_deref() == Some("payload_json"))
    );
}

#[test]
fn empty_project_page_advances_past_unrelated_publications_and_reads_later_matches() {
    let store = store();
    {
        let connection = store.lock();
        insert_project_session(&connection);
        for identity in ["other-1", "other-2"] {
            insert_update(
                &connection,
                identity,
                true,
                Some(7),
                Some("/other-project"),
                SourceFormat::PiV3Jsonl,
            );
        }
    }

    let reader = StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic");
    let empty = reader
        .read_batch(PublicationRead {
            after: PublicationCursor(0),
            limit: 10,
        })
        .unwrap();
    assert!(empty.events.is_empty());
    assert_eq!(empty.next, PublicationCursor(2));
    assert!(!empty.has_more);

    let repeated_empty = reader
        .read_batch(PublicationRead {
            after: empty.next,
            limit: 10,
        })
        .unwrap();
    assert_eq!(repeated_empty.next, empty.next);
    assert!(!repeated_empty.has_more);

    let source = PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic"),
        1,
    )
    .unwrap();
    let normalized = source.read_batch(PublicationCursor(0)).unwrap();
    assert!(normalized.events.is_empty());
    assert_eq!(normalized.next, empty.next);
    assert!(!normalized.has_more);

    let ahead = source.read_batch(PublicationCursor(10)).unwrap();
    assert!(ahead.events.is_empty());
    assert_eq!(ahead.next, PublicationCursor(10));
    assert!(!ahead.has_more);

    let (unrelated, matching) = {
        let connection = store.lock();
        let unrelated = insert_update(
            &connection,
            "other-3",
            true,
            Some(7),
            Some("/other-project"),
            SourceFormat::PiV3Jsonl,
        );
        let matching = insert_update(
            &connection,
            "target",
            true,
            Some(7),
            Some("/synthetic"),
            SourceFormat::PiV3Jsonl,
        );
        (unrelated, matching)
    };
    assert_eq!(unrelated, 3);
    let later = reader
        .read_batch(PublicationRead {
            after: empty.next,
            limit: 10,
        })
        .unwrap();
    assert_eq!(later.events.len(), 1);
    assert_eq!(
        later.next,
        PublicationCursor(u64::try_from(matching).unwrap())
    );
    assert_eq!(later.events[0].cursor, later.next);
    assert!(!later.has_more);
}
