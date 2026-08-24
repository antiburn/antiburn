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
        activity_cursor: String::new(),
        activity_source: "mtime".into(),
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
fn session_analysis_keeps_only_the_cache_values_the_app_reads() {
    let store = store();
    let connection = store.lock();
    let mut statement = connection
        .prepare("SELECT name FROM pragma_table_info('session_analysis')")
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
            "model_breakdown_json",
            "inclusive_models_json",
            "source_fingerprint",
            "pricing_generation",
        ]
    );
}

/// Opting out is a withdrawal, not a pause: nothing queued survives it, and
/// neither does the identifier that would let a later opt-in be joined to it.
#[test]
fn opting_out_destroys_the_queue_and_the_installation_identity() {
    let store = store();
    store
        .set_analytics_identity("11111111-1111-4111-8111-111111111111")
        .unwrap();
    store.queue_analytics_event("app_launched", "{}").unwrap();
    assert_eq!(store.pending_analytics_events(10).unwrap().len(), 1);
    assert!(store.analytics_identity().unwrap().is_some());

    store.clear_analytics().unwrap();

    assert!(store.pending_analytics_events(10).unwrap().is_empty());
    assert!(store.analytics_identity().unwrap().is_none());
}

/// An undeliverable event is dropped rather than retried forever: a queue that
/// grows without bound on a machine that is offline for a week is a
/// disk-space bug, not a feature.
#[test]
fn events_are_given_up_on_after_a_bounded_number_of_attempts() {
    let store = store();
    store.queue_analytics_event("app_launched", "{}").unwrap();
    let ids: Vec<i64> = store
        .pending_analytics_events(10)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();

    for _ in 0..2 {
        assert_eq!(store.fail_analytics_events(&ids, 3).unwrap(), 0);
        assert_eq!(store.pending_analytics_events(10).unwrap().len(), 1);
    }
    assert_eq!(store.fail_analytics_events(&ids, 3).unwrap(), 1);
    assert!(store.pending_analytics_events(10).unwrap().is_empty());
}

#[test]
fn consent_grants_round_trip_and_revoke_individually() {
    let store = store();
    assert!(store.granted_dirs().unwrap().is_empty());

    store.grant_dir("Documents").unwrap();
    store.grant_dir("Desktop").unwrap();
    assert_eq!(store.granted_dirs().unwrap().len(), 2);

    // Re-granting refreshes the row rather than adding a second one.
    store.grant_dir("Documents").unwrap();
    assert_eq!(store.granted_dirs().unwrap().len(), 2);

    // Revoking is per directory, and revoking an absent one is a no-op.
    store.revoke_dir_grant("Documents").unwrap();
    store.revoke_dir_grant("Downloads").unwrap();
    assert_eq!(
        store.granted_dirs().unwrap(),
        std::collections::HashSet::from(["Desktop".to_string()])
    );
}

#[test]
fn deferred_permission_dirs_replace_the_previous_pass() {
    let store = store();

    store
        .set_deferred_permission_dirs(&[crate::dto::DeferredPermissionDir {
            dir: "Documents".to_string(),
            path_count: 3,
        }])
        .unwrap();
    assert_eq!(
        store.internal_value(super::DEFERRED_PERMISSION_DIRS_KEY),
        Some(r#"[{"dir":"Documents","pathCount":3}]"#.to_string())
    );

    // A later pass that defers nothing clears the list rather than leaving a
    // stale directory asking for permission it no longer needs.
    store.set_deferred_permission_dirs(&[]).unwrap();
    assert_eq!(
        store.internal_value(super::DEFERRED_PERMISSION_DIRS_KEY),
        Some("[]".to_string())
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
    assert!(defaults.launch_at_login);
    // On by default: fetching the reader's own usage from a provider they
    // already use, with a credential they already hold, is ordinary traffic,
    // not something that needs a first-run choice. See `live_usage_active`
    // for the onboarding gate that still applies regardless of this default.
    assert!(defaults.live_usage_enabled);
    // Open by default, same reasoning: a reader who has limits to see should
    // see them without an extra click the first time they notice the section.
    assert!(defaults.overview_limits_expanded);

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
            nudge_placement: NudgePlacement::TopRight,
            nudge_auto_dismiss_secs: 25,
            notification_sound: false,
            disk_space_display: DiskSpaceDisplay::Always,
            disk_space_threshold_gb: 100,
            notify_disk_space_low: false,
            notify_usage_anomalies: false,
            milestones_5h: Milestones {
                at50: false,
                at75: true,
                at90: true,
            },
            milestones_weekly: Milestones {
                at50: false,
                at75: false,
                at90: false,
            },
            live_usage_enabled: true,
            analytics_enabled: false,
            overview_limits_expanded: false,
        })
        .unwrap();
    assert_eq!(store.settings().unwrap(), saved);
    assert_eq!(saved.theme, ThemePreference::Dark);
    assert_eq!(saved.activity_window_days, 14);
    assert_eq!(saved.nudge_placement, NudgePlacement::TopRight);
    assert_eq!(saved.nudge_auto_dismiss_secs, 25);
    assert_eq!(saved.disk_space_display, DiskSpaceDisplay::Always);
    assert_eq!(saved.disk_space_threshold_gb, 100);
    // The empty milestone subset survives a round trip as "none selected",
    // not as a reset back to the defaults.
    assert!(!saved.milestones_weekly.any());
    assert!(saved.milestones_5h.at75 && !saved.milestones_5h.at50);
    assert!(saved.live_usage_enabled);
    assert!(!saved.overview_limits_expanded);
    assert!(saved.onboarding_completed);
    assert!(saved.discovery_paused);
    // Each notification preference is stored on its own key, so a reader who
    // silences the master switch keeps the per-kind choices they made.
    assert!(!saved.notifications_enabled);
    assert!(!saved.notify_update_available);
    assert!(saved.notify_scan_failure);
}

#[test]
fn an_explicit_launch_at_login_opt_out_overrides_the_default() {
    let store = store();
    let saved = store
        .save_settings(&AppSettings {
            onboarding_completed: true,
            launch_at_login: false,
            ..AppSettings::default()
        })
        .unwrap();

    assert!(!saved.launch_at_login);
    assert!(!store.settings().unwrap().launch_at_login);
}

#[test]
fn updating_settings_merges_against_the_latest_stored_value() {
    let store = store();
    store
        .save_settings(&AppSettings {
            theme: ThemePreference::Dark,
            auto_update: false,
            ..AppSettings::default()
        })
        .unwrap();

    let (previous, saved) = store
        .update_settings(|settings| {
            settings.activity_window_days = 14;
            settings.launch_at_login = false;
            settings.onboarding_completed = true;
        })
        .unwrap();

    assert_eq!(previous.theme, ThemePreference::Dark);
    assert!(!previous.auto_update);
    assert_eq!(saved.theme, ThemePreference::Dark);
    assert!(!saved.auto_update);
    assert_eq!(saved.activity_window_days, 14);
    assert!(!saved.launch_at_login);
    assert!(saved.onboarding_completed);
    assert_eq!(store.settings().unwrap(), saved);
}

/// Pin the current session shape so migrations remain deliberate. This is not
/// a restriction on what a future local visibility feature may store.
#[test]
fn the_session_table_shape_is_stable() {
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
            "title",
            "title_source",
            "cwd",
            "surface",
            "updated_at_epoch",
            "subagent_count",
            "first_seen_at",
            "last_seen_at",
            "activity_source",
            "activity_cursor",
        ]
    );
}

#[test]
fn clearing_local_data_forgets_session_records_and_keeps_the_readers_choices() {
    let store = store();
    store.upsert_sessions(&[session("abc", 2_000)]).unwrap();
    store
        .save_analysis(&AnalysisRecord {
            key: SessionKey::new("native", "claude-code", "abc"),
            model_breakdown_json: "{}".into(),
            inclusive_models_json: "[]".into(),
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

    assert_eq!(store.clear_local_session_data().unwrap(), 1);

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
    assert_eq!(store.clear_local_session_data().unwrap(), 0);
    assert_eq!(store.clear_local_session_data().unwrap(), 0);
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
    // it, and must not rewind the activity timestamp.
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
fn an_event_timestamp_survives_a_newer_mtime_only_upsert() {
    let store = store();
    let mut event = session("semantic", 1_000);
    event.activity_cursor = "[\"parent\",10]".into();
    event.activity_source = "event".into();
    store.upsert_sessions(&[event]).unwrap();

    let mut mtime = session("semantic", 2_000);
    mtime.activity_cursor = "[\"parent\",20]".into();
    mtime.activity_source = "mtime".into();
    store.upsert_sessions(&[mtime]).unwrap();

    let stored = store
        .session(&SessionKey::new("native", "claude-code", "semantic"))
        .unwrap()
        .expect("session");
    assert_eq!(stored.updated_at_epoch, Some(1_000));
    assert_eq!(stored.activity_source, "event");
    assert_eq!(stored.activity_cursor, "[\"parent\",20]");
}

#[test]
fn activity_cursors_do_not_collide_across_environments() {
    let store = store();
    let native = session("shared", 1_000);
    let mut wsl = native.clone();
    wsl.key = SessionKey::new("wsl:ubuntu", "claude-code", "shared");
    store.upsert_sessions(&[native, wsl]).unwrap();

    let states = store.session_activity_states().unwrap();
    assert_eq!(states.len(), 2);
    assert!(states.contains_key(&SessionActivityKey::new(
        "native",
        "claude-code",
        "/home/avery/.claude/projects/demo/shared.jsonl",
    )));
    assert!(states.contains_key(&SessionActivityKey::new(
        "wsl:ubuntu",
        "claude-code",
        "/home/avery/.claude/projects/demo/shared.jsonl",
    )));
}

#[test]
fn title_and_source_are_replaced_or_preserved_as_one_pair() {
    let store = store();

    let mut source_without_title = session("orphan-source", 500);
    source_without_title.title = None;
    source_without_title.title_source = Some("firstMessage".into());
    store.upsert_sessions(&[source_without_title]).unwrap();
    let stored = store
        .session(&SessionKey::new("native", "claude-code", "orphan-source"))
        .unwrap()
        .expect("session");
    assert_eq!((stored.title, stored.title_source), (None, None));

    // Heal rows written by the older independent-COALESCE upsert, where a
    // sanitized-away title could leave provenance behind.
    store
        .lock()
        .execute(
            "UPDATE session SET title_source = 'firstMessage'\
             WHERE environment_key = 'native' AND agent = 'claude-code'\
               AND session_id = 'orphan-source'",
            [],
        )
        .unwrap();
    let mut no_title_again = session("orphan-source", 750);
    no_title_again.title = None;
    no_title_again.title_source = None;
    store.upsert_sessions(&[no_title_again]).unwrap();
    let healed = store
        .session(&SessionKey::new("native", "claude-code", "orphan-source"))
        .unwrap()
        .expect("session");
    assert_eq!((healed.title, healed.title_source), (None, None));

    store.upsert_sessions(&[session("abc", 1_000)]).unwrap();

    let mut renamed = session("abc", 2_000);
    renamed.title = Some("Reader supplied title".into());
    renamed.title_source = Some("userRename".into());
    store.upsert_sessions(&[renamed]).unwrap();

    let mut missing = session("abc", 3_000);
    missing.title = None;
    missing.title_source = Some("firstMessage".into());
    store.upsert_sessions(&[missing]).unwrap();

    let stored = store
        .session(&SessionKey::new("native", "claude-code", "abc"))
        .unwrap()
        .expect("session");
    assert_eq!(stored.title.as_deref(), Some("Reader supplied title"));
    assert_eq!(stored.title_source.as_deref(), Some("userRename"));
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

    // A later scan can carry no observation. It must keep the recorded lineage.
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
        model_breakdown_json: r#"{"claude-opus-4-6":{"inputTokens":10}}"#.into(),
        inclusive_models_json:
            r#"[{"model":"claude-haiku-4-5"},{"model":"claude-opus-4-6","thinkingMode":"high"}]"#
                .into(),
        source_fingerprint: "1700000000:4096".into(),
        pricing_generation: 0,
    };
    store.save_analysis(&record).unwrap();
    assert_eq!(store.analysis(&key).unwrap().as_ref(), Some(&record));

    let updated = AnalysisRecord {
        source_fingerprint: "1700000900:8192".into(),
        ..record
    };
    store.save_analysis(&updated).unwrap();
    let stored = store.analysis(&key).unwrap().expect("analysis");
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
            model_breakdown_json: "{}".into(),
            inclusive_models_json: "[]".into(),
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
    assert!(!store.delete_session(&key).unwrap());
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
            model_breakdown_json: r#"{"claude-opus-4-6":{"input_tokens":10}}"#.into(),
            inclusive_models_json: r#"[{"model":"claude-opus-4-6","thinkingMode":"high"}]"#.into(),
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

#[test]
fn live_usage_is_only_active_once_both_the_switch_and_onboarding_agree() {
    // The switch defaults on, but that alone must never be enough: the
    // credential read this feature depends on — and, on macOS, the Keychain
    // prompt it can trigger — must wait for onboarding to finish.
    let mut settings = AppSettings::default();
    assert!(settings.live_usage_enabled, "the default is on");
    assert!(!settings.onboarding_completed, "the default is not");
    assert!(!settings.live_usage_active());

    settings.onboarding_completed = true;
    assert!(settings.live_usage_active());

    settings.live_usage_enabled = false;
    assert!(!settings.live_usage_active(), "the opt-out still works");
}

#[test]
fn migrating_forward_drops_a_legacy_live_usage_off_row_so_the_new_default_applies() {
    // Before this build, `liveUsageEnabled` defaulted to false, and
    // `write_settings` writes every key on every save regardless of whether
    // it changed — so any install that ever saved settings at all (finishing
    // onboarding is enough) already carries an explicit `liveUsageEnabled|
    // false` row from that old default, indistinguishable from a reader who
    // deliberately opted out. antiburn has no public installs yet to protect
    // from losing one, so migration V3 just drops the row. Simulated here by
    // building a v2 database by hand — a real fresh `Store::open_in_memory`
    // would already be at the latest version and could not exercise the
    // migration path at all.
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..2] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute(
            "INSERT INTO setting (key, value) VALUES ('liveUsageEnabled', 'false')",
            [],
        )
        .unwrap();
    connection
        .pragma_update(None, "user_version", 2i64)
        .unwrap();

    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-migration-test").to_path_buf(),
    )
    .expect("migrates cleanly to the latest version");

    assert_eq!(
        store.schema_version().unwrap(),
        super::schema::MIGRATIONS.len() as i64
    );
    let remaining: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM setting WHERE key = 'liveUsageEnabled'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0, "the row is gone, not merely reinterpreted");
    assert!(
        store.settings().unwrap().live_usage_enabled,
        "with the legacy row gone, the read path falls through to the new default"
    );
}

#[test]
fn migrating_forward_renames_the_analytics_tables_and_keeps_their_rows() {
    // V1 through V8 created and used `usage_analytics_event` and
    // `usage_analytics_identity`. V9 renames both tables to drop the
    // "usage_" prefix, to match the renamed Rust module and code. Built by
    // hand up to V8 so the rename migration actually runs; a fresh
    // `Store::open_in_memory` would already be past it.
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..8] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute(
            "INSERT INTO usage_analytics_identity (id, install_id, minted_at)
             VALUES (1, 'test-install-id', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO usage_analytics_event (name, payload, queued_at)
             VALUES ('app_launched', '{}', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    connection
        .pragma_update(None, "user_version", 8i64)
        .unwrap();

    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-migration-test").to_path_buf(),
    )
    .expect("migrates cleanly to the latest version");

    assert_eq!(
        store.schema_version().unwrap(),
        super::schema::MIGRATIONS.len() as i64
    );
    let (install_id, event_count): (String, i64) = store
        .lock()
        .query_row(
            "SELECT install_id, (SELECT COUNT(*) FROM analytics_event) FROM analytics_identity",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the renamed tables carry the rows the old names held");
    assert_eq!(install_id, "test-install-id");
    assert_eq!(event_count, 1);
}

#[test]
fn internal_values_round_trip_and_stay_out_of_settings() {
    let store = store();
    assert_eq!(store.internal_value("internal:diskSpaceLowFiredMs"), None);

    store.set_internal_value("internal:diskSpaceLowFiredMs", "1723600000000");
    assert_eq!(
        store
            .internal_value("internal:diskSpaceLowFiredMs")
            .as_deref(),
        Some("1723600000000")
    );

    // Overwrite, not append: one row per key, like every setting.
    store.set_internal_value("internal:diskSpaceLowFiredMs", "1723600001000");
    assert_eq!(
        store
            .internal_value("internal:diskSpaceLowFiredMs")
            .as_deref(),
        Some("1723600001000")
    );

    // Internal rows share the table but never surface as preferences.
    assert_eq!(store.settings().unwrap(), AppSettings::default());
}
