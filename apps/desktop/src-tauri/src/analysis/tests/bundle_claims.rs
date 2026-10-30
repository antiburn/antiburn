use super::*;
use antiburn_local::discovery::source_version::bundle_fingerprint;
use antiburn_local::discovery::{SourceDescriptor, session_log_read};
use std::cell::Cell;
use std::io::Write;
use std::path::{Path, PathBuf};

const ROOT_ID: &str = "11111111-1111-4111-8111-111111111111";

struct BundleFixture {
    agent: AgentKind,
    session_id: &'static str,
    format: SourceFormat,
    path: PathBuf,
    messages_path: PathBuf,
}

impl BundleFixture {
    fn source(&self) -> SessionSource {
        SessionSource::File(self.path.clone())
    }

    async fn input(&self) -> SessionInput {
        let source = self.source();
        assert_eq!(source_format(self.agent, &source), self.format);
        let raw = raw_source(self.agent, &source).await.unwrap();
        assert!(matches!(
            (&raw, self.format),
            (
                RawSource::CopilotCliBundle { .. },
                SourceFormat::CopilotCliJsonl
            ) | (
                RawSource::ClineBundle { .. },
                SourceFormat::ClineMessagesContractV1
            ) | (
                RawSource::KiroCliV2Bundle { .. },
                SourceFormat::KiroCliV2Bundle
            ) | (
                RawSource::KiroCliV3Bundle { .. },
                SourceFormat::KiroCliV3Bundle
            )
        ));
        SessionInput {
            agent: vendor_label(self.agent).to_owned(),
            session_id: self.session_id.to_owned(),
            source: raw,
            source_format: self.format,
            fork_parent_session_id: None,
        }
    }

    async fn discovery_claim(&self) -> String {
        let log = SessionLog {
            agent_type: self.agent,
            source: self.source(),
            updated_at: None,
            environment: DiscoveryEnvironment::Native,
        };
        let read = session_log_read(&log).await.unwrap();
        let descriptor = SourceDescriptor {
            agent: self.agent,
            session_id: self.session_id.to_owned(),
            environment: log.environment,
            source: log.source,
            source_format: self.format,
            surface: "cli".to_owned(),
            updated_at_epoch: None,
        };
        let version = Explorers::DISK
            .source_version(&descriptor, Some(&read))
            .await
            .unwrap();
        assert_eq!(
            Some(version.fingerprint.clone()),
            bundle_fingerprint(&descriptor.source, self.format)
        );
        version.fingerprint
    }
}

fn engine_fixture(path: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../crates/antiburn-local/tests/fixtures")
            .join(path),
    )
    .unwrap()
}

fn copilot_bundle(directory: &Path) -> BundleFixture {
    let state_root = directory.join("session-state");
    let session = state_root.join(ROOT_ID);
    std::fs::create_dir_all(&session).unwrap();
    let path = session.join("events.jsonl");
    std::fs::write(
        &path,
        engine_fixture("source_contracts/copilot_multi_lane.jsonl"),
    )
    .unwrap();
    let connection = rusqlite::Connection::open(state_root.join("session-store.db")).unwrap();
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
            [ROOT_ID],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO request_usage VALUES (?1, 'request-1', NULL, NULL, 'synthetic-main', 10, 5, 2, 1)",
            [ROOT_ID],
        )
        .unwrap();
    BundleFixture {
        agent: AgentKind::Copilot,
        session_id: ROOT_ID,
        format: SourceFormat::CopilotCliJsonl,
        messages_path: path.clone(),
        path,
    }
}

fn cline_bundle(directory: &Path) -> BundleFixture {
    let data = directory.join(".cline/data");
    let session = data.join("tasks/root_1");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::create_dir_all(data.join("db")).unwrap();
    let path = session.join("root_1.json");
    let messages_path = session.join("root_1.messages.json");
    std::fs::write(
        &path,
        engine_fixture("cline_messages_contract_v1/root.json").replace(
            "\"PLACEHOLDER\"",
            &serde_json::to_string(&messages_path.to_string_lossy()).unwrap(),
        ),
    )
    .unwrap();
    std::fs::write(
        &messages_path,
        engine_fixture("cline_messages_contract_v1/root.messages.json"),
    )
    .unwrap();
    let child = session.join("child_agent.messages.json");
    std::fs::write(
        &child,
        engine_fixture("cline_messages_contract_v1/child.messages.json"),
    )
    .unwrap();
    let connection = rusqlite::Connection::open(data.join("db/sessions.db")).unwrap();
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
            "INSERT INTO sessions VALUES ('root_1', 'completed', 'model-root', 'lead', NULL, 0, ?1)",
            [messages_path.to_string_lossy()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO sessions VALUES ('child_1', 'completed', 'model-child', 'child_agent', 'root_1', 1, ?1)",
            [child.to_string_lossy()],
        )
        .unwrap();
    BundleFixture {
        agent: AgentKind::Cline,
        session_id: "root_1",
        format: SourceFormat::ClineMessagesContractV1,
        path,
        messages_path,
    }
}

fn kiro_bundle(directory: &Path, format: SourceFormat) -> BundleFixture {
    let (path, messages_path) = if format == SourceFormat::KiroCliV2Bundle {
        let session = directory.join("cli");
        std::fs::create_dir_all(&session).unwrap();
        let metadata = session.join(format!("{ROOT_ID}.json"));
        let messages = metadata.with_extension("jsonl");
        std::fs::write(&metadata, engine_fixture("kiro_cli_v2_bundle/root.json")).unwrap();
        std::fs::write(&messages, engine_fixture("kiro_cli_v2_bundle/root.jsonl")).unwrap();
        (metadata, messages)
    } else {
        assert_eq!(format, SourceFormat::KiroCliV3Bundle);
        let session = directory.join(format!("sess_{ROOT_ID}"));
        std::fs::create_dir_all(&session).unwrap();
        let metadata = session.join("session.json");
        let messages = session.join("messages.jsonl");
        // Kiro V3 has no supported parser. These files test only the claim gate.
        std::fs::write(&metadata, "{}").unwrap();
        std::fs::write(&messages, "{}\n").unwrap();
        (metadata, messages)
    };
    BundleFixture {
        agent: AgentKind::Kiro,
        session_id: ROOT_ID,
        format,
        path,
        messages_path,
    }
}

fn bundles(directory: &Path) -> Vec<BundleFixture> {
    vec![
        copilot_bundle(directory),
        cline_bundle(directory),
        kiro_bundle(directory, SourceFormat::KiroCliV2Bundle),
        kiro_bundle(directory, SourceFormat::KiroCliV3Bundle),
    ]
}

fn append_whitespace(path: &Path) {
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b" \n")
        .unwrap();
}

#[tokio::test]
async fn stale_bundle_claims_stop_before_parsing_or_writing_rows() {
    let directory = tempfile::tempdir().unwrap();
    for fixture in bundles(directory.path()) {
        let claim = fixture.discovery_claim().await;
        let input = fixture.input().await;
        append_whitespace(&fixture.messages_path);
        assert_ne!(
            bundle_fingerprint(&fixture.source(), fixture.format).as_deref(),
            Some(claim.as_str())
        );
        let rows = MemoryTurnRowStore::new(&input.agent, fixture.session_id);
        let changes = rows.with_connection(|connection| connection.total_changes());
        let signal = PassSignal::new();
        let callbacks = Cell::new(0);

        let streamed = stream_vendor_with_hooks(
            &[input],
            &|| signal.observe(),
            &|_, _| callbacks.set(callbacks.get() + 1),
            Some(&claim),
            Some(rows.clone()),
        );

        assert!(
            matches!(streamed, StreamOutcome::SourceChanged),
            "{:?}",
            fixture.format
        );
        assert_eq!(callbacks.get(), 0, "{:?}", fixture.format);
        assert_eq!(signal.progress(), 1, "{:?}", fixture.format);
        assert_eq!(
            rows.with_connection(|connection| connection.total_changes()),
            changes,
            "{:?}",
            fixture.format
        );
        assert!(rows.query_coverage_record().unwrap().is_none());

        let signal = PassSignal::new();
        let pass = analyze_located_for_evidence(
            fixture.agent,
            fixture.session_id,
            ClaimedSource {
                fingerprint: Some(claim),
                generation: 1,
            },
            signal.clone(),
            Some(rows.clone()),
            None,
            LocatedTranscripts {
                source: fixture.source(),
                children: Vec::new(),
            },
        )
        .await;

        assert_eq!(
            pass.outcome,
            PassOutcome::SourceChanged,
            "{:?}",
            fixture.format
        );
        assert!(pass.evidence.is_none());
        assert!(pass.source_fingerprint.is_none());
        assert_eq!(signal.progress(), 1, "{:?}", fixture.format);
        assert_eq!(
            rows.with_connection(|connection| connection.total_changes()),
            changes,
            "{:?}",
            fixture.format
        );
        assert!(rows.query_coverage_record().unwrap().is_none());
    }
}

#[tokio::test]
async fn unchanged_supported_bundles_publish_with_current_or_absent_discovery_claims() {
    let directory = tempfile::tempdir().unwrap();
    for fixture in bundles(directory.path())
        .into_iter()
        .filter(|fixture| fixture.format != SourceFormat::KiroCliV3Bundle)
    {
        let claim = fixture.discovery_claim().await;
        let cache_fingerprint = fingerprint_of(&fixture.source());
        if matches!(fixture.agent, AgentKind::Copilot | AgentKind::Cline) {
            assert_ne!(claim, cache_fingerprint);
        }
        for expected in [Some(claim.clone()), None] {
            let signal = PassSignal::new();
            let rows = MemoryTurnRowStore::new(vendor_label(fixture.agent), fixture.session_id);
            let changes = rows.with_connection(|connection| connection.total_changes());
            let generation = i64::from(expected.is_some());
            let pass = analyze_located_for_evidence(
                fixture.agent,
                fixture.session_id,
                ClaimedSource {
                    fingerprint: expected,
                    generation: 1,
                },
                signal.clone(),
                Some(rows.clone()),
                None,
                LocatedTranscripts {
                    source: fixture.source(),
                    children: Vec::new(),
                },
            )
            .await;

            assert_eq!(pass.outcome, PassOutcome::Published, "{:?}", fixture.format);
            assert_eq!(pass.source_fingerprint.as_deref(), Some(claim.as_str()));
            assert_eq!(
                pass.analysis.fingerprint,
                combined_fingerprint_from_parent(fixture.agent, cache_fingerprint.clone(), &[])
            );
            assert_eq!(pass.analysis.analyzed_generation, generation);
            assert!(pass.analysis.metrics.is_some());
            assert!(pass.evidence.is_some());
            assert!(signal.progress() > 1);
            assert!(rows.with_connection(|connection| connection.total_changes()) > changes);
            assert!(rows.query_coverage_record().unwrap().is_some());
        }
    }
}

#[tokio::test]
async fn unchanged_kiro_v3_bundle_stays_fail_closed_with_a_current_discovery_claim() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = kiro_bundle(directory.path(), SourceFormat::KiroCliV3Bundle);
    let claim = fixture.discovery_claim().await;
    let rows = MemoryTurnRowStore::new("kiro", fixture.session_id);
    let pass = analyze_located_for_evidence(
        fixture.agent,
        fixture.session_id,
        ClaimedSource {
            fingerprint: Some(claim),
            generation: 1,
        },
        PassSignal::new(),
        Some(rows.clone()),
        None,
        LocatedTranscripts {
            source: fixture.source(),
            children: Vec::new(),
        },
    )
    .await;

    assert_eq!(pass.outcome, PassOutcome::Published);
    let evidence = pass.evidence.unwrap();
    assert_eq!(
        evidence.capabilities,
        SourceCapabilities::uncharacterized(SourceFormat::KiroCliV3Bundle)
    );
    assert!(matches!(
        evidence.coverage,
        antiburn_local::analysis::EvidenceCoverage::Partial(_)
    ));
    assert_eq!(
        evidence.provenance.source_acceptance,
        SourceAcceptance::Unvalidated
    );
    for detector in DetectorId::ALL {
        assert!(!eligible(detector, &evidence));
        assert!(!clean_facts_complete(detector, &evidence));
    }
    assert_eq!(
        rows.with_connection(|connection| connection
            .query_row("SELECT COUNT(*) FROM turn", [], |row| row.get::<_, i64>(0))
            .unwrap()),
        0
    );
}

#[tokio::test]
async fn bundle_changes_after_claim_do_not_publish() {
    let directory = tempfile::tempdir().unwrap();
    for fixture in bundles(directory.path()) {
        let claim = fixture.discovery_claim().await;
        let input = fixture.input().await;
        let rows = MemoryTurnRowStore::new(&input.agent, fixture.session_id);
        let callbacks = Cell::new(0);

        let streamed = stream_vendor_with_hooks(
            &[input],
            &|| false,
            &|index, path| {
                assert_eq!(index, 0);
                assert_eq!(path, fixture.path.as_path());
                callbacks.set(callbacks.get() + 1);
                append_whitespace(&fixture.messages_path);
            },
            Some(&claim),
            Some(rows.clone()),
        );

        assert_eq!(callbacks.get(), 1, "{:?}", fixture.format);
        assert!(
            matches!(streamed, StreamOutcome::SourceChanged),
            "{:?}",
            fixture.format
        );
        assert!(rows.query_coverage_record().unwrap().is_none());
    }
}

#[tokio::test]
async fn copilot_wal_changes_after_claim_keep_the_cache_snapshot_guard() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = copilot_bundle(directory.path());
    let input = fixture.input().await;
    let RawSource::CopilotCliBundle { db_path, .. } = &input.source else {
        unreachable!()
    };
    let db_path = db_path.clone();
    let connection = rusqlite::Connection::open(&db_path).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA wal_autocheckpoint = 0;
             PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .unwrap();
    let claim = fixture.discovery_claim().await;
    let cache_fingerprint = fingerprint_of(&fixture.source());
    let database_stat = SourceStat::from_path(&db_path).await.unwrap();
    let callbacks = Cell::new(0);

    let streamed = stream_vendor_with_hooks(
        &[input],
        &|| false,
        &|_, _| {
            callbacks.set(callbacks.get() + 1);
            connection
                .execute("UPDATE sessions SET shutdown_model = 'changed'", [])
                .unwrap();
        },
        Some(&claim),
        None,
    );

    assert_eq!(callbacks.get(), 1);
    assert_eq!(
        SourceStat::from_path(&db_path).await.unwrap(),
        database_stat
    );
    assert!(
        std::fs::metadata(db_path.with_file_name("session-store.db-wal"))
            .unwrap()
            .len()
            > 0
    );
    assert_eq!(
        bundle_fingerprint(&fixture.source(), fixture.format).as_deref(),
        Some(claim.as_str())
    );
    assert_ne!(fingerprint_of(&fixture.source()), cache_fingerprint);
    assert!(matches!(streamed, StreamOutcome::SourceChanged));
}
