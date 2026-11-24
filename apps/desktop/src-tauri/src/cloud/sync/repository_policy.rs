//! Checkout proof for repository rules and migration.

use super::super::{
    projects::repository_group_id,
    repository_checkout::{CheckoutOrigin, CheckoutOriginCache},
};
use super::{
    Policy, ProjectChange, ProjectSelection, SavedFolder, State, Status, Store, auth,
    team_has_proof,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::{Duration, Instant},
};

/// Resolve only exact legacy checkout roots. Plain paths, nested directories and
/// unverified origins keep their old folder scope.
async fn prove_legacy_checkouts(
    projects: &[ProjectSelection],
    current: &dyn Fn() -> bool,
) -> BTreeMap<String, String> {
    let paths: BTreeSet<_> = projects
        .iter()
        .map(|project| &project.project_path)
        .collect();
    let mut verified = BTreeMap::new();
    let mut cache = CheckoutOriginCache::default();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    for path in paths {
        if !current() {
            return BTreeMap::new();
        }
        let Ok(checked) = tokio::time::timeout_at(deadline, cache.lookup(Path::new(path))).await
        else {
            break;
        };
        if let CheckoutOrigin::Verified {
            checkout,
            origin: Some(origin),
        } = checked
            && checkout == Path::new(path)
        {
            verified.insert(path.clone(), origin.as_label().to_owned());
        }
    }
    if current() { verified } else { BTreeMap::new() }
}

/// Name one saved exact folder without reading its checkout.
pub(super) fn saved_team_folder_id(path: &str) -> String {
    format!(
        "saved-{}",
        super::super::projects::group_id("saved-team-folder", path)
    )
}

/// Convert currently verifiable legacy checkout roots without enabling uploads.
/// Only call from a bounded worker under current account and workspace authority.
fn migrate(
    store: &Store,
    policy: &Policy,
    current: &dyn Fn() -> bool,
) -> anyhow::Result<Option<Policy>> {
    // Old exclusions save only opaque IDs. They do not prove which clone was off.
    if policy.project_choices.values().any(|selected| !selected) {
        return Ok(None);
    }
    let anchors = tauri::async_runtime::block_on(prove_legacy_checkouts(&policy.projects, current));
    if anchors.is_empty() || !current() {
        return Ok(None);
    }
    store
        .migrate_cloud_repository_anchors(policy, &anchors, current)
        .map(Some)
}

/// Retry deferred migrations when old jobs finish. Do not probe on every turn.
pub(super) fn try_migration(
    store: &Store,
    policy: Policy,
    attempt: &mut Option<(String, Instant)>,
    current: &dyn Fn() -> bool,
) -> Policy {
    if policy.paused
        || policy.projects.is_empty()
        || !current()
        || attempt
            .as_ref()
            .is_some_and(|(revision, next)| revision == &policy.revision && Instant::now() < *next)
    {
        return policy;
    }
    match migrate(store, &policy, current) {
        Ok(Some(saved)) => saved,
        Ok(None) => {
            *attempt = Some((
                policy.revision.clone(),
                Instant::now() + Duration::from_secs(3600),
            ));
            policy
        }
        Err(_) => {
            *attempt = Some((
                policy.revision.clone(),
                Instant::now() + Duration::from_secs(60),
            ));
            policy
        }
    }
}

/// Check the exact reviewed source rules before revoking a repository group.
pub(super) fn revoke_repository(
    store: &Store,
    policy: &Policy,
    change: &ProjectChange,
    current: &dyn Fn() -> bool,
) -> anyhow::Result<Policy> {
    let rule = policy
        .repository_rule_for_group(&change.group_id)
        .ok_or_else(|| anyhow::anyhow!("Cloud repository selection changed"))?;
    if let Some(expected) = &change.expected_removals {
        let reviewed: BTreeSet<_> = policy
            .roots
            .iter()
            .filter(|root| rule.scopes.contains(&root.source_scope_id))
            .map(|root| ProjectSelection {
                source_kind: root.source_kind,
                source_path: root.path.clone(),
                project_path: rule.anchor_path.clone(),
            })
            .collect();
        anyhow::ensure!(
            reviewed.iter().collect::<BTreeSet<_>>() == expected.iter().collect(),
            "Cloud repository sources changed"
        );
    }
    store.revoke_cloud_repository(policy, &change.group_id, &rule.origin, current)
}

/// Check whether the saved policy grants any folder or repository access.
pub(super) fn has_folders(policy: &Policy) -> bool {
    !policy.projects.is_empty() || !policy.repository_rules.is_empty()
}

/// Report saved Team rules even when membership still needs confirmation.
pub(super) fn team_policy_status(
    policy: &Policy,
    live: Option<Status>,
    eligible: Option<&[auth::EligibleWorkspace]>,
) -> Status {
    let generation = eligible
        .into_iter()
        .flatten()
        .find(|entry| {
            entry.workspace.id == policy.destination_id
                && entry.workspace.kind == auth::WorkspaceKind::Team
        })
        .and_then(|entry| entry.membership_generation.as_deref());
    let folder_confirmation_required = has_folders(policy)
        && generation.is_some_and(|generation| policy.needs_team_membership(generation));
    let mut status = live
        .filter(|value| {
            !folder_confirmation_required
                && value.account_id.as_deref() == Some(&policy.account_id)
                && value.destination_id.as_deref() == Some(&policy.destination_id)
                && value.policy_revision.as_deref() == Some(&policy.revision)
        })
        .unwrap_or(Status {
            status: State::ReauthRequired,
            ..Status::default()
        });
    status.account_id = Some(policy.account_id.clone());
    status.destination_id = Some(policy.destination_id.clone());
    status.consent_version = Some(policy.consent_version.clone());
    status.policy_revision = Some(policy.revision.clone());
    status.workspace_name = policy.workspace_name.clone();
    status.team_statuses.clear();
    status.folder_confirmation_required = folder_confirmation_required;
    status.saved_folders = Some(saved_folders(policy));
    status.paused = policy.paused;
    status.source_confirmation_required = policy.paused;
    apply_folder_status(&mut status, policy);
    status.project_choices = policy.project_choices.clone();
    if policy.paused || !eligible.is_some_and(|entries| team_has_proof(policy, entries)) {
        status.automatic_recovery_count = None;
    }
    status
}

/// Report saved rules even if the paths are not present in current discovery.
pub(super) fn saved_folders(policy: &Policy) -> Vec<SavedFolder> {
    policy
        .projects
        .iter()
        .map(|project| &project.project_path)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|path| SavedFolder {
            group_id: saved_team_folder_id(path),
            path: path.clone(),
        })
        .chain(policy.repository_rules.iter().map(|rule| SavedFolder {
            group_id: repository_group_id(&rule.origin),
            path: rule.anchor_path.clone(),
        }))
        .collect()
}

/// Give the renderer stable rule IDs without sending origin labels.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RepositoryFolder {
    group_id: String,
    projects: Vec<ProjectSelection>,
}

/// Keep exact and repository grants distinct when their display paths coincide.
pub(super) fn apply_folder_status(status: &mut Status, policy: &Policy) {
    status.projects = policy.projects_for_display();
    status.exact_projects = (!policy.repository_rules.is_empty()).then(|| policy.projects.clone());
    status.repository_folders = policy
        .repository_rules
        .iter()
        .map(|rule| RepositoryFolder {
            group_id: repository_group_id(&rule.origin),
            projects: policy
                .roots
                .iter()
                .filter(|root| rule.scopes.contains(&root.source_scope_id))
                .map(|root| ProjectSelection {
                    source_kind: root.source_kind,
                    source_path: root.path.clone(),
                    project_path: rule.anchor_path.clone(),
                })
                .collect(),
        })
        .collect();
    status.repository_group_ids = status
        .repository_folders
        .iter()
        .map(|folder| folder.group_id.clone())
        .collect();
}
