use std::path::Path;

use antiburn_local::analysis::{
    SourceFormat, ToolResultChange, ToolResultCompleteness, ToolResultContext,
    ToolResultObservation, ToolResultPosition, ToolResultStatus,
};
use rusqlite::{Connection, StatementStatus, params};

use super::*;

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
    connection.execute(
        "INSERT INTO native_tool_result_publication (outcome, environment_key, agent, session_id,
             source_format, native_identity, native_position_json, native_revision, payload_json,
             complete, source_generation, claim_fence, live_eligible, session_incarnation, project_cwd)
         VALUES ('update', 'native', 'pi', 'session', ?1, ?2, ?3, 1, ?4, 1, 1, 1, 1, ?5, ?6)",
        params![serde_json::to_string(&format).unwrap(), identity,
            serde_json::to_string(&result.position).unwrap(), serde_json::to_string(&result).unwrap(),
            incarnation, cwd],
    ).unwrap();
    connection.last_insert_rowid()
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
                 source_label, cwd, first_seen_at, last_seen_at, incarnation)
             VALUES ('native', 'opencode', 'ses_v2', 'providerDb',
                 'opencode:ses_v2:open_code_v2_sqlite', '/synthetic', 'now', 'now', 7)",
            [],
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
fn nonterminal_removal_and_coverage_events_do_not_query_first_terminal_history() {
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
        let converted = reader.convert_publication(event.clone()).unwrap();
        assert_eq!(
            converted.cursor,
            PublicationCursor(u64::try_from(sequence).unwrap())
        );
        assert_eq!(converted.branch, event.session.to_identity_json());
    }
}

#[test]
fn startup_window_orders_more_than_forty_v2_results_by_event_sequence() {
    let store = store();
    {
        let connection = store.lock();
        insert_opencode_v2_session(&connection);
        for sequence in 1..=45 {
            insert_opencode_v2_result(&connection, sequence);
        }
    }

    let events = StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic")
        .recent_results(&SessionKey::new("native", "opencode", "ses_v2"))
        .unwrap();

    assert_eq!(events.len(), 40);
    let positions: Vec<_> = events
        .iter()
        .map(|event| {
            let SourceEvent::Observation { observation, .. } = event else {
                panic!("expected a V2 result observation");
            };
            observation.native_order.0.clone()
        })
        .collect();
    let expected: Vec<_> = (6_u64..=45)
        .map(|sequence| {
            serde_json::to_string(&ToolResultPosition::OpenCodeV2Event {
                sequence,
                item_index: 0,
            })
            .unwrap()
        })
        .collect();
    assert_eq!(positions, expected);
}

#[test]
fn ignored_terminal_updates_skip_history_and_context_conversion_but_keep_delivery_identity() {
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
    store.lock().execute_batch(
        "ALTER TABLE native_tool_result_publication RENAME COLUMN source_format TO unavailable_format",
    ).unwrap();
    let reader = StorePublishedEvidenceReader::for_live_project(&store, "native", "/synthetic");
    assert!(reader.first_terminal_publication(&event).is_err());
    let converted = reader.convert_publication(event.clone()).unwrap();
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
fn empty_project_page_advances_to_global_head_and_reads_later_matching_publications() {
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
