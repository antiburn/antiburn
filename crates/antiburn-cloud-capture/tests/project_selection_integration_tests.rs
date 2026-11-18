#![cfg(all(unix, feature = "cloud"))]

//! Synthetic native-file and SQLite project selection integration tests.

use antiburn_cloud_capture::*;
use rusqlite::{Connection, params};
use serde_json::json;
use sha2::Digest;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

fn temporary() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

fn directory(parent: &Path, name: &str) -> PathBuf {
    let path = parent.join(name);
    fs::create_dir_all(&path).unwrap();
    path
}

fn record(session: &str, cwd: &Path) -> String {
    format!("{}\n", json!({"sessionId": session, "cwd": cwd}))
}

fn claude(source: &Path, session: &str, bytes: impl AsRef<[u8]>) -> Candidate {
    fs::write(source.join(format!("{session}.jsonl")), bytes).unwrap();
    reopen_claude(source, session, None, &Limits::default()).unwrap()
}

#[test]
fn claude_project_selection_includes_descendants_but_not_sibling_prefixes() {
    let temp = temporary();
    let source = directory(temp.path(), "provider-storage");
    let selected = directory(temp.path(), "app");
    let child = directory(&selected, "nested");
    let sibling = directory(temp.path(), "app-other");
    let outside_worktree = directory(temp.path(), "separate-worktree");
    let selection = ProjectSelection::new(vec![selected.clone()]).unwrap();
    for (session, cwd, allowed) in [
        ("root", selected.as_path(), true),
        ("nested", child.as_path(), true),
        ("sibling", sibling.as_path(), false),
        ("worktree", outside_worktree.as_path(), false),
    ] {
        let candidate = claude(&source, session, record(session, cwd))
            .in_source_scope("account-source")
            .unwrap();
        assert_eq!(
            candidate.project_evidence().unwrap().project_path(),
            Some(cwd)
        );
        let admitted = selection.admit(&candidate).unwrap();
        assert_eq!(admitted.is_some(), allowed);
        if let Some(admitted) = admitted {
            assert_eq!(admitted.source_id(), candidate.source_id());
            assert_eq!(admitted.admitted_project_path(), Some(cwd));
        }
    }
    let separate = ProjectSelection::new(vec![outside_worktree.clone()]).unwrap();
    assert!(
        separate
            .admit(&reopen_claude(&source, "worktree", None, &Limits::default()).unwrap())
            .unwrap()
            .is_some()
    );
}

#[test]
fn deleted_recorded_cwd_does_not_block_an_existing_approved_cwd() {
    let temp = temporary();
    let source = directory(temp.path(), "provider");
    let selected = directory(temp.path(), "app");
    let deleted = selected.join("a-deleted-worktree");
    let valid = directory(&selected, "z-existing");
    let candidate = claude(
        &source,
        "session",
        format!(
            "{}{}",
            record("session", &deleted),
            record("session", &valid),
        ),
    );
    let selection = ProjectSelection::new(vec![selected]).unwrap();
    assert!(
        selection
            .matches(&candidate.project_evidence().unwrap())
            .unwrap()
    );
    let admitted = selection.admit(&candidate).unwrap().unwrap();
    assert_eq!(admitted.admitted_project_paths(), &[valid]);
}

#[test]
fn claude_storage_names_tool_paths_and_unowned_cwds_do_not_grant_permission() {
    let temp = temporary();
    let selected = directory(temp.path(), "app");
    let source = directory(temp.path(), "-encoded-app-path");
    let candidate = claude(
        &source,
        "unknown",
        format!(
            "{}\n",
            json!({
                "sessionId": "another-owner",
                "cwd": selected,
                "tool": {"cwd": selected, "path": selected},
                "forkedFrom": {"sessionId": "allowed-parent"}
            })
        ),
    );
    assert!(
        candidate
            .project_evidence()
            .unwrap()
            .project_path()
            .is_none()
    );
    for path in [source.clone(), selected.clone()] {
        assert!(
            ProjectSelection::new(vec![path])
                .unwrap()
                .admit(&candidate)
                .unwrap()
                .is_none()
        );
    }
    let proved = claude(&source, "proved", record("proved", &selected));
    assert!(
        ProjectSelection::new(vec![source])
            .unwrap()
            .admit(&proved)
            .unwrap()
            .is_none()
    );
    let sidechain = claude(
        &directory(temp.path(), "sidechains"),
        "root",
        format!(
            "{}\n",
            json!({"sessionId": "root", "isSidechain": true, "cwd": selected})
        ),
    );
    assert!(
        sidechain
            .project_evidence()
            .unwrap()
            .project_path()
            .is_none()
    );
}

#[test]
fn empty_project_selection_does_not_probe_native_bytes() {
    let temp = temporary();
    let source = directory(temp.path(), "provider");
    let candidate = claude(&source, "root", b"{}\n");
    fs::remove_file(source.join("root.jsonl")).unwrap();
    assert!(
        ProjectSelection::new(Vec::new())
            .unwrap()
            .admit(&candidate)
            .unwrap()
            .is_none()
    );
}

#[test]
fn claude_children_and_forks_need_their_own_recorded_cwd() {
    let temp = temporary();
    let source = directory(temp.path(), "provider");
    let selected = directory(temp.path(), "selected");
    let excluded = directory(temp.path(), "excluded");
    let children = directory(&source, "parent/subagents");
    claude(&source, "parent", record("parent", &selected));
    for (id, value, allowed) in [
        (
            "unknown",
            json!({"sessionId": "parent", "cwd": selected}),
            false,
        ),
        (
            "excluded",
            json!({"sessionId": "parent", "agentId": "excluded", "cwd": excluded}),
            false,
        ),
        (
            "selected",
            json!({"sessionId": "parent", "agentId": "selected", "isSidechain": true, "cwd": selected}),
            true,
        ),
    ] {
        fs::write(
            children.join(format!("agent-{id}.jsonl")),
            format!("{value}\n"),
        )
        .unwrap();
        let child = reopen_claude(&source, "parent", Some(id), &Limits::default()).unwrap();
        assert_eq!(
            ProjectSelection::new(vec![selected.clone()])
                .unwrap()
                .admit(&child)
                .unwrap()
                .is_some(),
            allowed
        );
    }
    let fork = claude(
        &source,
        "fork",
        format!(
            "{}{}\n",
            record("parent", &selected),
            json!({"sessionId": "fork", "cwd": excluded, "forkedFrom": {"sessionId": "parent"}})
        ),
    );
    assert!(
        ProjectSelection::new(vec![selected])
            .unwrap()
            .admit(&fork)
            .unwrap()
            .is_none()
    );
}

#[test]
fn claude_scan_keeps_changing_owned_cwds_and_bounds_unreadable_records() {
    let temp = temporary();
    let source = directory(temp.path(), "provider");
    let first = directory(temp.path(), "first");
    let second = directory(temp.path(), "second");
    let changing = claude(
        &source,
        "changing",
        format!(
            "{}{}",
            record("changing", &first),
            record("changing", &second)
        ),
    );
    let evidence = changing.project_evidence().unwrap();
    assert!(evidence.scan_complete());
    assert_eq!(evidence.project_paths(), &[first.clone(), second.clone()]);
    for folder in [&first, &second] {
        let admitted = ProjectSelection::new(vec![folder.clone()])
            .unwrap()
            .admit(&changing)
            .unwrap()
            .unwrap();
        assert_eq!(
            admitted.admitted_project_paths(),
            std::slice::from_ref(folder)
        );
    }
    let mut bytes = b"{\"ignored\":\"".to_vec();
    bytes.extend(vec![b'x'; PROJECT_RECORD_BYTES]);
    bytes.extend(b"\"}\n");
    bytes.extend(record("late", &first).as_bytes());
    let late = claude(&source, "late", bytes);
    let incomplete = late.project_evidence().unwrap();
    assert!(!incomplete.scan_complete());
    assert_eq!(incomplete.project_paths(), std::slice::from_ref(&first));
    assert!(
        ProjectSelection::new(vec![first.clone()])
            .unwrap()
            .admit(&late)
            .unwrap()
            .is_none()
    );
    let duplicate = claude(
        &source,
        "duplicate",
        format!(
            "{{\"sessionId\":\"duplicate\",\"cwd\":{},\"cwd\":{}}}\n",
            json!(first),
            json!(second)
        ),
    );
    assert!(
        duplicate
            .project_evidence()
            .unwrap()
            .project_path()
            .is_none()
    );
}

#[test]
fn claude_full_scan_finds_late_cwds_with_bounded_memory_and_explicit_byte_limits() {
    let temp = temporary();
    let source = directory(temp.path(), "provider");
    let first = directory(temp.path(), "first");
    let second = directory(temp.path(), "second");
    let mut bytes = record("changing", &first);
    bytes.push_str(&"{\"opaque\":\"discard this content\"}\n".repeat(64 * 1024 / 10));
    bytes.push_str(&record("changing", &second));
    let candidate = claude(&source, "changing", &bytes);
    let evidence = candidate.project_evidence().unwrap();
    assert!(evidence.scan_complete());
    assert_eq!(evidence.project_paths(), &[first, second]);
    let digest = sha2::Sha256::digest(bytes.as_bytes());
    let expected: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(evidence.source_digest(), Some(expected.as_str()));
    let limits = Limits {
        source_bytes: 64 * 1024,
        ..Limits::default()
    };
    assert!(matches!(
        candidate.project_evidence_with_limits(&limits),
        Err(Error::Limit)
    ));
}

#[test]
fn large_ignored_tool_results_keep_multiple_owned_cwds_available() {
    let temp = temporary();
    let source = directory(temp.path(), "provider");
    let first = directory(temp.path(), "first");
    let second = directory(temp.path(), "second");
    let tool_record = format!(
        "{}\n",
        json!({
            "sessionId": "changing",
            "cwd": first,
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "content": "synthetic output ".repeat(32 * 1024)
                }]
            },
            "tool": {"cwd": "/synthetic/unapproved-tool-path"}
        })
    );
    assert!(tool_record.len() > 64 * 1024);
    assert!(tool_record.len() < PROJECT_RECORD_BYTES);
    let candidate = claude(
        &source,
        "changing",
        format!(
            "{}{}{}",
            record("changing", &first),
            tool_record,
            record("changing", &second)
        ),
    );
    let evidence = candidate.project_evidence().unwrap();
    assert!(evidence.scan_complete());
    assert_eq!(evidence.project_paths(), &[first.clone(), second.clone()]);
    for folder in [first, second] {
        assert!(
            ProjectSelection::new(vec![folder])
                .unwrap()
                .admit(&candidate)
                .unwrap()
                .is_some()
        );
    }
}

#[test]
fn ambiguous_unowned_cwd_and_exceeded_path_sets_do_not_admit_capture() {
    let temp = temporary();
    let source = directory(temp.path(), "provider");
    let selected = directory(temp.path(), "selected");
    let ambiguous = claude(
        &source,
        "ambiguous",
        format!(
            "{}{}\n",
            record("ambiguous", &selected),
            json!({"cwd": selected})
        ),
    );
    assert!(!ambiguous.project_evidence().unwrap().scan_complete());
    assert!(
        ProjectSelection::new(vec![selected])
            .unwrap()
            .admit(&ambiguous)
            .unwrap()
            .is_none()
    );
    let mut bytes = String::new();
    for index in 0..257 {
        bytes.push_str(&record("many", &temp.path().join(format!("cwd-{index}"))));
    }
    let too_many = claude(&source, "many", bytes);
    assert!(matches!(too_many.project_evidence(), Err(Error::Limit)));
}

#[test]
fn opencode_both_shapes_use_only_each_sessions_recorded_directory() {
    for schema in [
        include_str!("../../../fixtures/cloud/opencode/earlier.sql"),
        include_str!("../../../fixtures/cloud/opencode/current.sql"),
    ] {
        let temp = temporary();
        let selected = directory(temp.path(), "app");
        let child = directory(&selected, "nested");
        let sibling = directory(temp.path(), "app-other");
        let database = temp.path().join("opencode.db");
        let connection = Connection::open(&database).unwrap();
        connection.execute_batch(schema).unwrap();
        connection
            .execute(
                "UPDATE session_v2 SET directory = ?1 WHERE id = 'ses_main'",
                [selected.to_str().unwrap()],
            )
            .unwrap();
        let selection = ProjectSelection::new(vec![selected.clone()]).unwrap();
        for (cwd, allowed) in [(&selected, true), (&child, true), (&sibling, false)] {
            connection
                .execute(
                    "UPDATE session_v2 SET directory = ?1 WHERE id = 'ses_main'",
                    [cwd.to_str().unwrap()],
                )
                .unwrap();
            let candidate = reopen_opencode(&database, "ses_main", &Limits::default()).unwrap();
            assert_eq!(
                candidate.project_evidence().unwrap().project_path(),
                Some(cwd.as_path())
            );
            assert_eq!(selection.admit(&candidate).unwrap().is_some(), allowed);
        }
        let required: bool = connection
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('session_v2') WHERE name = 'directory'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut unknown = vec![Some(""), Some("relative/path"), Some("/work/../alias")];
        if !required {
            unknown.push(None);
        }
        for value in unknown {
            connection
                .execute(
                    "UPDATE session_v2 SET directory = ?1 WHERE id = 'ses_main'",
                    [value],
                )
                .unwrap();
            let candidate = reopen_opencode(&database, "ses_main", &Limits::default()).unwrap();
            assert!(selection.admit(&candidate).unwrap().is_none());
        }
        connection
            .execute(
                "UPDATE session_v2 SET directory = zeroblob(5000) WHERE id = 'ses_main'",
                [],
            )
            .unwrap();
        let candidate = reopen_opencode(&database, "ses_main", &Limits::default()).unwrap();
        assert!(
            candidate
                .project_evidence()
                .unwrap()
                .project_path()
                .is_none()
        );
    }
}

#[test]
fn opencode_children_and_forks_do_not_inherit_parent_permission() {
    let temp = temporary();
    let selected = directory(temp.path(), "selected");
    let excluded = directory(temp.path(), "excluded");
    let database = temp.path().join("opencode.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(include_str!("../../../fixtures/cloud/opencode/earlier.sql"))
        .unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory = ?1 WHERE id = 'ses_main'",
            [selected.to_str().unwrap()],
        )
        .unwrap();
    for (id, parent, fork, cwd) in [
        ("child", Some("ses_main"), None, None),
        ("fork", None, Some("ses_main"), excluded.to_str()),
        ("independent", Some("absent"), None, selected.to_str()),
    ] {
        connection
            .execute(
                "INSERT INTO session_v2 (id, parent_id, fork_session_id, directory)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, parent, fork, cwd],
            )
            .unwrap();
        let candidate = reopen_opencode(&database, id, &Limits::default()).unwrap();
        assert_eq!(
            ProjectSelection::new(vec![selected.clone()])
                .unwrap()
                .admit(&candidate)
                .unwrap()
                .is_some(),
            id == "independent"
        );
    }
}

#[test]
fn opencode_project_probe_does_not_use_storage_root_or_read_message_payload() {
    let temp = temporary();
    let selected = directory(temp.path(), "selected");
    let storage = directory(temp.path(), "provider-storage");
    let database = storage.join("opencode.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(include_str!("../../../fixtures/cloud/opencode/earlier.sql"))
        .unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory = ?1 WHERE id = 'ses_main'",
            [selected.to_str().unwrap()],
        )
        .unwrap();
    connection
        .execute("UPDATE session_message SET data = zeroblob(2097152)", [])
        .unwrap();
    let candidate = reopen_opencode(&database, "ses_main", &Limits::default()).unwrap();
    assert_eq!(
        candidate.project_evidence().unwrap().project_path(),
        Some(selected.as_path())
    );
    assert!(
        ProjectSelection::new(vec![storage])
            .unwrap()
            .admit(&candidate)
            .unwrap()
            .is_none()
    );
    connection
        .execute_batch("ALTER TABLE session_v2 DROP COLUMN directory")
        .unwrap();
    assert!(
        candidate
            .project_evidence()
            .unwrap()
            .project_path()
            .is_none()
    );
    let alias = temp.path().join("database-alias.db");
    symlink(&database, &alias).unwrap();
    assert!(reopen_opencode(&alias, "ses_main", &Limits::default()).is_err());
}

#[test]
fn aliases_symlinks_and_replaced_sources_fail_closed() {
    let temp = temporary();
    let selected = directory(temp.path(), "selected");
    let source = directory(temp.path(), "provider");
    let alias = temp.path().join("alias");
    symlink(&selected, &alias).unwrap();
    assert!(ProjectSelection::new(vec![alias.clone()]).is_err());
    let nested_alias = selected.join("alias");
    symlink(&source, &nested_alias).unwrap();
    let candidate = claude(&source, "alias", record("alias", &nested_alias));
    assert!(
        ProjectSelection::new(vec![selected.clone()])
            .unwrap()
            .admit(&candidate)
            .is_err()
    );
    let source_alias = temp.path().join("provider-alias");
    symlink(&source, &source_alias).unwrap();
    assert!(reopen_claude(&source_alias, "alias", None, &Limits::default()).is_err());
    fs::remove_file(source.join("alias.jsonl")).unwrap();
    symlink(selected.join("outside.jsonl"), source.join("alias.jsonl")).unwrap();
    assert!(candidate.project_evidence().is_err());
    assert!(reopen_claude(&source, "../escape", None, &Limits::default()).is_err());
    assert!(ProjectSelection::new(vec![selected.join("..")]).is_err());
}

#[test]
fn reopened_candidates_keep_native_keys_and_scopes() {
    let temp = temporary();
    let source = directory(temp.path(), "provider");
    claude(&source, "root", b"{}\n");
    for candidate in discover_claude(&source, &Limits::default()).unwrap() {
        let reopened = reopen_claude(
            &source,
            candidate.native_session_id(),
            candidate.native_agent_id(),
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(
            candidate.in_source_scope("scope").unwrap().source_id(),
            reopened.in_source_scope("scope").unwrap().source_id()
        );
    }
    let database = temp.path().join("opencode.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(include_str!("../../../fixtures/cloud/opencode/earlier.sql"))
        .unwrap();
    let candidate = discover_opencode(&database, &Limits::default())
        .unwrap()
        .pop()
        .unwrap();
    let reopened =
        reopen_opencode(&database, candidate.native_session_id(), &Limits::default()).unwrap();
    assert_eq!(candidate.source_id(), reopened.source_id());
    assert_eq!(candidate.source_path(), reopened.source_path());
    assert_eq!(reopened.source_kind(), CaptureSourceKind::OpenCodeSqliteV2);
    assert!(reopen_opencode(&database, "absent", &Limits::default()).is_err());
}
