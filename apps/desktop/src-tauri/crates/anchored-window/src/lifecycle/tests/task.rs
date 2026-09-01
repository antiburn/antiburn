use super::fixtures::lifecycle;

#[test]
fn task_installation_rejects_stale_tokens_without_panicking() {
    let mut lifecycle = lifecycle();
    let current_token = lifecycle.reserve_task();
    let current = tauri::async_runtime::spawn(std::future::pending());

    assert!(lifecycle.install_task(current_token, current).is_ok());

    let stale = tauri::async_runtime::spawn(std::future::pending());
    let rejected = lifecycle
        .install_task(current_token.wrapping_add(1), stale)
        .expect_err("the stale task remains owned by the caller");
    rejected.abort();
    assert_eq!(
        lifecycle.task.as_ref().map(|task| task.token),
        Some(current_token)
    );
}
