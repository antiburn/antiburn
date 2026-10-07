use std::sync::Arc;

use antiburn_local::analysis::jev_evidence::{
    SessionContentEvidence, normalize_recorded_skill, prepare_session_content,
    select_session_content,
};
use antiburn_local::analysis::*;
use antiburn_local::checks::skill_opportunities::*;
use antiburn_local::model::AgentKind;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

const CLAUDE: &str = include_str!("fixtures/skill_use_characterization/claude.jsonl");
const OPENCODE: &str = include_str!("fixtures/skill_use_characterization/opencode.jsonl");
const PI: &str = include_str!("fixtures/skill_use_characterization/pi.jsonl");
const CODEX: &str = include_str!("fixtures/skill_use_characterization/codex.jsonl");

fn store(input: &SessionInput) -> Arc<MemoryTurnRowStore> {
    let reader = reader_for(&input.agent);
    let store = MemoryTurnRowStore::new(&input.agent, &input.session_id);
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new(&input.agent, &input.session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            kind: SourceKind::from(&input.source),
            capabilities: reader.capabilities(input),
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            &input.session_id,
            None,
        ),
    );
    let outcome = reader.visit(input, &mut sink).unwrap();
    sink.observe_source_outcome(outcome);
    store
}

fn prepared(
    store: &MemoryTurnRowStore,
    agent: &str,
    format: SourceFormat,
    session: &str,
    fence: i64,
) -> SessionContentEvidence {
    store.with_connection(|connection| {
        let content = query_turn_content_offset_selected(
            connection,
            &TurnSessionKey {
                environment_key: "native",
                agent,
                session_id: session,
            },
            &FenceScope::single(fence),
            None,
            &Default::default(),
            0,
            SKILL_USE_SELECTION,
        )
        .unwrap();
        prepare_session_content("session-digest", format, content, vec![])
    })
}

fn selected(
    store: &MemoryTurnRowStore,
    agent: &str,
    format: SourceFormat,
    session: &str,
    fence: i64,
) -> SessionContentEvidence {
    let prepared = prepared(store, agent, format, session, fence);
    select_session_content(&prepared, SKILL_USE_SELECTION)
}

fn boundary(content: &SessionContentEvidence, agent: AgentKind) -> SkillUseBoundary {
    SkillUseBoundary {
        session_identity: content.session_identity_digest.clone(),
        native_session_id: "root".into(),
        publication_fence: content.publication_fence,
        scope: SkillScope {
            agent,
            project_identity: Some("project-digest".into()),
            environment_identity: "native-home-digest".into(),
        },
    }
}

fn parsed_records(text: &str) -> Vec<Value> {
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn native_bindings<'a>(
    content: &'a SessionContentEvidence,
    records: &'a [Value],
) -> Vec<ParsedSkillRecord<'a>> {
    content
        .actions
        .iter()
        .filter(|action| action.kind == "tool_result")
        .filter_map(|action| {
            let record = records.iter().find(|record| {
                record
                    .get("uuid")
                    .or_else(|| record.get("messageID"))
                    .or_else(|| record.get("id"))
                    .and_then(Value::as_str)
                    == action.reference.native_record_id.as_deref()
            })?;
            Some(ParsedSkillRecord {
                session_identity: &content.session_identity_digest,
                publication_fence: content.publication_fence,
                reference: &action.reference,
                record,
            })
        })
        .collect()
}

fn jsonl_content(
    agent: &str,
    format: SourceFormat,
    text: &str,
) -> (Arc<MemoryTurnRowStore>, SessionContentEvidence) {
    let input = SessionInput {
        agent: agent.into(),
        session_id: "root".into(),
        source: RawSource::Jsonl(text.into()),
        source_format: format,
        fork_parent_session_id: None,
    };
    let store = store(&input);
    let content = selected(&store, agent, format, "root", 1);
    (store, content)
}

fn opencode_content(
    part: &Value,
) -> (
    tempfile::TempDir,
    Arc<MemoryTurnRowStore>,
    SessionContentEvidence,
) {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, title TEXT, time_created INTEGER, time_updated INTEGER);
        CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
        CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
        INSERT INTO session VALUES ('root', NULL, NULL, 1767225601000, 1767225602000);").unwrap();
    connection.execute("INSERT INTO message VALUES ('skill-message', 'root', 1767225601000, 1767225602000, ?1)",
        [json!({"role":"assistant", "modelID":"synthetic-model", "tokens":{"input":1,"output":1}}).to_string()]).unwrap();
    connection.execute("INSERT INTO part VALUES ('skill-part', 'skill-message', 'root', 1767225601000, 1767225602000, ?1)",
        params![part.to_string()]).unwrap();
    let input = SessionInput {
        agent: "opencode".into(),
        session_id: "root".into(),
        source: RawSource::Sqlite(path),
        source_format: SourceFormat::OpenCodeSqliteV2,
        fork_parent_session_id: None,
    };
    let store = store(&input);
    let content = selected(
        &store,
        "opencode",
        SourceFormat::OpenCodeSqliteV2,
        "root",
        1,
    );
    (temporary, store, content)
}

#[test]
fn claude_exact_request_survives_publication_without_claiming_launch_success() {
    let (_, content) = jsonl_content("claude", SourceFormat::ClaudeJsonl, CLAUDE);
    let snapshot =
        SkillUseSnapshot::from_published_content(&content, &boundary(&content, AgentKind::Claude))
            .unwrap();
    assert_eq!(snapshot.events().len(), 2, "{content:?}");
    let request = &snapshot.events()[0];
    let result = &snapshot.events()[1];
    assert_eq!(
        request.skill,
        RecordedSkillIdentity::Name {
            name: "review".into()
        }
    );
    assert_eq!(request.lifecycle, SkillUseLifecycle::Requested);
    assert_eq!(result.lifecycle, SkillUseLifecycle::Unknown, "{content:?}");
    assert_eq!(
        request.reference.native_record_id.as_deref(),
        Some("claude-request")
    );
    assert_eq!(
        result.reference.native_record_id.as_deref(),
        Some("claude-result")
    );
    assert_eq!(result.request_reference.as_ref(), Some(&request.reference));
    assert_eq!(request.tool_call_id.as_deref(), Some("skill-call"));
    assert_eq!(request.timestamp_ms, Some(1767225601000));
    assert_eq!(result.timestamp_ms, Some(1767225602000));
    assert_eq!(request.turn_role, "assistant");
    assert_eq!(result.authority, "tool");
    assert!(request.reference.stable && result.reference.stable);
    assert_eq!(snapshot.coverage().ordering, SkillUseOrdering::Monotonic);
    assert!(!snapshot.proves_session_wide_absence());
}

#[test]
fn opencode_selected_skill_requires_native_completed_metadata_and_full_document() {
    let native = parsed_records(OPENCODE);
    let (_temporary, _store, content) = opencode_content(&native[0]);
    let snapshot = SkillUseSnapshot::from_published_content(
        &content,
        &boundary(&content, AgentKind::OpenCode),
    )
    .unwrap();
    let proof = content.actions[1]
        .metadata
        .recorded_skill_result
        .as_ref()
        .expect("selected SQLite result retains its typed proof");
    assert_eq!(proof.source_format, SourceFormat::OpenCodeSqliteV2);
    assert_eq!(proof.session_id.as_deref(), Some("root"));
    assert_eq!(proof.message_id.as_deref(), Some("skill-message"));
    assert_eq!(proof.call_id.as_deref(), Some("skill-call"));
    assert_eq!(proof.name.as_deref(), Some("review"));
    assert_eq!(
        proof.location.as_deref(),
        Some("/synthetic/skills/review/SKILL.md")
    );
    assert!(proof.complete && !proof.truncated);
    let serialized = serde_json::to_string(proof).unwrap();
    assert!(!serialized.contains("skill_content"));
    assert!(!serialized.contains("producer"));
    assert!(!serialized.contains("\"input\""));
    assert!(!serialized.contains("\"output\""));
    assert_eq!(snapshot.events().len(), 2, "{content:?}");
    assert_eq!(snapshot.events()[0].lifecycle, SkillUseLifecycle::Requested);
    assert_eq!(
        snapshot.events()[1].lifecycle,
        SkillUseLifecycle::DocumentSelected,
        "{content:?}"
    );
    assert_eq!(
        snapshot.events()[0].reference.native_record_id.as_deref(),
        Some("skill-message")
    );
    assert_eq!(snapshot.events()[0].timestamp_ms, Some(1767225601000));
    assert_eq!(
        snapshot.events()[1].request_reference.as_ref(),
        Some(&snapshot.events()[0].reference)
    );
    assert_eq!(
        snapshot.events()[1].producer,
        SkillUseProducer::OpenCodeSkill77239205
    );
    assert!(
        !serde_json::to_string(snapshot.events())
            .unwrap()
            .contains("/synthetic/")
    );
}

#[test]
fn skill_consumer_uses_normalized_facts_without_native_tool_names() {
    let native = parsed_records(OPENCODE);
    let (_temporary, _store, mut content) = opencode_content(&native[0]);
    for action in &mut content.actions {
        action.tool_name = Some("normalized_skill_loader".into());
    }
    let usage = SkillUseSnapshot::from_published_content(
        &content,
        &boundary(&content, AgentKind::OpenCode),
    )
    .unwrap();
    assert_eq!(usage.events().len(), 2);
    assert_eq!(usage.events()[0].lifecycle, SkillUseLifecycle::Requested);
    assert_eq!(
        usage.events()[1].lifecycle,
        SkillUseLifecycle::DocumentSelected
    );
    assert_eq!(matching_candidates(&content, AgentKind::OpenCode), 0);
}

#[test]
fn opencode_jsonl_does_not_receive_sqlite_skill_result_support() {
    let (_, content) = jsonl_content("opencode", SourceFormat::OpenCodeJsonl, OPENCODE);
    let snapshot = SkillUseSnapshot::from_published_content(
        &content,
        &boundary(&content, AgentKind::OpenCode),
    )
    .unwrap();
    assert_eq!(snapshot.coverage().status, SkillUseStatus::Unsupported);
    assert!(snapshot.events().is_empty());
}

#[test]
fn pi_explicit_requests_preserve_identity_without_inventing_extension_success() {
    let (_, content) = jsonl_content("pi", SourceFormat::PiV3Jsonl, PI);
    let native = parsed_records(PI);
    let snapshot = SkillUseSnapshot::from_selected_content(
        &content,
        &boundary(&content, AgentKind::Pi),
        &native_bindings(&content, &native),
    )
    .unwrap();
    assert_eq!(snapshot.events().len(), 2);
    assert_eq!(snapshot.events()[0].lifecycle, SkillUseLifecycle::Requested);
    assert_eq!(snapshot.events()[1].lifecycle, SkillUseLifecycle::Unknown);
    assert_eq!(
        snapshot.events()[0].reference.native_record_id.as_deref(),
        Some("pi-request")
    );
    assert_eq!(snapshot.events()[0].timestamp_ms, Some(1767225601000));
    assert_eq!(snapshot.coverage().status, SkillUseStatus::Partial);
}

#[test]
fn codex_full_documents_are_selection_not_task_success_or_mentions() {
    let records = codex_skill_records();
    let (_, content) = jsonl_content("codex", SourceFormat::CodexRolloutJsonl, &jsonl(&records));
    let snapshot = SkillUseSnapshot::from_selected_content(
        &content,
        &boundary(&content, AgentKind::Codex),
        &[],
    )
    .unwrap();
    assert_eq!(snapshot.events().len(), 1, "{content:?}");
    let event = &snapshot.events()[0];
    assert_eq!(event.lifecycle, SkillUseLifecycle::DocumentSelected);
    assert_eq!(event.timestamp_ms, Some(1767225601000));
    assert!(
        matches!(&event.skill, RecordedSkillIdentity::Document { name, path_digest } if name == "review" && path_digest.len() == 64)
    );
    assert_eq!(event.tool_call_id, None);
    assert!(!snapshot.proves_session_wide_absence());
}

fn jsonl(records: &[Value]) -> String {
    records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn claude_snapshot(records: &[Value], native: bool) -> SkillUseSnapshot {
    let (_, content) = jsonl_content("claude", SourceFormat::ClaudeJsonl, &jsonl(records));
    let bindings = if native {
        native_bindings(&content, records)
    } else {
        vec![]
    };
    SkillUseSnapshot::from_selected_content(
        &content,
        &boundary(&content, AgentKind::Claude),
        &bindings,
    )
    .unwrap()
}

#[test]
fn claude_text_or_missing_conflicting_and_batched_native_results_do_not_prove_launch() {
    for case in 0..8 {
        let mut records = parsed_records(CLAUDE);
        match case {
            0 => {
                records[1].as_object_mut().unwrap().remove("toolUseResult");
            }
            1 => records[1]["toolUseResult"]["success"] = json!(false),
            2 => records[1]["toolUseResult"]["commandName"] = json!("other"),
            3 => records[1]["message"]["content"][0]["is_error"] = json!(true),
            4 => records[1]["sourceToolAssistantUUID"] = json!("unrelated-record"),
            5 => records[1]["sourceToolUseID"] = json!("unrelated-call"),
            6 => records[1]["message"]["content"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"tool_result","tool_use_id":"other-call","content":"ok"})),
            _ => {
                records[0]["message"]["content"][0]["input"] =
                    json!({"command":"/review extra-args"})
            }
        }
        let snapshot = claude_snapshot(&records, true);
        assert!(
            snapshot
                .events()
                .iter()
                .all(|event| event.lifecycle != SkillUseLifecycle::Succeeded),
            "case {case}"
        );
        assert_eq!(
            snapshot.coverage().status,
            SkillUseStatus::Partial,
            "case {case}"
        );
    }
    let snapshot = claude_snapshot(&parsed_records(CLAUDE), false);
    assert_eq!(snapshot.events()[1].lifecycle, SkillUseLifecycle::Unknown);
    assert!(
        snapshot
            .coverage()
            .limitations
            .contains(&SkillUseLimit::NativeMetadataUnavailable)
    );
}

#[test]
fn matched_explicit_native_failure_stays_failed() {
    let mut records = parsed_records(CLAUDE);
    records[0]["version"] = json!("2.1.278");
    records[1]["version"] = json!("2.1.278");
    records[1]["toolUseResult"]["success"] = json!(false);
    records[1]["message"]["content"][0]["is_error"] = json!(true);
    records[1]["message"]["content"][0]["content"] = json!("Skill permission denied.");
    assert_eq!(
        claude_snapshot(&records, false).events()[1].lifecycle,
        SkillUseLifecycle::Failed
    );
    let (_, published) = jsonl_content("claude", SourceFormat::ClaudeJsonl, &jsonl(&records));
    assert_eq!(
        published.actions[1].metadata.state,
        antiburn_local::analysis::jev_evidence::JevOperationState::Error
    );
    for case in 0..3 {
        let mut content = published.clone();
        match case {
            0 => content.actions[1].metadata.bindings.clear(),
            1 => content.actions[1].metadata.bindings[0].end -= 1,
            _ => content.actions[1].truncated = true,
        }
        let snapshot = SkillUseSnapshot::from_published_content(
            &content,
            &boundary(&content, AgentKind::Claude),
        )
        .unwrap();
        assert!(
            snapshot
                .events()
                .iter()
                .all(|event| event.lifecycle != SkillUseLifecycle::Failed)
        );
    }
}

#[test]
fn duplicate_call_id_cannot_bind_a_result_to_one_request() {
    let mut records = parsed_records(CLAUDE);
    let duplicate = records[0]["message"]["content"][0].clone();
    records[0]["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    let snapshot = claude_snapshot(&records, true);
    assert!(
        snapshot
            .events()
            .iter()
            .all(|event| event.lifecycle == SkillUseLifecycle::Requested)
    );
    assert!(
        snapshot
            .coverage()
            .limitations
            .contains(&SkillUseLimit::AmbiguousCallIdentity)
    );
}

#[test]
fn opencode_mutable_lifecycle_metadata_aliases_and_partial_outputs_stay_unknown() {
    for case in 0..8 {
        let mut records = parsed_records(OPENCODE);
        match case {
            0 => {
                records[0]["state"]["metadata"]
                    .as_object_mut()
                    .unwrap()
                    .remove("name");
            }
            1 => records[0]["state"]["status"] = json!("running"),
            2 => records[0]["state"]["metadata"]["truncated"] = json!(true),
            3 => records[0]["state"]["time"]["compacted"] = json!(1767225602000i64),
            4 => records[0]["metadata"] = json!({"providerExecuted":true}),
            5 => records[0]["state"]["output"] = json!("Loaded review successfully."),
            6 => records[0]["state"]["input"]["name"] = json!("alias-review"),
            _ => records[0]["state"]["metadata"]["dir"] = json!("/synthetic/other"),
        }
        let (_temporary, store, content) = opencode_content(&records[0]);
        store.with_connection(|connection| {
            let accepted: i64 = connection.query_row(
                "SELECT count(*) FROM turn_content WHERE json_extract(normalized_fields_json, '$.metadata.recorded_skill_result.status') = 'document_selected'",
                [], |row| row.get(0)).unwrap();
            assert_eq!(accepted, 0, "adapter rejects case {case} before checks");
        });
        let snapshot = SkillUseSnapshot::from_selected_content(
            &content,
            &boundary(&content, AgentKind::OpenCode),
            &native_bindings(&content, &records),
        )
        .unwrap();
        assert!(
            snapshot.events().iter().all(|event| !matches!(
                event.lifecycle,
                SkillUseLifecycle::Succeeded | SkillUseLifecycle::DocumentSelected
            )),
            "case {case}"
        );
        assert_eq!(
            snapshot.coverage().status,
            SkillUseStatus::Partial,
            "case {case}"
        );
    }
}

#[test]
fn source_thread_roles_and_native_call_ids_constrain_result_joins() {
    let (_, original) = jsonl_content("claude", SourceFormat::ClaudeJsonl, CLAUDE);
    let records = parsed_records(CLAUDE);
    for case in 0..7 {
        let mut content = original.clone();
        match case {
            0 => content.actions[1].reference.source_key_digest = "different-source".into(),
            1 => content.actions[1].reference.thread_digest = "different-thread".into(),
            2 => content.actions[1].turn_scope = "delegated".into(),
            3 => content.actions[1].authority = "user".into(),
            4 => content.actions[1].tool_call_id = Some("different-call".into()),
            5 => content.actions[0].turn_role = "user".into(),
            _ => content.actions[1].turn_role = "user".into(),
        }
        let snapshot = SkillUseSnapshot::from_selected_content(
            &content,
            &boundary(&content, AgentKind::Claude),
            &native_bindings(&content, &records),
        )
        .unwrap();
        assert!(
            snapshot
                .events()
                .iter()
                .all(|event| event.lifecycle != SkillUseLifecycle::Succeeded),
            "case {case}"
        );
    }
}

#[test]
fn timestamps_and_native_positions_remain_distinct_when_time_is_missing_or_reversed() {
    let mut records = parsed_records(CLAUDE);
    records[0].as_object_mut().unwrap().remove("timestamp");
    let missing = claude_snapshot(&records, true);
    assert_eq!(missing.events()[0].timestamp_ms, None);
    assert_eq!(missing.events()[0].reference.turn_index, 0);
    assert_eq!(missing.coverage().ordering, SkillUseOrdering::Unknown);
    assert!(
        missing
            .coverage()
            .limitations
            .contains(&SkillUseLimit::TimestampUnavailable)
    );
    let mut records = parsed_records(CLAUDE);
    records[1]["timestamp"] = json!("2026-01-01T00:00:00Z");
    let reversed = claude_snapshot(&records, true);
    assert_eq!(reversed.coverage().ordering, SkillUseOrdering::OutOfOrder);
    assert!(reversed.events()[0].reference.turn_index < reversed.events()[1].reference.turn_index);
}

#[test]
fn publication_and_session_fences_bind_selected_and_supplemental_native_evidence() {
    let (store, content) = jsonl_content("claude", SourceFormat::ClaudeJsonl, CLAUDE);
    let records = parsed_records(CLAUDE);
    let bound = boundary(&content, AgentKind::Claude);
    let snapshot = SkillUseSnapshot::from_selected_content(
        &content,
        &bound,
        &native_bindings(&content, &records),
    )
    .unwrap();
    assert!(snapshot.matches_boundary(&bound));
    let mut stale = bound.clone();
    stale.publication_fence += 1;
    assert!(!snapshot.matches_boundary(&stale));
    assert_eq!(
        SkillUseSnapshot::from_selected_content(&content, &stale, &[]),
        Err(SkillInputError::WrongSessionOrScope)
    );
    let mut other_session = bound.clone();
    other_session.session_identity = "different-session".into();
    assert_eq!(
        SkillUseSnapshot::from_selected_content(&content, &other_session, &[]),
        Err(SkillInputError::WrongSessionOrScope)
    );
    let mut native = native_bindings(&content, &records);
    native[0].publication_fence += 1;
    assert_eq!(
        SkillUseSnapshot::from_selected_content(&content, &bound, &native),
        Err(SkillInputError::WrongSessionOrScope)
    );
    native[0].publication_fence = content.publication_fence;
    native[0].session_identity = "other-session";
    assert_eq!(
        SkillUseSnapshot::from_selected_content(&content, &bound, &native),
        Err(SkillInputError::WrongSessionOrScope)
    );
    assert!(
        selected(
            &store,
            "claude",
            SourceFormat::ClaudeJsonl,
            "other-session",
            1
        )
        .actions
        .is_empty()
    );
    assert!(
        selected(&store, "claude", SourceFormat::ClaudeJsonl, "root", 2)
            .actions
            .is_empty()
    );
}

#[test]
fn unknown_field_selection_source_and_part_caps_remain_explicit() {
    let (_, original) = jsonl_content("claude", SourceFormat::ClaudeJsonl, CLAUDE);
    let mut content = original.clone();
    content.field_availability.clear();
    let snapshot = SkillUseSnapshot::from_selected_content(
        &content,
        &boundary(&content, AgentKind::Claude),
        &[],
    )
    .unwrap();
    assert!(
        snapshot
            .coverage()
            .limitations
            .contains(&SkillUseLimit::RequiredFieldNotSelected)
    );
    let mut unsupported = original.clone();
    unsupported.source_format = SourceFormat::OpenCodeJsonl;
    let snapshot = SkillUseSnapshot::from_selected_content(
        &unsupported,
        &boundary(&unsupported, AgentKind::OpenCode),
        &[],
    )
    .unwrap();
    assert_eq!(snapshot.coverage().status, SkillUseStatus::Unsupported);
    assert!(snapshot.events().is_empty());
    content = original.clone();
    content.actions = vec![original.actions[0].clone(); 289];
    assert_eq!(
        SkillUseSnapshot::from_selected_content(
            &content,
            &boundary(&content, AgentKind::Claude),
            &[]
        ),
        Err(SkillInputError::LimitExceeded)
    );
    let mut records = parsed_records(CLAUDE);
    records[1]["unused"] = json!("x".repeat(256 * 1024));
    assert_eq!(
        SkillUseSnapshot::from_selected_content(
            &original,
            &boundary(&original, AgentKind::Claude),
            &native_bindings(&original, &records)
        ),
        Err(SkillInputError::LimitExceeded)
    );
}

#[test]
fn aggregates_stay_inferred_without_success_timing_or_absence_proof() {
    let scope = SkillScope {
        agent: AgentKind::Claude,
        project_identity: None,
        environment_identity: "native".into(),
    };
    let aggregate = SkillUseEvidence {
        session_identity: "session".into(),
        scope,
        status: SkillUseStatus::Complete,
        ordering: SkillUseOrdering::Monotonic,
        events: vec![SkillUseEvent {
            identity: "review".into(),
            identity_kind: SkillUseIdentity::Exact,
            lifecycle: SkillUseLifecycle::Succeeded,
            source_identity: "session".into(),
            source_field: "skill_uses".into(),
            timestamp_ms: Some(100),
            order: Some(1),
        }],
    };
    let snapshot = SkillUseSnapshot::from_aggregates(aggregate).unwrap();
    assert_eq!(snapshot.coverage().status, SkillUseStatus::Partial);
    assert_eq!(
        snapshot.evidence().events[0].identity_kind,
        SkillUseIdentity::Inferred
    );
    assert_eq!(
        snapshot.evidence().events[0].lifecycle,
        SkillUseLifecycle::Requested
    );
    assert_eq!(snapshot.evidence().events[0].timestamp_ms, None);
    assert_eq!(snapshot.evidence().events[0].order, None);
    assert!(snapshot.events().is_empty());
    assert!(!snapshot.proves_session_wide_absence());
}

#[test]
fn current_inventory_binding_stays_inferred_and_candidates_keep_time_and_use_limits() {
    let (_, content) = jsonl_content("claude", SourceFormat::ClaudeJsonl, CLAUDE);
    let bound = boundary(&content, AgentKind::Claude);
    let usage = SkillUseSnapshot::from_selected_content(&content, &bound, &[]).unwrap();
    assert_eq!(
        usage.events()[0].skill,
        RecordedSkillIdentity::Name {
            name: "review".into()
        }
    );
    assert_eq!(
        usage.evidence().events[0].identity_kind,
        SkillUseIdentity::Inferred
    );
    let definition =
        |identity: &str, name: &str, aliases: Vec<String>, created_at_ms| SkillDefinition {
            identity: identity.into(),
            revision: "semantic-revision".into(),
            name: name.into(),
            aliases,
            description: "Review resource ownership.".into(),
            frontmatter: json!({"description":"Review resource ownership."}),
            scope: bound.scope.clone(),
            enabled: true,
            created_at_ms,
        };
    let inventory = SkillOpportunitySnapshot::new(
        bound.scope.clone(),
        vec![
            definition("used-file", "native-review", vec!["review".into()], None),
            definition("unused-file", "error-review", vec![], Some(1767225601500)),
            definition("unknown-birth", "test-review", vec![], None),
        ],
        true,
    )
    .unwrap();
    let mut work = SkillWorkContext {
        session_identity: bound.session_identity.clone(),
        scope: bound.scope.clone(),
        relevant_work_at_ms: Some(1767225601000),
    };
    let before_creation = inventory
        .eligible_candidates_with_recorded_use(&work, &usage)
        .unwrap();
    assert_eq!(before_creation.len(), 1);
    assert_eq!(before_creation[0].skill().identity, "unknown-birth");
    assert!(
        before_creation[0]
            .limitations()
            .contains(&SkillOpportunityLimit::CreationTimeUnknown)
    );
    assert!(
        before_creation[0]
            .limitations()
            .contains(&SkillOpportunityLimit::SelectedUseWindowOnly)
    );
    assert_eq!(
        before_creation[0].reference_snapshot().fields["use_snapshot_revision"],
        usage.revision()
    );
    work.relevant_work_at_ms = Some(1767225602000);
    assert_eq!(
        inventory
            .eligible_candidates_with_recorded_use(&work, &usage)
            .unwrap()
            .len(),
        2
    );
    work.scope.project_identity = Some("other-project".into());
    assert_eq!(
        inventory.eligible_candidates_with_recorded_use(&work, &usage),
        Err(SkillInputError::WrongSessionOrScope)
    );
}

#[test]
fn codex_empty_documents_and_quoted_or_wrong_role_shapes_are_not_use() {
    for text in [
        "<skill>\n<name>review</name>\n<path>/synthetic/SKILL.md</path>\n\n</skill>",
        "Quoted: <skill>\n<name>review</name>\n<path>/synthetic/SKILL.md</path>\nFull body.\n</skill>",
        "<skill>\n<name>review</name>\nListing only.\n</skill>",
    ] {
        let mut records = codex_skill_records();
        records[1]["payload"]["content"][0]["text"] = json!(text);
        let (_, content) =
            jsonl_content("codex", SourceFormat::CodexRolloutJsonl, &jsonl(&records));
        let snapshot = SkillUseSnapshot::from_selected_content(
            &content,
            &boundary(&content, AgentKind::Codex),
            &[],
        )
        .unwrap();
        assert!(snapshot.events().is_empty());
    }
    let mut records = codex_skill_records();
    records[1]["payload"]["role"] = json!("assistant");
    let (_, content) = jsonl_content("codex", SourceFormat::CodexRolloutJsonl, &jsonl(&records));
    assert!(
        SkillUseSnapshot::from_selected_content(
            &content,
            &boundary(&content, AgentKind::Codex),
            &[]
        )
        .unwrap()
        .events()
        .is_empty()
    );
}

#[test]
fn native_envelopes_from_other_sessions_or_with_wrong_record_ids_cannot_supply_success() {
    let (_, content) = jsonl_content("claude", SourceFormat::ClaudeJsonl, CLAUDE);
    for (field, value) in [("sessionId", "other-session"), ("uuid", "other-record")] {
        let mut records = parsed_records(CLAUDE);
        records[1][field] = json!(value);
        let native = vec![ParsedSkillRecord {
            session_identity: &content.session_identity_digest,
            publication_fence: content.publication_fence,
            reference: &content.actions[1].reference,
            record: &records[1],
        }];
        let snapshot = SkillUseSnapshot::from_selected_content(
            &content,
            &boundary(&content, AgentKind::Claude),
            &native,
        )
        .unwrap();
        assert_eq!(snapshot.events()[1].lifecycle, SkillUseLifecycle::Unknown);
    }
}

#[test]
fn compatibility_name_fields_and_paths_are_inferred_without_a_producer_success_contract() {
    for input in [
        json!({"name":"review"}),
        json!({"skill_name":"review"}),
        json!({"skillName":"review"}),
        json!({"command":"/review extra"}),
        json!({"path":"/synthetic/skills/review/SKILL.md"}),
    ] {
        let mut records = parsed_records(CLAUDE);
        records[0]["message"]["content"][0]["input"] = input;
        let snapshot = claude_snapshot(&records, true);
        assert_eq!(
            snapshot.events()[0].skill,
            RecordedSkillIdentity::InferredName {
                name: "review".into()
            }
        );
        assert_eq!(snapshot.events()[1].lifecycle, SkillUseLifecycle::Unknown);
        assert!(
            snapshot
                .coverage()
                .limitations
                .contains(&SkillUseLimit::UnknownSkillIdentity)
        );
    }
    let mut records = parsed_records(CLAUDE);
    records[0]["message"]["content"][0]["input"] = json!({"skill":"review","name":"different"});
    assert_eq!(
        claude_snapshot(&records, true).events()[0].skill,
        RecordedSkillIdentity::Unknown
    );
}

fn codex_skill_records() -> Vec<Value> {
    let mut records = parsed_records(CODEX);
    records[0]["payload"]["cli_version"] = json!("0.160.1");
    records[0]["payload"]["history_mode"] = json!("legacy");
    records[0]["payload"]["thread_source"] = json!("user");
    records[1]["payload"]["internal_chat_message_metadata_passthrough"] =
        json!({"content_item_kinds":["skills.selected_skill_instructions"]});
    records[2]["payload"]["id"] = json!("codex-assistant-mention");
    records[3]["payload"]["id"] = json!("codex-user-mention");
    records
}

fn published_selected_skill(agent: AgentKind) -> SessionContentEvidence {
    let input = selected_skill_input(agent);
    let store = store(&input);
    selected(&store, &input.agent, input.source_format, "root", 1)
}

fn selected_skill_input(agent: AgentKind) -> SessionInput {
    let (agent, source_format, text) = match agent {
        AgentKind::Codex => (
            "codex",
            SourceFormat::CodexRolloutJsonl,
            jsonl(&codex_skill_records()),
        ),
        AgentKind::Pi => {
            let records = [
                json!({"type":"session","version":3,"id":"root","timestamp":"2026-01-01T00:00:00Z","cwd":"/synthetic/project"}),
                json!({"type":"message","id":"pi-selected","parentId":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"text","text":"<skill name=\"review\" location=\"/synthetic/skills/review/SKILL.md\">\nReferences are relative to /synthetic/skills/review.\n\nReview ownership and error handling.\n</skill>\n\nFocus on resources."}]}}),
            ];
            ("pi", SourceFormat::PiV3Jsonl, jsonl(&records))
        }
        _ => unreachable!(),
    };
    SessionInput {
        agent: agent.into(),
        session_id: "root".into(),
        source: RawSource::Jsonl(text),
        source_format,
        fork_parent_session_id: None,
    }
}

#[test]
fn source_adapters_normalize_equivalent_skill_facts_before_projection() {
    use antiburn_local::analysis::jev_evidence::JevRecordedSkillStatus;
    let native = parsed_records(OPENCODE);
    let (_temporary, _store, opencode) = opencode_content(&native[0]);
    let expected = opencode.actions[1]
        .metadata
        .recorded_skill_result
        .as_ref()
        .unwrap();
    for agent in [AgentKind::Codex, AgentKind::Pi] {
        let input = selected_skill_input(agent);
        let store = store(&input);
        let mut content = prepared(&store, &input.agent, input.source_format, "root", 1);
        for action in &mut content.actions {
            normalize_recorded_skill(action, input.source_format);
            let once = action.metadata.clone();
            normalize_recorded_skill(action, input.source_format);
            assert_eq!(action.metadata, once, "normalization is idempotent");
        }
        let fact = content
            .actions
            .iter()
            .find_map(|action| {
                action
                    .metadata
                    .recorded_skill_result
                    .as_ref()
                    .filter(|fact| fact.status == JevRecordedSkillStatus::DocumentSelected)
            })
            .expect("source-qualified selection normalizes");
        assert!(
            content
                .actions
                .iter()
                .any(antiburn_local::analysis::jev_evidence::is_recorded_skill_selection)
        );
        assert_eq!(
            (
                fact.identity,
                &fact.name,
                &fact.location,
                fact.state,
                fact.status
            ),
            (
                expected.identity,
                &expected.name,
                &expected.location,
                expected.state,
                expected.status
            )
        );
        assert!(
            content
                .actions
                .iter()
                .all(|action| action.metadata.selected_skill.is_none())
        );
    }
}

#[test]
fn shared_normalization_rejects_changed_wrappers_and_unbound_adapter_proofs() {
    for agent in [AgentKind::Codex, AgentKind::Pi] {
        let input = selected_skill_input(agent);
        let store = store(&input);
        let original = prepared(&store, &input.agent, input.source_format, "root", 1);
        let original = original
            .actions
            .iter()
            .find(|action| action.metadata.recorded_skill_result.is_some())
            .expect("preparation consumes the accepted source proof");
        assert!(original.metadata.selected_skill.is_none());
        for case in 0..6 {
            let mut action = original.clone();
            match case {
                0 => action.text = "Quoted skill text.".into(),
                1 => action.metadata.bindings.clear(),
                2 => action.reference.native_record_id = Some("other-message".into()),
                3 => action.reference.part_index += 1,
                4 => {
                    action
                        .metadata
                        .recorded_skill_result
                        .as_mut()
                        .unwrap()
                        .source_format = SourceFormat::OpenCodeJsonl
                }
                _ => {
                    action
                        .metadata
                        .recorded_skill_result
                        .as_mut()
                        .unwrap()
                        .ranges[0]
                        .end -= 1
                }
            }
            normalize_recorded_skill(&mut action, input.source_format);
            assert!(action.metadata.selected_skill.is_none());
            assert!(
                action.metadata.recorded_skill_result.is_none(),
                "{agent:?} case {case}"
            );
        }
    }
}

#[test]
fn shared_normalization_rejects_stale_request_text_and_normalized_fields() {
    use antiburn_local::analysis::jev::JevInputField;
    let (store, selected) = jsonl_content("claude", SourceFormat::ClaudeJsonl, CLAUDE);
    let prepared = prepared(&store, "claude", SourceFormat::ClaudeJsonl, "root", 1);
    for original in [&prepared.actions[0], &selected.actions[0]] {
        let fact = original.metadata.recorded_skill_result.as_ref().unwrap();
        assert!(fact.matches_action(original, SourceFormat::ClaudeJsonl));
        for change_raw_text in [true, false] {
            let mut action = original.clone();
            let changed = r#"{"skill":"other-skill"}"#.to_owned();
            if change_raw_text {
                action.text = changed;
            } else {
                action
                    .normalized_fields
                    .as_mut()
                    .unwrap()
                    .values
                    .insert(JevInputField::OtherToolInput, changed);
            }
            assert!(!fact.matches_action(&action, SourceFormat::ClaudeJsonl));
            normalize_recorded_skill(&mut action, SourceFormat::ClaudeJsonl);
            assert!(action.metadata.recorded_skill_result.is_none());
        }
    }
}

fn matching_candidates(content: &SessionContentEvidence, agent: AgentKind) -> usize {
    let bound = boundary(content, agent);
    let usage = SkillUseSnapshot::from_published_content(content, &bound).unwrap();
    let inventory = SkillOpportunitySnapshot::new(
        bound.scope.clone(),
        vec![SkillDefinition {
            identity: "review-file".into(),
            revision: "current-revision".into(),
            name: "review".into(),
            aliases: vec![],
            description: "Review resource ownership.".into(),
            frontmatter: json!({"description":"Review resource ownership."}),
            scope: bound.scope.clone(),
            enabled: true,
            created_at_ms: None,
        }],
        true,
    )
    .unwrap();
    inventory
        .eligible_candidates_with_recorded_use(
            &SkillWorkContext {
                session_identity: bound.session_identity.clone(),
                scope: bound.scope.clone(),
                relevant_work_at_ms: Some(1767225601000),
            },
            &usage,
        )
        .unwrap()
        .len()
}

#[test]
fn persisted_selection_suppresses_matching_candidates_without_human_authority_or_success() {
    for agent in [AgentKind::Codex, AgentKind::Pi] {
        let mut content = published_selected_skill(agent);
        let action = content
            .actions
            .iter_mut()
            .find(|action| action.metadata.recorded_skill_result.is_some())
            .expect("parse, store, query, and selection retain typed skill proof");
        assert_ne!(action.authority, "user");
        action.authority = "unknown".into();
        let snapshot =
            SkillUseSnapshot::from_published_content(&content, &boundary(&content, agent)).unwrap();
        assert_eq!(snapshot.events().len(), 1, "{content:?}");
        assert!(matches!(
            snapshot.events()[0].lifecycle,
            SkillUseLifecycle::DocumentSelected | SkillUseLifecycle::Requested
        ));
        assert_eq!(matching_candidates(&content, agent), 0);
    }
}

#[test]
fn incomplete_or_mismatched_persisted_selection_does_not_suppress_candidates() {
    for agent in [AgentKind::Codex, AgentKind::Pi] {
        let original = published_selected_skill(agent);
        for case in 0..12 {
            let mut content = original.clone();
            if case == 0 {
                content.complete = false;
            } else {
                let action = content
                    .actions
                    .iter_mut()
                    .find(|action| action.metadata.recorded_skill_result.is_some())
                    .expect("published selected-skill proof");
                match case {
                    1 => action.truncated = true,
                    2 => action.reference.stable = false,
                    3 => action.metadata.bindings.clear(),
                    4 => action.metadata.recorded_skill_result = None,
                    _ => {
                        let proof = action.metadata.recorded_skill_result.as_mut().unwrap();
                        match case {
                            5 => proof.complete = false,
                            6 => proof.session_id = Some("other-session".into()),
                            7 => proof.message_id = Some("other-message".into()),
                            8 => proof.name = Some("other-skill".into()),
                            9 => proof.text_digest = "wrong-content-digest".into(),
                            10 => proof.source_format = SourceFormat::OpenCodeJsonl,
                            _ => proof.ranges[0].end -= 1,
                        }
                    }
                }
            }
            assert_eq!(
                matching_candidates(&content, agent),
                1,
                "{agent:?} case {case}"
            );
        }
    }
}

#[test]
fn selected_proof_preserves_requested_status_and_excludes_unselected_ranges() {
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    use antiburn_local::analysis::jev_evidence::JevRecordedSkillStatus;
    let mut content = published_selected_skill(AgentKind::Codex);
    let proof = content
        .actions
        .iter_mut()
        .find_map(|action| action.metadata.recorded_skill_result.as_mut())
        .unwrap();
    proof.status = JevRecordedSkillStatus::Requested;
    assert!(content.actions.iter().all(|action| {
        action
            .metadata
            .selected(JevInputSelection::from_fields(&[
                JevInputField::OtherToolOutput,
            ]))
            .recorded_skill_result
            .is_none()
    }));
    let snapshot =
        SkillUseSnapshot::from_published_content(&content, &boundary(&content, AgentKind::Codex))
            .unwrap();
    assert_eq!(snapshot.events()[0].lifecycle, SkillUseLifecycle::Requested);
    assert_eq!(matching_candidates(&content, AgentKind::Codex), 0);
}

#[test]
fn codex_wrapper_without_selected_skill_producer_tag_does_not_suppress_use() {
    let (_, content) = jsonl_content("codex", SourceFormat::CodexRolloutJsonl, CODEX);
    assert!(
        content
            .actions
            .iter()
            .all(|action| action.metadata.selected_skill.is_none())
    );
    assert_eq!(matching_candidates(&content, AgentKind::Codex), 1);
}

#[test]
fn opencode_codex_and_pi_publish_equivalent_document_selection() {
    use antiburn_local::analysis::jev_evidence::JevRecordedSkillStatus;
    let native = parsed_records(OPENCODE);
    let (_temporary, _store, opencode) = opencode_content(&native[0]);
    let mut expected_fact = None;
    let mut expected_event = None;
    for (agent, content) in [
        (AgentKind::OpenCode, opencode),
        (AgentKind::Codex, published_selected_skill(AgentKind::Codex)),
        (AgentKind::Pi, published_selected_skill(AgentKind::Pi)),
    ] {
        assert!(
            content
                .actions
                .iter()
                .all(|action| action.metadata.selected_skill.is_none())
        );
        let fact = content
            .actions
            .iter()
            .find_map(|action| {
                action
                    .metadata
                    .recorded_skill_result
                    .as_ref()
                    .filter(|fact| fact.status == JevRecordedSkillStatus::DocumentSelected)
            })
            .expect("source adapter publishes generic document selection");
        let normalized = (
            fact.identity,
            fact.name.clone(),
            fact.location.clone(),
            fact.state,
            fact.status,
            fact.complete,
            fact.truncated,
        );
        if let Some(expected) = &expected_fact {
            assert_eq!(&normalized, expected, "{agent:?}");
        } else {
            expected_fact = Some(normalized);
        }
        let snapshot =
            SkillUseSnapshot::from_published_content(&content, &boundary(&content, agent)).unwrap();
        let event = snapshot
            .events()
            .iter()
            .find(|event| event.lifecycle == SkillUseLifecycle::DocumentSelected)
            .unwrap();
        let normalized = (event.skill.clone(), event.lifecycle);
        if let Some(expected) = &expected_event {
            assert_eq!(&normalized, expected, "{agent:?}");
        } else {
            expected_event = Some(normalized);
        }
        assert_eq!(matching_candidates(&content, agent), 0);
    }
}

#[test]
fn pi_quoted_empty_and_incomplete_wrappers_do_not_suppress_use() {
    for text in [
        "Quoted: <skill name=\"review\" location=\"/synthetic/skills/review/SKILL.md\">\nReferences are relative to /synthetic/skills/review.\n\nReview resources.\n</skill>",
        "<skill name=\"review\" location=\"/synthetic/skills/review/SKILL.md\">\nReferences are relative to /synthetic/skills/review.\n\n\n</skill>",
        "<skill name=\"review\" location=\"/synthetic/skills/review/SKILL.md\">\nReferences are relative to /synthetic/skills/review.\n\nReview resources.",
    ] {
        let records = [
            json!({"type":"session","version":3,"id":"root","timestamp":"2026-01-01T00:00:00Z","cwd":"/synthetic/project"}),
            json!({"type":"message","id":"pi-invalid","parentId":null,"timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"text","text":text}]}}),
        ];
        let (_, content) = jsonl_content("pi", SourceFormat::PiV3Jsonl, &jsonl(&records));
        assert_eq!(matching_candidates(&content, AgentKind::Pi), 1, "{text}");
    }
}
