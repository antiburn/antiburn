use std::path::Path;

use rusqlite::Connection;

use super::{SESSION_SEARCH_BACKFILL_BATCH_SIZE, SESSION_SEARCH_BUDGET_ERROR};
use crate::store::{
    AnalysisRecord, RepositoryRecord, SessionKey, SessionRecord, Store, apply_session_retention_in,
    database_path, schema,
};

fn store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-session-search-test"))
        .expect("opens search store")
}

fn session(
    environment_key: &str,
    agent: &str,
    session_id: &str,
    title: &str,
    cwd: &str,
    wsl_distro: Option<&str>,
    updated_at_epoch: i64,
) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new(environment_key, agent, session_id),
        source_kind: "file".to_owned(),
        source_label: format!("/tmp/{session_id}.jsonl"),
        wsl_distro: wsl_distro.map(str::to_owned),
        title: Some(title.to_owned()),
        title_source: Some("vendor".to_owned()),
        cwd: Some(cwd.to_owned()),
        surface: "cli".to_owned(),
        updated_at_epoch: Some(updated_at_epoch),
        activity_cursor: format!("{updated_at_epoch}:1"),
        activity_source: "event".to_owned(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: None,
    }
}

fn analysis(key: SessionKey, models: &str) -> AnalysisRecord {
    AnalysisRecord {
        key,
        model_breakdown_json: "{}".to_owned(),
        pricing_breakdown_json: "{}".to_owned(),
        inclusive_models_json: models.to_owned(),
        initial_context_json: None,
        source_summaries_json: None,
        provider_hints_json: None,
        source_fingerprint: "1:1".to_owned(),
        pricing_generation: 1,
        analyzed_generation: 1,
        parser_revision: 1,
        analyzer_revision: 1,
        metrics_schema_revision: 1,
    }
}

#[test]
fn search_has_no_recent_or_five_hundred_session_cap() {
    let store = store();
    let sessions = (0..521)
        .map(|index| {
            session(
                "native",
                "codex",
                &format!("bulk-{index:04}"),
                "Bulk needle session",
                "/work/antiburn",
                None,
                i64::from(index + 1),
            )
        })
        .collect::<Vec<_>>();
    store
        .upsert_sessions(&sessions, &[])
        .expect("indexes sessions");

    let mut cursor = None;
    let mut found = Vec::new();
    loop {
        let page = store
            .search_sessions("bulk needle", cursor.as_deref(), None)
            .expect("searches one page");
        assert!(!page.indexing);
        found.extend(page.results.into_iter().map(|result| result.session_id));
        if !page.has_more {
            assert!(page.next_cursor.is_none());
            break;
        }
        cursor = page.next_cursor;
    }

    assert_eq!(found.len(), 521);
    assert_eq!(found.first().map(String::as_str), Some("bulk-0520"));
    assert_eq!(found.last().map(String::as_str), Some("bulk-0000"));
}

#[test]
fn punctuation_is_literal_and_native_identity_stays_distinct() {
    let store = store();
    let id = "01J7-X9:alpha/beta";
    store
        .upsert_sessions(
            &[
                session(
                    "native",
                    "codex",
                    id,
                    "Fix syntax:error [parser]",
                    "/work/native",
                    None,
                    10,
                ),
                session(
                    "wsl:ubuntu-24.04",
                    "codex",
                    id,
                    "Fix syntax:error [parser]",
                    "/work/wsl",
                    Some("Ubuntu-24.04"),
                    20,
                ),
            ],
            &[],
        )
        .expect("indexes native and WSL sessions");

    let by_id = store
        .search_sessions(id, None, None)
        .expect("searches punctuation id");
    assert_eq!(by_id.results.len(), 2);
    let identities = by_id
        .results
        .iter()
        .map(|result| {
            (
                result.environment_key.as_str(),
                result.wsl_distro.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    assert!(identities.contains(&("native", None)));
    assert!(identities.contains(&("wsl:ubuntu-24.04", Some("Ubuntu-24.04"))));

    assert_eq!(
        store
            .search_sessions("syntax:error [parser]", None, None)
            .expect("treats punctuation as text")
            .results
            .len(),
        2
    );
    assert!(store.search_sessions(&"x".repeat(201), None, None).is_err());
    assert!(store.search_sessions("needle", Some("1"), None).is_err());
    for punctuation in ["\"", "(", ")", "-", "*"] {
        store
            .search_sessions(punctuation, None, None)
            .expect("does not parse punctuation as FTS grammar");
    }
}

#[test]
fn agent_display_names_and_slugs_are_searchable() {
    let store = store();
    store
        .upsert_sessions(
            &[
                session(
                    "native",
                    "claude-code",
                    "claude-alias",
                    "Alias fixture",
                    "/work/claude",
                    None,
                    2,
                ),
                session(
                    "native",
                    "codex",
                    "codex-alias",
                    "Alias fixture",
                    "/work/codex",
                    None,
                    1,
                ),
            ],
            &[],
        )
        .expect("indexes agent aliases");

    assert_eq!(
        store
            .search_sessions("Claude Code", None, None)
            .expect("searches a display name")
            .results[0]
            .session_id,
        "claude-alias"
    );
    assert_eq!(
        store
            .search_sessions("claude-code", None, None)
            .expect("searches a slug")
            .results[0]
            .session_id,
        "claude-alias"
    );
    assert_eq!(
        store
            .search_sessions("Codex", None, None)
            .expect("searches the Codex label")
            .results[0]
            .session_id,
        "codex-alias"
    );
}

#[test]
fn exact_id_and_title_precede_prefixes_then_recency_breaks_ties() {
    let store = store();
    let mut sessions = vec![
        session(
            "native",
            "codex",
            "alpha",
            "Alpha prefix from the exact id",
            "/work/id",
            None,
            1,
        ),
        session(
            "native",
            "codex",
            "title-exact",
            "Alpha",
            "/work/title",
            None,
            2,
        ),
    ];
    sessions.extend((0..43).map(|index| {
        session(
            "native",
            "codex",
            &format!("prefix-{index:02}"),
            "Alphabet project",
            "/work/prefix",
            None,
            100 + i64::from(index),
        )
    }));
    store
        .upsert_sessions(&sessions, &[])
        .expect("indexes ranked fixtures");

    let mut cursor = None;
    let mut ids = Vec::new();
    loop {
        let page = store
            .search_sessions("alpha", cursor.as_deref(), None)
            .expect("searches a deterministic page");
        ids.extend(page.results.into_iter().map(|result| result.session_id));
        if !page.has_more {
            assert!(page.next_cursor.is_none());
            break;
        }
        cursor = page.next_cursor;
    }

    assert_eq!(ids.len(), 45);
    assert_eq!(
        &ids[..4],
        ["alpha", "title-exact", "prefix-42", "prefix-41"]
    );
    let unique = ids.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(unique.len(), ids.len());
}

#[test]
fn metadata_and_model_publication_replace_search_terms() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "publication");
    store
        .upsert_sessions(
            &[session(
                "native",
                "claude-code",
                "publication",
                "Old searchable title",
                "/work/project",
                None,
                1,
            )],
            &[],
        )
        .expect("indexes initial metadata");
    store
        .save_analysis(
            &analysis(key.clone(), r#"[{"model":"claude-opus-4-6"}]"#),
            None,
        )
        .expect("publishes first model");
    assert_eq!(
        store
            .search_sessions("opus-4-6", None, None)
            .expect("finds first model")
            .results[0]
            .models,
        ["claude-opus-4-6"]
    );

    store
        .upsert_sessions(
            &[session(
                "native",
                "claude-code",
                "publication",
                "New searchable title",
                "/work/project",
                None,
                2,
            )],
            &[],
        )
        .expect("updates metadata");
    store
        .save_analysis(&analysis(key, r#"[{"model":"claude-sonnet-4-5"}]"#), None)
        .expect("replaces model publication");

    assert!(
        store
            .search_sessions("old", None, None)
            .expect("searches old title")
            .results
            .is_empty()
    );
    assert!(
        store
            .search_sessions("opus", None, None)
            .expect("searches old model")
            .results
            .is_empty()
    );
    let updated = store
        .search_sessions("new sonnet", None, None)
        .expect("finds new metadata");
    assert_eq!(updated.results.len(), 1);
    assert_eq!(updated.results[0].models, ["claude-sonnet-4-5"]);
}

#[test]
fn cursor_is_query_bound_stable_for_no_op_writes_and_stale_after_changes() {
    let store = store();
    let records = (0..25)
        .map(|index| {
            session(
                "native",
                "codex",
                &format!("cursor-{index:02}"),
                "Cursor needle",
                "/work/cursor",
                None,
                i64::from(index),
            )
        })
        .collect::<Vec<_>>();
    store
        .upsert_sessions(&records, &[])
        .expect("indexes cursor fixtures");
    let model_key = records[0].key.clone();
    let model_record = analysis(model_key, r#"[{"model":"gpt-6"}]"#);
    store
        .save_analysis(&model_record, None)
        .expect("publishes indexed models");

    let first = store
        .search_sessions("cursor", None, None)
        .expect("searches the first page");
    let cursor = first.next_cursor.expect("has a next page");
    assert!(!cursor.contains("cursor"));
    assert!(store.search_sessions("other", Some(&cursor), None).is_err());
    assert!(store.search_sessions("cursor", Some("20"), None).is_err());

    store
        .upsert_sessions(&records, &[])
        .expect("repeats identical metadata");
    store
        .save_analysis(&model_record, None)
        .expect("repeats identical model publication");
    assert_eq!(
        store
            .search_sessions("cursor", Some(&cursor), None)
            .expect("keeps a cursor after no-op writes")
            .results
            .len(),
        5
    );

    let mut changed = records[0].clone();
    changed.title = Some("Cursor needle changed".to_owned());
    store
        .upsert_sessions(&[changed], &[])
        .expect("changes indexed metadata");
    let error = store
        .search_sessions("cursor", Some(&cursor), None)
        .expect_err("rejects a stale cursor");
    assert!(error.to_string().contains("stale session search cursor"));
}

#[test]
fn vm_budget_interrupts_broad_search_and_cleans_up_the_connection_handler() {
    let store = store();
    let records = (0..200)
        .map(|index| {
            session(
                "native",
                "codex",
                &format!("budget-{index:03}"),
                "Budget common match",
                "/work/budget",
                None,
                i64::from(index),
            )
        })
        .collect::<Vec<_>>();
    store
        .upsert_sessions(&records, &[])
        .expect("indexes budget fixtures");

    let error = store
        .search_sessions_with_vm_step_limit("budget", None, 1, None)
        .expect_err("interrupts the bounded statement");
    assert_eq!(error.to_string(), SESSION_SEARCH_BUDGET_ERROR);

    let cross_join_count: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM session AS first CROSS JOIN session AS second",
            [],
            |row| row.get(0),
        )
        .expect("uses the connection after the interrupted search");
    assert_eq!(cross_join_count, 40_000);
}

#[test]
fn repository_names_refresh_and_fallback_to_the_cwd_label() {
    let store = store();
    store
        .upsert_sessions(
            &[session(
                "native",
                "codex",
                "repository",
                "Repository metadata",
                r"C:\work\antiburn\feature",
                None,
                1,
            )],
            &[],
        )
        .expect("indexes session");
    let fallback = store
        .search_sessions("feature", None, None)
        .expect("searches cwd fallback");
    assert_eq!(fallback.results[0].repository, "feature");
    assert_eq!(fallback.results[0].cwd_label, "feature");

    store
        .replace_repositories(&[RepositoryRecord {
            key: "c:/work/antiburn".to_owned(),
            repo_name: "antiburn".to_owned(),
            full_name: "antiburn/antiburn".to_owned(),
            status: "accessible".to_owned(),
            repo_root: Some(r"C:\work\antiburn".to_owned()),
            suspected_path: None,
            worktree_count: 1,
            session_count: 1,
            wsl_distro: None,
            enabled: true,
        }])
        .expect("refreshes repositories");
    let result = store
        .search_sessions("antiburn", None, None)
        .expect("searches repository name");
    assert_eq!(result.results[0].repository, "antiburn");

    store
        .upsert_sessions(
            &[session(
                "native",
                "codex",
                "repository",
                "Repository metadata",
                r"C:\outside\other",
                None,
                2,
            )],
            &[],
        )
        .expect("updates the working directory");
    assert!(
        store
            .search_sessions("antiburn", None, None)
            .expect("removes the old repository term")
            .results
            .is_empty()
    );
    let moved = store
        .search_sessions("other", None, None)
        .expect("finds the new working directory");
    assert_eq!(moved.results[0].repository, "other");
}

#[test]
fn repository_matching_normalizes_windows_and_preserves_wsl_case() {
    let store = store();
    store
        .replace_repositories(&[
            RepositoryRecord {
                key: "c:/users/avery/checkout".to_owned(),
                repo_name: "windows-checkout".to_owned(),
                full_name: "owner/windows-checkout".to_owned(),
                status: "accessible".to_owned(),
                repo_root: Some("c:/users/avery/checkout".to_owned()),
                suspected_path: None,
                worktree_count: 1,
                session_count: 1,
                wsl_distro: None,
                enabled: true,
            },
            RepositoryRecord {
                key: "/home/avery/checkout".to_owned(),
                repo_name: "wsl-lowercase".to_owned(),
                full_name: "owner/wsl-lowercase".to_owned(),
                status: "accessible".to_owned(),
                repo_root: Some("/home/avery/checkout".to_owned()),
                suspected_path: None,
                worktree_count: 1,
                session_count: 1,
                wsl_distro: Some("Ubuntu-24.04".to_owned()),
                enabled: true,
            },
        ])
        .expect("stores platform-specific repositories");
    store
        .upsert_sessions(
            &[
                session(
                    "native",
                    "codex",
                    "windows-path",
                    "Windows path fixture",
                    r"\\?\C:\Users\Avery\Checkout\feature",
                    None,
                    2,
                ),
                session(
                    "wsl:ubuntu-24.04",
                    "codex",
                    "wsl-path",
                    "WSL path fixture",
                    "/home/Avery/Checkout/feature",
                    Some("Ubuntu-24.04"),
                    1,
                ),
            ],
            &[],
        )
        .expect("indexes platform-specific paths");

    let windows = store
        .search_sessions("Windows path fixture", None, None)
        .expect("searches the Windows fixture");
    assert_eq!(windows.results[0].repository, "windows-checkout");
    let wsl = store
        .search_sessions("WSL path fixture", None, None)
        .expect("searches the WSL fixture");
    assert_eq!(wsl.results[0].repository, "feature");
    assert!(
        store
            .search_sessions("wsl-lowercase", None, None)
            .expect("does not fold WSL path case")
            .results
            .is_empty()
    );

    store
        .replace_repositories(&[
            RepositoryRecord {
                key: "c:/users/avery/checkout".to_owned(),
                repo_name: "windows-renamed".to_owned(),
                full_name: "owner/windows-renamed".to_owned(),
                status: "accessible".to_owned(),
                repo_root: Some(r"C:\USERS\AVERY\CHECKOUT".to_owned()),
                suspected_path: None,
                worktree_count: 1,
                session_count: 1,
                wsl_distro: None,
                enabled: true,
            },
            RepositoryRecord {
                key: "/home/avery/checkout".to_owned(),
                repo_name: "wsl-lowercase".to_owned(),
                full_name: "owner/wsl-lowercase".to_owned(),
                status: "accessible".to_owned(),
                repo_root: Some("/home/avery/checkout".to_owned()),
                suspected_path: None,
                worktree_count: 1,
                session_count: 1,
                wsl_distro: Some("Ubuntu-24.04".to_owned()),
                enabled: true,
            },
        ])
        .expect("refreshes normalized repository names");
    assert_eq!(
        store
            .search_sessions("windows-renamed", None, None)
            .expect("searches the refreshed repository")
            .results[0]
            .session_id,
        "windows-path"
    );
}

#[test]
fn deletion_clear_and_rebuild_keep_the_index_consistent() {
    let store = store();
    let first = session(
        "native",
        "codex",
        "first",
        "Erasable needle",
        "/work/first",
        None,
        2,
    );
    let second = session(
        "native",
        "codex",
        "second",
        "Erasable needle",
        "/work/second",
        None,
        1,
    );
    store
        .upsert_sessions(&[first.clone(), second], &[])
        .expect("indexes sessions");
    assert!(
        store
            .delete_session(&first.key)
            .expect("deletes one session")
            .is_some()
    );
    assert_eq!(
        store
            .search_sessions("erasable", None, None)
            .expect("searches after delete")
            .results
            .len(),
        1
    );

    store.rebuild_session_search_index().expect("rebuilds FTS");
    assert_eq!(
        store
            .search_sessions("erasable", None, None)
            .expect("searches rebuilt index")
            .results
            .len(),
        1
    );
    assert_eq!(
        store.clear_local_session_data().expect("clears sessions").0,
        1
    );
    assert!(
        store
            .search_sessions("erasable", None, None)
            .expect("searches cleared index")
            .results
            .is_empty()
    );
}

#[test]
fn retention_removes_search_documents_with_their_sessions() {
    let store = store();
    store
        .upsert_sessions(
            &[session(
                "native",
                "codex",
                "expired",
                "Retention needle",
                "/work/expired",
                None,
                1,
            )],
            &[],
        )
        .expect("indexes expired session");
    assert_eq!(
        apply_session_retention_in(&store.lock(), 30, 2_000_000_000).expect("applies retention"),
        1
    );
    assert!(
        store
            .search_sessions("retention", None, None)
            .expect("searches after retention")
            .results
            .is_empty()
    );
}

#[test]
fn migration_backfill_is_bounded_progressive_and_keeps_concurrent_updates() {
    let directory = tempfile::tempdir().expect("creates data directory");
    let mut connection =
        Connection::open(database_path(directory.path())).expect("opens old store");
    for migration in &schema::MIGRATIONS[..51] {
        connection
            .execute_batch(migration)
            .expect("applies old migration");
    }
    connection
        .execute(
            "INSERT INTO repository(
                 key, repo_name, full_name, status, repo_root, worktree_count,
                 session_count, enabled, last_seen_at
             ) VALUES (
                 'c:/users/avery/checkout', 'backfill-checkout',
                 'owner/backfill-checkout', 'accessible',
                 'c:/users/avery/checkout', 1, 1, 1, '2025-01-01T00:00:00Z'
             )",
            [],
        )
        .expect("seeds an old repository");
    let transaction = connection
        .transaction()
        .expect("starts fixture transaction");
    for index in 0..(SESSION_SEARCH_BACKFILL_BATCH_SIZE * 2 + 1) {
        let cwd = if index == 0 {
            r"\\?\C:\Users\Avery\Checkout\legacy"
        } else {
            "/work/history"
        };
        transaction
            .execute(
                "INSERT INTO session(
                     environment_key, agent, session_id, source_kind, source_label,
                     title, title_source, cwd, surface, updated_at_epoch,
                     subagent_count, first_seen_at, last_seen_at
                 ) VALUES (
                     'native', 'codex', ?1, 'file', ?2,
                     'Historical searchable metadata', 'vendor', ?4,
                     'cli', ?3, 0, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z'
                 )",
                rusqlite::params![
                    format!("legacy-{index:03}"),
                    format!("/tmp/legacy-{index:03}.jsonl"),
                    i64::try_from(index).expect("fixture index fits i64"),
                    cwd,
                ],
            )
            .expect("seeds an old session");
    }
    transaction.commit().expect("commits old sessions");
    connection
        .pragma_update(None, "user_version", 51)
        .expect("marks old schema");
    drop(connection);

    let migrated = Store::open(directory.path()).expect("migrates store");
    let indexed_before: i64 = migrated
        .lock()
        .query_row("SELECT COUNT(*) FROM session_search_document", [], |row| {
            row.get(0)
        })
        .expect("counts documents before search");
    assert_eq!(indexed_before, 0);

    let first = migrated
        .search_sessions("historical", None, None)
        .expect("indexes the first bounded batch");
    assert!(first.indexing);
    let indexed_after_first: i64 = migrated
        .lock()
        .query_row("SELECT COUNT(*) FROM session_search_document", [], |row| {
            row.get(0)
        })
        .expect("counts the first batch");
    assert_eq!(
        indexed_after_first,
        i64::try_from(SESSION_SEARCH_BACKFILL_BATCH_SIZE).expect("batch size fits i64")
    );
    let backfilled_repository: String = migrated
        .lock()
        .query_row(
            "SELECT repository FROM session_search_document
              WHERE environment_key = 'native'
                AND agent = 'codex'
                AND session_id = 'legacy-000'",
            [],
            |row| row.get(0),
        )
        .expect("reads the backfilled repository");
    assert_eq!(backfilled_repository, "backfill-checkout");

    let concurrent_index = SESSION_SEARCH_BACKFILL_BATCH_SIZE * 2;
    let concurrent = session(
        "native",
        "codex",
        &format!("legacy-{concurrent_index:03}"),
        "Concurrent searchable update",
        "/work/history",
        None,
        10_000,
    );
    migrated
        .save_analysis(
            &analysis(concurrent.key.clone(), r#"[{"model":"backfill-model"}]"#),
            None,
        )
        .expect("publishes analysis before the session backfills");
    migrated
        .upsert_sessions(&[concurrent], &[])
        .expect("updates one session during backfill");

    let second = migrated
        .search_sessions("historical", None, None)
        .expect("indexes the second bounded batch");
    assert!(second.indexing);
    let third = migrated
        .search_sessions("historical", None, None)
        .expect("finishes the backfill target");
    assert!(!third.indexing);
    let completed_state: (i64, i64, bool) = migrated
        .lock()
        .query_row(
            "SELECT generation, backfill_rowid, backfill_complete
               FROM session_search_state
              WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("reads completed backfill state");
    migrated
        .search_sessions("historical", None, None)
        .expect("reuses completed backfill state");
    let repeated_state: (i64, i64, bool) = migrated
        .lock()
        .query_row(
            "SELECT generation, backfill_rowid, backfill_complete
               FROM session_search_state
              WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("reads repeated backfill state");
    assert_eq!(repeated_state, completed_state);
    assert!(completed_state.2);
    let updated = migrated
        .search_sessions("concurrent backfill-model", None, None)
        .expect("searches the concurrent metadata and model");
    assert_eq!(updated.results.len(), 1);
    assert_eq!(
        updated.results[0].session_id,
        format!("legacy-{concurrent_index:03}")
    );
}

#[test]
fn prerelease_search_upgrade_preserves_search_and_adds_incarnations() {
    let directory = tempfile::tempdir().expect("creates data directory");
    let connection =
        Connection::open(database_path(directory.path())).expect("opens preview store");
    for migration in &schema::MIGRATIONS[..50] {
        connection
            .execute_batch(migration)
            .expect("applies main migration");
    }
    connection
        .execute_batch(
            "INSERT INTO session (
            environment_key, agent, session_id, source_kind, source_label,
            title, first_seen_at, last_seen_at)
         VALUES ('native', 'codex', 'preview-session', 'file', '/synthetic/preview.jsonl',
            'Preview retained metadata', 'now', 'now');",
        )
        .expect("seeds preview metadata");
    connection
        .execute_batch(schema::MIGRATIONS[57])
        .expect("applies preview search schema");
    connection
        .pragma_update(None, "user_version", 51)
        .expect("sets preview version");
    drop(connection);
    let store = Store::open(directory.path()).expect("upgrades preview store");
    assert_eq!(store.schema_version().unwrap(), 58);
    let incarnation_columns: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('session') WHERE name = 'incarnation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(incarnation_columns, 1);
    let page = store
        .search_sessions("preview", None, None)
        .expect("searches upgraded store");
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].session_id, "preview-session");
    store.migrate().expect("reopens current schema");
}

#[test]
fn activity_scope_filters_before_pagination_and_candidate_limits() {
    let store = store();
    let mut records = (0..30)
        .map(|i| {
            session(
                "native",
                "codex",
                &format!("recent-{i}"),
                "scope marker",
                "/work",
                None,
                100 + i,
            )
        })
        .collect::<Vec<_>>();
    for (id, timestamp) in [("old", 99), ("future", 201), ("unknown", 0)] {
        records.push(session(
            "native",
            "codex",
            id,
            "scope marker",
            "/work",
            None,
            timestamp,
        ));
    }
    store.upsert_sessions(&records, &[]).unwrap();
    let scope = crate::session_search_scope::SessionSearchScope {
        days: 7,
        from_epoch: 100,
        through_epoch: 200,
        time_zone: "Australia/Brisbane".into(),
    };
    let first = store.search_sessions("scope", None, Some(&scope)).unwrap();
    assert_eq!(first.results.len(), 20);
    assert!(
        first
            .results
            .iter()
            .all(|hit| hit.session_id.starts_with("recent-"))
    );
    let cursor = first.next_cursor.as_deref().unwrap();
    let second = store
        .search_sessions("scope", Some(cursor), Some(&scope))
        .unwrap();
    assert_eq!(second.results.len(), 10);
    assert!(!second.has_more);
    assert!(store.search_sessions("scope", Some(cursor), None).is_err());
    for readonly in [false, true] {
        let (hits, _) = if readonly {
            store.readonly_local_session_candidates("scope", Some(&scope))
        } else {
            store.local_session_candidates("scope", Some(&scope))
        }
        .unwrap();
        assert_eq!(hits.len(), 8);
        assert!(hits.iter().all(|hit| hit.session_id.starts_with("recent-")));
    }
    let all = store.search_sessions("scope", None, None).unwrap();
    assert!(all.results.iter().any(|hit| hit.session_id == "future"));
}
