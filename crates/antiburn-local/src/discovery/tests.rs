// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Discovery framework tests: fan-out plumbing, path classification, and the
//! surface contract every adapter is held to.

use super::*;
use serial_test::serial;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tempfile::TempDir;

#[tokio::test]
async fn one_read_serves_metadata_preview_and_fingerprint() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let content = b"{\"type\":\"user\",\"sessionId\":\"session-1\",\"cwd\":\"/repo\"}\n";
    tokio::fs::write(&path, content)
        .await
        .expect("write source");
    let log = SessionLog {
        agent_type: AgentKind::Claude,
        source: SessionSource::File(path),
        updated_at: None,
        environment: Default::default(),
    };

    let read = session_log_read(&log).await.expect("source read");

    assert!(read.stat.is_some());
    assert_eq!(read.head_hash, Some(source_version::head_hash_of(content)));
    assert_eq!(read.content.as_deref(), std::str::from_utf8(content).ok());
    assert_eq!(read.metadata.session_id.as_deref(), Some("session-1"));
}

#[tokio::test]
async fn a_non_claude_file_source_also_has_a_head_hash() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let content = b"{\"session_id\":\"session-1\"}\n";
    tokio::fs::write(&path, content)
        .await
        .expect("write source");
    let log = SessionLog {
        agent_type: AgentKind::Pi,
        source: SessionSource::File(path),
        updated_at: None,
        environment: Default::default(),
    };

    let read = session_log_read(&log).await.expect("source read");

    assert_eq!(read.head_hash, Some(source_version::head_hash_of(content)));
}

#[tokio::test]
async fn the_preview_output_is_unchanged() {
    let dir = TempDir::new().expect("tempdir");
    let invalid_tail = dir.path().join("invalid-tail.jsonl");
    tokio::fs::write(&invalid_tail, b"valid\n\xf0\x9f")
        .await
        .expect("write invalid tail");
    let oversized = dir.path().join("oversized.jsonl");
    let oversized_bytes = vec![b'a'; SOURCE_PREVIEW_BYTES as usize + 1];
    tokio::fs::write(&oversized, &oversized_bytes)
        .await
        .expect("write oversized source");

    assert_eq!(
        session_source_preview(&SessionSource::File(invalid_tail)).await,
        Some("valid\n".to_string())
    );
    assert_eq!(
        session_source_preview(&SessionSource::File(oversized))
            .await
            .map(|content| content.len()),
        Some(SOURCE_PREVIEW_BYTES as usize)
    );
}

#[test]
fn the_preview_conversion_consumes_its_buffer() {
    let mut bytes = Vec::with_capacity(64);
    bytes.extend_from_slice(b"valid");
    let expected_capacity = bytes.capacity();

    let content = preview_from_owned(bytes).expect("preview");

    assert_eq!(content, "valid");
    assert_eq!(content.capacity(), expected_capacity);
}

#[test]
fn resolved_titles_are_normalized_and_unicode_safely_bounded() {
    let raw = format!("  Reader\n\trequest  {}  ", "🧪".repeat(250));
    let resolved = ResolvedTitle::new(raw, TitleSource::AiGenerated);

    assert_eq!(resolved.text.chars().count(), 200);
    assert!(resolved.text.starts_with("Reader request 🧪"));
    assert!(!resolved.text.contains('\n'));
    assert!(!resolved.text.contains('\t'));
}

#[test]
fn cwd_occurrences_merge_native_and_wsl_counts() {
    let mut counts = std::collections::HashMap::new();
    add_cwd_occurrences(&mut counts, ["/repo".into(), "/repo".into()]);
    add_cwd_occurrences(
        &mut counts,
        ["/repo".into(), r"\\wsl.localhost\Ubuntu\repo".into()],
    );

    assert_eq!(counts.get("/repo"), Some(&3));
    assert_eq!(counts.get(r"\\wsl.localhost\Ubuntu\repo"), Some(&1));
}

#[tokio::test]
async fn bounded_log_tasks_preserves_order_and_limits_concurrency() {
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let logs = (0..40)
        .map(|index| SessionLog {
            environment: Default::default(),
            agent_type: AgentKind::Claude,
            source: SessionSource::Inline {
                label: index.to_string(),
                content: String::new(),
            },
            updated_at: None,
        })
        .collect();

    let results = bounded_log_tasks(logs, |log| {
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        async move {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(2)).await;
            active.fetch_sub(1, Ordering::SeqCst);
            let SessionSource::Inline { label, .. } = log.source else {
                unreachable!()
            };
            Some(label)
        }
    })
    .await;

    assert_eq!(
        results,
        (0..40).map(|index| index.to_string()).collect::<Vec<_>>()
    );
    assert!(peak.load(Ordering::SeqCst) <= LOG_METADATA_CONCURRENCY);
    assert!(peak.load(Ordering::SeqCst) > 1);
}

#[tokio::test]
async fn cwd_session_discovery_timeout_preserves_agent_label() {
    let (agent, cwds) = cwds_per_recent_session(
        "Test Agent",
        std::future::pending(),
        Duration::from_millis(1),
    )
    .await;

    assert_eq!(agent, "Test Agent");
    assert!(cwds.is_empty());
}

fn timed_file_log(agent: AgentKind, path: &str, updated_at: Option<i64>) -> SessionLog {
    SessionLog {
        environment: Default::default(),
        agent_type: agent,
        source: SessionSource::File(PathBuf::from(path)),
        updated_at,
    }
}

#[test]
fn live_session_index_keeps_only_sessions_inside_the_window() {
    let now = 1_700_000_000;
    let logs = vec![
        timed_file_log(
            AgentKind::Claude,
            "/home/u/.claude/projects/p/aaaa-bbbb.jsonl",
            Some(now - 10),
        ),
        timed_file_log(
            AgentKind::Claude,
            "/home/u/.claude/projects/p/cccc-dddd.jsonl",
            Some(now - 500),
        ),
        timed_file_log(
            AgentKind::Claude,
            "/home/u/.claude/projects/p/eeee-ffff.jsonl",
            None,
        ),
    ];
    let index = Explorers::DISK.live_session_index(&logs, now, 180);

    // Fresh heartbeat → live; stale or unknown mtime → not live (the flag
    // drives extra refresh work, so unknown freshness must not pin it on).
    assert!(index.contains("claude-code", "aaaa-bbbb", "native"));
    assert!(!index.contains("claude-code", "cccc-dddd", "native"));
    assert!(!index.contains("claude-code", "eeee-ffff", "native"));
    // The key is (agent slug, id): another agent's slug never matches, and
    // an empty id must not substring-match every label.
    assert!(!index.contains("codex", "aaaa-bbbb", "native"));
    assert!(!index.contains("claude-code", "", "native"));
}

#[test]
fn live_session_index_falls_back_to_label_substring_for_embedded_ids() {
    // Codex rollout filenames embed the thread id but the file stem isn't
    // equal to it, so membership must fall back to a label substring match
    // (mirroring locate_session_source).
    let now = 1_700_000_000;
    let logs = vec![timed_file_log(
        AgentKind::Codex,
        "/home/u/.codex/sessions/rollout-2026-06-30T10-00-00-0195f4f9-ab12.jsonl",
        Some(now - 5),
    )];
    let index = Explorers::DISK.live_session_index(&logs, now, 180);

    assert!(index.contains("codex", "0195f4f9-ab12", "native"));
    assert!(!index.contains("codex", "some-other-thread", "native"));
}

#[test]
fn live_session_index_matches_provider_db_ids_directly() {
    let now = 1_700_000_000;
    let logs = vec![SessionLog {
        environment: Default::default(),
        agent_type: AgentKind::Cursor,
        source: SessionSource::ProviderDb {
            agent: AgentKind::Cursor,
            db_path: PathBuf::from("/home/u/cursor/state.db"),
            session_id: "composer-1".to_string(),
        },
        updated_at: Some(now - 30),
    }];
    let index = Explorers::DISK.live_session_index(&logs, now, 180);

    assert!(index.contains("cursor", "composer-1", "native"));
    assert!(!index.contains("cursor", "composer-2", "native"));
}

#[test]
fn supports_subagents_only_for_subagent_vendors() {
    // The gate lets `list_subagents` / `locate_subagent_source` skip
    // locating the parent transcript for vendors that could only ever
    // return the empty trait default. Keep in sync with the explorers
    // that override the sub-agent hooks.
    for (agent, expected) in [
        (AgentKind::Claude, true),
        (AgentKind::Codex, true),
        (AgentKind::Antigravity, true),
        (AgentKind::Cursor, false),
        (AgentKind::Copilot, false),
        (AgentKind::Cline, false),
        (AgentKind::OpenCode, false),
        (AgentKind::Kiro, false),
        (AgentKind::AmpCode, false),
        (AgentKind::Windsurf, false),
        (AgentKind::Pi, false),
    ] {
        assert_eq!(
            Explorers::DISK.get(&agent).supports_subagents(),
            expected,
            "{agent:?}"
        );
    }
}

#[tokio::test]
async fn test_recent_files_within_window() {
    let dir = TempDir::new().unwrap();
    let now: i64 = 1_700_000_000;
    let since_secs: i64 = 7 * 86_400; // 7 days

    // File modified recently (within window)
    let file = dir.path().join("recent.jsonl");
    tokio::fs::write(&file, "{}").await.unwrap();
    set_file_mtime(&file, now - 3 * 86_400); // 3 days ago

    let result =
        recent_files_with_exts(&[dir.path().to_path_buf()], now, since_secs, &["jsonl"]).await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].path, file);
}

#[tokio::test]
async fn test_recent_files_outside_window() {
    let dir = TempDir::new().unwrap();
    let now: i64 = 1_700_000_000;
    let since_secs: i64 = 7 * 86_400; // 7 days

    // File modified too long ago
    let file = dir.path().join("old.jsonl");
    tokio::fs::write(&file, "{}").await.unwrap();
    set_file_mtime(&file, now - 10 * 86_400); // 10 days ago

    let result =
        recent_files_with_exts(&[dir.path().to_path_buf()], now, since_secs, &["jsonl"]).await;
    assert!(result.is_empty());
}

#[tokio::test]
async fn test_recent_files_at_boundary() {
    let dir = TempDir::new().unwrap();
    let now: i64 = 1_700_000_000;
    let since_secs: i64 = 7 * 86_400;

    // File at exact cutoff (mtime == now - since_secs)
    let file = dir.path().join("boundary.jsonl");
    tokio::fs::write(&file, "{}").await.unwrap();
    set_file_mtime(&file, now - since_secs); // exactly at the cutoff

    let result =
        recent_files_with_exts(&[dir.path().to_path_buf()], now, since_secs, &["jsonl"]).await;
    assert_eq!(result.len(), 1, "file at exact cutoff should be included");
}

#[tokio::test]
async fn test_recent_files_ignores_non_jsonl() {
    let dir = TempDir::new().unwrap();
    let now: i64 = 1_700_000_000;

    let txt_file = dir.path().join("session.txt");
    tokio::fs::write(&txt_file, "{}").await.unwrap();
    set_file_mtime(&txt_file, now);

    let result = recent_files_with_exts(&[dir.path().to_path_buf()], now, 86_400, &["jsonl"]).await;
    assert!(result.is_empty());
}

#[tokio::test]
async fn test_recent_files_empty_dirs() {
    let result = recent_files_with_exts(&[], 1_700_000_000, 86_400, &["jsonl"]).await;
    assert!(result.is_empty());
}

#[tokio::test]
async fn test_recent_files_nonexistent_dir() {
    let result = recent_files_with_exts(
        &[PathBuf::from("/nonexistent/dir")],
        1_700_000_000,
        86_400,
        &["jsonl"],
    )
    .await;
    assert!(result.is_empty());
}

#[tokio::test]
async fn dir_has_json_files_only_for_directories_holding_json() {
    let dir = TempDir::new().unwrap();
    assert!(!dir_has_json_files(dir.path()).await);
    tokio::fs::write(dir.path().join("note.txt"), "x")
        .await
        .unwrap();
    assert!(!dir_has_json_files(dir.path()).await);
    tokio::fs::write(dir.path().join("one.json"), "{}")
        .await
        .unwrap();
    assert!(dir_has_json_files(dir.path()).await);
    assert!(!dir_has_json_files(&dir.path().join("missing")).await);
}

#[test]
#[serial]
fn test_home_dir_returns_some() {
    // In a normal test environment, HOME should be set
    let home = home_dir();
    assert!(home.is_some());
    assert!(home.unwrap().is_absolute());
}

#[test]
#[serial]
fn test_home_dir_falls_back_to_userprofile() {
    let home_backup = std::env::var("HOME").ok();
    let userprofile_backup = std::env::var("USERPROFILE").ok();
    let homedrive_backup = std::env::var("HOMEDRIVE").ok();
    let homepath_backup = std::env::var("HOMEPATH").ok();

    unsafe {
        std::env::remove_var("HOME");
        std::env::remove_var("HOMEDRIVE");
        std::env::remove_var("HOMEPATH");
        std::env::set_var("USERPROFILE", "/tmp/test-userprofile");
    }

    let result = home_dir();
    assert_eq!(result, Some(PathBuf::from("/tmp/test-userprofile")));

    match home_backup {
        Some(v) => unsafe { std::env::set_var("HOME", v) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match userprofile_backup {
        Some(v) => unsafe { std::env::set_var("USERPROFILE", v) },
        None => unsafe { std::env::remove_var("USERPROFILE") },
    }
    match homedrive_backup {
        Some(v) => unsafe { std::env::set_var("HOMEDRIVE", v) },
        None => unsafe { std::env::remove_var("HOMEDRIVE") },
    }
    match homepath_backup {
        Some(v) => unsafe { std::env::set_var("HOMEPATH", v) },
        None => unsafe { std::env::remove_var("HOMEPATH") },
    }
}

#[test]
#[serial]
fn test_app_config_dir_in_platform() {
    let home = PathBuf::from("/home/tester");
    let dir = app_config_dir_in("Code", &home);
    if cfg!(target_os = "macos") {
        assert_eq!(
            dir,
            PathBuf::from("/home/tester/Library/Application Support/Code")
        );
    } else if cfg!(target_os = "windows") {
        assert_eq!(dir, home.join("AppData").join("Roaming").join("Code"));
    } else {
        assert_eq!(dir, home.join(".config").join("Code"));
    }
}

#[test]
fn app_config_dir_for_platform_covers_windows_and_linux_paths() {
    let home = PathBuf::from("C:\\Users\\tester");
    assert_eq!(
        app_config_dir_for_platform(
            "Windsurf",
            &home,
            DesktopPlatform::Windows,
            Some(Path::new("D:\\Roaming")),
            None,
        ),
        PathBuf::from("D:\\Roaming").join("Windsurf")
    );
    assert_eq!(
        app_config_dir_for_platform("Windsurf", &home, DesktopPlatform::Windows, None, None),
        home.join("AppData").join("Roaming").join("Windsurf")
    );

    let linux_home = PathBuf::from("/home/tester");
    assert_eq!(
        app_config_dir_for_platform(
            "Antigravity",
            &linux_home,
            DesktopPlatform::Linux,
            None,
            Some(Path::new("/tmp/xdg-config")),
        ),
        PathBuf::from("/tmp/xdg-config").join("Antigravity")
    );
    assert_eq!(
        app_config_dir_for_platform(
            "Antigravity",
            &linux_home,
            DesktopPlatform::Linux,
            None,
            None
        ),
        linux_home.join(".config").join("Antigravity")
    );
}

fn sample_session(agent_type: AgentKind, label: &str) -> SessionLog {
    SessionLog {
        environment: Default::default(),
        agent_type,
        source: SessionSource::Inline {
            label: label.to_string(),
            content: "{}".to_string(),
        },
        updated_at: Some(1),
    }
}

#[test]
fn collect_discovered_sessions_concatenates_per_agent_logs() {
    let results = collect_discovered_sessions(vec![
        vec![sample_session(AgentKind::Codex, "codex")],
        vec![sample_session(AgentKind::Cursor, "cursor")],
    ]);

    let agent_types = results
        .into_iter()
        .map(|log| log.agent_type)
        .collect::<Vec<_>>();
    assert_eq!(agent_types, vec![AgentKind::Codex, AgentKind::Cursor]);
}

fn file_log(agent_type: AgentKind, path: &str) -> SessionLog {
    SessionLog {
        environment: Default::default(),
        agent_type,
        source: SessionSource::File(PathBuf::from(path)),
        updated_at: Some(0),
    }
}

fn file_log_path(agent_type: AgentKind, path: PathBuf) -> SessionLog {
    SessionLog {
        environment: Default::default(),
        agent_type,
        source: SessionSource::File(path),
        updated_at: Some(0),
    }
}

fn inline_log(agent_type: AgentKind, label: &str) -> SessionLog {
    SessionLog {
        environment: Default::default(),
        agent_type,
        source: SessionSource::Inline {
            label: label.to_string(),
            content: "{}".to_string(),
        },
        updated_at: Some(0),
    }
}

#[test]
fn inline_session_keeps_explicit_wsl_environment_in_dedupe_key() {
    let mut log = inline_log(AgentKind::Claude, "inline:session");
    log.environment = DiscoveryEnvironment::Wsl {
        distribution: "Ubuntu".to_string(),
        user: "dev".to_string(),
    };
    assert_eq!(log.environment.wsl_distro(), Some("Ubuntu"));
    assert!(log.dedupe_key().starts_with("wsl:ubuntu|"));
}

/// Synthetic home that anchors the `/Users/x/...` paths used by these
/// tests. Build IDE/desktop paths via `app_config_dir_in` so they
/// resolve to the current platform's bucket (macOS:
/// `~/Library/Application Support`, Linux: `~/.config`, Windows:
/// `%APPDATA%`) — hardcoding macOS-style paths makes the tests fail
/// on Linux CI.
fn test_home() -> PathBuf {
    PathBuf::from("/Users/x")
}

/// Build a path under the platform-correct VS Code config dir for a
/// given trailing path segment list. Use this anywhere a test needs to
/// place a file inside the IDE/desktop surface bucket.
fn vscode_config_path(home: &Path, trailing: &[&str]) -> PathBuf {
    let mut p = app_config_dir_in("Code", home);
    for seg in trailing {
        p = p.join(seg);
    }
    p
}

#[test]
fn surface_label_single_variant_agents() {
    let home = test_home();
    assert_eq!(
        file_log(
            AgentKind::OpenCode,
            "/Users/x/.local/share/opencode/storage/session/global/s.json"
        )
        .surface_label(&home),
        "cli",
    );
    assert_eq!(
        file_log(
            AgentKind::AmpCode,
            "/Users/x/.local/share/amp/threads/T-1.json"
        )
        .surface_label(&home),
        "cli",
    );
    let kiro_ide_path = app_config_dir_in("Kiro", &home)
        .join("User")
        .join("globalStorage")
        .join("kiro.kiroagent")
        .join("x.json");
    assert_eq!(
        file_log(AgentKind::Kiro, &kiro_ide_path.to_string_lossy()).surface_label(&home),
        "ide_desktop",
    );
    assert_eq!(
        file_log(
            AgentKind::Windsurf,
            "/Users/x/.codeium/windsurf/cascade/c.json"
        )
        .surface_label(&home),
        "ide_desktop",
    );
}

#[test]
fn surface_label_claude_falls_back_to_cli_when_no_marker() {
    let home = test_home();
    // Path-based: `~/.claude/projects/**` with no `entrypoint` content
    // falls back to "cli" (matches the historical default for that tree).
    assert_eq!(
        file_log(
            AgentKind::Claude,
            "/Users/x/.claude/projects/-Users-x-foo/session.jsonl"
        )
        .surface_label(&home),
        "cli",
    );
    // Path-based: Desktop manifest dir → ide_desktop regardless of content.
    let claude_desktop_path = app_config_dir_in("Claude", &home)
        .join("claude-code-sessions")
        .join("m.json");
    assert_eq!(
        file_log(AgentKind::Claude, &claude_desktop_path.to_string_lossy()).surface_label(&home),
        "ide_desktop",
    );
    // Inline label from desktop_manifest_session_log fallback → ide_desktop.
    assert_eq!(
        inline_log(
            AgentKind::Claude,
            "claude-desktop:Library/Application Support/Claude/..."
        )
        .surface_label(&home),
        "ide_desktop",
    );
}

#[test]
fn surface_label_claude_entrypoint_marker_overrides_path() {
    let home = test_home();
    let log = file_log(
        AgentKind::Claude,
        "/Users/x/.claude/projects/-Users-x-foo/s.jsonl",
    );

    // Real Claude session content: queue-operation lines first, then the
    // user message carrying `entrypoint`. Mirrors the layout observed on
    // disk so the scanner has to look past the first record.
    let vscode_content = r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-05-11T02:07:41.374Z","sessionId":"s"}
{"type":"queue-operation","operation":"dequeue","timestamp":"2026-05-11T02:07:41.382Z","sessionId":"s"}
{"type":"user","entrypoint":"claude-vscode","cwd":"/x","sessionId":"s"}"#;
    assert_eq!(
        log.surface_label_with_content(vscode_content, &home),
        "ide_desktop"
    );

    let desktop_content = r#"{"type":"user","entrypoint":"claude-desktop","sessionId":"s"}"#;
    assert_eq!(
        log.surface_label_with_content(desktop_content, &home),
        "ide_desktop"
    );

    let cli_content = r#"{"type":"user","entrypoint":"claude-cli","sessionId":"s"}"#;
    assert_eq!(log.surface_label_with_content(cli_content, &home), "cli");

    let sdk_content = r#"{"type":"user","entrypoint":"sdk-ts","sessionId":"s"}"#;
    assert_eq!(log.surface_label_with_content(sdk_content, &home), "cli");
}

#[test]
fn extract_json_string_field_happy_path() {
    let line = r#"{"type":"user","entrypoint":"claude-vscode","cwd":"/x"}"#;
    assert_eq!(
        extract_json_string_field(line, "entrypoint"),
        Some("claude-vscode")
    );
    assert_eq!(extract_json_string_field(line, "cwd"), Some("/x"));
    assert_eq!(extract_json_string_field(line, "type"), Some("user"));
}

#[test]
fn extract_json_string_field_missing_field_returns_none() {
    let line = r#"{"type":"user","cwd":"/x"}"#;
    assert_eq!(extract_json_string_field(line, "entrypoint"), None);
}

#[test]
fn extract_json_string_field_empty_value_returns_empty_str() {
    let line = r#"{"entrypoint":"","other":"x"}"#;
    assert_eq!(extract_json_string_field(line, "entrypoint"), Some(""));
}

#[test]
fn extract_json_string_field_rejects_escaped_value() {
    // Backslash in the value would otherwise produce a truncated extract.
    // Returning None lets the caller fall back to path-based classification.
    let line = r#"{"entrypoint":"claude-\"vscode","other":"x"}"#;
    assert_eq!(extract_json_string_field(line, "entrypoint"), None);
    let line2 = r#"{"entrypoint":"line\nbreak"}"#;
    assert_eq!(extract_json_string_field(line2, "entrypoint"), None);
}

#[test]
fn extract_json_string_field_handles_empty_input() {
    assert_eq!(extract_json_string_field("", "entrypoint"), None);
    assert_eq!(
        extract_json_string_field("not json at all", "entrypoint"),
        None
    );
}

#[test]
fn extract_json_string_field_returns_first_match() {
    // If the same field appears twice (unusual but possible across nested
    // objects), the substring scan returns the first occurrence — that's
    // intentional, since the per-agent classifiers expect the
    // outermost/top-level value.
    let line = r#"{"entrypoint":"first","nested":{"entrypoint":"second"}}"#;
    assert_eq!(extract_json_string_field(line, "entrypoint"), Some("first"));
}

#[test]
fn surface_label_codex_uses_session_meta_source() {
    let home = test_home();
    let log = file_log(
        AgentKind::Codex,
        "/Users/x/.codex/sessions/2026/05/25/rollout-abc.jsonl",
    );

    // Without content we can't disambiguate.
    assert_eq!(log.surface_label(&home), "unknown");

    let cli_content = r#"{"timestamp":"2026-05-25T05:39:25.515Z","type":"session_meta","payload":{"id":"abc","source":"cli","originator":"codex-tui","cwd":"/x"}}"#;
    assert_eq!(log.surface_label_with_content(cli_content, &home), "cli");

    let desktop_content = r#"{"timestamp":"2026-05-25T05:38:48.719Z","type":"session_meta","payload":{"id":"abc","source":"vscode","originator":"Codex Desktop","cwd":"/x"}}"#;
    assert_eq!(
        log.surface_label_with_content(desktop_content, &home),
        "ide_desktop"
    );
}

#[test]
fn surface_label_copilot_disambiguates_cli_vs_ide() {
    let home = test_home();
    // Copilot CLI root is platform-dependent (Windows: `<LOCALAPPDATA>/
    // GitHub Copilot CLI/session-state`; Unix: `~/.copilot/session-state`).
    // Build the test path to match the current platform's root so the
    // prefix-match in `classify_source_against_surface_paths` succeeds —
    // otherwise we'd fall back to Copilot's `unmatched_surface()` of
    // `"ide_desktop"` and the bi-modal disambiguation contract is lost.
    let copilot_cli = if cfg!(windows) {
        home.join("AppData")
            .join("Local")
            .join("GitHub Copilot CLI")
            .join("session-state")
            .join("abc")
            .join("transcript.jsonl")
    } else {
        home.join(".copilot")
            .join("session-state")
            .join("abc")
            .join("transcript.jsonl")
    };
    assert_eq!(
        file_log_path(AgentKind::Copilot, copilot_cli).surface_label(&home),
        "cli",
    );
    let copilot_ide = vscode_config_path(
        &home,
        &["User", "workspaceStorage", "x", "chatSessions", "s.json"],
    );
    assert_eq!(
        file_log_path(AgentKind::Copilot, copilot_ide).surface_label(&home),
        "ide_desktop",
    );
}

#[test]
fn surface_label_cline_disambiguates_cli_vs_ide() {
    let home = test_home();
    assert_eq!(
        file_log(AgentKind::Cline, "/Users/x/.cline/tasks/task-1/task.json").surface_label(&home),
        "cli",
    );
    let cline_ide = vscode_config_path(
        &home,
        &[
            "User",
            "globalStorage",
            "saoudrizwan.claude-dev",
            "tasks",
            "t",
            "task.json",
        ],
    );
    assert_eq!(
        file_log_path(AgentKind::Cline, cline_ide).surface_label(&home),
        "ide_desktop",
    );
}

#[test]
fn surface_label_antigravity_disambiguates_cli_vs_ide() {
    let home = test_home();
    assert_eq!(
        file_log(
            AgentKind::Antigravity,
            "/Users/x/.gemini/antigravity-cli/brain/123/transcript.jsonl"
        )
        .surface_label(&home),
        "cli",
    );
    let antigravity_ide = app_config_dir_in("Antigravity", &home)
        .join("User")
        .join("workspaceStorage")
        .join("x")
        .join("chatSessions")
        .join("s.json");
    assert_eq!(
        file_log_path(AgentKind::Antigravity, antigravity_ide).surface_label(&home),
        "ide_desktop",
    );
}

#[test]
fn surface_label_cursor_disambiguates_cli_vs_ide() {
    let home = test_home();
    // cursor-agent CLI writes under `~/.cursor/{projects,chats}/...`.
    assert_eq!(
        file_log(
            AgentKind::Cursor,
            "/Users/x/.cursor/projects/Users-x-foo/agent-transcripts/abc.json",
        )
        .surface_label(&home),
        "cli",
    );
    let cursor_ide = app_config_dir_in("Cursor", &home)
        .join("User")
        .join("workspaceStorage")
        .join("x")
        .join("chatSessions")
        .join("s.json");
    assert_eq!(
        file_log_path(AgentKind::Cursor, cursor_ide).surface_label(&home),
        "ide_desktop",
    );
}

#[test]
fn surface_label_inline_label_substring_match() {
    // Locks in the `PayloadKind::InlineLabel` contract: inline labels
    // that embed an absolute path classify by `contains` against the
    // owning agent's surface roots. Labels without an embedded surface
    // root fall through to `unmatched_surface()`.
    let home = test_home();

    // Antigravity emits `antigravity-brain:<absolute path>` for brain
    // transcripts; the embedded path sits under the CLI surface root.
    let cli_label = "antigravity-brain:/Users/x/.gemini/antigravity-cli/brain/abc/transcript.jsonl";
    assert_eq!(
        inline_log(AgentKind::Antigravity, cli_label).surface_label(&home),
        "cli",
    );

    // Same prefix, but the embedded path is under the IDE v2 brain
    // root → IDE classification.
    let ide_label = "antigravity-brain:/Users/x/.gemini/antigravity-ide/brain/xyz/transcript.jsonl";
    assert_eq!(
        inline_log(AgentKind::Antigravity, ide_label).surface_label(&home),
        "ide_desktop",
    );

    // OpenCode's `opencode:<session_id>` label has no embedded path,
    // so no surface root matches — falls through to OpenCode's
    // `unmatched_surface()` which returns "cli" (its only surface).
    assert_eq!(
        inline_log(AgentKind::OpenCode, "opencode:ses_abc123").surface_label(&home),
        "cli",
    );
}

#[test]
fn surface_label_normalizes_windows_separators() {
    // `normalize_for_matching` should fold `\` to `/` so Windows-style
    // paths classify correctly without producer-side normalization.
    // Build a synthetic Windows home and reach the IDE root through
    // `app_config_dir_for_platform` so the test is OS-independent.
    let home = PathBuf::from(r"C:\Users\x");
    let copilot_ide = app_config_dir_for_platform(
        "Code",
        &home,
        DesktopPlatform::Windows,
        Some(Path::new(r"C:\Users\x\AppData\Roaming")),
        None,
    )
    .join("User")
    .join("workspaceStorage")
    .join("ws")
    .join("chatSessions")
    .join("s.json");

    // Force a backslash-rooted absolute path into the SessionLog. The
    // classifier must normalize both sides before comparing.
    let win_path_string = copilot_ide.to_string_lossy().replace('/', "\\");
    let log = file_log(AgentKind::Copilot, &win_path_string);

    // On non-Windows hosts `current_desktop_platform()` returns macOS or
    // Linux, so `Copilot::surface_paths(home)` would produce *non-Windows*
    // IDE roots that don't match this backslash path. Drive matching
    // directly via the classifier with an explicit Windows-shaped
    // SurfacePaths instead.
    let sp = SurfacePaths {
        cli: Vec::new(),
        ide_desktop: vec![
            app_config_dir_for_platform(
                "Code",
                &home,
                DesktopPlatform::Windows,
                Some(Path::new(r"C:\Users\x\AppData\Roaming")),
                None,
            )
            .join("User")
            .join("workspaceStorage"),
        ],
        mirror: Vec::new(),
    };
    assert_eq!(
        classify_source_against_surface_paths(&log.source, &sp),
        Some("ide_desktop"),
    );
}

#[test]
fn surface_paths_each_agent_returns_expected_shape() {
    let home = PathBuf::from("/home/tester");

    // Bi-modal agents — both surfaces populated.
    for ty in [
        AgentKind::Claude,
        AgentKind::Cursor,
        AgentKind::Copilot,
        AgentKind::Cline,
        AgentKind::Antigravity,
    ] {
        let sp = Explorers::DISK.surface_paths_for(&ty, &home);
        assert!(!sp.cli.is_empty(), "{ty:?} should expose CLI roots");
        assert!(
            !sp.ide_desktop.is_empty(),
            "{ty:?} should expose IDE/Desktop roots"
        );
    }

    // CLI-only agents.
    for ty in [
        AgentKind::Codex,
        AgentKind::OpenCode,
        AgentKind::AmpCode,
        AgentKind::Pi,
    ] {
        let sp = Explorers::DISK.surface_paths_for(&ty, &home);
        assert!(!sp.cli.is_empty(), "{ty:?} should expose CLI roots");
        assert!(
            sp.ide_desktop.is_empty(),
            "{ty:?} should not expose IDE roots"
        );
    }

    // IDE-only agents.
    for ty in [AgentKind::Kiro, AgentKind::Windsurf] {
        let sp = Explorers::DISK.surface_paths_for(&ty, &home);
        assert!(sp.cli.is_empty(), "{ty:?} should not expose CLI roots");
        assert!(
            !sp.ide_desktop.is_empty(),
            "{ty:?} should expose IDE/Desktop roots"
        );
    }
}

#[test]
fn surface_paths_route_through_platform_helpers() {
    // All IDE/Desktop roots for app_config-backed agents must round-trip
    // through `app_config_dir_in` so they resolve correctly on each OS.
    let home = PathBuf::from("/home/tester");

    let claude = Explorers::DISK.surface_paths_for(&AgentKind::Claude, &home);
    let expected_claude_ide = app_config_dir_in("Claude", &home).join("claude-code-sessions");
    assert!(claude.ide_desktop.contains(&expected_claude_ide));

    let copilot = Explorers::DISK.surface_paths_for(&AgentKind::Copilot, &home);
    let expected_copilot_ide = app_config_dir_in("Code", &home)
        .join("User")
        .join("workspaceStorage");
    assert!(copilot.ide_desktop.contains(&expected_copilot_ide));

    let cline = Explorers::DISK.surface_paths_for(&AgentKind::Cline, &home);
    // Cline IDE roots cover all four VS Code-family apps.
    for app in ["Code", "Cursor", "Windsurf", "VSCodium"] {
        let root = app_config_dir_in(app, &home)
            .join("User")
            .join("globalStorage");
        assert!(
            cline.ide_desktop.iter().any(|p| p.starts_with(&root)),
            "Cline IDE roots should include something under {}",
            root.display()
        );
    }
}

#[test]
fn infer_agent_and_surface_classifies_cli_paths() {
    let home = PathBuf::from("/home/tester");

    // Claude CLI.
    assert_eq!(
        Explorers::DISK.infer_agent_and_surface(
            &home
                .join(".claude")
                .join("projects")
                .join("p1")
                .join("s.jsonl"),
            &home,
        ),
        (AgentKind::Claude, "cli"),
    );

    // Copilot CLI. Layout is platform-dependent: Windows uses
    // `<home>/AppData/Local/GitHub Copilot CLI/session-state/` when
    // `LOCALAPPDATA` is unset (it's gated to the *real* home, which the
    // synthetic `/home/tester` here isn't), Unix uses `~/.copilot/
    // session-state/`. Match what Copilot's `cli_root_candidates_for_
    // platform` would expose on this OS so the prefix-match succeeds.
    let copilot_cli = if cfg!(windows) {
        home.join("AppData")
            .join("Local")
            .join("GitHub Copilot CLI")
            .join("session-state")
            .join("abc")
            .join("t.jsonl")
    } else {
        home.join(".copilot")
            .join("session-state")
            .join("abc")
            .join("t.jsonl")
    };
    assert_eq!(
        Explorers::DISK.infer_agent_and_surface(&copilot_cli, &home),
        (AgentKind::Copilot, "cli"),
    );

    // Cline CLI.
    assert_eq!(
        Explorers::DISK.infer_agent_and_surface(
            &home
                .join(".cline")
                .join("tasks")
                .join("t1")
                .join("task.json"),
            &home,
        ),
        (AgentKind::Cline, "cli"),
    );

    // Antigravity CLI brain.
    assert_eq!(
        Explorers::DISK.infer_agent_and_surface(
            &home
                .join(".gemini")
                .join("antigravity-cli")
                .join("brain")
                .join("uuid-1")
                .join("memory.jsonl"),
            &home,
        ),
        (AgentKind::Antigravity, "cli"),
    );
}

#[test]
fn infer_agent_and_surface_classifies_ide_paths() {
    let home = PathBuf::from("/home/tester");

    // Claude IDE manifest.
    let claude_ide = app_config_dir_in("Claude", &home)
        .join("claude-code-sessions")
        .join("ws")
        .join("m.json");
    assert_eq!(
        Explorers::DISK.infer_agent_and_surface(&claude_ide, &home),
        (AgentKind::Claude, "ide_desktop"),
    );

    // Copilot VS Code workspace storage.
    let copilot_ide = app_config_dir_in("Code", &home)
        .join("User")
        .join("workspaceStorage")
        .join("ws")
        .join("chatSessions")
        .join("s.json");
    assert_eq!(
        Explorers::DISK.infer_agent_and_surface(&copilot_ide, &home),
        (AgentKind::Copilot, "ide_desktop"),
    );

    // Kiro IDE-only.
    let kiro_ide = app_config_dir_in("Kiro", &home)
        .join("User")
        .join("globalStorage")
        .join("kiro.kiroagent")
        .join("workspace-sessions")
        .join("ws-1")
        .join("s.json");
    assert_eq!(
        Explorers::DISK.infer_agent_and_surface(&kiro_ide, &home),
        (AgentKind::Kiro, "ide_desktop"),
    );

    // Windsurf IDE-only.
    let windsurf_ide = app_config_dir_in("Windsurf", &home)
        .join("User")
        .join("workspaceStorage")
        .join("ws")
        .join("chatSessions")
        .join("s.json");
    assert_eq!(
        Explorers::DISK.infer_agent_and_surface(&windsurf_ide, &home),
        (AgentKind::Windsurf, "ide_desktop"),
    );
}

#[test]
fn infer_agent_and_surface_codex_falls_back_to_unknown_surface() {
    // Codex CLI and the IDE extension share `~/.codex/sessions/`, so
    // `surface_paths` exposes the root only under `cli`. A path that
    // hits the shared tree returns (Codex, "cli") for an exact root
    // match; the fallback contract is preserved for paths *outside*
    // any registered root.
    let home = PathBuf::from("/home/tester");

    let codex_path = home
        .join(".codex")
        .join("sessions")
        .join("2026")
        .join("05")
        .join("25")
        .join("rollout.jsonl");
    // The path is under the CLI root, so the surface is "cli" — content
    // is required to flip to "ide_desktop". This locks in the design:
    // path inference alone never returns "ide_desktop" for Codex.
    assert_eq!(
        Explorers::DISK.infer_agent_and_surface(&codex_path, &home),
        (AgentKind::Codex, "cli"),
    );

    // Path under no registered root → falls back to `infer_agent_type`
    // with surface "unknown".
    let unknown = PathBuf::from("/var/tmp/some-random.jsonl");
    let (agent, surface) = Explorers::DISK.infer_agent_and_surface(&unknown, &home);
    assert_eq!(agent, AgentKind::Claude); // fallback default
    assert_eq!(surface, "unknown");
}

#[test]
fn disk_adapters_declare_no_mirror_roots() {
    // Mirrors are supplied by the embedding application, never by the engine.
    let home = PathBuf::from("/home/tester");
    for ty in AgentKind::ALL {
        let sp = Explorers::DISK.surface_paths_for(ty, &home);
        assert!(
            sp.mirror.is_empty(),
            "{ty:?} must not declare a mirror root by default",
        );
    }
}

/// A configured mirror is claimed by `owns_path`, surfaces as a `mirror` root,
/// and classifies to the same label the adapter's `unmatched_surface` returns —
/// so registering a mirror never changes an existing session's surface.
#[test]
fn configured_mirror_is_owned_and_classified_as_ide_desktop() {
    fn mirror_dir(home: &Path) -> Option<PathBuf> {
        Some(home.join("mirrors").join("antigravity-api"))
    }
    const MIRROR: SessionMirror = SessionMirror {
        dir: mirror_dir,
        path_marker: Some("/antigravity-api/"),
    };
    static MIRRORED: agents::antigravity::AntigravityExplorer =
        agents::antigravity::AntigravityExplorer {
            mirror: MIRROR,
            spawn_edges_path: |_| None,
        };

    let home = PathBuf::from("/home/tester");
    let sp = MIRRORED.surface_paths(&home);
    assert_eq!(
        sp.mirror,
        vec![home.join("mirrors").join("antigravity-api")]
    );
    assert!(MIRRORED.owns_path("/home/tester/mirrors/antigravity-api/abc.json"));
    assert_eq!(
        classify_path_against_surface_paths(
            &home.join("mirrors").join("antigravity-api").join("a.json"),
            &sp
        ),
        Some("ide_desktop"),
    );
    assert_eq!(MIRRORED.unmatched_surface(), "ide_desktop");
}

#[test]
fn matcher_precedence_covers_every_agent_exactly_once() {
    // Sanity: precedence must enumerate every agent so the inference
    // dispatcher doesn't silently skip one.
    use std::collections::BTreeSet;
    let slugs: BTreeSet<&str> = MATCHER_PRECEDENCE.iter().map(|ty| ty.slug()).collect();
    assert_eq!(
        slugs.len(),
        AgentKind::ALL.len(),
        "MATCHER_PRECEDENCE must list every agent exactly once: got {slugs:?}",
    );
}

/// A real sub-agent tree on disk is surfaced for the vendor whose explorer
/// overrides the orchestration hooks (Claude) and ignored for one that
/// doesn't (Codex), purely via the default trait impl — no `matches!` gate
/// in the dispatcher. This is the seam that lets a future vendor opt in by
/// implementing the hooks on its own explorer alone.
#[tokio::test]
async fn subagent_dispatch_is_vendor_specific_via_trait() {
    let home = TempDir::new().unwrap();
    let project = home.path().join("-Users-foo-bar");
    let subs = project.join("sess-1").join("subagents");
    tokio::fs::create_dir_all(&subs).await.unwrap();
    tokio::fs::write(subs.join("agent-aaa.jsonl"), "{}")
        .await
        .unwrap();
    tokio::fs::write(subs.join("agent-bbb.jsonl"), "{}")
        .await
        .unwrap();
    let parent = project.join("sess-1.jsonl");

    // Claude overrides the hooks → finds the tree.
    let claude_subs = Explorers::DISK
        .list_subagents_for_transcript(&AgentKind::Claude, &parent)
        .await;
    assert_eq!(claude_subs.len(), 2);
    let first = &claude_subs[0];
    assert!(
        Explorers::DISK
            .subagent_id(&AgentKind::Claude, first)
            .is_some()
    );

    // Codex inherits the default no-op hooks → empty even though the
    // identical tree exists on disk. Proves dispatch is trait-based.
    assert!(
        Explorers::DISK
            .list_subagents_for_transcript(&AgentKind::Codex, &parent)
            .await
            .is_empty()
    );
    assert!(
        Explorers::DISK
            .subagent_id(&AgentKind::Codex, first)
            .is_none()
    );
    assert_eq!(
        Explorers::DISK
            .subagent_label(&AgentKind::Codex, first)
            .await,
        "Sub-agent"
    );
}
