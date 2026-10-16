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
        connection.execute(
            "UPDATE burn_check_candidate SET frozen_input_json = '{}', dependencies_json = '{}',
                preparation_json = CASE WHEN json_extract(preparation_json, '$.status') = 'ready'
                    THEN json_set(preparation_json, '$.input', json('{}'), '$.request', NULL)
                    ELSE preparation_json END
              WHERE created_at_epoch < ?1
                AND state IN ('accepted', 'coalesced', 'not_assessed',
                              'execution_failed', 'cancelled', 'stale')
                AND candidate_id NOT IN (
                    SELECT candidate_id FROM burn_check_assessment_history
                     WHERE assessment_id IN (
                         SELECT assessment_id FROM burn_check_current_status))",
            params![cutoff_epoch],
        )?;
        Ok(())
    }
}
