use super::*;
use std::sync::Arc;
use tokio::sync::broadcast::error::TryRecvError;

fn key(session_id: &str) -> SessionKey {
    SessionKey::new("native", "claude-code", session_id)
}

fn session_ref(session_id: &str) -> SessionRef {
    SessionRef::from(&key(session_id))
}

fn indexed(session_id: &str, at: i64, is_new: bool) -> IndexedSession {
    IndexedSession {
        key: key(session_id),
        agent: AgentKind::Claude,
        at,
        is_new,
    }
}

/// A clock tied to tokio's own (pausable) instant, not the wall clock: every
/// `tokio::time::sleep` the actor awaits advances it exactly as far as it
/// advances tokio's clock, so a paused test can drive it deterministically.
fn instant_clock(base_epoch: i64) -> impl Fn() -> i64 + Send + Sync + 'static {
    let base_instant = Instant::now();
    move || base_epoch + base_instant.elapsed().as_secs() as i64
}

/// Seed the registry, start the actor on tokio's clock, and return the
/// handle a test reports through and subscribes to.
fn start(base_epoch: i64, seed: Vec<(SessionKey, i64)>) -> Arc<SessionEvents> {
    let events = Arc::new(SessionEvents::default());
    events.seed(seed, base_epoch);
    assert!(events.claim_actor(), "a fresh bus has no actor yet");
    let actor_events = events.clone();
    tokio::spawn(async move {
        let now = instant_clock(base_epoch);
        run(&actor_events, &now).await;
    });
    events
}

/// Let the actor drain its inbox without advancing the clock.
async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

/// The next event on the bus, without its sequence.
async fn next(bus: &mut broadcast::Receiver<Sequenced>) -> SessionEvent {
    bus.recv().await.unwrap().event
}

/// The live sessions from an effectively unbounded snapshot.
fn live(events: &SessionEvents) -> Vec<LiveSession> {
    events.snapshot(usize::MAX).sessions
}

const BASE: i64 = 1_000_000;

#[tokio::test(start_paused = true)]
async fn touched_publishes_activity_with_the_same_key_and_time() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    events.report(Observation::Touched {
        session: Some(key("busy")),
        agent: AgentKind::Claude,
        at: BASE + 3,
    });

    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: Some(session_ref("busy")),
            agent: AgentKind::Claude,
            at: BASE + 3,
            resumed: false,
        }
    );
    assert_eq!(
        live(&events),
        vec![LiveSession {
            session: session_ref("busy"),
            agent: AgentKind::Claude,
            last_activity_at: BASE + 3,
            quiet: false,
        }]
    );
}

#[tokio::test(start_paused = true)]
async fn a_duplicate_touch_publishes_nothing() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    for _ in 0..3 {
        events.report(Observation::Touched {
            session: Some(key("busy")),
            agent: AgentKind::Claude,
            at: BASE + 3,
        });
    }
    settle().await;

    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { .. }
    ));
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // An older epoch for the same session is also nothing new.
    events.report(Observation::Touched {
        session: Some(key("busy")),
        agent: AgentKind::Claude,
        at: BASE + 1,
    });
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
}

#[tokio::test(start_paused = true)]
async fn a_touch_older_than_the_window_is_rejected() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    events.report(Observation::Touched {
        session: Some(key("ancient")),
        agent: AgentKind::Claude,
        at: BASE - ACTIVE_SESSION_WINDOW_SECS,
    });
    settle().await;

    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    assert!(live(&events).is_empty());
}

#[tokio::test(start_paused = true)]
async fn touched_without_a_session_publishes_agent_activity_once_per_epoch() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    events.report(Observation::Touched {
        session: None,
        agent: AgentKind::Codex,
        at: BASE,
    });
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: None,
            agent: AgentKind::Codex,
            at: BASE,
            resumed: false,
        }
    );
    assert!(live(&events).is_empty());

    // The same epoch again says nothing new.
    events.report(Observation::Touched {
        session: None,
        agent: AgentKind::Codex,
        at: BASE,
    });
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // A later keyless write publishes again.
    events.report(Observation::Touched {
        session: None,
        agent: AgentKind::Codex,
        at: BASE + 4,
    });
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { session: None, at, .. } if at == BASE + 4
    ));
}

#[tokio::test(start_paused = true)]
async fn indexing_a_new_session_resolves_keyless_activity() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    events.report(Observation::Touched {
        session: None,
        agent: AgentKind::Claude,
        at: BASE + 2,
    });
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { session: None, .. }
    ));

    // Discovery resolves the key: one Started, no duplicate resume.
    events.report(Observation::Indexed {
        sessions: vec![indexed("resolved", BASE + 2, true)],
    });
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Started {
            session: session_ref("resolved"),
            agent: AgentKind::Claude,
            at: BASE + 2,
        }
    );

    // The keyless state cleared: an equal keyless epoch publishes again.
    events.report(Observation::Touched {
        session: None,
        agent: AgentKind::Claude,
        at: BASE + 2,
    });
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { session: None, .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn a_first_index_publishes_started_once() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    let observation = Observation::Indexed {
        sessions: vec![indexed("fresh", BASE, true)],
    };
    events.report(observation.clone());
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Started {
            session: session_ref("fresh"),
            agent: AgentKind::Claude,
            at: BASE,
        }
    );

    // The same rows again, with no newer epoch: nothing to say.
    events.report(observation);
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // A later epoch for a known session is activity, not another start.
    events.report(Observation::Indexed {
        sessions: vec![indexed("fresh", BASE + 5, false)],
    });
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: Some(session_ref("fresh")),
            agent: AgentKind::Claude,
            at: BASE + 5,
            resumed: false,
        }
    );
}

#[tokio::test(start_paused = true)]
async fn an_index_older_than_the_window_is_history_not_activity() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    events.report(Observation::Indexed {
        sessions: vec![indexed("old", BASE - ACTIVE_SESSION_WINDOW_SECS, true)],
    });
    settle().await;

    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    assert!(live(&events).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_session_goes_idle_at_the_window_and_a_touch_moves_the_deadline() {
    let events = start(BASE, vec![(key("seeded"), BASE)]);
    let mut bus = events.subscribe();

    // 170 s in: quiet since 30 s, still 10 s left, and a touch restarts
    // both windows.
    tokio::time::sleep(Duration::from_secs(170)).await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Quiet {
            session: session_ref("seeded"),
            agent: AgentKind::Claude,
            at: BASE + 31,
        }
    );
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    events.report(Observation::Touched {
        session: Some(key("seeded")),
        agent: AgentKind::Claude,
        at: BASE + 170,
    });
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { resumed: true, .. }
    ));

    // The original deadline (180 s plus slack) passes with nothing to say.
    tokio::time::sleep(Duration::from_secs(15)).await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // The moved deadlines: quiet at 170 + 30 + 1 s of slack, then idle at
    // 170 + 180 + 1.
    tokio::time::sleep(Duration::from_secs(170)).await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Quiet {
            session: session_ref("seeded"),
            agent: AgentKind::Claude,
            at: BASE + 201,
        }
    );
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Idle {
            session: session_ref("seeded"),
            agent: AgentKind::Claude,
            at: BASE + 351,
        }
    );
    assert!(live(&events).is_empty());
}

#[tokio::test(start_paused = true)]
async fn sessions_expire_in_deadline_order() {
    let events = start(
        BASE,
        vec![(key("older"), BASE - 100), (key("newer"), BASE - 10)],
    );
    let mut bus = events.subscribe();

    // The newer session, seeded inside the quiet window, goes quiet at
    // t=20 s plus slack. The older one was seeded quiet and says nothing.
    tokio::time::sleep(Duration::from_secs(22)).await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Quiet {
            session: session_ref("newer"),
            agent: AgentKind::Claude,
            at: BASE + 21,
        }
    );

    // The older session crosses its window at t=80 s, plus slack.
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Idle { session, .. } if session == session_ref("older")
    ));
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // The newer one at t=170 s, plus slack.
    tokio::time::sleep(Duration::from_secs(90)).await;
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Idle { session, .. } if session == session_ref("newer")
    ));
}

#[tokio::test(start_paused = true)]
async fn a_session_goes_quiet_at_thirty_seconds_and_resumes_on_a_touch() {
    let events = start(BASE, vec![(key("busy"), BASE)]);
    let mut bus = events.subscribe();

    tokio::time::sleep(Duration::from_secs(31)).await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Quiet {
            session: session_ref("busy"),
            agent: AgentKind::Claude,
            at: BASE + 31,
        }
    );
    // Quiet is not idle: the session is still in the snapshot, marked
    // quiet, so a reader never derives the window from the timestamp.
    assert_eq!(live(&events).len(), 1);
    assert!(live(&events)[0].quiet);

    // A write after quiet is a resume, and starts a new quiet window.
    events.report(Observation::Touched {
        session: Some(key("busy")),
        agent: AgentKind::Claude,
        at: BASE + 40,
    });
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: Some(session_ref("busy")),
            agent: AgentKind::Claude,
            at: BASE + 40,
            resumed: true,
        }
    );
    tokio::time::sleep(Duration::from_secs(40)).await;
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Quiet {
            session: session_ref("busy"),
            agent: AgentKind::Claude,
            at: BASE + 71,
        }
    );
    assert_eq!(live(&events).len(), 1);
    assert!(live(&events)[0].quiet);
}

#[tokio::test(start_paused = true)]
async fn a_stale_observation_cannot_resurrect_an_idle_session() {
    let events = start(BASE, vec![(key("done"), BASE)]);
    let mut bus = events.subscribe();

    tokio::time::sleep(Duration::from_secs(181)).await;
    assert!(matches!(next(&mut bus).await, SessionEvent::Quiet { .. }));
    assert!(matches!(next(&mut bus).await, SessionEvent::Idle { .. }));
    assert!(live(&events).is_empty());

    // A write at or before the last known write is old news.
    for at in [BASE, BASE - 10] {
        events.report(Observation::Touched {
            session: Some(key("done")),
            agent: AgentKind::Claude,
            at,
        });
    }
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    assert!(live(&events).is_empty());

    // A genuinely newer write resumes the session without a new Started.
    events.report(Observation::Touched {
        session: Some(key("done")),
        agent: AgentKind::Claude,
        at: BASE + 181,
    });
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: Some(session_ref("done")),
            agent: AgentKind::Claude,
            at: BASE + 181,
            resumed: true,
        }
    );
    assert_eq!(live(&events).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_indexed_report_for_a_recently_idle_session_resumes_it() {
    let events = start(BASE, vec![(key("done"), BASE)]);
    let mut bus = events.subscribe();

    tokio::time::sleep(Duration::from_secs(181)).await;
    assert!(matches!(next(&mut bus).await, SessionEvent::Quiet { .. }));
    assert!(matches!(next(&mut bus).await, SessionEvent::Idle { .. }));

    // A source-label reuse can mislabel a known identity as new. The
    // registry remembers the identity and publishes a resume, not Started.
    events.report(Observation::Indexed {
        sessions: vec![indexed("done", BASE + 182, true)],
    });
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Activity {
            session: Some(session_ref("done")),
            agent: AgentKind::Claude,
            at: BASE + 182,
            resumed: true,
        }
    );
}

#[tokio::test(start_paused = true)]
async fn a_full_inbox_coalesces_same_identity_reports() {
    let events = start(BASE, Vec::new());

    // Fill the queue without yielding, so the actor cannot drain it yet.
    for index in 0..INBOX_CAPACITY {
        events.report(Observation::Touched {
            session: Some(key(&format!("s{index}"))),
            agent: AgentKind::Claude,
            at: BASE,
        });
    }
    // The queue is full. A newer epoch for a queued session coalesces.
    events.report(Observation::Touched {
        session: Some(key("s0")),
        agent: AgentKind::Claude,
        at: BASE + 9,
    });
    settle().await;

    let snapshot = events.snapshot(usize::MAX);
    assert_eq!(snapshot.sessions.len(), INBOX_CAPACITY);
    assert_eq!(snapshot.sessions[0].session, session_ref("s0"));
    assert_eq!(snapshot.sessions[0].last_activity_at, BASE + 9);
    // One Activity per session, and no Resync: nothing was lost.
    assert_eq!(snapshot.seq, INBOX_CAPACITY as u64);
}

#[tokio::test(start_paused = true)]
async fn an_overflowed_inbox_publishes_resync_for_reconciliation() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    for index in 0..INBOX_CAPACITY {
        events.report(Observation::Touched {
            session: Some(key(&format!("s{index}"))),
            agent: AgentKind::Claude,
            at: BASE,
        });
    }
    // The queue is full and this key is not queued: the report is lost.
    events.report(Observation::Touched {
        session: Some(key("lost")),
        agent: AgentKind::Claude,
        at: BASE,
    });
    settle().await;

    // The subscriber lags past the burst; the newest event is Resync.
    let mut resync_seq = None;
    loop {
        match bus.try_recv() {
            Ok(sequenced) => {
                if sequenced.event == SessionEvent::Resync {
                    resync_seq = Some(sequenced.seq);
                }
            }
            Err(TryRecvError::Lagged(_)) => continue,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Closed) => unreachable!(),
        }
    }
    let snapshot = events.snapshot(usize::MAX);
    assert_eq!(resync_seq, Some(snapshot.seq));
    assert_eq!(snapshot.sessions.len(), INBOX_CAPACITY);
    assert!(
        !snapshot
            .sessions
            .iter()
            .any(|session| session.session == session_ref("lost"))
    );
}

/// Drain every retained bus event, tolerating lag, and say whether a
/// `Resync` was among them.
fn drained_resync(bus: &mut broadcast::Receiver<Sequenced>) -> bool {
    let mut saw_resync = false;
    loop {
        match bus.try_recv() {
            Ok(sequenced) => {
                if sequenced.event == SessionEvent::Resync {
                    saw_resync = true;
                }
            }
            Err(TryRecvError::Lagged(_)) => continue,
            Err(TryRecvError::Empty) => return saw_resync,
            Err(TryRecvError::Closed) => unreachable!(),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_full_inbox_does_not_merge_a_touch_across_a_queued_removal() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    // The true order ends alive: touch, removal, then a newer touch.
    events.report(Observation::Touched {
        session: Some(key("victim")),
        agent: AgentKind::Claude,
        at: BASE + 5,
    });
    events.report(Observation::Removed {
        session: Some(key("victim")),
        reason: RemovalReason::Deleted,
    });
    for index in 0..(INBOX_CAPACITY - 2) {
        events.report(Observation::Touched {
            session: Some(key(&format!("f{index}"))),
            agent: AgentKind::Claude,
            at: BASE,
        });
    }

    // Merging this into the queued touch would replay the removal last
    // and kill a session the newest report said is alive. The report is
    // dropped and `Resync` reconciles instead.
    events.report(Observation::Touched {
        session: Some(key("victim")),
        agent: AgentKind::Claude,
        at: BASE + 10,
    });
    settle().await;

    assert!(drained_resync(&mut bus), "the refused merge must resync");
}

#[tokio::test(start_paused = true)]
async fn a_full_inbox_does_not_absorb_a_removal_into_an_earlier_duplicate() {
    let events = start(BASE, vec![(key("victim"), BASE)]);
    let mut bus = events.subscribe();

    // The true order ends dead: removal, a newer touch, then removal again.
    events.report(Observation::Removed {
        session: Some(key("victim")),
        reason: RemovalReason::Deleted,
    });
    events.report(Observation::Touched {
        session: Some(key("victim")),
        agent: AgentKind::Claude,
        at: BASE + 10,
    });
    for index in 0..(INBOX_CAPACITY - 2) {
        events.report(Observation::Touched {
            session: Some(key(&format!("f{index}"))),
            agent: AgentKind::Claude,
            at: BASE,
        });
    }

    // Absorbing this into the earlier duplicate would leave the newer
    // touch as the last word and keep the session alive.
    events.report(Observation::Removed {
        session: Some(key("victim")),
        reason: RemovalReason::Deleted,
    });
    settle().await;

    assert!(drained_resync(&mut bus), "the refused dedup must resync");
}

#[tokio::test(start_paused = true)]
async fn events_carry_an_increasing_sequence_the_snapshot_matches() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();
    assert_eq!(events.snapshot(usize::MAX).seq, 0);

    for (index, name) in ["one", "two", "three"].iter().enumerate() {
        events.report(Observation::Touched {
            session: Some(key(name)),
            agent: AgentKind::Claude,
            at: BASE + index as i64,
        });
    }
    settle().await;

    for expected in 1..=3 {
        assert_eq!(bus.recv().await.unwrap().seq, expected);
    }
    assert_eq!(events.snapshot(usize::MAX).seq, 3);
    assert_eq!(events.current_seq(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_returns_the_requested_most_recent_limit() {
    let events = start(
        BASE,
        vec![
            (key("oldest"), BASE - 20),
            (key("middle"), BASE - 10),
            (key("newest"), BASE - 1),
            // An equal epoch orders by the full identity.
            (key("also-middle"), BASE - 10),
        ],
    );
    settle().await;

    let snapshot = events.snapshot(3);
    assert_eq!(
        snapshot
            .sessions
            .iter()
            .map(|session| session.session.session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["newest", "also-middle", "middle"]
    );
    assert_eq!(events.snapshot(0).sessions.len(), 0);
    assert_eq!(events.snapshot(usize::MAX).sessions.len(), 4);
}

#[tokio::test(start_paused = true)]
async fn a_late_subscriber_reads_the_seeded_set_from_the_snapshot_not_a_replay() {
    let events = start(
        BASE,
        vec![
            (key("first"), BASE - 30),
            (key("second"), BASE - 5),
            (key("stale"), BASE - ACTIVE_SESSION_WINDOW_SECS),
            (SessionKey::new("native", "not-an-agent", "unknown"), BASE),
        ],
    );
    settle().await;

    let mut late = events.subscribe();
    assert_eq!(late.try_recv().unwrap_err(), TryRecvError::Empty);
    assert_eq!(
        live(&events),
        vec![
            LiveSession {
                session: session_ref("second"),
                agent: AgentKind::Claude,
                last_activity_at: BASE - 5,
                quiet: false,
            },
            LiveSession {
                session: session_ref("first"),
                agent: AgentKind::Claude,
                last_activity_at: BASE - 30,
                // Seeded past the quiet window: the snapshot says so.
                quiet: true,
            },
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn row_changed_publishes_an_updated_projection_trigger() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    let facets = UpdateFacets {
        analysis: true,
        ..UpdateFacets::default()
    };
    events.report(Observation::RowChanged {
        session: key("row"),
        facets,
        at: BASE + 1,
    });

    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Updated {
            session: session_ref("row"),
            facets,
            at: BASE + 1,
        }
    );
    // Updated is a projection trigger, not liveness.
    assert!(live(&events).is_empty());
}

#[tokio::test(start_paused = true)]
async fn removed_clears_the_live_entry_and_publishes() {
    let events = start(BASE, vec![(key("gone"), BASE)]);
    let mut bus = events.subscribe();

    events.report(Observation::Removed {
        session: Some(key("gone")),
        reason: RemovalReason::Deleted,
    });
    // The lifecycle scope narrates the decay first: a reader tracking
    // working state must not wait for a `Quiet` or `Idle` that the
    // registry can no longer publish for a dropped entry.
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Idle {
            session: session_ref("gone"),
            agent: AgentKind::Claude,
            at: BASE,
        }
    );
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Removed {
            session: Some(session_ref("gone")),
            reason: RemovalReason::Deleted,
        }
    );
    assert!(live(&events).is_empty());

    // No deadline remains: the actor sleeps instead of waking for it.
    tokio::time::sleep(Duration::from_secs(200)).await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    events.report(Observation::IndexChanged {
        reason: IndexChangeReason::Invalidated,
    });
    assert_eq!(
        next(&mut bus).await,
        SessionEvent::IndexChanged {
            reason: IndexChangeReason::Invalidated,
        }
    );
}

#[tokio::test(start_paused = true)]
async fn removing_an_unknown_session_publishes_removed_only() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    events.report(Observation::Removed {
        session: Some(key("never-seen")),
        reason: RemovalReason::Purged,
    });

    assert_eq!(
        next(&mut bus).await,
        SessionEvent::Removed {
            session: Some(session_ref("never-seen")),
            reason: RemovalReason::Purged,
        }
    );
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
}

#[tokio::test(start_paused = true)]
async fn a_removed_session_cannot_be_resurrected_by_a_stale_touch() {
    let events = start(BASE, vec![(key("gone"), BASE)]);
    let mut bus = events.subscribe();

    events.report(Observation::Removed {
        session: Some(key("gone")),
        reason: RemovalReason::Rejected,
    });
    assert!(matches!(next(&mut bus).await, SessionEvent::Idle { .. }));
    assert!(matches!(next(&mut bus).await, SessionEvent::Removed { .. }));

    // A write at or before the deleted row's last activity is old news.
    events.report(Observation::Touched {
        session: Some(key("gone")),
        agent: AgentKind::Claude,
        at: BASE,
    });
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    assert!(live(&events).is_empty());

    // A genuinely newer rediscovery resumes the identity.
    events.report(Observation::Touched {
        session: Some(key("gone")),
        agent: AgentKind::Claude,
        at: BASE + 5,
    });
    assert!(matches!(
        next(&mut bus).await,
        SessionEvent::Activity { resumed: true, .. }
    ));
    assert_eq!(live(&events).len(), 1);
}

#[test]
fn events_serialize_with_a_kind_tag_and_camel_case_fields() {
    let event = SessionEvent::Started {
        session: session_ref("abc"),
        agent: AgentKind::Claude,
        at: 7,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "started");
    assert_eq!(json["agent"], "claude-code");
    assert_eq!(json["session"]["sessionId"], "abc");
    assert_eq!(json["session"]["environmentKey"], "native");

    let activity = SessionEvent::Activity {
        session: None,
        agent: AgentKind::Codex,
        at: 8,
        resumed: true,
    };
    let json = serde_json::to_value(&activity).unwrap();
    assert_eq!(json["kind"], "activity");
    assert!(json["session"].is_null());
    assert_eq!(json["resumed"], true);

    let quiet = SessionEvent::Quiet {
        session: session_ref("abc"),
        agent: AgentKind::Claude,
        at: 9,
    };
    assert_eq!(serde_json::to_value(&quiet).unwrap()["kind"], "quiet");

    let updated = SessionEvent::Updated {
        session: session_ref("abc"),
        facets: UpdateFacets {
            title: true,
            ..UpdateFacets::default()
        },
        at: 10,
    };
    let json = serde_json::to_value(&updated).unwrap();
    assert_eq!(json["kind"], "updated");
    assert_eq!(json["facets"]["title"], true);
    assert_eq!(json["facets"]["usage"], false);

    let removed = SessionEvent::Removed {
        session: None,
        reason: RemovalReason::Purged,
    };
    let json = serde_json::to_value(&removed).unwrap();
    assert_eq!(json["kind"], "removed");
    assert_eq!(json["reason"], "purged");
}

#[test]
fn the_lifecycle_envelope_flattens_the_event_beside_the_sequence() {
    let event = SessionEvent::Resync;
    let envelope = LifecycleEnvelope {
        seq: 41,
        event: &event,
    };
    let json = serde_json::to_value(&envelope).unwrap();
    assert_eq!(json["seq"], 41);
    assert_eq!(json["kind"], "resync");

    let event = SessionEvent::Activity {
        session: Some(session_ref("abc")),
        agent: AgentKind::Claude,
        at: 7,
        resumed: false,
    };
    let envelope = LifecycleEnvelope {
        seq: 42,
        event: &event,
    };
    let json = serde_json::to_value(&envelope).unwrap();
    assert_eq!(json["seq"], 42);
    assert_eq!(json["kind"], "activity");
    assert_eq!(json["session"]["sessionId"], "abc");
}
