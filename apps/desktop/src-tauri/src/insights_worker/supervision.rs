use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;

const RESTART_BASE: Duration = Duration::from_secs(1);
const RESTART_MAX: Duration = Duration::from_secs(60);

pub(super) async fn run<F, Fut>(role: &'static str, worker: F)
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let worker = Arc::new(worker);
    let mut restart_delay = RESTART_BASE;
    let mut tasks = JoinSet::new();
    loop {
        let started = tokio::time::Instant::now();
        let worker = Arc::clone(&worker);
        // JoinSet aborts this child when its supervisor stops.
        tasks.spawn(async move { worker().await });
        match tasks.join_next().await {
            Some(Err(error)) => {
                ::tracing::error!(event = "insights_worker_task_failed", role, error = %error);
            }
            Some(Ok(())) => {
                ::tracing::error!(event = "insights_worker_task_stopped", role);
            }
            None => return,
        }
        if started.elapsed() >= RESTART_MAX {
            restart_delay = RESTART_BASE;
        }
        ::tracing::warn!(
            event = "insights_worker_restart_scheduled",
            role,
            delay_secs = restart_delay.as_secs(),
        );
        tokio::time::sleep(restart_delay).await;
        restart_delay = restart_delay.saturating_mul(2).min(RESTART_MAX);
    }
}
