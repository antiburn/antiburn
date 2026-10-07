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
use serde_json::json;

#[test]
fn native_reads_reach_preparation_history_and_source_bound_publication() {
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
        candidate
            .boundary_positions
            .insert(snapshot.boundary().source_key.clone(), u64::MAX);
        let future = prepare(&candidate, snapshot, &ModelCapabilities::jev_default()).unwrap();
        assert!(future.plan.prepared.targets.is_empty(), "{agent}");
        candidate.historical = true;
        let input = load_input(&store, &candidate, &ModelCapabilities::jev_default())
            .unwrap_or_else(|error| panic!("{agent}: {error:?}"));
        assert!(
            !input.plan.prepared.targets.is_empty(),
            "{agent}: {:#?}",
            input.plan.prepared
        );
        let result = reduced(&input, Reason::UnrelatedFiles);
        assert!(!result.findings.is_empty(), "{agent}");
        let publication = publication(&input, result);
        assert!(
            publication
                .assessment
                .findings
                .iter()
                .all(|finding| publishable_finding(finding, &publication)),
            "{agent}"
        );
        assert!(
            store
                .queue_burn_check_assessment(&input.durable, 1000, POLICY.idle_secs)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(
                    &input.durable,
                    1000,
                    POLICY.lease_secs,
                    POLICY.idle_secs
                )
                .unwrap()
        );
        assert!(
            store
                .complete_burn_check_assessment(
                    &input.durable,
                    &serde_json::to_string(&publication).unwrap(),
                    1000,
                    POLICY.idle_secs
                )
                .unwrap()
        );
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        let restored = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
        assert_eq!(
            restored.durable.input_revision, input.durable.input_revision,
            "{agent}"
        );
        let saved = store
            .burn_check_assessment(&candidate.session.key, CHECK_ID)
            .unwrap()
            .unwrap();
        let persisted: Publication =
            serde_json::from_str(saved.result_json.as_ref().unwrap()).unwrap();
        assert_eq!(
            persisted.snapshot_revision, input.snapshot_revision,
            "{agent}"
        );
        let changed_output = match agent.as_str() {
            "codex" => records.replace("second", "second updated"),
            "claude-code" => records.replace("nine", "nine updated"),
            "pi" => records.replace("return café", "return différent"),
            _ => unreachable!(),
        };
        assert_ne!(changed_output, records, "{agent}");
        let output_candidate = native_sources::publish(
            &store,
            &agent,
            &session,
            format,
            &changed_output,
            directory.path(),
        );
        let output_input =
            load_input(&store, &output_candidate, &ModelCapabilities::jev_default()).unwrap();
        assert_ne!(
            output_input.durable.input_revision, input.durable.input_revision,
            "{agent}"
        );
        let digests = |prepared: &PreparedInput| {
            prepared
                .plan
                .prepared
                .targets
                .values()
                .flat_map(|target| {
                    target
                        .bindings
                        .iter()
                        .map(|binding| binding.output_digest.clone())
                })
                .collect::<BTreeSet<_>>()
        };
        assert_ne!(digests(&output_input), digests(&input), "{agent}");
        let changed = records
            .replace("Keep billing unchanged", "Do not investigate billing")
            .replace("Do not publish", "Do not read other files");
        let current =
            native_sources::publish(&store, &agent, &session, format, &changed, directory.path());
        let updated = load_input(&store, &current, &ModelCapabilities::jev_default()).unwrap();
        assert_ne!(
            updated.durable.input_revision, input.durable.input_revision,
            "{agent}"
        );
        assert!(
            !store
                .complete_burn_check_assessment(
                    &input.durable,
                    &serde_json::to_string(&publication).unwrap(),
                    1001,
                    POLICY.idle_secs
                )
                .unwrap(),
            "{agent}"
        );
    }
}

#[test]
fn original_incomplete_codex_read_fixture_remains_deferred() {
    use crate::scope_creep_worker::tests::native_sources;
    let (agent, session, format, records) = native_sources::sources()[0];
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    for detector in [
        antiburn_local::checks::DetectorId::OverExploring,
        antiburn_local::checks::DetectorId::ScopeCreep,
    ] {
        store.set_check_enabled(detector, true).unwrap();
    }
    store.capture_burn_check_boundaries(&[CHECK_ID], 0).unwrap();
    let candidate =
        native_sources::publish(&store, agent, session, format, records, directory.path());
    let input = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    assert!(input.plan.prepared.targets.is_empty());
    assert!(input.plan.prepared.candidates.is_empty());
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
    assert_eq!(reads[0].extent.limit, Some(2));
    assert_eq!(reads[1].paths, ["/synthetic/billing.rs"]);
    assert_eq!(reads[1].extent.offset, Some(1));
    assert_eq!(reads[1].extent.limit, Some(2));

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
    let source_dir = tempfile::tempdir().unwrap();
    let source_path = source_dir.path().join("opencode.db");
    let source = rusqlite::Connection::open(&source_path).unwrap();
    source.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, time_created INTEGER, time_updated INTEGER);
        CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
        CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
        INSERT INTO session VALUES ('investigation', NULL, 1000, 9000);").unwrap();
    let mut records = vec![(
        "user",
        json!({"type":"text","text":"Fix the parser. Keep billing unchanged."}),
    )];
    for (index, path) in ["/synthetic/parser.rs", "/synthetic/billing.rs"]
        .iter()
        .enumerate()
    {
        records.push(("assistant", json!({"type":"tool","tool":"read","callID":format!("read-{index}"),
            "state":{"status":"completed","input":{"filePath":path,"offset":1,"limit":2},
            "output":format!("<path>{path}</path>\n<type>file</type>\n<content>\n1: recorded code\n2: more code\n\n(End of file - total 2 lines)\n</content>")}})));
    }
    records.push((
        if ending == "user" {
            "user"
        } else {
            "assistant"
        },
        json!({"type":"text","text":"Now explain the result."}),
    ));
    for (index, (role, part)) in records.iter().enumerate() {
        source
            .execute(
                "INSERT INTO message VALUES (?1, 'investigation', ?2, ?2, ?3)",
                rusqlite::params![
                    format!("m{index}"),
                    index + 1001,
                    json!({"role":role}).to_string()
                ],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO part VALUES (?1, ?2, 'investigation', ?3, ?3, ?4)",
                rusqlite::params![
                    format!("p{index}"),
                    format!("m{index}"),
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
                "investigation".into(),
            ),
        )
        .unwrap();
    let fingerprint = format!("sv1:db:{latest}:{rows}");
    let store = directory
        .map(|path| Store::open(path).unwrap())
        .unwrap_or_else(|| {
            Store::open_in_memory(Path::new("/tmp/antiburn-over-exploring-tests")).unwrap()
        });
    store
        .set_check_enabled(antiburn_local::checks::DetectorId::OverExploring, true)
        .unwrap();
    store.capture_burn_check_boundaries(&[CHECK_ID], 0).unwrap();
    let key = SessionKey::new("native", "opencode", "investigation");
    store
        .upsert_sessions(
            &[SessionRecord {
                key: key.clone(),
                source_kind: "providerDb".into(),
                source_label: "synthetic-db".into(),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: None,
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
            16,
        )
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.session.key == key)
        .unwrap();
    (store, candidate)
}

pub(crate) fn reduced(input: &PreparedInput, reason: Reason) -> Assessment {
    let results = input
        .plan
        .work_items
        .iter()
        .map(|item| {
            let selected = input.plan.prepared.targets[&item.id].reason;
            let answers = item
                .questions
                .keys()
                .map(|gate| {
                    let choice = if gate == "sufficiency" {
                        "supported"
                    } else if selected != reason
                        || (reason == Reason::ExcessiveWithinFileReading && gate == "relevance")
                    {
                        "justified"
                    } else {
                        "supported"
                    };
                    (
                        gate.clone(),
                        JevAnswer::Choice {
                            choice: choice.into(),
                            confidence: 1.0,
                            probabilities: ["supported", "justified", "unknown"]
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
                model: input.plan.capabilities.model.clone(),
                answers,
                evidence: input
                    .plan
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
        .collect::<Vec<_>>();
    OverExploringCheck
        .reduce(&input.plan, &results, true)
        .unwrap()
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
            .contains("Keep billing unchanged")
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
fn production_loader_binds_all_reasons_to_observed_reads_and_full_task() {
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
            .contains("Keep billing unchanged")
    );
    for reason in [
        Reason::UnrelatedFiles,
        Reason::ExcessiveFileBreadth,
        Reason::ExcessiveWithinFileReading,
    ] {
        let result = reduced(&input, reason);
        assert!(!result.findings.is_empty(), "{reason:?}");
        let publication = publication(&input, result);
        for finding in &publication.assessment.findings {
            assert_eq!(finding.reason, reason);
            assert!(publishable_finding(finding, &publication));
            let mut broken = finding.clone();
            broken.reads[0].result_id = "not-an-observed-result".into();
            assert!(!publishable_finding(&broken, &publication));
        }
    }
    assert_eq!(
        CHECK.evaluator_revision(),
        "over-exploring-adapter-v2:2:1:7:4"
    );
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
    let result = reduced(&input, Reason::ExcessiveFileBreadth);
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
fn deferred_and_absent_episodes_never_produce_clean_coverage() {
    let (store, candidate) = fixture(None, "assistant");
    let input = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    let result = OverExploringCheck.reduce(&input.plan, &[], true).unwrap();
    assert!(!publication_has_clean_coverage(&result));
    assert!(!result.unassessed.is_empty());
    assert!(result.findings.is_empty());
    assert!(input.plan.prepared.candidates.is_empty());
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
    cursor.result = Some(reduced(&input, Reason::ExcessiveFileBreadth));
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
    cursor.result = Some(reduced(&input, Reason::ExcessiveFileBreadth));
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
    assert_eq!(
        store
            .burn_check_assessment(&input.durable.key, CHECK_ID)
            .unwrap()
            .unwrap()
            .status,
        "completed"
    );
}
