use super::*;
use crate::store::cloud_capture::RootScope;
use std::{fs, path::PathBuf};

fn fixture() -> (tempfile::TempDir, Policy, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let database = base.join("opencode.db");
    let project = base.join("project");
    let outside = base.join("outside");
    fs::create_dir(&project).unwrap();
    fs::create_dir(&outside).unwrap();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session_v2(id TEXT PRIMARY KEY, directory TEXT);
             CREATE TABLE session_message(
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
                seq INTEGER NOT NULL, data TEXT NOT NULL, UNIQUE(session_id,seq));",
        )
        .unwrap();
    connection
        .execute(
            "WITH RECURSIVE sessions(n) AS (
                VALUES(0) UNION ALL SELECT n+1 FROM sessions WHERE n < 1024
             )
             INSERT INTO session_v2 SELECT printf('session-%04d',n), ?1 FROM sessions",
            [project.to_str().unwrap()],
        )
        .unwrap();
    let path = database.to_str().unwrap().to_owned();
    let policy = Policy {
        account_id: "account".into(),
        destination_id: "personal".into(),
        resource_origin: Some("https://api.example".into()),
        issuer_origin: Some("https://api.example".into()),
        team_grant_id: None,
        membership_generation: None,
        workspace_name: None,
        consent_version: ONGOING_CONSENT_VERSION.into(),
        revision: "revision".into(),
        roots: vec![RootScope {
            path: path.clone(),
            source_scope_id: "scope".into(),
            source_kind: SourceKind::OpenCodeSqliteV2,
        }],
        projects: vec![ProjectSelection {
            source_kind: SourceKind::OpenCodeSqliteV2,
            source_path: path,
            project_path: project.to_str().unwrap().into(),
        }],
        project_choices: BTreeMap::new(),
        paused: false,
    };
    (directory, policy, project, outside)
}

#[test]
fn cloud_scheduler_pages_wait_for_queue_drain_and_use_current_recorded_cwds() {
    let (_directory, policy, _project, outside) = fixture();
    let mut runner = Runner::default();
    let mut cache = super::super::projects::Cache::default();
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    assert_eq!(runner.candidates.len(), 512);
    let cursor = runner.discovery_cursor.clone().unwrap();
    assert_eq!(cursor.after.as_deref(), Some("session-0511"));
    runner.next_reconcile = None;
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    assert_eq!(runner.discovery_cursor.as_ref(), Some(&cursor));
    assert_eq!(runner.candidates.len(), 512);
    runner.candidates.clear();
    let connection = rusqlite::Connection::open(&policy.roots[0].path).unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory=?1 WHERE id='session-0512'",
            [outside.to_str().unwrap()],
        )
        .unwrap();
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    assert_eq!(runner.candidates.len(), 511);
    assert_eq!(
        runner
            .candidates
            .front()
            .unwrap()
            .candidate
            .native_session_id(),
        "session-0513"
    );
    assert_eq!(
        runner.discovery_cursor.as_ref().unwrap().after.as_deref(),
        Some("session-1023")
    );
    runner.candidates.clear();
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    assert_eq!(runner.candidates.len(), 1);
    assert!(runner.discovery_cursor.is_none());
    assert!(runner.next_reconcile.unwrap() > Instant::now());
    let source_id = runner.candidates[0].candidate.source_id().to_owned();
    runner.candidates.clear();
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    assert!(runner.candidates.is_empty());
    runner.next_reconcile = None;
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    assert_eq!(runner.candidates.len(), 512);
    let last = antiburn_cloud_capture::reopen_opencode(
        Path::new(&policy.roots[0].path),
        "session-1024",
        &Limits::default(),
    )
    .unwrap()
    .in_source_scope("scope")
    .unwrap();
    assert_eq!(last.source_id(), source_id);
}

#[test]
fn cloud_scheduler_backfill_starts_over_and_retains_its_own_page_position() {
    let (_directory, policy, _, _) = fixture();
    let mut runner = Runner::default();
    let mut cache = super::super::projects::Cache::default();
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    runner.candidates.clear();
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    assert_eq!(
        runner.discovery_cursor.as_ref().unwrap().after.as_deref(),
        Some("session-1023")
    );

    runner.schedule_backfill(&policy);
    for (first, count) in [
        ("session-0000", 512),
        ("session-0512", 512),
        ("session-1024", 1),
    ] {
        runner.refresh_candidates(&policy, &|| true, &mut cache);
        assert!(runner.backfill_page);
        assert_eq!(
            runner
                .candidates
                .front()
                .unwrap()
                .candidate
                .native_session_id(),
            first
        );
        assert_eq!(runner.candidates.len(), count);
        runner.candidates.clear();
    }
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    assert!(!runner.backfill_page);
    assert!(runner.backfill_revision.is_none());
    assert!(runner.candidates.is_empty());
}

#[test]
fn cloud_scheduler_pages_multiple_claude_roots_without_discarding_earlier_selections() {
    let (directory, mut policy, project, _) = fixture();
    policy.roots.clear();
    policy.projects.clear();
    for root_index in 0..2 {
        let root = directory
            .path()
            .canonicalize()
            .unwrap()
            .join(format!("claude-{root_index}"));
        fs::create_dir(&root).unwrap();
        for index in 0..300 {
            let id = format!("session-{index:04}");
            fs::write(
                root.join(format!("{id}.jsonl")),
                format!(
                    "{}\n",
                    serde_json::json!({
                        "sessionId": id, "cwd": project, "type": "user",
                    })
                ),
            )
            .unwrap();
        }
        policy.roots.push(RootScope {
            path: root.to_str().unwrap().into(),
            source_scope_id: format!("scope-{root_index}"),
            source_kind: SourceKind::ClaudeJsonl,
        });
        policy.projects.push(ProjectSelection {
            source_kind: SourceKind::ClaudeJsonl,
            source_path: root.to_str().unwrap().into(),
            project_path: project.to_str().unwrap().into(),
        });
    }
    let mut cache = super::super::projects::Cache::default();
    let (first, cursor, blocked) =
        discover_policy_page(&policy, &|| true, &mut cache, &DiscoveryCursor::default()).unwrap();
    assert_eq!(first.len(), 512);
    assert!(!blocked);
    let (second, cursor, blocked) =
        discover_policy_page(&policy, &|| true, &mut cache, &cursor.unwrap()).unwrap();
    assert_eq!(second.len(), 88);
    assert!(!blocked);
    assert!(cursor.is_none());
    let ids: BTreeSet<_> = first
        .iter()
        .chain(&second)
        .map(|selection| selection.candidate.source_id())
        .collect();
    assert_eq!(ids.len(), 600);
}

#[test]
fn cloud_scheduler_content_observation_errors_do_not_discard_healthy_pages() {
    let (_directory, policy, _, _) = fixture();
    let connection = rusqlite::Connection::open(&policy.roots[0].path).unwrap();
    connection.execute("INSERT INTO session_message VALUES ('broken','session-0001','user',1,?1)",
        [r#"{"files":[{"uri":"media:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#]).unwrap();
    let mut cache = super::super::projects::Cache::default();
    let (first, cursor, blocked) =
        discover_policy_page(&policy, &|| true, &mut cache, &DiscoveryCursor::default()).unwrap();
    assert!(blocked);
    assert_eq!(first.len(), 511);
    assert_eq!(first[0].candidate.native_session_id(), "session-0000");
    assert_eq!(first[1].candidate.native_session_id(), "session-0002");
    let (second, cursor, blocked) =
        discover_policy_page(&policy, &|| true, &mut cache, &cursor.unwrap()).unwrap();
    assert!(!blocked);
    assert_eq!(second.len(), 512);
    let (last, cursor, blocked) =
        discover_policy_page(&policy, &|| true, &mut cache, &cursor.unwrap()).unwrap();
    assert!(!blocked);
    assert!(cursor.is_none());
    assert_eq!(last.len(), 1);
}

#[test]
fn cloud_scheduler_pause_before_content_observation_stops_discovery() {
    let (_directory, policy, _, _) = fixture();
    let calls = std::cell::Cell::new(0);
    let current = || {
        calls.set(calls.get() + 1);
        calls.get() < 3
    };
    let mut cache = super::super::projects::Cache::default();
    let result = discover_policy_page(&policy, &current, &mut cache, &DiscoveryCursor::default());
    assert!(result.is_err());
    assert_eq!(calls.get(), 3);
}

#[test]
fn cloud_scheduler_page_cursors_reset_for_each_authority_and_policy_binding() {
    let (_directory, policy, _project, _outside) = fixture();
    let mut runner = Runner::default();
    let mut cache = super::super::projects::Cache::default();
    for changed in 0..4 {
        runner.refresh_candidates(&policy, &|| true, &mut cache);
        runner.candidates.clear();
        runner.refresh_candidates(&policy, &|| true, &mut cache);
        let cursor = runner.discovery_cursor.clone();
        runner.refresh_candidates(&policy, &|| false, &mut cache);
        assert_eq!(runner.discovery_cursor, cursor);
        let mut next = policy.clone();
        match changed {
            0 => next.account_id = "another-account".into(),
            1 => next.destination_id = "another-workspace".into(),
            2 => next.resource_origin = Some("https://other.example".into()),
            _ => next.revision = "another-revision".into(),
        }
        runner.refresh_candidates(&next, &|| true, &mut cache);
        assert_eq!(
            runner
                .candidates
                .front()
                .unwrap()
                .candidate
                .native_session_id(),
            "session-0000"
        );
        assert_eq!(
            runner.discovery_cursor.as_ref().unwrap().after.as_deref(),
            Some("session-0511")
        );
    }
}
