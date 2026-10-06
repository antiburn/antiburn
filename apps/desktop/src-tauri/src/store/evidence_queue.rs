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
use rusqlite::{OptionalExtension, Transaction, params};

use super::{EvidenceClaim, EvidenceFailure, ProjectionRevisions, SessionKey, Store};

/// The candidate order of one claim.
#[derive(Clone, Copy)]
enum ClaimOrder {
    /// The next due row, oldest attempt first.
    Arrival,
    /// The most recently active session first.
    Recency,
}

impl Store {
    /// Enroll missing evidence rows and requeue stale transcript projections.
    pub fn reconcile_evidence_revisions(
        &self,
        agents: &[&str],
        revisions: ProjectionRevisions,
    ) -> Result<usize> {
        if agents.is_empty() {
            return Ok(0);
        }

        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let agent_placeholders = vec!["?"; agents.len()].join(", ");
        let agent_values: Vec<rusqlite::types::Value> = agents
            .iter()
            .map(|agent| rusqlite::types::Value::Text((*agent).to_string()))
            .collect();
        let enroll_sql = format!(
            "INSERT INTO session_evidence (environment_key, agent, session_id)
                 SELECT session.environment_key, session.agent, session.session_id
                   FROM session
                  WHERE session.agent IN ({agent_placeholders})
                    AND NOT EXISTS (
                        SELECT 1 FROM session_evidence
                         WHERE session_evidence.environment_key = session.environment_key
                           AND session_evidence.agent = session.agent
                            AND session_evidence.session_id = session.session_id
                     )
                 RETURNING environment_key, agent"
        );
        let mut enroll_statement = transaction.prepare(&enroll_sql)?;
        let enrolled_scopes = enroll_statement
            .query_map(rusqlite::params_from_iter(agent_values.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let enrolled = enrolled_scopes.len();
        drop(enroll_statement);

        let parser_parameter = agents.len() + 1;
        let analyzer_parameter = agents.len() + 2;
        let metrics_parameter = agents.len() + 3;
        let evidence_parameter = agents.len() + 4;
        let update_sql = format!(
            "UPDATE session_evidence AS evidence
                SET status = 'pending', last_error = NULL,
                    next_attempt_at_epoch = NULL, retry_count = 0
              WHERE evidence.agent IN ({agent_placeholders})
                AND (
                    evidence.status <> 'pending'
                    OR evidence.last_error IS NOT NULL
                    OR evidence.next_attempt_at_epoch IS NOT NULL
                    OR evidence.retry_count <> 0
                )
                AND EXISTS (
                    SELECT 1 FROM session
                     WHERE session.environment_key = evidence.environment_key
                       AND session.agent = evidence.agent
                       AND session.session_id = evidence.session_id
                       AND (
                            evidence.analyzed_generation IS NOT session.source_generation
                            OR (evidence.status NOT IN ('failed', 'unsupported')
                                AND evidence.processed_fingerprint IS NOT session.source_fingerprint)
                            OR evidence.parser_revision IS NOT ?{parser_parameter}
                           OR evidence.analyzer_revision IS NOT ?{analyzer_parameter}
                           OR evidence.evidence_schema_revision IS NOT ?{evidence_parameter}
                           OR (evidence.status NOT IN ('failed', 'unsupported')
                               AND NOT EXISTS (
                               SELECT 1 FROM session_analysis AS analysis
                                WHERE analysis.environment_key = session.environment_key
                                  AND analysis.agent = session.agent
                                  AND analysis.session_id = session.session_id
                                  AND analysis.analyzed_generation = session.source_generation
                                  AND analysis.parser_revision = ?{parser_parameter}
                                  AND analysis.analyzer_revision = ?{analyzer_parameter}
                                  AND analysis.metrics_schema_revision = ?{metrics_parameter}
                           ))
                       )
                 )
             RETURNING environment_key, agent"
        );
        let mut update_values = agent_values;
        update_values.extend([
            rusqlite::types::Value::Integer(revisions.parser_revision),
            rusqlite::types::Value::Integer(revisions.analyzer_revision),
            rusqlite::types::Value::Integer(revisions.metrics_schema_revision),
            rusqlite::types::Value::Integer(revisions.evidence_schema_revision),
        ]);
        let mut update_statement = transaction.prepare(&update_sql)?;
        let requeued_scopes = update_statement
            .query_map(rusqlite::params_from_iter(update_values.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let requeued = requeued_scopes.len();
        drop(update_statement);
        transaction.commit()?;
        Ok(enrolled + requeued)
    }

    /// Claim the next eligible evidence row for an enabled agent.
    pub fn claim_next_evidence(
        &self,
        agents: &[&str],
        now_epoch: i64,
        lease_secs: i64,
    ) -> Result<Option<EvidenceClaim>> {
        self.claim_next_evidence_in_order(agents, now_epoch, lease_secs, ClaimOrder::Arrival)
    }

    /// Claim the next pending or expired-lease evidence row for `agents`,
    /// preferring the session most recently active.
    ///
    /// Same claim rules as [`Self::claim_next_evidence`]. Only the candidate
    /// order differs: this orders by the claimed session's `updated_at_epoch`
    /// descending (NULLs last, so an unknown activity never jumps ahead of a
    /// known one), then falls back to the same tiebreakers. Analysis then
    /// runs newest-first, so a current session never waits behind a history
    /// one in the same backlog.
    pub fn claim_next_evidence_by_recency(
        &self,
        agents: &[&str],
        now_epoch: i64,
        lease_secs: i64,
    ) -> Result<Option<EvidenceClaim>> {
        self.claim_next_evidence_in_order(agents, now_epoch, lease_secs, ClaimOrder::Recency)
    }

    /// Claim a pending or expired-lease row with a due `next_attempt_at_epoch`.
    /// The claim takes a new fence from [`next_claim_fence_in`].
    fn claim_next_evidence_in_order(
        &self,
        agents: &[&str],
        now_epoch: i64,
        lease_secs: i64,
        order: ClaimOrder,
    ) -> Result<Option<EvidenceClaim>> {
        if agents.is_empty() {
            return Ok(None);
        }

        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let agent_placeholders = vec!["?"; agents.len()].join(", ");
        let mut values: Vec<rusqlite::types::Value> = agents
            .iter()
            .map(|agent| rusqlite::types::Value::Text((*agent).to_string()))
            .collect();
        values.push(rusqlite::types::Value::Integer(now_epoch));
        let now_parameter = values.len();
        let order_prefix = match order {
            ClaimOrder::Arrival => "",
            ClaimOrder::Recency => {
                "session.updated_at_epoch IS NULL, session.updated_at_epoch DESC,"
            }
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
                        AND (
                            evidence.status = 'pending'
                            OR (evidence.status = 'processing'
                                AND evidence.lease_expires_at_epoch <= ?{now_parameter})
                        )
                        AND (evidence.next_attempt_at_epoch IS NULL
                             OR evidence.next_attempt_at_epoch <= ?{now_parameter})
                      ORDER BY {order_prefix}
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

    /// Record a retry or terminal failure for a current claim.
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
        transaction.commit()?;
        Ok(updated > 0)
    }
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
