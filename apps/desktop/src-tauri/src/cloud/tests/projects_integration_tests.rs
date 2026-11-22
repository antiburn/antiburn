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
    let discovery = fixture.discover(known.0);
    assert!(!discovery.partial);
    assert_eq!(discovery.groups.len(), 1);
    let group = &discovery.groups[0];
    assert_eq!(group.agents, vec!["claude-code"]);
    assert_eq!(group.session_count, 1);
    assert_eq!(group.capture_session_count, 1);
    assert!(group.capture_supported);
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
    assert_eq!(selected.capture_session_count, 1);
    assert_eq!(discovery.source_roots, vec![fixture.source.clone()]);
    assert!(
        discovery
            .groups
            .iter()
            .map(|group| group.capture_session_count)
            .sum::<u32>()
            <= 512
    );
    let next_source = fixture
        .directory
        .path()
        .canonicalize()
        .unwrap()
        .join("later.db");
    fs::copy(&fixture.source.path, &next_source).unwrap();
    let bounded = fixture.discover(vec![
        fixture.source.clone(),
        SourceSelection {
            source_kind: SourceKind::OpenCodeSqliteV2,
            path: next_source.to_str().unwrap().into(),
        },
    ]);
    assert!(bounded.partial);
    assert_eq!(bounded.source_roots, vec![fixture.source.clone()]);

    connection
        .execute(
            "UPDATE session_v2 SET directory=?1 WHERE id='ses_main'",
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
