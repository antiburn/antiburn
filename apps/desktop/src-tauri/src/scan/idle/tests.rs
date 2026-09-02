use super::*;
use crate::agents;
use crate::store::{SessionKey, SessionRecord, Store};
use std::sync::{Arc, Mutex};

fn record(session_id: &str, updated_at: i64) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", "claude-code", session_id),
        source_kind: "file".into(),
        source_label: format!("/tmp/{session_id}.jsonl"),
        wsl_distro: None,
        title: None,
        title_source: None,
        cwd: None,
        surface: "cli".into(),
        updated_at_epoch: Some(updated_at),
        activity_cursor: String::new(),
        activity_source: "mtime".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: None,
    }
}

/// A clock tied to tokio's own (pausable) instant, not the wall clock: every
/// `tokio::time::sleep` this task awaits advances it exactly as far as it
/// advances tokio's clock, so a paused test can drive it deterministically.
fn instant_clock(base_epoch: i64) -> impl Fn() -> i64 + Send + Sync + 'static {
    let base_instant = Instant::now();
    move || base_epoch + base_instant.elapsed().as_secs() as i64
}

#[tokio::test(start_paused = true)]
async fn sessions_expire_in_deadline_order_and_not_before() {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let base_epoch = 1_000_000_i64;
    store
        .upsert_sessions(
            &[
                record("written-ten-seconds-ago", base_epoch - 10),
                record("written-a-hundred-seconds-ago", base_epoch - 100),
            ],
            &agents::evidence_cohort(),
        )
        .unwrap();

    let wake = IdleWake::default();
    let announced: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let announced_task = announced.clone();
    let store_task = store.clone();
    let now = instant_clock(base_epoch);
    tokio::spawn(async move {
        run(&store_task, &wake, &now, &move |entry: ActivityEntry| {
            announced_task.lock().unwrap().push(entry.session_id);
        })
        .await;
    });

    // Neither has reached its 180s window yet: the session written 100s ago
    // still has 79s left, the one written 10s ago has 169s left.
    tokio::time::sleep(Duration::from_secs(79)).await;
    assert!(
        announced.lock().unwrap().is_empty(),
        "nothing should expire before its own deadline"
    );

    // The session written 100s ago crosses its window first, at t=80s (plus
    // the 1s slack).
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        *announced.lock().unwrap(),
        vec!["written-a-hundred-seconds-ago".to_string()]
    );

    // The session written 10s ago crosses its window at t=170s.
    tokio::time::sleep(Duration::from_secs(90)).await;
    assert_eq!(
        *announced.lock().unwrap(),
        vec![
            "written-a-hundred-seconds-ago".to_string(),
            "written-ten-seconds-ago".to_string(),
        ],
        "the row written more recently expires later, and second"
    );
}

#[tokio::test(start_paused = true)]
async fn a_fresh_write_re_arms_an_earlier_deadline() {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let base_epoch = 1_000_000_i64;
    // 170s old: 10s from its window, the task's first deadline.
    store
        .upsert_sessions(
            &[record("moving", base_epoch - 170)],
            &agents::evidence_cohort(),
        )
        .unwrap();

    let wake = Arc::new(IdleWake::default());
    let wake_task = wake.clone();
    let announced: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let announced_task = announced.clone();
    let store_task = store.clone();
    let now = instant_clock(base_epoch);
    tokio::spawn(async move {
        run(
            &store_task,
            wake_task.as_ref(),
            &now,
            &move |entry: ActivityEntry| {
                announced_task.lock().unwrap().push(entry.session_id);
            },
        )
        .await;
    });
    // Give the task a chance to read the store and start sleeping on the
    // stale 11s deadline before the write below moves it.
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;

    // A fresh write moves the same session's activity to "now", pushing its
    // deadline out to a full window away, and wakes the task the way a scan
    // pass does.
    store
        .upsert_sessions(&[record("moving", base_epoch)], &agents::evidence_cohort())
        .unwrap();
    wake.0.notify_one();
    // Let the task observe the write before the old deadline would fire.
    tokio::time::sleep(Duration::from_secs(1)).await;

    // The old deadline (t=11s) passes with nothing announced: the rearm won.
    tokio::time::sleep(Duration::from_secs(15)).await;
    assert!(
        announced.lock().unwrap().is_empty(),
        "the rearmed deadline should replace the stale one, not both fire"
    );

    // The rearmed deadline, a full window from the fresh write, now passes.
    tokio::time::sleep(Duration::from_secs(180)).await;
    assert_eq!(*announced.lock().unwrap(), vec!["moving".to_string()]);
}

#[tokio::test(start_paused = true)]
async fn an_empty_store_emits_nothing_and_parks_instead_of_spinning() {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let announced: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let announced_task = announced.clone();
    let wake = IdleWake::default();
    let now = instant_clock(1_000_000);
    tokio::spawn(async move {
        run(&store, &wake, &now, &move |entry: ActivityEntry| {
            announced_task.lock().unwrap().push(entry.session_id);
        })
        .await;
    });

    // A long virtual wait resolves instantly under a paused clock; if the
    // loop were spinning instead of parking on `Notify`, this task would
    // never be scheduled and the sleep below would never return.
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert!(announced.lock().unwrap().is_empty());
}
