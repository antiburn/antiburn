use super::*;

#[test]
fn session_deletion_removes_gap_and_attachment_selector_state() {
    let fixture = Fixture::new();
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    let scope = host.projects.keys().next().unwrap().clone();
    let adapter = host.projects[&scope].store.clone();
    let activation = host.projects[&scope].active[0].clone();
    fixture.store.lock().execute(
        "INSERT INTO native_tool_result_publication (outcome, environment_key, agent, session_id,
             source_format, complete, source_generation, claim_fence, gap_reason)
         VALUES ('coverage_gap', 'native', 'pi', 'main', 'pi_v3_jsonl', 0, 1, 1, 'missing source history')",
        [],
    ).unwrap();
    host.catch_up().unwrap();
    let progress = adapter.load_source_progress(&activation).unwrap();
    assert_eq!(progress.selection.branches.len(), 1);
    assert!(
        !progress
            .selection
            .branches
            .values()
            .next()
            .unwrap()
            .coverage_complete
    );
    for table in ["burn_check_observation", "burn_check_candidate"] {
        assert_eq!(
            fixture
                .store
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, u64>(0),)
                .unwrap(),
            u64::from(table == "burn_check_observation")
        );
    }

    fixture
        .store
        .delete_session(&SessionKey::new("native", "pi", "main"))
        .unwrap()
        .unwrap();
    let cleared = adapter.load_source_progress(&activation).unwrap();
    assert!(cleared.selection.branches.is_empty());
    assert!(cleared.selection.seen_results.is_empty());
    assert_eq!(cleared.publication_cursor, progress.publication_cursor);
    assert_eq!(cleared.observation_position, progress.observation_position);
}

#[tokio::test]
async fn session_recreation_reuses_native_ids_without_old_selection_or_status() {
    let fixture = Fixture::new();
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    let scope = host.projects.keys().next().unwrap().clone();
    let adapter = host.projects[&scope].store.clone();
    let activation = host.projects[&scope].active[0].clone();
    let key = SessionKey::new("native", "pi", "main");

    fixture.publish("main", 3);
    host.catch_up().unwrap();
    fixture.evaluator.answer("stuck");
    assert_eq!(
        run_due_at(&mut host, 100_000).await.outcome,
        RunOutcome::Accepted
    );
    assert_eq!(
        status_writes(&fixture.store, &host.definitions[0])[0].0,
        "failure"
    );

    fixture.publish("main", 4);
    host.catch_up().unwrap();
    let RecoveryWork::Pending {
        candidate: old_candidate,
        ..
    } = adapter.load_recovery(None, 10).unwrap().remove(0).work
    else {
        panic!("pending work must exist before deletion");
    };
    fixture.publish("main", 5);
    let progress = adapter.load_source_progress(&activation).unwrap();
    let stale_page = antiburn_checks::source::PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_project(&fixture.store, "native", &scope.session_cwd),
        20,
    )
    .unwrap()
    .read_batch(progress.publication_cursor)
    .unwrap();
    assert!(!stale_page.events.is_empty());

    let (old_incarnation, _) = fixture.store.delete_session(&key).unwrap().unwrap();
    let cleared = adapter.load_source_progress(&activation).unwrap();
    assert_eq!(
        cleared.head().publication_cursor,
        progress.head().publication_cursor
    );
    assert_eq!(cleared.observation_position, progress.observation_position);
    assert!(cleared.selection.branches.is_empty());
    assert!(cleared.selection.seen_results.is_empty());
    assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
    assert!(status_writes(&fixture.store, &host.definitions[0]).is_empty());

    fixture.publish("main", 0);
    let new_incarnation: u64 = fixture.store.lock().query_row(
        "SELECT incarnation FROM session WHERE environment_key = 'native' AND agent = 'pi' AND session_id = 'main'",
        [], |row| row.get(0),
    ).unwrap();
    assert!(new_incarnation > old_incarnation.0);
    assert_eq!(
        adapter.claim_due(&old_candidate, 100_000, true).unwrap(),
        CandidateClaim::Stale
    );
    let stale_engine = CheckEngine::new(adapter.clone(), fixture.evaluator.clone());
    assert!(
        stale_engine
            .apply_source_page(&activation, &stale_page)
            .is_err()
    );
    assert!(host.catch_up().is_err());
    host.catch_up().unwrap();

    fixture.publish("main", 2);
    host.catch_up().unwrap();
    assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
    fixture.publish("main", 3);
    host.catch_up().unwrap();
    let RecoveryWork::Pending {
        candidate: new_candidate,
        ..
    } = adapter.load_recovery(None, 10).unwrap().remove(0).work
    else {
        panic!("re-created native results must produce fresh work");
    };
    assert_eq!(new_candidate.activation, old_candidate.activation);
    assert_eq!(new_candidate.branch, old_candidate.branch);
    assert_eq!(new_candidate.branch.0, key.to_identity_json());
    assert_eq!(new_candidate.messages, old_candidate.messages[..3]);
    assert!(new_candidate.trigger_position > old_candidate.trigger_position);
    assert_ne!(new_candidate.fingerprint, old_candidate.fingerprint);

    drop(host);
    let mut restarted = fixture.host();
    restarted.reconcile(&Admission::Ready).unwrap();
    fixture.evaluator.answer("clear");
    assert_eq!(
        run_due_at(&mut restarted, 100_000).await.outcome,
        RunOutcome::Accepted
    );
    assert_eq!(
        status_writes(&fixture.store, &restarted.definitions[0])[0].0,
        "success"
    );
    assert_eq!(fixture.evaluator.inputs.lock().unwrap().len(), 2);
}
