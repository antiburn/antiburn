use std::path::Path;

use antiburn_local::analysis::{TurnRow, TurnRowStore, TurnScope};

use super::*;
use crate::store::{
    AnalysisRecord, EvidenceCompletion, FencedTurnRowStore, PublishedEvidence, SessionRecord,
};

fn store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-fork-lineage-test")).unwrap()
}

fn session_record(session_id: &str, fingerprint: &str) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", AgentKind::Claude.slug(), session_id),
        source_kind: "file".into(),
        source_label: format!("/tmp/{session_id}.jsonl"),
        wsl_distro: None,
        title: None,
        title_source: None,
        cwd: None,
        surface: "cli".into(),
        updated_at_epoch: Some(1_000),
        activity_cursor: String::new(),
        activity_source: "event".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: Some(fingerprint.into()),
    }
}

fn turn_row(turn_index: u64, uuid: Option<&str>) -> TurnRow {
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
        uuid: uuid.map(str::to_string),
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

/// Publish `row_count` turn rows for `session_id`, the first one carrying
/// `uuid`, under a fresh claim. Safe to call more than once for the same
/// session: each call replaces its published row set with a new one of
/// `row_count` rows, which is how [`Store::publish_projections`] treats an
/// unnamed source with no resume snapshot.
fn publish_turns(store: &Store, session_id: &str, uuid: &str, row_count: u64) -> SessionKey {
    let fingerprint = format!("sv{row_count}:{session_id}");
    store
        .upsert_sessions(
            &[session_record(session_id, &fingerprint)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let claim = store
        .claim_next_evidence(&crate::agents::evidence_cohort(), 1_000, 60)
        .unwrap()
        .unwrap();
    let rows: Vec<TurnRow> = (0..row_count)
        .map(|index| turn_row(index, if index == 0 { Some(uuid) } else { None }))
        .collect();
    FencedTurnRowStore::new(store.clone(), claim.key.clone(), claim.claim_fence)
        .write_turn_rows(&rows)
        .unwrap();
    let record = AnalysisRecord {
        key: claim.key.clone(),
        model_breakdown_json: "{}".into(),
        inclusive_models_json: "[]".into(),
        initial_context_json: None,
        source_summaries_json: None,
        provider_hints_json: None,
        source_fingerprint: fingerprint,
        pricing_generation: 1,
        analyzed_generation: claim.source_generation,
        parser_revision: 1,
        analyzer_revision: 1,
        metrics_schema_revision: 1,
    };
    let completion = EvidenceCompletion {
        claim_fence: claim.claim_fence,
        status: PublishedEvidence::Ready,
        evidence_schema_revision: 1,
        evidence_json: "{}".into(),
    };
    assert!(
        store
            .publish_projections(&record, None, &completion, &[], &[])
            .unwrap()
    );
    claim.key
}

fn publish_first_turn(store: &Store, session_id: &str, uuid: &str) -> SessionKey {
    publish_turns(store, session_id, uuid, 1)
}

/* --------------------------------------------------------------------
 * rank
 * ----------------------------------------------------------------- */

fn owning(session_id: &str, first_seen_at: &str, published_turn_rows: i64) -> OwningSession {
    OwningSession {
        session_id: session_id.into(),
        first_seen_at: first_seen_at.into(),
        published_turn_rows,
    }
}

#[test]
fn rank_orders_by_first_seen_at_first() {
    let earlier = owning("a", "2024-01-01T00:00:00Z", 50);
    let later = owning("b", "2024-01-02T00:00:00Z", 1);
    assert_eq!(rank(&earlier, &later), Ordering::Less);
    assert_eq!(rank(&later, &earlier), Ordering::Greater);
}

#[test]
fn rank_breaks_a_first_seen_at_tie_on_row_count() {
    let shorter = owning("a", "2024-01-01T00:00:00Z", 2);
    let longer = owning("b", "2024-01-01T00:00:00Z", 9);
    assert_eq!(rank(&shorter, &longer), Ordering::Less);
    assert_eq!(rank(&longer, &shorter), Ordering::Greater);
}

#[test]
fn rank_breaks_a_full_tie_on_session_id() {
    let a = owning("a", "2024-01-01T00:00:00Z", 2);
    let b = owning("b", "2024-01-01T00:00:00Z", 2);
    assert_eq!(rank(&a, &b), Ordering::Less);
    assert_eq!(rank(&b, &a), Ordering::Greater);
}

/* --------------------------------------------------------------------
 * link_claude_fork
 * ----------------------------------------------------------------- */

#[test]
fn a_fork_published_after_its_parent_links_to_the_parent() {
    let store = store();
    let parent_key = publish_first_turn(&store, "parent", "u1");
    store
        .set_first_seen_at_for_test(&parent_key, "2024-01-01T00:00:00Z")
        .unwrap();
    let fork_key = publish_first_turn(&store, "fork", "u1");
    store
        .set_first_seen_at_for_test(&fork_key, "2024-01-02T00:00:00Z")
        .unwrap();

    link_claude_fork(&store, &fork_key).unwrap();

    assert_eq!(
        store.fork_parent(&fork_key).unwrap().as_deref(),
        Some("parent")
    );
    assert_eq!(store.fork_parent(&parent_key).unwrap(), None);
}

#[test]
fn a_parent_published_after_its_fork_still_links_with_no_reverse_relation() {
    let store = store();
    let fork_key = publish_first_turn(&store, "fork", "u1");
    store
        .set_first_seen_at_for_test(&fork_key, "2024-01-02T00:00:00Z")
        .unwrap();
    // The fork ran the lookup first and found no candidate yet.
    link_claude_fork(&store, &fork_key).unwrap();
    assert_eq!(store.fork_parent(&fork_key).unwrap(), None);

    let parent_key = publish_first_turn(&store, "parent", "u1");
    store
        .set_first_seen_at_for_test(&parent_key, "2024-01-01T00:00:00Z")
        .unwrap();

    link_claude_fork(&store, &parent_key).unwrap();

    assert_eq!(
        store.fork_parent(&fork_key).unwrap().as_deref(),
        Some("parent")
    );
    assert_eq!(store.fork_parent(&parent_key).unwrap(), None);
}

#[test]
fn equal_first_seen_at_breaks_the_tie_on_published_row_count() {
    let store = store();
    let short_key = publish_turns(&store, "short", "u1", 1);
    let long_key = publish_turns(&store, "long", "u1", 3);
    store
        .set_first_seen_at_for_test(&short_key, "2024-01-01T00:00:00Z")
        .unwrap();
    store
        .set_first_seen_at_for_test(&long_key, "2024-01-01T00:00:00Z")
        .unwrap();

    link_claude_fork(&store, &long_key).unwrap();

    assert_eq!(
        store.fork_parent(&long_key).unwrap().as_deref(),
        Some("short")
    );
    assert_eq!(store.fork_parent(&short_key).unwrap(), None);
}

#[test]
fn a_candidate_that_already_calls_this_session_its_parent_is_never_claimed_back() {
    let store = store();
    // x and y share a first uuid, and initially rank x before y, so linking
    // y first names x as y's parent.
    let x_key = publish_turns(&store, "x", "u1", 2);
    let y_key = publish_turns(&store, "y", "u1", 2);
    store
        .set_first_seen_at_for_test(&x_key, "2024-01-01T00:00:00Z")
        .unwrap();
    store
        .set_first_seen_at_for_test(&y_key, "2024-01-01T00:00:00Z")
        .unwrap();
    link_claude_fork(&store, &y_key).unwrap();
    assert_eq!(store.fork_parent(&y_key).unwrap().as_deref(), Some("x"));

    // x grows well past y's row count. With equal first_seen_at, rank now
    // favors y — but y already calls x its parent, so x must not claim y
    // back as its own parent.
    let x_key = publish_turns(&store, "x", "u1", 9);
    store
        .set_first_seen_at_for_test(&x_key, "2024-01-01T00:00:00Z")
        .unwrap();

    link_claude_fork(&store, &x_key).unwrap();

    assert_eq!(store.fork_parent(&x_key).unwrap(), None);
    assert_eq!(store.fork_parent(&y_key).unwrap().as_deref(), Some("x"));
}

#[test]
fn a_session_whose_first_uuid_nobody_else_owns_gets_no_relation() {
    let store = store();
    let key = publish_first_turn(&store, "solo", "u1");

    link_claude_fork(&store, &key).unwrap();

    assert_eq!(store.fork_parent(&key).unwrap(), None);
}

#[test]
fn a_second_call_is_idempotent() {
    let store = store();
    let parent_key = publish_first_turn(&store, "parent", "u1");
    store
        .set_first_seen_at_for_test(&parent_key, "2024-01-01T00:00:00Z")
        .unwrap();
    let fork_key = publish_first_turn(&store, "fork", "u1");
    store
        .set_first_seen_at_for_test(&fork_key, "2024-01-02T00:00:00Z")
        .unwrap();

    link_claude_fork(&store, &fork_key).unwrap();
    link_claude_fork(&store, &fork_key).unwrap();

    assert_eq!(
        store.fork_parent(&fork_key).unwrap().as_deref(),
        Some("parent")
    );
}

#[test]
fn a_non_claude_key_is_ignored() {
    let store = store();
    let key = SessionKey::new("native", "codex", "solo");
    // No session row exists at all for this key — a real call would still
    // return early on the agent guard before it ever queries the store.
    link_claude_fork(&store, &key).unwrap();
    assert_eq!(store.fork_parent(&key).unwrap(), None);
}
