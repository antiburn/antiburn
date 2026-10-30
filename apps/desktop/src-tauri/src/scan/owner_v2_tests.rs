use super::*;
use antiburn_local::analysis::SourceFormat;
use std::sync::atomic::AtomicI64;

fn source(path: &std::path::Path, id: &str, format: SourceFormat) -> SessionLog {
    SessionLog {
        agent_type: AgentKind::OpenCode,
        source: SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path: path.to_path_buf(),
            session_id: id.into(),
            source_format: Some(format),
        },
        updated_at: Some(1),
        environment: DiscoveryEnvironment::Native,
    }
}

fn fixture(path: &std::path::Path, cwd: &std::path::Path) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch("PRAGMA journal_mode=WAL").unwrap();
    connection.execute_batch(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/src/discovery/agents/tests/fixtures/opencode_pinned_mixed.sql"
    ))).unwrap();
    connection.execute(
        "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
         VALUES ('ses_selected', 'project', 'selected', ?1, 'V1 title', '1', 1000, 1000),
                ('ses_v1_only', 'project', 'v1-only', ?1, 'V1 only', '1', 1000, 1000)",
        [cwd.to_str().unwrap()],
    ).unwrap();
    connection
}

fn migrate(connection: &rusqlite::Connection, cwd: &std::path::Path) {
    connection.execute(
        "INSERT INTO session_v2 (id, project_id, slug, directory, title, version, time_created, time_updated)
         VALUES ('ses_selected', 'project', 'selected', ?1, 'V2 title', '2', 1000, 1000)",
        [cwd.to_str().unwrap()],
    ).unwrap();
    connection.execute_batch(
        "INSERT INTO session_message VALUES ('message', 'ses_selected', 'user', 1, 1000, 1000, '{\"text\":\"before\"}')"
    ).unwrap();
}

async fn scan_at(store: &Store, home: &std::path::Path, logs: Vec<SessionLog>, epoch: i64) {
    let mut writer = IncrementalAdmission {
        io: AdmissionIo::Test {
            cancel: Arc::new(AtomicBool::new(false)),
            reports: Arc::new(Mutex::new(Vec::new())),
            now: Arc::new(AtomicI64::new(epoch)),
            cancel_after_report: false,
        },
        store,
        previous_records: HashMap::new(),
        evidence_agents: vec!["opencode"],
        write_fence: store.capture_destructive_write_epoch(),
        error: None,
        outcomes: HashMap::new(),
        list_changed: false,
    };
    let policy = OwnerPolicy {
        home: home.to_path_buf(),
        ignored: HashSet::new(),
        cutoff: None,
        include_non_repo_folders: true,
        progress: false,
        refresh_repositories: false,
        refresh_file_sources: false,
        hooks: None,
    };
    let (sender, finite) = mpsc::channel(1);
    if !logs.is_empty() {
        sender
            .send(DiscoveryBatch {
                agent: AgentKind::OpenCode,
                candidates: logs,
                complete: true,
            })
            .await
            .unwrap();
    }
    drop(sender);
    let (_sender, live) = mpsc::channel(1);
    tokio::time::timeout(
        Duration::from_secs(10),
        run(finite, live, None, &policy, &mut writer),
    )
    .await
    .unwrap();
    writer.finish().unwrap();
}

fn key(id: &str) -> SessionKey {
    SessionKey::new("native", "opencode", id)
}

#[tokio::test]
async fn production_scan_switches_to_v2_and_reconstructs_its_strict_fingerprint_after_restart() {
    let state = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("opencode.db");
    let native = fixture(&path, home.path());
    let v1 = source(&path, "ses_selected", SourceFormat::OpenCodeSqliteV1);
    let v2 = source(&path, "ses_selected", SourceFormat::OpenCodeSqliteV2);
    let store = Store::open(state.path()).unwrap();
    scan_at(&store, home.path(), vec![v1], 1000).await;
    let first = store.session(&key("ses_selected")).unwrap().unwrap();
    assert_eq!(first.source_label, "opencode:ses_selected");
    assert!(first.source_fingerprint.is_some());
    migrate(&native, home.path());
    scan_at(&store, home.path(), vec![v2.clone()], 1001).await;
    let selected = store.session(&key("ses_selected")).unwrap().unwrap();
    assert_eq!(selected.source_label, v2.source_label());
    let revision = fingerprint::admission_revision(&v2).await.unwrap();
    let strict = fingerprint::strict_fingerprint(&v2, &revision)
        .await
        .unwrap();
    assert_eq!(
        selected.source_fingerprint.as_deref(),
        Some(strict.as_str())
    );
    assert_eq!(store.next_source_fingerprint_attempt().unwrap(), Some(1301));
    drop(store);

    native
        .execute(
            "UPDATE session_message SET data = '{\"text\":\"edited\"}'",
            [],
        )
        .unwrap();
    let store = Store::open(state.path()).unwrap();
    scan_at(&store, home.path(), Vec::new(), 1301).await;
    let current = store.session(&key("ses_selected")).unwrap().unwrap();
    assert_eq!(current.source_label, v2.source_label());
    assert_ne!(current.source_fingerprint.as_ref(), Some(&strict));
    assert_eq!(
        current.source_fingerprint,
        fingerprint::strict_fingerprint(&v2, &revision).await
    );
}

#[tokio::test]
async fn production_scan_hides_retained_v1_after_v2_deletion_app_delete_and_restart_until_clear() {
    let state = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("opencode.db");
    let native = fixture(&path, home.path());
    migrate(&native, home.path());
    let v1 = source(&path, "ses_selected", SourceFormat::OpenCodeSqliteV1);
    let v2 = source(&path, "ses_selected", SourceFormat::OpenCodeSqliteV2);
    let v1_only = source(&path, "ses_v1_only", SourceFormat::OpenCodeSqliteV1);
    let store = Store::open(state.path()).unwrap();
    scan_at(&store, home.path(), vec![v2.clone(), v1_only.clone()], 1000).await;
    native
        .execute("DELETE FROM session_v2 WHERE id = 'ses_selected'", [])
        .unwrap();
    native.execute(
        "INSERT INTO session (id, project_id, slug, directory, title, version, time_created, time_updated)
         VALUES ('ses_new_v1', 'project', 'new-v1', ?1, 'New V1 only', '1', 2000, 2000)",
        [home.path().to_str().unwrap()],
    ).unwrap();
    let new_v1 = source(&path, "ses_new_v1", SourceFormat::OpenCodeSqliteV1);
    assert!(fingerprint::source_is_deleted(&v2).await);
    assert!(fingerprint::admission_revision(&v1).await.is_some());
    scan_at(&store, home.path(), vec![v1.clone()], 1300).await;
    assert_eq!(
        store
            .session(&key("ses_selected"))
            .unwrap()
            .unwrap()
            .source_label,
        v2.source_label()
    );
    assert!(
        store
            .pending_source_fingerprint(&key("ses_selected"))
            .unwrap()
            .unwrap()
            .source_path
            .is_none()
    );
    store.delete_session(&key("ses_selected")).unwrap().unwrap();
    drop(store);

    let store = Store::open(state.path()).unwrap();
    scan_at(
        &store,
        home.path(),
        vec![v1.clone(), v1_only.clone(), new_v1.clone()],
        1600,
    )
    .await;
    assert!(store.session(&key("ses_selected")).unwrap().is_none());
    assert_eq!(
        store
            .session(&key("ses_new_v1"))
            .unwrap()
            .unwrap()
            .source_label,
        new_v1.source_label()
    );
    assert_eq!(
        store
            .session(&key("ses_v1_only"))
            .unwrap()
            .unwrap()
            .source_label,
        v1_only.source_label()
    );
    store.clear_local_session_data().unwrap();
    drop(store);

    let store = Store::open(state.path()).unwrap();
    scan_at(&store, home.path(), vec![v1.clone()], 1900).await;
    assert_eq!(
        store
            .session(&key("ses_selected"))
            .unwrap()
            .unwrap()
            .source_label,
        v1.source_label()
    );
    assert!(
        store
            .session(&key("ses_selected"))
            .unwrap()
            .unwrap()
            .source_fingerprint
            .is_some()
    );
}

#[tokio::test]
async fn production_scan_prefers_v2_to_a_newer_v1_copy_from_another_database() {
    let state = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let legacy_path = home.path().join("legacy.db");
    let current_path = home.path().join("current.db");
    let _legacy = fixture(&legacy_path, home.path());
    let current = fixture(&current_path, home.path());
    migrate(&current, home.path());
    let mut v1 = source(&legacy_path, "ses_selected", SourceFormat::OpenCodeSqliteV1);
    v1.updated_at = Some(2000);
    let v2 = source(
        &current_path,
        "ses_selected",
        SourceFormat::OpenCodeSqliteV2,
    );
    let store = Store::open(state.path()).unwrap();
    scan_at(&store, home.path(), vec![v1, v2.clone()], 1000).await;
    assert_eq!(
        store
            .session(&key("ses_selected"))
            .unwrap()
            .unwrap()
            .source_label,
        v2.source_label()
    );
}

#[tokio::test]
async fn production_missing_database_keeps_retained_v1_fragments_hidden_until_clear() {
    const ROOT_ENV: &str = "ANTIBURN_TEST_OPENCODE_SELECTION_ROOT";
    let Some(root) = std::env::var_os(ROOT_ENV) else {
        // Isolate discovery overrides from other tests in this process.
        let directory = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "scan::owner::v2_tests::production_missing_database_keeps_retained_v1_fragments_hidden_until_clear",
                "--nocapture",
            ])
            .env(ROOT_ENV, directory.path())
            .env("OPENCODE_DATA_DIR", directory.path())
            .env("OPENCODE_DB", "opencode.db")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated discovery regression failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    let home = PathBuf::from(root);
    let state = tempfile::tempdir().unwrap();
    let path = home.join("opencode.db");
    let native = fixture(&path, &home);
    migrate(&native, &home);
    let fragments = home.join("storage/session/global");
    std::fs::create_dir_all(&fragments).unwrap();
    for id in ["ses_selected", "ses_fragment_only"] {
        std::fs::write(
            fragments.join(format!("{id}.json")),
            serde_json::json!({
                "id": id, "directory": home, "title": "Retained V1 fragment",
                "time": {"created": 1000, "updated": 1000},
            })
            .to_string(),
        )
        .unwrap();
    }
    let explorer = Explorers::DISK.get(&AgentKind::OpenCode);
    let store = Store::open(state.path()).unwrap();
    scan_at(
        &store,
        &home,
        explorer.discover_recent(1000, 1000).await,
        1000,
    )
    .await;
    let v2_label = "opencode:ses_selected:open_code_v2_sqlite";
    assert_eq!(
        store
            .session(&key("ses_selected"))
            .unwrap()
            .unwrap()
            .source_label,
        v2_label
    );
    native
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    drop(native);
    std::fs::rename(&path, home.join("removed-database.saved")).unwrap();
    assert!(!path.exists());

    let logs = explorer.discover_recent(1000, 1000).await;
    let retained = logs
        .iter()
        .find(|log| log.source_label() == "opencode:ses_selected")
        .unwrap();
    let SessionSource::Inline { content, .. } = &retained.source else {
        panic!("missing database must reach legacy fragment discovery");
    };
    assert_eq!(
        crate::analysis::source_format(retained.agent_type, &retained.source),
        SourceFormat::OpenCodeJsonl
    );
    let export_path = home.join("retained-selected.jsonl");
    std::fs::write(&export_path, content).unwrap();
    let file = SessionLog {
        agent_type: AgentKind::OpenCode,
        source: SessionSource::File(export_path),
        updated_at: Some(1),
        environment: DiscoveryEnvironment::Native,
    };
    let mut candidates = logs.clone();
    candidates.push(file.clone());
    scan_at(&store, &home, candidates, 1300).await;
    assert_eq!(
        store
            .session(&key("ses_selected"))
            .unwrap()
            .unwrap()
            .source_label,
        v2_label
    );
    assert!(store.session(&key("ses_fragment_only")).unwrap().is_some());
    store.delete_session(&key("ses_selected")).unwrap().unwrap();
    drop(store);

    let store = Store::open(state.path()).unwrap();
    let mut candidates = explorer.discover_recent(1000, 1000).await;
    candidates.push(file.clone());
    scan_at(&store, &home, candidates, 1600).await;
    assert!(store.session(&key("ses_selected")).unwrap().is_none());
    assert!(store.session(&key("ses_fragment_only")).unwrap().is_some());
    store.clear_local_session_data().unwrap();
    drop(store);

    let store = Store::open(state.path()).unwrap();
    scan_at(
        &store,
        &home,
        explorer.discover_recent(1000, 1000).await,
        1900,
    )
    .await;
    let restored = store.session(&key("ses_selected")).unwrap().unwrap();
    assert_eq!(restored.source_kind, "inline");
    assert_eq!(restored.source_label, "opencode:ses_selected");
    store.delete_session(&key("ses_selected")).unwrap().unwrap();
    scan_at(&store, &home, vec![file.clone()], 1901).await;
    assert_eq!(
        store
            .session(&key("ses_selected"))
            .unwrap()
            .unwrap()
            .source_label,
        file.source_label()
    );
}
