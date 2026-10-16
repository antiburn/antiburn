use std::path::Path;

use antiburn_local::analysis::{
    SourceFormat, ToolResultChange, ToolResultCompleteness, ToolResultObservation,
    ToolResultPosition, ToolResultStatus,
};

use super::super::model::{AnalysisRecord, EvidenceCompletion, PublishedEvidence, SessionRecord};
use super::super::native_tool_results::ToolResultBatch;
use super::super::*;
use super::{ToolResultPublicationOutcome, ToolResultPublishOptions};

fn store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-native-tool-result-test"))
        .expect("in-memory store")
}

fn session() -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", "opencode", "session-1"),
        source_kind: "file".into(),
        source_label: "/home/test/.local/share/opencode/session.jsonl".into(),
        wsl_distro: None,
        title: Some("test".into()),
        title_source: Some("explicit".into()),
        cwd: Some("/home/test/repo".into()),
        surface: "cli".into(),
        updated_at_epoch: Some(1_000),
        activity_cursor: String::new(),
        activity_source: "mtime".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: Some("fp-1".into()),
    }
}

fn result(content: &str, revision: u64, change: ToolResultChange) -> ToolResultObservation {
    ToolResultObservation {
        context: None,
        source_format: SourceFormat::OpenCodeSqliteV2,
        session_id: "session-1".into(),
        record_id: Some("message-1".into()),
        call_id: Some("call-1".into()),
        result_id: Some("part-1".into()),
        position: ToolResultPosition::OpenCodeEvent {
            sequence: revision,
            item_index: 0,
        },
        revision,
        change,
        tool_name: Some("shell".into()),
        tool_category: None,
        arguments: Some(serde_json::json!({"command": "true"})),
        content: Some(content.into()),
        status: ToolResultStatus::Success,
        finality: true,
        completeness: ToolResultCompleteness::Complete,
    }
}

fn claim_and_record(store: &Store) -> (AnalysisRecord, EvidenceClaim) {
    claim_and_record_for(store, session())
}

fn claim_and_record_for(store: &Store, session: SessionRecord) -> (AnalysisRecord, EvidenceClaim) {
    store
        .upsert_sessions(
            std::slice::from_ref(&session),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let claim = store
        .claim_next_evidence(&[session.key.agent.as_str()], 100, 60)
        .unwrap()
        .expect("session claim");
    let revisions = crate::analysis::projection_revisions();
    let record = AnalysisRecord {
        key: session.key,
        model_breakdown_json: "{}".into(),
        pricing_breakdown_json: "{}".into(),
        inclusive_models_json: "[]".into(),
        initial_context_json: None,
        source_summaries_json: None,
        provider_hints_json: None,
        source_fingerprint: "fp-1".into(),
        pricing_generation: 1,
        analyzed_generation: claim.source_generation,
        parser_revision: revisions.parser_revision,
        analyzer_revision: revisions.analyzer_revision,
        metrics_schema_revision: revisions.metrics_schema_revision,
    };
    (record, claim)
}

fn publish(
    store: &Store,
    record: &AnalysisRecord,
    claim: &EvidenceClaim,
    batch: &ToolResultBatch,
) -> bool {
    store
        .publish_projections_with_native_tool_results(
            record,
            None,
            &EvidenceCompletion {
                claim_fence: claim.claim_fence,
                status: PublishedEvidence::Ready,
                evidence_schema_revision: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                evidence_json: crate::store::test_support::evidence_json(&claim.key),
            },
            &[],
            &[],
            ToolResultPublishOptions {
                source_fingerprint: Some("fp-1"),
                batch: Some(batch),
            },
        )
        .unwrap()
}

fn next_claim(store: &Store, key: &SessionKey) -> (AnalysisRecord, EvidenceClaim) {
    mark_evidence_pending_in(&store.lock(), key).unwrap();
    let claim = store
        .claim_next_evidence(&[key.agent.as_str()], 200, 60)
        .unwrap()
        .expect("next claim");
    let revisions = crate::analysis::projection_revisions();
    let record = AnalysisRecord {
        key: key.clone(),
        model_breakdown_json: "{}".into(),
        pricing_breakdown_json: "{}".into(),
        inclusive_models_json: "[]".into(),
        initial_context_json: None,
        source_summaries_json: None,
        provider_hints_json: None,
        source_fingerprint: "fp-1".into(),
        pricing_generation: 1,
        analyzed_generation: claim.source_generation,
        parser_revision: revisions.parser_revision,
        analyzer_revision: revisions.analyzer_revision,
        metrics_schema_revision: revisions.metrics_schema_revision,
    };
    (record, claim)
}

fn batch(results: Vec<ToolResultObservation>, complete: bool) -> ToolResultBatch {
    batch_with_format(SourceFormat::OpenCodeSqliteV2, results, complete)
}

fn batch_with_format(
    source_format: SourceFormat,
    results: Vec<ToolResultObservation>,
    complete: bool,
) -> ToolResultBatch {
    ToolResultBatch {
        source_format,
        results,
        complete,
    }
}

#[test]
fn deduplicates_unchanged_observations_and_publishes_content_corrections() {
    let store = store();
    let (first_record, first_claim) = claim_and_record(&store);
    let first = result("first output", 1, ToolResultChange::Update);
    assert!(publish(
        &store,
        &first_record,
        &first_claim,
        &batch(vec![first.clone()], true)
    ));
    assert_eq!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .len(),
        1
    );

    let (same_record, same_claim) = next_claim(&store, &first_record.key);
    assert!(publish(
        &store,
        &same_record,
        &same_claim,
        &batch(vec![first], true)
    ));
    assert_eq!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .len(),
        1
    );

    let (corrected_record, corrected_claim) = next_claim(&store, &first_record.key);
    let correction = result("corrected output", 2, ToolResultChange::Update);
    assert!(publish(
        &store,
        &corrected_record,
        &corrected_claim,
        &batch(vec![correction.clone()], true)
    ));
    let events = store.native_tool_result_publications_after(0, 10).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[1].publication_sequence,
        events[0].publication_sequence + 1
    );
    assert_eq!(events[1].native_position, Some(correction.position));
    assert_eq!(events[1].native_revision, Some(2));
    assert_eq!(events[1].payload, Some(correction));
    assert_eq!(
        store
            .native_tool_results_current(&first_record.key)
            .unwrap()[0]
            .result
            .content
            .as_deref(),
        Some("corrected output")
    );
}

#[test]
fn complete_snapshot_and_explicit_removal_create_deduplicated_tombstones() {
    let store = store();
    let (record, claim) = claim_and_record(&store);
    assert!(publish(
        &store,
        &record,
        &claim,
        &batch(vec![result("output", 1, ToolResultChange::Update)], true)
    ));

    let (removal_record, removal_claim) = next_claim(&store, &record.key);
    let removal = result("", 2, ToolResultChange::Removal);
    assert!(publish(
        &store,
        &removal_record,
        &removal_claim,
        &batch(vec![removal.clone()], true)
    ));
    assert!(store.native_tool_results_current(&record.key).unwrap()[0].removed);
    let event_count = store
        .native_tool_result_publications_after(0, 10)
        .unwrap()
        .len();

    let (repeat_record, repeat_claim) = next_claim(&store, &record.key);
    assert!(publish(
        &store,
        &repeat_record,
        &repeat_claim,
        &batch(vec![removal], true)
    ));
    assert_eq!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .len(),
        event_count
    );

    let (empty_record, empty_claim) = next_claim(&store, &record.key);
    assert!(publish(
        &store,
        &empty_record,
        &empty_claim,
        &batch(Vec::new(), true)
    ));
    assert_eq!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .len(),
        event_count
    );
}

#[test]
fn complete_snapshot_format_change_removes_prior_format_current_results() {
    let store = store();
    let (first_record, first_claim) = claim_and_record(&store);
    assert!(publish(
        &store,
        &first_record,
        &first_claim,
        &batch(
            vec![result("old format", 1, ToolResultChange::Update)],
            true
        )
    ));

    let (next_record, next_claim) = next_claim(&store, &first_record.key);
    let mut next_format_result = result("new format", 1, ToolResultChange::Update);
    next_format_result.source_format = SourceFormat::ClaudeJsonl;
    assert!(publish(
        &store,
        &next_record,
        &next_claim,
        &batch_with_format(SourceFormat::ClaudeJsonl, vec![next_format_result], true)
    ));

    let current = store
        .native_tool_results_current(&first_record.key)
        .unwrap();
    let old_format = current
        .iter()
        .find(|result| result.source_format == "open_code_sqlite_v2")
        .expect("prior-format result remains as a tombstone");
    assert!(old_format.removed);
    let new_format = current
        .iter()
        .find(|result| result.source_format == "claude_jsonl")
        .expect("new-format result is current");
    assert!(!new_format.removed);
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .iter()
            .any(
                |event| event.outcome == ToolResultPublicationOutcome::Removal
                    && event.source_format.as_deref() == Some("\"open_code_sqlite_v2\"")
            )
    );
}

#[test]
fn incomplete_snapshot_records_a_gap_and_never_infers_deletion() {
    let store = store();
    let (record, claim) = claim_and_record(&store);
    assert!(publish(
        &store,
        &record,
        &claim,
        &batch(vec![result("output", 1, ToolResultChange::Update)], true)
    ));
    let (incomplete_record, incomplete_claim) = next_claim(&store, &record.key);
    assert!(publish(
        &store,
        &incomplete_record,
        &incomplete_claim,
        &batch(Vec::new(), false)
    ));

    let events = store.native_tool_result_publications_after(0, 10).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].outcome, ToolResultPublicationOutcome::CoverageGap);
    assert!(!events[1].complete);
    assert_eq!(
        store
            .native_tool_results_current(&record.key)
            .unwrap()
            .len(),
        1
    );
    assert!(!store.native_tool_results_current(&record.key).unwrap()[0].removed);

    let (recovered_record, recovered_claim) = next_claim(&store, &record.key);
    assert!(publish(
        &store,
        &recovered_record,
        &recovered_claim,
        &batch(vec![result("output", 1, ToolResultChange::Update)], true),
    ));
    let recovered = store
        .native_tool_result_publications_after(events[1].publication_sequence, 10)
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        recovered[0].outcome,
        ToolResultPublicationOutcome::CoverageRestored
    );
    assert!(recovered[0].complete);
}

#[test]
fn lost_claim_commits_no_native_result_event_or_current_state() {
    let store = store();
    let (record, claim) = claim_and_record(&store);
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            rusqlite::params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();

    assert!(!publish(
        &store,
        &record,
        &claim,
        &batch(
            vec![result("must not publish", 1, ToolResultChange::Update)],
            false
        ),
    ));
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .native_tool_results_current(&record.key)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn read_cursor_is_bounded_and_returns_only_sequences_after_the_cursor() {
    let store = store();
    let (first_record, first_claim) = claim_and_record(&store);
    assert!(publish(
        &store,
        &first_record,
        &first_claim,
        &batch(vec![result("one", 1, ToolResultChange::Update)], true)
    ));
    let (second_record, second_claim) = next_claim(&store, &first_record.key);
    assert!(publish(
        &store,
        &second_record,
        &second_claim,
        &batch(vec![result("two", 2, ToolResultChange::Update)], true)
    ));

    let first_page = store.native_tool_result_publications_after(0, 1).unwrap();
    assert_eq!(first_page.len(), 1);
    let next_page = store
        .native_tool_result_publications_after(first_page[0].publication_sequence, 10)
        .unwrap();
    assert_eq!(next_page.len(), 1);
    assert!(next_page[0].publication_sequence > first_page[0].publication_sequence);
    assert!(store.native_tool_result_publications_after(-1, 1).is_err());
    assert!(store.native_tool_result_publications_after(0, 0).is_err());
}

#[test]
fn deleting_session_purges_its_outbox_payload_after_session_removal() {
    let store = store();
    let (record, claim) = claim_and_record(&store);
    assert!(publish(
        &store,
        &record,
        &claim,
        &batch(
            vec![result("private output", 1, ToolResultChange::Update)],
            true
        ),
    ));
    assert_eq!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .len(),
        1
    );

    store
        .delete_session(&record.key)
        .unwrap()
        .expect("session removed");
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .native_tool_results_current(&record.key)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn clear_local_data_purges_native_result_payloads() {
    let store = store();
    let (record, claim) = claim_and_record(&store);
    assert!(publish(
        &store,
        &record,
        &claim,
        &batch(
            vec![result("private output", 1, ToolResultChange::Update)],
            true
        ),
    ));

    store.clear_local_session_data().unwrap();
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .native_tool_results_current(&record.key)
            .unwrap()
            .is_empty()
    );
}

#[path = "reader_tests.rs"]
mod reader_tests;
