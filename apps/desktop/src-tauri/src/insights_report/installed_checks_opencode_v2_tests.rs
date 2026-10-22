use super::*;
use crate::store::SessionRecord;

fn v2_session(store: &Store, id: &str) -> SessionRecord {
    let label = format!("opencode:{id}:open_code_v2_sqlite");
    store.lock().execute(
        "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
          cwd, updated_at_epoch, started_at_epoch, first_seen_at, last_seen_at, incarnation)
         VALUES ('native', 'opencode', ?1, 'providerDb', ?2, '/workspace/demo', 100, 100, '', '', 7)",
        params![id, label],
    ).unwrap();
    SessionRecord {
        key: SessionKey::new("native", "opencode", id),
        source_kind: "providerDb".into(),
        source_label: label,
        wsl_distro: None,
        title: None,
        title_source: None,
        cwd: Some("/workspace/demo".into()),
        surface: "cli".into(),
        updated_at_epoch: Some(100),
        activity_cursor: String::new(),
        activity_source: "event".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: None,
    }
}

fn capture(store: &Store, gap: bool) {
    store
        .lock()
        .execute(
            "INSERT INTO opencode_v2_capture
          (environment_key, agent, session_id, incarnation, source_generation,
           source_label, cwd, capture_epoch, claim_fence, history_gap, lease_expires_at_epoch)
         SELECT environment_key, agent, session_id, incarnation, source_generation,
           source_label, cwd, 'epoch', 1, ?1, 260 FROM session WHERE agent = 'opencode'",
            [gap],
        )
        .unwrap();
}

#[test]
fn v2_blocked_sources_group_typed_safe_diagnostics_without_waiting_or_pass() {
    for (status, code) in [
        (
            OpenCodeV2SourceStatus::ServiceUnavailable,
            "service-unavailable",
        ),
        (
            OpenCodeV2SourceStatus::AuthenticationRejected,
            "authentication-rejected",
        ),
        (OpenCodeV2SourceStatus::SourceMismatch, "source-mismatch"),
        (
            OpenCodeV2SourceStatus::VersionUnsupported,
            "version-unsupported",
        ),
        (OpenCodeV2SourceStatus::CaptureGap, "capture-gap"),
        (OpenCodeV2SourceStatus::SourceLimit, "source-limit"),
    ] {
        let store = store();
        let definition = definition("/workspace/demo");
        activate(&store, &definition);
        let records = [v2_session(&store, "first"), v2_session(&store, "second")];
        store
            .set_opencode_v2_source_status(&records, status, 100)
            .unwrap();
        store.lock().execute(
            "INSERT INTO session_evidence (environment_key, agent, session_id, status,
              analyzed_generation, parser_revision, analyzer_revision, evidence_schema_revision, last_error)
             SELECT environment_key, agent, session_id, 'ready', source_generation, ?1, ?2, ?3,
               'RAW_CREDENTIAL_CANARY' FROM session",
            params![PARSER_REVISION, ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION],
        ).unwrap();
        let report = rows(&store, &[definition]);
        let row = &report[0];
        assert_eq!(
            (row.finding, row.clean, row.unavailable),
            (0, 0, 2),
            "{status:?}"
        );
        assert_eq!(
            (row.waiting_for_data_count, row.waiting_for_results_count),
            (0, 0)
        );
        assert!(row.waiting_for_events.is_none());
        assert_eq!(row.diagnostics.len(), 1);
        let diagnostic = &row.diagnostics[0];
        assert_eq!(diagnostic.code, format!("opencode-v2-{code}"));
        assert_eq!(diagnostic.severity, InstalledCheckDiagnosticSeverity::Error);
        assert_eq!(diagnostic.affected_session_count, 2);
        assert!(diagnostic.message.contains("Stored-session analysis"));
        assert!(diagnostic.source.is_none());
        let json = serde_json::to_string(diagnostic).unwrap();
        assert!(!json.contains("RAW_CREDENTIAL_CANARY"));
        assert!(!json.contains("/workspace/demo"));
        assert!(!json.contains("first"));
    }
}

#[test]
fn v2_exact_lease_is_ready_despite_metric_failure_and_appends() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    let record = v2_session(&store, "main");
    store
        .set_opencode_v2_source_status(&[record], OpenCodeV2SourceStatus::SourceMismatch, 100)
        .unwrap();
    capture(&store, false);
    store
        .lock()
        .execute(
            "INSERT INTO session_evidence (environment_key, agent, session_id, status, last_error)
         VALUES ('native', 'opencode', 'main', 'failed', 'METRIC_ERROR_CANARY')",
            [],
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1",
            [],
        )
        .unwrap();
    let report = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(report[0].waiting_for_results_count, 1);
    assert_eq!(report[0].waiting_for_data_count, 0);
    assert!(!report[0].waiting_for_events.unwrap().catching_up);
    assert!(report[0].diagnostics.is_empty());
    assert_eq!((report[0].finding, report[0].clean), (0, 0));

    store
        .lock()
        .execute("UPDATE opencode_v2_capture SET history_gap = 1", [])
        .unwrap();
    let report = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(report[0].diagnostics[0].code, "opencode-v2-capture-gap");
    assert_eq!(
        report[0].diagnostics[0].severity,
        InstalledCheckDiagnosticSeverity::Info
    );
    assert_eq!(report[0].waiting_for_results_count, 1);

    store
        .lock()
        .execute(
            "UPDATE opencode_v2_capture SET lease_expires_at_epoch = 200",
            [],
        )
        .unwrap();
    let report = rows(&store, &[definition]);
    assert_eq!(report[0].diagnostics[0].code, "opencode-v2-source-mismatch");
    assert_eq!(report[0].waiting_for_results_count, 0);
    assert_eq!(report[0].waiting_for_data_count, 0);
    assert!(report[0].waiting_for_events.is_none());
}

#[test]
fn v2_source_gates_reject_stale_scope_and_non_main_capture_receipts() {
    for mutation in [
        "UPDATE opencode_v2_capture SET incarnation = 6",
        "UPDATE opencode_v2_capture SET source_label = 'opencode:main'",
        "UPDATE opencode_v2_capture SET cwd = '/workspace/other'",
        "UPDATE session SET wsl_distro = 'Synthetic'",
        "INSERT INTO session_relation VALUES ('native', 'opencode', 'main', 'forkParent', 'parent', NULL)",
        "INSERT INTO session_relation VALUES ('native', 'opencode', 'parent', 'subagent', 'main', NULL)",
    ] {
        let store = store();
        v2_session(&store, "main");
        capture(&store, false);
        store.lock().execute(mutation, []).unwrap();
        let ready = source_ready_at_sql(1, 2, 3, "200");
        let can_arrive = source_can_arrive_sql(1, 4);
        let gates: (bool, bool) = store
            .lock()
            .query_row(
                &format!(
                    "SELECT ({ready}), ({can_arrive}) FROM session s
              LEFT JOIN session_evidence e USING(environment_key, agent, session_id)"
                ),
                params![
                    PARSER_REVISION,
                    ANALYZER_REVISION,
                    EVIDENCE_SCHEMA_REVISION,
                    20
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(gates, (false, false), "{mutation}");
    }
}

fn accepted_v2_session(
    store: &Store,
    definition: &InstalledCheckDefinition,
    id: &str,
    position: i64,
    status: StatusResult,
) -> SessionRecord {
    accept(store, definition, id, position, Some(status));
    let record = v2_session(store, id);
    store
        .lock()
        .execute(
            "UPDATE session SET cwd = '/workspace/other' WHERE agent = 'claude-code'",
            [],
        )
        .unwrap();
    store.lock().execute(
        "UPDATE burn_check_candidate SET session_key_json = json_array('native', 'opencode', ?1)
         WHERE branch_key = ?1", [id],
    ).unwrap();
    record
}

#[test]
fn v2_blocked_live_capture_keeps_findings_and_assessed_input_but_hides_saved_pass() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    let records = [
        accepted_v2_session(&store, &definition, "finding", 1, StatusResult::Failure),
        accepted_v2_session(&store, &definition, "clean", 2, StatusResult::Success),
    ];
    store.lock().execute("UPDATE burn_check_candidate SET preparation_json = ?1 WHERE branch_key = 'finding'", [json!({
        "status":"ready", "profile":"fixture", "input":{"used":"ASSESSED_INPUT"}, "request":null,
        "report":{"omissions":[{"path":"activity", "strategy":"recent", "original_count":4, "retained_count":2}]}
    }).to_string()]).unwrap();
    let saved: String = store.lock().query_row("SELECT json_group_array(json_array(assessment_id, actions_json, evidence_valid)) FROM burn_check_assessment_history", [], |row| row.get(0)).unwrap();
    store
        .set_opencode_v2_source_status(
            &records,
            OpenCodeV2SourceStatus::AuthenticationRejected,
            150,
        )
        .unwrap();
    let report = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(
        (report[0].finding, report[0].clean, report[0].unavailable),
        (1, 0, 1)
    );
    assert_eq!(
        (
            report[0].waiting_for_data_count,
            report[0].waiting_for_results_count
        ),
        (0, 0)
    );
    let evidence = read_evidence(
        &store,
        std::slice::from_ref(&definition),
        &request(),
        &report[0].targets[0].id,
    )
    .unwrap();
    assert_eq!(evidence.status, BurnCheckEvidenceStatus::Available);
    assert_eq!(evidence.items[0].source_label, "Assessed input");
    assert!(evidence.items[0].excerpt.contains("ASSESSED_INPUT"));
    assert_eq!(evidence.items[1].source_label, "Input omissions");
    capture(&store, false);
    let ready = rows(&store, &[definition]);
    assert_eq!((ready[0].finding, ready[0].clean), (1, 1));
    let after: String = store.lock().query_row("SELECT json_group_array(json_array(assessment_id, actions_json, evidence_valid)) FROM burn_check_assessment_history", [], |row| row.get(0)).unwrap();
    assert_eq!(saved, after);
}

#[test]
fn v2_live_capture_gap_keeps_a_saved_pass_and_reports_its_limit() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    accepted_v2_session(&store, &definition, "clean", 1, StatusResult::Success);
    capture(&store, true);

    let report = rows(&store, &[definition]);

    assert_eq!(
        (report[0].finding, report[0].clean, report[0].unavailable),
        (0, 1, 0)
    );
    assert!(report[0].has_started);
    assert_eq!(report[0].diagnostics.len(), 1);
    assert_eq!(report[0].diagnostics[0].code, "opencode-v2-capture-gap");
    assert_eq!(
        report[0].diagnostics[0].severity,
        InstalledCheckDiagnosticSeverity::Info
    );
    assert!(
        report[0].diagnostics[0]
            .message
            .contains("Earlier assessments cover only their saved input")
    );
}

#[test]
fn v2_missing_capture_and_expired_capture_do_not_wait_for_metric_work() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    v2_session(&store, "main");
    let missing = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(
        missing[0].diagnostics[0].code,
        "opencode-v2-service-unavailable"
    );
    assert!(missing[0].waiting_for_events.is_none());
    capture(&store, false);
    store
        .lock()
        .execute(
            "UPDATE opencode_v2_capture SET lease_expires_at_epoch = 200",
            [],
        )
        .unwrap();
    let expired = rows(&store, &[definition]);
    assert_eq!(expired[0].diagnostics[0].code, "opencode-v2-capture-gap");
    assert_eq!(expired[0].waiting_for_data_count, 0);
    assert!(expired[0].waiting_for_events.is_none());
}
