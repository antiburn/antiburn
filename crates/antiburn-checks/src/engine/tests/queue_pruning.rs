use super::*;

fn queue_ids(engine: &CheckEngine<FakeStore, FakeEvaluator>) -> BTreeSet<CandidateId> {
    engine
        .state
        .lock()
        .unwrap()
        .pending
        .values()
        .flat_map(BTreeMap::values)
        .map(|pending| CandidateId::from(&pending.candidate))
        .collect()
}

fn append_results(
    engine: &CheckEngine<FakeStore, FakeEvaluator>,
    activation: &EffectiveActivation,
    first: u64,
    last: u64,
) -> Result<BatchOutcome, StoreError> {
    engine.apply_source_page(
        activation,
        &SourceBatch {
            events: (first..=last)
                .map(|cursor| source_event(cursor, &format!("result-{cursor}")))
                .collect(),
            next: PublicationCursor(last),
            has_more: false,
        },
    )
}

fn cancel_pending(store: &FakeStore) -> BTreeSet<CandidateId> {
    let ids = store
        .pending
        .lock()
        .unwrap()
        .drain(..)
        .map(|record| CandidateId::from(&record.candidate))
        .collect::<BTreeSet<_>>();
    store.terminal.lock().unwrap().extend(ids.clone());
    ids
}

fn restore_claim_during_fresh_admission(
    engine: &Arc<CheckEngine<FakeStore, FakeEvaluator>>,
    store: &Arc<FakeStore>,
    activation: &EffectiveActivation,
) -> BTreeSet<CandidateId> {
    append_results(engine, activation, 1, 128).unwrap();
    let original = queue_ids(engine);
    assert_eq!(original.len(), MAX_PENDING_CANDIDATES);
    *store.cooldown_until_unix_ms.lock().unwrap() = 10_000;
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    *store.claim_barriers.lock().unwrap() = Some((entered.clone(), release.clone()));
    let claiming_engine = engine.clone();
    let claiming_activation = activation.clone();
    let claim = std::thread::spawn(move || {
        matches!(
            claiming_engine
                .start_one_due(&claiming_activation, 0)
                .unwrap(),
            StartOutcome::NotDue
        )
    });
    entered.wait();
    assert_eq!(queue_ids(engine).len(), MAX_PENDING_CANDIDATES - 1);
    let fresh = append_results(engine, activation, 129, 129).unwrap();
    assert_eq!((fresh.candidates, fresh.overflowed), (1, 0));
    assert_eq!(queue_ids(engine).len(), MAX_PENDING_CANDIDATES);
    release.wait();
    assert!(claim.join().unwrap());
    assert_eq!(queue_ids(engine).len(), MAX_PENDING_CANDIDATES + 1);
    assert!(store.terminal_lookup_calls.lock().unwrap().is_empty());
    original
}

fn cancel_ids(store: &FakeStore, ids: &BTreeSet<CandidateId>) {
    store.terminal.lock().unwrap().extend(ids.iter().cloned());
    store
        .pending
        .lock()
        .unwrap()
        .retain(|record| !ids.contains(&CandidateId::from(&record.candidate)));
}

#[test]
fn restored_claim_over_capacity_uses_bounded_lookups_and_preserves_fresh_work() {
    let store = Arc::new(FakeStore::default());
    let engine = Arc::new(CheckEngine::new(store.clone(), Arc::new(FakeEvaluator)));
    let activation = activation();
    let original = restore_claim_during_fresh_admission(&engine, &store, &activation);
    let fresh = store
        .stored_candidates
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    cancel_ids(&store, &original);
    let progress = store.progress.lock().unwrap().clone();
    let selectors = engine.selectors.lock().unwrap().clone();
    let cooldowns = engine.state.lock().unwrap().cooldown_until_unix_ms.clone();
    let snapshot = queue_ids(&engine);
    let recovered = engine.recover_pending().unwrap();
    assert_eq!((recovered.candidates, recovered.overflowed), (1, 0));
    assert_eq!(
        queue_ids(&engine),
        BTreeSet::from([CandidateId::from(&fresh)])
    );
    let calls = store.terminal_lookup_calls.lock().unwrap().clone();
    assert_eq!(calls.iter().map(Vec::len).collect::<Vec<_>>(), [128, 1]);
    assert_eq!(
        calls.into_iter().flatten().collect::<BTreeSet<_>>(),
        snapshot
    );
    assert_eq!(*store.terminal.lock().unwrap(), original);
    assert!(store.overflowed.lock().unwrap().is_empty());
    assert_eq!(*store.progress.lock().unwrap(), progress);
    assert_eq!(*engine.selectors.lock().unwrap(), selectors);
    assert_eq!(
        engine.state.lock().unwrap().cooldown_until_unix_ms,
        cooldowns
    );
    assert_eq!(*store.cooldown_until_unix_ms.lock().unwrap(), 10_000);
    assert!(matches!(
        engine.start_one_due(&activation, 9_999).unwrap(),
        StartOutcome::NotDue,
    ));
    let StartOutcome::Started(task) = engine.start_one_due(&activation, 10_000).unwrap() else {
        panic!("fresh work must keep its durable cooldown and remain runnable");
    };
    assert_eq!(run_task(task).outcome, RunOutcome::Accepted);
    assert_eq!(*store.dispatches.lock().unwrap(), 1);
    assert_eq!(store.accepted.lock().unwrap()[0].claimed.candidate, fresh);
}

#[test]
fn later_batch_failure_keeps_the_whole_queue_and_new_durable_work() {
    let store = Arc::new(FakeStore::default());
    let engine = Arc::new(CheckEngine::new(store.clone(), Arc::new(FakeEvaluator)));
    let activation = activation();
    let original = restore_claim_during_fresh_admission(&engine, &store, &activation);
    cancel_ids(&store, &original);
    let unchanged_queue = queue_ids(&engine);
    let cooldowns = engine.state.lock().unwrap().cooldown_until_unix_ms.clone();
    *store.fail_terminal_lookup_on_call.lock().unwrap() = Some(2);
    let error = append_results(&engine, &activation, 130, 130).unwrap_err();
    assert!(error.message.contains("terminal candidate lookup failure"));
    assert_eq!(queue_ids(&engine), unchanged_queue);
    assert!(store.overflowed.lock().unwrap().is_empty());
    assert_eq!(store.pending.lock().unwrap().len(), 2);
    let fresh = store
        .pending
        .lock()
        .unwrap()
        .iter()
        .map(|record| CandidateId::from(&record.candidate))
        .collect::<BTreeSet<_>>();
    let progress = store.progress.lock().unwrap().clone();
    assert_eq!(progress.publication_cursor, PublicationCursor(130));
    assert_eq!(
        engine.state.lock().unwrap().cooldown_until_unix_ms,
        cooldowns
    );

    *store.fail_terminal_lookup_on_call.lock().unwrap() = None;
    let recovered = engine.recover_pending().unwrap();
    assert_eq!((recovered.candidates, recovered.overflowed), (2, 0));
    assert_eq!(queue_ids(&engine), fresh);
    assert_eq!(*store.progress.lock().unwrap(), progress);
    assert_eq!(
        engine.state.lock().unwrap().cooldown_until_unix_ms,
        cooldowns
    );
    assert_eq!(
        store
            .terminal_lookup_calls
            .lock()
            .unwrap()
            .iter()
            .map(Vec::len)
            .collect::<Vec<_>>(),
        [128, 1, 128, 1],
    );
    assert_eq!(*store.terminal.lock().unwrap(), original);
    assert!(store.overflowed.lock().unwrap().is_empty());
    assert_eq!(*store.dispatches.lock().unwrap(), 0);
}

#[test]
fn terminal_lookup_rejects_an_id_from_another_snapshot_batch() {
    let store = Arc::new(FakeStore::default());
    let engine = Arc::new(CheckEngine::new(store.clone(), Arc::new(FakeEvaluator)));
    let activation = activation();
    let original = restore_claim_during_fresh_admission(&engine, &store, &activation);
    cancel_ids(&store, &original);
    let unchanged_queue = queue_ids(&engine);
    *store.terminal_lookup_extra_id.lock().unwrap() = unchanged_queue.last().cloned();
    let error = engine.recover_pending().unwrap_err();
    assert!(error.message.contains("unrequested identity"));
    assert_eq!(queue_ids(&engine), unchanged_queue);
    let calls = store.terminal_lookup_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].len(), MAX_PENDING_CANDIDATES);
    assert!(!calls[0].contains(unchanged_queue.last().unwrap()));
    assert!(store.overflowed.lock().unwrap().is_empty());

    let recovered = engine.recover_pending().unwrap();
    assert_eq!((recovered.candidates, recovered.overflowed), (1, 0));
    let fresh = store.pending.lock().unwrap()[0].candidate.clone();
    assert_eq!(
        queue_ids(&engine),
        BTreeSet::from([CandidateId::from(&fresh)])
    );
    assert_eq!(*store.terminal.lock().unwrap(), original);
}

#[test]
fn full_cancelled_queue_admits_fresh_work_without_reset_or_reconciliation() {
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let activation = activation();
    append_results(&engine, &activation, 1, 1).unwrap();
    let StartOutcome::Started(task) = engine.start_one_due(&activation, 0).unwrap() else {
        panic!("the first result must establish a retained status and cooldown");
    };
    assert_eq!(run_task(task).outcome, RunOutcome::Accepted);
    append_results(&engine, &activation, 2, 129).unwrap();
    assert_eq!(queue_ids(&engine).len(), MAX_PENDING_CANDIDATES);
    let cooldowns = engine.state.lock().unwrap().cooldown_until_unix_ms.clone();
    let progress = store.progress.lock().unwrap().clone();
    let accepted = store.accepted.lock().unwrap().clone();
    let terminal = cancel_pending(&store);
    assert_eq!(terminal.len(), MAX_PENDING_CANDIDATES);

    let outcome = append_results(&engine, &activation, 130, 130).unwrap();
    assert_eq!((outcome.candidates, outcome.overflowed), (1, 0));
    let fresh = store.pending.lock().unwrap()[0].candidate.clone();
    assert_eq!(fresh.activation, activation.id);
    assert_eq!(
        queue_ids(&engine),
        BTreeSet::from([CandidateId::from(&fresh)])
    );
    assert_eq!(*store.terminal.lock().unwrap(), terminal);
    assert!(store.overflowed.lock().unwrap().is_empty());
    assert_eq!(*store.accepted.lock().unwrap(), accepted);
    assert_eq!(
        engine.state.lock().unwrap().cooldown_until_unix_ms,
        cooldowns
    );
    let current = store.progress.lock().unwrap().clone();
    for (key, seen) in progress.selection.seen_results {
        let retained = current.selection.seen_results.get(&key).unwrap();
        assert_eq!(retained.terminal_seen, seen.terminal_seen);
        assert_eq!(retained.revision, seen.revision);
        assert_eq!(retained.version, seen.version);
        assert_eq!(retained.native_order, seen.native_order);
    }
    assert_eq!(current.observation_position, ObservationPosition(130));
    assert_eq!(current.publication_cursor, PublicationCursor(130));
    assert!(matches!(
        engine.start_one_due(&activation, 9_999).unwrap(),
        StartOutcome::NotDue,
    ));
    let StartOutcome::Started(task) = engine.start_one_due(&activation, 10_000).unwrap() else {
        panic!("fresh work must run when the unchanged cooldown ends");
    };
    assert_eq!(run_task(task).outcome, RunOutcome::Accepted);
    let accepted = store.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 2);
    assert_eq!(accepted[1].claimed.candidate.fingerprint, fresh.fingerprint);
    assert_eq!(*store.dispatches.lock().unwrap(), 2);
}

#[test]
fn recovery_prunes_terminal_entries_and_preserves_work_added_after_cancellation() {
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let activation = activation();
    append_results(&engine, &activation, 1, 127).unwrap();
    let terminal = cancel_pending(&store);
    append_results(&engine, &activation, 128, 128).unwrap();
    assert_eq!(queue_ids(&engine).len(), MAX_PENDING_CANDIDATES);
    let fresh = store.pending.lock().unwrap()[0].candidate.clone();
    let progress = store.progress.lock().unwrap().clone();
    let selectors = engine.selectors.lock().unwrap().clone();
    let recovered = engine.recover_pending().unwrap();
    assert_eq!((recovered.candidates, recovered.overflowed), (1, 0));
    assert_eq!(
        queue_ids(&engine),
        BTreeSet::from([CandidateId::from(&fresh)])
    );
    assert_eq!(*store.progress.lock().unwrap(), progress);
    assert_eq!(*engine.selectors.lock().unwrap(), selectors);
    assert_eq!(*store.terminal.lock().unwrap(), terminal);
    assert!(store.overflowed.lock().unwrap().is_empty());
    assert_eq!(engine.recover_pending().unwrap().overflowed, 0);
    let StartOutcome::Started(task) = engine.start_one_due(&activation, 0).unwrap() else {
        panic!("recovery must keep the fresh candidate runnable");
    };
    assert_eq!(run_task(task).outcome, RunOutcome::Accepted);
    assert_eq!(
        store.accepted.lock().unwrap()[0]
            .claimed
            .candidate
            .fingerprint,
        fresh.fingerprint
    );
    assert_eq!(*store.dispatches.lock().unwrap(), 1);
}

#[test]
fn terminal_lookup_failure_keeps_fresh_durable_work_without_false_overflow() {
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let activation = activation();
    append_results(&engine, &activation, 1, 128).unwrap();
    let terminal = cancel_pending(&store);
    *store.fail_terminal_lookup.lock().unwrap() = true;
    let error = append_results(&engine, &activation, 129, 129).unwrap_err();
    assert!(error.message.contains("terminal candidate lookup failure"));
    assert_eq!(queue_ids(&engine), terminal);
    assert!(store.overflowed.lock().unwrap().is_empty());
    assert_eq!(store.pending.lock().unwrap().len(), 1);
    let fresh = store.pending.lock().unwrap()[0].candidate.clone();
    assert_eq!(fresh.trigger_position, ObservationPosition(129));
    assert_eq!(
        store.progress.lock().unwrap().publication_cursor,
        PublicationCursor(129)
    );
    assert!(engine.recover_pending().is_err());
    assert_eq!(queue_ids(&engine), terminal);

    *store.fail_terminal_lookup.lock().unwrap() = false;
    let recovered = engine.recover_pending().unwrap();
    assert_eq!((recovered.candidates, recovered.overflowed), (1, 0));
    assert_eq!(
        queue_ids(&engine),
        BTreeSet::from([CandidateId::from(&fresh)])
    );
    let StartOutcome::Started(task) = engine.start_one_due(&activation, 0).unwrap() else {
        panic!("fresh work must survive the temporary lookup failure");
    };
    assert_eq!(run_task(task).outcome, RunOutcome::Accepted);
}

#[test]
fn recovery_does_not_treat_absence_from_recovery_as_terminal_proof() {
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let activation = activation();
    append_results(&engine, &activation, 1, 1).unwrap();
    let queued = queue_ids(&engine);
    store.pending.lock().unwrap().clear();
    assert_eq!(engine.recover_pending().unwrap().candidates, 0);
    assert_eq!(queue_ids(&engine), queued);
    assert!(store.terminal.lock().unwrap().is_empty());
}

#[test]
fn full_unfinished_queue_still_enforces_the_existing_capacity() {
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let activation = activation();
    append_results(&engine, &activation, 1, 128).unwrap();
    let queued = queue_ids(&engine);
    assert_eq!(queued.len(), MAX_PENDING_CANDIDATES);
    let outcome = append_results(&engine, &activation, 129, 129).unwrap();
    assert_eq!((outcome.candidates, outcome.overflowed), (1, 1));
    assert_eq!(queue_ids(&engine), queued);
    assert_eq!(store.pending.lock().unwrap().len(), MAX_PENDING_CANDIDATES);
    assert_eq!(store.overflowed.lock().unwrap().len(), 1);
}
