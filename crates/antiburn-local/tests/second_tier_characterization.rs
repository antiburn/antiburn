use std::path::{Path, PathBuf};
use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, EvidenceCoverage, EvidenceSource, EvidenceValue, MemoryTurnRowStore, RawSource,
    SessionEvidence, SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator,
    SourceCapabilities, SourceFormat, SourceKind, TurnRowSink, TurnRowStore, reader_for_input,
};
use antiburn_local::insights::{DetectorId, clean_facts_complete, eligible};
use rusqlite::{Connection, params};
use tempfile::TempDir;

const COPILOT_SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";
const COPILOT_EVENTS: &str = include_str!("fixtures/source_contracts/copilot_interleaved.jsonl");
const CLINE_MANIFEST: &str = include_str!("fixtures/cline_messages_contract_v1/root.json");
const CLINE_ROOT_MESSAGES: &str =
    include_str!("fixtures/cline_messages_contract_v1/root.messages.json");
const CLINE_CHILD_MESSAGES: &str =
    include_str!("fixtures/cline_messages_contract_v1/child.messages.json");
const KIRO_METADATA: &str = include_str!("fixtures/kiro_cli_v2_bundle/root.json");
const KIRO_MESSAGES: &str = include_str!("fixtures/kiro_cli_v2_bundle/root.jsonl");
const AMP_EXPORT: &str = include_str!("fixtures/source_contracts/amp_export_v39.json");
const DEVIN_SCHEMA: &str = include_str!("fixtures/devin_local_migration_17.sql");

fn evidence(input: &SessionInput) -> anyhow::Result<SessionEvidence> {
    let reader = reader_for_input(input);
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
            input.session_id.clone(),
            None,
        ),
    );
    let outcome = reader.visit(input, &mut sink)?;
    sink.observe_source_outcome(outcome);
    sink.evidence()
        .ok_or_else(|| anyhow::anyhow!("reader did not publish evidence"))
}

fn json_input(agent: &str, session_id: &str, format: SourceFormat, content: &str) -> SessionInput {
    SessionInput {
        agent: agent.to_owned(),
        session_id: session_id.to_owned(),
        source: RawSource::Jsonl(content.to_owned()),
        source_format: format,
        fork_parent_session_id: None,
    }
}

fn copilot_bundle() -> (TempDir, SessionInput) {
    let temp = TempDir::new().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let db_path = temp.path().join("session-store.db");
    std::fs::write(&events_path, COPILOT_EVENTS).unwrap();
    let connection = Connection::open(&db_path).unwrap();
    connection
        .execute_batch(
            "PRAGMA user_version = 7;
             CREATE TABLE sessions (session_id TEXT, shutdown_model TEXT);
             CREATE TABLE request_usage (
                 session_id TEXT, request_id TEXT, agent_id TEXT,
                 parent_tool_call_id TEXT, model TEXT,
                 input_tokens INTEGER, output_tokens INTEGER,
                 cache_read_tokens INTEGER, cache_write_tokens INTEGER
             );",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO sessions VALUES (?1, 'synthetic-main')",
            [COPILOT_SESSION_ID],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO request_usage VALUES (?1, 'request-1', NULL, NULL, 'synthetic-main', 10, 5, 2, 1)",
            [COPILOT_SESSION_ID],
        )
        .unwrap();
    drop(connection);
    (
        temp,
        SessionInput {
            agent: "copilot".to_owned(),
            session_id: COPILOT_SESSION_ID.to_owned(),
            source: RawSource::CopilotCliBundle {
                events_path,
                db_path,
            },
            source_format: SourceFormat::CopilotCliJsonl,
            fork_parent_session_id: None,
        },
    )
}

fn cline_bundle(include_child: bool) -> (TempDir, SessionInput) {
    let temp = TempDir::new().unwrap();
    let directory = temp.path().join("data/tasks/root_1");
    std::fs::create_dir_all(&directory).unwrap();
    let manifest_path = directory.join("root_1.json");
    let messages_path = directory.join("root_1.messages.json");
    std::fs::write(&messages_path, CLINE_ROOT_MESSAGES).unwrap();
    std::fs::write(
        &manifest_path,
        CLINE_MANIFEST.replace(
            "\"PLACEHOLDER\"",
            &serde_json::to_string(&messages_path.to_string_lossy()).unwrap(),
        ),
    )
    .unwrap();
    let child_path = directory.join("child_agent.messages.json");
    if include_child {
        std::fs::write(&child_path, CLINE_CHILD_MESSAGES).unwrap();
    }
    let db_path = temp.path().join("data/db/sessions.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let connection = Connection::open(&db_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE sessions (
                session_id TEXT, status TEXT, model TEXT, agent_id TEXT,
                parent_session_id TEXT, is_subagent INTEGER, messages_path TEXT
            );",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO sessions VALUES (?1, 'completed', 'model-root', 'lead', NULL, 0, ?2)",
            params!["root_1", messages_path.to_string_lossy()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO sessions VALUES ('child_1', 'completed', 'model-child', 'child_agent', 'root_1', 1, ?1)",
            params![child_path.to_string_lossy()],
        )
        .unwrap();
    drop(connection);
    (
        temp,
        SessionInput {
            agent: "cline".to_owned(),
            session_id: "root_1".to_owned(),
            source: RawSource::ClineBundle {
                db_path,
                manifest_path,
                messages_path,
            },
            source_format: SourceFormat::ClineMessagesContractV1,
            fork_parent_session_id: None,
        },
    )
}

fn kiro_bundle(messages: &str) -> (TempDir, SessionInput) {
    let temp = TempDir::new().unwrap();
    let metadata_path = temp.path().join(format!("{COPILOT_SESSION_ID}.json"));
    let messages_path = temp.path().join(format!("{COPILOT_SESSION_ID}.jsonl"));
    std::fs::write(&metadata_path, KIRO_METADATA).unwrap();
    std::fs::write(&messages_path, messages).unwrap();
    (
        temp,
        SessionInput {
            agent: "kiro".to_owned(),
            session_id: COPILOT_SESSION_ID.to_owned(),
            source: RawSource::KiroCliV2Bundle {
                metadata_path,
                messages_path,
            },
            source_format: SourceFormat::KiroCliV2Bundle,
            fork_parent_session_id: None,
        },
    )
}

fn devin_database() -> (TempDir, PathBuf) {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("sessions.db");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch(DEVIN_SCHEMA).unwrap();
    drop(connection);
    (temp, path)
}

fn devin_input(path: &Path) -> SessionInput {
    SessionInput {
        agent: "windsurf".to_owned(),
        session_id: "root".to_owned(),
        source: RawSource::Sqlite(path.to_owned()),
        source_format: SourceFormat::DevinLocalSqlite,
        fork_parent_session_id: None,
    }
}

fn assert_detector_contract(
    evidence: &SessionEvidence,
    eligible_detectors: &[DetectorId],
    clean_detectors: &[DetectorId],
) {
    for detector in DetectorId::ALL {
        assert_eq!(
            eligible(detector, evidence),
            eligible_detectors.contains(&detector),
            "eligibility for {:?}/{detector:?}",
            evidence.capabilities.source_format
        );
        assert_eq!(
            clean_facts_complete(detector, evidence),
            clean_detectors.contains(&detector),
            "clean limit for {:?}/{detector:?}",
            evidence.capabilities.source_format
        );
    }
}

#[test]
fn characterized_sources_pin_parsed_evidence_and_detector_limits() {
    let (_temp, input) = copilot_bundle();
    let copilot = evidence(&input).unwrap();
    assert_eq!(copilot.coverage, EvidenceCoverage::Complete);
    assert!(matches!(
        &copilot.models,
        EvidenceValue::Complete(models) if models.by_model.contains_key("synthetic-main")
    ));
    assert!(matches!(
        &copilot.subagents,
        EvidenceValue::Complete(subagents)
            if subagents.spawn_count == 1
                && subagents.children[0]
                    .observed_child_models
                    .contains("synthetic-requested")
    ));
    assert_detector_contract(
        &copilot,
        &[DetectorId::OverpoweredSubagents, DetectorId::OldModelUsage],
        &[DetectorId::OverpoweredSubagents, DetectorId::OldModelUsage],
    );

    let (_temp, input) = cline_bundle(true);
    let cline = evidence(&input).unwrap();
    assert_eq!(cline.coverage, EvidenceCoverage::Complete);
    assert!(matches!(
        &cline.subagents,
        EvidenceValue::Complete(subagents)
            if subagents.spawn_count == 1 && subagents.delegated_models.contains("model-child")
    ));
    assert_detector_contract(
        &cline,
        &[DetectorId::OverpoweredSubagents, DetectorId::OldModelUsage],
        &[],
    );

    let (_temp, input) = kiro_bundle(KIRO_MESSAGES);
    let kiro = evidence(&input).unwrap();
    assert_eq!(kiro.coverage, EvidenceCoverage::Complete);
    assert!(matches!(&kiro.models, EvidenceValue::Unsupported));
    assert_detector_contract(&kiro, &[], &[]);

    let amp = evidence(&json_input(
        "amp-code",
        "T-synthetic-39",
        SourceFormat::AmpThreadJson,
        AMP_EXPORT,
    ))
    .unwrap();
    assert_eq!(amp.coverage, EvidenceCoverage::Complete);
    assert!(matches!(
        &amp.context,
        EvidenceValue::Complete(context) if context.max_request_context_tokens == 14
    ));
    assert!(matches!(
        &amp.models,
        EvidenceValue::Complete(models) if models.by_model.contains_key("synthetic-model")
    ));
    assert_detector_contract(
        &amp,
        &[DetectorId::SessionsOverDepth, DetectorId::OldModelUsage],
        &[],
    );

    let (_temp, path) = devin_database();
    let devin = evidence(&devin_input(&path)).unwrap();
    assert_eq!(devin.coverage, EvidenceCoverage::Complete);
    assert!(matches!(
        &devin.subagents,
        EvidenceValue::Complete(subagents)
            if subagents.spawn_count == 1
                && subagents.children[0]
                    .observed_child_models
                    .contains("claude-opus-4-7-20260115")
    ));
    assert_detector_contract(&devin, &[DetectorId::OverpoweredSubagents], &[]);
}

#[test]
fn characterized_sources_reject_malformed_or_incomplete_input() {
    let malformed_copilot =
        include_str!("fixtures/copilot_characterization/malformed_partial.jsonl");
    let copilot = evidence(&json_input(
        "copilot",
        COPILOT_SESSION_ID,
        SourceFormat::CopilotCliJsonl,
        malformed_copilot,
    ))
    .unwrap();
    assert!(matches!(copilot.coverage, EvidenceCoverage::Partial(_)));
    for detector in DetectorId::ALL {
        assert!(!clean_facts_complete(detector, &copilot));
    }

    let (_temp, input) = cline_bundle(false);
    let cline = evidence(&input).unwrap();
    assert!(matches!(cline.coverage, EvidenceCoverage::Partial(_)));

    let (_temp, input) = kiro_bundle("{\"version\":\"v1\",\"kind\":\"AssistantMessage\"");
    let kiro = evidence(&input).unwrap();
    assert!(matches!(kiro.coverage, EvidenceCoverage::Partial(_)));

    let malformed_amp = AMP_EXPORT.replace("\"totalInputTokens\":14", "\"totalInputTokens\":15");
    assert!(
        evidence(&json_input(
            "amp-code",
            "T-synthetic-39",
            SourceFormat::AmpThreadJson,
            &malformed_amp,
        ))
        .is_err()
    );

    let (_temp, path) = devin_database();
    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE message_nodes SET raw_message = '{' WHERE session_id = 'root' AND node_id = 2",
            [],
        )
        .unwrap();
    let devin = evidence(&devin_input(&path)).unwrap();
    assert!(matches!(devin.coverage, EvidenceCoverage::Partial(_)));
    assert!(eligible(DetectorId::OverpoweredSubagents, &devin));
    assert!(!clean_facts_complete(
        DetectorId::OverpoweredSubagents,
        &devin
    ));
}

#[test]
fn every_fail_closed_second_tier_format_uses_its_production_reader() {
    let cases = [
        ("copilot", SourceFormat::CopilotIdeChatJson, false, false),
        ("cline", SourceFormat::ClineSessionJson, true, false),
        ("kiro", SourceFormat::KiroSessionJson, false, true),
        ("kiro", SourceFormat::KiroChat, false, true),
        ("kiro", SourceFormat::KiroCliV3Bundle, false, true),
        ("kiro", SourceFormat::KiroChatSaveExport, false, true),
        ("amp-code", SourceFormat::AmpFileChanges, true, false),
        (
            "windsurf",
            SourceFormat::WindsurfWorkspaceJson,
            false,
            false,
        ),
        ("windsurf", SourceFormat::WindsurfMirrorJson, false, false),
        (
            "windsurf",
            SourceFormat::WindsurfCascadeProtobuf,
            false,
            false,
        ),
    ];
    for (agent, format, visit_errors, partial) in cases {
        let input = json_input(
            agent,
            "fail-closed",
            format,
            "{\"role\":\"assistant\",\"model\":\"gpt-5\",\"timestamp\":1000}",
        );
        let reader = reader_for_input(&input);
        assert_eq!(
            reader.capabilities(&input),
            SourceCapabilities::uncharacterized(format),
            "{format:?}"
        );
        let result = evidence(&input);
        assert_eq!(result.is_err(), visit_errors, "{format:?}");
        if let Ok(evidence) = result {
            assert_eq!(
                matches!(evidence.coverage, EvidenceCoverage::Partial(_)),
                partial,
                "{format:?}"
            );
            assert_detector_contract(&evidence, &[], &[]);
        }
    }
}
