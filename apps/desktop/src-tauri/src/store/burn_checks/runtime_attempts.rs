use super::*;

type ClaimCandidateRow = (String, String, u64, Option<u64>, Option<String>);

impl StoreCheckStore {
    pub(super) fn release_unattempted_claim(
        &self,
        claimed: &ClaimedCandidate,
    ) -> Result<ClaimOutcome> {
        let claim = self.claim_record(claimed)?;
        let coalesces = self
            .activations
            .lock()
            .expect("activation mutex is not poisoned")
            .get(&claimed.candidate.activation)
            .is_some_and(|active| active.compiled.trigger.window.is_some());
        let next_fence = integer(
            claimed
                .fence
                .checked_add(1)
                .context("claim fence is exhausted")?,
        )?;
        let mut connection = self.store.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("UPDATE burn_check_candidate SET state = CASE WHEN ?4
                AND NOT EXISTS(SELECT 1 FROM burn_check_evaluation_attempt a WHERE a.candidate_id = ?1) AND EXISTS(
                SELECT 1 FROM burn_check_candidate newer WHERE newer.check_id = burn_check_candidate.check_id
                AND newer.activation_scope_key = burn_check_candidate.activation_scope_key AND newer.generation = burn_check_candidate.generation
                AND newer.branch_key = burn_check_candidate.branch_key AND newer.trigger_position > burn_check_candidate.trigger_position
                AND newer.state NOT IN ('stale', 'cancelled', 'coalesced')) THEN 'coalesced' ELSE 'pending' END,
                claim_owner = NULL, claim_fence = ?5, updated_at_epoch = ?6
            WHERE candidate_id = ?1 AND claim_owner = ?2 AND claim_fence = ?3 AND state = 'claimed'
                AND NOT EXISTS(SELECT 1 FROM burn_check_evaluation_attempt a WHERE a.candidate_id = ?1
                  AND (a.state <> 'settled' OR a.outcome NOT IN ('answer', 'safe_transient')
                    OR (a.outcome = 'safe_transient' AND a.attempt_number = 2)))",
            params![claim.candidate_id, claim.owner, claim.fence, coalesces, next_fence, now_epoch()])?;
        if changed == 0 {
            return Ok(ClaimOutcome::Stale);
        }
        let pending: bool = tx.query_row(
            "SELECT state = 'pending' FROM burn_check_candidate WHERE candidate_id = ?1",
            [&claim.candidate_id],
            |row| row.get(0),
        )?;
        tx.commit()?;
        ::tracing::debug!(
            event = "burn_check_claim_released",
            activation_ref = activation_ref(&claimed.candidate.activation),
            candidate_ref = candidate_ref(&claimed.candidate),
            generation = claimed.candidate.activation.generation.0,
            claim_fence = claimed.fence,
            next_fence,
            status = if pending { "pending" } else { "coalesced" },
            reason = "pre_dispatch_release",
        );
        Ok(if pending {
            ClaimOutcome::Current
        } else {
            ClaimOutcome::Stale
        })
    }

    pub(super) fn saved_preparation(
        &self,
        claimed: &ClaimedCandidate,
    ) -> Result<Option<PreparationRecord>> {
        let claim = self.claim_record(claimed)?;
        let saved: Option<String> = self.store.lock().query_row(
            "SELECT preparation_json FROM burn_check_candidate WHERE candidate_id = ?1 AND claim_owner = ?2 AND claim_fence = ?3",
            params![claim.candidate_id, claim.owner, claim.fence], |row| row.get(0),
        )?;
        Ok(saved.map(|saved| {
            match antiburn_checks::preparation::decode_preparation_record(saved.as_bytes()) {
                Ok(preparation) => preparation,
                Err(failure) => PreparationRecord::Rejected(failure),
            }
        }))
    }

    pub(super) fn save_preparation(
        &self,
        claimed: &ClaimedCandidate,
        preparation: &PreparationRecord,
    ) -> Result<ClaimOutcome> {
        let started = Instant::now();
        ensure!(
            !matches!(preparation, PreparationRecord::Local(_)),
            "local preparation must be saved with its answer and cooldown"
        );
        self.require_model_claim(claimed)?;
        let activation = self
            .registered_activation(&claimed.candidate.activation)
            .context("model preparation requires its registered activation")?;
        if let PreparationRecord::Ready(prepared) = preparation {
            prepared.validate_for_declarations(&activation.compiled.decisions)?;
        }
        let claim = self.claim_record(claimed)?;
        let saved = encode_preparation(preparation)?;
        let mut connection = self.store.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<Option<String>> = tx.query_row(
            "SELECT preparation_json FROM burn_check_candidate WHERE candidate_id = ?1 AND claim_owner = ?2 AND claim_fence = ?3 AND state = 'claimed'",
            params![claim.candidate_id, claim.owner, claim.fence], |row| row.get(0),
        ).optional()?;
        let Some(previous) = previous else {
            return Ok(ClaimOutcome::Stale);
        };
        let newly_saved = previous.is_none();
        if let Some(previous) = previous {
            ensure!(
                antiburn_checks::preparation::decode_preparation_record(previous.as_bytes())
                    .map_err(|error| anyhow::anyhow!(error.detail))?
                    == *preparation,
                "decision preparation is immutable"
            );
        } else {
            let attempted: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM burn_check_evaluation_attempt WHERE candidate_id = ?1)", [&claim.candidate_id], |row| row.get(0))?;
            ensure!(
                !attempted,
                "cannot prepare a retry without its original request"
            );
            tx.execute("UPDATE burn_check_candidate SET preparation_json = ?4 WHERE candidate_id = ?1 AND claim_owner = ?2 AND claim_fence = ?3", params![claim.candidate_id, claim.owner, claim.fence, saved])?;
        }
        tx.commit()?;
        if newly_saved {
            let failure = match preparation {
                PreparationRecord::Rejected(failure) => Some(failure),
                _ => None,
            };
            ::tracing::debug!(
                event = "burn_check_preparation_committed",
                activation_ref = activation_ref(&claimed.candidate.activation),
                candidate_ref = candidate_ref(&claimed.candidate),
                generation = claimed.candidate.activation.generation.0,
                claim_fence = claimed.fence,
                status = match preparation {
                    PreparationRecord::Ready(_) => "ready",
                    PreparationRecord::Rejected(_) => "rejected",
                    PreparationRecord::Local(_) => "local",
                },
                preparation_failure_code = ?failure.map(|failure| failure.code),
                no_fit_reason = ?failure.and_then(|failure| failure.context.no_fit_reason),
                budget_kind = ?failure.and_then(|failure| failure.context.budget.as_ref().map(|budget| budget.kind)),
                budget_observed = failure.and_then(|failure| failure.context.budget.as_ref().map(|budget| budget.observed)),
                budget_maximum = failure.and_then(|failure| failure.context.budget.as_ref().map(|budget| budget.maximum)),
                elapsed_ms = started.elapsed().as_millis() as u64,
            );
        }
        Ok(ClaimOutcome::Current)
    }

    pub(super) fn claim(
        &self,
        candidate: &FrozenCandidate,
        now_ms: u64,
        bypass: bool,
    ) -> Result<CandidateClaim> {
        let started = Instant::now();
        ensure!(
            candidate.activation.scope == self.scope,
            "candidate belongs to another scope"
        );
        let Some(activation) = self.registered_activation(&candidate.activation) else {
            return Ok(CandidateClaim::Stale);
        };
        let local = activation.compiled.is_local();
        let id = hex(&candidate.fingerprint);
        let mut connection = self.store.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved: Option<ClaimCandidateRow> = tx.query_row(
            "SELECT c.state, c.frozen_input_json, c.claim_fence, c.started_cooldown_until_epoch, c.prior_status FROM burn_check_candidate c
             JOIN burn_check_activation a USING(check_id, activation_scope_key)
             WHERE c.candidate_id = ?1 AND c.activation_scope_key = ?2 AND c.generation = a.generation AND a.enabled = 1",
            params![id, self.activation_scope_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        ).optional()?;
        let Some((state, frozen, fence, started_cooldown, saved_prior)) = saved else {
            return Ok(CandidateClaim::Stale);
        };
        if state != "pending" {
            return Ok(CandidateClaim::Stale);
        }
        if !crate::store::native_tool_results::request_origin::new_candidate_request_origin_current_in(&tx, &id)?
            || !crate::store::native_tool_results::observers::typed_dependencies_current_in(&tx, &id)? {
            tx.execute("UPDATE burn_check_candidate SET state = 'stale' WHERE candidate_id = ?1 AND state = 'pending'", [&id])?;
            tx.commit()?;
            ::tracing::debug!(
                event = "burn_check_candidate_invalidated",
                activation_ref = activation_ref(&candidate.activation),
                candidate_ref = candidate_ref(candidate),
                generation = candidate.activation.generation.0,
                reason = "dependencies_changed",
                status = "stale"
            );
            return Ok(CandidateClaim::Stale);
        }
        ensure!(
            serde_json::from_str::<FrozenCandidate>(&frozen)? == *candidate,
            "claim does not match frozen candidate"
        );
        let busy: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM burn_check_candidate WHERE check_id = ?1 AND activation_scope_key = ?2
              AND generation = ?3 AND branch_key = ?4 AND state IN ('claimed', 'dispatching', 'answered'))",
            params![candidate.activation.definition_id, self.activation_scope_key, integer(candidate.activation.generation.0)?, candidate.branch.0], |row| row.get(0),
        )?;
        let until: Option<u64> = tx.query_row(
            "SELECT cooldown_until_epoch FROM burn_check_cooldown WHERE check_id = ?1 AND activation_scope_key = ?2 AND generation = ?3 AND branch_key = ?4",
            params![candidate.activation.definition_id, self.activation_scope_key, integer(candidate.activation.generation.0)?, candidate.branch.0], |row| row.get(0),
        ).optional()?;
        let resumed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM burn_check_evaluation_attempt WHERE candidate_id = ?1)",
            [&id],
            |row| row.get(0),
        )?;
        let own_cooldown =
            resumed && started_cooldown.is_some_and(|own| until.is_some_and(|until| until <= own));
        if busy
            || (!bypass
                && !own_cooldown
                && until.is_some_and(|until| now_ms < until.saturating_mul(1000)))
        {
            return Ok(CandidateClaim::NotDue {
                retry_at_unix_ms: until.map_or(now_ms.saturating_add(1000), |until| {
                    until.saturating_mul(1000).max(now_ms.saturating_add(1))
                }),
            });
        }
        ensure!(
            !local || !resumed,
            "local work cannot retain provider attempts"
        );
        let prior: Option<String> = if resumed {
            saved_prior
        } else {
            tx.query_row(
            "SELECT status FROM burn_check_current_status WHERE check_id = ?1 AND activation_scope_key = ?2 AND branch_key = ?3 AND generation = ?4",
            params![candidate.activation.definition_id, self.activation_scope_key, candidate.branch.0, integer(candidate.activation.generation.0)?], |row| row.get(0),
        ).optional()?
        };
        let fence = fence.checked_add(1).context("claim fence is exhausted")?;
        tx.execute("UPDATE burn_check_candidate SET state = 'claimed', claim_owner = ?2, claim_fence = ?3,
            prior_status = ?4, claim_epoch = ?5, updated_at_epoch = ?5 WHERE candidate_id = ?1 AND state = 'pending'",
            params![id, self.owner, integer(fence)?, prior, integer(now_ms / 1000)?])?;
        tx.commit()?;
        ::tracing::debug!(
            event = "burn_check_candidate_claimed",
            activation_ref = activation_ref(&candidate.activation),
            candidate_ref = candidate_ref(candidate),
            generation = candidate.activation.generation.0,
            claim_fence = fence,
            resumed,
            local,
            elapsed_ms = started.elapsed().as_millis() as u64,
        );
        Ok(CandidateClaim::Claimed(Box::new(ClaimedCandidate {
            id: CandidateId::from(candidate),
            candidate: candidate.clone(),
            prior_status: parse_status(prior)?,
            fence,
        })))
    }

    pub(super) fn dispatch(
        &self,
        claimed: &ClaimedCandidate,
        now_ms: u64,
        cooldown: Duration,
        attempt: &DecisionAttempt,
    ) -> Result<ClaimOutcome> {
        let started = Instant::now();
        self.require_model_claim(claimed)?;
        let claim = self.claim_record(claimed)?;
        attempt.validate()?;
        let Some(activation) = self.registered_activation(&claimed.candidate.activation) else {
            return Ok(ClaimOutcome::Stale);
        };
        let mut connection = self.store.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !self.claim_is_current_in(&tx, &claim, &activation)? {
            tx.execute(
                "UPDATE burn_check_candidate SET state = 'stale', updated_at_epoch = ?4
                WHERE candidate_id = ?1 AND claim_owner = ?2 AND claim_fence = ?3
                  AND state IN ('claimed', 'dispatching', 'answered')",
                params![claim.candidate_id, claim.owner, claim.fence, now_epoch()],
            )?;
            tx.commit()?;
            ::tracing::debug!(
                event = "burn_check_dispatch_blocked",
                activation_ref = activation_ref(&claimed.candidate.activation),
                candidate_ref = candidate_ref(&claimed.candidate),
                decision_ref = decision_ref(&claimed.candidate, attempt),
                generation = claimed.candidate.activation.generation.0,
                claim_fence = claimed.fence,
                attempt_number = attempt.number,
                reason = "claim_or_source_changed"
            );
            return Ok(ClaimOutcome::Stale);
        }
        let frozen: String = tx.query_row(
            "SELECT frozen_input_json FROM burn_check_candidate WHERE candidate_id = ?1",
            [&claim.candidate_id],
            |row| row.get(0),
        )?;
        ensure!(
            serde_json::from_str::<FrozenCandidate>(&frozen)? == claimed.candidate,
            "dispatch does not match the exact frozen candidate"
        );
        let progress = self.read_decision_progress(&tx, &claim, &activation)?;
        let validated = validate_decision_progress(
            &activation.compiled.decisions,
            &activation.compiled.question_requirements(),
            &progress,
        )?;
        ensure!(
            matches!(validated.state, DecisionProgressState::Next { attempt: ref next, .. } if next == attempt),
            "dispatch is not the next declared decision attempt"
        );
        let saved: Option<String> = tx.query_row(
            "SELECT preparation_json FROM burn_check_candidate WHERE candidate_id = ?1",
            [&claim.candidate_id],
            |row| row.get(0),
        )?;
        let preparation = saved.context("dispatch requires the complete saved preparation")?;
        let PreparationRecord::Ready(prepared) =
            antiburn_checks::preparation::decode_preparation_record(preparation.as_bytes())
                .map_err(|error| anyhow::anyhow!(error.detail))?
        else {
            anyhow::bail!("dispatch requires a ready model preparation");
        };
        prepared.validate_for_declarations(&activation.compiled.decisions)?;
        let changed = tx.execute("UPDATE burn_check_candidate SET state = 'dispatching', updated_at_epoch = ?4, diagnostic_json = NULL
            WHERE candidate_id = ?1 AND claim_owner = ?2 AND claim_fence = ?3 AND state = 'claimed'
            AND EXISTS(SELECT 1 FROM burn_check_activation a WHERE a.check_id = burn_check_candidate.check_id
              AND a.activation_scope_key = burn_check_candidate.activation_scope_key AND a.generation = burn_check_candidate.generation AND a.enabled = 1)",
            params![claim.candidate_id, claim.owner, claim.fence, integer(now_ms / 1000)?])?;
        if changed == 0 {
            return Ok(ClaimOutcome::Stale);
        }
        tx.execute("INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, decision_name, attempt_number, state, updated_at_epoch)
            VALUES (?1, ?2, ?3, ?4, 'dispatch_intent', ?5)",
            params![attempt_id(&claimed.id.0, attempt), claim.candidate_id, attempt.decision, attempt.number, integer(now_ms / 1000)?])?;
        if progress.attempts.is_empty() {
            let cooldown_ms = u64::try_from(cooldown.as_millis())
                .context("cooldown exceeds supported duration")?;
            let until = if cooldown.is_zero() {
                0
            } else {
                now_ms.saturating_add(cooldown_ms).saturating_add(999) / 1000
            };
            tx.execute("UPDATE burn_check_candidate SET started_cooldown_until_epoch = ?2 WHERE candidate_id = ?1",
                params![claim.candidate_id, integer(until)?])?;
            if !cooldown.is_zero() {
                tx.execute("INSERT INTO burn_check_cooldown SELECT check_id, activation_scope_key, generation, branch_key, ?2 FROM burn_check_candidate WHERE candidate_id = ?1
                ON CONFLICT(check_id, activation_scope_key, generation, branch_key) DO UPDATE SET cooldown_until_epoch = MAX(cooldown_until_epoch, excluded.cooldown_until_epoch)",
                params![claim.candidate_id, integer(until)?])?;
            }
        }
        tx.commit()?;
        ::tracing::debug!(
            event = "burn_check_dispatch_intent_committed",
            activation_ref = activation_ref(&claimed.candidate.activation),
            candidate_ref = candidate_ref(&claimed.candidate),
            decision_ref = decision_ref(&claimed.candidate, attempt),
            generation = claimed.candidate.activation.generation.0,
            claim_fence = claimed.fence,
            attempt_number = attempt.number,
            completed_attempts = progress.attempts.len(),
            status = "dispatch_intent",
            elapsed_ms = started.elapsed().as_millis() as u64,
        );
        Ok(ClaimOutcome::Current)
    }

    pub(super) fn settle_failure(
        &self,
        claimed: &ClaimedCandidate,
        attempt: &DecisionAttempt,
        outcome: AttemptFailure,
        diagnostic: &EvaluationDiagnostic,
    ) -> Result<ClaimOutcome> {
        let started = Instant::now();
        self.require_model_claim(claimed)?;
        attempt.validate()?;
        let claim = self.claim_record(claimed)?;
        let mut connection = self.store.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcome_name = match outcome {
            AttemptFailure::SafeTransient => "safe_transient",
            AttemptFailure::Unknown => "unknown",
            AttemptFailure::Permanent => "permanent",
        };
        let changed = tx.execute("UPDATE burn_check_evaluation_attempt SET state = 'settled', outcome = ?3, updated_at_epoch = ?4
            WHERE candidate_id = ?1 AND attempt_id = ?2 AND state = 'dispatch_intent'
              AND EXISTS(SELECT 1 FROM burn_check_candidate c WHERE c.candidate_id = ?1 AND c.claim_owner = ?5 AND c.claim_fence = ?6)",
            params![claim.candidate_id, attempt_id(&claimed.id.0, attempt), outcome_name, now_epoch(), claim.owner, claim.fence])?;
        ensure!(changed == 1, "attempt claim is stale or already settled");
        tx.execute("UPDATE burn_check_candidate SET diagnostic_json = ?4 WHERE candidate_id = ?1 AND claim_owner = ?2 AND claim_fence = ?3",
            params![claim.candidate_id, claim.owner, claim.fence, serde_json::to_string(diagnostic)?])?;
        if outcome == AttemptFailure::SafeTransient && attempt.number == 1 {
            tx.execute("UPDATE burn_check_candidate SET state = 'claimed' WHERE candidate_id = ?1 AND state = 'dispatching'", [&claim.candidate_id])?;
        }
        let current = match self.registered_activation(&claimed.candidate.activation) {
            Some(activation) => self.claim_is_current_in(&tx, &claim, &activation)?,
            None => false,
        };
        if !current {
            tx.execute("UPDATE burn_check_candidate SET state = 'stale' WHERE candidate_id = ?1
                AND claim_owner = ?2 AND claim_fence = ?3 AND state IN ('claimed', 'dispatching', 'answered')",
                params![claim.candidate_id, claim.owner, claim.fence])?;
            tx.execute("INSERT INTO burn_check_assessment_history
                SELECT ?2, candidate_id, check_id, activation_scope_key, generation, branch_key, trigger_position,
                    state, answer_json, '{}', 0, ?3 FROM burn_check_candidate
                WHERE candidate_id = ?1 AND state IN ('stale', 'cancelled')
                ON CONFLICT(candidate_id) DO NOTHING",
                params![claim.candidate_id, format!("assessment:{}", claim.candidate_id), now_epoch()])?;
        }
        tx.commit()?;
        ::tracing::warn!(
            event = "burn_check_attempt_failed",
            activation_ref = activation_ref(&claimed.candidate.activation),
            candidate_ref = candidate_ref(&claimed.candidate),
            decision_ref = decision_ref(&claimed.candidate, attempt),
            generation = claimed.candidate.activation.generation.0,
            claim_fence = claimed.fence,
            attempt_number = attempt.number,
            status = outcome_name,
            reason = ?diagnostic.code,
            publication_current = current,
            retry_allowed = current && outcome == AttemptFailure::SafeTransient && attempt.number == 1,
            elapsed_ms = started.elapsed().as_millis() as u64,
        );
        Ok(if current {
            ClaimOutcome::Current
        } else {
            ClaimOutcome::Stale
        })
    }

    pub(super) fn save_decision_answer(
        &self,
        claimed: &ClaimedCandidate,
        attempt: &DecisionAttempt,
        evaluated: &DecisionResult,
    ) -> Result<ClaimOutcome> {
        let started = Instant::now();
        self.require_model_claim(claimed)?;
        attempt.validate()?;
        ensure!(
            evaluated.decision() == attempt.decision && evaluated.attempts() == attempt.number,
            "decision answer does not match its exact attempt"
        );
        antiburn_checks::results::measure_decision_results_with_next(&[], evaluated)?;
        #[cfg(test)]
        self.store
            .burn_check_test_stage(super::super::test_hooks::Stage::BeforeAnswer);
        let claim = self.claim_record(claimed)?;
        let answer = serde_json::to_string(evaluated)?;
        let activation = self.registered_activation(&claimed.candidate.activation);
        let mut connection = self.store.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("UPDATE burn_check_evaluation_attempt SET state = 'settled', outcome = 'answer', response_json = ?3, updated_at_epoch = ?4
            WHERE candidate_id = ?1 AND attempt_id = ?2 AND state = 'dispatch_intent'
              AND EXISTS(SELECT 1 FROM burn_check_candidate c WHERE c.candidate_id = ?1 AND c.claim_owner = ?5 AND c.claim_fence = ?6)",
            params![claim.candidate_id, attempt_id(&claimed.id.0, attempt), answer, now_epoch(), claim.owner, claim.fence])?;
        ensure!(changed == 1, "answer has no exact dispatch intent");
        let current = match &activation {
            Some(activation) => self.claim_is_current_in(&tx, &claim, activation)?,
            None => false,
        };
        let complete = if let Some(activation) = &activation {
            let progress = self.read_decision_progress(&tx, &claim, activation)?;
            let validated = validate_decision_progress(
                &activation.compiled.decisions,
                &activation.compiled.question_requirements(),
                &progress,
            )?;
            match validated.state {
                DecisionProgressState::Complete(complete) => {
                    Some(serde_json::to_string(&complete)?)
                }
                DecisionProgressState::Next { .. } => None,
                DecisionProgressState::Terminal(_) => {
                    anyhow::bail!("decision answer leaves terminal progress")
                }
            }
        } else {
            None
        };
        tx.execute(
            "UPDATE burn_check_candidate SET state = CASE
                WHEN ?4 THEN CASE WHEN ?5 IS NULL THEN 'claimed' ELSE 'answered' END
                WHEN state IN ('claimed', 'dispatching', 'answered') THEN 'stale' ELSE state END,
                answer_json = COALESCE(?5, answer_json), updated_at_epoch = ?6
            WHERE candidate_id = ?1 AND claim_owner = ?2 AND claim_fence = ?3",
            params![
                claim.candidate_id,
                claim.owner,
                claim.fence,
                current,
                complete,
                now_epoch()
            ],
        )?;
        if !current {
            tx.execute(
                "INSERT INTO burn_check_assessment_history
                SELECT ?2, candidate_id, check_id, activation_scope_key, generation, branch_key,
                    trigger_position, state, answer_json, '{}', 0, ?3 FROM burn_check_candidate
                WHERE candidate_id = ?1 AND state IN ('stale', 'cancelled')
                ON CONFLICT(candidate_id) DO UPDATE SET answer_json = excluded.answer_json",
                params![
                    claim.candidate_id,
                    format!("assessment:{}", claim.candidate_id),
                    now_epoch()
                ],
            )?;
        }
        tx.commit()?;
        drop(connection);
        ::tracing::debug!(
            event = "burn_check_answer_committed",
            activation_ref = activation_ref(&claimed.candidate.activation),
            candidate_ref = candidate_ref(&claimed.candidate),
            decision_ref = decision_ref(&claimed.candidate, attempt),
            generation = claimed.candidate.activation.generation.0,
            claim_fence = claimed.fence,
            attempt_number = attempt.number,
            publication_current = current,
            complete = complete.is_some(),
            elapsed_ms = started.elapsed().as_millis() as u64,
        );
        #[cfg(test)]
        self.store
            .burn_check_test_stage(super::super::test_hooks::Stage::AfterAnswer);
        Ok(if current {
            ClaimOutcome::Current
        } else {
            ClaimOutcome::Stale
        })
    }

    pub(super) fn terminal(
        &self,
        claimed: &ClaimedCandidate,
        detail: &str,
        not_assessed: bool,
    ) -> Result<ClaimOutcome> {
        self.terminal_with_diagnostic(claimed, detail, not_assessed, None, None)
    }

    pub(super) fn terminal_with_diagnostic(
        &self,
        claimed: &ClaimedCandidate,
        detail: &str,
        not_assessed: bool,
        preparation: Option<&PreparationRecord>,
        diagnostic: Option<&EvaluationDiagnostic>,
    ) -> Result<ClaimOutcome> {
        let claim = self.claim_record(claimed)?;
        let preparation_failure = match preparation {
            Some(PreparationRecord::Rejected(failure)) => Some(failure),
            _ => None,
        };
        let preparation = preparation.map(encode_preparation).transpose()?;
        let state = if not_assessed {
            "not_assessed"
        } else {
            "execution_failed"
        };
        let mut connection = self.store.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved_state: Option<String> = tx.query_row(
            "SELECT state FROM burn_check_candidate WHERE candidate_id = ?1 AND claim_owner = ?2 AND claim_fence = ?3",
            params![claim.candidate_id, claim.owner, claim.fence], |row| row.get(0),
        ).optional()?;
        let Some(saved_state) = saved_state else {
            return Ok(ClaimOutcome::Stale);
        };
        let stale = matches!(saved_state.as_str(), "stale" | "cancelled");
        ensure!(
            stale || matches!(saved_state.as_str(), "claimed" | "dispatching" | "answered"),
            "terminal claim is already settled"
        );
        let state = if stale { saved_state.as_str() } else { state };
        let inserted = tx.execute("INSERT INTO burn_check_assessment_history
            SELECT ?4, candidate_id, check_id, activation_scope_key, generation, branch_key, trigger_position, ?5, answer_json, ?6, 0, ?7
              FROM burn_check_candidate WHERE candidate_id = ?1 AND claim_owner = ?2 AND claim_fence = ?3
              ON CONFLICT(candidate_id) DO NOTHING",
            params![claim.candidate_id, claim.owner, claim.fence, format!("assessment:{}", claim.candidate_id), state, serde_json::to_string(&serde_json::json!({"detail": detail}))?, now_epoch()])?;
        tx.execute("UPDATE burn_check_candidate SET state = ?4, updated_at_epoch = ?5,
            preparation_json = COALESCE(?6, preparation_json), diagnostic_json = COALESCE(diagnostic_json, ?7)
            WHERE candidate_id = ?1 AND claim_owner = ?2 AND claim_fence = ?3",
            params![claim.candidate_id, claim.owner, claim.fence, state, now_epoch(), preparation, diagnostic.map(serde_json::to_string).transpose()?])?;
        tx.commit()?;
        if inserted > 0 || saved_state != state {
            ::tracing::debug!(
                event = "burn_check_candidate_settled",
                activation_ref = activation_ref(&claimed.candidate.activation),
                candidate_ref = candidate_ref(&claimed.candidate),
                generation = claimed.candidate.activation.generation.0,
                claim_fence = claimed.fence,
                status = if stale { "stale" } else if not_assessed { "not_assessed" } else { "execution_failed" },
                preparation_reason = ?preparation_failure.map(|failure| failure.code),
                no_fit_reason = ?preparation_failure.and_then(|failure| failure.context.no_fit_reason),
                budget_kind = ?preparation_failure.and_then(|failure| failure.context.budget.as_ref().map(|budget| budget.kind)),
                budget_observed = preparation_failure.and_then(|failure| failure.context.budget.as_ref().map(|budget| budget.observed)),
                budget_maximum = preparation_failure.and_then(|failure| failure.context.budget.as_ref().map(|budget| budget.maximum)),
                evaluation_reason = ?diagnostic.map(|diagnostic| diagnostic.code),
            );
        }
        Ok(if stale {
            ClaimOutcome::Stale
        } else {
            ClaimOutcome::Current
        })
    }
}

pub(super) fn encode_preparation(preparation: &PreparationRecord) -> Result<String> {
    antiburn_checks::bounded_json::measure_json(
        preparation,
        antiburn_checks::prepared_assessment::PREPARATION_LIMITS,
    )?;
    let saved = serde_json::to_string(preparation)?;
    antiburn_checks::preparation::decode_preparation_record(saved.as_bytes())
        .map_err(|error| anyhow::anyhow!(error.detail))?;
    Ok(saved)
}
