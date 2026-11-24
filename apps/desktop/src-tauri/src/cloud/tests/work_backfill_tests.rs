use super::{BACKFILL_ADMISSION_TIMEOUT, BUSY, BlockingWork, Priority};
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
};

async fn assert_waiting<T>(mut work: Pin<&mut impl Future<Output = T>>) {
    assert!(
        poll_fn(|cx| Poll::Ready(work.as_mut().poll(cx)))
            .await
            .is_pending()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cloud_backfill_waits_behind_existing_cleanup_without_queueing_duplicates() {
    let work = BlockingWork::default();
    let running = work.admission.try_acquire().unwrap();
    let cleaned = Arc::new(AtomicBool::new(false));
    let cleanup_finished = cleaned.clone();
    let mut cleanup = std::pin::pin!(work.run(Priority::Cleanup, move || {
        cleanup_finished.store(true, Ordering::Release);
        Ok(())
    }));
    assert_waiting(cleanup.as_mut()).await;
    let mut backfill = std::pin::pin!(work.run(Priority::Backfill, move || {
        assert!(cleaned.load(Ordering::Acquire));
        Ok("started")
    }));
    assert_waiting(backfill.as_mut()).await;
    assert_eq!(
        work.run(Priority::Backfill, || panic!("duplicate backfill ran"))
            .await,
        Err::<(), _>(BUSY.into())
    );
    assert_eq!(
        work.run(Priority::Normal, || panic!("normal work ran"))
            .await,
        Err::<(), _>(BUSY.into())
    );

    drop(running);
    assert_eq!(tokio::join!(cleanup, backfill), (Ok(()), Ok("started")));
}

#[tokio::test(flavor = "current_thread")]
async fn cloud_backfill_cancelled_wait_releases_queue_space() {
    let work = BlockingWork::default();
    let running = work.admission.try_acquire().unwrap();
    {
        let mut backfill = std::pin::pin!(work.run::<(), _>(Priority::Backfill, || {
            panic!("cancelled backfill ran");
        }));
        assert_waiting(backfill.as_mut()).await;
    }
    let mut next = std::pin::pin!(work.run(Priority::Backfill, || Ok("started")));
    assert_waiting(next.as_mut()).await;
    drop(running);
    assert_eq!(next.await, Ok("started"));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cloud_backfill_times_out_without_starting_work() {
    let work = BlockingWork::default();
    let running = work.admission.try_acquire().unwrap();
    let mut queued = std::pin::pin!(work.run::<(), _>(Priority::Backfill, || {
        panic!("expired backfill ran");
    }));
    assert_waiting(queued.as_mut()).await;
    tokio::time::advance(BACKFILL_ADMISSION_TIMEOUT).await;
    drop(running);
    assert_eq!(queued.await, Err(BUSY.into()));
    assert_eq!(
        work.run(Priority::Backfill, || Ok("started")).await,
        Ok("started")
    );
}
