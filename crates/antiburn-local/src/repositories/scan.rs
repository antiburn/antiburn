// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded directory traversal for repositories on disk.
//!
//! This is the forward path: walk the places developers keep clones and match
//! what is there by git remote, so a repository is found even when its folder
//! name differs from its name and even when it has no agent sessions at all.
//!
//! Every walk is bounded. Depth is capped at [`SCAN_DEPTH`], hidden and known
//! heavy directories are never descended into, a repository directory is a leaf,
//! and the number of caller-derived roots per run is capped at
//! [`MAX_PARENT_SCAN_ROOTS`]. Consent-protected roots are never read unless the
//! user already granted them.

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::paths::protected::protected_dir_name;
use crate::paths::scan_roots;
use crate::platform::git;

use super::access::{path_exists_without_prompting, verify_dir_access};
use super::consent::ConsentGrants;
use super::identity::{normalize_for_prefix, normalize_remote_url, repo_root_identity};
use super::model::{DeferredProtectedPath, LocatedRepo};
use super::platform::{self, PlatformDiscovery as _};

/// Walk depth for every scan root (common code directories, session-parent
/// roots, and user-declared roots): covers `<dir>/<repo>` and
/// `<dir>/<owner>/<repo>`.
pub const SCAN_DEPTH: usize = 2;

/// Cap on parent scan roots derived from one discovery run, so a machine with
/// many non-repository session working directories cannot blow up scan time.
/// Excess candidates are dropped with a log rather than silently; parents
/// confirmed on earlier runs are still re-scanned from the persisted store.
pub const MAX_PARENT_SCAN_ROOTS: usize = 64;

/// Directory names never worth descending into when scanning for repositories.
const EXCLUDED_SCAN_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "build",
    "dist",
    "vendor",
    "Library",
    "venv",
    "__pycache__",
];

/// True if `path`'s final segment is a hidden directory or a known heavy
/// directory that should never be descended into. A repository *named* one of
/// these is still detected (the `.git` check runs first); this only prunes
/// recursion.
pub(super) fn is_excluded_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with('.') || EXCLUDED_SCAN_DIRS.contains(&name))
        .unwrap_or(false)
}

/// Scan common code directories for repositories on disk, independent of agent
/// sessions. Returns located repositories (as [`LocatedRepo`] with
/// `session_count: 0`) plus deferred entries for consent-protected scan roots
/// that could not be read.
///
/// Repositories are matched to `owner` by parsing the owner from their git
/// remote, so a clone's folder name is irrelevant. Consent-protected roots (for
/// example `~/Documents/GitHub`) are never enumerated unless already granted —
/// they are deferred so the caller can ask for consent; the next run after a
/// grant picks them up.
///
/// `extra_roots` are additional directories to scan beyond the platform's
/// common code directories. All roots are walked at [`SCAN_DEPTH`].
pub async fn scan_roots_for_repos(
    home: &Path,
    owner: &str,
    probe_protected: bool,
    extra_roots: &[PathBuf],
    consent: &dyn ConsentGrants,
) -> (Vec<LocatedRepo>, Vec<DeferredProtectedPath>) {
    let owner_lower = owner.to_ascii_lowercase();
    let plat = platform::platform();

    // Protected directory names already granted, plus — when the caller asked
    // for it — any granted outside the application for the protected common
    // directories.
    let mut granted_names = consent.granted_dirs();
    if probe_protected {
        let protected_names: HashSet<String> = plat
            .common_code_dirs()
            .iter()
            .filter_map(|dir| {
                let root = home.join(dir);
                plat.is_access_protected(&root)
                    .then(|| protected_dir_name(&root))
                    .flatten()
            })
            .collect();
        if !protected_names.is_empty() {
            granted_names.extend(consent.discover_external_grants(&protected_names).await);
        }
    }
    let mut accessible_dirs: HashSet<PathBuf> =
        granted_names.iter().map(|name| home.join(name)).collect();

    let mut by_root: HashMap<String, LocatedRepo> = HashMap::new();
    let mut deferred: Vec<DeferredProtectedPath> = Vec::new();

    // Roots to scan: the platform's common code directories plus the
    // caller-supplied extra roots, deduped by path and sorted for deterministic
    // order.
    let mut roots: Vec<PathBuf> = plat
        .common_code_dirs()
        .iter()
        .map(|dir| home.join(dir))
        .collect();
    roots.extend(extra_roots.iter().cloned());
    roots.sort();
    roots.dedup();

    for root in &roots {
        // Defer consent-protected roots that are not granted; never enumerate
        // them (that is what raises the dialog). Only defer roots that actually
        // exist — a missing common directory must not be reported as needing
        // permission. `stat()` is allowed on protected paths, so existence is
        // checkable without prompting.
        if plat.is_access_protected(root) && !accessible_dirs.iter().any(|d| root.starts_with(d)) {
            if path_exists_without_prompting(root).await
                && let Some(protected) = protected_dir_name(root)
            {
                deferred.push(DeferredProtectedPath {
                    cwd: root.to_string_lossy().to_string(),
                    protected_dir: protected,
                });
            }
            continue;
        }

        // A protected root believed to be granted: confirm the grant is still
        // live before scanning. `scan_dir_for_repos` opens this same directory
        // immediately below and discards the error kind, so a revoked or reset
        // grant would otherwise leave the root silently unreadable — no
        // deferral, and the stale record never dropped. This adds no new probe
        // class: under a recorded grant the very next statement already reads
        // this directory. A live grant is a recorded allow (silent); a revoked
        // one is a recorded deny (silent).
        if plat.is_access_protected(root) {
            let t0 = std::time::Instant::now();
            match tokio::fs::read_dir(root).await {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                    consent.record_probe(
                        &root.to_string_lossy(),
                        "denied",
                        t0.elapsed().as_millis() as u64,
                    );
                    if let Some(stale) = consent.revoke_grant_covering(root).await {
                        // Drop it from this run's snapshot too, so a second root
                        // under the same dead grant defers now rather than next
                        // run.
                        accessible_dirs.remove(&home.join(&stale));
                        ::tracing::debug!(
                            event = "repo_discovery_scan_root_grant_evicted",
                            root = root.to_string_lossy().as_ref(),
                            stale_dir = stale.as_str(),
                        );
                    }
                    if let Some(protected) = protected_dir_name(root) {
                        deferred.push(DeferredProtectedPath {
                            cwd: root.to_string_lossy().to_string(),
                            protected_dir: protected,
                        });
                    }
                    continue;
                }
                Err(_) => continue,
            }
        }

        scan_dir_for_repos(root, &owner_lower, SCAN_DEPTH, consent, &mut by_root).await;
    }

    (by_root.into_values().collect(), deferred)
}

/// Recursively look for repositories under `dir` (bounded by `depth`), keeping
/// those whose remote's owner matches `owner_lower`, keyed by canonical root.
pub(super) async fn scan_dir_for_repos(
    dir: &Path,
    owner_lower: &str,
    depth: usize,
    consent: &dyn ConsentGrants,
    by_root: &mut HashMap<String, LocatedRepo>,
) {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(_) => return,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }

        // Is this directory itself a repository?
        if tokio::fs::metadata(path.join(".git")).await.is_ok() {
            if let Ok(Some(url)) = git::preferred_remote_url_at(&path).await {
                let repo_owner = git::parse_owner_from_url(&url).unwrap_or_default();
                if repo_owner.to_ascii_lowercase() == owner_lower {
                    let canonical_root = git::canonical_main_repo_root(&path).await;
                    let root_key = repo_root_identity(&canonical_root);
                    if let std::collections::hash_map::Entry::Vacant(e) = by_root.entry(root_key) {
                        let dir_accessible = verify_dir_access(consent, &canonical_root).await;
                        let worktree_count = git::worktree_count_at(&canonical_root).await;
                        e.insert(LocatedRepo {
                            remote_url: normalize_remote_url(&url),
                            repo_root: canonical_root,
                            dir_accessible,
                            worktree_count,
                            session_count: 0,
                        });
                    }
                }
            }
            // A repository directory is a leaf; do not descend into it.
            continue;
        }

        // Not a repository — descend one more level if budget remains, skipping
        // hidden and known heavy directories (keeps deep walks cheap).
        if depth > 1 && !is_excluded_dir(&path) {
            Box::pin(scan_dir_for_repos(
                &path,
                owner_lower,
                depth - 1,
                consent,
                by_root,
            ))
            .await;
        }
    }
}

/// Parent directories of already-located repositories, used as extra scan roots
/// so a sibling with no sessions is found alongside an active repository.
///
/// Excludes the home directory itself, the filesystem root, and
/// consent-protected parents (scanning those would be too broad or need a
/// grant; the session and common-directory paths already cover protected
/// locations). Deduped.
pub fn sibling_scan_roots(located: &[LocatedRepo], home: &Path) -> Vec<PathBuf> {
    let plat = platform::platform();
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for repo in located {
        let Some(parent) = repo.repo_root.parent() else {
            continue;
        };
        if parent == home || parent.parent().is_none() || plat.is_access_protected(parent) {
            continue;
        }
        if seen.insert(parent.to_path_buf()) {
            roots.push(parent.to_path_buf());
        }
    }
    roots
}

/// Candidate scan roots derived from session working directories that did not
/// resolve to a repository — typically a *parent folder above the repositories*
/// (an editor session opened on a workspace directory). The working directory
/// itself is the parent, so it is used directly as the scan root.
///
/// Guards mirror [`sibling_scan_roots`] and are deliberately **not** limited to
/// the home tree — repositories legitimately live outside it (mounted volumes,
/// `/opt`, shared workspace directories). Skip only what is too broad or unsafe:
/// the home directory itself and any ancestor of it (`/`, `/Users`, …), and
/// consent-protected directories (which need a grant and are surfaced through
/// the deferred flow). Scan cost is bounded by [`MAX_PARENT_SCAN_ROOTS`] rather
/// than by location, and an ephemeral scratch directory simply yields no
/// repository, so it is never persisted. Deduped.
pub fn parent_scan_roots(unresolved_cwds: &[String], home: &Path) -> Vec<PathBuf> {
    let plat = platform::platform();
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for cwd in unresolved_cwds {
        let path = PathBuf::from(cwd);
        // `home.starts_with(&path)` is true when `path` is the home directory
        // itself or an ancestor of it (`/`, `/Users`, …) — all too broad to
        // scan. Locations outside the home tree are kept.
        if home.starts_with(&path) || plat.is_access_protected(&path) {
            continue;
        }
        if seen.insert(path.clone()) {
            roots.push(path);
        }
    }
    if roots.len() > MAX_PARENT_SCAN_ROOTS {
        ::tracing::debug!(
            event = "parent_scan_roots_capped",
            found = roots.len(),
            cap = MAX_PARENT_SCAN_ROOTS,
        );
        roots.truncate(MAX_PARENT_SCAN_ROOTS);
    }
    roots
}

/// Persist the parent scan roots that actually contained a repository, so a
/// parent folder keeps being scanned after its sessions age out of the lookback
/// window.
///
/// [`scan_roots::add_scan_root`] is idempotent and rejects the home directory
/// and `/` as too broad, so this is a safe best-effort write (failures are
/// logged, not propagated).
pub async fn persist_confirmed_parent_roots(
    state_dir: &Path,
    candidates: &[PathBuf],
    scan_repos: &[LocatedRepo],
) {
    for parent in candidates {
        // Scanned child roots are canonicalized (git resolves symlinks, for
        // example macOS `/var` → `/private/var`, and Windows short → long
        // names), so canonicalize the parent the same way before the prefix
        // check; the raw path is kept as a fallback for when canonicalize fails.
        // Compare as [`normalize_for_prefix`] strings rather than
        // `Path::starts_with`: on Windows `fs::canonicalize` returns a `\\?\`
        // verbatim path while git repository roots are non-verbatim, so
        // `starts_with` never matches there.
        let canonical_parent = tokio::fs::canonicalize(parent)
            .await
            .unwrap_or_else(|_| parent.clone());
        let parent_prefixes = [
            normalize_for_prefix(&canonical_parent),
            normalize_for_prefix(parent),
        ];
        let has_child = scan_repos.iter().any(|repo| {
            let child = normalize_for_prefix(&repo.repo_root);
            parent_prefixes
                .iter()
                .any(|p| !p.is_empty() && (child == *p || child.starts_with(&format!("{p}/"))))
        });
        if !has_child {
            continue;
        }
        // Persist the original path (not the canonicalized one): it is what a
        // later run re-derives from the same session, so the persisted root and
        // the re-derived candidate dedup. The store only feeds `read_dir`
        // (which follows symlinks), so anything keyed off canonical repository
        // roots is unaffected by the spelling stored here.
        if let Err(e) = scan_roots::add_scan_root(state_dir, &parent.to_string_lossy()).await {
            ::tracing::warn!(
                event = "parent_scan_root_persist_failed",
                parent = parent.to_string_lossy().as_ref(),
                error = %e,
            );
        }
    }
}
