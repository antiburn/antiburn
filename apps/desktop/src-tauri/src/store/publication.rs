//! Set-based turn-row publication for one session.

use std::collections::HashSet;

use antiburn_local::analysis::TurnSessionKey;
use anyhow::{Result, ensure};
use rusqlite::{Transaction, params};

use super::{SourcePublishMode, SourcePublishOutcome};

/// JSON source sets shared by the publication statements.
pub(super) struct SourceSets {
    named: String,
    resumed: String,
}

impl SourceSets {
    /// Builds the sets after checking that each source has one outcome.
    pub(super) fn new(sources: &[SourcePublishOutcome]) -> Result<Self> {
        let mut named = HashSet::with_capacity(sources.len());
        for source in sources {
            ensure!(
                named.insert(source.source_key.as_str()),
                "duplicate source publication outcome: {}",
                source.source_key
            );
        }
        let resumed = sources
            .iter()
            .filter_map(|source| {
                (source.mode == SourcePublishMode::Resumed).then_some(source.source_key.as_str())
            })
            .collect::<Vec<_>>();
        Ok(Self {
            named: serde_json::to_string(&named)?,
            resumed: serde_json::to_string(&resumed)?,
        })
    }
}

/// Publishes one pass's rows and removes snapshots for vanished sources.
///
/// Each statement scans the session at most once. The work grows with the
/// session row count and source list instead of their product.
pub(super) fn publish_turn_rows(
    transaction: &Transaction<'_>,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
    target_fence: i64,
    source_sets: &SourceSets,
) -> Result<()> {
    // The claim-fence rows still identify unnamed sources at this point.
    // Named sources also cover full reads that produced no rows.
    transaction.execute(
        "DELETE FROM source_resume
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
            AND source_key NOT IN (
                SELECT value FROM json_each(?4)
                UNION
                SELECT source_key FROM turn
                 WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                   AND claim_fence = ?5
            )",
        params![
            key.environment_key,
            key.agent,
            key.session_id,
            source_sets.named,
            claim_fence
        ],
    )?;

    // The first publication already stores every new row at the target fence.
    if target_fence == claim_fence {
        return Ok(());
    }

    // Only resumed sources retain their rows from the previous publication.
    transaction.execute(
        "DELETE FROM turn_content WHERE turn_rowid IN (
             SELECT rowid FROM turn
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND claim_fence = ?4
                AND source_key NOT IN (SELECT value FROM json_each(?5))
         )",
        params![
            key.environment_key,
            key.agent,
            key.session_id,
            target_fence,
            source_sets.resumed
        ],
    )?;
    transaction.execute(
        "DELETE FROM turn
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
            AND claim_fence = ?4
            AND source_key NOT IN (SELECT value FROM json_each(?5))",
        params![
            key.environment_key,
            key.agent,
            key.session_id,
            target_fence,
            source_sets.resumed
        ],
    )?;
    transaction.execute(
        "UPDATE turn SET claim_fence = ?1
          WHERE environment_key = ?2 AND agent = ?3 AND session_id = ?4
            AND claim_fence = ?5",
        params![
            target_fence,
            key.environment_key,
            key.agent,
            key.session_id,
            claim_fence
        ],
    )?;
    Ok(())
}
