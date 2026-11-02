use std::collections::HashMap;

use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::{BurnCheckCandidateRecord, BurnCheckObservationRecord, payload_sha256};
use crate::store::Store;
use crate::store::native_tool_results::trace_effects::InvalidationEffects;

const INVALIDATE_BRANCH_CURRENT_STATUS_SQL: &str =
    "UPDATE burn_check_current_status SET evidence_valid = 0
       WHERE evidence_valid = 1 AND check_id = ?1 AND activation_scope_key = ?2 AND branch_key = ?3
         AND EXISTS (SELECT 1 FROM burn_check_assessment_history h
           WHERE h.assessment_id = burn_check_current_status.assessment_id AND NOT EXISTS (
             SELECT 1 FROM burn_check_candidate_dependency d WHERE d.candidate_id = h.candidate_id
               AND json_type(d.dependency_binding_json, '$.SessionActivity') = 'object'))";

pub(super) struct RuntimeCheckpoint<'a> {
    pub expected: &'a str,
    pub next: &'a str,
    pub scope: &'a antiburn_checks::domain::ActivationScope,
    pub invalidated_branches: &'a [String],
    pub reset_branches: &'a [String],
    pub coalesce_pending: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct BurnCheckPublicationPage<'a> {
    pub(super) check_id: &'a str,
    pub(super) activation_scope_key: &'a str,
    pub(super) generation: i64,
    pub(super) through_sequence: i64,
    pub(super) publication_sequences: &'a [i64],
    pub(super) observations: &'a [BurnCheckObservationRecord],
    pub(super) candidates: &'a [BurnCheckCandidateRecord],
}

impl Store {
    pub(super) fn apply_burn_check_publications_with_checkpoint(
        &self,
        page: &BurnCheckPublicationPage<'_>,
        checkpoint: Option<RuntimeCheckpoint<'_>>,
    ) -> Result<bool> {
        let BurnCheckPublicationPage {
            check_id,
            activation_scope_key,
            generation,
            through_sequence,
            publication_sequences,
            observations,
            candidates,
        } = *page;
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut effects = InvalidationEffects::default();
        let activation: Option<(i64, i64, bool)> = tx
            .query_row(
                "SELECT generation, applied_sequence, enabled FROM burn_check_activation
                  WHERE check_id = ?1 AND activation_scope_key = ?2",
                params![check_id, activation_scope_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)),
            )
            .optional()?;
        let Some((active_generation, cursor, enabled)) = activation else {
            return Ok(false);
        };
        if active_generation != generation || !enabled {
            return Ok(false);
        }
        if let Some(checkpoint) = &checkpoint {
            let saved: Option<String> = tx.query_row(
                "SELECT checkpoint_json FROM burn_check_activation WHERE check_id = ?1 AND activation_scope_key = ?2",
                params![check_id, activation_scope_key], |row| row.get(0),
            )?;
            ensure!(
                saved.as_deref() == Some(checkpoint.expected),
                "selector checkpoint changed"
            );
        }
        ensure!(
            through_sequence >= cursor,
            "publication cursor cannot move backwards"
        );
        ensure!(
            publication_sequences
                .windows(2)
                .all(|pair| pair[0] < pair[1]),
            "publication cursors must be strictly increasing"
        );
        ensure!(
            publication_sequences
                .first()
                .is_none_or(|sequence| *sequence > cursor)
                && publication_sequences
                    .last()
                    .is_none_or(|sequence| *sequence == through_sequence),
            "publication cursor range does not match the page"
        );
        let persisted_sequences = {
            let target =
                crate::store::native_tool_results::evidence::target_predicate("p", 5, 6, 7);
            let mut statement = tx.prepare(
                &format!("SELECT p.publication_sequence FROM native_tool_result_publication p
                  WHERE p.publication_sequence > ?1 AND p.publication_sequence <= ?2
                    AND {target}
                    AND (?3 IS NULL OR (p.environment_key = ?3 AND EXISTS (
                        SELECT 1 FROM session s WHERE s.environment_key = p.environment_key
                          AND s.agent = p.agent AND s.session_id = p.session_id AND s.cwd = ?4
                          AND (p.session_incarnation IS NULL OR p.project_cwd = ?4)
                          AND (p.session_incarnation IS NULL OR p.session_incarnation = s.incarnation))))
                  ORDER BY p.publication_sequence"),
            )?;
            statement
                .query_map(
                    params![
                        cursor,
                        through_sequence,
                        checkpoint.as_ref().map(|value| &value.scope.environment),
                        checkpoint.as_ref().map(|value| &value.scope.session_cwd),
                        check_id,
                        activation_scope_key,
                        generation
                    ],
                    |row| row.get(0),
                )?
                .collect::<rusqlite::Result<Vec<i64>>>()?
        };
        let replay = checkpoint.as_ref().is_some_and(|value| {
            matches!(
                value.scope.run,
                antiburn_checks::domain::RunScope::Replay(_)
            )
        });
        if !replay && persisted_sequences != publication_sequences {
            return Ok(false);
        }
        if !replay && publication_sequences.is_empty() && through_sequence > cursor {
            let high_water: i64 = tx.query_row(
                "SELECT COALESCE(MAX(publication_sequence), 0) FROM native_tool_result_publication",
                [],
                |row| row.get(0),
            )?;
            if through_sequence > high_water {
                return Ok(false);
            }
        }
        let mut page_sessions = HashMap::with_capacity(publication_sequences.len());
        for sequence in publication_sequences {
            if replay {
                continue;
            }
            let event: Option<(String, String, String)> = tx
                .query_row(
                    "SELECT environment_key, agent, session_id
                       FROM native_tool_result_publication
                      WHERE publication_sequence = ?1",
                    params![sequence],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some((environment_key, agent, session_id)) = event else {
                return Ok(false);
            };
            let session_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM session
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3)",
                params![environment_key, agent, session_id],
                |row| row.get(0),
            )?;
            if !session_exists {
                return Ok(false);
            }
            page_sessions.insert(
                *sequence,
                crate::store::SessionKey::new(environment_key, agent, session_id),
            );
        }
        for observation in observations {
            ensure!(
                observation.publication_sequence > cursor
                    && observation.publication_sequence <= through_sequence,
                "observation is outside the requested cursor range"
            );
            if !replay {
                let Some(event_session) = page_sessions.get(&observation.publication_sequence)
                else {
                    return Ok(false);
                };
                if event_session != &observation.session {
                    return Ok(false);
                }
                let event: (Option<String>, Option<String>, Option<i64>, String) = tx.query_row(
                    "SELECT source_format, native_identity, native_revision, outcome
                   FROM native_tool_result_publication WHERE publication_sequence = ?1",
                    params![observation.publication_sequence],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;
                if event.0.as_deref() != Some(observation.source_format.as_str())
                    || (event.1.is_some()
                        && event.1.as_deref() != Some(observation.native_identity.as_str()))
                    || (event.2.is_some() && event.2 != Some(observation.native_revision))
                    || event.3
                        != if observation.removed {
                            "removal"
                        } else {
                            "update"
                        }
                {
                    return Ok(false);
                }
            }
            let session_json = observation.session.to_identity_json();
            let payload = serde_json::to_string(&observation.payload)?;
            let previous: Option<(String, String, String, i64, String, i64, String)> = tx
                .query_row(
                    "SELECT session_key_json, source_format, COALESCE(native_identity, ''),
                            COALESCE(native_revision, -1), evidence_version, removed, payload_json
                       FROM burn_check_observation
                      WHERE check_id = ?1 AND activation_scope_key = ?2 AND observation_id = ?3",
                    params![check_id, activation_scope_key, observation.observation_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                        ))
                    },
                )
                .optional()?;
            let changed = previous.as_ref().is_some_and(
                |(
                    old_session,
                    old_format,
                    old_identity,
                    old_revision,
                    old_version,
                    old_removed,
                    old_payload,
                )| {
                    old_session != &session_json
                        || old_format != &observation.source_format
                        || old_identity != &observation.native_identity
                        || *old_revision != observation.native_revision
                        || old_version != &observation.evidence_version
                        || *old_removed != i64::from(observation.removed)
                        || old_payload != &payload
                },
            );
            if changed {
                effects.current_status_revoked += tx.execute(
                    "UPDATE burn_check_current_status SET evidence_valid = 0
                       WHERE evidence_valid = 1 AND check_id = ?1 AND activation_scope_key = ?2 AND assessment_id IN (
                         SELECT a.assessment_id FROM burn_check_assessment_history a
                         JOIN burn_check_candidate_dependency d
                           ON d.candidate_id = a.candidate_id
                         WHERE d.observation_id = ?3 AND d.session_key_json = ?4)",
                    params![
                        check_id,
                        activation_scope_key,
                        observation.observation_id,
                        previous.as_ref().map(|entry| entry.0.as_str())
                    ],
                )?;
                effects.accepted_evidence_revoked += tx.execute(
                    "UPDATE burn_check_assessment_history SET evidence_valid = 0
                       WHERE evidence_valid = 1 AND check_id = ?1 AND activation_scope_key = ?2 AND candidate_id IN (
                         SELECT candidate_id FROM burn_check_candidate_dependency
                          WHERE observation_id = ?3 AND session_key_json = ?4)",
                    params![
                        check_id,
                        activation_scope_key,
                        observation.observation_id,
                        previous.as_ref().map(|entry| entry.0.as_str())
                    ],
                )?;
                effects.unfinished_stale += tx.execute(
                    "UPDATE burn_check_candidate SET state = 'stale'
                      WHERE check_id = ?1 AND activation_scope_key = ?2
                        AND state IN ('pending', 'claimed', 'dispatching', 'answered')
                        AND candidate_id IN (
                          SELECT candidate_id FROM burn_check_candidate_dependency
                           WHERE observation_id = ?3 AND session_key_json = ?4)",
                    params![
                        check_id,
                        activation_scope_key,
                        observation.observation_id,
                        previous.as_ref().map(|entry| entry.0.as_str())
                    ],
                )?;
            }
            tx.execute(
                "INSERT INTO burn_check_observation (
                     check_id, activation_scope_key, observation_id, session_key_json,
                     publication_sequence, source_format, native_identity,
                     native_revision, evidence_version, removed, payload_json, event_kind, observer_binding_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6,
                    CASE WHEN (SELECT target_check_id FROM native_tool_result_publication WHERE publication_sequence = ?5) IS NULL THEN ?7 END,
                    CASE WHEN (SELECT target_check_id FROM native_tool_result_publication WHERE publication_sequence = ?5) IS NULL THEN ?8 END,
                    ?9, ?10, ?11, COALESCE((SELECT event_kind FROM native_tool_result_publication WHERE publication_sequence = ?5), 'tool_result'),
                    (SELECT json_extract(payload_json, '$.observer') FROM native_tool_result_publication WHERE publication_sequence = ?5 AND target_check_id IS NOT NULL))
                 ON CONFLICT(check_id, activation_scope_key, observation_id) DO UPDATE SET
                    session_key_json = excluded.session_key_json,
                    publication_sequence = excluded.publication_sequence,
                    source_format = excluded.source_format,
                    native_identity = excluded.native_identity,
                    native_revision = excluded.native_revision,
                    evidence_version = excluded.evidence_version,
                    removed = excluded.removed,
                    payload_json = excluded.payload_json, event_kind = excluded.event_kind,
                    observer_binding_json = excluded.observer_binding_json",
                params![
                    check_id,
                    activation_scope_key,
                    observation.observation_id,
                    session_json,
                    observation.publication_sequence,
                    observation.source_format,
                    observation.native_identity,
                    observation.native_revision,
                    observation.evidence_version,
                    i64::from(observation.removed),
                    payload
                ],
            )?;
        }
        if let Some(checkpoint) = &checkpoint {
            for branch in checkpoint.invalidated_branches {
                effects.current_status_revoked += tx.execute(
                    INVALIDATE_BRANCH_CURRENT_STATUS_SQL,
                    params![check_id, activation_scope_key, branch],
                )?;
                effects.accepted_evidence_revoked += tx.execute("UPDATE burn_check_assessment_history SET evidence_valid = 0 WHERE evidence_valid = 1 AND check_id = ?1 AND activation_scope_key = ?2 AND branch_key = ?3
                    AND NOT EXISTS (SELECT 1 FROM burn_check_candidate_dependency d WHERE d.candidate_id = burn_check_assessment_history.candidate_id
                      AND json_type(d.dependency_binding_json, '$.SessionActivity') = 'object')", params![check_id, activation_scope_key, branch])?;
            }
            for branch in checkpoint
                .invalidated_branches
                .iter()
                .chain(checkpoint.reset_branches)
            {
                effects.unfinished_stale += tx.execute("UPDATE burn_check_candidate SET state = 'stale' WHERE check_id = ?1 AND activation_scope_key = ?2 AND branch_key = ?3 AND state IN ('pending', 'claimed', 'dispatching', 'answered')", params![check_id, activation_scope_key, branch])?;
            }
        }
        for candidate in candidates {
            ensure!(
                candidate.check_id == check_id
                    && candidate.activation_scope_key == activation_scope_key
                    && candidate.generation == generation,
                "candidate does not belong to the active generation"
            );
            ensure!(
                candidate.selected_publication_head <= through_sequence,
                "candidate selects evidence beyond the applied cursor"
            );
            ensure!(
                candidate.dependencies.is_object(),
                "candidate dependencies must be an object"
            );
            let session_json = candidate.session.to_identity_json();
            if !replay
                && !page_sessions
                    .values()
                    .any(|session| session == &candidate.session)
            {
                return Ok(false);
            }
            let inserted = tx.execute(
                "INSERT INTO burn_check_candidate (
                    candidate_id, check_id, activation_scope_key, generation, branch_key, session_key_json,
                    trigger_position, selected_publication_head, dependencies_json,
                    frozen_input_json, input_fingerprint, state, created_at_epoch, updated_at_epoch)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'pending', ?12, ?12)
                 ON CONFLICT(candidate_id) DO NOTHING",
                params![
                    candidate.candidate_id,
                    check_id,
                    activation_scope_key,
                    generation,
                    candidate.branch_key,
                    session_json,
                    candidate.trigger_position,
                    candidate.selected_publication_head,
                    serde_json::to_string(&candidate.dependencies)?,
                    serde_json::to_string(&candidate.frozen_input)?,
                    candidate.input_fingerprint,
                    candidate.created_at_epoch
                ],
            )?;
            if inserted == 1 {
                let dependencies = candidate
                    .dependencies
                    .as_object()
                    .expect("candidate dependencies were validated");
                let mut dependencies_current = true;
                for (observation_id, dependency) in dependencies {
                    if let Some(binding) = dependency.get("binding") {
                        let binding: crate::store::native_tool_results::observers::DependencyBinding = serde_json::from_value(binding.clone())?;
                        let (format, revision, kind, hash) = match &binding {
                            crate::store::native_tool_results::observers::DependencyBinding::NativeCurrent(native) =>
                                (native.source_format.clone(), Some(native.revision), native.event_kind.name(), native.payload_sha256.clone()),
                            crate::store::native_tool_results::observers::DependencyBinding::SessionActivity { .. } =>
                                ("attachment-observer".into(), None, "session_idle", String::new()),
                            crate::store::native_tool_results::observers::DependencyBinding::HostObservation { .. } =>
                                anyhow::bail!("host dependency requires a committed observation"),
                        };
                        tx.execute("INSERT INTO burn_check_candidate_dependency(candidate_id, observation_id, session_key_json,
                            source_format, native_revision, evidence_version, removed, payload_sha256, event_kind, dependency_binding_json)
                            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, ?9)",
                            params![candidate.candidate_id, observation_id, session_json, format, revision,
                                dependency.get("version").and_then(serde_json::Value::as_str).unwrap_or(""), hash, kind, serde_json::to_string(&binding)?])?;
                        continue;
                    }
                    let observation: Option<(i64, String, String, i64, String, i64, String)> = tx
                        .query_row(
                            "SELECT publication_sequence, session_key_json, source_format,
                                     COALESCE(native_revision, -1), evidence_version, removed, payload_json
                               FROM burn_check_observation
                              WHERE check_id = ?1 AND activation_scope_key = ?2 AND observation_id = ?3",
                            params![check_id, activation_scope_key, observation_id],
                            |row| {
                                Ok((
                                    row.get(0)?,
                                    row.get(1)?,
                                    row.get(2)?,
                                    row.get(3)?,
                                    row.get(4)?,
                                    row.get(5)?,
                                    row.get(6)?,
                                ))
                            },
                        )
                        .optional()?;
                    let Some((
                        publication_sequence,
                        observation_session,
                        source_format,
                        native_revision,
                        evidence_version,
                        removed,
                        payload_json,
                    )) = observation
                    else {
                        anyhow::bail!("candidate dependency has no committed observation");
                    };
                    if observation_session != session_json
                        || dependencies
                            .get(observation_id)
                            .and_then(serde_json::Value::as_str)
                            != Some(evidence_version.as_str())
                        || removed != 0
                        || publication_sequence > candidate.selected_publication_head
                    {
                        dependencies_current = false;
                    }
                    tx.execute(
                        "INSERT INTO burn_check_candidate_dependency (
                             candidate_id, observation_id, session_key_json, source_format,
                              native_revision, evidence_version, removed, payload_sha256, event_kind, dependency_binding_json)
                          VALUES (?1, ?2, ?3, ?4, NULLIF(?5, -1), ?6, ?7, ?8,
                             COALESCE((SELECT event_kind FROM burn_check_observation WHERE check_id = ?9 AND activation_scope_key = ?10 AND observation_id = ?2), 'tool_result'),
                             (SELECT json_object('HostObservation', json_object('observer', json(observer_binding_json)))
                              FROM burn_check_observation WHERE check_id = ?9 AND activation_scope_key = ?10
                                AND observation_id = ?2 AND observer_binding_json IS NOT NULL))",
                        params![
                            candidate.candidate_id,
                            observation_id,
                            observation_session,
                            source_format,
                            native_revision,
                            evidence_version,
                            removed,
                            payload_sha256(&payload_json), check_id, activation_scope_key
                        ],
                    )?;
                }
                dependencies_current &=
                    crate::store::native_tool_results::observers::typed_dependencies_current_in(
                        &tx,
                        &candidate.candidate_id,
                    )?;
                if !dependencies_current {
                    effects.unfinished_stale += tx.execute(
                        "UPDATE burn_check_candidate SET state = 'stale'
                          WHERE candidate_id = ?1 AND state = 'pending'",
                        params![candidate.candidate_id],
                    )?;
                } else if checkpoint
                    .as_ref()
                    .is_some_and(|value| value.coalesce_pending)
                {
                    tx.execute(
                        "UPDATE burn_check_candidate SET state = 'coalesced', updated_at_epoch = ?6
                          WHERE check_id = ?1 AND activation_scope_key = ?2 AND generation = ?3
                             AND branch_key = ?4 AND state = 'pending' AND candidate_id <> ?5
                              AND trigger_position < ?7
                              AND NOT EXISTS(SELECT 1 FROM burn_check_evaluation_attempt a
                                WHERE a.candidate_id = burn_check_candidate.candidate_id)",
                        params![
                            check_id,
                            activation_scope_key,
                            generation,
                            candidate.branch_key,
                            candidate.candidate_id,
                            candidate.created_at_epoch,
                            candidate.trigger_position
                        ],
                    )?;
                }
                let pending: usize = tx.query_row(
                    "SELECT COUNT(*) FROM burn_check_candidate WHERE activation_scope_key = ?1 AND state = 'pending'",
                    [activation_scope_key], |row| row.get(0),
                )?;
                if pending > antiburn_checks::engine::MAX_PENDING_CANDIDATES {
                    tx.execute("INSERT INTO burn_check_assessment_history
                        SELECT ?2, candidate_id, check_id, activation_scope_key, generation, branch_key, trigger_position,
                            'not_assessed', NULL, ?3, 0, ?4 FROM burn_check_candidate WHERE candidate_id = ?1 AND state = 'pending'",
                        params![candidate.candidate_id, format!("overflow:{}", candidate.candidate_id),
                            serde_json::to_string(&serde_json::json!({"detail": "pending candidate limit reached"}))?, candidate.created_at_epoch])?;
                    tx.execute("UPDATE burn_check_candidate SET state = 'not_assessed', frozen_input_json = '{}', dependencies_json = '{}'
                        WHERE candidate_id = ?1 AND state = 'pending'", [&candidate.candidate_id])?;
                }
            }
        }
        tx.execute(
            "UPDATE burn_check_activation SET applied_sequence = ?3
              WHERE check_id = ?1 AND activation_scope_key = ?2 AND generation = ?4
                AND applied_sequence = ?5",
            params![
                check_id,
                activation_scope_key,
                through_sequence,
                generation,
                cursor
            ],
        )?;
        if let Some(checkpoint) = &checkpoint {
            tx.execute(
                "UPDATE burn_check_activation SET checkpoint_json = ?3
                  WHERE check_id = ?1 AND activation_scope_key = ?2",
                params![check_id, activation_scope_key, checkpoint.next],
            )?;
        }
        let trace_generation = u64::try_from(generation)?;
        let activation_ref = checkpoint.as_ref().map(|checkpoint| {
            super::trace_refs::activation_ref(&antiburn_checks::domain::ActivationId {
                definition_id: check_id.into(),
                scope: checkpoint.scope.clone(),
                generation: antiburn_checks::domain::Generation(trace_generation),
            })
        });
        tx.commit()?;
        if effects != InvalidationEffects::default() {
            ::tracing::debug!(
                event = "burn_check_evidence_transition_committed",
                publication_origin = "runtime_source_page",
                reason = "selected_evidence_changed",
                activation_ref,
                generation,
                publication_after_cursor = cursor,
                publication_through_cursor = through_sequence,
                accepted_evidence_revoked = effects.accepted_evidence_revoked,
                current_status_revoked = effects.current_status_revoked,
                unfinished_stale = effects.unfinished_stale,
            );
        }
        Ok(true)
    }
}

#[cfg(test)]
#[path = "observations_tests.rs"]
mod tests;
