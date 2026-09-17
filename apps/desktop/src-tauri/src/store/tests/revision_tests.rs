//! Store-level tests for the write order evidence the session lifecycle
//! registry consumes: the writing connection's [`Revision`], each session
//! row's persisted [`Incarnation`], and the bounded presence lookup.

use super::*;

mod migration_collision;

/// The incarnation one session row holds, read straight from the table.
fn stored_incarnation(store: &Store, key: &SessionKey) -> Option<u64> {
    store
        .lock()
        .query_row(
            "SELECT incarnation FROM session
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
            |row| row.get::<_, u64>(0),
        )
        .optional()
        .unwrap()
}

fn counter(store: &Store) -> u64 {
    store
        .lock()
        .query_row(
            "SELECT value FROM session_incarnation_seq WHERE id = 1",
            [],
            |row| row.get::<_, u64>(0),
        )
        .unwrap()
}

/// The database contains one session row at v48 before the v51 incarnation migration.
fn v48_store_with_row(session_id: &str) -> Store {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..48] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute(
            "INSERT INTO session (
                 environment_key, agent, session_id, source_kind, source_label,
                 updated_at_epoch, first_seen_at, last_seen_at)
             VALUES ('native', 'claude-code', ?1, 'file', ?2, 4_000,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            params![session_id, format!("/pre/{session_id}.jsonl")],
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 48).unwrap();
    Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-incarnation-migration-test").to_path_buf(),
    )
    .expect("v51 migrates the v48 schema")
}

#[test]
fn every_session_write_path_advances_the_revision() {
    const NOW: i64 = 2_000_000_000;
    const DAY: i64 = 86_400;
    let store = store();
    let before = store.revision();

    let (_, after_upsert) = store
        .upsert_sessions(
            &[
                session("kept", NOW - DAY),
                session("deleted", NOW - DAY),
                session("expired", NOW - 31 * DAY),
            ],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert!(
        after_upsert > before,
        "upsert: {after_upsert:?} > {before:?}"
    );
    assert_eq!(after_upsert, store.revision());

    let (_, after_delete) = store
        .delete_session(&SessionKey::new("native", "claude-code", "deleted"))
        .unwrap()
        .expect("the row existed");
    assert!(after_delete > after_upsert, "delete advances");
    assert_eq!(after_delete, store.revision());

    store
        .save_settings(&AppSettings {
            session_data_retention_days: SESSION_DATA_RETENTION_DAYS_30,
            ..AppSettings::default()
        })
        .unwrap();
    let settings_revision = store.revision();
    assert!(
        settings_revision > after_delete,
        "a settings write is a write"
    );
    let (removed, after_retention) = store.apply_session_retention(NOW).unwrap();
    assert_eq!(removed, 1);
    assert!(after_retention > settings_revision, "retention advances");
    assert_eq!(after_retention, store.revision());

    let (cleared, after_clear) = store.clear_local_session_data().unwrap();
    assert_eq!(cleared, 1);
    assert!(after_clear > after_retention, "clear advances");
    assert_eq!(after_clear, store.revision());
}

#[test]
fn revisions_are_strictly_increasing_across_transactions() {
    let store = store();
    let mut last = store.revision();
    for index in 0..20 {
        let (_, revision) = store
            .upsert_sessions(
                &[session(&format!("row-{index}"), 1_000 + index)],
                &crate::agents::evidence_cohort(),
            )
            .unwrap();
        assert!(
            revision > last,
            "transaction {index}: {revision:?} > {last:?}"
        );
        last = revision;
    }
    for index in 0..20 {
        let (_, revision) = store
            .delete_session(&SessionKey::new(
                "native",
                "claude-code",
                format!("row-{index}"),
            ))
            .unwrap()
            .unwrap();
        assert!(revision > last, "delete {index}: {revision:?} > {last:?}");
        last = revision;
    }
    assert_eq!(
        store.revision(),
        last,
        "a read with no write in between returns the same revision"
    );
}

#[test]
fn a_read_returns_the_revision_of_the_rows_it_saw() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "witnessed");
    store
        .upsert_sessions(
            &[session("witnessed", 4_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    let (present, seen_at) = store
        .session_presence_for_keys(std::slice::from_ref(&key))
        .unwrap();
    assert_eq!(present.len(), 1);
    assert_eq!(seen_at, store.revision(), "no write since the read");

    let (again, seen_again) = store
        .session_presence_for_keys(std::slice::from_ref(&key))
        .unwrap();
    assert_eq!(
        (again, seen_again),
        (present.clone(), seen_at),
        "same rows, same revision"
    );

    let (deleted, deleted_at) = store.delete_session(&key).unwrap().unwrap();
    assert_eq!(deleted, present[0].incarnation);
    assert!(deleted_at > seen_at, "the deletion is after the read");

    let (absent, absent_at) = store
        .session_presence_for_keys(std::slice::from_ref(&key))
        .unwrap();
    assert!(absent.is_empty(), "the row is absent at the later revision");
    assert!(absent_at >= deleted_at, "{absent_at:?} >= {deleted_at:?}");
}

#[test]
fn a_rolled_back_transaction_leaves_the_revision_monotone() {
    let store = store();
    store
        .upsert_sessions(&[session("kept", 4_000)], &crate::agents::evidence_cohort())
        .unwrap();
    let before = store.revision();
    let counter_before = counter(&store);

    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        tx.execute(
            "UPDATE session_incarnation_seq SET value = value + 1 WHERE id = 1",
            [],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO session (
                 environment_key, agent, session_id, source_kind, source_label,
                 updated_at_epoch, first_seen_at, last_seen_at, incarnation)
             VALUES ('native', 'claude-code', 'rolled-back', 'file', '/rb.jsonl', 5_000,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                     (SELECT value FROM session_incarnation_seq WHERE id = 1))",
            [],
        )
        .unwrap();
        tx.rollback().unwrap();
    }

    let after_rollback = store.revision();
    assert!(
        after_rollback >= before,
        "never decreases: {after_rollback:?} >= {before:?}"
    );
    assert_eq!(store.session_count().unwrap(), 1, "nothing committed");
    assert_eq!(
        counter(&store),
        counter_before,
        "the counter rolls back too"
    );

    let (_, committed) = store
        .upsert_sessions(
            &[session("later", 6_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert!(
        committed > after_rollback,
        "a later commit ends above every earlier read"
    );
    assert!(committed > before);
    assert_eq!(
        stored_incarnation(&store, &SessionKey::new("native", "claude-code", "later")),
        Some(counter_before + 1),
        "the rolled-back allocation is reused"
    );
}

#[test]
fn incarnations_are_stable_across_updates_and_increase_across_recreates() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "reborn");
    let (first, _) = store
        .upsert_sessions(
            &[session("reborn", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let mut renamed = session("reborn", 2_000);
    renamed.title = Some("Renamed".into());
    renamed.source_fingerprint = Some("sv1:changed".into());
    let (second, _) = store
        .upsert_sessions(&[renamed], &crate::agents::evidence_cohort())
        .unwrap();
    assert_eq!(first, second, "an update keeps the incarnation");
    assert_eq!(first[0].1, Incarnation(1));
    assert_eq!(stored_incarnation(&store, &key), Some(1));

    let (deleted, _) = store.delete_session(&key).unwrap().unwrap();
    assert_eq!(deleted, Incarnation(1));

    let (third, _) = store
        .upsert_sessions(
            &[session("reborn", 3_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert!(
        third[0].1 > first[0].1,
        "a re-create is a higher incarnation"
    );
    assert_eq!(third[0].1, Incarnation(2));
    assert_eq!(stored_incarnation(&store, &key), Some(2));
}

#[test]
fn a_clear_keeps_the_incarnation_counter() {
    let store = store();
    let (first, _) = store
        .upsert_sessions(
            &[session("a", 1_000), session("b", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert_eq!(first[1].1, Incarnation(2));
    assert_eq!(counter(&store), 2);

    store.clear_local_session_data().unwrap();
    assert_eq!(counter(&store), 2, "a clear never touches the counter");

    let (again, _) = store
        .upsert_sessions(&[session("a", 1_000)], &crate::agents::evidence_cohort())
        .unwrap();
    assert_eq!(
        again[0].1,
        Incarnation(3),
        "rediscovery after a clear is a re-create"
    );
}

#[test]
fn pre_migration_rows_have_incarnation_zero_and_a_recreate_exceeds_it() {
    let store = v48_store_with_row("upgraded");
    let key = SessionKey::new("native", "claude-code", "upgraded");
    assert_eq!(store.schema_version().unwrap(), 52);
    assert_eq!(stored_incarnation(&store, &key), Some(0));
    assert_eq!(counter(&store), 0);

    let (updated, _) = store
        .upsert_sessions(
            &[session("upgraded", 5_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert_eq!(
        updated[0].1,
        Incarnation(0),
        "an update keeps the pre-migration zero"
    );
    assert_eq!(counter(&store), 0, "an update allocates nothing");

    let (deleted, _) = store.delete_session(&key).unwrap().unwrap();
    assert_eq!(deleted, Incarnation(0));
    let (recreated, _) = store
        .upsert_sessions(
            &[session("upgraded", 6_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert_eq!(
        recreated[0].1,
        Incarnation(1),
        "the first allocation exceeds zero"
    );
}

/// The upsert that carries a `fork_parent_session_id` writes the session
/// row and a `forkParent` relation placeholder for the parent. The parent
/// itself gets no session row from this path, so only the child row carries
/// an incarnation, and the relation carries none.
#[test]
fn the_fork_parent_placeholder_row_gets_an_incarnation() {
    let store = store();
    let child_key = SessionKey::new("native", "claude-code", "child");
    let parent_key = SessionKey::new("native", "claude-code", "parent");
    let mut child = session("child", 1_000);
    child.fork_parent_session_id = Some("parent".into());

    let (first, _) = store
        .upsert_sessions(
            std::slice::from_ref(&child),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert_eq!(first, vec![(child_key.clone(), Incarnation(1))]);
    assert_eq!(
        store.fork_parent(&child_key).unwrap().as_deref(),
        Some("parent")
    );
    assert_eq!(
        stored_incarnation(&store, &parent_key),
        None,
        "the placeholder is a relation row, not a session row"
    );
    assert_eq!(counter(&store), 1, "one row, one allocation");

    let (second, _) = store
        .upsert_sessions(
            std::slice::from_ref(&child),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert_eq!(
        second, first,
        "a repeat with the same parent keeps the incarnation"
    );

    // The parent discovered later is its own row with its own incarnation.
    let (parent, _) = store
        .upsert_sessions(&[session("parent", 900)], &crate::agents::evidence_cohort())
        .unwrap();
    assert_eq!(parent, vec![(parent_key.clone(), Incarnation(2))]);
    assert_eq!(
        store.fork_children(&parent_key).unwrap(),
        vec!["child".to_string()]
    );

    // A re-created child carrying the same parent gets a higher incarnation.
    store.delete_session(&child_key).unwrap().unwrap();
    let (third, _) = store
        .upsert_sessions(
            std::slice::from_ref(&child),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert_eq!(third, vec![(child_key.clone(), Incarnation(3))]);
    assert_eq!(
        store.fork_parent(&child_key).unwrap().as_deref(),
        Some("parent")
    );

    // `record_fork_parent` alone never creates a session row either.
    let orphan = SessionKey::new("native", "claude-code", "orphan");
    assert!(store.record_fork_parent(&orphan, "parent").unwrap());
    assert_eq!(stored_incarnation(&store, &orphan), None);
    assert_eq!(counter(&store), 3);
}

#[test]
fn delete_session_returns_the_deleted_incarnation_and_revision() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "gone");
    store
        .upsert_sessions(&[session("gone", 1_000)], &crate::agents::evidence_cohort())
        .unwrap();
    store.delete_session(&key).unwrap().unwrap();
    let (rows, _) = store
        .upsert_sessions(&[session("gone", 2_000)], &crate::agents::evidence_cohort())
        .unwrap();
    let before = store.revision();

    let (incarnation, revision) = store
        .delete_session(&key)
        .unwrap()
        .expect("the row existed");
    assert_eq!(
        incarnation, rows[0].1,
        "the incarnation of the row that was deleted"
    );
    assert_eq!(incarnation, Incarnation(2));
    assert!(revision > before);
    assert_eq!(revision, store.revision());

    assert_eq!(
        store.delete_session(&key).unwrap(),
        None,
        "no row, no evidence"
    );
}

#[test]
fn upsert_sessions_returns_each_row_incarnation() {
    let store = store();
    store
        .upsert_sessions(
            &[session("existing", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    let (rows, revision) = store
        .upsert_sessions(
            &[
                session("new-a", 2_000),
                session("existing", 2_000),
                session("new-b", 2_000),
            ],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    assert_eq!(
        rows,
        vec![
            (
                SessionKey::new("native", "claude-code", "new-a"),
                Incarnation(2)
            ),
            (
                SessionKey::new("native", "claude-code", "existing"),
                Incarnation(1)
            ),
            (
                SessionKey::new("native", "claude-code", "new-b"),
                Incarnation(3)
            ),
        ],
        "records order; existing on update, new on insert"
    );
    assert_eq!(revision, store.revision());
    let (none, empty_revision) = store
        .upsert_sessions(&[], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(none.is_empty());
    assert_eq!(empty_revision, revision, "an empty batch changes no row");
}

#[test]
fn session_presence_for_keys_returns_incarnation_epoch_and_a_revision() {
    let store = store();
    let mut null_epoch = session("null-epoch", 0);
    null_epoch.updated_at_epoch = None;
    let (rows, _) = store
        .upsert_sessions(
            &[session("one", 1_000), session("two", 2_000), null_epoch],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let one = SessionKey::new("native", "claude-code", "one");
    let two = SessionKey::new("native", "claude-code", "two");
    let null = SessionKey::new("native", "claude-code", "null-epoch");
    let missing = SessionKey::new("wsl:Ubuntu", "claude-code", "one");

    let (present, revision) = store
        .session_presence_for_keys(&[two.clone(), missing.clone(), one.clone(), null.clone()])
        .unwrap();
    let mut present = present;
    present.sort_by(|a, b| a.key.cmp(&b.key));
    assert_eq!(
        present,
        vec![
            Presence {
                key: null.clone(),
                incarnation: rows[2].1,
                epoch: 0,
            },
            Presence {
                key: one.clone(),
                incarnation: rows[0].1,
                epoch: 1_000,
            },
            Presence {
                key: two.clone(),
                incarnation: rows[1].1,
                epoch: 2_000,
            },
        ],
        "a missing environment twin is absent; a NULL epoch reads as zero"
    );
    assert_eq!(revision, store.revision());

    let (empty, empty_revision) = store.session_presence_for_keys(&[]).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty_revision, revision);
}

#[test]
fn session_presence_for_keys_answers_the_cap_and_rejects_one_more() {
    let store = store();
    let keys: Vec<SessionKey> = (0..=PRESENCE_LOOKUP_CAP)
        .map(|index| SessionKey::new("native", "claude-code", format!("k-{index:03}")))
        .collect();
    let records: Vec<SessionRecord> = keys
        .iter()
        .map(|key| session(&key.session_id, 1_000))
        .collect();
    store
        .upsert_sessions(&records, &crate::agents::evidence_cohort())
        .unwrap();

    let (rows, _) = store
        .session_presence_for_keys(&keys[..PRESENCE_LOOKUP_CAP])
        .unwrap();
    assert_eq!(
        rows.len(),
        PRESENCE_LOOKUP_CAP,
        "one statement answers the cap"
    );

    let error = store
        .session_presence_for_keys(&keys)
        .expect_err("one key past the cap is refused");
    assert!(
        error.to_string().contains("exceeds the cap"),
        "unexpected error: {error:#}"
    );
}

/// The one writing connection is the evidence source. Read-only connections
/// exist for reports and exports, but nothing reads `total_changes()` from
/// them, and `open_read_only` cannot write.
#[test]
fn the_app_database_has_one_writing_connection() {
    let store_source = include_str!("../mod.rs");
    assert_eq!(
        store_source.matches("Connection::open(").count(),
        1,
        "store/mod.rs opens the app database for writing in Store::open only"
    );
    assert_eq!(
        store_source.matches("Connection::open_with_flags(").count(),
        1,
        "the only other open in store/mod.rs is open_read_only"
    );
    let read_only = store_source
        .split("pub fn open_read_only")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("open_read_only body");
    assert!(read_only.contains("SQLITE_OPEN_READ_ONLY"));
    assert!(read_only.contains("\"query_only\", true"));

    for (name, source) in [
        ("store/model.rs", include_str!("../model.rs")),
        (
            "diagnostics_export.rs",
            include_str!("../../diagnostics_export.rs"),
        ),
        (
            "insights_report.rs",
            include_str!("../../insights_report.rs"),
        ),
        (
            "insights_report/findings.rs",
            include_str!("../../insights_report/findings.rs"),
        ),
        (
            "session_lifecycle.rs",
            include_str!("../../session_lifecycle.rs"),
        ),
        ("scan/mod.rs", include_str!("../../scan/mod.rs")),
        ("scan/scoped.rs", include_str!("../../scan/scoped.rs")),
        ("commands.rs", include_str!("../../commands/mod.rs")),
        ("retention.rs", include_str!("../../retention.rs")),
        ("repositories.rs", include_str!("../../repositories.rs")),
    ] {
        assert!(
            !source.contains(".total_changes()"),
            "{name} reads total_changes: only store/mod.rs supplies revisions"
        );
    }
    assert_eq!(
        store_source.matches(".total_changes()").count(),
        1,
        "store/mod.rs reads total_changes in revision_of only"
    );

    let directory = tempfile::tempdir().unwrap();
    let writer = Store::open(directory.path()).unwrap();
    writer
        .upsert_sessions(&[session("seen", 1_000)], &crate::agents::evidence_cohort())
        .unwrap();
    let reader = open_read_only(directory.path(), Duration::from_millis(100)).unwrap();
    let error = reader
        .execute("DELETE FROM session", [])
        .expect_err("a read-only connection cannot write");
    assert!(
        error.to_string().contains("readonly") || error.to_string().contains("read-only"),
        "unexpected error: {error}"
    );
}

/// Every production `INSERT INTO session` supplies `incarnation` from the
/// counter, and no `UPDATE` or `DO UPDATE` names the column.
#[test]
fn every_session_insert_assigns_an_incarnation_and_no_update_touches_it() {
    let sources = [
        ("store/mod.rs", include_str!("../mod.rs")),
        ("store/remediation.rs", include_str!("../remediation.rs")),
        (
            "store/provider_limit.rs",
            include_str!("../provider_limit.rs"),
        ),
        (
            "store/provider_usage_history.rs",
            include_str!("../provider_usage_history.rs"),
        ),
        (
            "store/codex_rollout_checkpoint.rs",
            include_str!("../codex_rollout_checkpoint.rs"),
        ),
        ("store/publication.rs", include_str!("../publication.rs")),
        ("scan/mod.rs", include_str!("../../scan/mod.rs")),
        ("scan/scoped.rs", include_str!("../../scan/scoped.rs")),
        ("commands.rs", include_str!("../../commands/mod.rs")),
        ("repositories.rs", include_str!("../../repositories.rs")),
        ("fork_lineage.rs", include_str!("../../fork_lineage.rs")),
        (
            "insights_worker.rs",
            include_str!("../../insights_worker.rs"),
        ),
    ];
    let mut inserts = 0;
    for (name, source) in sources {
        // Inline test modules hold fixture inserts; production code ends
        // where one begins.
        let production = source
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .unwrap_or(source);
        for (offset, _) in production.match_indices("INSERT INTO session") {
            let statement = &production[offset..];
            let head = statement.split("VALUES").next().unwrap_or(statement);
            // Other tables share the prefix: `session_relation`,
            // `session_evidence`, and friends are not session rows.
            if head.starts_with("INSERT INTO session_") {
                continue;
            }
            inserts += 1;
            assert!(
                head.contains("incarnation"),
                "{name}: a session insert without incarnation:\n{head}"
            );
            let update = statement
                .split("DO UPDATE SET")
                .nth(1)
                .map(|rest| rest.split("\",").next().unwrap_or(rest))
                .unwrap_or("");
            assert!(
                !update.contains("incarnation"),
                "{name}: DO UPDATE names incarnation:\n{update}"
            );
        }
        for (offset, _) in production
            .match_indices("UPDATE session\n")
            .chain(production.match_indices("UPDATE session "))
        {
            let statement = &production[offset..offset + 400.min(production.len() - offset)];
            assert!(
                !statement.contains("incarnation"),
                "{name}: an UPDATE names incarnation:\n{statement}"
            );
        }
    }
    assert_eq!(
        inserts, 1,
        "one production session insert: upsert_session_in"
    );
    let store_source = include_str!("../mod.rs");
    assert_eq!(
        store_source
            .matches("session_incarnation_seq SET value")
            .count(),
        1,
        "one allocation site: allocate_incarnation_in"
    );
    assert!(
        !store_source.contains("DELETE FROM session_incarnation_seq")
            && !store_source.contains("session_incarnation_seq SET value = 0"),
        "nothing resets the counter"
    );
}
