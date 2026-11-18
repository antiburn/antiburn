use super::*;

#[test]
fn cloud_project_pages_include_old_rows_all_agents_and_separate_environments() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    for (index, environment, agent, cwd) in [
        (1, "native", "claude", "/Example/project"),
        (2, "native", "codex", "/Example/project"),
        (3, "wsl:example", "claude", "/Example/project"),
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
