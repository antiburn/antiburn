use super::*;

#[test]
fn live_projects_admits_exact_v1_and_v2_native_main_labels_and_cwds() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    for (id, label, cwd, environment, distro, updated) in [
        ("v1", "opencode:v1", "/workspace/v1", "native", None, now),
        (
            "v2",
            "opencode:v2:open_code_v2_sqlite",
            "/workspace/v2/nested",
            "native",
            None,
            now,
        ),
        (
            "wrong",
            "opencode:wrong:open_code_v2_sqlite:extra",
            "/workspace/wrong",
            "native",
            None,
            now,
        ),
        (
            "remote",
            "opencode:remote:open_code_v2_sqlite",
            "/workspace/remote",
            "ssh:synthetic",
            None,
            now,
        ),
        (
            "wsl",
            "opencode:wsl:open_code_v2_sqlite",
            "/workspace/wsl",
            "native",
            Some("Synthetic"),
            now,
        ),
        (
            "empty",
            "opencode:empty:open_code_v2_sqlite",
            "",
            "native",
            None,
            now,
        ),
        (
            "old",
            "opencode:old:open_code_v2_sqlite",
            "/workspace/old",
            "native",
            None,
            1,
        ),
        (
            "fork",
            "opencode:fork:open_code_v2_sqlite",
            "/workspace/fork",
            "native",
            None,
            now,
        ),
        (
            "child",
            "opencode:child:open_code_v2_sqlite",
            "/workspace/child",
            "native",
            None,
            now,
        ),
    ] {
        store.lock().execute(
            "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
              cwd, wsl_distro, updated_at_epoch, first_seen_at, last_seen_at)
             VALUES (?1, 'opencode', ?2, 'providerDb', ?3, ?4, ?5, ?6, '', '')",
            rusqlite::params![environment, id, label, cwd, distro, updated],
        ).unwrap();
    }
    store.lock().execute("INSERT INTO session_relation VALUES ('native', 'opencode', 'fork', 'forkParent', 'parent', NULL)", []).unwrap();
    store.lock().execute("INSERT INTO session_relation VALUES ('native', 'opencode', 'parent', 'subagent', 'child', NULL)", []).unwrap();
    let scopes = live_projects(&store).unwrap();
    assert_eq!(scopes.len(), 2);
    assert_eq!(scopes[0].session_cwd, "/workspace/v1");
    assert_eq!(scopes[1].session_cwd, "/workspace/v2/nested");
    assert!(
        scopes
            .iter()
            .all(|scope| scope.environment == "native" && scope.run == RunScope::Live)
    );
}

#[test]
fn v2_scope_work_uses_exact_live_lease_instead_of_metric_backlog() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    let scope = ActivationScope {
        environment: "native".into(),
        session_cwd: "/workspace/demo".into(),
        run: RunScope::Live,
    };
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    store.lock().execute(
        "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
          cwd, updated_at_epoch, first_seen_at, last_seen_at, incarnation, source_format)
         VALUES ('native', 'opencode', 'main', 'providerDb', 'opencode:main:open_code_v2_sqlite', '/workspace/demo', 1, '', '', 7, ?1)",
        [serde_json::to_string(&SourceFormat::OpenCodeSqliteV2).unwrap()],
    ).unwrap();
    assert!(!scope_has_work(&store, &scope).unwrap());
    store
        .lock()
        .execute(
            "INSERT INTO opencode_v2_capture
          (environment_key, agent, session_id, incarnation, source_generation,
           source_label, cwd, capture_epoch, claim_fence, history_gap, lease_expires_at_epoch)
         SELECT environment_key, agent, session_id, incarnation, source_generation,
           source_label, cwd, 'epoch', 1, 1, ?1 FROM session",
            [now + 60],
        )
        .unwrap();
    assert!(scope_has_work(&store, &scope).unwrap());
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1",
            [],
        )
        .unwrap();
    assert!(scope_has_work(&store, &scope).unwrap());
    let mut other = scope.clone();
    other.session_cwd = "/workspace".into();
    assert!(!scope_has_work(&store, &other).unwrap());
    store
        .lock()
        .execute("UPDATE opencode_v2_capture SET incarnation = 6", [])
        .unwrap();
    assert!(!scope_has_work(&store, &scope).unwrap());
}
