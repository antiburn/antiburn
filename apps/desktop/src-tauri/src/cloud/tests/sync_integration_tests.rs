use super::*;
use crate::store::cloud_capture::RootScope;
use antiburn_cloud_capture::{
    CaptureIdentity, CaptureManifest, Chunk, ChunkKey, ServerStatus, Spool, Transport,
    TransportError, UploadPlan,
};
use std::{fs, io::Read, path::PathBuf};

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
        paused: false,
    };
    (directory, policy, selected, outside)
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
    assert!(discover_policy(&policy, &|| false).unwrap().is_empty());
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
    .limits()
    .unwrap();
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
