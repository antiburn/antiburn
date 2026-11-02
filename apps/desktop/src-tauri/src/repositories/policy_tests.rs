use super::*;
use crate::store::{SessionKey, SessionRecord};

const REPOSITORY_ROOT: &str = "/home/avery/code/widgets";

fn repository_policy_fixture() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(dir.path()).unwrap();
    store
        .replace_repositories(&[RepositoryRecord {
            key: "widgets".into(),
            repo_name: "widgets".into(),
            full_name: "avery/widgets".into(),
            status: "accessible".into(),
            repo_root: Some(REPOSITORY_ROOT.into()),
            suspected_path: None,
            worktree_count: 1,
            session_count: 1,
            wsl_distro: None,
            enabled: true,
        }])
        .unwrap();
    store
        .upsert_sessions(
            &[SessionRecord {
                key: SessionKey::new("native", "claude-code", "widgets-session"),
                source_kind: "file".into(),
                source_label: "file:widgets-session".into(),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: Some(format!("{REPOSITORY_ROOT}/src")),
                surface: "cli".into(),
                updated_at_epoch: Some(1),
                activity_cursor: String::new(),
                activity_source: "mtime".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: None,
            }],
            &[],
        )
        .unwrap();
    (dir, store)
}

#[tokio::test]
async fn repository_toggle_mirrors_ignored_paths_and_preserves_other_exclusions() {
    let (_dir, store) = repository_policy_fixture();
    ignored_paths::ignore_path(store.state_dir(), IGNORE_SCOPE, "/home/avery/code/private")
        .await
        .unwrap();
    ignored_paths::ignore_path(store.state_dir(), "another-scope", REPOSITORY_ROOT)
        .await
        .unwrap();

    assert!(set_enabled(&store, "widgets", false).await.unwrap());

    let ignored = ignored_paths::load_ignored(store.state_dir(), IGNORE_SCOPE);
    assert!(ignored.contains(REPOSITORY_ROOT));
    assert!(ignored.contains("/home/avery/code/private"));
    assert!(store.recent_sessions(0, 10).unwrap().is_empty());

    assert!(set_enabled(&store, "widgets", true).await.unwrap());

    let ignored = ignored_paths::load_ignored(store.state_dir(), IGNORE_SCOPE);
    assert!(!ignored.contains(REPOSITORY_ROOT));
    assert!(ignored.contains("/home/avery/code/private"));
    assert!(
        ignored_paths::load_ignored(store.state_dir(), "another-scope").contains(REPOSITORY_ROOT)
    );
    assert!(
        store
            .repository_policy_snapshot()
            .unwrap()
            .ignored_paths
            .is_empty()
    );
    assert!(store.repositories().unwrap()[0].enabled);
}

#[tokio::test]
async fn retrying_exclusion_repairs_an_unreadable_ignored_path_file() {
    let (_dir, store) = repository_policy_fixture();
    let file = ignored_paths::state_path(store.state_dir());
    std::fs::create_dir(&file).unwrap();
    let before = store.repository_policy_snapshot().unwrap().generation;

    let error = set_enabled(&store, "widgets", false).await.unwrap_err();

    assert!(error.to_string().contains("failed to read"));
    let policy = store.repository_policy_snapshot().unwrap();
    assert!(policy.generation > before);
    assert!(policy.ignored_paths.contains(REPOSITORY_ROOT));
    assert!(!store.repositories().unwrap()[0].enabled);
    assert_eq!(store.recent_sessions(0, 10).unwrap().len(), 1);
    assert!(file.is_dir());

    std::fs::remove_dir(&file).unwrap();
    assert!(set_enabled(&store, "widgets", false).await.unwrap());

    assert!(ignored_paths::load_ignored(store.state_dir(), IGNORE_SCOPE).contains(REPOSITORY_ROOT));
    assert!(store.recent_sessions(0, 10).unwrap().is_empty());
    assert_eq!(
        store.repository_policy_snapshot().unwrap().generation,
        policy.generation
    );
}

#[tokio::test]
async fn retrying_enable_repairs_a_malformed_ignored_path_file() {
    let (_dir, store) = repository_policy_fixture();
    assert!(set_enabled(&store, "widgets", false).await.unwrap());
    let file = ignored_paths::state_path(store.state_dir());
    let saved = std::fs::read(&file).unwrap();
    let malformed = serde_json::json!({
        "orgs": { (IGNORE_SCOPE): [{ "path": REPOSITORY_ROOT, "ignoredAt": 7 }] }
    })
    .to_string();
    std::fs::write(&file, &malformed).unwrap();
    let before = store.repository_policy_snapshot().unwrap().generation;

    let error = set_enabled(&store, "widgets", true).await.unwrap_err();

    assert!(error.to_string().contains("failed to parse"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), malformed);
    let policy = store.repository_policy_snapshot().unwrap();
    assert!(policy.generation > before);
    assert!(policy.ignored_paths.is_empty());
    assert!(store.repositories().unwrap()[0].enabled);

    std::fs::write(&file, &saved).unwrap();
    assert!(set_enabled(&store, "widgets", true).await.unwrap());

    assert!(
        !ignored_paths::load_ignored(store.state_dir(), IGNORE_SCOPE).contains(REPOSITORY_ROOT)
    );
    assert_eq!(
        store.repository_policy_snapshot().unwrap().generation,
        policy.generation
    );
}

#[tokio::test]
async fn repository_policy_write_failure_leaves_file_and_sessions_unchanged() {
    let (_dir, store) = repository_policy_fixture();
    ignored_paths::ignore_path(store.state_dir(), IGNORE_SCOPE, "/home/avery/code/private")
        .await
        .unwrap();
    let file = ignored_paths::state_path(store.state_dir());
    let saved = std::fs::read(&file).unwrap();
    let before = store.repository_policy_snapshot().unwrap().generation;
    store
        .lock()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_repository_policy BEFORE UPDATE OF enabled ON repository
             BEGIN SELECT RAISE(ABORT, 'injected repository policy failure'); END;",
        )
        .unwrap();

    let error = set_enabled(&store, "widgets", false).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("injected repository policy failure")
    );
    assert!(store.repositories().unwrap()[0].enabled);
    let policy = store.repository_policy_snapshot().unwrap();
    assert_eq!(policy.generation, before);
    assert!(policy.ignored_paths.is_empty());
    assert_eq!(std::fs::read(&file).unwrap(), saved);
    assert_eq!(store.recent_sessions(0, 10).unwrap().len(), 1);
    store
        .lock()
        .execute_batch("DROP TRIGGER fail_repository_policy")
        .unwrap();

    assert!(set_enabled(&store, "widgets", false).await.unwrap());

    assert!(ignored_paths::load_ignored(store.state_dir(), IGNORE_SCOPE).contains(REPOSITORY_ROOT));
    assert!(store.recent_sessions(0, 10).unwrap().is_empty());
}

#[test]
fn retrying_repository_toggle_repairs_a_failed_ignored_path_write() {
    use std::future::{Future, poll_fn};
    use std::task::Poll;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    for enabled in [false, true] {
        let (_dir, store) = repository_policy_fixture();
        store.set_repository_enabled("widgets", !enabled).unwrap();
        let file = ignored_paths::state_path(store.state_dir());
        let entries = if enabled {
            vec![serde_json::json!({ "path": REPOSITORY_ROOT })]
        } else {
            Vec::new()
        };
        let saved = serde_json::to_vec(&serde_json::json!({
            "orgs": { (IGNORE_SCOPE): entries }
        }))
        .unwrap();
        std::fs::write(&file, &saved).unwrap();

        runtime.block_on(async {
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            // Hold the only blocking worker until the file read and policy
            // commit finish. The next file write cannot start before release.
            let blocker = tokio::task::spawn_blocking(move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
            started_rx.recv().unwrap();
            let mut request = Box::pin(set_enabled(&store, "widgets", enabled));
            poll_fn(|context| {
                assert!(request.as_mut().poll(context).is_pending());
                if store.repositories().unwrap()[0].enabled == enabled {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            let generation = store.repository_policy_snapshot().unwrap().generation;
            std::fs::remove_file(&file).unwrap();
            std::fs::create_dir(&file).unwrap();
            release_tx.send(()).unwrap();

            let error = request.await.unwrap_err();
            blocker.await.unwrap();

            assert!(error.to_string().contains("failed to atomically replace"));
            assert_eq!(store.repositories().unwrap()[0].enabled, enabled);
            assert_eq!(store.recent_sessions(0, 10).unwrap().len(), 1);
            assert!(file.is_dir());
            std::fs::remove_dir(&file).unwrap();
            std::fs::write(&file, &saved).unwrap();

            assert!(set_enabled(&store, "widgets", enabled).await.unwrap());

            assert_eq!(
                ignored_paths::load_ignored(store.state_dir(), IGNORE_SCOPE)
                    .contains(REPOSITORY_ROOT),
                !enabled
            );
            assert_eq!(
                store.repository_policy_snapshot().unwrap().generation,
                generation
            );
            assert_eq!(
                store.recent_sessions(0, 10).unwrap().len(),
                usize::from(enabled)
            );
        });
    }
}

#[tokio::test]
async fn refresh_rejects_an_unreadable_ignored_path_file_without_publishing() {
    let (_dir, store) = repository_policy_fixture();
    std::fs::create_dir(ignored_paths::state_path(store.state_dir())).unwrap();
    let before = store.repository_policy_snapshot().unwrap().generation;

    let error = refresh_from_store(&store).await.unwrap_err();

    assert!(error.to_string().contains("failed to read"));
    assert_eq!(
        store.repository_policy_snapshot().unwrap().generation,
        before
    );
    let repositories = store.repositories().unwrap();
    assert_eq!(repositories.len(), 1);
    assert_eq!(repositories[0].key, "widgets");
    assert!(repositories[0].enabled);
    assert_eq!(store.recent_sessions(0, 10).unwrap().len(), 1);
}

#[tokio::test]
async fn a_suspected_repository_path_is_mirrored_and_retryable() {
    let (_dir, store) = repository_policy_fixture();
    let mut repository = store.repositories().unwrap().pop().unwrap();
    repository.suspected_path = repository.repo_root.take();
    store.replace_repositories(&[repository]).unwrap();

    assert!(set_enabled(&store, "widgets", false).await.unwrap());
    assert!(ignored_paths::load_ignored(store.state_dir(), IGNORE_SCOPE).contains(REPOSITORY_ROOT));
    assert!(store.recent_sessions(0, 10).unwrap().is_empty());

    assert!(set_enabled(&store, "widgets", false).await.unwrap());
    assert!(set_enabled(&store, "widgets", true).await.unwrap());
    assert!(
        !ignored_paths::load_ignored(store.state_dir(), IGNORE_SCOPE).contains(REPOSITORY_ROOT)
    );
}
