//! Startup seeds one current window only when the source proves result order.

use antiburn_checks::domain::{BranchId, EvidenceDependency};
use antiburn_checks::selection::SelectionOutcome;
use antiburn_checks::source::SourceEvent;

use super::*;
use crate::store::SessionKey;

const V2_RECEIPT_SQL: &str = "json_array(v.incarnation, v.source_label, v.cwd,
    v.capture_epoch, v.claim_fence, v.baseline_sequence)";
const V2_CAPTURE_MATCH_SQL: &str = "v.environment_key = s.environment_key
    AND v.agent = s.agent AND v.session_id = s.session_id
    AND v.incarnation = s.incarnation AND v.source_label = s.source_label AND v.cwd = s.cwd
    AND v.capture_epoch <> '' AND v.claim_fence > 0";

pub(crate) struct StartupSession {
    pub key: SessionKey,
    pub incarnation: i64,
    pub receipt: String,
    v2: bool,
}

impl StoreCheckStore {
    pub(crate) fn startup_sessions(
        &self,
        activation: &EffectiveActivation,
        active_since: i64,
    ) -> Result<Vec<StartupSession>> {
        let ready = crate::insights_report::installed_checks::source_ready_sql(4, 5, 6);
        let v2 = crate::store::native_tool_results::opencode_v2_status::SOURCE_SQL;
        let connection = self.store.lock();
        let mut statement = connection.prepare(&format!(
            "WITH eligible AS (
               SELECT s.agent, s.session_id, s.incarnation,
                  CASE WHEN ({v2}) THEN {V2_RECEIPT_SQL}
                    WHEN s.agent = 'opencode'
                    AND s.source_label = 'opencode:' || s.session_id
                    THEN j.cursor_json ELSE f.cursor_json END AS receipt,
                 s.updated_at_epoch, ({v2}) AS v2
                FROM session s LEFT JOIN session_evidence e USING(environment_key, agent, session_id)
                LEFT JOIN opencode_journal_delivery j USING(environment_key, agent, session_id)
                LEFT JOIN native_file_delivery f USING(environment_key, agent, session_id)
                LEFT JOIN opencode_v2_capture v USING(environment_key, agent, session_id)
               WHERE s.environment_key = ?1 AND s.cwd = ?2 AND s.wsl_distro IS NULL
                  AND s.agent IN ('claude-code', 'codex', 'pi', 'opencode')
                  AND CASE WHEN ({v2}) THEN ({V2_CAPTURE_MATCH_SQL})
                    AND json_extract(?9, '$.run') = 'Live' ELSE ({ready}) END
                  AND CASE WHEN ({v2}) THEN 1 WHEN s.agent = 'opencode'
                    AND s.source_label = 'opencode:' || s.session_id
                    THEN j.pending = 0 ELSE f.pending = 0 END
                  AND (({v2}) OR s.updated_at_epoch >= ?3 OR EXISTS(SELECT 1 FROM native_tool_result_publication p
                     WHERE p.environment_key = s.environment_key AND p.agent = s.agent
                       AND p.session_id = s.session_id AND p.publication_sequence > ?7
                       AND (p.session_incarnation IS NULL OR
                         (p.session_incarnation = s.incarnation AND p.project_cwd IS s.cwd))))
                  AND (({v2}) OR NOT EXISTS(SELECT 1 FROM native_tool_result_publication p
                   JOIN burn_check_activation a ON a.check_id = ?8 AND a.activation_scope_key = ?9 AND a.generation = ?10
                   WHERE p.environment_key = s.environment_key AND p.agent = s.agent AND p.session_id = s.session_id
                     AND p.publication_sequence > a.applied_sequence
                     AND (p.session_incarnation IS NULL OR
                        (p.session_incarnation = s.incarnation AND p.project_cwd IS s.cwd))))
                 AND NOT EXISTS(SELECT 1 FROM burn_check_session_attachment a
                     WHERE a.check_id = ?8 AND a.activation_scope_key = ?9 AND a.generation = ?10
                       AND a.branch_key = json_array(s.environment_key, s.agent, s.session_id)
                       AND a.incarnation = s.incarnation)
             ), recent AS (
               SELECT * FROM eligible ORDER BY updated_at_epoch DESC, agent, session_id LIMIT 127
             ), oldest AS (
               SELECT * FROM eligible WHERE NOT EXISTS (
                 SELECT 1 FROM recent WHERE recent.agent = eligible.agent
                   AND recent.session_id = eligible.session_id)
               ORDER BY updated_at_epoch ASC, agent, session_id LIMIT 1
             )
              SELECT agent, session_id, incarnation, receipt, v2 FROM (
               SELECT * FROM recent UNION ALL SELECT * FROM oldest
             ) ORDER BY updated_at_epoch DESC, agent, session_id LIMIT 128"
        ))?;
        Ok(statement
            .query_map(
                params![
                    self.scope.environment,
                    self.scope.session_cwd,
                    active_since,
                    antiburn_local::analysis::PARSER_REVISION,
                    antiburn_local::analysis::ANALYZER_REVISION,
                    antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                    activation.bootstrap_publication.0,
                    activation.id.definition_id,
                    self.activation_scope_key,
                    activation.id.generation.0
                ],
                |row| {
                    Ok(StartupSession {
                        key: SessionKey::new(
                            &self.scope.environment,
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                        ),
                        incarnation: row.get(2)?,
                        receipt: row.get(3)?,
                        v2: row.get(4)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub(crate) fn initialize_recent(
        &self,
        activation: &EffectiveActivation,
        session: &StartupSession,
        events: &[SourceEvent],
    ) -> Result<Option<antiburn_checks::engine::BatchOutcome>> {
        // Stored V2 message order does not prove native tool completion order.
        let events = if session.v2 { &[][..] } else { events };
        ensure!(events.len() <= 40, "startup window exceeds its bound");
        let saved = self.progress(activation)?;
        let mut next = saved.clone();
        let branch = BranchId(session.key.to_identity_json());
        let observed = next
            .selection
            .seen_results
            .keys()
            .any(|(key, _)| key == &branch);
        let mut startup = activation.clone();
        // This explicit startup boundary admits only the bounded current snapshot.
        startup.bootstrap_publication = PublicationCursor(0);
        startup.bootstrap_observation_position = ObservationPosition(0);
        let mut observations = Vec::new();
        let mut condition_outcomes = Vec::new();
        let mut candidate = None;
        let mut selected_head = saved.publication_cursor.0;
        if !observed {
            next.selection
                .branches
                .entry(branch.clone())
                .or_default()
                .coverage_complete = false;
            for event in events {
                let SourceEvent::Observation {
                    cursor,
                    identity,
                    revision,
                    observation: draft,
                } = event
                else {
                    continue;
                };
                ensure!(
                    draft.branch == branch,
                    "startup result belongs to another session"
                );
                selected_head = selected_head.max(cursor.0);
                next.observation_position.0 = next
                    .observation_position
                    .0
                    .checked_add(1)
                    .context("observation order exhausted")?;
                let observation = (**draft).clone().commit(next.observation_position);
                match next
                    .selection
                    .apply_published(&startup, identity, *revision, &observation)
                {
                    SelectionOutcome::Candidate(ready) => candidate = Some(*ready),
                    SelectionOutcome::ConditionFiltered(condition) => {
                        condition_outcomes.push((observation.position, condition))
                    }
                    _ => {}
                }
                observations.push((*cursor, identity, *revision, draft, observation));
            }
        }
        let expected = saved
            .encode()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        let checkpoint = next
            .encode()
            .map_err(|error| anyhow::anyhow!(error.message))?;
        let mut connection = self.store.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let v2 = crate::store::native_tool_results::opencode_v2_status::SOURCE_SQL;
        let current: bool = tx.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM burn_check_activation a JOIN session s
                ON s.environment_key = ?4 AND s.agent = ?5 AND s.session_id = ?6
              WHERE a.check_id = ?1 AND a.activation_scope_key = ?2 AND a.generation = ?3 AND a.enabled = 1
                AND a.checkpoint_json = ?7 AND s.incarnation = ?8 AND s.cwd = ?9
                AND NOT EXISTS(SELECT 1 FROM burn_check_session_attachment t
                  WHERE t.check_id = a.check_id AND t.activation_scope_key = a.activation_scope_key
                    AND t.generation = a.generation AND t.branch_key = ?10 AND t.incarnation = s.incarnation)
                AND ((?12 = 1 AND ({v2})
                  AND json_extract(a.activation_scope_key, '$.run') = 'Live'
                  AND EXISTS(SELECT 1 FROM opencode_v2_capture v
                    WHERE {V2_CAPTURE_MATCH_SQL} AND {V2_RECEIPT_SQL} = ?11))
                  OR (?12 = 0 AND s.agent = 'opencode' AND EXISTS(SELECT 1 FROM opencode_journal_delivery d
                  WHERE d.environment_key = s.environment_key AND d.agent = s.agent AND d.session_id = s.session_id
                    AND d.incarnation = s.incarnation AND d.source_label = s.source_label AND d.cwd IS s.cwd
                    AND d.blocked = 0 AND d.pending = 0 AND d.cursor_json = ?11))
                  OR (?12 = 0 AND s.agent <> 'opencode' AND EXISTS(SELECT 1 FROM native_file_delivery d
                    WHERE d.environment_key = s.environment_key AND d.agent = s.agent AND d.session_id = s.session_id
                      AND d.incarnation = s.incarnation AND d.source_label = s.source_label AND d.cwd IS s.cwd
                      AND d.blocked = 0 AND d.pending = 0 AND d.cursor_json = ?11))))"),
            params![activation.id.definition_id, self.activation_scope_key, activation.id.generation.0,
                session.key.environment_key, session.key.agent, session.key.session_id, expected,
                session.incarnation, self.scope.session_cwd, branch.0, session.receipt, session.v2], |row| row.get(0),
        )?;
        if !current {
            return Ok(None);
        }
        let observation_count = observations.len();
        let candidate_count = usize::from(candidate.is_some());
        for (cursor, identity, revision, draft, observation) in observations {
            let valid: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM native_tool_result_current c
                  WHERE c.environment_key = ?1 AND c.agent = ?2 AND c.session_id = ?3
                    AND c.publication_sequence = ?4 AND c.is_removed = 0)",
                params![
                    session.key.environment_key,
                    session.key.agent,
                    session.key.session_id,
                    cursor.0
                ],
                |row| row.get(0),
            )?;
            if !valid {
                return Ok(None);
            }
            tx.execute(
                "INSERT INTO burn_check_observation
                    (check_id, activation_scope_key, observation_id, session_key_json,
                     publication_sequence, source_format, native_identity, native_revision, evidence_version, removed, payload_json)
                 SELECT ?1, ?2, ?3, ?4, publication_sequence, source_format, native_identity,
                    ?5, ?6, 0, ?7 FROM native_tool_result_publication WHERE publication_sequence = ?8",
                params![activation.id.definition_id, self.activation_scope_key,
                    serde_json::to_string(&(&branch.0, identity))?, branch.0, revision,
                    observation.version.0, serde_json::to_string(draft)?, cursor.0],
            )?;
        }
        if let Some(candidate) = candidate {
            let id = hex(&candidate.fingerprint);
            let dependencies = candidate
                .dependencies
                .iter()
                .filter_map(|dependency| match dependency {
                    EvidenceDependency::Message { id, version } => Some((
                        serde_json::to_string(&(&branch.0, id)).expect("identity encodes"),
                        serde_json::Value::String(version.0.clone()),
                    )),
                    _ => None,
                })
                .collect::<serde_json::Map<_, _>>();
            tx.execute(
                "INSERT INTO burn_check_candidate
                    (candidate_id, check_id, activation_scope_key, generation, branch_key, session_key_json,
                     trigger_position, selected_publication_head, dependencies_json, frozen_input_json,
                     input_fingerprint, state, created_at_epoch, updated_at_epoch)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9, ?1, 'pending', ?10, ?10)",
                params![id, activation.id.definition_id, self.activation_scope_key, activation.id.generation.0,
                    branch.0, candidate.trigger_position.0, selected_head, serde_json::to_string(&dependencies)?,
                    serde_json::to_string(&candidate)?, now_epoch()],
            )?;
            for key in dependencies.keys() {
                let (format, revision, version, payload): (String, i64, String, String) = tx.query_row(
                    "SELECT source_format, native_revision, evidence_version, payload_json FROM burn_check_observation
                     WHERE check_id = ?1 AND activation_scope_key = ?2 AND observation_id = ?3",
                    params![activation.id.definition_id, self.activation_scope_key, key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;
                tx.execute("INSERT INTO burn_check_candidate_dependency
                    (candidate_id, observation_id, session_key_json, source_format, native_revision, evidence_version, removed, payload_sha256)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
                    params![id, key, branch.0, format, revision, version, super::super::payload_sha256(&payload)])?;
            }
        }
        tx.execute("UPDATE burn_check_candidate SET state = 'coalesced' WHERE check_id = ?1
            AND activation_scope_key = ?2 AND generation = ?3 AND branch_key = ?4 AND state = 'pending'
            AND trigger_position < (SELECT MAX(trigger_position) FROM burn_check_candidate
              WHERE check_id = ?1 AND activation_scope_key = ?2 AND generation = ?3 AND branch_key = ?4 AND state = 'pending')",
            params![activation.id.definition_id, self.activation_scope_key, activation.id.generation.0, branch.0])?;
        tx.execute(
            "UPDATE burn_check_activation SET checkpoint_json = ?3
            WHERE check_id = ?1 AND activation_scope_key = ?2",
            params![
                activation.id.definition_id,
                self.activation_scope_key,
                checkpoint
            ],
        )?;
        tx.execute(
            "DELETE FROM burn_check_session_attachment WHERE check_id = ?1
            AND activation_scope_key = ?2 AND generation <> ?3",
            params![
                activation.id.definition_id,
                self.activation_scope_key,
                activation.id.generation.0
            ],
        )?;
        tx.execute("INSERT OR REPLACE INTO burn_check_session_attachment
            (check_id, activation_scope_key, generation, branch_key, incarnation) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![activation.id.definition_id, self.activation_scope_key, activation.id.generation.0, branch.0, session.incarnation])?;
        tx.commit()?;
        Ok(Some(antiburn_checks::engine::BatchOutcome {
            observations: observation_count,
            candidates: candidate_count,
            coalesced: 0,
            overflowed: 0,
            condition_outcomes,
        }))
    }

    pub(crate) fn initialized_branches(
        &self,
        activation: &EffectiveActivation,
    ) -> Result<std::collections::BTreeSet<BranchId>> {
        let ready = crate::insights_report::installed_checks::source_ready_sql(4, 5, 6);
        let v2 = crate::store::native_tool_results::opencode_v2_status::SOURCE_SQL;
        let connection = self.store.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT t.branch_key FROM burn_check_session_attachment t
             JOIN session s ON s.environment_key = json_extract(t.branch_key, '$[0]')
               AND s.agent = json_extract(t.branch_key, '$[1]') AND s.session_id = json_extract(t.branch_key, '$[2]')
              LEFT JOIN session_evidence e USING(environment_key, agent, session_id)
              LEFT JOIN opencode_v2_capture v USING(environment_key, agent, session_id)
              WHERE t.check_id = ?1 AND t.activation_scope_key = ?2 AND t.generation = ?3
                AND s.environment_key = ?7 AND s.cwd = ?8 AND s.wsl_distro IS NULL
                AND t.incarnation = s.incarnation
                AND CASE WHEN ({v2}) THEN ({V2_CAPTURE_MATCH_SQL})
                  AND json_extract(?2, '$.run') = 'Live' ELSE ({ready}) END
               AND NOT EXISTS(SELECT 1 FROM native_tool_result_publication p
                 JOIN burn_check_activation a ON a.check_id = t.check_id AND a.activation_scope_key = t.activation_scope_key
                 WHERE p.environment_key = s.environment_key AND p.agent = s.agent AND p.session_id = s.session_id
                   AND p.publication_sequence > a.applied_sequence
                   AND (p.session_incarnation IS NULL OR
                     (p.session_incarnation = s.incarnation AND p.project_cwd IS s.cwd)))"
        ))?;
        Ok(statement
            .query_map(
                params![
                    activation.id.definition_id,
                    self.activation_scope_key,
                    activation.id.generation.0,
                    antiburn_local::analysis::PARSER_REVISION,
                    antiburn_local::analysis::ANALYZER_REVISION,
                    antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                    self.scope.environment,
                    self.scope.session_cwd,
                ],
                |row| Ok(BranchId(row.get(0)?)),
            )?
            .collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
#[path = "runtime_startup_v2_tests.rs"]
mod v2_tests;
