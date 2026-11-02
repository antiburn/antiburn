//! The dedicated historical pass, and the progress indicator built from it.
//!
//! A routine pass only ever discovers the current window (see the `scan::mod`
//! module doc and [`super::CURRENT_WINDOW_SECS`]). Retention can promise far
//! more than that, so one dedicated pass widens discovery to the retention
//! limit: the reader's own "Older sessions" Scan now button
//! (`crate::commands::scan_history`) asks for it directly through
//! [`super::ScanTrigger::HistoricalScan`], and [`maybe_start_automatic_pass`]
//! asks for it once automatically, after the first current pass finishes.
//! Evidence processing does not delay historical enrollment.

use tauri::{AppHandle, Emitter, Manager};

use crate::dto::{ScanHistoryProgress, ScanHistoryState};
use crate::first_run_gate::{FirstRunGate, FirstRunStage};
use crate::store::Store;
use crate::store::model::{CURRENT_WINDOW_DAYS, RETAIN_SESSION_DATA_FOREVER};

use super::{EVENT_PROGRESS, ScanController, ScanTrigger, unix_now};

/// Marks, in the store, which retention the historical pass last covered.
/// Empty (or absent) means "not done for any retention yet". Compared
/// against the live retention on every read, so a widened retention asks
/// again.
const HISTORY_DONE_RETENTION_KEY: &str = "internal:historyDoneForRetentionDays";

/// The age limit the historical pass applies, in seconds, or `None` when the
/// current retention leaves nothing for it to do.
///
/// Forever retention has no age limit: `now` makes discovery's cutoff zero,
/// the same "no limit" value a routine pass under unlimited retention would
/// use. A retention at or inside [`CURRENT_WINDOW_DAYS`] keeps nothing a
/// current pass does not already cover, so there is nothing left for a
/// historical pass to add.
pub(crate) fn window_secs(retention_days: i32, now: i64) -> Option<i64> {
    if retention_days == RETAIN_SESSION_DATA_FOREVER {
        return Some(now);
    }
    if retention_days > i32::try_from(CURRENT_WINDOW_DAYS).unwrap_or(i32::MAX) {
        return Some(i64::from(retention_days) * 86_400);
    }
    None
}

/// Record that the historical pass just covered `retention_days`. Read back
/// by [`compute`] and [`maybe_start_automatic_pass`], so neither asks again
/// until retention widens past it.
pub(crate) fn mark_done(store: &Store, retention_days: i32) {
    store.set_internal_value(HISTORY_DONE_RETENTION_KEY, &retention_days.to_string());
}

/// Forget which retention the historical pass covered. Called when the
/// local index is cleared, so the fresh index earns its own historical pass
/// rather than reading the old retention's completion as still current.
pub(crate) fn reset_done(store: &Store) {
    store.set_internal_value(HISTORY_DONE_RETENTION_KEY, "");
}

fn done_for_current_retention(store: &Store, retention_days: i32) -> bool {
    store.internal_value(HISTORY_DONE_RETENTION_KEY).as_deref()
        == Some(retention_days.to_string().as_str())
}

/// Compute the live progress for [`crate::dto::ScanStatus::history`].
///
/// `pass_running` is whether the historical pass's own discovery or
/// describe phase is in flight right now, which this cannot tell from the
/// store alone — it is what separates "pending" (not started) from
/// "running" (its first pass under this retention, under way) before either
/// has written a completion marker.
pub(crate) fn compute(store: &Store, now: i64, pass_running: bool) -> ScanHistoryProgress {
    let retention_days = store.settings_snapshot().session_data_retention_days;
    if window_secs(retention_days, now).is_none() {
        return ScanHistoryProgress {
            state: ScanHistoryState::None,
            completed: 0,
            total: 0,
            pass_running: false,
        };
    }
    let cutoff = now - i64::from(CURRENT_WINDOW_DAYS) * 86_400;
    let (total, completed) = store
        .history_progress_counts(&crate::agents::evidence_cohort(), cutoff)
        .unwrap_or((0, 0));
    let done = done_for_current_retention(store, retention_days);
    let state = if pass_running {
        ScanHistoryState::Running
    } else if !done {
        ScanHistoryState::Pending
    } else if completed >= total {
        ScanHistoryState::Done
    } else {
        ScanHistoryState::Running
    };
    ScanHistoryProgress {
        state,
        completed,
        total,
        pass_running,
    }
}

/// Push the live history progress onto [`crate::dto::ScanStatus`], and emit
/// it through the scan progress event.
///
/// The worker calls this after each analysed session, so an unforced call
/// runs at most about once a second: the count query holds the store lock.
/// Use `force` at the edges (a pass starts or ends, the backlog drains), so
/// the last state always reaches the reader.
pub(crate) fn push_progress(app: &AppHandle, force: bool) {
    let controller = app.state::<ScanController>();
    if !controller.throttle_history_emit() && !force {
        return;
    }
    let store = app.state::<Store>();
    let progress = compute(&store, unix_now(), controller.history_pass_running());
    let status = controller.update(|status| status.history = Some(progress));
    let _ = app.emit(EVENT_PROGRESS, status);
}

/// Ask the scheduler for the one-time automatic historical pass, if the
/// current retention warrants one, the first current pass has already
/// finished, onboarding is complete, and history is not already done for
/// this retention.
///
/// Safe to call repeatedly: once the completion marker matches the live
/// retention, every later call is a no-op. It asks at most once for each
/// retention in a launch, so a failed or cancelled pass waits for the next
/// launch or the reader's own "Older sessions" scan.
pub(crate) fn maybe_start_automatic_pass(app: &AppHandle) {
    request_automatic_pass(
        &app.state::<FirstRunGate>(),
        &app.state::<ScanController>(),
        &app.state::<Store>(),
    );
}

fn request_automatic_pass(gate: &FirstRunGate, controller: &ScanController, store: &Store) {
    // The first run's own steps own discovery until it finishes. The
    // reader's own "Older sessions" Scan now button is not gated — see
    // `crate::commands::scan_history`, which calls `run_pass` directly.
    if gate.stage() != FirstRunStage::Done {
        return;
    }
    if !controller.first_current_pass_done() {
        return;
    }
    let retention_days = store.settings_snapshot().session_data_retention_days;
    if window_secs(retention_days, unix_now()).is_none() {
        return;
    }
    if done_for_current_retention(store, retention_days) {
        return;
    }
    if !controller.claim_history_auto_request(retention_days) {
        return;
    }
    controller.request(ScanTrigger::HistoricalScan);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forever_retention_has_no_age_limit() {
        assert_eq!(window_secs(RETAIN_SESSION_DATA_FOREVER, 1_000), Some(1_000));
    }

    #[test]
    fn ninety_day_retention_widens_to_ninety_days() {
        assert_eq!(window_secs(90, 1_000), Some(90 * 86_400));
    }

    #[test]
    fn a_retention_at_or_inside_the_current_window_skips_the_pass() {
        assert_eq!(
            window_secs(i32::try_from(CURRENT_WINDOW_DAYS).unwrap(), 1_000),
            None
        );
        assert_eq!(window_secs(1, 1_000), None);
    }

    #[test]
    fn the_automatic_request_is_claimed_once_per_retention_until_reset() {
        let controller = ScanController::default();
        assert!(controller.claim_history_auto_request(90));
        assert!(!controller.claim_history_auto_request(90));
        // A widened retention asks again.
        assert!(controller.claim_history_auto_request(RETAIN_SESSION_DATA_FOREVER));
        controller.reset_history_auto_request();
        assert!(controller.claim_history_auto_request(RETAIN_SESSION_DATA_FOREVER));
    }

    #[test]
    fn automatic_history_enrolls_during_an_active_backlog_without_taking_scan_ownership() {
        use std::sync::atomic::Ordering;

        let store = store();
        let gate = FirstRunGate::new(true);
        let controller = ScanController::default();
        let worker = crate::insights_worker::WorkerHandle::with_active_backlog_for_test(&store);
        assert!(worker.backlog_active());
        controller.mark_current_pass_done();
        assert!(super::super::on_demand_start(&controller));
        controller.request(ScanTrigger::SettingsTransition);

        request_automatic_pass(&gate, &controller, &store);

        assert!(worker.backlog_active());
        assert!(controller.running.load(Ordering::SeqCst));
        assert!(!super::super::on_demand_start(&controller));
        assert!(!controller.history_pass_running());
        assert!(matches!(
            controller.take_pending_trigger(),
            Some(ScanTrigger::HistoricalScan)
        ));

        request_automatic_pass(&gate, &controller, &store);
        assert!(controller.take_pending_trigger().is_none());
        assert!(worker.backlog_active());
    }

    #[test]
    fn automatic_history_preserves_enrollment_gates_without_consuming_the_request_claim() {
        let store = store();
        let gate = FirstRunGate::new(false);
        let controller = ScanController::default();
        controller.mark_current_pass_done();

        request_automatic_pass(&gate, &controller, &store);
        assert!(controller.take_pending_trigger().is_none());

        gate.finish();
        request_automatic_pass(&gate, &controller, &store);
        assert!(matches!(
            controller.take_pending_trigger(),
            Some(ScanTrigger::HistoricalScan)
        ));

        let controller = ScanController::default();
        request_automatic_pass(&gate, &controller, &store);
        assert!(controller.take_pending_trigger().is_none());

        controller.mark_current_pass_done();
        let mut settings = store.settings().unwrap();
        settings.session_data_retention_days = i32::try_from(CURRENT_WINDOW_DAYS).unwrap();
        store.save_settings(&settings).unwrap();
        request_automatic_pass(&gate, &controller, &store);
        assert!(controller.take_pending_trigger().is_none());

        settings.session_data_retention_days = 90;
        store.save_settings(&settings).unwrap();
        mark_done(&store, 90);
        request_automatic_pass(&gate, &controller, &store);
        assert!(controller.take_pending_trigger().is_none());

        reset_done(&store);
        request_automatic_pass(&gate, &controller, &store);
        assert!(matches!(
            controller.take_pending_trigger(),
            Some(ScanTrigger::HistoricalScan)
        ));

        settings.session_data_retention_days = RETAIN_SESSION_DATA_FOREVER;
        store.save_settings(&settings).unwrap();
        request_automatic_pass(&gate, &controller, &store);
        assert!(matches!(
            controller.take_pending_trigger(),
            Some(ScanTrigger::HistoricalScan)
        ));
        request_automatic_pass(&gate, &controller, &store);
        assert!(controller.take_pending_trigger().is_none());
    }

    #[test]
    fn the_throttle_allows_one_emit_then_blocks_until_a_second_passes() {
        let controller = ScanController::default();
        assert!(controller.throttle_history_emit());
        assert!(!controller.throttle_history_emit());
    }

    fn store() -> Store {
        let dir = tempfile::tempdir().unwrap();
        Store::open_in_memory(dir.path()).unwrap()
    }

    #[test]
    fn compute_reports_none_when_retention_covers_only_the_current_window() {
        let store = store();
        let mut settings = store.settings().unwrap();
        settings.session_data_retention_days = i32::try_from(CURRENT_WINDOW_DAYS).unwrap();
        store.save_settings(&settings).unwrap();
        assert_eq!(compute(&store, 10_000, false).state, ScanHistoryState::None);
    }

    #[test]
    fn compute_reports_pending_before_the_pass_has_run() {
        // The default retention is Forever, which warrants a historical pass.
        let store = store();
        assert_eq!(
            compute(&store, 10_000, false).state,
            ScanHistoryState::Pending
        );
    }

    #[test]
    fn compute_reports_running_while_the_pass_itself_is_in_flight() {
        let store = store();
        let progress = compute(&store, 10_000, true);
        assert_eq!(progress.state, ScanHistoryState::Running);
        assert!(progress.pass_running);
        assert!(!compute(&store, 10_000, false).pass_running);
    }

    #[test]
    fn compute_reports_done_once_marked_with_nothing_outstanding() {
        let store = store();
        let retention = store.settings_snapshot().session_data_retention_days;
        mark_done(&store, retention);
        let progress = compute(&store, 10_000, false);
        assert_eq!(progress.state, ScanHistoryState::Done);
        assert_eq!(progress.total, 0);
        assert_eq!(progress.completed, 0);
    }

    #[test]
    fn reset_done_undoes_mark_done() {
        let store = store();
        let retention = store.settings_snapshot().session_data_retention_days;
        mark_done(&store, retention);
        assert_eq!(compute(&store, 10_000, false).state, ScanHistoryState::Done);
        reset_done(&store);
        assert_eq!(
            compute(&store, 10_000, false).state,
            ScanHistoryState::Pending
        );
    }
}
