use super::*;

#[test]
#[cfg(feature = "analytics")]
fn settings_and_opt_out_signal_can_commit_as_one_durable_transition() {
    let store = store();
    store
        .set_analytics_identity("11111111-1111-4111-8111-111111111111")
        .unwrap();
    let mut disabled = store.settings().unwrap();
    disabled.analytics_enabled = false;

    let (previous, saved, ()) = store
        .replace_settings_with_transition(&disabled, |transaction, previous, saved| {
            assert!(previous.analytics_enabled);
            assert!(!saved.analytics_enabled);
            Store::queue_analytics_event_in(
                transaction,
                "antiburn.analytics_opted_out",
                "{\"anonymousId\":\"11111111-1111-4111-8111-111111111111\"}",
            )
        })
        .unwrap();

    assert!(previous.analytics_enabled);
    assert!(!saved.analytics_enabled);
    assert!(!store.settings().unwrap().analytics_enabled);
    assert!(store.analytics_opt_out_pending().unwrap());
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
        store.internal_value(crate::store::DEFERRED_PERMISSION_DIRS_KEY),
        Some(r#"[{"dir":"Documents","pathCount":3}]"#.to_string())
    );

    // A later pass that defers nothing clears the list rather than leaving a
    // stale directory asking for permission it no longer needs.
    store.set_deferred_permission_dirs(&[]).unwrap();
    assert_eq!(
        store.internal_value(crate::store::DEFERRED_PERMISSION_DIRS_KEY),
        Some("[]".to_string())
    );
}

#[test]
fn settings_default_before_anything_is_written_and_round_trip_after() {
    let store = store();
    let defaults = store.settings().unwrap();
    assert_eq!(defaults, AppSettings::default());
    assert!(!defaults.onboarding_completed);
    assert!(defaults.launch_at_login);
    assert!(defaults.tray_icon_visible);
    assert!(defaults.dock_icon_visible);
    // On by default: fetching the reader's own usage from a provider they
    // already use, with a credential they already hold, is ordinary traffic,
    // not something that needs a first-run choice. See `live_usage_active`
    // for the onboarding gate that still applies regardless of this default.
    assert!(defaults.live_usage_enabled);
    // Analytics starts automatically only after onboarding completes.
    assert!(defaults.analytics_enabled);
    // Open by default, same reasoning: a reader who has limits to see should
    // see them without an extra click the first time they notice the section.
    assert!(defaults.overview_limits_expanded);
    // Closed by default, unlike the limits section: the skills table is a long
    // tail behind a summary, and opening it every time buries the rest.
    assert!(!defaults.skills_mcp_expanded);
    // Unfiltered by default: a fresh install shows every loaded session.
    assert_eq!(defaults.session_filter, "all");
    assert_eq!(
        defaults.session_data_retention_days,
        RETAIN_SESSION_DATA_FOREVER
    );

    // Notifications default on, both kinds with them, so the two per-kind
    // preferences below are a real change rather than a re-statement.
    assert!(defaults.notifications_enabled);
    assert!(defaults.notify_update_available);
    assert!(defaults.notify_scan_failure);
    // Off by default: the Focus-status check needs its own macOS
    // authorization prompt, so the reader opts in first.
    assert!(!defaults.nudges_respect_dnd);

    let saved = store
        .save_settings(&AppSettings {
            theme: ThemePreference::Dark,
            interface_scale_percent: 125,
            activity_window_days: 14,
            session_data_retention_days: SESSION_DATA_RETENTION_DAYS_90,
            onboarding_completed: true,
            launch_at_login: true,
            tray_icon_visible: false,
            dock_icon_visible: true,
            auto_update: false,
            discovery_paused: true,
            notifications_enabled: false,
            notify_update_available: false,
            notify_scan_failure: true,
            nudge_placement: NudgePlacement::TopRight,
            nudge_auto_dismiss_secs: 25,
            notification_sound: false,
            nudges_respect_dnd: true,
            disk_space_display: DiskSpaceDisplay::Always,
            disk_space_threshold_gb: 100,
            notify_disk_space_low: false,
            milestones_5h: Milestones::selected([75, 90]),
            milestones_weekly: Milestones::none(),
            live_usage_enabled: true,
            live_usage_hidden_providers: HiddenMeters::default(),
            disabled_agents: DisabledAgents::parse("windsurf,kiro"),
            analytics_enabled: false,
            overview_limits_expanded: false,
            skills_mcp_expanded: true,
            session_badge_metric: SessionBadgeMetric::WeeklyPercent,
            session_filter: "agent:codex".to_string(),
            working_week: WorkingWeek::Five,
        })
        .unwrap();
    assert_eq!(store.settings().unwrap(), saved);
    assert_eq!(saved.theme, ThemePreference::Dark);
    assert_eq!(saved.interface_scale_percent, 125);
    assert_eq!(saved.activity_window_days, 14);
    assert!(!saved.tray_icon_visible);
    assert!(saved.dock_icon_visible);
    assert_eq!(
        saved.session_data_retention_days,
        SESSION_DATA_RETENTION_DAYS_90
    );
    assert_eq!(saved.nudge_placement, NudgePlacement::TopRight);
    assert_eq!(saved.nudge_auto_dismiss_secs, 25);
    assert_eq!(saved.disk_space_display, DiskSpaceDisplay::Always);
    assert_eq!(saved.disk_space_threshold_gb, 100);
    // Stored and returned verbatim; this side does not validate the id.
    assert_eq!(saved.session_filter, "agent:codex");
    assert_eq!(saved.working_week, WorkingWeek::Five);
    let versioned =
        r#"v1:{"agents":["claude-code","codex"],"result":"failing","spend":"material"}"#;
    store
        .save_settings(&AppSettings {
            session_filter: versioned.to_string(),
            ..saved.clone()
        })
        .unwrap();
    assert_eq!(store.settings().unwrap().session_filter, versioned);
    // The empty milestone subset survives a round trip as "none selected",
    // not as a reset back to the defaults.
    assert!(!saved.milestones_weekly.any());
    assert!(saved.milestones_5h.contains(75) && !saved.milestones_5h.contains(50));
    assert!(saved.live_usage_enabled);
    // The disabled-agent set survives normalized to sorted slugs.
    assert_eq!(saved.disabled_agents.as_str(), "kiro,windsurf");
    assert!(!saved.overview_limits_expanded);
    // The two display preferences keep separate answers: this reader closed
    // the limits section and opened the skills table.
    assert!(saved.skills_mcp_expanded);
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
fn settings_restore_the_dock_when_both_presence_icons_are_hidden() {
    let store = store();
    let saved = store
        .save_settings(&AppSettings {
            tray_icon_visible: false,
            dock_icon_visible: false,
            ..AppSettings::default()
        })
        .unwrap();

    assert!(!saved.tray_icon_visible);
    assert!(saved.dock_icon_visible);
    assert_eq!(store.settings().unwrap(), saved);
}

#[test]
fn settings_snapshot_does_not_wait_for_the_database_connection() {
    let store = store();
    let connection = store.lock();
    let snapshot_store = store.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        sender.send(snapshot_store.settings_snapshot()).unwrap();
    });

    let snapshot = receiver
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("the settings snapshot stays independent from the database connection");
    drop(connection);
    reader.join().unwrap();

    assert_eq!(snapshot, AppSettings::default());
}

#[test]
fn settings_snapshot_tracks_commits_across_store_clones() {
    let store = store();
    let snapshot_store = store.clone();
    let saved = store
        .save_settings(&AppSettings {
            theme: ThemePreference::Dark,
            onboarding_completed: true,
            ..AppSettings::default()
        })
        .unwrap();

    assert_eq!(snapshot_store.settings_snapshot(), saved);
}

#[test]
fn settings_snapshot_ignores_a_rolled_back_transition() {
    let store = store();
    let before = store.settings_snapshot();
    let result: anyhow::Result<(AppSettings, AppSettings, ())> = store
        .replace_settings_with_transition(
            &AppSettings {
                theme: ThemePreference::Dark,
                ..AppSettings::default()
            },
            |_, _, _| anyhow::bail!("rollback"),
        );

    assert!(result.is_err());
    assert_eq!(store.settings_snapshot(), before);
}

#[test]
fn settings_snapshot_tracks_scale_and_preserving_writes() {
    let store = store();
    let snapshot_store = store.clone();
    let (_, scaled, ()) = store
        .update_settings_with(|settings| {
            settings.interface_scale_percent = 175;
            Ok(())
        })
        .unwrap();
    assert_eq!(snapshot_store.settings_snapshot(), scaled);

    let (_, saved, ()) = store
        .replace_settings_preserving_interface_scale(
            &AppSettings {
                theme: ThemePreference::Dark,
                interface_scale_percent: 100,
                ..AppSettings::default()
            },
            |_, _, _| Ok(()),
        )
        .unwrap();
    assert_eq!(saved.interface_scale_percent, 175);
    assert_eq!(saved.theme, ThemePreference::Dark);
    assert_eq!(snapshot_store.settings_snapshot(), saved);
    assert_eq!(store.settings().unwrap(), saved);
}

#[test]
fn settings_snapshot_ignores_failed_scale_preserving_write() {
    let store = store();
    let before = store.settings_snapshot();
    let result: anyhow::Result<(AppSettings, AppSettings, ())> = store
        .replace_settings_preserving_interface_scale(
            &AppSettings {
                theme: ThemePreference::Dark,
                ..AppSettings::default()
            },
            |_, _, _| anyhow::bail!("rollback"),
        );
    assert!(result.is_err());
    assert_eq!(store.settings_snapshot(), before);
    assert_eq!(store.settings().unwrap(), before);
}

#[test]
fn settings_repair_malformed_stored_presence_values() {
    let store = store();
    {
        let connection = store.lock();
        connection
            .execute(
                "INSERT INTO setting (key, value) VALUES (?1, ?2)",
                params!["trayIconVisible", "false"],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO setting (key, value) VALUES (?1, ?2)",
                params!["dockIconVisible", "false"],
            )
            .unwrap();
    }

    let settings = store.settings().unwrap();

    assert!(!settings.tray_icon_visible);
    assert!(settings.dock_icon_visible);
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

#[test]
fn restarting_onboarding_preserves_local_state_and_is_idempotent() {
    let store = store();
    let before = store
        .save_settings(&AppSettings {
            theme: ThemePreference::Dark,
            activity_window_days: 14,
            onboarding_completed: true,
            launch_at_login: false,
            analytics_enabled: false,
            ..AppSettings::default()
        })
        .unwrap();
    store
        .upsert_sessions(&[session("abc", 2_000)], &crate::agents::evidence_cohort())
        .unwrap();
    store.add_scan_root("/home/avery/work").unwrap();
    store.queue_analytics_event("app_launched", "{}").unwrap();

    let (previous, restarted) = store.restart_onboarding().unwrap();

    let mut expected = before.clone();
    expected.onboarding_completed = false;
    assert_eq!(previous, before);
    assert_eq!(restarted, expected);
    assert_eq!(store.settings().unwrap(), expected);
    assert_eq!(store.session_count().unwrap(), 1);
    assert_eq!(store.scan_roots().unwrap(), vec!["/home/avery/work"]);
    assert_eq!(store.pending_analytics_events(10).unwrap().len(), 1);
    assert!(store.onboarding_flow_is_restart());

    let (previous_again, restarted_again) = store.restart_onboarding().unwrap();
    assert_eq!(previous_again, expected);
    assert_eq!(restarted_again, expected);
}

#[test]
fn a_restarted_onboarding_flow_keeps_its_classification_after_relaunch() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store
        .update_settings(|settings| settings.onboarding_completed = true)
        .unwrap();
    store.restart_onboarding().unwrap();
    drop(store);

    let reopened = Store::open(directory.path()).unwrap();
    assert!(!reopened.settings().unwrap().onboarding_completed);
    assert!(reopened.onboarding_flow_is_restart());
}
