//! The actor's page machinery against a scripted store: admission pages,
//! presence walks, backoff, the blocked carry, and the paged seed.

use super::*;

fn keys_named(prefix: &str, count: usize) -> Vec<SessionKey> {
    (0..count)
        .map(|index| key(&format!("{prefix}{index:04}")))
        .collect()
}

fn indexed_chunks(keys: &[SessionKey], at: i64, revision: u64) -> Vec<Observation> {
    keys.chunks(INDEXED_CHUNK)
        .map(|chunk| Observation::Indexed {
            sessions: chunk
                .iter()
                .map(|key| IndexedSession {
                    key: key.clone(),
                    agent: AgentKind::Claude,
                    incarnation: Incarnation(1),
                    at,
                    is_new: false,
                })
                .collect(),
            revision: Revision(revision),
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn page_acknowledgements_follow_actor_processing_and_error_backoff() {
    let source = Arc::new(Scripted::default());
    source.set_rows(vec![(key("waiting"), 1, BASE)], 20);
    source.fail_next(1);
    source.hold();
    let harness = start_with(BASE, Vec::new(), source.clone());
    let events = &harness.events;
    harness.push(broad(RemovalReason::Purged, 10));
    harness.push(index("waiting", 1, BASE, false, 5));

    wait_until(|| source.answers.load(Ordering::Relaxed) == 1).await;
    assert_eq!(events.test_probe.processed(), 0);
    assert!(
        events
            .test_probe
            .progress
            .lock()
            .unwrap()
            .next_allowed
            .is_none()
    );
    assert!(
        !events
            .test_probe
            .page_task
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .is_finished(),
        "the computed error still waits at the return gate"
    );
    source.allow(1);
    events.test_probe.wait_for_finished_task();
    assert_eq!(
        events.test_probe.processed(),
        0,
        "task completion is not processing"
    );

    wait_for_processed_pages(events, 1).await;
    assert_eq!(
        events.test_probe.progress.lock().unwrap().next_allowed,
        Some(Instant::now() + RECONCILE_BACKOFF_MIN),
        "the actor installs backoff before acknowledging the error"
    );
    assert!(live(events).is_empty());
    tokio::time::advance(RECONCILE_BACKOFF_MIN - Duration::from_millis(1)).await;
    settle().await;
    assert_eq!(source.requests().len(), 1, "no retry before backoff");
    tokio::time::advance(Duration::from_millis(1)).await;
    wait_until(|| source.answers.load(Ordering::Relaxed) == 2).await;
    assert_eq!(source.requests(), vec![vec![key("waiting")]; 2]);
    assert_eq!(
        events.test_probe.processed(),
        1,
        "the successful answer remains held"
    );
    assert!(live(events).is_empty());

    source.release();
    events.test_probe.wait_for_finished_task();
    assert_eq!(
        events.test_probe.processed(),
        1,
        "the actor has not consumed the success"
    );
    assert!(live(events).is_empty());
    wait_for_processed_pages(events, 2).await;
    assert!(
        events
            .test_probe
            .progress
            .lock()
            .unwrap()
            .next_allowed
            .is_none()
    );
    assert_eq!(live(events)[0].session, session_ref("waiting"));
    assert!(harness.with_registry(|registry| registry.pending.is_empty()));
}

#[tokio::test(start_paused = true)]
async fn a_failed_page_keeps_the_cursor_and_retries_after_backoff() {
    let walked = keys_named("w", 600);
    let source = Arc::new(Scripted::default());
    // Every row but the last one is still present.
    source.set_rows(
        walked[..599]
            .iter()
            .map(|key| (key.clone(), 0, BASE))
            .collect(),
        20,
    );
    let harness = start_with(
        BASE,
        walked.iter().map(|key| (key.clone(), BASE)).collect(),
        source.clone(),
    );
    let mut bus = harness.events.subscribe();

    // The purge walks 600 entries in pages of 256, 256, 88. The third page
    // fails.
    source.hold();
    harness
        .events
        .report_async(broad(RemovalReason::Purged, 10))
        .await;
    wait_until(|| source.requests().len() == 1).await;
    source.allow(1);
    wait_until(|| source.requests().len() == 2).await;
    // Page 2 is computed and held. The failure budget lands before page 3
    // is computed.
    source.fail_next(1);
    source.allow(1);
    wait_until(|| source.requests().len() == 3).await;
    source.release();
    wait_for_processed_pages(&harness.events, 3).await;
    let requests = source.requests();
    assert_eq!(requests[0], walked[..256]);
    assert_eq!(requests[1], walked[256..512]);
    assert_eq!(requests[2], walked[512..]);
    assert_eq!(
        harness.with_registry(|registry| registry
            .reconcile
            .run
            .as_ref()
            .map(|run| run.cursor.clone())),
        Some(Some(walked[511].clone())),
        "the cursor stays before the failed page"
    );
    assert_eq!(
        live(&harness.events).len(),
        600,
        "nothing left on a failed page"
    );

    // Nothing is retried before the backoff elapses.
    tokio::time::sleep(RECONCILE_BACKOFF_MIN - Duration::from_millis(500)).await;
    settle().await;
    assert_eq!(source.requests().len(), 3);

    tokio::time::sleep(Duration::from_secs(1)).await;
    wait_until(|| source.requests().len() == 4).await;
    assert_eq!(source.requests()[3], walked[512..], "the same page again");
    wait_until(|| live(&harness.events).len() == 599).await;
    let events = drain(&mut bus);
    assert_eq!(
        events,
        vec![
            SessionEvent::Removed {
                session: None,
                reason: RemovalReason::Purged,
            },
            SessionEvent::Idle {
                session: SessionRef::from(&walked[599]),
                agent: AgentKind::Claude,
                at: BASE + 2,
            },
            SessionEvent::Removed {
                session: Some(SessionRef::from(&walked[599])),
                reason: RemovalReason::Purged,
            },
        ]
    );
    assert_eq!(
        harness
            .with_registry(|registry| (registry.reconcile.run.is_none(), registry.reconcile.needs)),
        (true, None),
        "the walk completed"
    );
}

#[tokio::test(start_paused = true)]
async fn a_broad_removal_walks_only_entries_below_its_revision() {
    let seeded = keys_named("s", 700);
    let admitted = keys_named("a", 300);
    let source = Arc::new(Scripted::default());
    source.set_rows(
        seeded
            .iter()
            .map(|key| (key.clone(), 0, BASE))
            .chain(admitted.iter().map(|key| (key.clone(), 1, BASE)))
            .collect(),
        30,
    );
    let harness = start_with(
        BASE,
        seeded.iter().map(|key| (key.clone(), BASE)).collect(),
        source.clone(),
    );
    for chunk in indexed_chunks(&admitted, BASE, 20) {
        harness.events.report_async(chunk).await;
    }
    wait_until(|| live(&harness.events).len() == 1000).await;

    harness
        .events
        .report_async(broad(RemovalReason::Purged, 10))
        .await;
    wait_until(|| source.requests().len() == 3).await;
    settle().await;
    let requests = source.requests();
    assert_eq!(
        requests.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![256, 256, 188],
        "700 entries predate the purge: three pages"
    );
    let checked = requests.concat();
    assert_eq!(
        checked, seeded,
        "only entries below the revision, in key order"
    );
    assert_eq!(
        live(&harness.events).len(),
        1000,
        "every row is still present"
    );
    wait_until(|| harness.with_registry(|registry| registry.reconcile.run.is_none())).await;
    assert_eq!(source.requests().len(), 3, "no fourth page");
}

#[tokio::test(start_paused = true)]
async fn admission_and_walk_pages_alternate_while_both_have_work() {
    let walked = keys_named("w", 600);
    let deferred = keys_named("p", 600);
    let source = Arc::new(Scripted::default());
    source.set_rows(
        walked
            .iter()
            .map(|key| (key.clone(), 0, BASE))
            .chain(deferred.iter().map(|key| (key.clone(), 1, BASE)))
            .collect(),
        20,
    );
    let harness = start_with(
        BASE,
        walked.iter().map(|key| (key.clone(), BASE)).collect(),
        source.clone(),
    );
    // Without yielding: the purge and the deferred facts arrive together.
    harness.push(broad(RemovalReason::Purged, 10));
    for chunk in indexed_chunks(&deferred, BASE, 5) {
        harness.push(chunk);
    }
    wait_until(|| live(&harness.events).len() == 1200).await;
    let requests = source.requests();
    assert!(requests.len() >= 6, "{} pages", requests.len());
    for (index, request) in requests.iter().enumerate() {
        assert!(request.len() <= RECONCILE_PAGE);
        let class = if index % 2 == 0 { "p" } else { "w" };
        assert!(
            request.iter().all(|key| key.session_id.starts_with(class)),
            "page {index} belongs to class {class}: {request:?}"
        );
    }
    let mut admissions = requests
        .iter()
        .step_by(2)
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    admissions.sort();
    assert_eq!(admissions, deferred, "every deferred key was paged once");
    let mut walk = requests
        .iter()
        .skip(1)
        .step_by(2)
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    walk.sort();
    assert_eq!(walk, walked, "every predating entry was checked once");
}

#[tokio::test(start_paused = true)]
async fn a_full_pending_set_holds_the_inbox_until_a_page_frees_room_and_loses_no_touch() {
    let deferred = keys_named("p", ADMISSION_PENDING_CAP);
    let source = Arc::new(Scripted::default());
    source.set_rows(
        deferred
            .iter()
            .map(|key| (key.clone(), 1, BASE - 10))
            .chain(std::iter::once((key("unique"), 1, BASE - 10)))
            .collect(),
        100,
    );
    let harness = start_with(BASE, Vec::new(), source.clone());
    let events = &harness.events;
    events.report_async(broad(RemovalReason::Purged, 100)).await;

    // Every page is held: nothing can free room in the pending set.
    source.hold();
    for chunk in indexed_chunks(&deferred, BASE, 50) {
        harness.push(chunk);
    }
    harness.push(touched("unique", 1, BASE + 7, 50));
    wait_until(|| {
        harness.with_registry(|registry| registry.pending.len() == ADMISSION_PENDING_CAP)
    })
    .await;
    settle().await;
    harness.with_registry(|registry| {
        assert!(
            !registry.pending.contains_key(&key("unique")),
            "blocked, not pending"
        );
        assert!(!registry.live.contains_key(&key("unique")));
    });

    // The inbox fills and is held: the async reporter would wait here.
    for index in 0..INBOX_CAPACITY {
        harness.push(touched(&format!("fill{index}"), 1, BASE, 100));
    }
    assert!(
        matches!(
            events.inbox.try_send(touched("one-more", 1, BASE, 100)),
            Err(mpsc::error::TrySendError::Full(_))
        ),
        "the inbox is held while the carry is blocked"
    );
    settle().await;
    assert!(
        live(events).is_empty(),
        "nothing moves while the page is held"
    );
    let rounds_before = harness.rounds();
    settle().await;
    assert_eq!(
        harness.rounds(),
        rounds_before,
        "a blocked carry with a page in flight does not spin"
    );

    // The first page frees 256 slots: the blocked touch is deferred, the
    // held inbox drains, and every page admits its keys.
    source.release();
    wait_until(|| live(events).len() == ADMISSION_PENDING_CAP + 1 + INBOX_CAPACITY).await;
    let unique = live(events)
        .into_iter()
        .find(|session| session.session.session_id == "unique")
        .expect("the blocked touch was admitted");
    assert_eq!(
        unique.last_activity_at,
        BASE + 7,
        "the transient watcher time survived the wait and the page"
    );
    harness.with_registry(|registry| assert!(registry.pending.is_empty()));
}

#[tokio::test(start_paused = true)]
async fn a_blocked_carry_does_not_busy_loop() {
    let deferred = keys_named("p", ADMISSION_PENDING_CAP);
    let source = Arc::new(Scripted::default());
    source.fail_next(100);
    let harness = start_with(BASE, Vec::new(), source.clone());
    harness
        .events
        .report_async(broad(RemovalReason::Purged, 100))
        .await;
    for chunk in indexed_chunks(&deferred, BASE, 50) {
        harness.push(chunk);
    }
    harness.push(touched("blocked", 1, BASE, 50));
    // The first page fails before the actor necessarily drains all admission facts.
    wait_for_processed_pages(&harness.events, 1).await;
    wait_until(|| {
        harness.with_registry(|registry| registry.pending.len() == ADMISSION_PENDING_CAP)
    })
    .await;
    harness.with_registry(|registry| {
        assert_eq!(registry.pending.len(), ADMISSION_PENDING_CAP);
        assert!(!registry.pending.contains_key(&key("blocked")));
    });

    // Nothing is ready: no round runs however often the test yields.
    let parked = harness.rounds();
    for _ in 0..20 {
        settle().await;
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        harness.rounds(),
        parked,
        "the actor parks until the backoff timer"
    );
    assert_eq!(source.requests().len(), 1);

    // The backoff timer is the one ready arm; it issues the retry.
    tokio::time::sleep(RECONCILE_BACKOFF_MIN + Duration::from_secs(1)).await;
    wait_until(|| source.requests().len() == 2).await;
    wait_for_processed_pages(&harness.events, 2).await;
    assert!(
        harness.rounds() <= parked + 2,
        "one round for the timer, one for the failed page"
    );
}

#[test]
fn the_seed_walks_pages_and_publishes_nothing() {
    let source = Scripted::default();
    let first = (0..RECONCILE_PAGE)
        .map(|index| presence(&format!("s{index:04}"), 1, BASE - (index % 100) as i64))
        .collect::<Vec<_>>();
    let second = vec![
        presence("late", 2, BASE - 400),
        presence("older", 1, BASE - 100),
        Presence {
            key: SessionKey::new("native", "not-an-agent", "unknown"),
            incarnation: Incarnation(1),
            epoch: BASE,
        },
    ];
    let cursor = ActiveCursor::after(&first[RECONCILE_PAGE - 1]);
    source
        .active_pages
        .lock()
        .unwrap()
        .extend([Ok((first, Revision(5))), Ok((second, Revision(6)))]);
    let events = SessionEvents::default();
    let mut bus = events.subscribe();
    seed(&events, &source, BASE);

    assert_eq!(
        *source.active_requests.lock().unwrap(),
        vec![
            (BASE - ACTIVE_SESSION_WINDOW_SECS, None, RECONCILE_PAGE),
            (
                BASE - ACTIVE_SESSION_WINDOW_SECS,
                Some(cursor),
                RECONCILE_PAGE
            ),
        ]
    );
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    assert_eq!(events.current_seq(), 0);
    let registry = events.registry.lock().unwrap();
    assert_eq!(
        registry.live.len(),
        RECONCILE_PAGE + 1,
        "the window and the agent filter apply"
    );
    assert_eq!(registry.live[&key("s0000")].exists_at, Revision(5));
    assert_eq!(registry.live[&key("s0000")].incarnation, Incarnation(1));
    assert_eq!(registry.live[&key("older")].exists_at, Revision(6));
    assert!(registry.live[&key("older")].quiet_published);
    assert!(!registry.live.contains_key(&key("late")));

    // A failing page ends the walk with the rows read so far.
    let source = Scripted::default();
    let full = (0..RECONCILE_PAGE)
        .map(|index| presence(&format!("s{index:04}"), 1, BASE))
        .collect::<Vec<_>>();
    source.active_pages.lock().unwrap().extend([
        Ok((full, Revision(5))),
        Err(anyhow::anyhow!("scripted seed failure")),
    ]);
    let events = SessionEvents::default();
    seed(&events, &source, BASE);
    assert_eq!(source.active_requests.lock().unwrap().len(), 2);
    assert_eq!(events.registry.lock().unwrap().live.len(), RECONCILE_PAGE);
}

/// S11: the launch pass re-reports seeded rows with equal incarnations and
/// revisions at or above the seed pages'.
#[tokio::test(start_paused = true)]
async fn the_launch_pass_advances_seeded_entries_without_duplicate_starts() {
    let source = Arc::new(Scripted::default());
    source.active_pages.lock().unwrap().push_back(Ok((
        vec![
            presence("seeded", 3, BASE - 5),
            presence("other", 1, BASE - 5),
        ],
        Revision(9),
    )));
    let events = Arc::new(SessionEvents::default());
    seed(&events, source.as_ref(), BASE);
    let inbox = events.claim_actor().expect("a fresh bus has no actor yet");
    let mut bus = events.subscribe();
    let actor_events = events.clone();
    let actor_source: Arc<dyn ReconcileSource> = source.clone();
    let clock = instant_clock(BASE, Arc::new(std::sync::atomic::AtomicI64::new(0)));
    tokio::spawn(async move {
        run(&actor_events, inbox, actor_source, &clock).await;
    });

    // The launch pass sees the same rows: nothing to say, even with the
    // identity-level new flag set.
    events
        .report_async(Observation::Indexed {
            sessions: vec![
                indexed("seeded", 3, BASE - 5, true),
                indexed("other", 1, BASE - 5, true),
            ],
            revision: Revision(9),
        })
        .await;
    settle().await;
    assert_eq!(drain(&mut bus), Vec::new());

    // A later write is activity, not a start; the deletion ends it.
    events
        .report_async(index("seeded", 3, BASE + 2, false, 11))
        .await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: Some(session_ref("seeded")),
            agent: AgentKind::Claude,
            at: BASE + 2,
            resumed: false,
        }
    );
    events
        .report_async(removed("seeded", 3, RemovalReason::Deleted, 12))
        .await;
    assert!(matches!(next(&mut bus).await, SessionEvent::Idle { .. }));
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Removed {
            session: Some(session_ref("seeded")),
            reason: RemovalReason::Deleted,
        }
    );
    assert_eq!(live(&events).len(), 1);
    // An older incarnation of a seeded key is history.
    events
        .report_async(index("other", 0, BASE + 3, true, 13))
        .await;
    settle().await;
    assert_eq!(drain(&mut bus), Vec::new());
    assert_eq!(live(&events)[0].last_activity_at, BASE - 5);
}

#[tokio::test(start_paused = true)]
async fn pruning_capacity_retries_the_blocked_carry_after_page_backoff() {
    let deferred = keys_named("p", ADMISSION_PENDING_CAP);
    let source = Arc::new(Scripted::default());
    source.fail_next(1);
    source.set_rows(vec![(key("unique"), 1, BASE - 10)], 100);
    let harness = start_with(BASE, Vec::new(), source.clone());
    harness.push(broad(RemovalReason::Purged, 100));
    for chunk in indexed_chunks(&deferred, BASE - 179, 50) {
        harness.push(chunk);
    }
    harness.push(touched("unique", 1, BASE, 50));
    wait_for_processed_pages(&harness.events, 1).await;
    wait_until(|| {
        harness.with_registry(|registry| registry.pending.len() == ADMISSION_PENDING_CAP)
    })
    .await;
    assert_eq!(
        harness.with_registry(|r| r.pending.len()),
        ADMISSION_PENDING_CAP
    );
    let parked = harness.rounds();
    settle().await;
    assert_eq!(harness.rounds(), parked);
    tokio::time::advance(Duration::from_secs(3)).await;
    wait_until(|| harness.with_registry(|r| r.live.contains_key(&key("unique")))).await;
    harness.with_registry(|r| {
        assert!(r.pending.is_empty());
        assert_eq!(r.live.len(), 1);
        assert_eq!(r.live[&key("unique")].last_activity_at, BASE);
    });
    assert_eq!(source.requests().len(), 2);
    harness.push(touched("next", 1, BASE + 3, 100));
    wait_until(|| harness.with_registry(|r| r.live.contains_key(&key("next")))).await;
}

fn failed_seed_source() -> Arc<Scripted> {
    let source = Arc::new(Scripted::default());
    source.active_pages.lock().unwrap().extend([
        Ok((
            (0..RECONCILE_PAGE)
                .map(|index| presence(&format!("s{index:04}"), 1, BASE))
                .collect(),
            Revision(5),
        )),
        Err(anyhow::anyhow!("seed page failed")),
    ]);
    source
}

#[tokio::test(start_paused = true)]
async fn failed_seed_recovers_to_existing_subscribers_without_seed_narration() {
    let source = failed_seed_source();
    source
        .active_pages
        .lock()
        .unwrap()
        .push_back(Ok((vec![presence("omitted", 1, BASE)], Revision(5))));
    let events = Arc::new(SessionEvents::default());
    let mut bus = events.subscribe();
    let actor = start_seeded(events.clone(), source.clone(), BASE);
    assert_eq!(events.current_seq(), 0);
    assert_eq!(live(&events).len(), 256);
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    tokio::task::yield_now().await;
    tokio::time::advance(RECONCILE_BACKOFF_MIN).await;
    wait_until(|| live(&events).len() == 257).await;
    let event = bus.try_recv().unwrap();
    assert_eq!(event.seq, 1);
    assert_eq!(
        event.event,
        SessionEvent::Activity {
            session: Some(session_ref("omitted")),
            agent: AgentKind::Claude,
            at: BASE,
            resumed: false
        }
    );
    assert_eq!(event.aggregate.unwrap().working, 257);
    assert_eq!(events.current_seq(), 1);
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    let requests = source.active_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[1], requests[2],
        "the cutoff and failed cursor survive startup"
    );
    assert!(events.registry.lock().unwrap().reconcile.recovery.is_none());
    actor.abort();
    assert!(actor.await.unwrap_err().is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn seed_recovery_failures_back_off_while_facts_spill_and_expiry_progress() {
    let source = failed_seed_source();
    source
        .active_pages
        .lock()
        .unwrap()
        .extend((0..8).map(|_| Err(anyhow::anyhow!("recovery read failed"))));
    source.active_gated.store(true, Ordering::Relaxed);
    let events = Arc::new(SessionEvents::default());
    let mut bus = events.subscribe();
    let actor = start_seeded(events.clone(), source.clone(), BASE);
    settle().await;
    let mut elapsed = 0;
    for (index, delay) in [2, 4, 8, 16, 30, 30].into_iter().enumerate() {
        let before = source.active_requests.lock().unwrap().len();
        tokio::time::advance(Duration::from_secs(delay - 1)).await;
        settle().await;
        assert_eq!(source.active_requests.lock().unwrap().len(), before);
        events
            .report_async(anonymous(
                AgentKind::Claude,
                BASE + elapsed + delay as i64 - 1,
                index as u64 + 1,
            ))
            .await;
        wait_until(|| {
            events
                .registry
                .lock()
                .unwrap()
                .anonymous
                .get(&AgentKind::Claude)
                .is_some_and(|entry| entry.generation == AnonymousGen(index as u64 + 1))
        })
        .await;
        events
            .spill
            .lock()
            .unwrap()
            .fold(sync_removed("spilled", 1, RemovalReason::Deleted, 10));
        events.spill_wake.notify_one();
        wait_until(|| {
            events
                .registry
                .lock()
                .unwrap()
                .deleted
                .contains_key(&key("spilled"))
        })
        .await;
        tokio::time::advance(Duration::from_secs(1)).await;
        wait_until(|| source.active_requests.lock().unwrap().len() == before + 1).await;
        wait_for_processed_pages(&events, index + 1).await;
        elapsed += delay as i64;
        let rounds = events.rounds.load(Ordering::Relaxed);
        settle().await;
        assert_eq!(
            events.rounds.load(Ordering::Relaxed),
            rounds,
            "backoff does not spin"
        );
    }
    let narrated = drain(&mut bus);
    assert!(
        narrated
            .iter()
            .any(|event| matches!(event, SessionEvent::Quiet { .. }))
    );
    assert!(
        !narrated
            .iter()
            .any(|event| matches!(event, SessionEvent::Started { .. }))
    );
    assert_eq!(events.registry.lock().unwrap().working, 0);
    let requests = source.active_requests.lock().unwrap().len();
    actor.abort();
    assert!(actor.await.unwrap_err().is_cancelled());
    tokio::time::advance(Duration::from_secs(60)).await;
    settle().await;
    assert_eq!(source.active_requests.lock().unwrap().len(), requests);
}

#[tokio::test(start_paused = true)]
async fn delayed_recovery_page_cannot_import_deleted_incarnation_activity() {
    let source = failed_seed_source();
    source.active_pages.lock().unwrap().push_back(Ok((
        vec![
            presence("recreated", 1, BASE + 10),
            presence("deleted", 1, BASE),
        ],
        Revision(5),
    )));
    let events = Arc::new(SessionEvents::default());
    let actor = start_seeded(events.clone(), source.clone(), BASE);
    source.active_gated.store(true, Ordering::Relaxed);
    source.hold();
    settle().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    wait_until(|| source.active_requests.lock().unwrap().len() == 3).await;
    events
        .report_async(removed("recreated", 1, RemovalReason::Deleted, 6))
        .await;
    events
        .report_async(index("recreated", 2, BASE - 10, true, 7))
        .await;
    events
        .report_async(removed("deleted", 1, RemovalReason::Deleted, 8))
        .await;
    wait_until(|| {
        events
            .registry
            .lock()
            .unwrap()
            .live
            .contains_key(&key("recreated"))
    })
    .await;
    let mut bus = events.subscribe();
    source.release();
    wait_until(|| events.registry.lock().unwrap().reconcile.recovery.is_none()).await;
    {
        let registry = events.registry.lock().unwrap();
        assert_eq!(registry.live[&key("recreated")].incarnation, Incarnation(2));
        assert_eq!(registry.live[&key("recreated")].last_activity_at, BASE - 10);
        assert!(!registry.live.contains_key(&key("deleted")));
    }
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    actor.abort();
    assert!(actor.await.unwrap_err().is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn shutdown_discards_a_held_recovery_page() {
    let source = failed_seed_source();
    source
        .active_pages
        .lock()
        .unwrap()
        .push_back(Ok((vec![presence("omitted", 1, BASE)], Revision(5))));
    let events = Arc::new(SessionEvents::default());
    let actor = start_seeded(events.clone(), source.clone(), BASE);
    source.active_gated.store(true, Ordering::Relaxed);
    source.hold();
    settle().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    wait_until(|| source.active_requests.lock().unwrap().len() == 3).await;
    actor.abort();
    assert!(actor.await.unwrap_err().is_cancelled());
    source.release();
    events.test_probe.wait_for_finished_task();
    assert_eq!(events.test_probe.processed(), 0);
    assert_eq!(live(&events).len(), 256);
    assert_eq!(events.current_seq(), 0);
    assert!(matches!(
        events.inbox.try_send(touched("closed", 1, BASE, 10)),
        Err(mpsc::error::TrySendError::Closed(_))
    ));
}

#[test]
fn recovery_retains_a_blocked_page_and_shares_turns_with_admission_and_walk() {
    let events = SessionEvents::default();
    let source = failed_seed_source();
    seed(&events, source.as_ref(), BASE);
    let mut registry = events.registry.lock().unwrap();
    let cursor = registry.reconcile.recovery.as_ref().unwrap().cursor.clone();
    registry.broad(RemovalReason::Purged, Revision(10), &mut Vec::new());
    for target in keys_named("p", ADMISSION_PENDING_CAP) {
        assert_eq!(
            registry.establish(
                &target,
                Existence {
                    agent: AgentKind::Claude,
                    incarnation: Incarnation(1),
                    at: BASE - 179,
                    revision: Revision(5),
                    is_new: false
                },
                BASE,
                &mut Vec::new()
            ),
            Touch::Deferred
        );
    }
    assert!(matches!(
        registry.next_page_request(BASE),
        Some(PageRequest::Admission { .. })
    ));
    assert!(matches!(
        registry.next_page_request(BASE),
        Some(PageRequest::Walk { .. })
    ));
    let request = registry.next_page_request(BASE).unwrap();
    assert!(matches!(request, PageRequest::Active { .. }));
    let mut out = Vec::new();
    registry.apply_page(
        &request,
        vec![
            presence("omitted", 1, BASE),
            presence("expired", 1, BASE - 179),
        ],
        Revision(5),
        BASE,
        &mut out,
    );
    let recovery = registry.reconcile.recovery.as_ref().unwrap();
    assert_eq!(
        recovery.cursor, cursor,
        "acceptance, not completion, advances the cursor"
    );
    assert_eq!(recovery.page.as_ref().unwrap().rows.len(), 2);
    assert!(out.is_empty());
    // Page selection prunes expired admissions and releases the retained suffix.
    assert!(
        registry.next_page_request(BASE + 2).is_none(),
        "available recovery capacity reserves the next page-service round before another read"
    );
    registry.apply_recovery(BASE + 2, &mut out);
    assert!(registry.reconcile.recovery.is_none());
    assert!(registry.pending.contains_key(&key("omitted")));
    assert!(!registry.pending.contains_key(&key("expired")));
    assert!(!registry.live.contains_key(&key("expired")));
    let admission = registry.next_admission_page().unwrap();
    registry.apply_page(
        &admission,
        vec![presence("omitted", 2, BASE - 20)],
        Revision(11),
        BASE + 2,
        &mut out,
    );
    assert_eq!(registry.live[&key("omitted")].incarnation, Incarnation(2));
    assert_eq!(registry.live[&key("omitted")].last_activity_at, BASE - 20);
    assert!(
        !out.iter()
            .any(|event| matches!(event, SessionEvent::Started { .. }))
    );
}

#[tokio::test(start_paused = true)]
async fn recovery_backpressure_keeps_one_page_task_and_releases_the_inbox() {
    let source = failed_seed_source();
    source
        .active_pages
        .lock()
        .unwrap()
        .push_back(Ok((vec![presence("omitted", 1, BASE)], Revision(5))));
    let events = Arc::new(SessionEvents::default());
    let actor = start_seeded(events.clone(), source.clone(), BASE);
    source.active_gated.store(true, Ordering::Relaxed);
    source.hold();
    settle().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    wait_until(|| source.active_requests.lock().unwrap().len() == 3).await;
    events.report_async(broad(RemovalReason::Purged, 10)).await;
    let deferred = keys_named("p", ADMISSION_PENDING_CAP);
    source.set_rows(
        deferred
            .iter()
            .map(|key| (key.clone(), 1, BASE))
            .chain([(key("omitted"), 1, BASE), (key("blocked"), 1, BASE)])
            .collect(),
        10,
    );
    for chunk in indexed_chunks(&deferred, BASE, 5) {
        events.report_async(chunk).await;
    }
    events
        .report_async(touched("blocked", 1, BASE + 1, 5))
        .await;
    wait_until(|| events.registry.lock().unwrap().pending.len() == ADMISSION_PENDING_CAP).await;
    settle().await;
    assert!(
        source.requests().is_empty(),
        "the held active read excludes a second page task"
    );
    source.allow(1);
    wait_until(|| !source.requests().is_empty()).await;
    {
        let registry = events.registry.lock().unwrap();
        assert_eq!(
            registry
                .reconcile
                .recovery
                .as_ref()
                .unwrap()
                .page
                .as_ref()
                .unwrap()
                .rows
                .len(),
            1
        );
        assert!(!registry.live.contains_key(&key("omitted")));
    }
    let parked = events.rounds.load(Ordering::Relaxed);
    settle().await;
    assert_eq!(events.rounds.load(Ordering::Relaxed), parked);
    assert_eq!(source.requests().len(), 1);
    source.release();
    wait_until(|| {
        let registry = events.registry.lock().unwrap();
        registry.live.contains_key(&key("omitted"))
            && registry.live.contains_key(&key("blocked"))
            && registry.reconcile.recovery.is_none()
    })
    .await;
    assert_eq!(
        events.registry.lock().unwrap().live[&key("blocked")].last_activity_at,
        BASE + 1
    );
    events.report_async(touched("after", 1, BASE + 2, 10)).await;
    wait_until(|| {
        events
            .registry
            .lock()
            .unwrap()
            .live
            .contains_key(&key("after"))
    })
    .await;
    actor.abort();
    assert!(actor.await.unwrap_err().is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn full_recovery_page_advances_the_cursor_before_a_later_failure_and_retry() {
    let source = failed_seed_source();
    let recovered: Vec<_> = (0..RECONCILE_PAGE)
        .rev()
        .map(|index| presence(&format!("r{index:04}"), 1, BASE))
        .collect();
    let cursor = ActiveCursor::after(recovered.last().unwrap());
    source.active_pages.lock().unwrap().extend([
        Ok((recovered, Revision(5))),
        Err(anyhow::anyhow!("later recovery page failed")),
        Ok((vec![presence("last", 1, BASE)], Revision(5))),
    ]);
    source.active_gated.store(true, Ordering::Relaxed);
    let events = Arc::new(SessionEvents::default());
    let mut bus = events.subscribe();
    let actor = start_seeded(events.clone(), source.clone(), BASE);
    settle().await;
    assert_eq!(events.current_seq(), 0);
    tokio::time::advance(RECONCILE_BACKOFF_MIN).await;
    wait_until(|| source.active_requests.lock().unwrap().len() == 4).await;
    wait_for_processed_pages(&events, 2).await;
    assert_eq!(live(&events).len(), RECONCILE_PAGE * 2);
    assert_eq!(events.current_seq(), RECONCILE_PAGE as u64);
    assert_eq!(
        events
            .registry
            .lock()
            .unwrap()
            .reconcile
            .recovery
            .as_ref()
            .unwrap()
            .cursor,
        Some(cursor.clone())
    );
    let narrated: Vec<_> = std::iter::from_fn(|| bus.try_recv().ok()).collect();
    assert_eq!(narrated.len(), RECONCILE_PAGE);
    for (index, event) in narrated.iter().enumerate() {
        assert_eq!(event.seq, index as u64 + 1);
        assert!(
            matches!(&event.event, SessionEvent::Activity { session: Some(session), at: BASE, resumed: false, .. } if session.session_id.starts_with('r'))
        );
    }
    assert_eq!(
        narrated.last().unwrap().aggregate.as_ref().unwrap().total,
        RECONCILE_PAGE * 2
    );
    tokio::time::advance(RECONCILE_BACKOFF_MIN - Duration::from_millis(1)).await;
    settle().await;
    assert_eq!(source.active_requests.lock().unwrap().len(), 4);
    tokio::time::advance(Duration::from_millis(1)).await;
    wait_until(|| live(&events).len() == RECONCILE_PAGE * 2 + 1).await;
    let requests = source.active_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 5);
    assert_eq!(requests[1], requests[2]);
    assert_eq!(requests[3], requests[4]);
    assert_eq!(
        requests[3],
        (
            BASE - ACTIVE_SESSION_WINDOW_SECS,
            Some(cursor),
            RECONCILE_PAGE
        )
    );
    let last = bus.try_recv().unwrap();
    assert_eq!(last.seq, RECONCILE_PAGE as u64 + 1);
    assert!(
        matches!(last.event, SessionEvent::Activity { session: Some(session), .. } if session == session_ref("last"))
    );
    assert_eq!(last.aggregate.unwrap().total, RECONCILE_PAGE * 2 + 1);
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    assert!(events.registry.lock().unwrap().reconcile.recovery.is_none());
    actor.abort();
    assert!(actor.await.unwrap_err().is_cancelled());
}
