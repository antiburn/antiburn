use std::path::Path;

use super::*;
use crate::store::{SessionKey, SessionRecord, Store};

fn store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-session-evidence-test"))
        .expect("opens evidence store")
}

fn insert_session(store: &Store, id: &str, messages: &[(&str, &str)]) -> Vec<i64> {
    store
        .upsert_sessions(
            &[SessionRecord {
                key: SessionKey::new("native", "codex", id),
                source_kind: "file".into(),
                source_label: format!("/synthetic/{id}.jsonl"),
                wsl_distro: None,
                title: Some("Evidence fixture".into()),
                title_source: Some("vendor".into()),
                cwd: Some(format!("/synthetic/{id}")),
                surface: "cli".into(),
                updated_at_epoch: Some(1_000),
                activity_cursor: "1:1".into(),
                activity_source: "event".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: None,
            }],
            &["codex"],
        )
        .expect("stores session");
    let connection = store.lock();
    connection
        .execute(
            "UPDATE session_evidence
                SET status = 'ready', analyzed_generation = 0, published_fence = 1,
                    claim_fence = 1
              WHERE environment_key = 'native' AND agent = 'codex' AND session_id = ?1",
            [id],
        )
        .expect("publishes fixture fence");
    messages
        .iter()
        .enumerate()
        .map(|(index, (scope, text))| {
            connection
                .execute(
                    "INSERT INTO turn (
                         environment_key, agent, session_id, claim_fence, source_key,
                         thread_id, turn_index, scope, role, input_tokens,
                         cache_read_tokens, cache_write_tokens, output_tokens,
                         is_compaction_boundary, last_tool, ts_ms
                     ) VALUES (
                         'native', 'codex', ?1, 1, 'source', ?2, ?3, ?2, 'assistant',
                         0, 0, 0, 0, 0, NULL, ?4
                     )",
                    rusqlite::params![
                        id,
                        scope,
                        i64::try_from(index).unwrap(),
                        1_000_i64 + i64::try_from(index).unwrap(),
                    ],
                )
                .expect("stores turn");
            let rowid = connection.last_insert_rowid();
            connection
                .execute(
                    "INSERT INTO turn_content (turn_rowid, part_index, kind, content, truncated)
                     VALUES (?1, 0, 'assistant', ?2, 0)",
                    rusqlite::params![rowid, text.as_bytes()],
                )
                .expect("stores retained content");
            rowid
        })
        .collect()
}

fn stored_reference(id: &str, rowid: i64, scope: &str, turn_index: i64) -> StoredEvidenceReference {
    StoredEvidenceReference {
        environment_key: "native".into(),
        agent: "codex".into(),
        session_id: id.into(),
        source_generation: 0,
        published_fence: 1,
        source_key: "source".into(),
        thread_id: scope.into(),
        scope: scope.into(),
        turn_rowid: rowid,
        turn_index,
        part_index: 0,
    }
}

#[test]
fn stale_generation_does_not_resolve_to_retained_content() {
    let store = store();
    let rows = insert_session(&store, "stale", &[("main", "stable passage")]);
    let reference = stored_reference("stale", rows[0], "main", 0);
    assert!(
        store
            .fetch_published_session_context(&reference)
            .unwrap()
            .is_some()
    );
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE session_id = 'stale'",
            [],
        )
        .unwrap();
    assert!(
        store
            .fetch_published_session_context(&reference)
            .unwrap()
            .is_none()
    );
}

#[test]
fn adjacent_context_stays_inside_the_source_thread_and_scope() {
    let store = store();
    let rows = insert_session(
        &store,
        "scope",
        &[
            ("main", "before"),
            ("delegated", "matched"),
            ("main", "after"),
        ],
    );
    let reference = stored_reference("scope", rows[1], "delegated", 1);
    let context = store
        .fetch_published_session_context(&reference)
        .unwrap()
        .unwrap();
    assert_eq!(context.matched.content, "matched");
    assert!(context.previous.is_none());
    assert!(context.next.is_none());
}

#[test]
fn json_path_context_uses_unicode_scalar_offsets_and_rejects_invalid_bounds() {
    let stored = stored_reference("json", 7, "main", 0);
    let bytes = r#"{"text":"🦊 orchard"}"#.as_bytes();
    let mut reference = evidence_reference(stored.clone(), bytes);
    reference.json_path = Some("/text".into());
    reference.match_start = Some(2);
    reference.match_end = Some(9);
    let row = RetainedContentRow {
        turn_rowid: 7,
        source_key: "source".into(),
        thread_id: "main".into(),
        turn_index: 0,
        scope: "main".into(),
        part_index: 0,
        kind: "tool_result".into(),
        content: String::from_utf8(bytes.to_vec()).unwrap(),
        reference_bytes: bytes.to_vec(),
        truncated: false,
    };
    let item = context_item(&reference, &row).unwrap();
    assert_eq!(item.text, "🦊 orchard");
    assert_eq!(validate_reference(&reference), Some(stored));

    reference.match_end = Some(100);
    assert!(context_item(&reference, &row).is_none());
    reference.key.clear();
    assert!(validate_reference(&reference).is_none());
}
