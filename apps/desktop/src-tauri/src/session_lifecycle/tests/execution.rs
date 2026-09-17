use super::*;
use crate::store::PublishedModel;
use tokio::sync::oneshot;

fn pi(id: &str) -> SessionKey {
    SessionKey::new("native", "pi", id)
}
fn seeded(count: usize) -> SessionEvents {
    let events = SessionEvents::default();
    events.seed(
        (0..count)
            .map(|i| Presence {
                key: pi(&i.to_string()),
                incarnation: Incarnation(1),
                epoch: BASE,
            })
            .collect(),
        Revision(1),
        BASE,
    );
    events
}
fn reply(events: &SessionEvents, requests: Vec<ModelRequest>, provider: Option<&str>, model: &str) {
    let rows = requests
        .iter()
        .map(|request| PublishedModel {
            key: request.key.clone(),
            incarnation: Incarnation(1),
            published_fence: Some(7),
            provider: provider.map(str::to_owned),
            model: Some(model.into()),
        })
        .collect();
    let (ack, _) = oneshot::channel();
    models::apply_reply(
        events,
        models::ModelReply {
            requests,
            result: Ok((rows, Revision(2))),
            ack,
        },
    );
}
fn invalidate(events: &SessionEvents) {
    apply_facts(
        events,
        &mut VecDeque::from([Observation::RowChanged {
            session: pi("0"),
            facets: UpdateFacets {
                analysis: true,
                ..Default::default()
            },
            at: BASE,
        }]),
        BASE,
    );
}

#[test]
fn pi_exposes_harness_route_model_and_vendor_from_registry_snapshots_and_presence() {
    for (provider, model, route, vendor) in [
        (
            Some("openai-codex"),
            "gpt-6-astra",
            Some("openai"),
            Some("openai"),
        ),
        (
            Some("anthropic"),
            "claude-fable-5",
            Some("anthropic"),
            Some("anthropic"),
        ),
        (
            Some("openrouter"),
            "anthropic/claude-fable-5",
            Some("openrouter"),
            Some("anthropic"),
        ),
        (
            Some("amazon-bedrock"),
            "claude-fable-5",
            Some("aws"),
            Some("anthropic"),
        ),
        (
            Some("aws"),
            "claude-fable-5",
            Some("aws"),
            Some("anthropic"),
        ),
        (
            Some("azure"),
            "claude-fable-5",
            Some("azure"),
            Some("anthropic"),
        ),
        (
            Some("google-vertex"),
            "gemini-pro",
            Some("google-vertex"),
            Some("google"),
        ),
        (
            Some("vertex"),
            "gemini-pro",
            Some("google-vertex"),
            Some("google"),
        ),
        (Some("custom-route"), "gpt-6-astra", None, Some("openai")),
        (None, "gpt-6-astra", None, Some("openai")),
        (Some(""), "claude-fable-5", None, Some("anthropic")),
        (Some("anthropic"), "unknown-model", Some("anthropic"), None),
        (
            Some("openrouter"),
            "openai/unknown-model",
            Some("openrouter"),
            None,
        ),
    ] {
        let events = seeded(1);
        reply(&events, events.model_page().0, provider, model);
        let snapshot = events.snapshot(1);
        let row = &snapshot.sessions[0];
        assert_eq!(row.agent, AgentKind::Pi);
        assert_eq!(row.session.agent, "pi");
        assert_eq!(row.last_activity_at, BASE);
        let execution = row.execution.as_ref().unwrap();
        assert_eq!(execution.model, model);
        assert_eq!(execution.recorded_provider.as_deref(), provider);
        assert_eq!(execution.provider_route.as_deref(), route);
        assert_eq!(execution.model_vendor.as_deref(), vendor);
        assert_eq!(snapshot.sweep[0].agent, "pi");
        assert_eq!(&snapshot.sweep[0].models[0].execution, execution);
        let presence = events.presence(&[SessionRef::from(&pi("0"))]);
        assert_eq!(presence.seq, snapshot.seq);
        assert_eq!(&presence.present[0], row);
        let wire = serde_json::to_value(row).unwrap();
        assert_eq!(wire["execution"]["model"], model);
        assert_eq!(
            wire["execution"]["recordedProvider"],
            serde_json::json!(provider)
        );
        assert_eq!(wire["execution"]["providerRoute"], serde_json::json!(route));
        assert_eq!(wire["execution"]["modelVendor"], serde_json::json!(vendor));
    }
}

#[test]
fn same_model_on_two_routes_stays_separate_beyond_the_snapshot_limit() {
    let events = seeded(130);
    let mut requests = events.model_page().0;
    let other = requests.split_off(129);
    reply(&events, requests, Some("anthropic"), "claude-fable-5");
    reply(&events, other, Some("openrouter"), "claude-fable-5");
    let snapshot = events.snapshot(128);
    assert_eq!(snapshot.sessions.len(), 128);
    assert_eq!(snapshot.working, 130);
    assert_eq!(snapshot.sweep[0].models.len(), 2);
    assert_eq!(
        snapshot.sweep[0]
            .models
            .iter()
            .map(|count| count.working)
            .sum::<usize>(),
        130
    );
    assert_eq!(snapshot.sweep[0].models[0].working, 129);
    assert_eq!(
        snapshot.sweep[0].models[0]
            .execution
            .provider_route
            .as_deref(),
        Some("anthropic")
    );
    assert_eq!(snapshot.sweep[0].models[1].working, 1);
    assert_eq!(
        snapshot.sweep[0].models[1]
            .execution
            .provider_route
            .as_deref(),
        Some("openrouter")
    );
}

#[test]
fn route_switch_invalidation_and_delayed_answers_never_invent_activity() {
    let events = seeded(1);
    let old = events.model_page().0;
    reply(&events, old.clone(), Some("openai-codex"), "gpt-6-astra");
    let mut bus = events.subscribe();
    invalidate(&events);
    assert!(events.snapshot(1).sessions[0].execution.is_none());
    assert!(events.snapshot(0).sweep[0].models.is_empty());
    reply(&events, old, Some("openai-codex"), "gpt-6-astra");
    assert!(events.snapshot(1).sessions[0].execution.is_none());
    reply(
        &events,
        events.model_page().0,
        Some("anthropic"),
        "claude-fable-5",
    );
    let snapshot = events.snapshot(1);
    assert_eq!(snapshot.working, 1);
    assert_eq!(snapshot.sessions[0].last_activity_at, BASE);
    assert_eq!(
        snapshot.sessions[0]
            .execution
            .as_ref()
            .unwrap()
            .provider_route
            .as_deref(),
        Some("anthropic")
    );
    while let Ok(event) = bus.try_recv() {
        assert!(!matches!(
            event.event,
            SessionEvent::Started { .. } | SessionEvent::Activity { .. }
        ));
    }
}

#[test]
fn failed_metadata_after_invalidation_has_no_stale_provider_signal() {
    let events = seeded(1);
    reply(
        &events,
        events.model_page().0,
        Some("openai-codex"),
        "gpt-6-astra",
    );
    invalidate(&events);
    let (ack, _) = oneshot::channel();
    models::apply_reply(
        &events,
        models::ModelReply {
            requests: events.model_page().0,
            result: Err(anyhow::anyhow!("synthetic read failure")),
            ack,
        },
    );
    let snapshot = events.snapshot(1);
    assert_eq!(snapshot.working, 1);
    assert_eq!(snapshot.sweep[0].model_failed_working, 1);
    assert!(snapshot.sweep[0].models.is_empty());
    assert!(snapshot.sessions[0].execution.is_none());
}

#[test]
fn idle_and_recreated_pi_sessions_reject_delayed_provider_answers() {
    for recreated in [false, true] {
        let events = seeded(1);
        let old = events.model_page().0;
        reply(&events, old.clone(), Some("openai-codex"), "gpt-6-astra");
        let at = if recreated { BASE + 1 } else { BASE + 181 };
        if !recreated {
            expire(&events, at);
        }
        apply_facts(
            &events,
            &mut VecDeque::from([Observation::Indexed {
                sessions: vec![IndexedSession {
                    key: pi("0"),
                    agent: AgentKind::Pi,
                    incarnation: Incarnation(if recreated { 2 } else { 1 }),
                    at,
                    is_new: recreated,
                }],
                revision: Revision(2),
            }]),
            at,
        );
        reply(&events, old, Some("openai-codex"), "gpt-6-astra");
        let snapshot = events.snapshot(1);
        assert!(snapshot.sessions[0].execution.is_none());
        assert!(snapshot.sweep[0].models.is_empty());
        assert_eq!(snapshot.sessions[0].last_activity_at, at);
    }
}

#[test]
fn quiet_metadata_changes_have_sequences_and_broad_invalidation_clears_named_evidence() {
    let events = seeded(1);
    expire(&events, BASE + 31);
    let before = events.current_seq();
    reply(
        &events,
        events.model_page().0,
        Some("openai-codex"),
        "gpt-6-astra",
    );
    assert!(events.current_seq() > before);
    let before = events.current_seq();
    apply_facts(
        &events,
        &mut VecDeque::from([Observation::IndexChanged {
            reason: IndexChangeReason::Invalidated,
        }]),
        BASE + 31,
    );
    assert!(events.current_seq() > before);
    let presence = events.presence(&[SessionRef::from(&pi("0"))]);
    assert!(presence.present[0].execution.is_none());
    assert!(presence.present[0].quiet);
    assert_eq!(presence.present[0].last_activity_at, BASE);
    assert_eq!(events.snapshot(0).working, 0);
}
