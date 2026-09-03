//! Store-level tests for the session activity queries the continuous
//! ingest surfaces read (phase 4b): the idle-expiry timer's active set and
//! the HUD's latest-activity signal.

use super::*;

#[test]
fn sessions_active_since_returns_earliest_activity_first() {
    let store = store();
    store
        .upsert_sessions(&[session("old", 1_000)], &crate::agents::evidence_cohort())
        .unwrap();
    store
        .upsert_sessions(&[session("mid", 2_000)], &crate::agents::evidence_cohort())
        .unwrap();
    store
        .upsert_sessions(&[session("new", 3_000)], &crate::agents::evidence_cohort())
        .unwrap();

    let active = store.sessions_active_since(1_500).unwrap();
    let ids: Vec<_> = active
        .iter()
        .map(|(key, _)| key.session_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["mid", "new"],
        "windowed and oldest activity first"
    );
    assert_eq!(active[0].1, 2_000);
    assert_eq!(active[1].1, 3_000);
}

#[test]
fn sessions_active_since_query_plan_uses_the_coalesced_recency_index() {
    let store = store();
    let connection = store.lock();
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {SESSIONS_ACTIVE_SINCE_SQL}"))
        .unwrap();
    let plan_lines: Vec<String> = statement
        .query_map(params![0_i64], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let plan = plan_lines.join("\n");
    assert!(
        plan.contains("session_recency_coalesced"),
        "query plan did not use the coalesced index: {plan}"
    );
}
#[test]
fn latest_session_activity_ignores_null_epochs() {
    let store = store();
    assert_eq!(
        store.latest_session_activity().unwrap(),
        None,
        "empty store"
    );

    let mut no_heartbeat = session("no-heartbeat", 0);
    no_heartbeat.updated_at_epoch = None;
    store
        .upsert_sessions(&[no_heartbeat], &crate::agents::evidence_cohort())
        .unwrap();
    assert_eq!(
        store.latest_session_activity().unwrap(),
        None,
        "a NULL epoch is not activity"
    );

    store
        .upsert_sessions(
            &[session("recent", 5_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert_eq!(store.latest_session_activity().unwrap(), Some(5_000));
}

#[test]
fn session_record_by_source_label_round_trips() {
    let store = store();
    let record = session("by-label", 4_000);
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    let (key, found) = store
        .session_record_by_source_label(&record.source_label)
        .unwrap()
        .expect("a stored session is found by its source label");
    assert_eq!(key.environment_key, "native");
    assert_eq!(key.agent, "claude-code");
    assert_eq!(key.source_label, record.source_label);
    assert_eq!(found.key.session_id, "by-label");

    assert!(
        store
            .session_record_by_source_label("/nowhere/unknown.jsonl")
            .unwrap()
            .is_none(),
        "an unknown source label finds nothing"
    );
}

#[test]
fn an_agent_scoped_upsert_leaves_another_agents_rows_intact() {
    let store = store();
    let claude = session("claude-session", 1_000);
    let codex = SessionRecord {
        key: SessionKey::new("native", "codex", "codex-session"),
        source_label: "/home/avery/.codex/sessions/codex-session.jsonl".into(),
        ..session("codex-session", 2_000)
    };
    store
        .upsert_sessions(
            &[claude.clone(), codex.clone()],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    // An agent-scoped pass upserts only Claude's row, naming Claude alone as
    // the evidence cohort for this pass.
    let renamed_claude = SessionRecord {
        title: Some("Renamed during a scoped pass".into()),
        ..claude.clone()
    };
    store
        .upsert_sessions(&[renamed_claude], &["claude-code"])
        .unwrap();

    let stored_claude = store.session(&claude.key).unwrap().expect("claude row");
    assert_eq!(
        stored_claude.title.as_deref(),
        Some("Renamed during a scoped pass")
    );
    let stored_codex = store
        .session(&codex.key)
        .unwrap()
        .expect("codex row is untouched by a Claude-scoped pass");
    assert_eq!(stored_codex.key.session_id, "codex-session");
}
