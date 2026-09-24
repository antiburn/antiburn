use super::*;

impl Store {
    /// Reserve the conservative request maximum before provider dispatch.
    pub fn reserve_burn_check_usage(
        &self,
        input: &BurnCheckInput,
        reserved_input_tokens: u64,
        now_epoch: i64,
        idle_secs: i64,
    ) -> anyhow::Result<BurnCheckReservation> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let row: Option<(i64, String, Option<i64>, bool)> = transaction
            .query_row(
                "SELECT request_count, status, lease_expires_at_epoch,
                        EXISTS (
                            SELECT 1 FROM session AS s
                            JOIN session_evidence AS e
                              ON e.environment_key = s.environment_key AND e.agent = s.agent
                             AND e.session_id = s.session_id
                             AND e.status = 'ready'
                             AND e.analyzed_generation = s.source_generation
                             AND e.processed_fingerprint IS s.source_fingerprint
                             AND e.published_fence = ?6
                             AND e.parser_revision = ?13
                             AND e.evidence_schema_revision = ?14
                            WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
                              AND s.incarnation = ?8 AND s.source_generation = ?9
                              AND s.source_fingerprint IS ?10 AND s.activity_cursor = ?11
                              AND s.updated_at_epoch <= ?7 - ?12
                        )
                   FROM burn_check_assessment
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                    AND check_id = ?4 AND input_revision = ?5",
                rusqlite::params![
                    input.key.environment_key,
                    input.key.agent,
                    input.key.session_id,
                    input.check_id,
                    input.input_revision,
                    input.published_fence,
                    now_epoch,
                    input.incarnation,
                    input.source_generation,
                    input.source_fingerprint,
                    input.activity_cursor,
                    idle_secs.max(0),
                    antiburn_local::analysis::PARSER_REVISION,
                    antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((request_count, status, lease_expires, input_is_current)) = row else {
            transaction.commit()?;
            return Ok(BurnCheckReservation::Stale);
        };
        if status != "running"
            || lease_expires.is_none_or(|expiry| expiry <= now_epoch)
            || !input_is_current
        {
            transaction.commit()?;
            return Ok(BurnCheckReservation::Stale);
        }
        if usize::try_from(request_count).unwrap_or(usize::MAX)
            >= antiburn_local::analysis::jev::MAX_REQUESTS_PER_ASSESSMENT
        {
            transaction.commit()?;
            return Ok(BurnCheckReservation::RequestLimitReached);
        }

        let raw = internal_value_in(&transaction, USAGE_LEDGER_KEY)?;
        let mut ledger = raw
            .as_deref()
            .map(serde_json::from_str::<UsageLedger>)
            .transpose()?
            .unwrap_or_default();
        ledger
            .reservations
            .retain(|reservation| reservation.expires_at_epoch > now_epoch);
        if ledger.reservations.len() >= MAX_USAGE_LEDGER_ENTRIES {
            write_json_setting(&transaction, USAGE_LEDGER_KEY, &ledger)?;
            transaction.commit()?;
            return Ok(BurnCheckReservation::UsageLimitReached);
        }
        let global = ledger
            .reservations
            .iter()
            .fold(0_u64, |total, reservation| {
                total.saturating_add(reservation.input_tokens)
            });
        let session_key = digest_parts([
            input.key.environment_key.as_str(),
            input.key.agent.as_str(),
            input.key.session_id.as_str(),
        ]);
        let session = ledger
            .reservations
            .iter()
            .filter(|reservation| reservation.session_key == session_key)
            .fold(0_u64, |total, reservation| {
                total.saturating_add(reservation.input_tokens)
            });
        if global.saturating_add(reserved_input_tokens) > GLOBAL_INPUT_TOKEN_LIMIT
            || session.saturating_add(reserved_input_tokens) > SESSION_INPUT_TOKEN_LIMIT
        {
            write_json_setting(&transaction, USAGE_LEDGER_KEY, &ledger)?;
            transaction.commit()?;
            return Ok(BurnCheckReservation::UsageLimitReached);
        }
        let attempt = request_count.saturating_add(1).to_string();
        let reservation_id = digest_parts([
            input.key.environment_key.as_str(),
            input.key.agent.as_str(),
            input.key.session_id.as_str(),
            input.check_id.as_str(),
            input.input_revision.as_str(),
            attempt.as_str(),
        ]);
        ledger.reservations.push(UsageReservation {
            id: reservation_id.clone(),
            session_key,
            input_tokens: reserved_input_tokens,
            expires_at_epoch: now_epoch.saturating_add(USAGE_WINDOW_SECS),
        });
        write_json_setting(&transaction, USAGE_LEDGER_KEY, &ledger)?;
        transaction.execute(
            "UPDATE burn_check_assessment SET request_count = request_count + 1,
                    updated_at_epoch = ?6
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND check_id = ?4 AND input_revision = ?5 AND status = 'running'",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id,
                input.input_revision,
                now_epoch,
            ],
        )?;
        transaction.commit()?;
        Ok(BurnCheckReservation::Reserved(reservation_id))
    }

    /// Replace a reservation with actual usage; `None` keeps an unknown outcome reserved.
    pub fn settle_burn_check_usage(
        &self,
        reservation_id: &str,
        actual_input_tokens: Option<u64>,
        now_epoch: i64,
    ) -> anyhow::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let raw = internal_value_in(&transaction, USAGE_LEDGER_KEY)?;
        let mut ledger = raw
            .as_deref()
            .map(serde_json::from_str::<UsageLedger>)
            .transpose()?
            .unwrap_or_default();
        ledger
            .reservations
            .retain(|reservation| reservation.expires_at_epoch > now_epoch);
        if let Some(actual) = actual_input_tokens {
            let Some(reservation) = ledger
                .reservations
                .iter_mut()
                .find(|reservation| reservation.id == reservation_id)
            else {
                anyhow::bail!("Burn Check usage reservation is missing");
            };
            reservation.input_tokens = actual;
        }
        write_json_setting(&transaction, USAGE_LEDGER_KEY, &ledger)?;
        transaction.commit()?;
        Ok(())
    }
}
