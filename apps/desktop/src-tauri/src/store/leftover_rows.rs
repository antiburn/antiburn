//! Removal of turn rows that a superseded analysis pass left behind.
//!
//! Old releases kept the rows of failed passes, and an interrupted pass
//! still leaves its own. Each copy makes the session's writes and deletes
//! slower. The removal works in short transactions so the writer stays free.

use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use super::{SessionKey, Store};

/// Limit turns between time checks because each turn can contain many parts.
const CHUNK_ROWS: usize = 16;
/// How long one batch deletes before it releases the writer.
const BATCH_TIME: Duration = Duration::from_millis(10);
/// The wait between two batches that lets other writers take the lock.
const BATCH_PAUSE: Duration = Duration::from_millis(10);

/// What one batch did. `more` is true when leftovers can remain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LeftoverBatch {
    pub removed: usize,
    pub more: bool,
}

impl Store {
    /// Remove one short batch of `key`'s rows from superseded fences.
    ///
    /// A fence is superseded when it is older than the session's current
    /// claim fence and is not its published fence. Each batch reads both
    /// fences in its own transaction, so a pass can start between batches.
    /// A session without an evidence row keeps all its rows.
    pub(crate) fn clear_leftover_batch(&self, key: &SessionKey) -> Result<LeftoverBatch> {
        self.clear_leftover_batch_within(key, BATCH_TIME)
    }

    /// Remove one batch with the specified time budget.
    pub(crate) fn clear_leftover_batch_within(
        &self,
        key: &SessionKey,
        limit: Duration,
    ) -> Result<LeftoverBatch> {
        let mut connection = self.lock();
        let started = Instant::now();
        let transaction = connection.transaction()?;
        let identity = params![key.environment_key, key.agent, key.session_id];
        let fences: Option<(i64, Option<i64>)> = transaction
            .query_row(
                "SELECT claim_fence, published_fence FROM session_evidence
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                identity,
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((claim, published)) = fences else {
            return Ok(LeftoverBatch::default());
        };
        // Two ranges step over the published rows instead of reading them.
        let ranges = match published.filter(|fence| *fence < claim) {
            Some(fence) => [Some((i64::MIN, fence)), Some((fence, claim))],
            None => [Some((i64::MIN, claim)), None],
        };
        let mut batch = LeftoverBatch::default();
        for (after, before) in ranges.into_iter().flatten() {
            loop {
                let removed = delete_chunk(&transaction, key, after, before)?;
                batch.removed += removed;
                if removed < CHUNK_ROWS {
                    break;
                }
                if started.elapsed() >= limit {
                    transaction.commit()?;
                    batch.more = true;
                    return Ok(batch);
                }
            }
        }
        transaction.commit()?;
        Ok(batch)
    }

    /// Remove every session's superseded rows and return how many went.
    ///
    /// Call this from a blocking worker. It sleeps between batches and holds
    /// the writer only inside a batch. A stop midway loses no work: the next
    /// call continues with the rows that remain.
    pub(crate) fn clear_leftover_rows(&self) -> Result<usize> {
        let keys = {
            let connection = self.lock();
            let mut statement = connection.prepare(
                "SELECT environment_key, agent, session_id FROM session_evidence
                  ORDER BY environment_key, agent, session_id",
            )?;
            statement
                .query_map([], |row| {
                    Ok(SessionKey {
                        environment_key: row.get(0)?,
                        agent: row.get(1)?,
                        session_id: row.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut total = 0;
        for key in keys {
            loop {
                let batch = self.clear_leftover_batch(&key)?;
                total += batch.removed;
                if batch.removed > 0 {
                    std::thread::sleep(BATCH_PAUSE);
                }
                if !batch.more {
                    break;
                }
            }
        }
        Ok(total)
    }
}

/// Delete up to [`CHUNK_ROWS`] turns, with their content, whose fence is
/// after `after` and before `before`. Coverage follows the last turn.
fn delete_chunk(
    transaction: &rusqlite::Transaction<'_>,
    key: &SessionKey,
    after: i64,
    before: i64,
) -> Result<usize> {
    let rowids = transaction
        .prepare_cached(
            "SELECT rowid FROM turn
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND claim_fence > ?4 AND claim_fence < ?5
              LIMIT ?6",
        )?
        .query_map(
            params![
                key.environment_key,
                key.agent,
                key.session_id,
                after,
                before,
                CHUNK_ROWS as i64
            ],
            |row| row.get::<_, i64>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut content =
        transaction.prepare_cached("DELETE FROM turn_content WHERE turn_rowid = ?1")?;
    let mut turn = transaction.prepare_cached("DELETE FROM turn WHERE rowid = ?1")?;
    for rowid in &rowids {
        content.execute([rowid])?;
        turn.execute([rowid])?;
    }
    if rowids.len() < CHUNK_ROWS {
        // One small row per fence, so one statement stays short.
        transaction.execute(
            "DELETE FROM session_coverage
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND claim_fence > ?4 AND claim_fence < ?5",
            params![
                key.environment_key,
                key.agent,
                key.session_id,
                after,
                before
            ],
        )?;
    }
    Ok(rowids.len())
}

#[cfg(test)]
#[path = "tests/leftover_rows_integration_tests.rs"]
mod tests;
