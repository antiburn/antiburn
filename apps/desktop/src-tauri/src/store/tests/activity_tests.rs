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
fn native_file_session_activity_keys_keep_full_identity() {
    let store = store();
    let record = session("by-label", 4_000);
    let mut wsl = record.clone();
    wsl.key.environment_key = "wsl:Ubuntu".into();
    wsl.key.session_id = "by-label-wsl".into();
    wsl.wsl_distro = Some("Ubuntu".into());
    let mut codex = record.clone();
    codex.key.agent = "codex".into();
    codex.key.session_id = "by-label-codex".into();
    let mut inline = record.clone();
    inline.key.session_id = "by-label-inline".into();
    inline.source_kind = "inline".into();
    store
        .upsert_sessions(
            &[record.clone(), wsl, codex, inline],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    let labels = BTreeSet::from([record.source_label.clone(), "/nowhere/unknown.jsonl".into()]);
    let found = store
        .native_file_session_activity_keys(&labels)
        .unwrap()
        .remove(&record.source_label)
        .expect("native file sessions are found by source label");
    assert_eq!(
        found,
        BTreeSet::from([
            SessionActivityKey::new("native", "claude-code", &record.source_label),
            SessionActivityKey::new("native", "codex", &record.source_label),
        ])
    );

    assert!(
        store
            .native_file_session_activity_keys(&BTreeSet::from(["/nowhere/unknown.jsonl".into()]))
            .unwrap()
            .is_empty(),
        "an unknown source label finds nothing"
    );
}

#[test]
fn native_file_session_activity_keys_cross_the_query_chunk_boundary() {
    let store = store();
    let mut first = session("chunk-first", 4_000);
    first.source_label = "/lookup/000.jsonl".into();
    let mut last = session("chunk-last", 4_001);
    last.source_label = format!("/lookup/{SOURCE_LABEL_LOOKUP_CHUNK_SIZE:03}.jsonl");
    store
        .upsert_sessions(
            &[first.clone(), last.clone()],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let labels = (0..=SOURCE_LABEL_LOOKUP_CHUNK_SIZE)
        .map(|index| format!("/lookup/{index:03}.jsonl"))
        .collect::<BTreeSet<_>>();

    let found = store.native_file_session_activity_keys(&labels).unwrap();

    assert_eq!(found.len(), 2);
    assert!(found.contains_key(&first.source_label));
    assert!(found.contains_key(&last.source_label));
}

#[test]
fn session_record_by_activity_key_keeps_environment_and_agent_identity() {
    let store = store();
    let native = session("native-label", 4_000);
    let mut wsl = native.clone();
    wsl.key.environment_key = "wsl:Ubuntu".into();
    wsl.key.session_id = "wsl-label".into();
    wsl.wsl_distro = Some("Ubuntu".into());
    let mut codex = native.clone();
    codex.key.agent = "codex".into();
    codex.key.session_id = "codex-label".into();
    store
        .upsert_sessions(
            &[native.clone(), wsl, codex],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    let key = SessionActivityKey::new("native", "claude-code", &native.source_label);
    let found = store
        .session_record_by_activity_key(&key)
        .unwrap()
        .expect("the exact native agent row is found");
    assert_eq!(found.key, native.key);
}

#[test]
fn migrated_source_lookup_query_plan_uses_the_source_index() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..37] {
        connection.execute_batch(sql).unwrap();
    }
    connection.pragma_update(None, "user_version", 37).unwrap();
    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-source-lookup-migration-test").to_path_buf(),
    )
    .expect("the real prior schema migrates to the source lookup index");

    let connection = store.lock();
    let sql = native_file_session_activity_keys_sql(2);
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap();
    let plan = statement
        .query_map(params!["/one.jsonl", "/two.jsonl"], |row| {
            row.get::<_, String>(3)
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    assert!(
        plan.contains("session_source_lookup") && !plan.contains("SCAN session"),
        "query plan did not search the source lookup index: {plan}"
    );
}

#[test]
fn bounded_scan_history_query_plan_uses_the_source_index() {
    let store = store();
    let connection = store.lock();
    let sql = session_records_for_activity_keys_sql(2);
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap();
    let plan = statement
        .query_map(
            params![
                "native",
                "claude-code",
                "/one.jsonl",
                "wsl:ubuntu",
                "codex",
                "/two.jsonl",
            ],
            |row| row.get::<_, String>(3),
        )
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    assert!(
        plan.contains("session_source_lookup") && !plan.contains("SCAN s"),
        "query plan did not search the source lookup index: {plan}"
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

/// R3: only a `source-missing` failure marks a session for a forced rewrite.
/// A different failure reason, or a session with no failure at all, must not
/// come back from this query.
#[test]
fn sessions_with_missing_source_returns_only_that_failure_reason() {
    let store = store();
    store
        .upsert_sessions(
            &[
                session("returned", 1_000),
                session("unrequested-returned", 1_500),
                session("other-failure", 2_000),
                session("healthy", 3_000),
            ],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    {
        let connection = store.lock();
        connection
            .execute(
                "UPDATE session_evidence SET status = 'failed', last_error = ?1
                  WHERE session_id IN ('returned', 'unrequested-returned')",
                params![crate::insights_worker::EVIDENCE_ERROR_SOURCE_MISSING],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE session_evidence SET status = 'failed', last_error = 'source-unreadable'
                  WHERE session_id = 'other-failure'",
                [],
            )
            .unwrap();
    }

    let candidates = [
        SessionKey::new("native", "claude-code", "returned"),
        SessionKey::new("native", "claude-code", "other-failure"),
        SessionKey::new("native", "claude-code", "healthy"),
    ];
    let missing = store.sessions_with_missing_source_for(&candidates).unwrap();
    let ids: Vec<_> = missing.iter().map(|key| key.session_id.as_str()).collect();
    assert_eq!(ids, vec!["returned"]);
}
