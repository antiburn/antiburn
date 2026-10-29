use super::*;
use crate::store::SessionRecord;
use crate::store::native_tool_results::source_delivery::opencode_v2::{
    OpenCodeV2AdmittedSource, OpenCodeV2CaptureStart,
};

pub(super) fn v2_session(store: &Store, id: &str) -> SessionRecord {
    let label = format!("opencode:{id}:open_code_v2_sqlite");
    store.lock().execute(
        "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
          cwd, updated_at_epoch, started_at_epoch, first_seen_at, last_seen_at, incarnation, source_format)
         VALUES ('native', 'opencode', ?1, 'providerDb', ?2, '/workspace/demo', 100, 100, '', '', 7, ?3)",
        params![id, label, serde_json::to_string(&antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2).unwrap()],
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
        let report = rows(&store, std::slice::from_ref(&definition));
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
        assert_eq!(row.checking_count, 0);
        assert!(!row.has_started);
        assert_eq!(row.diagnostics.len(), 1);
        let diagnostic = &row.diagnostics[0];
        assert_eq!(diagnostic.code, format!("opencode-v2-{code}"));
        assert_eq!(diagnostic.severity, InstalledCheckDiagnosticSeverity::Error);
        assert_eq!(diagnostic.affected_session_count, 2);
        assert!(!diagnostic.title.contains("V2"));
        assert!(!diagnostic.message.contains("V2"));
        assert!(!diagnostic.title.contains("missed activity"));
        assert!(diagnostic.details.iter().any(|detail| {
            detail.label == "Stored analysis" && detail.value.contains("Stored-session analysis")
        }));
        assert!(diagnostic.source.is_none());
        let json = serde_json::to_string(diagnostic).unwrap();
        assert!(!json.contains("RAW_CREDENTIAL_CANARY"));
        assert!(!json.contains("/workspace/demo"));
        assert!(!json.contains("first"));

        capture(&store, true);
        let recovered = rows(&store, &[definition]);
        assert!(recovered[0].diagnostics.is_empty(), "{status:?}");
        assert_eq!(recovered[0].waiting_for_results_count, 2);
    }
}

#[test]
fn blocked_v2_candidates_do_not_start_checks_or_hide_healthy_results() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    queue(&store, &definition, "blocked", 1);
    let blocked = v2_session(&store, "blocked");
    store
        .lock()
        .execute(
            "UPDATE session SET cwd = '/workspace/other' WHERE agent = 'claude-code'",
            [],
        )
        .unwrap();
    store.lock().execute("UPDATE burn_check_candidate SET session_key_json = json_array('native', 'opencode', 'blocked')", []).unwrap();
    store
        .set_opencode_v2_source_status(&[blocked], OpenCodeV2SourceStatus::ServiceUnavailable, 100)
        .unwrap();
    let report = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(report[0].checking_count, 0);
    assert!(!report[0].has_started);

    accept(
        &store,
        &definition,
        "healthy",
        2,
        Some(StatusResult::Success),
    );
    queue(&store, &definition, "healthy", 3);
    let report = rows(&store, &[definition]);
    assert_eq!(report[0].clean, 1);
    assert_eq!(report[0].checking_count, 1);
    assert!(report[0].has_started);
    assert!(
        report[0]
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "opencode-v2-service-unavailable")
    );
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
    assert!(report[0].diagnostics.is_empty());
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
fn source_readiness_uses_typed_format_instead_of_a_v1_shaped_label() {
    let store = store();
    v2_session(&store, "main");
    capture(&store, false);
    store
        .lock()
        .execute_batch(
            "UPDATE session SET source_label = 'opencode:main';
             UPDATE opencode_v2_capture SET source_label = 'opencode:main';",
        )
        .unwrap();
    let ready = source_ready_at_sql(1, 2, 3, "200");
    let source_ready = || {
        store
            .lock()
            .query_row(
                &format!(
                    "SELECT ({ready}) FROM session s
                     LEFT JOIN session_evidence e USING(environment_key, agent, session_id)"
                ),
                params![PARSER_REVISION, ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    };
    assert!(source_ready());
    store
        .lock()
        .execute(
            "UPDATE session SET source_format = ?1",
            [
                serde_json::to_string(&antiburn_local::analysis::SourceFormat::OpenCodeSqliteV1)
                    .unwrap(),
            ],
        )
        .unwrap();
    assert!(!source_ready());
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
fn v2_live_capture_history_gap_keeps_a_saved_pass_without_an_alert() {
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
    assert!(report[0].diagnostics.is_empty());
}

#[test]
fn v2_mixed_live_history_and_current_failures_report_only_current_unavailability() {
    let store = store();
    let mut definition = definition("/workspace/demo");
    definition
        .diagnostics
        .push(crate::installed_checks::diagnostics::payload(
            "provider-authentication",
            InstalledCheckDiagnosticSeverity::Error,
            "Reconnect the check provider",
            "Update the selected connection in Settings → Checks.",
            None,
            Vec::new(),
        ));
    activate(&store, &definition);
    v2_session(&store, "live");
    v2_session(&store, "live-history-gap");
    v2_session(&store, "current-gap");
    let unavailable = v2_session(&store, "unavailable");
    store
        .set_opencode_v2_source_status(
            &[unavailable],
            OpenCodeV2SourceStatus::ServiceUnavailable,
            100,
        )
        .unwrap();
    capture(&store, true);
    store
        .lock()
        .execute_batch(
            "UPDATE opencode_v2_capture SET history_gap = 0 WHERE session_id = 'live';
         UPDATE opencode_v2_capture SET lease_expires_at_epoch = NULL
           WHERE session_id IN ('current-gap', 'unavailable');",
        )
        .unwrap();

    let report = rows(&store, &[definition]);
    let row = &report[0];
    assert_eq!(row.waiting_for_results_count, 2);
    assert_eq!(row.waiting_for_data_count, 0);
    assert_eq!(row.diagnostics.len(), 3);
    for code in ["opencode-v2-capture-gap", "opencode-v2-service-unavailable"] {
        let diagnostic = row
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == code)
            .unwrap();
        assert_eq!(diagnostic.severity, InstalledCheckDiagnosticSeverity::Error);
        assert_eq!(diagnostic.affected_session_count, 1);
    }
    assert!(
        row.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "provider-authentication")
    );
}

#[test]
fn v2_first_attach_and_recovered_capture_keep_typed_gaps_without_an_alert() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    let record = v2_session(&store, "main");
    let admitted_source = OpenCodeV2AdmittedSource {
        database_path: Path::new(env!("CARGO_MANIFEST_DIR")).join("synthetic/opencode.db"),
        file_identity: "synthetic-database-identity".into(),
    };
    let cursor = format!(
        "opencode-admission-v2:{}",
        serde_json::to_string(&(
            &admitted_source.database_path,
            &record.key.session_id,
            &admitted_source.file_identity,
            "synthetic-scope",
        ))
        .unwrap()
    );
    store
        .lock()
        .execute("UPDATE session SET activity_cursor = ?1", [cursor])
        .unwrap();
    let start = OpenCodeV2CaptureStart {
        key: record.key,
        incarnation: crate::store::Incarnation(7),
        source_generation: 0,
        source_label: record.source_label,
        cwd: "/workspace/demo".into(),
        admitted_source,
        baseline_sequence: Some(4),
    };
    let first = store
        .start_opencode_v2_capture_epoch(std::slice::from_ref(&start), "first", 100, 160)
        .unwrap()
        .unwrap()
        .remove(0);
    assert!(first.history_gap);
    let attached = rows(&store, std::slice::from_ref(&definition));
    assert!(attached[0].diagnostics.is_empty());
    assert_eq!(
        attached[0].waiting_for_results_count + attached[0].waiting_for_data_count,
        1
    );

    assert_eq!(store.recover_opencode_v2_capture_claims(150).unwrap(), 1);
    let stopped = rows(&store, std::slice::from_ref(&definition));
    assert_eq!(stopped[0].diagnostics.len(), 1);
    assert_eq!(stopped[0].diagnostics[0].code, "opencode-v2-capture-gap");
    let diagnostic = &stopped[0].diagnostics[0];
    assert_eq!(diagnostic.title, "OpenCode live checks are not running");
    for text in [
        "Live activity is not available for this session.",
        "Keep OpenCode open, then rescan sessions.",
        "Live checks cannot recover missed activity.",
        "Saved results cover only their recorded input.",
    ] {
        assert!(diagnostic.message.contains(text), "{text}");
    }
    assert_eq!(
        stopped[0].diagnostics[0].severity,
        InstalledCheckDiagnosticSeverity::Error
    );
    assert_eq!(stopped[0].waiting_for_results_count, 0);

    let next = store
        .start_opencode_v2_capture_epoch(std::slice::from_ref(&start), "recovered", 160, 100)
        .unwrap()
        .unwrap()
        .remove(0);
    assert!(next.history_gap);
    assert!(next.claim_fence > first.claim_fence);
    assert!(
        !store
            .renew_opencode_v2_capture_epoch(&[first], 170, 100)
            .unwrap()
    );
    let recovered = rows(&store, &[definition]);
    assert!(recovered[0].diagnostics.is_empty());
    assert_eq!(
        recovered[0].waiting_for_results_count + recovered[0].waiting_for_data_count,
        1
    );
    assert!(
        store
            .lock()
            .query_row("SELECT history_gap FROM opencode_v2_capture", [], |row| row
                .get::<_, bool>(0))
            .unwrap()
    );
    let publications = store.native_tool_result_publications_after(0, 20).unwrap();
    for reason in ["cold-attach", "capture-recovered"] {
        assert!(
            publications.iter().any(|publication| {
                publication.outcome
                    == crate::store::native_tool_results::ToolResultPublicationOutcome::CoverageGap
                    && publication.gap_reason.as_deref() == Some(reason)
            }),
            "{reason}"
        );
    }
}

#[test]
fn v2_healthy_history_gap_does_not_restore_invalidated_assessments() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    accepted_v2_session(&store, &definition, "finding", 1, StatusResult::Failure);
    let target = {
        capture(&store, true);
        rows(&store, std::slice::from_ref(&definition))[0].targets[0]
            .id
            .clone()
    };
    store
        .lock()
        .execute_batch(
            "UPDATE burn_check_assessment_history SET evidence_valid = 0;
         UPDATE burn_check_current_status SET evidence_valid = 0;",
        )
        .unwrap();

    let report = rows(&store, std::slice::from_ref(&definition));
    assert!(report[0].diagnostics.is_empty());
    assert_eq!((report[0].finding, report[0].clean), (0, 0));
    assert!(report[0].targets.is_empty());
    assert_eq!(
        read_evidence(&store, &[definition], &request(), &target)
            .unwrap()
            .status,
        BurnCheckEvidenceStatus::Unavailable
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

#[test]
fn inactive_v2_capture_errors_do_not_alert_or_add_unassessed_sessions() {
    for status in [
        OpenCodeV2SourceStatus::ServiceUnavailable,
        OpenCodeV2SourceStatus::AuthenticationRejected,
        OpenCodeV2SourceStatus::SourceMismatch,
        OpenCodeV2SourceStatus::VersionUnsupported,
        OpenCodeV2SourceStatus::CaptureGap,
        OpenCodeV2SourceStatus::SourceLimit,
    ] {
        let store = store();
        let definition = definition("/workspace/demo");
        activate(&store, &definition);
        let old = v2_session(&store, "inactive");
        store
            .set_opencode_v2_source_status(&[old], status, 100)
            .unwrap();
        capture(&store, true);
        let mut request = request();
        request.computed_at_epoch = 400;
        let changes = store.lock().total_changes();

        let report = read_installed_checks_in_window(&store, &[definition], &request).unwrap();

        assert!(report[0].diagnostics.is_empty(), "{status:?}");
        assert_eq!(
            (report[0].finding, report[0].clean, report[0].unavailable),
            (0, 0, 0)
        );
        assert_eq!(
            (
                report[0].checking_count,
                report[0].waiting_for_results_count
            ),
            (0, 0)
        );
        assert!(report[0].waiting_for_events.is_none());
        assert_eq!(store.lock().total_changes(), changes);
    }
}

#[test]
fn inactive_v2_errors_do_not_hide_a_live_session_or_a_current_failure() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    let records = [
        v2_session(&store, "inactive"),
        v2_session(&store, "live"),
        v2_session(&store, "current-failure"),
    ];
    store
        .set_opencode_v2_source_status(&records, OpenCodeV2SourceStatus::CaptureGap, 100)
        .unwrap();
    capture(&store, true);
    store
        .lock()
        .execute_batch(
            "UPDATE opencode_v2_capture SET lease_expires_at_epoch = 500 WHERE session_id = 'live';
         UPDATE session SET updated_at_epoch = 190 WHERE session_id = 'current-failure';",
        )
        .unwrap();
    let mut request = request();
    request.computed_at_epoch = 300;

    let report = read_installed_checks_in_window(&store, &[definition], &request).unwrap();

    assert_eq!(report[0].diagnostics.len(), 1);
    assert_eq!(report[0].diagnostics[0].code, "opencode-v2-capture-gap");
    assert_eq!(report[0].diagnostics[0].affected_session_count, 1);
    assert_eq!(report[0].waiting_for_results_count, 1);
    assert_eq!(
        (report[0].finding, report[0].clean, report[0].unavailable),
        (0, 0, 2)
    );
}

#[test]
fn inactive_v2_session_with_unfinished_work_keeps_its_capture_error() {
    for state in ["pending", "claimed", "dispatching", "answered"] {
        let store = store();
        let definition = definition("/workspace/demo");
        activate(&store, &definition);
        queue(&store, &definition, "unfinished", 1);
        let record = v2_session(&store, "unfinished");
        store.lock().execute_batch(
        "UPDATE session SET cwd = '/workspace/other' WHERE agent = 'claude-code';
         UPDATE burn_check_candidate SET session_key_json = json_array('native', 'opencode', 'unfinished');",
    ).unwrap();
        store
            .set_opencode_v2_source_status(&[record], OpenCodeV2SourceStatus::CaptureGap, 100)
            .unwrap();
        capture(&store, true);
        store
            .lock()
            .execute("UPDATE burn_check_candidate SET state = ?1", [state])
            .unwrap();
        let mut request = request();
        request.computed_at_epoch = 400;

        let report = read_installed_checks_in_window(&store, &[definition], &request).unwrap();

        assert_eq!(report[0].diagnostics.len(), 1);
        assert_eq!(report[0].diagnostics[0].affected_session_count, 1);
        assert_eq!(report[0].unavailable, 1);
        assert_eq!(report[0].checking_count, 0);
        assert_eq!(report[0].clean, 0);
    }
}

#[test]
fn real_idle_detach_publication_does_not_alert_before_or_after_consumption() {
    use crate::published_evidence_reader::StorePublishedEvidenceReader;
    use antiburn_checks::engine::CheckEngine;
    use antiburn_checks::source::{PublicationCursor, PublishedEvidenceSource, SourceEvent};
    use antiburn_local::analysis::{
        NativeEventContext, NativeEvidenceBatch, NativeEvidenceEvent, NativeEvidenceFact,
        NativeMessageObservation, NativeRequestAttribution, PublicMessageObservation,
        PublicMessagePayload, SourceFormat, ToolResultChange, ToolResultCompleteness,
        ToolResultPosition, UserMessagePart,
    };

    let (store, adapter, activation, mut definition) = local::local_fixture_with_source(
        "name: Idle detach report\non:\n  assistant_message:\n  cooldown: 0s\nactions:\n  - type: status\n    input: {result: failure}\n",
    );
    definition.trigger = Some(InstalledCheckTrigger {
        event: InstalledCheckEvent::AssistantMessage,
        minimum: 1,
    });
    let engine = CheckEngine::new(adapter.clone(), Arc::new(local::NoModel));
    let record = v2_session(&store, "idle-detach");
    let source = OpenCodeV2AdmittedSource {
        database_path: Path::new(env!("CARGO_MANIFEST_DIR")).join("synthetic/opencode.db"),
        file_identity: "synthetic-database-identity".into(),
    };
    let cursor = format!(
        "opencode-admission-v2:{}",
        serde_json::to_string(&(
            &source.database_path,
            &record.key.session_id,
            &source.file_identity,
            "synthetic-scope",
        ))
        .unwrap()
    );
    store
        .lock()
        .execute("UPDATE session SET activity_cursor = ?1", [cursor])
        .unwrap();
    let start = OpenCodeV2CaptureStart {
        key: record.key,
        incarnation: crate::store::Incarnation(7),
        source_generation: 0,
        source_label: record.source_label,
        cwd: "/workspace/demo".into(),
        admitted_source: source,
        baseline_sequence: Some(4),
    };
    let claim = store
        .start_opencode_v2_capture_epoch(&[start], "idle", 100, 400)
        .unwrap()
        .unwrap()
        .remove(0);
    let position = ToolResultPosition::OpenCodeV2Event {
        sequence: 5,
        item_index: 0,
    };
    assert!(
        store
            .advance_opencode_v2_evidence(
                &claim,
                5,
                &NativeEvidenceBatch {
                    source_format: SourceFormat::OpenCodeSqliteV2,
                    history_complete: false,
                    events: vec![NativeEvidenceEvent {
                        live_eligible: true,
                        fact: NativeEvidenceFact::Message(NativeMessageObservation::Public(
                            PublicMessageObservation {
                                source_format: SourceFormat::OpenCodeSqliteV2,
                                session_id: claim.key.session_id.clone(),
                                record_id: "public-assistant".into(),
                                part_id: Some("text".into()),
                                call_id: None,
                                position,
                                revision: 5,
                                change: ToolResultChange::Update,
                                eligible: true,
                                completeness: ToolResultCompleteness::Complete,
                                context: Some(NativeEventContext {
                                    first_eligible_position: position,
                                    main_session: true,
                                    complete_through_event: false,
                                    user_messages: Vec::new(),
                                }),
                                payload: PublicMessagePayload::AssistantText {
                                    parts: vec![UserMessagePart {
                                        item_index: 0,
                                        native_part_id: Some("text".into()),
                                        text: "The synthetic input is ready.".into(),
                                    }],
                                    generator: NativeRequestAttribution::default(),
                                },
                            },
                        )),
                    }],
                },
                101,
                400,
            )
            .unwrap()
    );
    let source = PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_live_project(&store, "native", "/workspace/demo")
            .with_activation(&activation.id),
        20,
    )
    .unwrap();
    let initial_page = source.read_batch(PublicationCursor(0)).unwrap();
    assert!(!initial_page.has_more);
    let initial = engine
        .apply_source_page(&activation, &initial_page)
        .unwrap();
    assert_eq!((initial.observations, initial.candidates), (1, 1));
    local::start_and_accept(&engine, &activation, 101_000);
    let validity = || {
        store
            .lock()
            .query_row(
                "SELECT h.evidence_valid, s.evidence_valid FROM burn_check_assessment_history h
               JOIN burn_check_current_status s USING(assessment_id)
              WHERE h.check_id = ?1 AND h.activation_scope_key = ?2",
                params![definition.check_id, definition.activation_scope_key],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap()
    };
    assert_eq!(validity(), (1, 1));
    let accepted = rows(&store, std::slice::from_ref(&definition));
    assert_eq!((accepted[0].finding, accepted[0].clean), (1, 0));
    let branch = BranchId(claim.key.to_identity_json());
    let before = adapter.load_source_progress(&activation).unwrap();
    assert_eq!(before.publication_cursor, initial_page.next);
    assert_eq!(before.selection.branches[&branch].results.len(), 1);
    let frozen: (String, String) = store.lock().query_row(
        "SELECT frozen_input_json, input_fingerprint FROM burn_check_candidate WHERE state = 'accepted'",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    let (last_activity, unfinished): (i64, i64) = store.lock().query_row(
        "SELECT updated_at_epoch, (SELECT COUNT(*) FROM burn_check_candidate
           WHERE state IN ('pending', 'claimed', 'dispatching', 'answered'))
           FROM session WHERE environment_key = 'native' AND agent = 'opencode' AND session_id = 'idle-detach'",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert!(last_activity < 400 - antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS);
    assert_eq!(unfinished, 0);
    assert_eq!(
        store
            .mark_opencode_v2_capture_gap(std::slice::from_ref(&claim), "source-detached", 400)
            .unwrap(),
        1
    );
    assert_eq!(validity(), (0, 0));
    let publications = store.native_tool_result_publications_after(0, 20).unwrap();
    let detached_cursor = PublicationCursor(
        u64::try_from(
            publications
                .iter()
                .find(|publication| publication.gap_reason.as_deref() == Some("source-detached"))
                .unwrap()
                .publication_sequence,
        )
        .unwrap(),
    );
    assert!(detached_cursor > before.publication_cursor);
    let mut request = request();
    request.computed_at_epoch = 400;
    let assert_no_current_failure = || {
        let report =
            read_installed_checks_in_window(&store, std::slice::from_ref(&definition), &request)
                .unwrap();
        assert!(report[0].diagnostics.is_empty());
        assert_eq!((report[0].finding, report[0].clean), (0, 0));
        assert_eq!(report[0].unavailable, 1);
        assert_eq!(report[0].checking_count, 0);
        assert!(report[0].waiting_for_events.is_none());
    };
    assert_no_current_failure();
    assert_eq!(
        adapter
            .load_source_progress(&activation)
            .unwrap()
            .publication_cursor,
        before.publication_cursor
    );
    let page = source.read_batch(before.publication_cursor).unwrap();
    assert!(!page.has_more);
    assert_eq!(page.next, detached_cursor);
    assert!(page.events.iter().any(|event| matches!(
        event,
        SourceEvent::SegmentReset { cursor, branch: reset_branch, .. }
            if *cursor == detached_cursor && reset_branch == &branch
    )));
    let consumed = engine.apply_source_page(&activation, &page).unwrap();
    assert_eq!((consumed.observations, consumed.candidates), (0, 0));
    let after = adapter.load_source_progress(&activation).unwrap();
    assert_eq!(after.publication_cursor, detached_cursor);
    assert_eq!(after.observation_position, before.observation_position);
    assert!(after.selection.branches[&branch].results.is_empty());
    assert!(!after.selection.branches[&branch].coverage_complete);
    assert_eq!(validity(), (0, 0));
    assert_eq!(store.lock().query_row(
        "SELECT frozen_input_json, input_fingerprint FROM burn_check_candidate WHERE state = 'accepted'",
        [], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    ).unwrap(), frozen);
    assert_eq!(
        store.native_tool_result_publications_after(0, 20).unwrap(),
        publications
    );
    assert_no_current_failure();
}

#[test]
fn capture_failure_is_current_at_the_activity_window_boundary_only() {
    let store = store();
    let definition = definition("/workspace/demo");
    activate(&store, &definition);
    let record = v2_session(&store, "boundary");
    store
        .set_opencode_v2_source_status(&[record], OpenCodeV2SourceStatus::CaptureGap, 100)
        .unwrap();
    let mut request = request();
    request.computed_at_epoch = 100 + antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS;
    assert_eq!(
        read_installed_checks_in_window(&store, std::slice::from_ref(&definition), &request)
            .unwrap()[0]
            .diagnostics
            .len(),
        1
    );
    request.computed_at_epoch += 1;
    assert!(
        read_installed_checks_in_window(&store, &[definition], &request).unwrap()[0]
            .diagnostics
            .is_empty()
    );
}

#[test]
fn report_trace_dedup_uses_closed_diagnostic_categories_not_only_the_count() {
    use crate::store::burn_checks::trace_capture::LogCapture;
    let store = store();
    let definition = definition("/workspace/demo");
    let mut row = rows(&store, &[definition]).remove(0);
    row.effective_id = "PRIVATE_DIAGNOSTIC_CATEGORY_REPORT".into();
    row.diagnostics = vec![storage_diagnostic()];
    row.diagnostics[0].code = "opencode-v2-capture-gap".into();
    row.diagnostics[0].message = "PRIVATE_DIAGNOSTIC_MESSAGE".into();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        for _ in 0..100 {
            trace_report(&row);
        }
        row.diagnostics[0].code = "provider-authentication".into();
        for _ in 0..100 {
            trace_report(&row);
        }
        row.diagnostics[0].message = "PRIVATE_REVISED_MESSAGE".into();
        for _ in 0..100 {
            trace_report(&row);
        }
    });
    let events = captured.events("installed_check_report_state");
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].fields["diagnostics"],
        events[1].fields["diagnostics"]
    );
    assert_ne!(
        events[0].fields["diagnostic_categories"],
        events[1].fields["diagnostic_categories"]
    );
    let allowed = [
        "event",
        "report_ref",
        "check_ref",
        "reason",
        "finding",
        "clean",
        "unavailable",
        "diagnostics",
        "diagnostic_categories",
        "review_failed",
        "checking",
        "waiting_for_data",
        "waiting_for_results",
        "remaining_events",
        "catching_up",
    ]
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();
    let expected = allowed
        .difference(&["check_ref", "remaining_events"].into_iter().collect())
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        events[0]
            .fields
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    assert!(!captured.text().contains("PRIVATE_"));
}
