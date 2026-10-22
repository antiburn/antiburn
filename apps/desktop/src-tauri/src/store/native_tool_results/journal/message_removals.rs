//! Apply whole-message removals to durable bindings in native event order.

use antiburn_local::analysis::{
    MAX_TOOL_RESULT_BYTES, MAX_TOOL_RESULT_OBSERVATIONS, OpenCodeMessageRemoval, SourceFormat,
    ToolResultChange, ToolResultObservation, ToolResultPosition,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Transaction, params};

use super::{
    OpenCodeJournalClaim, PublicationPolicy, ToolResultBatch, publish_coverage_in,
    reconcile_with_policy,
};
use crate::store::SessionKey;

#[derive(Default)]
struct PublicationBudget {
    observations: usize,
    bytes: usize,
}

impl PublicationBudget {
    fn check(&self, bytes: usize) -> Result<()> {
        ensure!(
            self.observations < MAX_TOOL_RESULT_OBSERVATIONS,
            "OpenCode journal publication exceeds its observation limit"
        );
        ensure!(
            bytes <= MAX_TOOL_RESULT_BYTES - self.bytes,
            "OpenCode journal publication exceeds its byte limit"
        );
        Ok(())
    }

    fn retain(&mut self, result: &ToolResultObservation) -> Result<()> {
        let bytes = serde_json::to_vec(result)?.len();
        self.check(bytes)?;
        self.observations += 1;
        self.bytes += bytes;
        Ok(())
    }
}

pub(super) fn reconcile_page(
    tx: &Transaction<'_>,
    claim: &OpenCodeJournalClaim,
    generation: i64,
    batch: &ToolResultBatch,
    removals: &[OpenCodeMessageRemoval],
    policy: PublicationPolicy<'_>,
) -> Result<()> {
    let mut budget = PublicationBudget::default();
    for result in &batch.results {
        budget.retain(result)?;
    }
    publish_coverage_in(
        tx,
        &claim.key,
        generation,
        claim.claim_fence,
        batch,
        policy
            .history_complete
            .context("journal publication requires a coverage state")?,
        policy.gap_reason,
    )?;
    let mut results = batch.results.iter().peekable();
    for removal in removals {
        while let Some(result) = results.peek() {
            let ToolResultPosition::OpenCodeEvent { sequence, .. } = result.position else {
                anyhow::bail!("OpenCode journal result has a different source position");
            };
            if sequence > removal.sequence {
                break;
            }
            reconcile_result(tx, claim, generation, result, policy)?;
            results.next();
        }
        for result in stored_removals(tx, &claim.key, removal, &mut budget)? {
            reconcile_result(tx, claim, generation, &result, policy)?;
        }
    }
    for result in results {
        reconcile_result(tx, claim, generation, result, policy)?;
    }
    Ok(())
}

fn reconcile_result(
    tx: &Transaction<'_>,
    claim: &OpenCodeJournalClaim,
    generation: i64,
    result: &ToolResultObservation,
    policy: PublicationPolicy<'_>,
) -> Result<()> {
    if result.change == ToolResultChange::Update {
        let identity = serde_json::to_string(&(
            &result.session_id,
            &result.record_id,
            &result.call_id,
            &result.result_id,
        ))?;
        let removed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM native_tool_result_current
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND source_format = ?4 AND native_identity = ?5 AND is_removed = 1)",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                serde_json::to_string(&result.source_format)?,
                identity
            ],
            |row| row.get(0),
        )?;
        // Native IDs cannot restore a deleted binding, including after parser eviction.
        if removed {
            return Ok(());
        }
    }
    reconcile_with_policy(
        tx,
        &claim.key,
        generation,
        claim.claim_fence,
        &ToolResultBatch {
            source_format: SourceFormat::OpenCodeSqliteV1,
            results: vec![result.clone()],
            complete: false,
        },
        PublicationPolicy {
            suppress_coverage: true,
            ..policy
        },
    )
}

fn stored_removals(
    tx: &Transaction<'_>,
    key: &SessionKey,
    removal: &OpenCodeMessageRemoval,
    budget: &mut PublicationBudget,
) -> Result<Vec<ToolResultObservation>> {
    let mut prefix = serde_json::to_string(&(&key.session_id, &removal.message_id))?;
    prefix.pop();
    // The comma bounds the exact message tuple in the primary-key index.
    let lower = format!("{prefix},");
    let upper = format!("{prefix}-");
    let mut statement = tx.prepare(
        "SELECT length(CAST(payload_json AS BLOB)), payload_json
           FROM native_tool_result_current
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
            AND source_format = ?4 AND native_identity >= ?5 AND native_identity < ?6
            AND is_removed = 0 ORDER BY native_identity",
    )?;
    let mut rows = statement.query(params![
        key.environment_key,
        key.agent,
        key.session_id,
        serde_json::to_string(&SourceFormat::OpenCodeSqliteV1)?,
        lower,
        upper
    ])?;
    let mut results = Vec::new();
    while let Some(row) = rows.next()? {
        budget.check(row.get(0)?)?;
        let mut result: ToolResultObservation = serde_json::from_str(row.get_ref(1)?.as_str()?)?;
        ensure!(
            result.session_id == key.session_id
                && result.source_format == SourceFormat::OpenCodeSqliteV1
                && result.record_id.as_deref() == Some(removal.message_id.as_str()),
            "OpenCode message removal does not match its stored binding"
        );
        result.change = ToolResultChange::Removal;
        // A parser reset can replay a removal before a saved later correction.
        result.revision = removal
            .sequence
            .max(result.revision)
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("OpenCode removal revision exceeds its range"))?;
        result.position = ToolResultPosition::OpenCodeEvent {
            sequence: removal.sequence,
            item_index: 0,
        };
        budget.retain(&result)?;
        results.push(result);
    }
    Ok(results)
}
