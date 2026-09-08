use std::path::Path;

use rusqlite::params;

use super::*;
use crate::store::SessionRecord;

const AGENT: &str = "codex";

fn account(character: char) -> String {
    character.to_string().repeat(64)
}

fn memory_store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-rollout-checkpoint-test")).expect("opens store")
}

fn insert_session(store: &Store, session_id: &str, source_label: &str) -> SessionKey {
    let key = SessionKey::new("native", AGENT, session_id);
    store
        .upsert_sessions(
            &[SessionRecord {
                key: key.clone(),
                source_kind: "file".to_string(),
                source_label: source_label.to_string(),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: None,
                surface: "cli".to_string(),
                updated_at_epoch: Some(1),
                activity_cursor: "synthetic".to_string(),
                activity_source: "event".to_string(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some(format!("synthetic:{session_id}")),
            }],
            &[],
        )
        .expect("stores synthetic session");
    key
}

/// Publish one turn at `ts_ms`, so the candidate query's turn-activity
/// `EXISTS` clause matches it.
fn insert_turn(store: &Store, key: &SessionKey, ts_ms: i64) {
    let connection = store.lock();
    connection
        .execute(
            "INSERT OR IGNORE INTO session_evidence (
                 environment_key, agent, session_id, status, published_fence
             ) VALUES (?1, ?2, ?3, 'ready', 1)",
            params![key.environment_key, key.agent, key.session_id],
        )
        .expect("publishes synthetic evidence");
    connection
        .execute(
            "INSERT INTO turn (
                 environment_key, agent, session_id, claim_fence, source_key,
                 thread_id, turn_index, scope, role, ts_ms, model, effort, speed,
                 input_tokens, cache_read_tokens, cache_write_tokens, output_tokens,
                 is_compaction_boundary, message_id, uuid, parent_uuid
             ) VALUES (?1, ?2, ?3, 1, 'synthetic', 'synthetic', 0, 'main', 'assistant',
                       ?4, 'gpt-6-astra', NULL, NULL, 100, 0, 0, 20, 0, NULL, NULL, NULL)",
            params![key.environment_key, key.agent, key.session_id, ts_ms],
        )
        .expect("stores synthetic turn");
}

fn bind_account(store: &Store, key: &SessionKey, account_key: &str) {
    store
        .lock()
        .execute(
            "INSERT INTO session_provider_account (
                 environment_key, agent, session_id, provider, account_key,
                 provenance, confidence, first_seen_at
             ) VALUES (?1, ?2, ?3, 'openai', ?4, 'provider_live', 'direct', '2026-01-01T00:00:00Z')",
            params![key.environment_key, key.agent, key.session_id, account_key],
        )
        .expect("binds session account");
}

fn observe_account(store: &Store, account_key: &str) {
    store
        .lock()
        .execute(
            "INSERT INTO provider_account_seen (agent, provider, account_key,
                 first_seen_epoch, last_seen_epoch)
             VALUES (?1, 'openai', ?2, 1, 1)",
            params![AGENT, account_key],
        )
        .expect("records a seen account");
}

#[test]
fn an_unbound_session_with_one_known_account_is_a_candidate() {
    let store = memory_store();
    let key = insert_session(&store, "session", "/home/avery/.codex/sessions/a.jsonl");
    insert_turn(&store, &key, 150_000);
    observe_account(&store, &account('a'));

    let candidates = store
        .provider_usage_rollout_candidates(0, 1_000, 9)
        .expect("query succeeds");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].key, key);
    assert_eq!(candidates[0].account_key, account('a'));
}

#[test]
fn an_unbound_session_with_two_known_accounts_is_not_a_candidate() {
    let store = memory_store();
    let key = insert_session(&store, "session", "/home/avery/.codex/sessions/a.jsonl");
    insert_turn(&store, &key, 150_000);
    observe_account(&store, &account('a'));
    observe_account(&store, &account('b'));

    let candidates = store
        .provider_usage_rollout_candidates(0, 1_000, 9)
        .expect("query succeeds");
    assert!(
        candidates.is_empty(),
        "an ambiguous account set resolves to no candidate"
    );
}

#[test]
fn a_directly_bound_session_uses_its_own_account_over_the_fallback() {
    let store = memory_store();
    let key = insert_session(&store, "session", "/home/avery/.codex/sessions/a.jsonl");
    insert_turn(&store, &key, 150_000);
    bind_account(&store, &key, &account('a'));
    observe_account(&store, &account('b'));

    let candidates = store
        .provider_usage_rollout_candidates(0, 1_000, 9)
        .expect("query succeeds");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].account_key, account('a'));
}

#[test]
fn a_session_with_no_recent_turn_activity_is_not_a_candidate() {
    let store = memory_store();
    let key = insert_session(&store, "session", "/home/avery/.codex/sessions/a.jsonl");
    insert_turn(&store, &key, 50_000);
    observe_account(&store, &account('a'));

    let candidates = store
        .provider_usage_rollout_candidates(60, 1_000, 9)
        .expect("query succeeds");
    assert!(
        candidates.is_empty(),
        "activity older than the cutoff is not a candidate"
    );
}

#[test]
fn a_completed_checkpoint_reports_its_saved_progress() {
    let store = memory_store();
    let key = insert_session(&store, "session", "/home/avery/.codex/sessions/a.jsonl");
    insert_turn(&store, &key, 150_000);
    observe_account(&store, &account('a'));
    store
        .upsert_rollout_checkpoint(
            &key,
            "/home/avery/.codex/sessions/a.jsonl",
            &RolloutCheckpoint {
                cursor_bytes: 4_096,
                source_bytes: 4_096,
                source_modified_epoch: Some(10),
                source_identity: "17:23".to_string(),
                complete: true,
            },
            500,
        )
        .expect("stores checkpoint");

    let candidates = store
        .provider_usage_rollout_candidates(0, 1_000, 9)
        .expect("query succeeds");
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].complete);
    assert_eq!(candidates[0].cursor_bytes, 4_096);
    assert_eq!(candidates[0].source_identity, "17:23");
}

#[test]
fn an_incomplete_checkpoint_sorts_before_a_complete_one() {
    let store = memory_store();
    let done_key = insert_session(&store, "done", "/home/avery/.codex/sessions/done.jsonl");
    let pending_key = insert_session(
        &store,
        "pending",
        "/home/avery/.codex/sessions/pending.jsonl",
    );
    insert_turn(&store, &done_key, 150_000);
    insert_turn(&store, &pending_key, 150_000);
    observe_account(&store, &account('a'));
    store
        .upsert_rollout_checkpoint(
            &done_key,
            "/home/avery/.codex/sessions/done.jsonl",
            &RolloutCheckpoint {
                cursor_bytes: 10,
                source_bytes: 10,
                source_modified_epoch: Some(1),
                source_identity: "1:1".to_string(),
                complete: true,
            },
            100,
        )
        .expect("stores completed checkpoint");

    let candidates = store
        .provider_usage_rollout_candidates(0, 1_000, 9)
        .expect("query succeeds");
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].key, pending_key, "pending work comes first");
    assert_eq!(candidates[1].key, done_key);
}

#[test]
fn deferring_a_checkpoint_excludes_it_until_its_retry_time() {
    let store = memory_store();
    let key = insert_session(&store, "session", "/home/avery/.codex/sessions/a.jsonl");
    insert_turn(&store, &key, 150_000);
    observe_account(&store, &account('a'));

    let retry_at = store
        .defer_rollout_checkpoint(&key, "/home/avery/.codex/sessions/a.jsonl", 0, 1_000)
        .expect("defers");
    assert_eq!(retry_at, 1_060);

    let candidates = store
        .provider_usage_rollout_candidates(0, 1_000, 9)
        .expect("query succeeds");
    assert!(
        candidates.is_empty(),
        "a deferred source stays out until its retry time"
    );

    let candidates = store
        .provider_usage_rollout_candidates(0, retry_at, 9)
        .expect("query succeeds");
    assert_eq!(
        candidates.len(),
        1,
        "the source is ready once its time comes"
    );
}

#[test]
fn deferring_a_checkpoint_a_second_time_backs_off_further() {
    let store = memory_store();
    let key = insert_session(&store, "session", "/home/avery/.codex/sessions/a.jsonl");

    let first_retry = store
        .defer_rollout_checkpoint(&key, "/home/avery/.codex/sessions/a.jsonl", 0, 1_000)
        .expect("defers once");
    let second_retry = store
        .defer_rollout_checkpoint(&key, "/home/avery/.codex/sessions/a.jsonl", 0, first_retry)
        .expect("defers twice");
    assert!(
        second_retry - first_retry > first_retry - 1_000,
        "the second backoff is longer than the first"
    );
}

#[test]
fn touching_a_complete_checkpoint_updates_its_timestamp_without_reopening_it() {
    let store = memory_store();
    let key = insert_session(&store, "session", "/home/avery/.codex/sessions/a.jsonl");
    store
        .upsert_rollout_checkpoint(
            &key,
            "/home/avery/.codex/sessions/a.jsonl",
            &RolloutCheckpoint {
                cursor_bytes: 10,
                source_bytes: 10,
                source_modified_epoch: Some(1),
                source_identity: "1:1".to_string(),
                complete: true,
            },
            100,
        )
        .expect("stores checkpoint");

    store
        .touch_rollout_checkpoint(&key, 200)
        .expect("touches checkpoint");

    let updated_at: i64 = store
        .lock()
        .query_row(
            "SELECT updated_at_epoch FROM provider_usage_rollout_checkpoint
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
            |row| row.get(0),
        )
        .expect("reads updated_at_epoch");
    assert_eq!(updated_at, 200);
}

#[test]
fn deleting_a_session_removes_its_checkpoint() {
    let store = memory_store();
    let key = insert_session(&store, "session", "/home/avery/.codex/sessions/a.jsonl");
    store
        .upsert_rollout_checkpoint(
            &key,
            "/home/avery/.codex/sessions/a.jsonl",
            &RolloutCheckpoint {
                cursor_bytes: 10,
                source_bytes: 10,
                source_modified_epoch: Some(1),
                source_identity: "1:1".to_string(),
                complete: true,
            },
            100,
        )
        .expect("stores checkpoint");

    assert!(store.delete_session(&key).expect("delete session"));

    let remaining: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM provider_usage_rollout_checkpoint",
            [],
            |row| row.get(0),
        )
        .expect("counts checkpoints");
    assert_eq!(
        remaining, 0,
        "the checkpoint's session foreign key cascades the delete"
    );
}
