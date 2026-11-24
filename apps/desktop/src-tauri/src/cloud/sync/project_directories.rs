//! Validate folder paths separately from the provider roots that bind them.

use crate::store::cloud_capture::sync::{ProjectSelection, check_project_binding_limits};
use std::{collections::BTreeSet, path::PathBuf};

/// Deduplicate original path strings so capture can still reject path aliases.
pub(super) fn exact_project_directories<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<PathBuf> {
    paths
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(Into::into)
        .collect()
}

/// Validate each distinct directory without changing the caller's exact bindings.
pub(super) fn validate_project_directories<'a>(
    projects: impl ExactSizeIterator<Item = &'a ProjectSelection> + Clone,
) -> Result<(), String> {
    check_project_binding_limits(projects.clone())
        .map_err(|_| "Reload projects and choose a supported Cloud project.")?;
    antiburn_cloud_capture::ProjectSelection::new(exact_project_directories(
        projects.map(|project| project.project_path.as_str()),
    ))
    .map(|_| ())
    .map_err(|_| "Choose existing project directories without path aliases.".into())
}
