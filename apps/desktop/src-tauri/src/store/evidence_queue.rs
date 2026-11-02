//! The evidence queue: enrollment, claims, leases, and failures.
//!
//! A claim fence names one pass over one session. Turn rows, coverage
//! records, lease renewal, and publication all match on it. Each claim takes
//! its fence from `evidence_claim_fence_seq`, which only increases and which
//! [`Store::clear_local_session_data`] and session deletes never reset. A
//! session row that is deleted and then discovered again starts with a new
//! evidence row, so a per-row counter would give its first claim the same
//! fence as a claim that started before the delete. That pass could then
//! write rows into the new claim's fence and delete them when it loses the
//! publish race. A fence from the shared counter never repeats.

use anyhow::Result;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::{
    EvidenceClaim, EvidenceFailure, SessionKey, Store, delete_turn_rows_for_fence,
    read_published_fence, turn_session_key,
};

impl Store {
    /// Claim the next eligible evidence row for an enabled agent.
    pub fn claim_next_evidence(
        &self,
        agents: &[&str],
        now_epoch: i64,
        lease_secs: i64,
    ) -> Result<Option<EvidenceClaim>> {
        self.claim_next_evidence_in_order(agents, now_epoch, lease_secs)
    }

    /// Claim the next pending or expired-lease evidence row using recent-first
    /// turns with durable fair service.
    ///
    /// This alias keeps the explicit recency call used by the worker. Both
    /// methods use the same durable 3:1 selection policy.
    pub fn claim_next_evidence_by_recency(
        &self,
        agents: &[&str],
        now_epoch: i64,
        lease_secs: i64,
    ) -> Result<Option<EvidenceClaim>> {
        self.claim_next_evidence_in_order(agents, now_epoch, lease_secs)
    }

    /// Claim a pending or expired-lease row with a due `next_attempt_at_epoch`.
    /// The claim takes a new fence from [`next_claim_fence_in`].
    fn claim_next_evidence_in_order(
        &self,
        agents: &[&str],
        now_epoch: i64,
        lease_secs: i64,
    ) -> Result<Option<EvidenceClaim>> {
        if agents.is_empty() {
            return Ok(None);
        }

        let mut connection = self.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let agent_placeholders = vec!["?"; agents.len()].join(", ");
        let mut values: Vec<rusqlite::types::Value> = agents
            .iter()
            .map(|agent| rusqlite::types::Value::Text((*agent).to_string()))
            .collect();
        values.push(rusqlite::types::Value::Integer(now_epoch));
        let now_parameter = values.len();
        let unassigned = transaction.query_row(
            &format!(
                "SELECT COUNT(*) FROM session_evidence AS evidence
                   JOIN session ON session.environment_key = evidence.environment_key
                    AND session.agent = evidence.agent AND session.session_id = evidence.session_id
                  WHERE evidence.agent IN ({agent_placeholders})
                    AND evidence.service_ticket IS NULL
                    AND evidence.status IN ('pending', 'processing')"
            ),
            rusqlite::params_from_iter(agents.iter().copied()),
            |row| row.get::<_, i64>(0),
        )?;
        if unassigned > 0 {
            transaction.execute(
                &format!(
                    "WITH unassigned AS (
                         SELECT evidence.rowid, ROW_NUMBER() OVER (
                             ORDER BY session.first_seen_at, session.environment_key,
                                      session.agent, session.session_id) AS row_number
                           FROM session_evidence AS evidence
                           JOIN session ON session.environment_key = evidence.environment_key
                            AND session.agent = evidence.agent AND session.session_id = evidence.session_id
                          WHERE evidence.agent IN ({agent_placeholders})
                            AND evidence.service_ticket IS NULL
                            AND evidence.status IN ('pending', 'processing')
                     )
                     UPDATE session_evidence
                        SET service_ticket = (SELECT schedule.next_ticket + unassigned.row_number - 1
                                                FROM unassigned, work_queue_schedule AS schedule
                                               WHERE unassigned.rowid = session_evidence.rowid
                                                 AND schedule.domain = 'evidence')
                      WHERE rowid IN (SELECT rowid FROM unassigned)"
                ),
                rusqlite::params_from_iter(agents.iter().copied()),
            )?;
            transaction.execute(
                "UPDATE work_queue_schedule SET next_ticket = next_ticket + ?1
                  WHERE domain = 'evidence'",
                [unassigned],
            )?;
        }
        let fair_turn = transaction.query_row(
            "SELECT successful_turns % 4 = 3 FROM work_queue_schedule
              WHERE domain = 'evidence'",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        let order_prefix = if fair_turn {
            "evidence.service_ticket ASC,"
        } else {
            "session.updated_at_epoch IS NULL, session.updated_at_epoch DESC,"
        };
        let candidate = transaction
            .query_row(
                &format!(
                    "SELECT evidence.environment_key, evidence.agent, evidence.session_id
                       FROM session_evidence AS evidence
                       JOIN session
                         ON session.environment_key = evidence.environment_key
                        AND session.agent = evidence.agent
                        AND session.session_id = evidence.session_id
                       WHERE evidence.agent IN ({agent_placeholders})
                         AND session.strict_fingerprint_pending = 0
                         AND (
                            evidence.status = 'pending'
                            OR (evidence.status = 'processing'
                                AND evidence.lease_expires_at_epoch <= ?{now_parameter})
                        )
                        AND (evidence.next_attempt_at_epoch IS NULL
                             OR evidence.next_attempt_at_epoch <= ?{now_parameter})
                       ORDER BY {order_prefix}
                                evidence.service_ticket,
                                evidence.next_attempt_at_epoch,
                               evidence.claimed_at_epoch,
                               evidence.environment_key, evidence.agent, evidence.session_id
                      LIMIT 1"
                ),
                rusqlite::params_from_iter(values.iter()),
                |row| {
                    Ok(SessionKey::new(
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some(key) = candidate else {
            transaction.commit()?;
            return Ok(None);
        };

        let next_ticket: i64 = transaction.query_row(
            "SELECT next_ticket FROM work_queue_schedule WHERE domain = 'evidence'",
            [],
            |row| row.get(0),
        )?;
        transaction.execute(
            "UPDATE session_evidence SET service_ticket = ?4
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id, next_ticket],
        )?;
        transaction.execute(
            "UPDATE work_queue_schedule
                SET next_ticket = next_ticket + 1, successful_turns = successful_turns + 1
              WHERE domain = 'evidence'",
            [],
        )?;
        let claim_fence = next_claim_fence_in(&transaction)?;
        transaction.execute(
            "UPDATE session_evidence
                SET status = 'processing', claim_fence = ?6,
                    claimed_at_epoch = ?4, lease_expires_at_epoch = ?5
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                key.environment_key,
                key.agent,
                key.session_id,
                now_epoch,
                now_epoch + lease_secs,
                claim_fence,
            ],
        )?;
        let (source_generation, retry_count) = transaction.query_row(
            "SELECT session.source_generation, evidence.retry_count
               FROM session_evidence AS evidence
               JOIN session
                 ON session.environment_key = evidence.environment_key
                AND session.agent = evidence.agent
                AND session.session_id = evidence.session_id
              WHERE evidence.environment_key = ?1
                AND evidence.agent = ?2 AND evidence.session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        transaction.commit()?;
        Ok(Some(EvidenceClaim {
            key,
            source_generation,
            claim_fence,
            retry_count,
        }))
    }

    /// Extend a claim when its fence and source generation remain current.
    pub fn renew_evidence_lease(
        &self,
        claim: &EvidenceClaim,
        now_epoch: i64,
        lease_secs: i64,
    ) -> Result<bool> {
        let connection = self.lock();
        let updated = connection.execute(
            "UPDATE session_evidence AS evidence
                SET claimed_at_epoch = ?6, lease_expires_at_epoch = ?7
              WHERE evidence.environment_key = ?1
                AND evidence.agent = ?2 AND evidence.session_id = ?3
                AND evidence.status = 'processing' AND evidence.claim_fence = ?4
                AND EXISTS (
                    SELECT 1 FROM session
                     WHERE session.environment_key = evidence.environment_key
                       AND session.agent = evidence.agent
                        AND session.session_id = evidence.session_id
                        AND session.source_generation = ?5
                )",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                claim.claim_fence,
                claim.source_generation,
                now_epoch,
                now_epoch + lease_secs,
            ],
        )?;
        Ok(updated > 0)
    }

    /// Record a retry or terminal failure and discard this pass's unpublished rows.
    /// A superseded claim can discard its own rows without changing the current claim.
    /// Call after the pass stops.
    pub fn fail_evidence(
        &self,
        claim: &EvidenceClaim,
        failure: EvidenceFailure,
        last_error: &str,
    ) -> Result<bool> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let updated = match failure {
            EvidenceFailure::Retry {
                next_attempt_at_epoch,
                counts_as_attempt,
            } => transaction.execute(
                "UPDATE session_evidence AS evidence
                    SET status = 'pending', retry_count = retry_count + ?8,
                        last_error = ?6, claimed_at_epoch = NULL,
                        lease_expires_at_epoch = NULL, next_attempt_at_epoch = ?7
                  WHERE evidence.environment_key = ?1
                    AND evidence.agent = ?2 AND evidence.session_id = ?3
                    AND evidence.status = 'processing' AND evidence.claim_fence = ?4
                    AND EXISTS (
                        SELECT 1 FROM session
                         WHERE session.environment_key = evidence.environment_key
                           AND session.agent = evidence.agent
                           AND session.session_id = evidence.session_id
                           AND session.source_generation = ?5
                    )",
                params![
                    claim.key.environment_key,
                    claim.key.agent,
                    claim.key.session_id,
                    claim.claim_fence,
                    claim.source_generation,
                    last_error,
                    next_attempt_at_epoch,
                    i64::from(counts_as_attempt),
                ],
            )?,
            EvidenceFailure::Failed { revisions } => transaction.execute(
                "UPDATE session_evidence AS evidence
                    SET status = 'failed', retry_count = retry_count + 1,
                        analyzed_generation = ?5, parser_revision = ?7,
                        analyzer_revision = ?8, evidence_schema_revision = ?9,
                        evidence_json = NULL,
                        last_error = ?6, claimed_at_epoch = NULL,
                        lease_expires_at_epoch = NULL, next_attempt_at_epoch = NULL
                  WHERE evidence.environment_key = ?1
                    AND evidence.agent = ?2 AND evidence.session_id = ?3
                    AND evidence.status = 'processing' AND evidence.claim_fence = ?4
                    AND EXISTS (
                        SELECT 1 FROM session
                         WHERE session.environment_key = evidence.environment_key
                           AND session.agent = evidence.agent
                           AND session.session_id = evidence.session_id
                           AND session.source_generation = ?5
                    )",
                params![
                    claim.key.environment_key,
                    claim.key.agent,
                    claim.key.session_id,
                    claim.claim_fence,
                    claim.source_generation,
                    last_error,
                    revisions.parser_revision,
                    revisions.analyzer_revision,
                    revisions.evidence_schema_revision,
                ],
            )?,
        };
        discard_evidence_claim_rows_in(&transaction, &claim.key, claim.claim_fence)?;
        transaction.commit()?;
        Ok(updated > 0)
    }
}

/// The caller holds a write transaction across the fence check and deletion.
pub(super) fn discard_evidence_claim_rows_in(
    transaction: &Transaction<'_>,
    key: &SessionKey,
    claim_fence: i64,
) -> Result<usize> {
    if read_published_fence(transaction, key)? == Some(claim_fence) {
        return Ok(0);
    }
    Ok(delete_turn_rows_for_fence(
        transaction,
        &turn_session_key(key),
        claim_fence,
    )?)
}

/// Take the next claim fence. It is higher than every fence any claim has
/// had before, for every session.
fn next_claim_fence_in(transaction: &Transaction<'_>) -> Result<i64> {
    Ok(transaction.query_row(
        "UPDATE evidence_claim_fence_seq SET value = value + 1 WHERE id = 1 RETURNING value",
        [],
        |row| row.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SessionRecord;

    #[test]
    fn evidence_claims_use_three_recent_turns_then_the_oldest_ticket() {
        let state = tempfile::tempdir().unwrap();
        let store = Store::open(state.path()).unwrap();
        let records = (0..5)
            .map(|index| SessionRecord {
                key: SessionKey::new("native", "claude-code", format!("session-{index}")),
                source_kind: "file".into(),
                source_label: format!("/synthetic/session-{index}.jsonl"),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: Some("/synthetic".into()),
                surface: "cli".into(),
                updated_at_epoch: Some(1_000 - index),
                activity_cursor: String::new(),
                activity_source: "mtime".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: None,
            })
            .collect::<Vec<_>>();
        store.upsert_sessions(&records, &["claude-code"]).unwrap();

        let claimed = (0..3)
            .map(|_| {
                store
                    .claim_next_evidence(&["claude-code"], 100, 60)
                    .unwrap()
                    .unwrap()
                    .key
                    .session_id
            })
            .collect::<Vec<_>>();

        assert_eq!(claimed, ["session-0", "session-1", "session-2"]);
        drop(store);
        let store = Store::open(state.path()).unwrap();
        assert_eq!(
            store
                .claim_next_evidence(&["claude-code"], 100, 60)
                .unwrap()
                .unwrap()
                .key
                .session_id,
            "session-3"
        );
        let (turns, next_ticket): (i64, i64) = store
            .lock()
            .query_row(
                "SELECT successful_turns, next_ticket FROM work_queue_schedule
                  WHERE domain = 'evidence'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(turns, 4);
        assert_eq!(next_ticket, 10);
    }
}
