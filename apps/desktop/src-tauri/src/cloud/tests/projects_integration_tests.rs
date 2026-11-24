use super::*;
use crate::cloud::{
    auth::{
        self, AccountSession, Config, EligibleWorkspace, TokenRequest, Tokens, Transport, Vault,
    },
    state::StateStore,
};
use std::{fs, sync::Arc};

const ORIGIN: &str = "https://api.example";

#[test]
fn codex_discovery_continues_after_a_page_with_only_a_failed_header() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let bad_id = "eeeeeeee-1234-1234-1234-123456789abc";
    let good_id = "12345678-1234-1234-1234-123456789abc";
    let path = |id| root.join(format!("rollout-2026-11-19T12-00-00-{id}.jsonl"));
    fs::write(path(bad_id), "invalid header\n").unwrap();
    fs::write(
        path(good_id),
        format!(
            "{}\n",
            serde_json::json!({
                "type": "session_meta", "payload": {"id": good_id, "cwd": root}
            })
        ),
    )
    .unwrap();
    let root = root.to_str().unwrap();
    let first = sync::discover_root_page(SourceKind::CodexJsonl, root, None, 1).unwrap();
    assert_eq!(first.visited, 1);
    assert!(first.candidates.is_empty());
    assert!(first.partial);
    assert_eq!(first.next_cursor.as_deref(), Some(bad_id));

    let next = sync::discover_root_page(
        SourceKind::CodexJsonl,
        root,
        first.next_cursor.as_deref(),
        1,
    )
    .unwrap();
    assert_eq!(next.visited, 1);
    assert!(!next.partial);
    assert!(next.next_cursor.is_none());
    assert_eq!(next.candidates[0].source_id(), format!("codex/{good_id}"));
}

#[test]
fn project_discovery_follows_an_empty_cursor_cli_page_with_a_shared_budget() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap().join(".cursor");
    let transcripts = root.join("projects/synthetic-project/agent-transcripts");
    for index in 0..17 {
        let id = format!("session-{index:02}");
        let directory = transcripts.join(&id);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(format!("{id}.jsonl")), b"synthetic").unwrap();
    }
    // The workspace key is the MD5 of "/synthetic-project".
    let sidecar = root.join("chats/33fc6c2a7c8837fa73e8bd8dc4fcee23/session-16");
    fs::create_dir_all(&sidecar).unwrap();
    fs::write(
        sidecar.join("meta.json"),
        r#"{"schemaVersion":1,"hasConversation":true,"cwd":"/synthetic-project"}"#,
    )
    .unwrap();
    let draft = sidecar.with_file_name("session-00");
    fs::create_dir_all(&draft).unwrap();
    fs::write(
        draft.join("meta.json"),
        r#"{"schemaVersion":1,"hasConversation":false,"cwd":"/synthetic-project"}"#,
    )
    .unwrap();
    let selection = SourceSelection {
        source_kind: SourceKind::CursorCliAgentJsonl,
        path: transcripts.to_str().unwrap().into(),
    };
    let mut scans = ProjectScans::new(vec![&selection]);
    scans.remaining = 16;
    let (_, limited) = scans.next(&|| true, |_| true, |_| unreachable!()).unwrap();
    let limited = limited.unwrap();
    assert_eq!(scans.remaining, 0);
    assert!(scans.root_incomplete(0));
    assert!(limited.candidates.is_empty());

    let mut scans = ProjectScans::new(vec![&selection]);
    scans.remaining = 17;
    let (_, complete) = scans.next(&|| true, |_| true, |_| unreachable!()).unwrap();
    let complete = complete.unwrap();
    assert_eq!(scans.remaining, 0);
    assert!(!scans.root_incomplete(0));
    assert!(!complete.partial);
    assert_eq!(complete.candidates[0].source_id(), "cursor/cli/session-16");
}

#[test]
fn project_discovery_keeps_a_later_cursor_root_visible_after_512_mirrors() {
    let mut fixture = Fixture::new();
    let base = fixture.directory.path().canonicalize().unwrap();
    let mirrors = base.join("a/.cursor");
    let mirror_root = mirrors.join("projects/mirror-project/agent-transcripts");
    fs::create_dir_all(mirrors.join("chats")).unwrap();
    for index in 0..512 {
        let id = format!("mirror-{index:03}");
        let directory = mirror_root.join(&id);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(format!("{id}.jsonl")), b"synthetic").unwrap();
    }
    let healthy = base.join("b/.cursor");
    let healthy_root = healthy.join("projects/synthetic-project/agent-transcripts");
    let transcript = healthy_root.join("healthy");
    let sidecar = healthy.join("chats/33fc6c2a7c8837fa73e8bd8dc4fcee23/healthy");
    fs::create_dir_all(&transcript).unwrap();
    fs::create_dir_all(&sidecar).unwrap();
    fs::write(transcript.join("healthy.jsonl"), b"synthetic").unwrap();
    fs::write(
        sidecar.join("meta.json"),
        r#"{"schemaVersion":1,"hasConversation":true,"cwd":"/synthetic-project"}"#,
    )
    .unwrap();
    let discovery = fixture.discover(
        [mirror_root, healthy_root]
            .into_iter()
            .map(|path| SourceSelection {
                source_kind: SourceKind::CursorCliAgentJsonl,
                path: path.to_str().unwrap().into(),
            })
            .collect(),
    );
    assert!(!discovery.partial);
    assert_eq!(discovery.source_roots.len(), 2);
}

#[test]
fn git_path_records_accept_line_endings_but_not_embedded_controls() {
    let path = Path::new("checkout with spaces");
    assert_eq!(git_path_record("checkout with spaces\n"), Some(path));
    assert_eq!(git_path_record("checkout with spaces\r\n"), Some(path));
    for invalid in [
        "checkout", "\r\n", "a\rb\n", "a\nb\r\n", "a\tb\r\n", "a\r\r\n",
    ] {
        assert_eq!(git_path_record(invalid), None);
    }
}

#[test]
fn cloud_project_groups_keep_only_registered_worktrees_in_both_git_formats() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let main = root.join("main checkout");
    let linked = root.join("linked checkout");
    let stale = root.join("replaced checkout");
    let missing = root.join("missing checkout");
    let moved = root.join("moved checkout");
    let alias = root.join("alias checkout");
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&main)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .current_dir(&main)
            .args([
                "-c",
                "user.name=Synthetic",
                "-c",
                "user.email=synthetic@example.test",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "initial",
            ])
            .status()
            .unwrap()
            .success()
    );
    for checkout in [&linked, &stale, &missing] {
        assert!(
            std::process::Command::new("git")
                .current_dir(&main)
                .args(["worktree", "add", "--quiet", "--detach"])
                .arg(checkout)
                .status()
                .unwrap()
                .success()
        );
    }
    fs::rename(&missing, &moved).unwrap();
    fs::remove_file(stale.join(".git")).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&stale)
            .status()
            .unwrap()
            .success()
    );
    fs::create_dir(&alias).unwrap();
    fs::copy(linked.join(".git"), alias.join(".git")).unwrap();
    let nested = linked.join("src");
    fs::create_dir(&nested).unwrap();
    let expected = [linked.to_str().unwrap(), main.to_str().unwrap()];

    let mut confirmation = ConfirmationContexts::default();
    let primary = confirmation.get(main.to_str().unwrap());
    assert!(Rc::ptr_eq(
        &primary,
        &confirmation.get(linked.to_str().unwrap())
    ));
    assert!(!confirmation.0.contains_key(nested.to_str().unwrap()));
    assert!(!confirmation.0.contains_key(alias.to_str().unwrap()));
    assert!(!Rc::ptr_eq(
        &primary,
        &confirmation.get(nested.to_str().unwrap())
    ));
    assert!(!confirmation.get(alias.to_str().unwrap()).complete);

    for path in [&main, &nested] {
        let grouped = context("native", path.to_str().unwrap());
        assert_eq!(grouped.main, main.to_str().unwrap());
        assert_eq!(grouped.folders, expected);
        assert!(grouped.complete);
    }
    let replaced = context("native", stale.to_str().unwrap());
    assert_eq!(replaced.folders, [stale.to_str().unwrap()]);
    assert!(replaced.complete);
    assert!(
        context("native", alias.to_str().unwrap())
            .folders
            .is_empty()
    );
    assert!(
        context("native", missing.to_str().unwrap())
            .folders
            .is_empty()
    );
    assert!(
        context("wsl:synthetic", nested.to_str().unwrap())
            .folders
            .is_empty()
    );

    tauri::async_runtime::block_on(async {
        let environment = antiburn_local::platform::environment::DiscoveryEnvironment::Native;
        let common = worktree_common_dir(&environment, &linked).await.unwrap();
        let output = antiburn_local::platform::git::run_git_output_in_environment(
            &environment,
            Some(&linked),
            &["worktree", "list", "--porcelain"],
            &[],
        )
        .await
        .unwrap();
        assert!(output.status.success());
        let list = std::str::from_utf8(&output.stdout).unwrap();
        let grouped = worktree_context(list, '\n', &linked, &common).unwrap();
        assert_eq!(grouped.main, main.to_str().unwrap());
        assert_eq!(grouped.folders, expected);
        assert!(grouped.complete);

        let unverified_parent = root.join("unverified parent");
        fs::create_dir(&unverified_parent).unwrap();
        fs::write(
            unverified_parent.join(".git"),
            format!("gitdir: {}\n", common.display()),
        )
        .unwrap();
        let (_, records) = list.split_once('\n').unwrap();
        for field in ["", "bare\n"] {
            let forged = format!("worktree {}\n{field}{records}", unverified_parent.display());
            let fallback = worktree_context(&forged, '\n', &linked, &common).unwrap();
            assert_eq!(fallback.main, linked.to_str().unwrap());
            assert_eq!(fallback.folders, [linked.to_str().unwrap()]);
            assert!(!fallback.complete);
        }
    });
    assert!(
        std::process::Command::new("git")
            .current_dir(&main)
            .args(["worktree", "prune", "--expire", "now"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        context("native", moved.to_str().unwrap())
            .folders
            .is_empty()
    );

    let mut fixture = Fixture::new();
    let plain = context("native", &fixture.project);
    assert_eq!(plain.folders, [fixture.project.clone()]);
    assert!(plain.complete);
    fixture.save_source("synthetic-user", "synthetic-personal", true);
    rusqlite::Connection::open(&fixture.source.path)
        .unwrap()
        .execute(
            "UPDATE session_v2 SET directory=?1",
            [nested.to_str().unwrap()],
        )
        .unwrap();
    let discovery = fixture.discover(Vec::new());
    assert_eq!(discovery.groups.len(), 1);
    let group = &discovery.groups[0];
    assert_eq!(group.path, main.to_str().unwrap());
    assert_eq!(group.folder_paths, expected);
    assert!(group.capture_supported);
    assert_eq!(
        group
            .projects
            .iter()
            .map(|project| project.project_path.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    let binding = fixture.binding();
    let choices = BTreeMap::from([(group.id.clone(), true)]);
    let projects = group.projects.iter().cloned().collect();
    confirm_project_choices(
        &fixture.store,
        &mut fixture.cache,
        &binding,
        &choices,
        &projects,
        &|| true,
    )
    .unwrap();
    fs::write(linked.join(".git"), "invalid pointer\n").unwrap();
    assert_eq!(
        confirm_project_choices(
            &fixture.store,
            &mut fixture.cache,
            &binding,
            &choices,
            &projects,
            &|| true,
        )
        .unwrap_err(),
        "The project folders changed. Reload projects and confirm again."
    );

    let bare = root.join("bare.git");
    let bare_linked = root.join("bare checkout");
    assert!(
        std::process::Command::new("git")
            .args(["clone", "--local", "--quiet", "--bare"])
            .arg(&main)
            .arg(&bare)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .current_dir(&bare)
            .args(["worktree", "add", "--quiet", "--detach"])
            .arg(&bare_linked)
            .status()
            .unwrap()
            .success()
    );
    let grouped = context("native", bare_linked.to_str().unwrap());
    assert_eq!(grouped.main, bare.to_str().unwrap());
    assert_eq!(grouped.folders, [bare_linked.to_str().unwrap()]);
    assert!(grouped.complete);
}

#[test]
fn project_discovery_reports_git_inspection_failure_without_selecting_the_folder() {
    let mut fixture = Fixture::new();
    fixture.save_source("synthetic-user", "synthetic-personal", true);
    let root = fixture.directory.path().canonicalize().unwrap();
    let missing = root.join("missing");
    let plain = context("native", &fixture.project);
    assert!(plain.complete);
    assert!(!plain.inspection_incomplete);
    let missing = missing.to_str().unwrap();
    let missing_context = context("native", missing);
    assert!(!missing_context.complete);
    assert!(!missing_context.inspection_incomplete);
    #[cfg(unix)]
    {
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&fixture.project, &alias).unwrap();
        let alias = context("native", alias.to_str().unwrap());
        assert!(!alias.complete);
        assert!(!alias.inspection_incomplete);
    }
    for error in [
        std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        std::io::Error::from(std::io::ErrorKind::Other),
    ] {
        let checked = context_for_path(&fixture.project, Err(error));
        assert!(checked.inspection_incomplete);
        assert!(!checked.complete);
        assert!(checked.folders.is_empty());
    }
    fixture
        .store
        .lock()
        .execute(
            "INSERT INTO session(environment_key,agent,session_id,source_kind,source_label,cwd,
                 first_seen_at,last_seen_at,updated_at_epoch)
             VALUES('native','opencode','missing','providerDb','missing',?1,
                 'synthetic','synthetic',1)",
            [missing],
        )
        .unwrap();
    rusqlite::Connection::open(&fixture.source.path)
        .unwrap()
        .execute("UPDATE session_v2 SET directory=?1", [missing])
        .unwrap();
    let discovery = fixture.discover(Vec::new());
    assert!(!discovery.partial);
    assert!(discovery.groups.is_empty());
    for marker in ["invalid pointer\n", "gitdir: not-a-repository\n"] {
        let broken = tempfile::tempdir_in(&root).unwrap();
        let broken = broken.path().to_str().unwrap();
        fs::write(Path::new(broken).join(".git"), marker).unwrap();
        let checked = context("native", broken);
        assert!(checked.inspection_incomplete);
        assert!(checked.folders.is_empty());
        rusqlite::Connection::open(&fixture.source.path)
            .unwrap()
            .execute("UPDATE session_v2 SET directory=?1", [broken])
            .unwrap();
        let discovery = fixture.discover(Vec::new());
        assert!(discovery.partial);
        assert_eq!(discovery.groups.len(), 1);
        let group = &discovery.groups[0];
        assert!(!group.capture_supported);
        assert!(group.projects.is_empty());
    }
}

#[test]
fn settings_project_removes_unavailable_bindings_by_exact_folder_and_group_id() {
    let main = "/unavailable/project";
    let worktree = "/unavailable/worktree";
    let saved = [
        ProjectSelection {
            source_kind: SourceKind::OpenCodeSqliteV2,
            source_path: "/unavailable/opencode.db".into(),
            project_path: main.into(),
        },
        ProjectSelection {
            source_kind: SourceKind::ClaudeJsonl,
            source_path: "/unavailable/claude".into(),
            project_path: worktree.into(),
        },
    ];
    let id = group_id("native", main);
    assert_eq!(
        group_projects_to_remove(&id, &[worktree.into()], &saved).unwrap(),
        saved.iter().cloned().collect()
    );
    assert!(group_projects_to_remove(&id, &[], &saved).is_err());
    let mut overlapping = saved.to_vec();
    overlapping.push(ProjectSelection {
        project_path: "/unavailable".into(),
        ..saved[0].clone()
    });
    assert!(group_projects_to_remove(&id, &[worktree.into()], &overlapping).is_err());
}

struct Authority;

impl Transport for Authority {
    fn exchange(&self, _: &Config, _: &TokenRequest<'_>) -> Result<Tokens, auth::Failure> {
        Ok(Tokens {
            access_token: "synthetic-access".into(),
            refresh_token: "synthetic-refresh".into(),
            expires_in: 900,
            token_type: "Bearer".into(),
            grant_id: "synthetic-grant".into(),
            user_id: "synthetic-user".into(),
            legacy_workspace_claim: false,
            grant_expires_at: "1970-01-01T02:46:40Z".into(),
            scope: "capture:write".into(),
            resource: ORIGIN.into(),
            iss: ORIGIN.into(),
        })
    }
    fn session(&self, _: &Config, _: &str) -> Result<AccountSession, auth::Failure> {
        Ok(crate::cloud::tests::session().into())
    }
    fn workspaces(&self, _: &Config, _: &str) -> Result<Vec<EligibleWorkspace>, auth::Failure> {
        Ok(vec![EligibleWorkspace {
            workspace: crate::cloud::tests::session().workspace,
            membership_generation: None,
        }])
    }
    fn revoke(&self, _: &Config, _: &str) -> Result<(), auth::Failure> {
        Ok(())
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    store: Store,
    engine: Engine,
    cache: Cache,
    source: SourceSelection,
    project: String,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let source_parent = base.join("custom-provider");
        let project = base.join("project");
        fs::create_dir(&source_parent).unwrap();
        fs::create_dir(&project).unwrap();
        let database = source_parent.join("custom.db");
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(include_str!(
                "../../../../../../fixtures/cloud/opencode/current.sql"
            ))
            .unwrap();
        connection
            .execute(
                "UPDATE session_v2 SET directory=?1",
                [project.to_str().unwrap()],
            )
            .unwrap();
        drop(connection);
        let store = Store::open(&base).unwrap();
        store.initialize_cloud_setup().unwrap();
        let vault: Box<dyn Vault> = Box::new(Arc::new(crate::cloud::tests::MemoryVault::default()));
        let mut engine = Engine::new(
            Config::parse(ORIGIN, ORIGIN),
            vault,
            Box::new(Authority),
            StateStore::new(&base),
        )
        .with_clock(|| 100);
        let (_, _listener, attempt) = engine.begin(100, 0).unwrap();
        engine
            .callback(
                &attempt,
                auth::Callback::Code("synthetic".into()),
                100,
                || true,
            )
            .unwrap();
        let project = project.to_str().unwrap().to_owned();
        let mut cache = Cache::default();
        cache.contexts.insert(
            ("native".into(), project.clone()),
            (
                Instant::now(),
                Context {
                    main: project.clone(),
                    folders: vec![project.clone()],
                    complete: true,
                    inspection_incomplete: false,
                },
            ),
        );
        Self {
            directory,
            store,
            engine,
            cache,
            project,
            source: SourceSelection {
                source_kind: SourceKind::OpenCodeSqliteV2,
                path: database.to_str().unwrap().into(),
            },
        }
    }

    fn binding(&self) -> DestinationBinding {
        DestinationBinding {
            account_id: "synthetic-user".into(),
            destination_id: "synthetic-personal".into(),
        }
    }

    fn save_source(&self, account: &str, destination: &str, selected: bool) {
        let draft = Draft {
            folders: vec![Folder {
                source_kind: self.source.source_kind,
                path: self.source.path.clone(),
                selected,
                ..Folder::default()
            }],
            ..Draft::default()
        };
        self.store
            .save_cloud_draft(
                account,
                destination,
                &serde_json::to_string(&draft).unwrap(),
            )
            .unwrap();
    }

    fn discover(&mut self, known: Vec<SourceSelection>) -> Discovery {
        let binding = self.binding();
        discover_projects(
            &self.store,
            &mut self.cache,
            &self.engine,
            &binding,
            None,
            (known, false),
            &|| true,
        )
        .unwrap()
    }
}

#[test]
fn first_save_checks_current_group_and_explicit_source() {
    let mut fixture = Fixture::new();
    fixture.save_source("synthetic-user", "synthetic-personal", true);
    let discovery = fixture.discover(Vec::new());
    let group = &discovery.groups[0];
    let projects = group.projects.iter().cloned().collect();
    let binding = fixture.binding();
    let choices = BTreeMap::from([(group.id.clone(), true)]);
    let unselected = BTreeMap::from([(group.id.clone(), false)]);
    assert!(
        confirm_project_choices(
            &fixture.store,
            &mut fixture.cache,
            &binding,
            &unselected,
            &projects,
            &|| true,
        )
        .is_err()
    );
    assert!(
        confirm_project_choices(
            &fixture.store,
            &mut fixture.cache,
            &binding,
            &choices,
            &projects,
            &|| false,
        )
        .is_err()
    );
    assert!(
        confirm_project_choices(
            &fixture.store,
            &mut fixture.cache,
            &binding,
            &choices,
            &projects,
            &|| true,
        )
        .is_ok()
    );
    fixture.save_source("synthetic-user", "synthetic-personal", false);
    assert!(
        confirm_project_choices(
            &fixture.store,
            &mut fixture.cache,
            &binding,
            &choices,
            &projects,
            &|| true,
        )
        .is_err()
    );
}

#[test]
fn first_save_checks_more_than_one_page_of_opencode_folders() {
    let mut fixture = Fixture::new();
    fixture.save_source("synthetic-user", "synthetic-personal", true);
    let base = fixture.directory.path().canonicalize().unwrap();
    let connection = rusqlite::Connection::open(&fixture.source.path).unwrap();
    let mut choices = BTreeMap::new();
    let mut projects = BTreeSet::new();
    for index in 0..33 {
        let project = base.join(format!("project-{index:02}"));
        fs::create_dir(&project).unwrap();
        let project = project.to_str().unwrap();
        let id = format!("synthetic-project-{index:02}");
        connection
            .execute(
                "INSERT INTO session_v2(id,project_id,slug,directory,version,time_created,time_updated)
                 VALUES (?1, 'global', ?1, ?2, '2.3.0', 1, 2)",
                rusqlite::params![id, project],
            )
            .unwrap();
        choices.insert(group_id("native", project), true);
        projects.insert(ProjectSelection {
            source_kind: fixture.source.source_kind,
            source_path: fixture.source.path.clone(),
            project_path: project.into(),
        });
    }
    drop(connection);
    let binding = fixture.binding();
    confirm_project_choices(
        &fixture.store,
        &mut fixture.cache,
        &binding,
        &choices,
        &projects,
        &|| true,
    )
    .unwrap();
}

#[test]
fn first_save_accepts_a_folder_with_complete_evidence_from_only_one_source() {
    let mut fixture = Fixture::new();
    fixture.save_source("synthetic-user", "synthetic-personal", true);
    let claude = fixture
        .directory
        .path()
        .canonicalize()
        .unwrap()
        .join("claude");
    fs::create_dir(&claude).unwrap();
    let transcript = claude.join("synthetic-claude.jsonl");
    fs::write(
        &transcript,
        format!(
            "{}\nnot-json\n",
            serde_json::json!({
                "type": "user",
                "sessionId": "synthetic-claude",
                "cwd": fixture.project,
                "message": {"role": "user", "content": "Synthetic fixture"}
            })
        ),
    )
    .unwrap();
    fixture
        .store
        .lock()
        .execute(
            "INSERT INTO session(environment_key,agent,session_id,source_kind,source_label,cwd,
                first_seen_at,last_seen_at,updated_at_epoch)
             VALUES('native','claude-code','synthetic-claude','file',?1,?2,
                'synthetic','synthetic',1)",
            rusqlite::params![transcript.to_str().unwrap(), fixture.project],
        )
        .unwrap();
    let known = known_sources(&fixture.store, Vec::new()).unwrap();
    let discovery = fixture.discover(known.0);
    assert!(discovery.partial);
    let group = &discovery.groups[0];
    assert!(group.capture_supported);
    let projects = group.projects.iter().cloned().collect();
    let choices = BTreeMap::from([(group.id.clone(), true)]);
    let binding = fixture.binding();
    confirm_project_choices(
        &fixture.store,
        &mut fixture.cache,
        &binding,
        &choices,
        &projects,
        &|| true,
    )
    .unwrap();
    assert_eq!(group.projects.len(), 1);
    assert_eq!(group.projects[0].source_kind, fixture.source.source_kind);
}

#[test]
fn cloud_project_discovery_selected_custom_opencode_file_produces_groups_without_upload_consent() {
    let mut fixture = Fixture::new();
    let before = fs::read(&fixture.source.path).unwrap();
    let undiscovered = fixture.discover(Vec::new());
    assert!(undiscovered.groups.is_empty());
    assert!(undiscovered.source_roots.is_empty());
    fixture.save_source("synthetic-user", "synthetic-personal", false);
    assert!(fixture.discover(Vec::new()).source_roots.is_empty());
    fixture.save_source("synthetic-user", "synthetic-personal", true);
    let discovery = fixture.discover(Vec::new());
    assert!(!discovery.partial);
    assert_eq!(discovery.source_roots, vec![fixture.source.clone()]);
    assert_eq!(discovery.groups.len(), 1);
    let group = &discovery.groups[0];
    assert_eq!(group.path, fixture.project);
    assert!(group.capture_supported);
    assert_eq!(group.capture_session_count, 4);
    assert_eq!(
        group.projects,
        vec![ProjectSelection {
            source_kind: SourceKind::OpenCodeSqliteV2,
            source_path: fixture.source.path.clone(),
            project_path: fixture.project.clone(),
        }]
    );
    assert!(
        fixture
            .store
            .cloud_sync_policy("synthetic-user", "synthetic-personal", ORIGIN)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .store
            .cloud_sync_jobs("synthetic-user", "synthetic-personal", ORIGIN)
            .unwrap()
            .is_empty()
    );
    assert!(!fixture.directory.path().join("cloud-capture-v1").exists());
    assert_eq!(fs::read(&fixture.source.path).unwrap(), before);
}

#[test]
fn cloud_project_discovery_uses_default_opencode_source_without_saving_consent() {
    let mut fixture = Fixture::new();
    let binding = fixture.binding();
    let known = known_sources(&fixture.store, [PathBuf::from(&fixture.source.path)]).unwrap();
    let discovery = discover_projects(
        &fixture.store,
        &mut fixture.cache,
        &fixture.engine,
        &binding,
        None,
        known,
        &|| true,
    )
    .unwrap();

    assert!(!discovery.partial);
    assert_eq!(discovery.source_roots, vec![fixture.source.clone()]);
    assert!(discovery.groups.iter().any(|group| group.capture_supported));
    assert!(
        fixture
            .store
            .cloud_draft("synthetic-user", "synthetic-personal")
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .store
            .cloud_sync_policy("synthetic-user", "synthetic-personal", ORIGIN)
            .unwrap()
            .is_none()
    );
}

#[test]
fn cloud_project_discovery_saved_provider_opt_out_overrides_automatic_roots() {
    let mut fixture = Fixture::new();
    fixture.save_source("synthetic-user", "synthetic-personal", false);
    let discovery = fixture.discover(vec![fixture.source.clone()]);
    assert!(discovery.source_roots.is_empty());
    assert!(!discovery.groups.iter().any(|group| group.capture_supported));
    fixture.save_source("synthetic-user", "synthetic-personal", true);
    let discovery = fixture.discover(vec![fixture.source.clone(), fixture.source.clone()]);
    assert_eq!(discovery.source_roots, vec![fixture.source.clone()]);
    assert_eq!(discovery.groups[0].capture_session_count, 4);
}

#[test]
fn cloud_project_discovery_does_not_reuse_another_accounts_saved_provider_access() {
    let mut fixture = Fixture::new();
    fixture.save_source("another-account", "synthetic-personal", true);
    fixture.save_source("synthetic-user", "another-workspace", true);
    assert!(fixture.discover(Vec::new()).source_roots.is_empty());
    let wrong = DestinationBinding {
        account_id: "another-account".into(),
        destination_id: "synthetic-personal".into(),
    };
    assert!(
        discover_projects(
            &fixture.store,
            &mut fixture.cache,
            &fixture.engine,
            &wrong,
            None,
            (Vec::new(), false),
            &|| true
        )
        .is_err()
    );
    fixture.save_source("synthetic-user", "synthetic-personal", true);
    let current = fixture.binding();
    assert!(
        discover_projects(
            &fixture.store,
            &mut fixture.cache,
            &fixture.engine,
            &current,
            None,
            (Vec::new(), false),
            &|| false
        )
        .is_err()
    );
}

#[test]
fn cloud_project_discovery_merges_by_provider_and_canonical_path_with_opt_out_precedence() {
    let fixture = Fixture::new();
    let alias = Path::new(&fixture.source.path)
        .parent()
        .unwrap()
        .join(".")
        .join("custom.db");
    let selected = Folder {
        path: fixture.source.path.clone(),
        source_kind: SourceKind::OpenCodeSqliteV2,
        selected: true,
        ..Folder::default()
    };
    let excluded = Folder {
        path: alias.to_str().unwrap().into(),
        selected: false,
        ..selected.clone()
    };
    let (roots, partial) =
        merge_sources(vec![fixture.source.clone()], &[selected.clone(), excluded]).unwrap();
    assert!(roots.is_empty());
    assert!(!partial);
    let other_provider = Folder {
        source_kind: SourceKind::ClaudeJsonl,
        selected: false,
        ..selected
    };
    let (roots, _) = merge_sources(vec![fixture.source.clone()], &[other_provider]).unwrap();
    assert_eq!(roots, vec![fixture.source.clone()]);
}

#[test]
fn cloud_project_discovery_keeps_explicit_custom_access_within_existing_root_bound() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let mut known = Vec::new();
    for index in 0..100 {
        let path = base.join(format!("default-{index:03}.db"));
        fs::write(&path, b"synthetic path metadata only").unwrap();
        known.push(SourceSelection {
            source_kind: SourceKind::OpenCodeSqliteV2,
            path: path.to_str().unwrap().into(),
        });
    }
    let path = base.join("zzz-custom.db");
    fs::write(&path, b"synthetic path metadata only").unwrap();
    let folder = Folder {
        path: path.to_str().unwrap().into(),
        source_kind: SourceKind::OpenCodeSqliteV2,
        selected: true,
        ..Folder::default()
    };
    let (roots, partial) = merge_sources(known, &[folder]).unwrap();
    assert!(partial);
    assert_eq!(roots.len(), 100);
    assert!(roots.iter().any(|root| root.path == path.to_str().unwrap()));
}

#[test]
fn cloud_project_discovery_uses_recorded_claude_code_sources_at_the_production_boundary() {
    let mut fixture = Fixture::new();
    let source = fixture
        .directory
        .path()
        .canonicalize()
        .unwrap()
        .join("claude");
    fs::create_dir(&source).unwrap();
    let transcript = source.join("synthetic-claude.jsonl");
    fs::write(
        &transcript,
        format!(
            "{}\n",
            serde_json::json!({
                "type": "user",
                "sessionId": "synthetic-claude",
                "cwd": fixture.project,
                "message": {"role": "user", "content": "Synthetic fixture"}
            })
        ),
    )
    .unwrap();
    fixture
        .store
        .lock()
        .execute(
            "INSERT INTO session(environment_key,agent,session_id,source_kind,source_label,cwd,
                first_seen_at,last_seen_at,updated_at_epoch)
             VALUES('native','claude-code','synthetic-claude','file',?1,?2,
                'synthetic','synthetic',1)",
            rusqlite::params![transcript.to_str().unwrap(), fixture.project],
        )
        .unwrap();
    fixture.cache = Cache::default();
    let known = known_sources(&fixture.store, Vec::new()).unwrap();
    assert!(!known.1);
    assert_eq!(
        known.0,
        vec![SourceSelection {
            source_kind: SourceKind::ClaudeJsonl,
            path: source.to_str().unwrap().into(),
        }]
    );
    let candidate = antiburn_cloud_capture::reopen_claude(
        &source,
        "synthetic-claude",
        None,
        &sync::metadata_limits(),
    )
    .unwrap();
    let size = transcript.metadata().unwrap().len();
    fixture.cache.scan_bytes = CLAUDE_SCAN_BYTES - size + 1;
    assert!(
        fixture
            .cache
            .evidence(
                "synthetic-user",
                "synthetic-personal",
                &known.0[0],
                &candidate,
                &sync::metadata_limits(),
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(fixture.cache.scan_bytes, CLAUDE_SCAN_BYTES - size + 1);

    let discovery = fixture.discover(known.0.clone());
    assert!(!discovery.partial);
    assert_eq!(discovery.groups.len(), 1);
    let group = &discovery.groups[0];
    assert_eq!(group.agents, vec!["claude-code"]);
    assert_eq!(group.session_count, 1);
    assert_eq!(group.capture_session_count, 1);
    assert!(group.capture_supported);

    use std::io::Write;
    let record = format!(
        "{}\n",
        serde_json::json!({
            "type": "assistant",
            "sessionId": "synthetic-claude",
            "message": {"role": "assistant", "content": "x".repeat(1024 * 1024)}
        })
    );
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .unwrap();
    for _ in 0..65 {
        file.write_all(record.as_bytes()).unwrap();
    }
    let large_size = file.metadata().unwrap().len();
    drop(file);
    assert!(large_size > CLAUDE_SCAN_BYTES);
    let discovery = fixture.discover(known.0.clone());
    assert!(!discovery.partial);
    let group = &discovery.groups[0];
    assert!(group.capture_supported);
    assert_eq!(fixture.cache.scan_bytes, large_size);
    let mut approval_cache = Cache::default();
    confirm_project_choices(
        &fixture.store,
        &mut approval_cache,
        &fixture.binding(),
        &BTreeMap::from([(group.id.clone(), true)]),
        &group.projects.iter().cloned().collect(),
        &|| true,
    )
    .unwrap();
    assert_eq!(approval_cache.scan_bytes, large_size);

    let first = format!(
        "{}\n",
        serde_json::json!({
            "type": "user", "sessionId": "a-first", "cwd": fixture.project
        })
    );
    fs::write(source.join("a-first.jsonl"), &first).unwrap();
    fixture.cache = Cache::default();
    let discovery = fixture.discover(known.0);
    assert!(discovery.partial);
    assert_eq!(discovery.groups[0].capture_session_count, 1);
    assert_eq!(fixture.cache.scan_bytes, first.len() as u64);
}

#[test]
fn cloud_project_discovery_does_not_report_ordinary_opencode_pages_as_inspection_failures() {
    let mut fixture = Fixture::new();
    let connection = rusqlite::Connection::open(&fixture.source.path).unwrap();
    connection
        .execute_batch(
            "WITH RECURSIVE extra(n) AS (
                VALUES(1) UNION ALL SELECT n+1 FROM extra WHERE n < 509
             )
             INSERT INTO session_v2(id,project_id,slug,directory,version,time_created,time_updated)
             SELECT 'synthetic-extra-' || n, 'global', 'synthetic-extra-' || n,
                '/Example/other', '2.3.0', 1, 2 FROM extra;",
        )
        .unwrap();
    drop(connection);
    let known = known_sources(&fixture.store, vec![PathBuf::from(&fixture.source.path)]).unwrap();
    let discovery = fixture.discover(known.0);
    assert!(!discovery.partial);
    assert_eq!(discovery.source_roots, vec![fixture.source.clone()]);
    assert!(
        discovery
            .groups
            .iter()
            .any(|group| group.path == fixture.project && group.capture_supported)
    );
}

#[test]
fn project_scans_reach_a_later_opencode_page_and_stop_at_4096_candidates() {
    let mut fixture = Fixture::new();
    let root = fixture.directory.path().canonicalize().unwrap();
    let unrelated = root.join("unrelated");
    fs::create_dir(&unrelated).unwrap();
    let connection = rusqlite::Connection::open(&fixture.source.path).unwrap();
    connection
        .execute(
            "WITH RECURSIVE extra(n) AS (
                VALUES(1) UNION ALL SELECT n+1 FROM extra WHERE n < 4097
             )
             INSERT INTO session_v2(id,project_id,slug,directory,version,time_created,time_updated)
             SELECT printf('budget-%04d', n), 'global', printf('budget-%04d', n),
                 CASE WHEN n=513 THEN ?1 ELSE ?2 END, '2.3.0', 1, 2 FROM extra;",
            [&fixture.project, unrelated.to_str().unwrap()],
        )
        .unwrap();
    drop(connection);

    let mut scans = ProjectScans::new(vec![&fixture.source]);
    for page_index in 0..8 {
        let (index, page) = scans
            .next(&|| true, |_| true, |_| Ok(Cow::Borrowed(&[])))
            .unwrap();
        assert_eq!(index, 0);
        let page = page.unwrap();
        assert_eq!(page.visited, 512);
        assert_eq!(page.candidates.len(), 512);
        let first = format!("budget-{:04}", page_index * 512 + 1);
        assert_eq!(page.candidates[0].native_session_id(), first);
        let last = format!("budget-{:04}", (page_index + 1) * 512);
        assert_eq!(page.candidates[511].native_session_id(), last);
        assert!(page.next_cursor.is_some());
    }
    assert_eq!(scans.remaining, 0);
    assert!(scans.root_incomplete(0));
    assert!(
        scans
            .next(&|| true, |_| true, |_| Ok(Cow::Borrowed(&[])))
            .is_none()
    );

    fixture.save_source("synthetic-user", "synthetic-personal", true);
    let checks = std::cell::Cell::new(0);
    let current = || {
        checks.set(checks.get() + 1);
        true
    };
    confirm_project_choices(
        &fixture.store,
        &mut Cache::default(),
        &fixture.binding(),
        &BTreeMap::from([(group_id("native", &fixture.project), true)]),
        &BTreeSet::from([ProjectSelection {
            source_kind: fixture.source.source_kind,
            source_path: fixture.source.path.clone(),
            project_path: fixture.project.clone(),
        }]),
        &current,
    )
    .unwrap();
    assert!(checks.get() < 600, "approval must stop after finding proof");

    rusqlite::Connection::open(&fixture.source.path)
        .unwrap()
        .execute(
            "UPDATE session_v2 SET directory=CASE WHEN id='budget-4097' THEN ?1 ELSE ?2 END",
            [&fixture.project, unrelated.to_str().unwrap()],
        )
        .unwrap();
    let discovery = fixture.discover(Vec::new());
    assert!(discovery.partial);
    assert!(
        !discovery
            .groups
            .iter()
            .any(|group| group.path == fixture.project && group.capture_supported)
    );
}

#[test]
fn cloud_project_discovery_and_save_spend_unused_root_budget_on_late_folders() {
    let mut fixture = Fixture::new();
    let base = fixture.directory.path().canonicalize().unwrap();
    let outside = base.join("outside");
    let sibling_folder = base.join("sibling");
    fs::create_dir(&outside).unwrap();
    fs::create_dir(&sibling_folder).unwrap();
    let sibling = SourceSelection {
        source_kind: SourceKind::OpenCodeSqliteV2,
        path: base.join("later.db").to_str().unwrap().into(),
    };
    fs::copy(&fixture.source.path, &sibling.path).unwrap();
    rusqlite::Connection::open(&sibling.path)
        .unwrap()
        .execute(
            "UPDATE session_v2 SET directory=?1",
            [sibling_folder.to_str().unwrap()],
        )
        .unwrap();
    rusqlite::Connection::open(&fixture.source.path)
        .unwrap()
        .execute(
            "WITH RECURSIVE extra(n) AS (
                VALUES(1) UNION ALL SELECT n+1 FROM extra WHERE n < 300
             )
             INSERT INTO session_v2(id,project_id,slug,directory,version,time_created,time_updated)
             SELECT printf('before-%04d',n), 'global', 'synthetic-extra-' || n,
                 ?1, '2.3.0', 1, 2 FROM extra;",
            [outside.to_str().unwrap()],
        )
        .unwrap();
    let roots = vec![fixture.source.clone(), sibling];
    let discovery = fixture.discover(roots.clone());
    assert!(!discovery.partial);
    assert_eq!(discovery.source_roots, roots);
    let mut choices = BTreeMap::new();
    let mut projects = BTreeSet::new();
    for path in [Path::new(&fixture.project), sibling_folder.as_path()] {
        let group = discovery
            .groups
            .iter()
            .find(|group| group.path == path.to_str().unwrap())
            .expect("both the late folder and the sibling must be present");
        assert!(group.capture_supported);
        assert_eq!(group.capture_session_count, 4);
        choices.insert(group.id.clone(), true);
        projects.extend(group.projects.iter().cloned());
    }
    let binding = fixture.binding();
    let draft = Draft {
        folders: roots
            .into_iter()
            .map(|root| Folder {
                source_kind: root.source_kind,
                path: root.path,
                selected: true,
                ..Folder::default()
            })
            .collect(),
        ..Draft::default()
    };
    fixture
        .store
        .save_cloud_draft(
            &binding.account_id,
            &binding.destination_id,
            &serde_json::to_string(&draft).unwrap(),
        )
        .unwrap();
    confirm_project_choices(
        &fixture.store,
        &mut fixture.cache,
        &binding,
        &choices,
        &projects,
        &|| true,
    )
    .unwrap();
}

#[test]
fn cloud_project_discovery_qualifies_cached_opencode_ids_beyond_the_first_large_source_page() {
    let mut fixture = Fixture::new();
    let outside = fixture
        .directory
        .path()
        .canonicalize()
        .unwrap()
        .join("outside");
    fs::create_dir(&outside).unwrap();
    let connection = rusqlite::Connection::open(&fixture.source.path).unwrap();
    connection
        .execute(
            "WITH RECURSIVE extra(n) AS (
                VALUES(1) UNION ALL SELECT n+1 FROM extra WHERE n < 2417
             )
             INSERT INTO session_v2(id,project_id,slug,directory,version,time_created,time_updated)
             SELECT printf('before-%04d',n), 'global', 'synthetic-extra-' || n,
                ?1, '2.3.0', 1, 2 FROM extra;",
            [outside.to_str().unwrap()],
        )
        .unwrap();
    fixture
        .store
        .lock()
        .execute(
            "INSERT INTO session(environment_key,agent,session_id,source_kind,source_label,
                source_format,cwd,first_seen_at,last_seen_at,updated_at_epoch)
             VALUES('native','opencode','ses_main','providerDb',
                'opencode:ses_main:open_code_v2_sqlite','\"open_code_v2_sqlite\"',?1,
                'synthetic','synthetic',1)",
            [&fixture.project],
        )
        .unwrap();
    let known = known_sources(&fixture.store, vec![PathBuf::from(&fixture.source.path)]).unwrap();
    let discovery = fixture.discover(known.0.clone());
    let selected = discovery
        .groups
        .iter()
        .find(|group| group.path == fixture.project)
        .unwrap();
    assert!(!discovery.partial);
    assert!(selected.capture_supported);
    assert_eq!(selected.capture_session_count, 4);
    assert_eq!(discovery.source_roots, vec![fixture.source.clone()]);
    assert!(
        discovery
            .groups
            .iter()
            .map(|group| group.capture_session_count)
            .sum::<u32>()
            <= 4096
    );
    let next_source = fixture
        .directory
        .path()
        .canonicalize()
        .unwrap()
        .join("later.db");
    fs::copy(&fixture.source.path, &next_source).unwrap();
    let later = SourceSelection {
        source_kind: SourceKind::OpenCodeSqliteV2,
        path: next_source.to_str().unwrap().into(),
    };
    let bounded = fixture.discover(vec![fixture.source.clone(), later.clone()]);
    assert!(bounded.partial);
    assert_eq!(bounded.source_roots, vec![fixture.source.clone(), later]);
    assert!(
        bounded
            .groups
            .iter()
            .map(|group| group.capture_session_count)
            .sum::<u32>()
            <= 4096
    );

    connection
        .execute(
            "UPDATE session_v2 SET directory=?1",
            [outside.to_str().unwrap()],
        )
        .unwrap();
    let changed = fixture.discover(known.0.clone());
    assert!(
        !changed
            .groups
            .iter()
            .find(|group| group.path == fixture.project)
            .unwrap()
            .capture_supported
    );
    connection
        .execute_batch(
            "CREATE TABLE session_unknown(session_id TEXT);
             INSERT INTO session_unknown VALUES ('ses_main');",
        )
        .unwrap();
    let unsupported = fixture.discover(known.0);
    assert!(unsupported.partial);
    assert!(unsupported.source_roots.is_empty());
    assert!(
        !unsupported
            .groups
            .iter()
            .any(|group| group.capture_supported)
    );
}

#[test]
fn cloud_project_discovery_missing_cached_ids_are_hints_not_source_inspection_failures() {
    let mut fixture = Fixture::new();
    fixture
        .store
        .lock()
        .execute(
            "INSERT INTO session(environment_key,agent,session_id,source_kind,source_label,
                source_format,cwd,first_seen_at,last_seen_at,updated_at_epoch)
             VALUES('native','opencode','ses_missing','providerDb',
                'opencode:ses_missing:open_code_v2_sqlite','\"open_code_v2_sqlite\"',?1,
                'synthetic','synthetic',1)",
            [&fixture.project],
        )
        .unwrap();
    let discovery = fixture.discover(vec![fixture.source.clone()]);
    assert!(!discovery.partial);
    assert_eq!(discovery.source_roots, vec![fixture.source.clone()]);
    assert!(discovery.groups[0].capture_supported);
    assert_eq!(discovery.groups[0].capture_session_count, 4);
}
