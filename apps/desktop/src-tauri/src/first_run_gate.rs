//! The first-run gate: the backend half of "the work waits for Next".
//!
//! The first-run Overview shows one step at a time, and the reader's own
//! button press is what lets that step's work start. This module holds the
//! stage that press raises, and the wait every gated worker (the scan pass,
//! the evidence worker, the automatic historical pass) uses to block until
//! the reader's own progress reaches it.
//!
//! The stage lives in memory only. [`crate::commands::advance_first_run`] is
//! the only way the frontend raises it; nothing lowers it except the
//! debug-only [`FirstRunGate::reset`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::watch;

/// One step of the first run, in the order the reader moves through it.
///
/// `Ord` lets [`FirstRunGate::advance`] raise the stage without ever
/// lowering it: a command that names an earlier stage than the gate already
/// holds is a no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirstRunStage {
    /// The welcome step. Nothing runs yet.
    Welcome,
    Agents,
    Sessions,
    Checks,
    Done,
}

/// How often [`FirstRunGate::wait_until`] re-checks `cancelled` while the
/// stage it waits for has not arrived. The scan's own cancel flag is a plain
/// `AtomicBool` with no notification, so polling here is the only way a
/// cancel during a wait is ever noticed.
const CANCEL_POLL: Duration = Duration::from_millis(250);

/// Shared first-run stage, managed as Tauri state.
///
/// A cheap handle: cloning shares the same channel, the same way
/// [`crate::store::Store`] shares its connection.
#[derive(Clone)]
pub struct FirstRunGate {
    stage: watch::Sender<FirstRunStage>,
    /// Set when the evidence backlog first drains at or after
    /// [`FirstRunStage::Checks`]. Before that, the first run has no
    /// published turn rows.
    turns_published: Arc<AtomicBool>,
}

impl FirstRunGate {
    /// `onboarding_completed` is read once, at launch: `true` starts the
    /// gate at [`FirstRunStage::Done`], an existing install with nothing to
    /// gate. `false` starts it at [`FirstRunStage::Welcome`], a fresh first
    /// run. A relaunch during a first run therefore starts the gate, and the
    /// frontend, over from the welcome step.
    pub fn new(onboarding_completed: bool) -> Self {
        let initial = if onboarding_completed {
            FirstRunStage::Done
        } else {
            FirstRunStage::Welcome
        };
        let (stage, _receiver) = watch::channel(initial);
        Self {
            stage,
            turns_published: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The stage right now.
    pub fn stage(&self) -> FirstRunStage {
        *self.stage.borrow()
    }

    /// Raise the stage to `max(current, stage)`.
    pub fn advance(&self, stage: FirstRunStage) {
        self.stage.send_if_modified(|current| {
            let raised = stage > *current;
            if raised {
                *current = stage;
            }
            raised
        });
    }

    /// Debug-only: return the gate to [`FirstRunStage::Welcome`], for the
    /// "return to a new install" debug tool.
    pub fn reset(&self) {
        self.stage.send_replace(FirstRunStage::Welcome);
        self.turns_published.store(false, Ordering::SeqCst);
    }

    /// Record that the evidence backlog drained. Returns `true` only for the
    /// first drain at or after [`FirstRunStage::Checks`].
    pub fn mark_turns_published(&self) -> bool {
        self.stage() >= FirstRunStage::Checks && !self.turns_published.swap(true, Ordering::SeqCst)
    }

    /// Whether limit factor learning can price turns now.
    ///
    /// Learning keeps a sample whose interval closed more than a few minutes
    /// ago and does not compute it again. A sample that learning takes
    /// before the first run publishes its turns has no dollars, and it stays
    /// that way. So during the first run, learning waits for
    /// [`FirstRunGate::mark_turns_published`].
    pub fn limit_learning_ready(&self) -> bool {
        self.stage() == FirstRunStage::Done || self.turns_published.load(Ordering::SeqCst)
    }

    /// Mark the first run finished. [`crate::commands::finish_first_run`]
    /// calls this once its save actually turns onboarding on.
    pub fn finish(&self) {
        self.stage.send_replace(FirstRunStage::Done);
    }

    /// Wait until the stage reaches `stage`, or `cancelled()` turns true.
    ///
    /// Returns `true` as soon as the stage is at or past `stage` — at once,
    /// if it already is. Returns `false` as soon as `cancelled()` answers
    /// true first. The stage channel wakes this at once on every advance;
    /// `cancelled` has no such signal, so this also polls it every
    /// [`CANCEL_POLL`] while it waits.
    pub async fn wait_until(&self, stage: FirstRunStage, cancelled: impl Fn() -> bool) -> bool {
        let mut receiver = self.stage.subscribe();
        loop {
            if *receiver.borrow() >= stage {
                return true;
            }
            if cancelled() {
                return false;
            }
            tokio::select! {
                changed = receiver.changed() => {
                    // An error means the sender dropped, so the stage can
                    // never advance further. Treat it as a cancel.
                    if changed.is_err() {
                        return false;
                    }
                }
                () = tokio::time::sleep(CANCEL_POLL) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_fresh_install_starts_at_welcome_and_a_completed_one_at_done() {
        assert_eq!(FirstRunGate::new(false).stage(), FirstRunStage::Welcome);
        assert_eq!(FirstRunGate::new(true).stage(), FirstRunStage::Done);
    }

    #[test]
    fn advance_is_monotonic() {
        let gate = FirstRunGate::new(false);
        gate.advance(FirstRunStage::Sessions);
        assert_eq!(gate.stage(), FirstRunStage::Sessions);
        // An earlier stage than the gate already holds is a no-op.
        gate.advance(FirstRunStage::Agents);
        assert_eq!(gate.stage(), FirstRunStage::Sessions);
        gate.advance(FirstRunStage::Checks);
        assert_eq!(gate.stage(), FirstRunStage::Checks);
    }

    #[test]
    fn limit_learning_waits_for_the_first_drain_at_checks() {
        let gate = FirstRunGate::new(false);
        assert!(!gate.limit_learning_ready());
        // A drain before the checks step does not count: the evidence worker
        // has not published the first run's turns yet.
        assert!(!gate.mark_turns_published());
        gate.advance(FirstRunStage::Checks);
        assert!(!gate.limit_learning_ready());
        assert!(gate.mark_turns_published());
        assert!(gate.limit_learning_ready());
        // Only the first drain reports true.
        assert!(!gate.mark_turns_published());
        gate.reset();
        assert!(!gate.limit_learning_ready());
        assert!(FirstRunGate::new(true).limit_learning_ready());
    }

    #[test]
    fn reset_returns_to_welcome_and_finish_jumps_to_done() {
        let gate = FirstRunGate::new(false);
        gate.advance(FirstRunStage::Checks);
        gate.finish();
        assert_eq!(gate.stage(), FirstRunStage::Done);
        gate.reset();
        assert_eq!(gate.stage(), FirstRunStage::Welcome);
    }

    #[tokio::test]
    async fn wait_until_returns_at_once_when_the_stage_is_already_open() {
        let gate = FirstRunGate::new(false);
        gate.advance(FirstRunStage::Sessions);
        let opened = tokio::time::timeout(
            Duration::from_millis(50),
            gate.wait_until(FirstRunStage::Agents, || false),
        )
        .await
        .expect("resolves without waiting");
        assert!(opened);
    }

    #[tokio::test]
    async fn wait_until_returns_true_once_a_later_advance_opens_it() {
        let gate = FirstRunGate::new(false);
        let waiter = gate.clone();
        let waiting =
            tokio::spawn(async move { waiter.wait_until(FirstRunStage::Sessions, || false).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        gate.advance(FirstRunStage::Sessions);
        assert!(waiting.await.expect("the wait task does not panic"));
    }

    #[tokio::test]
    async fn wait_until_observes_cancellation_during_the_wait() {
        let gate = FirstRunGate::new(false);
        let cancelled = Arc::new(AtomicBool::new(false));
        let polls = Arc::new(AtomicUsize::new(0));
        let waiting = {
            let gate = gate.clone();
            let cancelled = Arc::clone(&cancelled);
            let polls = Arc::clone(&polls);
            tokio::spawn(async move {
                gate.wait_until(FirstRunStage::Sessions, || {
                    polls.fetch_add(1, Ordering::SeqCst);
                    cancelled.load(Ordering::SeqCst)
                })
                .await
            })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            while polls.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the gate checks cancellation again while waiting");
        cancelled.store(true, Ordering::SeqCst);
        let opened = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("the gate notices cancellation")
            .expect("the wait task does not panic");
        assert!(!opened);
        assert!(polls.load(Ordering::SeqCst) >= 2);
    }
}
