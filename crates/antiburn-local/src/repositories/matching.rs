//! Turning located clones into a discovery result.
//!
//! Everything here is pure: it takes repositories already located on disk (see
//! [`LocatedRepo`]) and, optionally, the repositories the caller already knows
//! about, and decides what to report. No filesystem, no git, no I/O.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::platform::environment::DiscoveryEnvironment;
use crate::platform::git::parse_owner_from_url;

use super::identity::{normalize_remote_url, parse_repo_name_from_url, repo_root_identity};
use super::model::{
    DeferredProtectedPath, DiscoveryMode, LocalRepoStatus, LocatedRepo, RepoAccessStatus,
    RepoDiscoveryResult, RepositoryDescriptor,
};

/// Match the caller's known-repository list against repositories located on
/// disk. Each known repository becomes Accessible / PermissionDenied when found
/// locally, or NotCloned otherwise. Repositories are located by git remote, so a
/// clone's folder name is irrelevant.
///
/// Local clones that belong to `owner` (parsed from their remote) but match no
/// known repository are also surfaced, under their local name — so a repository
/// renamed upstream (whose stale local remote will not URL-match the new name)
/// still appears instead of being silently dropped.
pub fn match_known_repos(
    known: Vec<RepositoryDescriptor>,
    located: &[LocatedRepo],
    owner: &str,
) -> RepoDiscoveryResult {
    let mut results = Vec::with_capacity(known.len());
    // Canonical roots of clones matched to a known repository, so the
    // unmatched-clone pass below does not re-emit a clone already surfaced.
    let mut matched_roots: HashSet<PathBuf> = HashSet::new();

    for descriptor in &known {
        let status = match find_matching_clone(descriptor, located) {
            Some(repo) => {
                matched_roots.insert(repo.repo_root.clone());
                let root_str = repo.repo_root.to_string_lossy().to_string();
                if repo.dir_accessible {
                    LocalRepoStatus {
                        environment: DiscoveryEnvironment::from_mounted_path(&repo.repo_root),
                        repo_name: descriptor.name.clone(),
                        full_name: descriptor.full_name.clone(),
                        status: RepoAccessStatus::Accessible,
                        repo_root: Some(root_str),
                        suspected_path: None,
                        worktree_count: repo.worktree_count,
                        session_count: repo.session_count,
                        // `None` (the caller expressed no opinion) maps to
                        // enabled, so a caller without selection data does not
                        // disable every repository.
                        enabled: descriptor.is_enabled(),
                    }
                } else {
                    LocalRepoStatus {
                        environment: DiscoveryEnvironment::from_mounted_path(&repo.repo_root),
                        repo_name: descriptor.name.clone(),
                        full_name: descriptor.full_name.clone(),
                        status: RepoAccessStatus::PermissionDenied,
                        repo_root: None,
                        suspected_path: Some(root_str),
                        worktree_count: repo.worktree_count,
                        session_count: repo.session_count,
                        enabled: descriptor.is_enabled(),
                    }
                }
            }
            None => LocalRepoStatus {
                environment: DiscoveryEnvironment::default(),
                repo_name: descriptor.name.clone(),
                full_name: descriptor.full_name.clone(),
                status: RepoAccessStatus::NotCloned,
                repo_root: None,
                suspected_path: None,
                worktree_count: 0,
                session_count: 0,
                // Not on disk, so it gates nothing. `None` maps to enabled so
                // it is not presented as excluded by the caller.
                enabled: descriptor.is_enabled(),
            },
        };
        results.push(status);
    }

    // Surface local clones that belong to the owner but matched no known
    // repository. The common cause is an upstream rename: the list carries the
    // new name while the clone's git remote still points at the old one, so URL
    // matching misses it. Without this the clone vanishes from the result yet
    // still exists on disk under its old identity. Mirror
    // [`located_repos_for_owner`]'s shape so the row lands in the located set,
    // not "not cloned".
    let owner_lower = owner.to_ascii_lowercase();
    for located_repo in located {
        if matched_roots.contains(&located_repo.repo_root) {
            continue;
        }
        let Some(repo_owner) = parse_owner_from_url(&located_repo.remote_url) else {
            continue;
        };
        if repo_owner.to_ascii_lowercase() != owner_lower {
            continue;
        }
        // Require a parseable repository name: an owner-only/odd remote (for
        // example `git@github.com:owner/`) yields an empty name, which would
        // surface a blank-titled `owner/` entry and collide with any other such
        // clone on the dedup key. Skipping it is safe — the session and scan
        // paths still cover the repository if it has a usable remote.
        let Some(repo_name) = parse_repo_name_from_url(&located_repo.remote_url) else {
            continue;
        };
        let full_name = format!("{repo_owner}/{repo_name}");
        let root_str = located_repo.repo_root.to_string_lossy().to_string();
        // Matched no known repository (for example a personal fork). Opt-out:
        // enabled by default, so a local clone is not silently excluded.
        let status = if located_repo.dir_accessible {
            LocalRepoStatus {
                environment: DiscoveryEnvironment::from_mounted_path(&located_repo.repo_root),
                repo_name,
                full_name,
                status: RepoAccessStatus::Accessible,
                repo_root: Some(root_str),
                suspected_path: None,
                worktree_count: located_repo.worktree_count,
                session_count: located_repo.session_count,
                enabled: true,
            }
        } else {
            LocalRepoStatus {
                environment: DiscoveryEnvironment::from_mounted_path(&located_repo.repo_root),
                repo_name,
                full_name,
                status: RepoAccessStatus::PermissionDenied,
                repo_root: None,
                suspected_path: Some(root_str),
                worktree_count: located_repo.worktree_count,
                session_count: located_repo.session_count,
                enabled: true,
            }
        };
        results.push(status);
    }

    results.sort_by(|a, b| a.repo_name.cmp(&b.repo_name));
    RepoDiscoveryResult {
        discovery_mode: DiscoveryMode::Full,
        repos: results,
        deferred_protected: Vec::new(),
    }
}

/// Report the located clones that belong to `owner`, with no known-repository
/// list to match against.
pub fn located_repos_for_owner(owner: &str, located: &[LocatedRepo]) -> RepoDiscoveryResult {
    let owner_lower = owner.to_ascii_lowercase();
    let mut repos: Vec<LocalRepoStatus> = located
        .iter()
        .filter_map(|located_repo| {
            let repo_owner = parse_owner_from_url(&located_repo.remote_url)?;
            if repo_owner.to_ascii_lowercase() != owner_lower {
                return None;
            }
            let repo_name = parse_repo_name_from_url(&located_repo.remote_url).unwrap_or_default();
            let full_name = format!("{repo_owner}/{repo_name}");
            let root_str = located_repo.repo_root.to_string_lossy().to_string();
            // No known-repository list here. Opt-out: default to enabled so a
            // caller that could not supply one does not disable everything.
            if located_repo.dir_accessible {
                Some(LocalRepoStatus {
                    environment: DiscoveryEnvironment::from_mounted_path(&located_repo.repo_root),
                    repo_name,
                    full_name,
                    status: RepoAccessStatus::Accessible,
                    repo_root: Some(root_str),
                    suspected_path: None,
                    worktree_count: located_repo.worktree_count,
                    session_count: located_repo.session_count,
                    enabled: true,
                })
            } else {
                Some(LocalRepoStatus {
                    environment: DiscoveryEnvironment::from_mounted_path(&located_repo.repo_root),
                    repo_name,
                    full_name,
                    status: RepoAccessStatus::PermissionDenied,
                    repo_root: None,
                    suspected_path: Some(root_str),
                    worktree_count: located_repo.worktree_count,
                    session_count: located_repo.session_count,
                    enabled: true,
                })
            }
        })
        .collect();

    repos.sort_by(|a, b| a.repo_name.cmp(&b.repo_name));

    RepoDiscoveryResult {
        discovery_mode: DiscoveryMode::SessionsOnly,
        repos,
        deferred_protected: Vec::new(),
    }
}

/// Find a located clone whose remote URL matches the known repository.
fn find_matching_clone<'a>(
    descriptor: &RepositoryDescriptor,
    located: &'a [LocatedRepo],
) -> Option<&'a LocatedRepo> {
    let clone_norm = normalize_remote_url(&descriptor.clone_url);
    let ssh_norm = normalize_remote_url(&descriptor.ssh_url);
    located
        .iter()
        .find(|repo| repo.remote_url == clone_norm || repo.remote_url == ssh_norm)
}

/// Merge session-located and scan-located repositories, deduped by canonical
/// repository root. Prefers an accessible location and keeps the larger session
/// count (scan-only repositories contribute `0`).
pub fn merge_located_repos(
    session_repos: Vec<LocatedRepo>,
    scan_repos: Vec<LocatedRepo>,
) -> Vec<LocatedRepo> {
    let mut by_root: HashMap<String, LocatedRepo> = HashMap::new();
    for repo in session_repos.into_iter().chain(scan_repos) {
        let key = repo_root_identity(&repo.repo_root);
        match by_root.entry(key) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(repo);
            }
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let existing = e.get_mut();
                existing.session_count = existing.session_count.max(repo.session_count);
                existing.worktree_count = existing.worktree_count.max(repo.worktree_count);
                if repo.dir_accessible {
                    existing.dir_accessible = true;
                }
            }
        }
    }
    // Deterministic order so matching resolves the same clone on every run;
    // prefer an accessible clone so a repository with one accessible and one
    // blocked clone surfaces as accessible rather than flipping by hash order.
    let mut located: Vec<LocatedRepo> = by_root.into_values().collect();
    located.sort_by(|a, b| {
        b.dir_accessible
            .cmp(&a.dir_accessible)
            .then_with(|| a.repo_root.cmp(&b.repo_root))
    });
    located
}

/// Drop duplicate deferred entries sharing a `cwd` (for example a scan root and
/// a session working directory under the same protected directory), preserving
/// first-seen order.
pub fn dedup_deferred_by_cwd(deferred: &mut Vec<DeferredProtectedPath>) {
    let mut seen: HashSet<String> = HashSet::new();
    deferred.retain(|entry| seen.insert(entry.cwd.clone()));
}

/// True if a deferred (consent-protected) path correlates to a known repository
/// by its folder name. Used to drop protected projects that are not known
/// repositories, since their git remote cannot be read to confirm.
/// `known_repo_names` must be lowercased; an empty set correlates nothing.
pub(super) fn deferred_correlates_to_known(cwd: &str, known_repo_names: &HashSet<String>) -> bool {
    Path::new(cwd)
        .file_name()
        .map(|name| known_repo_names.contains(name.to_string_lossy().to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Filter deferred (consent-protected) paths down to those that correlate to a
/// known repository by folder name.
///
/// When `has_known_list` is false — no authoritative repository list — the list
/// is returned unfiltered so legitimate repositories in protected folders are
/// not hidden; a post-consent run still filters once the user grants access.
/// When a list IS available, uncorrelated paths are dropped so discovery never
/// reports unrelated local projects — including the empty-list case, which
/// correctly drops everything rather than surfacing foreign projects.
pub fn filter_deferred_by_known(
    deferred: Vec<DeferredProtectedPath>,
    known_repo_names: &HashSet<String>,
    has_known_list: bool,
) -> Vec<DeferredProtectedPath> {
    if !has_known_list {
        return deferred;
    }
    deferred
        .into_iter()
        .filter(|entry| deferred_correlates_to_known(&entry.cwd, known_repo_names))
        .collect()
}
