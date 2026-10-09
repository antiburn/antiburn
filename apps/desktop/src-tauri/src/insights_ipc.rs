//! Deduplicates and cancels insights report reductions for the IPC surface.
//!
//! One report reduction runs at a time. A reduction reads one database
//! snapshot when it starts, so a request joins a run only before that run
//! starts to read. A request that arrives while a reduction reads queues one
//! follow-up reduction, which starts when the first one finishes. Every
//! request that arrives before the follow-up starts shares it. So each caller
//! gets a snapshot from after its request, and a change that lands during a
//! reduction is never hidden behind that reduction's older answer. A request
//! never cancels a run. Cancellation is a separate,
//! explicit signal — [`InsightsController::cancel`] — because request
//! identity must not stand in for it. The reduction reads one database
//! snapshot and writes nothing, so a cancelled run cannot corrupt the
//! durable evidence state.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use tokio::sync::watch;

use antiburn_local::insights::DetectorSelection;

use crate::insights_report::{self, ReducedReport, ReportRequest, reduce_report_with_selection};

/// The stable error string a cancelled report crosses the IPC edge with.
pub const REPORT_CANCELLED_ERROR: &str = "insights report cancelled";

const NO_RESULT_ERROR: &str = "insights report task ended without a result";

/// One in-flight or finished report run.
struct Run {
    cancel: Arc<AtomicBool>,
    /// Set when the reduction starts to read its snapshot. A request joins
    /// the run only before this.
    started: Arc<AtomicBool>,
    done: watch::Receiver<bool>,
    outcome: OnceLock<Result<ReducedReport, String>>,
    check_preferences_revision: u64,
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
    next_consumer_request: AtomicU64,
    #[cfg(test)]
    cancel_requests: std::sync::atomic::AtomicUsize,
}

#[derive(Default)]
struct Consumers {
    checks: HashMap<u64, String>,
}

impl InsightsController {
    /// True while a report reduction runs.
    #[cfg(test)]
    fn is_calculating(&self) -> bool {
        self.lock_slot().as_ref().is_some_and(|run| !run.finished())
    }

    /// Sets the cancel flag of the running reduction, when one runs.
    ///
    /// This is the only cancellation signal. A new report request joins
    /// the running reduction instead of cancelling it.
    pub fn cancel(&self) {
        let slot = self.lock_slot();
        if let Some(run) = slot.as_ref().filter(|run| !run.finished()) {
            #[cfg(test)]
            self.cancel_requests.fetch_add(1, Ordering::SeqCst);
            run.cancel.store(true, Ordering::SeqCst);
        }
    }

    pub fn release_checks(&self, consumer_id: &str) {
        let mut consumers = self.lock_consumers();
        let count = consumers.checks.len();
        consumers.checks.retain(|_, id| id != consumer_id);
        let released = consumers.checks.len() < count;
        if released && consumers.checks.is_empty() {
            self.cancel();
        }
    }

    fn register_checks(&self, consumer_id: String) -> ConsumerRequest<'_> {
        let request_id = self
            .next_consumer_request
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        self.lock_consumers()
            .checks
            .insert(request_id, consumer_id.clone());
        ConsumerRequest {
            controller: self,
            consumer_id,
            request_id,
        }
    }

    fn unregister_checks(&self, consumer_id: &str, request_id: u64) {
        let mut consumers = self.lock_consumers();
        let removed = consumers
            .checks
            .get(&request_id)
            .is_some_and(|id| id == consumer_id)
            && consumers.checks.remove(&request_id).is_some();
        if removed && consumers.checks.is_empty() {
            self.cancel();
        }
    }

    #[cfg(test)]
    pub(crate) fn cancel_requests(&self) -> usize {
        self.cancel_requests.load(Ordering::SeqCst)
    }

    pub async fn checks_report(
        &self,
        data_dir: PathBuf,
        request: ReportRequest,
        consumer_id: String,
        enabled_detectors: DetectorSelection,
        check_preferences_revision: u64,
    ) -> Result<ReducedReport, String> {
        self.report_for_consumer(
            consumer_id,
            request,
            check_preferences_revision,
            move |request, cancel| {
                reduce_report_with_selection(data_dir, request, cancel, enabled_detectors)
            },
        )
        .await
    }

    async fn report_for_consumer<F, Fut>(
        &self,
        consumer_id: String,
        request: ReportRequest,
        check_preferences_revision: u64,
        reduce: F,
    ) -> Result<ReducedReport, String>
    where
        F: FnOnce(ReportRequest, Arc<AtomicBool>) -> Fut,
        Fut: Future<Output = anyhow::Result<ReducedReport>> + Send + 'static,
    {
        let _consumer_request = self.register_checks(consumer_id);
        self.report_with(request, check_preferences_revision, reduce)
            .await
    }

    async fn report_with<F, Fut>(
        &self,
        request: ReportRequest,
        check_preferences_revision: u64,
        reduce: F,
    ) -> Result<ReducedReport, String>
    where
        F: FnOnce(ReportRequest, Arc<AtomicBool>) -> Fut,
        Fut: Future<Output = anyhow::Result<ReducedReport>> + Send + 'static,
    {
        let run = {
            let mut slot = self.lock_slot();
            // Deduplication: this request awaits a run that has not started
            // to read yet. Its own reducer is never invoked. A run with the
            // cancel flag set is not joined: that flag came from a caller
            // that already left, and a fresh request must not inherit its
            // cancellation.
            let joinable = slot
                .as_ref()
                .filter(|run| {
                    !run.finished()
                        && !run.cancel.load(Ordering::SeqCst)
                        && !run.started.load(Ordering::SeqCst)
                        && run.check_preferences_revision == check_preferences_revision
                })
                .cloned();
            match joinable {
                Some(run) => run,
                None => {
                    // A run that already reads cannot see changes made after
                    // it started. Queue behind it instead of joining it. Do
                    // not queue behind a cancelled run: it stops soon and
                    // its answer has no reader.
                    let previous = slot
                        .as_ref()
                        .filter(|run| !run.finished() && !run.cancel.load(Ordering::SeqCst))
                        .map(|run| run.done.clone());
                    let cancel = Arc::new(AtomicBool::new(false));
                    let started = Arc::new(AtomicBool::new(false));
                    let (done_tx, done_rx) = watch::channel(false);
                    let run = Arc::new(Run {
                        cancel: Arc::clone(&cancel),
                        started: Arc::clone(&started),
                        done: done_rx,
                        outcome: OnceLock::new(),
                        check_preferences_revision,
                    });
                    let task_run = Arc::clone(&run);
                    let future = reduce(request, cancel);
                    tokio::spawn(async move {
                        if let Some(mut previous) = previous {
                            let _ = previous.wait_for(|finished| *finished).await;
                        }
                        started.store(true, Ordering::SeqCst);
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

struct ConsumerRequest<'a> {
    controller: &'a InsightsController,
    consumer_id: String,
    request_id: u64,
}

impl Drop for ConsumerRequest<'_> {
    fn drop(&mut self) {
        self.controller
            .unregister_checks(&self.consumer_id, self.request_id);
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
            pending_evidence: 0,
            deferred_evidence: 0,
            resources: crate::insights_report::ResourceAssessment::default(),
            enabled_detectors: antiburn_local::insights::DetectorSelection::all(),
            check_progress: Default::default(),
            sampled_instructions: false,
        }
    }

    #[tokio::test]
    async fn a_request_during_a_reduction_queues_one_fresh_reduction_and_cancels_nothing() {
        let controller = Arc::new(InsightsController::default());
        let reductions = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

        let first = {
            let controller = Arc::clone(&controller);
            let reductions = Arc::clone(&reductions);
            tokio::spawn(async move {
                controller
                    .report_with(request(), 0, move |request, cancel| async move {
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

        // Two requests arrive while the first reduction reads. Its snapshot
        // predates them, so they share one follow-up reduction instead.
        let later = (0..2)
            .map(|_| {
                let controller = Arc::clone(&controller);
                let reductions = Arc::clone(&reductions);
                tokio::spawn(async move {
                    controller
                        .report_with(request(), 0, move |request, cancel| async move {
                            reductions.fetch_add(1, Ordering::SeqCst);
                            assert!(!cancel.load(Ordering::SeqCst));
                            Ok(empty_report(&request))
                        })
                        .await
                })
            })
            .collect::<Vec<_>>();
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            reductions.load(Ordering::SeqCst),
            1,
            "the follow-up waits for the running reduction"
        );
        release_tx.send(()).unwrap();

        first.await.unwrap().unwrap();
        for task in later {
            task.await.unwrap().unwrap();
        }
        assert_eq!(
            reductions.load(Ordering::SeqCst),
            2,
            "one running reduction plus one shared follow-up"
        );
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
                    .report_with(request(), 0, move |_request, cancel| async move {
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
                    .report_with(request(), 0, move |_request, _cancel| async move {
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
            .report_with(request(), 0, move |request, cancel| async move {
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
                .report_with(request(), 0, move |request, _cancel| async move {
                    reductions.fetch_add(1, Ordering::SeqCst);
                    Ok(empty_report(&request))
                })
                .await;
            assert!(result.is_ok());
        }

        assert_eq!(reductions.load(Ordering::SeqCst), 2);
        assert!(!controller.is_calculating());
    }

    #[tokio::test]
    async fn a_new_check_preference_revision_does_not_join_an_older_queued_report() {
        let controller = Arc::new(InsightsController::default());
        let reductions = Arc::new(AtomicUsize::new(0));
        let (release_base_tx, release_base_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_old_tx, release_old_rx) = tokio::sync::oneshot::channel::<()>();

        let base = {
            let controller = Arc::clone(&controller);
            let reductions = Arc::clone(&reductions);
            tokio::spawn(async move {
                controller
                    .report_with(request(), 0, move |request, _cancel| async move {
                        reductions.fetch_add(1, Ordering::SeqCst);
                        release_base_rx.await.unwrap();
                        Ok(empty_report(&request))
                    })
                    .await
            })
        };
        while reductions.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }

        let old_revision = {
            let controller = Arc::clone(&controller);
            let reductions = Arc::clone(&reductions);
            tokio::spawn(async move {
                controller
                    .report_with(request(), 1, move |request, _cancel| async move {
                        reductions.fetch_add(1, Ordering::SeqCst);
                        release_old_rx.await.unwrap();
                        Ok(empty_report(&request))
                    })
                    .await
            })
        };
        while controller
            .lock_slot()
            .as_ref()
            .is_none_or(|run| run.check_preferences_revision != 1)
        {
            tokio::task::yield_now().await;
        }

        let new_revision = {
            let controller = Arc::clone(&controller);
            let reductions = Arc::clone(&reductions);
            tokio::spawn(async move {
                controller
                    .report_with(request(), 2, move |request, _cancel| async move {
                        reductions.fetch_add(1, Ordering::SeqCst);
                        Ok(empty_report(&request))
                    })
                    .await
            })
        };
        while controller
            .lock_slot()
            .as_ref()
            .is_none_or(|run| run.check_preferences_revision != 2)
        {
            tokio::task::yield_now().await;
        }

        release_base_tx.send(()).unwrap();
        base.await.unwrap().unwrap();
        while reductions.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
        release_old_tx.send(()).unwrap();
        old_revision.await.unwrap().unwrap();
        new_revision.await.unwrap().unwrap();
        assert_eq!(reductions.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_finished_checks_request_unregisters_its_consumer() {
        let controller = InsightsController::default();
        let report = controller
            .report_for_consumer(
                "checks-1".to_string(),
                request(),
                0,
                |request, _cancel| async move { Ok(empty_report(&request)) },
            )
            .await;
        assert!(report.is_ok());

        controller.release_checks("checks-1");
        assert_eq!(controller.cancel_requests(), 0);
    }

    #[tokio::test]
    async fn releasing_one_checks_consumer_keeps_the_other_consumers_report() {
        let controller = Arc::new(InsightsController::default());
        let reductions = Arc::new(AtomicUsize::new(0));
        let base_started = Arc::new(AtomicBool::new(false));
        let (release_base_tx, release_base_rx) = tokio::sync::oneshot::channel::<()>();

        let base = {
            let controller = Arc::clone(&controller);
            let reductions = Arc::clone(&reductions);
            let base_started = Arc::clone(&base_started);
            tokio::spawn(async move {
                controller
                    .report_with(request(), 0, move |request, cancel| async move {
                        reductions.fetch_add(1, Ordering::SeqCst);
                        base_started.store(true, Ordering::SeqCst);
                        release_base_rx.await.unwrap();
                        assert!(!cancel.load(Ordering::SeqCst));
                        Ok(empty_report(&request))
                    })
                    .await
            })
        };
        while !base_started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }

        let overview = {
            let controller = Arc::clone(&controller);
            let reductions = Arc::clone(&reductions);
            tokio::spawn(async move {
                controller
                    .report_for_consumer(
                        "overview".to_string(),
                        request(),
                        0,
                        move |request, cancel| async move {
                            reductions.fetch_add(1, Ordering::SeqCst);
                            assert!(!cancel.load(Ordering::SeqCst));
                            Ok(empty_report(&request))
                        },
                    )
                    .await
            })
        };
        while !controller
            .lock_consumers()
            .checks
            .values()
            .any(|id| id == "overview")
        {
            tokio::task::yield_now().await;
        }
        assert!(
            controller
                .lock_slot()
                .as_ref()
                .is_some_and(|run| !run.started.load(Ordering::SeqCst)),
            "the overview waits on a fresh report behind the base run"
        );

        let popover = {
            let controller = Arc::clone(&controller);
            let reductions = Arc::clone(&reductions);
            tokio::spawn(async move {
                controller
                    .report_for_consumer(
                        "popover".to_string(),
                        request(),
                        0,
                        move |request, cancel| async move {
                            reductions.fetch_add(1, Ordering::SeqCst);
                            assert!(!cancel.load(Ordering::SeqCst));
                            Ok(empty_report(&request))
                        },
                    )
                    .await
            })
        };
        while !controller
            .lock_consumers()
            .checks
            .values()
            .any(|id| id == "popover")
        {
            tokio::task::yield_now().await;
        }
        tokio::task::yield_now().await;

        // Both surfaces joined the queued report. Closing the popover must
        // keep it alive for the overview's still-active request.
        controller.release_checks("popover");
        assert_eq!(controller.cancel_requests(), 0);
        release_base_tx.send(()).unwrap();
        base.await.unwrap().unwrap();
        overview.await.unwrap().unwrap();
        popover.await.unwrap().unwrap();
        assert_eq!(reductions.load(Ordering::SeqCst), 2);

        controller.release_checks("overview");
        assert_eq!(controller.cancel_requests(), 0);
    }

    #[tokio::test]
    async fn overlapping_requests_with_one_consumer_release_independently() {
        let controller = Arc::new(InsightsController::default());
        let first_started = Arc::new(AtomicBool::new(false));
        let second_started = Arc::new(AtomicBool::new(false));
        let (release_first_tx, release_first_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_second_tx, release_second_rx) = tokio::sync::oneshot::channel::<()>();

        let first = {
            let controller = Arc::clone(&controller);
            let first_started = Arc::clone(&first_started);
            tokio::spawn(async move {
                controller
                    .report_for_consumer(
                        "same-surface".to_string(),
                        request(),
                        0,
                        move |request, cancel| async move {
                            first_started.store(true, Ordering::SeqCst);
                            release_first_rx.await.unwrap();
                            assert!(!cancel.load(Ordering::SeqCst));
                            Ok(empty_report(&request))
                        },
                    )
                    .await
            })
        };
        while !first_started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }

        let second = {
            let controller = Arc::clone(&controller);
            let second_started = Arc::clone(&second_started);
            tokio::spawn(async move {
                controller
                    .report_for_consumer(
                        "same-surface".to_string(),
                        request(),
                        0,
                        move |request, cancel| async move {
                            second_started.store(true, Ordering::SeqCst);
                            release_second_rx.await.unwrap();
                            if cancel.load(Ordering::SeqCst) {
                                return Err(anyhow::Error::new(ReportCancelled));
                            }
                            Ok(empty_report(&request))
                        },
                    )
                    .await
            })
        };
        while controller
            .lock_slot()
            .as_ref()
            .is_none_or(|run| run.started.load(Ordering::SeqCst))
        {
            tokio::task::yield_now().await;
        }

        release_first_tx.send(()).unwrap();
        first.await.unwrap().unwrap();
        while !second_started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }

        // The first call completed and unregistered its token. The second
        // call under the same surface ID must remain releasable.
        controller.release_checks("same-surface");
        release_second_tx.send(()).unwrap();
        assert_eq!(second.await.unwrap().unwrap_err(), REPORT_CANCELLED_ERROR);
        assert_eq!(controller.cancel_requests(), 1);
    }
}
