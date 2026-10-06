use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use antiburn_remote::{Hello, PROTOCOL_VERSION, export::BundleManifest};
use serde_json::{Value, json};

fn call(home: &std::path::Path, request: Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_antiburn-remote"))
        .arg("stdio")
        .env("HOME", home)
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn call_with_timeout(home: &std::path::Path, request: Value, timeout: Duration) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_antiburn-remote"))
        .arg("stdio")
        .env("HOME", home)
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "helper exceeded {:?}: {}",
                timeout,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn hello_and_version_are_read_only_prerequisites() {
    let home = tempfile::tempdir().unwrap();
    let result = call(home.path(), json!({"operation":"hello","version":999}));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let hello: Hello = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(hello.version, PROTOCOL_VERSION);
    assert_eq!(hello.supported_agents, ["claude-code", "codex"]);

    let version = Command::new(env!("CARGO_BIN_EXE_antiburn-remote"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap(),
        format!("antiburn-remote {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn discovers_and_analyzes_synthetic_codex_without_exporting_messages() {
    let home = tempfile::tempdir().unwrap();
    let today = time::OffsetDateTime::now_utc();
    let dir = home.path().join(format!(
        ".codex/sessions/{}/{:02}/{:02}",
        today.year(),
        today.month() as u8,
        today.day()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = include_str!(
        "../../antiburn-local/tests/fixtures/codex_characterization/task_complete_errors.jsonl"
    );
    std::fs::write(dir.join("rollout-synthetic.jsonl"), fixture).unwrap();
    std::fs::write(
        home.path().join(".codex/session_index.jsonl"),
        r#"{"id":"synthetic-task-complete-errors","thread_name":"Synthetic saved task title"}"#,
    )
    .unwrap();

    let result = call(
        home.path(),
        json!({"operation":"list","version":PROTOCOL_VERSION}),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let snapshot: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(snapshot["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        snapshot["sessions"][0]["title"],
        "Synthetic saved task title"
    );
    let id = snapshot["sessions"][0]["sessionId"].as_str().unwrap();
    let result = call(
        home.path(),
        json!({"operation":"analyze","version":PROTOCOL_VERSION,"agent":"codex","session_id":id}),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let analysis: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(analysis["session"]["sessionId"], id);
    assert!(analysis["metrics"]["tokensOut"].as_u64().unwrap() > 0);
    assert!(!String::from_utf8_lossy(&result.stdout).contains("Synthetic check passed."));
}

#[test]
fn exports_codex_forks_without_a_separate_parent_companion() {
    let home = tempfile::tempdir().unwrap();
    let today = time::OffsetDateTime::now_utc();
    let dir = home.path().join(format!(
        ".codex/sessions/{}/{:02}/{:02}",
        today.year(),
        today.month() as u8,
        today.day()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = include_str!(
        "../../antiburn-local/tests/fixtures/codex_characterization/reverted_fork.jsonl"
    );
    std::fs::write(dir.join("rollout-forked.jsonl"), fixture).unwrap();

    let result = call(
        home.path(),
        json!({"operation":"export","version":PROTOCOL_VERSION,"agent":"codex","session_id":"child","known":null}),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(&result.stdout[..8], b"ABR2DATA");
    let length = u32::from_le_bytes(result.stdout[8..12].try_into().unwrap()) as usize;
    let manifest: BundleManifest = serde_json::from_slice(&result.stdout[12..12 + length]).unwrap();
    manifest.validate().unwrap();
    assert_eq!(manifest.session.session_id, "child");
    assert_eq!(manifest.fork_parent_session_id, None);
    assert!(manifest.files.iter().all(|file| !file.fork_parent));
}

#[test]
fn malformed_or_oversized_requests_produce_no_protocol_output() {
    let home = tempfile::tempdir().unwrap();
    let mismatch = call(home.path(), json!({"operation":"list","version":999}));
    assert!(!mismatch.status.success());
    assert!(mismatch.stdout.is_empty());

    let traversal = call(
        home.path(),
        json!({"operation":"export","version":PROTOCOL_VERSION,"agent":"claude-code","session_id":"../../secret","known":null}),
    );
    assert!(!traversal.status.success());
    assert!(traversal.stdout.is_empty());

    let oversized = call(
        home.path(),
        json!({"operation":"analyze","version":PROTOCOL_VERSION,"agent":"codex","session_id":"x".repeat(9 * 1024)}),
    );
    assert!(!oversized.status.success());
    assert!(oversized.stdout.is_empty());
    assert!(String::from_utf8_lossy(&oversized.stderr).contains("Request exceeds 8 KiB"));
}

#[test]
fn exports_claude_companions_fork_parent_and_only_skips_unchanged_bundles() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join(".claude/projects/synthetic");
    let children = project.join("forked/subagents");
    std::fs::create_dir_all(&children).unwrap();
    let parent = r#"{"type":"user","sessionId":"ancestor","cwd":"/synthetic/project","message":{"role":"user","content":"Original task"}}
{"type":"assistant","sessionId":"ancestor","message":{"role":"assistant","model":"claude-sonnet-4-6","content":[{"type":"text","text":"Original answer"}],"usage":{"input_tokens":10,"output_tokens":2}}}
"#;
    let forked = r#"{"type":"session_meta","metadata":{"local_fork_observation":{"parent_agent":"claude-code","parent_agent_session_id":"ancestor","fork_kind":"fork","provider_fork_point_id":null,"detection_source":"synthetic","confidence":100,"inherited_item_count":1,"extractor_version":"1"}}}
{"type":"user","sessionId":"forked","cwd":"/synthetic/project","message":{"role":"user","content":"Forked task"}}
{"type":"assistant","sessionId":"forked","message":{"role":"assistant","model":"claude-sonnet-4-6","content":[{"type":"text","text":"Forked answer"}],"usage":{"input_tokens":10,"output_tokens":2}}}
"#;
    std::fs::write(project.join("ancestor.jsonl"), parent).unwrap();
    std::fs::write(project.join("forked.jsonl"), forked).unwrap();
    std::fs::write(
        children.join("agent-child.jsonl"),
        forked.replace("forked", "child"),
    )
    .unwrap();
    std::fs::write(
        children.join("agent-child.meta.json"),
        r#"{"toolUseId":"synthetic-tool"}"#,
    )
    .unwrap();

    let request = json!({"operation":"export","version":PROTOCOL_VERSION,"agent":"claude-code","session_id":"forked","known":null});
    let first = call(home.path(), request.clone());
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(&first.stdout[..8], b"ABR2DATA");
    let length = u32::from_le_bytes(first.stdout[8..12].try_into().unwrap()) as usize;
    let manifest: BundleManifest = serde_json::from_slice(&first.stdout[12..12 + length]).unwrap();
    manifest.validate().unwrap();
    assert_eq!(manifest.fork_parent_session_id.as_deref(), Some("ancestor"));
    assert!(manifest.files.iter().any(|file| file.fork_parent));
    assert!(
        manifest
            .files
            .iter()
            .any(|file| file.subagent_id.as_deref() == Some("agent-child"))
    );
    assert_eq!(
        first.stdout.len(),
        12 + length
            + manifest
                .files
                .iter()
                .map(|file| file.size as usize)
                .sum::<usize>()
    );

    let mut known = request;
    known["known"] = json!(manifest.signature().unwrap());
    let second = call(home.path(), known.clone());
    assert!(second.status.success());
    assert_eq!(&second.stdout[..8], b"ABR2SAME");
    assert_eq!(second.stdout.len(), 12 + length);

    std::fs::write(
        children.join("agent-child.meta.json"),
        r#"{"toolUseId":"changed-tool","agentType":"Explore"}"#,
    )
    .unwrap();
    let changed = call(home.path(), known);
    assert!(changed.status.success());
    assert_eq!(&changed.stdout[..8], b"ABR2DATA");
}

#[cfg(unix)]
#[test]
fn list_excludes_leaf_and_intermediate_symlinks_without_disclosing_markers() {
    use std::os::unix::fs::symlink;

    let home = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let sessions = home.path().join(".codex/sessions/2026/09/28");
    std::fs::create_dir_all(&sessions).unwrap();
    let marker = "outside-private-title-marker";
    let transcript = format!(
        r#"{{"type":"session_meta","payload":{{"id":"outside","cwd":"/outside"}}}}
{{"type":"event_msg","payload":{{"type":"user_message","message":"{marker}"}}}}
"#
    );
    std::fs::write(outside.path().join("outside.jsonl"), transcript).unwrap();
    std::fs::write(
        sessions.join("inside.jsonl"),
        r#"{"type":"session_meta","payload":{"id":"inside","cwd":"/inside","source":"cli"}}
{"type":"event_msg","payload":{"type":"user_message","message":"Inside task"}}
"#,
    )
    .unwrap();
    std::fs::write(
        outside.path().join("session_index.jsonl"),
        format!(r#"{{"id":"inside","thread_name":"{marker}"}}"#),
    )
    .unwrap();
    symlink(
        outside.path().join("session_index.jsonl"),
        home.path().join(".codex/session_index.jsonl"),
    )
    .unwrap();
    symlink(
        outside.path().join("outside.jsonl"),
        sessions.join("leaf.jsonl"),
    )
    .unwrap();
    symlink(outside.path(), sessions.join("linked-directory")).unwrap();

    let result = call(
        home.path(),
        json!({"operation":"list","version":PROTOCOL_VERSION}),
    );
    assert!(result.status.success());
    assert!(!String::from_utf8_lossy(&result.stdout).contains(marker));
    let snapshot: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(snapshot["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["sessions"][0]["sessionId"], "inside");
}

#[cfg(unix)]
#[test]
fn export_rejects_symlinked_sidecar_with_typed_exit_and_no_protocol_output() {
    use std::os::unix::fs::symlink;

    let home = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), r#"{"private":"outside-marker"}"#).unwrap();
    let project = home.path().join(".claude/projects/synthetic");
    let children = project.join("parent/subagents");
    std::fs::create_dir_all(&children).unwrap();
    std::fs::write(
        project.join("parent.jsonl"),
        r#"{"type":"user","sessionId":"parent","message":{"role":"user","content":"Task"}}
{"type":"assistant","sessionId":"parent","message":{"role":"assistant","content":"Done"}}
"#,
    )
    .unwrap();
    std::fs::write(
        children.join("agent-child.jsonl"),
        r#"{"type":"user","sessionId":"child","message":{"role":"user","content":"Child"}}
"#,
    )
    .unwrap();
    symlink(outside.path(), children.join("agent-child.meta.json")).unwrap();

    let result = call(
        home.path(),
        json!({"operation":"export","version":PROTOCOL_VERSION,"agent":"claude-code","session_id":"parent","known":null}),
    );
    assert_eq!(result.status.code(), Some(65));
    assert!(result.stdout.is_empty());

    let invalid = call(
        home.path(),
        json!({"operation":"export","version":999,"agent":"claude-code","session_id":"parent","known":null}),
    );
    assert_ne!(invalid.status.code(), Some(65));
}

#[test]
fn companion_label_uses_only_its_bounded_prefix() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join(".claude/projects/synthetic");
    let children = project.join("parent/subagents");
    std::fs::create_dir_all(&children).unwrap();
    std::fs::write(
        project.join("parent.jsonl"),
        r#"{"type":"user","sessionId":"parent","message":{"role":"user","content":"Task"}}
{"type":"assistant","sessionId":"parent","message":{"role":"assistant","content":"Done"}}
"#,
    )
    .unwrap();
    let mut child = vec![b'x'; 70 * 1024];
    child.extend_from_slice(
        br#"
{"type":"user","sessionId":"child","message":{"role":"user","content":"unbounded-private-title-marker"}}
"#,
    );
    std::fs::write(children.join("agent-child.jsonl"), child).unwrap();

    let result = call(
        home.path(),
        json!({"operation":"export","version":PROTOCOL_VERSION,"agent":"claude-code","session_id":"parent","known":null}),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let length = u32::from_le_bytes(result.stdout[8..12].try_into().unwrap()) as usize;
    let manifest: BundleManifest = serde_json::from_slice(&result.stdout[12..12 + length]).unwrap();
    assert_eq!(manifest.files[1].label.as_deref(), Some("Sub-agent"));
}

#[cfg(unix)]
#[test]
fn fifo_and_jsonl_directory_are_skipped_without_blocking() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join(".codex/sessions/2026/09/28");
    std::fs::create_dir_all(sessions.join("directory.jsonl")).unwrap();
    let fifo = sessions.join("pipe.jsonl");
    let fifo_name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    let result = unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) };
    assert_eq!(result, 0, "{}", std::io::Error::last_os_error());

    let output = call_with_timeout(
        home.path(),
        json!({"operation":"list","version":PROTOCOL_VERSION}),
        Duration::from_secs(2),
    );

    assert!(output.status.success());
    let snapshot: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(snapshot["sessions"].as_array().unwrap().is_empty());
}

#[test]
fn export_rejects_file_with_a_different_session_identity() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join(".claude/projects/synthetic");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("requested.jsonl"),
        r#"{"type":"user","sessionId":"different","message":{"role":"user","content":"Private marker"}}
"#,
    )
    .unwrap();

    let output = call(
        home.path(),
        json!({"operation":"export","version":PROTOCOL_VERSION,"agent":"claude-code","session_id":"requested","known":null}),
    );

    assert_eq!(output.status.code(), Some(65));
    assert!(output.stdout.is_empty());
}

#[test]
fn cumulative_preview_budget_truncates_the_real_list_operation() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join(".codex/sessions/2026/09/28");
    std::fs::create_dir_all(&sessions).unwrap();
    for index in 0..65 {
        let path = sessions.join(format!("session-{index:03}.jsonl"));
        std::fs::write(
            &path,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"session-{index:03}\",\"source\":\"cli\"}}}}\n"
            ),
        )
        .unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_len(1024 * 1024)
            .unwrap();
    }

    let output = call(
        home.path(),
        json!({"operation":"list","version":PROTOCOL_VERSION}),
    );

    assert!(output.status.success());
    let snapshot: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(snapshot["truncated"], true);
    assert_eq!(snapshot["sessions"].as_array().unwrap().len(), 64);
}

#[test]
fn invalid_candidate_budget_truncates_the_real_list_operation() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join(".codex/sessions/2026/09/28");
    std::fs::create_dir_all(&sessions).unwrap();
    for index in 0..=10_000 {
        std::fs::write(sessions.join(format!("invalid-{index:05}.jsonl")), b"").unwrap();
    }

    let output = call(
        home.path(),
        json!({"operation":"list","version":PROTOCOL_VERSION}),
    );

    assert!(output.status.success());
    let snapshot: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(snapshot["truncated"], true);
    assert!(snapshot["sessions"].as_array().unwrap().is_empty());
}

#[test]
fn title_index_limit_is_reported_without_unbounded_title_fallback() {
    use std::io::{Seek, SeekFrom};

    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join(".codex/sessions/2026/09/28");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("inside.jsonl"),
        r#"{"type":"session_meta","payload":{"id":"inside","source":"cli"}}
"#,
    )
    .unwrap();
    let mut index = std::fs::File::create(home.path().join(".codex/session_index.jsonl")).unwrap();
    index.set_len(4 * 1024 * 1024 + 256).unwrap();
    index.seek(SeekFrom::End(-128)).unwrap();
    index
        .write_all(
            br#"{"id":"inside","thread_name":"outside-prefix-title"}
"#,
        )
        .unwrap();

    let output = call(
        home.path(),
        json!({"operation":"list","version":PROTOCOL_VERSION}),
    );

    assert!(output.status.success());
    let snapshot: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(snapshot["truncated"], true);
    assert_eq!(snapshot["sessions"][0]["title"], "Untitled session");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("outside-prefix-title"));
}
