use super::*;
use crate::store::SessionKey;

fn setup() -> (tempfile::TempDir, Store, SessionRecord) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    let record = SessionRecord {
        key: SessionKey::new("native", "opencode", "main"),
        source_kind: "providerDb".into(),
        source_label: "opencode:main:open_code_v2_sqlite".into(),
        wsl_distro: None,
        title: None,
        title_source: None,
        cwd: Some("/workspace/demo".into()),
        surface: "cli".into(),
        updated_at_epoch: Some(100),
        activity_cursor: "fixture".into(),
        activity_source: "event".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: Some("synthetic-source".into()),
    };
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    (directory, store, record)
}

fn current_status(store: &Store, now: i64) -> String {
    store
        .lock()
        .query_row(
            &format!("SELECT ({}) FROM session s", source_status_sql("?1")),
            [now],
            |row| row.get(0),
        )
        .unwrap()
}

fn lease(store: &Store, expires: i64) {
    store
        .lock()
        .execute(
            "INSERT INTO opencode_v2_capture
          (environment_key, agent, session_id, incarnation, source_generation,
           source_label, cwd, capture_epoch, claim_fence, history_gap, lease_expires_at_epoch)
         SELECT environment_key, agent, session_id, incarnation, source_generation,
           source_label, cwd, 'epoch', 1, 0, ?1 FROM session",
            [expires],
        )
        .unwrap();
}

#[test]
fn closed_status_codes_round_trip_and_reject_arbitrary_error_text() {
    let (_dir, store, record) = setup();
    for status in [
        OpenCodeV2SourceStatus::ServiceUnavailable,
        OpenCodeV2SourceStatus::AuthenticationRejected,
        OpenCodeV2SourceStatus::SourceMismatch,
        OpenCodeV2SourceStatus::VersionUnsupported,
        OpenCodeV2SourceStatus::CaptureGap,
        OpenCodeV2SourceStatus::SourceLimit,
        OpenCodeV2SourceStatus::Live,
    ] {
        assert_eq!(
            OpenCodeV2SourceStatus::from_code(status.code()).unwrap(),
            status
        );
        assert_eq!(
            store
                .set_opencode_v2_source_status(std::slice::from_ref(&record), status, 100)
                .unwrap(),
            1
        );
        let code: String = store
            .lock()
            .query_row("SELECT code FROM opencode_v2_source_status", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(code, status.code());
    }
    assert!(OpenCodeV2SourceStatus::from_code("PRIVATE_PATH_CANARY").is_err());
    assert!(
        store
            .lock()
            .execute(
                "UPDATE opencode_v2_source_status SET code = 'CREDENTIAL_CANARY'",
                []
            )
            .is_err()
    );
    assert_eq!(
        current_status(&store, 101),
        "service-unavailable",
        "Live is not proof of a lease"
    );
}

#[test]
fn source_status_is_monotonic_across_appends_and_rejects_changed_cwds() {
    let (_dir, store, record) = setup();
    store
        .set_opencode_v2_source_status(
            std::slice::from_ref(&record),
            OpenCodeV2SourceStatus::AuthenticationRejected,
            101,
        )
        .unwrap();
    assert_eq!(
        store
            .set_opencode_v2_source_status(
                std::slice::from_ref(&record),
                OpenCodeV2SourceStatus::ServiceUnavailable,
                100
            )
            .unwrap(),
        0
    );
    assert_eq!(current_status(&store, 102), "authentication-rejected");
    store.lock().execute("UPDATE session SET source_fingerprint = 'changed', source_generation = source_generation + 1", []).unwrap();
    assert_eq!(
        store
            .set_opencode_v2_source_status(
                std::slice::from_ref(&record),
                OpenCodeV2SourceStatus::SourceLimit,
                103
            )
            .unwrap(),
        1
    );
    assert_eq!(current_status(&store, 103), "source-limit");
    store
        .lock()
        .execute("UPDATE session SET cwd = '/workspace/other'", [])
        .unwrap();
    assert_eq!(
        store
            .set_opencode_v2_source_status(&[record], OpenCodeV2SourceStatus::SourceMismatch, 104)
            .unwrap(),
        0
    );
    assert_eq!(current_status(&store, 104), "service-unavailable");
}

#[test]
fn source_status_reads_require_every_session_fence() {
    for change in ["incarnation = incarnation + 1", "cwd = '/workspace/other'"] {
        let (_dir, store, record) = setup();
        store
            .set_opencode_v2_source_status(&[record], OpenCodeV2SourceStatus::SourceMismatch, 100)
            .unwrap();
        store
            .lock()
            .execute(&format!("UPDATE session SET {change}"), [])
            .unwrap();
        assert_eq!(
            current_status(&store, 101),
            "service-unavailable",
            "{change}"
        );
    }
    for change in [
        "source_label = 'opencode:main'",
        "wsl_distro = 'Synthetic'",
        "source_kind = 'file'",
    ] {
        let (_dir, store, record) = setup();
        store
            .set_opencode_v2_source_status(&[record], OpenCodeV2SourceStatus::SourceMismatch, 100)
            .unwrap();
        store
            .lock()
            .execute(&format!("UPDATE session SET {change}"), [])
            .unwrap();
        let code: Option<String> = store
            .lock()
            .query_row(
                &format!("SELECT ({}) FROM session s", source_status_sql("101")),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(code.is_none(), "{change}");
    }
}

#[test]
fn exact_live_lease_overrides_failures_but_not_capture_history_limits() {
    let (_dir, store, record) = setup();
    store
        .set_opencode_v2_source_status(
            &[record],
            OpenCodeV2SourceStatus::AuthenticationRejected,
            100,
        )
        .unwrap();
    lease(&store, 160);
    assert_eq!(current_status(&store, 159), "live");
    store
        .lock()
        .execute("UPDATE opencode_v2_capture SET history_gap = 1", [])
        .unwrap();
    assert_eq!(current_status(&store, 159), "capture-gap");
    assert_eq!(current_status(&store, 160), "authentication-rejected");
    store
        .lock()
        .execute("UPDATE opencode_v2_source_status SET code = 'live'", [])
        .unwrap();
    assert_eq!(current_status(&store, 160), "capture-gap");
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1",
            [],
        )
        .unwrap();
    assert_eq!(
        current_status(&store, 159),
        "capture-gap",
        "appends keep active receipts"
    );
    store
        .lock()
        .execute(
            "UPDATE opencode_v2_capture SET incarnation = incarnation + 1",
            [],
        )
        .unwrap();
    assert_eq!(current_status(&store, 159), "service-unavailable");
}

#[test]
fn source_status_batch_is_bounded_and_deletion_cascades() {
    let (_dir, store, record) = setup();
    assert!(
        store
            .set_opencode_v2_source_status(
                &vec![record.clone(); MAX_STATUS_RECORDS + 1],
                OpenCodeV2SourceStatus::ServiceUnavailable,
                100
            )
            .is_err()
    );
    let mut invalid = record.clone();
    invalid.source_label = "opencode:main".into();
    assert!(
        store
            .set_opencode_v2_source_status(
                &[record.clone(), invalid],
                OpenCodeV2SourceStatus::SourceLimit,
                100
            )
            .is_err()
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM opencode_v2_source_status",
                [],
                |row| row.get::<_, usize>(0)
            )
            .unwrap(),
        0
    );
    store
        .set_opencode_v2_source_status(
            std::slice::from_ref(&record),
            OpenCodeV2SourceStatus::SourceLimit,
            100,
        )
        .unwrap();
    store.delete_session(&record.key).unwrap();
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM opencode_v2_source_status",
                [],
                |row| row.get::<_, usize>(0)
            )
            .unwrap(),
        0
    );
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .set_opencode_v2_source_status(&[record], OpenCodeV2SourceStatus::SourceLimit, 101)
        .unwrap();
    store.clear_local_session_data().unwrap();
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM opencode_v2_source_status",
                [],
                |row| row.get::<_, usize>(0)
            )
            .unwrap(),
        0
    );
}
