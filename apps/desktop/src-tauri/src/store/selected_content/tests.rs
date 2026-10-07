use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use antiburn_local::analysis::jev_evidence::{
    JevAnswerSelection, JevNativeFieldContainer, JevNativeFieldRange, JevPlanContentStatus,
    JevPlanReference, JevPlanStatus, JevQuestionOption, JevScopeEvidenceProvenance,
    JevScopeEvidenceRole, JevScopeEvidenceSource, JevUserAnswer, JevUserAnswerOrigin,
    JevUserAnswerStatus,
};
use antiburn_local::analysis::{
    ContentKind, ContentPart, JevInputField, JevInputSelection, MAX_CONTENT_PART_BYTES,
    SelectedContentCursor, SourceFormat, TurnRow, TurnRowStore, TurnScope,
};

use super::*;

fn fixture() -> (Store, SessionKey) {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-keyset-tests")).unwrap();
    fixture_in(store)
}

#[test]
fn nearby_identity_probe_uses_the_source_position_index_without_sorting_history() {
    let (store, key) = fixture();
    let connection = store.lock();
    for (sql, range) in [
        (NEARBY_CONTENT_IDENTITIES_SQL, "turn_index<?"),
        (FOLLOWING_CONTENT_IDENTITIES_SQL, "turn_index>?"),
    ] {
        let plan = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map(
                params![
                    key.environment_key,
                    key.agent,
                    key.session_id,
                    11,
                    "a",
                    70,
                    1
                ],
                |row| row.get::<_, String>(3),
            )
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            plan.iter().any(|line| line
                .contains("SEARCH turn USING INDEX turn_content_source_page")
                && line.contains("source_key=?")
                && line.contains(range)),
            "{plan:?}"
        );
        assert!(
            plan.iter()
                .any(|line| line.contains("SEARCH content USING PRIMARY KEY (turn_rowid=?)")),
            "{plan:?}"
        );
        assert!(
            !plan.iter().any(|line| line.starts_with("SCAN turn")
                || line.starts_with("SCAN content")
                || line.contains("USE TEMP B-TREE")),
            "{plan:?}"
        );
    }
}

fn fixture_in(store: Store) -> (Store, SessionKey) {
    let key = SessionKey::new("native", "claude-code", "keyset");
    {
        let conn = store.lock();
        conn.execute_batch(
            "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
                first_seen_at, last_seen_at, source_generation, source_fingerprint)
             VALUES ('native', 'claude-code', 'keyset', 'file', 'synthetic', 'now', 'now', 7, 'source');",
        ).unwrap();
        conn.execute(
            "INSERT INTO session_evidence (environment_key, agent, session_id, status,
                analyzed_generation, processed_fingerprint, parser_revision, analyzer_revision,
                evidence_schema_revision, published_fence, claim_fence)
             VALUES ('native', 'claude-code', 'keyset', 'ready', 7, 'source', ?1, ?2, ?3, 11, 11)",
            params![
                PARSER_REVISION,
                antiburn_local::analysis::ANALYZER_REVISION,
                EVIDENCE_SCHEMA_REVISION
            ],
        )
        .unwrap();
        for source in ["a", "b"] {
            for ordinal in 0..90 {
                for tie in 0..2 {
                    conn.execute(
                        "INSERT INTO turn (environment_key, agent, session_id, claim_fence,
                            source_key, thread_id, turn_index, scope, role, ts_ms, uuid,
                            input_tokens, cache_read_tokens, cache_write_tokens, output_tokens,
                            is_compaction_boundary)
                         VALUES ('native', 'claude-code', 'keyset', 11, ?1, ?1, ?2, 'main',
                            'assistant', 2000, ?3, 0, 0, 0, 0, 0)",
                        params![source, ordinal, format!("{source}-{ordinal}-{tie}")],
                    )
                    .unwrap();
                    let rowid = conn.last_insert_rowid();
                    for part in 0..3 {
                        conn.execute(
                            "INSERT INTO turn_content (turn_rowid, part_index, kind, authority, content,
                                truncated) VALUES (?1, ?2, 'assistant', 'assistant', ?3, 0)",
                            params![rowid, part, format!("{source}-{ordinal}-{tie}-{part}").as_bytes()],
                        ).unwrap();
                    }
                }
            }
        }
    }
    (store, key)
}

fn scope_part() -> ContentPart {
    let source = JevScopeEvidenceSource {
        source_format: SourceFormat::ClaudeJsonl,
        role: JevScopeEvidenceRole::Tool,
        native_record_id: Some("record-1".into()),
        call_id: Some("call-1".into()),
        question_id: Some("question-1".into()),
        order: 2,
        acceptance_order: Some(7),
        provenance: JevScopeEvidenceProvenance::RecognizedQuestionWorkflow,
        producer_revision: "synthetic-storage-contract-v1".into(),
        normalization_revision: 1,
        bindings: vec![JevNativeFieldRange {
            native_record_id: Some("record-1".into()),
            field: JevInputField::UserAnswer,
            container: JevNativeFieldContainer::Record,
            pointer: "/answer".into(),
            start: 0,
            end: 12,
        }],
        truncated: false,
    };
    let answer = JevUserAnswer {
        source: source.clone(),
        prompt: "Which scope?\nKeep the existing exclusions.".into(),
        header: Some("Scope".into()),
        context: Some("Keep billing unchanged.\n  Keep nested conditions.".into()),
        comment: Some("Keep the retry limit.".into()),
        options: vec![JevQuestionOption {
            id: Some("api".into()),
            value: Some("api-only".into()),
            label: "API only".into(),
            description: Some("Do not change billing.\n  Keep nested conditions.".into()),
        }],
        multi_select: Some(true),
        selections: vec![JevAnswerSelection {
            option_id: Some("api".into()),
            option_index: Some(0),
            value: Some("api-only".into()),
            label: Some("API only".into()),
            custom: Some(false),
        }],
        free_text: Some("No, keep this exact condition:\n```\n  retries = 2\n```".into()),
        status: JevUserAnswerStatus::Submitted,
        origin: JevUserAnswerOrigin::User,
    };
    let plan = JevPlanReference {
        source: JevScopeEvidenceSource {
            provenance: JevScopeEvidenceProvenance::RecognizedPlanWorkflow,
            order: 3,
            bindings: vec![JevNativeFieldRange {
                native_record_id: Some("record-1".into()),
                field: JevInputField::PlanReference,
                container: JevNativeFieldContainer::Record,
                pointer: "/plan".into(),
                start: 0,
                end: 20,
            }],
            ..source
        },
        plan_id: Some("plan-1".into()),
        path: Some("plans/task.md".into()),
        revision: Some("v2".into()),
        content_digest: Some("current-digest".into()),
        approved_revision: Some("v1".into()),
        approved_content_digest: Some("approved-digest".into()),
        text: Some("# Scope\n- Keep API\n  - Do not change billing\n".into()),
        feedback: Some("Keep the earlier exclusion.".into()),
        status: JevPlanStatus::Feedback,
        origin: JevUserAnswerOrigin::User,
        content_status: JevPlanContentStatus::MutableCompanion,
    };
    let mut answers = vec![answer.clone()];
    for status in [
        JevUserAnswerStatus::Skipped,
        JevUserAnswerStatus::Cancelled,
        JevUserAnswerStatus::TimedOut,
        JevUserAnswerStatus::Pending,
        JevUserAnswerStatus::Unknown,
    ] {
        answers.push(JevUserAnswer {
            status,
            ..answer.clone()
        });
    }
    for origin in [
        JevUserAnswerOrigin::Synthetic,
        JevUserAnswerOrigin::Automatic,
        JevUserAnswerOrigin::UnknownOrigin,
    ] {
        answers.push(JevUserAnswer {
            origin,
            ..answer.clone()
        });
    }
    ContentPart::new(ContentKind::AssistantText, "ordinary assistant text")
        .with_tool_identity(Some("synthetic-workflow".into()), Some("call-1".into()))
        .with_scope_evidence(answers, vec![plan])
        .unwrap()
}

fn write_scope_parts(store: &Store, key: &SessionKey, fence: i64, source: &str, count: usize) {
    write_parts(store, key, fence, source, vec![scope_part(); count]);
}

fn write_parts(
    store: &Store,
    key: &SessionKey,
    fence: i64,
    source: &str,
    content: Vec<ContentPart>,
) {
    let row = TurnRow {
        source_key: source.into(),
        thread_id: format!("branch-{source}"),
        turn_index: 0,
        scope: TurnScope::Main,
        child_id: None,
        role: "assistant",
        ts_ms: Some(2000),
        model: None,
        provider: None,
        api: None,
        effort: None,
        speed: None,
        input_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cache_write_1h_tokens: 0,
        output_tokens: 0,
        is_compaction_boundary: false,
        message_id: Some(format!("message-{source}")),
        uuid: Some(format!("uuid-{source}")),
        parent_uuid: None,
        compaction_trigger: None,
        compaction_pre_tokens: None,
        compaction_post_tokens: None,
        has_thinking: false,
        last_tool: None,
        subagent_launches: 0,
        content,
    };
    super::super::FencedTurnRowStore::new(store.clone(), key.clone(), fence)
        .write_turn_rows(&[row])
        .unwrap();
}

fn read_scope(
    store: &Store,
    key: &SessionKey,
    selection: JevInputSelection,
    cursor: Option<&SelectedContentCursor>,
) -> Result<Option<SelectedContentPage>, SelectedContentQueryError> {
    store.published_turn_content_keyset_selected(
        key,
        SelectedContentRequest {
            source_generation: 7,
            after_ms: None,
            source_positions: &BTreeMap::new(),
            selection,
            cursor,
        },
    )
}

fn scope_selection() -> JevInputSelection {
    JevInputSelection::from_fields(&[JevInputField::UserAnswer, JevInputField::PlanReference])
}

#[test]
fn persisted_scope_metadata_reload_preserves_typed_records_and_explicit_opt_in() {
    let dir = tempfile::tempdir().unwrap();
    let (store, key) = fixture_in(Store::open(dir.path()).unwrap());
    store.lock().execute("DELETE FROM turn", []).unwrap();
    write_scope_parts(&store, &key, 11, "scope", 1);
    write_parts(
        &store,
        &key,
        11,
        "ordinary-tools",
        [
            "question",
            "AskUserQuestion",
            "request_user_input",
            "ordinary",
        ]
        .into_iter()
        .map(|name| {
            ContentPart::new(
                ContentKind::ToolResult,
                r#"{"answers":[["approved"]],"status":"submitted","origin":"user"}"#,
            )
            .with_tool_identity(Some(name.into()), Some(format!("call-{name}")))
        })
        .collect(),
    );
    let ignored = antiburn_local::checks::ignored_instructions::INPUT_SELECTION;
    let baseline = read_scope(&store, &key, ignored, None).unwrap().unwrap();
    assert_eq!(baseline.content.parts.len(), 1);
    assert_eq!(baseline.content.parts[0].source_key, "scope");
    assert_eq!(
        baseline.content.parts[0].part.text,
        "ordinary assistant text"
    );
    let expected = scope_part();
    let before = read_scope(&store, &key, scope_selection(), None)
        .unwrap()
        .unwrap();
    assert_eq!(before.content.parts.len(), 1);
    assert_eq!(before.content.parts[0].part.metadata, expected.metadata);
    let reader = store
        .open_reader(std::time::Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        read_scope(&reader, &key, scope_selection(), None)
            .unwrap()
            .unwrap(),
        before
    );
    drop(reader);
    drop(store);

    let store = Store::open(dir.path()).unwrap();
    assert_eq!(
        read_scope(&store, &key, scope_selection(), None)
            .unwrap()
            .unwrap(),
        before
    );
    for fields in [
        vec![JevInputField::UserAnswer],
        vec![JevInputField::PlanReference],
        vec![JevInputField::UserAnswer, JevInputField::PlanReference],
    ] {
        let selection = JevInputSelection::from_fields(&fields);
        let page = read_scope(&store, &key, selection, None).unwrap().unwrap();
        let part = &page.content.parts[0].part;
        assert!(part.text.is_empty());
        assert_eq!(part.metadata, expected.metadata.selected(selection));
        assert_eq!(page.content.parts[0].source_key, "scope");
        assert_eq!(page.content.parts[0].thread_id, "branch-scope");
    }
    for selection in [JevInputSelection::ALL, ignored] {
        let page = read_scope(&store, &key, selection, None).unwrap().unwrap();
        assert!(
            page.content
                .parts
                .iter()
                .all(|part| part.part.metadata.user_answers.is_empty()
                    && part.part.metadata.plan_references.is_empty())
        );
    }
    store
        .lock()
        .execute(
            "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json,
         '$.metadata.user_answers[0].free_text', 'changed answer',
         '$.metadata.plan_references[0].revision', 'v3') WHERE normalized_fields_json IS NOT NULL",
            [],
        )
        .unwrap();
    assert_eq!(
        read_scope(&store, &key, ignored, None).unwrap().unwrap(),
        baseline
    );
    let prepare = |content| {
        antiburn_local::analysis::jev_evidence::prepare_session_content(
            "keyset",
            SourceFormat::ClaudeJsonl,
            content,
            Vec::new(),
        )
    };
    let selected_before = antiburn_local::analysis::jev_evidence::select_session_content(
        &prepare(baseline.content),
        ignored,
    );
    let selected_after = antiburn_local::analysis::jev_evidence::select_session_content(
        &prepare(
            read_scope(&store, &key, ignored, None)
                .unwrap()
                .unwrap()
                .content,
        ),
        ignored,
    );
    assert_eq!(selected_after, selected_before);
    assert_ne!(
        read_scope(&store, &key, scope_selection(), None)
            .unwrap()
            .unwrap(),
        before
    );
}

#[test]
fn persisted_scope_cursor_restart_preserves_order_and_excludes_other_sessions_and_fences() {
    let dir = tempfile::tempdir().unwrap();
    let (store, key) = fixture_in(Store::open(dir.path()).unwrap());
    write_scope_parts(&store, &key, 11, "scope-a", 135);
    write_scope_parts(&store, &key, 11, "scope-b", 135);
    write_scope_parts(&store, &key, 12, "unpublished", 1);
    for other in [
        SessionKey::new("remote", "claude-code", "keyset"),
        SessionKey::new("native", "opencode", "keyset"),
        SessionKey::new("native", "claude-code", "other"),
    ] {
        {
            let conn = store.lock();
            conn.execute(
                "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
                 first_seen_at, last_seen_at, source_generation, source_fingerprint)
                 VALUES (?1, ?2, ?3, 'file', 'synthetic', 'now', 'now', 7, 'source')",
                params![other.environment_key, other.agent, other.session_id],
            ).unwrap();
            conn.execute(
                "INSERT INTO session_evidence (environment_key, agent, session_id, status,
                 analyzed_generation, processed_fingerprint, parser_revision, analyzer_revision,
                 evidence_schema_revision, published_fence, claim_fence)
                 SELECT ?1, ?2, ?3, status, analyzed_generation, processed_fingerprint,
                 parser_revision, analyzer_revision, evidence_schema_revision, published_fence, claim_fence
                 FROM session_evidence WHERE environment_key = 'native'
                 AND agent = 'claude-code' AND session_id = 'keyset'",
                params![other.environment_key, other.agent, other.session_id],
            ).unwrap();
        }
        write_scope_parts(&store, &other, 11, "other-session", 1);
    }
    let first = read_scope(&store, &key, scope_selection(), None)
        .unwrap()
        .unwrap();
    let cursor = first
        .next_cursor
        .as_ref()
        .expect("typed metadata spans pages");
    let expected = read_scope(&store, &key, scope_selection(), Some(cursor))
        .unwrap()
        .unwrap();
    store.set_internal_value(
        "internal:scopeProgress",
        &serde_json::to_string(&SelectedContentProgress {
            revision: SELECTED_CONTENT_PROGRESS_REVISION,
            cursor: Some(cursor.clone()),
        })
        .unwrap(),
    );
    drop(store);

    let store = Store::open(dir.path()).unwrap();
    let progress: SelectedContentProgress =
        serde_json::from_str(&store.internal_value("internal:scopeProgress").unwrap()).unwrap();
    assert_eq!(progress.revision, SELECTED_CONTENT_PROGRESS_REVISION);
    let resumed = read_scope(&store, &key, scope_selection(), progress.cursor.as_ref())
        .unwrap()
        .unwrap();
    assert_eq!(resumed, expected);
    for other in [
        SessionKey::new("remote", "claude-code", "keyset"),
        SessionKey::new("native", "opencode", "keyset"),
        SessionKey::new("native", "claude-code", "other"),
    ] {
        assert_eq!(
            read_scope(&store, &other, scope_selection(), None)
                .unwrap()
                .unwrap()
                .content
                .parts
                .len(),
            1
        );
        assert!(matches!(
            read_scope(&store, &other, scope_selection(), progress.cursor.as_ref()),
            Err(SelectedContentQueryError::StaleCursor)
        ));
    }
    let mut cursor = resumed.next_cursor;
    let mut parts = first.content.parts;
    parts.extend(resumed.content.parts);
    while let Some(next) = cursor {
        let page = read_scope(&store, &key, scope_selection(), Some(&next))
            .unwrap()
            .unwrap();
        cursor = page.next_cursor;
        parts.extend(page.content.parts);
    }
    assert_eq!(parts.len(), 270);
    let positions = parts
        .iter()
        .map(|part| (part.source_key.as_str(), part.part_index))
        .collect::<Vec<_>>();
    assert_eq!(
        positions,
        ["scope-a", "scope-b"]
            .into_iter()
            .flat_map(|source| (0..135).map(move |index| (source, index)))
            .collect::<Vec<_>>()
    );
    assert!(
        parts
            .iter()
            .all(|part| part.part.metadata == scope_part().metadata && part.part.text.is_empty())
    );
    store
        .lock()
        .execute(
            "UPDATE turn SET ts_ms = NULL WHERE source_key = 'scope-a'",
            [],
        )
        .unwrap();
    let filtered = store
        .published_turn_content_keyset_selected(
            &key,
            SelectedContentRequest {
                source_generation: 7,
                after_ms: Some(1000),
                source_positions: &BTreeMap::from([("scope-a".into(), 0)]),
                selection: scope_selection(),
                cursor: None,
            },
        )
        .unwrap()
        .unwrap();
    let mut cursor = filtered.next_cursor;
    let mut filtered_parts = filtered.content.parts;
    while let Some(next) = cursor {
        let page = store
            .published_turn_content_keyset_selected(
                &key,
                SelectedContentRequest {
                    source_generation: 7,
                    after_ms: Some(1000),
                    source_positions: &BTreeMap::from([("scope-a".into(), 0)]),
                    selection: scope_selection(),
                    cursor: Some(&next),
                },
            )
            .unwrap()
            .unwrap();
        cursor = page.next_cursor;
        filtered_parts.extend(page.content.parts);
    }
    assert_eq!(
        filtered_parts
            .iter()
            .filter(|part| !part.context_only)
            .count(),
        135
    );
    assert!(filtered_parts.iter().all(|part| if part.context_only {
        matches!(part.source_key.as_str(), "scope-a" | "scope-b")
    } else {
        part.source_key == "scope-b"
    }));
}

#[test]
fn persisted_scope_cursor_restart_rejects_changed_inputs_and_stale_publications() {
    let dir = tempfile::tempdir().unwrap();
    let (store, key) = fixture_in(Store::open(dir.path()).unwrap());
    write_scope_parts(&store, &key, 11, "scope", 270);
    let first = read_scope(&store, &key, scope_selection(), None)
        .unwrap()
        .unwrap();
    store.set_internal_value(
        "internal:scopeProgress",
        &serde_json::to_string(&SelectedContentProgress {
            revision: SELECTED_CONTENT_PROGRESS_REVISION,
            cursor: first.next_cursor,
        })
        .unwrap(),
    );
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    let progress: SelectedContentProgress =
        serde_json::from_str(&store.internal_value("internal:scopeProgress").unwrap()).unwrap();
    let cursor = progress.cursor.as_ref().unwrap();
    for selection in [
        JevInputSelection::ALL,
        JevInputSelection::from_fields(&[JevInputField::UserAnswer]),
    ] {
        assert!(matches!(
            read_scope(&store, &key, selection, Some(cursor)),
            Err(SelectedContentQueryError::StaleCursor)
        ));
    }
    for (generation, after_ms, positions) in [
        (8, None, BTreeMap::new()),
        (7, Some(1000), BTreeMap::new()),
        (7, None, BTreeMap::from([("scope".into(), 0)])),
    ] {
        let result = store.published_turn_content_keyset_selected(
            &key,
            SelectedContentRequest {
                source_generation: generation,
                after_ms,
                source_positions: &positions,
                selection: scope_selection(),
                cursor: Some(cursor),
            },
        );
        if generation == 8 {
            assert!(result.unwrap().is_none());
        } else {
            assert!(matches!(
                result,
                Err(SelectedContentQueryError::StaleCursor)
            ));
        }
    }
    for change in [
        "UPDATE session_evidence SET status = 'processing'",
        "UPDATE session SET source_generation = 8",
        "UPDATE session SET source_fingerprint = 'replacement'",
        "UPDATE session_evidence SET analyzed_generation = 8",
        "UPDATE session_evidence SET processed_fingerprint = 'replacement'",
        "UPDATE session_evidence SET parser_revision = 0",
        "UPDATE session_evidence SET analyzer_revision = 0",
        "UPDATE session_evidence SET evidence_schema_revision = 0",
        "UPDATE session_evidence SET published_fence = NULL",
        "DELETE FROM session_evidence",
        "DELETE FROM session",
    ] {
        let changed_dir = tempfile::tempdir().unwrap();
        let (changed_store, changed_key) = fixture_in(Store::open(changed_dir.path()).unwrap());
        write_scope_parts(&changed_store, &changed_key, 11, "scope", 270);
        let saved_cursor = read_scope(&changed_store, &changed_key, scope_selection(), None)
            .unwrap()
            .unwrap()
            .next_cursor
            .unwrap();
        changed_store.lock().execute(change, []).unwrap();
        drop(changed_store);
        let changed_store = Store::open(changed_dir.path()).unwrap();
        assert!(
            read_scope(
                &changed_store,
                &changed_key,
                scope_selection(),
                Some(&saved_cursor)
            )
            .unwrap()
            .is_none(),
            "{change}"
        );
    }
    store
        .lock()
        .execute("UPDATE session_evidence SET published_fence = 12", [])
        .unwrap();
    assert!(matches!(
        read_scope(&store, &key, scope_selection(), Some(cursor)),
        Err(SelectedContentQueryError::StaleCursor)
    ));
    assert!(
        read_scope(&store, &key, scope_selection(), None)
            .unwrap()
            .unwrap()
            .content
            .parts
            .is_empty()
    );
    store
        .lock()
        .execute_batch(
            "UPDATE session_evidence SET published_fence = 11, analyzed_generation = 8;
         UPDATE session SET source_generation = 8;",
        )
        .unwrap();
    assert!(matches!(
        store.published_turn_content_keyset_selected(
            &key,
            SelectedContentRequest {
                source_generation: 8,
                after_ms: None,
                source_positions: &BTreeMap::new(),
                selection: scope_selection(),
                cursor: Some(cursor),
            }
        ),
        Err(SelectedContentQueryError::StaleCursor)
    ));
}

fn read(
    store: &Store,
    key: &SessionKey,
    cursor: Option<&SelectedContentCursor>,
    after_ms: Option<i64>,
) -> SelectedContentPage {
    store
        .published_turn_content_keyset_selected(
            key,
            SelectedContentRequest {
                source_generation: 7,
                after_ms,
                source_positions: &BTreeMap::new(),
                selection: JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
                cursor,
            },
        )
        .unwrap()
        .unwrap()
}

#[test]
fn selected_pages_resume_json_with_source_turn_and_part_ties() {
    for after_ms in [None, Some(1000)] {
        let (store, key) = fixture();
        let mut cursor = None;
        let mut seen = BTreeSet::new();
        loop {
            let page = read(&store, &key, cursor.as_ref(), after_ms);
            for part in page.content.parts.iter().filter(|part| !part.context_only) {
                assert!(
                    seen.insert(part.part.text.clone()),
                    "duplicate selected part"
                );
            }
            let Some(next) = page.next_cursor else { break };
            let saved = SelectedContentProgress {
                revision: SELECTED_CONTENT_PROGRESS_REVISION,
                cursor: Some(next),
            };
            store.set_internal_value(
                "internal:keysetTest",
                &serde_json::to_string(&saved).unwrap(),
            );
            let restored: SelectedContentProgress =
                serde_json::from_str(&store.internal_value("internal:keysetTest").unwrap())
                    .unwrap();
            assert_eq!(restored.revision, SELECTED_CONTENT_PROGRESS_REVISION);
            cursor = restored.cursor;
        }
        assert_eq!(seen.len(), 1080);
    }
}

#[test]
fn selected_resume_survives_earlier_deletion_and_excludes_unpublished_rows() {
    let (store, key) = fixture();
    let first = read(&store, &key, None, None);
    let expected = read(&store, &key, first.next_cursor.as_ref(), None);
    {
        let conn = store.lock();
        conn.execute(
            "DELETE FROM turn WHERE rowid = (SELECT MIN(rowid) FROM turn)",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO turn (environment_key, agent, session_id, claim_fence, source_key,
                thread_id, turn_index, scope, role, input_tokens, cache_read_tokens,
                cache_write_tokens, output_tokens, is_compaction_boundary)
             VALUES ('native', 'claude-code', 'keyset', 12, 'a', 'a', 45, 'main', 'assistant', 0, 0, 0, 0, 0);
             INSERT INTO turn_content (turn_rowid, part_index, kind, authority, content, truncated)
             VALUES (last_insert_rowid(), 0, 'assistant', 'assistant', CAST('unpublished' AS BLOB), 0);",
        ).unwrap();
    }
    assert_eq!(
        read(&store, &key, first.next_cursor.as_ref(), None),
        expected
    );
}

#[test]
fn selected_resume_rejects_changed_selection_positions_and_fence() {
    let (store, key) = fixture();
    let first = read(&store, &key, None, Some(1000));
    let positions = BTreeMap::from([("a".to_owned(), 2)]);
    for (selection, source_positions, after_ms) in [
        (JevInputSelection::ALL, &BTreeMap::new(), Some(1000)),
        (
            JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
            &positions,
            Some(1000),
        ),
        (
            JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
            &BTreeMap::new(),
            Some(1001),
        ),
    ] {
        assert!(matches!(
            store.published_turn_content_keyset_selected(
                &key,
                SelectedContentRequest {
                    source_generation: 7,
                    after_ms,
                    source_positions,
                    selection,
                    cursor: first.next_cursor.as_ref(),
                }
            ),
            Err(SelectedContentQueryError::StaleCursor)
        ));
    }
    store
        .lock()
        .execute("UPDATE session_evidence SET published_fence = 12", [])
        .unwrap();
    assert!(matches!(
        store.published_turn_content_keyset_selected(
            &key,
            SelectedContentRequest {
                source_generation: 7,
                after_ms: Some(1000),
                source_positions: &BTreeMap::new(),
                selection: JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
                cursor: first.next_cursor.as_ref(),
            }
        ),
        Err(SelectedContentQueryError::StaleCursor)
    ));
}

#[test]
fn selected_adapter_rejects_stale_publications_before_reading_content() {
    for change in [
        "UPDATE session_evidence SET status = 'processing'",
        "UPDATE session SET source_generation = 8",
        "UPDATE session SET source_fingerprint = 'replacement'",
        "UPDATE session_evidence SET parser_revision = 0",
        "UPDATE session_evidence SET evidence_schema_revision = 0",
    ] {
        let (store, key) = fixture();
        let first = read(&store, &key, None, None);
        store.lock().execute(change, []).unwrap();
        assert!(
            store
                .published_turn_content_keyset_selected(
                    &key,
                    SelectedContentRequest {
                        source_generation: 7,
                        after_ms: None,
                        source_positions: &BTreeMap::new(),
                        selection: JevInputSelection::ALL,
                        cursor: first.next_cursor.as_ref(),
                    }
                )
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn large_excluded_bodies_do_not_use_selected_page_budget() {
    let (store, key) = fixture();
    let expected = read(&store, &key, None, Some(1000));
    {
        let conn = store.lock();
        conn.execute(
            "INSERT INTO turn_content (turn_rowid, part_index, kind, authority, tool_name,
                content, truncated) VALUES ((SELECT MAX(rowid) FROM turn), 3,
                'tool_result', 'tool', 'bash', ?1, 0)",
            [vec![b'x'; 2 * 1024 * 1024]],
        )
        .unwrap();
    }
    assert_eq!(read(&store, &key, None, Some(1000)), expected);
}

#[test]
fn shared_adapter_keeps_selected_edit_paths_and_other_checks_outputs() {
    let (store, key) = fixture();
    {
        let conn = store.lock();
        let fields = serde_json::json!({
            "category": "file_edit", "malformed": false,
            "values": {"file_edit_path": "src/app.rs", "file_edit_content": "x".repeat(2 * 1024 * 1024)}
        });
        conn.execute(
            "INSERT INTO turn_content (turn_rowid, part_index, kind, authority, tool_name,
                content, truncated, normalized_fields_json)
             VALUES ((SELECT MAX(rowid) FROM turn), 3, 'tool_input', 'assistant', 'edit',
                CAST('' AS BLOB), 0, ?1)",
            [fields.to_string()],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO turn_content (turn_rowid, part_index, kind, authority, tool_name,
                content, truncated) VALUES ((SELECT MAX(rowid) FROM turn), 4,
                'tool_result', 'tool', 'bash', CAST('selected output' AS BLOB), 0);",
        )
        .unwrap();
    }
    for (field, expected) in [
        (JevInputField::FileEditPath, "src/app.rs"),
        (JevInputField::BashCommandOutput, "selected output"),
    ] {
        let page = store
            .published_turn_content_keyset_selected(
                &key,
                SelectedContentRequest {
                    source_generation: 7,
                    after_ms: None,
                    source_positions: &BTreeMap::new(),
                    selection: JevInputSelection::from_fields(&[field]),
                    cursor: None,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(page.content.parts.len(), 1);
        assert!(page.next_cursor.is_none());
        assert_eq!(page.content.coverage.oversized_parts, 0);
        let part = &page.content.parts[0].part;
        if let Some(normalized) = &part.normalized_fields {
            assert_eq!(normalized.values.len(), 1);
            assert_eq!(normalized.values.get(&field).unwrap(), expected);
            assert!(part.text.is_empty());
        } else {
            assert_eq!(part.text, expected);
        }
    }
}

fn store_native_fixture(
    store: &Store,
    key: &SessionKey,
    agent: &str,
    session_id: &str,
    source_format: SourceFormat,
    records: &str,
) -> Vec<ContentPart> {
    use antiburn_local::analysis::{
        NormalizedRecord, RawSource, RecordSink, SessionInput, SessionSummary, TurnRowSink,
        reader_for,
    };
    struct Sink {
        rows: TurnRowSink,
        parts: Vec<ContentPart>,
    }
    impl RecordSink for Sink {
        fn record(&mut self, record: NormalizedRecord) {
            self.rows.observe(&record);
            if let NormalizedRecord::TurnContent(content) = record {
                self.parts.extend(content.parts);
            }
        }
        fn finish(&mut self, _: SessionSummary) {
            self.rows.flush();
        }
    }
    let mut sink = Sink {
        rows: TurnRowSink::new(
            std::sync::Arc::new(super::super::FencedTurnRowStore::new(
                store.clone(),
                key.clone(),
                11,
            )),
            "native-fixture",
            None,
        ),
        parts: Vec::new(),
    };
    reader_for(agent)
        .visit(
            &SessionInput {
                agent: agent.into(),
                session_id: session_id.into(),
                source: RawSource::Jsonl(records.into()),
                fork_parent_session_id: None,
                source_format,
            },
            &mut sink,
        )
        .unwrap();
    sink.rows.flush();
    assert!(!sink.rows.has_error());
    sink.parts
}

#[test]
fn native_human_read_and_output_proofs_survive_parse_store_selected_query() {
    use antiburn_local::analysis::jev_evidence::JevReadStatus;
    let pi = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/pi_characterization/core_linear_v3.jsonl"
    ))
    .replace(
        "Keep billing unchanged. Report remaining gaps.",
        "Do not edit café. Report remaining gaps.",
    );
    for (agent, session_id, format, records) in [
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
            "claude",
            "scope-test",
            SourceFormat::ClaudeJsonl,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../crates/antiburn-local/tests/fixtures/claude_characterization/retained_native_results.jsonl"
            )),
        ),
        ("pi", "synthetic", SourceFormat::PiV3Jsonl, pi.as_str()),
    ] {
        let (store, key) = fixture();
        store.lock().execute("DELETE FROM turn", []).unwrap();
        let parsed = store_native_fixture(&store, &key, agent, session_id, format, records);
        if agent == "codex" {
            let page = read_scope(
                &store,
                &key,
                JevInputSelection::from_fields(&[JevInputField::ReadFileRequest]),
                None,
            )
            .unwrap()
            .unwrap();
            assert_eq!(page.content.parts.len(), 1);
            assert_eq!(
                page.content.parts[0].part.metadata.read_request.as_ref(),
                parsed.iter().find_map(|p| p.metadata.read_request.as_ref())
            );
            let paths = read_scope(
                &store,
                &key,
                JevInputSelection::from_fields(&[JevInputField::FileEditPath]),
                None,
            )
            .unwrap()
            .unwrap();
            assert_eq!(paths.content.parts.len(), 1);
            let part = &paths.content.parts[0].part;
            let paths: serde_json::Value = serde_json::from_str(
                &part.normalized_fields.as_ref().unwrap().values[&JevInputField::FileEditPath],
            )
            .unwrap();
            assert_eq!(paths["paths"], serde_json::json!(["/synthetic/sample.py"]));
            assert!(part.metadata.bindings.is_empty());
        }
        for (field, proof_count) in [
            (
                JevInputField::UserMessage,
                if agent == "codex" { 1 } else { 2 },
            ),
            (JevInputField::ReadFileResult, 1),
        ] {
            let page = read_scope(&store, &key, JevInputSelection::from_fields(&[field]), None)
                .unwrap()
                .unwrap();
            let selected = page
                .content
                .parts
                .iter()
                .map(|p| &p.part)
                .collect::<Vec<_>>();
            if field == JevInputField::UserMessage {
                let humans = selected
                    .iter()
                    .filter(|p| p.metadata.user_text_history.is_some())
                    .collect::<Vec<_>>();
                assert_eq!(humans.len(), proof_count, "{agent}");
                for human in humans {
                    let original = parsed.iter().find(|p| p.text == human.text).unwrap();
                    assert_eq!(
                        human.metadata.user_text_history,
                        original.metadata.user_text_history
                    );
                    assert_eq!(
                        human.metadata.bindings,
                        original
                            .metadata
                            .bindings
                            .iter()
                            .filter(|b| b.field == field)
                            .cloned()
                            .collect::<Vec<_>>()
                    );
                }
                if agent == "pi" {
                    let suffix = selected
                        .iter()
                        .find(|p| p.text.starts_with("Do not edit café"))
                        .unwrap();
                    let binding = &suffix.metadata.bindings[0];
                    assert!(binding.start > 0);
                    assert_eq!(binding.end - binding.start, suffix.text.len());
                    assert_ne!(suffix.text.len(), suffix.text.chars().count());
                }
            } else {
                let results = selected
                    .iter()
                    .filter_map(|p| p.metadata.read_result.as_ref())
                    .collect::<Vec<_>>();
                assert_eq!(results.len(), proof_count, "{agent}");
                let original = parsed
                    .iter()
                    .find_map(|p| p.metadata.read_result.as_ref())
                    .unwrap();
                assert_eq!(results[0], original, "{agent}");
                assert_eq!(results[0].status, JevReadStatus::Success);
                assert!(results[0].returned_extent.is_some());
                assert!(selected.iter().all(|p| p.metadata.read_request.is_none()));
            }
            assert!(
                selected
                    .iter()
                    .all(|p| p.metadata.bindings.iter().all(|b| b.field == field))
            );
        }
        for field in [
            JevInputField::BashCommandOutput,
            JevInputField::ReadFileOutput,
            JevInputField::OtherToolOutput,
        ] {
            let page = read_scope(&store, &key, JevInputSelection::from_fields(&[field]), None)
                .unwrap()
                .unwrap();
            let expected_count = if field == JevInputField::ReadFileOutput {
                1
            } else {
                usize::from(agent != "pi")
            };
            assert_eq!(
                page.content.parts.len(),
                expected_count,
                "{agent} {field:?}"
            );
            for part in page.content.parts {
                let original = parsed
                    .iter()
                    .find(|p| {
                        p.kind == part.part.kind
                            && p.tool_call_id == part.part.tool_call_id
                            && p.text == part.part.text
                    })
                    .unwrap();
                assert_eq!(part.part.metadata.state, original.metadata.state);
                assert_eq!(
                    part.part.metadata.bindings,
                    original
                        .metadata
                        .bindings
                        .iter()
                        .filter(|b| b.field == field)
                        .cloned()
                        .collect::<Vec<_>>()
                );
                assert!(part.part.metadata.read_result.is_none());
                assert!(part.part.metadata.user_text_history.is_none());
            }
        }
        let context_selection = JevInputSelection::from_fields(&[
            JevInputField::UserMessage,
            JevInputField::BashCommandInput,
            JevInputField::BashCommandOutput,
        ]);
        let page = read_scope(&store, &key, context_selection, None)
            .unwrap()
            .unwrap();
        let prepared = antiburn_local::analysis::jev_evidence::prepare_session_content(
            session_id,
            format,
            page.content,
            vec![],
        );
        assert_eq!(
            prepared
                .actions
                .iter()
                .filter(|action| action.metadata.human_text.is_some())
                .count(),
            if agent == "codex" { 1 } else { 2 }
        );
        assert_eq!(
            prepared
                .actions
                .iter()
                .filter(|action| action.metadata.command_result.is_some())
                .count(),
            usize::from(agent != "pi")
        );
        assert!(
            prepared
                .actions
                .iter()
                .all(|action| action.metadata.selected_skill.is_none())
        );
        let input_only = JevInputSelection::from_fields(&[
            JevInputField::AssistantMessage,
            JevInputField::BashCommandInput,
            JevInputField::FileEditPath,
            JevInputField::ReadFilePath,
            JevInputField::SearchFilesQuery,
            JevInputField::OtherToolInput,
        ]);
        let page = read_scope(&store, &key, input_only, None).unwrap().unwrap();
        assert!(!page.content.parts.is_empty());
        assert!(
            page.content
                .parts
                .iter()
                .all(|p| p.part.kind != ContentKind::ToolResult
                    && p.part.kind != ContentKind::UserText
                    && p.part.metadata.read_request.is_none()
                    && p.part.metadata.read_result.is_none()
                    && p.part.metadata.user_text_history.is_none()
                    && p.part
                        .metadata
                        .bindings
                        .iter()
                        .all(|b| input_only.includes(b.field))),
            "{agent}"
        );
    }
}

#[test]
fn metadata_only_native_read_request_and_result_use_selected_byte_limits() {
    use antiburn_local::analysis::jev_evidence::{JevReadRequest, JevReadResult};
    let (store, key) = fixture();
    store.lock().execute("DELETE FROM turn", []).unwrap();
    let mut request = ContentPart::new(ContentKind::ToolInput, "")
        .with_tool_identity(Some("read".into()), Some("read-1".into()));
    request.normalized_fields = None;
    request.metadata.read_request = Some(JevReadRequest {
        reference_id: "request".into(),
        paths: vec!["src/é.rs".into()],
        cwd: None,
        extent: Default::default(),
        native_extent: BTreeMap::new(),
        truncated: false,
        extent_contract: None,
    });
    let mut result = ContentPart::new(ContentKind::ToolResult, "")
        .with_tool_identity(Some("read".into()), Some("read-1".into()));
    result.metadata.read_result = Some(JevReadResult {
        reference_id: "result".into(),
        request_reference_id: Some("request".into()),
        status: Default::default(),
        kind: Default::default(),
        recorded_output_bytes: 0,
        recorded_output_digest: String::new(),
        returned_extent: None,
        truncated: false,
        recorded_file_version: None,
        extent_contract: None,
    });
    write_parts(
        &store,
        &key,
        11,
        "read",
        vec![request.clone(), result.clone()],
    );
    let selection = JevInputSelection::from_fields(&[
        JevInputField::ReadFileRequest,
        JevInputField::ReadFileResult,
    ]);
    let page = read_scope(&store, &key, selection, None).unwrap().unwrap();
    assert_eq!(page.content.parts.len(), 2);
    assert_eq!(
        page.content.parts[0].part.metadata.read_request,
        request.metadata.read_request
    );
    assert_eq!(
        page.content.parts[1].part.metadata.read_result,
        result.metadata.read_result
    );
    store
        .lock()
        .execute(
            "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json,
        '$.metadata.read_result.recorded_output_digest', ?1) WHERE kind = 'tool_result'",
            ["é".repeat(MAX_CONTENT_PART_BYTES / 2 + 1)],
        )
        .unwrap();
    let oversized = read_scope(&store, &key, selection, None).unwrap().unwrap();
    assert_eq!(oversized.content.coverage.oversized_parts, 1);
    assert_eq!(oversized.content.parts.len(), 1);
    let output_only = read_scope(
        &store,
        &key,
        JevInputSelection::from_fields(&[JevInputField::ReadFileOutput]),
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(output_only.content.coverage.oversized_parts, 0);
    assert_eq!(output_only.content.parts.len(), 1);
    assert!(
        output_only.content.parts[0]
            .part
            .metadata
            .read_result
            .is_none()
    );
    store
        .lock()
        .execute(
            "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json,
        '$.metadata.bindings', json_array(json_object('field', 'read_file_output',
        'container', 'record', 'pointer', ?1, 'start', 0, 'end', 0))) WHERE kind = 'tool_result'",
            ["é".repeat(MAX_CONTENT_PART_BYTES / 2 + 1)],
        )
        .unwrap();
    let output_only = read_scope(
        &store,
        &key,
        JevInputSelection::from_fields(&[JevInputField::ReadFileOutput]),
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(output_only.content.coverage.oversized_parts, 1);
    assert!(output_only.content.parts.is_empty());
}

#[test]
fn selected_skill_only_metadata_reloads_with_exact_ranges_and_explicit_user_selection() {
    use antiburn_local::analysis::jev_evidence::{
        JevSelectedSkillNormalization, JevSelectedSkillProducer, JevSelectedSkillProof,
        JevSelectedSkillStatus,
    };
    let directory = tempfile::tempdir().unwrap();
    let (store, key) = fixture_in(Store::open(directory.path()).unwrap());
    store.lock().execute("DELETE FROM turn", []).unwrap();
    let proof = JevSelectedSkillProof {
        source_format: SourceFormat::PiV3Jsonl,
        session_id: "keyset".into(),
        message_id: "selected-skill".into(),
        name: "boundary-review".into(),
        location: "/synthetic/skills/boundary-review/SKILL.md".into(),
        producer: JevSelectedSkillProducer::PiSkillWrapper,
        normalization: JevSelectedSkillNormalization::PiSkillWrapper,
        normalization_revision: 1,
        ranges: vec![JevNativeFieldRange {
            native_record_id: Some("selected-skill".into()),
            field: JevInputField::UserMessage,
            container: JevNativeFieldContainer::Record,
            pointer: "/message/content/0/text".into(),
            start: 7,
            end: 39,
        }],
        status: JevSelectedSkillStatus::DocumentSelected,
        complete: true,
    };
    assert!(proof.is_bounded());
    let mut part = ContentPart::new(ContentKind::AssistantText, "excluded assistant body");
    part.metadata.selected_skill = Some(proof.clone());
    assert!(part.normalized_fields.is_none());
    assert!(part.metadata.bindings.is_empty());
    let mut thinking = ContentPart::new(ContentKind::Thinking, "excluded thinking body");
    thinking.metadata.selected_skill = Some(proof.clone());
    write_parts(&store, &key, 11, "selected-skill", vec![part, thinking]);
    let user = JevInputSelection::from_fields(&[JevInputField::UserMessage]);
    let selected = read_scope(&store, &key, user, None).unwrap().unwrap();
    assert_eq!(selected.content.parts.len(), 1);
    assert!(selected.content.parts[0].part.text.is_empty());
    assert_eq!(
        selected.content.parts[0]
            .part
            .metadata
            .selected_skill
            .as_ref(),
        Some(&proof)
    );
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(
        read_scope(&store, &key, user, None).unwrap().unwrap(),
        selected
    );
    let assistant = JevInputSelection::from_fields(&[JevInputField::AssistantMessage]);
    let excluded = read_scope(&store, &key, assistant, None).unwrap().unwrap();
    assert_eq!(excluded.content.parts.len(), 1);
    assert_eq!(
        excluded.content.parts[0].part.text,
        "excluded assistant body"
    );
    assert!(
        excluded.content.parts[0]
            .part
            .metadata
            .selected_skill
            .is_none()
    );
    for pointer in [
        "$.metadata.selected_skill.location",
        "$.metadata.selected_skill.ranges[0].pointer",
    ] {
        store.lock().execute(
            "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json, ?1, ?2)",
            params![pointer, "é".repeat(MAX_CONTENT_PART_BYTES / 2 + 1)],
        ).unwrap();
        let oversized = read_scope(&store, &key, user, None).unwrap().unwrap();
        assert_eq!(oversized.content.coverage.oversized_parts, 1);
        assert!(oversized.content.parts.is_empty());
        assert_eq!(
            read_scope(&store, &key, assistant, None).unwrap().unwrap(),
            excluded
        );
        store
            .lock()
            .execute(
                "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json,
            '$.metadata.selected_skill', json(?1))",
                [serde_json::to_string(&proof).unwrap()],
            )
            .unwrap();
    }
}

#[test]
fn codex_environment_context_survives_published_reload_and_shared_preparation() {
    use antiburn_local::analysis::jev_evidence::prepare_session_content;
    let directory = tempfile::tempdir().unwrap();
    let (store, key) = fixture_in(Store::open(directory.path()).unwrap());
    store.lock().execute("DELETE FROM turn", []).unwrap();
    let source = SourceFormat::CodexRolloutJsonl;
    let session_id = "environment-root";
    let parsed = store_native_fixture(
        &store,
        &key,
        "codex",
        session_id,
        source,
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../crates/antiburn-local/tests/fixtures/codex_characterization/environment_context.jsonl"
        )),
    );
    let proof = parsed
        .iter()
        .find_map(|part| part.metadata.non_authorizing_context_proof.as_ref())
        .unwrap()
        .clone();
    let user = JevInputSelection::from_fields(&[JevInputField::UserMessage]);
    let before = read_scope(&store, &key, user, None).unwrap().unwrap();
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    let reloaded = read_scope(&store, &key, user, None).unwrap().unwrap();
    assert_eq!(reloaded, before);
    assert_eq!(
        reloaded.content.parts.len(),
        2,
        "parsed: {parsed:?}; queried: {reloaded:?}"
    );
    assert!(
        reloaded
            .content
            .parts
            .iter()
            .all(|part| part.part.metadata.human_text.is_none()
                && part.part.metadata.command_result.is_none())
    );
    assert_eq!(
        reloaded.content.parts.iter().find_map(|part| part
            .part
            .metadata
            .non_authorizing_context_proof
            .as_ref()),
        Some(&proof)
    );
    let prepared = prepare_session_content(session_id, source, reloaded.content, vec![]);
    let environment = prepared
        .actions
        .iter()
        .find(|action| action.metadata.non_authorizing_context.is_some())
        .unwrap();
    let context = environment
        .metadata
        .non_authorizing_context
        .as_ref()
        .unwrap();
    assert!(context.matches_session(environment, source, session_id));
    assert_eq!(environment.authority, "unknown");
    assert!(environment.metadata.human_text.is_none());
    assert_eq!(
        prepared
            .actions
            .iter()
            .filter(|action| action.metadata.human_text.is_some())
            .count(),
        1
    );
    store.lock().execute(
        "UPDATE turn_content SET normalized_fields_json = json_remove(json_set(normalized_fields_json,
        '$.metadata.non_authorizing_context', json(?1)), '$.metadata.non_authorizing_context_proof')
        WHERE json_type(normalized_fields_json, '$.metadata.non_authorizing_context_proof') = 'object'",
        [serde_json::to_string(context).unwrap()],
    ).unwrap();
    let reloaded = read_scope(&store, &key, user, None).unwrap().unwrap();
    assert_eq!(
        reloaded.content.parts.iter().find_map(|part| part
            .part
            .metadata
            .non_authorizing_context
            .as_ref()),
        Some(context)
    );
    assert!(
        reloaded.content.parts.iter().all(|part| part
            .part
            .metadata
            .non_authorizing_context_proof
            .is_none())
    );
    let prepared = prepare_session_content(session_id, source, reloaded.content, vec![]);
    let environment = prepared
        .actions
        .iter()
        .find(|action| action.metadata.non_authorizing_context.is_some())
        .unwrap();
    assert!(
        environment
            .metadata
            .non_authorizing_context
            .as_ref()
            .unwrap()
            .matches_session(environment, source, session_id)
    );
    store
        .lock()
        .execute(
            "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json,
        '$.metadata.non_authorizing_context.range.pointer', ?1)
        WHERE json_type(normalized_fields_json, '$.metadata.non_authorizing_context') = 'object'",
            ["é".repeat(MAX_CONTENT_PART_BYTES / 2 + 1)],
        )
        .unwrap();
    let oversized = read_scope(&store, &key, user, None).unwrap().unwrap();
    assert_eq!(oversized.content.coverage.oversized_parts, 1);
    assert_eq!(oversized.content.parts.len(), 1);
    let excluded = read_scope(
        &store,
        &key,
        JevInputSelection::from_fields(&[JevInputField::AssistantMessage]),
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(excluded.content.coverage.oversized_parts, 0);
    assert!(excluded.content.parts.is_empty());
    let mut proof_only = ContentPart::new(ContentKind::AssistantText, "");
    proof_only.metadata.non_authorizing_context_proof = Some(proof.clone());
    let mut context_only = ContentPart::new(ContentKind::AssistantText, "");
    context_only.metadata.non_authorizing_context = Some(context.clone());
    write_parts(
        &store,
        &key,
        11,
        "z-metadata-only",
        vec![proof_only, context_only],
    );
    let page = read_scope(&store, &key, user, None).unwrap().unwrap();
    let metadata_only = page
        .content
        .parts
        .iter()
        .filter(|part| part.source_key == "z-metadata-only")
        .collect::<Vec<_>>();
    assert_eq!(metadata_only.len(), 2);
    assert!(
        metadata_only
            .iter()
            .all(|part| part.part.text.is_empty() && part.part.metadata.bindings.is_empty())
    );
    assert_eq!(
        metadata_only[0]
            .part
            .metadata
            .non_authorizing_context_proof
            .as_ref(),
        Some(&proof)
    );
    assert_eq!(
        metadata_only[1]
            .part
            .metadata
            .non_authorizing_context
            .as_ref(),
        Some(context)
    );
}
