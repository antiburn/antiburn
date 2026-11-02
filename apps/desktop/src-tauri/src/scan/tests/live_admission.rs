use super::*;
use crate::check_store::StoreCheckStore;
use crate::insights_worker::journal::tests::{SyntheticJournal, run_page};
use crate::published_evidence_reader::StorePublishedEvidenceReader;
use antiburn_checks::check_store::CheckStore;
use antiburn_checks::decision::{DecisionAttempt, DecisionInput, Question};
use antiburn_checks::definition_lifecycle::{EffectiveActivation, LifecycleTransition};
use antiburn_checks::discovery::DefinitionScope;
use antiburn_checks::domain::{
    ActivationId, ActivationScope, FrozenCandidate, Generation, ObservationPosition, RunScope,
};
use antiburn_checks::engine::{
    CheckEngine, DecisionEvaluator, DecisionFuture, PreparationValidationPurpose, RunOutcome,
    StartOutcome,
};
use antiburn_checks::expression_eval::EvaluationBudget;
use antiburn_checks::fit::{FitReport, PreparationFailure};
use antiburn_checks::prepared_assessment::{PreparedAssessment, PreparedBlock};
use antiburn_checks::results::{ChoiceResult, DecisionResult, NamedAnswer, QuestionResult};
use antiburn_checks::source::{PublicationCursor, PublishedEvidenceSource};

fn native_source(home: &std::path::Path, id: &str) -> (SyntheticJournal, SessionLog) {
    let source = SyntheticJournal::new(16);
    source.connection.execute_batch(
        "ALTER TABLE session ADD COLUMN directory TEXT; ALTER TABLE session ADD COLUMN title TEXT;"
    ).unwrap();
    source.connection.execute(
        "UPDATE session SET id = ?1, directory = ?2, title = 'Initial title', time_updated = ?3",
        rusqlite::params![id, home.to_str(), unix_now() * 1000],
    ).unwrap();
    for table in ["message", "part"] {
        source
            .connection
            .execute(&format!("UPDATE {table} SET session_id = ?1"), [id])
            .unwrap();
    }
    for table in ["event", "event_sequence"] {
        source
            .connection
            .execute(&format!("UPDATE {table} SET aggregate_id = ?1"), [id])
            .unwrap();
    }
    let log = SessionLog {
        agent_type: AgentKind::OpenCode,
        source: SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path: source.path.clone(),
            session_id: id.into(),
            source_format: Some(antiburn_local::analysis::SourceFormat::OpenCodeSqliteV1),
        },
        updated_at: Some(unix_now()),
        environment: DiscoveryEnvironment::Native,
    };
    (source, log)
}

fn hash_barrier(
    logs: &[SessionLog],
    stage: PreparationStage,
) -> (Arc<PreparationHooks>, mpsc::UnboundedReceiver<String>) {
    let (entered, arrivals) = mpsc::unbounded_channel();
    (
        Arc::new(PreparationHooks {
            stage,
            blocked: logs.iter().map(SessionLog::source_label).collect(),
            panic: HashSet::new(),
            panic_once: AtomicBool::new(false),
            entered,
            release: Semaphore::new(0),
        }),
        arrivals,
    )
}

async fn wait_for_title(store: &Store, id: &str, title: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if store
                .session(&SessionKey::new("native", "opencode", id))
                .unwrap()
                .is_some_and(|record| record.title.as_deref() == Some(title))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("metadata commits while the strict hash stays blocked");
}

#[tokio::test]
async fn changing_revisions_do_not_repeat_hashes_or_hold_the_next_scan() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let fixtures = [
        native_source(home.path(), "native-session"),
        native_source(home.path(), "other-session"),
    ];
    let logs = fixtures
        .iter()
        .map(|(_, log)| log.clone())
        .collect::<Vec<_>>();
    let (hooks, mut arrivals) = hash_barrier(&logs, PreparationStage::FingerprintCommit);
    let policy = policy(home.path(), Some(hooks.clone()));
    let mut admission = writer(&store);
    let AdmissionIo::Test { now, .. } = &admission.io else {
        unreachable!()
    };
    let clock = now.clone();
    clock.store(1000, Ordering::Release);
    let (sender, finite) = mpsc::channel(1);
    let (live_sender, live) = mpsc::channel(1);
    let (queued_sender, queued_scan) = mpsc::channel(1);
    sender.send(batch(logs.clone())).await.unwrap();
    drop(sender);
    let control = async {
        let first = tokio::time::timeout(Duration::from_secs(5), arrivals.recv())
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), arrivals.recv())
            .await
            .unwrap()
            .unwrap();
        queued_sender
            .send(batch(vec![fixture_log(home.path(), "queued-full-scan")]))
            .await
            .unwrap();
        drop(queued_sender);
        for revision in 1..=4 {
            let title = format!("Revision {revision}");
            for (source, _) in &fixtures {
                source
                    .connection
                    .execute("UPDATE session SET title = ?1", [&title])
                    .unwrap();
            }
            live_sender.send(batch(logs.clone())).await.unwrap();
            for id in ["native-session", "other-session"] {
                wait_for_title(&store, id, &title).await;
            }
        }
        hooks.release.add_permits(1);
        let first_index = logs
            .iter()
            .position(|log| log.source_label() == first)
            .unwrap();
        let SessionSource::ProviderDb { session_id, .. } = &logs[first_index].source else {
            unreachable!()
        };
        clock.store(1002, Ordering::Release);
        fixtures[first_index]
            .0
            .connection
            .execute("UPDATE session SET title = 'Still changing'", [])
            .unwrap();
        live_sender
            .send(batch(vec![logs[first_index].clone()]))
            .await
            .unwrap();
        wait_for_title(&store, session_id, "Still changing").await;
        assert!(
            arrivals.try_recv().is_err(),
            "an expired retry does not start a second hash in this pass"
        );
        hooks.release.add_permits(1);
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(run(finite, live, None, &policy, &mut admission), control)
    })
    .await
    .expect("changing sources cannot keep the finite owner running");
    admission.finish().unwrap();
    assert!(
        arrivals.try_recv().is_err(),
        "each source gets at most one hash attempt"
    );
    assert_eq!(
        store.next_source_fingerprint_attempt().unwrap(),
        Some(0),
        "a changed admission revision becomes eligible without the old retry delay"
    );
    for id in ["native-session", "other-session"] {
        let retries: i64 = store
            .lock()
            .query_row(
                "SELECT fingerprint_retry_count FROM session WHERE session_id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retries, 0, "stale hashes do not restore retry backoff");
    }
    let mut queued = writer(&store);
    let AdmissionIo::Test { now, .. } = &queued.io else {
        unreachable!()
    };
    now.store(1002, Ordering::Release);
    let (_sender, live) = mpsc::channel(1);
    let next_policy = super::policy(home.path(), None);
    tokio::time::timeout(
        Duration::from_secs(5),
        run(queued_scan, live, None, &next_policy, &mut queued),
    )
    .await
    .unwrap();
    queued.finish().unwrap();
    assert!(
        store
            .session(&SessionKey::new(
                "native",
                AgentKind::Claude.slug(),
                "queued-full-scan"
            ))
            .unwrap()
            .is_some()
    );
    assert_eq!(store.next_source_fingerprint_attempt().unwrap(), Some(1302));
    recover_at(&store, home.path(), 1302).await;
    for id in ["native-session", "other-session"] {
        assert!(
            store
                .pending_source_fingerprint(&SessionKey::new("native", "opencode", id))
                .unwrap()
                .is_none()
        );
    }
}

#[derive(Default)]
struct MockDecision {
    inputs: Mutex<Vec<serde_json::Value>>,
}

impl DecisionEvaluator for MockDecision {
    fn prepare(
        &self,
        activation: &EffectiveActivation,
        inputs: Vec<DecisionInput>,
        evaluation_budget: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        let prepared = PreparedAssessment::new(
            "live-admission-test-v1".into(),
            evaluation_budget,
            0,
            inputs
                .into_iter()
                .map(|input| PreparedBlock {
                    decision: input.decision,
                    original_input: input.input.clone(),
                    input: input.input,
                    request: serde_json::json!({"test": true}),
                    report: FitReport::default(),
                })
                .collect(),
        )
        .unwrap();
        prepared
            .validate_for_declarations(&activation.compiled.decisions)
            .unwrap();
        Ok(prepared)
    }

    fn validate_prepared(
        &self,
        activation: &EffectiveActivation,
        prepared: &PreparedAssessment,
        _: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        assert_eq!(prepared.profile, "live-admission-test-v1");
        prepared
            .validate_for_declarations(&activation.compiled.decisions)
            .unwrap();
        Ok(())
    }

    fn evaluate_prepared<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        prepared: &'a PreparedAssessment,
        attempt: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        Box::pin(async move {
            let input = &prepared
                .decisions
                .iter()
                .find(|block| block.decision == attempt.decision)
                .unwrap()
                .input;
            self.inputs.lock().unwrap().push(input.clone());
            let decision = activation
                .compiled
                .decisions
                .iter()
                .find(|decision| decision.name == attempt.decision)
                .unwrap();
            let [question] = decision.outputs.as_slice() else {
                panic!("the live admission fixture requires one question");
            };
            let Question::Choice { criteria, .. } = &question.question else {
                panic!("the live admission fixture requires a Choice question");
            };
            let labels = criteria
                .iter()
                .map(|(label, _)| label.clone())
                .collect::<Vec<_>>();
            Ok(DecisionResult::new(
                decision,
                &activation.compiled.question_requirements(),
                vec![
                    NamedAnswer::new(
                        &question.name,
                        QuestionResult::Choice(
                            ChoiceResult::new(
                                &question.question,
                                "stuck",
                                Some(
                                    labels
                                        .iter()
                                        .map(|label| {
                                            (
                                                label.clone(),
                                                if label == "stuck" { 0.99 } else { 0.005 },
                                            )
                                        })
                                        .collect(),
                                ),
                                Some(0.99),
                            )
                            .unwrap(),
                        ),
                    )
                    .unwrap(),
                ],
                attempt.number,
            )
            .unwrap())
        })
    }
}

fn catch_up(
    store: &Store,
    adapter: &StoreCheckStore,
    engine: &CheckEngine<StoreCheckStore, MockDecision>,
    active: &EffectiveActivation,
) {
    let cwd = active.id.scope.session_cwd.as_str();
    let source = PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_live_project(store, "native", cwd)
            .with_activation(&active.id)
            .with_repository_root(Some(cwd)),
        64,
    )
    .unwrap();
    let mut drained = false;
    for _ in 0..4 {
        let progress = adapter.load_source_progress(active).unwrap();
        let page = source.read_batch(progress.publication_cursor).unwrap();
        if !page.events.is_empty() {
            engine.apply_source_page(active, &page).unwrap();
        }
        if !page.has_more {
            drained = true;
            break;
        }
    }
    assert!(drained);
    for session in adapter.startup_sessions(active, unix_now() - 180).unwrap() {
        let events = StorePublishedEvidenceReader::for_project(store, "native", cwd)
            .with_repository_root(Some(cwd))
            .recent_results_before(&session.key, session.before_publication)
            .unwrap();
        adapter
            .initialize_recent(active, &session, &events)
            .unwrap();
        engine.refresh_source_progress(active).unwrap();
    }
    engine.recover_pending().unwrap();
}

#[tokio::test]
async fn three_post_baseline_results_write_a_mocked_status_while_fingerprint_is_stalled() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().canonicalize().unwrap();
    let store = Arc::new(Store::open_in_memory(&root).unwrap());
    store
        .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "true")
        .unwrap();
    let (mut source, log) = native_source(&root, "native-session");
    let scope = ActivationScope {
        environment: "native".into(),
        session_cwd: root.to_str().unwrap().into(),
        run: RunScope::Live,
    };
    let adapter = Arc::new(
        StoreCheckStore::new(store.clone(), "live-admission-test".into(), scope.clone()).unwrap(),
    );
    let decision = Arc::new(MockDecision::default());
    let engine = CheckEngine::new(adapter.clone(), decision.clone());
    let active = EffectiveActivation {
        id: ActivationId {
            scope,
            definition_id: "failed-retry-loop".into(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: DefinitionScope::Global,
        source_paths: vec![],
        project_root: Some(root.clone()),
        compiled: Arc::new(
            antiburn_checks::compiler::compile(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../crates/antiburn-checks/tests/fixtures/retry-review.yml"
            )))
            .unwrap(),
        ),
        bootstrap_observation_position: ObservationPosition(0),
        bootstrap_publication: PublicationCursor(0),
    };
    engine
        .persist_activation(
            "failed-retry-loop",
            &LifecycleTransition::Activated(active.clone()),
            Some(&active),
        )
        .unwrap();
    let (hooks, mut arrivals) =
        hash_barrier(std::slice::from_ref(&log), PreparationStage::Fingerprint);
    let policy = policy(&root, Some(hooks.clone()));
    let mut admission = writer(&store);
    let (sender, finite) = mpsc::channel(1);
    let (_live_sender, live) = mpsc::channel(1);
    sender.send(batch(vec![log])).await.unwrap();
    drop(sender);
    let control = async {
        tokio::time::timeout(Duration::from_secs(5), arrivals.recv())
            .await
            .unwrap()
            .unwrap();
        let now = unix_now();
        assert!(run_page(&store, source.source(), now).await);
        catch_up(&store, &adapter, &engine, &active);
        assert!(matches!(
            engine.start_one_due(&active, now as u64 * 1000).unwrap(),
            StartOutcome::NotDue
        ));
        for index in 0..3 {
            source.start_result(index);
            source.result(index);
            assert!(run_page(&store, source.source(), now + index as i64 + 1).await);
            catch_up(&store, &adapter, &engine, &active);
            if index < 2 {
                assert!(matches!(
                    engine.start_one_due(&active, now as u64 * 1000).unwrap(),
                    StartOutcome::NotDue
                ));
            }
        }
        let StartOutcome::Started(job) = engine.start_one_due(&active, now as u64 * 1000).unwrap()
        else {
            panic!("three new terminal results must produce a decision");
        };
        assert_eq!(job.await.unwrap().outcome, RunOutcome::Accepted);
        let inputs = decision.inputs.lock().unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0]["activity"].as_array().unwrap().len(), 3);
        for index in 0..3 {
            assert!(
                inputs[0]["activity"]
                    .to_string()
                    .contains(&format!("Parser failure {index}"))
            );
        }
        drop(inputs);
        let status: String = store
            .lock()
            .query_row(
                "SELECT status FROM burn_check_current_status WHERE check_id = 'failed-retry-loop'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "failure");
        let key = SessionKey::new("native", "opencode", "native-session");
        assert!(store.pending_source_fingerprint(&key).unwrap().is_some());
        assert!(
            store
                .session(&key)
                .unwrap()
                .unwrap()
                .source_fingerprint
                .is_none()
        );
        assert!(
            store
                .claim_next_evidence(&["opencode"], now, 60)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            hooks.release.available_permits(),
            0,
            "the full hash is still blocked"
        );
        hooks.release.add_permits(1);
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(run(finite, live, None, &policy, &mut admission), control)
    })
    .await
    .unwrap();
    admission.finish().unwrap();
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT status FROM burn_check_current_status WHERE check_id = 'failed-retry-loop'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "failure"
    );
}
