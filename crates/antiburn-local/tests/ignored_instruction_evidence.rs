use std::sync::Arc;

use antiburn_local::analysis::ignored_instructions::prepare_session_content;
use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, MemoryTurnRowStore, RawSource, SessionEvidenceAccumulator,
    SessionInput, SessionMetricsAccumulator, SourceCapabilities, SourceFormat, SourceKind,
    TurnRowSink, TurnRowStore, TurnSessionKey, query_turn_content, reader_for,
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
            format: SourceFormat::OpenCodeJsonl,
            source: RawSource::Jsonl(
                r#"{"type":"session_meta","sessionID":"opencode-fixture","sessionRole":"root","time":{"created":1000},"payload":{"id":"opencode-fixture"}}
{"type":"message","rootSessionID":"opencode-fixture","sessionID":"opencode-fixture","sessionRole":"root","messageID":"m1","time":{"created":1001},"payload":{"role":"user"}}
{"type":"part","messageID":"m1","time":{"created":1001},"payload":{"type":"text","text":"OPENCODE-USER"}}
{"type":"message","rootSessionID":"opencode-fixture","sessionID":"opencode-fixture","sessionRole":"root","messageID":"m2","time":{"created":1002},"payload":{"role":"assistant","modelID":"model-a"}}
{"type":"part","messageID":"m2","time":{"created":1002},"payload":{"type":"tool","id":"oc-call","callID":"oc-call","tool":"shell","state":{"status":"completed","input":{"cmd":"true"},"output":"done"}}}"#.to_owned(),
            ),
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
            agent: "antigravity",
            format: SourceFormat::AntigravityBrainJsonl,
            source: RawSource::Jsonl(include_str!("fixtures/antigravity_characterization/ignored_instructions_content.jsonl").to_owned()),
            content_marker: "ANTIGRAVITY-USER",
        },
    ]
}

#[test]
fn all_six_first_tier_agents_use_the_same_normalized_preparation() {
    for (index, case) in cases().into_iter().enumerate() {
        let session_id = format!("ignored-instructions-{index}");
        let input = SessionInput {
            agent: case.agent.to_owned(),
            session_id: session_id.clone(),
            source: case.source,
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
            assert!(
                prepared
                    .actions
                    .iter()
                    .any(|event| event.text.contains("test result: ok"))
            );
            assert_eq!(prepared.excluded_thinking_parts, 1);
        }
    }
}

fn capabilities(format: SourceFormat) -> SourceCapabilities {
    match format {
        SourceFormat::ClaudeJsonl => SourceCapabilities::claude(),
        SourceFormat::CodexRolloutJsonl => SourceCapabilities::codex(),
        SourceFormat::PiV3Jsonl => SourceCapabilities::pi(),
        SourceFormat::OpenCodeJsonl => SourceCapabilities::opencode(),
        SourceFormat::CursorCliAgentJsonl => SourceCapabilities::cursor(),
        SourceFormat::AntigravityBrainJsonl => SourceCapabilities::antigravity(),
        _ => unreachable!("test only uses the six first-tier source formats"),
    }
}
