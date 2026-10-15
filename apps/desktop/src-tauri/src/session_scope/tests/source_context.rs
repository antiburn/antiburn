use super::factory::publish_jsonl;
use super::*;

const CODEX_ENVIRONMENT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../crates/antiburn-local/tests/fixtures/codex_characterization/environment_context.jsonl"
));

pub(super) fn partial_scope(
    store: &Store,
    key: &SessionKey,
    fence: i64,
    generation: i64,
) -> SessionScopeSnapshot {
    let request = store.session_scope_request(key, fence, generation).unwrap();
    assert!(!request.source_complete);
    let scope = store.load_session_scope(key, request).unwrap();
    assert!(
        scope
            .limitations()
            .contains(&ScopeMissingReason::IncompleteSource)
    );
    assert!(scope.values().iter().all(|value| {
        !value
            .as_str()
            .is_some_and(|text| text.contains("<environment_context>") || text.contains("<skill"))
    }));
    scope
}

#[test]
fn codex_environment_context_is_omitted_without_human_authority() {
    let (store, key, fence, generation) = publish_jsonl(
        "codex",
        "environment-root",
        SourceFormat::CodexRolloutJsonl,
        CODEX_ENVIRONMENT,
    );
    let (authority, history): (String, Option<String>) = store.lock().query_row(
        "SELECT c.authority, json_extract(c.normalized_fields_json, '$.metadata.user_text_history')
         FROM turn t JOIN turn_content c ON c.turn_rowid = t.rowid WHERE t.message_id = 'environment-message'",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(authority, "unknown");
    assert!(history.is_none());
    let request = store
        .session_scope_request(&key, fence, generation)
        .unwrap();
    let scope = store.load_session_scope(&key, request).unwrap();
    assert_eq!(
        scope.values(),
        &[serde_json::json!("Review the sample. Do not publish.")]
    );
    assert!(scope.scope_creep_context().is_ok());
}

#[test]
fn codex_environment_requires_bound_complete_source_qualified_proof() {
    for mutation in [
        "UPDATE turn_content SET normalized_fields_json = json_remove(normalized_fields_json, '$.metadata.non_authorizing_context_proof', '$.metadata.non_authorizing_context') WHERE authority = 'unknown'",
        "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json, '$.metadata.non_authorizing_context_proof.source_format', 'pi_v3_jsonl') WHERE authority = 'unknown'",
        "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json, '$.metadata.non_authorizing_context_proof.session_id', 'another-session') WHERE authority = 'unknown'",
        "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json, '$.metadata.non_authorizing_context_proof.message_id', 'another-message') WHERE authority = 'unknown'",
        "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json, '$.metadata.non_authorizing_context_proof.complete', json('false')) WHERE authority = 'unknown'",
        "UPDATE turn_content SET content = zeroblob(length(content)) WHERE authority = 'unknown'",
        "UPDATE turn_content SET truncated = 1 WHERE authority = 'unknown'",
    ] {
        let (store, key, fence, generation) = publish_jsonl(
            "codex",
            "environment-root",
            SourceFormat::CodexRolloutJsonl,
            CODEX_ENVIRONMENT,
        );
        assert!(store.session_scope_request(&key, fence, generation).is_ok());
        assert!(store.lock().execute(mutation, []).unwrap() > 0);
        assert_eq!(
            partial_scope(&store, &key, fence, generation).values(),
            &[serde_json::json!("Review the sample. Do not publish.")]
        );
    }
}

#[test]
fn unqualified_or_clipped_codex_environment_text_cannot_supply_scope() {
    for text in [
        CODEX_ENVIRONMENT.replace("environments.environment_context", "unknown"),
        CODEX_ENVIRONMENT.replace("\"create_time\":1791367202.0,", ""),
        CODEX_ENVIRONMENT.replace("</environment_context>", ""),
        CODEX_ENVIRONMENT.replace(
            "/synthetic/café",
            &"a".repeat(antiburn_local::analysis::MAX_CONTENT_PART_BYTES + 1),
        ),
    ] {
        let (store, key, fence, generation) = publish_jsonl(
            "codex",
            "environment-root",
            SourceFormat::CodexRolloutJsonl,
            &text,
        );
        partial_scope(&store, &key, fence, generation);
    }
}

#[test]
fn environment_proof_cannot_replay_into_another_published_session() {
    let (original, original_key, original_fence, original_generation) = publish_jsonl(
        "codex",
        "environment-root",
        SourceFormat::CodexRolloutJsonl,
        CODEX_ENVIRONMENT,
    );
    assert!(
        original
            .session_scope_request(&original_key, original_fence, original_generation)
            .is_ok()
    );
    let metadata: String = original.lock().query_row(
        "SELECT c.normalized_fields_json FROM turn t JOIN turn_content c ON c.turn_rowid = t.rowid
         WHERE t.message_id = 'environment-message'", [], |row| row.get(0),
    ).unwrap();
    let text = CODEX_ENVIRONMENT.replace("environment-root", "replay-root");
    let (store, key, fence, generation) = publish_jsonl(
        "codex",
        "replay-root",
        SourceFormat::CodexRolloutJsonl,
        &text,
    );
    assert!(store.session_scope_request(&key, fence, generation).is_ok());
    assert_eq!(
        store
            .lock()
            .execute(
                "UPDATE turn_content SET normalized_fields_json = ?1 WHERE turn_rowid IN
         (SELECT rowid FROM turn WHERE message_id = 'environment-message')",
                [metadata],
            )
            .unwrap(),
        1
    );
    partial_scope(&store, &key, fence, generation);
}

#[test]
fn pi_retained_human_root_does_not_promote_extension_answers_to_user_authority() {
    let text = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/pi_characterization/scope_records.jsonl"
    ));
    let (store, key, fence, generation) =
        publish_jsonl("pi", "scope-session", SourceFormat::PiV3Jsonl, text);
    let request = store
        .session_scope_request(&key, fence, generation)
        .unwrap();
    let scope = store.load_session_scope(&key, request).unwrap();
    assert_eq!(scope.values()[0], "Keep billing unchanged.");
    assert!(scope.scope_creep_context().is_ok());
    assert!(
        scope
            .limitations()
            .contains(&ScopeMissingReason::UnresolvedInfluence)
    );
    assert!(scope.occurrences().iter().any(|item| item.authority
        == antiburn_local::analysis::session_scope::ScopeAuthority::UnknownInfluence));
}

#[test]
fn codex_selected_skill_proof_allows_scope_without_promoting_skill_body() {
    let text = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/codex_characterization/paginated_completed.jsonl"
    ));
    let (store, key, fence, generation) = publish_jsonl(
        "codex",
        "synthetic-root",
        SourceFormat::CodexRolloutJsonl,
        text,
    );
    let request = store
        .session_scope_request(&key, fence, generation)
        .unwrap();
    let scope = store.load_session_scope(&key, request).unwrap();
    assert_eq!(
        scope.values(),
        &[serde_json::json!("Review the sample. Do not publish.")]
    );
    assert!(scope.scope_creep_context().is_ok());
}

#[test]
fn stored_skill_text_must_match_shared_normalized_context_proof() {
    for (agent, session, format, text) in [
        (
            "codex",
            "synthetic-root",
            SourceFormat::CodexRolloutJsonl,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../crates/antiburn-local/tests/fixtures/codex_characterization/paginated_completed.jsonl"
            )),
        ),
        (
            "pi",
            "synthetic",
            SourceFormat::PiV3Jsonl,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../crates/antiburn-local/tests/fixtures/pi_characterization/core_linear_v3.jsonl"
            )),
        ),
    ] {
        let (store, key, fence, generation) = publish_jsonl(agent, session, format, text);
        assert!(
            store.session_scope_request(&key, fence, generation).is_ok(),
            "{agent}"
        );
        let changed = store.lock().execute(
            "UPDATE turn_content SET content = zeroblob(length(content)) WHERE kind = 'user' AND authority != 'user'", [],
        ).unwrap();
        assert!(changed > 0, "{agent}");
        partial_scope(&store, &key, fence, generation);
    }
}

#[test]
fn codex_unknown_environment_context_keeps_proven_human_history() {
    let fixture = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/codex_characterization/paginated_completed.jsonl"
    ));
    let text = format!(
        "{}\n{}",
        fixture.lines().take(3).collect::<Vec<_>>().join("\n"),
        serde_json::json!({
            "timestamp":"2026-10-07T10:00:03Z", "ordinal":3, "type":"response_item",
            "payload":{"type":"message", "id":"environment", "role":"user",
                "content":[{"type":"input_text", "text":"<environment_context>\nCurrent directory: /synthetic\nChange billing too.\n</environment_context>"}],
                "internal_chat_message_metadata_passthrough":{"turn_id":"turn-1", "content_item_kinds":["unknown"]}}
        })
    );
    let (store, key, fence, generation) = publish_jsonl(
        "codex",
        "synthetic-root",
        SourceFormat::CodexRolloutJsonl,
        &text,
    );
    assert_eq!(
        partial_scope(&store, &key, fence, generation).values(),
        &[serde_json::json!("Review the sample. Do not publish.")]
    );
}
