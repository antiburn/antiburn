use std::sync::Arc;
use std::time::Duration;

use crate::lifecycle::ScheduledTask;

use super::super::AnchoredWindowManager;
use super::fixtures::manager;

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
