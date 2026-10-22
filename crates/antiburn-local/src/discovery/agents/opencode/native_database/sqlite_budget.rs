use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use rusqlite::{Connection, limits::Limit};

const PROGRESS_INTERVAL: i32 = 1_000;
const QUERY_INSTRUCTION_LIMIT: u64 = 5_000_000;
const QUERY_TIME_LIMIT: Duration = Duration::from_secs(1);
const SQLITE_VALUE_BYTES: i32 = 9 * 1024 * 1024;

pub(super) struct QueryBudget<'a> {
    connection: &'a Connection,
    previous_value_limit: i32,
    exhausted: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    started: Instant,
    duration: Duration,
}

impl<'a> QueryBudget<'a> {
    pub(super) fn new(connection: &'a Connection, cancelled: Arc<AtomicBool>) -> Self {
        Self::with_limits(
            connection,
            cancelled,
            QUERY_INSTRUCTION_LIMIT,
            QUERY_TIME_LIMIT,
        )
    }

    pub(super) fn strict(connection: &'a Connection) -> Self {
        Self::with_limits(
            connection,
            Arc::new(AtomicBool::new(false)),
            25_000_000,
            Duration::from_secs(10),
        )
    }

    fn with_limits(
        connection: &'a Connection,
        cancelled: Arc<AtomicBool>,
        instructions: u64,
        duration: Duration,
    ) -> Self {
        let previous_value_limit = connection.limit(Limit::SQLITE_LIMIT_LENGTH);
        connection.set_limit(
            Limit::SQLITE_LIMIT_LENGTH,
            previous_value_limit.min(SQLITE_VALUE_BYTES),
        );
        let exhausted = Arc::new(AtomicBool::new(false));
        let limit_reached = exhausted.clone();
        let cancellation = cancelled.clone();
        let started = Instant::now();
        let mut remaining = instructions;
        connection.progress_handler(
            PROGRESS_INTERVAL,
            Some(move || {
                if cancellation.load(Ordering::Acquire) {
                    return true;
                }
                remaining = remaining.saturating_sub(PROGRESS_INTERVAL as u64);
                let stop = remaining == 0 || started.elapsed() >= duration;
                limit_reached.store(stop, Ordering::Release);
                stop
            }),
        );
        Self {
            connection,
            previous_value_limit,
            exhausted,
            cancelled,
            started,
            duration,
        }
    }

    pub(super) fn check(&self) -> Result<(), &'static str> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err("cancelled");
        }
        if self.exhausted.load(Ordering::Acquire) || self.started.elapsed() >= self.duration {
            self.exhausted.store(true, Ordering::Release);
            return Err("query_limit");
        }
        Ok(())
    }

    pub(super) fn failure(&self, stage: &'static str) -> &'static str {
        self.check().err().unwrap_or(stage)
    }
}

impl Drop for QueryBudget<'_> {
    fn drop(&mut self) {
        self.connection.progress_handler(0, None::<fn() -> bool>);
        self.connection
            .set_limit(Limit::SQLITE_LIMIT_LENGTH, self.previous_value_limit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG_QUERY: &str = "WITH RECURSIVE numbers(n) AS (VALUES(1) UNION ALL SELECT n + 1 FROM numbers WHERE n < 1000000) SELECT sum(n) FROM numbers";

    #[test]
    fn instruction_budget_interrupts_work_and_restores_connection_limits() {
        let connection = Connection::open_in_memory().unwrap();
        let original = connection.limit(Limit::SQLITE_LIMIT_LENGTH);
        {
            let budget = QueryBudget::with_limits(
                &connection,
                Arc::new(AtomicBool::new(false)),
                2_000,
                Duration::from_secs(10),
            );
            assert!(
                connection
                    .query_row(LONG_QUERY, [], |row| row.get::<_, i64>(0))
                    .is_err()
            );
            assert_eq!(budget.failure("query"), "query_limit");
        }
        assert_eq!(connection.limit(Limit::SQLITE_LIMIT_LENGTH), original);
        assert_eq!(
            connection
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn cancellation_and_deadline_interrupt_sqlite_work() {
        for (cancelled, duration, expected) in [
            (true, Duration::from_secs(10), "cancelled"),
            (false, Duration::ZERO, "query_limit"),
        ] {
            let connection = Connection::open_in_memory().unwrap();
            let budget = QueryBudget::with_limits(
                &connection,
                Arc::new(AtomicBool::new(cancelled)),
                u64::MAX,
                duration,
            );
            assert!(
                connection
                    .query_row(LONG_QUERY, [], |row| row.get::<_, i64>(0))
                    .is_err()
            );
            assert_eq!(budget.failure("query"), expected);
        }
    }

    #[test]
    fn sqlite_rejects_oversized_values_before_returning_them() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE payload(data BLOB); INSERT INTO payload VALUES (zeroblob(33554432));",
            )
            .unwrap();
        let _budget = QueryBudget::new(&connection, Arc::new(AtomicBool::new(false)));
        assert_eq!(
            connection.limit(Limit::SQLITE_LIMIT_LENGTH),
            SQLITE_VALUE_BYTES
        );
        let result = connection.query_row("SELECT data FROM payload", [], |row| {
            row.get::<_, Vec<u8>>(0)
        });
        assert!(
            matches!(result, Err(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::TooBig)
        );
    }
}
