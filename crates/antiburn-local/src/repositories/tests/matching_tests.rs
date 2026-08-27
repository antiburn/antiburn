//! Turning located clones into a result: merging, matching against a
//! known-repository list, and filtering consent-deferred paths.

use std::collections::HashSet;

use super::support::located;
use crate::repositories::matching::deferred_correlates_to_known;
use crate::repositories::{
    DeferredProtectedPath, DiscoveryMode, RepoAccessStatus, RepositoryDescriptor,
    dedup_deferred_by_cwd, filter_deferred_by_known, located_repos_for_owner, match_known_repos,
    merge_located_repos,
};

fn descriptor(name: &str, owner: &str, enabled: Option<bool>) -> RepositoryDescriptor {
    RepositoryDescriptor {
        full_name: format!("{owner}/{name}"),
        name: name.to_string(),
        clone_url: format!("https://github.com/{owner}/{name}.git"),
        ssh_url: format!("git@github.com:{owner}/{name}.git"),
        enabled,
    }
}

#[test]
fn merge_prefers_accessible_and_keeps_max_count() {
    // Same root located twice: by session (blocked, 5 sessions) and by scan
    // (accessible, 0 sessions, 2 worktrees).
    let session = located("/r", "git@github.com:avery/r", false, 1, 5);
    let scan = located("/r", "git@github.com:avery/r", true, 2, 0);
    let merged = merge_located_repos(vec![session], vec![scan]);
    assert_eq!(merged.len(), 1);
    assert!(merged[0].dir_accessible);
    assert_eq!(merged[0].session_count, 5);
    assert_eq!(merged[0].worktree_count, 2);
}

#[test]
fn merge_keeps_distinct_roots() {
    let a = located("/a", "git@github.com:avery/a", true, 1, 1);
    let b = located("/b", "git@github.com:avery/b", true, 1, 0);
    assert_eq!(merge_located_repos(vec![a], vec![b]).len(), 2);
}

#[test]
fn merge_collapses_windows_separator_variants() {
    let backslashes = located(
        r"C:\Users\avery\widgets",
        "git@github.com:avery/widgets",
        true,
        1,
        2,
    );
    let forward_slashes = located(
        "C:/Users/avery/widgets",
        "git@github.com:avery/widgets",
        true,
        1,
        3,
    );

    let merged = merge_located_repos(vec![backslashes], vec![forward_slashes]);

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].session_count, 3);
}

/// The rename case: the known list carries the new name while the clone's git
/// remote still carries the old one, so URL matching misses it. The clone must
/// still surface under its local name instead of vanishing.
#[test]
fn match_known_surfaces_unmatched_owner_clone() {
    let known = vec![descriptor("sprockets", "acme", Some(true))];
    let clone = located(
        "/home/avery/acme/widgets",
        "https://github.com/acme/widgets",
        true,
        0,
        5,
    );
    let result = match_known_repos(known, &[clone], "acme");

    let renamed = result
        .repos
        .iter()
        .find(|repo| repo.full_name == "acme/sprockets")
        .expect("renamed known repository present");
    assert_eq!(renamed.status, RepoAccessStatus::NotCloned);

    let surfaced = result
        .repos
        .iter()
        .find(|repo| repo.repo_name == "widgets")
        .expect("local clone surfaced");
    assert_eq!(surfaced.status, RepoAccessStatus::Accessible);
    assert_eq!(surfaced.session_count, 5);
    assert_eq!(
        surfaced.repo_root.as_deref(),
        Some("/home/avery/acme/widgets")
    );
}

#[test]
fn match_known_does_not_surface_foreign_owner_clone() {
    let clone = located(
        "/home/avery/other",
        "https://github.com/otherowner/other",
        true,
        0,
        9,
    );
    let result = match_known_repos(Vec::new(), &[clone], "acme");
    assert!(
        result.repos.is_empty(),
        "a clone belonging to another owner must not be surfaced"
    );
}

#[test]
fn match_known_no_double_emit_for_matched_clone() {
    let known = vec![descriptor("widgets", "acme", Some(true))];
    let clone = located(
        "/home/avery/widgets",
        "https://github.com/acme/widgets",
        true,
        0,
        3,
    );
    let result = match_known_repos(known, &[clone], "acme");
    assert_eq!(result.repos.len(), 1);
    assert_eq!(result.repos[0].status, RepoAccessStatus::Accessible);
    assert_eq!(result.repos[0].session_count, 3);
}

#[test]
fn match_known_unmatched_clone_permission_denied() {
    let clone = located(
        "/home/avery/acme/widgets",
        "https://github.com/acme/widgets",
        false,
        0,
        2,
    );
    let result = match_known_repos(Vec::new(), &[clone], "acme");
    let surfaced = result
        .repos
        .iter()
        .find(|repo| repo.repo_name == "widgets")
        .expect("clone surfaced");
    assert_eq!(surfaced.status, RepoAccessStatus::PermissionDenied);
    assert_eq!(
        surfaced.suspected_path.as_deref(),
        Some("/home/avery/acme/widgets")
    );
    assert!(surfaced.repo_root.is_none());
}

#[test]
fn match_known_respects_disable_and_defaults_unmatched_enabled() {
    let known = vec![
        descriptor("widgets", "acme", Some(true)),
        descriptor("toolkit", "acme", Some(false)),
    ];
    let clones = vec![
        located(
            "/home/avery/widgets",
            "https://github.com/acme/widgets",
            true,
            0,
            1,
        ),
        located(
            "/home/avery/toolkit",
            "https://github.com/acme/toolkit",
            true,
            0,
            1,
        ),
        located(
            "/home/avery/legacy",
            "https://github.com/acme/legacy",
            true,
            0,
            1,
        ),
    ];
    let result = match_known_repos(known, &clones, "acme");

    let enabled_of = |name: &str| {
        result
            .repos
            .iter()
            .find(|repo| repo.repo_name == name)
            .unwrap_or_else(|| panic!("{name} present"))
            .enabled
    };
    assert!(enabled_of("widgets"), "selected repository is enabled");
    assert!(
        !enabled_of("toolkit"),
        "explicitly excluded repository stays disabled"
    );
    assert!(
        enabled_of("legacy"),
        "a clone matching no known repository defaults to enabled"
    );
}

/// The tri-state that matters: a caller whose source predates the notion of
/// selection leaves `enabled` unset, and that must read as enabled — otherwise
/// one absent field disables every repository at once.
#[test]
fn match_known_treats_absent_enabled_as_enabled() {
    let known = vec![descriptor("widgets", "acme", None)];
    let clones = vec![located(
        "/home/avery/widgets",
        "https://github.com/acme/widgets",
        true,
        0,
        1,
    )];
    let result = match_known_repos(known, &clones, "acme");
    let widgets = result
        .repos
        .iter()
        .find(|repo| repo.repo_name == "widgets")
        .expect("present");
    assert!(
        widgets.enabled,
        "an unset selection flag is treated as enabled"
    );
}

/// The same tri-state on the not-cloned path: an unset flag must not present a
/// missing repository as excluded by the caller.
#[test]
fn match_known_treats_absent_enabled_as_enabled_when_not_cloned() {
    let result = match_known_repos(vec![descriptor("widgets", "acme", None)], &[], "acme");
    assert_eq!(result.repos[0].status, RepoAccessStatus::NotCloned);
    assert!(result.repos[0].enabled);
}

#[test]
fn located_repos_for_owner_defaults_all_enabled() {
    let clones = vec![
        located("/home/avery/a", "https://github.com/acme/a", true, 0, 1),
        located("/home/avery/b", "https://github.com/acme/b", false, 0, 1),
    ];
    let result = located_repos_for_owner("acme", &clones);
    assert_eq!(result.discovery_mode, DiscoveryMode::SessionsOnly);
    assert!(!result.repos.is_empty());
    assert!(
        result.repos.iter().all(|repo| repo.enabled),
        "without a known-repository list every repository defaults to enabled"
    );
}

#[test]
fn located_repos_for_owner_exposes_wsl_distro_for_unc_repo() {
    let repos = vec![located(
        r"\\wsl.localhost\Ubuntu\home\avery\widgets",
        "git@github.com:acme/widgets.git",
        true,
        1,
        2,
    )];

    let result = located_repos_for_owner("acme", &repos);

    assert_eq!(result.repos.len(), 1);
    assert_eq!(result.repos[0].environment.wsl_distro(), Some("Ubuntu"));
    assert_eq!(
        serde_json::to_value(&result.repos[0]).unwrap()["wslDistro"],
        "Ubuntu"
    );
}

/// A record written before the selection flag existed must deserialize to
/// enabled, so an upgrade does not silently disable everything.
#[test]
fn local_repo_status_defaults_enabled_when_field_absent() {
    let json = serde_json::json!({
        "repoName": "widgets",
        "fullName": "acme/widgets",
        "status": "accessible",
        "repoRoot": "/home/avery/widgets",
        "suspectedPath": null,
        "worktreeCount": 0,
    });
    let status: crate::repositories::LocalRepoStatus = serde_json::from_value(json).unwrap();
    assert!(status.enabled);
    assert_eq!(status.session_count, 0);
    assert!(status.environment.is_native());
}

#[test]
fn dedup_deferred_keeps_first_per_cwd() {
    let mut deferred = vec![
        DeferredProtectedPath {
            cwd: "/home/avery/Documents/GitHub".into(),
            protected_dir: "Documents".into(),
        },
        DeferredProtectedPath {
            cwd: "/home/avery/Documents/GitHub".into(),
            protected_dir: "Documents".into(),
        },
        DeferredProtectedPath {
            cwd: "/home/avery/Documents/widgets".into(),
            protected_dir: "Documents".into(),
        },
    ];
    dedup_deferred_by_cwd(&mut deferred);
    assert_eq!(deferred.len(), 2);
    assert_eq!(deferred[0].cwd, "/home/avery/Documents/GitHub");
    assert_eq!(deferred[1].cwd, "/home/avery/Documents/widgets");
}

#[test]
fn deferred_correlates_to_known_by_folder_name() {
    let names: HashSet<String> = ["widgets".to_string(), "toolkit".to_string()]
        .into_iter()
        .collect();
    // Folder name matches a known repository (case-insensitive).
    assert!(deferred_correlates_to_known(
        "/home/avery/Documents/widgets",
        &names
    ));
    assert!(deferred_correlates_to_known(
        "/home/avery/Documents/Widgets",
        &names
    ));
    // Unrelated local projects are dropped.
    assert!(!deferred_correlates_to_known(
        "/home/avery/Documents/tax-returns",
        &names
    ));
    assert!(!deferred_correlates_to_known(
        "/home/avery/Desktop/scratch",
        &names
    ));
    // No known-repository list correlates nothing.
    assert!(!deferred_correlates_to_known(
        "/home/avery/Documents/widgets",
        &HashSet::new()
    ));
}

#[test]
fn filter_deferred_keeps_all_without_known_list() {
    let deferred = vec![
        DeferredProtectedPath {
            cwd: "/home/avery/Documents/widgets".into(),
            protected_dir: "Documents".into(),
        },
        DeferredProtectedPath {
            cwd: "/home/avery/Desktop/scratch".into(),
            protected_dir: "Desktop".into(),
        },
    ];
    // No known-repository list: nothing is hidden — the filter is held until
    // correlation is possible.
    assert_eq!(
        filter_deferred_by_known(deferred.clone(), &HashSet::new(), false).len(),
        2
    );

    // With a list, only correlating paths survive.
    let names: HashSet<String> = ["widgets".to_string()].into_iter().collect();
    let kept = filter_deferred_by_known(deferred.clone(), &names, true);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].cwd, "/home/avery/Documents/widgets");

    // A present-but-empty list drops every uncorrelated protected folder rather
    // than surfacing foreign projects.
    assert!(filter_deferred_by_known(deferred, &HashSet::new(), true).is_empty());
}
