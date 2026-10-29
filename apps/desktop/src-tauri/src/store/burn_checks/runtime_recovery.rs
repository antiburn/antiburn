use super::*;

#[derive(Debug)]
struct RecoveryActivationMissing;
impl std::fmt::Display for RecoveryActivationMissing {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("register all durable activations before recovery")
    }
}
impl std::error::Error for RecoveryActivationMissing {}

struct RecoveryCandidateRow {
    id: String,
    state: String,
    prior_status: Option<String>,
    fence: u64,
    check_id: String,
    generation: u64,
}

impl StoreCheckStore {
    pub(super) fn recover_abandoned(&self) -> Result<()> {
        let started = Instant::now();
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
        let unknown_attempts = tx.execute("UPDATE burn_check_evaluation_attempt SET state = 'settled', outcome = 'unknown', updated_at_epoch = ?2
            WHERE state = 'dispatch_intent' AND candidate_id IN (SELECT candidate_id FROM burn_check_candidate WHERE activation_scope_key = ?1)", params![self.activation_scope_key, now_epoch()])?;
        let requeued = tx.execute("UPDATE burn_check_candidate SET state = 'pending', claim_owner = NULL WHERE activation_scope_key = ?1
            AND state IN ('claimed', 'dispatching') AND NOT EXISTS (
                SELECT 1 FROM burn_check_evaluation_attempt a WHERE a.candidate_id = burn_check_candidate.candidate_id
                  AND (a.decision_name IS NULL OR a.state <> 'settled' OR a.outcome NOT IN ('answer', 'safe_transient')
                    OR (a.outcome = 'safe_transient' AND a.attempt_number = 2)))", [&self.activation_scope_key])?;
        tx.execute("INSERT INTO burn_check_assessment_history
            SELECT 'assessment:' || candidate_id, candidate_id, check_id, activation_scope_key, generation, branch_key, trigger_position,
                'execution_failed', NULL,
                json_object('detail', COALESCE(json_extract(diagnostic_json, '$.detail'), json_extract(?2, '$.detail'))),
                 0, ?3 FROM burn_check_candidate
               WHERE activation_scope_key = ?1 AND state IN ('claimed', 'dispatching')
               ON CONFLICT(candidate_id) DO NOTHING",
            params![self.activation_scope_key, serde_json::to_string(&serde_json::json!({"detail": "dispatch outcome unknown or retry limit exhausted"}))?, now_epoch()])?;
        let failed = tx.execute("UPDATE burn_check_candidate SET state = 'execution_failed', claim_owner = NULL, updated_at_epoch = ?2
            WHERE activation_scope_key = ?1 AND state IN ('claimed', 'dispatching')", params![self.activation_scope_key, now_epoch()])?;
        let answered = tx.execute("UPDATE burn_check_candidate SET claim_owner = ?2, claim_fence = claim_fence + 1 WHERE activation_scope_key = ?1 AND state = 'answered'", params![self.activation_scope_key, self.owner])?;
        tx.commit()?;
        ::tracing::debug!(
            event = "burn_check_startup_recovery",
            status = "committed",
            requeued_candidates = requeued,
            failed_candidates = failed,
            answered_candidates = answered,
            unknown_attempts,
            elapsed_ms = started.elapsed().as_millis() as u64
        );
        if unknown_attempts > 0 {
            ::tracing::warn!(
                event = "burn_check_delivery_unknown",
                reason = "restart_after_dispatch_intent",
                attempts = unknown_attempts,
                automatic_resend = false
            );
        }
        Ok(())
    }

    pub(super) fn recovery_page(
        &self,
        after: Option<&CandidateId>,
        limit: usize,
    ) -> Result<Vec<RecoveryRecord>> {
        let started = Instant::now();
        let trace_after = after.map(|id| id.0);
        let mut examined = 0;
        let mut skipped = 0;
        ensure!(
            (1..=500).contains(&limit),
            "recovery page limit must be between 1 and 500"
        );
        let mut after = after.map(|id| hex(&id.0)).unwrap_or_default();
        loop {
            let connection = self.store.lock();
            let mut statement = connection.prepare(
                "SELECT c.candidate_id, c.state, c.prior_status, c.claim_fence,
                    c.check_id, c.generation
                FROM burn_check_candidate c JOIN burn_check_activation a USING(check_id, activation_scope_key)
                WHERE c.activation_scope_key = ?1 AND c.candidate_id > ?2 AND c.state IN ('pending', 'answered')
                  AND c.generation = a.generation AND a.enabled = 1 ORDER BY c.candidate_id LIMIT ?3",
            )?;
            let rows = statement
                .query_map(
                    params![self.activation_scope_key, after, limit as i64],
                    |row| {
                        Ok(RecoveryCandidateRow {
                            id: row.get(0)?,
                            state: row.get(1)?,
                            prior_status: row.get(2)?,
                            fence: row.get(3)?,
                            check_id: row.get(4)?,
                            generation: row.get(5)?,
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            drop(connection);
            if rows.is_empty() {
                self.trace_recovery_page(trace_after, examined, skipped, &[], started.elapsed());
                return Ok(Vec::new());
            }
            #[cfg(test)]
            self.store
                .burn_check_test_stage(super::super::test_hooks::Stage::AfterRecoveryPage);
            let mut records = Vec::with_capacity(rows.len());
            for row in rows {
                examined += 1;
                after = row.id.clone();
                if let Some(record) = self.recover_row(&row)? {
                    records.push(record);
                } else {
                    skipped += 1;
                }
            }
            if !records.is_empty() {
                self.trace_recovery_page(
                    trace_after,
                    examined,
                    skipped,
                    &records,
                    started.elapsed(),
                );
                return Ok(records);
            }
        }
    }

    fn recover_row(&self, row: &RecoveryCandidateRow) -> Result<Option<RecoveryRecord>> {
        let mut connection = self.store.lock();
        // Keep saved inputs, decision progress, and rejection in one transaction.
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM burn_check_candidate c
             JOIN burn_check_activation a USING(check_id, activation_scope_key)
             WHERE c.candidate_id = ?1 AND c.activation_scope_key = ?2
               AND c.state = ?3 AND c.claim_fence = ?4 AND c.prior_status IS ?5
               AND c.check_id = ?6 AND c.generation = ?7
               AND c.generation = a.generation AND a.enabled = 1)",
            params![
                row.id,
                self.activation_scope_key,
                row.state,
                row.fence,
                row.prior_status,
                row.check_id,
                row.generation
            ],
            |row| row.get(0),
        )?;
        if !current {
            return Ok(None);
        }
        let record = match self.recover_row_in(&tx, row) {
            Ok(record) => Some(record),
            Err(error)
                if error.is::<rusqlite::Error>() || error.is::<RecoveryActivationMissing>() =>
            {
                return Err(error);
            }
            Err(_) => {
                self.reject_recovery_row(&tx, row)?;
                None
            }
        };
        tx.commit()?;
        if record.is_none() {
            ::tracing::warn!(
                event = "burn_check_recovery_rejected",
                check_ref = check_ref(&row.check_id),
                activation_ref = activation_ref(&ActivationId {
                    scope: self.scope.clone(),
                    definition_id: row.check_id.clone(),
                    generation: Generation(row.generation)
                }),
                candidate_ref = parse_candidate_id(&row.id)
                    .ok()
                    .map(|id| candidate_id_ref(&id)),
                generation = row.generation,
                claim_fence = row.fence,
                reason = "saved_work_invalid_or_incompatible"
            );
        }
        Ok(record)
    }

    fn recover_row_in(
        &self,
        connection: &rusqlite::Connection,
        row: &RecoveryCandidateRow,
    ) -> Result<RecoveryRecord> {
        let id = &row.id;
        let state = row.state.as_str();
        let activation_id = ActivationId {
            scope: self.scope.clone(),
            definition_id: row.check_id.clone(),
            generation: Generation(row.generation),
        };
        let activation = self
            .registered_activation(&activation_id)
            .ok_or(RecoveryActivationMissing)?;
        let frozen: String = connection.query_row(
            "SELECT frozen_input_json FROM burn_check_candidate WHERE candidate_id = ?1",
            [id],
            |row| row.get(0),
        )?;
        let candidate: FrozenCandidate = serde_json::from_str(&frozen)?;
        ensure!(
            hex(&candidate.fingerprint) == *id && candidate.activation == activation.id,
            "saved candidate identity is invalid"
        );
        let preparation: Option<String> = connection.query_row(
            "SELECT preparation_json FROM burn_check_candidate WHERE candidate_id = ?1",
            [id],
            |row| row.get(0),
        )?;
        let has_preparation = preparation.is_some();
        if let Some(preparation) = preparation {
            match antiburn_checks::preparation::decode_preparation_record(preparation.as_bytes())
                .map_err(|error| anyhow::anyhow!(error.detail))?
            {
                PreparationRecord::Ready(prepared) => {
                    ensure!(
                        !activation.compiled.is_local(),
                        "local work has a provider preparation"
                    );
                    prepared.validate_for_declarations(&activation.compiled.decisions)?;
                }
                PreparationRecord::Local(_) => ensure!(
                    activation.compiled.is_local(),
                    "model work has a local preparation"
                ),
                PreparationRecord::Rejected(_) => {
                    anyhow::bail!("unfinished work has a rejected preparation")
                }
            }
        } else {
            ensure!(
                state == "pending",
                "answered work requires its saved preparation"
            );
        }
        let progress = self.read_decision_rows(connection, id, &activation)?;
        #[cfg(test)]
        self.store
            .burn_check_test_stage(super::super::test_hooks::Stage::AfterRecoveryProgress);
        ensure!(
            has_preparation || progress.attempts.is_empty(),
            "started work requires its exact saved preparation"
        );
        let validated = validate_decision_progress(
            &activation.compiled.decisions,
            &activation.compiled.question_requirements(),
            &progress,
        )?;
        let work = if state == "answered" {
            let answer: Option<String> = connection.query_row(
                "SELECT answer_json FROM burn_check_candidate WHERE candidate_id = ?1",
                [id],
                |row| row.get(0),
            )?;
            let answer = answer.context("answered work has no saved result")?;
            let evaluated = decode_results(answer.as_bytes())?.validate_for_declarations(
                &activation.compiled.decisions,
                &activation.compiled.question_requirements(),
            )?;
            let DecisionProgressState::Complete(durable) = validated.state else {
                anyhow::bail!("answered work has incomplete durable progress");
            };
            ensure!(
                evaluated == durable,
                "saved complete result does not match durable answers"
            );
            let claimed = ClaimedCandidate {
                id: CandidateId::from(&candidate),
                candidate,
                prior_status: parse_status(row.prior_status.clone())?,
                fence: row.fence,
            };
            RecoveryWork::Answered { claimed, evaluated }
        } else {
            ensure!(
                activation.compiled.is_local()
                    || matches!(validated.state, DecisionProgressState::Next { .. }),
                "pending work has terminal or complete progress"
            );
            RecoveryWork::Pending {
                candidate,
                resumed: !progress.attempts.is_empty(),
            }
        };
        Ok(RecoveryRecord { activation, work })
    }

    fn reject_recovery_row(
        &self,
        connection: &rusqlite::Connection,
        row: &RecoveryCandidateRow,
    ) -> Result<()> {
        let detail = "The saved assessment is invalid or incompatible.";
        let diagnostic = EvaluationDiagnostic {
            code: antiburn_checks::engine::EvaluationFailureCode::InvalidDecision,
            detail: detail.into(),
        };
        let state = if row.state == "pending" {
            "not_assessed"
        } else {
            "execution_failed"
        };
        connection.execute(
            "INSERT INTO burn_check_assessment_history
            SELECT 'assessment:' || candidate_id, candidate_id, check_id, activation_scope_key,
                generation, branch_key, trigger_position, ?3, answer_json, ?4, 0, ?5
            FROM burn_check_candidate WHERE candidate_id = ?1 AND activation_scope_key = ?2
                AND state = ?6 AND claim_fence = ?7 ON CONFLICT(candidate_id) DO NOTHING",
            params![
                row.id,
                self.activation_scope_key,
                state,
                serde_json::to_string(&serde_json::json!({"detail": detail}))?,
                now_epoch(),
                row.state,
                row.fence
            ],
        )?;
        connection.execute("UPDATE burn_check_candidate SET state = ?3, diagnostic_json = ?4,
            claim_owner = NULL, claim_fence = claim_fence + 1, updated_at_epoch = ?5
            WHERE candidate_id = ?1 AND activation_scope_key = ?2 AND state = ?6 AND claim_fence = ?7",
            params![row.id, self.activation_scope_key, state, serde_json::to_string(&diagnostic)?, now_epoch(), row.state, row.fence])?;
        Ok(())
    }
}
