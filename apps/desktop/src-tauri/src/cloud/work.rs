//! Admit Cloud blocking work before it enters Tokio's shared blocking pool.
//! The permit stays with a running closure even if its awaiting caller stops.

use std::sync::Arc;
use tokio::sync::Semaphore;

/// Safe renderer error for work that has not started and can be retried.
pub(super) const BUSY: &str = "Cloud is busy. Try again.";

/// Normal work fails fast. Cleanup waits asynchronously for the current job and
/// takes the next permit before later normal work can enter.
pub(super) enum Priority {
    Normal,
    Cleanup,
}

/// One shared blocking slot for Cloud auth, SQLite, keyring, and source work.
/// This bounds submitted closures, not just work inside the controller mutex.
#[derive(Clone)]
pub(super) struct BlockingWork {
    admission: Arc<Semaphore>,
}

impl Default for BlockingWork {
    fn default() -> Self {
        Self {
            admission: Arc::new(Semaphore::new(1)),
        }
    }
}

impl BlockingWork {
    /// Run synchronous work off runtime/UI threads after acquiring admission.
    /// Generation-changing cleanup callers invalidate before awaiting this method.
    /// Browser socket waiting belongs in an async task, outside this slot.
    pub(super) async fn run<T, F>(&self, priority: Priority, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, String> + Send + 'static,
    {
        let permit = match priority {
            Priority::Normal => self
                .admission
                .clone()
                .try_acquire_owned()
                .map_err(|_| BUSY.to_owned())?,
            Priority::Cleanup => self
                .admission
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| "Cloud is unavailable.".to_owned())?,
        };
        tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            operation()
        })
        .await
        .map_err(|_| "Cloud is unavailable.".to_owned())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicU64, Ordering},
            mpsc,
        },
        time::Duration,
    };
    use tokio::sync::oneshot;

    #[tokio::test(flavor = "current_thread")]
    async fn cloud_blocking_admission_rejects_extra_work_without_blocking_the_runtime() {
        let work = BlockingWork::default();
        let running = work.clone();
        let runtime_thread = std::thread::current().id();
        let (started, start) = oneshot::channel();
        let (release, wait) = mpsc::channel();
        let first = tokio::spawn(async move {
            running
                .run(Priority::Normal, move || {
                    assert_ne!(std::thread::current().id(), runtime_thread);
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(5))
                        .map_err(|_| "test release".into())
                })
                .await
        });
        start.await.unwrap();
        assert_eq!(
            work.run(Priority::Normal, || Ok(())).await,
            Err(BUSY.into())
        );
        tokio::time::timeout(
            Duration::from_millis(250),
            tokio::time::sleep(Duration::from_millis(1)),
        )
        .await
        .unwrap();
        release.send(()).unwrap();
        first.await.unwrap().unwrap();
        work.run(Priority::Normal, || Ok(())).await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cloud_cleanup_invalidates_immediately_and_waits_ahead_of_normal_work() {
        let work = BlockingWork::default();
        let generation = Arc::new(AtomicU64::new(0));
        let active_generation = generation.clone();
        let running = work.clone();
        let (started, start) = oneshot::channel();
        let (release, wait) = mpsc::channel();
        let first = tokio::spawn(async move {
            running
                .run(Priority::Normal, move || {
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(5))
                        .map_err(|_| "test release".to_owned())?;
                    assert_eq!(active_generation.load(Ordering::Acquire), 1);
                    Ok(())
                })
                .await
        });
        start.await.unwrap();
        generation.fetch_add(1, Ordering::AcqRel);
        let cleanup_work = work.clone();
        let (queued, queue) = oneshot::channel();
        let cleanup = tokio::spawn(async move {
            queued.send(()).unwrap();
            cleanup_work.run(Priority::Cleanup, || Ok("cleaned")).await
        });
        queue.await.unwrap();
        assert_eq!(
            work.run(Priority::Normal, || Ok(())).await,
            Err(BUSY.into())
        );
        release.send(()).unwrap();
        assert_eq!(cleanup.await.unwrap().unwrap(), "cleaned");
        first.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cloud_cancelled_awaiters_do_not_release_running_work_admission() {
        let work = BlockingWork::default();
        let running = work.clone();
        let (started, start) = oneshot::channel();
        let (release, wait) = mpsc::channel();
        let caller = tokio::spawn(async move {
            running
                .run(Priority::Normal, move || {
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(5))
                        .map_err(|_| "test release".into())
                })
                .await
        });
        start.await.unwrap();
        caller.abort();
        assert_eq!(
            work.run(Priority::Normal, || Ok(())).await,
            Err(BUSY.into())
        );
        release.send(()).unwrap();
        work.run(Priority::Cleanup, || Ok(())).await.unwrap();
        work.run(Priority::Normal, || Ok(())).await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cloud_admitted_browser_begin_cannot_replace_a_later_cancellation() {
        let work = BlockingWork::default();
        let generation = Arc::new(AtomicU64::new(7));
        let expected_generation = generation.load(Ordering::Acquire);
        let active_generation = generation.clone();
        let (started, start) = oneshot::channel();
        let (release, wait) = mpsc::channel();
        let begin = tokio::spawn(async move {
            work.run(Priority::Normal, move || {
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5))
                    .map_err(|_| "test release".to_owned())?;
                active_generation
                    .compare_exchange(
                        expected_generation,
                        expected_generation + 1,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .map_err(|_| "Cloud sign-in was cancelled.".to_owned())
            })
            .await
        });
        start.await.unwrap();
        generation.fetch_add(1, Ordering::AcqRel);
        release.send(()).unwrap();
        assert_eq!(
            begin.await.unwrap(),
            Err("Cloud sign-in was cancelled.".into())
        );
        assert_eq!(generation.load(Ordering::Acquire), 8);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cloud_lazy_sqlite_setup_runs_off_runtime_and_retries_failed_initialization() {
        let runtime_thread = std::thread::current().id();
        BlockingWork::default()
            .run(Priority::Normal, move || {
                assert_ne!(std::thread::current().id(), runtime_thread);
                let directory = tempfile::tempdir().unwrap();
                let store = crate::store::Store::open_in_memory(directory.path()).unwrap();
                let ready = std::sync::atomic::AtomicBool::new(false);
                assert!(store.cloud_draft("account", "personal").is_err());
                store
                    .lock()
                    .execute_batch("CREATE TABLE cloud_capture_request (invalid INTEGER);")
                    .unwrap();
                assert!(super::super::ensure_cloud_setup(&ready, &store).is_err());
                assert!(!ready.load(Ordering::Acquire));
                store
                    .lock()
                    .execute_batch("DROP TABLE cloud_capture_request;")
                    .unwrap();
                super::super::ensure_cloud_setup(&ready, &store)?;
                assert!(ready.load(Ordering::Acquire));
                assert_eq!(store.cloud_draft("account", "personal").unwrap(), None);
                super::super::ensure_cloud_setup(&ready, &store)?;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cloud_store_readers_cannot_overtake_initialization() {
        let work = BlockingWork::default();
        let setup = work.clone();
        let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let setup_ready = ready.clone();
        let (started, start) = oneshot::channel();
        let (release, wait) = mpsc::channel();
        let first = tokio::spawn(async move {
            setup
                .run(Priority::Normal, move || {
                    let directory = tempfile::tempdir().unwrap();
                    let store = crate::store::Store::open_in_memory(directory.path()).unwrap();
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(5))
                        .map_err(|_| "test release".to_owned())?;
                    super::super::ensure_cloud_setup(&setup_ready, &store)?;
                    Ok((directory, store))
                })
                .await
        });
        start.await.unwrap();
        assert_eq!(
            work.run(Priority::Normal, || Ok(())).await,
            Err(BUSY.into())
        );
        assert!(!ready.load(Ordering::Acquire));
        release.send(()).unwrap();
        let (directory, store) = first.await.unwrap().unwrap();
        work.run(Priority::Normal, move || {
            assert!(ready.load(Ordering::Acquire));
            assert_eq!(store.cloud_draft("account", "personal").unwrap(), None);
            drop(store);
            drop(directory);
            Ok(())
        })
        .await
        .unwrap();
    }
}
