use antiburn_checks::check_store::CheckStore;
use antiburn_checks::compiler::compile;
use antiburn_checks::engine::{CheckEngine, DecisionEvaluator, DecisionFuture};
use antiburn_checks::source::PublishedEvidenceSource;
use antiburn_local::analysis::{
    SourceFormat, ToolCategory, ToolResultChange, ToolResultCompleteness, ToolResultContext,
    ToolResultObservation, ToolResultPosition, ToolResultStatus,
};

use super::*;
use crate::published_evidence_reader::StorePublishedEvidenceReader;
use crate::store::Incarnation;
use crate::store::native_tool_results::source_delivery::opencode_v2::{
    OpenCodeV2AdmittedSource, OpenCodeV2CaptureClaim, OpenCodeV2CaptureStart,
};

fn admitted_source(session_id: &str) -> OpenCodeV2AdmittedSource {
    OpenCodeV2AdmittedSource {
        database_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("synthetic/opencode.db"),
        file_identity: format!("synthetic-{session_id}"),
    }
}

fn admission_cursor(session_id: &str) -> String {
    let source = admitted_source(session_id);
    format!(
        "opencode-admission-v2:{}",
        serde_json::to_string(&(
            &source.database_path,
            session_id,
            &source.file_identity,
            "synthetic-scope"
        ))
        .unwrap()
    )
}

const CHECK: &str = "name: Retry\non:\n  tool_result:\ndecisions:\n  review:\n    model: typesafe/jev-1.13.0\n    input: {activity: '${{ message }}'}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    input: {result: failure}\n";

struct NoEvaluator;

impl DecisionEvaluator for NoEvaluator {
    fn evaluate<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        _: &'a serde_json::Value,
        _: u8,
    ) -> DecisionFuture<'a> {
        panic!("startup tests do not send provider requests")
    }
}

fn fixture() -> (
    tempfile::TempDir,
    Arc<Store>,
    Arc<StoreCheckStore>,
    EffectiveActivation,
    OpenCodeV2CaptureClaim,
) {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open_in_memory(directory.path()).unwrap());
    let scope = ActivationScope {
        environment: "native".into(),
        session_cwd: "/repo".into(),
        run: RunScope::Live,
    };
    let adapter = Arc::new(
        StoreCheckStore::new(store.clone(), "startup-owner".into(), scope.clone()).unwrap(),
    );
    let active = EffectiveActivation {
        id: ActivationId {
            scope,
            definition_id: "retry".into(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: DefinitionScope::Global,
        source_paths: vec![],
        project_root: Some(PathBuf::from("/repo")),
        compiled: Arc::new(compile(CHECK).unwrap()),
        bootstrap_observation_position: ObservationPosition(0),
        bootstrap_publication: PublicationCursor(0),
    };
    adapter
        .activate(
            "retry",
            &LifecycleTransition::Activated(active.clone()),
            Some(&active),
        )
        .unwrap();
    store
        .lock()
        .execute(
            "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
            cwd, updated_at_epoch, first_seen_at, last_seen_at)
         VALUES ('native', 'opencode', 'v2-session', 'providerDb',
            'opencode:v2-session:open_code_v2_sqlite', '/repo', unixepoch(), 'now', 'now')",
            [],
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session SET activity_cursor=?1 WHERE session_id='v2-session'",
            [admission_cursor("v2-session")],
        )
        .unwrap();
    let (incarnation, source_generation) = store
        .lock()
        .query_row(
            "SELECT incarnation, source_generation FROM session",
            [],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap();
    let start = OpenCodeV2CaptureStart {
        key: SessionKey::new("native", "opencode", "v2-session"),
        incarnation: Incarnation(incarnation),
        source_generation,
        source_label: "opencode:v2-session:open_code_v2_sqlite".into(),
        cwd: "/repo".into(),
        admitted_source: admitted_source("v2-session"),
        baseline_sequence: Some(4),
    };
    let claim = store
        .start_opencode_v2_capture_epoch(&[start], "startup-epoch", now_epoch(), 600)
        .unwrap()
        .unwrap()
        .remove(0);
    (directory, store, adapter, active, claim)
}

fn result(sequence: u64) -> ToolResultObservation {
    let position = ToolResultPosition::OpenCodeV2Event {
        sequence,
        item_index: 0,
    };
    ToolResultObservation {
        outcome: None,
        provider_executed: None,
        source_format: SourceFormat::OpenCodeSqliteV2,
        session_id: "v2-session".into(),
        record_id: Some(format!("message-{sequence}")),
        call_id: Some(format!("call-{sequence}")),
        result_id: None,
        position,
        revision: 1,
        change: ToolResultChange::Update,
        tool_name: Some("shell".into()),
        tool_category: Some(ToolCategory::Bash),
        arguments: Some(serde_json::json!({"command": "true"})),
        content: Some("ok".into()),
        status: ToolResultStatus::Success,
        finality: true,
        completeness: ToolResultCompleteness::Complete,
        context: Some(ToolResultContext {
            result_position: position,
            main_session: true,
            complete_through_event: false,
            user_messages: vec![],
        }),
    }
}

fn publish(store: &Store, claim: &OpenCodeV2CaptureClaim, sequence: u64) {
    let mut observation = result(sequence);
    observation.session_id.clone_from(&claim.key.session_id);
    assert!(
        store
            .advance_opencode_v2_capture(claim, sequence, Some(&observation), now_epoch(), 600)
            .unwrap()
    );
}

#[test]
fn v2_startup_writes_empty_attachment_and_never_seeds_current_terminals() {
    let (_directory, store, adapter, active, claim) = fixture();
    publish(&store, &claim, 5);
    let session = adapter
        .startup_sessions(&active, now_epoch())
        .unwrap()
        .remove(0);
    assert!(session.v2);
    let mut events = PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_project(&store, "native", "/repo"),
        20,
    )
    .unwrap()
    .read_batch(PublicationCursor(0))
    .unwrap();
    events
        .events
        .retain(|event| matches!(event, SourceEvent::Observation { .. }));
    assert_eq!(events.events.len(), 1);
    let before = adapter.load_source_progress(&active).unwrap();
    let attached = adapter
        .initialize_recent(&active, &session, &events.events)
        .unwrap()
        .unwrap();
    assert_eq!(attached.observations, 0);
    assert_eq!(attached.candidates, 0);
    let after = adapter.load_source_progress(&active).unwrap();
    assert_eq!(after.publication_cursor, before.publication_cursor);
    assert_eq!(after.observation_position, before.observation_position);
    assert!(after.selection.seen_results.is_empty());
    let branch = &after.selection.branches[&BranchId(claim.key.to_identity_json())];
    assert!(branch.results.is_empty());
    assert!(!branch.coverage_complete);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_session_attachment",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert!(
        adapter
            .initialize_recent(&active, &session, &events.events)
            .unwrap()
            .is_none()
    );
    assert!(
        adapter
            .startup_sessions(&active, now_epoch())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn v2_activation_reader_keeps_first_live_update_until_empty_attachment_commits() {
    let (_directory, store, adapter, active, claim) = fixture();
    publish(&store, &claim, 5);
    let first_update = store
        .native_tool_result_publications_after(0, 20)
        .unwrap()
        .into_iter()
        .find(|event| event.payload.is_some())
        .unwrap()
        .publication_sequence as u64;
    let source = PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_live_project(&store, "native", "/repo")
            .with_activation(&active.id),
        20,
    )
    .unwrap();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    let before_attach = source.read_batch(PublicationCursor(0)).unwrap();
    assert!(before_attach.next.0 < first_update);
    assert!(
        !before_attach
            .events
            .iter()
            .any(|event| matches!(event, SourceEvent::Observation { .. }))
    );
    if !before_attach.events.is_empty() {
        engine.apply_source_page(&active, &before_attach).unwrap();
    }
    let session = adapter
        .startup_sessions(&active, now_epoch())
        .unwrap()
        .remove(0);
    assert_eq!(
        adapter
            .initialize_recent(&active, &session, &[])
            .unwrap()
            .unwrap()
            .candidates,
        0
    );
    engine.refresh_source_progress(&active).unwrap();
    let progress = adapter.load_source_progress(&active).unwrap();
    let ready = source.read_batch(progress.publication_cursor).unwrap();
    assert!(ready.events.iter().any(
        |event| matches!(event, SourceEvent::Observation { cursor, .. } if cursor.0 == first_update)
    ));
    assert_eq!(
        engine
            .apply_source_page(&active, &ready)
            .unwrap()
            .candidates,
        1
    );
    assert!(
        adapter
            .initialized_branches(&active)
            .unwrap()
            .contains(&BranchId(claim.key.to_identity_json()))
    );
    let restart = Arc::new(
        StoreCheckStore::new(store.clone(), "next-owner".into(), active.id.scope.clone()).unwrap(),
    );
    restart.register_activation(&active).unwrap();
    assert!(
        restart
            .startup_sessions(&active, now_epoch())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        restart
            .load_source_progress(&active)
            .unwrap()
            .observation_position,
        ObservationPosition(1)
    );
    assert!(source.read_batch(ready.next).unwrap().events.is_empty());
    publish(&store, &claim, 6);
    let engine = CheckEngine::new(restart.clone(), Arc::new(NoEvaluator));
    let future = source.read_batch(ready.next).unwrap();
    assert_eq!(
        engine
            .apply_source_page(&active, &future)
            .unwrap()
            .candidates,
        1
    );
    assert_eq!(restart.load_recovery(None, 10).unwrap().len(), 2);
}

#[test]
fn v2_startup_receipt_fences_capture_replacement_source_and_scope() {
    for sql in [
        "UPDATE opencode_v2_capture SET capture_epoch = 'replacement'",
        "UPDATE opencode_v2_capture SET claim_fence = claim_fence + 1",
        "UPDATE opencode_v2_capture SET incarnation = incarnation + 1",
        "UPDATE opencode_v2_capture SET source_label = 'other-source'",
        "UPDATE opencode_v2_capture SET cwd = '/other'",
        "UPDATE opencode_v2_capture SET baseline_sequence = baseline_sequence + 1",
        "UPDATE opencode_v2_capture SET capture_epoch = ''",
        "DELETE FROM opencode_v2_capture",
        "UPDATE session SET incarnation = incarnation + 1",
        "UPDATE session SET source_label = 'opencode:v2-session'",
        "UPDATE session SET cwd = '/other'",
        "UPDATE session SET source_kind = 'file'",
        "UPDATE session SET wsl_distro = 'synthetic-distro'",
    ] {
        let (_directory, store, adapter, active, _claim) = fixture();
        let receipt = adapter
            .startup_sessions(&active, now_epoch())
            .unwrap()
            .remove(0);
        store.lock().execute(sql, []).unwrap();
        assert!(
            adapter
                .initialize_recent(&active, &receipt, &[])
                .unwrap()
                .is_none(),
            "{sql}"
        );
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_session_attachment",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0,
            "{sql}"
        );
    }
}

#[test]
fn ineligible_v2_branch_does_not_block_a_healthy_project_branch() {
    for relation in [
        "INSERT INTO session_relation VALUES ('native', 'opencode', 'v2-session', 'forkParent', 'parent', NULL)",
        "INSERT INTO session_relation VALUES ('native', 'opencode', 'parent', 'subagent', 'v2-session', NULL)",
    ] {
        let (_directory, store, adapter, active, claim) = fixture();
        publish(&store, &claim, 5);
        store.lock().execute(relation, []).unwrap();
        assert!(
            adapter
                .startup_sessions(&active, now_epoch())
                .unwrap()
                .is_empty()
        );

        store.lock().execute(
            "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
              cwd, updated_at_epoch, first_seen_at, last_seen_at)
             VALUES ('native', 'opencode', 'healthy', 'providerDb',
              'opencode:healthy:open_code_v2_sqlite', '/repo', unixepoch(), 'now', 'now')", [],
        ).unwrap();
        store
            .lock()
            .execute(
                "UPDATE session SET activity_cursor=?1 WHERE session_id='healthy'",
                [admission_cursor("healthy")],
            )
            .unwrap();
        let (incarnation, source_generation) = store
            .lock()
            .query_row(
                "SELECT incarnation, source_generation FROM session WHERE session_id = 'healthy'",
                [],
                |row| Ok((Incarnation(row.get(0)?), row.get(1)?)),
            )
            .unwrap();
        let healthy = store
            .start_opencode_v2_capture_epoch(
                &[OpenCodeV2CaptureStart {
                    key: SessionKey::new("native", "opencode", "healthy"),
                    incarnation,
                    source_generation,
                    source_label: "opencode:healthy:open_code_v2_sqlite".into(),
                    cwd: "/repo".into(),
                    admitted_source: admitted_source("healthy"),
                    baseline_sequence: Some(4),
                }],
                "healthy-epoch",
                now_epoch(),
                600,
            )
            .unwrap()
            .unwrap()
            .remove(0);
        let startup = adapter.startup_sessions(&active, now_epoch()).unwrap();
        assert_eq!(startup.len(), 1);
        assert_eq!(startup[0].key.session_id, "healthy");
        adapter
            .initialize_recent(&active, &startup[0], &[])
            .unwrap()
            .unwrap();
        publish(&store, &healthy, 5);

        let source = PublishedEvidenceSource::new(
            StorePublishedEvidenceReader::for_live_project(&store, "native", "/repo")
                .with_activation(&active.id),
            100,
        )
        .unwrap();
        let page = source.read_batch(PublicationCursor(0)).unwrap();
        assert!(!page.has_more);
        assert!(
            page.events
                .iter()
                .any(|event| matches!(event, SourceEvent::Ignored { .. }))
        );
        let branches: Vec<_> = page
            .events
            .iter()
            .filter_map(|event| match event {
                SourceEvent::Observation { observation, .. } => Some(&observation.branch),
                _ => None,
            })
            .collect();
        assert_eq!(branches, [&BranchId(healthy.key.to_identity_json())]);
    }
}

#[test]
fn v2_startup_accepts_ordinary_generation_changes_and_capture_progress() {
    let (_directory, store, adapter, active, claim) = fixture();
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1, updated_at_epoch = 0",
            [],
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session SET activity_cursor=?1 WHERE session_id='healthy-session'",
            [admission_cursor("healthy-session")],
        )
        .unwrap();
    let receipt = adapter
        .startup_sessions(&active, now_epoch())
        .unwrap()
        .remove(0);
    publish(&store, &claim, 5);
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1",
            [],
        )
        .unwrap();
    assert!(
        adapter
            .initialize_recent(&active, &receipt, &[])
            .unwrap()
            .is_some()
    );
}

#[test]
fn v2_empty_attachment_accepts_unleased_receipt_without_live_report_readiness() {
    for lease in ["NULL", "unixepoch() - 1"] {
        let (_directory, store, adapter, active, claim) = fixture();
        let receipt = adapter
            .startup_sessions(&active, now_epoch())
            .unwrap()
            .remove(0);
        store
            .lock()
            .execute(
                &format!("UPDATE opencode_v2_capture SET lease_expires_at_epoch = {lease}"),
                [],
            )
            .unwrap();
        store.lock().execute("UPDATE session SET updated_at_epoch = 0, source_generation = source_generation + 1", []).unwrap();
        let ended = adapter
            .startup_sessions(&active, now_epoch())
            .unwrap()
            .remove(0);
        assert_eq!(receipt.receipt, ended.receipt);
        let outcome = adapter
            .initialize_recent(&active, &receipt, &[])
            .unwrap()
            .unwrap();
        assert_eq!(outcome.observations, 0);
        assert_eq!(outcome.candidates, 0);
        let source = PublishedEvidenceSource::new(
            StorePublishedEvidenceReader::for_live_project(&store, "native", "/repo")
                .with_activation(&active.id),
            20,
        )
        .unwrap();
        let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
        let batch = source.read_batch(PublicationCursor(0)).unwrap();
        engine.apply_source_page(&active, &batch).unwrap();
        assert!(
            adapter
                .initialized_branches(&active)
                .unwrap()
                .contains(&BranchId(claim.key.to_identity_json()))
        );
        let ready = crate::insights_report::installed_checks::source_ready_sql(1, 2, 3);
        let live_ready: bool = store
            .lock()
            .query_row(
                &format!(
                    "SELECT ({ready}) FROM session s
                LEFT JOIN session_evidence e USING(environment_key, agent, session_id)"
                ),
                params![
                    antiburn_local::analysis::PARSER_REVISION,
                    antiburn_local::analysis::ANALYZER_REVISION,
                    antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                ],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!live_ready);
    }
}

#[test]
fn v2_disconnect_before_attachment_drains_publications_and_unblocks_healthy_session() {
    let (_directory, store, adapter, active, claim) = fixture();
    publish(&store, &claim, 5);
    assert_eq!(
        store
            .mark_opencode_v2_capture_gap(
                std::slice::from_ref(&claim),
                "event-feed-ended",
                now_epoch()
            )
            .unwrap(),
        1
    );
    store
        .lock()
        .execute(
            "UPDATE session SET updated_at_epoch = 0, source_generation = source_generation + 1",
            [],
        )
        .unwrap();
    store
        .lock()
        .execute(
            "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
            cwd, updated_at_epoch, first_seen_at, last_seen_at)
         VALUES ('native', 'opencode', 'healthy-session', 'providerDb',
            'opencode:healthy-session:open_code_v2_sqlite', '/repo', unixepoch(), 'now', 'now')",
            [],
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session SET activity_cursor = ?1
              WHERE environment_key = 'native' AND agent = 'opencode'
                AND session_id = 'healthy-session'",
            [admission_cursor("healthy-session")],
        )
        .unwrap();
    let (healthy_incarnation, healthy_source_generation) = store
        .lock()
        .query_row(
            "SELECT incarnation, source_generation FROM session
              WHERE environment_key = 'native' AND agent = 'opencode'
                AND session_id = 'healthy-session'",
            [],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap();
    let healthy = store
        .start_opencode_v2_capture_epoch(
            &[OpenCodeV2CaptureStart {
                key: SessionKey::new("native", "opencode", "healthy-session"),
                incarnation: Incarnation(healthy_incarnation),
                source_generation: healthy_source_generation,
                source_label: "opencode:healthy-session:open_code_v2_sqlite".into(),
                cwd: "/repo".into(),
                admitted_source: admitted_source("healthy-session"),
                baseline_sequence: Some(4),
            }],
            "healthy-epoch",
            now_epoch(),
            600,
        )
        .unwrap()
        .unwrap()
        .remove(0);
    publish(&store, &healthy, 5);
    let source = PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_live_project(&store, "native", "/repo")
            .with_activation(&active.id),
        20,
    )
    .unwrap();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    let waiting = source.read_batch(PublicationCursor(0)).unwrap();
    assert!(
        !waiting
            .events
            .iter()
            .any(|event| matches!(event, SourceEvent::Observation { .. }))
    );
    if !waiting.events.is_empty() {
        engine.apply_source_page(&active, &waiting).unwrap();
    }
    let sessions = adapter.startup_sessions(&active, now_epoch()).unwrap();
    assert_eq!(sessions.len(), 2);
    for session in sessions {
        assert_eq!(
            adapter
                .initialize_recent(&active, &session, &[])
                .unwrap()
                .unwrap()
                .observations,
            0
        );
    }
    engine.refresh_source_progress(&active).unwrap();
    let ready = source.read_batch(waiting.next).unwrap();
    let outcome = engine.apply_source_page(&active, &ready).unwrap();
    assert_eq!(outcome.observations, 2);
    assert_eq!(outcome.candidates, 1);
    assert!(source.read_batch(ready.next).unwrap().events.is_empty());
    let pending = adapter.load_recovery(None, 10).unwrap();
    assert_eq!(pending.len(), 1);
    let RecoveryWork::Pending(candidate) = &pending[0].work else {
        panic!("healthy evidence must retain one pending candidate")
    };
    assert_eq!(candidate.branch, BranchId(healthy.key.to_identity_json()));
    let initialized = adapter.initialized_branches(&active).unwrap();
    assert!(initialized.contains(&BranchId(claim.key.to_identity_json())));
    assert!(initialized.contains(&BranchId(healthy.key.to_identity_json())));
}
