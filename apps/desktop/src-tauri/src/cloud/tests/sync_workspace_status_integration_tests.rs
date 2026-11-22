use super::*;
use std::cell::Cell;

const ISSUER: &str = "https://issuer.example";
const RESOURCE: &str = "https://api.example";
const ACCOUNT: &str = "synthetic-user";
const PERSONAL: &str = "synthetic-personal";
const TEAM: &str = "synthetic-team";
const FIRST: &str = "/Example/Saved Folder";
const SECOND: &str = "/Example/Other Folder";

fn anchor(directory: &std::path::Path) -> Engine {
    Engine::new(
        auth::Config::parse(ISSUER, RESOURCE),
        Box::new(Arc::new(crate::cloud::tests::MemoryVault::default())),
        Box::new(auth::HttpTransport),
        super::super::state::StateStore::new(directory),
    )
}

fn policy(store: &Store, account: &str, workspace: &str, resource: &str, issuer: &str) -> Policy {
    let root = SourceSelection {
        source_kind: SourceKind::ClaudeJsonl,
        path: "/Example/Provider".into(),
    };
    let mut projects = [FIRST, SECOND].map(|path| ProjectSelection {
        source_kind: root.source_kind,
        source_path: root.path.clone(),
        project_path: path.into(),
    });
    projects.sort();
    store
        .save_cloud_sync_policy_for_grant(
            (
                account,
                workspace,
                resource,
                issuer,
                Some(("team-grant", "Example Team")),
            ),
            &[root],
            &projects,
            &BTreeMap::new(),
        )
        .unwrap()
}

fn personal() -> auth::Session {
    let mut session = crate::cloud::tests::session();
    session.user.id = ACCOUNT.into();
    session.workspace.id = PERSONAL.into();
    session.resource = RESOURCE.into();
    session.iss = ISSUER.into();
    session
}

fn off(policy: &Policy, path: &str) -> ProjectChange {
    ProjectChange {
        account_id: policy.account_id.clone(),
        destination_id: policy.destination_id.clone(),
        group_id: saved_team_folder_id(path),
        selected: false,
        projects: Vec::new(),
        folder_paths: vec![path.into()],
        expected_policy_revision: policy.revision.clone(),
    }
}

#[test]
fn cloud_status_lists_disconnected_team_consent_only_for_the_verified_personal_scope() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let saved = policy(&store, ACCOUNT, TEAM, RESOURCE, ISSUER);
    policy(
        &store,
        "other-account",
        "other-account-team",
        RESOURCE,
        ISSUER,
    );
    policy(
        &store,
        ACCOUNT,
        "other-issuer-team",
        RESOURCE,
        "https://other-issuer.example",
    );
    policy(
        &store,
        ACCOUNT,
        "other-resource-team",
        "https://other-api.example",
        ISSUER,
    );
    policy(&store, ACCOUNT, PERSONAL, RESOURCE, ISSUER);
    policy(&store, ACCOUNT, "legacy-team", RESOURCE, ISSUER);
    store
        .lock()
        .execute(
            "UPDATE cloud_sync_policy SET record_json=json_remove(record_json, '$.issuerOrigin')
         WHERE destination_id='legacy-team'",
            [],
        )
        .unwrap();

    let mut runners = BTreeMap::new();
    // A previously live status must not claim ready without a current grant.
    runners.insert(
        (ACCOUNT.into(), TEAM.into()),
        Runner {
            last_status: Some(Status {
                status: State::Ready,
                account_id: Some(ACCOUNT.into()),
                destination_id: Some(TEAM.into()),
                policy_revision: Some(saved.revision.clone()),
                storage_known: true,
                retry_available: true,
                queued_count: 7,
                ..Status::default()
            }),
            ..Runner::default()
        },
    );
    let mut anchor = anchor(directory.path());
    let statuses = saved_team_statuses(
        &store,
        Some(&personal()),
        &runners,
        (&mut anchor, "synthetic"),
        None,
    )
    .unwrap();
    let wire = serde_json::to_value(Status {
        team_statuses: statuses,
        ..Status::default()
    })
    .unwrap();
    let teams = wire["teamStatuses"].as_array().unwrap();
    assert_eq!(teams.len(), 1);
    let team = &teams[0];
    assert_eq!(team["accountId"], ACCOUNT);
    assert_eq!(team["destinationId"], TEAM);
    assert_eq!(team["workspaceName"], "Example Team");
    assert_eq!(team["status"], "reauth_required");
    assert_eq!(team["consentVersion"], ONGOING_CONSENT_VERSION);
    assert_eq!(team["policyRevision"], saved.revision);
    assert_eq!(team["storageKnown"], false);
    assert_eq!(team["retryAvailable"], false);
    assert_eq!(team["folderConfirmationRequired"], false);
    assert_eq!(team["queuedCount"], 0);
    assert_eq!(
        team["savedFolders"],
        serde_json::json!([
            { "groupId": saved_team_folder_id(SECOND), "path": SECOND },
            { "groupId": saved_team_folder_id(FIRST), "path": FIRST }
        ])
    );
    assert!(team.get("teamGrantId").is_none());
    let mut team_principal = personal();
    team_principal.workspace.kind = auth::WorkspaceKind::Team;
    assert!(
        saved_team_statuses(
            &store,
            Some(&team_principal),
            &runners,
            (&mut anchor, "synthetic"),
            None,
        )
        .unwrap()
        .is_empty()
    );
    assert!(
        saved_team_statuses(&store, None, &runners, (&mut anchor, "synthetic"), None)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cloud_team_folder_confirmation_requires_current_eligibility_and_stale_consent() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let legacy = policy(&store, ACCOUNT, TEAM, RESOURCE, ISSUER);
    let eligible = |id: &str, kind, generation: Option<&str>| auth::EligibleWorkspace {
        workspace: auth::Workspace {
            id: id.into(),
            name: "Example Team".into(),
            kind,
        },
        membership_generation: generation.map(str::to_owned),
    };
    let team = |generation| eligible(TEAM, auth::WorkspaceKind::Team, Some(generation));
    let confirm_required = |policy, entries: Option<&[auth::EligibleWorkspace]>| {
        team_policy_status(policy, None, entries).folder_confirmation_required
    };
    assert!(!confirm_required(&legacy, None));
    assert!(!confirm_required(&legacy, Some(&[])));
    assert!(!confirm_required(
        &legacy,
        Some(&[eligible(
            "other-team",
            auth::WorkspaceKind::Team,
            Some("member")
        )])
    ));
    assert!(!confirm_required(
        &legacy,
        Some(&[eligible(TEAM, auth::WorkspaceKind::Personal, None)])
    ));
    assert!(!confirm_required(
        &legacy,
        Some(&[eligible(TEAM, auth::WorkspaceKind::Team, None)])
    ));
    assert!(confirm_required(&legacy, Some(&[team("member")])));
    let wire =
        serde_json::to_value(team_policy_status(&legacy, None, Some(&[team("member")]))).unwrap();
    assert_eq!(wire["folderConfirmationRequired"], true);
    let renewed = store
        .renew_cloud_team_sync_policy(
            (
                ACCOUNT,
                TEAM,
                RESOURCE,
                ISSUER,
                Some(("member", "Example Team")),
            ),
            &legacy.revision,
            &[FIRST.into(), SECOND.into()].into(),
            None,
            || true,
        )
        .unwrap();
    assert!(!confirm_required(&renewed, Some(&[team("member")])));
    assert!(!confirm_required(&renewed, None));
    assert!(confirm_required(&renewed, Some(&[team("new-member")])));
}

#[test]
fn cloud_retry_availability_is_false_when_device_upload_is_off() {
    let status = Status {
        retry_available: true,
        team_statuses: vec![Status {
            retry_available: true,
            ..Status::default()
        }],
        ..Status::default()
    };
    let value = serde_json::to_value(status.with_device_upload_enabled(false)).unwrap();
    assert_eq!(value["retryAvailable"], false);
    assert_eq!(value["teamStatuses"][0]["retryAvailable"], false);
}

#[test]
fn cloud_scheduler_keeps_unverified_team_work_without_running_it() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let mut policy = policy(&store, ACCOUNT, TEAM, RESOURCE, ISSUER);
    policy.team_grant_id = None;
    policy.membership_generation = Some("synthetic-membership".into());
    let session = personal();
    let key = (ACCOUNT.into(), TEAM.into());
    let deadline = Instant::now() + Duration::from_secs(30);
    let cursor = DiscoveryCursor {
        root: 0,
        after: Some("kept-page".into()),
    };
    let mut runners = BTreeMap::from([(
        key.clone(),
        Runner {
            team: Some((TEAM.into(), "synthetic-membership".into())),
            last_status: Some(Status {
                account_id: Some(ACCOUNT.into()),
                destination_id: Some(TEAM.into()),
                policy_revision: Some(policy.revision.clone()),
                retry_available: true,
                recovery_required_count: 2,
                ..Status::default()
            }),
            last_status_origin: Some(RESOURCE.into()),
            retry_grant_id: Some(session.grant_id.clone()),
            backfill_revision: Some(policy.revision.clone()),
            discovery_cursor: Some(cursor.clone()),
            retry_deadlines: BTreeMap::from([(
                "saved".into(),
                (deadline, upload::Failure::Offline),
            )]),
            conflict_attempts: BTreeMap::from([("saved".into(), 2)]),
            conflict_finalization_replays: BTreeMap::from([("saved".into(), 1)]),
            ..Runner::default()
        },
    )]);
    let policies = [policy];
    let mut observed = Vec::new();
    assert!(
        verified_team_turns(
            &policies,
            &session,
            &mut runners,
            Err(auth::Failure::Offline),
            |status| observed.push(status)
        )
        .is_empty()
    );
    assert_eq!(observed[0].status, State::Offline);
    assert!(observed[0].backfill_in_progress);
    assert_eq!(observed[0].recovery_required_count, 2);
    assert!(!observed[0].retry_available);
    assert_eq!(runners[&key].discovery_cursor.as_ref(), Some(&cursor));
    assert_eq!(runners[&key].retry_deadlines["saved"].0, deadline);
    assert_eq!(runners[&key].conflict_attempts["saved"], 2);
    assert_eq!(runners[&key].conflict_finalization_replays["saved"], 1);
    let eligible = [auth::EligibleWorkspace {
        workspace: auth::Workspace {
            id: TEAM.into(),
            name: "Example Team".into(),
            kind: auth::WorkspaceKind::Team,
        },
        membership_generation: Some("synthetic-membership".into()),
    }];
    assert_eq!(
        verified_team_turns(&policies, &session, &mut runners, Ok(&eligible), |_| {}).len(),
        1
    );
    assert_eq!(runners[&key].discovery_cursor.as_ref(), Some(&cursor));
    assert!(verified_team_turns(&policies, &session, &mut runners, Ok(&[]), |_| {}).is_empty());
    assert!(runners.is_empty());
}

#[test]
fn cloud_retry_keeps_discovery_and_unrelated_saved_retry_timers() {
    let deadline = Instant::now();
    let mut runner = Runner {
        next_reconcile: Some(deadline),
        next_reservation: Some(deadline),
        retry_deadlines: BTreeMap::from([
            ("retry-this".into(), (deadline, upload::Failure::Offline)),
            (
                "keep-this".into(),
                (deadline, upload::Failure::Backpressure),
            ),
        ]),
        finalization_submissions: BTreeSet::from(["retry-this".into(), "keep-this".into()]),
        ..Runner::default()
    };
    runner.retry(&BTreeSet::from(["retry-this".into()]));
    assert_eq!(runner.next_reconcile, Some(deadline));
    assert_eq!(runner.next_reservation, Some(deadline));
    assert!(!runner.retry_deadlines.contains_key("retry-this"));
    assert!(runner.retry_deadlines.contains_key("keep-this"));
    assert_eq!(
        runner.finalization_submissions,
        BTreeSet::from(["keep-this".into()])
    );
}

#[test]
fn cloud_legacy_team_cleanup_warning_does_not_create_a_policy() {
    use super::super::state::{Cleanup, Phase};
    const LOST: &str = "synthetic-lost-team";
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let mut anchor = anchor(directory.path());
    for workspace in [TEAM, LOST] {
        if workspace == TEAM {
            policy(&store, ACCOUNT, workspace, RESOURCE, ISSUER);
        }
        store
            .register_cloud_workspace_slot(ISSUER, RESOURCE, ACCOUNT, workspace)
            .unwrap();
        let engine = anchor
            .workspace_engine(
                "synthetic".into(),
                ACCOUNT.into(),
                auth::Workspace {
                    id: workspace.into(),
                    name: "Example Team".into(),
                    kind: auth::WorkspaceKind::Team,
                },
            )
            .unwrap();
        // This warning-only phase does not read the keyring.
        engine
            .state
            .lock()
            .unwrap()
            .replace(Phase::SignedOut, Cleanup::RemoteUnconfirmed)
            .unwrap();
    }
    let assert_flags = |value: &serde_json::Value| {
        assert_eq!(value["localResetAvailable"], false);
        assert_eq!(value["remoteRevocationUnconfirmed"], true);
    };
    let mut read_as = |principal: &auth::Session| {
        serde_json::to_value(Status {
            team_statuses: saved_team_statuses(
                &store,
                Some(principal),
                &BTreeMap::new(),
                (&mut anchor, "synthetic"),
                None,
            )
            .unwrap(),
            ..Status::default()
        })
        .unwrap()
    };
    for principal in [
        auth::Session {
            iss: "https://other-issuer.example".into(),
            ..personal()
        },
        auth::Session {
            resource: "https://other-api.example".into(),
            ..personal()
        },
        auth::Session {
            user: auth::User {
                id: "other-account".into(),
                ..personal().user
            },
            ..personal()
        },
    ] {
        assert!(read_as(&principal).get("teamStatuses").is_none());
    }
    let before = read_as(&personal());
    let index = |id| {
        before["teamStatuses"]
            .as_array()
            .unwrap()
            .iter()
            .position(|row| row["destinationId"] == id)
            .unwrap()
    };
    let team = index(TEAM);
    let lost = index(LOST);
    assert_flags(&before["teamStatuses"][team]);
    assert_flags(&before["teamStatuses"][lost]);
    let lost_only = &before["teamStatuses"][lost];
    assert_eq!(lost_only["accountId"], ACCOUNT);
    assert_eq!(lost_only["status"], "reauth_required");
    assert!(lost_only["consentVersion"].is_null());
    assert!(lost_only["policyRevision"].is_null());
    assert_eq!(lost_only["projects"], serde_json::json!([]));
    assert!(lost_only.get("savedFolders").is_none());
}

#[test]
fn cloud_disconnected_team_off_uses_exact_saved_path_and_current_revision() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let saved = policy(&store, ACCOUNT, TEAM, RESOURCE, ISSUER);
    let change = off(&saved, FIRST);
    let current = Cell::new(true);
    assert!(disable_team_folder(&store, &saved, &change, &|| current.replace(false)).is_err());
    let load = || {
        store
            .cloud_sync_policy_for_origins(ACCOUNT, TEAM, RESOURCE, ISSUER)
            .unwrap()
            .unwrap()
    };
    assert_eq!(
        load(),
        saved,
        "the transaction rolls back after a generation change"
    );
    let mut wrong_path = off(&saved, FIRST);
    wrong_path.folder_paths = vec![SECOND.into()];
    assert!(disable_team_folder(&store, &saved, &wrong_path, &|| true).is_err());
    let mut enable = off(&saved, FIRST);
    enable.selected = true;
    assert!(disable_team_folder(&store, &saved, &enable, &|| true).is_err());

    let updated = disable_team_folder(&store, &saved, &change, &|| true).unwrap();
    assert_ne!(updated.revision, saved.revision);
    assert_eq!(load(), updated);
    assert_eq!(
        updated
            .projects
            .iter()
            .map(|item| item.project_path.as_str())
            .collect::<Vec<_>>(),
        vec![SECOND]
    );
    assert!(disable_team_folder(&store, &saved, &change, &|| true).is_err());
    assert!(disable_team_folder(&store, &updated, &off(&saved, SECOND), &|| true).is_err());
    let stale = Status {
        status: State::Ready,
        account_id: Some(ACCOUNT.into()),
        destination_id: Some(TEAM.into()),
        policy_revision: Some(saved.revision),
        projects: saved.projects,
        storage_known: true,
        ..Status::default()
    };
    let disconnected = team_policy_status(&updated, Some(stale), None);
    assert_eq!(disconnected.status, State::ReauthRequired);
    assert!(!disconnected.storage_known);
    assert_eq!(disconnected.saved_folders.unwrap()[0].path, SECOND);
}
