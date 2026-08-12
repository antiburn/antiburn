// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::discovery::scanner::{self, TitleSource};
use tempfile::TempDir;

fn pi_sessions_dir(home: &Path) -> PathBuf {
    home.join(".pi").join("agent").join("sessions")
}

#[tokio::test]
async fn test_log_dirs_finds_subdirs() {
    let home = TempDir::new().unwrap();
    let sessions = pi_sessions_dir(home.path());
    let project_dir = sessions.join("--Users-test-projects-foo--");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let dirs = log_dirs_in(home.path()).await;
    assert_eq!(dirs, vec![project_dir]);
}

#[tokio::test]
async fn test_log_dirs_graceful_when_missing() {
    let home = TempDir::new().unwrap();
    let dirs = log_dirs_in(home.path()).await;
    assert!(dirs.is_empty());
}

#[tokio::test]
async fn test_discover_recent_finds_jsonl_files() {
    let home = TempDir::new().unwrap();
    let sessions = pi_sessions_dir(home.path());
    let project_dir = sessions.join("--Users-test--");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let session_file =
        project_dir.join("2026-05-26T01-02-03-000Z_019e61cd-aaaa-bbbb-cccc-dddddddddddd.jsonl");
    tokio::fs::write(
        &session_file,
        r#"{"type":"session","cwd":"/Users/test","version":3}"#,
    )
    .await
    .unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let dirs = log_dirs_in(home.path()).await;
    let results = recent_files_with_exts(&dirs, now, 86_400, &["jsonl"]).await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].path, session_file);
}

#[tokio::test]
#[serial_test::serial]
async fn recent_provider_usage_records_preserve_session_identity_and_payload() {
    let home = TempDir::new().unwrap();
    let previous_home = std::env::var_os("HOME");
    unsafe { std::env::set_var("HOME", home.path()) };
    let project_dir = pi_sessions_dir(home.path()).join("--Users-test--");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let session_id = "019e61cd-aaaa-bbbb-cccc-dddddddddddd";
    let payload = r#"{"type":"message","message":{"role":"assistant","provider":"openai-codex"}}"#;
    tokio::fs::write(
        project_dir.join(format!("2026-05-26T01-02-03-000Z_{session_id}.jsonl")),
        payload,
    )
    .await
    .unwrap();

    let records = recent_session_payloads(0, 10, 1024).await;

    unsafe {
        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }
    assert_eq!(records, vec![(session_id.to_string(), payload.to_string())]);
}

#[tokio::test]
async fn test_discover_cwds_reads_cwd_from_session_record() {
    let home = TempDir::new().unwrap();
    let sessions = pi_sessions_dir(home.path());
    let project_dir = sessions.join("--Users-test-projects-bar--");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let session_file =
        project_dir.join("2026-05-26T01-02-03-000Z_019e61cd-aaaa-bbbb-cccc-dddddddddddd.jsonl");
    tokio::fs::write(
        &session_file,
        r#"{"type":"session","cwd":"/Users/test/projects/bar","version":3}
{"type":"message","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}"#,
    )
    .await
    .unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let cwds = discover_cwds_in(home.path(), now, 86_400).await;
    assert_eq!(cwds, vec!["/Users/test/projects/bar".to_string()]);
}

#[tokio::test]
async fn test_find_session_file_by_uuid_suffix() {
    let home = TempDir::new().unwrap();
    let sessions = pi_sessions_dir(home.path());
    let project_dir = sessions.join("--Users-test--");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let uuid = "019e61cd-aaaa-bbbb-cccc-dddddddddddd";
    let session_file = project_dir.join(format!("2026-05-26T01-02-03-000Z_{uuid}.jsonl"));
    tokio::fs::write(&session_file, r#"{"type":"session","cwd":"/Users/test"}"#)
        .await
        .unwrap();

    let dirs = vec![project_dir.clone()];
    let found = find_session_file_by_id(&dirs, uuid).await;
    assert_eq!(found, Some(session_file));
}

#[tokio::test]
async fn test_find_session_file_returns_none_when_missing() {
    let home = TempDir::new().unwrap();
    let sessions = pi_sessions_dir(home.path());
    let project_dir = sessions.join("--Users-test--");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let dirs = vec![project_dir];
    let found = find_session_file_by_id(&dirs, "nonexistent-uuid").await;
    assert!(found.is_none());
}

#[tokio::test]
async fn test_find_session_file_does_not_match_dash_separator() {
    let home = TempDir::new().unwrap();
    let sessions = pi_sessions_dir(home.path());
    let project_dir = sessions.join("--Users-test--");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let uuid = "019e61cd-aaaa-bbbb-cccc-dddddddddddd";
    let dash_file = project_dir.join(format!("2026-05-26T01-02-03-000Z-{uuid}.jsonl"));
    tokio::fs::write(&dash_file, r#"{"type":"session","cwd":"/Users/test"}"#)
        .await
        .unwrap();

    let dirs = vec![project_dir];
    let found = find_session_file_by_id(&dirs, uuid).await;
    assert!(found.is_none());
}

#[tokio::test]
async fn test_session_title_returns_first_user_message() {
    let home = TempDir::new().unwrap();
    let sessions = pi_sessions_dir(home.path());
    let project_dir = sessions.join("--Users-test--");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let uuid = "019e61cd-aaaa-bbbb-cccc-dddddddddddd";
    let session_file = project_dir.join(format!("2026-05-26T01-02-03-000Z_{uuid}.jsonl"));
    tokio::fs::write(
            &session_file,
            r#"{"type":"session","cwd":"/Users/test","version":3}
{"type":"message","message":{"role":"user","content":[{"type":"text","text":"fix the login bug"}]}}
{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"I'll look into that."}]}}"#,
        )
        .await
        .unwrap();

    let dirs = vec![project_dir];
    let path = find_session_file_by_id(&dirs, uuid).await.unwrap();
    let metadata = scanner::parse_session_metadata(&path).await;
    assert_eq!(metadata.title, Some("fix the login bug".to_string()));
}

#[tokio::test]
async fn test_cwd_from_first_line_extracts_cwd() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    tokio::fs::write(
        &file,
        r#"{"type":"session","id":"abc","cwd":"/some/path","version":3}
{"type":"message","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#,
    )
    .await
    .unwrap();

    let cwd = cwd_from_first_line(&file).await;
    assert_eq!(cwd, Some("/some/path".to_string()));
}

#[tokio::test]
async fn test_cwd_from_first_line_returns_none_for_no_cwd() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("session.jsonl");
    tokio::fs::write(&file, r#"{"type":"message","role":"user"}"#)
        .await
        .unwrap();

    let cwd = cwd_from_first_line(&file).await;
    assert!(cwd.is_none());
}

#[tokio::test]
async fn test_recover_session_id_from_path_extracts_uuid_suffix() {
    let path = Path::new(
        "/Users/foo/.pi/agent/sessions/--Users-foo-bar--/2026-05-26T01-02-03-000Z_019e61cd-aaaa-bbbb-cccc-dddddddddddd.jsonl",
    );
    assert_eq!(
        PiExplorer.recover_session_id_from_path(path),
        Some("019e61cd-aaaa-bbbb-cccc-dddddddddddd".to_string())
    );
}

#[tokio::test]
async fn test_recover_session_id_returns_none_for_filename_without_underscore() {
    // The Pi filename contract is `{timestamp}_{uuid}.jsonl`. A stem with
    // no underscore is malformed for Pi — return `None` rather than
    // promoting the whole stem to a session id.
    let path = Path::new("/Users/foo/.pi/agent/sessions/--proj--/orphan.jsonl");
    assert_eq!(PiExplorer.recover_session_id_from_path(path), None);
}

#[tokio::test]
async fn test_recover_session_id_returns_none_when_uuid_part_empty() {
    let path = Path::new("/Users/foo/.pi/agent/sessions/--proj--/timestamp_.jsonl");
    assert_eq!(PiExplorer.recover_session_id_from_path(path), None);
}

#[tokio::test]
async fn test_owns_path_matches_pi_session_path() {
    // Mirror the production normalisation in `agents::normalize_for_matching`:
    // backslashes → forward slashes, then lowercase. `Path::join` produces
    // backslashes on Windows, so the `\\` → `/` step is load-bearing for
    // cross-platform tests.
    let path = sample_log_path(Path::new("/Users/foo"));
    let path_lower = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    assert!(PiExplorer.owns_path(&path_lower));
}

#[tokio::test]
async fn test_owns_path_rejects_unrelated_path() {
    assert!(!PiExplorer.owns_path("/users/foo/.claude/projects/repo/session.jsonl"));
}

#[tokio::test]
async fn test_surface_paths_only_populates_cli() {
    let home = Path::new("/Users/foo");
    let sp = PiExplorer.surface_paths(home);
    assert_eq!(
        sp.cli,
        vec![home.join(".pi").join("agent").join("sessions")]
    );
    assert!(sp.ide_desktop.is_empty());
    assert!(sp.mirror.is_empty());
}

/// End-to-end title extraction: a Pi-shaped JSONL is parsed by the
/// shared scanner and the title comes back tagged as `FirstMessage`.
/// This is the path `default_session_titles_and_surfaces` (in `agents::
/// mod`) exercises for every Pi session under the `Scan` lookup branch.
#[tokio::test]
async fn test_parse_session_metadata_tags_pi_title_as_first_message() {
    let home = TempDir::new().unwrap();
    let sessions = pi_sessions_dir(home.path());
    let project_dir = sessions.join("--Users-test--");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let uuid = "019e61cd-aaaa-bbbb-cccc-dddddddddddd";
    let session_file = project_dir.join(format!("2026-05-26T01-02-03-000Z_{uuid}.jsonl"));
    tokio::fs::write(
            &session_file,
            r#"{"type":"session","cwd":"/Users/test","version":3}
{"type":"message","message":{"role":"user","content":[{"type":"text","text":"deploy the staging build"}]}}"#,
        )
        .await
        .unwrap();

    let metadata = scanner::parse_session_metadata(&session_file).await;
    assert_eq!(metadata.title.as_deref(), Some("deploy the staging build"));
    assert_eq!(metadata.title_source, Some(TitleSource::FirstMessage));
}

/// `discover_recent` must tag every returned `SessionLog` with
/// `AgentKind::Pi`; the rest of the pipeline (scanner, upload, surface
/// classification) keys off that field. Drives the real `PiExplorer.
/// discover_recent` end-to-end with HOME redirected at a tempdir.
///
/// Serialised because it mutates the `HOME` env var.
#[tokio::test]
#[serial_test::serial]
async fn test_discover_recent_tags_session_logs_as_pi() {
    let home = TempDir::new().unwrap();
    let prev_home = std::env::var_os("HOME");
    // SAFETY: serialised via `serial_test::serial`; no other test thread
    // can observe the partial env state.
    unsafe {
        std::env::set_var("HOME", home.path());
    }

    let sessions = pi_sessions_dir(home.path());
    let project_dir = sessions.join("--Users-test--");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();
    let session_file =
        project_dir.join("2026-05-26T01-02-03-000Z_019e61cd-aaaa-bbbb-cccc-dddddddddddd.jsonl");
    tokio::fs::write(
        &session_file,
        r#"{"type":"session","cwd":"/Users/test","version":3}"#,
    )
    .await
    .unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let logs = PiExplorer.discover_recent(now, 86_400).await;

    // SAFETY: restore HOME within the serialised section.
    unsafe {
        match prev_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    assert_eq!(logs.len(), 1, "expected exactly one discovered Pi log");
    assert_eq!(logs[0].agent_type, AgentKind::Pi);
    match &logs[0].source {
        SessionSource::File(p) => assert_eq!(p, &session_file),
        SessionSource::Inline { .. } => panic!("expected File source for Pi"),
        SessionSource::ProviderDb { .. } => panic!("expected File source for Pi"),
    }
}
