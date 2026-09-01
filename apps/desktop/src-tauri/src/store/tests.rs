use std::path::Path;

use antiburn_local::analysis::{
    ContentKind, ContentPart, MemoryTurnRowStore, TurnRowStore, TurnScope, count_turn_content_rows,
    count_turn_rows,
};

use super::model::{
    PublishedEvidence, SESSION_DATA_RETENTION_DAYS_30, SESSION_DATA_RETENTION_DAYS_90,
};
use super::*;

mod reconcile_tests;

fn store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-test-state")).expect("in-memory store")
}

fn session(session_id: &str, updated_at: i64) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", "claude-code", session_id),
        source_kind: "file".into(),
        source_label: format!("/home/avery/.claude/projects/demo/{session_id}.jsonl"),
        wsl_distro: None,
        title: Some("Wire the popover".into()),
        title_source: Some("explicit".into()),
        cwd: Some("/home/avery/code/widgets".into()),
        surface: "cli".into(),
        updated_at_epoch: Some(updated_at),
        activity_cursor: String::new(),
        activity_source: "mtime".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: None,
    }
}

fn projection_revisions() -> ProjectionRevisions {
    ProjectionRevisions {
        parser_revision: 1,
        analyzer_revision: 1,
        metrics_schema_revision: 1,
        evidence_schema_revision: 1,
    }
}

fn projection_record(key: SessionKey, fingerprint: &str, generation: i64) -> AnalysisRecord {
    AnalysisRecord {
        key,
        model_breakdown_json: "{}".into(),
        inclusive_models_json: "[]".into(),
        initial_context_json: None,
        source_summaries_json: None,
        provider_hints_json: None,
        source_fingerprint: fingerprint.into(),
        pricing_generation: 1,
        analyzed_generation: generation,
        parser_revision: 1,
        analyzer_revision: 1,
        metrics_schema_revision: 1,
    }
}

fn claimed_projection(
    store: &Store,
    session_id: &str,
    now_epoch: i64,
    lease_secs: i64,
) -> (AnalysisRecord, EvidenceClaim) {
    let mut session = session(session_id, 1_000);
    let fingerprint = format!("sv1:{session_id}");
    session.source_fingerprint = Some(fingerprint.clone());
    store
        .upsert_sessions(
            std::slice::from_ref(&session),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let claim = store
        .claim_next_evidence(&["claude-code"], now_epoch, lease_secs)
        .unwrap()
        .unwrap();
    (
        projection_record(session.key, &fingerprint, claim.source_generation),
        claim,
    )
}

fn evidence_completion(
    claim: &EvidenceClaim,
    status: PublishedEvidence,
    evidence_json: String,
) -> EvidenceCompletion {
    EvidenceCompletion {
        claim_fence: claim.claim_fence,
        status,
        evidence_schema_revision: 1,
        evidence_json,
    }
}

/// Seed a session and mark its evidence row Ready with every column current,
/// but write no `session_analysis` row. Use this to isolate the
/// missing-analysis requeue arm from the `metrics_schema_revision` join it
/// depends on.
fn seed_ready_evidence_row(store: &Store, session_id: &str) -> SessionRecord {
    let mut record = session(session_id, 1_000);
    record.source_fingerprint = Some("sv1:current".into());
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
                SET status = 'ready', analyzed_generation = 1,
                    processed_fingerprint = 'sv1:current', parser_revision = 1,
                    analyzer_revision = 1, evidence_schema_revision = 1,
                    evidence_json = '{\"groups\":[]}',
                    retry_count = 0, claim_fence = 4, analyzed_at_epoch = 900
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();
    record
}

fn seed_current_session_evidence(store: &Store, session_id: &str) -> SessionRecord {
    let record = seed_ready_evidence_row(store, session_id);
    store
        .save_analysis(
            &projection_record(record.key.clone(), "sv1:current", 1),
            None,
        )
        .unwrap();
    record
}

fn seed_revision_one_placeholder(store: &Store, session_id: &str) -> SessionRecord {
    let record = seed_current_session_evidence(store, session_id);
    store
        .lock()
        .execute(
            "UPDATE session_evidence
                SET evidence_json = '{\"state\":\"unimplemented\"}'
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();
    record
}

fn published_evidence_pass(record: &SessionRecord) -> crate::analysis::EvidencePass {
    let store: Arc<dyn TurnRowStore> =
        MemoryTurnRowStore::new("claude", record.key.session_id.clone());
    let mut pass = crate::analysis::evidence_pass_with_turn_rows(
        &[antiburn_local::analysis::SessionInput {
            agent: "claude".into(),
            session_id: record.key.session_id.clone(),
            source: antiburn_local::analysis::RawSource::Jsonl(
                r#"{"type":"assistant","timestamp":100,"message":{"id":"m","role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":2,"output_tokens":3},"content":[]}}
"#
                .into(),
            ),
        }],
        &|| false,
        Some(store),
    );
    pass.analysis.fingerprint = record
        .source_fingerprint
        .clone()
        .unwrap_or_else(|| crate::analysis::MISSING_FINGERPRINT.into());
    pass
}

fn assert_unchanged_session_evidence(
    session_id: &str,
    status: &str,
    retry_count: i64,
    claimed_at_epoch: Option<i64>,
    lease_expires_at_epoch: Option<i64>,
    next_attempt_at_epoch: Option<i64>,
    last_error: Option<&str>,
) {
    let store = store();
    let record = seed_current_session_evidence(&store, session_id);
    store
        .lock()
        .execute(
            "UPDATE session_evidence
                SET status = ?4, retry_count = ?5, claimed_at_epoch = ?6,
                    lease_expires_at_epoch = ?7, next_attempt_at_epoch = ?8,
                    last_error = ?9
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id,
                status,
                retry_count,
                claimed_at_epoch,
                lease_expires_at_epoch,
                next_attempt_at_epoch,
                last_error,
            ],
        )
        .unwrap();
    let before = store.evidence(&record.key).unwrap().unwrap();

    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], projection_revisions())
            .unwrap(),
        0
    );

    assert_eq!(store.evidence(&record.key).unwrap().unwrap(), before);
}

#[test]
fn a_fresh_database_is_migrated_to_the_latest_version() {
    let store = store();
    assert_eq!(
        store.schema_version().unwrap(),
        super::schema::MIGRATIONS.len() as i64
    );
}

#[test]
fn evidence_batch_preserves_keys_and_missing_rows_in_request_order() {
    let store = store();
    let first = seed_current_session_evidence(&store, "synthetic-batch-first");
    let second = seed_current_session_evidence(&store, "synthetic-batch-second");
    let missing = SessionKey::new("native", "claude-code", "synthetic-batch-missing");

    let rows = store
        .evidence_batch(&[second.key.clone(), missing, first.key.clone()])
        .unwrap();

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].as_ref().map(|row| &row.key), Some(&second.key));
    assert!(rows[1].is_none());
    assert_eq!(rows[2].as_ref().map(|row| &row.key), Some(&first.key));
}

#[test]
fn a_fresh_database_selects_every_ten_percent_milestone() {
    let settings = store().settings().unwrap();

    for threshold in MILESTONE_OPTIONS {
        let expected = threshold % 10 == 0;
        assert_eq!(settings.milestones_5h.contains(threshold), expected);
        assert_eq!(settings.milestones_weekly.contains(threshold), expected);
    }
}

#[test]
fn session_analysis_holds_the_cache_values_and_the_projection_revisions() {
    let store = store();
    let connection = store.lock();
    let mut statement = connection
        .prepare("SELECT name FROM pragma_table_info('session_analysis')")
        .unwrap();
    let columns: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();

    assert_eq!(
        columns,
        vec![
            "environment_key",
            "agent",
            "session_id",
            "model_breakdown_json",
            "inclusive_models_json",
            "source_fingerprint",
            "pricing_generation",
            "analyzed_generation",
            "parser_revision",
            "analyzer_revision",
            "metrics_schema_revision",
            "initial_context_json",
            "source_summaries_json",
            "provider_hints_json",
        ]
    );
}

#[test]
fn provider_hints_migration_keeps_old_analysis_unknown() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..23] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute(
            "INSERT INTO session_analysis (
                 environment_key, agent, session_id, model_breakdown_json,
                 inclusive_models_json, source_fingerprint, pricing_generation,
                 analyzed_generation, parser_revision, analyzer_revision,
                 metrics_schema_revision)
             VALUES ('native', 'pi', 'legacy', '{}', '[]', 'sv1:legacy', 1, 1, 1, 1, 3)",
            [],
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 23).unwrap();

    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-provider-hints-migration-test").to_path_buf(),
    )
    .expect("V24 migrates the prior schema");
    let analysis = store
        .analysis(&SessionKey::new("native", "pi", "legacy"))
        .unwrap()
        .expect("legacy analysis survives");

    assert_eq!(store.schema_version().unwrap(), 27);
    assert_eq!(analysis.provider_hints_json, None);
}

#[test]
fn account_observation_migration_initializes_the_latest_timestamp() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..26] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute(
            "INSERT INTO provider_account_seen (
                agent, provider, account_key, first_seen_epoch
             ) VALUES ('pi', 'anthropic', ?1, 1234)",
            ["a".repeat(64)],
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 26).unwrap();

    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-account-observation-migration-test").to_path_buf(),
    )
    .unwrap();
    let latest = store
        .lock()
        .query_row(
            "SELECT last_seen_epoch FROM provider_account_seen",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();

    assert_eq!(latest, 1234);
}

/// Opting out is a withdrawal, not a pause: nothing queued survives it, and
/// neither does the identifier that would let a later opt-in be joined to it.
#[test]
fn opting_out_destroys_the_queue_and_the_installation_identity() {
    let store = store();
    store
        .set_analytics_identity("11111111-1111-4111-8111-111111111111")
        .unwrap();
    store.queue_analytics_event("app_launched", "{}").unwrap();
    assert_eq!(store.pending_analytics_events(10).unwrap().len(), 1);
    assert!(store.analytics_identity().unwrap().is_some());

    store.clear_analytics().unwrap();

    assert!(store.pending_analytics_events(10).unwrap().is_empty());
    assert!(store.analytics_identity().unwrap().is_none());
}

/// An undeliverable event is dropped rather than retried forever: a queue that
/// grows without bound on a machine that is offline for a week is a
/// disk-space bug, not a feature.
#[test]
fn events_are_given_up_on_after_a_bounded_number_of_attempts() {
    let store = store();
    store.queue_analytics_event("app_launched", "{}").unwrap();
    let ids: Vec<i64> = store
        .pending_analytics_events(10)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();

    for _ in 0..2 {
        assert_eq!(store.fail_analytics_events(&ids, 3).unwrap(), 0);
        assert_eq!(store.pending_analytics_events(10).unwrap().len(), 1);
    }
    assert_eq!(store.fail_analytics_events(&ids, 3).unwrap(), 1);
    assert!(store.pending_analytics_events(10).unwrap().is_empty());
}

#[test]
fn consent_grants_round_trip_and_revoke_individually() {
    let store = store();
    assert!(store.granted_dirs().unwrap().is_empty());

    store.grant_dir("Documents").unwrap();
    store.grant_dir("Desktop").unwrap();
    assert_eq!(store.granted_dirs().unwrap().len(), 2);

    // Re-granting refreshes the row rather than adding a second one.
    store.grant_dir("Documents").unwrap();
    assert_eq!(store.granted_dirs().unwrap().len(), 2);

    // Revoking is per directory, and revoking an absent one is a no-op.
    store.revoke_dir_grant("Documents").unwrap();
    store.revoke_dir_grant("Downloads").unwrap();
    assert_eq!(
        store.granted_dirs().unwrap(),
        std::collections::HashSet::from(["Desktop".to_string()])
    );
}

#[test]
fn deferred_permission_dirs_replace_the_previous_pass() {
    let store = store();

    store
        .set_deferred_permission_dirs(&[crate::dto::DeferredPermissionDir {
            dir: "Documents".to_string(),
            path_count: 3,
        }])
        .unwrap();
    assert_eq!(
        store.internal_value(super::DEFERRED_PERMISSION_DIRS_KEY),
        Some(r#"[{"dir":"Documents","pathCount":3}]"#.to_string())
    );

    // A later pass that defers nothing clears the list rather than leaving a
    // stale directory asking for permission it no longer needs.
    store.set_deferred_permission_dirs(&[]).unwrap();
    assert_eq!(
        store.internal_value(super::DEFERRED_PERMISSION_DIRS_KEY),
        Some("[]".to_string())
    );
}

#[test]
fn migrating_an_already_current_database_is_a_no_op() {
    let store = store();
    // `migrate` runs on open; running it again must neither fail nor re-apply.
    store.migrate().unwrap();
    store.migrate().unwrap();
    assert_eq!(
        store.schema_version().unwrap(),
        super::schema::MIGRATIONS.len() as i64
    );
}

#[test]
fn settings_default_before_anything_is_written_and_round_trip_after() {
    let store = store();
    let defaults = store.settings().unwrap();
    assert_eq!(defaults, AppSettings::default());
    assert!(!defaults.onboarding_completed);
    assert!(defaults.launch_at_login);
    // On by default: fetching the reader's own usage from a provider they
    // already use, with a credential they already hold, is ordinary traffic,
    // not something that needs a first-run choice. See `live_usage_active`
    // for the onboarding gate that still applies regardless of this default.
    assert!(defaults.live_usage_enabled);
    // Analytics starts automatically only after onboarding completes.
    assert!(defaults.analytics_enabled);
    // Open by default, same reasoning: a reader who has limits to see should
    // see them without an extra click the first time they notice the section.
    assert!(defaults.overview_limits_expanded);
    assert_eq!(
        defaults.session_data_retention_days,
        RETAIN_SESSION_DATA_FOREVER
    );

    // Notifications default on, both kinds with them, so the two per-kind
    // preferences below are a real change rather than a re-statement.
    assert!(defaults.notifications_enabled);
    assert!(defaults.notify_update_available);
    assert!(defaults.notify_scan_failure);

    let saved = store
        .save_settings(&AppSettings {
            theme: ThemePreference::Dark,
            activity_window_days: 14,
            session_data_retention_days: SESSION_DATA_RETENTION_DAYS_90,
            onboarding_completed: true,
            launch_at_login: true,
            auto_update: false,
            discovery_paused: true,
            notifications_enabled: false,
            notify_update_available: false,
            notify_scan_failure: true,
            nudge_placement: NudgePlacement::TopRight,
            nudge_auto_dismiss_secs: 25,
            notification_sound: false,
            disk_space_display: DiskSpaceDisplay::Always,
            disk_space_threshold_gb: 100,
            notify_disk_space_low: false,
            milestones_5h: Milestones::selected([75, 90]),
            milestones_weekly: Milestones::none(),
            live_usage_enabled: true,
            live_usage_hidden_providers: HiddenMeters::default(),
            analytics_enabled: false,
            overview_limits_expanded: false,
        })
        .unwrap();
    assert_eq!(store.settings().unwrap(), saved);
    assert_eq!(saved.theme, ThemePreference::Dark);
    assert_eq!(saved.activity_window_days, 14);
    assert_eq!(
        saved.session_data_retention_days,
        SESSION_DATA_RETENTION_DAYS_90
    );
    assert_eq!(saved.nudge_placement, NudgePlacement::TopRight);
    assert_eq!(saved.nudge_auto_dismiss_secs, 25);
    assert_eq!(saved.disk_space_display, DiskSpaceDisplay::Always);
    assert_eq!(saved.disk_space_threshold_gb, 100);
    // The empty milestone subset survives a round trip as "none selected",
    // not as a reset back to the defaults.
    assert!(!saved.milestones_weekly.any());
    assert!(saved.milestones_5h.contains(75) && !saved.milestones_5h.contains(50));
    assert!(saved.live_usage_enabled);
    assert!(!saved.overview_limits_expanded);
    assert!(saved.onboarding_completed);
    assert!(saved.discovery_paused);
    // Each notification preference is stored on its own key, so a reader who
    // silences the master switch keeps the per-kind choices they made.
    assert!(!saved.notifications_enabled);
    assert!(!saved.notify_update_available);
    assert!(saved.notify_scan_failure);
}

#[test]
fn an_explicit_launch_at_login_opt_out_overrides_the_default() {
    let store = store();
    let saved = store
        .save_settings(&AppSettings {
            onboarding_completed: true,
            launch_at_login: false,
            ..AppSettings::default()
        })
        .unwrap();

    assert!(!saved.launch_at_login);
    assert!(!store.settings().unwrap().launch_at_login);
}

#[test]
fn updating_settings_merges_against_the_latest_stored_value() {
    let store = store();
    store
        .save_settings(&AppSettings {
            theme: ThemePreference::Dark,
            auto_update: false,
            ..AppSettings::default()
        })
        .unwrap();

    let (previous, saved) = store
        .update_settings(|settings| {
            settings.activity_window_days = 14;
            settings.launch_at_login = false;
            settings.onboarding_completed = true;
        })
        .unwrap();

    assert_eq!(previous.theme, ThemePreference::Dark);
    assert!(!previous.auto_update);
    assert_eq!(saved.theme, ThemePreference::Dark);
    assert!(!saved.auto_update);
    assert_eq!(saved.activity_window_days, 14);
    assert!(!saved.launch_at_login);
    assert!(saved.onboarding_completed);
    assert_eq!(store.settings().unwrap(), saved);
}

#[test]
fn restarting_onboarding_preserves_local_state_and_is_idempotent() {
    let store = store();
    let before = store
        .save_settings(&AppSettings {
            theme: ThemePreference::Dark,
            activity_window_days: 14,
            onboarding_completed: true,
            launch_at_login: false,
            analytics_enabled: false,
            ..AppSettings::default()
        })
        .unwrap();
    store
        .upsert_sessions(&[session("abc", 2_000)], &crate::agents::evidence_cohort())
        .unwrap();
    store.add_scan_root("/home/avery/work").unwrap();
    store.queue_analytics_event("app_launched", "{}").unwrap();

    let (previous, restarted) = store.restart_onboarding().unwrap();

    let mut expected = before.clone();
    expected.onboarding_completed = false;
    assert_eq!(previous, before);
    assert_eq!(restarted, expected);
    assert_eq!(store.settings().unwrap(), expected);
    assert_eq!(store.session_count().unwrap(), 1);
    assert_eq!(store.scan_roots().unwrap(), vec!["/home/avery/work"]);
    assert_eq!(store.pending_analytics_events(10).unwrap().len(), 1);

    let (previous_again, restarted_again) = store.restart_onboarding().unwrap();
    assert_eq!(previous_again, expected);
    assert_eq!(restarted_again, expected);
}

/// Pin the current session shape so migrations remain deliberate. This is not
/// a restriction on what a future local visibility feature may store.
#[test]
fn the_session_table_shape_is_stable() {
    let store = store();
    let connection = store.lock();
    let mut statement = connection
        .prepare("SELECT name FROM pragma_table_info('session')")
        .unwrap();
    let columns: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();

    assert_eq!(
        columns,
        vec![
            "environment_key",
            "agent",
            "session_id",
            "source_kind",
            "source_label",
            "wsl_distro",
            "title",
            "title_source",
            "cwd",
            "surface",
            "updated_at_epoch",
            "subagent_count",
            "first_seen_at",
            "last_seen_at",
            "activity_source",
            "activity_cursor",
            "source_fingerprint",
            "source_generation",
            "started_at_epoch",
        ]
    );
}

#[test]
fn session_evidence_table_shape_is_stable() {
    let store = store();
    let connection = store.lock();
    let mut statement = connection
        .prepare("SELECT name FROM pragma_table_info('session_evidence')")
        .unwrap();
    let columns: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();

    assert_eq!(
        columns,
        vec![
            "environment_key",
            "agent",
            "session_id",
            "status",
            "analyzed_generation",
            "processed_fingerprint",
            "parser_revision",
            "analyzer_revision",
            "evidence_schema_revision",
            "evidence_json",
            "retry_count",
            "claim_fence",
            "claimed_at_epoch",
            "lease_expires_at_epoch",
            "next_attempt_at_epoch",
            "analyzed_at_epoch",
            "last_error",
            "published_fence",
        ]
    );
}

#[test]
fn session_evidence_status_index_exists() {
    let store = store();
    let connection = store.lock();
    let columns: Vec<String> = connection
        .prepare("SELECT name FROM pragma_index_info('session_evidence_status') ORDER BY seqno")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();

    assert_eq!(
        columns,
        vec!["status", "next_attempt_at_epoch", "lease_expires_at_epoch"]
    );
}

#[test]
fn session_evidence_rejects_an_unknown_status() {
    let store = store();
    store
        .upsert_sessions(
            &[session("bad-status", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let result = store.lock().execute(
        "INSERT INTO session_evidence (environment_key, agent, session_id, status)
         VALUES ('native', 'claude-code', 'bad-status', 'unknown')",
        [],
    );

    assert!(result.is_err());
}

#[test]
fn session_evidence_survives_an_upgrade_from_the_shipped_head() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..10] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute(
            "INSERT INTO session (
                 environment_key, agent, session_id, source_kind, source_label,
                 first_seen_at, last_seen_at)
             VALUES ('native', 'claude-code', 'upgrade', 'file',
                     '/home/avery/.claude/projects/demo/upgrade.jsonl',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 10).unwrap();

    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-evidence-migration-test").to_path_buf(),
    )
    .expect("V11 migrates the shipped schema");
    let key = SessionKey::new("native", "claude-code", "upgrade");

    assert_eq!(store.session_count().unwrap(), 1);

    // Verify the original session row survives the migration unchanged.
    let session_record = store
        .session(&key)
        .unwrap()
        .expect("session row survives migration");
    assert_eq!(session_record.source_kind, "file");
    assert_eq!(
        session_record.source_label,
        "/home/avery/.claude/projects/demo/upgrade.jsonl"
    );

    // Verify the timestamp fields survive unchanged.
    let (first_seen, last_seen): (String, String) = store
        .lock()
        .query_row(
            "SELECT first_seen_at, last_seen_at FROM session
             WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            rusqlite::params!["native", "claude-code", "upgrade"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(first_seen, "2026-01-01T00:00:00Z");
    assert_eq!(last_seen, "2026-01-01T00:00:00Z");

    assert!(store.evidence(&key).unwrap().is_none());
    store
        .lock()
        .execute(
            "INSERT INTO session_evidence (environment_key, agent, session_id)
             VALUES ('native', 'claude-code', 'upgrade')",
            [],
        )
        .unwrap();
    assert_eq!(
        store.evidence(&key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
}

#[test]
fn clearing_local_data_forgets_session_records_and_keeps_the_readers_choices() {
    let store = store();
    store
        .upsert_sessions(&[session("abc", 2_000)], &crate::agents::evidence_cohort())
        .unwrap();
    store
        .save_analysis(
            &AnalysisRecord {
                key: SessionKey::new("native", "claude-code", "abc"),
                model_breakdown_json: "{}".into(),
                inclusive_models_json: "[]".into(),
                initial_context_json: None,
                source_summaries_json: None,
                provider_hints_json: None,
                source_fingerprint: "1:1".into(),
                pricing_generation: 0,
                analyzed_generation: 0,
                parser_revision: 0,
                analyzer_revision: 0,
                metrics_schema_revision: 0,
            },
            None,
        )
        .unwrap();
    store
        .replace_relations(
            &SessionKey::new("native", "claude-code", "abc"),
            RelationKind::Subagent,
            &[RelationRecord {
                kind: RelationKind::Subagent,
                related_id: "child".into(),
                label: Some("Reviewer".into()),
            }],
        )
        .unwrap();
    store
        .record_agent_scan("claude-code", Some(2_000), 1)
        .unwrap();
    store.add_scan_root("/home/avery/work").unwrap();
    let settings = store
        .save_settings(&AppSettings {
            onboarding_completed: true,
            ..AppSettings::default()
        })
        .unwrap();
    store
        .replace_repositories(&[RepositoryRecord {
            key: "widgets".into(),
            repo_name: "widgets".into(),
            full_name: "avery/widgets".into(),
            status: "accessible".into(),
            repo_root: Some("/home/avery/code/widgets".into()),
            suspected_path: None,
            worktree_count: 1,
            session_count: 3,
            wsl_distro: None,
            enabled: false,
        }])
        .unwrap();

    assert_eq!(store.clear_local_session_data().unwrap(), 1);

    assert!(store.recent_sessions(0, 100).unwrap().is_empty());
    assert!(
        store
            .analysis(&SessionKey::new("native", "claude-code", "abc"))
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .relations(&SessionKey::new("native", "claude-code", "abc"))
            .unwrap()
            .is_empty()
    );
    assert!(store.scan_state().unwrap().is_empty());

    // Everything that is a choice rather than a derivation survives.
    assert_eq!(store.settings().unwrap(), settings);
    assert_eq!(store.scan_roots().unwrap(), vec!["/home/avery/work"]);
    let repositories = store.repositories().unwrap();
    assert_eq!(repositories.len(), 1);
    assert!(
        !repositories[0].enabled,
        "the include choice is the reader's"
    );
    assert_eq!(
        repositories[0].session_count, 0,
        "the count was derived from the index that just went away"
    );
}

#[test]
fn clearing_an_already_empty_index_is_a_no_op() {
    let store = store();
    assert_eq!(store.clear_local_session_data().unwrap(), 0);
    assert_eq!(store.clear_local_session_data().unwrap(), 0);
}

#[test]
fn clearing_local_data_forgets_account_pseudonyms_and_rotates_the_install_key() {
    let store = store();
    let old_secret = store.provider_account_secret().unwrap();
    store
        .observe_provider_account("pi", "anthropic", &"a".repeat(64), 2_000, "tool_oauth")
        .unwrap();
    store.set_internal_value("internal:liveUsageHistoryV2", "old-account-key");
    store.set_internal_value("internal:liveUsageSnapshotV2", "old-account-key");

    assert_eq!(
        store
            .lock()
            .query_row("SELECT COUNT(*) FROM provider_account_seen", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );

    assert_eq!(store.clear_local_session_data().unwrap(), 0);

    let connection = store.lock();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM provider_account_seen", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    assert!(
        connection
            .query_row(
                "SELECT value FROM setting WHERE key = 'internal:providerAccountHmacSecretV1'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .unwrap()
            .is_none()
    );
    for key in [
        "internal:liveUsageHistoryV2",
        "internal:liveUsageSnapshotV2",
    ] {
        assert!(
            connection
                .query_row("SELECT value FROM setting WHERE key = ?1", [key], |row| row
                    .get::<_, String>(0),)
                .optional()
                .unwrap()
                .is_none()
        );
    }
    drop(connection);
    assert_ne!(store.provider_account_secret().unwrap(), old_secret);
}

#[test]
fn session_retention_removes_only_sessions_before_the_cutoff() {
    const NOW: i64 = 2_000_000_000;
    const DAY: i64 = 86_400;
    let store = store();
    store
        .upsert_sessions(
            &[
                session("expired", NOW - 30 * DAY - 1),
                session("boundary", NOW - 30 * DAY),
                session("recent", NOW - DAY),
            ],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .save_settings(&AppSettings {
            session_data_retention_days: SESSION_DATA_RETENTION_DAYS_30,
            ..AppSettings::default()
        })
        .unwrap();

    assert_eq!(store.apply_session_retention(NOW).unwrap(), 1);
    let remaining = store.recent_sessions(0, 100).unwrap();
    assert_eq!(remaining.len(), 2);
    assert!(
        remaining
            .iter()
            .any(|record| record.key.session_id == "boundary")
    );
    assert!(
        remaining
            .iter()
            .any(|record| record.key.session_id == "recent")
    );
}

#[test]
fn session_retention_uses_last_seen_when_activity_time_is_unknown() {
    let store = store();
    store
        .upsert_sessions(
            &[session("unknown-time", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session
                SET updated_at_epoch = NULL, last_seen_at = '2020-01-01T00:00:00Z'
              WHERE session_id = 'unknown-time'",
            [],
        )
        .unwrap();
    store
        .save_settings(&AppSettings {
            session_data_retention_days: SESSION_DATA_RETENTION_DAYS_90,
            ..AppSettings::default()
        })
        .unwrap();

    assert_eq!(store.apply_session_retention(2_000_000_000).unwrap(), 1);
    assert_eq!(store.session_count().unwrap(), 0);
}

#[test]
fn session_retention_removes_all_derived_session_data() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "expired-derived");
    store
        .upsert_sessions(
            &[session("expired-derived", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .save_analysis(
            &AnalysisRecord {
                key: key.clone(),
                model_breakdown_json: "{}".into(),
                inclusive_models_json: "[]".into(),
                initial_context_json: None,
                source_summaries_json: None,
                provider_hints_json: None,
                source_fingerprint: "1:1".into(),
                pricing_generation: 0,
                analyzed_generation: 0,
                parser_revision: 0,
                analyzer_revision: 0,
                metrics_schema_revision: 0,
            },
            None,
        )
        .unwrap();
    store
        .replace_relations(
            &key,
            RelationKind::Subagent,
            &[RelationRecord {
                kind: RelationKind::Subagent,
                related_id: "child".into(),
                label: None,
            }],
        )
        .unwrap();
    {
        let connection = store.lock();
        insert_turn_rows(&connection, &turn_session_key(&key), 1, &[turn_row(0)]).unwrap();
    }
    store
        .save_settings(&AppSettings {
            session_data_retention_days: SESSION_DATA_RETENTION_DAYS_30,
            ..AppSettings::default()
        })
        .unwrap();

    assert_eq!(store.apply_session_retention(2_000_000_000).unwrap(), 1);
    assert!(store.analysis(&key).unwrap().is_none());
    assert!(store.evidence(&key).unwrap().is_none());
    assert!(store.relations(&key).unwrap().is_empty());
    assert_eq!(
        count_turn_rows(&store.lock(), &turn_session_key(&key), 1).unwrap(),
        0
    );
}

#[test]
fn forever_retention_is_a_no_op() {
    let store = store();
    store
        .upsert_sessions(&[session("kept", 1)], &crate::agents::evidence_cohort())
        .unwrap();

    assert_eq!(store.apply_session_retention(2_000_000_000).unwrap(), 0);
    assert_eq!(store.session_count().unwrap(), 1);
}

#[test]
fn saving_retention_and_removing_expired_sessions_is_one_store_operation() {
    let store = store();
    store
        .upsert_sessions(&[session("expired", 1)], &crate::agents::evidence_cohort())
        .unwrap();

    let (_, saved, removed) = store
        .replace_settings_with(
            &AppSettings {
                session_data_retention_days: SESSION_DATA_RETENTION_DAYS_30,
                ..AppSettings::default()
            },
            |tx, saved| {
                apply_session_retention_in(tx, saved.session_data_retention_days, 2_000_000_000)
            },
        )
        .unwrap();

    assert_eq!(removed, 1);
    assert_eq!(
        saved.session_data_retention_days,
        SESSION_DATA_RETENTION_DAYS_30
    );
    assert_eq!(store.settings().unwrap(), saved);
}

#[test]
fn unsupported_session_retention_values_normalize_to_forever() {
    let store = store();
    for value in [0, 60, -2] {
        let saved = store
            .save_settings(&AppSettings {
                session_data_retention_days: value,
                ..AppSettings::default()
            })
            .unwrap();
        assert_eq!(
            saved.session_data_retention_days,
            RETAIN_SESSION_DATA_FOREVER
        );
    }

    store
        .lock()
        .execute(
            "UPDATE setting SET value = 'not-a-number' WHERE key = 'sessionDataRetentionDays'",
            [],
        )
        .unwrap();
    assert_eq!(
        store.settings().unwrap().session_data_retention_days,
        RETAIN_SESSION_DATA_FOREVER
    );
}

#[test]
fn repository_counts_can_be_refreshed_after_retention_cleanup() {
    const NOW: i64 = 2_000_000_000;
    let store = store();
    store
        .upsert_sessions(
            &[session("expired", 1), session("recent", NOW - 1)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .replace_repositories(&[RepositoryRecord {
            key: "widgets".into(),
            repo_name: "widgets".into(),
            full_name: "avery/widgets".into(),
            status: "accessible".into(),
            repo_root: Some("/home/avery/code/widgets".into()),
            suspected_path: None,
            worktree_count: 1,
            session_count: 2,
            wsl_distro: None,
            enabled: true,
        }])
        .unwrap();
    store
        .save_settings(&AppSettings {
            session_data_retention_days: SESSION_DATA_RETENTION_DAYS_30,
            ..AppSettings::default()
        })
        .unwrap();

    assert_eq!(store.apply_session_retention(NOW).unwrap(), 1);
    crate::repositories::refresh_session_counts(&store).unwrap();

    assert_eq!(store.repositories().unwrap()[0].session_count, 1);
}

#[test]
fn an_out_of_range_activity_window_is_clamped_on_the_way_in() {
    let store = store();
    let saved = store
        .save_settings(&AppSettings {
            activity_window_days: 9_000,
            ..AppSettings::default()
        })
        .unwrap();
    assert_eq!(saved.activity_window_days, MAX_ACTIVITY_DAYS);
    assert_eq!(
        store.settings().unwrap().activity_window_days,
        MAX_ACTIVITY_DAYS
    );

    let saved = store
        .save_settings(&AppSettings {
            activity_window_days: 0,
            ..AppSettings::default()
        })
        .unwrap();
    assert_eq!(saved.activity_window_days, MIN_ACTIVITY_DAYS);
}

#[test]
fn a_session_round_trips_and_a_rescan_updates_it_in_place() {
    let store = store();
    store
        .upsert_sessions(&[session("abc", 1_000)], &crate::agents::evidence_cohort())
        .unwrap();

    let mut later = session("abc", 2_000);
    later.title = Some("Wire the popover, take two".into());
    later.subagent_count = 3;
    store
        .upsert_sessions(
            std::slice::from_ref(&later),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    // Idempotent: a rescan that sees the same session again must not duplicate
    // it, and must not rewind the activity timestamp.
    store
        .upsert_sessions(&[session("abc", 1_500)], &crate::agents::evidence_cohort())
        .unwrap();

    assert_eq!(store.recent_sessions(0, 100).unwrap().len(), 1);
    let stored = store
        .session(&SessionKey::new("native", "claude-code", "abc"))
        .unwrap()
        .expect("session");
    assert_eq!(stored.updated_at_epoch, Some(2_000));
    assert_eq!(stored.subagent_count, 0, "the last scan saw no sub-agents");
    assert_eq!(stored.title.as_deref(), Some("Wire the popover"));
}

#[test]
fn an_event_timestamp_survives_a_newer_mtime_only_upsert() {
    let store = store();
    let mut event = session("semantic", 1_000);
    event.activity_cursor = "[\"parent\",10]".into();
    event.activity_source = "event".into();
    store
        .upsert_sessions(&[event], &crate::agents::evidence_cohort())
        .unwrap();

    let mut mtime = session("semantic", 2_000);
    mtime.activity_cursor = "[\"parent\",20]".into();
    mtime.activity_source = "mtime".into();
    store
        .upsert_sessions(&[mtime], &crate::agents::evidence_cohort())
        .unwrap();

    let stored = store
        .session(&SessionKey::new("native", "claude-code", "semantic"))
        .unwrap()
        .expect("session");
    assert_eq!(stored.updated_at_epoch, Some(1_000));
    assert_eq!(stored.activity_source, "event");
    assert_eq!(stored.activity_cursor, "[\"parent\",20]");
}

#[test]
fn activity_cursors_do_not_collide_across_environments() {
    let store = store();
    let native = session("shared", 1_000);
    let mut wsl = native.clone();
    wsl.key = SessionKey::new("wsl:ubuntu", "claude-code", "shared");
    store
        .upsert_sessions(&[native, wsl], &crate::agents::evidence_cohort())
        .unwrap();

    let states = store.session_activity_states().unwrap();
    assert_eq!(states.len(), 2);
    assert!(states.contains_key(&SessionActivityKey::new(
        "native",
        "claude-code",
        "/home/avery/.claude/projects/demo/shared.jsonl",
    )));
    assert!(states.contains_key(&SessionActivityKey::new(
        "wsl:ubuntu",
        "claude-code",
        "/home/avery/.claude/projects/demo/shared.jsonl",
    )));
}

#[test]
fn title_and_source_are_replaced_or_preserved_as_one_pair() {
    let store = store();

    let mut source_without_title = session("orphan-source", 500);
    source_without_title.title = None;
    source_without_title.title_source = Some("firstMessage".into());
    store
        .upsert_sessions(&[source_without_title], &crate::agents::evidence_cohort())
        .unwrap();
    let stored = store
        .session(&SessionKey::new("native", "claude-code", "orphan-source"))
        .unwrap()
        .expect("session");
    assert_eq!((stored.title, stored.title_source), (None, None));

    // Heal rows written by the older independent-COALESCE upsert, where a
    // sanitized-away title could leave provenance behind.
    store
        .lock()
        .execute(
            "UPDATE session SET title_source = 'firstMessage'\
             WHERE environment_key = 'native' AND agent = 'claude-code'\
               AND session_id = 'orphan-source'",
            [],
        )
        .unwrap();
    let mut no_title_again = session("orphan-source", 750);
    no_title_again.title = None;
    no_title_again.title_source = None;
    store
        .upsert_sessions(&[no_title_again], &crate::agents::evidence_cohort())
        .unwrap();
    let healed = store
        .session(&SessionKey::new("native", "claude-code", "orphan-source"))
        .unwrap()
        .expect("session");
    assert_eq!((healed.title, healed.title_source), (None, None));

    store
        .upsert_sessions(&[session("abc", 1_000)], &crate::agents::evidence_cohort())
        .unwrap();

    let mut renamed = session("abc", 2_000);
    renamed.title = Some("Reader supplied title".into());
    renamed.title_source = Some("userRename".into());
    store
        .upsert_sessions(&[renamed], &crate::agents::evidence_cohort())
        .unwrap();

    let mut missing = session("abc", 3_000);
    missing.title = None;
    missing.title_source = Some("firstMessage".into());
    store
        .upsert_sessions(&[missing], &crate::agents::evidence_cohort())
        .unwrap();

    let stored = store
        .session(&SessionKey::new("native", "claude-code", "abc"))
        .unwrap()
        .expect("session");
    assert_eq!(stored.title.as_deref(), Some("Reader supplied title"));
    assert_eq!(stored.title_source.as_deref(), Some("userRename"));
}

#[test]
fn the_same_session_id_in_two_environments_stays_two_sessions() {
    let store = store();
    store
        .upsert_sessions(&[session("abc", 1_000)], &crate::agents::evidence_cohort())
        .unwrap();

    let mut in_wsl = session("abc", 1_100);
    in_wsl.key.environment_key = "wsl:ubuntu".into();
    in_wsl.wsl_distro = Some("Ubuntu".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&in_wsl),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    assert_eq!(store.recent_sessions(0, 100).unwrap().len(), 2);
}

#[test]
fn recent_sessions_are_windowed_and_ordered_newest_first() {
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

    let recent = store.recent_sessions(1_500, 100).unwrap();
    let ids: Vec<_> = recent
        .iter()
        .map(|record| record.key.session_id.as_str())
        .collect();
    assert_eq!(ids, vec!["new", "mid"]);

    assert_eq!(
        store.recent_sessions(0, 2).unwrap().len(),
        2,
        "limit applies"
    );
}

#[test]
fn recent_sessions_query_plan_uses_the_coalesced_recency_index() {
    let store = store();
    let connection = store.lock();
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {RECENT_SESSIONS_SQL}"))
        .unwrap();
    let plan_lines: Vec<String> = statement
        .query_map(params![0_i64, 100_i64], |row| row.get::<_, String>(3))
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
fn a_fork_parent_rides_with_the_session_and_resolves_children_back() {
    let store = store();
    store
        .upsert_sessions(
            &[session("parent", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let mut child = session("child", 2_000);
    child.fork_parent_session_id = Some("parent".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&child),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    let stored = store
        .session(&SessionKey::new("native", "claude-code", "child"))
        .unwrap()
        .expect("child");
    assert_eq!(stored.fork_parent_session_id.as_deref(), Some("parent"));

    let children = store
        .fork_children(&SessionKey::new("native", "claude-code", "parent"))
        .unwrap();
    assert_eq!(children, vec!["child".to_string()]);

    // A later scan can carry no observation. It must keep the recorded lineage.
    store
        .upsert_sessions(
            &[session("child", 2_100)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert_eq!(
        store
            .fork_children(&SessionKey::new("native", "claude-code", "parent"))
            .unwrap(),
        vec!["child".to_string()]
    );
}

#[test]
fn analysis_round_trips_and_is_replaced_rather_than_duplicated() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "abc");
    store
        .upsert_sessions(&[session("abc", 1_000)], &crate::agents::evidence_cohort())
        .unwrap();

    let record = AnalysisRecord {
        key: key.clone(),
        model_breakdown_json: r#"{"claude-opus-4-6":{"inputTokens":10}}"#.into(),
        inclusive_models_json:
            r#"[{"model":"claude-haiku-4-5"},{"model":"claude-opus-4-6","thinkingMode":"high"}]"#
                .into(),
        initial_context_json: None,
        source_summaries_json: None,
        provider_hints_json: None,
        source_fingerprint: "1700000000:4096".into(),
        pricing_generation: 0,
        analyzed_generation: 7,
        parser_revision: 1,
        analyzer_revision: 1,
        metrics_schema_revision: 1,
    };
    store.save_analysis(&record, None).unwrap();
    assert_eq!(store.analysis(&key).unwrap().as_ref(), Some(&record));

    let updated = AnalysisRecord {
        source_fingerprint: "1700000900:8192".into(),
        ..record
    };
    store.save_analysis(&updated, None).unwrap();
    let stored = store.analysis(&key).unwrap().expect("analysis");
    assert_eq!(stored.source_fingerprint, "1700000900:8192");
}

/// `publish_projections` is the worker's own write path (`save_analysis` is
/// test scaffolding only), so this pins the generation and revision columns
/// against the path production actually uses.
#[test]
fn publish_projections_writes_the_generation_and_revision_columns() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "revisions", 100, 60);
    let record = AnalysisRecord {
        pricing_generation: 3,
        parser_revision: 1,
        analyzer_revision: 2,
        metrics_schema_revision: 3,
        ..record
    };
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());

    assert!(
        store
            .publish_projections(&record, Some(900), &completion, &[])
            .unwrap()
    );

    assert_eq!(store.analysis(&record.key).unwrap(), Some(record.clone()));
    assert_eq!(
        store
            .session_source_state(&record.key)
            .unwrap()
            .expect("source state")
            .started_at_epoch,
        Some(900)
    );
}

#[test]
fn publish_projections_round_trips_initial_context_json() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "publish-initial-context", 100, 60);
    let record = AnalysisRecord {
        initial_context_json: Some(r#"{"sources":[{"name":"CLAUDE.md","tokens":120}]}"#.into()),
        source_summaries_json: None,
        ..record
    };
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());

    assert!(
        store
            .publish_projections(&record, None, &completion, &[])
            .unwrap()
    );

    assert_eq!(
        store
            .analysis(&record.key)
            .unwrap()
            .and_then(|stored| stored.initial_context_json),
        record.initial_context_json
    );
}

/// V19 adds `source_summaries_json`. `publish_projections` — the worker's
/// own publish path — is what actually writes it; the drilldown's
/// rows-replay path (seam R3c) reads it back through `analysis()`.
#[test]
fn publish_projections_round_trips_source_summaries_json() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "publish-source-summaries", 100, 60);
    let record = AnalysisRecord {
        source_summaries_json: Some(
            r#"{"child-1":{"model":"claude-haiku-4-6"},"publish-source-summaries":{"model":"claude-opus-4-6"}}"#
                .into(),
        ),
        ..record
    };
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());

    assert!(
        store
            .publish_projections(&record, None, &completion, &[])
            .unwrap()
    );

    assert_eq!(
        store
            .analysis(&record.key)
            .unwrap()
            .and_then(|stored| stored.source_summaries_json),
        record.source_summaries_json
    );
}

#[test]
fn publish_projections_round_trips_provider_hints_json() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "publish-provider-hints", 100, 60);
    let record = AnalysisRecord {
        provider_hints_json: Some(r#"[{"provider":"anthropic","model":"claude-opus-4-6"}]"#.into()),
        ..record
    };
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());

    assert!(
        store
            .publish_projections(&record, None, &completion, &[])
            .unwrap()
    );

    assert_eq!(
        store
            .analysis(&record.key)
            .unwrap()
            .and_then(|stored| stored.provider_hints_json),
        record.provider_hints_json
    );
}

/// A later publish with no `started_at_epoch` (the common case: the worker
/// already learned the start time on an earlier pass) must not clear it.
#[test]
fn publish_projections_never_clears_a_known_start_time() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "known-start", 100, 60);
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&record, Some(800), &completion, &[])
            .unwrap()
    );

    // Requeue and reclaim, mirroring a second worker pass over the same
    // session, then publish again with no start time.
    store.requeue_session_evidence(&record.key).unwrap();
    let claim = store
        .claim_next_evidence(&["claude-code"], 200, 60)
        .unwrap()
        .expect("requeued session is claimable");
    let record = AnalysisRecord {
        analyzed_generation: claim.source_generation,
        ..record
    };
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    assert!(
        store
            .publish_projections(&record, None, &completion, &[])
            .unwrap()
    );

    assert_eq!(
        store
            .session_source_state(&record.key)
            .unwrap()
            .expect("source state")
            .started_at_epoch,
        Some(800)
    );
}

#[test]
fn subagent_relations_are_replaced_wholesale() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "abc");
    store
        .upsert_sessions(&[session("abc", 1_000)], &crate::agents::evidence_cohort())
        .unwrap();

    store
        .replace_relations(
            &key,
            RelationKind::Subagent,
            &[
                RelationRecord {
                    kind: RelationKind::Subagent,
                    related_id: "agent-1".into(),
                    label: Some("Review the diff".into()),
                },
                RelationRecord {
                    kind: RelationKind::Subagent,
                    related_id: "agent-2".into(),
                    label: None,
                },
            ],
        )
        .unwrap();
    assert_eq!(store.relations(&key).unwrap().len(), 2);

    store
        .replace_relations(
            &key,
            RelationKind::Subagent,
            &[RelationRecord {
                kind: RelationKind::Subagent,
                related_id: "agent-2".into(),
                label: Some("Run the tests".into()),
            }],
        )
        .unwrap();
    let relations = store.relations(&key).unwrap();
    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0].related_id, "agent-2");
    assert_eq!(relations[0].label.as_deref(), Some("Run the tests"));
}

#[test]
fn deleting_a_session_takes_its_derived_records_with_it() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "abc");
    store
        .upsert_sessions(&[session("abc", 1_000)], &crate::agents::evidence_cohort())
        .unwrap();
    store
        .save_analysis(
            &AnalysisRecord {
                key: key.clone(),
                model_breakdown_json: "{}".into(),
                inclusive_models_json: "[]".into(),
                initial_context_json: None,
                source_summaries_json: None,
                provider_hints_json: None,
                source_fingerprint: "x".into(),
                pricing_generation: 0,
                analyzed_generation: 0,
                parser_revision: 0,
                analyzer_revision: 0,
                metrics_schema_revision: 0,
            },
            None,
        )
        .unwrap();
    store
        .replace_relations(
            &key,
            RelationKind::Subagent,
            &[RelationRecord {
                kind: RelationKind::Subagent,
                related_id: "agent-1".into(),
                label: None,
            }],
        )
        .unwrap();

    assert!(store.delete_session(&key).unwrap());
    assert!(store.session(&key).unwrap().is_none());
    assert!(store.analysis(&key).unwrap().is_none());
    assert!(store.relations(&key).unwrap().is_empty());
    assert!(!store.delete_session(&key).unwrap());
}

#[test]
fn scan_roots_dedup_and_ignore_a_trailing_separator() {
    let store = store();
    store.add_scan_root("/home/avery/work/").unwrap();
    store.add_scan_root("/home/avery/work").unwrap();
    store.add_scan_root("").unwrap();
    assert_eq!(store.scan_roots().unwrap(), vec!["/home/avery/work"]);

    store.remove_scan_root("/home/avery/work/").unwrap();
    assert!(store.scan_roots().unwrap().is_empty());
    store.remove_scan_root("/home/avery/work").unwrap();
}

#[test]
fn a_rescan_refreshes_repositories_but_keeps_the_users_include_choice() {
    let store = store();
    let located = |session_count: u32| RepositoryRecord {
        key: "/home/avery/code/widgets".into(),
        repo_name: "widgets".into(),
        full_name: "avery/widgets".into(),
        status: "accessible".into(),
        repo_root: Some("/home/avery/code/widgets".into()),
        suspected_path: None,
        worktree_count: 1,
        session_count,
        wsl_distro: None,
        enabled: true,
    };

    store.replace_repositories(&[located(2)]).unwrap();
    assert!(
        store
            .set_repository_enabled("/home/avery/code/widgets", false)
            .unwrap()
    );

    store.replace_repositories(&[located(7)]).unwrap();
    let repositories = store.repositories().unwrap();
    assert_eq!(repositories.len(), 1);
    assert_eq!(repositories[0].session_count, 7, "facts refresh");
    assert!(
        !repositories[0].enabled,
        "the user's choice survives a rescan"
    );
}

#[test]
fn a_repository_the_scan_no_longer_sees_is_dropped() {
    let store = store();
    let record = |key: &str| RepositoryRecord {
        key: key.into(),
        repo_name: "widgets".into(),
        full_name: "avery/widgets".into(),
        status: "accessible".into(),
        repo_root: Some(key.into()),
        suspected_path: None,
        worktree_count: 1,
        session_count: 0,
        wsl_distro: None,
        enabled: true,
    };
    store
        .replace_repositories(&[record("/a/widgets"), record("/b/widgets")])
        .unwrap();
    assert_eq!(store.repositories().unwrap().len(), 2);

    store.replace_repositories(&[record("/a/widgets")]).unwrap();
    let repositories = store.repositories().unwrap();
    assert_eq!(repositories.len(), 1);
    assert_eq!(repositories[0].key, "/a/widgets");
}

#[test]
fn agent_scan_state_records_the_high_water_cursor() {
    let store = store();
    store
        .record_agent_scan("claude-code", Some(1_000), 4)
        .unwrap();
    store
        .record_agent_scan("claude-code", Some(500), 2)
        .unwrap();

    let state = store.scan_state().unwrap();
    assert_eq!(state.len(), 1);
    assert_eq!(state[0].0, "claude-code");
    assert_eq!(state[0].2, 2, "the latest pass's count wins");

    let cursor: i64 = store
        .lock()
        .query_row(
            "SELECT cursor_epoch FROM scan_state WHERE agent = 'claude-code'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cursor, 1_000, "the cursor never rewinds");
}

#[test]
fn usage_evidence_joins_the_analysis_and_keeps_sessions_that_have_none() {
    let store = store();
    store
        .upsert_sessions(
            &[session("analyzed", 2_000), session("pending", 1_500)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .save_analysis(
            &AnalysisRecord {
                key: SessionKey::new("native", "claude-code", "analyzed"),
                model_breakdown_json: r#"{"claude-opus-4-6":{"input_tokens":10}}"#.into(),
                inclusive_models_json: r#"[{"model":"claude-opus-4-6","thinkingMode":"high"}]"#
                    .into(),
                initial_context_json: None,
                source_summaries_json: None,
                provider_hints_json: Some(
                    r#"[{"provider":"anthropic","model":"claude-opus-4-6"}]"#.into(),
                ),
                source_fingerprint: "1:1".into(),
                pricing_generation: 0,
                analyzed_generation: 0,
                parser_revision: 0,
                analyzer_revision: 0,
                metrics_schema_revision: 0,
            },
            None,
        )
        .unwrap();

    let evidence = store.usage_evidence(1_000).unwrap();
    assert_eq!(evidence.len(), 2, "newest first");
    assert_eq!(evidence[0].updated_at_epoch, 2_000);
    assert!(
        evidence[0]
            .model_breakdown_json
            .as_deref()
            .is_some_and(|json| json.contains("claude-opus-4-6"))
    );
    assert_eq!(
        evidence[0].provider_hints_json.as_deref(),
        Some(r#"[{"provider":"anthropic","model":"claude-opus-4-6"}]"#)
    );
    // A session analysis has not reached yet comes back with no breakdown
    // rather than being dropped: "not measured" is not "measured zero".
    assert_eq!(evidence[1].updated_at_epoch, 1_500);
    assert_eq!(evidence[1].model_breakdown_json, None);
    assert_eq!(evidence[1].provider_hints_json, None);
    assert_eq!(evidence[1].agent, "claude-code");

    // The bound is inclusive and excludes everything below it.
    assert_eq!(store.usage_evidence(2_000).unwrap().len(), 1);
    assert!(store.usage_evidence(2_001).unwrap().is_empty());
}

#[test]
fn close_account_switches_keep_completed_sessions_with_the_previous_account() {
    let store = store();
    store.set_internal_value("internal:providerAccountRolloutV1", "1000");
    let mut completed = session("completed-a", 2_000);
    completed.key.agent = "pi".into();
    let mut spanning = session("spanning", 2_000);
    spanning.key.agent = "pi".into();
    let mut old = session("old", 900);
    old.key.agent = "pi".into();
    store
        .upsert_sessions(
            &[completed, spanning.clone(), old],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    let first = "a".repeat(64);
    let second = "b".repeat(64);
    store
        .observe_provider_account("pi", "anthropic", &first, 2_025, "tool_oauth")
        .unwrap();
    store
        .observe_provider_account("pi", "anthropic", &first, 2_050, "tool_oauth")
        .unwrap();
    spanning.updated_at_epoch = Some(2_075);
    let mut new = session("new-b", 2_075);
    new.key.agent = "pi".into();
    store
        .upsert_sessions(&[spanning, new], &crate::agents::evidence_cohort())
        .unwrap();
    store
        .observe_provider_account("pi", "anthropic", &second, 2_100, "tool_oauth")
        .unwrap();

    let accounts_for = |session_id: &str| {
        let connection = store.lock();
        let mut statement = connection
            .prepare(
                "SELECT account_key FROM session_provider_account
                  WHERE agent = 'pi' AND session_id = ?1
                  ORDER BY account_key",
            )
            .unwrap();
        statement
            .query_map([session_id], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
    };
    assert_eq!(accounts_for("completed-a"), vec![first.clone()]);
    assert_eq!(accounts_for("new-b"), vec![second.clone()]);
    assert_eq!(accounts_for("spanning"), vec![first, second]);
    assert!(accounts_for("old").is_empty());
}

#[test]
fn account_switches_bind_only_sessions_near_each_observation() {
    let store = store();
    store.set_internal_value("internal:providerAccountRolloutV1", "0");
    let mut first = session("account-a-first", 1_000);
    first.key.agent = "pi".into();
    let mut middle = session("account-b", 2_000);
    middle.key.agent = "pi".into();
    let mut future = session("account-a-return", 3_000);
    future.key.agent = "pi".into();
    store
        .upsert_sessions(&[first, middle, future], &crate::agents::evidence_cohort())
        .unwrap();

    let account_a = "a".repeat(64);
    let account_b = "b".repeat(64);
    store
        .observe_provider_account("pi", "anthropic", &account_a, 1_050, "tool_oauth")
        .unwrap();
    store
        .observe_provider_account("pi", "anthropic", &account_b, 2_050, "tool_oauth")
        .unwrap();
    store
        .observe_provider_account("pi", "anthropic", &account_a, 3_050, "tool_oauth")
        .unwrap();

    let evidence = store.usage_evidence(0).unwrap();
    let accounts_at = |updated_at| {
        let json = &evidence
            .iter()
            .find(|record| record.updated_at_epoch == updated_at)
            .unwrap()
            .provider_accounts_json;
        serde_json::from_str::<Vec<serde_json::Value>>(json)
            .unwrap()
            .into_iter()
            .map(|value| value["accountKey"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        accounts_at(1_000).as_slice(),
        std::slice::from_ref(&account_a)
    );
    assert_eq!(
        accounts_at(2_000).as_slice(),
        std::slice::from_ref(&account_b)
    );
    assert_eq!(
        accounts_at(3_000).as_slice(),
        std::slice::from_ref(&account_a)
    );
}

#[test]
fn live_usage_is_only_active_once_both_the_switch_and_onboarding_agree() {
    // The switch defaults on, but that alone must never be enough: the
    // credential read this feature depends on — and, on macOS, the Keychain
    // prompt it can trigger — must wait for onboarding to finish.
    let mut settings = AppSettings::default();
    assert!(settings.live_usage_enabled, "the default is on");
    assert!(!settings.onboarding_completed, "the default is not");
    assert!(!settings.live_usage_active());

    settings.onboarding_completed = true;
    assert!(settings.live_usage_active());

    settings.live_usage_enabled = false;
    assert!(!settings.live_usage_active(), "the opt-out still works");
}

#[test]
fn migrating_forward_drops_a_legacy_live_usage_off_row_so_the_new_default_applies() {
    // Before this build, `liveUsageEnabled` defaulted to false, and
    // `write_settings` writes every key on every save regardless of whether
    // it changed — so any install that ever saved settings at all (finishing
    // onboarding is enough) already carries an explicit `liveUsageEnabled|
    // false` row from that old default, indistinguishable from a reader who
    // deliberately opted out. antiburn has no public installs yet to protect
    // from losing one, so migration V3 just drops the row. Simulated here by
    // building a v2 database by hand — a real fresh `Store::open_in_memory`
    // would already be at the latest version and could not exercise the
    // migration path at all.
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..2] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute(
            "INSERT INTO setting (key, value) VALUES ('liveUsageEnabled', 'false')",
            [],
        )
        .unwrap();
    connection
        .pragma_update(None, "user_version", 2i64)
        .unwrap();

    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-migration-test").to_path_buf(),
    )
    .expect("migrates cleanly to the latest version");

    assert_eq!(
        store.schema_version().unwrap(),
        super::schema::MIGRATIONS.len() as i64
    );
    let remaining: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM setting WHERE key = 'liveUsageEnabled'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0, "the row is gone, not merely reinterpreted");
    assert!(
        store.settings().unwrap().live_usage_enabled,
        "with the legacy row gone, the read path falls through to the new default"
    );
}

#[test]
fn migrating_forward_renames_the_analytics_tables_and_keeps_their_rows() {
    // V1 through V8 created and used `usage_analytics_event` and
    // `usage_analytics_identity`. V9 (source generations) does not touch
    // them. V10 renames both tables to drop the "usage_" prefix, to match
    // the renamed Rust module and code. Built by hand up to V9 so only the
    // rename migration runs; a fresh `Store::open_in_memory` would already
    // be past it.
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..9] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute(
            "INSERT INTO usage_analytics_identity (id, install_id, minted_at)
             VALUES (1, 'test-install-id', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO usage_analytics_event (name, payload, queued_at)
             VALUES ('app_launched', '{}', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    connection
        .pragma_update(None, "user_version", 9i64)
        .unwrap();

    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-migration-test").to_path_buf(),
    )
    .expect("migrates cleanly to the latest version");

    assert_eq!(
        store.schema_version().unwrap(),
        super::schema::MIGRATIONS.len() as i64
    );
    let (install_id, event_count): (String, i64) = store
        .lock()
        .query_row(
            "SELECT install_id, (SELECT COUNT(*) FROM analytics_event) FROM analytics_identity",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the renamed tables carry the rows the old names held");
    assert_eq!(install_id, "test-install-id");
    assert_eq!(event_count, 1);
}

#[test]
fn migrating_from_every_prior_schema_version_reaches_the_current_head() {
    for start in 0..super::schema::MIGRATIONS.len() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        for &sql in &super::schema::MIGRATIONS[..start] {
            connection.execute_batch(sql).unwrap();
        }
        connection
            .pragma_update(None, "user_version", start as i64)
            .unwrap();

        let store = Store::from_connection(
            connection,
            Path::new("/tmp/antiburn-all-migrations-test").to_path_buf(),
        )
        .expect("migration reaches the head");
        assert_eq!(
            store.schema_version().unwrap(),
            super::schema::MIGRATIONS.len() as i64
        );
        let connection = store.lock();
        let added_columns: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('session')
                   WHERE name IN ('source_fingerprint', 'source_generation', 'started_at_epoch')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let projection_columns: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('session_analysis')
                   WHERE name IN ('analyzed_generation', 'parser_revision',
                                  'analyzer_revision', 'metrics_schema_revision')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(added_columns, 3, "start version {start}");
        assert_eq!(projection_columns, 4, "start version {start}");

        let diagnostics_column: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('session_evidence')
                   WHERE name = 'diagnostics_json'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(diagnostics_column, 0, "start version {start}");

        let index_names: Vec<String> = connection
            .prepare("SELECT name FROM pragma_index_list('session')")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            index_names
                .iter()
                .any(|name| name == "session_recency_coalesced"),
            "start version {start}: {index_names:?}"
        );
        assert!(
            !index_names.iter().any(|name| name == "session_recency"),
            "start version {start}: {index_names:?}"
        );
    }
}

#[test]
fn the_generation_increments_only_when_the_fingerprint_changes() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "generation");
    let mut record = session("generation", 1_000);
    record.source_fingerprint = Some("sv1:first".to_string());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let first = store
        .session_source_state(&key)
        .unwrap()
        .expect("source state");
    assert_eq!(first.source_generation, 1);

    record.source_fingerprint = Some("sv1:second".to_string());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let second = store
        .session_source_state(&key)
        .unwrap()
        .expect("source state");
    assert_eq!(second.source_generation, 2);
    assert_eq!(second.source_fingerprint.as_deref(), Some("sv1:second"));

    record.source_fingerprint = None;
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    let unreadable = store
        .session_source_state(&key)
        .unwrap()
        .expect("source state");
    assert_eq!(unreadable, second);
}

#[test]
fn a_new_source_generation_marks_session_evidence_pending() {
    let store = store();
    let mut record = seed_current_session_evidence(&store, "new-generation-evidence");
    record.source_fingerprint = Some("sv1:changed".into());

    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    assert_eq!(
        store
            .session_source_state(&record.key)
            .unwrap()
            .unwrap()
            .source_generation,
        2
    );
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
}

#[test]
fn a_changed_child_activity_cursor_requeues_the_same_parent_generation() {
    let store = store();
    let mut record = seed_current_session_evidence(&store, "changed-child-cursor");
    record.activity_cursor = "parent-and-child-v2".to_owned();

    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    assert_eq!(
        store
            .session_source_state(&record.key)
            .unwrap()
            .unwrap()
            .source_generation,
        1
    );
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
}

#[test]
fn marking_session_evidence_pending_keeps_the_last_completed_payload() {
    let store = store();
    let mut record = seed_current_session_evidence(&store, "preserved-evidence");
    let before = store.evidence(&record.key).unwrap().unwrap();
    record.source_fingerprint = Some("sv1:changed".into());

    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();

    let after = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(after.status, EvidenceStatus::Pending);
    assert_eq!(after.evidence_json, before.evidence_json);
    assert_eq!(after.analyzed_generation, before.analyzed_generation);
    assert_eq!(after.processed_fingerprint, before.processed_fingerprint);
    assert_eq!(after.parser_revision, before.parser_revision);
    assert_eq!(after.analyzer_revision, before.analyzer_revision);
    assert_eq!(
        after.evidence_schema_revision,
        before.evidence_schema_revision
    );
    assert_eq!(after.claim_fence, before.claim_fence);
    assert_eq!(after.retry_count, 0);
    assert_eq!(after.next_attempt_at_epoch, None);
    assert_eq!(after.last_error, None);
}

#[test]
fn an_unchanged_fingerprint_leaves_a_ready_session_evidence_row_alone() {
    assert_unchanged_session_evidence("unchanged-ready", "ready", 0, None, None, None, None);
}

#[test]
fn an_unchanged_fingerprint_leaves_a_processing_session_evidence_claim_alone() {
    assert_unchanged_session_evidence(
        "unchanged-processing",
        "processing",
        2,
        Some(100),
        Some(200),
        None,
        None,
    );
}

#[test]
fn an_unchanged_fingerprint_keeps_session_evidence_retry_backoff() {
    assert_unchanged_session_evidence(
        "unchanged-backoff",
        "pending",
        3,
        None,
        None,
        Some(300),
        Some("try later"),
    );
}

#[test]
fn an_unchanged_fingerprint_leaves_a_failed_session_evidence_row_failed() {
    assert_unchanged_session_evidence(
        "unchanged-failed",
        "failed",
        4,
        None,
        None,
        None,
        Some("terminal"),
    );
}

#[test]
fn reconciling_enrolls_a_session_evidence_row_for_an_upgraded_session() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &super::schema::MIGRATIONS[..10] {
        connection.execute_batch(sql).unwrap();
    }
    connection
        .execute(
            "INSERT INTO session (
                 environment_key, agent, session_id, source_kind, source_label,
                 first_seen_at, last_seen_at, source_fingerprint, source_generation)
             VALUES ('native', 'claude-code', 'upgrade-enrollment', 'file',
                     '/home/avery/.claude/projects/demo/upgrade-enrollment.jsonl',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                     'sv1:current', 1)",
            [],
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 10).unwrap();
    let store = Store::from_connection(
        connection,
        Path::new("/tmp/antiburn-evidence-enrollment-test").to_path_buf(),
    )
    .unwrap();
    let mut record = session("upgrade-enrollment", 1_000);
    record.source_fingerprint = Some("sv1:current".into());

    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(store.evidence(&record.key).unwrap().is_none());
    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], projection_revisions())
            .unwrap(),
        1
    );

    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
    assert_eq!(
        store
            .session_source_state(&record.key)
            .unwrap()
            .unwrap()
            .source_generation,
        1
    );
    assert_eq!(
        store
            .claim_next_evidence(&["claude-code"], 100, 60)
            .unwrap()
            .unwrap()
            .key,
        record.key
    );
}

/// Seam R3c: a worker pass publishes rows and per-source summaries, and
/// `analysis::analysis_from_rows` rebuilds the session-detail payload from
/// `store` alone. `record.source_label` names a transcript path that does
/// not exist on disk, so a payload here proves the replay path reads no
/// transcript — a live parse of that path would find nothing.
#[tokio::test]
async fn analysis_from_rows_serves_a_published_pass_without_reading_a_transcript() {
    let store = store();
    let mut record = session("rows-replay-parent", 1_000);
    record.source_fingerprint = Some("sv1:rows-replay-parent".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    // `published_evidence_pass` (used elsewhere in this module) writes its
    // rows to a standalone `MemoryTurnRowStore`, disconnected from `store`'s
    // own tables — fine for the evidence-JSON assertions those tests make,
    // but this test needs rows `store.published_turn_rows` can actually
    // read back, so the runner fences its rows into `store` itself, exactly
    // as `insights_worker::run_record_pass` does in production.
    let store_for_runner = store.clone();
    let runner = move |record: &SessionRecord,
                       _signal: crate::analysis::PassSignal,
                       claim_fence: i64| {
        let row_store: Arc<dyn TurnRowStore> = Arc::new(FencedTurnRowStore::new(
            store_for_runner.clone(),
            record.key.clone(),
            claim_fence,
        ));
        let mut pass = crate::analysis::evidence_pass_with_turn_rows(
            &[antiburn_local::analysis::SessionInput {
                agent: "claude".into(),
                session_id: record.key.session_id.clone(),
                source: antiburn_local::analysis::RawSource::Jsonl(
                    r#"{"type":"assistant","timestamp":100,"message":{"id":"m","role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":2,"output_tokens":3},"content":[]}}
"#
                    .into(),
                ),
            }],
            &|| false,
            Some(row_store),
        );
        pass.analysis.fingerprint = record
            .source_fingerprint
            .clone()
            .unwrap_or_else(|| crate::analysis::MISSING_FINGERPRINT.into());
        Box::pin(async move { pass }) as crate::insights_worker::PassFuture
    };
    assert!(
        crate::insights_worker::process_next(
            &store,
            &crate::insights_worker::WorkerHandle::default(),
            &|| 1_100,
            &runner,
            &|_| {},
        )
        .await
        .unwrap()
    );
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Ready
    );

    let replayed = crate::analysis::analysis_from_rows(
        &store,
        &record.key,
        &record.key.session_id,
        "claude-code",
    )
    .expect("a ready, published pass replays from rows");

    assert!(replayed.metrics.is_some());
    assert_eq!(replayed.fingerprint, "sv1:rows-replay-parent");
    assert_eq!(replayed.cost, replayed.top_level_cost);
    assert!(replayed.source_path.is_none());
}

/// The published_fence carve-out this whole change exists for: an
/// actively-written session is requeued on nearly every fingerprint check,
/// which used to make `published_turn_rows` — and so `analysis_from_rows`
/// — return `None` on every drilldown open, however many passes had
/// already published. Requeuing flips `status` back to `pending` without
/// touching `published_fence`, so the last winning pass's rows and analysis
/// stay fully replayable while the fresh pass is only queued.
#[tokio::test]
async fn analysis_from_rows_still_serves_a_published_pass_after_a_requeue() {
    let store = store();
    let mut record = session("rows-replay-requeued", 1_000);
    record.source_fingerprint = Some("sv1:rows-replay-requeued".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    let store_for_runner = store.clone();
    let runner = move |record: &SessionRecord,
                       _signal: crate::analysis::PassSignal,
                       claim_fence: i64| {
        let row_store: Arc<dyn TurnRowStore> = Arc::new(FencedTurnRowStore::new(
            store_for_runner.clone(),
            record.key.clone(),
            claim_fence,
        ));
        let mut pass = crate::analysis::evidence_pass_with_turn_rows(
            &[antiburn_local::analysis::SessionInput {
                agent: "claude".into(),
                session_id: record.key.session_id.clone(),
                source: antiburn_local::analysis::RawSource::Jsonl(
                    r#"{"type":"assistant","timestamp":100,"message":{"id":"m","role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":2,"output_tokens":3},"content":[]}}
"#
                    .into(),
                ),
            }],
            &|| false,
            Some(row_store),
        );
        pass.analysis.fingerprint = record
            .source_fingerprint
            .clone()
            .unwrap_or_else(|| crate::analysis::MISSING_FINGERPRINT.into());
        Box::pin(async move { pass }) as crate::insights_worker::PassFuture
    };
    assert!(
        crate::insights_worker::process_next(
            &store,
            &crate::insights_worker::WorkerHandle::default(),
            &|| 1_100,
            &runner,
            &|_| {},
        )
        .await
        .unwrap()
    );

    // The transcript grew: the drilldown's own nudge requeues the session
    // for a fresh pass, exactly as `commands::nudge_if_evidence_stale` does.
    store.requeue_session_evidence(&record.key).unwrap();
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );

    let replayed = crate::analysis::analysis_from_rows(
        &store,
        &record.key,
        &record.key.session_id,
        "claude-code",
    )
    .expect("a requeued session still replays its last published rows");

    assert!(replayed.metrics.is_some());
    assert_eq!(replayed.fingerprint, "sv1:rows-replay-requeued");
}

/// Before a worker pass has ever published anything, `published_fence` is
/// `NULL`, so `analysis_from_rows` finds no row set and returns `None` — the
/// command switch's own signal that this session's drilldown is still
/// pending. The worker fills the gap on its own next pass; the command no
/// longer re-parses the transcript in-process to answer this call. A claim
/// in flight over an *already-published* session is a different case —
/// see `published_turn_rows_serves_the_last_published_fence_while_a_newer_claim_is_in_flight`
/// in `publish_tests`.
#[test]
fn analysis_from_rows_returns_none_before_anything_was_ever_published() {
    let store = store();
    let (_, claim) = claimed_projection(&store, "rows-not-ready", 100, 60);
    assert_eq!(
        store.evidence(&claim.key).unwrap().unwrap().status,
        EvidenceStatus::Processing
    );

    assert!(
        crate::analysis::analysis_from_rows(
            &store,
            &claim.key,
            &claim.key.session_id,
            "claude-code"
        )
        .is_none()
    );
}

/// `published_turn_rows`' widening (R5 part 1) reaches `analysis_from_rows`
/// too: a pass published with status `unsupported` is exactly as replayable
/// as one published `ready`, because `Unsupported` is an insights verdict
/// (no detector was eligible), not a parse-quality one — the rows and the
/// `session_analysis` record a winning pass wrote are complete either way.
#[test]
fn analysis_from_rows_serves_a_pass_published_unsupported() {
    let store = store();
    let (mut record, claim) = claimed_projection(&store, "s1", 100, 60);
    // A row whose own `source_key` names the parent session, so
    // `metrics_by_source` groups it under `record.key.session_id` the way
    // `analysis_from_rows` expects for the parent entry.
    record.source_summaries_json = Some("{}".into());
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0)]).unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Unsupported, "{}".into());
    assert!(
        store
            .publish_projections(&record, None, &completion, &[])
            .unwrap()
    );
    assert_eq!(
        store.evidence(&key).unwrap().unwrap().status,
        EvidenceStatus::Unsupported
    );

    let replayed =
        crate::analysis::analysis_from_rows(&store, &key, &key.session_id, "claude-code")
            .expect("an unsupported, published pass still replays from rows");

    assert!(replayed.metrics.is_some());
}

/// The drilldown's rows-replay path requeues a session whose stored
/// fingerprint no longer matches the live transcript
/// (`commands::nudge_if_evidence_stale`). This proves the underlying store
/// mechanism it calls: requeuing a `ready` row moves it back to `pending`
/// for the worker's next pass.
#[test]
fn requeue_session_evidence_marks_a_ready_session_pending() {
    let store = store();
    let record = seed_current_session_evidence(&store, "requeue-on-mismatch");
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Ready
    );

    store.requeue_session_evidence(&record.key).unwrap();

    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
}

#[tokio::test]
async fn reprocessing_a_revision_one_row_leaves_no_placeholder_in_stored_evidence_json() {
    let store = store();
    let record = seed_revision_one_placeholder(&store, "revision-placeholder-success");
    let revisions = ProjectionRevisions {
        evidence_schema_revision: 2,
        ..projection_revisions()
    };
    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], revisions)
            .unwrap(),
        1
    );
    let pending = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(pending.status, EvidenceStatus::Pending);
    assert_eq!(
        pending.evidence_json.as_deref(),
        Some("{\"state\":\"unimplemented\"}")
    );

    let runner = |record: &SessionRecord, _signal: crate::analysis::PassSignal, _: i64| {
        let pass = published_evidence_pass(record);
        Box::pin(async move { pass }) as crate::insights_worker::PassFuture
    };
    assert!(
        crate::insights_worker::process_next(
            &store,
            &crate::insights_worker::WorkerHandle::default(),
            &|| 1_100,
            &runner,
            &|_| {},
        )
        .await
        .unwrap()
    );

    let ready = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(ready.status, EvidenceStatus::Ready);
    assert_eq!(ready.evidence_schema_revision, Some(12));
    assert!(!ready.evidence_json.unwrap().contains("unimplemented"));
}

#[tokio::test]
async fn a_terminal_failure_clears_an_outdated_placeholder_payload() {
    let store = store();
    let record = seed_revision_one_placeholder(&store, "revision-placeholder-failed");
    let revisions = ProjectionRevisions {
        evidence_schema_revision: 2,
        ..projection_revisions()
    };
    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], revisions)
            .unwrap(),
        1
    );
    let runner = |_record: &SessionRecord, _signal: crate::analysis::PassSignal, _: i64| {
        Box::pin(async {
            crate::analysis::EvidencePass {
                analysis: crate::analysis::SessionAnalysis::unavailable(),
                evidence: None,
                outcome: crate::analysis::PassOutcome::SourceMissing,
            }
        }) as crate::insights_worker::PassFuture
    };
    assert!(
        crate::insights_worker::process_next(
            &store,
            &crate::insights_worker::WorkerHandle::default(),
            &|| 1_100,
            &runner,
            &|_| {},
        )
        .await
        .unwrap()
    );

    let failed = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(failed.status, EvidenceStatus::Failed);
    assert_eq!(failed.evidence_schema_revision, Some(12));
    assert!(failed.evidence_json.is_none());
}

#[test]
fn a_catalog_change_requeues_no_session_evidence() {
    let store = store();
    let record = seed_current_session_evidence(&store, "catalog-no-requeue");
    store
        .save_analysis(
            &AnalysisRecord {
                key: record.key.clone(),
                model_breakdown_json: "{}".into(),
                inclusive_models_json: "[]".into(),
                initial_context_json: None,
                source_summaries_json: None,
                provider_hints_json: None,
                source_fingerprint: "sv1:current".into(),
                pricing_generation: 2,
                analyzed_generation: 1,
                parser_revision: 1,
                analyzer_revision: 1,
                metrics_schema_revision: 1,
            },
            None,
        )
        .unwrap();
    let before = store.evidence(&record.key).unwrap().unwrap();

    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], projection_revisions())
            .unwrap(),
        0
    );
    assert_eq!(store.evidence(&record.key).unwrap().unwrap(), before);
}

#[test]
fn reconciling_session_evidence_skips_a_disabled_agent() {
    let store = store();
    let claude = session("enabled-evidence", 1_000);
    let mut codex = session("disabled-evidence", 1_000);
    codex.key.agent = "codex".into();
    store
        .upsert_sessions(
            &[claude.clone(), codex.clone()],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .lock()
        .execute("DELETE FROM session_evidence", [])
        .unwrap();

    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], projection_revisions())
            .unwrap(),
        1
    );
    assert!(store.evidence(&claude.key).unwrap().is_some());
    assert!(store.evidence(&codex.key).unwrap().is_none());
}

#[test]
fn every_agent_kind_is_in_the_evidence_cohort_and_gets_a_row() {
    // The cohort now covers every AgentKind (crate::agents::evidence_cohort
    // derives from AgentKind::ALL), so upserting a session for any slug
    // queues an evidence row. No agent is outside the cohort any more.
    let store = store();
    let cohort = crate::agents::evidence_cohort();
    let sessions: Vec<SessionRecord> = cohort
        .iter()
        .map(|slug| {
            let mut record = session(&format!("cohort-{slug}"), 1_000);
            record.key.agent = (*slug).to_string();
            record
        })
        .collect();

    store.upsert_sessions(&sessions, &cohort).unwrap();

    for record in &sessions {
        assert!(
            store.evidence(&record.key).unwrap().is_some(),
            "{} should get an evidence row",
            record.key.agent
        );
    }
}

#[test]
fn a_terminal_failure_survives_two_startup_reconciles() {
    let store = store();
    let (_, claim) = claimed_projection(&store, "terminal-reconcile", 100, 60);
    assert!(
        store
            .fail_evidence(
                &claim,
                EvidenceFailure::Failed {
                    revisions: projection_revisions(),
                },
                "source-missing",
            )
            .unwrap()
    );

    for _ in 0..2 {
        assert_eq!(
            store
                .reconcile_evidence_revisions(&["claude-code"], projection_revisions())
                .unwrap(),
            0
        );
        assert_eq!(
            store.evidence(&claim.key).unwrap().unwrap().status,
            EvidenceStatus::Failed
        );
        assert!(
            store
                .claim_next_evidence(&["claude-code"], 200, 60)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn a_returned_source_requeues_with_an_unchanged_fingerprint() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "returned-source", 100, 60);
    assert!(
        store
            .fail_evidence(
                &claim,
                EvidenceFailure::Failed {
                    revisions: projection_revisions(),
                },
                "source-missing",
            )
            .unwrap()
    );
    let generation = claim.source_generation;
    let mut returned = session("returned-source", 1_000);
    returned.source_fingerprint = Some(record.source_fingerprint);

    store
        .upsert_sessions(&[returned], &crate::agents::evidence_cohort())
        .unwrap();

    let evidence = store.evidence(&claim.key).unwrap().unwrap();
    assert_eq!(
        store
            .session_source_state(&claim.key)
            .unwrap()
            .unwrap()
            .source_generation,
        generation
    );
    assert_eq!(evidence.status, EvidenceStatus::Pending);
    assert_eq!(evidence.retry_count, 0);
}

#[test]
fn an_abandoned_processing_row_requeues_at_startup() {
    let store = store();
    let (_, claim) = claimed_projection(&store, "abandoned-startup", 100, 60);

    assert_eq!(
        store
            .reconcile_evidence_revisions(&["claude-code"], projection_revisions())
            .unwrap(),
        1
    );
    assert_eq!(
        store.evidence(&claim.key).unwrap().unwrap().status,
        EvidenceStatus::Pending
    );
}

#[test]
fn the_first_session_evidence_claim_excludes_a_second_claim() {
    let store = store();
    let mut record = session("exclusive-claim", 1_000);
    record.source_fingerprint = Some("sv1:exclusive".into());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();

    let first = store
        .claim_next_evidence(&["claude-code"], 100, 60)
        .unwrap()
        .unwrap();

    assert_eq!(first.claim_fence, 1);
    assert!(
        store
            .claim_next_evidence(&["claude-code"], 100, 60)
            .unwrap()
            .is_none()
    );
}

#[test]
fn reclaiming_an_abandoned_session_evidence_row_raises_the_fence() {
    let store = store();
    let mut record = session("reclaimed-claim", 1_000);
    record.source_fingerprint = Some("sv1:reclaim".into());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    let first = store
        .claim_next_evidence(&["claude-code"], 100, 10)
        .unwrap()
        .unwrap();

    let reclaimed = store
        .claim_next_evidence(&["claude-code"], 110, 10)
        .unwrap()
        .unwrap();

    assert_eq!(reclaimed.key, first.key);
    assert_eq!(reclaimed.source_generation, first.source_generation);
    assert_eq!(reclaimed.claim_fence, first.claim_fence + 1);
}

#[test]
fn a_late_session_evidence_transition_is_rejected() {
    let store = store();
    let mut record = session("late-transition", 1_000);
    record.source_fingerprint = Some("sv1:late".into());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    let first = store
        .claim_next_evidence(&["claude-code"], 100, 10)
        .unwrap()
        .unwrap();
    let current = store
        .claim_next_evidence(&["claude-code"], 110, 10)
        .unwrap()
        .unwrap();
    let before = store.evidence(&current.key).unwrap().unwrap();

    assert!(
        !store
            .fail_evidence(
                &first,
                EvidenceFailure::Failed {
                    revisions: projection_revisions()
                },
                "late"
            )
            .unwrap()
    );
    assert_eq!(store.evidence(&current.key).unwrap().unwrap(), before);
}

#[test]
fn a_stale_generation_rejects_a_session_evidence_lease_renewal() {
    let store = store();
    let mut record = session("stale-renewal", 1_000);
    record.source_fingerprint = Some("sv1:renewal".into());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    let claim = store
        .claim_next_evidence(&["claude-code"], 100, 60)
        .unwrap()
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();
    let before = store.evidence(&record.key).unwrap().unwrap();

    assert!(!store.renew_evidence_lease(&claim, 110, 60).unwrap());
    assert_eq!(store.evidence(&record.key).unwrap().unwrap(), before);
}

#[test]
fn a_stale_generation_rejects_a_session_evidence_failure() {
    let store = store();
    let mut record = session("stale-failure", 1_000);
    record.source_fingerprint = Some("sv1:failure".into());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    let claim = store
        .claim_next_evidence(&["claude-code"], 100, 60)
        .unwrap()
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();
    let before = store.evidence(&record.key).unwrap().unwrap();

    assert!(
        !store
            .fail_evidence(
                &claim,
                EvidenceFailure::Failed {
                    revisions: projection_revisions()
                },
                "stale"
            )
            .unwrap()
    );
    assert_eq!(store.evidence(&record.key).unwrap().unwrap(), before);
}

#[test]
fn a_session_evidence_claim_is_not_eligible_before_its_next_attempt() {
    let store = store();
    let mut record = session("delayed-claim", 1_000);
    record.source_fingerprint = Some("sv1:delayed".into());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET next_attempt_at_epoch = 200
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();

    assert!(
        store
            .claim_next_evidence(&["claude-code"], 199, 60)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .claim_next_evidence(&["claude-code"], 200, 60)
            .unwrap()
            .is_some()
    );
}

#[test]
fn failing_session_evidence_with_retry_returns_it_to_pending_with_backoff() {
    let store = store();
    let mut record = session("retry-failure", 1_000);
    record.source_fingerprint = Some("sv1:retry".into());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    let claim = store
        .claim_next_evidence(&["claude-code"], 100, 60)
        .unwrap()
        .unwrap();

    assert!(
        store
            .fail_evidence(
                &claim,
                EvidenceFailure::Retry {
                    next_attempt_at_epoch: 300,
                },
                "try later",
            )
            .unwrap()
    );

    let evidence = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(evidence.status, EvidenceStatus::Pending);
    assert_eq!(evidence.retry_count, 1);
    assert_eq!(evidence.next_attempt_at_epoch, Some(300));
    assert_eq!(evidence.last_error.as_deref(), Some("try later"));
    assert_eq!(evidence.claimed_at_epoch, None);
    assert_eq!(evidence.lease_expires_at_epoch, None);
    assert!(
        store
            .claim_next_evidence(&["claude-code"], 299, 60)
            .unwrap()
            .is_none()
    );
}

#[test]
fn failing_session_evidence_terminally_marks_it_failed() {
    let store = store();
    let mut record = session("terminal-failure", 1_000);
    record.source_fingerprint = Some("sv1:terminal".into());
    store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    let claim = store
        .claim_next_evidence(&["claude-code"], 100, 60)
        .unwrap()
        .unwrap();

    assert!(
        store
            .fail_evidence(
                &claim,
                EvidenceFailure::Failed {
                    revisions: projection_revisions()
                },
                "terminal"
            )
            .unwrap()
    );

    let evidence = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(evidence.status, EvidenceStatus::Failed);
    assert_eq!(evidence.retry_count, 1);
    assert_eq!(evidence.next_attempt_at_epoch, None);
    assert_eq!(evidence.last_error.as_deref(), Some("terminal"));
    assert_ne!(evidence.status, EvidenceStatus::Ready);
    assert_ne!(evidence.status, EvidenceStatus::Unsupported);
}

#[test]
fn publishing_session_evidence_writes_both_projections_and_the_start_time() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "publish-both", 100, 60);
    let completion = evidence_completion(
        &claim,
        PublishedEvidence::Unsupported,
        "{\"unsupported\":true}".into(),
    );

    assert!(
        store
            .publish_projections(&record, Some(50), &completion, &[])
            .unwrap()
    );

    assert_eq!(store.analysis(&record.key).unwrap(), Some(record.clone()));
    assert_eq!(
        store
            .session_source_state(&record.key)
            .unwrap()
            .unwrap()
            .started_at_epoch,
        Some(50)
    );
    let evidence = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(evidence.status, EvidenceStatus::Unsupported);
    assert_eq!(
        evidence.analyzed_generation,
        Some(record.analyzed_generation)
    );
    assert_eq!(
        evidence.processed_fingerprint.as_deref(),
        Some(record.source_fingerprint.as_str())
    );
    assert_eq!(evidence.parser_revision, Some(record.parser_revision));
    assert_eq!(evidence.analyzer_revision, Some(record.analyzer_revision));
    assert_eq!(evidence.evidence_schema_revision, Some(1));
    assert_eq!(evidence.retry_count, 0);
    assert_eq!(evidence.last_error, None);
    assert_eq!(evidence.claimed_at_epoch, None);
    assert_eq!(evidence.lease_expires_at_epoch, None);
    assert_eq!(evidence.next_attempt_at_epoch, None);
    assert!(evidence.analyzed_at_epoch.is_some());
}

#[test]
fn published_session_evidence_and_analysis_describe_the_same_pass() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "same-pass", 100, 60);
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());

    assert!(
        store
            .publish_projections(&record, None, &completion, &[])
            .unwrap()
    );

    let analysis = store.analysis(&record.key).unwrap().unwrap();
    let evidence = store.evidence(&record.key).unwrap().unwrap();
    assert_eq!(
        evidence.analyzed_generation,
        Some(analysis.analyzed_generation)
    );
    assert_eq!(
        evidence.processed_fingerprint,
        Some(analysis.source_fingerprint)
    );
    assert_eq!(evidence.parser_revision, Some(analysis.parser_revision));
    assert_eq!(evidence.analyzer_revision, Some(analysis.analyzer_revision));
}

#[test]
fn a_stale_generation_publishes_no_session_evidence_and_no_analysis() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "stale-publish-generation", 100, 60);
    let sentinel = projection_record(record.key.clone(), "sv1:sentinel", 0);
    store.save_analysis(&sentinel, Some(77)).unwrap();
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )
        .unwrap();
    let source_before = store.session_source_state(&record.key).unwrap().unwrap();
    let analysis_before = store.analysis(&record.key).unwrap().unwrap();
    let evidence_before = store.evidence(&record.key).unwrap().unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());

    assert!(
        !store
            .publish_projections(&record, Some(88), &completion, &[])
            .unwrap()
    );

    assert_eq!(
        store.session_source_state(&record.key).unwrap().unwrap(),
        source_before
    );
    assert_eq!(
        store.analysis(&record.key).unwrap().unwrap(),
        analysis_before
    );
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap(),
        evidence_before
    );
}

#[test]
fn a_stale_fence_publishes_no_session_evidence_and_no_analysis() {
    let store = store();
    let (record, first_claim) = claimed_projection(&store, "stale-publish-fence", 100, 10);
    let current_claim = store
        .claim_next_evidence(&["claude-code"], 110, 60)
        .unwrap()
        .unwrap();
    assert!(current_claim.claim_fence > first_claim.claim_fence);
    let sentinel = projection_record(record.key.clone(), "sv1:sentinel", 0);
    store.save_analysis(&sentinel, Some(77)).unwrap();
    let source_before = store.session_source_state(&record.key).unwrap().unwrap();
    let analysis_before = store.analysis(&record.key).unwrap().unwrap();
    let evidence_before = store.evidence(&record.key).unwrap().unwrap();
    let completion = evidence_completion(&first_claim, PublishedEvidence::Ready, "{}".into());

    assert!(
        !store
            .publish_projections(&record, Some(88), &completion, &[])
            .unwrap()
    );

    assert_eq!(
        store.session_source_state(&record.key).unwrap().unwrap(),
        source_before
    );
    assert_eq!(
        store.analysis(&record.key).unwrap().unwrap(),
        analysis_before
    );
    assert_eq!(
        store.evidence(&record.key).unwrap().unwrap(),
        evidence_before
    );
}

#[test]
fn a_stale_claim_cannot_change_projections_or_relations() {
    let store = store();
    let (record, first_claim) = claimed_projection(&store, "stale-all-projections", 100, 10);
    let current_claim = store
        .claim_next_evidence(&["claude-code"], 110, 60)
        .unwrap()
        .unwrap();
    let current_completion = evidence_completion(
        &current_claim,
        PublishedEvidence::Ready,
        "{\"new\":true}".into(),
    );
    let current_relations = [RelationRecord {
        kind: RelationKind::Subagent,
        related_id: "new-child".into(),
        label: Some("New child".into()),
    }];
    assert!(
        store
            .publish_projections(&record, None, &current_completion, &current_relations)
            .unwrap()
    );
    let analysis_before = store.analysis(&record.key).unwrap();
    let evidence_before = store.evidence(&record.key).unwrap();
    let relations_before = store.relations(&record.key).unwrap();
    let mut stale_record = record.clone();
    stale_record.model_breakdown_json = "{\"old\":true}".into();
    let stale_completion = evidence_completion(
        &first_claim,
        PublishedEvidence::Ready,
        "{\"old\":true}".into(),
    );
    let stale_relations = [RelationRecord {
        kind: RelationKind::Subagent,
        related_id: "old-child".into(),
        label: Some("Old child".into()),
    }];

    assert!(
        !store
            .publish_projections(&stale_record, None, &stale_completion, &stale_relations)
            .unwrap()
    );
    assert_eq!(store.analysis(&record.key).unwrap(), analysis_before);
    assert_eq!(store.evidence(&record.key).unwrap(), evidence_before);
    assert_eq!(store.relations(&record.key).unwrap(), relations_before);
}

#[test]
fn publication_replaces_subagent_relations_in_one_transaction() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "relation-publication", 100, 60);
    store
        .replace_relations(
            &record.key,
            RelationKind::ForkParent,
            &[RelationRecord {
                kind: RelationKind::ForkParent,
                related_id: "fork-parent".into(),
                label: None,
            }],
        )
        .unwrap();
    store
        .replace_relations(
            &record.key,
            RelationKind::Subagent,
            &[RelationRecord {
                kind: RelationKind::Subagent,
                related_id: "old-child".into(),
                label: None,
            }],
        )
        .unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());
    let new_relations = [RelationRecord {
        kind: RelationKind::Subagent,
        related_id: "new-child".into(),
        label: Some("New child".into()),
    }];

    assert!(
        store
            .publish_projections(&record, None, &completion, &new_relations)
            .unwrap()
    );
    assert_eq!(
        store.relations(&record.key).unwrap(),
        vec![
            RelationRecord {
                kind: RelationKind::ForkParent,
                related_id: "fork-parent".into(),
                label: None,
            },
            new_relations[0].clone(),
        ]
    );
}

#[test]
fn deleting_a_session_removes_its_session_evidence() {
    let store = store();
    let mut record = session("delete-evidence", 1_000);
    record.source_fingerprint = Some("sv1:delete".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    assert!(store.evidence(&record.key).unwrap().is_some());

    assert!(store.delete_session(&record.key).unwrap());

    assert!(store.evidence(&record.key).unwrap().is_none());
}

#[test]
fn clearing_local_session_data_removes_every_session_evidence_row() {
    let store = store();
    let mut first = session("clear-evidence-one", 1_000);
    first.source_fingerprint = Some("sv1:clear-one".into());
    let mut second = session("clear-evidence-two", 1_000);
    second.source_fingerprint = Some("sv1:clear-two".into());
    store
        .upsert_sessions(&[first, second], &crate::agents::evidence_cohort())
        .unwrap();

    assert_eq!(store.clear_local_session_data().unwrap(), 2);

    let count: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM session_evidence", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn a_session_evidence_payload_round_trips_through_the_store() {
    use antiburn_local::analysis::{
        EvidenceSource, SessionEvidence, SessionEvidenceAccumulator, SourceCapabilities,
        SourceKind, TurnFacts,
    };

    let store = store();
    let (record, claim) = claimed_projection(&store, "payload-round-trip", 100, 60);
    let payload = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude-code".into(),
        session_id: "payload-round-trip".into(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    })
    .evidence(&TurnFacts::default());
    let payload_json = serde_json::to_string(&payload).unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, payload_json);

    assert!(
        store
            .publish_projections(&record, None, &completion, &[])
            .unwrap()
    );

    let stored_json = store
        .evidence(&record.key)
        .unwrap()
        .unwrap()
        .evidence_json
        .unwrap();
    let restored: SessionEvidence = serde_json::from_str(&stored_json).unwrap();
    assert_eq!(restored, payload);
}

#[test]
fn started_at_epoch_stays_null_through_scan_upserts() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "no-start");
    let mut record = session("no-start", 1_000);
    record.source_fingerprint = Some("sv1:first".to_string());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    record.source_fingerprint = Some("sv1:second".to_string());
    store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();

    let state = store
        .session_source_state(&key)
        .unwrap()
        .expect("source state");
    assert_eq!(state.started_at_epoch, None);
}

#[test]
fn internal_values_round_trip_and_stay_out_of_settings() {
    let store = store();
    assert_eq!(store.internal_value("internal:diskSpaceLowFiredMs"), None);

    store.set_internal_value("internal:diskSpaceLowFiredMs", "1723600000000");
    assert_eq!(
        store
            .internal_value("internal:diskSpaceLowFiredMs")
            .as_deref(),
        Some("1723600000000")
    );

    // Overwrite, not append: one row per key, like every setting.
    store.set_internal_value("internal:diskSpaceLowFiredMs", "1723600001000");
    assert_eq!(
        store
            .internal_value("internal:diskSpaceLowFiredMs")
            .as_deref(),
        Some("1723600001000")
    );

    // Internal rows share the table but never surface as preferences.
    assert_eq!(store.settings().unwrap(), AppSettings::default());
}

#[test]
fn evidence_backlog_counts_split_pending_from_processing_within_one_environment() {
    let store = store();
    for session_id in ["backlog-one", "backlog-two", "backlog-three"] {
        let mut record = session(session_id, 1_000);
        record.source_fingerprint = Some(format!("sv1:{session_id}"));
        store
            .upsert_sessions(
                std::slice::from_ref(&record),
                &crate::agents::evidence_cohort(),
            )
            .unwrap();
    }
    // One claim moves one native row from pending to processing.
    let claim = store
        .claim_next_evidence(&["claude-code"], 10, 60)
        .unwrap()
        .unwrap();
    assert_eq!(claim.key.environment_key, "native");

    // A backlog in another environment must not leak into the native counts.
    let mut other = session("backlog-wsl", 1_000);
    other.key.environment_key = "wsl:ubuntu".into();
    other.source_fingerprint = Some("sv1:backlog-wsl".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&other),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();

    let native = store.evidence_backlog_counts("native").unwrap();
    assert_eq!(native.pending, 2);
    assert_eq!(native.processing, 1);

    let wsl = store.evidence_backlog_counts("wsl:ubuntu").unwrap();
    assert_eq!(wsl.pending, 1);
    assert_eq!(wsl.processing, 0);
}

/* --------------------------------------------------------------------
 * Turn rows
 * ----------------------------------------------------------------- */

fn turn_row(turn_index: u64) -> TurnRow {
    TurnRow {
        source_key: "s1".into(),
        thread_id: "s1".into(),
        turn_index,
        scope: TurnScope::Main,
        child_id: None,
        role: "assistant",
        ts_ms: Some(1_000 + turn_index as i64),
        model: Some("claude-opus-4-6".into()),
        effort: None,
        speed: None,
        input_tokens: 10,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        output_tokens: 5,
        is_compaction_boundary: false,
        message_id: None,
        uuid: None,
        parent_uuid: None,
        compaction_trigger: None,
        compaction_pre_tokens: None,
        compaction_post_tokens: None,
        has_thinking: false,
        last_tool: None,
        subagent_launches: 0,
        content: Vec::new(),
    }
}

#[test]
fn the_migration_ladder_reaches_the_turn_row_schema() {
    // Pinned so this test fails loudly if a future migration is appended
    // without also being counted here — the number is the whole point of
    // the assertion, not an incidental detail.
    assert_eq!(super::schema::MIGRATIONS.len(), 27);

    let store = store();
    assert_eq!(store.schema_version().unwrap(), 27);
}

#[test]
fn a_fenced_turn_row_writer_inserts_rows_the_store_can_count() {
    let store = store();
    let mut record = session("turn-rows-writer", 1_000);
    record.source_fingerprint = Some("sv1:turn-rows-writer".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let key = record.key.clone();

    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), 7);
    writer.write_turn_rows(&[turn_row(0), turn_row(1)]).unwrap();

    let connection = store.lock();
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), 7).unwrap(),
        2
    );
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), 8).unwrap(),
        0
    );
}

#[test]
fn publishing_evidence_keeps_only_the_current_fence_turn_rows() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "publish-current-fence", 100, 60);
    let key = record.key.clone();

    // A row left over from an earlier, superseded pass under a stale fence.
    {
        let connection = store.lock();
        insert_turn_rows(
            &connection,
            &turn_session_key(&key),
            claim.claim_fence - 1,
            &[turn_row(0)],
        )
        .unwrap();
    }
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0), turn_row(1)]).unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());

    assert!(
        store
            .publish_projections(&record, None, &completion, &[])
            .unwrap()
    );

    let connection = store.lock();
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), claim.claim_fence - 1).unwrap(),
        0
    );
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), claim.claim_fence).unwrap(),
        2
    );
}

#[test]
fn a_lost_publish_race_deletes_only_its_own_fences_turn_rows() {
    let store = store();
    let (record, claim) = claimed_projection(&store, "lost-race-turn-rows", 100, 60);
    let key = record.key.clone();
    // A row from a still-current, earlier pass. A lost race must not touch
    // rows it does not own.
    {
        let connection = store.lock();
        insert_turn_rows(
            &connection,
            &turn_session_key(&key),
            claim.claim_fence - 1,
            &[turn_row(0)],
        )
        .unwrap();
    }
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), claim.claim_fence);
    writer.write_turn_rows(&[turn_row(0)]).unwrap();
    // Bumping the source generation makes the fenced UPDATE inside
    // `publish_projections` affect zero rows — the same "lost the race"
    // shape `a_stale_generation_publishes_no_session_evidence_and_no_analysis`
    // exercises for the evidence and analysis projections.
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
        )
        .unwrap();
    let completion = evidence_completion(&claim, PublishedEvidence::Ready, "{}".into());

    assert!(
        !store
            .publish_projections(&record, None, &completion, &[])
            .unwrap()
    );

    let connection = store.lock();
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), claim.claim_fence).unwrap(),
        0
    );
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), claim.claim_fence - 1).unwrap(),
        1
    );
}

#[test]
fn deleting_a_session_removes_its_turn_rows() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "turn-rows-delete");
    store
        .upsert_sessions(
            &[session("turn-rows-delete", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    {
        let connection = store.lock();
        insert_turn_rows(&connection, &turn_session_key(&key), 1, &[turn_row(0)]).unwrap();
    }

    assert!(store.delete_session(&key).unwrap());

    let connection = store.lock();
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), 1).unwrap(),
        0
    );
}

#[test]
fn clearing_local_session_data_removes_every_turn_row() {
    let store = store();
    let mut first = session("clear-turn-rows-one", 1_000);
    first.source_fingerprint = Some("sv1:clear-turn-rows-one".into());
    let mut second = session("clear-turn-rows-two", 1_000);
    second.source_fingerprint = Some("sv1:clear-turn-rows-two".into());
    store
        .upsert_sessions(&[first, second], &crate::agents::evidence_cohort())
        .unwrap();
    {
        let connection = store.lock();
        insert_turn_rows(
            &connection,
            &TurnSessionKey {
                environment_key: "native",
                agent: "claude-code",
                session_id: "clear-turn-rows-one",
            },
            1,
            &[turn_row(0)],
        )
        .unwrap();
        insert_turn_rows(
            &connection,
            &TurnSessionKey {
                environment_key: "native",
                agent: "claude-code",
                session_id: "clear-turn-rows-two",
            },
            1,
            &[turn_row(0)],
        )
        .unwrap();
    }

    assert_eq!(store.clear_local_session_data().unwrap(), 2);

    let count: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM turn", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn deleting_the_session_row_directly_cascades_to_its_turn_rows() {
    // `delete_session` deletes `turn` rows explicitly (see its doc comment),
    // matching how it already treats `session_evidence`. This test instead
    // deletes the `session` row with raw SQL, bypassing that explicit
    // delete, to prove the migrated schema's `ON DELETE CASCADE` also holds
    // on its own — a backstop, not the primary mechanism.
    let store = store();
    let key = SessionKey::new("native", "claude-code", "turn-rows-cascade");
    store
        .upsert_sessions(
            &[session("turn-rows-cascade", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    {
        let connection = store.lock();
        insert_turn_rows(&connection, &turn_session_key(&key), 1, &[turn_row(0)]).unwrap();
    }

    store
        .lock()
        .execute(
            "DELETE FROM session WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
        )
        .unwrap();

    let connection = store.lock();
    assert_eq!(
        count_turn_rows(&connection, &turn_session_key(&key), 1).unwrap(),
        0
    );
}

fn turn_row_with_content(turn_index: u64, text: &str) -> TurnRow {
    TurnRow {
        content: vec![ContentPart::new(ContentKind::AssistantText, text)],
        ..turn_row(turn_index)
    }
}

#[test]
fn deleting_a_session_removes_turn_content_written_through_the_fenced_writer() {
    let store = store();
    let key = SessionKey::new("native", "claude-code", "turn-content-delete-session");
    store
        .upsert_sessions(
            &[session("turn-content-delete-session", 1_000)],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), 1);
    writer
        .write_turn_rows(&[turn_row_with_content(0, "PRIVATE_TURN_CONTENT")])
        .unwrap();
    assert_eq!(
        count_turn_content_rows(&store.lock(), &turn_session_key(&key), 1).unwrap(),
        1
    );

    assert!(store.delete_session(&key).unwrap());

    assert_eq!(
        count_turn_content_rows(&store.lock(), &turn_session_key(&key), 1).unwrap(),
        0
    );
}

#[test]
fn clearing_local_session_data_removes_turn_content_written_through_the_fenced_writer() {
    let store = store();
    let mut record = session("turn-content-clear", 1_000);
    record.source_fingerprint = Some("sv1:turn-content-clear".into());
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let key = record.key.clone();
    let writer = FencedTurnRowStore::new(store.clone(), key.clone(), 1);
    writer
        .write_turn_rows(&[turn_row_with_content(0, "PRIVATE_TURN_CONTENT")])
        .unwrap();
    assert_eq!(
        count_turn_content_rows(&store.lock(), &turn_session_key(&key), 1).unwrap(),
        1
    );

    store.clear_local_session_data().unwrap();

    let remaining: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM turn_content", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, 0);
}
