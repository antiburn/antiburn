use super::*;

pub(super) const RESPONSE_CACHE_KEY: &str = "internal:burnCheckResponseCacheV1";
const MAX_RESPONSE_CACHE_ENTRIES: usize = 128;
const MAX_RESPONSE_CACHE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedAssessmentResponse {
    pub provider: String,
    pub request_digest: String,
    pub returned_model: String,
    pub response_json: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub created_at_epoch: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurnCheckRequestAdmission {
    Admitted,
    Stale,
    Unresolved,
    Deferred,
    Exhausted,
}

impl Store {
    /// Check each unanswered target before packing. A blocked sibling must not
    /// prevent other targets from receiving their own dispatch.
    pub fn burn_check_dispatch_readiness(
        &self,
        identities: &[String],
        now: i64,
    ) -> anyhow::Result<BurnCheckRequestAdmission> {
        dispatch_readiness_in(&self.lock(), identities, now)
    }

    pub fn burn_check_requests_are_unresolved(
        &self,
        identities: &[String],
    ) -> anyhow::Result<bool> {
        Ok(self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM burn_check_request_outcome
            WHERE request_identity IN (SELECT value FROM json_each(?1)))",
            [serde_json::to_string(identities)?],
            |row| row.get(0),
        )?)
    }

    pub fn track_burn_check_request(
        &self,
        identity: &str,
        reservation_id: &str,
        now: i64,
    ) -> anyhow::Result<bool> {
        self.track_burn_check_requests(&[identity.to_owned()], reservation_id, now)
    }

    pub fn track_burn_check_requests(
        &self,
        identities: &[String],
        reservation_id: &str,
        now: i64,
    ) -> anyhow::Result<bool> {
        validate_request_tracking(identities, reservation_id)?;
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let tracked = track_burn_check_requests_in(&transaction, identities, reservation_id, now)?;
        transaction.commit()?;
        Ok(tracked)
    }

    /// Admit a provider request only while its durable assessment policy is current.
    pub fn admit_burn_check_requests(
        &self,
        input: &BurnCheckInput,
        identities: &[String],
        reservation_id: &str,
        now: i64,
    ) -> anyhow::Result<BurnCheckRequestAdmission> {
        validate_request_tracking(identities, reservation_id)?;
        let mut connection = self.lock();
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let assessment_is_current: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM burn_check_assessment
                 WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                   AND check_id = ?4 AND input_revision = ?5 AND status = 'running'
                   AND lease_expires_at_epoch > ?6
            )",
            rusqlite::params![
                input.key.environment_key,
                input.key.agent,
                input.key.session_id,
                input.check_id,
                input.input_revision,
                now,
            ],
            |row| row.get(0),
        )?;
        if internal_value_in(&transaction, ENABLED_AT_KEY)?.is_none()
            || !check_id_enabled_in(&transaction, &input.check_id)?
            || !assessment_is_current
        {
            transaction.commit()?;
            return Ok(BurnCheckRequestAdmission::Stale);
        }
        let readiness = dispatch_readiness_in(&transaction, identities, now)?;
        if readiness != BurnCheckRequestAdmission::Admitted {
            return Ok(readiness);
        }
        let retained: usize = transaction.query_row(
            "SELECT count(*) FROM burn_check_dispatch_attempt",
            [],
            |row| row.get(0),
        )?;
        let new: usize = transaction.query_row(
            "SELECT count(*) FROM json_each(?1) AS ids WHERE NOT EXISTS
                (SELECT 1 FROM burn_check_dispatch_attempt WHERE request_identity = ids.value)",
            [serde_json::to_string(identities)?],
            |row| row.get(0),
        )?;
        if retained.saturating_add(new) > 32768 {
            return Ok(BurnCheckRequestAdmission::Exhausted);
        }
        let tracked = track_burn_check_requests_in(&transaction, identities, reservation_id, now)?;
        if tracked {
            for identity in identities {
                transaction.execute(
                    "INSERT INTO burn_check_dispatch_attempt
                       (request_identity, environment_key, agent, session_id, attempts)
                     VALUES (?1, ?2, ?3, ?4, 1)
                     ON CONFLICT(request_identity) DO UPDATE SET attempts = attempts + 1,
                         next_attempt_at_epoch = NULL",
                    rusqlite::params![
                        identity,
                        input.key.environment_key,
                        input.key.agent,
                        input.key.session_id
                    ],
                )?;
            }
        }
        transaction.commit()?;
        Ok(if tracked {
            BurnCheckRequestAdmission::Admitted
        } else {
            BurnCheckRequestAdmission::Unresolved
        })
    }

    pub(crate) fn burn_check_dispatch_attempts(
        &self,
        identities: &[String],
    ) -> anyhow::Result<usize> {
        Ok(self.lock().query_row(
            "SELECT COALESCE(MAX(attempts), 0) FROM burn_check_dispatch_attempt
             WHERE request_identity IN (SELECT value FROM json_each(?1))",
            [serde_json::to_string(identities)?],
            |row| row.get(0),
        )?)
    }

    /// A rejected dispatch can retry later. Unknown delivery retains its block.
    pub(crate) fn defer_burn_check_dispatch(
        &self,
        input: &BurnCheckInput,
        identities: &[String],
        retry_at: Option<i64>,
    ) -> anyhow::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE burn_check_dispatch_attempt SET next_attempt_at_epoch = ?2,
                 terminal = (?2 IS NULL OR attempts >= 3)
             WHERE request_identity IN (SELECT value FROM json_each(?1))",
            rusqlite::params![serde_json::to_string(identities)?, retry_at],
        )?;
        if let Some(retry_at) = retry_at {
            transaction.execute(
                "UPDATE burn_check_assessment SET next_attempt_at_epoch = ?6
                 WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                   AND check_id = ?4 AND input_revision = ?5",
                rusqlite::params![
                    input.key.environment_key,
                    input.key.agent,
                    input.key.session_id,
                    input.check_id,
                    input.input_revision,
                    retry_at
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
}

fn dispatch_readiness_in(
    connection: &rusqlite::Connection,
    identities: &[String],
    now: i64,
) -> anyhow::Result<BurnCheckRequestAdmission> {
    let blocked: (bool, bool, bool) = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM burn_check_request_outcome
                     WHERE request_identity IN (SELECT value FROM json_each(?1))),
                COALESCE(MAX(terminal = 1 OR attempts >= 3), 0),
                COALESCE(MAX(next_attempt_at_epoch > ?2), 0)
         FROM burn_check_dispatch_attempt
         WHERE request_identity IN (SELECT value FROM json_each(?1))",
        rusqlite::params![serde_json::to_string(identities)?, now],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok(if blocked.0 {
        BurnCheckRequestAdmission::Unresolved
    } else if blocked.1 {
        BurnCheckRequestAdmission::Exhausted
    } else if blocked.2 {
        BurnCheckRequestAdmission::Deferred
    } else {
        BurnCheckRequestAdmission::Admitted
    })
}

fn validate_request_tracking(identities: &[String], reservation_id: &str) -> anyhow::Result<()> {
    if identities.is_empty()
        || identities.len() > 128
        || identities
            .iter()
            .any(|identity| identity.is_empty() || identity.len() > 128)
        || reservation_id.is_empty()
        || reservation_id.len() > 128
    {
        anyhow::bail!("Burn Check request identity is invalid");
    }
    Ok(())
}

fn track_burn_check_requests_in(
    connection: &rusqlite::Connection,
    identities: &[String],
    reservation_id: &str,
    now: i64,
) -> anyhow::Result<bool> {
    let count: usize = connection.query_row(
        "SELECT count(*) FROM burn_check_request_outcome",
        [],
        |row| row.get(0),
    )?;
    let conflict: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM burn_check_request_outcome
            WHERE request_identity IN (SELECT value FROM json_each(?1)))",
        [serde_json::to_string(identities)?],
        |row| row.get(0),
    )?;
    if conflict || count.saturating_add(identities.len()) > 1024 {
        return Ok(false);
    }
    for identity in identities {
        connection.execute("INSERT INTO burn_check_request_outcome (request_identity, reservation_id, created_at_epoch)
                VALUES (?1, ?2, ?3)", rusqlite::params![identity, reservation_id, now])?;
    }
    Ok(true)
}

impl Store {
    pub fn clear_burn_check_request_outcomes(&self, identities: &[String]) -> anyhow::Result<()> {
        self.lock().execute(
            "DELETE FROM burn_check_request_outcome
             WHERE request_identity IN (SELECT value FROM json_each(?1))",
            [serde_json::to_string(identities)?],
        )?;
        Ok(())
    }

    /// Atomically settle usage and cache a successful typed response.
    pub fn record_burn_check_response(
        &self,
        reservation_id: &str,
        entry: CachedAssessmentResponse,
    ) -> anyhow::Result<()> {
        validate_cache_entry(&entry)?;
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut ledger = super::usage::load_ledger(&transaction)?;
        let mut reservation =
            super::usage::load_reservation(&transaction, reservation_id, entry.created_at_epoch)?;
        if reservation.provider != entry.provider || reservation.model != entry.returned_model {
            anyhow::bail!("Burn Check usage identity does not match the response");
        }
        if !reservation.settled {
            super::usage::record_confirmed_usage(
                &mut ledger.summary,
                &reservation,
                entry.input_tokens,
                entry.output_tokens,
                entry.created_at_epoch,
            );
            reservation.settled = true;
        }
        reservation.input_tokens = entry.input_tokens;

        insert_cache_entry(&transaction, &entry)?;
        super::usage::save_reservation(&transaction, &reservation)?;
        write_json_setting(&transaction, USAGE_LEDGER_KEY, &ledger)?;
        transaction.execute(
            "DELETE FROM burn_check_request_outcome WHERE reservation_id = ?1",
            [reservation_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Read an exact-payload response from the bounded shared cache.
    pub fn cached_assessment_response(
        &self,
        provider: &str,
        request_digest: &str,
        now_epoch: i64,
    ) -> anyhow::Result<Option<CachedAssessmentResponse>> {
        let connection = self.lock();
        Ok(connection.query_row(
            "SELECT returned_model, response_json, input_tokens, output_tokens, created_at_epoch
             FROM burn_check_response_cache WHERE provider = ?1 AND request_digest = ?2
               AND created_at_epoch > ?3",
            rusqlite::params![provider, request_digest, now_epoch.saturating_sub(7 * 24 * 60 * 60)],
            |row| Ok(CachedAssessmentResponse {
                provider: provider.to_owned(), request_digest: request_digest.to_owned(),
                returned_model: row.get(0)?, response_json: row.get(1)?,
                input_tokens: row.get(2)?, output_tokens: row.get(3)?, created_at_epoch: row.get(4)?,
            }),
        ).optional()?)
    }
}

fn validate_cache_entry(entry: &CachedAssessmentResponse) -> anyhow::Result<()> {
    if entry.provider.is_empty()
        || entry.provider.len() > 128
        || entry.request_digest.is_empty()
        || entry.request_digest.len() > 128
        || entry.returned_model.is_empty()
        || entry.returned_model.len() > 128
        || entry.response_json.len() > 64 * 1024
        || serde_json::from_str::<Value>(&entry.response_json).is_err()
    {
        anyhow::bail!("Burn Check response cache entry is invalid");
    }
    Ok(())
}

fn insert_cache_entry(
    connection: &rusqlite::Connection,
    entry: &CachedAssessmentResponse,
) -> anyhow::Result<()> {
    connection.execute("INSERT OR REPLACE INTO burn_check_response_cache
        (provider, request_digest, returned_model, response_json, input_tokens, output_tokens, created_at_epoch)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![entry.provider, entry.request_digest, entry.returned_model,
            entry.response_json, entry.input_tokens, entry.output_tokens, entry.created_at_epoch])?;
    connection.execute(
        "DELETE FROM burn_check_response_cache WHERE created_at_epoch <= ?1",
        [entry.created_at_epoch.saturating_sub(7 * 24 * 60 * 60)],
    )?;
    loop {
        let (count, bytes): (usize, usize) = connection.query_row(
            "SELECT count(*), COALESCE(sum(length(CAST(response_json AS BLOB)) +
                length(CAST(provider AS BLOB)) + length(CAST(request_digest AS BLOB)) +
                length(CAST(returned_model AS BLOB)) + 32), 0) FROM burn_check_response_cache",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if count <= MAX_RESPONSE_CACHE_ENTRIES && bytes <= MAX_RESPONSE_CACHE_BYTES {
            break;
        }
        connection.execute(
            "DELETE FROM burn_check_response_cache WHERE rowid =
            (SELECT rowid FROM burn_check_response_cache ORDER BY created_at_epoch, rowid LIMIT 1)",
            [],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_cache_usage_and_dispatch_resolution_commit_atomically() {
        let store = Store::open_in_memory(std::path::Path::new("synthetic-state")).unwrap();
        store.set_internal_value(
            USAGE_LEDGER_KEY,
            &serde_json::json!({
                "reservations": [{"id": "reservation", "session_key": "synthetic-session",
                    "provider": "synthetic-provider", "check_id": "synthetic-check",
                    "model": "jev-1.13.0", "input_tokens": 65536,
                    "expires_at_epoch": 86400, "settled": false, "unknown_recorded": false}],
                "summary": {}
            })
            .to_string(),
        );
        let identities = vec!["first-item".to_owned(), "second-item".to_owned()];
        store.burn_check_usage_summary().unwrap();
        assert!(
            store
                .track_burn_check_requests(&identities, "reservation", 1000)
                .unwrap()
        );
        let original_ledger = store.internal_value(USAGE_LEDGER_KEY).unwrap();
        let original_reservation: String = store
            .lock()
            .query_row(
                "SELECT data FROM burn_check_usage_reservation WHERE id = 'reservation'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let response = entry(1, "saved answer");
        for failure in [
            "CREATE TEMP TRIGGER fail_settlement BEFORE UPDATE ON setting
             WHEN NEW.key = 'internal:burnCheckUsageLedgerV1'
             BEGIN SELECT RAISE(ABORT, 'injected ledger write failure'); END;",
            "CREATE TEMP TRIGGER fail_settlement BEFORE DELETE ON burn_check_request_outcome
             BEGIN SELECT RAISE(ABORT, 'injected resolution failure'); END;",
        ] {
            store.lock().execute_batch(failure).unwrap();
            assert!(
                store
                    .record_burn_check_response("reservation", response.clone())
                    .is_err()
            );
            assert_eq!(
                store.internal_value(USAGE_LEDGER_KEY).as_deref(),
                Some(original_ledger.as_str())
            );
            let reservation: String = store
                .lock()
                .query_row(
                    "SELECT data FROM burn_check_usage_reservation WHERE id = 'reservation'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(reservation, original_reservation);
            assert!(
                store
                    .cached_assessment_response(&response.provider, &response.request_digest, 1002)
                    .unwrap()
                    .is_none()
            );
            for identity in &identities {
                assert!(
                    store
                        .burn_check_requests_are_unresolved(std::slice::from_ref(identity))
                        .unwrap()
                );
            }
            store
                .lock()
                .execute_batch("DROP TRIGGER fail_settlement")
                .unwrap();
        }
        for _ in 0..2 {
            store
                .record_burn_check_response("reservation", response.clone())
                .unwrap();
        }
        assert_eq!(
            store
                .cached_assessment_response(&response.provider, &response.request_digest, 1002)
                .unwrap(),
            Some(response)
        );
        assert!(
            !store
                .burn_check_requests_are_unresolved(&identities)
                .unwrap()
        );
        let usage = store.burn_check_usage_summary().unwrap();
        assert_eq!(usage.confirmed_calls, 1);
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 1);
        assert_eq!(usage.unknown_outcomes, 0);
    }

    #[test]
    fn unresolved_dispatch_blocks_resume_and_cannot_grow_without_bound() {
        let store = Store::open_in_memory(std::path::Path::new("synthetic-state")).unwrap();
        assert!(
            store
                .track_burn_check_request("request-1", "reservation-1", 1)
                .unwrap()
        );
        assert!(
            !store
                .track_burn_check_request("request-1", "reservation-2", 2)
                .unwrap()
        );
        let request = ["request-1".to_owned()];
        assert!(store.burn_check_requests_are_unresolved(&request).unwrap());
        store.clear_burn_check_request_outcomes(&request).unwrap();
        assert!(!store.burn_check_requests_are_unresolved(&request).unwrap());
        for index in 0..1024 {
            assert!(
                store
                    .track_burn_check_request(
                        &format!("request-{index}"),
                        &format!("reservation-{index}"),
                        3
                    )
                    .unwrap()
            );
        }
        assert!(
            !store
                .track_burn_check_request("over-limit", "reservation-over-limit", 3)
                .unwrap()
        );
    }

    #[test]
    fn retry_clears_only_the_request_identities_being_resent() {
        let store = Store::open_in_memory(std::path::Path::new("synthetic-state")).unwrap();
        let identities = vec!["retry-this".to_owned(), "keep-blocked".to_owned()];
        assert!(
            store
                .track_burn_check_requests(&identities, "reservation", 1000)
                .unwrap()
        );

        store
            .clear_burn_check_request_outcomes(&["retry-this".to_owned()])
            .unwrap();

        assert!(
            !store
                .burn_check_requests_are_unresolved(&["retry-this".to_owned()])
                .unwrap()
        );
        assert!(
            store
                .burn_check_requests_are_unresolved(&["keep-blocked".to_owned()])
                .unwrap()
        );
    }

    fn entry(index: usize, text: &str) -> CachedAssessmentResponse {
        CachedAssessmentResponse {
            provider: "synthetic-provider".to_owned(),
            request_digest: format!("digest-{index}"),
            returned_model: "jev-1.13.0".to_owned(),
            response_json: serde_json::json!({"answer": text}).to_string(),
            input_tokens: 10,
            output_tokens: 1,
            created_at_epoch: 1000 + index as i64,
        }
    }

    #[test]
    fn response_cache_migration_preserves_existing_paid_responses() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        for migration in &super::super::super::schema::MIGRATIONS[..63] {
            connection.execute_batch(migration).unwrap();
        }
        let saved = entry(1, "saved answer");
        connection
            .execute(
                "INSERT INTO setting (key, value) VALUES (?1, ?2)",
                rusqlite::params![
                    RESPONSE_CACHE_KEY,
                    serde_json::json!({"entries": [saved]}).to_string()
                ],
            )
            .unwrap();
        connection
            .execute_batch(super::super::super::schema::MIGRATIONS[63])
            .unwrap();
        let restored: String = connection.query_row("SELECT response_json FROM burn_check_response_cache WHERE request_digest = 'digest-1'", [], |row| row.get(0)).unwrap();
        assert_eq!(restored, saved.response_json);
    }
}
