use super::app_config_dir_in;
use super::*;
use crate::discovery::SessionMirror;
use serde_json::json;
use serial_test::serial;
use tempfile::TempDir;

/// Where an embedding application would keep its Antigravity mirror in these
/// tests. Nothing in the engine chooses this location.
fn mirror_dir(home: &Path) -> Option<PathBuf> {
    Some(home.join("mirror").join("antigravity"))
}

fn spawn_edges_path(home: &Path) -> Option<PathBuf> {
    Some(home.join("mirror").join("antigravity-spawn-edges.json"))
}

/// An adapter configured the way an embedding application would configure it.
static MIRRORED: AntigravityExplorer = AntigravityExplorer {
    mirror: SessionMirror {
        dir: mirror_dir,
        path_marker: Some("/mirror/antigravity/"),
    },
    spawn_edges_path,
};

#[tokio::test]
async fn test_brain_main_transcript_under_unknown_root_stays_a_file() {
    let dir = TempDir::new().unwrap();
    let logs_dir = dir
        .path()
        .join("brain")
        .join("uuid-xyz")
        .join(".system_generated")
        .join("logs");
    tokio::fs::create_dir_all(&logs_dir).await.unwrap();
    let transcript = logs_dir.join("transcript.jsonl");
    tokio::fs::write(&transcript, "{}\n").await.unwrap();

    assert!(is_brain_transcript_main(&transcript));
    assert!(brain_origin_of(&transcript).is_none());

    match classify_session_file(&transcript) {
        SessionFileDecision::File => {}
        _ => panic!("expected a file source"),
    }
}

#[tokio::test]
async fn test_antigravity_log_dirs_cached_collects_chat_sessions() {
    let home = TempDir::new().unwrap();
    let ws_root = app_config_dir_in("Antigravity", home.path())
        .join("User")
        .join("workspaceStorage")
        .join("abc")
        .join("chatSessions");
    tokio::fs::create_dir_all(&ws_root).await.unwrap();
    let other_root = app_config_dir_in("Antigravity", home.path())
        .join("User")
        .join("other")
        .join("chatSessions");
    tokio::fs::create_dir_all(&other_root).await.unwrap();

    let dirs = DISK_ANTIGRAVITY.log_dirs_in(home.path()).await;
    assert!(dirs.contains(&ws_root));
    assert!(dirs.contains(&other_root));
}

#[tokio::test]
async fn test_log_dirs_include_mirror_with_json() {
    let home = TempDir::new().unwrap();
    let mirror = mirror_dir(home.path()).unwrap();
    tokio::fs::create_dir_all(&mirror).await.unwrap();
    tokio::fs::write(mirror.join("cascade-1.json"), "{}")
        .await
        .unwrap();

    assert!(
        MIRRORED.log_dirs_in(home.path()).await.contains(&mirror),
        "a mirror holding cascade JSON should be included in discovery"
    );
    assert!(
        !DISK_ANTIGRAVITY
            .log_dirs_in(home.path())
            .await
            .contains(&mirror),
        "an unconfigured adapter never looks at a mirror"
    );
}

#[tokio::test]
async fn test_log_dirs_exclude_empty_mirror() {
    let home = TempDir::new().unwrap();
    let mirror = mirror_dir(home.path()).unwrap();
    tokio::fs::create_dir_all(&mirror).await.unwrap();

    assert!(
        !MIRRORED.log_dirs_in(home.path()).await.contains(&mirror),
        "empty mirror should not be included"
    );
}

#[tokio::test]
async fn test_antigravity_log_dirs_includes_cli_brain() {
    let home = TempDir::new().unwrap();
    let brain_dir = home
        .path()
        .join(".gemini")
        .join("antigravity-cli")
        .join("brain")
        .join("12345");
    tokio::fs::create_dir_all(&brain_dir).await.unwrap();
    tokio::fs::write(brain_dir.join("memory.jsonl"), "{}\n")
        .await
        .unwrap();

    let dirs = gemini_brain_dirs_for_overrides(home.path(), None).await;
    assert!(
        dirs.contains(&brain_dir),
        "expected brain dir to be discovered, got {:?}",
        dirs
    );
}

#[tokio::test]
async fn test_antigravity_log_dirs_includes_ide_brain() {
    let home = TempDir::new().unwrap();
    let brain_dir = home
        .path()
        .join(".gemini")
        .join("antigravity-ide")
        .join("brain")
        .join("3afb6691-6ba3-4a01-bd37-df54a0c1ee82")
        .join(".system_generated")
        .join("logs");
    tokio::fs::create_dir_all(&brain_dir).await.unwrap();
    tokio::fs::write(brain_dir.join("transcript.jsonl"), "{}\n")
        .await
        .unwrap();

    let dirs = gemini_brain_dirs_for_overrides(home.path(), None).await;
    assert!(
        dirs.contains(&brain_dir),
        "expected v2 IDE brain dir to be discovered, got {:?}",
        dirs
    );
}

#[tokio::test]
async fn test_antigravity_log_dirs_includes_legacy_brain() {
    let home = TempDir::new().unwrap();
    let brain_dir = home
        .path()
        .join(".gemini")
        .join("antigravity")
        .join("brain")
        .join("eaec3433-3eae-457a-a5bf-6bea5d323317");
    tokio::fs::create_dir_all(&brain_dir).await.unwrap();
    tokio::fs::write(brain_dir.join("state.json"), "{}")
        .await
        .unwrap();

    let dirs = gemini_brain_dirs_for_overrides(home.path(), None).await;
    assert!(
        dirs.contains(&brain_dir),
        "expected legacy ~/.gemini/antigravity/brain dir to be discovered, got {:?}",
        dirs
    );
}

#[tokio::test]
async fn test_antigravity_cli_honors_gemini_home_override() {
    let home = TempDir::new().unwrap();
    let gemini_home = TempDir::new().unwrap();
    let brain_dir = gemini_home
        .path()
        .join("antigravity-cli")
        .join("brain")
        .join("99");
    tokio::fs::create_dir_all(&brain_dir).await.unwrap();
    tokio::fs::write(brain_dir.join("state.json"), "{}")
        .await
        .unwrap();

    let dirs = gemini_brain_dirs_for_overrides(home.path(), Some(gemini_home.path())).await;
    assert!(
        dirs.contains(&brain_dir),
        "expected GEMINI_HOME override brain dir to be discovered, got {:?}",
        dirs
    );

    // Default ~/.gemini/... path should NOT be picked up when override is set
    // (the override fully replaces the gemini root).
    let default_brain_dir = home
        .path()
        .join(".gemini")
        .join("antigravity-cli")
        .join("brain");
    assert!(
        dirs.iter().all(|d| !d.starts_with(&default_brain_dir)),
        "override should replace default ~/.gemini path"
    );
}

#[tokio::test]
async fn test_antigravity_log_dirs_handles_missing_cli_brain() {
    let home = TempDir::new().unwrap();
    let ws_root = app_config_dir_in("Antigravity", home.path())
        .join("User")
        .join("workspaceStorage")
        .join("abc")
        .join("chatSessions");
    tokio::fs::create_dir_all(&ws_root).await.unwrap();

    // No ~/.gemini/antigravity-cli/brain seeded; must not panic.
    let dirs = DISK_ANTIGRAVITY.log_dirs_in(home.path()).await;
    assert!(dirs.contains(&ws_root));
}

#[tokio::test]
async fn test_antigravity_cli_jsonl_discovered_end_to_end() {
    use crate::discovery::set_file_mtime;

    let home = TempDir::new().unwrap();
    let brain_dir = home
        .path()
        .join(".gemini")
        .join("antigravity-cli")
        .join("brain")
        .join("777");
    tokio::fs::create_dir_all(&brain_dir).await.unwrap();
    let transcript = brain_dir.join("trace.jsonl");
    tokio::fs::write(&transcript, "{\"session_id\":\"x\"}\n")
        .await
        .unwrap();

    let now: i64 = 1_700_000_000;
    let since_secs: i64 = 7 * 86_400;
    set_file_mtime(&transcript, now - 60);

    let dirs = gemini_brain_dirs_for_overrides(home.path(), None).await;
    let files = recent_files_with_exts(&dirs, now, since_secs, SESSION_FILE_EXTS).await;
    assert!(
        files.iter().any(|f| f.path == transcript),
        "expected brain trace.jsonl to be discovered, got {:?}",
        files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let logs: Vec<SessionLog> = files
        .into_iter()
        .map(|file| SessionLog {
            environment: Default::default(),
            agent_type: AgentKind::Antigravity,
            source: SessionSource::File(file.path),
            updated_at: Some(file.mtime_epoch),
        })
        .collect();
    assert!(
        logs.iter()
            .any(|l| matches!(l.agent_type, AgentKind::Antigravity))
    );
}

/// End-to-end: API cache file with recent mtime is discovered by the
/// same pipeline that `discover_recent()` uses internally.
#[tokio::test]
async fn test_mirrored_session_discovered_end_to_end() {
    use super::recent_files_with_exts;
    use crate::discovery::set_file_mtime;

    let home = TempDir::new().unwrap();
    let cache_dir = mirror_dir(home.path()).unwrap();
    tokio::fs::create_dir_all(&cache_dir).await.unwrap();

    let now: i64 = 1_700_000_000;
    let since_secs: i64 = 7 * 86_400;

    let cascade = cache_dir.join("abc-123.json");
    let payload = json!({
        "sessionId": "abc-123",
        "cascadeId": "abc-123",
        "source": "antigravity_api",
        "baseUri": { "path": "file:///Users/dev/my-project" },
        "steps": []
    });
    tokio::fs::write(&cascade, payload.to_string())
        .await
        .unwrap();
    set_file_mtime(&cascade, now - 3600);

    let dirs = MIRRORED.log_dirs_in(home.path()).await;
    assert!(dirs.contains(&cache_dir));

    let files = recent_files_with_exts(&dirs, now, since_secs, &["json"]).await;
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, cascade);

    let logs: Vec<SessionLog> = files
        .into_iter()
        .map(|file| SessionLog {
            environment: Default::default(),
            agent_type: AgentKind::Antigravity,
            source: SessionSource::File(file.path),
            updated_at: Some(file.mtime_epoch),
        })
        .collect();
    assert_eq!(logs.len(), 1);
    assert!(matches!(logs[0].agent_type, AgentKind::Antigravity));
}

#[test]
fn test_excluded_subagent_matches_api_cache_and_brain_paths() {
    let worker_id = "0ecd1325-823d-4803-a9b1-5462db214ce4";
    let orchestrator_id = "2aabe6e8-dc5f-4257-8f71-08fbb11ae6f3";
    let child_ids = HashSet::from([worker_id.to_string()]);

    let worker_api_cache = PathBuf::from(format!("/cache/{worker_id}.json"));
    let worker_brain = PathBuf::from(format!(
        "/home/.gemini/antigravity-cli/brain/{worker_id}/.system_generated/logs/trace.jsonl"
    ));
    let orchestrator_api_cache = PathBuf::from(format!("/cache/{orchestrator_id}.json"));
    let orchestrator_brain = PathBuf::from(format!(
        "/home/.gemini/antigravity-cli/brain/{orchestrator_id}/.system_generated/logs/trace.jsonl"
    ));

    assert!(is_excluded_subagent(&worker_api_cache, &child_ids));
    assert!(is_excluded_subagent(&worker_brain, &child_ids));
    assert!(!is_excluded_subagent(&orchestrator_api_cache, &child_ids));
    assert!(!is_excluded_subagent(&orchestrator_brain, &child_ids));
}

// -----------------------------------------------------------------------
// Brain transcript metadata
// -----------------------------------------------------------------------

/// Helper: write a brain transcript at the expected nested path.
async fn write_brain_transcript(home: &Path, subroot: &str, uuid: &str, body: &str) -> PathBuf {
    let dir = home
        .join(".gemini")
        .join(subroot)
        .join("brain")
        .join(uuid)
        .join(".system_generated")
        .join("logs");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let path = dir.join("transcript.jsonl");
    tokio::fs::write(&path, body).await.unwrap();
    path
}

async fn augmented_metadata(path: &Path, preview: &str) -> SessionMetadata {
    let mut metadata = SessionMetadata::default();
    augment_brain_metadata(path, preview, &mut metadata).await;
    metadata
}

#[tokio::test]
async fn test_brain_metadata_uses_session_uuid_from_path() {
    let home = TempDir::new().unwrap();
    let uuid = "bfc4823e-f866-41cb-99eb-98194e0fea4e";
    let path = write_brain_transcript(home.path(), "antigravity-cli", uuid, "").await;

    let metadata = augmented_metadata(&path, "").await;
    assert_eq!(metadata.session_id.as_deref(), Some(uuid));
}

#[tokio::test]
async fn test_brain_metadata_extracts_active_document_cwd() {
    let home = TempDir::new().unwrap();
    let body = json!({
            "step_index": 0,
            "source": "USER_EXPLICIT",
            "type": "USER_INPUT",
            "status": "DONE",
            "created_at": "2026-05-25T04:19:47Z",
            "content": "<USER_REQUEST>\nhelp\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nActive Document: /tmp/foo/bar.rs (LANGUAGE_RUST)\n</ADDITIONAL_METADATA>",
        })
        .to_string();
    let path = write_brain_transcript(home.path(), "antigravity-ide", "u1", &body).await;

    let metadata = augmented_metadata(&path, &body).await;
    assert_eq!(metadata.cwd.as_deref(), Some("/tmp/foo"));
}

/// Legacy `~/.gemini/antigravity/brain/` sessions have no structured
/// cwd source, so we still prose-sniff `[label](file:///path)` markdown
/// links from PLANNER_RESPONSE content as a last-resort fallback.
#[tokio::test]
async fn test_brain_metadata_legacy_markdown_link_fallback() {
    let home = TempDir::new().unwrap();
    let body = format!(
        "{}\n{}\n",
        json!({
            "step_index": 0,
            "type": "USER_INPUT",
            "status": "DONE",
            "created_at": "2026-05-25T04:37:50Z",
            "content": "<USER_REQUEST>\nhello\n</USER_REQUEST>",
        }),
        json!({
            "step_index": 2,
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "created_at": "2026-05-25T04:37:50Z",
            "content": "Working in [demo-app](file:///Users/avery/demo-app)",
        })
    );
    // Legacy subroot, NOT antigravity-cli.
    let path = write_brain_transcript(home.path(), "antigravity", "u2", &body).await;

    let metadata = augmented_metadata(&path, &body).await;
    assert_eq!(metadata.cwd.as_deref(), Some("/Users/avery/demo-app"));
}

/// CLI brain transcripts derive cwd and title from
/// `~/.gemini/antigravity-cli/history.jsonl` (the CLI's own structured
/// session record), correlated to the brain transcript via the first
/// step's `created_at` timestamp.
#[tokio::test]
async fn test_brain_metadata_uses_cli_history_jsonl() {
    let home = TempDir::new().unwrap();
    // 2026-05-25T05:03:48Z = epoch 1779685428
    let body = format!(
        "{}\n",
        json!({
            "step_index": 0,
            "source": "USER_EXPLICIT",
            "type": "USER_INPUT",
            "status": "DONE",
            "created_at": "2026-05-25T05:03:48Z",
            "content": "<USER_REQUEST>\nhello world (test III)\n</USER_REQUEST>",
        }),
    );
    let path = write_brain_transcript(home.path(), "antigravity-cli", "u4", &body).await;

    // Seed history.jsonl with a matching entry.
    let history_path = home
        .path()
        .join(".gemini")
        .join("antigravity-cli")
        .join("history.jsonl");
    tokio::fs::create_dir_all(history_path.parent().unwrap())
        .await
        .unwrap();
    let history_line = json!({
        "display": "hello world (test III) from Antigravity CLI",
        "timestamp": 1779685428642i64,
        "workspace": "/Users/avery/demo-app",
    })
    .to_string();
    tokio::fs::write(&history_path, format!("{history_line}\n"))
        .await
        .unwrap();

    let metadata = augmented_metadata(&path, &body).await;
    assert_eq!(metadata.cwd.as_deref(), Some("/Users/avery/demo-app"));
    assert_eq!(
        metadata.title.as_deref(),
        Some("hello world (test III) from Antigravity CLI")
    );
}

/// A single malformed line in `history.jsonl` (missing field, wrong
/// type, or unparseable JSON) must not discard the valid entries
/// surrounding it.
#[tokio::test]
async fn test_read_cli_history_skips_malformed_lines() {
    let home = TempDir::new().unwrap();
    let history_path = home
        .path()
        .join(".gemini")
        .join("antigravity-cli")
        .join("history.jsonl");
    tokio::fs::create_dir_all(history_path.parent().unwrap())
        .await
        .unwrap();

    let good_a = json!({
        "display": "first session",
        "timestamp": 1779685428642i64,
        "workspace": "/Users/avery/repo-a",
    })
    .to_string();
    // Missing `workspace` — old schema / partial write.
    let missing_field = json!({
        "display": "broken — missing workspace",
        "timestamp": 1779685500000i64,
    })
    .to_string();
    // `timestamp` is a string, not an i64 — type mismatch.
    let wrong_type = json!({
        "display": "broken — wrong type",
        "timestamp": "not-a-number",
        "workspace": "/Users/avery/repo-b",
    })
    .to_string();
    // Truncated JSON — partial write from a crash.
    let unparseable = "{\"display\":\"truncat";
    let good_b = json!({
        "display": "second session",
        "timestamp": 1779685600000i64,
        "workspace": "/Users/avery/repo-c",
    })
    .to_string();

    let oversized = "x".repeat(64 * 1024 + 1);
    let contents =
        format!("{good_a}\n{missing_field}\n{wrong_type}\n{unparseable}\n{oversized}\n{good_b}\n");
    tokio::fs::write(&history_path, contents).await.unwrap();

    let history = read_cli_history(&home.path().join(".gemini"))
        .await
        .expect("file exists, should return Some");
    assert_eq!(
        history.len(),
        2,
        "expected 2 valid entries, got {history:?}"
    );
    assert_eq!(history[0].display, "first session");
    assert_eq!(history[0].workspace, "/Users/avery/repo-a");
    assert_eq!(history[1].display, "second session");
    assert_eq!(history[1].workspace, "/Users/avery/repo-c");
}

#[tokio::test]
async fn test_read_cli_history_retains_recent_entries_after_the_limit() {
    let home = TempDir::new().unwrap();
    let history_path = home
        .path()
        .join(".gemini")
        .join("antigravity-cli")
        .join("history.jsonl");
    tokio::fs::create_dir_all(history_path.parent().unwrap())
        .await
        .unwrap();
    let mut contents = String::new();
    for index in 0..4_100_i64 {
        contents.push_str(
            &json!({
                "display": format!("session {index}"),
                "timestamp": 1_779_000_000_000_i64 + index * 1_000,
                "workspace": format!("/tmp/repo-{index}"),
            })
            .to_string(),
        );
        contents.push('\n');
    }
    tokio::fs::write(&history_path, contents).await.unwrap();

    let history = read_cli_history(&home.path().join(".gemini"))
        .await
        .expect("history reads");

    assert_eq!(history.len(), 4_096);
    assert_eq!(history.first().unwrap().display, "session 4");
    assert_eq!(history.last().unwrap().display, "session 4099");
    assert_eq!(
        find_cli_history_entry(&history, 1_779_004_099)
            .unwrap()
            .workspace,
        "/tmp/repo-4099"
    );
}

/// CLI brain transcripts without a matching history entry leave both
/// `cwd` and `title` unset — we do NOT sniff random prose for CLI.
#[tokio::test]
async fn test_brain_metadata_cli_no_history_omits_metadata() {
    let home = TempDir::new().unwrap();
    let body = format!(
        "{}\n{}\n",
        json!({
            "step_index": 0,
            "type": "USER_INPUT",
            "status": "DONE",
            "created_at": "2026-05-25T05:03:48Z",
            "content": "<USER_REQUEST>\nhello\n</USER_REQUEST>",
        }),
        // Even an enticing markdown link in PLANNER_RESPONSE is ignored
        // for CLI: history.jsonl is the only structured source we trust.
        json!({
            "step_index": 2,
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "created_at": "2026-05-25T05:03:48Z",
            "content": "[repo](file:///Users/avery/demo-app)",
        })
    );
    let path = write_brain_transcript(home.path(), "antigravity-cli", "u5", &body).await;

    let metadata = augmented_metadata(&path, &body).await;
    assert!(metadata.cwd.is_none());
    assert!(metadata.title.is_none());
}

/// IDE / legacy USER_INPUT `<USER_REQUEST>` content becomes the title.
#[tokio::test]
async fn test_brain_metadata_title_from_user_request_for_ide() {
    let home = TempDir::new().unwrap();
    let body = json!({
            "step_index": 0,
            "source": "USER_EXPLICIT",
            "type": "USER_INPUT",
            "status": "DONE",
            "created_at": "2026-05-25T04:37:50Z",
            "content": "<USER_REQUEST>\nhello world from Antigravity IDE\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nignored\n</ADDITIONAL_METADATA>",
        })
        .to_string();
    let path = write_brain_transcript(home.path(), "antigravity-ide", "u3", &body).await;

    let metadata = augmented_metadata(&path, &body).await;
    assert_eq!(
        metadata.title.as_deref(),
        Some("hello world from Antigravity IDE")
    );
}

#[tokio::test]
async fn test_brain_metadata_normalizes_long_multiline_title() {
    let home = TempDir::new().unwrap();
    let words = (0..260).map(|_| "word").collect::<Vec<_>>().join("\n");
    let body = json!({
        "type": "USER_INPUT",
        "created_at": "2026-05-25T04:37:50Z",
        "content": format!("<USER_REQUEST>\n{words}\n</USER_REQUEST>"),
    })
    .to_string();
    let path = write_brain_transcript(home.path(), "antigravity-ide", "long-title", &body).await;

    let metadata = augmented_metadata(&path, &body).await;
    let title = metadata.title.expect("title");

    assert_eq!(title.chars().count(), 200);
    assert!(!title.contains('\n'));
    assert!(!title.contains("  "));
}

// Mutates the global `HOME`; `#[serial]` prevents the discover_recent
// sibling tests from clobbering each other's temp home under parallel runs.
#[tokio::test(flavor = "current_thread")]
#[serial]
async fn test_discover_recent_keeps_brain_transcript_as_file() {
    use crate::discovery::set_file_mtime;

    let home = TempDir::new().unwrap();
    // Override HOME so AntigravityExplorer::discover_recent resolves to
    // our temp dir for both ~/.gemini and ~/Library paths.
    // SAFETY: pinned to the current_thread flavor above so the tokio
    // runtime cannot spawn worker threads that observe the env mutation.
    // Other tests in this binary may still race; if a second test starts
    // mutating HOME, both must move to a serialized fixture.
    unsafe {
        std::env::set_var("HOME", home.path());
    }

    let uuid = "bfc4823e-f866-41cb-99eb-98194e0fea4e";
    let body = format!(
        "{}\n",
        json!({
            "step_index": 0,
            "source": "USER_EXPLICIT",
            "type": "USER_INPUT",
            "status": "DONE",
            "created_at": "2026-05-25T04:37:50Z",
            "content": "<USER_REQUEST>\nhello world from Antigravity CLI\n</USER_REQUEST>",
        }),
    );
    let path = write_brain_transcript(home.path(), "antigravity-cli", uuid, &body).await;

    // 2026-05-25T04:37:50Z = epoch 1779683870
    let history_path = home
        .path()
        .join(".gemini")
        .join("antigravity-cli")
        .join("history.jsonl");
    tokio::fs::write(
        &history_path,
        json!({
            "display": "hello world from Antigravity CLI",
            "timestamp": 1779683870000i64,
            "workspace": "/Users/avery/demo-app",
        })
        .to_string()
            + "\n",
    )
    .await
    .unwrap();

    let now: i64 = 1_700_000_000;
    let since_secs: i64 = 7 * 86_400;
    set_file_mtime(&path, now - 60);

    let logs = DISK_ANTIGRAVITY.discover_recent(now, since_secs).await;
    let file = logs
        .iter()
        .find(|log| matches!(&log.source, SessionSource::File(candidate) if candidate == &path))
        .expect("expected a file SessionLog for the brain transcript");

    let metadata = crate::discovery::session_log_read(file)
        .await
        .unwrap()
        .metadata;
    assert_eq!(metadata.session_id.as_deref(), Some(uuid));
    assert_eq!(metadata.cwd.as_deref(), Some("/Users/avery/demo-app"));
    assert_eq!(
        metadata.title.as_deref(),
        Some("hello world from Antigravity CLI")
    );
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn test_native_database_replaces_matching_brain_transcript() {
    use crate::discovery::set_file_mtime;

    let home = TempDir::new().unwrap();
    let old_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let session_id = "database-session";
    let body = format!(
        "{}\n",
        json!({
            "type": "USER_INPUT",
            "created_at": "2026-05-25T04:37:50Z",
            "content": "<USER_REQUEST>\ninspect database\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nActive Document: /tmp/project/main.rs (LANGUAGE_RUST)\n</ADDITIONAL_METADATA>",
        })
    );
    let transcript =
        write_brain_transcript(home.path(), "antigravity-ide", session_id, &body).await;
    let conversations = home
        .path()
        .join(".gemini")
        .join("antigravity-ide")
        .join("conversations");
    std::fs::create_dir_all(&conversations).unwrap();
    let db_path = conversations.join(format!("{session_id}.db"));
    let connection = Connection::open(&db_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);
             CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB);",
        )
        .unwrap();
    connection
        .execute("INSERT INTO steps(idx) VALUES (2), (7)", [])
        .unwrap();
    connection
        .execute("INSERT INTO gen_metadata(idx, data) VALUES (3, X'00')", [])
        .unwrap();
    drop(connection);
    let now = 1_800_000_000;
    set_file_mtime(&transcript, now - 30);
    set_file_mtime(&db_path, now - 10);

    let logs = DISK_ANTIGRAVITY.discover_recent(now, 3_600).await;

    assert_eq!(logs.len(), 1);
    assert!(matches!(
        &logs[0].source,
        SessionSource::ProviderDb {
            agent: AgentKind::Antigravity,
            db_path: path,
            session_id: id,
        } if path == &db_path && id == session_id
    ));
    assert_eq!(logs[0].surface_label(home.path()), "ide_desktop");
    let metadata = crate::discovery::session_log_read(&logs[0])
        .await
        .unwrap()
        .metadata;
    assert_eq!(metadata.session_id.as_deref(), Some(session_id));
    assert_eq!(metadata.cwd.as_deref(), Some("/tmp/project"));
    assert_eq!(metadata.title.as_deref(), Some("inspect database"));
    let located = crate::discovery::Explorers::DISK
        .locate_session_source(&AgentKind::Antigravity, session_id)
        .await;
    assert!(matches!(
        located,
        Some(SessionSource::ProviderDb { db_path: path, .. }) if path == db_path
    ));

    match old_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
}

#[test]
fn test_database_fingerprint_includes_step_and_generation_state() {
    let directory = TempDir::new().unwrap();
    let db_path = directory.path().join("session.db");
    let connection = Connection::open(&db_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);
             CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB);",
        )
        .unwrap();
    let empty = db_fingerprint_connection(&connection).unwrap();
    connection
        .execute("INSERT INTO steps(idx) VALUES (4)", [])
        .unwrap();
    let step_changed = db_fingerprint_connection(&connection).unwrap();
    connection.execute("DELETE FROM steps", []).unwrap();
    connection
        .execute("INSERT INTO steps(idx) VALUES (7)", [])
        .unwrap();
    let step_max_changed = db_fingerprint_connection(&connection).unwrap();
    connection
        .execute("INSERT INTO steps(idx) VALUES (2)", [])
        .unwrap();
    let step_count_changed = db_fingerprint_connection(&connection).unwrap();
    connection
        .execute("UPDATE steps SET metadata = X'00' WHERE idx = 2", [])
        .unwrap();
    let step_content_added = db_fingerprint_connection(&connection).unwrap();
    connection
        .execute("UPDATE steps SET metadata = X'01' WHERE idx = 2", [])
        .unwrap();
    let step_content_changed = db_fingerprint_connection(&connection).unwrap();
    connection
        .execute("INSERT INTO gen_metadata(idx, data) VALUES (9, X'00')", [])
        .unwrap();
    let generation_changed = db_fingerprint_connection(&connection).unwrap();
    connection.execute("DELETE FROM gen_metadata", []).unwrap();
    connection
        .execute("INSERT INTO gen_metadata(idx, data) VALUES (11, X'00')", [])
        .unwrap();
    let generation_max_changed = db_fingerprint_connection(&connection).unwrap();
    connection
        .execute("INSERT INTO gen_metadata(idx, data) VALUES (3, X'00')", [])
        .unwrap();
    let generation_count_changed = db_fingerprint_connection(&connection).unwrap();
    connection
        .execute("UPDATE gen_metadata SET data = X'01' WHERE idx = 3", [])
        .unwrap();
    let generation_content_changed = db_fingerprint_connection(&connection).unwrap();

    assert_ne!(empty, step_changed);
    assert_ne!(step_changed, step_max_changed);
    assert_ne!(step_max_changed, step_count_changed);
    assert_ne!(step_count_changed, step_content_added);
    assert_ne!(step_content_added, step_content_changed);
    assert_ne!(step_content_changed, generation_changed);
    assert_ne!(generation_changed, generation_max_changed);
    assert_ne!(generation_max_changed, generation_count_changed);
    assert_ne!(generation_count_changed, generation_content_changed);
}

#[test]
fn test_database_fingerprint_includes_the_sibling_transcript() {
    let directory = TempDir::new().unwrap();
    let session_id = "session";
    let conversations = directory.path().join("conversations");
    std::fs::create_dir_all(&conversations).unwrap();
    let db_path = conversations.join(format!("{session_id}.db"));
    Connection::open(&db_path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);
             CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB);",
        )
        .unwrap();
    let transcript = sibling_brain_transcript(&db_path, session_id).unwrap();
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, b"{}\n").unwrap();
    let before = db_fingerprint(&db_path, session_id).unwrap();

    std::fs::write(&transcript, b"{}\n{}\n").unwrap();

    assert_ne!(before, db_fingerprint(&db_path, session_id).unwrap());
}

#[test]
fn test_session_ids_cannot_escape_the_conversations_directory() {
    assert!(is_safe_session_id("session-123"));
    for session_id in ["", ".", "..", "../outside", "nested/session", "/absolute"] {
        assert!(!is_safe_session_id(session_id));
    }
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn test_invalid_database_keeps_the_matching_brain_transcript() {
    use crate::discovery::set_file_mtime;

    let home = TempDir::new().unwrap();
    let old_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let session_id = "usable-transcript";
    let transcript = write_brain_transcript(
        home.path(),
        "antigravity-ide",
        session_id,
        "{\"type\":\"USER_INPUT\",\"content\":\"hello\"}\n",
    )
    .await;
    let conversations = home.path().join(".gemini/antigravity-ide/conversations");
    std::fs::create_dir_all(&conversations).unwrap();
    let db_path = conversations.join(format!("{session_id}.db"));
    std::fs::write(&db_path, []).unwrap();
    let now = 1_800_000_000;
    set_file_mtime(&transcript, now - 1);
    set_file_mtime(&db_path, now - 1);

    let logs = DISK_ANTIGRAVITY.discover_recent(now, 60).await;

    assert_eq!(logs.len(), 1);
    assert!(matches!(&logs[0].source, SessionSource::File(path) if path == &transcript));

    match old_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
}

#[test]
fn test_database_surface_distinguishes_cli_from_ide() {
    let home = TempDir::new().unwrap();
    let log = |subroot: &str| SessionLog {
        environment: Default::default(),
        agent_type: AgentKind::Antigravity,
        source: SessionSource::ProviderDb {
            agent: AgentKind::Antigravity,
            db_path: home
                .path()
                .join(".gemini")
                .join(subroot)
                .join("conversations")
                .join("session.db"),
            session_id: "session".to_owned(),
        },
        updated_at: None,
    };

    assert_eq!(log("antigravity-cli").surface_label(home.path()), "cli");
    assert_eq!(
        log("antigravity-ide").surface_label(home.path()),
        "ide_desktop"
    );
    assert_eq!(log("antigravity").surface_label(home.path()), "ide_desktop");
}

#[tokio::test]
async fn test_database_metadata_falls_back_to_the_filename_without_a_transcript() {
    let directory = TempDir::new().unwrap();
    let session_id = "database-only";
    let db_path = directory
        .path()
        .join(".gemini")
        .join("antigravity-ide")
        .join("conversations")
        .join(format!("{session_id}.db"));

    let metadata = db_session_metadata(db_path, session_id.to_owned())
        .await
        .unwrap();

    assert_eq!(metadata.session_id.as_deref(), Some(session_id));
    assert_eq!(metadata.agent_type, Some(AgentKind::Antigravity));
    assert_eq!(metadata.title, None);
    assert_eq!(metadata.cwd, None);
}

#[tokio::test]
async fn test_database_freshness_includes_the_wal() {
    use crate::discovery::set_file_mtime;

    let directory = TempDir::new().unwrap();
    let db_path = directory.path().join("session.db");
    let wal_path = directory.path().join("session.db-wal");
    std::fs::write(&db_path, []).unwrap();
    std::fs::write(&wal_path, []).unwrap();
    set_file_mtime(&db_path, 100);
    set_file_mtime(&wal_path, 200);

    assert_eq!(database_mtime_epoch(&db_path).await, Some(200));
}

#[tokio::test]
async fn test_conversation_database_scan_covers_all_native_roots() {
    let home = TempDir::new().unwrap();
    let mut expected = Vec::new();
    for subroot in ["antigravity-cli", "antigravity-ide", "antigravity"] {
        let conversations = home
            .path()
            .join(".gemini")
            .join(subroot)
            .join("conversations");
        std::fs::create_dir_all(&conversations).unwrap();
        let path = conversations.join(format!("{subroot}.db"));
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);")
            .unwrap();
        expected.push(path);
    }

    let databases = conversation_databases_in(home.path(), 0).await;

    assert_eq!(databases.len(), 3);
    assert!(
        expected
            .iter()
            .all(|path| databases.iter().any(|(candidate, _, _)| candidate == path))
    );
}

/// D7 pruning: the window is applied from cheap stats, before the database is
/// opened, so a database whose own mtime is old must still surface when its
/// sibling transcript is recent.
#[tokio::test]
async fn an_old_database_with_a_recent_sibling_transcript_is_still_discovered() {
    use crate::discovery::set_file_mtime;

    let home = TempDir::new().unwrap();
    let session_id = "recent-transcript-old-db";
    let conversations = home
        .path()
        .join(".gemini")
        .join("antigravity-cli")
        .join("conversations");
    std::fs::create_dir_all(&conversations).unwrap();
    let db_path = conversations.join(format!("{session_id}.db"));
    Connection::open(&db_path)
        .unwrap()
        .execute_batch("CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);")
        .unwrap();
    let transcript = home
        .path()
        .join(".gemini")
        .join("antigravity-cli")
        .join("brain")
        .join(session_id)
        .join(".system_generated")
        .join("logs")
        .join("transcript.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, "{}\n").unwrap();

    let now = 1_800_000_000;
    let cutoff = now - 3_600;
    set_file_mtime(&db_path, cutoff - 10_000);
    set_file_mtime(&transcript, now - 60);

    let databases = conversation_databases_in(home.path(), cutoff).await;

    assert_eq!(databases.len(), 1);
    assert_eq!(databases[0].0, db_path);
    assert_eq!(databases[0].2, now - 60);
}

/// D7 pruning: an old, quiet database (and an equally old or absent sibling
/// transcript) never gets opened at all — the window excludes it from cheap
/// stats alone.
#[tokio::test]
async fn an_old_quiet_database_is_never_opened() {
    use crate::discovery::set_file_mtime;

    let home = TempDir::new().unwrap();
    let session_id = "old-quiet-database";
    let conversations = home
        .path()
        .join(".gemini")
        .join("antigravity-cli")
        .join("conversations");
    std::fs::create_dir_all(&conversations).unwrap();
    let db_path = conversations.join(format!("{session_id}.db"));
    Connection::open(&db_path)
        .unwrap()
        .execute_batch("CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);")
        .unwrap();

    let now = 1_800_000_000;
    let cutoff = now - 3_600;
    set_file_mtime(&db_path, cutoff - 10_000);

    track_database_opens(&db_path);
    let databases = conversation_databases_in(home.path(), cutoff).await;

    assert!(databases.is_empty());
    assert_eq!(take_tracked_database_opens(&db_path), 0);
}

/// Sidecar `.json` artifacts that share a brain `<uuid>/` dir
/// (`task.md.metadata.json`, plan metadata, …) must NOT be emitted as
/// session File logs. Otherwise they shadow the real transcript when
/// `locate_session_source` substring-matches the UUID, leaving the session
/// with no analyzable transcript ("no analysis for this session").
// Mutates the global `HOME`; `#[serial]` prevents the discover_recent
// sibling tests from clobbering each other's temp home under parallel runs.
#[tokio::test(flavor = "current_thread")]
#[serial]
async fn test_discover_recent_skips_brain_sidecar_artifacts() {
    use crate::discovery::set_file_mtime;

    let home = TempDir::new().unwrap();
    // SAFETY: see test_discover_recent_emits_inline_for_brain_transcript.
    unsafe {
        std::env::set_var("HOME", home.path());
    }

    let uuid = "95f23c70-6a9d-49ba-a929-8636a13296e2";
    let body = format!(
        "{}\n",
        json!({
            "step_index": 0,
            "type": "USER_INPUT",
            "status": "DONE",
            "created_at": "2026-05-25T04:37:50Z",
            "content": "<USER_REQUEST>\nreview engine.rs\n</USER_REQUEST>",
        }),
    );
    let transcript = write_brain_transcript(home.path(), "antigravity-ide", uuid, &body).await;

    // A metadata sidecar directly under the brain `<uuid>/` dir, sharing the
    // transcript's mtime so it can sort ahead of it.
    let sidecar = home
        .path()
        .join(".gemini")
        .join("antigravity-ide")
        .join("brain")
        .join(uuid)
        .join("task.md.metadata.json");
    tokio::fs::write(&sidecar, r#"{"title":"task"}"#)
        .await
        .unwrap();

    let now: i64 = 1_700_000_000;
    let since_secs: i64 = 7 * 86_400;
    set_file_mtime(&transcript, now - 60);
    set_file_mtime(&sidecar, now - 60);

    let logs = DISK_ANTIGRAVITY.discover_recent(now, since_secs).await;

    // The sidecar must not appear as a session at all.
    let sidecar_emitted = logs
        .iter()
        .any(|log| matches!(&log.source, SessionSource::File(p) if p == &sidecar));
    assert!(
        !sidecar_emitted,
        "brain sidecar artifact must not be emitted as a session File log"
    );

    // locate_session_source must resolve the UUID to the transcript,
    // not the sidecar.
    let located = crate::discovery::Explorers::DISK
        .locate_session_source(&AgentKind::Antigravity, uuid)
        .await;
    assert!(
        matches!(located, Some(SessionSource::File(ref path)) if path == &transcript),
        "expected the brain transcript file source, got {located:?}"
    );
}
