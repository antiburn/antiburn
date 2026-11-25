use super::*;
use crate::store::cloud_capture::RootScope;
use antiburn_cloud_capture::{
    CaptureIdentity, CaptureManifest, Chunk, ChunkKey, ServerStatus, Spool, Transport,
    TransportError, UploadPlan,
};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
};

fn write_session(root: &Path, id: &str, cwd: &Path, tool_path: &Path) {
    fs::write(
        root.join(format!("{id}.jsonl")),
        format!(
            "{}\n",
            serde_json::json!({
                "sessionId": id, "cwd": cwd, "version": "2.1.55", "type": "user",
                "message": {"role": "user", "content": "synthetic content"}, "toolPath": tool_path,
            })
        ),
    )
    .unwrap();
}

fn fixture() -> (tempfile::TempDir, Policy, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let root = base.join("desktop-sources");
    let selected = base.join("project");
    let outside = base.join("project-worktree");
    for path in [&root, &selected, &outside, &selected.join("src")] {
        fs::create_dir(path).unwrap();
    }
    let policy = Policy {
        account_id: "account".into(),
        destination_id: "personal".into(),
        resource_origin: Some("https://api.example".into()),
        issuer_origin: Some("https://api.example".into()),
        team_grant_id: None,
        membership_generation: None,
        workspace_name: None,
        consent_version: ONGOING_CONSENT_VERSION.into(),
        revision: "revision".into(),
        roots: vec![RootScope {
            path: root.to_str().unwrap().into(),
            source_scope_id: "scope".into(),
            source_kind: SourceKind::ClaudeJsonl,
        }],
        projects: vec![ProjectSelection {
            source_kind: SourceKind::ClaudeJsonl,
            source_path: root.to_str().unwrap().into(),
            project_path: selected.to_str().unwrap().into(),
        }],
        project_choices: BTreeMap::new(),
        repository_rules: Vec::new(),
        paused: false,
    };
    (directory, policy, selected, outside)
}

fn retry_policy(
    store: &Store,
    root: &Path,
    project: &Path,
    destination: &str,
    team: Option<(&str, &str)>,
) -> Policy {
    let root = root.to_str().unwrap();
    store
        .save_cloud_sync_policy_for_membership(
            (
                "account",
                destination,
                "https://api.example",
                "https://api.example",
                team,
            ),
            &[SourceSelection {
                source_kind: SourceKind::ClaudeJsonl,
                path: root.into(),
            }],
            &[ProjectSelection {
                source_kind: SourceKind::ClaudeJsonl,
                source_path: root.into(),
                project_path: project.to_str().unwrap().into(),
            }],
            &BTreeMap::new(),
            || true,
        )
        .unwrap()
}

fn interrupted_upload(store: &Store, spool: &mut Spool, policy: &Policy, id: &str) -> Job {
    let project = &policy.projects[0].project_path;
    write_session(
        Path::new(&policy.roots[0].path),
        id,
        Path::new(project),
        Path::new(project),
    );
    let selection = discover_policy(policy, &|| true)
        .unwrap()
        .into_iter()
        .find(|selection| selection.candidate.native_session_id() == id)
        .unwrap();
    let candidate = antiburn_cloud_capture::ProjectSelection::new(vec![project.into()])
        .unwrap()
        .admit_with_limits(&selection.candidate, &metadata_limits())
        .unwrap()
        .unwrap();
    let job = store
        .enqueue_cloud_sync_with_paths(
            policy,
            &selection.scope,
            project,
            candidate.source_id(),
            id,
            Some(std::slice::from_ref(project)),
        )
        .unwrap()
        .unwrap();
    spool
        .capture(
            job.identity.clone(),
            &candidate,
            MetadataSections::new(),
            &mut ReserveOnly {
                reservations: 0,
                maximum: 0,
            },
        )
        .unwrap();
    store
        .set_cloud_sync_job_state(&job, JobState::Interrupted)
        .unwrap();
    job
}

#[test]
fn cloud_project_directory_validation_keeps_aliases_visible() {
    let (_directory, policy, project, _) = fixture();
    let mut alias = policy.projects[0].clone();
    alias.project_path = format!("{}/.", project.display());
    assert_eq!(
        validate_project_directories([policy.projects[0].clone(), alias].iter()),
        Err("Choose existing project directories without path aliases.".into())
    );
}

#[test]
fn cloud_retry_preserves_the_policy_pause_from_changed_provider_roots() {
    let (directory, original, project, _) = fixture();
    let base = directory.path().canonicalize().unwrap();
    let store = Store::open_in_memory(&base).unwrap();
    store.initialize_cloud_setup().unwrap();
    let policy = retry_policy(
        &store,
        Path::new(&original.roots[0].path),
        &project,
        "personal",
        None,
    );
    let mut spool = Spool::open(&base.join("spool"), metadata_limits()).unwrap();
    let job = interrupted_upload(&store, &mut spool, &policy, "personal-saved");
    // The draft command saves this pause when provider roots change.
    store.set_cloud_sync_paused(&policy, true).unwrap();
    let load = || {
        store
            .cloud_sync_policy("account", "personal", "https://api.example")
            .unwrap()
            .unwrap()
    };
    let paused = load();
    assert!(retry_saved_jobs(&store, &mut spool, &paused, &|| true).is_err());
    assert!(paused.paused);
    assert_eq!(load(), paused);
    assert!(store.cloud_device_upload_enabled().unwrap());
    assert_eq!(
        store
            .cloud_sync_job(&job.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Interrupted
    );
}

#[test]
fn cloud_retry_rejects_a_stale_team_revision_and_only_changes_exact_saved_jobs() {
    let (directory, original, personal_project, team_project) = fixture();
    let base = directory.path().canonicalize().unwrap();
    let root = Path::new(&original.roots[0].path);
    let store = Store::open_in_memory(&base).unwrap();
    store.initialize_cloud_setup().unwrap();
    let mut spool = Spool::open(&base.join("spool"), metadata_limits()).unwrap();
    let personal = retry_policy(&store, root, &personal_project, "personal", None);
    let previous = retry_policy(
        &store,
        root,
        &team_project,
        "team",
        Some(("old-membership", "Example")),
    );
    let personal_job = interrupted_upload(&store, &mut spool, &personal, "personal-saved");
    let stale = interrupted_upload(&store, &mut spool, &previous, "old-team-saved");
    let stale = store
        .cloud_sync_job(&stale.identity.capture_id)
        .unwrap()
        .unwrap();
    let before_renewal = spool.entries().unwrap();
    let team = store
        .renew_cloud_team_sync_policy(
            (
                "account",
                "team",
                "https://api.example",
                "https://api.example",
                Some(("new-membership", "Example")),
            ),
            &previous.revision,
            &[team_project.to_str().unwrap().to_owned()].into(),
            None,
            || true,
        )
        .unwrap();
    assert_eq!(spool.entries().unwrap(), before_renewal);
    assert_eq!(
        store.cloud_sync_job(&stale.identity.capture_id).unwrap(),
        Some(stale.clone())
    );
    let current = interrupted_upload(&store, &mut spool, &team, "current-team-saved");
    let before = spool.entries().unwrap();
    let mut session = crate::cloud::tests::session();
    session.user.id = team.account_id.clone();
    let binding = DestinationBinding {
        account_id: team.account_id.clone(),
        destination_id: team.destination_id.clone(),
    };
    let mut retry_from_window = |revision: &str| {
        let policy = retry_policy_for_revision(&store, &session, &binding, revision)?;
        retry_saved_jobs(&store, &mut spool, &policy, &|| true)
    };
    // Another window reapproved the same Team after this window saw the old policy.
    assert_eq!(
        retry_from_window(&previous.revision),
        Err("Cloud folder choices changed. Reload before retrying saved work.".into())
    );
    assert_eq!(
        store
            .cloud_sync_job(&current.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Interrupted
    );
    assert_eq!(
        retry_from_window(&team.revision).unwrap(),
        BTreeSet::from([current.identity.capture_id.clone()]),
    );
    for (job, state) in [
        (current, JobState::Queued),
        (stale, JobState::Interrupted),
        (personal_job, JobState::Interrupted),
    ] {
        assert_eq!(
            store
                .cloud_sync_job(&job.identity.capture_id)
                .unwrap()
                .unwrap()
                .state,
            state
        );
    }
    assert_eq!(spool.entries().unwrap(), before);
}

#[test]
fn cloud_sync_allowlist_uses_recorded_cwd_and_descendants_not_tool_paths_or_worktree_names() {
    let (_directory, policy, selected, outside) = fixture();
    let root = Path::new(&policy.roots[0].path);
    write_session(root, "allowed", &selected, &outside.join("file"));
    write_session(
        root,
        "descendant",
        &selected.join("src"),
        &outside.join("file"),
    );
    write_session(root, "other-worktree", &outside, &selected.join("file"));
    write_session(root, "touch-only", &outside, &selected.join("file"));
    let sources = discover_policy(&policy, &|| true).unwrap();
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0].candidate.native_session_id(), "allowed");
    assert_eq!(sources[1].candidate.native_session_id(), "descendant");
    assert!(
        sources
            .iter()
            .all(|source| policy.allows(&source.scope, &source.project))
    );
    let folders = saved_folders(&policy);
    assert_eq!(folders.len(), 1);
    assert!(
        sources
            .iter()
            .all(|source| source.folder_group_id.as_ref() == Some(&folders[0].group_id))
    );
    assert!(discover_policy(&policy, &|| false).unwrap().is_empty());
}

#[test]
fn exact_folder_count_omits_a_session_that_matches_overlapping_saved_folders() {
    let (_directory, mut policy, selected, outside) = fixture();
    policy.projects.push(ProjectSelection {
        project_path: selected.join("src").to_str().unwrap().into(),
        ..policy.projects[0].clone()
    });
    let root = Path::new(&policy.roots[0].path);
    write_session(
        root,
        "overlap",
        &selected.join("src"),
        &outside.join("file"),
    );
    let sources = discover_policy(&policy, &|| true).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].folder_group_id, None);
}

#[test]
fn exact_folder_count_uses_distinct_saved_rule_ids_for_separate_paths() {
    let (_directory, mut policy, main, checkout) = fixture();
    policy.projects.push(ProjectSelection {
        project_path: checkout.to_str().unwrap().into(),
        ..policy.projects[0].clone()
    });
    let root = Path::new(&policy.roots[0].path);
    write_session(root, "main", &main, &checkout.join("file"));
    write_session(root, "worktree", &checkout, &main.join("file"));
    let sources = discover_policy(&policy, &|| true).unwrap();
    assert_eq!(sources.len(), 2);
    let folders = saved_folders(&policy);
    assert_eq!(folders.len(), 2);
    assert_ne!(folders[0].group_id, folders[1].group_id);
    for source in sources {
        let folder = folders
            .iter()
            .find(|folder| folder.path == source.project)
            .unwrap();
        assert_eq!(source.folder_group_id.as_ref(), Some(&folder.group_id));
    }
}

#[test]
fn repository_approval_captures_a_matching_clone_and_rechecks_its_origin() {
    use crate::store::cloud_capture::sync::RepositoryRule;
    let (_directory, mut policy, anchor, checkout) = fixture();
    let git = |path: &Path, args: &[&str]| {
        let status = tauri::async_runtime::block_on(async {
            tokio::process::Command::new("git")
                .current_dir(path)
                .args(args)
                .status()
                .await
        })
        .expect("run Git in the synthetic checkout");
        assert!(status.success());
    };
    git(&checkout, &["init", "--quiet"]);
    git(
        &checkout,
        &[
            "config",
            "--local",
            "remote.origin.url",
            "git@github.com:Example/Project.git",
        ],
    );
    policy.projects.clear();
    policy.repository_rules.push(RepositoryRule {
        origin: "github.com/Example/Project".into(),
        anchor_path: anchor.to_str().unwrap().into(),
        scopes: BTreeSet::from(["scope".into()]),
    });
    let folders = saved_folders(&policy);
    assert_eq!(folders.len(), 1);
    assert_eq!(
        policy.repository_rule_for_group(&folders[0].group_id),
        policy.repository_rules.first()
    );
    let root = Path::new(&policy.roots[0].path);
    write_session(root, "matching", &checkout, &anchor.join("file"));
    write_session(root, "plain", &anchor, &checkout.join("file"));
    let matches = discover_policy(&policy, &|| true).unwrap();
    assert_eq!(matches.len(), 1);
    let selection = &matches[0];
    assert_eq!(selection.candidate.native_session_id(), "matching");
    assert_eq!(
        selection.folder_group_id.as_ref(),
        Some(&folders[0].group_id)
    );
    let origin = selection.repository_origin.as_ref().unwrap();
    assert!(repository_capture::recheck_repository_project(
        &policy,
        &selection.scope,
        origin,
        &selection.project,
        std::slice::from_ref(&checkout)
    ));
    git(
        &checkout,
        &[
            "config",
            "--local",
            "remote.origin.url",
            "https://github.com/Example/Other.git",
        ],
    );
    assert!(!repository_capture::recheck_repository_project(
        &policy,
        &selection.scope,
        origin,
        &selection.project,
        std::slice::from_ref(&checkout)
    ));
    let mut cache = crate::cloud::projects::Cache::default();
    let (matches, next, coverage) =
        discover_policy_page(&policy, &|| true, &mut cache, &DiscoveryCursor::default()).unwrap();
    assert!(matches.is_empty());
    assert!(next.is_none());
    assert!(!coverage.partial);
    assert!(!coverage.blocked);

    let damaged = anchor.join("damaged");
    fs::create_dir(&damaged).unwrap();
    fs::write(damaged.join(".git"), "invalid pointer\n").unwrap();
    fs::remove_file(root.join("matching.jsonl")).unwrap();
    write_session(root, "mixed", &damaged, &anchor.join("file"));
    assert!(damaged < checkout);
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(root.join("mixed.jsonl"))
            .unwrap(),
        "{}",
        serde_json::json!({
            "sessionId": "mixed", "cwd": checkout, "version": "2.1.55", "type": "user",
            "message": {"role": "user", "content": "changed directory"},
        })
    )
    .unwrap();

    for (fetch_url, expected_partial) in [
        ("https://github.com/Example/Other.git", true),
        ("https://github.com/Example/Project.git", false),
    ] {
        git(
            &checkout,
            &["config", "--local", "remote.origin.url", fetch_url],
        );
        let mut cursor = DiscoveryCursor::default();
        let mut selected = Vec::new();
        let mut partial = false;
        loop {
            let (matches, next, coverage) =
                discover_policy_page(&policy, &|| true, &mut cache, &cursor).unwrap();
            assert!(!coverage.blocked);
            partial |= coverage.partial;
            selected.extend(matches);
            let Some(next) = next else {
                break;
            };
            assert_ne!(next, cursor);
            cursor = next;
        }
        assert_eq!(partial, expected_partial);
        if expected_partial {
            assert!(selected.is_empty());
        } else {
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0].candidate.native_session_id(), "mixed");
            assert_eq!(
                selected[0].repository_origin.as_deref(),
                Some(origin.as_str())
            );
        }
    }
}

#[test]
fn cloud_sync_full_capture_rechecks_project_after_reservation_and_keeps_interrupted_spool() {
    let (directory, policy, selected, outside) = fixture();
    let root = Path::new(&policy.roots[0].path);
    write_session(root, "allowed", &selected, &outside);
    let selection = discover_policy(&policy, &|| true).unwrap().remove(0);
    let candidate = antiburn_cloud_capture::ProjectSelection::new(vec![selected.clone()])
        .unwrap()
        .admit_with_limits(&selection.candidate, &metadata_limits())
        .unwrap()
        .unwrap();
    write_session(root, "allowed", &outside, &selected);
    let mut spool = Spool::open(
        &directory.path().canonicalize().unwrap().join("spool"),
        Limits::default(),
    )
    .unwrap();
    let identity = CaptureIdentity {
        capture_id: "capture".into(),
        account_id: "account".into(),
        destination_id: "personal".into(),
        source_id: selection.candidate.source_id().into(),
    };
    let mut transport = ReserveOnly {
        reservations: 0,
        maximum: 0,
    };
    assert!(
        spool
            .capture(
                identity.clone(),
                &candidate,
                MetadataSections::new(),
                &mut transport
            )
            .is_err()
    );
    assert_eq!(transport.reservations, 1);
    assert_eq!(spool.entries().unwrap()[0].state, CaptureState::Preparing);
    drop(spool);
    let spool = Spool::open(
        &directory.path().canonicalize().unwrap().join("spool"),
        Limits::default(),
    )
    .unwrap();
    assert_eq!(spool.entries().unwrap()[0].identity, identity);
}

#[test]
fn cloud_sync_source_observations_include_committed_opencode_wal_changes() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().canonicalize().unwrap().join("opencode.db");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(include_str!(
            "../../../../../../fixtures/cloud/opencode/current.sql"
        ))
        .unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL; UPDATE session_v2 SET title='first' WHERE id='ses_main';",
        )
        .unwrap();
    let candidate = discover_opencode(&database, &Limits::default())
        .unwrap()
        .remove(0);
    let before = source_observation(&candidate).unwrap();
    connection
        .execute_batch("UPDATE session_v2 SET title='second changed title' WHERE id='ses_main';")
        .unwrap();
    assert_ne!(before, source_observation(&candidate).unwrap());
}

struct ReserveOnly {
    reservations: u32,
    maximum: u64,
}

impl Transport for ReserveOnly {
    fn reserve(&mut self, _: &CaptureIdentity, maximum: u64) -> Result<String, TransportError> {
        self.reservations += 1;
        self.maximum = maximum;
        Ok("reservation".into())
    }
    fn negotiate(&mut self, _: &str, _: &CaptureManifest) -> Result<UploadPlan, TransportError> {
        panic!("capture must not negotiate upload");
    }
    fn status(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
    ) -> Result<ServerStatus, TransportError> {
        panic!("capture must not request status");
    }
    fn upload_part(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
        _: &ChunkKey,
        _: &Chunk,
        _: &mut dyn Read,
    ) -> Result<(), TransportError> {
        panic!("capture must not upload");
    }
    fn complete(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
    ) -> Result<(), TransportError> {
        panic!("capture must not finalize");
    }
}

#[test]
fn cloud_sync_changing_cwds_match_any_owned_project_without_folder_redaction() {
    let (directory, policy, selected, outside) = fixture();
    let root = Path::new(&policy.roots[0].path);
    write_session(root, "moving", &selected, &outside);
    let mut bytes = fs::read(root.join("moving.jsonl")).unwrap();
    bytes.extend_from_slice(
        format!(
            "{}\n",
            serde_json::json!({
                "sessionId": "moving", "cwd": outside, "type": "user", "version": "2.1.55",
                "message": {"role": "user", "content": "more synthetic content"},
            })
        )
        .as_bytes(),
    );
    fs::write(root.join("moving.jsonl"), &bytes).unwrap();
    let selection = discover_policy(&policy, &|| true).unwrap().remove(0);
    let candidate = antiburn_cloud_capture::ProjectSelection::new(vec![selected])
        .unwrap()
        .admit_with_limits(&selection.candidate, &metadata_limits())
        .unwrap()
        .unwrap();
    assert_eq!(
        candidate.project_evidence().unwrap().project_paths().len(),
        2
    );
    let mut spool = Spool::open(
        &directory.path().canonicalize().unwrap().join("spool"),
        metadata_limits(),
    )
    .unwrap();
    let mut transport = ReserveOnly {
        reservations: 0,
        maximum: 0,
    };
    let entry = spool
        .capture(
            CaptureIdentity {
                capture_id: "moving".into(),
                account_id: "account".into(),
                destination_id: "personal".into(),
                source_id: candidate.source_id().into(),
            },
            &candidate,
            MetadataSections::new(),
            &mut transport,
        )
        .unwrap();
    assert_eq!(
        entry.manifest.unwrap().streams[0].expanded_bytes,
        bytes.len() as u64
    );
}

#[test]
fn cloud_sync_project_cache_invalidates_same_length_edits_even_when_mtime_is_restored() {
    let (_directory, policy, selected, _outside) = fixture();
    let first = selected.join("folder-a");
    let second = selected.join("folder-b");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let root = Path::new(&policy.roots[0].path);
    write_session(root, "cached", &first, &selected);
    let path = root.join("cached.jsonl");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let candidate = discover_claude(root, &Limits::default()).unwrap().remove(0);
    let root = SourceSelection {
        source_kind: SourceKind::ClaudeJsonl,
        path: root.to_str().unwrap().into(),
    };
    let mut cache = super::super::projects::Cache::default();
    let first_evidence = cache
        .evidence("account", "personal", &root, &candidate, &metadata_limits())
        .unwrap()
        .unwrap();
    write_session(Path::new(&root.path), "cached", &second, &selected);
    fs::File::open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let second_evidence = cache
        .evidence("account", "personal", &root, &candidate, &metadata_limits())
        .unwrap()
        .unwrap();
    assert_ne!(
        first_evidence.source_digest(),
        second_evidence.source_digest()
    );
    assert_eq!(second_evidence.project_paths(), &[second]);
}

#[test]
fn cloud_sync_opencode_reservation_uses_verified_service_limits() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let project = base.join("project");
    fs::create_dir(&project).unwrap();
    let database = base.join("opencode.db");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(include_str!(
            "../../../../../../fixtures/cloud/opencode/current.sql"
        ))
        .unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory=?1 WHERE id='ses_main'",
            [project.to_str().unwrap()],
        )
        .unwrap();
    let limits = serde_json::from_value::<super::super::upload::Capabilities>(serde_json::json!({
        "enabled": true, "max_expanded_bytes": 256 * 1024 * 1024,
        "chunk_expanded_bytes": 8 * 1024 * 1024,
        "source_formats": ["ClaudeJsonl", "OpenCodeSqliteV2"],
    }))
    .unwrap()
    .validate()
    .unwrap()
    .limits;
    let candidate = discover_opencode(&database, &limits)
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.native_session_id() == "ses_main")
        .unwrap()
        .in_source_scope("scope")
        .unwrap();
    let candidate = antiburn_cloud_capture::ProjectSelection::new(vec![project])
        .unwrap()
        .admit_with_limits(&candidate, &limits)
        .unwrap()
        .unwrap();
    let mut spool = Spool::open(&base.join("spool"), limits).unwrap();
    let mut transport = ReserveOnly {
        reservations: 0,
        maximum: 0,
    };
    let entry = spool
        .capture(
            CaptureIdentity {
                capture_id: "opencode".into(),
                account_id: "account".into(),
                destination_id: "personal".into(),
                source_id: candidate.source_id().into(),
            },
            &candidate,
            MetadataSections::new(),
            &mut transport,
        )
        .unwrap();
    assert_eq!(entry.state, CaptureState::Queued);
    assert!(transport.maximum <= 256 * 1024 * 1024);
    assert_eq!(
        transport.maximum,
        128 * 1024 * 1024 + antiburn_cloud_capture::METADATA_BYTES
    );
}

#[test]
fn cloud_sync_restart_restores_preparing_as_interrupted_without_recapture() {
    let (directory, policy, selected, _outside) = fixture();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let policy = store
        .save_cloud_sync_policy(
            "account",
            "personal",
            "https://api.example",
            &[SourceSelection {
                source_kind: SourceKind::ClaudeJsonl,
                path: policy.roots[0].path.clone(),
            }],
            &policy.projects,
            &policy.project_choices,
        )
        .unwrap();
    let job = store
        .enqueue_cloud_sync(
            &policy,
            &policy.roots[0].source_scope_id,
            selected.to_str().unwrap(),
            &format!("{}/session", policy.roots[0].source_scope_id),
            "observation",
        )
        .unwrap()
        .unwrap();
    let entry = QueueEntry {
        identity: job.identity.clone(),
        state: CaptureState::Preparing,
        reservation_id: None,
        manifest: None,
        upload_plan: None,
        server_status: None,
        completion_submitted: false,
    };
    let mut jobs = store
        .cloud_sync_jobs("account", "personal", "https://api.example")
        .unwrap();
    restore_jobs(&store, &mut jobs, &[entry]).unwrap();
    assert_eq!(jobs[0].state, JobState::Interrupted);
    assert_eq!(
        store
            .cloud_sync_job(&job.identity.capture_id)
            .unwrap()
            .unwrap()
            .state,
        JobState::Interrupted
    );
}
