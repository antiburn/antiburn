//! Unsafe native admission stays separate from missing historical context.

use antiburn_local::analysis::ToolResultCompleteness;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::store::SessionKey;

pub(crate) const UNSAFE_REASONS_SQL: &str =
    "'native-unsafe-identity', 'native-unsafe-order', 'native-unsafe-ownership'";

const SUPERSEDED_DEPENDENCY_SQL: &str = "SELECT EXISTS(
    SELECT 1 FROM burn_check_candidate_dependency d
    JOIN burn_check_candidate c USING(candidate_id)
    JOIN burn_check_observation o ON o.check_id = c.check_id
      AND o.activation_scope_key = c.activation_scope_key
      AND o.observation_id = d.observation_id AND o.session_key_json = d.session_key_json
    JOIN native_tool_result_publication p ON p.publication_sequence = o.publication_sequence
    JOIN native_tool_result_publication newer ON newer.environment_key = p.environment_key
      AND newer.agent = p.agent AND newer.session_id = p.session_id
      AND newer.source_format = p.source_format AND newer.native_identity = p.native_identity
      AND newer.event_kind = p.event_kind AND newer.publication_sequence > p.publication_sequence
      AND (p.session_incarnation IS NULL OR (newer.session_incarnation IS p.session_incarnation
        AND newer.project_cwd IS p.project_cwd))
    WHERE d.candidate_id = ?1 AND d.dependency_binding_json IS NULL
      AND d.native_revision IS NOT NULL AND d.event_kind = 'tool_result'
      AND json_extract(c.activation_scope_key, '$.run') = 'Live'
      AND newer.outcome IN ('update', 'removal')
      AND newer.native_revision >= p.native_revision
      AND (newer.outcome = 'removal' OR newer.native_revision > p.native_revision
        OR newer.payload_json IS NOT p.payload_json
        OR newer.native_position_json IS NOT p.native_position_json))";

pub(crate) fn reason(reason: ToolResultCompleteness) -> Option<&'static str> {
    match reason {
        ToolResultCompleteness::UnsafeIdentity => Some("native-unsafe-identity"),
        ToolResultCompleteness::UnsafeOrder => Some("native-unsafe-order"),
        ToolResultCompleteness::UnsafeOwnership => Some("native-unsafe-ownership"),
        _ => None,
    }
}

pub(crate) fn is_unsafe(reason: &str) -> bool {
    matches!(
        reason,
        "native-unsafe-identity" | "native-unsafe-order" | "native-unsafe-ownership"
    )
}

pub(crate) fn source_reason_in(
    connection: &Connection,
    session: &SessionKey,
) -> Result<Option<String>> {
    Ok(connection.query_row(&format!("SELECT d.last_error FROM native_file_delivery d JOIN session s
        USING(environment_key, agent, session_id) WHERE d.environment_key = ?1 AND d.agent = ?2 AND d.session_id = ?3
          AND d.incarnation = s.incarnation AND d.last_error IN ({UNSAFE_REASONS_SQL})"),
        params![session.environment_key, session.agent, session.session_id], |row| row.get(0)).optional()?)
}

pub(crate) fn publication_reason_in(
    connection: &Connection,
    sequence: i64,
) -> Result<Option<String>> {
    Ok(connection.query_row(&format!("SELECT u.gap_reason FROM native_tool_result_publication p
        JOIN native_tool_result_publication u ON u.environment_key = p.environment_key AND u.agent = p.agent
          AND u.session_id = p.session_id AND u.source_format = p.source_format
          AND (p.session_incarnation IS NULL OR (u.session_incarnation IS p.session_incarnation AND u.project_cwd IS p.project_cwd))
        WHERE p.publication_sequence = ?1 AND p.native_identity IS NOT NULL
          AND u.publication_sequence > p.publication_sequence AND u.outcome = 'coverage_gap'
          AND u.gap_reason IN ({UNSAFE_REASONS_SQL}) ORDER BY u.publication_sequence DESC LIMIT 1"),
        [sequence], |row| row.get(0)).optional()?)
}

pub(crate) fn event_reason_in(
    connection: &Connection,
    session: &SessionKey,
    sequence: i64,
) -> Result<Option<String>> {
    if let Some(reason) = source_reason_in(connection, session)? {
        return Ok(Some(reason));
    }
    publication_reason_in(connection, sequence)
}

pub(crate) fn invalidate_in(
    tx: &Transaction<'_>,
    session: &SessionKey,
    reason: &str,
) -> Result<super::trace_effects::InvalidationEffects> {
    tx.execute(
        "UPDATE native_file_delivery SET last_error = ?4
        WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![
            session.environment_key,
            session.agent,
            session.session_id,
            reason
        ],
    )?;
    let mut effects = super::observers::break_continuity_in(tx, session)?;
    let branch = session.to_identity_json();
    effects.unfinished_stale += tx.execute(
        "UPDATE burn_check_candidate SET state = 'stale' WHERE session_key_json = ?1
        AND state IN ('pending', 'claimed', 'dispatching', 'answered')",
        [&branch],
    )?;
    effects.accepted_evidence_revoked += tx.execute("UPDATE burn_check_assessment_history SET evidence_valid = 0 WHERE evidence_valid = 1 AND candidate_id IN (
        SELECT c.candidate_id FROM burn_check_candidate c JOIN burn_check_candidate_dependency d USING(candidate_id)
        WHERE c.session_key_json = ?1 AND d.native_revision IS NOT NULL)", [&branch])?;
    effects.current_status_revoked += tx.execute("UPDATE burn_check_current_status SET evidence_valid = 0 WHERE evidence_valid = 1 AND assessment_id IN (
        SELECT assessment_id FROM burn_check_assessment_history WHERE evidence_valid = 0 AND candidate_id IN (
          SELECT candidate_id FROM burn_check_candidate WHERE session_key_json = ?1))", [&branch])?;
    Ok(effects)
}

pub(crate) fn candidate_current_in(connection: &Connection, candidate: &str) -> Result<bool> {
    let session: Option<String> = connection
        .query_row(
            "SELECT session_key_json FROM burn_check_candidate
        WHERE candidate_id = ?1",
            [candidate],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(session) = session {
        let (environment, agent, session): (String, String, String) =
            serde_json::from_str(&session)?;
        if source_reason_in(connection, &SessionKey::new(environment, agent, session))?.is_some() {
            return Ok(false);
        }
    }
    let unsafe_dependency: bool = connection.query_row(&format!("SELECT EXISTS(
        SELECT 1 FROM burn_check_candidate_dependency d JOIN burn_check_candidate c USING(candidate_id)
        JOIN burn_check_observation o ON o.check_id = c.check_id AND o.activation_scope_key = c.activation_scope_key
          AND o.observation_id = d.observation_id AND o.session_key_json = d.session_key_json
        JOIN native_tool_result_publication p ON p.publication_sequence = o.publication_sequence
        JOIN native_tool_result_publication u ON u.environment_key = p.environment_key AND u.agent = p.agent
          AND u.session_id = p.session_id AND u.source_format = p.source_format
          AND (p.session_incarnation IS NULL OR (u.session_incarnation IS p.session_incarnation AND u.project_cwd IS p.project_cwd))
        WHERE d.candidate_id = ?1 AND p.native_identity IS NOT NULL AND u.publication_sequence > p.publication_sequence
          AND u.outcome = 'coverage_gap' AND u.gap_reason IN ({UNSAFE_REASONS_SQL}))"), [candidate], |row| row.get(0))?;
    if unsafe_dependency {
        return Ok(false);
    }
    // Tool-result observations can lag their native correction while a request waits.
    let superseded_dependency: bool =
        connection.query_row(SUPERSEDED_DEPENDENCY_SQL, [candidate], |row| row.get(0))?;
    Ok(!superseded_dependency)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn correction_lookup_uses_the_identity_and_sequence_index_for_updates_and_removals() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        let connection = store.lock();
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {SUPERSEDED_DEPENDENCY_SQL}"))
            .unwrap();
        let plan = statement
            .query_map(["candidate"], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            plan.iter().any(|step| {
                step.contains("SEARCH newer USING INDEX native_evidence_publication_correction")
                    && step.contains("native_identity=?")
                    && step.contains("event_kind=?")
                    && step.contains("publication_sequence>?")
            }),
            "{plan:?}"
        );
    }

    #[test]
    fn untyped_tool_result_fence_matches_exact_native_identity_and_scope_before_catch_up() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        let mut connection = store.lock();
        connection.execute_batch(r#"
            INSERT INTO native_tool_result_publication
                (publication_sequence, outcome, environment_key, agent, session_id,
                 source_format, native_identity, native_position_json, native_revision,
                 payload_json, complete, source_generation, claim_fence, session_incarnation, project_cwd)
            VALUES (1, 'update', 'native', 'pi', 'session', 'native-format', 'result', '{}',
                2, '{"result":"original"}', 1, 1, 1, 7, '/repo');
            INSERT INTO burn_check_candidate
                (candidate_id, check_id, activation_scope_key, generation, branch_key, session_key_json,
                 trigger_position, selected_publication_head, dependencies_json, frozen_input_json,
                 input_fingerprint, state, claim_owner, claim_fence, created_at_epoch, updated_at_epoch)
            VALUES ('candidate', 'check', '{"run":"Live"}', 1, 'branch', '["native","pi","session"]',
                1, 1, '{}', '{}', 'fingerprint', 'dispatching', 'owner', 1, 1, 1);
            INSERT INTO burn_check_observation
                (check_id, activation_scope_key, observation_id, session_key_json, publication_sequence,
                 source_format, native_identity, native_revision, evidence_version, removed, payload_json)
            VALUES ('check', '{"run":"Live"}', 'observation', '["native","pi","session"]',
                1, 'native-format', 'result', 2, 'version', 0, '{}');
            INSERT INTO burn_check_candidate_dependency
                (candidate_id, observation_id, session_key_json, source_format, native_revision,
                 evidence_version, removed, payload_sha256)
            VALUES ('candidate', 'observation', '["native","pi","session"]',
                'native-format', 2, 'version', 0, 'normalized-hash');
        "#).unwrap();
        assert!(candidate_current_in(&connection, "candidate").unwrap());
        for change in [
            "environment",
            "agent",
            "session",
            "format",
            "identity",
            "kind",
            "incarnation",
            "cwd",
            "older",
            "duplicate",
            "revision",
            "payload",
            "position",
            "removal",
        ] {
            let tx = connection.transaction().unwrap();
            tx.execute(
                "INSERT INTO native_tool_result_publication
                    (outcome, environment_key, agent, session_id, source_format, native_identity,
                     native_position_json, native_revision, payload_json, complete, source_generation,
                     claim_fence, session_incarnation, project_cwd, event_kind)
                 SELECT CASE WHEN ?1 = 'removal' THEN 'removal' ELSE 'update' END,
                    CASE WHEN ?1 = 'environment' THEN 'other' ELSE environment_key END,
                    CASE WHEN ?1 = 'agent' THEN 'other' ELSE agent END,
                    CASE WHEN ?1 = 'session' THEN 'other' ELSE session_id END,
                    CASE WHEN ?1 = 'format' THEN 'other' ELSE source_format END,
                    CASE WHEN ?1 = 'identity' THEN 'other' ELSE native_identity END,
                    CASE WHEN ?1 = 'position' THEN '{\"position\":2}' ELSE native_position_json END,
                    CASE WHEN ?1 = 'older' THEN 1 WHEN ?1 IN ('duplicate', 'payload', 'position', 'removal')
                        THEN native_revision ELSE native_revision + 1 END,
                    CASE WHEN ?1 = 'payload' THEN '{\"result\":\"corrected\"}' ELSE payload_json END,
                    complete, source_generation, claim_fence,
                    CASE WHEN ?1 = 'incarnation' THEN session_incarnation + 1 ELSE session_incarnation END,
                    CASE WHEN ?1 = 'cwd' THEN '/other' ELSE project_cwd END,
                    CASE WHEN ?1 = 'kind' THEN 'tool_call' ELSE event_kind END
                 FROM native_tool_result_publication WHERE publication_sequence = 1",
                [change],
            ).unwrap();
            assert_eq!(
                candidate_current_in(&tx, "candidate").unwrap(),
                !matches!(change, "revision" | "payload" | "position" | "removal"),
                "{change}"
            );
            assert_eq!(
                tx.query_row("SELECT state FROM burn_check_candidate", [], |row| row
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
                "dispatching"
            );
            let effects = super::super::evidence::invalidate_native_identity_dependents_in(
                &tx,
                &SessionKey::new("native", "pi", "session"),
                "native-format",
                "result",
                super::super::evidence::EvidenceEventKind::ToolResult,
            )
            .unwrap();
            let superseded = matches!(change, "revision" | "payload" | "position" | "removal");
            assert_eq!(
                effects.unfinished_stale,
                usize::from(superseded),
                "{change}"
            );
            let state: String = tx
                .query_row("SELECT state FROM burn_check_candidate", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(state, if superseded { "stale" } else { "dispatching" });
            tx.rollback().unwrap();
            assert!(candidate_current_in(&connection, "candidate").unwrap());
        }
    }

    #[test]
    fn unsafe_publication_keeps_legacy_unbound_evidence_unusable_after_new_binding() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        store.lock().execute_batch("INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id,
            source_format, native_identity, native_position_json, native_revision, payload_json, complete, source_generation, claim_fence)
            VALUES ('update', 'native', 'pi', 'session', 'legacy-format', 'legacy-result', '{}', 1, '{}', 1, 0, 1);
            INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id, source_format,
                complete, source_generation, claim_fence, session_incarnation, project_cwd, gap_reason)
            VALUES ('coverage_gap', 'native', 'pi', 'session', 'legacy-format', 0, 0, 2, 1, '/repo', 'native-unsafe-identity');
            INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id, source_format,
                complete, source_generation, claim_fence, session_incarnation, project_cwd)
            VALUES ('coverage_complete', 'native', 'pi', 'session', 'legacy-format', 1, 0, 3, 2, '/repo');
            INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id,
                source_format, native_identity, native_position_json, native_revision, payload_json, complete, source_generation, claim_fence,
                session_incarnation, project_cwd)
            VALUES ('update', 'native', 'pi', 'session', 'legacy-format', 'new-result', '{}', 1, '{}', 1, 0, 3, 2, '/repo');").unwrap();
        assert_eq!(
            publication_reason_in(&store.lock(), 1).unwrap().as_deref(),
            Some("native-unsafe-identity")
        );
        assert!(publication_reason_in(&store.lock(), 4).unwrap().is_none());
    }
}
