use super::*;

fn source_guard(store: &Store, record: &SessionRecord, source: &str) -> SessionWriteGuard {
    store
        .begin_session_write_for_source_at(
            store.capture_destructive_write_epoch(),
            &record.key,
            &record.source_label,
            source,
        )
        .unwrap()
}

#[test]
fn unchanged_acceptance_is_once_only_and_binds_the_database_path() {
    let store = store();
    let mut record = session("unchanged-database", 1000);
    record.source_kind = "sqlite".into();
    store
        .upsert_sessions(std::slice::from_ref(&record), &[])
        .unwrap();
    let revision = store.revision();
    let guard = source_guard(&store, &record, "sqlite:/one/opencode.db");
    assert!(store.accept_session_if_current(&guard).unwrap());
    assert!(!store.accept_session_if_current(&guard).unwrap());
    assert!(matches!(
        store
            .upsert_session_if_current(&guard, &record, &[])
            .unwrap(),
        SessionCommitOutcome::Rejected(_)
    ));
    assert!(matches!(
        store.delete_session_if_current(&guard).unwrap(),
        SessionCommitOutcome::Rejected(_)
    ));
    assert_eq!(store.revision(), revision);

    let other_database = source_guard(&store, &record, "sqlite:/two/opencode.db");
    assert!(!store.accept_session_if_current(&other_database).unwrap());
    assert_eq!(
        store.delete_session_if_current(&other_database).unwrap(),
        SessionCommitOutcome::Rejected(SessionCommitRejection::SourceChanged)
    );
    let repeated = source_guard(&store, &record, "sqlite:/one/opencode.db");
    assert!(store.accept_session_if_current(&repeated).unwrap());
}

#[test]
fn opencode_format_switch_invalidates_the_old_source_generation_and_fingerprint() {
    let store = store();
    let mut record = session("format-switch", 1_000);
    record.key.agent = "opencode".into();
    record.source_kind = "providerDb".into();
    record.source_label = "opencode:format-switch".into();
    record.source_fingerprint = Some("same-fingerprint".into());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    let source_generation = || {
        store
            .lock()
            .query_row(
                "SELECT source_generation FROM session
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                params![
                    record.key.environment_key,
                    record.key.agent,
                    record.key.session_id
                ],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    };
    let v1_generation = source_generation();

    record.source_label = "opencode:format-switch:open_code_v2_sqlite".into();
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    let v2_generation = source_generation();
    assert!(v2_generation > v1_generation);
    assert_eq!(
        store
            .session(&record.key)
            .unwrap()
            .unwrap()
            .source_fingerprint,
        None
    );

    record.source_kind = "inline".into();
    record.source_label = "opencode-v2-export:format-switch".into();
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    let export_generation = source_generation();
    assert!(export_generation > v2_generation);
    assert_eq!(
        store
            .session(&record.key)
            .unwrap()
            .unwrap()
            .source_fingerprint,
        None
    );

    record.source_kind = "providerDb".into();
    record.source_label = "opencode:format-switch".into();
    record.source_fingerprint = Some("same-fingerprint".into());
    assert!(
        store
            .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
            .is_err()
    );
    assert_eq!(source_generation(), export_generation);
    assert_eq!(
        store
            .session(&record.key)
            .unwrap()
            .unwrap()
            .source_fingerprint,
        None
    );
}

#[test]
fn retention_preserves_physical_bindings_for_surviving_sessions() {
    const NOW: i64 = 2_000_000_000;
    let store = store();
    store
        .update_settings(|settings| {
            settings.session_data_retention_days = SESSION_DATA_RETENTION_DAYS_30;
        })
        .unwrap();
    let mut recent = session("retained-database", NOW);
    recent.source_kind = "sqlite".into();
    let expired = session("expired-unrelated", NOW - 31 * 86_400);
    for record in [&recent, &expired] {
        let guard = source_guard(&store, record, "sqlite:/one/opencode.db");
        assert!(matches!(
            store
                .upsert_session_if_current(&guard, record, &[])
                .unwrap(),
            SessionCommitOutcome::Upserted(_)
        ));
    }
    let pending = source_guard(&store, &recent, "sqlite:/one/opencode.db");
    let epoch = store.capture_destructive_write_epoch();

    assert_eq!(store.apply_session_retention(NOW).unwrap().0, 1);
    assert_ne!(store.capture_destructive_write_epoch(), epoch);
    assert_eq!(
        store.delete_session_if_current(&pending).unwrap(),
        SessionCommitOutcome::Rejected(SessionCommitRejection::GlobalFenceChanged)
    );
    assert!(store.session(&expired.key).unwrap().is_none());
    {
        let state = store.session_write_state.lock().unwrap();
        assert!(state.physical_sources.contains_key(&recent.key));
        assert!(!state.physical_sources.contains_key(&expired.key));
    }

    let other_database = source_guard(&store, &recent, "sqlite:/two/opencode.db");
    assert!(!store.accept_session_if_current(&other_database).unwrap());
    assert_eq!(
        store.delete_session_if_current(&other_database).unwrap(),
        SessionCommitOutcome::Rejected(SessionCommitRejection::SourceChanged)
    );
    assert_eq!(store.session(&recent.key).unwrap(), Some(recent.clone()));
    let original_database = source_guard(&store, &recent, "sqlite:/one/opencode.db");
    assert!(store.accept_session_if_current(&original_database).unwrap());
}

#[test]
fn physical_replacement_fences_old_rejection_and_indexed_title() {
    let store = store();
    let mut record = session("physical-replacement", 1000);
    record.source_kind = "sqlite".into();
    let first = source_guard(&store, &record, "sqlite:/one/opencode.db");
    store
        .upsert_session_if_current(&first, &record, &[])
        .unwrap();
    let title_guard = store
        .native_session_title_refresh_guards("claude-code")
        .unwrap()
        .pop()
        .unwrap();
    let old = source_guard(&store, &record, "sqlite:/one/opencode.db");
    let replacement = source_guard(&store, &record, "sqlite:/two/opencode.db");
    record.title = Some("Replacement title".into());
    assert!(matches!(
        store
            .upsert_session_if_current(&replacement, &record, &[])
            .unwrap(),
        SessionCommitOutcome::Upserted(_)
    ));
    assert!(matches!(
        store.delete_session_if_current(&old).unwrap(),
        SessionCommitOutcome::Rejected(_)
    ));
    assert!(!store.accept_session_if_current(&old).unwrap());
    assert!(
        !store
            .update_session_title_if_current(&title_guard, Some("Old title"), Some("explicit"))
            .unwrap()
    );
    assert_eq!(store.session(&record.key).unwrap(), Some(record));
}

#[test]
fn stale_discovery_does_not_supersede_a_current_dispatch() {
    let store = store();
    let record = session("stale-discovery", 1000);
    let epoch = store.capture_destructive_write_epoch();
    store
        .update_settings(|settings| settings.include_non_repo_folders = true)
        .unwrap();
    let current = source_guard(&store, &record, "file:/current");
    let stale = store
        .begin_session_write_for_source_at(epoch, &record.key, &record.source_label, "file:/old")
        .unwrap();
    assert!(!store.accept_session_if_current(&stale).unwrap());
    assert!(matches!(
        store
            .upsert_session_if_current(&current, &record, &[])
            .unwrap(),
        SessionCommitOutcome::Upserted(_)
    ));
}

#[test]
fn every_settings_transaction_path_fences_discovery_only_after_commit() {
    for path in 0..6 {
        let store = store();
        let record = session("settings-transaction", 1000);
        let epoch = store.capture_destructive_write_epoch();
        let guard = source_guard(&store, &record, "file:/session");
        let mut settings = store.settings().unwrap();
        settings.include_non_repo_folders = true;
        match path {
            0 => {
                store.replace_settings(&settings).unwrap();
            }
            1 => {
                store
                    .replace_settings_with(&settings, |_, _| Ok(()))
                    .unwrap();
            }
            2 => {
                store
                    .replace_settings_with_transition(&settings, |_, _, _| Ok(()))
                    .unwrap();
            }
            3 => {
                store
                    .replace_settings_preserving_interface_scale(&settings, |_, _, _| Ok(()))
                    .unwrap();
            }
            4 => {
                store
                    .update_settings(|saved| saved.include_non_repo_folders = true)
                    .unwrap();
            }
            _ => {
                store
                    .update_settings_with(|saved| {
                        saved.include_non_repo_folders = true;
                        Ok(())
                    })
                    .unwrap();
            }
        }
        assert_ne!(
            store.capture_destructive_write_epoch(),
            epoch,
            "path {path}"
        );
        assert!(!store.accept_session_if_current(&guard).unwrap());
        assert_eq!(
            store
                .upsert_session_if_current(&guard, &record, &[])
                .unwrap(),
            SessionCommitOutcome::Rejected(SessionCommitRejection::GlobalFenceChanged)
        );
        let epoch = store.capture_destructive_write_epoch();
        settings.include_non_repo_folders = false;
        let failed = store
            .replace_settings_with_transition::<()>(&settings, |_, _, _| anyhow::bail!("rollback"));
        assert!(failed.is_err());
        assert_eq!(store.capture_destructive_write_epoch(), epoch);
        assert!(store.settings_snapshot().include_non_repo_folders);
    }
}

#[test]
fn repository_count_updates_do_not_restore_removed_policy_rows() {
    let store = store();
    store
        .update_repository_session_counts(&[("deleted-repository".into(), 12)])
        .unwrap();
    assert!(store.repositories().unwrap().is_empty());
}

#[test]
fn failed_guarded_delete_preserves_binding_and_can_retry() {
    let store = store();
    let record = session("delete-retry", 1000);
    let first = source_guard(&store, &record, "file:/session");
    store
        .upsert_session_if_current(&first, &record, &[])
        .unwrap();
    let guard = source_guard(&store, &record, "file:/session");
    store
        .lock()
        .execute_batch(
            "CREATE TEMP TRIGGER reject_guarded_delete BEFORE DELETE ON session
         BEGIN SELECT RAISE(ABORT, 'delete failed'); END;",
        )
        .unwrap();
    assert!(store.delete_session_if_current(&guard).is_err());
    assert_eq!(store.session(&record.key).unwrap(), Some(record.clone()));
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_guarded_delete")
        .unwrap();
    assert!(matches!(
        store.delete_session_if_current(&guard).unwrap(),
        SessionCommitOutcome::Deleted(Some(_))
    ));
    assert!(store.session(&record.key).unwrap().is_none());
    assert!(
        !store
            .session_write_state
            .lock()
            .unwrap()
            .physical_sources
            .contains_key(&record.key)
    );
}

#[test]
fn unchanged_acceptance_rejects_a_changed_incarnation() {
    let store = store();
    let record = session("accept-incarnation", 1000);
    let absent = source_guard(&store, &record, "file:/session");
    assert!(!store.accept_session_if_current(&absent).unwrap());
    store
        .upsert_sessions(std::slice::from_ref(&record), &[])
        .unwrap();
    let guard = source_guard(&store, &record, "file:/session");
    store
        .lock()
        .execute("UPDATE session SET incarnation = incarnation + 1", [])
        .unwrap();
    assert!(!store.accept_session_if_current(&guard).unwrap());
    assert_eq!(
        store.delete_session_if_current(&guard).unwrap(),
        SessionCommitOutcome::Rejected(SessionCommitRejection::SourceChanged)
    );
}

#[test]
fn local_exclusion_keeps_remote_sessions_with_the_same_working_directory() {
    let store = store();
    let local = session("shared-path", 1000);
    let mut remote = local.clone();
    remote.key = SessionKey::new("ssh:host", "claude-code", "shared-path");
    store
        .upsert_sessions(&[local.clone(), remote.clone()], &[])
        .unwrap();
    store
        .replace_repositories(&[RepositoryRecord {
            key: "widgets".into(),
            repo_name: "widgets".into(),
            full_name: "avery/widgets".into(),
            status: "accessible".into(),
            repo_root: local.cwd.clone(),
            suspected_path: None,
            worktree_count: 1,
            session_count: 1,
            wsl_distro: None,
            enabled: false,
        }])
        .unwrap();
    assert_eq!(store.purge_ignored_sessions().unwrap(), 1);
    assert!(store.session(&local.key).unwrap().is_none());
    assert_eq!(store.session(&remote.key).unwrap(), Some(remote));
}

#[test]
fn failed_remote_host_delete_keeps_rows_and_source_bindings() {
    let store = store();
    let mut first = session("remote-one", 1000);
    first.key = SessionKey::new("ssh:host", "claude-code", "remote-one");
    let mut second = first.clone();
    second.key.session_id = "remote-two".into();
    for record in [&first, &second] {
        let guard = source_guard(&store, record, "file:/remote/session");
        store
            .upsert_session_if_current(&guard, record, &[])
            .unwrap();
    }
    let pending = source_guard(&store, &first, "file:/remote/session");
    store
        .lock()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_second_remote_delete BEFORE DELETE ON session
         WHEN OLD.session_id = 'remote-two'
         BEGIN SELECT RAISE(ABORT, 'remote delete failed'); END;",
        )
        .unwrap();
    assert!(store.delete_remote_host("host").is_err());
    assert_eq!(store.session_count().unwrap(), 2);
    assert_eq!(
        store
            .session_write_state
            .lock()
            .unwrap()
            .physical_sources
            .len(),
        2
    );
    assert!(store.accept_session_if_current(&pending).unwrap());
}
