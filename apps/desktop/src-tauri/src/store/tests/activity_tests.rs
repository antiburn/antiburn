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
