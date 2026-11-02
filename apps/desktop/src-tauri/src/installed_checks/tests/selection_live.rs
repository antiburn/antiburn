use antiburn_checks::check_store::SourceProgress;
use antiburn_checks::domain::BranchId;
use antiburn_local::analysis::ToolResultPosition;

use super::*;
use crate::insights_worker::journal::tests::{SyntheticJournal, run_page};

fn open_host(
    store: &Arc<Store>,
    evaluator: &Arc<ScriptedEvaluator>,
    global: &Path,
) -> InstalledCheckHost<ScriptedEvaluator> {
    let mut host =
        InstalledCheckHost::new(store.clone(), evaluator.clone(), Some(global.to_path_buf()))
            .unwrap();
    host.reconcile(&Admission::Ready).unwrap();
    host
}

fn catch_up(host: &mut InstalledCheckHost<ScriptedEvaluator>) {
    for _ in 0..20 {
        if !host.catch_up().unwrap().1 {
            return;
        }
    }
    panic!("live selector publications did not drain");
}

fn progress(host: &InstalledCheckHost<ScriptedEvaluator>) -> SourceProgress {
    let project = host.projects.values().next().unwrap();
    project
        .store
        .load_source_progress(&project.active[0])
        .unwrap()
}

fn tool_update(source: &mut SyntheticJournal, index: usize, status: &str, progress: &str) -> u64 {
    let mut state = json!({
        "status": status,
        "input": {"command": "cargo test parser"},
    });
    match status {
        "pending" => state["raw"] = json!(""),
        "running" => {
            state["title"] = json!("Parser tests");
            state["metadata"] = json!({"output": progress});
            state["time"] = json!({"start": 1000});
        }
        "error" => {
            state["error"] = json!(format!("Parser failure {index}"));
            state["time"] = json!({"start": 1000, "end": 2000});
        }
        _ => panic!("unsupported test lifecycle state"),
    }
    source.event(
        "message.part.updated.1",
        json!({
            "sessionID": "native-session",
            "part": {
                "id": format!("part-{index}"), "messageID": "assistant",
                "sessionID": "native-session", "type": "tool",
                "callID": format!("call-{index}"), "tool": "bash", "state": state,
            },
        }),
    );
    u64::try_from(source.sequence).unwrap()
}

fn candidate_count(store: &Store) -> u64 {
    store
        .lock()
        .query_row("SELECT COUNT(*) FROM burn_check_candidate", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[tokio::test]
async fn opencode_progress_lifecycles_reach_one_assessment_across_store_and_host_restart() {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let global = root.join("global/checks");
    let project = root.join("project");
    let state = root.join("state");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(global.join("failed-retry-loop.yml"), CHECK).unwrap();
    let store = Arc::new(Store::open(&state).unwrap());
    store
        .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "true")
        .unwrap();
    store.replace_repositories(&[repository(&project)]).unwrap();
    let mut source = SyntheticJournal::new(16);
    source.index(&store, project.to_str().unwrap());
    let evaluator = Arc::new(ScriptedEvaluator::default());
    let mut host = open_host(&store, &evaluator, &global);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    assert!(run_page(&store, source.source(), now).await);
    catch_up(&mut host);
    let activation = host.projects.values().next().unwrap().active[0].clone();
    let branch =
        BranchId(SessionKey::new("native", "opencode", "native-session").to_identity_json());
    let initial_coverage = progress(&host).selection.branches[&branch].coverage_complete;
    assert_eq!(candidate_count(&store), 0);
    let mut terminals = Vec::new();
    let lifecycle = [
        (0, "pending", ""),
        (1, "pending", ""),
        (0, "running", "starting"),
        (1, "running", "starting"),
        (1, "running", "test output"),
        (1, "error", ""),
        (0, "running", "more test output"),
        (0, "error", ""),
        (2, "pending", ""),
        (2, "running", "starting"),
    ];
    for (turn, (index, status, output)) in lifecycle.into_iter().enumerate() {
        let sequence = tool_update(&mut source, index, status, output);
        source.index(&store, project.to_str().unwrap());
        assert!(run_page(&store, source.source(), now + turn as i64 + 1).await);
        catch_up(&mut host);
        if status == "error" {
            terminals.push((index, sequence));
        }
        let saved = progress(&host);
        assert_eq!(
            saved.selection.branches[&branch].results.len(),
            terminals.len()
        );
        assert_eq!(
            saved.selection.branches[&branch].coverage_complete,
            initial_coverage
        );
        assert_eq!(candidate_count(&store), 0);
    }
    let saved = progress(&host);
    let attachments: u64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM burn_check_session_attachment",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(attachments, 1);
    drop(host);
    drop(store);

    let store = Arc::new(Store::open(&state).unwrap());
    let mut host = open_host(&store, &evaluator, &global);
    catch_up(&mut host);
    assert_eq!(host.projects.values().next().unwrap().active[0], activation);
    assert_eq!(progress(&host), saved);
    assert_eq!(candidate_count(&store), 0);
    tool_update(&mut source, 1, "error", "");
    source.index(&store, project.to_str().unwrap());
    assert!(run_page(&store, source.source(), now + 19).await);
    catch_up(&mut host);
    assert_eq!(progress(&host).selection.branches[&branch].results.len(), 2);
    assert_eq!(candidate_count(&store), 0);
    for (turn, (status, output)) in [
        ("running", "first output"),
        ("running", "later output"),
        ("error", ""),
    ]
    .into_iter()
    .enumerate()
    {
        let sequence = tool_update(&mut source, 2, status, output);
        source.index(&store, project.to_str().unwrap());
        assert!(run_page(&store, source.source(), now + 20 + turn as i64).await);
        catch_up(&mut host);
        if status == "error" {
            terminals.push((2, sequence));
        }
        assert_eq!(
            progress(&host).selection.branches[&branch].results.len(),
            terminals.len()
        );
        assert_eq!(candidate_count(&store), u64::from(status == "error"));
    }
    assert_eq!(
        terminals
            .iter()
            .map(|(index, _)| *index)
            .collect::<Vec<_>>(),
        [1, 0, 2]
    );
    let frozen: String = store
        .lock()
        .query_row(
            "SELECT frozen_input_json FROM burn_check_candidate",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let candidate: FrozenCandidate = serde_json::from_str(&frozen).unwrap();
    assert_eq!(candidate.messages.len(), 3);
    for (message, (index, sequence)) in candidate.messages.iter().zip(&terminals) {
        let identity: Value = serde_json::from_str(&message.id).unwrap();
        assert_eq!(identity[2], format!("call-{index}"));
        let native: ToolResultPosition =
            serde_json::from_str(message.value["native_order"].as_str().unwrap()).unwrap();
        assert_eq!(
            native,
            ToolResultPosition::OpenCodeEvent {
                sequence: *sequence,
                item_index: 0
            }
        );
        let (publication, payload, first_terminal): (u64, String, u64) = store
            .lock()
            .query_row(
                "SELECT o.publication_sequence, o.payload_json,
                (SELECT MIN(p.publication_sequence) FROM native_tool_result_publication p
                 WHERE p.native_identity = o.native_identity AND p.outcome = 'update'
                   AND json_extract(p.payload_json, '$.finality') = 1)
             FROM burn_check_observation o WHERE o.native_identity = ?1",
                [&message.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let draft: antiburn_checks::domain::ObservationDraft =
            serde_json::from_str(&payload).unwrap();
        assert_eq!(
            draft.first_terminal_publication,
            Some(antiburn_checks::source::PublicationCursor(first_terminal))
        );
        assert_eq!(publication > first_terminal, *index == 1);
    }
    evaluator.answer("stuck");
    assert_eq!(
        host.evaluation_jobs(1)
            .pop()
            .unwrap()
            .run()
            .await
            .unwrap()
            .unwrap()
            .outcome,
        RunOutcome::Accepted
    );
    let inputs = evaluator.inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0]["activity"].as_array().unwrap().len(), 3);
    assert_eq!(
        inputs[0]["source_coverage"],
        if initial_coverage {
            "complete"
        } else {
            "partial"
        }
    );
    assert_eq!(status_writes(&store, &host.definitions[0])[0].0, "failure");
    assert_eq!(store.lock().query_row("SELECT COUNT(*) FROM native_tool_result_publication WHERE outcome = 'coverage_gap'", [], |row| row.get::<_, u64>(0)).unwrap(), 0);
    let (terminal_updates, terminal_identities): (u64, u64) = store
        .lock()
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT native_identity) FROM native_tool_result_publication
         WHERE outcome = 'update' AND json_extract(payload_json, '$.finality') = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((terminal_updates, terminal_identities), (4, 3));
    assert_eq!(store.lock().query_row(
        "SELECT COUNT(DISTINCT json_extract(payload_json, '$.context.result_position.OpenCodeEvent.sequence'))
         FROM native_tool_result_publication WHERE outcome = 'update' AND json_extract(payload_json, '$.finality') = 0",
        [], |row| row.get::<_, u64>(0),
    ).unwrap(), 10);
    let watermark: u64 = store
        .lock()
        .query_row(
            "SELECT position FROM burn_check_acceptance_watermark",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(watermark, candidate.trigger_position.0);
    drop(host);
    drop(store);

    let store = Arc::new(Store::open(&state).unwrap());
    let mut host = open_host(&store, &evaluator, &global);
    catch_up(&mut host);
    assert!(
        host.evaluation_jobs(1)
            .pop()
            .unwrap()
            .run()
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(candidate_count(&store), 1);
    assert_eq!(evaluator.inputs.lock().unwrap().len(), 1);
    assert_eq!(status_writes(&store, &host.definitions[0])[0].0, "failure");
    let (assessments, attempts, attachments): (u64, u64, u64) = store
        .lock()
        .query_row(
            "SELECT (SELECT COUNT(*) FROM burn_check_assessment_history),
                (SELECT COUNT(*) FROM burn_check_evaluation_attempt),
                (SELECT COUNT(*) FROM burn_check_session_attachment)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((assessments, attempts, attachments), (1, 1, 1));
}
