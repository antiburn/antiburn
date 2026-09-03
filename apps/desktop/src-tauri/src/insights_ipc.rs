//! Deduplicates and cancels insights report reductions for the IPC surface.
//!
//! One report reduction runs at a time. A request that arrives while a
//! reduction runs awaits that same reduction; it never starts a second
//! one and it never cancels the first. Cancellation is a separate,
//! explicit signal — [`InsightsController::cancel`] — because request
//! identity must not stand in for it. The reduction reads one database
//! snapshot and writes nothing, so a cancelled run cannot corrupt the
//! durable evidence state.

use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use tokio::sync::watch;

use crate::insights_report::{self, ReducedReport, ReportRequest, reduce_report};

/// The stable error string a cancelled report crosses the IPC edge with.
pub const REPORT_CANCELLED_ERROR: &str = "insights report cancelled";

const NO_RESULT_ERROR: &str = "insights report task ended without a result";

/// One in-flight or finished report run.
struct Run {
    cancel: Arc<AtomicBool>,
    done: watch::Receiver<bool>,
    outcome: OnceLock<Result<ReducedReport, String>>,
}

impl Run {
    fn finished(&self) -> bool {
        *self.done.borrow()
    }
}

/// Owns the single report slot behind the insights IPC commands.
#[derive(Default)]
pub struct InsightsController {
    slot: Mutex<Option<Arc<Run>>>,
    consumers: Mutex<Consumers>,
    #[cfg(test)]
    cancel_requests: std::sync::atomic::AtomicUsize,
}

#[derive(Default)]
struct Consumers {
    settings_active: bool,
    checks: Option<String>,
}

impl InsightsController {
    /// True while a report reduction runs.
    pub fn is_calculating(&self) -> bool {
        self.lock_slot().as_ref().is_some_and(|run| !run.finished())
    }

    /// Sets the cancel flag of the running reduction, when one runs.
    ///
    /// This is the only cancellation signal. A new report request joins
    /// the running reduction instead of cancelling it.
    pub fn cancel(&self) {
        #[cfg(test)]
        self.cancel_requests.fetch_add(1, Ordering::SeqCst);
        if let Some(run) = self.lock_slot().as_ref() {
            run.cancel.store(true, Ordering::SeqCst);
        }
    }

    pub fn release_settings(&self) {
        let mut consumers = self.lock_consumers();
        consumers.settings_active = false;
        self.cancel_if_unused(&consumers);
    }

    pub fn release_checks(&self, consumer_id: &str) {
        let mut consumers = self.lock_consumers();
        if consumers.checks.as_deref() == Some(consumer_id) {
            consumers.checks = None;
        }
        self.cancel_if_unused(&consumers);
    }

    fn cancel_if_unused(&self, consumers: &Consumers) {
        if !consumers.settings_active && consumers.checks.is_none() {
            self.cancel();
        }
    }

    #[cfg(test)]
    pub(crate) fn cancel_requests(&self) -> usize {
        self.cancel_requests.load(Ordering::SeqCst)
    }

    /// Resolves one report, sharing the running reduction when one runs.
    pub async fn report(
        &self,
        data_dir: PathBuf,
        request: ReportRequest,
    ) -> Result<ReducedReport, String> {
        self.report_with(request, move |request, cancel| {
            reduce_report(data_dir, request, cancel)
        })
        .await
    }

    pub async fn settings_report(
        &self,
        data_dir: PathBuf,
        request: ReportRequest,
    ) -> Result<ReducedReport, String> {
        self.lock_consumers().settings_active = true;
        self.report(data_dir, request).await
    }

    pub async fn checks_report(
        &self,
        data_dir: PathBuf,
        request: ReportRequest,
        consumer_id: String,
    ) -> Result<ReducedReport, String> {
        self.lock_consumers().checks = Some(consumer_id);
        self.report(data_dir, request).await
    }

    async fn report_with<F, Fut>(
        &self,
        request: ReportRequest,
        reduce: F,
    ) -> Result<ReducedReport, String>
    where
        F: FnOnce(ReportRequest, Arc<AtomicBool>) -> Fut,
        Fut: Future<Output = anyhow::Result<ReducedReport>> + Send + 'static,
    {
        let run = {
            let mut slot = self.lock_slot();
            match slot.as_ref() {
                // Deduplication: this request awaits the reduction that
                // already runs. Its own reducer is never invoked. A run
                // with the cancel flag set is not joined: that flag came
                // from a caller that already left, and a fresh request
                // must not inherit its cancellation.
                Some(run) if !run.finished() && !run.cancel.load(Ordering::SeqCst) => {
                    Arc::clone(run)
                }
                _ => {
                    let cancel = Arc::new(AtomicBool::new(false));
                    let (done_tx, done_rx) = watch::channel(false);
                    let run = Arc::new(Run {
                        cancel: Arc::clone(&cancel),
                        done: done_rx,
                        outcome: OnceLock::new(),
                    });
                    let task_run = Arc::clone(&run);
                    let future = reduce(request, cancel);
                    tokio::spawn(async move {
                        let result = future.await.map_err(|error| {
                            if insights_report::is_cancelled(&error) {
                                REPORT_CANCELLED_ERROR.to_string()
                            } else {
                                error.to_string()
                            }
                        });
                        let _ = task_run.outcome.set(result);
                        let _ = done_tx.send(true);
                    });
                    *slot = Some(Arc::clone(&run));
                    run
                }
            }
        };

        let mut done = run.done.clone();
        if done.wait_for(|finished| *finished).await.is_err() {
            return Err(NO_RESULT_ERROR.to_string());
        }
        run.outcome
            .get()
            .cloned()
            .unwrap_or_else(|| Err(NO_RESULT_ERROR.to_string()))
    }

    fn lock_slot(&self) -> MutexGuard<'_, Option<Arc<Run>>> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_consumers(&self) -> MutexGuard<'_, Consumers> {
        self.consumers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use antiburn_local::insights::{
        CoverageCounts, EfficiencyReportAccumulator, ReportContext, ReportWindow,
    };

    use super::*;
    use crate::insights_report::ReportCancelled;

    fn request() -> ReportRequest {
        ReportRequest {
            environment_key: "native".to_owned(),
            window: ReportWindow {
                start_epoch: 0,
                end_epoch: 100,
            },
            computed_at_epoch: 100,
        }
    }

    fn empty_report(request: &ReportRequest) -> ReducedReport {
        ReducedReport {
            report: EfficiencyReportAccumulator::new().finish(ReportContext {
                environment_key: request.environment_key.clone(),
                window: request.window,
                computed_at_epoch: request.computed_at_epoch,
                parser_revision: 1,
                analyzer_revision: 1,
                evidence_schema_revision: 1,
                coverage: CoverageCounts::default(),
            }),
            evidence_settled: true,
        }
    }

    #[tokio::test]
    async fn concurrent_requests_share_one_reduction_and_do_not_cancel_it() {
        let controller = Arc::new(InsightsController::default());
        let reductions = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

        let first = {
            let controller = Arc::clone(&controller);
            let reductions = Arc::clone(&reductions);
            tokio::spawn(async move {
                controller
                    .report_with(request(), move |request, cancel| async move {
                        reductions.fetch_add(1, Ordering::SeqCst);
                        release_rx.await.unwrap();
                        // A second request while this runs must not set
                        // the cancel flag: identity is not cancellation.
                        assert!(!cancel.load(Ordering::SeqCst));
                        Ok(empty_report(&request))
                    })
                    .await
            })
        };
        while reductions.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        assert!(controller.is_calculating());

        let second = {
            let controller = Arc::clone(&controller);
            let reductions = Arc::clone(&reductions);
            tokio::spawn(async move {
                controller
                    .report_with(request(), move |request, cancel| async move {
                        reductions.fetch_add(1, Ordering::SeqCst);
                        assert!(!cancel.load(Ordering::SeqCst));
                        Ok(empty_report(&request))
                    })
                    .await
            })
        };
        // Let the second request reach the slot while the first still
        // holds it open, then release the shared reduction.
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        release_tx.send(()).unwrap();

        let first = first.await.unwrap().unwrap();
        let second = second.await.unwrap().unwrap();
        assert_eq!(first, second);
        assert_eq!(reductions.load(Ordering::SeqCst), 1, "one shared reduction");
        assert!(!controller.is_calculating());
    }

    #[tokio::test]
    async fn the_explicit_cancel_signal_cancels_the_running_report() {
        let controller = Arc::new(InsightsController::default());
        let started = Arc::new(AtomicBool::new(false));

        let task = {
            let controller = Arc::clone(&controller);
            let started = Arc::clone(&started);
            tokio::spawn(async move {
                controller
                    .report_with(request(), move |_request, cancel| async move {
                        started.store(true, Ordering::SeqCst);
                        while !cancel.load(Ordering::SeqCst) {
                            tokio::task::yield_now().await;
                        }
                        Err(anyhow::Error::new(ReportCancelled))
                    })
                    .await
            })
        };
        while !started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        assert!(controller.is_calculating());

        controller.cancel();

        let result = task.await.unwrap();
        assert_eq!(result.unwrap_err(), REPORT_CANCELLED_ERROR);
        assert!(!controller.is_calculating());
    }

    #[tokio::test]
    async fn a_request_after_a_cancel_does_not_join_the_cancelled_run() {
        let controller = Arc::new(InsightsController::default());
        let started = Arc::new(AtomicBool::new(false));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

        let doomed = {
            let controller = Arc::clone(&controller);
            let started = Arc::clone(&started);
            tokio::spawn(async move {
                controller
                    .report_with(request(), move |_request, _cancel| async move {
                        started.store(true, Ordering::SeqCst);
                        // Hold the cancelled reduction open so the second
                        // request arrives before it observes the flag.
                        release_rx.await.unwrap();
                        Err(anyhow::Error::new(ReportCancelled))
                    })
                    .await
            })
        };
        while !started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }

        controller.cancel();

        // The fresh request must not join the cancelled run: it starts
        // its own reduction and succeeds while the doomed run still runs.
        let fresh = controller
            .report_with(request(), move |request, cancel| async move {
                assert!(!cancel.load(Ordering::SeqCst));
                Ok(empty_report(&request))
            })
            .await;
        assert!(fresh.is_ok());

        release_tx.send(()).unwrap();
        let doomed = doomed.await.unwrap();
        assert_eq!(doomed.unwrap_err(), REPORT_CANCELLED_ERROR);
    }

    #[tokio::test]
    async fn a_request_after_a_finished_run_starts_a_fresh_reduction() {
        let controller = InsightsController::default();
        let reductions = Arc::new(AtomicUsize::new(0));

        for _ in 0..2 {
            let reductions = Arc::clone(&reductions);
            let result = controller
                .report_with(request(), move |request, _cancel| async move {
                    reductions.fetch_add(1, Ordering::SeqCst);
                    Ok(empty_report(&request))
                })
                .await;
            assert!(result.is_ok());
        }

        assert_eq!(reductions.load(Ordering::SeqCst), 2);
        assert!(!controller.is_calculating());
    }

    #[test]
    fn one_consumer_cannot_cancel_a_reduction_needed_by_the_other() {
        let controller = InsightsController::default();
        controller.lock_consumers().checks = Some("checks-1".to_string());

        controller.release_settings();
        assert_eq!(controller.cancel_requests(), 0);

        controller.release_checks("checks-1");
        assert_eq!(controller.cancel_requests(), 1);
    }

    #[test]
    fn an_old_checks_release_cannot_cancel_a_new_consumer() {
        let controller = InsightsController::default();
        controller.lock_consumers().checks = Some("new".to_string());

        controller.release_checks("old");
        assert_eq!(controller.cancel_requests(), 0);
    }
}
