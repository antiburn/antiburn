use anyhow::Result;
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::store::Store;

impl Store {
    pub(crate) fn record_burn_check_overflow(
        &self,
        candidate_id: &str,
        safe_reason: &str,
        now_epoch: i64,
    ) -> Result<bool> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidate: Option<(String, String, i64, String, i64)> = tx
            .query_row(
                "SELECT check_id, activation_scope_key, generation, branch_key, trigger_position
                   FROM burn_check_candidate WHERE candidate_id = ?1 AND state = 'pending'",
                params![candidate_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((check_id, activation_scope_key, generation, branch_key, position)) = candidate
        else {
            return Ok(false);
        };
        tx.execute(
            "INSERT INTO burn_check_assessment_history VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'not_assessed', NULL, ?8, 0, ?9)",
            params![
                format!("overflow:{candidate_id}"),
                candidate_id,
                check_id,
                activation_scope_key,
                generation,
                branch_key,
                position,
                serde_json::to_string(&serde_json::json!({"detail": safe_reason}))?,
                now_epoch
            ],
        )?;
        tx.execute(
            "UPDATE burn_check_candidate SET state = 'not_assessed', frozen_input_json = '{}',
                 dependencies_json = '{}', updated_at_epoch = ?2 WHERE candidate_id = ?1",
            params![candidate_id, now_epoch],
        )?;
        tx.commit()?;
        Ok(true)
    }
}
