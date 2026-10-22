use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use antiburn_local::analysis::ignored_instructions::{
    AssessmentInput, ContentAction, ContentEventReference, IgnoredInstructionsCheck,
    InstructionProvenance, InstructionScope, SessionContentEvidence, build_jev_context,
    prepare_session_content, select_session_content, snapshot_from_text,
};
use antiburn_local::analysis::jev::{
    JevInputField, JevNormalizedCategory, JevNormalizedFields, JevRunProgress, run_jev_check,
};
use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, JevAnswer, JevCheck, JevQuestion, JevRequest, JevResponse,
    JevUsage, MemoryTurnRowStore, RawSource, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SourceCapabilities, SourceFormat, SourceKind, TurnRowSink,
    TurnRowStore, TurnSessionKey, query_turn_content, reader_for,
};

struct Case {
    agent: &'static str,
    format: SourceFormat,
    source: RawSource,
    content_marker: &'static str,
}

#[test]
fn accepted_native_report_keeps_selected_text_and_shell_output_but_excludes_thinking() {
    let report = format!(
        "{}I added disableLeaseGuard to the receiver.",
        "Recorded UTF-8 context é🦀.\n".repeat(650)
    );
    let transcript = [
        serde_json::json!({"type":"user","uuid":"family-user","message":{"role":"user","content":"Review the receiver change."}}),
        serde_json::json!({"type":"assistant","uuid":"family-assistant","parentUuid":"family-user","message":{"role":"assistant","stop_reason":"end_turn","content":[
            {"type":"thinking","thinking":"PRIVATE_SOURCE_THINKING"},
            {"type":"text","text":report},
            {"type":"tool_use","id":"family-check","name":"Bash","input":{"command":"git push --force"}}
        ]}}),
        serde_json::json!({"type":"user","uuid":"family-result","parentUuid":"family-assistant","message":{"role":"user","content":[
             {"type":"tool_result","tool_use_id":"family-check","content":"OBSERVED_NATIVE_CHECK_FAILURE","is_error":true}
        ]}}),
    ].into_iter().map(|record|record.to_string()).collect::<Vec<_>>().join("\n");
    let session_id = "native-family-characterization";
    let source = SessionInput {
        agent: "claude".to_owned(),
        session_id: session_id.to_owned(),
        source: RawSource::Jsonl(transcript),
        source_format: SourceFormat::ClaudeJsonl,
        fork_parent_session_id: None,
    };
    let store = MemoryTurnRowStore::new("claude", session_id);
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".to_owned(),
        session_id: session_id.to_owned(),
        kind: SourceKind::from(&source.source),
        capabilities: capabilities(source.source_format),
    });
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("claude", session_id),
        evidence,
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            session_id.to_owned(),
            None,
        ),
    );
    let outcome = reader_for("claude").visit(&source, &mut sink).unwrap();
    sink.observe_source_outcome(outcome);
    let content = store.with_connection(|connection| {
        query_turn_content(
            connection,
            &TurnSessionKey {
                environment_key: "native",
                agent: "claude",
                session_id,
            },
            &antiburn_local::analysis::FenceScope::single(1),
        )
        .unwrap()
    });
    let content =
        prepare_session_content(session_id, SourceFormat::ClaudeJsonl, content, Vec::new());
    let selected = select_session_content(&content, IgnoredInstructionsCheck.input_selection());
    let text = selected
        .actions
        .iter()
        .find(|action| action.kind == "tool_input")
        .unwrap();
    assert!(text.reference.stable);
    assert_eq!(text.authority, "assistant");
    assert_eq!(text.text, "git push --force");
    assert!(
        selected
            .actions
            .iter()
            .any(|action| action.kind == "assistant" && action.text == report)
    );
    assert!(!text.truncated);
    let selected_json = serde_json::to_string(&selected).unwrap();
    assert!(!selected_json.contains("PRIVATE_SOURCE_THINKING"));
    assert!(selected_json.contains("OBSERVED_NATIVE_CHECK_FAILURE"));
    let output = selected
        .actions
        .iter()
        .find(|action| action.kind == "tool_result")
        .unwrap();
    assert_eq!(output.authority, "tool");
    assert!(output.metadata.human_text.is_none());
    assert!(
        selected.actions.iter().any(
            |action| action.authority == "user" && action.text == "Review the receiver change."
        )
    );
    assert!(!selected_json.contains("user approval"));
    for provenance in [
        InstructionProvenance::RecordedInjection,
        InstructionProvenance::CurrentFileComparison,
    ] {
        let mut content = selected.clone();
        content.instructions = vec![
            snapshot_from_text(
                "AGENTS.md",
                "Do not run `git push --force`.".to_owned(),
                provenance,
                InstructionScope::Global,
            )
            .unwrap(),
        ];
        let input = AssessmentInput {
            content,
            prior_history_complete: true,
            activity_after_ms: None,
            boundary_positions: BTreeMap::new(),
            source_generation: 1,
            source_fingerprint: None,
            incarnation: 1,
            comparison_after: None,
        };
        let context = build_jev_context(&input).unwrap();
        let plan = IgnoredInstructionsCheck.prepare(&context).unwrap().prepared;
        let fragment = plan
            .comparisons
            .iter()
            .find(|comparison| comparison.reference.action_id == text.reference.id)
            .unwrap();
        assert_eq!(fragment.reference.provenance, provenance);
        assert_eq!(fragment.reference.action_id, text.reference.id);
        assert!(!fragment.action.truncated);
        assert!(
            !fragment
                .source_binding
                .as_ref()
                .unwrap()
                .excerpt
                .as_ref()
                .unwrap()
                .source_truncated
        );
        assert_eq!(
            text.text
                .get(fragment.action_text_start..fragment.action_text_end),
            Some(fragment.action.text.as_str())
        );
    }
}

#[test]
fn native_same_turn_read_order_retains_exact_long_action_ranges() {
    use antiburn_local::analysis::jev::capabilities::ModelCapabilities;
    use antiburn_local::checks::ignored_instructions::{
        SamplingLedger, build_jev_context_with_capabilities,
    };
    let command = format!("{}transfer-request --live", "# recorded note\n".repeat(10));
    let transcript = [
        serde_json::json!({"type":"user","uuid":"native-user","message":{"role":"user","content":"Inspect the transfer adapter."}}),
        serde_json::json!({"type":"assistant","uuid":"native-assistant","parentUuid":"native-user","message":{"role":"assistant","content":[
            {"type":"thinking","thinking":"EXCLUDED_PRIVATE_NATIVE_CONTEXT"},
            {"type":"tool_use","id":"native-read","name":"Read","input":{"file_path":"docs/transfer-order.md"}},
            {"type":"tool_use","id":"native-command","name":"Bash","input":{"command":command}}
        ]}}),
    ].into_iter().map(|record| record.to_string()).collect::<Vec<_>>().join("\n");
    let session_id = "native-prerequisite-characterization";
    let source = SessionInput {
        agent: "claude".to_owned(),
        session_id: session_id.to_owned(),
        source: RawSource::Jsonl(transcript),
        source_format: SourceFormat::ClaudeJsonl,
        fork_parent_session_id: None,
    };
    let store = MemoryTurnRowStore::new("claude", session_id);
    let metrics = SessionMetricsAccumulator::new("claude", session_id);
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".to_owned(),
        session_id: session_id.to_owned(),
        kind: SourceKind::from(&source.source),
        capabilities: capabilities(source.source_format),
    });
    let rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        session_id.to_owned(),
        None,
    );
    let mut sink = CompositeSink::with_turn_rows(metrics, evidence, rows);
    let outcome = reader_for("claude").visit(&source, &mut sink).unwrap();
    sink.observe_source_outcome(outcome);
    let content = store.with_connection(|connection| {
        query_turn_content(
            connection,
            &TurnSessionKey {
                environment_key: "native",
                agent: "claude",
                session_id,
            },
            &antiburn_local::analysis::FenceScope::single(1),
        )
        .unwrap()
    });
    let content = prepare_session_content(
        session_id,
        SourceFormat::ClaudeJsonl,
        content,
        vec![
            snapshot_from_text(
                "AGENTS.md",
                "Read `docs/transfer-order.md` before `transfer-request --live`.".to_owned(),
                InstructionProvenance::RecordedInjection,
                InstructionScope::Project,
            )
            .unwrap(),
        ],
    );
    let input = AssessmentInput {
        content,
        prior_history_complete: true,
        activity_after_ms: None,
        boundary_positions: BTreeMap::new(),
        source_generation: 1,
        source_fingerprint: None,
        incarnation: 1,
        comparison_after: None,
    };
    let context = build_jev_context_with_capabilities(
        &input,
        &SamplingLedger::default(),
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    assert!(
        !context
            .check_context
            .to_string()
            .contains("EXCLUDED_PRIVATE_NATIVE_CONTEXT")
    );
    let plan = IgnoredInstructionsCheck.prepare(&context).unwrap().prepared;
    let selected =
        select_session_content(&input.content, IgnoredInstructionsCheck.input_selection());
    let action = selected
        .actions
        .iter()
        .find(|action| action.tool_call_id.as_deref() == Some("native-command"))
        .unwrap();
    let earlier = selected
        .actions
        .iter()
        .find(|action| action.tool_call_id.as_deref() == Some("native-read"))
        .unwrap();
    assert_eq!(action.reference.turn_index, earlier.reference.turn_index);
    assert!(earlier.reference.part_index < action.reference.part_index);
    let comparisons = plan
        .comparisons
        .iter()
        .filter(|comparison| comparison.reference.action_id == action.reference.id)
        .collect::<Vec<_>>();
    assert_eq!(comparisons.len(), 1);
    assert_eq!(comparisons[0].action_text_start, 0);
    assert_eq!(comparisons[0].action_text_end, action.text.len());
    assert_eq!(comparisons[0].action.text, command);
    for comparison in comparisons {
        let excerpt = comparison
            .source_binding
            .as_ref()
            .unwrap()
            .excerpt
            .as_ref()
            .unwrap();
        assert_eq!(
            action.text.get(excerpt.start_byte..excerpt.end_byte),
            Some(comparison.action.text.as_str())
        );
        assert!(!excerpt.source_truncated);
        let order = plan.read_request_orders[&comparison.id]
            .iter()
            .find(|order| order.required_path == "docs/transfer-order.md")
            .unwrap();
        assert!(order.paths_known);
        assert_eq!(
            order.earlier_request_id.as_deref(),
            Some(earlier.reference.id.as_str())
        );
    }
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            agent: "claude",
            format: SourceFormat::ClaudeJsonl,
            source: RawSource::Jsonl(
                r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"CLAUDE-USER"}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"shell","input":{"cmd":"git push --force"}}]}}
{"type":"user","uuid":"r1","parentUuid":"a1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":"done"}]}}"#.to_owned(),
            ),
            content_marker: "CLAUDE-USER",
        },
        Case {
            agent: "codex",
            format: SourceFormat::CodexRolloutJsonl,
            source: RawSource::Jsonl(
                r#"{"timestamp":"2026-08-01T10:00:00Z","type":"session_meta","payload":{"id":"codex-fixture","cwd":"/work","cli_version":"0.0.0-test","source":"cli"}}
{"timestamp":"2026-08-01T10:00:01Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"CODEX-DEVELOPER"}]}}
{"timestamp":"2026-08-01T10:00:02Z","type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"git push --force\"}","call_id":"codex-call"}}
{"timestamp":"2026-08-01T10:00:03Z","type":"response_item","payload":{"type":"function_call_output","output":"done","call_id":"codex-call"}}"#.to_owned(),
            ),
            content_marker: "CODEX-DEVELOPER",
        },
        Case {
            agent: "pi",
            format: SourceFormat::PiV3Jsonl,
            source: RawSource::Jsonl(
                r#"{"type":"session","version":3,"id":"pi-fixture","timestamp":"2026-01-01T00:00:00Z","cwd":"/work"}
{"type":"message","id":"pi-user","timestamp":"2026-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"text","text":"PI-USER"}]}}
{"type":"message","id":"pi-assistant","parentId":"pi-user","timestamp":"2026-01-01T00:00:02Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"pi-call","name":"shell","arguments":{"cmd":"git push --force"}}]}}
{"type":"message","id":"pi-result","parentId":"pi-assistant","timestamp":"2026-01-01T00:00:03Z","message":{"role":"toolResult","toolCallId":"pi-call","toolName":"shell","content":[{"type":"text","text":"done"}]}}"#.to_owned(),
            ),
            content_marker: "PI-USER",
        },
        Case {
            agent: "opencode",
            format: SourceFormat::OpenCodeSqliteV1,
            source: RawSource::Sqlite(Default::default()),
            content_marker: "OPENCODE-USER",
        },
        Case {
            agent: "cursor",
            format: SourceFormat::CursorCliAgentJsonl,
            source: RawSource::Jsonl(
                r#"{"sessionId":"cursor-fixture","cursor_source":"agent_transcript"}
{"role":"user","message":{"content":[{"type":"text","text":"CURSOR-USER"}]}}
{"role":"assistant","message":{"content":[{"type":"tool-use","id":"cursor-call","name":"shell","input":{"cmd":"git push --force"}}]}}
{"role":"assistant","message":{"content":[{"type":"tool_result","tool_call_id":"cursor-call","tool_name":"shell","content":"done"}]}}"#.to_owned(),
            ),
            content_marker: "CURSOR-USER",
        },
        Case {
            agent: "cursor",
            format: SourceFormat::CursorCliStoreDb,
            source: RawSource::Jsonl(
                concat!(r#"{"role":"user","content":"CURSOR-STORE-USER"}"#, "\n", r#"{"role":"assistant","content":"I will check the project rules."}"#).to_owned(),
            ),
            content_marker: "CURSOR-STORE-USER",
        },
        Case {
            agent: "cursor",
            format: SourceFormat::CursorChatStoreDb,
            source: RawSource::Jsonl(
                concat!(r#"{"role":"user","content":"CURSOR-CHAT-USER"}"#, "\n", r#"{"role":"assistant","content":"I will check the project rules."}"#).to_owned(),
            ),
            content_marker: "CURSOR-CHAT-USER",
        },
        Case {
            agent: "cursor",
            format: SourceFormat::CursorIdeComposer,
            source: RawSource::Jsonl(
                concat!(r#"{"role":"user","content":"CURSOR-COMPOSER-USER"}"#, "\n", r#"{"role":"assistant","content":"I will check the project rules."}"#).to_owned(),
            ),
            content_marker: "CURSOR-COMPOSER-USER",
        },
        Case {
            agent: "antigravity",
            format: SourceFormat::AntigravityBrainJsonl,
            source: RawSource::Jsonl(concat!(
                "{\"type\":\"USER_INPUT\",\"step_index\":1,\"content\":\"ANTIGRAVITY-USER\"}\n",
                "{\"type\":\"PLANNER_RESPONSE\",\"step_index\":2,\"content\":\"Checking the change.\",\"tool_calls\":[{\"name\":\"shell\",\"args\":{\"cmd\":\"git push --force\"}}]}\n"
            ).to_owned()),
            content_marker: "ANTIGRAVITY-USER",
        },
        Case {
            agent: "antigravity",
            format: SourceFormat::AntigravitySqlite,
            source: RawSource::Sqlite(Default::default()),
            content_marker: "ANTIGRAVITY-SQLITE-USER",
        },
    ]
}

#[tokio::test]
async fn native_routes_produce_findings_or_preserve_unsupported_evidence() {
    for (index, case) in cases().into_iter().enumerate() {
        let session_id = format!("ignored-instructions-{index}");
        let sqlite = if matches!(
            case.format,
            SourceFormat::OpenCodeSqliteV1 | SourceFormat::AntigravitySqlite
        ) {
            Some(tempfile::tempdir().expect("synthetic database directory"))
        } else {
            None
        };
        let source = if case.format == SourceFormat::AntigravitySqlite {
            let directory = sqlite.as_ref().expect("Antigravity database directory");
            let conversations = directory.path().join("conversations");
            std::fs::create_dir_all(&conversations).expect("conversation directory");
            let brain = directory
                .path()
                .join("brain/ignored-instructions-9/.system_generated/logs");
            std::fs::create_dir_all(&brain).expect("brain transcript directory");
            std::fs::write(
                brain.join("transcript.jsonl"),
                concat!(
                    "{\"source\":\"antigravity_brain\",\"model\":\"test-model\"}",
                    "\n",
                    r#"{"type":"USER_INPUT","step_index":1,"userInput":{"userResponse":"ANTIGRAVITY-SQLITE-USER"}}"#,
                    "\n",
                    r#"{"type":"PLANNER_RESPONSE","step_index":2,"content":"I will check the project rules.","tool_calls":[{"name":"shell","args":{"command":"git push --force"}}]}"#,
                    "\n"
                ),
            )
            .expect("companion transcript");
            let path = conversations.join("ignored-instructions-9.db");
            let connection = rusqlite::Connection::open(&path).expect("Antigravity database");
            connection
                .execute_batch("PRAGMA user_version = 1; CREATE TABLE steps (idx INTEGER, metadata BLOB); CREATE TABLE gen_metadata (idx INTEGER, data BLOB);")
                .expect("Antigravity schema");
            RawSource::Sqlite(path)
        } else if let Some(directory) = sqlite.as_ref() {
            let path = directory.path().join("opencode.db");
            let connection = rusqlite::Connection::open(&path).expect("synthetic database");
            connection.execute_batch(
                "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, title TEXT, time_created INTEGER, time_updated INTEGER);
                 CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
                 CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
                 INSERT INTO session VALUES ('ignored-instructions-3', NULL, NULL, 1000, 1002);
                 INSERT INTO message VALUES ('m1', 'ignored-instructions-3', 1001, 1001, '{\"role\":\"user\"}');
                 INSERT INTO part VALUES ('p1', 'm1', 'ignored-instructions-3', 1001, 1001, '{\"type\":\"text\",\"text\":\"OPENCODE-USER\"}');
                 INSERT INTO message VALUES ('m2', 'ignored-instructions-3', 1002, 1002, '{\"role\":\"assistant\",\"modelID\":\"model-a\"}');
                  INSERT INTO part VALUES ('p2', 'm2', 'ignored-instructions-3', 1002, 1002, '{\"type\":\"tool\",\"id\":\"oc-call\",\"callID\":\"oc-call\",\"tool\":\"shell\",\"state\":{\"status\":\"completed\",\"input\":{\"cmd\":\"git push --force\"},\"output\":\"done\"}}');",
            ).expect("synthetic OpenCode schema and rows");
            RawSource::Sqlite(path)
        } else {
            case.source
        };
        let input = SessionInput {
            agent: case.agent.to_owned(),
            session_id: session_id.clone(),
            source,
            source_format: case.format,
            fork_parent_session_id: None,
        };
        let store = MemoryTurnRowStore::new(case.agent, &session_id);
        let metrics = SessionMetricsAccumulator::new(case.agent, &session_id);
        let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: case.agent.to_owned(),
            session_id: session_id.clone(),
            kind: SourceKind::from(&input.source),
            capabilities: capabilities(case.format),
        });
        let rows = TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            session_id.clone(),
            None,
        );
        let mut sink = CompositeSink::with_turn_rows(metrics, evidence, rows);
        let reader = reader_for(case.agent);
        let visit = reader
            .visit(&input, &mut sink)
            .unwrap_or_else(|error| panic!("{} fixture visit failed: {error}", case.agent));
        sink.observe_source_outcome(visit);
        assert!(
            !sink.turn_row_write_failed(),
            "{} row write failed",
            case.agent
        );

        let key = TurnSessionKey {
            environment_key: "native",
            agent: case.agent,
            session_id: &session_id,
        };
        let content = store.with_connection(|connection| {
            query_turn_content(
                connection,
                &key,
                &antiburn_local::analysis::FenceScope::single(1),
            )
            .expect("read bounded synthetic content")
        });
        let prepared = prepare_session_content(&session_id, case.format, content, Vec::new());
        assert!(
            prepared
                .actions
                .iter()
                .any(|event| event.text.contains(case.content_marker)),
            "{} must retain its synthetic message through the shared path: {:?}",
            case.agent,
            prepared.actions
        );
        assert_eq!(prepared.publication_fence, 1);
        assert_eq!(prepared.session_identity_digest.len(), 64);
        assert_eq!(prepared.selected_input_digest.len(), 64);
        let mut assessed = prepared.clone();
        assessed.instructions.push(
            snapshot_from_text(
                "AGENTS.md",
                "- Do not run `git push --force`.".to_owned(),
                InstructionProvenance::RecordedInjection,
                InstructionScope::Project,
            )
            .expect("synthetic instruction snapshot"),
        );
        let assessment_input = AssessmentInput {
            content: assessed,
            prior_history_complete: true,
            activity_after_ms: None,
            boundary_positions: BTreeMap::new(),
            source_generation: 1,
            source_fingerprint: None,
            incarnation: 1,
            comparison_after: None,
        };
        let jev_context = build_jev_context(&assessment_input).expect("bounded Jev context");
        let actions = assessment_input.content.actions.clone();
        let check = IgnoredInstructionsCheck;
        let check_plan = check
            .prepare(&jev_context)
            .expect("production check preparation");
        if check_plan.work_items.is_empty() && check_plan.prepared.comparisons.is_empty() {
            assert!(
                matches!(
                    case.format,
                    SourceFormat::CursorCliStoreDb
                        | SourceFormat::CursorCliAgentJsonl
                        | SourceFormat::CursorChatStoreDb
                        | SourceFormat::CursorIdeComposer
                        | SourceFormat::CodexRolloutJsonl
                        | SourceFormat::AntigravityBrainJsonl
                        | SourceFormat::AntigravitySqlite
                ),
                "{}/{:?} has no eligible comparison: {:?}",
                case.agent,
                case.format,
                actions
            );
            assert!(
                actions
                    .iter()
                    .any(|action| action.text.contains(case.content_marker))
            );
            if case.format == SourceFormat::CodexRolloutJsonl {
                let command = actions
                    .iter()
                    .find(|action| action.kind == "tool_input")
                    .expect("Codex retains the parsed tool input");
                assert!(
                    !command.reference.stable,
                    "Codex call has no stable message anchor"
                );
                assert!(command.normalized_fields.as_ref().is_some_and(|fields| {
                    fields
                        .values
                        .values()
                        .any(|value| value.contains("git push --force"))
                }));
            } else if case.format == SourceFormat::CursorCliAgentJsonl {
                let command = actions
                    .iter()
                    .find(|action| action.kind == "tool_input")
                    .expect("Cursor retains the parsed tool input");
                assert!(!command.reference.stable);
                assert!(command.normalized_fields.is_some());
            } else if matches!(
                case.format,
                SourceFormat::AntigravityBrainJsonl | SourceFormat::AntigravitySqlite
            ) {
                let command = actions
                    .iter()
                    .find(|action| action.kind == "tool_input")
                    .expect("Antigravity retains the parsed tool input");
                assert!(command.reference.stable);
                assert!(command.normalized_fields.is_some());
            } else {
                assert!(actions.iter().all(|action| action.kind != "tool_input"));
            }
            continue;
        }
        let comparison_anchors = check_plan
            .prepared
            .comparisons
            .iter()
            .map(|comparison| {
                let action = actions
                    .iter()
                    .find(|action| action.reference.id == comparison.reference.action_id)
                    .unwrap();
                (
                    comparison.reference.action_id.as_str(),
                    comparison.reference.action_stable,
                    action.text.as_str(),
                )
            })
            .collect::<Vec<_>>();
        let request_sizes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded_sizes = Arc::clone(&request_sizes);
        let outcome = run_jev_check(
            &check,
            &jev_context,
            JevRunProgress::default(),
            move |batch| {
                recorded_sizes.lock().unwrap().push(batch.serialized_bytes);
                async move { Ok(synthetic_response(&batch.request, true)) }
            },
            |_| Ok(()),
        )
        .await
        .expect("production request packing, execution, and reduction");
        let request_sizes = request_sizes.lock().unwrap();
        assert!(outcome.complete, "{} assessment completes", case.agent);
        let result = &outcome.result;
        let has_stable_anchor = comparison_anchors.iter().any(|(_, stable, _)| *stable);
        if has_stable_anchor {
            assert!(
                !result.findings.is_empty(),
                "{} produces findings from stable anchors: {:?}",
                case.agent,
                result
            );
        } else {
            assert!(
                result.findings.is_empty(),
                "{} has no stable anchor",
                case.agent
            );
            assert!(
                !result.unassessed_comparisons.is_empty(),
                "{} cannot prove a clean result",
                case.agent
            );
        }
        assert!(
            comparison_anchors.iter().all(|(action_id, _, _)| actions
                .iter()
                .any(|action| action.reference.id == *action_id)),
            "candidate comparisons retain their selected action"
        );
        assert!(
            request_sizes
                .iter()
                .all(|size| { *size <= antiburn_local::analysis::MAX_REQUEST_BYTES })
        );
        assert_eq!(result.request_count as usize, request_sizes.len());
        assert_eq!(result.input_tokens, (request_sizes.len() * 20) as u64);
        assert!(result.findings.iter().all(|finding| {
            finding.decision_record().is_some()
                && prepared
                    .actions
                    .iter()
                    .any(|action| action.reference.id == finding.reference.action_id)
        }));
        let selected = select_session_content(&prepared, check.input_selection());
        assert!(
            selected
                .actions
                .iter()
                .filter(|event| event.kind == "tool_result")
                .all(|event| event.authority == "tool"
                    && event.metadata.human_text.is_none()
                    && event.tool_name.as_ref().is_some_and(
                        |name| ["bash", "shell"].contains(&name.to_ascii_lowercase().as_str())
                    ))
        );
        if case.agent == "codex" {
            assert!(
                prepared
                    .actions
                    .iter()
                    .any(|event| event.authority == "developer")
            );
            assert!(
                prepared
                    .actions
                    .iter()
                    .any(|event| event.tool_call_id.as_deref() == Some("codex-call"))
            );
        }
        if case.agent == "pi" {
            assert!(
                prepared
                    .actions
                    .iter()
                    .any(|event| event.tool_call_id.as_deref() == Some("pi-call"))
            );
        }
        if case.agent == "opencode" {
            assert!(
                prepared
                    .actions
                    .iter()
                    .any(|event| event.tool_call_id.as_deref() == Some("oc-call"))
            );
        }
        if case.agent == "antigravity" {
            assert!(
                prepared
                    .actions
                    .iter()
                    .any(|event| event.tool_name.as_deref() == Some("shell"))
            );
            assert_eq!(
                prepared.excluded_thinking_parts,
                u32::from(case.format == SourceFormat::AntigravityBrainJsonl)
            );
        }
    }
}

#[tokio::test]
async fn held_out_production_cases_separate_a_conflict_from_compliant_content() {
    let cases = [
        (
            "conflict",
            "Do not run `git push --force` to the remote repository.",
            "git push --force",
            true,
        ),
        (
            "compliant",
            "Run focused tests before committing.",
            "cargo test -p antiburn-local --test ignored_instruction_evidence",
            false,
        ),
        (
            "unanchored",
            "Do not run `git push --force`.",
            "git push --force",
            false,
        ),
    ];
    let started_at = Instant::now();
    let mut evaluation = (0_u64, 0_u64);
    let mut candidate_pairs = 0_usize;
    let mut selected_pairs = 0_usize;
    let mut omitted_pairs = 0_usize;
    let mut findings = 0_usize;
    let mut false_findings = 0_usize;
    let mut missed_findings = 0_usize;
    for (id, rule, action_text, conflict) in cases {
        let instruction = snapshot_from_text(
            "AGENTS.md",
            format!("- {rule}"),
            InstructionProvenance::RecordedInjection,
            InstructionScope::Project,
        )
        .expect("held-out instruction");
        let content = SessionContentEvidence {
            session_identity_digest: format!("session-{id}"),
            source_format: SourceFormat::ClaudeJsonl,
            publication_fence: 1,
            selected_input_digest: format!("input-{id}"),
            actions: vec![tool_input_action(id, action_text, id != "unanchored")],
            instructions: vec![instruction],
            complete: true,
            limitations: Vec::new(),
            excluded_thinking_parts: 0,
            field_availability: Vec::new(),
        };
        let context = build_jev_context(&AssessmentInput {
            content,
            prior_history_complete: true,
            activity_after_ms: None,
            boundary_positions: BTreeMap::new(),
            source_generation: 1,
            source_fingerprint: Some(format!("source-{id}")),
            incarnation: 1,
            comparison_after: None,
        })
        .expect("production Jev context");
        let request_sizes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded_sizes = Arc::clone(&request_sizes);
        let check = IgnoredInstructionsCheck;
        let outcome = run_jev_check(
            &check,
            &context,
            JevRunProgress::default(),
            move |batch| {
                recorded_sizes.lock().unwrap().push(batch.serialized_bytes);
                async move { Ok(synthetic_response(&batch.request, conflict)) }
            },
            |_| Ok(()),
        )
        .await
        .expect("production assessment");
        assert!(outcome.complete);
        let request_sizes = request_sizes.lock().unwrap();
        assert!(
            request_sizes
                .iter()
                .all(|size| { *size <= antiburn_local::analysis::MAX_REQUEST_BYTES })
        );
        assert_eq!(outcome.result.request_count as usize, request_sizes.len());
        assert_eq!(
            outcome.result.input_tokens,
            (request_sizes.len() * 20) as u64
        );
        evaluation.0 = evaluation.0.saturating_add(outcome.result.input_tokens);
        evaluation.1 = evaluation
            .1
            .saturating_add(u64::from(outcome.result.request_count));
        candidate_pairs = candidate_pairs.saturating_add(outcome.result.coverage.candidate_pairs);
        selected_pairs =
            selected_pairs.saturating_add(outcome.result.coverage.selected_comparisons);
        omitted_pairs = omitted_pairs.saturating_add(outcome.result.coverage.unselected_pairs);
        findings = findings.saturating_add(outcome.result.findings.len());
        if conflict {
            let found_expected = outcome
                .result
                .findings
                .iter()
                .any(|finding| finding.reference.action_id == format!("action-{id}"));
            missed_findings = missed_findings.saturating_add(usize::from(!found_expected));
            assert!(
                found_expected,
                "{id} conflict finding cites its action: {:#?}",
                outcome.result
            );
            assert_eq!(outcome.result.findings.len(), 1);
            assert_eq!(
                outcome.result.findings[0].reference.action_id,
                format!("action-{id}")
            );
        } else {
            false_findings = false_findings.saturating_add(outcome.result.findings.len());
            assert!(outcome.result.findings.is_empty());
            if id == "unanchored" {
                assert!(
                    request_sizes.is_empty(),
                    "unstable action must not dispatch"
                );
                assert_eq!(outcome.result.coverage.candidate_pairs, 0);
                assert_eq!(outcome.result.coverage.selected_comparisons, 0);
                assert_eq!(outcome.result.coverage.unselected_pairs, 0);
            } else if id == "compliant" {
                assert!(
                    request_sizes.is_empty(),
                    "unanchored compliant work must not dispatch"
                );
                assert_eq!(outcome.result.coverage.candidate_pairs, 1);
                assert_eq!(outcome.result.coverage.selected_comparisons, 0);
                assert_eq!(outcome.result.coverage.unselected_pairs, 1);
            } else {
                assert_eq!(outcome.result.coverage.candidate_pairs, 1);
                assert_eq!(outcome.result.coverage.selected_comparisons, 1);
                assert_eq!(outcome.result.coverage.unselected_pairs, 0);
            }
        }
    }
    assert_eq!(evaluation, (20, 1));
    assert_eq!((candidate_pairs, selected_pairs, omitted_pairs), (2, 1, 1));
    assert_eq!((findings, false_findings, missed_findings), (1, 0, 0));
    let estimated_cost_nanos = evaluation.0.saturating_mul(42);
    assert_eq!(estimated_cost_nanos, 840);
    eprintln!(
        "held-out Ignored Instructions: candidates={candidate_pairs}, selected={selected_pairs}, omitted={omitted_pairs}, findings={findings}, false_findings={false_findings}, misses={missed_findings}, provider_calls={}, cache_hits=0, input_tokens={}, estimated_usd=0.00000168, elapsed_ms={}",
        evaluation.1,
        evaluation.0,
        started_at.elapsed().as_millis(),
    );
}

fn tool_input_action(id: &str, command: &str, stable: bool) -> ContentAction {
    ContentAction {
        reference: ContentEventReference {
            id: format!("action-{id}"),
            source_key_digest: "synthetic-source".to_owned(),
            thread_digest: "main-thread".to_owned(),
            turn_index: 1,
            native_record_id: Some(format!("record-{id}")),
            part_index: 0,
            stable,
        },
        timestamp_ms: Some(1),
        turn_role: "assistant".to_owned(),
        turn_scope: "main".to_owned(),
        authority: "assistant".to_owned(),
        kind: "tool_input".to_owned(),
        text: command.to_owned(),
        tool_name: Some("bash".to_owned()),
        tool_call_id: Some(format!("call-{id}")),
        normalized_fields: Some(JevNormalizedFields {
            category: Some(JevNormalizedCategory::BashCommand),
            values: [(JevInputField::BashCommandInput, command.to_owned())].into(),
            malformed: false,
        }),
        metadata: Default::default(),
        truncated: false,
        context_only: false,
    }
}

fn synthetic_response(request: &JevRequest, conflict: bool) -> JevResponse {
    let answers = request
        .questions
        .iter()
        .map(|(question_id, question)| {
            let JevQuestion::Choice { criteria, .. } = question else {
                panic!("production questions use the typed choice contract")
            };
            assert!(!criteria.is_empty());
            let selected = if conflict { "conflict" } else { "no_issue" }.to_owned();
            assert!(criteria.contains_key(&selected));
            let other_probability = 0.01 / criteria.len().saturating_sub(1).max(1) as f64;
            let probabilities = criteria
                .keys()
                .map(|choice| {
                    (
                        choice.clone(),
                        if choice == &selected {
                            0.99
                        } else {
                            other_probability
                        },
                    )
                })
                .collect();
            (
                question_id.clone(),
                JevAnswer::Choice {
                    choice: selected,
                    probabilities,
                    confidence: 0.99,
                },
            )
        })
        .collect();
    JevResponse {
        model: request.model.clone(),
        answers,
        usage: JevUsage {
            input_tokens: 20,
            output_tokens: 2,
        },
    }
}

fn capabilities(format: SourceFormat) -> SourceCapabilities {
    let mut capabilities = match format {
        SourceFormat::ClaudeJsonl => SourceCapabilities::claude(),
        SourceFormat::CodexRolloutJsonl => SourceCapabilities::codex(),
        SourceFormat::PiV3Jsonl => SourceCapabilities::pi(),
        SourceFormat::OpenCodeSqliteV1 => SourceCapabilities {
            source_format: SourceFormat::OpenCodeSqliteV1,
            ..SourceCapabilities::opencode()
        },
        SourceFormat::CursorCliAgentJsonl => SourceCapabilities::cursor(),
        SourceFormat::CursorCliStoreDb
        | SourceFormat::CursorChatStoreDb
        | SourceFormat::CursorIdeComposer => SourceCapabilities::cursor(),
        SourceFormat::AntigravityBrainJsonl => SourceCapabilities::antigravity(),
        SourceFormat::AntigravitySqlite => SourceCapabilities::antigravity(),
        _ => unreachable!("test only uses supported Ignored Instructions formats"),
    };
    capabilities.source_format = format;
    capabilities
}
