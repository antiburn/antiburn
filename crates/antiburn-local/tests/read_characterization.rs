use std::sync::Arc;

use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
use antiburn_local::analysis::jev_evidence::*;
use antiburn_local::analysis::*;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

const FIXTURE: &str = include_str!("fixtures/read_characterization/opencode.jsonl");
const LEGACY_INPUT_SELECTION: JevInputSelection = JevInputSelection::from_fields(&[
    JevInputField::AssistantMessage,
    JevInputField::BashCommandInput,
    JevInputField::FileEditPath,
    JevInputField::ReadFilePath,
    JevInputField::SearchFilesQuery,
    JevInputField::OtherToolInput,
]);

fn native_fixture(agent: &str) -> (SourceFormat, &'static str, &'static str) {
    match agent {
        "codex" => (
            SourceFormat::CodexRolloutJsonl,
            "synthetic-root",
            include_str!("fixtures/codex_characterization/paginated_completed.jsonl"),
        ),
        "claude" => (
            SourceFormat::ClaudeJsonl,
            "scope-test",
            include_str!("fixtures/claude_characterization/retained_native_results.jsonl"),
        ),
        "pi" => (
            SourceFormat::PiV3Jsonl,
            "synthetic",
            include_str!("fixtures/pi_characterization/core_linear_v3.jsonl"),
        ),
        _ => panic!("unknown native fixture"),
    }
}

#[derive(Default)]
struct NativeContentSink {
    published: PublishedContent,
    native_id: Option<String>,
}

impl RecordSink for NativeContentSink {
    fn record(&mut self, record: NormalizedRecord) {
        match record {
            NormalizedRecord::MetricsEvent(event) => {
                self.native_id = event.uuid.or(event.message_id);
            }
            NormalizedRecord::TurnContent(content) => {
                let turn_index = self.published.parts.len() as u64 + 1;
                for (part_index, part) in content.parts.into_iter().enumerate() {
                    self.published.parts.push(PublishedContentPart {
                        source_key: "synthetic-native".into(),
                        thread_id: "root".into(),
                        turn_index,
                        role: "assistant",
                        scope: "main".into(),
                        ts_ms: None,
                        uuid: self.native_id.clone(),
                        message_id: None,
                        part_index: part_index as u32,
                        part,
                        context_only: false,
                        stable_event_identity: self.native_id.is_some(),
                    });
                }
            }
            _ => {}
        }
    }

    fn finish(&mut self, _summary: SessionSummary) {}
}

fn native_content(agent: &str, records: &str) -> PublishedContent {
    let (source_format, session_id, _) = native_fixture(agent);
    let input = SessionInput {
        agent: agent.into(),
        session_id: session_id.into(),
        source: RawSource::Jsonl(records.into()),
        fork_parent_session_id: None,
        source_format,
    };
    let mut sink = NativeContentSink::default();
    reader_for(agent).visit(&input, &mut sink).unwrap();
    sink.published
}

#[test]
fn native_adapters_preserve_observed_reads_and_rebind_final_references() {
    for (agent, contract, start, count, truncated) in [
        (
            "codex",
            "openai/codex@d27764b82f7118f674371e6d6e76271d9d606edb",
            2,
            1,
            false,
        ),
        (
            "claude",
            "claude-code/2.1.278:retained-cli-text-read",
            7,
            3,
            false,
        ),
        (
            "pi",
            "pi/core/tools/read.ts@b79e4cc834970cca69daebffab7df1da7d1e52c4",
            7,
            3,
            true,
        ),
    ] {
        let (source, session, records) = native_fixture(agent);
        let published = native_content(agent, records);
        let native = published
            .parts
            .iter()
            .find_map(|p| p.part.metadata.read_result.clone())
            .unwrap();
        if agent == "codex" {
            let mut selected = published.clone();
            for item in &mut selected.parts {
                if item.part.metadata.read_request.is_some() {
                    item.part.text.clear();
                    item.part.metadata.read_request = None;
                }
            }
            let selected = prepare_session_content(session, source, selected, vec![]);
            let result = selected
                .actions
                .iter()
                .find_map(|a| a.metadata.read_result.as_ref())
                .unwrap();
            assert_eq!(result.returned_extent, native.returned_extent);
            assert_eq!(result.extent_contract, native.extent_contract);
            assert_eq!(result.status, native.status);
            let request = selected
                .actions
                .iter()
                .find_map(|a| a.metadata.read_request.as_ref())
                .unwrap();
            assert_eq!(request.extent.end_inclusive, Some(4));
            assert_eq!(
                result.request_reference_id.as_deref(),
                Some(request.reference_id.as_str())
            );
        }
        let evidence = prepare_session_content(session, source, published, vec![]);
        let output = evidence
            .actions
            .iter()
            .find(|a| a.metadata.read_result.is_some())
            .unwrap();
        let result = output.metadata.read_result.as_ref().unwrap();
        let input = evidence
            .actions
            .iter()
            .find(|a| Some(a.reference.id.as_str()) == result.request_reference_id.as_deref())
            .unwrap();
        let request = input.metadata.read_request.as_ref().unwrap();
        assert_eq!(request.reference_id, input.reference.id, "{agent}");
        assert_eq!(result.reference_id, output.reference.id, "{agent}");
        assert_eq!(result.status, JevReadStatus::Success, "{agent}");
        assert_eq!(result.kind, JevReadResultKind::File, "{agent}");
        assert_eq!(
            result.recorded_output_bytes, native.recorded_output_bytes,
            "{agent}"
        );
        assert_eq!(
            result.recorded_output_digest, native.recorded_output_digest,
            "{agent}"
        );
        assert_eq!(result.returned_extent, native.returned_extent, "{agent}");
        assert_eq!(result.extent_contract.as_deref(), Some(contract), "{agent}");
        let extent = result.returned_extent.as_ref().unwrap();
        assert_eq!(extent.offset, Some(start), "{agent}");
        assert_eq!(extent.limit, Some(count), "{agent}");
        assert_eq!(extent.end_inclusive, Some(start + count - 1), "{agent}");
        assert_eq!(result.truncated, truncated, "{agent}");
        assert_eq!(result.recorded_file_version, None);
        if agent == "codex" {
            assert_eq!(request.extent.offset, Some(2));
            assert_eq!(request.extent.limit, Some(3));
            assert_eq!(request.extent.end_inclusive, Some(4));
            assert_eq!(request.cwd.as_deref(), Some("file:///synthetic"));
            assert_eq!(request.extent_contract.as_deref(), Some(contract));
        }
        for action in &evidence.actions {
            if matches!(
                action.tool_name.as_deref(),
                Some("Bash" | "Skill" | "exec_command" | "apply_patch")
            ) {
                assert!(action.metadata.read_request.is_none(), "{agent}");
                assert!(action.metadata.read_result.is_none(), "{agent}");
            }
        }
        let mut repeated = evidence.actions.clone();
        normalize_read_evidence(source, &mut repeated);
        assert_eq!(
            repeated, evidence.actions,
            "normalization must be idempotent: {agent}"
        );
    }
}

#[test]
fn native_read_facts_require_source_pin_binding_digest_and_observed_extent() {
    for agent in ["codex", "claude", "pi"] {
        let (source, session, records) = native_fixture(agent);
        let original = native_content(agent, records);
        for mutation in [
            "producer",
            "binding",
            "digest",
            "bytes",
            "extent",
            "version",
            "state",
            "source",
            "thread",
            "duplicate",
            "later_duplicate",
        ] {
            let mut published = original.clone();
            let index = published
                .parts
                .iter()
                .position(|p| p.part.metadata.read_result.is_some())
                .unwrap();
            let result_part = &mut published.parts[index];
            let result = result_part.part.metadata.read_result.as_mut().unwrap();
            match mutation {
                "producer" => result.extent_contract = Some("unreviewed-producer".into()),
                "binding" => result_part.part.metadata.bindings.clear(),
                "digest" => result.recorded_output_digest = "00".repeat(32),
                "bytes" => result.recorded_output_bytes += 1,
                "extent" => {
                    let extent = result.returned_extent.as_mut().unwrap();
                    extent.offset = Some(100);
                    extent.end_inclusive = Some(100 + extent.limit.unwrap() - 1);
                }
                "version" => result.recorded_file_version = Some("injected-version".into()),
                "state" => result_part.part.metadata.state = JevOperationState::Unknown,
                "thread" => result_part.thread_id = "other".into(),
                "duplicate" | "later_duplicate" => {
                    let call_id = result_part.part.tool_call_id.clone();
                    let request = published
                        .parts
                        .iter()
                        .find(|p| {
                            p.part.kind == ContentKind::ToolInput && p.part.tool_call_id == call_id
                        })
                        .unwrap()
                        .clone();
                    if mutation == "duplicate" {
                        published.parts.insert(index, request);
                    } else {
                        published.parts.push(request);
                    }
                }
                "source" => {}
                _ => unreachable!(),
            }
            let source = if mutation == "source" {
                SourceFormat::OpenCodeJsonl
            } else {
                source
            };
            let evidence = prepare_session_content(session, source, published, vec![]);
            let result = evidence
                .actions
                .iter()
                .find_map(|a| a.metadata.read_result.as_ref())
                .unwrap();
            assert_eq!(result.returned_extent, None, "{agent}: {mutation}");
            assert_ne!(result.status, JevReadStatus::Success, "{agent}: {mutation}");
            assert_eq!(result.recorded_file_version, None, "{agent}: {mutation}");
            if matches!(mutation, "thread" | "duplicate" | "later_duplicate") {
                assert_eq!(result.request_reference_id, None, "{agent}: {mutation}");
            }
        }
    }
}

#[test]
fn native_failed_clipped_and_nonread_records_do_not_claim_returned_extents() {
    for agent in ["codex", "claude", "pi"] {
        let (source, session, records) = native_fixture(agent);
        for mutation in ["failed", "clipped", "extent_mismatch", "nonread"] {
            let records = records.lines().map(|line| {
                let mut record: Value = serde_json::from_str(line).unwrap();
                match agent {
                    "codex" if record["payload"]["item"]["id"] == "read-1" => {
                        let item = &mut record["payload"]["item"];
                        match mutation {
                            "failed" => {
                                item["status"] = json!("failed");
                                item["exit_code"] = json!(1);
                            }
                            "clipped" => {
                                for key in ["stdout", "aggregated_output"] {
                                    item[key] = json!("     2\t[truncated]\n");
                                }
                            }
                            "extent_mismatch" => {
                                for key in ["stdout", "aggregated_output"] {
                                    item[key] = json!("    99\tunexpected line\n");
                                }
                            }
                            "nonread" => item["parsed_cmd"][0]["type"] = json!("unknown"),
                            _ => unreachable!(),
                        }
                    }
                    "claude" => {
                        if mutation == "nonread" && record["type"] == "assistant"
                            && record["message"]["content"][0]["id"] == "read-1"
                        {
                            record["message"]["content"][0]["name"] = json!("Bash");
                        }
                        if record["message"]["content"][0]["tool_use_id"] == "read-1" {
                            match mutation {
                                "failed" => record["message"]["content"][0]["is_error"] = json!(true),
                                "clipped" => record["toolUseResult"]["truncated"] = json!(true),
                                "extent_mismatch" => record["toolUseResult"]["file"]["startLine"] = json!(70),
                                "nonread" => {},
                                _ => unreachable!(),
                            }
                        }
                    }
                    "pi" => {
                        if mutation == "nonread" && record["id"] == "call" {
                            record["message"]["content"][0]["name"] = json!("bash");
                        }
                        if record["id"] == "result" {
                            match mutation {
                                "failed" => record["message"]["isError"] = json!(true),
                                "clipped" => record["message"]["details"]["truncation"] = json!({"truncated":true,"lastLinePartial":true}),
                                "extent_mismatch" => record["message"]["content"][0]["text"] = json!("return café\n\nreturn 2\n\n[4 more lines in file. Use offset=11 to continue.]"),
                                "nonread" => record["message"]["toolName"] = json!("bash"),
                                _ => unreachable!(),
                            }
                        }
                    }
                    _ => {},
                }
                record.to_string()
            }).collect::<Vec<_>>().join("\n") + "\n";
            let published = native_content(agent, &records);
            let evidence = prepare_session_content(session, source, published, vec![]);
            let results: Vec<_> = evidence
                .actions
                .iter()
                .filter_map(|a| a.metadata.read_result.as_ref())
                .collect();
            if mutation == "nonread" {
                assert!(results.is_empty(), "{agent}");
                assert!(
                    evidence
                        .actions
                        .iter()
                        .all(|a| a.metadata.read_request.is_none()),
                    "{agent}"
                );
            } else {
                assert_eq!(results.len(), 1, "{agent}: {mutation}");
                assert_eq!(results[0].returned_extent, None, "{agent}: {mutation}");
                assert_eq!(results[0].recorded_file_version, None);
                if mutation == "failed" {
                    assert_eq!(results[0].status, JevReadStatus::Failed, "{agent}");
                }
                if mutation == "clipped" {
                    assert!(results[0].truncated, "{agent}");
                }
            }
        }
    }
}

fn fixture_store() -> (tempfile::TempDir, Arc<MemoryTurnRowStore>) {
    fixture_store_for(FIXTURE)
}

fn fixture_store_for(records: &str) -> (tempfile::TempDir, Arc<MemoryTurnRowStore>) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, title TEXT, time_created INTEGER, time_updated INTEGER);
        CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
        CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
        INSERT INTO session VALUES ('root', NULL, NULL, 1000, 1000);").unwrap();
    for (index, line) in records.lines().enumerate() {
        let mut part: Value = serde_json::from_str(line).unwrap();
        part["type"] = json!("tool");
        let id = part["id"].as_str().unwrap();
        connection
            .execute(
                "INSERT INTO message VALUES (?1, 'root', ?2, ?2, ?3)",
                params![
                    id,
                    1000 + index,
                    json!({"role":"assistant", "modelID":"model", "tokens":{"input":1,"output":1}})
                        .to_string()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO part VALUES (?1, ?1, 'root', ?2, ?2, ?3)",
                params![id, 1000 + index, part.to_string()],
            )
            .unwrap();
    }
    let input = SessionInput {
        agent: "opencode".into(),
        session_id: "root".into(),
        source: RawSource::Sqlite(path),
        fork_parent_session_id: None,
        source_format: SourceFormat::OpenCodeSqliteV2,
    };
    (directory, input_store(&input))
}

fn input_store(input: &SessionInput) -> Arc<MemoryTurnRowStore> {
    let reader = reader_for(&input.agent);
    let store = MemoryTurnRowStore::new(&input.agent, "root");
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new(&input.agent, "root"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: input.agent.clone(),
            session_id: "root".into(),
            kind: SourceKind::from(&input.source),
            capabilities: reader.capabilities(input),
        }),
        TurnRowSink::new(Arc::clone(&store) as Arc<dyn TurnRowStore>, "root", None),
    );
    let outcome = reader.visit(input, &mut sink).unwrap();
    sink.observe_source_outcome(outcome);
    store
}

fn query(store: &MemoryTurnRowStore, selection: JevInputSelection) -> SessionContentEvidence {
    query_source(store, "opencode", SourceFormat::OpenCodeSqliteV2, selection)
}

fn query_source(
    store: &MemoryTurnRowStore,
    agent: &str,
    source: SourceFormat,
    selection: JevInputSelection,
) -> SessionContentEvidence {
    store.with_connection(|connection| {
        let content = query_turn_content_offset_selected(
            connection,
            &TurnSessionKey {
                environment_key: "native",
                agent,
                session_id: "root",
            },
            &FenceScope::single(1),
            None,
            &Default::default(),
            0,
            selection,
        )
        .unwrap();
        select_session_content(
            &prepare_session_content("root", source, content, vec![]),
            selection,
        )
    })
}

#[test]
fn claude_and_pi_request_units_are_characterized_without_invented_result_extents() {
    for (agent, source, fixture) in [
        (
            "claude",
            SourceFormat::ClaudeJsonl,
            include_str!("fixtures/read_characterization/claude.jsonl"),
        ),
        (
            "pi",
            SourceFormat::PiV3Jsonl,
            include_str!("fixtures/read_characterization/pi.jsonl"),
        ),
    ] {
        let input = SessionInput {
            agent: agent.into(),
            session_id: "root".into(),
            source: RawSource::Jsonl(fixture.into()),
            fork_parent_session_id: None,
            source_format: source,
        };
        let store = input_store(&input);
        let evidence = query_source(
            &store,
            agent,
            source,
            JevInputSelection::from_fields(&[
                JevInputField::ReadFileRequest,
                JevInputField::ReadFileResult,
            ]),
        );
        assert_eq!(evidence.actions.len(), 2, "{agent}");
        let request = evidence.actions[0].metadata.read_request.as_ref().unwrap();
        assert_eq!(request.extent.unit, JevReadUnit::Lines);
        assert_eq!(request.extent.offset, Some(7));
        assert_eq!(request.extent.limit, Some(3));
        assert!(request.extent_contract.is_some());
        let result = evidence.actions[1].metadata.read_result.as_ref().unwrap();
        assert_eq!(
            result.request_reference_id.as_deref(),
            Some(request.reference_id.as_str())
        );
        assert_eq!(result.returned_extent, None);
        assert_eq!(result.status, JevReadStatus::Unknown);
        assert_eq!(result.recorded_file_version, None);
    }
}

#[test]
fn native_reads_survive_selected_storage_without_expanding_legacy_input_selection() {
    let (_directory, store) = fixture_store();
    let selection = JevInputSelection::from_fields(&[
        JevInputField::ReadFileRequest,
        JevInputField::ReadFileResult,
    ]);
    let evidence = query(&store, selection);
    assert_eq!(evidence.actions.len(), 20);
    assert!(evidence.actions.iter().all(|action| action.text.is_empty()));
    let requests: Vec<_> = evidence
        .actions
        .iter()
        .filter_map(|a| a.metadata.read_request.as_ref())
        .collect();
    let results: Vec<_> = evidence
        .actions
        .iter()
        .filter_map(|a| a.metadata.read_result.as_ref())
        .collect();
    assert_eq!(requests.len(), 10);
    assert_eq!(results.len(), 10);
    assert!(results.iter().all(|r| r.request_reference_id.is_some()));
    assert_eq!(requests[0].extent.offset, Some(2));
    assert_eq!(requests[0].extent.limit, Some(2));
    assert_eq!(requests[0].extent.unit, JevReadUnit::Lines);
    assert_eq!(requests[0].paths, ["src/é.rs"]);
    assert_eq!(requests[0].cwd, None);
    assert_eq!(requests[5].extent.offset, None);
    assert_eq!(requests[5].extent.limit, None);
    let extent = |index: usize| results[index].returned_extent.clone().unwrap();
    assert_eq!(extent(0), extent(1));
    assert_eq!(extent(2).offset, extent(0).end_inclusive);
    assert!(extent(3).offset > extent(0).end_inclusive);
    assert_eq!(extent(4), extent(0));
    assert_ne!(
        results[4].recorded_output_bytes,
        results[0].recorded_output_bytes
    );
    assert_eq!(
        results[0].recorded_output_digest,
        results[1].recorded_output_digest
    );
    assert_ne!(
        results[0].recorded_output_digest,
        results[4].recorded_output_digest
    );
    assert!(results.iter().all(|r| r.recorded_file_version.is_none()));
    assert!(results[0].truncated);
    assert!(!results[5].truncated);
    assert_eq!(results[6].status, JevReadStatus::Failed);
    assert_eq!(results[6].returned_extent, None);
    assert_eq!(results[7].kind, JevReadResultKind::Directory);
    assert_eq!(requests[7].extent.unit, JevReadUnit::Unknown);
    assert_eq!(results[7].returned_extent, None);
    assert!(results[8].truncated);
    assert_eq!(results[8].returned_extent, None);
    assert_eq!(results[9].kind, JevReadResultKind::Unknown);
    assert_eq!(results[9].returned_extent, None);
    let native: Value = serde_json::from_str(FIXTURE.lines().next().unwrap()).unwrap();
    let output = native["state"]["output"].as_str().unwrap();
    assert_eq!(results[0].recorded_output_bytes, output.len() as u64);
    assert_ne!(output.len(), output.chars().count());

    let ignored = query(&store, LEGACY_INPUT_SELECTION);
    assert!(ignored.actions.iter().all(|a| a.kind != "tool_result"
        && a.metadata.read_request.is_none()
        && a.metadata.read_result.is_none()));
    assert!(ignored.field_availability.iter().all(|f| !matches!(
        f.field,
        JevInputField::ReadFileRequest | JevInputField::ReadFileResult
    )));
    let result_only = query(
        &store,
        JevInputSelection::from_fields(&[JevInputField::ReadFileResult]),
    );
    assert_eq!(result_only.actions.len(), 10);
    assert!(result_only.actions.iter().all(|a| {
        a.metadata
            .read_result
            .as_ref()
            .unwrap()
            .request_reference_id
            .is_some()
    }));
}

#[test]
fn ignored_instructions_selects_command_output_without_selecting_read_results() {
    let (_directory, store) = fixture_store();
    let legacy = query(&store, LEGACY_INPUT_SELECTION);
    let ignored = query(
        &store,
        antiburn_local::checks::ignored_instructions::INPUT_SELECTION,
    );
    let outputs: Vec<_> = ignored
        .actions
        .iter()
        .filter(|action| action.kind == "tool_result")
        .collect();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].tool_name.as_deref(), Some("bash"));
    assert_eq!(outputs[0].tool_call_id.as_deref(), Some("shell-1"));
    assert_eq!(outputs[0].text, "text");
    assert_eq!(ignored.actions.len(), legacy.actions.len() + 1);
    assert_eq!(
        select_session_content(&ignored, LEGACY_INPUT_SELECTION),
        legacy
    );
    assert!(ignored.actions.iter().all(|action| {
        action.metadata.read_request.is_none() && action.metadata.read_result.is_none()
    }));
    assert!(
        ignored
            .field_availability
            .iter()
            .any(|field| field.field == JevInputField::BashCommandOutput && field.selected)
    );
    for field in &ignored.field_availability {
        if matches!(
            field.field,
            JevInputField::ReadFileOutput
                | JevInputField::ReadFileRequest
                | JevInputField::ReadFileResult
        ) {
            assert!(!field.selected, "{:?} must remain unselected", field.field);
        }
    }
}

#[test]
fn unselected_read_facts_do_not_change_legacy_digests() {
    let (_directory, store) = fixture_store();
    let mut evidence = query(
        &store,
        JevInputSelection::from_fields(&[
            JevInputField::ReadFilePath,
            JevInputField::ReadFileRequest,
            JevInputField::ReadFileResult,
        ]),
    );
    let selection = LEGACY_INPUT_SELECTION;
    let before = select_session_content(&evidence, selection);
    for action in &mut evidence.actions {
        action.metadata.read_request = None;
        action.metadata.read_result = None;
    }
    assert_eq!(before, select_session_content(&evidence, selection));
}

#[test]
fn searches_listings_outlines_previews_and_shell_commands_do_not_prove_file_reads() {
    let (_directory, store) = fixture_store_for(include_str!(
        "fixtures/read_characterization/non_reads.jsonl"
    ));
    let evidence = query(
        &store,
        JevInputSelection::from_fields(&[
            JevInputField::ReadFileRequest,
            JevInputField::ReadFileResult,
        ]),
    );
    assert_eq!(evidence.actions.len(), 4);
    for action in &evidence.actions {
        assert_eq!(action.tool_name.as_deref(), Some("read"));
        if let Some(result) = &action.metadata.read_result {
            assert_eq!(result.kind, JevReadResultKind::Unknown);
            assert_eq!(result.returned_extent, None);
        }
    }
    let bash = query(
        &store,
        JevInputSelection::from_fields(&[JevInputField::BashCommandInput]),
    );
    assert_eq!(bash.actions.len(), 3);
    assert!(
        bash.actions
            .iter()
            .all(|a| a.metadata.read_request.is_none() && a.metadata.read_result.is_none())
    );
    let search = query(
        &store,
        JevInputSelection::from_fields(&[JevInputField::SearchFilesQuery]),
    );
    assert_eq!(search.actions.len(), 3);
    assert!(
        search
            .actions
            .iter()
            .all(|a| a.metadata.read_request.is_none())
    );
}

#[test]
fn reparsing_legacy_path_only_rows_restores_ranges_without_changing_selected_instruction_content() {
    let (directory, store) = fixture_store();
    let ignored_selection = LEGACY_INPUT_SELECTION;
    let current = query(&store, ignored_selection);
    store.with_connection(|conn| {
        conn.execute("UPDATE turn_content SET normalized_fields_json = json_remove(normalized_fields_json, '$.values.read_file_request') WHERE kind = 'tool_input'", []).unwrap();
    });
    let legacy = query(&store, ignored_selection);
    assert_eq!(current, legacy);
    let legacy_requests = query(
        &store,
        JevInputSelection::from_fields(&[JevInputField::ReadFileRequest]),
    );
    assert!(legacy_requests.actions.is_empty());
    let input = SessionInput {
        agent: "opencode".into(),
        session_id: "root".into(),
        source: RawSource::Sqlite(directory.path().join("opencode.db")),
        fork_parent_session_id: None,
        source_format: SourceFormat::OpenCodeSqliteV2,
    };
    let reparsed = input_store(&input);
    let restored = query(
        &reparsed,
        JevInputSelection::from_fields(&[JevInputField::ReadFileRequest]),
    );
    assert_eq!(restored.actions.len(), 10);
    let request = restored.actions[0].metadata.read_request.as_ref().unwrap();
    assert_eq!(request.extent.offset, Some(2));
    assert_eq!(request.extent.limit, Some(2));
    assert_eq!(legacy, query(&reparsed, ignored_selection));
    assert_eq!(PARSER_REVISION, 52);
    assert_eq!(EVIDENCE_SCHEMA_REVISION, 22);
}
