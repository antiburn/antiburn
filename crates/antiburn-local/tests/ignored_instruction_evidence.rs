use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use antiburn_local::analysis::ignored_instructions::{
    AssessmentInput, ContentAction, ContentEventReference, IgnoredInstructionsCheck,
    InstructionProvenance, InstructionScope, SessionContentEvidence, build_jev_context,
    prepare_session_content, select_session_content, snapshot_from_text,
};
use antiburn_local::analysis::jev::{JevRunProgress, run_jev_check};
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

fn cases() -> Vec<Case> {
    vec![
        Case {
            agent: "claude",
            format: SourceFormat::ClaudeJsonl,
            source: RawSource::Jsonl(
                r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"CLAUDE-USER"}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"shell","input":{"cmd":"true"}}]}}
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
{"timestamp":"2026-08-01T10:00:02Z","type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"true\"}","call_id":"codex-call"}}
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
{"type":"message","id":"pi-assistant","parentId":"pi-user","timestamp":"2026-01-01T00:00:02Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"pi-call","name":"shell","arguments":{"cmd":"true"}}]}}
{"type":"message","id":"pi-result","parentId":"pi-assistant","timestamp":"2026-01-01T00:00:03Z","message":{"role":"toolResult","toolCallId":"pi-call","toolName":"shell","content":[{"type":"text","text":"done"}]}}"#.to_owned(),
            ),
            content_marker: "PI-USER",
        },
        Case {
            agent: "opencode",
            format: SourceFormat::OpenCodeSqliteV2,
            source: RawSource::Sqlite(Default::default()),
            content_marker: "OPENCODE-USER",
        },
        Case {
            agent: "cursor",
            format: SourceFormat::CursorCliAgentJsonl,
            source: RawSource::Jsonl(
                r#"{"sessionId":"cursor-fixture","cursor_source":"agent_transcript"}
{"role":"user","message":{"content":[{"type":"text","text":"CURSOR-USER"}]}}
{"role":"assistant","message":{"content":[{"type":"tool-use","id":"cursor-call","name":"shell","input":{"cmd":"true"}}]}}
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
            source: RawSource::Jsonl(include_str!("fixtures/antigravity_characterization/ignored_instructions_content.jsonl").to_owned()),
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
async fn supported_native_routes_produce_findings_through_the_production_runner() {
    for (index, case) in cases().into_iter().enumerate() {
        let session_id = format!("ignored-instructions-{index}");
        let sqlite = if matches!(
            case.format,
            SourceFormat::OpenCodeSqliteV2 | SourceFormat::AntigravitySqlite
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
                    r#"{"type":"PLANNER_RESPONSE","step_index":2,"content":"I will check the project rules.","tool_calls":[{"name":"shell","args":{"command":"true"}}]}"#,
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
                 INSERT INTO part VALUES ('p2', 'm2', 'ignored-instructions-3', 1002, 1002, '{\"type\":\"tool\",\"id\":\"oc-call\",\"callID\":\"oc-call\",\"tool\":\"shell\",\"state\":{\"status\":\"completed\",\"input\":{\"cmd\":\"true\"},\"output\":\"done\"}}');",
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
                "- Do not use the shell tool.".to_owned(),
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
        assert!(
            !check_plan.work_items.is_empty(),
            "{}/{:?} selects actions: {:?}",
            case.agent,
            case.format,
            actions
        );
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
        assert!(
            !result.findings.is_empty(),
            "{} produces findings: {:?}",
            case.agent,
            result
        );
        assert!(
            request_sizes
                .iter()
                .all(|size| { *size <= antiburn_local::analysis::MAX_REQUEST_BYTES })
        );
        assert_eq!(result.request_count as usize, request_sizes.len());
        assert_eq!(result.input_tokens, (request_sizes.len() * 20) as u64);
        assert!(result.findings.iter().all(|finding| {
            prepared
                .actions
                .iter()
                .any(|action| action.reference.id == finding.reference.action_id)
        }));
        let selected = select_session_content(&prepared, check.input_selection());
        assert!(
            selected
                .actions
                .iter()
                .all(|event| event.kind != "tool_result")
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
            "Do not force push to the remote repository.",
            "I used git push --force to update the remote.",
            true,
        ),
        (
            "compliant",
            "Run focused tests before committing.",
            "I ran the focused tests, then committed the changes.",
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
            actions: vec![ContentAction {
                reference: ContentEventReference {
                    id: format!("action-{id}"),
                    source_key_digest: "synthetic-source".to_owned(),
                    thread_digest: "main-thread".to_owned(),
                    turn_index: 1,
                    native_record_id: Some(format!("record-{id}")),
                    part_index: 0,
                    stable: true,
                },
                timestamp_ms: Some(1),
                turn_role: "assistant".to_owned(),
                turn_scope: "main".to_owned(),
                authority: "agent".to_owned(),
                kind: "assistant_text".to_owned(),
                text: action_text.to_owned(),
                tool_name: None,
                tool_call_id: None,
                normalized_fields: None,
                metadata: Default::default(),
                truncated: false,
                context_only: false,
            }],
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
            assert!(found_expected, "{id} conflict finding cites its action");
            assert_eq!(outcome.result.findings.len(), 1);
            assert_eq!(
                outcome.result.findings[0].reference.action_id,
                format!("action-{id}")
            );
        } else {
            false_findings = false_findings.saturating_add(outcome.result.findings.len());
            assert!(outcome.result.findings.is_empty());
        }
    }
    assert_eq!(evaluation, (120, 6));
    assert_eq!((candidate_pairs, selected_pairs, omitted_pairs), (2, 2, 0));
    assert_eq!((findings, false_findings, missed_findings), (1, 0, 0));
    let estimated_cost_nanos = evaluation.0.saturating_mul(42);
    assert_eq!(estimated_cost_nanos, 5_040);
    eprintln!(
        "held-out Ignored Instructions: candidates={candidate_pairs}, selected={selected_pairs}, omitted={omitted_pairs}, findings={findings}, false_findings={false_findings}, misses={missed_findings}, provider_calls={}, cache_hits=0, input_tokens={}, estimated_usd=0.00000504, elapsed_ms={}",
        evaluation.1,
        evaluation.0,
        started_at.elapsed().as_millis(),
    );
}

fn synthetic_response(request: &JevRequest, conflict: bool) -> JevResponse {
    let answers = request
        .questions
        .iter()
        .map(|(question_id, question)| {
            let JevQuestion::Choice { criteria, .. } = question else {
                panic!("production questions use the typed choice contract")
            };
            let selected = if criteria.contains_key("not_read_rule") {
                "not_read_rule"
            } else if criteria.contains_key("unqualified") {
                "qualified"
            } else if criteria.contains_key("literal_other") {
                "literal_other"
            } else if criteria.contains_key("selected") {
                "selected"
            } else if criteria.contains_key("independent") {
                "independent"
            } else if criteria.contains_key("not_read_order") {
                "not_read_order"
            } else if criteria.contains_key("other_path") {
                "other_path"
            } else if criteria.contains_key("any") {
                "any"
            } else if criteria.contains_key("prerequisite") {
                "prerequisite"
            } else if criteria.contains_key("reports_addition") {
                if conflict {
                    "reports_addition"
                } else {
                    "no_addition_report"
                }
            } else if criteria.contains_key("conflict") {
                if conflict { "conflict" } else { "follows" }
            } else if criteria.contains_key("conflicting_action") {
                if conflict {
                    "conflicting_action"
                } else {
                    "no_conflict"
                }
            } else if criteria.contains_key("self_contained") {
                "self_contained"
            } else if criteria.contains_key("applies") {
                "applies"
            } else {
                criteria
                    .keys()
                    .next()
                    .map(String::as_str)
                    .expect("choice questions have criteria")
            }
            .to_owned();
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
        SourceFormat::OpenCodeSqliteV2 => SourceCapabilities {
            source_format: SourceFormat::OpenCodeSqliteV2,
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
