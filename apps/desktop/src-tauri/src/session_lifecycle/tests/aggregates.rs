//! Exact counts, their place on the bus, and named presence beyond the
//! snapshot's bounded rows.

use super::*;

/// Every event the bus holds right now, with its sequence and aggregate.
fn drain_sequenced(bus: &mut broadcast::Receiver<Sequenced>) -> Vec<Sequenced> {
    let mut out = Vec::new();
    loop {
        match bus.try_recv() {
            Ok(sequenced) => out.push(sequenced),
            Err(TryRecvError::Empty) => return out,
            Err(other) => panic!("unexpected bus state: {other:?}"),
        }
    }
}

fn aggregate(working: usize, total: usize, anonymous: usize) -> Aggregate {
    Aggregate {
        working,
        total,
        anonymous,
    }
}

/// An `Indexed` chunk of `count` new sessions named `prefix-i`, all at
/// `at`, committed at `revision`.
fn index_many(prefix: &str, count: usize, at: i64, revision: u64) -> Observation {
    Observation::Indexed {
        sessions: (0..count)
            .map(|i| indexed(&format!("{prefix}-{i}"), 1, at, true))
            .collect(),
        revision: Revision(revision),
    }
}

#[tokio::test(start_paused = true)]
async fn exact_counts_follow_every_transition_independent_of_the_row_limit() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    // Two hundred sessions, well past the default row limit.
    events.report_async(index_many("s", 200, BASE, 1)).await;
    settle().await;
    let snapshot = events.snapshot(DEFAULT_SNAPSHOT_LIMIT);
    assert_eq!(snapshot.sessions.len(), DEFAULT_SNAPSHOT_LIMIT);
    assert_eq!((snapshot.working, snapshot.total), (200, 200));
    let last = drain_sequenced(&mut bus).pop().unwrap();
    assert_eq!(last.aggregate, Some(aggregate(200, 200, 0)));
    assert_eq!(last.seq, snapshot.seq);

    // Anonymous activity counts as working without a row.
    events
        .report_async(anonymous(AgentKind::Codex, BASE, 1))
        .await;
    settle().await;
    assert_eq!(
        drain_sequenced(&mut bus).pop().unwrap().aggregate,
        Some(aggregate(200, 200, 1))
    );
    let snapshot = events.snapshot(0);
    assert!(snapshot.sessions.is_empty());
    assert_eq!((snapshot.working, snapshot.total), (200, 200));
    assert_eq!(snapshot.anonymous.len(), 1);

    // Ten sessions write again, later than the rest, so they alone stay
    // working when the quiet window passes for the others.
    for i in 0..10 {
        events
            .report_async(touched(&format!("s-{i}"), 1, BASE + 20, 2))
            .await;
    }
    settle().await;
    drain_sequenced(&mut bus);
    tokio::time::sleep(Duration::from_secs(31)).await;
    settle().await;
    let quiets = drain_sequenced(&mut bus);
    assert_eq!(quiets.len(), 191, "190 quiets and one anonymous expiry");
    assert_eq!(
        quiets.last().unwrap().aggregate,
        Some(aggregate(10, 200, 0))
    );
    let snapshot = events.snapshot(DEFAULT_SNAPSHOT_LIMIT);
    assert_eq!((snapshot.working, snapshot.total), (10, 200));
    assert_eq!(
        snapshot.sessions.iter().filter(|row| !row.quiet).count(),
        10,
        "the ten working rows are the most recent, so the bounded rows hold them"
    );

    // A resume moves one back to working.
    events.report_async(touched("s-100", 1, BASE + 40, 3)).await;
    settle().await;
    let resumed = drain_sequenced(&mut bus).pop().unwrap();
    assert!(matches!(
        resumed.event,
        SessionEvent::Activity { resumed: true, .. }
    ));
    assert_eq!(resumed.aggregate, Some(aggregate(11, 200, 0)));

    // By the idle window the eleven later writers have gone quiet too, and
    // the 189 sessions that never wrote again leave the registry.
    tokio::time::sleep(Duration::from_secs(150)).await;
    settle().await;
    let batch = drain_sequenced(&mut bus);
    assert_eq!(
        batch
            .iter()
            .filter(|event| matches!(event.event, SessionEvent::Quiet { .. }))
            .count(),
        11
    );
    assert_eq!(
        batch
            .iter()
            .filter(|event| matches!(event.event, SessionEvent::Idle { .. }))
            .count(),
        189
    );
    assert_eq!(batch.last().unwrap().aggregate, Some(aggregate(0, 11, 0)));
    let snapshot = events.snapshot(DEFAULT_SNAPSHOT_LIMIT);
    assert_eq!((snapshot.working, snapshot.total), (0, 11));
    assert_eq!(snapshot.sessions.len(), 11);
    assert!(snapshot.sessions.iter().all(|row| row.quiet));
}

#[tokio::test(start_paused = true)]
async fn the_aggregate_rides_on_the_last_lifecycle_event_of_a_batch_not_a_filtered_tail() {
    let harness = start(BASE, vec![(key("doomed"), BASE), (key("other"), BASE)]);
    let events = &harness.events;
    let mut bus = events.subscribe();

    // A keyed removal of a live entry is one batch: `Idle` then `Removed`.
    // `Removed` reaches readers as an index change, so the counts ride on
    // `Idle`, the last event the lifecycle scope relays.
    events.report(sync_removed("doomed", 0, RemovalReason::Deleted, 5));
    settle().await;
    let batch = drain_sequenced(&mut bus);
    assert_eq!(batch.len(), 2);
    assert!(matches!(batch[0].event, SessionEvent::Idle { .. }));
    assert_eq!(batch[0].aggregate, Some(aggregate(1, 1, 0)));
    assert!(matches!(batch[1].event, SessionEvent::Removed { .. }));
    assert_eq!(batch[1].aggregate, None);

    // A batch with no lifecycle event carries no counts: nothing changed.
    events.report(SyncObservation::RowChanged {
        session: key("other"),
        facets: UpdateFacets {
            title: true,
            ..UpdateFacets::default()
        },
        at: BASE + 1,
    });
    events.report(SyncObservation::IndexChanged {
        reason: IndexChangeReason::Invalidated,
    });
    settle().await;
    let batch = drain_sequenced(&mut bus);
    assert_eq!(batch.len(), 2);
    assert!(batch.iter().all(|event| event.aggregate.is_none()));

    // A higher incarnation replaces a live entry in one batch: `Idle`,
    // `Removed { Reconciled }`, then `Started`. Only `Started` carries the
    // counts, and they are the batch's final counts.
    events
        .report_async(index("other", 1, BASE + 2, true, 6))
        .await;
    settle().await;
    let batch = drain_sequenced(&mut bus);
    assert_eq!(batch.len(), 3);
    assert!(matches!(batch[0].event, SessionEvent::Idle { .. }));
    assert!(matches!(
        batch[1].event,
        SessionEvent::Removed {
            reason: RemovalReason::Reconciled,
            ..
        }
    ));
    assert!(matches!(batch[2].event, SessionEvent::Started { .. }));
    assert_eq!(batch[0].aggregate, None);
    assert_eq!(batch[1].aggregate, None);
    assert_eq!(batch[2].aggregate, Some(aggregate(1, 1, 0)));

    // One `Indexed` chunk is one batch: the counts ride on its last start,
    // and every earlier event in the batch carries none.
    events
        .report_async(index_many("chunk", 3, BASE + 3, 7))
        .await;
    settle().await;
    let batch = drain_sequenced(&mut bus);
    assert_eq!(batch.len(), 3);
    assert_eq!(batch[0].aggregate, None);
    assert_eq!(batch[1].aggregate, None);
    assert_eq!(batch[2].aggregate, Some(aggregate(4, 4, 0)));
    assert_eq!(events.snapshot(usize::MAX).seq, batch[2].seq);
}

#[tokio::test(start_paused = true)]
async fn a_removal_that_admits_nothing_afterwards_still_stamps_its_idle() {
    // The replacement fact is deferred by a guard, so the batch is `Idle`,
    // `Removed` with nothing narrated after them. The counts ride on `Idle`.
    let source = Arc::new(Scripted::default());
    // No page returns during this test, so the pending entry stays pending.
    source.hold();
    let harness = start_with(BASE, vec![(key("k"), BASE)], Arc::clone(&source));
    let events = &harness.events;
    let mut bus = events.subscribe();
    events.report(SyncObservation::Removed {
        scope: RemovalScope::Broad,
        reason: RemovalReason::Purged,
        revision: Revision(50),
    });
    settle().await;
    drain_sequenced(&mut bus);

    // Incarnation 1 at revision 10 is below the broad guard: it replaces
    // the live incarnation 0, then waits for a page.
    events.report_async(index("k", 1, BASE + 1, true, 10)).await;
    settle().await;
    let batch = drain_sequenced(&mut bus);
    assert_eq!(batch.len(), 2);
    assert!(matches!(batch[0].event, SessionEvent::Idle { .. }));
    assert_eq!(batch[0].aggregate, Some(aggregate(0, 0, 0)));
    assert_eq!(batch[1].aggregate, None);
    assert_eq!(events.snapshot(usize::MAX).total, 0);
    harness.with_registry(|registry| assert!(registry.pending.contains_key(&key("k"))));
    source.release();
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_reads_a_completed_batch_never_a_partial_one() {
    // Every batch below starts exactly `INDEXED_CHUNK` sessions, so a
    // snapshot taken between batches sees a total that is a multiple of
    // the chunk and a sequence equal to that total. A reader racing the
    // actor on another thread must never see anything else.
    let harness = start(BASE, Vec::new());
    let events = Arc::clone(&harness.events);
    let reader_events = Arc::clone(&events);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader_stop = Arc::clone(&stop);
    let reader = std::thread::spawn(move || {
        let mut observed = Vec::new();
        while !reader_stop.load(Ordering::Relaxed) {
            let snapshot = reader_events.snapshot(1);
            observed.push((snapshot.seq, snapshot.total, snapshot.working));
        }
        observed
    });

    for batch in 0..8 {
        events
            .report_async(index_many(
                &format!("b{batch}"),
                INDEXED_CHUNK,
                BASE,
                batch as u64 + 1,
            ))
            .await;
        settle().await;
    }
    stop.store(true, Ordering::Relaxed);
    let observed = reader.join().unwrap();
    assert!(!observed.is_empty());
    for (seq, total, working) in observed {
        assert_eq!(
            seq % INDEXED_CHUNK as u64,
            0,
            "a partial batch leaked: seq {seq}"
        );
        assert_eq!(total as u64, seq, "total {total} does not match seq {seq}");
        assert_eq!(working, total);
    }
    assert_eq!(events.snapshot(1).total, 8 * INDEXED_CHUNK);
}

#[tokio::test(start_paused = true)]
async fn presence_answers_named_identities_at_one_sequence_beyond_the_snapshot_limit() {
    let harness = start(BASE, Vec::new());
    let events = &harness.events;
    let mut bus = events.subscribe();

    // 130 live sessions; the two oldest fall outside the 128-row snapshot.
    events
        .report_async(index_many("recent", 128, BASE, 1))
        .await;
    events
        .report_async(Observation::Indexed {
            sessions: vec![
                indexed("old-a", 1, BASE - 20, true),
                indexed("old-b", 1, BASE - 20, true),
            ],
            revision: Revision(2),
        })
        .await;
    settle().await;
    let seq = drain_sequenced(&mut bus).pop().unwrap().seq;
    let snapshot = events.snapshot(DEFAULT_SNAPSHOT_LIMIT);
    assert_eq!(snapshot.total, 130);
    assert_eq!(snapshot.sessions.len(), 128);
    assert!(
        !snapshot
            .sessions
            .iter()
            .any(|row| row.session.session_id.starts_with("old-")),
        "the omitted rows are the oldest"
    );

    // The omitted identities are live, not absent; an unknown one is absent;
    // a duplicate is answered once.
    let presence = events.presence(&[
        session_ref("old-a"),
        session_ref("never-indexed"),
        session_ref("old-b"),
        session_ref("old-a"),
        session_ref("recent-5"),
    ]);
    assert_eq!(presence.seq, seq);
    assert_eq!(
        presence
            .present
            .iter()
            .map(|row| row.session.session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["old-a", "old-b", "recent-5"]
    );
    assert_eq!(presence.absent, vec![session_ref("never-indexed")]);
    assert!(presence.present.iter().all(|row| !row.quiet));

    // Presence mirrors registry state, including quiet, at the sequence
    // the read happened.
    tokio::time::sleep(Duration::from_secs(31)).await;
    settle().await;
    let seq = drain_sequenced(&mut bus).pop().unwrap().seq;
    let presence = events.presence(&[session_ref("old-a")]);
    assert_eq!(presence.seq, seq);
    assert_eq!(presence.present.len(), 1);
    assert!(presence.present[0].quiet);
    assert_eq!(presence.present[0].last_activity_at, BASE - 20);

    // An empty request is answered with the sequence alone.
    let presence = events.presence(&[]);
    assert_eq!(presence.seq, seq);
    assert!(presence.present.is_empty() && presence.absent.is_empty());
}

#[test]
fn counts_and_presence_serialize_in_camel_case() {
    let event = SessionEvent::Quiet {
        session: session_ref("abc"),
        agent: AgentKind::Claude,
        at: 9,
    };
    let stamped = LifecycleEnvelope {
        seq: 5,
        event: &event,
        aggregate: Some(aggregate(2, 3, 1)),
    };
    let json = serde_json::to_value(&stamped).unwrap();
    assert_eq!(json["seq"], 5);
    assert_eq!(json["kind"], "quiet");
    assert_eq!(json["aggregate"]["working"], 2);
    assert_eq!(json["aggregate"]["total"], 3);
    assert_eq!(json["aggregate"]["anonymous"], 1);

    let unstamped = LifecycleEnvelope {
        seq: 6,
        event: &event,
        aggregate: None,
    };
    let json = serde_json::to_value(&unstamped).unwrap();
    assert!(
        json.get("aggregate").is_none(),
        "an absent aggregate is omitted"
    );

    let snapshot = LiveSnapshot {
        seq: 3,
        working: 1,
        total: 2,
        sessions: Vec::new(),
        anonymous: Vec::new(),
    };
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(json["working"], 1);
    assert_eq!(json["total"], 2);

    let presence = LivePresence {
        seq: 4,
        present: vec![LiveSession {
            session: session_ref("here"),
            agent: AgentKind::Claude,
            last_activity_at: 10,
            quiet: true,
        }],
        absent: vec![session_ref("gone")],
    };
    let json = serde_json::to_value(&presence).unwrap();
    assert_eq!(json["seq"], 4);
    assert_eq!(json["present"][0]["session"]["sessionId"], "here");
    assert_eq!(json["present"][0]["lastActivityAt"], 10);
    assert_eq!(json["present"][0]["quiet"], true);
    assert_eq!(json["absent"][0]["sessionId"], "gone");
    assert_eq!(json["absent"][0]["environmentKey"], "native");

    let session_ref: SessionRef =
        serde_json::from_str(r#"{"environmentKey":"native","agent":"codex","sessionId":"x"}"#)
            .unwrap();
    assert_eq!(
        SessionKey::from(&session_ref),
        SessionKey::new("native", "codex", "x")
    );
}

#[test]
fn get_live_sessions_for_is_permitted_wherever_get_live_sessions_is() {
    let default = include_str!("../../../permissions/default.toml");
    let main = include_str!("../../../capabilities/main.json");
    let generated = include_str!("../../../permissions/autogenerated/get_live_sessions_for.toml");
    let registered = include_str!("../../../src/app_commands.rs");

    assert!(registered.contains("commands::get_live_sessions => \"get_live_sessions\""));
    assert!(registered.contains("commands::get_live_sessions_for => \"get_live_sessions_for\""));
    assert!(generated.contains("identifier = \"allow-get-live-sessions-for\""));
    assert!(generated.contains("commands.allow = [\"get_live_sessions_for\"]"));

    // Every window that receives the snapshot permission receives the
    // presence permission by the same route: the default set for the
    // windows `capabilities/default.json` names, and the explicit list for
    // the main window.
    for source in [default, main] {
        assert!(source.contains("\"allow-get-live-sessions\""));
        assert!(source.contains("\"allow-get-live-sessions-for\""));
    }

    // The removed command left no permission behind.
    assert!(!default.contains("get-latest-session-activity"));
    assert!(!registered.contains("get_latest_session_activity"));
    assert!(
        !std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/permissions/autogenerated/get_latest_session_activity.toml"
        ))
        .exists(),
        "the obsolete generated permission file still exists"
    );
}
