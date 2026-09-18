use super::*;

/// Seed a session and write its evidence row with a chosen status and blob.
/// `upsert_sessions` creates the evidence row, so this sets only the two
/// columns `Store::quota_incidents` reads.
fn seed_quota_evidence(
    store: &Store,
    session_id: &str,
    updated_at: i64,
    status: &str,
    evidence_json: &str,
) -> SessionRecord {
    let record = session(session_id, updated_at);
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session_evidence
                SET status = ?4, evidence_json = ?5
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id,
                status,
                evidence_json
            ],
        )
        .unwrap();
    record
}

/// One incident, as the engine serializes it. The timestamp identifies which
/// session a returned row came from.
fn quota_incidents_json(ts_ms: i64) -> String {
    format!(
        r#"[{{"tsMs":{ts_ms},"limitKind":"usage_limit","severity":"hard_hit","model":null,"resetTsMs":null,"resetClock":null,"utilizationPct":null,"confidence":"observed"}}]"#
    )
}

/// A complete evidence blob. The incidents sit directly under `value`.
fn complete_quota_evidence(incidents: &str) -> String {
    format!(
        r#"{{"schemaRevision":21,"quotaIncidents":{{"state":"complete","value":{{"incidents":{incidents}}}}}}}"#
    )
}

/// A partial evidence blob. The incidents sit under `value.observed`.
fn partial_quota_evidence(incidents: &str) -> String {
    format!(
        r#"{{"schemaRevision":21,"quotaIncidents":{{"state":"partial","value":{{"observed":{{"incidents":{incidents}}},"reason":"cap_exceeded"}}}}}}"#
    )
}

/// Every returned incident timestamp, sorted. The query states no row order.
fn incident_timestamps(records: &[QuotaIncidentRecord]) -> Vec<i64> {
    let mut stamps: Vec<i64> = records
        .iter()
        .flat_map(|record| {
            serde_json::from_str::<serde_json::Value>(&record.incidents_json)
                .expect("incidents json")
                .as_array()
                .expect("incidents array")
                .iter()
                .map(|incident| incident["tsMs"].as_i64().expect("tsMs"))
                .collect::<Vec<_>>()
        })
        .collect();
    stamps.sort_unstable();
    stamps
}

#[test]
fn quota_incidents_read_the_complete_and_the_partial_evidence_shape() {
    let store = store();
    seed_quota_evidence(
        &store,
        "complete",
        2_000,
        "ready",
        &complete_quota_evidence(&quota_incidents_json(11)),
    );
    seed_quota_evidence(
        &store,
        "partial",
        1_900,
        "ready",
        &partial_quota_evidence(&quota_incidents_json(22)),
    );

    let records = store.quota_incidents(1_000).unwrap();
    assert_eq!(records.len(), 2);
    // The partial blob nests its incidents one level deeper. Both shapes
    // must come back.
    assert_eq!(incident_timestamps(&records), vec![11, 22]);
    assert!(records.iter().all(|record| record.agent == "claude-code"));
    // No account observation reaches these sessions, so the projection is
    // an empty array rather than NULL.
    assert!(
        records
            .iter()
            .all(|record| record.provider_accounts_json == "[]")
    );
}

#[test]
fn quota_incidents_keep_only_ready_evidence_that_holds_an_incident() {
    let store = store();
    seed_quota_evidence(
        &store,
        "ready",
        2_000,
        "ready",
        &complete_quota_evidence(&quota_incidents_json(11)),
    );
    // Ready, but the session observed no refusal.
    seed_quota_evidence(
        &store,
        "no-incidents",
        2_000,
        "ready",
        &complete_quota_evidence("[]"),
    );
    // A blob from an agent the evidence pass does not read.
    seed_quota_evidence(
        &store,
        "unsupported",
        2_000,
        "unsupported",
        &complete_quota_evidence(&quota_incidents_json(33)),
    );
    // A blob the pass has not published yet.
    seed_quota_evidence(
        &store,
        "pending",
        2_000,
        "pending",
        &complete_quota_evidence(&quota_incidents_json(44)),
    );
    // A blob the pass gave up on.
    seed_quota_evidence(
        &store,
        "failed",
        2_000,
        "failed",
        &complete_quota_evidence(&quota_incidents_json(55)),
    );

    let records = store.quota_incidents(1_000).unwrap();
    // A record for a session that observed nothing carries no information,
    // so the query returns no row rather than an empty array.
    assert_eq!(records.len(), 1);
    assert_eq!(incident_timestamps(&records), vec![11]);
}

#[test]
fn quota_incidents_filter_on_an_inclusive_since_epoch() {
    let store = store();
    seed_quota_evidence(
        &store,
        "newer",
        2_000,
        "ready",
        &complete_quota_evidence(&quota_incidents_json(11)),
    );
    seed_quota_evidence(
        &store,
        "older",
        1_500,
        "ready",
        &complete_quota_evidence(&quota_incidents_json(22)),
    );

    assert_eq!(
        incident_timestamps(&store.quota_incidents(1_000).unwrap()),
        vec![11, 22]
    );
    // The bound is inclusive and excludes everything below it.
    assert_eq!(
        incident_timestamps(&store.quota_incidents(1_500).unwrap()),
        vec![11, 22]
    );
    assert_eq!(
        incident_timestamps(&store.quota_incidents(2_000).unwrap()),
        vec![11]
    );
    assert!(store.quota_incidents(2_001).unwrap().is_empty());
}

#[test]
fn quota_incidents_project_the_accounts_the_session_used() {
    let store = store();
    store.set_internal_value("internal:providerAccountRolloutV1", "1000");
    seed_quota_evidence(
        &store,
        "attributed",
        2_000,
        "ready",
        &complete_quota_evidence(&quota_incidents_json(11)),
    );
    // Outside the observation's activity window, so it keeps no account.
    seed_quota_evidence(
        &store,
        "unattributed",
        1_100,
        "ready",
        &complete_quota_evidence(&quota_incidents_json(22)),
    );
    let account_key = "a".repeat(64);
    store
        .observe_provider_account(
            "claude-code",
            "anthropic",
            &account_key,
            2_025,
            "tool_oauth",
        )
        .unwrap();

    let records = store.quota_incidents(1_000).unwrap();
    assert_eq!(records.len(), 2);
    let accounts: std::collections::BTreeMap<i64, serde_json::Value> = records
        .iter()
        .map(|record| {
            (
                incident_timestamps(std::slice::from_ref(record))[0],
                serde_json::from_str(&record.provider_accounts_json).expect("accounts json"),
            )
        })
        .collect();
    assert_eq!(
        accounts[&11],
        serde_json::json!([{"provider": "anthropic", "accountKey": account_key}])
    );
    assert_eq!(accounts[&22], serde_json::json!([]));
}
