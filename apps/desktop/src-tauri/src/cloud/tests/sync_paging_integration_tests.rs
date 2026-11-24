use super::*;
use crate::store::cloud_capture::RootScope;
use discovery::discover_policy_page_with_clock;
use std::{cell::Cell, fs, path::PathBuf};

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
        repository_rules: Vec::new(),
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
    let first_count = runner.candidates.len();
    assert!((1..=512).contains(&first_count));
    let cursor = runner.discovery_cursor.clone().unwrap();
    assert_eq!(
        cursor.after.as_deref(),
        runner
            .candidates
            .back()
            .map(|selection| selection.candidate.native_session_id())
    );
    runner.next_reconcile = None;
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    assert_eq!(runner.discovery_cursor.as_ref(), Some(&cursor));
    assert_eq!(runner.candidates.len(), first_count);
    let connection = rusqlite::Connection::open(&policy.roots[0].path).unwrap();
    let excluded = format!("session-{first_count:04}");
    connection
        .execute(
            "UPDATE session_v2 SET directory=?1 WHERE id=?2",
            [outside.to_str().unwrap(), &excluded],
        )
        .unwrap();
    let mut selected = Vec::new();
    loop {
        selected.extend(
            runner
                .candidates
                .drain(..)
                .map(|selection| selection.candidate.native_session_id().to_owned()),
        );
        let Some(cursor) = runner.discovery_cursor.clone() else {
            break;
        };
        assert!(!matches!(
            runner.refresh_candidates(&policy, &|| true, &mut cache),
            DiscoveryRefresh::Unchanged
        ));
        assert_ne!(runner.discovery_cursor.as_ref(), Some(&cursor));
    }
    assert_eq!(
        selected,
        (0..=1024)
            .filter(|index| *index != first_count)
            .map(|index| format!("session-{index:04}"))
            .collect::<Vec<_>>()
    );
    assert!(runner.next_reconcile.unwrap() > Instant::now());
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    assert!(runner.candidates.is_empty());
    runner.next_reconcile = None;
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    assert_eq!(
        runner.candidates[0].candidate.native_session_id(),
        "session-0000"
    );
}

#[test]
fn cloud_scheduler_yields_between_opencode_digests_and_resumes_with_fresh_versions() {
    let (_directory, policy, _, _) = fixture();
    let connection = rusqlite::Connection::open(&policy.roots[0].path).unwrap();
    connection
        .execute("DELETE FROM session_v2 WHERE id > 'session-0002'", [])
        .unwrap();
    let mut cache = super::super::projects::Cache::default();
    let start = Instant::now();
    let (before, _, _) = discover_policy_page_with_clock(
        &policy,
        &|| true,
        &mut cache,
        &DiscoveryCursor::default(),
        || start,
    )
    .unwrap();
    assert_eq!(before.len(), 3);

    let read_slice = |cache: &mut _, cursor: &DiscoveryCursor| {
        let clock = Cell::new(start);
        let (selected, next, coverage) =
            discover_policy_page_with_clock(&policy, &|| true, cache, cursor, || {
                let now = clock.get();
                clock.set(now + Duration::from_secs(1));
                now
            })
            .unwrap();
        assert!(!coverage.blocked && !coverage.partial);
        assert_eq!(selected.len(), 1);
        assert_eq!(coverage.opencode_slice_yielded, next.is_some());
        (selected, next)
    };
    let (first, next) = read_slice(&mut cache, &DiscoveryCursor::default());
    assert_eq!(first[0].candidate.native_session_id(), "session-0000");
    assert_eq!(first[0].observation, before[0].observation);
    let next = next.unwrap();
    assert_eq!(next.after.as_deref(), Some("session-0000"));
    connection
        .execute(
            "INSERT INTO session_message VALUES ('new','session-0001','user',1,'{}')",
            [],
        )
        .unwrap();

    let (second, next) = read_slice(&mut cache, &next);
    assert_eq!(second[0].candidate.native_session_id(), "session-0001");
    assert_ne!(second[0].observation, before[1].observation);
    let next = next.unwrap();
    assert_eq!(next.after.as_deref(), Some("session-0001"));
    let (third, next) = read_slice(&mut cache, &next);
    assert_eq!(third[0].candidate.native_session_id(), "session-0002");
    assert_eq!(third[0].observation, before[2].observation);
    assert!(next.is_none());
}

#[cfg(unix)]
#[test]
fn cloud_scheduler_routes_cursor_composer_pages_and_tracks_only_owned_rows() {
    let (temporary, mut policy, project, _) = fixture();
    let user = temporary.path().canonicalize().unwrap().join("Cursor/User");
    let workspace = user.join("workspaceStorage/synthetic-workspace");
    let global_path = user.join("globalStorage/state.vscdb");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(global_path.parent().unwrap()).unwrap();
    fs::write(
        workspace.join("workspace.json"),
        serde_json::json!({ "folder": format!("file://{}", project.display()) }).to_string(),
    )
    .unwrap();
    let local = rusqlite::Connection::open(workspace.join("state.vscdb")).unwrap();
    local
        .execute_batch(
            r#"CREATE TABLE ItemTable(key TEXT PRIMARY KEY, value BLOB);
           INSERT INTO ItemTable VALUES (
             'composer.composerData','{"allComposers":[{"composerId":"synthetic-main"}]}'
           );"#,
        )
        .unwrap();
    let global = rusqlite::Connection::open(&global_path).unwrap();
    global.execute_batch(
        r#"CREATE TABLE cursorDiskKV(key TEXT PRIMARY KEY, value BLOB);
           INSERT INTO cursorDiskKV VALUES (
             'composerData:synthetic-main','{"fullConversationHeadersOnly":[{"bubbleId":"main"}]}'
           );
           INSERT INTO cursorDiskKV VALUES ('bubbleId:synthetic-main:main','{"type":1,"text":"before"}');
           INSERT INTO cursorDiskKV VALUES ('bubbleId:synthetic-other:main','{"type":1,"text":"before"}');"#,
    )
    .unwrap();
    let root = workspace.to_str().unwrap();
    let kind = SourceKind::CursorIdeComposer;
    policy.roots[0].source_kind = kind;
    policy.roots[0].path = root.into();
    policy.projects[0].source_kind = kind;
    policy.projects[0].source_path = root.into();
    let complete = discover_root(kind, root, 2).unwrap();
    assert_eq!(complete[0].native_session_id(), "synthetic-main");
    let native = file_sources::reopen(
        kind,
        &workspace,
        "cursor/ide/synthetic-main",
        &metadata_limits(),
    )
    .unwrap();
    assert_eq!(native.source_id(), complete[0].source_id());

    let observed = || {
        let selected = discover_policy(&policy, &|| true).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(
            selected[0].candidate.source_id(),
            "scope/cursor/ide/synthetic-main"
        );
        selected[0].observation.clone()
    };
    let first = observed();
    assert!(first.starts_with("cursor-ide-native-v1:"));
    global.execute(
        "UPDATE cursorDiskKV SET value='unrelated change' WHERE key='bubbleId:synthetic-other:main'",
        [],
    ).unwrap();
    assert_eq!(observed(), first);
    global
        .execute(
            "UPDATE cursorDiskKV SET value='owned change' WHERE key='bubbleId:synthetic-main:main'",
            [],
        )
        .unwrap();
    assert_ne!(observed(), first);
}

#[test]
fn cloud_scheduler_backfill_starts_over_and_retains_its_own_page_position() {
    let (_directory, policy, _, _) = fixture();
    let mut runner = Runner::default();
    let mut cache = super::super::projects::Cache::default();
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    let first_cursor = runner.discovery_cursor.clone().unwrap();
    runner.candidates.clear();
    runner.refresh_candidates(&policy, &|| true, &mut cache);
    let second_cursor = runner.discovery_cursor.as_ref().unwrap();
    assert!(second_cursor.after > first_cursor.after);

    runner.schedule_backfill(&policy, "test-grant");
    let mut scanned = 0;
    loop {
        assert!(!matches!(
            runner.refresh_candidates(&policy, &|| true, &mut cache),
            DiscoveryRefresh::Unchanged
        ));
        assert!(runner.backfill_page);
        assert!(!runner.candidates.is_empty());
        let cursor = runner.discovery_cursor.clone();
        runner.schedule_backfill(&policy, "test-grant");
        assert_eq!(runner.discovery_cursor, cursor);
        for selection in runner.candidates.drain(..) {
            assert_eq!(
                selection.candidate.native_session_id(),
                format!("session-{scanned:04}")
            );
            scanned += 1;
        }
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(scanned, 1025);
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
    let (first, cursor, coverage) =
        discover_policy_page(&policy, &|| true, &mut cache, &DiscoveryCursor::default()).unwrap();
    assert_eq!(first.len(), 512);
    assert!(!coverage.blocked && !coverage.partial);
    let (second, cursor, coverage) =
        discover_policy_page(&policy, &|| true, &mut cache, &cursor.unwrap()).unwrap();
    assert_eq!(second.len(), 88);
    assert!(!coverage.blocked && !coverage.partial);
    assert!(cursor.is_none());
    let ids: BTreeSet<_> = first
        .iter()
        .chain(&second)
        .map(|selection| selection.candidate.source_id())
        .collect();
    assert_eq!(ids.len(), 600);
}

#[test]
fn cloud_scheduler_continues_at_the_first_unread_claude_source_after_its_read_budget() {
    let (directory, mut policy, project, _) = fixture();
    let root = directory.path().canonicalize().unwrap().join("claude");
    fs::create_dir(&root).unwrap();
    let oversized = fs::File::create(root.join("a.jsonl")).unwrap();
    oversized.set_len(65 * 1024 * 1024).unwrap();
    fs::write(
        root.join("b.jsonl"),
        format!(
            "{}\n",
            serde_json::json!({ "sessionId": "b", "cwd": project, "type": "user" })
        ),
    )
    .unwrap();
    policy.roots = vec![RootScope {
        path: root.to_str().unwrap().into(),
        source_scope_id: "scope".into(),
        source_kind: SourceKind::ClaudeJsonl,
    }];
    policy.projects = vec![ProjectSelection {
        source_kind: SourceKind::ClaudeJsonl,
        source_path: root.to_str().unwrap().into(),
        project_path: project.to_str().unwrap().into(),
    }];

    let mut cache = super::super::projects::Cache::default();
    let (first, cursor, _) =
        discover_policy_page(&policy, &|| true, &mut cache, &DiscoveryCursor::default()).unwrap();
    assert!(first.is_empty());
    let cursor = cursor.expect("the unread candidate needs a new page");
    assert_eq!(cursor.after.as_deref(), Some("a"));
    let (second, next, coverage) =
        discover_policy_page(&policy, &|| true, &mut cache, &cursor).unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].candidate.native_session_id(), "b");
    assert!(next.is_none());
    assert!(!coverage.partial && !coverage.blocked);
}

#[test]
fn cloud_scheduler_unavailable_root_does_not_skip_healthy_roots() {
    let (directory, mut policy, _, _) = fixture();
    policy.roots.insert(
        0,
        RootScope {
            path: directory
                .path()
                .canonicalize()
                .unwrap()
                .join("unavailable")
                .to_str()
                .unwrap()
                .into(),
            source_scope_id: "unavailable-scope".into(),
            source_kind: SourceKind::ClaudeJsonl,
        },
    );
    let mut cache = super::super::projects::Cache::default();
    let now = Instant::now();
    let (first, next, coverage) = discover_policy_page_with_clock(
        &policy,
        &|| true,
        &mut cache,
        &DiscoveryCursor::default(),
        || now,
    )
    .unwrap();
    assert!(coverage.blocked);
    assert!(next.is_some());
    assert_eq!(first.len(), 512);
}

#[test]
fn cloud_scheduler_content_observation_errors_do_not_discard_healthy_pages() {
    let (_directory, policy, _, _) = fixture();
    let connection = rusqlite::Connection::open(&policy.roots[0].path).unwrap();
    connection.execute("INSERT INTO session_message VALUES ('broken','session-0001','user',1,?1)",
        [r#"{"files":[{"uri":"media:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#]).unwrap();
    let mut cache = super::super::projects::Cache::default();
    let now = Instant::now();
    let (first, cursor, coverage) = discover_policy_page_with_clock(
        &policy,
        &|| true,
        &mut cache,
        &DiscoveryCursor::default(),
        || now,
    )
    .unwrap();
    assert!(coverage.blocked);
    assert_eq!(first.len(), 511);
    assert_eq!(first[0].candidate.native_session_id(), "session-0000");
    assert_eq!(first[1].candidate.native_session_id(), "session-0002");
    let (second, cursor, coverage) =
        discover_policy_page_with_clock(&policy, &|| true, &mut cache, &cursor.unwrap(), || now)
            .unwrap();
    assert!(!coverage.blocked && !coverage.partial);
    assert_eq!(second.len(), 512);
    let (last, cursor, coverage) =
        discover_policy_page_with_clock(&policy, &|| true, &mut cache, &cursor.unwrap(), || now)
            .unwrap();
    assert!(!coverage.blocked && !coverage.partial);
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
            runner
                .candidates
                .back()
                .map(|selection| selection.candidate.native_session_id())
        );
    }
}
