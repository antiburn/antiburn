// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::Path;

use super::*;

fn store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-test-state")).expect("in-memory store")
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
        subagent_count: 0,
        fork_parent_session_id: None,
    }
}

#[test]
fn a_fresh_database_is_migrated_to_the_latest_version() {
    let store = store();
    assert_eq!(
        store.schema_version().unwrap(),
        super::schema::MIGRATIONS.len() as i64
    );
}

#[test]
fn migrating_an_already_current_database_is_a_no_op() {
    let store = store();
    // `migrate` runs on open; running it again must neither fail nor re-apply.
    store.migrate().unwrap();
    store.migrate().unwrap();
    assert_eq!(
        store.schema_version().unwrap(),
        super::schema::MIGRATIONS.len() as i64
    );
}

#[test]
fn settings_default_before_anything_is_written_and_round_trip_after() {
    let store = store();
    let defaults = store.settings().unwrap();
    assert_eq!(defaults, AppSettings::default());
    assert!(!defaults.onboarding_completed);

    // Notifications default on, both kinds with them, so the two per-kind
    // preferences below are a real change rather than a re-statement.
    assert!(defaults.notifications_enabled);
    assert!(defaults.notify_update_available);
    assert!(defaults.notify_scan_failure);

    let saved = store
        .save_settings(&AppSettings {
            theme: ThemePreference::Dark,
            activity_window_days: 14,
            onboarding_completed: true,
            launch_at_login: true,
            auto_update: false,
            discovery_paused: true,
            notifications_enabled: false,
            notify_update_available: false,
            notify_scan_failure: true,
        })
        .unwrap();
    assert_eq!(store.settings().unwrap(), saved);
    assert_eq!(saved.theme, ThemePreference::Dark);
    assert_eq!(saved.activity_window_days, 14);
    assert!(saved.onboarding_completed);
    assert!(saved.discovery_paused);
    // Each notification preference is stored on its own key, so a reader who
    // silences the master switch keeps the per-kind choices they made.
    assert!(!saved.notifications_enabled);
    assert!(!saved.notify_update_available);
    assert!(saved.notify_scan_failure);
}

/// The schema's data policy says which columns may hold free text. This is the
/// mechanical half of that promise: a column added to `session` without a
/// deliberate decision fails here rather than quietly becoming the place
/// transcript text ends up.
#[test]
fn the_session_table_holds_exactly_the_columns_the_data_policy_names() {
    let store = store();
    let connection = store.lock();
    let mut statement = connection
        .prepare("SELECT name FROM pragma_table_info('session')")
        .unwrap();
    let columns: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();

    assert_eq!(
        columns,
        vec![
            "environment_key",
            "agent",
            "session_id",
            "source_kind",
            "source_label",
            "wsl_distro",
            // The one declared free-text excerpt on this table. Everything else
            // is identity, a location, or bookkeeping.
            "title",
            "title_source",
            "cwd",
            "surface",
            "updated_at_epoch",
            "subagent_count",
            "first_seen_at",
            "last_seen_at",
        ]
    );
}

#[test]
fn clearing_the_index_forgets_the_derived_records_and_keeps_the_readers_choices() {
    let store = store();
    store.upsert_sessions(&[session("abc", 2_000)]).unwrap();
    store
        .save_analysis(&AnalysisRecord {
            key: SessionKey::new("native", "claude-code", "abc"),
            metrics_json: "{}".into(),
            cost_json: None,
            model_breakdown_json: "{}".into(),
            active_secs: 1,
            duration_secs: 1,
            pattern_score: 0,
            source_fingerprint: "1:1".into(),
            pricing_generation: 0,
        })
        .unwrap();
    store
        .replace_relations(
            &SessionKey::new("native", "claude-code", "abc"),
            RelationKind::Subagent,
            &[RelationRecord {
                kind: RelationKind::Subagent,
                related_id: "child".into(),
                label: Some("Reviewer".into()),
            }],
        )
        .unwrap();
    store
        .record_agent_scan("claude-code", Some(2_000), 1)
        .unwrap();
    store.add_scan_root("/home/avery/work").unwrap();
    let settings = store
        .save_settings(&AppSettings {
            onboarding_completed: true,
            ..AppSettings::default()
        })
        .unwrap();
    store
        .replace_repositories(&[RepositoryRecord {
            key: "widgets".into(),
            repo_name: "widgets".into(),
            full_name: "avery/widgets".into(),
            status: "accessible".into(),
            repo_root: Some("/home/avery/code/widgets".into()),
            suspected_path: None,
            worktree_count: 1,
            session_count: 3,
            wsl_distro: None,
            enabled: false,
        }])
        .unwrap();

    assert_eq!(store.clear_derived_index().unwrap(), 1);

    assert!(store.recent_sessions(0, 100).unwrap().is_empty());
    assert!(
        store
            .analysis(&SessionKey::new("native", "claude-code", "abc"))
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .relations(&SessionKey::new("native", "claude-code", "abc"))
            .unwrap()
            .is_empty()
    );
    assert!(store.scan_state().unwrap().is_empty());

    // Everything that is a choice rather than a derivation survives.
    assert_eq!(store.settings().unwrap(), settings);
    assert_eq!(store.scan_roots().unwrap(), vec!["/home/avery/work"]);
    let repositories = store.repositories().unwrap();
    assert_eq!(repositories.len(), 1);
    assert!(
        !repositories[0].enabled,
        "the include choice is the reader's"
    );
    assert_eq!(
        repositories[0].session_count, 0,
        "the count was derived from the index that just went away"
    );
}

#[test]
fn clearing_an_already_empty_index_is_a_no_op() {
    let store = store();
    assert_eq!(store.clear_derived_index().unwrap(), 0);
    assert_eq!(store.clear_derived_index().unwrap(), 0);
}

#[test]
fn an_out_of_range_activity_window_is_clamped_on_the_way_in() {
    let store = store();
    let saved = store
        .save_settings(&AppSettings {
            activity_window_days: 9_000,
            ..AppSettings::default()
        })
        .unwrap();
    assert_eq!(saved.activity_window_days, MAX_ACTIVITY_DAYS);
    assert_eq!(
        store.settings().unwrap().activity_window_days,
        MAX_ACTIVITY_DAYS
    );

    let saved = store
        .save_settings(&AppSettings {
            activity_window_days: 0,
            ..AppSettings::default()
        })
        .unwrap();
    assert_eq!(saved.activity_window_days, MIN_ACTIVITY_DAYS);
}

#[test]
fn a_session_round_trips_and_a_rescan_updates_it_in_place() {
    let store = store();
    store.upsert_sessions(&[session("abc", 1_000)]).unwrap();

    let mut later = session("abc", 2_000);
    later.title = Some("Wire the popover, take two".into());
    later.subagent_count = 3;
    store.upsert_sessions(std::slice::from_ref(&later)).unwrap();

    // Idempotent: a rescan that sees the same session again must not duplicate
    // it, and must not rewind the heartbeat.
    store.upsert_sessions(&[session("abc", 1_500)]).unwrap();

    assert_eq!(store.recent_sessions(0, 100).unwrap().len(), 1);
    let stored = store
        .session(&SessionKey::new("native", "claude-code", "abc"))
        .unwrap()
        .expect("session");
    assert_eq!(stored.updated_at_epoch, Some(2_000));
    assert_eq!(stored.subagent_count, 0, "the last scan saw no sub-agents");
    assert_eq!(stored.title.as_deref(), Some("Wire the popover"));
}

#[test]
fn the_same_session_id_in_two_environments_stays_two_sessions() {
    let store = store();
    store.upsert_sessions(&[session("abc", 1_000)]).unwrap();

    let mut in_wsl = session("abc", 1_100);
    in_wsl.key.environment_key = "wsl:ubuntu".into();
    in_wsl.wsl_distro = Some("Ubuntu".into());
    store
        .upsert_sessions(std::slice::from_ref(&in_wsl))
        .unwrap();

    assert_eq!(store.recent_sessions(0, 100).unwrap().len(), 2);
}

#[test]
fn recent_sessions_are_windowed_and_ordered_newest_first() {
    let store = store();
    store.upsert_sessions(&[session("old", 1_000)]).unwrap();
    store.upsert_sessions(&[session("mid", 2_000)]).unwrap();
    store.upsert_sessions(&[session("new", 3_000)]).unwrap();

    let recent = store.recent_sessions(1_500, 100).unwrap();
    let ids: Vec<_> = recent
        .iter()
        .map(|record| record.key.session_id.as_str())
        .collect();
    assert_eq!(ids, vec!["new", "mid"]);

    assert_eq!(
        store.recent_sessions(0, 2).unwrap().len(),
        2,
        "limit applies"
    );
}

#[test]
fn a_fork_parent_rides_with_the_session_and_resolves_children_back() {
    let store = store();
    store.upsert_sessions(&[session("parent", 1_000)]).unwrap();
    let mut child = session("child", 2_000);
    child.fork_parent_session_id = Some("parent".into());
    store.upsert_sessions(std::slice::from_ref(&child)).unwrap();

    let stored = store
        .session(&SessionKey::new("native", "claude-code", "child"))
        .unwrap()
        .expect("child");
    assert_eq!(stored.fork_parent_session_id.as_deref(), Some("parent"));

    let children = store
        .fork_children(&SessionKey::new("native", "claude-code", "parent"))
        .unwrap();
    assert_eq!(children, vec!["child".to_string()]);

    // A later scan carries no observation — it never reads transcripts deeply
    // enough to see one — and must therefore leave the recorded lineage alone
    // rather than reading absence as "no parent".
    store.upsert_sessions(&[session("child", 2_100)]).unwrap();
    assert_eq!(
        store
            .fork_children(&SessionKey::new("native", "claude-code", "parent"))
            .unwrap(),
        vec!["child".to_string()]
    );
}

#[test]
fn analysis_round_trips_and_is_replaced_rather_than_duplicated() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "abc");
    store.upsert_sessions(&[session("abc", 1_000)]).unwrap();

    let record = AnalysisRecord {
        key: key.clone(),
        metrics_json: r#"{"agent":"claude-code","sessionId":"abc"}"#.into(),
        cost_json: Some(r#"{"totalUsd":1.5}"#.into()),
        model_breakdown_json: r#"{"claude-opus-4-6":{"inputTokens":10}}"#.into(),
        active_secs: 120,
        duration_secs: 300,
        pattern_score: 81,
        source_fingerprint: "1700000000:4096".into(),
        pricing_generation: 0,
    };
    store.save_analysis(&record).unwrap();
    assert_eq!(store.analysis(&key).unwrap().as_ref(), Some(&record));

    let updated = AnalysisRecord {
        active_secs: 240,
        source_fingerprint: "1700000900:8192".into(),
        ..record
    };
    store.save_analysis(&updated).unwrap();
    let stored = store.analysis(&key).unwrap().expect("analysis");
    assert_eq!(stored.active_secs, 240);
    assert_eq!(stored.source_fingerprint, "1700000900:8192");
}

#[test]
fn subagent_relations_are_replaced_wholesale() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "abc");
    store.upsert_sessions(&[session("abc", 1_000)]).unwrap();

    store
        .replace_relations(
            &key,
            RelationKind::Subagent,
            &[
                RelationRecord {
                    kind: RelationKind::Subagent,
                    related_id: "agent-1".into(),
                    label: Some("Review the diff".into()),
                },
                RelationRecord {
                    kind: RelationKind::Subagent,
                    related_id: "agent-2".into(),
                    label: None,
                },
            ],
        )
        .unwrap();
    assert_eq!(store.relations(&key).unwrap().len(), 2);

    store
        .replace_relations(
            &key,
            RelationKind::Subagent,
            &[RelationRecord {
                kind: RelationKind::Subagent,
                related_id: "agent-2".into(),
                label: Some("Run the tests".into()),
            }],
        )
        .unwrap();
    let relations = store.relations(&key).unwrap();
    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0].related_id, "agent-2");
    assert_eq!(relations[0].label.as_deref(), Some("Run the tests"));
}

#[test]
fn deleting_a_session_takes_its_derived_records_with_it() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "abc");
    store.upsert_sessions(&[session("abc", 1_000)]).unwrap();
    store
        .save_analysis(&AnalysisRecord {
            key: key.clone(),
            metrics_json: "{}".into(),
            cost_json: None,
            model_breakdown_json: "{}".into(),
            active_secs: 1,
            duration_secs: 1,
            pattern_score: 50,
            source_fingerprint: "x".into(),
            pricing_generation: 0,
        })
        .unwrap();
    store
        .replace_relations(
            &key,
            RelationKind::Subagent,
            &[RelationRecord {
                kind: RelationKind::Subagent,
                related_id: "agent-1".into(),
                label: None,
            }],
        )
        .unwrap();

    assert!(store.delete_session(&key).unwrap());
    assert!(store.session(&key).unwrap().is_none());
    assert!(store.analysis(&key).unwrap().is_none());
    assert!(store.relations(&key).unwrap().is_empty());
    // Deleting again is a no-op rather than an error.
    assert!(!store.delete_session(&key).unwrap());
}

#[test]
fn pruning_drops_only_sessions_older_than_the_retention_horizon() {
    let store = store();
    store.upsert_sessions(&[session("old", 1_000)]).unwrap();
    store.upsert_sessions(&[session("new", 5_000)]).unwrap();

    assert_eq!(store.prune_sessions_before(2_000).unwrap(), 1);
    let remaining = store.recent_sessions(0, 100).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].key.session_id, "new");
}

#[test]
fn scan_roots_dedup_and_ignore_a_trailing_separator() {
    let store = store();
    store.add_scan_root("/home/avery/work/").unwrap();
    store.add_scan_root("/home/avery/work").unwrap();
    store.add_scan_root("").unwrap();
    assert_eq!(store.scan_roots().unwrap(), vec!["/home/avery/work"]);

    store.remove_scan_root("/home/avery/work/").unwrap();
    assert!(store.scan_roots().unwrap().is_empty());
    // Removing an absent root is a no-op.
    store.remove_scan_root("/home/avery/work").unwrap();
}

#[test]
fn a_rescan_refreshes_repositories_but_keeps_the_users_include_choice() {
    let store = store();
    let located = |session_count: u32| RepositoryRecord {
        key: "/home/avery/code/widgets".into(),
        repo_name: "widgets".into(),
        full_name: "avery/widgets".into(),
        status: "accessible".into(),
        repo_root: Some("/home/avery/code/widgets".into()),
        suspected_path: None,
        worktree_count: 1,
        session_count,
        wsl_distro: None,
        enabled: true,
    };

    store.replace_repositories(&[located(2)]).unwrap();
    assert!(
        store
            .set_repository_enabled("/home/avery/code/widgets", false)
            .unwrap()
    );

    store.replace_repositories(&[located(7)]).unwrap();
    let repositories = store.repositories().unwrap();
    assert_eq!(repositories.len(), 1);
    assert_eq!(repositories[0].session_count, 7, "facts refresh");
    assert!(
        !repositories[0].enabled,
        "the user's choice survives a rescan"
    );
}

#[test]
fn a_repository_the_scan_no_longer_sees_is_dropped() {
    let store = store();
    let record = |key: &str| RepositoryRecord {
        key: key.into(),
        repo_name: "widgets".into(),
        full_name: "avery/widgets".into(),
        status: "accessible".into(),
        repo_root: Some(key.into()),
        suspected_path: None,
        worktree_count: 1,
        session_count: 0,
        wsl_distro: None,
        enabled: true,
    };
    store
        .replace_repositories(&[record("/a/widgets"), record("/b/widgets")])
        .unwrap();
    assert_eq!(store.repositories().unwrap().len(), 2);

    store.replace_repositories(&[record("/a/widgets")]).unwrap();
    let repositories = store.repositories().unwrap();
    assert_eq!(repositories.len(), 1);
    assert_eq!(repositories[0].key, "/a/widgets");
}

#[test]
fn agent_scan_state_records_the_high_water_cursor() {
    let store = store();
    store
        .record_agent_scan("claude-code", Some(1_000), 4)
        .unwrap();
    store
        .record_agent_scan("claude-code", Some(500), 2)
        .unwrap();

    let state = store.scan_state().unwrap();
    assert_eq!(state.len(), 1);
    assert_eq!(state[0].0, "claude-code");
    assert_eq!(state[0].2, 2, "the latest pass's count wins");

    let cursor: i64 = store
        .lock()
        .query_row(
            "SELECT cursor_epoch FROM scan_state WHERE agent = 'claude-code'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cursor, 1_000, "the cursor never rewinds");
}

#[test]
fn usage_evidence_joins_the_analysis_and_keeps_sessions_that_have_none() {
    let store = store();
    store
        .upsert_sessions(&[session("analyzed", 2_000), session("pending", 1_500)])
        .unwrap();
    store
        .save_analysis(&AnalysisRecord {
            key: SessionKey::new("native", "claude-code", "analyzed"),
            metrics_json: "{}".into(),
            cost_json: None,
            model_breakdown_json: r#"{"claude-opus-4-6":{"input_tokens":10}}"#.into(),
            active_secs: 1,
            duration_secs: 1,
            pattern_score: 0,
            source_fingerprint: "1:1".into(),
            pricing_generation: 0,
        })
        .unwrap();

    let evidence = store.usage_evidence(1_000).unwrap();
    assert_eq!(evidence.len(), 2, "newest first");
    assert_eq!(evidence[0].updated_at_epoch, 2_000);
    assert!(
        evidence[0]
            .model_breakdown_json
            .as_deref()
            .is_some_and(|json| json.contains("claude-opus-4-6"))
    );
    // A session analysis has not reached yet comes back with no breakdown
    // rather than being dropped: "not measured" is not "measured zero".
    assert_eq!(evidence[1].updated_at_epoch, 1_500);
    assert_eq!(evidence[1].model_breakdown_json, None);
    assert_eq!(evidence[1].agent, "claude-code");

    // The bound is inclusive and excludes everything below it.
    assert_eq!(store.usage_evidence(2_000).unwrap().len(), 1);
    assert!(store.usage_evidence(2_001).unwrap().is_empty());
}
