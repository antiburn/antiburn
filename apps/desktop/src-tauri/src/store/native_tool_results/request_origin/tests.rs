use super::*;
use antiburn_local::analysis::file_live::read_native_file_page;
use antiburn_local::analysis::{RawSource, SessionInput, SourceFormat};
use serde_json::json;

fn fixture() -> (tempfile::TempDir, crate::store::Store, SessionKey) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.jsonl");
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/claude_characterization/retained_native_results.jsonl"
    ));
    std::fs::write(&path, source).unwrap();
    let input = SessionInput {
        agent: "claude".into(),
        session_id: "scope-test".into(),
        source: RawSource::File(path.clone()),
        source_format: SourceFormat::ClaudeJsonl,
        fork_parent_session_id: None,
    };
    let page = read_native_file_page(&input, None, &|| false).unwrap();
    let store = crate::store::Store::open_in_memory(directory.path()).unwrap();
    let key = SessionKey::new("native", "claude-code", "scope-test");
    let scope = json!({"environment":"native", "session_cwd":"/repo", "run":"Live"}).to_string();
    let connection = store.lock();
    connection.execute("INSERT INTO session (environment_key, agent, session_id, source_kind,
        source_label, cwd, first_seen_at, last_seen_at) VALUES ('native', 'claude-code', 'scope-test',
        'file', ?1, '/repo', 'now', 'now')", [path.to_string_lossy().as_ref()]).unwrap();
    connection.execute("INSERT INTO native_file_delivery(environment_key, agent, session_id, incarnation,
        source_label, cwd, parser_revision, cursor_json, active_until_epoch)
        SELECT environment_key, agent, session_id, incarnation, source_label, cwd, 65, ?1, 1000 FROM session",
        [serde_json::to_string(&page.cursor).unwrap()]).unwrap();
    connection
        .execute(
            "INSERT INTO burn_check_activation(check_id, activation_scope_key, definition_revision,
        enabled, generation, bootstrap_sequence, applied_sequence, updated_at_epoch)
        VALUES ('origin', ?1, 'synthetic', 1, 1, 0, 100, 100)",
            [&scope],
        )
        .unwrap();
    connection.execute("INSERT INTO burn_check_candidate(candidate_id, check_id, activation_scope_key,
        generation, branch_key, session_key_json, trigger_position, selected_publication_head,
        dependencies_json, frozen_input_json, input_fingerprint, state, created_at_epoch, updated_at_epoch)
        VALUES ('candidate', 'origin', ?1, 1, ?2, ?2, 1, 100, '{}', '{}', 'candidate', 'pending', 100, 100)",
        params![scope, key.to_identity_json()]).unwrap();
    drop(connection);
    (directory, store, key)
}

fn marker(store: &crate::store::Store, key: &SessionKey) -> i64 {
    let mut connection = store.lock();
    let transaction = connection.transaction().unwrap();
    assert!(ensure_boundary_in(&transaction, key, 1, 1).unwrap());
    let sequence = transaction
        .query_row(
            &format!(
                "SELECT {} FROM session s WHERE s.session_id = 'scope-test'",
                boundary_sql("s")
            ),
            [],
            |row| row.get(0),
        )
        .unwrap();
    transaction.commit().unwrap();
    sequence
}

#[test]
fn origin_proof_accepts_only_parser_65_66_or_current_with_cursor_5() {
    use antiburn_local::analysis::PARSER_REVISION;

    let (_directory, store, key) = fixture();
    marker(&store, &key);
    let ready = source_ready_sql("s");
    for (parser, cursor, accepted) in [
        (62, 4, false),
        (63, 5, false),
        (64, 1, false),
        (64, 5, false),
        (65, 1, false),
        (65, 4, false),
        (65, 5, true),
        (66, 1, false),
        (66, 4, false),
        (66, 5, true),
        (PARSER_REVISION, 1, false),
        (PARSER_REVISION, 4, false),
        (PARSER_REVISION, 5, true),
        (PARSER_REVISION + 1, 5, false),
    ] {
        let connection = store.lock();
        connection
            .execute(
                "UPDATE native_file_delivery SET parser_revision = ?1,
                cursor_json = json_set(cursor_json, '$.revision', ?2)",
                params![parser, cursor],
            )
            .unwrap();
        assert_eq!(
            new_candidate_request_origin_current_in(&connection, "candidate").unwrap(),
            accepted,
            "parser {parser}, cursor {cursor}"
        );
        assert_eq!(
            connection
                .query_row(&format!("SELECT {ready} FROM session s"), [], |row| row
                    .get::<_, bool>(0))
                .unwrap(),
            accepted,
            "parser {parser}, cursor {cursor}"
        );
    }
}

#[test]
fn parser_65_boundary_requires_safe_receipt_consumed_reset_and_new_candidate_head() {
    let (_directory, store, key) = fixture();
    assert!(!new_candidate_request_origin_current_in(&store.lock(), "candidate").unwrap());
    let boundary = marker(&store, &key);
    assert!(new_candidate_request_origin_current_in(&store.lock(), "candidate").unwrap());
    for invalid in [
        "UPDATE native_file_delivery SET parser_revision = 64",
        "UPDATE native_file_delivery SET cursor_json = json_set(cursor_json, '$.revision', 4)",
        "UPDATE native_file_delivery SET cursor_json = json_set(cursor_json, '$.session_id', 'other')",
        "UPDATE native_file_delivery SET source_label = 'other'",
        "UPDATE native_file_delivery SET cwd = '/other'",
        "UPDATE session SET source_label = 'other'",
        "UPDATE session SET source_format = '\"pi_v3_jsonl\"'",
        "UPDATE session SET incarnation = incarnation + 1",
        "UPDATE burn_check_activation SET applied_sequence = 0",
        "UPDATE burn_check_activation SET generation = 2",
    ] {
        let connection = store.lock();
        connection
            .execute_batch("SAVEPOINT invalid_origin_receipt")
            .unwrap();
        connection.execute_batch(invalid).unwrap();
        assert!(
            !new_candidate_request_origin_current_in(&connection, "candidate").unwrap(),
            "{invalid}"
        );
        connection
            .execute_batch("ROLLBACK TO invalid_origin_receipt; RELEASE invalid_origin_receipt")
            .unwrap();
    }
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET selected_publication_head = ?1",
            [boundary],
        )
        .unwrap();
    assert!(!new_candidate_request_origin_current_in(&store.lock(), "candidate").unwrap());
    store
        .lock()
        .execute(
            "UPDATE burn_check_candidate SET selected_publication_head = ?1",
            [boundary + 1],
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE burn_check_activation SET applied_sequence = ?1",
            [boundary],
        )
        .unwrap();
    assert!(new_candidate_request_origin_current_in(&store.lock(), "candidate").unwrap());
}

#[test]
fn parser_65_origin_proof_survives_read_failures_while_startup_waits_for_source_health() {
    let (_directory, store, key) = fixture();
    marker(&store, &key);
    let ready = source_ready_sql("s");
    for failure in [
        "UPDATE native_file_delivery SET blocked = 1",
        "UPDATE native_file_delivery SET last_error = 'file-read-failed'",
        "UPDATE native_file_delivery SET last_error = 'file-unavailable'",
        "UPDATE native_file_delivery SET last_error = 'file-cursor-invalid'",
    ] {
        let connection = store.lock();
        connection
            .execute_batch("SAVEPOINT failed_origin_read")
            .unwrap();
        assert!(
            connection
                .query_row(&format!("SELECT {ready} FROM session s"), [], |row| row
                    .get::<_, bool>(0))
                .unwrap()
        );
        connection.execute_batch(failure).unwrap();
        assert!(
            !connection
                .query_row(&format!("SELECT {ready} FROM session s"), [], |row| row
                    .get::<_, bool>(0))
                .unwrap()
        );
        assert!(
            new_candidate_request_origin_current_in(&connection, "candidate").unwrap(),
            "{failure}"
        );
        connection
            .execute_batch("ROLLBACK TO failed_origin_read; RELEASE failed_origin_read")
            .unwrap();
    }
    let connection = store.lock();
    connection
        .execute(
            "UPDATE native_file_delivery SET last_error = 'native-unsafe-order'",
            [],
        )
        .unwrap();
    assert!(new_candidate_request_origin_current_in(&connection, "candidate").unwrap());
    assert!(
        !crate::store::native_tool_results::observers::typed_dependencies_current_in(
            &connection,
            "candidate"
        )
        .unwrap()
    );
}

#[test]
fn parser_65_session_deletion_removes_the_bounded_marker_binding() {
    let (_directory, store, key) = fixture();
    marker(&store, &key);
    assert!(store.delete_session(&key).unwrap().is_some());
    let connection = store.lock();
    assert!(
        !connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM setting WHERE key LIKE ?1)",
                [format!("{BINDING_PREFIX}%")],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
    assert!(
        !connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_tool_result_publication)",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
}

#[test]
fn parser_65_boundary_is_append_only_and_exact_to_source_label_incarnation_and_project() {
    let (_directory, store, key) = fixture();
    let first = marker(&store, &key);
    let mut connection = store.lock();
    let transaction = connection.transaction().unwrap();
    assert!(!ensure_boundary_in(&transaction, &key, 2, 2).unwrap());
    transaction
        .execute("UPDATE session SET incarnation = incarnation + 1", [])
        .unwrap();
    assert!(ensure_boundary_in(&transaction, &key, 2, 2).unwrap());
    transaction
        .execute("UPDATE session SET source_label = 'another.jsonl'", [])
        .unwrap();
    assert!(ensure_boundary_in(&transaction, &key, 2, 2).unwrap());
    transaction
        .execute("UPDATE session SET cwd = '/other'", [])
        .unwrap();
    assert!(ensure_boundary_in(&transaction, &key, 2, 2).unwrap());
    transaction.commit().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM native_tool_result_publication WHERE gap_reason = ?1",
                [RESET_REASON],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        4
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM setting WHERE key LIKE ?1",
                [format!("{BINDING_PREFIX}%")],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert!(connection.execute("UPDATE native_tool_result_publication SET gap_reason = 'changed' WHERE publication_sequence = ?1", [first]).is_err());
}

#[test]
fn parser_65_origin_gate_does_not_apply_to_other_agents_environments_or_replay() {
    for scope in [
        json!({"environment":"native", "session_cwd":"/repo", "run":{"Replay":"synthetic"}}),
        json!({"environment":"native", "session_cwd":"/repo", "run":"Live"}),
    ] {
        let (_directory, store, _key) = fixture();
        store
            .lock()
            .execute(
                "UPDATE burn_check_candidate SET activation_scope_key = ?1",
                [scope.to_string()],
            )
            .unwrap();
        if scope["run"] == "Live" {
            store.lock().execute("UPDATE burn_check_candidate SET session_key_json = json_array('native', 'pi', 'scope-test')", []).unwrap();
        }
        assert!(new_candidate_request_origin_current_in(&store.lock(), "candidate").unwrap());
    }
    let (_directory, store, _key) = fixture();
    store.lock().execute("UPDATE burn_check_candidate SET session_key_json = json_array('remote', 'claude-code', 'scope-test')", []).unwrap();
    assert!(new_candidate_request_origin_current_in(&store.lock(), "candidate").unwrap());
}
