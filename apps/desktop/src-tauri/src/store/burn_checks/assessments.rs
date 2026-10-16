use anyhow::Context;
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::{BurnCheckCandidateClaim, payload_sha256};
use crate::store::Store;
use antiburn_checks::check_store::AcceptanceOutcome;

type AnsweredCandidateRow = (
    String,
    String,
    i64,
    String,
    i64,
    i64,
    i64,
    String,
    Option<String>,
);

impl Store {
    /// Defers an answer while ingestion is ahead. The saved answer remains recoverable.
    pub(crate) fn try_accept_burn_check_answer(
        &self,
        claim: &BurnCheckCandidateClaim,
        assessment_id: &str,
        answer: &serde_json::Value,
        actions: &serde_json::Value,
        status_write: Option<&str>,
        now_epoch: i64,
    ) -> Result<AcceptanceOutcome> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let accepted: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM burn_check_candidate WHERE candidate_id = ?1 AND state = 'accepted')", [&claim.candidate_id], |row| row.get(0))?;
        if accepted {
            return Ok(AcceptanceOutcome::AlreadyAccepted);
        }
        let candidate: Option<AnsweredCandidateRow> = tx
            .query_row(
                "SELECT c.check_id, c.activation_scope_key, c.generation, c.branch_key,
                        c.trigger_position, c.selected_publication_head, a.applied_sequence,
                        c.session_key_json, c.answer_json
                   FROM burn_check_candidate c JOIN burn_check_activation a
                     USING(check_id, activation_scope_key)
                   WHERE c.candidate_id = ?1 AND c.state = 'answered'
                     AND c.claim_owner = ?2 AND c.claim_fence = ?3
                     AND c.generation = a.generation AND a.enabled = 1",
                params![claim.candidate_id, claim.owner, claim.fence],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            check_id,
            activation_scope_key,
            generation,
            branch_key,
            position,
            selected_head,
            applied_sequence,
            session_json,
            persisted_answer,
        )) = candidate
        else {
            tx.execute("UPDATE burn_check_candidate SET state = 'stale' WHERE candidate_id = ?1 AND state = 'answered' AND claim_owner = ?2 AND claim_fence = ?3", params![claim.candidate_id, claim.owner, claim.fence])?;
            tx.commit()?;
            return Ok(AcceptanceOutcome::Stale);
        };
        let persisted_answer: serde_json::Value = serde_json::from_str(
            persisted_answer
                .as_deref()
                .context("answered candidate has no saved response")?,
        )?;
        if persisted_answer != *answer {
            anyhow::bail!("answer does not match the durable response");
        }
        let (environment_key, agent, session_id): (String, String, String) =
            serde_json::from_str(&session_json)?;
        let newer_publication: bool = tx.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM native_tool_result_publication
                 WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                   AND publication_sequence > ?4)",
            params![environment_key, agent, session_id, applied_sequence],
            |row| row.get(0),
        )?;
        if applied_sequence < selected_head || newer_publication {
            return Ok(AcceptanceOutcome::Deferred);
        }
        let mut statement = tx.prepare(
            "SELECT d.source_format, d.native_revision, d.evidence_version, d.removed,
                    d.payload_sha256, o.source_format, o.native_revision, o.evidence_version,
                    o.removed, o.payload_json
               FROM burn_check_candidate_dependency d
               LEFT JOIN burn_check_observation o
                 ON o.check_id = ?2 AND o.activation_scope_key = ?3
                AND o.observation_id = d.observation_id
                AND o.session_key_json = d.session_key_json
              WHERE d.candidate_id = ?1",
        )?;
        let dependencies = statement
            .query_map(
                params![claim.candidate_id, check_id, activation_scope_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<i64>>(8)?,
                        row.get::<_, Option<String>>(9)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let mut dependencies_current = true;
        for (
            source_format,
            native_revision,
            evidence_version,
            removed,
            expected_hash,
            current_format,
            current_revision,
            current_version,
            current_removed,
            current_payload,
        ) in dependencies
        {
            if current_format.as_deref() != Some(source_format.as_str())
                || current_revision != Some(native_revision)
                || current_version.as_deref() != Some(evidence_version.as_str())
                || current_removed != Some(removed)
                || removed != 0
                || current_payload
                    .as_deref()
                    .is_none_or(|payload| payload_sha256(payload) != expected_hash)
            {
                dependencies_current = false;
                break;
            }
        }
        if !dependencies_current {
            tx.execute(
                "UPDATE burn_check_candidate SET state = 'stale', updated_at_epoch = ?2
                  WHERE candidate_id = ?1 AND state = 'answered'",
                params![claim.candidate_id, now_epoch],
            )?;
            tx.commit()?;
            return Ok(AcceptanceOutcome::Stale);
        }
        let watermark: Option<i64> = tx
            .query_row(
                "SELECT position FROM burn_check_acceptance_watermark
                  WHERE check_id = ?1 AND activation_scope_key = ?2 AND branch_key = ?3 AND generation = ?4",
                params![check_id, activation_scope_key, branch_key, generation],
                |row| row.get(0),
            )
            .optional()?;
        if watermark.is_some_and(|old| position <= old) {
            tx.execute("UPDATE burn_check_candidate SET state = 'stale' WHERE candidate_id = ?1 AND state = 'answered'", [&claim.candidate_id])?;
            tx.commit()?;
            return Ok(AcceptanceOutcome::Stale);
        }
        tx.execute(
            "INSERT INTO burn_check_assessment_history VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'assessed', ?8, ?9, 1, ?10)",
            params![
                assessment_id,
                claim.candidate_id,
                check_id,
                activation_scope_key,
                generation,
                branch_key,
                position,
                serde_json::to_string(answer)?,
                serde_json::to_string(actions)?,
                now_epoch
            ],
        )?;
        tx.execute(
            "INSERT INTO burn_check_acceptance_watermark VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(check_id, activation_scope_key, branch_key) DO UPDATE SET
               generation = excluded.generation, position = excluded.position",
            params![
                check_id,
                activation_scope_key,
                branch_key,
                generation,
                position
            ],
        )?;
        if let Some(status) = status_write {
            ensure!(
                status == "success" || status == "failure",
                "invalid Burn Check status"
            );
            tx.execute(
                "INSERT INTO burn_check_current_status VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8)
                 ON CONFLICT(check_id, activation_scope_key, branch_key) DO UPDATE SET
                    generation = excluded.generation, status = excluded.status,
                    assessment_id = excluded.assessment_id, position = excluded.position,
                    evidence_valid = 1, updated_at_epoch = excluded.updated_at_epoch",
                params![
                    check_id,
                    activation_scope_key,
                    branch_key,
                    generation,
                    status,
                    assessment_id,
                    position,
                    now_epoch
                ],
            )?;
        }
        tx.execute(
            "UPDATE burn_check_candidate SET state = 'accepted', updated_at_epoch = ?2
              WHERE candidate_id = ?1 AND state = 'answered'",
            params![claim.candidate_id, now_epoch],
        )?;
        tx.commit()?;
        Ok(AcceptanceOutcome::Accepted)
    }
}
