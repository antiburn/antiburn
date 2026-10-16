use antiburn_local::analysis::read_opencode_tool_result_page;

use super::*;
use crate::insights_worker::journal::tests::SyntheticJournal;

#[test]
fn live_window_and_incremental_state_commit_atomically_and_resume_after_restart() {
    use antiburn_local::analysis::{ToolResultAdmission, read_opencode_live_tool_result_page};
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.results(0..300);
    source.start_result(300);
    source.index(&store, "/synthetic");
    let claim = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    let page =
        read_opencode_live_tool_result_page(&source.path, "native-session", None, None, &|| false)
            .unwrap();
    assert_eq!(page.results.len(), 40);
    assert!(!page.has_more);
    store.lock().execute_batch("CREATE TEMP TRIGGER reject_live_cursor BEFORE UPDATE OF cursor_json ON opencode_journal_delivery
        BEGIN SELECT RAISE(ABORT, 'synthetic live rollback'); END;").unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .is_err()
    );
    assert!(
        store
            .native_tool_result_publications_after(0, 100)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT cursor_json FROM opencode_journal_delivery",
                [],
                |row| row.get::<_, Option<String>>(0)
            )
            .unwrap(),
        None
    );
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_live_cursor")
        .unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .unwrap()
    );
    let published = store.native_tool_result_publications_after(0, 100).unwrap();
    assert_eq!(published.len(), 40);
    assert!(
        published
            .iter()
            .all(|event| event.live_eligible == Some(true))
    );
    let frozen = published[30].payload.as_ref().unwrap().context.clone();
    let head = published.last().unwrap().publication_sequence;
    drop(store);
    let store = Store::open(state.path()).unwrap();
    source.result(300);
    source.result(290);
    source.result(0);
    source.index(&store, "/synthetic");
    let next = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    assert_eq!(next.cursor().unwrap(), Some(page.cursor.clone()));
    assert_eq!(next.baseline_sequence, page.cursor.live_baseline());
    let page = read_opencode_live_tool_result_page(
        &source.path,
        "native-session",
        next.cursor().unwrap().as_ref(),
        next.baseline_sequence,
        &|| false,
    )
    .unwrap();
    assert_eq!(page.live_input_events, 3);
    assert!(
        store
            .publish_opencode_journal_page(&next, &page, 101)
            .unwrap()
    );
    assert!(
        !store
            .publish_opencode_journal_page(&next, &page, 101)
            .unwrap()
    );
    let events = store
        .native_tool_result_publications_after(head, 100)
        .unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0].payload.as_ref().unwrap().admission(),
        ToolResultAdmission::Admitted
    );
    assert_eq!(events[1].payload.as_ref().unwrap().context, frozen);
    assert_eq!(
        events[2].payload.as_ref().unwrap().admission(),
        ToolResultAdmission::UnsafeIdentity
    );
}

#[test]
fn cursor_and_baseline_commit_atomically_with_outbox_and_survive_restart() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(40 * 1024);
    source.results(0..300);
    source.index(&store, "/synthetic");
    let claim = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    let page =
        read_opencode_tool_result_page(&source.path, "native-session", None, &|| false).unwrap();
    assert!(page.has_more && page.history_complete && !page.complete);
    store.lock().execute_batch("CREATE TEMP TRIGGER reject_journal_cursor BEFORE UPDATE OF cursor_json ON opencode_journal_delivery
        BEGIN SELECT RAISE(ABORT, 'synthetic rollback'); END;").unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .is_err()
    );
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT cursor_json FROM opencode_journal_delivery",
                [],
                |row| row.get::<_, Option<String>>(0)
            )
            .unwrap(),
        None
    );
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_journal_cursor")
        .unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .unwrap()
    );
    assert!(
        !store
            .publish_opencode_journal_page(&claim, &page, 100)
            .unwrap()
    );
    let count: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM native_tool_result_publication",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(store);
    let store = Store::open(state.path()).unwrap();
    let next = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    assert_eq!(next.cursor().unwrap(), Some(page.cursor));
    assert_eq!(next.baseline_sequence, Some(source.sequence as u64));
    let page = read_opencode_tool_result_page(
        &source.path,
        "native-session",
        next.cursor().unwrap().as_ref(),
        &|| false,
    )
    .unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&next, &page, 101)
            .unwrap()
    );
    assert!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM native_tool_result_publication",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap()
            > count
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM native_tool_result_current WHERE is_removed = 1",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    use antiburn_checks::source::{PublicationCursor, PublicationRead, PublishedEvidenceReader};
    let reader = crate::published_evidence_reader::StorePublishedEvidenceReader::new(&store);
    let published = reader
        .read_batch(PublicationRead {
            after: PublicationCursor(0),
            limit: 1000,
        })
        .unwrap();
    assert!(published.has_more);
    assert!(published.events.len() < count as usize + page.results.len());
}

#[test]
fn unassigned_sessions_wait_for_an_enabled_activation_in_the_assigned_project() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    let mut record = source.record("/synthetic");
    record.cwd = None;
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(
        store
            .claim_next_opencode_journal(100, 60)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.next_opencode_journal_attempt(100).unwrap(), None);
    record.cwd = Some("/new-project".into());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(
        store
            .claim_next_opencode_journal(101, 60)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
    SyntheticJournal::enable_live(&store, "/new-project");
    let claim = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    let page =
        read_opencode_tool_result_page(&source.path, "native-session", None, &|| false).unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 101)
            .unwrap()
    );
    assert_eq!(
        store
            .native_tool_result_publications_for_project_after("native", "/new-project", 0, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn append_during_a_page_keeps_lease_and_prefix_cas_but_not_project_or_incarnation_changes() {
    for change in [
        "append",
        "project",
        "incarnation",
        "lease",
        "fork",
        "subagent",
        "wsl",
    ] {
        let state = tempfile::tempdir().unwrap();
        let store = Store::open(state.path()).unwrap();
        SyntheticJournal::enable_live(&store, "/synthetic");
        SyntheticJournal::enable_live(&store, "/other");
        let mut source = SyntheticJournal::new(16);
        source.result(0);
        source.index(&store, "/synthetic");
        let claim = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
        let page = read_opencode_tool_result_page(&source.path, "native-session", None, &|| false)
            .unwrap();
        source.result(1);
        if change == "incarnation" {
            store.delete_session(&claim.key).unwrap();
        }
        let mut record = source.record(if change == "project" {
            "/other"
        } else {
            "/synthetic"
        });
        if change == "fork" {
            record.fork_parent_session_id = Some("synthetic-parent".into());
        }
        if change == "subagent" {
            store.lock().execute("INSERT INTO session_relation VALUES ('native', 'opencode', 'parent', 'subagent', 'native-session', NULL)", []).unwrap();
        }
        if change == "wsl" {
            record.wsl_distro = Some("Synthetic".into());
        }
        store
            .upsert_sessions(&[record], &crate::agents::evidence_cohort())
            .unwrap();
        let now = if change == "lease" { 161 } else { 101 };
        assert_eq!(
            store.renew_opencode_journal_lease(&claim, now, 60).unwrap(),
            change == "append"
        );
        assert_eq!(
            store
                .publish_opencode_journal_page(&claim, &page, now)
                .unwrap(),
            change == "append",
            "{change}"
        );
        let events = store.native_tool_result_publications_after(0, 10).unwrap();
        if change == "append" {
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].live_eligible, Some(false));
            let next = store.claim_next_opencode_journal(102, 60).unwrap().unwrap();
            let page = read_opencode_tool_result_page(
                &source.path,
                "native-session",
                next.cursor().unwrap().as_ref(),
                &|| false,
            )
            .unwrap();
            assert!(
                store
                    .publish_opencode_journal_page(&next, &page, 102)
                    .unwrap()
            );
            assert_eq!(
                store
                    .native_tool_result_publications_after(events[0].publication_sequence, 10)
                    .unwrap()[0]
                    .live_eligible,
                Some(true)
            );
        } else {
            assert!(events.is_empty());
            if matches!(change, "fork" | "subagent" | "wsl") {
                assert!(
                    store
                        .claim_next_opencode_journal(now, 60)
                        .unwrap()
                        .is_none()
                );
                continue;
            }
            let replacement = store.claim_next_opencode_journal(now, 60).unwrap().unwrap();
            assert_ne!(replacement.claim_fence, claim.claim_fence);
        }
    }
}

#[test]
fn known_fork_ownership_fences_admitted_work_and_prevents_new_reads() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.index(&store, "/synthetic");
    let claim = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    let page =
        read_opencode_tool_result_page(&source.path, "native-session", None, &|| false).unwrap();
    let original_context = page.results[0].context.clone();
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, 100)
            .unwrap()
    );
    let head = store.native_tool_result_publications_after(0, 10).unwrap()[0].publication_sequence;
    let admitted = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    let mut record = source.record("/synthetic");
    record.fork_parent_session_id = Some("synthetic-parent".into());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(
        !store
            .publish_opencode_journal_page(&admitted, &page, 101)
            .unwrap()
    );
    assert!(
        !store
            .renew_opencode_journal_lease(&admitted, 101, 60)
            .unwrap()
    );
    assert!(
        store
            .claim_next_opencode_journal(101, 60)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.next_opencode_journal_attempt(101).unwrap(), None);
    let current = store.native_tool_results_current(&claim.key).unwrap();
    assert_eq!(current[0].result.context, original_context);
    let events = store
        .native_tool_result_publications_after(head, 10)
        .unwrap();
    assert!(events.is_empty());
}

#[test]
fn changed_prefix_keeps_cursor_and_persists_a_blocked_gap_without_repeated_reads() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.index(&store, "/synthetic");
    let first = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    let page =
        read_opencode_tool_result_page(&source.path, "native-session", None, &|| false).unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&first, &page, 100)
            .unwrap()
    );
    source.result(1);
    source.index(&store, "/synthetic");
    let next = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    source.connection.execute("UPDATE event SET data = replace(data, 'Parser failure 0', 'Changed failure') WHERE seq = 4", []).unwrap();
    assert!(
        read_opencode_tool_result_page(
            &source.path,
            "native-session",
            next.cursor().unwrap().as_ref(),
            &|| false
        )
        .is_err()
    );
    assert!(
        store
            .fail_opencode_journal(&next, JournalFailure::SourceChanged, 101, 131)
            .unwrap()
    );
    source.result(2);
    source.index(&store, "/synthetic");
    assert!(
        store
            .claim_next_opencode_journal(1_000, 60)
            .unwrap()
            .is_none()
    );
    let saved: (String, String) = store
        .lock()
        .query_row(
            "SELECT cursor_json, last_error FROM opencode_journal_delivery",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(saved.0, serde_json::to_string(&page.cursor).unwrap());
    assert_eq!(saved.1, "journal-prefix-changed");
    let events = store.native_tool_result_publications_after(0, 10).unwrap();
    assert_eq!(
        events.last().unwrap().gap_reason.as_deref(),
        Some("journal-prefix-changed")
    );
    drop(store);
    let store = Store::open(state.path()).unwrap();
    assert!(
        store
            .claim_next_opencode_journal(2_000, 60)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .last()
            .unwrap()
            .gap_reason
            .as_deref(),
        Some("journal-prefix-changed")
    );
}

#[test]
fn cancelled_delivery_retains_its_cursor_and_does_not_consume_a_retry_or_publish_a_gap() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.index(&store, "/synthetic");
    let first = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    assert!(
        store
            .fail_opencode_journal(&first, JournalFailure::Cancelled, 100, 130)
            .unwrap()
    );
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .claim_next_opencode_journal(100, 60)
            .unwrap()
            .is_none()
    );
    let retry = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    assert_eq!(retry.retry_count, 0);
    assert!(retry.cursor().unwrap().is_none());
    let page =
        read_opencode_tool_result_page(&source.path, "native-session", None, &|| false).unwrap();
    assert!(
        !store
            .publish_opencode_journal_page(&first, &page, 101)
            .unwrap()
    );
    assert!(
        store
            .publish_opencode_journal_page(&retry, &page, 101)
            .unwrap()
    );
    assert_eq!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn clear_local_data_removes_journal_state_and_fences_old_incarnation_publication() {
    let state = tempfile::tempdir().unwrap();
    let store = Store::open(state.path()).unwrap();
    SyntheticJournal::enable_live(&store, "/synthetic");
    let mut source = SyntheticJournal::new(16);
    source.result(0);
    source.index(&store, "/synthetic");
    let first = store.claim_next_opencode_journal(100, 60).unwrap().unwrap();
    let page =
        read_opencode_tool_result_page(&source.path, "native-session", None, &|| false).unwrap();
    assert!(
        store
            .publish_opencode_journal_page(&first, &page, 100)
            .unwrap()
    );
    source.result(1);
    source.index(&store, "/synthetic");
    let old = store.claim_next_opencode_journal(101, 60).unwrap().unwrap();
    let page = read_opencode_tool_result_page(
        &source.path,
        "native-session",
        old.cursor().unwrap().as_ref(),
        &|| false,
    )
    .unwrap();
    store.clear_local_session_data().unwrap();
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM opencode_journal_delivery",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert!(
        store
            .native_tool_result_publications_after(0, 10)
            .unwrap()
            .is_empty()
    );
    source.index(&store, "/synthetic");
    SyntheticJournal::enable_live(&store, "/synthetic");
    let replacement = store.claim_next_opencode_journal(102, 60).unwrap().unwrap();
    assert_ne!(old.incarnation, replacement.incarnation);
    assert_ne!(old.claim_fence, replacement.claim_fence);
    assert!(
        !store
            .publish_opencode_journal_page(&old, &page, 102)
            .unwrap()
    );
    assert!(replacement.cursor().unwrap().is_none());
}
