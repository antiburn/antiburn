// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Stage B: concurrent per-root git details, then the serial access fold that
//! is allowed to change consent state.

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

// serial, the consent fakes, and the access probe are consumed only by the
// unix-gated tests below.
#[cfg(unix)]
use serial_test::serial;

use super::support::run_git;
#[cfg(unix)]
use super::support::{EnvGuard, FakeConsent};
#[cfg(unix)]
use crate::repositories::access::verify_dir_access;
use crate::repositories::repo_root_identity;
use crate::repositories::sessions::{
    DistinctRepoRoot, IndexedTaskResult, RootGitDetails, resolve_distinct_roots_bounded_with,
    resolve_root_details_bounded, verify_root_details_serially,
};

#[tokio::test]
async fn stage_b_resolves_each_distinct_root_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let roots = ["first", "second", "third", "fourth"]
        .into_iter()
        .map(|key| DistinctRepoRoot {
            root_key: key.to_string(),
            canonical_root: PathBuf::from(format!("/repos/{key}")),
        })
        .collect();

    let results = resolve_distinct_roots_bounded_with(roots, 2, {
        let calls = Arc::clone(&calls);
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        move |root| {
            let calls = Arc::clone(&calls);
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let in_flight = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(in_flight, Ordering::SeqCst);
                let delay_ms = match root.root_key.as_str() {
                    "first" => 30,
                    "second" => 5,
                    "third" => 15,
                    _ => 1,
                };
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                root.root_key
            }
        }
    })
    .await;

    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(peak.load(Ordering::SeqCst), 2);
    assert_eq!(
        results
            .into_iter()
            .map(|result| result.outcome.unwrap())
            .collect::<Vec<_>>(),
        vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string(),
            "fourth".to_string(),
        ]
    );
}

/// Stage B reads details for every root, whoever owns it — the owner filter is
/// applied later. A repository with no remote yields nothing at all.
#[tokio::test]
async fn root_details_keep_other_owner_and_skip_remote_less_repo() {
    let temp = tempfile::TempDir::new().unwrap();
    let foreign = temp.path().join("foreign");
    let remote_less = temp.path().join("remote-less");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::create_dir_all(&remote_less).unwrap();
    run_git(&foreign, &["init", "-q"]).await;
    run_git(
        &foreign,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:otherowner/tool.git",
        ],
    )
    .await;
    run_git(&remote_less, &["init", "-q"]).await;

    let roots = vec![
        DistinctRepoRoot {
            root_key: repo_root_identity(&foreign),
            canonical_root: foreign,
        },
        DistinctRepoRoot {
            root_key: repo_root_identity(&remote_less),
            canonical_root: remote_less,
        },
    ];
    let details = resolve_root_details_bounded(roots, 2).await;

    let foreign = details[0].outcome.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(foreign.repo_owner, "otherowner");
    assert_eq!(foreign.repo_name, "tool");
    assert!(details[1].outcome.as_ref().unwrap().is_none());
}

#[tokio::test]
async fn stage_b_fold_restores_root_order_and_verifies_access_serially() {
    let detail = |repo_name: &str| RootGitDetails {
        normalized_url: format!("git@github.com:acme/{repo_name}"),
        repo_name: repo_name.to_string(),
        repo_owner: "acme".to_string(),
        worktree_count: 1,
    };
    let root = |key: &str| DistinctRepoRoot {
        root_key: key.to_string(),
        canonical_root: PathBuf::from(format!("/repos/{key}")),
    };
    let results = vec![
        IndexedTaskResult {
            index: 1,
            input: root("second"),
            outcome: Ok(Some(detail("second"))),
        },
        IndexedTaskResult {
            index: 0,
            input: root("first"),
            outcome: Ok(Some(detail("first"))),
        },
    ];
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let verification_order = Arc::new(Mutex::new(Vec::new()));

    let verified = verify_root_details_serially(results, {
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        let verification_order = Arc::clone(&verification_order);
        move |root| {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            let verification_order = Arc::clone(&verification_order);
            async move {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::task::yield_now().await;
                verification_order.lock().unwrap().push(root);
                active.fetch_sub(1, Ordering::SeqCst);
                true
            }
        }
    })
    .await;

    assert_eq!(peak.load(Ordering::SeqCst), 1);
    assert_eq!(
        verified
            .iter()
            .map(|result| result.root.root_key.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
    assert_eq!(
        *verification_order.lock().unwrap(),
        vec![
            PathBuf::from("/repos/first"),
            PathBuf::from("/repos/second")
        ]
    );
}

/// The serial fold is where consent state changes: two blocked roots drop their
/// own grants while a readable third keeps its own.
#[cfg(unix)]
#[tokio::test]
#[serial]
async fn serial_access_fold_evicts_multiple_stale_grants_and_retains_valid_one() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::TempDir::new().unwrap();
    let _home = EnvGuard::set("HOME", home.path());
    let documents = home.path().join("Documents").join("widgets");
    let desktop = home.path().join("Desktop").join("widgets");
    let downloads = home.path().join("Downloads").join("widgets");
    for path in [&documents, &desktop, &downloads] {
        std::fs::create_dir_all(path).unwrap();
    }
    let consent = FakeConsent::with_grants(home.path(), ["Documents", "Desktop", "Downloads"]);

    let detail = |index, root: &Path, repo_name: &str| IndexedTaskResult {
        index,
        input: DistinctRepoRoot {
            root_key: repo_root_identity(root),
            canonical_root: root.to_path_buf(),
        },
        outcome: Ok(Some(RootGitDetails {
            normalized_url: format!("git@github.com:acme/{repo_name}"),
            repo_name: repo_name.to_string(),
            repo_owner: "acme".to_string(),
            worktree_count: 1,
        })),
    };
    let details = vec![
        detail(0, &documents, "documents"),
        detail(1, &desktop, "desktop"),
        detail(2, &downloads, "downloads"),
    ];
    let documents_permissions = std::fs::metadata(&documents).unwrap().permissions();
    let desktop_permissions = std::fs::metadata(&desktop).unwrap().permissions();
    std::fs::set_permissions(&documents, std::fs::Permissions::from_mode(0o000)).unwrap();
    std::fs::set_permissions(&desktop, std::fs::Permissions::from_mode(0o000)).unwrap();
    let verified = verify_root_details_serially(details, |root| {
        let consent = &consent;
        async move { verify_dir_access(consent, &root).await }
    })
    .await;
    std::fs::set_permissions(&documents, documents_permissions).unwrap();
    std::fs::set_permissions(&desktop, desktop_permissions).unwrap();

    assert_eq!(verified.len(), 3);
    assert!(!verified[0].dir_accessible);
    assert!(!verified[1].dir_accessible);
    assert!(verified[2].dir_accessible);
    assert_eq!(
        consent.grants(),
        std::collections::HashSet::from(["Downloads".to_string()])
    );
}
