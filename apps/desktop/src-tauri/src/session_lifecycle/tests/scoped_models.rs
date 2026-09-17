use super::*;
use crate::store::PublishedModel;
use tokio::sync::oneshot;

fn observe(events: &SessionEvents, fact: Observation, now: i64) {
    apply_facts(events, &mut VecDeque::from([fact]), now);
}
fn changed() -> Observation {
    Observation::RowChanged {
        session: key("one"),
        facets: UpdateFacets {
            analysis: true,
            ..Default::default()
        },
        at: BASE,
    }
}
fn answer(
    events: &SessionEvents,
    requests: Vec<ModelRequest>,
    model: Option<&str>,
    revision: u64,
    incarnation: u64,
) {
    let rows = requests
        .iter()
        .map(|request| PublishedModel {
            key: request.key.clone(),
            incarnation: Incarnation(incarnation),
            published_fence: Some(7),
            model: model.map(str::to_owned),
            provider: Some("anthropic".into()),
        })
        .collect();
    let (ack, _) = oneshot::channel();
    models::apply_reply(
        events,
        models::ModelReply {
            requests,
            result: Ok((rows, Revision(revision))),
            ack,
        },
    );
}
fn positive(events: &SessionEvents) -> usize {
    events
        .snapshot(0)
        .sweep
        .iter()
        .flat_map(|count| &count.models)
        .map(|model| model.working)
        .sum()
}
fn seed_one() -> SessionEvents {
    let events = SessionEvents::default();
    events.seed(vec![presence("one", 1, BASE)], Revision(1), BASE);
    events
}

#[test]
fn cold_129_counts_include_the_omitted_agent_and_late_snapshots_include_models() {
    let events = SessionEvents::default();
    let mut rows: Vec<_> = (0..128)
        .map(|i| Presence {
            key: SessionKey::new("native", "codex", format!("{i:03}")),
            incarnation: Incarnation(1),
            epoch: BASE,
        })
        .collect();
    rows.push(presence("oldest", 1, BASE - 1));
    events.seed(rows, Revision(1), BASE);
    let snapshot = events.snapshot(128);
    assert_eq!(snapshot.seq, 0);
    assert_eq!(snapshot.working, 129);
    assert!(
        snapshot
            .sessions
            .iter()
            .all(|row| row.agent == AgentKind::Codex)
    );
    assert_eq!(snapshot.sweep[0].agent, "claude-code");
    assert_eq!(snapshot.sweep[0].model_pending_working, 1);
    let (requests, _) = events.model_page();
    answer(&events, requests, Some("sonnet"), 1, 1);
    assert_eq!(positive(&events), 129);
    assert_eq!(events.snapshot(0).sweep[0].models[0].working, 1);
    assert!(events.snapshot(0).seq > 0);
}

#[test]
fn model_pages_cover_513_identities_without_rich_row_interests() {
    let events = SessionEvents::default();
    events.seed(
        (0..513)
            .map(|i| presence(&format!("{i:04}"), 1, BASE))
            .collect(),
        Revision(1),
        BASE,
    );
    let mut sizes = Vec::new();
    loop {
        let (requests, _) = events.model_page();
        if requests.is_empty() {
            break;
        }
        sizes.push(requests.len());
        answer(&events, requests, Some("sonnet"), 1, 1);
    }
    assert_eq!(sizes, [256, 256, 1]);
    assert_eq!(positive(&events), 513);
    assert_eq!(events.snapshot(128).sessions.len(), 128);
}

#[test]
fn reused_publication_fence_requires_a_current_ticket_and_revision() {
    let events = seed_one();
    let (old, _) = events.model_page();
    observe(&events, changed(), BASE);
    answer(&events, old, Some("old"), 9, 1);
    assert_eq!(positive(&events), 0);
    let (current, _) = events.model_page();
    answer(&events, current, Some("new"), 9, 1);
    assert_eq!(events.snapshot(0).sweep[0].models[0].execution.model, "new");
    observe(&events, touched("one", 1, BASE + 1, 10), BASE + 1);
    observe(&events, changed(), BASE + 1);
    let (stale, _) = events.model_page();
    answer(&events, stale, Some("old"), 9, 1);
    assert_eq!(positive(&events), 0);
    assert_eq!(events.snapshot(0).sweep[0].model_failed_working, 1);
}

#[test]
fn idle_readmission_and_replacement_reject_old_model_answers() {
    for replacement in [false, true] {
        let events = seed_one();
        let (old, _) = events.model_page();
        let at = if replacement { BASE + 1 } else { BASE + 181 };
        if !replacement {
            expire(&events, at);
        }
        observe(
            &events,
            index("one", if replacement { 2 } else { 1 }, at, false, 2),
            at,
        );
        answer(&events, old, Some("old"), 3, 1);
        assert_eq!(positive(&events), 0);
        let (current, _) = events.model_page();
        answer(
            &events,
            current,
            Some("new"),
            3,
            if replacement { 2 } else { 1 },
        );
        assert_eq!(positive(&events), 1);
    }
}

#[test]
fn model_answers_never_resurrect_removed_sessions_or_refresh_timestamps() {
    let events = seed_one();
    let (requests, _) = events.model_page();
    expire(&events, BASE + 31);
    answer(&events, requests, Some("sonnet"), 2, 1);
    assert_eq!(positive(&events), 0);
    assert_eq!(events.snapshot(1).sessions[0].last_activity_at, BASE);
    observe(&events, touched("one", 1, BASE + 32, 2), BASE + 32);
    assert_eq!(positive(&events), 1);
    observe(&events, changed(), BASE + 32);
    let (requests, _) = events.model_page();
    observe(
        &events,
        removed("one", 1, RemovalReason::Deleted, 3),
        BASE + 32,
    );
    answer(&events, requests, Some("sonnet"), 2, 1);
    assert_eq!(events.snapshot(0).total, 0);
}

#[test]
fn sync_spill_analysis_and_overflow_remove_positive_evidence() {
    let events = seed_one();
    let (requests, _) = events.model_page();
    answer(&events, requests, Some("sonnet"), 1, 1);
    let mut spill = Spill::default();
    spill.fold(SyncObservation::RowChanged {
        session: key("one"),
        facets: UpdateFacets {
            analysis: true,
            ..Default::default()
        },
        at: BASE,
    });
    apply_spill(&events, spill, BASE);
    assert_eq!(positive(&events), 0);
    let (requests, _) = events.model_page();
    answer(&events, requests, Some("sonnet"), 1, 1);
    let mut spill = Spill::default();
    for i in 0..=SPILL_KEY_CAP {
        spill.fold(SyncObservation::RowChanged {
            session: key(&format!("other-{i}")),
            facets: UpdateFacets {
                analysis: true,
                ..Default::default()
            },
            at: BASE,
        });
    }
    apply_spill(&events, spill, BASE);
    assert_eq!(positive(&events), 0);
    assert_eq!(events.snapshot(0).sweep[0].model_pending_working, 1);
}

#[test]
fn broad_invalidation_during_read_recovers_all_keys_and_new_keys_behind_cursor() {
    let events = SessionEvents::default();
    events.seed(
        (0..513)
            .map(|i| presence(&format!("{i:04}"), 1, BASE))
            .collect(),
        Revision(1),
        BASE,
    );
    let (stale, _) = events.model_page();
    observe(
        &events,
        Observation::IndexChanged {
            reason: IndexChangeReason::Invalidated,
        },
        BASE,
    );
    answer(&events, stale, Some("old"), 2, 1);
    assert_eq!(positive(&events), 0);
    let (page, _) = events.model_page();
    answer(&events, page, Some("new"), 2, 1);
    observe(&events, index("!behind", 1, BASE, true, 2), BASE);
    for _ in 0..6 {
        let (page, _) = events.model_page();
        answer(&events, page, Some("new"), 2, 1);
    }
    assert_eq!(positive(&events), 514);
}

#[tokio::test(start_paused = true)]
async fn failed_middle_page_backs_off_without_blocking_other_pages() {
    let events = SessionEvents::default();
    events.seed(
        (0..513)
            .map(|i| presence(&format!("{i:04}"), 1, BASE))
            .collect(),
        Revision(1),
        BASE,
    );
    let (first, _) = events.model_page();
    answer(&events, first, Some("sonnet"), 1, 1);
    let (middle, _) = events.model_page();
    let (ack, _) = oneshot::channel();
    models::apply_reply(
        &events,
        models::ModelReply {
            requests: middle,
            result: Err(anyhow::anyhow!("synthetic failure")),
            ack,
        },
    );
    let (last, _) = events.model_page();
    assert_eq!(last.len(), 1);
    answer(&events, last, None, 1, 1);
    assert!(events.model_page().0.is_empty());
    assert_eq!(events.snapshot(0).sweep[0].model_failed_working, 256);
    assert_eq!(events.snapshot(0).sweep[0].model_none_working, 1);
    tokio::time::advance(Duration::from_secs(2)).await;
    let (retry, _) = events.model_page();
    assert_eq!(retry.len(), 256);
    answer(&events, retry, Some("sonnet"), 1, 1);
    assert_eq!(positive(&events), 512);
}

#[tokio::test(start_paused = true)]
async fn actor_acknowledges_models_while_admission_carry_is_blocked() {
    let source = Arc::new(Scripted::default());
    let harness = start_with(BASE, vec![(key("one"), BASE)], source.clone());
    let events = &harness.events;
    let (requests, _) = events.model_page();
    {
        let mut registry = events.registry.lock().unwrap();
        registry.broad_through = Revision(100);
        for i in 0..ADMISSION_PENDING_CAP {
            registry.pending.insert(
                key(&format!("blocked-{i}")),
                PendingAdmission {
                    agent: AgentKind::Claude,
                    incarnation: Incarnation(1),
                    at: BASE,
                    revision: Revision(1),
                    is_new: true,
                },
            );
        }
    }
    source.hold();
    events
        .report_async(index("blocked-tail", 1, BASE, true, 1))
        .await;
    settle().await;
    let rows = vec![PublishedModel {
        key: key("one"),
        incarnation: Incarnation(0),
        published_fence: Some(7),
        model: Some("sonnet".into()),
        provider: Some("anthropic".into()),
    }];
    events
        .submit_models(requests, Ok((rows, Revision(200))))
        .await;
    assert_eq!(positive(events), 1);
    source.release();
}
