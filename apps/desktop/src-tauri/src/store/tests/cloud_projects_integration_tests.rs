use super::*;

#[test]
fn cloud_folder_queries_use_their_page_indexes() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let connection = store.lock();
    for (query, keys, index) in [
        (SUGGESTION_PAGE, &["", "", "32"][..], "cloud_session_cwd"),
        (
            OPENCODE_HINTS,
            &["/Example/project", "32"][..],
            "cloud_opencode_cwd",
        ),
        (CLAUDE_SOURCE_PAGE, &["", "32"][..], "cloud_claude_source"),
    ] {
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {query}"))
            .unwrap();
        let steps = statement
            .query_map(rusqlite::params_from_iter(keys), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            steps.iter().any(|step| step.contains(index)),
            "Expected {index}: {steps:?}"
        );
    }
}

#[test]
fn cloud_folder_queries_do_not_wait_for_the_writer_connection() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let writer = store.lock();
    writer
        .execute_batch(
            "INSERT INTO session(environment_key,agent,session_id,source_kind,source_label,cwd,
              first_seen_at,last_seen_at,updated_at_epoch)
             VALUES('native','pi','synthetic','file','synthetic','/Example/project','synthetic','synthetic',1);",
        )
        .unwrap();
    let reader = store
        .open_reader(std::time::Duration::from_millis(100))
        .unwrap();
    let rows = reader.cloud_project_suggestions_page(None, 32).unwrap();
    assert_eq!(rows[0].cwd, "/Example/project");
    drop(writer);
}

#[test]
fn cloud_project_pages_include_old_rows_all_agents_and_separate_environments() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    for (index, environment, agent, cwd) in [
        (1, "native", "claude-code", "/Example/project"),
        (2, "native", "codex", "/Example/project"),
        (3, "wsl:example", "claude-code", "/Example/project"),
        (4, "native", "pi", "/Example/older"),
    ] {
        store
            .lock()
            .execute(
                "INSERT INTO session(environment_key,agent,session_id,source_kind,source_label,cwd,
                first_seen_at,last_seen_at,updated_at_epoch)
             VALUES(?1,?2,?3,'file','/Example/native/session.jsonl',?4,'synthetic','synthetic',1)",
                params![environment, agent, format!("session-{index}"), cwd],
            )
            .unwrap();
    }
    let first = store.cloud_project_suggestions_page(None, 1).unwrap();
    assert_eq!(first[0].cwd, "/Example/older");
    let cursor = Cursor {
        environment: first[0].environment.clone(),
        cwd: first[0].cwd.clone(),
    };
    let next = store
        .cloud_project_suggestions_page(Some(&cursor), 2)
        .unwrap();
    assert_eq!(next.len(), 2);
    assert_eq!(next[0].agents.len(), 2);
    assert_eq!(next[0].session_count, 2);
    assert_eq!(next[1].environment, "wsl:example");
    assert_eq!(
        store.cloud_claude_source_paths_page("", 512).unwrap(),
        vec!["/Example/native/session.jsonl"]
    );
}

#[test]
fn cloud_project_native_id_hints_are_bounded_and_exclude_other_formats_and_environments() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store
        .lock()
        .execute_batch(
            "WITH RECURSIVE sessions(n) AS (
                VALUES(0) UNION ALL SELECT n+1 FROM sessions WHERE n < 512
             )
             INSERT INTO session(environment_key,agent,session_id,source_kind,source_label,
                source_format,cwd,first_seen_at,last_seen_at,updated_at_epoch)
             SELECT 'native','opencode', printf('session-%04d',n),'providerDb',
                'opencode:' || printf('session-%04d',n) || ':open_code_v2_sqlite',
                '\"open_code_v2_sqlite\"','/Example/project','synthetic','synthetic',1
             FROM sessions;",
        )
        .unwrap();
    for (id, environment, kind, label, format) in [
        (
            "foreign",
            "wsl:synthetic",
            "providerDb",
            "open_code_v2_sqlite",
            "open_code_v2_sqlite",
        ),
        (
            "a-v1",
            "native",
            "providerDb",
            "open_code_v2_sqlite",
            "open_code_sqlite_v2",
        ),
        (
            "inline",
            "native",
            "inline",
            "open_code_v2_sqlite",
            "open_code_v2_sqlite",
        ),
        (
            "a-wrong-label",
            "native",
            "providerDb",
            "other",
            "open_code_v2_sqlite",
        ),
    ] {
        store
            .lock()
            .execute(
                "INSERT INTO session(environment_key,agent,session_id,source_kind,source_label,
                    source_format,cwd,first_seen_at,last_seen_at,updated_at_epoch)
                 VALUES(?1,'opencode',?2,?3,?4,?5,'/Example/project','synthetic','synthetic',1)",
                params![
                    environment,
                    id,
                    kind,
                    format!("opencode:{id}:{label}"),
                    format!("\"{format}\"")
                ],
            )
            .unwrap();
    }
    let ids = store
        .cloud_opencode_session_ids(&["/Example/project".into()], 512)
        .unwrap();
    assert_eq!(ids.len(), 512);
    assert_eq!(ids[0], "session-0000");
    assert_eq!(ids[511], "session-0511");
    assert!(
        store
            .cloud_opencode_session_ids(&[], 512)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .cloud_opencode_session_ids(&["/Example/other".into()], 512)
            .unwrap()
            .is_empty()
    );
    assert!(store.cloud_opencode_session_ids(&[], 513).is_err());
    store
        .lock()
        .execute(
            "INSERT INTO session(environment_key,agent,session_id,source_kind,source_label,
            source_format,cwd,first_seen_at,last_seen_at,updated_at_epoch)
             VALUES('native','opencode','z-last','providerDb',
                 'opencode:z-last:open_code_v2_sqlite','\"open_code_v2_sqlite\"',
                 '/Example/last','synthetic','synthetic',1),
                ('native','opencode','middle-1','providerDb',
                 'opencode:middle-1:open_code_v2_sqlite','\"open_code_v2_sqlite\"',
                 '/Example/middle','synthetic','synthetic',1),
                ('native','opencode','middle-2','providerDb',
                 'opencode:middle-2:open_code_v2_sqlite','\"open_code_v2_sqlite\"',
                 '/Example/middle','synthetic','synthetic',1)",
            [],
        )
        .unwrap();
    let shared = store
        .cloud_opencode_session_ids(&["/Example/project".into(), "/Example/last".into()], 512)
        .unwrap();
    assert!(shared.iter().any(|id| id == "z-last"));
    assert!(shared.len() <= 512);
    let shared = store
        .cloud_opencode_session_ids(
            &[
                "/Example/project".into(),
                "/Example/middle".into(),
                "/Example/last".into(),
            ],
            4,
        )
        .unwrap();
    assert_eq!(
        shared,
        ["session-0000", "session-0001", "middle-1", "z-last"]
    );
}
