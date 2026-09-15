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
    source.spin_until_completed(3);
    settle().await;
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
    // The first page fails and the actor backs off.
    wait_until(|| source.requests().len() == 1).await;
    source.spin_until_completed(1);
    settle().await;
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
    source.spin_until_completed(2);
    settle().await;
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
