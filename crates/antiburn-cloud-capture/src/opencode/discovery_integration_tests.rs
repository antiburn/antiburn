use super::*;
use crate::{discover_opencode, discover_opencode_page, reopen_opencode};
use std::fs;

fn fixture() -> (tempfile::TempDir, PathBuf, Connection) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().canonicalize().unwrap().join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session_v2(id TEXT PRIMARY KEY, directory TEXT);
             CREATE TABLE session_message(
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
                seq INTEGER NOT NULL, data TEXT NOT NULL, UNIQUE(session_id,seq));
             WITH RECURSIVE sessions(n) AS (
                VALUES(0) UNION ALL SELECT n+1 FROM sessions WHERE n < 1024
             )
             INSERT INTO session_v2
             SELECT printf('session-%04d',n), '/Example/project' FROM sessions;",
        )
        .unwrap();
    (directory, path, connection)
}

#[test]
fn opencode_discovery_pages_cover_large_sources_without_changing_the_complete_listing_contract() {
    let (_directory, path, _connection) = fixture();
    let limits = Limits::default();
    assert!(matches!(
        discover_opencode(&path, &limits),
        Err(Error::Limit)
    ));
    let mut cursor = None;
    let mut ids = Vec::new();
    let mut sizes = Vec::new();
    loop {
        let page = discover_opencode_page(&path, cursor.as_deref(), &limits).unwrap();
        assert!(page.candidates.len() <= 512);
        sizes.push(page.candidates.len());
        ids.extend(
            page.candidates
                .iter()
                .map(|candidate| candidate.native_session_id().to_owned()),
        );
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(sizes, [512, 512, 1]);
    assert_eq!(
        ids,
        (0..1025)
            .map(|n| format!("session-{n:04}"))
            .collect::<Vec<_>>()
    );
}

#[test]
fn opencode_discovery_pages_use_binary_order_fresh_snapshots_and_current_cwd_evidence() {
    let (_directory, path, connection) = fixture();
    connection
        .execute_batch(
            "DELETE FROM session_v2;
             INSERT INTO session_v2 VALUES ('A','/Example/old'),('a','/Example/later');",
        )
        .unwrap();
    let limits = Limits {
        discovery_sessions: 1,
        ..Limits::default()
    };
    let first = discover_opencode_page(&path, None, &limits).unwrap();
    assert_eq!(first.candidates[0].native_session_id(), "A");
    assert_eq!(first.next_cursor.as_deref(), Some("A"));
    connection
        .execute_batch(
            "UPDATE session_v2 SET directory='/Example/current' WHERE id='A';
             INSERT INTO session_v2 VALUES ('B','/Example/new');",
        )
        .unwrap();
    let second = discover_opencode_page(&path, first.next_cursor.as_deref(), &limits).unwrap();
    assert_eq!(second.candidates[0].native_session_id(), "B");
    assert_eq!(
        first.candidates[0]
            .project_evidence()
            .unwrap()
            .project_path(),
        Some(Path::new("/Example/current"))
    );
    let reopened = reopen_opencode(&path, "a", &limits).unwrap();
    assert_eq!(reopened.native_session_id(), "a");
    connection
        .execute_batch("CREATE TABLE session_unknown(session_id TEXT);")
        .unwrap();
    assert!(matches!(
        discover_opencode_page(&path, second.next_cursor.as_deref(), &limits),
        Err(Error::UnsupportedSource)
    ));
}

#[test]
fn opencode_discovery_pages_keep_source_identity_and_input_bounds() {
    let (_directory, path, connection) = fixture();
    let page = discover_opencode_page(&path, None, &Limits::default()).unwrap();
    assert!(matches!(
        discover_opencode_page(&path, Some(&"x".repeat(129)), &Limits::default()),
        Err(Error::InvalidSource)
    ));
    assert!(matches!(
        discover_opencode_page(
            &path,
            None,
            &Limits {
                discovery_sessions: 513,
                ..Limits::default()
            }
        ),
        Err(Error::Limit)
    ));
    drop(connection);
    let replacement = path.with_file_name("replacement.db");
    fs::copy(&path, &replacement).unwrap();
    fs::rename(&replacement, &path).unwrap();
    assert!(page.candidates[0].project_evidence().is_err());
    assert!(discover_opencode_page(&path, page.next_cursor.as_deref(), &Limits::default()).is_ok());
}
