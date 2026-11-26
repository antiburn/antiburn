use super::*;
use antiburn_cloud_capture::{CAPTURE_VERSION, CaptureManifest, CaptureState, ServerStatus};

const ORIGIN: &str = "https://api.example";
const REPOSITORY: &str = "git.example.org/Group/Repo";

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

fn repository_rule(policy: &Policy) -> RepositoryRule {
    RepositoryRule {
        origin: REPOSITORY.into(),
        anchor_path: "/Example/project".into(),
        scopes: [policy.roots[0].source_scope_id.clone()].into(),
    }
}

#[test]
fn cloud_initial_repository_approval_saves_scope_and_display_anchor_not_folder_grant() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let (roots, projects) = selections();
    let anchors = [(projects[0].project_path.clone(), REPOSITORY.into())].into();
    let choices = [(repository_group_id(REPOSITORY), true)].into();
    let saved = store
        .save_cloud_sync_policy_with_repositories(
            ("account", "personal", ORIGIN, ORIGIN, None),
            &roots,
            &projects,
            &choices,
            &anchors,
            || true,
        )
        .unwrap();
    let scope = &saved.roots[0].source_scope_id;
    assert!(saved.projects.is_empty());
    assert_eq!(saved.projects_for_display(), projects);
    assert_eq!(saved.repository_rules, vec![repository_rule(&saved)]);
    assert_eq!(saved.project_choices, choices);
    assert!(saved.allows_repository(scope, REPOSITORY));
    assert!(!saved.allows_repository(scope, "git.example.org/Group/Other"));
    assert!(!saved.allows(scope, "/Example/project/src"));
    assert!(!saved.allows_repository("other-scope", REPOSITORY));
    let raw = serde_json::to_value(&saved).unwrap();
    assert_eq!(raw["repositoryRules"][0]["anchorPath"], "/Example/project");
    assert_eq!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap(),
        Some(saved)
    );
}

#[test]
fn cloud_repository_replacement_checks_scope_label_paths_order_and_current_policy() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let initial = policy(&store);
    assert!(
        serde_json::to_value(&initial)
            .unwrap()
            .get("repositoryRules")
            .is_none()
    );
    let exact_job = job(&store, &initial);
    assert!(
        serde_json::to_value(&exact_job)
            .unwrap()
            .get("repository_origin")
            .is_none()
    );
    store.set_cloud_device_upload_enabled(false).unwrap();
    let policy = store.set_cloud_sync_paused(&initial, true).unwrap();
    let rule = repository_rule(&policy);
    let mut other = rule.clone();
    other.origin = "ssh://git@[::1]:2222/Group/Other".into();
    other.anchor_path = "/Example/other".into();
    let mut invalid = vec![
        vec![rule.clone(), rule.clone()],
        vec![other.clone(), rule.clone()],
        vec![RepositoryRule {
            scopes: ["other-scope".into()].into(),
            ..rule.clone()
        }],
        vec![RepositoryRule {
            anchor_path: "/Example/../project".into(),
            ..rule.clone()
        }],
    ];
    for origin in [
        "https://synthetic-user@git.example.org:8443/Group/Repo",
        "https://git.example.org/Group/Repo",
        "git.example.org/Group/Repo?token=secret",
        "git.example.org/Group/../Repo",
        "git.example.org:Group/Repo",
        "/Example/project",
    ] {
        invalid.push(vec![RepositoryRule {
            origin: origin.into(),
            ..rule.clone()
        }]);
    }
    for rules in invalid {
        assert!(
            store
                .replace_cloud_repository_rules(&policy, rules, Vec::new(), || true)
                .is_err()
        );
    }
    assert!(
        store
            .replace_cloud_repository_rules(&policy, Vec::new(), Vec::new(), || true)
            .is_err()
    );
    assert_eq!(
        store
            .replace_cloud_repository_rules(&policy, vec![rule.clone()], Vec::new(), || true)
            .unwrap_err()
            .to_string(),
        "Cloud exact folders still authorize saved uploads"
    );
    assert_eq!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap(),
        Some(policy.clone())
    );
    store.abandon_cloud_sync_unreserved(&exact_job).unwrap();
    let finished_job = store
        .cloud_sync_job(&exact_job.identity.capture_id)
        .unwrap();
    assert_eq!(
        store
            .replace_cloud_repository_rules(&policy, vec![rule.clone()], Vec::new(), || false)
            .unwrap_err()
            .to_string(),
        "Cloud account or upload controls changed"
    );
    assert_eq!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap(),
        Some(policy.clone())
    );
    let saved = store
        .replace_cloud_repository_rules(&policy, vec![rule, other], Vec::new(), || true)
        .unwrap();
    assert_ne!(saved.revision, policy.revision);
    assert!(saved.paused);
    assert!(!store.cloud_device_upload_enabled().unwrap());
    assert_eq!(saved.roots, policy.roots);
    assert_eq!(saved.project_choices, policy.project_choices);
    assert_eq!(
        store
            .cloud_sync_job(&exact_job.identity.capture_id)
            .unwrap(),
        finished_job
    );
    assert!(
        store
            .replace_cloud_repository_rules(&policy, Vec::new(), initial.projects, || true)
            .is_err()
    );
}

#[test]
fn cloud_repository_add_and_revoke_keep_legacy_job_and_other_folders() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let (roots, projects) = selections();
    let binding = (
        "account",
        "team",
        ORIGIN,
        ORIGIN,
        Some(("generation", "Team")),
    );
    let policy = store
        .save_cloud_sync_policy_for_membership(binding, &roots, &projects, &BTreeMap::new(), || {
            true
        })
        .unwrap();
    let exact_job = job(&store, &policy);
    let group = repository_group_id(REPOSITORY);
    let other_selection = ProjectSelection {
        source_path: "/Example/other-sources".into(),
        project_path: "/Example/clone".into(),
        ..projects[0].clone()
    };
    let anchors = [(other_selection.project_path.clone(), REPOSITORY.into())].into();
    let selected = [other_selection.clone()].into();
    assert!(
        store
            .set_cloud_sync_project_with_repositories(
                &policy,
                &group,
                true,
                (&selected, &anchors),
                false,
                || false,
            )
            .is_err()
    );
    let approved = store
        .set_cloud_sync_project_with_repositories(
            &policy,
            &group,
            true,
            (&selected, &anchors),
            false,
            || true,
        )
        .unwrap();
    let scope = &approved.roots[0].source_scope_id;
    assert_eq!(approved.roots[0].path, other_selection.source_path);
    assert_eq!(approved.projects, projects);
    assert!(exact_job.allowed(&approved));
    assert!(!approved.allows(scope, &other_selection.project_path));
    let paths = vec!["/Example/future-clone/src".to_string()];
    assert!(
        store
            .enqueue_cloud_sync_for_repository(
                &approved,
                scope,
                "/Example/wrong-path",
                &format!("{scope}/repo"),
                "one",
                (REPOSITORY, &paths),
            )
            .is_err()
    );
    let repo_job = store
        .enqueue_cloud_sync_for_repository(
            &approved,
            scope,
            &paths[0],
            &format!("{scope}/repo"),
            "one",
            (REPOSITORY, &paths),
        )
        .unwrap()
        .unwrap();
    assert_eq!(repo_job.repository_origin.as_deref(), Some(REPOSITORY));
    assert_eq!(repo_job.recorded_project_paths, Some(paths));
    assert!(repo_job.allowed(&approved));
    let mut different_membership = approved.clone();
    different_membership.membership_generation = Some("different".into());
    assert!(!repo_job.allowed(&different_membership));
    let mut legacy = approved.clone();
    legacy.membership_generation = None;
    assert!(!legacy.allows_repository(scope, REPOSITORY));
    assert!(
        store
            .revoke_cloud_repository(&approved, &group, REPOSITORY, || false)
            .is_err()
    );
    let revoked = store
        .revoke_cloud_repository(&approved, &group, REPOSITORY, || true)
        .unwrap();
    assert_eq!(revoked.project_choices.get(&group), Some(&false));
    assert_eq!(revoked.projects_for_display(), projects);
    assert!(revoked.repository_rules.is_empty());
    assert!(!repo_job.allowed(&revoked));
    assert!(exact_job.allowed(&revoked));
    assert_eq!(
        store.cloud_sync_job(&repo_job.identity.capture_id).unwrap(),
        Some(repo_job)
    );
}

#[test]
fn cloud_team_renewal_reviews_saved_repository_anchors_and_adds_verified_rules() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let (roots, projects) = selections();
    let binding = ("account", "team", ORIGIN, ORIGIN, Some(("old", "Team")));
    let anchors = [(projects[0].project_path.clone(), REPOSITORY.into())].into();
    let policy = store
        .save_cloud_sync_policy_with_repositories(
            binding,
            &roots,
            &projects,
            &BTreeMap::new(),
            &anchors,
            || true,
        )
        .unwrap();
    let policy = store.set_cloud_sync_paused(&policy, true).unwrap();
    store.set_cloud_device_upload_enabled(false).unwrap();
    let next = ("account", "team", ORIGIN, ORIGIN, Some(("new", "Team")));
    let new_origin = "git.example.org/Group/Other";
    let new_anchor = "/Example/other";
    let additions = [ProjectSelection {
        source_path: "/Example/new-sources".into(),
        project_path: new_anchor.into(),
        ..projects[0].clone()
    }]
    .into();
    let new_choices = [(repository_group_id(new_origin), true)].into();
    let new_anchors = [(new_anchor.to_owned(), new_origin.to_owned())].into();
    let args = Some((&additions, &new_choices, &new_anchors));
    let confirmed = [projects[0].project_path.clone()].into();
    assert!(
        store
            .renew_cloud_team_sync_policy_with_repositories(
                next,
                &policy.revision,
                &[new_anchor.to_owned()].into(),
                args,
                || true,
            )
            .is_err()
    );
    assert!(
        store
            .renew_cloud_team_sync_policy_with_repositories(
                next,
                &policy.revision,
                &confirmed,
                args,
                || false,
            )
            .is_err()
    );
    assert_eq!(
        store.cloud_sync_policy("account", "team", ORIGIN).unwrap(),
        Some(policy.clone())
    );
    let renewed = store
        .renew_cloud_team_sync_policy_with_repositories(
            next,
            &policy.revision,
            &confirmed,
            args,
            || true,
        )
        .unwrap();
    assert_eq!(renewed.membership_generation.as_deref(), Some("new"));
    assert!(renewed.paused);
    assert!(!store.cloud_device_upload_enabled().unwrap());
    assert!(renewed.projects.is_empty());
    assert_eq!(renewed.repository_rules.len(), 2);
    assert!(
        renewed
            .repository_rules
            .contains(&policy.repository_rules[0])
    );
    assert_eq!(renewed.project_choices, new_choices);
    for rule in &renewed.repository_rules {
        assert_eq!(rule.scopes.len(), 1);
        assert!(renewed.allows_repository(rule.scopes.first().unwrap(), &rule.origin));
    }
}

#[test]
fn cloud_repository_migration_converts_only_approved_bindings_after_exact_jobs_finish() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let (mut roots, mut projects) = selections();
    let path = projects[0].project_path.clone();
    let other_source = "/Example/z-sources";
    roots.push(SourceSelection {
        source_kind: roots[0].source_kind,
        path: other_source.into(),
    });
    let untouched = ProjectSelection {
        project_path: "/Example/unrelated".into(),
        ..projects[0].clone()
    };
    projects.push(untouched.clone());
    projects.push(ProjectSelection {
        source_path: other_source.into(),
        ..projects[0].clone()
    });
    let policy = store
        .save_cloud_sync_policy_for_membership(
            ("account", "personal", ORIGIN, ORIGIN, None),
            &roots,
            &projects,
            &BTreeMap::new(),
            || true,
        )
        .unwrap();
    let job = job(&store, &policy);
    let anchors = [(path.clone(), REPOSITORY.to_owned())].into();
    assert_eq!(
        store
            .migrate_cloud_repository_anchors(&policy, &anchors, || true)
            .unwrap_err()
            .to_string(),
        "Cloud exact folders still authorize saved uploads"
    );
    store.abandon_cloud_sync_unreserved(&job).unwrap();
    assert!(
        store
            .migrate_cloud_repository_anchors(
                &policy,
                &[("/Example/unknown".into(), REPOSITORY.into())].into(),
                || true
            )
            .is_err()
    );
    let saved = store
        .migrate_cloud_repository_anchors(&policy, &anchors, || true)
        .unwrap();
    assert_eq!(saved.projects, vec![untouched]);
    assert_eq!(saved.projects_for_display(), projects);
    assert_eq!(saved.roots, policy.roots);
    assert_eq!(saved.repository_rules[0].scopes.len(), 2);
    for root in &saved.roots {
        assert!(!saved.allows(&root.source_scope_id, &path));
        assert!(saved.allows_repository(&root.source_scope_id, REPOSITORY));
    }
}

#[test]
fn cloud_repository_migration_keeps_ambiguous_legacy_rules_exact() {
    for with_exclusion in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        let (roots, projects) = selections();
        let choices = if with_exclusion {
            BTreeMap::from([("unmapped-old-group".into(), false)])
        } else {
            BTreeMap::new()
        };
        let policy = store
            .save_cloud_sync_policy_for_membership(
                ("account", "personal", ORIGIN, ORIGIN, None),
                &roots,
                &projects,
                &choices,
                || true,
            )
            .unwrap();
        if !with_exclusion {
            store
                .save_cloud_sync_policy_for_membership(
                    ("account", "team", ORIGIN, ORIGIN, Some(("current", "Team"))),
                    &roots,
                    &projects,
                    &BTreeMap::new(),
                    || true,
                )
                .unwrap();
        }
        let anchors = [(projects[0].project_path.clone(), REPOSITORY.into())].into();
        assert!(
            store
                .migrate_cloud_repository_anchors(&policy, &anchors, || true)
                .is_err()
        );
        assert_eq!(
            store
                .cloud_sync_policy("account", "personal", ORIGIN)
                .unwrap(),
            Some(policy)
        );
    }
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
fn cloud_saved_team_policy_accepts_512_bindings_and_rejects_growth() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let roots: Vec<_> = (0..8)
        .map(|index| SourceSelection {
            source_kind: SourceKind::ClaudeJsonl,
            path: format!("/Example/source-{index}"),
        })
        .collect();
    let mut projects = Vec::new();
    for root in &roots {
        for index in 0..64 {
            projects.push(ProjectSelection {
                source_kind: root.source_kind,
                source_path: root.path.clone(),
                project_path: format!("/Example/worktree-{index:02}"),
            });
        }
    }
    assert_eq!(projects.len(), MAX_SYNC_PROJECT_BINDINGS);
    let initial = store
        .save_cloud_sync_policy_for_grant(
            ("account", "team", ORIGIN, ORIGIN, Some(("old", "Team"))),
            &roots,
            &projects,
            &BTreeMap::new(),
        )
        .unwrap();
    let saved = store
        .saved_team_sync_policies("account", "personal", ORIGIN, ORIGIN)
        .unwrap();
    assert_eq!(saved, vec![initial.clone()]);
    let paths = projects
        .iter()
        .map(|project| project.project_path.clone())
        .collect();
    let renewed = store
        .renew_cloud_team_sync_policy(
            ("account", "team", ORIGIN, ORIGIN, Some(("new", "Team"))),
            &initial.revision,
            &paths,
            None,
            || true,
        )
        .unwrap();
    assert_eq!(renewed.projects, projects);
    assert_eq!(renewed.roots, initial.roots);
    let extra = ProjectSelection {
        source_path: "/Example/source-8".into(),
        ..projects[0].clone()
    };
    let outcome =
        store.set_cloud_sync_project(&renewed, "extra", true, &[extra].into(), false, || true);
    assert_eq!(
        outcome.unwrap_err().to_string(),
        "Cloud project bindings exceed the policy limit"
    );
    assert_eq!(
        store.cloud_sync_policy("account", "team", ORIGIN).unwrap(),
        Some(renewed)
    );
}

#[test]
fn cloud_policy_growth_caps_combined_distinct_paths_and_roots() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let original = policy(&store);
    let first = &original.projects[0];
    let extra_paths: BTreeSet<_> = (0..MAX_SYNC_PROJECT_PATHS)
        .map(|index| ProjectSelection {
            project_path: format!("/Example/worktree-{index}"),
            ..first.clone()
        })
        .collect();
    let extra_roots: BTreeSet<_> = (0..MAX_SYNC_SOURCE_ROOTS)
        .map(|index| ProjectSelection {
            source_path: format!("/Example/source-{index}"),
            ..first.clone()
        })
        .collect();
    for (additions, expected) in [
        (
            extra_paths,
            "Cloud project directories exceed the policy limit",
        ),
        (extra_roots, "Cloud source roots exceed the policy limit"),
    ] {
        check_project_binding_limits(additions.iter()).unwrap();
        let error = store
            .set_cloud_sync_project(&original, "extra", true, &additions, false, || true)
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert_eq!(
            store
                .cloud_sync_policy("account", "personal", ORIGIN)
                .unwrap(),
            Some(original.clone())
        );
    }
}

#[test]
fn cloud_personal_consent_and_jobs_do_not_follow_an_issuer_change() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let first = policy(&store);
    let old_job = job(&store, &first);
    let other_issuer = "https://issuer.example";
    assert!(old_job.allowed(&first));
    let mut legacy_job = old_job.clone();
    legacy_job.issuer_origin = None;
    assert!(!legacy_job.allowed(&first));
    assert!(
        store
            .cloud_sync_policy_for_origins("account", "personal", ORIGIN, other_issuer)
            .unwrap()
            .is_none()
    );

    let (roots, projects) = selections();
    let replacement = store
        .save_cloud_sync_policy_for_grant(
            ("account", "personal", ORIGIN, other_issuer, None),
            &roots,
            &projects,
            &BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(replacement.resource_origin, first.resource_origin);
    assert_eq!(replacement.roots, first.roots);
    assert!(!old_job.allowed(&replacement));
    assert_eq!(
        store.cloud_sync_job(&old_job.identity.capture_id).unwrap(),
        Some(old_job)
    );

    store
        .lock()
        .execute(
            "UPDATE cloud_sync_policy SET record_json=json_remove(record_json, '$.issuerOrigin')",
            [],
        )
        .unwrap();
    assert!(
        store
            .cloud_sync_policy_for_origins("account", "personal", ORIGIN, other_issuer)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap()
            .is_none()
    );
}

#[test]
fn cloud_source_upgrade_keeps_the_legacy_receipt_and_deletion_marker() {
    const LEGACY_SCOPE: &str = "legacy-source-scope";
    const SOURCE_ID: &str = "legacy-source-scope/session";
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let (roots, projects) = selections();
    store
        .lock()
        .execute(
            "INSERT INTO cloud_sync_service_scope VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                "account",
                "personal",
                ORIGIN,
                serde_json::to_string(&roots[0].source_kind).unwrap(),
                roots[0].path,
                LEGACY_SCOPE
            ],
        )
        .unwrap();
    let old = policy(&store);
    assert_eq!(old.roots[0].source_scope_id, LEGACY_SCOPE);
    let sent = job(&store, &old);
    let receipt = accepted(&sent);
    store.accept_cloud_sync(&receipt).unwrap();
    let deleted = store
        .enqueue_cloud_sync(
            &old,
            LEGACY_SCOPE,
            &sent.project_path,
            SOURCE_ID,
            "before deletion",
        )
        .unwrap()
        .unwrap();
    store.suppress_cloud_sync(&deleted).unwrap();
    store
        .lock()
        .execute_batch(
            "UPDATE cloud_sync_policy SET record_json=json_remove(record_json, '$.issuerOrigin');
             UPDATE cloud_sync_job SET record_json=json_remove(record_json, '$.issuer_origin');",
        )
        .unwrap();
    drop(store);

    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    assert!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap()
            .is_none()
    );
    let renewed = store
        .save_cloud_sync_policy_for_grant(
            (
                "account",
                "personal",
                ORIGIN,
                "https://issuer.example",
                None,
            ),
            &roots,
            &projects,
            &BTreeMap::new(),
        )
        .unwrap();
    let saved_scope = &renewed.roots[0].source_scope_id;
    assert_eq!(saved_scope, LEGACY_SCOPE);
    let source = store
        .cloud_sync_source("account", "personal", ORIGIN, SOURCE_ID)
        .unwrap()
        .unwrap();
    assert!(source.suppressed);
    assert_eq!(source.fingerprint, receipt.accepted_source_fingerprint());
    let legacy = store
        .cloud_sync_job(&deleted.identity.capture_id)
        .unwrap()
        .unwrap();
    assert!(legacy.issuer_origin.is_none());
    assert!(!legacy.allowed(&renewed));
    assert!(
        store
            .enqueue_cloud_sync(
                &renewed,
                saved_scope,
                &sent.project_path,
                SOURCE_ID,
                "changed after upgrade",
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn cloud_only_jobs_with_a_known_issuer_hold_active_queue_slots() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let old = policy(&store);
    let scope = &old.roots[0].source_scope_id;
    let enqueue = |policy: &Policy, index| {
        store.enqueue_cloud_sync(
            policy,
            scope,
            "/Example/project",
            &format!("{scope}/session-{index}"),
            "observation",
        )
    };
    let mut known = None;
    for index in 0..MAX_ACTIVE_JOBS {
        known = enqueue(&old, index).unwrap();
        assert!(known.is_some());
    }
    let known = known.unwrap();
    let (roots, projects) = selections();
    let current = store
        .save_cloud_sync_policy_for_grant(
            (
                "account",
                "personal",
                ORIGIN,
                "https://issuer.example",
                None,
            ),
            &roots,
            &projects,
            &BTreeMap::new(),
        )
        .unwrap();
    assert!(!known.allowed(&current));
    assert_eq!(
        enqueue(&current, MAX_ACTIVE_JOBS).unwrap_err().to_string(),
        "Cloud queue is full"
    );
    store
        .lock()
        .execute(
            "UPDATE cloud_sync_job SET record_json=json_remove(record_json, '$.issuer_origin')
         WHERE capture_id<>?1",
            [&known.identity.capture_id],
        )
        .unwrap();

    assert!(enqueue(&current, MAX_ACTIVE_JOBS - 1).unwrap().is_none());
    let fresh = enqueue(&current, 0).unwrap().unwrap();
    let unrelated = enqueue(&current, MAX_ACTIVE_JOBS).unwrap().unwrap();
    assert_eq!(
        store
            .cloud_sync_jobs("account", "personal", ORIGIN)
            .unwrap(),
        vec![known, fresh, unrelated]
    );
}

#[test]
fn cloud_team_membership_reapproval_needs_empty_current_consent_and_keeps_old_jobs() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let (roots, projects) = selections();
    let choices = BTreeMap::new();
    let binding = |generation| {
        (
            "account",
            "team",
            ORIGIN,
            ORIGIN,
            Some((generation, "Example Team")),
        )
    };
    let first = store
        .save_cloud_sync_policy_for_membership(
            binding("generation-one"),
            &roots,
            &projects,
            &choices,
            || true,
        )
        .unwrap();
    let old_job = job(&store, &first);
    assert_eq!(
        old_job.membership_generation.as_deref(),
        Some("generation-one")
    );
    assert!(old_job.team_grant_id.is_none());
    assert!(old_job.allowed(&first));
    assert!(
        store
            .save_cloud_sync_policy_for_membership(
                binding("generation-two"),
                &roots,
                &projects,
                &choices,
                || true,
            )
            .is_err()
    );
    assert!(
        store
            .save_cloud_sync_policy_for_membership(
                (
                    "account",
                    "team",
                    ORIGIN,
                    "https://issuer.example",
                    Some(("generation-two", "Example Team"))
                ),
                &roots,
                &projects,
                &choices,
                || true,
            )
            .is_err()
    );
    let reapprove = |previous, generation, current: fn() -> bool| {
        store.reapprove_cloud_sync_policy_for_membership(
            previous,
            binding(generation),
            &roots,
            &projects,
            &choices,
            current,
        )
    };
    assert!(reapprove(&first, "generation-two", || true).is_err());
    let empty = store
        .set_cloud_sync_project(
            &first,
            "folder",
            false,
            &projects.iter().cloned().collect(),
            false,
            || true,
        )
        .unwrap();
    assert!(empty.projects.is_empty());
    assert_ne!(empty.revision, first.revision);
    assert!(reapprove(&first, "generation-two", || true).is_err());
    assert!(reapprove(&empty, "generation-one", || true).is_err());
    assert!(reapprove(&empty, "generation-two", || false).is_err());
    assert_eq!(
        store.cloud_sync_policy("account", "team", ORIGIN).unwrap(),
        Some(empty.clone())
    );

    store.set_cloud_device_upload_enabled(false).unwrap();
    let renewed = reapprove(&empty, "generation-two", || true).unwrap();
    assert!(!store.cloud_device_upload_enabled().unwrap());
    assert!(!old_job.matches_membership(&renewed));
    assert!(!old_job.allowed(&renewed));
    assert_eq!(renewed.roots, first.roots);

    store.set_cloud_device_upload_enabled(true).unwrap();
    assert!(
        store
            .enqueue_cloud_sync(
                &first,
                &old_job.source_scope_id,
                &old_job.project_path,
                &old_job.identity.source_id,
                "other-observation",
            )
            .is_err()
    );

    let new_job = job(&store, &renewed);
    assert_eq!(new_job.identity.source_id, old_job.identity.source_id);
    assert_ne!(new_job.identity.capture_id, old_job.identity.capture_id);
    assert_eq!(
        new_job.membership_generation.as_deref(),
        Some("generation-two")
    );
    assert_eq!(
        store.cloud_sync_job(&old_job.identity.capture_id).unwrap(),
        Some(old_job)
    );
}

#[test]
fn cloud_device_upload_resume_keeps_the_exact_previous_preference_on_rejection() {
    for previous in [None, Some(false), Some(true)] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        if let Some(enabled) = previous {
            store.set_cloud_device_upload_enabled(enabled).unwrap();
        }

        assert!(
            !store
                .resume_cloud_device_uploads(|| false, || panic!("stale before commit"))
                .unwrap()
        );
        assert_eq!(
            saved_device_upload_enabled(&store.lock()).unwrap(),
            previous
        );

        assert!(
            !store
                .resume_cloud_device_uploads(|| true, || false)
                .unwrap()
        );
        assert_eq!(
            saved_device_upload_enabled(&store.lock()).unwrap(),
            previous
        );

        assert!(store.resume_cloud_device_uploads(|| true, || true).unwrap());
        assert!(store.cloud_device_upload_enabled().unwrap());
    }
}

#[test]
fn cloud_device_upload_rejection_needs_no_compensating_write() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    store.set_cloud_device_upload_enabled(false).unwrap();
    store
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_compensating_off
             BEFORE UPDATE OF enabled ON cloud_device_upload
             WHEN NEW.enabled = 0
             BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END;",
        )
        .unwrap();

    assert!(
        !store
            .resume_cloud_device_uploads(|| true, || false)
            .unwrap()
    );
    drop(store);
    let reopened = Store::open(directory.path()).unwrap();
    reopened.initialize_cloud_setup().unwrap();
    assert_eq!(
        saved_device_upload_enabled(&reopened.lock()).unwrap(),
        Some(false)
    );
}

#[test]
fn cloud_legacy_team_renewal_preserves_choices_and_cannot_authorize_old_jobs() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let (roots, mut projects) = selections();
    projects.push(ProjectSelection {
        project_path: "/projects/second".into(),
        ..projects[0].clone()
    });
    projects.sort();
    let choices = BTreeMap::from([("folder".into(), true), ("excluded".into(), false)]);
    let binding = (
        "account",
        "team",
        ORIGIN,
        ORIGIN,
        Some(("synthetic-generation", "Example Team")),
    );
    let original = store
        .save_cloud_sync_policy_for_membership(binding, &roots, &projects, &choices, || true)
        .unwrap();
    let mut legacy_job = job(&store, &original);
    legacy_job.membership_generation = None;
    legacy_job.team_grant_id = Some("legacy-grant".into());
    store
        .lock()
        .execute(
            "UPDATE cloud_sync_job SET record_json=?1 WHERE capture_id=?2",
            params![
                encode(&legacy_job, 16_384).unwrap(),
                legacy_job.identity.capture_id
            ],
        )
        .unwrap();
    let legacy = store
        .save_cloud_sync_policy_for_grant(
            (
                "account",
                "team",
                ORIGIN,
                ORIGIN,
                Some(("legacy-grant", "Example Team")),
            ),
            &roots,
            &projects,
            &choices,
        )
        .unwrap();
    assert!(legacy.is_team());
    assert_eq!(
        store
            .saved_team_sync_policies("account", "personal", ORIGIN, ORIGIN)
            .unwrap(),
        vec![legacy.clone()]
    );
    assert_eq!(
        store
            .cloud_sync_job(&legacy_job.identity.capture_id)
            .unwrap(),
        Some(legacy_job.clone())
    );
    assert!(!legacy_job.allowed(&legacy));
    assert!(!legacy_job.allowed(&original));
    assert!(
        store
            .enqueue_cloud_sync(
                &legacy,
                &legacy_job.source_scope_id,
                &legacy_job.project_path,
                &legacy_job.identity.source_id,
                "other-observation",
            )
            .is_err()
    );
    let projects_set = projects.iter().cloned().collect();
    assert!(
        store
            .set_cloud_sync_project(&legacy, "folder", true, &projects_set, false, || true)
            .is_err()
    );
    let legacy = store.set_cloud_sync_paused(&legacy, true).unwrap();
    store.set_cloud_device_upload_enabled(false).unwrap();
    let confirmed: BTreeSet<_> = projects
        .iter()
        .map(|project| project.project_path.clone())
        .collect();
    let mut wider = confirmed.clone();
    wider.insert("/projects".into());
    let subset = BTreeSet::from([projects[0].project_path.clone()]);
    for paths in [&wider, &subset] {
        assert!(
            store
                .renew_cloud_team_sync_policy(binding, &legacy.revision, paths, None, || true)
                .is_err()
        );
    }
    let (account, team, resource, issuer, proof) = binding;
    let other_origin = "https://other.example";
    for changed_binding in [
        ("other-account", team, resource, issuer, proof),
        (account, "other-team", resource, issuer, proof),
        (account, team, other_origin, issuer, proof),
        (account, team, resource, other_origin, proof),
        (account, team, resource, issuer, None),
    ] {
        assert!(
            store
                .renew_cloud_team_sync_policy(
                    changed_binding,
                    &legacy.revision,
                    &confirmed,
                    None,
                    || { true }
                )
                .is_err()
        );
    }
    assert!(
        store
            .renew_cloud_team_sync_policy(binding, &original.revision, &confirmed, None, || true)
            .is_err()
    );
    let added = ProjectSelection {
        source_kind: SourceKind::OpenCodeSqliteV2,
        source_path: "/Example/new.db".into(),
        project_path: "/Example/new-project".into(),
    };
    let added_projects = BTreeSet::from([added.clone()]);
    let added_choices = BTreeMap::from([("new-folder".into(), true)]);
    let additions = Some((&added_projects, &added_choices));
    for (paths, current) in [(&subset, true), (&confirmed, false)] {
        assert!(
            store
                .renew_cloud_team_sync_policy(binding, &legacy.revision, paths, additions, || {
                    current
                })
                .is_err()
        );
    }
    assert_eq!(
        store.cloud_sync_policy("account", "team", ORIGIN).unwrap(),
        Some(legacy.clone())
    );
    let approved = store
        .renew_cloud_team_sync_policy(binding, &legacy.revision, &confirmed, additions, || true)
        .unwrap();
    assert_ne!(approved.revision, legacy.revision);
    assert!(approved.team_grant_id.is_none());
    assert_eq!(
        approved.membership_generation.as_deref(),
        Some("synthetic-generation")
    );
    assert!(
        legacy
            .roots
            .iter()
            .all(|root| approved.roots.contains(root))
    );
    assert_eq!(approved.roots.len(), legacy.roots.len() + 1);
    let mut expected_projects = projects;
    expected_projects.push(added);
    expected_projects.sort();
    assert_eq!(approved.projects, expected_projects);
    assert_eq!(
        approved.project_choices,
        choices.into_iter().chain(added_choices).collect()
    );
    assert!(approved.paused);
    assert!(!store.cloud_device_upload_enabled().unwrap());
    assert!(
        store
            .renew_cloud_team_sync_policy(binding, &approved.revision, &confirmed, None, || true)
            .is_err()
    );
    assert!(!legacy_job.matches_membership(&approved));
    assert!(
        !Job {
            team_grant_id: None,
            ..legacy_job.clone()
        }
        .matches_membership(&approved)
    );
    assert_eq!(
        store
            .cloud_sync_job(&legacy_job.identity.capture_id)
            .unwrap(),
        Some(legacy_job)
    );
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
fn cloud_device_upload_preference_survives_restart_without_changing_bound_work() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    assert!(!store.cloud_device_upload_enabled().unwrap());
    let personal = policy(&store);
    assert!(store.cloud_device_upload_enabled().unwrap());
    let mut saved_job = job(&store, &personal);
    store
        .set_cloud_sync_job_state(&saved_job, JobState::Interrupted)
        .unwrap();
    saved_job.state = JobState::Interrupted;
    store.set_cloud_device_upload_enabled(false).unwrap();

    let (roots, mut projects) = selections();
    projects[0].project_path = "/Example/team".into();
    let team = store
        .save_cloud_sync_policy_for_membership(
            (
                "account",
                "team",
                ORIGIN,
                ORIGIN,
                Some(("team-generation", "Team")),
            ),
            &roots,
            &projects,
            &BTreeMap::new(),
            || true,
        )
        .unwrap();
    drop(store);
    store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    assert!(!store.cloud_device_upload_enabled().unwrap());

    let enqueue = |policy: &Policy| {
        let scope = &policy.roots[0].source_scope_id;
        store.enqueue_cloud_sync(
            policy,
            scope,
            &policy.projects[0].project_path,
            &format!("{scope}/new"),
            "new-observation",
        )
    };
    assert!(enqueue(&personal).is_err());
    assert!(enqueue(&team).is_err());

    store.set_cloud_device_upload_enabled(true).unwrap();
    assert!(store.cloud_device_upload_enabled().unwrap());
    assert!(enqueue(&team).unwrap().is_some());
    assert_eq!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap(),
        Some(personal)
    );
    assert_eq!(
        store.cloud_sync_policy("account", "team", ORIGIN).unwrap(),
        Some(team)
    );
    assert_eq!(
        store
            .cloud_sync_job(&saved_job.identity.capture_id)
            .unwrap(),
        Some(saved_job)
    );
}

#[test]
fn cloud_device_upload_upgrade_preserves_the_saved_workspace_pause() {
    for paused in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        let saved = store
            .set_cloud_sync_paused(&policy(&store), paused)
            .unwrap();
        store
            .lock()
            .execute("DROP TABLE cloud_device_upload", [])
            .unwrap();

        store.initialize_cloud_setup().unwrap();
        let enabled: bool = store
            .lock()
            .query_row(
                "SELECT enabled FROM cloud_device_upload WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(enabled, !paused);
        assert_eq!(store.cloud_device_upload_enabled().unwrap(), enabled);
        store.set_cloud_device_upload_enabled(!enabled).unwrap();
        store.initialize_cloud_setup().unwrap();
        assert_eq!(store.cloud_device_upload_enabled().unwrap(), !enabled);
        assert_eq!(
            store
                .cloud_sync_policy("account", "personal", ORIGIN)
                .unwrap(),
            Some(saved)
        );
    }
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
fn explicit_scoped_save_replaces_issuerless_policy_but_not_its_jobs() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let mut legacy = policy(&store);
    let mut old_job = job(&store, &legacy);
    legacy.issuer_origin = None;
    old_job.issuer_origin = None;
    {
        let connection = store.lock();
        connection
            .execute(
                "UPDATE cloud_sync_policy SET record_json=?1",
                [encode(&legacy, 524_288).unwrap()],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE cloud_sync_job SET record_json=?1",
                [encode(&old_job, 16_384).unwrap()],
            )
            .unwrap();
    }
    assert!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap()
            .is_none()
    );
    store.set_cloud_device_upload_enabled(false).unwrap();
    let (roots, projects) = selections();
    let binding = ("account", "personal", ORIGIN, ORIGIN, None);
    let current = store
        .save_cloud_sync_policy_for_membership(binding, &roots, &projects, &BTreeMap::new(), || {
            true
        })
        .unwrap();
    assert_ne!(current.revision, legacy.revision);
    assert_eq!(current.issuer_origin.as_deref(), Some(ORIGIN));
    assert!(!old_job.allowed(&current));
    assert_eq!(
        store.cloud_sync_job(&old_job.identity.capture_id).unwrap(),
        Some(old_job)
    );
    assert!(!store.cloud_device_upload_enabled().unwrap());
    assert!(
        store
            .save_cloud_sync_policy_for_membership(
                binding,
                &roots,
                &projects,
                &BTreeMap::new(),
                || true,
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
        .set_cloud_sync_project(&paused, &other_group, true, &other_projects, false, || true)
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
            false,
            || true,
        )
        .unwrap();
    assert_eq!(narrowed.projects, [added]);
    assert_eq!(narrowed.roots, [added_scope]);
    assert!(!narrowed.project_choices[&original_group]);
    assert!(narrowed.project_choices[&other_group]);
    let empty = store
        .set_cloud_sync_project(
            &narrowed,
            &other_group,
            false,
            &other_projects,
            false,
            || true,
        )
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
            false,
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
            .set_cloud_sync_project(&original, &group, true, &projects, true, || true)
            .is_err()
    );
    assert!(
        store
            .set_cloud_sync_project(&paused, &group, true, &projects, true, || false)
            .is_err()
    );
    let mut wrong_service = paused.clone();
    wrong_service.resource_origin = Some("https://other.example".into());
    assert!(
        store
            .set_cloud_sync_project(&wrong_service, &group, true, &projects, true, || true)
            .is_err()
    );
    assert_eq!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap(),
        Some(paused.clone())
    );
    let scopes: u64 = store
        .lock()
        .query_row("SELECT count(*) FROM cloud_sync_service_scope", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(scopes, 1);
    store.set_cloud_device_upload_enabled(false).unwrap();
    let saved = store
        .set_cloud_sync_project(&paused, &group, true, &projects, true, || true)
        .unwrap();
    assert!(!saved.paused);
    assert!(!store.cloud_device_upload_enabled().unwrap());
}

#[test]
fn first_folder_save_rolls_back_policy_and_device_enable_on_stale_generation() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let (roots, projects) = selections();
    assert!(
        store
            .save_cloud_sync_policy_for_membership(
                ("account", "personal", ORIGIN, ORIGIN, None),
                &roots,
                &projects,
                &BTreeMap::new(),
                || false,
            )
            .is_err()
    );
    assert!(
        store
            .cloud_sync_policy("account", "personal", ORIGIN)
            .unwrap()
            .is_none()
    );
    assert!(!store.cloud_device_upload_enabled().unwrap());
}
