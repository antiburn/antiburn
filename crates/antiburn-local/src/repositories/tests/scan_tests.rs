// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded traversal, user-declared roots, and the consent seam around them.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serial_test::serial;

use super::support::{EnvGuard, FakeConsent, init_repo, located};
use crate::paths::scan_roots;
use crate::repositories::access::verify_cwd_admitted_on_grant;
#[cfg(unix)]
use crate::repositories::access::verify_dir_access;
use crate::repositories::platform::{self, PlatformDiscovery as _};
use crate::repositories::scan::is_excluded_dir;
use crate::repositories::{
    NoConsentGrants, RepoAccessStatus, parent_scan_roots, persist_confirmed_parent_roots,
    resolve_granted_repos, scan_roots_for_repos, sibling_scan_roots,
};

#[cfg(unix)]
use super::support::with_unreadable_dir;

#[test]
fn excluded_dirs_prune_recursion_but_not_repo_detection() {
    assert!(is_excluded_dir(Path::new("/x/node_modules")));
    assert!(is_excluded_dir(Path::new("/x/.cache")));
    assert!(is_excluded_dir(Path::new("/x/target")));
    assert!(!is_excluded_dir(Path::new("/x/my-repo")));
    assert!(!is_excluded_dir(Path::new("/x/workspace")));
}

/// The headline case: a clone whose folder name differs from the repository
/// name is still located by its git remote, with no agent session required.
#[tokio::test]
async fn scan_finds_repo_with_mismatched_folder_name() {
    let home = tempfile::TempDir::new().unwrap();
    let code_dir = platform::platform().common_code_dirs()[0];
    let repo_dir = home.path().join(code_dir).join("totally-different-folder");
    init_repo(&repo_dir, "git@github.com:acme/widgets.git").await;

    let consent = FakeConsent::empty(home.path());
    let (repos, deferred) = scan_roots_for_repos(home.path(), "acme", false, &[], &consent).await;

    assert!(deferred.is_empty());
    assert_eq!(
        repos.len(),
        1,
        "expected the repository to be located by remote"
    );
    assert_eq!(repos[0].remote_url, "git@github.com:acme/widgets");
    assert_eq!(
        repos[0].session_count, 0,
        "scan-only repository has no sessions"
    );
    assert!(repos[0].dir_accessible);
    assert!(repos[0].repo_root.ends_with("totally-different-folder"));
}

#[tokio::test]
async fn scan_excludes_repo_of_another_owner() {
    let home = tempfile::TempDir::new().unwrap();
    let code_dir = platform::platform().common_code_dirs()[0];
    let repo_dir = home.path().join(code_dir).join("foreign");
    init_repo(&repo_dir, "git@github.com:otherowner/foreign.git").await;

    let consent = FakeConsent::empty(home.path());
    let (repos, _deferred) = scan_roots_for_repos(home.path(), "acme", false, &[], &consent).await;

    assert!(
        repos.is_empty(),
        "repositories belonging to another owner must be excluded"
    );
}

/// A repository cloned outside the common code directories, with no sessions,
/// is found when its parent is supplied as an extra scan root — and is *not*
/// found without it.
#[tokio::test]
async fn scan_finds_sibling_via_extra_root() {
    let home = tempfile::TempDir::new().unwrap();
    // `team-space` is not a common code directory.
    let parent = home.path().join("team-space");
    let repo_dir = parent.join("toolkit");
    init_repo(&repo_dir, "git@github.com:acme/toolkit.git").await;
    let consent = FakeConsent::empty(home.path());

    let (none, _) = scan_roots_for_repos(home.path(), "acme", false, &[], &consent).await;
    assert!(none.is_empty());

    let (found, _) = scan_roots_for_repos(
        home.path(),
        "acme",
        false,
        std::slice::from_ref(&parent),
        &consent,
    )
    .await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].remote_url, "git@github.com:acme/toolkit");
    assert_eq!(found[0].session_count, 0);
}

/// Once a recorded grant goes stale (revoked or reset outside the application),
/// *something* must notice. Without this the scan swallows the denial, so the
/// directory vanishes from discovery with no deferral and the dead grant is
/// cached forever.
#[cfg(all(unix, target_os = "macos"))]
#[tokio::test]
#[serial]
async fn scan_evicts_stale_grant_and_defers_dir() {
    let home = tempfile::TempDir::new().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path());

    let protected_root = home.path().join("Documents").join("GitHub");
    tokio::fs::create_dir_all(&protected_root).await.unwrap();
    let consent = FakeConsent::with_grants(home.path(), ["Documents"]);

    let deferred = with_unreadable_dir(&protected_root, || async {
        let (_repos, deferred) =
            scan_roots_for_repos(home.path(), "acme", false, &[], &consent).await;
        deferred
    })
    .await;

    assert!(
        deferred.iter().any(|d| d.protected_dir == "Documents"),
        "a denied read under a granted directory must re-defer it in the same run, got {deferred:?}"
    );
    assert!(
        !consent.grants().contains("Documents"),
        "the stale grant must be dropped so the next run defers via the normal path"
    );
    assert!(
        consent
            .probes()
            .iter()
            .any(|(_, outcome)| outcome == "PermissionDenied"),
        "the denial must be reported to the application for diagnostics"
    );

    // With the grant gone, the ungranted branch defers it without reading.
    let (_repos, deferred_again) =
        scan_roots_for_repos(home.path(), "acme", false, &[], &consent).await;
    assert!(
        deferred_again
            .iter()
            .any(|d| d.protected_dir == "Documents"),
        "expected the directory to stay deferred once the grant is dropped"
    );
}

/// Over-eviction guard: a readable root under a live grant must scan normally
/// and keep the grant.
#[cfg(target_os = "macos")]
#[tokio::test]
#[serial]
async fn scan_keeps_valid_grant_for_readable_root() {
    let home = tempfile::TempDir::new().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path());

    let repo_dir = home.path().join("Documents").join("GitHub").join("widgets");
    init_repo(&repo_dir, "git@github.com:acme/widgets.git").await;
    let consent = FakeConsent::with_grants(home.path(), ["Documents"]);

    let (repos, deferred) = scan_roots_for_repos(home.path(), "acme", false, &[], &consent).await;

    assert!(
        deferred.is_empty(),
        "a live grant must not defer: {deferred:?}"
    );
    assert!(
        repos
            .iter()
            .any(|repo| repo.remote_url.ends_with("acme/widgets")),
        "expected the repository under the granted directory to be found"
    );
    assert!(
        consent.grants().contains("Documents"),
        "a readable root must never drop its own grant"
    );
}

/// A grant made outside the application is only picked up when the caller opts
/// into probing — background runs must never probe, because a probe with no
/// recorded decision is what raises the consent dialog.
#[cfg(target_os = "macos")]
#[tokio::test]
#[serial]
async fn scan_probes_external_grants_only_when_asked() {
    let home = tempfile::TempDir::new().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path());

    let repo_dir = home.path().join("Documents").join("GitHub").join("widgets");
    init_repo(&repo_dir, "git@github.com:acme/widgets.git").await;
    let consent = FakeConsent::empty(home.path());
    consent.stage_external_grant("Documents");

    let (repos, deferred) = scan_roots_for_repos(home.path(), "acme", false, &[], &consent).await;
    assert!(repos.is_empty(), "no probe, so the root stays deferred");
    assert!(deferred.iter().any(|d| d.protected_dir == "Documents"));

    let (repos, deferred) = scan_roots_for_repos(home.path(), "acme", true, &[], &consent).await;
    assert!(
        deferred.is_empty(),
        "the external grant was picked up: {deferred:?}"
    );
    assert_eq!(repos.len(), 1);
}

/// The shared eviction path the session loop delegates to when git resolution
/// fails under a directory believed to be granted.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn verify_dir_access_evicts_stale_grant_on_denied_read() {
    let home = tempfile::TempDir::new().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path());

    let repo_dir = home.path().join("Documents").join("widgets");
    tokio::fs::create_dir_all(&repo_dir).await.unwrap();
    let consent = FakeConsent::with_grants(home.path(), ["Documents"]);

    let accessible =
        with_unreadable_dir(&repo_dir, || verify_dir_access(&consent, &repo_dir)).await;

    assert!(
        !accessible,
        "a denied read must report the directory inaccessible"
    );
    assert!(
        !consent.grants().contains("Documents"),
        "the covering grant must be dropped when a read under it is denied"
    );
}

/// Only working directories admitted on a recorded grant get re-checked.
/// Everything else is an ordinary "not a repository" and must not be probed —
/// that would be a filesystem read with no reason behind it.
#[tokio::test]
#[serial]
async fn verify_cwd_admitted_on_grant_skips_cwds_not_admitted() {
    let home = tempfile::TempDir::new().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path());

    let dir = home.path().join("Documents").join("widgets");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let consent = FakeConsent::with_grants(home.path(), ["Documents"]);

    let checked =
        verify_cwd_admitted_on_grant(&consent, &dir.to_string_lossy(), &HashSet::new()).await;

    assert!(
        !checked,
        "a path outside the admitted set must not be probed"
    );
    assert!(
        consent.grants().contains("Documents"),
        "skipping the check must leave the grant untouched"
    );
    assert!(consent.probes().is_empty(), "nothing was probed");
}

/// The admitted case: a denied read drops the covering grant, which is what
/// makes the directory defer again on the next run.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn verify_cwd_admitted_on_grant_evicts_when_denied() {
    let home = tempfile::TempDir::new().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path());

    let dir = home.path().join("Documents").join("widgets");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let consent = FakeConsent::with_grants(home.path(), ["Documents"]);

    let cwd = dir.to_string_lossy().to_string();
    let admitted: HashSet<String> = std::iter::once(cwd.clone()).collect();

    let accessible = with_unreadable_dir(&dir, || {
        verify_cwd_admitted_on_grant(&consent, &cwd, &admitted)
    })
    .await;

    assert!(!accessible, "a denied read must report inaccessible");
    assert!(
        !consent.grants().contains("Documents"),
        "the covering grant must be dropped so the directory defers again"
    );
}

/// A missing protected directory must read as absent (so it is never reported
/// as needing permission); a present one reads as existing.
#[tokio::test]
async fn path_existence_check_detects_missing_and_present() {
    use crate::repositories::access::path_exists_without_prompting;

    let dir = tempfile::TempDir::new().unwrap();
    assert!(path_exists_without_prompting(dir.path()).await);
    assert!(!path_exists_without_prompting(&dir.path().join("nope")).await);
}

/// Granting a protected *parent* directory — which is not a repository itself —
/// must surface the repositories *inside* it via a downward scan, not just the
/// repositories the granted path resolves up to.
#[tokio::test]
async fn resolve_granted_scans_parent_downward() {
    let parent = tempfile::TempDir::new().unwrap();
    let repo_dir = parent.path().join("nested-repo");
    init_repo(&repo_dir, "git@github.com:acme/nested-repo.git").await;

    let results = resolve_granted_repos(
        vec![parent.path().to_string_lossy().to_string()],
        "acme",
        &NoConsentGrants,
    )
    .await;

    assert_eq!(
        results.len(),
        1,
        "the child repository should be found by scanning the granted parent downward"
    );
    assert_eq!(results[0].full_name, "acme/nested-repo");
    assert_eq!(results[0].status, RepoAccessStatus::Accessible);
    assert_eq!(
        results[0].session_count, 0,
        "a scan-only granted repository has no sessions"
    );
    // The reported root is the canonical *path*, not the folded identity key —
    // on Windows the key lowercases and slash-normalizes, and `repo_root`'s
    // contract is a filesystem path. Matches the session-resolved arm.
    let canonical = crate::platform::git::canonical_main_repo_root(&repo_dir).await;
    assert_eq!(
        results[0].repo_root.as_deref(),
        Some(canonical.to_string_lossy().as_ref()),
        "scan-down must report the canonical path, not the identity key"
    );
}

#[tokio::test]
async fn resolve_granted_collapses_nested_cwds_and_counts_occurrences() {
    let parent = tempfile::TempDir::new().unwrap();
    let repo = parent.path().join("widgets");
    let first = repo.join("src/one");
    let second = repo.join("src/two");
    init_repo(&repo, "git@github.com:acme/widgets.git").await;
    tokio::fs::create_dir_all(&first).await.unwrap();
    tokio::fs::create_dir_all(&second).await.unwrap();

    let results = resolve_granted_repos(
        vec![
            repo.to_string_lossy().to_string(),
            first.to_string_lossy().to_string(),
            second.to_string_lossy().to_string(),
        ],
        "acme",
        &NoConsentGrants,
    )
    .await;

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].full_name, "acme/widgets");
    assert_eq!(results[0].status, RepoAccessStatus::Accessible);
    assert_eq!(results[0].session_count, 3);
    assert!(results[0].environment.is_native());
}

#[tokio::test]
async fn resolve_granted_skips_repo_of_another_owner_under_parent() {
    let parent = tempfile::TempDir::new().unwrap();
    init_repo(
        &parent.path().join("foreign"),
        "git@github.com:otherowner/foreign.git",
    )
    .await;

    let results = resolve_granted_repos(
        vec![parent.path().to_string_lossy().to_string()],
        "acme",
        &NoConsentGrants,
    )
    .await;

    assert!(
        results.is_empty(),
        "repositories belonging to another owner must not be surfaced"
    );
}

#[test]
fn sibling_scan_roots_are_repo_parents_excluding_home() {
    let home = PathBuf::from("/home/avery");
    let repos = vec![
        located(
            "/home/avery/acme/widgets",
            "git@github.com:acme/widgets",
            true,
            1,
            3,
        ),
        located(
            "/home/avery/acme/toolkit",
            "git@github.com:acme/toolkit",
            true,
            1,
            0,
        ),
        // A repository directly in the home directory must not turn the home
        // directory into a scan root.
        located(
            "/home/avery/loose-repo",
            "git@github.com:acme/loose",
            true,
            1,
            0,
        ),
    ];
    assert_eq!(
        sibling_scan_roots(&repos, &home),
        vec![PathBuf::from("/home/avery/acme")]
    );
}

#[test]
fn parent_scan_roots_skip_home_ancestors_and_dedup() {
    let home = PathBuf::from("/home/avery");
    let cwds = vec![
        // A workspace parent above the repositories, inside the home tree → kept.
        "/home/avery/workspace".to_string(),
        // Same parent again → deduped.
        "/home/avery/workspace".to_string(),
        // A parent OUTSIDE the home tree (mounted volume / shared dir) → kept;
        // discovery is intentionally not restricted to the home tree.
        "/opt/work".to_string(),
        // The home directory itself, an ancestor, and the filesystem root → all
        // too broad to scan, skipped.
        "/home/avery".to_string(),
        "/home".to_string(),
        "/".to_string(),
    ];
    assert_eq!(
        parent_scan_roots(&cwds, &home),
        vec![
            PathBuf::from("/home/avery/workspace"),
            PathBuf::from("/opt/work"),
        ]
    );
}

#[test]
fn parent_scan_roots_empty_when_everything_resolved() {
    assert!(parent_scan_roots(&[], &PathBuf::from("/home/avery")).is_empty());
}

#[test]
fn parent_scan_roots_are_capped() {
    let home = PathBuf::from("/home/avery");
    let cwds: Vec<String> = (0..(crate::repositories::MAX_PARENT_SCAN_ROOTS + 10))
        .map(|i| format!("/home/avery/workspace-{i}"))
        .collect();
    assert_eq!(
        parent_scan_roots(&cwds, &home).len(),
        crate::repositories::MAX_PARENT_SCAN_ROOTS
    );
}

/// A parent folder that is not itself a repository but holds one: deriving it as
/// a scan root and scanning surfaces the child, and the parent is persisted so
/// it keeps being scanned after the sessions age out.
#[tokio::test]
#[serial]
async fn unresolved_parent_cwd_yields_child_discovery_and_persists_root() {
    let home = tempfile::TempDir::new().unwrap();
    let _home = EnvGuard::set("HOME", home.path());
    let state_dir = home.path().join("state");
    tokio::fs::create_dir_all(&state_dir).await.unwrap();

    // `workspace` is a plain folder (no `.git`) holding one repository.
    let workspace = home.path().join("workspace");
    let child = workspace.join("child-repo");
    init_repo(&child, "git@github.com:acme/child-repo.git").await;
    // An unrelated empty parent must not be persisted.
    let empty_parent = home.path().join("empty");
    tokio::fs::create_dir_all(&empty_parent).await.unwrap();

    let unresolved = vec![
        workspace.to_string_lossy().to_string(),
        empty_parent.to_string_lossy().to_string(),
    ];
    let parent_roots = parent_scan_roots(&unresolved, home.path());
    assert!(parent_roots.contains(&workspace));
    assert!(parent_roots.contains(&empty_parent));

    // Scanning the derived parents surfaces the child repository.
    let consent = FakeConsent::empty(home.path());
    let (scan_repos, _deferred) =
        scan_roots_for_repos(home.path(), "acme", false, &parent_roots, &consent).await;
    assert_eq!(scan_repos.len(), 1);
    assert_eq!(scan_repos[0].remote_url, "git@github.com:acme/child-repo");

    // Only the parent that contained a repository is persisted.
    persist_confirmed_parent_roots(&state_dir, &parent_roots, &scan_repos).await;
    let persisted = scan_roots::load_scan_roots(&state_dir);
    assert!(
        persisted.iter().any(|path| path == &workspace),
        "a parent with a child repository should be persisted"
    );
    assert!(
        !persisted.iter().any(|path| path == &empty_parent),
        "an empty parent must not be persisted"
    );
}
