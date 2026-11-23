use std::path::Path;

use antiburn_local::analysis::{TurnRow, TurnScope};

use super::*;
use crate::store::SessionRecord;

const CLAIM: i64 = 50;

fn key() -> SessionKey {
    SessionKey::new("native", "pi", "leftover")
}

fn store(path: &Path) -> Store {
    let store = Store::open(path).unwrap();
    let record = SessionRecord {
        key: key(),
        source_kind: "file".into(),
        source_label: "/synthetic/leftover.jsonl".into(),
        wsl_distro: None,
        title: None,
        title_source: None,
        cwd: None,
        surface: "cli".into(),
        updated_at_epoch: Some(100),
        activity_cursor: "a".into(),
        activity_source: "mtime".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: Some("a".into()),
    };
    store.upsert_sessions(&[record], &["pi"]).unwrap();
    store
}

fn set_fences(store: &Store, claim: i64, published: Option<i64>, status: &str) {
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET claim_fence = ?1, published_fence = ?2, status = ?3",
            params![claim, published, status],
        )
        .unwrap();
}

fn add_copy(store: &Store, fence: i64, turns: u64) {
    let rows = (0..turns).map(row).collect::<Vec<_>>();
    let key = key();
    let mut connection = store.lock();
    let transaction = connection.transaction().unwrap();
    antiburn_local::analysis::insert_turn_rows(
        &transaction,
        &crate::store::turn_session_key(&key),
        fence,
        &rows,
    )
    .unwrap();
    transaction
        .execute(
            "INSERT INTO turn_content (turn_rowid, part_index, kind, content, truncated)
             SELECT rowid, 0, 'assistant', CAST('synthetic' AS BLOB), 0
               FROM turn WHERE claim_fence = ?1",
            [fence],
        )
        .unwrap();
    transaction
        .execute(
            "INSERT INTO session_coverage VALUES (?1, ?2, ?3, ?4, '{}', 1)",
            params![key.environment_key, key.agent, key.session_id, fence],
        )
        .unwrap();
    transaction.commit().unwrap();
}

fn row(turn_index: u64) -> TurnRow {
    TurnRow {
        accounting: None,
        source_key: "s1".into(),
        thread_id: "s1".into(),
        turn_index,
        scope: TurnScope::Main,
        child_id: None,
        role: "assistant",
        ts_ms: Some(1_000 + turn_index as i64),
        model: Some("claude-opus-4-6".into()),
        provider: None,
        api: None,
        effort: None,
        speed: None,
        input_tokens: 10,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cache_write_1h_tokens: 0,
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

fn fences(store: &Store, table: &str) -> Vec<i64> {
    store
        .lock()
        .prepare(&format!(
            "SELECT DISTINCT claim_fence FROM {table} ORDER BY 1"
        ))
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn counts(store: &Store) -> (i64, i64) {
    let connection = store.lock();
    let count = |table| {
        connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    };
    (count("turn"), count("turn_content"))
}

#[test]
fn published_current_and_newer_rows_survive_and_older_copies_go() {
    for status in ["processing", "pending", "ready", "failed"] {
        let directory = tempfile::tempdir().unwrap();
        let store = store(directory.path());
        for fence in [10, 20, 30, CLAIM, 60] {
            add_copy(&store, fence, 3);
        }
        set_fences(&store, CLAIM, Some(20), status);
        assert_eq!(store.clear_leftover_rows().unwrap(), 6, "{status}");
        assert_eq!(fences(&store, "turn"), [20, CLAIM, 60], "{status}");
        assert_eq!(fences(&store, "session_coverage"), [20, CLAIM, 60]);
        assert_eq!(counts(&store), (9, 9));
        assert_eq!(store.clear_leftover_rows().unwrap(), 0);
    }
}

#[test]
fn a_session_that_never_published_keeps_only_its_current_claim() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    for fence in [10, 20, CLAIM] {
        add_copy(&store, fence, 250);
    }
    set_fences(&store, CLAIM, None, "processing");
    assert_eq!(store.clear_leftover_rows().unwrap(), 500);
    assert_eq!(fences(&store, "turn"), [CLAIM]);
}

#[test]
fn a_claim_that_equals_the_published_fence_keeps_its_rows() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    add_copy(&store, 10, 2);
    add_copy(&store, CLAIM, 2);
    set_fences(&store, CLAIM, Some(CLAIM), "ready");
    assert_eq!(store.clear_leftover_rows().unwrap(), 2);
    assert_eq!(fences(&store, "turn"), [CLAIM]);
}

#[test]
fn a_session_without_evidence_keeps_its_rows() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    add_copy(&store, 10, 2);
    store
        .lock()
        .execute("DELETE FROM session_evidence", [])
        .unwrap();
    assert_eq!(
        store.clear_leftover_batch(&key()).unwrap(),
        LeftoverBatch::default()
    );
    assert_eq!(counts(&store).0, 2);
}

#[test]
fn a_stop_midway_resumes_and_a_new_claim_between_batches_is_respected() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    add_copy(&store, 10, 250);
    add_copy(&store, 20, 250);
    set_fences(&store, 20, None, "processing");
    let first = store
        .clear_leftover_batch_within(&key(), Duration::ZERO)
        .unwrap();
    assert_eq!(
        first,
        LeftoverBatch {
            removed: CHUNK_ROWS,
            more: true
        }
    );
    drop(store);

    let store = Store::open(directory.path()).unwrap();
    set_fences(&store, CLAIM, Some(20), "processing");
    assert_eq!(store.clear_leftover_rows().unwrap(), 150);
    assert_eq!(fences(&store, "turn"), [20]);
    assert_eq!(counts(&store), (250, 250));
}

#[test]
fn the_row_search_seeks_the_fence_index() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    let plan: String = store
        .lock()
        .query_row(
            "EXPLAIN QUERY PLAN SELECT rowid FROM turn
              WHERE environment_key = 'a' AND agent = 'b' AND session_id = 'c'
                AND claim_fence > 1 AND claim_fence < 9 LIMIT 5",
            [],
            |row| row.get(3),
        )
        .unwrap();
    assert!(plan.contains("claim_fence>?"), "{plan}");
}
