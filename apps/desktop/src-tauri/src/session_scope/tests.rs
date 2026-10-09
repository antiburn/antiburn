use super::*;
use antiburn_local::analysis::{ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION};
use rusqlite::params;
use std::path::Path;

mod factory;
mod retained_sources;
mod source_context;

fn fixture() -> Store {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-scope-tests")).unwrap();
    {
        let conn = store.lock();
        for environment in ["native", "wsl:synthetic", "remote:synthetic"] {
            conn.execute(
                "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
                 first_seen_at, last_seen_at, source_generation, source_fingerprint)
                 VALUES (?1, 'claude-code', 'scope', 'file', 'synthetic', 'now', 'now', 3, 'source')",
                params![environment],
            ).unwrap();
            conn.execute(
                "INSERT INTO session_evidence (environment_key, agent, session_id, status,
                 analyzed_generation, processed_fingerprint, parser_revision, analyzer_revision,
                 evidence_schema_revision, published_fence, claim_fence)
                 VALUES (?1, 'claude-code', 'scope', 'ready', 3, 'source', ?2, ?3, ?4, 4, 4)",
                params![
                    environment,
                    PARSER_REVISION,
                    ANALYZER_REVISION,
                    EVIDENCE_SCHEMA_REVISION
                ],
            )
            .unwrap();
            for index in 0..600 {
                conn.execute(
                    "INSERT INTO turn (environment_key, agent, session_id, claim_fence,
                     source_key, thread_id, turn_index, scope, role, ts_ms, uuid,
                     input_tokens, cache_read_tokens, cache_write_tokens, output_tokens, is_compaction_boundary)
                     VALUES (?1, 'claude-code', 'scope', 4, 'transcript', 'branch', ?2, 'main',
                     'user', ?2, ?3, 0, 0, 0, 0, 0)",
                    params![environment, index, format!("event-{index}")],
                ).unwrap();
                let rowid = conn.last_insert_rowid();
                conn.execute(
                    "INSERT INTO turn_content (turn_rowid, part_index, kind, authority, content, truncated)
                     VALUES (?1, 0, 'user', 'user', ?2, 0)",
                    params![rowid, format!("{environment}: instruction {index}\n  Do not delete.").as_bytes()],
                ).unwrap();
            }
        }
    }
    store
}

fn load(
    store: &Store,
    environment: &str,
    fence: i64,
    generation: i64,
    complete: bool,
) -> Result<SessionScopeSnapshot, ScopeLoadError> {
    store.load_session_scope(
        &SessionKey::new(environment, "claude-code", "scope"),
        ScopeLoadRequest {
            untrusted_user_parts: Default::default(),
            source_format: SourceFormat::ClaudeJsonl,
            boundary: SessionScopeBoundary {
                source_key: "transcript".into(),
                thread_id: "branch".into(),
                turn_index: 598,
                part_index: 0,
                branch: antiburn_local::analysis::session_scope::SessionScopeBranch::ProvenLinear,
            },
            publication_fence: fence,
            source_generation: generation,
            source_complete: complete,
            key: SessionKey::new(environment, "claude-code", "scope"),
        },
    )
}

#[test]
fn full_paging_keeps_pre_enablement_context_and_environment_boundaries() {
    let store = fixture();
    for environment in ["native", "wsl:synthetic", "remote:synthetic"] {
        let scope = load(&store, environment, 4, 3, true).unwrap();
        assert_eq!(scope.occurrences().len(), 599);
        assert_eq!(
            scope.values()[0],
            format!("{environment}: instruction 0\n  Do not delete.")
        );
        assert_eq!(
            scope.occurrences().last().unwrap().reference.turn_index,
            598
        );
        assert!(
            scope
                .values()
                .iter()
                .all(|value| value.as_str().unwrap().starts_with(environment))
        );
    }
}

#[test]
fn stale_publication_generation_cannot_supply_scope() {
    let store = fixture();
    assert!(matches!(
        load(&store, "native", 5, 3, true),
        Err(ScopeLoadError::Scope(SessionScopeError::Missing(
            ScopeMissingReason::PublicationChanged
        )))
    ));
    assert!(matches!(
        load(&store, "native", 4, 9, true),
        Err(ScopeLoadError::Scope(SessionScopeError::Missing(
            ScopeMissingReason::PublicationChanged
        )))
    ));
    let partial = load(&store, "native", 4, 3, false).unwrap();
    assert_eq!(partial.occurrences().len(), 599);
    assert_eq!(
        partial.limitations(),
        &[ScopeMissingReason::IncompleteSource]
    );
    assert_eq!(
        partial.scope_creep_context().unwrap().fields["limitations"],
        serde_json::json!(["IncompleteSource"])
    );
}

#[test]
fn ordinary_edit_and_tool_result_boundaries_need_no_selected_body() {
    let store = fixture();
    for kind in ["tool_input", "tool_result"] {
        {
            let conn = store.lock();
            conn.execute(
                "UPDATE turn_content SET kind = ?1, authority = ?2, content = ?3,
                tool_name = 'edit', tool_call_id = 'boundary-call' WHERE turn_rowid IN
                (SELECT rowid FROM turn WHERE environment_key = 'native' AND turn_index = 598)",
                params![
                    kind,
                    if kind == "tool_result" {
                        "tool"
                    } else {
                        "assistant"
                    },
                    vec![b'x'; 300_000]
                ],
            )
            .unwrap();
        }
        let snapshot = load(&store, "native", 4, 3, true).unwrap();
        assert_eq!(snapshot.occurrences().len(), 598);
        assert_eq!(
            snapshot.occurrences().last().unwrap().reference.turn_index,
            597
        );
        assert!(
            snapshot
                .values()
                .iter()
                .all(|value| value.as_str().unwrap().len() < 100)
        );
    }
}

#[test]
fn scoped_query_excludes_oversized_siblings_and_post_boundary_before_coverage() {
    let store = fixture();
    {
        let conn = store.lock();
        conn.execute(
            "UPDATE turn_content SET content = ?1 WHERE turn_rowid IN
            (SELECT rowid FROM turn WHERE environment_key = 'native' AND turn_index IN (300, 599))",
            params![vec![b'x'; 300_000]],
        )
        .unwrap();
        conn.execute(
            "UPDATE turn_content SET truncated = 1 WHERE turn_rowid IN
            (SELECT rowid FROM turn WHERE environment_key = 'native' AND turn_index = 301)",
            [],
        )
        .unwrap();
    }
    let boundary = SessionScopeBoundary {
        source_key: "transcript".into(),
        thread_id: "branch".into(),
        turn_index: 598,
        part_index: 0,
        branch: antiburn_local::analysis::session_scope::SessionScopeBranch::NativeRecords(
            (0..599)
                .filter(|index| ![300, 301].contains(index))
                .map(|index| format!("event-{index}"))
                .collect(),
        ),
    };
    let snapshot = store
        .load_session_scope(
            &SessionKey::new("native", "claude-code", "scope"),
            ScopeLoadRequest {
                untrusted_user_parts: Default::default(),
                source_format: SourceFormat::ClaudeJsonl,
                boundary,
                publication_fence: 4,
                source_generation: 3,
                source_complete: true,
                key: SessionKey::new("native", "claude-code", "scope"),
            },
        )
        .unwrap();
    assert_eq!(snapshot.occurrences().len(), 597);
    assert!(
        snapshot
            .values()
            .iter()
            .all(|value| value.as_str().unwrap().len() < 100)
    );
    let partial = load(&store, "native", 4, 3, true).unwrap();
    assert!(
        partial
            .limitations()
            .contains(&ScopeMissingReason::TruncatedEvidence)
    );
    assert_eq!(partial.occurrences().len(), 597);
}

#[test]
fn scoped_cursor_rejects_a_changed_boundary_before_continuation() {
    let store = fixture();
    let key = SessionKey::new("native", "claude-code", "scope");
    let mut filter = antiburn_local::analysis::SelectedContentScope {
        source_key: "transcript".into(),
        thread_id: "branch".into(),
        turn_index: 598,
        part_index: 0,
        native_record_ids: None,
    };
    let positions = BTreeMap::new();
    let first = store
        .published_turn_content_keyset_scoped(
            &key,
            SelectedContentRequest {
                source_generation: 3,
                after_ms: None,
                source_positions: &positions,
                selection: SCOPE_SELECTION,
                cursor: None,
            },
            &filter,
        )
        .unwrap()
        .unwrap();
    assert!(first.boundary.is_some());
    assert!(first.page.next_cursor.is_some());
    filter.turn_index = 597;
    assert!(matches!(
        store.published_turn_content_keyset_scoped(
            &key,
            SelectedContentRequest {
                source_generation: 3,
                after_ms: None,
                source_positions: &positions,
                selection: SCOPE_SELECTION,
                cursor: first.page.next_cursor.as_ref(),
            },
            &filter
        ),
        Err(SelectedContentQueryError::StaleCursor)
    ));
}

#[test]
fn partial_scope_keeps_intact_context_despite_a_truncated_sibling() {
    let store = fixture();
    store
        .lock()
        .execute(
            "UPDATE turn_content SET truncated = 1 WHERE turn_rowid IN
         (SELECT rowid FROM turn WHERE environment_key = 'native' AND turn_index = 300)",
            [],
        )
        .unwrap();
    let partial = load(&store, "native", 4, 3, false).unwrap();
    assert_eq!(partial.occurrences().len(), 598);
    assert_eq!(
        partial.values()[0],
        "native: instruction 0\n  Do not delete."
    );
    assert!(
        partial
            .limitations()
            .contains(&ScopeMissingReason::TruncatedEvidence)
    );
    assert!(partial.scope_creep_context().is_ok());
}

#[test]
fn factory_admits_retained_root_with_child_attribution_and_collection_limits() {
    let text = include_str!(concat!(env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/claude_characterization/retained_native_results.jsonl"))
        .lines().take(7).collect::<Vec<_>>().join("\n");
    let (store, key, fence, generation) =
        factory::publish_jsonl("claude", "scope-test", SourceFormat::ClaudeJsonl, &text);
    let original = store
        .load_session_scope(
            &key,
            store
                .session_scope_request(&key, fence, generation)
                .unwrap(),
        )
        .unwrap();
    let mut coverage = store.published_coverage_record(&key).unwrap().unwrap();
    coverage.subagent_linkage_incomplete = true;
    coverage.subagents_cap_exceeded = true;
    coverage
        .diagnostics
        .capped_collections
        .insert("subagent_children".into());
    antiburn_local::analysis::insert_coverage_record(
        &store.lock(),
        &crate::session_scope::factory::turn_session_key(&key),
        fence,
        &coverage,
    )
    .unwrap();
    let request = store
        .session_scope_request(&key, fence, generation)
        .unwrap();
    assert!(!request.source_complete);
    let partial = store.load_session_scope(&key, request).unwrap();
    assert_eq!(partial.values(), original.values());
    assert_eq!(partial.occurrences(), original.occurrences());
    assert_eq!(
        partial.limitations(),
        &[ScopeMissingReason::IncompleteSource]
    );
    assert!(partial.scope_creep_context().is_ok());
}

#[test]
fn factory_keeps_missing_root_history_as_partial_context() {
    let text = include_str!(concat!(env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/claude_characterization/retained_native_results.jsonl"))
        .lines().take(7).collect::<Vec<_>>().join("\n");
    let (store, key, fence, generation) =
        factory::publish_jsonl("claude", "scope-test", SourceFormat::ClaudeJsonl, &text);
    store
        .lock()
        .execute(
            "DELETE FROM turn_content WHERE turn_rowid IN
                  (SELECT rowid FROM turn WHERE turn_index = 0)",
            [],
        )
        .unwrap();
    let request = store
        .session_scope_request(&key, fence, generation)
        .unwrap();
    assert!(!request.source_complete);
    let partial = store.load_session_scope(&key, request).unwrap();
    assert!(partial.values().is_empty());
    assert!(
        partial
            .limitations()
            .contains(&ScopeMissingReason::NoUserContext)
    );
    assert!(
        partial
            .limitations()
            .contains(&ScopeMissingReason::IncompleteSource)
    );
    assert!(partial.scope_creep_context().is_ok());
}

#[test]
fn partial_scope_excludes_private_thinking_from_model_context() {
    let store = fixture();
    store
        .lock()
        .execute(
            "UPDATE turn_content SET kind = 'thinking', authority = 'assistant', content = ?1
         WHERE turn_rowid IN (SELECT rowid FROM turn WHERE environment_key = 'native'
         AND turn_index = 300)",
            params![b"private synthetic reasoning".as_slice()],
        )
        .unwrap();
    let partial = load(&store, "native", 4, 3, false).unwrap();
    assert_eq!(partial.occurrences().len(), 598);
    let context = partial.scope_creep_context().unwrap();
    assert!(
        !serde_json::to_string(&context.fields)
            .unwrap()
            .contains("private synthetic reasoning")
    );
}
