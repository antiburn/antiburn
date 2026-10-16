use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::store::SessionRecord;

const NOW: i64 = 1_000;

async fn fixture(warm: bool) -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"session\",\"version\":3,\"id\":\"file-session\",\"cwd\":\"/synthetic\"}\n",
    )
    .unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store
        .upsert_sessions(
            &[SessionRecord {
                key: SessionKey::new("native", "pi", "file-session"),
                source_kind: "file".into(),
                source_label: path.to_string_lossy().into_owned(),
                cwd: Some("/synthetic".into()),
                wsl_distro: None,
                title: None,
                title_source: None,
                surface: "cli".into(),
                updated_at_epoch: Some(NOW),
                activity_cursor: String::new(),
                activity_source: "event".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some("synthetic".into()),
            }],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store.lock().execute(
        "INSERT INTO burn_check_activation (check_id, activation_scope_key, definition_revision,
             enabled, generation, bootstrap_sequence, applied_sequence, updated_at_epoch)
         VALUES ('file-test', ?1, 'synthetic', 1, 1, 0, 0, ?2)",
        rusqlite::params![serde_json::json!({"environment":"native", "session_cwd":"/synthetic", "run":"Live"}).to_string(), NOW],
    ).unwrap();
    if warm {
        assert!(process_next(&store, &|| NOW, &|_| {}).await.unwrap());
    }
    (directory, store)
}

#[derive(Debug)]
struct Receipt {
    cursor: Option<String>,
    retry_count: i64,
    blocked: bool,
    pending: bool,
    next_attempt: Option<i64>,
    last_error: Option<String>,
    leased: bool,
}

fn receipt(store: &Store) -> Receipt {
    store.lock().query_row(
        "SELECT cursor_json, retry_count, blocked, pending, next_attempt_at_epoch, last_error,
             claim_fence IS NOT NULL OR lease_expires_at_epoch IS NOT NULL FROM native_file_delivery",
        [], |row| Ok(Receipt {
            cursor: row.get(0)?, retry_count: row.get(1)?, blocked: row.get(2)?,
            pending: row.get(3)?, next_attempt: row.get(4)?, last_error: row.get(5)?, leased: row.get(6)?,
        }),
    ).unwrap()
}

#[tokio::test]
async fn typed_transient_errors_retry_with_or_without_a_cursor_and_recover() {
    for warm in [false, true] {
        for (failure, label) in [
            (NativeFileReadError::Unavailable, "file-unavailable"),
            (NativeFileReadError::ReadFailed, "file-read-failed"),
        ] {
            let (_directory, store) = fixture(warm).await;
            let before = store.native_tool_result_publications_after(0, 100).unwrap();
            let cursor = warm.then(|| receipt(&store).cursor).flatten();
            let announced = AtomicUsize::new(0);
            assert!(
                process_next_with(
                    &store,
                    &|| NOW + 1,
                    &|_| {
                        announced.fetch_add(1, Ordering::SeqCst);
                    },
                    move |_, _| Err(failure.into())
                )
                .await
                .unwrap()
            );
            let failed = receipt(&store);
            let retry_at = NOW + 1 + super::super::backoff_secs(0);
            assert_eq!(failed.cursor, cursor);
            assert_eq!(failed.retry_count, 1);
            assert!(
                !failed.blocked && failed.pending && !failed.leased,
                "{failed:?}"
            );
            assert_eq!(failed.next_attempt, Some(retry_at));
            assert_eq!(failed.last_error.as_deref(), Some(label));
            assert_eq!(announced.load(Ordering::SeqCst), 1);
            assert_eq!(
                store.native_tool_result_publications_after(0, 100).unwrap(),
                before
            );
            assert!(
                !process_next(&store, &|| retry_at - 1, &|_| {})
                    .await
                    .unwrap()
            );
            assert!(process_next(&store, &|| retry_at, &|_| {}).await.unwrap());
            let recovered = receipt(&store);
            assert!(recovered.cursor.is_some());
            assert_eq!(recovered.retry_count, 0);
            assert_eq!(recovered.last_error, None);
            assert!(!recovered.blocked && !recovered.pending && !recovered.leased);
        }
    }
}

#[tokio::test]
async fn typed_transient_errors_stop_after_five_attempts_without_publishing() {
    for failure in [
        NativeFileReadError::Unavailable,
        NativeFileReadError::ReadFailed,
    ] {
        let (_directory, store) = fixture(true).await;
        let before = store.native_tool_result_publications_after(0, 100).unwrap();
        let mut now = NOW + 1;
        for attempt in 1..=5 {
            assert!(
                process_next_with(&store, &|| now, &|_| {}, move |_, _| {
                    Err(failure.into())
                })
                .await
                .unwrap()
            );
            let failed = receipt(&store);
            assert_eq!(failed.retry_count, attempt);
            assert!(!failed.blocked && !failed.leased);
            assert_eq!(failed.pending, attempt < 5);
            if attempt < 5 {
                now = failed.next_attempt.unwrap();
            } else {
                assert_eq!(failed.next_attempt, None);
            }
        }
        assert!(
            !process_next(&store, &|| now + 10_000, &|_| {})
                .await
                .unwrap()
        );
        assert_eq!(
            store.native_tool_result_publications_after(0, 100).unwrap(),
            before
        );
    }
}

#[tokio::test]
async fn typed_binding_errors_block_even_without_a_cursor() {
    for warm in [false, true] {
        for (failure, label) in [
            (NativeFileReadError::SourceChanged, "file-source-changed"),
            (NativeFileReadError::InvalidCursor, "file-cursor-invalid"),
            (NativeFileReadError::Unsupported, "file-unsupported"),
        ] {
            let (_directory, store) = fixture(warm).await;
            let cursor = warm.then(|| receipt(&store).cursor).flatten();
            let before = store.native_tool_result_publications_after(0, 100).unwrap();
            assert!(
                process_next_with(&store, &|| NOW + 1, &|_| {}, move |_, _| {
                    Err(failure.into())
                })
                .await
                .unwrap()
            );
            let failed = receipt(&store);
            assert!(
                failed.blocked && !failed.pending && !failed.leased,
                "{failed:?}"
            );
            assert_eq!(failed.cursor, cursor);
            assert_eq!(failed.retry_count, 1);
            assert_eq!(failed.next_attempt, None);
            assert_eq!(failed.last_error.as_deref(), Some(label));
            assert_eq!(
                store.native_tool_result_publications_after(0, 100).unwrap(),
                before
            );
            assert!(
                !process_next(&store, &|| NOW + 10_000, &|_| {})
                    .await
                    .unwrap()
            );
        }
    }
}

#[tokio::test]
async fn cancellation_preserves_cursor_retry_budget_and_diagnostics_without_a_gap() {
    for warm in [false, true] {
        let (_directory, store) = fixture(warm).await;
        if !warm {
            assert!(
                store
                    .claim_next_native_file(NOW, super::super::LEASE_SECS)
                    .unwrap()
                    .is_some()
            );
            store.recover_native_file_claims().unwrap();
        }
        store
            .lock()
            .execute(
                "UPDATE native_file_delivery SET retry_count = 4, pending = 1,
                 next_attempt_at_epoch = ?1, last_error = ?2",
                rusqlite::params![NOW + 1, warm.then_some("file-read-failed")],
            )
            .unwrap();
        let before = receipt(&store);
        let publications = store.native_tool_result_publications_after(0, 100).unwrap();
        let announced = AtomicUsize::new(0);
        assert!(
            process_next_with(
                &store,
                &|| NOW + 1,
                &|_| {
                    announced.fetch_add(1, Ordering::SeqCst);
                },
                |_, _| Err(NativeFileReadError::Cancelled.into())
            )
            .await
            .unwrap()
        );
        let cancelled = receipt(&store);
        assert_eq!(cancelled.cursor, before.cursor);
        assert_eq!(cancelled.retry_count, before.retry_count);
        assert_eq!(cancelled.last_error, before.last_error);
        assert!(cancelled.pending && !cancelled.blocked && !cancelled.leased);
        assert_eq!(cancelled.next_attempt, Some(NOW + 2));
        assert_eq!(announced.load(Ordering::SeqCst), 0);
        assert_eq!(
            store.native_tool_result_publications_after(0, 100).unwrap(),
            publications
        );
        assert!(!process_next(&store, &|| NOW + 1, &|_| {}).await.unwrap());
        assert!(process_next(&store, &|| NOW + 2, &|_| {}).await.unwrap());
        assert_eq!(receipt(&store).retry_count, 0);
    }
}
