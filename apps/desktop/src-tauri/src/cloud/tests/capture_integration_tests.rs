use super::*;
use std::fs;

fn fixture() -> (
    tempfile::TempDir,
    PathBuf,
    Store,
    super::super::auth::Session,
    Draft,
    Consent,
) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory
        .path()
        .canonicalize()
        .unwrap()
        .join("native-project");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("session-one.jsonl"),
        b"synthetic bytes, deliberately not parsed\n",
    )
    .unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let session = super::super::tests::session();
    let path = root.to_str().unwrap().to_owned();
    let draft = Draft {
        folders: vec![Folder {
            path: path.clone(),
            selected: true,
            ..Folder::default()
        }],
        ..Draft::default()
    };
    let consent = Consent {
        version: "native-full-content-v1".into(),
        account_id: session.user.id.clone(),
        destination_id: session.workspace.id.clone(),
        selected_roots: vec![path],
        selected_sources: Vec::new(),
        full_content: true,
        attachments: false,
    };
    (directory, root, store, session, draft, consent)
}

#[test]
fn cloud_selected_sources_stay_durable_without_capture_bytes_or_upload_authority() {
    let (directory, root, store, session, draft, consent) = fixture();
    let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
    let request = store
        .prepare_cloud_capture(
            "request",
            &session.user.id,
            &session.workspace.id,
            &roots,
            consent.version(),
            1,
        )
        .unwrap();
    let mut host = Host::new(&directory.path().canonicalize().unwrap());
    let selected = host.discover(&store, &request, || true).unwrap();
    assert!(selected.discovery_complete);
    assert_eq!(selected.sources.len(), 1);
    let SelectedSource::Located { identity, locator } = &selected.sources[0] else {
        panic!("new selections need typed locators");
    };
    assert_eq!(locator.source_kind, SourceKind::ClaudeJsonl);
    assert_eq!(locator.relative_path, "session-one.jsonl");
    assert_eq!(locator.native_session_id, "session-one");
    let source_id = format!("{}/session-one", request.roots[0].source_scope_id);
    assert_eq!(identity.source_id, source_id);
    assert_eq!(
        identity.capture_id,
        URL_SAFE_NO_PAD.encode(Sha256::digest(
            serde_json::to_vec(&("request", &source_id)).unwrap()
        ))
    );
    assert!(matches!(
        SyncResult::from(&selected),
        SyncResult::UploadUnavailable
    ));
    assert!(host.spool.as_ref().unwrap().entries().unwrap().is_empty());
    assert_eq!(
        fs::read(root.join("session-one.jsonl")).unwrap(),
        b"synthetic bytes, deliberately not parsed\n"
    );
    fs::write(
        root.join("later-session.jsonl"),
        b"another synthetic source",
    )
    .unwrap();
    let repeated = host.discover(&store, &selected, || true).unwrap();
    assert_eq!(repeated, selected);
    drop(host);
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    let saved = store
        .cloud_capture_request("request", &session.user.id, &session.workspace.id)
        .unwrap();
    assert_eq!(saved, selected);
}

#[test]
fn cloud_source_consent_rejects_changed_roots_accounts_destinations_and_attachments() {
    let (_directory, _, _store, session, draft, mut consent) = fixture();
    validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
    consent.selected_roots.clear();
    assert!(validate_selection(&session, &draft, &draft.folders, &consent).is_err());
    consent.selected_roots = selected_roots(&draft.folders)
        .into_iter()
        .map(|root| root.path)
        .collect();
    consent.account_id = "another".into();
    assert!(validate_selection(&session, &draft, &draft.folders, &consent).is_err());
    consent.account_id = session.user.id.clone();
    consent.destination_id = "team".into();
    assert!(validate_selection(&session, &draft, &draft.folders, &consent).is_err());
    consent.destination_id = session.workspace.id.clone();
    consent.attachments = true;
    assert!(validate_selection(&session, &draft, &draft.folders, &consent).is_err());
}

#[test]
fn cloud_source_roots_reject_exact_path_aliases_before_scope_creation() {
    let (_directory, root, _store, session, mut draft, mut consent) = fixture();
    for alias in [
        format!("{}/", root.display()),
        format!("{}/.", root.display()),
        format!("{}//native-project", root.parent().unwrap().display()),
    ] {
        draft.folders[0].path = alias.clone();
        consent.selected_roots = vec![alias];
        assert!(validate_selection(&session, &draft, &draft.folders, &consent).is_err());
    }
}

#[test]
fn cloud_spool_has_one_owner_and_account_loss_keeps_saved_requests() {
    let (directory, _, store, session, draft, consent) = fixture();
    let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
    let request = store
        .prepare_cloud_capture(
            "request",
            &session.user.id,
            &session.workspace.id,
            &roots,
            consent.version(),
            1,
        )
        .unwrap();
    let mut host = Host::new(&directory.path().canonicalize().unwrap());
    assert!(host.discover(&store, &request, || false).is_err());
    assert!(
        !store
            .cloud_capture_request("request", &session.user.id, &session.workspace.id)
            .unwrap()
            .discovery_complete
    );
    host.discover(&store, &request, || true).unwrap();
    let next = store
        .prepare_cloud_capture(
            "next-request",
            &session.user.id,
            &session.workspace.id,
            &roots,
            consent.version(),
            2,
        )
        .unwrap();
    let mut second = Host::new(&directory.path().canonicalize().unwrap());
    assert!(second.discover(&store, &next, || true).is_err());
    assert!(
        !store
            .cloud_capture_request("next-request", &session.user.id, &session.workspace.id)
            .unwrap()
            .discovery_complete
    );
}

fn authorize_database(draft: &mut Draft, consent: &mut Consent, database: &Path) {
    draft.folders.push(Folder {
        source_kind: SourceKind::OpenCodeSqliteV2,
        path: database.to_str().unwrap().into(),
        selected: true,
        ..Folder::default()
    });
    consent.version = SOURCE_CONSENT_VERSION.into();
    consent.selected_roots.clear();
    consent.selected_sources = selected_roots(&draft.folders);
}

#[test]
fn cloud_claude_child_locators_keep_root_relative_paths_and_independent_identities() {
    let (directory, root, store, session, draft, consent) = fixture();
    let children = root.join("session-one/subagents");
    fs::create_dir_all(&children).unwrap();
    fs::write(
        children.join("agent-child.jsonl"),
        b"synthetic child source",
    )
    .unwrap();
    let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
    let request = store
        .prepare_cloud_capture(
            "children",
            &session.user.id,
            &session.workspace.id,
            &roots,
            consent.version(),
            1,
        )
        .unwrap();
    let mut host = Host::new(&directory.path().canonicalize().unwrap());
    let selected = host.discover(&store, &request, || true).unwrap();
    assert_eq!(selected.sources.len(), 2);
    let SelectedSource::Located { identity, locator } = &selected.sources[1] else {
        panic!("typed child locator expected");
    };
    assert_eq!(
        locator.relative_path,
        "session-one/subagents/agent-child.jsonl"
    );
    assert_eq!(locator.native_session_id, "session-one");
    assert_eq!(locator.native_agent_id.as_deref(), Some("child"));
    assert_eq!(
        identity.source_id,
        format!(
            "{}/session-one/subagents/agent-child",
            request.roots[0].source_scope_id
        )
    );
    assert_eq!(
        fs::read(children.join("agent-child.jsonl")).unwrap(),
        b"synthetic child source"
    );
    assert!(host.spool.as_ref().unwrap().entries().unwrap().is_empty());
}

#[test]
fn cloud_opencode_selection_freezes_each_database_session_without_payload_capture() {
    let (directory, _, store, session, mut draft, mut consent) = fixture();
    let root = directory.path().canonicalize().unwrap();
    let sql = include_str!("../../../../../../fixtures/cloud/opencode/current.sql");
    let mut before = Vec::new();
    for name in ["first", "second"] {
        let parent = root.join(name);
        fs::create_dir(&parent).unwrap();
        let database = parent.join("opencode.db");
        rusqlite::Connection::open(&database)
            .unwrap()
            .execute_batch(sql)
            .unwrap();
        before.push((database.clone(), fs::read(&database).unwrap()));
        authorize_database(&mut draft, &mut consent, &database);
    }
    let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
    let request = store
        .prepare_cloud_capture(
            "mixed",
            &session.user.id,
            &session.workspace.id,
            &roots,
            consent.version(),
            1,
        )
        .unwrap();
    let mut host = Host::new(&root);
    let selected = host.discover(&store, &request, || true).unwrap();
    assert_eq!(selected.sources.len(), 9);
    let mut identities = std::collections::BTreeSet::new();
    let mut sessions = std::collections::BTreeMap::<String, usize>::new();
    for source in &selected.sources {
        let SelectedSource::Located { identity, locator } = source else {
            panic!("typed source expected")
        };
        assert!(identities.insert(identity.capture_id.clone()));
        if locator.source_kind == SourceKind::OpenCodeSqliteV2 {
            assert_eq!(locator.relative_path, "opencode.db");
            assert!(locator.native_agent_id.is_none());
            *sessions
                .entry(locator.native_session_id.clone())
                .or_default() += 1;
        }
    }
    assert_eq!(
        sessions,
        ["ses_main", "ses_child", "ses_detached", "ses_fork"]
            .into_iter()
            .map(|id| (id.into(), 2))
            .collect()
    );
    assert!(host.spool.as_ref().unwrap().entries().unwrap().is_empty());
    assert!(matches!(
        SyncResult::from(&selected),
        SyncResult::UploadUnavailable
    ));
    for (database, bytes) in before {
        assert_eq!(fs::read(&database).unwrap(), bytes);
        fs::remove_file(database).unwrap();
    }
    assert_eq!(host.discover(&store, &selected, || true).unwrap(), selected);
    drop(host);
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(
        store
            .cloud_capture_request("mixed", &session.user.id, &session.workspace.id)
            .unwrap(),
        selected
    );
}

#[test]
fn cloud_opencode_dispatch_requires_typed_consent_and_never_falls_back() {
    let (directory, _, store, session, mut draft, mut consent) = fixture();
    let root = directory.path().canonicalize().unwrap();
    let database = root.join("opencode.db");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY);")
        .unwrap();
    authorize_database(&mut draft, &mut consent, &database);
    consent.version = CONSENT_VERSION.into();
    consent.selected_sources.clear();
    consent.selected_roots = draft
        .folders
        .iter()
        .map(|folder| folder.path.clone())
        .collect();
    assert!(validate_selection(&session, &draft, &draft.folders, &consent).is_err());
    consent.version = SOURCE_CONSENT_VERSION.into();
    consent.selected_roots.clear();
    consent.selected_sources = selected_roots(&draft.folders);
    let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
    let request = store
        .prepare_cloud_capture(
            "typed",
            &session.user.id,
            &session.workspace.id,
            &roots,
            consent.version(),
            1,
        )
        .unwrap();
    let mut host = Host::new(&root);
    assert!(
        host.discover(&store, &request, || true)
            .unwrap_err()
            .contains("OpenCode V2")
    );
    let saved = store
        .cloud_capture_request("typed", &session.user.id, &session.workspace.id)
        .unwrap();
    assert_eq!(saved, request);
    connection
        .execute_batch(include_str!(
            "../../../../../../fixtures/cloud/opencode/earlier.sql"
        ))
        .unwrap();
    drop(connection);
    let selected = host.discover(&store, &saved, || true).unwrap();
    assert_eq!(selected.sources.len(), 2);
    assert_eq!(selected.consent_version, SOURCE_CONSENT_VERSION);
}

#[test]
fn cloud_discovery_at_the_source_limit_accepts_empty_roots_but_rejects_more_sources() {
    let (directory, root, store, session, mut draft, mut consent) = fixture();
    for index in 1..512 {
        fs::write(root.join(format!("session-{index}.jsonl")), b"").unwrap();
    }
    let empty = directory.path().canonicalize().unwrap().join("zz-empty");
    fs::create_dir(&empty).unwrap();
    draft.folders.push(Folder {
        path: empty.to_str().unwrap().into(),
        selected: true,
        ..Folder::default()
    });
    consent.selected_roots = selected_roots(&draft.folders)
        .into_iter()
        .map(|root| root.path)
        .collect();
    let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
    let mut host = Host::new(&directory.path().canonicalize().unwrap());
    let request = store
        .prepare_cloud_capture(
            "at-limit",
            &session.user.id,
            &session.workspace.id,
            &roots,
            consent.version(),
            1,
        )
        .unwrap();
    let selected = host.discover(&store, &request, || true).unwrap();
    assert!(selected.discovery_complete);
    assert_eq!(selected.sources.len(), 512);

    fs::write(empty.join("one-more.jsonl"), b"").unwrap();
    let overflow = store
        .prepare_cloud_capture(
            "over-limit",
            &session.user.id,
            &session.workspace.id,
            &roots,
            consent.version(),
            2,
        )
        .unwrap();
    assert!(
        host.discover(&store, &overflow, || true)
            .unwrap_err()
            .contains("Choose fewer")
    );
    assert_eq!(
        store
            .cloud_capture_request("over-limit", &session.user.id, &session.workspace.id)
            .unwrap(),
        overflow
    );
    assert!(host.spool.as_ref().unwrap().entries().unwrap().is_empty());
}

#[test]
fn cloud_mixed_discovery_limit_keeps_the_original_request_without_partial_selection() {
    let (directory, _, store, session, mut draft, mut consent) = fixture();
    let root = directory.path().canonicalize().unwrap();
    let database = root.join("opencode.db");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(include_str!(
            "../../../../../../fixtures/cloud/opencode/earlier.sql"
        ))
        .unwrap();
    connection
        .execute_batch(
            "WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n < 511)
             INSERT INTO session_v2(id) SELECT 'ses_' || n FROM ids;",
        )
        .unwrap();
    drop(connection);
    authorize_database(&mut draft, &mut consent, &database);
    let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
    let request = store
        .prepare_cloud_capture(
            "bounded",
            &session.user.id,
            &session.workspace.id,
            &roots,
            consent.version(),
            1,
        )
        .unwrap();
    let mut host = Host::new(&root);
    assert!(host.discover(&store, &request, || true).is_err());
    assert_eq!(
        store
            .cloud_capture_request("bounded", &session.user.id, &session.workspace.id)
            .unwrap(),
        request
    );
    assert!(host.spool.as_ref().unwrap().entries().unwrap().is_empty());
}

#[test]
fn cloud_account_invalidation_stops_discovery_before_the_next_provider_root() {
    let (directory, _, store, session, mut draft, mut consent) = fixture();
    let root = directory.path().canonicalize().unwrap();
    let database = root.join("opencode.db");
    fs::write(&database, b"not a database; this provider must not run").unwrap();
    authorize_database(&mut draft, &mut consent, &database);
    let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
    let request = store
        .prepare_cloud_capture(
            "fenced",
            &session.user.id,
            &session.workspace.id,
            &roots,
            consent.version(),
            1,
        )
        .unwrap();
    let checks = std::sync::atomic::AtomicUsize::new(0);
    let mut host = Host::new(&root);
    let error = host
        .discover(&store, &request, || {
            checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 2
        })
        .unwrap_err();
    assert!(error.contains("Cloud account changed"));
    assert_eq!(checks.load(std::sync::atomic::Ordering::Relaxed), 3);
    assert_eq!(
        store
            .cloud_capture_request("fenced", &session.user.id, &session.workspace.id)
            .unwrap(),
        request
    );
}
