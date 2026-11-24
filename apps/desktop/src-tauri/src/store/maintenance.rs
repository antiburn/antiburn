//! Passive checkpoints for the local database WAL.

use std::time::Duration;

use anyhow::Result;
use rusqlite::{Connection, OpenFlags};

use super::{Store, database_path};

/// The WAL size that SQLite keeps after a checkpoint resets the log.
pub(super) const WAL_SIZE_LIMIT_BYTES: i64 = 64 * 1024 * 1024;

/// Progress from a passive checkpoint. Frame counts are -1 without a WAL.
#[derive(Debug)]
pub(crate) struct WalCheckpoint {
    pub busy: bool,
    pub log_frames: i64,
    pub checkpointed_frames: i64,
}

impl Store {
    /// Copy available WAL frames on a blocking worker.
    /// This uses a separate connection so Sessions can read through the Store.
    /// Readers and writers can leave frames for a later checkpoint.
    pub(crate) fn checkpoint_wal(&self) -> Result<WalCheckpoint> {
        let connection = Connection::open_with_flags(
            database_path(&self.state_dir),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::ZERO)?;
        Ok(
            connection.query_row("PRAGMA main.wal_checkpoint(PASSIVE)", [], |row| {
                Ok(WalCheckpoint {
                    busy: row.get(0)?,
                    log_frames: row.get(1)?,
                    checkpointed_frames: row.get(2)?,
                })
            })?,
        )
    }
}

#[cfg(test)]
#[path = "tests/maintenance_integration_tests.rs"]
mod tests;
