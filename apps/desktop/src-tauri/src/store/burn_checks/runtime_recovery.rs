use super::*;

impl StoreCheckStore {
    pub(super) fn recover_abandoned(&self) -> Result<()> {
        let mut connection = self.store.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE burn_check_candidate SET diagnostic_json = ?2
            WHERE activation_scope_key = ?1 AND state IN ('claimed', 'dispatching')
              AND EXISTS(SELECT 1 FROM burn_check_evaluation_attempt a
                  WHERE a.candidate_id = burn_check_candidate.candidate_id AND a.state = 'dispatch_intent')",
            params![self.activation_scope_key, serde_json::to_string(&EvaluationDiagnostic {
                code: antiburn_checks::engine::EvaluationFailureCode::DeliveryUnknown,
                detail: "The provider delivery outcome is unknown after restart.".into(),
            })?])?;
        tx.execute("UPDATE burn_check_evaluation_attempt SET state = 'settled', outcome = 'unknown', updated_at_epoch = ?2
            WHERE state = 'dispatch_intent' AND candidate_id IN (SELECT candidate_id FROM burn_check_candidate WHERE activation_scope_key = ?1)", params![self.activation_scope_key, now_epoch()])?;
        tx.execute("DELETE FROM burn_check_evaluation_attempt WHERE state = 'prepared' AND candidate_id IN (SELECT candidate_id FROM burn_check_candidate WHERE activation_scope_key = ?1)", [&self.activation_scope_key])?;
        tx.execute("UPDATE burn_check_candidate SET state = 'pending', claim_owner = NULL WHERE activation_scope_key = ?1
            AND state IN ('claimed', 'dispatching') AND (NOT EXISTS (
                SELECT 1 FROM burn_check_evaluation_attempt a WHERE a.candidate_id = burn_check_candidate.candidate_id)
            OR (EXISTS (SELECT 1 FROM burn_check_evaluation_attempt a WHERE a.candidate_id = burn_check_candidate.candidate_id
                AND a.attempt_number = 1 AND a.state = 'settled' AND a.outcome = 'safe_transient')
            AND NOT EXISTS (SELECT 1 FROM burn_check_evaluation_attempt a WHERE a.candidate_id = burn_check_candidate.candidate_id AND a.attempt_number = 2)))", [&self.activation_scope_key])?;
        tx.execute("INSERT INTO burn_check_assessment_history
            SELECT 'assessment:' || candidate_id, candidate_id, check_id, activation_scope_key, generation, branch_key, trigger_position,
                'execution_failed', NULL,
                json_object('detail', COALESCE(json_extract(diagnostic_json, '$.detail'), json_extract(?2, '$.detail'))),
                0, ?3 FROM burn_check_candidate
              WHERE activation_scope_key = ?1 AND state IN ('claimed', 'dispatching')",
            params![self.activation_scope_key, serde_json::to_string(&serde_json::json!({"detail": "dispatch outcome unknown or retry limit exhausted"}))?, now_epoch()])?;
        tx.execute("UPDATE burn_check_candidate SET state = 'execution_failed', claim_owner = NULL, updated_at_epoch = ?2
            WHERE activation_scope_key = ?1 AND state IN ('claimed', 'dispatching')", params![self.activation_scope_key, now_epoch()])?;
        tx.execute("UPDATE burn_check_candidate SET claim_owner = ?2, claim_fence = claim_fence + 1 WHERE activation_scope_key = ?1 AND state = 'answered'", params![self.activation_scope_key, self.owner])?;
        tx.commit()?;
        Ok(())
    }

    pub(super) fn recovery_page(
        &self,
        after: Option<&CandidateId>,
        limit: usize,
    ) -> Result<Vec<RecoveryRecord>> {
        ensure!(
            (1..=500).contains(&limit),
            "recovery page limit must be between 1 and 500"
        );
        let connection = self.store.lock();
        let mut statement = connection.prepare(
            "SELECT c.state, c.frozen_input_json, c.answer_json, c.prior_status, c.claim_fence
            FROM burn_check_candidate c JOIN burn_check_activation a USING(check_id, activation_scope_key)
            WHERE c.activation_scope_key = ?1 AND c.candidate_id > ?2 AND c.state IN ('pending', 'answered')
              AND c.generation = a.generation AND a.enabled = 1 ORDER BY c.candidate_id LIMIT ?3",
        )?;
        let rows = statement
            .query_map(
                params![
                    self.activation_scope_key,
                    after.map(|id| hex(&id.0)).unwrap_or_default(),
                    limit as i64
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, u64>(4)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let activations = self
            .activations
            .lock()
            .expect("activation mutex is not poisoned");
        rows.into_iter()
            .map(|(state, frozen, answer, prior, fence)| {
                let candidate: FrozenCandidate = serde_json::from_str(&frozen)?;
                let activation = activations
                    .get(&candidate.activation)
                    .context("register all durable activations before recovery")?
                    .clone();
                let work = if state == "answered" {
                    let evaluated: EvaluatedCandidate = serde_json::from_str(
                        &answer.context("answered candidate has no saved response")?,
                    )?;
                    evaluated
                        .validate(&activation.compiled)
                        .map_err(|error| anyhow::anyhow!(error.message))?;
                    RecoveryWork::Answered {
                        claimed: ClaimedCandidate {
                            id: CandidateId::from(&candidate),
                            candidate,
                            prior_status: parse_status(prior)?,
                            fence,
                            next_attempt: evaluated.attempts,
                        },
                        evaluated,
                    }
                } else {
                    RecoveryWork::Pending(candidate)
                };
                Ok(RecoveryRecord { activation, work })
            })
            .collect()
    }
}
