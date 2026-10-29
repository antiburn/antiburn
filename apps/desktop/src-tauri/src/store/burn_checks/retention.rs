use std::collections::{BTreeMap, BTreeSet};

use antiburn_checks::check_store::SourceProgress;
use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use crate::store::{SessionKey, Store};

impl Store {
    pub(crate) fn delete_burn_check_session_in(
        connection: &rusqlite::Connection,
        key: &SessionKey,
    ) -> Result<()> {
        let session_json = key.to_identity_json();
        Self::remove_burn_check_session_checkpoints_in(connection, &session_json)?;
        connection.execute(
            "DELETE FROM burn_check_observation WHERE session_key_json = ?1",
            params![session_json],
        )?;
        let candidate_ids =
            "SELECT candidate_id FROM burn_check_candidate WHERE session_key_json = ?1";
        connection.execute(
            &format!("DELETE FROM burn_check_acceptance_watermark WHERE (check_id, activation_scope_key, branch_key) IN (SELECT check_id, activation_scope_key, branch_key FROM burn_check_candidate WHERE candidate_id IN ({candidate_ids}))"),
            params![session_json],
        )?;
        connection.execute(
            &format!("DELETE FROM burn_check_cooldown WHERE (check_id, activation_scope_key, generation, branch_key) IN (SELECT check_id, activation_scope_key, generation, branch_key FROM burn_check_candidate WHERE candidate_id IN ({candidate_ids}))"),
            params![session_json],
        )?;
        connection.execute(
            &format!("DELETE FROM burn_check_current_status WHERE assessment_id IN (SELECT assessment_id FROM burn_check_assessment_history WHERE candidate_id IN ({candidate_ids}))"),
            params![session_json],
        )?;
        connection.execute(
            &format!(
                "DELETE FROM burn_check_assessment_history WHERE candidate_id IN ({candidate_ids})"
            ),
            params![session_json],
        )?;
        connection.execute(
            &format!(
                "DELETE FROM burn_check_evaluation_attempt WHERE candidate_id IN ({candidate_ids})"
            ),
            params![session_json],
        )?;
        connection.execute(
            &format!("DELETE FROM burn_check_candidate WHERE candidate_id IN ({candidate_ids})"),
            params![session_json],
        )?;
        Ok(())
    }

    fn remove_burn_check_session_checkpoints_in(
        connection: &rusqlite::Connection,
        session_json: &str,
    ) -> Result<()> {
        let mut statement = connection.prepare(
            "SELECT check_id, activation_scope_key, branch_key FROM burn_check_candidate WHERE session_key_json = ?1
             UNION SELECT check_id, activation_scope_key, json_extract(payload_json, '$.branch') FROM burn_check_observation
               WHERE session_key_json = ?1 AND json_type(payload_json, '$.branch') = 'text'
             UNION SELECT a.check_id, a.activation_scope_key, b.key
               FROM burn_check_activation a, json_each(a.checkpoint_json, '$.branches') b
              WHERE b.key = ?1",
        )?;
        let mut branches = BTreeMap::<(String, String), BTreeSet<String>>::new();
        for row in statement.query_map([session_json], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })? {
            let (check_id, activation_scope_key, branch) = row?;
            branches
                .entry((check_id, activation_scope_key))
                .or_default()
                .insert(branch);
        }
        drop(statement);
        for ((check_id, activation_scope_key), branches) in branches {
            let saved: Option<String> = connection.query_row(
                "SELECT checkpoint_json FROM burn_check_activation WHERE check_id = ?1 AND activation_scope_key = ?2",
                params![check_id, activation_scope_key], |row| row.get(0),
            ).optional()?.flatten();
            if let Some(saved) = saved {
                let mut checkpoint = SourceProgress::decode(&saved)
                    .map_err(|error| anyhow::anyhow!(error.message))?;
                checkpoint
                    .selection
                    .branches
                    .retain(|branch, _| !branches.contains(&branch.0));
                checkpoint
                    .selection
                    .seen_results
                    .retain(|(branch, _), _| !branches.contains(&branch.0));
                connection.execute("UPDATE burn_check_activation SET checkpoint_json = ?3 WHERE check_id = ?1 AND activation_scope_key = ?2",
                    params![check_id, activation_scope_key, checkpoint.encode().map_err(|error| anyhow::anyhow!(error.message))?])?;
            }
        }
        Ok(())
    }

    pub(crate) fn clear_burn_check_session_data_in(
        connection: &rusqlite::Connection,
    ) -> Result<()> {
        connection.execute("DELETE FROM burn_check_current_status", [])?;
        connection.execute("DELETE FROM burn_check_assessment_history", [])?;
        connection.execute("DELETE FROM burn_check_evaluation_attempt", [])?;
        connection.execute("DELETE FROM burn_check_candidate", [])?;
        connection.execute("DELETE FROM burn_check_observation", [])?;
        connection.execute("DELETE FROM burn_check_acceptance_watermark", [])?;
        connection.execute("DELETE FROM burn_check_cooldown", [])?;
        connection.execute(
            "UPDATE burn_check_activation SET enabled = 0,
                 lifecycle_json = CASE WHEN lifecycle_json IS NULL THEN NULL
                     ELSE json_set(lifecycle_json, '$.state', 'removed') END,
                 checkpoint_json = NULL,
                 bootstrap_sequence = (SELECT COALESCE(MAX(publication_sequence), 0)
                                       FROM native_tool_result_publication),
                 applied_sequence = (SELECT COALESCE(MAX(publication_sequence), 0)
                                     FROM native_tool_result_publication)",
            [],
        )?;
        Ok(())
    }

    pub(crate) fn prune_burn_check_candidate_payloads_in(
        connection: &rusqlite::Connection,
        cutoff_epoch: i64,
    ) -> Result<()> {
        let eligible = "SELECT c.candidate_id FROM burn_check_candidate c
            WHERE c.created_at_epoch < ?1
              AND c.state IN ('accepted', 'coalesced', 'not_assessed', 'execution_failed', 'cancelled', 'stale')
              AND NOT EXISTS(SELECT 1 FROM burn_check_assessment_history h
                JOIN burn_check_current_status s USING(assessment_id) WHERE h.candidate_id = c.candidate_id)";
        connection.execute(
            &format!(
                "UPDATE burn_check_evaluation_attempt SET response_json = NULL
            WHERE candidate_id IN ({eligible})"
            ),
            [cutoff_epoch],
        )?;
        connection.execute(
            &format!(
                "UPDATE burn_check_assessment_history SET answer_json = NULL
            WHERE candidate_id IN ({eligible})"
            ),
            [cutoff_epoch],
        )?;
        connection.execute(
            &format!("UPDATE burn_check_candidate SET frozen_input_json = '{{}}', dependencies_json = '{{}}', answer_json = NULL,
                preparation_json = CASE WHEN json_extract(preparation_json, '$.status') = 'ready'
                    THEN CASE WHEN json_type(preparation_json, '$.decisions') = 'array'
                        THEN json_set(preparation_json, '$.decisions', json((
                            SELECT COALESCE(json_group_array(CASE WHEN type = 'object'
                                THEN json_remove(value, '$.original_input', '$.input', '$.request')
                                ELSE json('{{}}') END), '[]')
                            FROM json_each(preparation_json, '$.decisions'))))
                        ELSE json_remove(preparation_json, '$.original_input', '$.input', '$.request') END
                    ELSE preparation_json END
              WHERE candidate_id IN ({eligible})"),
            params![cutoff_epoch],
        )?;
        Ok(())
    }
}
