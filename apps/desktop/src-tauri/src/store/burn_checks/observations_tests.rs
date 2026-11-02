use antiburn_checks::domain::{ActivationId, ActivationScope, BranchId, Generation, RunScope};
use serde_json::json;

use super::*;
use crate::store::burn_checks::StoreCheckStore;
use crate::store::native_tool_results::evidence::SessionObserverKey;
use crate::store::native_tool_results::observers::DependencyBinding;

#[test]
fn branch_invalidation_looks_up_only_the_current_assessment() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    let connection = store.lock();
    let mut statement = connection
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {INVALIDATE_BRANCH_CURRENT_STATUS_SQL}"
        ))
        .unwrap();
    let plan = statement
        .query_map(["check", "scope", "branch"], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(
        plan.iter()
            .any(|step| step.contains("SEARCH h USING INDEX") && step.contains("assessment_id=?")),
        "{plan:?}"
    );
    assert!(
        !plan.iter().any(|step| step.starts_with("SCAN h")),
        "{plan:?}"
    );
}

fn insert_assessment(
    store: &Store,
    id: &str,
    target: (&str, &ActivationScope, &str),
    position: i64,
    idle: bool,
) {
    let (check, scope, branch) = target;
    let scope_key = StoreCheckStore::activation_scope_key(scope);
    let session = json!(["native", "pi", "session"]).to_string();
    let connection = store.lock();
    connection.execute(
        "INSERT INTO burn_check_candidate(candidate_id, check_id, activation_scope_key, generation,
            branch_key, session_key_json, trigger_position, selected_publication_head, dependencies_json,
            frozen_input_json, input_fingerprint, state, created_at_epoch, updated_at_epoch)
         VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, 0, '{}', '{}', 'synthetic', 'accepted', 100, 100)",
        params![id, check, scope_key, branch, session, position],
    ).unwrap();
    connection.execute(
        "INSERT INTO burn_check_candidate_dependency(candidate_id, observation_id, session_key_json,
            source_format, native_revision, evidence_version, removed, payload_sha256)
         VALUES (?1, 'native', ?2, 'synthetic', 1, 'synthetic', 0, 'synthetic')",
        params![id, session],
    ).unwrap();
    if idle {
        let binding = DependencyBinding::SessionActivity {
            observer: SessionObserverKey {
                activation: ActivationId {
                    definition_id: check.into(),
                    scope: scope.clone(),
                    generation: Generation(1),
                },
                branch: BranchId(branch.into()),
                incarnation: 1,
                attachment_epoch: "synthetic".into(),
            },
            activity_revision: 1,
        };
        connection.execute(
            "INSERT INTO burn_check_candidate_dependency(candidate_id, observation_id, session_key_json,
                source_format, native_revision, evidence_version, removed, payload_sha256, dependency_binding_json)
             VALUES (?1, 'idle', ?2, 'attachment-observer', NULL, 'synthetic', 0, '', ?3)",
            params![id, session, serde_json::to_string(&binding).unwrap()],
        ).unwrap();
    }
    connection
        .execute(
            "INSERT INTO burn_check_assessment_history(assessment_id, candidate_id, check_id,
            activation_scope_key, generation, branch_key, trigger_position, outcome, actions_json,
            evidence_valid, accepted_at_epoch)
         VALUES (?1, ?1, ?2, ?3, 1, ?4, ?5, 'assessed', '[]', 1, 100)",
            params![id, check, scope_key, branch, position],
        )
        .unwrap();
    connection.execute(
        "INSERT INTO burn_check_current_status(check_id, activation_scope_key, branch_key, generation,
            status, assessment_id, position, evidence_valid, updated_at_epoch)
         VALUES (?1, ?2, ?3, 1, 'failure', ?4, ?5, 1, 100)
         ON CONFLICT(check_id, activation_scope_key, branch_key) DO UPDATE SET
            assessment_id = excluded.assessment_id, position = excluded.position",
        params![check, scope_key, branch, id, position],
    ).unwrap();
}

#[test]
fn branch_invalidation_preserves_idle_history_and_unrelated_check_scope_and_branch() {
    for current_idle in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        let scope = ActivationScope {
            environment: "native".into(),
            session_cwd: "/synthetic".into(),
            run: RunScope::Live,
        };
        let other_scope = ActivationScope {
            session_cwd: "/other".into(),
            ..scope.clone()
        };
        let scope_key = StoreCheckStore::activation_scope_key(&scope);
        store.lock().execute(
            "INSERT INTO burn_check_activation(check_id, activation_scope_key, definition_revision,
                enabled, generation, bootstrap_sequence, applied_sequence, updated_at_epoch, checkpoint_json)
             VALUES ('check', ?1, 'synthetic', 1, 1, 0, 0, 100, '{}')",
            [&scope_key],
        ).unwrap();
        for (id, idle, position) in if current_idle {
            [("native", false, 1), ("idle", true, 2)]
        } else {
            [("idle", true, 1), ("native", false, 2)]
        } {
            insert_assessment(&store, id, ("check", &scope, "branch"), position, idle);
        }
        insert_assessment(&store, "other-check", ("other", &scope, "branch"), 1, false);
        insert_assessment(
            &store,
            "other-scope",
            ("check", &other_scope, "branch"),
            1,
            false,
        );
        insert_assessment(&store, "other-branch", ("check", &scope, "other"), 1, false);
        assert!(
            store
                .apply_burn_check_publications_with_checkpoint(
                    &BurnCheckPublicationPage {
                        check_id: "check",
                        activation_scope_key: &scope_key,
                        generation: 1,
                        through_sequence: 0,
                        publication_sequences: &[],
                        observations: &[],
                        candidates: &[],
                    },
                    Some(RuntimeCheckpoint {
                        expected: "{}",
                        next: "{\"done\":true}",
                        scope: &scope,
                        invalidated_branches: &["branch".into()],
                        reset_branches: &[],
                        coalesce_pending: false,
                    }),
                )
                .unwrap()
        );
        let connection = store.lock();
        for (id, expected) in [
            ("native", 0),
            ("idle", 1),
            ("other-check", 1),
            ("other-scope", 1),
            ("other-branch", 1),
        ] {
            let valid: i64 = connection.query_row(
                "SELECT evidence_valid FROM burn_check_assessment_history WHERE assessment_id = ?1",
                [id], |row| row.get(0),
            ).unwrap();
            assert_eq!(valid, expected, "{id}, current_idle={current_idle}");
        }
        let mut statement = connection
            .prepare("SELECT assessment_id, evidence_valid FROM burn_check_current_status")
            .unwrap();
        for row in statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap()
        {
            let (id, valid) = row.unwrap();
            assert_eq!(valid, i64::from(id != "native"), "{id}");
        }
    }
}
