use super::*;
use crate::cloud::{
    auth::{self, Config, Session, TokenRequest, Tokens, Transport, Vault},
    state::StateStore,
};
use std::{fs, sync::Arc};

const ORIGIN: &str = "https://api.example";

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
            workspace_id: "synthetic-personal".into(),
            grant_expires_at: "1970-01-01T02:46:40Z".into(),
            scope: "capture:write".into(),
            resource: ORIGIN.into(),
            iss: ORIGIN.into(),
        })
    }
    fn session(&self, _: &Config, _: &str) -> Result<Session, auth::Failure> {
        Ok(crate::cloud::tests::session())
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
