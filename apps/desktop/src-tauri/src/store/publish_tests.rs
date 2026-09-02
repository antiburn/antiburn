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
        initial_context_json: None,
        source_summaries_json: None,
        provider_hints_json: None,
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
        has_thinking: false,
        last_tool: None,
        subagent_launches: 0,
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
        .publish_projections(&record, None, &completion, &[], &[])
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
            .publish_projections(&record, None, &completion, &losing_relations, &[])
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
            .publish_projections(&record, None, &completion, &[], &[])
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

/* R1: `Store::published_turn_rows` only ever exposes `published_fence`'s own
 * rows — never a superseded fence, and never a claim in flight's partial
 * rows, even while that claim moves `status` and `claim_fence` out from
 * under it. */

fn revisions() -> ProjectionRevisions {
    ProjectionRevisions {
        parser_revision: 1,
        analyzer_revision: 1,
        metrics_schema_revision: 1,
        evidence_schema_revision: 1,
    }
}

#[test]
fn published_turn_rows_returns_the_winning_pass_rows_in_order() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "published-rows-order", 100, 60);
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    let written = [turn_row(2), turn_row(0), turn_row(1)];
    writer.write_turn_rows(&written).unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );

    let rows = store.published_turn_rows(&key).unwrap().expect("ready");
    assert_eq!(
        rows,
        vec![turn_row(0), turn_row(1), turn_row(2)],
        "rows must come back sorted by (source_key, turn_index), not insertion order"
    );
}

/// The whole point of `published_fence`: an actively-written session is
/// reclaimed and requeued constantly, so its evidence `status` is almost
/// never terminal at drilldown-open. Before `published_fence`,
/// `published_turn_rows` gated on `status`, so this in-flight claim made it
/// return `None` — the "Analyzing this session…" screen for minutes, even
/// though the last publish's rows were still sitting right there. Now it
/// keeps serving them: only the *next* winning publish moves
/// `published_fence` (and only then does it delete this fence's rows).
#[test]
fn published_turn_rows_serves_the_last_published_fence_while_a_newer_claim_is_in_flight() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "claim-in-flight", 100, 60);
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0), turn_row(1)]).unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );
    let published = store.published_turn_rows(&key).unwrap();
    assert!(published.is_some());

    // A newer pass claims the session: status flips to `processing` and the
    // fence bumps, while `published_fence` still names the earlier, winning
    // fence — only a publish moves it. This pass then writes some, but not
    // all, of its own rows before this check runs.
    mark_evidence_pending_in(&store.lock(), &key).unwrap();
    let next_claim = store
        .claim_next_evidence(&["claude-code"], 200, 60)
        .unwrap()
        .expect("reclaimable");
    assert_eq!(next_claim.claim_fence, claim.claim_fence + 1);
    let next_writer = FencedTurnRowStore::new(store.clone(), key.clone(), next_claim.claim_fence);
    next_writer.write_turn_rows(&[turn_row(0)]).unwrap();

    assert_eq!(
        store.published_turn_rows(&key).unwrap(),
        published,
        "an in-flight claim's partial rows must never leak into a published_fence read"
    );
}

#[test]
fn published_turn_rows_is_none_with_no_evidence_row() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "never-claimed");
    assert_eq!(store.published_turn_rows(&key).unwrap(), None);
}

#[test]
fn published_turn_rows_is_none_while_pending() {
    let store = store();
    let mut record = session("pending-status", 1_000);
    record.source_fingerprint = Some("sv1:pending-status".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert_eq!(store.published_turn_rows(&record.key).unwrap(), None);
}

#[test]
fn published_turn_rows_is_none_when_failed() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "failed-status", 100, 60);
    let key = record.key.clone();
    assert!(
        store
            .fail_evidence(
                &claim,
                EvidenceFailure::Failed {
                    revisions: revisions(),
                },
                "source-unreadable",
            )
            .unwrap()
    );
    assert_eq!(store.published_turn_rows(&key).unwrap(), None);
}

/// For a terminal row (`ready` or `unsupported`), `published_fence` always
/// equals `claim_fence`: a winning publish stamps both from the same
/// completion in the same statement. So reading rows by `published_fence`
/// alone serves exactly the rows the old status-gated read served for these
/// two statuses — including `unsupported`, which is an insights verdict (no
/// detector was eligible for this source), not a parse-quality one: the
/// rows and the `session_analysis` record a winning pass wrote are still
/// complete for it, same as `ready`.
#[test]
fn published_fence_equals_claim_fence_for_every_terminal_status() {
    let store = store();
    for status in [PublishedEvidence::Ready, PublishedEvidence::Unsupported] {
        let session_id = format!("terminal-equivalence-{}", status.as_str());
        let (record, claim) = claimed_projection(&store, &session_id, 100, 60);
        let key = record.key.clone();
        let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
        writer.write_turn_rows(&[turn_row(0), turn_row(1)]).unwrap();
        let completion = evidence_completion(&claim, status, "{}".into());
        assert!(
            store
                .publish_projections(&record, None, &completion, &[], &[])
                .unwrap(),
            "a {} completion still publishes",
            status.as_str()
        );

        let evidence = store.evidence(&key).unwrap().unwrap();
        assert_eq!(
            evidence.published_fence,
            Some(evidence.claim_fence),
            "a winning publish stamps published_fence and claim_fence together"
        );
        assert_eq!(
            store.published_turn_rows(&key).unwrap(),
            Some(vec![turn_row(0), turn_row(1)]),
            "status `{}` names a complete row projection, same as `ready`",
            status.as_str()
        );
    }
}

#[test]
fn a_second_publish_supersedes_the_first_in_published_turn_rows() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "second-publish-supersedes", 100, 60);
    let key = record.key.clone();
    let first_writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    first_writer.write_turn_rows(&[turn_row(0)]).unwrap();
    let first_completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&record, None, &first_completion, &[], &[])
            .unwrap()
    );
    assert_eq!(
        store.published_turn_rows(&key).unwrap().expect("ready"),
        vec![turn_row(0)]
    );

    mark_evidence_pending_in(&store.lock(), &key).unwrap();
    let next_claim = store
        .claim_next_evidence(&["claude-code"], 200, 60)
        .unwrap()
        .expect("reclaimable");
    let next_record = projection_record(
        key.clone(),
        "sv1:second-publish-supersedes",
        next_claim.source_generation,
    );
    let next_writer = FencedTurnRowStore::new(store.clone(), key.clone(), next_claim.claim_fence);
    next_writer
        .write_turn_rows(&[turn_row(0), turn_row(1)])
        .unwrap();
    let next_completion = evidence_completion(&next_claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&next_record, None, &next_completion, &[], &[])
            .unwrap()
    );

    assert_eq!(
        store.published_turn_rows(&key).unwrap().expect("ready"),
        vec![turn_row(0), turn_row(1)],
        "only the new pass's rows must come back, none of the superseded fence's"
    );
}

/* R6: `published_fence` itself — a winning publish stamps it, a lost race
 * and every other evidence transition leave it exactly where it was. */

#[test]
fn publish_projections_stamps_published_fence_with_the_completion_claim_fence() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "publish-stamps-fence", 100, 60);
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0)]).unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );

    assert_eq!(
        store.evidence(&key).unwrap().unwrap().published_fence,
        Some(claim.claim_fence)
    );
}

#[test]
fn a_lost_race_does_not_stamp_published_fence() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "lost-race-no-fence-stamp", 100, 60);
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0)]).unwrap();
    advance_source_generation_past(&store, &key);
    let completion =
        evidence_completion(&claim, PublishedEvidence::Ready, "{\"lost\":true}".into());

    assert!(
        !store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );

    assert_eq!(store.evidence(&key).unwrap().unwrap().published_fence, None);
}

#[test]
fn a_requeue_and_a_reclaim_leave_published_fence_intact() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "requeue-claim-preserve-fence", 100, 60);
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0)]).unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );
    let published_fence = store.evidence(&key).unwrap().unwrap().published_fence;
    assert_eq!(published_fence, Some(claim.claim_fence));

    // A fingerprint change requeues the session: status flips back to
    // `pending`. Only the next winning publish may move `published_fence`.
    mark_evidence_pending_in(&store.lock(), &key).unwrap();
    assert_eq!(
        store.evidence(&key).unwrap().unwrap().published_fence,
        published_fence
    );

    // The worker reclaims it: status flips to `processing` and `claim_fence`
    // bumps, while `published_fence` still names the earlier, winning fence.
    let next_claim = store
        .claim_next_evidence(&["claude-code"], 200, 60)
        .unwrap()
        .expect("reclaimable");
    assert_eq!(next_claim.claim_fence, claim.claim_fence + 1);
    assert_eq!(
        store.evidence(&key).unwrap().unwrap().published_fence,
        published_fence
    );
}

#[test]
fn fail_evidence_leaves_published_fence_intact() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "fail-preserve-fence", 100, 60);
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0)]).unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );
    let published_fence = store.evidence(&key).unwrap().unwrap().published_fence;

    mark_evidence_pending_in(&store.lock(), &key).unwrap();
    let next_claim = store
        .claim_next_evidence(&["claude-code"], 200, 60)
        .unwrap()
        .expect("reclaimable");
    assert!(
        store
            .fail_evidence(
                &next_claim,
                EvidenceFailure::Retry {
                    next_attempt_at_epoch: 300,
                },
                "transient",
            )
            .unwrap()
    );

    assert_eq!(
        store.evidence(&key).unwrap().unwrap().published_fence,
        published_fence
    );
}

/// The published-analysis carve-out this change exists for: a session
/// requeued after a publish (an actively-written transcript, fingerprinted
/// again) still serves its last published rows, not `None`, while the fresh
/// pass is only queued and has not run yet.
#[test]
fn published_turn_rows_serves_the_last_published_fence_after_a_requeue() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "requeue-after-publish", 100, 60);
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0), turn_row(1)]).unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );

    mark_evidence_pending_in(&store.lock(), &key).unwrap();
    assert_eq!(
        store.evidence(&key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );

    assert_eq!(
        store.published_turn_rows(&key).unwrap(),
        Some(vec![turn_row(0), turn_row(1)]),
        "a requeue must not hide the last published pass's rows"
    );
}
