//! Check saved folder rules before a move writes to the destination workspace.

use super::{MoveFrom, auth};
use crate::store::{Store, cloud_capture::sync::ProjectSelection};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// A move that creates consent cannot use a target policy saved in another window.
/// Hold the Cloud blocking permit and engine lock through the target write.
pub(super) fn verify_project_move_to_new_policy(
    store: &Store,
    session: &auth::Session,
    target_id: &str,
    move_from: &MoveFrom,
    projects: &[ProjectSelection],
    repository_anchors: &BTreeMap<String, String>,
) -> Result<(), String> {
    if store
        .cloud_sync_policy_for_origins(&session.user.id, target_id, &session.resource, &session.iss)
        .map_err(|_| "Could not read saved Cloud folders.")?
        .is_some()
    {
        return Err(
            "Cloud target folder choices changed. Reload folders before moving them.".into(),
        );
    }
    verify_project_move(
        store,
        session,
        target_id,
        move_from,
        projects,
        repository_anchors,
    )
}

/// Check saved routes just before the target write. The caller must hold the
/// Cloud blocking permit and engine lock through the write, as all policy writers do.
pub(super) fn verify_project_move<'a>(
    store: &Store,
    session: &auth::Session,
    target_id: &str,
    move_from: &MoveFrom,
    projects: impl IntoIterator<Item = &'a ProjectSelection>,
    repository_anchors: &BTreeMap<String, String>,
) -> Result<(), String> {
    const RELOAD: &str = "Cloud folder choices changed. Reload folders before moving them.";
    if session.workspace.kind != auth::WorkspaceKind::Personal
        || move_from.destination_id == target_id
    {
        return Err(RELOAD.into());
    }
    let personal = store
        .cloud_sync_policy_for_origins(
            &session.user.id,
            &session.workspace.id,
            &session.resource,
            &session.iss,
        )
        .map_err(|_| "Could not read saved Cloud folders.")?;
    let teams = store
        .saved_team_sync_policies(
            &session.user.id,
            &session.workspace.id,
            &session.resource,
            &session.iss,
        )
        .map_err(|_| "Could not read saved Cloud folders.")?;
    let other_policies = personal
        .iter()
        .filter(|policy| !policy.is_team())
        .chain(&teams)
        .filter(|policy| policy.destination_id != target_id);
    let projects: Vec<_> = projects.into_iter().collect();
    let paths: BTreeSet<_> = projects
        .iter()
        .map(|project| Path::new(&project.project_path))
        .collect();
    let repository_sources: BTreeSet<_> = projects
        .iter()
        .filter(|project| repository_anchors.contains_key(&project.project_path))
        .map(|project| (project.source_kind, &project.source_path))
        .collect();
    let mut found_source = false;
    let origins: BTreeSet<_> = repository_anchors.values().collect();
    for policy in other_policies {
        if policy.destination_id == move_from.destination_id {
            if policy.revision != move_from.policy_revision {
                return Err(RELOAD.into());
            }
            found_source = true;
        }
        if policy.projects.iter().any(|saved| {
            // An exact tree on a shared source can contain a matching checkout.
            let path = Path::new(&saved.project_path);
            repository_sources.contains(&(saved.source_kind, &saved.source_path))
                || paths
                    .iter()
                    .any(|added| path.starts_with(added) || added.starts_with(path))
        }) {
            return Err(RELOAD.into());
        }
        if policy
            .repository_rules
            .iter()
            .any(|rule| origins.contains(&rule.origin))
        {
            return Err(RELOAD.into());
        }
    }
    if !found_source {
        return Err(RELOAD.into());
    }
    Ok(())
}
