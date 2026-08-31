//! Store-level pinning tests for atomic evidence publication.
//!
//! `Store::publish_projections` guards its write with the evidence row's own
//! claim fence and the session's current source generation (see `store/mod.rs`).
//! A losing pass must publish nothing it computed, and a winning pass must
//! remove every turn row an earlier, superseded pass left behind. This module
//! pins both halves directly against turn rows and a zero-baseline session,
//! a case the existing suite in `store/tests.rs` does not cover on its own.
//! See `docs/plans/local-insights-followups.md` for the wider publish
//! contract this backs.

use std::path::Path;

use antiburn_local::analysis::{TurnRow, TurnScope, count_turn_rows, insert_turn_rows};

use super::model::PublishedEvidence;
use super::*;

fn store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-publish-test-state")).expect("in-memory store")
}

fn session(session_id: &str, updated_at: i64) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", "claude-code", session_id),
        source_kind: "file".into(),
        source_label: format!("/home/avery/.claude/projects/demo/{session_id}.jsonl"),
        wsl_distro: None,
        title: Some("Wire the popover".into()),
        title_source: Some("explicit".into()),
        cwd: Some("/home/avery/code/widgets".into()),
        surface: "cli".into(),
        updated_at_epoch: Some(updated_at),
        activity_cursor: String::new(),
        activity_source: "mtime".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: None,
    }
}

fn projection_record(key: SessionKey, fingerprint: &str, generation: i64) -> AnalysisRecord {
    AnalysisRecord {
        key,
        model_breakdown_json: "{}".into(),
        inclusive_models_json: "[]".into(),
        source_fingerprint: fingerprint.into(),
        pricing_generation: 1,
        analyzed_generation: generation,
        parser_revision: 1,
        analyzer_revision: 1,
        metrics_schema_revision: 1,
    }
}

fn claimed_projection(
    store: &Store,
    session_id: &str,
    now_epoch: i64,
    lease_secs: i64,
) -> (AnalysisRecord, EvidenceClaim) {
    let mut record = session(session_id, 1_000);
    let fingerprint = format!("sv1:{session_id}");
    record.source_fingerprint = Some(fingerprint.clone());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let claim = store
        .claim_next_evidence(&["claude-code"], now_epoch, lease_secs)
        .unwrap()
        .unwrap();
    (
        projection_record(record.key, &fingerprint, claim.source_generation),
        claim,
    )
}

fn evidence_completion(
    claim: &EvidenceClaim,
    status: PublishedEvidence,
    evidence_json: String,
) -> EvidenceCompletion {
    EvidenceCompletion {
        claim_fence: claim.claim_fence,
        status,
        evidence_schema_revision: 1,
        evidence_json,
        diagnostics_json: Some("[]".into()),
    }
}

fn turn_row(turn_index: u64) -> TurnRow {
    TurnRow {
        source_key: "s1".into(),
        thread_id: "s1".into(),
        turn_index,
        scope: TurnScope::Main,
        child_id: None,
        role: "assistant",
        ts_ms: Some(1_000 + turn_index as i64),
        model: Some("claude-opus-4-6".into()),
        effort: None,
        speed: None,
        input_tokens: 10,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        output_tokens: 5,
        is_compaction_boundary: false,
        message_id: None,
        uuid: None,
        parent_uuid: None,
        compaction_trigger: None,
        compaction_pre_tokens: None,
        compaction_post_tokens: None,
        content: Vec::new(),
    }
}

/// Advances the session's source generation past the given claim, the same
/// way a concurrent, faster pass would before this pass tries to publish.
fn advance_source_generation_past(store: &Store, key: &SessionKey) {
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
        )
        .unwrap();
}

/* I1(a): a lost race publishes nothing, from a zero baseline. */

#[test]
fn a_lost_race_from_a_zero_baseline_publishes_no_evidence_no_analysis_and_no_turn_rows() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "lost-race-zero-baseline", 100, 60);
    let key = record.key.clone();
    // Nothing has ever published for this session: no session_analysis row
    // exists yet, and session_evidence still sits at its claimed, unpublished
    // state.
    assert!(store.analysis(&key).unwrap().is_none());

    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0)]).unwrap();

    // A faster, concurrent pass wins the race first, which advances the
    // session's source generation past this pass's own claim. The baseline
    // is captured after this, so the comparison below isolates exactly what
    // `publish_projections` itself is and is not allowed to change.
    advance_source_generation_past(&store, &key);
    let evidence_before = store.evidence(&key).unwrap().unwrap();
    assert_eq!(evidence_before.evidence_json, None);
    let completion =
        evidence_completion(&claim, PublishedEvidence::Ready, "{\"lost\":true}".into());

    let published = store
        .publish_projections(&record, None, &completion, &[])
        .unwrap();
    assert!(
        !published,
        "a lost race must report that it did not publish"
    );

    // No session_analysis row appeared, the evidence row is exactly what it
    // was before this pass ran, and this pass's own turn rows are gone.
    assert!(store.analysis(&key).unwrap().is_none());
    assert_eq!(store.evidence(&key).unwrap().unwrap(), evidence_before);
    let connection = store.lock();
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), claim.claim_fence).unwrap(),
        0
    );
}

#[test]
fn a_lost_race_leaves_relations_untouched() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "lost-race-relations", 100, 60);
    let key = record.key.clone();
    store
        .replace_relations(
            &key,
            RelationKind::Subagent,
            &[RelationRecord {
                kind: RelationKind::Subagent,
                related_id: "kept-child".into(),
                label: None,
            }],
        )
        .unwrap();
    let relations_before = store.relations(&key).unwrap();

    advance_source_generation_past(&store, &key);
    let completion =
        evidence_completion(&claim, PublishedEvidence::Ready, "{\"lost\":true}".into());
    let losing_relations = [RelationRecord {
        kind: RelationKind::Subagent,
        related_id: "should-not-land".into(),
        label: None,
    }];

    assert!(
        !store
            .publish_projections(&record, None, &completion, &losing_relations)
            .unwrap()
    );
    assert_eq!(store.relations(&key).unwrap(), relations_before);
}

/* I1(b): a won race removes every turn row from every superseded fence. */

#[test]
fn a_won_race_removes_turn_rows_from_every_superseded_fence_and_keeps_only_its_own() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "won-race-many-fences", 100, 60);
    let key = record.key.clone();

    // Two fences older than the winning one, left over from two earlier,
    // superseded passes.
    {
        let connection = store.lock();
        insert_turn_rows(
            &connection,
            &turn_session_key(&key),
            claim.claim_fence - 2,
            &[turn_row(0)],
        )
        .unwrap();
        insert_turn_rows(
            &connection,
            &turn_session_key(&key),
            claim.claim_fence - 1,
            &[turn_row(0)],
        )
        .unwrap();
    }
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer
        .write_turn_rows(&[turn_row(0), turn_row(1), turn_row(2)])
        .unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());

    assert!(
        store
            .publish_projections(&record, None, &completion, &[])
            .unwrap()
    );

    let connection = store.lock();
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), claim.claim_fence - 2).unwrap(),
        0,
        "the oldest superseded fence must be gone"
    );
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), claim.claim_fence - 1).unwrap(),
        0,
        "the nearer superseded fence must be gone"
    );
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), claim.claim_fence).unwrap(),
        3,
        "the winning fence's own rows must survive untouched"
    );
}
