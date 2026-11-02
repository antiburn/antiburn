use super::*;
use crate::session_lifecycle::{IndexChangeReason, Observation, RemovalReason, RemovalScope};
use crate::store::{Incarnation, Revision};
use std::sync::atomic::{AtomicI64, Ordering};

fn fixture_now() -> i64 {
    time::OffsetDateTime::parse(
        "2026-08-01T10:01:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
    .unix_timestamp()
}

fn write_admission_session(home: &std::path::Path, session_id: &str) -> PathBuf {
    let path = write_claude_session(home, session_id);
    let content = std::fs::read_to_string(&path).unwrap();
    let cwd = serde_json::to_string(&home.canonicalize().unwrap()).unwrap();
    std::fs::write(&path, content.replace("\"/home/avery/code/widgets\"", &cwd)).unwrap();
    path
}

fn admission_writer(store: &Store, now: i64) -> IncrementalAdmission<'_> {
    IncrementalAdmission {
        io: AdmissionIo::Test {
            cancel: Arc::new(AtomicBool::new(false)),
            reports: Arc::new(Mutex::new(Vec::new())),
            now: Arc::new(AtomicI64::new(now)),
            cancel_after_report: false,
        },
        store,
        previous_records: store.session_records().unwrap(),
        evidence_agents: agents::evidence_cohort(),
        write_fence: store.capture_destructive_write_epoch(),
        error: None,
        outcomes: std::collections::HashMap::new(),
        list_changed: false,
    }
}

fn take_reports(writer: &IncrementalAdmission<'_>) -> Vec<Observation> {
    let AdmissionIo::Test { reports, .. } = &writer.io else {
        panic!("test admission IO")
    };
    std::mem::take(&mut *reports.lock().unwrap())
}

#[test]
fn ordinary_appends_do_not_trigger_project_discovery_but_cwd_changes_do() {
    let previous = record_for_facets("session", None, 1_000);
    let mut appended = previous.clone();
    appended.updated_at_epoch = Some(1_100);
    appended.activity_cursor = "next-cursor".into();

    let ordinary_facets = facets_between(&previous, &appended);
    assert!(ordinary_facets.metadata);
    assert!(!ordinary_facets.project_scope);
    let ordinary_event = crate::session_lifecycle::SessionEvent::Updated {
        session: (&appended.key).into(),
        facets: ordinary_facets,
        at: 1_100,
    };
    assert!(!crate::installed_checks::lifecycle_changes_projects(
        &ordinary_event
    ));

    appended.cwd = Some("/home/avery/code/another-project".into());
    let scope_facets = facets_between(&previous, &appended);
    assert!(scope_facets.metadata);
    assert!(scope_facets.project_scope);
    let scope_event = crate::session_lifecycle::SessionEvent::Updated {
        session: (&appended.key).into(),
        facets: scope_facets,
        at: 1_100,
    };
    assert!(crate::installed_checks::lifecycle_changes_projects(
        &scope_event
    ));
}

async fn describe_admitted(
    store: &Store,
    logs: Vec<SessionLog>,
    home: &std::path::Path,
    now: i64,
    include_non_repo_folders: bool,
) -> (Described, Vec<Observation>, bool) {
    let previous = store.session_records().unwrap();
    let ignored = HashSet::new();
    let precomputed = std::collections::HashMap::new();
    let mut writer = admission_writer(store, now);
    let described = describe_with_gate(
        logs,
        DescribeInputs {
            home,
            ignored: &ignored,
            previous_records: &previous,
            precomputed: &precomputed,
            include_non_repo_folders,
            probe_app: None,
            incremental: Some(&mut writer),
        },
        &mut |_, _, _| {},
    )
    .await;
    assert!(writer.error.is_none());
    (described, take_reports(&writer), writer.list_changed)
}

async fn persisted_facts(
    store: &Store,
    described: &Described,
    previous: &std::collections::HashMap<SessionActivityKey, SessionRecord>,
) -> Vec<Observation> {
    let now = described
        .records
        .iter()
        .filter_map(|record| record.updated_at_epoch)
        .max()
        .unwrap();
    let mut writer = admission_writer(store, now);
    writer.previous_records = previous.clone();
    for row in &described.records {
        let guard =
            session_write_guard(store, &admission_log(row), &row.key, writer.write_fence).unwrap();
        assert!(
            writer
                .commit(&guard, row, described.changed.contains(&row.key))
                .await
        );
    }
    assert!(writer.error.is_none());
    take_reports(&writer)
}

#[tokio::test]
async fn admission_reports_follow_real_new_unchanged_and_appended_sources() {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let path = write_admission_session(home.path(), "producer");
    let (first, facts, list_changed) = describe_admitted(
        &store,
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_000)],
        home.path(),
        fixture_now(),
        true,
    )
    .await;
    assert_eq!(first.records.len(), 1);
    let [
        Observation::Indexed { sessions, revision },
        Observation::IndexChanged { reason },
    ] = facts.as_slice()
    else {
        panic!("indexed then list change: {facts:?}")
    };
    assert_eq!(*revision, store.revision());
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].is_new);
    let incarnation = sessions[0].incarnation;
    assert!(incarnation > Incarnation(0));
    assert_eq!(*reason, IndexChangeReason::ScanPass);
    assert!(list_changed);

    let revision = store.revision();
    let (second, facts, list_changed) = describe_admitted(
        &store,
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_000)],
        home.path(),
        fixture_now(),
        true,
    )
    .await;
    assert_eq!(second.records.len(), 1);
    assert!(facts.is_empty());
    assert_eq!(store.revision(), revision);
    assert!(!list_changed);

    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"type\":\"assistant\",\"timestamp\":\"2026-08-01T10:05:00Z\"}\n")
        .unwrap();
    let (third, facts, list_changed) = describe_admitted(
        &store,
        vec![log(AgentKind::Claude, path, 1_800_000_100)],
        home.path(),
        fixture_now() + 240,
        true,
    )
    .await;
    assert_eq!(third.records.len(), 1);
    let [
        Observation::Indexed { sessions, revision },
        Observation::RowChanged {
            session,
            facets,
            at,
        },
    ] = facts.as_slice()
    else {
        panic!("indexed then metadata: {facts:?}")
    };
    assert_eq!(*revision, store.revision());
    assert_eq!(sessions[0].incarnation, incarnation);
    assert!(!sessions[0].is_new);
    assert_eq!(*session, sessions[0].key);
    assert_eq!(*at, sessions[0].at);
    assert!(facets.metadata);
    assert!(!facets.title);
    assert!(!list_changed);
}

#[tokio::test]
async fn admission_source_replacement_is_not_a_membership_addition() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let mut row = record_for_facets("existing-identity", None, 1_000);
    let (incarnations, _) = store
        .upsert_sessions(std::slice::from_ref(&row), &[])
        .unwrap();
    let mut writer = admission_writer(&store, 1_001);
    writer.previous_records.clear();
    row.source_label = "/replacement/existing-identity.jsonl".into();
    let guard =
        session_write_guard(&store, &admission_log(&row), &row.key, writer.write_fence).unwrap();

    assert!(writer.commit(&guard, &row, true).await);
    assert!(writer.finish().is_ok());
    assert!(!writer.list_changed);
    let reports = take_reports(&writer);
    assert!(matches!(reports.as_slice(), [Observation::Indexed { .. }]));
    let (presence, _) = store
        .session_presence_for_keys(std::slice::from_ref(&row.key))
        .unwrap();
    assert_eq!(presence[0].incarnation, incarnations[0].1);
    assert_eq!(
        store
            .session_records_for_session_keys(std::slice::from_ref(&row.key))
            .unwrap(),
        vec![row]
    );
}

#[tokio::test]
async fn admission_reports_only_successful_rejected_deletions_with_store_evidence() {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let path = write_claude_sidechain(home.path(), "rejected");
    let mut row = record("claude-code", "rejected", Some(1_800_000_000));
    row.source_label = path.to_string_lossy().into_owned();
    let (incarnations, before) = store
        .upsert_sessions(&[row], &agents::evidence_cohort())
        .unwrap();
    for attempt in 0..2 {
        let (described, reports, _) = describe_admitted(
            &store,
            vec![log(AgentKind::Claude, path.clone(), 1_800_000_050)],
            home.path(),
            1_800_000_050,
            true,
        )
        .await;
        assert_eq!(described.rejected.len(), 1);
        if attempt == 0 {
            let [
                Observation::Removed {
                    scope: RemovalScope::One(key, incarnation),
                    reason,
                    revision,
                },
                Observation::IndexChanged {
                    reason: IndexChangeReason::ScanPass,
                },
            ] = reports.as_slice()
            else {
                panic!("one rejected deletion")
            };
            assert_eq!(*key, incarnations[0].0);
            assert_eq!(*incarnation, incarnations[0].1);
            assert_eq!(*reason, RemovalReason::Rejected);
            assert!(*revision > before);
            assert_eq!(*revision, store.revision());
        } else {
            assert!(reports.is_empty(), "a missing row has no deletion evidence");
        }
    }
    assert!(store.recent_sessions(0, 10).unwrap().is_empty());
}

async fn assert_parsed_file_identity_is_admitted(
    home: &std::path::Path,
    path: PathBuf,
    session_id: &str,
) {
    let store = Store::open_in_memory(home).unwrap();
    let (described, facts, list_changed) = describe_admitted(
        &store,
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_000)],
        home,
        fixture_now(),
        true,
    )
    .await;
    assert_eq!(described.records.len(), 1);
    assert!(described.rejected.is_empty());
    let row = &described.records[0];
    assert_eq!(row.key.session_id, session_id);
    assert_eq!(row.source_label, path.to_string_lossy());
    let stored = store.session_records().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored.values().next().unwrap(), row);
    let [
        Observation::Indexed { sessions, revision },
        Observation::IndexChanged { reason },
    ] = facts.as_slice()
    else {
        panic!("parsed file admission reports: {facts:?}")
    };
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].key, row.key);
    assert!(sessions[0].is_new);
    assert!(sessions[0].incarnation > Incarnation(0));
    assert_eq!(*revision, store.revision());
    assert_eq!(*reason, IndexChangeReason::ScanPass);
    assert!(list_changed);

    let revision = store.revision();
    let (unchanged, facts, list_changed) = describe_admitted(
        &store,
        vec![log(AgentKind::Claude, path, 1_800_000_000)],
        home,
        fixture_now(),
        true,
    )
    .await;
    assert_eq!(unchanged.records, described.records);
    assert!(facts.is_empty());
    assert!(!list_changed);
    assert_eq!(store.revision(), revision);
    assert_eq!(store.session_records().unwrap(), stored);
}

#[tokio::test]
async fn admission_uses_the_parsed_file_session_id_when_the_filename_id_differs() {
    let home = tempfile::TempDir::new().unwrap();
    let original = write_admission_session(home.path(), "parsed-session");
    let path = original.with_file_name("filename-session.jsonl");
    std::fs::rename(original, &path).unwrap();
    assert_eq!(
        recovered_id(&log(AgentKind::Claude, path.clone(), 1_800_000_000)).as_deref(),
        Some("filename-session")
    );

    assert_parsed_file_identity_is_admitted(home.path(), path, "parsed-session").await;
}

#[tokio::test]
async fn admission_uses_the_parsed_file_session_id_without_a_recoverable_filename_id() {
    let home = tempfile::TempDir::new().unwrap();
    let path = write_admission_session(home.path(), "parsed-session");
    let mut source = log(AgentKind::Claude, path, 1_800_000_000);
    let read = session_log_read(&source).await.unwrap();
    // Inject the cached read with no filename component. Valid UTF-8 file
    // stems always supply a fallback ID, even when no vendor ID is present.
    source.source = SessionSource::File(PathBuf::from(std::path::MAIN_SEPARATOR_STR));
    assert_eq!(recovered_id(&source), None);
    let outcome =
        describe_one_with_activity(source.clone(), home.path(), None, None, None, Some(read)).await;
    let DescribeOutcome::Session(row) = outcome else {
        panic!("the cached read supplies the parsed session identity")
    };
    assert_eq!(row.key.session_id, "parsed-session");
    let store = Store::open_in_memory(home.path()).unwrap();
    let mut writer = admission_writer(&store, fixture_now());
    let guard = session_write_guard(&store, &source, &row.key, writer.write_fence).unwrap();
    assert!(writer.commit(&guard, &row, true).await);
    assert!(writer.finish().is_ok());
    assert_eq!(
        store.session_records().unwrap().values().next(),
        Some(row.as_ref())
    );
    assert!(
        matches!(take_reports(&writer).as_slice(), [Observation::Indexed { sessions, .. }, Observation::IndexChanged { .. }] if sessions[0].key == row.key)
    );
}

#[tokio::test]
async fn admission_detects_a_new_identity_behind_a_reused_label() {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let path = write_admission_session(home.path(), "next");
    let mut old = record_for_facets("previous", None, 1_800_000_000);
    old.source_label = path.to_string_lossy().into_owned();
    store
        .upsert_sessions(&[old], &agents::evidence_cohort())
        .unwrap();
    let (described, facts, list_changed) = describe_admitted(
        &store,
        vec![log(AgentKind::Claude, path, 1_800_000_000)],
        home.path(),
        fixture_now(),
        true,
    )
    .await;
    assert_eq!(described.records.len(), 1);
    let [
        Observation::Indexed { sessions, .. },
        Observation::IndexChanged { .. },
    ] = facts.as_slice()
    else {
        panic!("new identity, not a row patch")
    };
    assert_eq!(sessions[0].key.session_id, "next");
    assert!(sessions[0].is_new);
    assert!(list_changed);
}

#[test]
fn discovery_reports_bound_chunks_and_keep_each_upsert_incarnation_and_revision() {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let records: Vec<_> = (0..600)
        .map(|index| record_for_facets(&format!("row{index}"), None, 1000))
        .collect();
    let changed: Vec<_> = records.iter().map(|record| record.key.clone()).collect();
    let (incarnations, revision) = store
        .upsert_sessions(&records, &agents::evidence_cohort())
        .unwrap();
    let mut lengths = Vec::new();
    let mut seen = Vec::new();
    for report in discovery_report(
        1001,
        &records,
        &incarnations,
        &changed,
        &Default::default(),
        revision,
    ) {
        let Observation::Indexed {
            sessions,
            revision: reported,
        } = report
        else {
            panic!("new rows only index")
        };
        assert_eq!(reported, revision);
        lengths.push(sessions.len());
        seen.extend(
            sessions
                .into_iter()
                .map(|session| (session.key, session.incarnation)),
        );
    }
    assert_eq!(lengths, vec![256, 256, 88]);
    assert_eq!(seen, incarnations);
    assert!(
        discovery_report(
            1180,
            &records,
            &incarnations,
            &changed,
            &Default::default(),
            revision
        )
        .next()
        .is_none(),
        "history does not establish liveness"
    );
    assert!(
        discovery_report(
            1001,
            &records,
            &[],
            &changed,
            &Default::default(),
            Revision(0)
        )
        .next()
        .is_none(),
        "no invented existence evidence"
    );
}

#[tokio::test]
async fn admission_latest_rejection_replaces_an_accepted_attempt() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let row = record("claude-code", "accepted-then-rejected", Some(1_000));
    let mut writer = admission_writer(&store, 1_001);
    let guard =
        session_write_guard(&store, &admission_log(&row), &row.key, writer.write_fence).unwrap();
    assert!(writer.commit(&guard, &row, true).await);
    let committed_revision = store.revision();
    let initial = take_reports(&writer);
    let [
        Observation::Indexed { sessions, .. },
        Observation::IndexChanged { .. },
    ] = initial.as_slice()
    else {
        panic!("accepted admission reports: {initial:?}")
    };
    let incarnation = sessions[0].incarnation;

    let guard =
        session_write_guard(&store, &admission_log(&row), &row.key, writer.write_fence).unwrap();
    assert!(writer.delete_rejected(&guard, &row.key).await);
    assert!(matches!(
        writer.outcomes.get(&row.key),
        Some(AttemptOutcome::Rejected)
    ));
    assert!(
        !writer
            .previous_records
            .values()
            .any(|previous| previous.key == row.key)
    );
    assert!(
        store
            .session_records_for_session_keys(std::slice::from_ref(&row.key))
            .unwrap()
            .is_empty()
    );
    let reports = take_reports(&writer);
    let [
        Observation::Removed {
            scope: RemovalScope::One(key, removed),
            reason: RemovalReason::Rejected,
            revision,
        },
        Observation::IndexChanged { .. },
    ] = reports.as_slice()
    else {
        panic!("committed rejection reports: {reports:?}")
    };
    assert_eq!(*key, row.key);
    assert_eq!(*removed, incarnation);
    assert!(*revision > committed_revision);
    assert_eq!(*revision, store.revision());
}

#[tokio::test]
async fn admission_latest_acceptance_replaces_a_rejected_attempt() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let row = record("claude-code", "rejected-then-accepted", Some(1_000));
    let (old, _) = store
        .upsert_sessions(std::slice::from_ref(&row), &[])
        .unwrap();
    let mut writer = admission_writer(&store, 1_001);
    let guard =
        session_write_guard(&store, &admission_log(&row), &row.key, writer.write_fence).unwrap();
    assert!(writer.delete_rejected(&guard, &row.key).await);
    assert!(matches!(
        writer.outcomes.get(&row.key),
        Some(AttemptOutcome::Rejected)
    ));
    take_reports(&writer);

    let guard =
        session_write_guard(&store, &admission_log(&row), &row.key, writer.write_fence).unwrap();
    assert!(writer.commit(&guard, &row, true).await);
    assert!(
        matches!(writer.outcomes.get(&row.key), Some(AttemptOutcome::Accepted(accepted, true)) if **accepted == row)
    );
    assert_eq!(
        store
            .session_records_for_session_keys(std::slice::from_ref(&row.key))
            .unwrap(),
        vec![row]
    );
    let reports = take_reports(&writer);
    let [
        Observation::Indexed { sessions, .. },
        Observation::IndexChanged { .. },
    ] = reports.as_slice()
    else {
        panic!("new incarnation reports: {reports:?}")
    };
    assert!(sessions[0].is_new);
    assert!(sessions[0].incarnation > old[0].1);
}

#[tokio::test]
async fn admission_failed_newer_commit_keeps_prior_reports_but_replaces_its_outcome() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let mut row = record("claude-code", "failed-newer-commit", Some(1_000));
    let mut writer = admission_writer(&store, 1_001);
    let guard =
        session_write_guard(&store, &admission_log(&row), &row.key, writer.write_fence).unwrap();
    assert!(writer.commit(&guard, &row, true).await);
    let revision = store.revision();
    store
        .lock()
        .execute_batch(
            "CREATE TRIGGER fail_session_update BEFORE UPDATE ON session
         BEGIN SELECT RAISE(FAIL, 'synthetic admission failure'); END;",
        )
        .unwrap();
    let guard =
        session_write_guard(&store, &admission_log(&row), &row.key, writer.write_fence).unwrap();
    row.title = Some("must not commit".into());
    assert!(!writer.commit(&guard, &row, true).await);
    assert!(
        writer
            .error
            .as_ref()
            .unwrap()
            .to_string()
            .contains("synthetic admission failure")
    );
    assert!(matches!(
        writer.outcomes.get(&row.key),
        Some(AttemptOutcome::Failed)
    ));
    assert_eq!(store.revision(), revision);
    assert_eq!(
        store.session_records_for_session_keys(&[row.key]).unwrap()[0].title,
        None
    );
    let reports = take_reports(&writer);
    assert!(
        matches!(reports.as_slice(), [Observation::Indexed { revision: reported, .. }, Observation::IndexChanged { .. }] if *reported == revision)
    );
}

#[tokio::test]
async fn admission_drains_committed_reports_when_the_first_report_cancels_the_scan() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let mut row = record_for_facets("cancel-after-report", None, 1_000);
    store
        .upsert_sessions(std::slice::from_ref(&row), &[])
        .unwrap();
    let mut writer = admission_writer(&store, 1_001);
    let AdmissionIo::Test {
        cancel_after_report,
        ..
    } = &mut writer.io
    else {
        panic!("test admission IO")
    };
    *cancel_after_report = true;
    let guard =
        session_write_guard(&store, &admission_log(&row), &row.key, writer.write_fence).unwrap();
    row.title = Some("committed title".into());
    assert!(writer.commit(&guard, &row, true).await);
    let reports = take_reports(&writer);
    assert!(
        matches!(reports.as_slice(), [Observation::Indexed { .. }, Observation::RowChanged { facets, .. }] if facets.title)
    );
    let AdmissionIo::Test { cancel, .. } = &writer.io else {
        panic!("test admission IO")
    };
    assert!(cancel.load(Ordering::Acquire));
    assert_eq!(
        store
            .session_records_for_session_keys(std::slice::from_ref(&row.key))
            .unwrap()[0]
            .title,
        row.title
    );
    assert!(matches!(
        writer.outcomes.get(&row.key),
        Some(AttemptOutcome::Accepted(_, true))
    ));
    let next = record("claude-code", "cancelled-next", Some(1_000));
    let guard =
        session_write_guard(&store, &admission_log(&next), &next.key, writer.write_fence).unwrap();
    assert!(!writer.commit(&guard, &next, true).await);
    assert!(
        writer.finish().is_ok(),
        "cancellation is not a write failure"
    );
    assert!(take_reports(&writer).is_empty());
    assert!(
        store
            .session_records_for_session_keys(&[next.key])
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn admission_uses_actual_commit_time_for_active_eligibility() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let row = record("claude-code", "aged-during-description", Some(1_000));
    let mut writer = admission_writer(&store, 1_001);
    let AdmissionIo::Test { now, .. } = &writer.io else {
        panic!("test admission IO")
    };
    now.store(1_180, Ordering::Release);
    let guard =
        session_write_guard(&store, &admission_log(&row), &row.key, writer.write_fence).unwrap();
    assert!(writer.commit(&guard, &row, true).await);
    assert_eq!(
        store
            .session_records_for_session_keys(std::slice::from_ref(&row.key))
            .unwrap(),
        vec![row]
    );
    assert!(matches!(
        take_reports(&writer).as_slice(),
        [Observation::IndexChanged { .. }]
    ));
}

#[tokio::test]
async fn admission_rejects_delete_clear_and_policy_races_without_reports() {
    for mutation in ["delete", "recreate", "clear", "policy"] {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(home.path()).unwrap();
        let row = record("claude-code", "raced", Some(1_000));
        store
            .upsert_sessions(std::slice::from_ref(&row), &[])
            .unwrap();
        let mut writer = admission_writer(&store, 1_001);
        let guard = session_write_guard(&store, &admission_log(&row), &row.key, writer.write_fence)
            .unwrap();
        let mut expected = Vec::new();
        match mutation {
            "delete" => {
                store.delete_session(&row.key).unwrap();
            }
            "recreate" => {
                store.delete_session(&row.key).unwrap();
                let mut replacement = row.clone();
                replacement.title = Some("replacement".into());
                store
                    .upsert_sessions(std::slice::from_ref(&replacement), &[])
                    .unwrap();
                expected.push(replacement);
            }
            "clear" => {
                store.clear_local_session_data().unwrap();
            }
            "policy" => {
                store.grant_dir("Documents").unwrap();
                expected.push(row.clone());
            }
            _ => unreachable!(),
        }
        let revision = store.revision();
        assert!(!writer.commit(&guard, &row, true).await, "{mutation}");
        assert!(
            writer.finish().is_err(),
            "{mutation} commit must fail the pass"
        );
        assert!(
            !writer.delete_rejected(&guard, &row.key).await,
            "{mutation}"
        );
        assert!(
            writer.finish().is_err(),
            "{mutation} deletion must fail the pass"
        );
        assert!(take_reports(&writer).is_empty(), "{mutation}");
        assert_eq!(store.revision(), revision, "{mutation}");
        assert_eq!(
            store.session_records_for_session_keys(&[row.key]).unwrap(),
            expected,
            "{mutation}"
        );
    }
}

#[tokio::test]
async fn admission_rejects_a_stale_physical_source_without_reports() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let mut row = record("claude-code", "rebound", Some(1_000));
    let mut writer = admission_writer(&store, 1_001);
    let old =
        session_write_guard(&store, &admission_log(&row), &row.key, writer.write_fence).unwrap();
    row.source_label = "/replacement/rebound.jsonl".into();
    let current =
        session_write_guard(&store, &admission_log(&row), &row.key, writer.write_fence).unwrap();
    assert!(writer.commit(&current, &row, true).await);
    take_reports(&writer);
    let revision = store.revision();
    let mut stale = row.clone();
    stale.source_label = "/tmp/rebound.jsonl".into();
    assert!(!writer.commit(&old, &stale, true).await);
    assert!(!writer.delete_rejected(&old, &row.key).await);
    assert!(
        writer.finish().is_err(),
        "a stale final attempt must fail the pass"
    );
    assert!(take_reports(&writer).is_empty());
    assert_eq!(store.revision(), revision);
    assert_eq!(
        store
            .session_records_for_session_keys(std::slice::from_ref(&row.key))
            .unwrap(),
        vec![row]
    );
}

#[tokio::test(start_paused = true)]
async fn unchanged_launch_discovery_does_not_own_failed_seed_recovery() {
    use crate::session_lifecycle::{ReconcileSource, SessionEvent, SessionEvents, SessionRef};
    use crate::store::{ActiveCursor, Presence};
    use std::sync::{Arc, Mutex};

    struct FailingSeed {
        store: Store,
        requests: Mutex<Vec<Option<ActiveCursor>>>,
    }
    impl ReconcileSource for FailingSeed {
        fn presence(&self, keys: &[SessionKey]) -> anyhow::Result<(Vec<Presence>, Revision)> {
            self.store.session_presence_for_keys(keys)
        }
        fn active(
            &self,
            since: i64,
            after: Option<&ActiveCursor>,
            limit: usize,
        ) -> anyhow::Result<(Vec<Presence>, Revision)> {
            let mut requests = self.requests.lock().unwrap();
            requests.push(after.cloned());
            if requests.len() == 2 {
                anyhow::bail!("the second seed page fails");
            }
            self.store.sessions_active_since_page(since, after, limit)
        }
    }

    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let paths: Vec<_> = (0..257)
        .map(|index| write_claude_session(home.path(), &format!("seed-{index:03}")))
        .collect();
    let previous = store.session_records().unwrap();
    let logs = || {
        paths
            .iter()
            .map(|path| log(AgentKind::Claude, path.clone(), 1_800_000_000))
            .collect()
    };
    let first = describe_with_states(logs(), home.path(), &HashSet::new(), &previous).await;
    assert_eq!(first.records.len(), 257);
    assert!(!persisted_facts(&store, &first, &previous).await.is_empty());
    let now = first
        .records
        .iter()
        .filter_map(|record| record.updated_at_epoch)
        .max()
        .unwrap();
    let previous = store.session_records().unwrap();
    let source = Arc::new(FailingSeed {
        store: store.clone(),
        requests: Mutex::new(Vec::new()),
    });
    let events = Arc::new(SessionEvents::default());
    let mut bus = events.subscribe();
    let actor = crate::session_lifecycle::tests::start_seeded(events.clone(), source.clone(), now);
    let seeded = events.snapshot(500);
    assert_eq!(seeded.total, 256);
    assert_eq!(seeded.seq, 0);
    let omitted = first
        .records
        .iter()
        .find(|record| {
            !seeded
                .sessions
                .iter()
                .any(|live| live.session == SessionRef::from(&record.key))
        })
        .unwrap()
        .key
        .clone();
    let awake = tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    });
    let unchanged = describe_with_states(logs(), home.path(), &HashSet::new(), &previous).await;
    let revision = store.revision();
    assert!(
        persisted_facts(&store, &unchanged, &previous)
            .await
            .is_empty()
    );
    assert_eq!(store.revision(), revision);
    assert_eq!(events.snapshot(500).total, 256);
    assert_eq!(events.current_seq(), 0);
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    for _ in 0..5_000 {
        if events.snapshot(500).total == 257 {
            break;
        }
        tokio::task::yield_now().await;
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(events.snapshot(500).total, 257);
    let answer = events.presence(&[SessionRef::from(&omitted)]);
    assert_eq!(answer.seq, 1);
    assert_eq!(answer.present.len(), 1);
    assert!(answer.absent.is_empty());
    let event = bus.try_recv().unwrap();
    assert_eq!(event.seq, 1);
    assert!(
        matches!(event.event, SessionEvent::Activity { session: Some(session), .. } if session == SessionRef::from(&omitted))
    );
    assert_eq!(event.aggregate.unwrap().total, 257);
    assert!(
        bus.try_recv().is_err(),
        "seeded rows have no duplicate narration"
    );
    {
        let requests = source.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1], requests[2]);
    }
    awake.abort();
    actor.abort();
    assert!(actor.await.unwrap_err().is_cancelled());
}
