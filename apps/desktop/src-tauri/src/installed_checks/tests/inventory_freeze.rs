use super::*;
use antiburn_checks::domain::ObservationDraft;
use antiburn_checks::source::SourceEvent;

const INVENTORY_CHECK: &str = r#"name: Frozen inventory
if: global.instructions.length > 0
on:
  tool_result:
  window: {size: 3, min: 2}
  cooldown: 0s
decisions:
  review:
    input:
      instructions: "${{ global.instructions }}"
      activity: "${{ window.messages }}"
      repo_root: "${{ repo.root }}"
    output:
      verdict:
        type: choice
        question: Is the recorded failure unresolved?
        criteria: {stuck: Still unresolved., clear: Resolved.}
actions:
  - type: status
    input: {result: failure}
"#;

fn prepare() -> (Fixture, PathBuf) {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.global.join("failed-retry-loop.yml"),
        INVENTORY_CHECK,
    )
    .unwrap();
    let home = fixture
        .directory
        .path()
        .canonicalize()
        .unwrap()
        .join("source-home");
    std::fs::create_dir_all(home.join(".pi/agent")).unwrap();
    std::fs::write(home.join(".pi/agent/AGENTS.md"), "Global before\n").unwrap();
    std::fs::write(fixture.project.join("AGENTS.md"), "Repo before\n").unwrap();
    (fixture, home)
}

fn host(fixture: &Fixture, home: &Path) -> InstalledCheckHost<ScriptedEvaluator> {
    let mut host = fixture.host();
    host.current_context = current_context::CurrentContextCapture::new(Some(home.to_owned()));
    host
}

fn frozen(store: &Store) -> Vec<FrozenCandidate> {
    store
        .lock()
        .prepare("SELECT frozen_input_json FROM burn_check_candidate ORDER BY trigger_position")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
        .collect()
}

fn due_time() -> u64 {
    u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp()).unwrap() * 1000 + 1000
}

fn publications(store: &Store) -> Vec<(i64, String)> {
    store.lock().prepare("SELECT publication_sequence, payload_json FROM native_tool_result_publication WHERE outcome = 'update' AND event_kind = 'tool_result' ORDER BY publication_sequence").unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
        .collect::<rusqlite::Result<_>>().unwrap()
}

fn native_drafts(
    fixture: &Fixture,
    active: &EffectiveActivation,
    startup: bool,
) -> Vec<ObservationDraft> {
    let reader = StorePublishedEvidenceReader::for_project(
        &fixture.store,
        "native",
        fixture.project.to_str().unwrap(),
    )
    .with_repository_root(active.project_root.as_ref().and_then(|root| root.to_str()));
    let events = if startup {
        reader
            .recent_results_before(&SessionKey::new("native", "pi", "main"), None)
            .unwrap()
    } else {
        PublishedEvidenceSource::new(reader, 64)
            .unwrap()
            .read_batch(active.bootstrap_publication)
            .unwrap()
            .events
    };
    events
        .into_iter()
        .filter_map(|event| match event {
            SourceEvent::Observation { observation, .. } => Some(*observation),
            _ => None,
        })
        .collect()
}

fn assert_native_unchanged(fixture: &Fixture, expected: &[ObservationDraft]) {
    let stored = fixture
        .store
        .lock()
        .prepare(
            "SELECT evidence_version, payload_json FROM burn_check_observation WHERE removed = 0 AND event_kind = 'tool_result'",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(stored.len(), expected.len());
    for (version, payload) in stored {
        let draft: ObservationDraft = serde_json::from_str(&payload).unwrap();
        assert_eq!(version, draft.version.0);
        assert!(expected.contains(&draft));
        assert!(draft.context.value["global"].get("instructions").is_none());
    }
}

#[tokio::test]
async fn inventory_freeze_live_and_startup_use_current_files_and_preserve_source_hashes() {
    for startup in [false, true] {
        let (fixture, home) = prepare();
        if startup {
            fixture.publish("main", 2);
        }
        let mut host = host(&fixture, &home);
        host.reconcile(&Admission::Ready).unwrap();
        if !startup {
            host.catch_up().unwrap();
            fixture.publish("main", 2);
        }
        let active = host.projects.values().next().unwrap().active[0].clone();
        let native = publications(&fixture.store);
        let expected = native_drafts(&fixture, &active, startup);
        std::fs::write(home.join(".pi/agent/AGENTS.md"), "Current at capture\n").unwrap();
        host.catch_up().unwrap();
        let candidates = frozen(&fixture.store);
        assert_eq!(candidates.len(), 1);
        assert_eq!(host.current_context.calls, 2);
        let snapshot = &candidates[0].context.value;
        assert_eq!(
            snapshot["global"]["instructions"][0]["content"],
            "Current at capture\n"
        );
        assert_eq!(
            snapshot["repo"]["instructions"][0]["content"],
            "Repo before\n"
        );
        assert_eq!(snapshot["repo"]["root"], fixture.project.to_str().unwrap());
        assert_eq!(snapshot["global"]["instructions_complete"], false);
        assert!(
            !snapshot["global"]["instructions_limits"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(snapshot["agent"].get("skills").is_none());
        assert_eq!(publications(&fixture.store), native);
        assert_native_unchanged(&fixture, &expected);
        fixture.evaluator.answer("stuck");
        assert_eq!(
            run_due_at(&mut host, due_time()).await.outcome,
            RunOutcome::Accepted
        );
        assert_eq!(
            fixture.evaluator.inputs.lock().unwrap()[0]["instructions"][0]["content"],
            "Current at capture\n"
        );
        assert_eq!(host.current_context.calls, 2);
    }
}

#[tokio::test]
async fn inventory_freeze_restart_safe_retry_delay_and_acceptance_recovery_never_reread_files() {
    let (fixture, home) = prepare();
    let mut original = host(&fixture, &home);
    original.reconcile(&Admission::Ready).unwrap();
    original.catch_up().unwrap();
    fixture.publish("main", 2);
    original.catch_up().unwrap();
    let saved = frozen(&fixture.store);
    assert_eq!(original.current_context.calls, 2);
    std::fs::write(home.join(".pi/agent/AGENTS.md"), "Edited after capture\n").unwrap();
    std::fs::remove_file(fixture.project.join("AGENTS.md")).unwrap();
    drop(original);
    let mut restarted = host(&fixture, &home);
    restarted.reconcile(&Admission::Ready).unwrap();
    restarted.catch_up().unwrap();
    assert_eq!(restarted.current_context.calls, 0);
    assert_eq!(frozen(&fixture.store), saved);
    fixture
        .evaluator
        .answers
        .lock()
        .unwrap()
        .push_back(Err(AttemptFailure::SafeTransient));
    fixture.evaluator.answer("stuck");
    fixture.evaluator.wait.store(true, Ordering::Release);
    fixture.store.lock().execute_batch("CREATE TRIGGER fail_inventory_accept BEFORE INSERT ON burn_check_assessment_history BEGIN SELECT RAISE(ABORT, 'injected acceptance failure'); END;").unwrap();
    let task = tokio::spawn(restarted.evaluation_jobs(1).pop().unwrap().run());
    fixture.evaluator.started.notified().await;
    std::fs::remove_file(home.join(".pi/agent/AGENTS.md")).unwrap();
    fixture.evaluator.wait.store(false, Ordering::Release);
    fixture.evaluator.release.notify_one();
    assert!(task.await.unwrap().is_err());
    let inputs = fixture.evaluator.inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[0], inputs[1]);
    assert_eq!(inputs[0]["instructions"][0]["content"], "Global before\n");
    assert_eq!(restarted.current_context.calls, 0);
    assert_eq!(frozen(&fixture.store), saved);
    fixture
        .store
        .lock()
        .execute_batch("DROP TRIGGER fail_inventory_accept;")
        .unwrap();
    drop(restarted);
    let mut recovery = host(&fixture, &home);
    recovery.reconcile(&Admission::Ready).unwrap();
    recovery.catch_up().unwrap();
    assert_eq!(
        status_writes(&fixture.store, &recovery.definitions[0]).len(),
        1
    );
    assert!(
        recovery
            .evaluation_jobs(1)
            .pop()
            .unwrap()
            .run()
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(fixture.evaluator.inputs.lock().unwrap().len(), 2);
    assert_eq!(recovery.current_context.calls, 0);
    assert_eq!(frozen(&fixture.store), saved);
}

#[test]
fn inventory_freeze_failed_live_and_startup_transactions_allow_a_fresh_uncommitted_capture() {
    for startup in [false, true] {
        let (fixture, home) = prepare();
        if startup {
            fixture.publish("main", 2);
        }
        let mut host = host(&fixture, &home);
        host.reconcile(&Admission::Ready).unwrap();
        if !startup {
            host.catch_up().unwrap();
            fixture.publish("main", 2);
        }
        let native = publications(&fixture.store);
        fixture.store.lock().execute_batch("CREATE TRIGGER fail_inventory_candidate BEFORE INSERT ON burn_check_candidate BEGIN SELECT RAISE(ABORT, 'injected candidate failure'); END;").unwrap();
        assert!(host.catch_up().is_err());
        assert_eq!(host.current_context.calls, 2);
        assert!(frozen(&fixture.store).is_empty());
        let observations: i64 = fixture
            .store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_observation WHERE event_kind = 'tool_result'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(observations, 0);
        fixture
            .store
            .lock()
            .execute_batch("DROP TRIGGER fail_inventory_candidate;")
            .unwrap();
        std::fs::write(
            home.join(".pi/agent/AGENTS.md"),
            "After failed transaction\n",
        )
        .unwrap();
        host.catch_up().unwrap();
        assert_eq!(host.current_context.calls, 4);
        assert_eq!(
            frozen(&fixture.store)[0].context.value["global"]["instructions"][0]["content"],
            "After failed transaction\n"
        );
        assert_eq!(publications(&fixture.store), native);
    }
}

#[tokio::test]
async fn inventory_freeze_missing_repo_does_not_block_useful_partial_global_context() {
    let (fixture, home) = prepare();
    std::fs::write(
        fixture.global.join("failed-retry-loop.yml"),
        INVENTORY_CHECK.replace("      repo_root: \"${{ repo.root }}\"\n", ""),
    )
    .unwrap();
    let mut host = host(&fixture, &home);
    host.reconcile(&Admission::Ready).unwrap();
    host.projects.values_mut().next().unwrap().active[0].project_root = None;
    host.catch_up().unwrap();
    fixture.publish("main", 2);
    host.catch_up().unwrap();
    let candidates = frozen(&fixture.store);
    assert_eq!(candidates.len(), 1);
    assert!(
        candidates[0].context.value["repo"]
            .get("instructions")
            .is_none()
    );
    assert!(
        !candidates[0].context.value["repo"]["instructions_limits"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    fixture.evaluator.answer("stuck");
    assert_eq!(
        run_due_at(&mut host, due_time()).await.outcome,
        RunOutcome::Accepted
    );
}

#[test]
fn inventory_freeze_inventory_free_checks_make_zero_collector_calls() {
    let (fixture, home) = prepare();
    std::fs::write(
        fixture.global.join("failed-retry-loop.yml"),
        INVENTORY_CHECK
            .replace("if: global.instructions.length > 0\n", "")
            .replace("      instructions: \"${{ global.instructions }}\"\n", ""),
    )
    .unwrap();
    let mut host = host(&fixture, &home);
    host.reconcile(&Admission::Ready).unwrap();
    host.catch_up().unwrap();
    fixture.publish("main", 2);
    host.catch_up().unwrap();
    assert_eq!(host.current_context.calls, 0);
    assert_eq!(frozen(&fixture.store).len(), 1);
}

#[test]
fn inventory_freeze_missing_home_directory_keeps_safe_repo_values() {
    let (fixture, home) = prepare();
    std::fs::write(
        fixture.global.join("failed-retry-loop.yml"),
        INVENTORY_CHECK.replace("global.instructions", "repo.instructions"),
    )
    .unwrap();
    let mut host = host(&fixture, &home.join("missing-home"));
    host.reconcile(&Admission::Ready).unwrap();
    host.catch_up().unwrap();
    fixture.publish("main", 2);
    host.catch_up().unwrap();
    let candidates = frozen(&fixture.store);
    assert_eq!(candidates.len(), 1);
    assert!(
        candidates[0].context.value["global"]
            .get("instructions")
            .is_none()
    );
    assert_eq!(
        candidates[0].context.value["repo"]["instructions"][0]["content"],
        "Repo before\n"
    );
}

#[test]
fn inventory_freeze_unknown_home_does_not_invent_empty_inventories_or_read_files() {
    let (fixture, _) = prepare();
    std::fs::write(
        fixture.global.join("failed-retry-loop.yml"),
        INVENTORY_CHECK.replace("if: global.instructions.length > 0\n", ""),
    )
    .unwrap();
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    host.catch_up().unwrap();
    fixture.publish("main", 2);
    host.catch_up().unwrap();
    let candidates = frozen(&fixture.store);
    assert_eq!(candidates.len(), 1);
    assert_eq!(host.current_context.calls, 0);
    for scope in ["global", "repo"] {
        assert!(
            candidates[0].context.value[scope]
                .get("instructions")
                .is_none()
        );
        assert_eq!(
            candidates[0].context.value[scope]["instructions_limits"],
            json!(["native_home_unavailable"])
        );
    }
    assert_eq!(
        candidates[0].context.value["repo"]["root"],
        fixture.project.to_str().unwrap()
    );
}

#[test]
fn inventory_freeze_uses_publication_cwd_below_repo_and_bound_native_home() {
    let (fixture, home) = prepare();
    let cwd = fixture.project.join("package");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(cwd.join("AGENTS.md"), "Package policy\n").unwrap();
    fixture.publish_in_cwd("native", "main", 2, &cwd);
    let mut host = host(&fixture, &home);
    host.reconcile(&Admission::Ready).unwrap();
    host.catch_up().unwrap();
    let candidates = frozen(&fixture.store);
    let context = &candidates[0].context.value;
    assert_eq!(
        context["global"]["instructions"][0]["path"],
        home.join(".pi/agent/AGENTS.md").to_str().unwrap()
    );
    assert!(
        context["repo"]["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == cwd.join("AGENTS.md").to_str().unwrap())
    );
    assert_eq!(context["repo"]["root"], fixture.project.to_str().unwrap());
}

#[cfg(target_os = "macos")]
#[test]
fn inventory_freeze_uses_recorded_grants_without_a_whole_scope_gate() {
    let (fixture, home) = prepare();
    let root = home.join("Documents/project");
    std::fs::create_dir_all(root.join(".antiburn/checks")).unwrap();
    std::fs::write(root.join("AGENTS.md"), "Protected repo\n").unwrap();
    fixture
        .store
        .replace_repositories(&[repository(&root)])
        .unwrap();
    fixture.publish_in_cwd("native", "main", 2, &root);
    let mut host = host(&fixture, &home);
    host.reconcile(&Admission::Ready).unwrap();
    host.catch_up().unwrap();
    let candidates = frozen(&fixture.store);
    assert_eq!(candidates.len(), 1);
    assert!(
        candidates[0].context.value["global"]
            .get("instructions")
            .is_some()
    );
    assert!(
        candidates[0].context.value["repo"]
            .get("instructions")
            .is_none()
    );
    assert_eq!(
        candidates[0].context.value["repo"]["instructions_limits"],
        json!(["protected_path_not_granted"])
    );
    fixture.store.grant_dir("Documents").unwrap();
    fixture.publish_in_cwd("native", "main", 3, &root);
    host.catch_up().unwrap();
    let candidates = frozen(&fixture.store);
    assert_eq!(
        candidates.last().unwrap().context.value["repo"]["instructions"][0]["content"],
        "Protected repo\n"
    );
}
