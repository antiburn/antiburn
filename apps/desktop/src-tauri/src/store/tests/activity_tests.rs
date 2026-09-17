//! Store-level tests for the session activity queries the continuous
//! ingest surfaces read: the lifecycle registry's paged active window and
//! presence lookups, and the watcher's identity lookups.

use super::*;

/// Walk the active window from the first page to the last, `limit` rows
/// per page, and return every page in order.
fn walk_active_pages(store: &Store, since: i64, limit: usize) -> Vec<Vec<Presence>> {
    let mut pages = Vec::new();
    let mut cursor: Option<ActiveCursor> = None;
    loop {
        let (page, _revision) = store
            .sessions_active_since_page(since, cursor.as_ref(), limit)
            .unwrap();
        assert!(page.len() <= limit, "a page never exceeds its limit");
        let last = page.len() < limit;
        cursor = page.last().map(ActiveCursor::after);
        pages.push(page);
        if last {
            return pages;
        }
    }
}

/// The page order's key for one row: descending on every column.
fn page_order_key(row: &Presence) -> (i64, String, String, String) {
    (
        row.epoch,
        row.key.session_id.clone(),
        row.key.environment_key.clone(),
        row.key.agent.clone(),
    )
}

#[test]
fn the_active_page_walk_returns_newest_first_within_the_window() {
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

    let (page, revision) = store.sessions_active_since_page(1_500, None, 10).unwrap();
    let ids: Vec<_> = page.iter().map(|row| row.key.session_id.as_str()).collect();
    assert_eq!(ids, vec!["new", "mid"], "windowed and newest first");
    assert_eq!(page[0].epoch, 3_000);
    assert_eq!(page[1].epoch, 2_000);
    assert_eq!(page[0].incarnation, Incarnation(3));
    assert_eq!(page[1].incarnation, Incarnation(2));
    assert_eq!(revision, store.revision(), "the page carries its revision");
}

#[test]
fn the_active_page_walk_visits_every_identity_once_with_shared_epoch_and_session_id() {
    // More than two pages of identities that tie on the first two order
    // columns. Only `environment_key` and `agent` tell them apart, so a
    // cursor on fewer than four columns would skip or repeat rows.
    const TIED: usize = 600;
    const LIMIT: usize = 256;
    let store = store();
    let agents = ["claude-code", "codex", "cursor"];
    let mut records = Vec::with_capacity(TIED + 2);
    for index in 0..TIED {
        let mut record = session("shared", 5_000);
        record.key.environment_key = format!("wsl:distro-{:03}", index / agents.len());
        record.key.agent = agents[index % agents.len()].into();
        record.wsl_distro = Some(record.key.environment_key.clone());
        record.source_label = format!("/tied/{index:03}.jsonl");
        records.push(record);
    }
    // One row on each side of the tie, and one outside the window.
    records.push(session("zzz-newer", 5_000));
    records.push(session("aaa-older", 5_000));
    records.push(session("outside", 100));
    store
        .upsert_sessions(&records, &crate::agents::evidence_cohort())
        .unwrap();

    let pages = walk_active_pages(&store, 1_000, LIMIT);
    let rows: Vec<Presence> = pages.iter().flatten().cloned().collect();

    assert_eq!(
        pages.len(),
        (TIED + 2).div_ceil(LIMIT),
        "{} rows",
        rows.len()
    );
    assert_eq!(rows.len(), TIED + 2, "every windowed identity once");
    let distinct: BTreeSet<&SessionKey> = rows.iter().map(|row| &row.key).collect();
    assert_eq!(distinct.len(), TIED + 2, "no identity repeats");
    assert!(
        rows.iter().all(|row| row.key.session_id != "outside"),
        "the window excludes older rows"
    );
    assert_eq!(rows[0].key.session_id, "zzz-newer");
    assert_eq!(rows[TIED + 1].key.session_id, "aaa-older");
    let keys: Vec<_> = rows.iter().map(page_order_key).collect();
    assert!(
        keys.windows(2).all(|pair| pair[0] > pair[1]),
        "pages follow one strict descending order across page boundaries"
    );
    let mut expected: Vec<_> = records
        .iter()
        .filter(|record| record.key.session_id != "outside")
        .map(|record| {
            (
                record.updated_at_epoch.unwrap(),
                record.key.session_id.clone(),
                record.key.environment_key.clone(),
                record.key.agent.clone(),
            )
        })
        .collect();
    expected.sort();
    expected.reverse();
    assert_eq!(keys, expected, "the walk equals one sorted read");
}

#[test]
fn the_active_page_walk_terminates_when_rows_change_between_pages() {
    let store = store();
    let mut records = Vec::new();
    for index in 0..30 {
        records.push(session(&format!("row-{index:02}"), 2_000 + index));
    }
    store
        .upsert_sessions(&records, &crate::agents::evidence_cohort())
        .unwrap();

    let (first, _) = store.sessions_active_since_page(1_000, None, 10).unwrap();
    assert_eq!(first.len(), 10);
    let cursor = ActiveCursor::after(first.last().unwrap());

    // Between pages: a visited row moves ahead of the cursor, a new row
    // lands behind it, and a new row lands ahead of it.
    let mut moved = session("row-25", 9_000);
    moved.activity_source = "event".into();
    store
        .upsert_sessions(
            &[
                moved,
                session("inserted-behind", 1_500),
                session("inserted-ahead", 9_500),
            ],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    let mut visited: Vec<Presence> = first.clone();
    let mut cursor = Some(cursor);
    let mut pages = 1;
    loop {
        let (page, _) = store
            .sessions_active_since_page(1_000, cursor.as_ref(), 10)
            .unwrap();
        pages += 1;
        assert!(pages <= 5, "the walk terminates");
        let last = page.len() < 10;
        cursor = page.last().map(ActiveCursor::after);
        visited.extend(page);
        if last {
            break;
        }
    }

    let ids: BTreeSet<&str> = visited
        .iter()
        .map(|row| row.key.session_id.as_str())
        .collect();
    assert_eq!(visited.len(), ids.len(), "no row is visited twice");
    for index in 0..30 {
        assert!(ids.contains(format!("row-{index:02}").as_str()));
    }
    assert!(
        ids.contains("inserted-behind"),
        "a row inserted behind the cursor is visited"
    );
    assert!(
        !ids.contains("inserted-ahead"),
        "a row inserted ahead of the cursor belongs to its own report"
    );
    assert_eq!(
        visited
            .iter()
            .filter(|row| row.key.session_id == "row-25")
            .count(),
        1,
        "a visited row that moved ahead of the cursor is not visited again"
    );
}

#[test]
fn a_page_limit_of_zero_returns_no_rows_and_a_revision() {
    let store = store();
    store
        .upsert_sessions(&[session("one", 5_000)], &crate::agents::evidence_cohort())
        .unwrap();
    let (page, revision) = store.sessions_active_since_page(0, None, 0).unwrap();
    assert!(page.is_empty());
    assert_eq!(revision, store.revision());
}

fn plan_for(
    connection: &rusqlite::Connection,
    sql: &str,
    params: &[rusqlite::types::Value],
) -> String {
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap();
    statement
        .query_map(params_from_iter(params.iter()), |row| {
            row.get::<_, String>(3)
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join("\n")
}

#[test]
fn active_page_statements_use_the_keyset_index_without_a_temp_btree() {
    use rusqlite::types::Value;
    let store = store();
    let connection = store.lock();
    let first = plan_for(
        &connection,
        SESSIONS_ACTIVE_PAGE_FIRST_SQL,
        &[Value::Integer(0), Value::Integer(256)],
    );
    assert!(
        first.contains("USING INDEX session_recency_keyset") && !first.contains("TEMP B-TREE"),
        "first page plan: {first}"
    );
    let next = plan_for(
        &connection,
        SESSIONS_ACTIVE_PAGE_NEXT_SQL,
        &[
            Value::Integer(0),
            Value::Integer(256),
            Value::Integer(5_000),
            Value::Text("shared".into()),
            Value::Text("native".into()),
            Value::Text("codex".into()),
        ],
    );
    assert!(
        next.contains("USING INDEX session_recency_keyset") && !next.contains("TEMP B-TREE"),
        "next page plan: {next}"
    );
    assert!(
        SESSIONS_ACTIVE_PAGE_FIRST_SQL.contains("LIMIT ?2")
            && SESSIONS_ACTIVE_PAGE_NEXT_SQL.contains("LIMIT ?2"),
        "every page statement carries a LIMIT"
    );
}

#[test]
fn presence_lookup_query_plan_searches_the_primary_key() {
    use rusqlite::types::Value;
    let store = store();
    let connection = store.lock();
    let plan = plan_for(
        &connection,
        &session_presence_for_keys_sql(2),
        &[
            Value::Text("native".into()),
            Value::Text("claude-code".into()),
            Value::Text("one".into()),
            Value::Text("wsl:ubuntu".into()),
            Value::Text("codex".into()),
            Value::Text("two".into()),
        ],
    );
    assert!(
        plan.contains("sqlite_autoindex_session_1") && !plan.contains("SCAN session"),
        "presence lookup plan: {plan}"
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
fn session_keys_for_activity_keys_returns_identities_without_records() {
    let store = store();
    let native = session("identity-native", 4_000);
    let mut wsl = native.clone();
    wsl.key.environment_key = "wsl:Ubuntu".into();
    wsl.key.session_id = "identity-wsl".into();
    wsl.wsl_distro = Some("Ubuntu".into());
    store
        .upsert_sessions(
            &[native.clone(), wsl.clone()],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    let native_key = SessionActivityKey::new("native", "claude-code", &native.source_label);
    let wsl_key = SessionActivityKey::new("wsl:Ubuntu", "claude-code", &wsl.source_label);
    let missing = SessionActivityKey::new("native", "codex", "/nowhere/unknown.jsonl");

    let (identities, revision) = store
        .session_keys_for_activity_keys(&[native_key.clone(), wsl_key.clone(), missing.clone()])
        .unwrap();

    assert_eq!(identities.len(), 2, "an unknown activity key finds nothing");
    assert_eq!(
        identities.get(&native_key),
        Some(&(native.key.clone(), Incarnation(1)))
    );
    assert_eq!(
        identities.get(&wsl_key),
        Some(&(wsl.key.clone(), Incarnation(2)))
    );
    assert!(!identities.contains_key(&missing));
    assert_eq!(
        revision,
        store.revision(),
        "the lookup reads its revision with the rows"
    );
}

#[test]
fn session_keys_for_activity_keys_cross_the_query_chunk_boundary() {
    let store = store();
    let mut records = Vec::new();
    let mut keys = Vec::new();
    for index in 0..=SCAN_HISTORY_KEY_BATCH_SIZE {
        let mut record = session(&format!("chunk-{index:03}"), 4_000 + index as i64);
        record.source_label = format!("/chunk/{index:03}.jsonl");
        keys.push(SessionActivityKey::new(
            "native",
            "claude-code",
            &record.source_label,
        ));
        records.push(record);
    }
    store
        .upsert_sessions(&records, &crate::agents::evidence_cohort())
        .unwrap();

    let (identities, _revision) = store.session_keys_for_activity_keys(&keys).unwrap();

    assert_eq!(identities.len(), SCAN_HISTORY_KEY_BATCH_SIZE + 1);
    assert_eq!(
        identities.get(&keys[0]).map(|(key, _)| key),
        Some(&SessionKey::new("native", "claude-code", "chunk-000"))
    );
    assert_eq!(
        identities
            .get(&keys[SCAN_HISTORY_KEY_BATCH_SIZE])
            .map(|(key, _)| key),
        Some(&records[SCAN_HISTORY_KEY_BATCH_SIZE].key)
    );
}

#[test]
fn identity_only_activity_lookup_query_plan_uses_the_source_index() {
    let store = store();
    let connection = store.lock();
    let sql = session_keys_for_activity_keys_sql(2);
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
