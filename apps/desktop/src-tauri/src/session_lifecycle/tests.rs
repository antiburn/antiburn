use super::*;
use tokio::sync::broadcast::error::TryRecvError;

fn key(session_id: &str) -> SessionKey {
    SessionKey::new("native", "claude-code", session_id)
}

fn session_ref(session_id: &str) -> SessionRef {
    SessionRef::from(&key(session_id))
}

/// A clock tied to tokio's own (pausable) instant, not the wall clock: every
/// `tokio::time::sleep` the actor awaits advances it exactly as far as it
/// advances tokio's clock, so a paused test can drive it deterministically.
fn instant_clock(base_epoch: i64) -> impl Fn() -> i64 + Send + Sync + 'static {
    let base_instant = Instant::now();
    move || base_epoch + base_instant.elapsed().as_secs() as i64
}

/// Seed the map, start the actor on tokio's clock, and return the handle a
/// test reports through and subscribes to.
fn start(base_epoch: i64, seed: Vec<(SessionKey, i64)>) -> Arc<SessionEvents> {
    let events = Arc::new(SessionEvents::default());
    events.seed(seed, base_epoch);
    let inbox = events
        .take_inbox()
        .expect("a fresh bus still holds its inbox");
    let actor_events = events.clone();
    tokio::spawn(async move {
        let now = instant_clock(base_epoch);
        run(inbox, &actor_events, &now).await;
    });
    events
}

/// Let the actor drain its inbox without advancing the clock.
async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
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
        bus.recv().await.unwrap(),
        SessionEvent::Activity {
            session: Some(session_ref("busy")),
            agent: AgentKind::Claude,
            at: BASE + 3,
        }
    );
    assert_eq!(
        events.live_sessions(),
        vec![LiveSession {
            session: session_ref("busy"),
            agent: AgentKind::Claude,
            last_activity_at: BASE + 3,
        }]
    );
}

#[tokio::test(start_paused = true)]
async fn touched_without_a_session_publishes_activity_for_the_agent_only() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    events.report(Observation::Touched {
        session: None,
        agent: AgentKind::Codex,
        at: BASE,
    });

    assert_eq!(
        bus.recv().await.unwrap(),
        SessionEvent::Activity {
            session: None,
            agent: AgentKind::Codex,
            at: BASE,
        }
    );
    assert!(events.live_sessions().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_first_index_publishes_started_once() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    let indexed = Observation::Indexed {
        sessions: vec![(key("fresh"), AgentKind::Claude, BASE)],
        new: vec![key("fresh")],
    };
    events.report(indexed.clone());
    assert_eq!(
        bus.recv().await.unwrap(),
        SessionEvent::Started {
            session: session_ref("fresh"),
            agent: AgentKind::Claude,
            at: BASE,
        }
    );

    // The same rows again, with no newer epoch: nothing to say.
    events.report(indexed);
    settle().await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // A later epoch for a known session is activity, not another start.
    events.report(Observation::Indexed {
        sessions: vec![(key("fresh"), AgentKind::Claude, BASE + 5)],
        new: Vec::new(),
    });
    assert_eq!(
        bus.recv().await.unwrap(),
        SessionEvent::Activity {
            session: Some(session_ref("fresh")),
            agent: AgentKind::Claude,
            at: BASE + 5,
        }
    );
}

#[tokio::test(start_paused = true)]
async fn an_index_older_than_the_window_is_history_not_activity() {
    let events = start(BASE, Vec::new());
    let mut bus = events.subscribe();

    events.report(Observation::Indexed {
        sessions: vec![(
            key("old"),
            AgentKind::Claude,
            BASE - ACTIVE_SESSION_WINDOW_SECS,
        )],
        new: vec![key("old")],
    });
    settle().await;

    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);
    assert!(events.live_sessions().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_session_goes_idle_at_the_window_and_a_touch_moves_the_deadline() {
    let events = start(BASE, vec![(key("seeded"), BASE)]);
    let mut bus = events.subscribe();

    // 170 s in: quiet since 30 s, still 10 s left, and a touch restarts
    // both windows.
    tokio::time::sleep(Duration::from_secs(170)).await;
    assert_eq!(
        bus.recv().await.unwrap(),
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
        bus.recv().await.unwrap(),
        SessionEvent::Activity { .. }
    ));

    // The original deadline (180 s plus slack) passes with nothing to say.
    tokio::time::sleep(Duration::from_secs(15)).await;
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // The moved deadlines: quiet at 170 + 30 + 1 s of slack, then idle at
    // 170 + 180 + 1.
    tokio::time::sleep(Duration::from_secs(170)).await;
    assert_eq!(
        bus.recv().await.unwrap(),
        SessionEvent::Quiet {
            session: session_ref("seeded"),
            agent: AgentKind::Claude,
            at: BASE + 201,
        }
    );
    assert_eq!(
        bus.recv().await.unwrap(),
        SessionEvent::Idle {
            session: session_ref("seeded"),
            agent: AgentKind::Claude,
            at: BASE + 351,
        }
    );
    assert!(events.live_sessions().is_empty());
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
        bus.recv().await.unwrap(),
        SessionEvent::Quiet {
            session: session_ref("newer"),
            agent: AgentKind::Claude,
            at: BASE + 21,
        }
    );

    // The older session crosses its window at t=80 s, plus slack.
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(matches!(
        bus.recv().await.unwrap(),
        SessionEvent::Idle { session, .. } if session == session_ref("older")
    ));
    assert_eq!(bus.try_recv().unwrap_err(), TryRecvError::Empty);

    // The newer one at t=170 s, plus slack.
    tokio::time::sleep(Duration::from_secs(90)).await;
    assert!(matches!(
        bus.recv().await.unwrap(),
        SessionEvent::Idle { session, .. } if session == session_ref("newer")
    ));
}

#[tokio::test(start_paused = true)]
async fn a_session_goes_quiet_at_thirty_seconds_and_again_after_a_touch() {
    let events = start(BASE, vec![(key("busy"), BASE)]);
    let mut bus = events.subscribe();

    tokio::time::sleep(Duration::from_secs(31)).await;
    assert_eq!(
        bus.recv().await.unwrap(),
        SessionEvent::Quiet {
            session: session_ref("busy"),
            agent: AgentKind::Claude,
            at: BASE + 31,
        }
    );
    // Quiet is not idle: the session is still in the snapshot.
    assert_eq!(events.live_sessions().len(), 1);

    // A write after quiet is activity again, and starts a new quiet window.
    events.report(Observation::Touched {
        session: Some(key("busy")),
        agent: AgentKind::Claude,
        at: BASE + 40,
    });
    assert!(matches!(
        bus.recv().await.unwrap(),
        SessionEvent::Activity { .. }
    ));
    tokio::time::sleep(Duration::from_secs(40)).await;
    assert_eq!(
        bus.recv().await.unwrap(),
        SessionEvent::Quiet {
            session: session_ref("busy"),
            agent: AgentKind::Claude,
            at: BASE + 71,
        }
    );
    assert_eq!(events.live_sessions().len(), 1);
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
        events.live_sessions(),
        vec![
            LiveSession {
                session: session_ref("second"),
                agent: AgentKind::Claude,
                last_activity_at: BASE - 5,
            },
            LiveSession {
                session: session_ref("first"),
                agent: AgentKind::Claude,
                last_activity_at: BASE - 30,
            },
        ]
    );
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

    let idle = SessionEvent::Activity {
        session: None,
        agent: AgentKind::Codex,
        at: 8,
    };
    let json = serde_json::to_value(&idle).unwrap();
    assert_eq!(json["kind"], "activity");
    assert!(json["session"].is_null());

    let quiet = SessionEvent::Quiet {
        session: session_ref("abc"),
        agent: AgentKind::Claude,
        at: 9,
    };
    assert_eq!(serde_json::to_value(&quiet).unwrap()["kind"], "quiet");
}
