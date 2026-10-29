use super::*;

type LocalStartRow = (String, String, Option<String>, Option<String>);

impl StoreCheckStore {
    pub(super) fn start_local(
        &self,
        claimed: &ClaimedCandidate,
        now_ms: u64,
        cooldown: Duration,
        preparation: &LocalPreparation,
    ) -> Result<ClaimOutcome> {
        let started = Instant::now();
        let claim = self.claim_record(claimed)?;
        let Some(activation) = self.registered_activation(&claimed.candidate.activation) else {
            return Ok(ClaimOutcome::Stale);
        };
        ensure!(
            activation.compiled.is_local(),
            "local start requires a local check"
        );
        #[cfg(test)]
        self.store
            .burn_check_test_stage(super::super::test_hooks::Stage::BeforeAnswer);
        let mut connection = self.store.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !self.claim_is_current_in(&tx, &claim, &activation)? {
            return Ok(ClaimOutcome::Stale);
        }
        let saved: Option<LocalStartRow> = tx.query_row(
            "SELECT c.state, c.frozen_input_json, c.preparation_json, c.answer_json
             FROM burn_check_candidate c JOIN burn_check_activation a USING(check_id, activation_scope_key)
             WHERE c.candidate_id = ?1 AND c.claim_owner = ?2 AND c.claim_fence = ?3
               AND c.generation = a.generation AND a.enabled = 1 AND a.definition_revision = ?4",
            params![claim.candidate_id, claim.owner, claim.fence, hex(&activation.revision)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).optional()?;
        let Some((state, frozen, previous_preparation, previous_answer)) = saved else {
            return Ok(ClaimOutcome::Stale);
        };
        if !matches!(state.as_str(), "claimed" | "answered") {
            return Ok(ClaimOutcome::Stale);
        }
        ensure!(
            serde_json::from_str::<FrozenCandidate>(&frozen)? == claimed.candidate,
            "local start does not match the frozen candidate"
        );
        let attempted: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM burn_check_evaluation_attempt WHERE candidate_id = ?1)",
            [&claim.candidate_id],
            |row| row.get(0),
        )?;
        ensure!(!attempted, "local work cannot retain provider attempts");
        preparation
            .validate_for_resume()
            .map_err(|error| anyhow::anyhow!(error.detail))?;
        let saved_preparation = PreparationRecord::Local(preparation.clone());
        let encoded_preparation = super::attempts::encode_preparation(&saved_preparation)?;
        let evaluated = EvaluatedAssessment::local();
        evaluated
            .validate_for_declarations(
                &activation.compiled.decisions,
                &activation.compiled.question_requirements(),
            )
            .map_err(|error| anyhow::anyhow!(error.message))?;
        let answer = serde_json::to_string(&evaluated)?;
        if state == "answered" {
            ensure!(
                previous_preparation
                    .as_deref()
                    .map(
                        |saved| antiburn_checks::preparation::decode_preparation_record(
                            saved.as_bytes()
                        )
                        .map_err(|error| anyhow::anyhow!(error.detail))
                    )
                    .transpose()?
                    .as_ref()
                    == Some(&saved_preparation)
                    && previous_answer
                        .as_deref()
                        .map(
                            |saved| decode_results(saved.as_bytes())?.validate_for_declarations(
                                &activation.compiled.decisions,
                                &activation.compiled.question_requirements()
                            )
                        )
                        .transpose()?
                        .as_ref()
                        == Some(&evaluated),
                "local start is immutable"
            );
            return Ok(ClaimOutcome::Current);
        }
        ensure!(
            previous_preparation.is_none() && previous_answer.is_none(),
            "local start requires an unstarted claim"
        );
        ensure!(
            preparation.started_at_unix_ms == now_ms,
            "local start time does not match preparation"
        );
        let changed = tx.execute(
            "UPDATE burn_check_candidate SET state = 'answered', preparation_json = ?4,
                 answer_json = ?5, updated_at_epoch = ?6
             WHERE candidate_id = ?1 AND claim_owner = ?2 AND claim_fence = ?3 AND state = 'claimed'",
            params![claim.candidate_id, claim.owner, claim.fence, encoded_preparation, answer, integer(now_ms / 1000)?],
        )?;
        ensure!(changed == 1, "local start did not save its exact claim");
        if !cooldown.is_zero() {
            let cooldown_ms = u64::try_from(cooldown.as_millis())
                .context("cooldown exceeds supported duration")?;
            let until = now_ms.saturating_add(cooldown_ms).saturating_add(999) / 1000;
            let changed = tx.execute(
                "INSERT INTO burn_check_cooldown SELECT check_id, activation_scope_key, generation, branch_key, ?2
                 FROM burn_check_candidate WHERE candidate_id = ?1
                 ON CONFLICT(check_id, activation_scope_key, generation, branch_key) DO UPDATE SET
                     cooldown_until_epoch = MAX(cooldown_until_epoch, excluded.cooldown_until_epoch)",
                params![claim.candidate_id, integer(until)?],
            )?;
            ensure!(changed == 1, "local start did not save its first cooldown");
        }
        tx.commit()?;
        drop(connection);
        ::tracing::debug!(
            event = "burn_check_local_start_committed",
            activation_ref = activation_ref(&claimed.candidate.activation),
            candidate_ref = candidate_ref(&claimed.candidate),
            generation = claimed.candidate.activation.generation.0,
            claim_fence = claimed.fence,
            provider_attempts = 0,
            elapsed_ms = started.elapsed().as_millis() as u64
        );
        #[cfg(test)]
        self.store
            .burn_check_test_stage(super::super::test_hooks::Stage::AfterAnswer);
        Ok(ClaimOutcome::Current)
    }
}
