use super::*;
use antiburn_cloud_capture::{CAPTURE_VERSION, CaptureManifest, CaptureState, ServerStatus};

const ORIGIN: &str = "https://api.example";

fn selections() -> (Vec<SourceSelection>, Vec<ProjectSelection>) {
    (
        vec![SourceSelection {
            source_kind: SourceKind::ClaudeJsonl,
            path: "/Example/sources".into(),
        }],
        vec![ProjectSelection {
            source_kind: SourceKind::ClaudeJsonl,
            source_path: "/Example/sources".into(),
            project_path: "/Example/project".into(),
        }],
    )
}

fn policy(store: &Store) -> Policy {
    let (roots, projects) = selections();
    store
        .save_cloud_sync_policy(
            "account",
            "personal",
            ORIGIN,
            &roots,
            &projects,
            &BTreeMap::new(),
        )
        .unwrap()
}

fn job(store: &Store, policy: &Policy) -> Job {
    store
        .enqueue_cloud_sync(
            policy,
            &policy.roots[0].source_scope_id,
            "/Example/project/src",
            &format!("{}/session", policy.roots[0].source_scope_id),
            "observation-one",
        )
        .unwrap()
        .unwrap()
}

fn accepted(job: &Job) -> QueueEntry {
    let manifest = CaptureManifest {
        version: CAPTURE_VERSION,
        identity: job.identity.clone(),
        compression: "gzip-mtime0-os255-level6".into(),
        chunk_expanded_bytes: 8 * 1024 * 1024,
        metadata_fingerprint: "a".repeat(64),
        source_fingerprint: "b".repeat(64),
        streams: Vec::new(),
    };
    let status = ServerStatus {
        identity: job.identity.clone(),
        manifest_sha256: manifest.sha256().unwrap(),
        plan_id: "plan".into(),
        uploaded_parts: Vec::new(),
        integrity_accepted: true,
    };
    QueueEntry {
        identity: job.identity.clone(),
        state: CaptureState::Accepted,
        reservation_id: Some("reservation".into()),
        manifest: Some(manifest),
        upload_plan: None,
        server_status: Some(status),
        completion_submitted: true,
    }
}

#[test]
fn cloud_capacity_retry_deadline_survives_reopen_and_clears_before_pending_work() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let policy = policy(&store);
    let original = job(&store, &policy);
    assert!(original.reservation_retry_at.is_none());
    assert!(
        store
            .set_cloud_sync_job_state(&original, JobState::WaitingForCapacity)
            .is_err()
    );
    store
        .defer_cloud_sync_reservation(&original, 10_000)
        .unwrap();
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let waiting = store
        .cloud_sync_job(&original.identity.capture_id)
        .unwrap()
        .unwrap();
    assert_eq!(waiting.identity, original.identity);
    assert_eq!(waiting.state, JobState::WaitingForCapacity);
    assert_eq!(waiting.reservation_retry_at, Some(10_000));
    store
        .set_cloud_sync_job_state(&waiting, JobState::Pending)
        .unwrap();
    let pending = store
        .cloud_sync_job(&original.identity.capture_id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.identity, original.identity);
    assert!(pending.reservation_retry_at.is_none());
    assert!(
        serde_json::to_value(&pending)
            .unwrap()
            .get("reservation_retry_at")
            .is_none()
    );
}

#[test]
fn cloud_ongoing_consent_is_separate_from_preferences_and_scoped_to_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    store
        .save_cloud_draft("account", "personal", r#"{"cloudUploadEnabled":true}"#)
        .unwrap();
    assert!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap()
            .is_none()
    );
    let first = policy(&store);
    let next = policy(&store);
    assert_eq!(first.roots, next.roots);
    assert_ne!(first.revision, next.revision);
    let (roots, projects) = selections();
    let another = store
        .save_cloud_sync_policy(
            "account",
            "personal-two",
            ORIGIN,
            &roots,
            &projects,
            &BTreeMap::new(),
        )
        .unwrap();
    assert_ne!(first.roots, another.roots);
    assert!(first.allows(&first.roots[0].source_scope_id, "/Example/project"));
    assert!(first.allows(&first.roots[0].source_scope_id, "/Example/project/src"));
    assert!(!first.allows(&first.roots[0].source_scope_id, "/Example/project-worktree"));
    assert!(!first.allows(&first.roots[0].source_scope_id, "/Example/unrelated"));
    assert!(!first.allows("another-scope", "/Example/project"));
    assert!(!first.allows(
        &first.roots[0].source_scope_id,
        "/Example/project/../unrelated"
    ));
    let paused = store.set_cloud_sync_paused(&next, true).unwrap();
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    assert_eq!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap(),
        Some(paused)
    );
    assert!(
        store
            .cloud_sync_policy("other-account", "personal", ORIGIN)
            .unwrap()
            .is_none()
    );
}

#[test]
fn cloud_accepted_fingerprint_and_terminal_job_commit_together_and_survive_restart() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let policy = policy(&store);
    let job = job(&store, &policy);
    let entry = accepted(&job);
    store
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_sync_terminal BEFORE UPDATE ON cloud_sync_job
         BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
        )
        .unwrap();
    assert!(store.accept_cloud_sync(&entry).is_err());
    assert!(
        store
            .cloud_sync_source(
                &job.identity.account_id,
                &job.identity.destination_id,
                ORIGIN,
                &job.identity.source_id
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.cloud_sync_job(&job.identity.capture_id).unwrap(),
        Some(job.clone())
    );
    store
        .lock()
        .execute_batch("DROP TRIGGER reject_sync_terminal")
        .unwrap();
    store.accept_cloud_sync(&entry).unwrap();
    store.accept_cloud_sync(&entry).unwrap();
    assert!(
        store
            .cloud_sync_jobs("account", "personal", ORIGIN)
            .unwrap()
            .is_empty()
    );
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let source = store
        .cloud_sync_source(
            &job.identity.account_id,
            &job.identity.destination_id,
            ORIGIN,
            &job.identity.source_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(source.fingerprint, entry.accepted_source_fingerprint());
    assert_eq!(source.observation_key.as_deref(), Some("observation-one"));
    assert!(
        store
            .enqueue_cloud_sync(
                &policy,
                &job.source_scope_id,
                &job.project_path,
                &job.identity.source_id,
                "observation-one"
            )
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .enqueue_cloud_sync(
                &policy,
                &job.source_scope_id,
                &job.project_path,
                &job.identity.source_id,
                "observation-two"
            )
            .unwrap()
            .is_some()
    );
}

#[test]
fn cloud_service_suppression_survives_restart_policy_replacement_and_source_changes() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let policy = policy(&store);
    let job = job(&store, &policy);
    store.suppress_cloud_sync(&job).unwrap();
    assert!(store.accept_cloud_sync(&accepted(&job)).is_err());
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let replaced = self::policy(&store);
    assert_eq!(replaced.roots, policy.roots);
    assert!(
        store
            .enqueue_cloud_sync(
                &replaced,
                &job.source_scope_id,
                &job.project_path,
                &job.identity.source_id,
                "changed-source"
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .cloud_sync_outcome_counts("account", "personal", ORIGIN)
            .unwrap(),
        (0, 1)
    );
}

#[test]
fn cloud_pause_and_account_changes_keep_fixed_jobs_and_reject_retargeting() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let policy = policy(&store);
    let job = job(&store, &policy);
    let paused = store.set_cloud_sync_paused(&policy, true).unwrap();
    assert!(
        store
            .enqueue_cloud_sync(
                &paused,
                &job.source_scope_id,
                &job.project_path,
                &job.identity.source_id,
                "changed"
            )
            .is_err()
    );
    assert_eq!(
        store.cloud_sync_job(&job.identity.capture_id).unwrap(),
        Some(job.clone())
    );
    assert!(
        store
            .cloud_sync_jobs("another", "personal", ORIGIN)
            .unwrap()
            .is_empty()
    );
    let mut changed = accepted(&job);
    changed.identity.account_id = "another".into();
    assert!(store.accept_cloud_sync(&changed).is_err());
    assert_eq!(
        store.cloud_sync_job(&job.identity.capture_id).unwrap(),
        Some(job)
    );
}

#[test]
fn cloud_saved_job_scope_mismatch_fails_without_rewriting_records() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let policy = policy(&store);
    let job = job(&store, &policy);
    let mut changed = job.clone();
    changed.source_scope_id = "another-scope".into();
    let raw = serde_json::to_string(&changed).unwrap();
    store
        .lock()
        .execute(
            "UPDATE cloud_sync_job SET record_json=?2 WHERE capture_id=?1",
            params![job.identity.capture_id, raw],
        )
        .unwrap();
    assert!(store.cloud_sync_job(&job.identity.capture_id).is_err());
    assert!(
        store
            .cloud_sync_jobs("account", "personal", ORIGIN)
            .is_err()
    );
    assert!(store.accept_cloud_sync(&accepted(&job)).is_err());
    let saved: String = store
        .lock()
        .query_row(
            "SELECT record_json FROM cloud_sync_job WHERE capture_id=?1",
            [&job.identity.capture_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(saved, raw);
}

#[test]
fn cloud_service_origin_requires_fresh_consent_and_separates_jobs_and_fingerprints() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let first = policy(&store);
    let first_job = job(&store, &first);
    let receipt = accepted(&first_job);
    store.accept_cloud_sync(&receipt).unwrap();
    let pending = store
        .enqueue_cloud_sync(
            &first,
            &first_job.source_scope_id,
            &first_job.project_path,
            &first_job.identity.source_id,
            "changed",
        )
        .unwrap()
        .unwrap();
    let other = "https://other.example";
    assert!(
        store
            .cloud_sync_policy("account", "personal", other)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .cloud_sync_jobs("account", "personal", other)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .cloud_sync_outcome_counts("account", "personal", other)
            .unwrap(),
        (0, 0)
    );
    assert!(
        store
            .cloud_sync_source("account", "personal", other, &first_job.identity.source_id)
            .is_err()
    );
    let (roots, projects) = selections();
    let second = store
        .save_cloud_sync_policy(
            "account",
            "personal",
            other,
            &roots,
            &projects,
            &BTreeMap::new(),
        )
        .unwrap();
    assert_ne!(first.roots, second.roots);
    assert!(!pending.allowed(&second));
    assert_eq!(
        store.cloud_sync_job(&pending.identity.capture_id).unwrap(),
        Some(pending)
    );
    assert!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap()
            .is_none()
    );
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let second_again = store
        .save_cloud_sync_policy(
            "account",
            "personal",
            other,
            &roots,
            &projects,
            &BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(second.roots, second_again.roots);
    let first_again = store
        .save_cloud_sync_policy(
            "account",
            "personal",
            ORIGIN,
            &roots,
            &projects,
            &BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(first.roots, first_again.roots);
    assert_eq!(
        store
            .cloud_sync_source("account", "personal", ORIGIN, &first_job.identity.source_id)
            .unwrap()
            .unwrap()
            .fingerprint,
        receipt.accepted_source_fingerprint()
    );
}

#[test]
fn cloud_unbound_legacy_consent_is_not_migrated_or_used_for_upload() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let current = policy(&store);
    let mut legacy = serde_json::to_value(&current).unwrap();
    legacy.as_object_mut().unwrap().remove("resourceOrigin");
    let raw = serde_json::to_string(&legacy).unwrap();
    store
        .lock()
        .execute("UPDATE cloud_sync_policy SET record_json=?1", [&raw])
        .unwrap();
    assert!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap()
            .is_none()
    );
    let saved: String = store
        .lock()
        .query_row("SELECT record_json FROM cloud_sync_policy", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(raw, saved);
    let mut unknown = current.clone();
    unknown.resource_origin = None;
    assert!(
        store
            .enqueue_cloud_sync(
                &unknown,
                &unknown.roots[0].source_scope_id,
                "/Example/project",
                &format!("{}/session", unknown.roots[0].source_scope_id),
                "observation"
            )
            .is_err()
    );
}

#[test]
fn cloud_verified_acceptance_stays_with_its_original_service_after_consent_changes() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let first = policy(&store);
    let first_job = job(&store, &first);
    let receipt = accepted(&first_job);
    let (roots, projects) = selections();
    store
        .save_cloud_sync_policy(
            "account",
            "personal",
            "https://other.example",
            &roots,
            &projects,
            &BTreeMap::new(),
        )
        .unwrap();
    store.accept_cloud_sync(&receipt).unwrap();
    assert_eq!(
        store
            .cloud_sync_outcome_counts("account", "personal", ORIGIN)
            .unwrap(),
        (1, 0)
    );
    assert_eq!(
        store
            .cloud_sync_outcome_counts("account", "personal", "https://other.example")
            .unwrap(),
        (0, 0)
    );
}

#[test]
fn cloud_suppression_keeps_the_prior_accepted_fingerprint_in_its_service() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let policy = policy(&store);
    let first = job(&store, &policy);
    let receipt = accepted(&first);
    store.accept_cloud_sync(&receipt).unwrap();
    let next = store
        .enqueue_cloud_sync(
            &policy,
            &first.source_scope_id,
            &first.project_path,
            &first.identity.source_id,
            "a newer source",
        )
        .unwrap()
        .unwrap();
    store.suppress_cloud_sync(&next).unwrap();
    let source = store
        .cloud_sync_source("account", "personal", ORIGIN, &first.identity.source_id)
        .unwrap()
        .unwrap();
    assert!(source.suppressed);
    assert_eq!(source.fingerprint, receipt.accepted_source_fingerprint());
    assert_eq!(
        store
            .cloud_sync_outcome_counts("account", "personal", ORIGIN)
            .unwrap(),
        (1, 1)
    );
    assert_eq!(
        store
            .cloud_sync_outcome_counts("account", "personal", "https://other.example")
            .unwrap(),
        (0, 0)
    );
}

fn project_group_id(path: &str) -> String {
    use sha2::{Digest, Sha256};
    URL_SAFE_NO_PAD.encode(Sha256::digest(
        serde_json::to_vec(&("native", path)).unwrap(),
    ))
}

#[test]
fn cloud_project_choices_preserve_confirmed_false_and_unrelated_entries_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let (roots, projects) = selections();
    let choices = BTreeMap::from([
        (project_group_id("/Example/project"), true),
        (project_group_id("/Example/excluded"), false),
        (project_group_id("/Example/unrelated"), false),
    ]);
    let saved = store
        .save_cloud_sync_policy("account", "personal", ORIGIN, &roots, &projects, &choices)
        .unwrap();
    let paused = store.set_cloud_sync_paused(&saved, true).unwrap();
    assert_eq!(paused.project_choices, choices);
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    let restored = store
        .cloud_sync_policy("account", "personal", ORIGIN)
        .unwrap()
        .unwrap();
    assert_eq!(restored.project_choices, choices);
    assert_eq!(
        serde_json::to_value(&restored).unwrap()["projectChoices"]
            [project_group_id("/Example/excluded")],
        false
    );
}

#[test]
fn cloud_project_choices_old_policy_without_map_loads_empty_without_rewriting() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let saved = policy(&store);
    let mut legacy = serde_json::to_value(saved).unwrap();
    legacy.as_object_mut().unwrap().remove("projectChoices");
    let raw = legacy.to_string();
    store
        .lock()
        .execute("UPDATE cloud_sync_policy SET record_json=?1", [&raw])
        .unwrap();
    assert!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap()
            .unwrap()
            .project_choices
            .is_empty()
    );
    let stored: String = store
        .lock()
        .query_row("SELECT record_json FROM cloud_sync_policy", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(stored, raw);
    let legacy = store
        .cloud_sync_policy("account", "personal", ORIGIN)
        .unwrap()
        .unwrap();
    let paused = store.set_cloud_sync_paused(&legacy, true).unwrap();
    assert!(paused.project_choices.is_empty());
    assert!(paused.paused);
}

#[test]
fn cloud_project_choices_do_not_grant_or_replace_upload_permissions() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let (roots, projects) = selections();
    let choices = BTreeMap::from([
        (project_group_id("/Example/project"), false),
        (project_group_id("/Example/excluded"), true),
    ]);
    let policy = store
        .save_cloud_sync_policy("account", "personal", ORIGIN, &roots, &projects, &choices)
        .unwrap();
    let scope = &policy.roots[0].source_scope_id;
    assert!(!policy.allows(scope, "/Example/excluded"));
    assert!(
        store
            .enqueue_cloud_sync(
                &policy,
                scope,
                "/Example/excluded",
                &format!("{scope}/excluded-session"),
                "observation"
            )
            .is_err()
    );
    assert!(policy.allows(scope, "/Example/project"));
    assert!(
        store
            .enqueue_cloud_sync(
                &policy,
                scope,
                "/Example/project",
                &format!("{scope}/included-session"),
                "observation"
            )
            .unwrap()
            .is_some()
    );
}

#[test]
fn cloud_project_choices_and_permissions_roll_back_together_on_failed_save() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let (roots, projects) = selections();
    let choices = BTreeMap::from([(project_group_id("/Example/excluded"), false)]);
    let original = store
        .save_cloud_sync_policy("account", "personal", ORIGIN, &roots, &projects, &choices)
        .unwrap();
    store
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_project_choices BEFORE UPDATE ON cloud_sync_policy
         BEGIN SELECT RAISE(ABORT, 'synthetic save failure'); END;",
        )
        .unwrap();
    let replacement_roots = [SourceSelection {
        source_kind: SourceKind::ClaudeJsonl,
        path: "/Example/replacement-sources".into(),
    }];
    let replacement_projects = [ProjectSelection {
        source_kind: SourceKind::ClaudeJsonl,
        source_path: "/Example/replacement-sources".into(),
        project_path: "/Example/replacement-project".into(),
    }];
    let replacement_choices = BTreeMap::from([(project_group_id("/Example/excluded"), true)]);
    assert!(
        store
            .save_cloud_sync_policy(
                "account",
                "personal",
                ORIGIN,
                &replacement_roots,
                &replacement_projects,
                &replacement_choices
            )
            .is_err()
    );
    assert_eq!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap()
            .unwrap(),
        original
    );
    let scopes: u64 = store
        .lock()
        .query_row("SELECT count(*) FROM cloud_sync_service_scope", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(scopes, 1);
}

#[test]
fn cloud_project_choices_use_the_existing_policy_record_size_bound() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let original = policy(&store);
    let (roots, projects) = selections();
    let oversized = BTreeMap::from([("x".repeat(524_289), false)]);
    assert!(
        store
            .save_cloud_sync_policy("account", "personal", ORIGIN, &roots, &projects, &oversized)
            .is_err()
    );
    assert_eq!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap()
            .unwrap(),
        original
    );
}

#[test]
fn settings_project_change_keeps_hidden_policy_pause_and_stable_scopes() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let original = policy(&store);
    let paused = store.set_cloud_sync_paused(&original, true).unwrap();
    let original_group = project_group_id("/Example/project");
    let other_group = project_group_id("/Example/other");
    let added = ProjectSelection {
        source_kind: SourceKind::OpenCodeSqliteV2,
        source_path: "/Example/opencode.db".into(),
        project_path: "/Example/other".into(),
    };
    let other_projects = BTreeSet::from([added.clone()]);
    let expanded = store
        .set_cloud_sync_project(&paused, &other_group, true, &other_projects, || true)
        .unwrap();
    assert!(expanded.paused);
    assert_ne!(expanded.revision, paused.revision);
    assert_eq!(expanded.projects.len(), 2);
    assert!(expanded.roots.contains(&paused.roots[0]));
    let added_scope = expanded
        .roots
        .iter()
        .find(|root| root.path == added.source_path)
        .unwrap()
        .clone();
    let narrowed = store
        .set_cloud_sync_project(
            &expanded,
            &original_group,
            false,
            &paused.projects.iter().cloned().collect(),
            || true,
        )
        .unwrap();
    assert_eq!(narrowed.projects, [added]);
    assert_eq!(narrowed.roots, [added_scope]);
    assert!(!narrowed.project_choices[&original_group]);
    assert!(narrowed.project_choices[&other_group]);
    let empty = store
        .set_cloud_sync_project(&narrowed, &other_group, false, &other_projects, || true)
        .unwrap();
    assert!(empty.projects.is_empty());
    assert!(empty.roots.is_empty());
    assert!(empty.paused);
    assert!(!empty.allows(&paused.roots[0].source_scope_id, "/Example/project/src"));
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap(),
        Some(empty.clone())
    );
    let restored = store
        .set_cloud_sync_project(
            &empty,
            &original_group,
            true,
            &paused.projects.iter().cloned().collect(),
            || true,
        )
        .unwrap();
    assert_eq!(restored.roots, paused.roots);
    assert!(restored.paused);
    assert!(!restored.project_choices[&other_group]);
}

#[test]
fn settings_project_change_rejects_stale_binding_and_rolls_back_on_fence() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let original = policy(&store);
    let group = project_group_id("/Example/project");
    let paused = store.set_cloud_sync_paused(&original, true).unwrap();
    let projects = BTreeSet::from([ProjectSelection {
        source_kind: SourceKind::OpenCodeSqliteV2,
        source_path: "/Example/opencode.db".into(),
        project_path: "/Example/project".into(),
    }]);
    assert!(
        store
            .set_cloud_sync_project(&original, &group, true, &projects, || true)
            .is_err()
    );
    assert!(
        store
            .set_cloud_sync_project(&paused, &group, true, &projects, || false)
            .is_err()
    );
    let mut wrong_service = paused.clone();
    wrong_service.resource_origin = Some("https://other.example".into());
    assert!(
        store
            .set_cloud_sync_project(&wrong_service, &group, true, &projects, || true)
            .is_err()
    );
    assert_eq!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap(),
        Some(paused)
    );
    let scopes: u64 = store
        .lock()
        .query_row("SELECT count(*) FROM cloud_sync_service_scope", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(scopes, 1);
}
