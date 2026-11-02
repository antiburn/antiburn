use anyhow::Result;
use rusqlite::{TransactionBehavior, params};

use crate::store::Store;

impl Store {
    /// Revokes unfinished work without changing definitions or session evidence.
    pub(crate) fn cancel_unfinished_burn_check_assessments_for_connection_change(
        &self,
    ) -> Result<()> {
        #[cfg(test)]
        self.burn_check_test_stage(super::test_hooks::Stage::BeforeCancellation);
        let mut connection = self.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let diagnostic = serde_json::to_string(&antiburn_checks::engine::EvaluationDiagnostic {
            code: antiburn_checks::engine::EvaluationFailureCode::Cancelled,
            detail: "The selected connection changed before this assessment finished.".into(),
        })?;
        transaction.execute(
            "INSERT INTO burn_check_assessment_history
             SELECT 'assessment:' || candidate_id, candidate_id, check_id,
                 activation_scope_key, generation, branch_key, trigger_position,
                 'cancelled', answer_json, '{}', 0, ?1
             FROM burn_check_candidate
             WHERE state IN ('pending', 'claimed', 'dispatching', 'answered')
             ON CONFLICT(candidate_id) DO NOTHING",
            [now],
        )?;
        // Keep the claim so late attempts can settle without publication rights.
        transaction.execute(
            "UPDATE burn_check_candidate SET state = 'cancelled',
                 diagnostic_json = ?1, updated_at_epoch = ?2
             WHERE state IN ('pending', 'claimed', 'dispatching', 'answered')",
            params![diagnostic, now],
        )?;
        transaction.commit()?;
        Ok(())
    }
}
