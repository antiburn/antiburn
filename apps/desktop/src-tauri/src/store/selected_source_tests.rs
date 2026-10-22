use super::*;

fn record(id: &str, format: SourceFormat) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", "opencode", id),
        source_kind: "providerDb".into(),
        source_label: match format {
            SourceFormat::OpenCodeSqliteV1 => format!("opencode:{id}"),
            SourceFormat::OpenCodeSqliteV2 => format!("opencode:{id}:open_code_v2_sqlite"),
            _ => unreachable!(),
        },
        wsl_distro: None,
        title: None,
        title_source: None,
        cwd: Some("/synthetic".into()),
        surface: "cli".into(),
        updated_at_epoch: Some(1000),
        activity_cursor: format!(
            "opencode-admission-v2:{}",
            serde_json::json!(["/synthetic/opencode.db", id, "file-identity", "revision"])
        ),
        activity_source: "unknown".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: None,
    }
}

fn allowed(store: &Store, record: &SessionRecord) -> bool {
    source_is_allowed_in(
        &store.lock(),
        &record.key,
        stored_native_opencode_format(&record.key, &record.source_kind, &record.source_label),
    )
    .unwrap()
}

#[test]
fn selection_excludes_known_v1_formats_without_blocking_unknown_or_v2_routes() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    let v2 = record("selected", SourceFormat::OpenCodeSqliteV2);
    store
        .upsert_sessions(std::slice::from_ref(&v2), &[])
        .unwrap();
    for format in [SourceFormat::OpenCodeSqliteV1, SourceFormat::OpenCodeJsonl] {
        assert!(!store.session_source_is_allowed(&v2.key, format).unwrap());
        assert!(
            store
                .session_source_is_allowed(&SessionKey::new("native", "opencode", "new-v1"), format)
                .unwrap()
        );
        assert!(
            store
                .session_source_is_allowed(
                    &SessionKey::new("wsl:synthetic", "opencode", "selected"),
                    format
                )
                .unwrap()
        );
        assert!(
            store
                .session_source_is_allowed(
                    &SessionKey::new("native", "claude-code", "selected"),
                    format
                )
                .unwrap()
        );
    }
    for format in [
        SourceFormat::Uncharacterized,
        SourceFormat::OpenCodeSqliteV2,
        SourceFormat::OpenCodeV2SessionExport,
    ] {
        assert!(store.session_source_is_allowed(&v2.key, format).unwrap());
    }
}

#[test]
fn stored_inline_v1_guard_uses_the_exact_legacy_label_and_survives_restart() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let v2 = record("selected", SourceFormat::OpenCodeSqliteV2);
    store
        .upsert_sessions(std::slice::from_ref(&v2), &[])
        .unwrap();
    let mut inline = record("selected", SourceFormat::OpenCodeSqliteV1);
    inline.source_kind = "inline".into();
    assert_eq!(
        native_opencode_sqlite_format(&inline.key, &inline.source_kind, &inline.source_label),
        None
    );
    assert!(!allowed(&store, &inline));
    assert!(
        store
            .upsert_sessions(std::slice::from_ref(&inline), &[])
            .is_err()
    );
    for (kind, label) in [
        ("inline", "opencode:another-id"),
        ("inline", "opencode:selected:unknown"),
        ("inline", "opencode-v2-export:selected"),
        ("unknown", "opencode:selected"),
    ] {
        let mut unknown = inline.clone();
        unknown.source_kind = kind.into();
        unknown.source_label = label.into();
        assert!(allowed(&store, &unknown));
    }
    store.delete_session(&v2.key).unwrap();
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    assert!(!allowed(&store, &inline));
    assert!(
        store
            .upsert_sessions(std::slice::from_ref(&inline), &[])
            .is_err()
    );
    store.clear_local_session_data().unwrap();
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    assert!(allowed(&store, &inline));
    store.upsert_sessions(&[inline], &[]).unwrap();
}

#[test]
fn v2_selection_survives_session_deletion_and_restart_but_clear_resets_it() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let v1 = record("selected", SourceFormat::OpenCodeSqliteV1);
    let v2 = record("selected", SourceFormat::OpenCodeSqliteV2);
    store
        .upsert_sessions(std::slice::from_ref(&v1), &[])
        .unwrap();
    let old = store.pending_source_fingerprint(&v1.key).unwrap().unwrap();
    store
        .upsert_sessions(std::slice::from_ref(&v2), &[])
        .unwrap();
    assert!(!allowed(&store, &v1));
    assert!(allowed(&store, &v2));
    assert!(
        !store
            .complete_source_fingerprint(&old, "stale", 1000)
            .unwrap()
    );
    assert!(
        store
            .upsert_sessions(std::slice::from_ref(&v1), &[])
            .is_err()
    );
    assert_eq!(
        store.session(&v2.key).unwrap().unwrap().source_label,
        v2.source_label
    );

    store.delete_session(&v2.key).unwrap().unwrap();
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    assert!(!allowed(&store, &v1));
    assert!(
        store
            .upsert_sessions(std::slice::from_ref(&v1), &[])
            .is_err()
    );
    assert!(store.session(&v1.key).unwrap().is_none());
    let v1_only = record("new-v1-only", SourceFormat::OpenCodeSqliteV1);
    assert!(allowed(&store, &v1_only));
    store.upsert_sessions(&[v1_only], &[]).unwrap();

    store.clear_local_session_data().unwrap();
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    assert!(allowed(&store, &v1));
    store.upsert_sessions(&[v1], &[]).unwrap();
}

#[test]
fn selection_is_native_opencode_only_and_rolls_back_with_the_session_write() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    let v2 = record("selected", SourceFormat::OpenCodeSqliteV2);
    store.upsert_sessions(&[v2], &[]).unwrap();
    let v1 = record("selected", SourceFormat::OpenCodeSqliteV1);
    for (environment, agent) in [("wsl:synthetic", "opencode"), ("native", "claude-code")] {
        let mut other = v1.clone();
        other.key.environment_key = environment.into();
        other.key.agent = agent.into();
        assert!(allowed(&store, &other));
        store.upsert_sessions(&[other], &[]).unwrap();
    }
    let new_v2 = record("rolled-back", SourceFormat::OpenCodeSqliteV2);
    assert!(store.upsert_sessions(&[new_v2.clone(), v1], &[]).is_err());
    assert!(store.session(&new_v2.key).unwrap().is_none());
    assert!(allowed(
        &store,
        &record("rolled-back", SourceFormat::OpenCodeSqliteV1)
    ));
}

#[test]
fn v81_preserves_preexisting_exact_native_v2_selections() {
    let directory = tempfile::tempdir().unwrap();
    let connection = Connection::open_in_memory().unwrap();
    for sql in &schema::MIGRATIONS[..80] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute_batch(
            "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
             first_seen_at, last_seen_at, incarnation)
         VALUES ('native', 'opencode', 'selected', 'providerDb',
                    'opencode:selected:open_code_v2_sqlite', '2026-01-01', '2026-01-01', 1),
                ('native', 'opencode', 'v1-only', 'providerDb',
                    'opencode:v1-only', '2026-01-01', '2026-01-01', 2),
                ('native', 'opencode', 'wrong-id', 'providerDb',
                    'opencode:selected:open_code_v2_sqlite', '2026-01-01', '2026-01-01', 3);",
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 80).unwrap();
    let store = Store::from_connection(connection, directory.path().to_path_buf()).unwrap();
    store
        .delete_session(&SessionKey::new("native", "opencode", "selected"))
        .unwrap();
    assert!(!allowed(
        &store,
        &record("selected", SourceFormat::OpenCodeSqliteV1)
    ));
    assert!(allowed(
        &store,
        &record("v1-only", SourceFormat::OpenCodeSqliteV1)
    ));
    assert!(allowed(
        &store,
        &record("wrong-id", SourceFormat::OpenCodeSqliteV1)
    ));
}

#[test]
fn fingerprint_scheduling_accepts_only_exact_native_versioned_labels() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    let v1 = record("v1", SourceFormat::OpenCodeSqliteV1);
    let v2 = record("v2", SourceFormat::OpenCodeSqliteV2);
    store
        .upsert_sessions(&[v1.clone(), v2.clone()], &[])
        .unwrap();
    for (id, environment, agent, kind, label) in [
        (
            "wrong-id",
            "native",
            "opencode",
            "providerDb",
            "opencode:v2:open_code_v2_sqlite",
        ),
        (
            "unknown",
            "native",
            "opencode",
            "providerDb",
            "opencode:unknown:future",
        ),
        (
            "v2-suffix",
            "native",
            "opencode",
            "providerDb",
            "opencode:v2-suffix:open_code_v2_sqlite:extra",
        ),
        (
            "remote",
            "wsl:synthetic",
            "opencode",
            "providerDb",
            "opencode:remote:open_code_v2_sqlite",
        ),
        (
            "agent",
            "native",
            "claude-code",
            "providerDb",
            "opencode:agent:open_code_v2_sqlite",
        ),
        (
            "file",
            "native",
            "opencode",
            "file",
            "opencode:file:open_code_v2_sqlite",
        ),
    ] {
        let mut other = record(id, SourceFormat::OpenCodeSqliteV1);
        other.key.environment_key = environment.into();
        other.key.agent = agent.into();
        other.source_kind = kind.into();
        other.source_label = label.into();
        store
            .upsert_sessions(std::slice::from_ref(&other), &[])
            .unwrap();
        store
            .lock()
            .execute(
                "UPDATE session SET fingerprint_source_path = '/synthetic/unsupported.db',
                admission_revision = 'unsupported', fingerprint_next_attempt_at_epoch = -1
             WHERE session_id = ?1",
                [id],
            )
            .unwrap();
        assert!(
            store
                .pending_source_fingerprint(&other.key)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(store.next_source_fingerprint_attempt().unwrap(), Some(0));
    let work = store
        .claim_source_fingerprints(1000, &HashSet::new(), 20, 1000)
        .unwrap();
    assert_eq!(work.len(), 2);
    assert!(
        work.iter().any(|pending| pending.key == v1.key
            && pending.source_format == SourceFormat::OpenCodeSqliteV1)
    );
    assert!(
        work.iter().any(|pending| pending.key == v2.key
            && pending.source_format == SourceFormat::OpenCodeSqliteV2)
    );
    assert_eq!(store.next_source_fingerprint_attempt().unwrap(), None);
    for pending in work {
        assert!(
            store
                .complete_source_fingerprint(&pending, "strict", 1000)
                .unwrap()
        );
    }
    assert_eq!(store.next_source_fingerprint_attempt().unwrap(), Some(1300));
}
