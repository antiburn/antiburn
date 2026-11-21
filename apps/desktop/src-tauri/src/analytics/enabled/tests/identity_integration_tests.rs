use super::*;

const EXISTING_ID: &str = "11111111-1111-4111-8111-111111111111";

#[test]
fn existing_identity_survives_age_and_unreadable_timestamps() {
    let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
    let now = time::OffsetDateTime::now_utc();
    let mut stamps = [29, 30, 31, 365]
        .map(|days| {
            (now - time::Duration::days(days))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()
        })
        .to_vec();
    stamps.extend(["not a timestamp".to_string(), String::new()]);

    for stamp in stamps {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store
            .set_analytics_identity_at(EXISTING_ID, &stamp)
            .unwrap();
        reset_session();
        let session = current_session_id(true);

        assert_eq!(
            current_identity_pair(&store, true),
            Some((EXISTING_ID.into(), session.clone())),
            "existing identity and session must survive mint stamp {stamp:?}"
        );
        assert_eq!(
            store.analytics_identity().unwrap(),
            Some((EXISTING_ID.into(), stamp.clone()))
        );
        drop(store);

        reset_session();
        let reopened = Store::open(directory.path()).unwrap();
        let (install, restarted_session) = current_identity_pair(&reopened, true).unwrap();
        assert_eq!(install, EXISTING_ID);
        assert_ne!(restarted_session, session);
        assert_eq!(
            reopened.analytics_identity().unwrap(),
            Some((EXISTING_ID.into(), stamp))
        );
    }
    reset_session();
}

#[test]
fn identity_reuse_preserves_queue_and_settings_across_reopen() {
    let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let settings = AppSettings {
        notifications_enabled: false,
        activity_window_days: 7,
        ..store.settings().unwrap()
    };
    store.replace_settings(&settings).unwrap();
    store
        .set_analytics_identity_at(EXISTING_ID, "2000-01-01T00:00:00Z")
        .unwrap();
    let payload = r#"{"anonymousId":"previous-identity","sessionId":"previous-session","messageId":"original-message","originalTimestamp":"2026-01-01T00:00:00Z"}"#;
    store
        .queue_analytics_event("antiburn.app_launched", payload)
        .unwrap();
    let pending = store.pending_analytics_events(10).unwrap();
    store.fail_analytics_events(&[pending[0].0], 5).unwrap();
    let queued_at: String = store
        .lock()
        .query_row("SELECT queued_at FROM analytics_event", [], |row| {
            row.get(0)
        })
        .unwrap();
    reset_session();
    assert_eq!(current_install_id(&store).as_deref(), Some(EXISTING_ID));
    drop(store);

    let reopened = Store::open(directory.path()).unwrap();
    assert_eq!(current_install_id(&reopened).as_deref(), Some(EXISTING_ID));
    assert_eq!(reopened.settings().unwrap(), settings);
    assert_eq!(reopened.pending_analytics_events(10).unwrap(), pending);
    let queue_state: (String, String, u32) = reopened
        .lock()
        .query_row(
            "SELECT name, queued_at, attempts FROM analytics_event",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(queue_state, ("antiburn.app_launched".into(), queued_at, 1));
    reset_session();
}

#[test]
fn reenable_after_opt_out_creates_a_fresh_identity_and_session() {
    let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store
        .set_analytics_identity_at(EXISTING_ID, "2000-01-01T00:00:00Z")
        .unwrap();
    reset_session();
    let previous = current_identity_pair(&store, true).unwrap();
    store
        .queue_analytics_event("antiburn.app_launched", "{}")
        .unwrap();

    let enabled = store.settings().unwrap();
    let mut disabled = enabled.clone();
    disabled.analytics_enabled = false;
    store
        .replace_settings_with_transition(&disabled, |tx, previous, saved| {
            prepare_opt_out_payload(tx, previous, saved, "antiburn:test")
        })
        .unwrap();
    assert!(store.analytics_opt_out_pending().unwrap());
    store
        .replace_settings_preserving_interface_scale(&enabled, |tx, previous, saved| {
            prepare_opt_out_payload(tx, previous, saved, "antiburn:test")
        })
        .unwrap();
    assert!(!store.analytics_opt_out_pending().unwrap());
    assert!(store.analytics_identity().unwrap().is_none());
    assert!(store.pending_analytics_events(10).unwrap().is_empty());
    let fresh = current_identity_pair(&store, true).unwrap();
    assert_ne!(fresh.0, previous.0);
    assert_ne!(fresh.1, previous.1);
    assert_eq!(current_identity_pair(&store, true), Some(fresh));
    reset_session();
}

#[test]
fn failed_withdrawal_blocks_reenable_and_retries_after_restart() {
    let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
    for table in ["analytics_event", "analytics_identity"] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store.set_analytics_identity(EXISTING_ID).unwrap();
        store
            .queue_analytics_event("antiburn.app_launched", "{}")
            .unwrap();
        let mut disabled = store.settings().unwrap();
        disabled.analytics_enabled = false;
        store
            .replace_settings_with_transition(&disabled, |tx, previous, saved| {
                prepare_opt_out_payload(tx, previous, saved, "antiburn:test")
            })
            .unwrap();
        // Delivery can remove the final signal before local cleanup fails.
        store
            .lock()
            .execute(
                "DELETE FROM analytics_event WHERE name = 'antiburn.analytics_opted_out'",
                [],
            )
            .unwrap();
        let pending = store.pending_analytics_events(10).unwrap();
        store
            .lock()
            .execute_batch(&format!(
                "CREATE TRIGGER reject_cleanup BEFORE DELETE ON {table}
             BEGIN SELECT RAISE(ABORT, 'synthetic cleanup failure'); END;"
            ))
            .unwrap();

        assert!(clear_local_state(&store).is_err());
        assert!(store.analytics_opt_out_pending().unwrap());
        assert_eq!(store.analytics_identity().unwrap().unwrap().0, EXISTING_ID);
        assert_eq!(store.pending_analytics_events(10).unwrap(), pending);
        let mut enabled = disabled.clone();
        enabled.analytics_enabled = true;
        assert!(
            store
                .replace_settings_preserving_interface_scale(&enabled, |tx, previous, saved| {
                    prepare_opt_out_payload(tx, previous, saved, "antiburn:test")
                })
                .is_err()
        );
        assert!(!store.settings().unwrap().analytics_enabled);
        assert!(!store.settings_snapshot().analytics_enabled);
        drop(store);

        let reopened = Store::open(directory.path()).unwrap();
        assert!(clear_pending_withdrawal(&reopened, false).is_err());
        assert!(reopened.analytics_opt_out_pending().unwrap());
        reopened
            .lock()
            .execute_batch("DROP TRIGGER reject_cleanup;")
            .unwrap();
        clear_pending_withdrawal(&reopened, false).unwrap();
        assert!(!reopened.analytics_opt_out_pending().unwrap());
        assert!(reopened.analytics_identity().unwrap().is_none());
        assert!(reopened.pending_analytics_events(10).unwrap().is_empty());
        assert!(!reopened.settings().unwrap().analytics_enabled);
        reopened
            .replace_settings_preserving_interface_scale(&enabled, |tx, previous, saved| {
                prepare_opt_out_payload(tx, previous, saved, "antiburn:test")
            })
            .unwrap();
        assert_ne!(
            current_identity_pair(&reopened, true).unwrap().0,
            EXISTING_ID
        );
    }
    reset_session();
}

#[test]
fn failed_environment_cleanup_stays_pending_without_the_override_after_restart() {
    let _lock = SUPPRESSION_TEST_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.set_analytics_identity(EXISTING_ID).unwrap();
    store
        .queue_analytics_event("antiburn.app_launched", "{}")
        .unwrap();
    store
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_cleanup BEFORE DELETE ON analytics_identity
         BEGIN SELECT RAISE(ABORT, 'synthetic cleanup failure'); END;",
        )
        .unwrap();
    assert!(clear_pending_withdrawal(&store, true).is_err());
    assert!(store.analytics_opt_out_pending().unwrap());
    drop(store);

    let reopened = Store::open(directory.path()).unwrap();
    assert!(reopened.settings().unwrap().analytics_enabled);
    assert!(clear_pending_withdrawal(&reopened, false).is_err());
    assert!(reopened.analytics_opt_out_pending().unwrap());
    reopened
        .lock()
        .execute_batch("DROP TRIGGER reject_cleanup;")
        .unwrap();
    clear_pending_withdrawal(&reopened, false).unwrap();
    assert!(!reopened.analytics_opt_out_pending().unwrap());
    assert!(reopened.analytics_identity().unwrap().is_none());
    assert!(reopened.pending_analytics_events(10).unwrap().is_empty());
    assert_ne!(
        current_identity_pair(&reopened, true).unwrap().0,
        EXISTING_ID
    );
    reset_session();
}
