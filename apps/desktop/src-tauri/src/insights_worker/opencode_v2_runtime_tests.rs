use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context as TaskContext, Poll, Waker},
};

use antiburn_checks::{
    actions::ChoiceResult,
    check_store::{CandidateClaim, CheckStore, EvaluatedCandidate, RecoveryWork},
    compiler::compile,
    definition_lifecycle::{EffectiveActivation, LifecycleTransition},
    discovery::DefinitionScope,
    domain::{
        ActivationId, ActivationScope, BranchId, FrozenCandidate, Generation, ObservationPosition,
        RunScope,
    },
    engine::{
        CheckEngine, DecisionEvaluator, DecisionFuture, RunOutcome, RunSettlement, StartOutcome,
    },
    source::{PublicationCursor, PublishedEvidenceSource, SourceEvent},
};
use serde_json::json;

use super::*;
use crate::{
    check_store::StoreCheckStore, published_evidence_reader::StorePublishedEvidenceReader,
    store::SessionRecord,
};

const FAILED_RETRY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../docs/burn-check/examples/failed-retry-loop.yml"
));
const OVER_EXPLORING: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../docs/burn-check/examples/over-exploring.yml"
));
const SCOPE_CREEP: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../docs/burn-check/examples/scope-creep.yml"
));
const SINGLE_RESULT: &str = "name: Result\non:\n  tool_result:\n  cooldown: 0s\ndecisions:\n  review:\n    model: typesafe/jev-1.13.0\n    input:\n      content: ${{ message.tool.result.content }}\n    output:\n      verdict:\n        type: choice\n        question: Does this result show a failure?\n        criteria: {yes: A failure., no: No failure.}\nactions:\n  - type: status\n    input: {result: failure}\n";

#[derive(Default)]
struct TestEvaluator {
    calls: Mutex<Vec<(FrozenCandidate, Value, u8)>>,
    hold: AtomicBool,
    selected: Mutex<Option<String>>,
}

impl DecisionEvaluator for TestEvaluator {
    fn evaluate<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        candidate: &'a FrozenCandidate,
        input: &'a Value,
        attempt: u8,
    ) -> DecisionFuture<'a> {
        self.calls
            .lock()
            .unwrap()
            .push((candidate.clone(), input.clone(), attempt));
        let decision = &activation.compiled.decision;
        let labels: Vec<_> = decision.criteria.iter().map(|(id, _)| id.clone()).collect();
        let selected = self
            .selected
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| labels[0].clone());
        let probabilities = labels
            .iter()
            .map(|label| {
                (
                    label.clone(),
                    if label == &selected {
                        0.99
                    } else {
                        0.01 / (labels.len() - 1) as f64
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let choice = ChoiceResult::validate(
            &decision.name,
            &decision.question_id,
            &labels,
            &selected,
            probabilities,
        )
        .unwrap();
        Box::pin(std::future::poll_fn(move |_| {
            if self.hold.load(Ordering::SeqCst) {
                Poll::Pending
            } else {
                Poll::Ready(Ok(EvaluatedCandidate {
                    attempts: attempt,
                    choices: vec![choice.clone()],
                }))
            }
        }))
    }
}

pub(super) struct Runtime {
    pub(super) store: Arc<Store>,
    adapter: Arc<StoreCheckStore>,
    active: EffectiveActivation,
    evaluator: Arc<TestEvaluator>,
    engine: CheckEngine<StoreCheckStore, TestEvaluator>,
}

impl Runtime {
    #[cfg(unix)]
    pub(super) fn single_result(path: &Path) -> Self {
        Self::new(path, SINGLE_RESULT)
    }

    pub(super) fn evaluator_calls(&self) -> usize {
        self.evaluator.calls.lock().unwrap().len()
    }

    #[cfg(unix)]
    pub(super) fn restart(&self, path: &Path) -> Self {
        Self::reopen(path, self.active.clone(), self.evaluator.clone())
    }

    #[cfg(unix)]
    pub(super) fn abandon_dispatched(&self, now: u64) {
        self.evaluator.hold.store(true, Ordering::SeqCst);
        let branches = self.adapter.initialized_branches(&self.active).unwrap();
        let StartOutcome::Started(mut task) = self
            .engine
            .start_one_due_for_branches(&self.active, now, &branches)
            .unwrap()
        else {
            panic!("the V2 candidate must dispatch before capture is interrupted");
        };
        assert!(
            task.as_mut()
                .poll(&mut TaskContext::from_waker(Waker::noop()))
                .is_pending()
        );
        drop(task);
        self.evaluator.hold.store(false, Ordering::SeqCst);
    }
    fn new(path: &Path, source: &str) -> Self {
        let store = Arc::new(Store::open(path).unwrap());
        let scope = ActivationScope {
            environment: "native".into(),
            session_cwd: "/repo".into(),
            run: RunScope::Live,
        };
        let adapter = Arc::new(
            StoreCheckStore::new(store.clone(), "v2-runtime".into(), scope.clone()).unwrap(),
        );
        let active = EffectiveActivation {
            id: ActivationId {
                scope,
                definition_id: "v2-check".into(),
                generation: Generation(1),
            },
            revision: [1; 32],
            definition_scope: DefinitionScope::Global,
            source_paths: vec![],
            project_root: Some(PathBuf::from("/repo")),
            compiled: Arc::new(compile(source).unwrap()),
            bootstrap_observation_position: ObservationPosition(0),
            bootstrap_publication: PublicationCursor(0),
        };
        adapter
            .activate(
                &active.id.definition_id,
                &LifecycleTransition::Activated(active.clone()),
                Some(&active),
            )
            .unwrap();
        let evaluator = Arc::new(TestEvaluator::default());
        let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
        Self {
            store,
            adapter,
            active,
            evaluator,
            engine,
        }
    }

    fn reopen(path: &Path, active: EffectiveActivation, evaluator: Arc<TestEvaluator>) -> Self {
        let store = Arc::new(Store::open(path).unwrap());
        let adapter = Arc::new(
            StoreCheckStore::new(
                store.clone(),
                "v2-restarted".into(),
                active.id.scope.clone(),
            )
            .unwrap(),
        );
        adapter.register_activation(&active).unwrap();
        let engine = CheckEngine::new(adapter.clone(), evaluator.clone());
        Self {
            store,
            adapter,
            active,
            evaluator,
            engine,
        }
    }

    pub(super) fn sync(&self) -> usize {
        let source = PublishedEvidenceSource::new(
            StorePublishedEvidenceReader::for_live_project(&self.store, "native", "/repo")
                .with_activation(&self.active.id)
                .with_repository_root(Some("/repo")),
            2,
        )
        .unwrap();
        let mut candidates = 0;
        let mut drained = false;
        for _ in 0..100 {
            let progress = self.adapter.load_source_progress(&self.active).unwrap();
            let page = source.read_batch(progress.publication_cursor).unwrap();
            if !page.events.is_empty() {
                candidates += self
                    .engine
                    .apply_source_page(&self.active, &page)
                    .unwrap()
                    .candidates;
            }
            if !page.has_more {
                drained = true;
                break;
            }
        }
        assert!(drained, "the bounded live publication read must finish");
        for session in self
            .adapter
            .startup_sessions(&self.active, unix_now() - 180)
            .unwrap()
        {
            let events = StorePublishedEvidenceReader::for_project(&self.store, "native", "/repo")
                .with_repository_root(Some("/repo"))
                .recent_results(&session.key)
                .unwrap();
            candidates += self
                .adapter
                .initialize_recent(&self.active, &session, &events)
                .unwrap()
                .expect("the current V2 capture receipt must permit attachment")
                .candidates;
            self.engine.refresh_source_progress(&self.active).unwrap();
        }
        self.engine.recover_pending().unwrap();
        candidates
    }

    pub(super) fn run(&self, now: u64) -> RunSettlement {
        let branches = self.adapter.initialized_branches(&self.active).unwrap();
        let StartOutcome::Started(mut task) = self
            .engine
            .start_one_due_for_branches(&self.active, now, &branches)
            .unwrap()
        else {
            panic!("an attached V2 candidate must be due");
        };
        let Poll::Ready(result) = task
            .as_mut()
            .poll(&mut TaskContext::from_waker(Waker::noop()))
        else {
            panic!("the test evaluator must settle without external I/O");
        };
        result.unwrap()
    }

    pub(super) fn assert_idle(&self, now: u64) {
        let branches = self.adapter.initialized_branches(&self.active).unwrap();
        assert!(matches!(
            self.engine
                .start_one_due_for_branches(&self.active, now, &branches)
                .unwrap(),
            StartOutcome::NotDue
        ));
    }

    fn pending(&self) -> Vec<FrozenCandidate> {
        self.adapter
            .load_recovery(None, 100)
            .unwrap()
            .into_iter()
            .filter_map(|record| match record.work {
                RecoveryWork::Pending(candidate) => Some(candidate),
                _ => None,
            })
            .collect()
    }

    pub(super) fn count(&self, table: &str) -> u64 {
        self.store
            .lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }
}

fn record(session: &str) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", "opencode", session),
        source_kind: "providerDb".into(),
        source_label: format!("opencode:{session}:open_code_v2_sqlite"),
        wsl_distro: None,
        title: Some("Synthetic V2 runtime session".into()),
        title_source: Some("explicit".into()),
        cwd: Some("/repo".into()),
        surface: "cli".into(),
        updated_at_epoch: Some(unix_now()),
        activity_cursor: format!(
            "opencode-admission-v2:{}",
            json!([
                Path::new(env!("CARGO_MANIFEST_DIR")).join("synthetic/opencode.db"),
                session,
                format!("synthetic-identity-{session}"),
                "synthetic-scope"
            ])
        ),
        activity_source: "event".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: Some("synthetic-v2".into()),
    }
}

#[test]
fn changed_scope_sessions_isolates_missing_and_changed_sessions() {
    let previous = vec![record("ses_a"), record("ses_b")];
    let current = vec![record("ses_b")];
    assert_eq!(
        changed_scope_sessions(&previous, &current),
        std::collections::HashSet::from(["ses_a".to_owned()])
    );

    let mut moved = record("ses_b");
    moved.cwd = Some("/moved".into());
    let current = vec![record("ses_a"), moved];
    assert_eq!(
        changed_scope_sessions(&previous, &current),
        std::collections::HashSet::from(["ses_b".to_owned()])
    );
}

struct Capture {
    claims: HashMap<String, OpenCodeV2CaptureClaim>,
    joiners: HashMap<String, OpenCodeV2EventJoiner>,
    sequences: HashMap<String, u64>,
}

impl Capture {
    fn start(store: &Store, sessions: &[&str], baseline: Option<u64>) -> Self {
        let records: Vec<_> = sessions.iter().map(|session| record(session)).collect();
        store
            .upsert_sessions(&records, &crate::agents::evidence_cohort())
            .unwrap();
        let baseline = sessions
            .iter()
            .map(|session| ((*session).to_owned(), baseline))
            .collect();
        let starts = capture_starts(store, &records, &baseline).unwrap();
        let claims = store
            .start_opencode_v2_capture_epoch(&starts, "runtime-epoch-a", unix_now(), LEASE_SECS)
            .unwrap()
            .expect("a recent main V2 session in this Live scope must be admitted");
        Self {
            sequences: claims
                .iter()
                .map(|claim| {
                    (
                        claim.key.session_id.clone(),
                        claim.baseline_sequence.unwrap_or(0),
                    )
                })
                .collect(),
            joiners: sessions
                .iter()
                .map(|session| ((*session).to_owned(), OpenCodeV2EventJoiner::new()))
                .collect(),
            claims: claims
                .into_iter()
                .map(|claim| (claim.key.session_id.clone(), claim))
                .collect(),
        }
    }

    fn resume(&mut self, store: &Store, session: &str) {
        let baseline = self.sequences[session] + 20;
        let starts = capture_starts(
            store,
            &[record(session)],
            &HashMap::from([(session.to_owned(), Some(baseline))]),
        )
        .unwrap();
        let claim = store
            .start_opencode_v2_capture_epoch(&starts, "runtime-epoch-b", unix_now(), LEASE_SECS)
            .unwrap()
            .unwrap()
            .remove(0);
        self.claims.insert(session.to_owned(), claim);
        self.joiners
            .insert(session.to_owned(), OpenCodeV2EventJoiner::new());
        self.sequences.insert(session.to_owned(), baseline);
    }

    fn emit(&mut self, store: &Store, session: &str, kind: &str, extra: Value) -> ProcessResult {
        let sequence = self.sequences.get_mut(session).unwrap();
        *sequence += 1;
        let sequence = *sequence;
        let version = if matches!(kind, "session.tool.success" | "session.tool.failed") {
            2
        } else {
            1
        };
        let mut data = json!({"sessionID": session});
        data.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let event = json!({
            "id": format!("evt_{session}_{sequence}"),
            "type": kind,
            "created": 1_000,
            "durable": {"aggregateID": session, "seq": sequence, "version": version},
            "data": data,
        });
        process_captured_event(
            CapturedEvent {
                session_id: session.into(),
                sequence,
                kind: BoundaryKind::Live,
                event,
            },
            &mut self.joiners,
            &self.claims,
            store,
        )
        .unwrap()
    }

    fn request(&mut self, store: &Store, session: &str, id: &str, text: &str) {
        self.enqueue(store, session, id, text);
        assert_eq!(
            self.emit(
                store,
                session,
                "session.inbox.delivered",
                json!({"inboxID": id}),
            ),
            ProcessResult::Advanced
        );
    }

    fn enqueue(&mut self, store: &Store, session: &str, id: &str, text: &str) {
        assert_eq!(
            self.emit(
                store,
                session,
                "session.inbox.enqueued",
                json!({
                    "inboxID": id,
                    "item": {"type": "user", "delivery": "queue", "payload": {
                        "text": text, "files": [], "agents": [], "skills": [],
                    }},
                }),
            ),
            ProcessResult::Advanced
        );
    }

    fn step(&mut self, store: &Store, session: &str, message: &str) {
        assert_eq!(
            self.emit(
                store,
                session,
                "session.step.started",
                json!({
                    "assistantMessageID": message, "agent": "build", "started": 1_000,
                    "model": {"providerID": "synthetic", "id": "no-network"},
                }),
            ),
            ProcessResult::Advanced
        );
    }

    fn begin(&mut self, store: &Store, session: &str, call: &str, name: &str, input: Value) {
        for (kind, fields) in [
            (
                "session.tool.input.started",
                json!({"id": call, "name": name}),
            ),
            (
                "session.tool.input.ended",
                json!({"id": call, "text": input.to_string()}),
            ),
            (
                "session.tool.called",
                json!({"id": call, "input": input, "executed": false}),
            ),
        ] {
            let mut fields = fields;
            fields["assistantMessageID"] = json!("msg_step");
            assert_eq!(
                self.emit(store, session, kind, fields),
                ProcessResult::Advanced
            );
        }
    }

    fn finish(
        &mut self,
        store: &Store,
        session: &str,
        call: &str,
        failed: bool,
        text: Option<&str>,
    ) {
        let mut fields = json!({"assistantMessageID": "msg_step", "id": call, "executed": false});
        if failed {
            fields["error"] = json!({"type": "ToolError", "message": "Synthetic failure"});
        }
        if let Some(text) = text {
            fields["content"] = json!([{"type": "text", "text": text}]);
        }
        let kind = if failed {
            "session.tool.failed"
        } else {
            "session.tool.success"
        };
        assert_eq!(
            self.emit(store, session, kind, fields),
            ProcessResult::ResultPublished(SessionKey::new("native", "opencode", session))
        );
    }

    fn result(&mut self, store: &Store, session: &str, call: &str, failed: bool) {
        self.begin(
            store,
            session,
            call,
            "Shell",
            json!({"command": "cargo check", "workdir": "/repo"}),
        );
        self.finish(store, session, call, failed, Some(call));
    }
}

fn branch(session: &str) -> BranchId {
    BranchId(SessionKey::new("native", "opencode", session).to_identity_json())
}

fn activity_contents(input: &Value) -> Vec<&str> {
    input["activity"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["tool"]["result"]["content"].as_str().unwrap())
        .collect()
}

#[test]
fn maintained_failed_retry_yaml_runs_in_native_terminal_order_with_partial_requests() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(directory.path(), FAILED_RETRY);
    let mut capture = Capture::start(&runtime.store, &["ses_retry"], Some(0));
    assert_eq!(runtime.sync(), 0);
    assert_eq!(runtime.count("burn_check_session_attachment"), 1);
    capture.request(
        &runtime.store,
        "ses_retry",
        "msg_user",
        "Fix the compile error.",
    );
    capture.step(&runtime.store, "ses_retry", "msg_step");
    for call in ["call_first", "call_second", "call_third"] {
        capture.begin(
            &runtime.store,
            "ses_retry",
            call,
            "Shell",
            json!({"command": "cargo check", "workdir": "/repo"}),
        );
    }
    capture.enqueue(
        &runtime.store,
        "ses_retry",
        "msg_queued",
        "Unrelated queued instruction.",
    );
    for call in ["call_third", "call_second", "call_first"] {
        capture.finish(&runtime.store, "ses_retry", call, true, Some(call));
    }
    assert_eq!(runtime.sync(), 1);
    assert_eq!(runtime.run(100_000).outcome, RunOutcome::Accepted);
    let calls = runtime.evaluator.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let (candidate, input, attempt) = &calls[0];
    assert_eq!(*attempt, 1);
    assert_eq!(
        activity_contents(input),
        ["call_third", "call_second", "call_first"]
    );
    assert_eq!(input["requests"][0]["text"], "Fix the compile error.");
    assert_eq!(input["requests"].as_array().unwrap().len(), 1);
    assert_eq!(input["request_history_complete"], false);
    assert_eq!(input["source_coverage"], "partial");
    assert!(candidate.messages.iter().all(|message| {
        message.value["tool"]["result"]["status"] == "error"
            && message.value["quality"]["request_history_complete"] == false
    }));
    let status: String = runtime
        .store
        .lock()
        .query_row("SELECT status FROM burn_check_current_status", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(status, "failure");
    drop(calls);
    assert_eq!(runtime.sync(), 0);
    runtime.assert_idle(1_000_000);
    assert_eq!(runtime.count("burn_check_evaluation_attempt"), 1);
}

#[test]
fn failed_retry_success_survives_v2_quality_gaps_continuity_resets_and_restart() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(directory.path(), FAILED_RETRY);
    *runtime.evaluator.selected.lock().unwrap() = Some("clear".into());
    let session = "ses_saved_success";
    let mut capture = Capture::start(&runtime.store, &[session], Some(0));
    runtime.sync();
    capture.request(&runtime.store, session, "request", "Check the build.");
    capture.step(&runtime.store, session, "msg_step");
    for call in ["saved_a", "saved_b", "saved_c"] {
        capture.result(&runtime.store, session, call, false);
    }
    assert_eq!(runtime.sync(), 1);
    assert_eq!(runtime.run(100_000).outcome, RunOutcome::Accepted);

    let saved_assessment = |runtime: &Runtime| {
        runtime
            .store
            .lock()
            .query_row(
                "SELECT s.status, s.evidence_valid, h.evidence_valid,
                    c.frozen_input_json, c.dependencies_json, c.answer_json
               FROM burn_check_current_status s
               JOIN burn_check_assessment_history h ON h.assessment_id = s.assessment_id
               JOIN burn_check_candidate c ON c.candidate_id = h.candidate_id",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, bool>(1)?,
                        row.get::<_, bool>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .unwrap()
    };
    let saved = saved_assessment(&runtime);
    assert_eq!(saved.0, "success");
    assert!(saved.1);
    assert!(saved.2);

    let next_page = |runtime: &Runtime| {
        let progress = runtime
            .adapter
            .load_source_progress(&runtime.active)
            .unwrap();
        PublishedEvidenceSource::new(
            StorePublishedEvidenceReader::for_live_project(&runtime.store, "native", "/repo")
                .with_activation(&runtime.active.id)
                .with_repository_root(Some("/repo")),
            20,
        )
        .unwrap()
        .read_batch(progress.publication_cursor)
        .unwrap()
    };
    assert_eq!(
        runtime
            .store
            .mark_opencode_v2_capture_gap(
                &[capture.claims[session].clone()],
                "stream-disconnected",
                unix_now(),
            )
            .unwrap(),
        1
    );
    assert!(next_page(&runtime).events.iter().any(|event| matches!(
        event,
        SourceEvent::SegmentReset { reason, .. } if reason == "stream-disconnected"
    )));
    assert_eq!(runtime.sync(), 0);
    assert!(
        runtime
            .adapter
            .load_source_progress(&runtime.active)
            .unwrap()
            .selection
            .branches[&branch(session)]
            .results
            .is_empty()
    );
    assert_eq!(saved_assessment(&runtime), saved);

    capture.resume(&runtime.store, session);
    assert_eq!(runtime.sync(), 0);
    assert_eq!(saved_assessment(&runtime), saved);
    capture.request(&runtime.store, session, "request_after_gap", "Check again.");
    capture.step(&runtime.store, session, "msg_step");
    capture.result(&runtime.store, session, "before_restart", false);
    assert!(next_page(&runtime).events.iter().any(|event| matches!(
        event,
        SourceEvent::Gap { reason, .. } if reason == "v2-history-incomplete"
    )));
    assert_eq!(runtime.sync(), 0);
    assert_eq!(
        runtime
            .adapter
            .load_source_progress(&runtime.active)
            .unwrap()
            .selection
            .branches[&branch(session)]
            .results
            .len(),
        1
    );
    assert_eq!(saved_assessment(&runtime), saved);
    runtime.assert_idle(200_000);
    let active = runtime.active.clone();
    let evaluator = runtime.evaluator.clone();
    drop(runtime);

    let restarted = Runtime::reopen(directory.path(), active, evaluator);
    assert_eq!(
        restarted
            .store
            .recover_opencode_v2_capture_claims(unix_now())
            .unwrap(),
        1
    );
    assert_eq!(restarted.sync(), 0);
    assert_eq!(saved_assessment(&restarted), saved);
    assert_eq!(restarted.count("burn_check_assessment_history"), 1);
    assert_eq!(restarted.count("burn_check_evaluation_attempt"), 1);
    assert_eq!(restarted.evaluator_calls(), 1);
    restarted.assert_idle(200_000);

    capture.resume(&restarted.store, session);
    assert_eq!(restarted.sync(), 0);
    capture.request(
        &restarted.store,
        session,
        "request_after_restart",
        "Check again.",
    );
    capture.step(&restarted.store, session, "msg_step");
    for call in ["after_restart_a", "after_restart_b"] {
        capture.result(&restarted.store, session, call, false);
        assert_eq!(restarted.sync(), 0);
    }
    assert_eq!(saved_assessment(&restarted), saved);
    capture.result(&restarted.store, session, "after_restart_c", false);
    assert_eq!(restarted.sync(), 1);
    assert_eq!(restarted.run(300_000).outcome, RunOutcome::Accepted);
    let calls = restarted.evaluator.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        activity_contents(&calls[1].1),
        ["after_restart_a", "after_restart_b", "after_restart_c"]
    );
}

#[test]
fn maintained_over_exploring_yaml_counts_only_completed_direct_reads_and_searches() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(directory.path(), OVER_EXPLORING);
    let mut capture = Capture::start(&runtime.store, &["ses_explore"], Some(0));
    runtime.sync();
    capture.request(
        &runtime.store,
        "ses_explore",
        "msg_user",
        "Fix src/main.rs.",
    );
    capture.step(&runtime.store, "ses_explore", "msg_step");
    capture.result(&runtime.store, "ses_explore", "shell_not_selected", false);
    capture.begin(
        &runtime.store,
        "ses_explore",
        "read_error_not_selected",
        "Read",
        json!({"path": "missing.rs"}),
    );
    capture.finish(
        &runtime.store,
        "ses_explore",
        "read_error_not_selected",
        true,
        Some("File not found"),
    );
    for index in 0..10 {
        let call = format!("selected_{index}");
        let (name, input) = if index % 2 == 0 {
            ("Read", json!({"path": "src/main.rs"}))
        } else {
            ("Grep", json!({"pattern": "main", "path": "src"}))
        };
        capture.begin(&runtime.store, "ses_explore", &call, name, input);
        capture.finish(&runtime.store, "ses_explore", &call, false, Some(&call));
    }
    assert_eq!(runtime.sync(), 1);
    assert_eq!(runtime.run(100_000).outcome, RunOutcome::Accepted);
    let calls = runtime.evaluator.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.messages.len(), 10);
    assert_eq!(activity_contents(&calls[0].1).len(), 10);
    assert!(calls[0].0.messages.iter().all(|message| {
        matches!(
            message.value["tool"]["category"].as_str(),
            Some("read" | "search")
        ) && message.value["tool"]["result"]["status"] == "completed"
    }));
    assert_eq!(runtime.count("burn_check_assessment_history"), 1);
}

#[test]
fn maintained_scope_creep_yaml_receives_direct_edit_arguments_and_frozen_requests() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(directory.path(), SCOPE_CREEP);
    let mut capture = Capture::start(&runtime.store, &["ses_scope"], Some(0));
    runtime.sync();
    capture.request(
        &runtime.store,
        "ses_scope",
        "msg_user",
        "Fix only src/main.rs.",
    );
    capture.step(&runtime.store, "ses_scope", "msg_step");
    for index in 0..5 {
        let call = format!("edit_{index}");
        capture.begin(
            &runtime.store,
            "ses_scope",
            &call,
            "Patch",
            json!({"patchText": "*** Begin Patch\n*** Update File: src/extra.rs\n-old\n+new\n*** End Patch"}),
        );
        capture.finish(
            &runtime.store,
            "ses_scope",
            &call,
            false,
            Some("Applied edit"),
        );
    }
    capture.request(
        &runtime.store,
        "ses_scope",
        "msg_later",
        "A later request must not change this step.",
    );
    assert_eq!(runtime.sync(), 1);
    assert_eq!(runtime.run(100_000).outcome, RunOutcome::Accepted);
    let calls = runtime.evaluator.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1["requests"].as_array().unwrap().len(), 1);
    assert_eq!(calls[0].1["requests"][0]["text"], "Fix only src/main.rs.");
    assert_eq!(calls[0].0.messages.len(), 5);
    assert!(calls[0].0.messages.iter().all(|message| {
        message.value["tool"]["arguments"]["patchText"]
            .as_str()
            .unwrap()
            .contains("src/extra.rs")
    }));
    assert_eq!(runtime.count("burn_check_evaluation_attempt"), 1);
}

#[test]
fn empty_cold_attach_keeps_distinct_future_results_and_restart_does_not_replay() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(directory.path(), SINGLE_RESULT);
    let mut capture = Capture::start(&runtime.store, &["ses_empty"], None);
    assert_eq!(runtime.sync(), 0);
    assert_eq!(runtime.count("burn_check_session_attachment"), 1);
    runtime.assert_idle(100_000);
    assert_eq!(runtime.count("burn_check_evaluation_attempt"), 0);
    capture.request(&runtime.store, "ses_empty", "msg_user", "Check the build.");
    capture.step(&runtime.store, "ses_empty", "msg_step");
    for call in ["first", "second", "third"] {
        capture.result(&runtime.store, "ses_empty", call, true);
    }
    assert_eq!(runtime.sync(), 3);
    assert_eq!(runtime.pending().len(), 3);
    for now in [100_000, 101_000, 102_000] {
        assert_eq!(runtime.run(now).outcome, RunOutcome::Accepted);
    }
    let evaluator = runtime.evaluator.clone();
    let active = runtime.active.clone();
    let inputs: Vec<_> = evaluator
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(_, input, _)| input["content"].clone())
        .collect();
    assert_eq!(inputs, [json!("first"), json!("second"), json!("third")]);
    assert_eq!(runtime.count("burn_check_assessment_history"), 3);
    drop(runtime);

    let restarted = Runtime::reopen(directory.path(), active, evaluator.clone());
    assert_eq!(
        restarted
            .store
            .recover_opencode_v2_capture_claims(unix_now())
            .unwrap(),
        1
    );
    assert_eq!(restarted.sync(), 0);
    assert_eq!(restarted.count("burn_check_session_attachment"), 1);
    assert_eq!(restarted.count("burn_check_assessment_history"), 3);
    assert_eq!(restarted.count("burn_check_evaluation_attempt"), 3);
    restarted.assert_idle(1_000_000);
    assert_eq!(evaluator.calls.lock().unwrap().len(), 3);
}

#[test]
fn cold_empty_attachment_survives_restart_before_any_result() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(directory.path(), SINGLE_RESULT);
    let mut capture = Capture::start(&runtime.store, &["ses_restart_empty"], None);
    assert_eq!(runtime.sync(), 0);
    assert_eq!(runtime.count("burn_check_session_attachment"), 1);
    let evaluator = runtime.evaluator.clone();
    let active = runtime.active.clone();
    drop(runtime);

    let restarted = Runtime::reopen(directory.path(), active, evaluator.clone());
    assert_eq!(
        restarted
            .store
            .recover_opencode_v2_capture_claims(unix_now())
            .unwrap(),
        1
    );
    assert_eq!(restarted.sync(), 0);
    restarted.assert_idle(100_000);
    assert_eq!(restarted.count("burn_check_evaluation_attempt"), 0);
    assert_eq!(restarted.count("burn_check_session_attachment"), 1);
    capture.resume(&restarted.store, "ses_restart_empty");
    assert_eq!(restarted.sync(), 0);
    capture.request(
        &restarted.store,
        "ses_restart_empty",
        "msg_user",
        "Check the build.",
    );
    capture.step(&restarted.store, "ses_restart_empty", "msg_step");
    capture.result(
        &restarted.store,
        "ses_restart_empty",
        "first_after_restart",
        true,
    );
    assert_eq!(restarted.sync(), 1);
    assert_eq!(restarted.run(200_000).outcome, RunOutcome::Accepted);
    assert_eq!(evaluator.calls.lock().unwrap().len(), 1);
}

#[test]
fn fresh_activation_does_not_replay_prior_captured_results() {
    let directory = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::new(directory.path(), FAILED_RETRY);
    let mut capture = Capture::start(&runtime.store, &["ses_seed"], Some(0));
    capture.request(&runtime.store, "ses_seed", "msg_user", "Fix the build.");
    capture.step(&runtime.store, "ses_seed", "msg_step");
    for index in 0..7 {
        capture.result(&runtime.store, "ses_seed", &format!("seed_{index}"), true);
    }
    let mut active = runtime.active.clone();
    active.id.generation = Generation(2);
    active.revision = [2; 32];
    active.bootstrap_publication = runtime.adapter.publication_head().unwrap();
    runtime
        .engine
        .persist_activation(
            &active.id.definition_id,
            &LifecycleTransition::Activated(active.clone()),
            Some(&active),
        )
        .unwrap();
    runtime.active = active;
    assert_eq!(runtime.sync(), 0);
    assert_eq!(runtime.count("burn_check_session_attachment"), 1);
    assert!(runtime.pending().is_empty());
    runtime.assert_idle(100_000);
    assert_eq!(runtime.count("burn_check_evaluation_attempt"), 0);
    for call in ["future_a", "future_b"] {
        capture.result(&runtime.store, "ses_seed", call, true);
        assert_eq!(runtime.sync(), 0);
        runtime.assert_idle(100_000);
    }
    capture.result(&runtime.store, "ses_seed", "future_c", true);
    assert_eq!(runtime.sync(), 1);
    assert_eq!(runtime.pending().len(), 1);
    assert_eq!(runtime.run(100_000).outcome, RunOutcome::Accepted);
    let calls = runtime.evaluator.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        activity_contents(&calls[0].1),
        ["future_a", "future_b", "future_c"]
    );
    drop(calls);
    assert_eq!(runtime.sync(), 0);
    runtime.assert_idle(1_000_000);
    assert_eq!(runtime.count("burn_check_assessment_history"), 1);
    assert_eq!(runtime.count("burn_check_evaluation_attempt"), 1);
    let active = runtime.active.clone();
    let evaluator = runtime.evaluator.clone();
    drop(runtime);

    let restarted = Runtime::reopen(directory.path(), active, evaluator.clone());
    assert_eq!(
        restarted
            .store
            .recover_opencode_v2_capture_claims(unix_now())
            .unwrap(),
        1
    );
    assert_eq!(restarted.sync(), 0);
    restarted.assert_idle(1_000_000);
    assert_eq!(restarted.count("burn_check_assessment_history"), 1);
    assert_eq!(restarted.count("burn_check_evaluation_attempt"), 1);
    assert_eq!(evaluator.calls.lock().unwrap().len(), 1);
}

#[test]
fn live_reader_holds_an_unattached_result_until_its_marker_then_drains_future_work() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(directory.path(), SINGLE_RESULT);
    let mut capture = Capture::start(&runtime.store, &["ses_barrier"], Some(0));
    capture.request(
        &runtime.store,
        "ses_barrier",
        "msg_user",
        "Check the build.",
    );
    capture.step(&runtime.store, "ses_barrier", "msg_step");
    capture.result(&runtime.store, "ses_barrier", "before_marker", true);
    let source = PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_live_project(&runtime.store, "native", "/repo")
            .with_activation(&runtime.active.id),
        100,
    )
    .unwrap();
    let prefix = source.read_batch(PublicationCursor(0)).unwrap();
    assert!(prefix.events.iter().all(|event| matches!(
        event,
        SourceEvent::Gap { .. } | SourceEvent::SegmentReset { .. }
    )));
    runtime
        .engine
        .apply_source_page(&runtime.active, &prefix)
        .unwrap();
    let before_marker = runtime
        .adapter
        .load_source_progress(&runtime.active)
        .unwrap();
    for _ in 0..2 {
        let held = source.read_batch(before_marker.publication_cursor).unwrap();
        assert!(held.events.is_empty());
        assert_eq!(held.next, before_marker.publication_cursor);
        assert!(!held.has_more);
    }
    assert_eq!(runtime.count("burn_check_session_attachment"), 0);
    assert_eq!(runtime.count("burn_check_observation"), 0);
    assert_eq!(runtime.sync(), 0);
    assert_eq!(runtime.count("burn_check_session_attachment"), 1);
    assert_eq!(
        runtime
            .adapter
            .load_source_progress(&runtime.active)
            .unwrap()
            .publication_cursor,
        before_marker.publication_cursor
    );
    capture.result(&runtime.store, "ses_barrier", "after_marker", true);
    let ready = source.read_batch(before_marker.publication_cursor).unwrap();
    assert_eq!(ready.events.len(), 2);
    assert!(
        ready
            .events
            .iter()
            .all(|event| matches!(event, SourceEvent::Observation { .. }))
    );
    assert!(ready.next > before_marker.publication_cursor);
    assert!(!ready.has_more);
    assert_eq!(
        runtime
            .engine
            .apply_source_page(&runtime.active, &ready)
            .unwrap()
            .candidates,
        2
    );
    assert_eq!(runtime.run(100_000).outcome, RunOutcome::Accepted);
    assert_eq!(runtime.run(101_000).outcome, RunOutcome::Accepted);
    let calls = runtime.evaluator.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1["content"], "before_marker");
    assert_eq!(calls[1].1["content"], "after_marker");
    drop(calls);
    assert_eq!(runtime.sync(), 0);
    runtime.assert_idle(1_000_000);
    assert_eq!(runtime.count("burn_check_evaluation_attempt"), 2);
}

#[test]
fn expired_admitted_session_recovers_while_healthy_lease_and_window_continue() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(directory.path(), FAILED_RETRY);
    let mut capture = Capture::start(&runtime.store, &["ses_gap", "ses_other"], Some(0));
    runtime.sync();
    for session in ["ses_gap", "ses_other"] {
        capture.request(&runtime.store, session, "msg_user", "Fix the build.");
        capture.step(&runtime.store, session, "msg_step");
    }
    for call in ["old_a", "old_b", "old_c"] {
        capture.result(&runtime.store, "ses_gap", call, true);
    }
    assert_eq!(runtime.sync(), 1);
    assert_eq!(runtime.run(100_000).outcome, RunOutcome::Accepted);
    let accepted_input: String = runtime
        .store
        .lock()
        .query_row(
            "SELECT frozen_input_json FROM burn_check_candidate WHERE state = 'accepted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    capture.result(&runtime.store, "ses_gap", "pending_before_gap", true);
    for call in ["other_a", "other_b"] {
        capture.result(&runtime.store, "ses_other", call, true);
    }
    runtime.sync();
    let pending = runtime.pending().remove(0);
    let before = runtime
        .adapter
        .load_source_progress(&runtime.active)
        .unwrap();
    let original_scope = vec![record("ses_gap"), record("ses_other")];
    runtime
        .store
        .lock()
        .execute("UPDATE session SET updated_at_epoch = 0", [])
        .unwrap();
    assert!(
        changed_scope_sessions(
            &original_scope,
            &runtime
                .store
                .opencode_v2_live_candidates(unix_now(), 10)
                .unwrap()
        )
        .is_empty()
    );
    let healthy_lease: (String, i64, Option<i64>) = runtime
        .store
        .lock()
        .query_row(
            "SELECT capture_epoch, claim_fence, lease_expires_at_epoch
           FROM opencode_v2_capture WHERE session_id = 'ses_other'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let gap_claim = capture.claims["ses_gap"].clone();
    assert_eq!(
        runtime
            .store
            .mark_opencode_v2_capture_gap(&[gap_claim], "stream-disconnected", unix_now(),)
            .unwrap(),
        1
    );
    assert_eq!(runtime.sync(), 0);
    assert!(
        changed_scope_sessions(
            &original_scope,
            &runtime
                .store
                .opencode_v2_live_candidates(unix_now(), 10)
                .unwrap()
        )
        .is_empty()
    );
    let after = runtime
        .adapter
        .load_source_progress(&runtime.active)
        .unwrap();
    assert!(
        after.selection.branches[&branch("ses_gap")]
            .results
            .is_empty()
    );
    assert!(!after.selection.branches[&branch("ses_gap")].coverage_complete);
    assert_eq!(
        after.selection.branches[&branch("ses_other")],
        before.selection.branches[&branch("ses_other")]
    );
    assert_eq!(
        runtime.adapter.claim_due(&pending, 200_000, true).unwrap(),
        CandidateClaim::Stale
    );
    assert_eq!(runtime.count("burn_check_assessment_history"), 1);
    let valid: bool = runtime
        .store
        .lock()
        .query_row(
            "SELECT evidence_valid FROM burn_check_current_status",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(valid);
    let frozen_after_gap: String = runtime
        .store
        .lock()
        .query_row(
            "SELECT frozen_input_json FROM burn_check_candidate WHERE state = 'accepted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(frozen_after_gap, accepted_input);

    capture.resume(&runtime.store, "ses_gap");
    runtime.sync();
    let current_healthy_lease: (String, i64, Option<i64>) = runtime
        .store
        .lock()
        .query_row(
            "SELECT capture_epoch, claim_fence, lease_expires_at_epoch
           FROM opencode_v2_capture WHERE session_id = 'ses_other'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(current_healthy_lease, healthy_lease);
    capture.request(
        &runtime.store,
        "ses_gap",
        "msg_after_gap",
        "Fix the remaining error.",
    );
    capture.step(&runtime.store, "ses_gap", "msg_step");
    for call in ["new_a", "new_b"] {
        capture.result(&runtime.store, "ses_gap", call, true);
    }
    assert_eq!(runtime.sync(), 0);
    capture.result(&runtime.store, "ses_other", "other_c", true);
    assert_eq!(runtime.sync(), 1);
    assert_eq!(runtime.run(200_000).outcome, RunOutcome::Accepted);
    capture.result(&runtime.store, "ses_gap", "new_c", true);
    assert_eq!(runtime.sync(), 1);
    assert_eq!(runtime.run(300_000).outcome, RunOutcome::Accepted);
    let calls = runtime.evaluator.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(
        activity_contents(&calls[1].1),
        ["other_a", "other_b", "other_c"]
    );
    assert_eq!(activity_contents(&calls[2].1), ["new_a", "new_b", "new_c"]);
    assert_eq!(calls[2].1["request_history_complete"], false);
    assert_eq!(calls[2].1["source_coverage"], "partial");
    let successes: u64 = runtime
        .store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM burn_check_current_status WHERE status = 'success'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(successes, 0);
}

#[test]
fn missing_required_result_content_blocks_only_that_candidate() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(directory.path(), SINGLE_RESULT);
    let mut capture = Capture::start(&runtime.store, &["ses_missing"], Some(0));
    runtime.sync();
    capture.request(
        &runtime.store,
        "ses_missing",
        "msg_user",
        "Check the build.",
    );
    capture.step(&runtime.store, "ses_missing", "msg_step");
    capture.begin(
        &runtime.store,
        "ses_missing",
        "missing",
        "Shell",
        json!({"command": "cargo check"}),
    );
    capture.finish(&runtime.store, "ses_missing", "missing", true, None);
    capture.result(&runtime.store, "ses_missing", "safe", true);
    assert_eq!(runtime.sync(), 2);
    let missing = runtime.run(100_000);
    assert!(matches!(missing.outcome, RunOutcome::NotAssessed(_)));
    assert_eq!(missing.attempts, 0);
    assert_eq!(runtime.evaluator.calls.lock().unwrap().len(), 0);
    assert_eq!(runtime.count("burn_check_current_status"), 0);
    assert_eq!(runtime.run(100_001).outcome, RunOutcome::Accepted);
    let calls = runtime.evaluator.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1["content"], "safe");
    assert_eq!(runtime.count("burn_check_evaluation_attempt"), 1);
}

#[test]
fn definition_and_scope_fences_preserve_unknown_attempts_without_resending() {
    for fence in ["definition", "cwd", "source", "disable"] {
        let directory = tempfile::tempdir().unwrap();
        let runtime = Runtime::new(directory.path(), SINGLE_RESULT);
        let mut capture = Capture::start(&runtime.store, &["ses_fence"], Some(0));
        runtime.sync();
        capture.request(&runtime.store, "ses_fence", "msg_user", "Check the build.");
        capture.step(&runtime.store, "ses_fence", "msg_step");
        capture.result(&runtime.store, "ses_fence", "accepted", true);
        runtime.sync();
        assert_eq!(runtime.run(100_000).outcome, RunOutcome::Accepted);
        capture.result(&runtime.store, "ses_fence", "uncertain", true);
        runtime.sync();
        let uncertain = runtime.pending().remove(0);
        runtime.evaluator.hold.store(true, Ordering::SeqCst);
        let branches = runtime
            .adapter
            .initialized_branches(&runtime.active)
            .unwrap();
        let StartOutcome::Started(mut task) = runtime
            .engine
            .start_one_due_for_branches(&runtime.active, 101_000, &branches)
            .unwrap()
        else {
            panic!("the second candidate must start before the {fence} fence");
        };
        assert!(
            task.as_mut()
                .poll(&mut TaskContext::from_waker(Waker::noop()))
                .is_pending()
        );
        drop(task);
        let old_claim = capture.claims["ses_fence"].clone();
        let mut active = runtime.active.clone();
        match fence {
            "definition" => {
                active.id.generation = Generation(2);
                active.revision = [2; 32];
                active.bootstrap_publication = runtime.adapter.publication_head().unwrap();
                runtime
                    .engine
                    .persist_activation(
                        &active.id.definition_id,
                        &LifecycleTransition::Activated(active.clone()),
                        Some(&active),
                    )
                    .unwrap();
                assert_eq!(runtime.count("burn_check_current_status"), 0);
            }
            "disable" => {
                runtime
                    .engine
                    .persist_activation(
                        &active.id.definition_id,
                        &LifecycleTransition::Removed {
                            next_generation: Generation(2),
                        },
                        None,
                    )
                    .unwrap();
                assert_eq!(runtime.count("burn_check_current_status"), 0);
            }
            "cwd" | "source" => {
                let mut changed = record("ses_fence");
                if fence == "cwd" {
                    changed.cwd = Some("/other".into());
                } else {
                    changed.source_label = "opencode:ses_fence:unsupported".into();
                }
                runtime
                    .store
                    .upsert_sessions(&[changed], &crate::agents::evidence_cohort())
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            runtime
                .adapter
                .initialized_branches(&runtime.active)
                .unwrap()
                .is_empty(),
            "the {fence} fence must close the old live scope"
        );
        if fence != "definition" {
            assert!(
                !runtime
                    .store
                    .advance_opencode_v2_capture(
                        &old_claim,
                        capture.sequences["ses_fence"] + 1,
                        None,
                        unix_now(),
                        LEASE_SECS,
                    )
                    .unwrap()
            );
        }
        let evaluator = runtime.evaluator.clone();
        drop(runtime);
        let store = Arc::new(Store::open(directory.path()).unwrap());
        let restarted = Arc::new(
            StoreCheckStore::new(
                store.clone(),
                "v2-fence-restarted".into(),
                active.id.scope.clone(),
            )
            .unwrap(),
        );
        if fence != "disable" {
            restarted.register_activation(&active).unwrap();
        }
        let engine = CheckEngine::new(restarted.clone(), evaluator.clone());
        engine.recover_pending().unwrap();
        assert!(
            restarted.load_recovery(None, 100).unwrap().is_empty(),
            "{fence}"
        );
        assert_eq!(
            restarted.claim_due(&uncertain, 1_000_000, true).unwrap(),
            CandidateClaim::Stale
        );
        assert!(matches!(
            engine.start_one_due(&active, 1_000_000).unwrap(),
            StartOutcome::NotDue
        ));
        assert_eq!(evaluator.calls.lock().unwrap().len(), 2, "{fence}");
        let outcomes = store
            .lock()
            .prepare("SELECT outcome FROM burn_check_evaluation_attempt ORDER BY attempt_id")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(outcomes.len(), 2, "{fence}");
        assert!(
            outcomes.iter().any(|outcome| outcome == "unknown"),
            "{fence}"
        );
    }
}

fn boundary_events() -> Vec<Value> {
    [
        ("session.inbox.enqueued", json!({"inboxID":"request", "item":{"type":"user", "delivery":"queue", "payload":{"text":"Inspect the source"}}})),
        ("session.inbox.delivered", json!({"inboxID":"request"})),
        ("session.step.started", json!({"assistantMessageID":"step", "agent":"build", "model":{"providerID":"synthetic", "id":"no-network"}, "started":1000})),
        ("session.tool.input.started", json!({"assistantMessageID":"step", "id":"call", "name":"read"})),
        ("session.tool.input.ended", json!({"assistantMessageID":"step", "id":"call", "text":"{\"path\":\"source.rs\"}"})),
        ("session.tool.called", json!({"assistantMessageID":"step", "id":"call", "input":{"path":"source.rs"}, "executed":false})),
        ("session.tool.success", json!({"assistantMessageID":"step", "id":"call", "content":[{"type":"text", "text":"source"}], "executed":false})),
    ].into_iter().enumerate().map(|(index, (kind, extra))| {
        let sequence = index as u64 + 1;
        let mut data = json!({"sessionID":"ses_cut"});
        data.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        json!({"id":format!("event-{sequence}"), "type":kind, "created":1000,
            "durable":{"aggregateID":"ses_cut", "seq":sequence, "version":if kind == "session.tool.success" {2} else {1}}, "data":data})
    }).collect()
}

#[test]
fn every_captured_join_and_hydration_cut_preserves_one_live_attempt_and_partial_history() {
    let events = boundary_events();
    for baseline in 0..=events.len() {
        for consumed_during_hydration in 0..=events.len() {
            let directory = tempfile::tempdir().unwrap();
            let runtime = Runtime::new(directory.path(), SINGLE_RESULT);
            let mut joined = Capture::start(&runtime.store, &["ses_cut"], Some(baseline as u64));
            runtime.sync();
            let mut epoch = OpenCodeV2CaptureEpoch::new(vec!["ses_cut".into()]).unwrap();
            epoch.confirm_subscription().unwrap();
            for event in &events[..consumed_during_hydration] {
                epoch.observe(event.clone()).unwrap();
            }
            let mut captured = epoch
                .finish_hydration(HashMap::from([("ses_cut".into(), Some(baseline as u64))]))
                .unwrap();
            for event in &events[consumed_during_hydration..] {
                captured.extend(epoch.observe(event.clone()).unwrap());
            }
            for event in captured {
                match event.kind {
                    BoundaryKind::Seed => {
                        assert_eq!(
                            process_seed_event(event, &mut joined.joiners),
                            ProcessResult::Advanced
                        );
                    }
                    BoundaryKind::Live => {
                        process_captured_event(
                            event,
                            &mut joined.joiners,
                            &joined.claims,
                            &runtime.store,
                        )
                        .unwrap();
                    }
                }
            }
            assert!(
                epoch
                    .observe(events.last().unwrap().clone())
                    .unwrap()
                    .is_none()
            );
            let expected = usize::from(baseline < events.len());
            assert_eq!(
                runtime.sync(),
                expected,
                "baseline={baseline}, hydration={consumed_during_hydration}"
            );
            if expected == 1 {
                assert_eq!(runtime.run(100_000).outcome, RunOutcome::Accepted);
                let calls = runtime.evaluator.calls.lock().unwrap();
                let result = &calls[0].0.messages.last().unwrap().value;
                assert_eq!(result["quality"]["request_history_complete"], false);
                assert_eq!(calls[0].1["content"], "source");
            }
            runtime.sync();
            runtime.assert_idle(1_000_000);
            assert_eq!(runtime.evaluator_calls(), expected);
            assert_eq!(
                runtime.count("burn_check_evaluation_attempt"),
                expected as u64
            );
        }
    }
}

#[test]
fn pre_subscription_missing_call_join_does_not_invent_a_snapshot_seed() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(directory.path(), SINGLE_RESULT);
    let events = boundary_events();
    let sequence = events.len() as u64;
    let mut capture = Capture::start(&runtime.store, &["ses_cut"], Some(sequence - 1));
    runtime.sync();
    let event = events.last().unwrap().clone();
    assert_eq!(
        process_captured_event(
            CapturedEvent {
                session_id: "ses_cut".into(),
                sequence,
                kind: BoundaryKind::Live,
                event,
            },
            &mut capture.joiners,
            &capture.claims,
            &runtime.store
        )
        .unwrap(),
        ProcessResult::Advanced
    );
    runtime.sync();
    runtime.assert_idle(100_000);
    assert_eq!(runtime.count("native_tool_result_current"), 0);
    assert_eq!(runtime.count("burn_check_evaluation_attempt"), 0);
    assert_eq!(runtime.evaluator_calls(), 0);
}

#[test]
fn v2_saved_answer_catches_up_after_restart_or_becomes_stale_after_a_gap_without_resend() {
    for gap in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let runtime = Runtime::new(directory.path(), SINGLE_RESULT);
        let mut capture = Capture::start(&runtime.store, &["ses_answer"], Some(0));
        runtime.sync();
        capture.request(&runtime.store, "ses_answer", "request", "Inspect the build");
        capture.step(&runtime.store, "ses_answer", "msg_step");
        capture.result(&runtime.store, "ses_answer", "first", true);
        runtime.sync();
        runtime.evaluator.hold.store(true, Ordering::SeqCst);
        let branches = runtime
            .adapter
            .initialized_branches(&runtime.active)
            .unwrap();
        let StartOutcome::Started(mut task) = runtime
            .engine
            .start_one_due_for_branches(&runtime.active, 100_000, &branches)
            .unwrap()
        else {
            panic!("the first result must dispatch");
        };
        assert!(
            task.as_mut()
                .poll(&mut TaskContext::from_waker(Waker::noop()))
                .is_pending()
        );
        capture.result(&runtime.store, "ses_answer", "newer", true);
        runtime.evaluator.hold.store(false, Ordering::SeqCst);
        let Poll::Ready(settled) = task
            .as_mut()
            .poll(&mut TaskContext::from_waker(Waker::noop()))
        else {
            panic!("the saved answer must settle");
        };
        assert_eq!(settled.unwrap().outcome, RunOutcome::Deferred);
        drop(task);
        assert_eq!(runtime.count("burn_check_assessment_history"), 0);
        let (saved_id, saved_input, saved_answer): (String, String, String) = runtime.store.lock().query_row(
            "SELECT candidate_id, frozen_input_json, answer_json FROM burn_check_candidate WHERE state = 'answered'",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(runtime.evaluator_calls(), 1);
        assert_eq!(runtime.count("burn_check_evaluation_attempt"), 1);
        if gap {
            runtime
                .store
                .mark_opencode_v2_capture_gap(
                    &[capture.claims["ses_answer"].clone()],
                    "stream-disconnected",
                    unix_now(),
                )
                .unwrap();
        }
        let evaluator = runtime.evaluator.clone();
        let active = runtime.active.clone();
        drop(runtime);
        let restarted = Runtime::reopen(directory.path(), active, evaluator);
        restarted.sync();
        assert_eq!(
            restarted.count("burn_check_assessment_history"),
            u64::from(!gap)
        );
        assert_eq!(
            restarted.count("burn_check_current_status"),
            u64::from(!gap)
        );
        assert_eq!(restarted.count("burn_check_evaluation_attempt"), 1);
        assert_eq!(restarted.evaluator_calls(), 1);
        let (input, answer): (String, String) = restarted.store.lock().query_row(
            "SELECT frozen_input_json, answer_json FROM burn_check_candidate WHERE candidate_id = ?1",
            [&saved_id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(input, saved_input);
        assert_eq!(answer, saved_answer);
        if !gap {
            let accepted_id: String = restarted
                .store
                .lock()
                .query_row(
                    "SELECT candidate_id FROM burn_check_assessment_history",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(accepted_id, saved_id);
        }
        restarted.sync();
        assert_eq!(restarted.evaluator_calls(), 1);
        if gap {
            assert!(
                restarted
                    .adapter
                    .load_recovery(None, 100)
                    .unwrap()
                    .is_empty()
            );
            restarted.assert_idle(1_000_000);
        }
    }
}
