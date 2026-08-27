//! Stage A: bounded concurrent resolution of working directories to repository
//! roots, and the ordered fold that turns the results into distinct roots.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

// serial + the consent fakes are consumed only by the unix-gated tests below.
#[cfg(unix)]
use serial_test::serial;
use tokio::sync::Semaphore;

use super::support::run_git;
#[cfg(unix)]
use super::support::{EnvGuard, FakeConsent};
#[cfg(unix)]
use crate::repositories::sessions::verify_failed_granted_cwds;
use crate::repositories::sessions::{
    DistinctRepoRoot, IndexedTaskResult, SeenRepos, fold_cwd_resolutions, located_repos_from_seen,
    resolve_cwd_roots_bounded, run_indexed_bounded_with_progress,
};
use crate::repositories::{concurrency_from, located_repos_for_owner, repo_root_identity};

#[test]
fn concurrency_configuration_accepts_positive_overrides() {
    assert_eq!(concurrency_from(Some("1"), Some(16)), 1);
    assert_eq!(concurrency_from(Some(" 12 "), Some(2)), 12);
    assert_eq!(
        concurrency_from(Some(&usize::MAX.to_string()), Some(2)),
        Semaphore::MAX_PERMITS
    );
}

#[test]
fn concurrency_configuration_clamps_cpu_default_and_uses_fallback() {
    assert_eq!(concurrency_from(None, Some(1)), 2);
    assert_eq!(concurrency_from(None, Some(6)), 6);
    assert_eq!(concurrency_from(None, Some(64)), 8);
    assert_eq!(concurrency_from(None, None), 4);
    assert_eq!(concurrency_from(Some("0"), Some(5)), 5);
    assert_eq!(concurrency_from(Some("nope"), None), 4);
}

#[tokio::test]
async fn indexed_runner_enforces_peak_concurrency_and_input_order() {
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let completion_order = Arc::new(Mutex::new(Vec::new()));

    let mut progress = Vec::new();
    let results = run_indexed_bounded_with_progress(
        (0usize..8).collect(),
        3,
        {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            let completion_order = Arc::clone(&completion_order);
            move |value| {
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                let completion_order = Arc::clone(&completion_order);
                async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis((8 - value) as u64 * 4)).await;
                    completion_order.lock().unwrap().push(value);
                    active.fetch_sub(1, Ordering::SeqCst);
                    value * 10
                }
            }
        },
        |completed| progress.push(completed),
    )
    .await;

    assert_eq!(progress, (1usize..=8).collect::<Vec<_>>());
    assert_eq!(peak.load(Ordering::SeqCst), 3);
    assert_ne!(
        *completion_order.lock().unwrap(),
        (0usize..8).collect::<Vec<_>>(),
        "the fixture must actually complete out of order"
    );
    assert_eq!(
        results
            .iter()
            .map(|result| result.index)
            .collect::<Vec<_>>(),
        (0usize..8).collect::<Vec<_>>()
    );
    assert_eq!(
        results
            .into_iter()
            .map(|result| result.outcome.unwrap())
            .collect::<Vec<_>>(),
        (0usize..8).map(|value| value * 10).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn indexed_runner_isolates_task_failure() {
    let mut progress = Vec::new();
    let results = run_indexed_bounded_with_progress(
        vec![0usize, 1, 2],
        2,
        |value| async move {
            assert_ne!(value, 1, "intentional task failure");
            value
        },
        |completed| progress.push(completed),
    )
    .await;

    assert_eq!(progress, vec![1, 2, 3]);
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].outcome.as_ref().unwrap(), &0);
    assert!(results[1].outcome.is_err());
    assert_eq!(results[2].outcome.as_ref().unwrap(), &2);
}

#[test]
fn cwd_fold_is_deterministic_aggregates_counts_and_keeps_failed_parents() {
    let main_root = PathBuf::from("/repos/main");
    let clone_root = PathBuf::from("/repos/clone");
    let cwd_counts = HashMap::from([
        ("/repos/main/subdir".to_string(), 2),
        ("/repos/main-worktree".to_string(), 5),
        ("/repos/clone".to_string(), 3),
        ("/workspace".to_string(), 7),
    ]);
    let admitted = HashSet::from(["/workspace".to_string()]);
    let resolutions = vec![
        IndexedTaskResult {
            index: 3,
            input: "/repos/clone".to_string(),
            outcome: Ok(Some(clone_root.clone())),
        },
        IndexedTaskResult {
            index: 1,
            input: "/workspace".to_string(),
            outcome: Ok(None),
        },
        IndexedTaskResult {
            index: 2,
            input: "/repos/main-worktree".to_string(),
            outcome: Ok(Some(main_root.clone())),
        },
        IndexedTaskResult {
            index: 0,
            input: "/repos/main/subdir".to_string(),
            outcome: Ok(Some(main_root.clone())),
        },
        IndexedTaskResult {
            index: 4,
            input: "/task-failed".to_string(),
            outcome: Err("join failed".to_string()),
        },
    ];

    let folded = fold_cwd_resolutions(resolutions, &cwd_counts, &admitted);

    assert_eq!(
        folded.distinct_roots,
        vec![
            DistinctRepoRoot {
                root_key: repo_root_identity(&main_root),
                canonical_root: main_root.clone(),
            },
            DistinctRepoRoot {
                root_key: repo_root_identity(&clone_root),
                canonical_root: clone_root,
            },
        ]
    );
    assert_eq!(
        folded.root_session_counts[&repo_root_identity(&main_root)],
        7
    );
    assert_eq!(
        folded.unresolved_cwds,
        vec!["/workspace".to_string(), "/task-failed".to_string()]
    );
    assert_eq!(folded.failed_granted_cwds, vec!["/workspace"]);
}

/// A linked worktree and its main repository are one repository: both working
/// directories must collapse onto the same root and their session counts sum.
#[tokio::test]
async fn real_linked_worktree_cwds_collapse_and_sum_session_counts() {
    let temp = tempfile::TempDir::new().unwrap();
    let main = temp.path().join("main");
    let linked = temp.path().join("linked");
    std::fs::create_dir_all(&main).unwrap();
    run_git(&main, &["init", "-q"]).await;
    run_git(&main, &["config", "user.email", "avery@example.com"]).await;
    run_git(&main, &["config", "user.name", "Avery"]).await;
    std::fs::write(main.join("README.md"), "fixture\n").unwrap();
    run_git(&main, &["add", "README.md"]).await;
    run_git(&main, &["commit", "-q", "-m", "fixture"]).await;
    run_git(&main, &["worktree", "add", "-q", linked.to_str().unwrap()]).await;

    let main_cwd = main.to_string_lossy().to_string();
    let linked_cwd = linked.to_string_lossy().to_string();
    let counts = HashMap::from([(main_cwd.clone(), 2), (linked_cwd.clone(), 5)]);
    let resolutions = resolve_cwd_roots_bounded(vec![main_cwd, linked_cwd], 2).await;
    let folded = fold_cwd_resolutions(resolutions, &counts, &HashSet::new());

    assert_eq!(folded.distinct_roots.len(), 1);
    assert_eq!(folded.root_session_counts.values().copied().sum::<u32>(), 7);
}

#[test]
fn final_located_repo_mapping_preserves_wsl_environment_and_counts() {
    let root = PathBuf::from(r"\\wsl.localhost\Ubuntu\home\avery\widgets");
    let root_key = repo_root_identity(&root);
    let seen = SeenRepos::from([(
        root_key.clone(),
        ("git@github.com:acme/widgets".to_string(), root, true, 2),
    )]);
    let counts = HashMap::from([(root_key, 7)]);

    let repos = located_repos_from_seen(seen, &counts);
    let result = located_repos_for_owner("acme", &repos);

    assert_eq!(result.repos.len(), 1);
    assert_eq!(result.repos[0].environment.wsl_distro(), Some("Ubuntu"));
    assert_eq!(result.repos[0].session_count, 7);
    assert_eq!(result.repos[0].worktree_count, 2);
}

/// The Stage A recovery bridge: a working directory admitted on a recorded grant
/// that then fails to resolve gets re-checked, and a real denial drops the
/// grant.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn stage_a_failed_grant_bridge_evicts_the_stale_grant() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::TempDir::new().unwrap();
    let _home = EnvGuard::set("HOME", home.path());
    let cwd_path = home.path().join("Documents").join("widgets");
    std::fs::create_dir_all(&cwd_path).unwrap();
    let consent = FakeConsent::with_grants(home.path(), ["Documents"]);

    let cwd = cwd_path.to_string_lossy().to_string();
    let admitted = HashSet::from([cwd.clone()]);
    let folded = fold_cwd_resolutions(
        vec![IndexedTaskResult {
            index: 0,
            input: cwd.clone(),
            outcome: Ok(None),
        }],
        &HashMap::from([(cwd, 1)]),
        &admitted,
    );
    let permissions = std::fs::metadata(&cwd_path).unwrap().permissions();
    std::fs::set_permissions(&cwd_path, std::fs::Permissions::from_mode(0o000)).unwrap();
    verify_failed_granted_cwds(&consent, &folded.failed_granted_cwds, &admitted).await;
    std::fs::set_permissions(&cwd_path, permissions).unwrap();

    assert!(
        !consent.grants().contains("Documents"),
        "the ordered Stage A recovery bridge must drop a stale grant"
    );
}

/// `Path` is used only through the fixtures above on some targets; keep the
/// import honest across cfg combinations.
#[test]
fn identity_is_stable_for_a_plain_path() {
    assert_eq!(
        repo_root_identity(Path::new("/repos/main/")),
        repo_root_identity(Path::new("/repos/main"))
    );
}
