use std::sync::Arc;

use antiburn_local::analysis::jev_evidence::*;
use antiburn_local::analysis::*;
use antiburn_local::model::AgentKind;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

const CLAUDE: &str = include_str!("fixtures/skill_use_characterization/claude.jsonl");
const OPENCODE: &str = include_str!("fixtures/skill_use_characterization/opencode.jsonl");
const PI: &str = include_str!("fixtures/skill_use_characterization/pi.jsonl");
const CODEX: &str = include_str!("fixtures/skill_use_characterization/codex.jsonl");
const SKILL_USE_SELECTION: JevInputSelection = JevInputSelection::from_fields(&[
    JevInputField::UserMessage,
    JevInputField::AssistantMessage,
    JevInputField::OtherToolInput,
    JevInputField::OtherToolOutput,
]);

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
        prepare_session_content("session-digest", format, content)
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

fn parsed_records(text: &str) -> Vec<Value> {
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
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
        source_format: SourceFormat::OpenCodeSqliteV1,
        fork_parent_session_id: None,
    };
    let store = store(&input);
    let content = selected(
        &store,
        "opencode",
        SourceFormat::OpenCodeSqliteV1,
        "root",
        1,
    );
    (temporary, store, content)
}

fn jsonl(records: &[Value]) -> String {
    records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
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

fn skill_facts(content: &SessionContentEvidence) -> Vec<&JevRecordedSkillResult> {
    content
        .actions
        .iter()
        .filter_map(|action| {
            let fact = action.metadata.recorded_skill_result.as_ref()?;
            assert!(
                fact.matches_action(action, content.source_format),
                "{action:#?}"
            );
            Some(fact)
        })
        .collect()
}

#[test]
fn native_requests_keep_identity_order_and_unknown_result_status() {
    for (agent, source, fixture, record) in [
        (
            "claude",
            SourceFormat::ClaudeJsonl,
            CLAUDE,
            "claude-request",
        ),
        ("pi", SourceFormat::PiV3Jsonl, PI, "pi-request"),
    ] {
        let (_, content) = jsonl_content(agent, source, fixture);
        let facts = skill_facts(&content);
        assert_eq!(facts.len(), 2, "{content:#?}");
        assert_eq!(facts[0].identity, JevRecordedSkillIdentityKind::Name);
        assert_eq!(facts[0].name.as_deref(), Some("review"));
        assert_eq!(facts[0].status, JevRecordedSkillStatus::Requested);
        assert_eq!(facts[1].status, JevRecordedSkillStatus::Unknown);
        assert_eq!(facts[0].call_id.as_deref(), Some("skill-call"));
        assert_eq!(
            content.actions[0].reference.native_record_id.as_deref(),
            Some(record)
        );
        assert_eq!(content.actions[0].timestamp_ms, Some(1767225601000));
        assert!(content.actions[0].reference.turn_index < content.actions[1].reference.turn_index);
    }
}

#[test]
fn opencode_completed_document_retains_native_bindings_without_task_success() {
    let native = parsed_records(OPENCODE);
    let (_temporary, _store, content) = opencode_content(&native[0]);
    let facts = skill_facts(&content);
    assert_eq!(facts.len(), 2, "{content:#?}");
    assert_eq!(facts[0].status, JevRecordedSkillStatus::Requested);
    assert_eq!(facts[1].status, JevRecordedSkillStatus::DocumentSelected);
    assert_eq!(facts[1].identity, JevRecordedSkillIdentityKind::Document);
    assert_eq!(facts[1].name.as_deref(), Some("review"));
    assert_eq!(
        facts[1].location.as_deref(),
        Some("/synthetic/skills/review/SKILL.md")
    );
    assert_eq!(
        facts[1].request_message_id.as_deref(),
        Some("skill-message")
    );
    assert_eq!(
        facts[1].request_part_index,
        Some(content.actions[0].reference.part_index)
    );
    assert!(facts[1].complete && !facts[1].truncated);
    assert_eq!(facts[1].ranges, content.actions[1].metadata.bindings);
}

#[test]
fn opencode_jsonl_does_not_gain_sqlite_document_proof() {
    let (_, content) = jsonl_content("opencode", SourceFormat::OpenCodeJsonl, OPENCODE);
    assert!(skill_facts(&content).is_empty());
}

#[test]
fn matched_native_failure_keeps_error_status_and_exact_result_range() {
    for (version, entrypoint) in [
        ("2.1.278", None),
        ("2.1.278", Some("cli")),
        ("2.1.309", Some("cli")),
    ] {
        let mut records = parsed_records(CLAUDE);
        for record in &mut records {
            record["version"] = json!(version);
            if let Some(entrypoint) = entrypoint {
                record["entrypoint"] = json!(entrypoint);
            } else {
                record.as_object_mut().unwrap().remove("entrypoint");
            }
        }
        records[1]["toolUseResult"]["success"] = json!(false);
        records[1]["message"]["content"][0]["is_error"] = json!(true);
        records[1]["message"]["content"][0]["content"] = json!("Skill permission denied.");
        let (_, content) = jsonl_content("claude", SourceFormat::ClaudeJsonl, &jsonl(&records));
        let facts = skill_facts(&content);
        assert_eq!(
            facts[1].status,
            JevRecordedSkillStatus::Failed,
            "version {version}, entrypoint {entrypoint:?}"
        );
        assert_eq!(facts[1].state, JevOperationState::Error);
        let action = &content.actions[1];
        assert_eq!(facts[1].ranges[0].pointer, "/message/content/0/content");
        assert_eq!(facts[1].ranges[0].end, action.text.len());
        for change in 0..4 {
            let mut changed = action.clone();
            match change {
                0 => changed.metadata.bindings.clear(),
                1 => changed.reference.native_record_id = Some("other-record".into()),
                2 => changed.tool_call_id = Some("other-call".into()),
                _ => changed.metadata.bindings[0].end -= 1,
            }
            assert!(!facts[1].matches_action(&changed, content.source_format));
        }
    }
}

#[test]
fn opencode_partial_and_unbound_documents_do_not_gain_selection_status() {
    for case in 0..8 {
        let mut native = parsed_records(OPENCODE);
        match case {
            0 => {
                native[0]["state"]["metadata"]
                    .as_object_mut()
                    .unwrap()
                    .remove("name");
            }
            1 => native[0]["state"]["status"] = json!("running"),
            2 => native[0]["state"]["metadata"]["truncated"] = json!(true),
            3 => native[0]["state"]["time"]["compacted"] = json!(1767225602000i64),
            4 => native[0]["metadata"] = json!({"providerExecuted":true}),
            5 => native[0]["state"]["output"] = json!("Loaded review successfully."),
            6 => native[0]["state"]["input"]["name"] = json!("alias-review"),
            _ => native[0]["state"]["metadata"]["dir"] = json!("/synthetic/other"),
        }
        let (_temporary, store, content) = opencode_content(&native[0]);
        store.with_connection(|connection| {
            let accepted: i64 = connection.query_row(
                "SELECT count(*) FROM turn_content WHERE json_extract(normalized_fields_json, '$.metadata.recorded_skill_result.status') = 'document_selected'",
                [], |row| row.get(0)).unwrap();
            assert_eq!(accepted, 0, "adapter case {case}");
        });
        assert!(
            skill_facts(&content)
                .iter()
                .all(|fact| fact.status != JevRecordedSkillStatus::DocumentSelected),
            "case {case}"
        );
    }
}

#[test]
fn timestamps_do_not_replace_native_positions() {
    let mut records = parsed_records(CLAUDE);
    records[0].as_object_mut().unwrap().remove("timestamp");
    let (_, content) = jsonl_content("claude", SourceFormat::ClaudeJsonl, &jsonl(&records));
    assert_eq!(content.actions[0].timestamp_ms, None);
    assert_eq!(content.actions[0].reference.turn_index, 0);
    records = parsed_records(CLAUDE);
    records[1]["timestamp"] = json!("2026-01-01T00:00:00Z");
    let (_, content) = jsonl_content("claude", SourceFormat::ClaudeJsonl, &jsonl(&records));
    assert!(content.actions[0].timestamp_ms > content.actions[1].timestamp_ms);
    assert!(content.actions[0].reference.turn_index < content.actions[1].reference.turn_index);
}

#[test]
fn selected_skill_queries_keep_session_and_publication_fences() {
    for agent in [AgentKind::Codex, AgentKind::Pi] {
        let input = selected_skill_input(agent);
        let store = store(&input);
        let content = selected(&store, &input.agent, input.source_format, "root", 1);
        assert_eq!(skill_facts(&content).len(), 1);
        assert_eq!(content.publication_fence, 1);
        assert!(
            selected(
                &store,
                &input.agent,
                input.source_format,
                "other-session",
                1
            )
            .actions
            .is_empty()
        );
        assert!(
            selected(&store, &input.agent, input.source_format, "root", 2)
                .actions
                .is_empty()
        );
    }
}

#[test]
fn selected_document_proof_keeps_requested_status_and_requires_selected_fields() {
    for agent in [AgentKind::Codex, AgentKind::Pi] {
        let input = selected_skill_input(agent);
        let store = store(&input);
        let mut content = selected(&store, &input.agent, input.source_format, "root", 1);
        let action = content
            .actions
            .iter_mut()
            .find(|action| action.metadata.recorded_skill_result.is_some())
            .unwrap();
        assert_ne!(action.authority, "user");
        assert!(action.metadata.human_text.is_none());
        action
            .metadata
            .recorded_skill_result
            .as_mut()
            .unwrap()
            .status = JevRecordedSkillStatus::Requested;
        assert_eq!(
            skill_facts(&content)[0].status,
            JevRecordedSkillStatus::Requested
        );
        let excluded = select_session_content(
            &content,
            JevInputSelection::from_fields(&[JevInputField::OtherToolOutput]),
        );
        assert!(
            excluded
                .actions
                .iter()
                .all(|action| action.metadata.recorded_skill_result.is_none())
        );
        assert!(content.actions.iter().all(|action| {
            action
                .metadata
                .selected(JevInputSelection::NONE)
                .recorded_skill_result
                .is_none()
        }));
    }
}

#[test]
fn compatibility_names_and_paths_remain_inferred_requests() {
    for input in [
        json!({"name":"review"}),
        json!({"skill_name":"review"}),
        json!({"skillName":"review"}),
        json!({"command":"/review extra"}),
        json!({"path":"/synthetic/skills/review/SKILL.md"}),
    ] {
        let mut records = parsed_records(CLAUDE);
        records[0]["message"]["content"][0]["input"] = input;
        let (_, content) = jsonl_content("claude", SourceFormat::ClaudeJsonl, &jsonl(&records));
        let facts = skill_facts(&content);
        assert_eq!(
            facts[0].identity,
            JevRecordedSkillIdentityKind::InferredName
        );
        assert_eq!(facts[0].name.as_deref(), Some("review"));
        assert_eq!(facts[0].status, JevRecordedSkillStatus::Requested);
        assert_eq!(facts[1].status, JevRecordedSkillStatus::Unknown);
    }
    let mut records = parsed_records(CLAUDE);
    records[0]["message"]["content"][0]["input"] = json!({"skill":"review","name":"different"});
    let (_, content) = jsonl_content("claude", SourceFormat::ClaudeJsonl, &jsonl(&records));
    assert_eq!(
        skill_facts(&content)[0].identity,
        JevRecordedSkillIdentityKind::Unknown
    );
}

#[test]
fn codex_missing_producer_tag_and_invalid_wrappers_do_not_prove_selection() {
    let (_, content) = jsonl_content("codex", SourceFormat::CodexRolloutJsonl, CODEX);
    assert!(skill_facts(&content).is_empty());
    for text in [
        "<skill>\n<name>review</name>\n<path>/synthetic/SKILL.md</path>\n\n</skill>",
        "Quoted: <skill>\n<name>review</name>\n<path>/synthetic/SKILL.md</path>\nFull body.\n</skill>",
        "<skill>\n<name>review</name>\nListing only.\n</skill>",
    ] {
        let mut records = codex_skill_records();
        records[1]["payload"]["content"][0]["text"] = json!(text);
        let (_, content) =
            jsonl_content("codex", SourceFormat::CodexRolloutJsonl, &jsonl(&records));
        assert!(skill_facts(&content).is_empty(), "{text}");
    }
    let mut records = codex_skill_records();
    records[1]["payload"]["role"] = json!("assistant");
    let (_, content) = jsonl_content("codex", SourceFormat::CodexRolloutJsonl, &jsonl(&records));
    assert!(skill_facts(&content).is_empty());
}

#[test]
fn pi_quoted_empty_and_incomplete_wrappers_do_not_prove_selection() {
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
        assert!(skill_facts(&content).is_empty(), "{text}");
    }
}
