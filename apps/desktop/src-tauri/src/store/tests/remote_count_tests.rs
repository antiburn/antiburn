use super::*;

#[test]
fn remote_counts_follow_retention_and_clear_without_a_scan() {
    const NOW: i64 = 2_000_000_000;
    let store = store();
    let mut expired = session("expired", NOW - 31 * 86_400);
    expired.key.environment_key = "ssh:host".into();
    let mut kept = session("retained-outside-discovery", NOW - 8 * 86_400);
    kept.key.environment_key = "ssh:host".into();
    store
        .upsert_sessions(&[expired, kept], &["claude-code"])
        .unwrap();
    assert_eq!(store.remote_session_counts().unwrap()["host"], 2);
    store
        .save_settings(&AppSettings {
            session_data_retention_days: SESSION_DATA_RETENTION_DAYS_30,
            ..AppSettings::default()
        })
        .unwrap();
    assert_eq!(store.apply_session_retention(NOW).unwrap().0, 1);
    assert_eq!(store.remote_session_counts().unwrap()["host"], 1);
    store.clear_local_session_data().unwrap();
    assert!(store.remote_session_counts().unwrap().is_empty());
}

#[test]
fn remote_counts_include_old_cache_and_separate_origins() {
    let store = store();
    let local = session("same-id", 1);
    let mut a = local.clone();
    a.key.environment_key = "ssh:host-a".into();
    let mut b = a.clone();
    b.key.environment_key = "ssh:host-b".into();
    let mut wsl = local.clone();
    wsl.key.environment_key = "wsl:ubuntu".into();
    store
        .upsert_sessions(&[local, a.clone(), b.clone(), wsl], &["claude-code"])
        .unwrap();
    assert_eq!(
        store.remote_session_counts().unwrap(),
        HashMap::from([("host-a".into(), 1), ("host-b".into(), 1)])
    );
    let mut fresh = a.clone();
    fresh.key.session_id = "fresh".into();
    fresh.updated_at_epoch = Some(time::OffsetDateTime::now_utc().unix_timestamp());
    store.upsert_sessions(&[fresh], &["claude-code"]).unwrap();
    assert_eq!(store.remote_session_counts().unwrap()["host-a"], 2);
    store.delete_session(&a.key).unwrap();
    assert_eq!(store.remote_session_counts().unwrap()["host-a"], 1);
    store.delete_remote_host("host-a").unwrap();
    assert_eq!(
        store.remote_session_counts().unwrap(),
        HashMap::from([("host-b".into(), 1)])
    );
    assert!(store.session(&b.key).unwrap().is_some());
}

#[test]
fn remote_counts_recover_from_database_without_host_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path();
    let store = Store::open(state).unwrap();
    let mut remote = session("old-cached", 1);
    remote.key.environment_key = "ssh:host".into();
    store.upsert_sessions(&[remote], &["claude-code"]).unwrap();
    drop(store);
    let reopened = Store::open(state).unwrap();
    assert_eq!(reopened.remote_session_counts().unwrap()["host"], 1);
}
