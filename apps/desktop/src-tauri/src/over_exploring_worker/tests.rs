use super::*;

use std::path::Path;
use std::sync::Arc;

use crate::store::{
    AnalysisRecord, EvidenceCompletion, FencedTurnRowStore, PublishedEvidence, SessionKey,
    SessionRecord,
};
use antiburn_local::analysis::jev::{JevAnswer, JevWorkItemResult};
use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, RawSource, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SourceKind, TurnRowSink, TurnRowStore, reader_for,
};
use antiburn_local::checks::over_exploring::Reason;
use serde_json::json;

#[test]
fn continuation_reuses_prepared_input_before_loading_source() {
    let (store, candidate) = fixture(None, "user");
    let capabilities = ModelCapabilities::jev_default();
    let input = load_input(&store, &candidate, &capabilities).unwrap();
    let loads = crate::smart_check_inputs::INPUT_LOAD_COUNT.get();
    for _ in 0..10 {
        let continuation = load_input(&store, &candidate, &capabilities).unwrap();
        assert!(Arc::ptr_eq(&input, &continuation));
    }
    assert_eq!(crate::smart_check_inputs::INPUT_LOAD_COUNT.get(), loads);
    let mut changed = capabilities.clone();
    changed.model.push_str("-changed");
    let replacement = load_input(&store, &candidate, &changed).unwrap();
    assert!(!Arc::ptr_eq(&input, &replacement));
    assert_eq!(crate::smart_check_inputs::INPUT_LOAD_COUNT.get(), loads + 1);
    store
        .lock()
        .execute(
            "UPDATE session SET cwd = '/synthetic/changed-workspace'",
            [],
        )
        .unwrap();
    assert!(load_input(&store, &candidate, &capabilities).is_err());
    assert_eq!(crate::smart_check_inputs::INPUT_LOAD_COUNT.get(), loads + 1);
    store
        .lock()
        .execute("UPDATE session_evidence SET status = 'pending'", [])
        .unwrap();
    assert!(load_input(&store, &candidate, &capabilities).is_err());
    assert_eq!(crate::smart_check_inputs::INPUT_LOAD_COUNT.get(), loads + 1);
}

fn dispatched_fixture(
    capabilities: ModelCapabilities,
    handle: &crate::jev::worker::WorkerHandle,
    provider_generation: u64,
    attempts: usize,
) -> (
    Store,
    BurnCheckCandidate,
    Arc<PreparedInput>,
    AssessmentCursor,
) {
    let read_content = "recorded parser output 123\n".repeat(300);
    let (store, candidate) =
        fixture_with_read_content(None, "user", &["/synthetic/parser.rs"; 3], &read_content);
    let input = load_input(&store, &candidate, &capabilities).unwrap();
    let now = unix_now();
    assert!(
        store
            .queue_burn_check_assessment(&input.durable, now, POLICY.idle_secs)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input.durable, now, POLICY.lease_secs, POLICY.idle_secs,)
            .unwrap()
    );
    let mut sampling = new_sampling().unwrap();
    synchronize_sampling(&input.plan, &mut sampling).unwrap();
    sampling.begin_run();
    let job = sampling.choose_job().unwrap();
    let mut plan = input.plan.clone();
    PreparedAssessment::select_jobs(&mut plan, std::slice::from_ref(&job)).unwrap();
    let identities = request_identities(handle, &input.durable, &capabilities, &plan);
    assert!(!identities.is_empty());
    let mut cursor = restore_cursor(None, &input.durable, provider_generation);
    cursor.sampling = Some(sampling);
    cursor.active_job = Some(job);
    cursor.run_started = true;
    cursor.result = Some(OverExploringCheck.reduce(&input.plan, &[], false).unwrap());
    cursor.active_capabilities = Some(capabilities);
    cursor.active_connection = Some(handle.system_one_connection());
    cursor.active_request_identities = identities.clone();
    assert!(save_cursor(&store, &input.durable, &cursor).unwrap());
    for identity in identities {
        store
            .lock()
            .execute(
                "INSERT INTO burn_check_dispatch_attempt
                   (request_identity, environment_key, agent, session_id, attempts, terminal)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    identity,
                    input.durable.key.environment_key,
                    input.durable.key.agent,
                    input.durable.key.session_id,
                    attempts,
                    i64::from(attempts >= 3),
                ],
            )
            .unwrap();
    }
    (store, candidate, input, cursor)
}

#[test]
fn pending_dispatch_keeps_original_capabilities_and_attempt_identity_after_shrink() {
    let handle = crate::jev::worker::WorkerHandle::default();
    let generation = 7;
    let capabilities = ModelCapabilities::jev_default();
    let (store, candidate, input, cursor) =
        dispatched_fixture(capabilities.clone(), &handle, generation, 3);
    let saved = store
        .burn_check_assessment(&candidate.session.key, CHECK_ID)
        .unwrap()
        .unwrap();
    let recovered = pending_over_exploring_capabilities(&store, &candidate, &handle)
        .unwrap()
        .expect("a dispatched request keeps its original capabilities");
    assert_eq!(recovered, capabilities);
    let mut shrunk = recovered.clone();
    shrunk.runtime_context_tokens.value = Some(1);
    assert!(
        load_input(&store, &candidate, &shrunk)
            .unwrap()
            .plan
            .work_items
            .is_empty()
    );
    let restored = restore_cursor_for_recovery(Some(&saved), &input.durable, 99, true);
    assert_eq!(restored.active_job, cursor.active_job);
    assert_eq!(
        restored.active_request_identities,
        cursor.active_request_identities
    );
    assert_eq!(restored.provider_generation, 99);
    assert_eq!(
        store
            .burn_check_dispatch_attempts(&cursor.active_request_identities)
            .unwrap(),
        3 * cursor.active_request_identities.len()
    );
    assert_eq!(
        crate::jev::worker::dispatch_readiness(
            &store,
            &handle,
            &input.durable,
            &recovered,
            &{
                let mut plan = input.plan.clone();
                PreparedAssessment::select_jobs(
                    &mut plan,
                    std::slice::from_ref(cursor.active_job.as_ref().unwrap()),
                )
                .unwrap();
                plan
            },
            &cursor.run_progress,
        )
        .unwrap(),
        crate::store::BurnCheckRequestAdmission::Exhausted
    );
}

#[test]
fn pending_dispatch_is_invalidated_by_source_or_provider_changes() {
    for invalidate_provider in [false, true] {
        let handle = crate::jev::worker::WorkerHandle::default();
        let (store, candidate, _, _) =
            dispatched_fixture(ModelCapabilities::jev_default(), &handle, 7, 3);
        if invalidate_provider {
            let mut connection = handle.system_one_connection();
            connection.model_revision = Some("changed-model-revision".into());
            handle.set_system_one_connection(connection, None).unwrap();
        } else {
            store
                .lock()
                .execute(
                    "UPDATE session_evidence SET published_fence = published_fence + 1",
                    [],
                )
                .unwrap();
        }
        assert!(
            pending_over_exploring_capabilities(&store, &candidate, &handle)
                .unwrap()
                .is_none(),
            "provider_changed={invalidate_provider}"
        );
    }
}

#[tokio::test]
async fn pending_dispatch_recovers_after_failed_capability_discovery() {
    use crate::jev::config::{SystemOneEndpoint, SystemOneProvider};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let read = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..read]).starts_with("GET /api/version "));
            socket
                .write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
        });
        let handle = Arc::new(crate::jev::worker::WorkerHandle::default());
        let mut connection = handle.system_one_connection();
        connection.provider = SystemOneProvider::Ollama;
        connection.endpoint = SystemOneEndpoint::BaseUrl(endpoint);
        connection.model = "synthetic-model".into();
        connection.model_revision = None;
        connection.credential = None;
        connection.context_override = None;
        handle
            .set_system_one_connection(connection, None)
            .unwrap();
        let generation = 1;
        let mut capabilities = ModelCapabilities::jev_default();
        capabilities.model = "synthetic-model".into();
        capabilities.model_revision = Some("accepted-model-digest".into());
        let setup_handle = Arc::clone(&handle);
        let setup_capabilities = capabilities.clone();
        let (store, candidate, _, _) = tokio::task::spawn_blocking(move || {
            dispatched_fixture(setup_capabilities, &setup_handle, generation, 3)
        })
        .await
        .unwrap();

        assert_eq!(
            handle.resolve_capabilities(generation).await,
            Err(JevError::ProviderUnavailable)
        );
        server.await.unwrap();
        let (recovered, was_recovered) =
            candidate_capabilities(&store, &candidate, &handle, generation)
                .await
                .unwrap();
        assert!(was_recovered);
        assert_eq!(recovered, capabilities);
    })
    .await
    .unwrap();
}

#[test]
fn accepted_third_attempt_finding_publishes_once_to_store() {
    let handle = crate::jev::worker::WorkerHandle::default();
    let capabilities = ModelCapabilities::jev_default();
    let (store, candidate, input, mut cursor) =
        dispatched_fixture(capabilities.clone(), &handle, 7, 3);
    let job = cursor.active_job.clone().unwrap();
    let mut plan = input.plan.clone();
    PreparedAssessment::select_jobs(&mut plan, std::slice::from_ref(&job)).unwrap();
    let reason = input.plan.prepared.targets[&plan.work_items[0].id].reason;
    let page = OverExploringCheck
        .reduce(&plan, &mock_results_for_plan(&plan, reason), true)
        .unwrap();
    assert_eq!(page.findings.len(), 1);
    plan.prepared
        .record_completion(&page, &job, cursor.sampling.as_mut().unwrap())
        .unwrap();
    merge_result(cursor.result.as_mut().unwrap(), page, &plan);
    let coverage = cursor
        .sampling
        .as_ref()
        .unwrap()
        .coverage(check_identity())
        .unwrap();
    let runnable = cursor
        .sampling
        .as_ref()
        .unwrap()
        .runnable_count(check_identity());
    let identities = cursor.active_request_identities.clone();
    let result = cursor.result.as_mut().unwrap();
    result.coverage.selected_items = coverage.completed;
    result.coverage.not_selected_items = runnable;
    result.coverage.skipped_items = coverage
        .eligible
        .saturating_sub(coverage.completed + result.coverage.not_selected_items);
    cursor.active_job = None;
    cursor.active_capabilities = None;
    cursor.active_connection = None;
    cursor.active_request_identities.clear();

    assert_eq!(
        publish_current(&store, &candidate, &capabilities, &input, &cursor).unwrap(),
        Some(crate::analytics::event::SmartCheckAssessmentOutcome::Finding)
    );
    assert_eq!(
        publish_current(&store, &candidate, &capabilities, &input, &cursor).unwrap(),
        None
    );
    let saved = store
        .burn_check_assessment(&candidate.session.key, CHECK_ID)
        .unwrap()
        .unwrap();
    let publication: Publication =
        serde_json::from_str(saved.result_json.as_deref().unwrap()).unwrap();
    assert_eq!(publication.assessment.findings.len(), 1);
    assert!(publishable_finding(
        &publication.assessment.findings[0],
        &publication
    ));
    assert_eq!(
        store.burn_check_dispatch_attempts(&identities).unwrap(),
        3 * identities.len()
    );
}

#[path = "benchmark.rs"]
mod benchmark;

#[test]
fn native_reads_keep_exact_source_bindings_without_inventing_targets() {
    use crate::scope_creep_worker::tests::native_sources;
    for (agent, session, format, records) in native_sources::read_sources() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        for detector in [
            antiburn_local::checks::DetectorId::OverExploring,
            antiburn_local::checks::DetectorId::ScopeCreep,
        ] {
            store.set_check_enabled(detector, true).unwrap();
        }
        store
            .capture_burn_check_boundaries(&[CHECK_ID, crate::scope_creep_worker::CHECK_ID], 0)
            .unwrap_or_else(|error| panic!("{agent}: {error:?}"));
        let mut candidate =
            native_sources::publish(&store, &agent, &session, format, &records, directory.path());
        let coverage = store
            .published_coverage_record(&candidate.session.key)
            .unwrap()
            .unwrap_or_else(|| panic!("{agent}: missing published coverage"));
        assert_eq!(
            coverage.source_acceptance,
            antiburn_local::analysis::SourceAcceptance::AcceptedFull,
            "{agent}: {coverage:#?}"
        );
        assert!(
            coverage.ordering == antiburn_local::analysis::OrderingObservation::Monotonic
                && coverage.summary_observed
                && coverage.record_loss_reason.is_none()
                && coverage.child_loss_reason.is_none()
                && !coverage.session_cap_exceeded
                && !coverage.thread_parent_unresolved
                && !coverage.subagent_linkage_incomplete
                && coverage.diagnostics.records_unusable == 0
                && coverage.diagnostics.duplicate_turn_identities == 0
                && coverage.diagnostics.unusable_reasons.is_empty()
                && coverage.diagnostics.truncated_strings.is_empty()
                && coverage.diagnostics.capped_collections.is_empty(),
            "{agent}: {coverage:#?}"
        );
        let snapshot = store
            .load_smart_check_inputs(
                &candidate.session.key,
                candidate.published_fence,
                candidate.source_generation,
                DetectorInput::OverExploring,
            )
            .unwrap_or_else(|error| panic!("{agent}: {error:?}"));
        if agent == "codex" || agent == "claude-code" {
            let request = snapshot
                .content()
                .actions
                .iter()
                .find_map(|action| action.metadata.read_request.as_ref())
                .expect("Codex read request is observed");
            let contract = if agent == "codex" {
                "openai/codex@d27764b82f7118f674371e6d6e76271d9d606edb"
            } else {
                "https://platform.claude.com/docs/en/agent-sdk/typescript#read"
            };
            assert_eq!(
                request.extent_contract.as_deref(),
                Some(contract),
                "{agent}"
            );
        }
        let native_reads = &snapshot.content().actions;
        let requests = native_reads
            .iter()
            .filter_map(|action| action.metadata.read_request.as_ref())
            .collect::<Vec<_>>();
        assert!(!requests.is_empty(), "{agent}");
        for request in &requests {
            let request_action = native_reads
                .iter()
                .find(|action| action.reference.id == request.reference_id)
                .unwrap();
            assert!(!request_action.metadata.bindings.is_empty(), "{agent}");
            let result_action = native_reads.iter().find(|action| {
                action.metadata.read_result.as_ref().is_some_and(|result| {
                    result.request_reference_id.as_deref() == Some(&request.reference_id)
                })
            });
            if let Some(result_action) = result_action {
                let result = result_action.metadata.read_result.as_ref().unwrap();
                assert_eq!(result.reference_id, result_action.reference.id, "{agent}");
                assert!(!result_action.metadata.bindings.is_empty(), "{agent}");
            }
        }
        candidate
            .boundary_positions
            .insert(snapshot.boundary().source_key.clone(), u64::MAX);
        let future = prepare(&candidate, snapshot, &ModelCapabilities::jev_default()).unwrap();
        assert!(future.plan.prepared.targets.is_empty(), "{agent}");
    }
}

#[test]
fn opencode_sqlite_observed_reads_reach_worker_preparation() {
    let (store, candidate) = fixture(None, "user");
    let snapshot = store
        .load_smart_check_inputs(
            &candidate.session.key,
            candidate.published_fence,
            candidate.source_generation,
            DetectorInput::OverExploring,
        )
        .unwrap();
    let reads = snapshot
        .content()
        .actions
        .iter()
        .filter(|action| action.kind == "tool_input")
        .filter_map(|action| action.metadata.read_request.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(reads.len(), 2);
    assert_eq!(reads[0].paths, ["/synthetic/parser.rs"]);
    assert_eq!(reads[0].extent.offset, Some(1));
    assert_eq!(reads[0].extent.limit, Some(304));
    assert_eq!(reads[1].paths, ["/synthetic/parser.rs"]);
    assert_eq!(reads[1].extent.offset, Some(1));
    assert_eq!(reads[1].extent.limit, Some(304));
    assert!(
        reads
            .iter()
            .all(|read| read.cwd.as_deref() == Some("/synthetic"))
    );

    let input = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    assert!(!input.plan.prepared.targets.is_empty());
    assert!(input.plan.prepared.targets.values().any(|target| {
        target.bindings.iter().any(|binding| {
            reads
                .iter()
                .any(|request| binding.request_id == request.reference_id)
        })
    }));
}

pub(crate) fn fixture(directory: Option<&Path>, ending: &str) -> (Store, BurnCheckCandidate) {
    fixture_with_read_content(
        directory,
        ending,
        &["/synthetic/parser.rs", "/synthetic/parser.rs"],
        &"recorded parser output 123\n".repeat(300),
    )
}

fn fixture_with_read_content(
    directory: Option<&Path>,
    ending: &str,
    paths: &[&str],
    read_content: &str,
) -> (Store, BurnCheckCandidate) {
    let store = directory
        .map(|path| Store::open(path).unwrap())
        .unwrap_or_else(|| {
            Store::open_in_memory(Path::new("/tmp/antiburn-over-exploring-tests")).unwrap()
        });
    fixture_in_store(store, "investigation", ending, paths, read_content)
}

fn fixture_in_store(
    store: Store,
    session_id: &str,
    ending: &str,
    paths: &[&str],
    read_content: &str,
) -> (Store, BurnCheckCandidate) {
    let read_content = if read_content.len() < 8192 {
        format!("{read_content}{}", "x".repeat(8192 - read_content.len()))
    } else {
        read_content.to_owned()
    };
    let mut records = vec![(
        "user",
        json!({"type":"text","text":"Fix the parser. Reuse matching recorded reads."}),
    )];
    for (index, path) in paths.iter().enumerate() {
        records.push((
            "assistant",
            indexed_read_record(path, &format!("read-{index}"), &read_content),
        ));
    }
    records.push((
        if ending == "user" {
            "user"
        } else {
            "assistant"
        },
        json!({"type":"text","text":"Now explain the result."}),
    ));
    fixture_in_store_records(store, session_id, records)
}

fn fixture_in_store_with_long_read_context(
    store: Store,
    session_id: &str,
    parser_read_content: &str,
    garden_read_content: &str,
) -> (Store, BurnCheckCandidate) {
    assert!(garden_read_content.len() < 8192);
    let parser_read_content = if parser_read_content.len() < 8192 {
        format!(
            "{parser_read_content}{}",
            "x".repeat(8192 - parser_read_content.len())
        )
    } else {
        parser_read_content.to_owned()
    };
    let mut records = vec![(
        "user",
        json!({"type":"text","text":"Fix the parser. Reuse matching recorded reads."}),
    )];
    for index in 0..4 {
        records.push((
            "assistant",
            indexed_read_record(
                "/synthetic/parser.rs",
                &format!("parser-read-{index}"),
                &parser_read_content,
            ),
        ));
    }
    records.push((
        "assistant",
        json!({"type":"text","text":"The parser task is complete. I am checking a separate garden README for a different gardening task; this read does not support the parser task."}),
    ));
    records.push((
        "assistant",
        indexed_read_record(
            "/synthetic/garden/README.md",
            "garden-read",
            garden_read_content,
        ),
    ));
    records.push((
        "user",
        json!({"type":"text","text":"Now explain the result."}),
    ));
    fixture_in_store_records(store, session_id, records)
}

fn indexed_read_record(path: &str, call_id: &str, read_content: &str) -> serde_json::Value {
    let line_count = read_content.lines().count().max(1);
    let content = read_content
        .lines()
        .enumerate()
        .map(|(line, text)| format!("{}: {text}", line + 1))
        .collect::<Vec<_>>()
        .join("\n");
    json!({"type":"tool","tool":"read","callID":call_id,
        "state":{"status":"completed","input":{"filePath":path,"cwd":"/synthetic","offset":1,"limit":line_count + 3},
        "output":format!("<path>{path}</path>\n<type>file</type>\n<content>\n{content}\n\n(End of file - total {line_count} lines)\n</content>")}})
}

fn fixture_in_store_records(
    store: Store,
    session_id: &str,
    records: Vec<(&str, serde_json::Value)>,
) -> (Store, BurnCheckCandidate) {
    let source_dir = tempfile::tempdir().unwrap();
    let source_path = source_dir.path().join("opencode.db");
    let source = rusqlite::Connection::open(&source_path).unwrap();
    source.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, time_created INTEGER, time_updated INTEGER);
        CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
        CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);").unwrap();
    source
        .execute(
            "INSERT INTO session VALUES (?1, NULL, 1000, 9000)",
            rusqlite::params![session_id],
        )
        .unwrap();
    for (index, (role, part)) in records.iter().enumerate() {
        source
            .execute(
                "INSERT INTO message VALUES (?1, ?2, ?3, ?3, ?4)",
                rusqlite::params![
                    format!("m{index}"),
                    session_id,
                    index + 1001,
                    json!({"role":role}).to_string()
                ],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                rusqlite::params![
                    format!("p{index}"),
                    format!("m{index}"),
                    session_id,
                    index + 1001,
                    part.to_string()
                ],
            )
            .unwrap();
    }
    let (latest, rows) = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(
            antiburn_local::discovery::agents::opencode::db_session_fingerprint(
                source_path.clone(),
                session_id.to_owned(),
            ),
        )
        .unwrap();
    let fingerprint = format!("sv1:db:{latest}:{rows}");
    store
        .set_check_enabled(antiburn_local::checks::DetectorId::OverExploring, true)
        .unwrap();
    store.capture_burn_check_boundaries(&[CHECK_ID], 0).unwrap();
    let key = SessionKey::new("native", "opencode", session_id);
    store
        .upsert_sessions(
            &[SessionRecord {
                key: key.clone(),
                source_kind: "providerDb".into(),
                source_label: "synthetic-db".into(),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: Some("/synthetic".into()),
                surface: "cli".into(),
                updated_at_epoch: Some(9),
                activity_cursor: "activity".into(),
                activity_source: "event".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some(fingerprint.clone()),
            }],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let claim = store
        .claim_next_evidence(&["opencode"], 100, 60)
        .unwrap()
        .unwrap();
    let writer = Arc::new(FencedTurnRowStore::new(
        store.clone(),
        key.clone(),
        claim.claim_fence,
    ));
    let input = SessionInput {
        agent: "opencode".into(),
        session_id: key.session_id.clone(),
        source: RawSource::Sqlite(source_path),
        source_format: SourceFormat::OpenCodeSqliteV2,
        fork_parent_session_id: None,
    };
    let reader = reader_for("opencode");
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("opencode", &key.session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "opencode".into(),
            session_id: key.session_id.clone(),
            kind: SourceKind::Sqlite,
            capabilities: reader.capabilities(&input),
        }),
        TurnRowSink::new(
            writer.clone() as Arc<dyn TurnRowStore>,
            &key.session_id,
            None,
        ),
    );
    let outcome = reader
        .visit_db_claimed(&input, &fingerprint, &|| false, &mut sink)
        .unwrap();
    sink.observe_source_outcome(outcome);
    writer
        .write_coverage_record(&sink.coverage_record().unwrap())
        .unwrap();
    let revisions = crate::analysis::projection_revisions();
    store
        .publish_projections(
            &AnalysisRecord {
                key: key.clone(),
                model_breakdown_json: "{}".into(),
                pricing_breakdown_json: "{}".into(),
                inclusive_models_json: "[]".into(),
                initial_context_json: None,
                source_summaries_json: None,
                provider_hints_json: None,
                source_fingerprint: fingerprint,
                pricing_generation: 1,
                analyzed_generation: claim.source_generation,
                parser_revision: revisions.parser_revision,
                analyzer_revision: revisions.analyzer_revision,
                metrics_schema_revision: revisions.metrics_schema_revision,
            },
            Some(1),
            &EvidenceCompletion {
                claim_fence: claim.claim_fence,
                status: PublishedEvidence::Ready,
                evidence_schema_revision: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                evidence_json: serde_json::to_string(&sink.evidence().unwrap()).unwrap(),
            },
            &[],
            &[],
        )
        .unwrap();
    let candidate = store
        .burn_check_candidates_for_revision(
            CHECK_ID,
            &CHECK.evaluator_revision(),
            1000,
            POLICY.idle_secs,
            128,
        )
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.session.key == key)
        .unwrap_or_else(|| panic!("fixture candidate missing for {session_id}"));
    (store, candidate)
}

pub(crate) fn reduced(input: &PreparedInput, reason: Reason) -> Assessment {
    complete_sampling(input, reason).0
}

fn complete_sampling(input: &PreparedInput, reason: Reason) -> (Assessment, SamplingProgress) {
    let mut sampling = new_sampling().unwrap();
    over_exploring::synchronize_sampling(&input.plan, &mut sampling).unwrap();
    let mut result = OverExploringCheck.reduce(&input.plan, &[], false).unwrap();
    result.unassessed = input.plan.prepared.unassessed.clone();
    while sampling.coverage(check_identity()).unwrap().remaining > 0 {
        sampling.begin_run();
        while let Some(job) = sampling.choose_job() {
            let mut plan = input.plan.clone();
            PreparedAssessment::select_jobs(&mut plan, std::slice::from_ref(&job)).unwrap();
            let page = OverExploringCheck
                .reduce(&plan, &mock_results_for_plan(&plan, reason), true)
                .unwrap();
            plan.prepared
                .record_completion(&page, &job, &mut sampling)
                .unwrap();
            merge_result(&mut result, page, &plan);
        }
    }
    result.coverage.selected_items = result.completed_work_item_ids.len();
    result.coverage.not_selected_items = 0;
    (result, sampling)
}

fn mock_results(input: &PreparedInput, reason: Reason) -> Vec<JevWorkItemResult> {
    mock_results_for_plan(&input.plan, reason)
}

fn mock_results_for_plan(
    plan: &JevCheckPlan<PreparedAssessment>,
    reason: Reason,
) -> Vec<JevWorkItemResult> {
    plan.work_items
        .iter()
        .map(|item| {
            let selected = plan.prepared.targets[&item.id].reason;
            let answers = item
                .questions
                .keys()
                .map(|gate| {
                    let choice = if selected != reason {
                        "justified_or_minor"
                    } else {
                        "likely_excess"
                    };
                    (
                        gate.clone(),
                        JevAnswer::Choice {
                            choice: choice.into(),
                            confidence: 1.0,
                            probabilities: ["likely_excess", "justified_or_minor", "uncertain"]
                                .into_iter()
                                .map(|key| (key.into(), if key == choice { 1.0 } else { 0.0 }))
                                .collect(),
                        },
                    )
                })
                .collect();
            JevWorkItemResult {
                request_id: "synthetic-request".into(),
                work_item_id: item.id.clone(),
                model: plan.capabilities.model.clone(),
                answers,
                evidence: plan
                    .shared_context
                    .as_ref()
                    .unwrap()
                    .evidence
                    .iter()
                    .chain(&item.window.evidence)
                    .cloned()
                    .collect(),
                usage: antiburn_local::analysis::JevUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            }
        })
        .collect()
}

#[test]
fn merges_preserve_completed_positives_across_unanswered_work() {
    let (store, candidate) = fixture(None, "user");
    let input = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    let mut accepted = reduced(&input, Reason::ExcessiveWithinFileReading);
    let positive = accepted.findings.clone();
    let gap = OverExploringCheck.reduce(&input.plan, &[], false).unwrap();
    merge_result(&mut accepted, gap, &input.plan);
    assert_eq!(accepted.findings, positive);
}

#[test]
fn publication_accepts_produced_task_bindings_and_rejects_legacy_or_unbound_references() {
    let (store, candidate) = fixture(None, "user");
    let input = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    let assessment = reduced(&input, Reason::ExcessiveWithinFileReading);
    let saved = publication(&input, assessment);
    assert!(!saved.assessment.findings.is_empty());
    assert!(
        saved
            .assessment
            .findings
            .iter()
            .all(|finding| publishable_finding(finding, &saved))
    );
    let mut legacy = saved.assessment.clone();
    legacy.findings[0].task_evidence[0].part_id = "task[0]".into();
    let legacy = publication(&input, legacy);
    assert!(!publishable_finding(
        &legacy.assessment.findings[0],
        &legacy
    ));
    let mut invalid = saved.assessment.clone();
    invalid.findings[0].task_evidence[0].source_id = "unbound-task".into();
    let invalid = publication(&input, invalid);
    assert!(!publishable_finding(
        &invalid.assessment.findings[0],
        &invalid
    ));
}

#[test]
fn future_work_keeps_old_scope_and_reads_without_enrolling_old_targets() {
    let (store, mut candidate) = fixture(None, "user");
    let snapshot = store
        .load_smart_check_inputs(
            &candidate.session.key,
            candidate.published_fence,
            candidate.source_generation,
            DetectorInput::OverExploring,
        )
        .unwrap();
    candidate.boundary_positions = BTreeMap::from([(snapshot.boundary().source_key.clone(), 1)]);
    let prepared = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    let old_request = snapshot
        .content()
        .actions
        .iter()
        .find(|action| action.kind == "tool_input" && action.reference.turn_index == 1)
        .unwrap();
    assert!(
        prepared
            .plan
            .shared_context
            .as_ref()
            .unwrap()
            .fields
            .to_string()
            .contains("Reuse matching recorded reads")
    );
    assert!(!prepared.plan.prepared.targets.is_empty());
    assert!(prepared.plan.prepared.targets.values().all(|target| {
        target
            .bindings
            .iter()
            .all(|binding| binding.request_id != old_request.reference.id)
    }));
    let mut completed = candidate.clone();
    completed
        .boundary_positions
        .insert(snapshot.boundary().source_key.clone(), 3);
    assert_eq!(
        store
            .enrolled_burn_check_candidate(&completed, CHECK_ID)
            .unwrap()
            .boundary_positions,
        candidate.boundary_positions
    );
    candidate.historical = true;
    let history = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    assert!(history.plan.prepared.targets.values().any(|target| {
        target
            .bindings
            .iter()
            .any(|binding| binding.request_id == old_request.reference.id)
    }));
}

#[test]
fn production_loader_binds_supported_findings_to_observed_reads_and_full_task() {
    let (store, candidate) = fixture(None, "user");
    let input = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    assert!(
        input
            .plan
            .shared_context
            .as_ref()
            .unwrap()
            .fields
            .to_string()
            .contains("Reuse matching recorded reads")
    );
    let result = reduced(&input, Reason::ExcessiveWithinFileReading);
    assert!(!result.findings.is_empty(), "{result:#?}");
    let publication = publication(&input, result);
    for finding in &publication.assessment.findings {
        assert_eq!(finding.reason, Reason::ExcessiveWithinFileReading);
        assert!(publishable_finding(finding, &publication));
        let mut broken = finding.clone();
        broken.reads[0].result_id = Some("not-an-observed-result".into());
        assert!(!publishable_finding(&broken, &publication));
        let mut broken = finding.clone();
        broken.reads[0].output_digest = Some("not-the-recorded-output".into());
        assert!(!publishable_finding(&broken, &publication));
        let mut broken = finding.clone();
        broken.task_evidence[0].part_id = "not-the-selected-task-part".into();
        assert!(!publishable_finding(&broken, &publication));
        let mut broken = finding.clone();
        broken.source_evidence[0].part_id = "not-the-selected-source-range".into();
        assert!(!publishable_finding(&broken, &publication));
        let mut broken = finding.clone();
        broken.probability = 0.74;
        assert!(!publishable_finding(&broken, &publication));
    }
    let revisions = OverExploringCheck.revisions();
    assert_eq!(
        CHECK.evaluator_revision(),
        format!(
            "over-exploring-adapter-v6:{}:{}:{}:{}",
            revisions.projection, revisions.chunking, revisions.questions, revisions.reducer
        )
    );
}

#[test]
fn fresh_and_cached_results_require_exact_source_range_references() {
    let (store, candidate) = fixture(None, "user");
    let input = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    let fresh = mock_results(&input, Reason::ExcessiveWithinFileReading);
    let cached: Vec<JevWorkItemResult> =
        serde_json::from_str(&serde_json::to_string(&fresh).unwrap()).unwrap();
    assert_eq!(
        OverExploringCheck
            .reduce(&input.plan, &fresh, true)
            .unwrap(),
        OverExploringCheck
            .reduce(&input.plan, &cached, true)
            .unwrap()
    );
    let source_start = input.plan.shared_context.as_ref().unwrap().evidence.len();
    for mut results in [fresh, cached] {
        results[0].evidence[source_start].part_id = "different-range-of-the-same-source".into();
        assert!(
            OverExploringCheck
                .reduce(&input.plan, &results, true)
                .is_err()
        );
    }
}

#[test]
fn durable_sampling_reopens_without_replenishing_and_rejects_provider_and_revision_changes() {
    let directory = tempfile::tempdir().unwrap();
    let (store, candidate) = fixture(Some(directory.path()), "user");
    let input = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    let mut cursor = restore_cursor(None, &input.durable, 7);
    let mut sampling = new_sampling().unwrap();
    over_exploring::synchronize_sampling(&input.plan, &mut sampling).unwrap();
    sampling.begin_run();
    cursor.run_started = true;
    cursor.active_job = sampling.choose_job();
    cursor.sampling = Some(sampling);
    assert!(
        store
            .queue_burn_check_assessment(&input.durable, unix_now(), 180)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input.durable, unix_now(), 300, 180)
            .unwrap()
    );
    assert!(save_cursor(&store, &input.durable, &cursor).unwrap());
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    let saved = store
        .burn_check_assessment(&input.durable.key, CHECK_ID)
        .unwrap()
        .unwrap();
    let restored = restore_cursor(Some(&saved), &input.durable, 7);
    assert_eq!(restored.active_job, cursor.active_job);
    assert_eq!(restored.sampling, cursor.sampling);
    assert!(restored.run_started);
    assert!(!restored.run_finished);
    assert!(
        restore_cursor(Some(&saved), &input.durable, 8)
            .active_job
            .is_none()
    );
    let mut changed = input.durable.clone();
    changed.input_revision = "changed-semantic-context".into();
    assert!(restore_cursor(Some(&saved), &changed, 7).sampling.is_none());
    let mut restored = restore_cursor(Some(&saved), &input.durable, 7);
    let result = reduced(&input, Reason::ExcessiveWithinFileReading);
    input
        .plan
        .prepared
        .record_completion(
            &result,
            restored.active_job.as_ref().unwrap(),
            restored.sampling.as_mut().unwrap(),
        )
        .unwrap();
    restored.active_job = None;
    restored.result = Some(result);
    assert!(save_cursor(&store, &input.durable, &restored).unwrap());
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    let saved = store
        .burn_check_assessment(&input.durable.key, CHECK_ID)
        .unwrap()
        .unwrap();
    let restored = restore_cursor(Some(&saved), &input.durable, 7);
    assert_eq!(
        restored
            .sampling
            .unwrap()
            .coverage(check_identity())
            .unwrap()
            .completed,
        1
    );
}

#[test]
fn terminal_uncertainty_finishes_sampling_without_claiming_clean_coverage() {
    let (store, candidate) = fixture(None, "user");
    let input = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    let mut sampling = new_sampling().unwrap();
    over_exploring::synchronize_sampling(&input.plan, &mut sampling).unwrap();
    sampling.begin_run();
    let job = sampling.choose_job().unwrap();
    let mut plan = input.plan.clone();
    PreparedAssessment::select_jobs(&mut plan, std::slice::from_ref(&job)).unwrap();
    assert_eq!(plan.work_items.len(), 1);
    let mut results = mock_results_for_plan(&plan, Reason::ExcessiveWithinFileReading);
    results.retain(|result| result.work_item_id == plan.work_items[0].id);
    for answer in results[0].answers.values_mut() {
        *answer = JevAnswer::Choice {
            choice: "uncertain".into(),
            confidence: 1.0,
            probabilities: BTreeMap::from([
                ("likely_excess".into(), 0.0),
                ("justified_or_minor".into(), 0.0),
                ("uncertain".into(), 1.0),
            ]),
        };
    }
    let result = OverExploringCheck.reduce(&plan, &results, true).unwrap();
    assert!(
        result
            .completed_work_item_ids
            .contains(&plan.work_items[0].id)
    );
    assert!(!result.unassessed.is_empty());
    assert!(result.findings.is_empty());
    assert!(!publication_has_clean_coverage(&result));
    plan.prepared
        .record_completion(&result, &job, &mut sampling)
        .unwrap();
    assert_eq!(sampling.coverage(check_identity()).unwrap().completed, 1);
    let restored: SamplingProgress =
        serde_json::from_str(&serde_json::to_string(&sampling).unwrap()).unwrap();
    let mut restored = restored;
    assert!(
        restored
            .choose_job()
            .is_none_or(|next| next.candidate != job.candidate)
    );
}

#[test]
fn deletion_clear_and_publication_changes_fence_checkpoints_and_publication() {
    for change in [
        "delete",
        "clear",
        "generation",
        "fence",
        "fingerprint",
        "incarnation",
    ] {
        let (store, candidate) = fixture(None, "user");
        let input = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
        assert!(
            store
                .queue_burn_check_assessment(&input.durable, unix_now(), 180)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&input.durable, unix_now(), 300, 180)
                .unwrap()
        );
        match change {
            "delete" => {
                store.delete_session(&input.durable.key).unwrap();
            }
            "clear" => {
                store.clear_local_session_data().unwrap();
            }
            "generation" => {
                store
                    .lock()
                    .execute(
                        "UPDATE session SET source_generation = source_generation + 1",
                        [],
                    )
                    .unwrap();
            }
            "fence" => {
                store
                    .lock()
                    .execute(
                        "UPDATE session_evidence SET published_fence = published_fence + 1",
                        [],
                    )
                    .unwrap();
            }
            "fingerprint" => {
                store
                    .lock()
                    .execute("UPDATE session SET source_fingerprint = 'changed'", [])
                    .unwrap();
            }
            "incarnation" => {
                store
                    .lock()
                    .execute("UPDATE session SET incarnation = incarnation + 1", [])
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            load_input(&store, &candidate, &ModelCapabilities::jev_default()).is_err()
                || matches!(change, "incarnation" | "fingerprint")
        );
        assert!(
            !save_cursor(&store, &input.durable, &AssessmentCursor::default()).unwrap(),
            "{change}"
        );
        assert!(
            !store
                .complete_burn_check_assessment(&input.durable, "{}", unix_now(), 180)
                .unwrap(),
            "{change}"
        );
    }
}

#[test]
fn unanswered_targets_never_produce_clean_coverage() {
    let (store, candidate) = fixture(None, "assistant");
    let input = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    let result = OverExploringCheck.reduce(&input.plan, &[], true).unwrap();
    assert!(!publication_has_clean_coverage(&result));
    assert!(!result.unassessed.is_empty());
    assert!(result.findings.is_empty());
    assert!(!input.plan.prepared.candidates.is_empty());
}

#[test]
fn provider_failure_preserves_cursor_and_cannot_publish_clean() {
    let (store, candidate) = fixture(None, "user");
    let input = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    let mut cursor = restore_cursor(None, &input.durable, 7);
    cursor.result = Some(OverExploringCheck.reduce(&input.plan, &[], false).unwrap());
    assert!(
        store
            .queue_burn_check_assessment(&input.durable, unix_now(), 180)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input.durable, unix_now(), 300, 180)
            .unwrap()
    );
    save_failure(
        &store,
        &input,
        &cursor,
        "provider_unavailable",
        Some(unix_now() + 300),
    )
    .unwrap();
    let saved = store
        .burn_check_assessment(&input.durable.key, CHECK_ID)
        .unwrap()
        .unwrap();
    assert_eq!(saved.status, "failed");
    let publication: Publication =
        serde_json::from_str(saved.result_json.as_deref().unwrap()).unwrap();
    assert!(!publication_has_clean_coverage(&publication.assessment));
    assert_eq!(
        restore_cursor(Some(&saved), &input.durable, 7).input_revision,
        input.durable.input_revision
    );
}

#[test]
fn provider_change_rejects_final_production_publication() {
    let (store, candidate) = fixture(None, "user");
    let capabilities = ModelCapabilities::jev_default();
    let input = load_input(&store, &candidate, &capabilities).unwrap();
    let mut cursor = restore_cursor(None, &input.durable, 1);
    cursor.result = Some(reduced(&input, Reason::ExcessiveWithinFileReading));
    assert!(
        store
            .queue_burn_check_assessment(&input.durable, unix_now(), 180)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input.durable, unix_now(), 300, 180)
            .unwrap()
    );
    let handle = crate::jev::worker::WorkerHandle::default();
    handle
        .set_system_one_connection(
            crate::jev::config::SystemOneConnection::jev_default(),
            Some("synthetic-key".into()),
        )
        .unwrap();
    assert!(handle.key_is_current(1));
    handle
        .set_system_one_connection(
            crate::jev::config::SystemOneConnection::jev_default(),
            Some("replacement-key".into()),
        )
        .unwrap();
    assert!(
        handle
            .with_current_generation(1, || publish_current(
                &store,
                &candidate,
                &capabilities,
                &input,
                &cursor
            ))
            .is_none()
    );
    let saved = store
        .burn_check_assessment(&input.durable.key, CHECK_ID)
        .unwrap()
        .unwrap();
    assert_eq!(saved.status, "running");
    assert!(saved.result_json.is_none());
}

#[test]
fn provider_replacement_waits_for_the_publication_commit() {
    use std::sync::mpsc;
    use std::time::Duration;
    let (store, candidate) = fixture(None, "user");
    let capabilities = ModelCapabilities::jev_default();
    let input = load_input(&store, &candidate, &capabilities).unwrap();
    let mut cursor = restore_cursor(None, &input.durable, 1);
    let (mut result, sampling) = complete_sampling(&input, Reason::ExcessiveWithinFileReading);
    let coverage = sampling.coverage(check_identity()).unwrap();
    result.coverage.selected_items = coverage.completed;
    result.coverage.not_selected_items = sampling.runnable_count(check_identity());
    result.coverage.skipped_items = coverage
        .eligible
        .saturating_sub(result.coverage.selected_items + result.coverage.not_selected_items);
    cursor.sampling = Some(sampling);
    cursor.result = Some(result);
    assert!(
        store
            .queue_burn_check_assessment(&input.durable, unix_now(), 180)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input.durable, unix_now(), 300, 180)
            .unwrap()
    );
    let handle = Arc::new(crate::jev::worker::WorkerHandle::default());
    handle
        .set_system_one_connection(
            crate::jev::config::SystemOneConnection::jev_default(),
            Some("synthetic-key".into()),
        )
        .unwrap();
    let (start, ready) = mpsc::channel();
    let (replacing, started) = mpsc::channel();
    let (done, replaced) = mpsc::channel();
    let writer_handle = handle.clone();
    let writer = std::thread::spawn(move || {
        ready.recv().unwrap();
        replacing.send(()).unwrap();
        writer_handle
            .set_system_one_connection(
                crate::jev::config::SystemOneConnection::jev_default(),
                Some("replacement-key".into()),
            )
            .unwrap();
        done.send(()).unwrap();
    });
    let published = handle
        .with_current_generation(1, || {
            start.send(()).unwrap();
            started.recv().unwrap();
            let published =
                publish_current(&store, &candidate, &capabilities, &input, &cursor).unwrap();
            assert!(replaced.recv_timeout(Duration::from_millis(20)).is_err());
            published
        })
        .unwrap();
    assert_eq!(
        published,
        Some(crate::analytics::event::SmartCheckAssessmentOutcome::Finding)
    );
    replaced.recv_timeout(Duration::from_secs(1)).unwrap();
    writer.join().unwrap();
    assert!(!handle.key_is_current(1));
    let saved = store
        .burn_check_assessment(&input.durable.key, CHECK_ID)
        .unwrap()
        .unwrap();
    assert_eq!(saved.status, "completed", "{saved:#?}");
}
