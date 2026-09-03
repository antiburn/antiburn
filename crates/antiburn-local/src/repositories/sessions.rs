//! Resolving repositories from working directories the caller has already
//! collected, plus the shared concurrency-bound helper.
//!
//! This is the supplementary path to [`scan`](super::scan): it turns "where did
//! the user actually work" into repository roots, which finds clones outside the
//! scanned directories.
//!
//! [`resolve_granted_repos`] resolves paths the user has just granted
//! consent-protected access to: each is resolved UP to its enclosing
//! repository, or scanned DOWN when it is a parent directory rather than a
//! repository itself. Callers with their own source of recent session working
//! directories (the desktop app resolves these from its session store; see
//! `apps/desktop/src-tauri/src/repositories.rs`) use it directly rather than
//! going through a bounded-concurrency pipeline here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tokio::sync::Semaphore;

use crate::platform::environment::DiscoveryEnvironment;
use crate::platform::git;

use super::access::verify_dir_access;
use super::consent::ConsentGrants;
use super::identity::{parse_repo_name_from_url, repo_root_identity};
use super::model::{LocalRepoStatus, LocatedRepo, RepoAccessStatus};
use super::scan::{SCAN_DEPTH, scan_dir_for_repos};

const CONCURRENCY_FALLBACK: usize = 4;
const CONCURRENCY_MIN: usize = 2;
const CONCURRENCY_MAX: usize = 8;

/// Resolve how many repository probes may run at once.
///
/// `configured` is an application-supplied override (typically an environment
/// variable), which wins when it parses to a positive number. Otherwise the
/// host's available parallelism is clamped into a range that keeps a laptop
/// responsive without starving a workstation, falling back to a fixed default
/// when parallelism is unknown.
pub fn concurrency_from(configured: Option<&str>, available_parallelism: Option<usize>) -> usize {
    configured
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.min(Semaphore::MAX_PERMITS))
        .unwrap_or_else(|| {
            available_parallelism
                .map(|value| value.clamp(CONCURRENCY_MIN, CONCURRENCY_MAX))
                .unwrap_or(CONCURRENCY_FALLBACK)
        })
}

/// Resolve paths the user has just granted access to into repositories.
///
/// Each path is first resolved UP to its enclosing repository (a protected
/// *session working directory*); a path that is not itself a repository — a
/// protected *parent scan root* the user just granted — is scanned DOWN for the
/// repositories inside it.
pub async fn resolve_granted_repos(
    cwds: Vec<String>,
    owner: &str,
    consent: &dyn ConsentGrants,
) -> Vec<LocalRepoStatus> {
    let owner_lower = owner.to_ascii_lowercase();
    let mut results = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // Canonical root → count of supplied working directories resolving to it
    // (approximate session count; the next full run reconciles the real number).
    let mut root_cwd_counts: HashMap<String, u32> = HashMap::new();
    // Granted paths that are not a repository themselves (protected parent scan
    // roots); scanned downward for child repositories below.
    let mut scan_down_dirs: Vec<PathBuf> = Vec::new();

    for cwd in &cwds {
        let granted = consent.granted_dirs();
        let repo_root = match git::resolve_repo_root_with_fallbacks(Path::new(cwd), &granted).await
        {
            Ok(resolution) => resolution.repo_root,
            Err(_) => {
                scan_down_dirs.push(PathBuf::from(cwd));
                continue;
            }
        };

        let canonical_root = git::canonical_main_repo_root(&repo_root).await;

        let url = match git::preferred_remote_url_at(&canonical_root).await {
            Ok(Some(url)) => url,
            _ => continue,
        };

        let repo_owner = git::parse_owner_from_url(&url).unwrap_or_default();
        if repo_owner.to_ascii_lowercase() != owner_lower {
            continue;
        }

        let repo_name = parse_repo_name_from_url(&url).unwrap_or_else(|| {
            canonical_root
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default()
        });

        let root_key = repo_root_identity(&canonical_root);
        *root_cwd_counts.entry(root_key.clone()).or_insert(0) += 1;

        // Dedup by canonical root so worktrees collapse but different clones of
        // the same repository are reported separately.
        if seen.contains(&root_key) {
            continue;
        }
        seen.insert(root_key);
        let full_name = format!("{repo_owner}/{repo_name}");
        let root_str = canonical_root.to_string_lossy().to_string();
        let worktree_count = git::worktree_count_at(&canonical_root).await;

        // Resolved without a known-repository list. Opt-out: enabled by default.
        if verify_dir_access(consent, &canonical_root).await {
            ::tracing::info!(
                event = "deferred_repo_accessible",
                repo = repo_name.as_str(),
                root = root_str.as_str(),
            );
            results.push(LocalRepoStatus {
                environment: DiscoveryEnvironment::from_mounted_path(&canonical_root),
                repo_name,
                full_name,
                status: RepoAccessStatus::Accessible,
                repo_root: Some(root_str),
                suspected_path: None,
                worktree_count,
                session_count: 0,
                enabled: true,
            });
        } else {
            ::tracing::warn!(
                event = "deferred_repo_still_denied",
                repo = repo_name.as_str(),
                root = root_str.as_str(),
            );
            results.push(LocalRepoStatus {
                environment: DiscoveryEnvironment::from_mounted_path(&canonical_root),
                repo_name,
                full_name,
                status: RepoAccessStatus::PermissionDenied,
                repo_root: None,
                suspected_path: Some(root_str),
                worktree_count,
                session_count: 0,
                enabled: true,
            });
        }
    }

    // Backfill the approximate session count onto each session-resolved
    // repository (>= 1, since each came from a session working directory). Run
    // before the scanned-down repositories are appended so their real (often
    // zero) counts are not clobbered to 1.
    for status in &mut results {
        let key = status
            .repo_root
            .clone()
            .or_else(|| status.suspected_path.clone());
        if let Some(key) = key {
            status.session_count = root_cwd_counts
                .get(&repo_root_identity(Path::new(&key)))
                .copied()
                .unwrap_or(1);
        }
    }

    // Scan granted parent directories downward for child repositories — a grant
    // on a protected common-directory scan root resolves to no repository
    // itself. `scan_dir_for_repos` already filters by owner and dedups by
    // canonical root; `seen` drops anything a working directory already
    // surfaced above.
    if !scan_down_dirs.is_empty() {
        let mut by_root: HashMap<String, LocatedRepo> = HashMap::new();
        for dir in &scan_down_dirs {
            scan_dir_for_repos(dir, &owner_lower, SCAN_DEPTH, consent, &mut by_root).await;
        }
        for located in by_root.into_values() {
            // The identity key deduplicates; it is not a path. On Windows it
            // folds case and separators, so the reported root/suspected path
            // below carries the canonical path instead — matching the
            // session-resolved arm above and `LocalRepoStatus`'s contract.
            let root_key = repo_root_identity(&located.repo_root);
            if !seen.insert(root_key) {
                continue;
            }
            let root_str = located.repo_root.to_string_lossy().to_string();
            let repo_name = parse_repo_name_from_url(&located.remote_url).unwrap_or_else(|| {
                located
                    .repo_root
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default()
            });
            let repo_owner = git::parse_owner_from_url(&located.remote_url).unwrap_or_default();
            let full_name = format!("{repo_owner}/{repo_name}");
            if located.dir_accessible {
                results.push(LocalRepoStatus {
                    environment: DiscoveryEnvironment::from_mounted_path(&located.repo_root),
                    repo_name,
                    full_name,
                    status: RepoAccessStatus::Accessible,
                    repo_root: Some(root_str),
                    suspected_path: None,
                    worktree_count: located.worktree_count,
                    session_count: located.session_count,
                    // Scanned without a known-repository list; opt-out → enabled
                    // by default, same as above.
                    enabled: true,
                });
            } else {
                results.push(LocalRepoStatus {
                    environment: DiscoveryEnvironment::from_mounted_path(&located.repo_root),
                    repo_name,
                    full_name,
                    status: RepoAccessStatus::PermissionDenied,
                    repo_root: None,
                    suspected_path: Some(root_str),
                    worktree_count: located.worktree_count,
                    session_count: located.session_count,
                    enabled: true,
                });
            }
        }
    }

    results
}
