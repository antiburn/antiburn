use super::factory::publish_jsonl;
use super::*;

const CLAUDE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../crates/antiburn-local/tests/fixtures/claude_characterization/retained_native_results.jsonl"
));
const CODEX: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../crates/antiburn-local/tests/fixtures/codex_characterization/paginated_completed.jsonl"
));
const PI: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../crates/antiburn-local/tests/fixtures/pi_characterization/core_linear_v3.jsonl"
));

fn without_skill(text: &str) -> String {
    text.lines()
        .filter(|line| !line.contains("<skill"))
        .filter(|line| !line.contains("\"UserMessage\""))
        .enumerate()
        .map(|(index, line)| {
            let mut record: serde_json::Value = serde_json::from_str(line).unwrap();
            if record.get("ordinal").is_some() {
                record["ordinal"] = index.into();
            }
            record.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn accepted_retained_sources_parse_publish_request_and_load() {
    for (agent, session, format, text) in [
        (
            "claude",
            "scope-test",
            SourceFormat::ClaudeJsonl,
            CLAUDE.lines().take(7).collect::<Vec<_>>().join("\n"),
        ),
        (
            "codex",
            "synthetic-root",
            SourceFormat::CodexRolloutJsonl,
            without_skill(CODEX),
        ),
        (
            "pi",
            "synthetic",
            SourceFormat::PiV3Jsonl,
            without_skill(PI),
        ),
    ] {
        let (store, key, fence, generation) = publish_jsonl(agent, session, format, &text);
        let request = store.session_scope_request(&key, fence, generation);
        assert!(request.is_ok(), "{agent}: {:?}", request.err());
        let scope = store.load_session_scope(&key, request.unwrap()).unwrap();
        assert!(!scope.values().is_empty(), "{agent}");
        assert!(scope.scope_creep_context().is_ok(), "{agent}");
    }
}

#[test]
fn terminal_metadata_uses_the_last_content_boundary() {
    let text = format!(
        "{}\n{}",
        without_skill(PI),
        serde_json::json!({
            "type": "session_info", "id": "terminal", "parentId": "result",
            "timestamp": "2026-01-01T00:00:07Z", "name": "Synthetic session"
        })
    );
    let (store, key, fence, generation) =
        publish_jsonl("pi", "synthetic", SourceFormat::PiV3Jsonl, &text);
    let request = store
        .session_scope_request(&key, fence, generation)
        .unwrap();
    let last_content: u64 = store
        .lock()
        .query_row(
            "SELECT MAX(t.turn_index) FROM turn t JOIN turn_content c ON c.turn_rowid = t.rowid",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(request.boundary().turn_index, last_content);
    let scope = store.load_session_scope(&key, request).unwrap();
    assert_eq!(
        scope.values()[0],
        "Review the small function. Keep billing unchanged."
    );
}

#[test]
fn codex_metadata_root_and_terminal_record_do_not_require_user_roles() {
    let text = format!(
        "{}\n{}",
        CODEX.lines().take(3).collect::<Vec<_>>().join("\n"),
        serde_json::json!({"timestamp":"2026-10-07T10:00:03Z", "ordinal":3,
            "type":"turn_context", "payload":{"turn_id":"turn-2", "model":"gpt-test", "effort":"low"}})
    );
    let (store, key, fence, generation) = publish_jsonl(
        "codex",
        "synthetic-root",
        SourceFormat::CodexRolloutJsonl,
        &text,
    );
    let request = store
        .session_scope_request(&key, fence, generation)
        .unwrap();
    let scope = store.load_session_scope(&key, request).unwrap();
    assert_eq!(
        scope.values(),
        &[serde_json::json!("Review the sample. Do not publish.")]
    );
}

#[test]
fn claude_injected_context_and_uncharacterized_producer_remain_unavailable() {
    for text in [
        CLAUDE.to_owned(),
        CLAUDE
            .lines()
            .take(7)
            .collect::<Vec<_>>()
            .join("\n")
            .replace("2.1.278", "2.1.277"),
    ] {
        let (store, key, fence, generation) =
            publish_jsonl("claude", "scope-test", SourceFormat::ClaudeJsonl, &text);
        assert!(matches!(
            store.session_scope_request(&key, fence, generation),
            Err(ScopeLoadError::Scope(SessionScopeError::Missing(
                ScopeMissingReason::IncompleteSource
            )))
        ));
    }
}

#[test]
fn retained_source_proofs_and_native_ranges_fail_closed_after_storage_changes() {
    for (agent, session, format, text) in [
        (
            "claude",
            "scope-test",
            SourceFormat::ClaudeJsonl,
            CLAUDE.lines().take(7).collect::<Vec<_>>().join("\n"),
        ),
        (
            "codex",
            "synthetic-root",
            SourceFormat::CodexRolloutJsonl,
            without_skill(CODEX),
        ),
        (
            "pi",
            "synthetic",
            SourceFormat::PiV3Jsonl,
            without_skill(PI),
        ),
    ] {
        for mutation in [
            "UPDATE turn_content SET normalized_fields_json = NULL WHERE kind = 'user' AND authority = 'user'",
            "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json, '$.metadata.user_text_history.source_format', 'open_code_sqlite_v2') WHERE kind = 'user' AND authority = 'user'",
            "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json, '$.metadata.bindings[0].end', 1) WHERE kind = 'user' AND authority = 'user'",
        ] {
            let (store, key, fence, generation) = publish_jsonl(agent, session, format, &text);
            store.lock().execute(mutation, []).unwrap();
            assert!(
                matches!(
                    store.session_scope_request(&key, fence, generation),
                    Err(ScopeLoadError::Scope(SessionScopeError::Missing(
                        ScopeMissingReason::IncompleteSource
                    )))
                ),
                "{agent}: {mutation}"
            );
        }
    }
}

#[test]
fn retained_source_history_loss_keeps_intact_context_with_limits() {
    for (agent, session, format, text) in [
        (
            "claude",
            "scope-test",
            SourceFormat::ClaudeJsonl,
            CLAUDE.lines().take(7).collect::<Vec<_>>().join("\n"),
        ),
        (
            "codex",
            "synthetic-root",
            SourceFormat::CodexRolloutJsonl,
            without_skill(CODEX),
        ),
        (
            "pi",
            "synthetic",
            SourceFormat::PiV3Jsonl,
            without_skill(PI),
        ),
    ] {
        for mutation in [
            "UPDATE turn SET is_compaction_boundary = 1 WHERE turn_index = 0",
            "DELETE FROM turn WHERE turn_index = 0",
        ] {
            let (store, key, fence, generation) = publish_jsonl(agent, session, format, &text);
            store.lock().execute(mutation, []).unwrap();
            let request = store
                .session_scope_request(&key, fence, generation)
                .unwrap();
            assert!(!request.source_complete, "{agent}: {mutation}");
            let scope = store.load_session_scope(&key, request).unwrap();
            assert!(
                scope
                    .limitations()
                    .contains(&ScopeMissingReason::IncompleteSource),
                "{agent}: {mutation}"
            );
            assert!(scope.scope_creep_context().is_ok(), "{agent}: {mutation}");
        }
    }
}

#[test]
fn unknown_skill_context_does_not_become_human_scope() {
    for (agent, session, format, text) in [
        (
            "codex",
            "synthetic-root",
            SourceFormat::CodexRolloutJsonl,
            CODEX,
        ),
        ("pi", "synthetic", SourceFormat::PiV3Jsonl, PI),
    ] {
        let (store, key, fence, generation) = publish_jsonl(agent, session, format, text);
        store.lock().execute(
            "UPDATE turn_content SET normalized_fields_json = json_remove(normalized_fields_json, '$.metadata.selected_skill') WHERE kind = 'user' AND authority != 'user'", [],
        ).unwrap();
        assert!(
            matches!(
                store.session_scope_request(&key, fence, generation),
                Err(ScopeLoadError::Scope(SessionScopeError::Missing(
                    ScopeMissingReason::IncompleteSource
                )))
            ),
            "{agent}"
        );
    }
}

#[test]
fn pi_utf8_human_suffix_keeps_native_offsets_and_omits_typed_skill_context() {
    let text = PI.replace(
        "Keep billing unchanged. Report remaining gaps.",
        "Do not change café billing.",
    );
    let (store, key, fence, generation) =
        publish_jsonl("pi", "synthetic", SourceFormat::PiV3Jsonl, &text);
    let (content, start, end): (Vec<u8>, usize, usize) = store.lock().query_row(
        "SELECT c.content, json_extract(c.normalized_fields_json, '$.metadata.bindings[0].start'),
            json_extract(c.normalized_fields_json, '$.metadata.bindings[0].end')
         FROM turn t JOIN turn_content c ON c.turn_rowid = t.rowid
         WHERE t.uuid = 'selected' AND c.authority = 'user'",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    assert_eq!(content, "Do not change café billing.".as_bytes());
    assert!(start > 0);
    assert_eq!(end - start, content.len());
    let request = store
        .session_scope_request(&key, fence, generation)
        .unwrap();
    let scope = store.load_session_scope(&key, request).unwrap();
    assert!(
        scope
            .values()
            .contains(&serde_json::json!("Do not change café billing."))
    );
    assert!(
        scope
            .values()
            .iter()
            .all(|value| !value.as_str().is_some_and(|text| text.contains("<skill")))
    );
    store.lock().execute(
        "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json, '$.metadata.selected_skill.source_format', 'codex_rollout_jsonl') WHERE kind = 'user' AND authority != 'user'", [],
    ).unwrap();
    assert!(matches!(
        store.session_scope_request(&key, fence, generation),
        Err(ScopeLoadError::Scope(SessionScopeError::Missing(
            ScopeMissingReason::IncompleteSource
        )))
    ));
}
