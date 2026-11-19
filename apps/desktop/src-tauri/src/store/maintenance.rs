//! Storage upkeep: give unused database and WAL space back to the disk.
//!
//! SQLite keeps freed pages inside the database file, and a WAL file stays
//! at its largest size unless something trims it. [`Store::maintain_storage`]
//! vacuums when free space is large, then truncates the WAL.

use anyhow::Result;
use rusqlite::Connection;

use super::Store;

/// The WAL size that SQLite keeps after a checkpoint resets the log.
pub(super) const WAL_SIZE_LIMIT_BYTES: i64 = 64 * 1024 * 1024;

/// When [`Store::maintain_storage`] vacuums the database file.
///
/// A vacuum occurs only when both limits are met. The byte limit stops
/// small, frequent vacuums. The ratio limit stops a vacuum of a large,
/// mostly live file for a small gain.
#[derive(Debug, Clone, Copy)]
pub struct VacuumThreshold {
    pub min_free_bytes: u64,
    pub min_free_ratio: f64,
}

impl Default for VacuumThreshold {
    fn default() -> Self {
        Self {
            min_free_bytes: 256 * 1024 * 1024,
            min_free_ratio: 0.25,
        }
    }
}

/// What one [`Store::maintain_storage`] call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageUpkeep {
    pub file_bytes_before: u64,
    pub free_bytes_before: u64,
    pub file_bytes_after: u64,
    pub vacuumed: bool,
    /// False when a reader blocked the WAL reset. The next run tries again.
    pub wal_truncated: bool,
}

struct PageUse {
    file_bytes: u64,
    free_bytes: u64,
}

fn page_use(connection: &Connection) -> Result<PageUse> {
    let page_size: u64 = connection.pragma_query_value(None, "page_size", |row| row.get(0))?;
    let page_count: u64 = connection.pragma_query_value(None, "page_count", |row| row.get(0))?;
    let free_pages: u64 =
        connection.pragma_query_value(None, "freelist_count", |row| row.get(0))?;
    Ok(PageUse {
        file_bytes: page_size * page_count,
        free_bytes: page_size * free_pages,
    })
}

impl Store {
    /// Vacuum when free space meets `threshold`, then truncate the WAL.
    ///
    /// This holds the writer lock for the full call, so other writes wait.
    /// A vacuum takes time in proportion to live data. Call it from a
    /// blocking worker, away from app launch. A failed vacuum loses no
    /// data, and the caller can try again later.
    pub fn maintain_storage(&self, threshold: VacuumThreshold) -> Result<StorageUpkeep> {
        let connection = self.lock();
        let before = page_use(&connection)?;
        let vacuumed = before.free_bytes >= threshold.min_free_bytes
            && before.file_bytes > 0
            && before.free_bytes as f64 / before.file_bytes as f64 >= threshold.min_free_ratio;
        let vacuum = if vacuumed {
            connection.execute_batch("VACUUM")
        } else {
            Ok(())
        };
        // In WAL mode a vacuum writes every live page through the WAL. A
        // TRUNCATE checkpoint copies those pages back and empties the log.
        // The checkpoint also runs after a failed vacuum, which rolls back
        // and loses no data. The first column is 1 when a reader blocked
        // the checkpoint.
        let checkpoint = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            row.get::<_, i64>(0)
        });
        vacuum?;
        let busy = checkpoint?;
        let after = page_use(&connection)?;
        Ok(StorageUpkeep {
            file_bytes_before: before.file_bytes,
            free_bytes_before: before.free_bytes,
            file_bytes_after: after.file_bytes,
            vacuumed,
            wal_truncated: busy == 0,
        })
    }
}

#[cfg(test)]
#[path = "tests/maintenance_integration_tests.rs"]
mod tests;
