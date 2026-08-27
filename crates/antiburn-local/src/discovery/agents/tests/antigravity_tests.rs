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
async fn test_brain_main_transcript_falls_back_to_file_when_synthesis_fails() {
    // A path shaped like a brain main transcript but under an *unrecognized*
    // subroot makes `synthesize_brain_inline_payload` return `None`
    // (`brain_origin_of` is `None`). The session must still surface as a
    // `File` rather than being dropped (F9).
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

    // It *is* a brain main transcript, but not under a known subroot, so
    // synthesis bails before reading.
    assert!(is_brain_transcript_main(&transcript));
    assert!(brain_origin_of(&transcript).is_none());
    assert!(
        synthesize_brain_inline_payload(&transcript, None)
            .await
            .is_none()
    );

    match classify_session_file(&transcript, None).await {
        SessionFileDecision::File => {}
        _ => panic!("expected File fallback when synthesis returns None"),
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
// Brain-transcript -> cascade-shape synthesis
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

/// Helper: parse the metadata-header line (line 1) of a synthesized
/// payload as JSON.
fn parse_header(payload: &str) -> serde_json::Value {
    let header_line = payload
        .lines()
        .next()
        .expect("synthesized payload should have at least one line");
    serde_json::from_str(header_line).expect("synthesized header line should be valid JSON")
}

#[tokio::test]
async fn test_synthesize_brain_inline_payload_session_uuid_from_path() {
    let home = TempDir::new().unwrap();
    let uuid = "bfc4823e-f866-41cb-99eb-98194e0fea4e";
    let path = write_brain_transcript(home.path(), "antigravity-cli", uuid, "").await;

    let (got_uuid, payload) = synthesize_brain_inline_payload(&path, None).await.unwrap();
    assert_eq!(got_uuid, uuid);

    let header = parse_header(&payload);
    assert_eq!(header.get("sessionId").and_then(|v| v.as_str()), Some(uuid));
    assert_eq!(
        header.get("source").and_then(|v| v.as_str()),
        Some("antigravity_brain")
    );
}

#[tokio::test]
async fn test_synthesize_brain_inline_payload_extracts_active_document_cwd() {
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

    let (_uuid, payload) = synthesize_brain_inline_payload(&path, None).await.unwrap();
    let header = parse_header(&payload);
    // Active Document gives a file path; synthesizer normalizes to the
    // parent directory so the scanner's `cwd` consumer gets a dir.
    assert_eq!(header.get("cwd").and_then(|v| v.as_str()), Some("/tmp/foo"));
}

/// Legacy `~/.gemini/antigravity/brain/` sessions have no structured
/// cwd source, so we still prose-sniff `[label](file:///path)` markdown
/// links from PLANNER_RESPONSE content as a last-resort fallback.
#[tokio::test]
async fn test_synthesize_brain_inline_payload_legacy_markdown_link_fallback() {
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

    let (_uuid, payload) = synthesize_brain_inline_payload(&path, None).await.unwrap();
    let header = parse_header(&payload);
    assert_eq!(
        header.get("cwd").and_then(|v| v.as_str()),
        Some("/Users/avery/demo-app")
    );
}

/// CLI brain transcripts derive cwd and title from
/// `~/.gemini/antigravity-cli/history.jsonl` (the CLI's own structured
/// session record), correlated to the brain transcript via the first
/// step's `created_at` timestamp.
#[tokio::test]
async fn test_synthesize_brain_inline_payload_uses_cli_history_jsonl() {
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

    let history = read_cli_history(&home.path().join(".gemini")).await;
    assert!(history.is_some());
    let (_uuid, payload) = synthesize_brain_inline_payload(&path, history.as_deref())
        .await
        .unwrap();
    let header = parse_header(&payload);
    assert_eq!(
        header.get("cwd").and_then(|v| v.as_str()),
        Some("/Users/avery/demo-app")
    );
    assert_eq!(
        header.get("title").and_then(|v| v.as_str()),
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

    let contents = format!("{good_a}\n{missing_field}\n{wrong_type}\n{unparseable}\n{good_b}\n");
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

/// CLI brain transcripts without a matching history entry leave both
/// `cwd` and `title` unset — we do NOT sniff random prose for CLI.
#[tokio::test]
async fn test_synthesize_brain_inline_payload_cli_no_history_omits_metadata() {
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

    let (_uuid, payload) = synthesize_brain_inline_payload(&path, None).await.unwrap();
    let header = parse_header(&payload);
    assert!(header.get("cwd").is_none());
    assert!(header.get("title").is_none());
}

/// IDE / legacy USER_INPUT `<USER_REQUEST>` content becomes the title.
#[tokio::test]
async fn test_synthesize_brain_inline_payload_title_from_user_request_for_ide() {
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

    let (_uuid, payload) = synthesize_brain_inline_payload(&path, None).await.unwrap();
    let header = parse_header(&payload);
    assert_eq!(
        header.get("title").and_then(|v| v.as_str()),
        Some("hello world from Antigravity IDE")
    );
}

/// The raw transcript follows the header verbatim — uploads should
/// receive the original conversation untouched.
#[tokio::test]
async fn test_synthesize_brain_inline_payload_preserves_raw_transcript_body() {
    let home = TempDir::new().unwrap();
    let raw_line = json!({
        "step_index": 0,
        "type": "USER_INPUT",
        "content": "anything",
        "created_at": "2026-05-25T04:37:50Z",
    })
    .to_string();
    let body = format!("{raw_line}\n");
    let path = write_brain_transcript(home.path(), "antigravity-ide", "u6", &body).await;

    let (_uuid, payload) = synthesize_brain_inline_payload(&path, None).await.unwrap();
    let mut lines = payload.lines();
    let _header = lines.next().unwrap();
    assert_eq!(lines.next(), Some(raw_line.as_str()));
}

// Mutates the global `HOME`; `#[serial]` prevents the discover_recent
// sibling tests from clobbering each other's temp home under parallel runs.
#[tokio::test(flavor = "current_thread")]
#[serial]
async fn test_discover_recent_emits_inline_for_brain_transcript() {
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
    let inline = logs
        .iter()
        .find(|log| {
            matches!(
                &log.source,
                SessionSource::Inline { label, .. } if label.contains(uuid)
            )
        })
        .expect("expected an Inline SessionLog for the brain transcript");

    let content = match &inline.source {
        SessionSource::Inline { content, .. } => content.clone(),
        _ => unreachable!(),
    };
    let metadata = crate::discovery::scanner::parse_session_metadata_str(&content);
    assert_eq!(metadata.session_id.as_deref(), Some(uuid));
    assert_eq!(metadata.cwd.as_deref(), Some("/Users/avery/demo-app"));
    assert_eq!(
        metadata.title.as_deref(),
        Some("hello world from Antigravity CLI")
    );

    // No raw-File SessionLog should remain for the same transcript.
    let raw_file_for_same = logs
        .iter()
        .any(|log| matches!(&log.source, SessionSource::File(p) if p == &path));
    assert!(
        !raw_file_for_same,
        "brain transcript should be emitted only as Inline, not as a raw File source"
    );
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

    // locate_session_source must resolve the UUID to the Inline transcript,
    // not the sidecar.
    let located = crate::discovery::Explorers::DISK
        .locate_session_source(&AgentKind::Antigravity, uuid)
        .await;
    assert!(
        matches!(located, Some(SessionSource::Inline { ref label, .. }) if label.contains(uuid)),
        "expected the brain transcript Inline source, got {located:?}"
    );
}
