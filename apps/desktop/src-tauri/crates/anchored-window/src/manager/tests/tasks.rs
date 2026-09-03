use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::geometry::CursorProximity::{Inside, Outside, WithinTolerance};
use crate::lifecycle::ScheduledTask;

use super::super::AnchoredWindowManager;
use super::super::tasks::CursorExitTracker;
use super::fixtures::manager;

const BRIDGE_DELAY: Duration = Duration::from_millis(300);
const OUTSIDE_DELAY: Duration = Duration::from_millis(180);

fn tracker() -> CursorExitTracker {
    CursorExitTracker::new(BRIDGE_DELAY, OUTSIDE_DELAY)
}

fn elapsed(start: Instant, millis: u64) -> Instant {
    start + Duration::from_millis(millis)
}

#[test]
fn cursor_exit_without_entry_uses_only_the_bridge_delay() {
    let start = Instant::now();
    let mut tracker = tracker();

    assert!(!tracker.update(start, Some(Outside)));
    assert!(!tracker.update(elapsed(start, 299), Some(Outside)));
    assert!(tracker.update(elapsed(start, 300), Some(Outside)));
}

#[test]
fn tolerance_does_not_engage_the_companion_before_exact_entry() {
    let start = Instant::now();
    let mut tracker = tracker();

    assert!(!tracker.update(start, Some(WithinTolerance)));
    assert!(tracker.update(elapsed(start, 300), Some(WithinTolerance)));
}

#[test]
fn exact_entry_switches_from_bridge_delay_to_outside_delay() {
    let start = Instant::now();
    let mut tracker = tracker();

    assert!(!tracker.update(start, Some(Outside)));
    assert!(!tracker.update(elapsed(start, 299), Some(Inside)));
    assert!(!tracker.update(elapsed(start, 300), Some(Outside)));
    assert!(!tracker.update(elapsed(start, 479), Some(Outside)));
    assert!(tracker.update(elapsed(start, 480), Some(Outside)));
}

#[test]
fn tolerance_keeps_an_engaged_companion_open() {
    let start = Instant::now();
    let mut tracker = tracker();

    assert!(!tracker.update(start, Some(Inside)));
    assert!(!tracker.update(elapsed(start, 1), Some(Outside)));
    assert!(!tracker.update(elapsed(start, 180), Some(WithinTolerance)));
    assert!(!tracker.update(elapsed(start, 10_000), Some(WithinTolerance)));
}

#[test]
fn reentry_resets_the_continuous_outside_delay() {
    let start = Instant::now();
    let mut tracker = tracker();

    assert!(!tracker.update(start, Some(Inside)));
    assert!(!tracker.update(elapsed(start, 10), Some(Outside)));
    assert!(!tracker.update(elapsed(start, 189), Some(Outside)));
    assert!(!tracker.update(elapsed(start, 190), Some(Inside)));
    assert!(!tracker.update(elapsed(start, 200), Some(Outside)));
    assert!(!tracker.update(elapsed(start, 379), Some(Outside)));
    assert!(tracker.update(elapsed(start, 380), Some(Outside)));
}

#[test]
fn unavailable_cursor_resets_the_continuous_outside_delay() {
    let start = Instant::now();
    let mut tracker = tracker();

    assert!(!tracker.update(start, Some(Inside)));
    assert!(!tracker.update(elapsed(start, 10), Some(Outside)));
    assert!(!tracker.update(elapsed(start, 190), None));
    assert!(!tracker.update(elapsed(start, 200), Some(Outside)));
    assert!(!tracker.update(elapsed(start, 379), Some(Outside)));
    assert!(tracker.update(elapsed(start, 380), Some(Outside)));
}

#[test]
fn unavailable_cursor_never_triggers_concealment_directly() {
    let start = Instant::now();
    let mut tracker = tracker();

    assert!(!tracker.update(start, None));
    assert!(!tracker.update(elapsed(start, 10_000), None));
}

#[test]
fn configured_cursor_task_does_not_retain_its_manager() {
    let manager = manager();
    let weak = Arc::downgrade(&manager.inner);
    let task_weak = weak.clone();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let mut entered_tx = Some(entered_tx);
    let task = tauri::async_runtime::spawn(async move {
        let owner_survived = AnchoredWindowManager::wait_for_cursor_policy(
            &task_weak,
            BRIDGE_DELAY,
            OUTSIDE_DELAY,
            move |_| {
                if let Some(entered_tx) = entered_tx.take() {
                    let _ = entered_tx.send(());
                }
                Some(Inside)
            },
        )
        .await;
        let _ = done_tx.send(owner_survived);
    });
    manager.lock_lifecycle().task = Some(ScheduledTask {
        token: 1,
        handle: task,
    });

    tauri::async_runtime::block_on(async move {
        entered_rx.await.expect("the cursor policy loop starts");
        drop(manager);
        let owner_survived = tokio::time::timeout(Duration::from_secs(1), done_rx)
            .await
            .expect("the cursor policy loop stops after its owner drops")
            .expect("the cursor policy loop reports its result");
        assert!(!owner_survived);
    });
    assert!(weak.upgrade().is_none());
}

#[test]
fn stored_cursor_task_does_not_retain_its_manager() {
    let manager = manager();
    let weak = Arc::downgrade(&manager.inner);
    let task_weak = weak.clone();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let mut entered_tx = Some(entered_tx);
    let task = tauri::async_runtime::spawn(async move {
        let owner_survived = AnchoredWindowManager::wait_for_cursor_exit(&task_weak, move |_| {
            if let Some(entered_tx) = entered_tx.take() {
                let _ = entered_tx.send(());
            }
            Some(true)
        })
        .await;
        let _ = done_tx.send(owner_survived);
    });
    manager.lock_lifecycle().task = Some(ScheduledTask {
        token: 1,
        handle: task,
    });

    tauri::async_runtime::block_on(async move {
        entered_rx.await.expect("the cursor loop starts");
        drop(manager);
        let owner_survived = tokio::time::timeout(Duration::from_secs(1), done_rx)
            .await
            .expect("the cursor loop stops after its owner drops")
            .expect("the cursor loop reports its result");
        assert!(!owner_survived);
    });
    assert!(weak.upgrade().is_none());
}
